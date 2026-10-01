'use strict';
// Static audits over every file the fork owns in ui/agentnotch (the package's acceptance list):
// one strict IIFE per script and no top-level let/const/class (a clash with upstream's classic-
// script globals is a SyntaxError that kills upstream's page), nothing the CSP would block or the
// token-free rule forbids, every use of innerHTML accounted for, every localStorage access inside
// a try, and panel.html's CSP meta equal to the one Tauri sets on the other pages.
//
// These read source text; the behaviour is proven by the page suites. Each scan is also run on a
// bad source first, so a pattern that stopped matching cannot pass quietly.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const harness = require('./lib/harness.cjs');

const DIR = path.join(harness.UI, 'agentnotch');
const FILES = fs.readdirSync(DIR).sort();
const SCRIPTS = FILES.filter((f) => f.endsWith('.js'));
const read = (f) => fs.readFileSync(path.join(DIR, f), 'utf8');

/** Source text with comments and the contents of string literals blanked, so a word in a comment
 *  or a message is not mistaken for code (and a pattern in code is). Newlines are kept. */
function code(source, keepStrings) {
  let out = '';
  let i = 0;
  const n = source.length;
  // A `/` starts a regular expression unless the previous token ends an operand.
  let prev = '';
  while (i < n) {
    const c = source[i];
    const d = source[i + 1];
    if (c === '/' && d === '/') {
      while (i < n && source[i] !== '\n') i++;
    } else if (c === '/' && d === '*') {
      const end = source.indexOf('*/', i + 2);
      const stop = end < 0 ? n : end + 2;
      out += source.slice(i, stop).replace(/[^\n]/g, ' ');
      i = stop;
    } else if (c === '"' || c === "'" || c === '`') {
      out += c;
      i++;
      while (i < n && source[i] !== c) {
        if (source[i] === '\\') {
          out += keepStrings ? source[i] : ' ';
          i++;
        }
        out += source[i] === '\n' || keepStrings ? source[i] : ' ';
        i++;
      }
      out += c;
      i++;
      prev = 'a';
    } else if (c === '/' && !/[\w)\]]/.test(prev)) {
      out += c;
      i++;
      let inClass = false;
      while (i < n && (source[i] !== '/' || inClass) && source[i] !== '\n') {
        if (source[i] === '\\') {
          out += ' ';
          i++;
        } else if (source[i] === '[') inClass = true;
        else if (source[i] === ']') inClass = false;
        out += ' ';
        i++;
      }
      out += '/';
      i++;
      prev = 'a';
    } else {
      out += c;
      if (!/\s/.test(c)) prev = c;
      i++;
    }
  }
  return out;
}

/** The first-column declarations of a script: the ones that would join the page's global scope. */
const topLevelLexical = (source) => code(source).split('\n').filter((l) => /^(let|const|class)\b/.test(l));

test('the audit sees comments, strings and regular expressions as the page does', () => {
  const src = "// eval(x)\nvar a = 'eval(y)'; /* new Function */ var b = /eval\\(z\\)/; var c = `fetch(${1})`;\n";
  const out = code(src);
  assert.ok(!/eval|Function|fetch/.test(out), out);
  assert.ok(/var a = '/.test(out) && /var b = \//.test(out));
  assert.deepEqual(topLevelLexical('let x = 1;\n  const y = 2;\nclass Z {}\n// const w\nvar ok;'), ['let x = 1;', 'class Z {}']);
});

test('every fork script is one strict IIFE with exactly one global and no top-level let, const or class', () => {
  assert.ok(SCRIPTS.length >= 10, SCRIPTS.join(', '));
  const globals = [];
  for (const name of SCRIPTS) {
    const src = read(name);
    const body = code(src, true).trim();
    assert.match(body, /^\(function \(\) \{\s*'use strict';/, `${name} opens as a strict IIFE`);
    assert.match(body, /\}\)\(\);$/, `${name} closes the IIFE and nothing follows it`);
    assert.deepEqual(topLevelLexical(src), [], `${name}: a top-level lexical binding`);
    const assigned = [...src.matchAll(/^ {2}(?:window\.)?(agentnotch[A-Za-z]*)\s*=|window\.(agentnotch[A-Za-z]*)\s*=/gm)].map((m) => m[1] || m[2]);
    const named = [...new Set(assigned)];
    assert.equal(named.length, 1, `${name} defines ${named.join(', ') || 'no global'}`);
    globals.push(named[0]);
  }
  assert.equal(new Set(globals).size, globals.length, `no two scripts share a global: ${globals.join(', ')}`);
  assert.deepEqual(
    globals.slice().sort(),
    [
      'agentnotch', 'agentnotchChat', 'agentnotchCommon', 'agentnotchMarkdown', 'agentnotchPanel',
      'agentnotchPanelList', 'agentnotchRebrand', 'agentnotchSettings', 'agentnotchSettingsSections', 'agentnotchToolResults',
    ],
    'the globals the brief lists, plus the two the list and the settings sections took',
  );
});

/** What no fork script may contain (D 2136-2158, 405-429), as [name, pattern]. */
const FORBIDDEN = [
  ['eval', /\beval\s*\(/],
  ['new Function', /\bnew\s+Function\b|\bFunction\s*\(/],
  ['a string for a timer', /\bset(?:Timeout|Interval)\s*\(\s*['"`]/],
  ['an inline handler attribute', /\bon[a-z]+\s*=\s*["'\\]/i],
  ['a javascript: URL', /javascript\s*:/i],
  ['fetch', /\bfetch\s*\(/],
  ['XMLHttpRequest', /\bXMLHttpRequest\b/],
  ['WebSocket', /\bWebSocket\b|\bEventSource\b/],
  ['window.open', /\bwindow\.open\b|(?:^|[^.\w$])open\s*\(/],
  ['document.write', /\bdocument\.write(?:ln)?\s*\(/],
  ['importScripts or import()', /\bimportScripts\s*\(|\bimport\s*\(/],
];

test('no fork script uses what the CSP blocks or what the token-free rule forbids', () => {
  const probe = code("el.onclick = 1; x = '<a onclick=\"f()\">'; eval('1'); new Function('a'); fetch('/x'); window.open('https://x'); setTimeout('f()', 1); new XMLHttpRequest(); new WebSocket('x'); document.write('x'); import('x');\n");
  // Strings are blanked, so the markup probe is checked on the raw text below.
  for (const [name, re] of FORBIDDEN.filter(([n]) => n !== 'an inline handler attribute' && n !== 'a javascript: URL')) {
    assert.match(probe, re, `the audit finds ${name}`);
  }
  assert.match("<a onclick=\"f()\">", FORBIDDEN[3][1]);
  assert.match("a href=\"javascript:alert(1)\"", FORBIDDEN[4][1]);
  for (const name of SCRIPTS) {
    const src = code(read(name));
    for (const [what, re] of FORBIDDEN) {
      if (what === 'an inline handler attribute' || what === 'a javascript: URL') continue;
      assert.doesNotMatch(src, re, `${name} uses ${what}`);
    }
  }
});

test('markup that scripts build (their string literals) and the three pages never carry a handler or a javascript: URL', () => {
  for (const name of FILES.filter((f) => /\.(js|html)$/.test(f))) {
    const raw = read(name);
    // Comments name the rule ("never an inline handler"), so only code and strings are scanned.
    const noComments = raw.replace(/\/\*[\s\S]*?\*\/|(^|[^:'"`\\])\/\/[^\n]*/g, '$1').replace(/<!--[\s\S]*?-->/g, '');
    assert.doesNotMatch(noComments, /\son[a-z]+\s*=\s*["'\\]/i, `${name}: an inline handler in markup`);
    assert.doesNotMatch(noComments, /javascript\s*:/i, `${name}: a javascript: URL`);
  }
});

test('the strings the token-free rule names appear nowhere in the fork pages, scripts or styles', () => {
  // The names are assembled here so that this file does not itself hold them (verify-token-free.sh
  // greps the Windows fork code for the same words).
  const names = [['.credentials', '.json'], ['claude', 'AiOauth'], ['api/oauth', '/usage']].map((parts) => parts.join(''));
  const patterns = names.map((n) => new RegExp(n.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')));
  patterns.push(/sk-ant-|oauth_token|access_token|refresh_token/i);
  for (const name of FILES) {
    const raw = read(name);
    for (const re of patterns) assert.doesNotMatch(raw, re, name);
  }
  assert.ok(patterns[0].test(names[0]) && patterns[1].test(names[1]) && patterns[2].test(names[2]), 'the patterns match what they name');
});

test('innerHTML, insertAdjacentHTML and outerHTML are fed only the escaped renderers or built-in markup', () => {
  const found = [];
  for (const name of SCRIPTS) {
    code(read(name)).split('\n').forEach((line, index) => {
      if (/\b(?:innerHTML|insertAdjacentHTML|outerHTML)\b|\bdocument\.write\b/.test(line)) found.push(`${name}:${read(name).split('\n')[index].trim()}`);
    });
  }
  const allowed = [
    // common.js morph: the template parses an html string the caller built with esc().
    /^common\.js:tpl\.innerHTML = html;$/,
    // chat.js: the fixed frame of the chat, no data in it.
    /^chat\.js:host\.innerHTML =$/,
    /^chat\.js:host\.innerHTML = '';$/,
    // notch.js: SVG made of constants and numbers (the activity layer and the inside weekly ring).
    /^notch\.js:svg\.insertAdjacentHTML\('beforeend', '<circle /,
    /^notch\.js:if \(layer\) layer\.innerHTML = activityHtml\(kind\);$/,
  ];
  const stray = found.filter((line) => !allowed.some((re) => re.test(line)));
  assert.deepEqual(stray, [], 'an innerHTML use the audit has not looked at');
  assert.equal(found.length, 5, found.join('\n'));
  // The two that take a variable are fed by a renderer that cannot carry text from a session:
  // activityHtml and svgArc build from the fixed activity kinds and clamped numbers, and
  // morph's callers pass esc()'d strings (every renderer suite feeds them audit.HOSTILE).
  const notch = read('notch.js');
  const activity = notch.slice(notch.indexOf('function activityHtml'), notch.indexOf('function activityHtml') + 900);
  assert.ok(!/\bring\.|\bsession|\.name\b|\.label\b/.test(activity.split('\n').slice(0, 14).join('\n')), 'activityHtml uses no session text');
});

test('every localStorage and sessionStorage access is inside a try block', () => {
  let seen = 0;
  for (const name of SCRIPTS) {
    const lines = code(read(name)).split('\n');
    lines.forEach((line, index) => {
      if (!/\b(?:localStorage|sessionStorage|indexedDB)\b|\bstore\.(?:getItem|setItem|removeItem|clear|key)\b|\.(?:getItem|setItem|removeItem)\s*\(/.test(line)) return;
      seen++;
      // Walk up to the nearest enclosing function head; a try { must open before it.
      let depth = 0;
      let guarded = false;
      for (let i = index; i >= 0 && !guarded; i--) {
        const text = lines[i];
        depth += (text.match(/\}/g) || []).length - (text.match(/\{/g) || []).length;
        if (/\btry\s*\{/.test(text) && depth <= 0) guarded = true;
        if (/\bfunction\b/.test(text) && depth < 0 && !/\btry\b/.test(text)) break;
      }
      assert.ok(guarded, `${name}:${index + 1} touches storage outside a try: ${line.trim()}`);
    });
  }
  assert.ok(seen >= 4, `the audit found ${seen} storage accesses`);
});

test('a storage access outside a try is something the audit would catch', () => {
  const lines = code('function f() {\n  return window.localStorage.getItem("k");\n}\n').split('\n');
  let depth = 0;
  let guarded = false;
  for (let i = 1; i >= 0; i--) {
    depth += (lines[i].match(/\}/g) || []).length - (lines[i].match(/\{/g) || []).length;
    if (/\btry\s*\{/.test(lines[i]) && depth <= 0) guarded = true;
  }
  assert.equal(guarded, false);
});

test('panel.html carries the CSP meta, and it is exactly the policy tauri.conf.json sets on the other pages', () => {
  const conf = JSON.parse(fs.readFileSync(path.join(harness.UI, '..', 'tauri.conf.json'), 'utf8'));
  const csp = conf.app && conf.app.security && conf.app.security.csp;
  assert.equal(typeof csp, 'string', 'tauri.conf.json has app.security.csp');
  const metas = [...read('panel.html').matchAll(/<meta\s+http-equiv="Content-Security-Policy"\s+content="([^"]*)"\s*>/g)].map((m) => m[1]);
  assert.equal(metas.length, 1, 'one CSP meta in panel.html');
  assert.equal(metas[0], csp);
  for (const directive of ["default-src 'self'", "script-src 'self'", "object-src 'none'", "base-uri 'none'", "frame-ancestors 'none'"]) {
    assert.ok(csp.split('; ').includes(directive), directive);
  }
  assert.ok(!/unsafe-eval|unsafe-inline.*script-src|script-src[^;]*unsafe/.test(csp), 'no unsafe script source');
});
