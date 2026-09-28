//! `statusline`: the account's status line command (DESIGN-WIN §1.4, §4.3).
//!
//! The finished wrapper runs the user's previous status line command through Git Bash (when it
//! was safe to take over), relays its stdout and exit code, and forwards the status JSON to the
//! app on its own thread with a 0.3 s budget, so the relay never waits on the app. The loop guard
//! (`AGENTNOTCH_STATUSLINE_DEPTH`) keeps a wrapper that reaches itself from chaining again.
//!
//! This build neither chains nor forwards: it reads the status JSON Claude Code sends and prints
//! nothing, so the status line is simply empty, and exits 0.

use crate::io;

/// Claude Code's status JSON is a few KiB; anything far larger is not one.
const STDIN_LIMIT: usize = 8 << 20;

pub fn run() {
    let _status = io::read_stdin(STDIN_LIMIT);
}
