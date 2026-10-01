//! The Job object the status line wrapper runs the previous command in (DESIGN-WIN §4.3; HS§2).
//!
//! The Mac wrapper gives the previous command a process group of its own, so a timeout or a
//! cancel stops everything the command started and not just its shell. Windows has no process
//! groups to signal; a Job object does the same work, and better:
//!
//! - every process the command starts lands in the job, so `TerminateJobObject` ends the whole
//!   tree at the timeout;
//! - the job is made with `KILL_ON_JOB_CLOSE`, and the wrapper holds its only handle. Claude Code
//!   cancels a status line run with `TerminateProcess`, which runs no handler in the wrapper; the
//!   handle closes with the process all the same, and that ends the tree.
//!
//! The same closing handle ends what the command left running when the wrapper exits normally.
//! That is wanted here: such a process would hold the pipes Claude Code reads the status line
//! from, and the render would not finish until it did.

use std::process::{Child, ExitStatus};
use std::time::Duration;

#[cfg(windows)]
pub use windows_impl::{wait_exit, Job};

#[cfg(windows)]
mod windows_impl {
    use super::{Child, Duration, ExitStatus};
    use crate::win::Handle;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{addr_of, null};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    /// What a process ended by the job exits with; nobody reads it.
    const ENDED_BY_JOB: u32 = 1;

    /// A job whose processes end when this is dropped (or the wrapper is gone).
    pub struct Job(Handle);

    impl Job {
        /// An empty, unnamed job that kills its processes when its last handle closes. The
        /// handle is not inheritable, so the command's own processes never hold the job open.
        pub fn new() -> Option<Job> {
            // SAFETY: no security attributes and no name; the returned handle (null on failure)
            // is owned by `Handle`.
            let job = Handle::new(unsafe { CreateJobObjectW(null(), null()) })?;
            // SAFETY: the structure is plain integers, for which all zeroes means "no limit".
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` is the structure this information class takes, of the size
            // passed, and lives until the call returns.
            let set = unsafe {
                SetInformationJobObject(
                    job.raw(),
                    JobObjectExtendedLimitInformation,
                    addr_of!(limits).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            (set != 0).then_some(Job(job))
        }

        /// Puts a process that was just started into the job; what it starts from then on
        /// follows it. False when Windows refuses.
        pub fn adopt(&self, child: &Child) -> bool {
            // SAFETY: both handles are open; the child's is the process handle std keeps, with
            // full access.
            unsafe { AssignProcessToJobObject(self.0.raw(), child.as_raw_handle()) != 0 }
        }

        /// Ends every process in the job, now.
        pub fn end(&self) {
            // SAFETY: the job handle is open.
            unsafe { TerminateJobObject(self.0.raw(), ENDED_BY_JOB) };
        }
    }

    /// Waits up to `left` for the process to end; its status, or `None` when it still runs.
    pub fn wait_exit(child: &mut Child, left: Duration) -> Option<ExitStatus> {
        // u32::MAX would mean "for ever".
        let millis = left.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: the process handle is open for as long as `child` lives.
        unsafe { WaitForSingleObject(child.as_raw_handle(), millis) };
        child.try_wait().ok().flatten()
    }
}

#[cfg(not(windows))]
pub use other_impl::{wait_exit, Job};

/// Other systems have no Job objects. The exe is built there for its tests only, so the wrapper
/// ends just the shell it started (the caller kills its child as well); the tree is Windows' job.
#[cfg(not(windows))]
mod other_impl {
    use super::{Child, Duration, ExitStatus};
    use std::time::Instant;

    pub struct Job;

    impl Job {
        pub fn new() -> Option<Job> {
            Some(Job)
        }

        pub fn adopt(&self, _child: &Child) -> bool {
            true
        }

        pub fn end(&self) {}
    }

    /// Waits up to `left` for the process to end; its status, or `None` when it still runs.
    pub fn wait_exit(child: &mut Child, left: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + left;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                _ => return None,
            }
        }
    }
}
