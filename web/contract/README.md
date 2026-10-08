# Agent Notch app ⇄ website API, version 1

The contract between the Mac app (`Packages/ClaudeControl`, `Engine/Services/Cloud`) and the
website (`web/`, route handlers under `src/app/api/app/v1/`). Both sides test against the JSON
files in `fixtures/`: the website validates them with its zod schemas, and the app's tests
check that what it encodes has the same shape. Change a fixture only together with both sides.

## Authentication

- The app signs in with Supabase Auth (Google provider) using PKCE in an
  `ASWebAuthenticationSession`, redirect `agentnotch://auth-callback`, and keeps the Supabase
  session (access token, rotating refresh token) in `cloud-session.json` in its support folder
  (a 0700 folder; the file is 0600). It never reads or stores any Claude credential.
- Every endpoint except `config` needs `Authorization: Bearer <Supabase access token>`. The
  website verifies it against the project's JWKS (`<SUPABASE_URL>/auth/v1/.well-known/jwks.json`,
  issuer `<SUPABASE_URL>/auth/v1`, audience `authenticated`), and falls back to
  `SUPABASE_JWT_SECRET` (HS256) when that is set. The user id is the token's `sub`.
- The first authenticated call creates the user's row (id = `sub`, email and name from the token).

## Conventions

- JSON, UTF-8, `Content-Type: application/json`. Dates are ISO 8601 strings in UTC with a `Z`
  suffix; fractional seconds are allowed. Unknown fields are ignored on both sides.
- Errors: HTTP 4xx/5xx with `{"error": {"code": "...", "message": "..."}}` where `code` is one of
  `UNAUTHORIZED` (401), `FORBIDDEN` (403), `BAD_REQUEST` (400), `PAYLOAD_TOO_LARGE` (413),
  `RATE_LIMITED` (429), `INTERNAL` (500, or 503 when the website couldn't check the token right
  now, e.g. its signing keys didn't load: not a verdict on the token, so keep the session and
  retry after `Retry-After`). 401 means the token itself is bad or expired. See
  `fixtures/error.json`.
- Keys that identify a Claude account or a project are lowercase hex SHA-256 digests (64
  characters), computed on the Mac:
  - `accountKey` = SHA-256 of `<accountUuid>/<organizationUuid>`, lowercased, both from
    `oauthAccount` in `.claude.json`; SHA-256 of the lowercased `accountUuid` alone only when the
    organization is unknown. It never depends on what else is signed in on the Mac, so the same
    Claude account in the same organization has the same key for every user and machine, which
    is what pooling joins on. Folders with no signed-in account are never sent.
  - `project.key` = HMAC-SHA256, keyed with a random 32-byte secret generated once per install
    and never sent (`cloud-install-secret` in the app's support folder, 0600), of
    `<accountKey>:<project path>`, where the path is the session's working directory with `~`
    expanded, symlinks resolved and no trailing slash. It is stable per install and cannot be
    turned back into a path by guessing. The website never shows it to anyone.

## `GET /api/app/v1/config` (no auth)

What the app needs to start a sign-in. See `fixtures/config.json`.

| Field | Type | |
|---|---|---|
| `supabaseUrl` | string (URL) | `NEXT_PUBLIC_SUPABASE_URL` |
| `supabasePublishableKey` | string | `NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY` (the public key; never the secret one) |
| `redirectUrl` | string | always `agentnotch://auth-callback` |
| `dashboardUrl` | string (URL) | where "Open dashboard" goes |

## `GET /api/app/v1/me`

The signed-in user. See `fixtures/me.json`: `{"user": {"id", "email", "name"}, "dashboardUrl"}`.
`name` may be null.

## `POST /api/app/v1/sync`

Uploads what the app knows. Idempotent: sessions carry absolute totals and replace what the
server has for `(user, accountKey, sessionId)`; usage readings are deduplicated by
`(user, accountKey, source, window id, observedAt)`. A Claude Code session resumed under a
different account (Claude Parallel Profiles shares session history between folders) is sent
once per account, each with only the responses made while that account ran it. See `fixtures/sync-request.json` and
`fixtures/sync-response.json`. The website counts each part in its own account's figures, and a
resumed session once in any total that spans accounts (a project's, the usage page's), so such a
total's sessions can be fewer than its accounts' added up; tokens and cost add up exactly.

Request:

| Field | Type | Limits |
|---|---|---|
| `schemaVersion` | `1` | |
| `device.id` | string (UUID, generated once per Mac) | |
| `device.name` | string | ≤ 120 chars |
| `device.appVersion` | string | ≤ 40 chars |
| `accounts[]` | the Claude accounts this batch refers to | ≤ 50 |
| `accounts[].key` | accountKey | 64 hex |
| `accounts[].email` | string or null | ≤ 320 chars |
| `accounts[].organizationName` | string or null | ≤ 200 chars |
| `accounts[].plan` | string or null (e.g. "Max 20x", "Pro") | ≤ 60 chars |
| `accounts[].label` | string or null (the user's name for it in the app) | ≤ 80 chars |
| `sessions[]` | | ≤ 200 per request |
| `sessions[].accountKey` | accountKey; must appear in `accounts[]` | |
| `sessions[].sessionId` | Claude Code's session id (UUID) | |
| `sessions[].project.key` | project key | 64 hex |
| `sessions[].project.name` | the working directory's last path component | ≤ 120 chars |
| `sessions[].title` | string or null (Claude Code's title for the session; never the prompt) | ≤ 200 chars |
| `sessions[].source` | `"cli"`, `"vscode"`, `"desktop"`, `"sdk"` or `"other"` | |
| `sessions[].models` | model ids seen in the session, most used first | ≤ 10 |
| `sessions[].startedAt` | date | every date in a request lies between 2023-01-01 and one day after the server's clock, except `usage[].windows[].resetsAt`, which may be up to 32 days ahead |
| `sessions[].lastActivityAt` | date | |
| `sessions[].endedAt` | date or null (null while the session runs) | |
| `sessions[].messageCount` | integer ≥ 0 (assistant responses counted once) | |
| `sessions[].tokens` | `{input, output, cacheCreation, cacheRead}`, integers ≥ 0, subagents included | |
| `sessions[].costUsd` | number or null: an estimate. Claude Code's own when its status line reported one for a session one account ran, unless the app's own is larger (the session outgrew it); else the app's, from the transcript's responses at list prices. Null when it can't be priced (a model with no known price, or an app from before the estimates) | |
| `sessions[].summary` | optional; see below | |
| `usage[]` | usage-limit readings | ≤ 500 per request |
| `usage[].accountKey` | accountKey; must appear in `accounts[]` | |
| `usage[].source` | `"probe"`, `"statusLine"`, `"claudeJson"` or `"desktop"` | |
| `usage[].observedAt` | date the reading was taken by Claude Code or Claude Desktop | |
| `usage[].windows[]` | `{id, utilization, resetsAt}`: id `session`, `weekly_all`, `extra_usage` or `weekly_<model>` matching `weekly_[a-z0-9._-]{1,60}`; utilization 0–100 (may exceed 100); resetsAt date or null | ≤ 20 |

`sessions[].summary`, only when the user turned session summaries on:

- field absent: the server keeps whatever summary it has;
- `{"text": "...", "model": "...", "generatedAt": date}`: text ≤ 2000 chars, model ≤ 80 chars.

Response `200`: `{"accepted": {"sessions": <int>, "usage": <int>}, "serverTime": <date>}`.

## Pooling (website only)

A user may create a share code for an account they have synced (a `UserAccount` row exists).
Codes are 12 Crockford base32 characters (shown as `XXXX-XXXX-XXXX`), expire after 7 days, and
are revoked when the creator removes a member. Another signed-in user redeems one on the
website; failed attempts are rate-limited per user. Pool members see every member's sessions and
usage for that account, and nothing else of each other's. The creator can revoke the code or
remove members; members can leave. The app links to `<dashboardUrl>/pools` and does not call
pool endpoints itself.

The same Claude session can reach the website from more than one person: the app remembers what
it sent per website user, so a Mac signed in as one person and later as another sends its
sessions again. Wherever a member sees more than one copy of a session (same `accountKey` and
`sessionId`), it counts and is listed once, from one copy: the latest `lastActivityAt`, then an
ended one over one still running, then the larger `messageCount`, then the copy synced last, then
the smallest user id. Only copies the member can see take part, so a copy outside their pools
never changes what they see. The session shows that copy's summary, or when it has none the
latest one another copy the member sees carries. A project (one person's folders of one name)
whose sessions are all other people's copies is not listed, and a link to it opens the folder
those copies count in. Copies are never merged or deleted: each person still holds
and can delete their own rows.
