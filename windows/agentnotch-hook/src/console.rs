//! `type` and `console-info`: the console helper the app runs (DESIGN-WIN §1.1, §4.8).
//!
//! The GUI app never attaches to a console itself (Ctrl+C and close events would reach it); it
//! runs this exe with CREATE_NO_WINDOW and piped stdio. The helper leaves whatever console it
//! has (`FreeConsole`), attaches to the console of the session's Claude (`AttachConsole(pid)`,
//! never the parent's) and either describes that console or types a reply into its input buffer
//! in two phases: the text, then Return only after the engine looked at the session again. Keys
//! go into the input buffer of that one console with `WriteConsoleInputW`; nothing here sends
//! input to a window or to whatever has the focus.
//!
//! Both roles answer on stdout with one JSON object per line and exit 0, whatever happened; a
//! watchdog ends a helper that outlives `CONSOLE_HELPER_LIFETIME_MS`.
//!
//! What decides is in [`checks`], which is plain data and runs on every system; this file reads
//! the console's state and writes the records. Where there is no console API the helper attaches
//! to nothing: `type` reports a failure the app shows as such, `console-info` reports no console.

pub mod checks;

use std::io::Read;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use agentnotch_proto::limits::{
    CONSOLE_HELPER_LIFETIME_MS, MAX_CLIENT_MESSAGE, TYPE_SUBMIT_WAIT_MS,
};
use agentnotch_proto::{TypeArgs, TypePhase};

use crate::watchdog::Watchdog;
use crate::{io, trace};

/// One stdin line, or `None` once stdin ended.
type Lines = Receiver<Option<Vec<u8>>>;

/// Two-phase typing. Its final line is `{"outcome": …, "reason"?: …}`; with nothing typed,
/// there is no `{"phase":"typed"}` line before it.
pub fn type_reply(target: &TypeArgs) {
    let _lifetime = Watchdog::arm(Duration::from_millis(CONSOLE_HELPER_LIFETIME_MS));
    let outcome = platform::type_reply(target);
    // The outcome's word only: the reason is copy, the text is the user's.
    trace::note(|| match &outcome {
        TypePhase::Outcome { outcome, .. } => format!("type: {outcome}"),
        TypePhase::Typed => "type: typed".into(),
    });
    emit(&outcome.to_line());
}

/// `agentnotch_proto::ConsoleInfo` as one JSON line.
pub fn info(pid: u32) {
    let _lifetime = Watchdog::arm(Duration::from_millis(CONSOLE_HELPER_LIFETIME_MS));
    let info = platform::info(pid);
    trace::note(|| format!("console-info: attached={}", info.attached));
    emit(&serde_json::to_string(&info).unwrap_or_else(|_| "{}".into()));
}

fn emit(line: &str) {
    io::write_stdout(format!("{line}\n").as_bytes());
}

/// Reads `source` line by line on a thread of its own, so waiting for a line can time out: the
/// second line may never come, and a pipe read can't be abandoned.
///
/// A last line without its newline still counts; a line longer than a hook message ends the
/// reading.
fn lines(mut source: impl Read + Send + 'static) -> Lines {
    let (sender, receiver) = mpsc::channel();
    // When the thread can't start the sender is dropped, which reads as "stdin ended".
    let _ = std::thread::Builder::new()
        .name("stdin".into())
        .spawn(move || {
            let mut pending: Vec<u8> = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                match source.read(&mut buffer) {
                    Ok(read) if read > 0 => {
                        pending.extend_from_slice(&buffer[..read]);
                        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                            let line: Vec<u8> = pending.drain(..=end).collect();
                            if sender.send(Some(line)).is_err() {
                                return;
                            }
                        }
                        if pending.len() > MAX_CLIENT_MESSAGE {
                            pending.clear();
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => break,
                }
            }
            if !pending.is_empty() {
                let _ = sender.send(Some(pending));
            }
            let _ = sender.send(None);
        });
    receiver
}

/// Stdin line 1: the text to type. The app writes it right after starting the helper.
fn first_line(lines: &Lines) -> Result<String, &'static str> {
    match lines.recv_timeout(Duration::from_millis(TYPE_SUBMIT_WAIT_MS)) {
        Ok(Some(line)) => checks::parse_request(&line),
        _ => Err(checks::NO_REQUEST),
    }
}

#[cfg(not(windows))]
mod platform {
    use agentnotch_proto::{ConsoleInfo, TypeArgs, TypePhase};

    use super::checks;

    const NOT_AVAILABLE: &str = "Typing into a terminal isn't available in this build.";

    pub fn type_reply(_target: &TypeArgs) -> TypePhase {
        match super::first_line(&super::lines(std::io::stdin())) {
            Ok(_) => checks::failed(NOT_AVAILABLE),
            Err(reason) => checks::failed(reason),
        }
    }

    pub fn info(_pid: u32) -> ConsoleInfo {
        ConsoleInfo {
            error: Some(NOT_AVAILABLE.into()),
            ..ConsoleInfo::default()
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::io::Read;
    use std::mem::{size_of, zeroed};
    use std::ptr::{addr_of_mut, null, null_mut};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    use agentnotch_proto::limits::{TYPE_SETTLE_MS, TYPE_SUBMIT_WAIT_MS};
    use agentnotch_proto::{ConsoleInfo, TypeArgs, TypePhase};
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileType, ReadFile, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK,
        FILE_TYPE_PIPE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GetConsoleMode, GetConsoleProcessList, GetConsoleTitleW,
        GetConsoleWindow, GetStdHandle, SetConsoleCtrlHandler, SetStdHandle, WriteConsoleInputW,
        ENABLE_LINE_INPUT, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD,
        KEY_EVENT_RECORD_0, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
    };

    use super::checks::{self, Console, Facts, KeyRecord, Proc, Second};
    use super::Lines;
    use crate::win::{self, Handle};

    /// Records per `WriteConsoleInputW` call. Even, so a key's down and up go in together.
    const RECORDS_PER_WRITE: usize = 256;
    /// A parent chain is followed this far (`checks::descendants` stops there too).
    const MAX_HOPS: usize = 32;

    /// The three standard handles as the app gave them to the helper: pipes.
    ///
    /// They are taken before `FreeConsole`. A handle that is a pipe is not a console handle and
    /// stays valid when the console goes; but `AttachConsole` may put the new console's handles
    /// into the process's standard slots, and the answer must never be printed into Claude's
    /// console, nor the second line read from it. So the slots are put back after attaching, and
    /// stdin is read through the handle kept here, never through the slot.
    struct StdHandles {
        input: HANDLE,
        output: HANDLE,
        error: HANDLE,
    }

    impl StdHandles {
        fn capture() -> StdHandles {
            // SAFETY: reads the process's standard handle slots; no pointers.
            unsafe {
                StdHandles {
                    input: GetStdHandle(STD_INPUT_HANDLE),
                    output: GetStdHandle(STD_OUTPUT_HANDLE),
                    error: GetStdHandle(STD_ERROR_HANDLE),
                }
            }
        }

        fn restore(&self) {
            // SAFETY: writes the slots back to the values they had at start; a null or invalid
            // value is a valid "no handle" there.
            unsafe {
                SetStdHandle(STD_INPUT_HANDLE, self.input);
                SetStdHandle(STD_OUTPUT_HANDLE, self.output);
                SetStdHandle(STD_ERROR_HANDLE, self.error);
            }
        }

        /// Stdin, when it is a pipe (or a file): what survives `FreeConsole`. A console or the
        /// null device (both character devices) and a missing handle have no request to read,
        /// and a console is exactly what this helper must not read from.
        fn piped_input(&self) -> Option<PipeInput> {
            // SAFETY: asks for the type of a handle value; an invalid one answers "unknown".
            let kind = unsafe { GetFileType(self.input) };
            (kind == FILE_TYPE_PIPE || kind == FILE_TYPE_DISK)
                .then_some(PipeInput(self.input as usize))
        }
    }

    /// The stdin pipe, read directly. The value is the handle, kept as a number so the reader
    /// thread can own it; the process never closes its stdin.
    struct PipeInput(usize);

    impl Read for PipeInput {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let mut read = 0u32;
            let capacity = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
            // SAFETY: the handle is the process's stdin, open for its whole life; `buffer` holds
            // `capacity` bytes and `read` is a valid out pointer. A synchronous read: no
            // OVERLAPPED.
            let ok = unsafe {
                ReadFile(
                    self.0 as HANDLE,
                    buffer.as_mut_ptr(),
                    capacity,
                    &mut read,
                    null_mut(),
                )
            };
            // A broken pipe (the app closed its end) and every other failure end the input.
            Ok(if ok == 0 { 0 } else { read as usize })
        }
    }

    /// The console of the session's Claude, attached; left again when dropped.
    struct Attached {
        target: TypeArgs,
        own_pid: u32,
        /// `CONIN$` of the attached console.
        input: Handle,
    }

    impl Drop for Attached {
        fn drop(&mut self) {
            // SAFETY: detaches this process from the console; no arguments.
            unsafe { FreeConsole() };
        }
    }

    /// Leaves this process's console and attaches to `pid`'s.
    ///
    /// Once `FreeConsole` ran the helper has no console at all, so when attaching fails there is
    /// nothing it could type into by mistake. The pid is always an explicit one
    /// (`parse_invocation` admits 1..=2^31-1), never `ATTACH_PARENT_PROCESS`.
    fn attach(target: &TypeArgs, std: &StdHandles) -> Result<Attached, &'static str> {
        // SAFETY: returns this process's id; no arguments.
        let own_pid = unsafe { GetCurrentProcessId() };
        if target.pid == own_pid {
            return Err(checks::NO_CONSOLE);
        }
        // Keys for an elevated Claude must not come from a process that is not: refused whether
        // or not Windows would let the attach through.
        if out_of_reach(target.pid) {
            return Err(checks::ELEVATED);
        }
        // SAFETY: plain calls without pointers. Ctrl+C is ignored first: attached to Claude's
        // console, a Ctrl+C typed there is delivered to every process on it, this one included.
        let attached = unsafe {
            SetConsoleCtrlHandler(None, 1);
            FreeConsole();
            AttachConsole(target.pid) != 0
        };
        std.restore();
        if !attached {
            return Err(checks::NO_CONSOLE);
        }
        // SAFETY: as above; said again because attaching is what makes it matter.
        unsafe { SetConsoleCtrlHandler(None, 1) };
        let Some(input) = open_console_input() else {
            // SAFETY: detaches again; no arguments.
            unsafe { FreeConsole() };
            return Err(checks::NO_CONSOLE);
        };
        Ok(Attached {
            target: target.clone(),
            own_pid,
            input,
        })
    }

    /// `CONIN$`: the input buffer of the console this process is attached to, whatever its
    /// standard handles are.
    fn open_console_input() -> Option<Handle> {
        let name = win::wide("CONIN$");
        // SAFETY: `name` is NUL-terminated and outlives the call; the other arguments are plain
        // values or null. The returned handle is owned by `Handle`.
        // not a pipe: the console input buffer
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        Handle::new(raw)
    }

    /// Does the process's token carry full administrator rights? `None` when it can't be read.
    fn elevated(process: HANDLE) -> Option<bool> {
        let mut token: HANDLE = null_mut();
        // SAFETY: `process` is a live handle (or the current-process pseudo-handle); `token` is a
        // valid out pointer and what it receives is owned by `Handle` below.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let token = Handle::new(token)?;
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        // SAFETY: `elevation` is a buffer of exactly the size passed; `returned` is valid.
        let ok = unsafe {
            GetTokenInformation(
                token.raw(),
                TokenElevation,
                addr_of_mut!(elevation).cast::<c_void>(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        };
        (ok != 0).then_some(elevation.TokenIsElevated != 0)
    }

    /// The target runs elevated and this helper does not.
    fn out_of_reach(pid: u32) -> bool {
        // SAFETY: the pseudo-handle of the current process; nothing to close.
        if elevated(unsafe { GetCurrentProcess() }) != Some(false) {
            return false;
        }
        win::open_process(pid).and_then(|process| elevated(process.raw())) == Some(true)
    }

    /// The pids attached to this process's console, `own_pid` left out; empty when unreadable.
    fn console_processes(own_pid: u32) -> Vec<u32> {
        let mut pids = vec![0u32; 64];
        // The list may grow between two calls; a few rounds settle it.
        for _ in 0..4 {
            // SAFETY: `pids` holds the number of elements passed.
            let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
            let count = count as usize;
            if count == 0 {
                return Vec::new();
            }
            if count <= pids.len() {
                pids.truncate(count);
                pids.retain(|pid| *pid != own_pid);
                return pids;
            }
            pids = vec![0u32; count + 16];
        }
        Vec::new()
    }

    /// Every process's parent pid, as the system has them now.
    fn parents() -> HashMap<u32, u32> {
        let mut parents = HashMap::new();
        // SAFETY: plain arguments; the handle (INVALID_HANDLE_VALUE on failure) is owned by
        // `Handle`.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        let Some(snapshot) = Handle::new(snapshot) else {
            return parents;
        };
        // SAFETY: PROCESSENTRY32W is plain integers and a character array; all-zero is valid.
        let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: `snapshot` is live and `entry` has its size set, here and in the loop.
        let mut more = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != 0;
        while more {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != 0;
        }
        parents
    }

    /// The parent chains of `candidates` up to `root`, with each process's creation time, for
    /// `checks::descendants` to judge.
    fn chains(candidates: &[u32], root: u32) -> Vec<Proc> {
        let mut procs: Vec<Proc> = Vec::new();
        if candidates.is_empty() {
            return procs;
        }
        let parents = parents();
        for candidate in candidates {
            let mut current = *candidate;
            for _ in 0..MAX_HOPS {
                if current == root || procs.iter().any(|known| known.pid == current) {
                    break;
                }
                let Some(parent) = parents.get(&current).copied() else {
                    break;
                };
                procs.push(Proc {
                    pid: current,
                    parent,
                    created: win::open_process(current)
                        .and_then(|process| win::created(process.raw())),
                });
                current = parent;
            }
        }
        procs
    }

    /// `GetConsoleWindow()`: the conhost window, or the pseudo-window of a ConPTY.
    fn console_window() -> Option<u64> {
        // SAFETY: returns a window handle value; no arguments.
        let window = unsafe { GetConsoleWindow() } as usize;
        (window != 0).then_some(window as u64)
    }

    fn console_title() -> Option<String> {
        let mut buffer = vec![0u16; 1024];
        // SAFETY: `buffer` holds the number of units passed.
        let length = unsafe { GetConsoleTitleW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        (length > 0).then(|| String::from_utf16_lossy(&buffer[..length.min(buffer.len() - 1)]))
    }

    fn line_input(console_input: &Handle) -> Option<bool> {
        let mut mode = 0u32;
        // SAFETY: a live console input handle and a valid out pointer.
        let ok = unsafe { GetConsoleMode(console_input.raw(), &mut mode) } != 0;
        ok.then_some(mode & ENABLE_LINE_INPUT != 0)
    }

    fn input_record(record: &KeyRecord) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: i32::from(record.key_down),
                    wRepeatCount: record.repeat,
                    wVirtualKeyCode: record.virtual_key,
                    wVirtualScanCode: record.scan_code,
                    uChar: KEY_EVENT_RECORD_0 {
                        UnicodeChar: record.unit,
                    },
                    dwControlKeyState: 0,
                },
            },
        }
    }

    impl Console for Attached {
        fn facts(&mut self) -> Facts {
            let root = self.target.pid;
            let created = win::open_process(root).and_then(|process| win::created(process.raw()));
            let attached = console_processes(self.own_pid);
            // The snapshot is only taken for processes nothing else accounts for.
            let unknown: Vec<u32> = attached
                .iter()
                .copied()
                .filter(|pid| *pid != root && !self.target.allowed_shells.contains(pid))
                .collect();
            let descendants = checks::descendants(root, created, &unknown, &chains(&unknown, root));
            Facts {
                started_ms: created.map(win::filetime_to_unix_ms),
                window: console_window(),
                attached,
                descendants,
                line_input: line_input(&self.input),
            }
        }

        fn write(&mut self, records: &[KeyRecord]) -> usize {
            let mut total = 0usize;
            for chunk in records.chunks(RECORDS_PER_WRITE) {
                let raw: Vec<INPUT_RECORD> = chunk.iter().map(input_record).collect();
                let mut written = 0u32;
                // SAFETY: `raw` holds the number of records passed; `written` is a valid out
                // pointer; the handle is the attached console's input buffer.
                let ok = unsafe {
                    WriteConsoleInputW(
                        self.input.raw(),
                        raw.as_ptr(),
                        raw.len() as u32,
                        &mut written,
                    )
                } != 0;
                let written = (written as usize).min(raw.len());
                total += written;
                if !ok || written < raw.len() {
                    break;
                }
            }
            total
        }
    }

    /// Stdin line 2, within `TYPE_SUBMIT_WAIT_MS`.
    fn second(lines: &Lines) -> Second {
        match lines.recv_timeout(Duration::from_millis(TYPE_SUBMIT_WAIT_MS)) {
            Ok(Some(line)) => checks::second_line(&line),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => Second::Eof,
            Err(RecvTimeoutError::Timeout) => Second::Timeout,
        }
    }

    pub fn type_reply(target: &TypeArgs) -> TypePhase {
        let std = StdHandles::capture();
        let Some(input) = std.piped_input() else {
            return checks::failed(checks::NO_REQUEST);
        };
        let lines = super::lines(input);
        let text = match super::first_line(&lines) {
            Ok(text) => text,
            Err(reason) => return checks::failed(reason),
        };
        let mut console = match attach(target, &std) {
            Ok(console) => console,
            Err(reason) => return checks::refused(reason),
        };
        checks::type_flow(
            &mut console,
            target,
            &text,
            || std::thread::sleep(Duration::from_millis(TYPE_SETTLE_MS)),
            || {
                super::emit(&TypePhase::Typed.to_line());
                second(&lines)
            },
        )
    }

    pub fn info(pid: u32) -> ConsoleInfo {
        let std = StdHandles::capture();
        let target = TypeArgs {
            pid,
            started_ms: 0,
            expect_window: None,
            allowed_shells: Vec::new(),
        };
        let console = match attach(&target, &std) {
            Ok(console) => console,
            Err(reason) => {
                return ConsoleInfo {
                    elevated_target: reason == checks::ELEVATED,
                    error: Some(reason.into()),
                    ..ConsoleInfo::default()
                }
            }
        };
        ConsoleInfo {
            attached: true,
            window: console_window(),
            title: console_title(),
            processes: console_processes(console.own_pid),
            line_input: line_input(&console.input),
            elevated_target: false,
            error: None,
        }
    }
}
