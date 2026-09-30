'use strict';
// ui/agentnotch/rebrand.js on its own (the port of Fork.rebranded): the rules, the shared
// vectors (agentnotch-engine/tests/ui-contract/rebrand-vectors.json, also read by the engine's
// core::rebrand), the DOM walks, and the static half of "rebrand vectors over settings.html":
// every "Codenotch" the page holds is rebranded or is copy about a product that really is
// called Codenotch. The Mac tests it ports: RebrandTests' English and compound cases (the
// French elision and the per-locale template checks belong to the Mac's catalog).

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const dom = require('./lib/dom.cjs');
const harness = require('./lib/harness.cjs');
const { load } = require('./lib/scripts.cjs');

const REPO = path.resolve(harness.UI, '..', '..', '..');
const SETTINGS_HTML = fs.readFileSync(path.join(harness.UI, 'settings.html'), 'utf8');

const { window, document } = load(['rebrand'], { html: '<div id="app"></div>' });
const R = window.agentnotchRebrand;
const rebrand = R.rebrand;
const tick = () => new Promise((resolve) => setImmediate(resolve));

// ---- the kept product phrases (Fork.upstreamProductPhrases) -----------------------------------------

/** The Mac's list, read from its source so the two cannot drift apart. */
function macPhrases() {
  const source = fs.readFileSync(path.join(REPO, 'Sources', 'App', 'Fork.swift'), 'utf8');
  const block = /static let upstreamProductPhrases = \[([\s\S]*?)\]/.exec(source);
  assert.ok(block, 'Fork.swift still holds upstreamProductPhrases');
  return [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

const KEPT = [
  'Codenotch app on your phone', 'Codenotch on your phone', 'Codenotch phone app',
  'Codenotch for Windows', 'Codenotch-Setup', 'hivinz.com',
];

test('the kept product phrases are the Mac\'s (Fork.upstreamProductPhrases)', () => {
  assert.deepEqual(KEPT, macPhrases());
  assert.deepEqual(Array.from(R.PRODUCT_PHRASES), KEPT);
});

// ---- shape ------------------------------------------------------------------------------------

test('rebrand.js defines exactly one global and no top-level lexical binding', () => {
  const file = path.join(harness.UI, 'agentnotch', 'rebrand.js');
  const source = fs.readFileSync(file, 'utf8');
  assert.doesNotMatch(source, /^(let|const|class)\s/m);
  assert.match(source, /^\(function \(\) \{\n {2}'use strict';/m);
  const context = vm.createContext({});
  const win = vm.runInContext('this', context);
  Object.assign(win, { window: win, document: dom.parseDocument('<html><head></head><body></body></html>') });
  const before = new Set(Object.getOwnPropertyNames(win));
  vm.runInContext(source, context);
  assert.deepEqual(Object.getOwnPropertyNames(win).filter((n) => !before.has(n)), ['agentnotchRebrand']);
  assert.equal(vm.runInContext("typeof UPSTREAM === 'undefined' && typeof NAME === 'undefined' && typeof SKIP === 'undefined'", context), true);
  assert.deepEqual(Object.keys(win.agentnotchRebrand).sort(), ['PRODUCT_PHRASES', 'observe', 'rebrand', 'rebrandTree']);
});

// ---- the shared vectors ---------------------------------------------------------------------------

test('every vector of rebrand-vectors.json', () => {
  const vectors = harness.fixture('rebrand-vectors.json');
  assert.ok(vectors.length >= 10);
  for (const { input, expected } of vectors) assert.equal(rebrand(input), expected, input);
});

// ---- RebrandTests (ForkSPM), English and compounds ------------------------------------------------------

test('lookups name this app', () => {
  assert.equal(rebrand('Quit Codenotch'), 'Quit Agent Notch');
  assert.equal(rebrand('Signs out of MiniMax — the session belongs to Codenotch.'), 'Signs out of MiniMax — the session belongs to Agent Notch.');
  assert.equal(rebrand('Refresh all'), 'Refresh all');
  assert.equal(rebrand('Codenotch'), 'Agent Notch');
  assert.equal(rebrand('Codenotch, Codenotch and Codenotch'), 'Agent Notch, Agent Notch and Agent Notch');
});

test('English takes "an" before the name', () => {
  const key = 'Most readings are borrowed from a tool that already holds the account. DeepSeek and MiniMax are the exceptions: clicking Sign in opens a Codenotch window for that account, and signing out here clears only that session and its saved reading.';
  const out = rebrand(key);
  assert.ok(out.includes('opens an Agent Notch window'), out);
  assert.ok(out.includes('borrowed from a tool'), 'other "a" words stay');
  assert.equal(rebrand('A Codenotch ring'), 'An Agent Notch ring');
  assert.equal(rebrand('a Codenotch ring'), 'an Agent Notch ring');
  // Only the article: a word that ends in "a" is not one.
  assert.equal(rebrand('Data Codenotch'), 'Data Agent Notch');
  assert.equal(rebrand('Extra Codenotch'), 'Extra Agent Notch');
  assert.equal(rebrand('a Codenotchx'), 'a Agent Notchx', 'no word boundary after the name: only the plain rule');
});

test('the phone app and upstream\'s Windows build keep their name', () => {
  assert.equal(rebrand('Scan this code with the Codenotch app on your phone.'), 'Scan this code with the Codenotch app on your phone.');
  assert.equal(rebrand('1. Open Codenotch on your phone'), '1. Open Codenotch on your phone');
  assert.equal(rebrand('Codenotch for Windows, installable'), 'Codenotch for Windows, installable');
  assert.equal(rebrand('Download Codenotch-Setup.exe'), 'Download Codenotch-Setup.exe');
  assert.equal(rebrand('Made by hivinz.com'), 'Made by hivinz.com');
  for (const phrase of KEPT) {
    assert.equal(rebrand('See ' + phrase + ' now'), 'See ' + phrase + ' now', phrase);
  }
  // Copy that names such a product is left whole, even where it also names the app itself.
  assert.equal(rebrand('Quit Codenotch. Get Codenotch for Windows.'), 'Quit Codenotch. Get Codenotch for Windows.');
});

test('compounds join every word of the name', () => {
  assert.equal(rebrand('Codenotch-Einstellungen'), 'Agent-Notch-Einstellungen');
  assert.equal(rebrand('Codenotch-Einstellungen öffnen'), 'Agent-Notch-Einstellungen öffnen');
  assert.equal(rebrand('Codenotch-x Codenotch'), 'Agent-Notch-x Agent Notch');
  assert.equal(rebrand("Codenotch'tan Çık"), "Agent Notch'tan Çık");
  assert.equal(rebrand('Configurações de Codenotch'), 'Configurações de Agent Notch');
  assert.equal(rebrand('Encerrar o Codenotch'), 'Encerrar o Agent Notch');
  assert.equal(rebrand('退出 Codenotch'), '退出 Agent Notch');
});

test('values that are not text pass through; a text without the name is returned as it is', () => {
  assert.equal(rebrand(null), null);
  assert.equal(rebrand(undefined), undefined);
  assert.equal(rebrand(''), '');
  assert.equal(rebrand(42), '42');
  assert.equal(rebrand('codenotch'), 'codenotch', 'the name is matched exactly');
  assert.equal(rebrand('CODENOTCH'), 'CODENOTCH');
  assert.equal(rebrand('Agent Notch'), 'Agent Notch');
});

test('rebranding twice changes nothing more', () => {
  for (const s of ['Quit Codenotch', 'a Codenotch window', 'Codenotch-Einstellungen', 'Codenotch for Windows', 'A Codenotch']) {
    assert.equal(rebrand(rebrand(s)), rebrand(s), s);
  }
  for (const s of harness.fixture('rebrand-vectors.json').map((v) => v.input)) assert.doesNotMatch(rebrand(s).replace(/Codenotch(-Setup| for Windows| (app )?on your phone| phone app)/g, ''), /Codenotch/, s);
});

test('a name inside a longer word is data the caller marks with data-user, not something rebrand() can tell', () => {
  // The Mac keeps a folder or device named "Codenotch" by checking the catalog template; here
  // the page marks such nodes [data-user] and the walkers skip them (below).
  assert.equal(rebrand('Working in Codenotch-main'), 'Working in Agent-Notch-main');
});

// ---- the DOM walks ---------------------------------------------------------------------------------

function fresh(html) {
  const ctx = load(['rebrand'], { html });
  return { R: ctx.window.agentnotchRebrand, document: ctx.document };
}

test('rebrandTree: text nodes and the aria-label, title and placeholder attributes under a root', () => {
  const { R: r, document: d } = fresh(
    '<div id="app"><h1>Codenotch Settings</h1>' +
    '<button aria-label="Open Codenotch at login" title="Quit Codenotch" data-x="Codenotch">Quit <b>Codenotch</b></button>' +
    '<input placeholder="Ask Codenotch"><p>nothing here</p><img alt="Codenotch"></div>');
  r.rebrandTree(d.getElementById('app'));
  assert.equal(d.querySelector('h1').textContent, 'Agent Notch Settings');
  const button = d.querySelector('button');
  assert.equal(button.getAttribute('aria-label'), 'Open Agent Notch at login');
  assert.equal(button.getAttribute('title'), 'Quit Agent Notch');
  assert.equal(button.getAttribute('data-x'), 'Codenotch', 'other attributes are not copy');
  assert.equal(button.textContent, 'Quit Agent Notch');
  assert.equal(d.querySelector('input').getAttribute('placeholder'), 'Ask Agent Notch');
  assert.equal(d.querySelector('p').textContent, 'nothing here');
  assert.equal(d.querySelector('img').getAttribute('alt'), 'Codenotch');
});

test('rebrandTree: a single text node, a document, a missing or odd root', () => {
  const { R: r, document: d } = fresh('<div id="app"><span id="s">Quit Codenotch</span></div>');
  r.rebrandTree(d.getElementById('s').firstChild);
  assert.equal(d.getElementById('s').textContent, 'Quit Agent Notch');
  d.getElementById('s').firstChild.nodeValue = 'Codenotch again';
  r.rebrandTree(d);
  assert.equal(d.getElementById('s').textContent, 'Agent Notch again');
  assert.doesNotThrow(() => r.rebrandTree(null));
  assert.doesNotThrow(() => r.rebrandTree(undefined));
  assert.doesNotThrow(() => r.rebrandTree({ nodeType: 8 }));
  const title = d.querySelector('title');
  assert.equal(title, null);
});

test('rebrandTree: user data ([data-user]), the fork\'s own pane and script or style text are never touched', () => {
  const { R: r, document: d } = fresh(
    '<div id="app"><div id="body">' +
    '<span id="u" data-user>Codenotch</span>' +
    '<div data-user><b id="nested">Codenotch-main</b> <i title="Codenotch" id="tt">x</i></div>' +
    '<section id="pane-claude"><p id="mine">Remove Codenotch\'s hooks</p><button aria-label="Codenotch" id="pb"></button></section>' +
    '<p id="plain">Codenotch</p><style id="st">.Codenotch{}</style></div></div>');
  r.rebrandTree(d.getElementById('app'));
  assert.equal(d.getElementById('u').textContent, 'Codenotch');
  assert.equal(d.getElementById('nested').textContent, 'Codenotch-main');
  assert.equal(d.getElementById('tt').getAttribute('title'), 'Codenotch');
  assert.equal(d.getElementById('mine').textContent, "Remove Codenotch's hooks");
  assert.equal(d.getElementById('pb').getAttribute('aria-label'), 'Codenotch');
  assert.equal(d.getElementById('plain').textContent, 'Agent Notch');
  assert.equal(d.getElementById('st').textContent, '.Codenotch{}');
  // A root that is itself user data is skipped too.
  r.rebrandTree(d.getElementById('u'));
  assert.equal(d.getElementById('u').textContent, 'Codenotch');
});

test('rebrandTree keeps a kept phrase and leaves an unchanged node alone', () => {
  const { R: r, document: d } = fresh('<div id="app"><p id="a">Scan with the Codenotch app on your phone.</p><p id="b">Quit</p></div>');
  const b = d.getElementById('b').firstChild;
  r.rebrandTree(d.getElementById('app'));
  assert.equal(d.getElementById('a').textContent, 'Scan with the Codenotch app on your phone.');
  assert.equal(d.getElementById('b').firstChild, b);
});

test('observe: nodes added later, changed text and changed labels are rebranded as they land', async () => {
  const { R: r, document: d } = fresh('<div id="app"><div id="body"><p id="p">Static</p></div></div>');
  const body = d.getElementById('body');
  const observer = r.observe(body);
  assert.ok(observer && typeof observer.disconnect === 'function');
  const added = d.createElement('div');
  added.innerHTML = '<span id="new">Quit Codenotch</span><button id="btn" aria-label="Open Codenotch">x</button>';
  body.appendChild(added);
  await tick();
  assert.equal(d.getElementById('new').textContent, 'Quit Agent Notch');
  assert.equal(d.getElementById('btn').getAttribute('aria-label'), 'Open Agent Notch');
  // A text node's value changed in place (upstream redraws a row's text this way).
  d.getElementById('p').firstChild.nodeValue = 'Codenotch is ready';
  await tick();
  assert.equal(d.getElementById('p').textContent, 'Agent Notch is ready');
  // A label changed later.
  d.getElementById('btn').setAttribute('title', 'A Codenotch button');
  await tick();
  assert.equal(d.getElementById('btn').getAttribute('title'), 'An Agent Notch button');
  // Text set through textContent.
  d.getElementById('p').textContent = 'Quit Codenotch';
  await tick();
  assert.equal(d.getElementById('p').textContent, 'Quit Agent Notch');
  observer.disconnect();
});

test('observe skips [data-user] and the fork\'s own pane, and ends its own chain', async () => {
  const { R: r, document: d } = fresh(
    '<div id="app"><div id="body"><section id="pane-claude"></section></div></div>');
  const body = d.getElementById('body');
  r.observe(body);
  const user = d.createElement('span');
  user.setAttribute('data-user', '');
  user.textContent = 'Codenotch';
  body.appendChild(user);
  const inner = d.createElement('div');
  inner.innerHTML = '<b data-user id="deep">Codenotch-main</b> <i id="ok">Codenotch</i>';
  body.appendChild(inner);
  d.getElementById('pane-claude').innerHTML = '<p id="mine">Remove Codenotch\'s hooks</p>';
  await tick();
  assert.equal(user.textContent, 'Codenotch');
  assert.equal(d.getElementById('deep').textContent, 'Codenotch-main');
  assert.equal(d.getElementById('ok').textContent, 'Agent Notch');
  assert.equal(d.getElementById('mine').textContent, "Remove Codenotch's hooks");
  // The observer's own edits settle: nothing left to do, nothing pending.
  await tick();
  assert.equal(d.getElementById('ok').textContent, 'Agent Notch');
});

test('observe without a root or without MutationObserver does nothing', () => {
  assert.equal(R.observe(null), null);
  assert.equal(R.observe(undefined), null);
  const context = vm.createContext({});
  const win = vm.runInContext('this', context);
  Object.assign(win, { window: win, document: dom.parseDocument('<html><head></head><body><p id="p"></p></body></html>') });
  vm.runInContext(fs.readFileSync(path.join(harness.UI, 'agentnotch', 'rebrand.js'), 'utf8'), context);
  assert.equal(win.agentnotchRebrand.observe(win.document.getElementById('p')), null);
});

// ---- rebrand vectors over settings.html (the static half) ---------------------------------------------

/** A string literal's value from its source text (quotes included); template parts are raw text. */
function literalValue(source) {
  const quote = source.charAt(0);
  if (quote === '`') return source.slice(1, -1).split(/\$\{[^}]*\}/);
  return [vm.runInNewContext(source)];
}

/**
 * The string literals and comments of a script, with the line each starts on. A small scanner:
 * enough for the page's own code, and it fails loudly (a raw line end inside a quote, or a
 * literal left open) instead of guessing when it loses its place.
 */
function scan(code, firstLine) {
  const literals = [];
  const comments = [];
  let line = firstLine;
  let lastSignificant = '';
  let i = 0;
  const bump = (text) => { for (const c of text) if (c === '\n') line += 1; };
  while (i < code.length) {
    const c = code[i];
    const two = code.slice(i, i + 2);
    if (two === '//') {
      const end = code.indexOf('\n', i);
      const stop = end < 0 ? code.length : end;
      comments.push({ line, text: code.slice(i, stop) });
      i = stop;
    } else if (two === '/*') {
      const end = code.indexOf('*/', i + 2);
      assert.ok(end > 0, 'a block comment is left open');
      comments.push({ line, text: code.slice(i, end + 2) });
      bump(code.slice(i, end + 2));
      i = end + 2;
    } else if (c === '"' || c === "'" || c === '`') {
      let j = i + 1;
      while (j < code.length && code[j] !== c) {
        if (code[j] === '\\') j += 1;
        else if (c !== '`' && code[j] === '\n') assert.fail(`the scanner lost its place: a raw line end inside a literal at line ${line}`);
        j += 1;
      }
      assert.ok(j < code.length, `a literal is left open at line ${line}`);
      const source = code.slice(i, j + 1);
      literals.push({ line, source, values: literalValue(source) });
      bump(source);
      i = j + 1;
      lastSignificant = c;
    } else if (c === '/' && /[(,=:[!&|?{};+\-*%<>~^]|^$/.test(lastSignificant)) {
      // A regular expression literal: skip it, classes included.
      let j = i + 1;
      let inClass = false;
      while (j < code.length && (code[j] !== '/' || inClass)) {
        if (code[j] === '\\') j += 1;
        else if (code[j] === '[') inClass = true;
        else if (code[j] === ']') inClass = false;
        else if (code[j] === '\n') assert.fail(`the scanner lost its place in a regular expression at line ${line}`);
        j += 1;
      }
      i = j + 1;
      lastSignificant = ')';
    } else {
      if (c === '\n') line += 1;
      if (!/\s/.test(c)) lastSignificant = c;
      i += 1;
    }
  }
  return { literals, comments };
}

/** Whether `text` is fine after rebrand(): no Codenotch left, or copy about a kept product. */
function settled(text) {
  const out = rebrand(text);
  if (!out.includes('Codenotch')) return { ok: true, kept: false, out };
  return { ok: KEPT.some((phrase) => out.includes(phrase)) && out === text, kept: true, out };
}

test('settings.html: every text node and label attribute holding "Codenotch" is rebranded or a kept phrase', () => {
  const doc = dom.parseDocument(SETTINGS_HTML);
  const found = [];
  const visit = (node) => {
    if (node.nodeType === 3) {
      if (node.nodeValue.includes('Codenotch') && !/^(script|style)$/.test(node.parentElement ? node.parentElement.localName : '')) found.push(['text', node.nodeValue]);
    } else if (node.nodeType === 1) {
      for (const { name, value } of node.attributes) if (value.includes('Codenotch')) found.push(['@' + name, value]);
    }
    for (const child of node.childNodes) visit(child);
  };
  visit(doc);
  // The page names the app in its title, its buttons and their labels.
  assert.ok(found.length >= 4, JSON.stringify(found));
  for (const [where, text] of found) {
    const r = settled(text);
    assert.ok(r.ok, `${where}: ${text} -> ${r.out}`);
    if (where !== 'text') assert.match(where, /^@(aria-label|title|placeholder)$/, `an attribute copy walkers do not reach: ${where}=${text}`);
  }
  // And the walk itself gets them all.
  const ctx = load(['rebrand'], {});
  const copy = dom.parseDocument(SETTINGS_HTML);
  ctx.window.agentnotchRebrand.rebrandTree.call(null, copy);
  const left = [];
  const again = (node) => {
    if (node.nodeType === 3 && node.nodeValue.includes('Codenotch') && !/^(script|style)$/.test(node.parentElement ? node.parentElement.localName : '')) left.push(node.nodeValue);
    if (node.nodeType === 1) for (const { name, value } of node.attributes) if (value.includes('Codenotch')) left.push(name + '=' + value);
    for (const child of node.childNodes) again(child);
  };
  again(copy);
  assert.deepEqual(left.filter((s) => !KEPT.some((p) => s.includes(p))), []);
});

test('settings.html: every string literal of its scripts holding "Codenotch" is rebranded or a kept phrase', () => {
  const scripts = [...SETTINGS_HTML.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)];
  assert.ok(scripts.length >= 1);
  let strings = 0;
  const problems = [];
  const stats = { changed: 0, kept: 0, languages: new Set() };
  for (const block of scripts) {
    const firstLine = SETTINGS_HTML.slice(0, block.index + block[0].indexOf('>') + 1).split('\n').length;
    const { literals, comments } = scan(block[1], firstLine);
    // Nothing is lost: each line of the script that names Codenotch is in a literal or a comment.
    const covered = new Set();
    for (const lit of literals) {
      const lines = lit.source.split('\n').length;
      for (let n = 0; n < lines; n++) covered.add(lit.line + n);
    }
    for (const com of comments) for (let n = 0; n < com.text.split('\n').length; n++) covered.add(com.line + n);
    block[1].split('\n').forEach((text, n) => {
      if (text.includes('Codenotch') && !covered.has(firstLine + n)) problems.push(`line ${firstLine + n} names Codenotch outside any literal or comment: ${text.slice(0, 80)}`);
    });
    for (const lit of literals) {
      for (const value of lit.values) {
        if (!value.includes('Codenotch')) continue;
        strings += 1;
        const r = settled(value);
        if (!r.ok) problems.push(`line ${lit.line}: ${value.slice(0, 80)} -> ${r.out.slice(0, 80)}`);
        if (r.kept) stats.kept += 1;
        else stats.changed += 1;
      }
    }
  }
  assert.deepEqual(problems, []);
  // The page's English copy plus its translations: dozens of literals.
  assert.ok(strings >= 40, `only ${strings} literals name Codenotch`);
  assert.equal(stats.kept + stats.changed, strings);
});

test('settings.html: the page names no upstream product besides what the kept phrases cover', () => {
  // A phrase in the page is copy about upstream's own products: nothing in the settings page is.
  const hits = KEPT.filter((phrase) => SETTINGS_HTML.includes(phrase));
  assert.deepEqual(hits, []);
});

test('the scanner itself: strings, comments, regular expressions, templates and escapes', () => {
  const { literals, comments } = scan([
    "var a = 'it\\'s Codenotch'; // Codenotch in a comment",
    'var b = "say \\"hi\\" to Codenotch";',
    'var r = /[\'"]Codenotch/.test(a);',
    'var t = `x ${a} Codenotch y`; /* Codenotch\n block */',
    "var d = a / 2 / 3, e = 'z';",
  ].join('\n'), 1);
  assert.deepEqual(literals.map((l) => l.values.join('|')), ["it's Codenotch", 'say "hi" to Codenotch', 'x | Codenotch y', 'z']);
  assert.deepEqual(literals.map((l) => l.line), [1, 2, 4, 6], 'a block comment over two lines counts both');
  assert.equal(comments.length, 2);
  assert.throws(() => scan("var a = 'open\nnext';", 1));
});
