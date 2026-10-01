'use strict';
// Loads one of the app's pages into a node `vm` context the way WebView2 loads it: the page's
// own HTML, its scripts run in document order in one global scope, a fake `window.__TAURI__`
// standing in for the app, and a clock the test moves by hand.
//
// Nothing here is the app: the bridge answers `an_call` from the ui-contract fixtures (the same
// files the sealed hub serves), records every call, and holds each one against the contract
// (lib/contract.cjs), so a page that asks for something the engine would refuse fails its test.

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const dom = require('./dom.cjs');
const contract = require('./contract.cjs');

const UI = path.join(contract.WINDOWS, 'codenotch', 'ui');
/** `generated_at_ms` of the fixtures: the tests' "now" unless one says otherwise. */
const NOW = 1790000000000;

// ---- the clock ---------------------------------------------------------------------------------

function createClock(start) {
  let now = start;
  let nextId = 1;
  const timers = new Map();
  const clock = {
    now: () => now,
    setTimeout(fn, delay, ...args) {
      const id = nextId++;
      timers.set(id, { at: now + Math.max(0, Number(delay) || 0), fn, args, every: null });
      return id;
    },
    setInterval(fn, delay, ...args) {
      const id = nextId++;
      const every = Math.max(1, Number(delay) || 0);
      timers.set(id, { at: now + every, fn, args, every });
      return id;
    },
    clear(id) {
      timers.delete(id);
    },
    /** Moves time on by `ms`, running every timer that falls due, in order. */
    tick(ms) {
      const until = now + ms;
      for (;;) {
        let due = null;
        let dueId = null;
        for (const [id, t] of timers) {
          if (t.at <= until && (due === null || t.at < due.at || (t.at === due.at && id < dueId))) {
            due = t;
            dueId = id;
          }
        }
        if (!due) break;
        now = Math.max(now, due.at);
        if (due.every) due.at += due.every;
        else timers.delete(dueId);
        due.fn(...due.args);
      }
      now = until;
    },
    pending: () => timers.size,
    /** The delays (from now) of the waiting one-shot timers, soonest first. */
    delays: () => [...timers.values()].filter((t) => !t.every).map((t) => t.at - now).sort((a, b) => a - b),
  };
  return clock;
}

// ---- the app, faked ------------------------------------------------------------------------------

const ABSENT = () => ({ status: 'absent', windows: [], fetched_at: 0, note: '' });

/** What upstream's own commands answer on a quiet PC (the pages ask these while they load). */
function upstreamCommands(options) {
  const flags = { notch_visible: true, notch_on_hover: !!options.onHover, tray_visible: true };
  return {
    get_theme_resolved: () => options.theme || 'dark',
    get_theme: () => 'system',
    get_notch_insets: () => [0, 0, 0, 0],
    get_state: () => ({ sessions: [], agg: 'idle', lang_resolved: 'en', clock_24h: false }),
    get_usage: () => options.usage || { status: 'none', windows: [], fetched_at: 0, note: '', backoff_until: 0 },
    get_codex: ABSENT,
    get_cursor: ABSENT,
    get_grok: ABSENT,
    get_antigravity: ABSENT,
    get_glm: ABSENT,
    get_notch_slots: () => [],
    get_antigravity_prefs: () => ({ limit: 'automatic', model: 'gemini' }),
    get_weekly_ring: () => options.weeklyRing || 'outside',
    get_color_transition: () => 'hard_step',
    get_activity: () => [],
    get_glyphs: () => ({}),
    get_notch_edge: () => options.edge || 'right',
    get_ui_flags: () => flags,
    set_ui_flags: () => flags,
    get_move_handle: () => true,
    get_claude_auth: () => ({ busy: false, message: '' }),
    set_hot: () => null,
    report_dpr: () => null,
    log_js: () => null,
    notch_hidden: () => null,
    refresh_ring: () => false,
    show_notch_menu: () => null,
    open_settings: () => null,
    drag_begin: () => null,
    begin_move: () => null,
    // Settings
    get_system_look: () => ({ mica: false, accent: null }),
    get_tray_options: () => [
      { id: 'claude', label: 'Claude', status: 'ok', used: 0.34 },
      { id: 'codex', label: 'Codex', status: 'absent', used: 0 },
    ],
    get_scale: () => 1,
    get_monitors: () => [],
    get_lang: () => 'auto',
    get_lang_resolved: () => options.lang || 'en',
    get_autostart: () => false,
    get_hooks_installed: () => false,
    get_update_state: () => ({}),
    set_lang: () => null,
  };
}

function clone(value) {
  return value === undefined ? undefined : JSON.parse(JSON.stringify(value));
}

/**
 * What upstream's commands answer, as plain data: `{command: reply}`. The snapshot tool's fake
 * bridge (tools/render-scenes.cjs) serves these in a real browser, so both fakes agree.
 */
function upstreamAnswers(options) {
  const answers = {};
  for (const [command, handler] of Object.entries(upstreamCommands(Object.assign({}, options)))) {
    const value = clone(handler({}));
    if (value !== undefined) answers[command] = value;
  }
  return answers;
}

/**
 * The fork's side of the app: `an_call` answered from the fixtures. `replies[method]` (a value
 * or a function of the arguments) overrides an answer; a function may throw `{code, message}`.
 */
function createHub(options) {
  const calls = contract.fixture('calls.json');
  const hub = {
    snapshot: options.snapshot === undefined ? contract.fixture('snapshot.json') : options.snapshot,
    settings: options.settings === undefined ? contract.fixture('settings.json') : options.settings,
    chat: options.chat === undefined ? contract.fixture('chat.json') : options.chat,
    replies: Object.assign({}, options.replies),
    /** Every `an_call`, in order: `{method, args}`. */
    calls: [],
    /** Calls the contract or the window gate would refuse. A test asserts this stays empty. */
    violations: [],
    of(method) {
      return hub.calls.filter((c) => c.method === method);
    },
    last(method) {
      const all = hub.of(method);
      return all[all.length - 1] || null;
    },
    clear() {
      hub.calls.length = 0;
    },
  };

  function fromFixture(method, args) {
    const wanted = JSON.stringify(args === null || args === undefined ? {} : args);
    for (const entry of calls) {
      if (entry.call.method !== method) continue;
      if (JSON.stringify(entry.call.args === undefined ? {} : entry.call.args) !== wanted) continue;
      if (entry.error) return { error: entry.error };
      if (entry.reply !== undefined) return { reply: clone(entry.reply) };
    }
    return null;
  }

  const defaults = {
    snapshot: () => clone(hub.snapshot),
    settings: () => clone(hub.settings),
    answer: () => ({ result: 'delivered' }),
    send_message: () => ({ outcome: 'delivered' }),
    message_route: () => ({ available: true }),
    focus: () => ({ outcome: 'focused' }),
    refresh_usage: () => ({ coming: false }),
    chat_image: () => ({ data_url: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==' }),
    remove_codenotch_hooks: () => ({ removed: 1 }),
    choose_claude_binary: () => ({ version: '2.1.282' }),
    cloud_url: (args) => ({ url: `https://agentnotch.rivant.in/${args.target}` }),
    session_state_text: () => ({ text: 'Fix the login redirect loop [needs you] acme-web tasks 3/7' }),
    launch_command: () => ({ command: "$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-work'; claude" }),
    reveal_target: () => ({ path: 'C:\\Users\\me\\.claude' }),
    pick_folder: () => ({ path: null }),
  };

  hub.handle = function handle(label, method, args) {
    hub.calls.push({ method, args: clone(args) });
    const problem = contract.callProblem(method, args);
    if (problem) hub.violations.push(`${label}: ${problem}`);
    if (!contract.allowedFromWindow(label, method)) {
      hub.violations.push(`${label}: ${method} isn't available to this window (§3.7)`);
      return Promise.reject({ code: 'refused', message: `${method} isn't available to this window` });
    }
    try {
      if (Object.prototype.hasOwnProperty.call(hub.replies, method)) {
        const reply = hub.replies[method];
        return Promise.resolve(typeof reply === 'function' ? reply(args || {}, hub) : clone(reply));
      }
      if (method !== 'snapshot' && method !== 'settings') {
        const fixed = fromFixture(method, args);
        if (fixed && fixed.error) return Promise.reject({ code: fixed.error, message: `Sealed: ${method} does nothing here.` });
        if (fixed) return Promise.resolve(fixed.reply);
      }
      if (defaults[method]) return Promise.resolve(defaults[method](args || {}));
      return Promise.resolve({});
    } catch (error) {
      return Promise.reject(error);
    }
  };
  return hub;
}

// ---- a page ----------------------------------------------------------------------------------------

const LABELS = { 'notch.html': 'notch', 'settings.html': 'settings', 'agentnotch/panel.html': 'agentnotch-panel' };

/**
 * Loads `file` (relative to codenotch/ui). Options:
 *   now            the clock's start (default: the fixtures' generated_at_ms)
 *   snapshot, settings, chat, replies   the hub's data (see createHub); `snapshot: null` answers
 *                  the snapshot call with an error, as a hub that isn't running does
 *   commands       extra or replaced upstream commands
 *   edge, theme, lang, onHover, weeklyRing, usage   what upstream's commands report
 *   lateScripts    run scripts a script inserts only after DOMContentLoaded (both orders occur
 *                  in a browser)
 *   missing        file names (relative to the page) that fail to load
 *   bridge         false: no `window.__TAURI__` at all
 *   before         (window) => void, run before the first script (an initialization script)
 *   reducedMotion  what `(prefers-reduced-motion: reduce)` answers
 */
function loadPage(file, options) {
  const opts = Object.assign({}, options);
  const pagePath = path.join(UI, file);
  const pageDir = path.dirname(pagePath);
  const html = fs.readFileSync(pagePath, 'utf8');
  const document = dom.parseDocument(html);
  const clock = createClock(opts.now === undefined ? NOW : opts.now);
  const label = opts.label || LABELS[file] || 'unknown';
  const hub = createHub(opts);
  if (opts.snapshot === null) {
    hub.replies.snapshot = () => {
      throw { code: 'failed', message: "Agent Notch's Claude Code control isn't running" };
    };
  }
  const commands = Object.assign(upstreamCommands(opts), opts.commands);
  const errors = [];
  const logs = [];
  const listeners = new Map();
  const upstreamCalls = [];

  const windowTarget = dom.parseFragment('');
  const sandbox = {};
  const context = vm.createContext(sandbox);
  const window = vm.runInContext('this', context);
  windowTarget.__window = window;
  document.defaultView = { __eventTarget: windowTarget };

  const media = { '(prefers-reduced-motion: reduce)': !!opts.reducedMotion };
  const storage = () => {
    const map = new Map();
    return {
      getItem: (k) => (map.has(String(k)) ? map.get(String(k)) : null),
      setItem: (k, v) => map.set(String(k), String(v)),
      removeItem: (k) => map.delete(String(k)),
      clear: () => map.clear(),
      get length() {
        return map.size;
      },
    };
  };
  const clipboard = { text: null, writeText: (t) => { clipboard.text = String(t); return Promise.resolve(); } };

  const tauri = {
    core: {
      invoke(cmd, args) {
        if (cmd === 'an_call') {
          const a = args || {};
          return hub.handle(label, a.method, a.args === undefined ? null : a.args);
        }
        upstreamCalls.push({ cmd, args: clone(args) });
        const handler = commands[cmd];
        if (!handler) return Promise.reject(new Error(`no such command: ${cmd}`));
        try {
          return Promise.resolve(handler(args || {}));
        } catch (error) {
          return Promise.reject(error);
        }
      },
    },
    event: {
      listen(name, handler) {
        if (!listeners.has(name)) listeners.set(name, new Set());
        listeners.get(name).add(handler);
        return Promise.resolve(() => listeners.get(name).delete(handler));
      },
    },
    app: { getVersion: () => Promise.resolve('1.0.0') },
    window: { getCurrentWindow: () => ({ close: () => Promise.resolve(), label }) },
  };

  Object.assign(window, {
    window,
    self: window,
    document,
    navigator: { language: 'en-US', userAgent: 'agentnotch-ui-tests', clipboard },
    location: { href: `https://tauri.localhost/${file}`, origin: 'https://tauri.localhost', protocol: 'https:', hash: '', search: '' },
    localStorage: storage(),
    sessionStorage: storage(),
    console: {
      log: (...a) => logs.push(a.join(' ')),
      info: (...a) => logs.push(a.join(' ')),
      warn: (...a) => logs.push(a.join(' ')),
      error: (...a) => errors.push(new Error(`console.error: ${a.map(String).join(' ')}`)),
      debug: () => {},
    },
    setTimeout: (fn, ms, ...args) => clock.setTimeout(guard(fn), ms, ...args),
    clearTimeout: (id) => clock.clear(id),
    setInterval: (fn, ms, ...args) => clock.setInterval(guard(fn), ms, ...args),
    clearInterval: (id) => clock.clear(id),
    requestAnimationFrame: (fn) => clock.setTimeout(guard(fn), 16, clock.now()),
    cancelAnimationFrame: (id) => clock.clear(id),
    queueMicrotask,
    structuredClone: clone,
    performance: { now: () => clock.now() - (opts.now === undefined ? NOW : opts.now) },
    innerWidth: opts.width || 360,
    innerHeight: opts.height || 650,
    devicePixelRatio: opts.dpr || 1,
    getComputedStyle: (el) => ({
      getPropertyValue: (name) => (el && el.style ? el.style.getPropertyValue(name) : '') || (opts.css && opts.css[name]) || '',
    }),
    matchMedia: (query) => ({
      matches: !!media[query], media: query,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {},
    }),
    addEventListener: (type, listener, o) => windowTarget.addEventListener(type, listener, o),
    removeEventListener: (type, listener) => windowTarget.removeEventListener(type, listener),
    dispatchEvent: (event) => windowTarget.dispatchEvent(event),
    MutationObserver: dom.makeMutationObserver(document),
    NodeFilter: { SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_TEXT: 4, SHOW_COMMENT: 128 },
    Event: dom.Event,
    CustomEvent: dom.Event,
    KeyboardEvent: dom.Event,
    MouseEvent: dom.Event,
    ResizeObserver: class ResizeObserver {
      constructor(callback) {
        this.callback = callback;
        page.resizeObservers.push(this);
      }

      observe() {}

      unobserve() {}

      disconnect() {}
    },
    IntersectionObserver: class IntersectionObserver {
      constructor(callback) {
        this.callback = callback;
        this.targets = new Set();
        page.intersectionObservers.push(this);
      }

      observe(el) {
        this.targets.add(el);
      }

      unobserve(el) {
        this.targets.delete(el);
      }

      disconnect() {
        this.targets.clear();
      }
    },
    getSelection: () => ({ toString: () => '', isCollapsed: true }),
    focus() {},
    blur() {},
  });
  if (opts.bridge !== false) window.__TAURI__ = tauri;
  // `Date.now()` is the test's clock; `new Date(ms)` formats as usual.
  vm.runInContext('Date.now = function () { return __now(); };', Object.assign(context, { __now: () => clock.now() }));

  function guard(fn) {
    return (...args) => {
      try {
        return fn(...args);
      } catch (error) {
        errors.push(error);
        return undefined;
      }
    };
  }

  const pendingScripts = [];
  function runScript(el, source, filename, lineOffset) {
    const previous = document.currentScript;
    document.currentScript = el;
    try {
      vm.runInContext(source, context, { filename, lineOffset: lineOffset || 0 });
      return true;
    } catch (error) {
      errors.push(error);
      return false;
    } finally {
      document.currentScript = previous;
    }
  }
  function loadExternal(el) {
    const src = el.getAttribute('src').replace(/^https:\/\/tauri\.localhost\//, '');
    const target = path.normalize(path.join(pageDir, src));
    const relative = path.relative(pageDir, target).split(path.sep).join('/');
    const missing = (opts.missing || []).includes(relative) || !fs.existsSync(target);
    if (missing) {
      if (typeof el.onerror === 'function') guard(el.onerror).call(el, new dom.Event('error'));
      return;
    }
    page.loaded.push(relative);
    runScript(el, fs.readFileSync(target, 'utf8'), target);
    if (typeof el.onload === 'function') guard(el.onload).call(el, new dom.Event('load'));
  }
  function flushScripts() {
    while (pendingScripts.length) loadExternal(pendingScripts.shift());
  }

  const page = {
    file,
    label,
    window,
    document,
    clock,
    hub,
    errors,
    logs,
    media,
    clipboard,
    /** Upstream commands the page invoked: `{cmd, args}`. */
    upstreamCalls,
    /** Script files loaded, in order, relative to the page. */
    loaded: [],
    resizeObservers: [],
    intersectionObservers: [],
    run: (code) => vm.runInContext(code, context),
    $: (selector) => document.querySelector(selector),
    $$: (selector) => document.querySelectorAll(selector),
    text: (selector) => {
      const el = document.querySelector(selector);
      return el ? el.textContent.replace(/\s+/g, ' ').trim() : null;
    },
    /** Delivers a Tauri event to the page's listeners. */
    emit(name, payload) {
      for (const handler of [...(listeners.get(name) || [])]) guard(handler)({ event: name, payload: clone(payload), id: 0 });
    },
    listening: (name) => (listeners.get(name) || new Set()).size > 0,
    /** Lets promise chains (invoke answers, dynamic scripts) run to the end. */
    async settle() {
      for (let i = 0; i < 12; i++) {
        flushScripts();
        await new Promise((resolve) => setImmediate(resolve));
      }
      flushScripts();
    },
    tick(ms) {
      clock.tick(ms);
    },
    click(target) {
      const el = typeof target === 'string' ? document.querySelector(target) : target;
      if (!el) throw new Error(`nothing to click: ${target}`);
      guard(() => el.click())();
      return el;
    },
    /** A keydown on the focused element (or the body): `{key, ctrlKey, altKey, shiftKey}`. */
    key(init) {
      const event = new dom.Event('keydown', Object.assign({ bubbles: true, cancelable: true, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false }, init));
      guard(() => (document.activeElement || document.body).dispatchEvent(event))();
      return event;
    },
    fire(target, type, init) {
      const el = typeof target === 'string' ? document.querySelector(target) : target;
      if (!el) throw new Error(`nothing to fire ${type} on: ${target}`);
      const event = new dom.Event(type, Object.assign({ bubbles: true, cancelable: true }, init));
      guard(() => el.dispatchEvent(event))();
      return event;
    },
    /** Types into a field the way a user does: the value, then an `input` event. */
    type(target, value) {
      const el = typeof target === 'string' ? document.querySelector(target) : target;
      if (!el) throw new Error(`nothing to type into: ${target}`);
      el.value = value;
      guard(() => el.dispatchEvent(new dom.Event('input', { bubbles: true })))();
      return el;
    },
  };

  document.__scriptHook = (el) => {
    pendingScripts.push(el);
    if (!opts.lateScripts && !page.__parsing) flushScripts();
  };

  if (typeof opts.before === 'function') opts.before(window);

  // The parser: every script in document order. A script a script inserts runs once the one
  // that inserted it has finished (or after DOMContentLoaded with `lateScripts`).
  page.__parsing = true;
  let inline = 0;
  for (const el of document.querySelectorAll('script')) {
    if (el.__started) continue;
    el.__started = true;
    if (el.getAttribute('src')) {
      loadExternal(el);
    } else {
      inline += 1;
      const text = el.textContent;
      const lineOffset = html.slice(0, html.indexOf(text)).split('\n').length - 1;
      runScript(el, text, `${file} <script> #${inline}`, lineOffset);
    }
    if (!opts.lateScripts) flushScripts();
  }
  page.__parsing = false;
  document.readyState = 'interactive';
  guard(() => document.dispatchEvent(new dom.Event('DOMContentLoaded', { bubbles: true })))();
  flushScripts();
  document.readyState = 'complete';
  guard(() => windowTarget.dispatchEvent(new dom.Event('load')))();
  return page;
}

module.exports = { loadPage, createClock, createHub, upstreamAnswers, NOW, UI, fixture: contract.fixture, clone };
