WINDOWS PORT SPEC: upstream `windows/` tree (Rust + Tauri 2), mapped for porting Agent Notch's Claude features

Paths are relative to `windows/codenotch/src/` unless another path is given. `windows/` is unmodified upstream 642d329 (1.18.0); `git diff 642d329 HEAD -- windows` is empty. The fork's tooling ignores this tree: `Scripts/check-seams.sh` scans only `Sources/` and `Tests/`, and `Scripts/verify-token-free.sh` never looks under `windows/`.

====================================================================
0. HEADLINE FINDINGS
====================================================================
- **Can't run cargo here.** There is no Rust toolchain on this Mac (`cargo`, `rustc` and `rustup` are all missing). The crate also cannot compile for a macOS target unchanged; two blockers are confirmed in §19.
- **Node checks pass.** The upstream Node UI tests run on this Mac (node v26.3.0) and all pass: `check-ui-scripts.mjs`, `test-light-surface` 2/2, `test-claude-auth-ui` PASS, `test-ko-i18n` 2/2, `test-codex-headline` 5/5.
- **Token-reading path.** It lives in `usage.rs` and `claude_auth.rs`, is started from `main.rs:1891` and `main.rs:667`, and is reached by `doctor.rs:78` and `watcher.rs:81`. The complete kill list is in §9.
- **Would collide with the official Codenotch for Windows.** Unless renamed, these clash: the identifier `com.immidi.codenotch` (single-instance mutex, WebView2 data folder), `%APPDATA%\codenotch`, port 48666, hook entries matched by the substring "codenotch-hook", the Run value "Codenotch", and the NSIS product name. The rebrand is mandatory (§20).
- **Hook protocol is fire-and-forget.** The HTTP body is answered "ok" within about 2 s. It cannot carry permission decisions. PermissionRequest, answering questions and plan approval need a new blocking channel (§5, §22).
- **The notch window can't take keyboard input.** It is `focusable:false` (`tauri.conf.json`), so chat and answering questions need a separate focusable window (or `set_focusable`).
- **The update feed step is probably broken.** With `createUpdaterArtifacts:true` the Tauri v2 bundler emits `-setup.exe` plus `.exe.sig`, not `.nsis.zip`. The `windows-package.yml` feed step looks for `*.nsis.zip` and would throw once a signing key exists (§15).

====================================================================
1. WORKSPACE, BUILD, PACKAGING
====================================================================
**Workspace.** `windows/Cargo.toml` has members `["codenotch","codenotch-hook"]`, resolver 2. The release profile is `lto=true`, `codegen-units=1`, `strip=true`, `opt-level="z"`.

**codenotch** (package 1.18.0, edition 2021, `codenotch/Cargo.toml`):
- tauri 2.11 (lock 2.11.5), features `tray-icon`, `image-png`
- tauri-plugin-single-instance 2 (lock 2.4.4)
- tauri-plugin-updater 2 (lock 2.12.0), default-features off, features `native-tls` and `zip`
- serde / serde_json, tiny_http 0.12, dirs 5.0.1, notify 6.1.1, ureq 2 (`json`, `native-tls`), native-tls 0.2, png 0.17
- chrono 0.4 (`std`, `clock`), rusqlite 0.32 (`bundled`)
- cfg(windows) only: windows 0.58 with Win32_Foundation, UI_Accessibility, UI_WindowsAndMessaging, System_Diagnostics_ToolHelp, Security_Credentials, Graphics_Gdi, UI_Input_KeyboardAndMouse, System_Threading, System_JobObjects, System_Pipes, Globalization, System_Console, System_Registry
- Tauri pulls in windows 0.61 separately; this is why `hwnd()` values are rebuilt as `HWND(raw.0 as _)` (`topmost.rs:44-45`).

**codenotch-hook** 0.3.0 has no dependencies.

**Toolchain minimum** is Rust ≥1.88: `slice::as_chunks` (`settings_window.rs:125`, `glyphs.rs:226`) and `u32::is_multiple_of` (`activity.rs:599`, needs 1.87). CI uses stable.

**`build.rs`**: `tauri_build::build()`.

**`tauri.conf.json`:**
- productName "Codenotch", version "1.18.0", identifier "com.immidi.codenotch"
- `build.frontendDist` "ui"; `app.withGlobalTauri` true (pages use `window.__TAURI__`); CSP null
- The only static window is `label:"notch"`, `url:"notch.html"`, 340×460 (`place_notch` resizes it), with transparent, decorations false, alwaysOnTop, skipTaskbar, resizable false, shadow false, visible false, focus false, focusable false
- bundle: active, targets `["nsis"]`, icon `["icons/icon.ico"]`, createUpdaterArtifacts false
- plugins.updater: endpoints `["https://github.com/vinzdg/codenotch/releases/latest/download/latest.json"]`, pubkey "REPLACE_WITH_TAURI_PUBLIC_KEY", `windows.installMode` "passive"

**`tauri.bundle.conf.json`** (merged only by the packaging job) adds `bundle.resources {"../target/hook/release/codenotch-hook.exe":"codenotch-hook.exe"}`, which puts the hook beside the main exe.

**Capabilities:**
- `capabilities/default.json`: windows ["notch","settings"], permissions core:default, core:window:allow-start-dragging, core:window:allow-close, updater:default.
- `capabilities/dropzones.json`: window "dropzones", permissions core:event:allow-listen and allow-unlisten only.
- App-defined `#[tauri::command]`s are not ACL-gated. Any new window label (sessions panel, chat) must be added to a capability to get core event/window APIs.

**Assets:**
- `icons/icon.ico`, `icons/tray.png` (unused mono), `icons/tray-color.png` (32 px, used for the tray and `get_app_icon`), `ui/tray.png`
- `glyphs/*.svg` (lobehub, MIT, NOTICE.md), embedded with `include_str!` (`glyphs.rs:32-38`)

**`windows/.gitignore`**: `target/`, `*.log`, `codenotch/gen/` (tauri-build writes `gen/schemas` into the crate folder on every build).

**CI — `.github/workflows/windows.yml`:**
- Triggers on `windows/**`; gated by `if: github.repository == 'vinzdg/codenotch'`, so it never runs in the fork.
- Steps: node check-ui-scripts, node tests, dtolnay stable + clippy, rust-cache, `cargo build --locked`, `cargo test --locked`, node codex test, clippy (reported, not enforced).

**CI — `.github/workflows/windows-package.yml`:**
- Gated `vinzdg/codenotch || workflow_dispatch`; triggers on push, PR, release published, dispatch.
- Builds the hook with `RUSTFLAGS=-C target-feature=+crt-static` and `cargo build --release --locked -p codenotch-hook --target-dir target/hook`. It uses its own target dir so the bundler never copies the hook onto itself, and the static CRT because vcruntime may be missing on the machine.
- If the secret `TAURI_SIGNING_PRIVATE_KEY` exists, adds `--config {"bundle":{"createUpdaterArtifacts":true}}`.
- Runs `npx --yes @tauri-apps/cli@2.11.4 build --config tauri.bundle.conf.json -- --locked`.
- Copies `target/release/bundle/nsis/*-setup.exe` to `Codenotch-Setup.exe`.
- Smoke test: silent install (`/S`), find `codenotch-hook.exe` under `%LOCALAPPDATA%` (depth 3), check `codenotch.exe` and `uninstall.exe` sit beside it, run `codenotch.exe doctor`, grep "Codenotch doctor" in stdout or `%APPDATA%\codenotch\doctor.log`, uninstall `/S`.
- Uploads the artifact. The feed step builds `latest.json` from `*-setup.nsis.zip` (see §0).
- On `v*` releases: `gh release upload` of the exe, then of the `.nsis.zip` and `latest.json`.

**The fork's Mac release scheme** (for comparison): tag `agentnotch-v<VERSION>` (VERSION=1.0.0), `releases/latest/download/appcast.xml`. A Windows feed must go into the same release before it is published, or `latest/download` will point at a release without `latest.json`.

**Build commands (README):**
- Dev: `cargo build --release` gives `target\release\codenotch.exe`.
- Installer: `cargo build --release --locked -p codenotch-hook --target-dir target/hook; cd codenotch; npx @tauri-apps/cli@2 build --config tauri.bundle.conf.json` gives `..\target\release\bundle\nsis\Codenotch_<ver>_x64-setup.exe`.

**NSIS behaviour** (Tauri defaults, unverified): per-user install, no admin; WebView2 downloadBootstrapper; unsigned (SmartScreen warns). Tauri v2 bundles the main exe under `mainBinaryName` (recent bundlers default it to productName). Set `mainBinaryName` explicitly in the fork, because the hook launches the app by file name (`codenotch-hook/src/main.rs:80`).

====================================================================
2. PROCESS AND THREAD ARCHITECTURE (`main.rs`)
====================================================================
**Entry** `main()` (`main.rs:1748`):
- CLI subcommands `CONSOLE_CMDS` = install-hooks | uninstall-hooks | autostart on|off | doctor [deep] (1746-1785). They `AttachConsole(ATTACH_PARENT_PROCESS)`, print, write `install.log` / `doctor.log` next to `config.json`, then return.
- Otherwise it runs the GUI. The exe is `windows_subsystem="windows"` in release (line 1).

**Builder chain** (1790-1937):
- `.plugin(updater)`
- `.plugin(single_instance(|app,_args,_cwd| settings_window::open(app)))`: a second launch opens Settings; argv is ignored.
- `.manage(AppState{...})` (48-64): `store: Mutex<state::Store>`, `cfg: Mutex<Config>`, `usage` (Claude), `codex`, `cursor`, `grok`, `antigravity`, `glm` (all `Mutex<UsageSnapshot>`, seeded from each module's `load_persisted()`), `glyphs: Mutex<HashMap<String,Glyph>>`, `activity: Mutex<Vec<Activity>>`.
- `.invoke_handler(generate_handler![...])` (1810-1873).
- `.setup` (1874-1935).

**Setup order and threads:**
1. `place_notch`, `apply_theme`, show the notch.
2. `tray::setup`, `notchmenu::setup`, `start_menu_updater` (2 s poll; rebuilds when ring fractions change, and at least every 60 s).
3. `updater::check_on_launch` (after 20 s).
4. `apply_visibility`.
5. `server::start(port)`: tiny_http thread.
6. `watcher::start`: notify thread, 400 ms loop.
7. `usage::start`: Claude, 60 s while a session exists, else 300 s.
8. `codex/cursor/grok/antigravity/glm::start`: 300 s each; antigravity's CLI mode is on demand with a 5-minute TTL.
9. `activity::start` (2 s).
10. glyph collection thread.
11. `start_pointer_watchdog` (50 ms), `start_work_area_watch` (1 s; also detects system theme changes), `topmost::start_watchdog` (WinEvent hook thread plus a 30 s backstop).
12. ack scan every 1500 ms (`ack_scan`, "seen-clears-it", cfg(windows) only); sweeper every 30 s (`Store::sweep`).
13. `config::save` (the hook reads the port from it).

On-demand threads: `begin_move` (16 ms cursor poll), `drag_begin` (8 ms), `settings_window::open` (hops to the main thread).

**Data flow to the UI.** Each producer mutates its Mutex, persists (providers), then `app.emit(event, full_snapshot)`. The page primes itself with `invoke('get_*')` and then listens. Payloads are always whole snapshots, never diffs. `broadcast()` (81-89) emits "state" = `Store::snapshot(lang, resolved_lang, clock_24h, false)`.

====================================================================
3. IPC: COMMANDS (page → Rust)
====================================================================
JS passes camelCase argument keys (Tauri v2), which map to the snake_case Rust parameters. Returned JSON keeps the serde field names (snake_case).

**State and readings:**
- `get_state` returns the Snapshot (§6).
- `get_usage` returns the Claude UsageSnapshot. `get_codex`, `get_cursor`, `get_grok`, `get_antigravity`, `get_glm` return theirs.
- `get_activity` returns `Vec<Activity>`. `get_glyphs` returns `{id:Glyph}`.

**Claude:**
- `claude_sign_in` returns `Result<(),String>` (→ `claude_auth::start_login`).
- `get_claude_auth` returns `{busy, message}`.

**Updates:** `get_update_state` returns `{available?, checking, installing, message?}`; `check_for_update`; `install_update`.

**Refresh and sessions:**
- `refresh_ring{provider}` returns bool: false when Claude is inside its 429 backoff (674-704).
- `focus_session{id}` returns bool. `dismiss_session{id}`. Both are defined but never called by the page.

**Notch interaction:**
- `show_notch_menu{provider?}`
- `set_hot{rects:[[x,y,w,h] physical px], expanded}`: also triggers the antigravity hover refresh.
- `report_dpr{dpr,w,h,settled?}`, `notch_hidden`, `log_js{msg}` (appends "js: …" to run.log, 600 chars)
- `drag_begin`, `begin_move{depth,length}`

**Settings getters and setters:**
- `set_lang{lang}`, `get_lang`, `get_lang_resolved`
- `get_scale` / `set_scale{scale}` (snapped to 0.8, 1, 1.25)
- `get/set_weekly_ring{placement off|inside|outside}`, `get/set_color_transition{style hard_step|ramp}`
- `get/set_theme{theme system|light|dark}`, `get_theme_resolved`
- `get_tray_options` returns `[{id,label,status,used?}]`
- `get/set_notch_slots{slots:[{provider}]}` (an empty list means all)
- `get/set_antigravity_prefs{limit automatic|5h|weekly, model gemini|3p}`
- `get_app_icon` returns a data URL
- `get/set_ui_flags{notchVisible, trayVisible, notchOnHover?}` returns `{notch_visible, notch_on_hover, tray_visible}`. Tray is forced on when the notch is hidden.
- `get/set_autostart{on}`, `get/set_hooks_installed{on}` return `Result<String,String>`
- `reset_notch_position`
- `get_notch_edge`, `set_notch_edge{edge}`, `get_notch_insets` returns `[t,r,b,l]` CSS px
- `get_monitors` returns `[{id?,label,primary,current}]`, `set_notch_monitor{id?}`
- `get/set_move_handle{on}`

**Windows and misc:** `open_settings`, `open_data_dir` (explorer), `get_zones` returns `Zones?`, `get_system_look` returns `{mica, accent[7 #rrggbb]}`, `quit_app`, `open_author_page` (x.com/hivinz_).

====================================================================
4. IPC: EVENTS (Rust → pages)
====================================================================
- **"state"**: Snapshot. Emitted by `broadcast` from the server, watcher, ack and sweeper.
- **Provider snapshots**: "usage" (`usage.rs:514`, 681), "codex", "cursor", "grok", "antigravity", "glm", each a UsageSnapshot. "activity" is `Vec<Activity>`; "glyphs" is the map.
- **Notch window (emit_to "notch")**: "notch_edge"(String), "notch_insets"([f64;4]) (both from `place_notch`), "notch_landing"(), "notch_reveal"(), "notch_pointer"(bool).
- **Pointer and moves**: "pointer_left"(), "move_end"(), "move_target"(String), "drag_end"(bool).
- **Settings changes**: "theme_resolved"(String), "theme", "weekly_ring", "color_transition", "antigravity_prefs"{limit,model}, "notch_slots"[{provider}], "ui_flags"{…}, "move_handle"(bool).
- **"update_state"**: UpdateState.
- **"zones"**: `{w,h,depth,length,target}`, emit_to "dropzones".
- **"notice"** is listened for (`notch.html:1556`) but never emitted.

Page wiring: `notch.html:280-281` (invoke/listen), primes and listeners at 1546-1602. Settings uses `call(cmd,args,fallback)` wrappers (`settings.html:227-234`).

====================================================================
5. LOCAL HTTP SERVER AND HOOK BINARY PROTOCOL
====================================================================
**Hook binary** (`codenotch-hook/src/main.rs`):
- `argv[1]` is the internal event name (default "ping", line 13).
- Reads stdin up to 256 KiB (the Claude Code hook JSON).
- Port: hand-scans `%APPDATA%\codenotch\config.json` for `"port"` and its digits, default 48666 (37-56).
- ppid: `NtQueryInformationProcess(-1, ProcessBasicInformation)` gives InheritedFromUniqueProcessId (99-135). The parent is whatever shell Claude Code launched the hook through, so `focus.rs` walks up the chain.
- Sends raw HTTP over `TcpStream::connect_timeout(127.0.0.1, 300 ms)` with 700 ms write/read timeouts:

```
POST /event?e=<event>&ppid=<pid> HTTP/1.1
Host: 127.0.0.1
Content-Type: application/json
Content-Length: N
Connection: close

<stdin JSON>
```

  It then reads up to 64 bytes and ignores them.
- On failure it runs `spawn_main()`: `<hook dir>\codenotch.exe` with DETACHED_PROCESS|CREATE_NO_WINDOW and null stdio, then retries 20×100 ms.
- Always exits 0 and prints nothing. Its rule, quoted: "never block Claude Code — ~2 s total budget".

**Server** (`server.rs:9-54`):
- `tiny_http::Server::http(("127.0.0.1", port))`. On bind failure it `eprintln`s and the thread returns, so the failure is invisible in the GUI.
- `/event*`: only POST (405 otherwise); a CSRF guard returns 403 (`is_forbidden_headers` 111-127: any Origin not http(s)://127.0.0.1|localhost|[::1][:port], more than one Origin, or `Sec-Fetch-Site: cross-site`). The body is capped at 256 KiB, parsed by `parse` (67-92), `store.apply`, and a broadcast if anything changed. Every request, any path, gets "ok".
- No authentication: any local process or user can inject events.

**Fields consumed from the hook JSON**: `session_id` (empty becomes "unknown"), `cwd`, `prompt`, `message`, `tool_name`, `tool_input.command`, `model`. The query supplies `e` and `ppid`.
**Ignored**: `transcript_path`, `hook_event_name`, `permission_suggestions`, `agent_id`, `background_tasks`. `CLAUDE_CONFIG_DIR` and `CLAUDE_PID` are not forwarded.

**Windows mapping for the fork:**
- Replace with an `agentnotch-hook.exe` that forwards `CLAUDE_CONFIG_DIR`, `CLAUDE_PID`, `CLAUDE_CODE_ENTRYPOINT`, `hook_event_name` and the full JSON.
- PermissionRequest must block until the app answers. Use a named pipe (`\\.\pipe\agentnotch-<SID>`, DACL current user only, `PIPE_REJECT_REMOTE_CLIENTS`) instead of unauthenticated TCP, and print Claude Code's `hookSpecificOutput`.
- The fork's Python hook can't be reused as-is: CPython on Windows has no `socket.AF_UNIX` and Python may not be installed.
- tiny_http can hold a `Request` on another thread and respond later if TCP is kept.
- Keep fail-open: a missing app means exit 0 with no output. The auto-launch in `spawn_main` conflicts with that when a decision is pending.

====================================================================
6. SESSION STATE MACHINE (`state.rs`)
====================================================================
**States**: running | attention | done | idle; aggregate priority attention > running > done > idle.

**Constants**:

| Constant | Value | Meaning |
|---|---|---|
| RUNNING_STALE | 30 min | running goes to idle |
| DONE_STALE | 24 h | done is removed |
| IDLE_DROP | 10 min | idle is removed |
| ATTENTION_STALE | 24 h | attention is removed |
| MAX_SESSIONS | 200 | evict the oldest `last_event` |
| MAX_CWD_CHARS | 1024 | |
| HOOK_FRESH_MS | 5 min | watcher events ignored while hook data is fresh |

**`apply(ev)`** (116-217):
- session_end removes the entry.
- Otherwise the entry is created first, so unknown or "ping" events still create one.
- Watch-sourced events are dropped when `last_hook` < 5 min ago.
- session_start: idle, unless running.
- running: sets started on the transition, clears attn, prompt = first 120 chars, `last` = "🔧 tool: cmd(60)".
- attention: attn = message(200).
- done: total = now − started, clears attn.
- Returns whether state, last, attn, prompt or model changed.
- title = `"<last path segment of cwd> · <first 4 of id>"`.

**Other methods**: `dismiss`, `has_done`, `ack_done(pred)` (done→idle), `sweep`, `ppid_of`.

**Snapshot JSON**:

```
{ sessions:[{id,title,state,started,total,last,attn,prompt,model}],
  agg, counts:{attention,running,done}, lang, lang_resolved, clock_24h, drag:false }
```

Sorted by rank, then newest `started`. ppid, cwd and last_hook are `serde(skip)`.

**Seen-clears-it** (`main.rs:1692-1717`): every 1.5 s, if any session is done, take `GetForegroundWindow`'s pid and a Toolhelp process map. A done session whose ppid chain contains the foreground pid (or its parent) becomes idle. For ppid=0 (watcher sessions), the foreground exe name must contain "claude" and not "codenotch".

**Hook wiring** (`hooks_install.rs:8-16`):

| Claude Code event | Internal event |
|---|---|
| SessionStart | session_start |
| UserPromptSubmit | running |
| PreToolUse (matcher "*") | running |
| PostToolUse (matcher "*") | running |
| Notification | attention |
| Stop | done |
| SessionEnd | session_end |

No PermissionRequest, SubagentStop, background tasks or task-notification handling.

**Gap to the fork's five-state model** (needs you / failed / ready for review / working / idle): the fork needs account, transcript path, tasks, context %, pending prompts, the Stop `background_tasks` wait and the `idle_prompt` rule. Treat `state.rs` as display-only legacy and feed the UI from the fork engine rather than extending Store.

====================================================================
7. TRANSCRIPT WATCHER (`watcher.rs`): fallback for the Claude desktop app
====================================================================
**Roots** (76-106):
- `usage::profile_dirs()` joined with `projects`: `~/.claude` plus `~/.claude-<slug>` that hold `.credentials.json` or `credentials.json`.
- `%APPDATA%\Claude\local-agent-mode-sessions`.
- `%LOCALAPPDATA%\Packages\<name containing claude|anthropic>\LocalCache\Roaming\Claude\local-agent-mode-sessions` (MSIX virtualization; useful precedent for the Desktop paths the fork needs).
- Missing roots are retried every 60 s.

**Accepted files** (`is_session_jsonl` 109-128): `*.jsonl`, not `audit.jsonl`, no path component "subagents", and a path component exactly ".claude". **Upstream bug**: `~/.claude-work\projects\…` has no ".claude" component, so secondary-account transcripts are silently ignored.

**Loop**:
- notify recursive watch, events collected into a dirty set.
- Ingest at most once per 800 ms per file.
- Re-scan every 45 s for files with mtime inside the last 10 min.
- `watch.log` is truncated at start and the first 30 pushes are logged.

**`tail_info`** (266-307): the last 256 KiB, the last 80 non-empty lines backwards. It picks:
- the newest user or assistant entry,
- `message.model` of an assistant entry,
- the latest user text (string content or text blocks, skipping tool_result entries, 120 chars).

sessionId comes from the entry's `sessionId` (camelCase), else the file stem. A cwd containing "local-agent-mode-sessions" is shown as "Claude desktop".

**Inference** (442-476), where quiet = time since the last append:

| Last entry | Condition | Result |
|---|---|---|
| any append | — | running |
| assistant text | quiet > 2.5 s | done |
| assistant tool_use | quiet > 20 s | attention (inferred) |
| user with "interrupt" | quiet > 2.5 s | done |
| user | quiet > 75 s | done |
| other | quiet > 5 min | done |

Pushed as HookEvent with `src:"watch"` and ppid 0.

**Fork mapping**: the fork prefers hook `transcript_path` and account-aware scanning. Reuse the MSIX path logic and the notify plus rescan pattern. With Parallel Profiles on Windows (`%USERPROFILE%\.claude-windows\<12hex>\projects`, linked into `.claude-shared`), expect junctions or symlinks: watch the resolved physical folder once, and use `symlink_metadata` or FILE_FLAG_OPEN_REPARSE_POINT where the fork's rule is "never through a link".

====================================================================
8. ACTIVITY PROBE (`activity.rs`), 2 s, lowered thread priority
====================================================================
`Activity{provider, state: busy|waiting, name, detail, since}`.
- **Cursor**: `state.vscdb` table `composerHeaders` (`unfinishedRunAt`, `hasBlockingPendingActions`, `hasPendingPlan`).
- **Codex**: `~/.codex/thread_history_1.sqlite` `thread_turns` inProgress, plus `state_5.sqlite` names; otherwise classify the rollout tail.
- **Antigravity**: `~/.gemini/antigravity*/brain/*/.system_generated/logs/transcript.jsonl` mtime within 45 s.
- **Claude cloud sessions** (391-496, Windows only):
  - A PowerShell `Get-CimInstance Win32_Process Name='claude.exe'` filtered on a CommandLine containing `network.mojom.NetworkService` finds the network pid (cached 5 min; a miss is cached 60 s).
  - `GetProcessIoCounters.OtherTransferCount` rate ≥ 2500 B/s on two consecutive samples means busy; held 10 s.
  - The page uses it as Claude "running" when `agg` is idle (`notch.html:944-956`).

====================================================================
9. CLAUDE USAGE TOKEN PATH: everything to disable (Windows equivalent of Mac seam U1)
====================================================================
**`usage.rs`**:
- `ENDPOINT` = `https://api.anthropic.com/api/oauth/usage` (:30); header `anthropic-beta: oauth-2025-04-20`; 15 s timeout.
- `CRED_NAMES` = [".credentials.json", "credentials.json"] (:68).
- `has_credential(dir)` (:93): stat of those files.
- `profiles()` (:100-122): `~/.claude` plus every `~/.claude-<slug>` that is a directory holding a credential. `profile_dirs()` (:125) is used by `watcher::roots`.
- `read_credentials(dir)` (:196-218): **reads token files**; `claudeAiOauth.accessToken`, `expiresAt`, `subscriptionType`.
- `probe_credentials()` (:221-249): doctor; reads credentials (prints token length and expiry).
- `is_desktop_owned` (:255) and `find_cli()` (:261-282), which looks for standalone claude in this order: `%USERPROFILE%\.local\bin\claude.exe`, `%APPDATA%\npm\claude.cmd`, `%LOCALAPPDATA%\pnpm\claude.cmd`, `%USERPROFILE%\.volta\bin\claude.exe`, PATH `claude.exe|claude.cmd`, excluding `\AnthropicClaude\`, `\Claude\claude-code\`, `\WindowsApps\`. Reusable for the fork's probe and summaries.
- `should_renew`/`retry_wait_ms` (:285-308), `run_renewal` (:312-343: runs `claude -p` with null stdin and `CLAUDE_CONFIG_DIR` set to force Claude Code's startup token refresh; strips CLAUDECODE and CLAUDE_CODE_*; CREATE_NO_WINDOW; 30 s kill), `Renewer::maybe_renew` (:356-386). **This refreshes tokens, which the fork forbids** (rotating refresh tokens log Claude Code out).
- `fetch_once(token)` (:471-497) sends the bearer token. `parse_response` (:412-463) handles `limits[{kind,percent,resets_at}]` and falls back to `five_hour`/`seven_day{utilization,resets_at}`; ids session, weekly_all, seven_day, …
- `poll_account` (:604-673), `aggregate` (:567-601), `start` (:675-730), `usage.json` persistence (:160-178).
- Multi-account encoding reusable by the fork: the default account's window ids stay bare; secondary ids get `@slug`, and `group` = `"<slug|default> · <plan>"` when more than one account exists (`decorate` :537-547). The page splits into one cell per group (`notch.html:975-992`).

**`claude_auth.rs`**:
- `start_login` (:86-101) runs PowerShell `LOGIN_SCRIPT` (:38): `& $env:CODENOTCH_CLAUDE_CLI auth login --claudeai` in a visible console (CREATE_NEW_CONSOLE 0x10), 15-minute timeout, `taskkill /T /F` tree cleanup. It removes CLAUDECODE, CLAUDE_CODE_*, CLAUDE_CONFIG_DIR, ANTHROPIC_API_KEY and ANTHROPIC_AUTH_TOKEN from the environment.
- `BUSY` gate `try_acquire` (:26) is shared with renewal. `usage_succeeded` (:33).

**Call sites**:
- `main.rs:667` (`claude_sign_in`), `:670`, `:677-680` (`refresh_provider("claude")` → `usage::request_refresh`, `usage.backoff_until`), `:1801` (`usage::load_persisted`), `:1891` (`usage::start`).
- `doctor.rs:78` (`probe_credentials`), `watcher.rs:81` (`profile_dirs`).
- `notch.html:1146-1166` (sign-in UI, CLAUDE_AUTH_ACTIONS markers), `:1593-1602` (`get_claude_auth` poll). `scripts/test-claude-auth-ui.cjs` tests that UI.

**Borderline**: `glm.rs:142-153` `claude_code_key()` reads `~/.claude/settings.json` `env.ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY`, and only uses it when `ANTHROPIC_BASE_URL` is z.ai or bigmodel.cn.

**Minimal fork seam set**:
- (a) `main.rs:1891`: replace `usage::start` with the fork's usage feed, which writes `AppState.usage` and emits "usage" in the same shape.
- (b) `usage.rs` `profiles()`: return `vec![]` as belt-and-braces.
- (c) `main.rs:667`: `claude_sign_in` returns Err (or drop the notch.html block).
- (d) `main.rs:677`: route to the fork's refresh.
- (e) `doctor.rs:78`: fork probe.
- (f) Add a Rust `verify-token-free` check over `windows/` for `.credentials.json`, `credentials.json`, `accessToken`, `api/oauth/usage`, `oauth-2025-04-20`, `auth login`, `run_renewal`, `maybe_renew`, `read_credentials(`, and CredRead for Claude.

====================================================================
10. OTHER PROVIDERS (context; unchanged by the fork)
====================================================================
All follow the same pattern: `static REFRESH` AtomicBool plus `request_refresh()`, `load_persisted()`, `present()`, `start()`, `probe()`. Status "absent" hides the cell.

| Provider | Credential / source | Endpoint and notes | Persisted to |
|---|---|---|---|
| Codex | `~/.codex/auth.json` tokens | `chatgpt.com/backend-api/wham/usage`; fallback native `codex.exe app-server` JSON-RPC (initialize, then `account/rateLimits/read`, 20 s, 30-minute stand-down); rollout fallback via `state_5.sqlite` index | `codex.json` |
| Cursor | `%APPDATA%\Cursor\User\globalStorage\state.vscdb` (`cursorAuth/accessToken` + `stripeMembershipAuthId` as a cookie) | `cursor.com/api/usage-summary` | `cursor.json` |
| Grok | `~/.grok/auth.json`, issuer `auth.x.ai` only | `cli-chat-proxy.grok.com/v1/billing?format=credits` | `grok.json` |
| GLM / z.ai | `glm.json` (manual key), Claude settings env, `~/.zcode/v2/*`, opencode `auth.json` | `{console}/api/monitor/usage/quota/limit` | `glm-usage.json` |
| Antigravity | `agy.exe --sandbox --print-timeout 30s --print /usage` under ConPTY + JobObject (70 s, 64 KB, `quota-work\` cwd); else `language_server` bridge (PowerShell CIM command line, netstat ports, self-signed TLS on 127.0.0.1), Credential Manager `gemini:antigravity`, cloudcode-pa APIs, transcript counts | — | `antigravity.json` (atomic write) |

**Reusable Windows primitives** in `agy_cli.rs`:
- `run_cmd_conpty` (:231-482): CreateJobObject with KILL_ON_JOB_CLOSE, CREATE_SUSPENDED, then AssignProcessToJobObject, TerminateJobObject on timeout.
- `quote_arg` (CommandLineToArgvW rules, :203).
- `atomic_write` (temp file, sync, rename, :157-194).

The JobObject pattern is what the fork needs for `claude -p` probe and summary children ("cancelling stops the child at once"). Use plain pipes rather than ConPTY for the stream-json stdin/stdout protocol.

====================================================================
11. SETTINGS STORAGE
====================================================================
**Folder**: `config::config_path()` = `dirs::config_dir()/codenotch/config.json`, i.e. `%APPDATA%\codenotch\config.json` (FOLDERID_RoamingAppData). `watcher.rs:37`, `:141` and `doctor.rs:88` hard-code the same "codenotch" folder independently.

**Format**: pretty JSON via serde. Saved with plain `fs::write` (not atomic) on every change and once at setup. Unknown or malformed fields fall back per field.

**Config fields** (`config.rs:27-110`):
- `port` (48666), `lang` ("auto"|zh|zh-Hant|en|ja|ko|pt-BR|ru|uk)
- legacy: `bar_x`, `bar_y`, `bar_w`, `drag_enabled`; `notch_y` (read-only migration)
- `notch_along{edge: 0..1}`, `notch_edge` ("right"), `notch_monitor` (device name like `\\.\DISPLAY2` | null)
- `scale` (0.8|1|1.25), `weekly_ring` ("off"; the fork's Mac default is outside), `color_transition` ("hard_step"), `theme` ("system")
- `notch_providers` (legacy), `notch_slots[{provider}]` (empty means all)
- `antigravity_limit` ("automatic"), `antigravity_model` ("gemini"), `glm_notch_fixed`
- `notch_visible`, `notch_on_hover` (fresh install true; an upgraded config without the key gets false), `tray_visible`, `show_move_handle`

`load()` migrations: notch_providers → notch_slots, notch_y → notch_along, add GLM once, force the tray on if both switches are off, snap the scale (`config.rs:273-307`).

**Other files in the same folder**:
- `run.log` (≤1 MiB, then restarts; `main.rs:847-860`), `watch.log`, `install.log`, `doctor.log`
- `usage.json`, `codex.json`, `cursor.json`, `grok.json`, `antigravity.json`, `glm-usage.json`, `glm.json`
- `glyphs\` (user overrides; also `<exe dir>\glyphs\`), `quota-work\`

**Elsewhere**:
- WebView2 data: `%LOCALAPPDATA%\<identifier>\EBWebView` (Tauri default).
- Settings page localStorage key `codenotch.settings.pane`.
- Registry reads: HKCU `…\Explorer\Accent\AccentPalette`, HKLM `…\Windows NT\CurrentVersion\CurrentBuildNumber` (Mica when ≥22000), HKCU `…\Explorer\Advanced\ShowSecondsInSystemClock`.

**Test isolation caveat**: on Windows, `dirs` 5 resolves home and config through Known Folder APIs and ignores `USERPROFILE`/`APPDATA`. The fork's engine must take injectable roots, not `dirs::*`, for its "temporary HOME" test rule.

====================================================================
12. WINDOW AND EDGE PLACEMENT MODEL
====================================================================
**Logical design sizes**:
- `NOTCH_W` = 360, `NOTCH_LONG` = 650 (`main.rs:37`, `:46`).
- Upright (left/right) is 360×650; flat (top/bottom) is 650×650 (`notch_window_size` :286).
- Physical size = design × monitor scale × size factor, clamped to the monitor.
- The page's `DESIGN_W_UPRIGHT`/`DESIGN_W_FLAT` (`notch.html:1316`) are pinned to these by a test.

**Placement** (`place_notch` :294-363):
- target screen = `cfg.notch_monitor` if attached, else primary (`screens` :131, `target_screen` :152).
- `edge_origin` (:173-185) centres the window at ratio `notch_along[edge]` along the **work area** (not the monitor).
- set_size, set_zoom, set_position; re-pinned if the DPI changed the size.
- emits notch_edge and notch_insets (taskbar overlap in CSS px, `work_insets` :264).
- The work area is re-polled every 1 s (taskbar moved).

**DPI**: `zoom_notch` and `report_dpr` (:828-907) correct WebView2 DPR mismatches with `set_zoom`, at most 3 corrections. `land_on_another_screen` (:208-232) hides the page (not the window) while crossing monitors of different scale: notch_landing, then `notch_hidden`, then move, then a settled `report_dpr` or a 700 ms fallback, then notch_reveal.

**Click-through**: the page reports hot rects in physical px (pill; plus tail and card when open; plus orb and move handle). A 50 ms watchdog compares `cursor_position` to the rects (±10 px pad, bounding box for more than one rect; `cursor_in_hot` :916) and toggles `set_ignore_cursor_events`. It emits `notch_pointer`, and `pointer_left` after 300 ms outside while expanded (:963-1016). **Any fork UI drawn inside the notch window must be included in `set_hot` rects or it cannot be clicked.**

**Show on hover**: page-side fold/unfold with a 450 ms grace; the folded wake rect is the resting pill plus a 34 px band (`notch.html:1295-1305`, 1526-1545).

**Moves**:
- Alt+drag slides along the edge (`drag_begin` :566-630, saves `notch_along[edge]`).
- The move handle carries it to a different edge (`begin_move` :466-564). The `dropzones` window (click-through, unfocusable, on the work area, `dropzones.rs`) shows four silhouettes; the nearest edge wins (`edge_at` :447); monitors can be crossed. A monitor the platform won't name keeps its screen.

**Topmost** (`topmost.rs`): tao never re-asserts `alwaysOnTop`. A `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` plus 30 s backstop detects the notch leaving the topmost band (EnumWindows z-order walk) and calls `SetWindowPos(HWND_TOPMOST, NOACTIVATE)`.

**Theme**: `theme_script` is injected before first paint (`window.__CN_THEME__` sets `data-theme`); `apply_theme` sets all windows and emits theme_resolved. The settings window uses Mica / MicaLight / MicaDark on Windows 11.

**Settings window** (`settings_window.rs`): created on demand on the main thread through a spawned hop (building a window inside a sync command deadlocks WebView2), 680×520, frameless, shadow, centred, destroyed on close. Title "Codenotch Settings".

**Fork note**: the sessions panel with approve/deny can live in the notch window (hot rects). Chat and answer fields need keyboard focus, so use a new focusable `WebviewWindowBuilder` window. Anchor it to the ring using `place_notch`'s geometry (edge, insets, screen), give it a capability entry, and create it through the same main-thread hop.

====================================================================
13. TRAY AND NOTCH MENU
====================================================================
**Tray** (`tray.rs`):
- `TrayIconBuilder::with_id("main")`, icon `tray-color.png`, tooltip "Codenotch v<ver>", later "Codenotch — Claude 61% · …".
- Menu opens on left click. Lines per provider: header "Claude — 61% · 20m ago" with id `refresh:<id>`, then disabled indented window lines `line:<id>:<n>`. Then "refresh", "settings", "quit".
- Rebuilt only when the text changes, always on the main thread (`refresh_menu` :133-155).
- Text formatting lives in `traymenu.rs` (pct, used_left, reset_text, ago, label, header, window_line, stale_since 15 min; `system_datetime` uses GetDateFormatEx/GetTimeFormatEx).
- Multi-account lines don't name the account.

**Notch right-click** (`notchmenu.rs`): `show_notch_menu` popup with ids prefixed "notch:": refresh, `open:<provider>` (usage page, `main.rs:764-786`), keep_open (check item), quit. Foreground is handed back after the popup closes (`give_back`). The ring-to-id mapping for the tray is `ring_window` (`main.rs:1218-1236`), mirrored by `headlineOf` in `notch.html:1023-1034`.

====================================================================
14. FOCUS / JUMP (`focus.rs`)
====================================================================
- `focus_terminal(pid)` (:5-115): Toolhelp32 ppid map, ancestor chain up to 8 levels, EnumWindows over visible titled top-level windows. Scored by the window pid's position in the chain (or its parent's, the conhost case); highest ancestor wins. Then `ShowWindow(SW_RESTORE)` if iconic, `SetForegroundWindow`, `FlashWindowEx(FLASHW_ALL, 2)`. It reaches a top-level window only, not the tab.
- `focus_claude_desktop()` (:209-269): the largest visible window of a process named `*claude*` (not `*codenotch*`).
- Helpers: `proc_maps`, `fg_pid`, `chain_of`, `pid_hits_chain` (Windows only, no stubs; used only in cfg(windows) code).

**Fork "exact terminal tab" on Windows is hard**:
- Windows Terminal has no public focus-by-pid. The options are UI Automation (TabItem selection, matching tab title) or `wt -w <id> focus-tab -t <idx>` with an unknown index.
- VS Code needs the Parallel Profiles extension or a URI handler.
- `SetForegroundWindow` is subject to foreground-lock rules and may only flash.
- Keystroke or "chat" injection (the analog of tmux send-keys) would mean `AttachConsole(pid)` + `WriteConsoleInputW`. The fork's rules forbid doing this to real terminals in tests.

====================================================================
15. UPDATER (`updater.rs` + config)
====================================================================
- `configured()` (:64-72) is false while pubkey = `UNSET_PUBKEY` "REPLACE_WITH_TAURI_PUBLIC_KEY"; all updater commands are then silent no-ops.
- `check_for_update` (:79-108) runs `app.updater()?.check()` on a thread and emits `update_state`.
- `install_update` (:112-143) re-checks, then `download_and_install` (minisign verified against the pubkey; NSIS passive; the app exits and restarts).
- `check_on_launch` runs at 20 s. No nag.

**Feed format**:

```
{ version, pub_date,
  platforms: { "windows-x86_64": { signature, url } } }
```

The endpoint is `releases/latest/download/latest.json` (`tauri.conf.json`).

**Artifacts**: `createUpdaterArtifacts:true` produces `-setup.exe` + `-setup.exe.sig`; `"v1Compatible"` produces `.nsis.zip` + `.sig`. Upstream's workflow expects the latter.

**Fork**:
- `plugins.updater.endpoints` → `https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json`.
- Own minisign keypair (`npx @tauri-apps/cli signer generate`), secret in the fork's `release` environment.
- URL uses tag `agentnotch-v<VERSION>`.
- Upload into the same draft release as `appcast.xml` before it is published.
- Set `version` from `VERSION`.
- `settings.html:199` shows a hard-coded "0.3.0"; nothing sets `about-version` (bug).

====================================================================
16. AUTOSTART, SINGLE INSTANCE, DOCTOR
====================================================================
- **Autostart** (`autostart.rs`): `reg.exe` query/add/delete of `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value "Codenotch" = `"<exe>" --silent`. `--silent` is never parsed (`main.rs:1750-1785` ignores it).
- **Single instance**: the plugin keys its mutex and window class on the config `identifier`. The callback only opens Settings (`main.rs:1792-1797`). For the fork this is where a second launch's argv lands, e.g. an `agentnotch://auth-callback?code=…` deep link for the website sign-in on Windows (register `HKCU\Software\Classes\agentnotch` or use tauri-plugin-deep-link). **Seam here.**
- **doctor** (`doctor.rs`): config and port, whether port 48666 is free, watcher roots with the newest 5 transcripts and their parsed tail, provider probes (including Claude `probe_credentials`), glyphs, Codex activity, `watch.log` tail.
- **doctor deep** (`diag.rs`): recent files, Codex SQLite structure (values reduced to type and length), JSON keys, process pid/name (never command lines).

====================================================================
17. i18n
====================================================================
There are three independent tables, keyed by the exact English string:
- Tray, Rust: `i18n.rs` `tr(lang,key)` and `traymenu.rs` `label` in en, ru, zh, zh-Hant, ja, ko, pt-BR, uk. `resolve_auto` maps `GetUserDefaultLocaleName` to a code; `clock_24h` comes from `GetLocaleInfoEx`.
- Card: `notch.html` `TEXT` (:390), `PATTERNS` (:664), `UI` (:896), in ko, pt-BR, en, uk, ru, zh, zh-Hant (no ja). Language comes from `Snapshot.lang_resolved`.
- Settings: `settings.html` `STATIC_TEXT`/`STATUS_TEXT`, `ui(key,fallback)`.

"Codenotch" appears inside many translated strings (e.g. `i18n.rs:118`, `202-208`, and dozens in `settings.html:260-700`). The fork's rebrand needs a JS and Rust equivalent of `Fork.rebranded`. The fork's own UI can be English-only.

====================================================================
18. TESTS
====================================================================
There are 136 `#[test]`: 3 `#[ignore]` (live CLI) and 4 `cfg(windows)`-only.

| Module | Tests | Coverage |
|---|---|---|
| `main.rs` | 29 | placement math, work insets, hot-rect geometry (#106 values), ring/headline selection per provider, Antigravity lanes, `notch.html` design widths and palettes (include_str) |
| `codex.rs` | 20 (1 ignored) | app-server snapshot, rollout parsing, sqlite index, extras/Spark (temp dirs) |
| `usage.rs` | 14 (1 ignored) | renewal timing, backoff, desktop CLI refusal, multi-account decorate/split/aggregate. `the_default_account_is_first_and_always_listed` lists the **real home** and stats `~/.claude-*/.credentials.json`, so run it only with a temp HOME on macOS (Windows ignores env) |
| `traymenu.rs` | 11 | formatting, languages, card/menu name agreement (include_str `notch.html`) |
| `server.rs` | 10 | Origin/CSRF plus a loopback integration on port 0 |
| `i18n.rs` | 9 | |
| `agy_cli.rs` | 8 (1 ignored; ConPTY runner and timeout Windows-only) | |
| `antigravity.rs` | 8 | temp dirs |
| `config.rs` | 7 | |
| `grok.rs` | 7 | |
| `diag.rs` | 5 | never leaks values |
| `state.rs` | 4 | cap, cwd cap, attention sweep |
| `claude_auth.rs` | 3 (2 Windows-only, running powershell) | |
| `settings_window.rs` | 1 | accent palette |

**No tests at all**: `hooks_install`, `watcher`, `focus`, `topmost`, `updater`, `activity`, `cursor`, `glm`, `glyphs`, `autostart`, `dropzones`, `notchmenu`, `tray`, `trayicon`, `doctor`.

**Node tests** (CI order):
- `scripts/check-ui-scripts.mjs`: vm-parses inline `<script>` of every `ui/*.html`; skips `src=` scripts, so extend it for fork JS files.
- `test-light-surface.cjs`: palette parity, theme injection greps against `main.rs`, `settings_window.rs`, `dropzones.rs`.
- `scripts/test-claude-auth-ui.cjs`: the CLAUDE_AUTH_ACTIONS block.
- `scripts/test-ko-i18n.cjs`.
- `test-codex-headline.cjs`: BEGIN/END TESTABLE HEADLINE/WEEKLY markers.

The marker-extraction technique (vm on a marked source slice) is a good pattern for testing fork UI logic in Node on the Mac.

====================================================================
19. cfg(windows) GATING AND MAC-HOST BUILDABILITY
====================================================================
**Gating**:
- Gated with non-Windows stubs: `main.rs` (`left_button_down`, `ack_scan`, `attach_console`, creation_flags), `focus.rs`, `topmost.rs`, `notchmenu.rs` (`hwnd`/`give_back`), `activity.rs` (`claude_net_pid`, `claude_io_bytes`, `lower_thread_priority`), `glyphs.rs` (`from_exe`), `traymenu.rs` (`system_datetime`), `i18n.rs`, `settings_window.rs` (registry), `agy_cli.rs` (`run_cmd_conpty`), `antigravity.rs` (discover via ps/lsof, `listening_ports`, `read_credential_raw`), `diag.rs`, `codex.rs`, `claude_auth.rs`, and the hook's `parent_pid` (unix `parent_id`).
- Ungated, runtime-only Windows assumptions that still compile everywhere: `explorer`, `cmd /C start`, `reg`, `powershell`, `netstat` commands; `.exe` names; `%APPDATA%` in the hook.

**The macOS target does not compile unchanged** (confirmed against Tauri docs and source):
1. `WebviewWindowBuilder::transparent` is "Available on crate feature `macos-private-api` or non-macOS only". It is used in `dropzones.rs:46` and `settings_window.rs:49`. Enabling it also requires `app.macOSPrivateApi:true`, and tauri-build checks config/feature parity.
2. `tauri::generate_context!` on non-Windows looks for a `.png` in `bundle.icon`, else `icons/icon.png`. Neither exists (only `icon.ico`), so the build fails at codegen. Workaround: `TAURI_CONFIG='{"bundle":{"icon":["icons/tray-color.png","icons/icon.ico"]}}'`, or add a png.

Beyond those two, the code appears macOS-clean (rusqlite bundled, native-tls, notify, and tiny_http are cross-platform), but this is unverified since there is no toolchain.

**Mac-host options**:
- (a) `cargo check`/`cargo test` of the app crate after the icon, transparency and features workarounds (fork-only build tweaks).
- (b) `cargo xwin check`/`build --target x86_64-pc-windows-msvc`. Tauri documents experimental NSIS cross-builds from macOS with cargo-xwin, llvm and nsis. This avoids cfg issues but needs the MSVC SDK headers download, and rusqlite compiles C.
- (c) Recommended: put all fork logic in a Tauri-free library crate (e.g. `windows/agentnotch-engine`) with Win32 behind traits and cfg. Its `cargo test` then runs natively on the Mac (after installing rustup) and on windows-latest CI, and the Tauri app is built only on CI.

====================================================================
20. REBRAND CHECKLIST
====================================================================
- **Config and Cargo**: `tauri.conf.json` productName, identifier (single-instance mutex, WebView2 data dir, AUMID for toasts), version, `mainBinaryName`, updater endpoints and pubkey. Cargo package names and repository URLs (both crates, workspace members). `tauri.bundle.conf.json` hook resource name.
- **Data folder name**: `config.rs:269`, `watcher.rs:37`, `:141`, `doctor.rs:88`, hook `main.rs:39`.
- **Main exe name**: hook `main.rs:80`.
- **Hook identity**: `hooks_install.rs:29`, `52`, `61`, `71`. `is_ours` also matches the official app's "codenotch-hook", "eatbean-hook" and "pacman-hook" entries. Use a distinct substring so neither app deletes the other's entries.
- **Port**: `config.rs:229` and hook `DEFAULT_PORT` (both 48666), or drop TCP.
- **Run value**: `autostart.rs:8`.
- **Window and tray titles**: `tray.rs:16`, `113`, `115`; `settings_window.rs:34`; `dropzones.rs:42`.
- **Sign-in script**: `claude_auth.rs:38` (title and env var `CODENOTCH_CLAUDE_CLI`).
- **User-Agents and client name**: `codex.rs:199`, `568`; `glm.rs:263`.
- **Exe-name filters**: `main.rs:1707` and `focus.rs:235` (`!contains("codenotch")`); the fork's exe name must not contain "claude".
- **Settings extras**: `settings.html` localStorage key `codenotch.settings.pane`; the author credit and `open_author_page`.
- **Strings**: all UI strings containing "Codenotch".
- **Workflows**: artifact names `Codenotch-Setup.exe`, `Codenotch-Setup.nsis.zip`, and the doctor grep "Codenotch doctor".

====================================================================
21. UPSTREAM BUGS AND GOTCHAS FOUND
====================================================================
1. `hooks_install::load` (:36-41) returns `{}` for an unparseable `settings.json`; `install` then writes a file holding only hooks. A backup is made first, but this violates the fork's "refuse when it doesn't parse". Writes are non-atomic (:55).
2. Hooks go only into `~/.claude\settings.json` (:18-20), never into other `CLAUDE_CONFIG_DIR` folders. Every hook has `"timeout":5` and PermissionRequest is not wired. The command is `"\"<abs path>\" <event>"`: fine in cmd and Git Bash, but a missing exe yields a non-zero (not 2) hook error in every session.
3. `watcher::is_session_jsonl` requires a component equal to ".claude", so secondary accounts are ignored (§7).
4. `usage::profiles` requires `.credentials.json` to see an account at all.
5. `--silent` is written to the Run key and never parsed.
6. `about-version` is hard-coded to "0.3.0".
7. The package feed step expects `.nsis.zip`, but `createUpdaterArtifacts:true` doesn't produce one. The feed URL hard-codes `vinzdg` and `v$version`.
8. `focus_session`/`dismiss_session` are unused by the UI; the "notice" event is never emitted.
9. The server accepts any local process without auth. A ping or unknown event creates a phantom "unknown" session. A bind failure is silent (e.g. the official app already holds 48666), and hooks then feed the other app.
10. Tray lines for several accounts are indistinguishable.
11. `dirs` home and config on Windows ignore env overrides.
12. `glm` reads the `ANTHROPIC_AUTH_TOKEN` from Claude settings (conditional).

====================================================================
22. WHERE FORK FEATURES ATTACH: smallest seams (proposed W-ids, tag `// Fork: W…`)
====================================================================
**Architecture**: keep `windows/codenotch` as upstream plus seams, mirroring the Mac rule. New fork crates:
- `windows/agentnotch-engine`: pure Rust port of ClaudeControl (accounts, registry, hooks installer, session pipeline, usage via `get_usage`/`.claude.json`/statusline/Desktop cache, cloud sync). No Tauri; Mac-testable.
- `windows/agentnotch-hook`: blocking hook + statusline wrapper binary.
- Glue: either a Tauri plugin crate `windows/agentnotch-bridge` (own state, setup, commands and permissions via its build script) or a bridge module in the app. The bridge holds no logic.

**Seams**:
- **W-R (rebrand)**: the files in §20.
- **W-B (builder)** `main.rs:1790-1798`: add `.plugin(agentnotch_bridge::init())`. The plugin manages its own state, starts its threads in its setup, and exposes `plugin:agentnotch|*` commands. Add "agentnotch:default" to `capabilities/default.json` plus new window labels. Without a plugin, the alternatives are a second `.manage(...)` line and appending fork command names inside `generate_handler!` (:1810-1873).
- **W-U1 (token path off)**: `main.rs:1891` (don't start `usage::start`; the bridge feeds `AppState.usage` and "usage"); `main.rs:667`, `:670` (sign-in off); `main.rs:677-680` (Claude refresh goes to the bridge); `usage.rs` `profiles()` returns empty; `doctor.rs:78`. Plus a Rust token-free verify script over `windows/`.
- **W-H (hooks)**: `main.rs:1524-1536` (get/set_hooks_installed) and `main.rs:1759-1766` (CLI install and uninstall) go to the engine installer: consent, all run folders except Parallel-Profiles stores, atomic, backed up, refuse unparseable, fail-open command wrapper, PermissionRequest timeout 86400. `main.rs:1889` `server::start` is replaced or kept on a fork port; `main.rs:1890` `watcher::start` is kept only as Desktop fallback, or dropped. The settings Claude row `settings.html:1161-1166` gets the fork's consent copy ("Turn on").
- **W-S (sessions UI)**: one line `<script src="agentnotch/notch.js"></script>` before `</body>` in `notch.html` (and similarly in `settings.html`). Top-level `function` declarations in the classic upstream script are global and resolved at call time, and later classic scripts share its global `let` bindings (`usage`, `stateSnap`, `activity`, `hoverId`). So the fork script can override:
  - `renderCard`'s Claude block (`notch.html:1205-1213`)
  - `workState` (:944-956; five states plus badges)
  - `renderRing` decoration (:1097-1103)
  - `refreshRing`/click on Claude cells (:1392-1401; open the sessions panel)
  - `reportHot` (:1288-1294; add fork rects)
  - `offersClaudeSignIn` (:1149; return false)

  Extend `check-ui-scripts.mjs` to cover the fork's JS files.
- **W-P (panel/chat window)**: a new focusable window label (e.g. "sessions") built like `settings_window.rs` (a `run_on_main_thread` hop) and positioned from `place_notch` geometry. `main.rs` exposes `screens`, `target_screen`, `notch_window_size` and `Screen`; `edge_origin` and `work_insets` are private (need `pub(crate)`, which is another seam).
- **W-C (defaults)**: `config.rs:173-175` default_weekly_ring → "outside" for fresh installs (the Mac's `Fork.prepareDefaults` analog); `config.rs:229` port.
- **W-SI (single instance / deep link)**: `main.rs:1792-1797` passes argv to the bridge (`agentnotch://auth-callback`).
- **W-UP (updater)**: `tauri.conf.json` plugins.updater; a new fork workflow (`windows-release.yml` gated to `rivantmedia/agentnotch`, driven by `VERSION`, tag `agentnotch-v…`, `createUpdaterArtifacts` "v1Compatible" or exe+sig, feed uploaded into the same release as appcast before publish). Upstream's `windows*.yml` stay gated to vinzdg.
- **W-F (focus)**: `main.rs:1024-1035` `focus_session` goes to a bridge jump (WT UIA / VS Code / fallback to `focus::focus_terminal`).

**Windows paths the fork engine needs** (other areas cover these in depth):
- `%USERPROFILE%\.claude`, `%USERPROFILE%\.claude.json`, `<dir>\.claude.json`, `<dir>\sessions\<pid>.json` (never `*.key`), `<dir>\projects\<slug>\<id>.jsonl`, `%USERPROFILE%\.claude-<name>`, `%USERPROFILE%\.claude-windows\<12hex>\`, `.manifest.json`, `.claude-shared`.
- Claude Desktop: `%APPDATA%\Claude\claude-code-sessions\<acct>\<org>\<hostSessionId>.json` and `local-agent-mode-sessions`, plus the MSIX mirror `%LOCALAPPDATA%\Packages\*Claude*\LocalCache\Roaming\Claude\…`.
- Support folder, e.g. `%APPDATA%\Agent Notch\Claude` (user-only ACL by inheritance; 0600/0700 analog via an explicit DACL if wanted), holding `cloud-session.json` etc.

**Per-process facts on Windows**:
- `CLAUDE_CONFIG_DIR` of another process: `NtQueryInformationProcess` → PEB → `RTL_USER_PROCESS_PARAMETERS.Environment` via `ReadProcessMemory` (same user; keep only that variable). This is the `KERN_PROCARGS2` analog.
- Process start time: `GetProcessTimes` creation time (pid + start-time keys).
- Children: CREATE_NO_WINDOW (0x08000000) plus a JobObject with KILL_ON_JOB_CLOSE.
- Locating claude: `usage::find_cli` list. Spawning `claude.cmd` needs Rust ≥1.77 batch-arg escaping, or resolve the native `claude.exe`.
