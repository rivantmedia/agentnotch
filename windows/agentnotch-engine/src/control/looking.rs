//! Two questions the attention reactions ask (HS§9.3):
//!
//! - Is the user looking at *this* session? Only a session-precise answer
//!   counts: its console window is the foreground window; its Windows
//!   Terminal window is, and either holds this session alone or shows this
//!   session's title (the window's title is its selected tab's); or the
//!   foreground editor hosts this session and no other. Being in front alone
//!   is not enough: with four Claude tabs in one terminal window, that
//!   window being in front says nothing about which tab the user reads.
//! - Is a terminal window actually visible? Only a terminal or editor window
//!   that other windows don't cover counts, not one buried under a browser.
//!
//! A yes silences chime, peek and banner, and marks a completion viewed.

use crate::model::SessionView;
use crate::platform::{ConsoleInfo, Foreground, HostApp, HostKind};

/// How long an app switch has to stay put before the sessions waiting for
/// review in the front window count as seen.
pub const VIEWED_DWELL: std::time::Duration = std::time::Duration::from_millis(1500);

/// [`looking_at_console`] without the session's console: a Windows Terminal
/// window with several known sessions then answers no (its title can't be
/// compared).
pub fn looking_at(
    view: &SessionView,
    host: &HostApp,
    fg: &Foreground,
    sessions_in_window: usize,
) -> Option<bool> {
    decide(view, host, None, fg, sessions_in_window, &[])
}

/// Whether the user is looking at this session's own terminal. `None` when
/// it can't be told (the host isn't known).
///
/// `sessions_in_window` is how many known sessions that host shows, this
/// one included ([`sessions_sharing`]); `other_titles` are the console
/// titles of the others, so a title two sessions share decides nothing.
pub fn looking_at_console(
    view: &SessionView,
    host: &HostApp,
    info: &ConsoleInfo,
    fg: &Foreground,
    sessions_in_window: usize,
    other_titles: &[String],
) -> Option<bool> {
    decide(view, host, Some(info), fg, sessions_in_window, other_titles)
}

fn decide(
    view: &SessionView,
    host: &HostApp,
    info: Option<&ConsoleInfo>,
    fg: &Foreground,
    sessions_in_window: usize,
    other_titles: &[String],
) -> Option<bool> {
    // A session without a process has no terminal to look at.
    view.pid?;
    let alone = sessions_in_window <= 1;
    let console = info.filter(|info| info.attached);
    match &host.kind {
        HostKind::Conhost => {
            let window = console.and_then(|info| info.window).or(host.window)?;
            Some(fg.window == window)
        }
        HostKind::WindowsTerminal => {
            let window = host.window?;
            if fg.window != window {
                return Some(false);
            }
            if alone {
                return Some(true);
            }
            let title = console
                .and_then(|info| info.title.as_deref())
                .map(str::trim)
                .filter(|title| !title.is_empty());
            Some(title.is_some_and(|title| {
                fg.title.trim() == title && !other_titles.iter().any(|other| other.trim() == title)
            }))
        }
        // The terminal tab inside an editor can't be told from outside.
        HostKind::VsCode { .. } | HostKind::JetBrains | HostKind::OtherConsoleHost { .. } => {
            let in_front = match (host.host_pid, host.window) {
                (Some(pid), _) if fg.pid == pid => true,
                (_, Some(window)) => fg.window == window,
                _ => false,
            };
            Some(in_front && alone)
        }
        HostKind::NoConsole => Some(false),
        HostKind::Unknown => None,
    }
}

/// How many of `hosts` (every known session's host, this one's included)
/// show in the same place as `host`: the same window for a terminal, the
/// same app for an editor.
pub fn sessions_sharing(host: &HostApp, hosts: &[&HostApp]) -> usize {
    let same = |other: &HostApp| match &host.kind {
        HostKind::WindowsTerminal | HostKind::Conhost => {
            host.window.is_some() && other.window == host.window
        }
        _ => host.host_pid.is_some() && other.host_pid == host.host_pid,
    };
    hosts.iter().filter(|other| same(other)).count()
}

// ---- Visible terminal ----

/// A window on screen, as the platform lists them front to back: shown, not
/// minimised, not cloaked (another virtual desktop), not a see-through
/// overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenWindow {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// It belongs to a terminal or an editor (`hosts::is_terminal_process`).
    pub is_terminal: bool,
}

impl ScreenWindow {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= f64::from(self.left)
            && x < f64::from(self.right)
            && y >= f64::from(self.top)
            && y < f64::from(self.bottom)
    }
}

/// A window smaller than this on either side is neither a terminal worth
/// reading nor something that hides one.
pub const MINIMUM_WINDOW_SIDE: i32 = 40;
/// A terminal counts as visible when at least this much of it shows.
pub const MINIMUM_VISIBLE_FRACTION: f64 = 0.15;
const GRID: i32 = 5;

/// Whether any terminal window is visible: not hidden under the windows in
/// front of it. Coverage is sampled on a 5 × 5 grid; a window counts as
/// visible when at least `minimum_visible_fraction` of its samples show.
pub fn any_terminal_uncovered(windows: &[ScreenWindow], minimum_visible_fraction: f64) -> bool {
    let mut in_front: Vec<&ScreenWindow> = Vec::new();
    for window in windows {
        let (width, height) = (window.right - window.left, window.bottom - window.top);
        if width < MINIMUM_WINDOW_SIDE || height < MINIMUM_WINDOW_SIDE {
            continue;
        }
        if window.is_terminal {
            let mut showing = 0;
            for row in 0..GRID {
                for column in 0..GRID {
                    let x = f64::from(window.left)
                        + f64::from(width) * (f64::from(column) + 0.5) / f64::from(GRID);
                    let y = f64::from(window.top)
                        + f64::from(height) * (f64::from(row) + 0.5) / f64::from(GRID);
                    if !in_front.iter().any(|other| other.contains(x, y)) {
                        showing += 1;
                    }
                }
            }
            if f64::from(showing) / f64::from(GRID * GRID) >= minimum_visible_fraction {
                return true;
            }
        }
        in_front.push(window);
    }
    false
}

/// A window's bounds against its monitor's: it covers the whole monitor
/// (a full-screen app, a video, a game in borderless mode).
pub fn covers_monitor(window: (i32, i32, i32, i32), monitor: (i32, i32, i32, i32)) -> bool {
    let (left, top, right, bottom) = window;
    let (m_left, m_top, m_right, m_bottom) = monitor;
    m_right > m_left
        && m_bottom > m_top
        && left <= m_left
        && top <= m_top
        && right >= m_right
        && bottom >= m_bottom
}

/// Whether the foreground window is a full-screen app by its geometry
/// (`window` is its `GetWindowRect`, `maximized` its `IsZoomed`).
///
/// A maximized window with a frame overhangs its work area by the invisible
/// resize border on every side, so with the taskbar set to hide itself it
/// covers the whole monitor while still being an ordinary window with a
/// title bar: counting it would silence every banner while any maximized
/// window is in front. A full-screen app (a video, a browser's F11, a game
/// in borderless mode) has no such border: its rectangle is the monitor's
/// exactly, maximized or not.
pub fn is_full_screen_window(
    window: (i32, i32, i32, i32),
    monitor: (i32, i32, i32, i32),
    maximized: bool,
) -> bool {
    covers_monitor(window, monitor) && (!maximized || window == monitor)
}
