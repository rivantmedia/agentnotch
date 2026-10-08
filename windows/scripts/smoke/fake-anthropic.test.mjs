import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { startFakeAnthropic, EXAMPLE_SCENARIO, FAKE_KEY } from './fake-anthropic.mjs';

const TOOLS = [{ name: 'Bash' }, { name: 'AskUserQuestion' }, { name: 'ExitPlanMode' }];
const call = (site, body, headers = { 'x-api-key': FAKE_KEY }, path = '/v1/messages') =>
  fetch(`${site.url}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'anthropic-version': '2023-06-01', ...headers },
    body: JSON.stringify(body),
  });
const ask = (extra = {}) => ({ model: 'claude-fake-1', max_tokens: 1024, tools: TOOLS, messages: [{ role: 'user', content: 'go' }], ...extra });

function parseSse(text) {
  return text.trim().split('\n\n').map((chunk) => {
    const lines = chunk.split('\n');
    const event = lines.find((l) => l.startsWith('event: ')).slice(7);
    const data = JSON.parse(lines.find((l) => l.startsWith('data: ')).slice(6));
    assert.equal(data.type, event);
    return data;
  });
}

async function withSite(fn, scenario = EXAMPLE_SCENARIO) {
  const logFile = join(mkdtempSync(join(tmpdir(), 'fakeanth-')), 'log.jsonl');
  const site = await startFakeAnthropic({ scenario, logFile });
  try {
    await fn(site, logFile);
  } finally {
    await site.close();
  }
}

test('a streamed turn is a parsable SSE sequence ending in a tool_use', async () => {
  await withSite(async (site) => {
    const r = await call(site, ask({ stream: true }));
    assert.equal(r.status, 200);
    assert.match(r.headers.get('content-type'), /text\/event-stream/);
    const events = parseSse(await r.text());
    assert.deepEqual(events.map((e) => e.type), [
      'message_start',
      'content_block_start', 'content_block_delta', 'content_block_stop',
      'content_block_start', 'content_block_delta', 'content_block_delta', 'content_block_stop',
      'message_delta', 'message_stop',
    ]);
    assert.equal(events[0].message.role, 'assistant');
    assert.deepEqual(events[0].message.content, []);
    assert.equal(events[1].content_block.type, 'text');
    assert.equal(events[2].delta.type, 'text_delta');
    const start = events[4].content_block;
    assert.equal(start.type, 'tool_use');
    assert.equal(start.name, 'Bash');
    assert.match(start.id, /^toolu_/);
    assert.deepEqual(start.input, {});
    const json = events.filter((e) => e.delta?.type === 'input_json_delta').map((e) => e.delta.partial_json).join('');
    assert.deepEqual(JSON.parse(json), { command: 'echo agentnotch-smoke', description: 'Print a marker' });
    assert.equal(events[8].delta.stop_reason, 'tool_use');
    assert.equal(events[4].index, 1);
  });
});

test('without stream the same turn is plain JSON with the input as an object', async () => {
  await withSite(async (site) => {
    const body = await (await call(site, ask())).json();
    assert.equal(body.type, 'message');
    assert.equal(body.stop_reason, 'tool_use');
    assert.equal(body.model, 'claude-fake-1');
    const tool = body.content.find((b) => b.type === 'tool_use');
    assert.deepEqual(tool.input.command, 'echo agentnotch-smoke');
    assert.ok(tool.id);
  });
});

test('turns advance in order, and a tool_result gets plain text', async () => {
  await withSite(async (site) => {
    const first = await (await call(site, ask())).json();
    assert.equal(first.content.at(-1).name, 'Bash');
    const withResult = ask({
      messages: [
        { role: 'user', content: 'go' },
        { role: 'assistant', content: first.content },
        { role: 'user', content: [{ type: 'tool_result', tool_use_id: first.content.at(-1).id, content: 'agentnotch-smoke\n' }] },
      ],
    });
    const after = await (await call(site, withResult, undefined)).json();
    assert.equal(after.stop_reason, 'end_turn');
    assert.deepEqual(after.content, [{ type: 'text', text: 'Done.' }]);
    // A tool_result does not spend a scripted turn.
    assert.equal((await (await call(site, ask())).json()).content[0].name, 'AskUserQuestion');
    assert.equal((await (await call(site, ask())).json()).content[0].name, 'ExitPlanMode');
  });
});

test('streamed plain text after a tool_result', async () => {
  await withSite(async (site) => {
    const r = await call(site, ask({
      stream: true,
      messages: [{ role: 'user', content: [{ type: 'tool_result', tool_use_id: 'toolu_x', content: 'x' }] }],
    }));
    const events = parseSse(await r.text());
    assert.equal(events.at(-2).delta.stop_reason, 'end_turn');
    assert.equal(events.find((e) => e.delta?.type === 'text_delta').delta.text, 'Done.');
  });
});

test('side requests (max_tokens 1, no tools) get plain text and spend no turn', async () => {
  await withSite(async (site) => {
    const quota = await (await call(site, { model: 'm', max_tokens: 1, messages: [{ role: 'user', content: 'quota' }] })).json();
    assert.deepEqual(quota.content, [{ type: 'text', text: 'ok' }]);
    const title = await (await call(site, { model: 'm', max_tokens: 64, messages: [{ role: 'user', content: 'title' }] })).json();
    assert.equal(title.stop_reason, 'end_turn');
    assert.equal((await (await call(site, ask())).json()).content.at(-1).name, 'Bash');
  });
});

test('promptIncludes picks the matching turn', async () => {
  const scenario = {
    apiKey: FAKE_KEY,
    turns: [
      { content: [{ type: 'text', text: 'general' }] },
      { promptIncludes: 'plan', content: [{ type: 'tool_use', name: 'ExitPlanMode', input: { plan: 'p' } }] },
    ],
  };
  await withSite(async (site) => {
    const r = await (await call(site, ask({ messages: [{ role: 'user', content: [{ type: 'text', text: 'make a plan' }] }] }))).json();
    assert.equal(r.content[0].name, 'ExitPlanMode');
  }, scenario);
});

test('any key but the configured fake is refused with an authentication_error', async () => {
  await withSite(async (site, logFile) => {
    for (const headers of [{ 'x-api-key': 'something-else' }, {}, { authorization: 'Bearer abc' }]) {
      const r = await call(site, ask(), headers);
      assert.equal(r.status, 401);
      const body = await r.json();
      assert.equal(body.type, 'error');
      assert.equal(body.error.type, 'authentication_error');
    }
    const lines = readFileSync(logFile, 'utf8').trim().split('\n').map((l) => JSON.parse(l));
    assert.deepEqual(lines.map((l) => l.apiKey), ['bad', 'absent', 'absent']);
    assert.equal(lines[2].authorization, 'present');
    assert.ok(!readFileSync(logFile, 'utf8').includes('something-else'));
  });
});

test('other endpoints come from the route table; the rest are 404 in the API error shape', async () => {
  const scenario = { ...EXAMPLE_SCENARIO, routes: [{ method: 'GET', path: '^/api/hello$', status: 200, body: { hi: true } }] };
  await withSite(async (site) => {
    const h = { 'x-api-key': FAKE_KEY };
    assert.deepEqual(await (await fetch(`${site.url}/api/hello`, { headers: h })).json(), { hi: true });
    assert.deepEqual(await (await fetch(`${site.url}/v1/models`, { headers: h })).json(), { data: [], has_more: false, first_id: null, last_id: null });
    assert.equal((await (await call(site, ask(), h, '/v1/messages/count_tokens')).json()).input_tokens, 1);
    const nope = await fetch(`${site.url}/v1/unknown`, { headers: h });
    assert.equal(nope.status, 404);
    assert.equal((await nope.json()).error.type, 'not_found_error');
  }, scenario);
});

test('every request is logged without the key', async () => {
  await withSite(async (site, logFile) => {
    await call(site, ask({ stream: true }));
    const [line] = readFileSync(logFile, 'utf8').trim().split('\n').map((l) => JSON.parse(l));
    assert.equal(line.method, 'POST');
    assert.equal(line.path, '/v1/messages');
    assert.equal(line.status, 200);
    assert.equal(line.stream, true);
    assert.deepEqual(line.tools, ['Bash', 'AskUserQuestion', 'ExitPlanMode']);
    assert.equal(line.reply, 'turn');
    assert.ok(!JSON.stringify(line).includes(FAKE_KEY));
  });
});

test('the CLI listens on a free port and stops on SIGTERM', async () => {
  const child = spawn(process.execPath, [fileURLToPath(new URL('./fake-anthropic.mjs', import.meta.url)), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
  try {
    const [chunk] = await once(child.stdout, 'data');
    const { url } = JSON.parse(chunk.toString());
    const r = await fetch(`${url}/v1/messages`, { method: 'POST', headers: { 'x-api-key': FAKE_KEY }, body: JSON.stringify(ask()) });
    assert.equal(r.status, 200);
  } finally {
    child.kill('SIGTERM');
    await once(child, 'exit');
  }
});

test('two servers never mint the same tool_use id (real ids are unique; the panel answers an id once)', async () => {
  const idOf = async () => {
    let id;
    await withSite(async (site) => {
      const body = await (await call(site, ask())).json();
      id = body.content.find((b) => b.type === 'tool_use').id;
    });
    return id;
  };
  const [first, second] = [await idOf(), await idOf()];
  assert.match(first, /^toolu_/);
  assert.notEqual(first, second);
});
