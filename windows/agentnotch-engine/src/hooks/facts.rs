//! What the committed facts file (`tests/fixtures/claude-code-facts.json`,
//! written by the facts job, §6.2) establishes about Claude Code's builds,
//! compiled in. `EXEC_FORM_MIN` is `None` until the file sets it; exec-form
//! hooks are never written before.
//!
//! Owner: WP2 (the file: WP11). WP0 stub.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeFacts {
    /// The first Claude Code version that runs exec-form hooks
    /// (`{"command", "args"}`) and ignores unknown keys safely.
    pub exec_form_min: Option<String>,
}

impl ClaudeCodeFacts {
    /// The facts this build knows: none yet.
    pub fn compiled_in() -> ClaudeCodeFacts {
        ClaudeCodeFacts::default()
    }
}
