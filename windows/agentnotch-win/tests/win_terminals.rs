//! `WinTerminals` on a real Windows runner (WP6 wp6-11): smoke vectors, and the console helper's
//! runner against stand-in helpers (batch files the tests write: the Windows counterparts of
//! `A3_FocusAndMessagingTests.largeOutputNeverWedgesTheRunner`, `aHangingChildIsKilledAtTheTimeout`
//! and `failuresAreReported`). Host classification and focus plans are the engine's and tested
//! there (`control_focus`). Nothing here raises, types into or selects a tab of a window this
//! test did not create: the only window touched is a message-only window made below, which no
//! user can see; no console is attached to and no editor is started.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentnotch_engine::platform::{ConsoleInfo, FocusOutcome, FocusStep, HostKind, Terminals};
use agentnotch_win::focus::WinTerminals;
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};

/// The console helper's budget (3 s) plus room for a slow runner to start and kill a process.
const HELPER_LIMIT: Duration = Duration::from_secs(5);

fn terminals_with(helper: PathBuf) -> WinTerminals {
    WinTerminals::new(&helper)
}

fn missing_helper() -> PathBuf {
    let folder = tempfile::tempdir().expect("temporary folder");
    // The folder is removed when `folder` drops, so the path stays missing.
    folder.path().join("agentnotch-hook.exe")
}

/// A stand-in console helper: a batch file in `folder` running `lines`. It also writes the
/// arguments it was given to `args.txt` beside itself.
fn batch_helper(folder: &Path, lines: &[&str]) -> PathBuf {
    let path = folder.join("helper.cmd");
    let mut script = String::from("@echo off\r\n>\"%~dp0args.txt\" echo %*\r\n");
    for line in lines {
        script.push_str(line);
        script.push_str("\r\n");
    }
    std::fs::write(&path, script).expect("the stand-in helper");
    path
}

fn system32(exe: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join("System32").join(exe)
}

/// A message-only window of the system's `Static` class: never shown, owned by this test.
struct TestWindow(HWND);

impl TestWindow {
    fn new() -> Self {
        // SAFETY: a system class and a message-only parent; the window is destroyed on drop.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("agentnotch test window"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .expect("a message-only window");
        TestWindow(hwnd)
    }

    fn id(&self) -> u64 {
        self.0 .0 as usize as u64
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created on this thread by `new`.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

#[test]
fn a_missing_helper_means_no_console_with_a_reason() {
    let terminals = terminals_with(missing_helper());
    let info = terminals.console_info(4242);
    assert!(!info.attached);
    assert!(info.error.is_some());
    assert_eq!(info.window, None);
    assert!(info.processes.is_empty());
}

#[test]
fn a_relative_helper_path_is_never_run() {
    let terminals = terminals_with(PathBuf::from("agentnotch-hook.exe"));
    let info = terminals.console_info(4242);
    assert!(!info.attached);
    assert!(info.error.is_some());
}

#[test]
fn a_helper_that_fails_gives_no_console_within_the_budget() {
    // fake-claude answers only --version and exits 1 on `console-info`.
    let terminals = terminals_with(PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")));
    let started = Instant::now();
    let info = terminals.console_info(4242);
    assert!(started.elapsed() < HELPER_LIMIT, "{:?}", started.elapsed());
    assert!(!info.attached);
    assert!(info.error.is_some());
}

#[test]
fn classifying_with_a_failed_helper_returns() {
    let terminals = terminals_with(missing_helper());
    let table = agentnotch_engine::platform::ProcessTable {
        entries: Vec::new(),
    };
    let host = terminals.classify_host(4242, &table);
    assert_eq!(host.window, None);
}

#[test]
fn raising_no_window_is_not_found() {
    let terminals = terminals_with(missing_helper());
    assert_eq!(
        terminals.run_focus(&FocusStep::RaiseWindow { window: 0 }),
        FocusOutcome::NotFound
    );
}

#[test]
fn activating_a_pid_with_no_window_is_not_found() {
    let terminals = terminals_with(missing_helper());
    assert_eq!(
        terminals.run_focus(&FocusStep::ActivatePid { pid: 0xFFFF_FFF0 }),
        FocusOutcome::NotFound
    );
    assert_eq!(
        terminals.run_focus(&FocusStep::ActivatePid { pid: 0 }),
        FocusOutcome::NotFound
    );
}

#[test]
fn selecting_a_tab_of_a_window_that_isnt_windows_terminal_is_not_found() {
    let terminals = terminals_with(missing_helper());
    let window = TestWindow::new();
    let started = Instant::now();
    let outcome = terminals.run_focus(&FocusStep::SelectWtTab {
        window: window.id(),
        title: "agentnotch test window".into(),
    });
    assert_eq!(outcome, FocusOutcome::NotFound);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(terminals.wt_tab_titles(window.id()), None);
    assert_eq!(terminals.wt_tab_titles(0), None);
}

#[test]
fn window_titles() {
    let terminals = terminals_with(missing_helper());
    assert_eq!(terminals.window_title(0), None);
    let window = TestWindow::new();
    assert_eq!(
        terminals.window_title(window.id()).as_deref(),
        Some("agentnotch test window")
    );
}

#[test]
fn an_editor_that_does_not_exist_is_not_started() {
    let terminals = terminals_with(missing_helper());
    let outcome = terminals.run_focus(&FocusStep::OpenInEditor {
        editor_exe: missing_helper(),
        folder: std::env::temp_dir(),
    });
    assert!(matches!(outcome, FocusOutcome::Failed(_)), "{outcome:?}");
}

#[test]
fn the_foreground_and_visibility_answer() {
    let terminals = terminals_with(missing_helper());
    if let Some(foreground) = terminals.foreground() {
        assert_ne!(foreground.window, 0);
    }
    let _ = terminals.any_terminal_visible();
}

#[test]
fn watching_the_foreground_without_a_listener_returns() {
    let terminals = terminals_with(missing_helper());
    let (sink, receiver) = crossbeam_channel::unbounded();
    drop(receiver);
    terminals.watch_foreground(sink);
}

// ---- The console helper's runner ----

#[test]
fn the_helper_is_asked_about_the_pid_and_its_last_line_is_the_answer() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let helper = batch_helper(
        folder.path(),
        &[
            "echo a line before the answer",
            "echo {\"attached\":true,\"window\":1234,\"title\":\"claude\",\"processes\":[4242,300],\"line_input\":false}",
            "echo.",
        ],
    );
    let info = terminals_with(helper).console_info(4242);
    assert_eq!(
        info,
        ConsoleInfo {
            attached: true,
            window: Some(1234),
            title: Some("claude".into()),
            processes: vec![4242, 300],
            line_input: Some(false),
            elevated_target: false,
            error: None,
        }
    );
    let args = std::fs::read_to_string(folder.path().join("args.txt")).expect("the helper ran");
    assert_eq!(args.trim(), "console-info --pid 4242");
}

#[test]
fn a_helper_with_a_lot_of_output_never_wedges_the_runner() {
    let folder = tempfile::tempdir().expect("temporary folder");
    // About 200 KB, far more than a pipe holds: read while the helper runs, or it would stall.
    let filler = format!("for /l %%i in (1,1,2000) do echo {}", "x".repeat(100));
    let helper = batch_helper(
        folder.path(),
        &[
            &filler,
            "echo {\"attached\":false,\"error\":\"no console here\"}",
        ],
    );
    let started = Instant::now();
    let info = terminals_with(helper).console_info(4242);
    assert!(started.elapsed() < HELPER_LIMIT, "{:?}", started.elapsed());
    // The helper's own answer, not the runner's failure.
    assert!(!info.attached);
    assert_eq!(info.error.as_deref(), Some("no console here"));
}

#[test]
fn a_hanging_helper_is_killed_at_the_timeout() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let helper = batch_helper(folder.path(), &["ping -n 10 127.0.0.1 >nul"]);
    let started = Instant::now();
    let info = terminals_with(helper).console_info(4242);
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(2900), "{took:?}");
    assert!(took < HELPER_LIMIT, "{took:?}");
    assert!(!info.attached);
    assert!(
        info.error.as_deref().is_some_and(|e| e.contains("in time")),
        "{info:?}"
    );
}

#[test]
fn a_helper_that_prints_no_answer_is_no_console_with_a_reason() {
    for lines in [&["echo not json"][..], &["rem nothing at all"][..]] {
        let folder = tempfile::tempdir().expect("temporary folder");
        let helper = batch_helper(folder.path(), lines);
        let info = terminals_with(helper).console_info(4242);
        assert!(!info.attached, "{lines:?}");
        assert!(info.error.is_some(), "{lines:?}");
        assert!(info.processes.is_empty(), "{lines:?}");
    }
    // A helper that exits non-zero, whatever it printed.
    let folder = tempfile::tempdir().expect("temporary folder");
    let helper = batch_helper(
        folder.path(),
        &["echo {\"attached\":true,\"line_input\":false}", "exit /b 3"],
    );
    let info = terminals_with(helper).console_info(4242);
    assert!(!info.attached);
    assert!(
        info.error.as_deref().is_some_and(|e| e.contains('3')),
        "{info:?}"
    );
}

// ---- Classification over the helper's answer ----

#[test]
fn a_helper_that_could_not_run_leaves_the_host_unknown() {
    let terminals = terminals_with(missing_helper());
    let table = agentnotch_engine::platform::ProcessTable::default();
    let host = terminals.classify_host(4242, &table);
    // Not "no console": that would hide the session's terminal for good.
    assert_eq!(host.kind, HostKind::Unknown);
    assert_eq!(host.window, None);
}

#[test]
fn a_helper_that_says_no_console_makes_a_console_less_session() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let helper = batch_helper(folder.path(), &["echo {\"attached\":false}"]);
    let table = agentnotch_engine::platform::ProcessTable::default();
    let host = terminals_with(helper).classify_host(4242, &table);
    assert_eq!(host.kind, HostKind::NoConsole);
    assert_eq!(host.window, None);
}

// ---- Opening a folder in an editor ----

#[test]
fn only_an_editor_of_the_vs_code_family_is_started() {
    let terminals = terminals_with(missing_helper());
    let folder = std::env::temp_dir();
    // A real program that isn't an editor: refused before anything starts.
    let outcome = terminals.run_focus(&FocusStep::OpenInEditor {
        editor_exe: system32("cmd.exe"),
        folder: folder.clone(),
    });
    assert!(
        matches!(&outcome, FocusOutcome::Failed(why) if why.contains("isn't an editor")),
        "{outcome:?}"
    );
    // A script named like the editor's launcher is not the editor either.
    let scripts = tempfile::tempdir().expect("temporary folder");
    let launcher = scripts.path().join("code.cmd");
    std::fs::write(&launcher, "@echo off\r\n").expect("a launcher script");
    let outcome = terminals.run_focus(&FocusStep::OpenInEditor {
        editor_exe: launcher,
        folder: folder.clone(),
    });
    assert!(
        matches!(&outcome, FocusOutcome::Failed(why) if why.contains("isn't an editor")),
        "{outcome:?}"
    );
}

#[test]
fn an_editor_is_handed_only_a_full_folder_path() {
    let terminals = terminals_with(missing_helper());
    let editors = tempfile::tempdir().expect("temporary folder");
    // Named like the editor, but not a program: nothing can start from it.
    let editor = editors.path().join("Code.exe");
    std::fs::write(&editor, b"not a program").expect("a stand-in editor");
    let outcome = terminals.run_focus(&FocusStep::OpenInEditor {
        editor_exe: editor.clone(),
        folder: PathBuf::from("--relative"),
    });
    assert!(
        matches!(&outcome, FocusOutcome::Failed(why) if why.contains("full path")),
        "{outcome:?}"
    );
    // A full path gets as far as starting it, which fails for a file that isn't a program.
    let outcome = terminals.run_focus(&FocusStep::OpenInEditor {
        editor_exe: editor,
        folder: std::env::temp_dir(),
    });
    assert!(
        matches!(&outcome, FocusOutcome::Failed(why) if why.contains("didn't start")),
        "{outcome:?}"
    );
}
