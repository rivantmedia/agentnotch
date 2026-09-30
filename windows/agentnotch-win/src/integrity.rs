//! Token integrity levels (DESIGN-WIN §1.4, §7.3; WP1).
//!
//! The pipe's checks are built to work across integrity levels: the server identifies its peer
//! by impersonation, and the client checks the pipe's own descriptor, so neither opens the other
//! process's token (which fails from medium towards high). This module reads a token's level and
//! elevation (the doctor's `elevated:` line; the admin tests' preconditions) and, for
//! `tests/win_admin.rs` only, starts a child at medium integrity from an elevated process, so one
//! test run can put the hook and the server on either side of the boundary.
//!
//! The level mapping, the command line and the environment block are plain functions tested on
//! every system; the rest exists on Windows only.

/// A token's mandatory integrity level, from the last sub-authority of its label SID
/// (`S-1-16-<rid>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Untrusted,
    Low,
    /// Medium and medium-plus (UI Access): a normal, non-elevated user process.
    Medium,
    /// An elevated administrator.
    High,
    /// SYSTEM, and protected processes above it.
    System,
}

/// `SECURITY_MANDATORY_*_RID`.
pub const LOW_RID: u32 = 0x1000;
pub const MEDIUM_RID: u32 = 0x2000;
pub const MEDIUM_PLUS_RID: u32 = 0x2100;
pub const HIGH_RID: u32 = 0x3000;
pub const SYSTEM_RID: u32 = 0x4000;

/// The level a label RID stands for. Windows compares levels as numbers, so a RID between two
/// named ones counts as the lower of them.
pub const fn level_of_rid(rid: u32) -> Level {
    if rid < LOW_RID {
        Level::Untrusted
    } else if rid < MEDIUM_RID {
        Level::Low
    } else if rid < HIGH_RID {
        Level::Medium
    } else if rid < SYSTEM_RID {
        Level::High
    } else {
        Level::System
    }
}

/// One argument quoted the way `CommandLineToArgvW` and the C runtime split it again: left as
/// is when nothing in it needs quoting; otherwise in quotes, with the backslashes before a quote
/// (and before the closing quote) doubled and every inner quote escaped.
pub fn quote_argument(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// The command line for `program` and `arguments`. The program's name is split by its own rule
/// (quotes only, no escapes), so it is quoted whole when it holds a blank; a path never holds a
/// quote.
pub fn command_line(program: &str, arguments: &[String]) -> String {
    let mut line = if program.contains([' ', '\t']) {
        format!("\"{program}\"")
    } else {
        program.to_owned()
    };
    for argument in arguments {
        line.push(' ');
        line.push_str(&quote_argument(argument));
    }
    line
}

/// A `CREATE_UNICODE_ENVIRONMENT` block: `name=value` strings, each ending in NUL, then one more
/// NUL. Sorted by upper-cased name, as Windows expects of a block it is handed; a later duplicate
/// of a name (in any case) replaces the earlier one.
pub fn environment_block(variables: &[(String, String)]) -> Vec<u16> {
    let mut sorted: Vec<(String, &str, &str)> = Vec::with_capacity(variables.len());
    for (name, value) in variables {
        let key = name.to_uppercase();
        sorted.retain(|(existing, _, _)| *existing != key);
        sorted.push((key, name, value));
    }
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block = Vec::new();
    for (_, name, value) in sorted {
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        // An empty block is still two NULs.
        block.push(0);
    }
    block.push(0);
    block
}

/// Which of a medium child's standard streams are pipes back to the caller; the others are NUL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Piped {
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
}

#[cfg(windows)]
pub use win::{
    integrity_level, is_elevated, process_integrity_level, spawn_medium, token_integrity_level,
    MediumChild,
};

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;
    use std::time::Duration;

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
    };
    use windows::Win32::Security::AppLocker::{
        SaferCloseLevel, SaferComputeTokenFromLevel, SaferCreateLevel,
        SAFER_COMPUTE_TOKEN_FROM_LEVEL_FLAGS, SAFER_LEVELID_NORMALUSER, SAFER_LEVEL_OPEN,
        SAFER_SCOPEID_USER,
    };
    use windows::Win32::Security::{
        CreateWellKnownSid, GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount,
        GetTokenInformation, SetTokenInformation, TokenElevation, TokenIntegrityLevel,
        WinMediumLabelSid, PSID, SAFER_LEVEL_HANDLE, SID_AND_ATTRIBUTES, TOKEN_ELEVATION,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
        InitializeProcThreadAttributeList, OpenProcessToken, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
        CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES,
        STARTUPINFOEXW,
    };

    use super::{command_line, environment_block, level_of_rid, Level, Piped};

    /// `SE_GROUP_INTEGRITY` (winnt.h): the attribute a label SID carries in a token.
    const SE_GROUP_INTEGRITY: u32 = 0x20;
    /// `SECURITY_MAX_SID_SIZE` (68 bytes), in u64s so the buffer is aligned for a SID.
    const SID_BUFFER_WORDS: usize = 68usize.div_ceil(size_of::<u64>());

    /// A handle from a call that returned it open; closed on drop.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: the handle was opened by this module and is not used after this.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// The error of a failed step, named after it (a test failing on the runner says which).
    fn failed(step: &'static str) -> impl FnOnce(windows::core::Error) -> io::Error {
        move |error| {
            let code = error.code().0 as u32;
            // An HRESULT made from a Win32 error keeps the error in its low word.
            let raw = if code & 0xFFFF_0000 == 0x8007_0000 {
                (code & 0xFFFF) as i32
            } else {
                code as i32
            };
            let os = io::Error::from_raw_os_error(raw);
            io::Error::new(os.kind(), format!("{step}: {os}"))
        }
    }

    /// The integrity level of `token` (open with `TOKEN_QUERY`; neither closed nor kept).
    pub fn token_integrity_level(token: HANDLE) -> Option<Level> {
        let mut needed = 0u32;
        // SAFETY: a size query (no buffer, a valid out pointer); it fails by design.
        let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed) };
        if (needed as usize) < size_of::<TOKEN_MANDATORY_LABEL>() {
            return None;
        }
        // u64s keep the buffer aligned for the label's SID pointer.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
        // SAFETY: `buffer` holds at least `needed` bytes and outlives every use of it below.
        unsafe {
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .ok()?;
        // SAFETY: the call filled the aligned buffer with a TOKEN_MANDATORY_LABEL whose SID
        // points into the same buffer.
        let sid = unsafe { &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() }
            .Label
            .Sid;
        // SAFETY: `sid` is a valid SID inside `buffer`, which is alive.
        let count = unsafe { *GetSidSubAuthorityCount(sid) };
        if count == 0 {
            return None;
        }
        // SAFETY: as above; the index is below the SID's sub-authority count.
        let rid = unsafe { *GetSidSubAuthority(sid, u32::from(count) - 1) };
        Some(level_of_rid(rid))
    }

    /// The integrity level of the process behind `process` (a handle with
    /// `PROCESS_QUERY_LIMITED_INFORMATION`); `None` when its token can't be opened.
    pub fn process_integrity_level(process: HANDLE) -> Option<Level> {
        let mut token = HANDLE::default();
        // SAFETY: `token` is a valid out pointer; the handle is closed by `Owned`.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        let token = Owned(token);
        token_integrity_level(token.0)
    }

    /// This process's integrity level.
    pub fn integrity_level() -> Option<Level> {
        // SAFETY: the pseudo-handle of the current process needs no closing.
        process_integrity_level(unsafe { GetCurrentProcess() })
    }

    /// Whether this process runs elevated (a full administrator token); false when that can't
    /// be read.
    pub fn is_elevated() -> bool {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle needs no closing; `token` is closed by `Owned`.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
            return false;
        }
        let token = Owned(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        // SAFETY: `elevation` is a TOKEN_ELEVATION of the size given, alive for the call.
        let read = unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        };
        read.is_ok() && elevation.TokenIsElevated != 0
    }

    /// A child started by [`spawn_medium`]. Dropping it ends the process if it still runs (it
    /// is a test's helper; nothing may outlive the test).
    #[derive(Debug)]
    pub struct MediumChild {
        process: OwnedHandle,
        pid: u32,
        /// The caller's ends of the streams [`Piped`] asked for.
        pub stdin: Option<File>,
        pub stdout: Option<File>,
        pub stderr: Option<File>,
    }

    impl MediumChild {
        pub fn pid(&self) -> u32 {
            self.pid
        }

        fn handle(&self) -> HANDLE {
            HANDLE(self.process.as_raw_handle())
        }

        /// Its exit code once it has ended, waiting at most `within`; `None` while it runs.
        pub fn wait(&self, within: Duration) -> Option<u32> {
            // INFINITE is u32::MAX: a long wait stays just below it.
            let millis = u32::try_from(within.as_millis()).unwrap_or(u32::MAX - 1);
            // SAFETY: the process handle is open for as long as `self` lives.
            if unsafe { WaitForSingleObject(self.handle(), millis) } != WAIT_OBJECT_0 {
                return None;
            }
            let mut code = 0u32;
            // SAFETY: as above; `code` is a valid out pointer.
            unsafe { GetExitCodeProcess(self.handle(), &mut code) }.ok()?;
            Some(code)
        }

        /// Ends the process (exit code 1); false when Windows refused.
        pub fn kill(&self) -> bool {
            // SAFETY: the process handle is open and was created with full access.
            unsafe { TerminateProcess(self.handle(), 1) }.is_ok()
        }

        /// The level the child actually runs at.
        pub fn integrity_level(&self) -> Option<Level> {
            process_integrity_level(self.handle())
        }
    }

    impl Drop for MediumChild {
        fn drop(&mut self) {
            if self.wait(Duration::ZERO).is_none() {
                self.kill();
                let _ = self.wait(Duration::from_secs(5));
            }
        }
    }

    /// Starts `program` with `arguments` at **medium** integrity from an elevated process: the
    /// token of this process reduced to a normal user's (`SaferComputeTokenFromLevel` with
    /// `SAFER_LEVELID_NORMALUSER`: the Administrators group deny-only, the privileges gone),
    /// labelled Medium (the Safer token keeps the caller's High label), then
    /// `CreateProcessAsUserW`. A token derived from the caller's own this way may be assigned
    /// without any privilege.
    ///
    /// `environment` is the child's whole environment (`None`: this process's). The child has no
    /// console window and inherits only its three standard handles.
    pub fn spawn_medium(
        program: &Path,
        arguments: &[String],
        environment: Option<&[(String, String)]>,
        piped: Piped,
    ) -> io::Result<MediumChild> {
        let program = program.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the program's path is not Unicode",
            )
        })?;
        let mut line: Vec<u16> = command_line(program, arguments)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let block = environment.map(environment_block);
        let token = medium_token()?;
        // The child's ends live until this function returns: the child holds its own copies by
        // then, and the caller's reads see a stream end once the child closes them.
        let (stdin_child, stdin) = stream(piped.stdin, true)?;
        let (stdout_child, stdout) = stream(piped.stdout, false)?;
        let (stderr_child, stderr) = stream(piped.stderr, false)?;
        let inherited = [
            HANDLE(stdin_child.as_raw_handle()),
            HANDLE(stdout_child.as_raw_handle()),
            HANDLE(stderr_child.as_raw_handle()),
        ];
        for handle in inherited {
            // SAFETY: each is an open handle this function owns; only its inherit flag changes.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
                .map_err(failed("SetHandleInformation"))?;
        }

        // Only these three are inherited, whatever else this process has marked inheritable.
        let mut list_size = 0usize;
        // SAFETY: a size query; it fails with ERROR_INSUFFICIENT_BUFFER by design.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut list_size) };
        let mut list_buffer = vec![0u64; list_size.div_ceil(size_of::<u64>()).max(1)];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(list_buffer.as_mut_ptr().cast());
        // SAFETY: `list` points at `list_size` writable, aligned bytes that outlive the list.
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut list_size) }
            .map_err(failed("InitializeProcThreadAttributeList"))?;
        // SAFETY: `inherited` outlives the list's use by CreateProcessAsUserW below, and its size
        // is given exactly.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(inherited.as_ptr().cast::<c_void>()),
                size_of::<[HANDLE; 3]>(),
                None,
                None,
            )
        }
        .map_err(failed("UpdateProcThreadAttribute"));

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = inherited[0];
        startup.StartupInfo.hStdOutput = inherited[1];
        startup.StartupInfo.hStdError = inherited[2];
        startup.lpAttributeList = list;
        let mut info = PROCESS_INFORMATION::default();
        let created = updated.and_then(|()| {
            // SAFETY: every pointer is to a live local: the command line is writable and
            // NUL-terminated, the environment block is double-NUL-terminated UTF-16, and the
            // startup info (with its attribute list) outlives the call.
            unsafe {
                CreateProcessAsUserW(
                    Some(token.0),
                    PCWSTR::null(),
                    Some(PWSTR(line.as_mut_ptr())),
                    None,
                    None,
                    true,
                    CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                    block.as_ref().map(|block| block.as_ptr().cast::<c_void>()),
                    PCWSTR::null(),
                    &startup.StartupInfo,
                    &mut info,
                )
            }
            .map_err(failed("CreateProcessAsUserW"))
        });
        // SAFETY: the list was initialised above and is not used after this.
        unsafe { DeleteProcThreadAttributeList(list) };
        created?;
        drop(Owned(info.hThread));
        // SAFETY: the process handle was just returned open, and nothing else owns it.
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess.0) };
        Ok(MediumChild {
            process,
            pid: info.dwProcessId,
            stdin,
            stdout,
            stderr,
        })
    }

    /// The token [`spawn_medium`] starts its child with.
    fn medium_token() -> io::Result<Owned> {
        let mut level = SAFER_LEVEL_HANDLE::default();
        // SAFETY: `level` is a valid out pointer; the level is closed below.
        unsafe {
            SaferCreateLevel(
                SAFER_SCOPEID_USER,
                SAFER_LEVELID_NORMALUSER,
                SAFER_LEVEL_OPEN,
                &mut level,
                None,
            )
        }
        .map_err(failed("SaferCreateLevel"))?;
        let mut token = HANDLE::default();
        // SAFETY: `level` is open; `token` is a valid out pointer, closed by `Owned`.
        let computed = unsafe {
            SaferComputeTokenFromLevel(
                level,
                None,
                &mut token,
                SAFER_COMPUTE_TOKEN_FROM_LEVEL_FLAGS(0),
                None,
            )
        };
        // SAFETY: `level` was opened above and is not used after this.
        let _ = unsafe { SaferCloseLevel(level) };
        computed.map_err(failed("SaferComputeTokenFromLevel"))?;
        let token = Owned(token);

        let mut sid_buffer = [0u64; SID_BUFFER_WORDS];
        let sid = PSID(sid_buffer.as_mut_ptr().cast());
        let mut sid_size = (SID_BUFFER_WORDS * size_of::<u64>()) as u32;
        // SAFETY: `sid` points at `sid_size` writable bytes, aligned, alive until the end.
        unsafe { CreateWellKnownSid(WinMediumLabelSid, None, Some(sid), &mut sid_size) }
            .map_err(failed("CreateWellKnownSid"))?;
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: SE_GROUP_INTEGRITY,
            },
        };
        // SAFETY: `sid` holds the valid SID just written.
        let length = size_of::<TOKEN_MANDATORY_LABEL>() as u32 + unsafe { GetLengthSid(sid) };
        // SAFETY: `label` and the SID it points to are alive for the call. Lowering a token's
        // level needs no privilege.
        unsafe {
            SetTokenInformation(
                token.0,
                TokenIntegrityLevel,
                (&label as *const TOKEN_MANDATORY_LABEL).cast(),
                length,
            )
        }
        .map_err(failed("SetTokenInformation"))?;
        Ok(token)
    }

    /// One standard stream: the handle the child gets and, when piped, the caller's end.
    fn stream(piped: bool, child_reads: bool) -> io::Result<(OwnedHandle, Option<File>)> {
        if !piped {
            let null = OpenOptions::new().read(true).write(true).open("NUL")?;
            return Ok((OwnedHandle::from(null), None));
        }
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        // SAFETY: both are valid out pointers; each end is owned right below.
        unsafe { CreatePipe(&mut read, &mut write, None, 0) }.map_err(failed("CreatePipe"))?;
        // SAFETY: both handles were just returned open and nothing else owns them.
        let (read, write) = unsafe {
            (
                OwnedHandle::from_raw_handle(read.0),
                OwnedHandle::from_raw_handle(write.0),
            )
        };
        Ok(if child_reads {
            (read, Some(File::from(write)))
        } else {
            (write, Some(File::from(read)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_rid_maps_to_its_level() {
        assert_eq!(level_of_rid(0), Level::Untrusted);
        assert_eq!(level_of_rid(LOW_RID - 1), Level::Untrusted);
        assert_eq!(level_of_rid(LOW_RID), Level::Low);
        assert_eq!(level_of_rid(MEDIUM_RID), Level::Medium);
        assert_eq!(level_of_rid(MEDIUM_PLUS_RID), Level::Medium);
        assert_eq!(level_of_rid(HIGH_RID - 1), Level::Medium);
        assert_eq!(level_of_rid(HIGH_RID), Level::High);
        assert_eq!(level_of_rid(SYSTEM_RID), Level::System);
        // SECURITY_MANDATORY_PROTECTED_PROCESS_RID.
        assert_eq!(level_of_rid(0x5000), Level::System);
        assert!(Level::Medium < Level::High);
    }

    #[test]
    fn plain_arguments_are_left_alone() {
        assert_eq!(quote_argument("hook"), "hook");
        assert_eq!(
            quote_argument(r"\\.\pipe\agentnotch-test-1"),
            r"\\.\pipe\agentnotch-test-1"
        );
        assert_eq!(quote_argument(r"C:\a\b\"), r"C:\a\b\");
    }

    #[test]
    fn arguments_with_blanks_or_quotes_are_quoted_as_the_runtime_splits_them() {
        assert_eq!(quote_argument(""), r#""""#);
        assert_eq!(quote_argument("a b"), r#""a b""#);
        assert_eq!(quote_argument(r#"say "hi""#), r#""say \"hi\"""#);
        // Backslashes before a quote, and before the closing quote, are doubled.
        assert_eq!(quote_argument(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote_argument(r"C:\my dir\"), r#""C:\my dir\\""#);
        // Elsewhere they stay single.
        assert_eq!(quote_argument(r"C:\my dir\x"), r#""C:\my dir\x""#);
    }

    #[test]
    fn the_command_line_quotes_the_program_whole() {
        assert_eq!(
            command_line(
                r"C:\Program Files\x\server.exe",
                &[
                    "--pipe".into(),
                    r"\\.\pipe\p".into(),
                    "Bash=deny:not now".into()
                ]
            ),
            r#""C:\Program Files\x\server.exe" --pipe \\.\pipe\p "Bash=deny:not now""#
        );
        assert_eq!(command_line(r"C:\x\hook.exe", &[]), r"C:\x\hook.exe");
    }

    #[test]
    fn the_environment_block_is_sorted_and_double_terminated() {
        let block = environment_block(&[
            ("b".into(), "2".into()),
            ("A".into(), "1".into()),
            ("B".into(), "3".into()),
        ]);
        let text = String::from_utf16(&block).expect("UTF-16");
        assert_eq!(text, "A=1\0B=3\0\0");
        assert_eq!(environment_block(&[]), vec![0, 0]);
    }
}
