'use strict';
// Hostile text, and the check that a renderer drew it as text.
//
// Session titles, transcript text, tool output, account and organisation names and paths all
// reach the pages from outside the app. A renderer that forgot to escape one of them would turn
// it into markup in a window that can approve commands and sign in to the website. The app-wide
// CSP stops scripts from running anyway; these tests keep the text from becoming markup at all.

const dom = require('./dom.cjs');

/** Strings that become elements, attributes or links the moment a renderer fails to escape. */
const HOSTILE = [
  '<img src=x onerror=alert(1)>',
  '"><script>alert(1)</script>',
  "'><svg onload=alert(1)>",
  '</div></div><iframe src="javascript:alert(1)"></iframe>',
  '[click](javascript:alert(1))',
  '<a href="javascript:alert(1)">x</a>',
  '‮evil.exe‬ reversed',
  '`${alert(1)}` &lt;b&gt; &amp;amp; <b>bold</b>',
  '" onmouseover="alert(1)" x="',
  "javascript:alert(1)//' onclick='alert(1)",
  'A'.repeat(10000),
  '<!-- --><style>*{display:none}</style><base href="https://evil.example/">',
  '<textarea></textarea><input autofocus onfocus=alert(1)>',
];

/** The elements the fork's renderers draw with. Anything else in rendered markup is an injection. */
const TAGS = new Set([
  'div', 'span', 'p', 'button', 'ul', 'ol', 'li', 'pre', 'code', 'strong', 'em', 's', 'br', 'hr',
  'blockquote', 'label', 'input', 'textarea', 'select', 'option', 'img', 'section', 'header', 'h1', 'h2', 'h3',
  'svg', 'g', 'circle', 'path', 'rect', 'line',
]);

const URL_ATTRIBUTES = ['href', 'src', 'action', 'formaction', 'xlink:href', 'srcdoc', 'data', 'poster'];

/**
 * What is wrong with rendered markup (an HTML string or an element), as a list of sentences;
 * empty when every tag is one the renderers use, no attribute is an event handler, nothing
 * links anywhere by itself and every image is inline data.
 */
function problems(markup, options) {
  const root = typeof markup === 'string' ? dom.parseFragment(markup) : markup;
  const allow = new Set([...(options && options.allowTags ? options.allowTags : [])]);
  const found = [];
  for (const el of dom.elementsOf(root)) {
    const tag = el.localName;
    if (!TAGS.has(tag) && !allow.has(tag)) found.push(`<${tag}> in rendered markup`);
    for (const { name, value } of el.attributes) {
      if (/^on/i.test(name)) found.push(`<${tag} ${name}>: an event-handler attribute`);
      // Only where a URL is read: a title or an aria-label that shows the text "javascript:…"
      // is a correctly escaped hostile string, not an injection.
      if (URL_ATTRIBUTES.includes(name) && /^\s*javascript:/i.test(value)) found.push(`<${tag} ${name}="${value.slice(0, 40)}">: a javascript: URL`);
      if (URL_ATTRIBUTES.includes(name)) {
        const ok = tag === 'img' && name === 'src' && /^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/=]*$/.test(value);
        if (!ok) found.push(`<${tag} ${name}="${value.slice(0, 40)}">: markup must not load or link anything`);
      }
      if (name === 'style' && /url\s*\(|expression\s*\(|@import/i.test(value)) found.push(`<${tag} style>: ${value.slice(0, 60)}`);
    }
  }
  return found;
}

/** `text` with whitespace runs collapsed, the way a test compares what is on screen. */
function flat(text) {
  return String(text).replace(/\s+/g, ' ').trim();
}

module.exports = { HOSTILE, TAGS, problems, flat };
