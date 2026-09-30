// The chat screen of the sessions panel (DESIGN-WIN §5.3, UI§6). panel.html loads this before
// panel.js so the script list is final; the transcript, its bottom bars and the composer arrive
// with sub-tasks 7 and 8. Until then the page mounts nothing.
//
// The contract with panel.js: on a `session:<id>` route it calls `mount(host, ctx)` once with the
// chat's own region (`#an-chat`, already shown) and `ctx = {sessionId, panel}` (`panel` is
// `window.agentnotchPanel`), and `unmount()` when the route leaves the chat or changes session.
// One IIFE, one global, no top-level let/const (a clash with a classic script kills the page).
(function () {
  'use strict';

  window.agentnotchChat = {
    mount: function () {},
    unmount: function () {},
  };
})();
