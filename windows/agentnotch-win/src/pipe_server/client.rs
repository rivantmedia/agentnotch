//! The hook pipe's client for the app's own programs (DESIGN-WIN §1.4 "Client"): the CLI's
//! `control status|quit`, the doctor, the uninstaller and the tests. The hook exe has its own
//! copy of the same rules (`agentnotch-hook/src/pipe.rs`): it may not depend on this crate.
//!
//! - The pipe is opened with `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`: the server may
//!   find out who the client is, but can never act as it.
//! - Before a byte is written the pipe's own security descriptor is read back through the
//!   handle: it must be owned by this user and closed to everyone but this user and SYSTEM
//!   (`PipeSecurity::is_ours`). Pipe names are global, so another local user can make the name
//!   first; a request written to that pipe, or an answer read from it, would be theirs.
//! - One request frame goes out and at most one response frame comes back, all of it within the
//!   caller's timeout. A server that closes without a frame answered "nothing".
//!
//! The functions exist on every system with the same signature, so the code that calls them
//! builds and is tested on macOS and Linux too; there they answer [`ClientError::NotAvailable`].

use std::time::Duration;

use agentnotch_proto::{ControlOp, ControlRequest, ControlResponse, PIPE_NAME_PREFIX};

/// Why a request got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// This system has no hook pipe (anything but Windows).
    NotAvailable,
    /// No pipe of that name: the app is not running.
    NotRunning,
    /// The pipe exists but is not this user's app's. Nothing was written to it.
    NotOurs,
    /// Every instance of the pipe stayed busy for the whole timeout.
    Busy,
    /// The app did not take the request, or did not answer it, in time.
    Timeout,
    /// Anything else, in words.
    Other(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotAvailable => write!(f, "the hook pipe exists on Windows only"),
            ClientError::NotRunning => write!(f, "Agent Notch is not running"),
            ClientError::NotOurs => {
                write!(f, "the hook pipe belongs to another program or user")
            }
            ClientError::Busy => write!(f, "the hook pipe is busy"),
            ClientError::Timeout => write!(f, "Agent Notch did not answer in time"),
            ClientError::Other(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Whether `name` is `\\.\pipe\<name>` with nothing but a name after the prefix. Anything else
/// would make the client open, and write a frame into, some other device or file.
pub fn is_pipe_path(name: &str) -> bool {
    let prefix = PIPE_NAME_PREFIX.len();
    name.len() > prefix
        && name.is_char_boundary(prefix)
        && name[..prefix].eq_ignore_ascii_case(PIPE_NAME_PREFIX)
        && !name[prefix..].contains(['\\', '/'])
}

/// Sends one frame to the app listening on `pipe_name` and reads its answer.
///
/// `Ok(Some(json))` is the response frame's body; `Ok(None)` means the app took the request and
/// closed the connection without a frame. `timeout` bounds the whole exchange, the connect
/// included.
pub fn request(
    pipe_name: &str,
    frame_json: &[u8],
    timeout: Duration,
) -> Result<Option<Vec<u8>>, ClientError> {
    if !is_pipe_path(pipe_name) {
        return Err(ClientError::Other(format!("not a pipe name: {pipe_name}")));
    }
    imp::request(pipe_name, frame_json, timeout)
}

/// Asks the running app for its status, or to quit (`agentnotch.exe control status|quit`).
///
/// The answer is returned as the app gave it: `ok == false` with its `error` is still `Ok`.
pub fn control(
    pipe_name: &str,
    op: ControlOp,
    timeout: Duration,
) -> Result<ControlResponse, ClientError> {
    let answer = request(pipe_name, &ControlRequest::new(op).to_json(), timeout)?;
    control_response(answer)
}

/// Reads a control request's answer. An app that closed without one did not understand the
/// request (an older copy), which is not the same as "not running".
fn control_response(answer: Option<Vec<u8>>) -> Result<ControlResponse, ClientError> {
    let Some(json) = answer else {
        return Err(ClientError::Other(
            "Agent Notch closed the connection without answering".into(),
        ));
    };
    serde_json::from_slice(&json)
        .map_err(|_| ClientError::Other("Agent Notch's answer could not be read".into()))
}

/// How long a client that found every instance of the pipe busy waits for the next one
/// (`WaitNamedPipeW`): what is left until its deadline, in whole milliseconds; `None` once that
/// has passed. Never 0, which Windows reads as "the server's default wait", nor `INFINITE`.
// Only Windows has a pipe to be busy; the rule is tested on every system.
#[cfg_attr(not(windows), allow(dead_code))]
fn busy_wait_ms(left: Duration) -> Option<u32> {
    let left = left.as_millis();
    (left > 0).then(|| left.min(u128::from(u32::MAX - 1)) as u32)
}

#[cfg(not(windows))]
mod imp {
    use super::ClientError;
    use std::time::Duration;

    pub fn request(
        _pipe_name: &str,
        _frame_json: &[u8],
        _timeout: Duration,
    ) -> Result<Option<Vec<u8>>, ClientError> {
        Err(ClientError::NotAvailable)
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io::{self, Read, Write};
    use std::mem::size_of;
    use std::ptr::{addr_of_mut, null_mut};
    use std::time::{Duration, Instant};

    use agentnotch_proto::limits::MAX_RESPONSE;
    use agentnotch_proto::{read_frame, write_frame, FrameError, PipeAce, PipeSecurity};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
        SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::Win32::System::Threading::CreateEventW;
    use windows::Win32::System::IO::{
        CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
    };

    use super::ClientError;
    use crate::sid;

    // Win32 error codes, as plain numbers: they are compared with what `win32_code` takes out of
    // an HRESULT.
    const FILE_NOT_FOUND: u32 = 2;
    const HANDLE_EOF: u32 = 38;
    const BROKEN_PIPE: u32 = 109;
    const PIPE_BUSY: u32 = 231;
    const NO_DATA: u32 = 232;
    const PIPE_NOT_CONNECTED: u32 = 233;
    const WAIT_TIMEOUT: u32 = 258;
    const IO_INCOMPLETE: u32 = 996;
    const IO_PENDING: u32 = 997;

    /// `ACCESS_ALLOWED_ACE_TYPE`: the one kind of entry the app's pipe carries.
    const ACCESS_ALLOWED: u8 = 0;
    /// One write or read hands Windows at most this much, so a length always fits its `u32`.
    const CHUNK: usize = 1 << 30;

    /// A handle this module opened; closed when dropped.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and is not used after this.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// The Win32 error code inside `error`, when it carries one.
    fn win32_code(error: &windows::core::Error) -> Option<u32> {
        let hresult = error.code().0 as u32;
        // 0x8007xxxx is HRESULT_FROM_WIN32: severity "error", facility WIN32, the code below.
        (hresult & 0xFFFF_0000 == 0x8007_0000).then_some(hresult & 0xFFFF)
    }

    fn io_error(error: &windows::core::Error) -> io::Error {
        match win32_code(error) {
            Some(code) => io::Error::from_raw_os_error(code as i32),
            None => io::Error::other(error.message()),
        }
    }

    fn timed_out() -> io::Error {
        io::ErrorKind::TimedOut.into()
    }

    pub fn request(
        pipe_name: &str,
        frame_json: &[u8],
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, ClientError> {
        let deadline = Instant::now() + timeout;
        let me = sid::current_user_sid()
            .ok_or_else(|| ClientError::Other("this user's SID is unreadable".into()))?;
        let handle = open(&HSTRING::from(pipe_name), deadline)?;
        // Checked before anything is written: a pipe somebody else made must learn nothing.
        match security(handle.0) {
            Some(security) if security.is_ours(&me) => {}
            _ => return Err(ClientError::NotOurs),
        }
        // Manual reset, not signalled: each operation's completion sets it.
        // SAFETY: no security attributes and no name; the returned handle is owned by `Owned`.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Owned)
            .map_err(|error| ClientError::Other(error.message()))?;
        let mut pipe = Pipe {
            handle,
            event,
            deadline,
        };
        write_frame(&mut pipe, frame_json).map_err(|error| match error.kind() {
            io::ErrorKind::TimedOut => ClientError::Timeout,
            _ => ClientError::Other(format!("the request could not be written: {error}")),
        })?;
        match read_frame(&mut pipe, MAX_RESPONSE) {
            Ok(json) => Ok(Some(json)),
            // Closed without a frame: the app's way of answering nothing.
            Err(FrameError::Eof) => Ok(None),
            Err(FrameError::Io(error)) if error.kind() == io::ErrorKind::TimedOut => {
                Err(ClientError::Timeout)
            }
            Err(error) => Err(ClientError::Other(error.to_string())),
        }
    }

    /// Milliseconds until `deadline`, for a Win32 wait. Never `INFINITE`.
    fn left_ms(deadline: Instant) -> u32 {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        left.min(u128::from(u32::MAX - 1)) as u32
    }

    fn open(name: &HSTRING, deadline: Instant) -> Result<Owned, ClientError> {
        loop {
            // SAFETY: `name` is a NUL-terminated string that outlives the call; no security
            // attributes and no template file are passed.
            let opened = unsafe {
                CreateFileW(
                    name,
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    // The server may identify this client, never act as it. Overlapped, so
                    // every write and read can be given up at the deadline.
                    SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION | FILE_FLAG_OVERLAPPED,
                    None,
                )
            };
            let error = match opened {
                Ok(handle) => return Ok(Owned(handle)),
                Err(error) => error,
            };
            match win32_code(&error) {
                Some(FILE_NOT_FOUND) => return Err(ClientError::NotRunning),
                // Every instance is taken this instant. The server makes a new one as soon as
                // it accepts a connection, but every client waiting wakes when it comes and
                // only one gets it, so the others wait again until the deadline.
                Some(PIPE_BUSY) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    let Some(wait) = super::busy_wait_ms(left) else {
                        return Err(ClientError::Busy);
                    };
                    // SAFETY: `name` is a NUL-terminated string that outlives the call.
                    if !unsafe { WaitNamedPipeW(name, wait) }.as_bool() {
                        let gone = io::Error::last_os_error().raw_os_error()
                            == Some(FILE_NOT_FOUND as i32);
                        // The app went away meanwhile, or no instance came free in time.
                        return Err(if gone {
                            ClientError::NotRunning
                        } else {
                            ClientError::Busy
                        });
                    }
                }
                _ => {
                    return Err(ClientError::Other(format!(
                        "the hook pipe can't be opened: {}",
                        error.message()
                    )))
                }
            }
        }
    }

    /// The pipe object's owner and DACL, read through the client's own handle, so it works
    /// whatever the server's integrity level is.
    fn security(pipe: HANDLE) -> Option<PipeSecurity> {
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the handle is open (with READ_CONTROL, part of GENERIC_READ); the out pointers
        // are valid; the group and SACL are not asked for.
        let status = unsafe {
            GetSecurityInfo(
                pipe,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut descriptor),
            )
        };
        if status != ERROR_SUCCESS || descriptor.0.is_null() {
            return None;
        }
        let security = read_descriptor(owner, dacl, descriptor);
        // SAFETY: GetSecurityInfo allocated the descriptor with LocalAlloc; `owner` and `dacl`
        // point into it and are not used after this.
        unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        security
    }

    fn read_descriptor(
        owner: PSID,
        dacl: *const ACL,
        descriptor: PSECURITY_DESCRIPTOR,
    ) -> Option<PipeSecurity> {
        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `descriptor` is the valid descriptor GetSecurityInfo returned.
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.ok()?;
        let aces = if dacl.is_null() {
            None
        } else {
            Some(read_aces(dacl)?)
        };
        Some(PipeSecurity {
            owner: sid_to_string(owner),
            dacl_protected: control & SE_DACL_PROTECTED.0 != 0,
            dacl: aces,
        })
    }

    fn read_aces(dacl: *const ACL) -> Option<Vec<PipeAce>> {
        let mut info = ACL_SIZE_INFORMATION::default();
        // SAFETY: `dacl` is the valid ACL of the descriptor; `info` is a valid out buffer of the
        // size passed.
        unsafe {
            GetAclInformation(
                dacl,
                addr_of_mut!(info).cast::<c_void>(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        }
        .ok()?;
        let mut aces = Vec::with_capacity(info.AceCount as usize);
        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: `index` is below the ACL's entry count; `ace` is a valid out pointer.
            unsafe { GetAce(dacl, index, &mut ace) }.ok()?;
            if ace.is_null() {
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
                    sid: sid_to_string(PSID(sid.cast::<c_void>())).unwrap_or_default(),
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

    /// The string form of a SID that lives inside a descriptor this module holds.
    fn sid_to_string(sid: PSID) -> Option<String> {
        if sid.0.is_null() {
            return None;
        }
        let mut text = PWSTR::null();
        // SAFETY: `sid` points at a valid SID inside the live descriptor; `text` receives a
        // LocalAlloc'd string.
        unsafe { ConvertSidToStringSidW(sid, &mut text) }.ok()?;
        // SAFETY: `text` is the NUL-terminated string the call just returned.
        let string = unsafe { text.to_string() }.ok();
        // SAFETY: the string came from LocalAlloc and is not used after this.
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        string
    }

    /// An open, checked connection to the app whose every operation ends by `deadline`.
    struct Pipe {
        handle: Owned,
        /// What each overlapped operation signals when it is done.
        event: Owned,
        deadline: Instant,
    }

    impl Pipe {
        /// Waits for the operation `WriteFile`/`ReadFile` just started on `overlapped`, until
        /// the deadline. It never returns while Windows may still touch `overlapped` or the
        /// caller's buffer: an operation that is given up is cancelled and waited for.
        fn finish(
            &self,
            overlapped: &mut OVERLAPPED,
            started: windows::core::Result<()>,
        ) -> io::Result<usize> {
            if let Err(error) = started {
                // Pending is the normal case; anything else never started.
                if win32_code(&error) != Some(IO_PENDING) {
                    return Err(io_error(&error));
                }
            }
            let mut done = 0u32;
            // SAFETY: the handle is open; `overlapped` is the structure the operation was
            // started with; `done` is a valid out pointer.
            let waited = unsafe {
                GetOverlappedResultEx(
                    self.handle.0,
                    overlapped,
                    &mut done,
                    left_ms(self.deadline),
                    false,
                )
            };
            let error = match waited {
                Ok(()) => return Ok(done as usize),
                Err(error) => error,
            };
            if !matches!(win32_code(&error), Some(WAIT_TIMEOUT | IO_INCOMPLETE)) {
                return Err(io_error(&error));
            }
            // Out of time. The operation is still Windows's: cancel it and wait until it has
            // really ended (at once, either way).
            // SAFETY: as above; a cancel that finds nothing to cancel is harmless.
            let _ = unsafe { CancelIoEx(self.handle.0, Some(overlapped)) };
            // SAFETY: as above; waiting is what makes it safe to drop `overlapped` afterwards.
            match unsafe { GetOverlappedResult(self.handle.0, overlapped, &mut done, true) } {
                // It finished just before the cancel reached it.
                Ok(()) => Ok(done as usize),
                Err(_) => Err(timed_out()),
            }
        }

        fn overlapped(&self) -> OVERLAPPED {
            OVERLAPPED {
                hEvent: self.event.0,
                ..OVERLAPPED::default()
            }
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let buf = &buf[..buf.len().min(CHUNK)];
            let mut overlapped = self.overlapped();
            // SAFETY: the handle is open for overlapped I/O; `buf` and `overlapped` stay alive
            // and untouched until `finish` has seen the operation end.
            let started =
                unsafe { WriteFile(self.handle.0, Some(buf), None, Some(&mut overlapped)) };
            self.finish(&mut overlapped, started)
        }

        fn flush(&mut self) -> io::Result<()> {
            // Nothing is buffered here, and `FlushFileBuffers` would wait for the server to
            // read everything with no deadline.
            Ok(())
        }
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(CHUNK);
            let buf = &mut buf[..len];
            let mut overlapped = self.overlapped();
            // SAFETY: the handle is open for overlapped I/O; `buf` and `overlapped` stay alive
            // and untouched until `finish` has seen the operation end.
            let started =
                unsafe { ReadFile(self.handle.0, Some(buf), None, Some(&mut overlapped)) };
            match self.finish(&mut overlapped, started) {
                Ok(read) => Ok(read),
                Err(error) => match error.raw_os_error().map(|code| code as u32) {
                    // The server closed its end: the end of what it sent.
                    Some(BROKEN_PIPE | PIPE_NOT_CONNECTED | HANDLE_EOF | NO_DATA) => Ok(0),
                    _ => Err(error),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = r"\\.\pipe\agentnotch-test-client";
    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn only_a_pipe_name_is_opened() {
        assert!(is_pipe_path(NAME));
        assert!(is_pipe_path(
            r"\\.\PIPE\agentnotch-hook-S-1-5-21-1-2-3-1001"
        ));
        for other in [
            "",
            r"\\.\pipe\",
            r"\\.\pipe\a\b",
            r"\\.\pipe\a/b",
            r"C:\Users\me\file.json",
            r"\\server\pipe\agentnotch",
            "/tmp/agentnotch.sock",
            r"\\.\pipeéx",
        ] {
            assert!(!is_pipe_path(other), "{other}");
        }
    }

    #[test]
    fn anything_but_a_pipe_name_is_refused_before_it_is_opened() {
        let refused = request(r"C:\Users\me\settings.json", b"{}", SECOND);
        assert_eq!(
            refused,
            Err(ClientError::Other(
                r"not a pipe name: C:\Users\me\settings.json".into()
            ))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn there_is_no_pipe_on_other_systems() {
        assert_eq!(request(NAME, b"{}", SECOND), Err(ClientError::NotAvailable));
        assert_eq!(
            control(NAME, ControlOp::Status, SECOND),
            Err(ClientError::NotAvailable)
        );
        assert_eq!(
            control(NAME, ControlOp::Quit, SECOND),
            Err(ClientError::NotAvailable)
        );
    }

    #[test]
    fn a_control_answer_is_read_as_the_app_gave_it() {
        let status = br#"{"ok":true,"status":{"version":"1.1.0","sessions":3,"later":1}}"#;
        let parsed = control_response(Some(status.to_vec())).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.status.unwrap().sessions, 3);

        let quit = control_response(Some(br#"{"ok":true}"#.to_vec())).unwrap();
        assert_eq!(quit, ControlResponse::ok());

        // A refusal is the app's answer, not a failure of the exchange.
        let refused = control_response(Some(br#"{"ok":false,"error":"sealed"}"#.to_vec()));
        assert_eq!(refused, Ok(ControlResponse::error("sealed")));
    }

    #[test]
    fn no_frame_or_an_unreadable_one_is_an_error() {
        assert_eq!(
            control_response(None),
            Err(ClientError::Other(
                "Agent Notch closed the connection without answering".into()
            ))
        );
        assert_eq!(
            control_response(Some(b"not json".to_vec())),
            Err(ClientError::Other(
                "Agent Notch's answer could not be read".into()
            ))
        );
    }

    #[test]
    fn every_failure_reads_differently() {
        let all = [
            ClientError::NotAvailable,
            ClientError::NotRunning,
            ClientError::NotOurs,
            ClientError::Busy,
            ClientError::Timeout,
            ClientError::Other("why".into()),
        ];
        let mut texts: Vec<String> = all.iter().map(ToString::to_string).collect();
        assert_eq!(texts[1], "Agent Notch is not running");
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), all.len());
    }

    #[test]
    fn a_busy_pipe_is_waited_for_until_the_deadline() {
        assert_eq!(busy_wait_ms(SECOND), Some(1000));
        assert_eq!(busy_wait_ms(Duration::from_millis(1)), Some(1));
        // Less than a whole millisecond left would be a wait of 0, the server's default: no.
        assert_eq!(busy_wait_ms(Duration::from_micros(999)), None);
        assert_eq!(busy_wait_ms(Duration::ZERO), None);
        // A long timeout never becomes INFINITE.
        assert_eq!(
            busy_wait_ms(Duration::from_secs(u64::from(u32::MAX))),
            Some(u32::MAX - 1)
        );
    }
}
