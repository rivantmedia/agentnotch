//! The hook exe's argv, matched by hand. An argument parser crate would exit
//! 2 on a usage error, and exit 2 from a hook blocks Claude Code; here any
//! unknown subcommand, unknown flag, duplicate or extra argument is
//! [`Invocation::Ignore`]: exit 0 with nothing printed.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;

/// What the exe was asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Invocation {
    /// `hook` (string form) or `hook --exec` (exec form, DESIGN-WIN §4.3).
    Hook { exec_form: bool },
    /// `statusline`: the account's status line wrapper.
    StatusLine,
    /// `type --pid N --started MS [--expect-window HEX|none] [--shells a,b]`.
    Type(TypeArgs),
    /// `console-info --pid N`.
    ConsoleInfo { pid: u32 },
    /// Anything else: exit 0, no output.
    Ignore,
}

/// The two-phase typing helper's arguments (§4.8): the Claude process, its
/// creation time (epoch ms, re-checked after attaching to its console), the
/// console window it must have when one is known, and the direct parent
/// chain of known shells allowed to share the console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeArgs {
    pub pid: u32,
    pub started_ms: u64,
    pub expect_window: Option<u64>,
    pub allowed_shells: Vec<u32>,
}

impl TypeArgs {
    /// The argv after the exe for this request, as the engine spawns it.
    pub fn to_args(&self) -> Vec<String> {
        let window = self
            .expect_window
            .map_or_else(|| "none".to_owned(), |hwnd| format!("{hwnd:x}"));
        let shells = self
            .allowed_shells
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        vec![
            "type".into(),
            "--pid".into(),
            self.pid.to_string(),
            "--started".into(),
            self.started_ms.to_string(),
            "--expect-window".into(),
            window,
            "--shells".into(),
            shells,
        ]
    }
}

/// argv after the exe → what to do.
pub fn parse_invocation(args: &[OsString]) -> Invocation {
    let Some(args) = args
        .iter()
        .map(|a| a.to_str())
        .collect::<Option<Vec<&str>>>()
    else {
        return Invocation::Ignore;
    };
    match args.as_slice() {
        ["hook"] => Invocation::Hook { exec_form: false },
        ["hook", "--exec"] => Invocation::Hook { exec_form: true },
        ["statusline"] => Invocation::StatusLine,
        ["console-info", "--pid", pid] => {
            parse_pid(pid).map_or(Invocation::Ignore, |pid| Invocation::ConsoleInfo { pid })
        }
        ["type", rest @ ..] => parse_type(rest).map_or(Invocation::Ignore, Invocation::Type),
        _ => Invocation::Ignore,
    }
}

fn parse_type(rest: &[&str]) -> Option<TypeArgs> {
    if !rest.len().is_multiple_of(2) {
        return None;
    }
    let (mut pid, mut started, mut window, mut shells) = (None, None, None, None);
    for pair in rest.chunks(2) {
        let value = pair[1];
        let slot_was_empty = match pair[0] {
            "--pid" => pid.replace(parse_pid(value)?).is_none(),
            "--started" => started.replace(parse_u64(value)?).is_none(),
            "--expect-window" => window.replace(parse_window(value)?).is_none(),
            "--shells" => shells.replace(parse_shells(value)?).is_none(),
            _ => return None,
        };
        if !slot_was_empty {
            return None;
        }
    }
    Some(TypeArgs {
        pid: pid?,
        started_ms: started?,
        expect_window: window.flatten(),
        allowed_shells: shells.unwrap_or_default(),
    })
}

fn parse_u64(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_pid(text: &str) -> Option<u32> {
    parse_u64(text)
        .filter(|pid| (1..=i32::MAX as u64).contains(pid))
        .map(|pid| pid as u32)
}

/// `none`, or a window handle in hex (optionally `0x`-prefixed).
fn parse_window(text: &str) -> Option<Option<u64>> {
    if text == "none" {
        return Some(None);
    }
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(digits, 16).ok().map(Some)
}

/// A comma-separated pid list; empty means none.
fn parse_shells(text: &str) -> Option<Vec<u32>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    text.split(',').map(parse_pid).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Invocation {
        parse_invocation(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn hook_forms() {
        assert_eq!(parse(&["hook"]), Invocation::Hook { exec_form: false });
        assert_eq!(
            parse(&["hook", "--exec"]),
            Invocation::Hook { exec_form: true }
        );
        assert_eq!(parse(&["statusline"]), Invocation::StatusLine);
    }

    #[test]
    fn unknown_or_extra_is_ignored() {
        for args in [
            &[][..],
            &["Hook"],
            &["hook", "--exec", "--exec"],
            &["hook", "extra"],
            &["hook", "--EXEC"],
            &["statusline", "x"],
            &["--help"],
            &["PreToolUse"],
            &["console-info"],
            &["console-info", "--pid", "0"],
            &["console-info", "--pid", "12", "x"],
        ] {
            assert_eq!(parse(args), Invocation::Ignore, "{args:?}");
        }
    }

    #[test]
    fn console_info() {
        assert_eq!(
            parse(&["console-info", "--pid", "4242"]),
            Invocation::ConsoleInfo { pid: 4242 }
        );
    }

    #[test]
    fn type_round_trip() {
        let args = TypeArgs {
            pid: 77,
            started_ms: 1_790_000_000_123,
            expect_window: Some(0x1a2b),
            allowed_shells: vec![5, 6],
        };
        let argv = args.to_args();
        assert_eq!(
            argv,
            [
                "type",
                "--pid",
                "77",
                "--started",
                "1790000000123",
                "--expect-window",
                "1a2b",
                "--shells",
                "5,6"
            ]
        );
        assert_eq!(
            parse(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
            Invocation::Type(args)
        );
    }

    #[test]
    fn type_defaults_and_refusals() {
        let minimal = parse(&["type", "--started", "5", "--pid", "9"]);
        assert_eq!(
            minimal,
            Invocation::Type(TypeArgs {
                pid: 9,
                started_ms: 5,
                expect_window: None,
                allowed_shells: vec![]
            })
        );
        assert_eq!(
            parse(&[
                "type",
                "--pid",
                "9",
                "--started",
                "5",
                "--expect-window",
                "none",
                "--shells",
                ""
            ]),
            minimal
        );
        for args in [
            &["type"][..],
            &["type", "--pid", "9"],
            &["type", "--pid", "9", "--started"],
            &["type", "--pid", "9", "--pid", "9", "--started", "5"],
            &["type", "--pid", "9", "--started", "5", "--shells", "1,,2"],
            &["type", "--pid", "9", "--started", "-5"],
            &[
                "type",
                "--pid",
                "9",
                "--started",
                "5",
                "--expect-window",
                "zz",
            ],
            &["type", "--pid", "9", "--started", "5", "--text", "hi"],
        ] {
            assert_eq!(parse(args), Invocation::Ignore, "{args:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_is_ignored() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(
            parse_invocation(&[OsString::from_vec(vec![0x68, 0xff])]),
            Invocation::Ignore
        );
    }
}
