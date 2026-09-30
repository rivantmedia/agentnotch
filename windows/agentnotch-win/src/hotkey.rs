//! The panel shortcut (DESIGN-WIN §5.3; WP9): `RegisterHotKey` on a thread of its own
//! (`an-hotkey`) that waits in a message loop and calls back on every press. Off by default; a
//! registration Windows refuses because another app holds the chord is reported as
//! [`HotkeyError::Taken`], so Settings can say "That shortcut is taken by another app."
//!
//! The key goes to the thread's own message queue (a `NULL` window), which needs no window
//! class and is torn down with the thread: dropping the [`Registration`] posts `WM_QUIT`, the
//! loop unregisters the chord and the thread ends.

use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_J,
    VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT, WM_USER,
};

/// The chords Settings offers (Win+… is mostly the system's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    CtrlAltSpace,
    CtrlAltJ,
}

impl Chord {
    fn keys(self) -> (HOT_KEY_MODIFIERS, u32) {
        // MOD_NOREPEAT: holding the chord down opens the panel once, not once per key repeat.
        let modifiers = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
        match self {
            Chord::CtrlAltSpace => (modifiers, u32::from(VK_SPACE.0)),
            Chord::CtrlAltJ => (modifiers, u32::from(VK_J.0)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyError {
    /// Another app registered the chord first.
    Taken,
    Failed(String),
}

/// A registered chord. Dropping it unregisters the chord and ends its thread.
pub struct Registration {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// The id the chord is registered under (any value in 0..=0xBFFF is the app's own).
const HOTKEY_ID: i32 = 0x0A11;

/// Registers `chord`; `on_press` runs on the hotkey thread for every press, so it must return
/// quickly (the glue hands the work to another thread).
pub fn register(chord: Chord, on_press: Box<dyn Fn() + Send>) -> Result<Registration, HotkeyError> {
    let (ready, outcome) = mpsc::channel::<Result<u32, HotkeyError>>();
    let thread = std::thread::Builder::new()
        .name("an-hotkey".into())
        .spawn(move || run(chord, on_press, ready))
        .map_err(|e| HotkeyError::Failed(e.to_string()))?;
    match outcome.recv() {
        Ok(Ok(thread_id)) => Ok(Registration {
            thread_id,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(HotkeyError::Failed("the shortcut's thread ended".into()))
        }
    }
}

fn run(
    chord: Chord,
    on_press: Box<dyn Fn() + Send>,
    ready: mpsc::Sender<Result<u32, HotkeyError>>,
) {
    let mut msg = MSG::default();
    // A thread has no message queue until it asks for one; PostThreadMessageW from Drop would
    // fail against a thread without it.
    // SAFETY: `msg` is a valid out pointer; PM_NOREMOVE leaves the (empty) queue as it is.
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
    let (modifiers, key) = chord.keys();
    // SAFETY: registers for this thread's queue (no window); undone below before the thread ends.
    if let Err(e) = unsafe { RegisterHotKey(None, HOTKEY_ID, modifiers, key) } {
        let error = if e.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult() {
            HotkeyError::Taken
        } else {
            HotkeyError::Failed(e.message())
        };
        let _ = ready.send(Err(error));
        return;
    }
    // SAFETY: a read without arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        // SAFETY: the registration made above on this thread.
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
        return;
    }
    loop {
        // SAFETY: `msg` is a valid out pointer; the call blocks until a message for this thread.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        // 0 is WM_QUIT, -1 an error: either way the loop is over.
        if got.0 <= 0 || msg.message == WM_QUIT {
            break;
        }
        if msg.message == WM_HOTKEY && msg.wParam.0 == HOTKEY_ID as usize {
            // A panicking callback must not take the loop (and the registration) down with it.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&on_press));
        }
    }
    // SAFETY: the registration made above on this thread.
    let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: posts to a thread whose queue exists (made before it reported ready); a thread
        // that already ended makes the call fail harmlessly.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A chord is one per desktop and the tests of a binary run side by side: the one holding
    /// Ctrl+Alt+Space would make the other see it as taken.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    #[test]
    fn a_press_reaches_the_callback_and_a_held_chord_is_taken() {
        let _alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let (pressed, presses) = mpsc::channel();
        let held = match register(
            Chord::CtrlAltJ,
            Box::new(move || {
                let _ = pressed.send(());
            }),
        ) {
            Ok(registration) => registration,
            // On someone's own PC another app may hold the chord; CI's runner has none that does.
            Err(HotkeyError::Taken) if std::env::var("CI").as_deref() != Ok("true") => {
                eprintln!("skipped: another app holds Ctrl+Alt+J on this desktop");
                return;
            }
            Err(e) => panic!("Ctrl+Alt+J could not be registered: {e:?}"),
        };
        // What Windows posts to the registering thread when the chord is pressed. Posted rather
        // than typed: a test never sends keys to whatever window happens to be in front.
        // SAFETY: posts to the hotkey thread's own queue, which exists (it reported ready).
        unsafe {
            PostThreadMessageW(
                held.thread_id,
                WM_HOTKEY,
                WPARAM(HOTKEY_ID as usize),
                LPARAM(0),
            )
        }
        .expect("the hotkey thread takes messages");
        presses
            .recv_timeout(Duration::from_secs(5))
            .expect("the press reaches the callback");
        assert_eq!(
            register(Chord::CtrlAltJ, Box::new(|| {})).err(),
            Some(HotkeyError::Taken)
        );
        // The other chord is a registration of its own.
        let space = register(Chord::CtrlAltSpace, Box::new(|| {}));
        assert!(matches!(space, Ok(_) | Err(HotkeyError::Taken)));
        drop(space);
        drop(held);
        // Dropping the registration gave the chord back.
        let again = register(Chord::CtrlAltJ, Box::new(|| {}));
        assert!(
            again.is_ok(),
            "the chord was not released: {:?}",
            again.err()
        );
    }

    #[test]
    fn a_callback_that_panics_does_not_end_the_loop() {
        let _alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let (pressed, presses) = mpsc::channel();
        let held = match register(
            Chord::CtrlAltSpace,
            Box::new(move || {
                let first = pressed.send(()).is_ok();
                assert!(!first, "a panic inside the callback");
            }),
        ) {
            Ok(registration) => registration,
            Err(HotkeyError::Taken) if std::env::var("CI").as_deref() != Ok("true") => {
                eprintln!("skipped: another app holds Ctrl+Alt+Space on this desktop");
                return;
            }
            Err(e) => panic!("Ctrl+Alt+Space could not be registered: {e:?}"),
        };
        for _ in 0..2 {
            // SAFETY: as above.
            unsafe {
                PostThreadMessageW(
                    held.thread_id,
                    WM_HOTKEY,
                    WPARAM(HOTKEY_ID as usize),
                    LPARAM(0),
                )
            }
            .expect("the hotkey thread takes messages");
            presses
                .recv_timeout(Duration::from_secs(5))
                .expect("every press reaches the callback");
        }
    }
}
