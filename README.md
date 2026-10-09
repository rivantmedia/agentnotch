# Agent Notch

> [!NOTE]
> **Agent Notch** (formerly Superpowered Codenotch) is a fork of
> [Codenotch](https://github.com/vinzdg/codenotch) by Vinz, used under the MIT License (see
> [LICENSE](LICENSE), kept as-is), with the Claude Code session control of Superpowered Vibe
> Notch ported into it. This first part
> of the README, down to the line that says so, is the fork's. Everything after that line
> is upstream's README (unchanged, apart from the fork's paragraph under License) and
> describes Codenotch itself: its download, update and "Claude from the OAuth token in the
> keychain" notes do not apply to this fork, and its download buttons don't work here. This
> fork's own are [Download](#download) and [Updates](#updates).

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
  This app reads no keychain item or credential file of Claude's. The only sign-in it keeps
  is its own, to its website, if you turn on cloud sync: a 0600 file in its support folder,
  nothing to do with your Claude login.
- **Optional cloud sync.** Sign in with Google to the Agent Notch website (`web/`; its
  address is built into the app) to see every account's sessions, tokens, cost and limits
  from all your Macs in one place, and to share an account's history with the other people
  who use it. Off until you sign in and turn it on; see [Cloud sync](#cloud-sync-optional).
- **Installs beside the official app.** It has its own bundle id
  (`com.rivantmedia.agentnotch`), preferences, log subsystem, keychain items and
  `~/Library/Application Support/Agent Notch`. Codenotch's other providers (Codex, Cursor,
  Ollama, …) work as upstream describes.
- **Updates itself from this fork's own releases.** A downloaded copy checks this
  repository's GitHub releases once a day and installs updates signed with the fork's own
  key. It never reads upstream's feed, so an official Codenotch build can't replace it.
  Copies built from source never update themselves. See [Updates](#updates).
- **Builds without Xcode**, with the Command Line Tools only.

## Download

Download the disk image, `AgentNotch-<version>.dmg`, from the
[latest release](https://github.com/rivantmedia/agentnotch/releases/latest). An Agent Notch
website (`web/`) offers the same file on its **Download** page (`<website>/download`). It
runs on macOS 15 or later, on Apple silicon and Intel (one universal app).

1. **Open the disk image and drag Agent Notch to Applications**, then open it from there.
   It can't update itself while it runs from the disk image or from Downloads.
2. **Let macOS open it the first time.** Releases are signed without an Apple Developer ID
   and aren't notarized, so macOS refuses the first open. Open System Settings › Privacy &
   Security, scroll down and click **Open Anyway**. If macOS says the app is *damaged*, that
   is the quarantine flag on the download rather than a bad file. Clear it once in Terminal:

   ```sh
   xattr -dr com.apple.quarantine "/Applications/Agent Notch.app"
   ```

   If a release is ever notarized, its release notes leave this step out.
3. Go on with [First use](#first-use).

On Windows (a preview), see [Windows (preview)](#windows-preview).

From then on it [updates itself](#updates). A copy built from source, which every copy from
before 1.0.0 is, never does: replace it by hand once, as [Updates](#updates) describes.

## Build and run (Command Line Tools only)

To build it from source instead of downloading it, you need macOS 15 or later and the
Command Line Tools (`xcode-select --install`). They must include a macOS 26 SDK:
`/Library/Developer/CommandLineTools/SDKs/MacOSX26.*.sdk`. The scripts pick that SDK
themselves. With only the macOS 27 SDK the build fails, because SwiftUI's `@State` macro
plugin for it ships with Xcode. The first build fetches the Swift packages, so it needs the
network.

```sh
Scripts/spm-build-app.sh --release      # builds build/Agent Notch.app (ad-hoc signed)
ditto "build/Agent Notch.app" "/Applications/Agent Notch.app"
open "/Applications/Agent Notch.app"
```

- Without `--release` you get a debug build in the same place. The version is the one in
  the repository's `VERSION` file.
- `SIGN_IDENTITY="Apple Development: …" Scripts/spm-build-app.sh --release` signs with a
  stable identity. That keeps keychain "Always Allow" grants for the other providers
  across rebuilds (an ad-hoc signature is new on every build).
- **A copy built from source never updates itself:** it carries no update feed. To update
  it, quit the app, pull, build again and `ditto` again. Copying it over a downloaded
  release in `/Applications` stops that copy's updates too; install a release over it to
  get them back.
- `Scripts/spm-test.sh` runs the Swift Testing suites (the `ClaudeControl` package, then the
  app's `Tests/ForkSPM`). Upstream's XCTest suite needs Xcode (`make test`).
- `Scripts/spm-run-sealed.sh` opens a sealed copy for a few seconds, with sample data and
  nothing real touched: a safe first look (see [Development](#development)).
- With Xcode installed, upstream's xcodegen project and `make build` / `make test` still work.
- **The website's address** for [cloud sync](#cloud-sync-optional) is `websiteURL` in the
  repository's `app-config.json` (the Release workflow's website job reads the same file).
  `Scripts/spm-build-app.sh` writes it into the app's `Info.plist` as `AgentNotchWebsiteURL`;
  the app shows it and can't change it. The build stops when the file is missing, isn't JSON,
  or names an address the app wouldn't take exactly as written: `https://host[:port][/path]`
  with no trailing slash, query or `user:password`, or `http://` to `localhost`, `127.0.0.1`
  or `[::1]`. An empty string (`""`) builds a copy with no website, which can't sign in or
  sync; so does upstream's Xcode path, which doesn't write the key.

## First use

1. **Quit Superpowered Vibe Notch** if you use it. Both apps would keep rewriting the same
   `settings.json`, so this one won't turn on while it runs. Settings offers
   **Quit Superpowered Vibe Notch**.
2. **Open the app** (from `/Applications`; a download needs the first-open step in
   [Download](#download)). The notch appears on the right edge of the screen. To move it to
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

### Updates

- **A downloaded copy updates itself.** About once a day it reads this repository's feed,
  `https://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml`. A newer
  version downloads in the background and installs when Agent Notch quits, so it runs from
  the next start. Every update is signed with the fork's own EdDSA key and checked before
  it is unpacked; one that doesn't match is never installed. Upstream's feed is never read.
- **Settings › General.** **Check now** checks at once. Switch off **Install updates
  automatically** to stop the daily check and the background downloads; **Check now** still
  works. The notes for each version are on its
  [release page](https://github.com/rivantmedia/agentnotch/releases); the app shows no
  What's New window.
- **If macOS asks again after an update** for Automation (to reach your terminal) or for a
  keychain item of another provider, allow it again. A release signed ad hoc has a new
  signature each time, and macOS ties those grants to the signature.
- **Copies built from source never update themselves**, and neither do sealed or
  development copies: they carry no feed. There the switch is greyed out, and **Check now**
  says where to download a release.
- **Copies from before 1.0.0 need replacing once, by hand.** Every copy made before the
  first release was built from source and has no feed. It shows upstream's version number
  (such as 1.18.0); Agent Notch numbers its own releases from 1.0.0. Quit it, delete every
  old copy (`/Applications`, the fork's `build/`), then install a release as in
  [Download](#download). Preferences, accounts and the review queue carry over: the bundle
  id and support folder are the same.
- **Keep one copy.** Launching a copy quits the one already running (same bundle id), but
  an old copy launched again later would quit the new one and put its own rules back. Delete
  old copies rather than keeping them beside the new one.

### Coming from Superpowered Codenotch

Agent Notch is the same app under a new name, with a new bundle id
(`com.rivantmedia.agentnotch`), so macOS treats it as a different app.

- **Quit Superpowered Codenotch and delete it first.** Both would rewrite the same
  `settings.json` files. Leave its hooks where they are: they keep failing open while the app
  is gone.
- **Turn on again.** Its yes, preferences, account names, ring choices and review queue lived
  under the old bundle id and `~/Library/Application Support/Superpowered Codenotch`, so the
  consent card appears again. **Turn on** replaces the old hook entries and status line wrapper
  in place (the status line they wrapped is kept) and deletes the old scripts from each
  `hooks/` folder once nothing runs them. Its `settings.json.superpowered-codenotch-*.bak`
  backups are left alone; they are still read if a saved status line is ever lost.
- **Tidy up afterwards** if you like: `~/Library/Application Support/Superpowered Codenotch`
  and `defaults delete com.paraswtf.superpowered-codenotch`.

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
- **Usage limits are announced once per account.** When an account's 5-hour or weekly limit
  runs out you get one alert, saying when it resets: the account's banner ("Work: 2 sessions
  hit the limit") or Codenotch's "limit reached" card, whichever comes first, with one chime.
  Until that window resets nothing repeats it: not a retry, wake-up or `/loop` tick that fails
  on the limit, another session stopped by it, a session reopened with its old failure, or a
  relaunch. The 80% warning still comes once per window.

## Cloud sync (optional)

The website in [`web/`](web/README.md) (Next.js, tRPC, Prisma, Supabase; you host it) keeps a
history of what your Claude accounts were used for: each account's 5-hour and weekly limits
over time, the projects it worked in, and the tokens and cost of every Claude Code session,
from every Mac that syncs. Nothing leaves this Mac until you sign in and turn sync on.

- **Signing in.** In Settings › Claude Code › *Cloud*, click **Sign in with Google**. There
  is no address to enter: the website is the one the app was built with (`app-config.json`,
  see [Build and run](#build-and-run-command-line-tools-only)), shown read-only above the
  button (*None*, with **Sign in** unavailable, in a build that has none). Google's sign-in
  opens in your browser and comes back to the app (`agentnotch://auth-callback`, PKCE through
  the website's Supabase project). The app keeps that sign-in (a Supabase access token and
  refresh token) in `~/Library/Application Support/Agent Notch/Claude/cloud-session.json`, a
  file only your user can read (0600). It isn't in the Keychain and has nothing to do with
  Claude's login. **Sign out…** ends this Mac's sign-in; what the website has stays there; a
  sign-in still open in the browser when you sign out is thrown away (and ended on Supabase).
  The switches below belong to one sign-in on one website: signing out (or being signed out)
  turns them off, and every new sign-in starts with them off. A sync belongs to the sign-in
  and website it started with: if you sign out or sign in again while it is under way, its
  answer is dropped, and a request retried after a refused token only ever carries a token
  of its own sign-in on its own website.
- **Coming from a build where you typed the address.** Earlier builds asked for the website
  in Settings. The first launch of a newer one deletes that setting. If it named the
  website this build uses, nothing else changes. If it named another one, the website has
  changed: sync and summaries go off, and the sign-in you made there is set aside, kept in
  its file but not used (a run pointed back at that website with `AGENTNOTCH_WEB_URL` finds
  it again). Sign in to the new website to replace it.
- **What sync sends** (with **Sync sessions and usage** on: every 5 minutes, soon after a
  session ends, or on **Sync now**; at most five requests at a time, the rest half a minute
  later; after a failure, not before the backoff or the website's Retry-After ends). Sessions
  are captured, and usage readings kept, only while you are signed in with sync on. Turning
  sync off stops a sync that is under way.
  - Earlier sessions are sent too, from their transcripts, but only where the app knows which
    account ran them: from a folder whose `projects/` is its own (not shared), and only
    sessions begun after the app first saw that folder signed in as the account it names now.
    The app keeps that "signed in as … since …" date for each folder (from the first time this
    version runs, and again whenever someone else signs in there); a folder's history from
    before it, which may be another account's, is never sent.
  - Each signed-in Claude account: a key (SHA-256 of its account id and organization, so the
    same account in the same organization matches across people and Macs), its email,
    organization and plan, and your name for it.
  - Each session: its project folder's name and a key for the project (an HMAC of the path
    keyed with a random secret made on this Mac, `cloud-install-secret`, which never leaves it,
    so the path can't be guessed back from the key), Claude Code's title for it, where it ran
    (terminal, VS Code, Claude Desktop, SDK), its models, start, last activity and end times,
    how many responses, its token counts (input, output, cache writes and reads, subagents
    included) and an estimated cost: Claude Code's own figure when the status line gave one
    (at list prices, or the organization's rates when managed settings set `modelPricing`),
    otherwise the app's own at API list prices, worked out from the transcript's responses the
    way Claude Code does (per model: input, output, cache writes and reads, web searches, the
    advisor tool, fast mode), from the prices in Claude Code's own model catalog. A model it has
    no price for leaves the cost empty. The status line never runs for sessions in the VS Code
    extension's chat panel, Claude Desktop or the SDK, so theirs is the app's estimate; so is
    that of a session continued there after a terminal run, once the app's is the larger. On a
    subscription it is what the usage would have cost through the API, not what was paid. A
    session resumed under another account (Claude Parallel Profiles' "switch account, continue
    the same conversation") is sent once per account, each with only the responses, tokens and
    estimated cost of the time that account ran it: the status line's cost is the whole Claude
    Code process's total (a resumed session starts from what it had already spent), so it
    can't be divided between the accounts. The website's totals across accounts count such a
    session once.
  - Only responses the app can put on an account for certain. While it can't tell which
    account a running session runs as (a `~/.claude` that Claude Parallel Profiles is
    switching between accounts, a Claude Desktop session whose record isn't found, below),
    that session's new responses count for no account: its account's part ends at its last
    activity the app was sure of (sent, like a resumed session's, with only its part's
    estimated cost), and nothing it does meanwhile is ever synced under a guess.
    Once the account is certain again, counting goes on from then (for a new process of the
    session, from when that process started). A session first seen that way counts only from
    its first process the app is sure of. In a shared history (Claude Parallel Profiles'
    `~/.claude-shared`, or any `projects/` another folder reaches), where any account may
    continue a conversation, only what the app saw running counts: a conversation from before
    sync was on, continued now, counts from the Claude Code process the app saw, and what a
    conversation does in a process the app never saw running (one run while sync was off,
    while you were signed out or while the app was closed) counts for no account.
  - Usage-limit readings (5-hour, weekly, per model, extra usage) and where each came from:
    the usage check, the status line, `.claude.json` or Claude Desktop. A reset time the
    website wouldn't take (before 2023, or more than 32 days ahead) is sent empty.
  - This Mac's name, a random id made once, and the app version.
- **Never sent:** the paths of your files and folders, prompts, Claude's replies, file
  contents, tool input or output, or your Claude login. Accounts you don't track or have
  forgotten, and folders nobody is signed in to, are left out. The one exception is what a
  summary says, if you turn summaries on (below): it is Claude's own sentence about the
  session, written from your prompts and its replies. Before it leaves the Mac, its whitespace
  is folded to single spaces and absolute paths in it (under `/Users`, `/home`, `/private`,
  `/Volumes`, `/System`, `/opt`, `/var`, `/Library`, `/etc`, `/tmp` or `~/`) are cut to their
  last component, folder names with spaces included (`/Volumes/Macintosh HD/…`,
  `~/Library/Application Support/…`); the words after a path stay as written. A path that
  ends at a home folder (`/Users/<name>`, `/home/<name>`) becomes `~` and one that ends at a
  volume (`/Volumes/<name>`) becomes `…`, together with the capitalised words that finish the
  name (`/Volumes/My Passport`) or, for this Mac's own volumes and users, the whole name
  whatever it is, so no user or volume name is left. Text matching common key, token and
  password formats, and the value of any `NAME=value` or `NAME: value` whose name ends in
  `KEY`, `TOKEN`, `SECRET`, `PASSWORD` or `PASS`, is replaced by `[redacted]`. It may still
  name a file or folder by its name or a path within the project.
- **Session summaries** are a separate switch, **Summarise finished sessions with Claude**,
  off until you turn it on (and only while sync is on). Only sessions that end after you turn
  it on are summarised, never your earlier history. Ten minutes after such a session ends
  (with at least two responses), the app runs Claude Code on this Mac as that session's own
  account (for a session resumed under another account, each account's part as that account,
  from that part of the conversation only): `claude -p --model haiku --max-budget-usd 0.10`
  with hooks, tools and MCP servers off and no session saved, from an empty folder. It hands
  Claude Code your typed prompts and Claude's text replies (no tool output, text matching
  common key, token and password formats redacted, at most 24,000 characters) and asks for a
  summary without paths, names, hosts, URLs or credentials. That is an ordinary Claude Code
  request, so it uses that account's usage: a few cents per session with Haiku, at most $0.10
  a run, 20 an hour and 60 a day, and none for an account while its 5-hour limit is 80% used
  or more. Turning summaries or sync off, or signing out, stops a summary that is running.
  Only the one- or two-sentence summary goes to the website, scrubbed as described above.
  Turning summaries off (signing out and a change of website do too) deletes the summaries
  on this Mac that haven't been sent yet. **Summaries already sent stay on the website:**
  turning summaries off, signing out or removing the app doesn't take them off it, and the
  people you share the account with keep seeing them there. Remove them, or delete what was
  synced, on the website's *Settings* page (`<website>/settings`), which the *Cloud* section
  links to.
- **Sharing an account.** Pooling happens on the website: **Share accounts…** opens its
  *Pools* page. Create a share code for an account you have synced and give it to someone
  else who uses the same Claude account. Once they redeem it, everyone in the pool sees every
  member's sessions and usage for that account, and nothing else of each other's. The creator
  can revoke the code or remove members; members can leave. A Mac signed in to the website as
  one person and later as another sends its sessions to both; the website counts each session
  once, whoever's copies it holds, so sharing the account between those two people doesn't
  double anything.
- **Claude Desktop** here means two things: its plan-limit readings (with *Also read Claude
  Desktop's cached usage* on) and the Claude Code sessions it hosts. Claude Desktop runs those
  as whichever account it is signed in to, not as the folder they run in, so the app places
  each one by Claude Desktop's own record of it. Claude Code's session registry marks such a
  session (entrypoint `claude-desktop`, `claude-desktop-3p` or `local-agent`) with Desktop's
  id for it (`hostSessionId`), and the session is the account under whose folder
  `~/Library/Application Support/Claude/claude-code-sessions/<account>/<organization>/<id>.json`
  exists (checked with `lstat`, never through a link; the file is never opened; a sealed run
  checks nothing). The session then shows on that account's ring and is synced as that
  account's. When no account you have here has that record (or more than one does), the
  session is shown where its folder puts it but never synced; a Claude Desktop session found
  only on disk, not running, is never backfilled. It does not mean claude.ai chats, in Claude
  Desktop or a browser: the app doesn't read those.
- **Pointing a run at a development site:** `AGENTNOTCH_WEB_URL=http://localhost:3000`
  overrides the built-in address for that run, and the *Cloud* section says so under
  *Website* (a sign-in saved for another website is set aside, not deleted). Only an address
  the app accepts counts: https, or http to this Mac. A sealed run ignores it, shows a
  signed-in example and sends nothing.

## Privacy: what it reads, writes and runs

- **Never:**
  - a Claude login token, keychain item or credential file (`.credentials.json`,
    `sessions/*.key`);
  - a network request of its own for Claude.
- **Network:** only for these two things.
  - With [cloud sync](#cloud-sync-optional), to the website built into the app
    (`app-config.json`) and its Supabase project: to sign in, and to sync once you turn sync
    on.
  - In a downloaded copy, for [updates](#updates): about once a day, a request for
    `https://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml` (GitHub
    redirects it to its file host) and, when there is a newer version, the download of its
    zip from the same release. They name only the app and its version (Sparkle's usual
    User-Agent); Sparkle's system profiling, which would add details of this Mac, stays off.
    Switch off **Install updates automatically** in Settings › General to stop them;
    **Check now** then checks only when you click it. A copy built from source makes no
    update request.
- **Reads:**
  - each config folder's `.claude.json` (Claude Parallel Profiles' stores included): only
    the signed-in identity and Claude Code's cached usage figures; the rest of the file is
    skipped over, never parsed;
  - for a session in a shared `sessions/` folder, its process's `CLAUDE_CONFIG_DIR` from the
    kernel (same user only; nothing else of the environment is kept), to tell which account
    runs it;
  - its `sessions/` registry (`<pid>.json` only) and the transcripts under `projects/`, for
    titles, tasks, context and the conversation (and, with cloud sync on, token counts);
  - while a session Claude Desktop hosts is running, whether Claude Desktop's record of it
    exists under `~/Library/Application Support/Claude/claude-code-sessions/<account>/<organization>/`
    (an `lstat`, and a listing of an account's folder when its organization isn't known; a
    link there is never followed and no file there is opened), to tell which account the
    session runs as;
  - with **Also read Claude Desktop's cached usage** on (the default), the cached `/usage`
    response Claude Desktop keeps for the same organization.
- **Runs `claude`** only for these things:
  - to find it: `command -v claude` in your login shell, once, when it isn't in a usual place;
  - to check its version: `claude --version`;
  - to check usage: `claude -p` in stream-JSON mode, with hooks off, no session saved and an
    empty working folder. It asks only for `get_usage` (what `/usage` shows) and makes no
    model request. Claude Code may update its own files while it runs. It runs once per
    account, in a folder Claude Code runs in as that account (with Claude Parallel Profiles,
    one of its VS Code workspaces' folders, `~/.claude` only when it has none), never in a
    Claude Parallel Profiles store; an account no window runs isn't checked. Who the folder is
    signed in as is checked right before and after; an answer from a folder that changed
    hands meanwhile is thrown away;
  - to summarise a finished session, only with *Summarise finished sessions with Claude* on:
    `claude -p --model haiku --max-budget-usd 0.10`, chosen and checked the same way, as the
    session's own account.
    This one is a model request, and uses that account's usage (see
    [Cloud sync](#cloud-sync-optional)).

  The usage check runs from the first launch, before you turn on control. Set *Check usage
  every* to **Off** to never run it. Readings then come only from the live status line and
  Claude Code's cache.
- **Writes, only after you turn it on:**
  - hook entries and a status line wrapper in the `settings.json` of each folder a tracked
    account runs in (never a Claude Parallel Profiles store or `~/.claude-shared`);
  - in stores and `~/.claude-shared`, only to take out its own or Superpowered Vibe Notch's
    leftovers, with a backup kept until that cleanup is finished;
  - two scripts in `<config folder>/hooks/`: `agentnotch-hook.py` and
    `agentnotch-statusline.py`. They run with the developer tools' `python3` and
    talk only to this app's local socket.
  - Before every change it saves a backup, `settings.json.agentnotch-<time>.bak`,
    and keeps the five newest. The file as it was before the first change is kept as
    `settings.json.agentnotch.original.bak`.
  - Nothing is written to a `settings.json` that fails to parse.
- **Its own state** lives in `~/Library/Application Support/Agent Notch/Claude/` (0700):
  accounts, the review queue, usage state, the hook socket, and since when each config folder
  has been signed in as the account it names (`cloud-folder-logins.json`: digests of who, and
  a date; never sent; cloud sync's backfill uses it to tell whose older sessions are whose).
  With cloud sync, also the website sign-in (`cloud-session.json`, 0600, from when you sign
  in), the secret project keys are made with (`cloud-install-secret`, 0600, never sent:
  created the first time sync has a session to send, so only once you are signed in with sync
  on and never at launch, then kept) and what sync keeps between passes (`cloud-*.json`:
  sessions and readings waiting to be sent, summaries, what was sent).
- **Turning it off.** Switch off *Hooks in tracked accounts* in Settings › Claude Code. That
  takes the hooks and scripts out of every account and restores each status line exactly.
  Untracking or forgetting an account does the same for that account; with Claude Parallel
  Profiles, `~/.claude` keeps them while another account is tracked (the extension copies
  whichever account you last used into it), and an untracked account's sessions there are
  hidden, their permission prompts left to the terminal.
- **Removing the app.** Turn the hooks off first (and **Sign out…** under *Cloud*, if you
  signed in), then quit. Delete the app,
  `~/Library/Application Support/Agent Notch` and
  `~/Library/Caches/com.rivantmedia.agentnotch` (where updates are downloaded), then run
  `defaults delete com.rivantmedia.agentnotch`.

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
  `swift run --package-path Packages/ClaudeControl agentnotch-inspect-accounts` (read-only).
- **Logs:** `log stream --predicate 'subsystem == "com.rivantmedia.agentnotch"' --level debug`.

## Windows (preview)

Agent Notch also runs on Windows 10 and 11 (x64), as a **preview**: it is built and tested
automatically (including against the real Claude Code, in one hermetic CI job), but it has
not been used by people yet. Please report anything odd. It is upstream's Windows port
(`windows/`, Rust and Tauri 2 on WebView2) with the same Claude Code session control as the
Mac app, in Rust crates named `agentnotch-*`. It shows the same rings, sessions panel and
cloud sync, and follows the same rules as everything above (consent before writes, nothing
sent before you turn sync on, no login token). This section covers what differs.

### Download (Windows)

Download `AgentNotch-<version>-Setup.exe` from the
[latest release](https://github.com/rivantmedia/agentnotch/releases/latest), or from the
website's **Download** page.

1. **Run the installer.** It installs for your user only, with no administrator, into
   `%LOCALAPPDATA%\Agent Notch`, and fetches Microsoft's WebView2 if it is missing.
2. **Let SmartScreen run it.** The installer isn't code-signed yet, so Windows SmartScreen
   may say "Windows protected your PC": choose **More info**, then **Run anyway**.
3. **Smart App Control.** With Smart App Control turned on (Windows 11), unsigned apps are
   blocked with no way around it: Agent Notch can't be used there until it is code-signed
   (see *Code signing* under [Releases (Windows)](#releases-windows)).
4. Go on with [First use (Windows)](#first-use-windows).

### Build from source (Windows)

You need a Windows PC with Rust 1.98.1, Node 22, PowerShell 7 and a network connection (the
bundler fetches NSIS once). From the repository root:

```powershell
npm ci --prefix windows/tools
windows\scripts\agentnotch-build.ps1 -Out out
```

`agentnotch-build.ps1` builds the hook (`agentnotch-hook.exe`), the app and the installer, and
leaves `AgentNotch-<version>-Setup.exe` in `out`. A copy built this way carries no update key
and no feed: it [never updates itself](#updates-windows). The fork's crates also build and
test on a Mac (`cd windows && cargo test --locked -p agentnotch-engine`); only the
Windows-only code needs Windows, and CI type-checks it for `x86_64-pc-windows-msvc`.

### First use (Windows)

1. **Open Agent Notch** from the Start menu. The notch appears at the screen's edge, and the
   tray icon's menu has **Settings**. Opening the app again while it runs brings Settings
   forward.
2. **Turn on Claude Code control.** It asks for the same consent as the Mac app. The card at
   the top of Settings › Claude Code (Settings opens on that pane until you answer) and of the
   sessions panel lists every `settings.json` it will edit:
   `%USERPROFILE%\.claude\settings.json` and those of the other folders Claude Code runs in.
   **Turn on** writes the hook entries there and, where it can, the status line (below);
   **Not now** writes nothing (sessions still show, from Claude Code's own session files, but
   without approvals from the notch). Before every change it saves a backup (`settings.json.agentnotch-<time>.bak`,
   the five newest, and the file as it was before the first change as
   `settings.json.agentnotch.original.bak`). It keeps the file's line endings and byte order
   mark, and it refuses a file that doesn't parse or that another program holds open.
3. **Restart Claude Code sessions that were already running**, as on the Mac.

How the hooks work on Windows:

- **The hook is a small program**, `agentnotch-hook.exe`, copied into `<config folder>\hooks\`.
  It talks only to this app's local pipe (`\\.\pipe\agentnotch-hook-<your SID>`; no network
  port; a pipe owned by anyone else, or by a low-integrity process, is refused). It always exits
  0, so a missing or stopped app never blocks Claude Code.
- **Two command forms.** Usually the entry is a plain command line,
  `C:/Users/you/.claude/hooks/agentnotch-hook.exe hook`, which Git Bash and PowerShell both
  run (with the folder's 8.3 short name when the path has a space or another character a shell
  would read). When every Claude Code on the PC is 2.1.139 or later, VS Code's and Claude
  Desktop's bundled copies included, it writes Claude Code's exec form (`command` plus `args`)
  instead. A folder that neither form can name shows "Can't be hooked here", and no hook of
  this app's stays there: an older Claude Code would run an exec-form entry without its
  arguments, through Git Bash.
- **The status line.** A folder with none gets this app's. One you set yourself is wrapped
  (it keeps running, through Git Bash) only when Git Bash is installed and the command needs no
  PowerShell or cmd and has no Windows `\` paths. Otherwise it is left alone and Settings says
  why; usage then comes from Claude Code's own `get_usage` and `.claude.json`.
- Sessions inside WSL aren't tracked yet.

Everyday use is as on the Mac (the panel, the chat and its status line, alerts, cloud sync),
with these differences:

- **Usage limits are announced once per account**, as on the Mac: one notification from the
  account and one chime and peek. Windows has no "limit reached" card. What was announced is
  kept in `limit-announcements.json` in the app's state folder, so a relaunch doesn't
  announce it again.
- **Shared history.** Cloud sync's rule for a shared history applies when a folder's
  `projects\` (or a project folder in it) is a junction or symbolic link, or when another known
  folder reaches the same `projects\`. Folders are compared by their file identity, never by
  how the path is spelled.

### Privacy (Windows)

The rules in [Privacy](#privacy-what-it-reads-writes-and-runs) hold, with the Windows paths:

- **Its own state** lives in `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude` (accounts, the
  review queue, usage state, the website sign-in and cloud sync's files, as on the Mac) and,
  for upstream's own settings and logs, `%APPDATA%\Agent Notch`.
- **It never reads a Claude login token**: not `.credentials.json`, not the Credential
  Manager, not `sessions\*.key`. Usage comes from Claude Code's own `get_usage`, the cache in
  `.claude.json` and the status line.
- **The GLM provider is upstream's, and reads one key.** Codenotch's GLM provider (a usage
  ring for Z.ai's GLM plan) reads `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` from the `env`
  of Claude Code's `settings.json`, and keeps it only when `ANTHROPIC_BASE_URL` points at a
  Z.ai host. It reads the value before it checks the host, so an Anthropic API key you set
  there is held in memory for that moment and dropped unused. That is an API key you
  configured, not a Claude login token, and it goes nowhere else. It is kept as upstream
  ships it, and `Scripts/verify-token-free.sh` pins it, so it can only ever be that one read.
- **Updates** read the Windows feed (see [Updates (Windows)](#updates-windows)), about once
  at start; nothing else is sent to GitHub.

### Uninstalling (Windows)

Settings › Apps removes the program; the uninstaller first asks a running Agent Notch to quit.
Your Claude Code hooks are **kept** unless you tick **Delete the application data** in the
uninstaller or pass `/REMOVEHOOKS` to it
(`"%LOCALAPPDATA%\Agent Notch\uninstall.exe" /REMOVEHOOKS`): an uninstall that runs by
itself (a reinstall, an update) must never touch a `settings.json`. A hook left behind exits
at once while the app is gone, so it does no harm. To take the hooks
out first, switch off Claude Code control (the hooks) in Settings (it restores each
status line exactly), or run `"%LOCALAPPDATA%\Agent Notch\agentnotch.exe" uninstall-hooks`.
Signed in to the website? **Sign out…** under *Cloud* first. **Delete the application data**
also removes `%LOCALAPPDATA%\com.rivantmedia.agentnotch` and `%APPDATA%\Agent Notch`.

### Updates (Windows)

An installed copy checks for an update shortly after it starts; **Settings › General**
installs one. It reads `latest.json` from the latest release
(`https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json`). Every
update is signed, and the app verifies the signature with the key built into it before the
installer runs; the signature names the version, so a feed can't pair a newer number with an
older installer. The key is derived from the Mac updates' key (see
[Releases (Windows)](#releases-windows)). **Copies built from source never update
themselves**: they carry no key and no feed, and the General pane says so.

### Development (Windows)

The same switches as above work, with these differences; the full table is Appendix C of
`docs/design/DESIGN-WIN.md`.

- `AGENTNOTCH_SAFE_MODE=1` seals the run (fixtures; no settings, sessions, network or
  subprocesses), failing closed. `--no-install` / `AGENTNOTCH_NO_INSTALL`,
  `AGENTNOTCH_NO_NOTIFICATIONS`, `AGENTNOTCH_USAGE_PROBE`, `--dump-state` and
  `AGENTNOTCH_WEB_URL` behave as on the Mac.
- Paths: `AGENTNOTCH_SUPPORT_DIR`, `AGENTNOTCH_SOCKET` (a pipe name, `\\.\pipe\…`; the hook
  follows it only with `AGENTNOTCH_DEV=1`) and `AGENTNOTCH_EXTRA_CONFIG_DIRS` (separated by `;`).
- `USERPROFILE` is the home the engine uses, as it is for Claude Code: point it at a temporary
  folder to run against a throwaway setup.
- `AGENTNOTCH_DEV_BROWSER_LOG` (with `AGENTNOTCH_DEV=1`, never sealed) appends the sign-in's
  address to a file instead of opening a browser.
- Sealed only: `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH`, `AGENTNOTCH_PANEL_SELF_TEST` (with
  `AGENTNOTCH_SELF_TEST_OUT` and, for a scaled run, `AGENTNOTCH_SELF_TEST_SCALE=1.25`) and
  `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>`; `windows\scripts\agentnotch-selftest.ps1` runs them.
- **Command line.** `agentnotch.exe doctor` (what the app sees, whether it updates itself, and
  a running copy's status), `inspect-accounts` (read-only), `install-hooks` (only after the
  consent), `uninstall-hooks [--quiet]`, `control status|quit` (asks the running copy over its
  pipe; exit 3 when none runs) and `autostart on|off`. Sealed, `uninstall-hooks` removes
  nothing, and `inspect-accounts`, `install-hooks` and `control` are refused (exit 2).
- CI: `.github/workflows/agentnotch-windows.yml` builds the installer and runs the smoke test
  (`windows/scripts/agentnotch-smoke.ps1`: install, sealed self-tests, the real hook program,
  a fake website, updates, uninstall). **The only place a real Claude Code ever runs** is that
  workflow's `real-claude` job ("Real Claude Code (hermetic)",
  `windows/scripts/smoke/real-claude.ps1`). It runs on a GitHub-hosted Windows runner and
  refuses anywhere else, never for a release build. It installs Claude Code 2.1.285 from npm,
  checked against its integrity hash, into a throwaway profile, with a fake API key, against a
  fake Messages API on loopback, with the runner's firewall letting it reach nothing else. It
  holds no secret and no login, and it runs against the app it just installed.
  `.github/workflows/claude-code-facts.yml` checks weekly what newer Claude Code versions
  changed (it only reads their packages; it runs none of them).

### Releases (Windows)

Windows goes out in the same release as the Mac, from the same *Release* workflow: one tag,
`agentnotch-v<VERSION>`, and everything or nothing is published.

- **Six files.** `AgentNotch-<V>.dmg`, `AgentNotch-<V>.zip` and `appcast.xml` for the Mac, then
  `AgentNotch-<V>-Setup.exe` (the download), `AgentNotch-<V>-Setup.exe.sig` (its update
  signature) and `latest.json` (the Windows feed) for Windows. The website's `/download` page
  offers the disk image and the installer, under **Windows (preview)** with the SmartScreen
  and Smart App Control steps, and never a signature or a feed. The release notes carry the
  same Windows steps.
- **Version.** `VERSION` and the `"version"` line of `windows/codenotch/tauri.conf.json` must
  be equal. `Scripts/bump-version.sh <V>` sets both (the release commit is its output);
  `Scripts/bump-version.sh --sync` copies `VERSION` into the Windows file, which is what a
  branch needs after merging `main`.
- **The update key.** There is no second secret: the Windows key is derived from the Mac
  update key (`SPARKLE_ED_PRIVATE_KEY`) in a job of its own, with the `windows/agentnotch-release`
  tool, which also signs the installer and writes `latest.json`. The Windows build job only ever
  sees the public key. A release is refused when the derived key differs from the one the
  previous release's `latest.json` names, unless a manual run ticks *Publish although the update
  key changed*. After the first Windows release, commit the key the run summary prints as
  `Scripts/tauri-update-public-key.txt` (an optional pin file): every later release then has to
  derive that key. Rotating the Mac key also changes the Windows key, and an installed Windows
  copy takes only an update signed with the key it was built with. Moving them over takes one
  bridge release whose installer carries the new key but is signed with the old key's
  derivative; the Release workflow can't make one yet (it signs with `SPARKLE_ED_PRIVATE_KEY`
  only). So keep the old private key (as `SPARKLE_ED_PRIVATE_KEY_PREVIOUS`) and add that signing
  path before rotating, or every installed Windows copy has to be reinstalled by hand.
- **`skip_windows`.** A manual run can tick *Publish without the Windows build*: the last resort
  that keeps a Mac release from waiting on Windows. Installed Windows copies then miss that
  release until the next one, and the run warns when a previous release had Windows files.
- **Code signing.** Windows releases are unsigned until the maintainer adds signing secrets.
  Make an environment named `windows-signing` (deployment branches: `main` only) and put in it
  either `WINDOWS_SIGN_PFX_BASE64` and `WINDOWS_SIGN_PFX_PASSWORD` (a code signing certificate
  as a `.pfx`; optionally `WINDOWS_SIGN_TIMESTAMP_URL`) or all six `WINDOWS_SIGN_AZURE_*`
  values (Azure Artifact Signing: endpoint, account, profile, tenant, client id and secret).
  The next release then signs the app, the hook and the installer, and its notes drop the
  SmartScreen steps. Half a set is an error. Without the environment, release builds come out
  unsigned and the notes say so.

## Development

- **Sealed mode.** `AGENTNOTCH_SAFE_MODE=1` (or `CODENOTCH_DEMO=1`) starts the app sealed: fixture
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
- **Development switches** (`1`, `true` or `yes`): `--no-install` / `AGENTNOTCH_NO_INSTALL`,
  `AGENTNOTCH_NO_NOTIFICATIONS`, `AGENTNOTCH_USAGE_PROBE`, `--dump-state` / `AGENTNOTCH_DUMP_STATE` and
  `--dev-console` / `AGENTNOTCH_DEV_CONSOLE`. Path switches: `AGENTNOTCH_SUPPORT_DIR`, `AGENTNOTCH_SOCKET`
  and `AGENTNOTCH_EXTRA_CONFIG_DIRS` (`:`-separated). See `Engine/Core/DevFlags.swift`. The
  installed hook scripts follow `AGENTNOTCH_SOCKET` only when `AGENTNOTCH_DEV=1` is set too.
  `AGENTNOTCH_WEB_URL=<url>` overrides the cloud sync website the build carries
  (`app-config.json`) for one run (https, or http to this Mac; ignored when sealed). Session summaries never run in a `--no-install` run.
- **No updates outside releases.** A debug or `--release` build, a sealed run and a `.dev`
  copy carry no update feed, so they never check for updates or replace themselves with a
  release. Only the release workflow's builds update themselves (see [Releases](#releases)).
- **Website.** `web/` is the cloud sync website; its README covers setting up Supabase,
  Google sign-in, the database and Vercel. The app and the site agree on
  [`web/contract/`](web/contract/README.md): both test against its JSON fixtures. Its
  `/download` page offers the latest release's files
  ([`web/README.md`](web/README.md#downloads)).
- **Sealed-only switches** (ignored in a live run): `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH=<route>`,
  `AGENTNOTCH_PANEL_CLOSE_AFTER=<seconds>`, `AGENTNOTCH_PANEL_SELF_TEST=1`,
  `AGENTNOTCH_SEALED_SWITCH_OFF=<ring id>`, `AGENTNOTCH_SEALED_CAPTURE=<dir>` (timeline PNGs) and
  `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>` (render, then exit). See `Scripts/spm-run-sealed.sh` and
  `Sources/ClaudeBridge/ClaudeSealedDemo.swift`.
- **Simulated sessions.** `Packages/ClaudeControl/DevTools/simulate-sessions.py` drives a
  running build with fake Claude Code sessions over a private socket (`AGENTNOTCH_SOCKET`,
  `AGENTNOTCH_SUPPORT_DIR`, `AGENTNOTCH_EXTRA_CONFIG_DIRS`; see its header). It never touches a real
  Claude config.
- **Checks.**
  - `Scripts/check-seams.sh`: every edit to upstream's files is a listed, tagged seam.
  - `Scripts/verify-token-free.sh`: no Claude path can read a token.
  - `Scripts/cloud-contract-e2e.sh`: the app's real sync request goes through the website's
    real sync route into a throwaway Postgres, and the route's answer comes back through the
    app's client. It needs Docker and `web/node_modules`, and makes no network call (see
    [`web/README.md`](web/README.md#checks)).

  The first two compare against `upstream/main` (`git remote add upstream
  https://github.com/vinzdg/codenotch`) or `UPSTREAM_REF=<commit>`.
  `Packages/ClaudeControl/Scripts/embed-scripts.sh --check` confirms the embedded hook
  scripts match the `.py` files.

  On GitHub, `.github/workflows/fork.yml` runs the first two, the test suites and a build
  for every push to `main` and every pull request, and checks that the debug and sealed
  builds carry no update feed.
- **Licences.** See [License](#license) at the end: Codenotch is MIT; `Packages/ClaudeControl` is Apache-2.0, derived from Superpowered Vibe Notch and Vibe Notch.

### Releases

Releases are published to this repository's
[GitHub releases](https://github.com/rivantmedia/agentnotch/releases) by
`.github/workflows/release.yml` (the *Release* workflow). Installed copies update from there.

- **Version.** The root `VERSION` file (one line, `major.minor.patch`; the first release is
  1.0.0) is the app's version. `Scripts/spm-build-app.sh` writes it as both
  `CFBundleShortVersionString` and `CFBundleVersion`, which is what Sparkle compares.
  `project.yml`'s version numbers stay upstream's and only move with a merge. The Windows
  installer's version (`windows/codenotch/tauri.conf.json`) must equal it:
  `Scripts/bump-version.sh <V>` sets both.
- **Releasing.** Raise the version with `Scripts/bump-version.sh <V>` and push the commit to
  `main`. A push to `main` that changes
  the update key (`Scripts/sparkle-public-ed-key.txt`) starts it too, which is how the first
  release goes out; on a version that is out already, that run does nothing. Runs wait their
  turn, one at a time in the order they started, and none is cancelled. The workflow runs the
  Fork workflow's checks first, then, on a macOS 26 runner with Xcode 26.6,
  `Scripts/release-build.sh`, and, beside it, the Windows build (see
  [Releases (Windows)](#releases-windows)). It creates the release `agentnotch-v<VERSION>`,
  titled "Agent Notch <VERSION>", as a draft with the Mac's three files,
  `AgentNotch-<VERSION>.dmg` (the download), `AgentNotch-<VERSION>.zip` (the update) and
  `appcast.xml` (the feed), and Windows' three. The notes are the install steps plus GitHub's
  list of changes. Once every file is uploaded whole it
  publishes the release as the latest, so `releases/latest/download/appcast.xml` never points
  at a release without its files. Tags start with `agentnotch-v` because upstream's `v1.x`
  tags are in every clone that fetches upstream.
- **What stops a release.** A `VERSION` below the latest `agentnotch-v*` release
  (installed copies would never take it), a draft it didn't make, a tag for that version at
  another commit, or an update key other than the one the latest release was built with
  (see below). A version that is released already gives a green run with nothing to do; a
  draft an interrupted run left (titled "Agent Notch <VERSION>") is deleted and made again,
  also when you re-run the failed jobs. Drafts are checked by the publishing job, the only one
  whose token can see them, just before it makes its own; a foreign one stops it there,
  after the build, and nothing is deleted.
- **Only from `main`.** Every run, a dry run too, fails on any other branch before it builds
  anything: the job holds the update key, and only `main`'s code may run with it.
- **Dry run.** Actions › Release › Run workflow on `main`, with *Dry run* ticked, makes the
  same files and keeps them as the run's artifact, `AgentNotch-<VERSION>-dry-run`. Nothing is
  published, and the problems in *What stops a release* are only warnings. A manual run
  without it publishes, like a push.
- **The update key, once.** Every update is signed with one Ed25519 key.
  1. On your own Mac, run `Scripts/release-make-keys.sh --update-key ~/agentnotch-keys` (a
     folder outside the repository; one inside it is refused). It makes the key with
     CryptoKit, never in the Keychain. The private key goes to
     `~/agentnotch-keys/sparkle-ed-private-key.txt` (0600, never printed) and the public key
     into `Scripts/sparkle-public-ed-key.txt`.
  2. Set the `SPARKLE_ED_PRIVATE_KEY` secret (see *Secrets*).
  3. Commit `Scripts/sparkle-public-ed-key.txt` and push it to `main`. That push starts the
     Release workflow, which publishes the current `VERSION` unless it is out already.

  Keep a copy of the private key somewhere safe: installed copies take only updates signed
  with it. If a run already stopped for want of the key, re-running it won't help when the
  key file was missing, because a re-run checks out the same commit: push the key file, or,
  if it is on `main` already, start Actions › Release › Run workflow on `main` with *Dry run*
  unticked. When only the secret was missing, set it and re-run that run.

  `--rotate` replaces a committed key, but then every installed copy, however it is signed,
  stops updating and has to be reinstalled by hand: each checks an update against the key it
  shipped with before unpacking it. So the workflow publishes with a key other than the
  latest release's only from a manual run with *Publish although the update key changed*
  ticked.
- **A signing identity, optionally.** Without one, releases are signed ad hoc, with a new
  signature each time, so macOS forgets the app's Automation permission and keychain
  "Always Allow" grants at every update. `--signing-cert <dir>` makes a self-signed code
  signing certificate ("Agent Notch Release Signing", or `--cert-name <name>`) as a p12 with
  a random password. Every release is then signed by the same identity and keeps those
  grants. Gatekeeper still refuses the first open: only a notarized Developer ID build
  avoids that.
- **Secrets.** They are secrets of the repository's `release` environment, whose only
  deployment branch is `main`, so no other branch's code can read them (a repository secret
  reaches a run from any branch). The script prints the commands that make the environment,
  once, and set each secret with
  `gh secret set <NAME> --env release -R rivantmedia/agentnotch < <file>`. It runs them only
  with `--set-secrets`. Delete any repository-level copy of these secrets
  (`gh secret delete <NAME> -R rivantmedia/agentnotch`): `--set-secrets` warns about one but
  never deletes it. You can also give the environment a required reviewer in Settings ›
  Environments, so every run waits for your approval.
  - `SPARKLE_ED_PRIVATE_KEY`, required. Without it, or without a committed public key, the
    release job (dry runs included) fails with these setup steps in its run summary.
  - `MACOS_SIGNING_P12_BASE64` and `MACOS_SIGNING_P12_PASSWORD`, optional: the self-signed
    identity, or a Developer ID. It is imported into a temporary keychain that is deleted
    afterwards.
  - `APPLE_NOTARY_API_KEY_P8_BASE64`, `APPLE_NOTARY_API_KEY_ID` and
    `APPLE_NOTARY_API_ISSUER_ID`, optional: an App Store Connect API key, all three or none,
    and only with a Developer ID Application identity. The app is then signed for the
    hardened runtime (`spm-build-app.sh --hardened-runtime`), notarized and stapled, and so
    is the disk image; the release notes leave the first-open steps out.
- **Only release builds carry the feed.** `release-build.sh` builds with
  `Scripts/spm-build-app.sh --release --universal --with-updates`. `--with-updates` writes
  the feed and the public key into the Info.plist, and is refused unless the bundle id is
  exactly `com.rivantmedia.agentnotch` and the key file holds a valid key. Every other build
  has no feed or key and automatic checks off. The app also turns updates on only for that
  bundle id, feed and key, never sealed or under test (`Fork.updatesEnabled`), and always
  reads its own feed, whatever the defaults say.
- **Trying it on your Mac.** `release-build.sh` does everything but publish. Try it with a
  throwaway key: `release-make-keys.sh --update-key <scratch folder>`, then
  `release-build.sh --host-arch --out <scratch folder> --ed-key-file <its private key>`,
  both with `AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE=<scratch file>` so the committed key file is
  left alone. `--host-arch` builds for this Mac's architecture only: the Command Line Tools
  have no x86_64 Swift compatibility libraries, so a universal build needs Xcode 26, and
  the workflow refuses `--host-arch`. Never open the `app/Agent Notch.app` it leaves in the
  output folder: it has the real bundle id and the feed, so it would update itself.

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

**Agent Notch** (formerly Superpowered Codenotch) is a fork of Codenotch (MIT, © 2026 Vinz).
`Packages/ClaudeControl` is Apache-2.0, derived from Superpowered Vibe Notch and
Vibe Notch (© 2025 Farouq Aldori); see its [LICENSE](Packages/ClaudeControl/LICENSE)
and [NOTICE](Packages/ClaudeControl/NOTICE). It links swift-markdown (Apache-2.0)
and swift-cmark (BSD-2-Clause). The app bundle carries every one of these licences
and notices in `Contents/Resources`.
