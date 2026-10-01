//! Typing a reply into a session's console, and describing that console (DESIGN-WIN §3.2
//! `ConsoleInput`, §4.8; WP1): the app's side of `agentnotch-hook.exe type` and `console-info`.
//!
//! The GUI app never attaches to a console itself (Ctrl+C and close events would reach it), so
//! both are done by the hook exe, started with no window and piped stdio. For a reply the app
//! writes the text as stdin line 1; the helper types it and prints `{"phase":"typed"}`; the app
//! then asks the engine to look at the session again (`recheck`) and writes `submit` only when
//! that agrees, else `abort`, so a prompt that appeared while the text was typed is never
//! confirmed by our Return. The helper's last line says how it ended.
//!
//! Nothing here touches a console, and the conversation with the helper is plain reads and
//! writes, so all of it builds and is tested on every system; only "start with no window" is
//! Windows's own. The helper is ended when it outlives `CONSOLE_HELPER_LIFETIME_MS`.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agentnotch_engine::platform::{ConsoleInfo, ConsoleInput, ConsoleTarget, TypeOutcome};
use agentnotch_proto::limits::CONSOLE_HELPER_LIFETIME_MS;
use agentnotch_proto::{TypeArgs, TypePhase, TypeRequest, TYPE_ABORT, TYPE_SUBMIT};

/// The exe is missing or can't be run.
pub const HELPER_NOT_STARTED: &str = "The console helper couldn't be started";
/// It ended (or closed its output) without its final line.
pub const HELPER_SILENT: &str = "The console helper gave no answer";
/// It was still running at the end of its lifetime.
pub const HELPER_TOO_SLOW: &str = "The console helper took too long and was stopped";
/// A final line with an outcome this build doesn't know.
pub const HELPER_NOT_UNDERSTOOD: &str = "The console helper's answer wasn't understood";

/// What the outcomes say when the helper gave no reason with them.
const REFUSED: &str = "Your reply can't be typed into this terminal";
const TYPED_NOT_SENT: &str = "Your reply is in the terminal, not sent.";
const FAILED: &str = "Your reply couldn't be typed";

/// No line of the helper is anywhere near this long; one that is ends the reading.
const MAX_LINE: usize = 64 * 1024;
/// How often a helper that has answered is looked at until it has exited.
const REAP_POLL: Duration = Duration::from_millis(10);

/// The helper's arguments for `target`. Its start time goes as whole Unix milliseconds, rounded
/// down: the helper compares it with the process's creation time rounded the same way.
pub fn type_args(target: &ConsoleTarget) -> TypeArgs {
    TypeArgs {
        pid: target.claude_pid,
        started_ms: unix_ms(target.claude_started),
        expect_window: target.expected_window,
        allowed_shells: target.allowed_shells.clone(),
    }
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// The helper's final line as the engine's outcome.
pub fn outcome_of(outcome: &str, reason: Option<String>) -> TypeOutcome {
    let reason = |fallback: &str| {
        reason
            .clone()
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or_else(|| fallback.to_owned())
    };
    match outcome {
        TypePhase::DELIVERED => TypeOutcome::Delivered,
        TypePhase::REFUSED => TypeOutcome::Refused(reason(REFUSED)),
        TypePhase::TYPED_NOT_SUBMITTED => TypeOutcome::TypedNotSubmitted(reason(TYPED_NOT_SENT)),
        TypePhase::FAILED => TypeOutcome::Failed(reason(FAILED)),
        _ => TypeOutcome::Failed(HELPER_NOT_UNDERSTOOD.to_owned()),
    }
}

/// The lines of `source`, read on a thread of their own so that waiting for one can time out (a
/// pipe read can't be abandoned). The channel closes when the source ends, fails, or holds a
/// line longer than any the helper writes.
pub fn lines(mut source: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    // When the thread can't start the sender is dropped, which reads as "the helper ended".
    let _ = std::thread::Builder::new()
        .name("an-console-helper".into())
        .spawn(move || {
            let mut pending: Vec<u8> = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                match source.read(&mut buffer) {
                    Ok(read) if read > 0 => {
                        pending.extend_from_slice(&buffer[..read]);
                        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                            let line: Vec<u8> = pending.drain(..=end).collect();
                            let line = String::from_utf8_lossy(&line).into_owned();
                            if sender.send(line).is_err() {
                                return;
                            }
                        }
                        if pending.len() > MAX_LINE {
                            return;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => break,
                }
            }
            // A last line without its newline still counts.
            if !pending.is_empty() {
                let _ = sender.send(String::from_utf8_lossy(&pending).into_owned());
            }
        });
    receiver
}

/// How a conversation with the helper ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub outcome: TypeOutcome,
    /// The deadline passed before the helper's final line: whoever runs it must end it.
    pub overdue: bool,
}

/// One line of a `type` helper; `None` for anything that is not one of its JSON objects.
fn phase(line: &str) -> Option<TypePhase> {
    // An array of strings would read as an outcome by position.
    line.trim_start()
        .starts_with('{')
        .then(|| TypePhase::from_line(line))?
}

/// The two-phase conversation with a `type` helper: `output` is what the helper prints, `input`
/// its stdin.
///
/// `recheck` runs at most once, when the helper says the text is typed, and its answer decides
/// between `submit` and `abort`. `input` is kept open until the helper's final line (the helper
/// reads a closed stdin as `abort`) and closed on return.
pub fn converse(
    output: impl Read + Send + 'static,
    mut input: impl Write,
    text: &str,
    recheck: &mut dyn FnMut() -> bool,
    deadline: Instant,
) -> Conversation {
    let lines = lines(output);
    let request = TypeRequest {
        text: text.to_owned(),
    };
    // A helper that is gone already can't be written to; what it printed before it went, if
    // anything, is still read below.
    if let Ok(line) = serde_json::to_string(&request) {
        let _ = write_line(&mut input, &line);
    }
    let mut answered = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = match lines.recv_timeout(left) {
            Ok(line) => line,
            Err(RecvTimeoutError::Disconnected) => {
                return Conversation {
                    outcome: TypeOutcome::Failed(HELPER_SILENT.to_owned()),
                    overdue: false,
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                return Conversation {
                    outcome: TypeOutcome::Failed(HELPER_TOO_SLOW.to_owned()),
                    overdue: true,
                }
            }
        };
        match phase(&line) {
            Some(TypePhase::Typed) if !answered => {
                answered = true;
                let word = if recheck() { TYPE_SUBMIT } else { TYPE_ABORT };
                // When this can't be written the helper reads the end of its stdin, or
                // nothing: either way Return is not pressed and its final line says so.
                let _ = write_line(&mut input, word);
            }
            Some(TypePhase::Outcome { outcome, reason }) => {
                return Conversation {
                    outcome: outcome_of(&outcome, reason),
                    overdue: false,
                }
            }
            // A second "typed", or a line that is not the helper's: neither decides anything.
            _ => {}
        }
    }
}

fn write_line(input: &mut impl Write, line: &str) -> std::io::Result<()> {
    input.write_all(format!("{line}\n").as_bytes())?;
    input.flush()
}

/// One line of `console-info`: a JSON object. `None` for anything else.
pub fn parse_console_info(line: &str) -> Option<ConsoleInfo> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    // An array would read as the fields by position.
    value.is_object().then_some(())?;
    serde_json::from_value(value).ok()
}

/// "No console is known", with why.
fn no_console(why: &str) -> ConsoleInfo {
    ConsoleInfo {
        error: Some(why.to_owned()),
        ..ConsoleInfo::default()
    }
}

/// The first line of `lines` that is a `console-info` answer, and whether the deadline passed
/// without one.
fn console_info_from(lines: &Receiver<String>, deadline: Instant) -> (ConsoleInfo, bool) {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                if let Some(info) = parse_console_info(&line) {
                    return (info, false);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return (no_console(HELPER_SILENT), false),
            Err(RecvTimeoutError::Timeout) => return (no_console(HELPER_TOO_SLOW), true),
        }
    }
}

/// The console `claude_pid` is attached to, as `hook_exe console-info --pid N` reports it
/// (`Terminals::console_info`). A helper that can't run, says nothing or says something
/// unreadable gives `attached: false` with `error` set. Blocks until the helper answered, at
/// most its lifetime.
pub fn console_info(hook_exe: &Path, claude_pid: u32) -> ConsoleInfo {
    Helper::new(hook_exe).console_info(claude_pid)
}

/// How the helper exe is started.
#[derive(Debug, Clone)]
struct Helper {
    program: PathBuf,
    /// Arguments before the helper's own; none outside tests (which run a script through a
    /// shell in place of the exe).
    leading: Vec<OsString>,
    lifetime: Duration,
}

impl Helper {
    fn new(hook_exe: &Path) -> Helper {
        Helper {
            program: hook_exe.to_path_buf(),
            leading: Vec::new(),
            lifetime: Duration::from_millis(CONSOLE_HELPER_LIFETIME_MS),
        }
    }

    /// The exe with `args`, its output piped and its errors dropped. `None` when it can't be
    /// started.
    fn start(&self, args: &[String], stdin: Stdio) -> Option<Child> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.leading)
            .args(args)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        without_a_window(&mut command);
        command.spawn().ok()
    }

    fn type_text(
        &self,
        target: &ConsoleTarget,
        text: &str,
        recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome {
        let deadline = Instant::now() + self.lifetime;
        let Some(mut child) = self.start(&type_args(target).to_args(), Stdio::piped()) else {
            return TypeOutcome::Failed(HELPER_NOT_STARTED.to_owned());
        };
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            end(&mut child);
            return TypeOutcome::Failed(HELPER_NOT_STARTED.to_owned());
        };
        let conversation = converse(output, input, text, recheck, deadline);
        if conversation.overdue {
            end(&mut child);
        } else {
            reap(&mut child, deadline);
        }
        conversation.outcome
    }

    fn console_info(&self, claude_pid: u32) -> ConsoleInfo {
        let deadline = Instant::now() + self.lifetime;
        let args = [
            "console-info".to_owned(),
            "--pid".to_owned(),
            claude_pid.to_string(),
        ];
        let Some(mut child) = self.start(&args, Stdio::null()) else {
            return no_console(HELPER_NOT_STARTED);
        };
        let Some(output) = child.stdout.take() else {
            end(&mut child);
            return no_console(HELPER_NOT_STARTED);
        };
        let (info, overdue) = console_info_from(&lines(output), deadline);
        if overdue {
            end(&mut child);
        } else {
            reap(&mut child, deadline);
        }
        info
    }
}

/// The helper is a console program: started by a GUI app it would open a console window of its
/// own, and it must not start out attached to anything.
#[cfg(windows)]
fn without_a_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::CREATE_NO_WINDOW;

    command.creation_flags(CREATE_NO_WINDOW.0);
}

#[cfg(not(windows))]
fn without_a_window(_command: &mut Command) {}

/// Ends the helper and collects it.
fn end(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Collects a helper that has given its answer: it exits right after, and one that still runs
/// at `deadline` is ended.
fn reap(child: &mut Child, deadline: Instant) {
    loop {
        match child.try_wait() {
            Ok(None) if Instant::now() < deadline => std::thread::sleep(REAP_POLL),
            Ok(Some(_)) => return,
            _ => return end(child),
        }
    }
}

/// `ConsoleInput` through the installed `agentnotch-hook.exe`.
#[derive(Debug)]
pub struct ConsoleHelper {
    helper: Helper,
}

impl ConsoleHelper {
    /// `helper` is the installed `agentnotch-hook.exe`.
    pub fn new(helper: &Path) -> Self {
        ConsoleHelper {
            helper: Helper::new(helper),
        }
    }
}

impl ConsoleInput for ConsoleHelper {
    fn type_text(
        &self,
        target: &ConsoleTarget,
        text: &str,
        recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome {
        self.helper.type_text(target, text, recheck)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, PipeReader, PipeWriter};
    use std::thread::JoinHandle;

    use super::*;

    const TEXT: &str = "héllo \"wörld\" ✓";
    const TYPED: &str = r#"{"phase":"typed"}"#;
    const DELIVERED: &str = r#"{"outcome":"delivered"}"#;

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(20)
    }

    /// What a fake helper sees and says: the lines of its stdin, and its stdout.
    struct Ends {
        stdin: BufReader<PipeReader>,
        stdout: PipeWriter,
    }

    impl Ends {
        /// The next stdin line without its newline; `None` once the app closed it.
        fn read(&mut self) -> Option<String> {
            let mut line = String::new();
            match self.stdin.read_line(&mut line) {
                Ok(read) if read > 0 => Some(line.trim_end_matches('\n').to_owned()),
                _ => None,
            }
        }

        fn say(&mut self, line: &str) {
            self.stdout
                .write_all(format!("{line}\n").as_bytes())
                .expect("the app reads the helper's output");
        }
    }

    /// Runs `helper` as the other end of a conversation about [`TEXT`]. Returns how the
    /// conversation ended, how often `recheck` ran, and what the helper returned.
    fn with_helper<T: Send + 'static>(
        recheck: bool,
        deadline: Instant,
        helper: impl FnOnce(Ends) -> T + Send + 'static,
    ) -> (Conversation, u32, T) {
        let (stdin, input) = std::io::pipe().expect("a pipe");
        let (output, stdout) = std::io::pipe().expect("a pipe");
        let fake: JoinHandle<T> = std::thread::spawn(move || {
            helper(Ends {
                stdin: BufReader::new(stdin),
                stdout,
            })
        });
        let mut rechecks = 0;
        let conversation = converse(
            output,
            input,
            TEXT,
            &mut || {
                rechecks += 1;
                recheck
            },
            deadline,
        );
        (
            conversation,
            rechecks,
            fake.join().expect("the fake helper"),
        )
    }

    #[test]
    fn typed_then_a_recheck_that_agrees_submits() {
        let (conversation, rechecks, seen) = with_helper(true, soon(), |mut ends| {
            let first = ends.read();
            ends.say(TYPED);
            let second = ends.read();
            ends.say(DELIVERED);
            (first, second, ends.read())
        });
        assert_eq!(
            conversation,
            Conversation {
                outcome: TypeOutcome::Delivered,
                overdue: false
            }
        );
        assert_eq!(rechecks, 1);
        let (first, second, after) = seen;
        let request: TypeRequest =
            serde_json::from_str(&first.expect("line 1")).expect("line 1 is the request");
        assert_eq!(request.text, TEXT);
        assert_eq!(second.as_deref(), Some("submit"));
        // Stdin is closed once the helper has answered, and nothing more was written.
        assert_eq!(after, None);
    }

    #[test]
    fn a_recheck_that_disagrees_aborts() {
        let (conversation, rechecks, second) = with_helper(false, soon(), |mut ends| {
            ends.read();
            ends.say(TYPED);
            let second = ends.read();
            ends.say(r#"{"outcome":"typed_not_submitted","reason":"It's in the terminal."}"#);
            second
        });
        assert_eq!(
            conversation.outcome,
            TypeOutcome::TypedNotSubmitted("It's in the terminal.".into())
        );
        assert!(!conversation.overdue);
        assert_eq!(rechecks, 1);
        assert_eq!(second.as_deref(), Some("abort"));
    }

    #[test]
    fn a_helper_that_ends_without_a_line_failed() {
        let (conversation, rechecks, ()) = with_helper(true, soon(), |mut ends| {
            ends.read();
        });
        assert_eq!(
            conversation,
            Conversation {
                outcome: TypeOutcome::Failed(HELPER_SILENT.into()),
                overdue: false
            }
        );
        assert_eq!(rechecks, 0);

        // Gone before it read anything: line 1 can't be written, and that is the same failure.
        let (conversation, rechecks, ()) = with_helper(true, soon(), drop);
        assert_eq!(
            conversation.outcome,
            TypeOutcome::Failed(HELPER_SILENT.into())
        );
        assert_eq!(rechecks, 0);

        // It said the text is typed and then went: no final line, so no claim about Return.
        let (conversation, rechecks, ()) = with_helper(true, soon(), |mut ends| {
            ends.read();
            ends.say(TYPED);
            ends.read();
        });
        assert_eq!(
            conversation.outcome,
            TypeOutcome::Failed(HELPER_SILENT.into())
        );
        assert_eq!(rechecks, 1);
    }

    #[test]
    fn a_refusal_before_typing_never_asks_the_engine() {
        let (conversation, rechecks, second) = with_helper(true, soon(), |mut ends| {
            ends.read();
            ends.say(r#"{"outcome":"refused","reason":"Another program is reading this console"}"#);
            ends.read()
        });
        assert_eq!(
            conversation.outcome,
            TypeOutcome::Refused("Another program is reading this console".into())
        );
        assert_eq!(rechecks, 0);
        // Neither `submit` nor `abort` was written: stdin just ended.
        assert_eq!(second, None);
    }

    #[test]
    fn lines_that_are_not_the_helpers_are_ignored() {
        let (conversation, rechecks, second) = with_helper(true, soon(), |mut ends| {
            ends.read();
            ends.say("warning: something");
            ends.say("");
            ends.say(r#"{"phase":"warming up"}"#);
            ends.say(r#"["delivered"]"#);
            ends.say(r#"["refused","by position"]"#);
            ends.say(TYPED);
            let second = ends.read();
            // Said twice: the engine is asked once and one word is written.
            ends.say(TYPED);
            ends.say("\u{feff}not json");
            // The final line without its newline still counts.
            ends.stdout
                .write_all(DELIVERED.as_bytes())
                .expect("the app reads");
            drop(ends.stdout);
            (second, ends.stdin.lines().count())
        });
        assert_eq!(conversation.outcome, TypeOutcome::Delivered);
        assert_eq!(rechecks, 1);
        assert_eq!(second, (Some("submit".to_owned()), 0));
    }

    #[test]
    fn a_helper_that_never_answers_is_overdue() {
        let started = Instant::now();
        let (conversation, rechecks, ()) = with_helper(
            true,
            Instant::now() + Duration::from_millis(200),
            |mut ends| {
                ends.read();
                // Holds its stdout open until the app gives up and closes its stdin.
                ends.read();
            },
        );
        assert_eq!(
            conversation,
            Conversation {
                outcome: TypeOutcome::Failed(HELPER_TOO_SLOW.into()),
                overdue: true
            }
        );
        assert_eq!(rechecks, 0);
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn a_line_too_long_to_be_the_helpers_ends_the_reading() {
        let (conversation, _, ()) = with_helper(true, soon(), |mut ends| {
            ends.read();
            let flood = vec![b'x'; 4 * MAX_LINE];
            // The app stops reading part of the way through, so this may fail.
            let _ = ends.stdout.write_all(&flood);
        });
        assert_eq!(
            conversation.outcome,
            TypeOutcome::Failed(HELPER_SILENT.into())
        );
    }

    #[test]
    fn every_final_line_has_an_outcome() {
        assert_eq!(outcome_of("delivered", None), TypeOutcome::Delivered);
        assert_eq!(
            outcome_of("delivered", Some("ignored".into())),
            TypeOutcome::Delivered
        );
        assert_eq!(
            outcome_of(
                "refused",
                Some("The terminal isn't at Claude Code's prompt".into())
            ),
            TypeOutcome::Refused("The terminal isn't at Claude Code's prompt".into())
        );
        assert_eq!(
            outcome_of("typed_not_submitted", Some("not sent".into())),
            TypeOutcome::TypedNotSubmitted("not sent".into())
        );
        assert_eq!(
            outcome_of("failed", Some("The console didn't take the reply".into())),
            TypeOutcome::Failed("The console didn't take the reply".into())
        );
        // No reason, or a blank one: the outcome still says something.
        assert_eq!(
            outcome_of("refused", None),
            TypeOutcome::Refused(REFUSED.into())
        );
        assert_eq!(
            outcome_of("typed_not_submitted", Some("  ".into())),
            TypeOutcome::TypedNotSubmitted(TYPED_NOT_SENT.into())
        );
        assert_eq!(
            outcome_of("failed", None),
            TypeOutcome::Failed(FAILED.into())
        );
        // A word of a later version is never taken for "delivered".
        assert_eq!(
            outcome_of("queued", Some("later".into())),
            TypeOutcome::Failed(HELPER_NOT_UNDERSTOOD.into())
        );
    }

    #[test]
    fn the_target_becomes_the_helpers_arguments() {
        let target = ConsoleTarget {
            claude_pid: 77,
            // 100 ns units, as Windows counts: the fraction of a millisecond is dropped.
            claude_started: UNIX_EPOCH + Duration::from_nanos(1_790_000_000_123_999_900),
            expected_window: Some(0x1a2b),
            allowed_shells: vec![5, 6],
        };
        assert_eq!(
            type_args(&target).to_args(),
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
        let bare = ConsoleTarget {
            claude_started: UNIX_EPOCH - Duration::from_secs(1),
            expected_window: None,
            allowed_shells: Vec::new(),
            ..target
        };
        assert_eq!(
            type_args(&bare).to_args()[3..],
            ["--started", "0", "--expect-window", "none", "--shells", ""]
        );
    }

    #[test]
    fn console_info_lines() {
        let line = r#"{"attached":true,"window":6699,"title":"claude","processes":[77,5],"line_input":false,"elevated_target":false,"error":null}"#;
        assert_eq!(
            parse_console_info(line),
            Some(ConsoleInfo {
                attached: true,
                window: Some(6699),
                title: Some("claude".into()),
                processes: vec![77, 5],
                line_input: Some(false),
                elevated_target: false,
                error: None,
            })
        );
        assert_eq!(
            parse_console_info(&format!("  {line}\r\n")).map(|info| info.attached),
            Some(true)
        );
        // A later helper's extra field is ignored; a missing one is its default.
        assert_eq!(
            parse_console_info(
                r#"{"attached":false,"elevated_target":true,"error":"Claude runs as administrator","new":1}"#
            ),
            Some(ConsoleInfo {
                elevated_target: true,
                error: Some("Claude runs as administrator".into()),
                ..ConsoleInfo::default()
            })
        );
        for unreadable in [
            "",
            "garbage",
            "[]",
            "[true]",
            "7",
            "null",
            r#"{"attached":"yes"}"#,
            r#"{"attached":true"#,
        ] {
            assert_eq!(parse_console_info(unreadable), None, "{unreadable:?}");
        }
    }

    /// What `console_info_from` makes of a helper that printed `output` and then ended, or is
    /// still running at `deadline`.
    fn info_from(output: &[&str], ended: bool, deadline: Instant) -> (ConsoleInfo, bool) {
        let (sender, lines) = mpsc::channel();
        for line in output {
            sender.send((*line).to_owned()).expect("a channel");
        }
        let _running = (!ended).then(|| sender.clone());
        drop(sender);
        console_info_from(&lines, deadline)
    }

    #[test]
    fn console_info_answers() {
        let answer = r#"{"attached":true,"processes":[9]}"#;
        let (info, overdue) = info_from(&["noise", answer], true, soon());
        assert!(info.attached && info.processes == [9] && info.error.is_none() && !overdue);
        // The answer is taken as soon as it is there, not when the helper has ended.
        let (info, overdue) = info_from(&[answer], false, soon());
        assert!(info.attached && !overdue);

        assert_eq!(
            info_from(&["not an answer"], true, soon()),
            (no_console(HELPER_SILENT), false)
        );
        assert_eq!(
            info_from(&[], true, soon()),
            (no_console(HELPER_SILENT), false)
        );
        let shortly = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            info_from(&["noise"], false, shortly),
            (no_console(HELPER_TOO_SLOW), true)
        );
    }

    /// The whole path with a process in the helper's place: a shell script, since nothing on
    /// this side is Windows's own but the "no window" flag.
    #[cfg(unix)]
    mod with_a_process {
        use std::path::Path;

        use super::super::*;
        use super::TEXT;

        const LONG: Duration = Duration::from_secs(20);

        /// Records what it was given beside itself, and "presses Return" only for `submit`.
        const TYPE_SCRIPT: &str = r#"
printf '%s\n' "$*" > "$0.args"
IFS= read -r first
printf '%s\n' "$first" > "$0.first"
echo '{"phase":"typed"}'
IFS= read -r second || second=EOF
printf '%s\n' "$second" > "$0.second"
if [ "$second" = submit ]; then
  echo '{"outcome":"delivered"}'
else
  echo '{"outcome":"typed_not_submitted","reason":"not sent"}'
fi
"#;

        fn target() -> ConsoleTarget {
            ConsoleTarget {
                claude_pid: 77,
                claude_started: UNIX_EPOCH + Duration::from_millis(1_790_000_000_123),
                expected_window: Some(0x1a2b),
                allowed_shells: vec![5, 6],
            }
        }

        /// `script` run by `/bin/sh` in the exe's place (running the file itself would need it
        /// executable, and a just-written executable can be "text file busy").
        fn script_helper(folder: &Path, script: &str, lifetime: Duration) -> Helper {
            let path = folder.join("helper.sh");
            std::fs::write(&path, script).expect("the script is written");
            Helper {
                program: PathBuf::from("/bin/sh"),
                leading: vec![path.into_os_string()],
                lifetime,
            }
        }

        fn recorded(folder: &Path, what: &str) -> String {
            std::fs::read_to_string(folder.join(format!("helper.sh.{what}")))
                .unwrap_or_else(|error| panic!("{what}: {error}"))
        }

        #[test]
        fn a_reply_is_typed_and_submitted() {
            let folder = tempfile::tempdir().expect("a folder");
            let helper = script_helper(folder.path(), TYPE_SCRIPT, LONG);
            let mut rechecks = 0;
            let outcome = helper.type_text(&target(), TEXT, &mut || {
                rechecks += 1;
                true
            });
            assert_eq!(outcome, TypeOutcome::Delivered);
            assert_eq!(rechecks, 1);
            assert_eq!(
                recorded(folder.path(), "args"),
                "type --pid 77 --started 1790000000123 --expect-window 1a2b --shells 5,6\n"
            );
            let request: TypeRequest =
                serde_json::from_str(&recorded(folder.path(), "first")).expect("the request");
            assert_eq!(request.text, TEXT);
            assert_eq!(recorded(folder.path(), "second"), "submit\n");
        }

        #[test]
        fn a_reply_the_engine_no_longer_wants_sent_is_aborted() {
            let folder = tempfile::tempdir().expect("a folder");
            let helper = script_helper(folder.path(), TYPE_SCRIPT, LONG);
            let outcome = helper.type_text(&target(), TEXT, &mut || false);
            assert_eq!(outcome, TypeOutcome::TypedNotSubmitted("not sent".into()));
            assert_eq!(recorded(folder.path(), "second"), "abort\n");
        }

        #[test]
        fn a_helper_that_outlives_its_lifetime_is_ended() {
            let folder = tempfile::tempdir().expect("a folder");
            let script = "IFS= read -r first\necho started > \"$0.started\"\nexec sleep 30\n";
            let helper = script_helper(folder.path(), script, Duration::from_millis(1500));
            let started = Instant::now();
            let mut rechecks = 0;
            let outcome = helper.type_text(&target(), TEXT, &mut || {
                rechecks += 1;
                true
            });
            assert_eq!(outcome, TypeOutcome::Failed(HELPER_TOO_SLOW.into()));
            assert_eq!(rechecks, 0);
            // It was ended, not waited for.
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "{:?}",
                started.elapsed()
            );
            assert_eq!(recorded(folder.path(), "started"), "started\n");
        }

        #[test]
        fn a_helper_that_cannot_run_or_says_nothing_failed() {
            let folder = tempfile::tempdir().expect("a folder");
            let missing = folder.path().join("agentnotch-hook.exe");
            let mut rechecks = 0;
            let outcome = ConsoleHelper::new(&missing).type_text(&target(), TEXT, &mut || {
                rechecks += 1;
                true
            });
            assert_eq!(outcome, TypeOutcome::Failed(HELPER_NOT_STARTED.into()));
            assert_eq!(rechecks, 0);
            assert_eq!(console_info(&missing, 77), no_console(HELPER_NOT_STARTED));

            // It runs, and exits without a word.
            let silent = script_helper(folder.path(), "exit 0\n", LONG);
            assert_eq!(
                silent.type_text(&target(), TEXT, &mut || true),
                TypeOutcome::Failed(HELPER_SILENT.into())
            );
            assert_eq!(silent.console_info(77), no_console(HELPER_SILENT));
        }

        #[test]
        fn console_info_is_asked_by_pid() {
            let folder = tempfile::tempdir().expect("a folder");
            let script = r#"
printf '%s\n' "$*" > "$0.args"
echo '{"attached":true,"window":6699,"title":"claude","processes":[77],"line_input":false,"elevated_target":false,"error":null}'
"#;
            let info = script_helper(folder.path(), script, LONG).console_info(77);
            assert_eq!(recorded(folder.path(), "args"), "console-info --pid 77\n");
            assert!(info.attached);
            assert_eq!(info.window, Some(6699));
            assert_eq!(info.processes, [77]);
            assert_eq!(info.line_input, Some(false));

            let unreadable = script_helper(folder.path(), "echo 'no console here'\n", LONG);
            assert_eq!(unreadable.console_info(77), no_console(HELPER_SILENT));

            let slow = script_helper(folder.path(), "exec sleep 30\n", Duration::from_millis(300));
            let started = Instant::now();
            assert_eq!(slow.console_info(77), no_console(HELPER_TOO_SLOW));
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "{:?}",
                started.elapsed()
            );
        }
    }
}
