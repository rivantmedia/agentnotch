//! The types that cross between the engine's packages (DESIGN-WIN §3.4),
//! written out here so the packages can work in parallel. Each service is a
//! plain struct driven by `an-core` with an explicit `now`; blocking work is
//! a [`Job`] a worker lane runs, whose [`JobResult`] comes back as
//! [`Input::JobDone`]. Packages may add fields; renaming or removing one goes
//! through the lead.

use crate::hub::{Call, CallError};
use crate::model::*;
use crate::persist::hook_install::{HookInstallEntry, HookInstallRecord};
use crate::platform::{
    Chime, CommandSpec, ConnId, ConsoleInfo, ConsoleTarget, FileIdentity, FocusOutcome, FocusStep,
    Foreground, HostApp, NotifyPermission, Toast, TransportEvent, TypeOutcome,
};
use agentnotch_proto::ControlOp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

// ---- runtime (WP0 defines, WP7 runs the loop) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub u64);

/// The worker lanes (§1.2): a slow `claude` never starves file jobs, and a
/// hung Windows Terminal only delays other UI jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// `an-io-{0..2}`.
    Io,
    /// `an-probe`.
    Probe,
    /// `an-ui`.
    Ui,
}

/// Blocking work, with its lane and result variant.
// Moved once through a channel or a call; boxing variants would change the
// §3 signatures every package codes against.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    /// [Io] → `Folders`.
    ReadFolders { explicit: Vec<PathBuf> },
    /// [Io] → `ClaudeJson`.
    ReadClaudeJson { folder: AccountId, path: PathBuf },
    /// [Io] → `Registry`.
    ReadRegistry {
        sessions_dir: PathBuf,
        via_link: bool,
    },
    /// [Io] → `Transcript`.
    SyncTranscript {
        session: SessionId,
        path: PathBuf,
        cursor: TranscriptCursor,
    },
    /// [Io] → `Chat`.
    LoadChat {
        session: SessionId,
        path: PathBuf,
        before: Option<String>,
    },
    /// [Io] → `Hosted`.
    DesktopHosted {
        roots: Vec<PathBuf>,
        host_session_id: String,
        /// Added by WP5: the known identities whose record is looked for
        /// (a few `lstat`s each, as on the Mac; without them the job would
        /// have to list Desktop's whole folder).
        candidates: Vec<DesktopCandidate>,
    },
    /// [Io] → `Desktop`.
    ReadDesktopCache { organization_uuid: String },
    /// [Io] → `Installed`.
    Install { plans: Vec<InstallPlan> },
    /// [Io] → `Installed`.
    Uninstall {
        record: HookInstallRecord,
        folders: Vec<RunFolder>,
    },
    /// [Io] → `Persisted`.
    Persist { file: PersistFile, bytes: Vec<u8> },
    /// [Probe] → `Probe`.
    Probe(ProbePlan),
    /// [Probe] → `Versions`.
    Versions {
        binaries: Vec<PathBuf>,
        bundled: Vec<PathBuf>,
    },
    /// [Ui] → `Console`.
    ConsoleInfo { pid: u32, started: SystemTime },
    /// [Ui] → `Host`.
    Classify { pid: u32 },
    /// [Ui] → `Focus`.
    Focus { steps: Vec<FocusStep> },
    /// [Ui] → `Typed`; sends `Input::TypeCheckpoint` between the text and Return.
    Type {
        session: SessionId,
        target: ConsoleTarget,
        text: String,
    },
    /// [Ui] → `Visible`.
    Visibility,
    /// [Io] → `HookStatus`. Added by WP7: what each folder's settings.json
    /// registers, read back (read-only, so also before consent).
    ReadHookStatus { folders: Vec<(AccountId, PathBuf)> },
    /// [Io] → `CodenotchRemoved`. Added by WP7: the official app's entries
    /// out of one settings.json, on the user's click.
    RemoveCodenotchHooks {
        folder: AccountId,
        settings_path: PathBuf,
    },
}

impl Job {
    pub fn lane(&self) -> Lane {
        match self {
            Job::Probe(_) | Job::Versions { .. } => Lane::Probe,
            Job::ConsoleInfo { .. }
            | Job::Classify { .. }
            | Job::Focus { .. }
            | Job::Type { .. }
            | Job::Visibility => Lane::Ui,
            _ => Lane::Io,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobResult {
    Folders(FolderSnapshot),
    ClaudeJson(ClaudeJsonRead),
    Registry(RegistrySnapshot),
    Transcript(TranscriptDelta),
    Chat(ChatPage),
    Hosted(Option<IdentityId>),
    Desktop(DesktopReading),
    Installed(Vec<InstallOutcome>),
    Persisted(Result<(), String>),
    Probe(ProbeResult),
    Versions(Vec<VersionSighting>),
    Console(ConsoleInfo),
    Host(HostApp),
    Focus(FocusOutcome),
    Typed(TypeOutcome),
    Visible {
        any_terminal: bool,
        full_screen: bool,
    },
    /// Added by WP7 (`Job::ReadHookStatus`).
    HookStatus(Vec<(AccountId, FolderHookStatus)>),
    /// Added by WP7 (`Job::RemoveCodenotchHooks`): how many entries went.
    CodenotchRemoved {
        folder: AccountId,
        result: Result<u32, String>,
    },
}

/// The engine's own files in `<support>` (§1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistFile {
    Accounts,
    Review,
    Usage,
    Settings,
    HookInstall,
}

impl PersistFile {
    pub fn file_name(self) -> &'static str {
        match self {
            PersistFile::Accounts => crate::persist::accounts::FILE_NAME,
            PersistFile::Review => crate::persist::review::FILE_NAME,
            PersistFile::Usage => crate::persist::usage::FILE_NAME,
            PersistFile::Settings => crate::persist::settings::FILE_NAME,
            PersistFile::HookInstall => crate::persist::hook_install::FILE_NAME,
        }
    }
}

/// Everything `an-core` consumes, in arrival order.
// Moved once through a channel or a call; boxing variants would change the
// §3 signatures every package codes against.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Input {
    Transport(TransportEvent),
    Call {
        call: Call,
        reply: crossbeam_channel::Sender<Result<serde_json::Value, CallError>>,
    },
    JobDone {
        id: JobId,
        result: JobResult,
    },
    /// A fresh `message_safety` before Return (§4.8).
    TypeCheckpoint {
        job: JobId,
        reply: crossbeam_channel::Sender<bool>,
    },
    Foreground(Foreground),
    /// From `an-cloud` (and the glue): `an-core` is the only settings writer.
    SetSetting {
        key: String,
        value: serde_json::Value,
    },
    /// `an-cloud` → projections.
    CloudState(CloudState),
    /// The next deadline passed.
    Tick,
    Stop {
        done: crossbeam_channel::Sender<()>,
    },
}

/// A `.claude.json` read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeJsonRead {
    pub folder: AccountId,
    pub identity: Option<Identity>,
    /// `cachedUsageUtilization`, only when it belongs to the signed-in login.
    pub cached_usage: Option<AccountUsage>,
    /// (mtime ns, size) of what was parsed.
    pub stamp: Option<(i128, u64)>,
    pub error: Option<String>,
}

/// A Claude Code version seen somewhere (§4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSighting {
    pub source: VersionSource,
    pub path: Option<PathBuf>,
    /// `None`: seen but unknown (keeps string form).
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSource {
    Binary,
    BundledVsCode,
    BundledDesktop,
    Registry,
    StatusLine,
}

/// How far a transcript has been read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCursor {
    pub offset: u64,
    pub size: u64,
    pub file: Option<FileIdentity>,
}

/// New complete lines of a transcript, as the session pipeline needs them
/// (HS§5.8-5.11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDelta {
    pub session: SessionId,
    pub path: PathBuf,
    pub cursor: TranscriptCursor,
    /// The file shrank or was replaced: state is rebuilt from 0.
    pub reset: bool,
    pub entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEntry {
    Assistant {
        uuid: String,
        at: Option<SystemTime>,
        text: Option<String>,
        model: Option<String>,
        usage: Option<TokenUsage>,
        sidechain: bool,
        synthetic: bool,
    },
    HumanPrompt {
        uuid: String,
        at: Option<SystemTime>,
    },
    Interrupt {
        at: Option<SystemTime>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        /// `success` | `error` | `interrupted`.
        status: String,
        task_id: Option<String>,
    },
    Title {
        /// `custom` | `ai` | `summary`.
        kind: String,
        text: String,
    },
    Clear,
    // The variants below were added by WP5: the chat's incremental updates
    // and the transcript summary need them, and the sync job keeps no state
    // between reads (sessions::transcript). A subagent's own transcript
    // (`agent-<id>.jsonl`) is read by the same job; its deltas carry only
    // `ToolUse` and `ToolResult`, sidechain lines included.
    /// The text of the human prompt just before (`HumanPrompt` carries only
    /// its time): its first 200 characters, all the summary shows.
    PromptText {
        text: String,
    },
    /// A user line that isn't a person's words (a wake-up, a compact
    /// summary, a command echo): Claude works on.
    Injected {
        at: Option<SystemTime>,
    },
    /// A user or assistant line's chat blocks.
    Message(ChatMessage),
    /// A tool result's text and raw `toolUseResult`, for the chat.
    ToolOutput(ToolOutput),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

// ---- ingress (WP1) ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngressConfig {
    pub pipe_name: String,
    /// 60 min.
    pub tool_use_cache_ttl: Duration,
    /// true.
    pub ignore_sdk_entrypoints: bool,
    /// 512.
    pub max_connections: usize,
}

impl IngressConfig {
    pub fn new(pipe_name: impl Into<String>) -> Self {
        IngressConfig {
            pipe_name: pipe_name.into(),
            tool_use_cache_ttl: Duration::from_secs(60 * 60),
            ignore_sdk_entrypoints: true,
            max_connections: agentnotch_proto::limits::MAX_CONNECTIONS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngressOut {
    Hook(HookEvent),
    StatusLine(StatusLineMessage),
    PermissionHeld(HeldPermission),
    PermissionFailed {
        session: SessionId,
        tool_use_id: String,
    },
    Control {
        conn: ConnId,
        op: ControlOp,
    },
    /// `Ok(pipe name)` when listening, `Err(why)` otherwise.
    TransportStatus(Result<String, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerResult {
    Delivered,
    NotPending,
    PeerGone,
}

/// Which held PermissionRequests to close without an answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Release {
    /// PostToolUse / PostToolUseFailure / PermissionDenied for that id.
    Request {
        session: SessionId,
        tool_use_id: String,
    },
    /// Stop, main UserPromptSubmit, registry idle, interrupt.
    MainAgent(SessionId),
    /// SubagentStop.
    Agent {
        session: SessionId,
        agent_id: String,
    },
    /// SessionStart, SessionEnd, StopFailure, untracked.
    Session(SessionId),
    /// App quit, hub stop, sealed.
    All,
}

// ---- sessions (WP5) ----

/// What the hub knows about where an event came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestContext {
    pub attribution: Attribution,
    pub account: Option<AccountId>,
    /// The frame's pid, checked against the hooks' pid (CLAUDE.md).
    pub trusted_pid: Option<u32>,
    pub pid_started: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionInput {
    Hook {
        event: HookEvent,
        ctx: IngestContext,
    },
    Held(HeldPermission),
    /// The hook's pipe closed while held.
    PermissionFailed {
        session: SessionId,
        tool_use_id: String,
    },
    PermissionResolved {
        session: SessionId,
        tool_use_id: String,
        answer: Answer,
    },
    StatusLine {
        message: StatusLineMessage,
        ctx: IngestContext,
    },
    Registry(RegistrySnapshot),
    TranscriptSynced(TranscriptDelta),
    Hosted {
        session: SessionId,
        identity: Option<IdentityId>,
    },
    Interrupt {
        session: SessionId,
        at: SystemTime,
    },
    Review(ReviewAction),
    AccountsChanged(AccountsChanged),
    Tick,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    MarkReviewed {
        session: SessionId,
        at: SystemTime,
    },
    MarkViewed {
        session: SessionId,
        completed_at: SystemTime,
    },
    MarkAll {
        sessions: Vec<SessionId>,
        at: SystemTime,
    },
    DismissFailure(SessionId),
    Reset,
    HooksTurnedOff,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionEffects {
    pub release: Vec<Release>,
    pub jobs: Vec<Job>,
    pub sightings: Vec<AccountSighting>,
    pub transitions: Vec<AttentionTransition>,
    pub changed: bool,
    /// Save review-state.json after this delay (`ZERO`: now).
    pub persist_review: Option<Duration>,
}

// ---- accounts (WP3) ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsChanged {
    pub rings: bool,
    pub folders: bool,
    pub identities: bool,
    pub new_run_folders: Vec<AccountId>,
    pub removed_folders: Vec<AccountId>,
}

impl AccountsChanged {
    pub fn any(&self) -> bool {
        self.rings
            || self.folders
            || self.identities
            || !self.new_run_folders.is_empty()
            || !self.removed_folders.is_empty()
    }
}

// ---- usage (WP4) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshReason {
    Launch,
    Interval,
    RingClick,
    Manual,
    NewAccount,
}

/// A `claude` to run: the native exe, or Node with the npm package's
/// `cli.js` as its first argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeBinary {
    pub program: PathBuf,
    /// Node: `[<…>\cli.js]`.
    pub prefix_args: Vec<OsString>,
    pub version: Option<String>,
    /// A `.cmd` shim run as is (Rust std escapes or refuses its arguments).
    pub shim: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbePlan {
    pub identity: IdentityId,
    pub folder: AccountId,
    pub config_dir: PathBuf,
    pub config_dir_env: Option<String>,
    pub binary: ClaudeBinary,
    /// argv, the scrubbed environment (§4.6), cwd `<support>\usage-probe`.
    pub spec: CommandSpec,
    pub reason: RefreshReason,
    pub planned_at: SystemTime,
    /// The `.claude.json` that names who the folder is signed in as
    /// (`~\.claude.json` for the default folder), read right before and
    /// after the run.
    pub identity_file: PathBuf,
    /// Who that file must name for the check to run and its answer to count
    /// (AU§9.9's folder-changed-hands rule).
    pub expected: ExpectedLogin,
}

// Moved once through a channel or a call; boxing variants would change the
// §3 signatures every package codes against.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Reading(AccountUsage),
    RateLimited { retry_after: Option<Duration> },
    SignedOut,
    Unavailable(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub plan: ProbePlan,
    pub outcome: ProbeOutcome,
    pub started: SystemTime,
    pub finished: SystemTime,
    /// The folder-changed-hands re-check (AU§10).
    pub folder_identity_after: Option<IdentityId>,
}

// Moved once through a channel or a call; boxing variants would change the
// §3 signatures every package codes against.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingReading {
    Reading {
        usage: AccountUsage,
        status: RingStatus,
        stale_after: SystemTime,
    },
    Waiting,
    SignInNeeded,
    Unavailable(String),
    Failed(String),
}

/// A reading for the cloud's usage outbox (CL§8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageObservation {
    pub identity: IdentityId,
    pub source: UsageSource,
    pub observed_at: SystemTime,
    /// (contract window id, utilization, resets at).
    pub windows: Vec<(String, f64, Option<SystemTime>)>,
}

// ---- hooks (WP2) ----

/// How a hook command is written into settings.json (§4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandForm {
    /// `{"command": <exe>, "args": […]}`.
    Exec { command: PathBuf, args: Vec<String> },
    /// A string that parses in Git Bash and PowerShell.
    Text(String),
    /// Why, shown per folder.
    NotPossible(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLineIntent {
    Wrap,
    UpdateCommand,
    Unwrap,
    /// The reason shown in Settings.
    LeaveAlone(String),
    Nothing,
}

/// One settings.json to write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallPlan {
    pub folder: AccountId,
    /// Resolved.
    pub settings_path: PathBuf,
    pub expected: crate::platform::Expect,
    pub existed: bool,
    /// (source, `<cfg>\hooks\agentnotch-hook.exe`).
    pub hook_copy: Option<(PathBuf, PathBuf)>,
    pub form: CommandForm,
    pub events: Vec<String>,
    pub status_line: StatusLineIntent,
    pub remove_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallOutcome {
    pub folder: AccountId,
    pub settings_path: PathBuf,
    pub result: Result<InstallChange, String>,
    pub backup: Option<PathBuf>,
    /// What settings.json now holds of ours, for `hook-install.json`: set
    /// after an install that left our hooks in the file (written or already
    /// current), `None` after a removal or a failure.
    #[serde(default)]
    pub entry: Option<HookInstallEntry>,
    /// What was decided for the folder's status line, with the reason when
    /// it was left alone. `None` when the pass never got that far.
    #[serde(default)]
    pub status_line: Option<StatusLineIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallChange {
    Unchanged,
    Written,
    Removed,
    /// e.g. "settings.json disappeared while writing".
    Aborted(String),
}

/// What a folder's settings.json says about our hooks (HS§3.8).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderHookStatus {
    pub config_dir_exists: bool,
    pub settings_readable: bool,
    /// settings.json couldn't be read because another program holds it
    /// right now: it isn't broken, only busy. `settings_readable` is false
    /// as well (nothing is known of what it holds); say this one first.
    #[serde(default)]
    pub settings_in_use: bool,
    /// Our entries are registered.
    pub hooks_registered: bool,
    /// …and the hook copy is in place.
    pub hooks_installed: bool,
    pub status_line_installed: bool,
    /// Why the status line was left alone.
    pub status_line_left_alone: Option<String>,
    /// The official Codenotch's entries are there.
    pub codenotch_hooks: bool,
    /// The form written (`exec` | `string`), when ours are there.
    pub form: Option<String>,
    /// Why no hook command can run here, when none can.
    pub not_hookable: Option<String>,
    pub hook_command: Option<String>,
    pub newest_backup: Option<PathBuf>,
    pub last_outcome: Option<InstallChange>,
    pub last_error: Option<String>,
}

// ---- control (WP6) ----

/// The panel window as the glue reports it (`Call::PanelState`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelState {
    pub open: bool,
    pub route: Option<String>,
    pub ring_id: Option<String>,
    pub reason: Option<String>,
    /// The pointer is inside, or a text field has focus.
    pub engaged: bool,
    /// Confirmed foreground.
    pub focused: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToastContext {
    pub now: SystemTime,
    pub notify_needs_input: bool,
    pub notify_ready_for_review: bool,
    pub permission: NotifyPermission,
    /// Sealed, `AGENTNOTCH_NO_NOTIFICATIONS`, full screen.
    pub suppressed: bool,
    pub looking_at: Option<bool>,
    pub account_label: Option<String>,
    pub multi_account: bool,
    pub title: String,
    pub project: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReactionContext {
    pub now: SystemTime,
    /// `never` | `needsInput` | `needsInputOrDone`.
    pub auto_open: String,
    pub sound: bool,
    pub peek: bool,
    pub peek_seconds: u32,
    pub panel: PanelState,
    pub full_screen: bool,
    /// A terminal or editor window is on screen and not covered
    /// (`JobResult::Visible`): the panel then peeks instead of opening by
    /// itself (UI§3.8).
    pub any_terminal_visible: bool,
    pub looking_at: Option<bool>,
    pub ring_shown: bool,
    pub notch_hidden: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reactions {
    pub chime: Option<Chime>,
    pub peek: Option<(RingId, u32)>,
    pub open_panel: Option<PanelRequest>,
    pub toasts: Vec<Toast>,
    /// (tag, group).
    pub withdraw: Vec<(String, String)>,
}

// ---- cloud (WP8): its own thread ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudConfig {
    pub support: PathBuf,
    pub website: Option<String>,
    pub website_is_overridden: bool,
    pub app_version: String,
    pub device_name: String,
    /// From the settings; `an-core` mints it.
    pub device_id: String,
    pub sync_enabled: bool,
    pub summaries_enabled: bool,
    pub summaries_allowed_by_default: bool,
    pub sealed: bool,
    pub system_users: Option<PathBuf>,
    pub home: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudCall {
    SignIn,
    CancelSignIn,
    SignOut,
    SetSync(bool),
    SetSummaries(bool),
    SyncNow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudFolder {
    pub config_dir: PathBuf,
    pub config_dir_env: Option<String>,
    pub organization_uuid: Option<String>,
    /// A mirrored folder whose organisation is stale: never names the org.
    pub corrected: bool,
    pub kind: FolderKind,
    pub is_default: bool,
    pub mirrored_default: bool,
}

/// An identity the website may hear of. `accountKey` =
/// `CloudKeys.accountKey(identity)` from these folders (CLAUDE.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudAccount {
    pub identity_id: IdentityId,
    pub email: Option<String>,
    pub organization_name: Option<String>,
    pub plan: Option<String>,
    pub label: Option<String>,
    pub folders: Vec<CloudFolder>,
}

/// The running sessions, built by the hub from `SessionView`s (CL§6.1,
/// `PLACEMENT_GRACE`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveBatch {
    pub attributed: Vec<LiveSessionObservation>,
    pub unsure: BTreeSet<String>,
    pub waiting: BTreeSet<String>,
    pub live_ids: BTreeSet<String>,
    pub at: SystemTime,
}

/// What the cloud thread reads from the rest of the engine. Implemented by
/// the hub over an immutable view that `an-core` republishes after every
/// projection, so a call never waits on `an-core`.
pub trait CloudDeps: Send + Sync {
    /// Visible, remembered, signed-in identities (CL§0.1).
    fn accounts(&self) -> Vec<CloudAccount>;
    /// `None` until the registry has read its folders.
    fn folder_logins(&self) -> Option<BTreeMap<String, String>>;
    /// CL§7.2.
    fn backfill_folders(&self) -> Vec<BackfillFolder>;
    /// Summaries skip ≥ 80 %.
    fn five_hour(&self, identity: &IdentityId) -> Option<f64>;
    /// `probe_folder`'s rules (CL§9.1).
    fn summary_folder(&self, identity: &IdentityId) -> Option<RunFolder>;
    fn summary_folder_still_runs(&self, folder: &RunFolder, identity: &IdentityId) -> bool;
    /// A probe is running.
    fn is_launching_claude(&self) -> bool;
    fn claude_binary(&self) -> Option<ClaudeBinary>;
    /// → `Input::SetSetting`.
    fn set_setting(&self, key: &str, value: serde_json::Value);
}
