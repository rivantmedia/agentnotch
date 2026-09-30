//! What the status line wrapper chains to (DESIGN-WIN §4.3, HS§2).
//!
//! When the app takes over an account's `statusLine`, it saves the object it
//! replaced beside the hook exe ([`PREVIOUS_FILE_NAME`]); the wrapper runs
//! that command and passes its output through. The rules for what it may run
//! are here, pure, so the wrapper and the installer cannot disagree:
//! a wrapper must never chain to a wrapper, or every status line render
//! would start another process, which starts another.

use serde_json::Value;

/// The replaced `statusLine` object, verbatim, beside the hook exe; `{}`
/// means "chain nothing".
pub const PREVIOUS_FILE_NAME: &str = "agentnotch-statusline.previous.json";

/// Set (to `1`) in the environment of the command the wrapper chains. A
/// wrapper that finds it set was reached through another wrapper, under
/// whatever spelling of its path, and chains nothing.
pub const DEPTH_ENV: &str = "AGENTNOTCH_STATUSLINE_DEPTH";

/// Status line wrappers of this app, under this name, its Mac scripts' names
/// and its former names: never chained to.
const WRAPPER_NAMES: [&str; 3] = [
    "agentnotch-statusline",
    "superpowered-codenotch-statusline",
    "superpowered-notch-statusline",
];

/// The command of the status line the wrapper replaced, from the bytes of
/// the previous-status-line file; `None` to chain nothing.
///
/// Nothing is chained when the file isn't a JSON object, when its `command`
/// isn't a non-blank string, or when that command is one of our own
/// wrappers ([`is_own_wrapper`]).
pub fn previous_command(previous_json: &[u8]) -> Option<String> {
    // An editor may have saved the file with a byte order mark.
    let bytes = previous_json
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(previous_json);
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let command = value.as_object()?.get("command")?.as_str()?;
    if command.trim().is_empty() || is_own_wrapper(command) {
        return None;
    }
    Some(command.to_owned())
}

/// Whether a status line command runs one of this app's own wrappers.
///
/// Case is ignored, as Windows ignores it in paths, and so is how the path
/// is written (`/` or `\`, quoted or not): the command counts when it names
/// `agentnotch-hook` together with `statusline`, the hook exe's 8.3 alias
/// (`AGENTN~1.EXE`) together with `statusline`, or a wrapper script of this
/// app or its former names. A wrapper reached under a spelling this cannot
/// see is stopped by [`DEPTH_ENV`] instead.
pub fn is_own_wrapper(command: &str) -> bool {
    let lower = command.to_lowercase();
    if WRAPPER_NAMES.iter().any(|name| lower.contains(name)) {
        return true;
    }
    lower.contains("statusline") && (lower.contains("agentnotch-hook") || names_short_alias(&lower))
}

/// `agentn~<digits>.exe`: the short (8.3) name Windows gives
/// `agentnotch-hook.exe`. `lower` is already lower-cased.
fn names_short_alias(lower: &str) -> bool {
    const STEM: &str = "agentn~";
    let mut rest = lower;
    while let Some(at) = rest.find(STEM) {
        let after = &rest[at + STEM.len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(".exe") {
            return true;
        }
        rest = after;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previous(command: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"type": "command", "command": command}))
            .unwrap_or_default()
    }

    #[test]
    fn a_plain_command_is_chained() {
        assert_eq!(
            previous_command(&previous("cat; echo TAIL; exit 3")).as_deref(),
            Some("cat; echo TAIL; exit 3")
        );
        assert_eq!(
            previous_command(br#"{"type":"command","command":"npx ccstatusline","padding":0}"#)
                .as_deref(),
            Some("npx ccstatusline")
        );
        // A byte order mark in front does not hide it.
        let mut with_bom = b"\xEF\xBB\xBF".to_vec();
        with_bom.extend_from_slice(&previous("echo hi"));
        assert_eq!(previous_command(&with_bom).as_deref(), Some("echo hi"));
    }

    #[test]
    fn nothing_to_chain() {
        for bytes in [
            &b"{}"[..],
            b"{}\n",
            b"",
            b"not json",
            b"[1]",
            b"\"echo hi\"",
            br#"{"command": 5}"#,
            br#"{"command": ""}"#,
            br#"{"command": "  \t "}"#,
            br#"{"type": "command"}"#,
        ] {
            assert_eq!(
                previous_command(bytes),
                None,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    /// StatusLineScriptTests.neverChainsToAWrapper, with the Windows spellings.
    #[test]
    fn never_chains_to_a_wrapper() {
        for command in [
            // The Mac's script names, and the former names.
            "python3 '/x/hooks/agentnotch-statusline.py'",
            "python3 '/x/hooks/superpowered-codenotch-statusline.py'",
            "python3 '/x/hooks/superpowered-notch-statusline.py'",
            // The Windows wrapper, however its path is written.
            "C:/Users/me/.claude/hooks/agentnotch-hook.exe statusline",
            r"C:\Users\me\.claude\hooks\agentnotch-hook.exe statusline",
            r#""C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe" statusline"#,
            "/c/Users/me/.claude/hooks/AgentNotch-Hook.EXE StatusLine",
            r"C:\USERS\ME\.CLAUDE\HOOKS\AGENTNOTCH-HOOK.EXE STATUSLINE",
            // Its 8.3 alias, as the installer writes it for a path with a space.
            "C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE statusline",
            r"c:\users\johnsm~1\.claude\hooks\agentn~12.exe statusline",
            // Behind another command.
            "cd /tmp && C:/x/agentnotch-hook.exe statusline",
        ] {
            assert!(is_own_wrapper(command), "{command}");
            assert_eq!(previous_command(&previous(command)), None, "{command}");
        }
    }

    #[test]
    fn other_commands_are_not_wrappers() {
        for command in [
            "echo agentnotch-hook",
            "C:/x/agentnotch-hook.exe hook",
            "my-statusline --theme dark",
            "C:/tools/AGENTN~1.EXE --version",
            "C:/tools/agentn~.exe statusline",
            "C:/tools/agentn~x.exe statusline",
            "C:/tools/agentn~1.cmd statusline",
            "node C:/tools/sl.js",
        ] {
            assert!(!is_own_wrapper(command), "{command}");
            assert_eq!(
                previous_command(&previous(command)).as_deref(),
                Some(command)
            );
        }
    }
}
