//! The status line wrapper as Claude Code runs it on Windows (DESIGN-WIN §4.3, §7.3): the Mac's
//! StatusLineScriptTests against the real exe, the real pipe server and the runner's Git Bash.
//!
//! - the previous status line command is run through Git Bash with the same stdin, and its
//!   output and exit code pass through; the app is told meanwhile, within the send's 0.3 s, and
//!   an app that never reads costs that once;
//! - a previous command that hangs is ended at the cap with everything it started (the Job
//!   object), and so is one whose wrapper Claude Code kills: the Mac's SIGTERM tests, which
//!   Windows has no handler for;
//! - a wrapper never starts a wrapper: not one named in the saved command, however its path is
//!   written, and not one reached under a name that check cannot see;
//! - a command written for another shell, forced into the saved file, still fails open.
//!
//! The wrapper reads the saved status line from the folder of the exe it runs as, so every test
//! runs a copy in a folder of its own. What holds on every system is in `fail_open.rs` as well.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use agentnotch_engine::runtime_types::IngressOut;
use agentnotch_proto::build_statusline_message;
use agentnotch_proto::limits::STATUS_LINE_SEND_BUDGET_MS;
use agentnotch_proto::statusline::{DEPTH_ENV, PREVIOUS_FILE_NAME};
use common::{
    assert_silent_success, begin, fixture, hook_env, short_path, silent_server, spawn, temp_folder,
    trace_file, trace_lines, traced, unique_pipe, unquoted, with_hook_env, Finished, Harness,
    Shell, TempFolder, CONFIG_DIR, EXE,
};
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

/// The stand-in parent of `hook_hygiene.rs`: a Windows program that runs what it is given.
const RELAY: &str = env!("CARGO_BIN_EXE_relay");

/// Shortens the previous command's cap; debug builds of the exe only.
const TIMEOUT_SWITCH: &str = "AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS";

const SHORTLY: Duration = Duration::from_millis(300);
/// How long a process that was ended may take to leave the process list.
const GONE_WITHIN: Duration = Duration::from_secs(5);

// ---- a wrapper in a folder of its own ----

/// A copy of the exe where the installer puts it, and the status line saved beside it.
struct Wrapper {
    folder: TempFolder,
    exe: PathBuf,
}

impl Wrapper {
    fn new(what: &str) -> Wrapper {
        Wrapper::named(what, "agentnotch-hook.exe")
    }

    /// The copy under another file name.
    fn named(what: &str, file: &str) -> Wrapper {
        let folder = temp_folder(what);
        let exe = folder.copy_exe(EXE, &format!(r"hooks\{file}"));
        Wrapper { folder, exe }
    }

    /// Saves `command` as the status line this wrapper replaced, the way the installer writes it.
    fn previous(&self, command: &str) {
        let saved = json!({"type": "command", "command": command, "padding": 0});
        let path = self.exe.with_file_name(PREVIOUS_FILE_NAME);
        std::fs::write(&path, saved.to_string())
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }

    /// `<copy> statusline` as a session's status line, talking to `pipe` and traced to `trace`.
    fn command(&self, pipe: &str, trace: &Path) -> Command {
        let mut command = with_hook_env(Command::new(&self.exe), pipe);
        command
            .arg("statusline")
            .env_remove(TIMEOUT_SWITCH)
            .env("AGENTNOTCH_HOOK_TRACE", trace);
        command
    }

    /// A file in the wrapper's temporary folder, as a bash command names it.
    fn marker(&self, name: &str) -> (PathBuf, String) {
        let path = self.folder.path().join(name);
        let shown = format!("'{}'", forward(&path));
        (path, shown)
    }
}

fn forward(path: &Path) -> String {
    path.to_str().expect("a Unicode path").replace('\\', "/")
}

fn status() -> Vec<u8> {
    fixture("stdin/status_line.json")
}

/// The frame a wrapper started by `Wrapper::command` sends for `stdin`.
fn expected_frame(stdin: &[u8], wrapper_pid: u32) -> Vec<u8> {
    let status: Value = serde_json::from_slice(stdin).unwrap();
    let message = build_statusline_message(&status, &hook_env(wrapper_pid)).expect("a status");
    serde_json::to_vec(&message).unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---- the process list ----

#[derive(Debug, Clone, PartialEq, Eq)]
struct Process {
    pid: u32,
    parent: u32,
    /// The image's file name, lower-cased.
    name: String,
}

/// Every process running now.
fn processes() -> Vec<Process> {
    // SAFETY: plain arguments; the handle is checked before use and closed after.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert!(
        !snapshot.is_null() && snapshot != INVALID_HANDLE_VALUE,
        "no process snapshot: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: the entry is plain integers and a name buffer; all zeroes is a valid value.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut found = Vec::new();
    // SAFETY: `snapshot` is open and `entry` carries its own size.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        found.push(Process {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..length]).to_lowercase(),
        });
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot is ours and is not used after this.
    unsafe { CloseHandle(snapshot) };
    found
}

/// The shells and `sleep`s running now: what a previous command of these tests is made of.
fn shells_and_sleeps() -> Vec<Process> {
    processes()
        .into_iter()
        .filter(|process| ["bash.exe", "sleep.exe"].contains(&process.name.as_str()))
        .collect()
}

/// The processes of a previous command: the shells and `sleep`s that were not there `before`
/// the wrapper was started. Tests run one at a time, so they are this wrapper's. (They are not
/// found by walking down from the wrapper: a process Git Bash's runtime started through an
/// `exec` names a parent that has already exited.)
fn started_since(before: &[Process]) -> Vec<Process> {
    shells_and_sleeps()
        .into_iter()
        .filter(|process| !before.contains(process))
        .collect()
}

/// Whether every process of `tree` has left the process list.
fn all_gone(tree: &[Process]) -> bool {
    let now = processes();
    !tree.iter().any(|process| now.contains(process))
}

/// Waits for the previous command to be running, with its marker file made and `sleeps` of its
/// `sleep`s started, and returns its processes.
#[track_caller]
fn running_tree(before: &[Process], started: &Path, sleeps: usize) -> Vec<Process> {
    let count =
        |tree: &[Process], name: &str| tree.iter().filter(|process| process.name == name).count();
    let complete = || {
        let tree = started_since(before);
        started.exists() && count(&tree, "bash.exe") >= 1 && count(&tree, "sleep.exe") >= sleeps
    };
    assert!(
        wait_until(Duration::from_secs(20), complete),
        "the previous command is not running: marker {}, processes {:?}",
        started.exists(),
        started_since(before)
    );
    started_since(before)
}

// ---- chaining and forwarding ----

/// StatusLineScriptTests.forwardsToTheAppAndChainsThePreviousCommand.
#[test]
fn forwards_to_the_app_and_chains_the_previous_command() {
    let _test = begin("forwards_to_the_app_and_chains_the_previous_command");
    let mut app = Harness::start("status-line");
    let wrapper = Wrapper::new("chain");
    wrapper.previous("cat; echo TAIL; exit 3");
    let stdin = status();
    let trace = trace_file();

    let done = spawn(wrapper.command(&app.pipe, &trace), &stdin).finish();

    // The previous command saw the same stdin; its output and status pass through.
    let mut expected = stdin.clone();
    expected.extend_from_slice(b"TAIL\n");
    assert_eq!(text(&done.stdout), text(&expected));
    assert_eq!(done.code, Some(3));
    assert!(done.stderr.is_empty(), "{}", text(&done.stderr));

    let (frame, outs) = app.wait_frame();
    assert_eq!(text(&frame.bytes), text(&expected_frame(&stdin, done.pid)));
    let message: Value = serde_json::from_slice(&frame.bytes).expect("a JSON message");
    let sent: Value = serde_json::from_slice(&stdin).unwrap();
    assert_eq!(message["event"], json!("StatusLine"));
    assert_eq!(message["session_id"], sent["session_id"]);
    assert_eq!(message["transcript_path"], sent["transcript_path"]);
    assert_eq!(message["cwd"], sent["cwd"]);
    assert_eq!(message["config_dir_env"], json!(CONFIG_DIR));
    // The Claude Code process, whose rate limits these are.
    assert_eq!(message["pid"], json!(std::process::id()));
    let mut keys: Vec<&str> = message["status_line"]
        .as_object()
        .expect("a status_line object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "context_window",
            "cost",
            "model",
            "rate_limits",
            "session_name",
            "version"
        ]
    );
    // rate_limits reach the app exactly as Claude Code sent them.
    assert_eq!(message["status_line"]["rate_limits"], sent["rate_limits"]);

    // And the engine reads it as a status line of that session and process.
    let Some(IngressOut::StatusLine(read)) = outs.first() else {
        panic!("not a status line: {outs:?}");
    };
    assert_eq!(Some(read.session_id.as_str()), sent["session_id"].as_str());
    assert_eq!(read.pid, Some(std::process::id()));
    assert_eq!(read.config_dir_env.as_deref(), Some(CONFIG_DIR));
    assert_eq!(read.rate_limits.as_ref(), Some(&sent["rate_limits"]));
    assert_eq!(read.context_used_percent, Some(37.0));
    assert_eq!(read.context_window_size, Some(200_000));
    assert_eq!(read.cost_usd, Some(1.25));
    assert_eq!(read.session_name.as_deref(), Some("Fix login"));
    assert_eq!(read.claude_code_version.as_deref(), Some("2.1.282"));

    let lines = traced(&trace, done.pid);
    assert_eq!(lines[0], "invoked statusline", "{lines:?}");
    assert_eq!(lines[1], "chain: started", "{lines:?}");
    assert!(
        lines.contains(&format!(
            "sent StatusLine {} bytes pid={}",
            frame.bytes.len(),
            std::process::id()
        )),
        "{lines:?}"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some("chain: exit 3"),
        "{lines:?}"
    );
}

/// StatusLineScriptTests.withoutAPreviousCommandPrintsNothing: nothing saved, and `{}`.
#[test]
fn without_a_previous_command_prints_nothing_and_still_forwards() {
    let _test = begin("without_a_previous_command_prints_nothing_and_still_forwards");
    let mut app = Harness::start("no-previous");
    let wrapper = Wrapper::new("no-previous");
    let stdin = status();
    for saved in [None, Some("{}")] {
        if let Some(saved) = saved {
            std::fs::write(wrapper.exe.with_file_name(PREVIOUS_FILE_NAME), saved).unwrap();
        }
        let trace = trace_file();
        let done = spawn(wrapper.command(&app.pipe, &trace), &stdin).finish();
        assert_silent_success(&done, &format!("{saved:?}"));
        let (frame, _) = app.wait_frame();
        assert_eq!(text(&frame.bytes), text(&expected_frame(&stdin, done.pid)));
        assert_eq!(traced(&trace, done.pid)[1], "chain: nothing", "{saved:?}");
    }
}

/// StatusLineScriptTests.appNotRunningStillChains.
#[test]
fn app_not_running_still_chains() {
    let _test = begin("app_not_running_still_chains");
    let wrapper = Wrapper::new("no-app");
    wrapper.previous("echo from-previous");
    let trace = trace_file();
    let done = spawn(wrapper.command(&unique_pipe("no-app"), &trace), &status()).finish();
    assert_eq!(text(&done.stdout), "from-previous\n");
    assert_eq!(done.code, Some(0));
    assert!(done.stderr.is_empty(), "{}", text(&done.stderr));
    assert_eq!(
        traced(&trace, done.pid),
        [
            "invoked statusline",
            "chain: started",
            "env claude_pid=set config_dir=set",
            "no app",
            "chain: exit 0"
        ]
    );
}

/// StatusLineScriptTests.garbageInputIsNotForwardedButStillChained.
#[test]
fn garbage_input_is_not_forwarded_but_still_chained() {
    let _test = begin("garbage_input_is_not_forwarded_but_still_chained");
    let mut app = Harness::start("garbage");
    let wrapper = Wrapper::new("garbage");
    wrapper.previous("wc -c | tr -d ' '");
    let trace = trace_file();
    let done = spawn(wrapper.command(&app.pipe, &trace), b"not json").finish();
    assert_eq!(text(&done.stdout), "8\n");
    assert_eq!(done.code, Some(0));
    assert!(app.frames_during(Duration::from_secs(1)).is_empty());
    assert_eq!(
        traced(&trace, done.pid),
        [
            "invoked statusline",
            "chain: started",
            "stdin: not JSON",
            "chain: exit 0"
        ]
    );
}

/// What the wrapper writes on bash.exe's command line reaches bash as the command was saved:
/// quotes, runs of blanks, backslashes, and a command that begins with a drive letter, which
/// Git Bash's runtime would otherwise read by another rule.
#[test]
fn quotes_and_backslashes_reach_bash_as_written() {
    let _test = begin("quotes_and_backslashes_reach_bash_as_written");
    let wrapper = Wrapper::new("quoting");
    let nobody = unique_pipe("quoting");

    wrapper.previous(r#"printf '%s\n' 'a\\b' "c  d" 'e"f' "g\\h" '\'"#);
    let done = spawn(wrapper.command(&nobody, &trace_file()), &status()).finish();
    assert_eq!(text(&done.stdout), "a\\\\b\nc  d\ne\"f\ng\\h\n\\\n");
    assert_eq!(done.code, Some(0));
    assert!(done.stderr.is_empty(), "{}", text(&done.stderr));

    // A Windows program named by its path, with arguments in double quotes: the relay runs
    // Git's own echo (whose path holds a space) with an argument that holds two.
    let relay = forward(&wrapper.folder.copy_exe(RELAY, "relay.exe"));
    let bash = Shell::GitBash.program();
    let git = bash
        .parent()
        .and_then(Path::parent)
        .expect("Git's folder above bin");
    let echo = git.join(r"usr\bin\echo.exe");
    assert!(echo.is_file(), "Git's echo is not at {}", echo.display());
    let echo = forward(&echo);
    if !unquoted(&relay) {
        eprintln!(
            "statusline: NOTICE: the temporary folder can't be written without quotes ({relay}); \
             the drive-letter command was not run"
        );
        return;
    }
    assert!(relay.as_bytes()[1] == b':', "{relay}");
    wrapper.previous(&format!(r#"{relay} "{echo}" "a  b" c"#));
    let done = spawn(wrapper.command(&nobody, &trace_file()), &status()).finish();
    assert_eq!(text(&done.stdout), "a  b c\n");
    assert_eq!(done.code, Some(0));
}

// ---- the send never holds the status line up ----

/// The previous command is started first and the app is told while it runs: the status arrives
/// within the send's budget although the command has seconds to go.
#[test]
fn the_status_reaches_the_app_while_the_previous_command_still_runs() {
    let _test = begin("the_status_reaches_the_app_while_the_previous_command_still_runs");
    let mut app = Harness::start("forward-first");
    let wrapper = Wrapper::new("forward-first");
    wrapper.previous("sleep 2; echo late");
    let stdin = status();

    // Each run is measured from before the wrapper's process is created, so it holds the
    // runner's process start too: after one unmeasured run, the fastest of three counts.
    let mut fastest = Duration::MAX;
    for run in 0..4 {
        let started = Instant::now();
        let running = spawn(wrapper.command(&app.pipe, &trace_file()), &stdin);
        let (frame, _) = app.wait_frame();
        let forwarded = started.elapsed();
        assert_eq!(
            text(&frame.bytes),
            text(&expected_frame(&stdin, running.pid()))
        );
        // The previous command is nowhere near done.
        running.assert_waiting(SHORTLY);
        let done = running.finish();
        assert_eq!(text(&done.stdout), "late\n");
        assert_eq!(done.code, Some(0));
        assert!(done.elapsed >= Duration::from_secs(2), "{:?}", done.elapsed);
        if run > 0 {
            fastest = fastest.min(forwarded);
        }
    }
    assert!(
        fastest < Duration::from_millis(STATUS_LINE_SEND_BUDGET_MS),
        "the fastest forward took {fastest:?}"
    );
}

/// An app that accepts the connection and never reads: the send's thread stays in its write,
/// the wrapper waits for it for the budget, once, and the status line still comes out.
#[test]
fn an_app_that_never_reads_costs_the_send_budget_once() {
    let _test = begin("an_app_that_never_reads_costs_the_send_budget_once");
    let pipe = unique_pipe("never-reads");
    let server = silent_server(&pipe);
    let wrapper = Wrapper::new("never-reads");
    wrapper.previous("cat >/dev/null; echo shown");
    // More than the pipe's buffer holds, so the write cannot finish; `rate_limits` is
    // forwarded as it came.
    let mut stdin: Value = serde_json::from_slice(&status()).unwrap();
    stdin["rate_limits"]["padding"] = json!("x".repeat(200_000));
    let stdin = serde_json::to_vec(&stdin).unwrap();

    let trace = trace_file();
    let done = spawn(wrapper.command(&pipe, &trace), &stdin).finish();
    assert_eq!(text(&done.stdout), "shown\n");
    assert_eq!(done.code, Some(0));
    assert!(done.stderr.is_empty(), "{}", text(&done.stderr));
    assert_eq!(
        traced(&trace, done.pid),
        [
            "invoked statusline",
            "chain: started",
            "env claude_pid=set config_dir=set",
            "send: still running",
            "chain: exit 0"
        ]
    );
    assert!(
        done.elapsed >= Duration::from_millis(STATUS_LINE_SEND_BUDGET_MS),
        "it ended after {:?}, before the send's budget",
        done.elapsed
    );
    // The budget, a shell's start, and nothing that waits on the app again.
    assert!(
        done.elapsed < Duration::from_secs(5),
        "it took {:?}",
        done.elapsed
    );
    drop(server);
}

// ---- the previous command's processes ----

/// StatusLineScriptTests.aHangingPreviousCommandIsCutOff: past the cap everything the command
/// started is ended and nothing is printed. (A 5 s cap here; the shipped one is 30 s. The switch
/// exists in debug builds only, and tests build the exe in their own profile.)
#[cfg(debug_assertions)]
#[test]
fn a_hanging_previous_commands_tree_is_killed_at_the_timeout() {
    let _test = begin("a_hanging_previous_commands_tree_is_killed_at_the_timeout");
    let wrapper = Wrapper::new("hangs");
    let (started, started_shown) = wrapper.marker("started");
    let (finished, finished_shown) = wrapper.marker("finished");
    // Something printed before it hangs, a process in the background and one in front.
    wrapper.previous(&format!(
        "echo partial; touch {started_shown}; sleep 60 & sleep 60; touch {finished_shown}"
    ));
    let trace = trace_file();
    let mut command = wrapper.command(&unique_pipe("hangs"), &trace);
    command.env(TIMEOUT_SWITCH, "5000");

    let before = shells_and_sleeps();
    let running = spawn(command, &status());
    let tree = running_tree(&before, &started, 2);
    let done = running.finish();

    assert_silent_success(&done, "cut off");
    assert!(
        done.elapsed >= Duration::from_secs(5) && done.elapsed < Duration::from_secs(20),
        "{:?}",
        done.elapsed
    );
    assert_eq!(
        traced(&trace, done.pid).last().map(String::as_str),
        Some("chain: timed out")
    );
    assert!(
        wait_until(GONE_WITHIN, || all_gone(&tree)),
        "left behind: {:?}",
        started_since(&before)
    );
    assert!(!finished.exists());
}

/// StatusLineScriptTests.cancellingStopsThePreviousCommand (and aCancelWhileTheCommandStarts…):
/// Claude Code cancels a run by ending the wrapper's process, which runs no code of ours. The
/// job's last handle closes with it, and that ends the previous command's processes.
#[test]
fn killing_the_wrapper_ends_the_previous_commands_tree() {
    let _test = begin("killing_the_wrapper_ends_the_previous_commands_tree");
    let wrapper = Wrapper::new("cancelled");
    let (started, started_shown) = wrapper.marker("started");
    let (finished, finished_shown) = wrapper.marker("finished");
    wrapper.previous(&format!(
        "touch {started_shown}; sleep 60 & sleep 60; touch {finished_shown}"
    ));
    let trace = trace_file();
    let before = shells_and_sleeps();
    let running = spawn(
        wrapper.command(&unique_pipe("cancelled"), &trace),
        &status(),
    );
    let tree = running_tree(&before, &started, 2);

    running.kill();
    // The command's processes hold the wrapper's stderr: this returns once they are gone, long
    // before they would have finished.
    let done = running.finish();
    assert!(done.stdout.is_empty(), "{}", text(&done.stdout));
    assert!(done.elapsed < Duration::from_secs(30), "{:?}", done.elapsed);
    assert!(
        wait_until(GONE_WITHIN, || all_gone(&tree)),
        "left behind: {:?}",
        started_since(&before)
    );
    assert!(!finished.exists());
    // It was running the command when it was ended, not already past it.
    let lines = traced(&trace, done.pid);
    assert_eq!(lines[1], "chain: started", "{lines:?}");
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("chain: exit") || line == "chain: timed out"),
        "{lines:?}"
    );
}

// ---- a wrapper never starts a wrapper ----

/// StatusLineScriptTests.neverChainsToAWrapper, for the Windows wrapper: a saved command that
/// names it chains nothing, however the path is written.
#[test]
fn a_previous_command_naming_the_wrapper_chains_nothing() {
    let _test = begin("a_previous_command_naming_the_wrapper_chains_nothing");
    let wrapper = Wrapper::new("own-name");
    let long = wrapper.exe.to_str().expect("a Unicode path").to_owned();
    let mut spellings = vec![
        format!("{} statusline", forward(&wrapper.exe)),
        format!("{long} statusline"),
        format!(r#""{long}" statusline"#),
        format!("'{}' statusline", forward(&wrapper.exe)),
        format!("{} STATUSLINE", forward(&wrapper.exe).to_uppercase()),
        format!("{} StatusLine", long.to_lowercase()),
        format!("cd / && {} statusline", forward(&wrapper.exe)),
    ];
    // The 8.3 form, as the installer writes a path that holds a space.
    match short_path(&wrapper.exe) {
        Some(short) if short.to_lowercase().contains("agentn~") => {
            spellings.push(format!("{} statusline", short.replace('\\', "/")));
            spellings.push(format!("{short} statusline"));
        }
        other => eprintln!(
            "statusline: NOTICE: no 8.3 name for the wrapper on this volume ({other:?}); that \
             spelling was not run"
        ),
    }
    for spelling in spellings {
        wrapper.previous(&spelling);
        let trace = trace_file();
        let done = spawn(wrapper.command(&unique_pipe("own-name"), &trace), &status()).finish();
        assert_silent_success(&done, &spelling);
        // One process ran, and it started nothing.
        let lines = trace_lines(&trace);
        assert!(
            lines.iter().all(|(pid, _)| *pid == done.pid),
            "{spelling}: {lines:?}"
        );
        assert_eq!(lines[1].1, "chain: nothing", "{spelling}: {lines:?}");
    }
}

/// The guard for a wrapper the name check cannot see: a wrapper started by a wrapper finds the
/// mark in its environment and chains nothing, so twenty renders start twenty processes and
/// leave none.
#[test]
fn a_wrapper_started_by_a_wrapper_chains_nothing() {
    let _test = begin("a_wrapper_started_by_a_wrapper_chains_nothing");

    // Started with the mark already set: nothing is chained, whatever was saved.
    let wrapper = Wrapper::new("marked");
    wrapper.previous("echo chained");
    let trace = trace_file();
    let mut command = wrapper.command(&unique_pipe("marked"), &trace);
    command.env(DEPTH_ENV, "1");
    let done = spawn(command, &status()).finish();
    assert_silent_success(&done, "started with the mark");
    assert_eq!(traced(&trace, done.pid)[1], "chain: nested");

    // A copy under a name of its own whose saved command is itself.
    let image = format!("an-nested-{}.exe", std::process::id());
    let copy = Wrapper::named("nested", &image);
    copy.previous(&format!("'{}' statusline", forward(&copy.exe)));
    let trace = trace_file();
    let nobody = unique_pipe("nested");
    let mut outer = Vec::new();
    for attempt in 0..20 {
        let done = spawn(copy.command(&nobody, &trace), &status()).finish();
        assert_silent_success(&done, &format!("attempt {attempt}"));
        outer.push(done.pid);
    }
    let lines = trace_lines(&trace);
    let count = |what: &str| lines.iter().filter(|(_, line)| line == what).count();
    assert_eq!(count("invoked statusline"), 40, "{lines:?}");
    assert_eq!(count("chain: started"), 20, "{lines:?}");
    assert_eq!(count("chain: nested"), 20, "{lines:?}");
    assert_eq!(count("chain: exit 0"), 20, "{lines:?}");
    // The ones the test started chained; the ones they started did not.
    for (pid, line) in &lines {
        match line.as_str() {
            "chain: started" | "chain: exit 0" => assert!(outer.contains(pid), "{lines:?}"),
            "chain: nested" => assert!(!outer.contains(pid), "{lines:?}"),
            _ => {}
        }
    }
    // No pile-up: not one copy is still running.
    let running = || -> Vec<Process> {
        processes()
            .into_iter()
            .filter(|process| process.name == image)
            .collect()
    };
    assert!(
        wait_until(GONE_WITHIN, || running().is_empty()),
        "still running: {:?}",
        running()
    );
}

// ---- commands the app would not have wrapped ----

/// The app leaves a status line alone when its command is written for another shell (a `\`, a
/// `.ps1`, `$env:`: the engine's rule). One forced into the saved file anyway still fails open:
/// the run ends, with bash's own verdict or 0, never as a blocking 2.
#[test]
fn a_command_for_another_shell_forced_into_the_file_fails_open() {
    let _test = begin("a_command_for_another_shell_forced_into_the_file_fails_open");
    let wrapper = Wrapper::new("other-shell");
    for command in [
        r"C:\nowhere\status.exe --theme dark",
        r".\no-such-status.ps1",
        "./no-such-status.ps1 -NoLogo",
        "Write-Host $env:USERNAME",
        "echo $env:USERNAME",
    ] {
        wrapper.previous(command);
        let trace = trace_file();
        // `finish` fails the test when the run takes longer than its hard limit.
        let done: Finished = spawn(
            wrapper.command(&unique_pipe("other-shell"), &trace),
            &status(),
        )
        .finish();
        assert!(
            done.code.is_some_and(|code| code != 2),
            "{command}: {:?}",
            done.code
        );
        let lines = traced(&trace, done.pid);
        assert_eq!(lines[1], "chain: started", "{command}: {lines:?}");
        assert_eq!(
            lines.last().cloned(),
            done.code.map(|code| format!("chain: exit {code}")),
            "{command}: {lines:?}"
        );
    }
}
