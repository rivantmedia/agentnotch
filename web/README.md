# Agent Notch website

Sign in with Google, then see what your Claude accounts were used for: each account's 5-hour
and weekly limits, the projects it worked in, and the tokens and cost of every Claude Code
session, synced from every Mac running Agent Notch. Someone else who uses the same Claude
account can join yours with a share code, and then you both see that account's history.

Stack: Next.js 15 (App Router), tRPC v11 + superjson + React Query, Prisma on Supabase Postgres,
Supabase Auth (Google), Tailwind v4, TypeScript. Scaffolded with create-t3-app 7.40.0 (no
NextAuth).

## What the Mac app sends

Names and numbers only:

- the project folder's name (never its path);
- session titles, model ids, start and end times;
- token counts and Claude Code's cost estimate;
- usage-limit readings from Claude Code and Claude Desktop;
- each account's email, organization, plan and the user's own label for it;
- the Mac's name and the app version.

Session summaries arrive only when the user turned them on in the app. Prompts, replies, file
paths, file contents and Claude credentials are never sent. Accounts are identified by SHA-256
keys and projects by HMAC keys made with a secret that never leaves the Mac, so a project key
can't be turned back into a path; the website never shows project keys to anyone. The exact
format is the contract in [`contract/README.md`](contract/README.md), with fixtures that both
sides test against.

**Projects are grouped by name.** Each install of the app has its own secret, so the website
can't tell that two Macs worked in the same folder: each Mac's folder is its own `Project` row,
and a Mac that loses its secret (a new support folder) starts new rows. Project lists group one
person's rows on an account by folder name: a group adds up their sessions and tokens, takes the
latest use and counts the Macs its sessions came from, and the project filter picks the whole
group (a link may name any of its rows). Two different folders with the same name are grouped
too. The rows themselves, and the contract, stay per Mac. When a sync moves a session to another
project row (its Mac's secret changed), the row it left is deleted in the same transaction once
no session uses it. The account page's projects table says all this.

**Usage by project.** The dashboard, each account page and `/projects` split the tokens, cost and
sessions of a period (the last 7 or 30 days, or all time; `?period=7d|30d|all`, 7 days when left
out) by project, and each project's page (`/projects/<id>`) splits the same by account. A session
counts in a period by its start, as in the account cards' totals. Here a project is one person's
folders of one name across accounts as well as Macs: a project key is made from the account's key
too, so the website can't tell that two accounts worked in the same folder either, and groups by
name (`src/server/services/project-usage.ts`). A pool member's projects count only on the
accounts shared with you. Usage limits are per account and nothing reports them per project, so
these are shares of the tokens, never of a limit, and the pages say so. A project's sessions
across accounts are `sessions.list` with `acrossAccounts`; the project row it names must be one
you can see.

## Pages

| Path                   | What it shows                                                                                                                                                                                                                                         |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/`                    | What sync is, sign-in, and what the app sends                                                                                                                                                                                                         |
| `/login`               | "Continue with Google"; explains `?error=` from a failed sign-in                                                                                                                                                                                      |
| `/dashboard`           | One card per visible Claude account, with limits, 7-day totals and a pooled badge. Usage by project across all accounts (the first 8, the rest added up). Also the most recent sessions. With nothing synced yet, it shows how to connect the Mac app |
| `/accounts/<key>`      | 30-day usage charts per limit, with a table view. Usage by project on the account (every project in the period). The projects table (grouped by folder name, Macs counted). Sessions filtered by project and member (`?project=` / `?member=`)        |
| `/projects`            | Usage by project across all accounts: every project with sessions in the period, with each one's accounts                                                                                                                                             |
| `/projects/<id>`       | One project on every account you see it on: its usage by account, and its sessions. Any of its rows' ids names it; one you can't see answers 404                                                                                                      |
| `/pools`               | Create, copy and revoke share codes (they last 7 days). See members by email and remove them. Join with a code, or leave                                                                                                                              |
| `/settings`            | The signed-in email, the website address for the app, the Macs that synced. Removing your summaries, or all your synced data                                                                                                                          |
| `/download`            | The latest release's file for each platform, the visitor's marked; Windows and Linux read "Not available yet" until a release carries a file for them. How to open the app the first time, and how updates arrive. Public                             |
| `/download/<platform>` | `mac`, `windows` or `linux`: a redirect to that file on the repository's GitHub releases (and nowhere else), back to `/download` when there is none, or to GitHub's releases page when GitHub can't be reached. Public                                |

Everything signed-in reads through tRPC, and every read goes through the access rules in
`src/server/services/access.ts`. Pages prefetch their queries on the server and hand them to
client components (`useSuspenseQuery`), so the first render already has the data. Times are
written in the browser, in the viewer's own time zone. The browser's tRPC client logs calls to
the console in development only, so refusals a page expects and handles (an account you can't
see, a code that didn't work) print nothing in production.

An account you can't see (or that doesn't exist) answers HTTP 404, not a 200 carrying the
not-found page: `src/app/accounts/[key]/layout.tsx` checks access before anything streams, above
the route's `loading.tsx`, and the page only loads the account after that. A project page does
the same from `src/app/projects/[id]/layout.tsx`. A `loading.tsx` also wraps every route below
its folder, so the projects list keeps its own in the `(list)` route group, where it can't stream
ahead of that check; tests hold both routes to having none above them.

**Dates from the future.** Sync accepts dates up to a day past the server's clock (for Macs whose
clock runs a little ahead), and they are stored as sent, but no view dates anything later than
now. A usage meter's "latest reading" ignores readings dated after now. Sessions count as now at
the latest, clamped in the SQL: in the 7- and 30-day totals, in the order of recent sessions and
the times shown for them (their summaries' times included), and in an account's last activity
and a project's last use. The list of sessions pages through one snapshot: the first page's
"now" travels in its cursor and later pages clamp against it, so "Load more" never skips or
repeats a session, however much later it is pressed.

**Your data.** On `/settings`, each behind a phrase typed to confirm (the tRPC mutation takes it
too) and in one transaction that waits for any sync of yours to finish:

- **Remove my summaries** clears the summary text, model and time of every session you synced.
  Nothing else changes. A Mac sends a summary again only with a session that changes later while
  its summaries switch is on.
- **Delete all my synced data** deletes your sessions, projects, usage readings and window ids,
  account reports, Macs and pool memberships; the pools you created, with everyone's membership
  of them; and the bare Claude account keys you synced that nobody else's row refers to any
  more. It touches nobody else's rows. Your sign-in stays, as do the keys other people still
  use, your rate limits (your sync bucket and failed share-code attempts: a deletion must not
  reset them, and they empty on their own) and the per-IP limits' rows (which name nobody, and
  go once they are idle). Unless sync is turned off in the Mac app, its next sync
  sends data again: the app remembers what it sent, so that is new usage readings and sessions
  that are new or have changed. Signing in again later starts empty, apart from what a Mac
  sends after the deletion.

## Layout

| Path                                     | What                                                                                               |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `src/app/`                               | Pages. `_components/` holds the shared UI: meters, sparklines, session list, query boundary, times |
| `src/app/api/app/v1/{config,me,sync}`    | The Mac app's API (plain route handlers, bearer tokens only)                                       |
| `src/server/app-api/`                    | Its zod schemas (the contract), error shape, body cap, rate-limit arithmetic, handlers             |
| `src/server/auth/`                       | Bearer verification (`jose`, JWKS or legacy HS256) and `getViewer`                                 |
| `src/server/services/`                   | Access rules, sync, accounts, sessions, projects, usage, pools, your data, database rate limits    |
| `src/server/client-ip.ts`                | The client's IP address as the per-IP limits key it (hashed, never stored)                         |
| `src/server/releases.ts`                 | The latest GitHub release for `/download`, cached for 5 minutes                                    |
| `src/server/api/`                        | tRPC routers for the website's pages                                                               |
| `src/lib/`                               | Pure helpers: formatting, chart geometry, redirect checks, site address, release files             |
| `src/lib/supabase/`, `src/middleware.ts` | Supabase cookie session: refresh, protected pages                                                  |
| `prisma/`                                | Schema and migrations (the first one also enables row-level security)                              |
| `tests/unit`, `tests/integration`        | Vitest; integration runs against a throwaway Postgres in Docker                                    |

## Set up

### 1. Supabase project

Create a project at [supabase.com](https://supabase.com). Note its reference: the
`<ref>` in `https://<ref>.supabase.co`.

### 2. Google sign-in

1. In [Google Cloud Console](https://console.cloud.google.com/), open **APIs & Services >
   OAuth consent screen**. Configure it with your app name and support email. The scopes
   needed are `openid`, `.../auth/userinfo.email` and `.../auth/userinfo.profile`.
2. Open **APIs & Services > Credentials > Create credentials > OAuth client ID**. Choose
   **Web application** and add this **Authorized redirect URI**:
   ```
   https://<ref>.supabase.co/auth/v1/callback
   ```
   Supabase shows the same URL on its Google provider page.
3. In Supabase, open **Authentication > Sign In / Providers > Google**. Turn it on and paste
   the client ID and client secret.
4. Turn off every other provider there, **Email** included (new projects have it on). The site
   accepts Google sign-ins only (`app_metadata.provider`), because pool members are shown to
   each other by the email Google verified; turning the others off keeps people from making
   accounts that can't sign in anyway.

### 3. Redirect URLs

In Supabase, open **Authentication > URL Configuration**:

- **Site URL:** the site's public address, e.g. `https://notch.example.com`.
- **Redirect URLs:** add each of these.
  - `https://notch.example.com/auth/callback`: the website's own sign-in.
  - `http://localhost:3000/auth/callback`: local development.
  - `agentnotch://auth-callback`: the Mac app's sign-in (PKCE in a browser sheet).
  - For Vercel preview deployments, a wildcard such as
    `https://*-<your-team>.vercel.app/auth/callback`.

### 4. A database role for Prisma

Give Prisma its own role instead of `postgres`. In the Supabase SQL editor:

```sql
create user "prisma" with password 'choose-a-long-random-password' bypassrls createdb;
grant "prisma" to "postgres";
grant usage on schema public to prisma;
grant create on schema public to prisma;
grant all on all tables in schema public to prisma;
grant all on all routines in schema public to prisma;
grant all on all sequences in schema public to prisma;
alter default privileges for role postgres in schema public grant all on tables to prisma;
alter default privileges for role postgres in schema public grant all on routines to prisma;
alter default privileges for role postgres in schema public grant all on sequences to prisma;
```

The site reaches the database only through Prisma, as this role. The first migration turns
on row-level security for every table and adds no policies, so Supabase's Data API gives
nothing to anyone holding the publishable key. `bypassrls` lets Prisma through.

### 5. Environment

```sh
cp .env.example .env    # .env is git-ignored; never commit it
```

| Variable                               | Value                                                                                               |
| -------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `DATABASE_URL`                         | Pooler, transaction mode, port 6543, user `prisma.<ref>`, with `?pgbouncer=true&connection_limit=1` |
| `DIRECT_URL`                           | Session-mode pooler or direct connection, port 5432 (used by migrations)                            |
| `NEXT_PUBLIC_SUPABASE_URL`             | `https://<ref>.supabase.co`                                                                         |
| `NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY` | The **publishable** key (Project Settings > API Keys). Never the secret key                         |
| `NEXT_PUBLIC_SITE_URL`                 | The site's public address; the app's "Open dashboard" goes to `<this>/dashboard`                    |
| `SUPABASE_JWT_SECRET`                  | Optional: only for projects still signing tokens with the legacy HS256 secret                       |
| `RATE_LIMIT_PEPPER`                    | Optional, recommended: a long random string that keys the hash of IP addresses in the per-IP limits |
| `RELEASES_REPO`                        | Optional: GitHub `owner/name` whose releases `/download` offers; `rivantmedia/agentnotch` if unset  |
| `GITHUB_RELEASES_TOKEN`                | Optional, recommended on Vercel: a GitHub token for reading releases (see [Downloads](#downloads))  |

`NEXT_PUBLIC_*` values are compiled into the build. Set them before building, and rebuild
after changing them.

### 6. Migrate and run

```sh
npm install              # also runs `prisma generate`
npx prisma migrate deploy   # or: npm run db:migrate (uses DIRECT_URL)
npm run dev              # http://localhost:3000
```

To change the schema, edit `prisma/schema.prisma`, then run `npm run db:generate`
(`prisma migrate dev`) against a development database. Keep row-level security on for new
tables. `db:push` is removed on purpose, so nothing bypasses the migrations.

## Deploy on Vercel

1. Import the repository and set **Root Directory** to `web`. Next.js is detected, and the
   default build (`next build`) and install (`npm install`, which generates the Prisma client)
   are right.
2. Add the variables from the table above under **Project Settings > Environment Variables**.
   Set `NEXT_PUBLIC_SITE_URL` to the production address.
3. Run `npm run db:migrate` against the production database before the first deploy, and again
   whenever a change adds a migration. Run it from your machine or from CI, with `DIRECT_URL`
   set. The build never touches the database.
4. Add the production (and preview) `/auth/callback` URLs to Supabase's redirect list (step 3).

**Checking a connection string.** When the site logs Prisma's P1000 (authentication failed) or
P1001 (can't reach database server), check the values without showing anyone the password. From
`web/`:

```sh
node --env-file=.env scripts/check-database-url.mjs              # DATABASE_URL and DIRECT_URL from .env
node --env-file=.env scripts/check-database-url.mjs --clipboard  # a value copied from Vercel
```

Each finding is one line (`ok`, `warn` or `problem`), and any problem exits 1. It catches the
usual mistakes:

- quotes or a stray line break;
- a password character that must be percent-encoded (`@ # / ? %` or a space);
- the `[ ]` of Supabase's `[YOUR-PASSWORD]` left around the password;
- plain `postgres` as the user on the pooler;
- a missing `pgbouncer=true`;
- the IPv6-only direct host;
- a value cut short by an unquoted `#` in `.env`;
- two values with different passwords, for instance Vercel's copy against `.env`.

The password only ever shows as its length.

Add `--connect` to also log in, at Supabase's hosts and this computer only:

- a transaction-mode value (port 6543) is tested with the site's own Prisma client (`select 1`),
  since Prisma Migrate can't run through transaction mode;
- any other value is tested with `prisma migrate status`, which also reports pending migrations.

After a password reset:

- Supabase's **Reset database password** changes only `postgres`'s password (`alter user prisma
with password '…'` sets the `prisma` role's);
- Vercel applies changed variables only to new deployments.

Notes:

- **Limits.** Kept in Postgres, so they hold across every function instance:
  - sync: a burst of 12 calls (one catch-up pass of the app fits), then one every 10 s, per
    user; and 60 a minute per IP address (a burst of 60, then one a second), whoever is signed
    in (429 with `Retry-After` either way);
  - at most 50 Claude accounts per user: a sync that would add a 51st is refused (403);
  - per user and any 24 hours, at most 5,000 new sessions and 20,000 new usage readings (one
    window's value at one moment is one reading); and per user and account, at most 32 distinct
    usage window ids, ever. What doesn't fit is dropped from the batch, and the rest is stored:
    updates to sessions already stored always go through. `accepted` in the response counts
    what was stored;
  - usage readings older than 90 days are dropped;
  - an account card shows at most 20 meters: the 5-hour, weekly and extra-usage ones, then
    per-model ones you or the pool's creator reported, then other members', each by id;
  - share codes: 12 characters, valid for 7 days; after 10 failed codes in an hour a user can't
    join anything until the hour is up, and after 30 failed codes from one IP address in an
    hour nobody signed in from there can. Removing a member revokes the current code;
  - tRPC: at most 10 queries per batch, and mutations are never batched.

  The per-IP limits know a client by the **last** hop of `X-Forwarded-For` (the one the proxy
  in front of the site appended), else by `X-Real-IP`. Earlier hops are whatever
  the client sent and are never read, and a header that holds no address falls through to the
  next rather than switching the limits off. An IPv6 client is counted by its /64, since anyone
  given IPv6 can send from any address in theirs. Only a hash of the address (or /64) is kept:
  its SHA-256, or an HMAC keyed with `RATE_LIMIT_PEPPER` when that is set, so the stored keys
  can't be turned back into addresses by hashing all of them. The address itself is never
  stored, and a bucket is deleted once it has been idle long enough to be full again (a failed
  share code's row, after the hour). A request without a usable address gets only the per-user
  limits.

  This trusts the proxy in front of the site to write those headers. Vercel does: it sets
  `X-Real-IP` and `X-Forwarded-For` to the address it saw, overwriting whatever a client sent.
  See the self-hosting note for other proxies.

- **Errors.** In production, an unexpected error reaches the browser as a generic message and
  is logged on the server. A bearer token the site can't check right now (its signing keys
  didn't load) gets a 503, not a 401, so the Mac app retries instead of signing out.
- **Headers.** Every response carries `X-Frame-Options: DENY`,
  `Content-Security-Policy: frame-ancestors 'none'`, `X-Content-Type-Options: nosniff` and
  `Referrer-Policy: strict-origin-when-cross-origin` (`security-headers.js`).
- **Self-hosting.** `npm run build && npm start` works behind a reverse proxy. Pass the
  `Host` header through, or set `X-Forwarded-Host` and `X-Forwarded-Proto`: sign-in and
  sign-out redirects are built from them. Without them, Next.js would use its own listen
  address. The per-IP limits read the last `X-Forwarded-For` hop, else `X-Real-IP`, so the proxy
  must append the client's address to `X-Forwarded-For` (nginx: `$proxy_add_x_forwarded_for`;
  cloud load balancers do this themselves), or overwrite it. Behind more than one proxy (a CDN
  in front of nginx, say), have the last one append the client's address as the first one
  reported it (nginx's `real_ip` module does this), or every client counts as the CDN's address.

## Downloads

`/download` offers the latest release of the Mac app from the repository in `RELEASES_REPO`
(`rivantmedia/agentnotch` unless set), which the repository's release workflow publishes. The
latest release is the newest one that is neither a draft nor a prerelease. Each platform gets
one file, picked by extension and by the words in its name, since GitHub renames uploads (a
space becomes a dot): for the Mac the disk image, else the zip Sparkle updates from; for Windows
an `.exe`, else an `.msi`; for Linux an `.AppImage`, else a `.deb` or `.rpm`. Sparkle's
`appcast.xml`, signatures, checksums and anything named after upstream's Codenotch are never
offered, and Windows and Linux read "Not available yet" until a release carries a file for them.
The rules are `src/lib/releases.ts`; the fetch is `src/server/releases.ts`.

- **Caching.** The site asks GitHub's API for the 5 newest releases, and Next's Data Cache keeps
  the answer for 5 minutes, so a new release shows up within that without a redeploy. It lists
  releases instead of asking for `/releases/latest`, which answers 404 until the first release
  (Next caches only a 200). Without a token, GitHub allows 60 requests an hour per IP address.
  On Vercel, or any host whose outbound addresses are shared, other sites use the same
  allowance. A refusal isn't cached, so the buttons fall back to GitHub's releases page until
  the limit resets. Set `GITHUB_RELEASES_TOKEN` there.
- **The buttons** link to `/download/<platform>`, which redirects (302, never cached) to the
  file, and only to a file under `https://github.com/<repo>/releases/download/`. They are plain
  links, not `next/link`, which would prefetch the redirect. The landing page and the header
  link to these pages and never wait on GitHub themselves.
- **No release yet:** `/download` says so and links the source. `/download/<platform>` comes back
  to `/download?unavailable=<platform>`, as it does when the release has no file for that
  platform.
- **GitHub unreachable** (an error, an answer other than 200, one that doesn't read as a list of
  releases, or none within 5 seconds): `/download` points to GitHub's releases page, and
  `/download/<platform>` redirects there.

## Connect the Mac app

Install the app first, from `/download` (see [Downloads](#downloads)).

1. Sign in on the website with the Google account you want your history under.
2. In Agent Notch, open **Settings > Claude Code > Cloud**. Paste the website's address under
   **Website**, as shown on `/settings`, and choose **Save**. It's the origin, such as
   `https://notch.example.com`, with no path.
3. Choose **Sign in with Google**. The app reads `GET /api/app/v1/config` for the Supabase
   project and signs in with PKCE, returning to `agentnotch://auth-callback`. It keeps its own
   Supabase session in `cloud-session.json` in its support folder.
4. Turn on **Sync sessions and usage**. The app then calls `POST /api/app/v1/sync` with its
   bearer token, and the accounts appear on the dashboard. **Summarise finished sessions with
   Claude** is a separate switch, off until the user turns it on.

Signing out of the website doesn't sign the app out, and the other way round: both sign out
with Supabase's `local` scope, which ends only their own session. The app's "Open dashboard"
goes to `/dashboard`, and its pooling link to `/dashboard/pools`, which redirects to `/pools`.

## Checks

```sh
npm run typecheck
npm run lint
npm run format:check
npm test                   # unit tests: contract, schemas, bearer auth, access rules, formatting, charts, …
npm run test:integration   # needs Docker; see below
SKIP_ENV_VALIDATION=1 npm run build
```

`npm run test:integration` starts `postgres:16-alpine` as the container `agentnotch-web-test`
on `127.0.0.1:55432`. It applies the migrations and checks the schema has no drift. Then it
runs `tests/integration` and always stops the container, which deletes it. It touches no
other container.

**The contract end to end.** From the repository root, `Scripts/cloud-contract-e2e.sh` checks
that the Mac app and this site agree when each runs its real code. It needs Docker and
`node_modules`, and makes no network call:

1. A Swift test in `Packages/ClaudeControl` captures sessions and usage through the app's sync
   service and writes the exact request body it sent.
2. `tests/integration/contract-e2e.test.ts` posts that body twice through the real
   `POST /api/app/v1/sync` route (`src/app/api/app/v1/sync/route.ts` and its real dependencies).
   The bearer token is signed in the test and the key set is served from memory. The test checks
   every stored row against the body (accounts, projects, sessions with their token totals,
   usage windows), checks that the second post changes nothing, and writes the route's answer.
   Every field the app sent must be compared with a stored row or listed, with the reason, in
   `NOT_STORED` (`tests/support/contract-rows.ts`): the route's schema drops fields it doesn't
   know without a word, so a field the app adds or renames fails the check instead of being
   lost. It runs in a container of its own, `agentnotch-web-test-e2e` on `127.0.0.1:55439`,
   which `scripts/test-integration.mjs` starts and always stops; if that runner is killed before
   it can, the script removes the container on exit. `scripts/test-container.mjs` accepts only
   `agentnotch-web-test*` names on ports 55432 to 55439.
3. A second Swift test reads that answer with the app's own client.

In a plain `npm run test:integration` the contract test is skipped (it runs only when
`AGENTNOTCH_CONTRACT_OUT` names a request, and only against `127.0.0.1:55439`).
