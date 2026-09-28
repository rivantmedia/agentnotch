//! `type` and `console-info`: the console helper the app runs (DESIGN-WIN §1.1, §4.8).
//!
//! The GUI app never attaches to a console itself; it runs this exe (CREATE_NO_WINDOW), which
//! `FreeConsole`s, `AttachConsole`s to the session's Claude and either describes that console or
//! types a reply into it in two phases (the text, then Return only after the engine re-checks).
//! Both answer on stdout with one JSON object per line and exit 0, whatever happened.
//!
//! This build attaches to nothing: `type` reports a failure the app shows as such, and
//! `console-info` reports no console, so the engine offers no typing and plans focus from the
//! process tree alone.

use crate::io;
use agentnotch_proto::TypeArgs;

const NOT_AVAILABLE: &str = "Typing into a terminal isn't available in this build.";

/// Two-phase typing. Its final line is `{"outcome": …, "reason"?: …}`; with nothing typed,
/// there is no `{"phase":"typed"}` line before it.
pub fn type_reply(_target: &TypeArgs) {
    let outcome = serde_json::json!({ "outcome": "failed", "reason": NOT_AVAILABLE });
    io::write_stdout(&format!("{outcome}\n"));
}

/// `ConsoleInfo` of the engine's platform traits, as JSON.
pub fn info(_pid: u32) {
    let info = serde_json::json!({
        "attached": false,
        "window": null,
        "title": null,
        "processes": [],
        "line_input": null,
        "elevated_target": false,
        "error": NOT_AVAILABLE,
    });
    io::write_stdout(&format!("{info}\n"));
}
