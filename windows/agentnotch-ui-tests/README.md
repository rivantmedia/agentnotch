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

`{ "scenes": [ … ] }`; every scene of the porting notes' inventory (`docs/design/windows-port-notes/ui.md`, lines 1031-1064) goes here, named
lower-case with dashes. A scene's fields:

| Field | Meaning |
|---|---|
| `name` | Unique; also the file name (`--only` matches its start). |
| `page` | File under `codenotch/ui` (`notch.html`, `settings.html`, `agentnotch/panel.html`). |
| `global`, `scene` | The page's global (`agentnotch`, `agentnotchPanel`, `agentnotchChat`, `agentnotchSettings`) and the name passed to its `showScene`; `layoutReport()` of the same global is read after. Both may be left out for a page without them. |
| `width`, `height` | Viewport in CSS px. Or `widths: true` (rendered once per `--widths`, default 400 and 520, the file gets `-<width>`), and `height: "fit"` (the page's scroll height, at most `maxHeight`, default 800). |
| `edge` | `right` (default), `left`, `top`, `bottom`: what upstream's `get_notch_edge` answers. |
| `panel` | The `PanelRequest` set as `window.__AGENTNOTCH_PANEL__` before the page's scripts (with the optional `placement`). |
| `events` | `[{event, fixture}]` or `[{event, payload}]`, emitted in order after the page has loaded and gone quiet (`an:snapshot`, `an:chat`, …). |
| `replies` | `{method: reply}` overriding the fake's answer to an `an_call` method. |
| `wait` | Extra milliseconds before the screenshot. |

Notch windows are 360×650 on the left and right edges and 650×650 on the top and bottom
(`notch_window_size` in `main.rs`).

### The contract of `showScene` and `layoutReport`

WP9's sealed self-test calls exactly these on each page's global, so their names and shapes are
fixed. `showScene(name)` sets the static mode (`agentnotchCommon.setStatic(true)`) and shows that
state from the snapshot the page already holds; it returns `false` for a name it does not know.
`layoutReport()` returns `{ok, failures: [string], …}`: the page's layout invariants, measured in
the browser (badges inside the pill and clear of the percent label on the flat edges, no
`[data-an-text]` element wider than its box at 400 and 520 px, controls inside their container).
`dom.cjs` has no layout, so anything about real geometry is asserted here, not in the node tests.

## Hostile strings

Session titles, transcript and tool text, account names, emails, paths, notices and error messages
are untrusted. A renderer draws them with `textContent` or through `agentnotchCommon.esc` /
`agentnotchMarkdown.render`; a test feeds each renderer `audit.HOSTILE` and asserts
`audit.problems(rendered)` is empty (and, with the raw string, that it is not: the check must be
able to fail).
