//! `hook [--exec]`: one Claude Code hook event (DESIGN-WIN §1.4).
//!
//! The finished flow: read the event from stdin, arm the watchdog, build the message
//! (`agentnotch_proto::build_hook_message`, with `pid` from `CLAUDE_PID` or §1.4's ancestor walk),
//! connect to `\\.\pipe\agentnotch-hook-<SID>` after checking the pipe's owner and DACL, write one
//! frame; for a PermissionRequest disarm the watchdog and wait for the answer, then print
//! `agentnotch_proto::permission_output`. The app not running, a foreign pipe, a stall, a broken
//! pipe or an unreadable answer all end the same way: nothing printed, exit 0, and Claude Code's
//! own prompt decides.
//!
//! This build has no pipe client yet, so every event takes that fail-open path: the event is
//! read (Claude Code writes it and closes stdin before it waits on us) and dropped.

use crate::io;
use crate::watchdog::{Watchdog, HOOK_BUDGET};

/// Stdin larger than this is not a hook event worth forwarding. The message is truncated to the
/// protocol's limits after it is parsed (HS§1.5), so this only bounds the read itself: a Write
/// tool's input can be several MiB before truncation.
const STDIN_LIMIT: usize = 64 << 20;

pub fn run(exec_form: bool) {
    let Some(event) = io::read_stdin(STDIN_LIMIT) else {
        return;
    };
    // Armed from here on, as in the finished hook: nothing below may cost Claude Code more than
    // the budget, whatever the pipe does.
    let watchdog = Watchdog::arm(HOOK_BUDGET);
    deliver(exec_form, &event);
    watchdog.disarm();
}

/// Hands the event to the app. Not wired up in this build: the event is dropped, which is exactly
/// what the finished hook does when the app is not running.
fn deliver(_exec_form: bool, _event: &[u8]) {}
