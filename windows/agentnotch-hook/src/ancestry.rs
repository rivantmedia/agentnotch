//! Which process is Claude Code, when `CLAUDE_PID` is not in the environment (Claude Code before
//! 2.1.214). DESIGN-WIN §1.4.
//!
//! In exec form Claude Code spawns the exe itself, so its parent is Claude Code. In string form
//! the parent is the shell that ran the command (`bash.exe`, `powershell.exe`), so the chain is
//! followed past shells, and the first other ancestor counts only when it is `claude.exe`,
//! `node.exe` or `bun.exe`. Anything else, or any link that can't be checked, means "unknown":
//! the engine then keys the session by its id and never runs a liveness check on a wrong pid.
//!
//! This module only collects the chain (pid, image, creation time per process); the rule itself
//! is `agentnotch_proto::pid_guess`, tested on every OS.

/// Claude Code's pid as far as this process's ancestry shows it.
#[cfg(windows)]
pub fn claude_pid(exec_form: bool) -> Option<u32> {
    use crate::win;
    use agentnotch_proto::{pid_guess, ProcLink};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};

    /// `pid_guess` follows at most this many links.
    const MAX_LINKS: usize = 4;

    // SAFETY: both return values about the calling process and take no arguments; the
    // pseudo-handle needs no closing.
    let (own_process, own_pid) = unsafe { (GetCurrentProcess(), GetCurrentProcessId()) };
    let me = ProcLink {
        pid: own_pid,
        image: String::new(),
        created: win::created(own_process)?,
    };

    let mut ancestors = Vec::new();
    let mut next = windows_impl::parent_of(own_process)?;
    let links = if exec_form { 1 } else { MAX_LINKS };
    while ancestors.len() < links {
        // A parent that has exited can't be opened, or its pid names another process by now; the
        // creation time, compared by `pid_guess`, tells which.
        let Some(process) = win::open_process(next) else {
            break;
        };
        let Some(created) = win::created(process.raw()) else {
            break;
        };
        ancestors.push(ProcLink {
            pid: next,
            image: windows_impl::image_of(process.raw()).unwrap_or_default(),
            created,
        });
        match windows_impl::parent_of(process.raw()) {
            Some(parent) if parent != 0 && parent != next => next = parent,
            _ => break,
        }
    }
    pid_guess(exec_form, &me, &ancestors)
}

/// Other systems have no hook pipe; the pid is never needed.
#[cfg(not(windows))]
pub fn claude_pid(_exec_form: bool) -> Option<u32> {
    None
}

#[cfg(windows)]
mod windows_impl {
    use crate::win;
    use std::ffi::c_void;
    use std::ptr::{addr_of_mut, null_mut};
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE};
    use windows_sys::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    /// `PROCESS_BASIC_INFORMATION`, written out here: windows-sys only defines it behind a
    /// feature that pulls in the whole kernel module for one struct.
    #[repr(C)]
    struct BasicInformation {
        exit_status: i32,
        peb_base_address: *mut c_void,
        affinity_mask: usize,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    /// The pid a process was created by. It may have exited since, and its pid may have been
    /// given to another process: a link is only trusted after the creation times are compared.
    pub fn parent_of(process: HANDLE) -> Option<u32> {
        let mut info = BasicInformation {
            exit_status: 0,
            peb_base_address: null_mut(),
            affinity_mask: 0,
            base_priority: 0,
            unique_process_id: 0,
            inherited_from_unique_process_id: 0,
        };
        let mut returned = 0u32;
        // SAFETY: `process` is a live handle with query access; `info` is a valid, correctly
        // laid out buffer of the size passed.
        let status = unsafe {
            NtQueryInformationProcess(
                process,
                ProcessBasicInformation,
                addr_of_mut!(info).cast::<c_void>(),
                size_of::<BasicInformation>() as u32,
                &mut returned,
            )
        };
        (status >= 0)
            .then(|| u32::try_from(info.inherited_from_unique_process_id).ok())
            .flatten()
    }

    /// The full path of a process's image.
    pub fn image_of(process: HANDLE) -> Option<String> {
        // A path is rarely longer than this; the second size holds any path Windows allows.
        for capacity in [520usize, 32_768] {
            let mut buffer = vec![0u16; capacity];
            let mut len = capacity as u32;
            // SAFETY: `buffer` holds `len` units; `len` receives the length without the NUL.
            let ok = unsafe {
                QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    buffer.as_mut_ptr(),
                    &mut len,
                )
            };
            if ok != 0 {
                return Some(String::from_utf16_lossy(&buffer[..len as usize]));
            }
            if win::last_error() != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
        }
        None
    }
}
