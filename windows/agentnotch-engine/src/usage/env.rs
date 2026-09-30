//! The environment a usage check or a session summary runs `claude` with
//! (§4.6; UsageProbe.environment, CL§9.3): one list for both.
//!
//! The child never inherits what Claude Code sets for its own subprocesses
//! (a GUI app normally has none, but a run started from a Claude Code
//! terminal does), nor anything that would make it run as another login:
//! `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR` pick the folder
//! whose login is used, and the two `ANTHROPIC_*` names replace the login
//! altogether. With any of them left in, the check would spend, and answer
//! for, another account.

use crate::core::paths::PathStyle;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Removed whatever their case (environment names are case-insensitive on
/// Windows).
const REMOVED_NAMES: [&str; 8] = [
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "AI_AGENT",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Every name with one of these prefixes is removed too.
const REMOVED_PREFIXES: [&str; 2] = ["CLAUDE_CODE_", "CLAUDE_AGENT_SDK_"];

const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
const PATH: &str = "PATH";

/// Whether a variable of the app's environment is kept out of the child's.
pub fn is_scrubbed(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_uppercase();
    REMOVED_NAMES.contains(&name.as_str())
        || REMOVED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// The complete environment of the child, in the native style.
///
/// - `base`: the app's own environment.
/// - `config_dir_env`: the folder's raw `CLAUDE_CONFIG_DIR`; `None` (or
///   empty) for `~\.claude`, which runs with the variable unset.
/// - `binary_dir`: the folder of the program that is run, put first on
///   `PATH` so a shim finds the interpreter beside it.
pub fn scrubbed_env(
    base: &[(OsString, OsString)],
    config_dir_env: Option<&str>,
    binary_dir: &Path,
) -> Vec<(OsString, OsString)> {
    scrubbed_env_in(PathStyle::native(), base, config_dir_env, binary_dir)
}

/// [`scrubbed_env`] with the rules of `style` (`;` and case-insensitive
/// folders on Windows), so both are tested on every OS.
pub fn scrubbed_env_in(
    style: PathStyle,
    base: &[(OsString, OsString)],
    config_dir_env: Option<&str>,
    binary_dir: &Path,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = base
        .iter()
        .filter(|(name, _)| !is_scrubbed(name))
        .cloned()
        .collect();

    let binary_dir = binary_dir.as_os_str();
    if !binary_dir.is_empty() {
        let is_path = |name: &OsStr| name.to_string_lossy().eq_ignore_ascii_case(PATH);
        match env.iter_mut().find(|(name, _)| is_path(name)) {
            // The variable keeps the spelling it had (`Path` on Windows).
            Some((_, value)) => *value = prepend(style, binary_dir, value),
            None => env.push((PATH.into(), binary_dir.to_owned())),
        }
    }

    if let Some(config_dir) = config_dir_env.filter(|dir| !dir.is_empty()) {
        env.push((CONFIG_DIR.into(), config_dir.into()));
    }
    env
}

/// `dir`, then `path` as it was (never re-encoded: a `PATH` may hold text
/// that isn't Unicode).
fn prepend(style: PathStyle, dir: &OsStr, path: &OsStr) -> OsString {
    let mut joined = dir.to_owned();
    if !path.is_empty() {
        joined.push(style.list_separator().to_string());
        joined.push(path);
    }
    joined
}
