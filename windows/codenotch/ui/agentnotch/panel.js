// The sessions panel's page (DESIGN-WIN §5.3, UI§4-5): the shell every screen of the panel sits in.
// It owns the card and its tail, the header (title, Sealed badge, pin, gear menu, close, the
// attention strip and the account chips), the route (list or one session's chat), the keyboard
// gate's state, Esc, the size report and the sealed scenes. The list (sections, rows, folding,
// the undo toast) is panel-list.js's markup driven from here; the setup banners and the row
// action bars are drawn by a later sub-task into the regions this file leaves for them; the
// chat is chat.js's.
//
// How it is built, so the next sub-task can add to it without rewriting it:
//   * ONE state object (`state`) and ONE render(): it draws the whole page from the last whole
//     snapshot plus the view state (route, filter, menu, overrides) into fixed regions with
//     agentnotchCommon.morph, so a snapshot never drops focus, an open menu or a half-typed answer.
//       #an-header  -> headerHtml(v)      title row, attention strip, account chips
//       #an-banners -> bannersHtml(v)     setup banners (sub-task 6)
//       #an-rows    -> listHtml(v)        the sections and rows (agentnotchPanelList); the empty states
//       #an-toast   -> toastHtml(v)       the undo toast
//       #an-overlay -> overlayHtml(v)     the gear menu (a layer inside the card: the window is
//                                          exactly the card, so nothing can pop out of it)
//       #an-chat    -> agentnotchChat     mounted on a session route, never morphed here
//   * Clicks are delegated on the card: an element with data-an-action="name" (and optional
//     data-an-arg) runs ACTIONS[name](arg, element, event). A test drives one with
//     page.click('[data-an-action="pin"]'). Anything inside the gear menu closes it first.
//   * Times come from agentnotchCommon.now(), never Date.now().
//   * Every string from the snapshot is untrusted and is drawn through agentnotchCommon.esc.
//
// One IIFE, one global (agentnotchPanel), no top-level let/const (a clash with a classic script
// would be a SyntaxError that kills the page).
(function () {
  'use strict';

  var C = null;
  var L = null;
  var els = {};
  var started = false;

  var TAIL_LENGTH = 32;
  var TAIL_WIDTH = 36;
  var CORNER = 16;
  /** Card widths by placement and screen (ClaudePanelGeometry.width): list, chat. */
  var WIDTHS = { beside: [400, 440], flat: [440, 520] };
  var AUTO_OPEN = [
    ['never', 'Never'],
    ['needsInput', 'When a session needs you'],
    ['needsInputOrDone', 'When one needs you or is done'],
  ];
  /** [setting key, menu text, the field of Settings' notifications section that holds it]. */
  var NOTIFY = [
    ['notifyNeedsInput', 'Notify when a session needs you', 'notify_needs_input'],
    ['notifyReadyForReview', 'Notify when a session is done', 'notify_ready_for_review'],
  ];
  /** Chip labels shorter than this fit the chip's 150 px; longer ones are cut in the middle. */
  var CHIP_CHARS = 24;
  /** How long "Marked N reviewed" can be undone before it is sent (ClaudePanelState.undoWindow). */
  var UNDO_MS = 5000;
  /** How often the elapsed labels are redrawn (they read minutes: a wait never shows a stale one). */
  var TICK_MS = 15000;
  /** A set_setting's local value stands this long after its reply, for a snapshot still in flight. */
  var OVERRIDE_MS = 2000;

  var state = {
    /** The last whole snapshot (HubSnapshot), or null before the first. */
    snapshot: null,
    at: 0,
    /** Why there is none: the engine's message, shown in place of the list. */
    error: null,
    /** `sessions` | `session:<id>`. */
    route: 'sessions',
    /** The ring the list is narrowed to; null shows every account. */
    filter: null,
    /** A row to point out (a banner click, an auto-open), selected too. */
    highlight: null,
    selected: null,
    /** The PanelRequest's reason (ring_click, hover_row, notification, auto, ...). */
    reason: null,
    /** {kind, tail, offset, width}: where the glue put the card (see normalizePlacement). */
    placement: { kind: 'floating', tail: 'none', offset: 0, width: 0 },
    /** The keyboard gate: the glue has confirmed the window is the foreground one. */
    focused: false,
    menuOpen: false,
    /** Values set here that the snapshot does not show yet: {key: {value, settledAt}}. */
    overrides: {},
    /** The gear menu's notification switches (Settings' notifications section); null until asked. */
    notify: { notifyNeedsInput: null, notifyReadyForReview: null },
    /** The sealed scene being shown, or null. */
    scene: null,
    pointer: false,
    /** The pointer is over the list: rows keep their order until it leaves. */
    hoverList: false,
    /** The row ids in the order last drawn: what the frozen order keeps. */
    displayed: [],
    /** What the user folded or unfolded: {bucket: true|false}; a bucket without an entry follows the engine. */
    folds: {},
    /** A mark-all-reviewed that can still be undone: {ids, at, timer}; its rows are shown as gone. */
    pending: null,
    /** A row to bring into view on the next render (a highlight). */
    scrollTo: null,
    field: false,
    engagedSent: false,
    reported: { w: 0, h: 0 },
    chatId: null,
  };

  var ACTIONS = Object.create(null);

  function clone(value) {
    return JSON.parse(JSON.stringify(value));
  }

  function fail(what) {
    return function (error) {
      C.log('panel: ' + what + ' failed: ' + ((error && error.code) || 'error'));
    };
  }

  function numberOr(value, fallback) {
    var n = Number(value);
    return isFinite(n) ? n : fallback;
  }

  /** `text` cut in the middle to `max` characters (a long address stays recognisable at both ends). */
  function middle(text, max) {
    var chars = Array.from(C.oneLine(text));
    if (chars.length <= max) return chars.join('');
    var keep = max - 1;
    var head = Math.ceil(keep / 2);
    return chars.slice(0, head).join('') + '…' + chars.slice(chars.length - (keep - head)).join('');
  }

  /** A title attribute: the whole text, but never a 10 000-character tooltip. */
  function tip(text) {
    return C.esc(Array.from(C.oneLine(text)).slice(0, 200).join(''));
  }

  // ---- placement -------------------------------------------------------------------------

  /**
   * The optional `placement` of a PanelRequest (not in the contract yet: DESIGN-WIN §5.3 has the
   * glue place the window, and the page needs to know where the notch is to draw the tail):
   * `{kind: 'beside' | 'above_or_below' | 'floating', tail_edge: 'left' | 'right' | 'top' |
   * 'bottom', tail_offset, width}`. `tail_edge` is the side of the card the tail is on, which is
   * also the way it points (a card left of a right-edge notch has its tail on its right).
   * `tail_offset` is the tail's offset from the card's centre along that side (positive is down
   * or right, as the Mac's); `width` is the card's width. Anything else is floating, no tail.
   */
  function normalizePlacement(raw) {
    var none = { kind: 'floating', tail: 'none', offset: 0, width: 0 };
    if (!raw || typeof raw !== 'object') return none;
    var edge = raw.tail_edge;
    var tail = 'none';
    if (raw.kind === 'beside' && (edge === 'left' || edge === 'right')) tail = edge;
    else if (raw.kind === 'above_or_below' && (edge === 'top' || edge === 'bottom')) tail = edge;
    if (tail === 'none') return none;
    var width = numberOr(raw.width, 0);
    return {
      kind: raw.kind,
      tail: tail,
      offset: Math.max(-2000, Math.min(2000, numberOr(raw.tail_offset, 0))),
      width: width > 0 && width <= 2000 ? width : 0,
    };
  }

  function applyPlacement() {
    var p = state.placement;
    els.panel.setAttribute('data-tail', p.tail);
    els.panel.setAttribute('data-placement', p.kind);
    els.frame.style.setProperty('--an-tail-offset', p.offset + 'px');
  }

  function mode() {
    return C.parseRoute(state.route).kind === 'session' ? 'chat' : 'list';
  }

  /** The card's width when the page has none measured: by placement and screen (UI§4.1). */
  function defaultWidth() {
    var p = state.placement;
    if (p.width) return p.width;
    return WIDTHS[p.kind === 'beside' ? 'beside' : 'flat'][mode() === 'chat' ? 1 : 0];
  }

  // ---- the snapshot, and the parts of it the header reads --------------------------------

  var ZERO = { needs_you: 0, failed: 0, review: 0, working: 0, idle: 0 };

  /** What the page draws: the snapshot, or the sealed scene's reading of it. */
  function view() {
    if (!state.snapshot) return null;
    var scene = state.scene && SCENES[state.scene];
    return scene ? scene(clone(state.snapshot)) : state.snapshot;
  }

  function ringsOf(v) {
    return v && Array.isArray(v.rings) ? v.rings : [];
  }

  function sessionsOf(v) {
    return v && Array.isArray(v.sessions) ? v.sessions : [];
  }

  /** The filter, when it still names a ring (a ring that went away leaves no chip to clear it). */
  function effectiveFilter(v) {
    if (!state.filter) return null;
    return ringsOf(v).some(function (r) { return r.ring_id === state.filter; }) ? state.filter : null;
  }

  function visibleSessions(v) {
    var filter = effectiveFilter(v);
    return sessionsOf(v).filter(function (s) { return !filter || s.ring_id === filter; });
  }

  /** The strip's numbers: the totals, or the filtered ring's own counts. */
  function countsOf(v) {
    var filter = effectiveFilter(v);
    var src = v && v.totals;
    if (filter) {
      var ring = ringsOf(v).filter(function (r) { return r.ring_id === filter; })[0];
      if (ring && ring.counts) src = ring.counts;
    }
    var out = {};
    Object.keys(ZERO).forEach(function (k) { out[k] = Math.max(0, Math.floor(numberOr(src && src[k], 0))); });
    out.total = out.needs_you + out.failed + out.review + out.working + out.idle;
    return out;
  }

  /** One chip per account: how many sessions it has and how many of them need you. */
  function chipsOf(v) {
    var all = sessionsOf(v);
    return ringsOf(v).map(function (ring) {
      var own = all.filter(function (s) { return s.ring_id === ring.ring_id; });
      return {
        ringId: String(ring.ring_id),
        label: String(ring.label == null ? '' : ring.label),
        colorIndex: numberOr(ring.color_index, 0),
        count: own.length,
        needs: own.filter(function (s) { return s.bucket === 'needs_you' && !s.failed; }).length,
      };
    });
  }

  function pinned(v) {
    var o = state.overrides.panelPinned;
    if (o) return !!o.value;
    return !!(v && v.ui && v.ui.panel_pinned);
  }

  function autoOpen(v) {
    var o = state.overrides.autoOpen;
    if (o) return o.value;
    return v && v.ui ? v.ui.panel_open_mode : 'never';
  }

  // ---- header ----------------------------------------------------------------------------

  function iconButton(action, icon, label, attrs, on) {
    return '<button type="button" class="an-icbtn' + (on ? ' an-on' : '') + '" data-an-action="' + action +
      '" data-key="' + action + '" title="' + C.esc(label) + '" aria-label="' + C.esc(label) + '"' + (attrs || '') +
      '>' + C.icon(icon) + '</button>';
  }

  function stripItem(kind, tone, text, breathKey) {
    return '<span class="an-strip-item an-tone-' + tone + '" data-an-text data-key="strip-' + kind + '">' +
      C.statusRing(kind, { breathKey: breathKey }) + C.esc(text) + '</span>';
  }

  /** "2 need you, 1 failed, 2 ready for review, 3 working, 4 idle" (AttentionStrip.accessibilityText). */
  function stripLabel(c) {
    var parts = [];
    if (c.needs_you) parts.push(C.needYou(c.needs_you));
    if (c.failed) parts.push(c.failed + ' failed');
    if (c.review) parts.push(c.review + ' ready for review');
    if (c.working) parts.push(c.working + ' working');
    if (c.idle) parts.push(c.idle + ' idle');
    return parts.length ? parts.join(', ') : 'No sessions';
  }

  function stripHtml(c) {
    if (!c.total) return '';
    var items = '';
    if (c.needs_you) items += stripItem('needs', 'needs', C.needYou(c.needs_you), c.needs_you);
    if (c.failed) items += stripItem('error', 'error', c.failed + ' failed');
    if (c.review) items += stripItem('review', 'review', c.review + ' to review');
    if (c.working) items += stripItem('working', 'working', c.working + ' working');
    if (c.idle) items += stripItem('idle', 'idle', c.idle + ' idle');
    return '<div class="an-strip" role="group" data-key="strip" aria-label="' + C.esc(stripLabel(c)) + '">' + items + '</div>';
  }

  /** "Work, 9 sessions, 1 needs you" (FilterChip.accessibilityLabel). */
  function chipLabel(label, count, needs) {
    var text = label + ', ' + C.plural(count, 'session');
    if (needs) text += ', ' + C.needYou(needs);
    return text;
  }

  function chipHtml(key, ringId, label, hue, count, needs, selected) {
    var shown = middle(label, CHIP_CHARS);
    return '<button type="button" class="an-chip' + (selected ? ' an-sel' : '') + '" data-key="chip-' + C.esc(key) +
      '" data-an-action="filter" data-an-arg="' + C.esc(ringId) + '" aria-pressed="' + (selected ? 'true' : 'false') +
      '" aria-label="' + tip(chipLabel(label, count, needs)) + '" title="' + tip(label) + '">' +
      (hue === null ? '' : C.accountDot(hue)) +
      '<span class="an-chip-l" data-an-text>' + C.esc(shown) + '</span>' +
      '<span class="an-chip-n">' + count + '</span>' +
      (needs ? '<span class="an-chip-need" aria-hidden="true"><span class="an-need-dot"></span>' + needs + '</span>' : '') +
      '</button>';
  }

  /** "All 13 · ● Personal 6 · ● Work 7": shown only with more than one account and some session. */
  function chipsHtml(v) {
    if (!v || !v.accounts_multi) return '';
    var chips = chipsOf(v);
    if (chips.length < 1 || !chips.some(function (c) { return c.count > 0; })) return '';
    var filter = effectiveFilter(v);
    var total = 0;
    var needs = 0;
    chips.forEach(function (c) { total += c.count; needs += c.needs; });
    var html = chipHtml('all', '', 'All', null, total, needs, !filter);
    chips.forEach(function (c) {
      html += chipHtml(c.ringId, c.ringId, c.label, c.colorIndex, c.count, c.needs, filter === c.ringId);
    });
    return '<div class="an-chips" role="group" data-key="chips" aria-label="Filter by account">' + html + '</div>';
  }

  function headerHtml(v) {
    var on = pinned(v);
    var pinLabel = on ? 'Keep open: on' : 'Keep open';
    var html = '<div class="an-title-row" data-key="title-row">' +
      '<h1 class="an-title" data-an-text>Claude sessions</h1>' +
      (v && v.sealed ? '<span class="an-sealed" data-an-text aria-label="Sealed mode: showing fixture data">Sealed</span>' : '') +
      '<span class="an-spacer"></span><div class="an-icons">' +
      iconButton('pin', on ? 'pinFill' : 'pin', pinLabel, ' aria-pressed="' + (on ? 'true' : 'false') + '"', on) +
      iconButton('gear', 'gear', 'Settings', ' aria-haspopup="menu" aria-expanded="' + (state.menuOpen ? 'true' : 'false') + '"', false) +
      iconButton('close', 'xmark', 'Close', '', false) +
      '</div></div>';
    html += stripHtml(countsOf(v));
    html += chipsHtml(v);
    return html;
  }

  // ---- the gear menu ---------------------------------------------------------------------

  function menuItem(role, checked, action, arg, label, extra) {
    return '<button type="button" class="an-mi" role="' + role + '"' +
      (checked === null ? '' : ' aria-checked="' + (checked ? 'true' : 'false') + '"') +
      ' data-key="mi-' + action + '-' + C.esc(arg) + '" data-an-action="' + action + '" data-an-arg="' + C.esc(arg) + '"' +
      (extra && extra.disabled ? ' disabled' : '') + '>' +
      (checked === null ? '' : '<span class="an-mc">' + C.icon('check') + '</span>') +
      '<span>' + C.esc(label) + '</span>' + (extra && extra.hint ? '<span class="an-mk">' + C.esc(extra.hint) + '</span>' : '') +
      '</button>';
  }

  /** The Mac's gear menu: open automatically, two notify switches, mark all reviewed, Settings. */
  function overlayHtml(v) {
    if (!state.menuOpen) return '';
    var mode = autoOpen(v);
    var html = '<div class="an-menu" role="menu" aria-label="Settings" data-key="menu">' +
      '<div class="an-mh" role="presentation">Open automatically</div>';
    AUTO_OPEN.forEach(function (o) {
      html += menuItem('menuitemradio', o[0] === mode, 'menu-auto', o[0], o[1]);
    });
    html += '<div class="an-msep" role="separator" data-key="sep-1"></div>';
    NOTIFY.forEach(function (o) {
      html += menuItem('menuitemcheckbox', state.notify[o[0]] === true, 'menu-notify', o[0], o[1]);
    });
    html += '<div class="an-msep" role="separator" data-key="sep-2"></div>' +
      menuItem('menuitem', null, 'mark-all-reviewed', '', 'Mark all reviewed', { disabled: !countsOf(v).review, hint: 'Ctrl+Shift+R' }) +
      '<div class="an-msep" role="separator" data-key="sep-3"></div>' +
      menuItem('menuitem', null, 'open-settings', '', 'Open Settings…') +
      '</div>';
    return html;
  }

  function menuItems() {
    return Array.prototype.slice.call(els.overlay.querySelectorAll('.an-mi')).filter(function (el) {
      return !el.hasAttribute('disabled');
    });
  }

  function openMenu() {
    if (state.menuOpen) return;
    state.menuOpen = true;
    render();
    if (!C.isStatic()) {
      var first = menuItems()[0];
      if (first) first.focus();
    }
    loadNotify();
  }

  /** The notification switches live in Settings' section, not in the panel's snapshot: ask for them. */
  function loadNotify() {
    C.call('settings').then(function (s) {
      var n = s && s.notifications;
      if (!n) return;
      NOTIFY.forEach(function (o) {
        if (typeof n[o[2]] === 'boolean') state.notify[o[0]] = n[o[2]];
      });
      render();
    }, fail('settings'));
  }

  function closeMenu(restoreFocus) {
    if (!state.menuOpen) return;
    var hadFocus = els.overlay.contains(document.activeElement);
    state.menuOpen = false;
    render();
    if (restoreFocus && hadFocus) {
      var gear = els.header.querySelector('[data-an-action="gear"]');
      if (gear) gear.focus();
    }
  }

  // ---- settings written from the panel ---------------------------------------------------

  /** Shows `value` at once and sends it; the reply (or a refusal, e.g. sealed) settles it. */
  function changeSetting(key, value) {
    state.overrides[key] = { value: value, settledAt: null };
    render();
    C.call('set_setting', { key: key, value: value }).then(function () {
      var o = state.overrides[key];
      if (o && o.value === value) o.settledAt = C.now();
    }, function (error) {
      delete state.overrides[key];
      fail('set_setting ' + key)(error);
      render();
    });
  }

  /** A snapshot that shows the value, or that arrives after the reply's grace, ends the override. */
  function dropOverrides(v) {
    Object.keys(state.overrides).forEach(function (key) {
      var o = state.overrides[key];
      var shown = key === 'panelPinned' ? v.ui && v.ui.panel_pinned : key === 'autoOpen' ? v.ui && v.ui.panel_open_mode : undefined;
      if (shown === o.value || (o.settledAt !== null && C.now() - o.settledAt > OVERRIDE_MS)) delete state.overrides[key];
    });
  }

  // ---- the other regions (later sub-tasks fill these) ------------------------------------

  /** The setup banners (consent card, scope notice, pipe error, control off, hooks missing). */
  function bannersHtml() {
    return '';
  }

  /** The undo toast, while a mark-all-reviewed can still be taken back. */
  function toastHtml() {
    return L.toastHtml(state.pending);
  }

  function emptyHtml(v) {
    if (state.error) {
      return '<div class="an-empty" data-key="empty">' + C.statusRing('idle') +
        '<div class="an-empty-title" data-an-text>Couldn’t load the sessions</div>' +
        '<div class="an-empty-text" data-an-text>' + C.esc(state.error) + '</div></div>';
    }
    var narrowed = !!effectiveFilter(v) && sessionsOf(v).length > 0;
    return '<div class="an-empty" data-key="empty">' + C.statusRing('idle') +
      '<div class="an-empty-title" data-an-text>' + (narrowed ? 'No sessions in this account' : 'No Claude sessions yet') + '</div>' +
      '<div class="an-empty-text" data-an-text>' + (narrowed
        ? 'Choose All to see every account’s sessions.'
        : 'Start Claude Code in VS Code or a terminal. Sessions from every account show up here.') + '</div></div>';
  }

  // ---- the list ---------------------------------------------------------------------------

  /** A row of a pending mark-all-reviewed shows as gone, unless it finished again since the click. */
  function hiddenRow(row) {
    var p = state.pending;
    if (!p || p.ids.indexOf(row.session_id) < 0) return false;
    return !(row.since_ms > p.at) || row.bucket !== 'ready_for_review';
  }

  /** The sections and rows for the current snapshot and view state (see agentnotchPanelList.layout). */
  function listLayout(v) {
    return L.layout(v, {
      filter: effectiveFilter(v),
      folds: state.folds,
      frozen: state.hoverList ? state.displayed : null,
      hidden: hiddenRow,
    });
  }

  /** Where a row's action bar goes: sub-task 6 sets `agentnotchPanel.slots.actions(row, ctx) -> html`. */
  var SLOTS = { actions: null };

  /** The list region: the sections and rows, or an empty state. Nothing before the first snapshot. */
  function listHtml(v) {
    if (!v) return state.error ? emptyHtml(v) : '';
    var lay = listLayout(v);
    if (lay.isEmpty) return emptyHtml(v);
    state.displayed = lay.order;
    return L.html(lay, {
      selected: state.selected,
      multi: !!v.accounts_multi,
      filtered: !!effectiveFilter(v),
      now: C.now(),
      actions: SLOTS.actions,
    });
  }

  // ---- render ----------------------------------------------------------------------------

  function render() {
    if (!started) return;
    var v = view();
    C.morph(els.header, headerHtml(v));
    C.morph(els.banners, bannersHtml(v));
    var before = C.flipFirst(els.rows);
    C.morph(els.rows, listHtml(v));
    C.flipPlay(els.rows, before);
    C.morph(els.toast, toastHtml(v));
    scrollToRow();
    C.morph(els.overlay, overlayHtml(v));
    applyMode();
    makeRoomForMenu();
    reportSize();
  }

  /** A highlighted row (a banner click, an auto-open) is brought into view once it is drawn. */
  function scrollToRow() {
    if (!state.scrollTo) return;
    var id = state.scrollTo;
    var row = Array.prototype.slice.call(els.rows.querySelectorAll('[data-id]')).filter(function (el) {
      return el.getAttribute('data-id') === id;
    })[0];
    if (!row) return;
    state.scrollTo = null;
    if (typeof row.scrollIntoView === 'function') row.scrollIntoView({ block: 'nearest' });
  }

  /** A menu opened over a short card (the empty list) makes the card tall enough to hold it. */
  function makeRoomForMenu() {
    var menu = els.overlay.querySelector('.an-menu');
    els.card.style.minHeight = menu && menu.offsetHeight ? Math.ceil((menu.offsetTop || 0) + menu.offsetHeight + 8) + 'px' : '';
  }

  /** The list, or one session's chat: chat.js gets the region while the route is a session. */
  function applyMode() {
    var m = mode();
    els.card.setAttribute('data-mode', m);
    var id = m === 'chat' ? C.parseRoute(state.route).id : null;
    if (m === 'chat') els.chat.removeAttribute('hidden');
    else els.chat.setAttribute('hidden', '');
    if (id === state.chatId) return;
    var chat = window.agentnotchChat;
    if (state.chatId !== null && chat && typeof chat.unmount === 'function') chat.unmount();
    state.chatId = id;
    if (id !== null && chat && typeof chat.mount === 'function') chat.mount(els.chat, { sessionId: id, panel: window.agentnotchPanel });
  }

  // ---- the size the window follows -------------------------------------------------------

  /**
   * The card's natural height: what its parts want, however tall the window is now. The list is
   * the one part that scrolls, so it counts by its content (scrollHeight), the rest by their box;
   * an open menu counts too (it must not be cut off by a short card).
   */
  function naturalHeight() {
    var total = 0;
    Array.prototype.forEach.call(els.card.children, function (child) {
      if (child === els.overlay || child.hasAttribute('hidden')) return;
      total += child === els.list ? child.scrollHeight : child.offsetHeight;
    });
    var menu = els.overlay.querySelector('.an-menu');
    if (menu && menu.offsetHeight) total = Math.max(total, (menu.offsetTop || 0) + menu.offsetHeight + 8);
    return total;
  }

  /** `panel_report_size {w, h}` (CSS px of the card) whenever the content's height or the width changes. */
  function reportSize() {
    var h = Math.ceil(naturalHeight());
    if (!(h > 0)) return;
    var w = Math.ceil(els.card.offsetWidth) || defaultWidth();
    if (w === state.reported.w && h === state.reported.h) return;
    state.reported = { w: w, h: h };
    C.call('panel_report_size', { w: w, h: h }).catch(fail('panel_report_size'));
  }

  // ---- mark all reviewed, with undo ------------------------------------------------------

  /**
   * Every row of Ready for review (the ones the filter shows) leaves at once and is marked for
   * real after UNDO_MS, or when the panel closes; Undo puts them back and sends nothing. The call
   * carries the click's time, so a turn that finishes in those seconds stays unreviewed (BHV-9).
   * Another mark-all sends the one before it first. A sealed scene never sends.
   */
  function markAllReviewed() {
    var v = view();
    if (!v || state.route !== 'sessions') return;
    var section = listLayout(v).sections.filter(function (s) { return s.bucket === 'ready_for_review'; })[0];
    if (!section) return;
    var ids = section.rows.filter(function (r) { return !r.failed; }).map(function (r) { return String(r.session_id); });
    if (!ids.length) return;
    commitPending();
    var at = C.now();
    state.pending = { ids: ids, at: at, timer: null };
    if (!state.scene) {
      var pending = state.pending;
      pending.timer = window.setTimeout(function () {
        if (state.pending === pending) commitPending();
      }, UNDO_MS);
    }
    render();
  }

  /** Sends the pending review now (the window ended, another began, or the panel closes). */
  function commitPending() {
    var p = state.pending;
    if (!p) return;
    if (p.timer !== null) window.clearTimeout(p.timer);
    state.pending = null;
    if (!state.scene) C.call('mark_all_reviewed', { session_ids: p.ids, at_ms: p.at }).catch(fail('mark_all_reviewed'));
    render();
  }

  /** Puts the rows back in Ready for review; nothing was sent. */
  function undoReview() {
    var p = state.pending;
    if (!p) return;
    if (p.timer !== null) window.clearTimeout(p.timer);
    state.pending = null;
    render();
  }

  // ---- navigation and actions ------------------------------------------------------------

  /** Moves to `sessions` or `session:<id>`; the glue is told (`panel_route`) only of a real change. */
  function navigate(route) {
    var p = C.parseRoute(route);
    var next = p.kind === 'session' && p.id ? 'session:' + p.id : 'sessions';
    if (next === state.route) return;
    state.route = next;
    state.menuOpen = false;
    if (p.kind === 'session' && p.id) state.selected = p.id;
    render();
    C.call('panel_route', { route: next }).catch(fail('panel_route'));
  }

  function closePanel() {
    commitPending();
    C.call('panel_close').catch(fail('panel_close'));
  }

  /** Esc, one step out: the menu, then the chat, then the list, then the panel. */
  function escape() {
    if (state.menuOpen) closeMenu(true);
    else if (mode() === 'chat') navigate('sessions');
    else closePanel();
  }

  ACTIONS.pin = function () {
    changeSetting('panelPinned', !pinned(view()));
  };
  ACTIONS.gear = function () {
    if (state.menuOpen) closeMenu(true);
    else openMenu();
  };
  ACTIONS.close = closePanel;
  ACTIONS.filter = function (ringId) {
    state.filter = !ringId || ringId === state.filter ? null : ringId;
    render();
  };
  ACTIONS['menu-auto'] = function (value) {
    if (AUTO_OPEN.some(function (o) { return o[0] === value; })) changeSetting('autoOpen', value);
  };
  ACTIONS['menu-notify'] = function (key) {
    if (!NOTIFY.some(function (o) { return o[0] === key; })) return;
    var next = !state.notify[key];
    state.notify[key] = next;
    C.call('set_setting', { key: key, value: next }).catch(function (error) {
      state.notify[key] = !next;
      fail('set_setting ' + key)(error);
      render();
    });
  };
  // -- the list's actions (delegated from the rows: data-an-action + data-an-arg) --

  ACTIONS['open-chat'] = function (id) {
    if (id) navigate('session:' + id);
  };
  ACTIONS.fold = function (bucket) {
    if (!bucket || bucket === 'needs_you') return;
    var v = view();
    var section = v ? listLayout(v).sections.filter(function (s) { return s.bucket === bucket; })[0] : null;
    if (!section) return;
    state.folds[bucket] = !section.collapsed;
    render();
  };
  ACTIONS.jump = function (id) {
    if (id) C.call('focus', { session_id: id }).catch(fail('focus'));
  };
  ACTIONS['mark-reviewed'] = function (id) {
    if (id) C.call('mark_reviewed', { session_id: id, at_ms: C.now() }).catch(fail('mark_reviewed'));
  };
  ACTIONS['dismiss-failure'] = function (id) {
    if (id) C.call('dismiss_failure', { session_id: id }).catch(fail('dismiss_failure'));
  };
  ACTIONS['mark-all-reviewed'] = markAllReviewed;
  ACTIONS['undo-review'] = undoReview;
  ACTIONS['open-settings'] = function () {
    C.call('open_settings', { tab: 'claude' }).catch(fail('open_settings'));
  };

  function onCardClick(event) {
    var target = event.target && event.target.closest ? event.target.closest('[data-an-action]') : null;
    if (!target || !els.card.contains(target)) return;
    if (target.hasAttribute('disabled') || target.getAttribute('aria-disabled') === 'true') return;
    if (target.closest('.an-menu') && state.menuOpen) {
      state.menuOpen = false;
      render();
    }
    var name = target.getAttribute('data-an-action');
    if (!Object.prototype.hasOwnProperty.call(ACTIONS, name)) return;
    event.preventDefault();
    ACTIONS[name](target.getAttribute('data-an-arg'), target, event);
  }

  // ---- keys ------------------------------------------------------------------------------

  function onKeyDown(event) {
    // WebView2's own shortcuts (reload, print, find, downloads) never reach the panel.
    if (C.isBrowserAccelerator(event)) event.preventDefault();
    if (event.defaultPrevented && event.key === 'Escape') return;
    if (event.key === 'Escape') {
      event.preventDefault();
      escape();
      return;
    }
    if (state.menuOpen && (event.key === 'ArrowDown' || event.key === 'ArrowUp' || event.key === 'Home' || event.key === 'End')) {
      var items = menuItems();
      if (!items.length) return;
      event.preventDefault();
      var at = items.indexOf(document.activeElement);
      var next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
        : event.key === 'ArrowDown' ? (at + 1) % items.length : (at <= 0 ? items.length - 1 : at - 1);
      items[next].focus();
    }
  }

  function onPointerDown(event) {
    if (!state.menuOpen) return;
    var t = event.target;
    if (t && t.closest && (t.closest('.an-menu') || t.closest('[data-an-action="gear"]'))) return;
    closeMenu(false);
  }

  function isTextField(el) {
    if (!el || !el.tagName) return false;
    var tag = el.tagName;
    if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
    if (tag === 'INPUT') return !/^(checkbox|radio|button|submit|reset|range|color|file|image)$/i.test(el.getAttribute('type') || '');
    return el.getAttribute && el.getAttribute('contenteditable') === 'true';
  }

  /** `panel_engaged {on}`: the pointer is over the card or a field has focus (an auto-open then stays). */
  function setEngaged() {
    var on = state.pointer || state.field;
    if (on === state.engagedSent) return;
    state.engagedSent = on;
    C.call('panel_engaged', { on: on }).catch(fail('panel_engaged'));
  }

  // ---- events from the glue --------------------------------------------------------------

  function applySnapshot(s) {
    if (!s || typeof s !== 'object') return;
    var at = numberOr(s.generated_at_ms, 0);
    if (state.snapshot && at < state.at) return;
    state.snapshot = s;
    state.at = at;
    state.error = null;
    // A row pointed at by the first request (it came before the first snapshot) opens its section.
    if (state.scrollTo) unfoldFor(state.scrollTo);
    dropOverrides(s);
    render();
  }

  /** A PanelRequest: the first (window.__AGENTNOTCH_PANEL__) or a later open (`an:panel`). */
  function applyRequest(req) {
    if (!req || typeof req !== 'object') return;
    var p = C.parseRoute(typeof req.route === 'string' ? req.route : 'sessions');
    state.scene = null;
    state.route = p.kind === 'session' && p.id ? 'session:' + p.id : 'sessions';
    // A list route names its filter every time, so a banner re-opening "all sessions" over a
    // narrowed list shows the row it points at (ClaudePanelState.routeChanged).
    if (mode() === 'list') state.filter = typeof req.ring_id === 'string' && req.ring_id ? req.ring_id : null;
    state.highlight = typeof req.highlight === 'string' && req.highlight ? req.highlight : null;
    if (state.highlight) {
      state.selected = state.highlight;
      state.scrollTo = state.highlight;
      unfoldFor(state.highlight);
    }
    if (mode() === 'chat') state.selected = p.id;
    state.reason = typeof req.reason === 'string' ? req.reason : null;
    state.placement = normalizePlacement(req.placement);
    state.menuOpen = false;
    applyPlacement();
    render();
  }

  /** A row pointed at inside a folded section opens that section (SessionListLayout.foldedBucket). */
  function unfoldFor(id) {
    var v = state.snapshot;
    var row = v ? sessionsOf(v).filter(function (s) { return s.session_id === id; })[0] : null;
    if (row) state.folds[L.bucketOf(row)] = false;
  }

  function applyFocus(payload) {
    state.focused = !!(payload && payload.focused);
    document.body.classList.toggle('an-focused', state.focused);
  }

  function applyTheme(name) {
    if (name !== 'light' && name !== 'dark') return;
    document.documentElement.setAttribute('data-theme', name);
  }

  // ---- sealed scenes (§7.4) and the layout invariants (§5.10) ----------------------------

  function emptied(s) {
    s.sessions = [];
    s.sections = [];
    s.totals = clone(ZERO);
    ringsOf(s).forEach(function (r) {
      r.counts = clone(ZERO);
      r.badges = { needs_you: 0, review: 0 };
    });
    return s;
  }

  function same(s) {
    return s;
  }

  /** The counts the header and the sections carry, recomputed from the rows a scene changed. */
  function retally(s) {
    var totals = clone(ZERO);
    var byRing = Object.create(null);
    var sections = [];
    var per = Object.create(null);
    s.sessions.forEach(function (r) {
      var key = r.failed ? 'failed' : r.bucket === 'needs_you' ? 'needs_you' : r.bucket === 'ready_for_review' ? 'review' : r.bucket;
      totals[key] = (totals[key] || 0) + 1;
      var ring = byRing[r.ring_id] || (byRing[r.ring_id] = clone(ZERO));
      ring[key] = (ring[key] || 0) + 1;
      per[r.bucket] = (per[r.bucket] || 0) + 1;
    });
    L.BUCKETS.forEach(function (b) {
      if (!per[b]) return;
      sections.push({ bucket: b, title: b === 'needs_you' ? 'Needs you' : b === 'ready_for_review' ? 'Ready for review' : b === 'working' ? 'Working' : 'Idle',
        count: per[b], fold_by_default: b === 'idle' && per[b] > L.IDLE_FOLD_ABOVE });
    });
    s.totals = totals;
    s.sections = sections;
    ringsOf(s).forEach(function (r) {
      var own = byRing[r.ring_id] || clone(ZERO);
      r.counts = own;
      r.badges = { needs_you: own.needs_you + own.failed, review: own.review };
    });
    return s;
  }

  /**
   * `n` sessions for a busy list: the fixture's rows cycled per section with new ids and titles
   * (`Sweep the repo (2)`), in the Mac's proportions (a few needing you, a few done, mostly working).
   */
  function busy(s) {
    var plan = [['needs_you', 3], ['ready_for_review', 6], ['working', 10], ['idle', 6]];
    var made = [];
    plan.forEach(function (entry) {
      var pool = s.sessions.filter(function (r) { return r.bucket === entry[0]; });
      if (!pool.length) return;
      for (var i = 0; i < entry[1]; i++) {
        var row = clone(pool[i % pool.length]);
        if (i >= pool.length) {
          row.session_id = row.session_id + '-' + (Math.floor(i / pool.length) + 1);
          row.title = row.title + ' (' + (Math.floor(i / pool.length) + 1) + ')';
          row.since_ms = row.since_ms - Math.floor(i / pool.length) * 97000;
        }
        made.push(row);
      }
    });
    s.sessions = made;
    return retally(s);
  }

  /** Only what one screenshot shows: the sessions whose ids are listed, in the given buckets' order. */
  function only(ids) {
    return function (s) {
      s.sessions = s.sessions.filter(function (r) { return ids.indexOf(r.session_id) >= 0; });
      return retally(s);
    };
  }

  /** Scene name -> how it reads the current snapshot. Later sub-tasks add their scenes here. */
  var SCENES = {
    'panel-empty': emptied,
    'panel-every-state': same,
    'panel-header': same,
    'panel-menu': same,
    'panel-filtered': same,
    'panel-undo': same,
    'panel-keyboard-folded': same,
    'panel-regular-rows': only(['needs-question', 'needs-permission', 'needs-ratelimit', 'review-darkmode', 'work-migration', 'work-ci', 'work-summary', 'idle-notch']),
    'panel-busy-window': busy,
    'panel-busy-full': busy,
    'panel-pinned': function (s) {
      s.ui = s.ui || {};
      s.ui.panel_pinned = true;
      return s;
    },
    'panel-single-account': function (s) {
      var first = ringsOf(s)[0];
      s.accounts_multi = false;
      s.rings = first ? [first] : [];
      return s;
    },
  };

  /** View state a scene needs besides the snapshot: the selection, folds, a pending review. */
  var SCENE_STATE = {
    'panel-undo': function () {
      var v = view();
      var ids = v ? sessionsOf(v).filter(function (r) { return r.bucket === 'ready_for_review'; }).slice(0, 2).map(function (r) { return String(r.session_id); }) : [];
      if (ids.length) state.pending = { ids: ids, at: C.now(), timer: null };
    },
    'panel-keyboard-folded': function () {
      state.selected = 'needs-question';
      state.folds.working = true;
      state.folds.idle = true;
    },
  };

  /** Shows the named state from the snapshot the page already holds; false for a name it lacks. */
  function showScene(name) {
    if (!C || !Object.prototype.hasOwnProperty.call(SCENES, name) || !state.snapshot) return false;
    C.setStatic(true);
    state.scene = name;
    state.route = 'sessions';
    state.filter = null;
    state.selected = null;
    state.folds = {};
    if (state.pending && state.pending.timer !== null) window.clearTimeout(state.pending.timer);
    state.pending = null;
    state.menuOpen = name === 'panel-menu';
    if (state.menuOpen) loadNotify();
    if (name === 'panel-filtered') {
      var ring = ringsOf(state.snapshot)[1] || ringsOf(state.snapshot)[0];
      state.filter = ring ? ring.ring_id : null;
    }
    if (SCENE_STATE[name]) SCENE_STATE[name]();
    render();
    return true;
  }

  /** A title or detail that is cut ON PURPOSE (`data-an-clip`) and shows an ellipsis is not a clipped text. */
  function cutWithEllipsis(el) {
    if (!el.hasAttribute('data-an-clip')) return false;
    var style = window.getComputedStyle ? window.getComputedStyle(el) : null;
    return !!style && style.textOverflow === 'ellipsis';
  }

  function rectInside(inner, outer, slack) {
    var s = slack || 0;
    return inner.left >= outer.left - s && inner.top >= outer.top - s &&
      inner.right <= outer.right + s && inner.bottom <= outer.bottom + s;
  }

  /**
   * The panel's layout invariants for the sealed self-test: no [data-an-text] element wider than
   * its box (at 400 and 520 px), every control inside the card, the card and its tail inside the
   * window. Measured in the browser: the node DOM has no layout, so the rectangles there are zero
   * and only what a test sets is checked.
   */
  function layoutReport() {
    var failures = [];
    var cr = els.card.getBoundingClientRect();
    var texts = 0;
    Array.prototype.forEach.call(document.querySelectorAll('[data-an-text]'), function (el) {
      texts += 1;
      if (el.scrollWidth > el.clientWidth + 1 && !cutWithEllipsis(el)) failures.push('text is clipped: ' + C.oneLine(el.textContent).slice(0, 40));
    });
    var controls = els.card.querySelectorAll('button, [data-an-action], input, textarea, select');
    Array.prototype.forEach.call(controls, function (el) {
      var r = el.getBoundingClientRect();
      // A scrolling list has rows below the fold: they may lie below the card, never beside it.
      var scrolled = els.list.contains(el) && els.list.scrollHeight > els.list.clientHeight + 1;
      var box = scrolled ? { left: cr.left, right: cr.right, top: -1e9, bottom: 1e9 } : cr;
      if (r.width && cr.width && !rectInside(r, box, 0.5)) {
        failures.push('control outside the card: ' + (el.getAttribute('aria-label') || C.oneLine(el.textContent)).slice(0, 40));
      }
    });
    var win = { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
    if (cr.width && !rectInside(cr, win, 0.5)) failures.push('the card leaves the window');
    var tail = els.tail.getBoundingClientRect();
    if (state.placement.tail !== 'none' && tail.width) {
      if (!rectInside(tail, win, 0.5)) failures.push('the tail leaves the window');
      var t = state.placement.tail;
      var joined = t === 'left' ? Math.abs(tail.right - 1 - cr.left) < 1.5 : t === 'right' ? Math.abs(tail.left + 1 - cr.right) < 1.5
        : t === 'top' ? Math.abs(tail.bottom - 1 - cr.top) < 1.5 : Math.abs(tail.top + 1 - cr.bottom) < 1.5;
      if (!joined) failures.push('the tail does not meet the card');
    }
    return {
      ok: failures.length === 0,
      failures: failures,
      route: state.route,
      tail: state.placement.tail,
      texts: texts,
      card: { width: cr.width, height: cr.height, scrolls: els.list.scrollHeight > els.list.clientHeight + 1 },
    };
  }

  // ---- start -----------------------------------------------------------------------------

  function start() {
    if (started) return;
    C = window.agentnotchCommon;
    L = window.agentnotchPanelList;
    if (!C || !L) return;
    els = {
      panel: document.getElementById('an-panel'),
      top: document.getElementById('an-top'),
      frame: document.getElementById('an-frame'),
      tail: document.getElementById('an-tail'),
      card: document.getElementById('an-card'),
      header: document.getElementById('an-header'),
      banners: document.getElementById('an-banners'),
      list: document.getElementById('an-list'),
      rows: document.getElementById('an-rows'),
      toast: document.getElementById('an-toast'),
      chat: document.getElementById('an-chat'),
      overlay: document.getElementById('an-overlay'),
    };
    if (!els.panel || !els.card || !els.header || !els.rows || !els.overlay) return;
    started = true;

    els.card.addEventListener('click', onCardClick);
    els.card.addEventListener('pointerenter', function () { state.pointer = true; setEngaged(); });
    els.card.addEventListener('pointerleave', function () { state.pointer = false; setEngaged(); });
    els.card.addEventListener('focusin', function (e) { state.field = isTextField(e.target); setEngaged(); });
    els.card.addEventListener('focusout', function () { state.field = false; setEngaged(); });
    // Rows keep their order while the pointer is over the list, and move (glide) when it leaves.
    els.list.addEventListener('pointerenter', function () { state.hoverList = true; });
    els.list.addEventListener('pointerleave', function () {
      state.hoverList = false;
      render();
    });
    // A hidden window can no longer be undone from: send what is pending.
    document.addEventListener('visibilitychange', function () { if (document.hidden) commitPending(); });
    window.addEventListener('pagehide', commitPending);
    window.setInterval(function () {
      if (!C.isStatic() && state.snapshot && mode() === 'list') render();
    }, TICK_MS);
    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('pointerdown', onPointerDown);

    // The glue puts the theme in before the first script; a page opened without it asks.
    if (!document.documentElement.getAttribute('data-theme')) {
      C.invokeRaw('get_theme_resolved').then(applyTheme, function () {});
    }
    C.listen('theme_resolved', applyTheme);
    C.listen('an:snapshot', applySnapshot);
    C.listen('an:panel', applyRequest);
    C.listen('an:panel_focus', applyFocus);

    // The first request is on the window; without one the panel is a floating list.
    applyRequest(window.__AGENTNOTCH_PANEL__);
    render();

    if (typeof window.ResizeObserver === 'function') {
      var observer = new window.ResizeObserver(function () { reportSize(); });
      [els.top, els.rows, els.toast, els.chat, els.card].forEach(function (el) {
        if (el) observer.observe(el);
      });
    }

    C.call('snapshot').then(applySnapshot, function (error) {
      state.error = (error && error.message) || "Agent Notch's engine isn't reachable.";
      render();
    });
  }

  window.agentnotchPanel = {
    showScene: showScene,
    layoutReport: layoutReport,
    navigate: navigate,
    close: closePanel,
    /** For the later sub-tasks and the tests: the state and the tables they extend. */
    actions: ACTIONS,
    scenes: SCENES,
    /** Named places sub-task 6 fills: `slots.actions = function (row, ctx) { return html; }` (the row's action bar). */
    slots: SLOTS,
    undoReview: undoReview,
    markAllReviewed: markAllReviewed,
    _: {
      state: state,
      render: render,
      view: view,
      normalizePlacement: normalizePlacement,
      naturalHeight: naturalHeight,
      middle: middle,
      countsOf: countsOf,
      chipsOf: chipsOf,
      visibleSessions: visibleSessions,
      escape: escape,
      listLayout: listLayout,
      commitPending: commitPending,
    },
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();
