//! The hook exe's manners on Windows (DESIGN-WIN §1.4, §1.8, §7.3), with an app listening:
//! what `fail_open.rs` proves on every system with no app, and what needs real Windows
//! processes.
//!
//! - argv that is not ours, and a panic, send the app nothing;
//! - Claude Code closing its end of stdout before the answer is written is not an error;
//! - `--exec` is accepted;
//! - without `CLAUDE_PID` (Claude Code before 2.1.214) the hook finds Claude Code among its own
//!   ancestors. The trees here are real: `tests/bin/relay.rs`, copied to a temporary folder as
//!   `claude.exe` or `code.exe`, runs the hook itself or through Git Bash or PowerShell. No test
//!   runs a real `claude`.
//!
//! The Mac's `EmbeddedScriptsTests` has no counterpart: nothing is templated into the exe.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use agentnotch_engine::hooks::commands::Subcommand;
use agentnotch_engine::runtime_types::AnswerResult;
use agentnotch_proto::{build_hook_message, PermissionResponse};
use common::{
    assert_silent_success, begin, fixture, hook_command, hook_env, spawn, string_command,
    temp_folder, trace_file, traced, with_hook_env, Finished, Harness, Shell, TempFolder, EXE,
    HARD_TIMEOUT,
};
use serde_json::{json, Value};

/// The stand-in parent (`tests/bin/relay.rs`).
const RELAY: &str = env!("CARGO_BIN_EXE_relay");

/// A span in which something that must not happen would have happened.
const SHORTLY: Duration = Duration::from_millis(300);

/// Never a process: Windows hands out pids in multiples of four.
const ENV_PID: u32 = 4242;

/// How the process that runs the hook reaches it.
#[derive(Clone, Copy, Debug)]
enum Via {
    /// It starts the exe itself: exec form.
    Exec,
    /// It starts a shell with the string command.
    Shell(Shell),
}

/// `parent` (a copy of the relay) running the hook, with everything [`hook_command`] sets except
/// `CLAUDE_PID`: the hook has to find Claude Code itself.
fn tree(parent: &Path, via: Via, pipe: &str, trace: &Path) -> Command {
    let mut command = Command::new(parent);
    match via {
        Via::Exec => {
            command.arg(EXE).args(["hook", "--exec"]);
        }
        Via::Shell(shell) => {
            let line = string_command(Path::new(EXE), Subcommand::Hook)
                .expect("the test exe's path can be written into a string command");
            command.arg(shell.program()).args(shell.args(&line));
        }
    }
    let mut command = with_hook_env(command, pipe);
    command
        .env_remove("CLAUDE_PID")
        .env("AGENTNOTCH_HOOK_TRACE", trace);
    command
}

/// One run of a tree: how the parent ended, the message the app received, and the hook's trace.
struct Seen {
    parent: Finished,
    message: Value,
    lines: Vec<String>,
}

fn run_tree(app: &mut Harness, command: Command, trace: &Path, what: &str) -> Seen {
    let parent = spawn(command, &fixture("stdin/pre_tool_use.json")).finish();
    assert_silent_success(&parent, what);
    let (frame, _) = app.wait_frame();
    let message: Value = serde_json::from_slice(&frame.bytes).expect("a JSON message");
    assert_eq!(message["event"], json!("PreToolUse"), "{what}");
    let hook_pid = message["hook_pid"].as_u64().expect("the hook's own pid") as u32;
    assert_ne!(
        hook_pid, parent.pid,
        "{what}: the hook is a process of its own"
    );
    let lines = traced(trace, hook_pid);
    Seen {
        parent,
        message,
        lines,
    }
}

fn stand_in(folder: &TempFolder, image: &str) -> std::path::PathBuf {
    folder.copy_exe(RELAY, image)
}

// ---- argv and panics, with an app listening ----

#[test]
fn argv_that_is_not_ours_sends_the_app_nothing() {
    let _test = begin("argv_that_is_not_ours_sends_the_app_nothing");
    let mut app = Harness::start("argv");
    let stdin = fixture("stdin/permission_request_bash.json");
    let forms: [&[&str]; 6] = [
        &[],
        &["frobnicate"],
        &["--exec"],
        &["hook", "--exec", "extra"],
        &["hook", "--exec", "--exec"],
        &["HOOK"],
    ];
    for args in forms {
        let trace = trace_file();
        let mut command = with_hook_env(Command::new(EXE), &app.pipe);
        command.args(args).env("AGENTNOTCH_HOOK_TRACE", &trace);
        let done = spawn(command, &stdin).finish();
        assert_silent_success(&done, &format!("{args:?}"));
        assert_eq!(traced(&trace, done.pid), ["invoked unknown"], "{args:?}");
    }
    assert!(app.frames_during(SHORTLY).is_empty());
    assert_eq!(app.ingress.held_count(), 0);
}

/// The switch exists in debug builds only, and tests build the exe in their own profile.
#[cfg(debug_assertions)]
#[test]
fn a_panic_is_exit_zero_and_sends_the_app_nothing() {
    let _test = begin("a_panic_is_exit_zero_and_sends_the_app_nothing");
    let mut app = Harness::start("panic");
    for (stdin, exec) in [
        ("stdin/pre_tool_use.json", false),
        ("stdin/permission_request_bash.json", false),
        ("stdin/permission_request_bash.json", true),
    ] {
        let trace = trace_file();
        let mut command = hook_command(&app.pipe);
        if exec {
            command.arg("--exec");
        }
        command
            .env("AGENTNOTCH_HOOK_TEST_PANIC", "1")
            .env("AGENTNOTCH_HOOK_TRACE", &trace);
        let done = spawn(command, &fixture(stdin)).finish();
        // No exit code 101, no message on stderr, and no error dialog holding the process.
        assert_silent_success(&done, stdin);
        assert_eq!(
            traced(&trace, done.pid),
            [format!("invoked hook exec={exec}")],
            "{stdin}: the run ended where it panicked"
        );
    }
    assert!(app.frames_during(SHORTLY).is_empty());
}

// ---- stdout closed under the hook ----

/// Claude Code stops reading a hook whose dialog was answered in the terminal first. The answer
/// the app gives afterwards has nowhere to go: the write fails, and that is not an error.
#[test]
fn an_answer_written_to_a_closed_stdout_is_exit_zero() {
    let _test = begin("an_answer_written_to_a_closed_stdout_is_exit_zero");
    let mut app = Harness::start("closed-stdout");
    let trace = trace_file();
    let mut command = hook_command(&app.pipe);
    command
        .env("AGENTNOTCH_HOOK_TRACE", &trace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the hook exe starts");
    let pid = child.id();
    child
        .stdin
        .take()
        .expect("a piped stdin")
        .write_all(&fixture("stdin/permission_request_bash.json"))
        .expect("the hook reads its stdin");

    let held = app.wait_held();
    // The read end goes away while the hook waits; its own end stays open until it writes.
    drop(child.stdout.take());
    assert_eq!(
        app.ingress.answer(
            &held.session_id,
            &held.tool_use_id,
            PermissionResponse::allow()
        ),
        AnswerResult::Delivered
    );

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the hook exe can be waited for") {
            break status;
        }
        if started.elapsed() > HARD_TIMEOUT {
            let _ = child.kill();
            panic!("the hook exe hung on a closed stdout");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(status.code(), Some(0), "a panic would be 101");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("a piped stderr")
        .read_to_end(&mut stderr)
        .expect("stderr can be read");
    assert!(
        stderr.is_empty(),
        "stderr {:?}",
        String::from_utf8_lossy(&stderr)
    );
    // It got the decision and tried to print it.
    let lines = traced(&trace, pid);
    assert_eq!(
        lines.last().map(String::as_str),
        Some("answered"),
        "{lines:?}"
    );
    assert_eq!(app.ingress.held_count(), 0);
}

// ---- --exec ----

#[test]
fn exec_form_is_accepted_and_sends_the_same_message() {
    let _test = begin("exec_form_is_accepted_and_sends_the_same_message");
    let mut app = Harness::start("exec");
    let stdin = fixture("stdin/pre_tool_use.json");
    let trace = trace_file();
    let mut command = hook_command(&app.pipe);
    command.arg("--exec").env("AGENTNOTCH_HOOK_TRACE", &trace);
    let done = spawn(command, &stdin).finish();
    assert_silent_success(&done, "hook --exec");

    let (frame, _) = app.wait_frame();
    let event: Value = serde_json::from_slice(&stdin).unwrap();
    let expected = build_hook_message(&event, &hook_env(done.pid)).expect("an event object");
    assert_eq!(
        serde_json::from_slice::<Value>(&frame.bytes).unwrap(),
        expected
    );
    let lines = traced(&trace, done.pid);
    assert_eq!(lines[0], "invoked hook exec=true", "{lines:?}");
    assert_eq!(lines[1], "env claude_pid=set config_dir=set", "{lines:?}");
    assert!(lines[2].starts_with("sent PreToolUse "), "{lines:?}");
}

// ---- which process is Claude Code ----

/// Exec form: whoever started the exe is Claude Code, whatever it is called. String form started
/// by something that is neither a shell nor Claude Code: unknown.
#[test]
fn without_claude_pid_exec_form_trusts_its_parent_and_string_form_does_not() {
    let _test = begin("without_claude_pid_exec_form_trusts_its_parent_and_string_form_does_not");
    let mut app = Harness::start("parent");
    let stdin = fixture("stdin/pre_tool_use.json");
    for (exec, pid) in [(true, json!(std::process::id())), (false, Value::Null)] {
        let trace = trace_file();
        let mut command = hook_command(&app.pipe);
        if exec {
            command.arg("--exec");
        }
        command
            .env_remove("CLAUDE_PID")
            .env("AGENTNOTCH_HOOK_TRACE", &trace);
        let done = spawn(command, &stdin).finish();
        assert_silent_success(&done, "hook");
        let (frame, _) = app.wait_frame();
        let message: Value = serde_json::from_slice(&frame.bytes).unwrap();
        assert_eq!(message["pid"], pid, "exec={exec}");
        assert_eq!(message["hook_pid"], json!(done.pid));
        let lines = traced(&trace, done.pid);
        assert_eq!(lines[1], "env claude_pid=unset config_dir=set", "{lines:?}");
    }
}

#[test]
fn a_hook_started_by_claude_itself_reports_that_process() {
    let _test = begin("a_hook_started_by_claude_itself_reports_that_process");
    let mut app = Harness::start("tree-exec");
    let folder = temp_folder("tree-exec");
    let claude = stand_in(&folder, "claude.exe");
    let trace = trace_file();

    let command = tree(&claude, Via::Exec, &app.pipe, &trace);
    let seen = run_tree(&mut app, command, &trace, "claude.exe > hook --exec");
    assert_eq!(seen.message["pid"], json!(seen.parent.pid));
    assert_eq!(seen.lines[0], "invoked hook exec=true", "{:?}", seen.lines);
    assert_eq!(
        seen.lines[1], "env claude_pid=unset config_dir=set",
        "{:?}",
        seen.lines
    );
    let sent = seen.lines.last().expect("a trace");
    assert!(
        sent.starts_with("sent PreToolUse ")
            && sent.ends_with(&format!(" pid={}", seen.parent.pid)),
        "{sent}"
    );
}

/// String form: the hook's parent is the shell (Git Bash's `bin\bash.exe` starts the real
/// `usr\bin\bash.exe`, so there are two), and the first ancestor that is not a shell is Claude
/// Code.
#[test]
fn a_hook_behind_git_bash_reports_claude() {
    let _test = begin("a_hook_behind_git_bash_reports_claude");
    behind_a_shell_the_hook_reports_claude(Shell::GitBash);
}

#[test]
fn a_hook_behind_powershell_reports_claude() {
    let _test = begin("a_hook_behind_powershell_reports_claude");
    behind_a_shell_the_hook_reports_claude(Shell::PowerShell);
}

fn behind_a_shell_the_hook_reports_claude(shell: Shell) {
    let mut app = Harness::start("tree-shell");
    let folder = temp_folder("tree-shell");
    let claude = stand_in(&folder, "claude.exe");
    let trace = trace_file();

    let command = tree(&claude, Via::Shell(shell), &app.pipe, &trace);
    let seen = run_tree(
        &mut app,
        command,
        &trace,
        &format!("claude.exe > {shell:?} > hook"),
    );
    assert_eq!(seen.message["pid"], json!(seen.parent.pid), "{shell:?}");
    assert_eq!(seen.lines[0], "invoked hook exec=false", "{:?}", seen.lines);
    let sent = seen.lines.last().expect("a trace");
    assert!(
        sent.ends_with(&format!(" pid={}", seen.parent.pid)),
        "{shell:?}: {sent}"
    );
}

/// A shell started by anything else (VS Code's own task runner, a terminal) says nothing about
/// where Claude Code is: the pid is left out rather than guessed.
#[test]
fn a_hook_behind_a_shell_of_another_program_reports_no_pid() {
    let _test = begin("a_hook_behind_a_shell_of_another_program_reports_no_pid");
    let mut app = Harness::start("tree-other");
    let folder = temp_folder("tree-other");
    let code = stand_in(&folder, "code.exe");
    let trace = trace_file();

    let command = tree(&code, Via::Shell(Shell::GitBash), &app.pipe, &trace);
    let seen = run_tree(&mut app, command, &trace, "code.exe > bash > hook");
    assert_eq!(seen.message["pid"], Value::Null);
    let sent = seen.lines.last().expect("a trace");
    assert!(
        sent.starts_with("sent PreToolUse ") && sent.ends_with(" pid=none"),
        "{sent}"
    );
}

/// Claude Code 2.1.214 and later say which process they are; the ancestors are not consulted.
#[test]
fn claude_pid_in_the_environment_wins_over_the_ancestors() {
    let _test = begin("claude_pid_in_the_environment_wins_over_the_ancestors");
    let mut app = Harness::start("tree-env");
    let folder = temp_folder("tree-env");
    let claude = stand_in(&folder, "claude.exe");

    for via in [Via::Exec, Via::Shell(Shell::GitBash)] {
        let trace = trace_file();
        let mut command = tree(&claude, via, &app.pipe, &trace);
        command.env("CLAUDE_PID", ENV_PID.to_string());
        let seen = run_tree(&mut app, command, &trace, &format!("CLAUDE_PID, {via:?}"));
        assert_ne!(seen.parent.pid, ENV_PID);
        assert_eq!(seen.message["pid"], json!(ENV_PID), "{via:?}");
        assert_eq!(
            seen.lines[1], "env claude_pid=set config_dir=set",
            "{:?}",
            seen.lines
        );
    }
}
