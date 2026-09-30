'use strict';
// The pages' call table (lib/contract.cjs) against the engine's own files: every fixture
// example, every method of hub/api.rs, the window gate of DESIGN-WIN §3.7 and the events.
// api.rs is read as text, so a method added there without a table entry fails here.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const contract = require('./lib/contract.cjs');

const ENGINE_SRC = path.join(contract.WINDOWS, 'agentnotch-engine', 'src');
const api = fs.readFileSync(path.join(ENGINE_SRC, 'hub', 'api.rs'), 'utf8');
const settingsRs = fs.readFileSync(path.join(ENGINE_SRC, 'core', 'settings.rs'), 'utf8');

const snake = (name) => name.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();

/** `pub const NAME: [&str; N] = ["a", "b"];` → ['a', 'b']. */
function rustStrings(name) {
  const m = new RegExp(`pub const ${name}: \\[&str; \\d+\\] = \\[([^\\]]*)\\]`).exec(api);
  assert.ok(m, `${name} not found in api.rs`);
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

/** The variant names of `pub enum Call`, as method names. */
function callVariants() {
  const start = api.indexOf('pub enum Call {');
  assert.ok(start >= 0, 'pub enum Call not found in api.rs');
  const end = api.indexOf('\n}\n', start);
  const body = api.slice(start, end);
  return [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*)\b/gm)].map((m) => snake(m[1]));
}

const calls = contract.fixture('calls.json');

test('api.rs parses: the Call enum and both glue lists were found', () => {
  assert.ok(callVariants().length >= 30);
  assert.equal(rustStrings('GLUE_METHODS').length, 14);
  assert.deepEqual(rustStrings('GLUE_ONLY_METHODS'), ['panel_state', 'hotkey_status']);
});

test('every calls.json example is accepted by the table, except the ones the engine refuses', () => {
  assert.equal(calls.length, 47);
  for (const entry of calls) {
    const { method, args } = entry.call;
    const problem = contract.callProblem(method, args);
    if (entry.error === 'invalid') {
      assert.ok(problem, `${method} ${JSON.stringify(args)} is an "invalid" example but the table accepts it`);
    } else {
      assert.equal(problem, null, `${method} ${JSON.stringify(args)}: ${problem}`);
    }
  }
});

test('every example has a reply, a reply fixture, a reply_contains or an error', () => {
  for (const entry of calls) {
    const kinds = ['reply', 'reply_fixture', 'reply_contains', 'error'].filter((k) => entry[k] !== undefined);
    assert.equal(kinds.length, 1, `${entry.call.method}: ${kinds.join(',') || 'nothing'}`);
    if (entry.reply_fixture) assert.doesNotThrow(() => contract.fixture(entry.reply_fixture));
  }
});

test('every engine method of api.rs is in the table, and nothing else is', () => {
  const glue = new Set(rustStrings('GLUE_METHODS'));
  const engine = callVariants();
  assert.deepEqual([...new Set(engine)].sort(), engine.slice().sort(), 'a variant name appears twice');
  assert.deepEqual(Object.keys(contract.ENGINE).sort(), engine.slice().sort());
  for (const method of engine) assert.ok(!glue.has(method), `${method} is both an engine and a glue method`);
});

test('every glue method of api.rs is in the table, and nothing else is', () => {
  assert.deepEqual(Object.keys(contract.GLUE).sort(), rustStrings('GLUE_METHODS').slice().sort());
});

test('the glue-only calls are the engine lists and are refused from every window', () => {
  assert.deepEqual(contract.GLUE_ONLY.slice().sort(), rustStrings('GLUE_ONLY_METHODS').slice().sort());
  for (const label of ['agentnotch-panel', 'notch', 'settings', 'dropzones']) {
    for (const method of contract.GLUE_ONLY) assert.equal(contract.allowedFromWindow(label, method), false, `${label} ${method}`);
  }
});

test('calls.json covers every engine method and every glue-only call', () => {
  const covered = new Set(calls.map((e) => e.call.method));
  for (const method of callVariants()) assert.ok(covered.has(method), `no example for ${method}`);
  for (const method of rustStrings('GLUE_ONLY_METHODS')) assert.ok(covered.has(method), `no example for ${method}`);
});

test('every account action and cloud action of the engine is in the table', () => {
  const src = api.slice(api.indexOf('pub enum CloudAction'), api.indexOf('pub enum CloudUrlTarget'));
  const actions = [...src.matchAll(/^ {4}([A-Z][A-Za-z]*),/gm)].map((m) => snake(m[1]));
  assert.deepEqual(contract.ENGINE.cloud.action.slice(1).sort(), actions.sort());
  const sent = new Set(calls.filter((e) => e.call.method === 'account').map((e) => Object.keys(e.call.args.action)[0]));
  for (const one of contract.ENGINE.account.action.slice(1)) {
    assert.ok(sent.has(Object.keys(one)[0]), `no calls.json example for account.${Object.keys(one)[0]}`);
  }
});

// ---- §3.7 -----------------------------------------------------------------------------------

const ALL_METHODS = [...Object.keys(contract.ENGINE), ...Object.keys(contract.GLUE)];

test('the sessions panel may make every call but the glue-only ones', () => {
  for (const method of ALL_METHODS) {
    assert.equal(contract.allowedFromWindow('agentnotch-panel', method), !contract.GLUE_ONLY.includes(method), method);
  }
});

test('the notch may read the snapshot, refresh usage, jump, mark reviewed, drive the panel, open Settings and log', () => {
  // §3.7 plus `open_settings`, which LEAD-NOTES allows from the notch (the gear on the card).
  const allowed = new Set(['snapshot', 'refresh_usage', 'focus', 'mark_reviewed', 'open_settings', 'log',
    'panel_toggle', 'panel_open', 'panel_close', 'panel_route', 'panel_report_size', 'panel_take_focus', 'panel_engaged']);
  for (const method of ALL_METHODS) {
    assert.equal(contract.allowedFromWindow('notch', method), allowed.has(method), method);
  }
  // Nothing that answers, types or changes what Claude Code is allowed to do.
  for (const method of ['answer', 'send_message', 'set_setting', 'hook_consent', 'hooks_enabled', 'open_url', 'copy_text']) {
    assert.equal(contract.allowedFromWindow('notch', method), false, method);
  }
});

test('Settings may make every call but answering, typing and the glue-only ones', () => {
  for (const method of ALL_METHODS) {
    const expected = !['answer', 'send_message', 'panel_state', 'hotkey_status'].includes(method);
    assert.equal(contract.allowedFromWindow('settings', method), expected, method);
  }
});

test('the drop zones and unknown windows may make no call', () => {
  for (const label of ['dropzones', 'unknown', '']) {
    for (const method of ALL_METHODS) assert.equal(contract.allowedFromWindow(label, method), false, `${label} ${method}`);
  }
});

test('the notch rule in the table is the rule in api.rs', () => {
  const rule = /"notch" => \{\s*matches!\(\s*method,\s*([^)]*?)\s*\)\s*\|\|\s*method\.starts_with\("panel_"\)/.exec(api);
  assert.ok(rule, 'the notch arm of allowed_from_window was not found');
  const names = [...rule[1].matchAll(/"([a-z_]+)"/g)].map((m) => m[1]).sort();
  assert.deepEqual(names, ['focus', 'log', 'mark_reviewed', 'open_settings', 'refresh_usage', 'snapshot']);
  assert.match(api, /"settings" => !matches!\(method, "answer" \| "send_message"\)/);
});

// ---- the table refuses what the engine would ------------------------------------------------

test('callProblem refuses an unknown method, a wrong type, a missing or an extra argument', () => {
  assert.match(contract.callProblem('nope', {}), /unknown method/);
  assert.match(contract.callProblem('focus', { session_id: 5 }), /not a string/);
  assert.match(contract.callProblem('focus', {}), /missing/);
  assert.match(contract.callProblem('focus', { session_id: 'a', extra: 1 }), /unknown key/);
  assert.match(contract.callProblem('mark_reviewed', { session_id: 'a' }), /at_ms/);
  assert.match(contract.callProblem('refresh_usage', { reason: 'sometimes' }), /not one of/);
  assert.match(contract.callProblem('reveal', { kind: 'path', id: 'C:\\x' }), /not one of/);
  assert.match(contract.callProblem('answer', { session_id: 's', tool_use_id: 't', answer: 'yes' }), /answer/);
});

test('methods without arguments take none, null or {}', () => {
  for (const method of ['snapshot', 'settings', 'reset_review_queue', 'acknowledge_scope', 'session_state_text', 'panel_close', 'pick_folder']) {
    assert.equal(contract.callProblem(method, undefined), null, method);
    assert.equal(contract.callProblem(method, null), null, method);
    assert.equal(contract.callProblem(method, {}), null, method);
    assert.match(contract.callProblem(method, { a: 1 }), /unknown key/);
  }
});

test('optional arguments may be null or absent, never another type', () => {
  assert.equal(contract.callProblem('refresh_usage', { reason: 'manual' }), null);
  assert.equal(contract.callProblem('refresh_usage', { reason: 'manual', ring_id: null }), null);
  assert.match(contract.callProblem('refresh_usage', { reason: 'manual', ring_id: 3 }), /not a string/);
  assert.equal(contract.callProblem('choose_claude_binary', { path: null }), null);
  assert.equal(contract.callProblem('cloud', { action: 'sign_in' }), null);
});

test('every answer form is accepted and a malformed one is not', () => {
  const a = (answer) => contract.callProblem('answer', { session_id: 's', tool_use_id: 't', answer });
  assert.equal(a({ allow: { always: true } }), null);
  assert.equal(a({ deny: { reason: null } }), null);
  assert.equal(a({ deny: { reason: 'no' } }), null);
  assert.equal(a({ questions: { answers: { 'Which?': 'A' } } }), null);
  assert.equal(a('approve_plan'), null);
  assert.equal(a('keep_planning'), null);
  assert.notEqual(a({ allow: {} }), null);
  assert.notEqual(a({ allow: { always: 'yes' } }), null);
  assert.notEqual(a({ questions: { answers: { 'Which?': 3 } } }), null);
  assert.notEqual(a({ questions: {} }), null);
  assert.notEqual(a('allow'), null);
  assert.notEqual(a(null), null);
});

test('set_setting takes only the settings of §4.12 and their choices', () => {
  const set = (key, value) => contract.callProblem('set_setting', { key, value });
  assert.equal(set('autoOpen', 'needsInputOrDone'), null);
  assert.equal(set('peekSeconds', 10), null);
  assert.equal(set('typeReplies', true), null);
  assert.match(set('peekSeconds', 7), /not one of/);
  assert.match(set('autoOpen', 'sometimes'), /not one of/);
  assert.match(set('sound', 'on'), /not a boolean/);
  assert.match(set('noSuchSetting', true), /unknown key/);
  // Consent and the cloud switches have their own calls; set_setting never writes them.
  for (const key of ['hookConsent', 'hooksEnabled', 'cloudSyncEnabled', 'cloudSummariesEnabled', 'cloudDeviceId', 'claudeBinaryPath', 'statusLineIntegration']) {
    assert.match(set(key, true), /unknown key/, key);
  }
});

test('the settings a page may set are the engine\'s keys, with its choices', () => {
  const keys = new Map([...settingsRs.matchAll(/pub const [A-Z_]+: &str = "([A-Za-z]+)";/g)].map((m) => [m[1], true]));
  for (const key of Object.keys(contract.SETTINGS)) assert.ok(keys.has(key), `${key} is not a settings key of the engine`);
  const choices = { autoOpen: 'AUTO_OPEN', holdOpenWhileNeedsYou: 'HOLD_OPEN', ringClick: 'RING_CLICK', sessionClick: 'SESSION_CLICK', hotKey: 'HOT_KEY', peekSeconds: 'PEEK_SECONDS' };
  for (const [key, name] of Object.entries(choices)) {
    const m = new RegExp(`pub const ${name}: \\[[^\\]]*\\] = \\[([^\\]]*)\\]`).exec(settingsRs);
    assert.ok(m, name);
    const engine = m[1].split(',').map((s) => s.trim().replace(/"/g, '')).filter(Boolean).map((s) => (/^\d+$/.test(s) ? Number(s) : s));
    assert.deepEqual(contract.SETTINGS[key].slice(1).slice().sort(), engine.slice().sort(), key);
  }
});

// ---- events ---------------------------------------------------------------------------------

const KNOWN_EVENTS = ['an:snapshot', 'an:settings', 'an:cloud', 'an:chat', 'an:panel', 'an:panel_focus', 'an:peek', 'an:notice', 'usage'];

test('every event of events.json is one the pages know, and api.rs emits the fork\'s', () => {
  const events = contract.fixture('events.json');
  assert.ok(events.length >= 10);
  for (const entry of events) {
    assert.ok(KNOWN_EVENTS.includes(entry.event), `unknown event ${entry.event}`);
    if (entry.payload_fixture !== undefined) assert.doesNotThrow(() => contract.fixture(entry.payload_fixture));
    else assert.notEqual(entry.payload, undefined, `${entry.event} has no payload`);
    if (entry.event.startsWith('an:')) assert.ok(api.includes(`"${entry.event}"`), `${entry.event} is not in api.rs`);
  }
  for (const name of KNOWN_EVENTS) assert.ok(events.some((e) => e.event === name), `no fixture for ${name}`);
});

test('the event payloads have the shape the pages read', () => {
  const byEvent = (name) => contract.fixture('events.json').filter((e) => e.event === name);
  const [focus] = byEvent('an:panel_focus');
  assert.equal(typeof focus.payload.focused, 'boolean');
  const [peek] = byEvent('an:peek');
  assert.equal(typeof peek.payload.ring_id, 'string');
  assert.equal(typeof peek.payload.seconds, 'number');
  const [panel] = byEvent('an:panel');
  assert.match(panel.payload.route, /^(sessions|session:.+)$/);
  const [notice] = byEvent('an:notice');
  assert.equal(typeof notice.payload, 'string');
  const patch = byEvent('an:chat').find((e) => e.payload);
  assert.equal(patch.payload.reset, false);
  assert.equal(typeof patch.payload.revision, 'number');
  assert.ok(Array.isArray(patch.payload.items));
});

// ---- the scene list -------------------------------------------------------------------------------

test('scenes.json lists each scene once, on a page that exists, with a known edge and event', () => {
  const list = JSON.parse(fs.readFileSync(path.join(__dirname, 'scenes.json'), 'utf8')).scenes;
  const ui = path.join(contract.WINDOWS, 'codenotch', 'ui');
  const names = new Set();
  const events = contract.fixture('events.json').map((e) => e.event);
  for (const scene of list) {
    assert.match(scene.name, /^[a-z0-9]+(-[a-z0-9]+)*$/, scene.name);
    assert.ok(!names.has(scene.name), `${scene.name} is listed twice`);
    names.add(scene.name);
    assert.ok(fs.existsSync(path.join(ui, scene.page)), `${scene.name}: ${scene.page} does not exist`);
    if (scene.edge !== undefined) assert.ok(['right', 'left', 'top', 'bottom'].includes(scene.edge), `${scene.name}: edge`);
    if (scene.scene !== undefined) assert.ok(scene.global, `${scene.name}: a scene needs the global that shows it`);
    assert.ok(scene.widths === true || Number(scene.width) > 0, `${scene.name}: a fixed width or widths: true`);
    for (const ev of scene.events || []) assert.ok(events.includes(ev.event), `${scene.name}: unknown event ${ev.event}`);
    for (const method of Object.keys(scene.replies || {})) assert.ok(method in contract.ENGINE || method in contract.GLUE, `${scene.name}: unknown method ${method}`);
  }
  assert.ok(list.length >= 12);
});
