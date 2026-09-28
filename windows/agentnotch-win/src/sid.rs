//! The user's SID (DESIGN-WIN §1.4; WP1): the pipe's name (`agentnotch_proto::pipe_name`) and its
//! security descriptor both come from the string SID of the process token's user, so the hook
//! and the app compute the same name without anything being templated into hook commands.

#![cfg(windows)]

use std::mem::size_of;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `S-1-5-21-…` of the user this process runs as; `None` when the token can't be read.
pub fn current_user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle of the current process needs no closing; `token` is a valid out
    // pointer and is closed below on every path once it was opened.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let sid = token_user_sid(token);
    // SAFETY: `token` was opened above and is not used after this.
    let _ = unsafe { CloseHandle(token) };
    sid
}

fn token_user_sid(token: HANDLE) -> Option<String> {
    // TOKEN_USER is followed by the SID it points into, so the buffer is sized by the first call.
    let mut needed = 0u32;
    // SAFETY: a size query: no buffer, a valid out pointer for the length. It fails with
    // ERROR_INSUFFICIENT_BUFFER by design, which is why its result is ignored.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if (needed as usize) < size_of::<TOKEN_USER>() {
        return None;
    }
    // u64 elements keep the buffer aligned for TOKEN_USER's pointer field.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buffer` holds at least `needed` bytes and outlives every use of what it holds.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .ok()?;
    // SAFETY: the call above filled the buffer with a TOKEN_USER, and the buffer is aligned for it.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = PWSTR::null();
    // SAFETY: the SID points into `buffer`, which is alive; `text` receives a LocalAlloc'd string.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.ok()?;
    // SAFETY: `text` is the NUL-terminated string the call just returned.
    let sid = unsafe { text.to_string() }.ok();
    // SAFETY: the string was allocated by ConvertSidToStringSidW with LocalAlloc and is not used
    // after this.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    sid
}
