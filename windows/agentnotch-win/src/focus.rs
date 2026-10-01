//! Terminals on Windows (DESIGN-WIN §3.2 `Terminals`, §4.9; WP6): console facts through
//! `agentnotch-hook.exe console-info`, host classification (the engine's `hosts::resolve` over
//! what this file reads about the console's window), focus steps (raise a window, select a
//! Windows Terminal tab through `uia`, open an editor), the foreground window and its watcher
//! (`an-foreground`), terminal visibility (`visibility`).
//!
//! Nothing here decides which window or tab is the session's: the engine's plan does. This file
//! only touches other processes' windows, and always checks afterwards what actually happened:
//! a foreground Windows refused is a flash and `RaisedOnly`, never a fight (no
//! `AttachThreadInput`, no synthetic input, no ALT tricks). The app never attaches to a console
//! itself; the helper does, in its own process.

#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use agentnotch_engine::control::hosts::{self, ConsoleWindow};
use agentnotch_engine::platform::{
    ConsoleInfo, FocusOutcome, FocusStep, Foreground, HostApp, ProcessTable, Processes, Terminals,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, FlashWindowEx, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetMessageW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostQuitMessage,
    SetForegroundWindow, ShowWindow, TranslateMessage, ASFW_ANY, EVENT_SYSTEM_FOREGROUND,
    FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, GA_ROOTOWNER, MSG, SW_RESTORE, WINEVENT_OUTOFCONTEXT,
};

use crate::process::WinProcesses;
use crate::{uia, visibility};

/// How long the console helper may take to describe a console. It only attaches and reads a
/// few facts; one that takes longer is stuck (a console that never answers) and is killed, so a
/// stuck console delays the other `an-ui` jobs by at most this.
const HELPER_TIMEOUT: Duration = Duration::from_secs(3);
/// How often the helper is checked for having exited while it runs.
const HELPER_POLL: Duration = Duration::from_millis(20);
/// How many times a window Windows would not bring forward flashes its taskbar button (it
/// keeps flashing until the user activates it, `FLASHW_TIMERNOFG`).
const FLASH_COUNT: u32 = 3;

#[derive(Debug)]
pub struct WinTerminals {
    /// The installed `agentnotch-hook.exe`, which reads a console for us (`console-info`).
    helper: PathBuf,
}

impl WinTerminals {
    pub fn new(helper: &Path) -> Self {
        WinTerminals {
            helper: helper.to_path_buf(),
        }
    }

    /// Runs `<helper> console-info --pid <pid>`: `Err` when the helper could not be run or gave
    /// no answer (missing, failed to start, timed out, exited non-zero, unreadable output), which
    /// says nothing about the session's console; `Ok` is the helper's own report.
    fn run_console_helper(&self, claude_pid: u32) -> Result<ConsoleInfo, String> {
        // A relative path would be looked up on PATH and in the working folder: only the
        // installed helper, named exactly, is ever run.
        if !self.helper.is_absolute() {
            return Err("The console helper's path isn't absolute.".into());
        }
        if !self.helper.is_file() {
            return Err("The console helper is missing.".into());
        }
        let mut child = Command::new(&self.helper)
            .arg("console-info")
            .arg("--pid")
            .arg(claude_pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .map_err(|error| format!("The console helper didn't start: {error}"))?;

        // Read stdout on its own thread, so a helper that writes more than a pipe holds can't
        // stall while this thread only waits for it to exit.
        let (sent, received) = mpsc::channel();
        let stdout = child.stdout.take();
        let reader = std::thread::Builder::new()
            .name("an-console-info".into())
            .spawn(move || {
                let mut text = String::new();
                if let Some(mut stdout) = stdout {
                    let _ = stdout.read_to_string(&mut text);
                }
                let _ = sent.send(text);
            });
        if reader.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("The console helper's output couldn't be read.".into());
        }

        let deadline = Instant::now() + HELPER_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("The console helper didn't answer in time.".into());
                }
                Ok(None) => std::thread::sleep(HELPER_POLL),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "The console helper couldn't be waited for: {error}"
                    ));
                }
            }
        };
        if !status.success() {
            return Err(match status.code() {
                Some(code) => format!("The console helper failed (exit code {code})."),
                None => "The console helper failed.".into(),
            });
        }
        // The helper has exited, so its stdout is closed and the reader finishes at once; the
        // wait only covers a grandchild that inherited the pipe and keeps it open.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let text = received
            .recv_timeout(remaining.max(HELPER_POLL))
            .map_err(|_| "The console helper's output couldn't be read.".to_owned())?;
        parse_console_info(&text)
    }
}

/// The helper's answer: its last non-empty line, one JSON object.
fn parse_console_info(stdout: &str) -> Result<ConsoleInfo, String> {
    let line = stdout
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .ok_or_else(|| "The console helper printed nothing.".to_owned())?;
    serde_json::from_str(line)
        .map_err(|error| format!("The console helper's answer couldn't be read: {error}"))
}

fn not_attached(reason: String) -> ConsoleInfo {
    ConsoleInfo {
        attached: false,
        error: Some(reason),
        ..ConsoleInfo::default()
    }
}

fn hwnd_of(window: u64) -> HWND {
    HWND(window as usize as *mut c_void)
}

fn window_id(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

/// Whether `window` names a window that exists now (0 never does).
fn is_window(window: u64) -> bool {
    // SAFETY: a plain query; any value is safe to ask about.
    window != 0 && unsafe { IsWindow(Some(hwnd_of(window))) }.as_bool()
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer is valid for its whole length; a failure returns 0.
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn owner_pid(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer for the call; a dead window leaves it 0.
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (thread != 0 && pid != 0).then_some(pid)
}

/// A window's title bar text; `None` when `window` isn't a window.
fn title_of(window: u64) -> Option<String> {
    if !is_window(window) {
        return None;
    }
    let hwnd = hwnd_of(window);
    // SAFETY: plain queries; the buffer is one longer than the reported length (the terminator),
    // and a title that grew in between is cut at the buffer's end.
    unsafe {
        let length = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buffer = vec![0u16; length + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
        Some(String::from_utf16_lossy(&buffer[..copied.min(length)]))
    }
}

/// What the window `console-info` named looks like: its class and visibility, and the window
/// that owns it (Windows Terminal's window for a pseudo-console it shows).
fn console_window(window: u64) -> Option<ConsoleWindow> {
    if !is_window(window) {
        return None;
    }
    let hwnd = hwnd_of(window);
    // SAFETY: plain queries on a window that existed a moment ago; a window gone since answers
    // false / null, which reads as "not visible" / "no owner".
    let (visible, root_owner) = unsafe {
        (
            IsWindowVisible(hwnd).as_bool(),
            GetAncestor(hwnd, GA_ROOTOWNER),
        )
    };
    let root_owner = if root_owner.0.is_null() {
        hwnd
    } else {
        root_owner
    };
    Some(ConsoleWindow {
        window,
        class: class_name(hwnd),
        visible,
        root_owner: window_id(root_owner),
        root_owner_class: class_name(root_owner),
        root_owner_pid: owner_pid(root_owner),
    })
}

/// Brings `window` forward and says whether it really is in front now. Windows decides whether
/// this process may take the foreground; when it refuses, the window's taskbar button flashes
/// instead, and the user is told it was only brought near.
fn raise(window: u64) -> FocusOutcome {
    if !is_window(window) {
        return FocusOutcome::NotFound;
    }
    let hwnd = hwnd_of(window);
    // SAFETY: plain window calls on a window that exists; each failure is answered by the
    // foreground check below rather than by the call's result.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        if GetForegroundWindow() == hwnd {
            return FocusOutcome::Focused;
        }
        let flash = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: FLASH_COUNT,
            dwTimeout: 0,
        };
        let _ = FlashWindowEx(&flash);
    }
    FocusOutcome::RaisedOnly
}

/// The foreground as the engine sees it, for the window `hwnd` (`None` for no window).
fn foreground_of(hwnd: HWND) -> Option<Foreground> {
    if hwnd.0.is_null() {
        return None;
    }
    let window = window_id(hwnd);
    Some(Foreground {
        pid: owner_pid(hwnd).unwrap_or(0),
        window,
        title: title_of(window).unwrap_or_default(),
        fullscreen: visibility::is_full_screen(window),
    })
}

fn open_in_editor(editor_exe: &Path, folder: &Path) -> FocusOutcome {
    if !editor_exe.is_absolute() || !editor_exe.is_file() {
        return FocusOutcome::Failed(format!("The editor {} wasn't found.", editor_exe.display()));
    }
    // Only an editor of the VS Code family is ever started, by its own exe (never `code.cmd`, a
    // script, or whatever program a confused plan named): the click starts a process, so the
    // name is checked here as well as in the engine's classification.
    let is_editor = editor_exe
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| hosts::editor_product(name).is_some());
    if !is_editor {
        return FocusOutcome::Failed(format!(
            "{} isn't an editor this app opens folders with.",
            editor_exe.display()
        ));
    }
    // An absolute folder also never reads as one of the editor's options ("--…").
    if !folder.is_absolute() {
        return FocusOutcome::Failed(format!(
            "The folder {} isn't a full path.",
            folder.display()
        ));
    }
    // The editor already running takes the foreground when the new process hands it the folder,
    // and that instance is not our child: any process may take it for this one hand-over (the
    // right lapses at the next input or when someone takes the foreground).
    // SAFETY: a plain call; a refusal only means the editor may flash instead of coming forward.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let spawned = Command::new(editor_exe)
        .arg(folder)
        // Set by an editor's own terminals: it would make the editor run as plain Node.
        .env_remove("ELECTRON_RUN_AS_NODE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Nobody waits for the editor; the thread only collects its exit so the process
            // handle is closed when it ends.
            let _ = std::thread::Builder::new()
                .name("an-editor".into())
                .spawn(move || {
                    let _ = child.wait();
                });
            FocusOutcome::Focused
        }
        Err(error) => {
            FocusOutcome::Failed(format!("{} didn't start: {error}", editor_exe.display()))
        }
    }
}

impl Terminals for WinTerminals {
    fn classify_host(&self, claude_pid: u32, table: &ProcessTable) -> HostApp {
        // When the helper itself could not run, nothing is known about the console: that is
        // "unknown", not "no console" (which would hide the session's terminal for good).
        let (console, attached) = match self.run_console_helper(claude_pid) {
            Ok(info) => (info.window.and_then(console_window), Some(info.attached)),
            Err(_) => (None, None),
        };
        let processes = WinProcesses::new();
        hosts::resolve(
            table,
            claude_pid,
            console.as_ref(),
            attached,
            &visibility::top_windows(table),
            &|pid| processes.exe_path(pid),
        )
    }

    fn console_info(&self, claude_pid: u32) -> ConsoleInfo {
        self.run_console_helper(claude_pid)
            .unwrap_or_else(not_attached)
    }

    fn run_focus(&self, step: &FocusStep) -> FocusOutcome {
        match step {
            FocusStep::RaiseWindow { window } => raise(*window),
            // A tab that can't be told apart is left alone and reported as not found: the plan's
            // next step raises the window, and `run_plan` reports that as only brought near.
            // Raising here as well would hide that the tab wasn't selected.
            FocusStep::SelectWtTab { window, title } => match uia::select_tab(*window, title) {
                uia::TabSelect::Selected => raise(*window),
                uia::TabSelect::NoMatch | uia::TabSelect::Ambiguous | uia::TabSelect::Failed(_) => {
                    FocusOutcome::NotFound
                }
            },
            FocusStep::OpenInEditor { editor_exe, folder } => open_in_editor(editor_exe, folder),
            FocusStep::ActivatePid { pid } => {
                if *pid == 0 {
                    return FocusOutcome::NotFound;
                }
                let table = WinProcesses::new().table();
                match visibility::top_windows(&table)
                    .into_iter()
                    .find(|window| window.pid == *pid)
                {
                    Some(window) => raise(window.window),
                    None => FocusOutcome::NotFound,
                }
            }
        }
    }

    fn foreground(&self) -> Option<Foreground> {
        // SAFETY: a plain query; null when no window is in front (a switch in progress).
        foreground_of(unsafe { GetForegroundWindow() })
    }

    fn window_title(&self, window: u64) -> Option<String> {
        title_of(window)
    }

    fn wt_tab_titles(&self, window: u64) -> Option<Vec<(String, bool)>> {
        uia::tab_titles(window)
    }

    fn any_terminal_visible(&self) -> bool {
        visibility::any_terminal_visible(&WinProcesses::new().table())
    }

    fn watch_foreground(&self, sink: crossbeam_channel::Sender<Foreground>) {
        let started = std::thread::Builder::new()
            .name("an-foreground".into())
            .spawn(move || watch(sink));
        // Without the watcher the hub still asks for the foreground when it needs it; it only
        // misses the "stayed in front 1.5 s" marks.
        drop(started);
    }
}

thread_local! {
    /// Where the hook's callback sends on the `an-foreground` thread (an out-of-context hook
    /// calls back on the thread that set it, inside its message loop).
    static SINK: RefCell<Option<crossbeam_channel::Sender<Foreground>>> = const { RefCell::new(None) };
    /// The hook, so the callback can remove it once nobody listens.
    static HOOK: Cell<Option<HWINEVENTHOOK>> = const { Cell::new(None) };
}

fn unhook() {
    if let Some(hook) = HOOK.with(Cell::take) {
        // SAFETY: the hook was set on this thread and is removed once (taken out of the cell).
        let _ = unsafe { UnhookWinEvent(hook) };
    }
}

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let Some(foreground) = foreground_of(hwnd) else {
        return;
    };
    let delivered = SINK.with(|sink| match sink.borrow().as_ref() {
        Some(sink) => sink.send(foreground).is_ok(),
        None => false,
    });
    if !delivered {
        // The hub is gone: stop listening and end the thread's message loop.
        SINK.with(|sink| sink.borrow_mut().take());
        unhook();
        // SAFETY: posts WM_QUIT to this thread's own queue.
        unsafe { PostQuitMessage(0) };
    }
}

/// The `an-foreground` thread: an out-of-context foreground hook and the message loop that
/// delivers its events, until the receiver goes away.
fn watch(sink: crossbeam_channel::Sender<Foreground>) {
    SINK.with(|cell| *cell.borrow_mut() = Some(sink));
    // SAFETY: no module (out of context), a callback that lives for the whole program, and every
    // process and thread; the hook is removed on this same thread.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.is_invalid() {
        return;
    }
    HOOK.with(|cell| cell.set(Some(hook)));
    let mut message = MSG::default();
    // SAFETY: the standard message loop over this thread's queue; GetMessageW returns 0 on
    // WM_QUIT and -1 on an error, both of which end the loop.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    unhook();
    SINK.with(|sink| sink.borrow_mut().take());
}
