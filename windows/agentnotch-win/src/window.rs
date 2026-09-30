//! The sessions panel's window on Windows (DESIGN-WIN §5.3; WP9): its extended styles as the
//! self-test reads them, topmost re-assertion, the foreground window (saved before the panel
//! takes the keyboard, given back on close, and read to confirm the panel really holds it before
//! its keyboard gate opens), z-order and monitor facts, and the single-instance plumbing the
//! glue's duplicate-launch check needs.
//!
//! Every function does one OS thing and returns plain data. Window handles cross the API as
//! `isize`: the app crate, Tauri and this crate each link their own `windows` version, whose
//! `HWND` types differ, so the raw value is what they share (upstream's `topmost.rs` does the
//! same). A handle that no longer names a window makes each call fail harmlessly: `None`,
//! `false` or an error, never a panic.
//!
//! What is *not* here: setting `WS_EX_NOACTIVATE`. The panel is a tao window, and tao rewrites a
//! window's whole extended style from its own flags whenever one of them changes, so a bit set
//! behind its back is lost at the next change. The glue turns activation off and on through
//! Tauri's `set_focusable`, which is that flag.

use std::mem::size_of;

use windows::core::HSTRING;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenMutexW, ReleaseMutex, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, FlashWindowEx, GetCursorPos, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SendMessageW,
    SetForegroundWindow, SetWindowPos, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, GWL_EXSTYLE,
    GW_HWNDPREV, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WM_COPYDATA,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

/// A rectangle in physical screen pixels: `right` and `bottom` are exclusive, as Win32 has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Self {
        Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

/// A monitor as Windows lays it out: its whole area and the part the taskbar leaves free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorArea {
    pub monitor: Rect,
    pub work: Rect,
}

/// The extended styles the panel's rules and the self-test look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Styles {
    /// The raw `GWL_EXSTYLE` value.
    pub ex_style: u32,
    pub topmost: bool,
    pub no_activate: bool,
    pub tool_window: bool,
    pub visible: bool,
}

/// Who holds a named mutex, as far as this thread can tell ([`claim_mutex`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutexClaim {
    /// No mutex of that name exists.
    Missing,
    /// This thread owned it already; asking changed nothing.
    Ours,
    /// Nobody owned it: this thread owns it now, and keeps it until the process ends.
    Taken,
    /// Another thread or process owns it.
    Elsewhere,
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

/// Whether `window` still names a window.
pub fn exists(window: isize) -> bool {
    // SAFETY: IsWindow accepts any value and only reports whether it names a window.
    window != 0 && unsafe { IsWindow(Some(hwnd(window))) }.as_bool()
}

/// The window's extended styles and visibility; `None` when it no longer exists.
pub fn styles(window: isize) -> Option<Styles> {
    if !exists(window) {
        return None;
    }
    // SAFETY: plain reads of a window's long and visibility; a stale handle yields 0/false.
    let (ex, visible) = unsafe {
        (
            GetWindowLongPtrW(hwnd(window), GWL_EXSTYLE) as u32,
            IsWindowVisible(hwnd(window)).as_bool(),
        )
    };
    Some(Styles {
        ex_style: ex,
        topmost: ex & WS_EX_TOPMOST.0 != 0,
        no_activate: ex & WS_EX_NOACTIVATE.0 != 0,
        tool_window: ex & WS_EX_TOOLWINDOW.0 != 0,
        visible,
    })
}

/// Puts the window at the top of the topmost band without activating it (`HWND_TOPMOST`,
/// `SWP_NOACTIVATE`): tao never re-asserts its own always-on-top flag, and every other topmost
/// window of the app (the notch, re-asserted by upstream's watchdog) goes above it otherwise.
pub fn raise_topmost(window: isize) -> bool {
    if !exists(window) {
        return false;
    }
    // SAFETY: a z-order change only; no move, size or activation.
    unsafe {
        SetWindowPos(
            hwnd(window),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
    .is_ok()
}

/// The foreground window, if any.
pub fn foreground() -> Option<isize> {
    // SAFETY: a read without arguments.
    let raw = unsafe { GetForegroundWindow() }.0 as isize;
    (raw != 0).then_some(raw)
}

/// Asks Windows to make `window` the foreground window, with `SetForegroundWindow` alone: no
/// synthetic key press and no attached input queues, the tricks that force it. Windows may
/// refuse (the foreground lock rules): the answer says only whether the request was accepted,
/// so callers confirm with [`foreground`] before relying on it. Nothing else is done to the
/// window: one the user minimised in the meantime (the window the panel gives the keyboard back
/// to) stays minimised.
pub fn request_foreground(window: isize) -> bool {
    if !exists(window) {
        return false;
    }
    // SAFETY: takes a window handle and changes only which window is in front; a stale handle
    // makes it answer false.
    unsafe { SetForegroundWindow(hwnd(window)) }.as_bool()
}

/// Flashes the window's taskbar button (or the window) until it comes to the front: what Windows
/// shows instead when it refuses a foreground request.
pub fn flash(window: isize) {
    if !exists(window) {
        return;
    }
    let info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: hwnd(window),
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 2,
        dwTimeout: 0,
    };
    // SAFETY: `info` is a fully initialised FLASHWINFO that outlives the call.
    let _ = unsafe { FlashWindowEx(&info) };
}

/// The window's rectangle in physical screen pixels.
pub fn window_rect(window: isize) -> Option<Rect> {
    if !exists(window) {
        return None;
    }
    let mut r = RECT::default();
    // SAFETY: `r` is a valid out pointer for the duration of the call.
    unsafe { GetWindowRect(hwnd(window), &mut r) }.ok()?;
    Some(r.into())
}

/// The process that owns the window.
pub fn process_of(window: isize) -> Option<u32> {
    if !exists(window) {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer; a stale handle leaves it 0.
    unsafe { GetWindowThreadProcessId(hwnd(window), Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

fn monitor_info(monitor: HMONITOR) -> Option<MonitorArea> {
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` has its size set and is a valid out pointer for the duration of the call.
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then(|| MonitorArea {
            monitor: info.rcMonitor.into(),
            work: info.rcWork.into(),
        })
}

/// The monitor the window is mostly on (the nearest one when it is off screen).
pub fn monitor_of(window: isize) -> Option<MonitorArea> {
    if !exists(window) {
        return None;
    }
    // SAFETY: MonitorFromWindow with DEFAULTTONEAREST always answers for an existing window.
    monitor_info(unsafe { MonitorFromWindow(hwnd(window), MONITOR_DEFAULTTONEAREST) })
}

/// The monitor at a point in physical screen pixels (the nearest one outside every monitor).
pub fn monitor_at(x: i32, y: i32) -> Option<MonitorArea> {
    // SAFETY: MonitorFromPoint takes the point by value.
    monitor_info(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) })
}

/// The pointer's position in physical screen pixels.
pub fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out pointer for the duration of the call.
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((p.x, p.y))
}

/// Whether a mouse button is down right now, anywhere on the desktop: how the panel notices a
/// click outside itself while it is not the foreground window (it then gets no blur).
pub fn mouse_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|key| {
        // SAFETY: a read of the asynchronous key state; the high bit is "down now".
        (unsafe { GetAsyncKeyState(i32::from(key.0)) } as u16 & 0x8000) != 0
    })
}

/// Whether `upper` is in front of `lower` in the z-order; `None` when either window is gone or
/// the walk ran past its bound.
pub fn is_above(upper: isize, lower: isize) -> Option<bool> {
    if !exists(upper) || !exists(lower) || upper == lower {
        return None;
    }
    // Walk from `lower` towards the top of the z-order: `upper` is above exactly when the walk
    // meets it. Bounded, so a z-order that changes under the walk can't make it run on.
    let mut current = hwnd(lower);
    for _ in 0..20_000 {
        // SAFETY: GetWindow on a handle from the previous step; a stale one ends the walk with an
        // error, never undefined behaviour.
        match unsafe { GetWindow(current, GW_HWNDPREV) } {
            Ok(previous) if previous.0 as isize == upper => return Some(true),
            Ok(previous) => current = previous,
            Err(_) => return Some(false),
        }
    }
    None
}

/// A top-level window by class and title, with the process that owns it.
pub fn find_window(class: &str, title: &str) -> Option<(isize, u32)> {
    let (class, title) = (HSTRING::from(class), HSTRING::from(title));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let found = unsafe { FindWindowW(&class, &title) }.ok()?;
    if found.is_invalid() {
        return None;
    }
    let raw = found.0 as isize;
    Some((raw, process_of(raw)?))
}

/// This process's id.
pub fn current_process_id() -> u32 {
    // SAFETY: a read without arguments.
    unsafe { GetCurrentProcessId() }
}

/// Sends `bytes` to `window` as `WM_COPYDATA` tagged `kind`, waiting until the window has
/// handled it (as long as that takes: a window whose thread hangs keeps the caller waiting, as
/// it does the single-instance plugin this is the counterpart of). `true` when the window
/// answered that it did.
pub fn send_copy_data(window: isize, kind: usize, bytes: &[u8]) -> bool {
    // The message carries its length in 32 bits: more than that is never sent cut short.
    let Ok(length) = u32::try_from(bytes.len()) else {
        return false;
    };
    if !exists(window) {
        return false;
    }
    let data = COPYDATASTRUCT {
        dwData: kind,
        cbData: length,
        lpData: bytes.as_ptr() as *mut _,
    };
    // SAFETY: WM_COPYDATA's contract: `data` and the bytes it points at stay valid until
    // SendMessageW returns, which is after the receiver has copied them.
    let answer = unsafe {
        SendMessageW(
            hwnd(window),
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(&data as *const COPYDATASTRUCT as isize)),
        )
    };
    answer.0 != 0
}

/// Whether this thread can take `handle` without waiting: `true` and one more ownership count
/// when it could, `false` when another thread or process owns it.
fn acquire_now(handle: HANDLE) -> bool {
    // SAFETY: a zero-timeout wait on a handle the caller opened with SYNCHRONIZE.
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED
}

/// Finds out who holds the named mutex, from the calling thread.
///
/// A thread that owns a mutex may wait on it again without blocking, and so may any thread when
/// nobody owns it; telling the two apart takes a second opinion from a helper thread, which can
/// take the mutex only in the second case. When nobody owned it, the calling thread takes it and
/// keeps it ([`MutexClaim::Taken`]): a name that guards "one copy of the app" must not be left
/// free for the next launch to find.
pub fn claim_mutex(name: &str) -> MutexClaim {
    let wide = HSTRING::from(name);
    // SAFETY: the name is NUL-terminated; what becomes of the handle is settled at the end.
    let Ok(handle) = (unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &wide) }) else {
        return MutexClaim::Missing;
    };
    let helper_name = name.to_owned();
    // The helper's answer: could a thread that does not own the mutex take it?
    let free = std::thread::spawn(move || {
        let wide = HSTRING::from(helper_name);
        // SAFETY: as above; the handle never leaves this closure.
        let Ok(handle) = (unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &wide) }) else {
            return false;
        };
        let free = acquire_now(handle);
        if free {
            // SAFETY: gives back exactly the ownership the wait above took, on the same thread.
            let _ = unsafe { ReleaseMutex(handle) };
        }
        // SAFETY: opened above, not used after this.
        let _ = unsafe { CloseHandle(handle) };
        free
    })
    .join()
    .unwrap_or(false);
    let claim = if !acquire_now(handle) {
        MutexClaim::Elsewhere
    } else if free {
        // Nobody held it: the count the wait took is the ownership this thread keeps.
        MutexClaim::Taken
    } else {
        // It was ours already: the wait only added a count, which goes back.
        // SAFETY: releases the one count the wait above added, on the owning thread.
        let _ = unsafe { ReleaseMutex(handle) };
        MutexClaim::Ours
    };
    // A mutex this thread has just taken is kept alive by this handle, which is therefore left
    // open for the life of the process. In the other cases the owner has a handle of its own.
    if claim != MutexClaim::Taken {
        // SAFETY: opened above, not used after this.
        let _ = unsafe { CloseHandle(handle) };
    }
    claim
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, ShowWindow, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WS_POPUP,
    };

    /// A bare top-level window of a system class, off in a corner, never activated: a small
    /// empty rectangle for the length of a test is all a desktop sees of these tests, and no
    /// other app's window is touched.
    fn test_window(title: &str, ex: WINDOW_EX_STYLE) -> isize {
        let title = HSTRING::from(title);
        // SAFETY: a popup of the predefined STATIC class with no parent, menu or instance data.
        let window = unsafe {
            CreateWindowExW(
                ex,
                w!("STATIC"),
                &title,
                WS_POPUP,
                40,
                60,
                120,
                80,
                None,
                None,
                None,
                None,
            )
        }
        .expect("a test window");
        window.0 as isize
    }

    fn destroy(window: isize) {
        // SAFETY: a window this thread created.
        let _ = unsafe { DestroyWindow(hwnd(window)) };
    }

    fn unique(name: &str) -> String {
        format!(
            "agentnotch-test-{name}-{}-{:?}",
            current_process_id(),
            std::thread::current().id()
        )
    }

    #[test]
    fn styles_rect_monitor_and_owner_of_a_real_window() {
        let window = test_window(
            &unique("styles"),
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
        );
        assert!(exists(window));
        let before = styles(window).expect("styles");
        assert!(before.topmost && before.no_activate && before.tool_window);
        assert!(!before.visible);
        // SAFETY: shows a window this thread owns, without activating it.
        let _ = unsafe { ShowWindow(hwnd(window), SW_SHOWNOACTIVATE) };
        assert!(styles(window).expect("styles").visible);
        let rect = window_rect(window).expect("rect");
        assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (120, 80));
        let area = monitor_of(window).expect("a monitor");
        assert!(area.work.left >= area.monitor.left && area.work.right <= area.monitor.right);
        assert!(area.work.top >= area.monitor.top && area.work.bottom <= area.monitor.bottom);
        assert_eq!(monitor_at(rect.left + 1, rect.top + 1), Some(area));
        assert_eq!(process_of(window), Some(current_process_id()));
        destroy(window);
        assert!(!exists(window));
        assert_eq!(styles(window), None);
        assert_eq!(window_rect(window), None);
        assert_eq!(process_of(window), None);
        assert!(!raise_topmost(window));
        assert!(!request_foreground(window));
    }

    #[test]
    fn the_last_window_raised_is_above_the_other() {
        let (a, b) = (
            test_window(
                &unique("z-a"),
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            ),
            test_window(
                &unique("z-b"),
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            ),
        );
        for window in [a, b] {
            // SAFETY: shows a window this thread owns, without activating it.
            let _ = unsafe { ShowWindow(hwnd(window), SW_SHOWNOACTIVATE) };
        }
        assert!(raise_topmost(a));
        assert!(raise_topmost(b));
        assert_eq!(is_above(b, a), Some(true));
        assert_eq!(is_above(a, b), Some(false));
        assert!(raise_topmost(a));
        assert_eq!(is_above(a, b), Some(true));
        assert_eq!(is_above(a, a), None);
        destroy(a);
        assert_eq!(is_above(a, b), None);
        destroy(b);
    }

    #[test]
    fn a_window_is_found_by_class_and_title() {
        let title = unique("find");
        let window = test_window(&title, WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
        assert_eq!(
            find_window("STATIC", &title),
            Some((window, current_process_id()))
        );
        assert_eq!(find_window("STATIC", &format!("{title}-absent")), None);
        // A window that does not take WM_COPYDATA answers 0: "not handled".
        assert!(!send_copy_data(window, 1542, b"cwd|arg\0"));
        destroy(window);
        assert!(!send_copy_data(window, 1542, b"cwd|arg\0"));
    }

    #[test]
    fn the_pointer_and_the_foreground_can_be_read() {
        // Whatever they are on this desktop, reading them must never fail loudly.
        let _ = cursor();
        let _ = mouse_button_down();
        if let Some(front) = foreground() {
            assert!(exists(front));
        }
        flash(0);
    }

    #[test]
    fn a_mutex_is_missing_ours_taken_or_elsewhere() {
        let name = unique("mutex");
        assert_eq!(claim_mutex(&name), MutexClaim::Missing);
        let wide = HSTRING::from(name.as_str());
        // Owned by this thread from its creation, as the single-instance plugin makes its own.
        // SAFETY: a named mutex with default security; the handle lives to the end of the test.
        let owned = unsafe { CreateMutexW(None, true, &wide) }.expect("a mutex");
        assert_eq!(claim_mutex(&name), MutexClaim::Ours);
        // Asking changed nothing: still ours, and still not free for another thread.
        assert_eq!(claim_mutex(&name), MutexClaim::Ours);
        let other = name.clone();
        let seen = std::thread::spawn(move || claim_mutex(&other))
            .join()
            .expect("the other thread");
        assert_eq!(seen, MutexClaim::Elsewhere);
        // Released: the next thread to ask takes it and keeps it.
        // SAFETY: releases the ownership this thread has held since CreateMutexW.
        unsafe { ReleaseMutex(owned) }.expect("release");
        assert_eq!(claim_mutex(&name), MutexClaim::Taken);
        assert_eq!(claim_mutex(&name), MutexClaim::Ours);
        let other = name.clone();
        let seen = std::thread::spawn(move || claim_mutex(&other))
            .join()
            .expect("the other thread");
        assert_eq!(seen, MutexClaim::Elsewhere);
        // SAFETY: the handle made above.
        let _ = unsafe { CloseHandle(owned) };
    }
}
