import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { runProcess, runEntry, runFromSettings, hookEntries, plan } from './run-hook.mjs';

const dir = mkdtempSync(join(tmpdir(), 'runhook-'));
const echo = join(dir, 'echo.mjs');
writeFileSync(echo, `
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (c) => (input += c));
process.stdin.on('end', () => {
  if (process.argv[2] === 'sleep') return setTimeout(() => {}, 20000);
  process.stdout.write(JSON.stringify({ args: process.argv.slice(2), stdin: input, pid: process.env.CLAUDE_PID, dir: process.env.CLAUDE_CONFIG_DIR }));
  process.stderr.write('warn');
  process.exit(Number(process.env.EXIT ?? 0));
});
`);
const exe = process.execPath;
const have = (file, args) => spawnSync(file, args, { stdio: 'ignore' }).status === 0;
const hasBash = have('bash', ['-c', 'true']);
const hasPwsh = have(process.platform === 'win32' ? 'powershell' : 'pwsh', ['-NoProfile', '-Command', 'exit 0']);
const settingsFile = join(dir, 'settings.json');
writeFileSync(settingsFile, JSON.stringify({
  hooks: {
    PreToolUse: [{ matcher: '', hooks: [{ type: 'command', command: exe, args: [echo, 'pre', 'a b'] }] }],
    PermissionRequest: [
      { hooks: [{ type: 'command', command: exe, args: [echo, 'first'] }, { type: 'command', command: `"${exe}" "${echo}" second`, shell: 'bash' }] },
    ],
  },
  statusLine: { type: 'command', command: `"${exe}" "${echo}" status` },
}));

test('exec form: spawned directly, args kept whole, stdin, env, stderr and the result fields', async () => {
  const [r] = await runEntry({ command: exe, args: [echo, 'pre', 'a b'] }, {
    stdin: '{"hook_event_name":"PreToolUse"}', env: { CLAUDE_PID: '4242', CLAUDE_CONFIG_DIR: dir },
  }, 'PreToolUse[0]');
  assert.equal(r.shell, 'exec');
  assert.equal(r.source, 'PreToolUse[0]');
  assert.equal(r.exit, 0);
  assert.equal(r.timedOut, false);
  assert.deepEqual(JSON.parse(r.stdout), { args: ['pre', 'a b'], stdin: '{"hook_event_name":"PreToolUse"}', pid: '4242', dir });
  assert.equal(Buffer.from(r.stdout_b64, 'base64').toString('utf8'), r.stdout);
  assert.equal(r.stderr, 'warn');
  assert.ok(r.ms >= 0 && r.ms < 5000);
});

test('the exit code and non-UTF-8 bytes come through (stdout_b64 is the exact bytes)', async () => {
  const r = await runProcess(exe, ['-e', "process.stdout.write(Buffer.from([0xef,0xbb,0xbf,0x7b,0xff])); process.exit(2)"]);
  assert.equal(r.exit, 2);
  assert.equal(r.stdout_b64, Buffer.from([0xef, 0xbb, 0xbf, 0x7b, 0xff]).toString('base64'));
});

test('a command that never reads stdin does not hang or fail the run', async () => {
  const r = await runProcess(exe, ['-e', '0'], { stdin: 'x'.repeat(1_000_000) });
  assert.equal(r.exit, 0);
});

test('a timeout kills the process and says so', async () => {
  const r = await runProcess(exe, [echo, 'sleep'], { timeoutMs: 300 });
  assert.equal(r.timedOut, true);
  assert.ok(r.exit === null || r.exit !== 0);
  assert.ok(r.ms < 5000);
});

test('a missing program is skipped, not an error', async () => {
  const [r] = await runEntry({ command: 'echo hi' }, { shell: 'bash', bash: join(dir, 'no-such-bash') });
  assert.equal(r.skipped, true);
  assert.match(r.reason, /not found/);
});

test('plan: exec form never goes through a shell, string form through each shell asked for', () => {
  assert.deepEqual(plan({ command: 'a', args: ['b'] }, { shell: 'both' }), [{ shell: 'exec', file: 'a', args: ['b'] }]);
  const both = plan({ command: 'x y' }, { shell: 'both', bash: 'B', powershell: 'P' });
  assert.deepEqual(both, [
    { shell: 'bash', file: 'B', args: ['-c', 'x y'] },
    { shell: 'powershell', file: 'P', args: ['-NoProfile', '-Command', 'x y'] },
  ]);
  assert.deepEqual(plan({ command: 'x', shell: 'powershell' }, { shell: 'bash', powershell: 'P' }).map((p) => p.shell), ['powershell']);
  assert.deepEqual(plan({ command: 'x' }, { shell: 'bash', bash: 'B' }).map((p) => p.shell), ['bash']);
});

test('settings: all entries of an event, or one by index', async () => {
  assert.equal(hookEntries(JSON.parse(await import('node:fs').then((fs) => fs.readFileSync(settingsFile, 'utf8'))), 'PermissionRequest').length, 2);
  const all = await runFromSettings(settingsFile, { event: 'PermissionRequest', shell: 'bash', env: { CLAUDE_PID: '1' } });
  if (hasBash) {
    assert.deepEqual(all.map((r) => [r.source, r.shell, r.exit]), [['PermissionRequest[0]', 'exec', 0], ['PermissionRequest[1]', 'bash', 0]]);
    assert.deepEqual(JSON.parse(all[1].stdout).args, ['second']);
  } else {
    console.log('NOTICE: bash is not installed here; string-form entries are skipped');
    assert.equal(all[1].skipped, true);
  }
  const one = await runFromSettings(settingsFile, { event: 'PermissionRequest', index: 0 });
  assert.deepEqual(one.map((r) => r.source), ['PermissionRequest[0]']);
  await assert.rejects(runFromSettings(settingsFile, { event: 'PermissionRequest', index: 5 }), /no index 5/);
  await assert.rejects(runFromSettings(settingsFile, { event: 'Stop' }), /no Stop hook entries/);
});

test('settings: a file with a BOM and CRLF (how Windows editors write it) is read', async () => {
  const bomFile = join(dir, 'settings-bom.json');
  writeFileSync(bomFile, '\uFEFF' + JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: exe, args: [echo, 'bom'] }] }] } }, null, 2).replace(/\n/g, '\r\n'));
  const runs = await runFromSettings(bomFile, { event: 'Stop' });
  assert.deepEqual(runs.map((r) => [r.source, r.exit]), [['Stop[0]', 0]]);
});

test('settings: the status line runs with the status JSON on stdin', async (t) => {
  if (!hasBash) t.diagnostic('NOTICE: bash is not installed here; checking the skip');
  const [r] = await runFromSettings(settingsFile, { statusLine: true, shell: 'bash', stdin: '{"model":"x"}' });
  assert.equal(r.source, 'statusLine');
  if (hasBash) assert.deepEqual(JSON.parse(r.stdout), { args: ['status'], stdin: '{"model":"x"}' });
  else assert.equal(r.skipped, true);
});

test('string form through bash -c', { skip: !hasBash && 'bash is not installed here' }, async () => {
  const [r] = await runEntry({ command: 'printf %s "$CLAUDE_PID:$(cat)"' }, { shell: 'bash', stdin: 'in', env: { CLAUDE_PID: '7' } });
  assert.equal(r.shell, 'bash');
  assert.equal(r.stdout, '7:in');
});

test('string form through powershell -NoProfile -Command', { skip: !hasPwsh && 'no PowerShell here' }, async () => {
  const [r] = await runEntry({ command: '[Console]::Out.Write($env:CLAUDE_PID)' }, { shell: 'powershell', env: { CLAUDE_PID: '9' } });
  assert.equal(r.shell, 'powershell');
  assert.equal(r.stdout, '9');
});

test('the CLI prints {"runs": [...]} and takes env, stdin file and timeout', () => {
  const stdinFile = join(dir, 'stdin.json');
  writeFileSync(stdinFile, '{"a":1}');
  const script = fileURLToPath(new URL('./run-hook.mjs', import.meta.url));
  const run = (...a) => spawnSync(exe, [script, ...a], { encoding: 'utf8' });
  const ok = run('--exec', exe, '--arg', echo, '--arg', 'cli', '--stdin', stdinFile, '--env', 'CLAUDE_PID=5', '--env', 'EXIT=3', '--timeout', '5000');
  assert.equal(ok.status, 0, ok.stderr);
  const { runs } = JSON.parse(ok.stdout);
  assert.equal(runs.length, 1);
  assert.equal(runs[0].exit, 3);
  assert.deepEqual(JSON.parse(runs[0].stdout), { args: ['cli'], stdin: '{"a":1}', pid: '5' });
  const fromSettings = run('--settings', settingsFile, '--event', 'PreToolUse', '--index', '0');
  assert.equal(JSON.parse(fromSettings.stdout).runs[0].exit, 0);
  const bad = run('--shell', 'cmd', '--command', 'x');
  assert.equal(bad.status, 2);
  assert.match(JSON.parse(bad.stdout).error, /--shell/);
});
