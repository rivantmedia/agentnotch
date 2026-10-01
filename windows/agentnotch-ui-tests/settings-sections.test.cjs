'use strict';
// The second half of the Claude Code pane (ui/agentnotch/settings-sections.js, loaded by
// settings.js): Usage, Cloud, Sessions and attention, Notifications, Advanced.
//
// Swift tests ported: CloudSettingsTests (the website row, sign-in copy and its problem, what sync
// sends and never sends, what summaries cost and send, the summaries note, the sync row, the
// website's Settings link, every auth state drawn), Fix_SettingsAndPanelCopyTests (the usage
// check says what it runs: the page shows the engine's caption as given) and the settings-side
// cases of B_MovedChatSettingsTests (the engine words the summaries; the page shows them).
//
// The sync and summaries switches are consent gates for uploads: every test that touches them
// checks that a call leaves only from a click on that switch, and that the switch shows what the
// snapshot says, never what was clicked.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const audit = require('./lib/audit.cjs');
const contract = require('./lib/contract.cjs');
const harness = require('./lib/harness.cjs');
const scripts = require('./lib/scripts.cjs');

const plain = scripts.plain;
const SOURCE = fs.readFileSync(path.join(scripts.DIR, 'settings-sections.js'), 'utf8');
const NOW = harness.NOW;

// ---- helpers ---------------------------------------------------------------------------------

function settingsWith(edit) {
  const settings = harness.fixture('settings.json');
  if (edit) edit(settings);
  return settings;
}

async function open(edit, options) {
  const page = harness.loadPage('settings.html', Object.assign({ settings: settingsWith(edit) }, options));
  await page.settle();
  page.hub.clear();
  return page;
}

function clean(page) {
  assert.deepEqual(page.errors.map(String), []);
  assert.deepEqual(page.hub.violations, []);
}

function text(el) {
  if (!el) return null;
  const parts = [];
  const visit = (node) => {
    if (node.nodeType === 3) parts.push(node.nodeValue);
    else for (const child of node.childNodes) visit(child);
  };
  visit(el);
  return parts.join(' ').replace(/\s+/g, ' ').trim();
}
const region = (page, name) => page.$(`#pane-claude [data-an-section="${name}"]`);
const rowOf = (page, name, key) => region(page, name).querySelectorAll('[data-key]').find((el) => el.getAttribute('data-key') === key);
const action = (root, name, arg) => root.querySelectorAll('[data-an-action]').find((el) =>
  el.getAttribute('data-an-action') === name && (arg === undefined || el.getAttribute('data-an-arg') === arg));
const segButton = (page, key, value) => page.$$('#pane-claude [data-an-action="an-set-seg"]').find((el) =>
  el.getAttribute('data-an-arg') === key && el.getAttribute('data-an-value') === String(value));
const switchOf = (page, key) => action(page.$('#pane-claude'), 'an-set-switch', key);
const segValue = (page, key) => {
  const on = page.$$('#pane-claude [data-an-action="an-set-seg"]').filter((el) => el.getAttribute('data-an-arg') === key && el.classList.contains('on'));
  return on.length === 1 ? on[0].getAttribute('data-an-value') : on.length;
};
const isOn = (el) => el.getAttribute('aria-checked') === 'true';
const calls = (page) => plain(page.hub.calls);

async function click(page, el) {
  assert.ok(el, 'the control is on the page');
  page.click(el);
  await page.settle();
}

function signedOut(s, extra) {
  s.cloud = Object.assign({
    website_url: 'https://agentnotch.rivant.in', website_is_overridden: false, auth: 'signed_out',
    sync_enabled: false, summaries_enabled: false, summaries_available: true, is_syncing: false,
    last_sync_at_ms: null, last_error: null, pending_sessions: 0, pending_usage: 0, summarized_sessions: 0,
    dashboard_url: null, pools_url: null, settings_url: null,
  }, extra);
}

function sections(page) {
  return page.run('agentnotchSettingsSections');
}

// ---- the script ------------------------------------------------------------------------------

test('settings-sections.js is one strict IIFE with one global, no top-level let/const/class, no opener of its own', () => {
  assert.match(SOURCE, /^(\/\/[^\n]*\n)+\(function \(\) \{\n  'use strict';/);
  assert.match(SOURCE.trimEnd(), /\}\)\(\);$/);
  const globals = [...SOURCE.matchAll(/window\.(agentnotch[A-Za-z]*) =/g)].map((m) => m[1]);
  assert.deepEqual([...new Set(globals)], ['agentnotchSettingsSections']);
  assert.doesNotMatch(SOURCE, /^(let|const|class) /m);
  assert.doesNotMatch(SOURCE, /\.innerHTML\s*=|insertAdjacentHTML|eval\(|new Function|window\.open|location\s*[.=]|\.href\s*=|console\./);
  // The page never holds a website token: the cloud state has none, and nothing here asks for one.
  assert.doesNotMatch(SOURCE, /access_token|refresh_token|\.credentials/);
});

test('loaded by settings.js after the shared scripts; a load sends nothing but the settings call', async () => {
  const page = harness.loadPage('settings.html', {});
  await page.settle();
  assert.deepEqual(page.loaded.slice(-1), ['agentnotch/settings-sections.js']);
  assert.deepEqual(page.hub.calls.map((c) => c.method), ['settings']);
  assert.equal(page.run('typeof agentnotchSettingsSections.status'), 'function');
  clean(page);
});

test('every section draws from settings.json in the Mac\'s order, with its rows in order', async () => {
  const page = await open();
  const titles = page.$$('#pane-claude .sec').map(text);
  assert.deepEqual(titles.slice(2), ['Usage', 'Cloud', 'Sessions and attention', 'Notifications', 'Advanced']);
  const keys = (name) => region(page, name).querySelector('.group').children.map((el) => el.getAttribute('data-key'));
  assert.deepEqual(keys('usage'), ['interval', 'desktop', 'acct:claude-acct-5f3e1d2c0b9a', 'acct:claude-acct-8a7b6c5d4e3f', 'refresh']);
  assert.deepEqual(keys('cloud'), ['website', 'account', 'sync', 'summaries', 'status', 'dashboard']);
  assert.deepEqual(keys('attention'), ['auto-open', 'hold-open', 'ring-badges', 'resting-marks', 'tray-badge', 'ring-click',
    'session-click', 'hotkey', 'sound', 'peek', 'peek-seconds', 'type-replies', 'open-panel']);
  assert.deepEqual(keys('notifications'), ['needs-input', 'ready', 'permission']);
  assert.deepEqual(keys('advanced'), ['state', 'queue']);
  clean(page);
});

// ---- Usage ------------------------------------------------------------------------------------

test('Usage: the interval from its options, the engine\'s caption as given, the Desktop switch, a line per account', async () => {
  const page = await open();
  const usage = region(page, 'usage');
  assert.deepEqual(usage.querySelectorAll('[data-an-arg="usageProbeIntervalMinutes"]').map(text), ['Off', '5 min', '10 min', '15 min', '30 min']);
  assert.equal(segValue(page, 'usageProbeIntervalMinutes'), '5');
  // UsageCheckCopy (S5): what it runs and what it never touches, worded by the engine.
  const caption = text(rowOf(page, 'usage', 'interval').querySelector('.an-cap'));
  for (const words of ['Every 5 min', "Claude Code's own usage check", 'update its own files', 'never reads your login token']) {
    assert.ok(caption.includes(words), words);
  }
  assert.ok(isOn(switchOf(page, 'readsDesktopUsageCache')));
  assert.match(text(rowOf(page, 'usage', 'desktop')), /Also read Claude Desktop's cached usage Claude Desktop keeps the limits it last saw on disk; no token is involved\./);
  assert.equal(text(rowOf(page, 'usage', 'acct:claude-acct-8a7b6c5d4e3f')), 'Work 5-hour 72% · weekly 55% · 4m ago');
  assert.equal(text(rowOf(page, 'usage', 'refresh')), 'Refresh now');

  // The engine's Off caption is shown as given too.
  const off = await open((s) => {
    s.usage.interval_minutes = 0;
    s.usage.interval_caption = "Off: readings come only from live status lines and Claude Code's own cache.";
  });
  assert.equal(segValue(off, 'usageProbeIntervalMinutes'), '0');
  assert.match(text(rowOf(off, 'usage', 'interval')), /Off: readings come only from live status lines/);
});

test('Usage: the Desktop cache format line says what the engine found, only while reading is on', async () => {
  const absent = await open();
  assert.equal(text(rowOf(absent, 'usage', 'desktop').querySelector('[data-key="format"]')), "Claude Desktop's cache isn't on this PC.");
  const blockfile = await open((s) => { s.usage.desktop_format = 'blockfile'; });
  const line = rowOf(blockfile, 'usage', 'desktop').querySelector('[data-key="format"]');
  assert.equal(text(line), "Claude Desktop's cache on this PC uses a format this version can't read.");
  assert.ok(line.classList.contains('an-tone-warning'));
  for (const edit of [(s) => { s.usage.desktop_format = 'simple'; }, (s) => { s.usage.desktop_format = null; },
    (s) => { s.usage.desktop_format = 'blockfile'; s.usage.desktop_cache = false; }]) {
    const page = await open(edit);
    assert.equal(rowOf(page, 'usage', 'desktop').querySelector('[data-key="format"]'), null);
  }
});

test('Refresh now sends refresh_usage with no ring; while refreshing it is disabled beside a busy ring', async () => {
  const page = await open();
  await click(page, action(region(page, 'usage'), 'an-refresh'));
  assert.deepEqual(calls(page), [{ method: 'refresh_usage', args: { reason: 'manual' } }]);
  assert.equal(contract.callProblem('refresh_usage', { reason: 'manual' }), null);

  const busy = await open((s) => { s.usage.refreshing = true; });
  const button = action(region(busy, 'usage'), 'an-refresh');
  assert.equal(button.hasAttribute('disabled'), true);
  assert.ok(rowOf(busy, 'usage', 'refresh').querySelector('.an-busy[role="progressbar"]'));
  busy.run('agentnotchSettings.actions["an-refresh"]()');
  await busy.settle();
  assert.deepEqual(calls(busy), []);
  clean(busy);
});

// ---- every setting: its exact key and value -------------------------------------------------

test('every setting control sends set_setting with its exact key and value, as the contract\'s SETTINGS takes them', async () => {
  const expected = [
    ['seg', 'usageProbeIntervalMinutes', 15, 15],
    ['switch', 'readsDesktopUsageCache', null, false],
    ['seg', 'autoOpen', 'needsInputOrDone', 'needsInputOrDone'],
    ['seg', 'holdOpenWhileNeedsYou', 'always', 'always'],
    ['switch', 'ringBadges', null, false],
    ['switch', 'restingMarks', null, false],
    ['switch', 'trayBadge', null, false],
    ['seg', 'ringClick', 'refreshUsage', 'refreshUsage'],
    ['seg', 'sessionClick', 'terminal', 'terminal'],
    ['seg', 'hotKey', 'ctrlAltJ', 'ctrlAltJ'],
    ['switch', 'sound', null, false],
    ['switch', 'peek', null, false],
    ['seg', 'peekSeconds', 10, 10],
    ['switch', 'typeReplies', null, true],
    ['switch', 'notifyNeedsInput', null, false],
    ['switch', 'notifyReadyForReview', null, false],
  ];
  for (const [kind, key, value, sent] of expected) {
    const page = await open();
    await click(page, kind === 'seg' ? segButton(page, key, value) : switchOf(page, key));
    assert.deepEqual(calls(page), [{ method: 'set_setting', args: { key, value: sent } }], key);
    assert.equal(contract.callProblem('set_setting', { key, value: sent }), null, key);
    clean(page);
  }
  // Every setting a page may write, except the panel's pin (the panel's own), is on this pane.
  const covered = new Set(expected.map((e) => e[1]));
  assert.deepEqual(Object.keys(contract.SETTINGS).filter((k) => !covered.has(k)), ['panelPinned']);

  // Every segment the pane draws is a value its key takes.
  const page = await open();
  for (const el of page.$$('#pane-claude [data-an-action="an-set-seg"]')) {
    const key = el.getAttribute('data-an-arg');
    const raw = el.getAttribute('data-an-value');
    const value = /^\d+$/.test(raw) ? Number(raw) : raw;
    assert.equal(contract.check(value, contract.SETTINGS[key], key), null, `${key}=${raw}`);
  }
});

test('the value already shown sends nothing; a value outside the option table is never sent', async () => {
  const page = await open();
  await click(page, segButton(page, 'hotKey', 'off'));
  await click(page, segButton(page, 'usageProbeIntervalMinutes', 5));
  const bogus = segButton(page, 'hotKey', 'ctrlAltJ');
  bogus.setAttribute('data-an-value', 'ctrlAltF4');
  await click(page, bogus);
  page.run('agentnotchSettings.actions["an-set-switch"]("autoOpen", { getAttribute: function () { return "1"; } })');
  page.run('agentnotchSettings.actions["an-set-switch"]("noSuchSetting", { getAttribute: function () { return "1"; } })');
  await page.settle();
  assert.deepEqual(calls(page), []);
  clean(page);
});

test('a clicked value shows at once and stands 2 s for a stale snapshot; a snapshot that shows it ends the wait', async () => {
  const page = await open();
  page.click(segButton(page, 'holdOpenWhileNeedsYou', 'always'));
  assert.equal(segValue(page, 'holdOpenWhileNeedsYou'), 'always', 'at once, before the reply');
  await page.settle();
  page.emit('an:settings', settingsWith());
  assert.equal(segValue(page, 'holdOpenWhileNeedsYou'), 'always', 'a snapshot from before the change');
  page.tick(2100);
  page.emit('an:settings', settingsWith());
  assert.equal(segValue(page, 'holdOpenWhileNeedsYou'), 'never', 'after the grace, the snapshot wins');

  await click(page, switchOf(page, 'sound'));
  assert.equal(isOn(switchOf(page, 'sound')), false);
  page.emit('an:settings', settingsWith((s) => { s.attention.sound = false; }));
  page.emit('an:settings', settingsWith());
  assert.equal(isOn(switchOf(page, 'sound')), true, 'the hold ended when a snapshot showed the value');
  clean(page);
});

test('an invalid reply shows the hub\'s message and the control returns to the snapshot\'s value', async () => {
  const page = await open(null, {
    replies: { set_setting: () => { throw { code: 'invalid', message: 'That value isn’t one this setting takes.' }; } },
  });
  await click(page, segButton(page, 'peekSeconds', 10));
  assert.equal(segValue(page, 'peekSeconds'), '5');
  const note = region(page, 'attention').querySelector('.an-note');
  assert.equal(text(note), 'That value isn’t one this setting takes.');
  assert.ok(note.classList.contains('an-tone-critical'));
  await click(page, switchOf(page, 'notifyNeedsInput'));
  assert.equal(isOn(switchOf(page, 'notifyNeedsInput')), true);
  assert.match(text(region(page, 'notifications')), /That value isn’t one this setting takes\./);
  // One click, one call: nothing is retried.
  assert.equal(page.hub.of('set_setting').length, 2);
});

test('sealed: every write of these sections shows the sealed message and is not retried', async () => {
  const sealed = () => { throw { code: 'sealed', message: 'Sealed: nothing here changes.' }; };
  const methods = ['set_setting', 'refresh_usage', 'cloud', 'cloud_url', 'panel_open', 'open_notification_settings',
    'session_state_text', 'reset_review_queue'];
  const replies = Object.fromEntries(methods.map((m) => [m, sealed]));
  const steps = [
    ['usage', (p) => segButton(p, 'usageProbeIntervalMinutes', 10)],
    ['usage', (p) => action(region(p, 'usage'), 'an-refresh')],
    ['cloud', (p) => action(region(p, 'cloud'), 'an-cloud-sync')],
    ['cloud', (p) => action(region(p, 'cloud'), 'an-cloud-link', 'dashboard')],
    ['attention', (p) => switchOf(p, 'typeReplies')],
    ['attention', (p) => action(region(p, 'attention'), 'an-open-panel')],
    ['notifications', (p) => action(region(p, 'notifications'), 'an-open-notifications')],
    ['advanced', (p) => action(region(p, 'advanced'), 'an-state-copy')],
  ];
  for (const [name, find] of steps) {
    const page = await open((s) => {
      s.notifications.permission = 'disabled_for_user';
      s.notifications.permission_text = 'Off in Windows Settings';
    }, { replies });
    await click(page, find(page));
    const note = region(page, name).querySelector('.an-note');
    assert.equal(text(note), 'Sealed: nothing here changes.', name);
    assert.ok(note.classList.contains('an-tone-neutral'));
    assert.equal(page.hub.calls.length, 1, `${name}: one call`);
    assert.deepEqual(page.hub.violations, []);
  }
  const page = await open(null, { replies });
  await click(page, action(region(page, 'advanced'), 'an-reset-ask'));
  await click(page, action(region(page, 'advanced'), 'an-reset'));
  assert.equal(text(region(page, 'advanced').querySelector('.an-note')), 'Sealed: nothing here changes.');
});

// ---- Cloud: the copy (CloudSettingsTests) ----------------------------------------------------

test('CloudSettingsCopy: the website row shows the host, http:// kept; None; the override line', async () => {
  const page = await open();
  const S = sections(page);
  assert.equal(S.websiteDisplay('https://agentnotch.rivant.in'), 'agentnotch.rivant.in');
  assert.equal(S.websiteDisplay('https://agentnotch.example.com:8443/app'), 'agentnotch.example.com:8443/app');
  assert.equal(S.websiteDisplay('HTTPS://A.example'), 'A.example');
  assert.equal(S.websiteDisplay('http://localhost:3000'), 'http://localhost:3000');
  assert.equal(S.COPY.noWebsite, 'None');
  assert.equal(S.COPY.websiteOverridden, 'Set by AGENTNOTCH_WEB_URL for this run.');
});

test('CloudSettingsCopy: signing in needs a website and says where the sign-in is kept; a failure says why', async () => {
  const S = sections(await open());
  assert.equal(S.signInDetail({}), "This build has no website, so it can't sign in or sync.");
  const set = S.signInDetail({ website_url: 'https://agentnotch.example.com' });
  assert.ok(set.startsWith("Opens Google's sign-in in your browser."));
  assert.ok(set.includes(S.COPY.fileNote));
  assert.ok(S.COPY.fileNote.includes('only your user can read'));
  assert.equal(S.signInDetail({ website_url: 'https://a.example', auth: { signing_in: {} } }), 'Finish signing in in your browser.');
  assert.equal(S.signInDetail({ website_url: 'https://a.example', auth: 'signing_in' }), 'Finish signing in in your browser.');
  assert.equal(S.signInProblem({}), null);
  assert.equal(S.signInProblem({ auth: { error: { message: "The website isn't answering." } } }), "The website isn't answering.");
  assert.equal(S.signInProblem({ last_error: 'Signed out of the website. Sign in again.' }), 'Signed out of the website. Sign in again.');
});

test('CloudSettingsCopy: sync says what is sent and what never is; summaries say what they cost and send', async () => {
  const S = sections(await open());
  for (const reads of [true, false]) {
    const t = S.syncDetail(reads);
    for (const word of ['project folder names', 'session titles', 'models', 'times', 'token counts', 'cost', 'usage limits',
      'each signed-in Claude account', 'Claude Desktop']) assert.ok(t.includes(word), word);
    assert.ok(t.includes('Never file paths, prompts or your Claude login.'));
  }
  assert.ok(S.syncDetail(false).includes('once reading its cache is on'));
  const d = S.COPY.summariesDetail;
  assert.ok(d.includes("Runs Claude Code on this PC with the session's own account"));
  assert.ok(d.includes("uses that account's usage"));
  assert.ok(d.includes('a few cents per session with Haiku'));
  assert.ok(d.includes('one- or two-sentence summary'));
  assert.ok(d.startsWith('Only for sessions that end after you turn this on.'));
  assert.ok(d.includes('at most $0.10 a run, 20 an hour and 60 a day'));
  assert.ok(d.includes("none while that account's 5-hour limit is 80% used"));
  assert.ok(d.includes('file paths cut to their last part and anything like a key or password removed'));
  assert.ok(d.endsWith('Turning this off deletes the summaries not sent yet; summaries already sent stay on the website.'));
  assert.ok(S.COPY.dashboard.includes('every computer and account you sync'));

  const cloud = { auth: { signed_in: { email: null } }, sync_enabled: false, summaries_enabled: true };
  assert.equal(S.summariesNote(cloud), 'Needs sync.');
  cloud.sync_enabled = true;
  assert.equal(S.summariesNote(cloud), "Not in this run: it can't start Claude Code.");
  cloud.summaries_available = true;
  assert.equal(S.summariesNote(cloud), null);
  cloud.summarized_sessions = 1;
  assert.equal(S.summariesNote(cloud), '1 session summarised on this PC.');
  cloud.summarized_sessions = 12;
  assert.equal(S.summariesNote(cloud), '12 sessions summarised on this PC.');
});

test('CloudSettingsCopy: the sync row says when it last worked and what waits', async () => {
  const S = sections(await open());
  const st = (c) => plain(S.status(c, NOW));
  const cloud = { auth: { signed_in: { email: 'me@example.com' } } };
  assert.deepEqual(st(cloud), { title: 'Sync is off', detail: 'Nothing is sent while it is off.', isProblem: false });
  cloud.sync_enabled = true;
  assert.deepEqual(st(cloud), { title: 'Not synced yet', detail: null, isProblem: false });
  cloud.last_sync_at_ms = NOW - 180000;
  cloud.pending_sessions = 1;
  cloud.pending_usage = 4;
  assert.deepEqual(st(cloud), { title: 'Last synced 3m ago', detail: '1 session and 4 usage readings to send.', isProblem: false });
  cloud.is_syncing = true;
  assert.equal(st(cloud).title, 'Syncing…');
  cloud.is_syncing = false;
  cloud.last_error = 'The website is busy. Trying again in a minute.';
  assert.deepEqual(st(cloud), { title: 'Last sync failed', detail: 'The website is busy. Trying again in a minute.', isProblem: true });
  assert.equal(S.waiting(0, 0), null);
  assert.equal(S.waiting(3, 0), '3 sessions to send.');
  assert.equal(S.waiting(0, 1), '1 usage reading to send.');
});

// ---- Cloud: each auth state drawn ------------------------------------------------------------

test('signed in (the fixture): website, the email, the switches as the snapshot says, the sync row, the dashboard', async () => {
  const page = await open();
  const cloud = region(page, 'cloud');
  assert.equal(text(rowOf(page, 'cloud', 'website')), 'Website agentnotch.rivant.in');
  const who = rowOf(page, 'cloud', 'account').querySelector('[data-user]');
  assert.equal(text(who), 'Signed in as me@personal.example');
  assert.match(text(rowOf(page, 'cloud', 'account')), /The website sign-in is kept in a file only your user can read\. Sign out…$/);
  assert.equal(isOn(action(cloud, 'an-cloud-sync')), true);
  assert.equal(isOn(action(cloud, 'an-cloud-summaries')), false);
  assert.equal(action(cloud, 'an-cloud-summaries').hasAttribute('disabled'), false);
  assert.match(text(rowOf(page, 'cloud', 'sync')), /\(Claude Desktop's readings too\)/);
  assert.equal(text(rowOf(page, 'cloud', 'status')), 'Last synced 3m ago 1 session and 4 usage readings to send. Sync now');
  assert.equal(text(rowOf(page, 'cloud', 'dashboard')),
    "Dashboard Your sessions and usage from every computer and account you sync. Share an account there with a code: everyone in its pool sees what it was used for. Remove summaries or delete synced data in the website's Settings . Open dashboard Share accounts…");
  for (const name of ['an-cloud-sync', 'an-cloud-summaries', 'an-cloud-sign-out-ask', 'an-cloud-sync-now']) assert.ok(action(cloud, name), name);
  assert.equal(action(cloud, 'an-cloud-sign-in'), undefined);
  // The sign-in is the website's, kept in a file: no Claude sign-in button anywhere on the pane.
  assert.doesNotMatch(text(page.$('#pane-claude')), /Sign in to Claude|Log in to Claude/);
  clean(page);
});

test('signed in without an email, sync off, Desktop not read: plain "Signed in", summaries disabled with "Needs sync."', async () => {
  const page = await open((s) => {
    s.cloud.auth = { signed_in: { email: null } };
    s.cloud.sync_enabled = false;
    s.cloud.summaries_enabled = true;
    s.usage.desktop_cache = false;
  });
  assert.match(text(rowOf(page, 'cloud', 'account')), /^Signed in The website/);
  const summaries = action(region(page, 'cloud'), 'an-cloud-summaries');
  assert.equal(isOn(summaries), false, 'summaries ride on sync');
  assert.equal(summaries.hasAttribute('disabled'), true);
  assert.match(text(rowOf(page, 'cloud', 'summaries')), /Needs sync\.$/);
  assert.match(text(rowOf(page, 'cloud', 'sync')), /once reading its cache is on/);
  assert.equal(text(rowOf(page, 'cloud', 'status')), 'Sync is off Nothing is sent while it is off. Sync now');
  assert.equal(action(region(page, 'cloud'), 'an-cloud-sync-now').hasAttribute('disabled'), true);
  page.run('agentnotchSettings.actions["an-cloud-summaries"]()');
  page.run('agentnotchSettings.actions["an-cloud-sync-now"]()');
  await page.settle();
  assert.deepEqual(calls(page), []);
});

test('summaries that can\'t run in this run: disabled with the reason while off, amber while on (and can be turned off)', async () => {
  const off = await open((s) => { s.cloud.summaries_available = false; });
  const sw = action(region(off, 'cloud'), 'an-cloud-summaries');
  assert.equal(sw.hasAttribute('disabled'), true);
  const note = rowOf(off, 'cloud', 'summaries').querySelector('[data-key="note"]');
  assert.equal(text(note), "Not in this run: it can't start Claude Code.");
  assert.equal(note.classList.contains('an-tone-warning'), false);
  off.run('agentnotchSettings.actions["an-cloud-summaries"]()');
  await off.settle();
  assert.deepEqual(calls(off), []);

  const on = await open((s) => { s.cloud.summaries_available = false; s.cloud.summaries_enabled = true; });
  const sw2 = action(region(on, 'cloud'), 'an-cloud-summaries');
  assert.equal(isOn(sw2), true);
  assert.equal(sw2.hasAttribute('disabled'), false);
  assert.ok(rowOf(on, 'cloud', 'summaries').querySelector('[data-key="note"]').classList.contains('an-tone-warning'));
  await click(on, sw2);
  assert.deepEqual(calls(on), [{ method: 'cloud', args: { action: 'set_summaries', on: false } }]);

  const counted = await open((s) => { s.cloud.summaries_enabled = true; s.cloud.summarized_sessions = 12; });
  assert.match(text(rowOf(counted, 'cloud', 'summaries')), /12 sessions summarised on this PC\.$/);
});

test('the sync row: syncing (busy, Sync now disabled), a failure in red, not synced yet', async () => {
  const syncing = await open((s) => { s.cloud.is_syncing = true; s.cloud.last_error = 'Busy'; });
  assert.match(text(rowOf(syncing, 'cloud', 'status')), /^Syncing… 1 session and 4 usage readings to send\./);
  assert.ok(rowOf(syncing, 'cloud', 'status').querySelector('.an-busy'));
  assert.equal(action(region(syncing, 'cloud'), 'an-cloud-sync-now').hasAttribute('disabled'), true);

  const failed = await open((s) => { s.cloud.last_error = 'The website is busy. Trying again in a minute.'; });
  const detail = rowOf(failed, 'cloud', 'status').querySelector('[data-key="detail"]');
  assert.equal(text(detail), 'The website is busy. Trying again in a minute.');
  assert.ok(detail.classList.contains('an-tone-critical'));
  assert.match(text(rowOf(failed, 'cloud', 'status')), /^Last sync failed/);

  const fresh = await open((s) => { s.cloud.last_sync_at_ms = null; s.cloud.pending_sessions = 0; s.cloud.pending_usage = 0; });
  assert.equal(text(rowOf(fresh, 'cloud', 'status')), 'Not synced yet Sync now');
});

test('the dashboard buttons wait for the dashboard\'s address; the Settings link shows only with its address', async () => {
  const page = await open((s) => { s.cloud.dashboard_url = null; s.cloud.settings_url = null; });
  const cloud = region(page, 'cloud');
  assert.equal(action(cloud, 'an-cloud-link', 'dashboard').hasAttribute('disabled'), true);
  assert.equal(action(cloud, 'an-cloud-link', 'pools').hasAttribute('disabled'), true);
  assert.equal(action(cloud, 'an-cloud-link', 'settings'), undefined);
  assert.doesNotMatch(text(cloud), /Remove summaries/);
  for (const target of ['dashboard', 'pools', 'settings']) page.run(`agentnotchSettings.actions["an-cloud-link"](${JSON.stringify(target)})`);
  await page.settle();
  assert.deepEqual(calls(page), []);
});

test('signed out: "Not signed in", where the sign-in goes, Sign in with Google -> cloud {sign_in}', async () => {
  for (const auth of ['signed_out', { signed_out: {} }]) {
    const page = await open((s) => signedOut(s, { auth }));
    const row = rowOf(page, 'cloud', 'sign-in');
    assert.equal(text(row), "Not signed in Opens Google's sign-in in your browser. The website sign-in is kept in a file only your user can read. Sign in with Google");
    assert.equal(region(page, 'cloud').querySelector('.group').children.map((el) => el.getAttribute('data-key')).join(), 'website,sign-in');
    for (const name of ['an-cloud-sync', 'an-cloud-summaries', 'an-cloud-sync-now', 'an-cloud-link']) assert.equal(action(region(page, 'cloud'), name), undefined, name);
    await click(page, action(row, 'an-cloud-sign-in'));
    assert.deepEqual(calls(page), [{ method: 'cloud', args: { action: 'sign_in' } }]);
    clean(page);
  }
});

test('signing in: progress, Cancel -> cancel_sign_in, Sign in disabled; an error says why and allows a new try', async () => {
  const page = await open((s) => signedOut(s, { auth: 'signing_in' }));
  const row = rowOf(page, 'cloud', 'sign-in');
  assert.match(text(row), /^Signing in… Finish signing in in your browser\. Cancel Sign in with Google$/);
  assert.ok(row.querySelector('.an-busy[role="progressbar"]'));
  assert.equal(action(row, 'an-cloud-sign-in').hasAttribute('disabled'), true);
  page.run('agentnotchSettings.actions["an-cloud-sign-in"]()');
  await page.settle();
  assert.deepEqual(calls(page), []);
  await click(page, action(row, 'an-cloud-cancel'));
  assert.deepEqual(calls(page), [{ method: 'cloud', args: { action: 'cancel_sign_in' } }]);

  const failed = await open((s) => signedOut(s, { auth: { error: { message: "The website isn't answering." } } }));
  const problem = rowOf(failed, 'cloud', 'sign-in').querySelector('[data-key="problem"]');
  assert.equal(text(problem), "The website isn't answering.");
  assert.ok(problem.classList.contains('an-tone-critical'));
  assert.equal(action(region(failed, 'cloud'), 'an-cloud-cancel'), undefined);
  await click(failed, action(region(failed, 'cloud'), 'an-cloud-sign-in'));
  assert.deepEqual(calls(failed), [{ method: 'cloud', args: { action: 'sign_in' } }]);

  const signedOutWhy = await open((s) => signedOut(s, { last_error: 'Signed out of the website. Sign in again.' }));
  assert.equal(text(rowOf(signedOutWhy, 'cloud', 'sign-in').querySelector('[data-key="problem"]')), 'Signed out of the website. Sign in again.');
  // Cancel only while signing in.
  signedOutWhy.run('agentnotchSettings.actions["an-cloud-cancel"]()');
  await signedOutWhy.settle();
  assert.deepEqual(calls(signedOutWhy), []);
});

test('no website: "None", the build says it can\'t sign in, and the button is disabled; overridden says so', async () => {
  const page = await open((s) => signedOut(s, { website_url: null }));
  assert.equal(text(rowOf(page, 'cloud', 'website')), 'Website None');
  assert.match(text(rowOf(page, 'cloud', 'sign-in')), /This build has no website, so it can't sign in or sync\./);
  assert.equal(action(region(page, 'cloud'), 'an-cloud-sign-in').hasAttribute('disabled'), true);
  page.run('agentnotchSettings.actions["an-cloud-sign-in"]()');
  await page.settle();
  assert.deepEqual(calls(page), []);

  const dev = await open((s) => signedOut(s, { website_url: 'http://localhost:3000', website_is_overridden: true }));
  assert.equal(text(rowOf(dev, 'cloud', 'website')), 'Website Set by AGENTNOTCH_WEB_URL for this run. http://localhost:3000');
  assert.equal(rowOf(dev, 'cloud', 'website').querySelector('[data-user]').getAttribute('title'), 'http://localhost:3000');
});

// ---- Cloud: the calls ------------------------------------------------------------------------

test('the sync switch sends set_sync from its own click and shows only what the snapshot (an:cloud) says', async () => {
  const page = await open();
  const sync = () => action(region(page, 'cloud'), 'an-cloud-sync');
  await click(page, sync());
  assert.deepEqual(calls(page), [{ method: 'cloud', args: { action: 'set_sync', on: false } }]);
  assert.equal(isOn(sync()), true, 'never optimistic');
  const after = Object.assign(harness.fixture('settings.json').cloud, { sync_enabled: false });
  page.emit('an:cloud', after);
  assert.equal(isOn(sync()), false);
  assert.equal(action(region(page, 'cloud'), 'an-cloud-summaries').hasAttribute('disabled'), true);
  page.hub.clear();
  await click(page, sync());
  assert.deepEqual(calls(page), [{ method: 'cloud', args: { action: 'set_sync', on: true } }]);
  page.hub.clear();
  // Summaries: their own click, the opposite of what the snapshot shows.
  page.emit('an:cloud', Object.assign(after, { sync_enabled: true }));
  await click(page, action(region(page, 'cloud'), 'an-cloud-summaries'));
  assert.deepEqual(calls(page), [{ method: 'cloud', args: { action: 'set_summaries', on: true } }]);
  assert.equal(isOn(action(region(page, 'cloud'), 'an-cloud-summaries')), false);
  for (const c of calls(page)) assert.equal(contract.callProblem(c.method, c.args), null);
  clean(page);
});

test('Enter never flips a cloud switch or signs in; a refused switch keeps the snapshot\'s state and says why', async () => {
  const page = await open();
  for (const name of ['an-cloud-sync', 'an-cloud-summaries']) {
    const el = action(region(page, 'cloud'), name);
    assert.ok(el.hasAttribute('data-an-noenter'), name);
    el.focus();
    const event = page.key({ key: 'Enter' });
    assert.equal(event.defaultPrevented, true, name);
  }
  const out = await open((s) => signedOut(s));
  assert.ok(action(region(out, 'cloud'), 'an-cloud-sign-in').hasAttribute('data-an-noenter'));
  assert.deepEqual(calls(page), []);

  const refused = await open(null, { replies: { cloud: () => ({ error: 'Sign in again first.' }) } });
  await click(refused, action(region(refused, 'cloud'), 'an-cloud-sync'));
  assert.equal(isOn(action(region(refused, 'cloud'), 'an-cloud-sync')), true);
  assert.equal(text(region(refused, 'cloud').querySelector('.an-note')), 'Sign in again first.');
});

test('Sign out… asks first: Cancel sends nothing, Sign out sends sign_out once', async () => {
  const page = await open();
  await click(page, action(region(page, 'cloud'), 'an-cloud-sign-out-ask'));
  assert.deepEqual(calls(page), []);
  assert.match(text(rowOf(page, 'cloud', 'account')), /Cancel Sign out$/);
  await click(page, action(region(page, 'cloud'), 'an-cloud-sign-out-cancel'));
  assert.match(text(rowOf(page, 'cloud', 'account')), /Sign out…$/);
  page.run('agentnotchSettings.actions["an-cloud-sign-out"]()');
  await page.settle();
  assert.deepEqual(calls(page), [], 'no sign-out without the question');
  await click(page, action(region(page, 'cloud'), 'an-cloud-sign-out-ask'));
  const confirm = action(region(page, 'cloud'), 'an-cloud-sign-out');
  assert.ok(confirm.classList.contains('an-danger') && confirm.hasAttribute('data-an-noenter'));
  await click(page, confirm);
  assert.deepEqual(calls(page), [{ method: 'cloud', args: { action: 'sign_out' } }]);
  // Signed out by the snapshot: the question is gone.
  page.emit('an:cloud', Object.assign(harness.fixture('settings.json').cloud, { auth: 'signed_out', sync_enabled: false }));
  assert.ok(rowOf(page, 'cloud', 'sign-in'));
});

test('Sync now sends sync_now; the links go through cloud_url and then open_url, never window.open or location', async () => {
  const page = await open();
  await click(page, action(region(page, 'cloud'), 'an-cloud-sync-now'));
  await click(page, action(region(page, 'cloud'), 'an-cloud-link', 'dashboard'));
  await click(page, action(region(page, 'cloud'), 'an-cloud-link', 'pools'));
  await click(page, action(region(page, 'cloud'), 'an-cloud-link', 'settings'));
  assert.deepEqual(calls(page), [
    { method: 'cloud', args: { action: 'sync_now' } },
    { method: 'cloud_url', args: { target: 'dashboard' } },
    { method: 'open_url', args: { url: 'https://agentnotch.rivant.in/dashboard' } },
    { method: 'cloud_url', args: { target: 'pools' } },
    { method: 'open_url', args: { url: 'https://agentnotch.rivant.in/pools' } },
    { method: 'cloud_url', args: { target: 'settings' } },
    { method: 'open_url', args: { url: 'https://agentnotch.rivant.in/settings' } },
  ]);
  assert.deepEqual(page.upstreamCalls.filter((c) => /open|url/i.test(c.cmd)), []);
  assert.equal(page.$$('#pane-claude a').length, 0);

  // An answer that is not a web address is not opened.
  const odd = await open(null, { replies: { cloud_url: () => ({ url: 'javascript:alert(1)' }) } });
  await click(odd, action(region(odd, 'cloud'), 'an-cloud-link', 'dashboard'));
  assert.deepEqual(calls(odd).map((c) => c.method), ['cloud_url']);
});

// ---- Sessions and attention ------------------------------------------------------------------

test('the open-panel policy\'s caption follows the value; type replies is off by default with its caption', async () => {
  const captions = {
    never: 'The rings and the chime still tell you.',
    needsInput: "When a session needs you, unless you're already in its terminal. Never over a full-screen app.",
    needsInputOrDone: "Also when a session is done, unless you're already in its terminal. Never over a full-screen app.",
  };
  for (const [policy, caption] of Object.entries(captions)) {
    const page = await open((s) => { s.attention.panel_open_mode = policy; });
    assert.equal(text(rowOf(page, 'attention', 'auto-open').querySelector('[data-key="cap"]')), caption);
    assert.equal(segValue(page, 'autoOpen'), policy);
  }
  const page = await open();
  assert.deepEqual(region(page, 'attention').querySelectorAll('[data-an-arg="autoOpen"]').map(text),
    ['Never', 'When a session needs you', 'When one needs you or is done']);
  assert.deepEqual(region(page, 'attention').querySelectorAll('[data-an-arg="holdOpenWhileNeedsYou"]').map(text), ['Never', 'Always']);
  assert.deepEqual(region(page, 'attention').querySelectorAll('[data-an-arg="hotKey"]').map(text), ['Off', 'Ctrl+Alt+Space', 'Ctrl+Alt+J']);
  assert.deepEqual(region(page, 'attention').querySelectorAll('[data-an-arg="peekSeconds"]').map(text), ['3 s', '5 s', '10 s']);
  assert.equal(isOn(switchOf(page, 'typeReplies')), false);
  assert.equal(text(rowOf(page, 'attention', 'type-replies')),
    'Type replies into the terminal Types your reply into the session’s console and presses Enter. Works for Windows Terminal, VS Code’s terminal and console windows; not for a Claude started in the background of another shell (start /b).');
  assert.equal(text(rowOf(page, 'attention', 'session-click').querySelector('.an-cap')),
    'Smart opens the panel when the session needs you, and its terminal otherwise.');
  assert.equal(text(rowOf(page, 'attention', 'tray-badge')), 'Needs-you dot on the tray icon');
  assert.doesNotMatch(text(region(page, 'attention')), /camera|Dock|⌘|⌥/);
});

test('peek\'s duration is disabled while peek is off', async () => {
  const page = await open((s) => { s.attention.peek = false; });
  for (const el of region(page, 'attention').querySelectorAll('[data-an-arg="peekSeconds"]')) assert.equal(el.hasAttribute('disabled'), true);
  assert.ok(rowOf(page, 'attention', 'peek-seconds').classList.contains('an-dim'));
});

test('a shortcut Windows refused says "That shortcut is taken by another app."; Off or registered says nothing', async () => {
  const taken = await open((s) => { s.attention.hotkey = 'ctrlAltSpace'; s.attention.hotkey_ok = false; });
  const line = rowOf(taken, 'attention', 'hotkey').querySelector('[data-key="taken"]');
  assert.equal(text(line), 'That shortcut is taken by another app.');
  assert.ok(line.classList.contains('an-tone-warning'));
  const said = await open((s) => { s.attention.hotkey = 'ctrlAltJ'; s.attention.hotkey_ok = false; s.attention.hotkey_message = 'Windows refused Ctrl+Alt+J.'; });
  assert.equal(text(rowOf(said, 'attention', 'hotkey').querySelector('[data-key="taken"]')), 'Windows refused Ctrl+Alt+J.');
  for (const edit of [(s) => { s.attention.hotkey = 'ctrlAltSpace'; }, (s) => { s.attention.hotkey = 'ctrlAltSpace'; s.attention.hotkey_ok = true; },
    (s) => { s.attention.hotkey = 'off'; s.attention.hotkey_ok = false; }]) {
    const page = await open(edit);
    assert.equal(rowOf(page, 'attention', 'hotkey').querySelector('[data-key="taken"]'), null);
  }
});

test('Open the sessions panel sends panel_open {route: sessions, reason: settings}', async () => {
  const page = await open();
  await click(page, action(region(page, 'attention'), 'an-open-panel'));
  assert.deepEqual(calls(page), [{ method: 'panel_open', args: { route: 'sessions', reason: 'settings' } }]);
  clean(page);
});

// ---- Notifications ---------------------------------------------------------------------------

test('Notifications: the two switches and the Windows permission row, with Open… only where Windows can turn banners on', async () => {
  const page = await open();
  assert.equal(text(rowOf(page, 'notifications', 'needs-input')), 'Banner when a session needs you');
  assert.equal(text(rowOf(page, 'notifications', 'ready')), 'Banner when a session is done');
  assert.ok(isOn(switchOf(page, 'notifyNeedsInput')) && isOn(switchOf(page, 'notifyReadyForReview')));
  assert.equal(text(rowOf(page, 'notifications', 'permission')), 'Windows notifications Allowed');
  assert.equal(action(region(page, 'notifications'), 'an-open-notifications'), undefined);

  const off = await open((s) => Object.assign(s.notifications, { permission: 'disabled_for_user', permission_text: 'Off in Windows Settings', permission_warning: true }));
  const row = rowOf(off, 'notifications', 'permission');
  assert.equal(text(row), 'Windows notifications Off in Windows Settings Open…');
  assert.ok(row.querySelector('[data-key="text"]').classList.contains('an-tone-warning'));
  const openButton = action(row, 'an-open-notifications');
  assert.equal(openButton.getAttribute('aria-label'), 'Open notification settings');
  await click(off, openButton);
  assert.deepEqual(calls(off), [{ method: 'open_notification_settings', args: null }]);

  const quiet = await open((s) => Object.assign(s.notifications, { permission: 'disabled_by_policy', permission_text: 'Off in Windows Settings', permission_warning: false }));
  assert.equal(rowOf(quiet, 'notifications', 'permission').querySelector('[data-key="text"]').classList.contains('an-tone-warning'), false);

  const none = await open((s) => Object.assign(s.notifications, { permission: 'unavailable', permission_text: 'Banners need the installed app', permission_warning: true }));
  assert.equal(text(rowOf(none, 'notifications', 'permission')), 'Windows notifications Banners need the installed app');
  assert.equal(action(region(none, 'notifications'), 'an-open-notifications'), undefined);
});

// ---- Advanced --------------------------------------------------------------------------------

test('Session state: Copy asks for the text, copies exactly it and says Copied', async () => {
  const page = await open();
  assert.match(text(rowOf(page, 'advanced', 'state')), /^Session state A plain-text line per session for bug reports: its title, state and tasks, and the start of a finished reply\. Look it over before sharing it\. Copy$/);
  await click(page, action(region(page, 'advanced'), 'an-state-copy'));
  assert.deepEqual(calls(page), [
    { method: 'session_state_text', args: null },
    { method: 'copy_text', args: { text: 'Fix the login redirect loop [needs you] acme-web tasks 3/7' } },
  ]);
  assert.equal(text(page.$('#toast')), 'Copied');
  clean(page);
});

test('Review queue: the counts, Reset… asks, Cancel sends nothing, Mark all reviewed sends reset_review_queue', async () => {
  const page = await open();
  assert.equal(text(rowOf(page, 'advanced', 'queue').querySelector('[data-key="counts"]')), '3 of 15 sessions wait for review.');
  await click(page, action(region(page, 'advanced'), 'an-reset-ask'));
  assert.deepEqual(calls(page), []);
  await click(page, action(region(page, 'advanced'), 'an-reset-cancel'));
  page.run('agentnotchSettings.actions["an-reset"]()');
  await page.settle();
  assert.deepEqual(calls(page), []);
  await click(page, action(region(page, 'advanced'), 'an-reset-ask'));
  const confirm = action(region(page, 'advanced'), 'an-reset');
  assert.equal(text(confirm), 'Mark all reviewed');
  assert.ok(confirm.hasAttribute('data-an-noenter') && confirm.classList.contains('an-danger'));
  await click(page, confirm);
  assert.deepEqual(calls(page), [{ method: 'reset_review_queue', args: null }]);
  assert.match(text(rowOf(page, 'advanced', 'queue')), /Reset…$/);

  const one = await open((s) => { s.advanced = { session_count: 1, review_count: 1 }; });
  assert.equal(text(rowOf(one, 'advanced', 'queue').querySelector('[data-key="counts"]')), '1 of 1 session waits for review.');
  const none = await open((s) => { s.advanced = { session_count: 4, review_count: 0 }; });
  assert.equal(text(rowOf(none, 'advanced', 'queue').querySelector('[data-key="counts"]')), 'Nothing waits for review.');
});

// ---- untrusted text --------------------------------------------------------------------------

test('hostile strings in the email, website, errors, usage lines, captions, permission text and hotkey message stay text', async () => {
  for (const evil of audit.HOSTILE) {
    const edits = [
      (s) => {
        s.cloud.auth = { signed_in: { email: evil } };
        s.cloud.website_url = evil;
        s.cloud.last_error = evil;
        s.usage.interval_caption = evil;
        s.usage.desktop_caption = evil;
        s.usage.desktop_format = evil;
        s.usage.accounts.forEach((a) => { a.label = evil; a.line = evil; a.ring_id = evil; });
        s.notifications.permission = evil;
        s.notifications.permission_text = evil;
        s.attention.hotkey = 'ctrlAltJ';
        s.attention.hotkey_ok = false;
        s.attention.hotkey_message = evil;
        s.attention.panel_open_mode = evil;
      },
      (s) => signedOut(s, { website_url: evil, auth: { error: { message: evil } }, last_error: evil }),
    ];
    for (const edit of edits) {
      const page = await open(edit);
      for (const name of ['usage', 'cloud', 'attention', 'notifications', 'advanced']) {
        assert.deepEqual(audit.problems(region(page, name).innerHTML), [], `${name}: ${evil.slice(0, 40)}`);
      }
      clean(page);
    }
  }
  const page = await open((s) => { s.cloud.auth = { signed_in: { email: '<b>x</b>' } }; });
  assert.equal(text(rowOf(page, 'cloud', 'account').querySelector('[data-user]')), 'Signed in as <b>x</b>');
  assert.deepEqual(page.logs.filter((l) => /<b>x<\/b>/.test(JSON.stringify(l))), [], 'the email is never logged');
  assert.notDeepEqual(audit.problems(audit.HOSTILE[0]), [], 'the check can fail');
});

test('a snapshot with missing or odd sections still draws, with no page error', async () => {
  const page = await open((s) => {
    delete s.usage;
    s.cloud = null;
    s.attention = [];
    s.notifications = 'x';
    delete s.advanced;
  });
  for (const name of ['usage', 'cloud', 'attention', 'notifications', 'advanced']) assert.equal(region(page, name).hidden, false, name);
  assert.match(text(rowOf(page, 'cloud', 'sign-in')), /^Not signed in/);
  clean(page);
});

// ---- scenes ----------------------------------------------------------------------------------

test('the cloud scenes draw their state, start at the Cloud section, send nothing and ignore clicks', async () => {
  const listed = JSON.parse(fs.readFileSync(path.join(__dirname, 'scenes.json'), 'utf8')).scenes
    .filter((s) => s.page === 'settings.html').map((s) => s.scene);
  const want = {
    'settings-cloud-signed-out': /^Cloud Website agentnotch\.rivant\.in Not signed in Opens Google's sign-in/,
    'settings-cloud-signed-in': /^Cloud Website agentnotch\.rivant\.in Signed in as me@personal\.example/,
    'settings-cloud-overridden': /^Cloud Website Set by AGENTNOTCH_WEB_URL for this run\. http:\/\/localhost:3000 Not signed in/,
    'settings-cloud-no-website': /^Cloud Website None Not signed in This build has no website/,
  };
  for (const [name, pattern] of Object.entries(want)) {
    assert.ok(listed.includes(name), `${name} is in scenes.json`);
    const page = await open();
    assert.equal(page.run(`agentnotchSettings.showScene(${JSON.stringify(name)})`), true, name);
    assert.match(text(region(page, 'cloud')), pattern, name);
    for (const el of page.$$('#pane-claude [data-an-action]')) page.click(el);
    await page.settle();
    assert.deepEqual(calls(page), [], name);
    clean(page);
    assert.equal(plain(page.run('agentnotchSettings.layoutReport()')).scene, name);
  }
  // The full pane's scene still draws every section.
  const full = await open();
  full.run('agentnotchSettings.showScene("settings-full")');
  assert.deepEqual(full.$$('#pane-claude .sec').map(text).slice(2), ['Usage', 'Cloud', 'Sessions and attention', 'Notifications', 'Advanced']);
});

test('every method these sections call is one the settings window may make', () => {
  for (const method of ['set_setting', 'refresh_usage', 'cloud', 'cloud_url', 'open_url', 'panel_open',
    'open_notification_settings', 'session_state_text', 'copy_text', 'reset_review_queue']) {
    assert.ok(contract.allowedFromWindow('settings', method), method);
  }
});
