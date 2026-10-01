// A fake Anthropic Messages API for the hermetic Claude Code job (Maintainer decision Q3): it
// lets the real Claude Code run a turn with no login and no network. It answers from a
// scenario: each model request gets the next turn (a tool_use, e.g. Bash echo, AskUserQuestion,
// ExitPlanMode); a request whose last user message carries a tool_result gets plain text. Only an
// x-api-key equal to the configured fake key is accepted. Every request is logged as one JSON line.
//
//   node fake-anthropic.mjs [--port 0] [--scenario <file.json> | --example] [--log <file.jsonl>]
//
// Scenario: {"apiKey", "model"?, "turns":[{"content":[<text|tool_use block>], "promptIncludes"?}],
//            "afterToolResult"?:[<blocks>], "passiveText"?, "routes"?:[{"method","path","status","body"}]}
// Which other endpoints Claude Code calls at start-up is decided from its bundle in wp11-8; the
// router is a table (`routes`, then `defaultRoutes`) so that decision is data, not code.
import http from 'node:http';
import { readFileSync, appendFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const FAKE_KEY = 'fake-key-not-a-secret';

export const EXAMPLE_SCENARIO = {
  apiKey: FAKE_KEY,
  turns: [
    { content: [{ type: 'text', text: 'Running a command.' }, { type: 'tool_use', name: 'Bash', input: { command: 'echo agentnotch-smoke', description: 'Print a marker' } }] },
    {
      content: [{
        type: 'tool_use',
        name: 'AskUserQuestion',
        input: { questions: [{ question: 'Which colour?', header: 'Colour', multiSelect: false, options: [{ label: 'Red', description: 'Warm' }, { label: 'Blue', description: 'Cool' }] }] },
      }],
    },
    { content: [{ type: 'tool_use', name: 'ExitPlanMode', input: { plan: '1. Do the thing\n2. Check it' } }] },
  ],
  afterToolResult: [{ type: 'text', text: 'Done.' }],
  passiveText: 'ok',
};

const errorBody = (type, message) => ({ type: 'error', error: { type, message } });

// Endpoints besides /v1/messages. A handler gets (request info) and returns [status, body].
export const defaultRoutes = [
  { method: 'GET', path: /^\/v1\/models(\/.*)?$/, status: 200, body: { data: [], has_more: false, first_id: null, last_id: null } },
  { method: 'POST', path: /^\/v1\/messages\/count_tokens$/, status: 200, body: { input_tokens: 1 } },
];

const lastUser = (messages = []) => [...messages].reverse().find((m) => m.role === 'user');
const blocksOf = (m) => (typeof m?.content === 'string' ? [{ type: 'text', text: m.content }] : (m?.content ?? []));
const userText = (m) => blocksOf(m).filter((b) => b.type === 'text').map((b) => b.text).join('\n');

export async function startFakeAnthropic({ scenario, scenarioFile, port = 0, logFile, host = '127.0.0.1' } = {}) {
  const sc = scenario ?? (scenarioFile ? JSON.parse(readFileSync(scenarioFile, 'utf8')) : EXAMPLE_SCENARIO);
  const log = [];
  let ids = 0;
  const used = new Set();

  // The scenario's next turn: an unused one whose promptIncludes matches, else the first unused
  // one without a condition; when all are used, the last one again.
  function nextTurn(prompt) {
    const free = (t, k) => !used.has(k);
    let i = sc.turns.findIndex((t, k) => free(t, k) && t.promptIncludes && prompt.includes(t.promptIncludes));
    if (i === -1) i = sc.turns.findIndex((t, k) => free(t, k) && !t.promptIncludes);
    const k = i === -1 ? sc.turns.length - 1 : i;
    used.add(k);
    return { index: k, content: sc.turns[k].content };
  }

  function chooseContent(body) {
    const user = lastUser(body.messages);
    const hasResult = blocksOf(user).some((b) => b.type === 'tool_result');
    const prompt = userText(user);
    // Side requests (a quota ping, a title) neither carry tools nor spend a scripted turn.
    if (!hasResult && ((body.max_tokens ?? 2) <= 1 || !Array.isArray(body.tools) || body.tools.length === 0)) {
      return { kind: 'passive', content: [{ type: 'text', text: sc.passiveText ?? 'ok' }] };
    }
    if (hasResult) return { kind: 'after_tool_result', content: sc.afterToolResult ?? [{ type: 'text', text: 'Done.' }] };
    const t = nextTurn(prompt);
    return { kind: 'turn', turn: t.index, content: t.content };
  }

  const withIds = (blocks) => blocks.map((b) => (b.type === 'tool_use' ? { id: `toolu_fake_${++ids}`, input: {}, ...b } : b));
  const stopReason = (blocks) => (blocks.some((b) => b.type === 'tool_use') ? 'tool_use' : 'end_turn');

  function jsonMessage(model, blocks) {
    return {
      id: `msg_fake_${++ids}`, type: 'message', role: 'assistant', model, content: blocks,
      stop_reason: stopReason(blocks), stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 5, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
    };
  }

  function sse(res, model, blocks) {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
    const send = (type, data) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
    const msg = jsonMessage(model, []);
    send('message_start', { message: { ...msg, usage: { ...msg.usage, output_tokens: 1 } } });
    blocks.forEach((b, index) => {
      if (b.type === 'tool_use') {
        send('content_block_start', { index, content_block: { type: 'tool_use', id: b.id, name: b.name, input: {} } });
        const json = JSON.stringify(b.input ?? {});
        const cut = Math.max(1, Math.floor(json.length / 2)); // two fragments, as the real API streams
        for (const partial_json of [json.slice(0, cut), json.slice(cut)]) {
          send('content_block_delta', { index, delta: { type: 'input_json_delta', partial_json } });
        }
      } else {
        send('content_block_start', { index, content_block: { type: 'text', text: '' } });
        send('content_block_delta', { index, delta: { type: 'text_delta', text: b.text } });
      }
      send('content_block_stop', { index });
    });
    send('message_delta', { delta: { stop_reason: stopReason(blocks), stop_sequence: null }, usage: { output_tokens: 5 } });
    send('message_stop', {});
    res.end();
  }

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://fake.invalid');
    const entry = { method: req.method, path: url.pathname, headers: Object.keys(req.headers).sort() };
    if (req.headers.authorization) entry.authorization = 'present'; // never the value
    const finish = (status, body) => {
      entry.status = status;
      log.push(entry);
      if (logFile) appendFileSync(logFile, `${JSON.stringify(entry)}\n`);
      const text = JSON.stringify(body);
      res.writeHead(status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(text) });
      res.end(text);
    };
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const raw = Buffer.concat(chunks).toString('utf8');
    const key = req.headers['x-api-key'];
    entry.apiKey = key === undefined ? 'absent' : key === sc.apiKey ? 'ok' : 'bad';
    if (entry.apiKey !== 'ok') return finish(401, errorBody('authentication_error', 'invalid x-api-key'));

    const route = [...(sc.routes ?? []).map((r) => ({ ...r, path: new RegExp(r.path) })), ...defaultRoutes]
      .find((r) => r.method === req.method && r.path.test(url.pathname));
    if (route) return finish(route.status, route.body);
    if (req.method !== 'POST' || url.pathname !== '/v1/messages') {
      return finish(404, errorBody('not_found_error', `No route for ${req.method} ${url.pathname}`));
    }
    let body;
    try {
      body = JSON.parse(raw);
    } catch {
      return finish(400, errorBody('invalid_request_error', 'The body is not JSON'));
    }
    const choice = chooseContent(body);
    const blocks = withIds(choice.content);
    const model = sc.model ?? body.model ?? 'claude-fake';
    Object.assign(entry, {
      model: body.model, stream: body.stream === true, tools: (body.tools ?? []).map((t) => t.name),
      messages: (body.messages ?? []).length, reply: choice.kind, turn: choice.turn,
      lastUserText: userText(lastUser(body.messages)).slice(0, 200),
    });
    if (body.stream === true) {
      entry.status = 200;
      log.push(entry);
      if (logFile) appendFileSync(logFile, `${JSON.stringify(entry)}\n`);
      return sse(res, model, blocks);
    }
    return finish(200, jsonMessage(model, blocks));
  });

  await new Promise((ok, bad) => server.once('error', bad).listen(port, host, ok));
  const actual = server.address().port;
  return {
    port: actual, url: `http://${host}:${actual}`, log, scenario: sc,
    close: () => new Promise((ok) => { server.closeAllConnections?.(); server.close(ok); }),
  };
}

function main(argv) {
  const opt = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--example') opt.example = true;
    else opt[argv[i].replace(/^--/, '')] = argv[++i];
  }
  startFakeAnthropic({ port: Number(opt.port ?? 0), scenarioFile: opt.scenario, logFile: opt.log }).then((s) => {
    console.log(JSON.stringify({ port: s.port, url: s.url }));
    const stop = () => s.close().then(() => process.exit(0));
    process.on('SIGTERM', stop);
    process.on('SIGINT', stop);
  }, (e) => {
    console.error(JSON.stringify({ error: String(e?.message ?? e) }));
    process.exit(1);
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main(process.argv.slice(2));
