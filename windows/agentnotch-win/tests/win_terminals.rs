//! `WinTerminals` on a real Windows runner (WP6 wp6-11): smoke vectors only. Host classification
//! and focus plans are the engine's and tested there (`control_focus`). Nothing here raises,
//! types into or selects a tab of a window this test did not create: the only window touched is
//! a message-only window made below, which no user can see.
#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use agentnotch_engine::platform::{FocusOutcome, FocusStep, Terminals};
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
