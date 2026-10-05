import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { startFakeWebsite } from './fake-website.mjs';
import { validate, DEFAULT_FIXTURE } from './contract-shape.mjs';

const fixtures = (name) => JSON.parse(readFileSync(new URL(`../../../web/contract/fixtures/${name}`, import.meta.url), 'utf8'));
const post = (url, body, headers = {}) =>
  fetch(url, { method: 'POST', headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(body) });

async function withSite(fn) {
  const dir = mkdtempSync(join(tmpdir(), 'fakeweb-'));
  const logFile = join(dir, 'log.jsonl');
  const syncDir = join(dir, 'sync');
  const site = await startFakeWebsite({ logFile, syncDir });
  try {
    await fn(site, { logFile, syncDir });
  } finally {
    await site.close();
  }
}
const lines = (file) => readFileSync(file, 'utf8').trim().split('\n').map((l) => JSON.parse(l));

test('config is the fixture with supabaseUrl pointing at the server itself', async () => {
  await withSite(async (site) => {
    const r = await fetch(`${site.url}/api/app/v1/config`);
    assert.equal(r.status, 200);
    const body = await r.json();
    assert.equal(body.supabaseUrl, site.url);
    const want = fixtures('config.json');
    assert.deepEqual({ ...body, supabaseUrl: want.supabaseUrl }, want);
  });
});

test('the PKCE exchange needs the code and the verifier and answers a session', async () => {
  await withSite(async (site) => {
    const bad = await post(`${site.url}/auth/v1/token?grant_type=pkce`, { auth_code: 'smoke' });
    assert.equal(bad.status, 400);
    const r = await post(`${site.url}/auth/v1/token?grant_type=pkce`, { auth_code: 'smoke', code_verifier: 'v'.repeat(43) }, { apikey: 'k' });
    assert.equal(r.status, 200);
    const s = await r.json();
    assert.ok(s.access_token && s.refresh_token);
    assert.equal(s.token_type, 'bearer');
    assert.ok(s.expires_at > Date.now() / 1000);
    assert.equal(s.user.email, fixtures('me.json').user.email);
  });
});

test('refresh rotates: a refresh token works once', async () => {
  await withSite(async (site) => {
    const first = await (await post(`${site.url}/auth/v1/token?grant_type=pkce`, { auth_code: 'a', code_verifier: 'b' })).json();
    const ok = await post(`${site.url}/auth/v1/token?grant_type=refresh_token`, { refresh_token: first.refresh_token });
    assert.equal(ok.status, 200);
    const second = await ok.json();
    assert.notEqual(second.refresh_token, first.refresh_token);
    const again = await post(`${site.url}/auth/v1/token?grant_type=refresh_token`, { refresh_token: first.refresh_token });
    assert.equal(again.status, 400);
    assert.equal((await again.json()).error_code, 'refresh_token_not_found');
  });
});

test('me and sync need a token this server issued; logout revokes it', async () => {
  await withSite(async (site) => {
    assert.equal((await fetch(`${site.url}/api/app/v1/me`)).status, 401);
    const bad = await fetch(`${site.url}/api/app/v1/me`, { headers: { authorization: 'Bearer nope' } });
    assert.equal(bad.status, 401);
    assert.equal((await bad.json()).error.code, 'UNAUTHORIZED');

    const s = await (await post(`${site.url}/auth/v1/token?grant_type=pkce`, { auth_code: 'a', code_verifier: 'b' })).json();
    const auth = { authorization: `Bearer ${s.access_token}` };
    const me = await fetch(`${site.url}/api/app/v1/me`, { headers: auth });
    assert.deepEqual(await me.json(), fixtures('me.json'));

    const out = await post(`${site.url}/auth/v1/logout?scope=local`, {}, auth);
    assert.equal(out.status, 204);
    assert.equal((await fetch(`${site.url}/api/app/v1/me`, { headers: auth })).status, 401);
  });
});

test('sync stores the body, answers the fixture and writes the body to the sync dir', async () => {
  await withSite(async (site, { syncDir }) => {
    const s = await (await post(`${site.url}/auth/v1/token?grant_type=pkce`, { auth_code: 'a', code_verifier: 'b' })).json();
    const body = fixtures('sync-request.json');
    const r = await post(`${site.url}/api/app/v1/sync`, body, { authorization: `Bearer ${s.access_token}` });
    assert.equal(r.status, 200);
    assert.deepEqual(await r.json(), fixtures('sync-response.json'));
    assert.equal(site.syncBodies.length, 1);
    assert.deepEqual(site.syncBodies[0], body);
    const stored = JSON.parse(readFileSync(join(syncDir, 'sync-1.json'), 'utf8'));
    assert.deepEqual(validate(stored, JSON.parse(readFileSync(DEFAULT_FIXTURE, 'utf8')), { now: Date.parse('2026-09-26T00:00:00Z') }), []);

    const junk = await fetch(`${site.url}/api/app/v1/sync`, { method: 'POST', headers: { authorization: `Bearer ${s.access_token}` }, body: '{' });
    assert.equal(junk.status, 400);
    assert.equal((await junk.json()).error.code, 'BAD_REQUEST');
    assert.equal(site.syncBodies.length, 1);
  });
});

test('unknown routes answer the contract error shape', async () => {
  await withSite(async (site) => {
    const r = await fetch(`${site.url}/nothing`);
    assert.equal(r.status, 404);
    assert.equal((await r.json()).error.code, 'NOT_FOUND');
  });
});

test('every request is logged as a JSON line: names of headers, Authorization only as "present"', async () => {
  await withSite(async (site, { logFile }) => {
    await fetch(`${site.url}/api/app/v1/config`);
    await post(`${site.url}/auth/v1/token?grant_type=pkce`, { auth_code: 'secret-code', code_verifier: 'secret-verifier' }, { apikey: 'publishable-value' });
    await fetch(`${site.url}/api/app/v1/me`, { headers: { authorization: 'Bearer very-secret' } });
    const log = lines(logFile);
    assert.equal(log.length, 3);
    assert.deepEqual(log.map((l) => [l.method, l.path, l.status]), [
      ['GET', '/api/app/v1/config', 200],
      ['POST', '/auth/v1/token', 200],
      ['GET', '/api/app/v1/me', 401],
    ]);
    assert.deepEqual(log[1].query, { grant_type: 'pkce' });
    assert.ok(log[1].headers.includes('apikey'));
    assert.equal(log[2].authorization, 'present');
    assert.equal(log[0].authorization, undefined);
    const raw = readFileSync(logFile, 'utf8');
    for (const secret of ['very-secret', 'secret-code', 'secret-verifier', 'publishable-value']) {
      assert.ok(!raw.includes(secret), secret);
    }
  });
});

test('the CLI picks a port, prints it, serves, and stops on SIGTERM', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'fakeweb-cli-'));
  const logFile = join(dir, 'log.jsonl');
  const child = spawn(process.execPath, [fileURLToPath(new URL('./fake-website.mjs', import.meta.url)), '--port', '0', '--log', logFile], { stdio: ['ignore', 'pipe', 'inherit'] });
  try {
    const [chunk] = await once(child.stdout, 'data');
    const { port, url } = JSON.parse(chunk.toString());
    assert.ok(port > 0);
    assert.equal(url, `http://127.0.0.1:${port}`);
    const r = await fetch(`${url}/api/app/v1/config`);
    assert.equal((await r.json()).supabaseUrl, url);
    assert.ok(existsSync(logFile));
  } finally {
    child.kill('SIGTERM');
    await once(child, 'exit');
  }
});
