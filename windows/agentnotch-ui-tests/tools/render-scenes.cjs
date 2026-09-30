#!/usr/bin/env node
'use strict';
// Renders the app's pages in a real (headless) Chromium and writes what they look like.
//
//   node agentnotch-ui-tests/tools/render-scenes.cjs --browser <headless_shell> --out <dir>
//        [--only <name prefix>] [--widths 400,520] [--theme dark|light] [--dpr 2]
//        [--scenes <file>] [--backdrop <css colour>] [--no-layout-fail] [--list]
//        [--page <file> [--global <name>] [--scene <name>] [--edge <edge>] [--width n] [--height n|fit]]
//
// For each scene it serves codenotch/ui over http://127.0.0.1:<port> (the way the app serves it:
// `script-src 'self'` and the dynamic script loads behave the same, unlike file://), injects a
// fake `window.__TAURI__` before the page's first script, loads the page at the scene's viewport,
// theme and edge, feeds it the scene's events, calls `<global>.showScene(scene)`, waits for the
// fonts and two frames, and writes `<name>[-<width>][-light].png` (transparent background kept,
// as the app's windows are) and a `.json` beside it with the page's `layoutReport()`.
//
// The fake answers `an_call` from the ui-contract fixtures, like the harness's does, and every
// call is held against lib/contract.cjs afterwards. Upstream's own commands answer from
// `harness.upstreamAnswers`. Time is frozen at the fixtures' `generated_at_ms`, so a scene
// renders the same on every run.
//
// Exit status: 1 when a page throws, logs an error, loads nothing it asked for, breaks the CSP,
// makes a call the contract refuses, or fails its layout report (unless --no-layout-fail);
// 2 for bad arguments or a browser that will not start. Node built-ins only (Node 22 has the
// global WebSocket the DevTools protocol needs).

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
const contract = require('../lib/contract.cjs');
const harness = require('../lib/harness.cjs');

const T = path.join(__dirname, '..');
const UI = harness.UI;
const DEFAULT_WIDTHS = [400, 520];
const LABELS = { 'notch.html': 'notch', 'settings.html': 'settings', 'agentnotch/panel.html': 'agentnotch-panel', 'dropzones.html': 'dropzones' };

// ---- arguments ----------------------------------------------------------------------------------

function parseArgs(argv) {
  const flags = new Set(['list', 'no-layout-fail', 'help', 'verbose']);
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('--')) throw new Error(`unexpected argument ${a}`);
    const name = a.slice(2);
    if (flags.has(name)) args[name] = true;
    else if (i + 1 < argv.length) args[name] = argv[++i];
    else throw new Error(`--${name} needs a value`);
  }
  return args;
}

const USAGE = `usage: render-scenes.cjs --browser <headless_shell> --out <dir> [--only <prefix>] [--widths 400,520]
       [--theme dark|light] [--dpr 2] [--scenes <file>] [--verbose] [--backdrop <colour>] [--no-layout-fail] [--list]
       [--page <file> [--global <name>] [--scene <name>] [--edge <edge>] [--width <n>] [--height <n|fit>]]`;

// ---- the static server ------------------------------------------------------------------------------

const TYPES = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.json': 'application/json', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff', '.ttf': 'font/ttf',
};

/** The page CSP the app sets on every page (tauri.conf.json, seam WCSP), so a violation shows here. */
function appCsp() {
  const conf = JSON.parse(fs.readFileSync(path.join(contract.WINDOWS, 'codenotch', 'tauri.conf.json'), 'utf8'));
  const csp = conf && conf.app && conf.app.security && conf.app.security.csp;
  if (typeof csp !== 'string') throw new Error("tauri.conf.json has no app.security.csp");
  return csp;
}

/**
 * Tauri adds a hash to `script-src` for every inline script of a page it serves (only
 * `style-src` is exempted from that in tauri.conf.json), which is how upstream's own pages keep
 * their inline scripts under the app-wide policy. The fork's pages have none.
 */
function withInlineHashes(csp, html) {
  const hashes = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)]
    .map((m) => `'sha256-${crypto.createHash('sha256').update(m[1]).digest('base64')}'`);
  return hashes.length ? csp.replace(/script-src ([^;]*)/, (_, sources) => `script-src ${sources} ${hashes.join(' ')}`) : csp;
}

function startServer(problems) {
  const csp = appCsp();
  const root = path.resolve(UI);
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    let rel;
    try {
      rel = decodeURIComponent(url.pathname).replace(/^\/+/, '');
    } catch (e) {
      rel = '';
    }
    if (rel === 'favicon.ico') {
      res.writeHead(204);
      res.end();
      return;
    }
    const file = path.resolve(root, rel);
    const inside = file === root || file.startsWith(root + path.sep);
    if (!inside || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      problems.push(`404 ${url.pathname}`);
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
      return;
    }
    const type = TYPES[path.extname(file)] || 'application/octet-stream';
    const body = fs.readFileSync(file);
    res.writeHead(200, {
      'content-type': type,
      'content-security-policy': path.extname(file) === '.html' ? withInlineHashes(csp, body.toString('utf8')) : csp,
      'cache-control': 'no-store',
    });
    res.end(body);
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port }));
  });
}

// ---- the DevTools protocol ----------------------------------------------------------------------------

function connect(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    let next = 0;
    const waiting = new Map();
    const listeners = new Set();
    ws.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== undefined) {
        const entry = waiting.get(message.id);
        if (!entry) return;
        waiting.delete(message.id);
        if (message.error) entry.reject(new Error(`${entry.method}: ${message.error.message}`));
        else entry.resolve(message.result);
      } else {
        for (const fn of [...listeners]) fn(message);
      }
    });
    ws.addEventListener('close', () => {
      for (const entry of waiting.values()) entry.reject(new Error('the browser closed the connection'));
      waiting.clear();
    });
    ws.addEventListener('error', () => reject(new Error('could not connect to the browser')));
    ws.addEventListener('open', () => resolve({
      send(method, params, sessionId) {
        const id = ++next;
        const payload = { id, method, params: params || {} };
        if (sessionId) payload.sessionId = sessionId;
        return new Promise((res, rej) => {
          waiting.set(id, { resolve: res, reject: rej, method });
          ws.send(JSON.stringify(payload));
        });
      },
      on(fn) {
        listeners.add(fn);
        return () => listeners.delete(fn);
      },
      close: () => ws.close(),
    }));
  });
}

function launch(browser, profile) {
  // The browser's stderr goes to a file, not a pipe: its helper processes inherit it, and a pipe
  // would stay open (and hang a `| tail` on this tool) for as long as any of them lives.
  const logFile = path.join(profile, 'browser.log');
  const fd = fs.openSync(logFile, 'w');
  const child = spawn(browser, [
    '--remote-debugging-port=0', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--mute-audio', '--no-first-run',
    '--disable-extensions', '--disable-background-networking', '--font-render-hinting=none', `--user-data-dir=${path.join(profile, 'data')}`, 'about:blank',
  ], { stdio: ['ignore', 'ignore', fd], detached: true });
  fs.closeSync(fd);
  return new Promise((resolve, reject) => {
    let exited = false;
    child.once('error', (error) => {
      exited = true;
      reject(new Error(`cannot start ${browser}: ${error.message}`));
    });
    child.once('exit', (code) => {
      exited = true;
      reject(new Error(`the browser exited (${code}) before it was ready:\n${fs.readFileSync(logFile, 'utf8')}`));
    });
    const started = Date.now();
    const poll = setInterval(() => {
      if (exited) {
        clearInterval(poll);
        return;
      }
      const m = /DevTools listening on (ws:\/\/\S+)/.exec(fs.readFileSync(logFile, 'utf8'));
      if (m) {
        clearInterval(poll);
        child.removeAllListeners('exit');
        resolve({ child, url: m[1] });
      } else if (Date.now() - started > 20000) {
        clearInterval(poll);
        reject(new Error(`the browser did not report a DevTools address:\n${fs.readFileSync(logFile, 'utf8')}`));
      }
    }, 50);
  });
}

/** Kills the browser and every process it started. */
function stopBrowser(child) {
  return new Promise((resolve) => {
    child.once('exit', resolve);
    try {
      process.kill(-child.pid, 'SIGKILL');
    } catch (e) {
      child.kill('SIGKILL');
    }
  });
}

// ---- the fake bridge, injected before the page's first script -------------------------------------------
// Serialised with Function.prototype.toString, so it must not close over anything: its whole
// input is `config`.

function fakeBridge(config) {
  var RealDate = Date;
  function FrozenDate() {
    var a = Array.prototype.slice.call(arguments);
    if (!(this instanceof FrozenDate)) return new RealDate(config.now).toString();
    return a.length === 0 ? new RealDate(config.now) : new (Function.prototype.bind.apply(RealDate, [null].concat(a)))();
  }
  FrozenDate.prototype = RealDate.prototype;
  FrozenDate.now = function () { return config.now; };
  FrozenDate.parse = RealDate.parse;
  FrozenDate.UTC = RealDate.UTC;
  window.Date = FrozenDate;

  var clone = function (v) { return v === undefined ? undefined : JSON.parse(JSON.stringify(v)); };
  var listeners = {};
  var test = window.__ANTEST__ = {
    calls: [], upstream: [], violations: [], inflight: 0, lastActivity: performance.now(),
    emit: function (name, payload) {
      test.lastActivity = performance.now();
      (listeners[name] || []).slice().forEach(function (h) { h({ event: name, payload: clone(payload), id: 0 }); });
    },
  };
  document.addEventListener('securitypolicyviolation', function (e) {
    test.violations.push(e.violatedDirective + ' blocked ' + (e.blockedURI || 'inline') + ' at ' + (e.sourceFile || '') + ':' + e.lineNumber);
  });

  function fromFixture(method, args) {
    var wanted = JSON.stringify(args == null ? {} : args);
    for (var i = 0; i < config.calls.length; i++) {
      var entry = config.calls[i];
      if (entry.call.method !== method) continue;
      if (JSON.stringify(entry.call.args === undefined ? {} : entry.call.args) !== wanted) continue;
      if (entry.error) return { error: entry.error };
      if (entry.reply !== undefined) return { reply: clone(entry.reply) };
    }
    return null;
  }

  function anCall(method, args) {
    test.calls.push({ method: method, args: clone(args === undefined ? null : args) });
    if (Object.prototype.hasOwnProperty.call(config.replies, method)) return Promise.resolve(clone(config.replies[method]));
    if (method === 'snapshot') return Promise.resolve(clone(config.snapshot));
    if (method === 'settings') return Promise.resolve(clone(config.settings));
    if (method === 'chat_open') {
      setTimeout(function () { test.emit('an:chat', config.chat); }, 0);
      return Promise.resolve({});
    }
    var fixed = fromFixture(method, args);
    if (fixed && fixed.error) return Promise.reject({ code: fixed.error, message: 'Sealed: ' + method + ' does nothing here.' });
    if (fixed) return Promise.resolve(fixed.reply);
    var defaults = config.defaults;
    if (Object.prototype.hasOwnProperty.call(defaults, method)) return Promise.resolve(clone(defaults[method]));
    return Promise.resolve({});
  }

  window.__TAURI__ = {
    core: {
      invoke: function (cmd, args) {
        test.inflight += 1;
        test.lastActivity = performance.now();
        var result;
        if (cmd === 'an_call') {
          var a = args || {};
          result = anCall(a.method, a.args);
        } else {
          test.upstream.push(cmd);
          result = Object.prototype.hasOwnProperty.call(config.upstream, cmd)
            ? Promise.resolve(clone(config.upstream[cmd]))
            : Promise.reject(new Error('no such command: ' + cmd));
        }
        var done = function () { test.inflight -= 1; test.lastActivity = performance.now(); };
        result.then(done, done);
        return result;
      },
    },
    event: {
      listen: function (name, handler) {
        (listeners[name] = listeners[name] || []).push(handler);
        return Promise.resolve(function () { listeners[name] = (listeners[name] || []).filter(function (h) { return h !== handler; }); });
      },
    },
    app: { getVersion: function () { return Promise.resolve('1.0.0'); } },
    window: { getCurrentWindow: function () { return { label: config.label, close: function () { return Promise.resolve(); } }; } },
  };
  if (config.panel) window.__AGENTNOTCH_PANEL__ = clone(config.panel);
}

// ---- rendering one scene ---------------------------------------------------------------------------------

const IDLE = `new Promise(function (resolve) {
  var t0 = performance.now();
  (function poll() {
    var a = window.__ANTEST__;
    if (!a || (a.inflight === 0 && performance.now() - a.lastActivity > 150) || performance.now() - t0 > 4000) resolve(true);
    else setTimeout(poll, 30);
  })();
})`;
const FRAMES = `document.fonts.ready.then(function () {
  return new Promise(function (r) { requestAnimationFrame(function () { requestAnimationFrame(function () { r(true); }); }); });
})`;

let verbose = false;
const trace = (text) => {
  if (verbose) console.error(`  [${new Date().toISOString().slice(11, 23)}] ${text}`);
};

async function evaluate(cdp, sessionId, expression, awaitPromise) {
  trace(`evaluate ${expression.replace(/\s+/g, ' ').slice(0, 70)}`);
  const r = await cdp.send('Runtime.evaluate', { expression, awaitPromise: !!awaitPromise, returnByValue: true }, sessionId);
  if (r.exceptionDetails) throw new Error(`evaluate: ${(r.exceptionDetails.exception && r.exceptionDetails.exception.description) || r.exceptionDetails.text}`);
  return r.result.value;
}

function fileName(scene, variant) {
  let name = scene.name;
  if (variant.width && scene.widths) name += `-${variant.width}`;
  if (variant.theme === 'light') name += '-light';
  return name;
}

async function renderVariant(env, scene, variant) {
  const { cdp, port, args } = env;
  const problems = [];
  const theme = variant.theme;
  const width = variant.width || scene.width || 400;
  const height = scene.height === 'fit' ? 700 : scene.height || 700;
  const label = LABELS[scene.page] || 'unknown';
  const edge = scene.edge || 'right';
  const snapshot = harness.fixture('snapshot.json');
  const config = {
    now: harness.NOW,
    label,
    edge,
    snapshot,
    settings: harness.fixture('settings.json'),
    chat: harness.fixture('chat.json'),
    calls: harness.fixture('calls.json'),
    replies: scene.replies || {},
    defaults: {
      message_route: { available: true },
      send_message: { outcome: 'delivered' },
      answer: { result: 'delivered' },
      focus: { outcome: 'focused' },
      refresh_usage: { coming: false },
      choose_claude_binary: { version: '2.1.282' },
      pick_folder: { path: null },
    },
    upstream: harness.upstreamAnswers({ edge, theme }),
    panel: scene.panel || null,
  };

  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
  const s = (method, params) => cdp.send(method, params, sessionId);
  const stop = cdp.on((m) => {
    if (m.sessionId !== sessionId) return;
    if (m.method === 'Runtime.exceptionThrown') {
      const d = m.params.exceptionDetails;
      problems.push(`page error: ${(d.exception && d.exception.description) || d.text}`);
    } else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') {
      problems.push(`console.error: ${m.params.args.map((a) => a.value !== undefined ? a.value : a.description).join(' ')}`);
    } else if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') {
      const e = m.params.entry;
      if (!/favicon/.test(e.url || '')) problems.push(`${e.source} error: ${e.text}${e.url ? ` (${e.url})` : ''}`);
    }
  });
  try {
    await Promise.all(['Page.enable', 'Runtime.enable', 'Log.enable'].map((m) => s(m)));
    await s('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: Number(args.dpr) || 2, mobile: false });
    await s('Emulation.setDefaultBackgroundColorOverride', args.backdrop
      ? { color: parseColour(args.backdrop) } : { color: { r: 0, g: 0, b: 0, a: 0 } });
    await s('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    await s('Page.addScriptToEvaluateOnNewDocument', { source: `(${fakeBridge.toString()})(${JSON.stringify(config)});` });

    const loaded = new Promise((resolve) => {
      const off = cdp.on((m) => {
        if (m.sessionId === sessionId && m.method === 'Page.loadEventFired') {
          off();
          resolve();
        }
      });
    });
    trace('navigate');
    await s('Page.navigate', { url: `http://127.0.0.1:${port}/${scene.page}` });
    await Promise.race([loaded, new Promise((_, rej) => setTimeout(() => rej(new Error('the page did not finish loading in 15 s')), 15000))]);
    await evaluate(cdp, sessionId, IDLE, true);

    for (const ev of scene.events || []) {
      const payload = ev.fixture ? harness.fixture(ev.fixture) : ev.payload;
      await evaluate(cdp, sessionId, `window.__ANTEST__.emit(${JSON.stringify(ev.event)}, ${JSON.stringify(payload === undefined ? null : payload)})`);
      await evaluate(cdp, sessionId, IDLE, true);
    }

    if (scene.global && scene.scene) {
      const shown = await evaluate(cdp, sessionId, `(function () {
        var g = window[${JSON.stringify(scene.global)}];
        if (!g || typeof g.showScene !== 'function') return 'no ${scene.global}.showScene';
        try { return g.showScene(${JSON.stringify(scene.scene)}) === false ? 'showScene refused "${scene.scene}"' : ''; }
        catch (e) { return 'showScene threw: ' + e; }
      })()`);
      if (shown) problems.push(shown);
      await evaluate(cdp, sessionId, IDLE, true);
    }
    if (scene.wait) await new Promise((r) => setTimeout(r, scene.wait));
    await evaluate(cdp, sessionId, FRAMES, true);

    let size = { width, height };
    if (scene.height === 'fit') {
      const fitted = await evaluate(cdp, sessionId, 'Math.ceil(Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0))');
      size = { width, height: Math.min(Math.max(fitted, 40), Number(scene.maxHeight) || 800) };
      await s('Emulation.setDeviceMetricsOverride', { width, height: size.height, deviceScaleFactor: Number(args.dpr) || 2, mobile: false });
      await evaluate(cdp, sessionId, FRAMES, true);
    }

    let layout = null;
    if (scene.global) {
      layout = await evaluate(cdp, sessionId, `(function () {
        var g = window[${JSON.stringify(scene.global)}];
        try { return g && typeof g.layoutReport === 'function' ? g.layoutReport() : null; } catch (e) { return { ok: false, failures: ['layoutReport threw: ' + e] }; }
      })()`);
    }
    const state = await evaluate(cdp, sessionId, 'JSON.stringify({calls: window.__ANTEST__.calls, violations: window.__ANTEST__.violations})');
    const seen = JSON.parse(state);
    for (const v of seen.violations) problems.push(`CSP violation: ${v}`);
    for (const c of seen.calls) {
      const problem = contract.callProblem(c.method, c.args);
      if (problem) problems.push(`contract: ${problem}`);
      if (!contract.allowedFromWindow(label, c.method)) problems.push(`contract: ${c.method} isn't available to the ${label} window`);
    }
    trace('screenshot');
    const shot = await s('Page.captureScreenshot', { format: 'png', fromSurface: true });
    return { png: Buffer.from(shot.data, 'base64'), layout, problems, calls: seen.calls.map((c) => c.method), size };
  } finally {
    stop();
    await cdp.send('Target.closeTarget', { targetId }).catch(() => {});
  }
}

function parseColour(text) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(text));
  if (!m) throw new Error(`--backdrop wants a colour like #808080, got ${text}`);
  const n = parseInt(m[1], 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, a: 1 };
}

// ---- main ------------------------------------------------------------------------------------------------

function sceneList(args) {
  if (args.page) {
    return [{
      name: args.name || path.basename(args.page, '.html'),
      page: args.page, global: args.global, scene: args.scene, edge: args.edge,
      width: args.width ? Number(args.width) : undefined,
      height: args.height === 'fit' ? 'fit' : args.height ? Number(args.height) : undefined,
      widths: !args.width,
    }];
  }
  const file = args.scenes || path.join(T, 'scenes.json');
  const all = JSON.parse(fs.readFileSync(file, 'utf8')).scenes;
  return args.only ? all.filter((s) => s.name.startsWith(args.only)) : all;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  verbose = !!args.verbose;
  if (args.help) {
    console.log(USAGE);
    return 0;
  }
  const scenes = sceneList(args);
  if (args.list) {
    for (const s of scenes) console.log(s.name);
    return 0;
  }
  const browser = args.browser || process.env.AGENTNOTCH_BROWSER;
  if (!browser || !args.out) throw new Error(`--browser and --out are required\n${USAGE}`);
  if (!scenes.length) throw new Error('no scene matches');
  const widths = (args.widths ? args.widths.split(',') : DEFAULT_WIDTHS).map(Number);
  if (widths.some((w) => !(w >= 200 && w <= 2000))) throw new Error(`--widths: ${args.widths}`);
  const themes = args.theme ? [args.theme] : ['dark'];
  if (themes.some((t) => t !== 'dark' && t !== 'light')) throw new Error('--theme is dark or light');
  fs.mkdirSync(args.out, { recursive: true });

  const serverProblems = [];
  const { server, port } = await startServer(serverProblems);
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'an-render-'));
  let child = null;
  let cdp = null;
  let failed = 0;
  try {
    const started = await launch(browser, profile);
    child = started.child;
    cdp = await connect(started.url);
    const env = { cdp, port, args };
    for (const scene of scenes) {
      for (const theme of themes) {
        for (const width of scene.widths ? widths : [null]) {
          const variant = { width, theme };
          const name = fileName(scene, variant);
          let outcome;
          try {
            outcome = await renderVariant(env, scene, variant);
          } catch (error) {
            outcome = { problems: [`render failed: ${error.message}`], layout: null, calls: [] };
          }
          const layoutFailed = outcome.layout && outcome.layout.ok === false && !args['no-layout-fail'];
          const problems = outcome.problems.concat(serverProblems.splice(0));
          if (outcome.png) fs.writeFileSync(path.join(args.out, `${name}.png`), outcome.png);
          fs.writeFileSync(path.join(args.out, `${name}.json`), `${JSON.stringify({ scene, variant, size: outcome.size || null, layout: outcome.layout, problems, calls: outcome.calls }, null, 2)}\n`);
          const bad = problems.length > 0 || layoutFailed;
          if (bad) failed += 1;
          const layoutNote = outcome.layout ? (outcome.layout.ok === false ? ` layout FAILED: ${(outcome.layout.failures || []).join('; ')}` : ' layout ok') : '';
          console.log(`${bad ? 'FAIL' : 'ok  '} ${name}${outcome.png ? '.png' : ''}${layoutNote}`);
          for (const p of problems) console.log(`       ${p}`);
        }
      }
    }
  } finally {
    if (cdp) cdp.close();
    if (child) {
      await stopBrowser(child);
    }
    server.close();
    try {
      fs.rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    } catch (error) {
      console.error(`could not remove ${profile}: ${error.message}`);
    }
  }
  console.log(failed ? `${failed} scene(s) failed` : `rendered to ${args.out}`);
  return failed ? 1 : 0;
}

main().then((code) => {
  process.exit(code);
}, (error) => {
  console.error(error.message);
  process.exit(2);
});
