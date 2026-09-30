//! The user's SID (DESIGN-WIN §1.4; WP1): the pipe's name (`agentnotch_proto::pipe_name`) and its
//! security descriptor both come from the string SID of the process token's user, so the hook
//! and the app compute the same name without anything being templated into hook commands.
//!
//! Also here: the SID of whoever the calling thread is impersonating (the pipe server's peer
//! check), and the security descriptor an SDDL string describes (the pipe's owner and DACL).

#![cfg(windows)]

use std::mem::size_of;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

/// `S-1-5-21-…` of the user this process runs as; `None` when the token can't be read.
pub fn current_user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle of the current process needs no closing; `token` is a valid out
    // pointer and is closed below on every path once it was opened.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let sid = sid_of_token(token);
    // SAFETY: `token` was opened above and is not used after this.
    let _ = unsafe { CloseHandle(token) };
    sid
}

/// `S-1-…` of the user the calling thread is impersonating; `None` when it impersonates nobody
/// or its token can't be read.
///
/// The token is opened as the process (`OpenAsSelf`), not as the impersonated user: at
/// identification level, which is all a pipe client grants, the impersonated user may not open
/// anything, its own token included.
pub fn thread_token_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle of the current thread needs no closing; `token` is a valid out
    // pointer and is closed below on every path once it was opened.
    unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) }.ok()?;
    let sid = sid_of_token(token);
    // SAFETY: `token` was opened above and is not used after this.
    let _ = unsafe { CloseHandle(token) };
    sid
}

/// `S-1-…` of the user `token` stands for. The token must be open with `TOKEN_QUERY`; it is
/// neither closed nor kept.
pub fn sid_of_token(token: HANDLE) -> Option<String> {
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

/// A security descriptor built from SDDL, freed when dropped.
#[derive(Debug)]
pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is one block of process memory from LocalAlloc that nothing else
// points to and that is never written after it was built; no thread owns it.
unsafe impl Send for SecurityDescriptor {}

impl SecurityDescriptor {
    /// The descriptor `sddl` describes (`agentnotch_proto::pipe_sddl`); the error says why
    /// Windows refused the string.
    pub fn from_sddl(sddl: &str) -> Result<Self, String> {
        let text = HSTRING::from(sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `text` is a NUL-terminated string that outlives the call; `descriptor` is a
        // valid out pointer that receives a LocalAlloc'd block, freed in `drop`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &text,
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|error| error.message())?;
        if descriptor.0.is_null() {
            return Err("no security descriptor was returned".into());
        }
        Ok(SecurityDescriptor(descriptor))
    }

    /// For `SECURITY_ATTRIBUTES::lpSecurityDescriptor`; valid while `self` lives.
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0 .0
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the block was allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW
        // with LocalAlloc and is not used after this.
        unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}
