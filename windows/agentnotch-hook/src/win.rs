//! The few Win32 calls every part of the exe shares: wide strings, owned handles, this user's
//! SID, a process's creation time. Each does one thing and reports failure as `None`; nothing
//! here can panic or print. Declared for Windows only (`main.rs`).

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, PSID, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// A NUL-terminated UTF-16 copy of `text` for a `PCWSTR` argument.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The calling thread's last Win32 error.
pub fn last_error() -> u32 {
    // SAFETY: reads the thread's error slot; no arguments.
    unsafe { GetLastError() }
}

/// A handle this process owns; closed when dropped.
pub struct Handle(HANDLE);

impl Handle {
    /// Takes ownership of `raw` unless it is null or `INVALID_HANDLE_VALUE`.
    pub fn new(raw: HANDLE) -> Option<Handle> {
        (!raw.is_null() && raw != INVALID_HANDLE_VALUE).then_some(Handle(raw))
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and is not used after this.
        unsafe { CloseHandle(self.0) };
    }
}

/// `S-1-5-21-…` of the user this process runs as: the pipe's name comes from it, and the pipe's
/// owner is compared with it.
pub fn current_user_sid() -> Option<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo-handle of the current process needs no closing; `token` is a valid out
    // pointer, and what it receives is owned by `Handle` below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let token = Handle::new(token)?;
    // TOKEN_USER is followed by the SID it points into, so the first call only asks for the size.
    let mut needed = 0u32;
    // SAFETY: a size query: no buffer, a valid out pointer for the length. It fails with
    // ERROR_INSUFFICIENT_BUFFER by design.
    unsafe { GetTokenInformation(token.raw(), TokenUser, null_mut(), 0, &mut needed) };
    if (needed as usize) < size_of::<TOKEN_USER>() {
        return None;
    }
    // u64 elements keep the buffer aligned for TOKEN_USER's pointer field.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buffer` holds at least `needed` bytes and outlives every use of what it holds.
    let read = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };
    if read == 0 {
        return None;
    }
    // SAFETY: the call above filled the buffer with a TOKEN_USER, and the buffer is aligned for it.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_to_string(user.User.Sid)
}

/// The string form of a SID (`S-1-…`).
///
/// `sid` must point at a valid SID for the duration of the call.
pub fn sid_to_string(sid: PSID) -> Option<String> {
    if sid.is_null() {
        return None;
    }
    let mut text: *mut u16 = null_mut();
    // SAFETY: the caller guarantees `sid` is valid; `text` receives a LocalAlloc'd string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return None;
    }
    // SAFETY: `text` is the NUL-terminated string the call just returned.
    let string = unsafe { from_wide_ptr(text) };
    // SAFETY: the string came from LocalAlloc (ConvertSidToStringSidW) and is not used after this.
    unsafe { LocalFree(text.cast::<c_void>()) };
    Some(string)
}

/// A NUL-terminated UTF-16 string as a `String` (unpaired surrogates become U+FFFD).
///
/// # Safety
/// `text` must point at a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(text: *const u16) -> String {
    let mut len = 0usize;
    // SAFETY: the caller guarantees a terminating NUL, so every unit up to it is readable.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) })
}

/// A process opened for the little this exe asks of others: its times, its image, its parent.
pub fn open_process(pid: u32) -> Option<Handle> {
    // SAFETY: plain arguments; the returned handle (null on failure) is owned by `Handle`.
    Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// When a process was created, as a FILETIME (100 ns since 1601). Windows reuses pids quickly,
/// so a pid only names a process together with this.
pub fn created(process: HANDLE) -> Option<u64> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `process` is a live handle (or the current-process pseudo-handle) and the four out
    // pointers are valid.
    let ok =
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } != 0;
    ok.then(|| (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

/// FILETIME (100 ns since 1601-01-01) to milliseconds since the Unix epoch; 0 for times before it.
// The hook role compares creation times as they are; the console helper's `--started` is in
// Unix milliseconds and is the caller of this.
#[allow(dead_code)]
pub fn filetime_to_unix_ms(filetime: u64) -> u64 {
    /// 1601-01-01 to 1970-01-01, in 100 ns units.
    const EPOCH_DIFFERENCE: u64 = 116_444_736_000_000_000;
    filetime.saturating_sub(EPOCH_DIFFERENCE) / 10_000
}
