// A fake Agent Notch website plus the Supabase endpoints the app signs in through, for the
// installer smoke test (phase 10). It answers from web/contract/fixtures (read in place) and
// logs every request as one JSON line. Nothing here is a real token: the sessions it hands out
// are plain strings that only this server accepts.
//
//   node fake-website.mjs [--port 0] [--log <file.jsonl>] [--sync-dir <dir>] [--fixtures <dir>]
//
// Prints {"port":N,"url":"http://127.0.0.1:N"} when it listens, runs until it is signalled
// (or, with `--stop-on-stdin-close 1`, until stdin closes). Routes: GET /api/app/v1/config, POST /auth/v1/token (grant_type=pkce and
// refresh_token), POST /auth/v1/logout, GET /api/app/v1/me, POST /api/app/v1/sync.
import http from 'node:http';
import { readFileSync, appendFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const DEFAULT_FIXTURES = resolve(here, '../../../web/contract/fixtures');

const ERROR_STATUS = { UNAUTHORIZED: 401, BAD_REQUEST: 400, NOT_FOUND: 404, INTERNAL: 500 };

export async function startFakeWebsite({ port = 0, logFile, syncDir, fixturesDir = DEFAULT_FIXTURES, host = '127.0.0.1' } = {}) {
  const fixture = (name) => JSON.parse(readFileSync(join(fixturesDir, name), 'utf8'));
  const state = {
    requests: [],
    syncBodies: [],
    access: new Set(), // access tokens this server issued and has not revoked
    refresh: new Set(), // refresh tokens that may still be redeemed (they rotate)
    issued: 0,
    url: '',
  };

  function issue() {
    const n = ++state.issued;
    const access = `smoke-access-${n}`;
    const refresh = `smoke-refresh-${n}`;
    state.access.add(access);
    state.refresh.add(refresh);
    const me = fixture('me.json').user;
    return {
      access_token: access,
      token_type: 'bearer',
      expires_in: 3600,
      expires_at: Math.floor(Date.now() / 1000) + 3600,
      refresh_token: refresh,
      user: { id: me.id, email: me.email, aud: 'authenticated', role: 'authenticated' },
    };
  }

  const bearer = (req) => /^Bearer (.+)$/.exec(req.headers.authorization ?? '')?.[1];
  const authorised = (req) => state.access.has(bearer(req));

  async function readBody(req) {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    return Buffer.concat(chunks).toString('utf8');
  }

  async function route(req, url, send) {
    const key = `${req.method} ${url.pathname}`;
    const fail = (code, message) => send(ERROR_STATUS[code], { error: { code, message } });
    if (key === 'GET /api/app/v1/config') {
      return send(200, { ...fixture('config.json'), supabaseUrl: state.url });
    }
    if (key === 'POST /auth/v1/token') {
      let body;
      try {
        body = JSON.parse((await readBody(req)) || '{}');
      } catch {
        return send(400, { error: 'bad_json', error_description: 'The body is not JSON' });
      }
      const grant = url.searchParams.get('grant_type');
      if (grant === 'pkce') {
        if (!body.auth_code || !body.code_verifier) {
          return send(400, { error: 'invalid_grant', error_code: 'validation_failed', error_description: 'auth_code and code_verifier are required' });
        }
        return send(200, issue());
      }
      if (grant === 'refresh_token') {
        if (!state.refresh.delete(body.refresh_token)) {
          return send(400, { error: 'invalid_grant', error_code: 'refresh_token_not_found', error_description: 'Invalid Refresh Token: Refresh Token Not Found' });
        }
        return send(200, issue());
      }
      return send(400, { error: 'unsupported_grant_type', error_description: `grant_type ${grant}` });
    }
    if (key === 'POST /auth/v1/logout') {
      const token = bearer(req);
      if (!token) return send(401, { error: 'no_authorization', error_description: 'This endpoint requires a Bearer token' });
      state.access.delete(token);
      return send(204, null);
    }
    if (key === 'GET /api/app/v1/me') {
      if (!authorised(req)) return fail('UNAUTHORIZED', 'Sign in again.');
      return send(200, fixture('me.json'));
    }
    if (key === 'POST /api/app/v1/sync') {
      if (!authorised(req)) return fail('UNAUTHORIZED', 'Sign in again.');
      let body;
      try {
        body = JSON.parse(await readBody(req));
      } catch {
        return fail('BAD_REQUEST', 'The body is not JSON.');
      }
      state.syncBodies.push(body);
      if (syncDir) {
        mkdirSync(syncDir, { recursive: true });
        writeFileSync(join(syncDir, `sync-${state.syncBodies.length}.json`), JSON.stringify(body));
      }
      return send(200, fixture('sync-response.json'));
    }
    return fail('NOT_FOUND', `No route for ${key}`);
  }

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://fake.invalid');
    const entry = {
      method: req.method,
      path: url.pathname,
      query: Object.fromEntries(url.searchParams),
      headers: Object.keys(req.headers).sort(), // names only: a value could be a secret
    };
    if (req.headers.authorization) entry.authorization = 'present';
    const send = (status, body) => {
      entry.status = status;
      // Logged before the answer leaves, so a client that saw the answer finds the line.
      state.requests.push(entry);
      if (logFile) appendFileSync(logFile, `${JSON.stringify(entry)}\n`);
      const text = body === null ? '' : JSON.stringify(body);
      res.writeHead(status, text ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(text) } : {});
      res.end(text);
    };
    try {
      await route(req, url, send);
    } catch (e) {
      send(500, { error: { code: 'INTERNAL', message: String(e?.message ?? e) } });
    }
  });

  await new Promise((ok, bad) => server.once('error', bad).listen(port, host, ok));
  const actual = server.address().port;
  state.url = `http://${host}:${actual}`;
  return {
    ...state,
    port: actual,
    url: state.url,
    state,
    close: () => new Promise((ok) => { server.closeAllConnections?.(); server.close(ok); }),
  };
}

function main(argv) {
  const opt = {};
  for (let i = 0; i < argv.length; i += 2) opt[argv[i].replace(/^--/, '')] = argv[i + 1];
  startFakeWebsite({
    port: Number(opt.port ?? 0),
    logFile: opt.log,
    syncDir: opt['sync-dir'],
    fixturesDir: opt.fixtures,
  }).then((site) => {
    console.log(JSON.stringify({ port: site.port, url: site.url }));
    const stop = () => site.close().then(() => process.exit(0));
    process.on('SIGTERM', stop);
    process.on('SIGINT', stop);
    // Opt-in: a process started without a console has a closed stdin from the first moment.
    if (opt['stop-on-stdin-close'] === '1') process.stdin.on('end', stop).resume();
  }, (e) => {
    console.error(JSON.stringify({ error: String(e?.message ?? e) }));
    process.exit(1);
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main(process.argv.slice(2));
