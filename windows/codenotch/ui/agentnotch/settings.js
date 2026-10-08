// Agent Notch's Claude Code pane in upstream's Settings page (DESIGN-WIN §5.4, §5.5; the Mac's
// SettingsPaneContent.swift, AccountsSection.swift).
//
// Loaded by seam WSS5 AFTER the page's own script, into the `#pane-claude` section that seams
// WSS1-WSS4 add, so upstream's globals (`ui`, `toast`, `showTab`) already exist. One strict-mode
// IIFE, one global (`window.agentnotchSettings`), no top-level let/const. It loads the shared
// library (`common.js`) and the rebrand (`rebrand.js`) itself: the page keeps one seam line.
//
// It does two jobs:
//   * the rebrand of upstream's copy: `ui()` wrapped, `document.title`, the static DOM under
//     `#app` once, and later changes through a MutationObserver. Fork nodes that show someone's
//     data (names, emails, folders, the website) carry `data-user` and are never rewritten.
//   * the pane itself, drawn from the last whole `settings` snapshot (`an_call('settings')`,
//     then `an:settings` / `an:cloud`), with upstream's blocks (.sec .group .item .item.cap
//     .switch .btn .link #toast), in the Mac's order:
//       region "status"         the sealed line, or why the hub does not answer
//       region "consent"        the consent card (every settings.json it writes) or the scope notice
//       region "accounts"       one row per account, suggestions, unsigned folders, adding accounts
//       region "hooks"          hooks and status line, the pipe, Claude Code, the official app's hooks
//       regions "usage" "cloud" "attention" "notifications" "advanced"   drawn by `slots.<name>`,
//                               which settings-sections.js fills
//   Each region is patched with `agentnotchCommon.morph`, so a focused field, its caret and a
//   half-typed name survive every snapshot.
//
// Writes. The pane turns hook installation on and edits other apps' settings.json, so every
// call that writes goes out from a click on its own control and from nothing else: never on
// load, never on Enter in a field, never from a keyboard default. The sensitive buttons carry
// `data-an-noenter` (Enter on them does nothing; Space still presses them). A write answered
// `sealed` shows the hub's message and is not retried.
(function () {
  'use strict';

  var pane = document.getElementById('pane-claude');
  if (!pane) return;

  var me = document.currentScript;
  var base = (function (src) {
    var s = String(src || '');
    var cut = s.lastIndexOf('/');
    return cut >= 0 ? s.slice(0, cut + 1) : '';
  })(me && me.src);

  var C = null;
  var R = null;

  /** The official app, named where its own hooks are meant (never rebranded: this pane is ours). */
  var OFFICIAL_APP = 'Codenotch';

  /** Word for word the Mac's copy (ConsentCopy, AccountsSection, SettingsPaneContent), Windows words. */
  var COPY = {
    sealed: 'Sealed: this window shows sample data.',
    notRunning: "Claude Code control isn't running.",
    consentTitle: 'Turn on Claude Code control',
    consentText: 'To show every session live and let you answer prompts from here, this app adds its hooks and a status-line wrapper to the settings.json of each folder Claude Code runs in. Nothing is written until you turn it on, and turning it off puts your status line back exactly.',
    codenotchStays: OFFICIAL_APP + "'s hooks stay; remove them per folder under Hooks and status line.",
    installOff: 'Installing is off for this run (--no-install).',
    scopeTitle: 'Claude Code control now covers your VS Code workspaces',
    noAccounts: 'No Claude Code accounts yet. Add the folder Claude Code uses, or create a new account.',
    ringNeedsTracking: 'Needs Track sessions and hooks',
    ringNeedsTrackingCaption: 'Ring in notch needs Track sessions and hooks',
    defaultHolderHelp: "Terminals outside VS Code run as this account now: Claude Parallel Profiles copies the focused VS Code window's account into ~\\.claude.",
    footer: "Ring in notch draws the account's usage ring. Track sessions and hooks lists its sessions here; switching it off also removes this app's hooks from that account, and its ring and usage checks with them.",
    footerProfiles: "Ring in notch draws the account's usage ring. Track sessions and hooks lists its sessions here; switching it off also removes this app's hooks from that account's VS Code workspaces and folders, and its ring and usage checks with them. ~\\.claude keeps them while another account is tracked, since Claude Parallel Profiles copies whichever account you last used into it; that account's sessions there are then hidden and its prompts stay in the terminal.",
    profilesTitle: 'Add an account in VS Code',
    profiles: "Claude Parallel Profiles adds accounts: open a VS Code window, sign in with Claude Code's account menu or /login, and the extension saves the account. It appears here by itself, with its usage and sessions, and new windows get this app's hooks automatically. Signing in switches that window to the new account and reloads it (sessions running in it stop). The previous account stays saved, and you can switch any window back from the status bar.",
    terminalAlternative: 'To use another account from a terminal without VS Code, you can instead create a separate config folder (~\\.claude-<name>) and sign in to it with CLAUDE_CONFIG_DIR.',
    createdHint: "Run this in a terminal, then /login. It's on your clipboard.",
    controlOffTitle: 'Claude Code control is off',
    controlOff: "Nothing is written to any settings.json. Sessions still show from Claude Code's session files, without approvals or \u201Cdone\u201D.",
    hooksSwitch: 'Hooks in tracked accounts',
    statusLine: 'Live status line data',
    statusLineCaption: "Wraps each account's status line to read limits and context live. Turning it off restores your status line exactly.",
    turnOnFirst: 'Turn on Claude Code control first.',
    busy: 'Changing settings.json files\u2026',
    noVersion: 'No Claude Code answered at that path.',
    notCreated: "The account wasn't created.",
  };

  var NAME_CHARS = 200;
  var PATH_CHARS = 400;
  var TEXT_CHARS = 2000;
  var COPIED_MS = 1500;

  function closedAdd() {
    return { step: 'closed', name: '', error: '', created: '', command: '' };
  }

  var state = {
    settings: null,
    /** An `an:cloud` that arrived before the first settings. */
    cloud: null,
    /** Why the hub did not answer (`{code, message}`), until a settings snapshot arrives. */
    failure: null,
    /** Consent answered (or the scope notice) from this page, until the snapshot says so. */
    consentAnswered: false,
    scopeAnswered: false,
    /** "Not now" was answered earlier and "Turn on…" asked for the card again. */
    reconsider: false,
    /** identity_id -> the draft name while renaming. */
    renaming: Object.create(null),
    /** identity_id -> true while "Forget…" asks. */
    forgetting: Object.create(null),
    /** identity_id -> folders shown. */
    expanded: Object.create(null),
    /** identity_id -> "Copied" shown until (ms). */
    copied: Object.create(null),
    /** New account: {step: closed|guidance|naming|created, name, error, created, command}. */
    add: closedAdd(),
    /** The Claude Code path field: null when closed, else the draft. */
    binary: null,
    /** Calls in flight, by key: their controls are disabled so one click sends one call. */
    pending: Object.create(null),
    /** A line under a region after a call: {text, tone}. */
    notes: Object.create(null),
    /** A snapshot scene is shown (`showScene`): writes are not sent. */
    scene: null,
    sceneSettings: null,
    /** The first snapshot asked for consent and the tab was opened on this pane once. */
    tabOpened: false,
  };

  var REGIONS = ['status', 'consent', 'accounts', 'hooks', 'usage', 'cloud', 'attention', 'notifications', 'advanced'];
  var regions = Object.create(null);
  /** settings-sections.js's sections: `slots.<region> = function (settings, api) { return html; }`. */
  var SLOTS = Object.create(null);
  /** settings-sections.js's scenes: `scenes[name] = function (settingsCopy) { return {tab, scroll, …} }`. */
  var SLOT_SCENES = Object.create(null);
  var ACTIONS = Object.create(null);

  // ---- small helpers ---------------------------------------------------------------------------

  function esc(value) {
    return C.esc(value);
  }

  function cut(text, max) {
    var s = String(text == null ? '' : text);
    return s.length > max ? s.slice(0, max - 1) + '\u2026' : s;
  }

  /** User text on one line, capped. */
  function line(value, max) {
    return cut(C.oneLine(value == null ? '' : value), max || NAME_CHARS);
  }

  /** User text that may hold line breaks (a folder summary), capped. */
  function block(value, max) {
    return cut(String(value == null ? '' : value).replace(/\r\n?/g, '\n'), max || TEXT_CHARS);
  }

  function has(o, key) {
    return !!o && Object.prototype.hasOwnProperty.call(o, key);
  }

  function list(value) {
    return Array.isArray(value) ? value : [];
  }

  function strings(value) {
    return list(value).filter(function (x) { return typeof x === 'string' && x; });
  }

  function obj(value) {
    return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  }

  function view() {
    return state.scene ? state.sceneSettings : state.settings;
  }

  function plural(n, word, many) {
    return n + ' ' + (n === 1 ? word : (many || word + 's'));
  }

  function attr(name, value) {
    return ' ' + name + '="' + esc(value) + '"';
  }

  function button(label, action, arg, opts) {
    var o = opts || {};
    var key = o.key || (action + ':' + (arg == null ? '' : arg));
    var disabled = !!o.disabled || !!state.pending[key];
    return '<button type="button" class="btn' + (o.cls ? ' ' + o.cls : '') + '"' +
      attr('data-key', 'b:' + key) + attr('data-an-action', action) +
      (arg == null ? '' : attr('data-an-arg', arg)) +
      (o.noenter ? ' data-an-noenter' : '') +
      (o.title ? attr('title', o.title) : '') +
      (o.label ? attr('aria-label', o.label) : '') +
      (disabled ? ' disabled' : '') + '>' + esc(label) + '</button>';
  }

  function link(label, action, arg, ariaLabel) {
    return '<button type="button" class="link"' + attr('data-key', 'l:' + action + ':' + arg) +
      attr('data-an-action', action) + attr('data-an-arg', arg) +
      (ariaLabel ? attr('aria-label', ariaLabel) : '') + '>' + esc(label) + '</button>';
  }

  function toggle(on, label, action, arg, opts) {
    var o = opts || {};
    var key = o.key || (action + ':' + (arg == null ? '' : arg));
    var disabled = !!o.disabled || !!state.pending[key];
    return '<button type="button" class="switch' + (on ? ' on' : '') + (disabled ? ' locked' : '') + '" role="switch"' +
      attr('aria-checked', on ? 'true' : 'false') + attr('aria-label', label) +
      attr('data-key', 's:' + key) + attr('data-an-action', action) +
      (arg == null ? '' : attr('data-an-arg', arg)) + attr('data-an-on', on ? '1' : '0') +
      (o.noenter ? ' data-an-noenter' : '') +
      (o.title ? attr('title', o.title) : '') +
      (disabled ? ' disabled' : '') + '></button>';
  }

  function caption(text, cls, extra) {
    return '<div class="an-cap' + (cls ? ' ' + cls : '') + '"' + (extra || '') + '>' + esc(text) + '</div>';
  }

  /** The line a call left under its region (a refusal, a sealed answer). */
  function note(region) {
    var n = state.notes[region];
    if (!n) return '';
    return '<div class="item cap an-note an-tone-' + esc(n.tone) + '" data-key="note" role="status" data-user>' + esc(n.text) + '</div>';
  }

  /** Account hue as a dot (the colour comes from our own table, never from data). */
  function dot(colorIndex, faded) {
    return '<span class="an-sdot' + (faded ? ' an-faded' : '') + '" aria-hidden="true" style="background:' +
      C.accountHue(colorIndex) + '"></span>';
  }

  // ---- the frame: status line ------------------------------------------------------------------

  function statusHtml(s) {
    if (!s) {
      if (!state.failure) return '';
      var why = state.failure.message ? line(state.failure.message, 300) : '';
      return '<div class="group an-status" data-key="failure" role="status">' +
        '<div class="item cap an-tone-critical" data-key="t" data-an-text>' + esc(COPY.notRunning) + '</div>' +
        (why && why !== COPY.notRunning ? '<div class="item cap" data-key="why" data-user>' + esc(why) + '</div>' : '') +
        '</div>';
    }
    if (!s.sealed) return '';
    return '<div class="group an-status" data-key="sealed" role="status"><div class="item cap" data-an-text>' +
      esc(COPY.sealed) + '</div></div>';
  }

  // ---- 1. consent card / scope notice (SettingsConsentCard, scopeNoticeSection) --------------

  function consentFiles(setup) {
    return list(setup.consent_files).filter(function (f) {
      return f && typeof f === 'object' && typeof f.path === 'string' && f.path;
    });
  }

  function installOff(s) {
    return !!obj(s.setup).install_disabled || obj(s.hooks).install_allowed === false;
  }

  /**
   * "Turn on Claude Code control": every settings.json it would edit (the whole list), and Turn
   * on as the emphasised button that is never a default (`data-an-noenter`).
   */
  function consentCard(s, where) {
    var setup = obj(s.setup);
    var files = consentFiles(setup);
    var off = installOff(s);
    var html = '<div class="group an-consent" data-key="consent-' + where + '" role="group"' + attr('aria-label', COPY.consentTitle) + '>' +
      '<div class="item an-stack" data-key="body">' +
      '<div class="an-title" role="heading" aria-level="2">' + esc(COPY.consentTitle) + '</div>' +
      caption(COPY.consentText, 'an-callout');
    if (files.length) {
      html += '<ul class="an-files" data-key="files"' + attr('aria-label', 'Files it edits') + ' data-user>';
      files.forEach(function (f, i) {
        var account = typeof f.account === 'string' && f.account ? line(f.account, NAME_CHARS) : '';
        html += '<li class="an-file" data-key="f' + i + '">' + esc(line(f.path, PATH_CHARS)) +
          (account ? ' <span class="an-file-acct">(' + esc(account) + ')</span>' : '') + '</li>';
      });
      html += '</ul>';
    }
    if (strings(setup.codenotch_hooks_folders).length) html += caption(COPY.codenotchStays, '', ' data-key="codenotch"');
    if (off) html += caption(COPY.installOff, 'an-tone-warning', ' data-key="install-off"');
    html += '<div class="an-btns an-right" data-key="btns">' +
      button('Not now', 'consent-later', where, { key: 'consent-later' }) +
      button('Turn on', 'consent-on', where, { key: 'consent-on', cls: 'an-primary', noenter: true, disabled: off }) +
      '</div></div>' + note('consent') + '</div>';
    return html;
  }

  function scopeMessage(count) {
    var folders = count === 1 ? "1 VS Code workspace's folder" : count + " VS Code workspaces' folders";
    return 'This version puts its hooks and status line in ' + folders + ' too, and in new ones as they appear: Claude Parallel Profiles runs Claude Code there. Each settings.json has a backup beside it. Account stores never get hooks.';
  }

  function consentShown(s) {
    return !!s && !!obj(s.setup).needs_hook_consent && !state.consentAnswered;
  }

  function scopeShown(s) {
    if (!s) return false;
    var setup = obj(s.setup);
    return !setup.needs_hook_consent && obj(s.hooks).consent === true &&
      strings(setup.new_install_folders).length > 0 && !state.scopeAnswered;
  }

  function consentHtml(s) {
    if (consentShown(s)) return consentCard(s, 'top');
    if (!scopeShown(s)) return state.notes.consent ? '<div class="group" data-key="consent-note">' + note('consent') + '</div>' : '';
    var folders = strings(obj(s.setup).new_install_folders).map(function (f) { return line(f, PATH_CHARS); });
    return '<div class="group an-scope" data-key="scope" role="group"' + attr('aria-label', COPY.scopeTitle) + '>' +
      '<div class="item an-stack" data-key="body">' +
      '<div class="an-title" role="heading" aria-level="2">' + esc(COPY.scopeTitle) + '</div>' +
      caption(scopeMessage(folders.length), 'an-callout') +
      '<ul class="an-files an-files-small" data-key="folders"' + attr('aria-label', 'Folders: ' + folders.join(', ')) + ' data-user>' +
      folders.map(function (f, i) { return '<li class="an-file" data-key="f' + i + '">' + esc(f) + '</li>'; }).join('') +
      '</ul><div class="an-btns an-right" data-key="btns">' +
      button('Turn off', 'scope-off', null, { key: 'scope-off' }) +
      button('OK', 'scope-ok', null, { key: 'scope-ok' }) +
      '</div></div>' + note('consent') + '</div>';
  }

  // ---- 2. accounts (AccountsSection.swift) --------------------------------------------------

  function accountsOf(s) {
    return list(s && s.accounts).filter(function (a) {
      return a && typeof a === 'object' && typeof a.identity_id === 'string' && a.identity_id;
    });
  }

  function accountById(id) {
    var found = null;
    accountsOf(view()).forEach(function (a) { if (a.identity_id === id) found = a; });
    return found;
  }

  /**
   * Claude Parallel Profiles manages accounts here: a workspace's folder or a store is listed.
   * (The settings snapshot has no flag of its own for it; the folders' roles say it.)
   */
  function parallelProfiles(s) {
    return accountsOf(s).some(function (a) {
      return list(a.folders).some(function (f) {
        var role = f && typeof f.role === 'string' ? f.role : '';
        var title = f && typeof f.title === 'string' ? f.title : '';
        return role === 'Account store' || role === 'VS Code workspace' || title.indexOf('VS Code \u00B7 ') === 0;
      });
    });
  }

  var TONES = { ok: 'ok', warning: 'warning', critical: 'critical', neutral: 'neutral' };

  function chip(text, tone, key) {
    return '<span class="an-chip an-tone-' + (has(TONES, tone) ? TONES[tone] : 'neutral') + '"' + attr('data-key', key) + '>' +
      esc(line(text, 120)) + '</span>';
  }

  /** Can this pane's writes go out now (consent given, installing allowed, nothing running)? */
  function installState(s) {
    var hooks = obj(s && s.hooks);
    return {
      allowed: !installOff(s || {}),
      busy: !!hooks.busy,
      consent: hooks.consent === true,
      enabled: !!hooks.enabled,
    };
  }

  function folderRows(a) {
    return '<div class="an-folders" data-key="folders" data-user>' + list(a.folders).map(function (f, i) {
      if (!f || typeof f !== 'object') return '';
      var title = line(f.title, PATH_CHARS);
      var project = title.indexOf('VS Code \u00B7 ') === 0;
      // A project name is short and leads; a path keeps its end (the hash or the folder name).
      var html = '<div class="an-folder" data-key="f' + i + '">' +
        '<span class="an-folder-title' + (project ? '' : ' an-mono an-head') + '" data-an-text data-an-clip' +
        attr('title', line(f.id || f.title, PATH_CHARS)) + '><span class="an-bidi">' + esc(title) + '</span></span>' +
        '<span class="an-folder-role' + (project ? ' an-mono an-head' : '') + '" data-an-text data-an-clip><span class="an-bidi">' +
        esc(line(f.role, PATH_CHARS)) + '</span></span>' +
        '<span class="an-folder-state' + (f.role === 'Account store' ? ' an-tertiary' : '') + '">' + esc(line(f.state, 120)) + '</span></div>';
      if (typeof f.status_line_note === 'string' && f.status_line_note) html += caption(line(f.status_line_note, TEXT_CHARS), 'an-sub', ' data-key="sl' + i + '"');
      if (typeof f.not_hookable === 'string' && f.not_hookable) html += caption(line(f.not_hookable, TEXT_CHARS), 'an-sub an-tone-warning', ' data-key="nh' + i + '"');
      return html;
    }).join('') + '</div>';
  }

  function usageLine(a) {
    var text = line(a.usage_line, 300);
    if (a.usage_stale && !/stale$/.test(text)) text += ', stale';
    return caption(text, a.usage_stale ? 'an-stale' : '', ' data-key="usage"');
  }

  function accountRow(s, a, profiles) {
    var id = a.identity_id;
    var ins = installState(s);
    var tracked = !!a.is_tracked;
    var name = line(a.label || a.default_label || 'Claude', NAME_CHARS);
    var renaming = has(state.renaming, id);
    var folders = list(a.folders);
    var html = '<div class="an-acct' + (tracked ? '' : ' off') + '"' + attr('data-key', 'acct:' + id) + ' role="group"' +
      attr('aria-label', name + ', ' + line(a.identity_line, 300) + ', ' + line(a.hook_state, 120)) + '>';

    // Line 1: dot, name (or the rename field), the default caption; Ring in notch on the right.
    html += '<div class="an-acct-line" data-key="line"><span class="an-acct-name" data-key="name">' + dot(a.color_index, !tracked);
    if (renaming) {
      html += '<input type="text" class="an-input" data-an-keep data-an-field="rename"' + attr('data-an-arg', id) +
        attr('data-key', 'rename') + attr('value', state.renaming[id]) + attr('placeholder', line(a.default_label, NAME_CHARS)) +
        attr('aria-label', 'Name for ' + name) + ' maxlength="200" spellcheck="false" autocomplete="off">' +
        button('Save', 'rename-save', id) + button('Cancel', 'rename-cancel', id);
    } else {
      html += '<span class="an-name" data-key="label" data-an-text data-an-clip data-user>' + esc(name) + '</span>';
      if (a.is_default) {
        html += profiles
          ? '<span class="an-cap an-inline" data-key="default"' + attr('title', COPY.defaultHolderHelp) + '>In ~\\.claude now</span>'
          : '<span class="an-cap an-inline" data-key="default">Default</span>';
      }
    }
    // An untracked account has no ring and no usage checks: the switch would do nothing.
    html += '</span><span class="an-ring" data-key="ring"><span class="an-cap an-inline' + (tracked ? '' : ' an-tertiary') + '" aria-hidden="true">Ring in notch</span>' +
      toggle(tracked && !!a.ring_shown, 'Ring in notch for ' + name, 'ring-shown', id, {
        disabled: !tracked, title: tracked ? 'Ring in notch' : COPY.ringNeedsTracking,
      }) + '</span></div>';

    // Under it, indented: who, where, its folders, its usage, its hooks.
    html += '<div class="an-acct-body" data-key="body">';
    html += caption(line(a.identity_line, 300), '', ' data-key="identity" data-user');
    if (a.folder_summary) html += caption(block(a.folder_summary), 'an-pre', ' data-key="summary" data-user');
    if (folders.length) {
      var open = !!state.expanded[id];
      html += '<div data-key="toggle-folders">' + link(open ? 'Hide folders' : 'Show folders', 'folders', id,
        (open ? 'Hide ' : 'Show ') + name + "'s folders") + '</div>';
      if (open) html += folderRows(a);
    }
    html += usageLine(a);

    html += '<div class="an-chips" data-key="chips">' + chip(a.hook_state, a.hook_state_tone, 'hook');
    if (a.live_status_line) html += chip('Live status line', 'neutral', 'sl');
    if (folders.some(function (f) { return f && f.codenotch_hooks; })) html += chip(OFFICIAL_APP + ' hooks', 'warning', 'cn');
    html += '</div>';

    if (typeof a.hook_problem === 'string' && a.hook_problem) {
      html += caption(line(a.hook_problem, TEXT_CHARS), a.hook_problem_critical ? 'an-tone-critical' : 'an-tone-warning', ' data-key="problem" data-user');
    }

    html += '<div class="an-track" data-key="track"><span>Track sessions and hooks</span>' +
      toggle(tracked, 'Track sessions and hooks for ' + name, 'track', id, { noenter: true }) + '</div>';
    if (!tracked) html += caption(COPY.ringNeedsTrackingCaption, '', ' data-key="needs-track"');

    html += '<div class="an-btns" data-key="btns">' + button('Rename\u2026', 'rename', id, { label: 'Rename ' + name });
    if (a.can_install) {
      html += button(line(a.install_label || 'Install hooks', 60), 'install', id, {
        noenter: true, disabled: !ins.allowed || ins.busy || !ins.consent,
      });
    }
    if (typeof a.launch_command === 'string' && a.launch_command) {
      var copied = (state.copied[id] || 0) > C.now();
      html += button(copied ? 'Copied' : 'Copy launch command', 'copy-launch', id, { title: line(a.launch_command, TEXT_CHARS) });
    }
    if (folders.length && folders[0] && typeof folders[0].id === 'string') html += button('Show in Explorer', 'reveal', id);
    if (a.can_forget) {
      if (state.forgetting[id]) {
        html += button('Forget and remove hooks', 'forget', id, { cls: 'an-danger', noenter: true }) + button('Cancel', 'forget-cancel', id);
      } else {
        html += button('Forget\u2026', 'forget-ask', id);
      }
    }
    html += '</div>';
    if (state.forgetting[id] && a.can_forget) html += caption(line(a.forget_caption, TEXT_CHARS), '', ' data-key="forget-cap"');
    html += '</div></div>';
    return html;
  }

  /** UnsignedFoldersRow.caption, with Windows' folder spelling. */
  function unsignedCaption(folders) {
    var listText = folders.join(', ');
    var workspace = folders.some(function (f) {
      return /[\\/]\.claude-windows[\\/]/.test(f) || f.indexOf('VS Code \u00B7 ') === 0;
    });
    return workspace
      ? listText + ': Claude Code runs here, but nobody is signed in yet. Pick an account for that window from the Claude Parallel Profiles status bar item, or /login there; it gets a ring then.'
      : listText + ': Claude Code runs here, but nobody is signed in yet. It gets a ring once someone signs in with /login.';
  }

  /** The `<slug>` of "~\.claude-<slug>" (AccountRegistry.sanitizedAccountName), for the caption only. */
  function slug(name) {
    var out = '';
    var trimmed = String(name || '').trim();
    for (var i = 0; i < trimmed.length; i++) {
      var ch = trimmed[i];
      if (/[A-Za-z0-9_.\-]/.test(ch)) out += ch;
      else if (/\s/.test(ch)) out += '-';
    }
    out = out.replace(/^[.\-]+/, '');
    return out ? out.toLowerCase() : null;
  }

  function addAccountHtml(s) {
    var add = state.add;
    var html = '<div class="an-add" data-key="add">';
    if (add.step === 'guidance') {
      html += '<div class="an-title an-title-small" role="heading" aria-level="3">' + esc(COPY.profilesTitle) + '</div>' +
        caption(COPY.profiles) + caption(COPY.terminalAlternative) +
        '<div class="an-btns an-right" data-key="btns">' +
        button('Create a folder for the terminal\u2026', 'add-naming', null) + button('Done', 'add-close', null) + '</div>';
    } else if (add.step === 'naming') {
      var named = add.name.trim() !== '';
      html += '<div class="an-row" data-key="row"><input type="text" class="an-input an-grow" data-an-keep data-an-field="new-account" data-key="name"' +
        attr('value', add.name) + ' placeholder="Name, for example work" aria-label="Account name" maxlength="80" spellcheck="false" autocomplete="off">' +
        button('Cancel', 'add-close', null) + button('Create', 'add-create', null, { key: 'add-create', disabled: !named }) + '</div>';
      html += add.error
        ? caption(line(add.error, TEXT_CHARS), 'an-tone-critical', ' data-key="err" data-user')
        : caption('Creates ~\\.claude-' + (slug(add.name) || 'name') + ', a separate Claude Code config folder you sign in to once.', '', ' data-key="hint" data-user');
    } else if (add.step === 'created') {
      html += '<div class="an-title an-title-small" role="heading" aria-level="3" data-user>' + esc('Created ' + line(add.created, PATH_CHARS)) + '</div>' +
        caption(COPY.createdHint) +
        '<div class="an-command an-mono an-select" data-key="command" data-user>' + esc(line(add.command, TEXT_CHARS)) + '</div>' +
        '<div class="an-btns an-right" data-key="btns">' + button('Copy again', 'add-copy', null) + button('Done', 'add-close', null) + '</div>';
    } else {
      html += '<div class="an-btns an-right" data-key="btns">' +
        button('Add existing folder\u2026', 'add-folder', null, { key: 'add-folder', disabled: !installState(s).allowed }) +
        button('New account\u2026', 'add-open', null) + '</div>';
    }
    return html + '</div>';
  }

  function accountsHtml(s) {
    var profiles = parallelProfiles(s);
    var ins = installState(s);
    var accounts = accountsOf(s);
    var html = '<div class="sec" data-key="title">Accounts</div><div class="group an-accounts" data-key="group">';
    if (!accounts.length) html += '<div class="item cap" data-key="empty">' + esc(COPY.noAccounts) + '</div>';
    accounts.forEach(function (a) { html += accountRow(s, a, profiles); });
    list(s.suggestions).forEach(function (sg) {
      if (!sg || typeof sg.path !== 'string' || !sg.path) return;
      var path = line(sg.path, PATH_CHARS);
      html += '<div class="item an-suggestion"' + attr('data-key', 'sug:' + sg.path) + ' role="group"' +
        attr('aria-label', 'Suggested account folder ' + path + ': ' + line(sg.reason, 300)) + '>' +
        '<div class="an-col"><div class="an-secondary" data-an-text data-an-clip data-user>' + esc('Found ' + path) + '</div>' +
        caption(line(sg.reason, 300)) + '</div><div class="an-btns">' +
        button('Dismiss', 'suggestion-dismiss', sg.path) +
        button('Add', 'suggestion-add', sg.path, { disabled: !ins.allowed }) + '</div></div>';
    });
    var unsigned = strings(s.unsigned_folders).map(function (f) { return line(f, PATH_CHARS); });
    if (unsigned.length) {
      html += '<div class="item an-unsigned" data-key="unsigned"><div class="an-col"><div class="an-secondary">Not signed in</div>' +
        caption(unsignedCaption(unsigned), '', ' data-user') + '</div></div>';
    }
    html += addAccountHtml(s) + note('accounts') + '</div>';
    html += '<div class="an-footer" data-key="footer">' + esc(profiles ? COPY.footerProfiles : COPY.footer) + '</div>';
    return html;
  }

  // ---- 3. hooks and status line (SettingsPaneContent.hooksSection) ----------------------------

  function hooksHtml(s) {
    var hooks = obj(s.hooks);
    var setup = obj(s.setup);
    var ins = installState(s);
    var html = '<div class="sec" data-key="title">Hooks and status line</div><div class="group an-hooks" data-key="group">';
    if (hooks.consent === false) {
      if (state.reconsider) {
        html += '<div class="item an-inline-card" data-key="reconsider">' + consentCard(s, 'hooks') + '</div>';
      } else {
        html += '<div class="item" data-key="off"><div class="an-col"><span>' + esc(COPY.controlOffTitle) + '</span>' +
          caption(COPY.controlOff) + '</div>' + button('Turn on\u2026', 'reconsider', null) + '</div>';
      }
    }
    var lockReason = !ins.consent ? COPY.turnOnFirst : !ins.allowed ? COPY.installOff : ins.busy ? COPY.busy : '';
    var locked = !!hooks.enabled_locked || !ins.consent || !ins.allowed || ins.busy;
    var summary = line(hooks.summary, 300);
    html += '<div class="item" data-key="hooks"><div class="an-col"><span>' + esc(COPY.hooksSwitch) + '</span>' +
      caption(summary, hooks.summary_warning ? 'an-tone-warning' : '', ' data-key="summary"') +
      (locked && lockReason && lockReason !== summary ? caption(lockReason, 'an-tertiary', ' data-key="lock"') : '') +
      '</div>' + toggle(!!hooks.enabled, COPY.hooksSwitch, 'hooks-enabled', null, {
      disabled: locked, noenter: true, title: locked ? lockReason : '',
    }) + '</div>';
    html += '<div class="item" data-key="status-line"><div class="an-col"><span>' + esc(COPY.statusLine) + '</span>' +
      caption(COPY.statusLineCaption) + '</div>' + toggle(!!hooks.status_line, COPY.statusLine, 'status-line', null, {
      disabled: !ins.consent || !ins.allowed, noenter: true,
    }) + '</div>';
    html += '<div class="item" data-key="pipe"><span>Pipe</span><span class="an-mono an-value an-select" data-an-text data-an-clip data-user' +
      attr('title', line(hooks.pipe_name, PATH_CHARS)) + '>' + esc(line(hooks.pipe_name, PATH_CHARS)) + '</span></div>';

    var version = typeof hooks.claude_version === 'string' && hooks.claude_version ? 'Version ' + line(hooks.claude_version, 60) : 'Not found yet';
    html += '<div class="item an-wrap" data-key="claude"><div class="an-col"><span>Claude Code</span>' +
      caption(line(hooks.claude_caption, TEXT_CHARS), '', ' data-key="caption" data-user') +
      (typeof hooks.claude_path === 'string' && hooks.claude_path
        ? caption(line(hooks.claude_path, PATH_CHARS), 'an-mono an-select', ' data-key="path" data-user') : '') +
      '</div><div class="an-btns an-right" data-key="btns"><span class="value" data-key="version">' + esc(version) + '</span>' +
      (hooks.claude_path_chosen ? button('Find automatically', 'binary-auto', null, { key: 'binary', disabled: !ins.allowed }) : '') +
      button('Choose\u2026', 'binary-open', null, { disabled: !ins.allowed || state.binary !== null }) + '</div></div>';
    if (state.binary !== null) {
      // The glue's picker chooses folders only, so the path is typed or pasted here.
      html += '<div class="item an-row" data-key="binary-field"><input type="text" class="an-input an-grow an-mono" data-an-keep data-an-field="binary" data-key="binary-input"' +
        attr('value', state.binary) + ' placeholder="C:\\Users\\you\\.local\\bin\\claude.exe" aria-label="Path to claude.exe" spellcheck="false" autocomplete="off">' +
        button('Cancel', 'binary-cancel', null) +
        button('Use', 'binary-use', null, { key: 'binary', disabled: !state.binary.trim() || !ins.allowed }) + '</div>';
    }
    if (typeof hooks.last_change === 'string' && hooks.last_change) {
      html += '<div class="item cap" data-key="last-change" data-user>' + esc(line(hooks.last_change, TEXT_CHARS)) + '</div>';
    }

    var official = strings(setup.codenotch_hooks_folders);
    if (official.length) {
      html += '<div class="item cap" data-key="cn-intro">' + esc('The official ' + OFFICIAL_APP + ' has its own hooks in ' +
        plural(official.length, 'folder') + ". They run beside this app's; removing them takes out only its entries.") + '</div>';
      official.forEach(function (folder) {
        var shown = line(folder, PATH_CHARS);
        html += '<div class="item"' + attr('data-key', 'cn:' + folder) + '><span class="an-mono an-value an-head" data-an-text data-an-clip data-user' +
          attr('title', shown) + '><span class="an-bidi">' + esc(shown) + '</span></span>' +
          button('Remove ' + OFFICIAL_APP + "'s hooks", 'remove-codenotch', folder, {
            noenter: true, disabled: !ins.allowed || ins.busy, label: 'Remove ' + OFFICIAL_APP + "'s hooks from " + shown,
          }) + '</div>';
      });
    }
    strings(hooks.footnotes).forEach(function (f, i) {
      html += '<div class="item cap an-footnote" data-key="fn' + i + '">' + esc(line(f, TEXT_CHARS)) + '</div>';
    });
    html += note('hooks') + '</div>';
    return html;
  }

  // ---- render ------------------------------------------------------------------------------------

  var BUILDERS = { consent: consentHtml, accounts: accountsHtml, hooks: hooksHtml };
  var api = null;

  function render() {
    if (!C) return;
    var s = view();
    REGIONS.forEach(function (name) {
      var el = regions[name];
      if (!el) return;
      var html = '';
      if (name === 'status') html = statusHtml(s);
      else if (s && has(BUILDERS, name)) html = BUILDERS[name](s);
      else if (s && typeof SLOTS[name] === 'function') {
        try {
          html = String(SLOTS[name](s, api) || '');
        } catch (e) {
          C.log('settings: the ' + name + ' section failed to draw');
          html = '';
        }
      }
      C.morph(el, html);
      el.hidden = !html;
    });
  }

  // ---- calls ---------------------------------------------------------------------------------

  function setNote(region, text, tone) {
    if (text) state.notes[region] = { text: String(text), tone: tone || 'neutral' };
    else delete state.notes[region];
  }

  function failureText(error) {
    var e = C.callError(error);
    return { text: e.message || 'That didn\u2019t work.', tone: e.code === 'sealed' ? 'neutral' : 'critical' };
  }

  /**
   * One write: its control is disabled while it runs; a refusal (sealed, or the hub's `{error}`)
   * becomes the region's note, and nothing is retried. Resolves with the reply, or null.
   */
  function write(region, key, method, args) {
    if (state.scene || state.pending[key]) return Promise.resolve(null);
    state.pending[key] = true;
    setNote(region, null);
    render();
    return C.call(method, args).then(function (reply) {
      delete state.pending[key];
      if (reply && typeof reply === 'object' && typeof reply.error === 'string' && reply.error) {
        setNote(region, line(reply.error, TEXT_CHARS), 'critical');
        render();
        return null;
      }
      render();
      return reply || {};
    }, function (error) {
      delete state.pending[key];
      var f = failureText(error);
      setNote(region, line(f.text, TEXT_CHARS), f.tone);
      render();
      return null;
    });
  }

  /** Upstream's #toast ("Copied"). */
  function toast(text) {
    if (typeof window.toast === 'function') {
      try {
        window.toast(text);
        return;
      } catch (e) { /* nothing else to say it with */ }
    }
    var t = document.getElementById('toast');
    if (t) t.textContent = text;
  }

  function copy(region, text, then) {
    return write(region, 'copy', 'copy_text', { text: text }).then(function (reply) {
      if (reply && then) then();
      return reply;
    });
  }

  function account(key, action) {
    return write('accounts', key, 'account', { action: action });
  }

  // ---- actions (each from a click on its own control) ------------------------------------------

  /** A switch's new value: the opposite of what it showed when it was clicked. */
  function switchValue(el) {
    return el.getAttribute('data-an-on') !== '1';
  }

  ACTIONS['consent-on'] = function (where) {
    var s = view();
    var fromTop = where === 'top' && consentShown(s);
    var fromHooks = where === 'hooks' && state.reconsider && obj(s && s.hooks).consent === false;
    if ((!fromTop && !fromHooks) || installOff(s)) return;
    state.consentAnswered = true;
    state.reconsider = false;
    write('consent', 'consent-on', 'hook_consent', { grant: true }).then(function (reply) {
      if (reply) return;
      // Nothing was decided: the card comes back where it was.
      state.consentAnswered = false;
      if (fromHooks) state.reconsider = true;
      render();
    });
  };
  ACTIONS['consent-later'] = function (where) {
    var s = view();
    if (where === 'hooks') {
      // "Not now" was already the answer: asking again and declining changes nothing.
      state.reconsider = false;
      render();
      return;
    }
    if (!consentShown(s)) return;
    state.consentAnswered = true;
    write('consent', 'consent-later', 'hook_consent', { grant: false }).then(function (reply) {
      if (reply) return;
      state.consentAnswered = false;
      render();
    });
  };
  function answerScope(key, method, args) {
    if (!scopeShown(view())) return;
    state.scopeAnswered = true;
    write('consent', key, method, args).then(function (reply) {
      if (reply) return;
      state.scopeAnswered = false;
      render();
    });
  }
  ACTIONS['scope-ok'] = function () {
    answerScope('scope-ok', 'acknowledge_scope', null);
  };
  ACTIONS['scope-off'] = function () {
    answerScope('scope-off', 'hooks_enabled', { on: false });
  };
  ACTIONS.reconsider = function () {
    state.reconsider = true;
    render();
  };

  ACTIONS['hooks-enabled'] = function (arg, el) {
    write('hooks', 'hooks-enabled:', 'hooks_enabled', { on: switchValue(el) });
  };
  ACTIONS['status-line'] = function (arg, el) {
    write('hooks', 'status-line:', 'status_line_enabled', { on: switchValue(el) });
  };

  function focusField(kind) {
    var field = pane.querySelector('[data-an-field="' + kind + '"]');
    if (field && typeof field.focus === 'function') field.focus();
  }

  ACTIONS['binary-open'] = function () {
    state.binary = '';
    render();
    focusField('binary');
  };
  ACTIONS['binary-cancel'] = function () {
    state.binary = null;
    setNote('hooks', null);
    render();
  };
  function chooseBinary(path) {
    write('hooks', 'binary', 'choose_claude_binary', { path: path }).then(function (reply) {
      if (!reply) return;
      var version = typeof reply.version === 'string' && reply.version ? reply.version : null;
      if (path !== null && !version) {
        setNote('hooks', COPY.noVersion, 'warning');
      } else {
        state.binary = null;
        if (version) toast('Claude Code ' + line(version, 60));
      }
      render();
    });
  }
  ACTIONS['binary-use'] = function () {
    // A path pasted from Explorer's "Copy as path" comes in quotes.
    var path = String(state.binary || '').trim().replace(/^"(.*)"$/, '$1').trim();
    if (path) chooseBinary(path);
  };
  ACTIONS['binary-auto'] = function () {
    chooseBinary(null);
  };

  ACTIONS['remove-codenotch'] = function (folder) {
    if (strings(obj(view() && view().setup).codenotch_hooks_folders).indexOf(folder) < 0) return;
    write('hooks', 'remove-codenotch:' + folder, 'remove_codenotch_hooks', { folder: folder }).then(function (reply) {
      if (!reply) return;
      var n = Math.max(0, Math.floor(Number(reply.removed) || 0));
      toast(n ? 'Removed ' + plural(n, OFFICIAL_APP + ' hook entry', OFFICIAL_APP + ' hook entries')
        : 'No ' + OFFICIAL_APP + ' hooks were left there');
    });
  };

  // Accounts: each names its account by identity id, checked against the snapshot on screen.

  function withAccount(fn) {
    return function (id, el, event) {
      var a = accountById(id);
      if (a) fn(a, el, event);
    };
  }

  ACTIONS.folders = withAccount(function (a) {
    state.expanded[a.identity_id] = !state.expanded[a.identity_id];
    render();
  });
  ACTIONS.rename = withAccount(function (a) {
    state.renaming[a.identity_id] = a.has_custom_label ? line(a.label, NAME_CHARS) : '';
    render();
    focusField('rename');
  });
  ACTIONS['rename-cancel'] = function (id) {
    delete state.renaming[id];
    render();
  };
  function commitRename(id) {
    if (!accountById(id) || !has(state.renaming, id)) return;
    var name = String(state.renaming[id]).trim();
    delete state.renaming[id];
    // An empty name goes back to the default one.
    account('rename:' + id, { rename: { id: id, label: name ? name : null } });
  }
  ACTIONS['rename-save'] = function (id) {
    commitRename(id);
  };
  ACTIONS['ring-shown'] = withAccount(function (a, el) {
    if (!a.is_tracked) return;
    account('ring-shown:' + a.identity_id, { ring_shown: { ring_id: a.ring_id, on: switchValue(el) } });
  });
  ACTIONS.track = withAccount(function (a, el) {
    account('track:' + a.identity_id, { track: { id: a.identity_id, on: switchValue(el) } });
  });
  ACTIONS.install = withAccount(function (a) {
    var ins = installState(view());
    if (!a.can_install || !ins.allowed || ins.busy || !ins.consent) return;
    write('accounts', 'install:' + a.identity_id, 'hooks_reinstall', { account_id: a.identity_id });
  });
  ACTIONS['copy-launch'] = withAccount(function (a) {
    var id = a.identity_id;
    write('accounts', 'copy-launch:' + id, 'launch_command', { account_id: id }).then(function (reply) {
      if (!reply || typeof reply.command !== 'string' || !reply.command) return;
      copy('accounts', reply.command, function () {
        state.copied[id] = C.now() + COPIED_MS;
        render();
        toast('Copied');
        window.setTimeout(render, COPIED_MS + 10);
      });
    });
  });
  ACTIONS.reveal = withAccount(function (a) {
    var folder = list(a.folders)[0];
    if (!folder || typeof folder.id !== 'string') return;
    write('accounts', 'reveal:' + a.identity_id, 'reveal', { kind: 'config_dir', id: folder.id });
  });
  ACTIONS['forget-ask'] = withAccount(function (a) {
    state.forgetting[a.identity_id] = true;
    render();
  });
  ACTIONS['forget-cancel'] = function (id) {
    delete state.forgetting[id];
    render();
  };
  ACTIONS.forget = withAccount(function (a) {
    if (!a.can_forget || !state.forgetting[a.identity_id]) return;
    delete state.forgetting[a.identity_id];
    account('forget:' + a.identity_id, { forget: { id: a.identity_id } });
  });

  function suggestion(kind) {
    return function (path) {
      var known = list(view() && view().suggestions).some(function (sg) { return sg && sg.path === path; });
      if (!known) return;
      var action = {};
      action[kind] = { path: path };
      account(kind + ':' + path, action);
    };
  }
  ACTIONS['suggestion-dismiss'] = suggestion('suggestion_dismiss');
  ACTIONS['suggestion-add'] = suggestion('suggestion_add');

  ACTIONS['add-folder'] = function () {
    write('accounts', 'add-folder', 'pick_folder', null).then(function (reply) {
      // The picker answers `{path: null}` when it was cancelled: nothing to add.
      if (!reply || typeof reply.path !== 'string' || !reply.path) return;
      account('add-folder', { add_folder: { path: reply.path } });
    });
  };
  ACTIONS['add-open'] = function () {
    state.add = closedAdd();
    state.add.step = parallelProfiles(view()) ? 'guidance' : 'naming';
    render();
    focusField('new-account');
  };
  ACTIONS['add-naming'] = function () {
    state.add = closedAdd();
    state.add.step = 'naming';
    render();
    focusField('new-account');
  };
  ACTIONS['add-close'] = function () {
    state.add = closedAdd();
    render();
  };
  function createAccount() {
    var name = state.add.name;
    if (state.scene || state.add.step !== 'naming' || !name.trim() || state.pending['add-create']) return;
    state.pending['add-create'] = true;
    setNote('accounts', null);
    render();
    C.call('account', { action: { create: { name: name } } }).then(function (reply) {
      delete state.pending['add-create'];
      if (reply && typeof reply.created === 'string' && typeof reply.command === 'string') {
        state.add = closedAdd();
        state.add.step = 'created';
        state.add.created = reply.created;
        state.add.command = reply.command;
        render();
        copy('accounts', reply.command);
        return;
      }
      state.add.error = reply && typeof reply.error === 'string' && reply.error ? reply.error : COPY.notCreated;
      render();
    }, function (error) {
      delete state.pending['add-create'];
      state.add.error = failureText(error).text;
      render();
    });
  }
  ACTIONS['add-create'] = function () {
    createAccount();
  };
  ACTIONS['add-copy'] = function () {
    if (state.add.step === 'created' && state.add.command) {
      copy('accounts', state.add.command, function () { toast('Copied'); });
    }
  };

  // ---- events ----------------------------------------------------------------------------------

  function onClick(event) {
    var target = event.target && event.target.closest ? event.target.closest('[data-an-action]') : null;
    if (!target || !pane.contains(target) || state.scene) return;
    if (target.hasAttribute('disabled') || target.getAttribute('aria-disabled') === 'true') return;
    var name = target.getAttribute('data-an-action');
    if (!has(ACTIONS, name)) return;
    ACTIONS[name](target.getAttribute('data-an-arg'), target, event);
  }

  function onInput(event) {
    var field = event.target;
    if (!field || !field.getAttribute) return;
    var kind = field.getAttribute('data-an-field');
    var value = String(field.value == null ? '' : field.value);
    if (kind === 'rename') {
      var id = field.getAttribute('data-an-arg');
      if (has(state.renaming, id)) state.renaming[id] = value;
    } else if (kind === 'new-account') {
      state.add.name = value;
      state.add.error = '';
      render();
    } else if (kind === 'binary') {
      state.binary = value;
      render();
    }
  }

  /**
   * Enter in a field does what that field's own form does (Save a name, Create an account) and
   * nothing else; on a sensitive button it does nothing at all, so a stray Enter never turns
   * the hooks on or edits a settings.json.
   */
  function onKeydown(event) {
    var target = event.target;
    if (!target || !target.getAttribute) return;
    var kind = target.getAttribute('data-an-field');
    if (event.key === 'Enter') {
      if (kind) {
        event.preventDefault();
        if (event.repeat) return;
        if (kind === 'rename') commitRename(target.getAttribute('data-an-arg'));
        else if (kind === 'new-account') createAccount();
        return;
      }
      if (target.closest && target.closest('[data-an-noenter]')) event.preventDefault();
      return;
    }
    if (event.key === 'Escape' && kind) {
      event.preventDefault();
      if (kind === 'rename') ACTIONS['rename-cancel'](target.getAttribute('data-an-arg'));
      else if (kind === 'new-account') ACTIONS['add-close']();
      else if (kind === 'binary') ACTIONS['binary-cancel']();
    }
  }

  function forgetGone(s) {
    var ids = accountsOf(s).map(function (a) { return a.identity_id; });
    [state.renaming, state.forgetting, state.expanded, state.copied].forEach(function (map) {
      Object.keys(map).forEach(function (id) {
        if (ids.indexOf(id) < 0) delete map[id];
      });
    });
  }

  function takeSettings(s) {
    if (!s || typeof s !== 'object') return;
    if (state.cloud && !s.cloud) s.cloud = state.cloud;
    state.cloud = null;
    state.settings = s;
    state.failure = null;
    var setup = obj(s.setup);
    if (!setup.needs_hook_consent) state.consentAnswered = false;
    if (!strings(setup.new_install_folders).length) state.scopeAnswered = false;
    if (obj(s.hooks).consent !== false) state.reconsider = false;
    forgetGone(s);
    // The first launch opens Settings on this pane (DESIGN-WIN §5.4): consent is asked here.
    if (setup.needs_hook_consent && !state.tabOpened && !state.scene && typeof window.showTab === 'function') {
      state.tabOpened = true;
      try {
        window.showTab('claude');
      } catch (e) { /* the tab stays where it is */ }
    }
    render();
  }

  // ---- rebrand of upstream's copy (DESIGN-WIN §5.5) --------------------------------------------

  function wireRebrand() {
    R = window.agentnotchRebrand;
    if (!R) return;
    var orig = window.ui;
    if (typeof orig === 'function' && !orig.__anRebranded) {
      var wrapped = function (key, fallback) {
        return R.rebrand(orig(key, fallback));
      };
      wrapped.__anRebranded = true;
      window.ui = wrapped;
    }
    var title = document.title;
    var next = R.rebrand(title);
    if (next !== title) document.title = next;
    var app = document.getElementById('app');
    R.rebrandTree(app);
    // All of #app, #body included: a language switch rewrites the sidebar ("Quit …") too, from
    // upstream's English text, outside #body.
    R.observe(app || document.getElementById('body'));
  }

  // ---- scenes and the layout report (the sealed self-test, T/README.md) -----------------------

  function clone(value) {
    return value == null ? value : JSON.parse(JSON.stringify(value));
  }

  function folder(id, title, role, stateText, codenotch) {
    return { id: id, title: title, role: role, state: stateText, status_line_note: null, not_hookable: null, codenotch_hooks: !!codenotch };
  }

  /** The Mac's Parallel Profiles sheets: ~\.claude, VS Code workspaces and account stores. */
  function profilesAccounts(s, named) {
    var home = 'C:\\Users\\me\\';
    var a = s.accounts[0];
    var b = s.accounts[1];
    if (!a || !b) return;
    a.folders = [
      folder(home + '.claude', '~\\.claude', 'Default', 'Hooks and live status line'),
      folder(home + '.claude-windows\\5d1e0a7b3c21', 'VS Code \u00B7 dotfiles', '~\\.claude-windows\\5d1e0a7b3c21', 'Hooks and live status line'),
      folder(home + '.claude-me', '~\\.claude-me', 'Account store', 'Read only, never changed'),
    ];
    a.folder_summary = 'Runs in ~\\.claude and 1 VS Code workspace\nStore (Claude Parallel Profiles): ~\\.claude-me';
    a.hook_state = 'Hooks in 2 of 2 folders';
    var bState = named ? 'Hooks not installed' : 'Hooks and live status line';
    b.folders = [
      folder(home + '.claude-windows\\9b4f2e8d6a10', 'VS Code \u00B7 checkout-web', '~\\.claude-windows\\9b4f2e8d6a10', bState, named),
      folder(home + '.claude-windows\\c07a3f5e1d94', '~\\.claude-windows\\c07a3f5e1d94', 'VS Code workspace', bState),
      folder(home + '.claude-company', '~\\.claude-company', 'Account store', 'Read only, never changed'),
      folder(home + '.claude-company-team', '~\\.claude-company-team', 'Account store', 'Read only, never changed'),
    ];
    b.folder_summary = 'Runs in 2 VS Code workspaces\nStores (Claude Parallel Profiles):\n  ~\\.claude-company\n  ~\\.claude-company-team';
    b.launch_command = null;
    if (!named) {
      b.hook_state = 'Hooks in 2 of 2 folders';
      b.live_status_line = true;
      s.accounts.forEach(function (x) {
        x.label = x.default_label;
        x.has_custom_label = false;
      });
      s.suggestions = [];
      s.hooks.summary = 'Installed in all 4 folders of 2 tracked accounts.';
      return;
    }
    b.hook_state = 'Hooks in 0 of 2 folders';
    b.hook_state_tone = 'warning';
    b.live_status_line = false;
    b.hook_problem = 'Its sessions still show; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are in.';
    b.install_label = 'Install hooks';
    s.accounts.push({
      identity_id: 'uuid:side', ring_id: 'claude-acct-side', label: 'Side project', default_label: 'Claude Side',
      has_custom_label: true, color_index: 2, is_default: false,
      identity_line: 'hello@side.example \u00B7 Pro \u00B7 ~\\.claude-side', folder_summary: '', folders: [],
      usage_line: "Not checked while it isn't tracked", usage_stale: false, hook_state: 'Hooks off',
      hook_state_tone: 'neutral', live_status_line: false, hook_problem: null, hook_problem_critical: false,
      is_tracked: false, ring_shown: false, is_signed_in: true,
      launch_command: "$env:CLAUDE_CONFIG_DIR='C:\\Users\\me\\.claude-side'; claude",
      can_install: false, install_label: 'Install hooks', can_forget: true, forget_caption: a.forget_caption || '',
    });
    s.unsigned_folders = ['~\\.claude-windows\\0a1b2c3d4e5f'];
    s.setup.codenotch_hooks_folders = [home + '.claude-windows\\9b4f2e8d6a10'];
    s.hooks.summary = 'Installed in 2 of 4 folders of 2 tracked accounts.';
    s.hooks.summary_warning = true;
  }

  var SCENES = {
    /** The pane as the hub sends it (the Mac's "settings" sheet). */
    'settings-full': function () {},
    'settings-first-run': function (s) {
      s.hooks.consent = null;
      s.hooks.enabled = false;
      s.hooks.status_line = false;
      s.hooks.enabled_locked = true;
      s.hooks.summary = COPY.turnOnFirst;
      s.hooks.last_change = null;
      s.setup.hook_consent = null;
      s.setup.needs_hook_consent = true;
      s.accounts.forEach(function (a) {
        a.hook_state = 'Hooks off';
        a.hook_state_tone = 'neutral';
        a.live_status_line = false;
        a.can_install = false;
        a.folders.forEach(function (f) { f.state = 'Hooks off'; });
      });
    },
    'settings-scope-notice': function (s) {
      s.setup.new_install_folders = ['VS Code \u00B7 dotfiles', '~\\.claude-windows\\c07a3f5e1d94'];
    },
    'settings-parallel-profiles': function (s) {
      profilesAccounts(s, true);
      return { expanded: true, add: 'guidance' };
    },
    'settings-unnamed-accounts': function (s) {
      profilesAccounts(s, false);
      return { expanded: true };
    },
  };

  function resetView() {
    state.consentAnswered = false;
    state.scopeAnswered = false;
    state.reconsider = false;
    state.renaming = Object.create(null);
    state.forgetting = Object.create(null);
    state.expanded = Object.create(null);
    state.copied = Object.create(null);
    state.add = closedAdd();
    state.binary = null;
    state.notes = Object.create(null);
  }

  /** Shows a sealed scene from the settings the page holds; false for a name it doesn't know. */
  function showScene(name) {
    if (!C || !state.settings) return false;
    var make = has(SCENES, name) ? SCENES[name] : has(SLOT_SCENES, name) ? SLOT_SCENES[name] : null;
    if (typeof make !== 'function') return false;
    C.setStatic(true);
    resetView();
    var s = clone(state.settings);
    s.setup = obj(s.setup);
    s.hooks = obj(s.hooks);
    s.accounts = accountsOf(s);
    s.accounts.forEach(function (a) { a.folders = list(a.folders); });
    var extra = obj(make(s));
    state.scene = name;
    state.sceneSettings = s;
    if (extra.expanded) s.accounts.forEach(function (a) { state.expanded[a.identity_id] = true; });
    if (typeof extra.add === 'string') state.add.step = extra.add;
    if (typeof window.showTab === 'function') {
      try {
        window.showTab(typeof extra.tab === 'string' ? extra.tab : 'claude');
      } catch (e) { /* the scene still draws */ }
    }
    render();
    if (typeof extra.scroll === 'string' && has(regions, extra.scroll)) scrollTo(regions[extra.scroll]);
    return true;
  }

  /** A scene of one section (the Mac's settings-cloud-* sheets) starts at that section. */
  function scrollTo(el) {
    var body = document.getElementById('body');
    if (!body || !el.getBoundingClientRect || !body.getBoundingClientRect) return;
    var top = el.getBoundingClientRect().top - body.getBoundingClientRect().top;
    body.scrollTop = Math.max(0, (body.scrollTop || 0) + top - 8);
  }

  function insideAcross(inner, outer, slack) {
    var s = slack || 0;
    return inner.left >= outer.left - s && inner.right <= outer.right + s;
  }

  function cutWithEllipsis(el) {
    if (!el.hasAttribute('data-an-clip')) return false;
    var style = window.getComputedStyle ? window.getComputedStyle(el) : null;
    return !!style && style.textOverflow === 'ellipsis';
  }

  /** The pane's layout invariants: no clipped text, every control inside the pane, no sideways scroll. */
  function layoutReport() {
    var failures = [];
    var pr = pane.getBoundingClientRect();
    var texts = 0;
    Array.prototype.forEach.call(pane.querySelectorAll('[data-an-text]'), function (el) {
      texts += 1;
      if (el.scrollWidth > el.clientWidth + 1 && !cutWithEllipsis(el)) failures.push('text is clipped: ' + C.oneLine(el.textContent).slice(0, 40));
    });
    Array.prototype.forEach.call(pane.querySelectorAll('button, [data-an-action], [role="button"], a[href], input, select, textarea'), function (el) {
      var r = el.getBoundingClientRect();
      if (r.width && pr.width && !insideAcross(r, pr, 0.5)) {
        failures.push('control outside the pane: ' + (el.getAttribute('aria-label') || C.oneLine(el.textContent)).slice(0, 40));
      }
    });
    var body = document.getElementById('body');
    if (body && body.scrollWidth > body.clientWidth + 1) failures.push('the pane scrolls sideways');
    return { ok: failures.length === 0, failures: failures, scene: state.scene, texts: texts, pane: { width: pr.width, height: pr.height } };
  }

  // ---- start -------------------------------------------------------------------------------------

  var started = false;

  function start() {
    if (started || !window.agentnotchCommon) return;
    started = true;
    C = window.agentnotchCommon;
    while (pane.firstChild) pane.removeChild(pane.firstChild);
    REGIONS.forEach(function (name) {
      var el = document.createElement('div');
      el.className = 'an-region';
      el.setAttribute('data-an-section', name);
      el.hidden = true;
      pane.appendChild(el);
      regions[name] = el;
    });
    pane.addEventListener('click', onClick);
    pane.addEventListener('input', onInput);
    pane.addEventListener('keydown', onKeydown);
    api = {
      esc: esc, line: line, attr: attr, button: button, toggle: toggle, link: link, caption: caption,
      note: note, dot: dot, write: write, toast: toast, render: render, state: state, obj: obj, list: list,
    };
    C.listen('an:settings', function (payload) { takeSettings(payload); });
    C.listen('an:cloud', function (payload) {
      if (!payload || typeof payload !== 'object') return;
      if (state.settings) {
        state.settings.cloud = payload;
        render();
      } else {
        state.cloud = payload;
      }
    });
    C.call('settings', null).then(takeSettings, function (error) {
      if (state.settings) return;
      state.failure = C.callError(error);
      render();
    });
    // The window stays open between visits, and Windows' banner switch can change meanwhile
    // (Open… in Notifications sends the user there): asking again when the window comes to the
    // front makes the hub read it afresh. A change comes back as `an:settings`, ordered with the
    // hub's other events; this reply isn't, and could be older than one of them, so it is unused.
    window.addEventListener('focus', function () {
      C.call('settings', null).then(null, function () {});
    });
    // The rebrand, and the second half of the pane (Usage, Cloud, Sessions and attention,
    // Notifications, Advanced: settings-sections.js fills `slots`), then a draw with them.
    C.load(base, ['rebrand.js', 'settings-sections.js'], function () {
      wireRebrand();
      render();
    });
  }

  window.agentnotchSettings = {
    pane: pane,
    render: function () { render(); },
    showScene: showScene,
    layoutReport: layoutReport,
    /** settings-sections.js: section renderers (`slots.usage = fn(settings, api)`) and their scenes. */
    slots: SLOTS,
    scenes: SLOT_SCENES,
    actions: ACTIONS,
    _: { state: state, SCENES: SCENES, COPY: COPY },
  };

  if (window.agentnotchCommon) {
    start();
  } else {
    var s = document.createElement('script');
    s.src = base + 'common.js';
    s.async = false;
    s.onload = start;
    s.onerror = function () {
      pane.textContent = COPY.notRunning;
    };
    (document.head || document.documentElement).appendChild(s);
  }
})();
