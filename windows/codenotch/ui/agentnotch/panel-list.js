// The sessions panel's list, as pure functions (DESIGN-WIN §5.3, UI§5.3-5.7): which rows go in
// which section and in what order, what folds, when rows go to one line, and the markup of a
// section header, a folded summary, a row (regular and compact) and the undo toast.
//
// Nothing here touches the page or the bridge: panel.js owns the view state (folds, the
// selection, the frozen order, the pending review), calls `layout()` and `html()` with it and
// morphs the result into #an-rows. What the ENGINE decides arrives in the snapshot and is only
// shown: state_word-free rows (`detail`, `a11y`, `focus_label`, `pending`), the sections' titles
// and their default folding. What the Mac decides in Swift and the snapshot carries no field for
// is decided here, by the Mac's rules (SessionSections, SessionListLayout): the order inside a
// section (from `since_ms`), the counts drawn, the folded summary line, compact rows past eight.
//
// Every string of a row is untrusted (titles, projects, requests, account labels): it reaches
// the markup only through agentnotchCommon.esc, long ones cut first so a hostile 10 000-character
// title does not become a 10 000-character DOM. The action bar of a row is drawn by panel.js
// into the named slot `.an-row-actions` (`o.actions(row, ctx)`).
//
// One IIFE, one global (agentnotchPanelList), no top-level let/const.
(function () {
  'use strict';

  var BUCKETS = ['needs_you', 'ready_for_review', 'working', 'idle'];
  var TITLES = { needs_you: 'Needs you', ready_for_review: 'Ready for review', working: 'Working', idle: 'Idle' };
  var GLYPH_TONE = { needs: 'needs', error: 'error', review: 'review', working: 'working', idle: 'idle' };
  /** A section that starts folded when it has more rows than this (SessionSections.idleCollapseThreshold). */
  var IDLE_FOLD_ABOVE = 3;
  /** Rows go to one line when more than this many are drawn (SessionSections.compactThreshold). */
  var COMPACT_ABOVE = 8;
  /** The task bar's segments: at most this many, each at least this wide (TaskProgressBar). */
  var MAX_SEGMENTS = 12;
  var MIN_SEGMENT = 6.5;
  var TASK_WIDTH = 64;
  var TASK_WIDTH_COMPACT = 28;
  /** How much of a string is kept in the DOM; the CSS clips the rest to its lines anyway. */
  var LIMITS = { title: 300, detail: 500, request: 4000, project: 120, account: 80, a11y: 600, tip: 200 };
  var TERMINAL_WAIT = 'waiting in the terminal';

  function C() {
    return window.agentnotchCommon;
  }

  function str(value) {
    return value == null ? '' : String(value);
  }

  /** `text` cut to `max` characters with an ellipsis (by code point, never inside a pair). */
  function clip(text, max) {
    var s = str(text);
    if (s.length <= max) return s;
    var chars = Array.from(s);
    return chars.length <= max ? s : chars.slice(0, max - 1).join('') + '…';
  }

  function esc(text) {
    return C().esc(text);
  }

  /** A one-line, escaped attribute text (a tooltip or an aria-label), cut to `max`. */
  function attr(text, max) {
    return esc(clip(C().oneLine(text), max || LIMITS.tip));
  }

  function bucketOf(row) {
    return BUCKETS.indexOf(row && row.bucket) >= 0 ? row.bucket : 'idle';
  }

  function since(row) {
    var n = Number(row && row.since_ms);
    return n > 0 && isFinite(n) ? n : null;
  }

  // ---- ordering (SessionSections.naturallyPrecedes) --------------------------------------

  /**
   * Needs you: an answerable request before a failed turn, each oldest wait first. Ready for
   * review and Idle: newest first. Working: longest running first. A row with no time goes last;
   * ties are broken by session id so equal keys never swap between renders.
   */
  function precedes(a, b, bucket) {
    if (bucket === 'needs_you') {
      var ra = a.failed ? 1 : 0;
      var rb = b.failed ? 1 : 0;
      if (ra !== rb) return ra < rb;
    }
    var sa = since(a);
    var sb = since(b);
    if (sa !== sb) {
      if (sa === null) return false;
      if (sb === null) return true;
      return bucket === 'ready_for_review' || bucket === 'idle' ? sa > sb : sa < sb;
    }
    return str(a.session_id) < str(b.session_id);
  }

  function sorted(rows, bucket) {
    return rows.slice().sort(function (a, b) {
      if (precedes(a, b, bucket)) return -1;
      if (precedes(b, a, bucket)) return 1;
      return 0;
    });
  }

  /** While the pointer is over the list rows keep the order they had; newcomers go to the end. */
  function preserved(rows, frozen) {
    var index = Object.create(null);
    frozen.forEach(function (id, i) {
      if (!(id in index)) index[id] = i;
    });
    var known = [];
    var fresh = [];
    rows.forEach(function (row) {
      (str(row.session_id) in index ? known : fresh).push(row);
    });
    known.sort(function (a, b) { return index[str(a.session_id)] - index[str(b.session_id)]; });
    return known.concat(fresh);
  }

  /** "Write tests, Fix CI and 1 more" (SessionSections.collapsedSummary). */
  function summary(rows) {
    var titles = rows.slice(0, 2).map(function (r) { return clip(C().oneLine(r.title), LIMITS.title); });
    var rest = rows.length - titles.length;
    return rest > 0 ? titles.join(', ') + ' and ' + rest + ' more' : titles.join(', ');
  }

  // ---- the layout ------------------------------------------------------------------------

  /**
   * The list as values. `v` is the snapshot, `o`: `{filter, folds, frozen, hidden}`:
   *   filter  a ring id the rows are narrowed to (null: every account)
   *   folds   {bucket: true|false}, what the user chose (Needs you never folds)
   *   frozen  the ids as displayed, or null; set while the pointer is over the list
   *   hidden  (row) -> true for a row shown as already gone (a pending mark-all-reviewed)
   * Returns `{sections, compact, order, visible, count, isEmpty}`; each section is
   * `{bucket, title, rows, count, collapsed, canFold, summary}` in the snapshot's section order.
   */
  function layout(v, o) {
    var opts = o || {};
    var folds = opts.folds || {};
    var all = v && Array.isArray(v.sessions) ? v.sessions : [];
    var rows = all.filter(function (r) {
      if (!r || typeof r !== 'object') return false;
      if (opts.filter && r.ring_id !== opts.filter) return false;
      return !(typeof opts.hidden === 'function' && opts.hidden(r));
    });
    var groups = Object.create(null);
    rows.forEach(function (r) {
      var b = bucketOf(r);
      (groups[b] || (groups[b] = [])).push(r);
    });
    // The snapshot's section order and titles; a bucket it does not list follows in the Mac's order.
    var infos = Object.create(null);
    var order = [];
    (v && Array.isArray(v.sections) ? v.sections : []).forEach(function (s) {
      if (s && BUCKETS.indexOf(s.bucket) >= 0 && !infos[s.bucket]) {
        infos[s.bucket] = s;
        order.push(s.bucket);
      }
    });
    BUCKETS.forEach(function (b) {
      if (order.indexOf(b) < 0) order.push(b);
    });
    var sections = [];
    order.forEach(function (bucket) {
      var members = groups[bucket];
      if (!members || !members.length) return;
      members = sorted(members, bucket);
      if (opts.frozen && opts.frozen.length) members = preserved(members, opts.frozen);
      var info = infos[bucket];
      var canFold = bucket !== 'needs_you';
      // The engine says which section starts folded; a narrowed list starts one folded only when
      // it is still long (the Mac counts the rows it shows).
      var byDefault = info ? !!info.fold_by_default : bucket === 'idle';
      if (byDefault && (opts.filter || !info)) byDefault = members.length > IDLE_FOLD_ABOVE;
      var collapsed = canFold && (Object.prototype.hasOwnProperty.call(folds, bucket) ? !!folds[bucket] : byDefault);
      sections.push({
        bucket: bucket,
        title: info && typeof info.title === 'string' && info.title ? info.title : TITLES[bucket],
        rows: members,
        count: members.length,
        collapsed: collapsed,
        canFold: canFold,
        summary: summary(members),
      });
    });
    var drawn = 0;
    var visible = [];
    var flat = [];
    sections.forEach(function (s) {
      s.rows.forEach(function (r) {
        flat.push(str(r.session_id));
        if (!s.collapsed) {
          drawn += 1;
          visible.push(str(r.session_id));
        }
      });
    });
    return {
      sections: sections,
      // Rows drawn, not sessions: a long idle list folded to one line does not squeeze the rows above.
      compact: drawn > COMPACT_ABOVE,
      order: flat,
      visible: visible,
      count: rows.length,
      isEmpty: sections.length === 0,
    };
  }

  // ---- what a row says -------------------------------------------------------------------

  /** "2m" for a wait or a turn, "5m ago" for a finished or idle one; null when there is no time. */
  function elapsed(row, now) {
    var t = since(row);
    if (t === null) return null;
    var ms = Math.max(0, now - t);
    var b = bucketOf(row);
    return (b === 'ready_for_review' || b === 'idle') && !row.failed ? C().age(ms) : C().duration(ms);
  }

  function tone(kind) {
    return GLYPH_TONE[kind] || 'idle';
  }

  function trail(row, now) {
    var kind = C().glyphKind(row);
    var p = row.pending;
    var breath = p && p.tool_use_id ? p.tool_use_id : since(row);
    var time = elapsed(row, now);
    return '<span class="an-trail an-tone-' + tone(kind) + '" aria-hidden="true">' +
      C().statusRing(kind, { breathKey: breath, stroke: 1.6 }) +
      (time === null ? '' : '<span class="an-elapsed" data-an-text>' + esc(time) + '</span>') + '</span>';
  }

  function rowButton(kind, action, id, label, icon) {
    return '<button type="button" class="an-rbtn an-rb-' + kind + '" data-key="rb-' + kind + '" data-an-action="' + action +
      '" data-an-arg="' + esc(id) + '" title="' + attr(label) + '" aria-label="' + attr(label) + '">' + C().icon(icon) + '</button>';
  }

  /** ✓ mark reviewed (or ✕ dismiss a failure), ↗ show the terminal, 💬 open the chat. */
  function hoverActions(row) {
    var id = str(row.session_id);
    var html = '';
    if (row.failed) html += rowButton('dismiss', 'dismiss-failure', id, 'Dismiss (Ctrl+R)', 'xCircle');
    else if (bucketOf(row) === 'ready_for_review') html += rowButton('review', 'mark-reviewed', id, 'Mark reviewed (Ctrl+R)', 'checkCircle');
    if (row.focus_label) html += rowButton('jump', 'jump', id, str(row.focus_label) + ' (Ctrl+J)', 'openApp');
    html += rowButton('chat', 'open-chat', id, 'Open chat (Enter)', 'bubble');
    return '<span class="an-hover">' + html + '</span>';
  }

  function trailSlot(row, now) {
    return '<div class="an-slot" data-key="slot">' + trail(row, now) + hoverActions(row) + '</div>';
  }

  // ---- the detail line (SessionDetailView) -----------------------------------------------

  function codeBox(text, whole) {
    return '<div class="an-code' + (whole ? ' an-code-whole' : '') + '" data-an-text>' + esc(clip(text, LIMITS.request)) + '</div>';
  }

  function textLine(text, cls, lines) {
    return '<div class="an-detail ' + cls + '" data-an-text data-an-clip data-lines="' + lines + '">' + esc(clip(C().oneLine(text), LIMITS.detail)) + '</div>';
  }

  function detailHtml(row) {
    var d = row.detail && typeof row.detail === 'object' ? row.detail : {};
    var text = str(d.text);
    switch (d.kind) {
      case 'permission': {
        var terminal = !!d.waiting_in_terminal;
        var name = str(d.tool) || 'Permission';
        var actions = C().primaryActions(row);
        var whole = !terminal && actions.kind === 'permission' && !actions.needsReview;
        // Allow sits right below a request the row can answer: show it whole, however it wraps.
        return '<div class="an-detail an-d-att">' +
          '<div class="an-tool" data-an-text data-an-clip>' + esc(clip(C().oneLine(name), LIMITS.title)) + '</div>' +
          (terminal ? codeBox(TERMINAL_WAIT, false) : str(d.request) ? codeBox(d.request, whole) : '') + '</div>';
      }
      case 'question':
        return '<div class="an-detail an-d-prompt" data-lines="2"><span class="an-lbl" data-an-text>Asks</span> ' +
          '<span class="an-ptext" data-an-text data-an-clip>' + esc(clip(C().oneLine(text) || 'A question for you', LIMITS.detail)) + '</span></div>';
      case 'plan':
        return textLine('Plan ready for approval', 'an-d-primary', 1);
      case 'dialog':
        return textLine(text || 'Waiting for you', 'an-d-primary', 2);
      case 'failed':
        return textLine(text || 'Turn failed', 'an-d-error', 2);
      case 'working':
        return text ? textLine(text, d.secondary ? 'an-d-secondary' : 'an-d-primary', 1) : '';
      case 'review':
        return textLine(text || 'Finished', 'an-d-primary', 2);
      default:
        return textLine(text || 'No messages yet', 'an-d-secondary', 1);
    }
  }

  // ---- meta line: account · tasks · context · project · background -----------------------

  /** One bar of the task list: a segment per task while they fit, else a continuous fill. */
  function taskBar(tasks, width) {
    var total = Math.floor(Number(tasks && tasks.total));
    if (!(total > 0)) return '';
    var done = Math.min(Math.max(Math.floor(Number(tasks.done) || 0), 0), total);
    var items = Array.isArray(tasks.items) && tasks.items.length === total ? tasks.items : null;
    var bar;
    if (items && total <= MAX_SEGMENTS && width / total >= MIN_SEGMENT) {
      bar = items.map(function (item) {
        var status = item && item.status;
        return '<span class="an-tk an-tk-' + (status === 'completed' ? 'done' : status === 'in_progress' ? 'active' : 'todo') + '"></span>';
      }).join('');
      bar = '<span class="an-tbar an-tbar-seg" style="width:' + width + 'px">' + bar + '</span>';
    } else {
      var active = items ? items.some(function (it) { return it && it.status === 'in_progress'; }) : !!str(tasks.active_label);
      bar = '<span class="an-tbar an-tbar-fill" style="width:' + width + 'px"><span class="an-tk-done" style="width:' + (done / total * 100).toFixed(2) +
        '%"></span>' + (active && done < total ? '<span class="an-tk-active" style="left:' + (done / total * 100).toFixed(2) + '%;width:' + (100 / total).toFixed(2) + '%"></span>' : '') + '</span>';
    }
    return { total: total, done: done, bar: bar };
  }

  function tasksItem(tasks, width, withCount) {
    var t = taskBar(tasks, width);
    if (!t) return '';
    var label = t.done + ' of ' + t.total + ' tasks done' + (str(tasks.active_label) ? ', now: ' + clip(C().oneLine(tasks.active_label), 80) : '');
    return '<span class="an-tasks" data-key="tasks" role="img" aria-label="' + attr(label) + '" title="' + attr(label) + '">' + t.bar +
      (withCount ? '<span class="an-tcount" data-an-text>' + t.done + '/' + t.total + '</span>' : '') + '</span>';
  }

  function contextBar(pct) {
    var p = Number(pct);
    if (!isFinite(p)) return '';
    var fraction = Math.min(Math.max(p / 100, 0), 1);
    return '<span class="an-cbar"><span style="width:' + (fraction * 100).toFixed(1) + '%"></span></span>';
  }

  function contextItem(pct, compact) {
    var p = Number(pct);
    if (pct == null || !isFinite(p)) return '';
    var level = C().contextLevel(p);
    var text = C().percent(p);
    var label = 'Context window ' + text + ' full';
    return '<span class="an-ctx an-ctx-' + level + '" data-key="ctx" role="img" aria-label="' + attr(label) + '" title="' + attr(label) + '">' +
      contextBar(p) + '<span class="an-ctx-t" data-an-text>' + esc(text) + (compact ? '' : '<span class="an-ctx-word"> context</span>') + '</span></span>';
  }

  function accountName(row) {
    return row.account_label == null ? '' : clip(C().oneLine(row.account_label), LIMITS.account);
  }

  function accountItem(row) {
    var label = accountName(row);
    if (!label) return '';
    return '<span class="an-acct" data-key="acct" title="' + attr(label) + '">' + C().accountDot(row.account_color) +
      '<span class="an-acct-l" data-an-text data-an-clip>' + esc(label) + '</span></span>';
  }

  function backgroundItem(row) {
    var n = Math.floor(Number(row.background_count) || 0);
    if (n <= 0) return '';
    return '<span class="an-bg" data-key="bg" data-an-text title="' + n + ' background task' + (n === 1 ? '' : 's') + ' still running">' + n + ' background</span>';
  }

  function projectItem(row) {
    var project = str(row.project);
    if (!project || project === str(row.title)) return '';
    return '<span class="an-proj" data-key="proj" data-an-text data-an-clip>' + esc(clip(C().oneLine(project), LIMITS.project)) + '</span>';
  }

  function metaHtml(row, showAccount) {
    var items = [];
    var push = function (html) { if (html) items.push(html); };
    if (showAccount) push(accountItem(row));
    push(row.tasks ? tasksItem(row.tasks, TASK_WIDTH, true) : '');
    push(contextItem(row.context_pct, false));
    push(projectItem(row));
    push(backgroundItem(row));
    if (!items.length) return '';
    return '<div class="an-meta" data-key="meta">' + items.join('<span class="an-dotsep" aria-hidden="true">·</span>') + '</div>';
  }

  // ---- rows ------------------------------------------------------------------------------

  function rowClasses(row, o, extra) {
    var kind = C().glyphKind(row);
    return 'an-row an-b-' + tone(kind) + extra + (str(row.session_id) === o.selected ? ' an-sel' : '') + (str(row.session_id) === o.forceHover ? ' an-force' : '') + (o.first ? ' an-first' : '');
  }

  function rowOpen(row, o, extra) {
    var id = str(row.session_id);
    return '<div class="' + rowClasses(row, o, extra) + '" data-key="row-' + esc(id) + '" data-flip="' + esc(id) + '" data-id="' + esc(id) +
      '" data-an-action="open-chat" data-an-arg="' + esc(id) + '" role="group" aria-label="' + attr(row.a11y, LIMITS.a11y) + '"' +
      (id === o.selected ? ' aria-current="true"' : '') + '>';
  }

  /** The action bar's place: sub-task 6 (panel.js `slots.actions`) fills it; empty it takes no room. */
  function actionsSlot(row, o) {
    var inner = typeof o.actions === 'function' ? o.actions(row, o) : '';
    return '<div class="an-row-actions" data-key="actions" data-slot="actions">' + (inner || '') + '</div>';
  }

  function regularRow(row, o) {
    var showMeta = metaHtml(row, o.showAccount);
    return rowOpen(row, o, ' an-reg') +
      '<div class="an-r1"><span class="an-rtitle" data-an-text data-an-clip>' + esc(clip(C().oneLine(row.title), LIMITS.title)) + '</span>' + trailSlot(row, o.now) + '</div>' +
      detailHtml(row) + showMeta + actionsSlot(row, o) + '</div>';
  }

  /** What a one-line row says after its title: the turn's text while working, else the project. */
  function compactDetail(row) {
    var d = row.detail && typeof row.detail === 'object' ? row.detail : {};
    var b = bucketOf(row);
    if (b === 'working') return d.kind === 'working' ? str(d.text) : '';
    if (b === 'needs_you') return d.kind === 'question' || d.kind === 'dialog' || d.kind === 'failed' ? str(d.text) : '';
    return str(row.project);
  }

  /** The bar, the percentage and the account dot beside a one-line row's ring (CompactProgress). */
  function compactProgress(row, showAccount) {
    var html = '';
    if (row.tasks) html += tasksItem(row.tasks, TASK_WIDTH_COMPACT, false);
    html += contextItem(row.context_pct, true);
    if (showAccount && accountName(row)) html += '<span class="an-acct-dot" data-key="acct" title="' + attr(accountName(row)) + '">' + C().accountDot(row.account_color) + '</span>';
    return html ? '<span class="an-cprog" data-key="cprog">' + html + '</span>' : '';
  }

  function compactRow(row, o) {
    var detail = clip(C().oneLine(compactDetail(row)), LIMITS.detail);
    return rowOpen(row, o, ' an-cmp') +
      // The detail wraps out of a one-line box (and is clipped there) when it does not fit whole.
      '<div class="an-cmain"><span class="an-ctitle" data-an-text data-an-clip>' + esc(clip(C().oneLine(row.title), LIMITS.title)) + '</span>' +
      (detail ? '<span class="an-cdet" data-an-text>' + esc(detail) + '</span>' : '') + '</div>' +
      compactProgress(row, o.showAccount) + trailSlot(row, o.now) + '</div>';
  }

  /** One session's row; `o`: {compact, selected, first, showAccount, now, actions}. */
  function rowHtml(row, o) {
    var opts = o || {};
    if (opts.compact && bucketOf(row) !== 'needs_you') return compactRow(row, opts);
    return regularRow(row, opts);
  }

  // ---- sections --------------------------------------------------------------------------

  function headerHtml(section, first) {
    var b = section.bucket;
    var label = section.title + ', ' + section.count;
    var inner = '<span class="an-sh-title an-sh-' + b + '" data-an-text data-an-clip>' + esc(clip(C().oneLine(section.title), 60)) + '</span>' +
      '<span class="an-sh-n" data-an-text>' + section.count + '</span>' +
      (section.canFold ? '<span class="an-sh-chev' + (section.collapsed ? ' an-folded' : '') + '">' + C().icon('chevronDown') + '</span>' : '');
    var head = section.canFold
      ? '<button type="button" class="an-sh-btn" data-an-action="fold" data-an-arg="' + b + '" aria-expanded="' + (section.collapsed ? 'false' : 'true') +
        '" aria-label="' + attr(label) + ', ' + (section.collapsed ? 'folded' : 'unfolded') + '" title="' + (section.collapsed ? 'Shows the sessions' : 'Folds the section') + '">' + inner + '</button>'
      : '<span class="an-sh-btn an-sh-fixed" aria-label="' + attr(label) + '">' + inner + '</span>';
    var markAll = b === 'ready_for_review' && !section.collapsed
      ? '<button type="button" class="an-quiet" data-key="mark-all" data-an-action="mark-all-reviewed" title="Mark every session here reviewed (Ctrl+Shift+R). Undo for a few seconds after.">' +
        '<span data-an-text>Mark all reviewed</span></button>'
      : '';
    return '<div class="an-sh' + (first ? ' an-first' : '') + '" data-key="head-' + b + '"><h2 class="an-sh-h">' + head + '</h2>' + markAll + '</div>';
  }

  function foldedHtml(section) {
    var label = section.count + ' ' + section.title.toLowerCase() + ' sessions: ' + section.summary;
    return '<button type="button" class="an-foldsum" data-key="folded-' + section.bucket + '" data-an-action="fold" data-an-arg="' + section.bucket +
      '" aria-label="' + attr(label) + '" title="Shows them"><span data-an-text data-an-clip>' + esc(section.summary) + '</span></button>';
  }

  /**
   * The list's markup: one flat run of headers, folded summaries and rows keyed by session, so a
   * row that moves to another section keeps its element (and its FLIP glide). `o`: {selected,
   * multi, filtered, now, actions}.
   */
  function html(lay, o) {
    var opts = o || {};
    var showAccount = !!opts.multi && !opts.filtered;
    var out = '';
    lay.sections.forEach(function (section, index) {
      out += headerHtml(section, index === 0);
      if (section.collapsed) {
        out += foldedHtml(section);
        return;
      }
      section.rows.forEach(function (row, position) {
        out += rowHtml(row, {
          compact: lay.compact, selected: opts.selected, forceHover: opts.forceHover, first: position === 0, showAccount: showAccount,
          now: opts.now, actions: opts.actions,
        });
      });
    });
    return '<div class="an-lst" data-key="lst"' + (lay.compact ? ' data-compact="1"' : '') + ' role="list">' + out + '</div>';
  }

  // ---- the undo toast --------------------------------------------------------------------

  /** "Marked 2 reviewed" and Undo, while a mark-all-reviewed can still be taken back. */
  function toastHtml(pending) {
    if (!pending || !pending.ids || !pending.ids.length) return '';
    return '<div class="an-toast" data-key="toast" role="status">' +
      '<span class="an-toast-ring">' + C().statusRing('review', { stroke: 1.9 }) + '</span>' +
      '<span class="an-toast-t" data-an-text>Marked ' + pending.ids.length + ' reviewed</span>' +
      '<button type="button" class="an-toast-undo" data-key="undo" data-an-action="undo-review" title="Put them back in Ready for review (Ctrl+Z)" aria-label="Undo">' +
      '<span data-an-text>Undo</span></button></div>';
  }

  window.agentnotchPanelList = {
    BUCKETS: BUCKETS,
    COMPACT_ABOVE: COMPACT_ABOVE,
    IDLE_FOLD_ABOVE: IDLE_FOLD_ABOVE,
    layout: layout,
    html: html,
    rowHtml: rowHtml,
    toastHtml: toastHtml,
    elapsed: elapsed,
    summary: summary,
    sorted: sorted,
    bucketOf: bucketOf,
    detailHtml: detailHtml,
    taskBar: taskBar,
    // The chat's header draws the same task bar, context meter and account tag as a row.
    tasksItem: tasksItem,
    contextItem: contextItem,
    accountItem: accountItem,
  };
})();
