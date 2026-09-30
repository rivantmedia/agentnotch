//! A trace of what the exe did, for development and the tests on Windows CI.
//!
//! A hook that fails open is silent by design: it prints nothing and exits 0 whether it delivered
//! its event, found no app, or refused a pipe that was not the app's. A test (or the hermetic
//! Claude Code job) still has to tell those apart, so with **both** `AGENTNOTCH_DEV=1` and
//! `AGENTNOTCH_HOOK_TRACE=<file>` set, each run appends lines `<unix ms> <pid> <what>` to that
//! file. A real session has neither variable (the same rule as the pipe override, so a leftover
//! export never makes every hook write a file), and nothing is ever written to stdout or stderr.
//!
//! Lines name states only (`invoked hook exec=true`, `no app`, `sent PreToolUse 412 bytes`,
//! `answered`): never a prompt, a tool input, a path or the value of a variable.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

fn target() -> Option<&'static PathBuf> {
    static TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();
    TARGET
        .get_or_init(|| {
            if std::env::var_os("AGENTNOTCH_DEV").is_none_or(|dev| dev != "1") {
                return None;
            }
            std::env::var_os("AGENTNOTCH_HOOK_TRACE")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .as_ref()
}

/// Appends one line; `what` is only built when tracing is on. Every failure is ignored.
pub fn note(what: impl FnOnce() -> String) {
    let Some(path) = target() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    // One write per line, so lines of hooks running at the same time never interleave.
    let line = format!("{now} {} {}\n", std::process::id(), what());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}
