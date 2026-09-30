'use strict';
// The pages' side of the app's one command, `an_call` (DESIGN-WIN §3.5, §3.7), written out so
// the tests can hold every call a page makes against it: the method exists, its arguments have
// the engine's names and types, and the window it comes from may make it.
//
// The engine's half is `hub::Call` in agentnotch-engine; `tests/ui-contract/calls.json` holds
// one example per method, which the engine's own test deserialises. `contract.test.cjs` checks
// this table against that file (every example is accepted, every method is covered), and
// against the engine's lists of glue methods, so the two cannot drift apart unnoticed.

const fs = require('node:fs');
const path = require('node:path');

const WINDOWS = path.join(__dirname, '..', '..');
const CONTRACT_DIR = path.join(WINDOWS, 'agentnotch-engine', 'tests', 'ui-contract');

/** A ui-contract fixture, parsed fresh (tests may change their copy). */
function fixture(name) {
  return JSON.parse(fs.readFileSync(path.join(CONTRACT_DIR, name), 'utf8'));
}

// ---- a very small schema language -------------------------------------------------------------
// 'string' | 'number' | 'boolean' | 'any'; a trailing '?' allows null or absence;
// {key: schema} an object with exactly those keys (optional ones may be absent);
// ['array', schema]; ['map', schema] (any keys); ['enum', ...values]; ['one', ...schemas].

function check(value, schema, where) {
  if (typeof schema === 'string') {
    const optional = schema.endsWith('?');
    const type = optional ? schema.slice(0, -1) : schema;
    if (value === undefined || value === null) return optional ? null : `${where}: missing (${type})`;
    if (type === 'any') return null;
    return typeof value === type ? null : `${where}: ${JSON.stringify(value)} is not a ${type}`;
  }
  if (Array.isArray(schema)) {
    const [kind, ...rest] = schema;
    if (kind === 'enum') return rest.includes(value) ? null : `${where}: ${JSON.stringify(value)} is not one of ${rest.join(', ')}`;
    if (kind === 'array') {
      if (!Array.isArray(value)) return `${where}: not an array`;
      for (let i = 0; i < value.length; i++) {
        const problem = check(value[i], rest[0], `${where}[${i}]`);
        if (problem) return problem;
      }
      return null;
    }
    if (kind === 'map') {
      if (!value || typeof value !== 'object' || Array.isArray(value)) return `${where}: not an object`;
      for (const key of Object.keys(value)) {
        const problem = check(value[key], rest[0], `${where}.${key}`);
        if (problem) return problem;
      }
      return null;
    }
    if (kind === 'one') {
      const problems = rest.map((s) => check(value, s, where));
      return problems.includes(null) ? null : problems.join(' | ');
    }
    throw new Error(`unknown schema kind ${kind}`);
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return `${where}: not an object`;
  for (const key of Object.keys(value)) {
    if (!(key in schema)) return `${where}: unknown key "${key}"`;
  }
  for (const key of Object.keys(schema)) {
    const problem = check(value[key], schema[key], `${where}.${key}`);
    if (problem) return problem;
  }
  return null;
}

const ANSWER = ['one',
  { allow: { always: 'boolean' } },
  { deny: { reason: 'string?' } },
  { questions: { answers: ['map', 'string'] } },
  ['enum', 'approve_plan', 'keep_planning'],
];

const ACCOUNT_ACTION = ['one',
  { rename: { id: 'string', label: 'string?' } },
  { track: { id: 'string', on: 'boolean' } },
  { ring_shown: { ring_id: 'string', on: 'boolean' } },
  { forget: { id: 'string' } },
  { add_folder: { path: 'string' } },
  { create: { name: 'string' } },
  { suggestion_dismiss: { path: 'string' } },
  { suggestion_add: { path: 'string' } },
];

const NONE = {};

/** The engine's methods (`hub::Call`), by name. */
const ENGINE = {
  snapshot: NONE,
  settings: NONE,
  answer: { session_id: 'string', tool_use_id: 'string', answer: ANSWER },
  send_message: { session_id: 'string', text: 'string' },
  message_route: { session_id: 'string' },
  focus: { session_id: 'string' },
  mark_reviewed: { session_id: 'string', at_ms: 'number' },
  mark_viewed: { session_id: 'string', completed_at_ms: 'number' },
  mark_all_reviewed: { session_ids: ['array', 'string'], at_ms: 'number' },
  dismiss_failure: { session_id: 'string' },
  reset_review_queue: NONE,
  chat_open: { session_id: 'string' },
  chat_close: { session_id: 'string' },
  chat_more: { session_id: 'string', before_id: 'string' },
  chat_image: { session_id: 'string', image_id: 'string' },
  refresh_usage: { ring_id: 'string?', reason: ['enum', 'ring_click', 'manual'] },
  hook_consent: { grant: 'boolean' },
  hooks_enabled: { on: 'boolean' },
  status_line_enabled: { on: 'boolean' },
  hooks_reinstall: { account_id: 'string?' },
  remove_codenotch_hooks: { folder: 'string' },
  acknowledge_scope: NONE,
  account: { action: ACCOUNT_ACTION },
  set_setting: { key: 'string', value: 'any?' },
  choose_claude_binary: { path: 'string?' },
  cloud: { action: ['enum', 'sign_in', 'cancel_sign_in', 'sign_out', 'set_sync', 'set_summaries', 'sync_now'], on: 'boolean?' },
  cloud_url: { target: ['enum', 'dashboard', 'pools', 'settings'] },
  session_state_text: NONE,
  launch_command: { account_id: 'string' },
  reveal_target: { kind: ['enum', 'config_dir', 'session_cwd', 'backup'], id: 'string' },
  panel_state: {
    open: 'boolean', route: 'string?', ring_id: 'string?', reason: 'string?',
    engaged: 'boolean', focused: 'boolean', pinned: 'boolean',
  },
  hotkey_status: { ok: 'boolean', message: 'string?' },
};

/** The methods the glue answers itself (§3.5 "Glue-level methods"). */
const GLUE = {
  panel_toggle: { ring_id: 'string?', rect: ['one', ['array', 'number'], 'string?'], reason: 'string' },
  panel_open: { route: 'string', reason: 'string', ring_id: 'string?' },
  panel_close: NONE,
  panel_route: { route: 'string' },
  panel_report_size: { w: 'number', h: 'number' },
  panel_take_focus: NONE,
  panel_engaged: { on: 'boolean' },
  open_settings: { tab: 'string?' },
  open_url: { url: 'string' },
  open_notification_settings: NONE,
  copy_text: { text: 'string' },
  reveal: { kind: ['enum', 'config_dir', 'session_cwd', 'backup'], id: 'string' },
  pick_folder: NONE,
  log: { msg: 'string' },
};

const GLUE_ONLY = ['panel_state', 'hotkey_status'];

/** §4.12: the settings a page may set, and what each takes. */
const SETTINGS = {
  usageProbeIntervalMinutes: ['enum', 0, 5, 10, 15, 30],
  readsDesktopUsageCache: 'boolean',
  notifyNeedsInput: 'boolean',
  notifyReadyForReview: 'boolean',
  autoOpen: ['enum', 'never', 'needsInput', 'needsInputOrDone'],
  holdOpenWhileNeedsYou: ['enum', 'never', 'always'],
  ringBadges: 'boolean',
  restingMarks: 'boolean',
  trayBadge: 'boolean',
  ringClick: ['enum', 'openPanel', 'refreshUsage'],
  sessionClick: ['enum', 'smart', 'panel', 'terminal'],
  hotKey: ['enum', 'off', 'ctrlAltSpace', 'ctrlAltJ'],
  panelPinned: 'boolean',
  sound: 'boolean',
  peek: 'boolean',
  peekSeconds: ['enum', 3, 5, 10],
  typeReplies: 'boolean',
};

/** §3.7: which window may make which call (the engine's `allowed_from_window`). */
function allowedFromWindow(label, method) {
  if (GLUE_ONLY.includes(method)) return false;
  if (label === 'agentnotch-panel') return true;
  if (label === 'notch') {
    return ['snapshot', 'refresh_usage', 'focus', 'mark_reviewed', 'open_settings', 'log'].includes(method) ||
      method.startsWith('panel_');
  }
  if (label === 'settings') return !['answer', 'send_message'].includes(method);
  return false;
}

/** Why `{method, args}` is not a call the engine or the glue takes; null when it is one. */
function callProblem(method, args) {
  const schema = Object.prototype.hasOwnProperty.call(ENGINE, method) ? ENGINE[method]
    : Object.prototype.hasOwnProperty.call(GLUE, method) ? GLUE[method] : null;
  if (!schema) return `unknown method "${method}"`;
  // Methods without arguments take none, `null` or `{}` (`Call::from_parts`).
  const given = args === undefined || args === null ? {} : args;
  const problem = check(given, schema, method);
  if (problem) return problem;
  if (method === 'set_setting') {
    if (!Object.prototype.hasOwnProperty.call(SETTINGS, given.key)) return `set_setting: unknown key "${given.key}"`;
    return check(given.value, SETTINGS[given.key], `set_setting.${given.key}`);
  }
  return null;
}

module.exports = {
  WINDOWS,
  CONTRACT_DIR,
  fixture,
  check,
  ENGINE,
  GLUE,
  GLUE_ONLY,
  SETTINGS,
  allowedFromWindow,
  callProblem,
};
