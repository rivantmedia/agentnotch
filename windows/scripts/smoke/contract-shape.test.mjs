import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { validate, DEFAULT_FIXTURE } from './contract-shape.mjs';

const fixture = JSON.parse(readFileSync(DEFAULT_FIXTURE, 'utf8'));
const clone = () => structuredClone(fixture);
const NOW = Date.parse('2026-09-26T00:00:00Z');
const check = (body, options = {}) => validate(body, fixture, { now: NOW, ...options });

// What the Mac app writes: sorted keys at every level, `.mmmZ` dates.
function appStyle(value) {
  if (Array.isArray(value)) return value.map(appStyle);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map((k) => [k, appStyle(value[k])]));
  }
  if (typeof value === 'string' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(value)) return value.replace('Z', '.000Z');
  return value;
}

test('the contract fixture passes against itself', () => {
  assert.deepEqual(check(clone()), []);
});

test('an app-style body passes the strict options', () => {
  assert.deepEqual(check(appStyle(fixture), { sortedKeys: true, millisDates: true }), []);
});

test('a missing key fails', () => {
  const b = clone();
  delete b.device.name;
  assert.match(check(b).join('\n'), /device\.name: missing/);
});

test('a missing optional key (summary) is fine', () => {
  const b = clone();
  delete b.sessions[0].summary;
  assert.deepEqual(check(b), []);
});

test('an extra key fails, at any depth', () => {
  const b = clone();
  b.sessions[1].tokens.extra = 1;
  assert.match(check(b).join('\n'), /sessions\[1\]\.tokens\.extra: not in the contract/);
});

test('a wrong type fails', () => {
  const b = clone();
  b.sessions[0].messageCount = '212';
  b.usage[0].windows[0].utilization = 'high';
  const p = check(b).join('\n');
  assert.match(p, /sessions\[0\]\.messageCount: expected number, got string/);
  assert.match(p, /usage\[0\]\.windows\[0\]\.utilization: expected number, got string/);
});

test('a fraction where an integer is required fails', () => {
  const b = clone();
  b.sessions[0].tokens.input = 1.5;
  assert.match(check(b).join('\n'), /tokens\.input: not an integer/);
});

test('null is allowed only where the contract allows it', () => {
  const ok = clone();
  ok.accounts[0].email = null;
  ok.sessions[0].costUsd = null;
  ok.sessions[1].endedAt = '2026-09-25T12:00:00Z';
  ok.sessions[1].costUsd = 1.5;
  ok.usage[0].windows[2].resetsAt = '2026-09-26T00:00:00Z';
  assert.deepEqual(check(ok), []);
  const bad = clone();
  bad.sessions[0].sessionId = null;
  bad.device.id = null;
  const p = check(bad).join('\n');
  assert.match(p, /sessions\[0\]\.sessionId: null where string is expected/);
  assert.match(p, /device\.id: null where string is expected/);
});

test('sorted keys are required only when asked for', () => {
  assert.deepEqual(check(clone()), []);
  const p = check(clone(), { sortedKeys: true }).join('\n');
  assert.match(p, /keys are not sorted/);
  const b = appStyle(fixture);
  b.device = { name: b.device.name, id: b.device.id, appVersion: b.device.appVersion };
  assert.match(check(b, { sortedKeys: true }).join('\n'), /device: keys are not sorted/);
});

test('a bad date fails, and `.mmmZ` is required only when asked for', () => {
  const b = clone();
  b.sessions[0].startedAt = '2026-09-25 08:02:11';
  b.usage[0].observedAt = '2026-09-25T09:50:00+02:00';
  const p = check(b).join('\n');
  assert.match(p, /sessions\[0\]\.startedAt: not a UTC ISO 8601 date/);
  assert.match(p, /usage\[0\]\.observedAt: not a UTC ISO 8601 date/);
  assert.match(check(clone(), { millisDates: true }).join('\n'), /not a YYYY-MM-DDTHH:MM:SS\.mmmZ date/);
});

test('dates outside 2023-01-01 .. now + 1 day fail, resetsAt gets 32 days', () => {
  const b = clone();
  b.sessions[0].startedAt = '2022-12-31T23:59:59Z';
  b.sessions[1].lastActivityAt = '2026-09-28T00:00:00Z';
  b.usage[0].windows[0].resetsAt = '2026-10-20T00:00:00Z';
  b.usage[0].windows[1].resetsAt = '2026-11-01T00:00:00Z';
  const p = check(b);
  assert.equal(p.length, 3, p.join('\n'));
  assert.ok(p.every((m) => /outside the allowed date range/.test(m)));
  assert.ok(!p.some((m) => /windows\[0\]/.test(m)));
});

test('an accountKey that is not 64 lowercase hex fails', () => {
  const b = clone();
  b.accounts[0].key = b.accounts[0].key.slice(0, 63);
  b.sessions[1].accountKey = b.sessions[1].accountKey.toUpperCase();
  b.sessions[0].project.key = 'xyz';
  const p = check(b).join('\n');
  assert.match(p, /accounts\[0\]\.key: not 64 lowercase hex/);
  assert.match(p, /sessions\[1\]\.accountKey: not 64 lowercase hex/);
  assert.match(p, /sessions\[0\]\.project\.key: not 64 lowercase hex/);
});

test('an accountKey missing from accounts[] fails', () => {
  const b = clone();
  b.usage[1].accountKey = '0'.repeat(64);
  assert.match(check(b).join('\n'), /usage\[1\]\.accountKey: not listed in accounts\[\]/);
});

test('every array element is checked against the first fixture element', () => {
  const b = clone();
  b.sessions.push({ ...clone().sessions[1], extra: true });
  assert.match(check(b).join('\n'), /sessions\[2\]\.extra: not in the contract/);
});

test('the CLI prints JSON and its exit code says the verdict', () => {
  const dir = mkdtempSync(join(tmpdir(), 'shape-'));
  const good = join(dir, 'good.json');
  const bad = join(dir, 'bad.json');
  writeFileSync(good, JSON.stringify(appStyle(fixture)));
  const b = clone();
  delete b.device;
  writeFileSync(bad, JSON.stringify(b));
  const script = fileURLToPath(new URL('./contract-shape.mjs', import.meta.url));
  const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' });
  const g = run(good, '--sorted', '--millis');
  assert.equal(g.status, 0, g.stdout);
  assert.deepEqual(JSON.parse(g.stdout), { ok: true, problems: [] });
  const r = run(bad);
  assert.equal(r.status, 1);
  assert.equal(JSON.parse(r.stdout).ok, false);
});
