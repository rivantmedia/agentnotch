'use strict';
// The parts of the sessions panel that ACT (ui/agentnotch/panel.js): the row action bars, the
// AnswerGate every answer passes, the keyboard and its gate, the setup banners and the consent
// card. This is the code that approves a command and lets the app write settings.json, so every
// "must not act" case below ends by asserting that NOTHING was sent (`page.hub.calls` is empty),
// not only that the right thing was not.
//
// Swift tests ported: B_ReviewFixesTests (an answered request stays answered after it leaves the
// screen, the bars redraw when a request arms, scenes draw requests armed, Always never skips the
// review of a long request, a Ctrl shortcut with nothing to do leaves the key to the field, the
// hook banner agrees in number), B_ReviewPass2Tests (a terminal dialog offers its terminal once,
// rows offer their answers: the cases that are the page's; the chat, diff and folder cases are
// other suites'), Fix_SettingsAndPanelCopyTests (control off is said once and honestly). The pure
// gate and router tables are common.test.cjs's; here they are driven through real clicks and keys
// on the real page with the fake clock.

const test = require('node:test');
const assert = require('node:assert/strict');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');
const scripts = require('./lib/scripts.cjs');

const plain = scripts.plain;
const NOW = harness.NOW;
const CALLS = harness.fixture('calls.json');

// ---- helpers ---------------------------------------------------------------------------------

function snapshotWith(edit) {
  const snapshot = harness.fixture('snapshot.json');
  if (edit) edit(snapshot);
  return snapshot;
}

/** The panel on the fixture (changed by `edit`), loaded, with the load's own calls forgotten. */
async function open(edit, options) {
  const page = harness.loadPage('agentnotch/panel.html', Object.assign({
    snapshot: snapshotWith(edit),
    before: (window) => { window.__AGENTNOTCH_PANEL__ = { route: 'sessions' }; },
  }, options));
  await page.settle();
  page.hub.clear();
  return page;
}

function clean(page) {
  assert.deepEqual(page.errors.map(String), []);
  assert.deepEqual(page.hub.violations, []);
}

/** Nothing at all left the page. */
function silent(page, why) {
  assert.deepEqual(plain(page.hub.calls), [], why);
}

const text = (el) => (el ? el.textContent.replace(/\s+/g, ' ').trim() : null);
const row = (page, id) => page.$$('#an-rows .an-row').find((r) => r.getAttribute('data-id') === id);
const bar = (page, id) => row(page, id).querySelector('.an-row-actions');
const buttons = (page, id) => bar(page, id).querySelectorAll('button').map(text);
const button = (page, id, arg) => bar(page, id).querySelectorAll('[data-an-action="answer"]').find((b) => b.getAttribute('data-an-arg') === arg);
const named = (page, id, label) => bar(page, id).querySelectorAll('button').find((b) => text(b) === label);
const answers = (page) => page.hub.of('answer').map((c) => plain(c.args));
const state = (page) => plain(page.run('agentnotchPanel._.state'));
const session = (s, id) => s.sessions.find((x) => x.session_id === id);
const example = (n) => CALLS.filter((e) => e.call.method === 'answer')[n].call.args;

function select(page, id) {
  page.run(`agentnotchPanel._.state.selected = ${JSON.stringify(id)}; agentnotchPanel._.render();`);
}

/** The glue's confirmation that the panel is the foreground window: the keyboard gate opens. */
function focus(page, on) {
  page.emit('an:panel_focus', { focused: on !== false });
}

/** The page once its bars are answerable and the keyboard is its own. */
async function ready(edit, options) {
  const page = await open(edit, options);
  page.tick(350);
  focus(page);
  return page;
}

const ENTER = { key: 'Enter' };
const CTRL_ENTER = { key: 'Enter', ctrlKey: true };
const CTRL_ALT_ENTER = { key: 'Enter', ctrlKey: true, altKey: true };
const CTRL_BACKSPACE = { key: 'Backspace', ctrlKey: true };

// ---- the action bar per kind of request ------------------------------------------------------

test('a permission row offers Deny, Always (a narrow rule, said out loud) and Allow', async () => {
  const page = await open();
  assert.deepEqual(buttons(page, 'needs-permission'), ['Deny', 'Always', 'Allow']);
  assert.equal(text(bar(page, 'needs-permission').querySelector('.an-act-cap')),
    "Always: Don't ask again for Bash(npm run test:*) in this project (just you)");
  assert.equal(button(page, 'needs-permission', 'deny').getAttribute('title'), 'Deny (Ctrl+Backspace)');
  assert.equal(button(page, 'needs-permission', 'allow').getAttribute('title'), 'Allow (Ctrl+Enter)');
  assert.equal(button(page, 'needs-permission', 'always').getAttribute('title'),
    "Don't ask again for Bash(npm run test:*) in this project (just you) (Ctrl+Alt+Enter)");
  // the lasting rule is never the eye-catching button
  assert.ok(button(page, 'needs-permission', 'always').classList.contains('an-btn-secondary'));
  assert.ok(button(page, 'needs-permission', 'allow').classList.contains('an-btn-primary'));
  // every answering button carries the request it was drawn for
  for (const b of bar(page, 'needs-permission').querySelectorAll('[data-an-action="answer"]')) {
    assert.equal(b.getAttribute('data-an-session'), 'needs-permission');
    assert.equal(b.getAttribute('data-an-tool'), 'toolu_sample_bash');
  }
  clean(page);
  silent(page, 'drawing a bar sends nothing');
});

test('Always shows only for a rule the row may offer: none without one, none when it is not inline', async () => {
  const none = await open((s) => { session(s, 'needs-permission').pending.always = null; });
  assert.deepEqual(buttons(none, 'needs-permission'), ['Deny', 'Allow']);
  assert.equal(bar(none, 'needs-permission').querySelector('.an-act-cap'), null);
  const wide = await open((s) => { session(s, 'needs-permission').pending.inline_always = false; });
  assert.deepEqual(buttons(wide, 'needs-permission'), ['Deny', 'Allow'], 'a wide rule is offered in the chat only');
  assert.equal(bar(wide, 'needs-permission').querySelector('.an-act-cap'), null);
});

test('a request too long to judge from the row is reviewed first: Deny and Review…, never Allow or Always', async () => {
  const page = await ready((s) => { session(s, 'needs-permission').pending.needs_review = true; });
  assert.deepEqual(buttons(page, 'needs-permission'), ['Deny', 'Review…']);
  assert.equal(text(bar(page, 'needs-permission').querySelector('.an-act-cap')), 'Too long to judge from here: review it whole first.');
  assert.equal(button(page, 'needs-permission', 'allow'), undefined);
  assert.equal(button(page, 'needs-permission', 'always'), undefined);
  page.click(named(page, 'needs-permission', 'Review…'));
  assert.equal(state(page).route, 'session:needs-permission', 'Review… opens the chat');
  assert.deepEqual(answers(page), []);
  clean(page);
});

test('a single question with up to four options is answered with numbered chips, then Other…', async () => {
  const page = await open();
  // (the number and the label are two elements; the markup has no space between them)
  assert.deepEqual(buttons(page, 'needs-question'), ['1Recharts', '2Chart.js', '3ECharts', 'Other…']);
  const chips = bar(page, 'needs-question').querySelectorAll('[data-an-action="answer"]');
  assert.deepEqual(chips.map((c) => c.getAttribute('data-an-arg')), ['option:0', 'option:1', 'option:2']);
  assert.deepEqual(chips.map((c) => text(c.querySelector('.an-chipnum'))), ['1', '2', '3']);
  assert.deepEqual(chips.map((c) => c.getAttribute('aria-label')), ['Recharts', 'Chart.js', 'ECharts']);
  assert.deepEqual(chips.map((c) => c.getAttribute('title')), ['Composable React components (1)', 'Canvas, small bundle (2)', 'Feature-rich, larger bundle (3)']);
  assert.ok(chips.every((c) => c.classList.contains('an-btn-tinted')));
  const other = named(page, 'needs-question', 'Other…');
  assert.equal(other.getAttribute('data-an-action'), 'open-chat', 'Other… is typed in the chat');
  const bare = await open((s) => { session(s, 'needs-question').pending.questions[0].options[0].description = null; });
  assert.equal(button(bare, 'needs-question', 'option:0').getAttribute('title'), 'Answer Recharts (1)');
});

test('anything richer than one single-select question is answered in the chat: Answer…', async () => {
  const several = (s) => {
    const p = session(s, 'needs-question').pending;
    p.single_tap = false;
    p.questions.push({ text: 'Dark mode too?', header: 'Theme', multi_select: false, options: [{ label: 'Yes', description: null }] });
  };
  for (const edit of [
    several,
    (s) => { session(s, 'needs-question').pending.questions[0].multi_select = true; },
    (s) => { session(s, 'needs-question').pending.single_tap = false; },
    (s) => { const q = session(s, 'needs-question').pending.questions[0]; q.options = [1, 2, 3, 4, 5].map((n) => ({ label: 'Option ' + n, description: null })); },
  ]) {
    const page = await ready(edit);
    assert.deepEqual(buttons(page, 'needs-question'), ['Answer…']);
    assert.equal(named(page, 'needs-question', 'Answer…').getAttribute('title'), 'Answer in the chat (Ctrl+Enter)');
    page.click(named(page, 'needs-question', 'Answer…'));
    assert.equal(state(page).route, 'session:needs-question');
    assert.deepEqual(answers(page), []);
  }
  // exactly four options still fit the chips and the keys 1-4
  const four = await open((s) => { session(s, 'needs-question').pending.questions[0].options.push({ label: 'uPlot', description: null }); });
  assert.equal(bar(four, 'needs-question').querySelectorAll('[data-an-action="answer"]').length, 4);
});

test('a plan offers Review plan (the chat) and Approve', async () => {
  const page = await ready();
  assert.deepEqual(buttons(page, 'needs-plan'), ['Review plan', 'Approve']);
  assert.equal(named(page, 'needs-plan', 'Review plan').getAttribute('title'), 'Read the whole plan (Enter)');
  assert.equal(button(page, 'needs-plan', 'approve').getAttribute('title'), 'Approve the plan and let Claude start (Ctrl+Enter)');
  page.click(named(page, 'needs-plan', 'Review plan'));
  assert.equal(state(page).route, 'session:needs-plan');
  assert.deepEqual(answers(page), []);
});

test('a dialog only the terminal can answer offers its terminal, once, and nothing to answer with', async () => {
  const page = await ready();
  assert.deepEqual(buttons(page, 'needs-elicitation'), ['Show terminal']);
  assert.equal(text(bar(page, 'needs-elicitation').querySelector('.an-act-note')), 'Answer in the terminal');
  assert.equal(bar(page, 'needs-elicitation').querySelectorAll('[data-an-action="answer"]').length, 0);
  page.click(named(page, 'needs-elicitation', 'Show terminal'));
  assert.deepEqual(plain(page.hub.calls), [{ method: 'focus', args: { session_id: 'needs-elicitation' } }]);
  // a permission seen only in the terminal is the same; the label is the engine's
  const waiting = await open((s) => {
    const r = session(s, 'needs-permission');
    r.pending = null;
    r.detail.waiting_in_terminal = true;
    r.focus_label = 'Show in editor';
  });
  assert.deepEqual(buttons(waiting, 'needs-permission'), ['Show in editor']);
  // nothing to bring to the front: the line alone
  const unfocusable = await open((s) => { session(s, 'needs-elicitation').focus_label = null; });
  assert.deepEqual(buttons(unfocusable, 'needs-elicitation'), []);
  assert.equal(text(bar(unfocusable, 'needs-elicitation')), 'Answer in the terminal');
});

test('rows with nothing to answer have no bar: a failed turn, a finished one, a working one', async () => {
  const page = await open((s) => { s.sessions = s.sessions.filter((x) => ['needs-ratelimit', 'review-darkmode', 'work-ci'].includes(x.session_id)); });
  for (const id of ['needs-ratelimit', 'review-darkmode', 'work-ci']) assert.equal(bar(page, id).childNodes.length, 0, id);
});

// ---- the AnswerGate, through real clicks on the fake clock --------------------------------------

test('a bar is inert for 350 ms after it appears: 349 ms nothing, 350 ms one call, a second click nothing', async () => {
  const page = await open();
  const allow = () => button(page, 'needs-permission', 'allow');
  assert.equal(allow().getAttribute('aria-disabled'), 'true', 'drawn disarmed');
  assert.ok(allow().classList.contains('an-unarmed'));
  page.click(allow());
  page.tick(349);
  page.click(allow());
  // and the gate itself refuses, whatever the markup says
  allow().removeAttribute('aria-disabled');
  page.click(allow());
  silent(page, 'too new to answer');
  page.tick(1);
  assert.equal(allow().hasAttribute('aria-disabled'), false, 'redrawn armed by the page\'s own timer');
  assert.equal(allow().classList.contains('an-unarmed'), false);
  page.click(allow());
  assert.deepEqual(plain(page.hub.calls), [{ method: 'answer', args: example(0) }]);
  assert.deepEqual(answers(page)[0], { session_id: 'needs-permission', tool_use_id: 'toolu_sample_bash', answer: { allow: { always: false } } });
  // once: a double click, another button of the same request, a key
  assert.ok(allow().classList.contains('an-answered'));
  page.click(allow());
  allow().removeAttribute('aria-disabled');
  page.click(allow());
  page.click(button(page, 'needs-permission', 'deny'));
  button(page, 'needs-permission', 'always').removeAttribute('aria-disabled');
  page.click(button(page, 'needs-permission', 'always'));
  focus(page);
  select(page, 'needs-permission');
  page.key(CTRL_ENTER);
  page.key(CTRL_BACKSPACE);
  await page.settle();
  assert.equal(answers(page).length, 1, 'one answer per request');
  clean(page);
});

test('a request that takes the place of an answered one waits its own 350 ms', async () => {
  const page = await open();
  page.tick(350);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.equal(answers(page).length, 1);
  // Claude Code's next queued request lands in the same row, same buttons, same spot
  const element = button(page, 'needs-permission', 'allow');
  page.emit('an:snapshot', snapshotWith((s) => {
    s.generated_at_ms += 1;
    session(s, 'needs-permission').pending.tool_use_id = 'toolu_next';
    session(s, 'needs-permission').pending.request = 'rm -rf build';
  }));
  const next = button(page, 'needs-permission', 'allow');
  assert.equal(next.getAttribute('data-an-tool'), 'toolu_next');
  assert.equal(next.getAttribute('aria-disabled'), 'true');
  page.click(next);
  page.click(element);
  page.tick(349);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.equal(answers(page).length, 1, 'the click meant for the first never lands on the second');
  page.tick(1);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.deepEqual(answers(page)[1], { session_id: 'needs-permission', tool_use_id: 'toolu_next', answer: { allow: { always: false } } });
  clean(page);
});

test('a snapshot that still shows the same request neither restarts its wait nor re-arms an answered one', async () => {
  const page = await open();
  page.tick(200);
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; }));
  page.tick(150);
  page.click(button(page, 'needs-plan', 'approve'));
  assert.equal(answers(page).length, 1, '350 ms from when it first showed');
  for (let i = 2; i < 6; i++) {
    page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += i; }));
    page.tick(400);
    page.click(button(page, 'needs-plan', 'approve'));
  }
  assert.equal(answers(page).length, 1);
});

test('an answered request stays answered for 600 s wherever it shows up again (B_ReviewFixesTests)', async () => {
  const page = await open();
  page.tick(350);
  page.click(button(page, 'needs-permission', 'deny'));
  assert.deepEqual(answers(page), [{ session_id: 'needs-permission', tool_use_id: 'toolu_sample_bash', answer: { deny: { reason: null } } }]);
  // the chat opens on another session, then the list comes back while the engine still shows it
  page.run("agentnotchPanel.navigate('session:needs-plan')");
  page.tick(200);
  page.run("agentnotchPanel.navigate('sessions')");
  page.tick(1000);
  assert.equal(button(page, 'needs-permission', 'allow').getAttribute('aria-disabled'), 'true');
  page.click(button(page, 'needs-permission', 'allow'));
  page.tick(590000);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.equal(answers(page).length, 1, 'still remembered just under 600 s');
  // the requests that were not answered had to wait again after the chat, and then arm
  page.click(button(page, 'needs-plan', 'approve'));
  assert.equal(answers(page).length, 2);
  // long after, the memory is let go (at the next redraw, one every 15 s): a request still
  // pending then can be answered again
  page.tick(30000);
  assert.equal(button(page, 'needs-permission', 'allow').hasAttribute('aria-disabled'), false);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.equal(answers(page).length, 3);
  clean(page);
});

test('leaving the list disarms its bars: back from a chat they wait again', async () => {
  const page = await open();
  page.tick(350);
  page.run("agentnotchPanel.navigate('session:needs-plan')");
  page.run("agentnotchPanel.navigate('sessions')");
  page.hub.clear();
  page.click(button(page, 'needs-permission', 'allow'));
  silent(page, 'just back on screen');
  page.tick(350);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.equal(answers(page).length, 1);
});

test('a panel opened again starts every wait over, so a click aimed at what was under it answers nothing', async () => {
  const page = await open();
  page.tick(5000);
  page.emit('an:panel', { route: 'sessions', ring_id: null, highlight: null, reason: 'auto' });
  page.click(button(page, 'needs-permission', 'allow'));
  page.tick(349);
  page.click(button(page, 'needs-permission', 'allow'));
  silent(page);
  page.tick(1);
  page.click(button(page, 'needs-permission', 'allow'));
  assert.equal(answers(page).length, 1);
});

test('a folded section\'s requests are not on screen: nothing arms there', async () => {
  const page = await open((s) => {
    const r = session(s, 'work-ci');
    r.pending = JSON.parse(JSON.stringify(session(s, 'needs-plan').pending));
    r.pending.tool_use_id = 'toolu_folded';
  });
  page.click(page.$$('[data-an-action="fold"]').find((b) => b.getAttribute('data-an-arg') === 'working'));
  page.tick(1000);
  assert.equal(plain(page.run("agentnotchPanel._.perform({cmd: 'approvePlan', sessionId: 'work-ci', toolUseId: 'toolu_folded'})")), false);
  silent(page);
});

// ---- the answers' shapes (calls.json) ----------------------------------------------------------

test('each answer has exactly the shape of its calls.json example', async () => {
  const page = await ready();
  page.click(button(page, 'needs-permission', 'always'));
  page.click(button(page, 'needs-question', 'option:0'));
  page.click(button(page, 'needs-plan', 'approve'));
  assert.deepEqual(answers(page), [
    { session_id: 'needs-permission', tool_use_id: 'toolu_sample_bash', answer: { allow: { always: true } } },
    example(2),
    example(3),
  ]);
  assert.deepEqual(example(2).answer, { questions: { answers: { 'Which charting library should the dashboard use?': 'Recharts' } } });
  assert.equal(example(3).answer, 'approve_plan');
  const keep = await ready();
  select(keep, 'needs-plan');
  keep.key(CTRL_BACKSPACE);
  assert.deepEqual(answers(keep), [example(4)]);
  assert.equal(example(4).answer, 'keep_planning');
  const deny = await ready();
  deny.click(button(deny, 'needs-permission', 'deny'));
  assert.deepEqual(answers(deny)[0].answer, { deny: { reason: null } });
  clean(page);
  clean(keep);
  clean(deny);
});

test('chips 1-4 answer with the label at that place, by click and by key', async () => {
  const four = (s) => { session(s, 'needs-question').pending.questions[0].options.push({ label: 'uPlot', description: null }); };
  const labels = ['Recharts', 'Chart.js', 'ECharts', 'uPlot'];
  for (let i = 0; i < 4; i++) {
    const clicked = await ready(four);
    clicked.click(button(clicked, 'needs-question', 'option:' + i));
    const keyed = await ready(four);
    select(keyed, 'needs-question');
    keyed.key({ key: String(i + 1) });
    for (const page of [clicked, keyed]) {
      assert.deepEqual(answers(page), [{ session_id: 'needs-question', tool_use_id: 'toolu_sample_question',
        answer: { questions: { answers: { 'Which charting library should the dashboard use?': labels[i] } } } }]);
      clean(page);
    }
  }
});

test('a question form\'s answers map: one label, several joined in the options\' order, Other typed', async () => {
  const page = await open();
  const map = (questions, picks) => plain(page.run(`agentnotchPanel.answers(${JSON.stringify(questions)}, ${JSON.stringify(picks)})`));
  const single = { text: 'Which library?', multi_select: false, options: [{ label: 'A' }, { label: 'B' }, { label: 'C' }] };
  const multi = { text: 'Which screens?', multi_select: true, options: [{ label: 'Welcome' }, { label: 'Permissions' }, { label: 'Done' }] };
  assert.deepEqual(map([single], [{ labels: ['B'], other: null }]), { 'Which library?': 'B' });
  assert.deepEqual(map([single, multi], [{ labels: ['A'], other: null }, { labels: ['Done', 'Welcome'], other: null }]),
    { 'Which library?': 'A', 'Which screens?': 'Welcome, Done' });
  assert.deepEqual(map([multi], [{ labels: ['Permissions'], other: '  Settings ' }]), { 'Which screens?': 'Permissions, Settings' });
  assert.deepEqual(map([single], [{ labels: [], other: ' my own ' }]), { 'Which library?': 'my own' }, 'Other sends the typed text');
  // until every question has an answer there is no map to send
  assert.equal(map([single, multi], [{ labels: ['A'], other: null }]), null);
  assert.equal(map([single], [{ labels: [], other: '   ' }]), null);
  assert.equal(map([multi], [{ labels: [], other: null }]), null);
  assert.equal(map([], []), null);
  silent(page);
});

test('an answer is checked against what the session waits for now: a stale or forged button sends nothing', async () => {
  const page = await ready();
  const perform = (cmd) => plain(page.run(`agentnotchPanel._.perform(${JSON.stringify(cmd)})`));
  // the wrong kind of answer for the request, another session's request, a request that is gone
  assert.equal(perform({ cmd: 'approvePlan', sessionId: 'needs-permission', toolUseId: 'toolu_sample_bash' }), false);
  assert.equal(perform({ cmd: 'allow', sessionId: 'needs-plan', toolUseId: 'toolu_sample_plan' }), false);
  assert.equal(perform({ cmd: 'allow', sessionId: 'needs-question', toolUseId: 'toolu_sample_bash' }), false);
  assert.equal(perform({ cmd: 'allow', sessionId: 'needs-permission', toolUseId: 'toolu_gone' }), false);
  assert.equal(perform({ cmd: 'chooseOption', sessionId: 'needs-question', toolUseId: 'toolu_sample_question', index: 7 }), false);
  assert.equal(perform({ cmd: 'allow', sessionId: 'nobody', toolUseId: 'toolu_sample_bash' }), false);
  // a button whose arg was tampered with
  const allow = button(page, 'needs-permission', 'allow');
  allow.setAttribute('data-an-arg', 'approve');
  page.click(allow);
  allow.setAttribute('data-an-arg', 'constructor');
  page.click(allow);
  allow.setAttribute('data-an-arg', 'allow');
  allow.setAttribute('data-an-tool', 'toolu_sample_plan');
  page.click(allow);
  // the chat's entry refuses from the list
  assert.equal(plain(page.run("agentnotchPanel.answer('needs-permission', 'toolu_sample_bash', {allow: {always: true}})")), false);
  silent(page);
  clean(page);
});

// ---- a reply that says the request had moved on ------------------------------------------------

test('not_pending and peer_gone are said once and never retried', async () => {
  for (const [result, line] of [
    ['not_pending', 'That request was already answered or is no longer waiting.'],
    ['peer_gone', 'The session went away before the answer arrived.'],
  ]) {
    const page = await ready(null, { replies: { answer: { result } } });
    page.click(button(page, 'needs-permission', 'allow'));
    await page.settle();
    assert.equal(text(page.$('#an-toast .an-notice')), line);
    assert.equal(page.$('#an-toast .an-notice').getAttribute('role'), 'status');
    page.tick(5999);
    assert.ok(page.$('#an-toast .an-notice'));
    page.tick(1);
    assert.equal(page.$('#an-toast .an-notice'), null, 'the line goes by itself');
    page.click(button(page, 'needs-permission', 'allow'));
    page.tick(60000);
    await page.settle();
    assert.equal(answers(page).length, 1, 'never sent again');
    clean(page);
  }
  const delivered = await ready();
  delivered.click(button(delivered, 'needs-permission', 'allow'));
  await delivered.settle();
  assert.equal(delivered.$('#an-toast .an-notice'), null, 'a delivered answer says nothing');
});

test('an answer the engine could not take is said, not retried, and its text is never the error\'s', async () => {
  const page = await ready(null, { replies: { answer: () => { throw { code: 'failed', message: '<img src=x onerror=alert(1)>' }; } } });
  page.click(button(page, 'needs-plan', 'approve'));
  await page.settle();
  assert.equal(text(page.$('#an-toast .an-notice')), 'The answer didn’t reach Claude Code. Answer it where Claude Code runs.');
  page.tick(60000);
  await page.settle();
  assert.equal(answers(page).length, 1);
  assert.deepEqual(audit.problems(page.$('#an-toast')), []);
});

// ---- the keyboard gate --------------------------------------------------------------------------

const SHORTCUTS = [
  { key: '1' }, { key: '2' }, { key: '3' }, { key: '4' }, ENTER, CTRL_ENTER, CTRL_ALT_ENTER, CTRL_BACKSPACE,
  { key: 'ArrowDown' }, { key: 'ArrowUp' }, { key: 'j', ctrlKey: true }, { key: 'r', ctrlKey: true },
  { key: 'R', ctrlKey: true, shiftKey: true }, { key: 'z', ctrlKey: true }, { key: ' ' },
];

test('THE KEYBOARD GATE: until an:panel_focus {focused:true} no shortcut does anything, on any row', async () => {
  const page = await open();
  page.tick(1000);
  for (const id of ['needs-permission', 'needs-question', 'needs-plan', 'needs-elicitation', 'needs-ratelimit', 'review-darkmode', null]) {
    select(page, id);
    const before = state(page);
    for (const key of SHORTCUTS) page.key(key);
    const after = state(page);
    assert.equal(after.selected, before.selected, 'the selection did not move');
    assert.equal(after.route, 'sessions');
    assert.equal(after.pending, null);
  }
  await page.settle();
  silent(page, 'keys typed while the gate is shut reach nothing');
  clean(page);
});

test('the gate opens on the glue\'s confirmation only, shuts again on {focused:false}, and queues nothing', async () => {
  const page = await open();
  page.tick(1000);
  select(page, 'needs-question');
  // DOM focus in every form the page could see it: none opens the gate
  page.window.dispatchEvent(new page.window.Event('focus'));
  page.fire(page.document.body, 'focus');
  page.fire(page.$('#an-card'), 'focusin');
  button(page, 'needs-question', 'option:1').focus();
  page.fire(page.$('#an-card'), 'pointerdown');
  page.key({ key: '2' });
  page.key(CTRL_ENTER);
  silent(page, 'DOM focus alone does not open the gate');
  page.document.activeElement.blur();
  // confirmed: the keys typed before are gone, the next one acts, once
  focus(page);
  await page.settle();
  silent(page, 'nothing was queued');
  page.key({ key: '2' });
  assert.deepEqual(answers(page), [{ session_id: 'needs-question', tool_use_id: 'toolu_sample_question',
    answer: { questions: { answers: { 'Which charting library should the dashboard use?': 'Chart.js' } } } }]);
  // shut again
  page.hub.clear();
  select(page, 'needs-permission');
  focus(page, false);
  for (const key of SHORTCUTS) page.key(key);
  assert.equal(state(page).selected, 'needs-permission');
  silent(page, 'shut again');
  page.emit('an:panel_focus', {});
  page.key(CTRL_ENTER);
  silent(page, 'anything but focused:true keeps it shut');
  focus(page);
  page.key(CTRL_ENTER);
  assert.equal(answers(page).length, 1);
  clean(page);
});

test('the AnswerGate still applies once the keyboard gate is open', async () => {
  const page = await open();
  focus(page);
  select(page, 'needs-permission');
  page.key(CTRL_ENTER);
  page.key(CTRL_ALT_ENTER);
  page.key(CTRL_BACKSPACE);
  page.tick(349);
  page.key(CTRL_ENTER);
  silent(page, 'the request has not been on screen for 350 ms');
  page.tick(1);
  page.key(CTRL_ENTER);
  page.key(CTRL_ENTER);
  page.key(CTRL_BACKSPACE);
  assert.deepEqual(answers(page), [example(0)]);
});

test('a held key never answers the request that replaces the one it answered', async () => {
  const page = await ready();
  select(page, 'needs-permission');
  page.key(CTRL_ENTER);
  page.emit('an:snapshot', snapshotWith((s) => {
    s.generated_at_ms += 1;
    session(s, 'needs-permission').pending.tool_use_id = 'toolu_next';
  }));
  page.tick(2000);
  for (let i = 0; i < 5; i++) {
    const event = page.key(Object.assign({ repeat: true }, CTRL_ENTER));
    assert.equal(event.defaultPrevented, true);
  }
  assert.equal(answers(page).length, 1);
  page.key(CTRL_ENTER);
  assert.equal(answers(page).length, 2, 'a fresh press does');
});

test('the Windows key is never part of a shortcut', async () => {
  const page = await ready();
  select(page, 'needs-permission');
  page.key({ key: 'Enter', ctrlKey: true, metaKey: true });
  page.key({ key: '1', metaKey: true });
  silent(page);
});

// ---- the keys (ClaudeKeyRouter, Ctrl for Cmd and Alt for Option) ------------------------------

test('Up and Down move the selection through the rows on screen, from the first or the last, without wrapping', async () => {
  const page = await ready();
  const order = plain(page.run('agentnotchPanel._.listLayout(agentnotchPanel._.view()).visible'));
  assert.ok(order.length > 5);
  page.key({ key: 'ArrowDown' });
  assert.equal(state(page).selected, order[0], 'the first press selects the first row');
  assert.ok(row(page, order[0]).classList.contains('an-sel'));
  page.key({ key: 'ArrowDown' });
  assert.equal(state(page).selected, order[1]);
  page.key({ key: 'ArrowUp' });
  page.key({ key: 'ArrowUp' });
  page.key({ key: 'ArrowUp' });
  assert.equal(state(page).selected, order[0], 'no wrap at the top');
  const fresh = await ready();
  const up = fresh.key({ key: 'ArrowUp' });
  assert.equal(state(fresh).selected, order[order.length - 1], 'Up with nothing selected takes the last row');
  assert.equal(up.defaultPrevented, true, 'the list does not scroll under the key as well');
  fresh.key({ key: 'ArrowDown' });
  assert.equal(state(fresh).selected, order[order.length - 1], 'no wrap at the bottom');
  // a folded section's rows are skipped
  assert.ok(!order.includes('idle-notch'), 'Idle is folded in the fixture');
  silent(page);
  silent(fresh);
});

test('a selection that is folded away, filtered out or gone is dropped', async () => {
  const page = await ready();
  select(page, 'work-ci');
  page.click(page.$$('[data-an-action="fold"]').find((b) => b.getAttribute('data-an-arg') === 'working'));
  assert.equal(state(page).selected, null);
  select(page, 'needs-permission');
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; s.sessions = s.sessions.filter((x) => x.session_id !== 'needs-permission'); }));
  assert.equal(state(page).selected, null);
  page.key(CTRL_ENTER);
  silent(page, 'a key acts on a row that is on screen, never on a remembered one');
});

test('a bare Enter opens the chat and never approves anything', async () => {
  for (const id of ['needs-permission', 'needs-plan', 'needs-question']) {
    const page = await ready();
    select(page, id);
    const event = page.key(ENTER);
    assert.equal(event.defaultPrevented, true);
    assert.equal(state(page).route, 'session:' + id);
    // (the chat's own first calls, and the size it now asks for, are chat.test.cjs's)
    assert.deepEqual(plain(page.hub.calls.filter((c) => c.method === 'panel_route')), [{ method: 'panel_route', args: { route: 'session:' + id } }]);
  }
  const none = await ready();
  none.key(ENTER);
  silent(none, 'nothing selected, nothing opened');
});

test('Ctrl+Enter is the row\'s primary action', async () => {
  const cases = [
    ['needs-permission', null, [{ method: 'answer', args: example(0) }]],
    ['needs-plan', null, [{ method: 'answer', args: example(3) }]],
    ['needs-question', null, [{ method: 'panel_route', args: { route: 'session:needs-question' } }]],
    ['needs-permission', (s) => { session(s, 'needs-permission').pending.needs_review = true; }, [{ method: 'panel_route', args: { route: 'session:needs-permission' } }]],
    ['review-darkmode', null, [{ method: 'mark_reviewed', args: { session_id: 'review-darkmode', at_ms: NOW + 350 } }]],
    ['needs-elicitation', null, []],
    ['needs-ratelimit', null, []],
    ['work-ci', null, []],
  ];
  for (const [id, edit, expected] of cases) {
    const page = await ready(edit);
    select(page, id);
    page.key(CTRL_ENTER);
    assert.deepEqual(plain(page.hub.calls.filter((c) => c.method !== 'panel_report_size')), expected, id);
    clean(page);
  }
});

test('Ctrl+Alt+Enter allows always only what the row offers: never past the review of a long request (B_ReviewFixesTests)', async () => {
  const page = await ready();
  select(page, 'needs-permission');
  page.key(CTRL_ALT_ENTER);
  assert.deepEqual(answers(page), [{ session_id: 'needs-permission', tool_use_id: 'toolu_sample_bash', answer: { allow: { always: true } } }]);
  for (const edit of [
    (s) => { session(s, 'needs-permission').pending.needs_review = true; },
    (s) => { session(s, 'needs-permission').pending.inline_always = false; },
    (s) => { session(s, 'needs-permission').pending.always = null; },
  ]) {
    const long = await ready(edit);
    select(long, 'needs-permission');
    long.key(CTRL_ALT_ENTER);
    silent(long);
    // and no button or forged command gets there either
    assert.equal(plain(long.run("agentnotchPanel._.perform({cmd: 'alwaysAllow', sessionId: 'needs-permission', toolUseId: 'toolu_sample_bash'})")), false);
    silent(long);
  }
  const reviewed = await ready((s) => { session(s, 'needs-permission').pending.needs_review = true; });
  assert.equal(plain(reviewed.run("agentnotchPanel._.perform({cmd: 'allow', sessionId: 'needs-permission', toolUseId: 'toolu_sample_bash'})")), false,
    'Allow from the list is refused too while the request must be read whole');
  silent(reviewed);
  const plan = await ready();
  select(plan, 'needs-plan');
  plan.key(CTRL_ALT_ENTER);
  silent(plan, 'a plan has no Always');
});

test('Ctrl+Backspace denies a permission and keeps planning on a plan; elsewhere it is left alone', async () => {
  const page = await ready();
  select(page, 'needs-permission');
  assert.equal(page.key(CTRL_BACKSPACE).defaultPrevented, true);
  assert.deepEqual(answers(page)[0].answer, { deny: { reason: null } });
  select(page, 'needs-plan');
  page.key({ key: 'Delete', ctrlKey: true });
  assert.equal(answers(page)[1].answer, 'keep_planning');
  page.hub.clear();
  select(page, 'needs-question');
  assert.equal(page.key(CTRL_BACKSPACE).defaultPrevented, false, 'nothing to do: the key is not swallowed');
  select(page, null);
  assert.equal(page.key(CTRL_BACKSPACE).defaultPrevented, false);
  silent(page);
});

test('1-4 choose an option only on a chips row, only a digit it has, and only bare', async () => {
  const page = await ready();
  select(page, 'needs-question');
  page.key({ key: '4' });
  page.key({ key: '0' });
  page.key({ key: '5' });
  page.key({ key: '1', ctrlKey: true });
  page.key({ key: '1', altKey: true });
  page.key({ key: '1', shiftKey: true });
  for (const id of ['needs-permission', 'needs-plan', 'needs-elicitation', 'review-darkmode']) {
    select(page, id);
    for (const key of ['1', '2', '3', '4']) page.key({ key });
  }
  silent(page);
  const multi = await ready((s) => { session(s, 'needs-question').pending.questions[0].multi_select = true; });
  select(multi, 'needs-question');
  multi.key({ key: '1' });
  silent(multi, 'a question answered in the chat takes no digit');
  select(page, 'needs-question');
  page.key({ key: '3' });
  assert.equal(answers(page)[0].answer.questions.answers['Which charting library should the dashboard use?'], 'ECharts');
});

test('Ctrl+J shows the terminal, Ctrl+R marks reviewed or dismisses a failure, Ctrl+Shift+R marks all, Ctrl+Z undoes', async () => {
  const page = await ready();
  select(page, 'needs-elicitation');
  page.key({ key: 'j', ctrlKey: true });
  select(page, 'review-darkmode');
  page.key({ key: 'r', ctrlKey: true });
  select(page, 'needs-ratelimit');
  page.key({ key: 'r', ctrlKey: true });
  assert.deepEqual(plain(page.hub.calls), [
    { method: 'focus', args: { session_id: 'needs-elicitation' } },
    { method: 'mark_reviewed', args: { session_id: 'review-darkmode', at_ms: NOW + 350 } },
    { method: 'dismiss_failure', args: { session_id: 'needs-ratelimit' } },
  ]);
  page.hub.clear();
  // nothing to do for the row: nothing sent
  select(page, 'needs-permission');
  page.key({ key: 'r', ctrlKey: true });
  select(page, null);
  page.key({ key: 'j', ctrlKey: true });
  page.key({ key: 'r', ctrlKey: true });
  page.key({ key: 'z', ctrlKey: true });
  silent(page);
  const unfocusable = await ready((s) => { session(s, 'needs-elicitation').focus_label = null; });
  select(unfocusable, 'needs-elicitation');
  unfocusable.key({ key: 'j', ctrlKey: true });
  silent(unfocusable);
  // mark all, then take it back: nothing is sent
  page.key({ key: 'R', ctrlKey: true, shiftKey: true });
  assert.equal(state(page).pending.ids.length, 3);
  assert.ok(page.$('#an-toast .an-toast'));
  assert.equal(page.key({ key: 'z', ctrlKey: true }).defaultPrevented, true);
  assert.equal(state(page).pending, null);
  page.tick(6000);
  silent(page, 'undone');
  clean(page);
});

test('Esc is behind the keyboard gate too (DESIGN-WIN §5.3): nothing while it is shut, the panel closes once it is open', async () => {
  const page = await open();
  page.tick(1000);
  const shut = page.key({ key: 'Escape' });
  assert.equal(shut.defaultPrevented, false, 'not swallowed: the page did nothing with it');
  page.run("agentnotchPanel.navigate('session:needs-permission')");
  page.hub.clear();
  page.key({ key: 'Escape' });
  assert.equal(state(page).route, 'session:needs-permission', 'a chat stays a chat while the gate is shut');
  silent(page);
  focus(page, false);
  page.emit('an:panel_focus', {});
  page.key({ key: 'Escape' });
  silent(page, 'anything but focused:true keeps Esc shut out');
  focus(page);
  page.key({ key: 'Escape' });
  assert.equal(state(page).route, 'sessions');
  page.hub.clear();
  page.key({ key: 'Escape' });
  assert.deepEqual(plain(page.hub.calls), [{ method: 'panel_close', args: null }]);
  focus(page, false);
  page.hub.clear();
  page.key({ key: 'Escape' });
  silent(page, 'shut again');
  clean(page);
});

test('in a chat the list\'s bars are gone: keys answer only what the chat says it shows', async () => {
  const page = await ready();
  page.run("agentnotchPanel.navigate('session:needs-permission')");
  page.hub.clear();
  // The list's armed bar went with the list; the chat reports its own bar (chat-bars.test.cjs
  // has the bar itself), whose 350 ms start now.
  assert.equal(plain(page.run("agentnotchPanel.isArmed('toolu_sample_bash')")), false);
  page.key(CTRL_ENTER);
  page.key(CTRL_BACKSPACE);
  page.key({ key: '1' });
  silent(page, 'the chat\'s bar has not armed yet');
  page.tick(349);
  page.key(CTRL_ENTER);
  silent(page);
  page.tick(1);
  assert.equal(plain(page.run("agentnotchPanel.isArmed('toolu_sample_bash')")), true);
  page.key(CTRL_ENTER);
  assert.equal(plain(page.run("agentnotchPanel.answer('needs-permission', 'toolu_sample_bash', {deny: {reason: null}})")), false);
  assert.deepEqual(answers(page), [example(0)]);
  assert.equal(plain(page.run('agentnotchPanel.keyboardOpen()')), true);
  clean(page);
});

// ---- keys on buttons and in fields ---------------------------------------------------------------

test('Enter never clicks a button that answers; while the gate is shut no key clicks any button', async () => {
  const page = await ready();
  const allow = button(page, 'needs-permission', 'allow');
  allow.focus();
  assert.equal(page.key(ENTER).defaultPrevented, true, 'the browser\'s click-on-Enter is cancelled');
  assert.equal(page.fire(allow, 'keyup', { key: 'Enter' }).defaultPrevented, true);
  assert.equal(page.key({ key: ' ' }).defaultPrevented, false, 'Space on the focused button is the user\'s own press');
  const pin = page.$('[data-an-action="pin"]');
  pin.focus();
  assert.equal(page.key(ENTER).defaultPrevented, false, 'an ordinary button keeps Enter');
  focus(page, false);
  for (const el of [allow, pin, page.$('[data-an-action="close"]')]) {
    el.focus();
    assert.equal(page.key(ENTER).defaultPrevented, true);
    assert.equal(page.key({ key: ' ' }).defaultPrevented, true);
    assert.equal(page.fire(el, 'keyup', { key: ' ' }).defaultPrevented, true);
  }
  silent(page);
});

function field(page) {
  page.run("(function () { var t = document.createElement('textarea'); t.id = 't-field'; document.getElementById('an-toast').appendChild(t); })()");
  return page.$('#t-field');
}

test('no text field accepts keys until the gate opens; a click on one asks for the keyboard', async () => {
  const page = await open();
  page.tick(1000);
  const input = field(page);
  input.focus();
  page.hub.clear();
  assert.equal(page.key({ key: 'a' }).defaultPrevented, true);
  assert.equal(page.key({ key: '2' }).defaultPrevented, true);
  assert.equal(page.key(ENTER).defaultPrevented, true);
  assert.equal(page.fire(input, 'beforeinput', { data: 'pasted' }).defaultPrevented, true, 'a paste or an IME neither');
  silent(page);
  page.fire(input, 'pointerdown');
  assert.deepEqual(plain(page.hub.calls), [{ method: 'panel_take_focus', args: null }]);
  assert.equal(state(page).focused, false, 'the click asks; only the glue\'s answer opens the gate');
  assert.equal(page.key({ key: 'a' }).defaultPrevented, true);
  focus(page);
  page.hub.clear();
  assert.equal(page.key({ key: 'a' }).defaultPrevented, false);
  assert.equal(page.fire(input, 'beforeinput', { data: 'x' }).defaultPrevented, false);
  page.fire(input, 'pointerdown');
  silent(page, 'nothing to ask for once it has the keyboard');
  focus(page, false);
  assert.equal(page.document.activeElement === input, false, 'a field keeps no caret it cannot use');
  clean(page);
});

test('a field owns plain keys, digits and Enter; a Ctrl shortcut with nothing to do is left to the field (B_ReviewFixesTests)', async () => {
  const page = await ready();
  select(page, 'needs-question');
  const input = field(page);
  input.focus();
  page.hub.clear();
  for (const key of [{ key: '1' }, ENTER, { key: 'ArrowDown' }, { key: 'x' }]) assert.equal(page.key(key).defaultPrevented, false);
  assert.equal(state(page).selected, 'needs-question');
  // Ctrl+Backspace deletes a word there: no command for this row, so it is not swallowed
  assert.equal(page.key(CTRL_BACKSPACE).defaultPrevented, false);
  assert.equal(page.key({ key: 'z', ctrlKey: true }).defaultPrevented, false, 'the field\'s own undo');
  silent(page);
  // a Ctrl combination with a command still routes
  assert.equal(page.key({ key: 'j', ctrlKey: true }).defaultPrevented, true);
  assert.deepEqual(plain(page.hub.calls), [{ method: 'focus', args: { session_id: 'needs-question' } }]);
});

// ---- the consent card ----------------------------------------------------------------------------

// A first run's card: the sealed fixture finds nothing and installs nothing, so it shows none.
const unasked = (s) => {
  s.setup.hook_consent = null;
  s.setup.needs_hook_consent = true;
  s.setup.consent_files = [
    { path: '~\\.claude\\settings.json', account: 'me@personal.example' },
    { path: '~\\.claude-work\\settings.json', account: 'me@work.example' },
  ];
};
const consentButton = (page, action) => page.$(`#an-banners [data-an-action="${action}"]`);

test('the consent card names every file it would edit, with its account, and says nothing is written yet', async () => {
  const page = await open((s) => {
    unasked(s);
    for (let i = 0; i < 6; i++) s.setup.consent_files.push({ path: `~\\.claude-windows\\${i}a1b2c3d4e5f6\\settings.json`, account: i % 2 ? null : 'me@work.example' });
  });
  const card = page.$('#an-banners .an-consent');
  assert.equal(text(card.querySelector('.an-consent-title')), 'Turn on Claude Code control');
  assert.equal(text(card.querySelector('.an-cap')),
    'To show every session live and let you answer prompts from here, this app adds its hooks and a status-line wrapper to the settings.json of each folder Claude Code runs in. Nothing is written until you turn it on, and turning it off puts your status line back exactly.');
  const files = card.querySelectorAll('.an-cfile').map(text);
  assert.equal(files.length, 8, 'the whole list, never "and N more"');
  assert.deepEqual(files.slice(0, 3), ['~\\.claude\\settings.json (me@personal.example)', '~\\.claude-work\\settings.json (me@work.example)',
    '~\\.claude-windows\\0a1b2c3d4e5f6\\settings.json (me@work.example)']);
  assert.equal(files[3], '~\\.claude-windows\\1a1b2c3d4e5f6\\settings.json', 'no account, no brackets');
  assert.equal(card.querySelector('.an-cfiles').getAttribute('aria-label'), 'Files it edits');
  assert.deepEqual(card.querySelectorAll('button').map(text), ['Not now', 'Turn on']);
  assert.ok(consentButton(page, 'consent-on').classList.contains('an-btn-primary'), 'emphasised');
  // emphasised is not default: nothing on the page has the focus, and no button is a submit
  assert.equal(page.document.activeElement === consentButton(page, 'consent-on'), false);
  assert.equal(page.$('#an-card [autofocus]'), null);
  assert.ok(page.$$('#an-card button').every((b) => b.getAttribute('type') === 'button'));
  assert.doesNotMatch(text(card), /Vibe|Superpowered|Take over/, 'the Mac-only apps are not named');
  silent(page, 'showing the card sends nothing');
  clean(page);
});

test('Turn on sends hook_consent {grant:true} once, from its own click, and the card goes at once', async () => {
  const page = await open(unasked);
  const on = consentButton(page, 'consent-on');
  page.click(on);
  assert.deepEqual(plain(page.hub.calls), [CALLS.find((e) => e.call.method === 'hook_consent').call]);
  assert.deepEqual(plain(page.hub.calls[0].args), { grant: true });
  assert.equal(page.$('#an-banners .an-consent'), null, 'hidden before the engine says so');
  page.click(on);
  page.fire(on, 'click');
  await page.settle();
  assert.equal(page.hub.of('hook_consent').length, 1, 'a double click is one consent');
  // the engine agrees: the card stays away; if it ever asks again the card is back
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 1; }));
  assert.equal(page.$('#an-banners .an-consent'), null);
  page.emit('an:snapshot', snapshotWith((s) => { s.generated_at_ms += 2; unasked(s); }));
  assert.ok(page.$('#an-banners .an-consent'));
  clean(page);
});

test('Not now sends hook_consent {grant:false} once and hides the card', async () => {
  const page = await open(unasked);
  page.click(consentButton(page, 'consent-later'));
  page.click(consentButton(page, 'consent-later') || page.$('#an-card'));
  assert.deepEqual(plain(page.hub.calls), [{ method: 'hook_consent', args: { grant: false } }]);
  assert.equal(page.$('#an-banners .an-consent'), null);
  clean(page);
});

test('no key, no shortcut and no other click turns the hooks on', async () => {
  const page = await open(unasked);
  page.tick(1000);
  focus(page);
  const on = consentButton(page, 'consent-on');
  assert.ok(on.hasAttribute('data-an-noenter'));
  // Enter and every shortcut, with nothing focused, with a row selected, with Turn on itself focused
  const everyKey = SHORTCUTS.concat([{ key: 'Enter', shiftKey: true }, { key: 'Tab' }, { key: 'y' }, { key: 'Y' }]);
  for (const key of everyKey) page.key(key);
  on.focus();
  assert.equal(page.key(ENTER).defaultPrevented, true, 'Enter on the focused button is cancelled');
  assert.equal(page.fire(on, 'keyup', { key: 'Enter' }).defaultPrevented, true);
  for (const key of [CTRL_ENTER, CTRL_ALT_ENTER, { key: 'Enter', shiftKey: true }]) assert.equal(page.key(key).defaultPrevented, true);
  on.blur();
  // clicks elsewhere in the card, and the action run by hand or on another element
  page.click(page.$('#an-banners .an-consent'));
  page.click(page.$('#an-banners .an-consent-title'));
  page.click(page.$('#an-header .an-title'));
  page.run("agentnotchPanel.actions['consent-on']()");
  page.run("agentnotchPanel.actions['consent-on']('', document.querySelector('[data-an-action=\"consent-later\"]'), {type: 'click'})");
  page.run("agentnotchPanel.actions['consent-on']('', document.querySelector('[data-an-action=\"consent-on\"]'), {type: 'keydown'})");
  page.run("(function () { var b = document.createElement('button'); b.setAttribute('data-an-action', 'consent-on'); document.getElementById('an-rows').appendChild(b); b.click(); b.remove(); })()");
  await page.settle();
  assert.deepEqual(page.hub.of('hook_consent'), [], 'only a click on Turn on itself');
  assert.deepEqual(page.hub.of('hooks_enabled'), []);
  assert.ok(page.$('#an-banners .an-consent'), 'and the card is still there to be answered');
  clean(page);
});

test('a refused consent (sealed) brings the card back; --no-install says so and cannot be turned on', async () => {
  const page = await open(unasked, { replies: { hook_consent: () => { throw { code: 'sealed', message: 'Sealed: hook_consent does nothing here.' }; } } });
  page.click(consentButton(page, 'consent-on'));
  assert.equal(page.$('#an-banners .an-consent'), null);
  await page.settle();
  assert.ok(page.$('#an-banners .an-consent'), 'nothing was decided');
  assert.equal(page.hub.of('hook_consent').length, 1, 'not retried');
  const off = await open((s) => { unasked(s); s.setup.install_disabled = true; });
  assert.equal(text(off.$('#an-banners [data-key="install-off"]')), 'Installing is off for this run (--no-install).');
  assert.ok(consentButton(off, 'consent-on').hasAttribute('disabled'));
  off.click(consentButton(off, 'consent-on'));
  off.fire(consentButton(off, 'consent-on'), 'click');
  silent(off);
});

test('the official Codenotch\'s hooks stay: said in the card, never a take-over', async () => {
  const page = await open((s) => { unasked(s); s.setup.codenotch_hooks_folders = ['~\\.claude']; });
  assert.equal(text(page.$('#an-banners [data-key="codenotch"]')), 'Codenotch’s hooks stay; remove them per folder in Settings › Claude Code.');
  assert.equal(text(consentButton(page, 'consent-on')), 'Turn on');
  page.click(consentButton(page, 'consent-on'));
  assert.deepEqual(plain(page.hub.calls).map((c) => c.method), ['hook_consent'], 'turning on removes nothing of Codenotch\'s');
});

// ---- the other banners ---------------------------------------------------------------------------

const banner = (page, id) => page.$(`#an-banners [data-key="b-${id}"]`);
const bannerText = (page, id) => [text(banner(page, id).querySelector('.an-banner-title')), text(banner(page, id).querySelector('.an-banner-msg'))];

test('the fixture\'s healthy setup shows no banner, and before consent only the card shows', async () => {
  const page = await open();
  assert.equal(page.$('#an-banners').childNodes.length, 0);
  const before = await open((s) => {
    unasked(s);
    s.setup.new_install_folders = ['a'];
    s.setup.control_off = true;
    s.setup.missing_hooks_accounts = ['Work'];
    s.setup.codenotch_hooks_folders = ['b'];
    s.setup.install_disabled = true;
  });
  assert.deepEqual(before.$('#an-banners').children.map((c) => c.getAttribute('data-key')), ['consent'],
    'the card already explains why nothing is live');
});

test('the scope notice: OK acknowledges, Turn off turns the hooks off, each once', async () => {
  const folders = (n) => (s) => { s.setup.new_install_folders = Array.from({ length: n }, (_, i) => '~\\.claude-windows\\' + i); };
  const page = await open(folders(3));
  assert.deepEqual(bannerText(page, 'scope'), ['Claude Code control now covers your VS Code workspaces',
    'This version puts its hooks and status line in 3 VS Code workspaces’ folders too, and in new ones as they appear: Claude Parallel Profiles runs Claude Code there. Each settings.json has a backup beside it. Account stores never get hooks.']);
  assert.deepEqual(banner(page, 'scope').querySelectorAll('button').map(text), ['OK', 'Turn off']);
  const ok = banner(page, 'scope').querySelector('[data-an-action="scope-ok"]');
  page.click(ok);
  page.fire(ok, 'click');
  assert.deepEqual(plain(page.hub.calls), [{ method: 'acknowledge_scope', args: null }]);
  assert.equal(banner(page, 'scope'), null);
  const one = await open(folders(1));
  assert.match(bannerText(one, 'scope')[1], /in 1 VS Code workspace’s folder too/);
  const off = one.$('[data-an-action="scope-off"]');
  one.click(off);
  one.fire(off, 'click');
  assert.deepEqual(plain(one.hub.calls), [CALLS.find((e) => e.call.method === 'hooks_enabled').call]);
  assert.deepEqual(plain(one.hub.calls[0].args), { on: false });
  // no key reaches either
  const keyed = await ready(folders(2));
  for (const key of SHORTCUTS) keyed.key(key);
  keyed.run("agentnotchPanel.actions['scope-off']()");
  keyed.run("agentnotchPanel.actions['scope-ok']()");
  assert.deepEqual(keyed.hub.of('hooks_enabled').concat(keyed.hub.of('acknowledge_scope')), []);
  clean(page);
  clean(one);
});

test('the pipe error is shown with what still works', async () => {
  const page = await open((s) => { s.setup.transport_error = 'The hook pipe couldn’t be opened (access is denied).'; });
  assert.deepEqual(bannerText(page, 'pipe'), ['Not receiving hook events',
    'The hook pipe couldn’t be opened (access is denied). Sessions still update from Claude Code’s session files, without approvals.']);
  assert.equal(banner(page, 'pipe').querySelectorAll('button').length, 0);
});

test('control off is said once and honestly; missing hooks agree in number (B_ReviewFixesTests, Fix_SettingsAndPanelCopyTests)', async () => {
  const off = await open((s) => { s.setup.control_off = true; s.setup.missing_hooks_accounts = ['Work', 'Personal']; });
  assert.deepEqual(bannerText(off, 'off'), ['Claude Code control is off',
    'Answer prompts where Claude Code runs (VS Code or the terminal). Sessions still show here and finish from their transcripts.']);
  assert.equal(banner(off, 'missing'), null, 'not per account as well');
  assert.doesNotMatch(bannerText(off, 'off')[1], /done/);
  off.click(banner(off, 'off').querySelector('button'));
  assert.equal(text(banner(off, 'off').querySelector('button')), 'Turn on…');
  assert.deepEqual(plain(off.hub.calls), [{ method: 'open_settings', args: { tab: 'claude' } }], 'Turn on… opens Settings; it turns nothing on');

  const missing = (names) => open((s) => { s.setup.missing_hooks_accounts = names; });
  const one = await missing(['Work']);
  assert.deepEqual(bannerText(one, 'missing'), ['Hooks are missing in Work.',
    'Its sessions still show here; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are back.']);
  assert.doesNotMatch(bannerText(one, 'missing')[1], /done/);
  const two = await missing(['Work', 'Side project']);
  assert.equal(bannerText(two, 'missing')[0], 'Hooks are missing in Work and Side project.');
  assert.match(bannerText(two, 'missing')[1], /^Their sessions/);
  const three = await missing(['A', 'B', 'C']);
  assert.equal(bannerText(three, 'missing')[0], 'Hooks are missing in 3 accounts.');
  assert.equal(text(banner(one, 'missing').querySelector('button')), 'Settings…');
  one.click(banner(one, 'missing').querySelector('button'));
  assert.deepEqual(plain(one.hub.calls), [{ method: 'open_settings', args: { tab: 'claude' } }]);
});

test('the Codenotch-hooks note points at Settings and removes nothing; --no-install is said', async () => {
  const page = await open((s) => { s.setup.codenotch_hooks_folders = ['~\\.claude', '~\\.claude-work']; s.setup.install_disabled = true; });
  assert.deepEqual(bannerText(page, 'codenotch'), ['Codenotch’s hooks are installed too',
    'The official Codenotch has its own hooks in 2 folders. They run beside this app’s; remove them per folder in Settings › Claude Code.']);
  page.click(banner(page, 'codenotch').querySelector('button'));
  assert.deepEqual(plain(page.hub.calls), [{ method: 'open_settings', args: { tab: 'claude' } }]);
  assert.deepEqual(page.hub.of('remove_codenotch_hooks'), []);
  assert.equal(bannerText(page, 'install-off')[0], 'Installing is off for this run (--no-install).');
  assert.deepEqual(page.$('#an-banners').children.map((c) => c.getAttribute('data-key')), ['b-codenotch', 'b-install-off']);
  const single = await open((s) => { s.setup.codenotch_hooks_folders = ['~\\.claude']; });
  assert.match(bannerText(single, 'codenotch')[1], /in 1 folder\./);
});

test('the banners keep the Mac\'s order: scope, pipe, hooks, then the notes', async () => {
  const page = await open((s) => {
    s.setup.new_install_folders = ['x'];
    s.setup.transport_error = 'Broken.';
    s.setup.missing_hooks_accounts = ['Work'];
    s.setup.codenotch_hooks_folders = ['y'];
  });
  assert.deepEqual(page.$('#an-banners').children.map((c) => c.getAttribute('data-key')), ['b-scope', 'b-pipe', 'b-missing', 'b-codenotch']);
  silent(page);
});

// ---- hostile strings -----------------------------------------------------------------------------

test('hostile strings in requests, questions, options, rules, paths, accounts and errors are text', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await ready((s) => {
      const perm = session(s, 'needs-permission');
      perm.pending.request = evil;
      perm.pending.always = evil;
      perm.pending.tool_name = evil;
      perm.detail.request = evil;
      perm.detail.tool = evil;
      perm.focus_label = evil;
      const q = session(s, 'needs-question').pending.questions[0];
      q.text = evil;
      q.header = evil;
      q.options = [{ label: evil, description: evil }, { label: evil + '2', description: null }];
      session(s, 'needs-question').detail.text = evil;
      session(s, 'needs-elicitation').focus_label = evil;
      s.setup.consent_files = [{ path: evil, account: evil }];
      s.setup.transport_error = evil;
      s.setup.missing_hooks_accounts = [evil, evil];
      s.setup.new_install_folders = [evil];
      s.setup.codenotch_hooks_folders = [evil];
    });
    assert.deepEqual(audit.problems(page.$('#an-banners')), [], evil.slice(0, 30));
    assert.deepEqual(audit.problems(page.$('#an-rows')), [], evil.slice(0, 30));
    assert.deepEqual(page.$$('#an-card script, #an-card iframe, #an-card img, #an-card a, #an-card style, #an-card base, #an-card input, #an-card textarea'), []);
    const cap = text(bar(page, 'needs-permission').querySelector('.an-act-cap'));
    if (evil.length < 300) assert.equal(cap, audit.flat('Always: ' + evil));
    else assert.ok(cap.length < 500, 'a 10 000-character rule is cut');
    // the answer carries the question and the label exactly as the engine sent them
    page.click(button(page, 'needs-question', 'option:0'));
    assert.deepEqual(answers(page)[0].answer, { questions: { answers: { [evil]: evil } } });
    clean(page);
    // the consent card with the same strings
    const card = await open((s) => { unasked(s); s.setup.consent_files = [{ path: evil, account: evil }]; s.setup.codenotch_hooks_folders = [evil]; });
    assert.deepEqual(audit.problems(card.$('#an-banners')), [], evil.slice(0, 30));
    assert.equal(card.$$('#an-banners button').length, 2, 'no button came from the text');
    if (evil.length < 300) assert.equal(text(card.$('.an-cfile')), audit.flat(`${evil} (${evil})`));
    clean(card);
  }
});

test('an option that looks like a command or a prototype name is only ever a label', async () => {
  const page = await ready((s) => {
    const q = session(s, 'needs-question').pending.questions[0];
    q.text = '__proto__';
    q.options = [{ label: 'constructor', description: null }, { label: 'toString', description: null }];
  });
  select(page, 'needs-question');
  page.key({ key: '2' });
  const sent = page.hub.of('answer')[0].args.answer.questions.answers;
  assert.deepEqual(Object.keys(sent), ['__proto__']);
  assert.equal(sent['__proto__'], 'toString');
  clean(page);
});

// ---- sealed scenes -------------------------------------------------------------------------------

test('the four scenes show their states; a scene\'s bars are drawn answerable and answer nothing', async () => {
  const page = await open();
  const show = (name) => assert.equal(page.run(`agentnotchPanel.showScene('${name}')`), true, name);
  show('panel-needs-you');
  const kinds = page.$$('#an-rows .an-row').map((r) => r.querySelector('.an-row-actions').querySelectorAll('button').map(text).join('|'));
  for (const bar of ['Review plan|Approve', '1Recharts|2Chart.js|3ECharts|Other…', 'Show terminal', 'Deny|Always|Allow', 'Answer…', 'Deny|Review…', '']) {
    assert.ok(kinds.includes(bar), `a row with "${bar}" in ${JSON.stringify(kinds)}`);
  }
  assert.deepEqual(page.$$('#an-rows .an-sh').length, 1, 'only Needs you');
  assert.equal(page.$$('#an-rows [data-an-action="answer"][aria-disabled]').length, 0, 'a still picture has no wait');
  page.tick(1000);
  page.click(page.$$('#an-rows [data-an-action="answer"]').find((b) => b.getAttribute('data-an-arg') === 'allow'));
  focus(page);
  page.run("agentnotchPanel._.state.selected = 'needs-plan'");
  page.key(CTRL_ENTER);
  show('panel-consent');
  assert.ok(page.$('#an-banners .an-consent'));
  assert.equal(page.text('.an-empty-title'), 'No Claude sessions yet');
  page.click(consentButton(page, 'consent-on'));
  page.click(consentButton(page, 'consent-later'));
  assert.ok(page.$('#an-banners .an-consent'), 'a scene is a picture: its card stays');
  show('panel-banners');
  assert.deepEqual(page.$('#an-banners').children.map((c) => c.getAttribute('data-key')), ['b-pipe', 'b-missing', 'b-codenotch']);
  show('panel-scope-notice');
  assert.deepEqual(page.$('#an-banners').children.map((c) => c.getAttribute('data-key')), ['b-scope']);
  page.click(page.$('[data-an-action="scope-off"]'));
  page.click(page.$('[data-an-action="scope-ok"]'));
  await page.settle();
  silent(page, 'nothing is answered, granted or turned off from a scene');
  assert.equal(plain(page.run('agentnotchPanel.layoutReport()')).ok, true);
  clean(page);
});

test('scenes.json lists the four scenes of this sub-task', () => {
  const names = require('./scenes.json').scenes.map((s) => s.name);
  for (const name of ['panel-needs-you', 'panel-banners', 'panel-consent', 'panel-scope-notice']) assert.ok(names.includes(name), name);
});
