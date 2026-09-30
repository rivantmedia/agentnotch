//! `hook [--exec]`: one Claude Code hook event (DESIGN-WIN §1.4; the Mac's agentnotch-hook.py).
//!
//! Read the event from stdin, arm the watchdog, open this user's hook pipe (no pipe means the app
//! is not running: done), check that the pipe is the app's, build the message and write one
//! frame. Every event but one ends there. A PermissionRequest then waits for the app's decision
//! and prints Claude Code's hook output for it.
//!
//! The app not running, a pipe that is not ours, a stall, a broken pipe, an event that is not
//! JSON, a decision that is "ask" or unreadable: all end the same way, with nothing printed and
//! exit 0, and Claude Code's own prompt (shown in parallel) decides.

use crate::watchdog::{Watchdog, DECISION_WAIT, HOOK_BUDGET};
use crate::{ancestry, io, pipe, trace};
use agentnotch_proto::limits::MAX_RESPONSE;
use agentnotch_proto::{
    build_hook_message, encode_hook_message, permission_output_for_frames, HookEnv,
};
use serde_json::Value;

/// Stdin larger than this is not a hook event worth forwarding. The message is truncated to the
/// protocol's limits after it is parsed (HS§1.5), so this only bounds the read itself: a Write
/// tool's input can be several MiB before truncation.
const STDIN_LIMIT: usize = 64 << 20;

pub fn run(exec_form: bool) {
    let Some(stdin) = io::read_stdin(STDIN_LIMIT) else {
        trace::note(|| "stdin: unreadable or too large".into());
        return;
    };
    // Armed from here on: nothing below may cost Claude Code more than the budget, whatever the
    // pipe does.
    let watchdog = Watchdog::arm(HOOK_BUDGET);
    forced_panic_for_tests();

    let mut env = HookEnv::from_env(io::env_text, std::process::id());
    // Whether Claude Code's variables reached the hook; never what they hold.
    trace::note(|| {
        format!(
            "env claude_pid={} config_dir={}",
            set_or_unset(env.claude_pid.is_some()),
            set_or_unset(env.claude_config_dir.is_some())
        )
    });

    let mut pipe = match pipe::connect(HOOK_BUDGET) {
        Ok(pipe) => pipe,
        Err(why) => {
            trace::note(|| why.as_str().into());
            return;
        }
    };
    let Ok(event) = serde_json::from_slice::<Value>(&stdin) else {
        trace::note(|| "stdin: not JSON".into());
        return;
    };

    if env.claude_pid_value().is_none() {
        env.pid_guess = ancestry::claude_pid(exec_form);
    }
    let Some(message) = build_hook_message(&event, &env) else {
        trace::note(|| "stdin: not a JSON object".into());
        return;
    };
    let name = message["event"].as_str().unwrap_or_default().to_owned();
    let label = event_label(&name);
    let frame = encode_hook_message(&message);
    if !pipe::send(&mut pipe, &frame) {
        trace::note(|| format!("send failed: {label}"));
        return;
    }
    trace::note(|| {
        let pid = message["pid"]
            .as_u64()
            .map_or_else(|| "none".to_owned(), |pid| pid.to_string());
        format!("sent {label} {} bytes pid={pid}", frame.len())
    });
    if name != "PermissionRequest" {
        // Fire and forget: closing the pipe (on return) is the end of the message.
        return;
    }

    // Only the answer is awaited now, for as long as Claude Code lets the hook run. The terminal
    // shows its own prompt meanwhile; whichever is answered first wins.
    watchdog.rearm(DECISION_WAIT);
    let Some(response) = pipe::receive(&mut pipe, MAX_RESPONSE) else {
        trace::note(|| "no decision".into());
        return;
    };
    // Merged onto the ORIGINAL input from stdin, never onto the truncated copy the app saw.
    match permission_output_for_frames(&stdin, &response) {
        Some(output) => {
            trace::note(|| "answered".into());
            io::write_stdout(output.as_bytes());
        }
        None => trace::note(|| "decision: ask".into()),
    }
}

fn set_or_unset(set: bool) -> &'static str {
    if set {
        "set"
    } else {
        "unset"
    }
}

/// The event's name as the trace shows it. The name comes from stdin, and the trace holds no
/// text from an event: only what an event name is made of gets through, and not much of it.
fn event_label(name: &str) -> String {
    let label: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(40)
        .collect();
    if label.is_empty() {
        "?".into()
    } else {
        label
    }
}

/// A test switch, compiled into debug builds only (never into the shipped exe): panics inside
/// the hook body, so `tests/fail_open.rs` can prove a panic still ends as exit 0 with nothing
/// printed.
fn forced_panic_for_tests() {
    #[cfg(debug_assertions)]
    if std::env::var_os("AGENTNOTCH_HOOK_TEST_PANIC").is_some() {
        panic!("forced by AGENTNOTCH_HOOK_TEST_PANIC");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_shows_event_names_and_nothing_else() {
        assert_eq!(event_label("PermissionRequest"), "PermissionRequest");
        assert_eq!(event_label(""), "?");
        assert_eq!(event_label("C:\\Users\\me secret\n"), "CUsersmesecret");
        assert_eq!(event_label(&"x".repeat(500)).len(), 40);
        assert_eq!(set_or_unset(true), "set");
        assert_eq!(set_or_unset(false), "unset");
    }
}
