// The sessions panel's page (DESIGN-WIN §5.3, UI§4-5): the shell every screen of the panel sits in.
// It owns the card and its tail, the header (title, Sealed badge, pin, gear menu, close, the
// attention strip and the account chips), the route (list or one session's chat), the keyboard
// gate's state, Esc, the size report and the sealed scenes. The list (sections, rows, folding,
// the undo toast) is panel-list.js's markup driven from here; the chat is chat.js's.
//
// This file is also the part of the panel that ACTS: the row action bars (Deny / Always / Allow,
// the question chips, Review plan / Approve), the keyboard, and the consent card that lets the
// app write settings.json. Three rules hold everywhere below, and the tests pin each:
//   * every answer goes through ONE AnswerGate (agentnotchCommon.createAnswerGate): a request can
//     be answered once, and only after it has been on screen for 350 ms, so a click or a key
//     meant for the request it replaced never lands on it;
//   * the keyboard gate: no shortcut does anything and no field takes a key until the glue says
//     the panel is the foreground window (`an:panel_focus {focused:true}`). A DOM focus event
//     never opens it. Keys that arrive while it is shut are dropped, never queued;
//   * consent: `hook_consent {grant:true}` is sent by a click on "Turn on" and by nothing else.
//     No key reaches it (Enter on it is swallowed), it is never focused, never a default.
//
// How it is built, so the next sub-task can add to it without rewriting it:
//   * ONE state object (`state`) and ONE render(): it draws the whole page from the last whole
//     snapshot plus the view state (route, filter, menu, overrides) into fixed regions with
//     agentnotchCommon.morph, so a snapshot never drops focus, an open menu or a half-typed answer.
//       #an-header  -> headerHtml(v)      title row, attention strip, account chips
//       #an-banners -> bannersHtml(v)     setup banners (consent card, scope notice, pipe error, ...)
//       #an-rows    -> listHtml(v)        the sections and rows (agentnotchPanelList); the empty states
//       #an-toast   -> toastHtml(v)       the undo toast, and a notice when an answer did not arrive
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

  /** The chat asks for at most this much height (the list's cap is the glue's: the window is clamped there). */
  var CHAT_HEIGHT_CAP = 780;
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
  /** How long "That request was already answered" stays. */
  var NOTICE_MS = 6000;
  /** What is kept of an untrusted string in a banner (the CSS wraps the rest). */
  var BANNER_CHARS = 400;
  var PATH_CHARS = 300;

  /**
   * The official app, named on purpose: its own hooks run beside ours, and the user knows it by
   * this name (DESIGN-WIN §1.8). The one place the fork's panel says it (check-seams NAME_OK).
   */
  var OFFICIAL_APP = 'Codenotch';

  /** Word for word the Mac's (ConsentCopy, HookHealth), with the Windows substitutions. */
  var COPY = {
    consentTitle: 'Turn on Claude Code control',
    consentText: 'To show every session live and let you answer prompts from here, this app adds its hooks and a status-line wrapper to the settings.json of each folder Claude Code runs in. Nothing is written until you turn it on, and turning it off puts your status line back exactly.',
    // The official Codenotch's hooks run beside ours and are never removed by "Turn on".
    codenotchStays: OFFICIAL_APP + '’s hooks stay; remove them per folder in Settings › Claude Code.',
    installOff: 'Installing is off for this run (--no-install).',
    scopeTitle: 'Claude Code control now covers your VS Code workspaces',
    pipeTitle: 'Not receiving hook events',
    pipeTail: 'Sessions still update from Claude Code’s session files, without approvals.',
    offTitle: 'Claude Code control is off',
    offText: 'Answer prompts where Claude Code runs (VS Code or the terminal). Sessions still show here and finish from their transcripts.',
    codenotchTitle: OFFICIAL_APP + '’s hooks are installed too',
    installOffText: 'Nothing is written to any settings.json in this run; hooks that are already installed keep working.',
    tooLong: 'Too long to judge from here: review it whole first.',
    notPending: 'That request was already answered or is no longer waiting.',
    peerGone: 'The session went away before the answer arrived.',
    notSent: 'The answer didn’t reach Claude Code. Answer it where Claude Code runs.',
  };

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
    /** "Turn on" or "Not now" was clicked here: the card goes at once, before the engine says so. */
    consentAnswered: false,
    /** OK or Turn off on the scope notice was clicked here. */
    scopeAnswered: false,
    /** A line about an answer that did not arrive: {text, timer}; shown where the toast is. */
    notice: null,
    /** The panel was opened (again): requests on screen start their wait over. */
    rearm: false,
    engagedSent: false,
    reported: { w: 0, h: 0 },
    chatId: null,
  };

  var ACTIONS = Object.create(null);
  /** The page's one AnswerGate (made at start), and the timer that redraws a bar when it arms. */
  var gate = null;
  var armTimer = null;
  /** The screen the gate's requests were last noted for (`list` | `chat`). */
  var gateMode = 'list';

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

  function cut(text, max) {
    var chars = Array.from(text == null ? '' : String(text));
    return chars.length <= max ? chars.join('') : chars.slice(0, max - 1).join('') + '…';
  }

  function strings(list, max) {
    return (Array.isArray(list) ? list : []).filter(function (x) { return typeof x === 'string' && x; })
      .map(function (x) { return cut(C.oneLine(x), max); });
  }

  function setupOf(v) {
    return v && v.setup && typeof v.setup === 'object' ? v.setup : {};
  }

  /** The consent card shows: the engine asks for consent and nobody answered it in this panel. */
  function consentShown(v) {
    return !!setupOf(v).needs_hook_consent && !state.consentAnswered;
  }

  /** A pill button (ClaudeButtonStyle): kind primary | secondary | tinted | quiet. */
  function pill(kind, compact, label, attrs) {
    return '<button type="button" class="an-btn an-btn-' + kind + (compact ? ' an-compact' : '') + '"' + (attrs || '') +
      '><span data-an-text>' + C.esc(label) + '</span></button>';
  }

  function bannerButton(action, label, key) {
    return pill('secondary', true, label, ' data-key="' + key + '" data-an-action="' + action + '"');
  }

  /** One thing to know (NoticeBanner): a coloured mark, a line, an explanation, its actions. */
  function noticeBanner(id, icon, tone, title, message, actions) {
    return '<div class="an-banner" data-key="b-' + id + '" role="group" aria-label="' + tip(title) + '">' +
      '<span class="an-banner-ic an-tone-' + tone + '" aria-hidden="true">' + C.icon(icon) + '</span>' +
      '<div class="an-banner-body"><div class="an-banner-title" data-an-text>' + C.esc(title) + '</div>' +
      '<div class="an-banner-msg" data-an-text>' + C.esc(message) + '</div></div>' +
      (actions ? '<div class="an-banner-acts">' + actions + '</div>' : '') + '</div>';
  }

  /**
   * "Turn on Claude Code control": every settings.json it would edit, and nothing written until
   * the user clicks Turn on. The list is whole (a consent names everything it covers) and wraps.
   * Turn on is the emphasised button but not a default: `data-an-noenter` keeps Enter off it.
   */
  function consentCard(setup) {
    var files = (Array.isArray(setup.consent_files) ? setup.consent_files : []).filter(function (f) {
      return f && typeof f === 'object' && typeof f.path === 'string' && f.path;
    });
    var html = '<section class="an-consent" data-key="consent" aria-label="' + C.esc(COPY.consentTitle) + '">' +
      '<div class="an-consent-head"><span class="an-consent-ic" aria-hidden="true">' + C.icon('terminal') + '</span>' +
      '<h2 class="an-consent-title" data-an-text>' + C.esc(COPY.consentTitle) + '</h2></div>' +
      '<p class="an-cap" data-an-text>' + C.esc(COPY.consentText) + '</p>';
    if (files.length) {
      html += '<ul class="an-cfiles" data-key="files" aria-label="Files it edits">';
      files.forEach(function (f) {
        var account = typeof f.account === 'string' && f.account ? cut(C.oneLine(f.account), 120) : '';
        html += '<li class="an-cfile" data-an-text>' + C.esc(cut(C.oneLine(f.path), PATH_CHARS)) +
          (account ? ' <span class="an-cfile-acct">(' + C.esc(account) + ')</span>' : '') + '</li>';
      });
      html += '</ul>';
    }
    if (strings(setup.codenotch_hooks_folders, PATH_CHARS).length) {
      html += '<p class="an-cap" data-key="codenotch" data-an-text>' + C.esc(COPY.codenotchStays) + '</p>';
    }
    if (setup.install_disabled) {
      html += '<p class="an-cap an-cap-warn" data-key="install-off" data-an-text>' + C.esc(COPY.installOff) + '</p>';
    }
    html += '<div class="an-consent-btns" data-key="btns">' +
      pill('secondary', false, 'Not now', ' data-key="later" data-an-action="consent-later"') +
      pill('primary', false, 'Turn on', ' data-key="on" data-an-action="consent-on" data-an-noenter' + (setup.install_disabled ? ' disabled' : '')) +
      '</div></section>';
    return html;
  }

  /** "Hooks are missing in Work." / "…in Work and Side project." / "…in 3 accounts." (HookHealth.summary). */
  function missingSummary(names) {
    if (names.length === 1) return 'Hooks are missing in ' + names[0] + '.';
    if (names.length === 2) return 'Hooks are missing in ' + names[0] + ' and ' + names[1] + '.';
    return 'Hooks are missing in ' + names.length + ' accounts.';
  }

  function scopeMessage(count) {
    var folders = count === 1 ? '1 VS Code workspace’s folder' : count + ' VS Code workspaces’ folders';
    return 'This version puts its hooks and status line in ' + folders + ' too, and in new ones as they appear: Claude Parallel Profiles runs Claude Code there. Each settings.json has a backup beside it. Account stores never get hooks.';
  }

  /** The setup banners, in order of what blocks the most (SetupBanners.swift). */
  function bannersHtml(v) {
    if (!v) return '';
    var setup = setupOf(v);
    var needsConsent = !!setup.needs_hook_consent;
    var html = '';
    if (consentShown(v)) html += consentCard(setup);
    var scope = strings(setup.new_install_folders, PATH_CHARS);
    if (scope.length && !needsConsent && !state.scopeAnswered) {
      html += noticeBanner('scope', 'info', 'idle', COPY.scopeTitle, scopeMessage(scope.length),
        bannerButton('scope-ok', 'OK', 'ok') + bannerButton('scope-off', 'Turn off', 'off'));
    }
    if (typeof setup.transport_error === 'string' && setup.transport_error) {
      html += noticeBanner('pipe', 'bolt', 'error', COPY.pipeTitle, cut(C.oneLine(setup.transport_error), BANNER_CHARS) + ' ' + COPY.pipeTail, '');
    }
    var missing = strings(setup.missing_hooks_accounts, 80);
    if (setup.control_off && !needsConsent) {
      html += noticeBanner('off', 'pause', 'idle', COPY.offTitle, COPY.offText, bannerButton('open-settings', 'Turn on…', 'settings'));
    } else if (missing.length && !needsConsent) {
      html += noticeBanner('missing', 'exclaim', 'needs', missingSummary(missing),
        (missing.length === 1 ? 'Its' : 'Their') + ' sessions still show here; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are back.',
        bannerButton('open-settings', 'Settings…', 'settings'));
    }
    // Inside the consent card these two are a line of the card; after consent they stand alone.
    var codenotch = strings(setup.codenotch_hooks_folders, PATH_CHARS);
    if (codenotch.length && !needsConsent) {
      html += noticeBanner('codenotch', 'info', 'idle', COPY.codenotchTitle,
        'The official ' + OFFICIAL_APP + ' has its own hooks in ' + C.plural(codenotch.length, 'folder') + '. They run beside this app’s; remove them per folder in Settings › Claude Code.',
        bannerButton('open-settings', 'Settings…', 'settings'));
    }
    if (setup.install_disabled && !needsConsent) {
      html += noticeBanner('install-off', 'info', 'idle', COPY.installOff, COPY.installOffText, '');
    }
    return html;
  }

  /** The undo toast, while a mark-all-reviewed can still be taken back; a notice about an answer. */
  function toastHtml() {
    var notice = state.notice
      ? '<div class="an-notice" data-key="notice" role="status"><span class="an-notice-ic an-tone-needs" aria-hidden="true">' + C.icon('exclaim') +
        '</span><span class="an-notice-t" data-an-text>' + C.esc(cut(C.oneLine(state.notice.text), BANNER_CHARS)) + '</span></div>'
      : '';
    return L.toastHtml(state.pending) + notice;
  }

  /** Says why an answer did nothing, for a few seconds. Never a retry: the request has moved on. */
  function showNotice(text) {
    if (state.notice && state.notice.timer !== null) window.clearTimeout(state.notice.timer);
    var notice = { text: String(text), timer: null };
    state.notice = notice;
    notice.timer = window.setTimeout(function () {
      if (state.notice !== notice) return;
      state.notice = null;
      render();
    }, NOTICE_MS);
    render();
  }

  // ---- answering: the row action bars and the one gate every answer passes ----------------

  /** The session's row in what the page shows now, or null. */
  function rowOf(sessionId) {
    var v = view();
    return sessionsOf(v).filter(function (s) { return s && String(s.session_id) === String(sessionId); })[0] || null;
  }

  /** The requests whose answer bars are on screen: the unfolded rows of the list that carry one. */
  function shownRequests(v) {
    if (!v || mode() !== 'list') return [];
    var ids = [];
    listLayout(v).sections.forEach(function (section) {
      if (section.collapsed) return;
      section.rows.forEach(function (row) {
        var id = C.primaryActions(row).toolUseId;
        if (typeof id === 'string' && id) ids.push(id);
      });
    });
    return ids;
  }

  /**
   * Tells the gate what is on screen, before the bars are drawn from it. A scene's requests are
   * armed at once (a still picture has no wait). In a chat the list's bars are gone: chat.js
   * reports its own bar through `agentnotchPanel.noteShown`.
   */
  function noteRequests(v) {
    if (!gate) return;
    var now = C.now();
    var m = mode();
    // The list's bars are off screen in a chat, and the chat's in the list: none stays armed
    // across the change. A panel opened again starts every wait over too, so a click aimed at
    // what was under it never lands on a bar.
    if (m !== gateMode || state.rearm) gate.noteShown([], now);
    gateMode = m;
    state.rearm = false;
    if (m !== 'list') return;
    var ids = shownRequests(v);
    if (state.scene) gate.noteShownArmed(ids);
    gate.noteShown(ids, now);
  }

  /** One timer for the page: redraws the bars the moment the next waiting request arms. */
  function scheduleArming() {
    if (armTimer !== null) {
      window.clearTimeout(armTimer);
      armTimer = null;
    }
    if (!gate || state.scene) return;
    var now = C.now();
    var next = gate.nextArming(now);
    if (next === null) return;
    armTimer = window.setTimeout(function () {
      armTimer = null;
      render();
      // The chat draws its own bar: tell it the wait is over.
      var chat = window.agentnotchChat;
      if (mode() === 'chat' && chat && typeof chat.refresh === 'function') chat.refresh();
    }, Math.max(1, next - now));
  }

  function answerButton(kind, label, title, row, toolUseId, arg, extra) {
    // A still scene has no wait: its bars are drawn answerable (and answer nothing).
    var answered = !!gate && !state.scene && gate.wasAnswered(toolUseId);
    var armed = state.scene ? true : !!gate && gate.isArmed(toolUseId, C.now());
    return '<button type="button" class="an-btn an-btn-' + kind + ' an-compact' + (armed ? '' : ' an-unarmed') + (answered ? ' an-answered' : '') +
      '" data-key="ans-' + C.esc(arg) + '-' + C.esc(toolUseId) + '" data-an-action="answer" data-an-arg="' + C.esc(arg) +
      '" data-an-session="' + C.esc(row.session_id) + '" data-an-tool="' + C.esc(toolUseId) + '" data-an-noenter title="' + tip(title) + '"' +
      (armed ? '' : ' aria-disabled="true"') + (extra && extra.label ? ' aria-label="' + tip(extra.label) + '"' : '') + '>' +
      (extra && extra.number ? '<span class="an-chipnum" data-an-text aria-hidden="true">' + extra.number + '</span>' : '') +
      '<span data-an-text>' + C.esc(cut(C.oneLine(label), 80)) + '</span></button>';
  }

  function chatButton(kind, label, title, row, key) {
    return pill(kind, true, label, ' data-key="' + key + '" data-an-action="open-chat" data-an-arg="' + C.esc(row.session_id) + '" title="' + tip(title) + '"');
  }

  /**
   * A row's action bar (RowActionBar.swift): what `agentnotchCommon.primaryActions` says the row
   * offers. Every answering button carries the session and the request it was drawn for.
   */
  function actionBarHtml(row) {
    var a = C.primaryActions(row);
    var p = row.pending || {};
    switch (a.kind) {
      case 'permission': {
        var showsAlways = a.alwaysInline && !a.needsReview;
        var caption = showsAlways ? 'Always: ' + cut(C.oneLine(p.always), BANNER_CHARS) : a.needsReview ? COPY.tooLong : '';
        return (caption ? '<div class="an-act-cap" data-key="cap" data-an-text>' + C.esc(caption) + '</div>' : '') +
          '<div class="an-acts' + (showsAlways ? ' an-acts-always' : '') + '" data-key="acts">' +
          answerButton('secondary', 'Deny', 'Deny (Ctrl+Backspace)', row, a.toolUseId, 'deny') +
          // A lasting rule: plain, never the eye-catching one.
          (showsAlways ? answerButton('secondary', 'Always', cut(C.oneLine(p.always), 160) + ' (Ctrl+Alt+Enter)', row, a.toolUseId, 'always') : '') +
          (a.needsReview
            ? chatButton('primary', 'Review…', 'Open the whole request in the chat (Ctrl+Enter)', row, 'review')
            : answerButton('primary', 'Allow', 'Allow (Ctrl+Enter)', row, a.toolUseId, 'allow')) +
          '</div>';
      }
      case 'question_chips': {
        var chips = a.question.options.map(function (option, index) {
          var label = option && option.label != null ? String(option.label) : '';
          var n = index + 1;
          var title = option && typeof option.description === 'string' && option.description
            ? option.description + ' (' + n + ')' : 'Answer ' + label + ' (' + n + ')';
          return answerButton('tinted', label, title, row, a.toolUseId, 'option:' + index, { number: n, label: label });
        }).join('');
        return '<div class="an-acts an-acts-chips" data-key="acts">' + chips +
          chatButton('quiet', 'Other…', 'Type another answer in the chat', row, 'other') + '</div>';
      }
      case 'answer_in_chat':
        return '<div class="an-acts" data-key="acts">' + chatButton('primary', 'Answer…', 'Answer in the chat (Ctrl+Enter)', row, 'answer-chat') + '</div>';
      case 'plan':
        return '<div class="an-acts" data-key="acts">' +
          chatButton('secondary', 'Review plan', 'Read the whole plan (Enter)', row, 'review-plan') +
          answerButton('primary', 'Approve', 'Approve the plan and let Claude start (Ctrl+Enter)', row, a.toolUseId, 'approve') + '</div>';
      case 'answer_in_terminal':
        return '<div class="an-acts an-acts-term" data-key="acts"><span class="an-act-note" data-an-text>Answer in the terminal</span>' +
          (row.focus_label ? pill('secondary', true, cut(C.oneLine(row.focus_label), 40), ' data-key="jump" data-an-action="jump" data-an-arg="' + C.esc(row.session_id) +
            '" title="' + tip(String(row.focus_label) + ' (Ctrl+J)') + '"') : '') + '</div>';
      default:
        return '';
    }
  }

  /**
   * Claude Code's `answers` map (question text -> answer) for a question form, or null until every
   * question has one (ChatQuestionAnswers). `picks[i]`: `{labels: [...], other: 'typed' | null}`.
   * One choice answers with its label; several are joined by ", " in the options' order, the
   * typed "Other" last; a single-select "Other" answers with the typed text alone.
   */
  function questionAnswers(questions, picks) {
    if (!Array.isArray(questions) || !questions.length) return null;
    var out = {};
    for (var i = 0; i < questions.length; i++) {
      var q = questions[i] || {};
      var pick = picks && picks[i];
      if (!pick) return null;
      var chosen = Array.isArray(pick.labels) ? pick.labels.map(String) : [];
      var options = (Array.isArray(q.options) ? q.options : []).map(function (o) { return String(o && o.label); });
      var ordered = options.filter(function (l) { return chosen.indexOf(l) >= 0; })
        .concat(chosen.filter(function (l) { return options.indexOf(l) < 0; }));
      var other = typeof pick.other === 'string' ? pick.other.trim() : '';
      var answer;
      if (q.multi_select) {
        var parts = other ? ordered.concat([other]) : ordered;
        answer = parts.length ? parts.join(', ') : null;
      } else if (typeof pick.other === 'string') {
        answer = other || null;
      } else {
        answer = ordered.length ? ordered[0] : null;
      }
      if (answer === null) return null;
      // An own key whatever the question says (a question named "__proto__" is still a question).
      Object.defineProperty(out, String(q.text), { value: answer, enumerable: true, writable: true, configurable: true });
    }
    return out;
  }

  /**
   * The answer a command stands for, checked against what the session is waiting for NOW: the
   * request named must be the pending one and of the kind the answer fits, and from the list
   * only what the row offers (never Allow or Always past "review it whole first"). Null refuses.
   */
  function answerFor(cmd) {
    var row = rowOf(cmd.sessionId);
    if (!row) return null;
    var a = C.primaryActions(row);
    if (!a.toolUseId || a.toolUseId !== cmd.toolUseId) return null;
    var inChat = mode() === 'chat';
    switch (cmd.cmd) {
      case 'allow':
        return a.kind === 'permission' && (inChat || !a.needsReview) ? { allow: { always: false } } : null;
      case 'alwaysAllow':
        return a.kind === 'permission' && a.hasAlways && (inChat || (a.alwaysInline && !a.needsReview)) ? { allow: { always: true } } : null;
      case 'deny':
        return a.kind === 'permission' ? { deny: { reason: null } } : null;
      case 'approvePlan':
        return a.kind === 'plan' ? 'approve_plan' : null;
      case 'keepPlanning':
        return a.kind === 'plan' ? 'keep_planning' : null;
      case 'chooseOption': {
        if (a.kind !== 'question_chips') return null;
        var option = a.question.options[cmd.index];
        if (!option || option.label == null) return null;
        var answers = questionAnswers([a.question], [{ labels: [String(option.label)], other: null }]);
        return answers ? { questions: { answers: answers } } : null;
      }
      default:
        return null;
    }
  }

  /**
   * Sends one answer for one request, once. The gate decides: not before the request has been on
   * screen for 350 ms, never twice. A reply that says the request had moved on is said and left
   * at that; nothing here ever sends again. A sealed scene sends nothing. True when it was sent.
   */
  function sendAnswer(sessionId, toolUseId, answer) {
    if (!gate || state.scene || answer == null) return false;
    if (typeof sessionId !== 'string' || typeof toolUseId !== 'string' || !sessionId || !toolUseId) return false;
    if (!gate.claim(toolUseId, C.now())) return false;
    render();
    C.call('answer', { session_id: sessionId, tool_use_id: toolUseId, answer: answer }).then(function (reply) {
      var result = reply && reply.result;
      if (result === 'not_pending') showNotice(COPY.notPending);
      else if (result === 'peer_gone') showNotice(COPY.peerGone);
    }, function (error) {
      fail('answer')(error);
      showNotice(COPY.notSent);
    });
    return true;
  }

  /** Carries out a router command (a key or a click). True when it did something. */
  function perform(cmd) {
    if (!cmd) return false;
    switch (cmd.cmd) {
      case 'move': {
        var v = view();
        if (!v || mode() !== 'list') return false;
        var next = C.moveSelection(state.selected, cmd.delta, listLayout(v).visible);
        if (next === null) return false;
        state.selected = next;
        state.scrollTo = next;
        render();
        return true;
      }
      case 'openChat':
        navigate('session:' + cmd.sessionId);
        return true;
      case 'allow':
      case 'alwaysAllow':
      case 'deny':
      case 'approvePlan':
      case 'keepPlanning':
      case 'chooseOption':
        return sendAnswer(cmd.sessionId, cmd.toolUseId, answerFor(cmd));
      case 'markReviewed':
        ACTIONS['mark-reviewed'](cmd.sessionId);
        return true;
      case 'dismissFailure':
        ACTIONS['dismiss-failure'](cmd.sessionId);
        return true;
      case 'markAllReviewed':
        markAllReviewed();
        return true;
      case 'jump':
        ACTIONS.jump(cmd.sessionId);
        return true;
      case 'back':
        navigate('sessions');
        return true;
      case 'close':
        closePanel();
        return true;
      default:
        return false;
    }
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

  /** Where a row's action bar goes (`agentnotchPanelList` calls it per row). */
  var SLOTS = { actions: actionBarHtml };

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
    settleSetup(v);
    dropSelection(v);
    noteRequests(v);
    C.morph(els.header, headerHtml(v));
    C.morph(els.banners, bannersHtml(v));
    var before = C.flipFirst(els.rows);
    C.morph(els.rows, listHtml(v));
    C.flipPlay(els.rows, before);
    C.morph(els.toast, toastHtml(v));
    scrollToRow();
    C.morph(els.overlay, overlayHtml(v));
    applyMode();
    var chat = window.agentnotchChat;
    if (mode() === 'chat' && chat && typeof chat.refresh === 'function') chat.refresh();
    makeRoomForMenu();
    reportSize();
    scheduleArming();
  }

  /** Once the engine no longer asks (or lists no new folder), a later ask shows its card again. */
  function settleSetup(v) {
    if (!v || state.scene) return;
    var setup = setupOf(v);
    if (!setup.needs_hook_consent) state.consentAnswered = false;
    if (!strings(setup.new_install_folders, PATH_CHARS).length) state.scopeAnswered = false;
  }

  /** A selection folded away, filtered out or ended is dropped: the arrows start from the top again. */
  function dropSelection(v) {
    if (!v || mode() !== 'list' || state.selected === null) return;
    if (listLayout(v).visible.indexOf(state.selected) < 0) state.selected = null;
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
  /** The chat's own ask (header, the transcript's content, the bottom bar): its box is whatever the window gives it. */
  function chatHeight() {
    var chat = window.agentnotchChat;
    var h = chat && typeof chat.naturalHeight === 'function' ? chat.naturalHeight() : 0;
    return h > 0 ? h : els.chat.offsetHeight;
  }

  function naturalHeight() {
    var total = 0;
    Array.prototype.forEach.call(els.card.children, function (child) {
      if (child === els.overlay || child.hasAttribute('hidden')) return;
      total += child === els.list ? child.scrollHeight : child === els.chat ? chatHeight() : child.offsetHeight;
    });
    var menu = els.overlay.querySelector('.an-menu');
    if (menu && menu.offsetHeight) total = Math.max(total, (menu.offsetTop || 0) + menu.offsetHeight + 8);
    return total;
  }

  /** `panel_report_size {w, h}` (CSS px of the card) whenever the content's height or the width changes. */
  function reportSize() {
    var h = Math.ceil(naturalHeight());
    if (!(h > 0)) return;
    if (mode() === 'chat') h = Math.min(h, CHAT_HEIGHT_CAP);
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

  ACTIONS.back = function () {
    navigate('sessions');
  };
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

  // -- answers: a click on a bar's button is the same command the keyboard sends --

  var ANSWER_ARGS = { allow: 'allow', always: 'alwaysAllow', deny: 'deny', approve: 'approvePlan', keep: 'keepPlanning' };

  ACTIONS.answer = function (arg, el) {
    if (!el || typeof el.getAttribute !== 'function') return;
    var cmd = { sessionId: el.getAttribute('data-an-session'), toolUseId: el.getAttribute('data-an-tool') };
    var option = /^option:(\d{1,2})$/.exec(String(arg));
    if (option) {
      cmd.cmd = 'chooseOption';
      cmd.index = Number(option[1]);
    } else if (Object.prototype.hasOwnProperty.call(ANSWER_ARGS, arg)) {
      cmd.cmd = ANSWER_ARGS[arg];
    } else {
      return;
    }
    perform(cmd);
  };

  // -- setup: each of these writes settings.json (or stops writing it), so each runs only from a
  //    click on its own button inside the banners, once --

  function ownButton(el, action) {
    return !!el && typeof el.getAttribute === 'function' && els.banners.contains(el) &&
      el.getAttribute('data-an-action') === action && !el.hasAttribute('disabled');
  }

  function answerConsent(grant) {
    state.consentAnswered = true;
    render();
    C.call('hook_consent', { grant: grant }).catch(function (error) {
      // Refused (sealed, or the engine is gone): nothing was decided, so the card comes back.
      fail('hook_consent')(error);
      state.consentAnswered = false;
      render();
    });
  }

  ACTIONS['consent-on'] = function (arg, el, event) {
    if (!ownButton(el, 'consent-on') || !event || event.type !== 'click') return;
    var v = view();
    if (state.scene || !consentShown(v) || setupOf(v).install_disabled) return;
    answerConsent(true);
  };
  ACTIONS['consent-later'] = function (arg, el, event) {
    if (!ownButton(el, 'consent-later') || !event || event.type !== 'click') return;
    if (state.scene || !consentShown(view())) return;
    answerConsent(false);
  };

  function answerScope(method, args) {
    state.scopeAnswered = true;
    render();
    C.call(method, args).catch(function (error) {
      fail(method)(error);
      state.scopeAnswered = false;
      render();
    });
  }

  ACTIONS['scope-ok'] = function (arg, el, event) {
    if (!ownButton(el, 'scope-ok') || !event || event.type !== 'click' || state.scene || state.scopeAnswered) return;
    answerScope('acknowledge_scope');
  };
  ACTIONS['scope-off'] = function (arg, el, event) {
    if (!ownButton(el, 'scope-off') || !event || event.type !== 'click' || state.scene || state.scopeAnswered) return;
    answerScope('hooks_enabled', { on: false });
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
      return;
    }
    if (state.menuOpen) return;
    guardButtonKey(event);
    // THE KEYBOARD GATE. Until the glue confirms this window is the foreground one, keys here
    // may be keys the user is typing into a terminal: nothing acts on them and no field takes
    // them. They are dropped, not remembered.
    if (!state.focused) {
      if (isTextField(event.target)) event.preventDefault();
      return;
    }
    // The Windows key is never part of a panel shortcut.
    if (event.metaKey) return;
    var key = C.routerKey(event);
    if (key === null) return;
    var typing = isTextField(event.target);
    var mods = { ctrl: !!event.ctrlKey, alt: !!event.altKey, shift: !!event.shiftKey };
    // Ctrl+Z takes back a mark-all-reviewed while its toast shows (in a field it is the field's undo).
    if (!typing && mods.ctrl && !mods.alt && !mods.shift && String(key).toLowerCase() === 'z') {
      if (!state.pending) return;
      event.preventDefault();
      undoReview();
      return;
    }
    // A field owns plain keys, digits and Enter included; Ctrl combinations still route.
    if (typing && !mods.ctrl) return;
    var cmd = C.commandFor(key, mods, keyContext(typing));
    if (!cmd) return;
    event.preventDefault();
    // A held key repeats: it must never answer the request that takes the place of the one it answered.
    if (event.repeat && ANSWER_COMMANDS.indexOf(cmd.cmd) >= 0) return;
    perform(cmd);
  }

  var ANSWER_COMMANDS = ['allow', 'alwaysAllow', 'deny', 'approvePlan', 'keepPlanning', 'chooseOption'];

  /** What the keyboard acts on: the selected row of the list, or the open chat's session. */
  function keyContext(typing) {
    if (mode() === 'chat') {
      var chatRow = rowOf(C.parseRoute(state.route).id);
      // A chat whose session ended has nothing to answer; Esc (handled before) still goes back.
      return chatRow ? { kind: 'chat', target: C.keyTarget(chatRow), typing: typing } : { kind: 'setup' };
    }
    var v = view();
    var row = v && state.selected !== null && listLayout(v).visible.indexOf(state.selected) >= 0 ? rowOf(state.selected) : null;
    return { kind: 'list', target: row ? C.keyTarget(row) : null };
  }

  /**
   * Enter and Space on a focused button are clicks the browser makes up. While the gate is shut
   * no key may click anything; and Enter never clicks a button that answers a request or turns
   * the hooks on (`data-an-noenter`): "a bare Enter never approves anything".
   */
  function guardButtonKey(event) {
    if (event.key !== 'Enter' && event.key !== ' ' && event.key !== 'Spacebar') return;
    var button = event.target && event.target.closest ? event.target.closest('button, [role="button"]') : null;
    if (!button) return;
    if (!state.focused || (event.key === 'Enter' && button.hasAttribute('data-an-noenter'))) event.preventDefault();
  }

  function onKeyUp(event) {
    if (!state.menuOpen) guardButtonKey(event);
  }

  /** While the gate is shut a field takes no text, however it arrives (a key, a paste, an IME). */
  function onBeforeInput(event) {
    if (!state.focused && isTextField(event.target)) event.preventDefault();
  }

  function onPointerDown(event) {
    // A click on a field while the gate is shut asks for the keyboard; the gate opens only when
    // the glue answers with an:panel_focus.
    if (!state.focused && isTextField(event.target)) C.call('panel_take_focus').catch(fail('panel_take_focus'));
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
    state.rearm = true;
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
    // A field that had the DOM focus while the gate was shut keeps no caret it cannot use.
    if (!state.focused && isTextField(document.activeElement) && typeof document.activeElement.blur === 'function') document.activeElement.blur();
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

  /** A copy of the row `id` with a new id and `patch` laid over it (and over its `pending`). */
  function variant(s, id, newId, patch, pending) {
    var base = s.sessions.filter(function (r) { return r.session_id === id; })[0];
    if (!base) return null;
    var row = clone(base);
    row.session_id = newId;
    Object.keys(patch || {}).forEach(function (k) { row[k] = patch[k]; });
    if (pending && row.pending) Object.keys(pending).forEach(function (k) { row.pending[k] = pending[k]; });
    return row;
  }

  /**
   * Every kind of action bar at once (the Mac's panel-needs-you): the fixture's rows that need
   * you, plus the variants the fixture has no row for: several questions (Answer…), a request
   * too long to judge from the row (Review…), and a permission only the terminal can answer.
   */
  function needsYou(s) {
    var long = 'git push origin --delete release/2025.1 && \\\n  git push origin --delete release/2025.2 && \\\n  git push origin --delete release/2025.3 && \\\n  git push origin --delete release/2025.4 && \\\n  git push origin --delete release/2025.5';
    var extra = [
      variant(s, 'needs-question', 'needs-questions', { title: 'Plan the onboarding flow', project: 'mobile-app', since_ms: s.generated_at_ms - 70000,
        detail: { kind: 'question', text: 'Which screens should onboarding include?' } },
      { tool_use_id: 'toolu_scene_questions', single_tap: false, questions: [
        { text: 'Which screens should onboarding include?', header: 'Screens', multi_select: true, options: [{ label: 'Welcome', description: null }, { label: 'Permissions', description: null }] },
        { text: 'Should it be skippable?', header: 'Skip', multi_select: false, options: [{ label: 'Yes', description: null }, { label: 'No', description: null }] },
      ] }),
      variant(s, 'needs-permission', 'needs-long', { title: 'Clean up old release branches', tasks: null, since_ms: s.generated_at_ms - 40000,
        detail: { kind: 'permission', tool: 'Bash', request: long, waiting_in_terminal: false } },
      { tool_use_id: 'toolu_scene_long', request: long, needs_review: true }),
      variant(s, 'needs-elicitation', 'needs-terminal', { title: 'Clean up old branches', project: 'infra', since_ms: s.generated_at_ms - 65000,
        detail: { kind: 'permission', tool: 'Bash', request: null, waiting_in_terminal: true } }),
    ].filter(Boolean);
    s.sessions = s.sessions.filter(function (r) { return r.bucket === 'needs_you'; }).concat(extra);
    return retally(s);
  }

  /** The consent card over an empty list: nothing is tracked before the hooks are on. */
  function consent(s) {
    emptied(s);
    s.setup = s.setup || {};
    s.setup.hook_consent = null;
    s.setup.needs_hook_consent = true;
    s.setup.codenotch_hooks_folders = ['~\\.claude'];
    return s;
  }

  function banners(s) {
    s.setup = s.setup || {};
    s.setup.needs_hook_consent = false;
    s.setup.transport_error = 'The hook pipe couldn’t be opened (access is denied).';
    s.setup.missing_hooks_accounts = ['Work'];
    s.setup.codenotch_hooks_folders = ['~\\.claude'];
    return s;
  }

  function scopeNotice(s) {
    s.setup = s.setup || {};
    s.setup.needs_hook_consent = false;
    s.setup.new_install_folders = ['~\\.claude-windows\\5d1e0a7b3c21', '~\\.claude-windows\\9b4f2e8d6a10', '~\\.claude-windows\\c07a3f5e1d94'];
    return s;
  }

  /** Scene name -> how it reads the current snapshot. Later sub-tasks add their scenes here. */
  var SCENES = {
    'panel-needs-you': needsYou,
    'panel-banners': banners,
    'panel-consent': consent,
    'panel-scope-notice': scopeNotice,
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
    // The chat scenes show the same snapshot; the route (SCENE_ROUTES) and the chat's own state do the rest.
    'chat-approval': same,
    'chat-tasks': same,
    'panel-single-account': function (s) {
      var first = ringsOf(s)[0];
      s.accounts_multi = false;
      s.rings = first ? [first] : [];
      return s;
    },
  };

  /** The scenes that are a chat: the session each one shows. */
  var SCENE_ROUTES = {
    'chat-approval': 'session:needs-permission',
    'chat-tasks': 'session:work-migration',
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
    state.route = SCENE_ROUTES[name] || 'sessions';
    state.filter = null;
    state.selected = null;
    state.folds = {};
    if (state.pending && state.pending.timer !== null) window.clearTimeout(state.pending.timer);
    state.pending = null;
    if (state.notice && state.notice.timer !== null) window.clearTimeout(state.notice.timer);
    state.notice = null;
    state.consentAnswered = false;
    state.scopeAnswered = false;
    state.menuOpen = name === 'panel-menu';
    if (state.menuOpen) loadNotify();
    if (name === 'panel-filtered') {
      var ring = ringsOf(state.snapshot)[1] || ringsOf(state.snapshot)[0];
      state.filter = ring ? ring.ring_id : null;
    }
    if (SCENE_STATE[name]) SCENE_STATE[name]();
    render();
    // The chat exists only once the route has drawn it: its own scene state comes after.
    var chat = window.agentnotchChat;
    if (name === 'chat-tasks' && chat && typeof chat.openBoard === 'function') chat.openBoard(true);
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
  /** The chat's transcript when `el` is inside it: it scrolls, like the list. */
  function chatScroller(el) {
    var scroll = els.chat.querySelector('.an-chat-scroll');
    return scroll && scroll.contains(el) ? scroll : null;
  }

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
      var scroller = els.list.contains(el) ? els.list : chatScroller(el);
      var scrolled = !!scroller && scroller.scrollHeight > scroller.clientHeight + 1;
      var box = scrolled ? { left: cr.left, right: cr.right, top: -1e9, bottom: 1e9 } : cr;
      if (r.width && cr.width && !rectInside(r, box, 0.5)) {
        failures.push('control outside the card: ' + (el.getAttribute('aria-label') || C.oneLine(el.textContent)).slice(0, 40));
      }
    });
    var chat = window.agentnotchChat;
    if (mode() === 'chat' && chat && typeof chat.layoutProblems === 'function') chat.layoutProblems().forEach(function (p) { failures.push(p); });
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
    gate = C.createAnswerGate();

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
    document.addEventListener('keyup', onKeyUp);
    document.addEventListener('beforeinput', onBeforeInput);
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
    slots: SLOTS,
    undoReview: undoReview,
    markAllReviewed: markAllReviewed,
    /**
     * For chat.js, so the whole page answers through one gate: `answer(sessionId, toolUseId,
     * answer)` sends once when the request is armed (true when sent); `noteShown(ids)` says which
     * requests the chat's bar shows (call it on every draw of the bar); `isArmed(id)` draws the
     * buttons; `answers(questions, picks)` builds a question form's map; `keyboardOpen()` is the
     * keyboard gate (a field is read-only and says "Click to type" until it is true).
     */
    answer: function (sessionId, toolUseId, answer) {
      if (mode() !== 'chat') return false;
      return sendAnswer(sessionId, toolUseId, answer);
    },
    noteShown: function (ids) {
      if (!gate || mode() !== 'chat') return;
      var list = (Array.isArray(ids) ? ids : []).filter(function (id) { return typeof id === 'string' && id; });
      if (state.scene) gate.noteShownArmed(list);
      gate.noteShown(list, C.now());
      scheduleArming();
    },
    isArmed: function (id) {
      return !!gate && gate.isArmed(id, C.now());
    },
    answers: questionAnswers,
    keyboardOpen: function () {
      return state.focused;
    },
    /** For chat.js: a session's row as the page shows it, whether several accounts are in use, and the size report. */
    row: rowOf,
    multiAccounts: function () {
      var v = view();
      return !!(v && v.accounts_multi);
    },
    reportSize: reportSize,
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
      perform: perform,
      answerFor: answerFor,
      actionBarHtml: actionBarHtml,
      bannersHtml: bannersHtml,
    },
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();
