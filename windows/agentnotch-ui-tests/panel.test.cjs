'use strict';
// ui/agentnotch/panel.html, panel.js, panel.css and theme.css: the shell of the sessions panel.
// The real page (its own markup, its five scripts in document order in one vm context), a fake
// bridge that holds every call against the contract (lib/contract.cjs) and a hand-moved clock.
// What node cannot know is layout; the snapshot tool (tools/render-scenes.cjs, the panel-*
// scenes) measures that in a real browser through layoutReport(), and here only what a test
// sets with `__rect` is checked.
//
// Swift tests ported: B_PanelStateTests (routes, back and close, a highlighted row is selected,
// the list route naming its filter, the ring filter; the height clamp is the glue's, the folds,
// undo and drafts belong to the list and chat sub-tasks). The rest is new: the CSP rules, the
// tokens, the header from the fixture, the gear menu and what it calls, pin, close and Esc order,
// the size report, engagement, accelerators, the tail per placement, hostile strings, a snapshot
// re-render that keeps an open menu, the sealed scenes and the keyboard gate's state.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');

const DIR = path.join(harness.UI, 'agentnotch');
const HTML = fs.readFileSync(path.join(DIR, 'panel.html'), 'utf8');
const CSS = fs.readFileSync(path.join(DIR, 'panel.css'), 'utf8');
const THEME = fs.readFileSync(path.join(DIR, 'theme.css'), 'utf8');
const PERSONAL = 'claude-acct-1e41d94e802a';
const WORK = 'claude-acct-5688209c6cfb';
const POLICY = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

async function load(options) {
  const page = harness.loadPage('agentnotch/panel.html', options);
  await page.settle();
  return page;
}

/** A fresh copy of the ui-contract snapshot, edited by `edit`. */
function snapshotWith(edit) {
  const snapshot = harness.fixture('snapshot.json');
  if (edit) edit(snapshot);
  return snapshot;
}

/** The page with the glue's first request on the window (`window.__AGENTNOTCH_PANEL__`). */
async function open(request, edit, options) {
  return load(Object.assign({
    snapshot: snapshotWith(edit),
    before: (window) => { window.__AGENTNOTCH_PANEL__ = request; },
  }, options));
}

/** The glue confirms the panel has the keyboard (an:panel_focus): only then do keys act, Esc included. */
function keyboard(page, on) {
  page.emit('an:panel_focus', { focused: on !== false });
}

/** No page error and no call the contract or the window gate would refuse. */
function clean(page) {
  assert.deepEqual(page.errors.map(String), []);
  assert.deepEqual(page.hub.violations, []);
}

const state = (page) => plain(page.run('agentnotchPanel._.state'));
const calls = (page, method) => page.hub.of(method).map((c) => plain(c.args));
const strip = (page) => page.$$('.an-strip-item').map((el) => el.textContent.trim());
/** The parts of an element's text, apart: layout puts the space between them, the markup does not. */
const parts = (el) => el.children.map((c) => c.textContent.replace(/\s+/g, ' ').trim()).filter(Boolean).join(' ');
const chips = (page) => page.$$('.an-chip').map(parts);
const gear = (page) => page.$('[data-an-action="gear"]');
const menuText = (page) => page.$$('.an-mi').map(parts);
const checked = (page) => page.$$('.an-mi[aria-checked="true"]').map((el) => el.getAttribute('data-an-arg'));

/** `key` is one of the entries the gear menu's radio group and switches use. */
const item = (page, action, arg) => page.$$('.an-mi').find((el) => el.getAttribute('data-an-action') === action && (arg === undefined || el.getAttribute('data-an-arg') === arg));

function rect(el, width, height) {
  el.__rect = { left: 0, top: 0, right: width, bottom: height, width, height };
}

/** Lets the natural height be measured: the header block, the list's content and the card's width. */
function size(page, opts) {
  rect(page.$('#an-card'), opts.width || 440, 0);
  rect(page.$('#an-top'), opts.width || 440, opts.top);
  rect(page.$('#an-hair'), opts.width || 440, 1);
  const list = page.$('#an-list');
  rect(list, opts.width || 440, 200);
  list.__scrollHeight = opts.list;
  page.resizeObservers.forEach((o) => o.callback([]));
}

// ---- the markup and the stylesheets --------------------------------------------------------------

test('panel.html keeps the CSP meta and loads nothing but its own files, with no inline script or handler', () => {
  assert.equal(HTML.split('Content-Security-Policy').length - 1, 1, 'one CSP meta');
  assert.ok(HTML.includes(`content="${POLICY}"`), 'the policy text is the app-wide one, unchanged');
  const dom = require('./lib/dom.cjs');
  const document = dom.parseDocument(HTML);
  const scripts = document.querySelectorAll('script');
  assert.deepEqual(scripts.map((s) => s.getAttribute('src')), ['common.js', 'markdown.js', 'toolresults.js', 'chat.js', 'panel-list.js', 'panel.js']);
  for (const script of scripts) assert.equal(script.textContent.trim(), '', 'no inline script');
  assert.deepEqual(document.querySelectorAll('link[rel="stylesheet"]').map((l) => l.getAttribute('href')), ['theme.css', 'panel.css']);
  for (const el of dom.elementsOf(document)) {
    for (const { name } of el.attributes) assert.ok(!/^on/i.test(name), `<${el.localName} ${name}>`);
  }
  assert.ok(!/javascript:/i.test(HTML));
  assert.ok(!/<(iframe|object|embed|base|form)\b/i.test(HTML));
  assert.ok(!/https?:\/\//i.test(HTML.replace(POLICY, '')), 'nothing is fetched from the network');
});

/** The custom properties of one block of theme.css. */
function tokens(selector) {
  const start = THEME.indexOf(selector + ' {');
  assert.ok(start >= 0, `${selector} block`);
  const body = THEME.slice(THEME.indexOf('{', start) + 1, THEME.indexOf('\n}', start));
  const out = {};
  for (const m of body.matchAll(/(--an-[a-z0-9-]+)\s*:\s*([^;]+);/g)) out[m[1]] = m[2].trim();
  return out;
}

test('theme.css: every token of UI§1 exists for dark and for light, with the Mac\'s values', () => {
  const dark = tokens(':root');
  const light = tokens(':root[data-theme="light"]');
  const themed = ['--an-text', '--an-text-2', '--an-needs-you', '--an-review', '--an-working', '--an-critical',
    '--an-track', '--an-bar-track', '--an-accent', '--an-card', '--an-text-3', '--an-fill', '--an-fill-hover',
    '--an-row-hover', '--an-row-sel', '--an-row-sel-stroke', '--an-sep', '--an-primary', '--an-on-primary',
    '--an-accent-fill', '--an-menu', '--an-shadow', ...[0, 1, 2, 3, 4, 5, 6, 7].map((i) => `--an-hue-${i}`)];
  for (const name of themed) {
    assert.ok(name in dark, `dark ${name}`);
    assert.ok(name in light, `light ${name}`);
  }
  // UI§1.1
  assert.deepEqual([dark['--an-text'], dark['--an-text-2'], dark['--an-needs-you'], dark['--an-review'], dark['--an-critical'], dark['--an-card']],
    ['#ffffff', '#808080', '#f2ff00', '#00ff88', '#ff3f00', '#000000']);
  assert.deepEqual([light['--an-text'], light['--an-text-2'], light['--an-needs-you'], light['--an-review'], light['--an-critical']],
    ['#000000', '#6b6b6b', '#b08800', '#00a356', '#ff3f00']);
  assert.equal(dark['--an-track'], 'rgba(255, 255, 255, .188)');
  assert.equal(light['--an-track'], 'rgba(0, 0, 0, .16)');
  assert.equal(dark['--an-bar-track'], 'rgba(255, 255, 255, .176)');
  assert.equal(light['--an-bar-track'], 'rgba(0, 0, 0, .15)');
  assert.equal(dark['--an-working'], dark['--an-text']);
  assert.equal(light['--an-working'], light['--an-text']);
  // UI§1.2: the same alpha of the text colour, over white in dark and black in light
  for (const [name, alpha] of [['--an-text-3', '.30'], ['--an-fill', '.09'], ['--an-fill-hover', '.16'], ['--an-row-hover', '.05'],
    ['--an-row-sel', '.08'], ['--an-row-sel-stroke', '.24'], ['--an-primary', '.95']]) {
    assert.equal(dark[name], `rgba(255, 255, 255, ${alpha})`, `dark ${name}`);
    assert.equal(light[name], `rgba(0, 0, 0, ${alpha})`, `light ${name}`);
  }
  assert.equal(dark['--an-sep'], 'var(--an-track)');
  assert.equal(dark['--an-on-primary'], '#000000');
  assert.equal(light['--an-on-primary'], '#ffffff');
  // UI§1.3
  assert.deepEqual([0, 1, 2, 3, 4, 5, 6, 7].map((i) => dark[`--an-hue-${i}`].toUpperCase()),
    ['#5C9EFA', '#B885F5', '#F573B3', '#40CCCC', '#858FFF', '#66D1F5', '#FF949E', '#D6C29E']);
  assert.deepEqual([0, 1, 2, 3, 4, 5, 6, 7].map((i) => light[`--an-hue-${i}`]), [0, 1, 2, 3, 4, 5, 6, 7].map((i) => dark[`--an-hue-${i}`]));
  const common = require('./lib/scripts.cjs').load(['common']).window.agentnotchCommon;
  assert.deepEqual(plain(common.ACCOUNT_HUES.map((h) => h.toLowerCase())), [0, 1, 2, 3, 4, 5, 6, 7].map((i) => dark[`--an-hue-${i}`]), 'common.js and theme.css agree');
});

test('theme.css: the type scale, the metrics and the motion of UI§1.4-1.6', () => {
  const t = tokens(':root');
  assert.match(t['--an-font'], /^"Segoe UI Variable Text", "Segoe UI"/);
  assert.match(t['--an-mono'], /^"Cascadia Mono", Consolas/);
  assert.equal(t['--an-ui-scale'], '1.15');
  const px = (name) => parseFloat(t[name]);
  assert.ok(px('--an-fs-title') >= 15 && px('--an-fs-title') <= 16);
  assert.deepEqual([px('--an-fs-chat'), px('--an-fs-row-title'), px('--an-fs-body'), px('--an-fs-caption'), px('--an-fs-mono')], [14, 13, 12, 11, 12]);
  assert.deepEqual([t['--an-fw-title'], t['--an-fw-row-title']], ['600', '500']);
  assert.deepEqual([px('--an-corner'), px('--an-pad'), px('--an-row-corner'), px('--an-control-h'), px('--an-chip-h'), px('--an-block'), px('--an-line-gap'), px('--an-bar-h')],
    [16, 12, 10, 20, 18, 7.5, 3.8, 3.9]);
  assert.deepEqual([px('--an-tail-len'), px('--an-tail-w')], [32, 36]);
  assert.equal(t['--an-glide'], '320ms cubic-bezier(.2, .8, .2, 1)');
  // the working arc turns in steps, the breath is finite, nothing loops smoothly
  assert.match(THEME, /\.an-spin \{ animation: an-spin 1\.4s steps\(12\) infinite/);
  assert.match(THEME, /\.an-breathe \{ animation: an-breathe \.9s ease-in-out 7 alternate both/);
  assert.equal((THEME.match(/infinite/g) || []).length, 1, 'the one infinite animation is the stepped arc');
  // Windows' "Animation effects" off and the static scenes both still every animation and transition
  const reduced = THEME.slice(THEME.indexOf('@media (prefers-reduced-motion: reduce)'));
  assert.match(reduced, /animation: none !important; transition: none !important/);
  assert.match(THEME, /html\.an-static \*, html\.an-static \*::before, html\.an-static \*::after \{\s*animation: none !important; transition: none !important/);
});

test('panel.css: transparent page, a solid card, a tail per side, and colour only from tokens', () => {
  assert.match(CSS, /html, body \{ margin: 0; background: transparent/);
  assert.match(CSS, /\.an-card \{[^}]*background: var\(--an-card\); border-radius: var\(--an-corner\)/);
  for (const side of ['left', 'right', 'top', 'bottom']) {
    assert.match(CSS, new RegExp(`\\.an-panel\\[data-tail="${side}"\\] \\.an-tail \\{[^}]*clip-path: path\\(`), `${side} tail`);
    assert.match(CSS, new RegExp(`\\.an-panel\\[data-tail="${side}"\\] \\{ padding-`), `${side} strip`);
  }
  assert.match(CSS, /\.an-panel:not\(\[data-tail="none"\]\) \.an-tail \{ display: block; \}/, 'no tail when floating');
  assert.match(CSS, /\.an-tail \{[^}]*background: var\(--an-card\)/, 'the tail is the card\'s own colour');
  const bare = CSS.replace(/\/\*[\s\S]*?\*\//g, '');
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(bare), 'no hex colour');
  assert.ok(!/\brgba?\(/.test(bare), 'no rgb() colour');
  assert.ok(!/@keyframes/.test(bare), 'keyframes live in theme.css');
});

// ---- loading and priming ---------------------------------------------------------------------------

test('the page loads its five scripts in order with no error, and a load only asks for the snapshot', async () => {
  const page = await load();
  clean(page);
  assert.deepEqual(page.loaded, ['common.js', 'markdown.js', 'toolresults.js', 'chat.js', 'panel-list.js', 'panel.js']);
  for (const name of ['agentnotchCommon', 'agentnotchMarkdown', 'agentnotchToolResults', 'agentnotchChat', 'agentnotchPanel']) {
    assert.equal(typeof page.window[name], 'object', name);
  }
  assert.deepEqual(page.hub.calls.map((c) => c.method), ['snapshot'], 'no consent, no setting, nothing else on load');
  for (const name of ['an:snapshot', 'an:panel', 'an:panel_focus']) assert.ok(page.listening(name), name);
  assert.equal(page.text('#an-header .an-title'), 'Claude sessions');
});

test('the theme comes from Rust when the page has none, and follows theme_resolved', async () => {
  const page = await load({ theme: 'light' });
  assert.equal(page.document.documentElement.getAttribute('data-theme'), 'light');
  page.emit('theme_resolved', 'dark');
  assert.equal(page.document.documentElement.getAttribute('data-theme'), 'dark');
  page.emit('theme_resolved', 'sepia');
  assert.equal(page.document.documentElement.getAttribute('data-theme'), 'dark', 'anything but light or dark is ignored');
  const set = await load({ theme: 'light', before: (w) => { w.document.documentElement.setAttribute('data-theme', 'dark'); } });
  assert.equal(set.upstreamCalls.filter((c) => c.cmd === 'get_theme_resolved').length, 0, 'the glue already set it');
  clean(page);
});

test('the first request on the window sets the route, the filter, the highlight, the reason and the placement', async () => {
  // (a row of the filtered account: a selection the list does not show is dropped)
  const page = await open({ route: 'sessions', ring_id: WORK, highlight: 'needs-permission', reason: 'notification',
    placement: { kind: 'beside', tail_edge: 'right', tail_offset: 40, width: 400 } });
  clean(page);
  const s = state(page);
  assert.equal(s.route, 'sessions');
  assert.equal(s.filter, WORK);
  assert.equal(s.highlight, 'needs-permission');
  assert.equal(s.selected, 'needs-permission');
  assert.equal(s.reason, 'notification');
  assert.deepEqual(s.placement, { kind: 'beside', tail: 'right', offset: 40, width: 400 });
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'right');
});

test('a later an:panel replaces the request; a list route names its filter every time', async () => {
  const page = await open({ route: 'sessions', ring_id: WORK, reason: 'ring_click' });
  page.hub.clear();
  page.emit('an:panel', { route: 'sessions', ring_id: null, highlight: 'idle-notch', reason: 'notification' });
  assert.equal(state(page).filter, null, 'B_PanelStateTests: re-opening all sessions over a narrowed list drops the filter');
  assert.equal(state(page).highlight, 'idle-notch');
  assert.equal(state(page).selected, 'idle-notch');
  page.emit('an:panel', { route: 'sessions', ring_id: PERSONAL, reason: 'ring_click' });
  assert.equal(state(page).filter, PERSONAL);
  assert.equal(state(page).highlight, null);
  page.emit('an:panel', { route: 'session:needs-plan', reason: 'hover_row', placement: { kind: 'floating' } });
  assert.equal(state(page).route, 'session:needs-plan');
  assert.equal(state(page).filter, PERSONAL, 'a chat keeps the list\'s filter for the way back');
  assert.equal(page.$('#an-card').getAttribute('data-mode'), 'chat');
  assert.deepEqual(page.hub.of('panel_route'), [], 'the glue knows what it sent: the page does not echo a route back');
  clean(page);
});

test('a request that says nothing is a floating list; junk requests and snapshots are ignored', async () => {
  const page = await open(undefined);
  assert.equal(state(page).route, 'sessions');
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'none');
  page.emit('an:panel', null);
  page.emit('an:panel', 'sessions');
  page.emit('an:snapshot', null);
  page.emit('an:snapshot', 'nope');
  assert.equal(state(page).route, 'sessions');
  assert.equal(page.text('#an-header .an-title'), 'Claude sessions');
  clean(page);
});

test('an older snapshot never replaces a newer one', async () => {
  const page = await open({ route: 'sessions' });
  const older = snapshotWith((s) => { s.generated_at_ms -= 1000; s.sealed = false; });
  page.emit('an:snapshot', older);
  assert.ok(page.$('.an-sealed'), 'the newer snapshot stands');
  const newer = snapshotWith((s) => { s.generated_at_ms += 1000; s.sealed = false; });
  page.emit('an:snapshot', newer);
  assert.equal(page.$('.an-sealed'), null);
});

// ---- the header ----------------------------------------------------------------------------------------

test('the header from the fixture: title, Sealed badge, three buttons, the attention strip', async () => {
  const page = await open({ route: 'sessions' });
  clean(page);
  assert.equal(page.text('.an-title'), 'Claude sessions');
  assert.equal(page.$('.an-title').tagName, 'H1');
  assert.equal(page.text('.an-sealed'), 'Sealed');
  assert.equal(page.$('.an-sealed').getAttribute('aria-label'), 'Sealed mode: showing fixture data');
  const buttons = page.$$('.an-icbtn');
  assert.deepEqual(buttons.map((b) => b.getAttribute('aria-label')), ['Keep open', 'Settings', 'Close']);
  assert.deepEqual(buttons.map((b) => b.getAttribute('title')), ['Keep open', 'Settings', 'Close']);
  // fixture totals: 4 answerable, 1 failed, 3 review, 3 working, 4 idle
  assert.deepEqual(strip(page), ['4 need you', '1 failed', '3 to review', '3 working', '4 idle']);
  assert.equal(page.$('.an-strip').getAttribute('aria-label'), '4 need you, 1 failed, 3 ready for review, 3 working, 4 idle');
  assert.deepEqual(page.$$('.an-strip-item svg').map((s) => s.getAttribute('class').split(' ')[1]),
    ['an-ring-needs', 'an-ring-error', 'an-ring-review', 'an-ring-working', 'an-ring-idle']);
  assert.deepEqual(page.$$('.an-strip-item').map((s) => s.getAttribute('class').split(' ')[1]),
    ['an-tone-needs', 'an-tone-error', 'an-tone-review', 'an-tone-working', 'an-tone-idle']);
});

test('the strip lists only the groups that have sessions, and says "1 needs you" for one', async () => {
  const page = await open({ route: 'sessions' }, (s) => { s.totals = { needs_you: 1, failed: 0, review: 0, working: 2, idle: 0 }; });
  assert.deepEqual(strip(page), ['1 needs you', '2 working']);
  assert.equal(page.$('.an-strip').getAttribute('aria-label'), '1 needs you, 2 working');
  const none = await open({ route: 'sessions' }, (s) => { s.totals = { needs_you: 0, failed: 0, review: 0, working: 0, idle: 0 }; });
  assert.equal(none.$('.an-strip'), null, 'nothing to count, no strip');
});

test('the Sealed badge shows only for a sealed snapshot', async () => {
  const live = await open({ route: 'sessions' }, (s) => { s.sealed = false; });
  assert.equal(live.$('.an-sealed'), null);
  live.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 5; s.sealed = true; }));
  assert.equal(live.text('.an-sealed'), 'Sealed');
});

test('the account chips: All and one per account, with counts and the needs-you count beside them', async () => {
  const page = await open({ route: 'sessions' });
  assert.deepEqual(chips(page), ['All 15 4', 'Personal 8 3', 'Work 7 1']);
  assert.deepEqual(page.$$('.an-chip').map((c) => c.getAttribute('aria-label')),
    ['All, 15 sessions, 4 need you', 'Personal, 8 sessions, 3 need you', 'Work, 7 sessions, 1 needs you']);
  assert.deepEqual(page.$$('.an-chip').map((c) => c.getAttribute('aria-pressed')), ['true', 'false', 'false']);
  assert.deepEqual(page.$$('.an-chip-need').map((c) => c.getAttribute('aria-hidden')), ['true', 'true', 'true']);
  assert.equal(page.$$('.an-chip .an-dot').length, 2, 'the All chip has no account dot');
  assert.deepEqual(page.$$('.an-chip .an-dot').map((d) => d.style.background), ['#5C9EFA', '#40CCCC']);
});

test('a chip with nothing needing you shows no amber count', async () => {
  const page = await open({ route: 'sessions' }, (s) => {
    for (const row of s.sessions) if (row.ring_id === WORK && row.bucket === 'needs_you') row.bucket = 'idle';
  });
  assert.deepEqual(chips(page), ['All 15 3', 'Personal 8 3', 'Work 7']);
  assert.equal(page.$$('.an-chip')[2].getAttribute('aria-label'), 'Work, 7 sessions');
});

test('the chips show only with several accounts and at least one session', async () => {
  const single = await open({ route: 'sessions' }, (s) => { s.accounts_multi = false; });
  assert.equal(single.$('.an-chips'), null);
  const empty = await open({ route: 'sessions' }, (s) => { s.sessions = []; });
  assert.equal(empty.$('.an-chips'), null, 'no sessions in any account: no chips');
  assert.equal(empty.text('.an-empty-title'), 'No Claude sessions yet');
});

test('choosing a chip narrows the list to that account; the strip follows; the selected chip clears it', async () => {
  const page = await open({ route: 'sessions' });
  page.click('.an-chip[data-an-arg="' + WORK + '"]');
  assert.equal(state(page).filter, WORK);
  assert.deepEqual(page.$$('.an-chip').map((c) => c.getAttribute('aria-pressed')), ['false', 'false', 'true']);
  assert.equal(page.$('.an-chip.an-sel').getAttribute('data-an-arg'), WORK);
  // Work's own counts: 1 answerable, 1 failed, 1 review, 2 working, 2 idle
  assert.deepEqual(strip(page), ['1 needs you', '1 failed', '1 to review', '2 working', '2 idle']);
  assert.equal(page.$$('#an-rows .an-row').length, 7, 'Work has 7 sessions, none folded (2 idle)');
  page.click('.an-chip[data-an-arg="' + WORK + '"]');
  assert.equal(state(page).filter, null, 'a selected account chip goes back to All');
  assert.deepEqual(strip(page), ['4 need you', '1 failed', '3 to review', '3 working', '4 idle']);
  page.click('.an-chip[data-an-arg="' + PERSONAL + '"]');
  page.click('.an-chip[data-an-arg=""]');
  assert.equal(state(page).filter, null, 'All');
  assert.equal(page.$$('#an-rows .an-row').length, 11, '15 sessions, the 4 idle ones folded to a summary');
  clean(page);
});

test('a filtered account with no sessions says so; a filter for a ring that is gone is dropped', async () => {
  const page = await open({ route: 'sessions', ring_id: WORK }, (s) => { s.sessions = s.sessions.filter((r) => r.ring_id !== WORK); });
  assert.equal(page.text('.an-empty-title'), 'No sessions in this account');
  assert.equal(page.text('.an-empty-text'), 'Choose All to see every account\u2019s sessions.');
  assert.deepEqual(chips(page), ['All 8 3', 'Personal 8 3', 'Work 0']);
  const gone = await open({ route: 'sessions', ring_id: 'claude-acct-gone' });
  assert.equal(gone.$('.an-chip.an-sel').getAttribute('data-an-arg'), '', 'no chip could clear that filter, so it does not apply');
  assert.equal(gone.$$('#an-rows .an-row').length, 11);
});

test('a long account label is cut in the middle and keeps its full text as the tooltip', async () => {
  const label = 'me.with.a.very.long.address@a-company-with-a-long-name.example · Max 20x';
  const page = await open({ route: 'sessions' }, (s) => { s.rings[0].label = label; });
  const el = page.$$('.an-chip-l')[1];
  assert.ok(Array.from(el.textContent).length <= 24, el.textContent);
  assert.ok(el.textContent.includes('…'));
  assert.ok(el.textContent.startsWith('me.with.a.'), 'the start survives');
  assert.ok(el.textContent.endsWith('Max 20x'), 'so does the end');
  assert.equal(page.$$('.an-chip')[1].getAttribute('title'), label);
  const short = page.window.agentnotchPanel._.middle('abc', 24);
  assert.equal(short, 'abc');
  assert.equal(page.window.agentnotchPanel._.middle('a  b\n c', 24), 'a b c', 'whitespace collapses');
  assert.equal(page.window.agentnotchPanel._.middle('\u{1F600}'.repeat(30), 24).length, 2 * 23 + 1, 'characters, not code units');
});

// ---- the gear menu ---------------------------------------------------------------------------------------

test('the gear opens the Mac\'s menu in the page: open automatically, two switches, mark all reviewed, Settings', async () => {
  const page = await open({ route: 'sessions' });
  assert.equal(page.$('.an-menu'), null);
  assert.equal(gear(page).getAttribute('aria-expanded'), 'false');
  page.click(gear(page));
  await page.settle();
  clean(page);
  assert.equal(gear(page).getAttribute('aria-expanded'), 'true');
  assert.equal(page.$('.an-menu').getAttribute('role'), 'menu');
  assert.deepEqual(menuText(page), ['Never', 'When a session needs you', 'When one needs you or is done',
    'Notify when a session needs you', 'Notify when a session is done', 'Mark all reviewed Ctrl+Shift+R', 'Open Settings…']);
  assert.equal(page.text('.an-mh'), 'Open automatically');
  assert.deepEqual(page.$$('.an-mi').map((m) => m.getAttribute('role')),
    ['menuitemradio', 'menuitemradio', 'menuitemradio', 'menuitemcheckbox', 'menuitemcheckbox', 'menuitem', 'menuitem']);
  assert.equal(page.$$('.an-msep').length, 3);
});

test('the menu shows the snapshot\'s open-automatically mode and Settings\' notification switches', async () => {
  const page = await open({ route: 'sessions' }, (s) => { s.ui.panel_open_mode = 'needsInput'; });
  page.click(gear(page));
  await page.settle();
  assert.deepEqual(plain(calls(page, 'settings')), [null], 'the switches live in Settings\' section: one ask per opening');
  assert.deepEqual(checked(page), ['needsInput', 'notifyNeedsInput', 'notifyReadyForReview'], 'fixture: both switches on');
  const off = await open({ route: 'sessions' }, null, { settings: (() => { const s = harness.fixture('settings.json'); s.notifications.notify_ready_for_review = false; return s; })() });
  off.click(gear(off));
  await off.settle();
  assert.deepEqual(checked(off), ['never', 'notifyNeedsInput']);
});

test('picking a mode sends autoOpen and closes the menu; the pick shows at once and the snapshot then agrees', async () => {
  const page = await open({ route: 'sessions' });
  page.click(gear(page));
  await page.settle();
  page.hub.clear();
  page.click(item(page, 'menu-auto', 'needsInputOrDone'));
  await page.settle();
  assert.deepEqual(calls(page, 'set_setting'), [{ key: 'autoOpen', value: 'needsInputOrDone' }]);
  assert.equal(page.$('.an-menu'), null, 'a pick closes the menu');
  page.click(gear(page));
  assert.ok(checked(page).includes('needsInputOrDone'), 'shown before the snapshot says so');
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 10; s.ui.panel_open_mode = 'needsInputOrDone'; }));
  assert.ok(checked(page).includes('needsInputOrDone'));
  assert.deepEqual(Object.keys(state(page).overrides), [], 'the override ends when the snapshot shows the value');
  clean(page);
});

test('the notification switches send their own settings and flip; a refusal puts them back', async () => {
  const page = await open({ route: 'sessions' });
  page.click(gear(page));
  await page.settle();
  page.hub.clear();
  page.click(item(page, 'menu-notify', 'notifyNeedsInput'));
  await page.settle();
  assert.deepEqual(calls(page, 'set_setting'), [{ key: 'notifyNeedsInput', value: false }]);
  page.click(gear(page));
  assert.ok(!checked(page).includes('notifyNeedsInput'));
  page.click(item(page, 'menu-notify', 'notifyReadyForReview'));
  await page.settle();
  assert.deepEqual(calls(page, 'set_setting').slice(-1), [{ key: 'notifyReadyForReview', value: false }]);
  const sealed = await open({ route: 'sessions' }, null, { replies: { set_setting: () => { throw { code: 'sealed', message: 'Sealed: nothing changes here.' }; } } });
  sealed.click(gear(sealed));
  await sealed.settle();
  sealed.click(item(sealed, 'menu-notify', 'notifyNeedsInput'));
  await sealed.settle();
  sealed.click(gear(sealed));
  assert.ok(checked(sealed).includes('notifyNeedsInput'), 'a refused write leaves the switch where it was');
  clean(sealed);
});

test('an action name that is not in the menu is refused; only the two switch keys and three modes go out', async () => {
  const page = await open({ route: 'sessions' });
  await page.settle();
  page.hub.clear();
  const tamper = (action, arg, attribute, value) => {
    page.click(gear(page));
    const el = item(page, action, arg);
    el.setAttribute(attribute, value);
    page.click(el);
  };
  tamper('menu-auto', 'never', 'data-an-arg', 'sometimes');
  tamper('menu-notify', 'notifyNeedsInput', 'data-an-arg', 'ringBadges');
  tamper('open-settings', undefined, 'data-an-action', 'constructor');
  tamper('open-settings', undefined, 'data-an-action', '__proto__');
  await page.settle();
  assert.deepEqual(page.hub.calls.map((c) => c.method).filter((m) => m !== 'settings'), []);
  clean(page);
});

test('Mark all reviewed is disabled with nothing to review, and Open Settings opens the Claude Code pane', async () => {
  const page = await open({ route: 'sessions' });
  page.click(gear(page));
  assert.equal(item(page, 'mark-all-reviewed').hasAttribute('disabled'), false, 'the fixture has 3 to review');
  const none = await open({ route: 'sessions' }, (s) => { s.totals.review = 0; });
  none.click(gear(none));
  assert.equal(item(none, 'mark-all-reviewed').hasAttribute('disabled'), true);
  none.hub.clear();
  none.click(item(none, 'mark-all-reviewed'));
  assert.deepEqual(none.hub.calls, [], 'a disabled item does nothing');
  page.hub.clear();
  page.click(item(page, 'open-settings'));
  await page.settle();
  assert.deepEqual(calls(page, 'open_settings'), [{ tab: 'claude' }]);
  assert.equal(page.$('.an-menu'), null);
  clean(page);
});

test('Mark all reviewed closes the menu and runs the action a later sub-task registers', async () => {
  const page = await open({ route: 'sessions' });
  page.click(gear(page));
  page.run('agentnotchPanel.actions["mark-all-reviewed"] = function () { window.__marked = (window.__marked || 0) + 1; };');
  page.click(item(page, 'mark-all-reviewed'));
  assert.equal(page.run('window.__marked'), 1);
  assert.equal(page.$('.an-menu'), null);
});

test('a click outside closes the menu, a click on the gear toggles it, and Esc closes it before anything else', async () => {
  const page = await open({ route: 'sessions' });
  page.click(gear(page));
  page.fire('#an-rows', 'pointerdown');
  assert.equal(page.$('.an-menu'), null, 'outside');
  page.click(gear(page));
  page.fire('.an-menu', 'pointerdown');
  assert.ok(page.$('.an-menu'), 'a press inside the menu keeps it');
  page.fire(gear(page), 'pointerdown');
  page.click(gear(page));
  assert.equal(page.$('.an-menu'), null, 'the gear toggles');
  page.click(gear(page));
  page.hub.clear();
  // A click opens the menu whoever has the keyboard; its keys wait for the glue's word.
  const shut = page.key({ key: 'Escape' });
  assert.ok(page.$('.an-menu') !== null, 'Esc while the keyboard gate is shut does nothing');
  assert.equal(shut.defaultPrevented, false);
  keyboard(page);
  const esc = page.key({ key: 'Escape' });
  assert.ok(page.$('.an-menu') === null);
  assert.equal(esc.defaultPrevented, true);
  assert.deepEqual(page.hub.calls, [], 'Esc with a menu open only closes the menu');
  clean(page);
});

test('arrow keys move through the menu items and skip a disabled one', async () => {
  const page = await open({ route: 'sessions' }, (s) => { s.totals.review = 0; });
  page.click(gear(page));
  const items = page.$$('.an-mi').filter((m) => !m.hasAttribute('disabled'));
  assert.equal(items.length, 6);
  assert.ok(page.document.activeElement === items[0], 'the first item has focus');
  // While the keyboard gate is shut the menu takes no key: no move, and Enter or Space on the
  // focused item clicks nothing (the browser's own click is cancelled on both).
  page.key({ key: 'ArrowDown' });
  assert.ok(page.document.activeElement === items[0], 'no move while the gate is shut');
  for (const key of ['Enter', ' ']) {
    assert.equal(page.key({ key }).defaultPrevented, true, `${JSON.stringify(key)} down`);
    assert.equal(page.fire(items[0], 'keyup', { key }).defaultPrevented, true, `${JSON.stringify(key)} up`);
  }
  keyboard(page);
  page.key({ key: 'ArrowDown' });
  assert.equal(page.document.activeElement, items[1]);
  page.key({ key: 'End' });
  assert.equal(page.document.activeElement, items[5]);
  page.key({ key: 'ArrowDown' });
  assert.equal(page.document.activeElement, items[0], 'wraps');
  page.key({ key: 'ArrowUp' });
  assert.equal(page.document.activeElement, items[5]);
  page.key({ key: 'Home' });
  assert.equal(page.document.activeElement, items[0]);
  page.key({ key: 'Escape' });
  assert.equal(page.document.activeElement, gear(page), 'focus goes back to the gear');
});

test('a snapshot that arrives while the menu is open keeps the menu, its focus and its items', async () => {
  const page = await open({ route: 'sessions' });
  page.click(gear(page));
  await page.settle();
  const menu = page.$('.an-menu');
  const second = page.$$('.an-mi')[1];
  second.focus();
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 100; s.totals.working = 7; s.sealed = false; }));
  assert.equal(page.$('.an-menu'), menu, 'the same element');
  assert.equal(page.$$('.an-mi')[1], second);
  assert.equal(page.document.activeElement, second, 'focus survives');
  assert.deepEqual(strip(page).slice(3, 4), ['7 working'], 'and the header did update');
  assert.equal(page.$('.an-sealed'), null);
  const chip = page.$('.an-chip');
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 200; }));
  assert.equal(page.$('.an-chip'), chip, 'the chips are patched, not rebuilt');
});

// ---- pin, close, Esc ---------------------------------------------------------------------------------------

test('pin sets panelPinned, shows at once and toggles back; the title and label say which', async () => {
  const page = await open({ route: 'sessions' });
  const pin = () => page.$('[data-an-action="pin"]');
  assert.equal(pin().getAttribute('aria-pressed'), 'false');
  page.hub.clear();
  page.click(pin());
  assert.deepEqual(calls(page, 'set_setting'), [{ key: 'panelPinned', value: true }]);
  assert.equal(pin().getAttribute('aria-pressed'), 'true');
  assert.equal(pin().getAttribute('title'), 'Keep open: on');
  assert.ok(pin().classList.contains('an-on'));
  await page.settle();
  page.click(pin());
  assert.deepEqual(calls(page, 'set_setting').slice(-1), [{ key: 'panelPinned', value: false }]);
  assert.equal(pin().getAttribute('title'), 'Keep open');
  clean(page);
});

test('the pin follows the snapshot, and a refused write (sealed) puts it back', async () => {
  const page = await open({ route: 'sessions' }, (s) => { s.ui.panel_pinned = true; });
  assert.equal(page.$('[data-an-action="pin"]').getAttribute('aria-pressed'), 'true');
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; s.ui.panel_pinned = false; }));
  assert.equal(page.$('[data-an-action="pin"]').getAttribute('aria-pressed'), 'false');
  const sealed = await open({ route: 'sessions' }, null, { replies: { set_setting: () => { throw { code: 'sealed', message: 'no' }; } } });
  sealed.click('[data-an-action="pin"]');
  await sealed.settle();
  assert.equal(sealed.$('[data-an-action="pin"]').getAttribute('aria-pressed'), 'false');
  clean(sealed);
});

test('an override stands for a moment after its reply so a snapshot still in flight cannot undo it', async () => {
  const page = await open({ route: 'sessions' });
  page.click('[data-an-action="pin"]');
  await page.settle();
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; }));
  assert.equal(page.$('[data-an-action="pin"]').getAttribute('aria-pressed'), 'true', 'the stale snapshot');
  page.tick(2500);
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 2; }));
  assert.equal(page.$('[data-an-action="pin"]').getAttribute('aria-pressed'), 'false', 'later, the snapshot is the truth');
});

test('the close button asks the glue to close the panel', async () => {
  const page = await open({ route: 'sessions' });
  page.hub.clear();
  page.click('[data-an-action="close"]');
  assert.deepEqual(page.hub.calls.map((c) => c.method), ['panel_close']);
  clean(page);
});

test('Esc steps out one level at a time: menu, chat, list, panel (B_PanelStateTests routesBackAndClose)', async () => {
  const page = await open({ route: 'sessions', ring_id: WORK });
  page.emit('an:panel', { route: 'session:needs-permission', ring_id: WORK, reason: 'hover_row' });
  assert.equal(state(page).route, 'session:needs-permission');
  assert.equal(state(page).selected, 'needs-permission');
  page.hub.clear();
  // Esc is behind the keyboard gate like every other key (DESIGN-WIN §5.3).
  page.key({ key: 'Escape' });
  assert.equal(state(page).route, 'session:needs-permission', 'nothing while the gate is shut');
  assert.deepEqual(page.hub.calls, []);
  keyboard(page);
  page.key({ key: 'Escape' });
  assert.equal(state(page).route, 'sessions', 'the chat goes back to the list');
  assert.equal(state(page).filter, WORK, 'keeping the ring filter');
  assert.deepEqual(page.hub.calls.map((c) => [c.method, plain(c.args)]), [['chat_close', { session_id: 'needs-permission' }], ['panel_route', { route: 'sessions' }]], 'the chat is told it is left, then the glue');
  assert.equal(page.$('#an-card').getAttribute('data-mode'), 'list');
  page.click(gear(page));
  page.hub.clear();
  page.key({ key: 'Escape' });
  assert.deepEqual(page.hub.calls, [], 'the menu first');
  page.key({ key: 'Escape' });
  assert.deepEqual(page.hub.calls.map((c) => c.method), ['panel_close'], 'then the panel');
  clean(page);
});

test('Esc that something else already handled does nothing here', async () => {
  const page = await open({ route: 'sessions' });
  keyboard(page);
  // a composer's own Esc handling sits below the document in the event path
  page.document.body.addEventListener('keydown', (e) => e.preventDefault());
  page.hub.clear();
  page.key({ key: 'Escape' });
  assert.deepEqual(page.hub.calls, []);
});

test('navigate() moves between the list and a chat, tells the glue once per change and selects the session', async () => {
  const page = await open({ route: 'sessions' });
  page.hub.clear();
  page.run("agentnotchPanel.navigate('session:s1')");
  page.run("agentnotchPanel.navigate('session:s1')");
  assert.deepEqual(page.hub.calls.filter((c) => c.method === 'panel_route').map((c) => [c.method, plain(c.args)]), [['panel_route', { route: 'session:s1' }]]);
  assert.equal(state(page).selected, 's1');
  assert.equal(page.$('#an-chat').hasAttribute('hidden'), false);
  page.run("agentnotchPanel.navigate('session:')");
  assert.equal(state(page).route, 'sessions', 'a chat needs a session');
  assert.equal(page.$('#an-chat').hasAttribute('hidden'), true);
  page.run("agentnotchPanel.navigate('nonsense')");
  assert.equal(state(page).route, 'sessions');
  assert.equal(page.hub.of('panel_route').length, 2);
  clean(page);
});

test('the chat is mounted into its region on a session route and unmounted when the route leaves it', async () => {
  const page = await open({ route: 'sessions' });
  page.run(`window.__chat = []; agentnotchChat.mount = function (host, ctx) { window.__chat.push(['mount', host.id, ctx.sessionId, ctx.panel === agentnotchPanel]); };
    agentnotchChat.unmount = function () { window.__chat.push(['unmount']); };`);
  page.run("agentnotchPanel.navigate('session:a')");
  page.emit('an:panel', { route: 'session:b' });
  page.emit('an:panel', { route: 'sessions' });
  assert.deepEqual(plain(page.run('window.__chat')), [['mount', 'an-chat', 'a', true], ['unmount'], ['mount', 'an-chat', 'b', true], ['unmount']]);
});

// ---- the keyboard gate's state -------------------------------------------------------------------------------

test('an:panel_focus sets the gate\'s flag and the body class; a window focus event alone does not', async () => {
  const page = await open({ route: 'sessions' });
  assert.equal(state(page).focused, false);
  assert.equal(page.document.body.classList.contains('an-focused'), false);
  page.window.dispatchEvent(new page.window.Event('focus'));
  page.fire(page.document.body, 'focus');
  assert.equal(state(page).focused, false, 'the DOM never opens the gate');
  page.emit('an:panel_focus', { focused: true });
  assert.equal(state(page).focused, true);
  assert.equal(page.document.body.classList.contains('an-focused'), true);
  page.emit('an:panel_focus', { focused: false });
  assert.equal(state(page).focused, false);
  assert.equal(page.document.body.classList.contains('an-focused'), false);
  page.emit('an:panel_focus', {});
  assert.equal(state(page).focused, false, 'anything but true shuts it');
  page.emit('an:panel_focus', { focused: 'yes' });
  assert.equal(state(page).focused, true, 'truthy is what the glue sends: a boolean');
  clean(page);
});

// ---- engagement ------------------------------------------------------------------------------------------------

test('panel_engaged follows the pointer over the card and a focused text field, and is sent only on a change', async () => {
  const page = await open({ route: 'sessions' });
  page.hub.clear();
  const card = page.$('#an-card');
  page.fire(card, 'pointerleave');
  assert.deepEqual(page.hub.calls, [], 'never engaged, nothing to say');
  page.fire(card, 'pointerenter');
  page.fire(card, 'pointerenter');
  assert.deepEqual(page.hub.of('panel_engaged').map((c) => plain(c.args)), [{ on: true }]);
  page.fire(card, 'pointerleave');
  assert.deepEqual(page.hub.of('panel_engaged').map((c) => plain(c.args)), [{ on: true }, { on: false }]);
  const field = page.document.createElement('textarea');
  page.$('#an-toast').appendChild(field);
  field.focus();
  assert.deepEqual(page.hub.of('panel_engaged').slice(-1).map((c) => plain(c.args)), [{ on: true }], 'a field with focus');
  field.blur();
  assert.deepEqual(page.hub.of('panel_engaged').slice(-1).map((c) => plain(c.args)), [{ on: false }]);
  const button = page.document.createElement('button');
  page.$('#an-toast').appendChild(button);
  page.hub.clear();
  button.focus();
  assert.deepEqual(page.hub.calls, [], 'a button is not a field');
  page.fire(card, 'pointerenter');
  field.focus();
  page.fire(card, 'pointerleave');
  assert.equal(page.hub.of('panel_engaged').length, 1, 'engaged by the pointer, then by the field: nothing more to say');
  clean(page);
});

// ---- the size the window follows ---------------------------------------------------------------------------

test('panel_report_size sends the card\'s natural size when the content changes, and only then', async () => {
  const page = await open({ route: 'sessions' });
  assert.deepEqual(page.hub.of('panel_report_size'), [], 'no layout, no report');
  page.hub.clear();
  size(page, { width: 440, top: 120, list: 300 });
  assert.deepEqual(page.hub.of('panel_report_size').map((c) => plain(c.args)), [{ w: 440, h: 421 }]);
  size(page, { width: 440, top: 120, list: 300 });
  assert.equal(page.hub.of('panel_report_size').length, 1, 'unchanged: quiet');
  size(page, { width: 440, top: 120, list: 420.2 });
  assert.deepEqual(page.hub.of('panel_report_size').slice(-1).map((c) => plain(c.args)), [{ w: 440, h: 542 }], 'rounded up');
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; s.totals.working = 9; }));
  assert.equal(page.hub.of('panel_report_size').length, 2, 'a render measures too, and finds the same height');
  size(page, { width: 520, top: 120, list: 420.2 });
  assert.deepEqual(page.hub.of('panel_report_size').slice(-1).map((c) => plain(c.args)), [{ w: 520, h: 542 }], 'a new width counts');
  clean(page);
});

test('the natural height counts the list by its content, the toast, an open menu and skips the hidden chat', async () => {
  const page = await open({ route: 'sessions' });
  size(page, { width: 400, top: 100, list: 500 });
  rect(page.$('#an-toast'), 400, 50);
  assert.equal(page.run('agentnotchPanel._.naturalHeight()'), 100 + 1 + 500 + 50);
  page.click(gear(page));
  const menu = page.$('.an-menu');
  menu.__rect = { left: 0, top: 40, right: 240, bottom: 340, width: 240, height: 300 };
  assert.equal(page.run('agentnotchPanel._.naturalHeight()'), 651, 'the list is taller than the menu here');
  size(page, { width: 400, top: 100, list: 10 });
  rect(page.$('#an-toast'), 400, 0);
  assert.equal(page.run('agentnotchPanel._.naturalHeight()'), 308, 'a short list with the menu open: the menu and its margin');
  page.run('agentnotchPanel._.render()');
  assert.equal(page.$('#an-card').style.minHeight, '308px', 'the card grows to hold the menu');
  page.click(gear(page));
  assert.equal(page.$('#an-card').style.minHeight, '', 'and lets go of it');
  page.emit('an:panel', { route: 'session:x' });
  // a chat counts by what it holds (header, the transcript's content, hairline), not by the box it is given
  rect(page.$('#an-chat'), 440, 640);
  rect(page.$('#an-chat-head'), 440, 60);
  rect(page.$('#an-chat-list'), 440, 700);
  assert.equal(page.run('agentnotchChat.naturalHeight()'), 60 + 1 + 700);
  assert.equal(page.run('agentnotchPanel._.naturalHeight()') >= 761, true);
});

test('without a measured width the report uses the placement\'s widths: 400/440 beside, 440/520 flat and floating', async () => {
  const widths = async (placement, route) => {
    const page = await open({ route, placement });
    rect(page.$('#an-top'), 0, 100);
    page.resizeObservers.forEach((o) => o.callback([]));
    return plain(page.hub.of('panel_report_size').slice(-1)[0].args.w);
  };
  const beside = { kind: 'beside', tail_edge: 'left', tail_offset: 0 };
  const flat = { kind: 'above_or_below', tail_edge: 'top', tail_offset: 0 };
  assert.equal(await widths(beside, 'sessions'), 400);
  assert.equal(await widths(beside, 'session:a'), 440);
  assert.equal(await widths(flat, 'sessions'), 440);
  assert.equal(await widths(flat, 'session:a'), 520);
  assert.equal(await widths(undefined, 'sessions'), 440);
  assert.equal(await widths(undefined, 'session:a'), 520);
  assert.equal(await widths({ kind: 'beside', tail_edge: 'right', width: 380 }, 'sessions'), 380, 'the glue\'s own width wins');
});

// ---- browser accelerators --------------------------------------------------------------------------------------

test('WebView2\'s own shortcuts are prevented: F5, Ctrl+R, Ctrl+Shift+R, Ctrl+J, Ctrl+P, Ctrl+F', async () => {
  const page = await open({ route: 'sessions' });
  for (const init of [{ key: 'F5' }, { key: 'r', ctrlKey: true }, { key: 'R', ctrlKey: true, shiftKey: true },
    { key: 'j', ctrlKey: true }, { key: 'p', ctrlKey: true }, { key: 'f', ctrlKey: true }]) {
    assert.equal(page.key(init).defaultPrevented, true, JSON.stringify(init));
  }
  for (const init of [{ key: 'a' }, { key: 'c', ctrlKey: true }, { key: 'v', ctrlKey: true }, { key: 'Enter' }, { key: 'r' }, { key: 'F6' }]) {
    assert.equal(page.key(init).defaultPrevented, false, JSON.stringify(init));
  }
  clean(page);
});

// ---- the tail per placement ------------------------------------------------------------------------------------

test('the tail follows the placement: left and right beside, top and bottom above or below, none floating', async () => {
  const cases = [
    [{ kind: 'beside', tail_edge: 'right', tail_offset: 12, width: 400 }, 'right', 12],
    [{ kind: 'beside', tail_edge: 'left', tail_offset: -30, width: 440 }, 'left', -30],
    [{ kind: 'above_or_below', tail_edge: 'top', tail_offset: 0, width: 440 }, 'top', 0],
    [{ kind: 'above_or_below', tail_edge: 'bottom', tail_offset: 60, width: 520 }, 'bottom', 60],
    [{ kind: 'floating', width: 440 }, 'none', 0],
    [undefined, 'none', 0],
    [{ kind: 'beside', tail_edge: 'top' }, 'none', 0],
    [{ kind: 'above_or_below', tail_edge: 'left' }, 'none', 0],
    [{ kind: 'beside' }, 'none', 0],
    [{ kind: 'sideways', tail_edge: 'left' }, 'none', 0],
    [{ kind: 'beside', tail_edge: 'left', tail_offset: 'far' }, 'left', 0],
    [{ kind: 'beside', tail_edge: 'left', tail_offset: 1e9 }, 'left', 2000],
    [{ kind: 'beside', tail_edge: 'left', tail_offset: Number.NaN }, 'left', 0],
  ];
  for (const [placement, tail, offset] of cases) {
    const page = await open({ route: 'sessions', placement });
    const label = JSON.stringify(placement);
    assert.equal(page.$('#an-panel').getAttribute('data-tail'), tail, label);
    assert.equal(page.$('#an-frame').style.getPropertyValue('--an-tail-offset'), `${offset}px`, label);
    assert.equal(page.$('#an-tail').getAttribute('aria-hidden'), 'true');
    clean(page);
  }
  const page = await open({ route: 'sessions', placement: { kind: 'beside', tail_edge: 'left' } });
  page.emit('an:panel', { route: 'sessions', placement: { kind: 'above_or_below', tail_edge: 'bottom', tail_offset: 5 } });
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'bottom', 'a later open moves the tail');
  page.emit('an:panel', { route: 'sessions' });
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'none', 'and one with no placement takes it away');
});

test('the glue\'s own placement fields (edge, floating, width, tail_offset) place the tail, and an:panel_place moves it', async () => {
  // What panel_window.rs sends: the notch's edge, which is the side of the card the tail is on.
  const cases = [
    [{ edge: 'right', floating: false, width: 400, tail_offset: 12 }, 'beside', 'right', 12, 400],
    [{ edge: 'left', floating: false, width: 440, tail_offset: -30 }, 'beside', 'left', -30, 440],
    [{ edge: 'top', floating: false, width: 520, tail_offset: 0 }, 'above_or_below', 'top', 0, 520],
    [{ edge: 'bottom', floating: false, width: 440, tail_offset: 60 }, 'above_or_below', 'bottom', 60, 440],
    [{ edge: null, floating: true, width: 520, tail_offset: 0 }, 'floating', 'none', 0, 0],
    // Anything the glue would not send is floating.
    [{ edge: 'right' }, 'floating', 'none', 0, 0],
    [{ edge: 'sideways', floating: false }, 'floating', 'none', 0, 0],
  ];
  for (const [fields, kind, tail, offset, width] of cases) {
    const page = await open(Object.assign({ route: 'sessions', reason: 'ring_click' }, fields));
    const label = JSON.stringify(fields);
    assert.deepEqual(state(page).placement, { kind, tail, offset, width }, label);
    assert.equal(page.$('#an-panel').getAttribute('data-tail'), tail, label);
    assert.equal(page.$('#an-frame').style.getPropertyValue('--an-tail-offset'), `${offset}px`, label);
    clean(page);
  }
  const page = await open({ route: 'sessions', reason: 'ring_click', edge: 'right', floating: false, width: 400, tail_offset: 0 });
  page.emit('an:panel', { route: 'sessions', reason: 'ring_click', edge: 'left', floating: false, width: 400, tail_offset: 8,
    placement: { kind: 'above_or_below', tail_edge: 'top', tail_offset: 3 } });
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'top', 'a placement object, when there is one, wins');
  page.emit('an:panel_place', { edge: 'bottom', floating: false, width: 520, tail_offset: -5 });
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'bottom', 'an open panel that moved takes its new side');
  assert.equal(page.$('#an-frame').style.getPropertyValue('--an-tail-offset'), '-5px');
  assert.equal(state(page).placement.width, 520);
  assert.equal(state(page).route, 'sessions', 'a move is not a new request');
  page.emit('an:panel_place', { edge: null, floating: true, width: 440, tail_offset: 0 });
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'none', 'and one that now floats has no tail');
  page.emit('an:panel_place', null);
  page.emit('an:panel_place', 'right');
  assert.equal(page.$('#an-panel').getAttribute('data-tail'), 'none', 'junk is ignored');
  clean(page);
});

// ---- hostile strings ---------------------------------------------------------------------------------------------

test('hostile account labels are text in the chips, the strip and the labels, whatever they say', async () => {
  for (const hostile of audit.HOSTILE) {
    const page = await open({ route: 'sessions', ring_id: WORK }, (s) => {
      s.rings[0].label = hostile;
      s.rings[1].label = hostile + hostile;
    });
    const header = page.$('#an-header');
    assert.deepEqual(audit.problems(header), [], hostile.slice(0, 30));
    assert.equal(header.querySelectorAll('img, script, iframe, style, textarea, input, a, base').length, 0, hostile.slice(0, 30));
    for (const chip of page.$$('.an-chip')) {
      assert.ok(Array.from(chip.querySelector('.an-chip-l').textContent).length <= 24);
      assert.ok(chip.getAttribute('title').length <= 400);
    }
    clean(page);
  }
  assert.notDeepEqual(audit.problems(audit.HOSTILE[0]), [], 'the check can fail');
});

test('a hostile engine message is text in the empty state; the U+202E override is isolated', async () => {
  for (const hostile of audit.HOSTILE) {
    const page = await load({ replies: { snapshot: () => { throw { code: 'failed', message: hostile }; } } });
    assert.deepEqual(audit.problems(page.$('#an-rows')), [], hostile.slice(0, 30));
    assert.equal(page.$$('.an-empty-text').length, 1);
    assert.equal(page.$('.an-empty-text').textContent, hostile);
  }
  assert.match(CSS, /\.an-chip-l \{[^}]*unicode-bidi: isolate/);
  assert.match(CSS, /\.an-title \{[^}]*unicode-bidi: isolate/);
});

test('an unreachable engine says so in the list and leaves the header working', async () => {
  const page = await load({ snapshot: null });
  assert.equal(page.text('.an-empty-title'), 'Couldn’t load the sessions');
  assert.match(page.text('.an-empty-text'), /isn't running/);
  assert.equal(page.$('.an-strip'), null);
  page.hub.clear();
  page.click('[data-an-action="close"]');
  assert.deepEqual(page.hub.calls.map((c) => c.method), ['panel_close']);
  page.emit('an:snapshot', snapshotWith());
  assert.equal(page.$('.an-empty-title'), null, 'a snapshot ends the error');
  const bare = await load({ bridge: false });
  assert.match(bare.text('.an-empty-text'), /isn't reachable/);
});

// ---- sealed scenes and the layout report ---------------------------------------------------------------------------

test('showScene: unknown names and a page with no snapshot yet answer false; known ones set the static mode', async () => {
  const page = await open({ route: 'sessions' });
  assert.equal(page.run("agentnotchPanel.showScene('nope')"), false);
  assert.equal(page.run("agentnotchPanel.showScene('constructor')"), false);
  assert.equal(page.document.documentElement.classList.contains('an-static'), false);
  assert.equal(page.run("agentnotchPanel.showScene('panel-every-state')"), true);
  assert.equal(page.document.documentElement.classList.contains('an-static'), true);
  const bare = harness.loadPage('agentnotch/panel.html', { snapshot: null });
  assert.equal(bare.run("agentnotchPanel.showScene('panel-empty')"), false, 'no snapshot to show a state from');
});

test('the header scenes: empty, menu, filtered, single account, pinned', async () => {
  const page = await open({ route: 'sessions' });
  page.run("agentnotchPanel.showScene('panel-empty')");
  assert.equal(page.$('.an-strip'), null);
  assert.equal(page.$('.an-chips'), null);
  assert.equal(page.text('.an-empty-title'), 'No Claude sessions yet');
  assert.equal(page.text('.an-empty-text'), 'Start Claude Code in VS Code or a terminal. Sessions from every account show up here.');
  assert.ok(page.$('.an-sealed'));
  page.run("agentnotchPanel.showScene('panel-menu')");
  await page.settle();
  assert.ok(page.$('.an-menu'));
  assert.deepEqual(checked(page), ['never', 'notifyNeedsInput', 'notifyReadyForReview'], 'the switches are read for the scene');
  assert.equal(page.document.activeElement === page.$('.an-mi'), false, 'a static scene does not take focus');
  page.run("agentnotchPanel.showScene('panel-filtered')");
  assert.equal(page.$('.an-menu'), null);
  assert.equal(page.$('.an-chip.an-sel').getAttribute('data-an-arg'), WORK);
  page.run("agentnotchPanel.showScene('panel-single-account')");
  assert.equal(page.$('.an-chips'), null);
  page.run("agentnotchPanel.showScene('panel-pinned')");
  assert.equal(page.$('[data-an-action="pin"]').getAttribute('aria-pressed'), 'true');
  clean(page);
});

test('a scene holds through snapshots and gives way to a real request', async () => {
  const page = await open({ route: 'sessions' });
  page.run("agentnotchPanel.showScene('panel-empty')");
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; }));
  assert.equal(page.$('.an-strip'), null, 'still the empty scene');
  page.emit('an:panel', { route: 'sessions', reason: 'ring_click' });
  assert.ok(page.$('.an-strip'), 'a real open shows the real state');
  assert.equal(state(page).scene, null);
});

test('layoutReport is ok when nothing is measured and names what breaks when something is', async () => {
  const page = await open({ route: 'sessions' });
  const report = () => plain(page.run('agentnotchPanel.layoutReport()'));
  const ok = report();
  assert.equal(ok.ok, true);
  assert.deepEqual(ok.failures, []);
  assert.equal(ok.route, 'sessions');
  assert.ok(ok.texts >= 8, 'the title, the strip and the chips are all checked');
  // a [data-an-text] element wider than its box
  const title = page.$('.an-title');
  title.__scrollWidth = 300;
  rect(title, 100, 20);
  assert.match(report().failures[0], /^text is clipped: Claude sessions/);
  title.__scrollWidth = undefined;
  // a control outside the card
  rect(page.$('#an-card'), 400, 300);
  page.window.innerWidth = 400;
  page.window.innerHeight = 300;
  const close = page.$('[data-an-action="close"]');
  rect(close, 22, 20);
  assert.equal(report().ok, true);
  close.__rect = { left: 390, top: 10, right: 430, bottom: 30, width: 40, height: 20 };
  assert.match(report().failures.join(), /control outside the card: Close/);
  rect(close, 22, 20);
  // the card leaving the window
  page.$('#an-card').__rect = { left: 0, top: 0, right: 500, bottom: 300, width: 500, height: 300 };
  assert.match(report().failures.join(), /the card leaves the window/);
});

test('layoutReport checks the tail meets the card', async () => {
  const page = await open({ route: 'sessions', placement: { kind: 'beside', tail_edge: 'left', tail_offset: 0, width: 400 } });
  const report = () => plain(page.run('agentnotchPanel.layoutReport()'));
  page.window.innerWidth = 432;
  page.window.innerHeight = 300;
  page.$('#an-card').__rect = { left: 32, top: 0, right: 432, bottom: 300, width: 400, height: 300 };
  page.$('#an-tail').__rect = { left: 0, top: 132, right: 33, bottom: 168, width: 33, height: 36 };
  assert.deepEqual(report().failures, []);
  assert.equal(report().tail, 'left');
  page.$('#an-tail').__rect = { left: 0, top: 132, right: 20, bottom: 168, width: 20, height: 36 };
  assert.match(report().failures.join(), /the tail does not meet the card/);
  page.$('#an-tail').__rect = { left: -40, top: 132, right: -7, bottom: 168, width: 33, height: 36 };
  assert.match(report().failures.join(), /the tail leaves the window/);
});

test('the region ids and action attributes the later sub-tasks build on exist and hold what they should', async () => {
  const page = await open({ route: 'sessions' });
  for (const id of ['an-panel', 'an-frame', 'an-tail', 'an-card', 'an-top', 'an-header', 'an-banners', 'an-hair', 'an-list', 'an-rows', 'an-toast', 'an-chat', 'an-overlay']) {
    assert.ok(page.$('#' + id), id);
  }
  assert.equal(page.$('#an-chat').hasAttribute('hidden'), true);
  assert.ok(page.$('#an-card').contains(page.$('#an-rows')));
  assert.deepEqual(plain(page.run('Object.keys(agentnotchPanel.actions)')).sort(),
    ['answer', 'back', 'close', 'consent-later', 'consent-on', 'dismiss-failure', 'filter', 'fold', 'gear', 'jump', 'mark-all-reviewed', 'mark-reviewed', 'menu-auto', 'menu-notify', 'open-chat', 'open-settings', 'pin', 'scope-off', 'scope-ok', 'undo-review']);
  assert.deepEqual(plain(page.run('Object.keys(agentnotchPanel.scenes)')).sort(),
    ['chat-approval', 'chat-composer', 'chat-no-route', 'chat-plan', 'chat-question-other', 'chat-tasks', 'chat-terminal-only', 'panel-banners', 'panel-busy-full', 'panel-busy-window', 'panel-consent', 'panel-empty', 'panel-every-state', 'panel-filtered', 'panel-header', 'panel-keyboard-folded', 'panel-menu', 'panel-needs-you', 'panel-pinned', 'panel-regular-rows', 'panel-scope-notice', 'panel-single-account', 'panel-undo']);
});
