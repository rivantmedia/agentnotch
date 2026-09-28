//! Typing a reply into a session's console (DESIGN-WIN §3.2 `ConsoleInput`, §4.8; WP1): runs
//! `agentnotch-hook.exe type --pid … --started … --expect-window … --shells …` on the `an-ui`
//! worker, answers the helper's `{"phase":"typed"}` with `submit` only when the engine's fresh
//! `recheck` agrees within 2 s, and maps its final line to a `TypeOutcome`.
//!
//! Not implemented in this build: nothing is typed and the outcome says so. Typing replies is
//! off by default on Windows (`typeReplies`), so the chat offers "Show terminal" instead.

use std::path::{Path, PathBuf};

use agentnotch_engine::platform::{ConsoleInput, ConsoleTarget, TypeOutcome};

use crate::NOT_IMPLEMENTED;

#[derive(Debug)]
pub struct ConsoleHelper {
    /// The installed `agentnotch-hook.exe`, which does the typing.
    #[allow(dead_code)]
    helper: PathBuf,
}

impl ConsoleHelper {
    pub fn new(helper: &Path) -> Self {
        ConsoleHelper {
            helper: helper.to_path_buf(),
        }
    }
}

impl ConsoleInput for ConsoleHelper {
    fn type_text(
        &self,
        _target: &ConsoleTarget,
        _text: &str,
        _recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome {
        TypeOutcome::Failed(format!("Typing into a terminal is {NOT_IMPLEMENTED}."))
    }
}
