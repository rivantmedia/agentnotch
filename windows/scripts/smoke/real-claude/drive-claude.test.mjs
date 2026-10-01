// drive-claude.mjs against a stand-in that speaks stream-json (never a real Claude Code).
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { BASE_ARGS, driveClaude, succeeded, userMessage } from './drive-claude.mjs';

const dir = mkdtempSync(join(tmpdir(), 'drive-claude-'));
const standIn = join(dir, 'stand-in.mjs');
// Prints init on the first message, a control_request when the text says so, then a result per
// message; records its argv; exits 0 when stdin ends (or hangs when asked to).
writeFileSync(standIn, `
import { writeFileSync } from 'node:fs';
writeFileSync(process.env.STAND_IN_ARGV, JSON.stringify(process.argv.slice(2)));
let buffer = '', first = true;
const out = (o) => process.stdout.write(JSON.stringify(o) + '\\n');
process.stdin.setEncoding('utf8');
process.stdin.on('data', (c) => {
  buffer += c;
  let i;
  while ((i = buffer.indexOf('\\n')) >= 0) {
    const m = JSON.parse(buffer.slice(0, i)); buffer = buffer.slice(i + 1);
    const text = m.message.content[0].text;
    if (first) { out({ type: 'system', subtype: 'init', session_id: 'sess-1' }); first = false; }
    if (text.includes('ask')) out({ type: 'control_request', request_id: 'r1', request: { subtype: 'can_use_tool' } });
    if (text.includes('hang')) return;
    process.stdout.write('not json\\n');
    out({ type: 'result', subtype: 'success', is_error: text.includes('fail'), session_id: 'sess-1' });
  }
});
process.stdin.on('end', () => process.exit(Number(process.env.STAND_IN_EXIT ?? 0)));
`);
const viaNode = (_cmd, args, opts) => spawn(process.execPath, [standIn, ...args], opts);

function run(name, prompts, extra = {}) {
  const events = join(dir, `${name}.jsonl`);
  const state = join(dir, `${name}.state.json`);
  const argvFile = join(dir, `${name}.argv.json`);
  return driveClaude({
    claude: 'claude.exe', cwd: dir, prompts, eventsFile: events, stateFile: state, spawnImpl: viaNode,
    env: { ...process.env, STAND_IN_ARGV: argvFile, ...(extra.env ?? {}) }, args: extra.args ?? [], timeoutMs: extra.timeoutMs ?? 20000,
  }).then((s) => ({ s, events, state, argv: JSON.parse(readFileSync(argvFile, 'utf8')) }));
}

test('a user message is one stream-json line', () => {
  const line = userMessage('hi');
  assert.ok(line.endsWith('\n'));
  assert.deepEqual(JSON.parse(line), { type: 'user', message: { role: 'user', content: [{ type: 'text', text: 'hi' }] }, parent_tool_use_id: null, session_id: '' });
});

test('prompts go one at a time, each after the previous result; the session id and pid are recorded', async () => {
  const { s, events, state, argv } = await run('order', ['one [ask]', 'two'], { args: ['--permission-mode', 'plan'] });
  assert.deepEqual(argv, [...BASE_ARGS, '--permission-mode', 'plan']);
  assert.equal(s.session_id, 'sess-1');
  assert.equal(s.sent, 2);
  assert.equal(s.results.length, 2);
  assert.equal(s.control_requests, 1);
  assert.equal(s.exit, 0);
  assert.ok(Number.isInteger(s.pid));
  assert.ok(succeeded(s, 2));
  const lines = readFileSync(events, 'utf8').trim().split('\n');
  assert.equal(lines.filter((l) => l.includes('"result"')).length, 2);
  assert.ok(lines.includes('not json'), 'every line is kept, JSON or not');
  assert.deepEqual(JSON.parse(readFileSync(state, 'utf8')), s);
});

test('the permission request is never answered by the driver', async () => {
  const { events } = await run('unanswered', ['[ask] once']);
  const written = readFileSync(events, 'utf8');
  assert.match(written, /control_request/);
  assert.doesNotMatch(written, /control_response/);
});

test('an error result, a non-zero exit and a timeout are failures', async () => {
  const failed = await run('failed', ['please fail']);
  assert.equal(succeeded(failed.s, 1), false);
  const exited = await run('exited', ['fine'], { env: { STAND_IN_EXIT: '3' } });
  assert.equal(exited.s.exit, 3);
  assert.equal(succeeded(exited.s, 1), false);
  const hung = await run('hung', ['hang here'], { timeoutMs: 1500 });
  assert.equal(hung.s.timedOut, true);
  assert.equal(succeeded(hung.s, 1), false);
});

test('a program that cannot start is reported, not thrown', async () => {
  const s = await driveClaude({ claude: join(dir, 'missing.exe'), cwd: dir, prompts: ['x'], timeoutMs: 5000 });
  assert.ok(s.error, 'the spawn error is in the state');
  assert.equal(succeeded(s, 1), false);
});
