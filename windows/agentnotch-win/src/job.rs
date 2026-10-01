//! Child processes on Windows (DESIGN-WIN §3.2 `CommandRunner`; WP4): every child in a Job object
//! with KILL_ON_JOB_CLOSE, CREATE_NO_WINDOW, a cleared and then complete environment, piped stdio;
//! `kill_tree` = `TerminateJobObject`. Used by the usage probe, `claude --version` and summaries.
//!
//! The child is started by std's `Command` (suspended), not by a hand-made `CreateProcessW`, so
//! std keeps its batch-file argument escaping for npm `.cmd` shims, and its refusal of arguments
//! it can't pass safely reaches the probe unchanged (D 1801-1803). Suspended, the child can't
//! start a grandchild before it is in the job; whatever it starts later joins the job too, so
//! `kill_tree`, and dropping the runner's handle, end the whole tree. A leaked `claude` would keep
//! spending the account's usage.

#![cfg(windows)]

use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use agentnotch_engine::platform::{CommandRunner, CommandSpec, Exit, RunningCommand};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_TIMEOUT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, INFINITE,
    THREAD_SUSPEND_RESUME,
};

/// The exit code `TerminateJobObject` gives what it ends. Nothing reads it: a child ended by
/// `kill_tree` is reported as [`Exit::Killed`].
const KILLED_EXIT_CODE: u32 = 1;

#[derive(Debug, Default)]
pub struct JobRunner;

impl JobRunner {
    pub fn new() -> Self {
        JobRunner
    }
}

impl CommandRunner for JobRunner {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        // The job first: a failure here has started nothing.
        let job = kill_on_close_job()?;

        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(spec.env.iter().map(|(name, value)| (name, value)))
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
        // std's own error (a missing program, a batch argument it refuses) passes through as it
        // is: the probe tells a refused shim argument by its kind.
        let mut child = command.spawn()?;

        if let Err(error) = place_and_resume(&job, &child) {
            // Still suspended, or at least never resumed: it has started nothing of its own.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Box::new(JobChild {
            child,
            job,
            killed: false,
        }))
    }
}

/// A new anonymous job that ends everything in it when its last handle is closed.
fn kill_on_close_job() -> io::Result<OwnedHandle> {
    // SAFETY: no security attributes and no name; the wrapper checks the returned handle, and
    // it is owned right below, so it is closed exactly once.
    let raw = unsafe { CreateJobObjectW(None, PCWSTR::null()) }?;
    // SAFETY: `raw` is a fresh, valid handle that nothing else owns.
    let job = unsafe { OwnedHandle::from_raw_handle(raw.0) };

    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: the pointer and size describe `limits`, which outlives the call, and the class
    // names that structure.
    unsafe {
        SetInformationJobObject(
            handle(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }?;
    Ok(job)
}

/// Puts the suspended child in the job, then lets it run.
fn place_and_resume(job: &OwnedHandle, child: &Child) -> io::Result<()> {
    // SAFETY: both handles are live for the length of the call (the job owned by `job`, the
    // process by std's `Child`).
    unsafe { AssignProcessToJobObject(handle(job), HANDLE(child.as_raw_handle())) }?;
    resume_threads_of(child.id())
}

/// Resumes the threads of process `pid`. A process created suspended has exactly one; std
/// doesn't hand out its handle, so it is found in a thread snapshot.
fn resume_threads_of(pid: u32) -> io::Result<()> {
    // SAFETY: plain call (the process id is ignored for thread snapshots); the wrapper checks
    // the handle, which is owned right below.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }?;
    // SAFETY: `raw` is a fresh, valid handle that nothing else owns.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw.0) };

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut resumed = 0;
    // SAFETY: `entry` is a THREADENTRY32 with its size set; the snapshot handle is live.
    let mut more = unsafe { Thread32First(handle(&snapshot), &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == pid {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        // SAFETY: as for Thread32First.
        more = unsafe { Thread32Next(handle(&snapshot), &mut entry) }.is_ok();
    }
    if resumed == 0 {
        return Err(io::Error::other(format!(
            "no thread of the new process {pid} to resume"
        )));
    }
    Ok(())
}

fn resume_thread(thread_id: u32) -> io::Result<()> {
    // SAFETY: plain call; the wrapper checks the handle, which is owned right below.
    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, thread_id) }?;
    // SAFETY: `raw` is a fresh, valid handle that nothing else owns.
    let thread = unsafe { OwnedHandle::from_raw_handle(raw.0) };
    // SAFETY: the thread handle is live and was opened with THREAD_SUSPEND_RESUME.
    let previous = unsafe { ResumeThread(handle(&thread)) };
    if previous == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn handle(owned: &OwnedHandle) -> HANDLE {
    HANDLE(owned.as_raw_handle())
}

/// A started child. Dropping it closes the job's only handle, which ends whatever is left of
/// the tree (KILL_ON_JOB_CLOSE); std's `Child` closes the process handle and the pipes.
struct JobChild {
    child: Child,
    job: OwnedHandle,
    /// `kill_tree` ended the child itself (it was still running then).
    killed: bool,
}

impl RunningCommand for JobChild {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.child
            .stdin
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Write + Send>)
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)
    }

    fn wait_timeout(&mut self, d: Duration) -> io::Result<Option<Exit>> {
        // INFINITE itself would never return; anything near 49 days is the same here.
        let millis = d.as_millis().min(u128::from(INFINITE - 1)) as u32;
        // SAFETY: the process handle is owned by `self.child` and live.
        let waited = unsafe { WaitForSingleObject(HANDLE(self.child.as_raw_handle()), millis) };
        if waited == WAIT_TIMEOUT {
            return Ok(None);
        }
        if waited == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        // Signalled: std reads the exit code without waiting.
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        if self.killed {
            return Ok(Some(Exit::Killed));
        }
        // Always present on Windows.
        Ok(Some(status.code().map_or(Exit::Killed, Exit::Code)))
    }

    fn kill_tree(&mut self) {
        let was_running = matches!(self.child.try_wait(), Ok(None));
        // SAFETY: the job handle is owned by `self` and live. Terminating an emptied job again
        // does nothing, so this is idempotent.
        let ended = unsafe { TerminateJobObject(handle(&self.job), KILLED_EXIT_CODE) }.is_ok();
        // Should the job refuse, the child itself still goes.
        let ended = ended || self.child.kill().is_ok();
        if was_running && ended {
            self.killed = true;
        }
    }
}
