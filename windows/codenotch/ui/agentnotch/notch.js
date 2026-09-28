// Agent Notch's additions to upstream's notch page (DESIGN-WIN §5.2).
//
// notch.html calls these hooks at five tagged seams (WS1-WS5), always behind
// `if(window.agentnotch)`, so the page works with or without this file. Loaded before the page's
// own script: one IIFE, one global, no top-level let/const (a clash with upstream's classic-script
// globals would be a SyntaxError that kills the whole page).
//
// This is the scaffold: every hook answers "not mine", so upstream's own behaviour runs unchanged
// (its Claude cells come from the fork's usage projection in `usage`). The rings, badges, hover
// rows and ring clicks of §5.2 arrive with the fork's UI package (WP10).
(function () {
  'use strict';

  window.agentnotch = {
    // WS2: a new array of upstream cell objects, one per shown Claude ring; null = upstream's own.
    claudeCells: function () {
      return null;
    },
    // WS3: decorates a Claude ring's cell after upstream drew it (activity, badges, stale).
    decorateCell: function (p, cell) {
      void p;
      void cell;
    },
    // WS4: the Claude hover card's session rows, as escaped HTML.
    cardSessions: function (p) {
      void p;
      return '';
    },
    // WS5: true when a ring click was handled here (the sessions panel), false for upstream's
    // refresh.
    ringClick: function (cellId) {
      void cellId;
      return false;
    },
  };
})();
