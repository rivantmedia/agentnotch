'use strict';
// ui/agentnotch/panel-list.js and the list region of panel.js/panel.css: sections, folding,
// rows regular and compact, empty states, mark all reviewed with undo.
//
// Two levels. The pure functions (layout, ordering, the markup of one row) run in a context of
// their own with lib/scripts.cjs; the page behaviour (fold state kept in the page, the frozen
// order under the pointer, actions and calls, the undo window on the fake clock, a snapshot
// re-render that keeps selection and scroll) runs on the real panel.html through the harness.
//
// Swift tests ported: SessionSectionsTests / SessionListTests (bucket order and skipping empty
// sections, the order inside a section, ties by id, the frozen order and newcomers, moving
// between sections while frozen, default folding, the compact threshold, the folded summary, the
// keyboard order through unfolded rows only), Fix_SessionListIdentityTests (a row keeps its
// element across sections, a folded section is one item, only the first row has no gap) and
// B_PanelRenderTests (each state renders: empty, one, regular, every state, busy). What the
// ENGINE decides (the row's words, its a11y sentence, the titles, the default fold) arrives in
// the snapshot and is only shown. New: every row detail kind of the fixture, compact rows, the
// filtered view, elapsed labels on the fake clock, the undo toast's timing, hostile strings
// through every row part, a 10 000-character title, a re-render that keeps selection and scroll.
//
// What node cannot know is layout: the snapshot tool renders the panel-* scenes in a browser and
// layoutReport() measures the overflow there.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const audit = require('./lib/audit.cjs');
const dom = require('./lib/dom.cjs');
const harness = require('./lib/harness.cjs');
const scripts = require('./lib/scripts.cjs');

const CSS = fs.readFileSync(path.join(harness.UI, 'agentnotch', 'panel.css'), 'utf8');
const PERSONAL = 'claude-acct-5f3e1d2c0b9a';
const WORK = 'claude-acct-8a7b6c5d4e3f';
const NOW = harness.NOW;
const plain = scripts.plain;

// ---- helpers ---------------------------------------------------------------------------------

async function load(options) {
  const page = harness.loadPage('agentnotch/panel.html', options);
  await page.settle();
  return page;
}

function snapshotWith(edit) {
  const snapshot = harness.fixture('snapshot.json');
  if (edit) edit(snapshot);
  return snapshot;
}

async function open(request, edit, options) {
  return load(Object.assign({
    snapshot: snapshotWith(edit),
    before: (window) => { window.__AGENTNOTCH_PANEL__ = request || { route: 'sessions' }; },
  }, options));
}

function clean(page) {
  assert.deepEqual(page.errors.map(String), []);
  assert.deepEqual(page.hub.violations, []);
}

const calls = (page, method) => page.hub.of(method).map((c) => plain(c.args));
const rows = (page) => page.$$('#an-rows .an-row');
const rowIds = (page) => rows(page).map((r) => r.getAttribute('data-id'));
const row = (page, id) => rows(page).find((r) => r.getAttribute('data-id') === id);
const text = (el) => el.textContent.replace(/\s+/g, ' ').trim();
const headers = (page) => page.$$('#an-rows .an-sh').map((h) => h.querySelector('.an-sh-btn').children.map((c) => text(c)).filter(Boolean).join(' '));
/** Edits a snapshot down to the sessions listed (fewer than nine rows are drawn regular). */
const only = (...ids) => (s) => { s.sessions = s.sessions.filter((x) => ids.includes(x.session_id)); };
const state = (page) => plain(page.run('agentnotchPanel._.state'));

/** A session row of the fixture's shape, changed by `patch`. */
function session(id, bucket, patch) {
  const base = harness.fixture('snapshot.json').sessions.find((s) => s.bucket === bucket);
  return Object.assign(JSON.parse(JSON.stringify(base)), { session_id: id, title: 'Title ' + id, since_ms: NOW - 60000 }, patch || {});
}

/** The fixture snapshot with `extra` sessions appended (sections and counts follow). */
function withExtra(s, extra) {
  s.sessions.push(...extra);
  return s;
}

/** A pure context: common.js and panel-list.js, no page. */
function pure() {
  const ctx = scripts.load(['common', 'panel-list']);
  return {
    ctx,
    L: ctx.window.agentnotchPanelList,
    layout: (v, o) => plain(ctx.window.agentnotchPanelList.layout(v, o)),
    html: (v, o, h) => ctx.window.agentnotchPanelList.html(ctx.window.agentnotchPanelList.layout(v, o), h || { now: NOW }),
    ids: (lay) => lay.sections.map((s) => s.rows.map((r) => r.session_id)),
  };
}

function snap(sessions, sections) {
  return { sessions, sections: sections || [], accounts_multi: true };
}

// ---- ordering and sections (SessionSectionsTests) --------------------------------------------

test('sections follow the snapshot\'s order and titles and skip the ones with no rows', () => {
  const p = pure();
  const s = snap([session('i', 'idle'), session('w', 'working'), session('r', 'ready_for_review'), session('n', 'needs_you')], [
    { bucket: 'needs_you', title: 'Needs you', count: 1, fold_by_default: false },
    { bucket: 'ready_for_review', title: 'Ready for review', count: 1, fold_by_default: false },
    { bucket: 'working', title: 'Working', count: 1, fold_by_default: false },
    { bucket: 'idle', title: 'Idle', count: 1, fold_by_default: false },
  ]);
  const lay = p.layout(s);
  assert.deepEqual(lay.sections.map((x) => x.bucket), ['needs_you', 'ready_for_review', 'working', 'idle']);
  assert.deepEqual(lay.sections.map((x) => x.title), ['Needs you', 'Ready for review', 'Working', 'Idle']);
  assert.deepEqual(p.ids(lay), [['n'], ['r'], ['w'], ['i']]);
  const two = p.layout(snap([session('w', 'working'), session('i', 'idle')]));
  assert.deepEqual(two.sections.map((x) => x.bucket), ['working', 'idle'], 'no snapshot sections: the Mac\'s bucket order');
  // a snapshot that lists a bucket first shows it first; an unlisted one follows
  const swapped = p.layout(snap([session('w', 'working'), session('n', 'needs_you')], [{ bucket: 'working', title: 'Busy', count: 1, fold_by_default: false }]));
  assert.deepEqual(swapped.sections.map((x) => [x.bucket, x.title]), [['working', 'Busy'], ['needs_you', 'Needs you']]);
  assert.equal(p.layout(snap([])).isEmpty, true);
});

test('Needs you: answerable requests before failed turns, each oldest wait first; ties by session id', () => {
  const p = pure();
  const s = snap([
    session('recent', 'needs_you', { since_ms: NOW - 30000 }),
    session('oldest', 'needs_you', { since_ms: NOW - 600000 }),
    session('dialog', 'needs_you', { since_ms: NOW - 120000 }),
    session('error', 'needs_you', { since_ms: NOW - 900000, failed: true }),
    session('error2', 'needs_you', { since_ms: NOW - 60000, failed: true }),
  ]);
  assert.deepEqual(p.ids(p.layout(s)), [['oldest', 'dialog', 'recent', 'error', 'error2']]);
  const ties = snap([session('c', 'working', { since_ms: NOW - 60000 }), session('a', 'working', { since_ms: NOW - 60000 }), session('b', 'working', { since_ms: NOW - 60000 })]);
  assert.deepEqual(p.ids(p.layout(ties)), [['a', 'b', 'c']]);
  ties.sessions.reverse();
  assert.deepEqual(p.ids(p.layout(ties)), [['a', 'b', 'c']], 'whatever the input order');
});

test('Ready for review and Idle: newest first; Working: longest running first, unknown starts last', () => {
  const p = pure();
  const review = snap([session('old', 'ready_for_review', { since_ms: NOW - 3600000 }), session('new', 'ready_for_review', { since_ms: NOW - 60000 }), session('mid', 'ready_for_review', { since_ms: NOW - 600000 })]);
  assert.deepEqual(p.ids(p.layout(review)), [['new', 'mid', 'old']]);
  const idle = snap([session('yesterday', 'idle', { since_ms: NOW - 86400000 }), session('now', 'idle', { since_ms: NOW - 5000 }), session('hour', 'idle', { since_ms: NOW - 3600000 })]);
  assert.deepEqual(p.ids(p.layout(idle)), [['now', 'hour', 'yesterday']]);
  const working = snap([session('short', 'working', { since_ms: NOW - 30000 }), session('unknown', 'working', { since_ms: 0 }), session('long', 'working', { since_ms: NOW - 900000 })]);
  assert.deepEqual(p.ids(p.layout(working)), [['long', 'short', 'unknown']]);
});

test('while the pointer is over the list rows keep their order and newcomers go to the end (preservedOrderKeepsRowsPut)', () => {
  const p = pure();
  const first = session('first', 'ready_for_review', { since_ms: NOW - 600000 });
  const second = session('second', 'ready_for_review', { since_ms: NOW - 300000 });
  const displayed = p.layout(snap([first, second])).order;
  assert.deepEqual(displayed, ['second', 'first']);
  const refreshed = Object.assign({}, first, { since_ms: NOW - 10000 });
  const newcomer = session('newcomer', 'ready_for_review', { since_ms: NOW - 5000 });
  const frozen = p.layout(snap([refreshed, second, newcomer]), { frozen: displayed });
  assert.deepEqual(p.ids(frozen), [['second', 'first', 'newcomer']]);
  assert.deepEqual(p.ids(p.layout(snap([refreshed, second, newcomer]))), [['newcomer', 'first', 'second']], 'released: the natural order');
});

test('a frozen order still moves a session between sections', () => {
  const p = pure();
  const displayed = p.layout(snap([session('w', 'working')])).order;
  const lay = p.layout(snap([session('w', 'ready_for_review')]), { frozen: displayed });
  assert.deepEqual(lay.sections.map((x) => x.bucket), ['ready_for_review']);
});

// ---- folding, compact, summary ----------------------------------------------------------------

test('the engine\'s fold_by_default folds a section; the user\'s choice wins; Needs you never folds', () => {
  const p = pure();
  const idle = (n) => Array.from({ length: n }, (_, i) => session('i' + i, 'idle', { since_ms: NOW - 1000 * (i + 1) }));
  const info = (bucket, count, fold) => [{ bucket, title: bucket, count, fold_by_default: fold }];
  assert.equal(p.layout(snap(idle(4), info('idle', 4, true))).sections[0].collapsed, true);
  assert.equal(p.layout(snap(idle(3), info('idle', 3, false))).sections[0].collapsed, false);
  const forty = Array.from({ length: 40 }, (_, i) => session('w' + i, 'working'));
  assert.equal(p.layout(snap(forty, info('working', 40, false))).sections[0].collapsed, false, 'a long working list is not folded by default');
  assert.equal(p.layout(snap(idle(2), info('idle', 2, false)), { folds: { idle: true } }).sections[0].collapsed, true, 'the user folds it');
  assert.equal(p.layout(snap(idle(9), info('idle', 9, true)), { folds: { idle: false } }).sections[0].collapsed, false, 'and unfolds it');
  const needs = p.layout(snap([session('n', 'needs_you')], info('needs_you', 1, false)), { folds: { needs_you: true } }).sections[0];
  assert.equal(needs.collapsed, false);
  assert.equal(needs.canFold, false);
  // narrowed to an account, the Mac counts the rows it shows: two idle rows are not a long list
  const narrowed = p.layout(snap(idle(2).map((r) => Object.assign(r, { ring_id: WORK })), info('idle', 4, true)), { filter: WORK });
  assert.equal(narrowed.sections[0].collapsed, false);
  assert.equal(narrowed.sections[0].count, 2, 'the count is what is shown');
});

test('rows go to one line past eight drawn rows; a folded section\'s rows are not drawn', () => {
  const p = pure();
  const many = (n, bucket, extra) => Array.from({ length: n }, (_, i) => session(bucket[0] + i, bucket, Object.assign({ since_ms: NOW - 1000 * (i + 1) }, extra)));
  assert.equal(p.layout(snap(many(8, 'working'))).compact, false);
  assert.equal(p.layout(snap(many(9, 'working'))).compact, true);
  const folded = p.layout(snap([...many(6, 'working'), ...many(5, 'idle')], [
    { bucket: 'working', title: 'Working', count: 6, fold_by_default: false },
    { bucket: 'idle', title: 'Idle', count: 5, fold_by_default: true },
  ]));
  assert.equal(folded.compact, false, '6 rows drawn; the 5 idle ones are one summary line');
  assert.deepEqual(folded.visible.length, 6);
  assert.equal(folded.order.length, 11, 'the frozen order still knows the folded rows');
  assert.equal(p.layout(snap(many(9, 'working')), { hidden: (r) => r.session_id === 'w0' }).compact, false, 'a row shown as gone does not count');
});

test('the folded summary names the first titles and counts the rest (foldedSummary)', () => {
  const p = pure();
  const s = (n) => snap(Array.from({ length: n }, (_, i) => session('w' + i, 'working', { title: ['Write tests', 'Fix CI', 'Bump deps'][i], since_ms: NOW - 1000 * (10 - i) })));
  assert.equal(p.layout(s(3)).sections[0].summary, 'Write tests, Fix CI and 1 more');
  assert.equal(p.layout(s(1)).sections[0].summary, 'Write tests');
  assert.equal(p.layout(s(2)).sections[0].summary, 'Write tests, Fix CI');
});

test('the keyboard order runs through unfolded rows only (layoutOrdersTheKeyboardThroughUnfoldedRowsOnly)', () => {
  const p = pure();
  const s = snap([
    session('needs', 'needs_you'), session('w', 'working'),
    ...['i1', 'i2', 'i3', 'i4'].map((id, i) => session(id, 'idle', { since_ms: NOW - 10000 * (i + 1) })),
  ], [
    { bucket: 'needs_you', title: 'Needs you', count: 1, fold_by_default: false },
    { bucket: 'working', title: 'Working', count: 1, fold_by_default: false },
    { bucket: 'idle', title: 'Idle', count: 4, fold_by_default: true },
  ]);
  const lay = p.layout(s);
  assert.deepEqual(lay.sections.map((x) => x.collapsed), [false, false, true]);
  assert.deepEqual(lay.visible, ['needs', 'w']);
  assert.equal(lay.compact, false);
});

// ---- identity of the list's items (Fix_SessionListIdentityTests) -----------------------------

const keys = (markup) => dom.elementsOf(dom.parseFragment(markup)).map((el) => el.getAttribute('data-key')).filter((k) => k && /^(head|folded|row)-/.test(k));

test('the list is one flat run of headers, summaries and rows; a row is keyed by its session alone', () => {
  const p = pure();
  const working = keys(p.html(snap([session('a', 'working'), session('b', 'working', { since_ms: NOW - 1000 })]), {}));
  assert.deepEqual(working, ['head-working', 'row-a', 'row-b']);
  const moved = keys(p.html(snap([session('a', 'ready_for_review'), session('b', 'working')]), {}));
  assert.ok(moved.includes('row-a') && moved.includes('row-b'), 'the same keys in another section');
  assert.equal(new Set(moved).size, moved.length);
  const folded = keys(p.html(snap([session('x', 'idle')]), { folds: { idle: true } }));
  assert.deepEqual(folded, ['head-idle', 'folded-idle'], 'a folded section is one item and its rows are not listed');
  const firsts = dom.elementsOf(dom.parseFragment(p.html(snap([session('a', 'working'), session('b', 'working', { since_ms: NOW - 1000 })]), {})))
    .filter((el) => (el.getAttribute('class') || '').split(' ').includes('an-row')).map((el) => el.getAttribute('class').includes('an-first'));
  assert.deepEqual(firsts, [true, false], 'only the first row of a section has no gap above');
});

test('in the page a row keeps its element when a snapshot moves it to another section', async () => {
  const page = await open({ route: 'sessions' }, only('needs-permission', 'review-darkmode', 'work-summary', 'work-ci'));
  const before = row(page, 'work-summary');
  assert.ok(before);
  const next = snapshotWith((s) => {
    only('needs-permission', 'review-darkmode', 'work-summary', 'work-ci')(s);
    s.generated_at_ms += 1000;
    const r = s.sessions.find((x) => x.session_id === 'work-summary');
    r.bucket = 'ready_for_review';
    r.detail = { kind: 'review', text: 'Done.' };
  });
  page.emit('an:snapshot', next);
  assert.equal(row(page, 'work-summary'), before, 'the same element, moved');
  assert.ok(text(before.querySelector('.an-detail')).includes('Done.'));
  assert.deepEqual(page.$$('.an-sh').map((h) => h.getAttribute('data-key')), ['head-needs_you', 'head-ready_for_review', 'head-working']);
  clean(page);
});

// ---- the list in the page: every state renders ------------------------------------------------

test('the fixture: sections in order with titles and counts, idle folded to one summary line', async () => {
  const page = await open();
  assert.deepEqual(headers(page), ['Needs you 5', 'Ready for review 3', 'Working 3', 'Idle 4']);
  assert.deepEqual(page.$$('#an-rows .an-sh').map((h) => h.getAttribute('data-key')), ['head-needs_you', 'head-ready_for_review', 'head-working', 'head-idle']);
  assert.equal(page.$('.an-sh-needs_you').closest('.an-sh').querySelector('.an-sh-chev'), null, 'Needs you has no chevron');
  assert.equal(rows(page).length, 11, '15 sessions, the 4 idle ones folded');
  const sum = page.$('.an-foldsum');
  assert.equal(text(sum), 'Explore the notch APIs, Tidy up the README and 2 more');
  assert.equal(sum.getAttribute('aria-label'), '4 idle sessions: Explore the notch APIs, Tidy up the README and 2 more');
  assert.equal(page.$('.an-sh-chev.an-folded') !== null, true);
  // the order inside each section is the Mac's: Needs you oldest first, failed last
  assert.deepEqual(rowIds(page).slice(0, 5), ['needs-plan', 'needs-question', 'needs-elicitation', 'needs-permission', 'needs-ratelimit']);
  assert.deepEqual(rowIds(page).slice(5, 8), ['review-just-finished', 'review-darkmode', 'review-devserver']);
  assert.deepEqual(rowIds(page).slice(8), ['work-migration', 'work-ci', 'work-summary']);
  clean(page);
});

test('every row detail kind of the fixture renders its own words', async () => {
  // eight rows or fewer are drawn regular; the other kinds come with a second page
  const page = await open({ route: 'sessions' }, only('needs-permission', 'needs-question', 'needs-plan', 'needs-elicitation', 'needs-ratelimit', 'review-darkmode', 'work-migration', 'work-ci'));
  const detail = (id) => row(page, id).querySelector('.an-detail');
  // permission: the tool (amber) and the request in a code box, whole because Allow sits below it
  const perm = detail('needs-permission');
  assert.equal(text(perm.querySelector('.an-tool')), 'Bash');
  assert.equal(text(perm.querySelector('.an-code')), 'npm run test -- --watch=false auth/redirect.spec.ts');
  assert.ok(perm.querySelector('.an-code').classList.contains('an-code-whole'));
  assert.equal(text(detail('needs-question')), 'Asks Which charting library should the dashboard use?');
  assert.equal(text(detail('needs-plan')), 'Plan ready for approval');
  assert.equal(text(detail('needs-elicitation')), 'Figma needs you to pick a file');
  assert.equal(text(detail('needs-ratelimit')), 'Rate limited · weekly limit resets Fri 9:00 AM');
  assert.ok(detail('needs-ratelimit').classList.contains('an-d-error'));
  assert.equal(text(detail('review-darkmode')), 'Added a Dark mode toggle under Settings › Appearance. It follows the system by default, persists the choice, and all 42 tests pass.');
  assert.equal(text(detail('work-migration')), 'Writing tests for the v2 schema');
  assert.ok(detail('work-migration').classList.contains('an-d-primary'));
  assert.equal(text(detail('work-ci')), 'Grep ETIMEDOUT|socket hang up');
  assert.ok(detail('work-ci').classList.contains('an-d-secondary'), 'a tool line or Thinking… is secondary ink');
  const more = await open({ route: 'sessions' }, only('work-summary', 'idle-notch', 'idle-readme', 'idle-logo', 'review-just-finished'));
  more.click('.an-foldsum');
  assert.equal(text(row(more, 'work-summary').querySelector('.an-detail')), 'Thinking…');
  assert.equal(text(row(more, 'idle-notch').querySelector('.an-detail')), 'The work area excludes the taskbar, so the notch sits below it.');
  assert.equal(text(row(more, 'idle-readme').querySelector('.an-detail')), 'You: thanks, that\'s all for now');
  assert.equal(text(row(more, 'idle-logo').querySelector('.an-detail')), 'No messages yet');
  assert.ok(row(more, 'idle-logo').querySelector('.an-detail').classList.contains('an-d-secondary'));
  clean(page);
  clean(more);
});

test('a permission the row cannot answer: "waiting in the terminal", the request limited to four lines', async () => {
  const page = await open({ route: 'sessions' }, (s) => {
    const r = s.sessions.find((x) => x.session_id === 'needs-permission');
    r.pending = null;
    r.detail = { kind: 'permission', tool: '', request: '', waiting_in_terminal: true };
  });
  const d = row(page, 'needs-permission').querySelector('.an-detail');
  assert.equal(text(d.querySelector('.an-tool')), 'Permission', 'no tool name: "Permission"');
  assert.equal(text(d.querySelector('.an-code')), 'waiting in the terminal');
  assert.ok(!d.querySelector('.an-code').classList.contains('an-code-whole'));
  // a request too long to judge inline is not shown whole either
  const long = await open({ route: 'sessions' }, (s) => {
    const r = s.sessions.find((x) => x.session_id === 'needs-permission');
    r.pending.needs_review = true;
  });
  assert.ok(!row(long, 'needs-permission').querySelector('.an-code').classList.contains('an-code-whole'));
  assert.match(CSS, /\.an-code \{[^}]*-webkit-line-clamp: 4/);
  clean(page);
});

test('the title, ring, elapsed label, account, project, tasks, context and background of a row', async () => {
  const page = await open({ route: 'sessions' }, only('needs-permission', 'needs-question', 'needs-ratelimit', 'review-darkmode', 'review-devserver', 'work-migration', 'work-ci', 'work-summary'));
  const r = row(page, 'needs-permission');
  assert.equal(text(r.querySelector('.an-rtitle')), 'Fix the login redirect loop');
  assert.equal(text(r.querySelector('.an-elapsed')), '2m');
  assert.ok(r.querySelector('.an-ring-needs'), 'needs you: half a ring');
  assert.equal(text(r.querySelector('.an-acct')), 'Work');
  assert.equal(text(r.querySelector('.an-tcount')), '3/7');
  assert.equal(r.querySelectorAll('.an-tk').length, 7, 'segmented: one segment per task while they fit');
  assert.deepEqual(r.querySelectorAll('.an-tk').map((s) => s.getAttribute('class').replace('an-tk an-tk-', '')), ['done', 'done', 'done', 'active', 'todo', 'todo', 'todo']);
  assert.equal(text(r.querySelector('.an-ctx-t')), '42% context');
  assert.equal(r.querySelector('.an-cbar span').getAttribute('style'), 'width:42.0%');
  assert.equal(text(r.querySelector('.an-proj')), 'acme-web');
  assert.equal(r.querySelector('.an-bg'), null);
  assert.equal(text(row(page, 'review-devserver').querySelector('.an-bg')), '2 background');
  assert.equal(row(page, 'review-devserver').querySelector('.an-bg').getAttribute('title'), '2 background tasks still running');
  assert.ok(row(page, 'needs-ratelimit').querySelector('.an-ring-error'), 'failed: a solid dot');
  assert.ok(row(page, 'work-ci').querySelector('.an-ring-working'));
  assert.ok(row(page, 'review-darkmode').querySelector('.an-ring-review'));
  // the meta line holds only what the row has: no tasks, no bar; no project when it is the title
  assert.equal(row(page, 'needs-question').querySelector('.an-tasks'), null);
  clean(page);
});

test('a task list too long for segments is one continuous bar with the active task marked', async () => {
  const page = await open({ route: 'sessions' }, only('work-ci', 'work-migration'));
  const ci = row(page, 'work-ci');
  assert.equal(text(ci.querySelector('.an-tcount')), '9/15');
  assert.equal(ci.querySelectorAll('.an-tk').length, 0);
  assert.equal(ci.querySelector('.an-tbar-fill .an-tk-done').getAttribute('style'), 'width:60.00%');
  assert.ok(ci.querySelector('.an-tbar-fill .an-tk-active'));
  const pure1 = pure();
  assert.equal(pure1.L.taskBar({ done: 1, total: 3, items: [{ status: 'completed' }, { status: 'in_progress' }, { status: 'pending' }] }, 28).total, 3);
  assert.match(pure1.L.taskBar({ done: 1, total: 5, items: [1, 2, 3, 4, 5].map(() => ({ status: 'pending' })) }, 28).bar, /an-tbar-fill/, '28 px cannot hold 5 segments of 6.5');
  assert.match(pure1.L.taskBar({ done: 1, total: 4, items: [1, 2, 3, 4].map(() => ({ status: 'pending' })) }, 28).bar, /an-tbar-seg/);
  assert.equal(pure1.L.taskBar({ done: 0, total: 0 }, 64), '');
});

test('the context meter\'s level: normal, 80 % amber, 90 % critical (ContextMeter.level)', async () => {
  const page = await open();
  assert.ok(row(page, 'needs-permission').querySelector('.an-ctx-normal'));
  assert.ok(row(page, 'work-migration').querySelector('.an-ctx-high'), '84 %');
  assert.ok(row(page, 'work-ci').querySelector('.an-ctx-critical'), '93 %');
  assert.match(CSS, /\.an-ctx-high \.an-ctx-t \{ color: var\(--an-needs-you\)/);
  assert.match(CSS, /\.an-ctx-critical \.an-ctx-t \{ color: var\(--an-critical\)/);
  const p = pure();
  const edge = (pct) => p.html(snap([session('x', 'working', { context_pct: pct })]), {}).match(/an-ctx-(normal|high|critical)/)[1];
  assert.deepEqual([79.9, 80, 89.9, 90, 100].map(edge), ['normal', 'high', 'high', 'critical', 'critical']);
  assert.equal(row(page, 'work-migration').querySelector('.an-ctx').getAttribute('title'), 'Context window 84% full');
});

test('aria-label is the engine\'s a11y sentence; every text is marked data-an-text', async () => {
  const page = await open();
  const fixture = harness.fixture('snapshot.json');
  for (const s of fixture.sessions.filter((x) => rowIds(page).includes(x.session_id))) {
    assert.equal(row(page, s.session_id).getAttribute('aria-label'), s.a11y, s.session_id);
  }
  // every element that holds text of its own is one (or sits in one) marked for the layout check
  const unmarked = [];
  for (const el of dom.elementsOf(page.$('#an-rows'))) {
    const own = el.childNodes.filter((n) => n.nodeType === 3 && n.nodeValue.trim()).length;
    if (!own) continue;
    let marked = false;
    for (let e = el; e && e !== page.$('#an-rows'); e = e.parentNode) if (e.hasAttribute && e.hasAttribute('data-an-text')) marked = true;
    if (!marked && !el.classList.contains('an-dotsep')) unmarked.push(el.getAttribute('class'));
  }
  assert.deepEqual(unmarked, []);
  clean(page);
});

// ---- the account tag and the filter -----------------------------------------------------------

test('the account tag shows with several accounts and no filter; hidden by a filter or by accounts_multi false', async () => {
  const all = await open();
  assert.equal(all.$$('#an-rows .an-acct').length, 5, 'the 5 regular rows (Needs you); the 6 compact ones have a dot');
  assert.equal(all.$$('#an-rows .an-acct-dot').length, 6);
  const filtered = await open({ route: 'sessions', ring_id: WORK });
  assert.equal(filtered.$$('#an-rows .an-acct').length, 0, 'the chip says the account');
  assert.equal(filtered.$$('#an-rows .an-acct-dot').length, 0);
  const single = await open({ route: 'sessions' }, (s) => { s.accounts_multi = false; });
  assert.equal(single.$$('#an-rows .an-acct').length, 0);
  assert.equal(single.$$('#an-rows .an-acct-dot').length, 0);
  clean(filtered);
});

test('a filtered view draws only that account\'s rows; the section counts follow', async () => {
  const page = await open({ route: 'sessions', ring_id: WORK });
  const work = harness.fixture('snapshot.json').sessions.filter((s) => s.ring_id === WORK);
  assert.equal(rows(page).length, work.length, '7 rows; the 2 idle ones are not a long list, so unfolded');
  assert.ok(rowIds(page).every((id) => work.some((s) => s.session_id === id)));
  const counts = {};
  for (const s of work) counts[s.bucket] = (counts[s.bucket] || 0) + 1;
  assert.deepEqual(headers(page), [`Needs you ${counts.needs_you}`, `Ready for review ${counts.ready_for_review}`, `Working ${counts.working}`, `Idle ${counts.idle}`]);
  page.click('.an-chip[data-an-arg="' + PERSONAL + '"]');
  assert.ok(rowIds(page).every((id) => !work.some((s) => s.session_id === id)));
  clean(page);
});

test('empty and filtered-empty states (UI 5.6) keep their words', async () => {
  const empty = await open({ route: 'sessions' }, (s) => { s.sessions = []; s.sections = []; });
  assert.equal(empty.text('.an-empty-title'), 'No Claude sessions yet');
  assert.equal(empty.text('.an-empty-text'), 'Start Claude Code in VS Code or a terminal. Sessions from every account show up here.');
  assert.equal(rows(empty).length, 0);
  const narrowed = await open({ route: 'sessions', ring_id: WORK }, (s) => { s.sessions = s.sessions.filter((r) => r.ring_id !== WORK); });
  assert.equal(narrowed.text('.an-empty-title'), 'No sessions in this account');
  assert.equal(narrowed.text('.an-empty-text'), 'Choose All to see every account’s sessions.');
  clean(empty);
});

// ---- folding in the page ----------------------------------------------------------------------

test('folding: the header toggles a section, the folded line unfolds it, the state stays across snapshots', async () => {
  const page = await open();
  const head = (bucket) => page.$('.an-sh[data-key="head-' + bucket + '"] .an-sh-btn');
  assert.equal(head('working').getAttribute('aria-expanded'), 'true');
  page.click(head('working'));
  assert.equal(head('working').getAttribute('aria-expanded'), 'false');
  assert.equal(page.$('.an-foldsum[data-key="folded-working"]').textContent.trim(), 'Write migration tests for the v2 schema, Investigate the flaky CI job and 1 more');
  assert.deepEqual(rowIds(page).filter((id) => id.startsWith('work-')), []);
  assert.equal(state(page).folds.working, true);
  const next = snapshotWith((s) => { s.generated_at_ms += 1000; });
  page.emit('an:snapshot', next);
  assert.equal(head('working').getAttribute('aria-expanded'), 'false', 'a new snapshot keeps the fold');
  page.click('.an-foldsum[data-key="folded-working"]');
  assert.equal(head('working').getAttribute('aria-expanded'), 'true');
  assert.equal(rowIds(page).filter((id) => id.startsWith('work-')).length, 3);
  // idle starts folded (the engine says so) and unfolds
  page.click('.an-foldsum[data-key="folded-idle"]');
  assert.equal(rowIds(page).filter((id) => id.startsWith('idle-')).length, 4);
  // Needs you does not fold: no button, and the action ignores it
  assert.equal(page.$('.an-sh[data-key="head-needs_you"] button'), null);
  page.run('agentnotchPanel.actions.fold("needs_you")');
  assert.equal(rowIds(page).filter((id) => id.startsWith('needs-')).length, 5);
  clean(page);
});

test('a highlighted row inside a folded section unfolds it and is selected', async () => {
  const page = await open({ route: 'sessions', highlight: 'idle-deps' });
  assert.ok(row(page, 'idle-deps'), 'the idle section opened');
  assert.ok(row(page, 'idle-deps').classList.contains('an-sel'));
  assert.equal(row(page, 'idle-deps').getAttribute('aria-current'), 'true');
  clean(page);
});

test('past eight drawn rows the rows other than Needs you are one line; Needs you rows stay regular', async () => {
  const page = await open({ route: 'sessions' }, (s) => {
    withExtra(s, Array.from({ length: 6 }, (_, i) => session('extra-' + i, 'working', { since_ms: NOW - 1000 * (i + 1) })));
  });
  assert.equal(page.$('.an-lst').getAttribute('data-compact'), '1');
  assert.ok(row(page, 'needs-permission').classList.contains('an-reg'), 'answers stay regular');
  assert.ok(row(page, 'needs-ratelimit').classList.contains('an-reg'));
  assert.ok(row(page, 'work-ci').classList.contains('an-cmp'));
  assert.ok(row(page, 'review-darkmode').classList.contains('an-cmp'));
  const ci = row(page, 'work-ci');
  assert.equal(text(ci.querySelector('.an-ctitle')), 'Investigate the flaky CI job');
  assert.equal(text(ci.querySelector('.an-cdet')), 'Grep ETIMEDOUT|socket hang up', 'working: the detail text');
  assert.equal(text(row(page, 'review-darkmode').querySelector('.an-cdet')), 'acme-web', 'review and idle: the project');
  assert.equal(ci.querySelector('.an-detail'), null, 'one line: no detail, no meta line');
  assert.equal(ci.querySelector('.an-meta'), null);
  const prog = ci.querySelector('.an-cprog');
  assert.equal(prog.querySelectorAll('.an-tk').length, 0, 'the 28 px bar of 15 tasks is continuous');
  assert.equal(text(prog.querySelector('.an-ctx-t')), '93%', 'context alone, no word');
  assert.ok(prog.querySelector('.an-acct-dot'), 'the account is a dot');
  assert.equal(prog.querySelector('.an-acct-dot').getAttribute('title'), 'Personal');
  assert.equal(text(ci.querySelector('.an-elapsed')), '8m');
  assert.match(CSS, /\.an-lst\[data-compact\] \.an-row \{ margin-top: 0/);
  // fewer than nine: regular again
  const few = await open({ route: 'sessions' }, only('needs-permission', 'work-ci'));
  assert.equal(few.$('.an-lst').getAttribute('data-compact'), null);
  assert.ok(row(few, 'work-ci').classList.contains('an-reg'));
  clean(page);
});

// ---- the frozen order --------------------------------------------------------------------------

test('the order is frozen while the pointer is over the list and released when it leaves', async () => {
  const page = await open();
  const before = rowIds(page).filter((id) => id.startsWith('review-'));
  assert.deepEqual(before, ['review-just-finished', 'review-darkmode', 'review-devserver']);
  page.fire('#an-list', 'pointerenter');
  page.emit('an:snapshot', snapshotWith((s) => {
    s.generated_at_ms += 1000;
    s.sessions.find((x) => x.session_id === 'review-devserver').since_ms = NOW - 1000;
    s.sessions.push(session('review-new', 'ready_for_review', { since_ms: NOW - 500, title: 'A newcomer' }));
  }));
  assert.deepEqual(rowIds(page).filter((id) => id.startsWith('review-')), [...before, 'review-new'], 'nothing moves under a click; the newcomer goes last');
  page.fire('#an-list', 'pointerleave');
  assert.deepEqual(rowIds(page).filter((id) => id.startsWith('review-')), ['review-new', 'review-devserver', 'review-just-finished', 'review-darkmode'], 'released: newest first');
  clean(page);
});

test('rows are marked for the FLIP glide (data-flip = the session) and the glide is 320 ms', async () => {
  const page = await open();
  for (const r of rows(page)) assert.equal(r.getAttribute('data-flip'), r.getAttribute('data-id'));
  // the dom has no layout: every row answers "100" the first time a render asks (before) and "140" the second (after)
  const glides = [];
  for (const el of page.$$('#an-rows [data-flip]')) {
    let asked = 0;
    el.getBoundingClientRect = () => ({ top: asked++ % 2 === 0 ? 100 : 140, bottom: 150, left: 0, right: 400, width: 400, height: 50 });
    el.animate = (frames, opts) => { glides.push([frames, opts]); return {}; };
  }
  page.click('.an-sh[data-key="head-working"] .an-sh-btn');
  assert.ok(glides.length > 0, 'the rows that stayed glide');
  for (const [frames, opts] of glides) {
    assert.equal(opts.duration, 320);
    assert.equal(opts.easing, 'cubic-bezier(.2,.8,.2,1)');
    assert.equal(frames[0].transform, 'translateY(-40px)');
  }
  // no glide when motion is still (a static scene or Windows' animation effects off)
  glides.length = 0;
  page.run('agentnotchCommon.setStatic(true)');
  page.click('.an-sh[data-key="head-working"] .an-sh-btn');
  assert.equal(glides.length, 0);
  clean(page);
});

// ---- elapsed labels -----------------------------------------------------------------------------

test('elapsed labels come from since_ms on the shared clock: durations for waits and turns, ages for done and idle', async () => {
  const p = pure();
  const at = (bucket, ago, patch) => p.L.elapsed(Object.assign(session('x', bucket, { since_ms: NOW - ago }), patch || {}), NOW);
  assert.equal(at('needs_you', 30000), '<1m');
  assert.equal(at('needs_you', 45 * 60000), '45m');
  assert.equal(at('needs_you', (2 * 60 + 13) * 60000), '2h 13m');
  assert.equal(at('needs_you', (3 * 24 + 5) * 3600000), '3d 5h');
  assert.equal(at('needs_you', 540000, { failed: true }), '9m', 'a failed turn is a wait, not an age');
  assert.equal(at('working', 14 * 60000), '14m');
  assert.equal(at('ready_for_review', 5000), 'just now');
  assert.equal(at('ready_for_review', 300000), '5m ago');
  assert.equal(at('idle', 3 * 3600000), '3h ago');
  assert.equal(at('idle', 2 * 86400000), '2d ago');
  assert.equal(at('idle', 0, { since_ms: 0 }), null, 'no time: nothing');
  assert.equal(p.L.elapsed(session('x', 'working', { since_ms: NOW + 60000 }), NOW), '<1m', 'a start in the future never reads negative');
});

test('the labels follow the clock without a snapshot; a card and a row never disagree', async () => {
  const page = await open();
  const label = (id) => text(row(page, id).querySelector('.an-elapsed'));
  assert.equal(label('needs-permission'), '2m');
  assert.equal(label('review-darkmode'), '5m ago');
  page.tick(3 * 60000);
  assert.equal(label('needs-permission'), '5m');
  assert.equal(label('review-darkmode'), '8m ago');
  page.tick(60 * 60000);
  assert.equal(label('needs-permission'), '1h 5m');
  assert.equal(label('work-migration'), '1h 17m');
  clean(page);
});

// ---- hover actions and clicks -----------------------------------------------------------------

test('the hover buttons: mark reviewed, dismiss a failure, show the terminal or editor, open the chat', async () => {
  const page = await open();
  const btn = (id, kind) => row(page, id).querySelector('.an-rb-' + kind);
  assert.equal(btn('review-darkmode', 'review').getAttribute('title'), 'Mark reviewed (Ctrl+R)');
  assert.equal(btn('needs-ratelimit', 'dismiss').getAttribute('title'), 'Dismiss (Ctrl+R)');
  assert.equal(btn('needs-ratelimit', 'review'), null);
  assert.equal(btn('needs-permission', 'review'), null, 'only review rows are reviewable');
  assert.equal(btn('needs-permission', 'jump').getAttribute('title'), 'Show terminal (Ctrl+J)');
  assert.equal(btn('needs-question', 'jump').getAttribute('title'), 'Show in editor (Ctrl+J)');
  assert.equal(btn('needs-permission', 'chat').getAttribute('title'), 'Open chat (Enter)');
  assert.equal(btn('needs-permission', 'chat').getAttribute('aria-label'), 'Open chat (Enter)');

  page.click(btn('review-darkmode', 'review'));
  assert.deepEqual(calls(page, 'mark_reviewed'), [{ session_id: 'review-darkmode', at_ms: NOW }]);
  page.click(btn('needs-ratelimit', 'dismiss'));
  assert.deepEqual(calls(page, 'dismiss_failure'), [{ session_id: 'needs-ratelimit' }]);
  page.click(btn('needs-elicitation', 'jump'));
  assert.deepEqual(calls(page, 'focus'), [{ session_id: 'needs-elicitation' }]);
  assert.equal(calls(page, 'panel_route').length, 0, 'the buttons take their own clicks: no chat opened');
  page.click(btn('needs-permission', 'chat'));
  assert.deepEqual(calls(page, 'panel_route'), [{ route: 'session:needs-permission' }]);
  clean(page);
});

test('a click anywhere on a row opens its chat; a session that cannot be jumped to has no jump button', async () => {
  const page = await open();
  page.click(row(page, 'work-ci').querySelector('.an-ctitle'));
  assert.deepEqual(calls(page, 'panel_route'), [{ route: 'session:work-ci' }]);
  assert.equal(state(page).route, 'session:work-ci');
  const none = await open({ route: 'sessions' }, (s) => {
    s.sessions.find((x) => x.session_id === 'idle-logo').focus_label = null;
    s.generated_at_ms += 0;
  });
  none.click('.an-foldsum');
  assert.equal(row(none, 'idle-logo').querySelector('.an-rb-jump'), null);
  clean(page);
});

test('the action bar has a named slot on every regular row for the next sub-task, empty until it is filled', async () => {
  const page = await open();
  for (const r of rows(page).filter((x) => x.classList.contains('an-reg'))) {
    const slot = r.querySelector('.an-row-actions');
    assert.ok(slot, r.getAttribute('data-id'));
    assert.equal(slot.getAttribute('data-slot'), 'actions');
    assert.equal(slot.childNodes.length, 0);
  }
  assert.match(CSS, /\.an-row-actions:empty \{ display: none/);
  const filled = await open();
  filled.run('agentnotchPanel.slots.actions = function (row) { return \'<span class="x-bar">\' + row.session_id + \'</span>\'; }');
  filled.run('agentnotchPanel._.render()');
  assert.equal(text(row(filled, 'needs-permission').querySelector('.an-row-actions .x-bar')), 'needs-permission');
  clean(page);
});

// ---- mark all reviewed, with undo --------------------------------------------------------------

const REVIEW = ['review-just-finished', 'review-darkmode', 'review-devserver'];

test('Mark all reviewed: the rows leave at once, the toast says how many, nothing is sent yet', async () => {
  const page = await open();
  page.click('[data-an-action="mark-all-reviewed"]');
  assert.equal(REVIEW.filter((id) => row(page, id)).length, 0, 'gone at once');
  assert.equal(page.$('.an-sh[data-key="head-ready_for_review"]'), null, 'and the section with them');
  assert.equal(text(page.$('.an-toast-t')), 'Marked 3 reviewed');
  assert.equal(page.$('.an-toast-undo').getAttribute('title'), 'Put them back in Ready for review (Ctrl+Z)');
  assert.equal(calls(page, 'mark_all_reviewed').length, 0);
  clean(page);
});

test('the pending review is sent when its 5 s end, with the click\'s time, and the toast goes', async () => {
  const page = await open();
  page.click('[data-an-action="mark-all-reviewed"]');
  page.tick(4999);
  assert.equal(calls(page, 'mark_all_reviewed').length, 0, 'not before 5 s');
  assert.ok(page.$('.an-toast'));
  page.tick(1);
  assert.deepEqual(calls(page, 'mark_all_reviewed'), [{ session_ids: REVIEW, at_ms: NOW }]);
  assert.equal(page.$('.an-toast'), null);
  page.tick(60000);
  assert.equal(calls(page, 'mark_all_reviewed').length, 1, 'once');
  clean(page);
});

test('the click\'s time is what is sent, not the moment the window ends (BHV-9)', async () => {
  const page = await open();
  page.tick(7000);
  page.click('[data-an-action="mark-all-reviewed"]');
  page.tick(5000);
  assert.equal(calls(page, 'mark_all_reviewed')[0].at_ms, NOW + 7000);
});

test('Undo puts the rows back, sends nothing now or later', async () => {
  const page = await open();
  page.click('[data-an-action="mark-all-reviewed"]');
  page.tick(2000);
  page.click('.an-toast-undo');
  assert.deepEqual(rowIds(page).filter((id) => id.startsWith('review-')), REVIEW);
  assert.equal(page.$('.an-toast'), null);
  page.tick(60000);
  assert.equal(calls(page, 'mark_all_reviewed').length, 0);
  assert.equal(state(page).pending, null);
  clean(page);
});

test('closing the panel commits the pending review at once; so does a second mark-all and a hidden window', async () => {
  const page = await open();
  page.click('[data-an-action="mark-all-reviewed"]');
  page.click('[data-an-action="close"]');
  assert.equal(calls(page, 'mark_all_reviewed').length, 1);
  assert.equal(calls(page, 'panel_close').length, 1);
  assert.ok(page.hub.calls.findIndex((c) => c.method === 'mark_all_reviewed') < page.hub.calls.findIndex((c) => c.method === 'panel_close'), 'sent before the panel closes');
  page.tick(10000);
  assert.equal(calls(page, 'mark_all_reviewed').length, 1, 'and never again');

  const second = await open({ route: 'sessions' }, (s) => {
    s.sessions.push(session('review-x', 'ready_for_review', { title: 'Later one' }));
  });
  second.click('[data-an-action="mark-all-reviewed"]');
  second.emit('an:snapshot', snapshotWith((s) => {
    s.generated_at_ms += 1000;
    s.sessions.push(session('review-x', 'ready_for_review', { title: 'Later one' }));
    s.sessions.push(session('review-y', 'ready_for_review', { title: 'Another', since_ms: NOW + 1500 }));
  }));
  assert.ok(row(second, 'review-y'), 'a review that arrived after the click is not hidden');
  second.click('[data-an-action="mark-all-reviewed"]');
  assert.equal(calls(second, 'mark_all_reviewed').length, 1, 'the first is sent before the second starts');
  assert.equal(calls(second, 'mark_all_reviewed')[0].session_ids.includes('review-y'), false);

  const hidden = await open();
  hidden.click('[data-an-action="mark-all-reviewed"]');
  hidden.run('Object.defineProperty(document, "hidden", {value: true, configurable: true}); document.dispatchEvent(new Event("visibilitychange"))');
  assert.equal(calls(hidden, 'mark_all_reviewed').length, 1, 'a hidden window can no longer be undone from');
});

test('a review row that finished again after the click is not hidden by the pending set', async () => {
  const page = await open();
  page.click('[data-an-action="mark-all-reviewed"]');
  page.emit('an:snapshot', snapshotWith((s) => {
    s.generated_at_ms += 1000;
    s.sessions.find((x) => x.session_id === 'review-darkmode').since_ms = NOW + 500;
  }));
  assert.deepEqual(rowIds(page).filter((id) => id.startsWith('review-')), ['review-darkmode']);
  clean(page);
});

test('the gear menu\'s Mark all reviewed does the same; the failed rows of Needs you are not part of it', async () => {
  const page = await open();
  page.click('[data-an-action="gear"]');
  page.click('.an-mi[data-an-action="mark-all-reviewed"]');
  assert.equal(text(page.$('.an-toast-t')), 'Marked 3 reviewed');
  assert.ok(row(page, 'needs-ratelimit'), 'a failed turn stays');
  page.tick(5000);
  assert.deepEqual(calls(page, 'mark_all_reviewed')[0].session_ids, REVIEW);
  const filtered = await open({ route: 'sessions', ring_id: WORK });
  filtered.click('[data-an-action="mark-all-reviewed"]');
  filtered.tick(5000);
  assert.deepEqual(calls(filtered, 'mark_all_reviewed')[0].session_ids, ['review-devserver'], 'only what the filter shows');
  clean(page);
});

test('the toast\'s Undo also has the keyboard\'s entry: agentnotchPanel.undoReview()', async () => {
  const page = await open();
  page.click('[data-an-action="mark-all-reviewed"]');
  page.run('agentnotchPanel.undoReview()');
  assert.equal(rowIds(page).filter((id) => id.startsWith('review-')).length, 3);
  page.run('agentnotchPanel.undoReview()');
  clean(page);
});

// ---- hostile strings and long ones -------------------------------------------------------------

test('hostile strings in title, project, detail, request, account and a11y are drawn as text', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await open({ route: 'sessions' }, (s) => {
      const r = s.sessions;
      r.find((x) => x.session_id === 'needs-permission').title = evil;
      r.find((x) => x.session_id === 'needs-permission').project = evil;
      r.find((x) => x.session_id === 'needs-permission').detail = { kind: 'permission', tool: evil, request: evil, waiting_in_terminal: false };
      r.find((x) => x.session_id === 'needs-permission').a11y = evil;
      r.find((x) => x.session_id === 'needs-permission').account_label = evil;
      r.find((x) => x.session_id === 'needs-question').detail = { kind: 'question', text: evil };
      r.find((x) => x.session_id === 'needs-elicitation').detail = { kind: 'dialog', text: evil };
      r.find((x) => x.session_id === 'needs-ratelimit').detail = { kind: 'failed', text: evil };
      r.find((x) => x.session_id === 'review-darkmode').detail = { kind: 'review', text: evil };
      r.find((x) => x.session_id === 'work-ci').detail = { kind: 'working', text: evil, secondary: true };
      r.find((x) => x.session_id === 'idle-notch').detail = { kind: 'idle', text: evil };
      r.find((x) => x.session_id === 'idle-notch').title = evil;
      r.find((x) => x.session_id === 'idle-readme').title = evil;
    });
    assert.deepEqual(audit.problems(page.$('#an-rows')), [], evil.slice(0, 30));
    page.click('.an-foldsum');
    assert.deepEqual(audit.problems(page.$('#an-rows')), [], 'folded and unfolded: ' + evil.slice(0, 30));
    assert.deepEqual(page.$$('#an-rows script, #an-rows iframe, #an-rows style, #an-rows textarea, #an-rows img').length, 0);
    clean(page);
  }
  // the check bites: the raw string is markup
  assert.notDeepEqual(audit.problems(audit.HOSTILE[0]), []);
  const p = pure();
  assert.deepEqual(audit.problems(p.html(snap([session('<b>', 'working', { title: audit.HOSTILE[0] })]), {})), []);
  assert.deepEqual(audit.problems(p.L.toastHtml({ ids: ['<img src=x onerror=1>'] })), []);
});

test('hostile session ids and labels keep the attributes intact and come back as the same id', async () => {
  const id = '"><script>alert(1)</script>';
  const page = await open({ route: 'sessions' }, (s) => { s.sessions.find((x) => x.session_id === 'work-ci').session_id = id; });
  const el = row(page, id);
  assert.ok(el);
  assert.equal(el.getAttribute('data-an-arg'), id);
  page.click(el.querySelector('.an-ctitle'));
  assert.deepEqual(calls(page, 'panel_route'), [{ route: 'session:' + id }]);
  clean(page);
});

test('a 10 000-character title is cut in the markup, still one line with an ellipsis, and the row stays whole', async () => {
  const long = 'A'.repeat(10000);
  const page = await open({ route: 'sessions' }, (s) => {
    only('needs-permission', 'work-ci', 'work-summary')(s);
    s.sessions.find((x) => x.session_id === 'work-ci').title = long;
    s.sessions.find((x) => x.session_id === 'work-ci').detail.text = 'B'.repeat(10000);
    s.sessions.find((x) => x.session_id === 'needs-permission').detail.request = 'C'.repeat(10000);
    s.sessions.find((x) => x.session_id === 'needs-permission').project = 'D'.repeat(10000);
    s.sessions.find((x) => x.session_id === 'needs-permission').account_label = 'E'.repeat(10000);
  });
  const title = row(page, 'work-ci').querySelector('.an-rtitle');
  assert.ok(title.textContent.length < 400, String(title.textContent.length));
  assert.ok(title.textContent.endsWith('…'));
  assert.ok(row(page, 'work-ci').querySelector('.an-detail').textContent.length < 600);
  assert.ok(page.$('#an-rows').innerHTML.length < 60000, 'the DOM stays small: ' + page.$('#an-rows').innerHTML.length);
  const perm = row(page, 'needs-permission');
  assert.ok(perm.querySelector('.an-proj').textContent.length <= 120);
  assert.ok(perm.querySelector('.an-acct-l').textContent.length <= 80);
  // the CSS: one line with an ellipsis for titles, projects and account labels; wrap for requests
  assert.match(CSS, /\.an-rtitle \{[^}]*text-overflow: ellipsis; white-space: nowrap/);
  assert.match(CSS, /\.an-code \{[^}]*overflow-wrap: anywhere/);
  assert.match(CSS, /\.an-meta > \.an-proj \{ flex: 0 1000 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis/);
  assert.equal(row(page, 'needs-permission').getAttribute('aria-label').length <= 600, true);
  clean(page);
});

// ---- a re-render keeps what the user is doing --------------------------------------------------

test('a snapshot re-render keeps the selection, the scroll position, the fold and the elements', async () => {
  const page = await open({ route: 'sessions', highlight: 'needs-question' });
  const list = page.$('#an-list');
  list.scrollTop = 321;
  page.click('.an-sh[data-key="head-working"] .an-sh-btn');
  const before = row(page, 'needs-question');
  assert.ok(before.classList.contains('an-sel'));
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1000; s.sessions.find((x) => x.session_id === 'needs-question').title = 'Pick a charting library, now'; }));
  assert.equal(row(page, 'needs-question'), before, 'the same element');
  assert.equal(text(before.querySelector('.an-rtitle')), 'Pick a charting library, now');
  assert.ok(before.classList.contains('an-sel'));
  assert.equal(page.$('#an-list').scrollTop, 321);
  assert.equal(page.$('.an-foldsum[data-key="folded-working"]') !== null, true);
  clean(page);
});

test('every render of the list is idempotent: the same snapshot again changes no node', async () => {
  const page = await open();
  const all = rows(page);
  const before = page.$('#an-rows').innerHTML;
  page.run('agentnotchPanel._.render()');
  assert.equal(page.$('#an-rows').innerHTML, before);
  assert.deepEqual(rows(page), all);
});

// ---- the scenes ----------------------------------------------------------------------------------

test('the list scenes render: regular rows, busy, undo, keyboard-folded, filtered; the counts follow the rows', async () => {
  const page = await open();
  const show = (name) => { assert.equal(page.run(`agentnotchPanel.showScene(${JSON.stringify(name)})`), true, name); return page; };
  show('panel-regular-rows');
  assert.equal(rows(page).length, 8);
  assert.equal(page.$('.an-lst').getAttribute('data-compact'), null, 'eight rows are still regular');
  show('panel-busy-window');
  assert.equal(rows(page).length, 19, '3 + 6 + 10 rows; the 6 idle ones are folded to a line');
  const ids = rowIds(page);
  assert.equal(new Set(ids).size, ids.length, 'cloned rows have distinct ids');
  assert.equal(page.$('.an-lst').getAttribute('data-compact'), '1');
  const chip = page.$$('.an-chip')[0].children.map((c) => text(c)).join(' ');
  assert.match(chip, /^All 25/, 'the header counts the 25 sessions');
  show('panel-busy-full');
  assert.equal(rows(page).length, 19);
  show('panel-undo');
  assert.equal(text(page.$('.an-toast-t')), 'Marked 2 reviewed');
  assert.equal(calls(page, 'mark_all_reviewed').length, 0, 'a scene never sends');
  page.tick(60000);
  assert.equal(calls(page, 'mark_all_reviewed').length, 0);
  show('panel-keyboard-folded');
  assert.ok(row(page, 'needs-question').classList.contains('an-sel'));
  assert.deepEqual(page.$$('.an-foldsum').map((f) => f.getAttribute('data-key')), ['folded-working', 'folded-idle']);
  assert.equal(page.$('.an-toast'), null, 'the earlier scene\'s toast is gone');
  show('panel-filtered');
  assert.ok(rows(page).length > 0 && rows(page).length < 15);
  show('panel-every-state');
  assert.equal(rows(page).length, 11);
  assert.equal(page.run('agentnotchPanel.showScene("panel-nonsense")'), false);
  clean(page);
});

test('layoutReport tolerates a title cut on purpose with an ellipsis and flags text cut without one', async () => {
  const page = await open();
  const report = () => plain(page.run('agentnotchPanel.layoutReport()'));
  const title = row(page, 'work-ci').querySelector('.an-ctitle');
  title.__rect = { left: 0, top: 0, right: 200, bottom: 20, width: 200, height: 20 };
  title.__scrollWidth = 500;
  page.run('window.getComputedStyle = function () { return { textOverflow: "ellipsis", getPropertyValue: function () { return ""; } }; }');
  assert.ok(title.hasAttribute('data-an-clip'));
  assert.deepEqual(report().failures.filter((f) => /clipped/.test(f)), []);
  page.run('window.getComputedStyle = function () { return { textOverflow: "clip", getPropertyValue: function () { return ""; } }; }');
  assert.ok(report().failures.some((f) => /text is clipped/.test(f)));
  const plainText = row(page, 'work-ci').querySelector('.an-elapsed');
  assert.equal(plainText.hasAttribute('data-an-clip'), false, 'an elapsed label is never cut');
});
