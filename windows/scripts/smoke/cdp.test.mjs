import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { listTargets, pickTarget, attach, evaluate, invoke, click, waitFor, errors } from './cdp.mjs';

// A minimal DevTools endpoint: /json/list over HTTP and one WebSocket per page, answering the
// methods the test programs. Frames from the client are masked, ours are not (RFC 6455).
const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

function readFrames(state, chunk, onText, onClose) {
  state.buf = Buffer.concat([state.buf, chunk]);
  for (;;) {
    const b = state.buf;
    if (b.length < 2) return;
    let len = b[1] & 0x7f;
    let off = 2;
    if (len === 126) { if (b.length < 4) return; len = b.readUInt16BE(2); off = 4; }
    else if (len === 127) { if (b.length < 10) return; len = Number(b.readBigUInt64BE(2)); off = 10; }
    if (b.length < off + 4 + len) return;
    const mask = b.subarray(off, off + 4);
    const payload = Buffer.from(b.subarray(off + 4, off + 4 + len));
    for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
    state.buf = b.subarray(off + 4 + len);
    if ((b[0] & 0x0f) === 1) onText(payload.toString('utf8'));
    if ((b[0] & 0x0f) === 8) onClose();
  }
}

function frame(text) {
  const p = Buffer.from(text);
  const head = p.length < 126 ? Buffer.from([0x81, p.length]) : Buffer.from([0x81, 126, p.length >> 8, p.length & 255]);
  return Buffer.concat([head, p]);
}

async function fakeCdp(targets, handlers) {
  const calls = [];
  const sockets = [];
  const server = http.createServer((req, res) => {
    if (req.url === '/json/list') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(targets(server.address().port)));
    } else res.writeHead(404).end();
  });
  server.on('upgrade', (req, socket) => {
    const accept = createHash('sha1').update(req.headers['sec-websocket-key'] + GUID).digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    sockets.push(socket);
    const state = { buf: Buffer.alloc(0) };
    const push = (method, params) => socket.write(frame(JSON.stringify({ method, params })));
    socket.on('data', (chunk) => readFrames(state, chunk, (text) => {
      const m = JSON.parse(text);
      calls.push(m);
      const h = handlers[m.method];
      const reply = h ? h(m.params, { push }) : {};
      socket.write(frame(JSON.stringify(reply?.error ? { id: m.id, error: reply.error } : { id: m.id, result: reply ?? {} })));
    }, () => socket.end(Buffer.from([0x88, 0x00]))));
    socket.on('error', () => {});
  });
  await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
  return {
    port: server.address().port, calls,
    close: () => new Promise((ok) => { sockets.forEach((s) => s.destroy()); server.close(ok); }),
  };
}

const wsUrl = (port, id) => `ws://127.0.0.1:${port}/devtools/page/${id}`;
const pages = (port) => [
  { id: 'A', type: 'page', title: 'Agent Notch', url: 'http://tauri.localhost/notch.html', webSocketDebuggerUrl: wsUrl(port, 'A') },
  { id: 'B', type: 'page', title: 'Settings', url: 'http://tauri.localhost/settings.html?x=1', webSocketDebuggerUrl: wsUrl(port, 'B') },
  { id: 'C', type: 'page', title: 'Sessions', url: 'http://tauri.localhost/agentnotch/panel.html', webSocketDebuggerUrl: wsUrl(port, 'C') },
  { id: 'D', type: 'other', title: 'worker', url: 'http://tauri.localhost/notch.html', webSocketDebuggerUrl: wsUrl(port, 'D') },
];
const exprs = (cdp) => cdp.calls.filter((c) => c.method === 'Runtime.evaluate').map((c) => c.params.expression);

async function withCdp(handlers, fn) {
  const cdp = await fakeCdp(pages, handlers);
  try {
    await fn(cdp);
  } finally {
    await cdp.close();
  }
}

test('targets are listed and pages are told apart by the file they serve', async () => {
  await withCdp({}, async (cdp) => {
    const targets = await listTargets(cdp.port);
    assert.equal(targets.length, 4);
    assert.equal(pickTarget(targets, 'notch').id, 'A');
    assert.equal(pickTarget(targets, 'settings').id, 'B');
    assert.equal(pickTarget(targets, 'agentnotch-panel').id, 'C');
    assert.equal(pickTarget(targets, 'Sessions').id, 'C'); // by title
    assert.equal(pickTarget(targets, 'dropzones'), undefined);
  });
});

test('evaluate sends awaitPromise and returnByValue and returns the value; an exception throws', async () => {
  await withCdp({
    'Runtime.evaluate': (p) => (p.expression === 'boom()'
      ? { exceptionDetails: { text: 'Uncaught', exception: { description: 'ReferenceError: boom is not defined' } } }
      : { result: { type: 'number', value: 42 } }),
  }, async (cdp) => {
    const page = await attach(cdp.port, 'notch');
    assert.equal(await evaluate(page, '6 * 7'), 42);
    const call = cdp.calls.findLast((c) => c.method === 'Runtime.evaluate');
    assert.equal(call.params.expression, '6 * 7');
    assert.equal(call.params.awaitPromise, true);
    assert.equal(call.params.returnByValue, true);
    await assert.rejects(evaluate(page, 'boom()'), /ReferenceError: boom is not defined/);
    page.close();
  });
});

test('attach enables the domains and installs the CSP listener for later documents', async () => {
  await withCdp({}, async (cdp) => {
    const page = await attach(cdp.port, 'settings');
    const methods = cdp.calls.map((c) => c.method);
    assert.deepEqual(methods.slice(0, 3), ['Runtime.enable', 'Log.enable', 'Page.addScriptToEvaluateOnNewDocument']);
    assert.match(cdp.calls[2].params.source, /securitypolicyviolation/);
    page.close();
  });
});

test('invoke calls window.__TAURI__.core.invoke as the pages do and unwraps the value', async () => {
  await withCdp({
    'Runtime.evaluate': (p) => (p.expression.includes('__TAURI__') ? { result: { value: { ok: true, value: { sessions: [] } } } } : {}),
  }, async (cdp) => {
    const page = await attach(cdp.port, 'agentnotch-panel');
    const value = await invoke(page, 'an_call', { method: 'snapshot', args: null });
    assert.deepEqual(value, { sessions: [] });
    const expr = exprs(cdp).findLast((e) => e.includes('__TAURI__'));
    assert.ok(expr.startsWith('window.__TAURI__.core.invoke("an_call", {"method":"snapshot","args":null})'), expr);
    page.close();
  });
});

test('a rejected invoke throws with the backend error code', async () => {
  await withCdp({
    'Runtime.evaluate': (p) => (p.expression.includes('__TAURI__') ? { result: { value: { ok: false, code: 'not_allowed', message: 'no' } } } : {}),
  }, async (cdp) => {
    const page = await attach(cdp.port, 'notch');
    await assert.rejects(invoke(page, 'an_call', { method: 'x' }), (e) => e.code === 'not_allowed' && /no/.test(e.message));
    page.close();
  });
});

test('click moves to the element centre and presses and releases the left button', async () => {
  await withCdp({
    'Runtime.evaluate': (p) => {
      if (p.expression.includes('[data-an-allow]')) return { result: { value: { x: 120.5, y: 40, w: 80, h: 24 } } };
      if (p.expression.includes('[data-an-gone]')) return { result: { value: null } };
      if (p.expression.includes('[data-an-flat]')) return { result: { value: { x: 1, y: 1, w: 0, h: 0 } } };
      return {};
    },
  }, async (cdp) => {
    const page = await attach(cdp.port, 'agentnotch-panel');
    const at = await click(page, '[data-an-allow]');
    assert.deepEqual(at, { x: 120.5, y: 40 });
    const mouse = cdp.calls.filter((c) => c.method === 'Input.dispatchMouseEvent');
    assert.deepEqual(mouse.map((c) => c.params.type), ['mouseMoved', 'mousePressed', 'mouseReleased']);
    assert.ok(mouse.every((c) => c.params.x === 120.5 && c.params.y === 40));
    assert.equal(mouse[1].params.button, 'left');
    assert.equal(mouse[1].params.clickCount, 1);
    await assert.rejects(click(page, '[data-an-gone]'), /nothing matches/);
    await assert.rejects(click(page, '[data-an-flat]'), /no size/);
    assert.equal(cdp.calls.filter((c) => c.method === 'Input.dispatchMouseEvent').length, 3);
    page.close();
  });
});

test('waitFor polls until the expression is truthy, and times out with the last value', async () => {
  let n = 0;
  await withCdp({
    'Runtime.evaluate': (p) => (p.expression === 'ready()' ? { result: { value: ++n >= 3 ? 'yes' : false } } : { result: { value: false } }),
  }, async (cdp) => {
    const page = await attach(cdp.port, 'settings');
    assert.equal(await waitFor(page, 'ready()', 3000, 10), 'yes');
    assert.equal(n, 3);
    await assert.rejects(waitFor(page, 'never()', 150, 20), /not truthy in 150 ms \(last value false\)/);
    page.close();
  });
});

test('errors: console errors, exceptions, error log entries (replayed on enable) and CSP violations', async () => {
  await withCdp({
    'Runtime.enable': (p, { push }) => {
      push('Runtime.consoleAPICalled', { type: 'log', args: [{ type: 'string', value: 'fine' }] });
      push('Runtime.consoleAPICalled', { type: 'error', args: [{ type: 'string', value: 'bad' }, { type: 'number', value: 7 }] });
      push('Runtime.exceptionThrown', { exceptionDetails: { text: 'Uncaught', exception: { description: 'TypeError: x' }, url: 'http://tauri.localhost/notch.html' } });
    },
    'Log.enable': (p, { push }) => {
      push('Log.entryAdded', { entry: { level: 'error', text: 'Failed to load resource', url: 'http://x/y.js' } });
      push('Log.entryAdded', { entry: { level: 'info', text: 'chatty' } });
    },
    'Runtime.evaluate': (p) => (p.expression === 'window.__anCsp || []'
      ? { result: { value: [{ directive: 'script-src', blocked: 'inline', source: '', sample: '' }] } }
      : {}),
  }, async (cdp) => {
    const page = await attach(cdp.port, 'notch');
    const found = await errors(page);
    assert.deepEqual(found.map((e) => [e.kind, e.text]), [
      ['console', 'bad 7'],
      ['exception', 'TypeError: x'],
      ['log', 'Failed to load resource'],
      ['csp', 'script-src blocked inline'],
    ]);
    page.close();
  });
});

test('attach waits for a window that does not exist yet, and gives up with what it saw', async () => {
  let ready = false;
  const cdp = await fakeCdp((port) => (ready ? pages(port) : [pages(port)[0]]), {});
  try {
    setTimeout(() => { ready = true; }, 300);
    const page = await attach(cdp.port, 'settings', { timeoutMs: 5000 });
    assert.equal(page.target.id, 'B');
    page.close();
    await assert.rejects(attach(cdp.port, 'dropzones', { timeoutMs: 300 }), /no page "dropzones".*notch\.html/);
  } finally {
    await cdp.close();
  }
});

const cli = (port, ...args) => new Promise((done) => {
  const child = spawn(process.execPath, [fileURLToPath(new URL('./cdp.mjs', import.meta.url)), '--port', String(port), ...args], { stdio: ['ignore', 'pipe', 'pipe'] });
  let out = '';
  child.stdout.on('data', (c) => (out += c));
  child.on('close', (code) => done({ code, json: JSON.parse(out) }));
});

test('the CLI: eval, invoke, click, wait, errors, targets print JSON', async () => {
  await withCdp({
    'Runtime.evaluate': (p) => {
      const e = p.expression;
      if (e === '1+1') return { result: { value: 2 } };
      if (e.includes('__TAURI__')) return { result: { value: { ok: true, value: 'delivered' } } };
      if (e.includes('data-an-ok')) return { result: { value: { x: 5, y: 6, w: 10, h: 10 } } };
      if (e === 'flag()') return { result: { value: true } };
      return { result: { value: [] } };
    },
  }, async (cdp) => {
    assert.deepEqual((await cli(cdp.port, 'eval', 'notch', '1+1')).json, { ok: true, result: 2 });
    assert.deepEqual((await cli(cdp.port, 'invoke', 'agentnotch-panel', 'an_call', '{"method":"answer"}')).json, { ok: true, result: 'delivered' });
    assert.deepEqual((await cli(cdp.port, 'click', 'settings', '[data-an-ok]')).json, { ok: true, result: { x: 5, y: 6 } });
    assert.deepEqual((await cli(cdp.port, 'wait', 'settings', 'flag()', '2000')).json, { ok: true, result: true });
    assert.deepEqual((await cli(cdp.port, 'errors', 'notch')).json, { ok: true, result: [] });
    const t = await cli(cdp.port, 'targets');
    assert.equal(t.json.result.length, 4);
    const missing = await cli(cdp.port, 'click', 'settings', '[data-an-gone-xyz]');
    assert.equal(missing.code, 1);
    assert.equal(missing.json.ok, false);
  });
});
