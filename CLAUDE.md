# Agent Notch

A fork of [vinzdg/codenotch](https://github.com/vinzdg/codenotch) (MIT; a macOS notch that pins
usage-limit rings for many AI tools to any screen edge) that adds **Claude Code session control
across several accounts**:

- One ring per Claude **account**. Folders are grouped by signed-in identity; the weekly ring is
  drawn outside the 5-hour session ring by default.
- Session states: needs you / failed / ready for review / working / idle. The review queue is
  persisted. Each session shows task progress and context %. Ring badges and folded-notch marks
  summarise them.
- A sessions panel that opens from a Claude ring on any edge:
  - approve / deny / always allow;
  - answer questions and approve plans;
  - chat with a session;
  - jump to its exact terminal tab.
- Usage that **never reads a Claude login token**: Claude Code's own `get_usage`, the cached usage
  in `.claude.json`, the status line, and Claude Desktop's cache.
- Optional **cloud sync** to the website in `web/` (sign in with Google through its Supabase
  project): sessions, tokens, cost and usage readings per Claude account, opt-in session
  summaries written by Claude Code, and account pooling on the website. Off until the user signs
  in and turns sync on.
- Works with the **Claude Parallel Profiles** VS Code extension (see below).

The app was called **Superpowered Codenotch** until it was renamed; the engine still treats
hook entries, status line wrappers and backups under that name as its own (`AppIdentity.former*`,
`HookInstaller.removeFormerNameFiles`), so an install replaces them in place. Keep those names.

The Claude code was ported from an earlier fork of Vibe Notch ("Superpowered Vibe Notch", SPVN,
now retired). The README's first half is the user-facing fork documentation: build, first use,
privacy, development switches. Read it before changing behaviour.

## Layout

| Path | What lives there |
|---|---|
| `Sources/` | Upstream Codenotch (Swift 5, minimal concurrency checking). Keep edits here to the listed seams. |
| `Sources/ClaudeBridge/` | Fork-only glue in the app module. It copies fields and calls AppKit, and **holds no logic**: providers/rings (`ClaudeUsageProvider`, `ClaudeProviderSync`, `ClaudeRingMigrator`), session feed, ring decoration/badges, resting marks, attention reactions, panel window/controller, settings host, the website sign-in's browser step (`CloudWebAuthSession`: `ASWebAuthenticationSession`, callback scheme `agentnotch`, over the settings window), sealed demo, snapshots. |
| `Sources/App/Fork.swift` | Fork identity: bundle id `com.rivantmedia.agentnotch`, support folder, keychain-service remap, default registration (weekly ring outside), and `Fork.rebranded`, which puts "Agent Notch" into upstream's "Codenotch" copy (seam R1). |
| `Packages/ClaudeControl/` | Local SwiftPM package (Apache-2.0 + `NOTICE`), **all Claude logic**, one module `ClaudeControl`. |
| `…/Sources/ClaudeControl/Engine/` | Must never import SwiftUI (checked by `check-seams.sh`). `Public/` holds the facade `ClaudeControlHub`, `ClaudeControlConfiguration`, summaries, projections, ring migration and account inspection. `Services/` holds hooks, session pipeline, accounts, usage, notifications, window/tmux focus, and `Cloud/` (website sign-in, session ledger, transcript token scanner, usage history, session summaries, `CloudSync`; the hub's side is `Public/ClaudeControlHub+Cloud.swift`). `Core/` holds `DevFlags`, `ClaudeControlSettings`, `AccountPaths`, `SealedMode`. `Geometry/` holds panel placement and ring-badge layout. `Scripts/` holds the generated `EmbeddedScripts.swift`. |
| `…/Sources/ClaudeControl/UI/` | SwiftUI: sessions panel, chat, settings pane (its Cloud section is `Settings/CloudSection.swift`), theme tokens (`ClaudeControlTheme`). |
| `…/Sources/ClaudeControlSnapshots/` | Renders panel/chat/settings sheets to PNGs from fixtures. |
| `…/Sources/agentnotch-inspect-accounts/` | Read-only account inspection of the real home (see "Checking a real setup"). |
| `Packages/ClaudeControl/Scripts/` | `agentnotch-{hook,statusline}.py` plus `embed-scripts.sh`. **Edit the `.py` files, then run `embed-scripts.sh`.** Never hand-edit `EmbeddedScripts.swift`. |
| `Packages/ClaudeControl/DevTools/simulate-sessions.py` | Drives a running dev build with fake sessions over a private socket. |
| `Tests/ForkSPM/` | Swift Testing tests for app-side fork code (the bridge). |
| `Tests/ClaudeBridgeTests.swift`, the rest of `Tests/` | XCTest (upstream + bridge). Needs Xcode; runs in CI only. |
| `Scripts/spm-*.sh`, `check-seams.sh`, `fork-seams.txt`, `verify-token-free.sh`, `cloud-contract-e2e.sh` | Fork tooling (below). |
| `project.yml`, `Makefile` | Upstream's xcodegen/Xcode path, kept working (the local package is wired into both). |
| `web/` | The cloud sync website: Next.js (T3: tRPC, Prisma, Tailwind, TypeScript) on Supabase (Postgres, Auth with Google). Its own README covers setup and checks (`npm test`, `npm run typecheck`). `web/.env` holds real values and stays untracked; `.env.example` has placeholders. |
| `web/contract/` | The **fixed** app⇄website API (`README.md` + JSON fixtures both sides test against: field names, key derivation, limits, error shape). Change a fixture only together with both sides. |

**Public API rule:** `ClaudeControl`'s public surface is the bridge contract (`Tests/ClaudeControlTests/PublicContractTests.swift` pins it). Add members freely; don't rename or remove them without updating the bridge and that test.

## Build, test, run (this Mac has only the Command Line Tools — no Xcode)

```sh
Scripts/spm-build-app.sh [--release]      # → build/Agent Notch.app (ad-hoc signed; SIGN_IDENTITY=… for a stable identity)
Scripts/spm-test.sh                        # Swift Testing: ClaudeControl package, then Tests/ForkSPM
Scripts/spm-test.sh ClaudeControl --filter <SuiteName>   # one package suite
Scripts/spm-test.sh app                    # only Tests/ForkSPM
Scripts/spm-snapshots.sh <dir>             # package UI sheets → PNGs
Scripts/spm-run-sealed.sh 8 --skip-build [--edge right|left|top|bottom] [--open-panel sessions] [--panel-self-test] [--snapshot-claude <dir>]
Scripts/check-seams.sh                     # upstream edits are all listed, tagged seams; Engine has no SwiftUI; embedded scripts current
Scripts/verify-token-free.sh               # no Claude code path can read a token
Scripts/cloud-contract-e2e.sh              # app⇄website: the app's real sync request through the website's real sync route into Postgres, and its answer back (Docker; no network)
Packages/ClaudeControl/Scripts/embed-scripts.sh [--check]
```

- **SDK:** the scripts select the Command Line Tools' **macOS 26.x SDK** themselves. Building
  against the macOS 27 SDK fails: SwiftUI's `@State` became a macro whose plugin ships only with
  Xcode. Likewise `#Preview` and XCTest are unavailable here. Swift Testing works, via the extra
  plugin/rpath flags `spm-test.sh` passes.
- `check-seams.sh` and `verify-token-free.sh` compare against the merge-base with `upstream/main`.
  Without that remote, set `UPSTREAM_REF=642d329` (the upstream commit the fork is based on).
- `cloud-contract-e2e.sh` runs three steps and stops at the first failure: the Swift test
  `CloudContractE2ETests/theAppsSyncRequestKeepsTheContract` writes the request the sync service
  encoded (`AGENTNOTCH_CONTRACT_OUT`); `web/tests/integration/contract-e2e.test.ts` posts it twice
  through `POST /api/app/v1/sync` (locally signed token, in-memory key set), checks the stored rows
  and writes the route's answer (`AGENTNOTCH_CONTRACT_RESPONSE`); `theWebsitesSyncResponseIsRead`
  reads it back. The web step runs in its own container, `agentnotch-web-test-e2e` on
  127.0.0.1:55439, always stopped afterwards. Without those variables the Swift tests check the
  contract's rules and fixture, and the web test is skipped. Run it after changing either side of
  `web/contract`.
- **Look at the PNGs you render.** Snapshots are the only way to see UI changes without launching
  the live app.
- **Definition of done:** a change is finished when all of these pass:
  - `spm-test.sh`, run twice, green;
  - `spm-build-app.sh` builds with no new kinds of warnings;
  - both check scripts;
  - `embed-scripts.sh --check`;
  - a sealed run on the affected edges.

## Safety rules (the maintainer's real setup is live on this Mac)

- **Never launch the live app for testing.** Use sealed mode: `Scripts/spm-run-sealed.sh`, or
  `AGENTNOTCH_SAFE_MODE=1`. It builds a separate `….sealed` bundle, uses fixture data, and never
  touches the keychain, sessions, network or subprocesses. It fails closed: any value except
  empty/0/false/no/off seals.
- For engine tests use a **temporary HOME / config dirs**, never the real ones.
- Never write to `~/.claude*`, `~/.claude.json`, `~/.claude-windows/**` or `~/.claude-shared`.
- Never read `.credentials.json`, Keychain secrets or `sessions/*.key`.
- Never run `claude`: tests use fake executables.
- Never touch the maintainer's running app (bundle id `com.rivantmedia.agentnotch`, or
  `com.paraswtf.superpowered-codenotch` under the former name) or
  the official Codenotch (`com.vinz.codenotch`, its prefs and its keychain items).
- Never send AppleScript or keystrokes to real terminals. Test the script generation instead.
- The only sanctioned contact with the real home is the read-only inspector (below).
- **Cloud.** Engine tests use the fake transport and stub `claude` scripts; never call a real
  website or Supabase project from a test. The app's website sign-in lives in
  `<support>/cloud-session.json` (0600, in the 0700 support folder), never in the Keychain;
  never read the real one. Never open `credentials.txt` or a `.env` with real values.
- **No token paths.** Upstream's keychain-token Claude path (`ClaudeOAuthProvider`,
  `ClaudeTokenRefresher`, `ClaudeUsageCLI`) is disabled by seam U1 (`claudeProfiles = []`).
  `verify-token-free.sh` enforces it. Don't reintroduce any of it.
- **Consent before writes.** Nothing is written to any `settings.json` until the user clicks Turn on.
- **Consent before uploads.** Nothing is sent to the website until the user signs in and turns
  sync on; summaries (a `claude -p --model haiku` run that spends the account's usage) need their
  own switch. The switches belong to one sign-in on one website: sign-out and a website change
  turn both off, and a new sign-in starts with them off. Nothing is captured while signed out or
  with sync off. Sync sends names and numbers only: never file paths, prompts or replies (a
  summary is scrubbed of absolute paths and key-like text before it leaves).

## Working with upstream

- The only remote is `upstream` (vinzdg/codenotch, which moves fast: roughly 200 commits a week).
  `main` = upstream `642d329` + the fork.
- To merge upstream:
  1. `git fetch upstream && git merge upstream/main`.
  2. Run `Scripts/check-seams.sh`. Every `SEAM` line it reports missing must be re-applied by hand
     at the same anchor, with its `// Fork: <id>` tag.
  3. Run the full definition of done.
- **Upstream files the fork edits** (all tagged `// Fork:`, listed in `Scripts/fork-seams.txt`):
  - `AppDelegate`, `Log`, `Updater`, `StatusItemSummary`;
  - `UsageStore` (the `replaceProviders`/`ingest` block, U6);
  - `ProviderRing` (U7);
  - `NotchRootView`, `NotchViewModel`, `NotchWindowController` (U8–U10);
  - `SettingsView` (the Claude Code sidebar section, U11);
  - `Preferences`, `KeychainItem` (the remap), `CustomEndpoint`, `PhoneLinkSecretStore`;
  - `ClaudeDesktopUsageCache` and `ClaudeUsageCLI` (plus its test);
  - `L10n` (R1: every lookup goes through `Fork.rebranded`), `OllamaSettingsRow` and two
    `SettingsView` literals (R2), and `AntigravityTests`/`LocalizationTests` (R3);
  - `Info.plist`, `project.yml`, `Makefile`.
- **Adding an edit to an upstream file:**
  1. Tag it `// Fork: <id>`.
  2. Add a `SEAM` line (or an `ALLOW` path) to `fork-seams.txt`.
  3. Keep it to a few lines, and put the logic in the package or the bridge.
- **Upstream's name.** Upstream's copy says "Codenotch"; `L10n.t` turns it into this app's name in
  every language (French elision, German compounds, "an" in English), except copy about the
  Codenotch phone app and upstream's Windows build (`Fork.upstreamProductPhrases`), and never
  inside interpolated user data. `check-seams.sh` fails on a new `"…Codenotch…"` literal outside
  `L10n.t`: after a merge, route it through `L10n.t` (or `Text(verbatim: Fork.displayName)`).
- Sparkle auto-update is **off** (no feed keys). `FORBID` rules in `fork-seams.txt` keep it off
  and keep `pkill -x Codenotch` out of the Makefile, so the fork can never install upstream
  builds over itself or kill the official app.
- **Upstream's rules still apply to upstream-style code** (see `CONTRIBUTING.md`):
  - comments explain *why*;
  - user-visible strings go through `L10n.t(…)` in upstream code (the fork's own UI is English-only);
  - provider adapters degrade failures to an honest status and never invent a number.

## Claude Code facts the engine relies on (verified against Claude Code 2.1.280)

- **Accounts are config folders** (`CLAUDE_CONFIG_DIR`; unset means `~/.claude`).
  - Identity comes from `oauthAccount` in `.claude.json`: `~/.claude.json` for the default folder,
    `<dir>/.claude.json` otherwise.
  - `cachedUsageUtilization` in the same file holds Claude Code's last usage reading.
  - Transcripts are `<dir>/projects/<slug>/<sessionId>.jsonl`. The slug is the cwd with every
    non-alphanumeric character replaced by `-`. Prefer the hook's `transcript_path` to rebuilding it.
- **Hooks.**
  - Hook processes inherit `CLAUDE_CONFIG_DIR`, `CLAUDE_PID` and `CLAUDE_CODE_ENTRYPOINT`.
  - `agent_id` is present only for subagent events.
  - `PermissionRequest` carries no `tool_use_id`; the engine matches it to the preceding `PreToolUse`.
  - To answer `AskUserQuestion`, allow with `updatedInput` = the original input plus
    `"answers": {question: label}`. `ExitPlanMode` also needs `updatedInput`; a plain allow is
    ignored for tools that require user interaction.
- **Session registry:** `<dir>/sessions/<pid>.json` (status busy/shell/idle/waiting, `waitingFor`, times
  in epoch ms). Never read the `*.key` files beside them. It stays `busy` after a Stop while any
  agent, workflow, cloud session or active teammate of the session runs (in the SDK too), says
  `shell` when only background shells/monitors are left, and isn't rewritten while unchanged.
- **Claude Desktop-hosted sessions (2.1.282, read from the bundle).** Desktop runs them as its own
  account, whatever the folder names. Their registry entry has entrypoint `claude-desktop`,
  `claude-desktop-3p` or `local-agent` and `hostSessionId` (`local_[0-9a-f-]{8,72}`, from
  `CLAUDE_CODE_HOST_SESSION_ID`). Desktop records each under
  `~/Library/Application Support/Claude/claude-code-sessions/<accountUuid>/<organizationUuid>/<hostSessionId>.json`;
  the hub attributes the session to the identity whose record exists (`DesktopHostedSessions`,
  `lstat`s only, never through a link, never opened, never when sealed) and to no one otherwise.
- **Unsure sessions and the website.** A running session the hub can't attribute for certain
  (or that runs as an account the website may not hear of) counts for no account: the ledger
  gives it a nobody (`""`) owner stretch until it is certain again (`SessionLedger`). A session
  the hub hasn't placed yet (`.known(nil)` for a folder not grouped yet, a Desktop-hosted one
  whose registry entry hasn't been read) is neither: it waits, for at most
  `ClaudeControlHub.placementGrace`.
- **Background work (2.1.282, read from the bundle).** Stop's `background_tasks` lists running tasks
  with friendly `type`s (shell, subagent, monitor, workflow, MCP task, teammate, dream, auto-mode scan,
  cloud session). Finishing ones wake Claude with a `UserPromptSubmit` whose prompt starts
  `<task-notification>` (no `source` field). `idle_prompt` fires 60 s after every turn regardless.
  The engine's background wait (`BackgroundWork`) relies on all of this.
- **Status line rate limits (2.1.282, read from the bundle).**
  - `rate_limits` is the process's latest API response's `anthropic-ratelimit-unified-5h/7d-*` headers.
  - There is one set per process. Each newer response replaces it, even a lower one.
  - It is empty until the process's first response; it is never seeded from disk.
  - A window whose `resets_at` has passed is left out.
  - A re-run (mode change, timer) repeats the set unchanged, with no timestamp.
  - An early reset (Claude Code's own `/limit-reset`: "your weekly reset day stays") lowers usage
    but keeps `resets_at`. Other processes keep the old numbers until their next response.
  - A response's headers arrive when it starts but are applied when it ends, so a process's
    changed numbers can be older than its previous render.
  - So the engine dates each process's readings (`UsageStore.advance`). Readings are keyed by
    the `pid` the wrapper forwards from `CLAUDE_PID` (checked against the hooks' pid) plus the
    process's kernel start time, else by session.
  - Records keyed with a start time are kept in `usage-state.json` for 7 days after their last
    report. A running process picks up where it left off; an ended one's readings still count.
  - A reading known to be later wins even when lower, if it drops by more than 5 points: a
    reset (`UsageStore.supersedes`, `isSmallDrop`). Otherwise "higher wins" within a window,
    within one process too.
- **Usage probe:**
  `claude -p --input-format stream-json --output-format stream-json --verbose --no-session-persistence --strict-mcp-config --settings '{"disableAllHooks":true}'`,
  sending `initialize` and then `get_usage {skip_behaviors:true}`.
  - Runs at most once per account per interval, in a folder Claude Code runs in, never in a
    Parallel Profiles store.
  - The app never refreshes tokens: refresh tokens rotate, so an outside refresh logs Claude Code out.
- **Hook commands must fail open.** If the script is missing they must exit 0, because a hook
  exiting 2 blocks Claude Code. Settings writes are atomic, backed up, and refused when the file
  doesn't parse.

## Cloud sync (app side)

- **Contract:** `web/contract/README.md` and its fixtures, fixed. Keys: `accountKey` =
  SHA-256 of the lowercased `<accountUuid>/<organizationUuid>` from the identity's own folders
  (the accountUuid alone only when no folder names an organization; never how this Mac groups
  identities, never a mirrored folder's stale organization: `CloudKeys.accountKey(identity:)`);
  project key = HMAC-SHA256 keyed with `<support>/cloud-install-secret` (32 random bytes, 0600,
  made once, never sent) of `<accountKey>:<resolved cwd>`. Tests use keys.json's
  `installSecretHex`. Every date in a request lies between 2023-01-01 and now + 1 day
  (`clamped(now:)` leaves out what doesn't; `resetsAt` is left alone).
- **Sign-in:** `GET <website>/api/app/v1/config`, then Supabase's Google authorize with PKCE (S256)
  in the bridge's `ASWebAuthenticationSession`, back to `agentnotch://auth-callback?code=…`, then
  `POST <supabase>/auth/v1/token?grant_type=pkce`. Refresh tokens rotate, so a refreshed session
  is saved before use. Sign out is `POST …/logout?scope=local`. A user cancel is
  `CancellationError` (quiet, back to signed out). A sign-in is bound to the website it began
  with (a result that comes back after the website changed is revoked, never adopted), and a
  token only ever goes to the website its session was made through. The session is forgotten
  only when Supabase refuses the refresh token (400/401 with `invalid_grant`,
  `refresh_token_*`, `session_*`…); a website 401 after a good refresh, a 5xx or a 429 backs off
  with the session kept.
- **Where it lives:** `<support>/cloud-session.json` (0600); also `cloud-install-secret`,
  `cloud-ledger.json`, `cloud-scan-state.json`, `cloud-usage-outbox.json`, `cloud-summaries.json`,
  `cloud-sync-state.json`, `cloud-folder-logins.json` (kept whether or not sync is on) and an
  empty `session-summary/` working folder. `<support>` is
  `~/Library/Application Support/Agent Notch/Claude` (0700). Nothing is read or written sealed
  or before bootstrap.
- **Summaries:** `claude -p --model haiku --max-turns 1 --max-budget-usd 0.10 --output-format json
  --no-session-persistence --strict-mcp-config --settings '{"disableAllHooks":true}' --tools ""`,
  the excerpt on stdin, in a run folder signed in as the session's account (chosen and re-checked
  like the usage probe's). Only sessions that ended after the switch was last turned on
  (`SessionSummaryStore.enabledAt`); at most 20 an hour and 60 a day; none for an account whose
  5-hour window is at least 80% used. The answer is scrubbed (`SessionSummarizer.scrub`).
  Cancelling the summary task (switch off, sign-out, quit) stops the child at once.
- **Ledger:** one entry per session and account (`CloudLedgerEntry.key`). A running session seen
  under another account than it last ran as is split there (`SessionOwner`s); the scanner
  counts each line for the account that ran the session at its timestamp, and each part is sent
  as its own row. What was sent is remembered per entry with whether it had ended and the
  transcript's size/mtime, so a session is read again (a stat when unchanged) until its end is
  sent. The backfill reads only real (unshared) `projects/` folders, oldest transcript first,
  and only transcripts begun after `CloudFolderLogins` first saw the folder signed in as its
  account (unknown before: never guessed).
- **Claude Desktop:** its usage readings come through `UsageStore` (only with the Desktop cache
  setting on). A Desktop-hosted session is recorded only when
  `~/Library/Application Support/Claude/claude-code-sessions` lists exactly one account and it is
  the session's (a directory listing; no file there is opened). claude.ai chats are never read.
- **Sealed:** `hub.cloud` is a signed-in fixture and every cloud action is a no-op; the bridge's
  browser step refuses to open.

## Claude Parallel Profiles compatibility

The maintainer runs the VS Code extension **Claude Parallel Profiles** (source in the sibling
folder `../claude-parallel-accounts-vsc-extension`). It creates this layout:

| Folder | Role | This app |
|---|---|---|
| `~/.claude` | default; the extension mirrors the last-used account into it (often rewriting only the email, not `accountUuid` — the email decides) | run folder: hooks + probe |
| `~/.claude-<name>` with `.parallel-accounts-store`, or under `created` in the manifest | **account store**, never run by Claude Code | identity + cached usage only; never hooked, probed or written |
| `~/.claude-<name>` under `stores` only | a profile the user made and the extension adopted | run folder |
| `~/.claude-windows/<12 hex>/` | per-VS-Code-workspace working copy (window sessions run here) | run folder; new ones are hooked automatically |
| `~/.claude-windows/.manifest.json` | extension manifest (`stores`, `created`) | read only |
| `~/.claude-shared` | shared history, symlinked into every folder (`projects`, `sessions`, …) | infrastructure, never an account |

- **One ring per identity.** Ring ids are `claude-acct-<12 hex of sha256(accountUuid)>`. Older
  per-folder ids migrate once through `ClaudeRingMigrator`.
- **Registry and transcript reading.** The registry is read once per physical `sessions` folder.
  Each entry is attributed by its process's `CLAUDE_CONFIG_DIR`, read from the kernel
  (`KERN_PROCARGS2`, same user only). **Only that variable is kept**, because the environment
  holds secrets.
- **Checking a real setup:** `swift run --package-path Packages/ClaudeControl agentnotch-inspect-accounts`.
  It is read-only and prints accounts, run and store folders, install targets and cleanup targets.
  On the maintainer's Mac it must report exactly 2 accounts.

## Handy dev switches

Full list in the README's Development section and in `Engine/Core/DevFlags.swift`.

- **Behaviour:** `--no-install` / `AGENTNOTCH_NO_INSTALL=1`, `AGENTNOTCH_NO_NOTIFICATIONS=1`,
  `AGENTNOTCH_USAGE_PROBE=1` (probes are off in dev runs otherwise), `--dump-state`,
  `--dev-console`.
- **Isolation:** `AGENTNOTCH_SUPPORT_DIR`, `AGENTNOTCH_SOCKET` (the hook scripts follow it only with
  `AGENTNOTCH_DEV=1`), `AGENTNOTCH_EXTRA_CONFIG_DIRS`.
- **Cloud:** `AGENTNOTCH_WEB_URL=<url>` sets the sync website for one run (https, or http to
  localhost/127.0.0.1/[::1]; ignored when sealed). A sign-in saved for another website is set
  aside, not deleted. Session summaries never run in a `--no-install` run.
- **Sealed-only:** `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH`, `AGENTNOTCH_PANEL_SELF_TEST`, `AGENTNOTCH_SNAPSHOT_CLAUDE`,
  `AGENTNOTCH_SEALED_SWITCH_OFF`, `AGENTNOTCH_SEALED_CAPTURE`.
