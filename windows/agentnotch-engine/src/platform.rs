//! Every OS service the engine uses, as a trait (DESIGN-WIN §3.2). The
//! Windows implementations live in `agentnotch-win` (each function does one
//! OS thing and returns plain data, no policy); the fakes in
//! [`crate::testkit`]. Anything decidable without the OS is engine code
//! taking the plain data these return.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub use agentnotch_proto::ConsoleInfo;

/// Time. Tests drive a manual clock; every engine rule takes `now` from here.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn monotonic(&self) -> Instant;
}

/// The folders the engine works in, each resolved the way its owner
/// resolves it (§1.5): the glue builds them once; tests inject them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roots {
    /// `%USERPROFILE%` (what Claude Code's `os.homedir()` reads).
    pub home: PathBuf,
    /// The folder of upstream's `config.json`: `%APPDATA%\Agent Notch`.
    pub data: PathBuf,
    /// `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude`, or
    /// `AGENTNOTCH_SUPPORT_DIR`.
    pub support: PathBuf,
    /// `%APPDATA%\Claude` and each
    /// `%LOCALAPPDATA%\Packages\*claude*|*anthropic*\LocalCache\Roaming\Claude`.
    pub claude_desktop: Vec<PathBuf>,
    /// `%SystemDrive%\Users` (the cloud scrub's local names).
    pub system_users: Option<PathBuf>,
    /// The folder of `agentnotch.exe` (the hook exe is copied from it).
    pub install_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    Alive,
    Gone,
    Unknown,
}

/// One process of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    /// The image's file name (`claude.exe`).
    pub exe_name: String,
    pub started: Option<SystemTime>,
}

/// A snapshot of the processes. A parent link is valid only when the
/// parent's start time is no later than the child's (Windows reuses pids).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTable {
    pub entries: Vec<ProcEntry>,
}

impl ProcessTable {
    pub fn get(&self, pid: u32) -> Option<&ProcEntry> {
        self.entries.iter().find(|e| e.pid == pid)
    }

    /// The parent of `pid`, only when the link is valid.
    pub fn parent(&self, pid: u32) -> Option<&ProcEntry> {
        let child = self.get(pid)?;
        let parent = self.get(child.ppid).filter(|p| p.pid != child.pid)?;
        match (parent.started, child.started) {
            (Some(parent_started), Some(child_started)) if parent_started <= child_started => {
                Some(parent)
            }
            _ => None,
        }
    }

    /// `pid`'s valid ancestors, nearest first, at most `max`.
    pub fn ancestors(&self, pid: u32, max: usize) -> Vec<&ProcEntry> {
        let mut chain = Vec::new();
        let mut current = pid;
        while chain.len() < max {
            let Some(parent) = self.parent(current) else {
                break;
            };
            // A cycle (equal start times in a racy snapshot) ends the walk,
            // and never lists `pid` as its own ancestor.
            if parent.pid == pid || chain.iter().any(|seen: &&ProcEntry| seen.pid == parent.pid) {
                break;
            }
            chain.push(parent);
            current = parent.pid;
        }
        chain
    }
}

/// A process's `CLAUDE_CONFIG_DIR` read from its environment block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvRead {
    Set(String),
    /// Only after a complete, terminated block was read without the name.
    Unset,
    /// Partial read, another user, elevated, gone: never taken for `Unset`.
    Unreadable,
}

pub trait Processes: Send + Sync {
    /// ACCESS_DENIED counts as alive.
    fn liveness(&self, pid: u32) -> Liveness;
    /// `GetProcessTimes` creation time.
    fn start_time(&self, pid: u32) -> Option<SystemTime>;
    /// Toolhelp32 plus creation times.
    fn table(&self) -> ProcessTable;
    /// Same user only; the PEB read of §4.2.
    fn config_dir_env(&self, pid: u32) -> EnvRead;
    fn same_user(&self, pid: u32) -> Option<bool>;
    /// `TokenElevation`; `None` when it can't be told.
    fn elevated(&self, pid: u32) -> Option<bool>;
    /// `QueryFullProcessImageNameW`.
    fn exe_path(&self, pid: u32) -> Option<PathBuf>;
}

/// A child to run: the runner clears the environment and sets exactly `env`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The COMPLETE environment.
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    Code(i32),
    Killed,
}

pub trait RunningCommand: Send {
    fn pid(&self) -> u32;
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>>;
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    fn wait_timeout(&mut self, d: Duration) -> io::Result<Option<Exit>>;
    /// Ends the whole tree (`TerminateJobObject`); idempotent.
    fn kill_tree(&mut self);
}

/// Children in a Job object (kill on close), no window, piped stdio.
pub trait CommandRunner: Send + Sync {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>>;
}

/// A pipe connection's id.
pub type ConnId = u64;

/// One frame read from a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingFrame {
    pub conn: ConnId,
    pub bytes: Vec<u8>,
    pub received_at: SystemTime,
    pub peer_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportEvent {
    Frame(IncomingFrame),
    /// A held connection's peer went away (the hook ended).
    PeerClosed(ConnId),
    /// Listening on this pipe name.
    Listening(String),
    Error(String),
}

/// The hook pipe's server.
pub trait HookTransport: Send + Sync {
    /// Starts listening; events go to `sink` in arrival order.
    fn start(
        &self,
        pipe_name: &str,
        sink: crossbeam_channel::Sender<TransportEvent>,
    ) -> Result<(), String>;
    /// Writes the frame (2 s), then closes. False when the peer is gone.
    fn respond(&self, conn: ConnId, frame_json: Vec<u8>) -> bool;
    /// Closes without answering (the terminal's own prompt decides).
    fn close(&self, conn: ConnId);
    fn stop(&self);
}

/// What hosts a Claude process's console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    WindowsTerminal,
    Conhost,
    VsCode { product: String },
    JetBrains,
    OtherConsoleHost { exe: String },
    NoConsole,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostApp {
    pub kind: HostKind,
    /// HWND.
    pub window: Option<u64>,
    pub host_pid: Option<u32>,
    pub exe_path: Option<PathBuf>,
}

/// One step of a jump to the terminal (§4.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusStep {
    SelectWtTab {
        window: u64,
        title: String,
    },
    RaiseWindow {
        window: u64,
    },
    OpenInEditor {
        editor_exe: PathBuf,
        folder: PathBuf,
    },
    ActivatePid {
        pid: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusOutcome {
    Focused,
    RaisedOnly,
    NotFound,
    Failed(String),
}

/// The foreground window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Foreground {
    pub pid: u32,
    pub window: u64,
    pub title: String,
    pub fullscreen: bool,
}

pub trait Terminals: Send + Sync {
    fn classify_host(&self, claude_pid: u32, table: &ProcessTable) -> HostApp;
    /// Via `agentnotch-hook.exe console-info`.
    fn console_info(&self, claude_pid: u32) -> ConsoleInfo;
    fn run_focus(&self, step: &FocusStep) -> FocusOutcome;
    fn foreground(&self) -> Option<Foreground>;
    fn window_title(&self, window: u64) -> Option<String>;
    /// Windows Terminal's tab names and which is selected (UI Automation).
    fn wt_tab_titles(&self, window: u64) -> Option<Vec<(String, bool)>>;
    /// EnumWindows, not cloaked or minimised.
    fn any_terminal_visible(&self) -> bool;
    /// `SetWinEventHook` on its own thread (`an-foreground`).
    fn watch_foreground(&self, sink: crossbeam_channel::Sender<Foreground>);
}

/// How a typed reply ended (§4.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeOutcome {
    Delivered,
    Refused(String),
    TypedNotSubmitted(String),
    Failed(String),
}

/// Where a reply is typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleTarget {
    pub claude_pid: u32,
    pub claude_started: SystemTime,
    pub expected_window: Option<u64>,
    /// Claude's direct parent chain of known shells, validated by creation time.
    pub allowed_shells: Vec<u32>,
}

/// Via `agentnotch-hook.exe type` (§4.8).
pub trait ConsoleInput: Send + Sync {
    /// Types `text`, then calls `recheck` (the engine's fresh
    /// `message_safety`); presses Return only when it returns true within
    /// 2 s, else reports `TypedNotSubmitted`.
    fn type_text(
        &self,
        target: &ConsoleTarget,
        text: &str,
        recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastKind {
    NeedsInput,
    Review,
    Failed,
    Limit,
}

/// A WinRT toast (§4.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toast {
    /// ≤ 64 characters.
    pub tag: String,
    /// ≤ 64 characters.
    pub group: String,
    pub kind: ToastKind,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    /// `agentnotch://open?…` (protocol activation).
    pub launch_url: String,
    /// (label, `agentnotch://` url).
    pub actions: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyPermission {
    Allowed,
    DisabledForApp,
    DisabledForUser,
    DisabledByPolicy,
    /// No Start-menu shortcut with the AUMID (a dev or portable run).
    Unavailable,
}

pub trait Notifier: Send + Sync {
    fn post(&self, t: &Toast);
    fn withdraw(&self, tag: &str, group: &str);
    fn permission(&self) -> NotifyPermission;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chime {
    Blocked,
    Finished,
}

pub trait Sounds: Send + Sync {
    fn play(&self, c: Chime);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpError {
    Timeout,
    Connect(String),
    Tls(String),
    Other(String),
}

/// No cookies, no cache.
pub trait Http: Send + Sync {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
}

pub trait Browser: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

/// Which file a path named at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub volume: u64,
    pub index: u128,
    pub modified_ns: i128,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Protected DACL: the user and SYSTEM only.
    Private,
    /// Copy the replaced file's DACL, protection flag and attributes; a new
    /// file inherits its folder's.
    KeepTargetSecurity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// No check.
    Nothing,
    /// Must not exist.
    Absent,
    /// Must still be exactly this file.
    Same(FileIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteResult {
    Written,
    /// Not what was expected at the last moment: nothing written.
    Changed,
    /// Expected an existing file and it is gone: nothing written.
    Vanished,
    ReadOnly,
}

/// Files with their security, written so they are never lost or readable by
/// others, even briefly.
pub trait SecureFiles: Send + Sync {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()>;
    /// Stage `.<name>.agentnotch-<8 hex>.tmp` beside `path` (CREATE_NEW,
    /// same security as the mode asks), write, flush, re-check `expect`,
    /// then one atomic rename over the target:
    /// `SetFileInformationByHandle(FileRenameInfoEx, REPLACE_IF_EXISTS |
    /// POSIX_SEMANTICS)`, falling back to `MoveFileExW(REPLACE_EXISTING |
    /// WRITE_THROUGH)` where the class is unsupported. A failed rename leaves
    /// the target untouched (all or nothing); ≤ 5 retries on sharing/access
    /// errors over ~500 ms; the stage is always deleted on failure. `path`
    /// must be already resolved (canonical); callers resolve links first.
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult>;
    /// False when the file existed.
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool>;
    fn identity(&self, path: &Path) -> io::Result<FileIdentity>;
    /// A symlink or junction; never follows.
    fn is_reparse(&self, path: &Path) -> io::Result<bool>;
    /// `GetFinalPathNameByHandleW`, without a `\\?\` prefix.
    fn canonical(&self, path: &Path) -> io::Result<PathBuf>;
    /// Protected, only the owner (and SYSTEM).
    fn is_private(&self, path: &Path) -> io::Result<bool>;
    /// The 8.3 spelling of an existing path (`GetShortPathNameW`), for a hook
    /// command whose long path can't be written unquoted. `None` where the
    /// volume keeps no short names, and on every other OS.
    fn short_path(&self, path: &Path) -> Option<PathBuf> {
        let _ = path;
        None
    }
    /// The long spelling of a path that may hold 8.3 names
    /// (`GetLongPathNameW`), so a hook command written with one is still
    /// recognised as ours. `None` when there is nothing to look up.
    fn long_path(&self, path: &Path) -> Option<PathBuf> {
        let _ = path;
        None
    }
}

pub trait Device: Send + Sync {
    fn computer_name(&self) -> String;
    fn user_sid(&self) -> Option<String>;
    /// This app's own token.
    fn elevated(&self) -> bool;
    /// `on` | `off` | `evaluation` (the doctor only).
    fn smart_app_control(&self) -> Option<String>;
}

/// The whole set of services one engine runs on.
#[derive(Clone)]
pub struct Platform {
    pub clock: Arc<dyn Clock>,
    pub processes: Arc<dyn Processes>,
    pub runner: Arc<dyn CommandRunner>,
    pub transport: Arc<dyn HookTransport>,
    pub terminals: Arc<dyn Terminals>,
    pub console: Arc<dyn ConsoleInput>,
    pub notifier: Arc<dyn Notifier>,
    pub sounds: Arc<dyn Sounds>,
    pub http: Arc<dyn Http>,
    pub browser: Arc<dyn Browser>,
    pub files: Arc<dyn SecureFiles>,
    pub device: Arc<dyn Device>,
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn monotonic(&self) -> Instant {
        Instant::now()
    }
}
