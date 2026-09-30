CLAUDE CODE HOOKS, SESSION PIPELINE AND SESSION CONTROL: PORTING SPEC (macOS engine → Windows)

Sources were read-only. Path prefixes used below:
- E = Packages/ClaudeControl/Sources/ClaudeControl/Engine/
- S = Packages/ClaudeControl/Scripts/
- U = Packages/ClaudeControl/Sources/ClaudeControl/UI/
- W = windows/ (upstream Codenotch for Windows, Rust/Tauri 2)

The Claude Code facts marked [CC 2.1.282] come from strings in the installed bundle ~/.local/share/claude/versions/2.1.282. The bundle was read, never run.

======================================================================
0. CROSS-CUTTING FINDINGS THAT SHAPE THE WINDOWS DESIGN
======================================================================

[CC 2.1.282] How Claude Code runs hook commands. This is the exact code path around the string "Exec form treats":
- Each hook entry may carry `"shell": "powershell"`. Without it, Windows runs the command through Git Bash. Git Bash is found via CLAUDE_CODE_GIT_BASH_PATH, then `C:\Program Files\Git\bin\bash.exe`, then `C:\Program Files (x86)\Git\bin\bash.exe`. If none is found the hook fails with: `Hook "<cmd>" requires bash but Git Bash was not found. Install Git for Windows …, or add "shell": "powershell" to this hook's config.`
- Exec form: `{"command": "<executable>", "args": [...]}` spawns the executable directly with no shell: `Pne(cmd, args, {env, cwd, detached: !windows, windowsHide: true})`. There is a warning if "command" contains whitespace but no path separator.
- In bash form on Windows, Claude Code rewrites the command (`Pt = yun(Pt)`; the transform is not decoded) and puts CLAUDE_PROJECT_DIR in the environment with forward slashes.
- Hook processes on Windows are not detached. `windowsHide` is true.
- A hook "ran OK" when it exited normally, status ≤ 128, and status is not 126 or 127. Exit 2 blocks. A spawn failure is reported as "could not start".
- For PermissionRequest, Claude Code reads `hookSpecificOutput.decision`: `behavior === "allow"` → allow and take `decision.updatedInput`; anything else → deny. The whole `decision` object is kept as `permissionRequestResult`.
- Not verified: the minimum Claude Code version that accepts `args` and `shell`. Check CHANGELOG.md, and gate them like the event list (§3.5).

Consequences for Windows:
1. Do not use Python on Windows.
   - `python3` there is often the Microsoft Store App Execution Alias stub in %LOCALAPPDATA%\Microsoft\WindowsApps. It opens the Store or prints a message and exits 9009. This is the same problem as the macOS xcode-select shim (S8, E/Core/HookCommands.swift:24-32).
   - The Python cold start would also run on every tool call.
   - Port both scripts to one native, dependency-free Rust binary, e.g. `agentnotch-hook.exe hook | statusline`.
2. Register hooks in exec form: `{"type":"command","command":"C:\\…\\agentnotch-hook.exe","args":["hook"]}`.
   - This bypasses bash and PowerShell quoting, MSYS path conversion and the Git Bash requirement.
   - Fallback for older Claude Code: bash form `[ -f 'C:/…/agentnotch-hook.exe' ] || exit 0; exec 'C:/…/agentnotch-hook.exe' hook`. Use forward slashes, because Windows paths can hold spaces. It works only when Git Bash exists.
   - Fail-open still holds. A missing exe gives exit 127 in bash form, or a spawn failure in exec form. Neither is 2. Verify that a spawn failure is non-blocking for PermissionRequest. `MAe` blocks only `type:"script"` hooks.
3. The upstream Windows crate (W/codenotch-hook, W/codenotch/src/hooks_install.rs, server.rs) breaks several fork rules. Details in §12.

======================================================================
1. HOOK SCRIPT: S/agentnotch-hook.py (425 lines, protocol v2)
======================================================================

1.1 Invocation and invariants (docstring lines 1-28, main 391-425)
- Claude Code runs it for every registered event (§3.5). The event JSON arrives on stdin.
- Python 3.9, standard library only, run with `-S`.
- It must never raise and always exits 0 (`sys.exit(0)` after `except BaseException`, lines 420-425).
- Flow:
  - Read all of stdin as bytes (393).
  - If the socket path does not exist (`os.path.exists`), return at once: the app is not running (397).
  - Parse JSON. A non-dict ends it (399-404).
  - Build the message (406).
  - PermissionRequest: request/response (408-415).
  - Any other event: fire and forget (417).
- SOCKET_PATH (line 35): `(os.environ["AGENTNOTCH_SOCKET"] if AGENTNOTCH_DEV == "1" else None) or "__AGENTNOTCH_SOCKET_PATH__"`. The placeholder is replaced at install time with a Python string literal (§3.2). The environment override needs both variables, so a leftover export can never redirect a real session.

1.2 Constants (37-58)
| Constant | Value |
|---|---|
| CONNECT_TIMEOUT | 1.0 s (also covers `sendall`, because `settimeout` persists) |
| DECISION_TIMEOUT | 86400 s (matches the installer's `"timeout": 86400`) |
| MAX_RESPONSE_BYTES | 4 MiB |
| MAX_TOOL_INPUT_STRING | 20000 chars |
| MAX_TOOL_ERROR | 500 |
| MAX_LAST_ASSISTANT_MESSAGE | 1500 |
| MAX_PROMPT | 300 |
| MAX_TEXT | 2000 |
| DEFAULT_DENY_MESSAGE | "Denied by user via Agent Notch" |
| MAX_MESSAGE_BYTES | 1 MiB |
| MAX_LIST_ITEMS | 50 |
| MAX_BACKGROUND_TYPES | 64 |

1.3 Fields read from Claude Code's stdin
hook_event_name, session_id, cwd, transcript_path, agent_id, agent_type, permission_mode, tool_name, tool_input, tool_use_id, tool_response.task.{id,subject}, error, is_interrupt, permission_suggestions, reason, message, notification_type, title, last_assistant_message, background_tasks[].type, session_crons, stop_hook_active, error_details, agent_transcript_path, task_id, task_subject, source, model, session_title, prompt, trigger.

Environment read: CLAUDE_PID, CLAUDE_CONFIG_DIR, CLAUDE_CODE_SESSION_ATTENDED, CLAUDE_CODE_ENTRYPOINT, AGENTNOTCH_DEV, AGENTNOTCH_SOCKET.

1.4 Message sent to the app, exact JSON by event (build_message 144-267)

Base fields, present on every hook event:
```
{"event": <hook_event_name or "">,
 "session_id": <session_id or "unknown">,
 "cwd": <cwd or "">,
 "transcript_path": <str|null>,
 "pid": <int(CLAUDE_PID) else os.getppid()>,          # 89-99
 "status": <coarse status, below>,
 "config_dir_env": <raw CLAUDE_CONFIG_DIR|null>,       # verbatim on purpose (157)
 "attended": <true if CLAUDE_CODE_SESSION_ATTENDED=="1", false if "0", else null>,
 "entrypoint": <CLAUDE_CODE_ENTRYPOINT|null>,
 "agent_id": <str|null>,   # present only for subagent events
 "agent_type": <str|null>,
 "permission_mode": <str|null>}
```

Coarse "status" (status_for 113-141):
| Event | status |
|---|---|
| PreToolUse | running_tool |
| PermissionRequest | waiting_for_approval |
| Stop, StopFailure, SessionStart | waiting_for_input |
| SessionEnd | ended |
| PreCompact | compacting |
| Notification with notification_type == "idle_prompt" | waiting_for_input |
| Any other Notification | notification |
| UserPromptSubmit, PostToolUse, PostToolUseFailure, PermissionDenied, SubagentStart, SubagentStop, PostCompact, TaskCreated, TaskCompleted | processing |
| Anything else | unknown |

Per-event additions:
- PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, PermissionDenied:
  - "tool": tool_name
  - "tool_input": truncate_deep(tool_input) if it is a dict, else {}. Every nested string is clamped to 20000 chars.
  - "tool_use_id": only when truthy. PermissionRequest has none.
- PostToolUse where tool_name == "TaskCreate":
  - "task_id": str(tool_response.task.id), when the id is not None
  - "task_subject": subject ≤ 2000, or null
- PostToolUseFailure:
  - "tool_error": error (non-strings go through json.dumps), ≤ 500, or null
  - "is_interrupt": only when it is a bool
- PermissionRequest: "permission_suggestions": the list verbatim, when it is a list.
- PermissionDenied: "denial_reason": (reason or message), ≤ 2000, or null.
- Notification: "notification_type", "message" (≤ 2000), "title" (≤ 2000).
- Stop, StopFailure, SubagentStop: "last_assistant_message" ≤ 1500. Then:
  - Stop:
    - "background_task_count": len(background_tasks) if it is a list, else 0
    - "background_task_types": the "type" strings of the first 64 dict entries (only when background_tasks is a list)
    - "session_cron_count": len(session_crons), when it is a list
    - "stop_hook_active": when it is a bool
  - StopFailure:
    - "stop_error": error if it is a string, else "unknown"
    - "stop_error_details": (non-strings go through json.dumps), ≤ 500
  - SubagentStop: "agent_transcript_path"
- TaskCreated, TaskCompleted: "task_id" (as str, when present), "task_subject" ≤ 2000.
- SessionStart: "source", "model" (strings only, else null), "session_title" ≤ 200.
- UserPromptSubmit: "source", "session_title" ≤ 200, "prompt" ≤ 300.
- SessionEnd: "reason".
- PreCompact, PostCompact: "trigger".
- `text_or_none` turns empty strings into null.

1.5 Size cap (encode 270-283)
- Payload is `json.dumps(message)` in UTF-8. json.dumps is ASCII-escaped with ", " and ": " separators.
- If it is over 1 MiB and there is a tool_input:
  1. Re-truncate tool_input: strings to 500 chars, lists to 50 items.
  2. If still over 1 MiB: tool_input = {}.
- The server drops anything over 8 MiB.
- The permission merge (§1.7) always uses the original stdin tool_input, never this copy.

1.6 Transport (286-354)
- is_own_socket: lstat(path) must show st_uid == getuid() and S_ISSOCK. Otherwise raise, which is swallowed. The /tmp fallback folder is shared between users, hence this check.
- Fire and forget (send_message): connect with a 1 s timeout, sendall, shutdown(SHUT_WR), close. Every error is swallowed.
- Request/decision (request_decision):
  1. connect, sendall, SHUT_WR
  2. settimeout(86400)
  3. recv until EOF or 4 MiB
  4. No bytes → None. Otherwise json.loads. Must be a dict, else None.

1.7 Decision printed to Claude Code (permission_output 357-388)
App → hook response (§4.4): `{"decision":"allow"|"deny"|"ask", "reason"?, "updated_input"?, "updated_permissions"?, "interrupt"?}`

- "allow":
  - result = `{"behavior":"allow"}`
  - If updated_input is a dict: `merged = dict(original stdin tool_input)` (or {} when that is not a dict), then `merged.update(updated_input)`, and `result["updatedInput"] = merged`. An empty dict therefore echoes the original input.
  - If updated_permissions is a non-empty list: `result["updatedPermissions"] = it`.
- "deny":
  - result = `{"behavior":"deny", "message": reason if it is a non-empty string, else "Denied by user via Agent Notch"}`
  - Plus `"interrupt": bool` when a bool was given.
- Anything else ("ask", missing, EOF, timeout, parse error): print nothing and exit 0. Claude Code's own terminal dialog, which runs in parallel, decides.
- stdout = `json.dumps({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":result}})`, then flush.

Exact stdout for each answer the UI can give (the UI's callers are in §6):
- Approve:
  `{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}}`
- Always allow (first permission suggestion, verbatim):
  `{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow", "updatedPermissions": [<permission_suggestions[0]>]}}}`
- ExitPlanMode "Approve plan" (updated_input {}):
  `{…"decision": {"behavior": "allow", "updatedInput": {<original tool_input verbatim, e.g. "plan": "...">}}}`
- AskUserQuestion:
  `{…"decision": {"behavior": "allow", "updatedInput": {<original tool_input>, "answers": {"<question text exactly as sent>": "<label>" | "<l1>, <l2>[, <other text>]"}}}}`
- Deny:
  `{…"decision": {"behavior": "deny", "message": "Denied by user via Agent Notch"}}`
- Plan "Keep planning":
  `{…"decision": {"behavior": "deny", "message": "The user reviewed the plan and wants to keep planning. Stay in plan mode and ask what to change before implementing."}}`
  (source: U/Components/Chat/ChatApprovalBars.swift:73)
- No decision, or the app closed the connection: empty stdout, exit 0.

WIN mapping (hook binary, Rust):
- Keep the same message schema byte for byte, so engine tests and the dev simulator port 1:1.
- pid: use CLAUDE_PID. It is set for hooks since 2.1.214. The parent-pid fallback (NtQueryInformationProcess, as upstream does) is wrong in bash form, where the parent is bash.exe. It is right only in exec form.
- stdin: read all bytes as UTF-8.
- stdout: write raw UTF-8 bytes with no BOM and no "\r\n" translation. If Python is ever used, write to sys.stdout.buffer; json.dumps' ensure_ascii keeps it ASCII-safe.
- Transport: §4 WIN (named pipe). Replace "socket file exists" with CreateFileW on the pipe returning ERROR_FILE_NOT_FOUND → exit 0 immediately.
- Never launch the app from the hook. Upstream's hook does (W/codenotch-hook/src/main.rs:25-33); remove that.
- Code-sign the exe: an unsigned binary run on every tool call from a user folder draws AV/SmartScreen trouble.

======================================================================
2. STATUS LINE WRAPPER: S/agentnotch-statusline.py (257 lines)
======================================================================

- Registered as the account's `statusLine.command`. Claude Code runs it after assistant messages with the status line JSON on stdin.
- Flow (main 229-253):
  1. Read stdin.
  2. Start the previous command first (start_previous) so it runs in parallel.
  3. Build and send the message.
  4. Pipe the previous command's stdout through and return its exit code (negative → 1). No previous command → no output, exit 0.
- Constants: SEND_TIMEOUT 0.3 s, PREVIOUS_TIMEOUT 30 s.
- PREVIOUS_FILE: `<dir of script>/agentnotch-statusline.previous.json`. It holds the replaced statusLine object verbatim, or `{}` meaning "chain nothing".
- previous_command (137-155): `command` must be a non-empty string. It is refused if it contains "agentnotch-statusline.py", "superpowered-codenotch-statusline.py" or "superpowered-notch-statusline.py", which prevents loops.
- Process handling:
  - Popen(command, shell=True, stdin/stdout=PIPE, start_new_session=True): the previous command gets its own process group.
  - SIGTERM or SIGHUP → killpg(child) and `os._exit(128+sig)`. A signal that arrives during Popen is deferred (`_pending_signal`).
  - Timeout → killpg SIGKILL, return 0 with no output.
- Message sent (build_message 71-100). Fire and forget; is_own_socket check; 0.3 s timeout:
```
{"event":"StatusLine","session_id":…,"transcript_path":…,"cwd":…,
 "config_dir_env": <raw CLAUDE_CONFIG_DIR|null>,
 "pid": <int CLAUDE_PID in 1..2^31-1, else null; no ppid fallback>,
 "status_line":{"rate_limits": <verbatim>,
                "context_window":{"used_percentage":…,"context_window_size":…},
                "model": <verbatim object {id, display_name}>,
                "cost":{"total_cost_usd":…},
                "session_name":…, "version":…}}
```
- App side: E/Services/Hooks/HookEvent.swift:496-541 (StatusLineMessage). It needs a non-empty session_id. Rate limits are parsed by UsageParser (the usage area).

WIN:
- Same subcommand in the native exe: `agentnotch-hook.exe statusline`.
- The previous command must run under the shell Claude Code would have used. On Windows that is presumably Git Bash, `bash.exe -c "<cmd>"` with the same bash discovery as Claude Code. Verify whether statusLine honours a `shell` key.
- Replace process groups with a Job Object: CreateJobObject with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, then AssignProcessToJobObject on the child. Create it suspended, or assign right after spawn.
- Claude Code cancels by TerminateProcess, and no handler runs. The job handle closes with the wrapper, which kills the tree. Timeout → TerminateJobObject.
- Send uses the pipe with a 0.3 s budget (WaitNamedPipe/connect bounded).

======================================================================
3. HOOK INSTALLATION
======================================================================

3.1 Where, and when
- Files:
  - `<configDir>/hooks/agentnotch-hook.py`
  - `<configDir>/hooks/agentnotch-statusline.py`
  - `<configDir>/hooks/agentnotch-statusline.previous.json`
  - `<configDir>/settings.json`
  - (E/Services/Hooks/HookInstaller.swift:196-224)
- Script names come from the configuration: E/Public/ClaudeControlConfiguration.swift:90-91.
- Consent (E/Services/Hooks/AccountHookManager.swift:19-28, 185, 285-305, 335-350):
  - Nothing is written until the user answers "Turn on Claude Code control" (`claudeControl.hookConsent`: nil = not asked, true, false).
  - `hooksEnabled` defaults to true but only counts while consent == true (E/Core/ClaudeControlSettings.swift:243).
  - `hookConsentScope` = 2: the yes now covers VS Code window folders. Older yeses are shown once (`foldersBeyondConsent`).
  - `statusLineIntegration` defaults to true.
  - "Not now" stores false.
  - Turning hooks off uninstalls from every folder and restores status lines.
- Blocked: while Superpowered Vibe Notch is running, and for folders that still hold SPVN hooks until "Take over". Two blocking PermissionRequest hooks would race.
- Disabled by `--no-install` / AGENTNOTCH_NO_INSTALL.
- Before bootstrap, never touches ~, ~/.claude*, ~/.config/claude* (HookInstaller.swift:232-245).
- Targets: `installTargets` = tracked run folders only (AccountHookManager.swift:155-174).
  - Included: ~/.claude, standalone ~/.claude-<name> adopted via manifest `stores`, and ~/.claude-windows/<12hex>/ working copies.
  - Never: Parallel Profiles stores (marker `.parallel-accounts-store`, or listed in the manifest's `created`), ~/.claude-shared, the ~/.claude-windows root. An unreadable manifest refuses standalone ~/.claude-* folders (isNeverInstallTarget 254-273).
  - A mirrored ~/.claude stays hooked while any identity is tracked.
- Passes run at start, when accounts change (1 s debounce), when a lower Claude Code version is seen, when SPVN quits, and every 10 min (recheckInterval 102).
- One write per physical settings.json (resolved through links, `settingsFileIdentity` 739-745). Untracked folders get ours removed unless a tracked folder shares the file (runPass 590-690).

3.2 Script templating (E/Scripts/EmbeddedScripts+Install.swift:14-52)
- The `.py` files are embedded byte for byte as Swift raw strings by S/embed-scripts.sh.
- At install, the quoted placeholder `"__AGENTNOTCH_SOCKET_PATH__"` is replaced with a Python literal. Backslash, quote, \n, \r and \t are escaped; other controls become \xNN.
- A script is written only when its bytes differ, then set to 0755 (installScript 1102-1121).
- Scripts are written before settings.json, so settings never point at a missing script.
- WIN: with a SID-derived pipe name (§4 WIN) nothing needs templating. Copy agentnotch-hook.exe into `<configDir>\hooks\`, which mirrors macOS: the command is independent of the app's install location, and an uninstalled app fails open.
  - A running exe cannot be overwritten; PermissionRequest hooks can live for 24 h.
  - Update by rename: MoveFileEx the old file to `agentnotch-hook.old-<ts>.exe` (renaming a running exe is allowed), move the new one into place, and delete old copies later.

3.3 Commands (E/Core/HookCommands.swift)
- With an absolute interpreter (45-54):
  `[ -f '<script>' ] || exit 0; P='<python>'; [ -x "$P" ] || P=python3; exec "$P" -S '<script>'`
- With a bare name:
  `[ -f '<script>' ] || exit 0; P='python3'; case "$(command -v "$P")" in ''|/usr/bin/python3) /usr/bin/xcode-select -p >/dev/null 2>&1 || exit 0;; esac; exec "$P" -S '<script>'`
- Missing script → exit 0. Missing interpreter → 127, which Claude Code only logs.
- Interpreter choice (HookInstaller.swift:1070-1096): `xcode-select -p`/usr/bin/python3, then /Library/Developer/CommandLineTools/usr/bin/python3, /opt/homebrew/bin/python3, /usr/local/bin/python3, else "python3".
- Recognition (scriptPath 63-69):
  - The last simple command (after ; && || | & or newline) must run a python* interpreter word or `$P`. `exec`, `env` with options, and NAME=val prefixes are skipped.
  - The final word must expand (~, $HOME, ${HOME}) to an absolute path whose basename equals the script name.
  - Substring mentions never match.
- WIN:
  - New recognizer that accepts exec form (`command` basename equals agentnotch-hook.exe case-insensitively, and args[0] == "hook" or "statusline") and bash form with Windows paths (`C:/…`, `C:\…`, `/c/…`).
  - The current `path.hasPrefix("/")` test rejects every Windows path.
  - Compare paths case-insensitively after normalisation.

3.4 settings.json write safety (HookInstaller.swift header 13-30, applyChange 1175-1291)
- Refuses:
  - a file that is not a JSON object; both parsers must agree (E/Core/SettingsDocument.swift:28-43)
  - a `hooks` value that is not an object
  - a symlink to a missing target (`settingsLinkBroken`)
  - an existing file that cannot be read
  - A blank or missing file counts as `{}`.
- preflight (1124-1136) refuses before any script is written.
- Only `hooks` and `statusLine` are changed, by splicing into the original bytes (SettingsDocument): other keys keep their bytes, order and indentation. The comparison is by JSON equivalence (finish 772-793), so a reformat alone never writes.
- Sequence:
  1. Save the previous status line.
  2. Back up the current bytes, owner-only (0600), when they are not blank:
     - timestamped `settings.json.agentnotch-yyyyMMdd-HHmmss-SSS.bak`, newest 5 kept
     - `settings.json.agentnotch.original.bak` kept forever
     - skipped when the newest backup is identical (1397-1464)
  3. Stage `.settings.json.agentnotch-<8 hex>.tmp` in the same folder: O_EXCL, the original's permissions (default 0644), fsync (stage 1364-1381).
  4. Re-read FileState (device, inode, mtime in ns, size, bytes). If anything changed, delete the staged file and plan again, up to 5 times ("kept changing").
  5. rename(2) over the target. A symlinked settings.json is written through to its resolved target.
- Only entries whose command runs our script are touched. Groups left empty are dropped, and events left empty are dropped (removingHooks 1012-1042). An event value that is not an array is left alone.
- WIN:
  - File identity: GetFileInformationByHandleEx(FileIdInfo), i.e. volume serial plus 128-bit file id, LastWriteTime (100 ns), size and bytes.
  - Stage in the same directory, then MoveFileExW(MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH) or ReplaceFileW (keeps the ACL and attributes). Retry around 5× on ERROR_SHARING_VIOLATION or ERROR_ACCESS_DENIED (AV or indexer holds).
  - Resolve links with GetFinalPathNameByHandleW. Symlinks need Developer Mode; junctions exist for folders.
  - "Owner-only" backups: set an explicit protected DACL (current user plus SYSTEM), or rely on the profile's ACL.
  - Preserve CRLF line endings and any UTF-8 BOM in splices, or refuse a BOM. Windows editors produce both, and SettingsDocument's style detection must learn "\r\n".
  - Do not adopt upstream's installer (§12).

3.5 Registered events (hookEventConfigs 925-984)
```
UserPromptSubmit  [{"hooks":[H]}]
PreToolUse        [{"matcher":"*","hooks":[H]}]
PostToolUse       [{"matcher":"*","hooks":[H]}]
PermissionRequest [{"matcher":"*","hooks":[{"type":"command","command":CMD,"timeout":86400}]}]
Notification      [{"matcher":"*","hooks":[H]}]
Stop, SubagentStop, SessionStart, SessionEnd  [{"hooks":[H]}]
PreCompact        [{"matcher":"auto","hooks":[H]},{"matcher":"manual","hooks":[H]}]
```
H = `{"type":"command","command":CMD}`

Added by version:
| Version | Event |
|---|---|
| ≥ 2.0.0 | PostToolUseFailure (matcher "*") |
| ≥ 2.0.43 | SubagentStart |
| ≥ 2.1.33 | TaskCompleted |
| ≥ 2.1.76 | PostCompact (auto/manual) |
| ≥ 2.1.78 | StopFailure |
| ≥ 2.1.84 | TaskCreated |
| ≥ 2.1.89 | PermissionDenied (matcher "*") |

- Version nil → baseline only. Before 2.1.101, an unknown event made Claude Code ignore the whole file.
- Ours are stripped from every event first, then appended after other tools' groups.
- Effective version = min(every `claude --version` found, versions reported by status lines, `version` in live sessions/<pid>.json) (AccountHookManager.swift:523-549).
- noteVersion rewrites when a lower version appears (510-518).
- Binary candidates: ~/.local/bin/claude, /opt/homebrew/bin, /usr/local/bin, <cfg>/local/claude, ~/.bun/bin, /opt/local/bin, /usr/bin, then a login-shell probe (ClaudeBinaryLocator.swift:122-145).
- WIN:
  - Candidates: %USERPROFILE%\.local\bin\claude.exe (native installer), %APPDATA%\npm\claude.cmd (npm global), %USERPROFILE%\.bun\bin\claude.exe.
  - Prefer the versions in sessions/<pid>.json and the status line over spawning `claude --version`, which never runs in tests.
  - Also gate exec form (`args`) and `shell` on the minimum version that supports them.

3.6 Status line takeover (applyStatusLineIntent 799-862, chainTarget 868-874, restored 880-892)
- wrap:
  - No statusLine → `{"type":"command","command":SL}`, and write `{}\n` as previous.json (chainNothing).
  - Someone else's statusLine → same object, with type and command replaced. The original object is saved to previous.json first, owner-only; if that fails, nothing is taken over.
  - Our wrapper already there → update only the command. Re-save or recover the chain from backups when previous.json is lost.
  - A non-object statusLine, or SPVN's wrapper → left alone.
- unwrap (integration off, or uninstall): restore the saved object, merged with keys the user changed on the wrapper entry. A `padding: 0` that only the wrapper has is dropped. Delete previous.json.
- Chain target: the saved copy (`{}` = nothing), else the newest backup that was not taken while a wrapper was active. Backups are ranked by prefix: this name, the former name, then the originals.

3.7 Former names and legacy apps
- Former name (AppIdentity.swift:69-76): superpowered-codenotch-hook.py, superpowered-codenotch-statusline.py, …previous.json, backups `settings.json.superpowered-codenotch-*`.
  - Their entries count as ours: they are replaced in place.
  - removeFormerNameFiles deletes those scripts once settings.json no longer runs them (411-440).
- Legacy apps (92-108): claude-island-state.py (Vibe Notch), superpowered-notch-hook.py/statusline (SPVN, taken over), ~/.vibe-island/bin (Vibe Island, reported only). They are removed only on explicit user action (planLegacyRemoval 711-756).
- WIN: none of these exist on Windows. The Windows "legacy" is upstream Codenotch-Windows' entries: commands containing "codenotch-hook", "eatbean-hook" or "pacman-hook" (W/codenotch/src/hooks_install.rs:22-35).
  - They are fire-and-forget with a 5 s timeout and never answer PermissionRequest, so they can coexist.
  - Offer "take over", and never auto-remove.

3.8 readStatus (475-526) per folder
configDirExists, settingsReadable, hooksRegistered, hooksInstalled (the script exists), statusLineInstalled, legacy flags, hookCommand, newestBackupPath, lastOutcome/lastError. The UI renders these.

======================================================================
4. HOOK SOCKET SERVER AND PROTOCOL: E/Services/Hooks/HookSocketServer.swift
======================================================================

4.1 Endpoint (ClaudeControlConfiguration.swift:122-158, 215-229; server 190-268; HookSocketDirectory 731-780)
- Path: AGENTNOTCH_SOCKET, else `<support>/hook.sock`, else `/tmp/agentnotch-<uid>/hook.sock` when the path would exceed 103 bytes (sun_path).
- The folder is created 0700. The /tmp fallback folder must be a real directory owned by the uid; it is chmod-ed to 0700.
- A stale socket at the path is unlinked. A non-socket there is refused. A too-long override is refused.
- bind, chmod 0600, listen(SOMAXCONN), non-blocking accept on a GCD source.
- The bound (dev, inode) is remembered. Every 5 s: missing → rebind; replaced by another process → leave it alone (log once). stop() unlinks only its own inode.
- Status callback feeds `socketError` to the UI.

4.2 Framing (v2)
- The client writes one JSON object and half-closes. The server reads until EOF.
- Limits: 8 MiB (drop the client); 5 s to finish writing (drop).
- Decode (HookEvent.swift:552-562):
  - `event == "StatusLine"` → StatusLineMessage.
  - Otherwise lenient decode of HookEvent. session_id and event are required. Loosely typed values are coerced (lossyString/Int/Bool). pid must be in 1..Int32.max.
  - receivedAt = the server's read time. Every store timestamp uses it.

4.3 Dispatch (finishMessage 597-673)
- StatusLine → close, then deliver.
- Hook event from an ignored session → close, not delivered. Ignored means `attended == false`, or an entrypoint that starts with "sdk" (sdk-ts, sdk-py, sdk-cli = `claude -p`). claude-vscode and cli are kept (SessionFilter 461-465).
- Before handling, the ToolUseIdCache is updated (678-709):
  - PreToolUse with a tool_use_id: record (session, tool, canonical tool_input) → id (plus agent_id for subagents).
  - PostToolUse, PostToolUseFailure, PermissionDenied: remove the id and close its held request (answered in the terminal or by a rule).
  - Stop or StopFailure (main session only): forget the main session's ids and close its held requests; subagents' stay.
  - SessionEnd: all.
- A non-PermissionRequest event → close, then deliver.
- PermissionRequest:
  - If the session's folder or identity is untracked or forgotten (`passesThrough` 593-595 → FolderRings.isUntracked): close immediately with no decision, so the terminal asks.
  - Otherwise keep the fd, and find a tool_use_id:
    1. `pop(session, tool, input)` exact match, FIFO
    2. `popOnlyInFlight(session, tool, agentId)`: the single in-flight call of that tool by the same agent (another hook rewrote the input)
    3. Else a synthetic `"permission-<UUID>"` with hasSyntheticToolUseId = true
  - A stale pending request with the same id is closed. PendingPermission{sessionId, toolUseId, agentId, fd, event, receivedAt} is stored and delivered.
- Cache key: `"<sessionId>:<toolName|unknown>:" + JSON(tool_input with sorted keys)`. Entries expire after 60 min (830-947).
  - WIN: any canonical JSON works, as long as PreToolUse and PermissionRequest use the same function. Both come through the hook's identical truncation.

4.4 Responses (respondToPermission 368-388, sendPermissionResponse 423-448, writeAll 516-536)
- Encoded PermissionResponse, with nil optionals omitted: `{"decision":…, "reason"?, "updated_input"?, "updated_permissions"?, "interrupt"?}`.
- Written with a blocking write (SO_SNDTIMEO 2 s), then close.
- Returns false when the id is not pending, or the peer is gone (poll POLLOUT shows POLLHUP/ERR/NVAL).
- A liveness timer (every 2 s while anything is pending) drops requests whose hook died (it timed out, or Claude Code aborted it because the terminal answered first). It calls permissionFailureHandler(sessionId, toolUseId) → store `.permissionSocketFailed` (477-513).
- Cancel APIs close without answering, so Claude Code's own dialog stays: `cancelPendingPermissions(sessionId:)`, `(toolUseIds:)`.

WIN transport recommendation: named pipe, not loopback HTTP.
- Name: `\\.\pipe\agentnotch-hook-<current user SID>`. Both sides compute it, so no templating is needed. The dev override is AGENTNOTCH_SOCKET together with AGENTNOTCH_DEV=1.
- Server:
  - CreateNamedPipeW, byte mode, with a DACL granting only the current user SID (plus SYSTEM), PIPE_REJECT_REMOTE_CLIENTS, and FILE_FLAG_FIRST_PIPE_INSTANCE on the first instance. Failure with ACCESS_DENIED = a squatter or another instance; report it as socketError.
  - Keep several instances listening with overlapped ConnectNamedPipe. Create the next instance before serving the connected one; each held PermissionRequest consumes an instance.
  - tokio `ServerOptions::first_pipe_instance(true).reject_remote_clients(true)` plus raw security attributes.
- Framing: pipes have no half-close. Use `[u32 LE length][UTF-8 JSON]` from the client.
  - Response: `[u32 len][JSON]` then close, or close with no frame = no decision.
  - Keep the 5 s read deadline, the 8 MiB cap and the 1 MiB client cap.
- Client (hook):
  - CreateFileW. ERROR_FILE_NOT_FOUND → app not running → exit 0. ERROR_PIPE_BUSY → WaitNamedPipeW with 1 s, retry once.
  - Verify the server is ours: GetNamedPipeServerProcessId → OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) → OpenProcessToken → TokenUser SID == own SID. This is the analog of `is_own_socket` st_uid. Pipe names are global, so another user can pre-create the name.
- Peer-alive: an overlapped ReadFile pending on each held handle completes with ERROR_BROKEN_PIPE when the hook exits. That is event-driven; PeekNamedPipe polling every 2 s also works.
- Loopback TCP (upstream: 127.0.0.1:48666, tiny_http) is not acceptable for permission answers. Any local user can bind the port first, then receive tool inputs and approve arbitrary tools. Browsers can reach it (upstream added Origin checks). The peer's user cannot be checked cheaply.

======================================================================
5. SESSION PIPELINE, STATE MACHINE, RULES
======================================================================

5.1 Ordering (E/Services/Session/HookEventPipeline.swift; ClaudeSessionMonitor.swift:108-139)
- One unbounded AsyncStream with one consumer awaits each input: socket messages, registry snapshots, permissionFailed, and ordered session events (a permission outcome after the answer was written; interrupts).
- Side effects (interrupt watcher start/stop, the statusLine bus, account sightings throttled to one per session per 60 s) are sent to the main queue in FIFO order.
- The store is an actor. File IO happens before `.fileUpdated` so there is no re-entrancy.
- Publishing is coalesced to 50 ms and skipped when nothing changed (SessionStore.swift:1838-1861). Sort order: (projectName, sessionId).
- WIN: tokio mpsc (unbounded) with one consumer task that owns the store struct. Transcript IO runs in spawn_blocking, and results come back as a `FileUpdated` input. UI side effects go to Tauri's main thread in order.

5.2 Phases (E/Models/SessionPhase.swift)
- Phases: idle, processing, waitingForInput, waitingForApproval(PermissionContext), compacting, ended.
- canTransition (172-236):
  - ended has no exits; anything → ended.
  - idle → processing | waitingForApproval | compacting | waitingForInput
  - processing → waitingForInput | waitingForApproval | compacting | idle
  - waitingForInput → processing | idle | compacting | waitingForApproval
  - waitingForApproval → processing | idle | waitingForInput | waitingForApproval
  - compacting → processing | idle | waitingForInput | waitingForApproval
  - Same → same is allowed.
- PermissionContext: toolUseId, toolName, toolInput (full, nested), receivedAt, permissionSuggestions, hasSyntheticToolUseId, agentId (nil = main session), activatedAt.
- Event → target phase (determinePhase, E/Models/SessionEvent.swift:187-232):
  | Event | Target phase |
  |---|---|
  | PreCompact | compacting |
  | PostCompact | trigger == "manual" ? waitingForInput : processing |
  | PermissionRequest | waitingForApproval(ctx) |
  | Notification | idle_prompt → waitingForInput, else nil |
  | SessionStart with source "compact" | nil |
  | SessionEnd | nil (the session is removed) |
  | Otherwise, by status | waiting_for_input → waitingForInput; running_tool, processing, starting → processing; compacting → compacting; else nil |
- resumesTurn (241-251): not a subagent, and (UserPromptSubmit or PreToolUse, or PreCompact/PostCompact with trigger != "auto").
- shouldSyncFile: UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop, StopFailure, SubagentStop.

5.3 The five user-facing states (SessionAttention.derive, E/Models/SessionAttention.swift:216-249), in priority order:
1. waitingForApproval → needsInput(.question for AskUserQuestion | .planApproval for ExitPlanMode | .permission(tool)).
2. needsInputReason set → needsInput(reason).
   - Reasons: permission(tool), question, planApproval, elicitation(msg), dialog(detail), error(humanized).
   - "failed" = needsInput(.error). It sorts after answerable reasons.
3. processing or compacting → working.
4. completionPendingSince != nil, or backgroundWaitSince != nil → working.
5. Phase waitingForInput or idle, completedAt set, and reviewedAt nil or < completedAt → readyForReview. Otherwise idle.
- Buckets, in order: needsInput, readyForReview, working, idle.
- StopErrorKind (105-155): rate_limit → "Rate limited"; overloaded; server_error; authentication_failed, oauth_org_not_allowed, account_on_hold, cloud_credential_error → "Sign-in failed"; billing_error; invalid_request, model_not_found; max_output_tokens → "Output limit reached"; other → "Turn failed". Unknown codes are humanized from snake_case.

5.4 processHookEvent (E/Services/State/SessionStore.swift:244-299)
- SessionEnd → removeSession: kept in recentlyEnded for 10 min so late data cannot revive it; held sockets closed; parser state forgotten.
- New session: created at cwd with projectName = last path component. The review record is restored (completedAt, reviewedAt, lastAssistantMessage, background wait, and a failure → needsInput(.error)).
  - A session first seen with anything other than SessionStart startup/clear gets its tasks reconstructed from the transcript.
- applyMetadata (302-332):
  - A pid change → adopt: kernel start time, tty and isInTmux (337-348).
  - currentCwd.
  - transcriptPath (subagent events only when none is set yet).
  - accountId = resolvedConfigDir (transcript path, else config_dir_env, else ~/.claude).
  - configDirEnv, entrypoint, permissionMode (main session only), model, title (source hook).
- lastActivity, lastEventAt and lastHookEventAt = receivedAt.
- A main-session resumesTurn or StopFailure clears completionPendingSince.
- applyPhase (352-408):
  - waitingForApproval → enqueueApproval.
    - Same id → refresh.
    - Another active → append to queuedApprovals.
    - Else activate: phaseAfterApprovals = the current waitingForInput/idle when a background agent asks after Stop (new session → processing), else processing.
  - Subagent events never move to waitingForInput, idle or ended.
  - waitingForInput/idle → processing/compacting only when isNew or resumesTurn. This is the late-event protection.
  - waitingForApproval → processing/compacting: only a main UserPromptSubmit, which drops the main session's approvals.
  - waitingForApproval + idle_prompt Notification → ignored.
  - waitingForApproval → anything else (Stop, StopFailure, SessionStart) → dropMainSessionApprovals then target.
  - Otherwise, when the transition is valid.
- applyLifecycle (412-523), main session only:
  - UserPromptSubmit:
    - turnStartedAt = now; tracker endMainTurn; clearFailure.
    - agentsAtTurnStart = knownWakingAgents, wakeupsAtTurnStart = knownWakeups.
    - Background counts reset to 0; scheduledWakeupCount = 0.
    - lastPromptSource; lastPromptWasUserAuthored = isUserAuthoredPrompt; completionCheckSince = nil.
    - A user-authored prompt → reviewedAt = now.
    - isUserAuthoredPrompt (HookEvent.swift:298-310):
      - source nil or "user", and the prompt is not injected.
      - source "sdk" only when the entrypoint starts with "claude-" and the prompt is non-empty and not injected.
      - system, loop_wakeup, schedule_wakeup and poll_event never count.
      - Injected prefixes after leading whitespace: `<task-notification>`, `<command-name>`, `<command-message>`, `<local-command`, `<bash-input>`, `<bash-stdout>`, `Caveat:`.
      - A background task finishing wakes Claude with a UserPromptSubmit whose prompt starts `<task-notification>` and which has no source. That is not user-authored, so it does not review and does not count.
  - Stop:
    - wasWorking = previous phase nil, processing, compacting or waitingForApproval; else stop_hook_active == true or backgroundWaitSince != nil.
    - finalMessage = last_assistant_message, else the transcript's lastMessage. isContextResume if it starts with "This session is being continued from a previous conversation".
    - Store lastAssistantMessage.
    - awaited = background_task_types ∩ BackgroundWork.awaitedTypes: {subagent, agent, local_agent, workflow, local_workflow, teammate, in_process_teammate, cloud session, remote_agent, remote_session}.
    - backgroundTaskCount = count or types.count; backgroundAgentCount/Types = awaited; backgroundWaitSince = awaited non-empty ? now : nil.
    - scheduledWakeupCount = session_cron_count; knownWakingAgents and knownWakeups updated.
    - clearFailure; subagentState reset; tracker endMainTurn.
    - If wasWorking and not a context resume: completionPendingSince = now, settle now, and schedule a recheck.
    - rescanRegistry(account) with quick rescans at 0.3 s and 1.2 s; settleBackgroundWait.
  - StopFailure: stopError (humanized), stopErrorCode, stopErrorAt; background counts reset; subagents reset; settleBackgroundWait. The wait stands.
  - SessionStart with source "clear" → reviewedAt = now, and tasks reset (SessionTaskList.apply).
  - Notification idle_prompt: fires about 60 s after every turn. It confirms a pending completion immediately.
- applyNeedsInput (532-576):
  - StopFailure (main) → .error.
  - UserPromptSubmit, PreToolUse, PostToolUse, Stop, PermissionRequest: main session clears the reason; a subagent clears only a .permission reason.
  - Notification (ignored if it is an agent-view announcement: type agent_completed, or agent_needs_input whose message matches `^.+ needs your input(:|$)`):
    - elicitation_dialog / elicitation_url_dialog → .elicitation(message)
    - agent_needs_input / worker_permission_prompt → .dialog(message ?? title)
    - permission_prompt → .permission(tool parsed from "…permission to use <Tool>") when no approval is showing
    - elicitation_complete / elicitation_response clear an elicitation
- PermissionRequest with an id → tool item status waitingForApproval, tracker pendingApproval.
- tasks.apply(event) (§5.9); tool tracking; subagent tracking.
- PreToolUse: tracker startTool (max 64); a chat placeholder item (id = tool_use_id, flat top-level scalar input, status running) unless it is a subagent's tool.
- PostToolUse → success; PostToolUseFailure → interrupted if is_interrupt, else error; PermissionDenied → error. The tracker completes and resolveApproval(id).
- Task/Agent container PreToolUse (main) → subagentState.startTask(description). Inner tools of an active subagent are attached live. The container's PostToolUse → stopTask.
- File sync: debounced 100 ms, one per session at a time.

5.5 Turn completion (E/Services/State/TurnCompletion.swift)
Our Stop hook runs in parallel with other Stop hooks; a blocking one (/goal) continues the turn. decide(stopAt, …):
- registry idle or shell with changedAt ≥ stopAt − 1 s → confirm.
- The registry follows the turn (busy or waiting since turnStart − 1 s) → confirm after 90 s.
- Else confirm after 4 s.
- confirm → completedAt = stopAt (→ readyForReview unless reviewed later).
- An idle_prompt Notification also confirms.

5.6 Background wait (E/Models/BackgroundWork.swift:90-106)
- registry idle or shell → end at max(changedAt, since) + 10 s grace.
- Otherwise (busy, or not followed) → end after 30 min without any hook event.
- end → the wait is cleared, and completedAt = max(completedAt, endedAt), making it ready for review.
- A wake-up turn (task-notification) ends it through its own Stop.
- "Waiting on 1 workflow / 2 background agents and 1 teammate" (phrase 37-48).

5.7 Quiet completions (SessionState.swift:311-317)
A completion is quiet when any of these holds:
- lastPromptSource is loop_wakeup or schedule_wakeup;
- scheduledWakeupCount > wakeupsAtTurnStart;
- the prompt was user-authored and backgroundAgentCount > agentsAtTurnStart;
- the prompt was not user-authored and backgroundAgentCount > 0.

Quiet completions stay in the review queue and are never announced.

5.8 Registry reconciliation (processRegistrySnapshot 989-1101; createSession(fromRegistry) 1118-1154)
- A pid now naming another session id (after /clear or /resume with no SessionEnd) → remove the old session, unless hooks spoke after the registry changed.
- New entry → session created from it:
  - busy → processing (if a background wait was restored → waitingForInput)
  - waiting → waitingForInput + .dialog(waitingFor)
  - else idle, with completionCheckSince = the previous run's lastAliveAt, when the entry went idle before launch + 5 s
- Existing entry:
  - Update pid, entrypoint, hostSessionId and name.
  - On a status change: settleAnsweredSyntheticRequest (busy after a synthetic main request arrived → close it and resolve).
  - Only if changedAt > lastHookEventAt, reconcile:
    - idle or shell:
      - processing/compacting + hooks → idle (an interrupt, not a completion)
      - processing/compacting without hooks → waitingForInput + completionCheckSince (the transcript decides)
      - waitingForApproval → drop main approvals → idle
      - else clear a .dialog
    - waiting: while processing → .dialog(waitingFor)
    - busy: idle/waitingForInput → processing (not while a completion or background wait is pending); clear .dialog
  - Then settle completion and background wait.
- inferCompletion (1336-1352), on the next sync: only when the registry says idle/shell, the phase is idle/waitingForInput, and no completion is pending, and the transcript ends with a reply newer than max(since, completedAt, reviewedAt), any human prompt and any interrupt.

5.9 Task progress (E/Models/SessionTaskList.swift)
- Hooks, main session only:
  - PreToolUse TaskCreate → pending create keyed by tool_use_id (subject, description, activeForm).
  - PostToolUse TaskCreate carries task_id → task created.
  - PostToolUseFailure TaskCreate → failed.
  - PreToolUse TaskUpdate {taskId, status (pending, in_progress, completed, deleted), subject, activeForm}.
  - TodoWrite replaces the todos (content, status, activeForm).
  - TaskCreated and TaskCompleted events.
  - SessionStart clear → reset.
- A new create when every task is completed starts a new batch.
- Display: Task* tasks if any, else todos. Counts, fraction, activeItem = first in_progress. activeLabel = activeForm or subject.
- Mid-flight sessions: rebuilt from the transcript (non-sidechain tool_use blocks; results parsed via toolUseResult.task.id or "Task #<id> created successfully"; /clear resets), then merged with hook data, hook data winning.

5.10 Context % and titles
- Status line used_percentage clamped to 0...100, plus window size; it wins.
- Else estimate (ContextUsageEstimator, ConversationParser.swift:361-379):
  - tokens = input + cache_creation + cache_read of the latest main-thread, non-synthetic assistant response
  - window = the status line's size, else 1,000,000 if the model id contains "[1m]" or tokens > 200k, else 200,000
  - percent = tokens / window × 100, clamped
- Model: status line display_name/id, else SessionStart model, else the transcript.
- Title priority (SessionState.swift:404-448):
  1. hook session_title (SessionStart/UserPromptSubmit)
  2. registry `name` or status line `session_name`
  3. transcript `custom-title` (/rename), else `ai-title`, else `summary`
  4. registry name with nameSource "derived"
- displayTitle falls back to summary, first user message (50 chars), then project name.
- Notification publicTitle never uses the first prompt (ClaudeHostProjections.swift:189-198).

5.11 Transcript reading and chat display
- Paths: `<cfg>/projects/<slug>/<sid>.jsonl`. The slug replaces every UTF-16 unit outside [A-Za-z0-9] with "-" (TranscriptLocator.swift:21-32). Over 200 chars it is truncated and hashed, so the hook's path is preferred.
- Subagents: `<project>/<sid>/subagents/agent-<id>.jsonl`, or `subagents/workflows/<id>/…`, or flat `<project>/agent-<id>.jsonl` (122-141).
- Reads are incremental: 8 MiB chunks split on 0x0A, only complete lines (TranscriptLineReader). A shrink means reset.
- Parsing (ConversationParser.swift):
  - summary, ai-title and custom-title lines.
  - user/assistant, skipping sidechain and meta entries.
  - Chat items: text (user or assistant), tool_use (flat input), thinking, image (base64 plus media type), and an interrupt marker "[Request interrupted by user".
  - Item id = tool_use id, or `<uuid>-<text|tool|thinking|image|interrupted>-<blockIndex>`.
  - tool_result → status success, error or interrupted (error with "Interrupted by user", "interrupted by user" or "user doesn't want to proceed"). Result text = stdout, else stderr, else content. Structured results come from toolUseResult (ToolResultParser).
  - Agent results → the subagent transcript is followed; it is settled after completion or 10 min idle; max 64.
  - Usage de-duplicated by message.id + requestId.
  - /clear drops earlier items.
- Retention: the store keeps 40 items per session (plus running tools) unless the chat is open. The parser hands over 30 per sync. Open chats: at most 2 full histories (ChatHistoryManager). Opening a chat marks the session reviewed (U/Views/ChatView.swift:71-74).
- isHumanPrompt: type user, not meta, no toolUseResult, not a compact summary, origin.kind is "human" or missing, and not an injected prefix.
- Interrupt watcher (JSONLInterruptWatcher.swift):
  - Runs while a main turn is processing; stops at Stop, StopFailure or SessionEnd.
  - Waits up to 30 × 1 s for the file to appear.
  - A line is an interrupt when it contains `"type":"user"` plus "[Request interrupted by user", or it is a tool_result line with `"is_error":true` plus an interrupt phrase, or it contains `"interrupted":true`.
  - → `.interruptDetected` (ignored if a newer turn started): tools marked interrupted, main approvals dropped, → idle, background wait kept.
- WIN:
  - Open files with FILE_SHARE_READ|WRITE|DELETE (Rust std's default).
  - DispatchSource(.write/.extend) → poll GetFileSizeEx every 250 ms on the watched transcripts, with ReadDirectoryChangesW or `notify` as a hint only. NTFS directory metadata lags for files held open by the writer.
  - Slug of `C:\Users\me\proj` = `C--Users-me-proj`.

5.12 Periodic check (every 3 s, recheckAllSessions 1786-1824)
- Remove ended sessions.
- Remove sessions whose pid is not running, or whose start time differs by ≥ 1 s (pid reuse).
- Remove pid-less (status-line-only) sessions after 15 min of silence.
- Re-sync processing and waitingForApproval sessions.
- WIN:
  - Liveness: OpenProcess(SYNCHRONIZE|QUERY_LIMITED) + WaitForSingleObject(0). ACCESS_DENIED means it exists.
  - Start time: GetProcessTimes creation time, 100 ns.
  - Windows reuses pids quickly, so the start-time check is essential.

5.13 Review queue persistence (E/Services/State/ReviewStateStore.swift)
- File: `<support>/review-state.json`, JSONEncoder with sorted keys and dates as epoch seconds:
  `{"version":2,"lastAliveAt":<s>,"sessions":{"<sid>":{"completedAt","reviewedAt","lastAssistantMessage"(≤1500),"stopError","stopErrorCode","failedAt","backgroundWaitSince","backgroundAgentTypes","updatedAt"}}}`
- Also reads SPVN's bare `{sid: record}` shape.
- Written atomically, 0600. Completion or failure changes are written immediately; review marks after a 1 s debounce. Heartbeat every 30 s. Prune after 7 days.
- Failures are persisted only while they still count (hasFailedTurn).
- Restored review state is reconciled on the first sync (reviewRestoredState 1318-1329): a prompt after the completion → reviewed; a prompt or reply after a failure → the failure is cleared.
- User review actions:
  - markReviewed(at click time)
  - markViewed(completedAt): only that completion
  - markAllReviewed
  - dismissFailure
  - hooksTurnedOff: lastHookEventAt = nil everywhere
  - dropAccountSessions
- WIN: `%LOCALAPPDATA%\Agent Notch\Claude\review-state.json`. Use LOCALAPPDATA because the state is machine-specific (pids). Upstream uses %APPDATA%\codenotch for its own config. Atomic write as in §3.4.

======================================================================
6. PERMISSIONS, QUESTIONS, PLANS (engine and UI)
======================================================================

- Answers always name the exact tool_use_id the UI showed. If that request is no longer pending, nothing happens (ClaudeSessionMonitor.swift:186-307).
- Approve (190-211):
  - AskUserQuestion is refused; it must use answerQuestion.
  - updatedPermissions = [permissionSuggestions.first] when alwaysAllow.
  - updatedInput = [:] for ExitPlanMode (the echo; `toolsNeedingInputEcho`, line 263).
- Deny: reason nil → the hook's default message. Plan "Keep planning" = deny with keepPlanningReason.
- answerQuestion(answers): allow with updatedInput {"answers": {questionText: answer}}.
- `interrupt` exists in the protocol but the UI never sets it.
- After writing the answer:
  - not delivered → `.permissionSocketFailed` (resolve only)
  - deny → `.permissionDenied`
  - allow → `.permissionApproved`
  - Queued through the pipeline so it cannot overtake a PostToolUse.
- processPermissionResolved (799-818): the tool goes to running (approved) or error (denied) only if still pending; resolveApproval activates the next queued request (activatedAt = now) or returns to phaseAfterApprovals; a non-error needs-input reason is cleared.
- Held-socket cleanup comes from the store's effects: dropMainSessionApprovals (Stop, StopFailure, SessionStart, a new prompt, registry idle, interrupt) closes the main session's held sockets with no answer and marks those tools interrupted. Background agents' requests stay (769-797).
- UI:
  - Panel actions: U/Public/ClaudeSessionsPanel.swift:133-145.
  - AnswerGate (U/Panel/AnswerGate.swift): a newly shown request can't be answered for 0.35 s; each request can be answered once; answered ids are remembered for 600 s.
  - "Always allow" text (U/Components/Chat/ChatApprovalBars.swift:17-68):
    - addRules/replaceRules → "Don't ask again for Tool(content) [for this session | in this project (just you) | in this project (shared) | in all projects]"
    - setMode → "Switch to <mode> mode"
    - addDirectories → "Allow access to <names>"
  - Offered inline only for narrow rules (addRules or replaceRules with destination nil, session or localSettings) that have a description (E/Attention/SessionRowContent.swift:296-307).
  - Questions (U/Components/Chat/ChatQuestionForm.swift:57-181): parsed from `{"questions":[{"question","header","multiSelect","options":[{"label","description"}]|["label"]}]}`.
    - The answer key is the raw question text.
    - multiSelect: labels in option order joined ", ", plus the Other text.
    - Single: the label, or the Other text.
    - Submitted only when every question has an answer.
    - A single single-choice question with 1-4 options answers with one tap.
- WIN: identical logic. Only the transport changes (§4 WIN).

======================================================================
7. NOTIFICATIONS: E/Services/Notifications/NotificationService.swift
======================================================================

- Driven by AttentionTracker transitions (AttentionTracker.swift).
- Silent baseline: until every registry has been read once, plus 2 s (hard cap 15 s after start).
- News (AttentionNews.swift:33-60):
  - resolved: left needsInput or readyForReview
  - needsInput: newly, or for a different reason
  - readyForReview: newly, not quiet, and completedAt ≥ launch
- Bursts are handled together within 150 ms, with one focus check per session.
- Suppressed while the user looks at that session's own tab (§9.3), or while its account is hidden or forgotten.
- Content (104-204): title ≤ 60 chars, body ≤ 220, "…" truncation.
  - needsInput: title "<publicTitle> needs you"; subtitle = account label (only with several accounts; organization, plan or folder added when names collide).
  - Bodies:
    - permission: "Approve <Tool>: <offPanelPreview>", or "Approve <Tool>". The off-panel preview is only the command, file name, pattern, URL host or query (HookEvent.swift:398-417).
    - question: "Question · <header ≤24>[ (+N more)]", or "Question for you[ (+N more)]"
    - plan: "Plan ready for approval"
    - elicitation/dialog: reason.displayText
    - error: "<reason> · <reset>" for a rate limit
  - failed (not a rate limit): title "<title> stopped", body "<reason>[ · retry in its terminal | run /login in its terminal | check the account's billing | ask Claude to go on in smaller steps]".
  - readyForReview: title "Done: <title>", body "Ready for review · <project>[ · N background task(s) running]".
  - Rate limit: one banner per account ring, identifier `agentnotch.limit.<ringID>`.
    - Title "<Account>: N sessions hit the limit", or "<title> hit the limit".
    - Body "Rate limited · <reset>", or "Rate limited. Claude Code waits for you once it lifts."
    - Re-posted only when new sessions join; withdrawn at zero.
- Identifiers `agentnotch.<needsInput|review|failed>.<sid>` (thread = sid) replace older banners. They are withdrawn when the state no longer applies, when the session goes away, and 10 s after launch if stale.
- Banners are silent; the app chimes itself.
- Categories: needsInput and failed and limit have "Open"; review has "Open" and "Mark Reviewed". A click opens the panel on the session or ring. Default on for needsInput and readyForReview.
- WIN:
  - WinRT toasts through the `windows` crate (ToastNotificationManager). An AppUserModelID must be registered by a Start-menu shortcut from the installer.
  - Tag = kind, Group = sessionId (each ≤ 64 chars) for replace and remove via ToastNotificationHistory.Remove(tag, group).
  - `<audio silent="true"/>`.
  - Activation arguments carry the kind and sid. "Mark Reviewed" is a background activation.
  - tauri-plugin-notification cannot tag or remove toasts, so do not use it here.

======================================================================
8. CHAT REPLIES (typing into the terminal)
======================================================================

8.1 macOS routes (E/Services/Window/SessionMessenger.swift, TerminalScripting.swift, Tmux/TmuxMessageSender.swift)
- Route: tmux if the session is in tmux (pane found by pane_tty); else iTerm2 or Terminal.app by host app; else unavailable ("Replies can be typed into tmux, iTerm2 and Terminal sessions"). A valid TTY is required: `/dev/<alnum>` only.
- Text: trimmed; newlines, CR and tab become spaces; control characters dropped (TerminalScript.singleLine 71-83), because Claude Code submits on every newline. There is no escaping of leading "/", "!" or "#": they act as Claude Code commands.
- tmux: `tmux send-keys -t <pane> -l -- <text>`, then `send-keys -t <pane> Enter`, with a recheck before each.
- iTerm2 (AppleScript by TTY):
  - `tell s to write text "<text>" newline NO`
  - wait 0.15 s
  - recheck
  - `write text (character id 13) newline NO`
  - Return is separate, because a single write can look like a paste, or like Ctrl+J.
- Terminal.app: `do script "<text>" in t`, which includes the newline.
- Never System Events keystrokes. osascript runs serialized, with a 90 s timeout (Automation prompt) and 5 s for queries. The Automation prompt is raised first with a harmless selected-tab query.
- Safety (MessageSafety 60-111), checked at availability, before each script or keystroke, and between the text and Return:
  - Refused while a non-error needsInput shows:
    - question: "Answer the question in the terminal or above"
    - plan: "Approve or reject the plan first"
    - permission: "Answer the permission prompt first"
    - elicitation, dialog: "Answer in the terminal: Claude is showing a prompt"
  - The process must be alive ("The session's process has ended").
  - The kernel tty must equal the session tty ("… can't be confirmed").
  - Not stopped ("suspended … (Ctrl+Z)").
  - Its process group is the terminal's foreground group ("isn't in the foreground …").
  - Busy: with hooks, any main-session tool in flight, or any tool while working. Without hooks, any working state. Such a send is held and polled every 0.25 s for up to 10 s, then refused.
- Outcomes: delivered; refused(reason) (draft kept: "Not sent: <reason>. Your message is kept."); typedNotSubmitted(reason) (iTerm2: "Typed but not submitted: … Press Return in the terminal when it's safe."); failed.
- claude-vscode (extension/SDK) sessions have no terminal → unavailable.

8.2 WIN mapping
- Use the console input buffer. Never SendInput or keybd_event (they type into the foreground window), and never PostMessage WM_CHAR to Windows Terminal (unreliable).
- Mechanism: a short-lived helper, e.g. `agentnotch-hook.exe type --pid <claudePid>` with the text on stdin, spawned with CREATE_NO_WINDOW. The GUI app must not attach to a console itself: Ctrl+C and close events would reach it.
  1. FreeConsole()
  2. AttachConsole(pid). Failure → unavailable: no console (claude-vscode, SDK), mintty/winpty, or an elevated target.
  3. SetConsoleCtrlHandler(NULL, TRUE)
  4. CreateFileW("CONIN$", GENERIC_READ|GENERIC_WRITE, FILE_SHARE_READ|WRITE)
  5. Safety checks (below)
  6. WriteConsoleInputW with KEY_EVENT down/up pairs per UTF-16 unit (uChar = unit, wRepeatCount = 1)
  7. Sleep 0.15 s, recheck
  8. VK_RETURN down/up with uChar '\r'
- This works for classic conhost and for ConPTY (Windows Terminal, VS Code integrated terminal), because the input buffer belongs to the console server the claude process is attached to, independent of which window or tab has focus. VS Code integrated terminals become typeable, which macOS cannot do.
- Safety mapping:
  - tty identity → console identity: record GetConsoleWindow() (the conhost HWND, or the ConPTY pseudo-window) at adoption; after attaching, it must match.
  - Ctrl+Z stopped → N/A natively.
  - Foreground process group → GetConsoleProcessList(). It must contain claudePid, and every other attached pid (excluding the helper) must be an ancestor (the launching shell) or a descendant of claude, with parent links validated by creation time.
  - Keep the dialog and busy rules as they are.
  - The last-moment recheck runs inside the helper, or through a second helper call for Return.
- Risks, to verify on native claude.exe (Bun) and npm node:
  - Claude Code's raw-mode reader may treat a burst of records as a paste; keep the separate Return with the delay.
  - IME and surrogate handling.
  - An elevated (Administrator) Claude cannot be attached from a non-elevated app → unavailable ("Claude runs as administrator").
  - Git Bash/mintty windows are not consoles → unavailable.
  - WSL and tmux are out of scope.

======================================================================
9. JUMP TO TERMINAL, AND "IS THE USER LOOKING AT IT"
======================================================================

9.1 macOS plan (FocusPlanner.plan, SessionFocusService.swift:97-138). Steps are tried in order; success marks the session reviewed.
1. tmux:
   - list-panes → the pane whose pane_pid is an ancestor of claude
   - select-window, select-pane
   - list-clients → per client: iTerm2/Terminal tab by client tty; Ghostty/cmux through the host's externalTabFocus
   - else yabai window focus
   - else activate the client's app
2. iTerm2/Terminal.app: AppleScript selects the session or tab whose `tty` matches, raises the window and activates (TerminalScript.focus 86-125).
3. Ghostty/cmux: the host app's `externalTabFocus(bundleID, pid, tty, cwd)` (Sources/ClaudeBridge/CodenotchTabFocus.swift, via upstream TerminalTabFocus), then activate.
4. VS Code family (Code, Insiders, Cursor, Windsurf, VSCodium):
   - claude-vscode → open cwd with the editor
   - CLI in the editor's terminal → open its git toplevel (WorkspaceRoot: nearest .git dir or file below home), else just activate
   - "Opening" a folder focuses the window that has it open.
5. Activate the nearest ancestor that is a regular app.

Host resolution (SessionHostResolver.swift:114-131):
- Walk parent pids (sysctl process table, proc_pidpath).
- First ancestor that is a regular NSRunningApplication; else match by the `.app` bundle path of the executable, or by helper hints (iTermServer → iTerm2, wezterm-mux-server → WezTerm).
- Cached per pid; unresolved hosts retried after 30 s (HostAppCache.swift).

9.2 WIN mapping
- Process tree: CreateToolhelp32Snapshot (pid, ppid, exe) plus QueryFullProcessImageNameW. Validate each parent link with creation times (a parent's creation time must be ≤ the child's): Windows ppids go stale.
- Host classification by exe: WindowsTerminal.exe; OpenConsole.exe and conhost.exe (console hosts); Code.exe, "Code - Insiders.exe", Cursor.exe, Windsurf.exe, VSCodium.exe; wezterm-gui.exe; alacritty.exe; Hyper.exe; Tabby.exe; mintty.exe; ConEmu64.exe; JetBrains *64.exe.
- Classic conhost: the conhost is a child of the shell, not an ancestor. Get it exactly via AttachConsole(pid) + GetConsoleWindow() in the helper, then SetForegroundWindow (+ ShowWindow(SW_RESTORE) when iconic).
- Windows Terminal (one process owns every WT window):
  - Window: GetConsoleWindow() of claude's console returns the ConPTY pseudo-window. Its owner or root owner (GetWindow(GW_OWNER) / GetAncestor(GA_ROOTOWNER)) should be the hosting WT window (class CASCADIA_HOSTING_WINDOW_CLASS). Verify the minimum WT version.
  - Tab: UI Automation on that window. Find ControlType.TabItem whose Name equals the console title (GetConsoleTitleW while attached), then SelectionItemPattern.Select.
  - 0 or ≥ 2 matches → just raise the window (the "not_found" analog). Renamed tabs and suppressApplicationTitle defeat title matching. There is no public WT API by pid or WT_SESSION. `wt -w <id> focus-tab -t <index>` needs the index, which UIA's order gives.
  - Panes cannot be targeted.
- VS Code family: launch `<resolved editor exe> <folder>`, the exe path taken from the host process (not `code.cmd`; do not inherit ELECTRON_RUN_AS_NODE). This forwards to the running instance and focuses the window with that folder. Same workspace-root rule as macOS. The exact terminal tab cannot be selected from outside (same as macOS).
- Foreground rights: the user clicked our window, so SetForegroundWindow from the app's UI thread is allowed. If a helper does it, call AllowSetForegroundWindow(helperPid) first. No ALT-key or AttachThreadInput hacks. UIA against an elevated WT from a non-elevated app fails (UIPI).
- Optional: the hook can forward a whitelist of terminal identity variables to strengthen matching: WT_SESSION, TERM_PROGRAM (=vscode), VSCODE_IPC_HOOK_CLI (targets the right VS Code window). Never forward the whole environment.

9.3 "Is the user looking at this session" (E/Utilities/TerminalVisibilityDetector.swift:133-172; hub ClaudeControlHub.swift:554-617)
- macOS:
  - tmux: the pane is the active pane of a client whose terminal is frontmost (and whose tab is selected).
  - iTerm2/Terminal: the host is frontmost and the selected tab's tty equals the session tty. The read-only query runs only if Automation is already granted (AEDeterminePermissionToAutomateTarget with no prompt), cached for 1 s.
  - Otherwise: the host is frontmost and no other session runs in that app.
- Uses:
  - suppress banners
  - markViewed when a completion happens with its tab in front
  - after an app activation that stays 1.5 s, markViewed for ready sessions whose tab is in front
- Visible terminal check: CGWindowList front to back, layer 0, alpha > 0.05, at least 40×40 px; ≥ 15% of a 5×5 sample grid not covered.
- WIN:
  - Foreground: GetForegroundWindow → pid, HWND.
  - Exact: conhost → foreground HWND == claude's console window. WT → foreground is the owning WT window and its selected TabItem's Name == console title (UIA). VS Code → only-session-in-app rule.
  - Activation events: SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT).
  - Visibility: EnumWindows z-order with DWMWA_EXTENDED_FRAME_BOUNDS, skipping cloaked windows (DWMWA_CLOAKED, which covers other virtual desktops) and minimized ones; same grid rule.

======================================================================
10. SESSION REGISTRY READER AND PROCESS ATTRIBUTION
======================================================================

10.1 Registry (E/Services/Session/SessionRegistryScanner.swift)
- Files: `<cfg>/sessions/<pid>.json`. Only names matching `<digits>.json` and under 1 MB are read. `*.key` is never opened.
- Entry fields: pid, sessionId (both required), cwd, kind (interactive|bg|daemon|daemon-worker), entrypoint, name, nameSource ("derived"), version, status (busy|idle|shell|waiting), waitingFor ("permission prompt"|"input needed"|"dialog open"|"sandbox request"|"worker request"), startedAt/updatedAt/statusUpdatedAt (epoch ms), procStart ("EEE MMM d HH:mm:ss yyyy", UTC, from ps lstart), hostSessionId (`local_…`, Desktop only).
- Tracked: kind interactive (or missing), entrypoint not sdk*.
- statusChangedAt = statusUpdatedAt ?? updatedAt.
- Live check:
  - The pid is running.
  - With procStart: |kernel start − procStart| ≤ 2 s.
  - Else with startedAt: kernel start ≤ startedAt + 5 s.
  - Else live.
- Scanning:
  - Every 3 s, plus quick rescans at 0.3 s and 1.2 s after a Stop.
  - Every physical sessions folder is read once, however many config folders link to it (links resolved).
  - Snapshots are handed on per attributed config folder only when they changed.
  - The initial-scan callback opens the attention baseline (2 s grace for account folders to arrive).
  - Folders scanned: ~/.claude + AGENTNOTCH_EXTRA_CONFIG_DIRS + every visible run folder (RegistryDirsBridge).
- Attribution (attribute 387-419): a shared or linked folder asks each pid's CLAUDE_CONFIG_DIR:
  - set → the alias with the same resolved path
  - unset → ~/.claude
  - unreadable → ~/.claude if it leads here, else the first alias

10.2 CLAUDE_CONFIG_DIR from the kernel (E/Services/Session/ProcessConfigDir.swift)
- Same uid only (proc_pidinfo pbi_uid).
- sysctl KERN_PROCARGS2 buffer layout: argc (int32), exec path, NUL padding, argv, then environment.
- Only the entry with the byte prefix `CLAUDE_CONFIG_DIR=` is decoded. Empty value → unset. No environment at all → unreadable, never "unset".
- The buffer is memset_s-zeroed before free.
- Cached per (pid, start time); pruned to the live pids of each scan; cleared above 1024 entries.
- WIN:
  1. OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_VM_READ).
  2. Require the token user SID == ours (OpenProcessToken/GetTokenInformation(TokenUser)), the analog of uid.
  3. NtQueryInformationProcess(ProcessBasicInformation) → PebBaseAddress.
  4. ReadProcessMemory the PEB → ProcessParameters (x64: +0x20) → Environment (+0x80) and EnvironmentSize (+0x3F0).
  5. Read the UTF-16 block. Scan for `CLAUDE_CONFIG_DIR=` case-insensitively (Windows variable names are case-insensitive). Extract only that value, then SecureZeroMemory and free.
  - Undocumented offsets: pin them to x64/ARM64 and test.
  - Fails for elevated targets or other users → unreadable.
  - The hook already sends config_dir_env; the kernel read only matters for registry-only sessions in shared folders.

10.3 ProcessInspector (E/Services/Session/ProcessInspector.swift) and the process table (E/Services/Shared/ProcessTreeBuilder.swift)
- start time (µs), tty (devname of e_tdev), tmux among ancestors (depth 24), and status (tty, foreground = pgid == tpgid, stopped = SSTOP).
- WIN:
  - GetProcessTimes for start time.
  - "tty" → console identity (§8.2).
  - isInTmux → always false natively.
  - foreground/stopped → GetConsoleProcessList rule (§8.2).

10.4 Unresolved Windows specifics
- Whether Claude Code writes the registry on Windows, and what `procStart` looks like there. It is unverified; the engine already falls back to startedAt.
- PIDs are claude.exe (native) or node.exe (npm).
- Path normalisation (AccountPaths.normalize, configDir(fromTranscriptPath:)) must become Windows-aware:
  - both separators
  - drive-letter case
  - case-insensitive comparison
  - GetFinalPathNameByHandleW for links and junctions
  - `\\?\` prefixes
  - transcript_path arrives as `C:\Users\…\.claude\projects\<slug>\<sid>.jsonl`
- AGENTNOTCH_EXTRA_CONFIG_DIRS uses ":" as separator; Windows needs ";".
- Parallel Profiles on Windows (sibling repo ../claude-parallel-accounts-vsc-extension): check whether it uses junctions or copies for `.claude-shared` and `.claude-windows`.

======================================================================
11. WINDOWS: WHAT IS HARD (ranked)
======================================================================

1. Hook command form, and which shell runs it. Git Bash is the default; exec form and `shell` exist in 2.1.282 but their minimum version is unknown. Test on Claude Code with and without Git Bash, with a username containing spaces, and via npm and native installs.
2. Pipe security (squatting, SID verification) and the framing replacement for half-close. Held PermissionRequest connections can last 24 h: size the instance pool, and the server must cope with many concurrent held pipes.
3. Typing replies via AttachConsole/WriteConsoleInput: Bun/Node raw-mode reader behaviour, paste detection, the foreground-reader heuristic via GetConsoleProcessList, elevation, and the helper-process requirement.
4. Windows Terminal exact-tab focus and "looking at it" through UIA with title matching: ambiguous titles, renamed tabs, the pseudo-window owner on older WT, no pane targeting.
5. PEB environment reading with undocumented offsets.
6. Atomic settings.json replacement with sharing violations, and CRLF/BOM preservation in the splicing SettingsDocument.
7. Updating an in-use hook exe (rename dance); AV/SmartScreen on an unsigned exe in `%USERPROFILE%\.claude\hooks`.
8. Toast replace/withdraw/actions need an AUMID registered by the installer.
9. Pid reuse is frequent, so start-time checks are mandatory everywhere (session liveness, the config-dir cache, parent chains).

======================================================================
12. UPSTREAM windows/ AND THIS AREA (what to replace)
======================================================================

W/codenotch-hook/src/main.rs (140 lines) conflicts with fork rules:
- Posts `POST /event?e=<internal>&ppid=<ppid>` with the raw stdin body over TCP 127.0.0.1:<port from %APPDATA%\codenotch\config.json "port", default 48666>.
- About a 2 s budget; waits for a response fragment.
- Spawns codenotch.exe when it is not running. That violates "never launch" and quit semantics.
- ppid via NtQueryInformationProcess.
- It never returns a PermissionRequest decision.

W/codenotch/src/hooks_install.rs (122 lines): unsafe by fork rules.
- Only ~/.claude.
- `load()` turns an unparsable file into `{}`, then overwrites it (lines 38-43).
- Non-atomic `fs::write` with a pretty-printed rewrite of the whole file (45-58).
- Backup `settings.json.codenotch-bak-<ts>` with no rotation.
- Identifies entries by substring "codenotch-hook", "eatbean-hook" or "pacman-hook".
- 7 events (SessionStart, UserPromptSubmit, PreToolUse*, PostToolUse*, Notification, Stop, SessionEnd), timeout 5, command `"<exe>" <internal>`, which does not work under PowerShell.
- No consent gate beyond an explicit menu action (main.rs:1526-1534, 1760-1764).

Other upstream modules:
- W/codenotch/src/server.rs: tiny_http with Origin/Sec-Fetch-Site CSRF checks and a lenient parse into a small HookEvent. Can stay for upstream features, but must not carry permissions.
- W/codenotch/src/focus.rs: ancestor chain up to 8 levels (Toolhelp) and the best-scoring visible top-level window whose pid is on the chain (or whose parent is on the chain: conhost), then SetForegroundWindow and FlashWindowEx. Also proc_maps, fg_pid, chain_of, pid_hits_chain, focus_claude_desktop.
  - Reusable as the "activate host" fallback step.
  - Lacks tab selection, creation-time validation of parents, and multi-window WT handling.
- W/codenotch/src/state.rs, watcher.rs, activity.rs: a simple session map ("running/done/attention") fed by hooks plus transcript watching. It must be replaced by a port of the fork's SessionStore rules (§5).

======================================================================
13. TESTS TO PORT (behavioural oracles)
======================================================================

In Packages/ClaudeControl/Tests/ClaudeControlTests/Engine/:

| Area | Test files |
|---|---|
| Hooks and installer | HookInstallerTests, PP_HooksTests, A2_SettingsDocumentTests, EmbeddedScriptsTests, StatusLineScriptTests, Fix_ScriptSocketOverrideTests |
| Wire format and socket | HookEventDecodingTests, HookSocketIntegrationTests, A1_SocketLifecycleTests, A1_HookPipelineIntegrationTests |
| Session store | SessionStoreFlowTests, SessionCoreRegressionTests, A1_SessionStoreRegressionTests, PP_SessionTests, BackgroundWaitTests, Fix_FailedTurnTests, SessionTaskListTests |
| Attention and review | SessionAttentionTests, Fix_AttentionNewsTests, A1_AttentionAndReviewTests, A1/A2/A3 review suites, PPFix_ReviewTests |
| Transcripts | TranscriptTests, A1_TranscriptParserTests, A1_InterruptWatcherTests |
| Notifications and messaging | A3_NotificationTests, ChatAndNotificationTests, Fix_OffPanelPreviewTests, A3_FocusAndMessagingTests, Fix_MessagingSafetyTests, TerminalFocusTests |
| Desktop-hosted sessions | DesktopHostedSessionsTests |

Also port the dev driver Packages/ClaudeControl/DevTools/simulate-sessions.py. It pipes events through the real hook with CLAUDE_PID, CLAUDE_CONFIG_DIR and AGENTNOTCH_DEV/AGENTNOTCH_SOCKET, and writes fake registries and transcripts. It is the natural end-to-end harness for the Windows engine once it speaks the pipe.

Scratch helpers, not deliverables, used to read the Claude Code bundle: <scratch>/winmap/{ctx.py,find.py,code.py,hookexec.txt}
