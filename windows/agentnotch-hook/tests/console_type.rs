//! Typing a reply into a console, for real (DESIGN-WIN §4.8, §7.3): the helper inside the hook
//! exe (`type`, `console-info`) driven by the app's side of it (`agentnotch_win::console`).
//!
//! Nothing is ever typed into a real terminal, and never into the console this test run was
//! started from. What is typed into is `console-reader.exe` (`tests/bin/console-reader.rs`), a
//! stand-in for Claude Code at its prompt, which the tests start in a console of its own: a
//! hidden one (conhost), and a pseudo console (what Windows Terminal and VS Code's terminal
//! are). The reader says what reached it: the characters, and whether Return came.
//!
//! Proven here:
//! - the two phases: the text arrives whole (`héllo wörld ✓`), and Return only after `submit`,
//!   which the app writes only when the engine's fresh look at the session agrees;
//! - `abort`, the end of the helper's stdin and two seconds of silence each leave the text typed
//!   and unsent;
//! - what `console-info` says about the reader's console;
//! - every refusal before a key is written: another window, another start time, a process that
//!   is gone, another program on the console, a parent on the console that is not one of the
//!   session's shells, and a console that reads lines (a shell's prompt, not Claude Code's);
//! - a reply holding a control key (Ctrl+C, Escape, a tab) is not typed at all.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, UNIX_EPOCH};

use agentnotch_engine::platform::{ConsoleInfo, ConsoleInput, ConsoleTarget, TypeOutcome};
use agentnotch_proto::{TypePhase, TypeRequest};
use agentnotch_win::console::{console_info, type_args, ConsoleHelper};
use common::{begin, spawn_in_console, temp_folder, ConsoleHost, Hosted, TempFolder, EXE, READER};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

/// The stand-in parent (`tests/bin/relay.rs`): runs the reader as its child, on its own console.
const RELAY: &str = env!("CARGO_BIN_EXE_relay");

/// Two-byte and three-byte characters: each is one UTF-16 unit, so one key.
const TEXT: &str = "héllo wörld ✓";

/// The helper's reasons, as the chat shows them.
const TYPED_NOT_SENT: &str =
    "Claude asked for something while your reply was typed; it's in the terminal, not sent.";
const NO_CONSOLE: &str = "This session has no console to type into";
const PROCESS_ENDED: &str = "The session's process has ended";
const OTHER_CONSOLE: &str = "The session's terminal can't be confirmed";
const OTHER_PROGRAM: &str = "Another program is reading this console";
const NOT_AT_PROMPT: &str = "The terminal isn't at Claude Code's prompt";
const CONTROL_KEY: &str = "A reply can't hold control keys";

/// How long the reader, the helper or a file of theirs may take to show up.
const WAIT: Duration = Duration::from_secs(15);
/// The reader's own limit. No test waits for it: each ends its reader with Return or by asking.
const READER_LIMIT_MS: &str = "45000";

// ---- the reader ----

/// What the reader received.
#[derive(Debug, PartialEq, Eq)]
struct Got {
    text: String,
    got_return: bool,
}

impl Got {
    fn nothing() -> Got {
        Got {
            text: String::new(),
            got_return: false,
        }
    }

    fn typed_not_sent(text: &str) -> Got {
        Got {
            text: text.to_owned(),
            got_return: false,
        }
    }

    fn sent(text: &str) -> Got {
        Got {
            text: text.to_owned(),
            got_return: true,
        }
    }
}

/// A reader at its prompt, in a console of its own.
struct Reader {
    /// The process the test started: the reader, or the parent that runs it.
    started: Hosted,
    out: PathBuf,
    pid: u32,
    started_ms: u64,
    window: Option<u64>,
    _folder: TempFolder,
}

fn beside(out: &Path, suffix: &str) -> PathBuf {
    let mut name = out.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The JSON a reader wrote at `path`, once it is there.
#[track_caller]
fn wait_for_json(path: &Path, what: &str) -> serde_json::Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let read = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        if let Some(value) = read {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Reader {
    /// A reader in `mode` (`raw`, as Claude Code reads; `cooked`, as a shell's prompt does),
    /// started by the test itself.
    fn start(host: ConsoleHost, mode: &str) -> Reader {
        let reader = Reader::start_with(host, mode, None);
        assert_eq!(
            reader.pid,
            reader.started.pid(),
            "the reader says who it is"
        );
        reader
    }

    /// A raw reader run by `parent`, which shares its console with it.
    fn start_under(host: ConsoleHost, parent: &str) -> Reader {
        let reader = Reader::start_with(host, "raw", Some(parent));
        assert_ne!(
            reader.pid,
            reader.started.pid(),
            "the reader is its parent's child"
        );
        reader
    }

    fn start_with(host: ConsoleHost, mode: &str, parent: Option<&str>) -> Reader {
        let folder = temp_folder("console");
        let out = folder.path().join("reader.json");
        let out_text = out.to_str().expect("a temporary path is Unicode");
        let reader = [READER, out_text, mode, READER_LIMIT_MS];
        let started = match parent {
            Some(parent) => spawn_in_console(host, parent, &reader),
            None => spawn_in_console(host, READER, &reader[1..]),
        };
        let ready = wait_for_json(&beside(&out, ".ready"), "the reader to be at its prompt");
        let pid = ready["pid"].as_u64().expect("the reader's pid") as u32;
        let started_ms = ready["started_ms"]
            .as_u64()
            .expect("the reader's start time");
        Reader {
            started,
            pid,
            started_ms,
            window: ready["window"].as_u64(),
            out,
            _folder: folder,
        }
    }

    /// The session the reader stands in for, as the engine would describe it.
    fn target(&self) -> ConsoleTarget {
        ConsoleTarget {
            claude_pid: self.pid,
            claude_started: UNIX_EPOCH + Duration::from_millis(self.started_ms),
            expected_window: self.window,
            allowed_shells: Vec::new(),
        }
    }

    /// The reader has ended: Return reached it (or it was asked to stop).
    fn finished(&self) -> bool {
        self.out.exists()
    }

    /// What the reader got, once Return ended it.
    #[track_caller]
    fn result(&self) -> Got {
        let result = wait_for_json(&self.out, "the reader's result");
        assert_eq!(result.get("error"), None, "the reader had a console");
        Got {
            text: result["text"].as_str().expect("the text").to_owned(),
            got_return: result["return"].as_bool().expect("whether Return came"),
        }
    }

    /// What the reader got so far: it is asked to stop, takes what is still in its input buffer
    /// and ends. The helper writes its keys before it answers, so after the helper's final line
    /// everything it typed is in there.
    #[track_caller]
    fn stop(&self) -> Got {
        assert!(!self.finished(), "the reader ended by itself: Return came");
        std::fs::write(beside(&self.out, ".stop"), b"").expect("the stop file");
        self.result()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // A reader run by a parent is not the process the test ends; it ends itself on this.
        let _ = std::fs::write(beside(&self.out, ".stop"), b"");
    }
}

// ---- the helper ----

fn helper() -> ConsoleHelper {
    ConsoleHelper::new(Path::new(EXE))
}

/// Types `text` for `target` through the app's side. Returns the outcome and how often the
/// engine was asked whether to press Return.
fn type_reply(target: &ConsoleTarget, text: &str, submit: bool) -> (TypeOutcome, u32) {
    let mut rechecks = 0;
    let outcome = helper().type_text(target, text, &mut || {
        rechecks += 1;
        submit
    });
    (outcome, rechecks)
}

/// Refused with `reason`, before the engine was asked anything.
#[track_caller]
fn assert_refused(target: &ConsoleTarget, reason: &str) {
    assert_eq!(
        type_reply(target, TEXT, true),
        (TypeOutcome::Refused(reason.to_owned()), 0)
    );
}

fn info(pid: u32) -> ConsoleInfo {
    console_info(Path::new(EXE), pid)
}

/// The helper run by the test itself, for what the app's side never does: ending its stdin, or
/// saying nothing.
struct RawHelper {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<(Instant, String)>,
}

impl RawHelper {
    /// `agentnotch-hook.exe type …` for `target`, started as the app starts it, with `text` as
    /// stdin line 1.
    fn type_reply(target: &ConsoleTarget, text: &str) -> RawHelper {
        let mut child = Command::new(EXE)
            .args(type_args(target).to_args())
            .env_remove("AGENTNOTCH_HOOK_TRACE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("the hook exe starts");
        let mut stdin = child.stdin.take().expect("the helper's stdin");
        let stdout = child.stdout.take().expect("the helper's stdout");
        let (tell, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tell.send((Instant::now(), line)).is_err() {
                    break;
                }
            }
        });
        let request = serde_json::to_string(&TypeRequest {
            text: text.to_owned(),
        })
        .expect("the request");
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .expect("the helper reads its stdin");
        RawHelper {
            child,
            stdin: Some(stdin),
            lines,
        }
    }

    /// The helper's next line and when it arrived.
    #[track_caller]
    fn line(&self) -> (Instant, TypePhase) {
        let (at, line) = self
            .lines
            .recv_timeout(WAIT)
            .expect("the helper prints a line");
        let phase = TypePhase::from_line(&line)
            .unwrap_or_else(|| panic!("not a line of the helper: {line:?}"));
        (at, phase)
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The helper printed nothing more and exited 0.
    #[track_caller]
    fn finish(mut self) {
        let deadline = Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the helper's exit") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("the helper did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(0), "the helper's exit code");
        if let Ok((_, more)) = self.lines.recv_timeout(Duration::from_secs(2)) {
            panic!("the helper printed after its final line: {more:?}");
        }
    }
}

fn typed_not_submitted() -> TypePhase {
    TypePhase::Outcome {
        outcome: TypePhase::TYPED_NOT_SUBMITTED.to_owned(),
        reason: Some(TYPED_NOT_SENT.to_owned()),
    }
}

// ---- the two phases ----

fn a_reply_is_delivered(host: ConsoleHost) {
    let reader = Reader::start(host, "raw");
    let mut finished_before_submit = Vec::new();
    let outcome = helper().type_text(&reader.target(), TEXT, &mut || {
        // The text is in the reader's input buffer by now. Return would end the reader, and a
        // Return written along with the text would have done so within this pause.
        std::thread::sleep(Duration::from_millis(400));
        finished_before_submit.push(reader.finished());
        true
    });
    assert_eq!(outcome, TypeOutcome::Delivered);
    assert_eq!(
        finished_before_submit,
        [false],
        "the engine is asked once, and Return is not pressed before it answered"
    );
    assert_eq!(reader.result(), Got::sent(TEXT));
}

#[test]
fn a_reply_reaches_a_reader_in_its_own_console_and_return_follows_submit() {
    let _guard = begin("a_reply_reaches_a_reader_in_its_own_console_and_return_follows_submit");
    a_reply_is_delivered(ConsoleHost::Hidden);
}

#[test]
fn a_reply_reaches_a_reader_in_a_pseudo_console_and_return_follows_submit() {
    let _guard = begin("a_reply_reaches_a_reader_in_a_pseudo_console_and_return_follows_submit");
    a_reply_is_delivered(ConsoleHost::Pseudo);
}

fn an_aborted_reply_stays_typed(host: ConsoleHost) {
    let reader = Reader::start(host, "raw");
    assert_eq!(
        type_reply(&reader.target(), TEXT, false),
        (TypeOutcome::TypedNotSubmitted(TYPED_NOT_SENT.to_owned()), 1)
    );
    assert_eq!(reader.stop(), Got::typed_not_sent(TEXT));
}

#[test]
fn a_reply_the_engine_no_longer_wants_sent_stays_typed_without_return() {
    let _guard = begin("a_reply_the_engine_no_longer_wants_sent_stays_typed_without_return");
    an_aborted_reply_stays_typed(ConsoleHost::Hidden);
}

#[test]
fn an_aborted_reply_stays_typed_without_return_in_a_pseudo_console() {
    let _guard = begin("an_aborted_reply_stays_typed_without_return_in_a_pseudo_console");
    an_aborted_reply_stays_typed(ConsoleHost::Pseudo);
}

#[test]
fn the_end_of_stdin_leaves_the_reply_typed_without_return() {
    let _guard = begin("the_end_of_stdin_leaves_the_reply_typed_without_return");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    let mut raw = RawHelper::type_reply(&reader.target(), TEXT);
    assert_eq!(raw.line().1, TypePhase::Typed);
    // The app went away between the two phases.
    raw.close_stdin();
    assert_eq!(raw.line().1, typed_not_submitted());
    raw.finish();
    assert_eq!(reader.stop(), Got::typed_not_sent(TEXT));
}

#[test]
fn two_seconds_of_silence_leave_the_reply_typed_without_return() {
    let _guard = begin("two_seconds_of_silence_leave_the_reply_typed_without_return");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    let raw = RawHelper::type_reply(&reader.target(), TEXT);
    let (typed_at, typed) = raw.line();
    assert_eq!(typed, TypePhase::Typed);
    // Stdin stays open and nothing is written: the engine did not answer in time.
    let (ended_at, ended) = raw.line();
    assert_eq!(ended, typed_not_submitted());
    let waited = ended_at.duration_since(typed_at);
    // Both times are taken when this process read the line, so a little under two seconds is
    // possible when the first read was late.
    assert!(
        waited >= Duration::from_millis(1800) && waited < Duration::from_secs(8),
        "the helper waited {waited:?} for the second line"
    );
    raw.finish();
    assert_eq!(reader.stop(), Got::typed_not_sent(TEXT));
}

// ---- console-info ----

#[test]
fn console_info_describes_the_readers_console() {
    let _guard = begin("console_info_describes_the_readers_console");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    let described = info(reader.pid);
    assert!(described.attached, "{described:?}");
    assert_eq!(described.error, None);
    assert!(!described.elevated_target);
    // The reader alone: the helper leaves itself out, and the console host is not on the list.
    assert_eq!(described.processes, [reader.pid]);
    assert_eq!(described.line_input, Some(false));
    assert_eq!(described.window, reader.window);
    // Looking types nothing.
    assert_eq!(reader.stop(), Got::nothing());

    // A process that has ended has no console. Its pid can't be another process's yet: the
    // test still holds the process open.
    assert_eq!(reader.started.wait(WAIT), Some(0), "the reader exited");
    let gone = info(reader.pid);
    assert!(!gone.attached, "{gone:?}");
    assert_eq!(gone.error.as_deref(), Some(NO_CONSOLE));
    assert!(gone.processes.is_empty() && gone.window.is_none() && gone.line_input.is_none());
    assert_refused(&reader.target(), NO_CONSOLE);
}

#[test]
fn console_info_says_when_the_console_reads_lines() {
    let _guard = begin("console_info_says_when_the_console_reads_lines");
    let reader = Reader::start(ConsoleHost::Hidden, "cooked");
    let described = info(reader.pid);
    assert!(described.attached, "{described:?}");
    assert_eq!(described.processes, [reader.pid]);
    assert_eq!(described.line_input, Some(true));
    assert_eq!(reader.stop(), Got::nothing());
}

#[test]
fn console_info_describes_a_pseudo_console() {
    let _guard = begin("console_info_describes_a_pseudo_console");
    let reader = Reader::start(ConsoleHost::Pseudo, "raw");
    let described = info(reader.pid);
    assert!(described.attached, "{described:?}");
    assert_eq!(described.error, None);
    assert!(
        described.processes.contains(&reader.pid),
        "{:?}",
        described.processes
    );
    assert_eq!(described.line_input, Some(false));
    // The helper sees the window the reader sees (a pseudo console's is a hidden stand-in).
    assert_eq!(described.window, reader.window);
    assert_eq!(reader.stop(), Got::nothing());
}

// ---- refusals: nothing is typed ----

#[test]
fn another_window_is_refused() {
    let _guard = begin("another_window_is_refused");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    let target = ConsoleTarget {
        // Window handles are multiples of four; this is nobody's, and not the reader's.
        expected_window: Some(reader.window.map_or(0x1234, |window| window + 4)),
        ..reader.target()
    };
    assert_refused(&target, OTHER_CONSOLE);
    assert_eq!(reader.stop(), Got::nothing());
}

#[test]
fn another_start_time_is_refused() {
    let _guard = begin("another_start_time_is_refused");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    // The pid is the reader's, the process the session was adopted with is not: one
    // millisecond either way is another process.
    for started_ms in [reader.started_ms + 1, reader.started_ms - 1] {
        let target = ConsoleTarget {
            claude_started: UNIX_EPOCH + Duration::from_millis(started_ms),
            ..reader.target()
        };
        assert_refused(&target, PROCESS_ENDED);
    }
    assert_eq!(reader.stop(), Got::nothing());
}

/// A process the test started without a console of the test's; ended when dropped.
struct Ended(Child);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn another_program_on_the_console_is_refused() {
    let _guard = begin("another_program_on_the_console_is_refused");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    // A second program joins the reader's console. It is the test's child, not the reader's,
    // and no shell of the session: it could be reading what is typed.
    let other = Ended(
        Command::new(READER)
            .args(["attach", &reader.pid.to_string(), "60000"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("the second program starts"),
    );
    let other_pid = other.0.id();
    let deadline = Instant::now() + WAIT;
    while !info(reader.pid).processes.contains(&other_pid) {
        assert!(
            Instant::now() < deadline,
            "the second program never joined the reader's console"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_refused(&reader.target(), OTHER_PROGRAM);

    // Named as one of the session's shells it is no stranger. What arrives is this reply
    // alone: the refusal typed nothing.
    let target = ConsoleTarget {
        allowed_shells: vec![other_pid],
        ..reader.target()
    };
    assert_eq!(type_reply(&target, "ok", true), (TypeOutcome::Delivered, 1));
    assert_eq!(reader.result(), Got::sent("ok"));
}

#[test]
fn a_parent_on_the_console_that_is_no_shell_of_the_session_is_refused() {
    let _guard = begin("a_parent_on_the_console_that_is_no_shell_of_the_session_is_refused");
    // The console belongs to the reader's parent, which started it there: a program that ran
    // Claude Code and may read the console itself.
    let reader = Reader::start_under(ConsoleHost::Hidden, RELAY);
    let parent = reader.started.pid();
    let mut on_console = info(reader.pid).processes;
    on_console.sort_unstable();
    let mut expected = vec![parent, reader.pid];
    expected.sort_unstable();
    assert_eq!(on_console, expected);
    assert_refused(&reader.target(), OTHER_PROGRAM);

    // The same parent, known as the shell the session was started from, is allowed.
    let target = ConsoleTarget {
        allowed_shells: vec![parent],
        ..reader.target()
    };
    assert_eq!(type_reply(&target, "ok", true), (TypeOutcome::Delivered, 1));
    assert_eq!(reader.result(), Got::sent("ok"));
}

#[test]
fn a_console_that_reads_lines_is_refused() {
    let _guard = begin("a_console_that_reads_lines_is_refused");
    // A shell's prompt reads whole lines; a line typed there would be run as a command.
    let reader = Reader::start(ConsoleHost::Hidden, "cooked");
    assert_refused(&reader.target(), NOT_AT_PROMPT);
    assert_eq!(reader.stop(), Got::nothing());
}

/// A reply holding a key that acts on arrival (Ctrl+C interrupts Claude, Escape cancels it) is
/// never typed, not even its plain part, and the engine is not asked anything. The engine drops
/// such characters itself; this is the helper's own guard should one get through.
#[test]
fn a_reply_with_a_control_key_types_nothing() {
    let _guard = begin("a_reply_with_a_control_key_types_nothing");
    let reader = Reader::start(ConsoleHost::Hidden, "raw");
    for text in ["stop\u{3}", "\u{1b}", "yes\tplease"] {
        assert_eq!(
            type_reply(&reader.target(), text, true),
            (TypeOutcome::Failed(CONTROL_KEY.to_owned()), 0),
            "{text:?}"
        );
    }
    assert_eq!(reader.stop(), Got::nothing());
}
