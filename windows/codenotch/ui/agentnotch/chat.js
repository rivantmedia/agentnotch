// The chat screen of the sessions panel (DESIGN-WIN §5.3, UI§6; ChatView.swift, ChatSessionHeader.swift):
// the header (back, title, account, task summary and its board, context, show terminal), the
// transcript (user and assistant text, thinking, tool calls with their results, images, the
// working indicator) and a named slot for the bottom bar, which sub-task 8 fills.
//
// The contract with panel.js: on a `session:<id>` route it calls `mount(host, ctx)` once with the
// chat's own region (`#an-chat`, already shown) and `ctx = {sessionId, panel}` (`panel` is
// `window.agentnotchPanel`), `refresh()` after every render of the page (the row it reads may have
// changed) and `unmount()` when the route leaves the chat or changes session. `naturalHeight()` is
// what the chat wants of the window; `layoutProblems()` feeds the page's layoutReport.
//
// Data: `chat_open {session_id}` on entering asks the engine for the page; it answers with an
// `an:chat` reset, then patches. A reset replaces the page. A patch adds or replaces items by id,
// drops `removed`, and the `order` it carries is the page's order; a patch whose revision is not
// newer, or whose session is not the open chat, is ignored. `chat_close` on leaving.
//
// Everything a transcript holds is untrusted text: it reaches the page through
// agentnotchCommon.esc, agentnotchMarkdown.render and agentnotchToolResults.render only, never as
// markup. A link is text with `data-an-url`, opened by the glue (`open_url`) when it is https. An
// image is fetched only when the reader asks for it (`chat_image`) and is drawn only when it comes
// back as a `data:image/(png|jpeg|gif|webp);base64,` URL, the one image source the page's CSP lets
// through besides blob:.
//
// One IIFE, one global (agentnotchChat), no top-level let/const (a clash with a classic script
// kills the page).
(function () {
  'use strict';

  var C = window.agentnotchCommon;

  /** Items drawn per page; `has_earlier` counts what "Show earlier messages" would add. */
  var PAGE_SIZE = 150;
  var THINK_PREVIEW = 90;
  var BOARD_ROWS = 8;
  var BOARD_ROWS_COMPACT = 4;
  var BOARD_ROW_PX = 19;
  var DATA_URL_MAX = 3 * 1024 * 1024;
  var IMAGE_URL = /^data:image\/(?:png|jpeg|gif|webp);base64,[A-Za-z0-9+/]+={0,2}$/;
  var LIMITS = { name: 120, summary: 400, title: 300, label: 200, media: 60, working: 200 };
  var STATUSES = { running: 1, waiting_for_approval: 1, success: 1, error: 1, interrupted: 1 };

  /** The open chat, or null. */
  var S = null;
  var host = null;
  var els = {};
  var panel = null;
  var listening = null;
  var observer = null;

  /** What the bottom bar shows (sub-task 8): `(row, chat) => html`. Empty: no bar, no hairline. */
  var SLOTS = {
    bottom: function () { return ''; },
  };

  // ---- small helpers ---------------------------------------------------------------------

  function str(value) {
    return typeof value === 'string' ? value : '';
  }

  function clip(text, max) {
    var s = str(text);
    return s.length > max ? s.slice(0, max) : s;
  }

  function plural(n, one, many) {
    return n + ' ' + (n === 1 ? one : many);
  }

  function bytesLabel(n) {
    var b = Number(n);
    if (!isFinite(b) || b < 0) return '';
    if (b < 1024) return Math.floor(b) + ' B';
    if (b < 1024 * 1024) return Math.round(b / 1024) + ' KB';
    return (b / (1024 * 1024)).toFixed(1) + ' MB';
  }

  function keyOf(id) {
    return 'i:' + id;
  }

  function attr(text, max) {
    return C.esc(max ? clip(text, max) : text);
  }

  function logIt(message) {
    if (C && typeof C.log === 'function') C.log(message);
  }

  function fail(what) {
    return function (error) {
      var e = C.callError(error);
      logIt('chat: ' + what + ' failed (' + e.code + ')');
    };
  }

  function reportSize() {
    if (panel && typeof panel.reportSize === 'function') panel.reportSize();
  }

  /** The session's row as the panel shows it now; the last one seen when the session has left the list. */
  function rowNow() {
    if (!S) return null;
    var row = panel && typeof panel.row === 'function' ? panel.row(S.sessionId) : null;
    if (row) S.lastRow = row;
    return row || S.lastRow;
  }

  // ---- the page's state -------------------------------------------------------------------

  function fresh(sessionId) {
    return {
      sessionId: sessionId,
      revision: -1,
      items: Object.create(null),
      order: [],
      hasEarlier: 0,
      working: null,
      ended: false,
      loading: true,
      error: null,
      moreAsked: false,
      expanded: Object.create(null),
      images: Object.create(null),
      cache: Object.create(null),
      boardOpen: false,
      requestId: null,
      lastRow: null,
      stick: true,
      firstId: null,
    };
  }

  function validItem(item) {
    return !!item && typeof item === 'object' && typeof item.id === 'string' && item.id && typeof item.kind === 'string';
  }

  /**
   * An `an:chat` payload. True when it changed the page. A reset replaces everything (and is
   * dropped only when an older one arrives late); a patch needs a newer revision.
   */
  function apply(update) {
    if (!S || !update || typeof update !== 'object' || update.session_id !== S.sessionId) return false;
    var revision = Number(update.revision);
    if (!isFinite(revision)) return false;
    if (update.reset) {
      if (S.revision >= 0 && revision < S.revision) return false;
      S.items = Object.create(null);
      S.order = [];
      S.cache = Object.create(null);
    } else if (!(revision > S.revision)) {
      return false;
    }
    S.revision = revision;
    (Array.isArray(update.items) ? update.items : []).forEach(function (item) {
      if (validItem(item)) S.items[item.id] = item;
    });
    (Array.isArray(update.removed) ? update.removed : []).forEach(function (id) {
      if (typeof id !== 'string') return;
      delete S.items[id];
      delete S.cache[id];
    });
    if (Array.isArray(update.order)) {
      var seen = Object.create(null);
      S.order = update.order.filter(function (id) {
        if (typeof id !== 'string' || seen[id] || !S.items[id]) return false;
        seen[id] = true;
        return true;
      });
    } else {
      var have = Object.create(null);
      S.order = S.order.filter(function (id) {
        have[id] = true;
        return !!S.items[id];
      });
      Object.keys(S.items).forEach(function (id) {
        if (!have[id]) S.order.push(id);
      });
    }
    S.hasEarlier = Math.max(0, Math.floor(Number(update.has_earlier) || 0));
    S.working = str(update.working) ? clip(update.working, LIMITS.working) : null;
    S.ended = !!update.ended;
    S.loading = !!update.loading;
    S.error = null;
    S.moreAsked = false;
    Object.keys(S.expanded).forEach(function (id) { if (!S.items[id]) delete S.expanded[id]; });
    Object.keys(S.images).forEach(function (id) { if (!S.items[id]) delete S.images[id]; });
    draw(true);
    return true;
  }

  // ---- the transcript's items -------------------------------------------------------------

  function imageState(id) {
    var image = S.images[id];
    return image ? image.state : '';
  }

  function markdown(text) {
    var md = window.agentnotchMarkdown;
    return md ? md.render(text, { cls: 'an-md-chat' }) : '<div class="an-md an-md-chat">' + C.esc(text) + '</div>';
  }

  function userHtml(item) {
    return '<div class="an-ci an-ci-user" data-key="' + attr(keyOf(item.id)) + '"><div class="an-bubble">' + markdown(str(item.text)) + '</div></div>';
  }

  function assistantHtml(item) {
    var text = str(item.text);
    // A tool-only turn carries no text: draw nothing rather than an empty line.
    if (!text.trim()) return '';
    return '<div class="an-ci an-ci-assistant" data-key="' + attr(keyOf(item.id)) + '">' + markdown(text) + '</div>';
  }

  function chevron(open) {
    return '<span class="an-chev' + (open ? ' an-chev-open' : '') + '">' + C.icon('chevronRight') + '</span>';
  }

  function thinkingHtml(item) {
    var text = str(item.text);
    if (!text.trim()) return '';
    var can = text.length > THINK_PREVIEW;
    var open = can && !!S.expanded[item.id];
    var shown = open || !can ? text : text.slice(0, THINK_PREVIEW) + '…';
    var label = 'Thinking: ' + clip(text, 400);
    var inner = '<span class="an-think-t' + (open ? ' an-think-open' : '') + '">' + C.esc(shown) + '</span>' + (can ? chevron(open) : '');
    return '<div class="an-ci an-ci-thinking" data-key="' + attr(keyOf(item.id)) + '">' +
      (can
        ? '<button type="button" class="an-think" data-an-chat="toggle" data-id="' + attr(item.id) + '" aria-expanded="' + open + '" aria-label="' + attr(label) + '">' + inner + '</button>'
        : '<div class="an-think" aria-label="' + attr(label) + '">' + inner + '</div>') +
      '</div>';
  }

  function toolNameOf(name) {
    var results = window.agentnotchToolResults;
    return clip(results ? results.toolName(name) : String(name == null ? '' : name), LIMITS.name);
  }

  function subToolHtml(tool) {
    if (!tool || typeof tool !== 'object') return '';
    var status = STATUSES[tool.status] ? tool.status : 'success';
    var text = status === 'interrupted' ? 'Interrupted' : clip(C.oneLine(tool.summary), LIMITS.summary);
    return '<div class="an-subtool" data-key="' + attr('s:' + str(tool.id)) + '">' + C.toolMark(status) +
      '<span class="an-subtool-name" data-an-text data-an-clip>' + C.esc(toolNameOf(tool.name)) + '</span>' +
      '<span class="an-subtool-text" data-an-text data-an-clip>' + C.esc(text) + '</span></div>';
  }

  /** The last two tools a subagent ran, and how many before them (SubagentToolsList). */
  function subagentHtml(tools) {
    var out = '';
    if (tools.length > 2) out += '<div class="an-subtool-more">' + C.esc('+' + (tools.length - 2) + ' earlier tool uses') + '</div>';
    out += tools.slice(-2).map(subToolHtml).join('');
    return '<div class="an-subtools">' + out + '</div>';
  }

  /** The quiet text after a tool's name (ToolCallSummary): a subagent says what it is doing and how many tools it ran. */
  function summaryOf(item, sub, subTools) {
    if (sub && subTools.length) {
      var input = item.input && typeof item.input === 'object' ? item.input : {};
      var description = str(sub.description).trim() || str(input.description).trim() || 'Running an agent';
      return description + ' · ' + plural(subTools.length, 'tool', 'tools');
    }
    return str(item.summary);
  }

  function toolHtml(item) {
    var results = window.agentnotchToolResults;
    var status = STATUSES[item.status] ? item.status : 'success';
    var name = item.name == null ? '' : String(item.name);
    var sub = item.subagent && typeof item.subagent === 'object' ? item.subagent : null;
    var subTools = sub && Array.isArray(sub.tools) ? sub.tools : [];
    var hasResult = item.result != null && typeof item.result === 'object';
    var isEdit = name === 'Edit';
    var can = !sub && !isEdit && hasResult;
    var active = status === 'running' || status === 'waiting_for_approval';
    var open = can && !!S.expanded[item.id];
    var shownName = toolNameOf(name);
    var summary = clip(C.oneLine(summaryOf(item, sub, subTools)), LIMITS.summary);
    var head = C.toolMark(status) +
      '<span class="an-tool-name' + (status === 'error' ? ' an-critical' : '') + '" data-an-text data-an-clip>' + C.esc(shownName) + '</span>' +
      '<span class="an-tool-sum" data-an-text data-an-clip>' + C.esc(summary) + '</span>' +
      (can && !active ? chevron(open) : '');
    var label = shownName + (summary ? ', ' + summary : '');
    var headHtml = can
      ? '<button type="button" class="an-tool-head" data-an-chat="toggle" data-id="' + attr(item.id) + '" aria-expanded="' + open + '" aria-label="' + attr(label) + '">' + head + '</button>'
      : '<div class="an-tool-head" role="group" aria-label="' + attr(label) + '">' + head + '</div>';
    var body = '';
    if (sub && subTools.length) body += subagentHtml(subTools);
    // Edit shows its change without being asked (from its input while it is still running); the
    // others show their result on a click, and never while they run.
    if (!sub && (isEdit || open) && (status !== 'running' || isEdit) && results) {
      var view = results.render(item);
      if (view) body += '<div class="an-tool-result">' + view + '</div>';
    }
    return '<div class="an-ci an-titem" data-key="' + attr(keyOf(item.id)) + '" data-status="' + status + '">' + headHtml + body + '</div>';
  }

  function imageHtml(item) {
    var media = clip(C.oneLine(item.media_type), LIMITS.media);
    var label = 'Image' + (media ? ' (' + media + ')' : '');
    var image = S.images[item.id];
    var inner;
    if (image && image.state === 'ready') {
      inner = '<img class="an-img" src="' + C.esc(image.url) + '" alt="' + attr(label) + '">';
    } else {
      var loading = !!image && image.state === 'loading';
      var note = loading ? 'Loading…' : image && image.state === 'failed' ? 'Couldn’t load it. Try again' : bytesLabel(item.bytes);
      inner = '<button type="button" class="an-img-ph" data-an-chat="image" data-id="' + attr(item.id) + '" title="Show the image"' + (loading ? ' disabled' : '') + '>' +
        C.icon('photo') + '<span class="an-img-l">' + C.esc(label) + '</span>' + (note ? '<span class="an-img-n">' + C.esc(note) + '</span>' : '') + '</button>';
    }
    return '<div class="an-ci an-ci-image" data-key="' + attr(keyOf(item.id)) + '">' + inner + '</div>';
  }

  function interruptedHtml(item) {
    return '<div class="an-ci an-ci-interrupted" data-key="' + attr(keyOf(item.id)) + '">Interrupted</div>';
  }

  var KINDS = {
    user: userHtml,
    assistant: assistantHtml,
    thinking: thinkingHtml,
    tool: toolHtml,
    image: imageHtml,
    interrupted: interruptedHtml,
  };

  /** One item's markup, cached until the item, its expansion or its image changes. */
  function itemHtml(id) {
    var item = S.items[id];
    if (!item) return '';
    var make = Object.prototype.hasOwnProperty.call(KINDS, item.kind) ? KINDS[item.kind] : null;
    if (!make) return '';
    var sig = JSON.stringify(item) + '|' + (S.expanded[id] ? 1 : 0) + '|' + imageState(id);
    var cached = S.cache[id];
    if (cached && cached.sig === sig) return cached.html;
    var html = '';
    try {
      html = make(item);
    } catch (e) {
      logIt('chat: could not draw an item');
    }
    S.cache[id] = { sig: sig, html: html };
    return html;
  }

  function placeholderHtml(kind, text) {
    return '<div class="an-ph" data-key="ph-' + kind + '">' +
      (kind === 'loading' ? C.statusRing('working', { cls: 'an-ph-spin' }) : '') + '<span>' + C.esc(text) + '</span></div>';
  }

  function workingHtml() {
    return '<div class="an-working" data-key="working" role="status">' + C.statusRing('working') +
      '<span class="an-working-t" data-an-text data-an-clip>' + C.esc(S.working) + '</span></div>';
  }

  function listHtml() {
    if (S.loading) return placeholderHtml('loading', 'Loading the conversation…');
    var showWorking = !!S.working && !S.ended;
    if (!S.order.length && !showWorking) return placeholderHtml('empty', S.error || 'No messages yet');
    var out = '';
    if (S.hasEarlier > 0) {
      var n = Math.min(S.hasEarlier, PAGE_SIZE);
      out += '<button type="button" class="an-btn an-btn-quiet an-compact an-earlier" data-key="earlier" data-an-chat="earlier"' + (S.moreAsked ? ' disabled' : '') + '>' +
        C.esc('Show ' + plural(n, 'earlier message', 'earlier messages')) + '</button>';
    }
    S.order.forEach(function (id) { out += itemHtml(id); });
    if (showWorking) out += workingHtml();
    return out;
  }

  // ---- the header --------------------------------------------------------------------------

  function tasksOf(row) {
    var t = row && row.tasks;
    return t && Math.floor(Number(t.total)) > 0 ? t : null;
  }

  function boardMark(status) {
    if (status === 'completed') {
      return '<svg class="an-tmark an-tmark-done" viewBox="0 0 10 10" aria-hidden="true" focusable="false"><circle cx="5" cy="5" r="4.1" fill="none" stroke="currentColor" stroke-width="1.3"/><circle cx="5" cy="5" r="2.4" fill="currentColor"/></svg>';
    }
    if (status === 'in_progress') return C.statusRing('working', { stroke: 1.5, cls: 'an-tmark an-tmark-active' });
    return C.statusRing('idle', { stroke: 1.3, cls: 'an-tmark an-tmark-todo' });
  }

  /** Every task with its state: done (struck through), in progress (turning), to do. */
  function boardHtml(tasks, compact) {
    var items = Array.isArray(tasks.items) ? tasks.items : [];
    var total = Math.floor(Number(tasks.total)) || items.length;
    var done = Math.min(Math.max(Math.floor(Number(tasks.done) || 0), 0), total);
    var rows = items.map(function (item, index) {
      var status = item && (item.status === 'completed' || item.status === 'in_progress') ? item.status : 'pending';
      var label = clip(C.oneLine(item && item.label), LIMITS.label);
      return '<div class="an-trow an-trow-' + status + '" data-key="t' + index + '">' + boardMark(status) +
        '<span class="an-ttext" data-an-text data-an-clip>' + C.esc(label) + '</span></div>';
    }).join('');
    var max = (compact ? BOARD_ROWS_COMPACT : BOARD_ROWS) * BOARD_ROW_PX;
    return '<div class="an-board" data-key="board"><div class="an-board-head"><span>Tasks</span><span class="an-board-count">' +
      done + ' of ' + total + ' done</span></div><div class="an-board-list" style="max-height:' + max + 'px">' + rows + '</div></div>';
  }

  /** The task Claude is on while it works, else the project (and background tasks). */
  function subtitleOf(row) {
    var working = row.bucket === 'working' && !row.failed;
    var tasks = tasksOf(row);
    if (working && tasks && str(tasks.active_label)) return { text: C.oneLine(tasks.active_label), activity: true };
    var parts = [];
    if (str(row.project)) parts.push(C.oneLine(row.project));
    var bg = Math.floor(Number(row.background_count) || 0);
    if (bg > 0) parts.push(bg + ' background');
    return { text: parts.join(' · '), activity: false };
  }

  function headerHtml() {
    var row = rowNow();
    var list = window.agentnotchPanelList;
    var title = row ? clip(C.oneLine(row.title), LIMITS.title) || 'Session' : 'Session';
    var multi = !!(panel && typeof panel.multiAccounts === 'function' && panel.multiAccounts());
    var sub = row ? subtitleOf(row) : { text: '', activity: false };
    var account = row && multi && list && typeof list.accountItem === 'function' ? list.accountItem(row) : '';
    var tasks = tasksOf(row);
    var right = '';
    if (tasks && list && typeof list.tasksItem === 'function') {
      var active = str(tasks.active_label);
      var tip = active ? 'Now: ' + C.oneLine(active) : 'Tasks';
      right += '<button type="button" class="an-ch-tasks' + (S.boardOpen ? ' an-on' : '') + '" data-an-chat="toggle-tasks" aria-expanded="' + S.boardOpen +
        '" title="' + attr(tip, 200) + '">' + list.tasksItem(tasks, 30, true) + '<span class="an-ch-chev' + (S.boardOpen ? ' an-on' : '') + '">' + C.icon('chevronDown') + '</span></button>';
    }
    if (row && row.context_pct != null && list && typeof list.contextItem === 'function') right += list.contextItem(row.context_pct, true);
    var focus = row && str(row.focus_label) ? clip(C.oneLine(row.focus_label), 40) : '';
    if (focus) {
      right += '<button type="button" class="an-icbtn an-ch-focus" data-an-action="jump" data-an-arg="' + attr(S.sessionId) + '" aria-label="' + attr(focus + ' (Ctrl+J)') +
        '" title="' + attr(focus + ' (Ctrl+J)') + '">' + C.icon('openApp') + '</button>';
    }
    return '<div class="an-ch-row">' +
      '<button type="button" class="an-icbtn an-ch-back" data-an-action="back" aria-label="Back to sessions (Esc)" title="Back to sessions (Esc)">' + C.icon('chevronLeft') + '</button>' +
      '<div class="an-ch-titles"><h2 class="an-ch-title" data-an-text data-an-clip>' + C.esc(title) + '</h2>' +
      '<div class="an-ch-sub">' + account + (account && sub.text ? '<span class="an-dotsep">·</span>' : '') +
      '<span class="an-ch-subt' + (sub.activity ? ' an-ch-act' : '') + '" data-an-text data-an-clip>' + C.esc(sub.text) + '</span></div></div>' +
      '<div class="an-ch-right">' + right + '</div></div>' +
      (S.boardOpen && tasks ? boardHtml(tasks, !!(row && row.pending)) : '');
  }

  // ---- drawing -----------------------------------------------------------------------------

  function atBottom(el) {
    return el.scrollTop + el.clientHeight >= el.scrollHeight - 4;
  }

  /**
   * Draws the transcript. `follow`: the content changed under the reader (a patch): the view stays
   * on the newest message when it was already there, and stays put, with the same message under the
   * eye, when earlier messages arrive above. A toggle or an image keeps the position.
   */
  function draw(follow) {
    if (!S || !els.scroll) return;
    var scroll = els.scroll;
    var before = scroll.scrollHeight;
    var top = scroll.scrollTop;
    var stick = S.stick;
    var first = S.order.length ? S.order[0] : null;
    var prepended = follow && S.firstId !== null && first !== S.firstId && S.order.indexOf(S.firstId) > 0;
    C.morph(els.list, listHtml());
    if (follow && prepended && !stick) scroll.scrollTop = top + (scroll.scrollHeight - before);
    else if (follow && stick) scroll.scrollTop = scroll.scrollHeight;
    S.firstId = first;
    drawBar();
    reportSize();
  }

  function drawHeader() {
    if (S && els.head) C.morph(els.head, headerHtml());
  }

  /** The bottom bar's slot (sub-task 8 fills it); an empty bar takes no room and no hairline. */
  function drawBar() {
    if (!S || !els.bar) return;
    var html = '';
    try {
      html = SLOTS.bottom(rowNow(), api) || '';
    } catch (e) {
      logIt('chat: could not draw the bottom bar');
    }
    C.morph(els.bar, html);
    els.foot.classList.toggle('an-chat-foot-empty', !html);
  }

  /** What the page reads from the row changed (a snapshot, a request arming): redraw the parts that follow it. */
  function refresh() {
    if (!S) return;
    var row = rowNow();
    var id = row && row.pending ? str(row.pending.tool_use_id) : null;
    // The board closes when a request appears: a question or plan needs its room.
    if (id && id !== S.requestId) S.boardOpen = false;
    S.requestId = id;
    drawHeader();
    drawBar();
    reportSize();
  }

  // ---- what the reader does ----------------------------------------------------------------

  function openUrl(url) {
    var u = str(url);
    if (!/^https:\/\/[^\s]+$/i.test(u)) return;
    C.call('open_url', { url: u }).catch(fail('open_url'));
  }

  /** Fetches one image (`chat_image`) when the reader asks; what comes back must be an image data URL. */
  function loadImage(id) {
    if (!S || !S.items[id] || S.items[id].kind !== 'image') return;
    var current = S.images[id];
    if (current && (current.state === 'loading' || current.state === 'ready')) return;
    var mine = S;
    S.images[id] = { state: 'loading', url: '' };
    draw(false);
    C.call('chat_image', { session_id: S.sessionId, image_id: id }).then(function (reply) {
      if (S !== mine || !S.images[id]) return;
      var url = reply && typeof reply.data_url === 'string' ? reply.data_url : '';
      S.images[id] = url.length <= DATA_URL_MAX && IMAGE_URL.test(url) ? { state: 'ready', url: url } : { state: 'failed', url: '' };
      draw(false);
    }, function (error) {
      fail('chat_image')(error);
      if (S !== mine || !S.images[id]) return;
      S.images[id] = { state: 'failed', url: '' };
      draw(false);
    });
  }

  function askEarlier() {
    if (!S || S.moreAsked || !S.order.length) return;
    S.moreAsked = true;
    draw(false);
    C.call('chat_more', { session_id: S.sessionId, before_id: S.order[0] }).catch(fail('chat_more'));
  }

  function toggle(id) {
    if (!S || !S.items[id]) return;
    S.expanded[id] = !S.expanded[id];
    draw(false);
  }

  function setBoard(on) {
    if (!S) return;
    S.boardOpen = !!on;
    drawHeader();
    reportSize();
  }

  function onClick(event) {
    var t = event.target;
    if (!S || !t || typeof t.closest !== 'function') return;
    var link = t.closest('[data-an-url]');
    if (link) {
      event.preventDefault();
      openUrl(link.getAttribute('data-an-url'));
      return;
    }
    var control = t.closest('[data-an-chat]');
    if (!control || control.disabled || !host.contains(control)) return;
    var id = control.getAttribute('data-id');
    switch (control.getAttribute('data-an-chat')) {
      case 'toggle': toggle(id); break;
      case 'toggle-tasks': setBoard(!S.boardOpen); break;
      case 'image': loadImage(id); break;
      case 'earlier': askEarlier(); break;
      default: break;
    }
  }

  /** A link is a span, not a button: Enter opens it, but only once the keyboard gate is open. */
  function onKeyDown(event) {
    var t = event.target;
    if (event.key !== 'Enter' || !t || typeof t.closest !== 'function') return;
    var link = t.closest('[data-an-url]');
    if (!link) return;
    event.stopPropagation();
    if (panel && typeof panel.keyboardOpen === 'function' && !panel.keyboardOpen()) return;
    event.preventDefault();
    openUrl(link.getAttribute('data-an-url'));
  }

  function onScroll() {
    if (S && els.scroll) S.stick = atBottom(els.scroll);
  }

  // ---- mounting ----------------------------------------------------------------------------

  function mount(region, ctx) {
    if (S) unmount();
    var id = ctx && typeof ctx.sessionId === 'string' ? ctx.sessionId : '';
    if (!id || !region) return;
    host = region;
    panel = (ctx && ctx.panel) || window.agentnotchPanel || null;
    S = fresh(id);
    host.innerHTML =
      '<div class="an-chat-head" id="an-chat-head"></div>' +
      '<div class="an-chat-hair"></div>' +
      '<div class="an-chat-scroll" id="an-chat-scroll" role="log" aria-label="Conversation"><div class="an-chat-list" id="an-chat-list"></div></div>' +
      '<div class="an-chat-foot an-chat-foot-empty" id="an-chat-foot"><div class="an-chat-hair"></div><div class="an-chat-bar" id="an-chat-bar" data-an-slot="bottom"></div></div>';
    els = {
      head: host.querySelector('#an-chat-head'),
      scroll: host.querySelector('#an-chat-scroll'),
      list: host.querySelector('#an-chat-list'),
      foot: host.querySelector('#an-chat-foot'),
      bar: host.querySelector('#an-chat-bar'),
    };
    host.addEventListener('click', onClick);
    host.addEventListener('keydown', onKeyDown);
    els.scroll.addEventListener('scroll', onScroll);
    if (typeof window.ResizeObserver === 'function') {
      observer = new window.ResizeObserver(reportSize);
      observer.observe(els.list);
      observer.observe(els.head);
      observer.observe(els.foot);
    }
    drawHeader();
    draw(false);
    var mine = S;
    // The listener first, then the question: the engine's reset must not beat it.
    if (!listening) listening = C.listen('an:chat', apply);
    Promise.resolve(listening).then(function () {
      if (S !== mine) return;
      C.call('chat_open', { session_id: id }).catch(function (error) {
        fail('chat_open')(error);
        if (S !== mine) return;
        S.loading = false;
        S.error = 'The conversation couldn’t be loaded.';
        draw(false);
      });
    });
  }

  function unmount() {
    if (!S) return;
    var id = S.sessionId;
    if (host) {
      host.removeEventListener('click', onClick);
      host.removeEventListener('keydown', onKeyDown);
      host.innerHTML = '';
    }
    if (observer) observer.disconnect();
    observer = null;
    S = null;
    els = {};
    C.call('chat_close', { session_id: id }).catch(fail('chat_close'));
  }

  /** The window's ask: header, the transcript's content and the bottom bar (the hairlines are 1 px each). */
  function naturalHeight() {
    if (!S || !els.head) return 0;
    var foot = els.foot.classList.contains('an-chat-foot-empty') ? 0 : els.foot.offsetHeight;
    return els.head.offsetHeight + 1 + els.list.offsetHeight + foot;
  }

  /** What the snapshot tool and the self-test measure: nothing may be wider than the transcript (code scrolls in its own box). */
  function layoutProblems() {
    var problems = [];
    if (!S || !els.scroll) return problems;
    var box = els.scroll.getBoundingClientRect();
    if (!box.width) return problems;
    if (els.scroll.scrollWidth > els.scroll.clientWidth + 1) problems.push('the transcript is wider than the card');
    Array.prototype.forEach.call(els.list.querySelectorAll('*'), function (el) {
      if (problems.length >= 5 || (el.closest && el.closest('.an-code-body'))) return;
      var r = el.getBoundingClientRect();
      if (r.width && r.right > box.right + 1) problems.push('wider than the card: ' + C.oneLine(el.textContent).slice(0, 40));
    });
    return problems;
  }

  var api = {
    mount: mount,
    unmount: unmount,
    refresh: refresh,
    apply: apply,
    naturalHeight: naturalHeight,
    layoutProblems: layoutProblems,
    /** Snapshots: the task board open or shut. */
    openBoard: setBoard,
    /** The bottom bar is sub-task 8's: assign `slots.bottom = (row, chat) => html`. */
    slots: SLOTS,
    /** The open chat's state for the bar and the tests (a copy; `null` when no chat is open). */
    current: function () {
      return S ? {
        sessionId: S.sessionId,
        revision: S.revision,
        order: S.order.slice(),
        hasEarlier: S.hasEarlier,
        working: S.working,
        ended: S.ended,
        loading: S.loading,
        boardOpen: S.boardOpen,
      } : null;
    },
    PAGE_SIZE: PAGE_SIZE,
  };

  window.agentnotchChat = api;
})();
