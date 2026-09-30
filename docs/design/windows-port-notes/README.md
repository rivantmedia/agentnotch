# Windows port: the porting notes

These six files are the porting spec the Windows port was built from. They were written by
reading the Mac app (`Packages/ClaudeControl`, `Sources/ClaudeBridge`), upstream's Windows port
(`windows/`) and the release tooling as they stood when the port began (main at `aeb6419`,
upstream `642d329`). [`../DESIGN-WIN.md`](../DESIGN-WIN.md) and comments in the Windows code cite
them by section:

| Tag | File | Area |
|---|---|---|
| **WP§n** | [`win-port.md`](win-port.md) | upstream's `windows/` tree: process model, commands, events, placement, its token path, the rebrand |
| **HS§n** | [`hooks-sessions.md`](hooks-sessions.md) | the hook script, the status line, the installer, the socket protocol, the session pipeline, control, focus |
| **AU§n** | [`accounts-usage.md`](accounts-usage.md) | paths, folders, identities, rings, usage formats, the usage store, the probe, Claude Desktop's cache |
| **CL§n** | [`cloud.md`](cloud.md) | the web contract, sign-in, files, sync, the ledger, the scanner, the outbox, summaries |
| **UI§n** | [`ui.md`](ui.md) | tokens, what the notch gains, the panel, chat, the settings pane |
| **BT§n** | [`build-tooling.md`](build-tooling.md) | the release pipeline, Tauri facts, CI, the seam scripts, local tooling |

They are notes, not documentation that is kept up to date: where they and the code disagree, the
code is right. `<repo>` is this repository and `<scratch>` the scratch folder of whoever wrote
them.
