// Upstream's "Codenotch" copy, renamed to this app (DESIGN-WIN §4.16, §5.5; the Mac's
// `Fork.rebranded`). The engine's `core::rebrand` does the same for the tray and window titles;
// both are held to `agentnotch-engine/tests/ui-contract/rebrand-vectors.json`.
//
// The rules, in order:
//  - copy that names one of upstream's own products (the Codenotch phone app, Codenotch for
//    Windows, its installer, its author's site) is left exactly as it is;
//  - "a Codenotch" becomes "an Agent Notch" (the name starts with a vowel);
//  - compounds join every word: "Codenotch-Einstellungen" → "Agent-Notch-Einstellungen";
//  - every other "Codenotch" becomes "Agent Notch".
//
// One IIFE, one global (`window.agentnotchRebrand`), no top-level let/const.
(function () {
  'use strict';

  var UPSTREAM = 'Codenotch';
  var NAME = 'Agent Notch';
  var PRODUCT_PHRASES = [
    'Codenotch app on your phone', 'Codenotch on your phone', 'Codenotch phone app',
    'Codenotch for Windows', 'Codenotch-Setup', 'hivinz.com',
  ];

  function namesUpstreamProduct(text) {
    for (var i = 0; i < PRODUCT_PHRASES.length; i++) {
      if (text.indexOf(PRODUCT_PHRASES[i]) >= 0) return true;
    }
    return false;
  }

  /** `text` with upstream's name replaced by this app's, by the rules above. */
  function rebrand(value) {
    if (value == null) return value;
    var text = String(value);
    if (text.indexOf(UPSTREAM) < 0 || namesUpstreamProduct(text)) return text;
    return text
      .replace(/\b([aA]) Codenotch\b/g, '$1n ' + NAME)
      .replace(/Codenotch-/g, NAME.replace(/ /g, '-') + '-')
      .replace(/Codenotch/g, NAME);
  }

  // Text the fork draws itself, and user data inside upstream's rows, is never touched: the
  // Claude Code pane is the fork's own English copy (where "Codenotch" names the official app
  // on purpose: "Remove Codenotch's hooks"), and `data-user` marks names that came from
  // someone's account, not from upstream.
  var SKIP = '#pane-claude, [data-user], script, style';

  function skipped(node) {
    var el = node.nodeType === 1 ? node : node.parentElement;
    return !el || !!el.closest(SKIP);
  }

  function rebrandText(node) {
    if (skipped(node)) return;
    var v = node.nodeValue;
    if (v && v.indexOf(UPSTREAM) >= 0) {
      var next = rebrand(v);
      if (next !== v) node.nodeValue = next;
    }
  }

  function rebrandAttributes(el) {
    if (skipped(el)) return;
    ['aria-label', 'title', 'placeholder'].forEach(function (name) {
      var v = el.getAttribute(name);
      if (v && v.indexOf(UPSTREAM) >= 0) {
        var next = rebrand(v);
        if (next !== v) el.setAttribute(name, next);
      }
    });
  }

  /** Rebrands every text node and label attribute under `root`. */
  function rebrandTree(root) {
    if (!root) return;
    if (root.nodeType === 3) {
      rebrandText(root);
      return;
    }
    if (root.nodeType !== 1 && root.nodeType !== 9 && root.nodeType !== 11) return;
    var walker = document.createTreeWalker(root, 4 /* NodeFilter.SHOW_TEXT */);
    var texts = [];
    while (walker.nextNode()) texts.push(walker.currentNode);
    texts.forEach(rebrandText);
    if (root.nodeType === 1) rebrandAttributes(root);
    if (root.querySelectorAll) root.querySelectorAll('[aria-label],[title],[placeholder]').forEach(rebrandAttributes);
  }

  /**
   * Keeps `root` rebranded: the changes upstream's renderers make later (a language switch, a
   * provider row, the strip under the title) are rebranded as they land. A rebranded node no
   * longer names upstream, so the observer's own edits end the chain at once.
   */
  function observe(root) {
    if (!root || typeof MutationObserver !== 'function') return null;
    var observer = new MutationObserver(function (records) {
      records.forEach(function (r) {
        if (r.type === 'characterData') rebrandText(r.target);
        else if (r.type === 'attributes') rebrandAttributes(r.target);
        else r.addedNodes.forEach(rebrandTree);
      });
    });
    observer.observe(root, {
      subtree: true, childList: true, characterData: true,
      attributes: true, attributeFilter: ['aria-label', 'title', 'placeholder'],
    });
    return observer;
  }

  window.agentnotchRebrand = {
    rebrand: rebrand,
    rebrandTree: rebrandTree,
    observe: observe,
    PRODUCT_PHRASES: PRODUCT_PHRASES.slice(),
  };
})();
