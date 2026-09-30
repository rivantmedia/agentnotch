//! Processes on Windows (DESIGN-WIN §3.2 `Processes`, §4.2; WP3): Toolhelp32 snapshots with
//! creation times, liveness (`OpenProcess`; ACCESS_DENIED counts as alive), `GetProcessTimes`,
//! the PEB read of `CLAUDE_CONFIG_DIR` (same user only; a partial read is `Unreadable`, never
//! `Unset`), token user and elevation, `QueryFullProcessImageNameW`.
//!
//! The rules that need no system call (what an exit code or an error code says about a process,
//! how a FILETIME becomes a `SystemTime`) are plain functions here, compiled and tested on every
//! OS; the Win32 calls are in the Windows-only `imp` module below.

use std::time::{Duration, SystemTime};

use agentnotch_engine::platform::Liveness;

#[cfg(windows)]
pub use imp::WinProcesses;

/// 1601-01-01 to 1970-01-01 in FILETIME ticks (100 ns).
pub const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
const TICKS_PER_SECOND: u64 = 10_000_000;

/// `GetExitCodeProcess`'s answer for a process that has not exited.
pub const STILL_ACTIVE: u32 = 259;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_PARAMETER: u32 = 87;

/// A FILETIME (100 ns ticks since 1601, UTC) as a `SystemTime`. Dates before 1970 are kept as
/// they are; `None` only when the platform's `SystemTime` can't hold the value.
pub fn filetime_to_system_time(ticks: u64) -> Option<SystemTime> {
    let since_epoch = |ticks: u64| {
        Duration::new(
            ticks / TICKS_PER_SECOND,
            // Below 10^9 by construction: fewer than 10^7 ticks of 100 ns.
            (ticks % TICKS_PER_SECOND) as u32 * 100,
        )
    };
    if ticks >= FILETIME_UNIX_EPOCH {
        SystemTime::UNIX_EPOCH.checked_add(since_epoch(ticks - FILETIME_UNIX_EPOCH))
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(since_epoch(FILETIME_UNIX_EPOCH - ticks))
    }
}

/// A process's creation time from `GetProcessTimes`. Zero is what the system reports for
/// processes that have none (the idle process): that is "unknown", not the year 1601, which
/// would make every such process a valid parent of everything.
pub fn creation_time(ticks: u64) -> Option<SystemTime> {
    if ticks == 0 {
        return None;
    }
    filetime_to_system_time(ticks)
}

/// What asking the system for a process's exit code came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitProbe {
    /// `GetExitCodeProcess` answered.
    ExitCode(u32),
    /// `OpenProcess` failed with this Win32 error (0 when the error was not a Win32 one).
    OpenFailed(u32),
    /// The process opened but its exit code could not be read.
    QueryFailed,
}

/// The liveness rule (AU§4.5). A process that can't be opened for lack of rights exists, so it
/// is alive (the Mac's `EPERM`); a pid the system doesn't know is gone. Pid 0 is the idle
/// process, never a process of ours: the system refuses it with the same error as an unknown
/// pid, and "gone" there would be a claim nobody checked.
pub fn liveness_from(pid: u32, probe: ExitProbe) -> Liveness {
    if pid == 0 {
        return Liveness::Unknown;
    }
    match probe {
        ExitProbe::ExitCode(STILL_ACTIVE) => Liveness::Alive,
        ExitProbe::ExitCode(_) => Liveness::Gone,
        ExitProbe::OpenFailed(ERROR_ACCESS_DENIED) => Liveness::Alive,
        ExitProbe::OpenFailed(ERROR_INVALID_PARAMETER) => Liveness::Gone,
        ExitProbe::OpenFailed(_) | ExitProbe::QueryFailed => Liveness::Unknown,
    }
}

/// The image name of a Toolhelp entry: the UTF-16 text up to the first NUL.
pub fn exe_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|&unit| unit == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

#[cfg(windows)]
mod imp {
    use std::mem::size_of;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use agentnotch_engine::platform::{EnvRead, Liveness, ProcEntry, ProcessTable, Processes};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WIN32_ERROR};
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
        QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::{creation_time, exe_name, liveness_from, ExitProbe, STILL_ACTIVE};

    /// The longest path `QueryFullProcessImageNameW` can return, in UTF-16 units.
    const MAX_LONG_PATH: usize = 32_768;

    /// A kernel handle this module opened, closed when dropped.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle was returned open by the call that made this guard, is owned by
            // it alone and is not used after this.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// The process, opened with the one right every call here needs. It is granted for other
    /// users' and elevated processes too, which is why nothing here asks for more.
    fn open(pid: u32) -> windows::core::Result<Handle> {
        // SAFETY: no pointers are passed; the returned handle is owned by the guard.
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map(Handle)
    }

    fn exit_code(process: &Handle) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: `process` is an open handle with the query right; `code` is a valid out pointer.
        unsafe { GetExitCodeProcess(process.0, &mut code) }.ok()?;
        Some(code)
    }

    fn created(process: &Handle) -> Option<SystemTime> {
        let mut creation = FILETIME::default();
        let (mut exit, mut kernel, mut user) = <(FILETIME, FILETIME, FILETIME)>::default();
        // SAFETY: `process` is an open handle with the query right; the four out pointers are
        // valid for the call, which needs all of them.
        unsafe { GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user) }
            .ok()?;
        creation_time(u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime))
    }

    fn token_of(process: HANDLE) -> Option<Handle> {
        let mut token = HANDLE::default();
        // SAFETY: `process` is an open process handle (or the current process's pseudo-handle);
        // `token` is a valid out pointer, and the opened token is owned by the guard.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        Some(Handle(token))
    }

    /// The token's TOKEN_USER, in a buffer aligned for it (the SID it points to lies inside).
    fn token_user(token: &Handle) -> Option<Vec<u64>> {
        let mut needed = 0u32;
        // SAFETY: a size query: no buffer, a valid out pointer for the length. It fails with
        // ERROR_INSUFFICIENT_BUFFER by design, which is why its result is ignored.
        let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
        if (needed as usize) < size_of::<TOKEN_USER>() {
            return None;
        }
        // u64 elements keep the buffer aligned for TOKEN_USER's pointer field.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
        // SAFETY: `buffer` holds at least `needed` bytes and is valid for writes of that length.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .ok()?;
        Some(buffer)
    }

    #[derive(Debug, Default)]
    pub struct WinProcesses;

    impl WinProcesses {
        pub fn new() -> Self {
            WinProcesses
        }
    }

    impl Processes for WinProcesses {
        fn liveness(&self, pid: u32) -> Liveness {
            if pid == 0 {
                return liveness_from(pid, ExitProbe::QueryFailed);
            }
            let probe = match open(pid) {
                Ok(process) => {
                    exit_code(&process).map_or(ExitProbe::QueryFailed, ExitProbe::ExitCode)
                }
                Err(error) => {
                    ExitProbe::OpenFailed(WIN32_ERROR::from_error(&error).map_or(0, |code| code.0))
                }
            };
            liveness_from(pid, probe)
        }

        /// `None` for a process that has exited, like the Mac's `startDate`: a process object
        /// outlives its process while anyone holds a handle to it, and a start time would say
        /// the pid is still that process.
        fn start_time(&self, pid: u32) -> Option<SystemTime> {
            let process = open(pid).ok()?;
            if exit_code(&process)? != STILL_ACTIVE {
                return None;
            }
            created(&process)
        }

        fn table(&self) -> ProcessTable {
            let mut entries = Vec::new();
            // SAFETY: no pointers are passed; the snapshot handle is owned by the guard.
            let Ok(snapshot) =
                unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map(Handle)
            else {
                return ProcessTable { entries };
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            // SAFETY: `snapshot` is an open snapshot handle; `entry` is a valid PROCESSENTRY32W
            // whose `dwSize` is set, as the call requires.
            let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
            while more {
                let pid = entry.th32ProcessID;
                entries.push(ProcEntry {
                    pid,
                    ppid: entry.th32ParentProcessID,
                    exe_name: exe_name(&entry.szExeFile),
                    // Without a start time the entry's parent link is not trusted
                    // (`ProcessTable::parent`), which is right for a process we can't open.
                    started: open(pid).ok().and_then(|process| created(&process)),
                });
                // SAFETY: as for Process32FirstW above; `entry` is still a valid, sized struct.
                more = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
            }
            ProcessTable { entries }
        }

        fn config_dir_env(&self, _pid: u32) -> EnvRead {
            // The PEB read is the next sub-task's. Until then "unreadable", which the engine
            // never takes for "unset" (that would mean `~\.claude` and pick the wrong account).
            EnvRead::Unreadable
        }

        fn same_user(&self, pid: u32) -> Option<bool> {
            let process = open(pid).ok()?;
            let theirs = token_user(&token_of(process.0)?)?;
            // SAFETY: the current process's pseudo-handle is always valid and needs no closing.
            let ours = token_user(&token_of(unsafe { GetCurrentProcess() })?)?;
            // SAFETY: `token_user` returned each buffer filled with a TOKEN_USER and aligned for
            // it; both buffers live to the end of this function.
            let (theirs, ours) = unsafe {
                (
                    &*theirs.as_ptr().cast::<TOKEN_USER>(),
                    &*ours.as_ptr().cast::<TOKEN_USER>(),
                )
            };
            // SAFETY: both SIDs are valid: each points into its own live buffer, filled by the
            // system. The call fails exactly when they differ.
            Some(unsafe { EqualSid(theirs.User.Sid, ours.User.Sid) }.is_ok())
        }

        fn elevated(&self, pid: u32) -> Option<bool> {
            let process = open(pid).ok()?;
            let token = token_of(process.0)?;
            let mut elevation = TOKEN_ELEVATION::default();
            let mut written = 0u32;
            // SAFETY: `elevation` is a TOKEN_ELEVATION, valid for writes of the size passed;
            // `written` is a valid out pointer.
            unsafe {
                GetTokenInformation(
                    token.0,
                    TokenElevation,
                    Some((&raw mut elevation).cast()),
                    size_of::<TOKEN_ELEVATION>() as u32,
                    &mut written,
                )
            }
            .ok()?;
            Some(elevation.TokenIsElevated != 0)
        }

        fn exe_path(&self, pid: u32) -> Option<PathBuf> {
            let process = open(pid).ok()?;
            let mut buffer = vec![0u16; MAX_LONG_PATH];
            let mut length = buffer.len() as u32;
            // SAFETY: `buffer` is valid for writes of `length` UTF-16 units, the size passed in;
            // on success `length` is the number of units written, without the terminator.
            unsafe {
                QueryFullProcessImageNameW(
                    process.0,
                    PROCESS_NAME_WIN32,
                    PWSTR(buffer.as_mut_ptr()),
                    &mut length,
                )
            }
            .ok()?;
            let path = buffer.get(..length as usize)?;
            if path.is_empty() {
                return None;
            }
            Some(PathBuf::from(String::from_utf16(path).ok()?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(seconds: u64, nanos: u32) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos)
    }

    // AU§14.2: Unix seconds = (ft − 116444736000000000) / 10^7.
    #[test]
    fn a_filetime_counts_from_1601() {
        assert_eq!(
            filetime_to_system_time(FILETIME_UNIX_EPOCH),
            Some(SystemTime::UNIX_EPOCH)
        );
        // 2023-01-01T00:00:00Z = 1672531200 s, which is FILETIME 133170048000000000.
        assert_eq!(
            filetime_to_system_time(133_170_048_000_000_000),
            Some(unix(1_672_531_200, 0))
        );
        // One tick is 100 ns, and the fraction is kept.
        assert_eq!(
            filetime_to_system_time(FILETIME_UNIX_EPOCH + 1),
            Some(unix(0, 100))
        );
        assert_eq!(
            filetime_to_system_time(133_170_048_000_000_000 + 1_234_567),
            Some(unix(1_672_531_200, 123_456_700))
        );
    }

    #[test]
    fn a_filetime_before_1970_is_before_the_unix_epoch() {
        // One second and one tick before 1970.
        assert_eq!(
            filetime_to_system_time(FILETIME_UNIX_EPOCH - 10_000_001),
            Some(SystemTime::UNIX_EPOCH - Duration::new(1, 100))
        );
        // 1601-01-01 itself: 11644473600 s before 1970.
        assert_eq!(
            filetime_to_system_time(0),
            Some(SystemTime::UNIX_EPOCH - Duration::from_secs(11_644_473_600))
        );
    }

    #[test]
    fn a_creation_time_of_zero_is_unknown() {
        assert_eq!(creation_time(0), None);
        assert_eq!(
            creation_time(FILETIME_UNIX_EPOCH),
            Some(SystemTime::UNIX_EPOCH)
        );
        assert_eq!(creation_time(1), filetime_to_system_time(1));
    }

    // AU§4.5 (the Mac's `kill(pid, 0) == 0 || errno == EPERM`).
    #[test]
    fn liveness_follows_the_exit_code_and_the_open_error() {
        assert_eq!(
            liveness_from(42, ExitProbe::ExitCode(STILL_ACTIVE)),
            Liveness::Alive
        );
        assert_eq!(liveness_from(42, ExitProbe::ExitCode(0)), Liveness::Gone);
        assert_eq!(liveness_from(42, ExitProbe::ExitCode(1)), Liveness::Gone);
        assert_eq!(
            liveness_from(42, ExitProbe::ExitCode(0xC000_013A)),
            Liveness::Gone
        );
        assert_eq!(
            liveness_from(42, ExitProbe::OpenFailed(ERROR_ACCESS_DENIED)),
            Liveness::Alive
        );
        assert_eq!(
            liveness_from(42, ExitProbe::OpenFailed(ERROR_INVALID_PARAMETER)),
            Liveness::Gone
        );
        // Anything else says nothing either way: 0 (not a Win32 error), ERROR_INVALID_HANDLE,
        // ERROR_NOT_ENOUGH_MEMORY.
        for code in [0, 6, 8] {
            assert_eq!(
                liveness_from(42, ExitProbe::OpenFailed(code)),
                Liveness::Unknown
            );
        }
        assert_eq!(liveness_from(42, ExitProbe::QueryFailed), Liveness::Unknown);
    }

    #[test]
    fn pid_zero_is_never_alive_or_gone() {
        for probe in [
            ExitProbe::ExitCode(STILL_ACTIVE),
            ExitProbe::ExitCode(0),
            ExitProbe::OpenFailed(ERROR_ACCESS_DENIED),
            ExitProbe::OpenFailed(ERROR_INVALID_PARAMETER),
            ExitProbe::QueryFailed,
        ] {
            assert_eq!(liveness_from(0, probe), Liveness::Unknown);
        }
    }

    #[test]
    fn an_image_name_ends_at_the_first_nul() {
        let mut raw: Vec<u16> = "claude.exe".encode_utf16().collect();
        raw.extend([0, u16::from(b'x'), 0]);
        assert_eq!(exe_name(&raw), "claude.exe");
        assert_eq!(exe_name(&[0; 4]), "");
        // A full buffer has no terminator.
        let full: Vec<u16> = "cmd.exe".encode_utf16().collect();
        assert_eq!(exe_name(&full), "cmd.exe");
    }
}
