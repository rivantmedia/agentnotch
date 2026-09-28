//! Every platform service on systems other than Windows: "not available on this platform".
//!
//! Only the fork crates are built here (their `cargo test` runs on macOS and Linux); the app that
//! would use these services builds only on Windows. So nothing below ever does anything: each
//! call fails, reports "unknown" or does nothing, the way the engine already treats a service
//! that can't answer.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use agentnotch_engine::platform::{
    Browser, Chime, CommandRunner, CommandSpec, ConnId, ConsoleInfo, ConsoleInput, ConsoleTarget,
    Device, EnvRead, Exit, Expect, FileIdentity, FocusOutcome, FocusStep, Foreground,
    HookTransport, HostApp, HostKind, Http, HttpError, HttpRequest, HttpResponse, Liveness,
    Notifier, NotifyPermission, ProcessTable, Processes, RunningCommand, SecureFiles, Sounds,
    Terminals, Toast, TransportEvent, TypeOutcome, WriteMode, WriteResult,
};

const UNAVAILABLE: &str = "not available on this platform";

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, UNAVAILABLE)
}

/// One value standing in for every service.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unavailable;

impl Processes for Unavailable {
    fn liveness(&self, _pid: u32) -> Liveness {
        Liveness::Unknown
    }
    fn start_time(&self, _pid: u32) -> Option<SystemTime> {
        None
    }
    fn table(&self) -> ProcessTable {
        ProcessTable {
            entries: Vec::new(),
        }
    }
    fn config_dir_env(&self, _pid: u32) -> EnvRead {
        EnvRead::Unreadable
    }
    fn same_user(&self, _pid: u32) -> Option<bool> {
        None
    }
    fn elevated(&self, _pid: u32) -> Option<bool> {
        None
    }
    fn exe_path(&self, _pid: u32) -> Option<PathBuf> {
        None
    }
}

impl CommandRunner for Unavailable {
    fn spawn(&self, _spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        Err(unsupported())
    }
}

/// Never constructed (`spawn` always fails); here so the trait's shape is checked on every OS.
#[allow(dead_code)]
struct NoCommand;

impl RunningCommand for NoCommand {
    fn pid(&self) -> u32 {
        0
    }
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        None
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        None
    }
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        None
    }
    fn wait_timeout(&mut self, _d: Duration) -> io::Result<Option<Exit>> {
        Err(unsupported())
    }
    fn kill_tree(&mut self) {}
}

impl HookTransport for Unavailable {
    fn start(
        &self,
        _pipe_name: &str,
        _sink: crossbeam_channel::Sender<TransportEvent>,
    ) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    fn respond(&self, _conn: ConnId, _frame_json: Vec<u8>) -> bool {
        false
    }
    fn close(&self, _conn: ConnId) {}
    fn stop(&self) {}
}

impl Terminals for Unavailable {
    fn classify_host(&self, _claude_pid: u32, _table: &ProcessTable) -> HostApp {
        HostApp {
            kind: HostKind::Unknown,
            window: None,
            host_pid: None,
            exe_path: None,
        }
    }
    fn console_info(&self, _claude_pid: u32) -> ConsoleInfo {
        ConsoleInfo {
            attached: false,
            window: None,
            title: None,
            processes: Vec::new(),
            line_input: None,
            elevated_target: false,
            error: Some(UNAVAILABLE.into()),
        }
    }
    fn run_focus(&self, _step: &FocusStep) -> FocusOutcome {
        FocusOutcome::Failed(UNAVAILABLE.into())
    }
    fn foreground(&self) -> Option<Foreground> {
        None
    }
    fn window_title(&self, _window: u64) -> Option<String> {
        None
    }
    fn wt_tab_titles(&self, _window: u64) -> Option<Vec<(String, bool)>> {
        None
    }
    fn any_terminal_visible(&self) -> bool {
        false
    }
    fn watch_foreground(&self, _sink: crossbeam_channel::Sender<Foreground>) {}
}

impl ConsoleInput for Unavailable {
    fn type_text(
        &self,
        _target: &ConsoleTarget,
        _text: &str,
        _recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome {
        TypeOutcome::Failed(UNAVAILABLE.into())
    }
}

impl Notifier for Unavailable {
    fn post(&self, _t: &Toast) {}
    fn withdraw(&self, _tag: &str, _group: &str) {}
    fn permission(&self) -> NotifyPermission {
        NotifyPermission::Unavailable
    }
}

impl Sounds for Unavailable {
    fn play(&self, _c: Chime) {}
}

impl Http for Unavailable {
    fn send(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Other(UNAVAILABLE.into()))
    }
}

impl Browser for Unavailable {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
}

impl SecureFiles for Unavailable {
    fn ensure_private_dir(&self, _dir: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    fn write_atomic(
        &self,
        _path: &Path,
        _bytes: &[u8],
        _mode: WriteMode,
        _expect: Expect,
    ) -> io::Result<WriteResult> {
        Err(unsupported())
    }
    fn create_exclusive(&self, _path: &Path, _bytes: &[u8]) -> io::Result<bool> {
        Err(unsupported())
    }
    fn identity(&self, _path: &Path) -> io::Result<FileIdentity> {
        Err(unsupported())
    }
    fn is_reparse(&self, _path: &Path) -> io::Result<bool> {
        Err(unsupported())
    }
    fn canonical(&self, _path: &Path) -> io::Result<PathBuf> {
        Err(unsupported())
    }
    fn is_private(&self, _path: &Path) -> io::Result<bool> {
        Err(unsupported())
    }
}

impl Device for Unavailable {
    fn computer_name(&self) -> String {
        String::new()
    }
    fn user_sid(&self) -> Option<String> {
        None
    }
    fn elevated(&self) -> bool {
        false
    }
    fn smart_app_control(&self) -> Option<String> {
        None
    }
}

/// Folder resolution (`paths` on Windows).
pub mod paths {
    use std::path::PathBuf;

    use agentnotch_engine::platform::Roots;

    /// The app's folders exist only on Windows.
    pub fn roots(
        _app_identifier: &str,
        _data: PathBuf,
        _install_dir: Option<PathBuf>,
    ) -> Result<Roots, String> {
        Err(super::UNAVAILABLE.into())
    }
}

/// The sessions panel's window styles and foreground checks (`window` on Windows, WP9).
pub mod window {}

/// The panel shortcut (`hotkey` on Windows, WP9).
pub mod hotkey {}

/// Copying text for the panel (`clipboard` on Windows, WP9).
pub mod clipboard {}

/// Sealed snapshots of a page (`capture` on Windows, WP9).
pub mod capture {}

/// Showing a file in Explorer (`shell` on Windows, WP9).
pub mod shell {}
