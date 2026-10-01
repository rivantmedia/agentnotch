// Agent Notch's shared page library (DESIGN-WIN §5.1): what the notch additions, the sessions
// panel, its chat and the Claude Code settings pane all draw with.
//
// One IIFE, one global (`window.agentnotchCommon`), no top-level let/const: the notch and
// Settings load it next to upstream's classic scripts, where a second `let` of the same name
// would be a SyntaxError that stops upstream's whole page.
//
// Every string the pages show that did not come from this file (session titles, transcript
// text, account and organisation names, paths) is untrusted. Renderers build HTML strings and
// put every such value through `esc`, the one escaper; nothing here ever writes raw HTML from
// data, an inline handler, or a `javascript:` URL. The app-wide CSP (seam WCSP) blocks those
// anyway; the escaping is what keeps a hostile title from becoming markup at all.
(function () {
  'use strict';

  var C = {};

  // ---- escaping -------------------------------------------------------------------------

  var ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;', '`': '&#96;' };

  /** Text or attribute value → HTML-safe text. `null`/`undefined` become empty. */
  C.esc = function (value) {
    return String(value == null ? '' : value).replace(/[&<>"'`]/g, function (c) {
      return ESCAPES[c];
    });
  };

  /** Collapses runs of whitespace (a transcript line drawn on one line). */
  C.oneLine = function (value) {
    return String(value == null ? '' : value).replace(/\s+/g, ' ').trim();
  };

  // ---- the clock ------------------------------------------------------------------------

  // Every elapsed label, the answer gate and the undo window read time here, so tests and the
  // sealed snapshot scenes can pin it.
  var clock = function () {
    return Date.now();
  };
  C.now = function () {
    return clock();
  };
  C.setClock = function (fn) {
    clock = typeof fn === 'function' ? fn : function () {
      return Date.now();
    };
  };

  // ---- formatting (UsageFormatter, ElapsedCopy) -----------------------------------------

  var MAX_SPAN_MS = 10000 * 86400 * 1000;

  /** Compact countdown: "<1m", "45m", "2h 13m", "2h", "3d 5h", "3d" (UsageFormatter.duration). */
  C.duration = function (ms) {
    var total = Math.floor(Math.min(Math.max(Number(ms) || 0, 0), MAX_SPAN_MS) / 1000);
    var days = Math.floor(total / 86400);
    var hours = Math.floor((total % 86400) / 3600);
    var minutes = Math.floor((total % 3600) / 60);
    if (days > 0) return hours > 0 ? days + 'd ' + hours + 'h' : days + 'd';
    if (hours > 0) return minutes > 0 ? hours + 'h ' + minutes + 'm' : hours + 'h';
    return minutes > 0 ? minutes + 'm' : '<1m';
  };

  /** "just now", "5m ago", "3h ago", "2d ago" (UsageFormatter.age). Future times read "just now". */
  C.age = function (ms) {
    var seconds = Math.min(Number(ms) || 0, MAX_SPAN_MS) / 1000;
    if (!(seconds >= 60)) return 'just now';
    if (seconds < 3600) return Math.floor(seconds / 60) + 'm ago';
    if (seconds < 86400) return Math.floor(seconds / 3600) + 'h ago';
    return Math.floor(seconds / 86400) + 'd ago';
  };

  /** The hover card's elapsed words (ElapsedCopy.text): "just now", "5 min", "2 hr 10 min". */
  C.cardElapsed = function (ms) {
    var seconds = Math.min(Math.max(Number(ms) || 0, 0), MAX_SPAN_MS) / 1000;
    if (seconds < 45) return 'just now';
    var minutes = Math.round(seconds / 60);
    if (minutes < 60) return Math.max(1, minutes) + ' min';
    var hours = Math.floor(minutes / 60);
    var rest = minutes % 60;
    return rest === 0 ? hours + ' hr' : hours + ' hr ' + rest + ' min';
  };

  /** "42%", rounded down so a limit never reads as hit before it is (UsageFormatter.percent). */
  C.percent = function (value) {
    var n = Number(value);
    if (!isFinite(n)) return '–';
    return Math.floor(Math.min(Math.max(n, 0), 999)) + '%';
  };

  /** A count badge's label: nothing for zero, "9+" past nine (ClaudeRingBadgeLayout.label). */
  C.badgeLabel = function (count) {
    var n = Math.floor(Number(count) || 0);
    if (n <= 0) return '';
    return n > 9 ? '9+' : String(n);
  };

  /** Context meter level: 80 % turns amber, 90 % critical (ContextMeter.level). */
  C.contextLevel = function (percent) {
    var p = Number(percent) || 0;
    if (p >= 90) return 'critical';
    if (p >= 80) return 'high';
    return 'normal';
  };

  /** "1 need you" / "2 need you" wording: "1 needs you", "2 need you". */
  C.needYou = function (n) {
    return n + (n === 1 ? ' needs you' : ' need you');
  };

  /** The Mac's plural helper: "1 session", "3 sessions". */
  C.plural = function (n, one, many) {
    return n + ' ' + (n === 1 ? one : many || one + 's');
  };

  /**
   * A reason ends a sentence of ours: drop its own full stop so it never reads "off..". A loop,
   * not `/[.。]+$/`, which retries from every dot of a long run that doesn't end the text.
   */
  C.clause = function (reason) {
    var s = String(reason == null ? '' : reason).trim();
    var end = s.length;
    while (end > 0 && (s.charAt(end - 1) === '.' || s.charAt(end - 1) === '\u3002')) end -= 1;
    return s.slice(0, end);
  };

  // ---- account colours (ClaudeControlTheme.accountHues) ---------------------------------

  // Kept clear of the three signal colours on purpose: an account is never told by colour
  // alone, and a dot must never be read as a state.
  C.ACCOUNT_HUES = ['#5C9EFA', '#B885F5', '#F573B3', '#40CCCC', '#858FFF', '#66D1F5', '#FF949E', '#D6C29E'];
  C.accountHue = function (index) {
    var n = C.ACCOUNT_HUES.length;
    var i = Math.floor(Number(index) || 0);
    return C.ACCOUNT_HUES[((i % n) + n) % n];
  };

  /** A filled account dot. */
  C.accountDot = function (colorIndex, size) {
    var s = size || 5;
    return '<span class="an-dot" aria-hidden="true" style="width:' + s + 'px;height:' + s +
      'px;background:' + C.accountHue(colorIndex) + '"></span>';
  };

  // ---- status marks (StatusRing) --------------------------------------------------------

  var RING_BOX = 10;

  function arc(fraction, stroke) {
    var r = (RING_BOX - stroke) / 2;
    var c = 2 * Math.PI * r;
    var dash = (c * fraction).toFixed(3) + ' ' + c.toFixed(3);
    return '<circle cx="5" cy="5" r="' + r.toFixed(3) + '" fill="none" stroke="currentColor" stroke-width="' +
      stroke + '" stroke-linecap="round" stroke-dasharray="' + dash + '" transform="rotate(-90 5 5)"/>';
  }

  /**
   * The mark beside a session, in Codenotch's vocabulary: a three-quarter arc turning while
   * Claude works, half a ring (breathing, then still) while it waits on you, a solid dot for a
   * failed turn, a full ring when done or idle.
   *
   * kind: working | needs | error | review | idle. `breathKey` restarts the breath: a new key
   * gives the element a new identity (`data-key`), so the morph replaces it and the finite CSS
   * animation runs again; the same key leaves it alone.
   */
  C.statusRing = function (kind, opts) {
    var o = opts || {};
    var stroke = o.stroke || 1.9;
    var body;
    var cls = 'an-ring an-ring-' + kind;
    if (kind === 'working') {
      body = arc(0.75, stroke);
      cls += ' an-spin';
    } else if (kind === 'needs') {
      body = arc(0.5, stroke);
      cls += ' an-breathe';
    } else if (kind === 'error') {
      body = '<circle cx="5" cy="5" r="3.6" fill="currentColor"/>';
    } else {
      body = arc(1, stroke);
    }
    var key = kind === 'needs' && o.breathKey != null ? ' data-key="breath-' + C.esc(o.breathKey) + '"' : '';
    return '<svg class="' + cls + (o.cls ? ' ' + o.cls : '') + '"' + key +
      ' viewBox="0 0 10 10" aria-hidden="true" focusable="false">' + body + '</svg>';
  };

  /** A tool's mark in the chat (ToolStatusMark): running spins, waiting is half amber. */
  C.toolMark = function (status) {
    var kind = status === 'running' ? 'working'
      : status === 'waiting_for_approval' ? 'needs'
        : status === 'error' || status === 'interrupted' ? 'error' : 'idle';
    return C.statusRing(kind, { stroke: 1.6, cls: 'an-toolmark' });
  };

  /** The glyph kind of a row. */
  C.glyphKind = function (row) {
    if (row.failed) return 'error';
    switch (row.bucket) {
      case 'needs_you': return 'needs';
      case 'ready_for_review': return 'review';
      case 'working': return 'working';
      default: return 'idle';
    }
  };

  // ---- icons (drawn after the SF Symbols the Mac panel uses) ---------------------------

  var STROKE = ' fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"';
  var ICONS = {
    chevronLeft: '<path d="M7.6 2.4 4 6l3.6 3.6"' + STROKE + '/>',
    chevronRight: '<path d="M4.4 2.4 8 6l-3.6 3.6"' + STROKE + '/>',
    chevronDown: '<path d="M2.4 4.4 6 8l3.6-3.6"' + STROKE + '/>',
    xmark: '<path d="M3 3l6 6M9 3 3 9"' + STROKE + '/>',
    pin: '<path d="M4.3 1.6h3.4l-.45 3 1.75 1.9H3l1.75-1.9zM6 6.5v4"' + STROKE + '/>',
    pinFill: '<path d="M4.3 1.6h3.4l-.45 3 1.75 1.9H3l1.75-1.9z" fill="currentColor" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/><path d="M6 6.5v4"' + STROKE + '/>',
    // Upstream's own settings orb glyph (notch.html), so the gear reads the same in both.
    gear: '<path fill="currentColor" fill-rule="evenodd" d="M5.1.6h1.8l.3 1.5 1.1.5 1.3-.8 1.3 1.3-.8 1.3.5 1.1 1.5.3v1.8l-1.5.3-.5 1.1.8 1.3-1.3 1.3-1.3-.8-1.1.5-.3 1.5H5.1l-.3-1.5-1.1-.5-1.3.8-1.3-1.3.8-1.3-.5-1.1L.6 6.9V5.1l1.5-.3.5-1.1-.8-1.3 1.3-1.3 1.3.8 1.1-.5zM6 4.1a1.9 1.9 0 1 0 0 3.8 1.9 1.9 0 0 0 0-3.8z"/>',
    checkCircle: '<circle cx="6" cy="6" r="4.6"' + STROKE + '/><path d="M3.9 6.1 5.3 7.5 8.1 4.6"' + STROKE + '/>',
    xCircle: '<circle cx="6" cy="6" r="4.6"' + STROKE + '/><path d="M4.4 4.4l3.2 3.2M7.6 4.4 4.4 7.6"' + STROKE + '/>',
    openApp: '<path d="M5.2 2H3.3A1.3 1.3 0 0 0 2 3.3v5.4A1.3 1.3 0 0 0 3.3 10h5.4A1.3 1.3 0 0 0 10 8.7V6.8"' + STROKE + '/><path d="M6.6 2H10v3.4M10 2 5.6 6.4"' + STROKE + '/>',
    bubble: '<path d="M6 2c2.4 0 4.2 1.5 4.2 3.4S8.4 8.8 6 8.8c-.5 0-1 0-1.4-.2L2.3 9.9l.6-2C2.2 7.3 1.8 6.4 1.8 5.4 1.8 3.5 3.6 2 6 2z"' + STROKE + '/>',
    terminal: '<rect x="1.2" y="2" width="9.6" height="8" rx="1.8" fill="currentColor"/><path d="M3.4 4.6 5 6 3.4 7.4M5.9 7.6h2.6" fill="none" stroke="var(--an-card)" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>',
    info: '<circle cx="6" cy="6" r="5" fill="currentColor"/><path d="M6 5.4v3M6 3.6v.1" stroke="var(--an-card)" stroke-width="1.3" stroke-linecap="round"/>',
    warning: '<path d="M6 1.3 11 10.2H1z" fill="currentColor" stroke="currentColor" stroke-width="1" stroke-linejoin="round"/><path d="M6 4.6v2.7M6 8.8v.1" stroke="var(--an-card)" stroke-width="1.3" stroke-linecap="round"/>',
    exclaim: '<circle cx="6" cy="6" r="5" fill="currentColor"/><path d="M6 3.4v3.2M6 8.4v.1" stroke="var(--an-card)" stroke-width="1.3" stroke-linecap="round"/>',
    bolt: '<circle cx="6" cy="6" r="5" fill="currentColor"/><path d="M6.6 2.9 4 6.3h2.2L5.4 9.1 8 5.7H5.8z" fill="var(--an-card)"/>',
    pause: '<circle cx="6" cy="6" r="4.6"' + STROKE + '/><path d="M5 4.3v3.4M7 4.3v3.4"' + STROKE + '/>',
    arrowUp: '<path d="M6 9.6V2.6M2.8 5.6 6 2.4l3.2 3.2"' + STROKE.replace('1.35', '1.7') + '/>',
    photo: '<rect x="1.4" y="2.4" width="9.2" height="7.2" rx="1.5"' + STROKE + '/><path d="m2.4 8.4 2.5-2.6 1.8 1.8 1.2-1.2 1.8 2"' + STROKE + '/>',
    check: '<path d="M2.6 6.2 5 8.6l4.4-5"' + STROKE.replace('1.35', '1.8') + '/>',
  };

  /** An icon by name, sized by CSS (`.an-icon`). */
  C.icon = function (name, cls) {
    return '<svg class="an-icon' + (cls ? ' ' + cls : '') + '" viewBox="0 0 12 12" aria-hidden="true" focusable="false">' +
      (C.hasIcon(name) ? ICONS[name] : '') + '</svg>';
  };
  C.hasIcon = function (name) {
    return Object.prototype.hasOwnProperty.call(ICONS, name);
  };

  // ---- the bridge -----------------------------------------------------------------------

  function tauri() {
    return window.__TAURI__ || null;
  }

  /**
   * `invoke('an_call', {method, args})`. Resolves with the engine's (or the glue's) reply;
   * rejects with `{code, message}`. Without the bridge (a page opened outside the app) it
   * rejects the same way, so callers need only one failure path.
   */
  C.call = function (method, args) {
    var t = tauri();
    if (!t || !t.core || typeof t.core.invoke !== 'function') {
      return Promise.reject({ code: 'failed', message: "Agent Notch's engine isn't reachable from this page." });
    }
    var sent = args === undefined ? null : args;
    return t.core.invoke('an_call', { method: method, args: sent }).catch(function (error) {
      throw C.callError(error);
    });
  };

  /** Any rejection as `{code, message}`. */
  C.callError = function (error) {
    if (error && typeof error === 'object' && typeof error.message === 'string') {
      return { code: String(error.code || 'failed'), message: error.message };
    }
    return { code: 'failed', message: String(error == null ? 'failed' : error) };
  };

  /** One of upstream's own commands (a theme lookup), when the page needs it. */
  C.invokeRaw = function (command, args) {
    var t = tauri();
    if (!t || !t.core || typeof t.core.invoke !== 'function') return Promise.reject(new Error('no bridge'));
    return t.core.invoke(command, args);
  };

  /** Listens to an event; the handler gets the payload. Quiet when there is no bridge. */
  C.listen = function (name, handler) {
    var t = tauri();
    if (!t || !t.event || typeof t.event.listen !== 'function') return Promise.resolve(function () {});
    return t.event.listen(name, function (event) {
      handler(event ? event.payload : undefined);
    }).catch(function () {
      return function () {};
    });
  };

  /** A line in upstream's run.log (`an: ` is added by the glue). Never carries user text. */
  C.log = function (message) {
    C.call('log', { msg: String(message) }).catch(function () {});
  };

  // ---- the answer gate (AnswerGate.swift) -----------------------------------------------

  C.ARM_DELAY_MS = 350;
  C.ANSWERED_MEMORY_MS = 600000;

  /**
   * Keeps one click from answering two requests: a request can be answered once, and only
   * after it has been on screen for 0.35 s, so a click meant for the request it replaced never
   * lands on it. Pure: time comes in with every call.
   */
  C.createAnswerGate = function () {
    var firstShown = Object.create(null);
    var answered = Object.create(null);
    var gate = {
      /** Records the requests on screen now; new ones start their wait. */
      noteShown: function (ids, now) {
        var shown = Object.create(null);
        (ids || []).forEach(function (id) {
          shown[id] = true;
        });
        Object.keys(firstShown).forEach(function (id) {
          if (!shown[id]) delete firstShown[id];
        });
        Object.keys(answered).forEach(function (id) {
          if (!(now - answered[id] < C.ANSWERED_MEMORY_MS)) delete answered[id];
        });
        Object.keys(shown).forEach(function (id) {
          if (!(id in firstShown)) firstShown[id] = now;
        });
      },
      /** Records requests as shown long ago (static snapshot scenes). */
      noteShownArmed: function (ids) {
        (ids || []).forEach(function (id) {
          if (!(id in firstShown)) firstShown[id] = -Infinity;
        });
      },
      isArmed: function (id, now) {
        if (id in answered) return false;
        if (!(id in firstShown)) return false;
        return now - firstShown[id] >= C.ARM_DELAY_MS;
      },
      /** Takes the one answer `id` gets: false when answered already, not shown, or too new. */
      claim: function (id, now) {
        if (!gate.isArmed(id, now)) return false;
        answered[id] = now;
        return true;
      },
      /** When the next waiting request becomes answerable, or null. */
      nextArming: function (now) {
        var next = null;
        Object.keys(firstShown).forEach(function (id) {
          if (id in answered) return;
          var at = firstShown[id] + C.ARM_DELAY_MS;
          if (at > now && (next === null || at < next)) next = at;
        });
        return next;
      },
      wasAnswered: function (id) {
        return id in answered;
      },
    };
    return gate;
  };

  // ---- the key router (ClaudeKeyRouter.swift, Ctrl for ⌘ and Alt for ⌥) -----------------

  C.MAX_INLINE_OPTIONS = 4;

  /**
   * What the row or chat under the keyboard offers, as the router needs it.
   * actions: {kind:'none'|'permission'|'question_chips'|'answer_in_chat'|'plan'|'answer_in_terminal',
   *           toolUseId, alwaysInline, hasAlways, needsReview, options}
   */
  C.primaryActions = function (row) {
    var p = row && row.pending;
    if (!p) {
      var d = row && row.detail;
      if (row && row.bucket === 'needs_you' && !row.failed && d &&
          (d.kind === 'dialog' || d.kind === 'permission' || d.kind === 'question' || d.kind === 'plan')) {
        return { kind: 'answer_in_terminal' };
      }
      return { kind: 'none' };
    }
    if (p.kind === 'question') {
      var questions = p.questions || [];
      var q = questions.length === 1 ? questions[0] : null;
      var chips = p.single_tap && q && !q.multi_select && q.options.length >= 1 &&
        q.options.length <= C.MAX_INLINE_OPTIONS;
      if (chips) return { kind: 'question_chips', toolUseId: p.tool_use_id, question: q };
      return { kind: 'answer_in_chat', toolUseId: p.tool_use_id };
    }
    if (p.kind === 'plan') return { kind: 'plan', toolUseId: p.tool_use_id };
    return {
      kind: 'permission',
      toolUseId: p.tool_use_id,
      hasAlways: p.always != null,
      alwaysInline: !!p.inline_always && p.always != null,
      needsReview: !!p.needs_review,
    };
  };

  /** The keyboard's target for a row. */
  C.keyTarget = function (row) {
    return {
      sessionId: row.session_id,
      actions: C.primaryActions(row),
      canMarkReviewed: row.bucket === 'ready_for_review',
      canDismissFailure: !!row.failed,
      canJump: !!row.focus_label,
    };
  };

  function primaryCommand(target, inChat) {
    var a = target.actions;
    var id = target.sessionId;
    switch (a.kind) {
      case 'permission':
        if (a.needsReview && !inChat) return { cmd: 'openChat', sessionId: id };
        return { cmd: 'allow', sessionId: id, toolUseId: a.toolUseId };
      case 'plan':
        return { cmd: 'approvePlan', sessionId: id, toolUseId: a.toolUseId };
      case 'question_chips':
      case 'answer_in_chat':
        return inChat ? null : { cmd: 'openChat', sessionId: id };
      default:
        return target.canMarkReviewed ? { cmd: 'markReviewed', sessionId: id } : null;
    }
  }

  /**
   * The command for a key, or null to let it through. `key`: 'up' | 'down' | 'return' |
   * 'delete' | 'escape' | one character. `mods`: {ctrl, alt, shift}. `context`:
   * {kind:'list', target|null} | {kind:'chat', target, typing} | {kind:'setup'}.
   * Approvals never fire on a bare key: an accidental Enter opens a chat, it never allows.
   */
  C.commandFor = function (key, mods, context) {
    var m = mods || {};
    var ctrl = !!m.ctrl, alt = !!m.alt, shift = !!m.shift;
    if (key === 'escape') return context.kind === 'chat' ? { cmd: 'back' } : { cmd: 'close' };
    if (context.kind === 'setup') return null;
    var inChat = context.kind === 'chat';
    if (inChat && context.typing && !ctrl) return null;
    var target = context.target || null;
    var lower = typeof key === 'string' && key.length === 1 ? key.toLowerCase() : key;

    if (lower === 'r' && ctrl && shift && !alt) return { cmd: 'markAllReviewed' };
    if (lower === 'r' && ctrl && !shift && !alt) {
      if (!target) return null;
      if (target.canMarkReviewed) return { cmd: 'markReviewed', sessionId: target.sessionId };
      if (target.canDismissFailure) return { cmd: 'dismissFailure', sessionId: target.sessionId };
      return null;
    }
    if (lower === 'j' && ctrl && !shift && !alt) {
      return target && target.canJump ? { cmd: 'jump', sessionId: target.sessionId } : null;
    }
    if (key === 'return' && ctrl && !alt && !shift) return target ? primaryCommand(target, inChat) : null;
    if (key === 'return' && ctrl && alt && !shift) {
      // From the list only what the row offers: a narrow rule, for a request short enough to
      // be read there whole. In the chat, any suggestion.
      if (!target || target.actions.kind !== 'permission' || !target.actions.hasAlways) return null;
      if (!inChat && !(target.actions.alwaysInline && !target.actions.needsReview)) return null;
      return { cmd: 'alwaysAllow', sessionId: target.sessionId, toolUseId: target.actions.toolUseId };
    }
    if (key === 'delete' && ctrl && !alt && !shift) {
      if (!target) return null;
      if (target.actions.kind === 'permission') {
        return { cmd: 'deny', sessionId: target.sessionId, toolUseId: target.actions.toolUseId };
      }
      if (target.actions.kind === 'plan') {
        return { cmd: 'keepPlanning', sessionId: target.sessionId, toolUseId: target.actions.toolUseId };
      }
      return null;
    }
    if (inChat || ctrl || alt || shift) return null;
    if (key === 'up') return { cmd: 'move', delta: -1 };
    if (key === 'down') return { cmd: 'move', delta: 1 };
    if (key === 'return') return target ? { cmd: 'openChat', sessionId: target.sessionId } : null;
    if (typeof key === 'string' && /^[1-4]$/.test(key) && target && target.actions.kind === 'question_chips') {
      var digit = Number(key);
      if (digit <= target.actions.question.options.length) {
        return { cmd: 'chooseOption', sessionId: target.sessionId, toolUseId: target.actions.toolUseId, index: digit - 1 };
      }
    }
    return null;
  };

  /** Selection after moving `delta` rows through `order`; stops at the ends. */
  C.moveSelection = function (selection, delta, order) {
    if (!order || !order.length) return null;
    var index = order.indexOf(selection);
    if (selection == null || index < 0) return delta >= 0 ? order[0] : order[order.length - 1];
    return order[Math.min(Math.max(index + delta, 0), order.length - 1)];
  };

  /** A DOM keyboard event as the router's key; null for keys it never routes. */
  C.routerKey = function (event) {
    switch (event.key) {
      case 'ArrowUp': return 'up';
      case 'ArrowDown': return 'down';
      case 'Enter': return 'return';
      case 'Backspace':
      case 'Delete': return 'delete';
      case 'Escape': return 'escape';
      default:
        return typeof event.key === 'string' && event.key.length === 1 ? event.key : null;
    }
  };

  /**
   * WebView2's browser accelerators that collide with the panel's keys or reload the page
   * (F5, Ctrl+R, Ctrl+Shift+R, Ctrl+J, Ctrl+P, Ctrl+F). The glue turns accelerators off in the
   * WebView itself; this is the page's half, for a build or a WebView that still has them.
   */
  C.isBrowserAccelerator = function (event) {
    if (event.key === 'F5') return true;
    if (!event.ctrlKey) return false;
    var k = String(event.key || '').toLowerCase();
    return k === 'r' || k === 'j' || k === 'p' || k === 'f';
  };

  // ---- routes ---------------------------------------------------------------------------

  /** "sessions" | "session:<id>" → {kind, id}. */
  C.parseRoute = function (route) {
    var r = String(route || 'sessions');
    if (r.indexOf('session:') === 0) return { kind: 'session', id: r.slice('session:'.length) };
    return { kind: 'sessions', id: null };
  };

  // ---- the DOM morph --------------------------------------------------------------------

  function keyOf(node) {
    return node.nodeType === 1 ? node.getAttribute('data-key') : null;
  }

  function sameKind(a, b) {
    return a.nodeType === b.nodeType && (a.nodeType !== 1 || a.tagName === b.tagName);
  }

  function syncAttributes(live, next, keep) {
    var i, attr;
    for (i = 0; i < next.attributes.length; i++) {
      attr = next.attributes[i];
      if (keep && attr.name === 'value') continue;
      if (live.getAttribute(attr.name) !== attr.value) live.setAttribute(attr.name, attr.value);
    }
    for (i = live.attributes.length - 1; i >= 0; i--) {
      attr = live.attributes[i];
      if (keep && attr.name === 'value') continue;
      if (!next.hasAttribute(attr.name)) live.removeAttribute(attr.name);
    }
    var tag = live.tagName;
    if (tag === 'INPUT') {
      var type = (live.getAttribute('type') || '').toLowerCase();
      if (type === 'checkbox' || type === 'radio') live.checked = next.hasAttribute('checked');
      else if (!keep && document.activeElement !== live) live.value = next.getAttribute('value') || '';
    }
  }

  function patchNode(live, next) {
    if (live.nodeType !== 1) {
      if (live.nodeValue !== next.nodeValue) live.nodeValue = next.nodeValue;
      return;
    }
    // `data-an-keep`: something the user is editing (a draft, a name). Its attributes follow
    // the render (readonly, placeholder); its value and children are the user's.
    var keep = live.hasAttribute('data-an-keep') && next.hasAttribute('data-an-keep');
    syncAttributes(live, next, keep);
    if (keep) return;
    if (live.tagName === 'TEXTAREA') {
      if (document.activeElement !== live) live.value = next.textContent;
      return;
    }
    patchChildren(live, next);
  }

  function patchChildren(live, next) {
    var nextKids = Array.prototype.slice.call(next.childNodes);
    var keyed = Object.create(null);
    for (var c = live.firstChild; c; c = c.nextSibling) {
      var k = keyOf(c);
      if (k != null) keyed[k] = c;
    }
    var cursor = live.firstChild;
    for (var i = 0; i < nextKids.length; i++) {
      var n = nextKids[i];
      var key = keyOf(n);
      var match = null;
      if (key != null) {
        match = keyed[key] || null;
        if (match && match.tagName !== n.tagName) match = null;
        if (match) delete keyed[key];
      } else if (cursor && keyOf(cursor) == null && sameKind(cursor, n)) {
        match = cursor;
      }
      if (match) {
        if (match === cursor) cursor = cursor.nextSibling;
        else live.insertBefore(match, cursor);
        patchNode(match, n);
      } else {
        live.insertBefore(n, cursor);
      }
    }
    while (cursor) {
      var after = cursor.nextSibling;
      live.removeChild(cursor);
      cursor = after;
    }
  }

  /**
   * Makes `root`'s children equal to `html`, touching only what differs. Elements keep their
   * identity where they stay (a spinner keeps turning, a breath is not restarted, a field keeps
   * its caret); `data-key` pairs moved elements up by name.
   */
  C.morph = function (root, html) {
    var tpl = document.createElement('template');
    tpl.innerHTML = html;
    patchChildren(root, tpl.content);
  };

  /** FLIP: remembers where each `[data-flip]` is, then glides moved ones from there (320 ms). */
  C.flipFirst = function (root) {
    var rects = Object.create(null);
    root.querySelectorAll('[data-flip]').forEach(function (el) {
      rects[el.getAttribute('data-flip')] = el.getBoundingClientRect().top;
    });
    return rects;
  };
  C.flipPlay = function (root, before) {
    if (C.stillMotion()) return;
    root.querySelectorAll('[data-flip]').forEach(function (el) {
      var was = before[el.getAttribute('data-flip')];
      if (was === undefined || typeof el.animate !== 'function') return;
      var dy = was - el.getBoundingClientRect().top;
      if (Math.abs(dy) < 1) return;
      el.animate([{ transform: 'translateY(' + dy + 'px)' }, { transform: 'none' }],
        { duration: 320, easing: 'cubic-bezier(.2,.8,.2,1)' });
    });
  };

  // ---- motion and static rendering ------------------------------------------------------

  /** Snapshot scenes: timers stop and animations start settled, like the Mac's static rendering. */
  C.isStatic = function () {
    return document.documentElement.classList.contains('an-static');
  };
  C.setStatic = function (on) {
    document.documentElement.classList.toggle('an-static', !!on);
  };
  /** Nothing moves: Windows' "Animation effects" off, or a static scene. */
  C.stillMotion = function () {
    if (C.isStatic()) return true;
    try {
      return !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
    } catch (e) {
      return false;
    }
  };

  // ---- loading the other fork scripts ---------------------------------------------------

  /**
   * Loads `names` (files beside `base`) in order, then calls `done(error|null)`. Upstream's
   * notch and Settings pages take one fork script each (seams WS1, WSS5); that script pulls the
   * shared ones in with this, in order, so the pages keep a single tagged seam line.
   */
  C.load = function (base, names, done) {
    var left = names.length;
    var failed = null;
    if (!left) {
      done(null);
      return;
    }
    names.forEach(function (name) {
      var s = document.createElement('script');
      s.src = base + name;
      s.async = false;
      s.onload = function () {
        left -= 1;
        if (!left) done(failed);
      };
      s.onerror = function () {
        failed = failed || new Error(name + " didn't load");
        left -= 1;
        if (!left) done(failed);
      };
      (document.head || document.documentElement).appendChild(s);
    });
  };

  /** The folder a script was loaded from (its URL up to the last slash). */
  C.baseOf = function (src) {
    var s = String(src || '');
    var cut = s.lastIndexOf('/');
    return cut >= 0 ? s.slice(0, cut + 1) : '';
  };

  window.agentnotchCommon = C;
})();
