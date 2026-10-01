//! Whether the user is looking at a session, and whether a terminal is on
//! screen (`control::looking`). Ported from `A3_FocusPrecisionTests`
//! (`theSelectedTabDecidesWhenTheTerminalCanSay` becomes the Windows
//! Terminal title vectors, `otherwiseOnlyASessionAloneInItsAppIsLookedAt`
//! the editor and other-host vectors), plus new vectors for
//! `any_terminal_uncovered` (the Mac's `TerminalVisibilityDetector`
//! 15 % rule on a 5 x 5 grid), `covers_monitor`, `is_full_screen_window`
//! and `sessions_sharing`.
//!
//! Rectangles: `ScreenWindow` is (left, top, right, bottom), right and
//! bottom exclusive; `covers_monitor` takes the same order as tuples.
//! A 5 x 5 grid over a window samples its cell centres, so a 100 x 100
//! window is read at 10, 30, 50, 70 and 90 on each axis, each point
//! worth 4 %.
//!
//! Skipped (no Windows counterpart): the Mac's tty comparison; the
//! console title plays that part on Windows Terminal.

mod control_support;

use agentnotch_engine::control::hosts::is_terminal_process;
use agentnotch_engine::control::looking::{
    any_terminal_uncovered, covers_monitor, is_full_screen_window, looking_at, looking_at_console,
    sessions_sharing, ScreenWindow, MINIMUM_VISIBLE_FRACTION, VIEWED_DWELL,
};
use agentnotch_engine::platform::{ConsoleInfo, Foreground, HostApp, HostKind};
use agentnotch_engine::runtime_types::ReactionContext;
use control_support::*;

const CONSOLE_WINDOW: u64 = 0x50_0A12;
const WT_WINDOW: u64 = 0x70_0001;
const EDITOR_PID: u32 = 9100;
const SESSION_TITLE: &str = "✳ Refactor the parser";

fn foreground(pid: u32, window: u64, title: &str) -> Foreground {
    Foreground {
        pid,
        window,
        title: title.into(),
        fullscreen: false,
    }
}

fn conhost_host() -> HostApp {
    HostApp {
        window: Some(CONSOLE_WINDOW),
        ..host(HostKind::Conhost)
    }
}

fn wt_host() -> HostApp {
    HostApp {
        window: Some(WT_WINDOW),
        host_pid: Some(8800),
        ..host(HostKind::WindowsTerminal)
    }
}

fn editor_host() -> HostApp {
    HostApp {
        window: Some(0x90_0001),
        host_pid: Some(EDITOR_PID),
        ..host(HostKind::VsCode {
            product: "VS Code".into(),
        })
    }
}

fn titled(title: &str) -> ConsoleInfo {
    ConsoleInfo {
        title: Some(title.into()),
        ..console(4242)
    }
}

// ---- Classic console ----

#[test]
fn a_classic_console_is_looked_at_when_its_window_is_in_front() {
    let v = view("s1");
    let h = conhost_host();
    let info = console(4242);
    assert_eq!(info.window, Some(CONSOLE_WINDOW));
    assert_eq!(
        looking_at_console(&v, &h, &info, &foreground(1, CONSOLE_WINDOW, ""), 1, &[]),
        Some(true)
    );
    assert_eq!(
        looking_at_console(&v, &h, &info, &foreground(1, 0x1234, ""), 1, &[]),
        Some(false)
    );
}

#[test]
fn a_classic_console_ignores_how_many_sessions_share_the_machine() {
    // One window is one console: the count only matters for tabbed hosts.
    let v = view("s1");
    let info = console(4242);
    assert_eq!(
        looking_at_console(
            &v,
            &conhost_host(),
            &info,
            &foreground(1, CONSOLE_WINDOW, ""),
            4,
            &[]
        ),
        Some(true)
    );
}

#[test]
fn a_classic_console_falls_back_to_the_host_window_when_detached() {
    let v = view("s1");
    let detached = ConsoleInfo {
        attached: false,
        window: Some(0x6666),
        ..console(4242)
    };
    // The detached console's own window is not trusted; the host's is used.
    assert_eq!(
        looking_at_console(
            &v,
            &conhost_host(),
            &detached,
            &foreground(1, CONSOLE_WINDOW, ""),
            1,
            &[]
        ),
        Some(true)
    );
    assert_eq!(
        looking_at_console(
            &v,
            &conhost_host(),
            &detached,
            &foreground(1, 0x6666, ""),
            1,
            &[]
        ),
        Some(false)
    );
}

#[test]
fn a_classic_console_with_no_window_known_cannot_be_told() {
    let v = view("s1");
    let nameless = ConsoleInfo {
        window: None,
        ..console(4242)
    };
    assert_eq!(
        looking_at_console(
            &v,
            &host(HostKind::Conhost),
            &nameless,
            &foreground(1, CONSOLE_WINDOW, ""),
            1,
            &[]
        ),
        None
    );
    assert_eq!(
        looking_at(
            &v,
            &host(HostKind::Conhost),
            &foreground(1, CONSOLE_WINDOW, ""),
            1
        ),
        None
    );
}

// ---- Windows Terminal ----

#[test]
fn a_windows_terminal_window_holding_one_session_is_looked_at_when_in_front() {
    let v = view("s1");
    let h = wt_host();
    assert_eq!(
        looking_at(&v, &h, &foreground(8800, WT_WINDOW, "anything"), 1),
        Some(true)
    );
    // Another window in front, even of the same Windows Terminal process.
    assert_eq!(
        looking_at(&v, &h, &foreground(8800, WT_WINDOW + 1, "anything"), 1),
        Some(false)
    );
    assert_eq!(
        looking_at(&v, &h, &foreground(77, 0x1234, "Chrome"), 1),
        Some(false)
    );
}

#[test]
fn the_selected_tab_decides_when_the_terminal_can_say() {
    // Two sessions in one window: the window's title is its selected tab's.
    let v = view("s1");
    let h = wt_host();
    let mine = titled(SESSION_TITLE);
    let other = vec!["✳ Fix the build".to_string()];
    assert_eq!(
        looking_at_console(
            &v,
            &h,
            &mine,
            &foreground(8800, WT_WINDOW, SESSION_TITLE),
            2,
            &other
        ),
        Some(true)
    );
    // The other tab is selected.
    assert_eq!(
        looking_at_console(
            &v,
            &h,
            &mine,
            &foreground(8800, WT_WINDOW, "✳ Fix the build"),
            2,
            &other
        ),
        Some(false)
    );
}

#[test]
fn a_title_is_compared_trimmed() {
    let v = view("s1");
    let h = wt_host();
    assert_eq!(
        looking_at_console(
            &v,
            &h,
            &titled(&format!("  {SESSION_TITLE} ")),
            &foreground(8800, WT_WINDOW, &format!("{SESSION_TITLE}  ")),
            2,
            &[]
        ),
        Some(true)
    );
}

#[test]
fn a_title_two_sessions_share_decides_nothing() {
    let v = view("s1");
    let h = wt_host();
    let same = vec![format!(" {SESSION_TITLE}")];
    assert_eq!(
        looking_at_console(
            &v,
            &h,
            &titled(SESSION_TITLE),
            &foreground(8800, WT_WINDOW, SESSION_TITLE),
            2,
            &same
        ),
        Some(false)
    );
}

#[test]
fn several_sessions_with_no_title_to_compare_are_not_looked_at() {
    let v = view("s1");
    let h = wt_host();
    let fg = foreground(8800, WT_WINDOW, SESSION_TITLE);
    // `looking_at` has no console to compare against.
    assert_eq!(looking_at(&v, &h, &fg, 2), Some(false));
    // A blank title, an unknown title and a detached console say nothing.
    for info in [
        titled("   "),
        ConsoleInfo {
            title: None,
            ..console(4242)
        },
        ConsoleInfo {
            attached: false,
            ..titled(SESSION_TITLE)
        },
    ] {
        assert_eq!(
            looking_at_console(&v, &h, &info, &fg, 3, &[]),
            Some(false),
            "{info:?}"
        );
    }
}

#[test]
fn a_blank_foreground_title_never_matches_a_blank_session_title() {
    let v = view("s1");
    assert_eq!(
        looking_at_console(
            &v,
            &wt_host(),
            &titled(""),
            &foreground(8800, WT_WINDOW, ""),
            2,
            &[]
        ),
        Some(false)
    );
}

#[test]
fn a_session_alone_in_a_window_needs_no_title() {
    let v = view("s1");
    let info = ConsoleInfo {
        title: None,
        ..console(4242)
    };
    assert_eq!(
        looking_at_console(
            &v,
            &wt_host(),
            &info,
            &foreground(8800, WT_WINDOW, "whatever"),
            1,
            &[]
        ),
        Some(true)
    );
}

#[test]
fn a_windows_terminal_without_a_known_window_cannot_be_told() {
    let v = view("s1");
    let h = host(HostKind::WindowsTerminal);
    assert_eq!(
        looking_at(&v, &h, &foreground(8800, WT_WINDOW, ""), 1),
        None
    );
}

// ---- Editors and other hosts ----

#[test]
fn only_a_session_alone_in_its_editor_is_looked_at() {
    let v = view("s1");
    let h = editor_host();
    let in_editor = foreground(EDITOR_PID, 0x90_0001, "app - Visual Studio Code");
    assert_eq!(looking_at(&v, &h, &in_editor, 1), Some(true));
    // Four sessions in one VS Code: activating it says nothing about which
    // one the user reads (the Mac's `[11, 10, nil]` case).
    assert_eq!(looking_at(&v, &h, &in_editor, 4), Some(false));
    assert_eq!(looking_at(&v, &h, &in_editor, 2), Some(false));
    // No count at all is alone.
    assert_eq!(looking_at(&v, &h, &in_editor, 0), Some(true));
}

#[test]
fn an_editor_is_not_looked_at_when_something_else_is_in_front() {
    let v = view("s1");
    assert_eq!(
        looking_at(&v, &editor_host(), &foreground(77, 0x1234, "Chrome"), 1),
        Some(false)
    );
}

#[test]
fn an_editor_without_a_pid_is_found_by_its_window() {
    let v = view("s1");
    let h = HostApp {
        host_pid: None,
        ..editor_host()
    };
    assert_eq!(
        looking_at(&v, &h, &foreground(1, 0x90_0001, ""), 1),
        Some(true)
    );
    assert_eq!(
        looking_at(&v, &h, &foreground(1, 0x90_0002, ""), 1),
        Some(false)
    );
    // Neither a pid nor a window: nothing can be in front.
    assert_eq!(
        looking_at(
            &v,
            &host(HostKind::VsCode {
                product: "VS Code".into()
            }),
            &foreground(1, 0x90_0001, ""),
            1
        ),
        Some(false)
    );
}

#[test]
fn jetbrains_and_other_terminals_follow_the_editor_rule() {
    let v = view("s1");
    let in_front = foreground(EDITOR_PID, 0x90_0001, "");
    for kind in [
        HostKind::JetBrains,
        HostKind::OtherConsoleHost {
            exe: "wezterm-gui.exe".into(),
        },
    ] {
        let h = HostApp {
            kind,
            ..editor_host()
        };
        assert_eq!(looking_at(&v, &h, &in_front, 1), Some(true), "{h:?}");
        assert_eq!(looking_at(&v, &h, &in_front, 2), Some(false), "{h:?}");
        assert_eq!(
            looking_at(&v, &h, &foreground(1, 0x1, ""), 1),
            Some(false),
            "{h:?}"
        );
    }
}

// ---- What can't be told ----

#[test]
fn an_unknown_host_cannot_be_told_and_never_counts_as_looking() {
    let v = view("s1");
    let fg = foreground(EDITOR_PID, 0x90_0001, "");
    assert_eq!(looking_at(&v, &host(HostKind::Unknown), &fg, 1), None);
    assert_eq!(
        looking_at_console(&v, &host(HostKind::Unknown), &console(4242), &fg, 1, &[]),
        None
    );
    // The reactions treat "can't tell" as not looking.
    let ctx = ReactionContext {
        looking_at: None,
        ..reaction_ctx()
    };
    assert!(!agentnotch_engine::control::reactions::policy_context(&ctx).terminal_focused);
    let yes = ReactionContext {
        looking_at: Some(true),
        ..reaction_ctx()
    };
    assert!(agentnotch_engine::control::reactions::policy_context(&yes).terminal_focused);
}

#[test]
fn a_session_without_a_console_is_not_looked_at() {
    let v = view("s1");
    assert_eq!(
        looking_at(&v, &host(HostKind::NoConsole), &foreground(1, 2, "x"), 1),
        Some(false)
    );
}

#[test]
fn a_session_without_a_process_has_nothing_to_look_at() {
    let mut v = view("s1");
    v.pid = None;
    assert_eq!(
        looking_at(&v, &conhost_host(), &foreground(1, CONSOLE_WINDOW, ""), 1),
        None
    );
    assert_eq!(
        looking_at(&v, &wt_host(), &foreground(8800, WT_WINDOW, ""), 1),
        None
    );
}

// ---- sessions_sharing ----

#[test]
fn sessions_are_counted_per_window_for_terminals_and_per_app_for_editors() {
    let wt_a = wt_host();
    let wt_b = HostApp {
        window: Some(WT_WINDOW + 1),
        ..wt_host() // the same Windows Terminal process, another window
    };
    let editor = editor_host();
    let editor_two = HostApp {
        window: Some(0x90_0002),
        ..editor_host() // another window of the same editor process
    };
    let conhost = conhost_host();
    let conhost_other = HostApp {
        window: Some(0x50_0A13),
        ..conhost_host()
    };
    let nowhere = host(HostKind::Unknown);
    let all = [
        &wt_a,
        &wt_a,
        &wt_a,
        &wt_b,
        &editor,
        &editor,
        &editor_two,
        &conhost,
        &conhost_other,
        &nowhere,
    ];
    assert_eq!(sessions_sharing(&wt_a, &all), 3);
    assert_eq!(sessions_sharing(&wt_b, &all), 1);
    // The editor is one app: its windows count together.
    assert_eq!(sessions_sharing(&editor, &all), 3);
    assert_eq!(sessions_sharing(&conhost, &all), 1);
    assert_eq!(sessions_sharing(&conhost_other, &all), 1);
}

#[test]
fn a_host_that_does_not_say_where_it_is_shares_with_no_one() {
    let nowhere = host(HostKind::Unknown);
    let blind_terminal = host(HostKind::WindowsTerminal);
    let all = [&nowhere, &nowhere, &blind_terminal];
    assert_eq!(sessions_sharing(&nowhere, &all), 0);
    assert_eq!(sessions_sharing(&blind_terminal, &all), 0);
    assert_eq!(sessions_sharing(&editor_host(), &[]), 0);
}

#[test]
fn the_count_decides_the_editor_vector_end_to_end() {
    let v = view("s1");
    let h = editor_host();
    let fg = foreground(EDITOR_PID, 0x90_0001, "");
    let four = [&h, &h, &h, &h];
    assert_eq!(
        looking_at(&v, &h, &fg, sessions_sharing(&h, &four)),
        Some(false)
    );
    let one = [&h];
    assert_eq!(
        looking_at(&v, &h, &fg, sessions_sharing(&h, &one)),
        Some(true)
    );
}

// ---- any_terminal_uncovered ----

fn win(left: i32, top: i32, right: i32, bottom: i32, is_terminal: bool) -> ScreenWindow {
    ScreenWindow {
        left,
        top,
        right,
        bottom,
        is_terminal,
    }
}

fn uncovered(windows: &[ScreenWindow]) -> bool {
    any_terminal_uncovered(windows, MINIMUM_VISIBLE_FRACTION)
}

#[test]
fn a_lone_terminal_is_visible() {
    assert!(uncovered(&[win(0, 0, 100, 100, true)]));
}

#[test]
fn nothing_on_screen_shows_no_terminal() {
    assert!(!uncovered(&[]));
}

#[test]
fn a_non_terminal_window_alone_is_not_a_terminal() {
    assert!(!uncovered(&[win(0, 0, 800, 600, false)]));
}

#[test]
fn a_terminal_fully_under_a_browser_is_hidden() {
    // Front to back: the browser first, the terminal behind it.
    assert!(!uncovered(&[
        win(0, 0, 200, 200, false),
        win(20, 20, 120, 120, true),
    ]));
    // The same pair the other way round: the terminal is in front.
    assert!(uncovered(&[
        win(20, 20, 120, 120, true),
        win(0, 0, 200, 200, false),
    ]));
}

#[test]
fn a_terminal_twenty_percent_uncovered_is_visible() {
    // The browser covers y < 80, leaving the bottom row of samples (y = 90):
    // 5 of 25 points = 20 %.
    assert!(uncovered(&[
        win(0, 0, 100, 80, false),
        win(0, 0, 100, 100, true),
    ]));
}

#[test]
fn a_terminal_under_fifteen_percent_uncovered_is_hidden() {
    // The bottom row (5 points) minus the two left columns hidden by a
    // second window (40 x 40, so it counts as an occluder): 3 of 25 = 12 %.
    assert!(!uncovered(&[
        win(0, 0, 100, 80, false),
        win(0, 60, 40, 100, false),
        win(0, 0, 100, 100, true),
    ]));
    // Hiding just one column instead (centre x = 10 only) leaves 4 of 25 =
    // 16 %: visible.
    assert!(uncovered(&[
        win(0, 0, 100, 80, false),
        win(-30, 60, 20, 100, false),
        win(0, 0, 100, 100, true),
    ]));
}

#[test]
fn the_fraction_is_the_callers_to_choose() {
    // 5 of 25 points show; a 25 % demand refuses what 15 % accepts.
    let windows = [win(0, 0, 100, 80, false), win(0, 0, 100, 100, true)];
    assert!(any_terminal_uncovered(&windows, 0.2));
    assert!(!any_terminal_uncovered(&windows, 0.25));
    assert_eq!(MINIMUM_VISIBLE_FRACTION, 0.15);
}

#[test]
fn a_terminal_smaller_than_forty_pixels_is_ignored() {
    assert!(!uncovered(&[win(0, 0, 39, 100, true)]));
    assert!(!uncovered(&[win(0, 0, 100, 39, true)]));
    // Forty is enough.
    assert!(uncovered(&[win(0, 0, 40, 40, true)]));
}

#[test]
fn a_window_smaller_than_forty_pixels_hides_nothing() {
    // A tooltip-sized window over the middle of a terminal.
    assert!(uncovered(&[
        win(0, 0, 39, 39, false),
        win(0, 0, 100, 100, true),
    ]));
    // The same window at 40 x 40 covers the first two columns and rows
    // (centres 10 and 30), so 21 of 25 points still show.
    assert!(uncovered(&[
        win(0, 0, 40, 40, false),
        win(0, 0, 100, 100, true),
    ]));
}

#[test]
fn a_hidden_terminal_does_not_stop_the_search() {
    // The first terminal is buried; the second, behind it, is not.
    assert!(uncovered(&[
        win(0, 0, 200, 200, false),
        win(0, 0, 100, 100, true),
        win(300, 0, 400, 100, true),
    ]));
    // Two buried ones stay buried.
    assert!(!uncovered(&[
        win(0, 0, 500, 200, false),
        win(0, 0, 100, 100, true),
        win(300, 0, 400, 100, true),
    ]));
}

#[test]
fn a_terminal_covers_what_lies_behind_it() {
    // A terminal in front of another one hides it, but is itself seen.
    assert!(uncovered(&[
        win(0, 0, 100, 100, true),
        win(0, 0, 100, 100, true),
    ]));
    // A non-terminal exactly over a terminal leaves it no samples.
    assert!(!uncovered(&[
        win(0, 0, 100, 100, false),
        win(0, 0, 100, 100, true),
    ]));
}

#[test]
fn the_terminal_names_the_platform_lists_are_terminals() {
    for exe in [
        "WindowsTerminal.exe",
        "OpenConsole.exe",
        "conhost.exe",
        "Code.exe",
        "cursor.exe",
        "wezterm-gui.exe",
        "idea64.exe",
    ] {
        assert!(is_terminal_process(exe), "{exe}");
    }
    for exe in ["chrome.exe", "explorer.exe", "xcode.exe", ""] {
        assert!(!is_terminal_process(exe), "{exe}");
    }
}

// ---- covers_monitor ----

#[test]
fn a_window_covers_a_monitor_it_matches_or_exceeds() {
    let monitor = (0, 0, 1920, 1080);
    assert!(covers_monitor((0, 0, 1920, 1080), monitor));
    assert!(covers_monitor((-8, -8, 1928, 1088), monitor));
    // Larger than the monitor: a borderless game across two screens.
    assert!(covers_monitor((-1920, 0, 1920, 1080), monitor));
}

#[test]
fn a_window_one_pixel_short_does_not_cover_a_monitor() {
    let monitor = (0, 0, 1920, 1080);
    assert!(!covers_monitor((1, 0, 1920, 1080), monitor));
    assert!(!covers_monitor((0, 1, 1920, 1080), monitor));
    assert!(!covers_monitor((0, 0, 1919, 1080), monitor));
    assert!(!covers_monitor((0, 0, 1920, 1079), monitor));
}

#[test]
fn a_monitor_off_the_origin_is_compared_where_it_is() {
    let second = (1920, 0, 3840, 1080);
    assert!(covers_monitor((1920, 0, 3840, 1080), second));
    assert!(!covers_monitor((0, 0, 1920, 1080), second));
}

#[test]
fn an_empty_monitor_is_covered_by_nothing() {
    assert!(!covers_monitor((0, 0, 100, 100), (0, 0, 0, 0)));
    assert!(!covers_monitor((0, 0, 100, 100), (50, 50, 50, 80)));
}

// ---- is_full_screen_window ----

/// A maximized window with a frame overhangs the monitor by its resize
/// border once the taskbar hides itself: it is an ordinary window, and
/// counting it as full screen would silence every banner behind it.
#[test]
fn a_maximized_window_with_a_frame_is_not_full_screen() {
    let monitor = (0, 0, 1920, 1080);
    // Taskbar set to hide itself: the work area is the whole monitor.
    assert!(!is_full_screen_window((-8, -8, 1928, 1088), monitor, true));
    // Taskbar shown: the window stops above it.
    assert!(!is_full_screen_window((-8, -8, 1928, 1040), monitor, true));
    // On a second monitor, the same.
    let second = (1920, 0, 4480, 1440);
    assert!(!is_full_screen_window((1912, -8, 4488, 1448), second, true));
}

#[test]
fn a_window_that_is_exactly_the_monitor_is_full_screen() {
    let monitor = (0, 0, 1920, 1080);
    // A video, a browser's F11, a borderless game: no frame at all.
    assert!(is_full_screen_window(monitor, monitor, false));
    // A borderless window maximized to the whole monitor looks the same.
    assert!(is_full_screen_window(monitor, monitor, true));
    let second = (1920, 0, 4480, 1440);
    assert!(is_full_screen_window(second, second, false));
    assert!(!is_full_screen_window(monitor, second, false));
}

#[test]
fn an_unmaximized_window_larger_than_its_monitor_is_full_screen() {
    let monitor = (0, 0, 1920, 1080);
    // A borderless game stretched across two screens.
    assert!(is_full_screen_window(
        (-1920, 0, 1920, 1080),
        monitor,
        false
    ));
    assert!(is_full_screen_window((-8, -8, 1928, 1088), monitor, false));
    // Short of the monitor by a pixel: an ordinary window.
    assert!(!is_full_screen_window((0, 0, 1920, 1079), monitor, false));
    assert!(!is_full_screen_window((0, 0, 1919, 1080), monitor, true));
    assert!(!is_full_screen_window(
        (0, 0, 100, 100),
        (0, 0, 0, 0),
        false
    ));
}

// ---- Dwell ----

#[test]
fn an_app_switch_has_to_stay_put_a_second_and_a_half() {
    assert_eq!(VIEWED_DWELL, std::time::Duration::from_millis(1500));
}
