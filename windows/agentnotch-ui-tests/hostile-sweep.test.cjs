'use strict';
// Hostile text through EVERY field at once. The page suites each feed audit.HOSTILE through the
// fields they know of; this one does not choose: every string of snapshot.json, settings.json and
// chat.json gets a hostile string appended (session titles and ids, account labels, emails,
// paths, requests, questions and options, notices, transcript text, tool input and output,
// cloud addresses...), and every scene of scenes.json and every session's chat is drawn from
// that. A field a renderer forgets to escape, today or after a fixture gains one, shows up here.
//
// Left as they are: enum-like values (`needs_you`, `permission`, `ok`), which select code paths
// rather than being drawn, and ring ids, which the engine makes (`claude-acct-<hex>`) and which
// upstream's own notch code puts in a selector.

const test = require('node:test');
const assert = require('node:assert/strict');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');

const SCENES = require('./scenes.json').scenes;
const ENUM = /^[a-z0-9_]+$/;
/**
 * One hostile string per way out: markup in text, out of a double- and a single-quoted
 * attribute, a new attribute, closing the containers, a markdown `javascript:` link. (The page
 * suites run every string of audit.HOSTILE through the fields they name; this run is about
 * reaching every field.)
 */
const EVIL = [0, 1, 2, 8, 3, 4].map((i) => audit.HOSTILE[i]);

function poison(value, evil) {
  if (typeof value === 'string') return ENUM.test(value) || /^claude-acct-/.test(value) ? value : value + evil;
  if (Array.isArray(value)) return value.map((v) => poison(v, evil));
  if (value && typeof value === 'object') {
    const out = {};
    for (const key of Object.keys(value)) out[key] = poison(value[key], evil);
    return out;
  }
  return value;
}

/** Where each page draws what the fork draws (upstream's own markup around it is not ours to audit). */
const ROOTS = { 'notch.html': ['#card', '#an-marks', '#pill'], 'settings.html': ['#pane-claude'], 'agentnotch/panel.html': ['#an-panel'] };
/** Elements the pages themselves are built of, besides the renderers' (audit.TAGS). */
const PAGE_TAGS = ['main', 'img'];

function problemsOf(page, where) {
  const found = [];
  for (const selector of ROOTS[page.file]) {
    const root = page.$(selector);
    if (!root) continue;
    for (const p of audit.problems(root, { allowTags: PAGE_TAGS })) found.push(`${where} ${selector}: ${p}`);
  }
  for (const e of page.errors) found.push(`${where}: page error ${String(e).slice(0, 160)}`);
  for (const v of page.hub.violations) found.push(`${where}: ${v}`);
  return found;
}

/** How many times `marker` shows as text in the page's fork regions: the sweep must draw what it poisons. */
function shown(page, marker) {
  let n = 0;
  for (const selector of ROOTS[page.file]) {
    const root = page.$(selector);
    if (root) n += root.textContent.split(marker).length - 1;
  }
  return n;
}

test('every fixture string, poisoned, through every scene and every session\'s chat: text only, no page error', async () => {
  const found = [];
  for (const evil of EVIL) {
    const snapshot = poison(harness.fixture('snapshot.json'), evil);
    const settings = poison(harness.fixture('settings.json'), evil);
    const chat = poison(harness.fixture('chat.json'), evil);
    for (const s of SCENES) {
      const page = harness.loadPage(s.page, {
        edge: s.edge || 'right', width: s.width || 400, height: Number(s.height) || 700, snapshot, settings, chat,
        before: s.panel ? (window) => { window.__AGENTNOTCH_PANEL__ = JSON.parse(JSON.stringify(s.panel)); } : undefined,
      });
      await page.settle();
      page.run(`window.${s.global}.showScene(${JSON.stringify(s.scene)})`);
      await page.settle();
      found.push(...problemsOf(page, `${s.name} <- ${evil.slice(0, 30)}`));
    }
    // Each session's chat, live (not a scene), with the keyboard confirmed so the composer is drawn editable.
    for (const row of snapshot.sessions) {
      const page = harness.loadPage('agentnotch/panel.html', { snapshot, settings, chat });
      await page.settle();
      page.emit('an:panel_focus', { focused: true });
      page.run(`agentnotchPanel.navigate(${JSON.stringify('session:' + row.session_id)})`);
      await page.settle();
      const update = poison(harness.fixture('chat.json'), evil);
      update.session_id = row.session_id;
      page.emit('an:chat', update);
      await page.settle();
      page.tick(400);
      await page.settle();
      found.push(...problemsOf(page, `chat ${row.session_id.slice(0, 24)} <- ${evil.slice(0, 30)}`));
    }
  }
  assert.deepEqual([...new Set(found)].slice(0, 20), []);
});

test('the sweep draws what it poisons: the marker shows as text on every page', async () => {
  const marker = '<b>SWEEP</b>';
  const snapshot = poison(harness.fixture('snapshot.json'), marker);
  const settings = poison(harness.fixture('settings.json'), marker);
  const cases = [['agentnotch/panel.html', 'agentnotchPanel', 'panel-every-state', 20], ['settings.html', 'agentnotchSettings', 'settings-full', 20], ['notch.html', 'agentnotch', 'notch-card', 5]];
  for (const [file, global, scene, least] of cases) {
    const page = harness.loadPage(file, { snapshot, settings });
    await page.settle();
    assert.equal(page.run(`window.${global}.showScene(${JSON.stringify(scene)})`), true);
    await page.settle();
    assert.ok(shown(page, marker) >= least, `${file}: ${shown(page, marker)} times`);
    for (const selector of ROOTS[file]) assert.equal(page.$(selector) ? page.$(selector).querySelectorAll('b').length : 0, 0, `${file} ${selector}`);
  }
  const page = harness.loadPage('agentnotch/panel.html', { snapshot, settings });
  await page.settle();
  const id = snapshot.sessions[0].session_id;
  page.run(`agentnotchPanel.navigate(${JSON.stringify('session:' + id)})`);
  await page.settle();
  const update = poison(harness.fixture('chat.json'), marker);
  update.session_id = id;
  page.emit('an:chat', update);
  await page.settle();
  assert.ok(page.$$('#an-chat .an-ci').length >= 5, 'the transcript is drawn');
  assert.ok(shown(page, marker) >= 10, `chat: ${shown(page, marker)} times`);
});
