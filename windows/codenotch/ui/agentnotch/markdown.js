// Claude's markdown as escaped HTML for the chat (MarkdownRenderer.swift): headings,
// paragraphs with emphasis, inline code, strikethrough and links, lists, quotes, fenced code
// and rules.
//
// Transcript text is untrusted: it can hold anything a model or a tool wrote. So this is a
// renderer, not a sanitiser: it never passes HTML through. Every piece of text goes out through
// `agentnotchCommon.esc`, markup comes only from the fixed tags below, and a link is drawn as
// text carrying its address in `data-an-url` only when that address is `https:`; the page opens
// it through the glue's `open_url` on a click, and nothing else is ever a link.
//
// One IIFE, one global (`window.agentnotchMarkdown`), no top-level let/const.
(function () {
  'use strict';

  var C = window.agentnotchCommon;
  var esc = C.esc;

  // A fence is its run of backticks or tildes; the rest of the line may hold no backtick. Tested
  // in two steps, not as one pattern: `\s*([^`\s]*)[^`]*$` backtracks quadratically over a
  // long run of spaces that ends in a backtick (a 100 000-character line took seconds).
  var FENCE_RUN = /^ {0,3}(`{3,}|~{3,})/;
  // A heading's marker; its text and closing run are cut by headingOf, not by one pattern: the
  // lazy `(.*?)` before `(?:[ \t]+#+)?[ \t]*$` backtracks quadratically over a long run of spaces.
  var HEADING_START = /^ {0,3}(#{1,6})(?=[ \t]|$)/;
  var RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
  var QUOTE = /^ {0,3}>[ \t]?(.*)$/;
  var ITEM = /^( {0,3})([-*+]|\d{1,9}[.)])(?:[ \t]+(.*))?$/;

  // Deeper than this, a quote or list inside a quote or list renders as text: a pathological
  // transcript can't make the renderer recurse without end.
  var MAX_DEPTH = 8;

  /** Whether `url` is one the page may open: an absolute `https:` address. */
  function safeUrl(url) {
    var u = String(url || '').trim();
    if (!/^https:\/\//i.test(u)) return null;
    if (/[\s<>"'`\\]/.test(u)) return null;
    return u;
  }

  function isSpace(c) {
    return c === ' ' || c === '\t';
  }

  /**
   * `{level, text}` for an ATX heading line, or null. The text is what follows the marker, its
   * spaces trimmed; a closing run of `#` goes only when whitespace parts it from at least one
   * character of text ("# foo ##" is "foo", "# #" and "# foo#" keep their `#`).
   */
  function headingOf(line) {
    var m = HEADING_START.exec(line);
    if (!m) return null;
    var rest = line.slice(m[0].length);
    var start = 0;
    while (start < rest.length && isSpace(rest.charAt(start))) start += 1;
    var end = rest.length;
    while (end > start && isSpace(rest.charAt(end - 1))) end -= 1;
    var hashes = end;
    while (hashes > start && rest.charAt(hashes - 1) === '#') hashes -= 1;
    if (hashes < end) {
      var gap = hashes;
      while (gap > start && isSpace(rest.charAt(gap - 1))) gap -= 1;
      if (gap < hashes && gap > start) end = gap;
    }
    return { level: m[1].length, text: rest.slice(start, end) };
  }

  /** The fence a line opens (its run of backticks or tildes), or null. */
  function fenceOf(line) {
    var m = FENCE_RUN.exec(line);
    return m && line.indexOf('`', m[0].length) < 0 ? m[1] : null;
  }

  // ---- inline ---------------------------------------------------------------------------

  function findClosing(text, from, delim) {
    var i = from;
    while (i < text.length) {
      var at = text.indexOf(delim, i);
      if (at < 0) return -1;
      if (text.charAt(at - 1) === '\\') {
        i = at + 1;
        continue;
      }
      if (at > from && !/\s/.test(text.charAt(at - 1))) return at;
      i = at + 1;
    }
    return -1;
  }

  /**
   * For every `[`, the index of its `]`, by one pass with a stack: what a per-`[` scan would
   * find, without rescanning a long run of unclosed brackets once per bracket.
   */
  function bracketPairs(text) {
    var pairs = {};
    var stack = [];
    for (var i = 0; i < text.length; i++) {
      var c = text.charAt(i);
      if (c === '\\') i += 1;
      else if (c === '[') stack.push(i);
      else if (c === ']' && stack.length) pairs[stack.pop()] = i;
    }
    return pairs;
  }

  function inline(text, depth) {
    var out = '';
    var plain = '';
    var i = 0;
    // A search for a closing delimiter that failed from `n` fails from every later start too:
    // remembering it keeps a long run of unclosed `*`, `_` or `~~` from being quadratic.
    var noCloser = {};
    var pairs = null;
    function flush() {
      if (plain) out += esc(plain);
      plain = '';
    }
    while (i < text.length) {
      var ch = text.charAt(i);
      var next = text.charAt(i + 1);
      if (ch === '\\' && next && /[\\`*_{}\[\]()#+\-.!~>|<]/.test(next)) {
        plain += next;
        i += 2;
        continue;
      }
      if (ch === '\\' && next === '\n') {
        flush();
        out += '<br>';
        i += 2;
        continue;
      }
      if (ch === '\n') {
        // Two spaces before a line end are a hard break; a bare line end is a space. Counted, not
        // matched: `/ {2,}$/` retries from every space of a long run that doesn't end the text.
        var spaces = 0;
        while (spaces < plain.length && plain.charAt(plain.length - 1 - spaces) === ' ') spaces += 1;
        var hard = spaces >= 2;
        if (hard) plain = plain.slice(0, plain.length - spaces);
        flush();
        out += hard ? '<br>' : ' ';
        i += 1;
        continue;
      }
      if (ch === '`') {
        var run = /^`+/.exec(text.slice(i))[0];
        var close = text.indexOf(run, i + run.length);
        while (close >= 0 && text.charAt(close + run.length) === '`') close = text.indexOf(run, close + run.length + 1);
        if (close > i) {
          flush();
          var code = text.slice(i + run.length, close).replace(/\n/g, ' ');
          if (/^ .* $/.test(code) && code.trim()) code = code.slice(1, -1);
          out += '<code class="an-md-code">' + esc(code) + '</code>';
          i = close + run.length;
          continue;
        }
        plain += run;
        i += run.length;
        continue;
      }
      if (ch === '<') {
        var auto = /^<(https?:\/\/[^\s<>]+)>/i.exec(text.slice(i));
        if (auto) {
          flush();
          out += link(esc(auto[1]), auto[1]);
          i += auto[0].length;
          continue;
        }
      }
      if (ch === '!' && next === '[' && depth < MAX_DEPTH) {
        // An image is not drawn: its description stands in for it, as text.
        pairs = pairs || bracketPairs(text);
        var image = parseLink(text, i + 1, pairs);
        if (image) {
          flush();
          out += inline(image.label, depth + 1);
          i = image.end;
          continue;
        }
      }
      if (ch === '[' && depth < MAX_DEPTH) {
        pairs = pairs || bracketPairs(text);
        var parsed = parseLink(text, i, pairs);
        if (parsed) {
          flush();
          out += link(inline(parsed.label, depth + 1), parsed.url);
          i = parsed.end;
          continue;
        }
      }
      if ((ch === '*' || ch === '_' || ch === '~') && depth < MAX_DEPTH) {
        var doubled = next === ch;
        var delim = doubled ? ch + ch : ch;
        if (ch === '~' && !doubled) {
          plain += ch;
          i += 1;
          continue;
        }
        var start = i + delim.length;
        // `snake_case` stays text: an underscore opens emphasis only at a word's edge.
        var intraword = ch === '_' && /[A-Za-z0-9]/.test(text.charAt(i - 1));
        if (!intraword && start < text.length && !/\s/.test(text.charAt(start)) &&
            !(noCloser[delim] !== undefined && start >= noCloser[delim])) {
          var end = findClosing(text, start, delim);
          if (end < 0) noCloser[delim] = start;
          if (end > start && !(ch === '_' && /[A-Za-z0-9]/.test(text.charAt(end + delim.length)))) {
            flush();
            var inner = inline(text.slice(start, end), depth + 1);
            if (ch === '~') out += '<s>' + inner + '</s>';
            else if (doubled) out += '<strong>' + inner + '</strong>';
            else out += '<em>' + inner + '</em>';
            i = end + delim.length;
            continue;
          }
        }
        plain += delim;
        i += delim.length;
        continue;
      }
      plain += ch;
      i += 1;
    }
    flush();
    return out;
  }

  // A link's address is looked for this far after its label: longer than any address the page
  // would open, and a bound on the work an unclosed `](` can cause.
  var MAX_TARGET = 2048;

  function parseLink(text, at, pairs) {
    var i = pairs[at];
    if (i === undefined || text.charAt(i + 1) !== '(') return null;
    // The address ends at the `)` that balances the `(`; a pair inside it is part of it.
    var depth = 1;
    var close = -1;
    var stop = Math.min(text.length, i + 2 + MAX_TARGET);
    for (var k = i + 2; k < stop; k++) {
      var c = text.charAt(k);
      if (c === '\\') k += 1;
      else if (c === '(') depth += 1;
      else if (c === ')' && --depth === 0) {
        close = k;
        break;
      }
    }
    if (close < 0) return null;
    var target = text.slice(i + 2, close).trim();
    var url = target.split(/\s+/)[0].replace(/^<|>$/g, '');
    return { label: text.slice(at + 1, i), url: url, end: close + 1 };
  }

  /** A link's text; clickable only for an `https:` address. */
  function link(labelHtml, url) {
    var safe = safeUrl(url);
    if (!safe) return labelHtml;
    return '<span class="an-md-link" role="link" tabindex="0" data-an-url="' + esc(safe) + '" title="' +
      esc(safe) + '">' + labelHtml + '</span>';
  }

  // ---- blocks ---------------------------------------------------------------------------

  function startsBlock(line) {
    return fenceOf(line) !== null || headingOf(line) !== null || RULE.test(line) || QUOTE.test(line) || ITEM.test(line);
  }

  function indentOf(line) {
    var m = /^[ \t]*/.exec(line)[0];
    return m.replace(/\t/g, '    ').length;
  }

  function parseBlocks(lines, depth) {
    var blocks = [];
    var i = 0;
    while (i < lines.length) {
      var line = lines[i];
      if (!line.trim()) {
        i += 1;
        continue;
      }
      var m;
      var fence = fenceOf(line);
      if (fence !== null) {
        var body = [];
        var closer = new RegExp('^ {0,3}' + fence.charAt(0) + '{' + fence.length + ',}\\s*$');
        i += 1;
        while (i < lines.length) {
          if (closer.test(lines[i])) {
            i += 1;
            break;
          }
          body.push(lines[i]);
          i += 1;
        }
        blocks.push({ type: 'code', text: body.join('\n') });
        continue;
      }
      var heading = headingOf(line);
      if (heading) {
        blocks.push({ type: 'heading', level: heading.level, text: heading.text });
        i += 1;
        continue;
      }
      if (RULE.test(line)) {
        blocks.push({ type: 'rule' });
        i += 1;
        continue;
      }
      if (QUOTE.test(line) && depth < MAX_DEPTH) {
        var quoted = [];
        while (i < lines.length && lines[i].trim() && (QUOTE.test(lines[i]) || !startsBlock(lines[i]))) {
          var q = QUOTE.exec(lines[i]);
          quoted.push(q ? q[1] : lines[i]);
          i += 1;
        }
        blocks.push({ type: 'quote', blocks: parseBlocks(quoted, depth + 1) });
        continue;
      }
      m = ITEM.exec(line);
      if (m && depth < MAX_DEPTH) {
        var ordered = /\d/.test(m[2]);
        var first = ordered ? parseInt(m[2], 10) : 1;
        var items = [];
        var baseIndent = m[1].length;
        while (i < lines.length) {
          var im = ITEM.exec(lines[i]);
          if (!im || /\d/.test(im[2]) !== ordered || im[1].length > baseIndent + 1) break;
          var content = [im[3] || ''];
          var contentIndent = im[1].length + im[2].length + 1;
          i += 1;
          while (i < lines.length) {
            var l = lines[i];
            if (!l.trim()) {
              // A blank line ends the item unless the next line is indented into it.
              if (i + 1 < lines.length && lines[i + 1].trim() && indentOf(lines[i + 1]) >= contentIndent) {
                content.push('');
                i += 1;
                continue;
              }
              break;
            }
            if (indentOf(l) >= contentIndent) {
              content.push(l.replace(new RegExp('^[ \\t]{0,' + contentIndent + '}'), ''));
              i += 1;
              continue;
            }
            if (ITEM.test(l) || startsBlock(l)) break;
            content.push(l.trim());
            i += 1;
          }
          items.push(parseBlocks(content, depth + 1));
          // A blank line between items keeps the list going.
          if (i < lines.length && !lines[i].trim() && i + 1 < lines.length) {
            var after = ITEM.exec(lines[i + 1]);
            if (after && /\d/.test(after[2]) === ordered && after[1].length <= baseIndent + 1) i += 1;
          }
        }
        blocks.push({ type: 'list', ordered: ordered, start: first, items: items });
        continue;
      }
      var para = [];
      while (i < lines.length && lines[i].trim() && (para.length === 0 || !startsBlock(lines[i]))) {
        para.push(lines[i].replace(/^[ \t]+/, ''));
        i += 1;
      }
      blocks.push({ type: 'paragraph', text: para.join('\n') });
    }
    return blocks;
  }

  function renderBlocks(blocks, depth) {
    return blocks.map(function (b) {
      switch (b.type) {
        case 'code':
          return '<pre class="an-md-pre"><code>' + esc(b.text) + '</code></pre>';
        case 'heading':
          return '<div class="an-md-h an-md-h' + b.level + '" role="heading" aria-level="' + b.level + '">' +
            inline(b.text, depth) + '</div>';
        case 'rule':
          return '<hr class="an-md-rule">';
        case 'quote':
          return '<blockquote class="an-md-quote">' + renderBlocks(b.blocks, depth + 1) + '</blockquote>';
        case 'list': {
          var tag = b.ordered ? 'ol' : 'ul';
          var items = b.items.map(function (item, index) {
            var marker = b.ordered ? (b.start + index) + '.' : '•';
            return '<li><span class="an-md-marker" aria-hidden="true">' + esc(marker) + '</span><div class="an-md-item">' +
              renderBlocks(item, depth + 1) + '</div></li>';
          }).join('');
          return '<' + tag + ' class="an-md-list">' + items + '</' + tag + '>';
        }
        default:
          return '<p class="an-md-p">' + inline(b.text, depth) + '</p>';
      }
    }).join('');
  }

  /** Markdown → escaped HTML inside `<div class="an-md">`. */
  function render(source, opts) {
    // U+2028 and U+2029 end a line too. Left in, they would stop every `.` of the block patterns
    // short of the line's end, and a long run of spaces before one made a heading or list
    // pattern backtrack for minutes.
    var text = String(source == null ? '' : source).replace(/\r\n?|[\u2028\u2029]/g, '\n');
    var cls = 'an-md' + (opts && opts.cls ? ' ' + opts.cls : '');
    return '<div class="' + cls + '">' + renderBlocks(parseBlocks(text.split('\n'), 0), 0) + '</div>';
  }

  window.agentnotchMarkdown = {
    render: render,
    inline: function (text) {
      return inline(String(text == null ? '' : text), 0);
    },
    safeUrl: safeUrl,
  };
})();
