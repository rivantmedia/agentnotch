'use strict';
// The Claude Code pane of upstream's Settings page (ui/agentnotch/settings.js), first half: the
// frame (load order, priming, events, the sealed and failure lines), the rebrand of upstream's
// copy at run time, the consent card and the scope notice, Accounts, and Hooks and status line.
//
// This pane turns hook installation on and removes other apps' hooks, so every "must not act"
// case ends by asserting that NOTHING was sent, and every write is checked to leave only from a
// click on its own control.
//
// Swift tests ported (the pane's side; the engine words what arrives in the snapshot, so these
// check that the page shows it as given): PP_SettingsTests (folder summary, hook title, identity,
// folder roles and states), PPFix_SettingsCopyTests (consent copy, scope message, the whole file
// list with accounts, "In ~\.claude now", workspace folder names, the unsigned caption's VS Code
// variant, "VS Code or the terminal"), B_SettingsTests (the hooks switch says what is true, usage
// line and its stale mark), Integration_SettingsTests (suggestion reasons, the last change, the
// official app's hook chip only where its hooks are).

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');
const scripts = require('./lib/scripts.cjs');

const plain = scripts.plain;
const SETTINGS_JS = fs.readFileSync(path.join(scripts.DIR, 'settings.js'), 'utf8');
const SETTINGS_CSS = fs.readFileSync(path.join(scripts.DIR, 'settings.css'), 'utf8');
const PERSONAL = 'uuid:5f0c3a1e-0000-4000-8000-000000000001';
const WORK = 'uuid:8a7b6c5d-0000-4000-8000-000000000002/org-work-0002';
const WRITES = ['hook_consent', 'hooks_enabled', 'status_line_enabled', 'hooks_reinstall', 'remove_codenotch_hooks',
  'acknowledge_scope', 'account', 'choose_claude_binary', 'copy_text', 'reveal', 'pick_folder', 'launch_command', 'set_setting'];
/** rebrand.js's product phrases: copy about upstream's own products keeps its name. */
const KEPT = ['Codenotch app on your phone', 'Codenotch on your phone', 'Codenotch phone app',
  'Codenotch for Windows', 'Codenotch-Setup', 'hivinz.com'];

// ---- helpers ---------------------------------------------------------------------------------

function settingsWith(edit) {
  const settings = harness.fixture('settings.json');
  if (edit) edit(settings);
  return settings;
}

/** settings.html on the fixture (changed by `edit`), loaded, with the load's own calls forgotten. */
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

function silent(page, why) {
  assert.deepEqual(plain(page.hub.calls), [], why);
}

/** What is on screen: every text node, one space between blocks, runs of space collapsed. */
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
const acct = (page, id) => page.$$('#pane-claude .an-acct').find((el) => el.getAttribute('data-key') === `acct:${id}`);
const action = (root, name, arg) => root.querySelectorAll('[data-an-action]').find((el) =>
  el.getAttribute('data-an-action') === name && (arg === undefined || el.getAttribute('data-an-arg') === arg));
const calls = (page) => plain(page.hub.calls);

async function click(page, el) {
  assert.ok(el, 'the control is on the page');
  page.click(el);
  await page.settle();
}

function consentNeeded(s) {
  s.hooks.consent = null;
  s.hooks.enabled = false;
  s.hooks.enabled_locked = true;
  s.hooks.summary = 'Turn on Claude Code control first.';
  s.setup.hook_consent = null;
  s.setup.needs_hook_consent = true;
  s.setup.consent_files = [];
  for (let i = 0; i < 9; i++) s.setup.consent_files.push({ path: `~\\.claude-windows\\${i}a1b2c3d4e5f\\settings.json`, account: i % 2 ? 'me@work.example' : null });
}

/** Every text node and label attribute under `root` that still says "Codenotch" outside a kept phrase. */
function unbranded(root) {
  const left = [];
  const visit = (node) => {
    if (node.nodeType === 1 && (node.hasAttribute('data-user') || node.id === 'pane-claude' || /^(script|style)$/.test(node.localName))) return;
    if (node.nodeType === 3 && node.nodeValue.includes('Codenotch')) left.push(node.nodeValue);
    if (node.nodeType === 1) {
      for (const name of ['aria-label', 'title', 'placeholder']) {
        const v = node.getAttribute(name);
        if (v && v.includes('Codenotch')) left.push(`${name}=${v}`);
      }
    }
    for (const child of node.childNodes) visit(child);
  };
  visit(root);
  return left.filter((s) => !KEPT.some((p) => s.includes(p)));
}

// ---- the script and the frame ------------------------------------------------------------------

test('settings.js is one strict IIFE with one global and no top-level let, const or class', () => {
  assert.match(SETTINGS_JS, /^(\/\/[^\n]*\n)+\(function \(\) \{\n  'use strict';/);
  assert.match(SETTINGS_JS.trimEnd(), /\}\)\(\);$/);
  const globals = [...SETTINGS_JS.matchAll(/window\.(agentnotch[A-Za-z]*) =/g)].map((m) => m[1]);
  assert.deepEqual([...new Set(globals)], ['agentnotchSettings']);
  assert.doesNotMatch(SETTINGS_JS, /^(let|const|class) /m);
  assert.doesNotMatch(SETTINGS_JS, /\.innerHTML\s*=|insertAdjacentHTML|eval\(|new Function/);
});

test('settings.css scopes the fork\'s rules to its pane and keeps upstream\'s hooks switch hidden', () => {
  assert.match(SETTINGS_CSS, /\.acct-sub:has\(\[data-remote="hooks"\]\)/);
  const rules = SETTINGS_CSS.replace(/\/\*[\s\S]*?\*\//g, '').split('}').map((r) => r.split('{')[0].trim()).filter(Boolean);
  for (const selector of rules) {
    if (/^@media/.test(selector) || /^\.badge\.b-claude$|^\.acct-sub|^\s*\.acct-sub/.test(selector)) continue;
    for (const part of selector.split(',')) assert.match(part.trim(), /#pane-claude|^\.acct-sub|^@/, `unscoped rule: ${part}`);
  }
  assert.doesNotMatch(SETTINGS_CSS, /animation:[^;]*infinite/);
});

test('page order: upstream\'s script then settings.js in one context, no clash, no page error; the shared scripts are loaded by settings.js', async () => {
  const page = harness.loadPage('settings.html', {});
  await page.settle();
  clean(page);
  assert.deepEqual(page.loaded, ['agentnotch/settings.js', 'agentnotch/common.js', 'agentnotch/rebrand.js']);
  // Upstream's globals are still there, and ours beside them.
  assert.equal(page.run('typeof showTab + typeof toast + typeof ui + TABS.join()'), 'functionfunctionfunctionclaude,accounts,appearance,general');
  assert.equal(page.run('typeof agentnotchSettings.showScene + typeof agentnotchSettings.layoutReport'), 'functionfunction');
  // The Claude Code tab exists and is first.
  const tabs = page.$$('#side [role="tab"]').map((el) => el.id);
  assert.equal(tabs[0], 'tab-claude');
  assert.equal(text(page.$('#tab-claude')), 'Claude Code');
});

test('a load asks for the settings and nothing else; the tab opens on the pane only when consent is needed', async () => {
  const page = harness.loadPage('settings.html', {});
  await page.settle();
  assert.deepEqual(page.hub.calls.map((c) => c.method), ['settings']);
  assert.ok(page.listening('an:settings') && page.listening('an:cloud'));

  const first = harness.loadPage('settings.html', { settings: settingsWith(consentNeeded) });
  await first.settle();
  assert.deepEqual(first.hub.calls.map((c) => c.method), ['settings']);
  assert.equal(first.$('#pane-claude').hidden, false);
  assert.equal(first.$('#tab-claude').getAttribute('aria-selected'), 'true');
  clean(first);
});

test('the sections stand in the Mac\'s order, with named empty containers for the rest', async () => {
  const page = await open();
  const names = page.$$('#pane-claude > [data-an-section]').map((el) => el.getAttribute('data-an-section'));
  assert.deepEqual(names, ['status', 'consent', 'accounts', 'hooks', 'usage', 'cloud', 'attention', 'notifications', 'advanced']);
  for (const name of ['usage', 'cloud', 'attention', 'notifications', 'advanced']) assert.equal(region(page, name).hidden, true, name);
  assert.equal(region(page, 'consent').hidden, true);
  assert.equal(text(region(page, 'status')), 'Sealed: this window shows sample data.');
  const titles = page.$$('#pane-claude .sec').map(text);
  assert.deepEqual(titles, ['Accounts', 'Hooks and status line']);
});

test('the sealed line goes away when the hub is not sealed; a hub that does not answer is said', async () => {
  const page = await open((s) => { s.sealed = false; });
  assert.equal(region(page, 'status').hidden, true);

  const down = await open(null, { replies: { settings: () => { throw { code: 'failed', message: 'The pipe is closed.' }; } } });
  assert.equal(text(region(down, 'status')), "Claude Code control isn't running. The pipe is closed.");
  assert.equal(region(down, 'accounts').hidden, true);
  // A snapshot that arrives later replaces the failure.
  down.emit('an:settings', settingsWith());
  assert.equal(text(region(down, 'status')), 'Sealed: this window shows sample data.');
  assert.equal(region(down, 'accounts').hidden, false);
});

test('an:settings re-renders in place: a half-typed name and its field survive', async () => {
  const page = await open();
  await click(page, action(acct(page, WORK), 'rename', WORK));
  const field = page.$('#pane-claude [data-an-field="rename"]');
  page.type(field, 'Day job');
  page.emit('an:settings', settingsWith((s) => { s.accounts[1].usage_line = '5-hour 80% · weekly 60% · just now'; }));
  const after = page.$('#pane-claude [data-an-field="rename"]');
  assert.equal(after, field, 'the same element');
  assert.equal(after.value, 'Day job');
  assert.match(text(acct(page, WORK)), /5-hour 80%/);
  silent(page);
});

test('an:cloud before the first settings is kept for it; after it, it replaces the cloud part only', async () => {
  const page = harness.loadPage('settings.html', { replies: { settings: () => new Promise(() => {}) } });
  await page.settle();
  page.emit('an:cloud', { auth: { signed_out: {} } });
  // A settings snapshot without its own cloud part takes the one that came first.
  page.emit('an:settings', settingsWith((x) => { delete x.cloud; }));
  assert.deepEqual(plain(page.run('agentnotchSettings._.state.settings.cloud')), { auth: { signed_out: {} } });
  page.emit('an:cloud', { auth: { signing_in: {} } });
  assert.deepEqual(plain(page.run('agentnotchSettings._.state.settings.cloud')), { auth: { signing_in: {} } });
  assert.equal(page.run('agentnotchSettings._.state.settings.accounts.length'), 2);
});

// ---- rebrand at run time ---------------------------------------------------------------------

test('rebrand over settings.html: title, sidebar, every tab, ui() wrapped; [data-user] and the pane untouched', async () => {
  const page = await open();
  clean(page);
  assert.equal(page.document.title, 'Agent Notch Settings');
  assert.equal(text(page.$('#quit')), 'Quit Agent Notch');
  assert.equal(page.run('ui("no.such.key", "Open Codenotch at login")'), 'Open Agent Notch at login');
  assert.equal(page.run('ui("no.such.key", "Get the Codenotch app on your phone")'), 'Get the Codenotch app on your phone');
  assert.equal(page.run('ui.__anRebranded'), true);
  assert.deepEqual(unbranded(page.$('#app')), []);
  for (const tab of ['accounts', 'appearance', 'general', 'claude']) {
    page.run(`showTab(${JSON.stringify(tab)})`);
    await page.settle();
    assert.deepEqual(unbranded(page.$('#app')), [], tab);
  }
  // Later changes are rebranded through the observer; a [data-user] node never is.
  const host = page.$('#acc-on');
  const added = page.document.createElement('div');
  added.textContent = 'Codenotch reads this';
  const user = page.document.createElement('div');
  user.setAttribute('data-user', '');
  user.textContent = 'Codenotch Fan Club';
  user.setAttribute('title', 'Codenotch Fan Club');
  host.appendChild(added);
  host.appendChild(user);
  await page.settle();
  assert.equal(added.textContent, 'Agent Notch reads this');
  assert.equal(user.textContent, 'Codenotch Fan Club');
  assert.equal(user.getAttribute('title'), 'Codenotch Fan Club');
  // The pane names the official app on purpose and is never rewritten.
  const s = settingsWith((x) => { x.setup.codenotch_hooks_folders = ['C:\\Users\\me\\.claude']; });
  page.emit('an:settings', s);
  await page.settle();
  assert.match(text(region(page, 'hooks')), /Remove Codenotch's hooks/);
  clean(page);
});

test('the static page is rebranded by the walk alone, before upstream relabels anything', async () => {
  // Upstream's language never arrives, so nothing passes through ui(): only the walk of #app.
  const page = await open(null, { commands: { get_lang_resolved: () => new Promise(() => {}), get_lang: () => new Promise(() => {}) } });
  assert.equal(text(page.$('#quit')), 'Quit Agent Notch');
  assert.deepEqual(unbranded(page.$('#app')), []);
  clean(page);
});

test('the rebrand does not loop on its own writes', async () => {
  const page = await open();
  const title = page.$('#title');
  let writes = 0;
  const MO = page.window.MutationObserver;
  const watcher = new MO(() => { writes += 1; });
  watcher.observe(title, { childList: true, characterData: true, subtree: true });
  title.textContent = 'Codenotch Settings';
  for (let i = 0; i < 5; i++) await page.settle();
  assert.equal(title.textContent, 'Agent Notch Settings');
  assert.ok(writes <= 3, `${writes} mutation rounds`);
});

// ---- 1. consent card and scope notice --------------------------------------------------------

test('the consent card: copy, the WHOLE file list with accounts, Turn on emphasised and never a default', async () => {
  const page = await open(consentNeeded);
  const card = region(page, 'consent');
  assert.equal(card.hidden, false);
  const t = text(card);
  assert.match(t, /^Turn on Claude Code control /);
  assert.ok(t.includes('each folder Claude Code runs in'));
  assert.ok(t.includes('Nothing is written until you turn it on, and turning it off puts your status line back exactly.'));
  const files = card.querySelectorAll('.an-file').map(text);
  assert.equal(files.length, 9, 'not capped');
  assert.equal(files[0], '~\\.claude-windows\\0a1b2c3d4e5f\\settings.json');
  assert.equal(files[1], '~\\.claude-windows\\1a1b2c3d4e5f\\settings.json (me@work.example)');
  assert.ok(card.querySelector('.an-files').hasAttribute('data-user'));
  const on = action(card, 'consent-on');
  assert.equal(text(on), 'Turn on');
  assert.ok(on.classList.contains('an-primary'));
  assert.ok(on.hasAttribute('data-an-noenter'));
  assert.equal(on.getAttribute('type'), 'button');
  assert.equal(page.$$('#pane-claude [autofocus]').length, 0);
  assert.equal(text(action(card, 'consent-later')), 'Not now');
  silent(page, 'drawing the card sends nothing');
});

test('Turn on sends hook_consent {grant:true} exactly once, and only from its own click', async () => {
  const page = await open(consentNeeded);
  const card = region(page, 'consent');
  const on = action(card, 'consent-on');
  // Enter on the button itself, and Enter in every field, send nothing.
  const enter = page.fire(on, 'keydown', { key: 'Enter' });
  assert.equal(enter.defaultPrevented, true);
  await click(page, action(acct(page, WORK), 'rename', WORK));
  page.fire(page.$('#pane-claude [data-an-field="rename"]'), 'keydown', { key: 'Escape' });
  await click(page, action(region(page, 'hooks'), 'binary-open'));
  const binary = page.$('#pane-claude [data-an-field="binary"]');
  page.type(binary, 'C:\\claude.exe');
  page.fire(binary, 'keydown', { key: 'Enter' });
  await page.settle();
  silent(page, 'nothing on Enter');
  await click(page, action(region(page, 'hooks'), 'binary-cancel'));

  await click(page, action(region(page, 'consent'), 'consent-on'));
  assert.deepEqual(calls(page), [{ method: 'hook_consent', args: { grant: true } }]);
  // The card is gone at once; a second click has nothing to press, and the engine's next
  // snapshot (still asking, as a slow engine may) does not bring a second call either.
  assert.equal(region(page, 'consent').hidden, true);
  page.click(on);
  page.emit('an:settings', settingsWith(consentNeeded));
  await page.settle();
  assert.equal(page.hub.of('hook_consent').length, 1);
  clean(page);
});

test('Not now sends hook_consent {grant:false}; a refused answer brings the card back with the message', async () => {
  const page = await open(consentNeeded);
  await click(page, action(region(page, 'consent'), 'consent-later'));
  assert.deepEqual(calls(page), [{ method: 'hook_consent', args: { grant: false } }]);

  const sealed = await open(consentNeeded, { replies: { hook_consent: () => { throw { code: 'sealed', message: 'Sealed: hook_consent does nothing here.' }; } } });
  await click(sealed, action(region(sealed, 'consent'), 'consent-on'));
  assert.equal(sealed.hub.of('hook_consent').length, 1, 'no retry');
  assert.equal(region(sealed, 'consent').hidden, false);
  assert.match(text(region(sealed, 'consent')), /Sealed: hook_consent does nothing here\./);
});

test('installing off for this run: the card says so and Turn on cannot be pressed', async () => {
  const page = await open((s) => { consentNeeded(s); s.setup.install_disabled = true; s.hooks.install_allowed = false; });
  const card = region(page, 'consent');
  assert.match(text(card), /Installing is off for this run \(--no-install\)\./);
  const on = action(card, 'consent-on');
  assert.ok(on.hasAttribute('disabled'));
  await click(page, on);
  silent(page);
});

test('the official app\'s hooks stay: the card says where to remove them', async () => {
  const page = await open((s) => { consentNeeded(s); s.setup.codenotch_hooks_folders = ['C:\\Users\\me\\.claude']; });
  assert.match(text(region(page, 'consent')), /Codenotch's hooks stay; remove them per folder under Hooks and status line\./);
});

test('the scope notice: copy, the folders, OK -> acknowledge_scope, Turn off -> hooks_enabled {on:false}', async () => {
  const scoped = (s) => { s.setup.new_install_folders = ['VS Code · dotfiles', '~\\.claude-windows\\c07a3f5e1d94', '~\\.claude-windows\\9b4f2e8d6a10']; };
  const page = await open(scoped);
  const notice = region(page, 'consent');
  const t = text(notice);
  assert.match(t, /^Claude Code control now covers your VS Code workspaces /);
  assert.ok(t.includes("This version puts its hooks and status line in 3 VS Code workspaces' folders too"));
  assert.ok(t.includes('Account stores never get hooks.') && !t.includes('untouched'));
  assert.deepEqual(notice.querySelectorAll('.an-file').map(text), ['VS Code · dotfiles', '~\\.claude-windows\\c07a3f5e1d94', '~\\.claude-windows\\9b4f2e8d6a10']);
  silent(page);
  await click(page, action(notice, 'scope-ok'));
  assert.deepEqual(calls(page), [{ method: 'acknowledge_scope', args: null }]);
  assert.equal(region(page, 'consent').hidden, true);

  const off = await open(scoped);
  await click(off, action(region(off, 'consent'), 'scope-off'));
  assert.deepEqual(calls(off), [{ method: 'hooks_enabled', args: { on: false } }]);

  const one = await open((s) => { s.setup.new_install_folders = ['VS Code · dotfiles']; });
  assert.ok(text(region(one, 'consent')).includes("1 VS Code workspace's folder too"));
});

test('control off: the Hooks section offers Turn on…, which shows the full card inline; it never writes blind', async () => {
  const page = await open((s) => { s.hooks.consent = false; s.hooks.enabled = false; s.setup.hook_consent = false; });
  const hooks = region(page, 'hooks');
  assert.match(text(hooks), /Claude Code control is off Nothing is written to any settings\.json\. Sessions still show from Claude Code's session files, without approvals or “done”\./);
  await click(page, action(hooks, 'reconsider'));
  silent(page, 'Turn on… only shows the card');
  assert.equal(region(page, 'hooks').querySelectorAll('.an-file').length, 2);
  await click(page, action(region(page, 'hooks'), 'consent-on', 'hooks'));
  assert.deepEqual(calls(page), [{ method: 'hook_consent', args: { grant: true } }]);
});

// ---- 2. accounts ---------------------------------------------------------------------------

test('account rows from the fixture: dot, name, default caption, identity, summary, usage, chips, switches, buttons', async () => {
  const page = await open();
  const rows = page.$$('#pane-claude .an-acct');
  assert.equal(rows.length, 2);
  const personal = acct(page, PERSONAL);
  assert.equal(text(personal.querySelector('.an-name')), 'Personal');
  assert.equal(text(personal.querySelector('[data-key="default"]')), 'Default');
  assert.equal(text(personal.querySelector('[data-key="identity"]')), 'me@personal.example · Max 5x');
  assert.equal(text(personal.querySelector('[data-key="summary"]')), 'Runs in ~\\.claude');
  assert.equal(text(personal.querySelector('[data-key="usage"]')), '5-hour 34% · weekly 41% · 4m ago');
  assert.deepEqual(personal.querySelectorAll('.an-chip').map(text), ['Hooks installed', 'Live status line']);
  assert.ok(personal.querySelector('.an-chip').classList.contains('an-tone-ok'));
  assert.ok(personal.querySelector('.an-sdot').getAttribute('style').includes('#5C9EFA'));
  assert.deepEqual(personal.querySelectorAll('.btn').map(text), ['Rename…', 'Reinstall hooks', 'Copy launch command', 'Show in Explorer']);
  const work = acct(page, WORK);
  assert.deepEqual(work.querySelectorAll('.btn').map(text), ['Rename…', 'Reinstall hooks', 'Copy launch command', 'Show in Explorer', 'Forget…']);
  assert.equal(work.querySelector('[data-key="default"]'), null);
  for (const row of rows) {
    for (const sel of ['.an-name', '[data-key="identity"]', '[data-key="summary"]']) assert.ok(row.querySelector(sel).hasAttribute('data-user'), sel);
  }
  // Folders, when shown: title, role, state, and the status-line note.
  await click(page, action(work, 'folders', WORK));
  silent(page, 'showing folders asks nothing');
  const folder = acct(page, WORK).querySelector('.an-folder');
  assert.equal(text(folder), '~\\.claude-work Folder Hooks installed');
  assert.match(text(acct(page, WORK).querySelector('.an-folders')), /Status line left alone: its command uses Windows paths/);
  assert.equal(text(action(acct(page, WORK), 'folders', WORK)), 'Hide folders');
});

test('Parallel Profiles: "In ~\\.claude now", workspace names, roles, stores, and the guidance-first New account', async () => {
  const page = await open();
  assert.equal(page.run('agentnotchSettings.showScene("settings-parallel-profiles")'), true);
  const personal = acct(page, PERSONAL);
  const holder = personal.querySelector('[data-key="default"]');
  assert.equal(text(holder), 'In ~\\.claude now');
  assert.match(holder.getAttribute('title'), /Claude Parallel Profiles copies the focused VS Code window's account into ~\\\.claude\./);
  assert.equal(text(personal.querySelector('[data-key="summary"]')), 'Runs in ~\\.claude and 1 VS Code workspace Store (Claude Parallel Profiles): ~\\.claude-me');
  const folders = personal.querySelectorAll('.an-folder').map((f) => f.children.map(text));
  assert.deepEqual(folders, [
    ['~\\.claude', 'Default', 'Hooks and live status line'],
    ['VS Code · dotfiles', '~\\.claude-windows\\5d1e0a7b3c21', 'Hooks and live status line'],
    ['~\\.claude-me', 'Account store', 'Read only, never changed'],
  ]);
  assert.ok(personal.querySelectorAll('.an-folder-state')[2].classList.contains('an-tertiary'));
  assert.match(text(acct(page, WORK)), /Its sessions still show; answer their prompts where Claude Code runs \(VS Code or the terminal\) until the hooks are in\./);
  assert.match(text(region(page, 'accounts')), /Add an account in VS Code Claude Parallel Profiles adds accounts: open a VS Code window, sign in with Claude Code's account menu or \/login/);
  assert.ok(text(region(page, 'accounts')).includes('reloads it'));
  assert.ok(text(region(page, 'accounts')).includes('(~\\.claude-<name>)'));
  // The unsigned workspace gets the status bar caption.
  assert.match(text(region(page, 'accounts')), /Not signed in ~\\\.claude-windows\\0a1b2c3d4e5f: Claude Code runs here, but nobody is signed in yet\. Pick an account for that window from the Claude Parallel Profiles status bar item/);
  // The footer's Parallel Profiles variant.
  assert.match(text(page.$('#pane-claude .an-footer')), /~\\\.claude keeps them while another account is tracked/);
  // A scene sends nothing, even when clicked.
  await click(page, action(acct(page, WORK), 'install', WORK));
  silent(page);
});

test('an unsigned plain folder gets the /login caption, not the status bar one', async () => {
  const page = await open((s) => { s.unsigned_folders = ['~\\.claude-new']; });
  const t = text(page.$('#pane-claude .an-unsigned'));
  assert.equal(t, 'Not signed in ~\\.claude-new: Claude Code runs here, but nobody is signed in yet. It gets a ring once someone signs in with /login.');
});

test('an untracked account: faded dot, ring switch off and disabled with its reason, the caption under Track', async () => {
  const page = await open((s) => {
    const w = s.accounts[1];
    w.is_tracked = false;
    w.ring_shown = true;
    w.usage_line = "Not checked while it isn't tracked";
    w.hook_state = 'Hooks off';
    w.hook_state_tone = 'neutral';
  });
  const work = acct(page, WORK);
  assert.ok(work.classList.contains('off'));
  assert.ok(work.querySelector('.an-sdot').classList.contains('an-faded'));
  const ring = action(work, 'ring-shown', WORK);
  assert.equal(ring.getAttribute('aria-checked'), 'false');
  assert.ok(ring.hasAttribute('disabled'));
  assert.equal(ring.getAttribute('title'), 'Needs Track sessions and hooks');
  assert.match(text(work), /Ring in notch needs Track sessions and hooks/);
  assert.match(text(work), /Not checked while it isn't tracked/);
  await click(page, ring);
  silent(page);
});

test('usage line: what the engine says, with ", stale" and the stale class when stale', async () => {
  const page = await open((s) => { s.accounts[0].usage_stale = true; s.accounts[1].usage_line = "Not checked while its ring is off"; });
  const usage = acct(page, PERSONAL).querySelector('[data-key="usage"]');
  assert.equal(text(usage), '5-hour 34% · weekly 41% · 4m ago, stale');
  assert.ok(usage.classList.contains('an-stale'));
  assert.equal(text(acct(page, WORK).querySelector('[data-key="usage"]')), 'Not checked while its ring is off');
});

test('hook problem: amber, or critical when the engine says so', async () => {
  const page = await open((s) => {
    s.accounts[0].hook_problem = 'Hooks are turned off (see Hooks below).';
    s.accounts[1].hook_problem = 'settings.json can\'t be read: it isn\'t valid JSON.';
    s.accounts[1].hook_problem_critical = true;
    s.accounts[1].hook_state = 'settings.json unreadable';
    s.accounts[1].hook_state_tone = 'critical';
  });
  assert.ok(acct(page, PERSONAL).querySelector('[data-key="problem"]').classList.contains('an-tone-warning'));
  const p = acct(page, WORK).querySelector('[data-key="problem"]');
  assert.ok(p.classList.contains('an-tone-critical'));
  assert.ok(acct(page, WORK).querySelector('.an-chip').classList.contains('an-tone-critical'));
});

test('the official app\'s hook chip shows only where one of the account\'s folders has its hooks', async () => {
  const page = await open((s) => { s.accounts[1].folders[0].codenotch_hooks = true; });
  assert.deepEqual(acct(page, PERSONAL).querySelectorAll('.an-chip').map(text), ['Hooks installed', 'Live status line']);
  assert.deepEqual(acct(page, WORK).querySelectorAll('.an-chip').map(text), ['Hooks installed', 'Codenotch hooks']);
});

test('every account action sends its exact call, from its own click', async () => {
  const page = await open();
  const work = () => acct(page, WORK);

  await click(page, action(work(), 'track', WORK));
  await click(page, action(work(), 'ring-shown', WORK));
  await click(page, action(work(), 'install', WORK));
  await click(page, action(work(), 'reveal', WORK));
  assert.deepEqual(calls(page), [
    { method: 'account', args: { action: { track: { id: WORK, on: false } } } },
    { method: 'account', args: { action: { ring_shown: { ring_id: 'claude-acct-8a7b6c5d4e3f', on: false } } } },
    { method: 'hooks_reinstall', args: { account_id: WORK } },
    { method: 'reveal', args: { kind: 'config_dir', id: 'C:\\Users\\me\\.claude-work' } },
  ]);
  page.hub.clear();

  // Rename: the field starts with the nickname; Save sends it; an empty name goes back to the default.
  await click(page, action(work(), 'rename', WORK));
  silent(page);
  const field = page.$('#pane-claude [data-an-field="rename"]');
  assert.equal(field.value, 'Work');
  assert.equal(field.getAttribute('placeholder'), 'Claude Work');
  page.type(field, '  Day job ');
  await click(page, action(work(), 'rename-save', WORK));
  await click(page, action(acct(page, PERSONAL), 'rename', PERSONAL));
  page.type(page.$('#pane-claude [data-an-field="rename"]'), '');
  page.fire(page.$('#pane-claude [data-an-field="rename"]'), 'keydown', { key: 'Enter' });
  await page.settle();
  assert.deepEqual(calls(page), [
    { method: 'account', args: { action: { rename: { id: WORK, label: 'Day job' } } } },
    { method: 'account', args: { action: { rename: { id: PERSONAL, label: null } } } },
  ]);
  page.hub.clear();

  // Forget asks in the page first, with its caption; Cancel sends nothing.
  await click(page, action(work(), 'forget-ask', WORK));
  silent(page);
  assert.match(text(work()), /This app's hooks are removed from its settings\.json and it stops being tracked\. The folder and its sessions are left alone\./);
  await click(page, action(work(), 'forget-cancel', WORK));
  assert.equal(action(work(), 'forget', WORK), undefined);
  await click(page, action(work(), 'forget-ask', WORK));
  const forget = action(work(), 'forget', WORK);
  assert.equal(text(forget), 'Forget and remove hooks');
  assert.ok(forget.classList.contains('an-danger') && forget.hasAttribute('data-an-noenter'));
  await click(page, forget);
  assert.deepEqual(calls(page), [{ method: 'account', args: { action: { forget: { id: WORK } } } }]);
  // The sealed answer is shown, not retried.
  assert.match(text(region(page, 'accounts')), /Sealed: account does nothing here\./);
  assert.equal(page.hub.of('account').length, 1);
  page.hub.clear();

  // Suggestions.
  await click(page, action(region(page, 'accounts'), 'suggestion-dismiss', '~\\.claude-old'));
  await click(page, action(region(page, 'accounts'), 'suggestion-add', '~\\.claude-old'));
  assert.deepEqual(calls(page), [
    { method: 'account', args: { action: { suggestion_dismiss: { path: '~\\.claude-old' } } } },
    { method: 'account', args: { action: { suggestion_add: { path: '~\\.claude-old' } } } },
  ]);
  clean(page);
});

test('Copy launch command asks the hub for the command, copies it, says Copied for 1.5 s and in the toast', async () => {
  const page = await open();
  const button = action(acct(page, WORK), 'copy-launch', WORK);
  assert.equal(button.getAttribute('title'), "$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-work'; claude");
  await click(page, button);
  assert.deepEqual(calls(page), [
    { method: 'launch_command', args: { account_id: WORK } },
    { method: 'copy_text', args: { text: "$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-work'; claude" } },
  ]);
  assert.equal(text(action(acct(page, WORK), 'copy-launch', WORK)), 'Copied');
  assert.equal(text(page.$('#toast')), 'Copied');
  page.tick(1600);
  assert.equal(text(action(acct(page, WORK), 'copy-launch', WORK)), 'Copy launch command');
});

test('Add existing folder…: the picker, then add_folder with its path; a cancelled pick adds nothing', async () => {
  const page = await open();
  await click(page, action(region(page, 'accounts'), 'add-folder'));
  assert.deepEqual(calls(page), [{ method: 'pick_folder', args: null }]);
  page.hub.clear();
  page.hub.replies.pick_folder = { path: 'D:\\claude-profiles\\.claude-lab' };
  await click(page, action(region(page, 'accounts'), 'add-folder'));
  assert.deepEqual(calls(page), [
    { method: 'pick_folder', args: null },
    { method: 'account', args: { action: { add_folder: { path: 'D:\\claude-profiles\\.claude-lab' } } } },
  ]);
});

test('New account…: name it, Create sends create {name}, the returned command is shown and copied', async () => {
  const page = await open(null, { replies: { account: (args) => (args.action.create ? { created: '~\\.claude-side', command: "$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-side'; claude" } : {}) } });
  await click(page, action(region(page, 'accounts'), 'add-open'));
  const field = page.$('#pane-claude [data-an-field="new-account"]');
  assert.equal(field.getAttribute('placeholder'), 'Name, for example work');
  assert.ok(action(region(page, 'accounts'), 'add-create').hasAttribute('disabled'));
  page.type(field, 'My Side');
  await page.settle();
  assert.match(text(region(page, 'accounts')), /Creates ~\\\.claude-my-side, a separate Claude Code config folder you sign in to once\./);
  silent(page, 'typing sends nothing');
  await click(page, action(region(page, 'accounts'), 'add-create'));
  assert.deepEqual(calls(page), [
    { method: 'account', args: { action: { create: { name: 'My Side' } } } },
    { method: 'copy_text', args: { text: "$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-side'; claude" } },
  ]);
  const t = text(region(page, 'accounts'));
  assert.ok(t.includes("Created ~\\.claude-side Run this in a terminal, then /login. It's on your clipboard."));
  assert.ok(t.includes("$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-side'; claude"));
  await click(page, action(region(page, 'accounts'), 'add-close'));
  assert.equal(page.$('#pane-claude [data-an-field="new-account"]'), null);

  // A refusal stays under the field, in the critical tone.
  const sealed = await open();
  await click(sealed, action(region(sealed, 'accounts'), 'add-open'));
  sealed.type(sealed.$('#pane-claude [data-an-field="new-account"]'), 'side');
  sealed.fire(sealed.$('#pane-claude [data-an-field="new-account"]'), 'keydown', { key: 'Enter' });
  await sealed.settle();
  assert.equal(sealed.hub.of('account').length, 1);
  assert.equal(text(sealed.$('#pane-claude .an-add [data-key="err"]')), 'Sealed: account does nothing here.');
});

test('no accounts: the Mac\'s empty line', async () => {
  const page = await open((s) => { s.accounts = []; s.suggestions = []; });
  assert.match(text(region(page, 'accounts')), /No Claude Code accounts yet\. Add the folder Claude Code uses, or create a new account\./);
});

// ---- 3. hooks and status line ------------------------------------------------------------------

test('the hooks section from the fixture: switches, summary, pipe, Claude Code, last change, both footnotes', async () => {
  const page = await open();
  const hooks = region(page, 'hooks');
  const sw = action(hooks, 'hooks-enabled');
  assert.equal(sw.getAttribute('aria-checked'), 'true');
  assert.equal(sw.hasAttribute('disabled'), false);
  assert.match(text(hooks), /Hooks in tracked accounts Installed in all 2 tracked accounts\./);
  assert.equal(action(hooks, 'status-line').getAttribute('aria-checked'), 'true');
  assert.match(text(hooks), /Live status line data Wraps each account's status line to read limits and context live\. Turning it off restores your status line exactly\./);
  assert.equal(text(hooks.querySelector('[data-key="pipe"]')), 'Pipe \\\\.\\pipe\\agentnotch-hook-S-1-5-21-1000000001-1000000002-1000000003-1001');
  assert.match(text(hooks.querySelector('[data-key="claude"]')), /^Claude Code Hooks are written for the oldest claude found, so every version reads them\. C:\\Users\\me\\\.local\\bin\\claude\.exe Version 2\.1\.282 Choose…$/);
  assert.equal(action(hooks, 'binary-auto'), undefined, 'Find automatically only after a path was chosen');
  assert.match(text(hooks.querySelector('[data-key="last-change"]')), /^Last change: settings\.json in ~\\\.claude-work\./);
  assert.deepEqual(hooks.querySelectorAll('.an-footnote').map(text), [
    "Sessions inside WSL aren't tracked yet.",
    "Uninstalling keeps Claude Code's hooks unless you tick Delete the application data; turn Claude Code control off first to remove them.",
  ]);
});

test('the hooks switches send their calls; locked, without consent, without install or while busy they send nothing', async () => {
  const page = await open();
  await click(page, action(region(page, 'hooks'), 'hooks-enabled'));
  await click(page, action(region(page, 'hooks'), 'status-line'));
  assert.deepEqual(calls(page), [
    { method: 'hooks_enabled', args: { on: false } },
    { method: 'status_line_enabled', args: { on: false } },
  ]);
  const cases = [
    ['Turn on Claude Code control first.', (s) => { s.hooks.consent = null; s.hooks.enabled = false; s.hooks.summary = 'Turn on Claude Code control first.'; }],
    ['Installing is off for this run (--no-install).', (s) => { s.hooks.install_allowed = false; s.setup.install_disabled = true; s.hooks.summary = 'Installing is off for this run (--no-install).'; }],
    ['Changing settings.json files…', (s) => { s.hooks.busy = true; }],
  ];
  for (const [said, edit] of cases) {
    const p = await open(edit);
    const sw = action(region(p, 'hooks'), 'hooks-enabled');
    assert.ok(sw.hasAttribute('disabled'), said);
    assert.ok(text(region(p, 'hooks')).includes(said), said);
    await click(p, sw);
    await click(p, action(acct(p, WORK), 'install', WORK));
    assert.equal(p.hub.of('hooks_enabled').length + p.hub.of('hooks_reinstall').length, 0, said);
  }
  const locked = await open((s) => { s.hooks.enabled_locked = true; });
  await click(locked, action(region(locked, 'hooks'), 'hooks-enabled'));
  silent(locked);
  const warn = await open((s) => { s.hooks.summary = 'Installed in 1 of 2 tracked accounts.'; s.hooks.summary_warning = true; });
  assert.ok(region(warn, 'hooks').querySelector('[data-key="summary"]').classList.contains('an-tone-warning'));
});

test('Claude Code: a typed path goes to choose_claude_binary, quotes taken off; Find automatically sends null', async () => {
  const page = await open((s) => { s.hooks.claude_path_chosen = true; });
  await click(page, action(region(page, 'hooks'), 'binary-open'));
  silent(page);
  const field = page.$('#pane-claude [data-an-field="binary"]');
  assert.equal(field.getAttribute('aria-label'), 'Path to claude.exe');
  assert.ok(action(region(page, 'hooks'), 'binary-use').hasAttribute('disabled'));
  page.type(field, '"D:\\tools\\claude.exe"');
  await click(page, action(region(page, 'hooks'), 'binary-use'));
  assert.deepEqual(calls(page), [{ method: 'choose_claude_binary', args: { path: 'D:\\tools\\claude.exe' } }]);
  assert.equal(page.$('#pane-claude [data-an-field="binary"]'), null, 'closed after a version answered');
  page.hub.clear();
  await click(page, action(region(page, 'hooks'), 'binary-auto'));
  assert.deepEqual(calls(page), [{ method: 'choose_claude_binary', args: { path: null } }]);

  // No Claude Code at that path: the field stays, with the reason.
  const none = await open(null, { replies: { choose_claude_binary: { version: null } } });
  await click(none, action(region(none, 'hooks'), 'binary-open'));
  none.type(none.$('#pane-claude [data-an-field="binary"]'), 'C:\\nothing.exe');
  await click(none, action(region(none, 'hooks'), 'binary-use'));
  assert.ok(none.$('#pane-claude [data-an-field="binary"]'));
  assert.match(text(region(none, 'hooks')), /No Claude Code answered at that path\./);
});

test('Remove Codenotch\'s hooks: one button per folder, its call, the count in the toast', async () => {
  const folders = ['C:\\Users\\me\\.claude', 'C:\\Users\\me\\.claude-windows\\9b4f2e8d6a10'];
  const page = await open((s) => { s.setup.codenotch_hooks_folders = folders; }, { replies: { remove_codenotch_hooks: { removed: 3 } } });
  const hooks = region(page, 'hooks');
  assert.match(text(hooks), /The official Codenotch has its own hooks in 2 folders\. They run beside this app's; removing them takes out only its entries\./);
  const buttons = hooks.querySelectorAll('[data-an-action="remove-codenotch"]');
  assert.deepEqual(buttons.map(text), ["Remove Codenotch's hooks", "Remove Codenotch's hooks"]);
  assert.ok(buttons.every((b) => b.hasAttribute('data-an-noenter')));
  await click(page, buttons[1]);
  assert.deepEqual(calls(page), [{ method: 'remove_codenotch_hooks', args: { folder: folders[1] } }]);
  assert.equal(text(page.$('#toast')), 'Removed 3 Codenotch hook entries');

  const busy = await open((s) => { s.setup.codenotch_hooks_folders = folders; s.hooks.busy = true; });
  await click(busy, action(region(busy, 'hooks'), 'remove-codenotch', folders[0]));
  silent(busy);
});

// ---- hostile strings ---------------------------------------------------------------------------

test('hostile strings in names, emails, organisations, folders, problems, the last change and the claude path stay text', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await open((s) => {
      const a = s.accounts[1];
      a.label = evil;
      a.default_label = evil;
      a.identity_line = `${evil} · ${evil}`;
      a.folder_summary = evil;
      a.usage_line = evil;
      a.hook_state = evil;
      a.hook_problem = evil;
      a.forget_caption = evil;
      a.launch_command = evil;
      a.folders[0].title = evil;
      a.folders[0].id = evil;
      a.folders[0].role = evil;
      a.folders[0].status_line_note = evil;
      s.suggestions = [{ path: evil, reason: evil }];
      s.unsigned_folders = [evil];
      s.hooks.last_change = evil;
      s.hooks.claude_path = evil;
      s.hooks.claude_caption = evil;
      s.hooks.pipe_name = evil;
      s.hooks.summary = evil;
      s.setup.codenotch_hooks_folders = [evil];
      s.setup.consent_files = [{ path: evil, account: evil }];
      s.setup.new_install_folders = [evil];
    });
    await click(page, action(acct(page, WORK), 'folders', WORK));
    await click(page, action(acct(page, WORK), 'forget-ask', WORK));
    const markup = page.$('#pane-claude').innerHTML;
    assert.deepEqual(audit.problems(markup), [], evil.slice(0, 40));
    clean(page);
  }
  assert.notDeepEqual(audit.problems(audit.HOSTILE[0]), [], 'the check can fail');
});

test('a consent card with hostile files and accounts stays text', async () => {
  for (const evil of audit.HOSTILE) {
    const page = await open((s) => { consentNeeded(s); s.setup.consent_files = [{ path: evil, account: evil }]; });
    assert.deepEqual(audit.problems(region(page, 'consent').innerHTML), []);
    silent(page);
  }
});

// ---- scenes ------------------------------------------------------------------------------------

test('the scenes draw, send nothing and ignore clicks; unknown names are refused', async () => {
  const scenes = JSON.parse(fs.readFileSync(path.join(__dirname, 'scenes.json'), 'utf8')).scenes
    .filter((s) => s.page === 'settings.html').map((s) => s.scene);
  for (const name of ['settings-full', 'settings-first-run', 'settings-scope-notice', 'settings-parallel-profiles', 'settings-unnamed-accounts']) {
    assert.ok(scenes.includes(name), `${name} is in scenes.json`);
    const page = await open();
    assert.equal(page.run(`agentnotchSettings.showScene(${JSON.stringify(name)})`), true, name);
    assert.ok(page.document.documentElement.classList.contains('an-static'));
    for (const el of page.$$('#pane-claude [data-an-action]')) page.click(el);
    await page.settle();
    silent(page, name);
    clean(page);
    const report = plain(page.run('agentnotchSettings.layoutReport()'));
    assert.equal(typeof report.ok, 'boolean');
    assert.ok(Array.isArray(report.failures));
  }
  const page = await open();
  assert.equal(page.run('agentnotchSettings.showScene("nope")'), false);
  assert.equal(page.run('agentnotchSettings.showScene("first-run")'), false);
  const first = await open();
  first.run('agentnotchSettings.showScene("settings-first-run")');
  assert.equal(region(first, 'consent').querySelectorAll('.an-file').length, 2);
  const scope = await open();
  scope.run('agentnotchSettings.showScene("settings-scope-notice")');
  assert.match(text(region(scope, 'consent')), /Claude Code control now covers your VS Code workspaces/);
  const unnamed = await open();
  unnamed.run('agentnotchSettings.showScene("settings-unnamed-accounts")');
  assert.deepEqual(unnamed.$$('#pane-claude .an-acct .an-name').map(text), ['Claude Personal', 'Claude Work']);
});

test('every write method this pane uses is one the settings window may make', () => {
  for (const method of WRITES) assert.ok(require('./lib/contract.cjs').allowedFromWindow('settings', method), method);
});
