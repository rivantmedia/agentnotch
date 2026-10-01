// The hermetic job's scenario files drive the fake Messages API as real-claude.ps1 expects, and
// its env probe records what a hook was handed. Host-only: nothing here runs Claude Code.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { FAKE_KEY, startFakeAnthropic } from '../fake-anthropic.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const scenario = (name) => JSON.parse(readFileSync(join(here, `scenario-${name}.json`), 'utf8'));
const tools = [{ name: 'Bash' }, { name: 'AskUserQuestion' }, { name: 'ExitPlanMode' }];

async function ask(server, prompt, extra = {}) {
  const r = await fetch(`${server.url}/v1/messages?beta=true`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-api-key': FAKE_KEY },
    body: JSON.stringify({ model: 'claude-x', max_tokens: 1000, tools, messages: [{ role: 'user', content: [{ type: 'text', text: prompt }] }], ...extra }),
  });
  assert.equal(r.status, 200);
  return r.json();
}

test('every scenario uses the fake key the fake API accepts', () => {
  for (const name of ['headless', 'plan', 'interactive']) assert.equal(scenario(name).apiKey, FAKE_KEY, name);
});

test('headless: each marker gets its own turn, whatever the order', async () => {
  const server = await startFakeAnthropic({ scenario: scenario('headless') });
  try {
    const question = await ask(server, 'reminder text\n[question] pick one');
    assert.equal(question.content.find((b) => b.type === 'tool_use').name, 'AskUserQuestion');
    const allow = await ask(server, '[allow] make the file');
    const bash = allow.content.find((b) => b.type === 'tool_use');
    assert.equal(bash.name, 'Bash');
    assert.equal(bash.input.command, 'touch real-claude-allowed.txt');
    const deny = await ask(server, '[deny] make the other file');
    assert.equal(deny.content.find((b) => b.type === 'tool_use').input.command, 'touch real-claude-denied.txt');
    const after = await ask(server, 'x', { messages: [{ role: 'user', content: [{ type: 'tool_result', tool_use_id: bash.id, content: 'ok' }] }] });
    assert.equal(after.stop_reason, 'end_turn');
  } finally {
    await server.close();
  }
});

test('plan: an ExitPlanMode with a plan; interactive: plain text only', async () => {
  const plan = await startFakeAnthropic({ scenario: scenario('plan') });
  try {
    const reply = await ask(plan, '[plan] go');
    const use = reply.content.find((b) => b.type === 'tool_use');
    assert.equal(use.name, 'ExitPlanMode');
    assert.ok(use.input.plan.length > 0);
  } finally {
    await plan.close();
  }
  const chat = await startFakeAnthropic({ scenario: scenario('interactive') });
  try {
    const reply = await ask(chat, '[hello]');
    assert.deepEqual(reply.content.map((b) => b.type), ['text']);
    assert.equal(reply.stop_reason, 'end_turn');
  } finally {
    await chat.close();
  }
});

test('the env probe appends what the hook was handed and prints nothing', () => {
  const out = join(mkdtempSync(join(tmpdir(), 'env-probe-')), 'probe.jsonl');
  const stdin = JSON.stringify({ hook_event_name: 'PreToolUse', tool_name: 'Bash', session_id: 's-1' });
  for (const form of ['string', 'exec']) {
    const r = spawnSync(process.execPath, [join(here, 'env-probe.mjs'), out, form], {
      input: stdin, env: { ...process.env, CLAUDE_PID: '1234', CLAUDE_CONFIG_DIR: 'C:\\p\\.claude-real', CLAUDE_CODE_ENTRYPOINT: 'sdk-cli' },
    });
    assert.equal(r.status, 0);
    assert.equal(r.stdout.length, 0);
  }
  const garbage = spawnSync(process.execPath, [join(here, 'env-probe.mjs'), out, 'string'], { input: 'not json' });
  assert.equal(garbage.status, 0);
  const lines = readFileSync(out, 'utf8').trim().split('\n').map((l) => JSON.parse(l));
  assert.equal(lines.length, 3);
  assert.deepEqual(lines.map((l) => l.form), ['string', 'exec', 'string']);
  assert.equal(lines[0].claude_pid, '1234');
  assert.equal(lines[0].claude_config_dir, 'C:\\p\\.claude-real');
  assert.equal(lines[1].hook_event_name, 'PreToolUse');
  assert.equal(lines[2].hook_event_name, null);
});
