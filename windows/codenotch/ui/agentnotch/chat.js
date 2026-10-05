// The chat screen of the sessions panel (DESIGN-WIN §5.3, UI§6; ChatView.swift, ChatSessionHeader.swift):
// the header (back, title, account, task summary and its board, context, show terminal), the
// transcript (user and assistant text, thinking, tool calls with their results, images, the
// working indicator) and the one bottom bar (a request's answers, a terminal-only note, or the
// composer with its per-session drafts).
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

  /** What the bottom bar shows: `(row, chat) => html` (barHtml below). Empty: no bar, no hairline. */
  var SLOTS = {
    bottom: function () { return ''; },
  };

  /** Replies not sent yet, per session: kept in memory, and in localStorage when it works. */
  var DRAFT_KEY = 'agentnotch.chat.drafts';
  var DRAFT_MAX = 20000;
  var DRAFT_SESSIONS = 40;
  var drafts = Object.create(null);
  var draftsLoaded = false;
  var focusListening = null;

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
      // the bottom bar
      route: { state: 'asking', reason: '' },
      routeSeq: 0,
      canMessage: null,
      sending: false,
      failure: null,
      forms: Object.create(null),
      wantFocus: null,
      composerEl: null,
      sceneDraft: null,
      scene: false,
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

  // ---- the bottom bar (ChatView.bottomBar, ChatApprovalBars, ChatQuestionPanel; UI§6.3) -----
  //
  // Exactly one bar, by precedence: a pending request (permission, question, plan), a dialog
  // only the terminal can answer, no way to type a reply, the composer. A request's bar answers
  // only through agentnotchPanel's AnswerGate (inert for 0.35 s after it appears, one answer
  // per tool_use_id). Every field obeys the keyboard gate: read-only and "Click to type" until
  // the glue confirms the panel has the keyboard, so a key meant for a terminal never lands
  // here and nothing typed here answers anything while the gate is shut.

  var COPY = {
    routeSentence: 'Replies can be typed from here for sessions in Windows Terminal, VS Code’s terminal and console windows.',
    dialog: 'Answer it in the terminal. Anything typed here would go straight into that dialog.',
    unshownQuestion: 'It can’t be shown here. Answer it in the terminal.',
    clickToType: 'Click to type',
    reply: 'Reply to Claude',
    other: 'Type your answer',
    planNote: 'Approving lets Claude start on it.',
    sent: 'Sent to Claude',
    showTerminalTip: 'Bring the session’s terminal to the front (Ctrl+J)',
  };
  // The request and the plan are drawn whole up to C.SHOWN_WHOLE (see cutNote), never cut silently.
  var LIMITS_BAR = { reason: 300, question: 1000, option: 300, header: 60, always: 400, failure: 600 };

  function panelCall(name) {
    var args = Array.prototype.slice.call(arguments, 1);
    return panel && typeof panel[name] === 'function' ? panel[name].apply(panel, args) : undefined;
  }

  /** The keyboard gate: true only after the glue's `an:panel_focus {focused:true}`. */
  function keyboardOpen() {
    return !!panelCall('keyboardOpen');
  }

  function isArmed(id) {
    return !!panelCall('isArmed', id);
  }

  /** "reason." or "reason" → "reason": the copy puts its own full stop after it. */
  function sentence(text, max) {
    return clip(C.oneLine(text), max).replace(/[.\s]+$/, '');
  }

  // -- drafts: memory first, then localStorage (which may be missing or throw: sealed, private) --

  function storageOf() {
    try {
      return window.localStorage || null;
    } catch (e) {
      return null;
    }
  }

  function loadDrafts() {
    if (draftsLoaded) return;
    draftsLoaded = true;
    try {
      var store = storageOf();
      var raw = store ? store.getItem(DRAFT_KEY) : null;
      var saved = raw ? JSON.parse(raw) : null;
      if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return;
      Object.keys(saved).forEach(function (id) {
        if (typeof saved[id] === 'string' && saved[id] && !(id in drafts)) drafts[id] = saved[id].slice(0, DRAFT_MAX);
      });
    } catch (e) {
      // Unreadable or not ours: the drafts in memory are all there is.
    }
  }

  function saveDrafts() {
    try {
      var store = storageOf();
      if (!store) return;
      var ids = Object.keys(drafts).filter(function (id) { return drafts[id]; });
      var out = {};
      ids.slice(-DRAFT_SESSIONS).forEach(function (id) {
        Object.defineProperty(out, id, { value: drafts[id], enumerable: true, writable: true, configurable: true });
      });
      if (ids.length) store.setItem(DRAFT_KEY, JSON.stringify(out));
      else store.removeItem(DRAFT_KEY);
    } catch (e) {
      // Full, blocked or gone: the draft stays in memory for this run.
    }
  }

  function draftOf(id) {
    if (S && S.sceneDraft !== null && S.sessionId === id) return S.sceneDraft;
    loadDrafts();
    return typeof drafts[id] === 'string' ? drafts[id] : '';
  }

  function setDraft(id, text) {
    if (S && S.sceneDraft !== null) return;
    loadDrafts();
    // Re-added last, so the newest drafts are the ones storage keeps.
    delete drafts[id];
    var value = String(text == null ? '' : text).slice(0, DRAFT_MAX);
    if (value) drafts[id] = value;
    saveDrafts();
  }

  // -- the route: whether a reply typed here can reach the session's terminal --

  function askRoute() {
    if (!S) return;
    var mine = S;
    var seq = ++S.routeSeq;
    C.call('message_route', { session_id: S.sessionId }).then(function (reply) {
      if (S !== mine || seq !== S.routeSeq) return;
      S.route = reply && reply.available === true
        ? { state: 'available', reason: '' }
        : { state: 'unavailable', reason: reply && typeof reply.reason === 'string' ? sentence(reply.reason, LIMITS_BAR.reason) : '' };
      drawBar();
      reportSize();
    }, function (error) {
      fail('message_route')(error);
      if (S !== mine || seq !== S.routeSeq) return;
      S.route = { state: 'unavailable', reason: '' };
      drawBar();
      reportSize();
    });
  }

  // -- pieces --------------------------------------------------------------------------------

  function barTitle(text) {
    return '<div class="an-bbar-title" data-key="title" role="heading" aria-level="3">' + C.statusRing('needs', { cls: 'an-bbar-mark' }) +
      '<span class="an-bbar-title-t" data-an-text data-an-clip>' + C.esc(text) + '</span></div>';
  }

  function jumpButton(row, primary) {
    var label = row && str(row.focus_label) ? clip(C.oneLine(row.focus_label), 40) : '';
    if (!label) return '';
    return '<button type="button" class="an-btn an-btn-' + (primary ? 'primary' : 'secondary') + ' an-bbar-jump" data-key="jump" data-an-action="jump" data-an-arg="' +
      attr(row.session_id) + '" title="' + attr(COPY.showTerminalTip) + '"><span data-an-text>' + C.esc(label) + '</span></button>';
  }

  /** An answering button: it names the request it was drawn for, and is inert until that request is armed. */
  function answerButton(kind, label, title, sessionId, toolUseId, arg, armed) {
    return '<button type="button" class="an-btn an-btn-' + kind + (armed ? '' : ' an-unarmed') + '" data-key="ans-' + attr(arg) +
      '" data-an-action="answer" data-an-arg="' + attr(arg) + '" data-an-session="' + attr(sessionId) + '" data-an-tool="' + attr(toolUseId) +
      '" data-an-noenter title="' + attr(title) + '"' + (armed ? '' : ' aria-disabled="true"') + '><span data-an-text>' + C.esc(label) + '</span></button>';
  }

  function bar(kind, key, armed, inner) {
    return '<div class="an-bbar an-bbar-' + kind + (armed === false ? ' an-bbar-unarmed' : '') + '" data-key="' + attr(key) + '"' +
      (armed === false ? ' aria-busy="true"' : '') + '>' + inner + '</div>';
  }

  function terminalOnlyBar(row, title, message) {
    return bar('term', 'bar-term', null,
      (title ? barTitle(title) : '') +
      '<div class="an-bbar-line"><span class="an-bbar-msg" data-an-text>' + C.esc(message) + '</span>' + jumpButton(row, !!title) + '</div>');
  }

  /**
   * What a request or plan longer than the chat draws left out, said outside its scrolling box so
   * it shows without scrolling. An answer approves the whole text, so such a bar offers no
   * approval at all (C.primaryActions' `cut`): the terminal shows it all.
   */
  function cutNote(left) {
    return '<div class="an-bbar-note an-bbar-cut" data-key="cut" role="status" data-an-text>' +
      C.esc('… ' + plural(left, 'more character', 'more characters') + ' not shown. Open the terminal to read it all.') + '</div>';
  }

  // -- a permission ---------------------------------------------------------------------------

  function approvalBar(row, p, armed) {
    var results = window.agentnotchToolResults;
    var tool = toolNameOf(p.tool_name);
    var left = C.cutOff(p.request, C.SHOWN_WHOLE.request);
    var request = clip(str(p.request), C.SHOWN_WHOLE.request);
    // No Always caption on a cut request: nothing here could save the rule.
    var always = !left && p.always != null && str(p.always) ? sentence(p.always, LIMITS_BAR.always) : '';
    var diff = Array.isArray(p.diff) && p.diff.length && results && typeof results.diffView === 'function' ? results.diffView(p.diff, null) : '';
    var sid = row.session_id;
    var id = p.tool_use_id;
    return bar('perm', 'bar-perm-' + id, armed,
      barTitle((tool || 'A tool') + ' needs your permission') +
      (request.trim() ? '<div class="an-bbar-box an-bbar-req" data-key="req" tabindex="0" aria-label="The request"><div class="an-bbar-req-t">' + C.esc(request) + '</div></div>' : '') +
      (left ? cutNote(left) : '') +
      (diff ? '<div class="an-bbar-diff" data-key="diff">' + diff + '</div>' : '') +
      (always ? '<div class="an-bbar-note an-bbar-always" data-key="always" data-an-text>' + C.esc('Always allow: ' + always + '.') + '</div>' : '') +
      '<div class="an-bbar-acts' + (always ? ' an-bbar-acts-always' : '') + '" data-key="acts">' +
      answerButton('secondary', 'Deny', 'Deny (Ctrl+Backspace)', sid, id, 'deny', armed) +
      (always ? answerButton('secondary', 'Always allow', always + ' (Ctrl+Alt+Enter)', sid, id, 'always', armed) : '') +
      (left ? jumpButton(row, true) : answerButton('primary', 'Allow', 'Allow (Ctrl+Enter)', sid, id, 'allow', armed)) +
      '</div>');
  }

  // -- a plan -----------------------------------------------------------------------------------

  function planBar(row, p, armed) {
    var left = C.cutOff(p.plan_markdown, C.SHOWN_WHOLE.plan);
    var plan = clip(str(p.plan_markdown), C.SHOWN_WHOLE.plan);
    var md = window.agentnotchMarkdown;
    var body = plan.trim() ? (md ? md.render(plan, { cls: 'an-md-plan' }) : '<div class="an-md an-md-plan">' + C.esc(plan) + '</div>') : '';
    var sid = row.session_id;
    var id = p.tool_use_id;
    return bar('plan', 'bar-plan-' + id, armed,
      barTitle('Plan ready for approval') +
      (body ? '<div class="an-bbar-box an-bbar-plantext" data-key="plan" tabindex="0" aria-label="The plan">' + body + '</div>' : '') +
      (left ? cutNote(left) : '') +
      '<div class="an-bbar-foot" data-key="foot"><span class="an-bbar-note an-bbar-foot-t" data-an-text data-an-clip>' + C.esc(COPY.planNote) + '</span>' +
      answerButton('secondary', 'Keep planning', 'Stay in plan mode and say what to change (Ctrl+Backspace)', sid, id, 'keep', armed) +
      (left ? jumpButton(row, true) : answerButton('primary', 'Approve plan', 'Approve the plan (Ctrl+Enter)', sid, id, 'approve', armed)) +
      '</div>');
  }

  // -- questions (ChatQuestionPanel, ChatQuestionForm) ------------------------------------------

  /** The engine's parsed questions, each with a text and labelled options; others are left out, as the Mac's parser skips them. */
  function questionsOf(p) {
    return (Array.isArray(p.questions) ? p.questions : []).filter(function (q) {
      return q && typeof q === 'object' && typeof q.text === 'string' && q.text.trim();
    }).map(function (q) {
      return {
        text: q.text,
        header: str(q.header).trim(),
        multi: q.multi_select === true,
        options: (Array.isArray(q.options) ? q.options : []).filter(function (o) {
          return o && typeof o.label === 'string' && o.label.trim();
        }).map(function (o) {
          return { label: o.label, description: str(o.description).trim() };
        }),
      };
    });
  }

  function formOf(id, count) {
    var form = S.forms[id];
    if (!form) {
      // One request's answers at a time: an answered or withdrawn request's picks go with it.
      S.forms = Object.create(null);
      form = S.forms[id] = { picks: [], submitted: false };
    }
    while (form.picks.length < count) form.picks.push({ labels: [], other: false, text: '' });
    return form;
  }

  /** ChatQuestionSelection.toggle / toggleOther. */
  function toggleOption(pick, label, multi) {
    if (multi) {
      var at = pick.labels.indexOf(label);
      if (at >= 0) pick.labels.splice(at, 1);
      else pick.labels.push(label);
    } else {
      pick.labels = [label];
      pick.other = false;
    }
  }

  function toggleOther(pick, multi) {
    if (multi) {
      pick.other = !pick.other;
    } else {
      pick.other = true;
      pick.labels = [];
    }
  }

  function picksFor(form) {
    return form.picks.map(function (pick) {
      return { labels: pick.labels.slice(), other: pick.other ? pick.text : null };
    });
  }

  function answersFor(questions, form) {
    var build = panel && typeof panel.answers === 'function' ? panel.answers : null;
    if (!build) return null;
    return build(questions.map(function (q) {
      return { text: q.text, multi_select: q.multi, options: q.options.map(function (o) { return { label: o.label }; }) };
    }), picksFor(form));
  }

  function answeredCount(questions, form) {
    var n = 0;
    questions.forEach(function (q, i) {
      if (answersFor([q], { picks: [form.picks[i]] })) n += 1;
    });
    return n;
  }

  function selectionMark(multi, on) {
    return '<span class="an-qmark ' + (multi ? 'an-qmark-box' : 'an-qmark-radio') + (on ? ' an-on' : '') + '" aria-hidden="true">' +
      (multi && on ? C.icon('check') : '') + '</span>';
  }

  function fieldAttrs(name, placeholder, label) {
    var open = keyboardOpen();
    return ' data-an-keep data-an-field="' + attr(name) + '" placeholder="' + attr(open ? placeholder : COPY.clickToType) + '" aria-label="' + attr(label) + '"' +
      (open ? '' : ' readonly title="' + attr(COPY.clickToType) + '"');
  }

  function questionBlock(q, i, pick) {
    var multi = q.multi;
    var options = q.options.map(function (o, j) {
      var on = pick.labels.indexOf(o.label) >= 0;
      var label = clip(o.label, LIMITS_BAR.option);
      var desc = clip(o.description, LIMITS_BAR.option);
      return '<button type="button" class="an-qopt' + (on ? ' an-on' : '') + '" data-key="o' + j + '" data-an-chat="q-option" data-q="' + i + '" data-o="' + j +
        '" role="' + (multi ? 'checkbox' : 'radio') + '" aria-checked="' + on + '" aria-label="' + attr(desc ? label + ', ' + desc : label) + '">' +
        selectionMark(multi, on) + '<span class="an-qopt-t"><span class="an-qopt-l">' + C.esc(label) + '</span>' +
        (desc ? '<span class="an-qopt-d">' + C.esc(desc) + '</span>' : '') + '</span></button>';
    }).join('');
    var other = '<div class="an-qother' + (pick.other ? ' an-on' : '') + '" data-key="other">' +
      '<button type="button" class="an-qother-b" data-an-chat="q-other" data-q="' + i + '" role="' + (multi ? 'checkbox' : 'radio') + '" aria-checked="' + pick.other + '">' +
      selectionMark(multi, pick.other) + '<span class="an-qother-l">Other</span></button>' +
      (pick.other ? '<input type="text" class="an-field an-qother-f" data-key="field" value="' + attr(pick.text) + '" maxlength="2000"' +
        fieldAttrs('other:' + i, COPY.other, 'Other answer') + '>' : '') +
      '</div>';
    return '<div class="an-q" data-key="q' + i + '" role="group" aria-label="' + attr(clip(q.text, LIMITS_BAR.question)) + '">' +
      '<div class="an-q-head">' + (q.header ? '<span class="an-q-chip" data-an-text>' + C.esc(clip(q.header, LIMITS_BAR.header)) + '</span>' : '') +
      '<span class="an-q-text">' + C.esc(clip(q.text.trim(), LIMITS_BAR.question)) + '</span>' +
      (multi ? '<span class="an-q-any">Choose any</span>' : '') + '</div>' +
      '<div class="an-q-opts">' + options + other + '</div></div>';
  }

  function questionBar(row, p, questions, armed) {
    var id = p.tool_use_id;
    var form = formOf(id, questions.length);
    var ready = !!answersFor(questions, form);
    var n = questions.length;
    var answered = answeredCount(questions, form);
    var footer = form.submitted ? COPY.sent
      : n > 1 ? answered + ' of ' + n + ' answered'
        : answered === 1 ? 'Ready to send' : 'Pick an answer';
    var canSubmit = ready && !form.submitted;
    return bar('question', 'bar-q-' + id, armed,
      barTitle(n === 1 ? 'Claude has a question' : 'Claude has ' + n + ' questions') +
      '<div class="an-bbar-qs" data-key="qs">' + questions.map(function (q, i) { return questionBlock(q, i, form.picks[i]); }).join('') + '</div>' +
      '<div class="an-bbar-foot" data-key="foot"><span class="an-bbar-note an-bbar-foot-t" data-an-text data-an-clip role="status">' + C.esc(footer) + '</span>' +
      jumpButton(row, false) +
      '<button type="button" class="an-btn an-btn-primary' + (armed ? '' : ' an-unarmed') + '" data-key="submit" data-an-chat="q-submit" data-an-noenter' +
      (canSubmit ? '' : ' disabled') + (armed ? '' : ' aria-disabled="true"') + '><span data-an-text>' + (n === 1 ? 'Submit' : 'Submit answers') + '</span></button>' +
      '</div>');
  }

  // -- no route, and the composer -----------------------------------------------------------------

  function noRouteBar(row) {
    var reason = S.route.reason;
    return terminalOnlyBar(row, null, reason ? reason + '. Type in the terminal instead.' : COPY.routeSentence);
  }

  function composerBar() {
    var draft = draftOf(S.sessionId);
    var empty = !draft.trim();
    var can = !empty && !S.sending;
    return bar('comp', 'bar-comp', null,
      (S.failure ? '<div class="an-bbar-fail" data-key="fail" role="alert">' + C.esc(S.failure) + '</div>' : '') +
      '<div class="an-comp" data-key="comp">' +
      '<textarea class="an-field an-comp-f" data-key="field" rows="1" maxlength="' + DRAFT_MAX + '"' + fieldAttrs('composer', COPY.reply, COPY.reply) + '>' + C.esc(draft) + '</textarea>' +
      '<button type="button" class="an-send' + (can ? ' an-on' : '') + '" data-key="send" data-an-chat="send" aria-label="Send" title="Send (Enter). Shift+Enter starts a new line."' +
      (can ? '' : ' disabled') + '>' + C.icon('arrowUp') + '</button></div>');
  }

  /** The row's bar, by the Mac's precedence. Tells the page's AnswerGate which request it shows. */
  function barHtml(row) {
    if (!S) return '';
    var p = row && row.pending && typeof row.pending === 'object' ? row.pending : null;
    var id = p && typeof p.tool_use_id === 'string' && p.tool_use_id ? p.tool_use_id : null;
    panelCall('noteShown', id ? [id] : []);
    if (id) {
      // A still scene has no wait: its bar is drawn answerable (and panel.js sends nothing from it).
      var armed = S.scene || isArmed(id);
      if (p.kind === 'question') {
        var questions = questionsOf(p);
        if (!questions.length) return terminalOnlyBar(row, 'Claude has a question', COPY.unshownQuestion);
        return questionBar(row, p, questions, armed);
      }
      if (p.kind === 'plan') return planBar(row, p, armed);
      return approvalBar(row, p, armed);
    }
    if (row && C.primaryActions(row).kind === 'answer_in_terminal') {
      var d = row.detail || {};
      var title = str(d.text).trim() ? clip(C.oneLine(d.text), LIMITS.title)
        : d.kind === 'permission' ? (toolNameOf(d.tool) || 'A tool') + ' needs your permission'
          : d.kind === 'plan' ? 'Plan ready for approval'
            : d.kind === 'question' ? 'Claude has a question' : 'Claude is waiting in the terminal';
      return terminalOnlyBar(row, title, COPY.dialog);
    }
    // Until the engine has said whether a reply can reach the terminal, nothing is offered.
    if (S.route.state === 'asking') return '';
    if (S.route.state !== 'available') return noRouteBar(row);
    return composerBar();
  }

  SLOTS.bottom = barHtml;

  // -- what the bar's controls do -------------------------------------------------------------

  function fieldEl(name) {
    if (!els.bar) return null;
    var all = els.bar.querySelectorAll('[data-an-field]');
    for (var i = 0; i < all.length; i++) if (all[i].getAttribute('data-an-field') === name) return all[i];
    return null;
  }

  function fieldValue(name) {
    if (name === 'composer') return draftOf(S.sessionId);
    var m = /^other:(\d+)$/.exec(name);
    var form = currentForm();
    var pick = m && form ? form.form.picks[Number(m[1])] : null;
    return pick ? pick.text : null;
  }

  /** The fields show the state the page keeps (a restored draft, a scene) unless the reader is in them. */
  function syncFields() {
    if (!els.bar) return;
    Array.prototype.forEach.call(els.bar.querySelectorAll('[data-an-field]'), function (el) {
      var want = fieldValue(el.getAttribute('data-an-field'));
      if (want !== null && document.activeElement !== el && el.value !== want) el.value = want;
    });
    var composer = fieldEl('composer');
    var appeared = !!composer && composer !== S.composerEl;
    S.composerEl = composer;
    // The composer takes the keyboard when it appears, as on the Mac, but only when the panel
    // already has it: never while the gate is shut.
    if (appeared && keyboardOpen() && !textFieldFocused()) focusField(composer);
  }

  function textFieldFocused() {
    var a = document.activeElement;
    return !!a && typeof a.hasAttribute === 'function' && a.hasAttribute('data-an-field');
  }

  function focusField(el) {
    if (!el || !keyboardOpen() || typeof el.focus !== 'function') return;
    el.focus();
  }

  /** The request whose question form is on screen: `{row, p, questions, form}` or null. */
  function currentForm() {
    var row = rowNow();
    var p = row && row.pending;
    if (!p || p.kind !== 'question' || typeof p.tool_use_id !== 'string') return null;
    var questions = questionsOf(p);
    if (!questions.length) return null;
    return { row: row, p: p, questions: questions, form: formOf(p.tool_use_id, questions.length) };
  }

  function barArmed() {
    var row = rowNow();
    var id = row && row.pending ? str(row.pending.tool_use_id) : '';
    return !!id && isArmed(id);
  }

  function pickOption(qi, oi) {
    var f = currentForm();
    if (!f || f.form.submitted || !barArmed()) return;
    var q = f.questions[qi];
    var o = q && q.options[oi];
    if (!o) return;
    toggleOption(f.form.picks[qi], o.label, q.multi);
    drawBar();
  }

  function pickOther(qi) {
    var f = currentForm();
    if (!f || f.form.submitted || !barArmed() || !f.questions[qi]) return;
    toggleOther(f.form.picks[qi], f.questions[qi].multi);
    drawBar();
    if (f.form.picks[qi].other) focusField(fieldEl('other:' + qi));
  }

  function submitQuestions() {
    var f = currentForm();
    if (!f || f.form.submitted) return;
    var answers = answersFor(f.questions, f.form);
    if (!answers) return;
    if (panelCall('answer', f.row.session_id, f.p.tool_use_id, { questions: { answers: answers } }) !== true) return;
    f.form.submitted = true;
    drawBar();
  }

  /** send_message's outcome in the Mac's words (ChatComposerCopy), the engine's reason in them. */
  function failureCopy(outcome, reason) {
    var r = sentence(reason, LIMITS_BAR.failure);
    switch (outcome) {
      case 'refused':
        return 'Not sent: ' + (r || 'the session can’t take a reply now') + '. Your message is kept.';
      case 'typed_not_submitted':
        return 'Typed but not submitted: ' + (r || 'the terminal wasn’t ready') + '. Press Enter in the terminal when it’s safe.';
      default:
        return 'Couldn’t reach the session’s terminal' + (r ? ': ' + r : '') + '. Type in the terminal instead.';
    }
  }

  function send() {
    if (!S || S.sending || S.sceneDraft !== null) return;
    var id = S.sessionId;
    var text = draftOf(id).trim();
    if (!text) return;
    var mine = S;
    S.sending = true;
    S.failure = null;
    drawBar();
    reportSize();
    var done = function (outcome, reason) {
      // A draft is the reader's words: dropped only once Claude has them. A reply typed but not
      // submitted is in Claude's prompt already; keeping it here too would send it twice.
      var sent = (outcome === 'delivered' || outcome === 'typed_not_submitted') && draftOf(id).trim() === text;
      if (sent) setDraft(id, '');
      if (S !== mine) return;
      // The field the reader is in is left alone by a redraw: empty it here, unless more was typed.
      var field = fieldEl('composer');
      if (sent && field && String(field.value).trim() === text) field.value = '';
      S.sending = false;
      S.failure = outcome === 'delivered' ? null : failureCopy(outcome, reason);
      drawBar();
      reportSize();
    };
    C.call('send_message', { session_id: id, text: text }).then(function (reply) {
      var outcome = reply && typeof reply.outcome === 'string' ? reply.outcome : 'failed';
      done(outcome, reply && typeof reply.reason === 'string' ? reply.reason : '');
    }, function (error) {
      fail('send_message')(error);
      done('failed', '');
    });
  }

  function onInput(event) {
    var t = event.target;
    var name = t && typeof t.getAttribute === 'function' ? t.getAttribute('data-an-field') : null;
    if (!S || !name) return;
    // The gate is shut: whatever reached the field is not the reader's (the page takes no text
    // before the glue confirms the keyboard). Put back what was there.
    if (!keyboardOpen()) {
      var was = fieldValue(name);
      if (was !== null) t.value = was;
      return;
    }
    if (name === 'composer') {
      setDraft(S.sessionId, t.value);
    } else {
      var m = /^other:(\d+)$/.exec(name);
      var f = currentForm();
      var pick = m && f ? f.form.picks[Number(m[1])] : null;
      if (!pick || f.form.submitted) return;
      pick.text = String(t.value).slice(0, 2000);
    }
    drawBar();
  }

  /** A press on a field while the gate is shut: panel.js asks for the keyboard; this remembers where the caret goes. */
  function onPointerDown(event) {
    var t = event.target;
    var name = t && typeof t.getAttribute === 'function' ? t.getAttribute('data-an-field') : null;
    if (S && name && !keyboardOpen()) S.wantFocus = name;
  }

  function onPanelFocus(payload) {
    if (!S) return;
    var focused = !!(payload && payload.focused);
    drawBar();
    if (!focused) return;
    var want = S.wantFocus;
    S.wantFocus = null;
    var el = want ? fieldEl(want) : null;
    if (!el && !textFieldFocused()) el = fieldEl('composer');
    focusField(el);
  }

  /** Enter in the composer sends (Ctrl+Enter too, as on the Mac); Shift+Enter and Alt+Enter start a line. */
  function composerKey(event, field) {
    if (event.key !== 'Enter' || event.isComposing) return false;
    // The bar owns Enter here: the panel's own Ctrl+Enter must not act on the session as well.
    event.stopPropagation();
    if (!keyboardOpen()) {
      event.preventDefault();
      return true;
    }
    if (event.shiftKey) return true;
    event.preventDefault();
    if (event.altKey) {
      if (typeof field.setRangeText === 'function' && typeof field.selectionStart === 'number') {
        field.setRangeText('\n', field.selectionStart, field.selectionEnd, 'end');
      } else {
        field.value += '\n';
      }
      setDraft(S.sessionId, field.value);
      drawBar();
      return true;
    }
    if (!event.repeat) send();
    return true;
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
    syncFields();
  }

  /** What the page reads from the row changed (a snapshot, a request arming): redraw the parts that follow it. */
  function refresh() {
    if (!S) return;
    var row = rowNow();
    // Whether a reply can be typed is asked again when the engine's view of it changes.
    if (row && typeof row.can_message === 'boolean') {
      if (S.canMessage !== null && S.canMessage !== row.can_message) askRoute();
      S.canMessage = row.can_message;
    }
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
      case 'q-option': pickOption(Number(control.getAttribute('data-q')), Number(control.getAttribute('data-o'))); break;
      case 'q-other': pickOther(Number(control.getAttribute('data-q'))); break;
      case 'q-submit': submitQuestions(); break;
      case 'send': send(); break;
      default: break;
    }
  }

  /** A link is a span, not a button: Enter opens it, but only once the keyboard gate is open. */
  function onKeyDown(event) {
    var t = event.target;
    if (!t || typeof t.closest !== 'function') return;
    if (S && typeof t.getAttribute === 'function' && t.getAttribute('data-an-field') === 'composer' && composerKey(event, t)) return;
    if (event.key !== 'Enter') return;
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
    host.addEventListener('input', onInput);
    host.addEventListener('pointerdown', onPointerDown);
    els.scroll.addEventListener('scroll', onScroll);
    if (!focusListening) focusListening = C.listen('an:panel_focus', onPanelFocus);
    if (typeof window.ResizeObserver === 'function') {
      observer = new window.ResizeObserver(reportSize);
      observer.observe(els.list);
      observer.observe(els.head);
      observer.observe(els.foot);
    }
    var first = rowNow();
    if (first && typeof first.can_message === 'boolean') S.canMessage = first.can_message;
    drawHeader();
    draw(false);
    var mine = S;
    // The listener first, then the question: the engine's reset must not beat it.
    if (!listening) listening = C.listen('an:chat', apply);
    Promise.resolve(listening).then(function () {
      if (S !== mine) return;
      askRoute();
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
      host.removeEventListener('input', onInput);
      host.removeEventListener('pointerdown', onPointerDown);
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

  /** The sealed chat scenes' own state, set after the route has drawn the chat (panel.showScene). */
  var SCENE_STATE = {
    'chat-question-other': function () {
      var f = currentForm();
      if (!f) return;
      f.form.picks[0] = { labels: [], other: true, text: 'Victory, it matches our design system' };
    },
    'chat-composer': function () {
      S.sceneDraft = 'Great, do the same for the e2e suite';
    },
  };

  function scene(name) {
    if (!S) return false;
    S.scene = true;
    if (Object.prototype.hasOwnProperty.call(SCENE_STATE, name)) SCENE_STATE[name]();
    drawBar();
    reportSize();
    return true;
  }

  var api = {
    mount: mount,
    unmount: unmount,
    refresh: refresh,
    apply: apply,
    naturalHeight: naturalHeight,
    layoutProblems: layoutProblems,
    /** The page's report (the chat lives in the panel's card): `{ok, failures, ...}`, the chat's own problems included in a chat. */
    layoutReport: function () {
      var panel = window.agentnotchPanel;
      return panel && typeof panel.layoutReport === 'function' ? panel.layoutReport() : { ok: false, failures: ['no panel page'] };
    },
    /** Snapshots: the task board open or shut. */
    openBoard: setBoard,
    /** The bottom bar's renderer (`(row, chat) => html`); tests may replace it. */
    slots: SLOTS,
    /** Snapshots: a chat scene's own state (an "Other" answer typed, a draft). */
    scene: scene,
    /** The draft kept for a session (memory, else localStorage). */
    draft: function (sessionId) {
      return draftOf(String(sessionId));
    },
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
        route: { state: S.route.state, reason: S.route.reason },
        sending: S.sending,
        failure: S.failure,
      } : null;
    },
    PAGE_SIZE: PAGE_SIZE,
  };

  window.agentnotchChat = api;
})();
