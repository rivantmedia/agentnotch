'use strict';
// ui/agentnotch/chat.js and the chat's part of panel.js and panel.css: the header, the transcript
// (reset and patches, every item kind), images on demand, links, the task board, scrolling, the
// sealed scenes. The markup of a message is markdown.js's and of a result toolresults.js's (their
// suites prove them); here they are driven the way the chat drives them, on the real panel.html.
//
// Swift tests ported: B_MovedChatSettingsTests' chat side (the context meter's levels; the
// question forms of the same file are the bottom bar's, sub-task 8), ChatTranscript's paging
// (`pageSize` 150, "Show N earlier messages", the window that stays put), ToolCallSummary (a
// subagent's "description · N tools", the name never repeated), ThinkingView's 90 characters,
// ToolStatusMark's four marks, ChatTaskBoard (8 rows, 4 while an answer bar shows, the board
// closes when a request appears). New: the an:chat protocol (reset, patch, removed, order, stale
// revision, wrong session), keyed re-rendering, expanded state across patches, images only on
// demand and only as data URLs, https-only links, the keyboard gate on Enter, hostile strings
// through every field a transcript carries, a 10 000-character line.
//
// What node cannot know is layout: the snapshot tool renders chat-* in a browser and
// layoutReport() measures the overflow there.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');
const scripts = require('./lib/scripts.cjs');

const CSS = fs.readFileSync(path.join(harness.UI, 'agentnotch', 'panel.css'), 'utf8');
const plain = scripts.plain;
const text = (el) => el.textContent.replace(/\s+/g, ' ').trim();
/** The parts of an element, apart: layout puts the space between them, the markup does not. */
const parts = (el) => el.children.map(text).filter(Boolean).join(' ');

// ---- helpers ---------------------------------------------------------------------------------

const ID = 'needs-permission';

/** The panel on a chat route, with the snapshot edited by `edit`. `chat` false: nothing emitted yet. */
async function openChat(options) {
  const o = Object.assign({ id: ID, edit: null, chat: true, focused: false }, options);
  const snapshot = harness.fixture('snapshot.json');
  if (o.edit) o.edit(snapshot);
  const page = harness.loadPage('agentnotch/panel.html', {
    snapshot,
    before: (window) => { window.__AGENTNOTCH_PANEL__ = { route: 'session:' + o.id, reason: 'hover_row' }; },
  });
  await page.settle();
  if (o.focused) page.emit('an:panel_focus', { focused: true });
  if (o.chat) page.emit('an:chat', Object.assign(harness.fixture('chat.json'), { session_id: o.id }));
  return page;
}

const chatFx = (edit) => {
  const fx = harness.fixture('chat.json');
  if (edit) edit(fx);
  return fx;
};

const patch = (revision, items, extra) => Object.assign({
  session_id: ID, revision, reset: false, items, removed: [], order: null, has_earlier: 0, working: null, ended: false, loading: false,
}, extra);

const assistant = (id, body) => ({ id, kind: 'assistant', text: body });
const user = (id, body) => ({ id, kind: 'user', text: body });
const listEl = (page) => page.$('#an-chat-list');
const itemKeys = (page) => listEl(page).children.map((el) => el.getAttribute('data-key')).filter((k) => k && k.startsWith('i:')).map((k) => k.slice(2));
const itemEl = (page, id) => listEl(page).children.find((el) => el.getAttribute('data-key') === 'i:' + id);
const calls = (page, method) => page.hub.of(method).map((c) => plain(c.args));
const state = (page) => plain(page.run('agentnotchChat.current()'));
const headText = (page) => text(page.$('#an-chat-head'));
const clean = (page) => {
  assert.deepEqual(page.errors.map(String), [], 'no page error');
  assert.deepEqual(page.hub.violations, [], 'no call the contract or the window gate refuses');
};

/** Counts the calls a renderer gets: only what changed may be drawn again. */
function spy(page, path) {
  page.run(`(function () { var o = ${path.split('.').slice(0, -1).join('.')}; var k = '${path.split('.').pop()}'; var f = o[k]; window.__spy = window.__spy || {}; window.__spy['${path}'] = 0; o[k] = function () { window.__spy['${path}'] += 1; return f.apply(this, arguments); }; })()`);
  return () => page.run(`window.__spy['${path}']`);
}

// ---- entering and leaving ---------------------------------------------------------------------

test('entering a chat asks the engine for it once, shows "Loading the conversation…" until the reset, and leaving closes it', async () => {
  const page = await openChat({ chat: false });
  assert.deepEqual(calls(page, 'chat_open'), [{ session_id: ID }]);
  assert.equal(text(listEl(page)), 'Loading the conversation…');
  assert.equal(state(page).loading, true);
  page.emit('an:chat', chatFx());
  assert.equal(state(page).loading, false);
  assert.equal(state(page).order.length, 8);
  page.hub.clear();
  page.click('[data-an-action="back"]');
  assert.deepEqual(calls(page, 'chat_close'), [{ session_id: ID }]);
  assert.equal(page.$('#an-card').getAttribute('data-mode'), 'list');
  assert.equal(page.run('agentnotchChat.current()'), null);
  assert.equal(page.$('#an-chat').children.length, 0, 'nothing of the chat stays in the page');
  clean(page);
});

test('the chat listens before it asks, and a second visit starts from nothing', async () => {
  const page = await openChat({ chat: false });
  assert.equal(page.listening('an:chat'), true);
  page.emit('an:chat', chatFx());
  assert.equal(state(page).revision, 1);
  page.click('[data-an-action="back"]');
  page.run("agentnotchPanel.navigate('session:" + ID + "')");
  await page.settle();
  assert.equal(state(page).loading, true, 'the earlier page is gone');
  assert.equal(state(page).revision, -1);
  page.emit('an:chat', chatFx());
  assert.equal(state(page).revision, 1, 'a reset of the same revision is taken: it is a new visit');
  assert.equal(calls(page, 'chat_open').length, 2);
  clean(page);
});

test('an engine that refuses chat_open leaves a quiet line, not a spinner for ever', async () => {
  const page = harness.loadPage('agentnotch/panel.html', {
    before: (window) => { window.__AGENTNOTCH_PANEL__ = { route: 'session:' + ID }; },
    replies: { chat_open: () => { throw { code: 'failed', message: 'nope' }; } },
  });
  await page.settle();
  assert.equal(state(page).loading, false);
  assert.equal(text(listEl(page)), 'The conversation couldn’t be loaded.');
  assert.deepEqual(page.errors.map(String), []);
});

// ---- the reset: every kind of item -------------------------------------------------------------

test('a reset draws every item kind of the fixture in order', async () => {
  const page = await openChat();
  assert.deepEqual(itemKeys(page), ['u-0001-text-0', 'a-0002-thinking-0', 'a-0002-text-1', 'toolu_read_1', 'toolu_edit_1', 'toolu_agent_1', 'u-0003-image-0', 'toolu_sample_bash']);
  const u = itemEl(page, 'u-0001-text-0');
  assert.ok(u.querySelector('.an-bubble'), 'a user message is a bubble');
  assert.equal(text(u), 'The login page keeps redirecting to itself. Can you fix it?');
  const thinking = itemEl(page, 'a-0002-thinking-0');
  assert.equal(text(thinking), 'The redirect target comes from the query string without a check.');
  assert.equal(thinking.querySelector('button'), null, 'short thinking has nothing to expand');
  const a = itemEl(page, 'a-0002-text-1');
  assert.equal(a.querySelector('strong').textContent, 'safe-target check', 'an assistant message is markdown');
  clean(page);
});

test('a tool call: its mark, its name, what it worked on; the result waits for a click; Edit always shows its change', async () => {
  const page = await openChat();
  const read = itemEl(page, 'toolu_read_1');
  assert.equal(text(read.querySelector('.an-tool-name')), 'Read');
  assert.equal(text(read.querySelector('.an-tool-sum')), 'src\\auth\\redirect.ts');
  assert.ok(read.querySelector('.an-toolmark.an-ring-idle'), 'success: a grey ring');
  assert.equal(read.querySelector('.an-tool-result'), null, 'the result is asked for');
  const head = read.querySelector('button.an-tool-head');
  assert.equal(head.getAttribute('aria-expanded'), 'false');
  page.click(head);
  assert.equal(itemEl(page, 'toolu_read_1').querySelector('button.an-tool-head').getAttribute('aria-expanded'), 'true');
  assert.match(text(itemEl(page, 'toolu_read_1').querySelector('.an-tool-result')), /export function next/);
  page.click(itemEl(page, 'toolu_read_1').querySelector('button.an-tool-head'));
  assert.equal(itemEl(page, 'toolu_read_1').querySelector('.an-tool-result'), null, 'a second click closes it');

  const edit = itemEl(page, 'toolu_edit_1');
  assert.equal(edit.querySelector('button'), null, 'Edit cannot be toggled');
  assert.equal(edit.querySelectorAll('.an-diff-add').length, 2);
  assert.equal(edit.querySelectorAll('.an-diff-remove').length, 1);
  assert.equal(text(edit.querySelector('.an-code-head')), 'redirect.ts');
  clean(page);
});

test('the four marks of a tool: running turns, waiting is half an amber ring, success a grey ring, error and interrupted a red dot', async () => {
  const page = await openChat();
  const marks = {};
  const items = ['running', 'waiting_for_approval', 'success', 'error', 'interrupted'].map((status) => ({
    id: 'm-' + status, kind: 'tool', name: 'Grep', summary: 'x', status, input: {}, result: null, subagent: null,
  }));
  page.emit('an:chat', patch(2, items, { order: items.map((i) => i.id) }));
  for (const status of ['running', 'waiting_for_approval', 'success', 'error', 'interrupted']) marks[status] = itemEl(page, 'm-' + status).querySelector('.an-toolmark').getAttribute('class');
  assert.match(marks.running, /an-ring-working/);
  assert.match(marks.running, /an-spin/);
  assert.match(marks.waiting_for_approval, /an-ring-needs/);
  assert.match(marks.success, /an-ring-idle/);
  assert.match(marks.error, /an-ring-error/);
  assert.match(marks.interrupted, /an-ring-error/);
  assert.match(text(itemEl(page, 'm-error').querySelector('.an-tool-name')), /Grep/);
  assert.match(itemEl(page, 'm-error').querySelector('.an-tool-name').getAttribute('class'), /an-critical/, 'a failed tool\'s name is red');
  assert.doesNotMatch(itemEl(page, 'm-success').querySelector('.an-tool-name').getAttribute('class'), /an-critical/);
  clean(page);
});

test('a running tool shows no result; a running Edit shows the change it is about to make from its input', async () => {
  const page = await openChat();
  const items = [
    { id: 'r1', kind: 'tool', name: 'Bash', summary: 'ls', status: 'running', input: { command: 'ls' }, result: { tool: 'bash', stdout: 'a', stderr: '', interrupted: false, return_code_interpretation: null, background_task_id: null }, subagent: null },
    { id: 'e1', kind: 'tool', name: 'Edit', summary: 'a.ts', status: 'waiting_for_approval', input: { file_path: 'C:\\x\\a.ts', old_string: 'one', new_string: 'two' }, result: null, subagent: null },
  ];
  page.emit('an:chat', patch(2, items, { order: ['r1', 'e1'] }));
  assert.equal(itemEl(page, 'r1').querySelector('.an-tool-result'), null);
  assert.equal(itemEl(page, 'r1').querySelector('.an-chev'), null, 'nothing to expand while it runs');
  const edit = itemEl(page, 'e1');
  assert.equal(edit.querySelectorAll('.an-diff-add').length, 1);
  assert.equal(edit.querySelectorAll('.an-diff-remove').length, 1);
  assert.equal(text(edit.querySelector('.an-code-head')), 'a.ts');
  clean(page);
});

test('a subagent: "description · N tools", the last two tools, "+N earlier tool uses"; the container is not expandable', async () => {
  const page = await openChat();
  const agent = itemEl(page, 'toolu_agent_1');
  assert.equal(text(agent.querySelector('.an-tool-name')), 'Agent');
  assert.equal(text(agent.querySelector('.an-tool-sum')), 'Find every caller of next() · 1 tool');
  assert.equal(agent.querySelector('button'), null);
  assert.equal(agent.querySelector('.an-tool-result'), null, 'its result is the subagent\'s list, not a view');
  assert.equal(agent.querySelectorAll('.an-subtool').length, 1);
  assert.equal(parts(agent.querySelector('.an-subtool')), 'Grep next\\(');
  assert.equal(agent.querySelector('.an-subtool-more'), null);
  const tools = ['a', 'b', 'c', 'd'].map((n, i) => ({ id: 'sub-' + n, name: 'Read', summary: 'file-' + n, status: i === 3 ? 'interrupted' : 'success' }));
  const bigger = { id: 'ag2', kind: 'tool', name: 'Task', summary: '', status: 'running', input: { description: 'Outer' }, result: null, subagent: { agent_id: 'q', description: null, tools } };
  page.emit('an:chat', patch(2, [bigger], { order: ['ag2'] }));
  const two = itemEl(page, 'ag2');
  assert.equal(text(two.querySelector('.an-tool-sum')), 'Outer · 4 tools', 'the input names it when the subagent does not');
  assert.equal(text(two.querySelector('.an-subtool-more')), '+2 earlier tool uses');
  assert.deepEqual(two.querySelectorAll('.an-subtool').map(parts), ['Read file-c', 'Read Interrupted']);
  const plainAgent = { id: 'ag3', kind: 'tool', name: 'Agent', summary: '', status: 'running', input: {}, result: null, subagent: { agent_id: 'q', description: null, tools: [{ id: 't', name: 'Bash', summary: 'go', status: 'running' }] } };
  page.emit('an:chat', patch(3, [plainAgent], { order: ['ag3'] }));
  assert.equal(text(itemEl(page, 'ag3').querySelector('.an-tool-sum')), 'Running an agent · 1 tool');
  clean(page);
});

test('MCP and alias names are the readable ones', async () => {
  const page = await openChat();
  const items = [
    { id: 'n1', kind: 'tool', name: 'mcp__deepwiki__ask_question', summary: 'q', status: 'success', input: {}, result: null, subagent: null },
    { id: 'n2', kind: 'tool', name: 'WebFetch', summary: 'u', status: 'success', input: {}, result: null, subagent: null },
  ];
  page.emit('an:chat', patch(2, items, { order: ['n1', 'n2'] }));
  assert.equal(text(itemEl(page, 'n1').querySelector('.an-tool-name')), 'Deepwiki - Ask Question');
  assert.equal(text(itemEl(page, 'n2').querySelector('.an-tool-name')), 'Fetch');
  clean(page);
});

test('thinking is one italic line cut at 90 characters with a chevron; a click opens the rest', async () => {
  const page = await openChat();
  const long = 'Safari blocks third-party cookies by default, so the session cookie set by the auth callback never arrives. '.repeat(3);
  page.emit('an:chat', patch(2, [{ id: 'th', kind: 'thinking', text: long }, { id: 'th2', kind: 'thinking', text: '   ' }, { id: 'th3', kind: 'thinking', text: 'x'.repeat(90) }], { order: ['th', 'th2', 'th3'] }));
  const t = itemEl(page, 'th');
  assert.equal(text(t.querySelector('.an-think-t')), long.slice(0, 90).trim() + '…');
  assert.ok(t.querySelector('.an-chev'));
  assert.equal(itemEl(page, 'th2'), undefined, 'a blank thought draws nothing');
  assert.equal(itemEl(page, 'th3').querySelector('button'), null, 'exactly 90 characters fits');
  page.click(t.querySelector('button'));
  const open = itemEl(page, 'th');
  assert.equal(text(open.querySelector('.an-think-t')), text({ textContent: long }));
  assert.match(open.querySelector('.an-chev').getAttribute('class'), /an-chev-open/);
  assert.equal(open.querySelector('button').getAttribute('aria-expanded'), 'true');
  clean(page);
});

test('an assistant message with no text, an interrupted marker, an unknown kind and an item without an id', async () => {
  const page = await openChat();
  page.emit('an:chat', patch(2, [assistant('blank', '  \n '), { id: 'int', kind: 'interrupted' }, { id: 'odd', kind: 'hologram', text: 'x' }, { kind: 'assistant', text: 'no id' }, null, 'junk'], { order: ['blank', 'int', 'odd', 'ghost', 'int'] }));
  assert.equal(itemEl(page, 'blank'), undefined, 'tool-only turns draw no empty line');
  assert.equal(text(itemEl(page, 'int')), 'Interrupted');
  assert.match(itemEl(page, 'int').getAttribute('class'), /an-ci-interrupted/);
  assert.equal(itemEl(page, 'odd'), undefined);
  assert.deepEqual(state(page).order, ['blank', 'int', 'odd'], 'the order names only items it has, once each');
  clean(page);
});

// ---- patches --------------------------------------------------------------------------------------

test('the events.json patch updates the Bash row in place, adds the assistant item and follows `order`', async () => {
  const page = await openChat();
  const events = harness.fixture('events.json');
  const payload = events.find((e) => e.event === 'an:chat' && e.payload).payload;
  const read = itemEl(page, 'toolu_read_1');
  const bash = itemEl(page, 'toolu_sample_bash');
  assert.match(bash.querySelector('.an-toolmark').getAttribute('class'), /an-ring-needs/);
  const markdownCalls = spy(page, 'agentnotchMarkdown.render');
  const resultCalls = spy(page, 'agentnotchToolResults.render');
  page.emit('an:chat', payload);
  assert.deepEqual(itemKeys(page), payload.order);
  assert.equal(itemEl(page, 'toolu_read_1'), read, 'an unchanged item keeps its element');
  assert.equal(itemEl(page, 'toolu_sample_bash'), bash, 'the changed item is patched in place');
  assert.match(bash.querySelector('.an-toolmark').getAttribute('class'), /an-ring-idle/, 'now succeeded');
  assert.equal(text(itemEl(page, 'a-0004-text-0')), 'All 4 redirect tests pass.');
  assert.equal(markdownCalls(), 1, 'only the new message was rendered');
  assert.equal(resultCalls(), 0, 'Edit is unchanged (cached) and the changed Bash is closed: no result was drawn');
  assert.equal(state(page).revision, 2);
  clean(page);
});

test('the order of a patch is the page\'s order; `removed` ids vanish', async () => {
  const page = await openChat();
  page.emit('an:chat', patch(2, [], { order: ['toolu_sample_bash', 'u-0001-text-0', 'toolu_read_1'], removed: ['a-0002-thinking-0', 'a-0002-text-1', 'toolu_edit_1', 'toolu_agent_1', 'u-0003-image-0'] }));
  assert.deepEqual(itemKeys(page), ['toolu_sample_bash', 'u-0001-text-0', 'toolu_read_1']);
  assert.deepEqual(state(page).order, ['toolu_sample_bash', 'u-0001-text-0', 'toolu_read_1']);
  page.emit('an:chat', patch(3, [], { removed: ['toolu_read_1', 7, null], order: ['toolu_sample_bash', 'u-0001-text-0', 'toolu_read_1'] }));
  assert.deepEqual(itemKeys(page), ['toolu_sample_bash', 'u-0001-text-0'], 'a removed id stays removed even when a stale order still names it');
  clean(page);
});

test('a patch without an order keeps the order and appends what is new', async () => {
  const page = await openChat();
  const update = patch(2, [assistant('later', 'one more')]);
  delete update.order;
  page.emit('an:chat', update);
  assert.equal(itemKeys(page).pop(), 'later');
  assert.equal(itemKeys(page).length, 9);
  clean(page);
});

test('a patch with an older or equal revision, or for another session, is ignored; a late older reset too', async () => {
  const page = await openChat();
  const before = itemKeys(page);
  page.emit('an:chat', patch(1, [assistant('same', 'equal revision')], { order: before.concat('same') }));
  page.emit('an:chat', patch(0, [assistant('old', 'older')], { order: before.concat('old') }));
  page.emit('an:chat', Object.assign(patch(9, [assistant('other', 'wrong session')], { order: ['other'] }), { session_id: 'needs-plan' }));
  page.emit('an:chat', Object.assign(chatFx(), { revision: 0 }));
  page.emit('an:chat', null);
  page.emit('an:chat', 'junk');
  page.emit('an:chat', patch('x', [assistant('nan', 'not a number')]));
  assert.deepEqual(itemKeys(page), before);
  assert.equal(state(page).revision, 1);
  page.emit('an:chat', patch(5, [assistant('new', 'newer')], { order: before.concat('new') }));
  assert.equal(itemKeys(page).pop(), 'new');
  page.emit('an:chat', patch(4, [assistant('late', 'late')], { order: before.concat('new', 'late') }));
  assert.equal(itemKeys(page).pop(), 'new', 'revisions only go forward');
  clean(page);
});

test('a reset replaces the page; a reset for another session does not', async () => {
  const page = await openChat();
  page.emit('an:chat', chatFx((fx) => { fx.session_id = 'needs-plan'; fx.revision = 50; }));
  assert.equal(itemKeys(page).length, 8, 'another session\'s reset is not ours');
  const reset = patch(2, [assistant('only', 'the whole page')], { reset: true, order: ['only'] });
  page.emit('an:chat', reset);
  assert.deepEqual(itemKeys(page), ['only']);
  clean(page);
});

test('expanded state survives a patch; a removed item\'s state does not come back', async () => {
  const page = await openChat();
  const long = 'a thought that is certainly longer than ninety characters, so that it can open and close, ok? '.repeat(2);
  page.emit('an:chat', patch(2, [{ id: 'th', kind: 'thinking', text: long }], { order: state(page).order.concat('th') }));
  page.click(itemEl(page, 'th').querySelector('button'));
  page.click(itemEl(page, 'toolu_sample_bash').querySelector('button') || itemEl(page, 'toolu_read_1').querySelector('button'));
  const open = () => page.$$('#an-chat-list [aria-expanded="true"]').map((el) => el.getAttribute('data-id'));
  assert.deepEqual(open().sort(), ['th', 'toolu_read_1'].sort());
  page.emit('an:chat', patch(3, [assistant('next', 'another reply')], { order: state(page).order.concat('next') }));
  assert.deepEqual(open().sort(), ['th', 'toolu_read_1'].sort(), 'both stay open');
  page.emit('an:chat', patch(4, [], { removed: ['th'], order: state(page).order.filter((id) => id !== 'th') }));
  page.emit('an:chat', patch(5, [{ id: 'th', kind: 'thinking', text: long }], { order: state(page).order.concat('th') }));
  assert.deepEqual(open(), ['toolu_read_1'], 'a thought that came back is closed');
  clean(page);
});

test('an item changed by a patch is redrawn, the rest is not', async () => {
  const page = await openChat();
  const markdownCalls = spy(page, 'agentnotchMarkdown.render');
  page.emit('an:chat', patch(2, [assistant('a-0002-text-1', 'Now **changed**')], { order: state(page).order }));
  assert.equal(markdownCalls(), 1);
  assert.equal(itemEl(page, 'a-0002-text-1').querySelector('strong').textContent, 'changed');
  page.emit('an:chat', patch(3, [], { order: state(page).order }));
  assert.equal(markdownCalls(), 1, 'nothing changed: nothing was drawn');
  clean(page);
});

// ---- the states ---------------------------------------------------------------------------------------

test('loading, empty, working and ended', async () => {
  const page = await openChat({ chat: false });
  page.emit('an:chat', patch(1, [], { reset: true, order: [], loading: true }));
  assert.equal(text(listEl(page)), 'Loading the conversation…');
  page.emit('an:chat', patch(2, [], { reset: true, order: [] }));
  assert.equal(text(listEl(page)), 'No messages yet');
  page.emit('an:chat', patch(3, [], { reset: true, order: [], working: 'Working…' }));
  assert.equal(text(page.$('.an-working')), 'Working…', 'a working session with nothing yet shows the indicator, not "No messages"');
  assert.ok(page.$('.an-working .an-ring-working.an-spin'));
  page.emit('an:chat', patch(4, [user('u', 'Go')], { reset: true, order: ['u'], working: 'Compacting context…' }));
  assert.equal(listEl(page).children.map((c) => c.getAttribute('class')).pop().includes('an-working'), true, 'the indicator is under the last item');
  assert.equal(text(page.$('.an-working')), 'Compacting context…');
  page.emit('an:chat', patch(5, [], { order: ['u'], working: 'Waiting on the build…', ended: true }));
  assert.equal(page.$('.an-working'), null, 'an ended session is not working');
  assert.equal(state(page).ended, true);
  page.emit('an:chat', patch(6, [], { order: ['u'], working: 'Thinking…' }));
  assert.equal(text(page.$('.an-working')), 'Thinking…');
  page.emit('an:chat', patch(7, [], { order: ['u'], working: '' }));
  assert.equal(page.$('.an-working'), null);
  clean(page);
});

// ---- the status line (ChatStatusLine.swift, ChatStatusRow) ---------------------------------------------

const FAILED = { glyph: 'error', text: 'Rate limited · 5-hour limit resets in 47m', can_dismiss: true };
const REVIEW = { glyph: 'review', text: 'Ready for review · finished 5m ago', can_dismiss: false };
const IDLE = { glyph: 'idle', text: 'Idle · last active 1h ago', can_dismiss: false };
const withStatus = (status) => ({ edit: (s) => { s.sessions.find((r) => r.session_id === ID).chat_status = status; } });
const statusEl = (page) => listEl(page).children.find((el) => el.getAttribute('data-key') === 'status');

test('a stopped session ends its chat with the row\'s words: a failure with its reset time, review, idle', async () => {
  for (const [status, mark] of [[FAILED, 'error'], [REVIEW, 'review'], [IDLE, 'idle']]) {
    const page = await openChat(withStatus(status));
    const el = statusEl(page);
    assert.ok(el, 'the status line is there: ' + status.glyph);
    assert.equal(listEl(page).children[listEl(page).children.length - 1], el, 'under the last message');
    assert.equal(text(el.querySelector('.an-status-t')), status.text);
    assert.ok(el.querySelector('.an-ring-' + mark), 'the row\'s mark for the state');
    assert.equal(page.$$('#an-chat-list .an-working').length, 0);
    clean(page);
  }
});

test('the mark and the words read as one element; Dismiss is a button of its own, only for a failure', async () => {
  const failed = await openChat(withStatus(FAILED));
  const read = statusEl(failed).querySelector('.an-status-read');
  assert.equal(read.getAttribute('role'), 'img');
  assert.equal(read.getAttribute('aria-label'), FAILED.text);
  assert.ok(read.querySelector('svg') && read.querySelector('.an-status-t'), 'the mark and the words are inside it');
  const buttons = failed.$$('#an-chat-list .an-status button');
  assert.equal(buttons.length, 1);
  assert.equal(buttons[0].textContent.trim(), 'Dismiss');
  assert.equal(read.contains(buttons[0]), false, 'reading the status never dismisses the failure');
  assert.equal(failed.$$('#an-chat-list .an-status-t button').length, 0);
  assert.ok(failed.$('.an-status-err'), 'the words of a failure are in the critical ink');
  failed.hub.clear();
  failed.click(buttons[0]);
  assert.deepEqual(calls(failed, 'dismiss_failure'), [{ session_id: ID }]);
  assert.deepEqual(calls(failed, 'mark_reviewed'), []);
  clean(failed);

  for (const status of [REVIEW, IDLE]) {
    const page = await openChat(withStatus(status));
    assert.equal(page.$$('#an-chat-list .an-status button').length, 0, 'nothing to dismiss');
    assert.equal(statusEl(page).querySelector('.an-status-read').getAttribute('aria-label'), status.text);
    assert.equal(statusEl(page).classList.contains('an-status-err'), false);
  }
  // only a failure can be dismissed, whatever the payload says
  const odd = await openChat(withStatus({ glyph: 'idle', text: 'Idle', can_dismiss: true }));
  assert.equal(odd.$$('#an-chat-list .an-status button').length, 0);
});

test('an empty chat keeps "No messages yet" unless the session failed', async () => {
  for (const status of [REVIEW, IDLE, null]) {
    const page = await openChat(Object.assign(withStatus(status), { chat: false }));
    page.emit('an:chat', patch(1, [], { reset: true, order: [] }));
    assert.equal(text(listEl(page)), 'No messages yet', 'a line like "Idle · last active…" alone would read as a broken chat');
    assert.equal(statusEl(page), undefined);
    clean(page);
  }
  const failed = await openChat(Object.assign(withStatus(FAILED), { chat: false }));
  failed.emit('an:chat', patch(1, [], { reset: true, order: [] }));
  assert.equal(parts(statusEl(failed)), FAILED.text + ' Dismiss', 'a failure shows with nothing above it');
  assert.equal(listEl(failed).children.length, 1);
  assert.equal(failed.$$('#an-chat-list .an-ph').length, 0);
  clean(failed);
});

test('the status line never shares the transcript with the working indicator, and a session that has gone shows none', async () => {
  const page = await openChat(Object.assign(withStatus(IDLE), { chat: false }));
  page.emit('an:chat', patch(1, [user('u', 'Go')], { reset: true, order: ['u'], working: 'Thinking…' }));
  assert.equal(statusEl(page), undefined, 'the working indicator says it');
  assert.ok(page.$('.an-working'));
  page.emit('an:chat', patch(2, [], { order: ['u'], working: null }));
  assert.ok(statusEl(page));
  assert.equal(page.$('.an-working'), null);
  page.emit('an:chat', patch(3, [], { order: ['u'], ended: true }));
  assert.equal(statusEl(page), undefined, 'an ended chat says nothing of a session that is gone');
  clean(page);
});

test('the status line follows the row: a new snapshot redraws it, an unchanged one leaves it alone', async () => {
  const page = await openChat(withStatus(REVIEW));
  const before = statusEl(page);
  const snapshot = (status) => {
    const s = harness.fixture('snapshot.json');
    s.sessions.find((r) => r.session_id === ID).chat_status = status;
    s.generated_at_ms += 10;
    return s;
  };
  page.emit('an:snapshot', snapshot(REVIEW));
  assert.equal(statusEl(page), before, 'the same words: the same element');
  page.emit('an:snapshot', snapshot(Object.assign({}, REVIEW, { text: 'Ready for review · finished 6m ago' })));
  assert.equal(text(statusEl(page).querySelector('.an-status-t')), 'Ready for review · finished 6m ago');
  page.emit('an:snapshot', snapshot(FAILED));
  assert.equal(text(statusEl(page).querySelector('.an-status-t')), FAILED.text);
  assert.equal(page.$$('#an-chat-list .an-status button').length, 1);
  page.emit('an:snapshot', snapshot(null));
  assert.equal(statusEl(page), undefined, 'the session works again: no line');
  // a row without the field (an older engine) has none either
  const old = snapshot(null);
  delete old.sessions.find((r) => r.session_id === ID).chat_status;
  old.generated_at_ms += 10;
  page.emit('an:snapshot', old);
  assert.equal(statusEl(page), undefined);
  clean(page);
});

test('hostile strings in the status line draw as text; a long one is bounded and wraps', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await openChat(withStatus({ glyph: 'error', text: evil, can_dismiss: true }));
    assert.deepEqual(audit.problems(listEl(page)), [], evil.slice(0, 30));
    assert.deepEqual(page.errors.map(String), []);
  }
  const long = await openChat(withStatus({ glyph: 'error', text: 'W'.repeat(10000) + ' ' + 'x'.repeat(10000), can_dismiss: true }));
  assert.ok(text(statusEl(long).querySelector('.an-status-t')).length <= 500);
  assert.ok(statusEl(long).querySelector('.an-status-read').getAttribute('aria-label').length <= 500);
  const unknown = await openChat(withStatus({ glyph: '<img src=x>', text: 'x', can_dismiss: true }));
  assert.equal(statusEl(unknown), undefined, 'a mark the page does not know draws nothing');
  const empty = await openChat(withStatus({ glyph: 'idle', text: '   ', can_dismiss: false }));
  assert.equal(statusEl(empty), undefined);
  clean(long);
});

// ---- earlier messages and scrolling ---------------------------------------------------------------------

test('has_earlier shows "Show N earlier messages" (at most a page); a click asks for them once', async () => {
  const page = await openChat();
  assert.equal(page.$('.an-earlier'), null);
  page.emit('an:chat', patch(2, [], { order: state(page).order, has_earlier: 1 }));
  assert.equal(text(page.$('.an-earlier')), 'Show 1 earlier message');
  page.emit('an:chat', patch(3, [], { order: state(page).order, has_earlier: 400 }));
  assert.equal(text(page.$('.an-earlier')), 'Show 150 earlier messages');
  assert.equal(listEl(page).children[0], page.$('.an-earlier'), 'at the top');
  page.hub.clear();
  page.click('.an-earlier');
  page.click('.an-earlier');
  assert.deepEqual(calls(page, 'chat_more'), [{ session_id: ID, before_id: 'u-0001-text-0' }], 'one request, for what is before the oldest shown');
  assert.equal(page.$('.an-earlier').hasAttribute('disabled'), true);
  page.emit('an:chat', patch(4, [user('older', 'an earlier message')], { order: ['older'].concat(state(page).order), has_earlier: 0 }));
  assert.equal(page.$('.an-earlier'), null);
  assert.equal(itemKeys(page)[0], 'older');
  clean(page);
});

test('scrolling: stick to the bottom only when already there; keep the reader\'s place when earlier items arrive', async () => {
  const page = await openChat({ chat: false });
  const scroll = page.$('#an-chat-scroll');
  scroll.__rect = { left: 0, top: 0, right: 440, bottom: 300, width: 440, height: 300 };
  Object.defineProperty(scroll, 'scrollHeight', { configurable: true, get: () => 100 * listEl(page).children.length });
  const items = (from, to) => { const out = []; for (let i = from; i <= to; i++) out.push(assistant('m' + i, 'message ' + i)); return out; };
  const order = (from, to) => items(from, to).map((i) => i.id);
  page.emit('an:chat', patch(1, items(10, 15), { reset: true, order: order(10, 15) }));
  assert.equal(scroll.scrollTop, 600, 'opens on the newest');
  page.emit('an:chat', patch(2, items(16, 16), { order: order(10, 16) }));
  assert.equal(scroll.scrollTop, 700, 'still at the bottom: follows');
  // the reader scrolls up
  scroll.scrollTop = 100;
  page.fire(scroll, 'scroll');
  page.emit('an:chat', patch(3, items(17, 17), { order: order(10, 17) }));
  assert.equal(scroll.scrollTop, 100, 'scrolled up: a new message does not pull the view');
  page.emit('an:chat', patch(4, items(8, 9), { order: order(8, 17) }));
  assert.equal(scroll.scrollTop, 300, 'two earlier messages came in above: the same message stays under the eye');
  // back to the bottom
  scroll.scrollTop = 10 * 100 - 300;
  page.fire(scroll, 'scroll');
  page.emit('an:chat', patch(5, items(18, 18), { order: order(8, 18) }));
  assert.equal(scroll.scrollTop, scroll.scrollHeight, 'the reader came back to the bottom: follows again');
  clean(page);
});

test('opening or closing a result does not move the reader to the bottom', async () => {
  const page = await openChat();
  const scroll = page.$('#an-chat-scroll');
  scroll.__rect = { left: 0, top: 0, right: 440, bottom: 300, width: 440, height: 300 };
  scroll.__scrollHeight = 2000;
  scroll.scrollTop = 500;
  page.fire(scroll, 'scroll');
  page.click(itemEl(page, 'toolu_read_1').querySelector('button'));
  assert.equal(scroll.scrollTop, 500);
  clean(page);
});

// ---- images -----------------------------------------------------------------------------------------------

const PIXEL = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==';

test('an image is a placeholder with its size until the reader asks; then chat_image, and an <img> of the data URL', async () => {
  const page = await openChat();
  const holder = itemEl(page, 'u-0003-image-0');
  assert.equal(parts(holder.querySelector('button')), 'Image (image/png) 47 KB');
  assert.equal(page.$$('img').length, 0);
  assert.equal(calls(page, 'chat_image').length, 0, 'nothing is fetched until asked');
  page.click(holder.querySelector('button'));
  assert.deepEqual(calls(page, 'chat_image'), [{ session_id: ID, image_id: 'u-0003-image-0' }]);
  assert.match(text(itemEl(page, 'u-0003-image-0')), /Loading…/);
  await page.settle();
  const img = itemEl(page, 'u-0003-image-0').querySelector('img');
  assert.equal(img.getAttribute('src'), PIXEL);
  assert.equal(img.getAttribute('alt'), 'Image (image/png)');
  assert.deepEqual(audit.problems(listEl(page)), [], 'the audit allows exactly this image source');
  page.emit('an:chat', patch(2, [assistant('x', 'more')], { order: state(page).order.concat('x') }));
  assert.equal(itemEl(page, 'u-0003-image-0').querySelector('img').getAttribute('src'), PIXEL, 'a patch keeps the loaded image');
  assert.equal(calls(page, 'chat_image').length, 1, 'and does not fetch it again');
  clean(page);
});

test('only image data URLs of png, jpeg, gif and webp are drawn; anything else stays a placeholder that can be retried', async () => {
  const bad = [
    'javascript:alert(1)',
    'data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==',
    'data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+',
    'https://evil.example/x.png',
    'data:image/png;base64,AAAA" onerror="alert(1)',
    'data:image/png,AAAA',
    'data:image/png;base64,' + 'A'.repeat(3 * 1024 * 1024 + 4),
    '',
    null,
  ];
  for (const url of bad) {
    const page = await openChat({ chat: false });
    page.hub.replies.chat_image = () => ({ data_url: url });
    page.emit('an:chat', chatFx());
    page.click(itemEl(page, 'u-0003-image-0').querySelector('button'));
    await page.settle();
    assert.equal(page.$$('img').length, 0, String(url).slice(0, 40));
    assert.match(text(itemEl(page, 'u-0003-image-0')), /Couldn’t load it\. Try again/);
    assert.equal(itemEl(page, 'u-0003-image-0').querySelector('button').hasAttribute('disabled'), false, 'and it can be asked again');
    clean(page);
  }
  for (const type of ['jpeg', 'gif', 'webp']) {
    const page = await openChat({ chat: false });
    page.hub.replies.chat_image = () => ({ data_url: `data:image/${type};base64,AAAA` });
    page.emit('an:chat', chatFx());
    page.click(itemEl(page, 'u-0003-image-0').querySelector('button'));
    await page.settle();
    assert.equal(page.$$('img').length, 1, type);
  }
});

test('a refused or failing chat_image is a placeholder; an answer that arrives after leaving the chat is dropped', async () => {
  const refused = await openChat({ chat: false });
  refused.hub.replies.chat_image = () => { throw { code: 'not_found', message: 'gone' }; };
  refused.emit('an:chat', chatFx());
  refused.click(itemEl(refused, 'u-0003-image-0').querySelector('button'));
  await refused.settle();
  assert.match(text(itemEl(refused, 'u-0003-image-0')), /Couldn’t load it/);
  assert.deepEqual(refused.errors.map(String), []);

  const late = await openChat({ chat: false });
  let release;
  late.hub.replies.chat_image = () => new Promise((resolve) => { release = () => resolve({ data_url: PIXEL }); });
  late.emit('an:chat', chatFx());
  late.click(itemEl(late, 'u-0003-image-0').querySelector('button'));
  late.click('[data-an-action="back"]');
  release();
  await late.settle();
  assert.deepEqual(late.errors.map(String), [], 'the answer for a chat that is gone is ignored');
  assert.equal(late.$$('img').length, 0);
});

test('an odd media type is text; sizes are read as they come; nothing is fetched by drawing', async () => {
  const page = await openChat();
  page.emit('an:chat', patch(2, [
    { id: 'img-evil', kind: 'image', media_type: '<img src=x onerror=alert(1)>', image_id: 'img-evil', bytes: 5 },
    { id: 'img-huge', kind: 'image', media_type: 'image/webp', image_id: 'img-huge', bytes: 3.5 * 1024 * 1024 },
    { id: 'img-none', kind: 'image', media_type: 'image/png', image_id: 'img-none' },
  ], { order: ['img-evil', 'img-huge', 'img-none'] }));
  assert.equal(parts(itemEl(page, 'img-evil').querySelector('button')), 'Image (<img src=x onerror=alert(1)>) 5 B');
  assert.equal(itemEl(page, 'img-evil').querySelectorAll('img').length, 0);
  assert.equal(parts(itemEl(page, 'img-huge').querySelector('button')), 'Image (image/webp) 3.5 MB');
  assert.equal(text(itemEl(page, 'img-none')), 'Image (image/png)');
  assert.deepEqual(audit.problems(listEl(page)), []);
  assert.equal(calls(page, 'chat_image').length, 0, 'drawing asks for nothing');
  clean(page);
});

// ---- links -----------------------------------------------------------------------------------------------------

test('a link in a message opens through open_url, https only', async () => {
  const page = await openChat();
  page.emit('an:chat', patch(2, [assistant('lk', 'See [the docs](https://example.com/a_(b)) and [plain](http://example.com) and [bad](javascript:alert(1)).')], { order: ['lk'] }));
  const links = page.$$('#an-chat-list .an-md-link');
  assert.equal(links.length, 1, 'only the https link is a link');
  page.click(links[0]);
  assert.deepEqual(calls(page, 'open_url'), [{ url: 'https://example.com/a_(b)' }]);
  // a forged attribute is refused by the chat as well
  for (const url of ['http://example.com', 'javascript:alert(1)', 'data:text/html,x', 'file:///C:/x', 'https://', 'https://a b', '']) {
    links[0].setAttribute('data-an-url', url);
    page.hub.clear();
    page.click(links[0]);
    assert.deepEqual(calls(page, 'open_url'), [], url);
  }
  clean(page);
});

test('Enter on a link opens it only once the keyboard gate is open, and is not the panel\'s Enter', async () => {
  const page = await openChat({ chat: false });
  page.emit('an:chat', patch(1, [assistant('lk', '[docs](https://example.com/x)')], { reset: true, order: ['lk'] }));
  const link = page.$('#an-chat-list .an-md-link');
  link.focus();
  const shut = page.key({ key: 'Enter' });
  assert.deepEqual(calls(page, 'open_url'), [], 'the gate is shut: no key does anything');
  assert.equal(shut.defaultPrevented, false);
  page.emit('an:panel_focus', { focused: true });
  link.focus();
  const open = page.key({ key: 'Enter' });
  assert.deepEqual(calls(page, 'open_url'), [{ url: 'https://example.com/x' }]);
  assert.equal(open.defaultPrevented, true);
  assert.deepEqual(calls(page, 'answer'), [], 'and never an answer');
  clean(page);
});

// ---- the header ------------------------------------------------------------------------------------------------------

test('the header: back, title, project, task summary, context and the terminal button', async () => {
  const page = await openChat();
  assert.equal(text(page.$('.an-ch-title')), 'Fix the login redirect loop');
  assert.equal(page.$('.an-ch-back').getAttribute('aria-label'), 'Back to sessions (Esc)');
  assert.equal(text(page.$('.an-ch-subt')), 'acme-web');
  assert.equal(page.$('.an-ch-act'), null, 'the project is quiet, not activity');
  assert.equal(text(page.$('.an-ch-tasks .an-tcount')), '3/7');
  assert.equal(page.$('.an-ch-tasks').getAttribute('title'), 'Now: Running the auth test suite');
  assert.equal(text(page.$('.an-ch-right .an-ctx')), '42%');
  const focus = page.$('.an-ch-focus');
  assert.equal(focus.getAttribute('aria-label'), 'Show terminal (Ctrl+J)');
  page.click(focus);
  assert.deepEqual(calls(page, 'focus'), [{ session_id: ID }]);
  clean(page);
});

test('the account tag appears only when several accounts are in use', async () => {
  const multi = await openChat();
  assert.equal(text(multi.$('.an-ch-sub .an-acct')), 'Work');
  assert.ok(multi.$('.an-ch-sub .an-dotsep'));
  const single = await openChat({ edit: (s) => { s.accounts_multi = false; } });
  assert.equal(single.$('.an-ch-sub .an-acct'), null);
  assert.equal(single.$('.an-ch-sub .an-dotsep'), null);
});

test('a working session\'s subtitle is the task it is on; a background count follows the project', async () => {
  const page = await openChat({ id: 'work-migration' });
  assert.equal(text(page.$('.an-ch-subt')), 'Writing tests for the v2 schema');
  assert.ok(page.$('.an-ch-act'), 'in primary ink');
  assert.equal(text(page.$('.an-ch-tasks .an-tcount')), '2/6');
  const bg = await openChat({ edit: (s) => { s.sessions.find((r) => r.session_id === ID).background_count = 2; } });
  assert.equal(text(bg.$('.an-ch-subt')), 'acme-web · 2 background');
});

test('"Show in editor" names what the row says; a session with nowhere to jump has no button; no tasks, no summary; no context, no meter', async () => {
  const page = await openChat({ id: 'work-ci' });
  assert.equal(page.$('.an-ch-focus').getAttribute('aria-label'), 'Show in editor (Ctrl+J)');
  const bare = await openChat({ edit: (s) => { const r = s.sessions.find((x) => x.session_id === ID); r.focus_label = null; r.tasks = null; r.context_pct = null; } });
  assert.equal(bare.$('.an-ch-focus'), null);
  assert.equal(bare.$('.an-ch-tasks'), null);
  assert.equal(bare.$('.an-ch-right .an-ctx'), null);
});

test('the context meter\'s levels (B_MovedChatSettingsTests contextMeterLevels): normal, high from 80, critical from 90', async () => {
  const level = async (pct) => {
    const page = await openChat({ edit: (s) => { s.sessions.find((r) => r.session_id === ID).context_pct = pct; } });
    return page.$('.an-ch-right .an-ctx').getAttribute('class').match(/an-ctx-(\w+)/)[1];
  };
  assert.equal(await level(42), 'normal');
  assert.equal(await level(80), 'high');
  assert.equal(await level(89.9), 'high');
  assert.equal(await level(90), 'critical');
});

test('a session that has left the list keeps the last header the chat saw', async () => {
  const page = await openChat();
  const snapshot = harness.fixture('snapshot.json');
  snapshot.sessions = snapshot.sessions.filter((r) => r.session_id !== ID);
  snapshot.generated_at_ms += 10;
  page.emit('an:snapshot', snapshot);
  assert.equal(text(page.$('.an-ch-title')), 'Fix the login redirect loop');
  const blank = harness.loadPage('agentnotch/panel.html', { before: (window) => { window.__AGENTNOTCH_PANEL__ = { route: 'session:nobody' }; } });
  await blank.settle();
  assert.equal(text(blank.$('.an-ch-title')), 'Session', 'an unknown session has a plain header');
  assert.equal(blank.$('.an-ch-focus'), null);
  assert.deepEqual(blank.errors.map(String), []);
});

// ---- the task board ------------------------------------------------------------------------------------------------------

test('the task summary opens the board: every task with its state, the count, and closes again', async () => {
  const page = await openChat({ id: 'work-migration' });
  assert.equal(page.$('.an-board'), null);
  const button = page.$('.an-ch-tasks');
  assert.equal(button.getAttribute('aria-expanded'), 'false');
  page.click(button);
  const board = page.$('.an-board');
  assert.ok(board);
  assert.equal(parts(board.querySelector('.an-board-head')), 'Tasks 2 of 6 done');
  const rows = board.querySelectorAll('.an-trow');
  assert.deepEqual(rows.map((r) => r.getAttribute('class').match(/an-trow-(\w+)/)[1]), ['completed', 'completed', 'in_progress', 'pending', 'pending', 'pending']);
  assert.equal(text(rows[2]), 'Writing tests for the v2 schema');
  assert.match(rows[0].querySelector('.an-tmark').getAttribute('class'), /an-tmark-done/);
  assert.match(rows[2].querySelector('.an-tmark').getAttribute('class'), /an-spin/, 'the one in progress turns');
  assert.match(rows[3].querySelector('.an-tmark').getAttribute('class'), /an-tmark-todo/);
  assert.equal(page.$('.an-ch-tasks').getAttribute('aria-expanded'), 'true');
  assert.equal(board.querySelector('.an-board-list').style.maxHeight, '152px', 'eight rows of 17 and a gap of 2, then it scrolls');
  page.click('.an-ch-tasks');
  assert.equal(page.$('.an-board'), null);
  clean(page);
});

test('the board keeps its place in a header that a snapshot redraws', async () => {
  const page = await openChat({ id: 'work-migration' });
  page.click('.an-ch-tasks');
  const board = page.$('.an-board');
  const snapshot = harness.fixture('snapshot.json');
  snapshot.generated_at_ms += 10;
  page.emit('an:snapshot', snapshot);
  assert.equal(page.$('.an-board'), board, 'the same element');
});

test('the board holds four rows while an answer bar shows, and closes when a request appears', async () => {
  const page = await openChat({ id: ID });
  page.click('.an-ch-tasks');
  assert.equal(page.$('.an-board-list').style.maxHeight, '76px', 'a request is pending: four rows');
  // the request is answered elsewhere and the board reopened: the regular room
  const snapshot = harness.fixture('snapshot.json');
  snapshot.generated_at_ms += 10;
  snapshot.sessions.find((r) => r.session_id === ID).pending = null;
  page.emit('an:snapshot', snapshot);
  assert.equal(page.$('.an-board-list').style.maxHeight, '152px');
  // a new request arrives: the board closes for it
  const again = harness.fixture('snapshot.json');
  again.generated_at_ms += 20;
  again.sessions.find((r) => r.session_id === ID).pending.tool_use_id = 'toolu_new';
  page.emit('an:snapshot', again);
  assert.equal(page.$('.an-board'), null);
  clean(page);
});

test('the board\'s strings are text: a hostile task label and a 10 000-character one', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await openChat({
      id: 'work-migration',
      edit: (s) => { const t = s.sessions.find((r) => r.session_id === 'work-migration').tasks; t.items[2].label = evil; t.active_label = evil; },
    });
    page.click('.an-ch-tasks');
    assert.deepEqual(audit.problems(page.$('#an-chat-head')), [], evil.slice(0, 30));
    assert.ok(text(page.$('.an-board')).length > 0);
    assert.ok(text(page.$('.an-board')).length < 400, 'a long label is cut');
  }
});

// ---- the window ---------------------------------------------------------------------------------------------------------

test('the chat asks for header + content + bar, at most 780 high, and again when the content changes', async () => {
  const page = await openChat();
  const rect = (el, w, h) => { el.__rect = { left: 0, top: 0, right: w, bottom: h, width: w, height: h }; };
  rect(page.$('#an-card'), 440, 0);
  rect(page.$('#an-chat-head'), 440, 60);
  rect(page.$('#an-chat-list'), 440, 300);
  page.hub.clear();
  page.emit('an:chat', patch(2, [assistant('m', 'x')], { order: state(page).order.concat('m') }));
  assert.deepEqual(calls(page, 'panel_report_size').pop(), { w: 440, h: 60 + 1 + 300 });
  rect(page.$('#an-chat-list'), 440, 3000);
  page.emit('an:chat', patch(3, [assistant('n', 'x')], { order: state(page).order.concat('n') }));
  assert.deepEqual(calls(page, 'panel_report_size').pop(), { w: 440, h: 780 }, 'capped');
  clean(page);
});

// ---- hostile strings -------------------------------------------------------------------------------------------------------

function hostileItems(evil) {
  const result = (r) => Object.assign({ tool: 'generic' }, r);
  return [
    user('h-user', evil),
    assistant('h-asst', evil),
    { id: 'h-think', kind: 'thinking', text: evil },
    { id: 'h-tool-name', kind: 'tool', name: evil, summary: evil, status: 'success', input: { description: evil, command: evil, file_path: evil }, result: result({ text: evil }), subagent: null },
    { id: 'h-bash', kind: 'tool', name: 'Bash', summary: evil, status: 'success', input: { command: evil, description: evil }, result: { tool: 'bash', stdout: evil, stderr: evil, interrupted: false, return_code_interpretation: evil, background_task_id: evil }, subagent: null },
    { id: 'h-read', kind: 'tool', name: 'Read', summary: evil, status: 'success', input: { file_path: evil }, result: { tool: 'read', file_path: evil, content: evil, num_lines: 1, start_line: 1, total_lines: 1 }, subagent: null },
    { id: 'h-edit', kind: 'tool', name: 'Edit', summary: evil, status: 'error', input: { file_path: evil, old_string: evil, new_string: evil + 'x' }, result: { tool: 'edit', file_path: evil, replace_all: false, user_modified: true, diff: [{ kind: 'add', text: evil, old_line: null, new_line: 1 }, { kind: evil, text: evil, old_line: evil, new_line: evil }] }, subagent: null },
    { id: 'h-grep', kind: 'tool', name: 'Grep', summary: evil, status: 'success', input: { pattern: evil }, result: { tool: 'grep', mode: 'files', filenames: [evil], num_files: 1, content: evil }, subagent: null },
    { id: 'h-fetch', kind: 'tool', name: 'WebFetch', summary: evil, status: 'success', input: { url: evil }, result: { tool: 'web_fetch', url: evil, code: 200, code_text: evil, result: evil }, subagent: null },
    { id: 'h-search', kind: 'tool', name: 'WebSearch', summary: evil, status: 'success', input: { query: evil }, result: { tool: 'web_search', query: evil, results: [{ title: evil, url: evil, snippet: evil }] }, subagent: null },
    { id: 'h-todo', kind: 'tool', name: 'TodoWrite', summary: evil, status: 'success', input: {}, result: { tool: 'todo', items: [{ content: evil, status: evil, active_form: evil }] }, subagent: null },
    { id: 'h-ask', kind: 'tool', name: 'AskUserQuestion', summary: evil, status: 'success', input: {}, result: { tool: 'ask_user_question', questions: [{ text: evil, header: evil, options: [{ label: evil, description: evil }] }], answers: { [evil]: evil } }, subagent: null },
    { id: 'h-mcp', kind: 'tool', name: 'mcp__' + evil + '__x', summary: evil, status: 'success', input: { [evil]: evil }, result: { tool: 'mcp', server_name: evil, tool_name: evil, raw: { [evil]: evil } }, subagent: null },
    { id: 'h-task', kind: 'tool', name: 'Task', summary: evil, status: 'running', input: { description: evil }, result: { tool: 'task', agent_id: evil, status: evil, content: evil }, subagent: { agent_id: evil, description: evil, tools: [{ id: evil, name: evil, summary: evil, status: evil }] } },
    { id: 'h-img', kind: 'image', media_type: evil, image_id: evil, bytes: evil },
    { id: evil, kind: 'assistant', text: 'the id is hostile' },
  ];
}

test('hostile strings through every field a transcript carries draw as text, nothing else', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await openChat({ chat: false });
    const items = hostileItems(evil);
    page.emit('an:chat', patch(1, items, { reset: true, order: items.map((i) => i.id), working: evil }));
    // open every result the reader can open
    for (const b of page.$$('#an-chat-list button[data-an-chat="toggle"]')) page.click(b);
    assert.deepEqual(audit.problems(listEl(page)), [], 'list: ' + evil.slice(0, 30));
    assert.deepEqual(page.errors.map(String), [], 'no error: ' + evil.slice(0, 30));
    // the ids went into attributes; they come back as the same id
    const keys = listEl(page).children.map((el) => el.getAttribute('data-key')).filter((k) => k && k.startsWith('i:'));
    assert.ok(keys.includes('i:' + evil), 'the hostile id is an attribute value, intact');
    assert.equal(page.$$('#an-chat-list img').length, 0);
    assert.equal(page.$$('#an-chat-list script, #an-chat-list iframe, #an-chat-list style, #an-chat-list base').length, 0);
    // the image button asks for exactly the id it was given
    const button = itemEl(page, 'h-img').querySelector('button');
    page.click(button);
    assert.deepEqual(calls(page, 'chat_image'), [{ session_id: ID, image_id: 'h-img' }]);
  }
});

test('hostile strings in the header', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await openChat({
      edit: (s) => { const r = s.sessions.find((x) => x.session_id === ID); r.title = evil; r.project = evil; r.account_label = evil; r.focus_label = evil; r.tasks.active_label = evil; },
    });
    assert.deepEqual(audit.problems(page.$('#an-chat-head')), [], evil.slice(0, 30));
    assert.deepEqual(page.errors.map(String), []);
    assert.ok(text(page.$('.an-ch-title')).length <= 300, 'a long title is cut');
    assert.ok(page.$('.an-ch-focus').getAttribute('aria-label').length < 70);
  }
});

test('a 10 000-character line in every text a transcript draws: bounded where it should be, whole where it is the message', async () => {
  const page = await openChat({ chat: false });
  const long = 'W'.repeat(10000) + ' ' + 'x'.repeat(10000);
  const items = [
    user('l-user', long), assistant('l-asst', long),
    { id: 'l-think', kind: 'thinking', text: long },
    { id: 'l-tool', kind: 'tool', name: long, summary: long, status: 'success', input: {}, result: { tool: 'generic', text: long }, subagent: null },
    { id: 'l-bash', kind: 'tool', name: 'Bash', summary: 'go', status: 'success', input: {}, result: { tool: 'bash', stdout: long, stderr: '', interrupted: false, return_code_interpretation: null, background_task_id: null }, subagent: null },
  ];
  page.emit('an:chat', patch(1, items, { reset: true, order: items.map((i) => i.id), working: long }));
  assert.equal(text(itemEl(page, 'l-user')).length, long.length, 'what the user wrote is shown whole (the box wraps it)');
  assert.equal(text(itemEl(page, 'l-think')).length, 91, 'thinking is one line of 90 characters and a mark');
  assert.ok(text(itemEl(page, 'l-tool').querySelector('.an-tool-name')).length <= 120);
  assert.ok(text(itemEl(page, 'l-tool').querySelector('.an-tool-sum')).length <= 400);
  assert.ok(text(page.$('.an-working')).length <= 200);
  page.click(itemEl(page, 'l-bash').querySelector('button'));
  assert.ok(text(itemEl(page, 'l-bash').querySelector('.an-tool-result')).length > 0);
  assert.deepEqual(page.errors.map(String), []);
});

// ---- the stylesheet ------------------------------------------------------------------------------------------------------------

test('the chat\'s CSS: tokens only, code scrolls in its own box, links and marks', () => {
  const start = CSS.indexOf('the chat screen (chat.js');
  assert.ok(start > 0);
  const chat = CSS.slice(start);
  assert.doesNotMatch(chat.replace(/\/\*[\s\S]*?\*\//g, ''), /#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\(/, 'no literal colour: the tokens of theme.css');
  assert.match(chat, /\.an-code-body \{[^}]*overflow-x: auto/, 'a long line scrolls in its box');
  assert.match(chat, /\.an-md-pre \{[^}]*overflow-x: auto/);
  assert.match(chat, /\.an-chat \.an-code \{[^}]*-webkit-line-clamp: unset/, 'the row\'s clamped code box is reset inside the chat');
  assert.match(chat, /\.an-chat-scroll \{[^}]*overflow-x: hidden[^}]*overflow-y: auto/);
  assert.match(chat, /\.an-chat-list \{[^}]*margin-top: auto/, 'bottom-anchored');
  assert.match(chat, /\.an-md-link \{[^}]*var\(--an-accent\)/);
  assert.match(chat, /\.an-diff-add \{[^}]*var\(--an-review\) 12%/);
  assert.match(chat, /\.an-diff-remove \{[^}]*var\(--an-critical\) 12%/);
  assert.match(chat, /\.an-status-read \{[^}]*min-width: 0/, 'the status words can shrink');
  assert.match(chat, /\.an-status-t \{[^}]*overflow-wrap: anywhere/, 'the status words wrap rather than clip');
  assert.match(chat, /\.an-status-err \.an-status-t \{[^}]*var\(--an-critical\)/);
  assert.doesNotMatch(chat, /animation:[^;]*infinite/, 'the marks\' stepped arc is theme.css\'s, nothing here loops');
});

// ---- scenes --------------------------------------------------------------------------------------------------------------------

test('the chat scenes: chat-approval shows the fixture\'s session, chat-tasks opens the board', async () => {
  const page = await openChat({ id: 'work-migration', chat: false });
  page.emit('an:chat', patch(1, [user('u', 'Write migration tests'), assistant('a', 'On it.')], { reset: true, order: ['u', 'a'], working: 'Working…', session_id: 'work-migration' }));
  assert.equal(page.run("agentnotchPanel.showScene('chat-tasks')"), true);
  assert.equal(page.run('agentnotchPanel._.state.route'), 'session:work-migration');
  assert.ok(page.$('.an-board'), 'the board is open');
  assert.equal(text(page.$('.an-working')), 'Working…');
  assert.equal(page.hub.of('chat_open').length, 1, 'showing the scene did not leave and re-enter the chat');
  assert.equal(page.run("agentnotchPanel.showScene('chat-approval')"), true);
  assert.equal(page.run('agentnotchPanel._.state.route'), 'session:needs-permission');
  assert.equal(page.$('#an-card').getAttribute('data-mode'), 'chat');
  assert.equal(page.$('.an-board'), null);
  assert.equal(page.run("agentnotchPanel.showScene('panel-every-state')"), true);
  assert.equal(page.$('#an-card').getAttribute('data-mode'), 'list');
  const report = plain(page.run('agentnotchPanel.layoutReport()'));
  assert.equal(report.ok, true);
});
