'use strict';
// A small DOM for the fork's page tests: enough of the platform for upstream's pages and the
// fork's scripts to run in node, with no dependency to install (the Windows job runs
// `node --test` on a bare Node 22).
//
// It parses the pages' real HTML, so a test drives the same markup WebView2 gets. What it
// cannot know is layout: every rectangle is whatever the test says it is (`el.__rect`), zero
// otherwise. Layout is checked in a real browser (snapshots.test.cjs).
//
// The parser is strict in the ways that matter for the hostile-string tests: text that a
// renderer failed to escape becomes elements and attributes here exactly as it would in a
// browser, so a test that walks the tree finds the injected <img> or the broken-out attribute.

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);
const RAW_TEXT = new Set(['script', 'style']);
const ESCAPABLE_RAW_TEXT = new Set(['textarea', 'title']);
const SVG_NS = 'http://www.w3.org/2000/svg';
const HTML_NS = 'http://www.w3.org/1999/xhtml';

const NAMED_ENTITIES = {
  amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', hellip: '…',
  mdash: '—', ndash: '–', middot: '·', rsaquo: '›', lsaquo: '‹',
  times: '×', copy: '©',
};

function decodeEntities(text) {
  if (text.indexOf('&') < 0) return text;
  return text.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[A-Za-z][A-Za-z0-9]*);?/g, (whole, body) => {
    if (body[0] === '#') {
      const code = body[1] === 'x' || body[1] === 'X' ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
      if (!Number.isFinite(code) || code <= 0 || code > 0x10ffff) return whole;
      return String.fromCodePoint(code);
    }
    return Object.prototype.hasOwnProperty.call(NAMED_ENTITIES, body) && whole.endsWith(';') ? NAMED_ENTITIES[body] : whole;
  });
}

function escapeText(text) {
  return String(text).replace(/[&<> ]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', ' ': '&nbsp;' }[c]));
}

function escapeAttribute(text) {
  return String(text).replace(/[&" ]/g, (c) => ({ '&': '&amp;', '"': '&quot;', ' ': '&nbsp;' }[c]));
}

// ---- selectors -----------------------------------------------------------------------------

function parseSelectorList(source) {
  const text = String(source).trim();
  let i = 0;
  const fail = (what) => {
    throw new SyntaxError(`selector not supported by the test DOM (${what}): ${source}`);
  };
  const ident = () => {
    const m = /^(?:\\.|[A-Za-z0-9_\- -￿])+/.exec(text.slice(i));
    if (!m) fail('name');
    i += m[0].length;
    return m[0].replace(/\\(.)/g, '$1');
  };
  const skipSpace = () => {
    while (i < text.length && /\s/.test(text[i])) i += 1;
  };
  function compound() {
    const c = { tag: null, id: null, classes: [], attrs: [], pseudos: [] };
    let any = false;
    for (;;) {
      const ch = text[i];
      if (ch === '*') {
        i += 1;
        any = true;
      } else if (ch === '#') {
        i += 1;
        c.id = ident();
        any = true;
      } else if (ch === '.') {
        i += 1;
        c.classes.push(ident());
        any = true;
      } else if (ch === '[') {
        i += 1;
        skipSpace();
        const name = ident().toLowerCase();
        skipSpace();
        let op = null;
        let value = null;
        const opMatch = /^(=|\^=|\$=|\*=|~=|\|=)/.exec(text.slice(i));
        if (opMatch) {
          op = opMatch[1];
          i += op.length;
          skipSpace();
          if (text[i] === '"' || text[i] === "'") {
            const quote = text[i];
            const end = text.indexOf(quote, i + 1);
            if (end < 0) fail('unterminated string');
            value = text.slice(i + 1, end);
            i = end + 1;
          } else {
            value = ident();
          }
          skipSpace();
        }
        if (text[i] !== ']') fail('attribute');
        i += 1;
        c.attrs.push({ name, op, value });
        any = true;
      } else if (ch === ':') {
        i += 1;
        const name = ident();
        let arg = null;
        if (text[i] === '(') {
          let depth = 1;
          let j = i + 1;
          while (j < text.length && depth > 0) {
            if (text[j] === '(') depth += 1;
            else if (text[j] === ')') depth -= 1;
            j += 1;
          }
          if (depth) fail('parenthesis');
          arg = text.slice(i + 1, j - 1);
          i = j;
        }
        if (name === 'not') c.pseudos.push({ name, list: parseSelectorList(arg) });
        else if (['first-child', 'last-child', 'checked', 'disabled', 'enabled', 'empty'].includes(name)) c.pseudos.push({ name });
        else fail(`:${name}`);
        any = true;
      } else if (ch && /[A-Za-z]/.test(ch) && !any) {
        c.tag = ident().toLowerCase();
        any = true;
      } else {
        break;
      }
    }
    if (!any) fail('empty');
    return c;
  }
  const list = [];
  for (;;) {
    skipSpace();
    const parts = [{ combinator: null, compound: compound() }];
    for (;;) {
      const before = i;
      skipSpace();
      const ch = text[i];
      if (ch === undefined || ch === ',') break;
      let combinator = ' ';
      if (ch === '>' || ch === '+' || ch === '~') {
        combinator = ch;
        i += 1;
        skipSpace();
      } else if (before === i) {
        fail('combinator');
      }
      parts.push({ combinator, compound: compound() });
    }
    list.push(parts);
    skipSpace();
    if (text[i] === ',') {
      i += 1;
      continue;
    }
    if (i < text.length) fail('trailing text');
    break;
  }
  return list;
}

const selectorCache = new Map();
function selectorList(source) {
  let list = selectorCache.get(source);
  if (!list) {
    list = parseSelectorList(source);
    selectorCache.set(source, list);
  }
  return list;
}

function matchesCompound(el, c) {
  if (c.tag && el.localName !== c.tag) return false;
  if (c.id !== null && el.getAttribute('id') !== c.id) return false;
  for (const cls of c.classes) if (!el.classList.contains(cls)) return false;
  for (const a of c.attrs) {
    const value = el.getAttribute(a.name);
    if (value === null) return false;
    if (a.op === null) continue;
    if (a.op === '=' && value !== a.value) return false;
    if (a.op === '^=' && !(a.value && value.startsWith(a.value))) return false;
    if (a.op === '$=' && !(a.value && value.endsWith(a.value))) return false;
    if (a.op === '*=' && !(a.value && value.includes(a.value))) return false;
    if (a.op === '~=' && !value.split(/\s+/).includes(a.value)) return false;
    if (a.op === '|=' && !(value === a.value || value.startsWith(`${a.value}-`))) return false;
  }
  for (const p of c.pseudos) {
    if (p.name === 'not' && p.list.some((parts) => matchesParts(el, parts, parts.length - 1))) return false;
    if (p.name === 'first-child' && el.previousElementSibling) return false;
    if (p.name === 'last-child' && el.nextElementSibling) return false;
    if (p.name === 'checked' && !el.checked) return false;
    if (p.name === 'disabled' && !el.hasAttribute('disabled')) return false;
    if (p.name === 'enabled' && el.hasAttribute('disabled')) return false;
    if (p.name === 'empty' && el.childNodes.length) return false;
  }
  return true;
}

function matchesParts(el, parts, index) {
  if (!matchesCompound(el, parts[index].compound)) return false;
  if (index === 0) return true;
  const combinator = parts[index].combinator;
  if (combinator === '>') {
    const parent = el.parentElement;
    return !!parent && matchesParts(parent, parts, index - 1);
  }
  if (combinator === '+') {
    const previous = el.previousElementSibling;
    return !!previous && matchesParts(previous, parts, index - 1);
  }
  if (combinator === '~') {
    for (let p = el.previousElementSibling; p; p = p.previousElementSibling) if (matchesParts(p, parts, index - 1)) return true;
    return false;
  }
  for (let p = el.parentElement; p; p = p.parentElement) if (matchesParts(p, parts, index - 1)) return true;
  return false;
}

function matchesSelector(el, source) {
  return selectorList(source).some((parts) => matchesParts(el, parts, parts.length - 1));
}

// ---- mutation observers --------------------------------------------------------------------

function notify(target, record) {
  const doc = target.nodeType === 9 ? target : target.ownerDocument;
  if (!doc || !doc.__observers.length) return;
  for (const observer of doc.__observers) {
    for (const watch of observer.__targets) {
      const inScope = watch.node === target || (watch.options.subtree && watch.node.contains(target));
      if (!inScope) continue;
      if (record.type === 'childList' && !watch.options.childList) continue;
      if (record.type === 'characterData' && !watch.options.characterData) continue;
      if (record.type === 'attributes') {
        if (!watch.options.attributes) continue;
        const filter = watch.options.attributeFilter;
        if (filter && !filter.includes(record.attributeName)) continue;
      }
      observer.__queue.push(Object.assign({ target, addedNodes: [], removedNodes: [] }, record));
      if (!observer.__scheduled) {
        observer.__scheduled = true;
        queueMicrotask(() => {
          observer.__scheduled = false;
          const records = observer.__queue.splice(0);
          if (records.length) observer.__callback(records, observer);
        });
      }
      break;
    }
  }
}

function makeMutationObserver(doc) {
  return class MutationObserver {
    constructor(callback) {
      this.__callback = callback;
      this.__targets = [];
      this.__queue = [];
      this.__scheduled = false;
    }

    observe(node, options) {
      const opts = Object.assign({}, options);
      if (opts.attributeFilter && opts.attributes === undefined) opts.attributes = true;
      this.__targets.push({ node, options: opts });
      if (!doc.__observers.includes(this)) doc.__observers.push(this);
    }

    disconnect() {
      this.__targets = [];
      doc.__observers = doc.__observers.filter((o) => o !== this);
    }

    takeRecords() {
      return this.__queue.splice(0);
    }
  };
}

// ---- nodes -----------------------------------------------------------------------------------

class Node {
  constructor(nodeType, ownerDocument) {
    this.nodeType = nodeType;
    this.ownerDocument = ownerDocument;
    this.parentNode = null;
    this.childNodes = [];
  }

  get firstChild() {
    return this.childNodes[0] || null;
  }

  get lastChild() {
    return this.childNodes[this.childNodes.length - 1] || null;
  }

  get nextSibling() {
    const p = this.parentNode;
    if (!p) return null;
    return p.childNodes[p.childNodes.indexOf(this) + 1] || null;
  }

  get previousSibling() {
    const p = this.parentNode;
    if (!p) return null;
    return p.childNodes[p.childNodes.indexOf(this) - 1] || null;
  }

  get parentElement() {
    return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null;
  }

  get isConnected() {
    let n = this;
    while (n.parentNode) n = n.parentNode;
    return n.nodeType === 9;
  }

  contains(other) {
    for (let n = other; n; n = n.parentNode) if (n === this) return true;
    return false;
  }

  hasChildNodes() {
    return this.childNodes.length > 0;
  }

  appendChild(node) {
    return this.insertBefore(node, null);
  }

  insertBefore(node, reference) {
    if (node.nodeType === 11) {
      for (const child of node.childNodes.slice()) this.insertBefore(child, reference);
      return node;
    }
    if (node === reference) return node;
    if (node.contains(this)) throw new Error('HierarchyRequestError: a node cannot hold its own ancestor');
    if (node.parentNode) node.parentNode.removeChild(node);
    let index = this.childNodes.length;
    if (reference) {
      index = this.childNodes.indexOf(reference);
      if (index < 0) throw new Error('NotFoundError: the reference node is not a child of this node');
    }
    this.childNodes.splice(index, 0, node);
    node.parentNode = this;
    adopt(node, this.nodeType === 9 ? this : this.ownerDocument);
    notify(this, { type: 'childList', addedNodes: [node] });
    const doc = this.nodeType === 9 ? this : this.ownerDocument;
    if (doc && node.nodeType === 1 && this.isConnected) doc.__connected(node);
    return node;
  }

  removeChild(node) {
    const index = this.childNodes.indexOf(node);
    if (index < 0) throw new Error('NotFoundError: the node to remove is not a child of this node');
    this.childNodes.splice(index, 1);
    node.parentNode = null;
    notify(this, { type: 'childList', removedNodes: [node] });
    const doc = this.nodeType === 9 ? this : this.ownerDocument;
    if (doc && doc.activeElement && (doc.activeElement === node || node.contains(doc.activeElement))) doc.__active = null;
    return node;
  }

  replaceChild(node, old) {
    this.insertBefore(node, old);
    this.removeChild(old);
    return old;
  }

  remove() {
    if (this.parentNode) this.parentNode.removeChild(this);
  }

  get textContent() {
    if (this.nodeType === 3 || this.nodeType === 8) return this.nodeValue;
    let out = '';
    for (const child of this.childNodes) if (child.nodeType !== 8) out += child.textContent;
    return out;
  }

  set textContent(value) {
    if (this.nodeType === 3 || this.nodeType === 8) {
      this.nodeValue = value;
      return;
    }
    for (const child of this.childNodes.slice()) this.removeChild(child);
    const text = value == null ? '' : String(value);
    if (text) this.appendChild(this.ownerDocument.createTextNode(text));
  }

  cloneNode(deep) {
    const doc = this.ownerDocument;
    let copy;
    if (this.nodeType === 3) copy = doc.createTextNode(this.nodeValue);
    else if (this.nodeType === 8) copy = doc.createComment(this.nodeValue);
    else if (this.nodeType === 11) copy = doc.createDocumentFragment();
    else {
      copy = doc.createElementNS(this.namespaceURI, this.localName);
      for (const [name, value] of this.__attrs) copy.setAttribute(name, value);
    }
    if (deep) for (const child of this.childNodes) copy.appendChild(child.cloneNode(true));
    return copy;
  }

  // Events: capture is not modelled; listeners run on the target, then on each ancestor.
  addEventListener(type, listener, options) {
    if (typeof listener !== 'function') return;
    if (!this.__listeners) this.__listeners = new Map();
    if (!this.__listeners.has(type)) this.__listeners.set(type, []);
    const once = !!(options && typeof options === 'object' && options.once);
    this.__listeners.get(type).push({ listener, once });
  }

  removeEventListener(type, listener) {
    const list = this.__listeners && this.__listeners.get(type);
    if (!list) return;
    const index = list.findIndex((entry) => entry.listener === listener);
    if (index >= 0) list.splice(index, 1);
  }

  dispatchEvent(event) {
    if (!event.target) event.target = this;
    const path = [];
    for (let n = this; n; n = n.parentNode) path.push(n);
    const doc = this.nodeType === 9 ? this : this.ownerDocument;
    if (doc && doc.defaultView && path[path.length - 1] === doc) path.push(doc.defaultView.__eventTarget);
    for (const node of path) {
      if (node !== this && !event.bubbles) break;
      event.currentTarget = node.__window || node;
      const handler = node[`on${event.type}`];
      if (typeof handler === 'function') handler.call(event.currentTarget, event);
      const list = node.__listeners && node.__listeners.get(event.type);
      if (list) {
        for (const entry of list.slice()) {
          if (entry.once) list.splice(list.indexOf(entry), 1);
          entry.listener.call(event.currentTarget, event);
          if (event.__stopImmediate) break;
        }
      }
      if (event.__stopped) break;
    }
    return !event.defaultPrevented;
  }
}

function adopt(node, doc) {
  if (!doc || node.ownerDocument === doc) return;
  node.ownerDocument = doc;
  for (const child of node.childNodes) adopt(child, doc);
}

class Text extends Node {
  constructor(doc, value) {
    super(3, doc);
    this.__value = String(value);
  }

  get nodeValue() {
    return this.__value;
  }

  set nodeValue(value) {
    this.__value = value == null ? '' : String(value);
    notify(this, { type: 'characterData' });
  }

  get data() {
    return this.__value;
  }

  set data(value) {
    this.nodeValue = value;
  }

  get nodeName() {
    return '#text';
  }
}

class Comment extends Node {
  constructor(doc, value) {
    super(8, doc);
    this.nodeValue = String(value);
  }

  get nodeName() {
    return '#comment';
  }
}

class DocumentFragment extends Node {
  constructor(doc) {
    super(11, doc);
  }

  get children() {
    return this.childNodes.filter((n) => n.nodeType === 1);
  }

  querySelector(selector) {
    return queryAll(this, selector, true)[0] || null;
  }

  querySelectorAll(selector) {
    return queryAll(this, selector, false);
  }
}

function queryAll(root, selector, firstOnly) {
  const list = selectorList(selector);
  const out = [];
  const visit = (node) => {
    for (const child of node.childNodes) {
      if (child.nodeType !== 1) continue;
      if (list.some((parts) => matchesParts(child, parts, parts.length - 1))) {
        out.push(child);
        if (firstOnly) return true;
      }
      if (visit(child)) return true;
    }
    return false;
  };
  visit(root);
  return out;
}

function camel(name) {
  return name.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
}

function kebab(name) {
  return name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
}

function parseStyle(text) {
  const map = new Map();
  for (const part of String(text || '').split(';')) {
    const cut = part.indexOf(':');
    if (cut < 0) continue;
    const name = part.slice(0, cut).trim();
    const value = part.slice(cut + 1).trim();
    if (name) map.set(name.startsWith('--') ? name : name.toLowerCase(), value);
  }
  return map;
}

function styleProxy(el) {
  const read = () => parseStyle(el.getAttribute('style'));
  const write = (map) => {
    const text = [...map].map(([k, v]) => `${k}: ${v};`).join(' ');
    if (text) el.setAttribute('style', text);
    else el.removeAttribute('style');
  };
  const api = {
    setProperty(name, value) {
      const map = read();
      if (value == null || value === '') map.delete(name);
      else map.set(name, String(value));
      write(map);
    },
    getPropertyValue(name) {
      return read().get(name) || '';
    },
    removeProperty(name) {
      const map = read();
      const old = map.get(name) || '';
      map.delete(name);
      write(map);
      return old;
    },
  };
  return new Proxy(api, {
    get(target, key) {
      if (key in target) return target[key];
      if (typeof key !== 'string') return undefined;
      if (key === 'cssText') return el.getAttribute('style') || '';
      return read().get(kebab(key)) || '';
    },
    set(target, key, value) {
      if (typeof key !== 'string') return true;
      if (key === 'cssText') {
        if (value) el.setAttribute('style', String(value));
        else el.removeAttribute('style');
        return true;
      }
      api.setProperty(kebab(key), value);
      return true;
    },
  });
}

const REFLECTED_BOOLEANS = ['hidden', 'disabled', 'readonly', 'multiple', 'required', 'async', 'defer', 'open'];
const REFLECTED_STRINGS = ['id', 'title', 'lang', 'src', 'href', 'rel', 'type', 'name', 'placeholder', 'role', 'alt', 'dir'];

class Element extends Node {
  constructor(doc, localName, namespaceURI) {
    super(1, doc);
    this.namespaceURI = namespaceURI || HTML_NS;
    this.localName = this.namespaceURI === HTML_NS ? localName.toLowerCase() : localName;
    this.__attrs = new Map();
    this.__rect = null;
    this.scrollTop = 0;
    this.scrollLeft = 0;
  }

  get tagName() {
    return this.namespaceURI === HTML_NS ? this.localName.toUpperCase() : this.localName;
  }

  get nodeName() {
    return this.tagName;
  }

  // attributes

  getAttribute(name) {
    const key = this.namespaceURI === HTML_NS ? String(name).toLowerCase() : String(name);
    return this.__attrs.has(key) ? this.__attrs.get(key) : null;
  }

  setAttribute(name, value) {
    const key = this.namespaceURI === HTML_NS ? String(name).toLowerCase() : String(name);
    if (!/^[^\s"'<>/=]+$/.test(key)) throw new Error(`InvalidCharacterError: attribute name "${name}"`);
    this.__attrs.set(key, String(value));
    notify(this, { type: 'attributes', attributeName: key });
  }

  removeAttribute(name) {
    const key = this.namespaceURI === HTML_NS ? String(name).toLowerCase() : String(name);
    if (this.__attrs.delete(key)) notify(this, { type: 'attributes', attributeName: key });
  }

  hasAttribute(name) {
    return this.getAttribute(name) !== null;
  }

  toggleAttribute(name, force) {
    const on = force === undefined ? !this.hasAttribute(name) : !!force;
    if (on) this.setAttribute(name, '');
    else this.removeAttribute(name);
    return on;
  }

  getAttributeNames() {
    return [...this.__attrs.keys()];
  }

  get attributes() {
    return [...this.__attrs].map(([name, value]) => ({ name, value }));
  }

  get className() {
    return this.getAttribute('class') || '';
  }

  set className(value) {
    this.setAttribute('class', value);
  }

  get classList() {
    const el = this;
    const read = () => (el.getAttribute('class') || '').split(/\s+/).filter(Boolean);
    const write = (list) => {
      if (list.length) el.setAttribute('class', list.join(' '));
      else if (el.hasAttribute('class')) el.setAttribute('class', '');
    };
    return {
      contains: (name) => read().includes(name),
      add: (...names) => {
        const list = read();
        for (const n of names) if (!list.includes(n)) list.push(n);
        write(list);
      },
      remove: (...names) => write(read().filter((n) => !names.includes(n))),
      toggle: (name, force) => {
        const list = read();
        const has = list.includes(name);
        const want = force === undefined ? !has : !!force;
        if (want && !has) list.push(name);
        write(want ? list : list.filter((n) => n !== name));
        return want;
      },
      get length() {
        return read().length;
      },
      toString: () => read().join(' '),
      [Symbol.iterator]: () => read()[Symbol.iterator](),
    };
  }

  get dataset() {
    const el = this;
    return new Proxy({}, {
      get(_, key) {
        if (typeof key !== 'string') return undefined;
        const value = el.getAttribute(`data-${kebab(key)}`);
        return value === null ? undefined : value;
      },
      set(_, key, value) {
        el.setAttribute(`data-${kebab(String(key))}`, String(value));
        return true;
      },
      deleteProperty(_, key) {
        el.removeAttribute(`data-${kebab(String(key))}`);
        return true;
      },
      has(_, key) {
        return typeof key === 'string' && el.hasAttribute(`data-${kebab(key)}`);
      },
      ownKeys() {
        return el.getAttributeNames().filter((n) => n.startsWith('data-')).map((n) => camel(n.slice(5)));
      },
      getOwnPropertyDescriptor(_, key) {
        const value = el.getAttribute(`data-${kebab(String(key))}`);
        return value === null ? undefined : { value, enumerable: true, configurable: true, writable: true };
      },
    });
  }

  get style() {
    if (!this.__style) this.__style = styleProxy(this);
    return this.__style;
  }

  set style(value) {
    this.setAttribute('style', String(value));
  }

  // tree

  get children() {
    return this.childNodes.filter((n) => n.nodeType === 1);
  }

  get childElementCount() {
    return this.children.length;
  }

  get firstElementChild() {
    return this.children[0] || null;
  }

  get lastElementChild() {
    const kids = this.children;
    return kids[kids.length - 1] || null;
  }

  get nextElementSibling() {
    for (let n = this.nextSibling; n; n = n.nextSibling) if (n.nodeType === 1) return n;
    return null;
  }

  get previousElementSibling() {
    for (let n = this.previousSibling; n; n = n.previousSibling) if (n.nodeType === 1) return n;
    return null;
  }

  append(...nodes) {
    for (const n of nodes) this.appendChild(typeof n === 'string' ? this.ownerDocument.createTextNode(n) : n);
  }

  matches(selector) {
    return matchesSelector(this, selector);
  }

  closest(selector) {
    for (let el = this; el; el = el.parentElement) if (matchesSelector(el, selector)) return el;
    return null;
  }

  querySelector(selector) {
    return queryAll(this, selector, true)[0] || null;
  }

  querySelectorAll(selector) {
    return queryAll(this, selector, false);
  }

  getElementsByTagName(name) {
    return queryAll(this, name === '*' ? '*' : name.toLowerCase(), false);
  }

  // markup

  get innerHTML() {
    const holder = this.localName === 'template' && this.namespaceURI === HTML_NS ? this.content : this;
    return holder.childNodes.map((n) => serialize(n, this)).join('');
  }

  set innerHTML(html) {
    const holder = this.localName === 'template' && this.namespaceURI === HTML_NS ? this.content : this;
    for (const child of holder.childNodes.slice()) holder.removeChild(child);
    parseInto(holder, String(html == null ? '' : html), this.ownerDocument, this);
    // One record for the whole replacement, as a browser reports it. Scripts that arrive this
    // way never run (in a browser either), so the page loader is not told.
    if (holder.childNodes.length) notify(holder, { type: 'childList', addedNodes: holder.childNodes.slice() });
  }

  get outerHTML() {
    return serialize(this, this.parentNode);
  }

  insertAdjacentHTML(position, html) {
    const fragment = this.ownerDocument.createDocumentFragment();
    const context = position === 'beforebegin' || position === 'afterend' ? this.parentElement : this;
    parseInto(fragment, String(html), this.ownerDocument, context);
    const nodes = fragment.childNodes.slice();
    if (position === 'beforeend') for (const n of nodes) this.appendChild(n);
    else if (position === 'afterbegin') {
      const first = this.firstChild;
      for (const n of nodes) this.insertBefore(n, first);
    } else if (position === 'beforebegin') for (const n of nodes) this.parentNode.insertBefore(n, this);
    else if (position === 'afterend') {
      const next = this.nextSibling;
      for (const n of nodes) this.parentNode.insertBefore(n, next);
    } else throw new SyntaxError(`insertAdjacentHTML: ${position}`);
  }

  insertAdjacentElement(position, element) {
    if (position === 'beforeend') this.appendChild(element);
    else if (position === 'afterbegin') this.insertBefore(element, this.firstChild);
    else if (position === 'beforebegin') this.parentNode.insertBefore(element, this);
    else if (position === 'afterend') this.parentNode.insertBefore(element, this.nextSibling);
    return element;
  }

  // template

  get content() {
    if (this.localName !== 'template') return undefined;
    if (!this.__content) this.__content = new DocumentFragment(this.ownerDocument);
    return this.__content;
  }

  // form state

  get value() {
    if (this.localName === 'textarea') return this.__value !== undefined ? this.__value : this.textContent;
    if (this.localName === 'select') {
      const options = this.querySelectorAll('option');
      const picked = options.find((o) => o.selected) || options[0];
      return picked ? picked.value : '';
    }
    if (this.localName === 'option') return this.hasAttribute('value') ? this.getAttribute('value') : this.textContent;
    if (this.__value !== undefined) return this.__value;
    return this.getAttribute('value') || '';
  }

  set value(next) {
    if (this.localName === 'select') {
      for (const o of this.querySelectorAll('option')) o.selected = o.value === String(next);
      return;
    }
    if (this.localName === 'option') {
      this.setAttribute('value', next);
      return;
    }
    this.__value = next == null ? '' : String(next);
  }

  get checked() {
    return this.__checked !== undefined ? this.__checked : this.hasAttribute('checked');
  }

  set checked(on) {
    this.__checked = !!on;
  }

  get selected() {
    return this.__selected !== undefined ? this.__selected : this.hasAttribute('selected');
  }

  set selected(on) {
    this.__selected = !!on;
  }

  get readOnly() {
    return this.hasAttribute('readonly');
  }

  set readOnly(on) {
    this.toggleAttribute('readonly', !!on);
  }

  get tabIndex() {
    const raw = this.getAttribute('tabindex');
    return raw === null ? (['a', 'button', 'input', 'select', 'textarea'].includes(this.localName) ? 0 : -1) : Number(raw);
  }

  set tabIndex(value) {
    this.setAttribute('tabindex', String(value));
  }

  get selectionStart() {
    return this.__selectionStart !== undefined ? this.__selectionStart : this.value.length;
  }

  set selectionStart(n) {
    this.__selectionStart = n;
  }

  get selectionEnd() {
    return this.__selectionEnd !== undefined ? this.__selectionEnd : this.value.length;
  }

  set selectionEnd(n) {
    this.__selectionEnd = n;
  }

  setSelectionRange(start, end) {
    this.__selectionStart = start;
    this.__selectionEnd = end;
  }

  select() {
    this.__selectionStart = 0;
    this.__selectionEnd = this.value.length;
  }

  // focus and clicks

  focus() {
    const doc = this.ownerDocument;
    if (doc.activeElement === this || !this.isConnected) return;
    const previous = doc.__active;
    doc.__active = this;
    if (previous) {
      previous.dispatchEvent(new Event('blur'));
      previous.dispatchEvent(new Event('focusout', { bubbles: true }));
    }
    this.dispatchEvent(new Event('focus'));
    this.dispatchEvent(new Event('focusin', { bubbles: true }));
  }

  blur() {
    const doc = this.ownerDocument;
    if (doc.__active !== this) return;
    doc.__active = null;
    this.dispatchEvent(new Event('blur'));
    this.dispatchEvent(new Event('focusout', { bubbles: true }));
  }

  click() {
    if (this.hasAttribute('disabled')) return;
    this.dispatchEvent(new Event('click', { bubbles: true, cancelable: true }));
  }

  // layout: whatever the test sets, zero otherwise

  getBoundingClientRect() {
    const r = this.__rect || { left: 0, top: 0, width: 0, height: 0 };
    return {
      left: r.left, top: r.top, width: r.width, height: r.height, x: r.left, y: r.top,
      right: r.left + r.width, bottom: r.top + r.height,
    };
  }

  get offsetWidth() {
    return this.__rect ? this.__rect.width : 0;
  }

  get offsetHeight() {
    return this.__rect ? this.__rect.height : 0;
  }

  get clientWidth() {
    return this.offsetWidth;
  }

  get clientHeight() {
    return this.offsetHeight;
  }

  get scrollWidth() {
    return this.__scrollWidth !== undefined ? this.__scrollWidth : this.offsetWidth;
  }

  get scrollHeight() {
    return this.__scrollHeight !== undefined ? this.__scrollHeight : this.offsetHeight;
  }

  scrollIntoView() {}

  scrollTo() {}

  animate() {
    return { playState: 'finished', cancel() {}, finished: Promise.resolve(), onfinish: null };
  }

  getAnimations() {
    return [];
  }
}

for (const name of REFLECTED_BOOLEANS) {
  if (name === 'readonly') continue;
  Object.defineProperty(Element.prototype, name, {
    configurable: true,
    get() {
      return this.hasAttribute(name);
    },
    set(on) {
      this.toggleAttribute(name, !!on);
    },
  });
}
for (const name of REFLECTED_STRINGS) {
  Object.defineProperty(Element.prototype, name, {
    configurable: true,
    get() {
      return this.getAttribute(name) || '';
    },
    set(value) {
      this.setAttribute(name, value);
    },
  });
}

class Event {
  constructor(type, init) {
    this.type = type;
    this.bubbles = !!(init && init.bubbles);
    this.cancelable = !!(init && init.cancelable);
    this.defaultPrevented = false;
    this.target = null;
    this.currentTarget = null;
    this.__stopped = false;
    this.__stopImmediate = false;
    this.timeStamp = 0;
    if (init) {
      for (const key of Object.keys(init)) {
        if (!(key in this) || this[key] === undefined) this[key] = init[key];
      }
    }
  }

  preventDefault() {
    if (this.cancelable) this.defaultPrevented = true;
  }

  stopPropagation() {
    this.__stopped = true;
  }

  stopImmediatePropagation() {
    this.__stopped = true;
    this.__stopImmediate = true;
  }
}

// ---- parsing and serialising -----------------------------------------------------------------

function serialize(node, parent) {
  if (node.nodeType === 3) {
    const raw = parent && parent.nodeType === 1 && parent.namespaceURI === HTML_NS && RAW_TEXT.has(parent.localName);
    return raw ? node.nodeValue : escapeText(node.nodeValue);
  }
  if (node.nodeType === 8) return `<!--${node.nodeValue}-->`;
  if (node.nodeType === 11) return node.childNodes.map((n) => serialize(n, node)).join('');
  let out = `<${node.localName}`;
  for (const [name, value] of node.__attrs) out += ` ${name}="${escapeAttribute(value)}"`;
  out += '>';
  if (node.namespaceURI === HTML_NS && VOID.has(node.localName)) return out;
  const holder = node.localName === 'template' && node.namespaceURI === HTML_NS ? node.content : node;
  out += holder.childNodes.map((n) => serialize(n, node)).join('');
  return `${out}</${node.localName}>`;
}

/**
 * Parses `html` into children of `parent`. `context` is the element the markup is parsed
 * inside (its namespace decides whether `<circle/>` closes itself).
 */
function parseInto(parent, html, doc, context) {
  let i = 0;
  const n = html.length;
  const stack = [];
  const top = () => (stack.length ? stack[stack.length - 1] : parent);
  const holderOf = (el) => (el.nodeType === 1 && el.localName === 'template' && el.namespaceURI === HTML_NS ? el.content : el);
  const namespaceHere = () => {
    const current = stack.length ? stack[stack.length - 1] : context;
    return current && current.nodeType === 1 ? current.namespaceURI : HTML_NS;
  };
  const addText = (text) => {
    if (!text) return;
    const holder = holderOf(top());
    const last = holder.lastChild;
    if (last && last.nodeType === 3) last.__value += text;
    else holder.childNodes.push(Object.assign(new Text(doc, text), { parentNode: holder }));
  };
  const addNode = (node) => {
    const holder = holderOf(top());
    holder.childNodes.push(node);
    node.parentNode = holder;
  };

  while (i < n) {
    const lt = html.indexOf('<', i);
    if (lt < 0) {
      addText(decodeEntities(html.slice(i)));
      break;
    }
    if (lt > i) addText(decodeEntities(html.slice(i, lt)));
    i = lt;
    if (html.startsWith('<!--', i)) {
      const end = html.indexOf('-->', i + 4);
      const stop = end < 0 ? n : end;
      addNode(new Comment(doc, html.slice(i + 4, stop)));
      i = end < 0 ? n : end + 3;
      continue;
    }
    if (html[i + 1] === '!' || html[i + 1] === '?') {
      const end = html.indexOf('>', i);
      i = end < 0 ? n : end + 1;
      continue;
    }
    if (html[i + 1] === '/') {
      const m = /^<\/([A-Za-z][^\s/>]*)[^>]*>/.exec(html.slice(i, i + 200));
      if (!m) {
        // `</` with no tag name: a bogus comment in a browser; nothing is drawn.
        const end = html.indexOf('>', i);
        i = end < 0 ? n : end + 1;
        continue;
      }
      const name = m[1];
      for (let s = stack.length - 1; s >= 0; s--) {
        const el = stack[s];
        const same = el.namespaceURI === HTML_NS ? el.localName === name.toLowerCase() : el.localName === name;
        if (same) {
          stack.length = s;
          break;
        }
      }
      i += m[0].length;
      continue;
    }
    if (!/[A-Za-z]/.test(html[i + 1] || '')) {
      addText('<');
      i += 1;
      continue;
    }

    // A start tag.
    let j = i + 1;
    while (j < n && !/[\s/>]/.test(html[j])) j += 1;
    const rawName = html.slice(i + 1, j);
    const lower = rawName.toLowerCase();
    const ns = lower === 'svg' ? SVG_NS : namespaceHere() === SVG_NS ? SVG_NS : HTML_NS;
    const el = new Element(doc, ns === HTML_NS ? lower : rawName, ns);
    let selfClosing = false;
    for (;;) {
      while (j < n && /\s/.test(html[j])) j += 1;
      if (j >= n) break;
      if (html[j] === '>') {
        j += 1;
        break;
      }
      if (html[j] === '/') {
        if (html[j + 1] === '>') {
          selfClosing = true;
          j += 2;
          break;
        }
        j += 1;
        continue;
      }
      let k = j;
      while (k < n && !/[\s=/>]/.test(html[k])) k += 1;
      // A name that starts with `=` (as in `<a =x>`) still names an attribute in a browser.
      if (k === j) k += 1;
      const attrName = html.slice(j, k);
      j = k;
      while (j < n && /\s/.test(html[j])) j += 1;
      let value = '';
      if (html[j] === '=') {
        j += 1;
        while (j < n && /\s/.test(html[j])) j += 1;
        if (html[j] === '"' || html[j] === "'") {
          const quote = html[j];
          const end = html.indexOf(quote, j + 1);
          const stop = end < 0 ? n : end;
          value = html.slice(j + 1, stop);
          j = end < 0 ? n : end + 1;
        } else {
          let v = j;
          while (v < n && !/[\s>]/.test(html[v])) v += 1;
          value = html.slice(j, v);
          j = v;
        }
      }
      const key = ns === HTML_NS ? attrName.toLowerCase() : attrName;
      if (!el.__attrs.has(key)) el.__attrs.set(key, decodeEntities(value));
    }
    i = j;
    addNode(el);
    if (ns === HTML_NS && VOID.has(lower)) continue;
    if (selfClosing && ns === SVG_NS) continue;
    if (ns === HTML_NS && (RAW_TEXT.has(lower) || ESCAPABLE_RAW_TEXT.has(lower))) {
      const close = new RegExp(`</${lower}\\s*>`, 'i');
      const rest = html.slice(i);
      const m = close.exec(rest);
      const body = m ? rest.slice(0, m.index) : rest;
      if (body) {
        const text = RAW_TEXT.has(lower) ? body : decodeEntities(body);
        el.childNodes.push(Object.assign(new Text(doc, text), { parentNode: el }));
      }
      i += m ? m.index + m[0].length : rest.length;
      continue;
    }
    stack.push(el);
  }
}

// ---- document ----------------------------------------------------------------------------------

class TreeWalker {
  constructor(root, whatToShow) {
    this.root = root;
    this.whatToShow = whatToShow === undefined ? 0xffffffff : whatToShow;
    this.currentNode = root;
  }

  __accepts(node) {
    return (this.whatToShow & (1 << (node.nodeType - 1))) !== 0;
  }

  nextNode() {
    let node = this.currentNode;
    for (;;) {
      if (node.firstChild) {
        node = node.firstChild;
      } else {
        while (node && node !== this.root && !node.nextSibling) node = node.parentNode;
        if (!node || node === this.root) return null;
        node = node.nextSibling;
      }
      if (this.__accepts(node)) {
        this.currentNode = node;
        return node;
      }
    }
  }
}

class Document extends Node {
  constructor() {
    super(9, null);
    this.__observers = [];
    this.__active = null;
    this.__scriptHook = null;
    this.readyState = 'loading';
    this.currentScript = null;
    this.defaultView = null;
    this.visibilityState = 'visible';
    this.hidden = false;
  }

  get nodeName() {
    return '#document';
  }

  get documentElement() {
    return this.childNodes.find((n) => n.nodeType === 1) || null;
  }

  get head() {
    const root = this.documentElement;
    return root ? root.children.find((c) => c.localName === 'head') || null : null;
  }

  get body() {
    const root = this.documentElement;
    return root ? root.children.find((c) => c.localName === 'body') || null : null;
  }

  get activeElement() {
    return this.__active || this.body;
  }

  get title() {
    const el = this.querySelector('title');
    return el ? el.textContent : '';
  }

  set title(value) {
    let el = this.querySelector('title');
    if (!el) {
      el = this.createElement('title');
      (this.head || this.documentElement).appendChild(el);
    }
    el.textContent = value;
  }

  createElement(name) {
    return new Element(this, String(name), HTML_NS);
  }

  createElementNS(ns, name) {
    return new Element(this, String(name), ns || HTML_NS);
  }

  createTextNode(text) {
    return new Text(this, text);
  }

  createComment(text) {
    return new Comment(this, text);
  }

  createDocumentFragment() {
    return new DocumentFragment(this);
  }

  createTreeWalker(root, whatToShow) {
    return new TreeWalker(root, whatToShow);
  }

  createRange() {
    // Ranges measure glyph boxes; with no layout there is nothing to measure.
    throw new Error('NotSupportedError: the test DOM has no ranges');
  }

  getElementById(id) {
    const want = String(id);
    const visit = (node) => {
      for (const child of node.childNodes) {
        if (child.nodeType !== 1) continue;
        if (child.getAttribute('id') === want) return child;
        const found = visit(child);
        if (found) return found;
      }
      return null;
    };
    return visit(this);
  }

  querySelector(selector) {
    return queryAll(this, selector, true)[0] || null;
  }

  querySelectorAll(selector) {
    return queryAll(this, selector, false);
  }

  getElementsByTagName(name) {
    return queryAll(this, name.toLowerCase(), false);
  }

  /** A node joined the document: a `<script src>` among it is handed to the page loader. */
  __connected(node) {
    if (!this.__scriptHook) return;
    const scripts = node.localName === 'script' ? [node] : node.querySelectorAll('script');
    for (const script of scripts) {
      if (script.__started || !script.getAttribute('src')) continue;
      script.__started = true;
      this.__scriptHook(script);
    }
  }
}

/**
 * Parses a whole page. Returns the document; `<script>` elements are left for the caller to
 * run in order (`document.querySelectorAll('script')`).
 */
function parseDocument(html) {
  const doc = new Document();
  const holder = new DocumentFragment(doc);
  parseInto(holder, html, doc, null);
  // Pages here always write <html>, <head> and <body>; one that didn't gets them.
  let root = holder.childNodes.find((n) => n.nodeType === 1 && n.localName === 'html');
  if (!root) {
    root = doc.createElement('html');
    const head = doc.createElement('head');
    const body = doc.createElement('body');
    root.childNodes.push(head, body);
    head.parentNode = root;
    body.parentNode = root;
    for (const child of holder.childNodes.slice()) {
      const target = child.nodeType === 1 && ['title', 'meta', 'link', 'style'].includes(child.localName) ? head : body;
      target.childNodes.push(child);
      child.parentNode = target;
    }
  } else {
    // upstream's pages close </head> and open <body> explicitly; nothing is moved.
    if (!root.children.some((c) => c.localName === 'head')) {
      const head = doc.createElement('head');
      root.childNodes.unshift(head);
      head.parentNode = root;
    }
    if (!root.children.some((c) => c.localName === 'body')) {
      const body = doc.createElement('body');
      root.childNodes.push(body);
      body.parentNode = root;
    }
  }
  doc.childNodes.push(root);
  root.parentNode = doc;
  return doc;
}

/** Parses a fragment of markup into a detached container, for assertions on rendered HTML. */
function parseFragment(html) {
  const doc = new Document();
  const root = doc.createElement('div');
  doc.childNodes.push(root);
  root.parentNode = doc;
  root.innerHTML = html;
  return root;
}

/** Every element under `root`, in document order. */
function elementsOf(root) {
  const out = [];
  const visit = (node) => {
    const holder = node.nodeType === 1 && node.localName === 'template' && node.namespaceURI === HTML_NS ? node.content : node;
    for (const child of holder.childNodes) {
      if (child.nodeType !== 1) continue;
      out.push(child);
      visit(child);
    }
  };
  visit(root);
  return out;
}

module.exports = {
  parseDocument,
  parseFragment,
  elementsOf,
  decodeEntities,
  escapeText,
  Event,
  Element,
  Text,
  Document,
  makeMutationObserver,
  SVG_NS,
  HTML_NS,
};
