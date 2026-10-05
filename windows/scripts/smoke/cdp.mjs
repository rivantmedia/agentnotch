// A small Chrome DevTools Protocol driver for the installer smoke test. The app is started with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>; this attaches to one of
// its pages (notch, settings, agentnotch-panel), calls the backend exactly as the pages do
// (window.__TAURI__.core.invoke), clicks with real pointer events and reads rendered state.
// Node 22's built-in WebSocket; no dependencies.
//
//   node cdp.mjs --port P targets
//   node cdp.mjs --port P eval <page> <expression>
//   node cdp.mjs --port P invoke <page> <command> [<args as JSON>]      (e.g. an_call '{"method":"snapshot"}')
//   node cdp.mjs --port P click <page> <selector>
//   node cdp.mjs --port P wait <page> <expression> [<timeout ms>]
//   node cdp.mjs --port P errors <page>
// Prints {"ok":true,"result":...} or {"ok":false,"error":"..."} (exit 1).
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function listTargets(port, host = '127.0.0.1') {
  const r = await fetch(`http://${host}:${port}/json/list`);
  if (!r.ok) throw new Error(`/json/list answered ${r.status}`);
  return r.json();
}

// A page is found by the file its URL serves (notch.html, settings.html, agentnotch/panel.html:
// the window label `agentnotch-panel` loses its prefix) or by its exact title or window label.
export function pickTarget(targets, name) {
  const key = name.replace(/^agentnotch-/, '');
  const pages = targets.filter((t) => t.type === 'page');
  const file = (t) => {
    try {
      return new URL(t.url).pathname.split('/').pop();
    } catch {
      return '';
    }
  };
  return pages.find((t) => file(t) === `${key}.html`) ?? pages.find((t) => t.title === name || t.id === name);
}

// A page-side listener: a CSP violation is an event on the document, not a console message.
const CSP_HOOK = `(() => {
  if (window.__anCsp) return;
  window.__anCsp = [];
  document.addEventListener('securitypolicyviolation', (e) => window.__anCsp.push({
    directive: e.violatedDirective, blocked: e.blockedURI, source: e.sourceFile || '', sample: e.sample || '',
  }));
})()`;

export class Page {
  constructor(ws, target) {
    this.ws = ws;
    this.target = target;
    this.seq = 0;
    this.pending = new Map();
    this.problems = [];
    ws.addEventListener('message', (ev) => this.#message(JSON.parse(String(ev.data))));
    ws.addEventListener('close', () => {
      for (const { reject } of this.pending.values()) reject(new Error('the DevTools connection closed'));
      this.pending.clear();
    });
  }

  static async open(target) {
    const ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((ok, bad) => {
      ws.addEventListener('open', ok, { once: true });
      ws.addEventListener('error', () => bad(new Error(`cannot open ${target.webSocketDebuggerUrl}`)), { once: true });
    });
    const page = new Page(ws, target);
    await page.send('Runtime.enable'); // replays console messages and exceptions from before we attached
    await page.send('Log.enable');
    await page.send('Page.addScriptToEvaluateOnNewDocument', { source: CSP_HOOK });
    await page.evaluate(CSP_HOOK);
    return page;
  }

  #message(m) {
    if (m.id !== undefined) {
      const p = this.pending.get(m.id);
      if (!p) return;
      this.pending.delete(m.id);
      if (m.error) p.reject(new Error(`${p.method}: ${m.error.message ?? JSON.stringify(m.error)}`));
      else p.resolve(m.result ?? {});
      return;
    }
    const text = (args = []) => args.map((a) => a.value ?? a.description ?? a.type).join(' ');
    if (m.method === 'Runtime.exceptionThrown') {
      const d = m.params.exceptionDetails;
      this.problems.push({ kind: 'exception', text: d.exception?.description ?? d.text, url: d.url });
    } else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') {
      this.problems.push({ kind: 'console', text: text(m.params.args) });
    } else if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') {
      this.problems.push({ kind: 'log', text: m.params.entry.text, url: m.params.entry.url });
    }
  }

  send(method, params = {}, timeoutMs = 15_000) {
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method}: no answer in ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, { method, reject, resolve: (r) => { clearTimeout(timer); resolve(r); } });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  // The value of an expression (a promise is awaited, the result is returned by value).
  async evaluate(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) {
      const d = r.exceptionDetails;
      throw new Error(d.exception?.description ?? d.text ?? 'evaluation failed');
    }
    return r.result?.value;
  }

  // window.__TAURI__.core.invoke(command, args), as the pages call it. A rejection ({code, message}
  // from the backend) throws an Error carrying `code`.
  async invoke(command, args) {
    const call = `window.__TAURI__.core.invoke(${JSON.stringify(command)}, ${JSON.stringify(args ?? {})})`;
    const r = await this.evaluate(`${call}.then(
      (value) => ({ ok: true, value }),
      (e) => ({ ok: false, code: e && e.code, message: e && e.message !== undefined ? e.message : String(e) }))`);
    if (!r?.ok) {
      const e = new Error(`${command}: ${r?.message ?? 'rejected'}`);
      e.code = r?.code;
      throw e;
    }
    return r.value;
  }

  // A real click: the pointer is moved to the element's centre, then pressed and released, so the
  // page's own handlers run (not el.click()).
  async click(selector) {
    const box = await this.evaluate(`(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null;
      el.scrollIntoView({ block: 'center', inline: 'center' });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
    })()`);
    if (!box) throw new Error(`click: nothing matches ${selector}`);
    if (!(box.w > 0 && box.h > 0)) throw new Error(`click: ${selector} has no size`);
    const at = { x: box.x, y: box.y };
    await this.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...at });
    await this.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...at, button: 'left', buttons: 1, clickCount: 1 });
    await this.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...at, button: 'left', buttons: 0, clickCount: 1 });
    return at;
  }

  // Polls an expression until it is truthy; returns its value.
  async waitFor(expression, timeoutMs = 10_000, intervalMs = 100) {
    const end = Date.now() + timeoutMs;
    let last;
    for (;;) {
      try {
        const v = await this.evaluate(expression);
        if (v) return v;
        last = `last value ${JSON.stringify(v)}`;
      } catch (e) {
        last = e.message; // a page that is still loading
      }
      if (Date.now() >= end) throw new Error(`waitFor: ${expression} not truthy in ${timeoutMs} ms (${last})`);
      await sleep(intervalMs);
    }
  }

  // Console errors, uncaught exceptions, error log entries and CSP violations seen so far.
  async errors() {
    let csp = [];
    try {
      csp = (await this.evaluate('window.__anCsp || []')).map((v) => ({ kind: 'csp', text: `${v.directive} blocked ${v.blocked}`, ...v }));
    } catch {
      // the page is gone; what was collected still counts
    }
    return [...this.problems, ...csp];
  }

  close() {
    this.ws.close();
  }
}

// Waits for the page's window to exist (the app creates them lazily), then attaches.
export async function attach(port, name, { timeoutMs = 10_000, host = '127.0.0.1' } = {}) {
  const end = Date.now() + timeoutMs;
  let last = 'no targets';
  for (;;) {
    try {
      const targets = await listTargets(port, host);
      const t = pickTarget(targets, name);
      if (t) return await Page.open(t);
      last = `pages: ${targets.filter((x) => x.type === 'page').map((x) => x.url).join(', ') || 'none'}`;
    } catch (e) {
      last = e.message;
    }
    if (Date.now() >= end) throw new Error(`no page "${name}" on port ${port} (${last})`);
    await sleep(200);
  }
}

// Function forms of the same calls, for scripts that think in (page, ...) terms.
export const evaluate = (page, expression) => page.evaluate(expression);
export const invoke = (page, command, args) => page.invoke(command, args);
export const click = (page, selector) => page.click(selector);
export const waitFor = (page, expression, timeoutMs, intervalMs) => page.waitFor(expression, timeoutMs, intervalMs);
export const errors = (page) => page.errors();

async function main(argv) {
  const out = (o) => console.log(JSON.stringify(o));
  let port;
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--port') port = Number(argv[++i]);
    else rest.push(argv[i]);
  }
  const [cmd, name, ...a] = rest;
  let page;
  try {
    if (!port) throw new Error('--port is required');
    if (cmd === 'targets') {
      out({ ok: true, result: await listTargets(port) });
      return 0;
    }
    if (!['eval', 'invoke', 'click', 'wait', 'errors'].includes(cmd) || !name) throw new Error('usage: cdp.mjs --port P eval|invoke|click|wait|errors <page> ...');
    page = await attach(port, name);
    let result;
    if (cmd === 'eval') result = await page.evaluate(a[0]);
    else if (cmd === 'invoke') result = await page.invoke(a[0], a[1] ? JSON.parse(a[1]) : {});
    else if (cmd === 'click') result = await page.click(a[0]);
    else if (cmd === 'wait') result = await page.waitFor(a[0], a[1] ? Number(a[1]) : undefined);
    else result = await page.errors();
    out({ ok: true, result: result ?? null });
    return 0;
  } catch (e) {
    out({ ok: false, error: String(e.message ?? e), ...(e.code ? { code: e.code } : {}) });
    return 1;
  } finally {
    page?.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  // Exit explicitly once the answer is flushed: a DevTools socket that is slow to finish its
  // close handshake must not keep a smoke step waiting.
  main(process.argv.slice(2)).then((c) => process.stdout.write('', () => process.exit(c)));
}
