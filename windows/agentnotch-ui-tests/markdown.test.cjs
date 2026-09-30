'use strict';
// ui/agentnotch/markdown.js on its own (the port of MarkdownRenderer.swift): the markup each
// construct becomes, the link rule (text, and an address only for https:), no HTML passthrough,
// hostile text, and bounded work on pathological input. Transcript text is untrusted.

const test = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./lib/dom.cjs');
const audit = require('./lib/audit.cjs');
const { load } = require('./lib/scripts.cjs');

const { window } = load(['common', 'markdown']);
const M = window.agentnotchMarkdown;

const P = (inner) => '<div class="an-md"><p class="an-md-p">' + inner + '</p></div>';
const BODY = (blocks) => '<div class="an-md">' + blocks + '</div>';
const LINK = (url, label) =>
  `<span class="an-md-link" role="link" tabindex="0" data-an-url="${url}" title="${url}">${label}</span>`;
const fragment = (html) => dom.parseFragment(html);
const text = (html) => fragment(html).textContent;

// ---- shape ------------------------------------------------------------------------------------

test('markdown.js defines exactly one global and no top-level lexical binding', () => {
  const fs = require('node:fs');
  const path = require('node:path');
  const vm = require('node:vm');
  const file = path.join(require('./lib/harness.cjs').UI, 'agentnotch', 'markdown.js');
  const source = fs.readFileSync(file, 'utf8');
  assert.doesNotMatch(source, /^(let|const|class)\s/m);
  assert.match(source, /^\(function \(\) \{\n {2}'use strict';/m);
  const context = vm.createContext({});
  const win = vm.runInContext('this', context);
  Object.assign(win, { window: win, document: dom.parseDocument('<html><head></head><body></body></html>') });
  vm.runInContext(fs.readFileSync(path.join(path.dirname(file), 'common.js'), 'utf8'), context);
  const before = new Set(Object.getOwnPropertyNames(win));
  vm.runInContext(source, context);
  assert.deepEqual(Object.getOwnPropertyNames(win).filter((n) => !before.has(n)), ['agentnotchMarkdown']);
  assert.equal(vm.runInContext("typeof C === 'undefined' && typeof esc === 'undefined' && typeof MAX_DEPTH === 'undefined'", context), true);
});

// ---- blocks -----------------------------------------------------------------------------------

test('an empty or missing source is an empty body', () => {
  assert.equal(M.render(''), BODY(''));
  assert.equal(M.render(null), BODY(''));
  assert.equal(M.render(undefined), BODY(''));
  assert.equal(M.render('  \n\n  '), BODY(''));
});

test('headings carry their level as a role and a class; the levels are not different elements', () => {
  for (let level = 1; level <= 6; level++) {
    assert.equal(M.render('#'.repeat(level) + ' Title'),
      BODY(`<div class="an-md-h an-md-h${level}" role="heading" aria-level="${level}">Title</div>`));
  }
  assert.equal(M.render('####### seven'), P('####### seven'), 'seven hashes are text');
  assert.equal(M.render('#nospace'), P('#nospace'));
  assert.equal(M.render('## Closed ##'), BODY('<div class="an-md-h an-md-h2" role="heading" aria-level="2">Closed</div>'));
  assert.equal(M.render('# **bold** head'),
    BODY('<div class="an-md-h an-md-h1" role="heading" aria-level="1"><strong>bold</strong> head</div>'));
});

test('paragraphs: a bare line end is a space, two trailing spaces or a backslash a break, a blank line a new paragraph', () => {
  assert.equal(M.render('one\ntwo'), P('one two'));
  assert.equal(M.render('one  \ntwo'), P('one<br>two'));
  assert.equal(M.render('one\\\ntwo'), P('one<br>two'));
  assert.equal(M.render('one\n\ntwo'), BODY('<p class="an-md-p">one</p><p class="an-md-p">two</p>'));
  assert.equal(M.render('a\r\nb\rc'), P('a b c'), 'line ends of any platform');
});

test('emphasis, strong, strikethrough and inline code', () => {
  assert.equal(M.render('*em* and _em_'), P('<em>em</em> and <em>em</em>'));
  assert.equal(M.render('**strong** and __strong__'), P('<strong>strong</strong> and <strong>strong</strong>'));
  assert.equal(M.render('~~gone~~'), P('<s>gone</s>'));
  assert.equal(M.render('**a *b* c**'), P('<strong>a <em>b</em> c</strong>'));
  assert.equal(M.render('use `npm test` now'), P('use <code class="an-md-code">npm test</code> now'));
  assert.equal(M.render('`` a`b ``'), P('<code class="an-md-code">a&#96;b</code>'));
  assert.equal(M.render('`<b>x</b>`'), P('<code class="an-md-code">&lt;b&gt;x&lt;/b&gt;</code>'));
  assert.equal(M.render('`unclosed'), P('&#96;unclosed'));
});

test('emphasis needs a word edge: snake_case, a * b * c and a single ~ stay text', () => {
  assert.equal(M.render('snake_case_name and my_var_2'), P('snake_case_name and my_var_2'));
  assert.equal(M.render('2 * 3 * 4'), P('2 * 3 * 4'));
  assert.equal(M.render('a ~ b'), P('a ~ b'));
  assert.equal(M.render('*unclosed'), P('*unclosed'));
  assert.equal(M.render('\\*not em\\*'), P('*not em*'));
});

test('a backslash escapes markdown punctuation and nothing else', () => {
  assert.equal(M.render('\\# not a heading'), P('# not a heading'));
  assert.equal(M.render('C:\\Users\\me'), P('C:\\Users\\me'));
  assert.equal(M.render('\\`tick\\`'), P('&#96;tick&#96;'));
});

test('fenced code keeps its text exactly, in one pre, with no inline parsing', () => {
  assert.equal(M.render('```js\nlet a = "<x>" && *b*;\n```'),
    BODY('<pre class="an-md-pre"><code>let a = &quot;&lt;x&gt;&quot; &amp;&amp; *b*;</code></pre>'));
  assert.equal(M.render('~~~\nplain\n~~~'), BODY('<pre class="an-md-pre"><code>plain</code></pre>'));
  assert.equal(M.render('```\n  indented\n\nblank above\n```'),
    BODY('<pre class="an-md-pre"><code>  indented\n\nblank above</code></pre>'));
  assert.equal(M.render('```\nunclosed\nto the end'), BODY('<pre class="an-md-pre"><code>unclosed\nto the end</code></pre>'));
  assert.equal(M.render('````\n```\ninner\n```\n````'), BODY('<pre class="an-md-pre"><code>&#96;&#96;&#96;\ninner\n&#96;&#96;&#96;</code></pre>'));
  assert.equal(M.render('```\n```'), BODY('<pre class="an-md-pre"><code></code></pre>'));
});

test('lists: bullets and numbers with the markers drawn as text, nesting, and a numbered list keeps its start', () => {
  const item = (marker, inner) =>
    `<li><span class="an-md-marker" aria-hidden="true">${marker}</span><div class="an-md-item">${inner}</div></li>`;
  const p = (t) => `<p class="an-md-p">${t}</p>`;
  assert.equal(M.render('- a\n- b'), BODY('<ul class="an-md-list">' + item('•', p('a')) + item('•', p('b')) + '</ul>'));
  assert.equal(M.render('* a\n+ b'), BODY('<ul class="an-md-list">' + item('•', p('a')) + item('•', p('b')) + '</ul>'));
  assert.equal(M.render('1. a\n2. b'), BODY('<ol class="an-md-list">' + item('1.', p('a')) + item('2.', p('b')) + '</ol>'));
  assert.equal(M.render('3) c\n4) d'), BODY('<ol class="an-md-list">' + item('3.', p('c')) + item('4.', p('d')) + '</ol>'));
  assert.equal(M.render('- a\n  - b\n- c'),
    BODY('<ul class="an-md-list">' + item('•', p('a') + '<ul class="an-md-list">' + item('•', p('b')) + '</ul>') + item('•', p('c')) + '</ul>'));
  // A blank line between items keeps one list going.
  assert.equal(M.render('- a\n\n- b'), BODY('<ul class="an-md-list">' + item('•', p('a')) + item('•', p('b')) + '</ul>'));
  // A continuation line joins its item; a fenced block indented into an item stays in it.
  assert.equal(M.render('- a\n  more'), BODY('<ul class="an-md-list">' + item('•', p('a more')) + '</ul>'));
  assert.equal(M.render('- a\n  ```\n  x\n  ```'),
    BODY('<ul class="an-md-list">' + item('•', p('a') + '<pre class="an-md-pre"><code>x</code></pre>') + '</ul>'));
  assert.equal(M.render('-not a list'), P('-not a list'));
});

test('a list following a paragraph without a blank line starts a list', () => {
  assert.match(M.render('Steps:\n- one\n- two'), /^<div class="an-md"><p class="an-md-p">Steps:<\/p><ul class="an-md-list">/);
});

test('quotes hold blocks, nest, and a lazy continuation stays inside', () => {
  const p = (t) => `<p class="an-md-p">${t}</p>`;
  assert.equal(M.render('> quoted\n> more'), BODY('<blockquote class="an-md-quote">' + p('quoted more') + '</blockquote>'));
  assert.equal(M.render('> a\nlazy'), BODY('<blockquote class="an-md-quote">' + p('a lazy') + '</blockquote>'));
  assert.equal(M.render('> > deep'),
    BODY('<blockquote class="an-md-quote"><blockquote class="an-md-quote">' + p('deep') + '</blockquote></blockquote>'));
  assert.equal(M.render('> - a\n> - b'),
    BODY('<blockquote class="an-md-quote"><ul class="an-md-list">' +
      '<li><span class="an-md-marker" aria-hidden="true">•</span><div class="an-md-item">' + p('a') + '</div></li>' +
      '<li><span class="an-md-marker" aria-hidden="true">•</span><div class="an-md-item">' + p('b') + '</div></li></ul></blockquote>'));
});

test('rules: three or more of - * _, spaced or not; two are text', () => {
  for (const rule of ['---', '***', '___', '- - -', '----------']) {
    assert.equal(M.render('a\n\n' + rule + '\n\nb'), BODY('<p class="an-md-p">a</p><hr class="an-md-rule"><p class="an-md-p">b</p>'), rule);
  }
  assert.equal(M.render('--'), P('--'));
});

test('the class option adds to the wrapper', () => {
  assert.equal(M.render('x', { cls: 'an-chat-md' }), '<div class="an-md an-chat-md"><p class="an-md-p">x</p></div>');
});

// ---- links ------------------------------------------------------------------------------------

test('an https link is text carrying its address; the label keeps its own inline markup', () => {
  assert.equal(M.render('[docs](https://example.com/a?b=1&c=2)'),
    P(LINK('https://example.com/a?b=1&amp;c=2', 'docs')));
  assert.equal(M.render('[**bold** docs](https://example.com)'),
    P(LINK('https://example.com', '<strong>bold</strong> docs')));
  assert.equal(M.render('<https://example.com/x>'), P(LINK('https://example.com/x', 'https://example.com/x')));
  // Words after the address are its title, not part of it.
  assert.equal(M.render('[a](https://e.com "a title")'), P(LINK('https://e.com', 'a')));
  assert.equal(M.render('[a](https://e.com b)'), P(LINK('https://e.com', 'a')));
  assert.equal(M.render('[a](<https://e.com/x>)'), P(LINK('https://e.com/x', 'a')));
  assert.equal(M.render('[wiki](https://en.wikipedia.org/wiki/Foo_(bar))'),
    P(LINK('https://en.wikipedia.org/wiki/Foo_(bar)', 'wiki')));
  assert.equal(M.render('[Docs](HTTPS://EXAMPLE.COM)'), P(LINK('HTTPS://EXAMPLE.COM', 'Docs')));
});

test('nothing but https is ever a link: the label stays as plain text', () => {
  const rejected = [
    'javascript:alert(1)', 'JaVaScRiPt:alert(1)', ' javascript:alert(1)', 'data:text/html,<b>x</b>',
    'http://example.com', 'file:///C:/Windows/win.ini', 'vbscript:x', 'ftp://x.y', '//evil.example',
    '/relative/path', 'mailto:a@b.c', 'https:evil', 'https://a"b', 'https://a\\b', 'x',
    'about:blank', 'blob:https://x/y', 'ms-settings:', 'tel:1',
  ];
  for (const url of rejected) {
    const out = M.render(`[label](${url})`);
    assert.equal(out, P('label'), url);
    assert.doesNotMatch(out, /data-an-url|href|role="link"/, url);
  }
  // A bare address and an autolink of another scheme are text too.
  assert.equal(M.render('see http://example.com now'), P('see http://example.com now'));
  assert.equal(M.render('<javascript:alert(1)>'), P('&lt;javascript:alert(1)&gt;'));
  assert.equal(M.render('<mailto:a@b.c>'), P('&lt;mailto:a@b.c&gt;'));
});

test('an image is not drawn: its description stands in as text', () => {
  assert.equal(M.render('![a chart](https://example.com/c.png)'), P('a chart'));
  assert.equal(M.render('![x](javascript:alert(1))'), P('x'));
  assert.doesNotMatch(M.render('![x](https://e.com/i.png)'), /<img|src=/);
});

test('an address cannot break out of its attributes', () => {
  assert.equal(M.safeUrl('https://a.b/c'), 'https://a.b/c');
  assert.equal(M.safeUrl(' https://a.b/c '), 'https://a.b/c');
  for (const bad of ['', null, undefined, 'https://', 'https://a"onmouseover="x', "https://a'b", 'https://a`b', 'https://a<b', 'https://a>b', 'javascript:1', 'HTTP://a.b']) {
    assert.equal(M.safeUrl(bad), bad === 'https://' ? 'https://' : null, String(bad));
  }
  const out = M.render('[x](https://a.b/?q=&quot;&amp;<i>)');
  assert.deepEqual(audit.problems(out), []);
});

// ---- no HTML passthrough --------------------------------------------------------------------------

test('HTML in the source is text: tags, comments, entities and attributes are escaped, never interpreted', () => {
  const cases = {
    '<b>bold</b>': P('&lt;b&gt;bold&lt;/b&gt;'),
    '<script>alert(1)</script>': P('&lt;script&gt;alert(1)&lt;/script&gt;'),
    '<img src=x onerror=alert(1)>': P('&lt;img src=x onerror=alert(1)&gt;'),
    '&lt;b&gt; &amp;': P('&amp;lt;b&amp;gt; &amp;amp;'),
    'a "quoted" \'single\'': P('a &quot;quoted&quot; &#39;single&#39;'),
  };
  for (const [source, expected] of Object.entries(cases)) assert.equal(M.render(source), expected, source);
  assert.equal(text(M.render('<b>bold</b>')), '<b>bold</b>');
  assert.equal(fragment(M.render('<b>bold</b>')).querySelector('b'), null);
});

test('block-level HTML is text too', () => {
  const out = M.render('<div onclick="x()">\n<iframe src="javascript:1"></iframe>\n</div>');
  assert.deepEqual(audit.problems(out), []);
  assert.equal(fragment(out).querySelector('iframe'), null);
  assert.match(text(out), /<iframe src="javascript:1"><\/iframe>/);
});

test('every tag rendered is one the renderers use, and every class is ours', () => {
  const source = [
    '# H', 'text **b** *i* ~~s~~ `c` [l](https://a.b) <https://c.d>', '- a\n  - b', '1. x', '> q', '---', '```\ncode\n```',
  ].join('\n\n');
  const root = fragment(M.render(source));
  assert.deepEqual(audit.problems(root), []);
  const tags = new Set(dom.elementsOf(root).map((el) => el.localName));
  assert.deepEqual([...tags].sort(), ['blockquote', 'code', 'div', 'em', 'hr', 'li', 'ol', 'p', 'pre', 's', 'span', 'strong', 'ul'].sort());
  for (const el of dom.elementsOf(root)) {
    const cls = el.getAttribute('class');
    if (cls) assert.match(cls, /^an-md(-[a-z0-9]+)*( an-md(-[a-z0-9]+)*)*$/, cls);
  }
});

// ---- hostile text ---------------------------------------------------------------------------------

const wrappers = {
  paragraph: (s) => s,
  'in a heading': (s) => '# ' + s,
  'in strong': (s) => '**' + s + '**',
  'in emphasis': (s) => '_' + s + '_',
  'in a link label': (s) => '[' + s + '](https://example.com)',
  'in a list': (s) => '- ' + s + '\n- ' + s,
  'in a quote': (s) => '> ' + s,
  'after a fence marker': (s) => '```' + s + '\nx\n```',
};

test('hostile strings through every position: no injected element, handler or link', () => {
  const long = 'B'.repeat(10000);
  for (const hostile of [...audit.HOSTILE, long, '‮' + 'x'.repeat(50)]) {
    for (const [where, wrap] of Object.entries(wrappers)) {
      const out = M.render(wrap(hostile));
      assert.deepEqual(audit.problems(out), [], `${where}: ${hostile.slice(0, 40)}`);
      // Whatever the text, nothing but the page's own https links may carry an address.
      for (const el of dom.elementsOf(fragment(out))) {
        if (el.hasAttribute('data-an-url')) {
          assert.match(el.getAttribute('data-an-url'), /^https:\/\/[^\s<>"'`\\]+$/, where);
        }
      }
    }
  }
});

test('hostile strings inside a fenced block and inline code come back exactly', () => {
  for (const hostile of [...audit.HOSTILE.filter((s) => !s.includes('\n')), '‮evil.exe‬', 'x'.repeat(10000)]) {
    const fenced = fragment(M.render('~~~~\n' + hostile + '\n~~~~'));
    assert.equal(fenced.querySelector('pre code').textContent, hostile);
    assert.deepEqual(audit.problems(fenced), []);
    if (!hostile.includes('`')) {
      const inline = fragment(M.render('`' + hostile + '`'));
      assert.equal(inline.querySelector('code').textContent, hostile.trim() === hostile ? hostile : hostile.trim());
    }
  }
});

test('plain hostile text survives as visible text in a paragraph', () => {
  const survives = [
    '<img src=x onerror=alert(1)>',
    '"><script>alert(1)</script>',
    "'><svg onload=alert(1)>",
    '<a href="javascript:alert(1)">x</a>',
    '‮evil.exe‬ reversed',
    '" onmouseover="alert(1)" x="',
    "javascript:alert(1)//' onclick='alert(1)",
    '<!-- --><style>*{display:none}</style><base href="https://evil.example/">',
    '<textarea></textarea><input autofocus onfocus=alert(1)>',
    'A'.repeat(10000),
  ];
  for (const s of survives) {
    const root = fragment(M.render(s));
    assert.equal(root.textContent, s);
    assert.equal(root.querySelectorAll('p').length, 1);
  }
});

test('U+202E is kept as a character, not stripped and not turned into markup', () => {
  const s = 'file‮gnp.exe';
  assert.equal(text(M.render(s)), s);
  assert.ok(M.render(s).includes('‮'));
  assert.equal(text(M.render('[' + s + '](https://x.y)')), s);
});

test('a 10 000-character line is one paragraph, escaped once', () => {
  const line = '<' + 'a&'.repeat(5000) + '>';
  const out = M.render(line);
  assert.equal(out.length, P('').length + '&lt;'.length + '&amp;'.length * 5000 + 'a'.length * 5000 + '&gt;'.length);
  assert.equal(text(out), line);
});

test('inline() renders one line without a block wrapper', () => {
  assert.equal(M.inline('a **b** `c`'), 'a <strong>b</strong> <code class="an-md-code">c</code>');
  assert.equal(M.inline(null), '');
  assert.equal(M.inline('<i>'), '&lt;i&gt;');
});

// ---- bounded work -------------------------------------------------------------------------------

test('pathological input renders in bounded time (no quadratic scans, no unbounded nesting)', () => {
  const cases = {
    'unclosed stars': '*a '.repeat(20000),
    'unclosed underscores': '_a '.repeat(20000),
    'unclosed strikethrough': '~~a '.repeat(20000),
    'unclosed brackets': '['.repeat(50000),
    'unclosed links': '[a]('.repeat(20000),
    'unclosed images': '![a]('.repeat(20000),
    'backticks': '`a'.repeat(30000),
    'nested quotes': '> '.repeat(5000) + 'x',
    'nested lists': '- '.repeat(5000) + 'x',
    'indented lists': Array.from({ length: 2000 }, (_, n) => ' '.repeat(n % 40) + '- x').join('\n'),
    'many fences': '```\n'.repeat(10000),
    'many lines': '- a\n'.repeat(20000),
  };
  for (const [name, source] of Object.entries(cases)) {
    const started = Date.now();
    const out = M.render(source);
    const took = Date.now() - started;
    assert.ok(took < 1500, `${name}: ${took} ms`);
    assert.deepEqual(audit.problems(out.slice(0, 200000)), [], name);
  }
});

test('nesting stops at a fixed depth and the rest is text', () => {
  const out = M.render('> '.repeat(30) + 'deep');
  assert.equal((out.match(/<blockquote/g) || []).length, 8);
  assert.match(text(out), /deep$/);
  const emph = M.render('*'.repeat(3) + 'x' + '*'.repeat(3));
  assert.deepEqual(audit.problems(emph), []);
});
