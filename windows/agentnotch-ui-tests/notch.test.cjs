'use strict';
// ui/agentnotch/notch.js and notch.css on the real notch page: upstream's `notch.html` with its
// own script, the fork's scripts in the same vm context, a fake bridge that holds every call
// against the contract (lib/contract.cjs) and a hand-moved clock. What node cannot know is
// layout; that is asserted in a real browser through `layoutReport()` by the snapshot tool
// (tools/render-scenes.cjs, the notch-* scenes), and here only where a test sets the rectangles.
//
// Swift tests ported: C_RingBadgeLayoutTests (angles, centres per edge, the label rule, the
// clearance of the weekly ring, the labels), C_BurstAndRestingMarkTests (resting marks) and
// Fix_RestingMarkShapeTests, NotchSessionCapTests (rows that fit, "and N more"),
// C_NotchCountsTests (which rings count). The rest is new: the page-order test, the
// wrapped-globals test, cells, activity, the stale class, card rows, ring clicks, hover clicks,
// the wrappers, the peek and hostile strings.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const dom = require('./lib/dom.cjs');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');

const AGENTNOTCH = path.join(harness.UI, 'agentnotch');
const NOTCH_JS = fs.readFileSync(path.join(AGENTNOTCH, 'notch.js'), 'utf8');
const NOTCH_CSS = fs.readFileSync(path.join(AGENTNOTCH, 'notch.css'), 'utf8');
const NOTCH_HTML = fs.readFileSync(path.join(harness.UI, 'notch.html'), 'utf8');
const NOW = harness.NOW;
const EDGES = ['right', 'left', 'top', 'bottom'];
const PERSONAL = 'claude-acct-5f3e1d2c0b9a';
const WORK = 'claude-acct-8a7b6c5d4e3f';

// Objects built in the page's vm context have that context's prototypes: compare plain data.
const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

async function load(options) {
  const page = harness.loadPage('notch.html', options);
  await page.settle();
  return page;
}

/** A fresh copy of the ui-contract snapshot, edited by `edit`. */
function snapshotWith(edit) {
  const snapshot = harness.fixture('snapshot.json');
  if (edit) edit(snapshot);
  return snapshot;
}

async function loadWith(edit, options) {
  return load(Object.assign({ snapshot: snapshotWith(edit) }, options));
}

/** No page error and no call the contract or the window gate would refuse. */
function clean(page) {
  assert.deepEqual(page.errors.map(String), []);
  assert.deepEqual(page.hub.violations, []);
}

const cellsOf = (page) => page.$$('#pill .cell');
const cellFor = (page, id) => cellsOf(page).find((c) => c.getAttribute('data-p') === id);
const ringOf = (snapshot, id) => snapshot.rings.find((r) => r.ring_id === id);
const styleNumber = (el, name) => parseFloat(el.style.getPropertyValue(name));

/** The Claude card of a ring, opened the way a hover does. */
function openCard(page, ringId) {
  page.run(`hoverId = ${JSON.stringify(ringId)}; showCard();`);
  return page.$('#card');
}

async function clickRow(page, sessionId) {
  const button = page.$$('#card [data-an-session]').find((b) => b.getAttribute('data-an-session') === sessionId);
  assert.ok(button, `no card row for ${sessionId}`);
  page.click(button);
  await page.settle();
}

const isShown = (page) => page.$('#card').classList.contains('show');

// ---- the page order ---------------------------------------------------------------------------

test('the page loads: upstream script and fork scripts in one context, no error, one snapshot call', async () => {
  const page = await load();
  clean(page);
  // notch.js is in <head> (seam WS1) and pulls common.js in itself.
  assert.deepEqual(page.loaded, ['agentnotch/notch.js', 'agentnotch/common.js']);
  assert.equal(typeof page.window.agentnotch, 'object');
  assert.equal(typeof page.window.agentnotchCommon, 'object');
  assert.deepEqual(page.hub.of('snapshot').map((c) => c.args), [null]);
  assert.equal(page.listening('an:snapshot'), true);
  assert.equal(page.listening('an:peek'), true);
  assert.equal(page.listening('an:notice'), true);
  assert.equal(page.listening('an:panel_state'), true);
  // The hooks answer for real: two cells, one per account, from the snapshot.
  assert.deepEqual(cellsOf(page).map((c) => c.getAttribute('data-p')), [PERSONAL, WORK]);
});

test('the page loads with the scripts arriving late, and without common.js', async () => {
  // Both orders occur in a browser: the dynamic script before or after DOMContentLoaded.
  const late = await load({ lateScripts: true });
  clean(late);
  assert.deepEqual(cellsOf(late).map((c) => c.getAttribute('data-p')), [PERSONAL, WORK]);
  // A common.js that fails to load leaves upstream's own Claude cell and no error.
  const missing = await load({ missing: ['agentnotch/common.js'] });
  clean(missing);
  assert.equal(missing.window.agentnotch.claudeCells(), null);
  assert.deepEqual(cellsOf(missing).map((c) => c.getAttribute('data-p')), ['claude']);
  assert.equal(missing.window.agentnotch.ringClick('claude'), false);
  assert.equal(missing.window.agentnotch.cardSessions({ id: 'claude', snap: { windows: [] } }), '');
});

test('notch.js is one strict IIFE with one global and no top-level lexical binding', () => {
  const context = vm.createContext({});
  const window = vm.runInContext('this', context);
  const before = new Set(Object.getOwnPropertyNames(window));
  const document = dom.parseDocument('<html><head></head><body></body></html>');
  document.readyState = 'complete';
  Object.assign(window, { window, document, setTimeout, clearTimeout });
  vm.runInContext(NOTCH_JS, context);
  const added = Object.getOwnPropertyNames(window).filter((n) => !before.has(n) && !['window', 'document', 'setTimeout', 'clearTimeout'].includes(n));
  assert.deepEqual(added, ['agentnotch']);
  assert.equal(vm.runInContext("typeof state === 'undefined' && typeof C === 'undefined' && typeof CARD === 'undefined'", context), true);
  assert.doesNotMatch(NOTCH_JS, /^(let|const|class)\s/m);
  assert.match(NOTCH_JS, /^\(function \(\) \{\n {2}'use strict';/m);
  // Every fork script has this shape: a clash with an upstream global kills upstream's page.
  for (const name of fs.readdirSync(AGENTNOTCH).filter((n) => n.endsWith('.js'))) {
    const source = fs.readFileSync(path.join(AGENTNOTCH, name), 'utf8');
    assert.doesNotMatch(source, /^(let|const|class)\s/m, `${name}: a top-level lexical binding`);
    assert.doesNotMatch(source, /\beval\s*\(|new Function\s*\(/, `${name}: eval`);
  }
});

// ---- the wrapped globals ------------------------------------------------------------------------

/** What notch.js needs from upstream's script that is not declared as it expects (empty when fine). */
function missingUpstream(html, upstream) {
  const inline = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n');
  const missing = [];
  for (const name of upstream.functions) {
    // A function declaration at the top level starts a line: that is what makes it a property of
    // window, which `window.foldAllowed = …` needs to take effect for upstream's bare calls.
    if (!new RegExp(`^(async )?function\\s+${name}\\s*\\(`, 'm').test(inline)) missing.push(`function ${name}`);
  }
  for (const name of upstream.bindings) {
    if (!new RegExp(`^(?:let|const|var)\\s+(?:[^;\\n]*?[,\\s])?${name}\\s*=`, 'm').test(inline)) missing.push(`binding ${name}`);
  }
  return missing;
}

test('every upstream function notch.js wraps or calls is a top-level declaration of notch.html', async () => {
  const page = await load();
  const upstream = plain(page.window.agentnotch._.UPSTREAM);
  assert.ok(upstream.functions.includes('foldAllowed') && upstream.functions.includes('showCard'));
  assert.deepEqual(missingUpstream(NOTCH_HTML, upstream), []);
  // In the running page: a global property (only declarations are), writable so a wrapper takes.
  for (const name of upstream.functions) {
    const descriptor = plain(Object.getOwnPropertyDescriptor(page.window, name) && { writable: Object.getOwnPropertyDescriptor(page.window, name).writable });
    assert.deepEqual(descriptor, { writable: true }, `${name} is not a global function declaration`);
    assert.equal(typeof page.window[name], 'function', name);
  }
});

test('every upstream binding notch.js reads is declared at the top level of the page script', async () => {
  const page = await load();
  const upstream = plain(page.window.agentnotch._.UPSTREAM);
  for (const name of upstream.bindings) {
    const kind = page.run(`(function () { try { return typeof ${name}; } catch (e) { return 'throws'; } })()`);
    assert.notEqual(kind, 'undefined', `${name} is not visible to a later script`);
    assert.notEqual(kind, 'throws', name);
    // let and const are not properties of window; that is why notch.js reads them by bare name.
  }
  assert.equal(page.run('typeof SESSION_ROWS'), 'number');
  assert.equal(page.run('typeof INK + typeof WATCH + typeof AMPLE + typeof TRACK'), 'stringstringstringstring');
});

test('the declaration check catches an upstream rename or a conversion to a const', () => {
  const upstream = { functions: ['foldAllowed', 'showCard'], bindings: ['hoverId', 'SESSION_ROWS'] };
  assert.deepEqual(missingUpstream(NOTCH_HTML, upstream), []);
  const asConst = NOTCH_HTML.replace('function showCard(){', 'const showCard=function(){');
  assert.notEqual(asConst, NOTCH_HTML);
  assert.deepEqual(missingUpstream(asConst, upstream), ['function showCard']);
  const renamed = NOTCH_HTML.replace('function foldAllowed(){', 'function foldIsAllowed(){');
  assert.deepEqual(missingUpstream(renamed, upstream), ['function foldAllowed']);
  const moved = NOTCH_HTML.replace('const SESSION_ROWS=5;', 'const SESSION_LIMIT=5;');
  assert.deepEqual(missingUpstream(moved, upstream), ['binding SESSION_ROWS']);
});

test('the seams of notch.html call the four hooks behind a guard', () => {
  assert.match(NOTCH_HTML, /<link rel="stylesheet" href="agentnotch\/notch\.css"><script src="agentnotch\/notch\.js"><\/script><!-- Fork: WS1 -->/);
  assert.match(NOTCH_HTML, /if\(window\.agentnotch\)\{const c=agentnotch\.claudeCells\(\);if\(c\)return c;\} \/\/ Fork: WS2/);
  assert.match(NOTCH_HTML, /if\(window\.agentnotch\)agentnotch\.decorateCell\(p,cell\); \/\/ Fork: WS3/);
  assert.match(NOTCH_HTML, /window\.agentnotch&&agentnotch\.cardSessions\(p\)|agentnotch\.cardSessions\(p\)[\s\S]{0,80}Fork: WS4/);
  assert.match(NOTCH_HTML, /window\.agentnotch&&agentnotch\.ringClick\(press\.id\)[\s\S]{0,40}Fork: WS5/);
});

// ---- claudeCells ------------------------------------------------------------------------------------

test('claudeCells is null before the first snapshot, then one cell per shown ring', async () => {
  // A hub that is not running: the snapshot call fails and upstream's own Claude cell stays.
  const before = await load({ snapshot: null });
  assert.equal(before.window.agentnotch.claudeCells(), null);
  assert.deepEqual(cellsOf(before).map((c) => c.getAttribute('data-p')), ['claude']);
  // The failure is logged by code, never with the message; nothing else is refused.
  assert.deepEqual(before.errors.map(String), []);

  const page = await load();
  const cells = plain(page.window.agentnotch.claudeCells());
  assert.deepEqual(cells.map((c) => c.id), [PERSONAL, WORK]);
  const personal = cells[0];
  assert.equal(personal.base, 'claude');
  assert.equal(personal.name, 'Personal');
  assert.equal(personal.glyph, 'C');
  assert.equal(personal.snap.status, 'ok');
  assert.equal(personal.snap.backoff_until, 0);
  assert.equal(personal.snap.fetched_at, 1789999760000);
  // Window ids are the engine's own, unsuffixed, so headlineOf and weeklyOf need no account rule.
  assert.deepEqual(personal.snap.windows.map((w) => w.id), ['session', 'weekly_all', 'weekly_opus']);
  assert.equal(personal.snap.windows[0].used, 0.34);
  assert.equal(personal.snap.windows[0].resets_at, 1790007800000);
  // A new array every time: upstream pushes the other providers onto it.
  assert.notEqual(page.window.agentnotch.claudeCells(), page.window.agentnotch.claudeCells());
  const headline = plain(page.run("headlineOf(agentnotch.claudeCells()[1].snap, 'claude')"));
  const weekly = plain(page.run("weeklyOf(agentnotch.claudeCells()[1].snap, 'claude')"));
  assert.equal(headline.id, 'session');
  assert.equal(weekly.id, 'weekly_all');
  assert.deepEqual(cellsOf(page).map((c) => c.querySelector('.pct').textContent), ['34%', '72%']);
  clean(page);
});

test('a ring switched off in the notch has no cell, a ring with no reading still has one', async () => {
  const page = await loadWith((s) => {
    s.rings[0].shown = false;
    s.rings.push(Object.assign(JSON.parse(JSON.stringify(s.rings[1])), {
      ring_id: 'claude-acct-0123456789ab', label: 'New', is_default: false,
      usage: { status: 'waiting', windows: [], fetched_at_ms: 0, note: '', stale: false },
      activity: 'idle', badges: { needs_you: 0, review: 0 },
      counts: { needs_you: 0, failed: 0, review: 0, working: 0, idle: 0 }, a11y: 'New: no reading yet',
    }));
  });
  const cells = plain(page.window.agentnotch.claudeCells());
  assert.deepEqual(cells.map((c) => c.id), [WORK, 'claude-acct-0123456789ab']);
  const fresh = cells[1];
  assert.equal(fresh.snap.status, 'none');
  assert.deepEqual(fresh.snap.windows, []);
  assert.equal(fresh.snap.note, 'Waiting for the first reading…');
  // …and it is drawn: a cell whose reading has not arrived is a dash in the pill.
  const cell = cellFor(page, 'claude-acct-0123456789ab');
  assert.ok(cell);
  assert.equal(cell.querySelector('.pct').textContent, '—');
  assert.equal(cellFor(page, PERSONAL), undefined);
  clean(page);
});

test('the engine statuses map to upstream ones, and never to a sign-in prompt', async () => {
  const page = await load();
  const map = (status) => page.window.agentnotch._.upstreamStatus(status);
  assert.deepEqual(['ok', 'stale', 'waiting', 'sign_in_needed', 'unavailable', 'failed', 'unheard'].map(map),
    ['ok', 'stale', 'none', 'none', 'none', 'error', 'none']);
  // Upstream's `needsAuth` draws "Sign in to Claude Code" and a button: there is no token here.
  assert.equal([map('sign_in_needed'), map('waiting')].includes('needsAuth'), false);
});

test('an:snapshot redraws the rings, and ignores what is not a snapshot', async () => {
  const page = await load();
  const next = snapshotWith((s) => {
    s.rings[0].usage.windows[0].used = 0.9;
    s.rings.splice(1, 1);
  });
  page.emit('an:snapshot', next);
  assert.deepEqual(cellsOf(page).map((c) => c.getAttribute('data-p')), [PERSONAL]);
  assert.equal(cellsOf(page)[0].querySelector('.pct').textContent, '90%');
  page.emit('an:snapshot', { rings: 'nope' });
  page.emit('an:snapshot', null);
  page.emit('an:snapshot', { rings: [], sessions: null });
  assert.equal(cellsOf(page)[0].querySelector('.pct').textContent, '90%');
  clean(page);
});

// ---- decorateCell: the activity layer ---------------------------------------------------------------------

const activityMarkup = (page, id) => cellFor(page, id).querySelector('svg.activity').innerHTML;

test('activity: working turns an arc in ink, in steps', async () => {
  const page = await loadWith((s) => { s.rings[0].activity = 'working'; });
  const markup = activityMarkup(page, PERSONAL);
  assert.match(markup, /<g class="arc-spin">/);
  assert.ok(markup.includes(`stroke="${page.run('INK')}"`));
  // A short arc as on the Mac (ActivityArc: a quarter, upstream draws .28), not a full ring.
  assert.match(markup, /stroke-dasharray="(\d+\.\d+) (\d+\.\d+)"/);
  const [, dash, whole] = markup.match(/stroke-dasharray="(\d+\.\d+) (\d+\.\d+)"/);
  assert.ok(Math.abs(dash / whole - 0.28) < 0.001);
  clean(page);
});

test('activity: waiting is an amber ring that pulses, idle draws nothing', async () => {
  const page = await loadWith((s) => { s.rings[1].activity = 'idle'; });
  const waiting = activityMarkup(page, PERSONAL);
  assert.match(waiting, /<g class="arc-pulse">/);
  assert.ok(waiting.includes(`stroke="${page.run('WATCH')}"`));
  assert.doesNotMatch(waiting, /stroke-dasharray/);
  assert.equal(activityMarkup(page, WORK), '');
  clean(page);
});

test('activity: success pulses green until success_settles_at_ms, then holds steady at .85', async () => {
  const settles = NOW + 90000;
  const page = await loadWith((s) => {
    s.rings[0].activity = 'success';
    s.rings[0].success_settles_at_ms = settles;
  });
  const ample = page.run('AMPLE');
  const pulsing = activityMarkup(page, PERSONAL);
  assert.match(pulsing, /<g class="arc-pulse">/);
  assert.ok(pulsing.includes(`stroke="${ample}"`));
  // Just before the deadline: still pulsing. At it: steady, with no page event in between (a
  // timer of the notch's own redraws the ring).
  page.tick(89000);
  assert.match(activityMarkup(page, PERSONAL), /arc-pulse/);
  page.tick(1100);
  const steady = activityMarkup(page, PERSONAL);
  assert.doesNotMatch(steady, /arc-pulse/);
  assert.match(steady, /opacity="0.85"/);
  assert.ok(steady.includes(`stroke="${ample}"`));
  clean(page);
});

test('activityOf: the deadline is "now >= settles"; no deadline never settles', async () => {
  const page = await load();
  const of = (ring, now) => page.window.agentnotch._.activityOf(ring, now);
  assert.equal(of({ activity: 'success', success_settles_at_ms: 1000 }, 999), 'success');
  assert.equal(of({ activity: 'success', success_settles_at_ms: 1000 }, 1000), 'success_steady');
  assert.equal(of({ activity: 'success', success_settles_at_ms: 1000 }, 5000), 'success_steady');
  assert.equal(of({ activity: 'success', success_settles_at_ms: null }, 5000), 'success');
  assert.equal(of({ activity: 'working' }, 0), 'working');
  assert.equal(of({ activity: 'waiting' }, 0), 'waiting');
  assert.equal(of({ activity: 'idle' }, 0), 'idle');
  assert.equal(of({ activity: 'something new' }, 0), 'idle');
  assert.equal(of({}, 0), 'idle');
});

test('a snapshot that arrives after the settle time draws steady at once, and its timer is cleared', async () => {
  const page = await load();
  page.tick(200000);
  const stale = snapshotWith((s) => {
    s.rings[0].activity = 'success';
    s.rings[0].success_settles_at_ms = NOW + 90000;
  });
  page.emit('an:snapshot', stale);
  assert.doesNotMatch(activityMarkup(page, PERSONAL), /arc-pulse/);
  assert.match(activityMarkup(page, PERSONAL), /opacity="0.85"/);
  // A newer snapshot replaces the timer instead of stacking one.
  page.emit('an:snapshot', snapshotWith((s) => { s.rings[0].activity = 'waiting'; }));
  assert.match(activityMarkup(page, PERSONAL), /arc-pulse/);
  clean(page);
});

test('decorateCell leaves a cell of another provider alone', async () => {
  const page = await load();
  const cell = dom.parseFragment('<div class="cell"><div class="ringwrap"><svg class="activity"><circle/></svg></div></div>').querySelector('.cell');
  page.window.agentnotch.decorateCell({ id: 'codex', base: 'codex' }, cell);
  page.window.agentnotch.decorateCell({ id: 'claude', base: 'claude' }, cell); // not a ring of the snapshot
  assert.equal(cell.querySelector('svg.activity').innerHTML, '<circle></circle>');
  assert.equal(cell.querySelector('.ringwrap').classList.contains('stale'), false);
  assert.equal(cell.getAttribute('aria-label'), null);
  assert.equal(cell.querySelectorAll('.an-badge').length, 0);
});

// ---- decorateCell: the stale class ----------------------------------------------------------------------

test('the stale class comes from the engine, over upstream\'s 15-minute rule', async () => {
  const oneHourAgo = NOW - 3600 * 1000;
  const page = await loadWith((s) => {
    s.rings[0].usage.fetched_at_ms = oneHourAgo; // upstream would dim this
    s.rings[0].usage.stale = false;
    s.rings[1].usage.fetched_at_ms = NOW - 1000; // upstream would not dim this
    s.rings[1].usage.stale = true;
  });
  const wrap = (id) => cellFor(page, id).querySelector('.ringwrap');
  assert.equal(wrap(PERSONAL).classList.contains('stale'), false);
  assert.equal(wrap(WORK).classList.contains('stale'), true);
  // Back again on the next snapshot.
  page.emit('an:snapshot', snapshotWith());
  assert.equal(wrap(WORK).classList.contains('stale'), false);
  clean(page);
});

// ---- decorateCell: badges -------------------------------------------------------------------------------

const BADGE_ANGLES = { right: [225, 135], left: [315, 45], top: [135, 45], bottom: [225, 315] };
const rad = (deg) => (deg * Math.PI) / 180;

test('badge angles are the design table (C_RingBadgeLayoutTests.angleTableMatchesTheDesign)', async () => {
  const page = await load();
  for (const edge of EDGES) {
    assert.equal(page.window.agentnotch._.badgeAngle('needs', edge), BADGE_ANGLES[edge][0], edge);
    assert.equal(page.window.agentnotch._.badgeAngle('review', edge), BADGE_ANGLES[edge][1], edge);
  }
});

test('badges face the screen centre, needs-you leads, and the two never share a spot', async () => {
  const page = await load();
  const centre = (slot, edge, ring) => plain(page.window.agentnotch._.badgeCentre(slot, edge, ring));
  const footprint = (slot, edge) => {
    const c = centre(slot, edge, 44);
    // capsule 18 wide (the widest, "9+") by 12, plus the 1.5 knockout
    return { left: c.x - 9 - 1.5, right: c.x + 9 + 1.5, top: c.y - 6 - 1.5, bottom: c.y + 6 + 1.5 };
  };
  const overlap = (a, b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
  for (const edge of EDGES) {
    for (const slot of ['needs', 'review']) {
      const c = centre(slot, edge, 44);
      if (edge === 'right') assert.ok(c.x < 22, `${edge} ${slot}`);
      if (edge === 'left') assert.ok(c.x > 22, `${edge} ${slot}`);
      if (edge === 'top') assert.ok(c.y > 22, `${edge} ${slot}`);
      if (edge === 'bottom') assert.ok(c.y < 22, `${edge} ${slot}`);
    }
    assert.equal(overlap(footprint('needs', edge), footprint('review', edge)), false, edge);
    const n = centre('needs', edge, 44);
    const r = centre('review', edge, 44);
    if (edge === 'right' || edge === 'left') assert.ok(n.y < r.y, edge);
    else assert.ok(n.x < r.x, edge);
  }
});

test('badge centres sit 11 outside the ring, in proportion for another size (r = 33 in the 44 frame)', async () => {
  const page = await load();
  const centre = (slot, edge, ring) => plain(page.window.agentnotch._.badgeCentre(slot, edge, ring));
  for (const edge of EDGES) {
    for (const [i, slot] of ['needs', 'review'].entries()) {
      const c = centre(slot, edge, 44);
      assert.ok(Math.abs(Math.hypot(c.x - 22, c.y - 22) - 33) < 0.001, `${edge} ${slot}`);
      const a = rad(BADGE_ANGLES[edge][i]);
      assert.ok(Math.abs(c.x - (22 + 33 * Math.cos(a))) < 0.001 && Math.abs(c.y - (22 + 33 * Math.sin(a))) < 0.001);
    }
  }
  // The number the earlier probe printed for the right edge's review badge.
  const review = centre('review', 'right', 44);
  assert.equal(review.x.toFixed(2), '-1.33');
  assert.equal(review.y.toFixed(2), '45.33');
  const big = centre('review', 'right', 88);
  assert.ok(Math.abs(Math.hypot(big.x - 44, big.y - 44) - 66) < 0.001);
});

test('count badges clear the weekly ring drawn outside the track (GUX-12), on the real ring geometry', async () => {
  const page = await load();
  const centre = (slot, edge) => plain(page.window.agentnotch._.badgeCentre(slot, edge, 44));
  // The page's outside weekly ring: r = 31 and a 2.4 stroke in a 56-unit box scaled to 44.
  const weeklyOuter = ((31 + 2.4 / 2) * 44) / 56;
  const mac = (65 * 44) / 117 + (5 * 44) / 117 / 2; // weeklyOutsideRadius + stroke / 2
  assert.ok(Math.abs(weeklyOuter - mac) < 0.1, `${weeklyOuter} vs ${mac}`);
  for (const edge of EDGES) {
    for (const slot of ['needs', 'review']) {
      const c = centre(slot, edge);
      const nearest = Math.hypot(c.x - 22, c.y - 22) - 6 - 1.5;
      assert.ok(nearest >= mac - 0.001, `${edge} ${slot}: ${nearest}`);
    }
  }
});

test('side-edge badges stay inside the body and above the label; flat-edge ones keep to their cell', async () => {
  const page = await load();
  const centre = (slot, edge) => plain(page.window.agentnotch._.badgeCentre(slot, edge, 44));
  const foot = (slot, edge) => {
    const c = centre(slot, edge);
    return { minX: c.x - 10.5, maxX: c.x + 10.5, minY: c.y - 7.5, maxY: c.y + 7.5 };
  };
  const labelGap = (26.9 * 44) / 117;
  const sideBodyDepth = (186 * 44) / 117;
  const cellSpacing = (83.5 * 44) / 117;
  const ringMargin = (sideBodyDepth - 44) / 2;
  for (const edge of ['right', 'left']) {
    for (const slot of ['needs', 'review']) {
      const b = foot(slot, edge);
      assert.ok(b.minX >= 22 - sideBodyDepth / 2 && b.maxX <= 22 + sideBodyDepth / 2, `${edge} ${slot}`);
      assert.ok(b.maxY < 44 + labelGap, `${edge} ${slot} runs into the label`);
      assert.ok(b.minY > -cellSpacing, `${edge} ${slot} runs into the cell above`);
    }
  }
  for (const edge of ['top', 'bottom']) {
    for (const slot of ['needs', 'review']) {
      const b = foot(slot, edge);
      assert.ok(b.minX > -cellSpacing / 2 && b.maxX < 44 + cellSpacing / 2, `${edge} ${slot}`);
      assert.ok(b.maxY < 44 + labelGap, `${edge} ${slot} runs into the label`);
      assert.ok(b.minY > -ringMargin, `${edge} ${slot} leaves the body`);
    }
  }
});

test('badge labels: none for 0, 1..9, "9+" from ten', async () => {
  const page = await loadWith((s) => {
    s.rings[0].badges = { needs_you: 0, review: 1 };
    s.rings[1].badges = { needs_you: 9, review: 10 };
  });
  const labels = (id) => cellFor(page, id).querySelectorAll('.an-badge').map((b) => `${b.className.split(' ')[1]}:${b.textContent}`);
  assert.deepEqual(labels(PERSONAL), ['an-badge-review:1']);
  assert.deepEqual(labels(WORK), ['an-badge-needs:9', 'an-badge-review:9+']);
  page.emit('an:snapshot', snapshotWith((s) => { s.rings[0].badges = { needs_you: 250, review: 0 }; }));
  assert.deepEqual(labels(PERSONAL), ['an-badge-needs:9+']);
  page.emit('an:snapshot', snapshotWith((s) => { s.rings[0].badges = { needs_you: -1, review: 0 }; }));
  assert.deepEqual(labels(PERSONAL), []);
  clean(page);
});

test('badge placement per edge on the page: needs left/right/top/bottom as the table says', async () => {
  for (const edge of EDGES) {
    const page = await load({ edge });
    assert.equal(page.document.body.getAttribute('data-edge') || 'right', edge);
    const wrap = cellFor(page, PERSONAL).querySelector('.ringwrap');
    for (const [i, slot] of ['needs', 'review'].entries()) {
      const a = rad(BADGE_ANGLES[edge][i]);
      const el = wrap.querySelector(`.an-badge-${slot}`);
      assert.ok(el, `${edge} ${slot}`);
      assert.equal(el.style.getPropertyValue('left'), (22 + 33 * Math.cos(a)).toFixed(2) + 'px', `${edge} ${slot} x`);
      assert.equal(el.style.getPropertyValue('top'), (22 + 33 * Math.sin(a)).toFixed(2) + 'px', `${edge} ${slot} y`);
    }
    clean(page);
  }
});

test('a badge takes the ring\'s real size from its box when the page has one', async () => {
  const page = await load({ edge: 'right' });
  const wrap = cellFor(page, PERSONAL).querySelector('.ringwrap');
  wrap.__rect = { left: 0, top: 0, width: 88, height: 88 };
  Object.defineProperty(wrap, 'offsetWidth', { value: 88 });
  page.emit('an:snapshot', snapshotWith());
  const review = wrap.querySelector('.an-badge-review');
  assert.equal(review.style.getPropertyValue('left'), (44 + 66 * Math.cos(rad(135))).toFixed(2) + 'px');
});

test('the badges follow the notch when it moves to another edge, with no snapshot in between', async () => {
  const page = await load({ edge: 'right' });
  const needs = () => cellFor(page, PERSONAL).querySelector('.an-badge-needs');
  assert.equal(needs().style.getPropertyValue('left'), '-1.33px');
  // The main process moves the notch: upstream's `notch_edge` handler sets the attribute.
  page.emit('notch_edge', 'bottom');
  await page.settle();
  assert.equal(needs().style.getPropertyValue('left'), '-1.33px'); // 225°: up-left on the bottom edge as well
  page.emit('notch_edge', 'left');
  await page.settle();
  assert.equal(needs().style.getPropertyValue('left'), '45.33px'); // 315°: up-right
  assert.equal(needs().style.getPropertyValue('top'), '-1.33px');
  clean(page);
});

test('ui.ring_badges off: no badges, and they come back when it is on again', async () => {
  const page = await loadWith((s) => { s.ui.ring_badges = false; });
  assert.equal(page.$$('.an-badge').length, 0);
  page.emit('an:snapshot', snapshotWith());
  assert.equal(page.$$('.an-badge').length, 4);
  page.emit('an:snapshot', snapshotWith((s) => { s.ui.ring_badges = false; }));
  assert.equal(page.$$('.an-badge').length, 0);
  clean(page);
});

test('the cell says what the ring says: aria-label is ring.a11y', async () => {
  const page = await load();
  const snapshot = harness.fixture('snapshot.json');
  for (const ring of snapshot.rings) {
    assert.equal(cellFor(page, ring.ring_id).getAttribute('aria-label'), ring.a11y);
  }
  // Badges are decoration for the label, never read twice.
  assert.ok(page.$$('.an-badge').every((b) => b.getAttribute('aria-hidden') === 'true'));
});

test('a badge takes no click: pointer-events none, on top of the ring, in the pill\'s colour', () => {
  const rule = NOTCH_CSS.match(/\.ringwrap > \.an-badge\{[^}]*\}/)[0];
  assert.match(rule, /pointer-events:none/);
  assert.match(rule, /height:12px/);
  assert.match(rule, /min-width:12px/);
  assert.match(rule, /max-width:18px/);
  assert.match(rule, /font:700 9px/);
  assert.match(rule, /box-shadow:0 0 0 1\.5px var\(--pill\)/);
  assert.match(NOTCH_CSS, /\.an-badge-needs\{background:var\(--watch\)\}/);
  assert.match(NOTCH_CSS, /\.an-badge-review\{background:var\(--ample\)\}/);
});

// ---- cardSessions ------------------------------------------------------------------------------------------

const rowsOf = (page) => page.$$('#card [data-an-session]');

test('card rows come from each session\'s card: name, status word, detail or waiting line, elapsed', async () => {
  const page = await load();
  openCard(page, PERSONAL);
  assert.ok(isShown(page));
  const rows = rowsOf(page);
  const ids = rows.map((r) => r.getAttribute('data-an-session'));
  // Waiting first (newest first), then working, then done, then idle: 8 sessions on this ring.
  assert.deepEqual(ids, [
    'needs-elicitation', 'needs-question', 'needs-plan', // waiting, newest first
    'work-ci', // working
    'review-just-finished', 'review-darkmode', // done
    'idle-notch', 'idle-logo', // idle
  ]);
  // Every row is a button, and what it shows is what `card` carries.
  const byId = Object.fromEntries(rows.map((r) => [r.getAttribute('data-an-session'), r]));
  assert.ok(rows.every((r) => r.localName === 'button' && r.getAttribute('type') === 'button'));
  const text = (id, cls) => byId[id].querySelector(cls).textContent;
  assert.equal(text('needs-question', '.an-srow-name'), 'VS Code · dashboard');
  assert.equal(text('needs-question', '.an-srow-word'), 'waiting');
  // Waiting shows what it waits for, in place of the detail.
  assert.equal(text('needs-question', '.an-srow-detail'), 'Question · Charts');
  assert.equal(text('needs-question', '.an-srow-since'), '6 min');
  assert.equal(text('work-ci', '.an-srow-word'), 'working');
  assert.equal(text('work-ci', '.an-srow-detail'), '9/15 Bisecting the failing commit · ctx 93%');
  assert.equal(text('review-just-finished', '.an-srow-word'), 'complete');
  assert.equal(text('review-just-finished', '.an-srow-since'), 'just now');
  assert.equal(text('idle-logo', '.an-srow-word'), 'idle');
  assert.equal(text('idle-logo', '.an-srow-detail'), '');
  assert.equal(text('idle-logo', '.an-srow-since'), '26 hr');
  clean(page);
});

test('card rows are ranked waiting, failed, working, done, idle; newest first within each', async () => {
  const page = await load();
  const html = page.window.agentnotch._.cardRowsHtml(snapshotWith(), WORK, NOW, 20);
  const list = dom.parseFragment(html);
  const ids = list.querySelectorAll('[data-an-session]').map((b) => b.getAttribute('data-an-session'));
  assert.deepEqual(ids, [
    'needs-permission', // waiting
    'needs-ratelimit', // failed
    'work-summary', 'work-migration', // working, newest first
    'review-devserver', // done
    'idle-readme', 'idle-deps', // idle, newest first
  ]);
  const failed = list.querySelector('[data-an-session="needs-ratelimit"]');
  assert.equal(failed.querySelector('.an-srow-word').textContent, 'failed');
  assert.equal(failed.querySelector('.an-srow-detail').textContent, 'Rate limited');
  assert.match(failed.getAttribute('aria-label'), /failed, Rate limited/);
});

test('a session with no known ring counts on the default ring, and nowhere else', async () => {
  const snapshot = snapshotWith((s) => {
    s.sessions.find((x) => x.session_id === 'work-ci').ring_id = null;
  });
  const page = await load({ snapshot });
  const ids = (ring) => dom.parseFragment(page.window.agentnotch._.cardRowsHtml(snapshot, ring, NOW, 20))
    .querySelectorAll('[data-an-session]').map((b) => b.getAttribute('data-an-session'));
  assert.ok(ids(PERSONAL).includes('work-ci')); // the default ring
  assert.equal(ids(WORK).includes('work-ci'), false);
});

test('a ring with no sessions draws no list at all', async () => {
  const snapshot = snapshotWith((s) => { s.sessions = s.sessions.filter((x) => x.ring_id !== WORK); });
  const page = await load({ snapshot });
  assert.equal(page.window.agentnotch._.cardRowsHtml(snapshot, WORK, NOW, 5), '');
  openCard(page, WORK);
  assert.equal(page.$$('#card .c-sessions').length, 0);
  assert.equal(rowsOf(page).length, 0);
  clean(page);
});

test('"and N more" follows the rows that fit (NotchSessionCapTests)', async () => {
  const page = await load();
  const html = page.window.agentnotch._.cardRowsHtml(snapshotWith(), PERSONAL, NOW, 5);
  const list = dom.parseFragment(html);
  assert.equal(list.querySelectorAll('[data-an-session]').length, 5);
  assert.equal(list.querySelector('.s-more').textContent, 'and 3 more');
  // Everything fits: no "more" line.
  const all = dom.parseFragment(page.window.agentnotch._.cardRowsHtml(snapshotWith(), PERSONAL, NOW, 8));
  assert.equal(all.querySelectorAll('[data-an-session]').length, 8);
  assert.equal(all.querySelector('.s-more'), null);
  // One row is the least a card shows.
  const one = dom.parseFragment(page.window.agentnotch._.cardRowsHtml(snapshotWith(), PERSONAL, NOW, 0));
  assert.equal(one.querySelectorAll('[data-an-session]').length, 5); // 0 is "unknown": upstream's five
  const least = dom.parseFragment(page.window.agentnotch._.cardRowsHtml(snapshotWith(), PERSONAL, NOW, 1));
  assert.equal(least.querySelectorAll('[data-an-session]').length, 1);
  assert.equal(least.querySelector('.s-more').textContent, 'and 7 more');
});

test('session cap: a two-window card lists more than a four-window one, and every card fits the window', async () => {
  const page = await load();
  const cap = (total, windows, edge, height) => page.window.agentnotch._.sessionCap(total, windows, page.window.agentnotch._.cardBudget(edge, height, null), 0);
  for (const edge of EDGES) {
    const two = cap(20, 2, edge, 982);
    const four = cap(20, 4, edge, 982);
    assert.ok(two > four, `${edge}: ${two} vs ${four}`);
    assert.ok(two >= 3, edge);
    // A card with more windows never gets more rows than a smaller one.
    assert.ok(cap(20, 6, edge, 982) <= four, edge);
    // And the tallest it is ever drawn (cap rows and "and N more") fits the room the page gives it.
    for (const windows of [0, 1, 2, 3, 4, 6]) {
      for (const height of [400, 650, 982, 1440]) {
        const budget = page.window.agentnotch._.cardBudget(edge, height, [0, 0, 40, 0]);
        const rows = page.window.agentnotch._.sessionCap(30, windows, budget, 0);
        const C = plain(page.window.agentnotch._.CARD);
        const tallest = C.padding + C.head + windows * C.window + C.list + rows * C.row + C.more;
        assert.ok(rows === 1 || tallest <= budget + 0.5, `${edge} ${windows} windows in ${height}: ${tallest} > ${budget}`);
      }
    }
  }
  // Before the window's height is known, the shipped default holds.
  assert.equal(page.window.agentnotch._.sessionCap(20, 2, 0, 0), 5);
  assert.equal(page.window.agentnotch._.sessionCap(20, 2, NaN, 0), 5);
  // A stale reading adds its "Updated" line and so can only cost a row.
  const budget = page.window.agentnotch._.cardBudget('right', 650, null);
  assert.ok(page.window.agentnotch._.sessionCap(20, 3, budget, 16) <= page.window.agentnotch._.sessionCap(20, 3, budget, 0));
});

test('the card lists as many rows as its window has room for, taskbar included', async () => {
  const shortWindow = await load({ height: 500 });
  openCard(shortWindow, PERSONAL);
  const short = rowsOf(shortWindow).length;
  const tallWindow = await load({ height: 900 });
  openCard(tallWindow, PERSONAL);
  const tall = rowsOf(tallWindow).length;
  assert.ok(short >= 1 && short < tall, `${short} vs ${tall}`);
  assert.equal(tall, 8); // all of this ring's sessions
  assert.equal(shortWindow.$$('#card .s-more').length, 1);
  assert.equal(tallWindow.$$('#card .s-more').length, 0);
  clean(shortWindow);
});

test('the card redraws with the next snapshot while it is shown', async () => {
  const page = await load();
  openCard(page, PERSONAL);
  assert.equal(rowsOf(page).find((r) => r.getAttribute('data-an-session') === 'work-ci').querySelector('.an-srow-word').textContent, 'working');
  page.emit('an:snapshot', snapshotWith((s) => {
    const row = s.sessions.find((x) => x.session_id === 'work-ci');
    row.card.state = 'review';
    row.card.detail = 'All done';
  }));
  const row = rowsOf(page).find((r) => r.getAttribute('data-an-session') === 'work-ci');
  assert.equal(row.querySelector('.an-srow-word').textContent, 'complete');
  assert.equal(row.querySelector('.an-srow-detail').textContent, 'All done');
  clean(page);
});

// ---- ringClick --------------------------------------------------------------------------------------

function setRects(page) {
  // Layout is the browser's; the click reads the ring's box, so give it one.
  cellsOf(page).forEach((cell, i) => {
    cell.__rect = { left: 280, top: 100 + i * 80, width: 44, height: 70 };
    cell.querySelector('.ringwrap').__rect = { left: 280.4, top: 100.2 + i * 80, width: 44, height: 44 };
  });
}

test('ring_click openPanel: panel_toggle with the ring, its rectangle in physical pixels, and the reason', async () => {
  const page = await load({ dpr: 1.5 });
  setRects(page);
  assert.equal(page.window.agentnotch.ringClick(WORK), true);
  await page.settle();
  assert.deepEqual(page.hub.of('panel_toggle').map((c) => c.args), [
    { ring_id: WORK, rect: [Math.round(280.4 * 1.5), Math.round(180.2 * 1.5), 66, 66], reason: 'ring_click' },
  ]);
  // Nothing else: not upstream's refresh, not the engine's.
  assert.equal(page.hub.of('refresh_usage').length, 0);
  assert.equal(page.upstreamCalls.filter((c) => c.cmd === 'refresh_ring').length, 0);
  clean(page);
});

test('ring_click openPanel through a real press and release on the pill (seam WS5)', async () => {
  const page = await load();
  setRects(page);
  page.fire('#pill', 'mousedown', { button: 0, clientX: 300, clientY: 130 });
  page.fire(page.document, 'mouseup', { button: 0, clientX: 300, clientY: 130 });
  await page.settle();
  assert.deepEqual(page.hub.of('panel_toggle').map((c) => c.args.ring_id), [PERSONAL]);
  assert.equal(page.upstreamCalls.filter((c) => c.cmd === 'refresh_ring').length, 0);
  clean(page);
});

test('ring_click refreshUsage: refresh_usage for the ring, upstream\'s press, and it settles when the reading lands', async () => {
  const page = await loadWith((s) => { s.ui.ring_click = 'refreshUsage'; }, { replies: { refresh_usage: { coming: true } } });
  setRects(page);
  assert.equal(page.window.agentnotch.ringClick(PERSONAL), true);
  assert.deepEqual(page.hub.of('refresh_usage').map((c) => c.args), [{ ring_id: PERSONAL, reason: 'ring_click' }]);
  assert.equal(page.hub.of('panel_toggle').length, 0);
  assert.equal(page.upstreamCalls.filter((c) => c.cmd === 'refresh_ring').length, 0);
  await page.settle();
  const wrap = () => cellFor(page, PERSONAL).querySelector('.ringwrap');
  // Pressed in until the reading lands (a probe is coming)…
  assert.equal(wrap().classList.contains('pressed'), true);
  assert.equal(page.run('Object.keys(refreshing).join()'), PERSONAL);
  // …a second click meanwhile asks nothing more…
  page.window.agentnotch.ringClick(PERSONAL);
  assert.equal(page.hub.of('refresh_usage').length, 1);
  // …and the next snapshot with a newer reading lets go (after upstream's minimum press).
  page.tick(400);
  page.emit('an:snapshot', snapshotWith((s) => { s.rings[0].usage.fetched_at_ms = NOW; }));
  page.tick(400);
  assert.equal(wrap().classList.contains('pressed'), false);
  assert.equal(page.run('Object.keys(refreshing).length'), 0);
  clean(page);
});

test('ring_click refreshUsage: nothing coming settles at once; a failed call settles too', async () => {
  const none = await loadWith((s) => { s.ui.ring_click = 'refreshUsage'; }, { replies: { refresh_usage: { coming: false } } });
  none.window.agentnotch.ringClick(PERSONAL);
  await none.settle();
  none.tick(400);
  assert.equal(none.run('Object.keys(refreshing).length'), 0);
  const failed = await loadWith((s) => { s.ui.ring_click = 'refreshUsage'; }, { replies: { refresh_usage: () => { throw { code: 'failed', message: 'no' }; } } });
  failed.window.agentnotch.ringClick(PERSONAL);
  await failed.settle();
  failed.tick(400);
  assert.equal(failed.run('Object.keys(refreshing).length'), 0);
  clean(none);
  clean(failed);
});

test('ringClick is false for a cell that is not a Claude ring', async () => {
  const page = await load();
  assert.equal(page.window.agentnotch.ringClick('codex'), false);
  assert.equal(page.window.agentnotch.ringClick('claude'), false);
  assert.equal(page.window.agentnotch.ringClick(undefined), false);
  assert.equal(page.hub.of('panel_toggle').length + page.hub.of('refresh_usage').length, 0);
  // Before the first snapshot even a Claude id is not ours.
  const early = await load({ snapshot: null });
  assert.equal(early.window.agentnotch.ringClick(PERSONAL), false);
});

test('a ring click guesses what the glue will do until it says: open, or close on the same ring', async () => {
  const page = await load();
  setRects(page);
  const n = page.window.agentnotch;
  assert.equal(n._.panelOpen(), false);
  n.ringClick(PERSONAL);
  assert.equal(n._.panelOpen(), true); // the card must not flash up over the opening panel
  // The glue's word replaces the guess.
  page.emit('an:panel_state', { open: true, ring_id: PERSONAL });
  assert.equal(n._.panelOpen(), true);
  // The same ring again closes.
  n.ringClick(PERSONAL);
  assert.equal(n._.panelOpen(), false);
  page.emit('an:panel_state', { open: false, ring_id: null });
  // Another ring while one is open moves the panel: still open.
  page.emit('an:panel_state', { open: true, ring_id: PERSONAL });
  n.ringClick(WORK);
  assert.equal(n._.panelOpen(), true);
  // A guess is short-lived: with no word from the glue it lapses.
  page.emit('an:panel_state', { open: false, ring_id: null });
  n.ringClick(WORK);
  page.tick(2000);
  assert.equal(n._.panelOpen(), false);
  assert.deepEqual(page.hub.of('panel_toggle').map((c) => c.args.ring_id), [PERSONAL, PERSONAL, WORK, WORK]);
  clean(page);
});

test('a refused panel_toggle drops the guess', async () => {
  const page = await load({ replies: { panel_toggle: () => { throw { code: 'failed', message: 'no window' }; } } });
  setRects(page);
  page.window.agentnotch.ringClick(PERSONAL);
  await page.settle();
  assert.equal(page.window.agentnotch._.panelOpen(), false);
  assert.ok(page.hub.of('log').every((c) => !/no window/.test(JSON.stringify(c.args)))); // codes only, never messages
  clean(page);
});

// ---- hover_click ------------------------------------------------------------------------------------------

test('hover_click smart: a session that needs you opens the panel at it', async () => {
  const page = await load();
  openCard(page, WORK);
  await clickRow(page, 'needs-permission');
  assert.deepEqual(page.hub.of('panel_open').map((c) => c.args), [
    { route: 'session:needs-permission', reason: 'hover_row', ring_id: WORK },
  ]);
  assert.equal(page.hub.of('focus').length, 0);
  assert.equal(isShown(page), false); // the panel takes over from the card
  clean(page);
});

test('hover_click smart: any other session jumps to its terminal; the panel only if that fails', async () => {
  const page = await load();
  openCard(page, PERSONAL);
  await clickRow(page, 'review-just-finished');
  assert.deepEqual(page.hub.of('focus').map((c) => c.args), [{ session_id: 'review-just-finished' }]);
  assert.equal(page.hub.of('panel_open').length, 0);
  page.hub.clear();
  for (const outcome of ['not_found', 'failed']) {
    page.hub.replies.focus = { outcome };
    openCard(page, PERSONAL);
    await clickRow(page, 'review-darkmode');
    assert.deepEqual(page.hub.of('panel_open').map((c) => c.args.route), ['session:review-darkmode'], outcome);
    page.hub.clear();
  }
  page.hub.replies.focus = { outcome: 'raised_only' };
  openCard(page, PERSONAL);
  await clickRow(page, 'review-darkmode');
  assert.equal(page.hub.of('panel_open').length, 0);
  page.hub.replies.focus = () => { throw { code: 'failed', message: 'x' }; };
  openCard(page, PERSONAL);
  await clickRow(page, 'review-darkmode');
  assert.equal(page.hub.of('panel_open').length, 1);
  clean(page);
});

test('hover_click smart: a session with no terminal to show opens the panel without asking', async () => {
  const page = await load();
  openCard(page, PERSONAL);
  await clickRow(page, 'idle-logo'); // focus_label null
  assert.equal(page.hub.of('focus').length, 0);
  assert.deepEqual(page.hub.of('panel_open').map((c) => c.args.route), ['session:idle-logo']);
  clean(page);
});

test('hover_click panel: always the panel, even for a working session', async () => {
  const page = await loadWith((s) => { s.ui.hover_click = 'panel'; });
  openCard(page, PERSONAL);
  await clickRow(page, 'work-ci');
  assert.equal(page.hub.of('focus').length, 0);
  assert.deepEqual(page.hub.of('panel_open').map((c) => c.args.route), ['session:work-ci']);
  clean(page);
});

test('hover_click terminal: always the terminal, even for one that needs you; the panel when there is none', async () => {
  const page = await loadWith((s) => { s.ui.hover_click = 'terminal'; });
  openCard(page, WORK);
  await clickRow(page, 'needs-permission');
  assert.deepEqual(page.hub.of('focus').map((c) => c.args.session_id), ['needs-permission']);
  assert.equal(page.hub.of('panel_open').length, 0);
  page.hub.clear();
  openCard(page, PERSONAL);
  await clickRow(page, 'idle-logo');
  assert.equal(page.hub.of('focus').length, 0);
  assert.equal(page.hub.of('panel_open').length, 1);
  clean(page);
});

test('a click just after a peek is the peek\'s: reason peek_click', async () => {
  const page = await load();
  page.emit('an:peek', { ring_id: WORK, seconds: 3 });
  assert.ok(isShown(page));
  await clickRow(page, 'needs-permission');
  assert.deepEqual(page.hub.of('panel_open').map((c) => c.args.reason), ['peek_click']);
  // Long after it is a plain hover click again.
  page.hub.clear();
  page.tick(3000 + 2100);
  openCard(page, WORK);
  await clickRow(page, 'needs-permission');
  assert.deepEqual(page.hub.of('panel_open').map((c) => c.args.reason), ['hover_row']);
  clean(page);
});

test('a click on the card outside a row does nothing; a row click does not reach upstream\'s card handlers', async () => {
  const page = await load();
  openCard(page, PERSONAL);
  page.click('#card .c-title');
  await page.settle();
  assert.equal(page.hub.calls.filter((c) => c.method !== 'snapshot').length, 0);
  const event = page.fire(rowsOf(page)[0], 'click');
  assert.equal(event.defaultPrevented, true);
  await page.settle();
  clean(page);
});

// ---- the wrappers of foldAllowed and showCard -------------------------------------------------------------

test('foldAllowed: upstream\'s rule, and false while a session needs you under hold_open always', async () => {
  const held = await loadWith((s) => { s.ui.hold_open = 'always'; }, { onHover: true });
  assert.equal(held.run('foldAllowed()'), false);
  assert.equal(held.window.foldAllowed.__agentnotch, true);
  const never = await loadWith((s) => { s.ui.hold_open = 'never'; }, { onHover: true });
  assert.equal(never.run('foldAllowed()'), true);
  // hold_open always, but nothing needs you on a shown ring: not held.
  const calm = await loadWith((s) => {
    s.ui.hold_open = 'always';
    for (const ring of s.rings) ring.counts = { needs_you: 0, failed: 0, review: 0, working: 1, idle: 0 };
  }, { onHover: true });
  assert.equal(calm.run('foldAllowed()'), true);
  // A ring switched off in the notch holds nothing (C_NotchCountsTests): its prompts are not shown.
  const hidden = await loadWith((s) => {
    s.ui.hold_open = 'always';
    for (const ring of s.rings) { ring.shown = ring.ring_id === PERSONAL; }
    s.rings[0].counts = { needs_you: 0, failed: 0, review: 0, working: 1, idle: 0 };
    s.rings[1].counts.needs_you = 4;
  }, { onHover: true });
  assert.equal(hidden.run('foldAllowed()'), true);
  // Upstream's own reasons still hold (the pointer is in).
  never.run('pointerIn = true');
  assert.equal(never.run('foldAllowed()'), false);
  clean(held);
});

test('foldAllowed: false while the panel is open, true again when it closes; the notch folds then', async () => {
  const page = await loadWith(null, { onHover: true });
  page.run('setFolded(true)');
  assert.equal(page.document.body.classList.contains('folded'), true);
  page.emit('an:panel_state', { open: true, ring_id: PERSONAL });
  assert.equal(page.document.body.classList.contains('folded'), false); // pinned open
  assert.equal(page.run('foldAllowed()'), false);
  page.tick(2000);
  assert.equal(page.document.body.classList.contains('folded'), false);
  page.emit('an:panel_state', { open: false, ring_id: null });
  assert.equal(page.run('foldAllowed()'), true);
  page.tick(500); // FOLD_GRACE
  assert.equal(page.document.body.classList.contains('folded'), true);
  clean(page);
});

test('hold_open always unfolds a folded notch when something needs you, and lets it fold when that is over', async () => {
  const page = await loadWith((s) => {
    s.ui.hold_open = 'always';
    for (const ring of s.rings) ring.counts = { needs_you: 0, failed: 0, review: 0, working: 1, idle: 0 };
  }, { onHover: true });
  page.tick(500);
  assert.equal(page.document.body.classList.contains('folded'), true);
  page.emit('an:snapshot', snapshotWith((s) => { s.ui.hold_open = 'always'; }));
  assert.equal(page.document.body.classList.contains('folded'), false);
  page.emit('an:snapshot', snapshotWith((s) => {
    s.ui.hold_open = 'always';
    for (const ring of s.rings) ring.counts = { needs_you: 0, failed: 0, review: 0, working: 1, idle: 0 };
  }));
  page.tick(500);
  assert.equal(page.document.body.classList.contains('folded'), true);
  clean(page);
});

test('showCard: nothing while the panel is open, the card otherwise', async () => {
  const page = await load();
  assert.equal(page.window.showCard.__agentnotch, true);
  page.emit('an:panel_state', { open: true, ring_id: WORK });
  openCard(page, WORK);
  assert.equal(isShown(page), false);
  // A card that was up when the panel opened goes away.
  page.emit('an:panel_state', { open: false, ring_id: null });
  openCard(page, WORK);
  assert.equal(isShown(page), true);
  page.emit('an:panel_state', { open: true, ring_id: WORK });
  assert.equal(isShown(page), false);
  page.emit('an:panel_state', { open: false, ring_id: null });
  openCard(page, WORK);
  assert.equal(isShown(page), true);
  clean(page);
});

test('the wrappers are put on once, whatever loads twice', async () => {
  const page = await load();
  const fold = page.window.foldAllowed;
  const show = page.window.showCard;
  page.window.agentnotch._.start();
  assert.equal(page.window.foldAllowed, fold);
  assert.equal(page.window.showCard, show);
});

test('an:panel_state ignores what is not a state', async () => {
  const page = await load();
  page.emit('an:panel_state', null);
  page.emit('an:panel_state', 'open');
  assert.equal(page.window.agentnotch._.panelOpen(), false);
  page.emit('an:panel_state', { open: true });
  assert.equal(page.window.agentnotch._.panelOpen(), true);
  clean(page);
});

// ---- the peek ---------------------------------------------------------------------------------------------

test('a peek unfolds the notch, hovers the ring, shows its card, and folds after the seconds are up', async () => {
  const page = await load({ onHover: true });
  page.tick(500);
  assert.equal(page.document.body.classList.contains('folded'), true);
  page.emit('an:peek', { ring_id: WORK, seconds: 3 });
  assert.equal(page.document.body.classList.contains('folded'), false);
  assert.equal(page.run('hoverId'), WORK);
  assert.equal(isShown(page), true);
  assert.match(page.$('#card .c-title').textContent, /Work/);
  // Held open for the whole peek, whatever upstream's timers say.
  page.tick(2900);
  assert.equal(page.document.body.classList.contains('folded'), false);
  assert.equal(isShown(page), true);
  page.tick(200);
  assert.equal(isShown(page), false);
  page.tick(500);
  assert.equal(page.document.body.classList.contains('folded'), true);
  clean(page);
});

test('a peek keeps its card while the pointer is on the notch', async () => {
  const page = await load({ onHover: true });
  page.emit('an:peek', { ring_id: PERSONAL, seconds: 3 });
  page.run('pointerIn = true');
  page.tick(3100);
  assert.equal(isShown(page), true);
  clean(page);
});

test('a peek is ignored: for an unknown ring, with the panel open, before a snapshot, or malformed', async () => {
  const page = await load({ onHover: true });
  page.tick(500);
  page.emit('an:peek', { ring_id: 'claude-acct-nope', seconds: 3 });
  page.emit('an:peek', {});
  page.emit('an:peek', null);
  assert.equal(page.document.body.classList.contains('folded'), true);
  page.emit('an:panel_state', { open: true, ring_id: WORK });
  page.emit('an:peek', { ring_id: WORK, seconds: 3 });
  assert.equal(isShown(page), false);
  const early = await load({ onHover: true, snapshot: null });
  early.tick(500);
  early.emit('an:peek', { ring_id: WORK, seconds: 3 });
  assert.equal(early.document.body.classList.contains('folded'), true);
  clean(page);
  clean(early);
});

test('a peek with no seconds is five', async () => {
  const page = await load({ onHover: true });
  page.emit('an:peek', { ring_id: WORK });
  page.tick(4900);
  assert.equal(isShown(page), true);
  page.tick(200);
  assert.equal(isShown(page), false);
});

// ---- resting marks ------------------------------------------------------------------------------------------

const marksOf = (page) => page.$$('#an-marks .an-mark').map((m) => m.className.replace('an-mark ', ''));

test('resting marks: needs-you bar, review dot, working dot in that order, in an element after #rest', async () => {
  const page = await load();
  const host = page.$('#an-marks');
  assert.ok(host);
  assert.equal(host.getAttribute('aria-hidden'), 'true');
  assert.equal(host.parentNode, page.$('#root'));
  assert.equal(host.previousElementSibling, page.$('#rest'));
  assert.deepEqual(marksOf(page), ['an-mark-needs', 'an-mark-review', 'an-mark-working']);
  // At most one of each, and only what the engine says.
  page.emit('an:snapshot', snapshotWith((s) => { s.resting_marks = { needs_you: false, review: true, working: true, needs_you_key: 5 }; }));
  assert.deepEqual(marksOf(page), ['an-mark-review', 'an-mark-working']);
  page.emit('an:snapshot', snapshotWith((s) => { s.resting_marks = { needs_you: true, review: false, working: false, needs_you_key: 5 }; }));
  assert.deepEqual(marksOf(page), ['an-mark-needs']);
  page.emit('an:snapshot', snapshotWith((s) => { s.resting_marks = { needs_you: false, review: false, working: false, needs_you_key: 5 }; }));
  assert.deepEqual(marksOf(page), []);
  clean(page);
});

test('resting marks lay out 3 px apart, centred (Fix_RestingMarkShapeTests, C_BurstAndRestingMarkTests)', async () => {
  const page = await load();
  const offsets = (marks) => plain(page.window.agentnotch._.markOffsets(marks));
  assert.deepEqual(offsets([]), []);
  assert.deepEqual(offsets(['needs']), [0]);
  const three = offsets(['needs', 'review', 'working']);
  // Symmetric run.
  assert.ok(Math.abs((three[0] - 9 / 2) + (three[2] + 4 / 2)) < 0.001);
  // Edge to edge, a gap of 3 apart.
  assert.ok(Math.abs(three[1] - 2 - (three[0] + 4.5) - 3) < 0.001);
  assert.ok(Math.abs(three[2] - 2 - (three[1] + 2) - 3) < 0.001);
  assert.deepEqual(three, [-7, 2.5, 9.5]);
  // Dots only: `alongOffsets(count:)`, a pitch of dot + gap = 7.
  assert.deepEqual(offsets(['review', 'working']), [-3.5, 3.5]);
  assert.deepEqual(offsets(['review']), [0]);
  assert.deepEqual(offsets(['review', 'working', 'review']), [-7, 0, 7]);
  // What the page draws is those numbers.
  assert.deepEqual(page.$$('#an-marks .an-mark').map((m) => m.style.getPropertyValue('--an-along')), ['-7.0px', '2.5px', '9.5px']);
  // A bar is longer than a dot.
  assert.match(NOTCH_CSS, /#an-marks \.an-mark-needs\{height:9px;/);
  assert.match(NOTCH_CSS, /#an-marks \.an-mark\{[^}]*width:4px;height:4px/);
});

test('resting marks fit the folded pill (10 x 79) and the smallest one the Mac tests', async () => {
  const page = await load();
  const fit = (marks, length, depth) => page.window.agentnotch._.marksFit(marks, length, depth);
  const all = ['needs', 'review', 'working'];
  assert.equal(fit(all, 79, 10), true);
  assert.equal(fit(all, 40, 6), true);
  assert.equal(fit(all, 20, 6), false);
  assert.equal(fit(['review'], 79, 4.5), false);
  assert.equal(fit([], 1, 1), true);
  assert.equal(fit(['review', 'working', 'review'], 19, 8), false);
  assert.equal(fit(['review', 'working', 'review'], 210 * 44 / 117, 26 * 44 / 117 - 2), true);
});

test('a new needs_you_key restarts the bar\'s breath; the same key leaves it alone', async () => {
  const page = await load();
  const bar = () => page.$('#an-marks .an-mark-needs');
  const first = bar();
  assert.equal(first.getAttribute('data-key'), 'needs-5');
  page.emit('an:snapshot', snapshotWith());
  assert.equal(bar(), first, 'same key: the same element, its breath is not restarted');
  page.emit('an:snapshot', snapshotWith((s) => { s.resting_marks.needs_you_key = 6; }));
  assert.notEqual(bar(), first, 'new key: a new element, a new breath');
  assert.equal(bar().getAttribute('data-key'), 'needs-6');
  clean(page);
});

test('ui.resting_marks off: no marks; on again: they are back', async () => {
  const page = await loadWith((s) => { s.ui.resting_marks = false; });
  assert.deepEqual(marksOf(page), []);
  page.emit('an:snapshot', snapshotWith());
  assert.equal(marksOf(page).length, 3);
  page.emit('an:snapshot', snapshotWith((s) => { s.ui.resting_marks = false; }));
  assert.deepEqual(marksOf(page), []);
  clean(page);
});

test('resting marks show only under body.folded, never take a click, and lie along the pill on every edge', () => {
  assert.match(NOTCH_CSS, /#an-marks\{[^}]*pointer-events:none[^}]*opacity:0/);
  assert.match(NOTCH_CSS, /body\.folded #an-marks\{opacity:1;transition:opacity \.25s ease-in \.3s\}/);
  assert.match(NOTCH_CSS, /#an-marks\{[^}]*transition:opacity \.1s ease-out/);
  // Right: 10 wide and 79 long, at the right edge; left mirrors it; flat 79 x 10 on top and bottom.
  assert.match(NOTCH_CSS, /#an-marks\{[^}]*right:0;top:50%;width:10px;height:79px/);
  assert.match(NOTCH_CSS, /body\[data-edge="left"\] #an-marks\{right:auto;left:0\}/);
  assert.match(NOTCH_CSS, /body\[data-edge="top"\] #an-marks,body\[data-edge="bottom"\] #an-marks\{[^}]*width:79px;height:10px\}/);
  assert.match(NOTCH_CSS, /body\[data-edge="top"\] #an-marks\{top:0\}/);
  assert.match(NOTCH_CSS, /body\[data-edge="bottom"\] #an-marks\{bottom:0\}/);
  // The bar stands upright on a side edge (9 tall, 4 wide) and lies down flat (9 wide, 4 tall).
  assert.match(NOTCH_CSS, /body\[data-edge="top"\] #an-marks \.an-mark-needs,body\[data-edge="bottom"\] #an-marks \.an-mark-needs\{width:9px;height:4px\}/);
  // Marks run along y on a side edge and along x when flat.
  assert.match(NOTCH_CSS, /translateY\(var\(--an-along\)\)/);
  assert.match(NOTCH_CSS, /translateX\(var\(--an-along\)\)/);
  // The same box as upstream's #rest, so the marks lie inside what is drawn.
  assert.match(NOTCH_HTML, /#rest\{[^}]*width:10px;height:79px/);
});

test('motion: the spinner turns in steps, every breath is finite, and a still notch stops them all', () => {
  for (const m of NOTCH_CSS.matchAll(/animation:([^;}]*)/g)) {
    const value = m[1];
    if (value === 'none') continue;
    if (/infinite/.test(value)) assert.match(value, /steps\(/, `an endless animation must be stepped: ${value}`);
  }
  assert.match(NOTCH_CSS, /#an-marks \.an-mark-needs\{[^}]*animation:an-mark-breathe \.8s ease-in-out 7 alternate both/);
  assert.match(NOTCH_CSS, /#card \.an-breathe\{animation:an-card-breathe \.9s ease-in-out 7 alternate both\}/);
  assert.match(NOTCH_CSS, /#card \.an-spin\{animation:an-card-spin 1\.4s steps\(12\) infinite/);
  assert.match(NOTCH_CSS, /@media \(prefers-reduced-motion: reduce\)\{[^@]*animation:none/);
  assert.match(NOTCH_CSS, /html\.an-static [^{]*\.arc-spin,html\.an-static \.arc-pulse\{animation:none\}/);
});

// ---- the notice --------------------------------------------------------------------------------------------

test('an:notice shows through upstream\'s notice, as text', async () => {
  const page = await load();
  page.emit('an:notice', 'Claude Code control stopped');
  assert.equal(page.$('#notice').textContent, 'Claude Code control stopped');
  assert.ok(page.$('#notice').classList.contains('show'));
  for (const hostile of audit.HOSTILE) {
    page.emit('an:notice', hostile);
    assert.equal(page.$('#notice').textContent, hostile);
    assert.deepEqual(audit.problems(page.$('#notice')), []);
  }
  page.emit('an:notice', '');
  page.emit('an:notice', { text: 'x' });
  clean(page);
});

// ---- scenes and the layout report ----------------------------------------------------------------------------

test('showScene: open, card and folded, and the report after each', async () => {
  const page = await load({ onHover: true });
  const n = page.window.agentnotch;
  assert.equal(n.showScene('notch-open'), true);
  assert.equal(isShown(page), false);
  assert.equal(page.document.body.classList.contains('folded'), false);
  assert.equal(page.document.documentElement.classList.contains('an-static'), true);
  assert.equal(plain(n.layoutReport()).ok, true);
  assert.equal(n.showScene('notch-card'), true);
  assert.equal(isShown(page), true);
  assert.equal(page.run('hoverId'), PERSONAL);
  assert.equal(plain(n.layoutReport()).ok, true);
  assert.equal(n.showScene('notch-folded'), true);
  assert.equal(page.document.body.classList.contains('folded'), true);
  assert.equal(isShown(page), false);
  assert.equal(n.showScene('notch-nope'), false);
  clean(page);
});

test('layoutReport: a badge over the percent label fails on a flat edge only; a row outside the card fails', async () => {
  for (const [edge, expectFailure] of [['top', true], ['bottom', true], ['right', false], ['left', false]]) {
    const page = await load({ edge });
    const pill = page.$('#pill');
    pill.__rect = { left: 0, top: 0, width: 400, height: 100 };
    const cell = cellFor(page, PERSONAL);
    cell.querySelector('.pct').__rect = { left: 10, top: 50, width: 30, height: 20 };
    cell.querySelector('.an-badge-review').__rect = { left: 20, top: 55, width: 12, height: 12 };
    cell.querySelector('.an-badge-needs').__rect = { left: 0, top: 0, width: 12, height: 12 };
    const report = plain(page.window.agentnotch.layoutReport());
    assert.equal(report.edge, edge);
    assert.equal(report.badges, 4);
    assert.equal(report.ok, !expectFailure, `${edge}: ${report.failures}`);
    if (expectFailure) assert.match(report.failures[0], /badge over the percent label/);
  }
  const page = await load();
  page.$('#pill').__rect = { left: 0, top: 0, width: 70, height: 100 };
  const outside = cellFor(page, PERSONAL).querySelector('.an-badge-needs');
  outside.__rect = { left: -20, top: 0, width: 12, height: 12 };
  assert.match(plain(page.window.agentnotch.layoutReport()).failures[0], /badge outside the pill/);
  outside.__rect = null;
  openCard(page, PERSONAL);
  page.$('#card').__rect = { left: 0, top: 0, width: 246, height: 300 };
  rowsOf(page)[0].__rect = { left: 0, top: 10, width: 246, height: 400 };
  assert.match(plain(page.window.agentnotch.layoutReport()).failures.join(), /card row outside the card/);
});

test('layoutReport: marks outside the folded pill fail', async () => {
  const page = await load();
  page.window.agentnotch.showScene('notch-folded');
  page.$('#rest').__rect = { left: 350, top: 285, width: 10, height: 79 };
  const marks = page.$$('#an-marks .an-mark');
  marks.forEach((m, i) => { m.__rect = { left: 353, top: 300 + i * 8, width: 4, height: 4 }; });
  assert.equal(plain(page.window.agentnotch.layoutReport()).ok, true);
  marks[0].__rect = { left: 353, top: 500, width: 4, height: 9 };
  assert.match(plain(page.window.agentnotch.layoutReport()).failures[0], /resting mark outside the folded pill/);
});

// ---- hostile strings ------------------------------------------------------------------------------------------

test('an account label can\'t break out of upstream\'s alt="…" when the Claude glyph is an image', async () => {
  // Upstream's glyphHtml writes the cell's name into alt="…" unescaped when the glyph is a PNG
  // (an override in the glyph folder); an account's label is not a constant like upstream's names.
  const glyphs = { claude: { kind: 'appicon', url: 'data:image/png;base64,iVBORw0KGgo=' } };
  for (const hostile of audit.HOSTILE.concat(['Work" style="position:fixed;inset:0" data-x="'])) {
    const page = await loadWith((s) => { s.rings[0].label = hostile; }, { commands: { get_glyphs: () => glyphs } });
    await page.settle();
    const img = cellFor(page, PERSONAL).querySelector('img');
    assert.ok(img, 'the image glyph is drawn');
    assert.deepEqual(img.attributes.map((a) => a.name).sort(), ['alt', 'class', 'src'], hostile.slice(0, 40));
    // What upstream's unescaped alt="…" reads back as (entities decode; nothing ends the attribute).
    const read = require('./lib/dom.cjs').parseFragment(`<img alt="${hostile.replace(/"/g, '\u201D')}">`).querySelector('img');
    assert.equal(img.getAttribute('alt'), read.getAttribute('alt'));
    assert.deepEqual(audit.problems(page.$('#pill'), { allowTags: ['img'] }).filter((x) => !/^<img src=/.test(x)), [], hostile.slice(0, 40));
    clean(page);
  }
});

test('a snapshot the hooks can\'t read leaves upstream\'s notch drawing: each hook answers "not mine", logged once', async () => {
  const page = await loadWith((s) => { s.rings[0].badges = null; s.sessions[0] = null; });
  await page.settle();
  // renderRing went on past the Claude cells (the hot rect was reported) and nothing escaped.
  assert.deepEqual(page.errors.map(String), []);
  assert.ok(cellFor(page, PERSONAL), 'the cells are drawn');
  page.run('renderRing(); renderRing();');
  assert.equal(page.run(`agentnotch.cardSessions({ id: ${JSON.stringify(PERSONAL)}, base: 'claude', snap: { windows: [] } })`), '');
  assert.equal(page.run(`agentnotch.ringClick(${JSON.stringify(PERSONAL)})`), true, 'a click on our cell stays ours');
  assert.equal(page.run("agentnotch.ringClick('codex')"), false);
  await page.settle();
  const logs = page.hub.of('log').map((c) => c.args.msg);
  assert.deepEqual(logs.filter((m) => /decorateCell/.test(m)), ['notch: decorateCell failed'], 'once, and no snapshot text');
  clean(page);
});

test('hostile strings through the ring label, its a11y text and the card\'s name, detail and waiting line', async () => {
  for (const hostile of audit.HOSTILE) {
    const page = await loadWith((s) => {
      s.rings[0].label = hostile;
      s.rings[0].a11y = hostile;
      for (const row of s.sessions.filter((x) => x.ring_id === PERSONAL)) {
        row.title = hostile;
        row.card.name = hostile;
        row.card.detail = hostile;
        row.card.waiting_for = hostile;
      }
    });
    const cell = cellFor(page, PERSONAL);
    assert.equal(cell.getAttribute('aria-label'), hostile);
    assert.deepEqual(audit.problems(page.$('#pill')), [], 'the pill');
    openCard(page, PERSONAL);
    const card = page.$('#card');
    assert.deepEqual(audit.problems(card), [], `the card for ${hostile.slice(0, 30)}`);
    // The cell's name carries a " as ” (cellName: upstream's glyphHtml puts the name in alt="…").
    const shown = hostile.replace(/"/g, '\u201D');
    assert.ok(card.querySelector('.c-title').textContent.includes(shown.slice(0, 200)), 'the label is drawn as text');
    const rows = rowsOf(page);
    assert.ok(rows.length >= 3);
    for (const row of rows) {
      assert.equal(row.querySelector('.an-srow-name').textContent, hostile);
      assert.equal(row.getAttribute('aria-label').includes(hostile), true);
      // No element came out of the text: a row is its two lines and nothing else.
      assert.equal(row.querySelectorAll('.an-srow-line').length, 2);
      assert.equal(row.querySelectorAll('img, script, iframe, style, base, input, textarea, a').length, 0);
    }
    assert.deepEqual(page.errors.map(String), []);
  }
});

test('hostile session ids stay text in the row\'s attribute and come back exact on a click', async () => {
  for (const hostile of audit.HOSTILE.filter((h) => h.length < 200)) {
    const page = await loadWith((s) => { s.sessions.find((x) => x.session_id === 'work-ci').session_id = hostile; });
    openCard(page, PERSONAL);
    assert.deepEqual(audit.problems(page.$('#card')), []);
    await clickRow(page, hostile);
    assert.deepEqual(page.hub.of('focus').map((c) => c.args.session_id), [hostile]);
    clean(page);
  }
});

test('a session with none of its card\'s fields still draws a row', async () => {
  const page = await loadWith((s) => {
    const row = s.sessions.find((x) => x.session_id === 'work-ci');
    row.card = {};
    const other = s.sessions.find((x) => x.session_id === 'idle-notch');
    delete other.card;
    other.title = 'Only a title';
  });
  openCard(page, PERSONAL);
  const rows = rowsOf(page);
  const bare = rows.find((r) => r.getAttribute('data-an-session') === 'work-ci');
  assert.equal(bare.querySelector('.an-srow-word').textContent, 'idle');
  assert.equal(bare.querySelector('.an-srow-name').textContent, harness.fixture('snapshot.json').sessions.find((x) => x.session_id === 'work-ci').title);
  const titled = rows.find((r) => r.getAttribute('data-an-session') === 'idle-notch');
  assert.equal(titled.querySelector('.an-srow-name').textContent, 'Only a title');
  clean(page);
});

// ---- the counts of C_NotchCountsTests ---------------------------------------------------------------------------

test('what the notch counts: only shown rings, and only what needs you (answerable or failed)', async () => {
  const page = await load();
  const needs = (edit) => page.window.agentnotch._.shownNeedsYou(snapshotWith(edit));
  assert.equal(needs(), true);
  assert.equal(needs((s) => { for (const r of s.rings) r.shown = false; }), false);
  assert.equal(needs((s) => { for (const r of s.rings) r.counts = { needs_you: 0, failed: 0, review: 3, working: 3, idle: 3 }; }), false);
  assert.equal(needs((s) => { for (const r of s.rings) r.counts = { needs_you: 0, failed: 1, review: 0, working: 0, idle: 0 }; }), true);
  assert.equal(needs((s) => { s.rings[0].shown = false; s.rings[1].counts = { needs_you: 0, failed: 0, review: 0, working: 0, idle: 0 }; }), false);
  assert.equal(page.window.agentnotch._.shownNeedsYou(null), false);
});

test('marks come from the engine\'s own flags, and off means none whatever the flags say', async () => {
  const page = await load();
  const marks = (edit) => plain(page.window.agentnotch._.restingMarks(snapshotWith(edit)));
  assert.deepEqual(marks(), ['needs', 'review', 'working']);
  assert.deepEqual(marks((s) => { s.ui.resting_marks = false; }), []);
  assert.deepEqual(marks((s) => { s.resting_marks = { needs_you: false, review: false, working: true, needs_you_key: 0 }; }), ['working']);
  assert.deepEqual(plain(page.window.agentnotch._.restingMarks(null)), []);
});
