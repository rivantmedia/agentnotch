//! The client end of the hook pipe (DESIGN-WIN §1.4 "Client").
//!
//! - The name is `\\.\pipe\agentnotch-hook-<SID of this user>`; `AGENTNOTCH_SOCKET` replaces it
//!   only together with `AGENTNOTCH_DEV=1`, so a leftover export never sends a real session
//!   elsewhere.
//! - The pipe is opened with `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`: the server may
//!   find out who this client is, but can never act as it.
//! - Before a byte is written the pipe's own security descriptor is read back: it must be owned
//!   by this user and closed to everyone else (`PipeSecurity::is_ours`). Pipe names are global,
//!   so another local user can create the name first; what a hook sends are tool inputs, and what
//!   it reads back approves tools. Reading the pipe object rather than the server process works
//!   whatever the two integrity levels are.
//! - When every instance is busy, the open waits for the next one and tries again, for as long
//!   as the caller's budget lasts: a burst of hooks (parallel tool calls) all get through.
//! - The handle is synchronous: a write blocks while the server does not read. The watchdog
//!   bounds that, not this module.
//!
//! "No app" is the common case on a PC where Agent Notch is not running, and the cheapest: one
//! failed open.

use std::time::Duration;

/// Why no pipe was opened. The hook does nothing in every case; the trace tells them apart.
// Only Windows has a pipe to be busy or someone else's; elsewhere every run is `NoApp`.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No pipe of that name: the app is not running.
    NoApp,
    /// Every instance stayed busy for what was left of the budget.
    Busy,
    /// The pipe exists but is not this user's app's.
    NotOurs,
    /// Anything else (the token could not be read, access was denied, …).
    Failed,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::NoApp => "no app",
            Refusal::Busy => "pipe busy",
            Refusal::NotOurs => "pipe is not ours",
            Refusal::Failed => "pipe failed",
        }
    }
}

/// The pipe's name for this user, or the development override.
fn name_for(user_sid: &str) -> String {
    agentnotch_proto::dev_pipe_override(crate::io::env_text, false)
        .unwrap_or_else(|| agentnotch_proto::pipe_name(user_sid))
}

/// Writes one frame; false when the pipe broke.
pub fn send(pipe: &mut Pipe, json: &[u8]) -> bool {
    agentnotch_proto::write_frame(pipe, json).is_ok()
}

/// Reads one frame of at most `max` bytes; `None` when the server closed without one ("no
/// decision") or the frame was cut or too large.
pub fn receive(pipe: &mut Pipe, max: usize) -> Option<Vec<u8>> {
    agentnotch_proto::read_frame(pipe, max).ok()
}

/// How long a client that found every instance of the pipe busy waits for the next one
/// (`WaitNamedPipeW`): what is left of its budget, in whole milliseconds; `None` once that is
/// spent, and the client gives up. Never 0, which Windows reads as "the server's default wait".
// Only Windows has a pipe to be busy; the rule is tested on every system.
#[cfg_attr(not(windows), allow(dead_code))]
fn busy_wait_ms(budget: Duration, elapsed: Duration) -> Option<u32> {
    let left = budget.checked_sub(elapsed)?.as_millis();
    (left > 0).then(|| left.min(60_000) as u32)
}

#[cfg(windows)]
pub use windows_impl::{connect, Pipe};

#[cfg(windows)]
mod windows_impl {
    use super::{name_for, Refusal};
    use crate::win::{self, Handle};
    use agentnotch_proto::{PipeAce, PipeSecurity};
    use std::ffi::c_void;
    use std::io::{self, Read, Write};
    use std::ptr::{addr_of_mut, null, null_mut};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        LocalFree, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
        ERROR_PIPE_NOT_CONNECTED, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE,
    };
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    };
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    /// `ACCESS_ALLOWED_ACE_TYPE`: the one kind of entry the app's pipe carries.
    const ACCESS_ALLOWED: u8 = 0;

    /// An open, checked connection to the app.
    pub struct Pipe {
        handle: Handle,
    }

    /// Opens this user's hook pipe and checks that it is the app's, within `budget`.
    pub fn connect(budget: Duration) -> Result<Pipe, Refusal> {
        let started = Instant::now();
        let me = win::current_user_sid().ok_or(Refusal::Failed)?;
        let name = win::wide(&name_for(&me));
        let pipe = Pipe {
            handle: open(&name, budget, started)?,
        };
        match pipe.security() {
            Some(security) if security.is_ours(&me) => Ok(pipe),
            _ => Err(Refusal::NotOurs),
        }
    }

    fn open(name: &[u16], budget: Duration, started: Instant) -> Result<Handle, Refusal> {
        loop {
            // SAFETY: `name` is NUL-terminated and outlives the call; no security attributes and
            // no template file are passed.
            let raw = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    // The server may identify this client, never act as it.
                    SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    null_mut(),
                )
            };
            if let Some(handle) = Handle::new(raw) {
                return Ok(handle);
            }
            match win::last_error() {
                ERROR_FILE_NOT_FOUND => return Err(Refusal::NoApp),
                // Every instance is taken this instant. The server makes a new one as soon as it
                // accepts a connection, but every client waiting wakes when it comes and only
                // one gets it: the hooks of parallel tool calls arrive together. So the others
                // wait again, for as long as the budget lasts (`busy_wait_ms`).
                ERROR_PIPE_BUSY => {
                    let Some(wait) = super::busy_wait_ms(budget, started.elapsed()) else {
                        return Err(Refusal::Busy);
                    };
                    // SAFETY: `name` is NUL-terminated and outlives the call.
                    if unsafe { WaitNamedPipeW(name.as_ptr(), wait) } == 0 {
                        return Err(match win::last_error() {
                            // The app went away meanwhile.
                            ERROR_FILE_NOT_FOUND => Refusal::NoApp,
                            _ => Refusal::Busy,
                        });
                    }
                }
                _ => return Err(Refusal::Failed),
            }
        }
    }

    impl Pipe {
        /// The pipe object's owner and DACL, read through this handle.
        fn security(&self) -> Option<PipeSecurity> {
            let mut owner: PSID = null_mut();
            let mut dacl: *mut ACL = null_mut();
            let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
            // SAFETY: the handle is open (with READ_CONTROL, part of GENERIC_READ); the out
            // pointers are valid; the group and SACL are not asked for.
            let status = unsafe {
                GetSecurityInfo(
                    self.handle.raw(),
                    SE_KERNEL_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner,
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut descriptor,
                )
            };
            if status != ERROR_SUCCESS || descriptor.is_null() {
                return None;
            }
            let security = read_descriptor(owner, dacl, descriptor);
            // SAFETY: GetSecurityInfo allocated the descriptor with LocalAlloc; `owner` and `dacl`
            // point into it and are not used after this.
            unsafe { LocalFree(descriptor) };
            security
        }
    }

    fn read_descriptor(
        owner: PSID,
        dacl: *const ACL,
        descriptor: PSECURITY_DESCRIPTOR,
    ) -> Option<PipeSecurity> {
        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `descriptor` is the valid descriptor GetSecurityInfo returned.
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return None;
        }
        let aces = if dacl.is_null() {
            None
        } else {
            Some(read_aces(dacl)?)
        };
        Some(PipeSecurity {
            owner: win::sid_to_string(owner),
            dacl_protected: control & SE_DACL_PROTECTED != 0,
            dacl: aces,
        })
    }

    fn read_aces(dacl: *const ACL) -> Option<Vec<PipeAce>> {
        let mut info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        // SAFETY: `dacl` is the valid ACL of the descriptor; `info` is a valid out buffer of the
        // size passed.
        let ok = unsafe {
            GetAclInformation(
                dacl,
                addr_of_mut!(info).cast::<c_void>(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        };
        if ok == 0 {
            return None;
        }
        let mut aces = Vec::with_capacity(info.AceCount as usize);
        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: `index` is below the ACL's entry count; `ace` is a valid out pointer.
            if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
                return None;
            }
            // SAFETY: every ACE starts with a header, and GetAce returned one inside the ACL.
            let kind = unsafe { (*ace.cast::<ACE_HEADER>()).AceType };
            if kind == ACCESS_ALLOWED {
                // SAFETY: an access-allowed ACE is a header, a mask and then the SID, which
                // starts where `SidStart` is.
                let sid = unsafe { addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart) };
                aces.push(PipeAce {
                    allow: true,
                    sid: win::sid_to_string(sid.cast::<c_void>()).unwrap_or_default(),
                });
            } else {
                // Deny, audit, object and callback entries have other layouts, and none belongs
                // on the app's pipe.
                aces.push(PipeAce {
                    allow: false,
                    sid: String::new(),
                });
            }
        }
        Some(aces)
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len().min(u32::MAX as usize) as u32;
            let mut written = 0u32;
            // SAFETY: `buf` is valid for `len` bytes; the handle is synchronous, so there is no
            // OVERLAPPED and the call returns once the bytes are in the pipe.
            let ok = unsafe {
                WriteFile(
                    self.handle.raw(),
                    buf.as_ptr(),
                    len,
                    &mut written,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::from_raw_os_error(win::last_error() as i32));
            }
            Ok(written as usize)
        }

        fn flush(&mut self) -> io::Result<()> {
            // Nothing is buffered here. `FlushFileBuffers` would wait until the server has read
            // everything, which a fire-and-forget hook must not.
            Ok(())
        }
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(u32::MAX as usize) as u32;
            let mut read = 0u32;
            // SAFETY: `buf` is valid for `len` bytes; synchronous handle, no OVERLAPPED.
            let ok = unsafe {
                ReadFile(
                    self.handle.raw(),
                    buf.as_mut_ptr(),
                    len,
                    &mut read,
                    null_mut(),
                )
            };
            if ok != 0 {
                return Ok(read as usize);
            }
            match win::last_error() {
                // The server closed its end: the end of what it sent.
                ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED => Ok(0),
                error => Err(io::Error::from_raw_os_error(error as i32)),
            }
        }
    }
}

#[cfg(not(windows))]
pub use other_impl::{connect, Pipe};

/// There is no hook pipe on other systems: the exe builds there so its argv handling and the
/// fail-open rules are tested on every OS, and every event takes the "no app" path.
#[cfg(not(windows))]
mod other_impl {
    use super::Refusal;
    use std::convert::Infallible;
    use std::io::{self, Read, Write};
    use std::time::Duration;

    /// Never constructed.
    pub struct Pipe(Infallible);

    pub fn connect(_budget: Duration) -> Result<Pipe, Refusal> {
        // The name is still computed, so the override's rules are exercised on every OS.
        let _ = super::name_for("");
        Err(Refusal::NoApp)
    }

    impl Write for Pipe {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            match self.0 {}
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.0 {}
        }
    }

    impl Read for Pipe {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            match self.0 {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_busy_pipe_is_waited_for_while_the_budget_lasts() {
        let budget = Duration::from_millis(1200);
        assert_eq!(busy_wait_ms(budget, Duration::ZERO), Some(1200));
        assert_eq!(busy_wait_ms(budget, 450 * MS), Some(750));
        assert_eq!(busy_wait_ms(budget, 1199 * MS), Some(1));
        // The status line's budget, the same way.
        let send = Duration::from_millis(300);
        assert_eq!(busy_wait_ms(send, 120 * MS), Some(180));
    }

    #[test]
    fn a_spent_budget_gives_up_and_never_asks_for_the_default_wait() {
        let budget = Duration::from_millis(1200);
        // Less than a whole millisecond left would be a wait of 0: the server's default.
        assert_eq!(
            busy_wait_ms(budget, budget - Duration::from_micros(400)),
            None
        );
        assert_eq!(busy_wait_ms(budget, budget), None);
        assert_eq!(busy_wait_ms(budget, budget + 5 * MS), None);
        assert_eq!(busy_wait_ms(Duration::ZERO, Duration::ZERO), None);
        // A budget of minutes still waits at most one minute at a time.
        assert_eq!(
            busy_wait_ms(Duration::from_secs(600), Duration::ZERO),
            Some(60_000)
        );
    }
}
