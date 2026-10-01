// The second half of Agent Notch's Claude Code pane (DESIGN-WIN §5.4; the Mac's
// SettingsPaneContent.swift usage/attention/notifications/advanced sections and CloudSection.swift):
// Usage, Cloud, Sessions and attention, Notifications, Advanced.
//
// settings.js loads this file itself (`agentnotchCommon.load`), after it has made its regions, and
// this file fills them through `agentnotchSettings.slots`. One strict-mode IIFE, one global
// (`window.agentnotchSettingsSections`: the pure copy functions, for the tests), no top-level
// let/const. Everything is drawn from the settings snapshot; the page keeps only view state (a
// confirmation that is asking, a setting just clicked).
//
// Writes. A setting goes out as `set_setting` with a key of DESIGN-WIN §4.12 and a value from this
// file's own option tables, from a click on its own control. The value shows at once and stands
// for 2 s after the reply (the new snapshot may still be on its way); a refusal (invalid, sealed)
// puts the snapshot's value back and says why. The cloud switches are consent gates for uploads:
// they act only on their own click and show what the snapshot reports, never a guess. No call is
// made on load. The page never sees a website token (the cloud state has none) and never logs the
// email.
(function () {
  'use strict';

  var S = window.agentnotchSettings;
  var C = window.agentnotchCommon;
  if (!S || !C) return;

  /** A set_setting's value stands this long after its reply, for a snapshot still in flight. */
  var HOLD_MS = 2000;
  var NAME_CHARS = 200;
  var TEXT_CHARS = 2000;

  /** Word for word the Mac's copy, with Windows' words (this PC, Ctrl+Alt, the tray). */
  var COPY = {
    autoOpenDetail: {
      never: 'The rings and the chime still tell you.',
      needsInput: "When a session needs you, unless you're already in its terminal.",
      needsInputOrDone: "Also when a session is done, unless you're already in its terminal.",
    },
    fullScreen: ' Never over a full-screen app.',
    sessionClick: 'Smart opens the panel when the session needs you, and its terminal otherwise.',
    hotkeyTaken: 'That shortcut is taken by another app.',
    typeReplies: 'Types your reply into the session’s console and presses Enter. Works for Windows Terminal, VS Code’s terminal and console windows; not for a Claude started in the background of another shell (start /b).',
    blockfile: "Claude Desktop's cache on this PC uses a format this version can't read.",
    noDesktopCache: "Claude Desktop's cache isn't on this PC.",
    sessionState: 'A plain-text line per session for bug reports: its title, state and tasks, and the start of a finished reply. Look it over before sharing it.',
    reviewQueue: 'Marks every finished session reviewed.',
    // CloudSettingsCopy
    noWebsite: 'None',
    websiteOverridden: 'Set by AGENTNOTCH_WEB_URL for this run.',
    fileNote: 'The website sign-in is kept in a file only your user can read.',
    dashboard: 'Your sessions and usage from every computer and account you sync. Share an account there with a code: everyone in its pool sees what it was used for.',
    dataSettingsBefore: "Remove summaries or delete synced data in the website's ",
    dataSettingsLink: 'Settings',
    summariesDetail: 'Only for sessions that end after you turn this on. Runs Claude Code on this PC ' +
      "with the session's own account, so it uses that account's usage: a few cents per session with Haiku, at " +
      "most $0.10 a run, 20 an hour and 60 a day, and none while that account's 5-hour limit is 80% used. Sends " +
      'the one- or two-sentence summary with the session, with file paths cut to their last part and anything ' +
      'like a key or password removed. Turning this off deletes the summaries not sent yet; summaries already ' +
      'sent stay on the website.',
  };

  // ---- the settings this pane writes, and the words for each value ----------------------------

  /** key -> {region, read(settings), options [[value, label]] (none for a switch)}. */
  var SETTINGS = {
    usageProbeIntervalMinutes: { region: 'usage', read: function (s) { return obj(s.usage).interval_minutes; } },
    readsDesktopUsageCache: { region: 'usage', read: function (s) { return obj(s.usage).desktop_cache; } },
    autoOpen: {
      region: 'attention', read: function (s) { return obj(s.attention).panel_open_mode; },
      options: [['never', 'Never'], ['needsInput', 'When a session needs you'], ['needsInputOrDone', 'When one needs you or is done']],
    },
    holdOpenWhileNeedsYou: {
      region: 'attention', read: function (s) { return obj(s.attention).hold_open; },
      options: [['never', 'Never'], ['always', 'Always']],
    },
    ringBadges: { region: 'attention', read: function (s) { return obj(s.attention).ring_badges; } },
    restingMarks: { region: 'attention', read: function (s) { return obj(s.attention).resting_marks; } },
    trayBadge: { region: 'attention', read: function (s) { return obj(s.attention).tray_badge; } },
    ringClick: {
      region: 'attention', read: function (s) { return obj(s.attention).ring_click; },
      options: [['openPanel', 'Opens its sessions'], ['refreshUsage', 'Refreshes its usage']],
    },
    sessionClick: {
      region: 'attention', read: function (s) { return obj(s.attention).hover_click; },
      options: [['smart', 'Smart'], ['panel', 'Opens the panel'], ['terminal', 'Shows the terminal']],
    },
    hotKey: {
      region: 'attention', read: function (s) { return obj(s.attention).hotkey; },
      options: [['off', 'Off'], ['ctrlAltSpace', 'Ctrl+Alt+Space'], ['ctrlAltJ', 'Ctrl+Alt+J']],
    },
    sound: { region: 'attention', read: function (s) { return obj(s.attention).sound; } },
    peek: { region: 'attention', read: function (s) { return obj(s.attention).peek; } },
    peekSeconds: {
      region: 'attention', read: function (s) { return obj(s.attention).peek_seconds; },
      options: [[3, '3 s'], [5, '5 s'], [10, '10 s']],
    },
    typeReplies: { region: 'attention', read: function (s) { return obj(s.attention).type_replies; } },
    notifyNeedsInput: { region: 'notifications', read: function (s) { return obj(s.notifications).notify_needs_input; } },
    notifyReadyForReview: { region: 'notifications', read: function (s) { return obj(s.notifications).notify_ready_for_review; } },
  };

  /** "Check usage every": the snapshot's options (0 is Off), else the Mac's. */
  var INTERVALS = [0, 5, 10, 15, 30];

  function intervalOptions(s) {
    var given = list(obj(s.usage).interval_options).filter(function (n) { return INTERVALS.indexOf(n) >= 0; });
    return (given.length ? given : INTERVALS).map(function (n) { return [n, n === 0 ? 'Off' : n + ' min']; });
  }

  /** A setting's values and their words; null for a switch. */
  function optionsOf(s, key) {
    return key === 'usageProbeIntervalMinutes' ? intervalOptions(s) : SETTINGS[key].options || null;
  }

  // ---- small helpers ---------------------------------------------------------------------------

  var A = null;

  function has(o, key) {
    return !!o && Object.prototype.hasOwnProperty.call(o, key);
  }

  function obj(value) {
    return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  }

  function list(value) {
    return Array.isArray(value) ? value : [];
  }

  function str(value) {
    return typeof value === 'string' && value ? value : null;
  }

  function esc(value) {
    return C.esc(value);
  }

  function line(value, max) {
    return A.line(value, max || NAME_CHARS);
  }

  function plural(n, word, many) {
    return n + ' ' + (n === 1 ? word : (many || word + 's'));
  }

  function count(value) {
    var n = Math.floor(Number(value));
    return isFinite(n) && n > 0 ? n : 0;
  }

  function sec(title) {
    return '<div class="sec" data-key="title">' + esc(title) + '</div>';
  }

  /** A row: its words on the left (a label and its captions), its control on the right. */
  function row(key, label, captions, control, cls) {
    return '<div class="item' + (cls ? ' ' + cls : '') + '"' + A.attr('data-key', key) + '><div class="an-col"><span>' + esc(label) + '</span>' +
      (captions || '') + '</div>' + (control || '') + '</div>';
  }

  /** A stepped busy ring, still in snapshots and with reduced motion. */
  function busy(label) {
    return '<span class="an-busy" role="progressbar"' + A.attr('aria-label', label) + '></span>';
  }

  // ---- settings: what shows, and the write --------------------------------------------------------

  var holds = Object.create(null);
  var view = { signOutAsk: false, resetAsk: false };

  /** What a setting's control shows: a value just clicked (while it stands), else the snapshot's. */
  function shown(s, key) {
    var snap = SETTINGS[key].read(s);
    var h = holds[key];
    if (!h) return snap;
    if (h.settledAt !== null && (snap === h.value || C.now() - h.settledAt > HOLD_MS)) {
      delete holds[key];
      return snap;
    }
    return h.value;
  }

  function allowed(s, key, value) {
    if (!has(SETTINGS, key)) return false;
    var options = optionsOf(s, key);
    if (!options) return typeof value === 'boolean';
    return options.some(function (o) { return o[0] === value; });
  }

  function setSetting(key, value) {
    var s = currentSettings();
    if (!s || A.state.scene || !allowed(s, key, value) || A.state.pending['set:' + key]) return;
    holds[key] = { value: value, settledAt: null };
    A.write(SETTINGS[key].region, 'set:' + key, 'set_setting', { key: key, value: value }).then(function (reply) {
      var h = holds[key];
      if (!h || h.value !== value) return;
      if (!reply) {
        // Refused (invalid, sealed): the control goes back to what the snapshot says.
        delete holds[key];
        A.render();
        return;
      }
      h.settledAt = C.now();
      window.setTimeout(function () { A.render(); }, HOLD_MS + 10);
    });
  }

  function currentSettings() {
    return A ? (A.state.scene ? A.state.sceneSettings : A.state.settings) : null;
  }

  function settingSwitch(s, key, label, opts) {
    var o = opts || {};
    return A.toggle(shown(s, key) === true, label, 'an-set-switch', key, { key: 'set:' + key, disabled: o.disabled, title: o.title });
  }

  /** Upstream's segmented control: one button per value, the shown one `on`. */
  function seg(s, key, label, opts) {
    var o = opts || {};
    var value = shown(s, key);
    var pending = !!A.state.pending['set:' + key];
    return '<span class="seg' + (o.column ? ' an-seg-col' : '') + '" role="radiogroup"' + A.attr('aria-label', label) + A.attr('data-key', 'seg:' + key) + '>' +
      optionsOf(s, key).map(function (option) {
        var on = option[0] === value;
        return '<button type="button"' + (on ? ' class="on"' : '') + ' role="radio"' + A.attr('aria-checked', on ? 'true' : 'false') +
          A.attr('data-key', 'v:' + option[0]) + A.attr('data-an-action', 'an-set-seg') + A.attr('data-an-arg', key) +
          A.attr('data-an-value', String(option[0])) + (pending || o.disabled ? ' disabled' : '') + '>' + esc(option[1]) + '</button>';
      }).join('') + '</span>';
  }

  // ---- 4. Usage (SettingsPaneContent.usageSection) ----------------------------------------------

  function usageHtml(s) {
    var u = obj(s.usage);
    var html = sec('Usage') + '<div class="group an-usage" data-key="group">';
    var caption = str(u.interval_caption);
    html += row('interval', 'Check usage every', caption ? A.caption(line(caption, TEXT_CHARS), '', ' data-key="cap" data-user') : '',
      seg(s, 'usageProbeIntervalMinutes', 'Check usage every'));

    var desktop = shown(s, 'readsDesktopUsageCache') === true;
    var format = str(u.desktop_format);
    var formatLine = !desktop ? '' : format === 'blockfile' ? A.caption(COPY.blockfile, 'an-tone-warning', ' data-key="format"')
      : format === 'absent' ? A.caption(COPY.noDesktopCache, 'an-tertiary', ' data-key="format"') : '';
    var desktopCaption = str(u.desktop_caption) || 'Claude Desktop keeps the limits it last saw on disk; no token is involved.';
    html += row('desktop', "Also read Claude Desktop's cached usage",
      A.caption(line(desktopCaption, TEXT_CHARS), '', ' data-key="cap" data-user') + formatLine,
      settingSwitch(s, 'readsDesktopUsageCache', "Also read Claude Desktop's cached usage"));

    list(u.accounts).forEach(function (a, i) {
      if (!a || typeof a !== 'object') return;
      var name = line(a.label || 'Claude');
      html += '<div class="item an-usage-acct"' + A.attr('data-key', 'acct:' + (str(a.ring_id) || i)) + '>' +
        '<span class="an-acct-name">' + A.dot(a.color_index) + '<span class="an-name" data-an-text data-an-clip data-user>' + esc(name) + '</span></span>' +
        '<span class="an-value an-nums" data-an-text data-an-clip data-user' + A.attr('title', line(a.line, 300)) + '>' + esc(line(a.line, 300)) + '</span></div>';
    });

    var refreshing = !!u.refreshing || !!A.state.pending.refresh;
    html += '<div class="item an-end" data-key="refresh"><span class="an-btns an-right">' +
      (refreshing ? busy('Refreshing usage') : '') +
      A.button('Refresh now', 'an-refresh', null, { key: 'refresh', disabled: refreshing }) + '</span></div>';
    return html + A.note('usage') + '</div>';
  }

  // ---- 5. Cloud (CloudSection.swift, CloudSettingsCopy) ---------------------------------------

  /** The auth state as `{kind, email, message}`: serde writes a unit variant as a bare string. */
  function authOf(cloud) {
    var a = cloud.auth;
    if (typeof a === 'string') return { kind: a, email: null, message: null };
    a = obj(a);
    var kinds = ['signed_in', 'signing_in', 'error', 'signed_out'];
    for (var i = 0; i < kinds.length; i++) {
      if (has(a, kinds[i])) {
        var inner = obj(a[kinds[i]]);
        return { kind: kinds[i], email: str(inner.email), message: str(inner.message) };
      }
    }
    return { kind: 'signed_out', email: null, message: null };
  }

  /** The website as the row shows it: without the `https://` every website has; `http://` kept. */
  function websiteDisplay(website) {
    var w = String(website);
    return w.toLowerCase().indexOf('https://') === 0 ? w.slice(8) : w;
  }

  function signInDetail(cloud) {
    if (authOf(cloud).kind === 'signing_in') return 'Finish signing in in your browser.';
    if (!str(cloud.website_url)) return "This build has no website, so it can't sign in or sync.";
    return "Opens Google's sign-in in your browser. " + COPY.fileNote;
  }

  function signInProblem(cloud) {
    var auth = authOf(cloud);
    if (auth.kind === 'error') return auth.message || str(cloud.last_error);
    return str(cloud.last_error);
  }

  function syncDetail(readsDesktopUsage) {
    var desktop = readsDesktopUsage ? "Claude Desktop's readings too" : "Claude Desktop's too, once reading its cache is on";
    return 'Sends project folder names, session titles, models, times, token counts, cost, and usage limits ' +
      'for each signed-in Claude account (' + desktop + '). Never file paths, prompts or your Claude login.';
  }

  /**
   * Why summaries don't run, or how many were written. Unlike the Mac, "not in this run" also shows
   * while the switch is off: the switch can't be turned on then, and this says why.
   */
  function summariesNote(cloud) {
    if (!cloud.sync_enabled) return 'Needs sync.';
    if (!cloud.summaries_available) return "Not in this run: it can't start Claude Code.";
    var n = count(cloud.summarized_sessions);
    if (!cloud.summaries_enabled || !n) return null;
    return n === 1 ? '1 session summarised on this PC.' : n + ' sessions summarised on this PC.';
  }

  function waiting(sessions, readings) {
    var parts = [];
    var s = count(sessions);
    var r = count(readings);
    if (s) parts.push(s === 1 ? '1 session' : s + ' sessions');
    if (r) parts.push(r === 1 ? '1 usage reading' : r + ' usage readings');
    return parts.length ? parts.join(' and ') + ' to send.' : null;
  }

  /** The sync row: when it last worked, what went wrong, what waits. */
  function status(cloud, nowMs) {
    if (!cloud.sync_enabled) return { title: 'Sync is off', detail: 'Nothing is sent while it is off.', isProblem: false };
    var error = str(cloud.last_error);
    var last = Number(cloud.last_sync_at_ms);
    var title = cloud.is_syncing ? 'Syncing…'
      : error ? 'Last sync failed'
        : cloud.last_sync_at_ms != null && isFinite(last) ? 'Last synced ' + C.age(nowMs - last)
          : 'Not synced yet';
    if (error && !cloud.is_syncing) return { title: title, detail: error, isProblem: true };
    return { title: title, detail: waiting(cloud.pending_sessions, cloud.pending_usage), isProblem: false };
  }

  function websiteRow(cloud) {
    var site = str(cloud.website_url);
    var shownSite = site ? line(websiteDisplay(site), 300) : COPY.noWebsite;
    return '<div class="item" data-key="website"><div class="an-col"><span>Website</span>' +
      (cloud.website_is_overridden ? A.caption(COPY.websiteOverridden, '', ' data-key="overridden"') : '') + '</div>' +
      '<span class="an-value an-select" data-an-text data-an-clip data-user' + (site ? A.attr('title', line(site, 300)) : '') + '>' +
      esc(shownSite) + '</span></div>';
  }

  function signInRow(cloud) {
    var auth = authOf(cloud);
    var signingIn = auth.kind === 'signing_in';
    var problem = signInProblem(cloud);
    var noSite = !str(cloud.website_url);
    return '<div class="item" data-key="sign-in"><div class="an-col"><span>' + esc(signingIn ? 'Signing in…' : 'Not signed in') + '</span>' +
      A.caption(signInDetail(cloud), '', ' data-key="detail"') +
      (problem ? A.caption(line(problem, TEXT_CHARS), 'an-tone-critical', ' data-key="problem" data-user') : '') + '</div>' +
      '<span class="an-btns an-right">' + (signingIn ? busy('Signing in') + A.button('Cancel', 'an-cloud-cancel', null, { key: 'cloud:cancel' }) : '') +
      A.button('Sign in with Google', 'an-cloud-sign-in', null, { key: 'cloud:sign-in', noenter: true, disabled: noSite || signingIn }) +
      '</span></div>';
  }

  function signedInRows(s, cloud) {
    var auth = authOf(cloud);
    var html = '';
    // Who is signed in, and Sign out… with its confirmation.
    var who = auth.email ? 'Signed in as ' + line(auth.email, 300) : 'Signed in';
    html += '<div class="item" data-key="account"><div class="an-col"><span data-an-text data-an-clip data-user>' + esc(who) + '</span>' +
      A.caption(COPY.fileNote) + '</div><span class="an-btns an-right">' +
      (view.signOutAsk
        ? A.button('Cancel', 'an-cloud-sign-out-cancel', null) + A.button('Sign out', 'an-cloud-sign-out', null, { key: 'cloud:sign-out', cls: 'an-danger', noenter: true })
        : A.button('Sign out…', 'an-cloud-sign-out-ask', null)) + '</span></div>';

    // The two consent switches: exactly what the snapshot says, disabled while their call runs.
    var sync = cloud.sync_enabled === true;
    html += row('sync', 'Sync sessions and usage', A.caption(syncDetail(obj(s.usage).desktop_cache === true)),
      A.toggle(sync, 'Sync sessions and usage', 'an-cloud-sync', null, { key: 'cloud:sync', noenter: true }));

    var summaries = sync && cloud.summaries_enabled === true;
    var available = cloud.summaries_available === true;
    var note = summariesNote(cloud);
    var noteTone = cloud.summaries_enabled && !available ? 'an-tone-warning' : '';
    html += row('summaries', 'Summarise finished sessions with Claude',
      A.caption(COPY.summariesDetail) + (note ? A.caption(note, noteTone, ' data-key="note"') : ''),
      A.toggle(summaries, 'Summarise finished sessions with Claude', 'an-cloud-summaries', null, {
        key: 'cloud:summaries', noenter: true, disabled: !sync || (!available && !summaries), title: !sync || !available ? note || '' : '',
      }));

    var st = status(cloud, C.now());
    html += '<div class="item" data-key="status"><div class="an-col"><span>' + esc(st.title) + '</span>' +
      (st.detail ? A.caption(line(st.detail, TEXT_CHARS), st.isProblem ? 'an-tone-critical' : '', ' data-key="detail" data-user') : '') + '</div>' +
      '<span class="an-btns an-right">' + (cloud.is_syncing ? busy('Syncing') : '') +
      A.button('Sync now', 'an-cloud-sync-now', null, { key: 'cloud:sync-now', disabled: !sync || !!cloud.is_syncing }) + '</span></div>';

    // The dashboard, sharing, and where synced data is removed: every link through cloud_url.
    var noDashboard = !str(cloud.dashboard_url);
    html += '<div class="item" data-key="dashboard"><div class="an-col"><span>Dashboard</span>' + A.caption(COPY.dashboard) +
      (str(cloud.settings_url)
        ? '<div class="an-cap" data-key="data">' + esc(COPY.dataSettingsBefore) +
          A.link(COPY.dataSettingsLink, 'an-cloud-link', 'settings', "Open the website's Settings") + '.</div>'
        : '') +
      '</div><span class="an-btns an-stack-btns">' +
      A.button('Open dashboard', 'an-cloud-link', 'dashboard', { key: 'cloud:link:dashboard', disabled: noDashboard }) +
      A.button('Share accounts…', 'an-cloud-link', 'pools', { key: 'cloud:link:pools', disabled: noDashboard }) + '</span></div>';
    return html;
  }

  function cloudHtml(s) {
    var cloud = obj(s.cloud);
    var signedIn = authOf(cloud).kind === 'signed_in';
    if (!signedIn) view.signOutAsk = false;
    return sec('Cloud') + '<div class="group an-cloud" data-key="group">' + websiteRow(cloud) +
      (signedIn ? signedInRows(s, cloud) : signInRow(cloud)) + A.note('cloud') + '</div>';
  }

  // ---- 6. Sessions and attention (SettingsPaneContent.attentionSection) ------------------------

  /** The shortcut couldn't be registered: the glue's `hotkey_status`, as the snapshot carries it. */
  function hotkeyProblem(s) {
    var a = obj(s.attention);
    if (a.hotkey_ok !== false || shown(s, 'hotKey') === 'off') return null;
    return str(a.hotkey_message) || COPY.hotkeyTaken;
  }

  function attentionHtml(s) {
    var html = sec('Sessions and attention') + '<div class="group an-attention" data-key="group">';
    var policy = shown(s, 'autoOpen');
    var detail = has(COPY.autoOpenDetail, policy) ? COPY.autoOpenDetail[policy] : '';
    html += row('auto-open', 'Open the sessions panel',
      detail ? A.caption(policy === 'never' ? detail : detail + COPY.fullScreen, '', ' data-key="cap"') : '',
      seg(s, 'autoOpen', 'Open the sessions panel', { column: true }));
    html += row('hold-open', 'Keep the notch open while a session needs you', '', seg(s, 'holdOpenWhileNeedsYou', 'Keep the notch open while a session needs you'));
    html += row('ring-badges', 'Counts on the Claude rings', '', settingSwitch(s, 'ringBadges', 'Counts on the Claude rings'));
    html += row('resting-marks', 'Dots on the folded notch', '', settingSwitch(s, 'restingMarks', 'Dots on the folded notch'));
    html += row('tray-badge', 'Needs-you dot on the tray icon', '', settingSwitch(s, 'trayBadge', 'Needs-you dot on the tray icon'));
    html += row('ring-click', 'Clicking a Claude ring', '', seg(s, 'ringClick', 'Clicking a Claude ring'));
    html += row('session-click', 'Clicking a session in the hover card', A.caption(COPY.sessionClick),
      seg(s, 'sessionClick', 'Clicking a session in the hover card'));
    var problem = hotkeyProblem(s);
    html += row('hotkey', 'Panel shortcut', problem ? A.caption(line(problem, TEXT_CHARS), 'an-tone-warning', ' data-key="taken" role="status" data-user') : '',
      seg(s, 'hotKey', 'Panel shortcut'));
    html += row('sound', 'Play a sound', '', settingSwitch(s, 'sound', 'Play a sound'));
    var peek = shown(s, 'peek') === true;
    html += row('peek', 'Open the notch when a session ends', '', settingSwitch(s, 'peek', 'Open the notch when a session ends'));
    html += row('peek-seconds', 'Keep it open for', '', seg(s, 'peekSeconds', 'Keep it open for', { disabled: !peek }), peek ? '' : 'an-dim');
    html += row('type-replies', 'Type replies into the terminal', A.caption(COPY.typeReplies),
      settingSwitch(s, 'typeReplies', 'Type replies into the terminal'));
    html += '<div class="item an-end" data-key="open-panel"><span class="an-btns an-right">' +
      A.button('Open the sessions panel', 'an-open-panel', null, { key: 'open-panel' }) + '</span></div>';
    return html + A.note('attention') + '</div>';
  }

  // ---- 7. Notifications (SettingsPaneContent.notificationsSection; DESIGN-WIN §4.10) -----------

  function notificationsHtml(s) {
    var n = obj(s.notifications);
    var html = sec('Notifications') + '<div class="group an-notifications" data-key="group">';
    html += row('needs-input', 'Banner when a session needs you', '', settingSwitch(s, 'notifyNeedsInput', 'Banner when a session needs you'));
    html += row('ready', 'Banner when a session is done', '', settingSwitch(s, 'notifyReadyForReview', 'Banner when a session is done'));
    var permission = str(n.permission) || 'allowed';
    var text = line(str(n.permission_text) || (permission === 'allowed' ? 'Allowed' : 'Off in Windows Settings'), 300);
    // Windows Settings can turn banners back on, unless there is no installed app to turn on.
    var canOpen = permission.indexOf('disabled') === 0;
    html += '<div class="item" data-key="permission"><span>Windows notifications</span><span class="an-btns an-right">' +
      '<span class="an-value' + (n.permission_warning ? ' an-tone-warning' : '') + '" data-key="text" data-an-text data-an-clip data-user>' + esc(text) + '</span>' +
      (canOpen ? A.button('Open…', 'an-open-notifications', null, { key: 'notifications-open', label: 'Open notification settings' }) : '') +
      '</span></div>';
    return html + A.note('notifications') + '</div>';
  }

  // ---- 8. Advanced (SettingsPaneContent.advancedSection) ----------------------------------------

  function queueLine(adv) {
    var total = count(adv.session_count);
    var review = count(adv.review_count);
    if (!review) return 'Nothing waits for review.';
    return review + ' of ' + plural(total, 'session') + (review === 1 ? ' waits' : ' wait') + ' for review.';
  }

  function advancedHtml(s) {
    var adv = obj(s.advanced);
    var html = sec('Advanced') + '<div class="group an-advanced" data-key="group">';
    html += row('state', 'Session state', A.caption(COPY.sessionState),
      A.button('Copy', 'an-state-copy', null, { key: 'state-copy' }));
    html += row('queue', 'Review queue', A.caption(COPY.reviewQueue) + A.caption(queueLine(adv), 'an-tertiary', ' data-key="counts"'),
      '<span class="an-btns an-right">' + (view.resetAsk
        ? A.button('Cancel', 'an-reset-cancel', null) + A.button('Mark all reviewed', 'an-reset', null, { key: 'reset', cls: 'an-danger', noenter: true })
        : A.button('Reset…', 'an-reset-ask', null)) + '</span>');
    return html + A.note('advanced') + '</div>';
  }

  // ---- actions (each from a click on its own control) -------------------------------------------

  var ACT = S.actions;

  ACT['an-set-switch'] = function (key, el) {
    var s = currentSettings();
    if (!s || !has(SETTINGS, key) || optionsOf(s, key)) return;
    setSetting(key, el.getAttribute('data-an-on') !== '1');
  };
  ACT['an-set-seg'] = function (key, el) {
    var s = currentSettings();
    if (!s || !has(SETTINGS, key)) return;
    var raw = el.getAttribute('data-an-value');
    var options = optionsOf(s, key) || [];
    for (var i = 0; i < options.length; i++) {
      if (String(options[i][0]) === raw) {
        if (options[i][0] !== shown(s, key)) setSetting(key, options[i][0]);
        return;
      }
    }
  };

  ACT['an-refresh'] = function () {
    var s = currentSettings();
    if (!s || obj(s.usage).refreshing) return;
    A.write('usage', 'refresh', 'refresh_usage', { reason: 'manual' });
  };

  function cloudNow() {
    var s = currentSettings();
    return s ? obj(s.cloud) : {};
  }

  function cloudCall(key, args) {
    return A.write('cloud', key, 'cloud', args);
  }

  ACT['an-cloud-sign-in'] = function () {
    var cloud = cloudNow();
    var kind = authOf(cloud).kind;
    if (!str(cloud.website_url) || (kind !== 'signed_out' && kind !== 'error')) return;
    cloudCall('cloud:sign-in', { action: 'sign_in' });
  };
  ACT['an-cloud-cancel'] = function () {
    if (authOf(cloudNow()).kind !== 'signing_in') return;
    cloudCall('cloud:cancel', { action: 'cancel_sign_in' });
  };
  ACT['an-cloud-sign-out-ask'] = function () {
    if (authOf(cloudNow()).kind !== 'signed_in') return;
    view.signOutAsk = true;
    A.render();
  };
  ACT['an-cloud-sign-out-cancel'] = function () {
    view.signOutAsk = false;
    A.render();
  };
  ACT['an-cloud-sign-out'] = function () {
    if (!view.signOutAsk || authOf(cloudNow()).kind !== 'signed_in') return;
    view.signOutAsk = false;
    cloudCall('cloud:sign-out', { action: 'sign_out' });
  };
  ACT['an-cloud-sync'] = function () {
    var cloud = cloudNow();
    if (authOf(cloud).kind !== 'signed_in') return;
    // The opposite of what the snapshot says, never of what a click last asked for.
    cloudCall('cloud:sync', { action: 'set_sync', on: cloud.sync_enabled !== true });
  };
  ACT['an-cloud-summaries'] = function () {
    var cloud = cloudNow();
    if (authOf(cloud).kind !== 'signed_in' || cloud.sync_enabled !== true) return;
    var on = cloud.summaries_enabled === true;
    if (!on && cloud.summaries_available !== true) return;
    cloudCall('cloud:summaries', { action: 'set_summaries', on: !on });
  };
  ACT['an-cloud-sync-now'] = function () {
    var cloud = cloudNow();
    if (authOf(cloud).kind !== 'signed_in' || cloud.sync_enabled !== true || cloud.is_syncing) return;
    cloudCall('cloud:sync-now', { action: 'sync_now' });
  };
  ACT['an-cloud-link'] = function (target) {
    var cloud = cloudNow();
    if (authOf(cloud).kind !== 'signed_in') return;
    var known = target === 'settings' ? str(cloud.settings_url) : (target === 'dashboard' || target === 'pools') ? str(cloud.dashboard_url) : null;
    if (!known) return;
    // The glue says where the page is; the glue opens it. The page never opens a URL itself.
    A.write('cloud', 'cloud:link:' + target, 'cloud_url', { target: target }).then(function (reply) {
      var url = reply && str(reply.url);
      if (!url || !/^https?:\/\//i.test(url)) return;
      A.write('cloud', 'cloud:open', 'open_url', { url: url });
    });
  };

  ACT['an-open-panel'] = function () {
    A.write('attention', 'open-panel', 'panel_open', { route: 'sessions', reason: 'settings' });
  };
  ACT['an-open-notifications'] = function () {
    A.write('notifications', 'notifications-open', 'open_notification_settings', null);
  };

  ACT['an-state-copy'] = function () {
    A.write('advanced', 'state-copy', 'session_state_text', null).then(function (reply) {
      var text = reply && typeof reply.text === 'string' ? reply.text : null;
      if (text === null) return;
      A.write('advanced', 'copy', 'copy_text', { text: text }).then(function (copied) {
        if (copied) A.toast('Copied');
      });
    });
  };
  ACT['an-reset-ask'] = function () {
    view.resetAsk = true;
    A.render();
  };
  ACT['an-reset-cancel'] = function () {
    view.resetAsk = false;
    A.render();
  };
  ACT['an-reset'] = function () {
    if (!view.resetAsk) return;
    view.resetAsk = false;
    A.write('advanced', 'reset', 'reset_review_queue', null);
  };

  // ---- slots and scenes ------------------------------------------------------------------------

  function slot(build) {
    return function (s, api) {
      A = api;
      return build(s);
    };
  }

  S.slots.usage = slot(usageHtml);
  S.slots.cloud = slot(cloudHtml);
  S.slots.attention = slot(attentionHtml);
  S.slots.notifications = slot(notificationsHtml);
  S.slots.advanced = slot(advancedHtml);

  var WEBSITE = 'https://agentnotch.rivant.in';

  function signedOut(s, website, overridden) {
    view.signOutAsk = false;
    view.resetAsk = false;
    var c = obj(s.cloud);
    s.cloud = {
      website_url: website, website_is_overridden: !!overridden, auth: 'signed_out',
      sync_enabled: false, summaries_enabled: false, summaries_available: !!c.summaries_available,
      is_syncing: false, last_sync_at_ms: null, last_error: null, pending_sessions: 0, pending_usage: 0,
      summarized_sessions: 0, dashboard_url: null, pools_url: null, settings_url: null,
    };
    return { scroll: 'cloud' };
  }

  /** The Mac's settings-cloud-* sheets: the Cloud section in each state, scrolled to the top. */
  S.scenes['settings-cloud-signed-out'] = function (s) { return signedOut(s, WEBSITE, false); };
  S.scenes['settings-cloud-overridden'] = function (s) { return signedOut(s, 'http://localhost:3000', true); };
  S.scenes['settings-cloud-no-website'] = function (s) { return signedOut(s, null, false); };
  S.scenes['settings-cloud-signed-in'] = function () {
    view.signOutAsk = false;
    view.resetAsk = false;
    return { scroll: 'cloud' };
  };

  window.agentnotchSettingsSections = {
    COPY: COPY,
    websiteDisplay: websiteDisplay,
    signInDetail: signInDetail,
    signInProblem: signInProblem,
    syncDetail: syncDetail,
    summariesNote: summariesNote,
    status: status,
    waiting: waiting,
    authOf: authOf,
    _: { holds: holds, view: view, SETTINGS: SETTINGS },
  };

  S.render();
})();
