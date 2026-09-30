'use strict';
// The fork's shared scripts (ui/agentnotch/*.js) in a context of their own, with a document,
// no bridge and a hand-moved clock: for suites of the renderers, which need no page.

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const dom = require('./dom.cjs');
const harness = require('./harness.cjs');

const DIR = path.join(harness.UI, 'agentnotch');

/**
 * Runs `names` (file names without `.js`, common first when others need it) in document order
 * in one fresh context. `html` is the document's `<body>` markup. Returns the context's
 * `window`, its `document` and the clock.
 */
function load(names, options) {
  const opts = Object.assign({}, options);
  const clock = harness.createClock(opts.now === undefined ? harness.NOW : opts.now);
  const document = dom.parseDocument(
    '<!doctype html><html><head></head><body>' + (opts.html || '') + '</body></html>');
  const context = vm.createContext({});
  const window = vm.runInContext('this', context);
  Object.assign(window, {
    window, document, console, Promise, queueMicrotask,
    MutationObserver: dom.makeMutationObserver(document),
    NodeFilter: { SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_TEXT: 4, SHOW_COMMENT: 128 },
    matchMedia: () => ({ matches: false }),
  });
  for (const name of names) {
    const file = path.join(DIR, name + '.js');
    vm.runInContext(fs.readFileSync(file, 'utf8'), context, { filename: file });
    if (name === 'common') window.agentnotchCommon.setClock(clock.now);
  }
  return { window, document, clock, context };
}

/** JSON round trip: objects made inside a context have that context's prototypes. */
const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

module.exports = { load, plain, DIR };
