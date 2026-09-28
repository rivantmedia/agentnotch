// Agent Notch's Claude Code pane in upstream's Settings page (DESIGN-WIN §5.4).
//
// Loaded by seam WSS5 after the page's own script, into the `#pane-claude` section that seams
// WSS1-WSS4 add. One IIFE, one global (`window.agentnotchSettings`), no top-level let/const.
// Everything is built with textContent, never HTML strings: names shown here come from Claude
// accounts and the website, not from this app.
//
// This is the scaffold: the pane's heading and whether Claude Code control answers. Its
// sections (consent, accounts, hooks, usage, cloud, sessions, notifications, advanced) arrive
// with the fork's UI package (WP10).
(function () {
  'use strict';

  var pane = document.getElementById('pane-claude');
  if (!pane) return;

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text) node.textContent = text;
    return node;
  }

  var heading = el('div', 'sec', 'Claude Code');
  var group = el('div', 'group');
  var about = el('div', 'item cap an-caption', 'Sessions, approvals, accounts and hooks.');
  var status = el('div', 'item cap an-caption');
  status.setAttribute('data-an-status', '');
  group.appendChild(about);
  group.appendChild(status);
  pane.appendChild(heading);
  pane.appendChild(group);

  function show(settings) {
    status.textContent = settings && settings.sealed
      ? 'Sealed: this window shows sample data.'
      : '';
    status.hidden = !status.textContent;
  }

  var tauri = window.__TAURI__;
  if (!tauri || !tauri.core) {
    show(null);
    return;
  }
  tauri.core.invoke('an_call', { method: 'settings', args: null }).then(show, function (error) {
    status.textContent = (error && error.message) || "Claude Code control isn't running.";
    status.hidden = false;
  });
  if (tauri.event) tauri.event.listen('an:settings', function (e) { show(e.payload); });

  window.agentnotchSettings = { pane: pane };
})();
