//! Processes on Windows (DESIGN-WIN §3.2 `Processes`, §4.2; WP3): Toolhelp32 snapshots with
//! creation times, liveness (`OpenProcess`; ACCESS_DENIED counts as alive), `GetProcessTimes`,
//! the PEB read of `CLAUDE_CONFIG_DIR` (same user only; a partial read is `Unreadable`, never
//! `Unset`), token user and elevation, `QueryFullProcessImageNameW`.
//!
//! The rules that need no system call (what an exit code or an error code says about a process,
//! how a FILETIME becomes a `SystemTime`, what an environment block says about
//! `CLAUDE_CONFIG_DIR`) are plain functions here, compiled and tested on every OS; the Win32
//! calls are in the Windows-only `imp` module below.

use std::time::{Duration, SystemTime};

use agentnotch_engine::platform::{EnvRead, Liveness};

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

/// Which children of `%LOCALAPPDATA%\\Packages` can hold Claude Desktop's Store-package data, in
/// the order to look at them: names containing `claude` or `anthropic` in any case, sorted (by
/// lowercase name, then exactly) so the order never depends on how the disk lists them.
///
/// It is a rule about names, not about processes; it lives here only because this is the one
/// always-compiled file of this crate that the package owns (`paths.rs` compiles on Windows
/// only, and `lib.rs`'s module lines are not this package's to grow), so the rule is unit-tested
/// on every OS.
pub fn desktop_package_names<'a>(children: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut names: Vec<String> = children
        .into_iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower.contains("claude") || lower.contains("anthropic")
        })
        .map(str::to_owned)
        .collect();
    names.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    names
}

/// The variable that names a Claude Code process's config folder.
pub const CONFIG_DIR_VARIABLE: &str = "CLAUDE_CONFIG_DIR";

/// The largest environment block that is read. Windows sets no limit on a block's size; one
/// larger than this is not read at all rather than in part.
pub const MAX_ENVIRONMENT_BYTES: usize = 1024 * 1024;

/// What a process's environment block says about `CLAUDE_CONFIG_DIR` (AU§14.1, the port of
/// `ProcessConfigDir.parse`).
///
/// `block` is the raw block as it lies in the process: UTF-16LE `NAME=value` entries, each ended
/// by a NUL, and an empty entry after the last one. Anything after that empty entry is ignored.
///
/// "Unset" means the process uses `~\.claude`, so it is answered only for a block that was
/// read whole: every entry ended, and the closing empty entry there. A block of odd length, one
/// that stops inside an entry or before the closing entry, or one with no entries at all is
/// `Unreadable`, and so is a block that names the variable but is cut off further on: a read that
/// went wrong somewhere is not trusted anywhere.
///
/// The environment holds secrets. Every other entry is compared unit by unit against the name
/// and skipped without ever becoming a string; only the one value is decoded.
pub fn config_dir_from_environment_block(block: &[u8]) -> EnvRead {
    if !block.len().is_multiple_of(2) {
        return EnvRead::Unreadable;
    }
    let unit = |index: usize| u16::from_le_bytes([block[2 * index], block[2 * index + 1]]);
    let units = block.len() / 2;
    let mut found: Option<EnvRead> = None;
    let mut saw_entry = false;
    let mut start = 0;
    loop {
        let Some(end) = (start..units).find(|&index| unit(index) == 0) else {
            // The block stops inside an entry, or where the closing empty entry should be.
            return EnvRead::Unreadable;
        };
        if end == start {
            break;
        }
        saw_entry = true;
        // The first match decides, as it does for the process itself.
        if found.is_none() {
            found = config_dir_of_entry(start, end, &unit);
        }
        start = end + 1;
    }
    match found {
        Some(read) => read,
        // A process always has some environment (the system's own variables): an empty block
        // is more likely memory that was not the block than a process with nothing set.
        None if saw_entry => EnvRead::Unset,
        None => EnvRead::Unreadable,
    }
}

/// The answer of one entry (units `start..end`, without its NUL), or `None` when the entry is
/// another variable.
fn config_dir_of_entry(start: usize, end: usize, unit: &dyn Fn(usize) -> u16) -> Option<EnvRead> {
    let name = CONFIG_DIR_VARIABLE.as_bytes();
    let value = start + name.len() + 1;
    // An entry that starts with `=` (`=C:=C:\x`, the per-drive current folders) has a name that
    // begins with the sign, so it can never be this one: the name must be followed by the
    // first `=` of the entry.
    if value > end || unit(start + name.len()) != u16::from(b'=') {
        return None;
    }
    // Windows compares variable names without regard to case; the name is ASCII.
    let same_name = name.iter().enumerate().all(|(offset, &expected)| {
        u8::try_from(unit(start + offset)).is_ok_and(|got| got.eq_ignore_ascii_case(&expected))
    });
    if !same_name {
        return None;
    }
    if value == end {
        // Set to nothing is not set, as on the Mac.
        return Some(EnvRead::Unset);
    }
    // A value that is not text is never guessed at (no replacement characters in a path).
    let text: Result<String, _> = char::decode_utf16((value..end).map(unit)).collect();
    Some(text.map_or(EnvRead::Unreadable, EnvRead::Set))
}

/// Overwrites `bytes` with zeros in a way the compiler may not drop, for buffers that held
/// another process's environment.
pub fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    // Without this the fill of a buffer that is about to be freed is a dead store.
    std::hint::black_box(bytes);
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use agentnotch_engine::platform::{EnvRead, Liveness, ProcEntry, ProcessTable, Processes};
    use windows::core::PWSTR;
    use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WIN32_ERROR};
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::SystemInformation::{
        IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_UNKNOWN,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, IsWow64Process2, OpenProcess,
        OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    use super::{
        config_dir_from_environment_block, creation_time, exe_name, liveness_from, wipe, ExitProbe,
        MAX_ENVIRONMENT_BYTES, STILL_ACTIVE,
    };

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

    // Where the environment block is found in an x64 process (DESIGN-WIN §4.2). These are
    // not documented; `tests/win_process.rs` proves them on the Windows the app runs on.
    /// `PEB.ProcessParameters`.
    const PEB_PROCESS_PARAMETERS: usize = 0x20;
    /// `RTL_USER_PROCESS_PARAMETERS.Environment`.
    const PARAMETERS_ENVIRONMENT: usize = 0x80;
    /// `RTL_USER_PROCESS_PARAMETERS.EnvironmentSize`, in bytes.
    const PARAMETERS_ENVIRONMENT_SIZE: usize = 0x3F0;

    /// `PROCESS_BASIC_INFORMATION` on x64 is six pointer-sized fields (the two 32-bit ones are
    /// padded), the second of them the PEB's address. It is read as plain numbers: that address
    /// lies in another process and is never dereferenced here.
    const BASIC_INFORMATION_WORDS: usize = 6;
    const BASIC_INFORMATION_PEB: usize = 1;

    /// A buffer that held another process's environment: zeroed before it is freed, on every
    /// way out of the function that owns it.
    struct Wiped(Vec<u8>);

    impl Drop for Wiped {
        fn drop(&mut self) {
            wipe(&mut self.0);
        }
    }

    /// Whether this is an x64 build on an x64 system, the one layout the offsets above are
    /// for. On an ARM64 host this build runs emulated and other processes are laid out
    /// otherwise.
    fn native_x64() -> bool {
        if !cfg!(target_arch = "x86_64") {
            return false;
        }
        let (mut process, mut native) = <(IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE)>::default();
        // SAFETY: the current process's pseudo-handle is always valid; both out pointers are
        // valid for the call.
        unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) }.is_ok()
            && native == IMAGE_FILE_MACHINE_AMD64
    }

    /// Whether `process` is a native process of this system rather than a 32-bit one run
    /// through WOW64, whose own environment lives behind a second, 32-bit PEB.
    fn is_native(process: &Handle) -> bool {
        let mut machine = IMAGE_FILE_MACHINE::default();
        // SAFETY: `process` is an open handle with a query right; the out pointer is valid for
        // the call, and the optional one is left out.
        unsafe { IsWow64Process2(process.0, &mut machine, None) }.is_ok()
            && machine == IMAGE_FILE_MACHINE_UNKNOWN
    }

    /// Fills `buffer` from `address` in `process`. False unless every byte was copied: a
    /// partial copy (ERROR_PARTIAL_COPY) or a short count is a failed read.
    fn read_memory(process: &Handle, address: usize, buffer: &mut [u8]) -> bool {
        let mut copied = 0usize;
        // SAFETY: `process` is an open handle with PROCESS_VM_READ; `buffer` is valid for
        // writes of the length passed; `address` is only handed to the system, which checks it
        // against the other process's address space; `copied` is a valid out pointer.
        unsafe {
            ReadProcessMemory(
                process.0,
                address as *const c_void,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                Some(&mut copied),
            )
        }
        .is_ok()
            && copied == buffer.len()
    }

    /// One pointer-sized field of the other process.
    fn read_word(process: &Handle, address: usize) -> Option<usize> {
        let mut word = [0u8; size_of::<usize>()];
        read_memory(process, address, &mut word).then(|| usize::from_ne_bytes(word))
    }

    /// Where the process's environment block is and how many bytes it has.
    fn environment_of(process: &Handle, parameters: usize) -> Option<(usize, usize)> {
        Some((
            read_word(process, parameters.checked_add(PARAMETERS_ENVIRONMENT)?)?,
            read_word(
                process,
                parameters.checked_add(PARAMETERS_ENVIRONMENT_SIZE)?,
            )?,
        ))
    }

    /// The PEB read (DESIGN-WIN §4.2). `limit` is the test hook of
    /// `WinProcesses::reading_at_most`.
    fn read_config_dir(pid: u32, limit: Option<usize>) -> EnvRead {
        if !native_x64() {
            return EnvRead::Unreadable;
        }
        // Reading memory needs more than `open` asks for, and is refused for elevated targets
        // when this process is not elevated.
        // SAFETY: no pointers are passed; the returned handle is owned by the guard.
        let Ok(process) =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
                .map(Handle)
        else {
            return EnvRead::Unreadable;
        };
        // A process object outlives its process while anyone holds a handle to it.
        if exit_code(&process) != Some(STILL_ACTIVE) || !is_native(&process) {
            return EnvRead::Unreadable;
        }

        let mut basic = [0usize; BASIC_INFORMATION_WORDS];
        let mut returned = 0u32;
        // SAFETY: `process` is an open handle with PROCESS_QUERY_INFORMATION; `basic` is as
        // large and as aligned as the structure the call fills for this class on x64, and is
        // valid for writes of the size passed; `returned` is a valid out pointer.
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                ProcessBasicInformation,
                basic.as_mut_ptr().cast(),
                size_of_val(&basic) as u32,
                &mut returned,
            )
        };
        let peb = basic[BASIC_INFORMATION_PEB];
        if status.0 != 0 || peb == 0 {
            return EnvRead::Unreadable;
        }

        let block = || -> Option<Wiped> {
            let parameters = read_word(&process, peb.checked_add(PEB_PROCESS_PARAMETERS)?)?;
            if parameters == 0 {
                return None;
            }
            let (address, size) = environment_of(&process, parameters)?;
            if address == 0 || size == 0 || size > MAX_ENVIRONMENT_BYTES {
                return None;
            }
            let mut block = Wiped(vec![0u8; size]);
            // The whole block in one read, tried twice: the process may be replacing its
            // environment at this moment.
            if !read_memory(&process, address, &mut block.0)
                && !read_memory(&process, address, &mut block.0)
            {
                return None;
            }
            // The block was replaced or resized while it was read: what was copied may be
            // neither the old nor the new one.
            if environment_of(&process, parameters) != Some((address, size)) {
                return None;
            }
            Some(block)
        };
        let Some(block) = block() else {
            return EnvRead::Unreadable;
        };
        let read = limit.map_or(block.0.len(), |limit| limit.min(block.0.len()));
        config_dir_from_environment_block(&block.0[..read])
    }

    #[derive(Debug, Default)]
    pub struct WinProcesses {
        /// Test hook: see `reading_at_most`.
        environment_limit: Option<usize>,
    }

    impl WinProcesses {
        pub fn new() -> Self {
            WinProcesses::default()
        }

        /// For tests only: process services whose environment read stops after `bytes` bytes,
        /// as a read cut short would, so a test can prove that such a read is never taken for
        /// "unset".
        #[doc(hidden)]
        pub fn reading_at_most(bytes: usize) -> Self {
            WinProcesses {
                environment_limit: Some(bytes),
            }
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

        /// Only the one value leaves this function: the environment holds secrets.
        fn config_dir_env(&self, pid: u32) -> EnvRead {
            // Same user only, as on the Mac; "can't tell" is not "same".
            if pid == 0 || self.same_user(pid) != Some(true) {
                return EnvRead::Unreadable;
            }
            read_config_dir(pid, self.environment_limit)
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

    #[test]
    fn store_package_names_are_picked_by_claude_or_anthropic_and_sorted() {
        let listed = [
            "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
            "Claude_pzs8sxrjxfjjc",
            "AnthropicPBC.Claude_x",
            "Microsoft.Windows.Photos_8wekyb3d8bbwe",
            "ANTHROPIC.Something",
            "claude_lower",
        ];
        assert_eq!(
            desktop_package_names(listed),
            [
                "ANTHROPIC.Something",
                "AnthropicPBC.Claude_x",
                "claude_lower",
                "Claude_pzs8sxrjxfjjc"
            ]
        );
        // The same names in another order give the same answer.
        let mut reversed = listed;
        reversed.reverse();
        assert_eq!(
            desktop_package_names(reversed),
            desktop_package_names(listed)
        );
        assert!(desktop_package_names([]).is_empty());
        assert!(desktop_package_names(["Microsoft.WindowsTerminal_x"]).is_empty());
    }

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

    /// An environment block as it lies in a process: each entry and its NUL, then the closing
    /// empty entry, in UTF-16LE.
    fn block(entries: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in entries {
            bytes.extend(units(entry));
            bytes.extend([0, 0]);
        }
        bytes.extend([0, 0]);
        bytes
    }

    fn units(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn set(value: &str) -> EnvRead {
        EnvRead::Set(value.to_owned())
    }

    const OTHERS: [&str; 3] = [
        r"ALLUSERSPROFILE=C:\ProgramData",
        "ANTHROPIC_API_KEY=not-a-real-key",
        r"SystemRoot=C:\WINDOWS",
    ];

    // ProcessConfigDir.parse: the value, verbatim, wherever the entry stands.
    #[test]
    fn the_block_names_the_config_dir() {
        let value = r"C:\Users\me\.claude-work";
        let entry = format!("CLAUDE_CONFIG_DIR={value}");
        assert_eq!(
            config_dir_from_environment_block(&block(&[OTHERS[0], &entry, OTHERS[2]])),
            set(value)
        );
        assert_eq!(
            config_dir_from_environment_block(&block(&[&entry])),
            set(value)
        );
        assert_eq!(
            config_dir_from_environment_block(&block(&[OTHERS[0], OTHERS[1], OTHERS[2], &entry])),
            set(value)
        );
        // A value may hold `=` and spaces; it is not trimmed.
        assert_eq!(
            config_dir_from_environment_block(&block(&["CLAUDE_CONFIG_DIR= D:\\a=b c "])),
            set(" D:\\a=b c ")
        );
        // The first entry decides.
        assert_eq!(
            config_dir_from_environment_block(&block(&[
                "CLAUDE_CONFIG_DIR=C:\\one",
                "CLAUDE_CONFIG_DIR=C:\\two"
            ])),
            set("C:\\one")
        );
        // What follows the closing entry is not the block.
        let mut with_slack = block(&[&entry]);
        with_slack.extend(units("CLAUDE_CONFIG_DIR=C:\\stale"));
        assert_eq!(config_dir_from_environment_block(&with_slack), set(value));
    }

    #[test]
    fn a_whole_block_without_the_name_is_unset() {
        assert_eq!(
            config_dir_from_environment_block(&block(&OTHERS)),
            EnvRead::Unset
        );
        assert_eq!(
            config_dir_from_environment_block(&block(&["A=1"])),
            EnvRead::Unset
        );
    }

    #[test]
    fn the_name_is_matched_without_regard_to_case() {
        for name in [
            "Claude_Config_Dir",
            "claude_config_dir",
            "CLAUDE_config_DIR",
        ] {
            assert_eq!(
                config_dir_from_environment_block(&block(&[
                    OTHERS[0],
                    &format!("{name}=D:\\profiles\\a")
                ])),
                set("D:\\profiles\\a"),
                "{name}"
            );
        }
        // ASCII case only: a name with U+0131 (dotless i) or a trailing U+017F (long s) is
        // another name, and so is one whose unit merely ends in the same byte (U+0143 for `C`).
        for name in [
            "CLAUDE_CONFIG_DIR\u{17f}",
            "\u{143}LAUDE_CONFIG_DIR",
            "CLAUDE_CONFIG_D\u{131}R",
        ] {
            assert_eq!(
                config_dir_from_environment_block(&block(&[&format!("{name}=D:\\x")])),
                EnvRead::Unset,
                "{name}"
            );
        }
    }

    // ProcessConfigDir.parse: `text.isEmpty ? .unset`.
    #[test]
    fn an_empty_value_is_unset() {
        assert_eq!(
            config_dir_from_environment_block(&block(&[OTHERS[0], "CLAUDE_CONFIG_DIR="])),
            EnvRead::Unset
        );
        // And it is the entry that decides, not a later one.
        assert_eq!(
            config_dir_from_environment_block(&block(&[
                "CLAUDE_CONFIG_DIR=",
                "CLAUDE_CONFIG_DIR=C:\\two"
            ])),
            EnvRead::Unset
        );
    }

    #[test]
    fn a_name_that_only_starts_or_ends_with_it_is_another_variable() {
        for entry in [
            "CLAUDE_CONFIG_DIR_X=C:\\x",
            "CLAUDE_CONFIG_DIRS=C:\\x",
            "XCLAUDE_CONFIG_DIR=C:\\x",
            "CLAUDE_CONFIG_DI=C:\\x",
            "CLAUDE_CONFIG=CLAUDE_CONFIG_DIR=C:\\x",
            // No `=` at all.
            "CLAUDE_CONFIG_DIR",
            "=CLAUDE_CONFIG_DIR=C:\\x",
        ] {
            assert_eq!(
                config_dir_from_environment_block(&block(&[entry])),
                EnvRead::Unset,
                "{entry}"
            );
        }
    }

    // cmd.exe keeps each drive's current folder in the block as `=C:=C:\x`. They are entries
    // like any other, not the end of the block.
    #[test]
    fn per_drive_entries_are_skipped() {
        assert_eq!(
            config_dir_from_environment_block(&block(&[
                "=::=::\\",
                "=C:=C:\\x",
                "=ExitCode=00000000",
                "CLAUDE_CONFIG_DIR=C:\\Users\\me\\.claude-b",
            ])),
            set("C:\\Users\\me\\.claude-b")
        );
        assert_eq!(
            config_dir_from_environment_block(&block(&["=C:=C:\\x", OTHERS[0]])),
            EnvRead::Unset
        );
    }

    #[test]
    fn a_block_without_its_closing_entry_is_unreadable() {
        let whole = block(&OTHERS);
        // Without the closing empty entry, and then without the last entry's own NUL.
        assert_eq!(
            config_dir_from_environment_block(&whole[..whole.len() - 2]),
            EnvRead::Unreadable
        );
        assert_eq!(
            config_dir_from_environment_block(&whole[..whole.len() - 4]),
            EnvRead::Unreadable
        );
        // Every shorter read of a block that does not name the variable: never "unset".
        for length in (0..whole.len()).step_by(2) {
            assert_eq!(
                config_dir_from_environment_block(&whole[..length]),
                EnvRead::Unreadable,
                "{length} of {} bytes",
                whole.len()
            );
        }
        // Naming the variable does not make a cut-off block trusted.
        let named = block(&["CLAUDE_CONFIG_DIR=C:\\a", OTHERS[2]]);
        for length in (0..named.len()).step_by(2) {
            assert_eq!(
                config_dir_from_environment_block(&named[..length]),
                EnvRead::Unreadable,
                "{length} of {} bytes",
                named.len()
            );
        }
        assert_eq!(config_dir_from_environment_block(&named), set("C:\\a"));
    }

    #[test]
    fn a_block_of_odd_length_is_unreadable() {
        for entries in [&OTHERS[..], &["CLAUDE_CONFIG_DIR=C:\\a"][..]] {
            let mut odd = block(entries);
            odd.push(0);
            assert_eq!(config_dir_from_environment_block(&odd), EnvRead::Unreadable);
            odd.truncate(odd.len() - 2);
            assert_eq!(config_dir_from_environment_block(&odd), EnvRead::Unreadable);
        }
        assert_eq!(config_dir_from_environment_block(&[0]), EnvRead::Unreadable);
    }

    #[test]
    fn a_block_cut_inside_the_wanted_entry_is_unreadable() {
        let whole = block(&[OTHERS[0], "CLAUDE_CONFIG_DIR=C:\\Users\\me\\.claude-work"]);
        let entry_start = units(OTHERS[0]).len() + 2;
        // In the name, right after the `=`, in the value, and at the value's end without its
        // NUL: a shortened value would name another folder.
        for cut in [10, 36, 50, whole.len() - 4 - entry_start] {
            assert_eq!(
                config_dir_from_environment_block(&whole[..entry_start + cut]),
                EnvRead::Unreadable,
                "cut {cut} bytes into the entry"
            );
        }
    }

    // ProcessConfigDir.parse has no environment at all as "can't tell"; so is memory that
    // holds only zeros.
    #[test]
    fn a_block_with_no_entries_is_unreadable() {
        assert_eq!(config_dir_from_environment_block(&[]), EnvRead::Unreadable);
        assert_eq!(
            config_dir_from_environment_block(&block(&[])),
            EnvRead::Unreadable
        );
        assert_eq!(
            config_dir_from_environment_block(&[0; 64]),
            EnvRead::Unreadable
        );
    }

    #[test]
    fn a_value_outside_ascii_is_kept() {
        // Two-unit characters too (U+1F4C1 is a surrogate pair).
        let value = "C:\\Users\\Zo\u{eb}\\.claude-\u{65e5}\u{672c}-\u{1f4c1}";
        assert_eq!(
            config_dir_from_environment_block(&block(&[
                OTHERS[0],
                &format!("CLAUDE_CONFIG_DIR={value}"),
                OTHERS[2]
            ])),
            set(value)
        );
    }

    #[test]
    fn a_value_that_is_not_utf16_is_unreadable() {
        for lone in [0xD800u16, 0xDC00] {
            let mut bytes = units("CLAUDE_CONFIG_DIR=C:\\a");
            bytes.extend(lone.to_le_bytes());
            bytes.extend(units("b"));
            bytes.extend([0, 0, 0, 0]);
            assert_eq!(
                config_dir_from_environment_block(&bytes),
                EnvRead::Unreadable
            );
        }
        // Another variable's broken text is never decoded, so it changes nothing.
        let mut bytes = units("OTHER=");
        bytes.extend(0xD800u16.to_le_bytes());
        bytes.extend([0, 0]);
        bytes.extend(block(&["CLAUDE_CONFIG_DIR=C:\\a"]));
        assert_eq!(config_dir_from_environment_block(&bytes), set("C:\\a"));
    }

    #[test]
    fn a_wiped_buffer_holds_only_zeros() {
        let mut bytes = block(&OTHERS);
        let length = bytes.len();
        wipe(&mut bytes);
        assert_eq!(bytes, vec![0; length]);
    }
}
