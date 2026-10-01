// Agent Notch's additions to upstream's notch page (DESIGN-WIN §5.2): one ring per Claude
// account, its activity arc and count badges, the Claude card's session rows, ring clicks that
// open the sessions panel, the folded notch's resting marks, the peek, and holding the notch
// open while the panel is up or a session needs you.
//
// notch.html calls four hooks at tagged seams (WS2-WS5), always behind `if(window.agentnotch)`,
// so the page works with or without this file. It is loaded before the page's own script
// (WS1): one IIFE, one global, no top-level let/const (a clash with upstream's classic-script
// globals would be a SyntaxError that kills the whole page).
//
// Upstream globals this file reads or wraps are listed in `UPSTREAM`; a node test pins that
// notch.html still declares each of them at its top level, so an upstream rename fails the
// test instead of silently switching a feature off. `foldAllowed` and `showCard` are wrapped by
// assignment, which only works because they are function declarations (global properties).
//
// Every hook answers "not mine" until the first `an:snapshot` has arrived: before that the
// page draws upstream's own Claude cell from the `usage` projection.
(function () {
  'use strict';

  var me = document.currentScript;
  var base = (function (src) {
    var s = String(src || '');
    var cut = s.lastIndexOf('/');
    return cut >= 0 ? s.slice(0, cut + 1) : '';
  })(me && me.src);

  var UPSTREAM = {
    functions: ['foldAllowed', 'showCard', 'hideCard', 'renderRing', 'renderCard', 'unfold', 'setFolded',
      'scheduleFold', 'rectOf', 'turnReading', 'settle', 'svgArc', 'headlineOf', 'weeklyOf',
      'notice', 'armWatchdog'],
    bindings: ['hoverId', 'refreshing', 'pointerIn', 'folded', 'notchEdge', 'weeklyRing', 'tone',
      'INK', 'WATCH', 'AMPLE', 'TRACK', 'PRESS_MAX', 'SESSION_ROWS'],
  };

  /** A session row ranks in the card: waiting first, then failed, busy, done, idle. */
  var CARD_RANK = { needs_you: 0, failed: 1, working: 2, review: 3, idle: 4 };
  var CARD_WORD = { needs_you: 'waiting', failed: 'failed', working: 'working', review: 'complete', idle: 'idle' };
  var CARD_RING = { needs_you: 'needs', failed: 'error', working: 'working', review: 'review', idle: 'idle' };
  /** How long a click after a peek still counts as the peek's (peekClickGrace). */
  var PEEK_CLICK_GRACE_MS = 2000;
  /** How long a ring click's own guess at the panel's state stands before the glue's word. */
  var PANEL_HINT_MS = 1500;
  var DESIGN_RING = 44;
  var BADGE_OUTSET = 11;

  var C = null;
  var started = false;
  var state = {
    snapshot: null,
    peek: null,
    lastPeek: null,
    panel: null,
    panelHint: null,
    fetched: {},
    successTimer: null,
    peekTimer: null,
  };

  // ---- pure helpers (tested in node) ----------------------------------------------------

  /** Upstream's usage status vocabulary for a ring's (never `needsAuth`: no token sign-in). */
  function upstreamStatus(status) {
    switch (status) {
      case 'ok': return 'ok';
      case 'stale': return 'stale';
      case 'failed': return 'error';
      default: return 'none';
    }
  }

  function ringNote(usage) {
    if (usage.note) return usage.note;
    if (usage.status === 'waiting') return 'Waiting for the first reading…';
    return '';
  }

  /** Upstream cell objects for the shown rings; null before a snapshot or with no account. */
  function cellsFrom(snapshot) {
    if (!snapshot || !Array.isArray(snapshot.rings) || !snapshot.rings.length) return null;
    return snapshot.rings.filter(function (r) {
      return r.shown;
    }).map(function (r) {
      var u = r.usage || {};
      return {
        id: r.ring_id,
        base: 'claude',
        name: r.label,
        glyph: 'C',
        snap: {
          status: upstreamStatus(u.status),
          windows: (u.windows || []).map(function (w) {
            return {
              id: w.id, label: w.label, used: w.used, resets_at: w.resets_at, count: w.count,
              derived: w.derived, group: w.group,
            };
          }),
          fetched_at: u.fetched_at_ms || 0,
          note: ringNote(u),
          backoff_until: 0,
        },
      };
    });
  }

  function ringOf(snapshot, id) {
    if (!snapshot || !Array.isArray(snapshot.rings)) return null;
    for (var i = 0; i < snapshot.rings.length; i++) if (snapshot.rings[i].ring_id === id) return snapshot.rings[i];
    return null;
  }

  /** Degrees clockwise from 3 o'clock (y down): the side facing the screen centre. */
  function badgeAngle(slot, edge) {
    var table = {
      right: { needs: 225, review: 135 },
      left: { needs: 315, review: 45 },
      top: { needs: 135, review: 45 },
      bottom: { needs: 225, review: 315 },
    };
    return (table[edge] || table.right)[slot];
  }

  /** A count badge's centre in the ring's own square (ClaudeRingBadgeLayout.center). */
  function badgeCentre(slot, edge, ring) {
    var d = ring || DESIGN_RING;
    var radius = d / 2 + BADGE_OUTSET * d / DESIGN_RING;
    var rad = badgeAngle(slot, edge) * Math.PI / 180;
    return { x: d / 2 + radius * Math.cos(rad), y: d / 2 + radius * Math.sin(rad) };
  }

  /** The ring's activity, settled: a success ring stops pulsing at `success_settles_at_ms`. */
  function activityOf(ring, now) {
    var a = ring.activity;
    if (a === 'success') {
      var settles = ring.success_settles_at_ms;
      return settles != null && now >= settles ? 'success_steady' : 'success';
    }
    return a === 'working' || a === 'waiting' ? a : 'idle';
  }

  /** Every session a ring counts: its own, and those without a known ring on the default one. */
  function sessionsOf(snapshot, ringId) {
    var ring = ringOf(snapshot, ringId);
    if (!ring) return [];
    return snapshot.sessions.filter(function (s) {
      return s.ring_id === ringId || (s.ring_id == null && ring.is_default);
    });
  }

  function cardState(row) {
    var s = row.card && row.card.state;
    return CARD_RANK[s] !== undefined ? s : 'idle';
  }

  // Heights of the hover card's parts in CSS px, measured in the page (layoutReport reports the
  // real card beside these so a change to the CSS shows in a test, not in a cropped card).
  var CARD = { padding: 32, head: 17, window: 50, updated: 16, note: 20, list: 21, row: 47, more: 22 };
  /** The rows upstream lists when it doesn't know its window yet (SESSION_ROWS). */
  var DEFAULT_ROWS = 5;

  /**
   * How many session rows fit under the card's other content (NotchViewModel.sessionCap): all of
   * them when they fit, else as many as leave room for "and N more", at least one. A card with
   * more windows gets no more rows than a card with fewer; before the window's height is known,
   * upstream's five.
   */
  function sessionCap(total, windowCount, budget, extra) {
    if (!(budget > 0)) return DEFAULT_ROWS;
    var fixed = CARD.padding + CARD.head + Math.max(0, windowCount) * CARD.window + (extra || 0);
    var room = budget - fixed - CARD.list;
    if (total * CARD.row <= room) return Math.max(1, total);
    return Math.max(1, Math.floor((room - CARD.more) / CARD.row));
  }

  /** The tallest the card may be: the window less its margins (upstream's max-height: 100% less 16 px, or 150 px on the flat edges, where the pill and the 30 px gap sit in the way). */
  function cardBudget(edge, height, insets) {
    var i = insets && insets.length === 4 ? insets : [0, 0, 0, 0];
    var room = height - 16 - i[0] - i[2];
    if (edge === 'top' || edge === 'bottom') room = Math.min(room, height - 150);
    return room;
  }

  /** The Claude card's session rows (UI§3.5): escaped HTML, up to `limit` then "and N more". */
  function cardRowsHtml(snapshot, ringId, now, limit) {
    var rows = sessionsOf(snapshot, ringId).slice().sort(function (a, b) {
      var ra = CARD_RANK[cardState(a)], rb = CARD_RANK[cardState(b)];
      if (ra !== rb) return ra - rb;
      var sa = a.card ? a.card.since_ms : a.since_ms, sb = b.card ? b.card.since_ms : b.since_ms;
      return sb - sa || (a.session_id < b.session_id ? -1 : 1);
    });
    if (!rows.length) return '';
    var shown = rows.slice(0, Math.max(1, limit || 5));
    var html = '<div class="c-sessions an-card-sessions">';
    shown.forEach(function (row) {
      var c = row.card || {};
      var st = cardState(row);
      var waiting = (st === 'needs_you' || st === 'failed') && c.waiting_for;
      var detail = waiting ? c.waiting_for : (c.detail || '');
      var since = c.since_ms != null ? c.since_ms : row.since_ms;
      var name = c.name || row.title;
      var label = name + ', ' + CARD_WORD[st] + (detail ? ', ' + detail : '');
      html += '<button type="button" class="an-srow" data-an-session="' + C.esc(row.session_id) + '" aria-label="' + C.esc(label) + '">' +
        '<span class="an-srow-line"><span class="an-srow-name">' + C.esc(name) + '</span>' +
        '<span class="an-srow-state an-st-' + st + '">' + C.statusRing(CARD_RING[st]) +
        '<span class="an-srow-word">' + CARD_WORD[st] + '</span></span></span>' +
        '<span class="an-srow-line an-srow-sub"><span class="an-srow-detail">' + C.esc(detail) + '</span>' +
        '<span class="an-srow-since">' + C.esc(C.cardElapsed(now - since)) + '</span></span></button>';
    });
    if (rows.length > shown.length) html += '<div class="s-more">' + C.esc('and ' + (rows.length - shown.length) + ' more') + '</div>';
    return html + '</div>';
  }

  /** The folded notch's marks, in drawing order (ClaudeAttentionPolicy.restingMarks). */
  function restingMarks(snapshot) {
    if (!snapshot || !snapshot.ui || !snapshot.ui.resting_marks) return [];
    var m = snapshot.resting_marks || {};
    var marks = [];
    if (m.needs_you) marks.push('needs');
    if (m.review) marks.push('review');
    if (m.working) marks.push('working');
    return marks;
  }

  /** Each mark's centre along the pill from its middle (ClaudeRestingMarkLayout.alongOffsets). */
  function markOffsets(marks) {
    var lengths = marks.map(function (m) {
      return m === 'needs' ? 9 : 4;
    });
    var run = lengths.reduce(function (a, b) {
      return a + b;
    }, 0) + Math.max(0, marks.length - 1) * 3;
    var cursor = -run / 2;
    return lengths.map(function (length) {
      var centre = cursor + length / 2;
      cursor += length + 3;
      return centre;
    });
  }

  /** Whether the marks fit a pill of that length and visible depth (ClaudeRestingMarkLayout.fits). */
  function marksFit(marks, pillLength, visibleDepth) {
    if (!marks.length) return true;
    var run = markOffsets(marks);
    var lengths = marks.map(function (m) {
      return m === 'needs' ? 9 : 4;
    });
    var span = run[run.length - 1] + lengths[lengths.length - 1] / 2 - (run[0] - lengths[0] / 2);
    return span + 2 <= pillLength && 4 + 1 <= visibleDepth;
  }

  /** Whether some session on a shown ring needs you (answerable or failed). */
  function shownNeedsYou(snapshot) {
    if (!snapshot || !Array.isArray(snapshot.rings)) return false;
    return snapshot.rings.some(function (r) {
      return r.shown && r.counts && (r.counts.needs_you > 0 || r.counts.failed > 0);
    });
  }

  // ---- the panel's state as the notch knows it ------------------------------------------

  /** Time comes from the shared clock (tests and static scenes pin it); upstream's own reads Date.now(). */
  function now() {
    return C ? C.now() : Date.now();
  }

  function panelOpen() {
    if (state.panelHint && now() < state.panelHint.until) return state.panelHint.open;
    return !!(state.panel && state.panel.open);
  }

  function peekActive() {
    return !!(state.peek && now() < state.peek.until);
  }

  /** The notch stays unfolded while this holds (on top of upstream's own reasons). */
  function holdsOpen() {
    if (panelOpen() || peekActive()) return true;
    var ui = state.snapshot && state.snapshot.ui;
    return !!(ui && ui.hold_open === 'always' && shownNeedsYou(state.snapshot));
  }

  // ---- upstream access ------------------------------------------------------------------

  function upstream(name) {
    var fn = window[name];
    return typeof fn === 'function' ? fn : null;
  }

  function callUpstream(name) {
    var fn = upstream(name);
    if (!fn) return undefined;
    return fn.apply(null, Array.prototype.slice.call(arguments, 1));
  }

  /** The screen edge the notch is on, from upstream's `notchEdge`. */
  function currentEdge() {
    try {
      return typeof notchEdge === 'string' ? notchEdge : 'right';
    } catch (e) {
      return 'right';
    }
  }

  function isFolded() {
    try {
      return folded === true;
    } catch (e) {
      return false;
    }
  }

  function cardShown() {
    var el = document.getElementById('card');
    return !!(el && el.classList.contains('show'));
  }

  // ---- WS2-WS5 hooks --------------------------------------------------------------------

  function claudeCells() {
    if (!C) return null;
    return cellsFrom(state.snapshot);
  }

  function activityHtml(kind) {
    var ink, watch, ample, arc;
    try {
      ink = INK;
      watch = WATCH;
      ample = AMPLE;
      arc = svgArc;
    } catch (e) {
      return '';
    }
    switch (kind) {
      case 'working': return '<g class="arc-spin">' + arc(19, 0.28, ink, 2.5) + '</g>';
      case 'waiting': return '<g class="arc-pulse"><circle cx="28" cy="28" r="19" fill="none" stroke="' + watch + '" stroke-width="2.5"/></g>';
      case 'success': return '<g class="arc-pulse"><circle cx="28" cy="28" r="19" fill="none" stroke="' + ample + '" stroke-width="2.5"/></g>';
      case 'success_steady': return '<circle cx="28" cy="28" r="19" fill="none" stroke="' + ample + '" stroke-width="2.5" opacity="0.85"/>';
      default: return '';
    }
  }

  /**
   * Upstream draws an inside weekly ring only while its own (empty) Claude state is idle; the
   * inside ring shares the gap with the activity arc, so it follows this ring's activity instead.
   */
  function fixInsideWeekly(p, cell, kind) {
    var inside;
    try {
      inside = weeklyRing === 'inside';
    } catch (e) {
      return;
    }
    if (!inside) return;
    var svg = cell.querySelector('svg.ring');
    if (!svg) return;
    svg.querySelectorAll('circle[r="16"]').forEach(function (c) {
      c.parentNode.removeChild(c);
    });
    if (kind !== 'idle') return;
    var wk = callUpstream('weeklyOf', p.snap, 'claude');
    var h = callUpstream('headlineOf', p.snap, 'claude');
    if (!wk || (h && wk.id === h.id)) return;
    try {
      svg.insertAdjacentHTML('beforeend', '<circle cx="28" cy="28" r="16" fill="none" stroke="' + TRACK +
        '" stroke-width="2.4" opacity="0.7"/>' + svgArc(16, Math.min(wk.used, 1), tone(wk.used), 2.4, 'opacity="0.85"'));
    } catch (e) {
      // The inside ring is decoration; the reading and the percent are drawn already.
    }
  }

  function placeBadges(wrap, ring, edge, show) {
    var size = wrap.offsetWidth || DESIGN_RING;
    ['needs', 'review'].forEach(function (slot) {
      var count = slot === 'needs' ? ring.badges.needs_you : ring.badges.review;
      var label = show ? C.badgeLabel(count) : '';
      var el = wrap.querySelector('.an-badge-' + slot);
      if (!label) {
        if (el) el.parentNode.removeChild(el);
        return;
      }
      if (!el) {
        el = document.createElement('span');
        el.className = 'an-badge an-badge-' + slot;
        el.setAttribute('aria-hidden', 'true');
        wrap.appendChild(el);
      }
      var c = badgeCentre(slot, edge, size);
      el.style.left = c.x.toFixed(2) + 'px';
      el.style.top = c.y.toFixed(2) + 'px';
      if (el.textContent !== label) el.textContent = label;
    });
  }

  /**
   * The badges follow the notch's edge, which arrives on its own (a move, a placement): upstream
   * redraws nothing then, so the badges are placed again here.
   */
  function replaceBadges() {
    var snap = state.snapshot;
    if (!C || !snap) return;
    var show = !!(snap.ui && snap.ui.ring_badges);
    document.querySelectorAll('#pill .cell.an-claude').forEach(function (cell) {
      var ring = ringOf(snap, cell.getAttribute('data-p'));
      var wrap = cell.querySelector('.ringwrap');
      if (ring && wrap) placeBadges(wrap, ring, currentEdge(), show);
    });
  }

  function decorateCell(p, cell) {
    if (!C || !state.snapshot || !p || p.base !== 'claude' || !cell) return;
    var ring = ringOf(state.snapshot, p.id);
    if (!ring) return;
    var kind = activityOf(ring, now());
    var layer = cell.querySelector('svg.activity');
    if (layer) layer.innerHTML = activityHtml(kind);
    fixInsideWeekly(p, cell, kind);
    var wrap = cell.querySelector('.ringwrap');
    if (wrap) {
      // The engine's freshness rule, not upstream's 15 minutes: with probes every 30 minutes a
      // reading 20 minutes old is current (AU§8.4). This runs after upstream's own toggle.
      wrap.classList.toggle('stale', !!(ring.usage && ring.usage.stale));
      placeBadges(wrap, ring, currentEdge(), !!(state.snapshot.ui && state.snapshot.ui.ring_badges));
    }
    cell.classList.add('an-claude');
    if (ring.a11y) cell.setAttribute('aria-label', ring.a11y);
  }

  function cardSessions(p) {
    if (!C || !state.snapshot || !p) return '';
    var total = sessionsOf(state.snapshot, p.id).length;
    var windows = p.snap && Array.isArray(p.snap.windows) ? p.snap.windows.length : 0;
    var limit = sessionCap(total, windows, cardBudget(currentEdge(), window.innerHeight || 0, upstreamInsets()), staleExtra(p));
    return cardRowsHtml(state.snapshot, p.id, now(), limit);
  }

  /** The card's own extra line when the reading is old ("Updated 3h ago"). */
  function staleExtra(p) {
    var ring = ringOf(state.snapshot, p.id);
    return ring && ring.usage && ring.usage.stale && ring.usage.fetched_at_ms ? CARD.updated : 0;
  }

  /** Upstream's `insets` (the taskbar's share of the window), read by name: not a window property. */
  function upstreamInsets() {
    try {
      return insets;
    } catch (e) {
      return null;
    }
  }

  function ringClick(cellId) {
    if (!C || !state.snapshot) return false;
    var ring = ringOf(state.snapshot, cellId);
    if (!ring) return false;
    if (state.snapshot.ui && state.snapshot.ui.ring_click === 'refreshUsage') {
      refreshRing(cellId);
      return true;
    }
    var cellEl = null;
    document.querySelectorAll('#pill .cell').forEach(function (el) {
      if (el.getAttribute('data-p') === cellId) cellEl = el;
    });
    var wrap = cellEl && cellEl.querySelector('.ringwrap');
    var rect = wrap ? callUpstream('rectOf', wrap) : null;
    // The glue toggles: the same ring's panel closes, any other opens (or moves) there. Until it
    // says what it did, guess the same, so the card doesn't flash up over the opening panel.
    var closing = panelOpen() && ((state.panelHint && state.panelHint.ring_id === cellId) ||
      (state.panel && state.panel.ring_id === cellId));
    state.panelHint = { open: !closing, ring_id: cellId, until: now() + PANEL_HINT_MS };
    if (!closing) callUpstream('hideCard');
    var args = { ring_id: cellId, reason: 'ring_click' };
    if (Array.isArray(rect)) args.rect = rect.map(function (n) {
      return Math.round(n);
    });
    C.call('panel_toggle', args).catch(function (e) {
      state.panelHint = null;
      C.log('panel_toggle failed: ' + (e && e.code));
    });
    return true;
  }

  /** "Refreshes its usage": upstream's press, then the engine's probe for this ring. */
  function refreshRing(id) {
    var pending = null;
    try {
      pending = refreshing;
    } catch (e) {
      pending = null;
    }
    if (pending && pending[id]) return;
    var maxMs = 6000;
    try {
      maxMs = PRESS_MAX;
    } catch (e) {
      // upstream's fallback length when it has one
    }
    if (pending) {
      pending[id] = {
        at: now(),
        timer: setTimeout(function () {
          callUpstream('settle', id);
        }, maxMs),
      };
    }
    callUpstream('renderRing');
    callUpstream('turnReading', id);
    C.call('refresh_usage', { ring_id: id, reason: 'ring_click' }).then(function (reply) {
      if (!reply || !reply.coming) callUpstream('settle', id);
    }, function () {
      callUpstream('settle', id);
    });
  }

  // ---- session rows in the card ---------------------------------------------------------

  function rowById(id) {
    var s = state.snapshot;
    if (!s) return null;
    for (var i = 0; i < s.sessions.length; i++) if (s.sessions[i].session_id === id) return s.sessions[i];
    return null;
  }

  /** A click on a card row: Smart sends one that needs you to the panel, the rest to their terminal. */
  function sessionClicked(id) {
    var row = rowById(id);
    if (!row || !C) return;
    var fromPeek = !!(state.lastPeek && now() < state.lastPeek.until + PEEK_CLICK_GRACE_MS);
    var reason = fromPeek ? 'peek_click' : 'hover_row';
    var setting = state.snapshot.ui ? state.snapshot.ui.hover_click : 'smart';
    var toPanel = setting === 'panel' || (setting !== 'terminal' && row.bucket === 'needs_you');
    function openPanel() {
      state.panelHint = { open: true, ring_id: row.ring_id, until: now() + PANEL_HINT_MS };
      callUpstream('hideCard');
      var args = { route: 'session:' + id, reason: reason };
      if (row.ring_id) args.ring_id = row.ring_id;
      C.call('panel_open', args).catch(function (e) {
        state.panelHint = null;
        C.log('panel_open failed: ' + (e && e.code));
      });
    }
    if (toPanel || !row.focus_label) {
      openPanel();
      return;
    }
    // No terminal to bring forward (it closed, or can't be found): the panel still has it.
    C.call('focus', { session_id: id }).then(function (reply) {
      var outcome = reply && reply.outcome;
      if (outcome !== 'focused' && outcome !== 'raised_only') openPanel();
    }, openPanel);
  }

  // ---- the folded notch's marks ---------------------------------------------------------

  function renderMarks() {
    var root = document.getElementById('root');
    if (!root) return;
    var host = document.getElementById('an-marks');
    if (!host) {
      host = document.createElement('div');
      host.id = 'an-marks';
      host.setAttribute('aria-hidden', 'true');
      var rest = document.getElementById('rest');
      if (rest && rest.parentNode === root) root.insertBefore(host, rest.nextSibling);
      else root.appendChild(host);
    }
    var marks = restingMarks(state.snapshot);
    var offsets = markOffsets(marks);
    var key = state.snapshot && state.snapshot.resting_marks ? state.snapshot.resting_marks.needs_you_key : 0;
    var html = marks.map(function (m, i) {
      // A new needs-you key is a new element: its finite breath runs again.
      var k = m === 'needs' ? 'needs-' + C.esc(key) : m;
      return '<span class="an-mark an-mark-' + m + '" data-key="' + k + '" style="--an-along:' + offsets[i].toFixed(1) + 'px"></span>';
    }).join('');
    C.morph(host, html);
  }

  // ---- events ---------------------------------------------------------------------------

  function settlePresses(snapshot) {
    var pending;
    try {
      pending = refreshing;
    } catch (e) {
      return;
    }
    snapshot.rings.forEach(function (r) {
      var at = r.usage ? r.usage.fetched_at_ms : 0;
      var before = state.fetched[r.ring_id];
      if (before !== undefined && at !== before && pending && pending[r.ring_id]) callUpstream('settle', r.ring_id);
      state.fetched[r.ring_id] = at;
    });
  }

  /** Redraws when the next "just finished" ring settles to steady. */
  function scheduleSuccessSettle(snapshot) {
    clearTimeout(state.successTimer);
    var at = now();
    var next = null;
    snapshot.rings.forEach(function (r) {
      var t = r.activity === 'success' ? r.success_settles_at_ms : null;
      if (t != null && t > at && (next === null || t < next)) next = t;
    });
    if (next !== null) {
      state.successTimer = setTimeout(function () {
        callUpstream('renderRing');
      }, Math.min(next - at + 50, 2147483000));
    }
  }

  function applySnapshot(snapshot) {
    if (!snapshot || !Array.isArray(snapshot.rings) || !Array.isArray(snapshot.sessions)) return;
    var held = holdsOpen();
    state.snapshot = snapshot;
    settlePresses(snapshot);
    scheduleSuccessSettle(snapshot);
    callUpstream('renderRing');
    if (cardShown()) {
      callUpstream('renderCard');
      callUpstream('armWatchdog');
    }
    renderMarks();
    var holds = holdsOpen();
    if (holds && isFolded()) callUpstream('unfold');
    else if (held && !holds) callUpstream('scheduleFold');
  }

  /** The panel's window state, as the glue reports it (`an:panel_state`, a `PanelState`). */
  function applyPanelState(payload) {
    if (!payload || typeof payload !== 'object') return;
    var was = panelOpen();
    state.panel = { open: !!payload.open, ring_id: payload.ring_id == null ? null : payload.ring_id };
    state.panelHint = null;
    var now = panelOpen();
    if (now && cardShown()) callUpstream('hideCard');
    if (now && isFolded()) callUpstream('unfold');
    else if (was && !now) callUpstream('scheduleFold');
  }

  function peek(payload) {
    if (!payload || !payload.ring_id || !state.snapshot || panelOpen()) return;
    if (!ringOf(state.snapshot, payload.ring_id)) return;
    var ms = Math.max(1, Number(payload.seconds) || 5) * 1000;
    state.peek = { ring: payload.ring_id, until: now() + ms };
    state.lastPeek = state.peek;
    callUpstream('unfold');
    try {
      hoverId = payload.ring_id;
    } catch (e) {
      return;
    }
    callUpstream('showCard');
    clearTimeout(state.peekTimer);
    state.peekTimer = setTimeout(function () {
      state.peek = null;
      var inside = false;
      try {
        inside = pointerIn === true;
      } catch (e) {
        inside = false;
      }
      if (!inside) callUpstream('hideCard');
      callUpstream('scheduleFold');
    }, ms);
  }

  function wrapUpstream() {
    var fold = upstream('foldAllowed');
    if (fold && !fold.__agentnotch) {
      var wrappedFold = function () {
        return fold.apply(this, arguments) && !holdsOpen();
      };
      wrappedFold.__agentnotch = true;
      window.foldAllowed = wrappedFold;
    }
    var show = upstream('showCard');
    if (show && !show.__agentnotch) {
      // The sessions panel says what the card would, and a card over it would sit under the
      // pointer that is using the panel (ClaudePanelController.setHoverCardsSuppressed).
      var wrappedShow = function () {
        if (panelOpen()) return undefined;
        return show.apply(this, arguments);
      };
      wrappedShow.__agentnotch = true;
      window.showCard = wrappedShow;
    }
  }

  function onCardClick(event) {
    var target = event.target && event.target.closest ? event.target.closest('[data-an-session]') : null;
    if (!target) return;
    event.preventDefault();
    event.stopPropagation();
    sessionClicked(target.getAttribute('data-an-session'));
  }

  /** Sealed snapshot scenes (§7.4): the notch open, with the first Claude ring's card, or folded. */
  function showScene(name) {
    if (!C || (name !== 'notch-open' && name !== 'notch-card' && name !== 'notch-folded')) return false;
    C.setStatic(true);
    var cells = cellsFrom(state.snapshot) || [];
    if (name === 'notch-folded') {
      callUpstream('hideCard');
      callUpstream('setFolded', true);
      return true;
    }
    callUpstream('setFolded', false);
    if (name === 'notch-card' && cells.length) {
      try {
        hoverId = cells[0].id;
      } catch (e) {
        return false;
      }
      callUpstream('showCard');
      return true;
    }
    callUpstream('hideCard');
    return name === 'notch-open';
  }

  function rectInside(inner, outer, slack) {
    var s = slack || 0;
    return inner.left >= outer.left - s && inner.top >= outer.top - s &&
      inner.right <= outer.right + s && inner.bottom <= outer.bottom + s;
  }

  function overlaps(a, b) {
    return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
  }

  /** The text box of a percent label: its glyphs, not the block the flex column gives it. */
  function textBox(el) {
    try {
      var range = document.createRange();
      range.selectNodeContents(el);
      var r = range.getBoundingClientRect();
      if (r && r.width) return r;
    } catch (e) {
      // the element's own box, then
    }
    return el.getBoundingClientRect();
  }

  /**
   * The notch's layout invariants (§5.6), measured in the page for the sealed self-test: badges
   * inside the pill and clear of the percent label on the flat edges, card rows inside the
   * card, resting marks inside the folded pill.
   */
  function layoutReport() {
    var failures = [];
    var pillEl = document.getElementById('pill');
    var edge = currentEdge();
    var flat = edge === 'top' || edge === 'bottom';
    var badges = 0;
    if (pillEl && !isFolded()) {
      var pr = pillEl.getBoundingClientRect();
      pillEl.querySelectorAll('.cell').forEach(function (cellEl) {
        var pct = cellEl.querySelector('.pct');
        cellEl.querySelectorAll('.an-badge').forEach(function (b) {
          badges += 1;
          var br = b.getBoundingClientRect();
          if (!rectInside(br, pr, 0.5)) failures.push('badge outside the pill: ' + cellEl.getAttribute('data-p'));
          if (flat && pct && overlaps(br, textBox(pct))) {
            failures.push('badge over the percent label: ' + cellEl.getAttribute('data-p'));
          }
        });
      });
    }
    var cardEl = document.getElementById('card');
    var card = null;
    if (cardEl && cardEl.classList.contains('show')) {
      var cr = cardEl.getBoundingClientRect();
      var rows = cardEl.querySelectorAll('[data-an-session]');
      rows.forEach(function (row) {
        var rr = row.getBoundingClientRect();
        if (rr.height && rr.top < cr.bottom && !rectInside(rr, cr, 0.5)) failures.push('card row outside the card');
      });
      if (cr.height && (cr.top < -0.5 || cr.bottom > window.innerHeight + 0.5)) failures.push('card outside the window');
      // The card's heights in the page, for keeping CARD (the estimate sessionCap uses) honest.
      var first = rows[0] && rows[0].getBoundingClientRect();
      var parts = [];
      Array.prototype.forEach.call(cardEl.children, function (el) {
        parts.push({ cls: el.className, height: el.getBoundingClientRect().height });
      });
      card = { height: cr.height, rows: rows.length, rowHeight: first ? first.height : 0, parts: parts,
        scrolls: cardEl.scrollHeight > cardEl.clientHeight + 1 };
      if (card.scrolls) failures.push('card scrolls: its rows do not fit');
    }
    var marks = document.getElementById('an-marks');
    var rest = document.getElementById('rest');
    if (marks && rest && isFolded()) {
      var restRect = rest.getBoundingClientRect();
      marks.querySelectorAll('.an-mark').forEach(function (m) {
        if (!rectInside(m.getBoundingClientRect(), restRect, 0.5)) failures.push('resting mark outside the folded pill');
      });
    }
    return { ok: failures.length === 0, failures: failures, edge: edge, badges: badges, card: card };
  }

  function start() {
    if (started) return;
    C = window.agentnotchCommon;
    if (!C) return;
    started = true;
    wrapUpstream();
    var cardEl = document.getElementById('card');
    if (cardEl) cardEl.addEventListener('click', onCardClick);
    if (typeof MutationObserver === 'function' && document.body) {
      new MutationObserver(replaceBadges).observe(document.body, { attributes: true, attributeFilter: ['data-edge'] });
    }
    C.listen('an:snapshot', applySnapshot);
    C.listen('an:peek', peek);
    C.listen('an:panel_state', applyPanelState);
    C.listen('an:notice', function (text) {
      if (typeof text === 'string' && text) callUpstream('notice', text);
    });
    C.call('snapshot').then(applySnapshot, function (e) {
      C.log('notch: snapshot failed: ' + (e && e.code));
    });
  }

  window.agentnotch = {
    claudeCells: claudeCells,
    decorateCell: decorateCell,
    cardSessions: cardSessions,
    ringClick: ringClick,
    showScene: showScene,
    layoutReport: layoutReport,
    /** For tests and the sealed self-test: the helpers above, pure where they can be. */
    _: {
      UPSTREAM: UPSTREAM,
      cellsFrom: cellsFrom,
      badgeAngle: badgeAngle,
      badgeCentre: badgeCentre,
      activityOf: activityOf,
      activityHtml: activityHtml,
      cardRowsHtml: cardRowsHtml,
      restingMarks: restingMarks,
      markOffsets: markOffsets,
      marksFit: marksFit,
      sessionCap: sessionCap,
      cardBudget: cardBudget,
      CARD: CARD,
      shownNeedsYou: shownNeedsYou,
      upstreamStatus: upstreamStatus,
      state: state,
      holdsOpen: holdsOpen,
      panelOpen: panelOpen,
      applySnapshot: applySnapshot,
      applyPanelState: applyPanelState,
      peek: peek,
      sessionClicked: sessionClicked,
      start: start,
    },
  };

  // The shared library first (one seam line brings in one file), then the page: upstream's
  // script has to have run before its globals can be wrapped.
  var loaded = !!window.agentnotchCommon;
  var parsed = document.readyState !== 'loading';
  function maybeStart() {
    if (loaded && parsed) start();
  }
  if (!loaded) {
    var s = document.createElement('script');
    s.src = base + 'common.js';
    s.async = false;
    s.onload = function () {
      loaded = true;
      maybeStart();
    };
    s.onerror = function () {
      // Without the library the hooks keep answering "not mine": upstream's own Claude cell.
    };
    (document.head || document.documentElement).appendChild(s);
  }
  if (!parsed) {
    document.addEventListener('DOMContentLoaded', function () {
      parsed = true;
      maybeStart();
    }, { once: true });
  } else {
    maybeStart();
  }
})();
