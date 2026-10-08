# Agent Notch for Windows: the binding design

Status: binding spec for the implementers of the Windows port and for its documentation.
Revision 2: amended after a feasibility review and a parity review; every point they raised is
settled in the "Decisions log" at the end (F1–F27, P1–P25).
Repo: `rivantmedia/agentnotch`. The port was built on branch `windows-port`. Upstream's Windows
port lives in `windows/` (Rust + Tauri 2, upstream 642d329 = Codenotch 1.18.0, unmodified in the
fork before this work).

Mapping reports (the porting spec, cited by section throughout, here and in code comments; do
not duplicate them, read them): `docs/design/windows-port-notes/`. They describe the Mac app and
upstream's Windows port as they were when the port began; where they and the code disagree, the
code is right. `<scratch>` below is the implementer's scratch folder (any folder outside the repo
and outside `~`'s dot-folders).

| Tag | File | Area |
|---|---|---|
| **WP§n** | `win-port.md` | upstream `windows/` tree: process model, commands, events, placement, token path, rebrand |
| **HS§n** | `hooks-sessions.md` | hook script, status line, installer, socket protocol, session pipeline, control, focus |
| **AU§n** | `accounts-usage.md` | paths, folders, identities, rings, usage formats, UsageStore, probe, Desktop cache |
| **CL§n** | `cloud.md` | web contract, sign-in, files, sync, ledger, scanner, outbox, summaries |
| **UI§n** | `ui.md` | tokens, notch additions, panel, chat, settings pane |
| **BT§n** | `build-tooling.md` | release pipeline, Tauri facts, CI, seams scripts, local tooling |

On the maintainer's Mac, Rust lives only in a scratch folder: `source <scratch>/rust/env.sh`
(stable 1.98.1, target `x86_64-pc-windows-msvc` added, `CARGO_TARGET_DIR` in scratch). Never
install into `~`.
All CLAUDE.md safety rules hold for every package: never launch the live Mac app, never write
the maintainer's `~/.claude*`, never read `.credentials.json`, Keychain items or `sessions/*.key`,
never run `claude`, never open `credentials.txt` or a real `.env`. Engine tests use temporary
roots only.

Markers used below: **[ASSUMPTION]** = Windows/Tauri/Claude Code behaviour not proven here,
followed by how CI (or the maintainer's read-only bundle check) proves it; **[DEGRADE]** = the
honest fallback when Windows cannot match the Mac.

---

## 0. Decisions at a glance

1. **Transport:** a per-user named pipe `\\.\pipe\agentnotch-hook-<user SID>` whose security
   descriptor names the user as owner and has a protected DACL (user + SYSTEM), with
   length-prefixed JSON frames, not upstream's loopback HTTP (§1.4). Both ends check each other
   in ways that work across integrity levels (server: identification-level impersonation;
   client: the pipe's owner and DACL).
2. **One native helper exe** `agentnotch-hook.exe` with subcommands `hook`, `statusline`,
   `type`, `console-info`. No Python on Windows. Copied into each run folder's
   `hooks\` (as the Mac copies its scripts), so an app update never fights a running hook. It
   parses its argv by hand, never panics out, never exits 2, and a watchdog ends it with exit 0
   when the pipe stalls (§1.4, §1.8).
3. **Hook commands**: a string form that parses in both Git Bash and PowerShell (an unquoted
   forward-slash path, else its 8.3 short path) by default; **exec form** (`"command": <exe>,
   "args": ["hook","--exec"]`) only when every Claude Code found on the PC, bundled copies
   included, is known and at least `EXEC_FORM_MIN`, a constant set only from committed bundle
   evidence (§4.3, §6.2 facts job). A folder where neither form is possible is shown as not
   hookable; nothing is written there.
4. **Upstream stays upstream.** `windows/codenotch` gets small tagged seams only (§2.5); all
   logic is in fork crates; the glue module `windows/codenotch/src/agentnotch/` holds no logic.
5. **Engine crate** `agentnotch-engine`: platform-independent, no Tauri, no `windows*` crates, no
   C dependencies; every platform service behind a trait (§3.2). `cargo test` runs on macOS,
   Linux and Windows.
6. **Upstream's Claude token path is dead code** (seam WU1), pinned by an extended
   `verify-token-free.sh`. Upstream's other providers (Codex, Cursor, Grok, Antigravity, GLM)
   stay exactly as upstream ships them.
7. **Data:** `%APPDATA%\Agent Notch\` holds only upstream's config and logs (nothing secret, fine
   to roam); `<support>` = `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude\` (machine-local,
   never roams, protected DACL: the user and SYSTEM only) holds every fork file, the cloud
   sign-in included. No machine binding is needed (§1.5).
8. **UI:** upstream `notch.html`/`settings.html` load fork scripts through 5 + 5 tagged seam
   lines; the sessions panel and chat are a new focusable Tauri window `agentnotch-panel`
   loading `ui/agentnotch/panel.html`. All fork commands go through one Tauri command `an_call`;
   all fork events are `an:*` (§3.7).
9. **Sign-in:** custom scheme `agentnotch://auth-callback` registered by the NSIS installer
   (from `plugins.deep-link.desktop.schemes` in the config; no deep-link plugin crate), delivered
   to the running instance by upstream's single-instance plugin (seam WSI). PKCE, one-shot
   in-memory pending gate, Cancel button, 10-minute timeout. No website change.
10. **Update signing: the proposal is adopted, amended** (§6.4): no new secret. A fork-owned,
    C-free Rust tool `agentnotch-release` derives a separate Ed25519 key from
    `SPARKLE_ED_PRIVATE_KEY` with HKDF-SHA256, a `keys` job publishes only the public key as a
    job output, the Windows build compiles it in, a separate `sign-windows` job (ubuntu, the only
    Windows job holding the seed) signs in minisign prehashed format with `version:<V>` in the
    trusted comment, verifies with `minisign-verify =0.2.5` (the updater's own crate) and
    writes `latest.json`. `requireSignedVersion: true` from the first release.
11. **Release:** everything stays in `release.yml` (OIDC website refresh unchanged). New job
    graph: `checks → plan → release-tool → {keys, mac} → windows (reusable
    agentnotch-windows.yml) → sign-windows → publish → website`; the jobs that receive the seed
    run a prebuilt, hash-checked tool and never build anything. One draft release gets all six
    assets, then is published. Assets: `AgentNotch-<V>.dmg`, `AgentNotch-<V>.zip`,
    `appcast.xml`, `AgentNotch-<V>-Setup.exe`, `AgentNotch-<V>-Setup.exe.sig`, `latest.json`.
12. **CI proves behaviour on `windows-2025`:** pipe ⇄ real hook exe (incl. a stalled server,
    medium ⇄ high integrity in both directions, and a squatter running as a second local user),
    PEB env read of a child (incl. a 100 KiB environment), two-phase `WriteConsoleInput` into a
    child console and a ConPTY, settings.json install/uninstall with fault injection, hook and
    status-line commands run through real Git Bash and PowerShell, Job-object kill trees, and an
    installer smoke test that drives the real UI over WebView2 remote debugging: install, sealed
    self-test with machine-checked layout invariants and snapshots, real launch that writes
    nothing before consent, "Turn on" → hooks in a temp profile → each entry run exactly as
    written → answers from the panel → "Turn off" restores the bytes, a probe against a fake
    `claude`, sign-in against a local fake website through a real `agentnotch://` deep link,
    update and reinstall paths, uninstall.
13. **Scope and ship gates.** Every Mac feature is ported (the maintainer's decision). The ones CI
    can prove only in part ship on conservative defaults, each with the honest UI the Mac already
    has: typing replies into a terminal is **opt-in** (off by default), the panel never opens by
    itself unless the user chooses an auto-open policy (default Never), Windows Terminal tab
    selection is best effort, and the status line is taken over only when its command is safe to
    re-run (§4.3). Windows is labelled **preview** in 1.1.0's release notes and on `/download`.
14. **The uninstaller keeps hooks** unless the user ticks "Delete the application data" (or passes
    `/REMOVEHOOKS`): the "Uninstall before installing" step of a manual upgrade must not strip
    every settings.json (§6.5). Entries left behind exit 0 at once while the app is gone.

---

## 1. Architecture overview

### 1.1 Processes

| Process | Image | Started by | Lifetime | Role |
|---|---|---|---|---|
| App | `%LOCALAPPDATA%\Agent Notch\agentnotch.exe` (GUI subsystem) | user, Start menu, autostart Run value `Agent Notch`, deep link, toast click | long | upstream notch + tray + other providers; the fork hub (engine) inside; windows `notch`, `settings`, `dropzones` (upstream), `agentnotch-panel` (fork) |
| App CLI | same exe with a subcommand (`doctor`, `inspect-accounts`, `install-hooks`, `uninstall-hooks`, `control quit\|status`, `autostart on\|off`) | user, installer, CI | short | handled before Tauri starts (seam WCLI); attaches to the parent console to print |
| Hook | `<configDir>\hooks\agentnotch-hook.exe hook [--exec]` (console subsystem, static CRT) | Claude Code, one per hook event | ms; up to 24 h for PermissionRequest | reads stdin JSON, sends one frame to the pipe (a watchdog ends it with exit 0 after 1.2 s of connect + write), for PermissionRequest then waits for the decision and prints `hookSpecificOutput` |
| Status line | `<configDir>\hooks\agentnotch-hook.exe statusline` | Claude Code (the account's `statusLine.command`) | ≤ 30 s | runs the previous status line command (chained, Git Bash only, §4.3) and forwards the status JSON (fire and forget on its own thread, 0.3 s budget) |
| Console helper | `<install dir>\agentnotch-hook.exe type\|console-info --pid N` | the app's `an-ui` worker (CREATE_NO_WINDOW) | ≤ 12 s | `FreeConsole` + `AttachConsole(pid)`; types a reply with `WriteConsoleInputW` in two phases (text, then Return only after the engine re-checks, §4.8), or reports the console window, title, attached processes and input mode. The GUI app itself never attaches to a console. |
| Probe / summary child | `claude.exe` (or `node.exe …\cli.js`) | the engine, via `CommandRunner` | ≤ 20 s / ≤ 90 s | `get_usage` probe; `-p --model haiku` summary. In a Job object with KILL_ON_JOB_CLOSE, CREATE_NO_WINDOW. Never in tests: tests use fake executables. |
| Installer / uninstaller | NSIS (Tauri template + fork hooks) | user, updater (`/UPDATE /P /R`) | short | per-user install to `%LOCALAPPDATA%\Agent Notch\`; registers `agentnotch:`; Start-menu shortcut with AUMID `com.rivantmedia.agentnotch`; the uninstaller asks the app to quit, and removes this app's hooks only when "Delete the application data" is ticked or `/REMOVEHOOKS` is given (§6.5) |

### 1.2 Threads and runtime inside the app

Upstream's threads are unchanged (WP§2) except three that the fork stops by seams:
`usage::start` (token path, WU1), `server::start` (TCP 48666, WH) and `watcher::start` (upstream's
transcript watcher, WH). Upstream's `state::Store`, ack scan and sweeper keep running on an empty
store; harmless.

The fork adds, all started from `crate::agentnotch::setup()` (seam WB2):

| Thread | Owner | Does |
|---|---|---|
| `an-core` | engine `hub::runtime` | owns `Core` (accounts, sessions, usage store, hook manager, attention tracker, review store, settings). One consumer of an unbounded `crossbeam_channel<Input>`; `recv_timeout(next_deadline)`; strictly ordered like the Mac's `HookEventPipeline` (HS§5.1). Never blocks on IO: it hands jobs to workers. **The only writer of `control-settings.json`**: other threads send `Input::SetSetting`. |
| `an-io-{0..2}` | engine `hub::runtime` | file jobs: transcript reads, registry scans, settings.json writes, `.claude.json` reads, Desktop cache reads, persistence. Results come back as `Input::JobDone`. |
| `an-probe` | engine `hub::runtime` | one child at a time: the usage probe and `claude --version` checks (a slow `claude` never starves file jobs). |
| `an-ui` | engine `hub::runtime` | OS-UI jobs that can hang on another process: console helper runs (typing, `console-info`), host classification, focus plans, UI Automation (connection and transaction timeouts 2 s each), visibility scans. A hung Windows Terminal only ever delays other UI jobs. |
| `an-cloud` | engine `cloud::CloudService` | its own 20 s tick, HTTP through `Http`, summaries through `CommandRunner`; receives live observations and usage observations over a channel from `an-core`; reads what else it needs through `CloudDeps` (§3.4); publishes `CloudState`; sends switch changes to `an-core` as `Input::SetSetting`. |
| `an-pipe` | `agentnotch-win::pipe_server` | a Tokio current-thread runtime running the named-pipe server; pushes `TransportEvent`s into the core channel; holds PermissionRequest connections. |
| `an-hotkey` | `agentnotch-win::hotkey` | message-only window + `RegisterHotKey` loop for the panel shortcut (off by default). |
| `an-foreground` | `agentnotch-win::focus` | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)`; pushes `Input::Foreground` (activation stays ≥ 1.5 s → mark viewed, HS§9.3). |
| emitter | glue | the engine calls the glue's event sink from `an-core`; the glue calls `AppHandle::emit`/`emit_to` (thread-safe) and hops to the main thread only for window work (`run_on_main_thread`, from a spawned thread, the `settings_window::open` pattern, WP§12). |

Tauri command `an_call` is `async`; it runs `hub.call()` inside
`tauri::async_runtime::spawn_blocking`, so a slow call (focus, typing, probe) never blocks the
WebView2 main thread.

### 1.3 Data flow

```
 Claude Code ──hook stdin──▶ agentnotch-hook.exe ──frame──▶ \\.\pipe\agentnotch-hook-<SID>
      ▲  (PermissionRequest: waits)                              │ an-pipe (agentnotch-win)
      └── hookSpecificOutput ◀── frame (decision) ◀──────────────┤ TransportEvent
                                                                  ▼
 <cfg>\sessions\*.json, projects\*.jsonl, .claude.json ──scan/read (workers)──▶ an-core: Core
 claude -p get_usage (Job object) ────────────────────────────────────────────▶   ├ accounts (registry, identities, rings)
 Claude Desktop Cache_Data (read-only) ───────────────────────────────────────▶   ├ usage store (merge, dated readings)
                                                                                  ├ sessions store + review store
                                                                                  ├ attention tracker → reactions, toasts
                                                                                  ├ hook manager (consent-gated writes)
                                                                                  └ projections → HubEvent
                                                                                        │
            ┌──────────────── glue (crate::agentnotch) ◀────────────────────────────────┘
            │  AppState.usage + "usage" (upstream shape: tray, upstream notch code)
            │  an:snapshot → notch + panel     an:settings/an:cloud → settings
            │  an:chat/an:panel → panel        an:peek → notch        tray badge, toasts, sounds
            ▼
 notch.html + ui/agentnotch/notch.js     panel.html (+chat)     settings.html + ui/agentnotch/settings.js
            └───────────── invoke('an_call', {method, args}) ──────────▶ glue ──▶ hub.call()
 an-cloud ◀── live sessions, usage observations ── an-core ;  an-cloud ──HTTPS──▶ website / Supabase
```

### 1.4 IPC protocol (hook ⇄ app)

**Transport choice: named pipe.** Reasons, each decisive on its own:
- PermissionRequest answers approve arbitrary tools. Loopback TCP (upstream: 127.0.0.1:48666,
  WP§5) can be bound first by any local user, reached by browsers (upstream needed an Origin
  guard), and its peer's user cannot be checked cheaply (HS§4 WIN).
- A pipe carries an owner and a DACL, `PIPE_REJECT_REMOTE_CLIENTS`, and lets the server read the
  client's token by identification-level impersonation, so both ends can verify the other runs
  as the same user, whatever either end's integrity level: the analog of the Mac's `st_uid`
  check on the socket.
- No port: no collision with an installed official Codenotch (48666), no firewall prompt.
- Both ends compute the name from the user's SID: nothing is templated into hook commands.

**Name.** `\\.\pipe\agentnotch-hook-<SID>` where `<SID>` is the string SID of the process token's
user (`S-1-5-21-…`). `agentnotch_proto::pipe_name(sid)`. Dev override (tests, the simulator):
`AGENTNOTCH_SOCKET=\\.\pipe\<name>` honoured by the hook exe **only** together with
`AGENTNOTCH_DEV=1`, and by the app always (same rule as the Mac, HS§1.1). A sealed run starts no
server.

**Server (agentnotch-win `pipe_server`).**
- `CreateNamedPipeW` via Tokio `ServerOptions` (`create_with_security_attributes_raw`): byte
  mode, `first_pipe_instance(true)` for the first instance, `reject_remote_clients(true)`, in/out
  buffer 64 KiB, **`max_instances` left unset** (Tokio asserts < 255; the app counts instead),
  security descriptor from SDDL `O:<SID>D:P(A;;GA;;;<SID>)(A;;GA;;;SY)`: the owner is set to the
  user SID explicitly (an elevated app's default owner would be Administrators, which the
  client's check would refuse), the DACL is protected (no inherited ACEs; no Everyone, no Users).
- First instance fails with ACCESS_DENIED → another process owns the name (a second copy of the
  app, or a squatter) → `TransportEvent::Error("The hook pipe is in use by another program")`;
  the app runs without hook events (panel banner (d), UI§5.2) and retries every 5 s.
- One idle instance is always listening; a connected instance is replaced before it is served.
  The app counts open connections; above 512 a new one is closed at once (the hook fails open).
- Read one frame within 5 s of connect (else close). Frames above 8 MiB: close.
- Peer check, right after the first frame is read (impersonation needs a read first):
  `ImpersonateNamedPipeClient` → `OpenThreadToken(TOKEN_QUERY, OpenAsSelf = TRUE)` → `TokenUser`
  SID must equal ours → `RevertToSelf` on every path (a failed revert aborts the process). All of
  it runs synchronously between two awaits on the pipe thread, never across an await.
  Identification-level impersonation works whatever the two integrity levels are (a hook from an
  elevated terminal is accepted by a normal app). Any failure, or SYSTEM, → close without
  reading more. `OpenProcessToken` on the client is never used (it fails for an elevated client).
- Held connections (PermissionRequest): one Tokio task per connection with a pending read to
  detect the peer going away (`ERROR_BROKEN_PIPE`) → `TransportEvent::PeerClosed(conn_id)`, and a
  oneshot for the answer: write the response frame with a 2 s timeout, then close.
- The server never calls `DisconnectNamedPipe` before the client has read (it discards unread
  data); it closes the handle after writing. **[ASSUMPTION]** data written before the server
  closes is still readable by the client, and data a fire-and-forget client wrote before closing
  is still readable by the server. Proven by `agentnotch-hook/tests/pipe_roundtrip.rs` on
  windows-2025 (1 MiB message, immediate client close; response then immediate server close).

**Client (the hook exe, and every other client: the CLI's `control`, the doctor).**
- `CreateFileW(name, GENERIC_READ|GENERIC_WRITE, 0, NULL, OPEN_EXISTING,
  SECURITY_SQOS_PRESENT|SECURITY_IDENTIFICATION, NULL)`: the server may identify the client but
  never act as it (every client open sets these flags; a test pins it).
  `ERROR_FILE_NOT_FOUND` → the app is not running → exit 0 immediately, no output.
  `ERROR_PIPE_BUSY` → `WaitNamedPipeW` for what is left of the budget, retry once, else exit 0.
- Server check: `GetSecurityInfo(pipe, SE_KERNEL_OBJECT, OWNER|DACL)`: the owner must be the
  hook's own user SID, the DACL must be protected and hold only allow ACEs for that SID and
  SYSTEM; else exit 0 without writing. This reads the pipe object, not the server process, so it
  works whatever the two integrity levels are (`OpenProcessToken` on an elevated app would fail).
  A squatter running as another user cannot make our SID the owner.
- **Watchdog.** Once stdin is read, a thread armed with a deadline calls `ExitProcess(0)` when it
  expires: 1.2 s for connect + write (every hook event). A PermissionRequest disarms it once the
  frame is fully written and then waits for the answer with no deadline of its own (Claude Code's
  86 400 s timeout ends it). A synchronous `WriteFile` of a 1 MiB frame into a 64 KiB pipe blocks
  while the server does not read; the watchdog is what bounds it. Stdout is still empty when it
  fires.
- The hook **never launches the app** (upstream's does, WP§5; the fork removes that).

**Framing.** Every frame is `u32 little-endian byte length` + that many bytes of UTF-8 JSON
(one object). Limits (`agentnotch_proto::limits`): client message ≤ 1 MiB after the hook's own
truncation (HS§1.5); server accepts ≤ 8 MiB; response ≤ 4 MiB. "No frame, then EOF" in either
direction means "no decision".

**Versions.** Hook copies in run folders can be older (or, after a downgrade, newer) than the
app. The server decodes any `protocol` ≥ 1 leniently (unknown fields ignored, missing optional
fields defaulted) and never refuses a frame for its version; the hook ignores unknown fields in a
response. A new field is always optional; a change that cannot be optional gets a new `event`
name instead. `ingress_protocol_compat.rs` pins that the committed v1 frames of
`agentnotch-proto/tests/fixtures/v1/*.json` are accepted by every later server.

**Messages hook → app.** Three kinds, told apart by `event`:

1. *Hook event* — exactly the Mac hook script's message (HS§1.4, field for field, same
   truncation limits, same coarse `status`), plus three Windows fields:
   ```json
   {"protocol":1,
    "event":"PreToolUse","session_id":"…","cwd":"C:\\Users\\me\\code\\app",
    "transcript_path":"C:\\Users\\me\\.claude\\projects\\C--Users-me-code-app\\<sid>.jsonl",
    "pid":12345,"status":"running_tool","config_dir_env":null,"attended":null,
    "entrypoint":"cli","agent_id":null,"agent_type":null,"permission_mode":"default",
    "tool":"Bash","tool_input":{"command":"npm test"},"tool_use_id":"toolu_…",
    "hook_pid":6789,
    "terminal":{"wt_session":"<WT_SESSION or null>","term_program":"<TERM_PROGRAM or null>"}}
   ```
   `pid` = `CLAUDE_PID` if it parses to 1..=2³¹−1 (set for hooks since Claude Code 2.1.214).
   Otherwise, only when argv carries the exec-form marker `--exec`, the hook's parent pid
   (`NtQueryInformationProcess(ProcessBasicInformation).InheritedFromUniqueProcessId`, kept only
   if the parent's creation time is ≤ the hook's): in exec form the parent is Claude Code itself.
   In string form the parent is `bash.exe` or `powershell.exe`, which exits right after the hook,
   so the hook walks up instead: skip ancestors whose image is `bash.exe`, `sh.exe`, `dash.exe`,
   `cmd.exe`, `pwsh.exe` or `powershell.exe` (each link validated by creation time, at most 4
   hops) and take the first other ancestor only if its image is `claude.exe`, `node.exe` or
   `bun.exe`; else `"pid": null` (the engine keys the session by id and never runs the 3 s
   liveness check on it). `terminal` forwards only those two variables (never the environment).
   JSON is written with serde_json (compact); key order is irrelevant.
2. *Status line* — exactly the Mac wrapper's message (HS§2, AU§11.1) with `"protocol":1`:
   `{"protocol":1,"event":"StatusLine","session_id","transcript_path","cwd","config_dir_env","pid",
   "status_line":{"rate_limits","context_window":{"used_percentage","context_window_size"},
   "model","cost":{"total_cost_usd"},"session_name","version"}}`. No ppid fallback for `pid`.
3. *Control* — from the app's own CLI (doctor, uninstaller, smoke test):
   `{"protocol":1,"event":"AgentNotchControl","op":"status"|"quit"}`. Response frame:
   `{"ok":true,"status":{"version":"1.1.0","sealed":false,"elevated":false,"accounts":2,"rings":2,
   "readings":2,"sessions":3,"held":0,"hook_consent":"granted"|"declined"|"unasked",
   "transport":"listening","cloud":"signed_out"|"signing_in"|"signed_in","sync":false}}` for
   `status`; `{"ok":true}` then a graceful app exit (hub stop first) for `quit`. Counts and
   states only; never paths or text.

**Response app → hook** (PermissionRequest only): the Mac's `PermissionResponse` (HS§1.7, §4.4):
`{"decision":"allow"|"deny"|"ask","reason"?:string,"updated_input"?:object,
"updated_permissions"?:array,"interrupt"?:bool}`, nulls omitted. The hook turns it into stdout
exactly as the Mac does (HS§1.7; `agentnotch_proto::permission_output`), UTF-8, no BOM, no CRLF
translation, flushed, exit 0.

**Blocking decisions: PermissionRequest (every tool, including AskUserQuestion and ExitPlanMode).**

| Step | Rule |
|---|---|
| Installer | `PermissionRequest` group `{"matcher":"*","hooks":[{…,"timeout":86400}]}` (HS§3.5). |
| Hook | after writing the frame it reads the response with a 86 400 s ceiling. EOF, error, parse failure, `"ask"` → print nothing, exit 0: Claude Code's own terminal dialog (shown in parallel) decides. |
| Server | holds the connection and the original event; finds the `tool_use_id` as the Mac does (exact cache match, single in-flight match, else synthetic `permission-<uuid>`; HS§4.3). Untracked folder/identity → release at once. |
| Answer | Allow / Always allow (first suggestion verbatim) / Deny (default message "Denied by user via Agent Notch") / AskUserQuestion answers (`updated_input` `{"answers":{question:label}}`, merged by the hook into the original input) / ExitPlanMode approve (`updated_input {}` = echo) / Keep planning (deny with the Mac's reason). Exact stdout per answer: HS§1.7. |
| Release without answer | Stop, StopFailure, SessionStart, main UserPromptSubmit, registry idle, interrupt, PostToolUse/PostToolUseFailure/PermissionDenied for that id (answered in the terminal), untracked, app quit, hub stop, sealed. Always "close, no frame". |
| Peer gone | `PeerClosed` → `permissionFailed(session, tool_use_id)` (HS§4.4 liveness). No 2 s polling needed. |
| UI gate | AnswerGate: armed after 0.35 s on screen, one answer per id, answered ids remembered 600 s (UI§5.8); the engine drops an answer whose id is no longer pending. |
| App absent / crashing | Hook exits 0 with no output within 50 ms (not running) or as soon as the pipe breaks. Never exit 2. |

**Fire and forget: every other hook event.** Connect (incl. one busy wait), write, close, all
inside the watchdog's 1.2 s; typical < 20 ms. A server that accepts but never reads costs Claude
Code at most 1.2 s per event (tested with a 1 MiB frame). **Status line:** the send runs on its
own thread with a 0.3 s budget including connect; the wrapper's main thread waits for it at most
that long and never waits on it again (the relay of the previous command's output is not
bounded by the send; the process exit ends a stuck send thread).

**Server dispatch** (engine `ingress`, ported from HS§4.2–4.4): decode leniently (session_id and
event required; loose types coerced; pid 1..=Int32.max); drop ignored sessions (`attended ==
false`, entrypoint starting `sdk`); maintain the ToolUseIdCache (60 min); deliver to the core in
arrival order; `received_at` = server read time.

### 1.5 Persistence

The engine never calls `dirs` or a Known Folder API itself: the glue resolves `Roots` (§3.2)
once and tests inject them. Each root is resolved **the way its owner resolves it**, so both
sides always agree: `home` from `%USERPROFILE%` (what Claude Code's `os.homedir()` reads; the
Known Folder only when the variable is empty), `data` = the folder of upstream's `config.json`
(`dirs::config_dir()` = Known Folder RoamingAppData, WP§11; `Agent Notch Sealed` instead of
`Agent Notch` when sealed, §4.13), Claude Desktop's userData from the Known Folder (what Electron
reads), and `support` from `AGENTNOTCH_SUPPORT_DIR`, else Known Folder LocalAppData +
`com.rivantmedia.agentnotch\Claude` (Tauri's app-local-data folder, where WebView2's data already
lives).

| What | Where | Notes |
|---|---|---|
| `<home>` | `%USERPROFILE%` (else `FOLDERID_Profile`) | what Claude Code's `os.homedir()` uses. Never `$HOME` (Git Bash sets `/c/Users/…`). |
| `<data>` | `%APPDATA%\Agent Notch\` (seam WR-DIR in `config.rs`) | upstream's `config.json`, `run.log` (fork lines prefixed `an:`), `<cmd>.log` of CLI runs (`doctor.log`, `control.log`, …), `install.log`, upstream provider caches, `glyphs\`, `quota-work\`. Upstream's `watch.log` is never written (watcher off). Nothing secret and nothing of the fork's state: it may roam. |
| `<support>` | `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude\`, override `AGENTNOTCH_SUPPORT_DIR` | machine-local (LocalAppData never roams). Created with a **protected DACL** SDDL `D:P(A;OICI;FA;;;<SID>)(A;OICI;FA;;;SY)`; files inherit it; temp files are created with the same security attributes (`CREATE_NEW`) so nothing is ever readable by others, even briefly (CL§4). Never read or written when sealed. |
| engine settings | `<support>\control-settings.json` v1 | every `claudeControl.*` switch of the Mac (§4.12) plus `cloudDeviceId`. Written only by `an-core`. |
| accounts | `<support>\accounts.json` v2 | Mac format (AU§3.7), through the `persist::accounts` DTOs (§3, serde rules). |
| review queue | `<support>\review-state.json` v2 | Mac format (HS§5.13), through `persist::review`. |
| usage | `<support>\usage-state.json` v1 | Mac format (AU§9.10), through `persist::usage`; status-line keys `pid:<pid>@<epoch s of GetProcessTimes creation>`. |
| hook install record | `<support>\hook-install.json` v1 | per physical settings.json: the folder, the command form written, the hook copy path. Lets `uninstall-hooks` clean up without re-discovery. |
| cloud | `<support>\cloud-session.json`, `cloud-install-secret` (32 raw bytes), `cloud-ledger.json`, `cloud-scan-state.json`, `cloud-usage-outbox.json`, `cloud-summaries.json`, `cloud-sync-state.json`, `cloud-folder-logins.json` | Mac formats (CL§4.2). Nothing read or written when sealed or before bootstrap. |
| working folders | `<support>\usage-probe\`, `<support>\session-summary\` | cwd of the probe and of summaries. |
| hook copies | `<cfg>\hooks\agentnotch-hook.exe`, `<cfg>\hooks\agentnotch-statusline.previous.json` | transient `agentnotch-hook.new-<ts>.exe` while a copy is staged; leftovers `agentnotch-hook.old-<ts>.exe` (a running copy renamed aside); both swept by later passes and by `uninstall-hooks`. |
| settings backups | `<cfg>\settings.json.agentnotch-yyyyMMdd-HHmmss-SSS.bak` (newest 5), `<cfg>\settings.json.agentnotch.original.bak` (forever) | owner-only DACL (HS§3.4). |
| WebView2 | `%LOCALAPPDATA%\com.rivantmedia.agentnotch\EBWebView` | Tauri default from the identifier; `<support>` sits beside it. |
| install | `%LOCALAPPDATA%\Agent Notch\` (`agentnotch.exe`, `agentnotch-hook.exe`, `uninstall.exe`) | Tauri NSIS currentUser default (BT§3). Not a data location (user-changeable, replaced by updates). |
| registry | `HKCU\…\Uninstall\Agent Notch`; `HKCU\Software\Classes\agentnotch`; `HKCU\Software\Rivant Media\Agent Notch` (install location, installer language); `HKCU\…\Run` value `Agent Notch` (only when the user turns autostart on) | every uninstall removes the uninstall key and (when it still points at this install) the scheme; every non-update uninstall removes the Run value (template 864-866); `HKCU\Software\Rivant Media\Agent Notch` is removed only with "Delete the application data" (template 870-879). |

Why `<support>` is in `%LOCALAPPDATA%`: everything in it is machine-local by nature: the website
refresh token, the install secret and the device id must never follow a roaming profile to
another PC (two PCs would share a device id; project keys would change), and the ledger, scanner
state and folder logins hold this PC's paths and times. The cloud report (CL§4) and HS§5.13
recommend the same. `%LOCALAPPDATA%\Agent Notch` cannot be used (the install directory), so the
folder is Tauri's own app-local-data folder, which the installer template already deletes with
"Delete the application data" (template line 883). No machine binding is needed.

Uninstall with "Delete the application data" ticked (not in update mode): the template removes
`%APPDATA%\com.rivantmedia.agentnotch` and `%LOCALAPPDATA%\com.rivantmedia.agentnotch`
(`<support>` + WebView2), the fork NSIS hook removes `%APPDATA%\Agent Notch` and this app's
hooks from every run folder (§6.5). A silent uninstall keeps all of it unless `/REMOVEHOOKS`
asks for the hooks.

### 1.6 Identifiers and names

| Thing | Value | Upstream (official) value it must differ from |
|---|---|---|
| Tauri `identifier` (single-instance mutex, WebView2 dir, AUMID) | `com.rivantmedia.agentnotch` | `com.immidi.codenotch` |
| `productName` (install dir, uninstall key, Start menu, Run value) | `Agent Notch` | `Codenotch` |
| `mainBinaryName` | `agentnotch` → `agentnotch.exe` (no space; contains neither `claude` nor `codenotch`, so upstream's Claude-desktop exe filters, WP§20, never match it) | `codenotch` |
| hook / helper exe | `agentnotch-hook.exe` (does not contain `codenotch-hook`, `eatbean-hook` or `pacman-hook`, which upstream's `is_ours` deletes, WP§20) | `codenotch-hook.exe` |
| IPC | `\\.\pipe\agentnotch-hook-<SID>`; no TCP port | `127.0.0.1:48666` |
| data folders | `%APPDATA%\Agent Notch` (upstream config; `Agent Notch Sealed` when sealed), `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude` (fork state) | `%APPDATA%\codenotch`, `%LOCALAPPDATA%\com.immidi.codenotch` |
| autostart | HKCU Run value `Agent Notch` = `"<exe>" --silent` | `Codenotch` |
| deep-link scheme | `agentnotch` (fixed by `web/contract`: `agentnotch://auth-callback`) | none |
| AUMID (toasts) | `com.rivantmedia.agentnotch` (set on the Start-menu shortcut by the Tauri template) | `com.immidi.codenotch` |
| publisher | `Rivant Media` (`bundle.publisher`) | derived |
| window titles | `Agent Notch Settings`, `Agent Notch drop zones`, panel `Agent Notch sessions` | `Codenotch …` |
| settings localStorage key | upstream's `codenotch.settings.pane` kept (separate WebView2 profile, no collision) | — |
| Cargo package names | upstream's `codenotch`, `codenotch-hook` untouched; fork crates `agentnotch-*` | — |

### 1.7 Coexistence with an installed official Codenotch for Windows

Both apps can be installed and run at once:
- Separate identifier ⇒ separate single-instance mutex, WebView2 profile, AUMID; separate install
  dir, uninstall key, Start-menu entry, Run value, data folder.
- The fork binds no TCP port and never reads `%APPDATA%\codenotch\config.json`.
- Hooks: the official app's entries (`"<…>\codenotch-hook.exe" <event>`, fire and forget,
  `timeout 5`) and ours never match each other's recognisers. The fork's settings.json writes
  splice bytes (other keys and other tools' entries keep their bytes, HS§3.4); upstream's writer
  re-serialises the whole file but merges, so our entries survive it. Both run in parallel; only
  ours answers PermissionRequest.
- The fork offers "Remove Codenotch's hooks" per folder in Settings (explicit, backed up); it
  never removes them on its own and never on "Turn on".
- The fork's CI and scripts never `taskkill /IM codenotch.exe` (FORBID rule), never touch
  `com.immidi.codenotch` paths or registry keys.
- Both draw a notch and a tray icon with upstream's artwork (the fork keeps upstream's icons, as
  the Mac fork does); the user moves one notch to another edge. A fork icon is an open question
  (§9).

### 1.8 Invariants every package keeps

- **Token-free.** No fork code, and no reachable upstream code, reads `.credentials.json`,
  `claudeAiOauth`, a Credential Manager item for Claude, `sessions\*.key`, or calls
  `api/oauth/usage`, `auth login`, `setup-token`, or refreshes a token.
- **Consent before writes.** Nothing is written to any `settings.json` or `hooks\` folder until
  "Turn on" (engine `hook_consent == Some(true)`); `install-hooks` from the CLI refuses without it
  (exit 2). Removal (`hooks_enabled(false)`, `uninstall-hooks`, the uninstaller) is always allowed.
- **Consent before uploads** exactly as CLAUDE.md "Cloud sync (app side)".
- **Fail open.** A hook command never exits 2; every error path is exit 0 with no output. In
  `agentnotch-hook`: no argument-parsing crate (clap-style parsers exit 2 on a usage error); argv
  is matched by hand and an unknown subcommand or extra argument is exit 0 with empty stdout;
  `main` runs inside `catch_unwind` and ends with `std::process::exit(0)` (a panic would exit
  101); output goes through `write_all` on the locked stdout with errors ignored (`println!`
  panics on a closed stdout); the watchdog of §1.4 bounds every wait except the PermissionRequest
  answer.
- **Settings.json is never lost.** At every instant each settings.json the app writes exists
  with either its old bytes or its new bytes (§4.3); a pass that saw a file never writes one
  planned from `{}`.
- **Never type into a real terminal in tests; never run `claude` in tests.** Windows CI uses test
  reader processes and fake executables built from the workspace; the smoke test first proves no
  real `claude` is reachable and pins the probe to `fake-claude.exe` (§7.5).
- **Fork UI copy is English-only**; upstream copy is rebranded at runtime (§5.5).

---

## 2. Crate and folder layout, ownership, seams

### 2.1 Tree (new paths are fork-owned; WPn = owning work package, §8)

```
windows/
  Cargo.toml                         upstream + seam WWS (members)                      WP0
  Cargo.lock                         regenerated (ALLOW)                                WP0, then any WP adding a dep
  agentnotch-proto/                  wire protocol, pure, used by hook exe and engine  WP0 (complete in WP0), WP1 maintains
  agentnotch-engine/                 all Claude logic; no Tauri/windows/C              per-directory owners below
    src/lib.rs, platform.rs, runtime_types.rs (§3.4), hub/api.rs                      WP0
    src/core/{roots,flags,sealed,time}.rs                                              WP0
    src/core/{paths,json_scan,claude_json,atomic}.rs  COMPLETE and tested in WP0      WP0, then WP3 (paths, json_scan, claude_json), WP2 (atomic)
    src/model/ids.rs, model/hook.rs (HookEvent, StatusLineMessage)                    WP0, then WP1
    src/model/accounts.rs    src/persist/accounts.rs                                  WP0, then WP3
    src/model/usage.rs       src/persist/usage.rs                                     WP0, then WP4
    src/model/{sessions,requests}.rs   src/persist/review.rs                          WP0, then WP5
    src/model/cloud.rs                                                                 WP0, then WP8
    src/model/ui.rs (snapshot types, §3.6)                                             WP0, then WP7
    src/testkit/{mod,clock,files}.rs                                                   WP0
    src/hub/sealed_fixture.rs (fixture-backed Hub::sealed from tests/ui-contract)     WP0 (replaced by WP7's sealed demo)
    src/ingress/*            testkit/transport.rs                                     WP1
    src/hooks/*              core/settings_doc.rs  hooks/facts.rs                     WP2
    src/accounts/*           testkit/process.rs                                       WP3
    src/usage/*              testkit/runner.rs                                        WP4
    src/sessions/*  src/review/*  src/attention/{tracker,news}.rs                     WP5
    src/control/*   src/attention/policy.rs   testkit/terminal.rs                     WP6
    src/hub/* (except api.rs)  src/geometry/*  src/attention/{rows,sections}.rs  core/{settings,rebrand}.rs   WP7
    src/cloud/*              testkit/http.rs                                          WP8
    tests/ui-contract/*.json (Rust⇄JS contract fixtures)                              WP0 creates, WP7 maintains
    tests/fixtures/claude-code-facts.json (committed bundle evidence, §6.2)           WP11 (read by WP2's facts.rs test)
    tests/*.rs               each WP its own files, named <area>_*.rs
  agentnotch-win/                    Windows implementations of engine traits
    src/lib.rs, src/stub.rs (non-Windows)                                             WP0
    src/pipe_server.rs, src/console.rs, src/sid.rs, src/integrity.rs (token levels, test spawns)   WP1
    src/files.rs (DACL, atomic replace, file id, reparse)                              WP2
    src/process.rs (toolhelp, times, liveness, PEB env), src/paths.rs (known folders)  WP3
    src/job.rs (CommandRunner), src/bin/fake-claude.rs (test-only bin)                 WP4
    src/focus.rs, src/uia.rs, src/toast.rs, src/sound.rs, src/visibility.rs            WP6
    src/http.rs (ureq/schannel), src/browser.rs, src/device.rs (name, SID, elevation, Smart App Control)   WP8
    src/window.rs (HWND styles, foreground save/restore/confirm), src/hotkey.rs, src/clipboard.rs, src/capture.rs, src/shell.rs (SHOpenFolderAndSelectItems)   WP9
    tests/win_*.rs (cfg(windows) integration tests; owners as above)
  agentnotch-hook/                   bin agentnotch-hook.exe                            WP1
  agentnotch-release/                bin agentnotch-release (key derivation, minisign, feed)   WP11
  agentnotch-ui-tests/               node --test suites for ui/agentnotch/*; baselines/*.png   WP10
  scripts/agentnotch-build.ps1       builds hooks + installer (CI and local)           WP11
  scripts/agentnotch-smoke.ps1       installer smoke test                               WP11
  scripts/smoke/{cdp.mjs,fake-website.mjs,run-hook.mjs,contract-shape.mjs}   smoke helpers (Node 22, no deps)   WP11
  scripts/check-claude-code-facts.mjs  fetches Claude Code packages from npm and greps them (never runs them)   WP11
  tools/package.json, package-lock.json   pinned @tauri-apps/cli 2.11.4 (npm ci)        WP11
  codenotch/                         upstream crate + seams (§2.5)                      seams: WP0 only
    capabilities/agentnotch.json     capability for the panel window                   WP0
    nsis/agentnotch-hooks.nsh        NSIS installer hooks                              WP0 stub, WP11
    src/agentnotch/                  the glue module (holds no logic)                   WP9 (WP0 stub)
      mod.rs setup.rs calls.rs emit.rs panel.rs deeplink.rs cli.rs tray.rs selftest.rs website.rs
      update.rs menu.rs hotkey.rs
    ui/agentnotch/                   fork UI (English only)                             WP10 (WP0 stubs)
      theme.css common.js rebrand.js notch.js notch.css panel.html panel.js panel.css chat.js
      markdown.js toolresults.js settings.js settings.css
.github/workflows/agentnotch-windows.yml                                                WP11 (WP0 skeleton)
.github/workflows/fork.yml, release.yml                                                 WP11 (WP0: fork.yml rust job skeleton)
Scripts/check-seams.sh, Scripts/fork-seams.txt, Scripts/verify-token-free.sh           WP0 (final in WP0)
Scripts/cloud-contract-e2e.sh                                                           WP8
Packages/ClaudeControl/Tests/ClaudeControlTests/Fixtures/model-pricing-vectors.json     WP8 (read by the Swift ModelPricingTests and the Rust cloud_pricing.rs)
.github/workflows/claude-code-facts.yml                                                 WP11
web/src/app/download/page.tsx, web/tests/unit/releases.test.ts                          WP11
README.md (Windows sections), CLAUDE.md (Windows sections)                              WP11
```

Ownership rules: a file has exactly one owner at a time. WP0 creates stubs for files other
packages own; ownership moves when WP0 lands. The model is split into one file per owning
package (`model/{hook,accounts,usage,sessions,requests,cloud,ui}.rs`): WP0 writes every type of
§3.3–§3.6 in full, then each file passes to its package, which may add fields (never rename or
remove one without the lead). A package that needs a field in another package's model file asks
that package's owner. `runtime_types.rs` (§3.4) and `hub/api.rs` stay with the lead until WP7
takes `hub/api.rs`. **Only WP0 edits upstream files** (the seams). A
package that finds it needs a new seam asks the lead (WP0 owner), who adds it with its
`fork-seams.txt` line. `Cargo.lock`: never regenerate it (`cargo generate-lockfile` or a bare
`cargo update` would move upstream's pins); a plain `cargo build` adds only what is new, and
`cargo update -p <crate> --precise <ver>` pins one crate. Two packages adding dependencies merge
their lock hunks; the lead re-runs `cargo build --locked` to confirm.

### 2.2 Workspace and Cargo.toml changes

`windows/Cargo.toml` (seam WWS, one line):
```toml
members = ["codenotch", "codenotch-hook", "agentnotch-proto", "agentnotch-engine", "agentnotch-win", "agentnotch-hook", "agentnotch-release"] # Fork: WWS
```
The release profile stays upstream's (`lto`, `codegen-units=1`, `strip`, `opt-level="z"`).
Toolchain: Rust 1.98.1 pinned in CI (upstream needs ≥ 1.88, WP§1). All fork crates:
`edition = "2021"`, `rust-version = "1.88"`, `version = "0.0.0"` (never shown; the app's version
comes from `tauri.conf.json`, §6.6), `publish = false`, `license = "Apache-2.0"` (like
`Packages/ClaudeControl`; add `windows/agentnotch-engine/NOTICE` mirroring the package's).

`agentnotch-proto`:
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

`agentnotch-engine` (no C, no Tauri, no `windows*`; `check-seams.sh` enforces the names):
```toml
[dependencies]
agentnotch-proto = { path = "../agentnotch-proto" }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["raw_value"] }
sha2 = "0.10"
hmac = "0.12"
base64 = "0.22"
getrandom = "0.2"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", default-features = false, features = ["std", "clock"] }
regex = "1"
fancy-regex = "0.14"            # lookaround in the redaction/scrub patterns (CL§9.6)
unicode-segmentation = "1"      # grapheme-safe UTF-16 clamps (CL§1.5)
url = "2"                       # parsing only; canonical strings are built by hand (CL§3.1)
ruzstd = "0.8"                  # pure-Rust zstd for Claude Desktop's Simple Cache bodies (AU§12)
crossbeam-channel = "0.5"
[dev-dependencies]
tempfile = "3"
[features]
testkit = []                    # exposes engine::testkit to other crates' tests
```

`agentnotch-win` (on non-Windows it compiles to `stub.rs`: every constructor returns an error
"not available on this platform", so the fork crates' `cargo test` runs on macOS/Linux; the
upstream app crate itself builds only on Windows):
```toml
[dependencies]
agentnotch-engine = { path = "../agentnotch-engine" }
agentnotch-proto = { path = "../agentnotch-proto" }
serde_json = "1"
[target.'cfg(windows)'.dependencies]
windows = { version = "0.61", features = [
  "Win32_Foundation", "Win32_Security", "Win32_Security_Authorization", "Win32_System_Pipes",
  "Win32_System_IO", "Win32_System_Threading", "Win32_System_Diagnostics_ToolHelp",
  "Win32_System_Diagnostics_Debug", "Win32_System_JobObjects", "Win32_System_Console",
  "Win32_System_Registry", "Win32_System_SystemInformation", "Win32_System_Com",
  "Win32_System_DataExchange", "Win32_System_Memory", "Win32_Storage_FileSystem",
  "Win32_UI_WindowsAndMessaging", "Win32_UI_Accessibility", "Win32_UI_Shell",
  "Win32_UI_Shell_Common", "Win32_Security_AppLocker" /* SaferComputeTokenFromLevel, tests */,
  "Win32_NetworkManagement_NetManagement" /* NetUserAdd, the squatter test */,
  "Win32_UI_Input_KeyboardAndMouse", "Win32_Graphics_Dwm", "Win32_Graphics_Gdi",
  "Win32_Media_Audio", "Wdk_System_Threading",
  "Data_Xml_Dom", "UI_Notifications", "Foundation" ] }   # 0.61 = the version Tauri already locks
tokio = { version = "1", features = ["rt", "net", "io-util", "sync", "time", "macros"] }
ureq = { version = "2", default-features = false, features = ["json", "native-tls", "gzip"] }
native-tls = "0.2"
[dev-dependencies]
tempfile = "3"
agentnotch-engine = { path = "../agentnotch-engine", features = ["testkit"] }
[[bin]]
name = "fake-claude"            # test-only fake executable (never shipped: not in any bundle)
path = "src/bin/fake-claude.rs"
test = false
[[bin]]
name = "pipe-test-server"       # test-only: the real pipe server in its own process (integrity-level tests)
path = "src/bin/pipe-test-server.rs"
test = false
[[bin]]
name = "pipe-squatter"          # test-only: creates the pipe name first, records any byte it receives
path = "src/bin/pipe-squatter.rs"
test = false
```
`fake-claude` answers `--version` with `$FAKE_CLAUDE_VERSION` (default `2.1.282 (Claude Code)`),
speaks the probe's stream-json (`initialize`, then `get_usage` from a fixture named by
`$FAKE_CLAUDE_USAGE`), echoes argv as JSON when asked, can spawn a grandchild, and appends one
line per run (argv, cwd, and the **names** of the environment variables it received that match
`CLAUDE*`/`ANTHROPIC*`) to `$FAKE_CLAUDE_LOG`, a file outside every profile the tests hash.
`ureq`/`native-tls` are Windows-only on purpose: on Linux they would pull OpenSSL (C).

`agentnotch-hook`:
```toml
[[bin]]
name = "agentnotch-hook"
[[bin]]
name = "console-reader"         # test-only reader for console_type.rs; never bundled
path = "tests/bin/console-reader.rs"
test = false
[dependencies]                  # no argument parser, ever (§1.8); check-seams.sh rejects clap/argh/pico-args/lexopt here
agentnotch-proto = { path = "../agentnotch-proto" }
serde_json = "1"
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_Storage_FileSystem",
  "Win32_System_Pipes", "Win32_System_IO", "Win32_System_Threading", "Win32_Security",
  "Win32_Security_Authorization", "Win32_System_Console", "Win32_System_JobObjects",
  "Win32_System_Diagnostics_ToolHelp", "Wdk_System_Threading"] }     # 0.61 = the one tokio already locks
[dev-dependencies]
agentnotch-win = { path = "../agentnotch-win" }
agentnotch-engine = { path = "../agentnotch-engine", features = ["testkit"] }
```
Always built with `RUSTFLAGS=-C target-feature=+crt-static` into `target/hook` (BT§2).

`agentnotch-release` (C-free; runs on ubuntu in `release.yml`):
```toml
[dependencies]
ed25519-dalek = "2"
hkdf = "0.12"
sha2 = "0.10"
blake2 = "0.10"
base64 = "0.22"
serde_json = "1"
zeroize = "1"
minisign-verify = "=0.2.5"      # the exact crate and version tauri-plugin-updater verifies with
```

`windows/codenotch/Cargo.toml` (seam WB, three lines):
```toml
agentnotch-engine = { path = "../agentnotch-engine" } # Fork: WB
agentnotch-win = { path = "../agentnotch-win" } # Fork: WB
```
and under the existing `[target.'cfg(windows)'.dependencies]` table:
```toml
webview2-com = "0.38" # Fork: WB (panel: accelerator keys off; sealed snapshots via CapturePreview)
```
No other upstream dependency changes. No Tauri plugin is added (deep link comes from the
bundler config; the hotkey from `agentnotch-win`).

### 2.3 How `cfg(windows)` stays thin

- The engine declares every OS service as a trait (`platform.rs`, §3.2) and never names an OS
  API. Its logic runs identically under `testkit` fakes (macOS/Linux/Windows) and real services.
- `agentnotch-win` implements the traits with Win32/WinRT; each function does one OS thing and
  returns plain data (no policy). Anything decidable without the OS (which window to prefer,
  whether a tab title matches, which folder to probe, message safety rules) is engine code
  taking that plain data.
- The glue module in the app crate copies fields between the engine and Tauri and calls
  `agentnotch-win` for window styles; it owns no decision. Its only non-trivial code is window
  creation on the main thread and event plumbing.
- Anything in `agentnotch-win` that is pure (SDDL string building, frame parsing, the console
  record encoding of a string, UIA tab matching input) is a plain function with a unit test that
  runs on every OS.
- `check-seams.sh` enforces: engine and proto `Cargo.toml` name none of
  `tauri* wry tao webview2-com windows windows-sys windows-core winapi libc cc ring openssl*
  libsqlite3-sys zstd-sys`; no `use tauri`/`#[cfg(windows)]`/`std::os::windows` in engine or
  proto sources (the unix `testkit` file-security fake may use `#[cfg(unix)]`).
- The fork CI runs `cargo check --target x86_64-pc-windows-msvc` for all five fork crates on
  ubuntu and, locally, on the Mac (no C toolchain needed because no fork crate builds C).

### 2.4 The glue module `windows/codenotch/src/agentnotch/` (holds no logic)

| File | Contents |
|---|---|
| `mod.rs` | consts `DISPLAY_NAME = "Agent Notch"`, `SETTINGS_TITLE = "Agent Notch Settings"`, `DROPZONES_TITLE`, `SIGN_IN_REFUSED`; `rebrand(&str) -> String` (calls engine `core::rebrand`); `tray_tooltip()`; `sealed()` (env, read once); `data_folder_name()` (`"Agent Notch"` or `"Agent Notch Sealed"`); `hub(app) -> Hub`; the seam entry points `setup`, `an_call`, `second_instance`, `refresh_claude`, `hooks_switch_get`, `hooks_switch_set`, `cli`, `updater`, `notch_menu_items`, `notch_menu_event` |
| `setup.rs` | first checks it is the only instance: finds the single-instance plugin's event window (`FindWindowW("<identifier>-sic", "<identifier>-siw")`); when it belongs to another process (the mutex existed but that instance had not made its window yet when the plugin looked), forwards argv to it with the plugin's `WM_COPYDATA` format and exits 0; when none exists after 2 s of polling, exits 0 (logged). Then builds `Roots` (from `agentnotch_win::paths`), `HubConfig` (website from `website.rs`, flags from env/argv, hook exe path = `current_exe().with_file_name("agentnotch-hook.exe")`), `Platform` (from `agentnotch_win::platform()`), `app.manage(Bridge{hub})`, subscribes the event sink, starts the hub, handles a cold-start deep link in argv, starts sealed self-test when asked |
| `calls.rs` | `#[tauri::command] async fn an_call(app, window, method: String, args: serde_json::Value) -> Result<Value, CallError>`: glue-level methods (panel/window/clipboard/reveal/notification settings) handled here; everything else deserialised into `engine::Call` and passed to `hub.call` in `spawn_blocking`. Refuses methods not allowed from the calling window label (§3.7). |
| `emit.rs` | `HubEvent` → Tauri: `an:*` emits, `AppState.usage` + upstream `"usage"`, tray badge, log lines to upstream `applog` |
| `panel.rs` | creates/shows/hides/places `agentnotch-panel` via `run_on_main_thread` from a spawned thread; asks engine `geometry::panel` for the frame; applies `agentnotch-win::window` styles; confirms keyboard focus (`GetForegroundWindow() == panel`) 50 ms and 250 ms after every activation attempt and on every `WindowEvent::Focused`, emitting `an:panel_focus {focused}`; reports every open/close/route/focus/pin change to the engine as `Call::PanelState` (§3.5); re-anchors 0.35 s after `notch_edge`/`notch_insets` (`app.listen_any`) |
| `deeplink.rs` | a deep link is accepted only as **exactly one** argument after the exe that starts with `agentnotch://` (the single-instance plugin joins argv with `\|` and splits it back, so a URL holding `\|` arrives as several arguments and is ignored); hands it to `hub.handle_deep_link` |
| `cli.rs` | fork subcommands (§4.14); prints and writes `<data>\<cmd>.log`; returns `Some(exit code)` for every name it owns, `None` otherwise, and always `None` when any argument starts with `agentnotch:` (a deep link is never a subcommand) |
| `update.rs` | `updater(app)` = `app.updater_builder()` with `on_before_exit(hub.stop(); app.cleanup_before_exit())` (the updater calls `std::process::exit(0)` right after starting the installer, so `RunEvent::Exit` never fires; without this the review marks, usage and ledger writes still debounced would be lost and held requests would only end by the process dying) and, when the builder allows it, `current_exe_args` without any `agentnotch:` argument (so a cold-start deep link is not replayed into the restarted app; the stale-callback rule of §4.11 covers it otherwise) |
| `menu.rs` | `notch_menu_items(app, menu, provider)`: for a Claude ring adds "Open sessions panel" (and nothing for other providers); `notch_menu_event(app, id) -> bool` handles the fork's `notch:an-*` ids |
| `tray.rs` | draws the needs-you dot onto upstream's tray icon (`tray_by_id("main")`) |
| `selftest.rs` | sealed-only: `AGENTNOTCH_PANEL_SELF_TEST`, `AGENTNOTCH_SNAPSHOT_CLAUDE`, `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH` (§7.4) |
| `hotkey.rs` | registers the panel shortcut through `agentnotch-win::hotkey` and reports the result to the engine (`Call::HotkeyStatus`), which shows "That shortcut is taken by another app." in Settings |
| `website.rs` | `include_str!("../../../../app-config.json")` → engine `cloud::website::from_app_config` (the Mac's build-time rules, CL§3.2); an invalid file yields "no website" and fails the engine test that reads the same file |

### 2.5 Seams in upstream files (exact)

Tags: Rust/JS `// Fork: <id>`, TOML `# Fork: <id>`, HTML `<!-- Fork: <id> -->`. JSON has no
comments: its seams are verified by content. Every line below is a `SEAM` line in
`Scripts/fork-seams.txt` (Appendix A lists the file's new section verbatim).

**`windows/codenotch/src/main.rs`**

| Id | Anchor | Change (one line each) |
|---|---|---|
| WB1 | after `mod updater;` (line 31) | `mod agentnotch; // Fork: WB1` |
| WU1a | `mod usage;` (13) | `#[allow(dead_code)] mod usage; // Fork: WU1 upstream's Claude token path: compiled, never started` |
| WU1b | `mod claude_auth;` (14) | `#[allow(dead_code)] mod claude_auth; // Fork: WU1` |
| WH1 | `mod server;` (11) | `#[allow(dead_code)] mod server; // Fork: WH` |
| WH2 | `mod watcher;` (28) | `#[allow(dead_code)] mod watcher; // Fork: WH` |
| WH3 | `mod hooks_install;` (7) | `#[allow(dead_code)] mod hooks_install; // Fork: WH` |
| WD1 | `mod doctor;` (5) | `#[allow(dead_code)] mod doctor; // Fork: WD (the fork's doctor runs instead: WCLI)` |
| WU1c | `fn claude_sign_in` (667) | `fn claude_sign_in() -> Result<(), String> { Err(agentnotch::SIGN_IN_REFUSED.into()) } // Fork: WU1` |
| WU1d | `"claude" => { … usage::request_refresh(); }` (676–681, six lines) | `p if p == "claude" \|\| p.starts_with("claude-") => return agentnotch::refresh_claude(app, p), // Fork: WU1` (the fork's cells are `claude-acct-*`; every refresh path reaching upstream's `refresh_provider` with a Claude ring id lands in the glue) |
| WU1e | `usage::start(handle.clone());` (1891) | `// Fork: WU1 usage::start never runs: the bridge feeds AppState.usage and "usage"` |
| WH4 | `server::start(handle.clone(), port);` (1889) | `let _ = port; // Fork: WH upstream's TCP hook server stays off; hooks reach the bridge's named pipe` |
| WH5 | `watcher::start(handle.clone());` (1890) | `// Fork: WH upstream's transcript watcher stays off; the engine reads transcripts` |
| WB2 | after `apply_visibility(&handle);` (1888) | `agentnotch::setup(&handle); // Fork: WB2` |
| WB3 | first entry of `tauri::generate_handler![` (1810) | `agentnotch::an_call, // Fork: WB3` |
| WSI | first statement of the single-instance closure (1792) | `if agentnotch::second_instance(app, &_args) { return; } // Fork: WSI` |
| WCLI | after `let args: Vec<String> = std::env::args().collect();` (1749) | `if let Some(code) = agentnotch::cli::run(&args) { std::process::exit(code); } // Fork: WCLI` |
| WH6 | body of `get_hooks_installed` (1526) | `agentnotch::hooks_switch_get() // Fork: WH` |
| WH7 | body of `set_hooks_installed` (1531–1535) | `agentnotch::hooks_switch_set(on) // Fork: WH` |

FORBID in `main.rs` (non-comment lines): `usage::start(`, `server::start(`, `watcher::start(`,
`claude_auth::start_login`, `usage::request_refresh()`. Upstream's CLI arms (`"install-hooks"`,
`"doctor"` …) stay in the file but are unreachable: WCLI claims those names first (a glue unit
test pins that `cli::run` returns `Some` for each of them).

**Other upstream Rust files**

| Id | File:anchor | Change |
|---|---|---|
| WR-DIR | `config.rs:269` `.join("codenotch")` | `.join(crate::agentnotch::data_folder_name()) // Fork: WR-DIR` (`Agent Notch`, or `Agent Notch Sealed` when sealed, so a sealed run never touches the real config) |
| WUP2 | `updater.rs:122` `let Some(update) = app.updater()?.check().await? else {` | `let Some(update) = crate::agentnotch::updater(&app)?.check().await? else { // Fork: WUP2` (the hub stops before the updater's `process::exit`) |
| WNM | `notchmenu.rs:39`, after the `if let Some((id, host)) = page { … }` block | `menu = crate::agentnotch::notch_menu_items(app, menu, provider.as_deref()); // Fork: WNM` |
| WNM | `notchmenu.rs:85`, first statement after `let Some(item) = … else { return; };` | `if crate::agentnotch::notch_menu_event(app, item) { return; } // Fork: WNM` |
| WC | `config.rs:174` body of `default_weekly_ring` | `"outside".into() // Fork: WC (the Mac fork's default: weekly ring outside)` |
| WR-RUN | `autostart.rs:8` | `const NAME: &str = "Agent Notch"; // Fork: WR-RUN (the uninstaller deletes the Run value named after productName)` |
| WR-TRAY | `tray.rs:16` | `.tooltip(crate::agentnotch::tray_tooltip()) // Fork: WR-TRAY` |
| WR-TRAY | `tray.rs:113` | `crate::agentnotch::tray_tooltip() // Fork: WR-TRAY` |
| WR-TRAY | `tray.rs:115` | `format!("{} — {}", crate::agentnotch::DISPLAY_NAME, parts.join(" · ")) // Fork: WR-TRAY` |
| WR-QUIT | `tray.rs:75` | `let quit = MenuItemBuilder::with_id("quit", crate::agentnotch::rebrand(tr(&lang, "quit_app"))).build(app)?; // Fork: WR-QUIT` |
| WR-QUIT | `notchmenu.rs:45` | `let quit = MenuItemBuilder::with_id(format!("{PREFIX}quit"), crate::agentnotch::rebrand(tr(&lang, "quit_app"))) // Fork: WR-QUIT` |
| WR-TITLE | `settings_window.rs:34` | `.title(crate::agentnotch::SETTINGS_TITLE) // Fork: WR-TITLE` |
| WR-TITLE | `dropzones.rs:42` | `.title(crate::agentnotch::DROPZONES_TITLE) // Fork: WR-TITLE` |
| WUP | `updater.rs:71` | `.is_some_and(\|k\| !k.is_empty() && k != UNSET_PUBKEY) && !crate::agentnotch::sealed() // Fork: WUP` |

`i18n.rs` is not edited: its tests pin upstream strings ("Выйти из Codenotch"); the two call
sites rebrand instead (WR-QUIT), the Mac's R1/R3 lesson.

**`windows/codenotch/tauri.conf.json`** (content seams; FORBID `com.immidi.codenotch`,
`vinzdg/codenotch`, `"createUpdaterArtifacts": true`, `dW50cnVzdGVkIGNvbW1lbnQ6` (a pubkey),
`releases/latest/download/latest.json` (a feed)):
```json
  "productName": "Agent Notch",
  "mainBinaryName": "agentnotch",
  "version": "<VERSION>",
  "identifier": "com.rivantmedia.agentnotch",
  …
  "bundle": { …, "publisher": "Rivant Media",
              "windows": { "nsis": { "installerHooks": "nsis/agentnotch-hooks.nsh" } } },
  "app": { …, "security": {
      "csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
      "dangerousDisableAssetCspModification": ["style-src"] } },
  "plugins": {
    "updater": { "pubkey": "", "windows": { "installMode": "passive" } },
    "deep-link": { "desktop": { "schemes": ["agentnotch"] } }
  }
```
`endpoints` is removed from the source config (a copy built from source never updates itself).
`check-seams.sh` has a special rule: the `"version"` value equals the repo's `VERSION`.

The CSP (seam WCSP) covers every page, upstream's `notch.html`, `settings.html` and
`dropzones.html` included, because fork scripts render third-party and model-influenced text
(organisation names, session titles) into the pages that can call `hook_consent` and `cloud`.
Tauri adds hashes of each page's inline `<script>` blocks to `script-src` at build time, so
upstream's inline scripts keep running; `style-src` is exempted from that modification so its
`'unsafe-inline'` stays effective for the style attributes upstream's HTML strings use. Inline
event-handler attributes, `javascript:` URLs, `eval` and remote resources are blocked. WP0 audits
upstream's three pages for those before landing the seam and records any exception in the seam
list; the sealed self-test fails on any `securitypolicyviolation` in any page (§7.4), so an
upstream merge that needs one is caught on the first CI run.

**`windows/codenotch/tauri.bundle.conf.json`**:
`"../target/hook/release/agentnotch-hook.exe": "agentnotch-hook.exe"` (FORBID `codenotch-hook.exe`).

**`windows/codenotch/ui/notch.html`** (§5.2 defines each hook's contract):

| Id | Anchor | Line |
|---|---|---|
| WS1 | before `<script>` (line 279) | `<link rel="stylesheet" href="agentnotch/notch.css"><script src="agentnotch/notch.js"></script><!-- Fork: WS1 -->` |
| WS2 | first line of `function claudeCells(){` (975) | `if(window.agentnotch){const c=agentnotch.claudeCells();if(c)return c;} // Fork: WS2` |
| WS3 | last statement inside `for(const p of ps){ … }` of `renderRing` (after `wrap.classList.toggle('stale', …)`, 1109) | `if(window.agentnotch)agentnotch.decorateCell(p,cell); // Fork: WS3` |
| WS4 | `if(p.base==='claude'){ // live sessions …` (1205) | `if(p.base==='claude'&&window.agentnotch){html+=agentnotch.cardSessions(p);}else if(p.base==='claude'){ // Fork: WS4` |
| WS5 | `if(press&&!dragging&&press.id) refreshRing(press.id);` (1430) | `if(press&&!dragging&&press.id&&!(window.agentnotch&&agentnotch.ringClick(press.id))) refreshRing(press.id); // Fork: WS5` |

**`windows/codenotch/ui/settings.html`**:

| Id | Anchor | Line |
|---|---|---|
| WSS1 | before `<button class="row" id="tab-accounts"` (126) | `<button class="row" id="tab-claude" role="tab" aria-controls="pane-claude" aria-selected="false" tabindex="-1"><span class="badge b-claude"></span><span>Claude Code</span></button><!-- Fork: WSS1 -->` |
| WSS2 | before `<section class="pane" id="pane-accounts"` (141) | `<section class="pane" id="pane-claude" role="tabpanel" aria-labelledby="tab-claude" hidden></section><!-- Fork: WSS2 -->` |
| WSS3 | `const TABS = ['accounts', …` (1020) | `const TABS = ['claude', 'accounts', 'appearance', 'general']; // Fork: WSS3` |
| WSS4 | `const TITLES = {…}` (1021) | `const TITLES = { claude:'Claude Code', accounts:'Accounts', appearance:'Appearance', general:'General' }; // Fork: WSS4` |
| WSS5 | after the page's closing `</script>` | `<link rel="stylesheet" href="agentnotch/settings.css"><script src="agentnotch/settings.js"></script><!-- Fork: WSS5 -->` |

`notch.html`/`settings.html` keep every upstream test passing (`test-claude-auth-ui.cjs` still
finds its block untouched; the fork simply never emits `needsAuth` for Claude, §5.2).

**New fork files inside upstream folders** (ALLOW): `windows/codenotch/capabilities/agentnotch.json`,
`windows/codenotch/nsis/**`, `windows/codenotch/src/agentnotch/**`, `windows/codenotch/ui/agentnotch/**`.

Upstream's `.github/workflows/windows.yml` and `windows-package.yml` are never edited (they stay
gated to `vinzdg/codenotch`, except `windows-package.yml`'s `workflow_dispatch`, which must never
be run in the fork: it builds upstream's token-reading app; CLAUDE.md says so).

---

## 3. Rust public interface

The code below is normative for names, fields and semantics; implementers may add private items
and derive more traits. Every public type derives `Debug, Clone, Serialize, Deserialize,
PartialEq` unless noted. JSON is **snake_case** (serde default) for everything the fork's UI
sees, like upstream's snapshots; the cloud contract types keep `web/contract`'s camelCase.
Every enum that reaches JSON carries `#[serde(rename_all = "snake_case")]` (externally tagged
unless a `tag` is given: `Answer::Allow { always: false }` is `{"allow":{"always":false}}`,
`Answer::ApprovePlan` is `"approve_plan"`). Newtype ids serialise as plain strings
(`#[serde(transparent)]`). Times: `SystemTime` inside Rust; `*_ms: u64` (epoch ms) in UI JSON.

Two exceptions, each with its own types so the rule above never leaks into them:
- **Wire formats fixed elsewhere** keep their own names: the cloud contract types (camelCase,
  `web/contract`), and every enum that reaches the contract through an explicit mapping, never
  through serde's default: `UsageSource::{Probe, StatusLine, Cache, Desktop}` →
  `"probe" | "statusLine" | "claudeJson" | "desktop"` (`UsageSource::contract_name()`, tested
  against `web/contract/fixtures/sync-request.json`).
- **Files in the Mac's formats** (`accounts.json`, `usage-state.json`, `review-state.json`,
  `control-settings.json`, the `cloud-*.json` files) are read and written only through
  per-file data-transfer types in `persist::*` (and `cloud::files`), with explicit
  `#[serde(rename = …)]` for every field (camelCase as the Mac writes them) and date adapters
  matching each file's Mac encoding (ISO-8601 whole seconds `Z`, or epoch seconds as `f64`,
  per AU§3.7, AU§9.10, HS§5.13, CL§4.2). The model types never derive the file format; a
  round-trip test per file reads a Mac-written fixture and writes it back byte-equivalent (JSON
  equivalence).

### 3.1 `agentnotch-proto` (complete in WP0)

```rust
pub const PROTOCOL: u32 = 1;
pub mod limits {
    pub const MAX_CLIENT_MESSAGE: usize = 1 << 20;   // hook re-truncates above (HS§1.5)
    pub const MAX_SERVER_MESSAGE: usize = 8 << 20;   // server closes above
    pub const MAX_RESPONSE: usize = 4 << 20;
    pub const MAX_TOOL_INPUT_STRING: usize = 20_000; // …and every other HS§1.2 constant
}
pub fn pipe_name(user_sid: &str) -> String;                 // \\.\pipe\agentnotch-hook-<sid>
pub fn dev_pipe_override(get_env: impl Fn(&str) -> Option<String>, honour_without_dev: bool) -> Option<String>;
pub fn write_frame(w: &mut impl std::io::Write, json: &[u8]) -> std::io::Result<()>;
pub fn read_frame(r: &mut impl std::io::Read, max: usize) -> Result<Vec<u8>, FrameError>;
pub enum FrameError { Eof, TooLarge(usize), Io(std::io::Error) }   // not Serialize

/// What the hook exe knows besides stdin. `pid_guess` is what §1.4's pid rule found when
/// CLAUDE_PID is absent (the exe computes it; the pure builder only applies the precedence).
pub struct HookEnv { pub claude_pid: Option<String>, pub claude_config_dir: Option<String>,
    pub attended: Option<String>, pub entrypoint: Option<String>, pub pid_guess: Option<u32>,
    pub hook_pid: u32, pub wt_session: Option<String>, pub term_program: Option<String> }
/// §1.4's walk, pure over a process list: `exec_form` = argv carried `--exec`.
pub fn pid_guess(exec_form: bool, me: &ProcLink, ancestors: &[ProcLink]) -> Option<u32>;
pub struct ProcLink { pub pid: u32, pub image: String, pub created: u64 /* FILETIME */ }
/// argv after the exe → what to do; anything unknown or extra is `Ignore` (exit 0, no output).
pub enum Invocation { Hook { exec_form: bool }, StatusLine, Type(TypeArgs), ConsoleInfo { pid: u32 }, Ignore }
pub fn parse_invocation(args: &[std::ffi::OsString]) -> Invocation;
/// HS§1.4 build_message, byte-for-byte field semantics; None when stdin is not a JSON object.
pub fn build_hook_message(stdin: &serde_json::Value, env: &HookEnv) -> Option<serde_json::Value>;
/// HS§1.5: re-truncate to fit MAX_CLIENT_MESSAGE.
pub fn encode_hook_message(msg: &serde_json::Value) -> Vec<u8>;
pub fn build_statusline_message(stdin: &serde_json::Value, env: &HookEnv) -> Option<serde_json::Value>;

#[serde(rename_all = "lowercase")] pub enum Decision { Allow, Deny, Ask }
pub struct PermissionResponse { pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")] pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub updated_input: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub updated_permissions: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub interrupt: Option<bool> }
/// HS§1.7: the exact stdout text (without trailing newline) or None (= print nothing).
pub fn permission_output(original_tool_input: &serde_json::Value, r: &PermissionResponse) -> Option<String>;
pub const DEFAULT_DENY_MESSAGE: &str = "Denied by user via Agent Notch";
pub const KEEP_PLANNING_REASON: &str = "The user reviewed the plan and wants to keep planning. Stay in plan mode and ask what to change before implementing.";

pub struct ControlRequest { pub protocol: u32, pub event: String /* "AgentNotchControl" */, pub op: ControlOp }
#[serde(rename_all = "lowercase")] pub enum ControlOp { Status, Quit }
pub struct ControlStatus { pub version: String, pub sealed: bool, pub elevated: bool, pub accounts: u32,
    pub rings: u32, pub readings: u32 /* rings with a usage reading */, pub sessions: u32, pub held: u32,
    pub hook_consent: String, pub transport: String, pub cloud: String, pub sync: bool }

/// The two-phase typing protocol between the engine and `agentnotch-hook.exe type` (§4.8).
pub struct TypeArgs { pub pid: u32, pub started_ms: u64, pub expect_window: Option<u64>, pub allowed_shells: Vec<u32> }
pub enum TypePhase { Typed, Outcome { outcome: String, reason: Option<String> } }   // one JSON line each on stdout
```
Every response and control struct is decoded with unknown fields ignored (`#[serde(default)]`
on optional fields, no `deny_unknown_fields` anywhere in proto).

Unit tests (every OS): the Mac's `HookEventDecodingTests`/script test vectors, all HS§1.7 stdout
strings, frame round trips, truncation at 1 MiB, `pipe_name`, dev override rule,
`parse_invocation` (unknown subcommand, extra arguments, `--exec`, no arguments → `Ignore` or the
right variant), `pid_guess` vectors (exec form; bash → claude; powershell → node; cmd → bash →
claude; a parent younger than its child is not a link; VS Code as first non-shell → none).

### 3.2 Engine platform traits (`agentnotch_engine::platform`)

```rust
pub trait Clock: Send + Sync { fn now(&self) -> SystemTime; fn monotonic(&self) -> Instant; }

pub struct Roots {
    pub home: PathBuf,                 // %USERPROFILE% (as Claude Code resolves it)
    pub data: PathBuf,                 // folder of upstream's config.json = %APPDATA%\Agent Notch (Known Folder)
    pub support: PathBuf,              // %LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude (or AGENTNOTCH_SUPPORT_DIR)
    pub claude_desktop: Vec<PathBuf>,  // %APPDATA%\Claude, each %LOCALAPPDATA%\Packages\*claude*|*anthropic*\LocalCache\Roaming\Claude
    pub system_users: Option<PathBuf>, // %SystemDrive%\Users (scrub LocalNames)
    pub install_dir: Option<PathBuf>,  // folder of agentnotch.exe (source of the hook exe copy)
}

#[derive(Copy)] pub enum Liveness { Alive, Gone, Unknown }
pub struct ProcEntry { pub pid: u32, pub ppid: u32, pub exe_name: String, pub started: Option<SystemTime> }
pub struct ProcessTable { pub entries: Vec<ProcEntry> }         // parent links valid only when parent.started <= child.started
pub enum EnvRead { Set(String), Unset, Unreadable }             // Unset only after a complete, terminated block was read
pub trait Processes: Send + Sync {
    fn liveness(&self, pid: u32) -> Liveness;                  // ACCESS_DENIED = Alive
    fn start_time(&self, pid: u32) -> Option<SystemTime>;     // GetProcessTimes creation
    fn table(&self) -> ProcessTable;                          // Toolhelp32 + creation times
    fn config_dir_env(&self, pid: u32) -> EnvRead;            // same user only; PEB read (§4.2)
    fn same_user(&self, pid: u32) -> Option<bool>;
    fn elevated(&self, pid: u32) -> Option<bool>;             // TokenElevation; None = can't tell (e.g. elevated target from a normal app)
    fn exe_path(&self, pid: u32) -> Option<PathBuf>;          // QueryFullProcessImageNameW
}

pub struct CommandSpec { pub program: PathBuf, pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,  // the COMPLETE environment; the runner clears first
    pub cwd: PathBuf }
pub enum Exit { Code(i32), Killed }
pub trait RunningCommand: Send {
    fn pid(&self) -> u32;
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>>;
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    fn wait_timeout(&mut self, d: Duration) -> io::Result<Option<Exit>>;
    fn kill_tree(&mut self);                                  // TerminateJobObject; idempotent
}
pub trait CommandRunner: Send + Sync {                        // Job object KILL_ON_JOB_CLOSE, CREATE_NO_WINDOW, piped stdio
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>>;
}

pub type ConnId = u64;
pub struct IncomingFrame { pub conn: ConnId, pub bytes: Vec<u8>, pub received_at: SystemTime, pub peer_pid: Option<u32> }
pub enum TransportEvent { Frame(IncomingFrame), PeerClosed(ConnId), Listening(String), Error(String) }
pub trait HookTransport: Send + Sync {
    /// Starts listening; events go to `sink` in arrival order.
    fn start(&self, pipe_name: &str, sink: crossbeam_channel::Sender<TransportEvent>) -> Result<(), String>;
    fn respond(&self, conn: ConnId, frame_json: Vec<u8>) -> bool;   // writes (2 s), then closes
    fn close(&self, conn: ConnId);                                   // closes without answering
    fn stop(&self);
}

pub enum HostKind { WindowsTerminal, Conhost, VsCode { product: String }, JetBrains, OtherConsoleHost { exe: String }, NoConsole, Unknown }
pub struct HostApp { pub kind: HostKind, pub window: Option<u64> /* HWND */, pub host_pid: Option<u32>, pub exe_path: Option<PathBuf> }
pub struct ConsoleInfo { pub attached: bool, pub window: Option<u64>, pub title: Option<String>,
    pub processes: Vec<u32>, pub line_input: Option<bool> /* CONIN$ has ENABLE_LINE_INPUT */,
    pub elevated_target: bool, pub error: Option<String> }
pub enum FocusStep { SelectWtTab { window: u64, title: String }, RaiseWindow { window: u64 },
    OpenInEditor { editor_exe: PathBuf, folder: PathBuf }, ActivatePid { pid: u32 } }
pub enum FocusOutcome { Focused, RaisedOnly, NotFound, Failed(String) }
pub struct Foreground { pub pid: u32, pub window: u64, pub title: String, pub fullscreen: bool }
pub trait Terminals: Send + Sync {
    fn classify_host(&self, claude_pid: u32, table: &ProcessTable) -> HostApp;
    fn console_info(&self, claude_pid: u32) -> ConsoleInfo;          // via `agentnotch-hook.exe console-info`
    fn run_focus(&self, step: &FocusStep) -> FocusOutcome;
    fn foreground(&self) -> Option<Foreground>;
    fn window_title(&self, window: u64) -> Option<String>;
    fn wt_tab_titles(&self, window: u64) -> Option<Vec<(String, bool /*selected*/)>>;   // UIA
    fn any_terminal_visible(&self) -> bool;                           // EnumWindows, not cloaked/minimized
    fn watch_foreground(&self, sink: crossbeam_channel::Sender<Foreground>);  // SetWinEventHook thread (an-foreground)
}
pub enum TypeOutcome { Delivered, Refused(String), TypedNotSubmitted(String), Failed(String) }
pub struct ConsoleTarget { pub claude_pid: u32, pub claude_started: SystemTime, pub expected_window: Option<u64>,
    pub allowed_shells: Vec<u32> /* claude's direct parent chain of known shells, validated by creation time */ }
pub trait ConsoleInput: Send + Sync {                              // via `agentnotch-hook.exe type` (§4.8)
    /// Types `text`, then calls `recheck` (the engine's fresh message_safety); presses Return only
    /// when it returns true within 2 s, else reports TypedNotSubmitted.
    fn type_text(&self, target: &ConsoleTarget, text: &str, recheck: &mut dyn FnMut() -> bool) -> TypeOutcome;
}

pub enum ToastKind { NeedsInput, Review, Failed, Limit }
pub struct Toast { pub tag: String /* ≤64 */, pub group: String /* ≤64 */, pub kind: ToastKind,
    pub title: String, pub subtitle: Option<String>, pub body: String,
    pub launch_url: String,                        // agentnotch://open?…  (protocol activation)
    pub actions: Vec<(String /*label*/, String /*agentnotch:// url*/)> }
pub enum NotifyPermission { Allowed, DisabledForApp, DisabledForUser, DisabledByPolicy, Unavailable }
pub trait Notifier: Send + Sync {
    fn post(&self, t: &Toast); fn withdraw(&self, tag: &str, group: &str); fn permission(&self) -> NotifyPermission;
}
pub enum Chime { Blocked, Finished }
pub trait Sounds: Send + Sync { fn play(&self, c: Chime); }

pub struct HttpRequest { pub method: String, pub url: String, pub headers: Vec<(String, String)>, pub body: Option<Vec<u8>>, pub timeout: Duration }
pub struct HttpResponse { pub status: u16, pub headers: Vec<(String, String)>, pub body: Vec<u8> }
pub enum HttpError { Timeout, Connect(String), Tls(String), Other(String) }
pub trait Http: Send + Sync { fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }  // no cookies, no cache
pub trait Browser: Send + Sync { fn open(&self, url: &str) -> Result<(), String>; }

pub struct FileIdentity { pub volume: u64, pub index: u128, pub modified_ns: i128, pub size: u64 }
pub enum WriteMode { Private /* protected DACL user+SYSTEM */, KeepTargetSecurity /* copy the replaced file's DACL, protection flag and attributes; a new file inherits the folder's */ }
pub enum Expect { Nothing /* no check */, Absent /* must not exist */, Same(FileIdentity) /* must still be exactly this file */ }
pub enum WriteResult { Written, Changed /* not what was expected at the last moment: nothing written */,
    Vanished /* expected an existing file and it is gone: nothing written */, ReadOnly }
pub trait SecureFiles: Send + Sync {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()>;
    /// Stage `.<name>.agentnotch-<8 hex>.tmp` beside `path` (CREATE_NEW, same security as the mode
    /// asks), write, FlushFileBuffers, re-check `expect`, then one atomic rename over the target:
    /// SetFileInformationByHandle(FileRenameInfoEx, REPLACE_IF_EXISTS | POSIX_SEMANTICS), falling
    /// back to MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH) where the class is unsupported. A
    /// failed rename leaves the target untouched (it is all or nothing); ≤ 5 retries on
    /// sharing/access errors over ~500 ms; the stage is always deleted on failure. `path` must be
    /// already resolved (canonical); callers resolve links first.
    fn write_atomic(&self, path: &Path, bytes: &[u8], mode: WriteMode, expect: Expect) -> io::Result<WriteResult>;
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool>;       // false = existed
    fn identity(&self, path: &Path) -> io::Result<FileIdentity>;
    fn is_reparse(&self, path: &Path) -> io::Result<bool>;                             // symlink or junction; never follows
    fn canonical(&self, path: &Path) -> io::Result<PathBuf>;                           // GetFinalPathNameByHandleW, no \\?\ prefix
    fn is_private(&self, path: &Path) -> io::Result<bool>;                             // protected, only owner (+SYSTEM)
}
pub trait Device: Send + Sync { fn computer_name(&self) -> String; fn user_sid(&self) -> Option<String>;
    fn elevated(&self) -> bool;                                      // this app's own token
    fn smart_app_control(&self) -> Option<String>;                   // "on" | "off" | "evaluation" (doctor only)
}

pub struct Platform {
    pub clock: Arc<dyn Clock>, pub processes: Arc<dyn Processes>, pub runner: Arc<dyn CommandRunner>,
    pub transport: Arc<dyn HookTransport>, pub terminals: Arc<dyn Terminals>, pub console: Arc<dyn ConsoleInput>,
    pub notifier: Arc<dyn Notifier>, pub sounds: Arc<dyn Sounds>, pub http: Arc<dyn Http>,
    pub browser: Arc<dyn Browser>, pub files: Arc<dyn SecureFiles>, pub device: Arc<dyn Device>,
}
```
`agentnotch_win::platform(roots: &Roots, hook_exe: &Path) -> Platform` builds the real set.
`agentnotch_engine::testkit::platform(root: &Path) -> (Platform, TestHandles)` builds fakes
(`FakeClock` manual time, `FakeProcesses` scripted table, `ScriptedRunner` (stdout lines per
stdin line), `MemoryTransport` (inject frames, capture responses), `RecordingNotifier`,
`FixtureHttp` (canned responses, records requests), `StdSecureFiles` (unix 0700/0600 + dev/inode),
`FakeTerminals`, `FakeConsole`, `FakeDevice`).

### 3.3 Core model types (`agentnotch_engine::model`, WP0)

```rust
pub struct AccountId(pub String);      // normalized config folder, display case (AU§1.2)
pub struct IdentityId(pub String);     // "uuid:<acct>[/<org>]" | "email:<e>" | "dir:<folder id>" (AU§5.1)
pub struct RingId(pub String);         // "claude-acct-<12 hex>" | "claude" | "claude-<slug>" | "claude-dir-<8 hex>" (AU§5.4)
pub struct SessionId(pub String);

pub enum FolderKind { Run, Store, Infrastructure }
pub struct Identity {                  // from oauthAccount (AU§4.3)
    pub account_uuid: Option<String>, pub email: Option<String>, pub display_name: Option<String>,
    pub organization_name: Option<String>, pub organization_uuid: Option<String>,
    pub organization_type: Option<String>, pub rate_limit_tier: Option<String>,
    pub billing_type: Option<String>, pub has_extra_usage_enabled: Option<bool> }
pub struct RunFolder {                 // one Claude config folder = ClaudeAccount (AU§2.1)
    pub id: AccountId, pub config_dir: PathBuf, pub config_dir_env: Option<String>,
    pub custom_label: Option<String>,
    pub seen_config_dir_envs: Vec<String>,   // collapsed by paths::key() on Windows: the login is the folder's own
                                             // .credentials.json (case-insensitive path), not a Keychain item per
                                             // spelling, so Windows never shows the "login conflict" chip nor runs the
                                             // spelling-switching identity refresh; only set-versus-unset matters for
                                             // the default folder (~\.claude.json vs ~\.claude\.claude.json)
    pub identity: Option<Identity>,
    pub subscription_type: Option<String>, pub color_index: u8, pub source: FolderSource,
    pub last_seen_at: Option<SystemTime>, pub is_hidden: bool, pub kind: FolderKind }
pub enum FolderSource { Discovered, Hook, Manual }
pub struct Account {                   // one identity = one ring (ClaudeIdentityAccount, AU§5.3)
    pub identity_id: IdentityId, pub ring_id: RingId, pub label: String, pub own_label: Option<String>,
    pub monogram: String, pub color_index: u8, pub email: Option<String>, pub plan_name: Option<String>,
    pub organization_uuid: Option<String>, pub run_dirs: Vec<AccountId>, pub store_dirs: Vec<AccountId>,
    pub includes_default: bool, pub is_tracked: bool, pub ring_shown: bool, pub is_signed_in: bool,
    pub launch_command: Option<String>, pub can_forget: bool }

pub struct UsageWindow { pub utilization: f64, pub resets_at: Option<SystemTime>, pub duration_s: u64 }
pub enum UsageSource { Probe, StatusLine, Cache, Desktop }   // contract names via contract_name(): probe|statusLine|claudeJson|desktop
pub struct AccountUsage {             // AU§8.4
    pub account_id: IdentityId, pub five_hour: Option<UsageWindow>, pub seven_day: Option<UsageWindow>,
    pub scoped: Vec<(String, UsageWindow)>, pub extra_usage: Option<ExtraUsage>,
    pub subscription_type: Option<String>, pub source: UsageSource, pub updated_at: SystemTime,
    pub taken_after: Option<SystemTime> }
pub struct ExtraUsage { pub is_enabled: bool, pub monthly_limit: Option<f64>, pub used_credits: Option<f64>, pub utilization: Option<f64>, pub currency: Option<String> }
pub struct UsageReading { pub window: UsageWindow, pub at: SystemTime, pub not_before: Option<SystemTime> }  // AU§9.3
pub enum RingStatus { Ok, Stale, Waiting, SignInNeeded, Unavailable, Failed }

pub enum Phase { Idle, Processing, WaitingForInput, WaitingForApproval(PermissionContext), Compacting, Ended } // HS§5.2
pub struct PermissionContext { pub tool_use_id: String, pub tool_name: String, pub tool_input: serde_json::Value,
    pub received_at: SystemTime, pub permission_suggestions: Vec<serde_json::Value>,
    pub has_synthetic_tool_use_id: bool, pub agent_id: Option<String>, pub activated_at: Option<SystemTime> }
pub enum NeedsInputReason { Permission { tool: Option<String> }, Question, PlanApproval,
    Elicitation { message: String }, Dialog { detail: String }, Error { text: String, code: Option<String> } }
pub enum SessionState {                // the five user-facing states (HS§5.3)
    NeedsYou(NeedsInputReason),        // answerable reasons
    Failed(NeedsInputReason /* Error */),
    ReadyForReview, Working, Idle }
pub enum Bucket { NeedsYou, ReadyForReview, Working, Idle }   // Failed sorts inside NeedsYou, after answerable
pub struct Session {                   // the engine's per-session record (SessionState.swift port); fields private to WP5,
    /* … */                            // exposed read-only through SessionView below
}
pub struct SessionView { pub id: SessionId, pub account: Option<AccountId>, pub ring: Option<RingId>,
    pub attribution: Attribution, pub attribution_since: SystemTime /* when the current attribution began */,
    pub cwd: PathBuf, pub project_name: String, pub title: String,
    pub title_from_folder: bool /* the title was derived from the folder name (the cloud then sends the conversation summary) */,
    pub state: SessionState, pub phase: Phase, pub pid: Option<u32>, pub pid_started: Option<SystemTime>,
    pub entrypoint: Option<String>, pub config_dir_env: Option<String>, pub host_session_id: Option<String>,
    pub registry_status: Option<String> /* busy|shell|idle|waiting from sessions\<pid>.json */,
    pub first_seen_at: SystemTime, pub model: Option<String>, pub context_pct: Option<f64>,
    pub tasks: Option<TaskProgress>, pub background: BackgroundWait, pub last_activity: SystemTime,
    pub turn_started_at: Option<SystemTime>, pub completed_at: Option<SystemTime>,
    pub reviewed_at: Option<SystemTime>, pub last_assistant_message: Option<String>,
    pub pending: Vec<PendingRequest>, pub cost_usd: Option<f64>, pub transcript_path: Option<PathBuf> }
pub enum Attribution { Known(Option<IdentityId>) /* Known(None) = folder not grouped yet */, Unsure(Option<IdentityId>), Waiting }
/// How long an unplaced session (Known(None), or Desktop-hosted without host id and registry
/// status) waits before it counts as unsure for the cloud (CLAUDE.md "Unsure sessions").
pub const PLACEMENT_GRACE: Duration = Duration::from_secs(30);
pub struct TaskProgress { pub done: u32, pub total: u32, pub active_label: Option<String>, pub items: Vec<TaskItem> }
pub struct TaskItem { pub id: Option<String>, pub label: String, pub status: TaskStatus }
pub enum TaskStatus { Pending, InProgress, Completed, Deleted /* TaskUpdate status "deleted": hidden, not counted */,
    CreateFailed /* a TaskCreate whose PostToolUse failed: never counted */ }   // HS§5.9
pub struct BackgroundWait { pub since: Option<SystemTime>, pub agent_types: Vec<String>, pub task_count: u32 }

pub enum RequestKind { Permission, Question, Plan }
pub struct PendingRequest {            // one held PermissionRequest as the UI sees it
    pub session_id: SessionId, pub tool_use_id: String, pub kind: RequestKind, pub tool_name: String,
    pub received_at: SystemTime, pub activated_at: Option<SystemTime>, pub input_preview: String,
    pub input: serde_json::Value, pub always: Option<AlwaysRule>, pub needs_review: bool,
    pub questions: Option<Vec<Question>>, pub plan_markdown: Option<String>, pub agent_id: Option<String> }
pub struct AlwaysRule { pub description: String, pub suggestion: serde_json::Value, pub inline: bool }
pub struct Question { pub text: String, pub header: Option<String>, pub multi_select: bool, pub options: Vec<QuestionOption> }
pub struct QuestionOption { pub label: String, pub description: Option<String> }
pub enum Answer {                      // what the UI can send (HS§6)
    Allow { always: bool }, Deny { reason: Option<String> },
    Questions { answers: BTreeMap<String, String> }, ApprovePlan, KeepPlanning }

pub struct ReviewItem {                // review-state.json record (HS§5.13)
    pub completed_at: Option<SystemTime>, pub reviewed_at: Option<SystemTime>, pub last_assistant_message: Option<String>,
    pub stop_error: Option<String>, pub stop_error_code: Option<String>, pub failed_at: Option<SystemTime>,
    pub background_wait_since: Option<SystemTime>, pub background_agent_types: Vec<String>, pub updated_at: SystemTime }

pub enum CloudAuthState { SignedOut, SigningIn, SignedIn { email: Option<String> }, Error { message: String } }
pub struct CloudState {                // ClaudeCloudState (CL§5.9); JSON is snake_case for the UI
    pub website_url: Option<String>, pub website_is_overridden: bool, pub auth: CloudAuthState,
    pub sync_enabled: bool, pub summaries_enabled: bool, pub summaries_available: bool,
    pub is_syncing: bool, pub last_sync_at_ms: Option<u64>, pub last_error: Option<String>,
    pub pending_sessions: u32, pub pending_usage: u32, pub summarized_sessions: u32,
    pub dashboard_url: Option<String>, pub pools_url: Option<String>, pub settings_url: Option<String> }
```

### 3.4 Internal service interfaces (fixed by WP0 so packages can work in parallel)

Each service is a plain struct driven by `an-core` with an explicit `now`; blocking work is
described as a `Job` the runtime executes on a worker. **Every type that crosses a package
boundary is written out below** (in `runtime_types.rs` and the model files, all by WP0). Only
these are "straight ports" of a Mac type with the same name, field for field, written by WP0 from
the cited report section: `HookEvent` and `StatusLineMessage` (HS§1.4, §2, HookEvent.swift),
`HeldPermission` (HS§4.3), `RegistrySnapshot`/`RegistryEntry` (HS§5.8, `sessions\<pid>.json`),
`AccountSighting` (AU§3.4), `FolderSnapshot` (AU§3.1), `AttentionTransition` (HS§7.1),
`LiveSessionObservation` (CL§6.1), `BackfillFolder` (CL§7.2), `ChatPage` (HS§5.11),
`DesktopReading` (AU§12), `HookInstallRecord` (§1.5). Later packages may add fields but never
rename or remove one without the lead. Signatures (bodies are the packages' work):

```rust
// ---- runtime (runtime_types.rs; WP0 defines, WP7 implements the loop) ----
pub struct JobId(pub u64);
pub enum Lane { Io, Probe, Ui }                    // an-io-{0..2}, an-probe, an-ui (§1.2)
pub enum Job {                                     // lane in brackets; result variant after →
    ReadFolders { explicit: Vec<PathBuf> },                                   // [Io] → Folders(FolderSnapshot)
    ReadClaudeJson { folder: AccountId, path: PathBuf },                      // [Io] → ClaudeJson(ClaudeJsonRead)
    ReadRegistry { sessions_dir: PathBuf, via_link: bool },                   // [Io] → Registry(RegistrySnapshot)
    SyncTranscript { session: SessionId, path: PathBuf, cursor: TranscriptCursor },   // [Io] → Transcript(TranscriptDelta)
    LoadChat { session: SessionId, path: PathBuf, before: Option<String> },   // [Io] → Chat(ChatPage)
    DesktopHosted { roots: Vec<PathBuf>, host_session_id: String },           // [Io] → Hosted(Option<IdentityId>)
    ReadDesktopCache { organization_uuid: String },                           // [Io] → Desktop(DesktopReading)
    Install { plans: Vec<InstallPlan> },                                      // [Io] → Installed(Vec<InstallOutcome>)
    Uninstall { record: HookInstallRecord, folders: Vec<RunFolder> },         // [Io] → Installed(Vec<InstallOutcome>)
    Persist { file: PersistFile, bytes: Vec<u8> },                            // [Io] → Persisted(Result<(), String>)
    Probe(ProbePlan),                                                         // [Probe] → Probe(ProbeResult)
    Versions { binaries: Vec<PathBuf>, bundled: Vec<PathBuf> },               // [Probe] → Versions(Vec<VersionSighting>)
    ConsoleInfo { pid: u32, started: SystemTime },                            // [Ui] → Console(ConsoleInfo)
    Classify { pid: u32 },                                                    // [Ui] → Host(HostApp)
    Focus { steps: Vec<FocusStep> },                                          // [Ui] → Focus(FocusOutcome)
    Type { session: SessionId, target: ConsoleTarget, text: String },         // [Ui] → Typed(TypeOutcome); mid-job Input::TypeCheckpoint
    Visibility,                                                               // [Ui] → Visible { any_terminal: bool, full_screen: bool }
}
pub enum JobResult { Folders(FolderSnapshot), ClaudeJson(ClaudeJsonRead), Registry(RegistrySnapshot),
    Transcript(TranscriptDelta), Chat(ChatPage), Hosted(Option<IdentityId>), Desktop(DesktopReading),
    Installed(Vec<InstallOutcome>), Persisted(Result<(), String>), Probe(ProbeResult),
    Versions(Vec<VersionSighting>), Console(ConsoleInfo), Host(HostApp), Focus(FocusOutcome),
    Typed(TypeOutcome), Visible { any_terminal: bool, full_screen: bool } }
pub enum PersistFile { Accounts, Review, Usage, Settings, HookInstall }
pub enum Input {                                   // everything an-core consumes, in arrival order
    Transport(TransportEvent),
    Call { call: Call, reply: crossbeam_channel::Sender<Result<serde_json::Value, CallError>> },
    JobDone { id: JobId, result: JobResult },
    TypeCheckpoint { job: JobId, reply: crossbeam_channel::Sender<bool> },   // fresh message_safety before Return (§4.8)
    Foreground(Foreground),
    SetSetting { key: String, value: serde_json::Value },                    // from an-cloud (and the glue): an-core is the only settings writer
    CloudState(CloudState),                                                  // an-cloud → projections
    Tick,                                                                    // the next deadline passed
    Stop { done: crossbeam_channel::Sender<()> },
}
pub struct ClaudeJsonRead { pub folder: AccountId, pub identity: Option<Identity>, pub cached_usage: Option<AccountUsage>,
    pub stamp: Option<(i128 /* mtime ns */, u64 /* size */)>, pub error: Option<String> }
pub struct VersionSighting { pub source: VersionSource, pub path: Option<PathBuf>, pub version: Option<String> }
pub enum VersionSource { Binary, BundledVsCode, BundledDesktop, Registry, StatusLine }   // §4.3
pub struct TranscriptCursor { pub offset: u64, pub size: u64, pub file: Option<FileIdentity> }
pub struct TranscriptDelta { pub session: SessionId, pub path: PathBuf, pub cursor: TranscriptCursor,
    pub reset: bool /* shrank or replaced: state rebuilt from 0 */, pub entries: Vec<TranscriptEntry> }
pub enum TranscriptEntry {                         // what the session pipeline needs from new complete lines (HS§5.8–5.11)
    Assistant { uuid: String, at: Option<SystemTime>, text: Option<String>, model: Option<String>,
        usage: Option<TokenUsage>, sidechain: bool, synthetic: bool },
    HumanPrompt { uuid: String, at: Option<SystemTime> },
    Interrupt { at: Option<SystemTime> },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, status: String /* success|error|interrupted */, task_id: Option<String> },
    Title { kind: String /* custom|ai|summary */, text: String },
    Clear,
}
pub struct TokenUsage { pub input: u64, pub output: u64, pub cache_creation: u64, pub cache_read: u64 }

// ---- ingress (WP1) ----
pub struct IngressConfig { pub pipe_name: String, pub tool_use_cache_ttl: Duration /* 60 min */,
    pub ignore_sdk_entrypoints: bool /* true */, pub max_connections: usize /* 512 */ }
impl HookIngress { pub fn new(cfg: IngressConfig) -> Self;
  pub fn on_transport(&mut self, ev: TransportEvent, now: SystemTime) -> Vec<IngressOut>;
  pub fn answer(&mut self, session: &SessionId, tool_use_id: &str, r: PermissionResponse) -> AnswerResult;
  pub fn release(&mut self, which: Release);                 // close held without answering
  pub fn pending(&self) -> Vec<(SessionId, String)>; }
pub enum IngressOut { Hook(HookEvent), StatusLine(StatusLineMessage), PermissionHeld(HeldPermission),
  PermissionFailed { session: SessionId, tool_use_id: String }, Control { conn: ConnId, op: ControlOp },
  TransportStatus(Result<String, String>) }
pub enum AnswerResult { Delivered, NotPending, PeerGone }
pub enum Release {                                 // which held PermissionRequests to close without an answer
    Request { session: SessionId, tool_use_id: String },     // PostToolUse/PostToolUseFailure/PermissionDenied for that id
    MainAgent(SessionId),                                     // Stop, main UserPromptSubmit, registry idle, interrupt
    Agent { session: SessionId, agent_id: String },           // SubagentStop
    Session(SessionId),                                       // SessionStart, SessionEnd, StopFailure, untracked
    All }                                                     // app quit, hub stop, sealed

// ---- sessions (WP5) ----
impl SessionStore { pub fn apply(&mut self, input: SessionInput, now: SystemTime) -> SessionEffects;
  pub fn views(&self) -> Vec<SessionView>; pub fn view(&self, id: &SessionId) -> Option<SessionView>;
  pub fn chat(&self, id: &SessionId) -> Option<ChatHistory>; }
pub struct IngestContext { pub attribution: Attribution, pub account: Option<AccountId>,
    pub trusted_pid: Option<u32> /* the frame's pid, checked against the hooks' pid (CLAUDE.md) */,
    pub pid_started: Option<SystemTime> }
pub enum SessionInput {
    Hook { event: HookEvent, ctx: IngestContext },
    Held(HeldPermission),
    PermissionFailed { session: SessionId, tool_use_id: String },        // the hook's pipe closed while held
    PermissionResolved { session: SessionId, tool_use_id: String, answer: Answer },
    StatusLine { message: StatusLineMessage, ctx: IngestContext },
    Registry(RegistrySnapshot),
    TranscriptSynced(TranscriptDelta),
    Hosted { session: SessionId, identity: Option<IdentityId> },
    Interrupt { session: SessionId, at: SystemTime },
    Review(ReviewAction),
    AccountsChanged(AccountsChanged),
    Tick }
pub enum ReviewAction { MarkReviewed { session: SessionId, at: SystemTime }, MarkViewed { session: SessionId, completed_at: SystemTime },
    MarkAll { sessions: Vec<SessionId>, at: SystemTime }, DismissFailure(SessionId), Reset, HooksTurnedOff }
pub struct SessionEffects { pub release: Vec<Release>, pub jobs: Vec<Job>, pub sightings: Vec<AccountSighting>,
  pub transitions: Vec<AttentionTransition>, pub changed: bool, pub persist_review: Option<Duration> }

// ---- accounts (WP3) ----
impl AccountRegistry { pub fn discover(&mut self, snap: FolderSnapshot, now: SystemTime) -> AccountsChanged;
  pub fn record(&mut self, s: AccountSighting, now: SystemTime) -> AccountsChanged; pub fn accounts(&self) -> Vec<Account>;
  pub fn folders(&self) -> Vec<RunFolder>; pub fn attribution(&self, folder: &AccountId, started: Option<SystemTime>) -> Attribution;
  pub fn apply_user(&mut self, a: AccountAction) -> Result<AccountsChanged, String>;
  pub fn folder_logins(&self) -> Option<BTreeMap<String, String>>;   // key() → login hash; None until folders were read (CL§5.2)
  pub fn backfill_folders(&self, allowed: &BTreeSet<IdentityId>) -> Vec<BackfillFolder>;   // CL§7.2 rules
}
pub struct AccountsChanged { pub rings: bool, pub folders: bool, pub identities: bool,
    pub new_run_folders: Vec<AccountId>, pub removed_folders: Vec<AccountId> }
pub fn read_folder_snapshot(roots: &Roots, explicit: &[PathBuf], files: &dyn SecureFiles, procs: &dyn Processes) -> FolderSnapshot;  // Job

// ---- usage (WP4) ----
pub enum RefreshReason { Launch, Interval, RingClick, Manual, NewAccount }
pub struct ClaudeBinary { pub program: PathBuf, pub prefix_args: Vec<OsString> /* node: [cli.js] */, pub version: Option<String>, pub shim: bool }
pub struct ProbePlan { pub identity: IdentityId, pub folder: AccountId, pub config_dir: PathBuf,
    pub config_dir_env: Option<String>, pub binary: ClaudeBinary, pub spec: CommandSpec /* argv, scrubbed env (§4.6), cwd */,
    pub reason: RefreshReason, pub planned_at: SystemTime }
pub enum ProbeOutcome { Reading(AccountUsage), RateLimited { retry_after: Option<Duration> }, SignedOut,
    Unavailable(String), Failed(String) }
pub struct ProbeResult { pub plan: ProbePlan, pub outcome: ProbeOutcome, pub started: SystemTime, pub finished: SystemTime,
    pub folder_identity_after: Option<IdentityId> /* the folder-changed-hands re-check (AU§10) */ }
pub enum RingReading { Reading { usage: AccountUsage, status: RingStatus, stale_after: SystemTime }, Waiting, SignInNeeded,
    Unavailable(String), Failed(String) }
impl UsageStore { pub fn ingest_status_line(&mut self, m: &StatusLineMessage, ctx: IngestContext, now: SystemTime) -> Option<UsageObservation>;
  pub fn accept_snapshot(&mut self, u: AccountUsage, now: SystemTime) -> Option<UsageObservation>;
  pub fn due_probe(&mut self, now: SystemTime) -> Option<ProbePlan>; pub fn finish_probe(&mut self, r: ProbeResult, now: SystemTime);
  pub fn is_probing(&self) -> bool;
  pub fn ring_reading(&self, id: &IdentityId, now: SystemTime) -> RingReading; pub fn state_file(&self) -> UsageStateFile;
  pub fn five_hour(&self, id: &IdentityId) -> Option<f64>; }
pub struct UsageObservation { pub identity: IdentityId, pub source: UsageSource, pub observed_at: SystemTime,
    pub windows: Vec<(String /* contract window id */, f64, Option<SystemTime>)> }   // CL§8
pub fn probe_folder(accounts: &[Account], folders: &[RunFolder], identity: &IdentityId) -> Option<RunFolder>;  // shared by the probe and summaries
pub fn locate_claude(roots: &Roots, settings_choice: Option<&Path>, env_path: &OsStr, exists: &dyn Fn(&Path) -> bool) -> Option<ClaudeBinary>;
pub fn scrubbed_env(base: &[(OsString, OsString)], config_dir_env: Option<&str>, binary_dir: &Path) -> Vec<(OsString, OsString)>;  // §4.6
pub fn run_probe(plan: &ProbePlan, runner: &dyn CommandRunner, clock: &dyn Clock) -> ProbeResult;           // Job
pub fn read_desktop_cache(roots: &Roots, organization_uuid: &str, now: SystemTime) -> DesktopReading;               // Job

// ---- hooks (WP2) ----
pub enum CommandForm { Exec { command: PathBuf, args: Vec<String> }, Text(String), NotPossible(String /* why, shown per folder */) }
pub enum StatusLineIntent { Wrap, UpdateCommand, Unwrap, LeaveAlone(String /* reason shown in Settings */), Nothing }
pub struct InstallPlan { pub folder: AccountId, pub settings_path: PathBuf /* resolved */, pub expected: Expect,
    pub existed: bool, pub hook_copy: Option<(PathBuf /* source */, PathBuf /* <cfg>\hooks\agentnotch-hook.exe */)>,
    pub form: CommandForm, pub events: Vec<String>, pub status_line: StatusLineIntent, pub remove_only: bool }
pub struct InstallOutcome { pub folder: AccountId, pub settings_path: PathBuf,
    pub result: Result<InstallChange, String>, pub backup: Option<PathBuf> }
pub enum InstallChange { Unchanged, Written, Removed, Aborted(String) /* e.g. "settings.json disappeared while writing" */ }
impl HookManager { pub fn plan(&self, accounts: &[Account], folders: &[RunFolder], settings: &ControlSettings,
      versions: &[VersionSighting], facts: &ClaudeCodeFacts) -> Vec<InstallPlan>;
  pub fn folder_status(&self, folder: &AccountId) -> FolderHookStatus; }
pub fn apply_install(plan: &InstallPlan, files: &dyn SecureFiles, clock: &dyn Clock) -> InstallOutcome;   // Job
pub fn uninstall_everything(record: &HookInstallRecord, files: &dyn SecureFiles, clock: &dyn Clock) -> Vec<InstallOutcome>;

// ---- control (WP6) ----
pub struct PanelState { pub open: bool, pub route: Option<String>, pub ring_id: Option<String>, pub reason: Option<String>,
    pub engaged: bool /* pointer inside or typing */, pub focused: bool /* confirmed foreground */, pub pinned: bool }
pub struct ToastContext { pub now: SystemTime, pub notify_needs_input: bool, pub notify_ready_for_review: bool,
    pub permission: NotifyPermission, pub suppressed: bool /* sealed, AGENTNOTCH_NO_NOTIFICATIONS, full screen */,
    pub looking_at: Option<bool>, pub account_label: Option<String>, pub multi_account: bool, pub title: String, pub project: String }
pub struct ReactionContext { pub now: SystemTime, pub auto_open: String /* never|needsInput|needsInputOrDone */,
    pub sound: bool, pub peek: bool, pub peek_seconds: u32, pub panel: PanelState, pub full_screen: bool,
    pub looking_at: Option<bool>, pub ring_shown: bool, pub notch_hidden: bool }
pub struct Reactions { pub chime: Option<Chime>, pub peek: Option<(RingId, u32)>, pub open_panel: Option<PanelRequest>,
    pub toasts: Vec<Toast>, pub withdraw: Vec<(String, String)> }
pub fn permission_response(req: &PendingRequest, a: &Answer) -> Result<PermissionResponse, String>;
pub fn message_safety(view: &SessionView, info: &ConsoleInfo, host: &HostApp) -> Result<(), String>;  // HS§8.1 rules, Windows mapping HS§8.2 + §4.8
pub fn focus_plan(view: &SessionView, host: &HostApp, info: &ConsoleInfo) -> Vec<FocusStep>;
pub fn looking_at(view: &SessionView, host: &HostApp, fg: &Foreground, sessions_in_window: usize) -> Option<bool>;
pub fn toast_for(tr: &AttentionTransition, ctx: &ToastContext) -> Option<Toast>;
pub fn reactions(burst: &[AttentionTransition], ctx: &ReactionContext) -> Reactions;   // chime, peek, auto-open
pub fn auto_close_deadline(panel: &PanelState, cause_resolved_at: Option<SystemTime>, opened_at: SystemTime,
    peek_seconds: u32) -> Option<SystemTime>;   // ClaudePanelPolicy.autoCloseDeadline

// ---- cloud (WP8): its own thread ----
pub struct CloudConfig { pub support: PathBuf, pub website: Option<String>, pub website_is_overridden: bool,
    pub app_version: String, pub device_name: String, pub device_id: String /* from settings; an-core mints it */,
    pub sync_enabled: bool, pub summaries_enabled: bool, pub summaries_allowed_by_default: bool, pub sealed: bool,
    pub system_users: Option<PathBuf>, pub home: PathBuf }
pub enum CloudCall { SignIn, CancelSignIn, SignOut, SetSync(bool), SetSummaries(bool), SyncNow }
pub struct CloudFolder { pub config_dir: PathBuf, pub config_dir_env: Option<String>, pub organization_uuid: Option<String>,
    pub corrected: bool /* a mirrored folder whose organisation is stale: never names the org */, pub kind: FolderKind,
    pub is_default: bool, pub mirrored_default: bool }
pub struct CloudAccount { pub identity_id: IdentityId, pub email: Option<String>, pub organization_name: Option<String>,
    pub plan: Option<String>, pub label: Option<String>, pub folders: Vec<CloudFolder> }   // accountKey = CloudKeys.accountKey(identity) from these (CLAUDE.md)
pub struct LiveBatch { pub attributed: Vec<LiveSessionObservation>, pub unsure: BTreeSet<String>,
    pub waiting: BTreeSet<String>, pub live_ids: BTreeSet<String>, pub at: SystemTime }   // built by the hub from SessionViews (CL§6.1, PLACEMENT_GRACE)
/// What the cloud thread reads from the rest of the engine. Implemented by the hub over an
/// immutable view that an-core republishes after every projection (Mutex<Arc<CloudView>>), so a
/// call never waits on an-core.
pub trait CloudDeps: Send + Sync {
    fn accounts(&self) -> Vec<CloudAccount>;                                  // visible, remembered, signed-in identities (CL§0.1)
    fn folder_logins(&self) -> Option<BTreeMap<String, String>>;              // None until the registry has read its folders
    fn backfill_folders(&self) -> Vec<BackfillFolder>;                        // CL§7.2
    fn five_hour(&self, identity: &IdentityId) -> Option<f64>;                // summaries skip ≥ 80 %
    fn summary_folder(&self, identity: &IdentityId) -> Option<RunFolder>;     // probe_folder's rules (CL§9.1)
    fn summary_folder_still_runs(&self, folder: &RunFolder, identity: &IdentityId) -> bool;
    fn is_launching_claude(&self) -> bool;                                    // a probe is running
    fn claude_binary(&self) -> Option<ClaudeBinary>;
    fn set_setting(&self, key: &str, value: serde_json::Value);              // → Input::SetSetting
}
impl CloudService { pub fn start(cfg: CloudConfig, deps: Arc<dyn CloudDeps>, platform: &Platform) -> CloudHandle; }
impl CloudHandle { pub fn observe_live(&self, o: LiveBatch); pub fn record_usage(&self, o: UsageObservation);
  pub fn call(&self, c: CloudCall) -> Result<(), String>;
  pub fn deep_link(&self, url: &str) -> DeepLinkOutcome; pub fn state(&self) -> CloudState; pub fn stop(&self); }
```

`accounts()` is read on demand (the cloud thread no longer receives a pushed account list). The
engine settings file has one writer, `an-core`; the cloud thread and the glue send
`Input::SetSetting`, and `an-core` republishes `SettingsSnapshot` and the cloud's config from it.
`ClaudeCodeFacts` is the parsed `tests/fixtures/claude-code-facts.json` compiled in with
`include_str!` (§6.2); `hooks::facts` exposes `EXEC_FORM_MIN` from it.

### 3.5 The hub facade (`agentnotch_engine::hub`, WP0 signatures, WP7 implementation)

```rust
pub struct HubConfig {
    pub roots: Roots,
    pub app_version: String,                 // tauri package_info().version (= VERSION)
    pub website: Option<String>,             // app-config.json, validated (CL§3.2)
    pub flags: DevFlags,                     // §4.13: no_install, no_notifications, usage_probe_on_dev_run,
                                             // extra_config_dirs (';'-separated), dump_state, web_url_override,
                                             // pipe_override, sealed (fails closed)
    pub hook_exe: PathBuf,                   // <install dir>\agentnotch-hook.exe
    pub pipe_name: String,                   // proto::pipe_name(sid) or the dev override
}
#[derive(Clone)] pub struct Hub { /* Arc inside */ }
impl Hub {
    pub fn new(cfg: HubConfig, platform: Platform) -> Hub;
    pub fn sealed(cfg: HubConfig, clock: Arc<dyn Clock>) -> Hub;       // fixtures; no IO beyond reading nothing
    pub fn start(&self) -> Result<(), String>;                         // spawns an-core, workers, an-cloud, transport
    pub fn stop(&self);                                                // saves stores now; releases held requests; kills children
    pub fn on_event(&self, sink: Box<dyn Fn(&HubEvent) + Send + Sync>);
    pub fn call(&self, call: Call) -> Result<serde_json::Value, CallError>;   // blocking; ≤ 25 s
    pub fn snapshot(&self) -> HubSnapshot;
    pub fn settings_snapshot(&self) -> SettingsSnapshot;
    pub fn launch_rings(&self) -> Vec<RingSummary>;                    // synchronous discovery at launch (AU§3.1)
    pub fn upstream_usage(&self) -> UpstreamUsage;                     // AppState.usage shape (§4.6)
    pub fn handle_deep_link(&self, url: &str) -> DeepLinkOutcome;      // auth-callback | open | review
    pub fn control_status(&self) -> agentnotch_proto::ControlStatus;
    pub fn doctor_report(&self, extra: &DoctorExtras) -> String;       // §4.14
    pub fn inspect_accounts(roots: &Roots, platform: &Platform) -> String;  // read-only, no hub needed
    pub fn uninstall_hooks(roots: &Roots, platform: &Platform) -> Result<String, String>;
}
pub struct DoctorExtras { pub exe: PathBuf, pub updates: String /* the doctor's `updates:` line */,
    pub deep_link: String, pub autostart: bool, pub shortcut_present: bool,
    pub providers: Vec<String> /* upstream probe lines */, pub running: Option<agentnotch_proto::ControlStatus> }
pub struct CallError { pub code: String /* "not_found"|"refused"|"invalid"|"sealed"|"busy"|"failed" */, pub message: String }
pub enum DeepLinkOutcome { SignInCompleted, SignInIgnored(String), Opened, Reviewed, Ignored(String) }
pub struct UpstreamUsage { pub status: String, pub windows: Vec<UpstreamWindow>, pub fetched_at: u64, pub note: String, pub backoff_until: u64 }
pub struct UpstreamWindow { pub id: String, pub label: String, pub used: f64, pub resets_at: Option<u64>, pub count: Option<i64>, pub derived: bool, pub group: Option<String> }
```

**`Call`** (`#[serde(tag = "method", content = "args", rename_all = "snake_case")]`). The JS
calls `invoke('an_call', {method, args})`; the glue deserialises `{method, args}` into `Call`.

| method | args | reply | Notes |
|---|---|---|---|
| `snapshot` | — | `HubSnapshot` | also pushed as `an:snapshot` |
| `settings` | — | `SettingsSnapshot` | also pushed as `an:settings` |
| `answer` | `{session_id, tool_use_id, answer: Answer}` | `{result:"delivered"\|"not_pending"\|"peer_gone"}` | `Answer` JSON: `{"allow":{"always":false}}`, `{"deny":{"reason":null}}`, `{"questions":{"answers":{…}}}`, `"approve_plan"`, `"keep_planning"` |
| `send_message` | `{session_id, text}` | `{outcome:"delivered"\|"refused"\|"typed_not_submitted"\|"failed", reason?}` | HS§8, two-phase (§4.8); refused while `typeReplies` is off |
| `message_route` | `{session_id}` | `{available:bool, reason?}` | shown by the chat's bottom bar; with `typeReplies` off: `available:false`, reason "Typing replies is off. Turn it on in Settings › Claude Code." |
| `focus` | `{session_id}` | `{outcome:"focused"\|"raised_only"\|"not_found"\|"failed", reason?}` | success marks reviewed (HS§9.1) |
| `mark_reviewed` | `{session_id, at_ms}` | `{}` | |
| `mark_viewed` | `{session_id, completed_at_ms}` | `{}` | |
| `mark_all_reviewed` | `{session_ids:[…], at_ms}` | `{}` | UI commits after its 5 s undo (UI§5.7) |
| `dismiss_failure` | `{session_id}` | `{}` | |
| `reset_review_queue` | — | `{}` | Advanced |
| `chat_open` / `chat_close` | `{session_id}` | `{}` | opening marks reviewed (HS§5.11); chat pushed as `an:chat` (a reset, then patches, §3.6) |
| `chat_more` | `{session_id, before_id}` | `{}` | next page (150 items) |
| `chat_image` | `{session_id, image_id}` | `{data_url}` | one image, ≤ 2 MiB, fetched when it scrolls into view |
| `refresh_usage` | `{ring_id?:string, reason:"ring_click"\|"manual"}` | `{coming:bool}` | AU§9.8 |
| `hook_consent` | `{grant:bool}` | `{}` | "Turn on" / "Not now" |
| `hooks_enabled` | `{on}` | `{}` | off uninstalls everywhere, restores status lines |
| `status_line_enabled` | `{on}` | `{}` | |
| `hooks_reinstall` | `{account_id?}` | `{}` | |
| `remove_codenotch_hooks` | `{folder}` | `{removed:u32}` | the official app's entries, explicit only |
| `acknowledge_scope` | — | `{}` | |
| `account` | `{action: AccountAction}` | `{}` or `{error}` | `rename{id,label?}`, `track{id,on}`, `ring_shown{ring_id,on}`, `forget{id}`, `add_folder{path}`, `create{name}` (returns `{created, command}`), `suggestion_dismiss{path}`, `suggestion_add{path}` |
| `set_setting` | `{key, value}` | `{}` | keys = §4.12 table |
| `choose_claude_binary` | `{path?:string}` | `{version?}` | `null` = find automatically |
| `cloud` | `{action:"sign_in"\|"cancel_sign_in"\|"sign_out"\|"set_sync"\|"set_summaries"\|"sync_now", on?:bool}` | `{}` | all no-ops when sealed |
| `cloud_url` | `{target:"dashboard"\|"pools"\|"settings"}` | `{url}` | the UI asks the glue to open it |
| `session_state_text` | — | `{text}` | Advanced "Copy" (HS/UI§7.8) |
| `launch_command` | `{account_id}` | `{command}` | PowerShell form (§4.2) |
| `reveal_target` | `{kind:"config_dir"\|"session_cwd"\|"backup", id}` | `{path}` | the engine resolves a folder it knows; the page never supplies a path |
| `panel_state` | `PanelState` (§3.4) | `{}` | **from the glue only** (refused from pages), on every open/close/route/focus/pin/engagement change; feeds `ReactionContext` and the auto-close tick |
| `hotkey_status` | `{ok, message?}` | `{}` | from the glue only, after each registration attempt |

The auto-close rule runs in the engine (`control::auto_close_deadline` on the `PanelState` it
was told about); when it fires the engine emits `HubEvent::PanelClose`. "Pin" is the setting
`panelPinned` (`set_setting`); the glue applies the window side of it.

Glue-level methods (handled in `calls.rs`, never reach the engine): `panel_toggle {ring_id?,
rect:[x,y,w,h] (physical px, client of the calling window), reason}`, `panel_open {route,
reason}`, `panel_close`, `panel_route {route}`, `panel_report_size {w,h}` (CSS px),
`panel_take_focus`, `panel_engaged {on}` (pointer inside the panel or a text field focused),
`open_settings {tab?}`, `open_url {url}` (only `https://` or the cloud URLs),
`open_notification_settings` (`ms-settings:notifications`, fixed; no argument), `copy_text
{text}`, `reveal {kind, id}` (asks the engine's `reveal_target`, then
`SHParseDisplayName` + `SHOpenFolderAndSelectItems`; never a command line), `pick_folder`
(native dialog via `agentnotch-win`), `log {msg}`.

**`HubEvent`** (engine → glue; glue → Tauri):

| Variant | Tauri event / effect | Target |
|---|---|---|
| `Snapshot(HubSnapshot)` | `an:snapshot` (coalesced ≥ 50 ms, only on change) | `notch`, `agentnotch-panel` |
| `Settings(SettingsSnapshot)` | `an:settings` (only while the settings window exists) | `settings` |
| `Cloud(CloudState)` | `an:cloud` | `settings` |
| `Chat(ChatUpdate)` | `an:chat` (a reset or a patch, §3.6) | `agentnotch-panel` |
| `Panel(PanelRequest)` | glue opens/places the panel, then `an:panel` | `agentnotch-panel` |
| `PanelClose { reason }` | glue hides the panel (auto-close, a successful jump unless pinned) | — |
| (glue, not the engine) | `an:panel_focus {focused}` after each focus confirmation (§2.4 `panel.rs`) | `agentnotch-panel` |
| `Peek { ring_id, seconds }` | `an:peek` | `notch` |
| `UpstreamUsage(UpstreamUsage)` | `AppState.usage` replaced, upstream `usage` emitted to all | all |
| `TrayBadge(u32)` | tray icon dot on/off | — |
| (no variant) toasts, withdrawals, chimes | the engine calls `Notifier` / `Sounds` itself (they are `Platform` services); the glue is not involved | — |
| `Notice(String)` | `an:notice` | `notch` |
| `Log(String)` | upstream `applog("an: …")` (never prompts, inputs, replies, tokens) | — |
| `Quit` | graceful `app.exit(0)` (control op `quit`) | — |

### 3.6 Snapshot types (UI contract; fixtures in `agentnotch-engine/tests/ui-contract/`)

```rust
pub struct HubSnapshot { pub version: u32 /*1*/, pub generated_at_ms: u64, pub sealed: bool,
    pub rings: Vec<RingSummary>, pub sessions: Vec<SessionRow>, pub sections: Vec<SectionInfo>,
    pub totals: Counts, pub resting_marks: RestingMarks, pub tray_badge: u32,
    pub setup: SetupState, pub ui: UiSettings, pub accounts_multi: bool }
pub struct RingSummary { pub ring_id: String, pub label: String, pub monogram: String, pub color_index: u8,
    pub shown: bool, pub is_default: bool, pub usage: RingUsage, pub activity: RingActivity /* working|waiting|success|idle */,
    pub success_settles_at_ms: Option<u64>, pub badges: Badges, pub counts: Counts, pub a11y: String }
pub struct RingUsage { pub status: String /* ok|stale|waiting|sign_in_needed|unavailable|failed */,
    pub windows: Vec<UpstreamWindow>,   // ids: session, weekly_all, weekly_<slug>, extra_usage (AU§8.5); used 0..1; resets_at ms
    pub fetched_at_ms: u64, pub note: String,
    pub stale: bool /* the engine's rule (AU§8.4: 1 h with probes off, else max(15 min, 1.5 × interval), never for an
                       exhausted window); decorateCell sets the cell's `stale` class from it, overriding upstream's 15-min rule */ }
pub struct Badges { pub needs_you: u32, pub review: u32 }          // label: "" | "1".."9" | "9+" (JS)
pub struct Counts { pub needs_you: u32, pub failed: u32, pub review: u32, pub working: u32, pub idle: u32 }
pub struct RestingMarks { pub needs_you: bool, pub review: bool, pub working: bool, pub needs_you_key: u32 }
pub struct SessionRow { pub session_id: String, pub ring_id: Option<String>, pub account_label: Option<String>,
    pub account_color: Option<u8>, pub title: String, pub project: Option<String>, pub bucket: String,
    pub failed: bool, pub state_word: String, pub since_ms: u64, pub detail: RowDetail,
    pub tasks: Option<TaskProgress>, pub context_pct: Option<f64>, pub background_count: u32,
    pub pending: Option<PendingRequestView>, pub focus_label: Option<String> /* "Show terminal"|"Show in editor" */,
    pub can_message: bool, pub reviewable: bool, pub a11y: String,
    pub card: CardRow }                 // the notch hover card's row (UI§3.5), a port of ClaudeHostProjections.activityRow
pub struct CardRow { pub name: String /* "<hostApp> · <project>" or the title */, pub detail: Option<String> /* "3/7 … · ctx 42%" */,
    pub state: String /* needs_you|failed|review|working|idle */, pub waiting_for: Option<String>, pub since_ms: u64 }
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowDetail { Permission { tool: String, request: String, waiting_in_terminal: bool },
    Question { text: String }, Plan, Dialog { text: String }, Failed { text: String },
    Working { text: String, secondary: bool }, Review { text: String }, Idle { text: String } }   // SessionRowContent (UI§5.4)
pub struct PendingRequestView { pub tool_use_id: String, pub kind: String /* permission|question|plan */,
    pub tool_name: String, pub received_at_ms: u64, pub request: String, pub needs_review: bool,
    pub always: Option<String>, pub inline_always: bool, pub questions: Option<Vec<Question>>,
    pub single_tap: bool, pub plan_markdown: Option<String>, pub diff: Option<Vec<DiffLine>> }
pub struct SectionInfo { pub bucket: String, pub title: String, pub count: u32, pub fold_by_default: bool }
pub struct SetupState { pub hook_consent: Option<bool>, pub needs_hook_consent: bool,
    pub consent_files: Vec<ConsentFile>, pub codenotch_hooks_folders: Vec<String>,
    pub new_install_folders: Vec<String>, pub transport_error: Option<String>, pub control_off: bool,
    pub missing_hooks_accounts: Vec<String>, pub install_disabled: bool }
pub struct UiSettings { pub panel_open_mode: String, pub hold_open: String, pub ring_badges: bool,
    pub resting_marks: bool, pub ring_click: String, pub hover_click: String, pub hotkey: String,
    pub panel_pinned: bool, pub peek: bool, pub peek_seconds: u32, pub type_replies: bool }
/// Chat goes over IPC as a reset (the whole page) when a chat opens or pages, then as patches:
/// only items that are new or whose content hash changed, the ids removed, and the page's id
/// order. Images travel by reference (`chat_image`), so a transcript change costs kilobytes.
pub struct ChatUpdate { pub session_id: String, pub revision: u64, pub reset: bool,
    pub items: Vec<ChatItem>, pub removed: Vec<String>, pub order: Vec<String>, pub has_earlier: u32,
    pub working: Option<String>, pub ended: bool, pub loading: bool }
pub struct ChatItem { pub id: String, #[serde(flatten)] pub body: ChatBody }
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatBody { User { text: String }, Assistant { text: String }, Thinking { text: String },
    Tool { name: String, summary: String, status: String, input: serde_json::Value, result: Option<ToolResultView>, subagent: Option<SubagentView> },
    Image { media_type: String, image_id: String, bytes: u64 }, Interrupted }
pub struct PanelRequest { pub route: String /* "sessions" | "session:<id>" */, pub ring_id: Option<String>,
    pub highlight: Option<String>, pub reason: String /* ring_click|hover_row|peek_click|notification|hotkey|settings|auto */ }
pub struct SettingsSnapshot { pub accounts: Vec<AccountRow>, pub suggestions: Vec<Suggestion>, pub unsigned_folders: Vec<String>,
    pub hooks: HooksSection, pub usage: UsageSection, pub cloud: CloudState, pub attention: UiSettings,
    pub notifications: NotificationsSection, pub advanced: AdvancedSection, pub sealed: bool, pub setup: SetupState }
```
`AccountRow`, `HooksSection`, `UsageSection`, `NotificationsSection`, `AdvancedSection` carry
the fields UI§7.2–7.8 render (folder rows with role and hook state, usage line text, pipe name,
Claude Code version and path, "Last change …" notice, notification permission). WP0 writes the
Rust structs and one JSON fixture per snapshot type; the engine test
`ui_contract_fixtures_roundtrip` deserialises and re-serialises every fixture (key sets equal);
the node tests render from the same fixtures.

### 3.7 Tauri surface summary

- **Command:** one, `an_call(method: String, args: Value) -> Result<Value, {code, message}>`
  (app command: not ACL-gated in Tauri 2; the glue itself gates by window label):
  `agentnotch-panel` → everything except the glue-only `panel_state` and `hotkey_status`;
  `notch` → `snapshot`, `refresh_usage`, `focus`, `mark_reviewed`, `panel_*`, `log`;
  `settings` → everything except `answer`, `send_message` and the glue-only calls; `dropzones`
  → nothing.
- **Events:** `an:snapshot`, `an:settings`, `an:cloud`, `an:chat`, `an:panel`, `an:panel_focus`,
  `an:peek`, `an:notice` (payloads §3.5/§3.6). Upstream's `usage` event carries the fork's
  projection.
- **CSP:** one policy for every page (seam WCSP, §2.5); `panel.html` also carries it as a meta
  tag.
- **Windows:** `notch`, `settings`, `dropzones` (upstream), `agentnotch-panel` (fork).
- **Capability** `capabilities/agentnotch.json`:
  `{"identifier":"agentnotch","description":"Agent Notch's sessions panel: events and its own window","windows":["agentnotch-panel"],"permissions":["core:default"]}`.

---

## 4. Feature-by-feature porting spec

Port the Mac behaviour as the reports describe it; below are only the Windows mechanisms,
differences and degradations. Mac test vectors are the oracles (HS§13, AU§16, CL§12): port the
suites listed there into Rust tests under the owning package.

### 4.1 Paths (written complete by WP0, then maintained by WP3; AU§1)
- `core::paths::normalize` accepts `/` and `\`, strips `\\?\` (`\\?\UNC\` → `\\`), resolves `.`
  and `..` lexically, upper-cases the drive letter, drops a trailing separator except at a root,
  expands `~`, `~\`, `~/` against `Roots.home`. `key()` = lower-cased form with `\` for map keys
  and hashes; `display()` keeps the case. `claude-dir-` ring ids hash `key()`.
- `configDir(fromTranscriptPath)` compares `projects` case-insensitively. Slug of
  `C:\Users\me\proj` is `C--Users-me-proj`; prefer the hook's `transcript_path`.
- Links: `SecureFiles::is_reparse` (symlink or junction), `canonical` for identity; "never
  through a link" rules check each component with `is_reparse`.
- `AGENTNOTCH_EXTRA_CONFIG_DIRS` splits on `;`.
- The engine keeps Unix-path behaviour too (tests on macOS feed `/Users/…` vectors): the path
  module is a pure function of a `PathStyle { Windows, Posix }` chosen from `cfg!(windows)`
  by default and passed explicitly in tests, so both styles are tested on every OS.

### 4.2 Accounts, identities, rings (WP3; AU§2–7)
- Port the classifier, discovery, identity grouping ("the email decides"), naming, monograms,
  ring ids, `accounts.json` v2 and the default-folder timeline unchanged. Default folder
  `%USERPROFILE%\.claude`, its identity file `%USERPROFILE%\.claude.json`; other folders
  `<dir>\.claude.json`; `CLAUDE_CONFIG_DIR` set → `<dir>\.claude.json` even for `~\.claude`.
- `.claude.json`: read fully into memory with default share modes (never memory-mapped: a
  mapping blocks Claude Code's replace, AU§4.4); JSON field scanner port; cache by (mtime, size);
  last good parse survives a torn read.
- `hasLiveSession`: `sessions\<pid>.json` exists and `Processes::liveness(pid) == Alive`, never
  opening the file.
- **Claude Parallel Profiles is inert on native Windows** (AU§0.2): keep its classifier branches
  (pure, tested) but expect only run folders; `~\.claude-windows`, manifests and stores never
  appear. WSL sessions are not tracked (Settings footnote "Sessions inside WSL aren't tracked
  yet."). [DEGRADE]
- Launch command (AU§2.1): PowerShell form `$env:CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work'; claude`
  (single quotes doubled inside); `claude` for the default folder. "Copy launch command" copies
  it; sign-in guidance "Run it once, then /login."
- Create account: `%USERPROFILE%\.claude-<slug>` with default ACL.
- Process attribution (AU§14.1): `Processes::config_dir_env(pid)` = PEB read (same-user only,
  x64 offsets ProcessParameters +0x20, Environment +0x80, EnvironmentSize +0x3F0; UTF-16 block;
  case-insensitive name match; buffer zeroed after use). The whole block is read: `EnvironmentSize`
  bytes, capped at 1 MiB (a larger block → `Unreadable`), in one `ReadProcessMemory` (retried
  once). `Unset` is returned **only** when the complete block was read and ends with the
  double-NUL terminator without naming the variable; a short or partial read
  (`ERROR_PARTIAL_COPY`, a size that changed between the two reads, a missing terminator) is
  `Unreadable`, never `Unset` (which would mean `~\.claude` and pick the wrong account). Used
  only when a `sessions` folder is reached through a link (rare on Windows); otherwise a folder's
  entries belong to it. Elevated or other-user targets → `Unreadable`. **[ASSUMPTION]** offsets
  hold on x64 Windows 10/11: proven by `agentnotch-win/tests/win_process.rs` (child with a known
  `CLAUDE_CONFIG_DIR` → `Set`; without → `Unset`; mixed case `Claude_Config_Dir` → `Set`; the
  variable placed after 100 KiB of other variables → `Set`). ARM64 hosts running this x64 build:
  `Unreadable` (degrade; see §9).
- Spellings of `CLAUDE_CONFIG_DIR`: a folder's login on Windows is its own
  `.credentials.json` (a case-insensitive path), not a Keychain item per spelling as on the Mac
  (AU§0.3), so `seen_config_dir_envs` collapses by `key()`, the "login conflict" chip never shows
  and the spelling-switching identity refresh never runs. The default folder keeps the Mac's
  set-versus-unset rule (`~\.claude.json` when unset, `~\.claude\.claude.json` when set to it).
- Process start time: `GetProcessTimes` creation FILETIME → `SystemTime`. Every pid use pairs
  with it (Windows reuses pids quickly).

### 4.3 Hook installation, consent, uninstall (WP2; HS§3)
- **Consent**: unchanged (Turn on / Not now; scope 2; `hooksEnabled`; `statusLineIntegration`).
  No legacy-app blocks exist on Windows. The consent card lists the exact files
  (`%USERPROFILE%\.claude\settings.json (me@work.com)`).
- **Targets**: tracked run folders (HS§3.1). One write per physical settings.json (identity via
  `canonical` + `FileIdentity`).
- **Hook copy**: `<install dir>\agentnotch-hook.exe` → `<cfg>\hooks\agentnotch-hook.exe` when
  size or SHA-256 differ, in this order so there is never a moment without an exe: (1) stage
  `agentnotch-hook.new-<yyyyMMddHHmmss>.exe` beside it (`CREATE_NEW`, flushed, SHA-256
  verified); (2) one atomic rename of the stage over the current copy (works when nothing runs
  it); (3) if that is refused because a hook is running the copy, rename the current copy to
  `agentnotch-hook.old-<ts>.exe` (a running image may be renamed) and immediately rename the stage
  into place; a spawn that falls in the microseconds between the two renames fails to start,
  which Claude Code treats as non-blocking. Later passes and `uninstall-hooks` delete
  `*.old-*.exe` and stale `*.new-*.exe`, ignoring failures. The copy is written before
  settings.json.
- **Command forms** (`hooks::commands`). Claude Code runs a string command through
  `e.shell ?? <its default shell>` (winmap/hookexec.txt: `bt=e.shell??R1()`), which may be Git
  Bash or PowerShell, so a string command must parse in both:
  - **string form (default)**: `C:/Users/me/.claude/hooks/agentnotch-hook.exe hook`, i.e. the
    path unquoted with forward slashes, used only when every character of the path is a Unicode
    letter or digit or one of `_ . - / :`, or `~` anywhere but first (mid-word `~` is literal in
    both shells; 8.3 names contain it). Else the 8.3 short path (`GetShortPathNameW`, forward
    slashes: `C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE hook` for a user folder with a space)
    when that passes the same test (8.3 names may be disabled on the volume; then it does not).
    Else there is no string form. No quotes, no `[ -f … ] || exit 0` guard (not
    PowerShell syntax): a missing exe exits 127 (bash) or 1 (PowerShell), never 2, which Claude
    Code only logs;
  - **exec form**: `{"type":"command","command":"C:\\Users\\me\\.claude\\hooks\\agentnotch-hook.exe","args":["hook","--exec"]}`,
    written only when **every** Claude Code version the app has seen is known and ≥
    `EXEC_FORM_MIN`. Versions come from: `sessions\<pid>.json` `version`, status-line `version`,
    `claude --version` of every located binary (§4.6's candidates; CREATE_NO_WINDOW, 5 s, never in
    tests), and the **folder names** of bundled copies the app never runs:
    `%USERPROFILE%\.vscode\extensions\anthropic.claude-code-<ver>-*`,
    `.vscode-insiders\extensions`, `.cursor\extensions`, `.windsurf\extensions` (same pattern),
    and `%APPDATA%\Claude\claude-code\<ver>` (Claude Desktop's). A bundled folder whose version
    cannot be parsed counts as unknown (→ no exec form). `EXEC_FORM_MIN` comes from the committed
    facts file (§6.2); while that file does not establish it, `EXEC_FORM_MIN` is `None` and exec
    form is never written. Before 2.1.101 an unknown key made Claude Code ignore the whole
    settings file (HS§3.5), deny rules included, which is why a single old or unknown copy keeps
    string form;
  - neither possible → the folder is not hooked and shows "Can't be hooked here: its path needs
    Claude Code <EXEC_FORM_MIN> or later everywhere on this PC" (or "…has characters a hook
    command can't carry" when exec form is not established at all). Exec-form entries an
    earlier pass wrote there are **taken out** (ours only; the status line, the hook copy and
    everything else stay; refused like any write when settings.json is unreadable): a Claude
    Code older than `EXEC_FORM_MIN` does not skip them but ignores `args` and runs `command`
    through Git Bash, where an unquoted path with `(`, `)`, `'`, `"` or a backtick is a syntax
    error that exits 2, which blocks every tool call, prompt and Stop. Such a plan runs after
    the pass's other installs, so a settings.json it shares with a hookable folder (a link)
    keeps that folder's entries;
  - a settings.json another program holds without read sharing (a backup or sync tool) is read
    again on the rename's retry schedule; one still held is reported as "in use by another
    program" (`FolderHookStatus.settings_in_use`), never as invalid JSON;
  - PermissionRequest adds `"timeout":86400`; events per version exactly as HS§3.5;
  - status line (a string, always): the same unquoted / 8.3 rule with `statusline`; when neither
    works the status line is not taken over ("Live status line isn't available for this folder";
    usage still comes from the probe and `.claude.json`). **[ASSUMPTION]** which shell runs
    `statusLine` on Windows: settled by the facts job (§6.2, R2); both candidates parse the form.
- **Recogniser**: ours = exec form whose `command` basename is `agentnotch-hook.exe`
  (case-insensitive) with `args[0]` ∈ {`hook`,`statusline`} and nothing after it but `--exec`,
  or a string whose last command runs a path ending `agentnotch-hook.exe` (quoted or not, `/` or
  `\`, `C:` or `/c/`, long or 8.3 — an 8.3 path is compared after `GetLongPathNameW`) with that
  argument. Substring mentions never match. The official app's entries (`codenotch-hook`
  substring, upstream `is_ours`) are reported (`setup.codenotch_hooks_folders`) and removed only
  by `remove_codenotch_hooks`.
- **settings.json safety** (HS§3.4): refusal rules, splicing that keeps other keys' bytes, JSON
  equivalence (a reformat never writes), backups (timestamped ×5 + original forever,
  owner-only DACL, skipped when identical). **Keeps CRLF line endings and a UTF-8 BOM** when the
  file has them (the splicer detects the newline style and the BOM; a file mixing styles is
  written with the majority style only inside our splice). The write itself:
  1. Resolve the path once (`GetFinalPathNameByHandleW` on a handle opened through links); a
     symlinked settings.json is written through to its target, as on the Mac; a link to a
     missing target is refused (`settingsLinkBroken`); a read-only target is refused ("settings.json
     is read-only"). Everything below uses the resolved path, so the stage sits beside the real
     file, never beside a link.
  2. Read bytes + `FileIdentity`; remember whether the file **existed**.
  3. Plan (splice). Equivalent → nothing written.
  4. Backups as above.
  5. `SecureFiles::write_atomic(resolved, bytes, KeepTargetSecurity, expect)` with
     `expect = Same(identity)` for an existing file and `Absent` for a new one: stage beside it,
     copy the target's DACL, protection flag and attributes onto the stage, flush, re-check the
     identity, then **one atomic rename** (`FileRenameInfoEx`, `REPLACE_IF_EXISTS |
     POSIX_SEMANTICS`; `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` where unsupported).
     `ReplaceFileW` is not used: without a backup name its failure 1176 can leave the target
     deleted, and even when it succeeds the target is briefly absent, which a Claude Code
     reloading at that moment would read as "no settings".
  6. `Changed` → re-plan from a fresh read (≤ 5 times, then "kept changing"). `Vanished` (the pass
     saw a file and it is gone now) → **abort this file's pass**, never re-plan from `{}` ("A blank
     or missing file counts as {}" applies only when the pass's first read found no file); the
     folder shows "settings.json disappeared while Agent Notch was writing it; nothing was
     changed". Sharing or access errors → ≤ 5 retries over ~500 ms, then give up with the target
     untouched.
  Invariant (fault-injected in `win_install.rs`): at every instant the settings.json exists with
  either its old bytes or its new bytes.
- **Status line takeover** (HS§3.6), with Windows limits. The fork's wrapper re-runs the user's
  previous command itself, and Claude Code's own treatment of a status line command on Windows
  is not decoded yet (its bash-form transform `yun`, and whether `statusLine` honours `shell`:
  facts job, §6.2). So the takeover is conservative:
  - no status line at all → ours is installed and chains nothing (no shell question arises);
  - someone else's status line is wrapped **only** when all hold: Git Bash is found (the same
    discovery as Claude Code: `CLAUDE_CODE_GIT_BASH_PATH`, `%ProgramFiles%\Git\bin\bash.exe`,
    `%ProgramFiles(x86)%\Git\bin\bash.exe`); the object is `{"type":"command", …}` with no
    `shell` key; the command contains no `\`, no `.ps1`, no `$env:`, and does not name
    `powershell`, `pwsh`, `cmd.exe` or `cmd /c` (case-insensitive); it is not ours (loop guard
    below). Otherwise it is **left alone** and Settings says why ("Status line left alone: its
    command uses Windows paths" / "…needs PowerShell" / "…Git Bash isn't installed"); usage still
    comes from the probe and the cache;
  - the wrapper runs the previous command as `bash.exe -c "<cmd>"` (Git Bash re-discovered at run
    time; missing → no output, exit 0), with `AGENTNOTCH_STATUSLINE_DEPTH=1` in its environment,
    in a Job object (kill on close, 30 s → terminate, no output, exit 0); stdout and exit code
    pass through;
  - **loop guard**: a previous command is refused (chain nothing) when it contains, case-
    insensitively, `agentnotch-hook` together with `statusline`, or `agentnotch-statusline`, or
    one of the former names (`superpowered-codenotch-statusline`, `superpowered-notch-statusline`);
    and the wrapper chains nothing when `AGENTNOTCH_STATUSLINE_DEPTH` is already set in its own
    environment (a copy of itself reached through any spelling), so processes can never pile up.
- **Passes** (HS§3.1): at start, on account changes (1 s debounce), on a lower Claude Code
  version, every 10 min. Version sources as in "Command forms".
- **Turning off / uninstall**: remove our entries from every folder in `hook-install.json` and
  every tracked folder, restore status lines, delete `previous.json`, the hook copy (renamed
  aside if running), `*.old-*.exe` and `*.new-*.exe`. CLI `uninstall-hooks` does the same without
  a running hub (record + discovery) and prints a summary. The NSIS uninstaller runs it only when
  the user ticked "Delete the application data" or passed `/REMOVEHOOKS` (§6.5); otherwise
  entries stay and exit 0 at once while the app is gone (Settings and the README say: turn
  Claude Code control off before uninstalling to remove them). CLI `install-hooks` refuses (exit
  2, "Turn on Claude Code control in Settings › Claude Code first") unless consent is granted.
- **`--no-install`** as on the Mac.

### 4.4 Hook ingress (WP1; HS§4)
§1.4 is the spec. Engine side = `ingress` (decode, filter, ToolUseIdCache, held map, answer,
release, peer-gone). Windows side = `agentnotch-win::pipe_server` + the hook exe. Status-line
messages go to the usage store and the session store (context %, model, title, cost).
`transport_error` feeds panel banner (d) with Windows copy: "Not receiving hook events" /
"<error> Sessions still update from Claude Code's session files, without approvals."
Integrity levels: hooks from an elevated terminal reach a normal app, and a normal hook reaches
an elevated app (§1.4's checks read the pipe and the impersonation token, never another
process's token). What elevation does limit is outside the pipe: typing into and UI Automation
of an elevated console or Windows Terminal from a normal app (UIPI), and reading an elevated
process's environment. The doctor prints `elevated: app=<yes|no>` and the count of sessions whose
Claude runs elevated (`Processes::elevated`, `None` counted as "unknown").

### 4.5 Sessions, states, background work, review queue (WP5; HS§5)
- Port `HookEventPipeline` ordering, phases, `SessionAttention.derive`, `processHookEvent`,
  `applyPhase`, `applyLifecycle`, `applyNeedsInput`, turn completion (4 s / 90 s / registry),
  `BackgroundWork` (awaited types, 10 s grace, 30 min cap), quiet completions, registry
  reconciliation, `inferCompletion`, task progress (TaskCreate/TaskUpdate/TodoWrite +
  transcript reconstruction), context % (status line first, estimator else), titles, the
  3 s periodic check, the review store (v2, atomic, owner-only, 1 s debounce for marks, 30 s
  heartbeat, 7-day prune). All pure: `SessionStore::apply(input, now)`.
- Windows specifics: transcripts opened with default Rust share modes (read/write/delete);
  incremental 8 MiB complete-line reads; the interrupt watcher and open chats poll the file
  size every 250 ms (no `notify` dependency; NTFS metadata can lag for files held open, so size
  from an open handle, `File::metadata`, is used); registry live check = `GetProcessTimes` vs
  `procStart` (UTC, format `EEE MMM d HH:mm:ss yyyy`) within 2 s, else creation ≤
  `startedAt` + 5 s. **[ASSUMPTION]** Claude Code writes `sessions\<pid>.json` on Windows (same
  JS bundle); without it sessions come from hooks alone and registry-driven rules idle (the
  engine already copes, HS§10.4).
- Desktop-hosted sessions (AU§13): records under each `Roots.claude_desktop` root
  `claude-code-sessions\<acct>\<org>\<hostSessionId>.json`, `is_reparse` on every component,
  never opened, never when sealed. **[ASSUMPTION]** path on Windows; a miss means "unsure", never
  a wrong account.
- Pid-less (status-line-only, or hook frames with `"pid": null`, §1.4) sessions dropped after
  15 min of silence; pid liveness via `Processes`.
- Placement for the cloud: a session attributed `Known(None)` (folder not grouped yet), or
  Desktop-hosted without a host session id and registry status, is "waiting" for at most
  `PLACEMENT_GRACE` (30 s, measured from `attribution_since`), then "unsure" (CL§6.1).

### 4.6 Usage (WP4; AU§8–12)
- Parser, ring window ids, `UsageStore` merge (`supersedes`, `isSmallDrop`, `isMoreCurrent`,
  `advance`), status-line keys `pid:<pid>@<start s>`, 7-day retention, `usage-state.json`,
  scheduling, backoff, `refresh(reason)`: unchanged.
- **Probe** (AU§10): binary order = Settings choice, remembered path,
  `%USERPROFILE%\.local\bin\claude.exe`, `%USERPROFILE%\.bun\bin\claude.exe`,
  `%USERPROFILE%\.volta\bin\claude.exe`, `%APPDATA%\npm\claude.cmd`, `%LOCALAPPDATA%\pnpm\claude.cmd`,
  `PATH` (`claude.exe`, then `claude.cmd`); never paths containing `\AnthropicClaude\`,
  `\Claude\claude-code\` or `\WindowsApps\` (Desktop-owned). A `.cmd` shim is resolved to
  `node.exe` + `<shim dir>\node_modules\@anthropic-ai\claude-code\cli.js` when that file exists;
  else the probe runs the `.cmd` through Rust std (which escapes or refuses batch arguments;
  a refusal is reported as "Claude Code's npm shim can't take the probe's arguments; choose
  claude.exe in Settings"). Environment: `usage::scrubbed_env`, **one list for the probe and for
  summaries**: start from the app's, remove (case-insensitively) `CLAUDECODE`, `CLAUDE_PID`,
  `CLAUDE_EFFORT`, `AI_AGENT`, `CLAUDE_CONFIG_DIR`, `CLAUDE_SECURESTORAGE_CONFIG_DIR` (else the run
  could use another folder's login and spend another account's usage), `CLAUDE_CODE_*`,
  `CLAUDE_AGENT_SDK_*`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` (the Mac summarizer's list,
  CL§9.3, plus the secure-storage variable; a superset of the Mac probe's); then set
  `CLAUDE_CONFIG_DIR` to the folder's raw `config_dir_env` for a non-default folder (unset for
  `~\.claude`); prepend the binary's folder to `PATH`. Test vector `usage_env_scrub.rs`: a base
  environment holding every removed name in mixed case, plus `Path` and `SystemRoot`, gives
  exactly `Path` (prefixed), `SystemRoot` and the folder's `CLAUDE_CONFIG_DIR`.
  cwd `<support>\usage-probe`. Strip trailing `\r`. Timeout 20 s; after stdin closes, 2 s grace
  then `kill_tree`. The folder-changed-hands re-check and seeded-answer dating are unchanged.
  **[ASSUMPTION]** `get_usage` exists in the Windows build (same bundle) — proven only by a
  user's first probe; failures surface as the ring's honest status.
- **Desktop cache** (AU§12): roots `Roots.claude_desktop`; folder `<root>\Cache\Cache_Data`.
  If it holds Simple Cache entries (`<16 hex>_0`) → port the Mac reader (magic, key, org match,
  zstd via `ruzstd` with the 256 KiB caps, `\0date:` trailer, reset credits). If it holds
  `index` + `data_0..3` (Chromium blockfile) → `DesktopReading::UnsupportedFormat` and the
  Settings caption "Claude Desktop's cache on this PC uses a format this version can't read."
  [DEGRADE] Sharing violations are misses. Never memory-mapped. The doctor prints which format
  it found, so the first reports settle the question (§9, R3).
- **Projection for upstream code** (`hub::upstream_usage`, WP7): `AppState.usage` gets every
  shown ring's windows; the default ring (the one that includes `~\.claude`, else the first)
  keeps bare ids (`session`, `weekly_all`, …), the others get `<id>@<ring_id>`; `group` = ring
  label when more than one ring is shown. Status: ok → `ok`, stale → `stale`, waiting → `none`
  + note "Waiting for the first reading…", sign-in needed → `none` + note "Not signed in to
  Claude. Run claude, then /login.", unavailable → `none` + its message, failed → `error` +
  message. **Never `needsAuth`** (that would offer upstream's token sign-in button; test pins it).
- Usage readings leave the engine for the cloud as `UsageObservation`s whose source goes out
  through `UsageSource::contract_name()` (`claudeJson` for the `.claude.json` cache), never
  through serde's default names (§3).

### 4.7 Permissions, questions, plans (WP6 answers, WP1 transport; HS§6)
Identical logic (`control::permission_response`); AnswerGate in the UI; the engine refuses an
answer whose `tool_use_id` is not the pending one; "Always" only for narrow rules
(addRules/replaceRules to session or localSettings with a description); answers queue through
the pipeline so they cannot overtake a PostToolUse.

### 4.8 Chat (WP5 transcript/chat items, WP6 messenger, WP1 helper; HS§5.11, §8)
- History: `ConversationParser`/`ToolResultParser` port → `ChatItem`s; 150 newest per page;
  images by reference (`image_id`, fetched with `chat_image` as a data URL capped at 2 MiB);
  subagent following as on the Mac. Updates after the first page are patches (§3.6).
- **Typing replies is opt-in on Windows** (setting `typeReplies`, default off, Settings ›
  Sessions: "Type replies into the terminal", caption "Types your reply into the session's
  console and presses Enter. Works for Windows Terminal, VS Code's terminal and console windows;
  not for a Claude started in the background of another shell (start /b)."). Off: `message_route`
  answers "Typing replies is off…" and the chat's no-route bar offers Show terminal.
- **Two-phase typing**: `agentnotch-hook.exe type --pid <claude pid> --started <epoch ms>
  --expect-window <hex|none> --shells <pid,pid…>` (Claude's direct parent chain of known shells:
  `cmd.exe`, `powershell.exe`, `pwsh.exe`, `bash.exe`, `sh.exe`, each link validated by creation
  time). Stdin line 1: `{"text": "…"}` (UTF-8). The helper: `FreeConsole`; `AttachConsole(pid)`
  (failure → refused "This session has no console to type into" — VS Code extension sessions,
  Git Bash/mintty windows, or "Claude runs as administrator"); `SetConsoleCtrlHandler(NULL,
  TRUE)`; open `CONIN$` read/write; **checks** (`console_checks`): the pid's creation time matches
  `--started`; `GetConsoleWindow()` equals the expected window when one is known;
  `GetConsoleProcessList()` holds only the pid, its descendants and the `--shells` pids, else
  refused "Another program is reading this console"; `GetConsoleMode(CONIN$)` has
  `ENABLE_LINE_INPUT` clear (Claude Code's raw mode; a shell prompt reads in line mode), else
  refused "The terminal isn't at Claude Code's prompt". Then it writes one KEY_EVENT down/up pair
  per UTF-16 unit, sleeps 150 ms (paste detection, as on the Mac), prints `{"phase":"typed"}`
  and waits up to 2 s for stdin line 2: `submit` or `abort`. Meanwhile the engine's `an-ui`
  worker sends `Input::TypeCheckpoint` and `an-core` re-runs `message_safety` on the session's
  **fresh** state (pending requests of every agent, registry status, phase); only `true` sends
  `submit`. On `submit` the helper re-runs `console_checks`, then writes VK_RETURN down/up with
  `'\r'`. It prints the final `{"outcome":"delivered"|"refused"|"typed_not_submitted"|"failed",
  "reason"?}` and exits 0. `abort`, EOF or the 2 s timeout → `typed_not_submitted` "Claude asked
  for something while your reply was typed; it's in the terminal, not sent." So a
  PermissionRequest raised by a background agent in the gap is never confirmed by our Return.
- Residual risk, stated in the caption above: a shell that is itself the direct parent of a
  background Claude and reads the console at its own prompt passes the process-list check; the
  line-input check catches it whenever that shell reads in line mode (cmd, PowerShell without
  PSReadLine); PSReadLine reads in raw mode too and is not caught. This is why typing is opt-in.
- `console-info --pid <pid>` prints `ConsoleInfo` JSON (attached, window, title, processes,
  line_input, elevated_target, error). The engine caches it per (pid, start) for 30 s.
- Text rules unchanged (single line, controls dropped), busy/dialog refusals unchanged (engine
  `message_safety`), outcomes and copy unchanged except the route sentence: "Replies can be
  typed from here for sessions in Windows Terminal, VS Code's terminal and console windows."
- **[ASSUMPTION]** `WriteConsoleInputW` records reach a Bun/Node raw-mode reader and a ConPTY
  host the same way they reach a plain reader: CI proves the two console paths with a test reader
  (`agentnotch-hook/tests/console_type.rs`: a child in its own console, and a child hosted by
  `CreatePseudoConsole`), not with Claude Code itself; the separate Return with a 150 ms gap
  guards against paste detection as on the Mac. The same tests prove the two phases (no Return
  after `abort`, EOF or 2 s of silence) and the line-input refusal (a reader in cooked mode).

### 4.9 Jump to the terminal and "is the user looking at it" (WP6; HS§9)
Host classification from the process tree (`Processes::table`, parent links validated by
creation time) by exe name: `WindowsTerminal.exe`; `OpenConsole.exe`/`conhost.exe`; `Code.exe`,
`Code - Insiders.exe`, `Cursor.exe`, `Windsurf.exe`, `VSCodium.exe`; `wezterm-gui.exe`,
`alacritty.exe`, `Hyper.exe`, `Tabby.exe`; `mintty.exe`; `ConEmu64.exe`; JetBrains `*64.exe`.

| Host | Focus plan (`control::focus_plan`) | Degradation |
|---|---|---|
| conhost | RaiseWindow(`console_info.window`) | exact (its own window) |
| Windows Terminal | window = root owner of the ConPTY pseudo-window (`GetAncestor(GA_ROOTOWNER)`, class `CASCADIA_HOSTING_WINDOW_CLASS`); SelectWtTab when exactly one UIA TabItem name equals the console title | 0 or ≥ 2 matches, renamed tabs, `suppressApplicationTitle`, panes: RaisedOnly ("Brought its Windows Terminal window forward"). [DEGRADE] |
| VS Code family | OpenInEditor(editor exe from the host process path, git top-level of cwd below home, else cwd); `claude-vscode` entrypoint → OpenInEditor(cwd) | the terminal tab inside VS Code can't be selected (same as the Mac) |
| mintty, JetBrains, others | RaiseWindow(best top-level window on the pid chain; upstream `focus.rs` scoring) | window only |
| none found | NotFound | the row offers no "Show terminal" |

Foreground rights: `SetForegroundWindow` from the app right after the user's click in our
window (**[ASSUMPTION]** allowed; the click lands in WebView2's child window, owned by
`msedgewebview2.exe`, so Windows may refuse); `ShowWindow(SW_RESTORE)` when iconic;
`AllowSetForegroundWindow` before a helper does it; no `AttachThreadInput` or ALT hacks. The
outcome is always confirmed with `GetForegroundWindow()` afterwards: a refused foreground
flashes the window (`FlashWindowEx`) and returns RaisedOnly. UIA against an elevated WT fails
(UIPI) → RaisedOnly. All of this runs on `an-ui`; UI Automation uses `IUIAutomation2` with
connection and transaction timeouts of 2 s, so a busy Windows Terminal costs at most that.

"Looking at" (`control::looking_at`): conhost → foreground window is the console window;
WT → foreground is its WT window and (it hosts exactly one known session, or its window title
equals the session's console title); VS Code → foreground pid is the editor and only one known
session runs in it; else false. Visibility (`any_terminal_visible`): EnumWindows front to back,
skip cloaked (`DWMWA_CLOAKED`: other virtual desktops) and minimised, ≥ 40×40, the Mac's 15 %
grid rule; full screen = `SHQueryUserNotificationState` busy/D3D/presentation or the foreground
window covering its monitor.

### 4.10 Notifications, chime, peek, auto-open, badges (WP6 engine + win; HS§7, UI§3.8)
- Content, identifiers, silent baseline, bursts, suppression and withdrawal: HS§7 unchanged.
- WinRT toasts: `ToastNotificationManager::CreateToastNotifierWithId("com.rivantmedia.agentnotch")`
  (the installer's Start-menu shortcut carries that AUMID), `<audio silent="true"/>`, tag = kind
  (`needs`|`review`|`failed`|`limit`), group = session id or ring id (≤ 64), removal via
  `ToastNotificationHistory::RemoveGroupedTagWithId`. Activation is **protocol** activation, no
  COM activator: `launch="agentnotch://open?session=<sid>"`, buttons `Open` →
  `agentnotch://open?session=<sid>` and (review only) `Mark Reviewed` →
  `agentnotch://review?session=<sid>&completed=<ms>`. The URL starts `agentnotch.exe "<url>"`;
  the single-instance plugin forwards it to the running app (WSI); a cold start handles it in
  `setup`. Deep-link actions never answer a request; unknown sessions are ignored; ≤ 10 per
  minute.
- Permission: `ToastNotifier::Setting()`; Settings row "Windows notifications: Allowed / Off in
  Windows Settings" with "Open…" → the glue method `open_notification_settings` (a fixed
  `ms-settings:notifications`; `open_url` stays `https:`-only). No shortcut (dev or portable
  run) → `Unavailable` → no banners, row says "Banners need the installed app". [DEGRADE]
- Chime: `PlaySoundW(SND_ALIAS|SND_ASYNC)`: needs you → `SystemExclamation`, finished →
  `SystemAsterisk`. Settings "Play a sound" (default on) and "Open the notch when a session ends"
  (peek, default on, 3/5/10 s, default 5) live in the Claude Code pane (upstream Windows has no
  such settings).
- Peek: `an:peek {ring_id, seconds}` → notch.js unfolds, shows that ring's card, folds after.
- Auto-open: **Never (default on Windows)** / needs you / needs you or done; never over a
  full-screen app, never when the session's terminal is in front, never replacing an open panel
  (the engine knows the panel's state from `panel_state`); opens without activation (§5.3), and
  its composer and keyboard shortcuts stay inert until a click confirms focus. Peek and toasts
  cover the default.
- "Keep the notch open while a session needs you": **Never (default) / Always** only (no camera
  on Windows; UI§3.7).
- Tray: "Needs-you dot on the tray icon" (default on): an amber dot drawn on upstream's tray
  image; the count is in the tooltip (`Agent Notch — 2 need you`). [DEGRADE: no digit.]

### 4.11 Cloud sync (WP8; CL)
Port CL§1–9 exactly: contract encoding (sorted keys, explicit nulls except `summary`, ms-rounded
`Z` dates, grapheme-safe UTF-16 clamps, key rules), sign-in with PKCE, refresh single-flight and
saved-before-use, website binding, sync service, ledger with owner stretches, token scanner,
backfill, folder logins, usage outbox, summaries with limits and scrub.

**Which Mac cloud code is ported.** The Mac's working tree at the time of this design adds cost
estimates: `ModelPricing.swift` (list prices per response, NanoUSD), `SessionTokenScanner` state
v4 (a per-part NanoUSD cost), `CloudSyncPass.payloadVersion`, and the contract rule for
`sessions[].costUsd`: "Claude Code's own when its status line reported one for a session one
account ran, unless the app's own is larger (the session outgrew it); else the app's, from the
transcript's responses at list prices; null when it can't be priced" (`web/contract/README.md`).
WP8 ports the Mac cloud code **at the first commit on `main` that contains `ModelPricing.swift`**
(it starts only once that commit exists; CL§5.5 step 11's "a split session gets cost nil" and
scanner v3 are superseded by it). The price table exists twice (Swift and Rust); one shared
vector file, `Packages/ClaudeControl/Tests/ClaudeControlTests/Fixtures/model-pricing-vectors.json`
(model id, usage object incl. `speed`, `inference_geo`, cache 1 h split, web searches, advisor
iterations → NanoUSD or null; one vector per table entry and fast entry, plus unknown and
continued-version ids), is read by a new Swift `ModelPricingTests` case and by Rust
`cloud_pricing.rs`, so the two tables cannot drift. A price change bumps both scanner state
versions and both payload versions together.

Windows specifics:
- Files in `<support>` (machine-local, §1.5) with the protected DACL; atomic replace and exclusive
  create via `SecureFiles` (CL§4 WIN); `cloud-install-secret` made lazily.
- What the cloud thread needs from the rest of the engine comes through `CloudDeps` (§3.4); the
  switches (`cloudSyncEnabled`, `cloudSummariesEnabled`) and `cloudDeviceId` are written by
  `an-core` on the cloud's `set_setting`.
- Browser step: `Browser::open(authorize_url)` (`ShellExecuteW "open"`, the default browser: the
  non-ephemeral analog). Dev only: with `AGENTNOTCH_DEV=1` and `AGENTNOTCH_DEV_BROWSER_LOG=<file>`
  (never when sealed), `Browser::open` appends the URL to that file instead of opening it, so the
  smoke test can complete a sign-in against a local fake website (§7.5); the URL holds only the
  PKCE challenge and state, nothing secret. Pending gate `{verifier, website, auth_generation, started_at}` in
  memory only; a callback is accepted once, only while pending, only within 10 min; a cold-start
  or stray callback is ignored ("Sign-in expired; try again" when a sign-in had been pending in
  this run). "Cancel" while signing in; the 10-min timeout is a quiet cancel; "Sign in" while
  pending restarts. After a callback the glue focuses Settings. No website or Supabase change
  (`agentnotch://auth-callback` is already allowed, CL§13).
- projectPath: `SecureFiles::canonical` (on-disk case, no `\\?\`), else lexical normalize
  (never lower-cased); projectName splits on `\` and `/`; `C:` for a drive root. Keys for the same
  folder differ from the Mac's by design (per-install secret).
- Device name `GetComputerNameExW(ComputerNamePhysicalDnsHostname)`, else `COMPUTERNAME`, ≤ 120;
  `appVersion` = `HubConfig.app_version`.
- Summaries: binary like the probe and the same `scrubbed_env` (§4.6); `--settings
  {"disableAllHooks":true}` one argv element, `--tools ""` an empty element; a `.cmd`-only
  install disables summaries with the note "Session summaries need claude.exe (or Node) on this
  PC." [DEGRADE]. Job-object kill on cancel. The run folder is `CloudDeps::summary_folder`
  (the probe's `probe_folder` rules), re-checked with `summary_folder_still_runs` after the run.
- Scrub learns Windows paths (drive paths either separator, UNC, `\\?\`, `%USERPROFILE%`-style,
  `~\`, `/c/Users/…`, `/mnt/c/…`, `\\wsl$\`, `\\wsl.localhost\`; `X:\Users\<name>` → `~`; drive
  root → `…`); LocalNames = entries of `%SystemDrive%\Users` minus Public, Default,
  "Default User", "All Users", desktop.ini, plus the home folder name. Windows vectors are added
  beside the Mac ones.
- Copy: "this Mac" → "this PC", "every Mac" → "every computer" (CL§10).
- Contract e2e: `agentnotch-engine/tests/cloud_contract_e2e.rs` with tests
  `contract_e2e_request` and `contract_e2e_response` gated on `AGENTNOTCH_CONTRACT_OUT` /
  `AGENTNOTCH_CONTRACT_RESPONSE`; `Scripts/cloud-contract-e2e.sh` gains
  `AGENTNOTCH_CONTRACT_APP=swift|rust|both` (default `swift` when `swift` exists, `rust` when only
  cargo exists; `both` runs the web step once per app) and checks cargo's "1 passed".

### 4.12 Engine settings (`<support>\control-settings.json`, WP7)

| Key | Default | Notes |
|---|---|---|
| `hookConsent` | null | true/false after the card |
| `hookConsentScope` | 2 | |
| `hooksEnabled` | true | counts only with consent |
| `statusLineIntegration` | true | |
| `usageProbeIntervalMinutes` | 5 | 0 = off; effective ≥ 300 s |
| `readsDesktopUsageCache` | true | |
| `claudeBinaryPath` | null | |
| `notifyNeedsInput` / `notifyReadyForReview` | true / true | |
| `autoOpen` | `never` (Windows) | `never` / `needsInput` / `needsInputOrDone`; the Mac's default `needsInput` waits until R6 is settled |
| `holdOpenWhileNeedsYou` | `never` | `never` / `always` (Windows) |
| `ringBadges` / `restingMarks` / `trayBadge` | true / true / true | |
| `ringClick` | `openPanel` | `openPanel` / `refreshUsage` |
| `sessionClick` | `smart` | `smart` / `panel` / `terminal` |
| `hotKey` | `off` | `off` / `ctrlAltSpace` / `ctrlAltJ` |
| `panelPinned` | false | |
| `sound` / `peek` / `peekSeconds` | true / true / 5 | Windows additions |
| `cloudSyncEnabled` / `cloudSummariesEnabled` | false / false | set by the cloud thread through `an-core` |
| `cloudDeviceId` | made on demand (uppercase UUID v4) | |
| `typeReplies` | false | Windows addition: typing replies into a terminal is opt-in (§4.8) |

Atomic, owner-only, version 1, unknown keys kept, bad values fall back per key. One writer:
`an-core` (§1.2). A file that doesn't parse is replaced by the next write; one that is there but
can't be read (another program holds it without read sharing) is never written over: the run
uses the defaults, reads it again on a backoff (2 s doubling to 60 s), and merges what the user
changed meanwhile into it. A failed write is tried again on the same backoff. The cloud's
off-switches and sign-out are saved by `an-core` when asked, before the call is queued. A stop
runs in this order: `an-core` releases held requests and saves every store, the workers end,
the cloud stops (its own files), then what the cloud sent as it stopped is applied to the
stopped core and saved, before the process may end.

### 4.13 Sealed mode and dev switches (WP7 engine, WP9 glue)
- `AGENTNOTCH_SAFE_MODE` seals (fails closed: any value except empty/0/false/no/off). Sealed:
  `Hub::sealed` with fixtures ported from `SampleLayout`/`SampleSessions`/`SampleData`; no pipe
  server, no reads or writes of Claude folders or `<support>`, no network, no subprocess, no
  toasts, no sounds, deep links ignored, cloud = the sealed fixture, updater off (WUP), a
  "Sealed" badge in the panel and settings. Upstream's config and logs go to
  `%APPDATA%\Agent Notch Sealed` (WR-DIR), so a sealed run never edits the real app's
  `config.json`. Upstream's other providers keep running (they are not the fork's; on a CI runner
  they find nothing) — unlike the Mac, where seams also seal upstream. [DEGRADE, documented]
- A sealed run shares the identifier with an installed copy (single-instance mutex, WebView2
  profile): it cannot run beside a running Agent Notch. Started beside one, the single-instance
  plugin forwards its argv (which carries nothing sealed-specific: the sealed switches are
  environment variables) and exits 0; the smoke script checks up front that no `agentnotch.exe`
  runs and fails loudly otherwise (a missing self-test report fails it too). A separate sealed
  identifier (a second bundle, as on the Mac) is not built: it would double the Windows build and
  test a binary that never ships.
- Dev switches: `--no-install`/`AGENTNOTCH_NO_INSTALL`, `AGENTNOTCH_NO_NOTIFICATIONS`,
  `AGENTNOTCH_USAGE_PROBE` (probes are off in `--no-install` runs otherwise),
  `AGENTNOTCH_SUPPORT_DIR`, `AGENTNOTCH_SOCKET` (pipe name; the hook honours it only with
  `AGENTNOTCH_DEV=1`), `AGENTNOTCH_EXTRA_CONFIG_DIRS` (`;`), `AGENTNOTCH_WEB_URL` (ignored when
  sealed), `--dump-state`/`AGENTNOTCH_DUMP_STATE` (to `run.log`).
- Sealed-only: `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH=sessions|session:<id>|settings`,
  `AGENTNOTCH_PANEL_SELF_TEST=1` + `AGENTNOTCH_SELF_TEST_OUT=<json path>`,
  `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>` (§7.4).

### 4.14 Doctor, inspector, CLI (WP7 text, WP9 plumbing)
`agentnotch.exe <cmd>` handled by `agentnotch::cli::run` before Tauri (attaches to the parent
console, prints, writes `<data>\<cmd>.log`, exits). The exe is GUI-subsystem: PowerShell neither
waits for it nor captures its output, so scripts (the smoke test, the uninstaller) run it with
`Start-Process -Wait -PassThru` (or NSIS `nsExec::Exec`) and read the exit code from the process
object and the text from `<data>\<cmd>.log`:

| Command | Behaviour | Exit |
|---|---|---|
| `doctor` | the report below; asks a running instance for `control status` over the pipe | 0 |
| `doctor deep` | upstream's `diag::run()` (unchanged; it never reads Claude credentials) | 0 |
| `inspect-accounts` | read-only account inspection (AU§16): accounts, rings, run/store folders, install targets, freshest cached usage (only `accountUuid`/`fetchedAtMs`) | 0 |
| `install-hooks` | refuses without consent (2); with consent, one pass now | 0/1/2 |
| `uninstall-hooks [--quiet]` | §4.3 | 0 (quiet always 0) |
| `control status\|quit` | sends a control frame; prints the answer | 0, or 3 when no instance |
| `autostart on\|off` | upstream's (Run value `Agent Notch`) | 0/1 |

Doctor lines (stable prefixes for CI greps; values never contain tokens, prompts or file bodies):
```
Agent Notch doctor v1.1.0 (com.rivantmedia.agentnotch)
exe: C:\Users\…\AppData\Local\Agent Notch\agentnotch.exe
sealed: no
elevated: app=no running=no sessions-elevated=0
smart-app-control: off|on|evaluation|unknown   (on: unsigned builds and hook copies are blocked; see the release notes)
data: C:\Users\…\AppData\Roaming\Agent Notch   support: C:\Users\…\AppData\Local\com.rivantmedia.agentnotch\Claude (private: yes)
updates: off (built from source)            | updates: on feed=https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json key=B5A5638361FBD019 signed-version=required
pipe: \\.\pipe\agentnotch-hook-S-1-5-21-… running (sessions 3, accounts 2)   | pipe: … no instance running
hook exe: C:\…\agentnotch-hook.exe (present)
accounts: 2
account: claude-acct-1a2b3c4d5e6f "Claude Work" signed-in=yes folders=~\.claude
hooks: consent=unasked|granted|declined installed=1/2 form=exec|string|none exec-form-min=<v|unset>
claude-versions: 2.1.282 (binary), 2.1.270 (vscode extension), unknown (desktop)
status-line: wrapped=1 left-alone=1 (reason: …)
claude: C:\Users\…\.local\bin\claude.exe (not run by the doctor)
desktop-cache: simple|blockfile|absent
deep-link: registered -> "C:\…\agentnotch.exe" "%1" | not registered
autostart: on|off
notifications: shortcut present (AUMID com.rivantmedia.agentnotch) | shortcut missing
providers: <upstream probe lines for codex, cursor, grok, glm, antigravity>
```

### 4.15 Updates (WP9 glue, WP11 pipeline; §6.4)
Upstream's updater UI and flow stay (check 20 s after launch; install from Settings › General;
NSIS passive; the app exits and restarts). Gate: pubkey non-empty and not sealed (WUP). Release
builds carry `endpoints` = `https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json`,
the derived pubkey and `requireSignedVersion: true`; source builds carry neither (and the doctor
says `updates: off (built from source)`). Updates never replace the hook copies in run folders;
the next hook pass after launch refreshes them. Before the updater starts the installer and calls
`std::process::exit(0)`, the glue's `on_before_exit` (seam WUP2, §2.4 `update.rs`) stops the hub:
stores are saved, held PermissionRequests are closed without an answer (the hooks exit 0 and
Claude Code's own prompt decides), children are killed; then `cleanup_before_exit()` runs as
upstream's default did. When the installer then fails to start (`ShellExecuteW` refused: a
declined UAC prompt, AppLocker, an antivirus), the plugin returns the error instead of exiting:
`install_update`'s error arm calls the glue's `install_failed` (seam WUP2), which starts the hub
again and brings back the tray icon and the windows the cleanup took away.

### 4.16 Other providers and the rebrand
Codex, Cursor, Grok, Antigravity, GLM run exactly as upstream (their own credentials, their own
threads); their User-Agents stay upstream's. One of them touches a Claude file: GLM's
`claude_code_key` (glm.rs:142-153) reads `env.ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY` from
`~\.claude\settings.json` and keeps it only when `ANTHROPIC_BASE_URL` is a Z.ai host (it reads the
value before checking the host, so a user's Anthropic API key is held in memory for that moment
and dropped unused). That is a user-configured API key, not a Claude login token, and it is
never sent anywhere else; it is **accepted as upstream ships it** (the maintainer chose to keep
upstream's providers unchanged), documented in the README's privacy section, and pinned by
`verify-token-free.sh` (§6.7) so it can only ever be that one read in `glm.rs`. The rebrand (`core::rebrand`, WP7, with
`ui/agentnotch/rebrand.js`, WP10, sharing
`agentnotch-engine/tests/ui-contract/rebrand-vectors.json`) ports `Fork.rebranded`: "a
Codenotch" → "an Agent Notch" (English), `Codenotch-` compounds → `Agent-Notch-`, other
occurrences → "Agent Notch"; phrases naming upstream's own products stay ("Codenotch for
Windows", "Codenotch-Setup", "hivinz.com", the phone app phrases). Applied to tray strings
(WR-QUIT/WR-TRAY), window titles, and every string in `settings.html` (§5.5).

---

## 5. UI plan (WP10 pages, WP9 windows)

### 5.1 Principles
- Vanilla JS and CSS, no framework, no bundler, no network (like upstream). Each fork script is
  an IIFE in strict mode that defines exactly one global (`window.agentnotch`,
  `window.agentnotchPanel`, `window.agentnotchSettings`) and declares no top-level `let/const`
  (a clash with upstream's classic-script globals would be a SyntaxError that kills upstream's
  page). A node test runs upstream's inline script and the fork script in one `vm` context in
  page order to prove there is no clash.
- Tokens from UI§1 as CSS variables in `theme.css` for dark and light, keyed on upstream's
  `data-theme`; Segoe UI Variable / Cascadia Mono; the Mac type scale ×1.15 (UI§1.4).
- Animations use `steps()` (compositor cost, UI§1.6); breaths are finite (7 half-cycles);
  `prefers-reduced-motion` stills everything.
- All transcript and tool text is untrusted: rendered with `textContent` or through the escaping
  markdown renderer (`markdown.js`: headings, paragraphs, lists, quotes, code, inline code,
  links shown as text + `open_url` on click for `https:` only); no raw HTML ever.
  Every page runs under the app-wide CSP (seam WCSP, §2.5); `panel.html` repeats it as a
  `<meta http-equiv="Content-Security-Policy">`. **[ASSUMPTION]** Tauri's IPC and injected
  globals work under it: the sealed self-test proves a round trip from each page and fails on
  any `securitypolicyviolation` (§7.4). Every renderer that interpolates text into HTML
  (`cardSessions`, `decorateCell`, every settings and panel renderer) goes through one `esc()`
  and is fed hostile strings (`<img src=x onerror=…>`, `"><script>`, `javascript:` links,
  U+202E, 10 000-character names) by the node tests.
- Copy is English-only and ported verbatim from the reports' quoted Mac copy, with the Windows
  substitutions listed there (Finder → Explorer, ⌘ → Ctrl, ⌥ → Alt, Dock → tray, camera clauses
  dropped, "this Mac" → "this PC", tmux/iTerm2 → Windows Terminal/console).
- Data flow: each page primes with `an_call('snapshot')` (or `settings`) and then listens to its
  `an:*` events; payloads are whole snapshots (upstream's model), never diffs.

### 5.2 Notch (`notch.js`, `notch.css`) — the seam hooks' contract
- `agentnotch.claudeCells()` → a **new array** of upstream cell objects, one per shown ring:
  `{id: ring_id, base:'claude', name: ring.label, glyph:'C', snap: {status, windows, fetched_at,
  note, backoff_until:0}}` built from the last `an:snapshot` (window ids `session`/`weekly_all`
  unsuffixed, so upstream's `headlineOf`/`weeklyOf` work per cell). Returns `null` before the
  first snapshot (upstream's own cell shows). A ring with no reading still gets its cell (status
  `none`).
- `agentnotch.decorateCell(p, cell)` (Claude cells only; no-op otherwise): replaces the cell's
  `svg.activity` layer by the ring's `activity` (working: 3/4 arc `arc-spin`; waiting: amber
  full ring pulsing; success: green full ring pulsing until `success_settles_at_ms`, then steady
  at .85; idle: empty), and draws the needs-you/review count badges (capsule 12 px, 9 px bold
  digits, "9+", knockout ring in the pill colour, positions per edge from UI§3.3's table at
  r = 33 in the 44 px ring frame; `pointer-events:none`); sets `aria-label` to `ring.a11y`;
  sets the cell's `stale` class from `ring.usage.stale` (it runs after upstream's own
  `staleOf()` toggle, whose 15-minute rule would dim a Claude ring the engine considers fresh).
  Respects `ui.ring_badges`.
- `agentnotch.cardSessions(p)` → HTML (escaped) for the Claude card's session rows (UI§3.5),
  from each row's `card` (§3.6): name, status ring + word, detail/waiting line, elapsed; up to
  what fits, then "and N more";
  each row a `<button data-an-session>`; clicks follow `ui.hover_click` (smart: panel if it needs
  you, else `focus`, else panel).
- `agentnotch.ringClick(cellId)` → `true` when it handled the click: for a Claude cell with
  `ring_click == openPanel` it calls `an_call('panel_toggle', {ring_id, rect: rectOf(ringwrap),
  reason:'ring_click'})`; with `refreshUsage` it calls `refresh_usage {ring_id}` and upstream's
  press animation (`refreshing[id]`, `turnReading`) itself. Non-Claude cells → `false`.
- Resting marks: a fork element inside `#rest`'s parent, shown only under `body.folded`
  (upstream's class), laid out per UI§3.4 (bar 9×4 amber breathing on `needs_you_key` change,
  review dot, working dot, 3 px gap, upright on side edges). `pointer-events:none`,
  `aria-hidden`.
- Wrapped upstream globals (assignment wrappers, pinned by a node test that asserts each is a
  top-level function declaration in `notch.html`): `foldAllowed` (hold open while needs you when
  `hold_open == always`, and while the panel is open), `showCard` (suppressed while the panel is
  open). Listeners: `an:snapshot` → store + `renderRing()` (+ `renderCard()` when shown);
  `an:peek` → `unfold()`, `hoverId = ring`, `showCard()`, fold after the peek.
- No new hot rects: badges sit inside the pill, marks inside the wake rect, rows inside the card.

### 5.3 Sessions panel window (`agentnotch-panel`; WP9 window, WP10 page)
- Created lazily on the main thread (spawned-thread hop), then kept hidden between uses.
  `transparent(true)`, `decorations(false)`, `always_on_top(true)`, `skip_taskbar(true)`,
  `shadow(false)`, `resizable(false)`, `focused(false)`, `visible(false)`, title
  `Agent Notch sessions`, capability `agentnotch`. Browser accelerator keys off
  (`ICoreWebView2Settings3::SetAreBrowserAcceleratorKeysEnabled(false)` via `with_webview`), plus
  `preventDefault` for F5/Ctrl+R/Ctrl+J/Ctrl+Shift+R/Ctrl+P/Ctrl+F in the page.
- Placement: engine `geometry::panel` (port of `ClaudePanelGeometry`: widths list 400/440 side,
  440/520 flat and floating; height caps 680 list / 780 chat; margin 8; tail 32×36; corner 16;
  `beside`/`aboveOrBelow`/`floating`; `tailOffset` clamp) from the ring rect in physical px
  (notch window position + the page's rect), the monitor work area and scale. The page draws
  the card + tail as one silhouette (solid `--card`; Mica/Acrylic can't follow the tail).
  Floating when the notch is hidden or the ring is off: top-centre of the pointer's monitor.
- Focus model (`agentnotch-win::window`): opened by ring click, hover row, peek click, hotkey,
  notification or Settings → shown and `set_focus()` asked (the terminal's HWND is saved first);
  auto-open (only when the user chose a policy) → `WS_EX_NOACTIVATE` + `SW_SHOWNOACTIVATE`
  (never steals keys; buttons still work by mouse). **Keyboard gate**: whatever opened it, the
  page treats itself as unfocused until the glue confirms `GetForegroundWindow() == panel HWND`
  (`an:panel_focus {focused:true}`). While unfocused the composer is read-only and shows "Click
  to type", and the keyboard shortcuts (1–4, Enter, Ctrl+Enter, Ctrl+Alt+Enter) do nothing;
  a click on the composer calls `panel_take_focus` (drops `WS_EX_NOACTIVATE`, asks the
  foreground) and the gate opens only on the confirmation. So a caret is never shown while
  keystrokes still go to the terminal (where "2" + Enter could answer a prompt). On close the
  saved window gets the foreground back when it still exists. **[ASSUMPTION]** foreground rules
  on a real desktop; the self-test checks styles, z-order and that the gate stays shut without
  a confirmation (§9, R6).
- Closing: Esc (chat → list → close), ✕, the same ring again, a successful jump (unless pinned),
  a mouse-down outside (unless pinned; detected by the notch's existing pointer poll plus a
  `WM_ACTIVATEAPP`/blur when the panel was foreground), display change, auto-close
  (`control::auto_close_deadline`, the port of `ClaudePanelPolicy.autoCloseDeadline`: 1 s after
  the cause resolves or max(peek, 8 s), unless engaged; the engine evaluates it on the
  `PanelState` the glue reports and emits `PanelClose`). Pin persists (`panelPinned`, set through
  `set_setting`). Re-anchors 0.35 s after the
  notch moves. Topmost re-asserted after upstream's topmost watchdog runs (the glue calls
  `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)` on the panel after each `notch_edge`/foreground
  event).
- Size: the page reports its natural content height (`panel_report_size`); the glue clamps to
  [220, cap] and resizes instantly (no animated resize on transparent WebView2), growing the
  window before the content grows and shrinking after it shrinks.
- List screen (UI§5): header (title, Sealed badge, pin/gear/close; attention strip; account
  chips), setup banners (consent card, scope notice, pipe error, control off / hooks missing, the
  Codenotch-hooks note), sections with folding and frozen order under the pointer (FLIP 320 ms),
  rows regular/compact, action bars with AnswerGate (0.35 s arm, one answer per id, 600 s
  memory), question chips (1–4 → keys 1–4), undo toast (5 s), empty states. Gear menu is an
  in-page menu (not native) with the same items.
- Chat screen (UI§6): header (back, title, account tag, task summary button + board, context
  meter, show-terminal), transcript items, tool results (`toolresults.js`: file view, Bash,
  Grep, Glob, WebFetch, WebSearch, Todo/Task, AskUserQuestion, ExitPlanMode, MCP, diffs), the one
  bottom bar by precedence (approval / question form / plan / terminal-only dialog / no route /
  composer), drafts per session (in memory + `localStorage` best effort).
- Keyboard (UI§5.9, Ctrl for ⌘, Alt for ⌥): Up/Down, Enter, Ctrl+Enter, Ctrl+Alt+Enter,
  Ctrl+Backspace, 1–4, Ctrl+J, Ctrl+R, Ctrl+Shift+R, Ctrl+Z, Esc, all behind the keyboard gate
  above. Hotkey Off / Ctrl+Alt+Space / Ctrl+Alt+J via `agentnotch-win::hotkey` (registration
  failure → the glue's `hotkey_status` → Settings shows "That shortcut is taken by another
  app.").

### 5.4 Settings (`settings.js`, `settings.css`; WP10)
- The "Claude Code" tab (WSS1–4) is first in the sidebar; the first launch (consent unasked)
  opens Settings on it. The pane is built with upstream's blocks (`.sec`, `.group`, `.item`,
  `.item.cap`, `.switch`, `.seg`, `select`, `.btn`, `.link`, `#toast`), sections in this order:
  consent card (full file list; "Turn on" emphasised but **not** the default button) / scope
  notice; Accounts (UI§7.2 with Windows copy: Show in Explorer, PowerShell launch command, native
  folder picker via `pick_folder`); Hooks and status line (pipe name instead of socket path;
  Claude Code version and path, Choose… / Find automatically; last-change notice; "Remove
  Codenotch's hooks" per folder when present); Usage (interval, Desktop cache switch + format
  caption, per-account lines, Refresh now); Cloud (CL§10 / UI§7.5 copy, plus Cancel while
  signing in); Sessions and attention (open panel policy, keep open Never/Always, ring counts,
  folded-notch dots, tray dot, ring click, hover click, shortcut, sound, peek + duration, "Type
  replies into the terminal" (off by default, §4.8), "Open the sessions panel"); Notifications
  (two switches, Windows permission row); Advanced (session state copy, review-queue reset).
  Footnotes: "Sessions inside WSL aren't tracked yet." and "Uninstalling keeps Claude Code's hooks
  unless you tick Delete the application data; turn Claude Code control off first to remove
  them."
- Upstream's own "Let Claude Code notify Codenotch" hooks switch (Accounts pane) is hidden by
  `settings.css`; its commands are rerouted anyway (WH6/WH7: get = consent && enabled; set(true)
  without consent → error "Turn on Claude Code control in Settings › Claude Code first." and the
  pane switches tab).
- General pane: upstream's updates block unchanged (it works once a pubkey exists); version shown
  through `getVersion()` (= VERSION).

### 5.5 Rebrand in upstream pages
`settings.js` wraps upstream's global `ui(key, fallback)` with `rebrand()`, rebrands
`document.title`, the static DOM once (text nodes and `aria-label`/`title` attributes under
`#app`), and later DOM changes through a `MutationObserver` on `#body` (user data inside
interpolated strings is never touched because upstream's dynamic text flows through `ui()` and
`esc()`; the observer only rebrands text nodes that contain "Codenotch" and are not inside
`[data-user]` — the fork marks its own user-data nodes). `notch.html` has no upstream name.
A node test asserts every "Codenotch" occurrence in `settings.html` is rebranded or is one of the
kept product phrases.

### 5.6 Visual verification without a human
Three layers, because no person runs the Windows app before users do:
1. **Machine-checked layout invariants** in the sealed self-test (§7.4), on every edge and at
   100 %, 125 % and 150 % page scale (hosted runners are 1024×768 at 100 %, so the extra scales
   are separate sealed runs with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--force-device-scale-factor=1.25`
   / `1.5`; window geometry at real 125 %, 150 % and mixed-DPI monitor layouts is covered by the
   engine's `geometry` vectors, §7.2): badges stay inside the ring pill and never overlap the % label
   on the flat edges (UI§3.3); every interactive element of the notch lies inside a hot rect;
   no text overflows its box (`scrollWidth > clientWidth` on any `[data-an-text]`) in the panel
   at 400 and 520 px; the panel rect lies inside its monitor's work area; the tail offset lies
   inside the card's corners.
2. **Pixel baselines**: `windows/agentnotch-ui-tests/baselines/*.png`, compared with a
   per-pixel tolerance (anti-aliasing) and a small changed-area budget. The first green run's
   snapshots become the baselines after the WP10 agent has looked at each one (agents can view
   PNGs: download the CI artifact and read the files); a deliberate UI change or a runner image
   roll re-baselines in the same commit, never silently.
3. **Look at the PNGs** on every UI change, as on the Mac.

---

## 6. Build, CI and release

### 6.1 `.github/workflows/agentnotch-windows.yml` (new; reusable)

```yaml
name: Windows
on:
  push:
    paths: [windows/**, VERSION, app-config.json, web/contract/**, Scripts/check-seams.sh,
            Scripts/fork-seams.txt, Scripts/verify-token-free.sh, .github/workflows/agentnotch-windows.yml]
  pull_request:
    paths: [same list]
  workflow_dispatch:
  workflow_call:
    inputs:
      updater_pubkey: { type: string, default: "" }     # release builds only
      expected_key_id: { type: string, default: "" }
      release: { type: boolean, default: false }
permissions: { contents: read }
concurrency:                                            # never the fork.yml group (both run inside Release)
  group: ${{ github.workflow }}-windows-${{ github.ref }}
  cancel-in-progress: ${{ github.workflow != 'Release' }}
jobs:
  build:
    if: github.repository != 'vinzdg/codenotch'
    runs-on: windows-2025                               # pinned image, like Xcode 26.6 on the Mac
    timeout-minutes: 90
    defaults: { run: { shell: pwsh } }
```
Steps, in order (each a named step; secrets: none, ever):
1. `actions/checkout@v7` (`persist-credentials: false`).
2. **Preflight** (fails the job, never skips a test): the session has an interactive desktop
   (`OpenInputDesktop` succeeds; `[Environment]::UserInteractive`); Git Bash exists at
   `%ProgramFiles%\Git\bin\bash.exe`; no `claude`, `claude.exe` or `claude.cmd` is reachable
   (`where.exe`, `%APPDATA%\npm`, `%USERPROFILE%\.local\bin`); no `agentnotch.exe` or
   `codenotch.exe` runs; records the runner's elevation and UAC state in the summary.
3. `dtolnay/rust-toolchain@<pinned sha>` with `toolchain: 1.98.1`, components `clippy, rustfmt`.
4. `Swatinem/rust-cache@v2` (`workspaces: windows`) — **skipped when `inputs.release`**.
5. `actions/setup-node` (Node 22: built-in `WebSocket` for the CDP driver, no npm deps).
6. UI checks: upstream's five (`check-ui-scripts.mjs`, `test-light-surface.cjs`,
   `test-claude-auth-ui.cjs`, `test-ko-i18n.cjs`, `test-codex-headline.cjs`) + `node --test
   windows/agentnotch-ui-tests`.
7. `cargo fmt --check` for the five fork crates.
8. `cargo clippy --locked --all-targets -p agentnotch-proto -p agentnotch-engine -p agentnotch-win -p agentnotch-hook -p agentnotch-release -- -D warnings`.
9. `cargo test --locked` for the same crates with `AGENTNOTCH_CI_ADMIN=1` (runs the Windows
   integration tests of §7.3, including `win_admin.rs`, which creates a second local user and
   spawns medium-integrity processes). `win_admin.rs` skips (with a printed notice) only when
   that variable is unset and `CI` is not `true`, so a developer's machine is never touched while
   CI can never silently skip it.
10. `cargo test --locked -p codenotch` (upstream's 129 tests with the seams in).
11. `cargo build --locked -p codenotch`; fail on any warning whose span is in
    `codenotch/src/agentnotch/` or a fork crate (upstream's own warnings are reported only).
12. `windows/scripts/agentnotch-build.ps1 -Out out [-UpdaterPubkey $inputs.updater_pubkey]`:
    checks VERSION == tauri.conf.json version; builds the hook (`cargo build --release --locked
    -p agentnotch-hook --bin agentnotch-hook --target-dir target/hook` with
    `RUSTFLAGS=-C target-feature=+crt-static`, no secrets in env); writes `$RUNNER_TEMP\agentnotch.build.json`
    (§6.5); `npm ci` in `windows/tools`; `npx tauri build --config tauri.bundle.conf.json
    --config <overlay> -- --locked`; copies the single `target/release/bundle/nsis/*-setup.exe`
    to `out\AgentNotch-<V>-Setup.exe`; asserts `VersionInfo.ProductVersion` starts with V.
13. `windows/scripts/agentnotch-smoke.ps1 -Installer out\AgentNotch-<V>-Setup.exe -Version <V>
    -Updates (on|off) -KeyId <expected_key_id> -Artifacts out\smoke` (§7.5; the fake-claude,
    pipe-test and hook exes come from step 9's build).
14. `actions/upload-artifact@v7`: release → name `windows-unsigned` (installer only, retention
    3 days); otherwise `AgentNotch-Setup-<V>-<sha>` (installer, `out\smoke\**` incl. snapshots,
    layout reports and baseline diffs, retention 14 days, `if-no-files-found: error`).
15. Step summary: "Unsigned, no update feed (built from source). SmartScreen: More info, then
    Run anyway. Never attach it to a release."

Timeout: 90 minutes (the smoke test's phases take about 15).

Runner: `windows-2025` is what `windows-latest` resolves to today; it is pinned (like Xcode 26.6
for the Mac) so an image roll-over cannot change a release build. Cross-checks: the
Windows-target `cargo clippy` of the fork crates also runs on ubuntu in `fork.yml` (§6.2) on every
push, so Windows-only code is linted even when this workflow's path filter skips a commit; on the
Mac, implementers run the same cross-check (§7.1); the app crate is never cross-built (it
compiles C: SQLite, ring) and is built only here.

### 6.2 `fork.yml` additions
- New job `rust` (ubuntu-latest, `if: github.repository != 'vinzdg/codenotch'`): toolchain
  1.98.1 + target `x86_64-pc-windows-msvc`; `cargo fmt --check`; clippy `-D warnings` (host) and
  `cargo clippy --target x86_64-pc-windows-msvc -D warnings` for the five fork crates (lints the
  Windows code without a Windows runner; no C toolchain needed); `cargo test --locked` for the
  fork crates; "no C dependencies": `cargo tree --locked -e normal,build --target x86_64-pc-windows-msvc`
  and `--target x86_64-unknown-linux-gnu` for each fork crate list none of `cc`, `ring`,
  `openssl-sys`, `libsqlite3-sys`, `zstd-sys`, `bindgen`; node UI tests (fork + upstream);
  `AGENTNOTCH_CONTRACT_APP=rust Scripts/cloud-contract-e2e.sh` (Docker is on ubuntu runners).
- The macOS `checks` job keeps `check-seams.sh` and `verify-token-free.sh` (both extended, §6.7);
  its Swift tests now include `ModelPricingTests` over the shared pricing vectors (§4.11).
- The Release workflow runs all of fork.yml first (`workflow_call`), as today.

**Claude Code facts** (`.github/workflows/claude-code-facts.yml`, ubuntu, `workflow_dispatch`
+ weekly schedule + changes to the script; never part of Release). The hook forms and the
status-line takeover depend on Claude Code behaviour that is read, never run.
`windows/scripts/check-claude-code-facts.mjs` downloads the npm packages of a list of Claude
Code versions (every minor's first and last release from 2.0.0 on, plus the newest; for versions
shipped as native binaries, the Windows platform package) with `npm pack`, and greps their
JavaScript (or the JavaScript embedded in the binary) for fixed patterns; **nothing is ever
executed**. It writes `windows/agentnotch-engine/tests/fixtures/claude-code-facts.json`:
per version, whether hook entries accept `args` (exec form), `shell`, whether the hook-entry
schema rejects unknown keys, whether `statusLine` honours `shell`, which default shell
string-form hooks and status lines use on Windows (`R1()`), whether `CLAUDE_PID` is exported to
hooks, and the text of the bash-form transform (`yun`). A pull request with the regenerated file
is opened by hand after reviewing the diff. The committed file is the only source of
`EXEC_FORM_MIN` (`hooks::facts`, tested against it), and of the status-line rule's
**[ASSUMPTION]**s; until it establishes them, the conservative choices of §4.3 stand (string
form only; status line wrapped only under the safe rule).

### 6.3 `release.yml` (all publishing stays here)

Triggers unchanged (push to main touching `VERSION` or `Scripts/sparkle-public-ed-key.txt`;
`workflow_dispatch` with `dry_run`, `rotate_update_key`), plus input `skip_windows` ("Publish
without the Windows build: Windows copies miss this release until the next one"). Concurrency
unchanged. Job graph:

```
checks (fork.yml) ─┐
plan ──────────────┼─▶ release-tool ─▶ keys ─┐
                   │                         ├─▶ windows (uses agentnotch-windows.yml, release: true) ─▶ sign-windows ─┐
                   └─▶ mac ──────────────────┼─────────────────────────────────────────────────────────────────────────┼─▶ publish ─▶ website
```

| Job | Runner | Permissions / environment | Does |
|---|---|---|---|
| `plan` | ubuntu | `contents: read`, no environment | today's "Plan the release" step moved out (main only, VERSION, skip/stale/foreign draft, VERSION ≥ latest, Sparkle key continuity, tag at commit) **plus**: `previous_has_feed` = the latest release carries `latest.json`; `skip_windows` with `previous_has_feed` is a problem unless ticked (then a warning); outputs `version`, `tag`, `skip`, `stale_draft`, `previous_tag`, `previous_has_feed`, `windows` (`true` unless skipped) |
| `release-tool` | ubuntu | `contents: read`, **no environment, no secrets** | `cargo build --locked --release -p agentnotch-release` (all dependency build scripts run here, where there is nothing to steal); uploads the binary as artifact `release-tool` (1 day); output `sha256` of the binary |
| `keys` | ubuntu | `contents: read`, **environment `release`** | no checkout of build tooling, no cargo, no npm: downloads `release-tool`, checks its SHA-256 against `needs.release-tool.outputs.sha256`, `chmod +x`; then one step pipes the seed into it (`printf '%s' "$SEED" \| ./agentnotch-release derive-public`, `SEED` set from `SPARKLE_ED_PRIVATE_KEY` in that step's `env` only; never an argument); outputs `pubkey`, `key_id`; continuity: with `previous_has_feed`, `key-id-of-feed` of the previous `latest.json` must equal `key_id` unless `rotate_update_key`; if `Scripts/tauri-update-public-key.txt` exists its first non-comment line must equal `pubkey`; writes both to the step summary with "commit this as Scripts/tauri-update-public-key.txt to pin it" |
| `mac` | macos-26 | `contents: read`, environment `release` | today's key check + `release-build.sh` (unchanged) + notes inputs; uploads artifact `mac-release` (dmg, zip, appcast.xml, release-info.env; 3 days); outputs `DMG`, `ZIP`, `NOTARIZED` |
| `windows` | windows-2025 (reusable) | `contents: read`, **no secrets, no environment** | §6.1 with `updater_pubkey`, `expected_key_id`, `release: true` |
| `sign-windows` | ubuntu | `contents: read`, **environment `release`** | downloads and hash-checks `release-tool` as `keys` does (never builds anything); downloads `windows-unsigned`; **sign** (seed piped on stdin, the only step with the secret); **verify** with `minisign-verify` against `needs.keys.outputs.pubkey` and version; **feed** (writes and re-verifies `latest.json`); uploads `windows-release` (exe, `.exe.sig`, `latest.json`; 3 days) |
| `publish` | ubuntu | **`contents: write`**, no environment | `if: !cancelled() && skip != 'true' && mac == success && (sign-windows == success \|\| (windows == 'false' && sign-windows == skipped))`; downloads both artifacts; writes notes (§6.3.1); dry run → uploads `AgentNotch-<V>-dry-run` with all files and stops; else deletes its stale draft, `gh release create "$TAG" --draft --target "$GITHUB_SHA" --title "Agent Notch $VERSION" --notes-file … --generate-notes [--notes-start-tag prev] <dmg> <zip> appcast.xml AgentNotch-$V-Setup.exe AgentNotch-$V-Setup.exe.sig latest.json`; asserts the asset set is exactly those six names, none matches `(?i)codenotch`; checks each uploaded size with `wc -c`; `gh release edit "$TAG" --draft=false --latest`; warning-only checks that `latest/download/appcast.xml` and `latest/download/latest.json` serve V (6 tries, 10 s) |
| `website` | ubuntu | unchanged (`id-token: write`, environment `release`) | `needs: publish`; `if: needs.publish.result == 'success' && !dry_run`; steps unchanged (the OIDC `job_workflow_ref` stays `release.yml@refs/heads/main`) |

Both `latest/download/*` URLs switch together when the draft is published. With required
reviewers on `release`, approvals: `keys`+`mac` together, then `sign-windows`, then `website`.

#### 6.3.1 Release notes (publish job)
Mac section unchanged; then:
```
## Windows (preview)
The Windows app is new in this release and still a preview: it is built and tested
automatically, but it has not been used by people yet. Please report anything odd.
1. Download **AgentNotch-<V>-Setup.exe** below and run it. It installs for your user only (no
   administrator) into %LOCALAPPDATA%\Agent Notch, and fetches Microsoft's WebView2 if needed.
2. The installer isn't code-signed yet, so Windows SmartScreen may say "Windows protected your
   PC": choose **More info**, then **Run anyway**. With **Smart App Control** turned on
   (Windows 11), unsigned apps are blocked with no way around it; Agent Notch can't be used there
   until it is code-signed.
Requires Windows 10 or 11 (x64).
To remove Agent Notch's Claude Code hooks when uninstalling, turn Claude Code control off in
Settings first, or tick "Delete the application data" in the uninstaller.
## Updates (Windows)
The app checks for updates shortly after it starts; install one from **Settings › General**.
Every update is verified with Agent Notch's own signing key before it runs. Copies built from
source never update themselves.
```

### 6.4 Windows update signing — decision

**Question.** Proposal P: no new secret; derive a separate Ed25519 key from the existing
`SPARKLE_ED_PRIVATE_KEY` seed with HKDF-SHA256; publish only the public key between jobs; sign
the NSIS installer in minisign prehashed format in a job holding the seed; verify with
`minisign-verify`; write `latest.json`. Alternative A: a separate `TAURI_SIGNING_PRIVATE_KEY`
(+ password) secret made with `tauri signer generate`, signing with the Tauri CLI.

| Criterion | P (derive) | A (separate secret) |
|---|---|---|
| Blocks 1.1.0 on the maintainer | no | yes: key generation, two secrets, a pubkey commit before the release |
| Compromise domain | one secret for both platforms | two secrets **in the same environment**: whoever can read one can read the other; no real separation |
| Independent rotation | yes: bump the HKDF `info` label (`…v1` → `…v2`) rotates Windows only; rotating the Sparkle seed rotates both (both strand installs anyway, and the guards already demand an explicit tick) | yes |
| Cross-protocol key reuse | none: HKDF domain separation gives Windows a different Ed25519 key from the one Sparkle uses. (The signature format is not what separates them: the updater verifies with `allow_legacy = true`, updater.rs:1544, so it would accept a non-prehashed `Ed` signature too.) | none |
| Tooling in the job holding a secret | our ~300-line Rust tool with RustCrypto/dalek crates, built in a separate secret-free job and passed on as a hash-checked binary; the jobs holding the seed run no build script, no cargo, no npm | `npx @tauri-apps/cli` (a large npm tree with native binaries) in the job holding the key |
| Custom crypto risk | low: vetted primitives, output verified by `minisign-verify 0.2.5` — the updater's own verifier — before publishing, plus a fixed test vector computed by two independent implementations (Appendix B) | none |
| Key continuity guard | the Sparkle guard pins the seed; the derivation constants are pinned by the test vector; the `keys` job also checks the previous release's `latest.json` key id | needs its own guard (a committed pubkey file) |
| Tauri CLI signer quirks | not used (`createUpdaterArtifacts` stays false) | the CLI only warns on a pubkey mismatch (BT§3) |

**Decision: adopt P, amended as follows.**
1. Signing happens in a **dedicated `sign-windows` job on ubuntu**, not in the Windows build
   job: the seed never reaches the runner that ran npm, the Tauri bundler and ~500 crates' build
   scripts. The Windows build gets only the public key (a `keys` job output; a public value, so
   GitHub's secret masking does not interfere).
2. The trusted comment carries `version:<V>` and releases ship `requireSignedVersion: true`
   (tauri-plugin-updater 2.12.0, already locked; BT§3), so a tampered feed cannot pair a newer
   version with an older signed installer.
3. The derived public key is additionally checked against the previous release's `latest.json`
   key id (continuity, independent of the derivation code), and — once the maintainer commits
   it — against `Scripts/tauri-update-public-key.txt` (optional pin; recommended after 1.1.0).
4. Branch/PR builds carry no updater key and no endpoint (like the Mac: only release builds
   update); the doctor proves it in the smoke test.
5. The seed reaches only prebuilt code: `release-tool` builds the tool without secrets; `keys`
   and `sign-windows` hash-check the binary and pipe the seed into it on stdin. No process left
   behind by a dependency's build script can read the signing step's environment.
6. **Rotation.** Windows-only rotation (`…v1` → `…v2` in the HKDF `info`) needs one **bridge
   release**: signed with the v1-derived key and carrying the v2 pubkey, so installed copies
   verify it and then trust v2; the `keys` job's continuity check is ticked off for that one
   release (`rotate_update_key`). Rotating the Sparkle seed itself changes the Windows key too:
   the bridge release must then be signed with the **old** seed's derived key, so the old seed
   must stay available (as a second secret, `SPARKLE_ED_PRIVATE_KEY_PREVIOUS`, for that one
   release); without it every installed Windows copy is stranded and must be reinstalled by hand
   (as every Mac copy would be). `Scripts/release-make-keys.sh`'s warning says so.

**The tool: `windows/agentnotch-release` (bin `agentnotch-release`, C-free, WP11).** The seed is
read only from stdin (base64, whitespace trimmed, must decode to 32 bytes), never argv, never
printed, zeroised after use.

| Subcommand | Input | Output |
|---|---|---|
| `derive-public` | seed on stdin | stdout `{"pubkey":"<Tauri pubkey>","key_id":"<16 hex>","minisign_public_key":"<RW…>"}` |
| `sign --file F --version V --out S [--timestamp UNIX]` | seed on stdin | writes S; stdout `{"key_id","trusted_comment"}` |
| `verify --pubkey P --file F --sig S --version V` | — | exit 0, or 1 with the reason (uses `minisign_verify::PublicKey::decode` + `verify(data, sig, true)`, then `version:` == V and `file:` == basename(F)) |
| `feed --version V --tag T --repo R --installer NAME --sig S --pubkey P --file F --notes-url U --out J [--pub-date RFC3339]` | — | writes `latest.json`, re-reads it and verifies the signature taken from the JSON against F |
| `key-id-of-feed J` / `key-id-of-pubkey P` | — | the 16-hex key id |

**Exact formats.**
- Derivation: `okm = HKDF-SHA256(ikm = seed[32], salt = "com.rivantmedia.agentnotch",
  info = "tauri-updater minisign ed25519 v1", L = 40)`; `sk = ed25519 SigningKey::from_bytes(okm[0..32])`;
  `key_id = okm[32..40]` (stored in blobs as these 8 bytes; displayed as the little-endian u64
  in upper-case hex, i.e. the bytes reversed, as minisign prints it).
- Public key text: `untrusted comment: minisign public key: <KEYID>\n` +
  `base64("Ed" ‖ key_id ‖ pk32)` + `\n`. **Tauri pubkey** (config `plugins.updater.pubkey`) =
  standard base64 (padded) of that text's UTF-8 bytes.
- Signature: `sig = Ed25519(sk, BLAKE2b-512(file bytes))`; blob = `"ED" ‖ key_id ‖ sig` (74 bytes);
  trusted comment = `timestamp:<unix s>\tfile:<file name>\tversion:<V>`;
  `global = Ed25519(sk, sig ‖ trusted comment bytes)` (the text after `trusted comment: `);
  signature text = `untrusted comment: signature from agentnotch release key\n` +
  `base64(blob)\n` + `trusted comment: <trusted>\n` + `base64(global)\n`.
  **`.sig` file** = standard base64 of that text, no trailing newline. `latest.json`'s
  `signature` = the `.sig` file's content. (Matches what `tauri signer sign` produces, BT§3,
  and what `tauri-plugin-updater::verify_signature` decodes.)
- `latest.json`:
  ```json
  {"version":"1.1.0",
   "notes":"https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.1.0",
   "pub_date":"2026-10-01T12:00:00Z",
   "platforms":{
     "windows-x86_64-nsis":{"signature":"<.sig content>","url":"https://github.com/rivantmedia/agentnotch/releases/download/agentnotch-v1.1.0/AgentNotch-1.1.0-Setup.exe"},
     "windows-x86_64":{"signature":"<same>","url":"<same>"}}}
  ```
  URLs are tag-specific (never `latest/download`), like the appcast enclosure.
- Tests (WP11): Appendix B's vector (seed → pubkey, key id); sign/verify round trip through
  `minisign-verify`; a committed fixture pair (a small file + a `.sig` + pubkey produced once
  with `tauri signer` and a **throwaway** key, public material only) that our `verify` accepts
  (format compatibility with the CLI); rejection of a wrong version, a wrong file name, a
  flipped byte, a foreign key id; `feed` round trip.

### 6.5 Tauri configuration per build
- Source `tauri.conf.json` (seams, §2.5): no endpoints, `pubkey: ""`, `createUpdaterArtifacts`
  false, deep-link scheme, NSIS hooks, identifiers.
- `agentnotch-build.ps1` always writes an overlay: branch/PR `{"version":"<V>"}`; release
  `{"version":"<V>","plugins":{"updater":{"pubkey":"<derived>","endpoints":["https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json"],"requireSignedVersion":true,"windows":{"installMode":"passive"}}}}`.
- Tauri CLI pinned to 2.11.4 (upstream's) through `windows/tools/package.json` +
  `package-lock.json` (`npm ci`, integrity hashes); signing never uses it.
- NSIS hooks (`nsis/agentnotch-hooks.nsh`, WP11):
  ```nsis
  !macro NSIS_HOOK_PREUNINSTALL
    ${If} $UpdateMode <> 1
      nsExec::Exec '"$INSTDIR\agentnotch.exe" control quit'
      Sleep 1500
      ; Hooks are removed only on an explicit request: "Delete the application data" (set by the
      ; confirm page before this section runs) or /REMOVEHOOKS. The "Uninstall before installing"
      ; step of a manual upgrade runs this uninstaller without /UPDATE and must leave every
      ; settings.json alone.
      ${GetOptions} $CMDLINE "/REMOVEHOOKS" $R9
      ${If} $DeleteAppDataCheckboxState = 1
      ${OrIfNot} ${Errors}
        nsExec::Exec '"$INSTDIR\agentnotch.exe" uninstall-hooks --quiet'
      ${EndIf}
    ${EndIf}
  !macroend
  !macro NSIS_HOOK_POSTUNINSTALL
    ${If} $DeleteAppDataCheckboxState = 1
    ${AndIf} $UpdateMode <> 1
      SetShellVarContext current
      RMDir /r "$APPDATA\Agent Notch"
      RMDir /r "$APPDATA\Agent Notch Sealed"
    ${EndIf}
  !macroend
  ```
  (Scheme registration comes from the template via `plugins.deep-link`; the Run value is deleted
  by the template; `%LOCALAPPDATA%\com.rivantmedia.agentnotch`, which holds `<support>`, is
  removed by the template itself under the same checkbox.) The smoke test covers the default
  path (phase 12: hooks kept) and the explicit one (phase 15: `/REMOVEHOOKS`); the checkbox itself
  is a UI page the silent runs cannot tick, so its variable is the only untested link, one
  `${If}` away from the tested `/REMOVEHOOKS` branch.

### 6.6 Version plumbing
`VERSION` (single source) → `tauri.conf.json` `"version"` (must be equal: `check-seams.sh`
rule and `agentnotch-build.ps1`) → overlay `version` → `package_info().version` → UI
(`getVersion()`), doctor, tray tooltip (WR-TRAY), cloud `appVersion`, the updater's version
compare, the signed `version:` → installer file name and PE version resource. Crate versions
are not used (upstream's crate stays 1.18.0; fork crates 0.0.0). `Scripts/bump-version.sh <V>`
(WP11) edits `VERSION` and the `tauri.conf.json` line together; the release commit is that
script's output.

### 6.7 `check-seams.sh`, `fork-seams.txt`, `verify-token-free.sh` (WP0, final in WP0)
`check-seams.sh`:
- Diff scope adds `windows` and upstream's workflow files `.github/workflows/{ci,package,windows,windows-package}.yml`
  (no ALLOW for those four: any fork edit fails).
- ALLOW additions: the edited upstream files of §2.5 and the fork paths
  (`windows/agentnotch-*/**`, `windows/codenotch/src/agentnotch/**`,
  `windows/codenotch/ui/agentnotch/**`, `windows/codenotch/capabilities/agentnotch.json`,
  `windows/codenotch/nsis/**`, `windows/scripts/agentnotch-build.ps1`,
  `windows/scripts/agentnotch-smoke.ps1`, `windows/tools/**`, `windows/Cargo.lock`).
- FORBID comment detection per extension: `.rs`/`.js`/`.mjs`/`.cjs` `//`, `/*`, `*`;
  `.html` those plus `<!--`; `.toml`/`.yml`/`.sh`/`.ps1` `#`; `.nsh`/`.nsi` `;` and `#`;
  `.json` none. (Rust `#[…]` lines are not comments.)
- New checks: (a) engine/proto purity (§2.3); (b) `tauri.conf.json` `"version"` == `VERSION`;
  (c) upstream's name: no `"…Codenotch…"` string literal in fork-owned `.rs`/`.js`/`.html`
  outside `core/rebrand.rs`, `ui/agentnotch/rebrand.js`, tests and a NAME_OK list, and none in
  fork-added lines of upstream windows files; (d) every notch/settings seam hook named in §2.5 is
  present (SEAM lines do it); (e) `agentnotch-hook/Cargo.toml` names no argument parser (`clap`,
  `argh`, `pico-args`, `lexopt`, `gumdrop`, `structopt`) and its sources contain no `println!`
  or `.unwrap()` outside tests (§1.8); (f) every client `CreateFileW` of the pipe in fork code
  passes `SECURITY_SQOS_PRESENT` (a grep over `agentnotch-hook` and `agentnotch-win`).
- `fork-seams.txt` gains the Windows section of Appendix A.

`verify-token-free.sh`:
- Windows pattern set (added to the Mac set): `claudeAiOauth`, `api/oauth/usage`,
  `oauth-2025-04-20`, `CredReadW`, `CredEnumerateW`, `read_credentials(`, `probe_credentials(`,
  `run_renewal(`, `maybe_renew(`, `start_login(`, `auth login`, `setup-token`, `sessions\\*.key`,
  `usage::start(`, `claude_auth::`, `doctor::run(`, `watcher::start(`, `usage::profile_dirs`,
  `usage::request_refresh`, `usage::find_cli`, plus the Mac's `.credentials.json` and
  `sessions/*.key`.
- Fork-owned Windows dirs: none of the patterns anywhere (excluding `target`, `gen`,
  `node_modules`), except `ALLOWED_LINES` (e.g. the smoke script's assertion that the doctor
  output never contains `.credentials.json`, listed verbatim).
- Fork-added lines under `windows/` (diff vs merge-base + untracked): none of the patterns.
- Dormant sites, **Claude-specific patterns and files only** (upstream's other providers read
  their own providers' credentials with functions of the same names, e.g. `read_credentials(` in
  `grok.rs`, `cursor.rs`, `antigravity.rs`; those are not Claude token paths and are not
  constrained): `claudeAiOauth`, `api/oauth/usage` and `oauth-2025-04-20` appear only in
  `windows/codenotch/src/usage.rs` and `claude_auth.rs`; `usage.rs`'s own `read_credentials(` and
  `probe_credentials(` are reachable only from `usage.rs` and `doctor.rs` (WD1 + WCLI make the
  doctor unreachable: no other upstream file calls `usage::read_credentials(`,
  `usage::probe_credentials(` or anything in `claude_auth::`); `start_login` only in
  `claude_auth.rs`, and main.rs's WU1c-replaced line must be gone; seams WU1a–e, WD1, WCLI
  present; `usage::start(` absent from main.rs code lines.
- By name, over upstream's files and the fork's glue (`src/agentnotch/`) alike, comment lines
  aside: `usage::` only for the reviewed names (`USAGE_REVIEWED`: the snapshot types and
  `load_persisted`), `doctor::` only in main's `"doctor"` arm (`doctor::run()`, which WCLI
  claims first), `watcher::` only in `watcher.rs` and `doctor.rs`. The glue is where a new call
  into the dormant path would be written, so it is not exempt
  (`windows/tools/tests/verify-token-free.test.sh`).
- GLM's read of `env.ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY` (§4.16) is pinned: those two names
  appear in `windows/codenotch/src/` only in `glm.rs`, and the SHA-256 of `glm.rs`'s
  `claude_code_key` function (its text from `fn claude_code_key` to the next top-level `fn`)
  equals the hash recorded in the script (`GLM_CLAUDE_KEY_PIN`). An upstream merge that changes
  that function fails the script until someone reviews the change and records the new hash.
- The final message names Windows.

### 6.8 Website
- `web/src/app/download/page.tsx`: replace "Agent Notch runs on macOS only for now." with the
  Windows section, labelled **Preview**: requirements, the SmartScreen steps and the Smart App
  Control note (§6.3.1 wording); metadata "for the Mac and Windows" (WP11).
- `web/tests/unit/releases.test.ts`: the six asset names classify as mac/mac/null/windows/null/null;
  `.nsis.zip` → null (optional hardening in `assetKind`: `/\.(nsis|msi)\.zip$/` → null).
- The refresh job is unchanged; `/download` picks up `AgentNotch-<V>-Setup.exe` by itself.

### 6.9 Secrets
None new. `SPARKLE_ED_PRIVATE_KEY` (environment `release`) is now read by `mac`, `keys` and
`sign-windows`. `release-make-keys.sh` is unchanged; its printed warning about the key now says
it also signs Windows updates, and that rotating it needs the old key kept for one bridge
release (§6.4 item 6) (WP11). `SPARKLE_ED_PRIVATE_KEY_PREVIOUS` exists only during such a
rotation.

---

## 7. Test strategy

### 7.1 On the Mac (implementers, every package)
```sh
source <scratch>/rust/env.sh
M=windows/Cargo.toml
cargo test  --manifest-path $M --locked -p agentnotch-proto -p agentnotch-engine -p agentnotch-release
cargo test  --manifest-path $M --locked -p agentnotch-win -p agentnotch-hook       # stubs compile; pure helpers tested
cargo clippy --manifest-path $M --locked --all-targets -p <your crates> -- -D warnings
cargo clippy --manifest-path $M --locked --target x86_64-pc-windows-msvc -p <your crates> -- -D warnings
node --test windows/agentnotch-ui-tests && node windows/scripts/check-ui-scripts.mjs
UPSTREAM_REF=642d329 Scripts/check-seams.sh && UPSTREAM_REF=642d329 Scripts/verify-token-free.sh
```
Never build `-p codenotch` inside the repo on the Mac (it fails on macOS and `tauri-build`
writes `codenotch/gen/`). Engine tests use `tempfile` roots and `testkit` fakes; nothing touches
`~`.

### 7.2 On ubuntu (fork.yml `rust`)
§6.2: fmt, clippy (host + Windows target), tests, no-C-deps tree check, node tests, Rust contract
e2e through Docker Postgres. Among the engine tests that run everywhere: `geometry` vectors at
100 %, 125 %, 150 % and mixed-DPI monitor layouts (a ring on a 150 % monitor whose panel lands on
a 100 % one; work areas with a top and a left taskbar), `pid_guess` and `parse_invocation`
vectors, the command-form and recogniser vectors (paths with spaces, non-ASCII, `~` in 8.3 names,
exec form with bundled versions unknown), the status-line takeover rule vectors (backslash,
`.ps1`, `$env:`, `shell` key, loop-guard spellings), the persisted-file round trips against
Mac-written fixtures, `UsageSource::contract_name`, the environment scrub vector, the pricing
vectors, and `ingress_protocol_compat.rs`.

### 7.3 On windows-2025 (agentnotch-windows.yml) — real behaviour
| Test file | Proves |
|---|---|
| `agentnotch-hook/tests/pipe_roundtrip.rs` | the real hook exe (`env!("CARGO_BIN_EXE_agentnotch-hook")`) against the real pipe server (`agentnotch-win`) on a unique dev pipe name: fire-and-forget delivery incl. a 1 MiB message and immediate client close; PermissionRequest blocks until answered and prints the exact HS§1.7 bytes for allow / always / deny / question / plan / keep planning; `ask` and server-close → empty stdout, exit 0; server killed mid-wait → exit 0 within 1 s; no server → exit 0 in < 200 ms with empty stdout; garbage stdin → exit 0; **a server that accepts and never reads, sent a 1 MiB frame → exit 0 in < 1.5 s with empty stdout** (the watchdog); second server instance on the same name fails (first-instance flag); the pipe's security descriptor has our SID as owner and a protected two-ACE DACL; a v1 frame fixture is accepted |
| `agentnotch-hook/tests/hook_hygiene.rs` | unknown subcommand, no arguments, extra arguments → exit 0, empty stdout; stdout closed before the hook writes (PermissionRequest answered) → exit 0, no panic; a forced panic inside the hook body (an env switch compiled only under `debug_assertions`, so never in the shipped release build) → exit 0; `--exec` accepted; `pid_guess` against real process trees (hook spawned by a fake `claude.exe` directly, through `bash.exe -c`, and through `powershell -Command`) |
| `agentnotch-hook/tests/shell_forms.rs` | the exact string commands the installer writes, run through the runner's Git Bash (`bash.exe -c`) **and** `powershell -NoProfile -Command`, for a profile path with a space (8.3 form), a non-ASCII path and a plain path: each reaches the hook with argv `["hook"]`; the exec form spawned the way Claude Code does it (`CreateProcessW` with `CREATE_NO_WINDOW`, piped stdin, `windowsHide`) reaches it with `["hook","--exec"]`; the status-line wrapper command likewise |
| `agentnotch-hook/tests/statusline.rs` | the wrapper chains a previous command through Git Bash (`echo`), passes its stdout and exit code, kills a hanging previous command's tree at the timeout (Job object), forwards the status message within 0.3 s even while the previous command is still running; the loop guard: a previous command naming our own wrapper (any case, `/` or `\`, 8.3) chains nothing, and a wrapper started with `AGENTNOTCH_STATUSLINE_DEPTH` set chains nothing (no process pile-up after 20 nested attempts); commands with `\`, `.ps1` or `$env:` are refused by the engine rule (vector) and, if forced into `previous.json`, still fail open |
| `agentnotch-hook/tests/console_type.rs` | a test reader (`tests/bin/console-reader.rs`, raw-mode `ReadConsoleInputW` until `\r`, writes what it got to a file) in its own hidden console, and again hosted by `CreatePseudoConsole`: two-phase `type` delivers `héllo wörld ✓` then Return only after `submit`; `abort`, stdin EOF and 2 s of silence leave the text typed with no Return (`typed_not_submitted`); `console-info` reports the reader in the process list and its input mode; typing is refused when the expected window differs, when the start time differs, when a foreign process is attached, when a non-shell ancestor is attached, and when the reader switches to line (cooked) mode |
| `agentnotch-win/tests/win_process.rs` | PEB env read of a child (`Set`/`Unset`/case-insensitive; the variable placed after 100 KiB of other variables → `Set`; a read cut short by a test hook → `Unreadable`, never `Unset`), start times, liveness (exited child → Gone), toolhelp parent links, `exe_path`, `elevated` |
| `agentnotch-win/tests/win_admin.rs` (CI admin only, §6.1) | **integrity levels**: the real pipe server running at medium integrity (`pipe-test-server` spawned with a `SaferComputeTokenFromLevel(NORMALUSER)` token lowered to Medium) accepts the high-integrity hook, and a high-integrity server accepts a medium-integrity hook, both for a PermissionRequest answered end to end; **squatter**: a second local user (`NetUserAdd`, random name and password, deleted afterwards) runs `pipe-squatter` (`CreateProcessWithLogonW`) that creates `\\.\pipe\agentnotch-hook-<our SID>` first with an Everyone DACL; our hook connects, finds the owner is not us, exits 0 without writing (the squatter records zero bytes); our server's start reports "in use" |
| `agentnotch-win/tests/win_job.rs` | `fake-claude.exe` spawning a grandchild: `kill_tree` ends both; stdin/stdout piping; argv round trip of `{"disableAllHooks":true}` and `""` through `CommandRunner` (the fake echoes argv as JSON); the scrubbed environment reaches the child (the fake logs variable names) |
| `agentnotch-win/tests/win_files.rs` | `ensure_private_dir` gives a protected DACL with only the user and SYSTEM (checked with `GetNamedSecurityInfoW`); `write_atomic` keeps the target's DACL (`KeepTargetSecurity`) and applies the private one (`Private`); retries while another thread holds the file without `FILE_SHARE_DELETE` for 300 ms; gives up cleanly (target unchanged, no stage left) when it is held for 2 s; `Expect::Same` detects a change, `Vanished` a deletion; a read-only target → `ReadOnly`; `create_exclusive`; `identity` stable across rename; `is_reparse` true for a junction (`mklink /J`) and a directory symlink when creatable |
| `agentnotch-win/tests/win_install.rs` (engine installer + the real `SecureFiles`) | full install → uninstall in a temp profile: settings.json with CRLF + BOM + other tools' hooks + a statusLine; after install the bytes outside our splice are identical, the hook copy exists, backups are private; a running hook copy (spawned and held) is replaced with no instant where `<cfg>\hooks\agentnotch-hook.exe` is missing (a watcher thread polls it every 1 ms) and `.old`/`.new` leftovers are swept by the next pass and by uninstall; **fault injection**: a thread holds settings.json without `FILE_SHARE_DELETE` during the replace, and another deletes it between plan and replace; a reader thread polling every 1 ms never sees settings.json missing, and at the end it holds the old or the new bytes, never `{}`-plus-hooks; a symlinked settings.json is written through to its target with the stage beside the target; uninstall restores the original bytes exactly |
| `agentnotch-win/tests/win_http.rs` | the real ureq/schannel client against a local test server: the contract headers (`Accept`, `User-Agent: AgentNotch/<v>`, `Content-Type`, `Authorization`), no cookie sent back after a `Set-Cookie`, gzip bodies decoded, the 30 s timeout surfaces as `HttpError::Timeout` (with a shortened timeout), a refused port as `Connect`, redirects not followed to another host |
| `agentnotch-win/tests/win_toast.rs` | toast XML builder output; `CreateToastNotifierWithId` without a shortcut → `Unavailable` without panicking |
| `codenotch` unit tests | upstream's suite with the seams; glue tests: `cli::run` claims `doctor`, `install-hooks`, `uninstall-hooks`, `control`, `inspect-accounts`, and returns `None` for any argv holding an `agentnotch:` argument; deep-link argv rules (one argument, `agentnotch://` prefix; a `\|`-split URL ignored); `hooks_switch_set(true)` refuses without consent; the upstream-usage projection never says `needsAuth`; the panel keyboard gate stays shut until a foreground confirmation (`panel.rs` with a fake window service); `refresh_provider` routes `claude-acct-*` to the glue |

### 7.4 Sealed self-test and snapshots (in the smoke test)
`agentnotch.exe` with `AGENTNOTCH_SAFE_MODE=1` (data in `%APPDATA%\Agent Notch Sealed`, deleted
before and after each run):
- `AGENTNOTCH_PANEL_SELF_TEST=1 AGENTNOTCH_SELF_TEST_OUT=<json>`: for each edge (right, left,
  top, bottom, floating) it opens the panel on the sealed ring, reads the window rect, styles and
  z-order, runs an `an_call('snapshot')` round trip from inside the panel page **and** the notch
  and settings pages, collects every `securitypolicyviolation` and console error from all three
  pages, evaluates the layout invariants of §5.6 in each page (badges inside the pill and clear of
  the % label, interactive elements inside hot rects, no overflow at 400 and 520 px), checks
  that the panel's keyboard gate is shut after an auto-open (no confirmation arrives) and opens
  after a simulated confirmation, then writes a JSON report and exits 0 (1 on any failure). The
  smoke script asserts: every rect inside its monitor's work area, the tail offset inside the
  card's corners, `WS_EX_TOPMOST` set, `WS_EX_NOACTIVATE` set for the auto-open case, zero CSP
  violations and page errors, every invariant true, round trips OK. It runs three times: at
  100 %, and with `--force-device-scale-factor=1.25` and `1.5` (§5.6).
- `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>`: renders fixture states (UI§10's inventory: panel every
  state, needs-you, busy, filtered, undo, banners, consent, empty; chat approval, plan, question,
  tasks, composer, terminal-only, no route; settings first run, full, cloud signed in/out; notch
  per edge with badges and folded marks) through WebView2 `CapturePreview` into PNGs, then
  exits. The smoke script requires every expected file, non-empty, compares each with its
  baseline in `windows/agentnotch-ui-tests/baselines/` (§5.6) and fails on a difference beyond
  the tolerance, writing a diff image; CI uploads snapshots and diffs.

### 7.5 Installer smoke test (`windows/scripts/agentnotch-smoke.ps1`, WP11)

Mechanics. Every CLI call is `Start-Process agentnotch.exe -ArgumentList … -Wait -PassThru`;
the exit code comes from the process object and the text from `<data>\<cmd>.log` (the exe is
GUI-subsystem, §4.14). The UI is driven with no backdoor: the app is launched with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<free port>` (a WebView2
feature; set only by this script) and `windows/scripts/smoke/cdp.mjs` (Node 22, built-in
`WebSocket`, no dependencies) attaches to the `notch`, `settings` and `agentnotch-panel` pages,
evaluates `invoke(…)` exactly as the pages do, clicks elements by their `data-an-*` attributes
and reads rendered state. Hook entries are run by `smoke/run-hook.mjs`, which spawns them the way
Claude Code does (exec form: `child_process.spawn(command, args, {windowsHide: true})`; string
form: through `bash.exe -c` **and** `powershell -NoProfile -Command`). `P` =
`$env:RUNNER_TEMP\profile`, the app runs with `USERPROFILE=P`, `AGENTNOTCH_NO_NOTIFICATIONS=1`
and `FAKE_CLAUDE_LOG=$env:RUNNER_TEMP\fake-claude.log`.

0. **Preflight**: the checks of §6.1 step 2 again; hash the Known-Folder profile's `.claude*`
   entries and `.claude.json` (`[Environment]::GetFolderPath('UserProfile')`, which `dirs` and
   upstream code resolve without `USERPROFILE`) and the runner's `%APPDATA%\Agent Notch*` and
   `%LOCALAPPDATA%\com.rivantmedia.agentnotch` (all expected absent); the last phase requires
   them unchanged, so a regression that writes to the runner's real profile fails.
1. Install `/S`; poll ≤ 60 s for `%LOCALAPPDATA%\Agent Notch\agentnotch.exe`,
   `agentnotch-hook.exe`, `uninstall.exe`; uninstall key DisplayName `Agent Notch`,
   DisplayVersion V, Publisher `Rivant Media`; Start-menu shortcut; `HKCU\Software\Classes\agentnotch\shell\open\command`
   = `"<install>\agentnotch.exe" "%1"`.
2. Build `P`: `P\.claude\settings.json` (CRLF, BOM, a foreign hook, a status line whose command
   is a plain `echo` and passes the takeover rule), `P\.claude-work\settings.json` with a
   status line `node C:\tools\sl.js` (must be left alone), `P\.claude.json` with a fixture
   `oauthAccount` (made-up UUIDs), `P\.claude\projects\…\<uuid>.jsonl`, `P\.claude-work\` with
   its own `.claude.json`; `P\.local\bin\claude.exe` = `fake-claude.exe` (`FAKE_CLAUDE_VERSION`
   `2.1.282 (Claude Code)`, `FAKE_CLAUDE_USAGE` a UsageParser fixture). Hash every file in `P`.
3. `doctor`: matches `Agent Notch doctor v<V>`, `updates: off (built from source)` or
   `updates: on … key=<KeyId> signed-version=required`, `accounts: 2`, `hooks: consent=unasked`,
   `deep-link: registered`, `pipe: … no instance running`, `elevated: app=`, `smart-app-control:`,
   `support: …\AppData\Local\com.rivantmedia.agentnotch\Claude`; never contains
   `.credentials.json`, `claudeAiOauth` or `accessToken`.
4. Sealed self-test (three scales) and snapshots with baselines (§7.4); `Agent Notch Sealed`
   removed afterwards; `%APPDATA%\Agent Notch` still absent (the sealed run did not touch it).
5. **Before consent.** Real launch; wait 30 s; alive; `control status`: `transport: listening`,
   `accounts: 2`, `hook_consent: unasked`, `readings: 2` (the probe ran against fake-claude);
   `P` byte-identical (no `hooks\`, no `.bak`, no settings change); the fake-claude log shows
   only probe runs with the exact probe argv, cwd `<support>\usage-probe`, and none of the
   scrubbed variable names; `<support>` exists with a protected DACL (user + SYSTEM only) and
   holds no `cloud-*` file; no `Run` value; `run.log` has `an: hub started` and
   `an: pipe listening`.
6. **Deep link with nothing pending**: `Start-Process "agentnotch://auth-callback?code=smoke"` →
   within 5 s `run.log` has `an: deep link ignored (no sign-in pending)`; one `agentnotch.exe`.
7. **Turn on** (CDP): open Settings (`invoke('open_settings')` from the notch page), click the
   consent card's "Turn on". Within 10 s: both run folders hold our entries in the form the facts
   file allows (string form today: `P/…/agentnotch-hook.exe hook`, or its 8.3 form when `P`
   contains a space); the JSON of every other key is unchanged and CRLF + BOM are kept;
   `P\.claude\hooks\agentnotch-hook.exe` exists; backups exist with a private DACL;
   `P\.claude`'s status line is wrapped and `previous.json` holds the original;
   `P\.claude-work`'s status line is untouched and Settings shows "Status line left alone".
8. **Each entry exactly as written**: for every hook entry in `P\.claude\settings.json`,
   `run-hook.mjs` runs it (through both shells for string form) with a PreToolUse stdin
   (`CLAUDE_PID` = a live dummy process, `CLAUDE_CONFIG_DIR=P\.claude`) → exit 0 in < 2 s,
   `control status` counts the session. Then PermissionRequest stdins, one per answer: CDP waits
   for the request in the panel (`invoke('an_call',{method:'panel_open',…})`), waits out the
   0.35 s AnswerGate, clicks Allow / Always / Deny / a question chip / Approve plan / Keep
   planning; the hook's stdout equals HS§1.7's bytes for that answer, exit 0. The wrapped status
   line, run as written with a status JSON on stdin, prints the original command's output and
   the ring's `readings` count is unchanged or up.
9. **Turn off** (CDP, the hooks switch): every `settings.json` in `P` is byte-identical to its
   original; `hooks\` holds nothing of ours; the only new files in `P` are our `.bak` backups.
10. **Sign-in and sync**: restart the app with `AGENTNOTCH_WEB_URL=http://127.0.0.1:<port>`,
    `AGENTNOTCH_DEV=1`, `AGENTNOTCH_DEV_BROWSER_LOG=<file>`, with `smoke/fake-website.mjs`
    serving `/api/app/v1/config` (from `web/contract/fixtures/config.json`, `supabaseUrl` = itself),
    `/auth/v1/token?grant_type=pkce`, `/auth/v1/logout`, `/api/app/v1/me`, `/api/app/v1/sync`.
    CDP clicks "Sign in"; the authorize URL appears in the log file (S256 challenge, redirect
    `agentnotch://auth-callback`); `Start-Process "agentnotch://auth-callback?code=smoke"` →
    `control status` says `cloud: signed_in` within 10 s; `cloud-session.json` exists with a
    private DACL. Consent before upload: no `/sync` request has arrived. CDP turns sync on → a
    `/sync` POST arrives whose body `smoke/contract-shape.mjs` validates against
    `web/contract/fixtures/sync-request.json`'s key sets and types (sorted keys, explicit nulls,
    `.mmmZ` dates, `accountKey` 64 hex). CDP signs out → `/logout` called, switches off,
    `cloud-session.json` gone.
11. **Update over a running app**: consent again (step 7) so hooks are installed; hold a
    PermissionRequest from `P`'s hook copy; run the installer with `/UPDATE /P /R`: the held
    hook exits 0 with empty stdout within 2 s of the app stopping; the installer finishes; the
    app is running again (`/R`); `P`'s settings.json and hook copy are unchanged.
12. **Manual reinstall path**: run `uninstall.exe /S` (what the reinstall page's default
    "Uninstall before installing" does, minus its UI: no `/UPDATE`, checkbox unticked), then
    install `/S` again: `P`'s settings.json still holds our entries, byte-identical to before;
    consent survived (`control status` after launch: `hook_consent: granted`).
13. **Fail-open**: with the app stopped (`control quit`), copy the hook exe to a temp folder and
    run it with garbage stdin and with a PermissionRequest stdin: exit 0 in < 1 s, empty stdout.
14. `autostart on` → Run value `Agent Notch` = `"<exe>" --silent`; `autostart off` → gone.
15. **Uninstall with `/S /REMOVEHOOKS`**; poll ≤ 60 s until the exe and the uninstall key are gone
    (the NSIS uninstaller re-launches itself, so `-Wait` returns early); Run value and
    `Classes\agentnotch` gone; every `settings.json` in `P` byte-identical to its original, no
    hook copy left; the Known-Folder profile and the runner's other hashed locations unchanged.
Never runs `claude` (only `fake-claude.exe`, whose log proves it); never touches anything outside
`P`, the install dir, the app's own data and the HKCU keys named above; stops only its own
processes by PID (never `taskkill /IM`).

### 7.6 Fixtures
- `web/contract/fixtures/*` read in place from Rust
  (`concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/contract/fixtures/")`); never copied.
- `agentnotch-engine/tests/ui-contract/*.json`: snapshot, settings, chat, calls, events,
  rebrand vectors — shared by Rust and node tests.
- Mac vectors: ported inline from the Swift suites the reports list (HS§13, AU§16, CL§12);
  each Rust test names its Swift origin in a comment.
- `Packages/ClaudeControl/Tests/ClaudeControlTests/Fixtures/model-pricing-vectors.json`: shared
  by the Swift and Rust pricing tests (§4.11).
- `agentnotch-engine/tests/fixtures/claude-code-facts.json`: the facts job's committed output
  (§6.2).
- `agentnotch-engine/tests/fixtures/mac-files/*.json`: Mac-written `accounts.json`,
  `usage-state.json`, `review-state.json`, `control-settings.json` and `cloud-*.json` for the
  persisted-format round trips (made-up data).
- `agentnotch-proto/tests/fixtures/v1/*.json`: frames of protocol 1, kept forever.
- Sample homes under `agentnotch-engine/tests/fixtures/home-*` (made-up identities only), used by
  engine tests and copied by the smoke script.
- Sealed fixtures: `hub/sealed_fixture.rs` (WP0) serves the ui-contract fixtures so the glue and
  the pages can be built and self-tested before the engine exists; WP7's `hub/sealed_demo.rs`
  (ports of `SampleLayout`, `SampleSessions`, `SampleData`) replaces it, and its snapshot must
  equal the committed ui-contract fixture.

### 7.7 What CI cannot prove
Claude Code's own Windows behaviour at run time (the facts job reads its code but never runs it:
`CLAUDE_PID` actually exported, `sessions\<pid>.json` actually written, Desktop paths,
`get_usage` on Windows), real Windows Terminal tab selection, foreground rights on a user's
desktop (the runner is an admin session with UAC off), toasts on screen, a Bun raw-mode reader's
paste detection. §9 lists how each is contained. A hermetic run of the real Claude Code on the
runner would close most of the first group; it needs the maintainer's permission (§9, Q3).

---

## 8. Work packages

Every package: branch off `windows-port` after WP0 lands, own only its files (§2.1), keep all
§1.8 invariants, and pass its commands plus `check-seams.sh` and `verify-token-free.sh` with
`UPSTREAM_REF=642d329` before handing back. "Proven on Windows" means green in
`agentnotch-windows.yml` on the branch.

**WP0 — Scaffold, interfaces, foundations, seams (lands first; lead; may be split between two
agents: WP0a scaffold/seams/scripts, WP0b foundations/model/fixture hub, both land before any
other package starts).**
Owns: `windows/Cargo.toml` seam, all five fork crates' `Cargo.toml`, `agentnotch-proto`
(complete: framing, messages, `parse_invocation`, `pid_guess`, permission output, v1 fixtures);
engine `lib.rs`, `platform.rs`, `runtime_types.rs` (every type of §3.4 written out),
`model/{ids,hook,accounts,usage,sessions,requests,cloud,ui}.rs` (every type of §3.3 and §3.6 in
full, enums complete), `core/{roots,flags,sealed,time}.rs`, and **complete, tested
foundations** other packages' tests depend on: `core::paths` (both `PathStyle`s, `normalize`,
`key`/`display`, the slug, the `;` split, config folder from a transcript path),
`core::json_scan` (the JSON field scanner), `core::claude_json` (the `.claude.json` reader with
its (mtime, size) cache and last-good-parse rule, AU§4.1), `core::atomic` (the unix
`SecureFiles` used by `testkit`); `persist/*` skeletons with the Mac-file fixtures;
`hub/api.rs` (Hub, Call, HubEvent) and `hub/sealed_fixture.rs` (a `Hub::sealed` that serves the
ui-contract fixtures, answers every `Call` with fixture data and emits `an:*` events, so the glue
and the pages can be built and self-tested before the engine exists); `testkit/{mod,clock,files}.rs`,
`tests/ui-contract/*.json` + round-trip test; stubs for every other module path of §2.1
(compiling, empty); `agentnotch-win` `lib.rs` + `stub.rs` + trait impl skeletons returning "not
implemented"; hook exe skeleton (parses argv by hand, exits 0); release tool skeleton; the glue
module skeleton (all seam entry points, `an_call` forwarding, `setup` starting the fixture hub
when sealed); UI stubs (`notch.js` with no-op hooks returning `null`/`false`, blank
`panel.html`, `settings.js` rendering a "Claude Code" heading); every seam of §2.5 incl. WCSP
after the audit of upstream's three pages; `capabilities/agentnotch.json`;
`nsis/agentnotch-hooks.nsh` stub; `Scripts/fork-seams.txt` Windows section, `check-seams.sh` and
`verify-token-free.sh` extensions (final); `agentnotch-windows.yml` skeleton (steps 1–11) and
fork.yml `rust` job skeleton.
Acceptance: `cargo check --workspace --locked` green on windows-2025; fork crates' tests (the
foundations' ported vectors: `AccountPathsTests` both styles, the JSON scanner's and
`.claude.json` reader's vectors), clippy and Windows-target clippy green on the Mac and ubuntu;
upstream `cargo test -p codenotch` green on Windows; `check-seams.sh`/`verify-token-free.sh`
green; ui-contract round trip green; the app starts sealed on Windows CI and shows its notch.

**Milestone M1 — sealed UI green (WP9 + WP10 against the fixture hub).** Right after WP0, before
any engine package lands: the non-activating panel, topmost ordering, the keyboard gate,
CapturePreview snapshots, CSP under every page and deep-link forwarding are the least provable
parts, so they are proven first. M1 = the sealed self-test (§7.4) green on all edges and three
scales, snapshots produced and looked at, baselines committed.

**WP1 — Hook exe, pipe transport, ingress, console helper.** Depends on WP0.
Owns: `agentnotch-hook/**`, `agentnotch-win/src/{pipe_server,console,sid,integrity}.rs`,
`agentnotch-win/src/bin/{pipe-test-server,pipe-squatter}.rs`, `tests/win_admin.rs`, engine
`ingress/**`, `model/hook.rs`, `testkit/transport.rs`, proto maintenance.
Acceptance: proto vectors; ingress unit tests (HS§4 incl. ToolUseIdCache, held/release/peer-gone,
ignored sessions, protocol compatibility); Windows tests `pipe_roundtrip.rs`, `hook_hygiene.rs`,
`shell_forms.rs`, `statusline.rs`, `console_type.rs`, `win_admin.rs` green.
Commands: §7.1 + Windows CI.

**WP2 — Hook installer, settings.json, status line takeover.** Depends on WP0.
Owns: engine `hooks/**` (incl. `hooks/facts.rs`), `core/{atomic,settings_doc}.rs` (atomic after
WP0), `agentnotch-win/src/files.rs`, `agentnotch-win/tests/{win_files,win_install}.rs`.
Acceptance: ports of `HookInstallerTests`, `A2_SettingsDocumentTests` (+ CRLF/BOM vectors),
`StatusLineScriptTests` (command forms: unquoted, 8.3, not possible; exec form gating with
bundled and unknown versions), recogniser vectors (exec/string, both separators, case, 8.3,
`--exec`), the status-line takeover rule and loop-guard vectors, the `Vanished` abort,
`remove_codenotch_hooks`; `win_files.rs`, `win_install.rs` (fault injection included) green on
Windows.

**WP3 — Accounts, processes.** Depends on WP0.
Owns: engine `accounts/**`, `core/{paths,json_scan,claude_json}.rs` (after WP0), `model/accounts.rs`,
`persist/accounts.rs`, `testkit/process.rs`, `agentnotch-win/src/{process,paths}.rs`,
`tests/win_process.rs`.
Acceptance: ports of `AccountRegistryTests`, `PP_*` layout/identity tests,
`A2_AccountNamingTests`, `A3_RingIdentityTests`, `Fix_ForgottenAccountTests`,
`Fix_AccountOwnLabelTests`; Windows spelling-collapse vectors (no login conflict); `folder_logins`
and `backfill_folders` vectors (CL§7.2, link and mirrored-folder rules); `accounts.json` round
trip; `win_process.rs` green.

**WP4 — Usage.** Depends on WP0.
Owns: engine `usage/**`, `model/usage.rs`, `persist/usage.rs`, `testkit/runner.rs`,
`agentnotch-win/src/job.rs`, `src/bin/fake-claude.rs`, `tests/win_job.rs`.
Acceptance: ports of `UsageParserTests`, `A3_UsageStoreTests`, `PP_UsageTests`,
`UsageProbeTests` (scripted runner), Desktop Simple Cache reader tests with synthetic cache files
(+ blockfile detection → UnsupportedFormat), binary locator (Windows candidate order, Desktop
exclusions, `.cmd` → node resolution) and bundled-version discovery against temp trees,
`probe_folder`, the environment-scrub vector, `UsageSource::contract_name`, `usage-state.json`
round trip; `win_job.rs` green.

**WP5 — Sessions pipeline, review queue, attention tracking, chat history.** Depends on WP0
(types), WP1 in practice (decoding).
Owns: engine `sessions/**`, `review/**`, `attention/{tracker,news}.rs`,
`model/{sessions,requests}.rs`, `persist/review.rs`.
Acceptance: ports of `SessionStoreFlowTests`, `SessionCoreRegressionTests`,
`A1_SessionStoreRegressionTests`, `PP_SessionTests`, `BackgroundWaitTests`,
`Fix_FailedTurnTests`, `SessionTaskListTests` (incl. deleted and failed creates),
`SessionAttentionTests`, `Fix_AttentionNewsTests`, `A1_AttentionAndReviewTests` + review suites,
`TranscriptTests`, `A1_TranscriptParserTests`, `A1_InterruptWatcherTests`,
`DesktopHostedSessionsTests`; chat patches (reset, then only changed items; images by id);
pid-less sessions; `review-state.json` round trip; an end-to-end test driving `MemoryTransport`
frames from the Mac simulator's scripts (ported `simulate-sessions.py` scenarios as JSON).

**WP6 — Control and OS integration.** Depends on WP0 (views and `PanelState` are WP0 types).
Owns: engine `control/**`, `attention/policy.rs`, `testkit/terminal.rs`,
`agentnotch-win/src/{focus,uia,toast,sound,visibility}.rs`, `tests/win_toast.rs`.
Acceptance: ports of `A3_NotificationTests`, `ChatAndNotificationTests`,
`Fix_OffPanelPreviewTests`, `A3_FocusAndMessagingTests`, `Fix_MessagingSafetyTests`,
`TerminalFocusTests`, `ClaudePanelPolicy` (`auto_close_deadline`); `reactions` with
`PanelState` (never over an open panel, never with auto-open off); `message_safety` Windows
vectors (shell chain, line input, `typeReplies` off); host classification and WT tab-match
vectors; toast XML vectors; permission-response vectors (HS§1.7 via proto).

**WP7 — Hub, runtime, projections, geometry, settings, sealed demo, doctor.** Depends on WP0;
integrates WP1–WP6, WP8 as they land; replaces WP0's fixture hub without changing the glue.
Owns: engine `hub/**` (incl. `api.rs` once WP0 hands it over), `geometry/**`,
`attention/{rows,sections}.rs`, `core/{settings,rebrand}.rs`, `model/ui.rs`,
`tests/ui-contract/*` (maintains).
Acceptance: runtime tests with the full `testkit` platform (hook frame → snapshot → answer →
response frame; probe → ring reading; consent → install job; two-phase typing checkpoint
refusing when a request appears; `SetSetting` from the cloud thread; quit and update-exit release
held requests); `SessionRowContent`/`SessionSections` and `activityRow` (`card`) ports;
`ClaudePanelGeometry` port with the DPI vectors of §7.2; upstream-usage projection tests (never
`needsAuth`; `stale` from the engine rule); `LiveBatch` placement with `PLACEMENT_GRACE`; sealed
demo snapshot equals the committed ui-contract fixture; doctor text golden test; rebrand vectors.

**WP8 — Cloud sync.** Depends on WP0 (CloudState, `CloudDeps`, platform traits) and on the Mac
commit that adds `ModelPricing.swift` (§4.11).
Owns: engine `cloud/**`, `model/cloud.rs`, `testkit/http.rs`,
`agentnotch-win/src/{http,browser,device}.rs`, `tests/win_http.rs`, `Scripts/cloud-contract-e2e.sh`,
`tests/cloud_contract_e2e.rs`, `tests/cloud_pricing.rs`, the shared pricing vectors and the
Swift `ModelPricingTests` case that reads them.
Acceptance: every CL§12 port (contract, auth, ledger, scanner v4 with costs, recorder,
summarizer, sync suites incl. the "morning" scenario with the pricing commit's expectations) +
Windows path/scrub vectors + pricing vectors in Swift and Rust; the dev browser override honoured
only with `AGENTNOTCH_DEV=1`; `AGENTNOTCH_CONTRACT_APP=rust Scripts/cloud-contract-e2e.sh`
green on ubuntu; `win_http.rs` green on Windows.

**WP9 — App glue and windows.** Depends on WP0 (the fixture hub); starts at once (M1).
Owns: `windows/codenotch/src/agentnotch/**`,
`agentnotch-win/src/{window,hotkey,clipboard,capture,shell}.rs`.
Acceptance: glue unit tests (§7.3 last row); M1; later, with WP7's hub, the sealed self-test
still green unchanged; deep-link forwarding, the duplicate-instance exit, `control` and the
update hook work in the smoke test.

**WP10 — Web UI.** Depends on WP0 (ui-contract fixtures); starts at once (M1).
Owns: `windows/codenotch/ui/agentnotch/**`, `windows/agentnotch-ui-tests/**` (incl. baselines).
Acceptance: node tests: page-order vm test (no global clash), wrapped-globals-exist test,
rendering from every ui-contract fixture (row detail kinds, action bars, AnswerGate timing with a
fake clock, question answer format, compact mode, sections/folding, badges label and placement
per edge, resting marks layout, markdown escaping, tool result views, settings sections and
consent card, the keyboard gate, chat patches, the `stale` class, `card` rows), hostile strings
through every renderer, rebrand vectors over `settings.html`; snapshots looked at and baselined.

**WP11 — Release, CI, packaging, website, docs.** Depends on WP0; the smoke phases after
consent need WP7 + WP9, sign-in needs WP8.
Owns: `agentnotch-release/**`,
`.github/workflows/{agentnotch-windows.yml,fork.yml,release.yml,claude-code-facts.yml}`,
`windows/scripts/agentnotch-{build,smoke}.ps1`, `windows/scripts/smoke/*`,
`windows/scripts/check-claude-code-facts.mjs` and its output
`agentnotch-engine/tests/fixtures/claude-code-facts.json`, `windows/tools/**`,
`nsis/agentnotch-hooks.nsh`, `Scripts/bump-version.sh`, `web/src/app/download/page.tsx`,
`web/tests/unit/releases.test.ts`, README.md and CLAUDE.md Windows sections (build, first use,
privacy incl. the GLM note, uninstall and hooks, dev switches, release contract additions,
Windows facts), `Scripts/release-make-keys.sh` wording.
Acceptance: release tool tests (§6.4); `actionlint` clean; the facts job produces the file; the
branch workflow green end to end with every smoke phase of §7.5; web `npm test` + `npm run
typecheck`; after merge to main a **dry run** of Release produces all six files, the signature
verifies, and the doctor inside the release-mode smoke test shows the derived key id.

**Dependency order.** WP0 → {WP1, WP2, WP3, WP4, WP8, WP9, WP10, WP11} in parallel (M1 = WP9 +
WP10 on the fixture hub) → WP5 (needs WP1 decode in practice) and WP6 → WP7 integrates and
swaps the fixture hub for the real one → the smoke phases after consent go green → final
integration on `windows-port` (lead) → merge.

**Ship gates for 1.1.0.** All of: `fork.yml` and `agentnotch-windows.yml` green on
`windows-port` with every §7.3 test and every §7.5 phase; M1's baselines committed; the facts
file committed (its content decides `EXEC_FORM_MIN`; an empty or inconclusive file just keeps
string form); the conservative defaults in place (`typeReplies` off, `autoOpen` never); the
Windows section labelled preview. If a feature is not green by then, the lead ships it in its
honest disabled state (the Mac's own "not available" copy) rather than holding the release, and
lists it in the release notes; the Mac never waits on Windows (`skip_windows` remains the last
resort, §6.3).

**Ship sequence.** (1) All packages merged on `windows-port`; `agentnotch-windows.yml` and
`fork.yml` green; PR reviewed. (2) Merge to main **without** changing `VERSION` (Release does not
trigger: its paths are `VERSION` and the Sparkle key). (3) Actions › Release › Run workflow on
main with *Dry run*: all jobs green, artifact holds the six files. (4) Push the
`bump-version.sh 1.1.0` commit to main: Release publishes 1.1.0 with Mac and Windows, the website
refreshes, `/download` offers `AgentNotch-1.1.0-Setup.exe`. (5) Optional: commit
`Scripts/tauri-update-public-key.txt` with the key from the run summary to pin it.

---

## 9. Risks and open questions

| # | Risk / question | Recommendation |
|---|---|---|
| R1 | Claude Code's Windows behaviour we rely on is read from its code, not run: exec form support per version (and what an older version does with an unknown `args` key), `CLAUDE_PID`/`CLAUDE_CONFIG_DIR` in hook env, `sessions\<pid>.json`, `procStart` format, Desktop `claude-code-sessions` path. | Conservative gates: string form (parses in Git Bash and PowerShell) by default; exec form only when every Claude Code on the PC, bundled copies included, is ≥ `EXEC_FORM_MIN`, which only the committed facts file (§6.2, a CI job that downloads and greps Claude Code packages, never runs them) can set. Fallbacks already in the engine (pid walk or `null`, session keys, hooks-only sessions, "unsure" attribution). A hermetic run of the real Claude Code would prove the rest (Q3). |
| R2 | Which shell runs string-form hooks and `statusLine` on Windows (`e.shell ?? R1()`: Git Bash or PowerShell), and what Claude Code's bash-form transform (`yun`) does. | Hook and status-line commands use forms that parse in both shells (CI runs them through both); someone else's status line is wrapped only when it is safe to re-run with Git Bash, else left alone with a visible reason; the facts job decodes `R1()` and `yun`, and the rule is loosened only with that evidence. |
| R3 | Claude Desktop's cache on Windows may be Chromium blockfile, not Simple Cache. | Ship Simple Cache only, detect blockfile and say so; the doctor reports the format; implement blockfile only if users report it (the other three sources work without it). |
| R4 | `WriteConsoleInputW` into Claude Code (Bun or Node raw mode) might be treated as a paste or mangle non-BMP text; typed text might reach a shell that reads the same console. | Opt-in (`typeReplies` off by default); two phases with a fresh engine re-check before Return; separate delayed Return (as on the Mac); UTF-16 units one record each; only Claude, its descendants and its direct shell chain may share the console, and the console must be in raw mode; CI proves plain console + ConPTY delivery and every refusal with a test reader; outcome copy "Typed but not submitted…" covers the rest. |
| R5 | Windows Terminal exact-tab selection via UIA/title matching is fragile (spinner glyphs in titles, renamed tabs, panes). | Best effort, RaisedOnly otherwise, honest copy; never guess between two matching tabs. |
| R6 | Foreground rules: `SetForegroundWindow` after a click that WebView2's child window (another process) received may be refused; a panel showing a caret while keys still go to the terminal could answer a prompt there. | The keyboard gate: composer and shortcuts stay inert until the glue confirms the panel is the foreground window; auto-open is off by default; refused foreground → flash + "Click to type". The self-test proves styles, z-order and the gate; real-desktop behaviour is revisited with first user reports. |
| R7 | Unsigned binaries: SmartScreen on the installer; antivirus heuristics on `agentnotch-hook.exe` copies run on every tool call from `%USERPROFILE%\.claude*\hooks`; **Smart App Control** (Windows 11) blocks unsigned apps outright, with no "Run anyway": the installer cannot run there, and hook copies would fail to start on every event. | Release notes and `/download` say so; the doctor prints `smart-app-control:`. **Authenticode is a priority, not an option** (Q4): Azure Trusted Signing or a certificate, applied through `bundle.windows.signCommand` to the app, the installer and `agentnotch-hook.exe` in the build job. Not needed for the updater (minisign). |
| R8 | Roaming profiles would carry cloud identity files (refresh token, install secret, device id) to another PC. | Closed by placement: `<support>` is in `%LOCALAPPDATA%` (never roams, §1.5); only upstream's non-secret config roams. |
| R9 | Restructuring the live `release.yml` could break Mac releases. | Keep every Mac step byte-identical inside the `mac` job; the ship sequence starts with a dry run on main; `skip_windows` exists as an emergency switch (with a loud warning when Windows has shipped before). |
| R10 | Held PermissionRequest pipes across an app update or crash. | The updater calls `std::process::exit(0)` right after starting the installer; the glue's `on_before_exit` (WUP2) stops the hub first, which closes held connections without an answer and saves the stores; the installer's own process check covers the rest. Hooks then exit 0 and Claude Code's terminal prompt decides. Covered by the pipe test (server killed mid-wait) and smoke phase 11 (update over a running app with a held request). |
| R11 | PEB offsets are undocumented; an x64 app reading an ARM64 Claude process. | Offsets pinned to x64 and tested on CI; any failure is `Unreadable` (only used for linked `sessions` folders, rare on Windows). ARM64-native build: open question (x64 runs emulated on ARM64 Windows). |
| R12 | Upstream moves fast in `windows/` (merge conflicts in `main.rs`, `notch.html`, `Cargo.lock`). | Seams are one line each at stable anchors, checked by `check-seams.sh`; `Cargo.lock` is regenerated per §2.1's rule after a merge; the wrapped-globals node test catches renamed upstream JS functions. |
| R13 | Two notches and two identical tray icons when the official Codenotch is also installed. | Documented; **open question:** a distinct Agent Notch icon for both platforms. |
| R14 | Tauri config: `plugins.deep-link` without the deep-link plugin crate. | The bundler reads it directly (verified in the pinned tauri-cli 2.11.4 `src/interface/rust.rs:888-900` and in 2.12.0); the smoke test checks the registry key. Fallback: write the key from the NSIS hook. |
| R15 | WSL and Claude Parallel Profiles on Windows are out of scope. | Settings footnote; the classifier stays ported and dormant; a later phase can add a WSL relay. |
| R16 | Upstream's Claude-desktop activity probe (`activity.rs`) spawns PowerShell every ≥ 60 s. | Left as upstream ships it (not a Claude token path); the fork's rings ignore it. Revisit if it shows up in user CPU complaints. |
| R17 | Integrity levels: an elevated terminal or an elevated app. | The pipe's checks work across levels (§1.4) and CI proves both directions at medium ⇄ high (`win_admin.rs`); what elevation still blocks (typing, UI Automation, environment reads of an elevated Claude) degrades to refusals and RaisedOnly with honest copy; the doctor reports elevation. |
| R18 | A settings.json rewrite racing antivirus or the user's editor. | One atomic rename, never `ReplaceFileW`; a vanished file aborts the pass; fault-injected tests (§7.3 `win_install.rs`); at every instant the file holds old or new bytes. |
| R19 | Old hook copies speaking an older protocol after an update, or a newer one after a downgrade. | Lenient decoding both ways, optional-only additions, v1 frames kept as fixtures forever (§1.4 Versions). |
| Q1 | Should the uninstaller remove this app's hooks from users' settings.json? | Only on an explicit request: "Delete the application data" or `/REMOVEHOOKS` (§6.5). The "Uninstall before installing" step of a manual upgrade runs the old uninstaller without `/UPDATE`, so removing hooks on every uninstall would strip and rewrite every settings.json on each upgrade. Entries left behind exit 0 at once while the app is gone; Settings, the release notes and the README say how to remove them. |
| Q2 | Pin the Windows key in the repo? | Yes, after 1.1.0 publishes (`Scripts/tauri-update-public-key.txt`); the `keys` job enforces it when present. |
| Q3 | **For the maintainer:** may CI run the real Claude Code, hermetically? | CLAUDE.md says "Never run `claude`", so not without permission. The proposal: an optional job on the Windows runner with a temporary `USERPROFILE`, a fake `ANTHROPIC_API_KEY` and `ANTHROPIC_BASE_URL` pointing at a local fake Messages API that returns a `tool_use`; no login token exists anywhere on the runner. It would prove the hook forms, the `CLAUDE_PID`/`CLAUDE_CONFIG_DIR` export, `sessions\<pid>.json` and a PermissionRequest end to end, and let `EXEC_FORM_MIN` rest on behaviour rather than on code reading. Until permitted, the facts job (§6.2) and the conservative forms stand. |
| Q4 | **For the maintainer:** Authenticode signing (Azure Trusted Signing, about $10 a month, or a certificate). | Recommended before promoting Windows beyond preview: it removes SmartScreen friction, makes Smart App Control PCs usable, and reduces antivirus trouble with hook copies run on every tool call. The pipeline change is small (`signCommand` + the hook exe in the build job); it needs an account and a secret only the maintainer can create. |

---

## Appendix A — `Scripts/fork-seams.txt`, Windows section (verbatim, tab-separated)

The header's ALLOW description changes to "a path under Sources/, Tests/ or windows/ that may
differ from upstream". Append:

```
# --- Windows (upstream's Tauri port in windows/): DESIGN-WIN §2.5 -----------
# Upstream's Claude token path (usage.rs, claude_auth.rs) stays compiled but is
# never started (WU1); upstream's TCP hook server, transcript watcher and hook
# installer are off (WH); the bridge (windows/codenotch/src/agentnotch) feeds
# "usage" and the fork's own events. JSON has no comments: its seams are content.
SEAM	WWS	windows/Cargo.toml	"agentnotch-proto", "agentnotch-engine", "agentnotch-win", "agentnotch-hook", "agentnotch-release"] # Fork: WWS
SEAM	WB	windows/codenotch/Cargo.toml	agentnotch-engine = { path = "../agentnotch-engine" } # Fork: WB
SEAM	WB	windows/codenotch/Cargo.toml	agentnotch-win = { path = "../agentnotch-win" } # Fork: WB
SEAM	WB	windows/codenotch/Cargo.toml	webview2-com = "0.38" # Fork: WB
SEAM	WB1	windows/codenotch/src/main.rs	mod agentnotch; // Fork: WB1
SEAM	WU1	windows/codenotch/src/main.rs	#[allow(dead_code)] mod usage; // Fork: WU1
SEAM	WU1	windows/codenotch/src/main.rs	#[allow(dead_code)] mod claude_auth; // Fork: WU1
SEAM	WH	windows/codenotch/src/main.rs	#[allow(dead_code)] mod server; // Fork: WH
SEAM	WH	windows/codenotch/src/main.rs	#[allow(dead_code)] mod watcher; // Fork: WH
SEAM	WH	windows/codenotch/src/main.rs	#[allow(dead_code)] mod hooks_install; // Fork: WH
SEAM	WD	windows/codenotch/src/main.rs	#[allow(dead_code)] mod doctor; // Fork: WD
SEAM	WU1	windows/codenotch/src/main.rs	fn claude_sign_in() -> Result<(), String> { Err(agentnotch::SIGN_IN_REFUSED.into()) } // Fork: WU1
SEAM	WU1	windows/codenotch/src/main.rs	p if p == "claude" || p.starts_with("claude-") => return agentnotch::refresh_claude(app, p), // Fork: WU1
SEAM	WU1	windows/codenotch/src/main.rs	// Fork: WU1 usage::start never runs
SEAM	WH	windows/codenotch/src/main.rs	let _ = port; // Fork: WH
SEAM	WH	windows/codenotch/src/main.rs	// Fork: WH upstream's transcript watcher stays off
SEAM	WB2	windows/codenotch/src/main.rs	agentnotch::setup(&handle); // Fork: WB2
SEAM	WB3	windows/codenotch/src/main.rs	agentnotch::an_call, // Fork: WB3
SEAM	WSI	windows/codenotch/src/main.rs	if agentnotch::second_instance(app, &_args) { return; } // Fork: WSI
SEAM	WCLI	windows/codenotch/src/main.rs	if let Some(code) = agentnotch::cli::run(&args) { std::process::exit(code); } // Fork: WCLI
SEAM	WH	windows/codenotch/src/main.rs	agentnotch::hooks_switch_get() // Fork: WH
SEAM	WH	windows/codenotch/src/main.rs	agentnotch::hooks_switch_set(on) // Fork: WH
FORBID	WU1	windows/codenotch/src/main.rs	usage::start(
FORBID	WU1	windows/codenotch/src/main.rs	claude_auth::start_login
FORBID	WU1	windows/codenotch/src/main.rs	usage::request_refresh()
FORBID	WH	windows/codenotch/src/main.rs	server::start(
FORBID	WH	windows/codenotch/src/main.rs	watcher::start(
SEAM	WR-DIR	windows/codenotch/src/config.rs	.join(crate::agentnotch::data_folder_name()) // Fork: WR-DIR
FORBID	WR-DIR	windows/codenotch/src/config.rs	.join("codenotch")
SEAM	WC	windows/codenotch/src/config.rs	"outside".into() // Fork: WC
SEAM	WR-RUN	windows/codenotch/src/autostart.rs	const NAME: &str = "Agent Notch"; // Fork: WR-RUN
SEAM	WR-TRAY	windows/codenotch/src/tray.rs	crate::agentnotch::tray_tooltip()	2
SEAM	WR-TRAY	windows/codenotch/src/tray.rs	format!("{} — {}", crate::agentnotch::DISPLAY_NAME, parts.join(" · ")) // Fork: WR-TRAY
SEAM	WR-QUIT	windows/codenotch/src/tray.rs	crate::agentnotch::rebrand(tr(&lang, "quit_app"))
SEAM	WR-QUIT	windows/codenotch/src/notchmenu.rs	crate::agentnotch::rebrand(tr(&lang, "quit_app"))
SEAM	WR-TITLE	windows/codenotch/src/settings_window.rs	.title(crate::agentnotch::SETTINGS_TITLE) // Fork: WR-TITLE
SEAM	WR-TITLE	windows/codenotch/src/dropzones.rs	.title(crate::agentnotch::DROPZONES_TITLE) // Fork: WR-TITLE
SEAM	WUP	windows/codenotch/src/updater.rs	&& !crate::agentnotch::sealed() // Fork: WUP
SEAM	WUP2	windows/codenotch/src/updater.rs	let Some(update) = crate::agentnotch::updater(&app)?.check().await? else { // Fork: WUP2
SEAM	WUP2	windows/codenotch/src/updater.rs	crate::agentnotch::install_failed(&app); // Fork: WUP2
SEAM	WNM	windows/codenotch/src/notchmenu.rs	menu = crate::agentnotch::notch_menu_items(app, menu, provider.as_deref()); // Fork: WNM
SEAM	WNM	windows/codenotch/src/notchmenu.rs	if crate::agentnotch::notch_menu_event(app, item) { return; } // Fork: WNM
SEAM	WR	windows/codenotch/tauri.conf.json	"productName": "Agent Notch",
SEAM	WR	windows/codenotch/tauri.conf.json	"mainBinaryName": "agentnotch",
SEAM	WR	windows/codenotch/tauri.conf.json	"identifier": "com.rivantmedia.agentnotch",
SEAM	WR	windows/codenotch/tauri.conf.json	"publisher": "Rivant Media"
SEAM	WNS	windows/codenotch/tauri.conf.json	"installerHooks": "nsis/agentnotch-hooks.nsh"
SEAM	WUP	windows/codenotch/tauri.conf.json	"pubkey": ""
SEAM	WSI	windows/codenotch/tauri.conf.json	"schemes": ["agentnotch"]
SEAM	WCSP	windows/codenotch/tauri.conf.json	"csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
SEAM	WCSP	windows/codenotch/tauri.conf.json	"dangerousDisableAssetCspModification": ["style-src"]
FORBID	WCSP	windows/codenotch/tauri.conf.json	"csp": null
FORBID	WR	windows/codenotch/tauri.conf.json	com.immidi.codenotch
FORBID	WUP	windows/codenotch/tauri.conf.json	vinzdg/codenotch
FORBID	WUP	windows/codenotch/tauri.conf.json	releases/latest/download/latest.json
FORBID	WUP	windows/codenotch/tauri.conf.json	"createUpdaterArtifacts": true
FORBID	WUP	windows/codenotch/tauri.conf.json	dW50cnVzdGVkIGNvbW1lbnQ6
SEAM	WR	windows/codenotch/tauri.bundle.conf.json	"../target/hook/release/agentnotch-hook.exe": "agentnotch-hook.exe"
FORBID	WR	windows/codenotch/tauri.bundle.conf.json	codenotch-hook.exe
SEAM	WS1	windows/codenotch/ui/notch.html	<script src="agentnotch/notch.js"></script><!-- Fork: WS1 -->
SEAM	WS2	windows/codenotch/ui/notch.html	if(window.agentnotch){const c=agentnotch.claudeCells();if(c)return c;} // Fork: WS2
SEAM	WS3	windows/codenotch/ui/notch.html	if(window.agentnotch)agentnotch.decorateCell(p,cell); // Fork: WS3
SEAM	WS4	windows/codenotch/ui/notch.html	if(p.base==='claude'&&window.agentnotch){html+=agentnotch.cardSessions(p);}else if(p.base==='claude'){ // Fork: WS4
SEAM	WS5	windows/codenotch/ui/notch.html	!(window.agentnotch&&agentnotch.ringClick(press.id))) refreshRing(press.id); // Fork: WS5
SEAM	WSS1	windows/codenotch/ui/settings.html	<button class="row" id="tab-claude" role="tab" aria-controls="pane-claude"
SEAM	WSS2	windows/codenotch/ui/settings.html	<section class="pane" id="pane-claude" role="tabpanel" aria-labelledby="tab-claude" hidden></section><!-- Fork: WSS2 -->
SEAM	WSS3	windows/codenotch/ui/settings.html	const TABS = ['claude', 'accounts', 'appearance', 'general']; // Fork: WSS3
SEAM	WSS4	windows/codenotch/ui/settings.html	const TITLES = { claude:'Claude Code', accounts:'Accounts', appearance:'Appearance', general:'General' }; // Fork: WSS4
SEAM	WSS5	windows/codenotch/ui/settings.html	<script src="agentnotch/settings.js"></script><!-- Fork: WSS5 -->
FORBID	ISO	.github/workflows/release.yml	Codenotch-Setup
FORBID	ISO	.github/workflows/release.yml	taskkill /IM
FORBID	ISO	.github/workflows/agentnotch-windows.yml	taskkill /IM
FORBID	ISO	windows/scripts/agentnotch-smoke.ps1	taskkill /IM
FORBID	ISO	windows/codenotch/nsis/agentnotch-hooks.nsh	codenotch.exe
ALLOW	windows/Cargo.toml
ALLOW	windows/Cargo.lock
ALLOW	windows/codenotch/Cargo.toml
ALLOW	windows/codenotch/tauri.conf.json
ALLOW	windows/codenotch/tauri.bundle.conf.json
ALLOW	windows/codenotch/src/main.rs
ALLOW	windows/codenotch/src/config.rs
ALLOW	windows/codenotch/src/autostart.rs
ALLOW	windows/codenotch/src/tray.rs
ALLOW	windows/codenotch/src/notchmenu.rs
ALLOW	windows/codenotch/src/settings_window.rs
ALLOW	windows/codenotch/src/dropzones.rs
ALLOW	windows/codenotch/src/updater.rs
ALLOW	windows/codenotch/ui/notch.html
ALLOW	windows/codenotch/ui/settings.html
ALLOW	windows/codenotch/capabilities/agentnotch.json
ALLOW	windows/codenotch/nsis/**
ALLOW	windows/codenotch/src/agentnotch/**
ALLOW	windows/codenotch/ui/agentnotch/**
ALLOW	windows/agentnotch-proto/**
ALLOW	windows/agentnotch-engine/**
ALLOW	windows/agentnotch-win/**
ALLOW	windows/agentnotch-hook/**
ALLOW	windows/agentnotch-release/**
ALLOW	windows/agentnotch-ui-tests/**
ALLOW	windows/scripts/agentnotch-build.ps1
ALLOW	windows/scripts/agentnotch-smoke.ps1
ALLOW	windows/scripts/smoke/**
ALLOW	windows/scripts/check-claude-code-facts.mjs
ALLOW	windows/tools/**
```
(`windows/tools/.gitignore` holds `node_modules/`; upstream's `windows/.gitignore` is not
edited.) The `"version"` rule of `tauri.conf.json` is code in `check-seams.sh`, not a line here.

---

## Appendix B — update key derivation test vector

Computed twice independently (Python `hmac`/`hashlib` HKDF; Apple CryptoKit Ed25519 via
`Scripts/release-ed25519.swift public`). A **test seed only**; never a real key.

| Item | Value |
|---|---|
| seed (base64 of bytes 0x00..0x1f) | `AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=` |
| HKDF salt / info / L | `com.rivantmedia.agentnotch` / `tauri-updater minisign ed25519 v1` / 40 |
| derived Ed25519 seed (hex) | `96767d4eebd08c92379fb197f9e0d3a3500f80a8c87faca08b3f00b3440b8f2c` |
| key id bytes (hex, blob order) | `19d0fb618363a5b5` |
| key id as printed | `B5A5638361FBD019` |
| raw public key (base64) | `zA8/6NE42whHHOfU14QWOKEVItB9KMnwL3wVMV/+O/g=` |
| minisign public key line | `RWQZ0Pthg2OltcwPP+jRONsIRxzn1NeEFjihFSLQfSjJ8C98FTFf/jv4` |
| Tauri pubkey (config value) | `dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI1QTU2MzgzNjFGQkQwMTkKUldRWjBQdGhnMk9sdGN3UFAralJPTnNJUnh6bjFOZUVGamloRlNMUWZTako4Qzk4RlRGZi9qdjQK` |

`agentnotch-release`'s unit test asserts all of these from the seed. (This vector's pubkey
begins with `dW50cnVzdGVkIGNvbW1lbnQ6`, which is exactly what the FORBID rule keeps out of the
source `tauri.conf.json`; the test file is not that file.)

---

## Appendix C — environment and switches (Windows)

| Variable / flag | Read by | Meaning |
|---|---|---|
| `AGENTNOTCH_SAFE_MODE` | app | sealed (fails closed) |
| `--no-install` / `AGENTNOTCH_NO_INSTALL` | app | never write settings.json or hooks folders; probes and summaries off |
| `AGENTNOTCH_NO_NOTIFICATIONS` | app | no toasts, no permission queries |
| `AGENTNOTCH_USAGE_PROBE` | app | scheduled probes even with `--no-install` |
| `AGENTNOTCH_SUPPORT_DIR` | app, CLI | `<support>` override (default `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude`) |
| `AGENTNOTCH_DEV_BROWSER_LOG` | app, only with `AGENTNOTCH_DEV=1`, never sealed | the sign-in's authorize URL is appended to this file instead of opening a browser (§4.11; the smoke test's sign-in) |
| `AGENTNOTCH_STATUSLINE_DEPTH` | hook exe (`statusline`) | set by the wrapper for the command it chains; when already present the wrapper chains nothing (loop guard, §4.3) |
| `AGENTNOTCH_CI_ADMIN` | `win_admin.rs` | allows the tests that create a local user and spawn medium-integrity processes (§6.1) |
| `FAKE_CLAUDE_VERSION`, `FAKE_CLAUDE_USAGE`, `FAKE_CLAUDE_LOG` | `fake-claude.exe` (tests, smoke) | its `--version` answer, its `get_usage` fixture, its call log (§2.2) |
| `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` | WebView2 itself (not the app) | set only by the smoke test: `--remote-debugging-port=<p>` for the CDP driver, `--force-device-scale-factor=<s>` for the scaled self-test runs |
| `AGENTNOTCH_SOCKET` | app; hook only with `AGENTNOTCH_DEV=1` | pipe name override (`\\.\pipe\…`) |
| `AGENTNOTCH_EXTRA_CONFIG_DIRS` | app | extra config folders, `;`-separated |
| `AGENTNOTCH_WEB_URL` | app (not sealed) | sync website override (https, or http to localhost) |
| `--dump-state` / `AGENTNOTCH_DUMP_STATE` | app | one line per session change into `run.log` |
| `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH`, `AGENTNOTCH_PANEL_SELF_TEST`, `AGENTNOTCH_SELF_TEST_OUT`, `AGENTNOTCH_SNAPSHOT_CLAUDE` | app, sealed only | §7.4 |
| `AGENTNOTCH_CONTRACT_OUT`, `AGENTNOTCH_CONTRACT_RESPONSE`, `AGENTNOTCH_CONTRACT_APP` | engine tests, e2e script | §4.11 |
| `USERPROFILE` | the engine's `home` (as Claude Code resolves it) | how the smoke test points the app at a temporary Claude setup |
| `CLAUDE_PID`, `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_CODE_ENTRYPOINT`, `WT_SESSION`, `TERM_PROGRAM` | hook exe | forwarded fields (§1.4) |

---

## Decisions log

Every point of the two reviews of revision 1, with its outcome. F = feasibility review,
P = parity review. "Verified" names what was re-read before deciding.

| # | Point | Decision | Where |
|---|---|---|---|
| F1 | A failed `ReplaceFileW` can delete settings.json, then a re-plan writes a hooks-only file | **Accepted, changed.** `ReplaceFileW` is dropped: one atomic rename (`FileRenameInfoEx`, `REPLACE_IF_EXISTS \| POSIX_SEMANTICS`, `MoveFileExW` fallback) after copying the target's DACL/attributes onto a stage beside the resolved target; a vanished file aborts the pass, never re-plans from `{}`; read-only targets refused; fault-injected test with a 1 ms reader. Verified: HS§3.4's "blank or missing counts as {}" rule and the re-plan loop. | §1.8, §3.2 `SecureFiles`, §4.3, §7.3 |
| F2 | A manual upgrade runs the old uninstaller, which stripped every hook | **Accepted.** Hooks are removed only with "Delete the application data" or `/REMOVEHOOKS`; `control quit` still runs. Verified in installer.nsi: 242-245 and 292-293 (upgrade defaults to "uninstall first"), 355-360 (old uninstaller run without `/UPDATE`), 458-461 (checkbox read on the confirm page, before the section), 776-780. Smoke phases 12 and 15. | §0 item 14, §1.1, §1.5, §4.3, §6.5, §7.5, §9 Q1 |
| F3 | `OpenProcessToken` peer checks fail across integrity levels; CI can't see it | **Accepted.** Server: identification-level `ImpersonateNamedPipeClient` + `OpenThreadToken` after the first read. Client: `GetSecurityInfo` on the pipe (owner set explicitly with `O:<SID>`, protected two-ACE DACL). Doctor elevation lines. `win_admin.rs`: medium ⇄ high both ways, and a squatter running as a second local user. | §1.4, §4.4, §4.14, §7.3 |
| F4 | The hook's 1 s write timeout had no mechanism | **Accepted.** Watchdog thread `ExitProcess(0)` at 1.2 s for connect + write, disarmed before the PermissionRequest read; the status-line send on its own thread (0.3 s) so the relay is never killed. Test: a server that never reads, a 1 MiB frame. | §1.4, §7.3 |
| F5 | Chat typing had no engine re-check between the text and Return | **Accepted.** Two-phase helper (`typed` → `submit`/`abort`, 2 s), with `Input::TypeCheckpoint` re-running `message_safety` on fresh state. | §3.2 `ConsoleInput`, §3.4, §4.8, §7.3 |
| F6 | Auto-open panel: keystrokes meant for the composer can reach the terminal | **Accepted.** Keyboard gate until the glue confirms the panel is the foreground window (`an:panel_focus`); "Click to type"; `autoOpen` defaults to Never on Windows; node and glue tests; the self-test checks the gate. | §2.4 `panel.rs`, §4.10, §5.3, §7.3, §7.4 |
| F7 | String-form hook command fails in PowerShell | **Accepted.** Verified `bt=e.shell??R1()` in winmap/hookexec.txt. String form is the unquoted forward-slash path, else its 8.3 path, else none (folder shown as not hookable); `shell_forms.rs` runs the exact commands through Git Bash and PowerShell. | §0 item 3, §4.3, §7.3 |
| F8 | Status-line takeover can break a working status line | **Accepted, with one refinement.** Wrap only when Git Bash exists and the command has no `\`, `.ps1`, `$env:`, no `shell` key and names no PowerShell/cmd; otherwise leave it alone with a visible reason; the facts job decodes `yun`. Forward-slash drive paths stay allowed (Git Bash resolves `C:/…` itself). CI backslash cases. | §4.3, §6.2, §7.3, §7.5 phase 7 |
| F9 | The exec-form gate misses bundled Claude Code copies | **Accepted.** Versions also come from the folder names of the VS Code-family extensions and Claude Desktop's `claude-code\<ver>`; an unknown one keeps string form; `EXEC_FORM_MIN` comes only from the committed facts file, which also records whether an old version rejects unknown keys. | §4.3, §6.2 |
| F10 | `<support>` in roaming `%APPDATA%` causes a reset loop and leaks refresh tokens | **Accepted.** `<support>` = `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude`; the machine binding and `machineId` are removed. Verified: the template deletes that folder with "Delete the application data" (installer.nsi 883). | §0 item 7, §1.5, §1.6, §3.2, §4.12, §9 R8 |
| F11 | CI does not test the real-world conditions | **Accepted.** `win_admin.rs` (integrity levels, second-user squatter); smoke phases after consent, update over a running app with a held request, the reinstall path, `/REMOVEHOOKS`; the Known-Folder profile and other real locations hashed; interactive-desktop preflight. | §6.1, §7.3, §7.5 |
| F12 | Typing can land in a shell that shares the console | **Accepted.** Only Claude, its descendants and its direct parent chain of known shells may share the console; the console must be in raw mode (`ENABLE_LINE_INPUT` clear); typing is opt-in; the residual risk is stated in the switch's caption. | §4.8, §4.12, §9 R4 |
| F13 | Argv and deep-link hardening | **Accepted.** `cli::run` returns `None` when any argument starts with `agentnotch:`; a deep link must be exactly one `agentnotch://` argument; `setup` exits when another instance owns the single-instance window (verified plugin windows.rs 69-107: a found mutex with no window lets a second instance run). Relaunch `/ARGS`: the glue's updater filters deep links when the builder allows; otherwise the stale-callback rule ignores them (verified updater.rs 887). | §2.4, §7.3 |
| F14 | Pipe client SQOS; Tokio `max_instances` cap | **Accepted.** SQOS identification on every client open (check-seams grep); instances unlimited and counted by the app. Verified tokio named_pipe.rs 2224-2226. | §1.4, §6.7 |
| F15 | A truncated PEB read reports `Unset` | **Accepted.** Whole block up to 1 MiB, double-NUL required, partial reads `Unreadable`; 100 KiB test. | §3.2, §4.2, §7.3 |
| F16 | Parent-pid fallback wrong in string form | **Accepted.** `--exec` marker; string form walks past shells to `claude.exe`/`node.exe`/`bun.exe`, else `pid: null`; `pid_guess` vectors and real-process tests. | §1.4, §3.1, §7.3 |
| F17 | Hook exit-code hygiene | **Accepted.** No argument-parser crates (check-seams rule), hand parsing, `catch_unwind`, `write_all`; tests for unknown/extra args, closed stdout, a forced panic. | §1.8, §2.2, §6.7, §7.3 |
| F18 | Replacing a running hook copy left a gap | **Accepted.** Stage `.new` first, try one atomic rename, then the two-rename fallback; `.old`/`.new` swept by passes and uninstall; test watches for a missing exe. | §1.5, §4.3, §7.3 |
| F19 | The self-update exits without stopping the hub | **Accepted with a different anchor.** The plugin's `Builder` has no `on_before_exit`; `UpdaterBuilder` has one, which `updater_builder()` sets to `cleanup_before_exit` (verified lib.rs 120-123, updater.rs 849-852 and 882). Seam WUP2 in `updater.rs:122` uses the glue's `updater(app)`, whose hook stops the hub then runs the cleanup. R10 reworded. | §2.4 `update.rs`, §2.5, §4.15, §9 R10, App. A |
| F20 | Token-free rule contradicted upstream's other providers; GLM reads an API key | **Accepted.** The dormant-site rule covers Claude-specific patterns and files only (verified glm.rs 142-153). GLM's read is accepted as upstream ships it (the maintainer keeps upstream's providers unchanged; no seam), documented, and pinned by a hash of `claude_code_key`. | §4.16, §6.7 |
| F21 | Sealed mode not isolated | **Partly accepted.** Sealed redirects upstream's data to `Agent Notch Sealed` (WR-DIR) and the smoke test proves no instance runs first. **Rejected:** a separate sealed identifier/installer: it doubles the build and tests a binary that never ships; running sealed beside a live copy is documented as impossible instead. | §2.5, §4.13, §7.4 |
| F22 | Signing notes | **Accepted.** Rationale corrected (key separation, not the prehash; `allow_legacy = true`); `release-tool` job builds the signer without secrets, seed piped on stdin; bridge rotation and Sparkle-seed rotation documented. | §6.3, §6.4, §6.9 |
| F23 | Smart App Control blocks unsigned binaries | **Accepted.** Release notes, `/download`, doctor line; Authenticode raised to a priority (Q4). | §4.14, §6.3.1, §6.8, §9 R7, Q4 |
| F24 | Only `panel.html` had a CSP | **Accepted.** App-wide CSP seam WCSP with `style-src` exempted from Tauri's hash injection; WP0 audits upstream's pages first; the self-test fails on any violation in any page; hostile-string node tests for every renderer. | §2.5, §3.7, §5.1, §7.4, App. A |
| F25 | UIA/focus/typing could starve the worker pool | **Accepted.** Lanes: `an-io-{0..2}`, `an-probe`, `an-ui`; UIA connection and transaction timeouts 2 s. | §1.2, §3.4, §4.9 |
| F26 | `reveal_folder` ran Explorer with a page-supplied path | **Accepted.** `reveal {kind, id}`: the engine resolves a folder it knows; `SHOpenFolderAndSelectItems`, no command line. | §3.5 |
| F27 | Factual fixes | **Accepted.** Registry row corrected (installer.nsi 864-879); DPI geometry vectors at 125 %, 150 % and mixed DPI, plus scaled self-test runs. | §1.5, §5.6, §7.2, §7.4 |
| P1 | Cross-package types unspecified | **Accepted.** §3.4 now writes out every crossing type (`Job`/`Lane`/`JobResult`, `Input`, `Release`, `TranscriptDelta`/`Entry`, `IngestContext`, `ProbePlan`/`Result`, `InstallPlan`/`Outcome`, `AccountsChanged`, `IngressConfig`, `LiveBatch`, `CloudAccount`/`Folder`, `CloudDeps` trait, `CloudCall`, `CloudConfig`, `ToastContext`, `ReactionContext`, `Reactions`, `PanelState`, every `SessionInput`); `SessionView` gains the cloud fields; `PLACEMENT_GRACE` = 30 s; `an-core` is the only settings writer. Only true Mac ports stay "straight ports", each with its report section. | §1.2, §3.3, §3.4, §4.5 |
| P2 | Scope unrealistic for an untested release | **Partly accepted.** Nothing is deferred (the maintainer decided on full parity, and summaries, the Desktop cache reader and the renderers are pure, fixture-tested code). Instead the parts CI cannot fully prove ship on conservative defaults (typing opt-in, auto-open Never, status-line safe rule, WT best effort), Windows is labelled **preview**, and ship gates let a feature that is not green ship in its honest disabled state rather than block. | §0 items 13-14, §8 ship gates, §6.3.1, §6.8 |
| P3 | The Mac's in-flight cost estimates change the contract semantics | **Accepted.** WP8 ports the first `main` commit containing `ModelPricing.swift` (scanner v4, `payloadVersion`, the new `costUsd` rule); one pricing vector file read by Swift and Rust tests. Verified in the working tree: `ModelPricing.swift`, `web/contract/README.md` `costUsd` row. | §4.11, §7.6, §8 WP8 |
| P4 | Status-line wrapper had no Windows loop guard | **Accepted.** Name guard (any spelling) plus `AGENTNOTCH_STATUSLINE_DEPTH`; nested-attempt test. | §4.3, §7.3, App. C |
| P5 | Parent-pid fallback wrong in string form | **Accepted** (same fix as F16). | §1.4 |
| P6 | The engine could not see the panel's state | **Accepted.** Glue-only `panel_state` call; `auto_close_deadline` in the engine emitting `PanelClose`; pin through `set_setting`. | §3.4, §3.5, §5.3 |
| P7 | Foundational modules owned by leaf packages | **Accepted.** WP0 delivers `core::{paths, json_scan, claude_json, atomic}` complete and tested. | §2.1, §8 WP0 |
| P8 | Riskiest Windows work came last | **Accepted.** WP0's fixture-backed `Hub::sealed`; WP9/WP10 start at once; milestone M1 (sealed self-test green) precedes the engine. | §2.1, §7.6, §8 |
| P9 | No test ran the assembled app past consent | **Accepted.** Smoke phases 7-11 through WebView2 remote debugging (no backdoor), fake-claude probe, fake website sign-in via a dev-only browser log, `win_http.rs`. | §4.11, §7.3, §7.5 |
| P10 | The smoke test could run a real `claude` | **Accepted.** Preflight proves none is reachable; `fake-claude.exe` is the first candidate and logs every run; the log is asserted. | §2.2, §6.1, §7.5 |
| P11 | Claude Code behaviours could be checked on CI | **(a) Accepted**: `shell_forms.rs` and smoke phase 8 run the exact commands through Git Bash and PowerShell with space and 8.3 paths (cmd omitted: Claude Code never uses cmd for hooks), plus the facts job. **(b) Needs the maintainer** (CLAUDE.md forbids running `claude`): Q3. | §6.2, §7.3, §9 Q3 |
| P12 | serde rules clashed with the contract and file formats | **Accepted.** `UsageSource::contract_name()`; per-file `persist::*` DTOs with explicit names and date adapters; Mac-file round trips. | §3, §7.6 |
| P13 | False "login conflict" on Windows | **Accepted.** Spellings collapse by `key()`; no conflict chip or spelling-switching refresh; the default folder's set/unset rule kept. | §3.3, §4.2 |
| P14 | Environment scrub underspecified | **Accepted.** One `scrubbed_env` for probe and summaries (incl. `CLAUDE_SECURESTORAGE_CONFIG_DIR`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`) with a vector. | §3.4, §4.6, §4.11 |
| P15 | Machine binding reset roamed state only partly | **Accepted** (same fix as F10: binding removed, state never roams). | §1.5 |
| P16 | UI actions the calls could not carry | **Accepted.** `open_notification_settings`; `hotkey_status`; `accountNicknames` dropped (the Mac's rename already sets a label). | §2.4, §3.5, §4.10, §4.12 |
| P17 | Notch dimmed Claude rings on upstream's 15-minute rule | **Accepted.** `RingUsage.stale` from the engine; `decorateCell` sets the class. | §3.6, §5.2 |
| P18 | Hover-card rows lacked data | **Accepted.** `SessionRow.card` (port of `activityRow`). | §3.6, §5.2 |
| P19 | Upstream's right-click menu vs the fork's rings | **Accepted.** WU1d routes `claude-*` ids to the glue; seam WNM adds "Open sessions panel". Verified: the menu's Refresh calls `refresh_all`; the cell-id path is `refresh_ring` → `refresh_provider` (main.rs 674-690). | §2.4, §2.5, App. A |
| P20 | Chat payloads too large | **Accepted.** Reset-then-patch chat updates; images by reference (`chat_image`). | §3.5, §3.6, §4.8 |
| P21 | Sealed mode not isolated | **Partly accepted** (same as F21). | §4.13 |
| P22 | Smoke CLI mechanics unspecified | **Accepted.** `Start-Process -Wait -PassThru` + `<data>\<cmd>.log`. | §4.14, §7.5 |
| P23 | Model ownership; incomplete enums; `core/rebrand.rs` missing | **Accepted.** One model file per owning package after WP0; `TaskStatus` gains `Deleted`, `CreateFailed`; `core/rebrand.rs` in the tree (WP7). | §2.1, §3.3 |
| P24 | Old hook copies may speak an older protocol | **Accepted.** Lenient both ways, optional-only additions, v1 fixtures kept forever. | §1.4, §3.1, §9 R19 |
| P25 | Nobody would look at the PNGs | **Accepted.** Machine-checked layout invariants, committed pixel baselines with tolerance, and agents view the PNGs from the CI artifact. | §5.6, §7.4 |

Questions that need the maintainer (everything else above is decided):
1. **Q3** — may CI run the real Claude Code hermetically (temporary profile, fake API key, local
   fake Messages API; no login token anywhere)? CLAUDE.md forbids running `claude` today.
2. **Q4** — Authenticode signing (an Azure Trusted Signing account or a certificate, plus its
   secret) before Windows leaves preview.
3. **The cost-estimate commit** — WP8 starts from the first `main` commit that contains
   `ModelPricing.swift`; the change is uncommitted in the working tree today.
4. **Two product defaults to confirm** — the "preview" label for Windows in 1.1.0, and the
   uninstaller keeping hooks unless "Delete the application data" is ticked.

---

## Maintainer decisions (2026-09-28, binding; they override anything above that disagrees)

- **Q3, real Claude Code in CI: allowed, CI only.** A job on the Windows runner (in
  `agentnotch-windows.yml`) installs the real Claude Code into a temporary profile and runs it
  hermetically: temporary `USERPROFILE`/`APPDATA`/`LOCALAPPDATA`, `CLAUDE_CONFIG_DIR` in that
  profile, a fake `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` pointing at a local fake Messages API
  that returns a `tool_use` (and plain text), no login token anywhere on the runner, no network
  except the package download. It proves end to end: the installed hook entries fire (string form,
  and exec form where the version allows), `CLAUDE_PID`/`CLAUDE_CONFIG_DIR` reach the hook,
  `sessions\<pid>.json`, a PermissionRequest answered from the engine (allow, deny),
  AskUserQuestion/ExitPlanMode answers, the status line wrapper. `EXEC_FORM_MIN` may rest on this
  job's evidence. CLAUDE.md keeps "Never run `claude`" for the maintainer's Mac and every other
  test, with an explicit exception naming only this CI job. The Claude Code version it installs is
  pinned (the facts job reports newer ones).
- **Q4, Authenticode: 1.1.0 ships unsigned, as "Windows (preview)".** `/download` and the release
  notes label it preview and state the SmartScreen ("More info", then "Run anyway") and Smart App
  Control caveats. `bundle.windows.signCommand` (Azure Trusted Signing or a certificate) is wired
  so signing turns on when its secrets exist later (`WINDOWS_SIGN_*`, documented), and then signs
  the app, the installer and `agentnotch-hook.exe`; without them the build is unsigned and says so
  in `release-info`.
- **Q1, uninstall: confirmed.** Hooks are removed only on "Delete the application data" or
  `/REMOVEHOOKS`.
- **Q2** as recommended: pin the Windows public key in the repo after 1.1.0 publishes.
- **Ship sequence** (the maintainer chose "merge and release when green"): all CI green on
  `windows-port`, merge to `main`, a Release dry run on main, then the `VERSION` bump to 1.1.0.
