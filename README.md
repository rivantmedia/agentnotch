# Superpowered Codenotch

> [!NOTE]
> **Superpowered Codenotch** is a fork of [Codenotch](https://github.com/vinzdg/codenotch)
> by Vinz, used under the MIT License (see [LICENSE](LICENSE), kept as-is), with the
> Claude Code session control of Superpowered Vibe Notch ported into it. This first part
> of the README, down to the line that says so, is the fork's. Everything after that line
> is upstream's README (unchanged, apart from the fork's paragraph under License) and
> describes Codenotch itself: its download, update and "Claude from the OAuth token in the
> keychain" notes do not apply to this fork.

## What's different from Codenotch

- **Every Claude account is a ring.** One ring per signed-in account (its `accountUuid`),
  however many config folders it lives in: `~/.claude`, `~/.claude-<name>` folders you sign
  in to (or add in Settings) and, with the VS Code extension
  [Claude Parallel Profiles](#claude-parallel-profiles), its account stores and every VS Code
  window's working copy. The ring shows the 5-hour session limit inside, the weekly limit
  around it. It is named as you name the account in Settings. Accounts come and go without
  a restart. Hover a ring for its limit windows (Current session, All models, each model)
  and its sessions.
- **Every Claude Code session at a glance.** A yellow badge on a ring counts the sessions
  that need you (a permission, a question, a plan to approve). A green badge counts the
  finished ones waiting for your review. A white arc turns while one is working. A turn that
  stopped on a rate limit or an error shows red, as failed, and can be dismissed. Folded,
  the notch shows the same as a yellow bar, a green dot and a white dot (except beside a
  camera notch, where it stays open while a session needs you).
- **Act from the notch.** Click a Claude ring for the sessions panel. There you can allow,
  always allow or deny a permission, answer a question, approve a plan or keep planning,
  read the conversation and type a reply, and jump to the session's exact terminal tab.
- **No login token, ever.** Claude usage comes from Claude Code itself (asked for its own
  usage), from the live status line, and from Claude Desktop's on-disk cache if you allow it.
  This app reads no keychain item or credential file of Claude's.
- **Installs beside the official app.** It has its own bundle id
  (`com.paraswtf.superpowered-codenotch`), preferences, log subsystem, keychain items and
  `~/Library/Application Support/Superpowered Codenotch`. It never updates itself: Sparkle
  is off, so an upstream build can't replace it. Codenotch's other providers (Codex, Cursor,
  Ollama, …) work as upstream describes.
- **Builds without Xcode**, with the Command Line Tools only.

## Build and run (Command Line Tools only)

You need macOS 15 or later and the Command Line Tools (`xcode-select --install`). They must
include a macOS 26 SDK: `/Library/Developer/CommandLineTools/SDKs/MacOSX26.*.sdk`. The scripts
pick that SDK themselves. With only the macOS 27 SDK the build fails, because SwiftUI's
`@State` macro plugin for it ships with Xcode. The first build fetches the Swift packages,
so it needs the network.

```sh
Scripts/spm-build-app.sh --release      # builds build/Superpowered Codenotch.app (ad-hoc signed)
ditto "build/Superpowered Codenotch.app" "/Applications/Superpowered Codenotch.app"
open "/Applications/Superpowered Codenotch.app"
```

- Without `--release` you get a debug build in the same place.
- `SIGN_IDENTITY="Apple Development: …" Scripts/spm-build-app.sh --release` signs with a
  stable identity. That keeps keychain "Always Allow" grants for the other providers
  across rebuilds (an ad-hoc signature is new on every build).
- To update: quit the app, pull, build again and `ditto` again.
- `Scripts/spm-test.sh` runs the Swift Testing suites (the `ClaudeControl` package, then the
  app's `Tests/ForkSPM`). Upstream's XCTest suite needs Xcode (`make test`).
- `Scripts/spm-run-sealed.sh` opens a sealed copy for a few seconds, with sample data and
  nothing real touched: a safe first look (see [Development](#development)).
- With Xcode installed, upstream's xcodegen project and `make build` / `make test` still work.

## First use

1. **Quit Superpowered Vibe Notch** if you use it. Both apps would keep rewriting the same
   `settings.json`, so this one won't turn on while it runs. Settings offers
   **Quit Superpowered Vibe Notch**.
2. **Open the app.** The notch appears on the right edge of the screen. To move it to
   another edge, use Settings › Appearance. To slide it along the edge, hold ⌥ and drag.
   The first launch opens Settings on **Claude Code**. Later, open Settings from the orb
   under the notch (a gear on hover) or from the menu bar item (**Settings…**).
3. **Check the accounts** in Settings › Claude Code › Accounts. There is one row per
   signed-in account, with where it runs and, with Claude Parallel Profiles, its stores
   (**Show folders** lists each folder and its hooks).
   - `~/.claude` is always found.
   - A `~/.claude-<name>` (or `~/.claude_<name>`) folder is added when it is signed in or
     has a running session. Look-alike folders, such as a backup copy or one with no login,
     are only suggested (**Add** / **Dismiss**).
   - **Add existing folder…** takes any other `CLAUDE_CONFIG_DIR`.
   - **New account…** creates `~/.claude-<name>` and copies its sign-in command. Run that
     command, then `/login`. With Claude Parallel Profiles it first explains that accounts
     are added by signing in inside a VS Code window (the extension saves them, and that
     window switches to the new account and reloads); the folder is the terminal-only
     alternative, and stays yours to run in even after the extension adopts it.
   - An account that runs only in VS Code windows has no **Copy launch command**: a window's
     working copy belongs to that window. Settings says how to use it instead.
   - Switch **Track sessions and hooks** off for any account this app should leave alone.
     **Ring in notch** only shows or hides the ring.
4. **Turn on Claude Code control.** The card at the top of the pane (also shown in the
   sessions panel) lists every `settings.json` it will edit: those of the folders Claude
   Code runs in. With Claude Parallel Profiles that is `~/.claude` and one per VS Code
   workspace (a workspace's folder stays after its window closes; ones opened later are set
   up automatically); its account stores never get hooks.
   - Click **Turn on**. If Superpowered Vibe Notch's hooks are present, the button says
     **Take over and turn on**: it removes those hooks and first puts back the status line
     they wrapped. It also cleans what that app wrote into Claude Parallel Profiles' stores
     and `~/.claude-shared` (its entries, its scripts, and the `settings.json` files it
     created there with their backups), listed on the card, and nothing else there.
   - **Not now** writes nothing. Sessions still show, from Claude Code's own session files,
     but you get no approvals from the notch and no "done" alerts. You can turn it on later
     with **Turn on…** under *Hooks and status line*.
5. **Restart Claude Code sessions that were already running.** Claude Code picks up a
   session's hooks when it starts, so an older session may need a restart before its prompts
   reach the notch. New sessions work at once.
6. **Allow the macOS prompts** as they appear:
   - *Notifications*, on the first banner.
   - *Automation* for iTerm2 or Terminal, the first time you jump to a tab or send a reply
     (System Settings › Privacy & Security › Automation).
   - If a keychain prompt ever asks for Claude Code's credentials on this app's behalf,
     deny it: this app never needs them.

### Upgrading from an earlier build of this fork

- **Replace the old copy, don't keep both.** Launching the new build quits the older
  instance (same bundle id), but an old `build/Superpowered Codenotch.app` launched again
  later would quit the new one and put its own rules back. Quit the old app, then replace or
  delete every old copy (`/Applications`, the fork's `build/`) before opening the new one.
  Until then, set *Check usage every* to **Off** in the old build: before this version the
  usage check could run inside Claude Parallel Profiles' account stores.
- **Your earlier yes carries over.** No consent card appears. On the first launch the hooks
  go into `~/.claude` and every VS Code workspace's folder, and a one-time notice ("Claude
  Code control now covers your VS Code workspaces") names them, with **OK** and
  **Turn off**.
- **Stores are cleaned by themselves.** Hooks an earlier build installed into the stores
  and `~/.claude-shared` come out on the first pass, and (after your yes) Superpowered Vibe
  Notch's leftovers there go too: its scripts, and a `settings.json` that ends up `{}` when
  no backup beside it holds anything of yours, together with those backups.

## Everyday use

- **Rings.** The inner ring is the current 5-hour session and the outer ring is the weekly
  limit. Hover a ring for the limit rows and its sessions. Clicking a session there opens the
  panel when it needs you and its terminal otherwise (Settings › *Clicking a session in the
  hover card*).
- **The sessions panel.** Click a Claude ring, or set a shortcut (⌃⌥Space or ⌥⌘J) in
  Settings › *Panel shortcut*. macOS also uses ⌃⌥Space to switch input sources, so pick
  ⌥⌘J if you type in more than one language.
  - Sessions are grouped under *Needs you*, *Ready for review*, *Working* and *Idle*. Chips
    filter by account.
  - Each row carries its actions: **Allow / Always / Deny**, a question's options (or
    **Other…**), **Review plan / Approve**, **Show terminal**. Click a row for the
    conversation and a reply box.
  - The panel opens by itself when a session needs you, unless you are already in its
    terminal or a full-screen app is in front. Settings › *Open the sessions panel* sets this
    to Never, Needs you, or Needs you or done.
- **Keys in the panel.**

  | Key | Action |
  |---|---|
  | ↑ ↓ | select |
  | ⏎ | open |
  | ⌘⏎ | allow, approve, or mark reviewed |
  | ⌥⌘⏎ | always allow |
  | ⌘⌫ | deny |
  | 1–4 | pick an option |
  | ⌘J | jump to the terminal |
  | ⌘R | mark reviewed |
  | ⌘⇧R | mark all reviewed |
  | Esc | back, then close |

  A bare ⏎ never approves anything.
- **Replies** are typed into the session's terminal for tmux, iTerm2 and Terminal.app. They
  go in only while Claude Code's own prompt is waiting, never into one of its dialogs.
  Dialogs are answered in the panel or in the terminal. For other terminals, use
  **Show terminal**.
- **Jump to the terminal:**
  - iTerm2, Terminal.app and tmux: selects the exact tab or pane.
  - Ghostty: selects the tab with the session's folder. cmux: selects its surface.
  - VS Code, Cursor, Windsurf and VSCodium: brings up the window with the session's
    workspace.
  - Anything else: brings the terminal app to the front.
- **Usage** is checked every 5 minutes, and only when nothing fresher has arrived. In
  Settings › *Usage* you can choose Off, 5, 10, 15 or 30 minutes, use **Refresh now**, and
  see each account's last reading.

## Privacy: what it reads, writes and runs

- **Never:**
  - a Claude login token, keychain item or credential file (`.credentials.json`,
    `sessions/*.key`);
  - a network request of its own for Claude.
- **Reads:**
  - each config folder's `.claude.json` (Claude Parallel Profiles' stores included): only
    the signed-in identity and Claude Code's cached usage figures; the rest of the file is
    skipped over, never parsed;
  - for a session in a shared `sessions/` folder, its process's `CLAUDE_CONFIG_DIR` from the
    kernel (same user only; nothing else of the environment is kept), to tell which account
    runs it;
  - its `sessions/` registry and the transcripts under `projects/`, for titles, tasks,
    context and the conversation;
  - with **Also read Claude Desktop's cached usage** on (the default), the cached `/usage`
    response Claude Desktop keeps for the same organization.
- **Runs `claude`** only for three things:
  - to find it: `command -v claude` in your login shell, once, when it isn't in a usual place;
  - to check its version: `claude --version`;
  - to check usage: `claude -p` in stream-JSON mode, with hooks off, no session saved and an
    empty working folder. It asks only for `get_usage` (what `/usage` shows) and makes no
    model request. Claude Code may update its own files while it runs. It runs once per
    account, in a folder Claude Code runs in as that account (with Claude Parallel Profiles,
    one of its VS Code workspaces' folders, `~/.claude` only when it has none), never in a
    Claude Parallel Profiles store; an account no window runs isn't checked. Who the folder is
    signed in as is checked right before and after; an answer from a folder that changed
    hands meanwhile is thrown away.

  The usage check runs from the first launch, before you turn on control. Set *Check usage
  every* to **Off** to never run it. Readings then come only from the live status line and
  Claude Code's cache.
- **Writes, only after you turn it on:**
  - hook entries and a status line wrapper in the `settings.json` of each folder a tracked
    account runs in (never a Claude Parallel Profiles store or `~/.claude-shared`);
  - in stores and `~/.claude-shared`, only to take out its own or Superpowered Vibe Notch's
    leftovers, with a backup kept until that cleanup is finished;
  - two scripts in `<config folder>/hooks/`: `superpowered-codenotch-hook.py` and
    `superpowered-codenotch-statusline.py`. They run with the developer tools' `python3` and
    talk only to this app's local socket.
  - Before every change it saves a backup, `settings.json.superpowered-codenotch-<time>.bak`,
    and keeps the five newest. The file as it was before the first change is kept as
    `settings.json.superpowered-codenotch.original.bak`.
  - Nothing is written to a `settings.json` that fails to parse.
- **Its own state** lives in `~/Library/Application Support/Superpowered Codenotch/Claude/`:
  accounts, the review queue, usage state and the hook socket.
- **Turning it off.** Switch off *Hooks in tracked accounts* in Settings › Claude Code. That
  takes the hooks and scripts out of every account and restores each status line exactly.
  Untracking or forgetting an account does the same for that account; with Claude Parallel
  Profiles, `~/.claude` keeps them while another account is tracked (the extension copies
  whichever account you last used into it), and an untracked account's sessions there are
  hidden, their permission prompts left to the terminal.
- **Removing the app.** Turn the hooks off first, then quit. Delete the app and
  `~/Library/Application Support/Superpowered Codenotch`, then run
  `defaults delete com.paraswtf.superpowered-codenotch`.

### Claude Parallel Profiles

**Compatibility.** Works alongside the VS Code extension
[Claude Parallel Profiles](https://github.com/rivantmedia/claude-parallel-profiles)
(checked against its 1.4.1 layout), with nothing to set up. The app recognises it by
`~/.claude-windows/.manifest.json` or a store's `.parallel-accounts-store` marker. Without
them, or after the extension is uninstalled, folders are treated as plain `CLAUDE_CONFIG_DIR`
folders. The app never writes the manifest, a marker, a `.claude.json`, a credential or the
shared history. It never runs `claude` in a store and never reads Claude Code's Keychain
items, so the extension's switching, mirroring into `~/.claude` and uninstall work as before.

The VS Code extension Claude Parallel Profiles keeps each account in a store it creates
(`~/.claude-<name>`, marked `.parallel-accounts-store` and listed under `created` in
`~/.claude-windows/.manifest.json`), runs every VS Code window on a working copy
(`~/.claude-windows/<id>`), mirrors the last-used account into `~/.claude`, and links one
shared history (`~/.claude-shared`) into all of them. A `~/.claude-<name>` profile of your
own that it adopted (listed under `stores` only) stays a folder you run Claude Code in: it
keeps its hooks and usage check. This app follows that layout:

- **One ring per account.** Folders are grouped by who is signed in, so two accounts are two
  rings however many stores and windows there are. `~/.claude-shared` is never an account.
  When the extension mirrors another account into `~/.claude`, new terminal sessions there
  go to that account's ring; a session already running stays with the account it started
  as (and its live rate limits count for that account). One the app can't place is shown on
  the current ring, and its rate limits are left out.
- **Hooks where Claude Code runs:** `~/.claude` and every VS Code workspace's folder. A
  workspace opened later gets them within seconds. Stores are read (so an account with no
  open window still has its ring and usage) but never hooked or used for the usage check,
  and written only to take leftovers out.
- **Sessions:** the shared `sessions/` folder is read once, and each session goes to the
  account its process runs as.
- **Names, switches, order, muted alerts and the menu-bar choice** you gave the rings of
  earlier versions (one per folder: `claude`, `claude-paras`, …) move to the account's ring
  once; rings like `claude-shared` disappear. `~/.claude`'s old ring goes to the account its
  own `accountUuid` names, not to whichever account the extension mirrored in last.
- To see what the app makes of your folders without running it:
  `swift run --package-path Packages/ClaudeControl spcn-inspect-accounts` (read-only).
- **Logs:** `log stream --predicate 'subsystem == "com.paraswtf.superpowered-codenotch"' --level debug`.

## Development

- **Sealed mode.** `SPCN_SAFE_MODE=1` (or `CODENOTCH_DEMO=1`) starts the app sealed: fixture
  data, and no keychain, session, network or subprocess access. It fails closed: any value
  except empty, `0`, `false`, `no` or `off` seals the run. `Scripts/spm-run-sealed.sh`
  builds a separate sealed bundle (`….sealed` bundle id), runs it for at most 10 seconds and
  reports what it touched:
  - `--edge right|left|top|bottom` sets the edge;
  - `--open-panel sessions` opens the panel and checks where it sits;
  - `--panel-self-test` drives the panel's keys and clicks;
  - `--snapshot-claude <dir>` renders the notch and panel sheets to PNGs.
- **Snapshots.** `Scripts/spm-snapshots.sh <dir>` renders the panel, chat and settings sheets
  of `Packages/ClaudeControl` from fixtures.
- **Development switches** (`1`, `true` or `yes`): `--no-install` / `SPCN_NO_INSTALL`,
  `SPCN_NO_NOTIFICATIONS`, `SPCN_USAGE_PROBE`, `--dump-state` / `SPCN_DUMP_STATE` and
  `--dev-console` / `SPCN_DEV_CONSOLE`. Path switches: `SPCN_SUPPORT_DIR`, `SPCN_SOCKET`
  and `SPCN_EXTRA_CONFIG_DIRS` (`:`-separated). See `Engine/Core/DevFlags.swift`. The
  installed hook scripts follow `SPCN_SOCKET` only when `SPCN_DEV=1` is set too.
- **Sealed-only switches** (ignored in a live run): `SPCN_OPEN_PANEL_ON_LAUNCH=<route>`,
  `SPCN_PANEL_CLOSE_AFTER=<seconds>`, `SPCN_PANEL_SELF_TEST=1`,
  `SPCN_SEALED_SWITCH_OFF=<ring id>`, `SPCN_SEALED_CAPTURE=<dir>` (timeline PNGs) and
  `SPCN_SNAPSHOT_CLAUDE=<dir>` (render, then exit). See `Scripts/spm-run-sealed.sh` and
  `Sources/ClaudeBridge/ClaudeSealedDemo.swift`.
- **Simulated sessions.** `Packages/ClaudeControl/DevTools/simulate-sessions.py` drives a
  running build with fake Claude Code sessions over a private socket (`SPCN_SOCKET`,
  `SPCN_SUPPORT_DIR`, `SPCN_EXTRA_CONFIG_DIRS`; see its header). It never touches a real
  Claude config.
- **Checks.**
  - `Scripts/check-seams.sh`: every edit to upstream's files is a listed, tagged seam.
  - `Scripts/verify-token-free.sh`: no Claude path can read a token.

  Both compare against `upstream/main` (`git remote add upstream
  https://github.com/vinzdg/codenotch`) or `UPSTREAM_REF=<commit>`.
  `Packages/ClaudeControl/Scripts/embed-scripts.sh --check` confirms the embedded hook
  scripts match the `.py` files.
- **Licences.** See [License](#license) at the end: Codenotch is MIT; `Packages/ClaudeControl` is Apache-2.0, derived from Superpowered Vibe Notch and Vibe Notch.

*The rest of this README is upstream Codenotch's.*

---

<div align="center">

![Codenotch](docs/design/codenotch-banner.png)

[![CI](https://github.com/vinzdg/codenotch/actions/workflows/ci.yml/badge.svg)](https://github.com/vinzdg/codenotch/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%2015%2B-black)
![Swift](https://img.shields.io/badge/swift-5-orange)
![License](https://img.shields.io/badge/license-MIT-green)

**A macOS app that pins a small black notch to a screen edge, showing how much
of each coding assistant's usage limit you have burned — and whether it is
still working, done, or waiting on you.**

![Collapsed notch with hover tooltip](docs/design/frame-124-hover-tooltip.png)

</div>

Hover a ring for its limit windows and when they reset. Claude's ring shows the
same **current session** window Claude Code's own `/usage` leads with, so the
two never disagree.

## Download

[![Download for macOS](docs/design/download-macos.svg)](../../releases/latest/download/Codenotch.dmg)

That button is the disk image itself, not the page it sits on — the asset is
named `Codenotch.dmg` in every release, so `releases/latest/download/` always
resolves to the newest one and the link never needs updating. Signed,
notarized, and updating itself from then on. Take this one unless you have a
reason not to; the [release page](../../releases/latest) has the notes.

To try unreleased `main` without an Xcode install, the [preview
build](../../releases/tag/preview) is rebuilt from every commit, and the
Package workflow keeps a per-commit disk image on each of its
[runs](../../actions/workflows/package.yml). Neither is notarized — they are
ad-hoc signed, because the Developer ID certificate exists on one machine — so
macOS quarantines the download. Clear the flag once, after dragging the app to
Applications:

```sh
xattr -dr com.apple.quarantine /Applications/Codenotch.app
```

If macOS says the app is *damaged*, that is the quarantine flag rather than a bad download — run the command above.

Universal binary. macOS 15 or later. To build and install a copy from source
instead, see [Building](#building).

## Windows

[![Download for Windows](docs/design/download-windows.svg)](../../releases/latest/download/Codenotch-Setup.exe)

A Windows port — Rust/Tauri 2, same design and providers — lives in [`windows/`](windows/README.md).
The button is the installer itself, named `Codenotch-Setup.exe` in every release for the same
reason the dmg keeps one name. It installs for the current user without administrator rights,
and fetches WebView2 if Windows does not already have it.

The installer is not code-signed, so the first time it runs SmartScreen says *Windows protected
your PC*. Choose **More info**, then **Run anyway**. Every Windows change also leaves an
installer on its [Windows Package run](../../actions/workflows/windows-package.yml).

## Connect your phone

The Codenotch phone app (iOS and Android) can show the same usage
percentages, reset times and session states as the notch on your Mac.
It reads only what the notch already displays — never tokens, credentials
or raw API responses.

To pair, open **Settings › Phone › Connect a Phone…** (or the menu item)
on your Mac. A QR code appears with a five-minute countdown; scan it with
the Codenotch phone app, or copy the link and paste it into the app. The
Mac and phone must be on the same Wi-Fi network — the server answers only
local-network addresses and rejects anything routed over the internet.

Each code is single-use and expires after five minutes. Reopening the
window always mints a fresh one.

To remove a paired phone, open **Settings › Phone**, find the device in
the list and click **Remove**. Its credentials are deleted immediately and
any subsequent request from that phone is rejected.

See [docs/phone-link-protocol.md](docs/phone-link-protocol.md) for the
wire-level details.

## What it reads

| Provider | Source | How |
|---|---|---|
| **Claude Code** | official | Claude Desktop's own cached usage response, where Desktop is running and signed into the same account. Then Claude Code's own `/usage`, asked of the installed `claude`. Then the OAuth token in the login keychain, against the endpoint that command uses. |
| **Cursor** | official | The editor's signed-in session in its local SQLite state, or the `cursor-agent` login in the keychain — no separate sign-in. |
| **Codex** | official | Using the local Codex sign-in. Shows the 5-hour and weekly limits when available, plus extra limit windows when the account has them. |
| **DeepSeek Platform** | derived from official Platform responses | Explicit sign-in in Codenotch's own WKWebView, then the Platform account summary and API-key/model usage endpoints. Shows funded/spent balance, 30-day tokens/cost, requests and API-key count. |
| **Antigravity** | official where licensed, otherwise a request count | Antigravity's local language server first, then Google's quota endpoint; a plain count when neither will answer for the account. |
| **GLM** | official | Z.ai's Coding Plan monitor endpoint, with a key borrowed from whichever coding tool already holds one — Claude Code's `settings.json`, ZCode, or OpenCode. |
| **MiniMax** | official where a Coding Plan key is used, derived from official Platform responses for the in-app sign-in | A Coding Plan key pasted in Settings, or explicit sign-in in Codenotch's own WKWebView. |
| **QianwenAI** | derived from official console responses | Explicit sign-in in Codenotch's own WKWebView, then the console's own Token Plan gateway. Shows the plan's credits window for whichever period the console reports — weekly or monthly. |
| **Ollama (Local)** | local runtime | Automatically detected local models, RAM/VRAM, unload time and context. Optional response capture adds thinking and generation speed. |
| **LM Studio** | local runtime | Loaded models from LM Studio's own listing, what each one is doing (prompt, generating, queue) from its SDK socket, and speed, context use and tokens per day from its server log. No relay needed. |
| **Grok** | official | The Grok CLI session in `~/.grok/auth.json`, against the same credits billing endpoint `/usage` uses. |
| **OpenCode** | official | The Go plan's official usage endpoint, with the `opencode-go` key OpenCode itself stores on sign-in. |
| **Command Code** | official | The GOAT plan's `/alpha` billing endpoints, with the key the Command Code app writes to `~/.commandcode/auth.json`. |
| **GitHub Copilot** | official | GitHub's Copilot quota endpoint, authenticated with the GitHub CLI session already on the Mac (`gh auth login`). |
| **Kimi** | official | The Kimi Code CLI session in `~/.kimi-code/credentials/kimi-code.json`, against the same `/usages` endpoint the CLI's `/usage` asks. Shows the 5-hour rate window and the weekly quota. |
| **Kiro** | official | The kiro-cli session already on this Mac, against the same `/usage` that command prints. Shows monthly credits. |
| **Amp** | official subscription percentages; derived free-allowance percentage | The Amp CLI login in `~/.local/share/amp/secrets.json`, against Amp's `userDisplayBalanceInfo` endpoint. Shows Agent and Orb usage, or the Free allowance and replenishment rate. See [Amp details](docs/providers/amp.md). |

Most providers borrow a credential or session from a tool already on your Mac.
DeepSeek is the explicit browser-login exception: it never reads a browser's
cookies or credentials, and only makes requests after you choose **Sign in to
DeepSeek** from Codenotch. MiniMax is the same kind of exception — a key you
paste in Settings, or an explicit WKWebView sign-in. QianwenAI is a third: it
publishes no usage API and has no key to paste, so that WKWebView session is the
only way in. None of them opens a browser's cookie store.

Ollama Cloud accepts an API key in Settings. Switching a provider off stops its
usage polling and forgets its readings; borrowed accounts stay signed in to
the tools that own them.

**Local Ollama is detected automatically.** Configure its address or stop monitoring in **Settings → Ollama**.
Each loaded model gets a notch cell; reorder or hide it in **Settings → Accounts**.
Hover for RAM/VRAM, unload time, context limit and quantization.

For generation speed (**tok/s**) and live **Thinking**, enable **Measure speed and thinking**
in Settings → Ollama, keep Codenotch open and connect through its local relay:

```sh
OLLAMA_HOST=http://127.0.0.1:11435 ollama run gemma4:e4b --think
```

Speed updates after completed native Ollama responses; thinking requires streamed
reasoning. Direct requests to Ollama's default port (`11434`) only provide model
detection. Monitoring never initiates inference or saves prompts, reasoning or replies.
See [Ollama details](docs/plans/2026-09-07-local-llm-provider-plan.md).

**Local LM Studio is detected automatically** on the port LM Studio's own settings name
(1234 unless you moved it). Configure the address or stop monitoring in **Settings → LM Studio**.
Each loaded language model gets a notch cell; embedding models are left out. The cell shows the
last response's **tok/s** and its ring fills with how much of the loaded **context** the last
request used. A white arc turns while the model reads a prompt or generates, and becomes a ring
of dots when requests are queued behind it. Hover for context used, tokens and requests today,
reasoning share, speculative-decoding acceptance, model size, quantization and context limit.

Nothing has to be pointed at Codenotch: what a model is doing comes from LM Studio's SDK socket
on the same port (the one `lms ps` uses), and speed and tokens come from `~/.lmstudio/server-logs`,
which LM Studio writes for every request from any client. Only counts and timings are read from
those files, never a prompt or a reply. Responses through the OpenAI-compatible endpoint carry no
clock, so their speed is timed from the generating phase and marked `~`. If LM Studio's server is
set to require an API token, paste one in Settings → LM Studio (or export `LM_API_TOKEN`); without
one, requests are sent with no Authorization header at all.
See [LM Studio details](docs/plans/2026-09-10-lm-studio-provider-plan.md).

Settings lists the connected providers in the order the notch draws them, and
you can drag one by its handle to move it. The order is remembered across
launches. A provider you switch back on joins the end of that list rather than
reclaiming an older position, so nothing you cannot currently see jumps ahead
of something you placed deliberately.

It also answers **"is it still working?"** — a thin arc spins inside a
provider's ring while a session is busy, and becomes a pulsing amber ring when
one is blocked waiting on you. Hover for every live session by name, where it
is running, and what it wants.

Two Claude Code logins are two rings. Anyone who keeps a work account apart with
`CLAUDE_CONFIG_DIR=~/.claude-work claude` gets a **Claude (work)** ring beside the
personal one, with its own limits, its own sessions and its own row in Settings.
Any `~/.claude-<slug>` directory Claude Code has run against is found at launch;
the default `~/.claude` always comes first, the rest in alphabetical order, so the
rings never swap places.

Codex accounts work the same way: `~/.codex` stays the **Codex** ring, and each
used `~/.codex-<slug>` directory adds a **Codex (slug)** ring with its own limits,
activity and Settings row. Profiles are discovered at launch, default first,
then alphabetically. To connect a second account, sign in through Codex CLI
using a separate home directory:

```sh
mkdir -p "$HOME/.codex-work"
CODEX_HOME="$HOME/.codex-work" codex -c 'cli_auth_credentials_store="file"' login
```

Choose the second account during sign-in, then restart Codenotch. Run that
account's CLI sessions with `CODEX_HOME="$HOME/.codex-work" codex` as well.
Repeat with another name, such as `.codex-personal`, for more accounts.
Settings shows each account's email and profile directory; each ring can be
reordered or switched off independently. Switching one off forgets only its
Codenotch readings and leaves the Codex login intact.

Codenotch reads each profile's `auth.json`; keychain-only or API-key-only
logins cannot provide these ChatGPT account limits. It never copies, refreshes
or writes Codex credentials. If a login expires, use that profile's Codex CLI
to renew it. Directories outside the `~/.codex-<slug>` convention are not
discovered automatically, and adding a profile requires restarting Codenotch,
just as it does for Claude.

## When a session ends

The notch opens itself for five seconds when an agent stops working, or stops
to ask you something, and sounds the system alert. Clicking it while it is open
brings that session's application to the front.

The app, not the tab. A session publishes its pid and nothing else — no window,
no tab, no tty — so the app is found by walking up the process tree from the
agent to whatever launched it. Choosing the *tab* inside that app needs the
terminal's own scripting interface, and there is no general one: Terminal.app
and iTerm2 can match a tab by tty, Warp and Ghostty publish no scripting
dictionary at all. So the app is raised for everybody and the tooltip names the
session, which leaves the last hop one keystroke rather than working for two
terminals and silently doing nothing in a third.

Both halves switch off separately in Settings, because they fail differently:
the peek is no use behind a full-screen window, and the sound is no use in a
meeting. Each of the two events — finished, and waiting on you — picks its own
sound there, with a preview button beside it.

The sound is played as a file on the ordinary output rather than handed to
`NSSound` as a system alert. A system alert goes through the interface
sound-effects channel, which System Settings → Sound can switch off — and on a
Mac where it is off, `NSSound.play()` reports success and nothing is heard.

Only *leaving* busy counts. A question being answered is not a piece of work
ending, and a session whose file disappears mid-turn — which is what quitting
Claude Code looks like — is not announced at all, since there is no window left
to jump to. Nothing is announced from the first reading either: every session
already running at launch arrives with no history, and treating that as a
transition would ring once per open window on every start.

## Alerts

A provider's headline limit crossing **80%** — and reaching **100%** —
becomes a system notification: once per crossing, never repeated while it
stays crossed, and again only after the window has genuinely rolled over.
Each provider can be muted from its own row in Settings, and macOS permission
is asked on the first real alert rather than at launch.

## Placement

The notch lives on any of the four screen edges. Right and left keep a
vertical column; top and bottom lay the readings out side by side. It pins
itself to the physical screen edge, so showing or hiding the Dock does not
move it. Hold Option and drag to move along the selected edge; each edge
remembers its position. On a Mac with a hardware notch, the top
placement takes its exact shape, so the two read as one rather than as a bar
parked underneath it.

Along that edge it sits wherever you put it: hold ⌥ and drag the notch to
slide it, and each edge remembers where you left it, so moving the notch to the
top and back does not lose the place you chose on the right. **Recentre** in
Settings → Appearance puts the current edge back in the middle.

**Size** in the same place draws the whole notch — rings, text, tooltip and all
— smaller or larger. Medium is the size it was designed at.

At rest it is a small pill on the screen edge that unfolds when the pointer
reaches it — configurable in Settings to always show, or to hide entirely.
Settings live in an orb below the notch: an arc at rest, a gear on hover.

Clicking the notch while it is open keeps it open, so it stays put while you
read it; clicking it again lets it fold away as usual. That click has to land
on the body itself, since a ring takes its own click to refetch that provider
and the orb takes one to open Settings. Right-clicking offers the same thing as
a menu item, **Keep open**, ticked while the notch is being held open, which is
the surer way to release one that was kept open by accident. The item is
greyed out when Settings says Always show, because that choice is Settings' to
change.

In Settings → Appearance → Reset time, choose **Time remaining** for countdowns
like "Resets in 3 Days 3h". **Reset date** keeps the reset date and time, with
minutes shown when less than an hour remains.

Appearance also carries the ring's accent colour. The device accent is the
default; fixed presets are available for pink, red, orange, yellow, green,
teal, blue, indigo, purple and off-white.

The app itself can show a Dock icon, a menu bar item, or neither. The menu bar
item is the Codenotch icon until you switch on **Show limit information in
menu bar** under Settings → Appearance → App; then it shows the five-hour
limits of the providers you choose there — the provider's mark, the share used
and the time until it resets, like `72% · 2h 18m | 41% · 4h 05m`. Choosing
what the bar shows never changes what Codenotch reads, and with nothing chosen
the icon comes back. Its menu has the full readings either way.

## Updates

Codenotch updates itself. [Sparkle](https://sparkle-project.org) checks daily
and installs in the background without prompting; Settings says so and can
switch it off. Every update is EdDSA-signed, so nothing installs that wasn't
built and signed by the maintainer.

## Building

```sh
brew install xcodegen   # once
make run                # generate, build, launch a Debug build
make test               # unit tests
```

No signing identity is required for either. `make release` — which archives,
notarizes, and produces a signed auto-update feed — needs a Developer ID
certificate and an App Store Connect notary profile, and is only ever run by
the maintainer to cut an official release. See
[CONTRIBUTING.md](CONTRIBUTING.md). CI runs the same unit tests unsigned via
`make test-ci`.

A Debug build is ad-hoc signed, which means it has no stable code identity, so
macOS cannot match it to a saved keychain "Always Allow" — the prompt to read a
tool's token returns on every launch. To make the grant stick during local
development, sign the built app with a stable self-signed identity:

```sh
Scripts/sign-local.sh   # signs /Applications/Codenotch.app (pass a path to override)
```

It creates a reusable `Codenotch Local Signing` certificate in your login
keychain (no Apple Developer account needed) and re-signs the app. Grant the
keychain prompt once more after signing; it will not ask again.

Run with `CODENOTCH_DEMO=1` to see fixed sample data instead of live readings.

## Architecture

Every provider implements `UsageProvider` (`Sources/Providers/`) and declares
its own `Fidelity` — `.official`, `.derived`, or `.manual` — so the UI never
presents a guess as if a vendor had published it. `UsageStore`
(`Sources/Model/`) polls them on a timer, keeps the last good reading across
launches, and degrades every failure to a visible status rather than a
made-up percentage.

The notch itself works in one-dimensional **stack space** (`along`/`across`)
regardless of which screen edge it's on; `NotchPlacement` is the only place
that maps that back onto real screen coordinates. `NotchLayout` holds every
measurement, quoted from `docs/design/frame-124-hover-tooltip.png` so the
layout can be checked against the design frame directly.

- Design spec: [`docs/specs/2026-08-28-usage-notch-design.md`](docs/specs/2026-08-28-usage-notch-design.md)
- Implementation history: [`TASKS.md`](TASKS.md)

## The honest caveat

No vendor publishes a clean "your session limit is N% used" API for any of
these tools. Each adapter reads whatever the owning app itself reads from —
an internal endpoint, a local database, a language server's own RPC — and
those can change without notice. Every adapter's response shape is pinned by
tests, and every failure degrades to a visible status (`stale`, `needsAuth`,
`error`) rather than an invented number.

**Claude Desktop's cache:** Claude Desktop is a Chromium app, so the usage
response its own panel draws is written to an HTTP cache file under
`~/Library/Application Support/Claude`. Reading it is how the ring stays right
for people who work in Desktop rather than in the terminal — the two Claude
Code paths below both go dark when `claude "/usage"` stops printing the windows
and the keychain token has not been re-minted since Claude Code last ran, which
is an ordinary state for a Desktop user. It is strictly read-only, and narrow:
only entries whose cached URL is *this account's* `/api/organizations/<id>/usage`
are opened at all, matched on the organization Claude Code records for the
profile, so one account's numbers can never land on another's ring. No token, no
cookie, no credential and no request to Anthropic are involved. A snapshot older
than 30 minutes is not shown as live — it drops through to the paths below, and
the last good reading ages and dims as any other would. Chromium's cache format
is private and may change; if it does, the source goes quiet and the existing
ones take over. Bodies are `content-encoding: zstd` and macOS ships no decoder,
so a decode-only build of Zstandard is vendored under
[`Sources/Vendor/zstd`](Sources/Vendor/zstd) (BSD-3-Clause).

**Claude's unused resets (macOS):** the hover card shows the remaining resets
and their expiry, using the same section as Codex. Open **Settings → Usage**
in Claude Desktop for the same account to populate its reset data. That data
is read from Desktop's usage cache and is labeled as cached with the time it
was last observed. Ordinary usage refreshes do not re-date it; old usage
windows still fall back to the CLI/OAuth sources after 30 minutes. Used, paused, future,
and expired grants are hidden. There is no built-in promotion date or assumed
entitlement. As checked on September 23, 2026, the OAuth usage endpoint does
not expose the grants (`ineligible_reason: surface`), so a CLI/OAuth-only
setup cannot show them yet. Codenotch displays availability only; redeem a
reset in Claude. See [the provider notes](docs/providers/claude-resets.md).

**Keychain:** Claude's readings do not use it where Claude Code is installed.
Claude Code files a *new* keychain item on every token rotation, and the new
item's access list does not carry this app, so an "Always Allow" granted
against the old one stops working about an hour later — asking `claude` itself
avoids the question entirely. Where the keychain is still the source (no
Claude Code on the machine, or Antigravity), the app is signed with a stable
Developer ID identity so a grant survives rebuilds, and the secret is read
only when the owning app has actually changed it — checked via the item's
modification date, which isn't behind the same access prompt as the
credential — so a valid grant does not mean a prompt on every poll.

**Rate limits:** Claude's endpoint returns 429 if polled too hard, with an
unhelpful `Retry-After: 0`. The back-off treats that as a floor-raiser only —
60s, doubling per consecutive 429, capped at 15 minutes — and the deadline is
persisted, so relaunching during a penalty waits instead of spending an
attempt on it. Polling drops to every 5 minutes when nothing is running, and
right-clicking the notch offers **Refresh now**.

**Logs:** the app has no window, so anything worth diagnosing goes to the
unified log.

```sh
/usr/bin/log stream --predicate 'subsystem == "com.vinz.codenotch"' --level debug
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © 2026 Vinz

**Superpowered Codenotch** is a fork of Codenotch (MIT, © 2026 Vinz).
`Packages/ClaudeControl` is Apache-2.0, derived from Superpowered Vibe Notch and
Vibe Notch (© 2025 Farouq Aldori); see its [LICENSE](Packages/ClaudeControl/LICENSE)
and [NOTICE](Packages/ClaudeControl/NOTICE). It links swift-markdown (Apache-2.0)
and swift-cmark (BSD-2-Clause). The app bundle carries every one of these licences
and notices in `Contents/Resources`.
