//! What `pipe-squatter.exe` does apart from its Win32 calls: its command line and the record it
//! writes (test-only; the exe is never bundled).
//!
//! The squatter stands in for another local user who created the hook pipe's name before the
//! app did, with a DACL that lets everyone in (DESIGN-WIN §7.3 `win_admin.rs`), or, with
//! `--descriptor app`, for a sandboxed process of the app's own user at Low integrity that gives
//! the pipe exactly the app's owner and DACL (only its label, Low, differs). The hook must
//! connect, see that the pipe is not its user's app's, and leave without writing a byte; the record
//! proves it: one line per connection with the number of bytes received on it (`0` when the
//! client wrote nothing), or `error <why>` when the squatter itself failed (it runs as another
//! user, whose stderr the test can't see).

use std::path::PathBuf;

use super::client::is_pipe_path;
pub use super::script::{EXIT_FAILED, EXIT_OK, EXIT_PIPE_IN_USE, EXIT_USAGE};

/// How long the squatter runs when `--max-seconds` is not given: short, because the test that
/// forgets it runs as another user and may not be able to end it.
pub const DEFAULT_MAX_SECONDS: u64 = 300;

/// The squatter's pipe: generic access for Everyone, owned by whoever created it. Anyone may
/// connect; only the owner check keeps the hook out.
pub const EVERYONE_SDDL: &str = "D:P(A;;GA;;;WD)";

pub const USAGE: &str = "usage: pipe-squatter --pipe <\\\\.\\pipe\\name> --record <file> \
                         --ready <file> [--max-seconds <n>] [--descriptor everyone|app]";

/// The security descriptor the squatter gives its pipe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Descriptor {
    /// [`EVERYONE_SDDL`], owned by whoever runs the squatter: another user's pipe.
    #[default]
    Everyone,
    /// `agentnotch_proto::pipe_sddl` of the user running the squatter: the app's own owner and
    /// DACL, which only the pipe's integrity label can tell apart.
    App,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// `\\.\pipe\<name>`.
    pub pipe: String,
    /// Appended to: one line per connection.
    pub record: PathBuf,
    /// Written (the pipe's name and a newline, by rename) once the first instance exists.
    pub ready: PathBuf,
    /// The squatter exits 0 this long after it started.
    pub max_seconds: u64,
    /// What the pipe's descriptor is.
    pub descriptor: Descriptor,
}

/// Reads the arguments after the program's name. Every flag takes one value and may appear
/// once; anything unknown is an error, as in `pipe-test-server`.
pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut pipe, mut record, mut ready, mut max_seconds) = (None, None, None, None);
    let mut descriptor = None;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let slot_is_free = match flag.as_str() {
            "--pipe" => pipe.is_none(),
            "--record" => record.is_none(),
            "--ready" => ready.is_none(),
            "--max-seconds" => max_seconds.is_none(),
            "--descriptor" => descriptor.is_none(),
            _ => return Err(format!("unknown argument: {flag}")),
        };
        if !slot_is_free {
            return Err(format!("{flag} was given twice"));
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?
            .as_str();
        match flag.as_str() {
            "--pipe" => {
                if !is_pipe_path(value) {
                    return Err(format!("--pipe must be \\\\.\\pipe\\<name>, not {value}"));
                }
                pipe = Some(value.to_owned());
            }
            "--record" => record = Some(path(flag, value)?),
            "--ready" => ready = Some(path(flag, value)?),
            "--descriptor" => {
                descriptor = Some(match value {
                    "everyone" => Descriptor::Everyone,
                    "app" => Descriptor::App,
                    _ => return Err(format!("--descriptor must be everyone or app, not {value}")),
                });
            }
            _ => {
                let seconds = value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| *seconds > 0)
                    .ok_or_else(|| {
                        format!("--max-seconds must be a number above 0, not {value}")
                    })?;
                max_seconds = Some(seconds);
            }
        }
    }
    Ok(Args {
        pipe: pipe.ok_or("--pipe is required")?,
        record: record.ok_or("--record is required")?,
        ready: ready.ok_or("--ready is required")?,
        max_seconds: max_seconds.unwrap_or(DEFAULT_MAX_SECONDS),
        descriptor: descriptor.unwrap_or_default(),
    })
}

fn path(flag: &str, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err(format!("{flag} needs a path"));
    }
    Ok(PathBuf::from(value))
}

/// The record's line for one connection that received `bytes`.
pub fn connection_line(bytes: u64) -> String {
    format!("{bytes}\n")
}

/// The record's line for a failure of the squatter itself (one line, whatever `why` holds).
pub fn error_line(why: &str) -> String {
    format!("error {}\n", why.replace(['\r', '\n'], " "))
}

/// One line of a record, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A connection and the bytes it sent.
    Connection(u64),
    /// The squatter failed; why.
    Error(String),
    /// A line neither writer produces.
    Unreadable(String),
}

/// A record's lines (a test reads them); an unfinished last line is left out.
pub fn read_record(text: &str) -> Vec<Entry> {
    let complete = match text.rfind('\n') {
        Some(end) => &text[..end],
        None => return Vec::new(),
    };
    complete
        .split('\n')
        .map(|line| {
            let line = line.trim_end_matches('\r');
            if let Some(why) = line.strip_prefix("error ") {
                Entry::Error(why.to_owned())
            } else if let Ok(bytes) = line.parse::<u64>() {
                Entry::Connection(bytes)
            } else {
                Entry::Unreadable(line.to_owned())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIPE: &str = r"\\.\pipe\agentnotch-test-squat";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn a_full_command_line_is_read() {
        let parsed = parse_args(&args(&[
            "--pipe",
            PIPE,
            "--record",
            r"C:\s\record.txt",
            "--ready",
            r"C:\s\ready.txt",
            "--max-seconds",
            "60",
        ]))
        .expect("valid");
        assert_eq!(
            parsed,
            Args {
                pipe: PIPE.into(),
                record: PathBuf::from(r"C:\s\record.txt"),
                ready: PathBuf::from(r"C:\s\ready.txt"),
                max_seconds: 60,
                descriptor: Descriptor::Everyone,
            }
        );
    }

    #[test]
    fn the_descriptor_is_everyones_unless_asked_for_the_apps() {
        let base = ["--pipe", PIPE, "--record", "x", "--ready", "r"];
        assert_eq!(
            parse_args(&args(&base)).expect("valid").descriptor,
            Descriptor::Everyone
        );
        for (value, expected) in [("everyone", Descriptor::Everyone), ("app", Descriptor::App)] {
            let mut line = base.to_vec();
            line.extend(["--descriptor", value]);
            assert_eq!(parse_args(&args(&line)).expect(value).descriptor, expected);
        }
        let mut line = base.to_vec();
        line.extend(["--descriptor", "App"]);
        let error = parse_args(&args(&line)).expect_err("case matters");
        assert!(error.contains("--descriptor must be"), "{error}");
        let mut twice = base.to_vec();
        twice.extend(["--descriptor", "app", "--descriptor", "app"]);
        assert_eq!(
            parse_args(&args(&twice)).expect_err("twice"),
            "--descriptor was given twice"
        );
    }

    #[test]
    fn the_time_limit_has_a_default() {
        let parsed =
            parse_args(&args(&["--ready", "r", "--record", "x", "--pipe", PIPE])).expect("valid");
        assert_eq!(parsed.max_seconds, DEFAULT_MAX_SECONDS);
    }

    #[test]
    fn wrong_command_lines_are_refused() {
        for (line, why) in [
            (vec!["--record", "x", "--ready", "r"], "--pipe is required"),
            (vec!["--pipe", PIPE, "--ready", "r"], "--record is required"),
            (vec!["--pipe", PIPE, "--record", "x"], "--ready is required"),
            (
                vec!["--pipe", "agentnotch-hook", "--record", "x", "--ready", "r"],
                "--pipe must be",
            ),
            (
                vec!["--pipe", r"\\.\pipe\a\b", "--record", "x", "--ready", "r"],
                "--pipe must be",
            ),
            (
                vec!["--pipe", PIPE, "--record", "", "--ready", "r"],
                "--record needs a path",
            ),
            (
                vec![
                    "--pipe",
                    PIPE,
                    "--record",
                    "x",
                    "--ready",
                    "r",
                    "--max-seconds",
                    "0",
                ],
                "--max-seconds must be",
            ),
            (
                vec![
                    "--pipe",
                    PIPE,
                    "--record",
                    "x",
                    "--ready",
                    "r",
                    "--max-seconds",
                    "soon",
                ],
                "--max-seconds must be",
            ),
            (
                vec!["--pipe", PIPE, "--record", "x", "--ready"],
                "--ready needs a value",
            ),
            (
                vec![
                    "--pipe",
                    PIPE,
                    "--record",
                    "x",
                    "--ready",
                    "r",
                    "--answer",
                    "Bash=allow",
                ],
                "unknown argument: --answer",
            ),
        ] {
            let error = parse_args(&args(&line)).expect_err(why);
            assert!(error.contains(why), "{line:?}: {error}");
        }
    }

    #[test]
    fn a_repeated_flag_is_refused() {
        let error = parse_args(&args(&[
            "--pipe", PIPE, "--record", "x", "--ready", "r", "--pipe", PIPE,
        ]))
        .expect_err("twice");
        assert_eq!(error, "--pipe was given twice");
    }

    #[test]
    fn the_record_reads_back() {
        let mut text = String::new();
        text.push_str(&connection_line(0));
        text.push_str(&connection_line(412));
        text.push_str(&error_line(
            "CreateNamedPipeW: Access is denied.\r\n(os error 5)",
        ));
        text.push_str("junk\n");
        text.push_str("17");
        assert_eq!(
            read_record(&text),
            vec![
                Entry::Connection(0),
                Entry::Connection(412),
                Entry::Error("CreateNamedPipeW: Access is denied.  (os error 5)".into()),
                Entry::Unreadable("junk".into()),
            ]
        );
        assert!(read_record("").is_empty());
        assert!(read_record("3").is_empty());
    }
}
