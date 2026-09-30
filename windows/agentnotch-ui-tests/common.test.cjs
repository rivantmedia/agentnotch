'use strict';
// ui/agentnotch/common.js on its own: the escaper, the formatters, the answer gate, the key
// router, the browser-accelerator list, routes and the DOM morph. The Swift tests it ports are
// named in each section (UsageFormatterTests, ElapsedCopyTests, B_KeyRouterTests, the AnswerGate
// tests of B_PanelStateTests and B_ReviewFixesTests); the Mac's Command key is Ctrl here and
// its Option key is Alt.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const dom = require('./lib/dom.cjs');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');

const COMMON = path.join(harness.UI, 'agentnotch', 'common.js');
const SOURCE = fs.readFileSync(COMMON, 'utf8');

/** common.js in a context of its own, with a document, a hand-moved clock and no bridge. */
function fresh(options) {
  const opts = Object.assign({}, options);
  const clock = harness.createClock(opts.now === undefined ? harness.NOW : opts.now);
  const document = dom.parseDocument('<!doctype html><html><head></head><body></body></html>');
  const sandbox = {};
  const context = vm.createContext(sandbox);
  const window = vm.runInContext('this', context);
  const media = { reduce: !!opts.reducedMotion };
  Object.assign(window, {
    window, document, console,
    Promise,
    matchMedia: (q) => ({ matches: q.includes('reduce') && media.reduce }),
  });
  if (opts.tauri) window.__TAURI__ = opts.tauri;
  vm.runInContext(SOURCE, context, { filename: COMMON });
  const C = window.agentnotchCommon;
  C.setClock(clock.now);
  return { C, document, window, clock, media };
}

// Objects built inside the page's vm context have that context's prototypes; deepStrictEqual
// compares prototypes, so results are compared as plain data.
const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

const { C } = fresh();

// ---- shape ----------------------------------------------------------------------------------

test('common.js defines exactly one global and no top-level lexical binding', () => {
  const context = vm.createContext({});
  const before = new Set(Object.getOwnPropertyNames(vm.runInContext('this', context)));
  const window = vm.runInContext('this', context);
  Object.assign(window, { window, document: dom.parseDocument('<html><head></head><body></body></html>') });
  vm.runInContext(SOURCE, context);
  const added = Object.getOwnPropertyNames(window).filter((n) => !before.has(n) && !['window', 'document'].includes(n));
  assert.deepEqual(added, ['agentnotchCommon']);
  // A `let x` at the top level would be visible to a later script through the shared scope.
  assert.equal(vm.runInContext("typeof C === 'undefined' && typeof ESCAPES === 'undefined' && typeof clock === 'undefined'", context), true);
  assert.doesNotMatch(SOURCE, /^(let|const|class)\s/m);
  assert.match(SOURCE, /^\(function \(\) \{\n {2}'use strict';/m);
});

test('the constants match the Mac', () => {
  assert.equal(C.ARM_DELAY_MS, 350);
  assert.equal(C.ANSWERED_MEMORY_MS, 600000);
  assert.equal(C.MAX_INLINE_OPTIONS, 4);
  assert.equal(C.ACCOUNT_HUES.length, 8);
});

// ---- escaping ---------------------------------------------------------------------------------

test('esc escapes the six characters that matter, once, and turns null into nothing', () => {
  assert.equal(C.esc('<a href="x" onclick=\'y\'>&`'), '&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;&#96;');
  assert.equal(C.esc('&amp;'), '&amp;amp;');
  assert.equal(C.esc(null), '');
  assert.equal(C.esc(undefined), '');
  assert.equal(C.esc(0), '0');
  assert.equal(C.esc(false), 'false');
  assert.equal(C.esc({ toString: () => '<b>' }), '&lt;b&gt;');
  assert.equal(C.esc('plain text, ünïcode ‮ and emoji'), 'plain text, ünïcode ‮ and emoji');
});

test('every hostile string is text through esc: in an element, in a quoted attribute, in the morph', () => {
  const { C: c, document } = fresh();
  for (const raw of audit.HOSTILE.concat(['x'.repeat(10000) + '<b>', '‮<img src=x onerror=1>'])) {
    const safe = c.esc(raw);
    const inText = dom.parseFragment(`<div class="t">${safe}</div>`);
    assert.deepEqual(audit.problems(inText), [], `text: ${raw.slice(0, 40)}`);
    assert.equal(inText.querySelector('.t').textContent, raw);
    const inDouble = dom.parseFragment(`<div class="t" title="${safe}" aria-label="${safe}">x</div>`).querySelector('.t');
    assert.equal(inDouble.getAttribute('title'), raw);
    assert.equal(inDouble.attributes.length, 3);
    const inSingle = dom.parseFragment(`<div class="t" title='${safe}'>x</div>`).querySelector('.t');
    assert.equal(inSingle.getAttribute('title'), raw);
    assert.equal(inSingle.attributes.length, 2);
    const host = document.createElement('div');
    c.morph(host, `<p data-an-text data-key="${safe}">${safe}</p>`);
    assert.deepEqual(audit.problems(host), [], `morph: ${raw.slice(0, 40)}`);
    assert.equal(host.querySelector('p').textContent, raw);
  }
});

test('oneLine, plural, needYou and clause', () => {
  assert.equal(C.oneLine('  a \n\t b   c '), 'a b c');
  assert.equal(C.oneLine(null), '');
  assert.equal(C.plural(1, 'session'), '1 session');
  assert.equal(C.plural(3, 'session'), '3 sessions');
  assert.equal(C.plural(2, 'child', 'children'), '2 children');
  assert.equal(C.needYou(1), '1 needs you');
  assert.equal(C.needYou(2), '2 need you');
  assert.equal(C.clause('Typing replies is off.'), 'Typing replies is off');
  assert.equal(C.clause(' done… '), 'done…');
  assert.equal(C.clause(null), '');
});

// ---- formatting (UsageFormatterTests, ElapsedCopyTests) -----------------------------------------

const S = 1000;
const MIN = 60 * S;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

test('duration is compact (UsageFormatterTests.durationIsCompact)', () => {
  assert.equal(C.duration(0), '<1m');
  assert.equal(C.duration(59 * S), '<1m');
  assert.equal(C.duration(-30 * S), '<1m');
  assert.equal(C.duration(MIN), '1m');
  assert.equal(C.duration(45 * MIN), '45m');
  assert.equal(C.duration(2 * HOUR + 13 * MIN), '2h 13m');
  assert.equal(C.duration(2 * HOUR), '2h');
  assert.equal(C.duration(3 * DAY + 5 * HOUR), '3d 5h');
  assert.equal(C.duration(3 * DAY + 30 * MIN), '3d');
});

test('percent rounds down and keeps overflow (UsageFormatterTests.percentRoundsDownAndKeepsOverflow)', () => {
  assert.equal(C.percent(0), '0%');
  assert.equal(C.percent(23.7), '23%');
  assert.equal(C.percent(99.99), '99%');
  assert.equal(C.percent(110.2), '110%');
  assert.equal(C.percent(-3), '0%');
  assert.equal(C.percent(NaN), '–');
  assert.equal(C.percent('12.9'), '12%');
});

test('freshness reads "just now" for the future and rounds down (UsageFormatterTests.freshness)', () => {
  assert.equal(C.age(30 * S), 'just now');
  assert.equal(C.age(-30 * S), 'just now');
  assert.equal(C.age(59 * S), 'just now');
  assert.equal(C.age(90 * S), '1m ago');
  assert.equal(C.age(3 * HOUR + 5 * MIN), '3h ago');
  assert.equal(C.age(2 * DAY), '2d ago');
});

test('bad or huge numbers never leave the formatters (UsageFormatterTests.survivesNonFiniteAndHugeValues)', () => {
  assert.equal(C.percent(Infinity), '–');
  assert.equal(C.percent(1e300), '999%');
  assert.equal(C.duration(NaN), '<1m');
  assert.equal(C.duration(Infinity), '10000d');
  assert.equal(C.duration(-Infinity), '<1m');
  assert.equal(C.duration(1e300), '10000d');
  assert.equal(C.age(NaN), 'just now');
  assert.equal(C.age(1e300), '10000d ago');
  assert.equal(C.age(undefined), 'just now');
  assert.equal(C.cardElapsed(NaN), 'just now');
  assert.match(C.cardElapsed(1e300), /^\d+ hr( \d+ min)?$/);
  assert.match(C.cardElapsed(Infinity), /^\d+ hr( \d+ min)?$/);
});

test('the hover card\'s elapsed words (ElapsedCopyTests)', () => {
  assert.equal(C.cardElapsed(5 * S), 'just now');
  assert.equal(C.cardElapsed(44 * S), 'just now');
  assert.equal(C.cardElapsed(45 * S), '1 min');
  assert.equal(C.cardElapsed(6 * MIN), '6 min');
  assert.equal(C.cardElapsed(59 * MIN), '59 min');
  assert.equal(C.cardElapsed(60 * MIN), '1 hr');
  assert.equal(C.cardElapsed(65 * MIN), '1 hr 5 min');
  assert.equal(C.cardElapsed(2 * HOUR + 10 * MIN), '2 hr 10 min');
  assert.equal(C.cardElapsed(-120 * S), 'just now');
});

test('a count badge reads "", 1 to 9, then 9+ (ClaudeRingBadgeLayout.label)', () => {
  assert.equal(C.badgeLabel(0), '');
  assert.equal(C.badgeLabel(-2), '');
  assert.equal(C.badgeLabel(NaN), '');
  assert.equal(C.badgeLabel(undefined), '');
  for (let n = 1; n <= 9; n++) assert.equal(C.badgeLabel(n), String(n));
  assert.equal(C.badgeLabel(10), '9+');
  assert.equal(C.badgeLabel(99), '9+');
  assert.equal(C.badgeLabel(3.9), '3');
});

test('the context meter turns high at 80 and critical at 90 (ContextMeter.level)', () => {
  assert.equal(C.contextLevel(0), 'normal');
  assert.equal(C.contextLevel(79.9), 'normal');
  assert.equal(C.contextLevel(80), 'high');
  assert.equal(C.contextLevel(89.9), 'high');
  assert.equal(C.contextLevel(90), 'critical');
  assert.equal(C.contextLevel(100), 'critical');
  assert.equal(C.contextLevel(NaN), 'normal');
});

// ---- colours and marks --------------------------------------------------------------------------

test('account hues wrap, including negative and fractional indexes', () => {
  assert.equal(C.accountHue(0), '#5C9EFA');
  assert.equal(C.accountHue(8), '#5C9EFA');
  assert.equal(C.accountHue(-1), C.ACCOUNT_HUES[7]);
  assert.equal(C.accountHue(2.9), C.ACCOUNT_HUES[2]);
  assert.equal(C.accountHue(undefined), '#5C9EFA');
  assert.deepEqual(audit.problems(C.accountDot(3, 6)), []);
});

test('status rings: working spins, needs-you breathes on a key, the rest are still', () => {
  const html = (kind, o) => dom.parseFragment(C.statusRing(kind, o)).querySelector('svg');
  assert.ok(html('working').classList.contains('an-spin'));
  assert.ok(html('needs', { breathKey: 4 }).classList.contains('an-breathe'));
  assert.equal(html('needs', { breathKey: 4 }).getAttribute('data-key'), 'breath-4');
  assert.equal(html('needs').getAttribute('data-key'), null);
  assert.equal(html('working', { breathKey: 4 }).getAttribute('data-key'), null);
  assert.equal(html('error').querySelectorAll('circle').length, 1);
  assert.equal(html('review').getAttribute('aria-hidden'), 'true');
  // A key from data is escaped.
  for (const raw of audit.HOSTILE) assert.deepEqual(audit.problems(C.statusRing('needs', { breathKey: raw })), [], raw.slice(0, 30));
  assert.equal(C.glyphKind({ failed: true, bucket: 'working' }), 'error');
  assert.equal(C.glyphKind({ bucket: 'needs_you' }), 'needs');
  assert.equal(C.glyphKind({ bucket: 'ready_for_review' }), 'review');
  assert.equal(C.glyphKind({ bucket: 'working' }), 'working');
  assert.equal(C.glyphKind({ bucket: 'idle' }), 'idle');
});

test('tool marks follow the tool\'s status', () => {
  const cls = (status) => dom.parseFragment(C.toolMark(status)).querySelector('svg').getAttribute('class');
  assert.match(cls('running'), /an-spin/);
  assert.match(cls('waiting_for_approval'), /an-breathe/);
  assert.match(cls('error'), /an-ring-error/);
  assert.match(cls('interrupted'), /an-ring-error/);
  assert.match(cls('done'), /an-ring-idle/);
  assert.match(cls('done'), /an-toolmark/);
});

test('icons draw known names only, as clean markup', () => {
  for (const name of ['chevronLeft', 'xmark', 'pin', 'gear', 'check', 'terminal', 'warning', 'photo']) {
    assert.equal(C.hasIcon(name), true, name);
    assert.deepEqual(audit.problems(C.icon(name, 'x')), [], name);
    assert.match(C.icon(name, 'x'), /class="an-icon x"/);
  }
  assert.equal(C.hasIcon('nope'), false);
  assert.equal(C.hasIcon('constructor'), false);
  assert.equal(C.hasIcon('__proto__'), false);
  assert.equal(C.icon('constructor'), '<svg class="an-icon" viewBox="0 0 12 12" aria-hidden="true" focusable="false"></svg>');
  assert.equal(C.icon('nope').includes('<path'), false);
});

// ---- the clock ------------------------------------------------------------------------------

test('every time reads the clock the page set', () => {
  const f = fresh({ now: 5000 });
  assert.equal(f.C.now(), 5000);
  f.clock.tick(250);
  assert.equal(f.C.now(), 5250);
  f.C.setClock(() => 7);
  assert.equal(f.C.now(), 7);
  f.C.setClock(null);
  assert.equal(typeof f.C.now(), 'number');
  assert.ok(Math.abs(f.C.now() - Date.now()) < 5000);
});

// ---- the bridge --------------------------------------------------------------------------------

test('call sends {method, args} to an_call, null for no arguments, and reports {code, message}', async () => {
  const seen = [];
  const f = fresh({
    tauri: {
      core: {
        invoke(cmd, args) {
          seen.push([cmd, args]);
          if (args.method === 'boom') return Promise.reject({ code: 'sealed', message: 'Sealed.' });
          if (args.method === 'text') return Promise.reject('plain string');
          if (args.method === 'raw') return Promise.reject(new Error('io'));
          return Promise.resolve({ ok: args.method });
        },
      },
      event: { listen: (name, cb) => { seen.push(['listen', name]); cb({ payload: { n: 1 } }); return Promise.resolve(() => {}); } },
    },
  });
  assert.deepEqual(plain(await f.C.call('snapshot')), { ok: 'snapshot' });
  assert.deepEqual(plain(await f.C.call('focus', { session_id: 'a' })), { ok: 'focus' });
  assert.deepEqual(plain(seen[0]), ['an_call', { method: 'snapshot', args: null }]);
  assert.deepEqual(plain(seen[1]), ['an_call', { method: 'focus', args: { session_id: 'a' } }]);
  await assert.rejects(f.C.call('boom'), (e) => e.code === 'sealed' && e.message === 'Sealed.');
  await assert.rejects(f.C.call('text'), (e) => e.code === 'failed' && e.message === 'plain string');
  await assert.rejects(f.C.call('raw'), (e) => e.code === 'failed' && e.message === 'io');
  const got = [];
  await f.C.listen('an:snapshot', (payload) => got.push(payload));
  assert.deepEqual(plain(got), [{ n: 1 }]);
  assert.deepEqual(plain(f.C.callError(null)), { code: 'failed', message: 'failed' });
});

test('without the bridge a call rejects like any other failure and listen is quiet', async () => {
  const f = fresh();
  await assert.rejects(f.C.call('snapshot'), (e) => e.code === 'failed' && /isn't reachable/.test(e.message));
  await assert.rejects(f.C.invokeRaw('get_theme'), /no bridge/);
  const off = await f.C.listen('an:snapshot', () => {});
  assert.equal(typeof off, 'function');
  f.C.log('a line');
});

test('log calls the glue\'s log and never rejects', async () => {
  const seen = [];
  const f = fresh({ tauri: { core: { invoke: (cmd, args) => { seen.push(args); return Promise.reject({ code: 'refused', message: 'no' }); } } } });
  f.C.log(42);
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(plain(seen), [{ method: 'log', args: { msg: '42' } }]);
});

// ---- the answer gate (B_PanelStateTests.B_AnswerGateTests, B_ReviewFixesTests) -------------------

test('a new request waits 350 ms before it can be answered', () => {
  const gate = C.createAnswerGate();
  const clock = harness.createClock(1_000_000);
  const t0 = clock.now();
  gate.noteShown(['a'], t0);
  assert.equal(gate.isArmed('a', t0), false);
  clock.tick(100);
  assert.equal(gate.isArmed('a', clock.now()), false);
  assert.equal(gate.claim('a', clock.now()), false);
  clock.tick(249);
  assert.equal(gate.isArmed('a', clock.now()), false, '349 ms');
  clock.tick(1);
  assert.equal(gate.isArmed('a', clock.now()), true, '350 ms');
  assert.equal(gate.nextArming(t0), t0 + 350);
  assert.equal(gate.nextArming(t0 + 1000), null);
});

test('a double-click sends one answer', () => {
  const gate = C.createAnswerGate();
  gate.noteShown(['a'], 0);
  assert.equal(gate.claim('a', 1000), true);
  assert.equal(gate.claim('a', 1100), false);
  assert.equal(gate.wasAnswered('a'), true);
});

test('the request that replaces an answered one is not answered by the second click', () => {
  const gate = C.createAnswerGate();
  gate.noteShown(['a'], 0);
  assert.equal(gate.claim('a', 1000), true);
  // The queue promotes b into the same row 100 ms later; the double-click's second click lands
  // 120 ms after the first.
  gate.noteShown(['b'], 1100);
  assert.equal(gate.claim('b', 1120), false);
  assert.equal(gate.claim('b', 1500), true);
});

test('unknown and forgotten requests cannot be answered', () => {
  const gate = C.createAnswerGate();
  assert.equal(gate.claim('x', 0), false);
  gate.noteShown(['a'], 0);
  gate.noteShown([], 5000);
  assert.equal(gate.isArmed('a', 10000), false);
  assert.equal(gate.claim('a', 10000), false);
  assert.equal(gate.nextArming(0), null);
});

test('requests shown armed can be answered at once', () => {
  const gate = C.createAnswerGate();
  gate.noteShownArmed(['a']);
  assert.equal(gate.isArmed('a', 0), true);
  assert.equal(gate.claim('a', 0), true);
  assert.equal(gate.claim('a', 0), false);
  // A request already known keeps the time it was first shown.
  const other = C.createAnswerGate();
  other.noteShown(['b'], 1000);
  other.noteShownArmed(['b']);
  assert.equal(other.isArmed('b', 1100), false);
});

test('an answered request stays answered after leaving the screen, for 600 s', () => {
  const gate = C.createAnswerGate();
  gate.noteShown(['a'], 0);
  assert.equal(gate.claim('a', 1000), true);
  // The chat opens on another session, then the list comes back while the engine still shows a.
  gate.noteShown(['b'], 1200);
  gate.noteShown(['a', 'b'], 1400);
  assert.equal(gate.isArmed('a', 3000), false);
  assert.equal(gate.claim('a', 3000), false);
  // Only b's wait is ahead: the answered one plans no redraw.
  assert.equal(gate.nextArming(1400), 1200 + 350);
  assert.equal(gate.nextArming(2000), null);
  // Just inside the memory it is still answered; past it the memory is let go.
  gate.noteShown(['b'], 1000 + 599_999);
  assert.equal(gate.wasAnswered('a'), true);
  gate.noteShown(['b'], 1000 + 600_000);
  assert.equal(gate.wasAnswered('a'), false);
  gate.noteShown(['a'], 1000 + 600_001);
  assert.equal(gate.isArmed('a', 1000 + 600_001 + 349), false);
  assert.equal(gate.isArmed('a', 1000 + 600_001 + 350), true);
});

test('the gate keeps a timer plan: nextArming is the soonest of several waiting requests', () => {
  const gate = C.createAnswerGate();
  gate.noteShown(['a'], 0);
  gate.noteShown(['a', 'b'], 200);
  assert.equal(gate.nextArming(100), 350);
  assert.equal(gate.nextArming(350), 550);
  assert.equal(gate.nextArming(550), null);
});

test('gates do not share state', () => {
  const one = C.createAnswerGate();
  const two = C.createAnswerGate();
  one.noteShownArmed(['a']);
  assert.equal(one.claim('a', 0), true);
  assert.equal(two.claim('a', 0), false);
});

// ---- the key router (B_KeyRouterTests) -----------------------------------------------------------

const ALWAYS_INLINE = { always: "Don't ask again for Bash(ls)", inline_always: true };
const ALWAYS_CHAT = { always: 'Switch to accept-edits mode', inline_always: false };
const permissionRow = (extra) => Object.assign({
  session_id: 's', bucket: 'needs_you', failed: false, focus_label: 'Show terminal', detail: { kind: 'permission' },
  pending: Object.assign({ kind: 'permission', tool_use_id: 't', always: null, inline_always: false, needs_review: false }, extra),
}, {});
const CTRL = { ctrl: true };
const CTRL_ALT = { ctrl: true, alt: true };
const CTRL_SHIFT = { ctrl: true, shift: true };

const actionsOf = {
  none: { kind: 'none' },
  permission: (always, needsReview) => C.primaryActions(permissionRow({ always: always ? always.always : null, inline_always: always ? always.inline_always : false, needs_review: !!needsReview })),
  plan: (id) => ({ kind: 'plan', toolUseId: id }),
  answerInChat: (id) => ({ kind: 'answer_in_chat', toolUseId: id }),
  chips: (id) => ({ kind: 'question_chips', toolUseId: id, question: { options: [{ label: 'A' }, { label: 'B' }] } }),
};

function target(actions, opts) {
  const o = Object.assign({ reviewable: false, jump: true, dismiss: false }, opts);
  return { sessionId: 's', actions, canMarkReviewed: o.reviewable, canDismissFailure: o.dismiss, canJump: o.jump };
}
const list = (actions, opts) => ({ kind: 'list', target: target(actions, opts) });
const chat = (actions, typing, opts) => ({ kind: 'chat', target: target(actions, opts), typing: !!typing });
const NO_SELECTION = { kind: 'list', target: null };
const cmd = (key, mods, context) => plain(C.commandFor(key, mods || {}, context));

test('arrows move the selection and Return opens the chat', () => {
  assert.deepEqual(cmd('up', {}, NO_SELECTION), { cmd: 'move', delta: -1 });
  assert.deepEqual(cmd('down', {}, NO_SELECTION), { cmd: 'move', delta: 1 });
  assert.deepEqual(cmd('return', {}, list(actionsOf.none)), { cmd: 'openChat', sessionId: 's' });
  assert.equal(cmd('return', {}, NO_SELECTION), null);
});

test('Ctrl+Enter is the row\'s primary action', () => {
  const permission = actionsOf.permission(ALWAYS_INLINE, false);
  assert.deepEqual(cmd('return', CTRL, list(permission)), { cmd: 'allow', sessionId: 's', toolUseId: 't' });
  assert.deepEqual(cmd('return', CTRL, list(actionsOf.plan('p'))), { cmd: 'approvePlan', sessionId: 's', toolUseId: 'p' });
  assert.deepEqual(cmd('return', CTRL, list(actionsOf.none, { reviewable: true })), { cmd: 'markReviewed', sessionId: 's' });
  assert.equal(cmd('return', CTRL, list(actionsOf.none)), null);
  assert.deepEqual(cmd('return', CTRL, list(actionsOf.answerInChat('q'))), { cmd: 'openChat', sessionId: 's' });
  // Too long to judge from the row: open it whole instead of allowing.
  const long = actionsOf.permission(null, true);
  assert.deepEqual(cmd('return', CTRL, list(long)), { cmd: 'openChat', sessionId: 's' });
  assert.deepEqual(cmd('return', CTRL, chat(long, false)), { cmd: 'allow', sessionId: 's', toolUseId: 't' });
});

test('approvals never fire on a bare key', () => {
  const permission = actionsOf.permission(ALWAYS_INLINE, false);
  assert.deepEqual(cmd('return', {}, list(permission)), { cmd: 'openChat', sessionId: 's' });
  assert.equal(cmd('delete', {}, list(permission)), null);
  assert.equal(cmd('y', {}, list(permission)), null);
  // Alt or Shift without Ctrl is not an approval either.
  assert.equal(cmd('return', { alt: true }, list(permission)), null);
  assert.equal(cmd('return', { shift: true }, list(permission)), null);
  assert.equal(cmd('return', { ctrl: true, shift: true }, list(permission)), null);
  assert.equal(cmd('delete', { alt: true }, list(permission)), null);
});

test('Ctrl+Alt+Enter always allows, Ctrl+Backspace denies or keeps planning', () => {
  const permission = actionsOf.permission(ALWAYS_INLINE, false);
  assert.deepEqual(cmd('return', CTRL_ALT, list(permission)), { cmd: 'alwaysAllow', sessionId: 's', toolUseId: 't' });
  assert.deepEqual(cmd('delete', CTRL, list(permission)), { cmd: 'deny', sessionId: 's', toolUseId: 't' });
  assert.deepEqual(cmd('delete', CTRL, list(actionsOf.plan('p'))), { cmd: 'keepPlanning', sessionId: 's', toolUseId: 'p' });
  assert.equal(cmd('delete', CTRL, list(actionsOf.none)), null);
  assert.equal(cmd('delete', CTRL, list(actionsOf.chips('q'))), null);
  // A broad rule is offered only where its description is shown: the chat.
  const broad = actionsOf.permission(ALWAYS_CHAT, false);
  assert.equal(cmd('return', CTRL_ALT, list(broad)), null);
  assert.deepEqual(cmd('return', CTRL_ALT, chat(broad, false)), { cmd: 'alwaysAllow', sessionId: 's', toolUseId: 't' });
  const noRule = actionsOf.permission(null, false);
  assert.equal(cmd('return', CTRL_ALT, chat(noRule, false)), null);
  // Not from the list when the request is too long to be read there whole.
  assert.equal(cmd('return', CTRL_ALT, list(actionsOf.permission(ALWAYS_INLINE, true))), null);
  assert.equal(cmd('return', CTRL_ALT, list(actionsOf.plan('p'))), null);
});

test('digits pick an answer chip', () => {
  const chips = actionsOf.chips('q');
  assert.deepEqual(cmd('1', {}, list(chips)), { cmd: 'chooseOption', sessionId: 's', toolUseId: 'q', index: 0 });
  assert.deepEqual(cmd('2', {}, list(chips)), { cmd: 'chooseOption', sessionId: 's', toolUseId: 'q', index: 1 });
  assert.equal(cmd('3', {}, list(chips)), null);
  assert.equal(cmd('0', {}, list(chips)), null);
  assert.equal(cmd('5', {}, list(chips)), null);
  assert.equal(cmd('1', {}, list(actionsOf.none)), null);
  assert.equal(cmd('1', CTRL, list(chips)), null);
  assert.equal(cmd('1', { alt: true }, list(chips)), null);
  assert.equal(cmd('1', {}, chat(chips, false)), null, 'the chat has its own buttons');
});

test('jump and review shortcuts', () => {
  assert.deepEqual(cmd('j', CTRL, list(actionsOf.none)), { cmd: 'jump', sessionId: 's' });
  assert.equal(cmd('j', CTRL, list(actionsOf.none, { jump: false })), null);
  assert.equal(cmd('j', CTRL, NO_SELECTION), null);
  assert.deepEqual(cmd('r', CTRL, list(actionsOf.none, { reviewable: true })), { cmd: 'markReviewed', sessionId: 's' });
  assert.equal(cmd('r', CTRL, list(actionsOf.none)), null);
  assert.deepEqual(cmd('r', CTRL, list(actionsOf.none, { dismiss: true })), { cmd: 'dismissFailure', sessionId: 's' });
  assert.deepEqual(cmd('r', CTRL_SHIFT, NO_SELECTION), { cmd: 'markAllReviewed' });
  assert.deepEqual(cmd('R', CTRL_SHIFT, NO_SELECTION), { cmd: 'markAllReviewed' });
  assert.deepEqual(cmd('R', CTRL, list(actionsOf.none, { reviewable: true })), { cmd: 'markReviewed', sessionId: 's' });
  assert.equal(cmd('r', { ctrl: true, alt: true }, list(actionsOf.none, { reviewable: true })), null);
});

test('Escape goes back from a chat, and closes anywhere else', () => {
  assert.deepEqual(cmd('escape', {}, chat(actionsOf.none, true)), { cmd: 'back' });
  assert.deepEqual(cmd('escape', {}, NO_SELECTION), { cmd: 'close' });
  assert.deepEqual(cmd('escape', {}, { kind: 'setup' }), { cmd: 'close' });
});

test('the composer owns plain keys but not the Ctrl ones', () => {
  const permission = actionsOf.permission(ALWAYS_INLINE, false);
  const typing = chat(permission, true);
  assert.equal(cmd('return', {}, typing), null);
  assert.equal(cmd('1', {}, typing), null);
  assert.equal(cmd('up', {}, typing), null);
  assert.deepEqual(cmd('return', CTRL, typing), { cmd: 'allow', sessionId: 's', toolUseId: 't' });
  // Arrows don't move a list that isn't shown.
  assert.equal(cmd('down', {}, chat(actionsOf.none, false)), null);
  // Ctrl and Alt together are still the always-allow rule; Ctrl+Shift+Enter is nothing.
  assert.deepEqual(cmd('return', CTRL_ALT, list(actionsOf.plan('p'))), null);
  assert.equal(cmd('return', CTRL_SHIFT, list(actionsOf.plan('p'))), null);
});

test('setup takes no session keys', () => {
  assert.equal(cmd('down', {}, { kind: 'setup' }), null);
  assert.equal(cmd('return', CTRL, { kind: 'setup' }), null);
  assert.equal(cmd('r', CTRL_SHIFT, { kind: 'setup' }), null);
});

test('moving stops at the ends', () => {
  const order = ['a', 'b', 'c'];
  assert.equal(C.moveSelection(null, 1, order), 'a');
  assert.equal(C.moveSelection(null, -1, order), 'c');
  assert.equal(C.moveSelection('a', 1, order), 'b');
  assert.equal(C.moveSelection('c', 1, order), 'c');
  assert.equal(C.moveSelection('a', -1, order), 'a');
  assert.equal(C.moveSelection('gone', 1, order), 'a');
  assert.equal(C.moveSelection('a', 1, []), null);
  assert.equal(C.moveSelection('a', 1, undefined), null);
});

test('primaryActions reads the snapshot\'s rows the way the Mac reads its own', () => {
  const rows = harness.fixture('snapshot.json').sessions;
  const byId = (id) => rows.find((r) => r.session_id === id);
  const permission = C.primaryActions(byId('needs-permission'));
  assert.equal(permission.kind, 'permission');
  assert.equal(permission.toolUseId, 'toolu_sample_bash');
  assert.equal(permission.hasAlways, true);
  assert.equal(permission.alwaysInline, true);
  assert.equal(permission.needsReview, false);
  const question = C.primaryActions(byId('needs-question'));
  assert.equal(question.kind, 'question_chips');
  assert.equal(question.question.options.length, 3);
  assert.equal(C.primaryActions(byId('needs-plan')).kind, 'plan');
  // Several questions, a multi-select question, or more than four options: answered in the chat.
  const many = JSON.parse(JSON.stringify(byId('needs-question')));
  many.pending.questions.push(many.pending.questions[0]);
  assert.equal(C.primaryActions(many).kind, 'answer_in_chat');
  const multi = JSON.parse(JSON.stringify(byId('needs-question')));
  multi.pending.questions[0].multi_select = true;
  assert.equal(C.primaryActions(multi).kind, 'answer_in_chat');
  const wide = JSON.parse(JSON.stringify(byId('needs-question')));
  wide.pending.questions[0].options = [1, 2, 3, 4, 5].map((n) => ({ label: `o${n}`, description: null }));
  assert.equal(C.primaryActions(wide).kind, 'answer_in_chat');
  const notSingle = JSON.parse(JSON.stringify(byId('needs-question')));
  notSingle.pending.single_tap = false;
  assert.equal(C.primaryActions(notSingle).kind, 'answer_in_chat');
  // A row that needs you with nothing to answer from here (a terminal dialog) says so.
  const dialog = rows.find((r) => r.detail && r.detail.kind === 'dialog');
  assert.equal(C.primaryActions(dialog).kind, 'answer_in_terminal');
  assert.equal(C.primaryActions(byId('needs-ratelimit')).kind, 'none');
  for (const row of rows.filter((r) => !r.pending && !(r.detail && r.detail.kind === 'dialog'))) {
    assert.equal(C.primaryActions(row).kind, 'none', row.session_id);
  }
  assert.deepEqual(plain(C.primaryActions(null)), { kind: 'none' });
});

test('keyTarget carries what the row can do', () => {
  const rows = harness.fixture('snapshot.json').sessions;
  for (const row of rows) {
    const t = C.keyTarget(row);
    assert.equal(t.sessionId, row.session_id);
    assert.equal(t.canMarkReviewed, row.bucket === 'ready_for_review');
    assert.equal(t.canDismissFailure, !!row.failed);
    assert.equal(t.canJump, !!row.focus_label);
  }
  const review = rows.find((r) => r.bucket === 'ready_for_review');
  assert.deepEqual(cmd('r', CTRL, { kind: 'list', target: C.keyTarget(review) }), { cmd: 'markReviewed', sessionId: review.session_id });
  const failed = rows.find((r) => r.failed);
  assert.deepEqual(cmd('r', CTRL, { kind: 'list', target: C.keyTarget(failed) }), { cmd: 'dismissFailure', sessionId: failed.session_id });
});

test('routerKey maps DOM keys to the router\'s and drops the rest', () => {
  const key = (k) => C.routerKey({ key: k });
  assert.equal(key('ArrowUp'), 'up');
  assert.equal(key('ArrowDown'), 'down');
  assert.equal(key('Enter'), 'return');
  assert.equal(key('Backspace'), 'delete');
  assert.equal(key('Delete'), 'delete');
  assert.equal(key('Escape'), 'escape');
  assert.equal(key('a'), 'a');
  assert.equal(key('R'), 'R');
  assert.equal(key('1'), '1');
  for (const k of ['Shift', 'Control', 'Alt', 'Tab', 'F5', 'ArrowLeft', 'Dead', 'Unidentified', undefined, '']) {
    assert.equal(key(k), null, String(k));
  }
});

// ---- the browser's own shortcuts ---------------------------------------------------------------

test('the accelerators the panel takes back are exactly F5, Ctrl+R, Ctrl+Shift+R, Ctrl+J, Ctrl+P and Ctrl+F', () => {
  const yes = [
    { key: 'F5' }, { key: 'F5', ctrlKey: true },
    { key: 'r', ctrlKey: true }, { key: 'R', ctrlKey: true }, { key: 'R', ctrlKey: true, shiftKey: true },
    { key: 'j', ctrlKey: true }, { key: 'p', ctrlKey: true }, { key: 'f', ctrlKey: true },
  ];
  for (const e of yes) assert.equal(C.isBrowserAccelerator(e), true, JSON.stringify(e));
  const no = [
    { key: 'r' }, { key: 'j' }, { key: 'p' }, { key: 'f' }, { key: 'F' },
    { key: 'r', altKey: true }, { key: 'j', metaKey: true },
    { key: 'a', ctrlKey: true }, { key: 'c', ctrlKey: true }, { key: 'v', ctrlKey: true }, { key: 'x', ctrlKey: true }, { key: 'z', ctrlKey: true },
    { key: 'Enter', ctrlKey: true }, { key: 'Backspace', ctrlKey: true }, { key: 'Escape' }, { key: 'F12' }, { key: 'Tab' },
    { key: 'ArrowDown', ctrlKey: true }, { key: undefined, ctrlKey: true }, { key: 'w', ctrlKey: true },
  ];
  for (const e of no) assert.equal(C.isBrowserAccelerator(e), false, JSON.stringify(e));
});

test('every Ctrl key the router uses that the browser also takes is on the accelerator list', () => {
  // The page prevents the default of an accelerator; a router command whose key the browser
  // handles first (reload, downloads) would otherwise reload the panel under the user.
  const target = { sessionId: 's', actions: { kind: 'none' }, canMarkReviewed: true, canDismissFailure: false, canJump: true };
  for (const key of ['j', 'r']) {
    assert.notEqual(cmd(key, CTRL, { kind: 'list', target }), null, key);
    assert.equal(C.isBrowserAccelerator({ key, ctrlKey: true }), true, key);
  }
  assert.notEqual(cmd('r', CTRL_SHIFT, NO_SELECTION), null);
  assert.equal(C.isBrowserAccelerator({ key: 'r', ctrlKey: true, shiftKey: true }), true);
});

// ---- routes -------------------------------------------------------------------------------------

test('parseRoute reads "sessions" and "session:<id>"', () => {
  assert.deepEqual(plain(C.parseRoute('sessions')), { kind: 'sessions', id: null });
  assert.deepEqual(plain(C.parseRoute('session:needs-permission')), { kind: 'session', id: 'needs-permission' });
  assert.deepEqual(plain(C.parseRoute('session:a:b')), { kind: 'session', id: 'a:b' });
  assert.deepEqual(plain(C.parseRoute('session:')), { kind: 'session', id: '' });
  assert.deepEqual(plain(C.parseRoute(undefined)), { kind: 'sessions', id: null });
  assert.deepEqual(plain(C.parseRoute('')), { kind: 'sessions', id: null });
  assert.deepEqual(plain(C.parseRoute('sessionsX')), { kind: 'sessions', id: null });
  assert.deepEqual(plain(C.parseRoute('settings')), { kind: 'sessions', id: null });
});

// ---- the morph ----------------------------------------------------------------------------------

function host(f, html) {
  const el = f.document.createElement('div');
  f.document.body.appendChild(el);
  f.C.morph(el, html);
  return el;
}

test('morph builds, then touches only what differs', () => {
  const f = fresh();
  const el = host(f, '<p class="a">one</p><p class="b">two</p>');
  const [p1, p2] = el.children;
  f.C.morph(el, '<p class="a">one!</p><p class="b c">two</p>');
  assert.equal(el.children[0], p1);
  assert.equal(el.children[1], p2);
  assert.equal(p1.textContent, 'one!');
  assert.equal(p2.getAttribute('class'), 'b c');
  f.C.morph(el, '<p class="a">one!</p>');
  assert.equal(el.children.length, 1);
  assert.equal(el.children[0], p1);
  f.C.morph(el, '');
  assert.equal(el.childNodes.length, 0);
  f.C.morph(el, 'just text');
  assert.equal(el.textContent, 'just text');
});

test('morph keeps keyed elements, and their spin, when the list is reordered', () => {
  const f = fresh();
  const el = host(f, '<div data-key="a">A</div><div data-key="b">B</div><div data-key="c">C</div>');
  const [a, b, c] = el.children;
  f.C.morph(el, '<div data-key="c">C</div><div data-key="a">A2</div><div data-key="d">D</div>');
  assert.deepEqual(el.children.map((e) => e.textContent), ['C', 'A2', 'D']);
  assert.equal(el.children[0], c);
  assert.equal(el.children[1], a);
  assert.equal(b.parentNode, null, 'b is gone');
  assert.equal(el.children[2].getAttribute('data-key'), 'd');
});

test('a key that changes replaces the element, so a finite animation runs again', () => {
  const f = fresh();
  const el = host(f, f.C.statusRing('needs', { breathKey: 'x1' }));
  const first = el.firstChild;
  f.C.morph(el, f.C.statusRing('needs', { breathKey: 'x1' }));
  assert.equal(el.firstChild, first, 'the same key leaves it alone');
  f.C.morph(el, f.C.statusRing('needs', { breathKey: 'x2' }));
  assert.notEqual(el.firstChild, first);
  assert.equal(el.firstChild.getAttribute('data-key'), 'breath-x2');
});

test('a working spinner keeps its identity through renders that do not change it', () => {
  const f = fresh();
  const html = `<div class="row">${f.C.statusRing('working')}<span>Fix</span></div>`;
  const el = host(f, html);
  const spinner = el.querySelector('svg');
  for (let i = 0; i < 3; i++) f.C.morph(el, html.replace('Fix', `Fix ${i}`));
  assert.equal(el.querySelector('svg'), spinner);
  assert.equal(el.querySelector('span').textContent, 'Fix 2');
});

test('a focused field keeps focus, caret and the text typed into it', () => {
  const f = fresh();
  const el = host(f, '<div class="bar"><input id="in" data-an-keep value="" placeholder="Reply"><textarea id="ta" data-an-keep></textarea></div>');
  const input = el.querySelector('#in');
  const area = el.querySelector('#ta');
  input.focus();
  input.value = 'half a reply';
  input.setSelectionRange(4, 4);
  area.value = 'a draft';
  f.C.morph(el, '<div class="bar wide"><input id="in" data-an-keep value="" placeholder="Reply…" readonly><textarea id="ta" data-an-keep></textarea></div>');
  assert.equal(el.querySelector('#in'), input);
  assert.equal(f.document.activeElement, input);
  assert.equal(input.value, 'half a reply');
  assert.equal(input.selectionStart, 4);
  assert.equal(input.getAttribute('placeholder'), 'Reply…');
  assert.equal(input.hasAttribute('readonly'), true, 'attributes follow the render');
  assert.equal(area.value, 'a draft');
  assert.equal(el.firstChild.getAttribute('class'), 'bar wide');
  f.C.morph(el, '<div class="bar wide"><input id="in" data-an-keep value="" placeholder="Reply…"><textarea id="ta" data-an-keep></textarea></div>');
  assert.equal(input.hasAttribute('readonly'), false);
});

test('a field without data-an-keep follows the render unless it has focus', () => {
  const f = fresh();
  const el = host(f, '<input id="a" value="one"><input id="b" value="two"><textarea id="t">seed</textarea>');
  const [a, b, area] = el.children;
  b.focus();
  a.value = 'typed a';
  b.value = 'typed b';
  area.value = 'typed t';
  f.C.morph(el, '<input id="a" value="new a"><input id="b" value="new b"><textarea id="t">new t</textarea>');
  assert.equal(a.value, 'new a');
  assert.equal(b.value, 'typed b', 'the focused field is the user\'s');
  assert.equal(area.value, 'new t');
  assert.equal(f.document.activeElement, b);
});

test('checkboxes follow the render, and other elements\' state is left alone', () => {
  const f = fresh();
  const el = host(f, '<input type="checkbox" id="c" checked><input type="checkbox" id="d">');
  const [c, d] = el.children;
  assert.equal(c.checked, true);
  f.C.morph(el, '<input type="checkbox" id="c"><input type="checkbox" id="d" checked>');
  assert.equal(c.checked, false);
  assert.equal(d.checked, true);
});

test('morph never runs or keeps what an unescaped hostile string would have made of a render', () => {
  const f = fresh();
  const el = host(f, '<p data-key="k">safe</p>');
  for (const raw of audit.HOSTILE) {
    f.C.morph(el, `<p data-key="k">${f.C.esc(raw)}</p>`);
    assert.deepEqual(audit.problems(el), [], raw.slice(0, 40));
    assert.equal(el.querySelector('p').textContent, raw);
  }
  // …and the unescaped form is caught by the audit, so the checks above can fail.
  f.C.morph(el, `<p data-key="k">${audit.HOSTILE[0]}</p>`);
  assert.notEqual(audit.problems(el).length, 0);
});

test('morph replaces an element of another tag with the same key', () => {
  const f = fresh();
  const el = host(f, '<div data-key="k">a</div>');
  const first = el.firstChild;
  f.C.morph(el, '<button data-key="k">a</button>');
  assert.equal(el.firstChild.localName, 'button');
  assert.notEqual(el.firstChild, first);
});

// ---- motion, loading ----------------------------------------------------------------------------

test('the static mode and reduced motion both still the page', () => {
  const f = fresh();
  assert.equal(f.C.isStatic(), false);
  assert.equal(f.C.stillMotion(), false);
  f.C.setStatic(true);
  assert.equal(f.C.isStatic(), true);
  assert.equal(f.document.documentElement.classList.contains('an-static'), true);
  assert.equal(f.C.stillMotion(), true);
  f.C.setStatic(false);
  assert.equal(f.C.stillMotion(), false);
  f.media.reduce = true;
  assert.equal(f.C.stillMotion(), true);
});

test('flip: a moved row glides from where it was, unless motion is still', () => {
  const f = fresh();
  const el = host(f, '<div data-flip="a" id="a"></div><div data-flip="b" id="b"></div>');
  const animations = [];
  for (const id of ['a', 'b']) {
    const row = el.querySelector(`#${id}`);
    row.animate = (frames, options) => animations.push({ id, frames, options });
  }
  el.querySelector('#a').__rect = { left: 0, top: 0, width: 10, height: 10 };
  el.querySelector('#b').__rect = { left: 0, top: 40, width: 10, height: 10 };
  const before = f.C.flipFirst(el);
  assert.deepEqual(plain(before), { a: 0, b: 40 });
  el.querySelector('#a').__rect.top = 40;
  el.querySelector('#b').__rect.top = 0;
  f.C.flipPlay(el, before);
  assert.deepEqual(animations.map((a) => a.id), ['a', 'b']);
  assert.equal(animations[0].frames[0].transform, 'translateY(-40px)');
  assert.equal(animations[1].frames[0].transform, 'translateY(40px)');
  assert.equal(animations[0].options.duration, 320);
  animations.length = 0;
  f.C.setStatic(true);
  f.C.flipPlay(el, before);
  assert.deepEqual(animations, []);
});

test('load inserts scripts in order without async and reports the first failure once', () => {
  const f = fresh();
  const inserted = [];
  f.document.__scriptHook = (el) => inserted.push(el);
  const results = [];
  f.C.load('agentnotch/', ['a.js', 'b.js', 'c.js'], (error) => results.push(error && error.message));
  assert.deepEqual(inserted.map((s) => s.getAttribute('src')), ['agentnotch/a.js', 'agentnotch/b.js', 'agentnotch/c.js']);
  assert.ok(inserted.every((s) => s.async === false));
  inserted[0].onload();
  inserted[1].onerror();
  assert.deepEqual(results, []);
  inserted[2].onload();
  assert.deepEqual(results, ["b.js didn't load"]);
  const empty = [];
  f.C.load('x/', [], (e) => empty.push(e));
  assert.deepEqual(empty, [null]);
});

test('baseOf gives the folder of a script URL', () => {
  assert.equal(C.baseOf('https://tauri.localhost/agentnotch/notch.js'), 'https://tauri.localhost/agentnotch/');
  assert.equal(C.baseOf('notch.js'), '');
  assert.equal(C.baseOf(''), '');
  assert.equal(C.baseOf(null), '');
});
