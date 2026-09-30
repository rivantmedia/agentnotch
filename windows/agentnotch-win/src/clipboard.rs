//! Copying text for the panel and Settings (DESIGN-WIN §3.5 `copy_text`; WP9): `CF_UNICODETEXT`
//! through the Win32 clipboard.
//!
//! Another app may hold the clipboard open for a moment (clipboard managers do, right after a
//! change), so opening it is retried briefly before giving up with an error the page can show.

use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

/// `CF_UNICODETEXT` (the standard clipboard format numbers are fixed by Win32).
const CF_UNICODETEXT: u32 = 13;
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_RETRY: Duration = Duration::from_millis(20);

/// Opens the clipboard for this thread, closing it again when dropped.
struct Open;

impl Open {
    fn new() -> Result<Open, String> {
        let mut last = String::new();
        for _ in 0..OPEN_ATTEMPTS {
            // SAFETY: opens the clipboard for this thread with no owner window; closed by Drop.
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(Open),
                Err(e) => last = e.message(),
            }
            std::thread::sleep(OPEN_RETRY);
        }
        Err(format!("the clipboard is in use by another app ({last})"))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard in `Open::new`.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Puts `text` on the clipboard as Unicode text, replacing what was there. The clipboard is
/// opened without an owner window: nothing is rendered later on request, so none is needed.
pub fn set_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    let _open = Open::new()?;
    // SAFETY: the clipboard is open for this thread (above).
    unsafe { EmptyClipboard() }.map_err(|e| e.message())?;
    // SAFETY: a movable block of the right size; freed below unless the clipboard takes it over.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }.map_err(|e| e.message())?;
    // SAFETY: `memory` is the block just allocated; the lock gives its address until unlocked.
    let target = unsafe { GlobalLock(memory) } as *mut u16;
    if target.is_null() {
        // SAFETY: the block was never handed over; freeing it is ours to do.
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err("the clipboard's memory couldn't be locked".into());
    }
    // SAFETY: `target` points at `bytes` writable bytes, exactly `wide.len()` u16s; the ranges
    // can't overlap (one is a fresh allocation).
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
        let _ = GlobalUnlock(memory);
    }
    // SAFETY: on success the clipboard owns `memory` and it must not be freed here.
    match unsafe { SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))) } {
        Ok(_) => Ok(()),
        Err(e) => {
            // SAFETY: not taken over by the clipboard, so still ours.
            let _ = unsafe { GlobalFree(Some(memory)) };
            Err(e.message())
        }
    }
}

/// The clipboard's Unicode text, if it holds any.
pub fn text() -> Result<Option<String>, String> {
    let _open = Open::new()?;
    // SAFETY: the clipboard is open for this thread; the handle stays owned by the clipboard.
    let Ok(handle) = (unsafe { GetClipboardData(CF_UNICODETEXT) }) else {
        return Ok(None);
    };
    let memory = HGLOBAL(handle.0);
    // SAFETY: a clipboard handle for CF_UNICODETEXT is a global memory block; locked while read.
    let source = unsafe { GlobalLock(memory) } as *const u16;
    if source.is_null() {
        return Ok(None);
    }
    // SAFETY: the block holds GlobalSize bytes; the text ends at the first NUL within them.
    let text = unsafe {
        let units = GlobalSize(memory) / 2;
        let slice = std::slice::from_raw_parts(source, units);
        let end = slice.iter().position(|&u| u == 0).unwrap_or(units);
        let text = String::from_utf16_lossy(&slice[..end]);
        let _ = GlobalUnlock(memory);
        text
    };
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_through_the_clipboard() {
        // The one test here that changes something a person at this desktop can see: it
        // replaces the clipboard. Text that was there is put back at the end; anything else
        // (an image, files, another app's own formats) cannot be read through this module and
        // is lost, as is the text itself when an assertion below fails first.
        let before = text().ok().flatten();
        for sample in ["héllo wörld ✓ 日本 😀", "", "line one\r\nline two"] {
            set_text(sample).expect("the clipboard takes text");
            assert_eq!(
                text().expect("the clipboard reads").as_deref(),
                Some(sample)
            );
        }
        if let Some(before) = before {
            let _ = set_text(&before);
        }
    }
}
