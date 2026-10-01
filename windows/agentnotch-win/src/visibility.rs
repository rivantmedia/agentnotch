//! Whether a terminal is visible (DESIGN-WIN §4.9; WP6): EnumWindows front to back, skipping
//! cloaked and minimised windows, plus full-screen detection. Plain OS reads only: the engine's
//! `control::looking` owns the Mac's 15 % grid rule and `covers_monitor`.
//!
//! Rects are `(left, top, right, bottom)` in physical pixels, right and bottom exclusive, which
//! is what `ScreenWindow` and `covers_monitor` expect. Nothing here changes the process's DPI
//! awareness (the app is per-monitor aware already), so `DWMWA_EXTENDED_FRAME_BOUNDS` and
//! `rcMonitor` share one coordinate space.

#![cfg(windows)]

use std::ffi::c_void;

use agentnotch_engine::control::hosts::{is_terminal_process, TopWindow};
use agentnotch_engine::control::looking::{
    any_terminal_uncovered, covers_monitor, ScreenWindow, MINIMUM_VISIBLE_FRACTION,
};
use agentnotch_engine::platform::ProcessTable;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetLayeredWindowAttributes, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, GW_OWNER,
    LWA_ALPHA, WS_EX_LAYERED,
};

/// The desktop and the shell's own windows: they cover a whole monitor without being a
/// full-screen app.
const SHELL_CLASSES: [&str; 3] = ["Progman", "WorkerW", "Shell_TrayWnd"];
/// A layered window at or below this opacity (the Mac's 0.05 of 255) neither shows a terminal
/// nor hides one: an invisible overlay.
const SEE_THROUGH_ALPHA: u8 = 13;

/// One window of the walk.
struct Walked {
    hwnd: HWND,
    pid: u32,
}

fn hwnd_of(window: u64) -> HWND {
    HWND(window as usize as *mut c_void)
}

fn window_id(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

/// Every top-level window, front to back, that is visible, unowned, not minimised and not
/// cloaked (another virtual desktop). The callback collects into the Vec behind `LPARAM`.
fn walk() -> Vec<Walked> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the address of the `Vec` `walk` owns for the whole EnumWindows call.
        let found = unsafe { &mut *(lparam.0 as *mut Vec<Walked>) };
        if is_candidate(hwnd) {
            let mut pid = 0u32;
            // SAFETY: `pid` is a valid out pointer for the call.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            found.push(Walked { hwnd, pid });
        }
        BOOL(1)
    }
    let mut found: Vec<Walked> = Vec::new();
    // SAFETY: the callback only touches the Vec passed here, which outlives the call. An error
    // (the walk stopped early) leaves what was collected, which is an honest partial answer.
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut found as *mut Vec<Walked> as isize),
        )
    };
    found
}

fn is_candidate(hwnd: HWND) -> bool {
    // SAFETY: plain queries on a window handle; a window that died meanwhile just answers no.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        // A window with an owner is a dialog or tool window of another one.
        if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.0.is_null()) {
            return false;
        }
    }
    !is_cloaked(hwnd)
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the out pointer and its size describe one u32. When the call fails the window is
    // treated as not cloaked.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer is a valid slice for the call.
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

fn is_shell_window(hwnd: HWND) -> bool {
    let class = class_name(hwnd);
    SHELL_CLASSES.iter().any(|shell| class == *shell)
}

fn is_see_through(hwnd: HWND) -> bool {
    // SAFETY: plain queries; the out pointers are valid for the call.
    unsafe {
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_LAYERED.0 == 0 {
            return false;
        }
        let mut alpha = 0u8;
        let mut flags = LWA_ALPHA;
        // Layered windows drawn per pixel have no single alpha: they count as opaque.
        GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)).is_ok()
            && flags.0 & LWA_ALPHA.0 != 0
            && alpha <= SEE_THROUGH_ALPHA
    }
}

fn rect_tuple(rect: RECT) -> (i32, i32, i32, i32) {
    (rect.left, rect.top, rect.right, rect.bottom)
}

/// The visible frame: DWM's extended bounds (without the invisible resize borders), else
/// `GetWindowRect`.
fn frame_bounds(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    let mut rect = RECT::default();
    // SAFETY: the out pointer and its size describe one RECT.
    let framed = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if framed.is_ok() {
        return Some(rect_tuple(rect));
    }
    window_rect(window_id(hwnd))
}

/// The visible top-level windows, front to back, with their process's file name. Windows of
/// processes the table does not know are left out.
pub fn top_windows(table: &ProcessTable) -> Vec<TopWindow> {
    walk()
        .into_iter()
        .filter_map(|walked| {
            let entry = table.get(walked.pid)?;
            Some(TopWindow {
                window: window_id(walked.hwnd),
                pid: walked.pid,
                exe_name: entry.exe_name.clone(),
            })
        })
        .collect()
}

/// The same walk as screen rectangles for the engine's grid rule. Unlike `top_windows`, a window
/// of an unknown process stays in the list as a non-terminal: it still hides what lies behind it.
/// The desktop and shell windows and see-through overlays are left out.
pub fn screen_windows(table: &ProcessTable) -> Vec<ScreenWindow> {
    walk()
        .into_iter()
        .filter(|walked| !is_shell_window(walked.hwnd) && !is_see_through(walked.hwnd))
        .filter_map(|walked| {
            let (left, top, right, bottom) = frame_bounds(walked.hwnd)?;
            let is_terminal = table
                .get(walked.pid)
                .is_some_and(|entry| is_terminal_process(&entry.exe_name));
            Some(ScreenWindow {
                left,
                top,
                right,
                bottom,
                is_terminal,
            })
        })
        .collect()
}

/// Whether any terminal or editor window shows, not hidden under the windows in front of it.
pub fn any_terminal_visible(table: &ProcessTable) -> bool {
    any_terminal_uncovered(&screen_windows(table), MINIMUM_VISIBLE_FRACTION)
}

/// A window's bounds, `GetWindowRect` (a maximised or full-screen window's own rect, which is
/// what `covers_monitor` compares with the monitor's).
pub fn window_rect(window: u64) -> Option<(i32, i32, i32, i32)> {
    let mut rect = RECT::default();
    // SAFETY: the out pointer is valid; a dead window makes the call fail.
    unsafe { GetWindowRect(hwnd_of(window), &mut rect) }
        .ok()
        .map(|()| rect_tuple(rect))
}

/// The bounds of the monitor the window is mostly on (the nearest one for a window on none).
pub fn monitor_rect(window: u64) -> Option<(i32, i32, i32, i32)> {
    // SAFETY: plain queries; `info.cbSize` is set as the call requires.
    unsafe {
        let monitor = MonitorFromWindow(hwnd_of(window), MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info)
            .as_bool()
            .then(|| rect_tuple(info.rcMonitor))
    }
}

/// Whether the user is in something full screen: Windows says a full-screen app, a game or a
/// presentation has the screen, or the foreground window (not the desktop or the shell) covers
/// its monitor. `foreground` is the window id, 0 for none.
pub fn is_full_screen(foreground: u64) -> bool {
    // SAFETY: a plain query. An error (no answer) falls through to the geometry check.
    let state = unsafe { SHQueryUserNotificationState() };
    if matches!(
        state,
        Ok(QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    ) {
        return true;
    }
    if foreground == 0 || is_shell_window(hwnd_of(foreground)) {
        return false;
    }
    match (window_rect(foreground), monitor_rect(foreground)) {
        (Some(window), Some(monitor)) => covers_monitor(window, monitor),
        _ => false,
    }
}
