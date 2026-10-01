# agentnotch-ui-tests

Node tests and a snapshot tool for the app's web pages (`../codenotch/ui`: upstream's `notch.html`
and `settings.html`, and the fork's `agentnotch/*`). Nothing to install: Node's built-ins only, so
CI runs it on a bare Node 22.

## Running

```sh
cd windows
node --test agentnotch-ui-tests              # every suite (Node 22+ loads index.js, which requires them)
node --test agentnotch-ui-tests/common.test.cjs   # one suite
node scripts/check-ui-scripts.mjs            # every inline script of every page parses
for f in codenotch/ui/agentnotch/*.js; do node --check "$f"; done
```

A run is only good when it ends `fail 0` **and** reports the number of tests you expect: a suite
that was never loaded also says "pass". Name a new suite `<area>.test.cjs` directly in this
folder (not a sub-folder): `index.js` finds it there.

## The suites

| Suite | What it holds |
|---|---|
| `contract.test.cjs` | `lib/contract.cjs` against the engine's own files: every `calls.json` example is accepted, every method of `hub/api.rs` (its `Call` enum and both glue lists, read as text) is in the table and nothing else is, the window gate of DESIGN-WIN §3.7, `set_setting` keys against `core/settings.rs`, every `events.json` event, and `scenes.json`. |
| `dom.test.cjs` | `lib/dom.cjs` and `lib/audit.cjs`: the parser turns unescaped hostile text into real elements and attributes (so the audit can find an injection), selectors, events, focus, form state, `MutationObserver`, `textContent` escaping. |
| `common.test.cjs` | `ui/agentnotch/common.js`: `esc` over the hostile strings, the formatters (ported from `UsageFormatterTests`, `ElapsedCopyTests`), badge label, context level, the AnswerGate with the fake clock, the key router (ported from `B_KeyRouterTests`, Ctrl for Cmd and Alt for Option), browser accelerators, routes, the DOM morph. |
| `notch.test.cjs` | `ui/agentnotch/notch.js` and `notch.css` on the real `notch.html` (upstream's script and ours in one context): the page-order test, the wrapped-globals test (every name of `agentnotch._.UPSTREAM` is a top-level declaration; a mutated copy of the page proves the check bites), `claudeCells`, the activity states with the fake clock (success settles at `success_settles_at_ms`), the `stale` class, badges (ported from `C_RingBadgeLayoutTests`: angles, centres at r = 33, the label rule, the weekly-ring clearance; placement per edge and when the notch moves), the card's rows from `card`, the session cap (`NotchSessionCapTests`), `ringClick` in both modes, `hover_click` smart/panel/terminal, the `foldAllowed` and `showCard` wrappers, the peek, resting marks (`Fix_RestingMarkShapeTests`, `C_BurstAndRestingMarkTests`, CSS per edge), the notice, `showScene` and `layoutReport`, and hostile strings through the ring label, a11y text and the card's name, detail and waiting line. |
| `panel.test.cjs` | `ui/agentnotch/panel.html`, `panel.js`, `panel.css`, `theme.css` (the panel's shell, on the real page): the CSP meta and "no inline script, no handler" over the markup, every token of UI§1 in both themes with the Mac's values, the type scale, the stepped arc and the still rules, the tail per side in the CSS, the load (five scripts in order, only `snapshot` asked), the theme, the first request (`window.__AGENTNOTCH_PANEL__`) and `an:panel`, the header from the fixture (strip, Sealed badge, chips with counts and the needs-you count, the filter), the gear menu and what each item calls (`autoOpen`, the two notify switches, Settings), pin (`panelPinned`, refusal, the override's grace), close and Esc's order (menu, chat, list, panel), routes (`B_PanelStateTests`), the keyboard gate's flag, `panel_engaged`, `panel_report_size`, accelerators, the tail per placement, hostile labels and messages, a snapshot that keeps an open menu, the sealed header scenes and `layoutReport`. |
| `panel-actions.test.cjs` | What the panel sends (`panel.js`): the row action bars per kind of request (Deny / Always / Allow, the review-first variant, chips 1-4 and Other…, Answer…, Review plan / Approve, the terminal-only line), the AnswerGate through real clicks on the fake clock (349 ms nothing, 350 ms one call, once per `tool_use_id`, a replacing request waits again, 600 s memory, a re-opened panel re-arms), each answer's shape against `calls.json`, the question answers map, `not_pending`/`peer_gone`, every key of the router with Ctrl/Alt, THE KEYBOARD GATE (nothing acts or types before `an:panel_focus {focused:true}`, DOM focus never opens it, nothing is queued), Enter never clicking an answer or Turn on, the consent card (every file, `hook_consent` only from Turn on's own click, once), each banner from a changed `setup`, hostile strings through every part, and the four scenes. Every "must not act" case asserts that no call at all was made. |
| `chat.test.cjs` | The chat screen (`chat.js`, the chat part of `panel.css`/`panel.js`): entering and leaving (`chat_open`, `chat_close`), the `an:chat` protocol (reset, patch, `removed`, `order`, stale revision, wrong session), every item kind, tool marks and results (closed until a click, Edit always open), subagents, thinking, keyed re-rendering (only a changed item is drawn, expanded state kept), loading / empty / working / ended, "Show N earlier messages" (`chat_more`), scroll following and keeping the reader's place, images only on demand and only as image data URLs, https-only links and Enter on a link behind the keyboard gate, the header (account tag, subtitle, tasks, context levels, terminal button), the task board, the size report (capped at 780), hostile strings through every transcript field, a 10 000-character line, the stylesheet's rules and the two chat scenes. |
| `chat-bars.test.cjs` | The chat's one bottom bar (`chat.js`, the bar part of `panel.css`): its precedence (request, terminal dialog, no route, composer; nothing until `message_route` answers), the approval bar (request, diff, Always caption), the plan bar, the question form (B_MovedChatSettingsTests' single and multi select, Other, every question needed, "Sent to Claude"), each answer's exact shape against `calls.json`, the AnswerGate on the chat's bars with the fake clock, THE KEYBOARD GATE on the composer and the Other field (read-only and "Click to type", `panel_take_focus`, editable only after `an:panel_focus` true, shut again without losing text, nothing sent while shut), `message_route` (asked on entry and when `can_message` changes), `send_message`'s outcomes and their copy, drafts per session in memory and `localStorage` (and a storage that throws), hostile strings through every field a bar draws, the stylesheet's rules and the six bar scenes. |
| `settings.test.cjs` | The Claude Code pane of `settings.html` (`settings.js`, `settings.css`), first half: the page-order run (upstream's script then ours, one context, the tab first), a load that asks only for `settings`, the sections' order and titles, the sealed and failure lines, morphing that keeps a half-typed name, `an:cloud`, the run-time rebrand (title, sidebar, every tab, `ui()` wrapped, the walk alone, the observer, `[data-user]` untouched, no loop), the consent card (every file, Turn on once and only from its own click, never on Enter, the sealed answer), the scope notice, Accounts (rows, Parallel Profiles, folders, every account action's exact call, Copy launch command, Add folder, New account), Hooks and status line (switches and their locks, the Claude Code path field, Remove Codenotch's hooks), hostile strings and the five settings scenes. |
| `settings-sections.test.cjs` | The pane's second half (`settings-sections.js`, loaded by `settings.js`): Usage (interval, the engine's captions, the Desktop cache format line, account lines, Refresh now), every setting's exact `set_setting` key and value held against `contract.SETTINGS`, a clicked value shown at once and held 2 s, a refused one put back with its message, sealed answers, Cloud (CloudSettingsTests' copy, each auth state, the sync and summaries switches only from their own click and never optimistic, Sign out asked first, links through `cloud_url` then `open_url`), Sessions and attention (captions, the taken shortcut, type replies off), Notifications (the Windows permission row), Advanced (Copy, the review-queue reset), hostile strings and the four `settings-cloud-*` scenes. |
| `static-audit.test.cjs` | Source audits over every file in `ui/agentnotch`, no page: one strict IIFE and one global per script, no top-level `let`/`const`/`class`, no `eval`, `new Function`, string timer, inline `on*=` or `javascript:` URL, no `fetch`/`XMLHttpRequest`/`WebSocket`/`window.open`, none of the token strings, every `innerHTML` use accounted for, every storage access inside a `try`, and panel.html's CSP meta equal to `tauri.conf.json`'s. Each scan is run on a bad source first. |
| `scenes.test.cjs` | `scenes.json` and `tools/compare-png.cjs`, no browser: the list is the porting notes' section 10 plus the notch scenes and the panel chrome (and nothing else), each entry names its page, global, scene, size and PNG file; every scene shows in the harness (its own request, events, replies and width) with no page error and no refused call, `showScene` answers `false` for an unknown name on all three pages, `layoutReport()` answers `{ok, failures}` on every global, and the report fails what a browser would measure (clipped text, a control outside the card, a tail in a corner); the PNG tool's decoder and encoder, tolerance, budget, size mismatch, diff image, directory mode and exit codes (generated images). |
| `snapshots.test.cjs` | Skipped unless `AGENTNOTCH_UI_BROWSER` names a headless Chromium. Then: `tools/render-scenes.cjs` over every scene, both widths, dark and light, at device scale 1, 1.25 and 1.5 (six runs, about four minutes); fails on a page error, a CSP violation, a refused call, a `showScene` that answers `false`, a failing `layoutReport()` or a PNG that is missing or the wrong width. `AGENTNOTCH_UI_SNAPSHOTS=<folder>` keeps the PNGs in `<folder>/<theme>-<scale>/`. |

## `lib/`

| File | What it is |
|---|---|
| `dom.cjs` | A dependency-free DOM: HTML parser (a real one for the cases that matter: entities, raw-text elements, void elements, SVG, `<template>`), selectors, events with bubbling, focus, form state, `MutationObserver`, a style proxy. No layout: rectangles are `el.__rect` or zero. |
| `harness.cjs` | `loadPage(file, options)` runs a real page (`notch.html`, `settings.html`, `agentnotch/panel.html`) with its scripts in document order in **one** `vm` context (a global clash between upstream's script and ours shows here), a fake `window.__TAURI__` answering `an_call` from the ui-contract fixtures and every other command from `upstreamAnswers()`, and a hand-moved clock (`createClock`, `page.tick(ms)`). `page.hub.calls` records every call; `page.hub.violations` holds those the contract or the window gate would refuse. Read its header for the options. |
| `contract.cjs` | The `an_call` table (`ENGINE`, `GLUE`, `SETTINGS`, `allowedFromWindow`, `callProblem`) the harness and the snapshot tool hold every call against, and `fixture(name)` (a fresh parse of a ui-contract file). |
| `audit.cjs` | `HOSTILE` (strings that become markup when unescaped, a 10 000-character name, U+202E) and `problems(markup)`: what in rendered markup is an injection (unknown tags, `on*` attributes, `javascript:` in a URL attribute, anything that loads or links, an image that is not inline data). Every renderer is fed `HOSTILE` and must leave `problems()` empty. |

## Snapshots

`tools/render-scenes.cjs` renders pages in a real headless Chromium so you can *look* at them
(layout, colour, copy; the PNGs use a fallback font because Segoe and Cascadia are absent on a
Mac). Baselines are a different thing: see `baselines/README.md`.

```sh
node agentnotch-ui-tests/tools/render-scenes.cjs \
  --browser <path to headless_shell> --out <dir> \
  [--only <name prefix>] [--widths 400,520] [--theme dark|light] [--dpr 2] \
  [--backdrop '#808080'] [--scenes <file>] [--no-layout-fail] [--verbose] [--list]
# or one page without a scene entry:
node agentnotch-ui-tests/tools/render-scenes.cjs --browser … --out … \
  --page settings.html [--global agentnotch] [--scene notch-card] [--edge left] --width 680 --height 520
```

- The browser is Chromium's headless shell (`--browser`, or `AGENTNOTCH_BROWSER`). On this Mac:
  `<scratch>/work-wp10/pw/shell/chrome-mac/headless_shell` (the way to get it again is in the
  package brief, section 7).
- It serves `codenotch/ui` over `http://127.0.0.1:<port>` (never `file://`), with the app's CSP
  from `tauri.conf.json` plus the hashes of a page's inline scripts (what Tauri adds for
  upstream's pages), injects the fake `__TAURI__` before the first script, freezes the clock at
  the fixtures' `generated_at_ms`, loads the page at the scene's viewport, theme (`prefers-color-scheme`
  and upstream's `get_theme_resolved`) and edge (`get_notch_edge`), emits the scene's events,
  calls `window[<global>].showScene(<scene>)`, waits for the fonts and two frames, and writes
  `<name>[-<width>][-light].png` (transparent background kept, as the app's windows are; a PNG
  viewer may show it on white or black, use `--backdrop` for a solid one) and `.json` (the scene,
  the page's `layoutReport()`, the problems, the `an_call` methods used).
- Exit 1: a page error, `console.error`, a resource that 404s, a CSP violation, a call the
  contract refuses, `showScene` returning false, or a failing `layoutReport()`
  (`--no-layout-fail` reports it without failing). Exit 2: bad arguments or no browser.
- A page whose window follows its content marks that element `data-an-fit` (the panel's `#an-panel`: card plus tail); `height: "fit"` measures its bottom edge instead of the scroll height. The notice that `frame-ancestors` is ignored in a `<meta>` CSP (panel.html) is not a failure: only a header can set it, and the app's header does.
- Output lines start `ok` or `FAIL`. The tool leaves no process behind.

### `scenes.json`

`{ "scenes": [ … ] }`: the porting notes' inventory (`docs/design/windows-port-notes/ui.md`, section 10, with `settings-full` named
`settings`), the notch per edge (`notch-<edge>-open|card|folded`), the panel chrome per placement (`panel-chrome-<edge|floating>`) and
three panel states of ours (`panel-menu`, `panel-single-account`, `panel-pinned`). `scenes.test.cjs` holds the list to exactly that. A scene's fields:

| Field | Meaning |
|---|---|
| `name` | Unique; also the file name (`--only` matches its start). |
| `file` | The PNG the app's snapshot run writes (`AGENTNOTCH_SNAPSHOT_CLAUDE`) and the smoke test looks up in `baselines/`: `<name>.png`, or `<name>-{width}.png` (width 400 or 520) for a scene with `widths: true`. The light theme adds `-light` before `.png`. |
| `mac` | When the Mac has a sheet of the same name (`Scripts/spm-snapshots.sh`), its base name. |
| `page` | File under `codenotch/ui` (`notch.html`, `settings.html`, `agentnotch/panel.html`). |
| `global`, `scene` | The page's global (`agentnotch`, `agentnotchPanel`, `agentnotchSettings`) and the name passed to its `showScene`; `layoutReport()` of the same global is read after. |
| `width`, `height` | Viewport in CSS px. Or `widths: true` (rendered once per `--widths`, default 400 and 520, the file gets `-<width>`), and `height: "fit"` (the page's `[data-an-fit]` bottom or scroll height, at most `maxHeight`, default 800). |
| `edge` | `right` (default), `left`, `top`, `bottom`: what upstream's `get_notch_edge` answers. |
| `panel` | The `PanelRequest` set as `window.__AGENTNOTCH_PANEL__` before the page's scripts (with the optional `placement`). |
| `events` | `[{event, fixture}]` or `[{event, payload}]`, emitted in order after the page has loaded and gone quiet (`an:snapshot`, `an:chat`, …). |
| `replies` | `{method: reply}` overriding the fake's answer to an `an_call` method. |
| `wait` | Extra milliseconds before the screenshot. |

Notch windows are 360×650 on the left and right edges and 650×650 on the top and bottom
(`notch_window_size` in `main.rs`). The settings window is 680 wide. The chat scenes are scenes of the panel page
(`agentnotchPanel.showScene('chat-…')` with the route in the request); the chat's own state (an "Other" answer, a draft) is set by the scene.

| Scene | Page | Hook | Viewport | File |
|---|---|---|---|---|
| `notch-right-open` | `notch.html` | `agentnotch.showScene('notch-open')` | 360x650 | `notch-right-open.png` |
| `notch-right-card` | `notch.html` | `agentnotch.showScene('notch-card')` | 360x650 | `notch-right-card.png` |
| `notch-right-folded` | `notch.html` | `agentnotch.showScene('notch-folded')` | 360x650 | `notch-right-folded.png` |
| `notch-left-open` | `notch.html` | `agentnotch.showScene('notch-open')` | 360x650 | `notch-left-open.png` |
| `notch-left-card` | `notch.html` | `agentnotch.showScene('notch-card')` | 360x650 | `notch-left-card.png` |
| `notch-left-folded` | `notch.html` | `agentnotch.showScene('notch-folded')` | 360x650 | `notch-left-folded.png` |
| `notch-top-open` | `notch.html` | `agentnotch.showScene('notch-open')` | 650x650 | `notch-top-open.png` |
| `notch-top-card` | `notch.html` | `agentnotch.showScene('notch-card')` | 650x650 | `notch-top-card.png` |
| `notch-top-folded` | `notch.html` | `agentnotch.showScene('notch-folded')` | 650x650 | `notch-top-folded.png` |
| `notch-bottom-open` | `notch.html` | `agentnotch.showScene('notch-open')` | 650x650 | `notch-bottom-open.png` |
| `notch-bottom-card` | `notch.html` | `agentnotch.showScene('notch-card')` | 650x650 | `notch-bottom-card.png` |
| `notch-bottom-folded` | `notch.html` | `agentnotch.showScene('notch-folded')` | 650x650 | `notch-bottom-folded.png` |
| `panel-empty` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-empty')` | widths 400, 520 | `panel-empty-{width}.png` |
| `panel-every-state` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-every-state')` | widths 400, 520 | `panel-every-state-{width}.png` |
| `panel-menu` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-menu')` | widths 400, 520 | `panel-menu-{width}.png` |
| `panel-filtered` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-filtered')` | widths 400, 520 | `panel-filtered-{width}.png` |
| `panel-needs-you` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-needs-you')` | widths 400, 520 | `panel-needs-you-{width}.png` |
| `panel-banners` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-banners')` | widths 400, 520 | `panel-banners-{width}.png` |
| `panel-consent` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-consent')` | widths 400, 520 | `panel-consent-{width}.png` |
| `panel-scope-notice` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-scope-notice')` | widths 400, 520 | `panel-scope-notice-{width}.png` |
| `panel-regular-rows` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-regular-rows')` | widths 400, 520 | `panel-regular-rows-{width}.png` |
| `panel-busy-window` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-busy-window')` | widths 400, 520 | `panel-busy-window-{width}.png` |
| `panel-busy-full` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-busy-full')` | widths 400, 520 | `panel-busy-full-{width}.png` |
| `panel-undo` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-undo')` | widths 400, 520 | `panel-undo-{width}.png` |
| `panel-keyboard-folded` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-keyboard-folded')` | widths 400, 520 | `panel-keyboard-folded-{width}.png` |
| `panel-single-account` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-single-account')` | widths 400, 520 | `panel-single-account-{width}.png` |
| `panel-pinned` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-pinned')` | widths 400, 520 | `panel-pinned-{width}.png` |
| `panel-chrome-left` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-every-state')` | 432xfit | `panel-chrome-left.png` |
| `panel-chrome-right` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-every-state')` | 432xfit | `panel-chrome-right.png` |
| `panel-chrome-top` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-every-state')` | 440xfit | `panel-chrome-top.png` |
| `panel-chrome-bottom` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-every-state')` | 440xfit | `panel-chrome-bottom.png` |
| `panel-chrome-floating` | `agentnotch/panel.html` | `agentnotchPanel.showScene('panel-every-state')` | 440xfit | `panel-chrome-floating.png` |
| `chat-approval` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-approval')` | widths 400, 520 | `chat-approval-{width}.png` |
| `chat-tasks` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-tasks')` | widths 400, 520 | `chat-tasks-{width}.png` |
| `chat-plan` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-plan')` | widths 400, 520 | `chat-plan-{width}.png` |
| `chat-question-other` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-question-other')` | widths 400, 520 | `chat-question-other-{width}.png` |
| `chat-composer` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-composer')` | widths 400, 520 | `chat-composer-{width}.png` |
| `chat-terminal-only` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-terminal-only')` | widths 400, 520 | `chat-terminal-only-{width}.png` |
| `chat-no-route` | `agentnotch/panel.html` | `agentnotchPanel.showScene('chat-no-route')` | widths 400, 520 | `chat-no-route-{width}.png` |
| `settings` | `settings.html` | `agentnotchSettings.showScene('settings-full')` | 680x3600 | `settings.png` |
| `settings-first-run` | `settings.html` | `agentnotchSettings.showScene('settings-first-run')` | 680x1500 | `settings-first-run.png` |
| `settings-scope-notice` | `settings.html` | `agentnotchSettings.showScene('settings-scope-notice')` | 680x1500 | `settings-scope-notice.png` |
| `settings-parallel-profiles` | `settings.html` | `agentnotchSettings.showScene('settings-parallel-profiles')` | 680x1900 | `settings-parallel-profiles.png` |
| `settings-unnamed-accounts` | `settings.html` | `agentnotchSettings.showScene('settings-unnamed-accounts')` | 680x1700 | `settings-unnamed-accounts.png` |
| `settings-cloud-signed-out` | `settings.html` | `agentnotchSettings.showScene('settings-cloud-signed-out')` | 680x420 | `settings-cloud-signed-out.png` |
| `settings-cloud-signed-in` | `settings.html` | `agentnotchSettings.showScene('settings-cloud-signed-in')` | 680x900 | `settings-cloud-signed-in.png` |
| `settings-cloud-overridden` | `settings.html` | `agentnotchSettings.showScene('settings-cloud-overridden')` | 680x420 | `settings-cloud-overridden.png` |
| `settings-cloud-no-website` | `settings.html` | `agentnotchSettings.showScene('settings-cloud-no-website')` | 680x420 | `settings-cloud-no-website.png` |

### The page test hooks (what WP9's sealed self-test and snapshot run call)

Each page has one global; the names and shapes are fixed.

| Page | Global | `showScene(name)` | `layoutReport()` |
|---|---|---|---|
| `notch.html` | `window.agentnotch` | `notch-open`, `notch-card` (the first Claude ring's hover card), `notch-folded` | badges inside the pill and clear of the percent label (flat edges), card rows inside the card, the card inside the window and not scrolling, resting marks inside the folded pill; plus `card {height, rows, parts}` |
| `agentnotch/panel.html` | `window.agentnotchPanel` (the chat is drawn by `window.agentnotchChat`, whose `layoutReport()` is the panel's) | every `panel-*` and `chat-*` scene of the list; the three `panel-chrome-*` placement scenes show `panel-every-state` with a `placement` in the request | no `[data-an-text]` wider than its box (a title cut on purpose carries `data-an-clip` and an ellipsis), every control, link and field inside the card (a scrolling list or transcript may run below the fold, never beside it), the card and the tail inside the window, the tail meeting the card, the tail's base clear of the card's rounded corners (16 px); the chat's own problems in a chat |
| `settings.html` | `window.agentnotchSettings` | `settings-*` (the Claude Code pane) | no `[data-an-text]` wider than its box, every control inside the pane, no sideways scroll |

`showScene` sets the static mode (`agentnotchCommon.setStatic(true)`: no motion, the clock pinned), shows the state from the snapshot the page
already holds, sends nothing to the engine and returns `true`; an unknown name returns `false` and changes nothing. `layoutReport()` returns
`{ok, failures: [string], …}` and `ok` is `failures.length === 0`. The self-test asks for it at 400 and 520 px on the panel page and at the
scale the run uses (100, 125, 150 %); a failure is a failure of the run.

What the pages expect from the glue (also in the package log):

- `window.__AGENTNOTCH_PANEL__` and the `an:panel` event carry the `PanelRequest` `{route, ring_id, highlight, reason}` and, for the tail, an optional
  `placement {kind: 'beside' | 'above_or_below' | 'floating', tail_edge: 'left' | 'right' | 'top' | 'bottom', tail_offset, width}`: `tail_edge` is the
  card side the tail is on (the way it points), `tail_offset` is px from the card's centre along that side, `width` is the card's width. Missing or
  invalid is a floating card with no tail.
- `an:panel_state {open, ring_id}` to the notch window on every open and close of the panel (the notch holds itself open and hides its card).
- `an:panel_focus {focused}` to the panel: the keyboard gate stays shut (every key dropped, every field read-only with "Click to type") until a
  `focused: true` arrives.
- The panel reports its size with `panel_report_size {w, h}`: the card's size; the glue adds the tail strip (32 px on the notch side).

### `tools/compare-png.cjs` (for WP11's smoke test)

```sh
node agentnotch-ui-tests/tools/compare-png.cjs <expected.png|dir> <actual.png|dir> [--tolerance 8] [--budget 0.002] [--diff <out.png|dir>] [--json]
```

A pixel is changed when one of its four channels differs by more than `--tolerance` (0 to 255, default 8: anti-aliasing and font smoothing);
the images match while the changed pixels are at most `--budget` of all pixels (a fraction, default 0.002) and have the same size. With two folders it
compares the PNGs of the same name; a file on one side only fails (`no capture` / `no baseline`). Exit 0: within the budget; 1: beyond it, a
different size or a missing file; 2: bad arguments or a file that is not a PNG it reads (8 and 16 bit, any colour type, not interlaced). The diff
image (the actual image faded, changed pixels red) is written whenever a pixel changed; `--json` prints one object per image
(`ok, changed, pixels, fraction, maxDelta, sizeMismatch, diffFile`). From node: `require('./tools/compare-png.cjs')` exports
`compare(expected, actual, options)`, `compareFiles(a, b, options)`, `decode(buffer)` and `encode({width, height, data})`.

## Hostile strings

Session titles, transcript and tool text, account names, emails, paths, notices and error messages
are untrusted. A renderer draws them with `textContent` or through `agentnotchCommon.esc` /
`agentnotchMarkdown.render`; a test feeds each renderer `audit.HOSTILE` and asserts
`audit.problems(rendered)` is empty (and, with the raw string, that it is not: the check must be
able to fail).
