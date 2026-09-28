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
- **Downloads and self-updates** from the fork's own GitHub Releases (rivantmedia/agentnotch):
  the release workflow (a push to main that changes `VERSION` or the update key) publishes
  `AgentNotch-<V>.dmg`, the Sparkle zip and `appcast.xml`; release builds update themselves
  through Sparkle (EdDSA-signed, the fork's feed only, never upstream's); the website's
  `/download` page links the latest release.

The app was called **Superpowered Codenotch** until it was renamed; the engine still treats
hook entries, status line wrappers and backups under that name as its own (`AppIdentity.former*`,
`HookInstaller.removeFormerNameFiles`), so an install replaces them in place. Keep those names.

The Claude code was ported from an earlier fork of Vibe Notch ("Superpowered Vibe Notch", SPVN,
now retired). The README's first half is the user-facing fork documentation: download, build,
first use, updates, privacy, development switches, releases. Read it before changing behaviour.

## Layout

| Path | What lives there |
|---|---|
| `Sources/` | Upstream Codenotch (Swift 5, minimal concurrency checking). Keep edits here to the listed seams. |
| `Sources/ClaudeBridge/` | Fork-only glue in the app module. It copies fields and calls AppKit, and **holds no logic**: providers/rings (`ClaudeUsageProvider`, `ClaudeProviderSync`, `ClaudeRingMigrator`), session feed, ring decoration/badges, resting marks, attention reactions, panel window/controller, settings host, the website sign-in's browser step (`CloudWebAuthSession`: `ASWebAuthenticationSession`, callback scheme `agentnotch`, over the settings window), sealed demo, snapshots. |
| `Sources/App/Fork.swift` | Fork identity: bundle id `com.rivantmedia.agentnotch`, support folder, keychain-service remap, default registration (weekly ring outside), and `Fork.rebranded`, which puts "Agent Notch" into upstream's "Codenotch" copy (seam R1). Updates: `updateFeedURL` (`…/releases/latest/download/appcast.xml`, also written by `spm-build-app.sh`; `UpdatesTests` pins both), `releasesPageURL`, the gate `updatesEnabled` (pure `updatesEnabled(info:bundleID:sealed:underTest:)`: not sealed, not under test, exact bundle id, Info.plist `SUFeedURL` == the feed, `SUPublicEDKey` strict base64 of 32 bytes), and `showsUpstreamReleaseNotes = false` / `showWhatsNewIfNeeded` (upstream's What's New notes are keyed by upstream's versions; never shown). |
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
| `app-config.json` | The one place the app's website address is set (`{"websiteURL": "https://…"}`). `spm-build-app.sh` checks it and writes it into the bundle's Info.plist as `AgentNotchWebsiteURL`; the Release workflow's website job reads the same file. See "Cloud sync (app side)". |
| `VERSION` | Fork-owned, one line `major.minor.patch` (first release 1.0.0). The app's only version source: `spm-build-app.sh` writes it as `CFBundleShortVersionString` **and** `CFBundleVersion` (Sparkle compares the latter). `project.yml`'s `MARKETING_VERSION`/`CURRENT_PROJECT_VERSION` stay upstream's (the Xcode path and `WhatsNewTests` read them). A push to main that changes it releases. |
| `Scripts/release-build.sh`, `release-make-keys.sh`, `release-ed25519.swift`, `sparkle-public-ed-key.txt`, `AgentNotch.entitlements` | Release pipeline. `release-build.sh`: everything the workflow does except publishing (build `--release --universal --with-updates`, optional p12 import into a temporary keychain + notarization, dmg, Sparkle zip, `sign_update`, CryptoKit check against the committed key, `appcast.xml`, `release-info.env`) into `--out` (default `build/release`). `release-make-keys.sh`: the maintainer's one-time key setup (`--update-key`, `--signing-cert`, `--rotate`, `--set-secrets`: makes the `release` environment, main only, and sets its secrets). `release-ed25519.swift`: CryptoKit generate/public/verify. `sparkle-public-ed-key.txt`: committed public key (header only until the maintainer runs the key script). The entitlements file (Apple Events only) is for `--hardened-runtime`. |
| `.github/workflows/fork.yml`, `release.yml` | Fork CI (push to main, PRs, dispatch, and `workflow_call` from Release; concurrency group `${{ github.workflow }}-${{ github.ref }}`; also checks debug/sealed builds carry no feed) and the Release workflow (below). Upstream's workflows run only in `vinzdg/codenotch`, except `windows-package.yml` on a manual dispatch: never ship its output (the upstream Windows port reads Claude's login token). |
| `project.yml`, `Makefile` | Upstream's xcodegen/Xcode path, kept working (the local package is wired into both). Not the fork's release path: `make appcast` stays refused (its recipe publishes upstream's hivinz.com feed). |
| `web/` | The cloud sync website: Next.js (T3: tRPC, Prisma, Tailwind, TypeScript) on Supabase (Postgres, Auth with Google). Its own README covers setup and checks (`npm test`, `npm run typecheck`). `web/.env` holds real values and stays untracked; `.env.example` has placeholders. |
| `web/contract/` | The **fixed** app⇄website API (`README.md` + JSON fixtures both sides test against: field names, key derivation, limits, error shape). Change a fixture only together with both sides. |
| `web/src/app/download/` (`page.tsx`, `loading.tsx`, `[platform]/route.ts`), `web/src/lib/releases.ts`, `web/src/server/releases.ts` | The website's public download page. `lib/releases.ts` is pure (release pick, asset classifier: dmg > pkg > zip for Mac, future Windows/Linux names, never `codenotch`/appcast/signatures; `isReleaseDownloadUrl`; User-Agent platform); `server/releases.ts` fetches `GET /repos/<RELEASES_REPO>/releases?per_page=5` (default `rivantmedia/agentnotch`, optional `GITHUB_RELEASES_TOKEN`, recommended on Vercel, whose shared outbound addresses share GitHub's 60 unauthenticated requests an hour; 5-minute Next cache of 200s only; never throws). `/download/<mac\|windows\|linux>` 302s only to `https://github.com/<repo>/releases/download/…` (`private, no-store`). Tests never call GitHub. |

**Public API rule:** `ClaudeControl`'s public surface is the bridge contract (`Tests/ClaudeControlTests/PublicContractTests.swift` pins it). Add members freely; don't rename or remove them without updating the bridge and that test.

## Build, test, run (this Mac has only the Command Line Tools — no Xcode)

```sh
Scripts/spm-build-app.sh [--release]      # → build/Agent Notch.app (ad-hoc signed; SIGN_IDENTITY=… for a stable identity); version = VERSION; no feed
Scripts/spm-build-app.sh --release --universal --with-updates [--hardened-runtime]   # release flags; only release-build.sh passes them
AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE=<scratch file> Scripts/release-make-keys.sh --update-key <scratch dir>   # throwaway key pair
AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE=<scratch file> Scripts/release-build.sh --host-arch --out <scratch> --ed-key-file <seed>   # release minus publishing
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
- **Build flags for releases.**
  - `--universal` builds arm64 and x86_64 one at a time (slices in
    `.build/agentnotch-slices/<config>/<arch>`) and joins them with lipo. It stops early here:
    the Command Line Tools ship the Swift compatibility libraries for arm64 only. Xcode 26 can
    build it (the workflow pins Xcode 26.6). Local trials use `release-build.sh --host-arch`,
    which is refused on GitHub Actions.
  - Every build embeds the Swift back-deployment libraries the executable loads through
    `@rpath` (`libswiftCompatibilitySpan.dylib`, for macOS 15) and drops build-machine
    `LC_RPATH`s. Signing goes inside-out, never `codesign --deep`.
  - `--with-updates` writes `SUFeedURL`, `SUPublicEDKey`, `SUEnableAutomaticChecks`,
    `SUAutomaticallyUpdate`, `SUScheduledCheckInterval` 86400 and
    `SUVerifyUpdateBeforeExtraction`. It exits 2 unless the bundle id is exactly
    `com.rivantmedia.agentnotch` and the key file holds a valid 32-byte key. The key file is
    `Scripts/sparkle-public-ed-key.txt`, or `AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE` for tests. Every
    other build strips all `SU*` keys and sets `SUEnableAutomaticChecks` false.
  - `--hardened-runtime` (Developer ID only; exit 2 with ad hoc) adds the hardened runtime,
    `--timestamp` and `Scripts/AgentNotch.entitlements`.
  - `SIGN_KEYCHAIN` names a keychain that isn't on the search list.
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
  - a sealed run on the affected edges;
  - for release pipeline changes: actionlint on both workflows and shellcheck on the scripts
    (neither is installed here: download the binaries into the scratchpad), `bash -n`, and a
    `release-build.sh --host-arch` run into a scratch folder with a throwaway key (never open
    its app). The first universal build happens on CI; the maintainer starts dry runs.

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
- **Releases.** Never publish a release, push a tag or commit, set or delete a secret
  (`gh secret set`, `release-make-keys.sh --set-secrets`), change the `release` environment
  (`gh api … environments`) or start the Release workflow from a session or a test;
  read-only `gh … -R rivantmedia/agentnotch` is fine (always pass `-R`: a bare `gh` here resolves
  to upstream). Never read, print or copy the update private key (`sparkle-ed-private-key.txt`,
  the `SPARKLE_ED_PRIVATE_KEY` secret) or the signing p12 and its password. Never overwrite the
  committed `Scripts/sparkle-public-ed-key.txt`: key-script and release-build trials set
  `AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE` to a scratch file and use throwaway keys in scratch
  folders outside the repo.
- **No feed outside releases.** Dev, sealed and `.dev` builds must never carry the feed or key
  (`--with-updates` is for `release-build.sh` only), so no copy on this Mac can check the feed or
  replace the maintainer's installed app. Never launch a `--with-updates` bundle (e.g.
  `<out>/app/Agent Notch.app` from `release-build.sh`, or anything from a release download): it
  has the real bundle id and would update itself. Delete such scratch bundles when done.

## Working with upstream

- Remotes: `origin` = rivantmedia/agentnotch (the fork; releases are published there) and
  `upstream` = vinzdg/codenotch (moves fast: roughly 200 commits a week). `main` = upstream
  `642d329` + the fork. Upstream's `v1.x` and `preview` tags come in with every fetch of
  upstream, which is why the fork's tags are `agentnotch-v<V>`.
- To merge upstream:
  1. `git fetch upstream && git merge upstream/main`.
  2. Run `Scripts/check-seams.sh`. Every `SEAM` line it reports missing must be re-applied by hand
     at the same anchor, with its `// Fork: <id>` tag.
  3. Run the full definition of done.
- **Upstream files the fork edits** (all tagged `// Fork:`, listed in `Scripts/fork-seams.txt`):
  - `AppDelegate`, `Log`, `Updater`, `StatusItemSummary`;
  - the fork's own updates (UPD): `Updater` (`feedURLString(for:)` always returns
    `Fork.updateFeedURL`; the two copy lines name `Fork.releasesPageURL`; the five
    `// Fork: updates off` guards stay), `AppDelegate` (`updater.start()` at launch; What's New
    through `Fork.showWhatsNewIfNeeded`), `SettingsView` (the "Install updates automatically"
    toggle is `.disabled(!Fork.updatesEnabled)`), and `Tests/UpdaterOutcomeTests.swift` (ALLOW;
    the stalled-check copy names the releases page, never hivinz.com);
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
- **Sparkle runs only in release builds, from the fork's own feed.** `project.yml` and
  `Sources/Info.plist` carry no feed or key (`FORBID SUFeedURL`/`SUPublicEDKey` there, and
  `hivinz.com` in `Updater.swift` and `spm-build-app.sh`); only `spm-build-app.sh
  --with-updates` injects them into the built bundle, and `Updater` hands Sparkle
  `Fork.updateFeedURL` over any Info.plist or defaults value, so an upstream merge can't point
  the app at upstream's feed (which would install the official Codenotch over it). `make
  appcast` stays refused and `FORBID` keeps `pkill -x Codenotch` out of the Makefile, so the
  fork never publishes upstream's feed or kills the official app. Upstream's What's New notes
  are never shown (`Fork.showsUpstreamReleaseNotes`); release notes live on the fork's
  releases.
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

- **Website:** not a user setting. `app-config.json`'s `websiteURL` →
  `Scripts/spm-build-app.sh` (refuses a missing file, bad JSON, or an address
  `CloudWebsite.validated` wouldn't keep exactly as written; `""` omits the key) → Info.plist
  `AgentNotchWebsiteURL` → the bridge (`ClaudeControlConfiguration.websiteURL(infoDictionary:)`)
  → `ClaudeControlConfiguration.websiteURL` → `CloudSync.Dependencies.website`.
  `CloudSync.effectiveWebsite` = `AGENTNOTCH_WEB_URL` (`DevFlags.webURLOverride`) else that,
  validated; fixed for the run; nil (Xcode path, `""`, a refused address) means no sign-in and
  no sync. The Cloud section shows it read-only (plus "Set by AGENTNOTCH_WEB_URL…" when
  overridden). The retired defaults key `claudeControl.cloudWebsiteURL` (the website earlier
  builds let the user type) is deleted by `CloudSync.start` (never sealed); if it named another
  website than the run's, that is a website change: both switches go off, and
  `restoreSession` sets the sign-in made there aside (kept on disk, not used, not revoked).
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
  `CancellationError` (quiet, back to signed out). A sign-in is bound to the website and
  sign-in state it began with (a result that comes back after a sign-out or a stop is revoked,
  never adopted), and a token only ever goes to the website its session was made through. The session is forgotten
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
- **Cost:** `ModelPricing` prices each response from the transcript the way Claude Code
  prices its own `total_cost_usd` (per model and token kind, 1-hour cache writes,
  `inference_geo` "us" ×1.1, web searches, advisor-tool iterations, fast mode). A session's
  `costUsd` is the larger of Claude Code's status-line figure (only when one account ran it)
  and its part's estimate; an unknown model leaves the estimate nil. The status line never
  runs in the VS Code extension's chat panel, Claude Desktop or the SDK. Prices are Claude
  Code 2.1.282's: the model catalog's `pricing_tiers` and each model's tier, plus the fast
  prices, which are constants in its cost function's `speed === "fast"` branch, outside the
  catalog. When updating them, bump `SessionTokenScanner.State.currentVersion` (rescans every
  transcript) and `CloudSyncPass.payloadVersion` (rebuilds sessions already sent as ended,
  once).
- **Claude Desktop:** its usage readings come through `UsageStore` (only with the Desktop cache
  setting on). A Desktop-hosted session is recorded only when
  `~/Library/Application Support/Claude/claude-code-sessions` lists exactly one account and it is
  the session's (a directory listing; no file there is opened). claude.ai chats are never read.
- **Sealed:** `hub.cloud` is a signed-in fixture and every cloud action is a no-op; the bridge's
  browser step refuses to open.

## Releases and updates

- **Contract** (the app gate, `release-build.sh`, `release.yml` and the website's classifier all
  depend on it; change them together):
  - tag `agentnotch-v<V>`, title `Agent Notch <V>`;
  - assets `AgentNotch-<V>.dmg` (UDZO, volume "Agent Notch", `/Applications` link),
    `AgentNotch-<V>.zip` (the Sparkle enclosure, `ditto -c -k --sequesterRsrc --keepParent`) and
    `appcast.xml`;
  - the feed, fixed in the app: `https://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml`;
  - the appcast has one item: `sparkle:version` = `shortVersionString` = V,
    `minimumSystemVersion` 15.0, `releaseNotesLink` `…/releases/tag/agentnotch-v<V>`, and an
    enclosure `…/releases/download/agentnotch-v<V>/AgentNotch-<V>.zip` with length and
    `sparkle:edSignature`. `hardwareRequirements` arm64 appears only with `--host-arch`.
  - Never name a fork asset `Codenotch.dmg`: the README's upstream half links
    `releases/latest/download/Codenotch.dmg` relatively.
- **`release.yml`.**
  - Triggers: a push to main that changes `VERSION` or `Scripts/sparkle-public-ed-key.txt` (the
    first release goes out on the key commit), or `workflow_dispatch` (inputs `dry_run` and
    `rotate_update_key`, labelled "Publish although the update key changed…"). Top-level
    `contents: read`. Concurrency group `release` with `queue: max` and no cancelling: runs
    queue first in, first out, dry runs in the same group, because `fork.yml`'s group
    (`Release-<ref>` inside a Release run) cancels in progress.
  - Job `checks` is `fork.yml` (`workflow_call`). Job `release` runs only in
    rivantmedia/agentnotch, on macos-26 with `Xcode_26.6`, with `contents: write` and
    `environment: release`, whose only deployment branch is main and which holds every secret.
  - The plan step first fails any ref other than main, dry runs included. Then an already
    published version is a green skip (a key-only push on a released version lands here), and
    its own leftover draft is replaced. A foreign draft, the tag at another commit, a `VERSION`
    below the latest `agentnotch-v*` release (`sort -V`), or a committed update key other than
    the one at the latest release's tag (read through the contents API; unreadable is an error
    too) is an error, the key one unless `rotate_update_key` is set. In a dry run each of these
    is only a warning.
  - With no `SPARKLE_ED_PRIVATE_KEY` or no committed public key, it fails with setup steps in
    the run summary (the environment commands; a missing key file means pushing it to main,
    since a re-run checks out the same commit; a missing secret alone means re-running).
  - Otherwise it runs `release-build.sh` and creates a draft with the three assets. The notes
    carry install steps (Gatekeeper/`xattr` only when not notarized) and the updates line,
    plus `--generate-notes` from the previous tag. It checks each uploaded size, then
    publishes with `--draft=false --latest`, then gives a warning-only check that the live
    feed serves V.
  - A dry run uploads the artifact `AgentNotch-<V>-dry-run` and publishes nothing.
- **Secrets** (the maintainer makes them with `release-make-keys.sh`, never a session), all in
  the `release` environment; a repository-level copy reaches every branch and should be deleted:
  - `SPARKLE_ED_PRIVATE_KEY`: required; the base64 of the 32-byte seed.
  - `MACOS_SIGNING_P12_BASE64` and `MACOS_SIGNING_P12_PASSWORD`: optional. A Developer ID, or
    the self-signed identity from `--signing-cert`; a stable identity keeps TCC Automation and
    keychain grants across updates.
  - `APPLE_NOTARY_API_KEY_P8_BASE64`, `APPLE_NOTARY_API_KEY_ID` and
    `APPLE_NOTARY_API_ISSUER_ID`: optional, all three or none, Developer ID only.
  - Rotating the update key strands every install, whatever its code signature: with
    `SUVerifyUpdateBeforeExtraction`, Sparkle checks the zip's EdDSA signature against the
    installed app's `SUPublicEDKey` before unpacking it, and its only fallback needs the archive
    itself to carry a Developer ID signature of the installed app's team, which a zip never
    does. `release-make-keys.sh` refuses to rotate without `--rotate`.
- **App side.**
  - `Fork.updatesEnabled` gates everything. `updater.start()` runs at launch in the non-sealed
    path (tests return earlier), and `feedURLString(for:)` pins the feed.
  - With updates off, the General pane's toggle is disabled and Check now shows the "built
    from source … Download the latest release from <releases page>" copy. The caption under
    the toggle still says updates install in the background; it was left alone to keep the
    seam to one line.
  - Sparkle keeps its settings as `SU*` keys in the app's defaults domain, and its downloads in
    `~/Library/Caches/com.rivantmedia.agentnotch/org.sparkle-project.Sparkle`. With the gate
    closed it makes no controller, so no feed request is sent.
- **Website:** `/download` and `/download/<platform>` (see Layout). Windows and Linux read "Not
  available yet" until a release carries a matching asset (Windows `.exe`, `.msi`,
  `.msix`/`.appx`; Linux `.AppImage`, `.deb`, `.rpm`; or an archive whose name says the
  platform: `assetKind` in `web/src/lib/releases.ts`).

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
- **Cloud:** `AGENTNOTCH_WEB_URL=<url>` overrides the build's sync website (`app-config.json`)
  for one run (https, or http to localhost/127.0.0.1/[::1]; ignored when sealed). A sign-in
  saved for another website is set aside, not deleted. Session summaries never run in a
  `--no-install` run.
- **Sealed-only:** `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH`, `AGENTNOTCH_PANEL_SELF_TEST`, `AGENTNOTCH_SNAPSHOT_CLAUDE`,
  `AGENTNOTCH_SEALED_SWITCH_OFF`, `AGENTNOTCH_SEALED_CAPTURE`.
- **Updates:** no runtime switch. A copy updates itself only when it was built with
  `--with-updates` (`Fork.updatesEnabled`). Build and release scripts take
  `AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE` (a throwaway public key file for tests) and
  `SIGN_KEYCHAIN`.
