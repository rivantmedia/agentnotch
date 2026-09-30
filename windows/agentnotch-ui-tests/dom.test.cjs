'use strict';
// The test DOM (lib/dom.cjs) and the markup audit (lib/audit.cjs) tested on their own: every
// other suite trusts them, and the hostile-string tests are only as good as the parser that
// turns unescaped text into elements and attributes.

const test = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./lib/dom.cjs');
const audit = require('./lib/audit.cjs');

const frag = (html) => dom.parseFragment(html);
const doc = () => dom.parseDocument('<!doctype html><html><head><title>t</title></head><body></body></html>');

// ---- parsing --------------------------------------------------------------------------------

test('the parser builds nested elements, attributes and text', () => {
  const root = frag('<div id="a" class="x y" data-k=v hidden><span>one</span> two<br><input value="q" disabled></div>');
  const div = root.querySelector('#a');
  assert.equal(div.localName, 'div');
  assert.equal(div.getAttribute('class'), 'x y');
  assert.equal(div.getAttribute('data-k'), 'v');
  assert.equal(div.getAttribute('hidden'), '');
  assert.equal(div.hasAttribute('hidden'), true);
  assert.equal(div.children.length, 3);
  assert.equal(div.textContent, 'one two');
  assert.equal(div.querySelector('input').getAttribute('value'), 'q');
  assert.equal(div.querySelector('input').hasAttribute('disabled'), true);
});

test('void elements take no children and elements close where a browser closes them', () => {
  const root = frag('<p>a<br>b<img src="data:image/png;base64,AA=="><hr>c</p>');
  const p = root.querySelector('p');
  assert.equal(p.querySelectorAll('br').length, 1);
  assert.equal(p.querySelector('br').childNodes.length, 0);
  assert.equal(p.textContent, 'abc');
});

test('entities decode in text and in attributes, and unknown ones stay as written', () => {
  const root = frag('<span title="a &amp; b &quot;q&quot; &#65;&#x42;">&lt;b&gt; &amp;amp; &notanentity; &copy;</span>');
  const span = root.querySelector('span');
  assert.equal(span.getAttribute('title'), 'a & b "q" AB');
  assert.equal(span.textContent, '<b> &amp; &notanentity; ©');
  assert.equal(span.children.length, 0);
});

test('script and style hold raw text, textarea and title decode it', () => {
  const root = frag('<script>if (a < b && c > d) { x = "</p>"; }</script><style>a > b { color: red }</style><textarea>&lt;i&gt;<b>x</b></textarea>');
  assert.equal(root.querySelector('script').textContent, 'if (a < b && c > d) { x = "</p>"; }');
  assert.equal(root.querySelector('script').children.length, 0);
  assert.equal(root.querySelector('style').textContent, 'a > b { color: red }');
  assert.equal(root.querySelector('textarea').textContent, '<i><b>x</b>');
  assert.equal(root.querySelector('textarea').children.length, 0);
});

test('comments are not text', () => {
  const root = frag('<div>a<!-- <b>hidden</b> -->b</div>');
  assert.equal(root.querySelector('div').textContent, 'ab');
  assert.equal(root.querySelectorAll('b').length, 0);
});

test('svg children are in the svg namespace and self-close', () => {
  const root = frag('<svg viewBox="0 0 10 10"><circle cx="5" cy="5" r="3"/><path d="M0 0"/></svg><p>x</p>');
  const svg = root.querySelector('svg');
  assert.equal(svg.namespaceURI, dom.SVG_NS);
  assert.equal(svg.children.length, 2);
  assert.equal(svg.querySelector('circle').namespaceURI, dom.SVG_NS);
  assert.equal(root.querySelector('p').namespaceURI, dom.HTML_NS);
});

test('a template keeps its markup in .content, not in its children', () => {
  const d = doc();
  const tpl = d.createElement('template');
  tpl.innerHTML = '<div class="k">x</div><p>y</p>';
  assert.equal(tpl.children.length, 0);
  assert.equal(tpl.content.children.length, 2);
  assert.equal(tpl.content.querySelector('.k').textContent, 'x');
  assert.equal(tpl.innerHTML, '<div class="k">x</div><p>y</p>');
});

test('a document parses head, body, title and scripts in order', () => {
  const d = dom.parseDocument('<!doctype html><html><head><meta charset="utf-8"><title>Hi &amp; bye</title><script src="a.js"></script></head><body><script>1</script><p id="p">x</p></body></html>');
  assert.equal(d.title, 'Hi & bye');
  assert.equal(d.head.querySelectorAll('meta').length, 1);
  assert.deepEqual(d.querySelectorAll('script').map((s) => s.getAttribute('src')), ['a.js', null]);
  assert.equal(d.getElementById('p').textContent, 'x');
  assert.equal(d.body.querySelector('p').isConnected, true);
});

test('markup round-trips through innerHTML and outerHTML', () => {
  const d = doc();
  const host = d.createElement('div');
  host.innerHTML = '<a class="a" data-x="1 &amp; 2">t &lt; u</a><br>';
  assert.equal(host.innerHTML, '<a class="a" data-x="1 &amp; 2">t &lt; u</a><br>');
  assert.equal(host.firstChild.outerHTML, '<a class="a" data-x="1 &amp; 2">t &lt; u</a>');
});

// ---- hostile markup ----------------------------------------------------------------------------

test('text a renderer forgot to escape becomes real elements and attributes', () => {
  const img = frag('<div><img src=x onerror=alert(1)></div>').querySelector('img');
  assert.ok(img, 'the <img> is an element');
  assert.equal(img.getAttribute('onerror'), 'alert(1)');
  const broken = frag('<div title="">"><script>alert(1)</script>"></div>');
  assert.equal(broken.querySelectorAll('script').length, 1);
  const attr = frag('<div title="" onmouseover="alert(1)" x="">t</div>').querySelector('div');
  assert.equal(attr.getAttribute('onmouseover'), 'alert(1)');
  const single = frag("<div title='javascript:alert(1)//' onclick='alert(1)'>t</div>").querySelector('div');
  assert.equal(single.getAttribute('onclick'), 'alert(1)');
});

test('audit.problems finds every injection in an unescaped hostile string', () => {
  const asText = (s) => audit.problems(`<div>${s}</div>`);
  const asDoubleQuoted = (s) => audit.problems(`<div title="${s}">t</div>`);
  const asSingleQuoted = (s) => audit.problems(`<div title='${s}'>t</div>`);
  const [imgHandler, scriptBreak, svgLoad, iframe, , anchor, , entityBold, handlerBreak, singleBreak, , styleBase, textareaInput] = audit.HOSTILE;
  for (const s of [imgHandler, scriptBreak, svgLoad, iframe, anchor, entityBold, styleBase, textareaInput]) {
    assert.notEqual(asText(s).length, 0, `not found as text: ${s.slice(0, 50)}`);
  }
  // (The iframe's own quotes end the attribute early but leave its tag as text: not an element.)
  for (const s of [scriptBreak, handlerBreak]) {
    assert.notEqual(asDoubleQuoted(s).length, 0, `not found in a double-quoted attribute: ${s.slice(0, 50)}`);
  }
  assert.notEqual(asSingleQuoted(singleBreak).length, 0);
  assert.notEqual(asSingleQuoted(svgLoad).length, 0);
});

test('audit.problems names what it found', () => {
  assert.deepEqual(audit.problems('<img src=x onerror=alert(1)>'), [
    '<img src="x">: markup must not load or link anything',
    '<img onerror>: an event-handler attribute',
  ]);
  assert.match(audit.problems('<a href="javascript:alert(1)">x</a>').join('|'), /<a> in rendered markup/);
  assert.match(audit.problems('<a href="javascript:alert(1)">x</a>').join('|'), /a javascript: URL/);
  assert.match(audit.problems('<div style="background:url(x)">x</div>').join('|'), /style/);
  assert.match(audit.problems('<iframe></iframe><base href="https://e.example/">').join('|'), /<iframe> in rendered markup/);
});

test('audit.problems flags javascript: where a URL is read, not in text that only shows it', () => {
  assert.deepEqual(audit.problems('<div title="javascript:alert(1)" aria-label="javascript:alert(1)" data-x="javascript:1">javascript:alert(1)</div>'), []);
  assert.match(audit.problems('<button formaction="javascript:alert(1)">x</button>').join('|'), /a javascript: URL/);
  assert.match(audit.problems('<img src=" JavaScript:alert(1)">').join('|'), /a javascript: URL/);
});

test('audit.problems accepts the markup the renderers draw with', () => {
  assert.deepEqual(audit.problems('<div class="an-row" data-key="a" style="width:5px"><span data-an-text>x</span><button data-an-session="s">Go</button><svg class="an-icon" viewBox="0 0 12 12"><path d="M1 1"/></svg><img src="data:image/png;base64,iVBORw0KGgo="></div>'), []);
  assert.deepEqual(audit.problems('plain text, no markup'), []);
  assert.deepEqual(audit.problems(frag('<p>ok</p>')), []);
  assert.deepEqual(audit.problems('<a>x</a>', { allowTags: ['a'] }), []);
});

test('audit.problems refuses an image that is not inline data', () => {
  assert.notEqual(audit.problems('<img src="https://e.example/x.png">').length, 0);
  assert.notEqual(audit.problems('<img src="data:text/html;base64,AAAA">').length, 0);
  assert.notEqual(audit.problems('<img src="data:image/svg+xml;base64,AAAA">').length, 0);
});

test('HOSTILE holds the ten-thousand-character name and the right-to-left override', () => {
  assert.ok(audit.HOSTILE.some((s) => s.length >= 10000));
  assert.ok(audit.HOSTILE.some((s) => s.includes('‮')));
  assert.ok(audit.HOSTILE.length >= 12);
});

// ---- selectors --------------------------------------------------------------------------------

const SAMPLE = '<div id="root" class="a b"><ul><li class="i first" data-id="1">one</li><li class="i" data-id="2" data-tag="x-y">two</li><li class="i last" data-id="3">three</li></ul><p class="i" hidden>p</p><input type="checkbox" checked><input type="checkbox" disabled></div>';

test('selectors: tag, id, class, attribute operators, combinators, :not, :first-child and lists', () => {
  const root = frag(SAMPLE);
  const q = (s) => root.querySelectorAll(s).map((e) => e.textContent || e.localName);
  assert.deepEqual(q('li'), ['one', 'two', 'three']);
  assert.deepEqual(q('#root > ul > li.i'), ['one', 'two', 'three']);
  assert.deepEqual(q('.a.b li.last'), ['three']);
  assert.deepEqual(q('li[data-id="2"]'), ['two']);
  assert.deepEqual(q('[data-tag^=x]'), ['two']);
  assert.deepEqual(q('[data-tag$="-y"]'), ['two']);
  assert.deepEqual(q('[data-tag*="-"]'), ['two']);
  assert.deepEqual(q('li:not(.first):not(.last)'), ['two']);
  assert.deepEqual(q('li:first-child'), ['one']);
  assert.deepEqual(q('li:last-child'), ['three']);
  assert.deepEqual(q('li.first + li'), ['two']);
  assert.deepEqual(q('li.first ~ li'), ['two', 'three']);
  assert.deepEqual(q('p, li.last'), ['three', 'p']);
  assert.deepEqual(q('[hidden]'), ['p']);
  assert.equal(root.querySelectorAll('input:checked').length, 1);
  assert.equal(root.querySelectorAll('input:disabled').length, 1);
  assert.equal(root.querySelectorAll('input:enabled').length, 1);
  assert.equal(root.querySelector('nothing'), null);
});

test('selectors: matches, closest and a selector the DOM does not know', () => {
  const root = frag(SAMPLE);
  const li = root.querySelector('li.last');
  assert.equal(li.matches('ul > li'), true);
  assert.equal(li.closest('#root').getAttribute('id'), 'root');
  assert.equal(li.closest('ul').localName, 'ul');
  assert.equal(li.closest('table'), null);
  assert.throws(() => root.querySelector('li:nth-child(2)'), /not supported/);
});

// ---- events -------------------------------------------------------------------------------------

test('events bubble target-first, stop, prevent and run once', () => {
  const root = frag('<div id="o"><div id="m"><button id="b">x</button></div></div>');
  const order = [];
  const button = root.querySelector('#b');
  root.querySelector('#o').addEventListener('click', () => order.push('o'));
  root.querySelector('#m').addEventListener('click', (e) => {
    order.push('m');
    e.preventDefault();
  });
  button.addEventListener('click', () => order.push('b'));
  button.addEventListener('click', () => order.push('once'), { once: true });
  const first = new dom.Event('click', { bubbles: true, cancelable: true });
  assert.equal(button.dispatchEvent(first), false);
  assert.deepEqual(order, ['b', 'once', 'm', 'o']);
  assert.equal(first.target, button);
  order.length = 0;
  button.dispatchEvent(new dom.Event('click', { bubbles: true }));
  assert.deepEqual(order, ['b', 'm', 'o']);
  order.length = 0;
  root.querySelector('#m').addEventListener('mouseover', (e) => e.stopPropagation());
  root.querySelector('#o').addEventListener('mouseover', () => order.push('o'));
  button.dispatchEvent(new dom.Event('mouseover', { bubbles: true }));
  assert.deepEqual(order, []);
  button.dispatchEvent(new dom.Event('focus'));
  assert.deepEqual(order, []);
});

test('a click on a disabled button does nothing, and removeEventListener works', () => {
  const root = frag('<button id="b" disabled>x</button><button id="c">y</button>');
  let clicks = 0;
  const handler = () => { clicks += 1; };
  root.querySelector('#b').addEventListener('click', handler);
  root.querySelector('#c').addEventListener('click', handler);
  root.querySelector('#b').click();
  assert.equal(clicks, 0);
  root.querySelector('#c').click();
  assert.equal(clicks, 1);
  root.querySelector('#c').removeEventListener('click', handler);
  root.querySelector('#c').click();
  assert.equal(clicks, 1);
});

test('focus moves between connected elements with blur and focusout, and a detached one cannot take it', () => {
  const d = doc();
  d.body.innerHTML = '<input id="a"><input id="b">';
  const seen = [];
  for (const id of ['a', 'b']) {
    for (const type of ['focus', 'blur', 'focusin', 'focusout']) d.getElementById(id).addEventListener(type, () => seen.push(`${id}:${type}`));
  }
  d.getElementById('a').focus();
  d.getElementById('b').focus();
  assert.equal(d.activeElement, d.getElementById('b'));
  assert.deepEqual(seen, ['a:focus', 'a:focusin', 'a:blur', 'a:focusout', 'b:focus', 'b:focusin']);
  const loose = d.createElement('input');
  loose.focus();
  assert.equal(d.activeElement, d.getElementById('b'));
  d.getElementById('b').blur();
  assert.equal(d.activeElement, d.body);
});

test('form state: value, checked, select, textarea and selection', () => {
  const d = doc();
  d.body.innerHTML = '<input id="i" value="v"><textarea id="t">seed</textarea><select id="s"><option value="a">A</option><option value="b" selected>B</option></select><input id="c" type="checkbox">';
  assert.equal(d.getElementById('i').value, 'v');
  d.getElementById('i').value = 'typed';
  assert.equal(d.getElementById('i').value, 'typed');
  assert.equal(d.getElementById('i').getAttribute('value'), 'v');
  assert.equal(d.getElementById('t').value, 'seed');
  d.getElementById('t').value = 'more';
  assert.equal(d.getElementById('t').value, 'more');
  assert.equal(d.getElementById('s').value, 'b');
  d.getElementById('s').value = 'a';
  assert.equal(d.getElementById('s').value, 'a');
  assert.equal(d.getElementById('c').checked, false);
  d.getElementById('c').checked = true;
  assert.equal(d.getElementById('c').checked, true);
});

test('classList, dataset and style behave like the platform\'s', () => {
  const el = frag('<div class="a" data-foo-bar="1"></div>').querySelector('div');
  el.classList.add('b', 'a');
  assert.equal(el.className, 'a b');
  assert.equal(el.classList.toggle('c'), true);
  assert.equal(el.classList.toggle('c'), false);
  el.classList.remove('a');
  assert.equal(el.className, 'b');
  assert.equal(el.dataset.fooBar, '1');
  el.dataset.newKey = 'x';
  assert.equal(el.getAttribute('data-new-key'), 'x');
  assert.equal(el.dataset.missing, undefined);
  el.style.color = 'red';
  el.style.setProperty('--an-x', '4px');
  assert.match(el.getAttribute('style'), /color:\s*red/);
  assert.equal(el.style.getPropertyValue('--an-x'), '4px');
});

test('rectangles are zero unless a test says otherwise', () => {
  const el = frag('<div></div>').querySelector('div');
  assert.equal(el.getBoundingClientRect().width, 0);
  el.__rect = { left: 4, top: 8, width: 20, height: 10 };
  const r = el.getBoundingClientRect();
  assert.deepEqual([r.left, r.top, r.width, r.height, r.right, r.bottom], [4, 8, 20, 10, 24, 18]);
});

// ---- mutation observers ------------------------------------------------------------------------

const tick = () => new Promise((resolve) => setImmediate(resolve));

test('MutationObserver reports child lists, attributes and text once per turn', async () => {
  const d = doc();
  const Observer = dom.makeMutationObserver(d);
  const records = [];
  const observer = new Observer((r) => records.push(...r));
  d.body.innerHTML = '<div id="h"><span>t</span></div>';
  const host = d.getElementById('h');
  observer.observe(host, { childList: true, attributes: true, subtree: true, characterData: true });
  host.appendChild(d.createElement('b'));
  host.setAttribute('data-x', '1');
  host.querySelector('span').firstChild.nodeValue = 'u';
  assert.equal(records.length, 0, 'delivery is asynchronous');
  await tick();
  assert.deepEqual(records.map((r) => r.type), ['childList', 'attributes', 'characterData']);
  assert.equal(records[1].attributeName, 'data-x');
  records.length = 0;
  observer.disconnect();
  host.setAttribute('data-x', '2');
  await tick();
  assert.deepEqual(records, []);
});

test('MutationObserver honours the attribute filter and the subtree flag', async () => {
  const d = doc();
  const Observer = dom.makeMutationObserver(d);
  d.body.innerHTML = '<div id="h"><p id="p">x</p></div>';
  const seen = [];
  const observer = new Observer((r) => seen.push(...r.map((x) => `${x.type}:${x.attributeName || ''}`)));
  observer.observe(d.getElementById('h'), { attributeFilter: ['aria-label'] });
  d.getElementById('h').setAttribute('title', 'no');
  d.getElementById('h').setAttribute('aria-label', 'yes');
  d.getElementById('p').setAttribute('aria-label', 'not observed: no subtree');
  await tick();
  assert.deepEqual(seen, ['attributes:aria-label']);
});

test('setting innerHTML is one childList record, and textContent replaces the children', async () => {
  const d = doc();
  const Observer = dom.makeMutationObserver(d);
  const seen = [];
  const observer = new Observer((r) => seen.push(...r));
  observer.observe(d.body, { childList: true, subtree: true });
  d.body.innerHTML = '<p>a</p><p>b</p>';
  await tick();
  assert.equal(seen.length, 1);
  assert.equal(seen[0].addedNodes.length, 2);
  d.body.textContent = '<b>not markup</b>';
  assert.equal(d.body.children.length, 0);
  assert.equal(d.body.textContent, '<b>not markup</b>');
  assert.equal(d.body.innerHTML, '&lt;b&gt;not markup&lt;/b&gt;');
});

// ---- textContent escaping ----------------------------------------------------------------------

test('textContent and setAttribute never make markup, and serialising them re-escapes', () => {
  const d = doc();
  const host = d.createElement('div');
  for (const s of audit.HOSTILE) {
    host.textContent = s;
    assert.equal(host.children.length, 0);
    assert.equal(host.textContent, s);
    assert.deepEqual(audit.problems(host.innerHTML), [], `innerHTML of textContent: ${s.slice(0, 40)}`);
    host.setAttribute('title', s);
    assert.equal(host.getAttribute('title'), s);
    assert.deepEqual(audit.problems(host.outerHTML), [], `outerHTML with a hostile attribute: ${s.slice(0, 40)}`);
    assert.equal(host.attributes.length, 1);
  }
});

test('createElement, cloneNode and insertBefore keep the tree consistent', () => {
  const d = doc();
  d.body.innerHTML = '<ul><li>a</li><li>c</li></ul>';
  const ul = d.querySelector('ul');
  const b = d.createElement('li');
  b.textContent = 'b';
  ul.insertBefore(b, ul.lastChild);
  assert.equal(ul.textContent, 'abc');
  assert.equal(b.previousElementSibling.textContent, 'a');
  assert.equal(b.nextElementSibling.textContent, 'c');
  const copy = ul.cloneNode(true);
  assert.equal(copy.textContent, 'abc');
  assert.equal(copy.isConnected, false);
  ul.removeChild(b);
  assert.equal(ul.textContent, 'ac');
  assert.equal(copy.textContent, 'abc');
  ul.firstChild.remove();
  assert.equal(ul.children.length, 1);
});

test('elementsOf walks every element, including a template\'s content', () => {
  const d = doc();
  d.body.innerHTML = '<div><p></p><template><i></i></template></div>';
  assert.deepEqual(dom.elementsOf(d.body).map((e) => e.localName), ['div', 'p', 'template', 'i']);
});
