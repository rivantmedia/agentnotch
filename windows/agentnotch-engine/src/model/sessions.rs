//! Sessions as the pipeline sees them (HS§5; SessionState.swift,
//! SessionPhase.swift, SessionAttention.swift).

use crate::model::{
    AccountId, ChatItem, HookTerminal, IdentityId, PendingRequest, RingId, SessionId,
};
use crate::platform::EnvRead;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// The session's lifecycle phase (HS§5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Processing,
    WaitingForInput,
    WaitingForApproval(PermissionContext),
    Compacting,
    Ended,
}

/// The active approval of a session in `WaitingForApproval`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionContext {
    pub tool_use_id: String,
    pub tool_name: String,
    /// Full, nested.
    pub tool_input: Value,
    pub received_at: SystemTime,
    pub permission_suggestions: Vec<Value>,
    pub has_synthetic_tool_use_id: bool,
    /// `None`: the main session asked.
    pub agent_id: Option<String>,
    /// When it became the active request (queued requests activate later).
    pub activated_at: Option<SystemTime>,
}

impl PermissionContext {
    /// Longest one-line preview.
    pub const PREVIEW_LENGTH: usize = 100;

    /// Asked by a subagent rather than the main session.
    pub fn is_from_subagent(&self) -> bool {
        self.agent_id.is_some()
    }

    /// Whether "always allow" can be offered (Claude Code sent a suggestion
    /// to apply).
    pub fn can_always_allow(&self) -> bool {
        !self.permission_suggestions.is_empty()
    }

    fn flat_input(&self) -> BTreeMap<String, String> {
        crate::sessions::tool_input::flatten_value(&self.tool_input)
    }

    /// One-line preview of the input, at most 100 characters (plus "...").
    /// For Write/Edit/Read only the file name: show [`Self::full_input`]
    /// before offering Allow on anything longer than the preview.
    pub fn formatted_input(&self) -> Option<String> {
        crate::sessions::tool_input::preview(
            &self.tool_name,
            &self.flat_input(),
            Some(Self::PREVIEW_LENGTH),
        )
    }

    /// What may be shown of the input outside the panel (hover rows, the
    /// phone, banners).
    pub fn off_panel_input(&self) -> Option<String> {
        crate::sessions::tool_input::off_panel_preview(
            &self.tool_name,
            &self.flat_input(),
            Some(Self::PREVIEW_LENGTH),
        )
    }

    /// The whole thing being approved, untruncated: the full command, or the
    /// full path (with `~` for the home folder) for file tools.
    pub fn full_input(&self, paths: &crate::core::paths::Paths) -> Option<String> {
        let input = self.flat_input();
        match self.tool_name.as_str() {
            "Write" | "Edit" | "MultiEdit" | "Read" | "NotebookEdit" => input
                .get("file_path")
                .or_else(|| input.get("notebook_path"))
                .map(|path| paths.abbreviate(path)),
            tool => crate::sessions::tool_input::preview(tool, &input, None),
        }
    }

    /// The preview hides part of the input (a long or multi-line command, a
    /// path shortened to its file name).
    pub fn is_preview_truncated(&self, paths: &crate::core::paths::Paths) -> bool {
        match self.full_input(paths) {
            Some(full) => Some(&full) != self.formatted_input().as_ref() || full.contains('\n'),
            None => false,
        }
    }

    /// What the first permission suggestion ("Always allow") would do.
    pub fn always_allow_suggestion(&self) -> Option<PermissionSuggestion> {
        self.permission_suggestions
            .first()
            .and_then(PermissionSuggestion::from_json)
    }
}

/// Claude Code's PermissionUpdate: what "Yes, and don't ask again" saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionSuggestion {
    /// addRules, replaceRules, removeRules, setMode, addDirectories,
    /// removeDirectories.
    pub kind: String,
    /// session, localSettings, projectSettings, userSettings (or none).
    pub destination: Option<String>,
    /// For setMode: acceptEdits, bypassPermissions, plan, default.
    pub mode: Option<String>,
}

impl PermissionSuggestion {
    pub fn from_json(json: &Value) -> Option<PermissionSuggestion> {
        let field = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_owned);
        let kind = field("type").filter(|kind| !kind.is_empty())?;
        Some(PermissionSuggestion {
            kind,
            destination: field("destination"),
            mode: field("mode"),
        })
    }

    /// A rule for this session or for this project and user only. Anything
    /// else (a permission mode, a rule shared with the team or for every
    /// project) should be offered only where the user can read it in full.
    pub fn is_narrow(&self) -> bool {
        matches!(self.kind.as_str(), "addRules" | "replaceRules")
            && matches!(
                self.destination.as_deref(),
                Some("session" | "localSettings")
            )
    }

    /// Changes how Claude Code asks from now on rather than allowing a rule.
    pub fn changes_permission_mode(&self) -> bool {
        self.kind == "setMode"
    }
}

/// Why a session is blocked on the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsInputReason {
    /// A tool waits for permission (`tool: None` when only a notification
    /// said so and named none).
    Permission { tool: Option<String> },
    /// AskUserQuestion.
    Question,
    /// ExitPlanMode.
    PlanApproval,
    /// An MCP server asked for input; `""` when its message is unknown.
    Elicitation { message: String },
    /// Another dialog is open in the terminal; `""` when unknown.
    Dialog { detail: String },
    /// The turn failed (StopFailure): humanised text and the raw code.
    Error { text: String, code: Option<String> },
}

impl NeedsInputReason {
    /// A failed turn, as opposed to something to answer.
    pub fn is_error(&self) -> bool {
        matches!(self, NeedsInputReason::Error { .. })
    }

    /// The reason for a pending tool approval.
    pub fn for_approval(tool_name: &str) -> NeedsInputReason {
        match tool_name {
            "AskUserQuestion" => NeedsInputReason::Question,
            "ExitPlanMode" => NeedsInputReason::PlanApproval,
            tool => NeedsInputReason::Permission {
                tool: Some(tool.to_owned()),
            },
        }
    }

    /// Short user-facing text ("Approve Bash", "Rate limited").
    pub fn display_text(&self) -> String {
        match self {
            NeedsInputReason::Permission { tool: Some(tool) } if !tool.is_empty() => {
                format!("Approve {tool}")
            }
            NeedsInputReason::Permission { .. } => "Needs permission".into(),
            NeedsInputReason::Question => "Question for you".into(),
            NeedsInputReason::PlanApproval => "Review plan".into(),
            NeedsInputReason::Elicitation { message } if !message.is_empty() => message.clone(),
            NeedsInputReason::Elicitation { .. } => "Input requested".into(),
            NeedsInputReason::Dialog { detail } if !detail.is_empty() => {
                let mut chars = detail.chars();
                let first = chars
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>())
                    .unwrap_or_default();
                first + chars.as_str()
            }
            NeedsInputReason::Dialog { .. } => "Waiting for you".into(),
            NeedsInputReason::Error { text, .. } => text.clone(),
        }
    }
}

/// The five user-facing states (HS§5.3), in priority order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// An answerable reason.
    NeedsYou(NeedsInputReason),
    /// A failed turn (`NeedsInputReason::Error`).
    Failed(NeedsInputReason),
    ReadyForReview,
    Working,
    Idle,
}

impl SessionState {
    pub fn bucket(&self) -> Bucket {
        match self {
            SessionState::NeedsYou(_) | SessionState::Failed(_) => Bucket::NeedsYou,
            SessionState::ReadyForReview => Bucket::ReadyForReview,
            SessionState::Working => Bucket::Working,
            SessionState::Idle => Bucket::Idle,
        }
    }

    /// The reason it needs the user, failed turns included.
    pub fn reason(&self) -> Option<&NeedsInputReason> {
        match self {
            SessionState::NeedsYou(reason) | SessionState::Failed(reason) => Some(reason),
            _ => None,
        }
    }
}

/// List sections, in order. Failed sessions sort inside `NeedsYou`, after
/// the answerable ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    NeedsYou,
    ReadyForReview,
    Working,
    Idle,
}

impl Bucket {
    pub const ALL: [Bucket; 4] = [
        Bucket::NeedsYou,
        Bucket::ReadyForReview,
        Bucket::Working,
        Bucket::Idle,
    ];

    /// The name the UI uses (`SessionRow::bucket`, `SectionInfo::bucket`).
    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::NeedsYou => "needs_you",
            Bucket::ReadyForReview => "ready_for_review",
            Bucket::Working => "working",
            Bucket::Idle => "idle",
        }
    }

    /// The section title (UI§5.3).
    pub fn title(self) -> &'static str {
        match self {
            Bucket::NeedsYou => "Needs you",
            Bucket::ReadyForReview => "Ready for review",
            Bucket::Working => "Working",
            Bucket::Idle => "Idle",
        }
    }
}

/// Which account a session counts for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// Certain. `Known(None)`: its folder is not grouped into an identity yet.
    Known(Option<IdentityId>),
    /// Can't be told for certain; the best guess, if any.
    Unsure(Option<IdentityId>),
    /// Not placed yet (a Desktop-hosted session whose registry entry hasn't
    /// been read).
    Waiting,
}

/// How long an unplaced session (`Known(None)`, or Desktop-hosted without
/// host id and registry status) waits before it counts as unsure for the
/// cloud (CLAUDE.md "Unsure sessions").
pub const PLACEMENT_GRACE: Duration = Duration::from_secs(30);

/// Task progress (HS§5.9).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProgress {
    pub done: u32,
    pub total: u32,
    /// The first in-progress item's active form or subject.
    pub active_label: Option<String>,
    pub items: Vec<TaskItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub id: Option<String>,
    pub label: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    /// TaskUpdate status `deleted`: hidden, not counted.
    Deleted,
    /// A TaskCreate whose PostToolUse failed: never counted.
    CreateFailed,
}

/// The turn ended waiting on background agents or workflows (HS§5.6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundWait {
    pub since: Option<SystemTime>,
    /// The awaited types (`subagent`, `workflow`, `teammate`, …).
    pub agent_types: Vec<String>,
    /// Every running background task at Stop.
    pub task_count: u32,
}

/// A session, read-only, as every other package sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: SessionId,
    pub account: Option<AccountId>,
    pub ring: Option<RingId>,
    pub attribution: Attribution,
    /// When the current attribution began.
    pub attribution_since: SystemTime,
    pub cwd: PathBuf,
    pub project_name: String,
    /// The panel's title: a hook, registry or transcript title, else the
    /// conversation summary, else the first prompt, else the project folder.
    pub title: String,
    /// `public_title` is only a name made from the folder (Claude Code's
    /// derived `name`, or the project folder): the session has no title and
    /// no summary of its own, so the cloud sends none.
    pub title_from_folder: bool,
    pub state: SessionState,
    pub phase: Phase,
    pub pid: Option<u32>,
    pub pid_started: Option<SystemTime>,
    pub entrypoint: Option<String>,
    pub config_dir_env: Option<String>,
    pub host_session_id: Option<String>,
    /// `busy` | `shell` | `idle` | `waiting` from `sessions\<pid>.json`.
    pub registry_status: Option<String>,
    pub first_seen_at: SystemTime,
    pub model: Option<String>,
    pub context_pct: Option<f64>,
    pub tasks: Option<TaskProgress>,
    pub background: BackgroundWait,
    pub last_activity: SystemTime,
    pub turn_started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub reviewed_at: Option<SystemTime>,
    pub last_assistant_message: Option<String>,
    pub pending: Vec<PendingRequest>,
    pub cost_usd: Option<f64>,
    pub transcript_path: Option<PathBuf>,

    // Added by WP5 for the packages that read sessions (rows, cards,
    // messaging, toasts, the cloud): the Mac reads these off `SessionState`.
    /// The folder the latest event reported (`cwd` is where it started).
    pub current_cwd: PathBuf,
    /// The last part of `current_cwd`.
    pub display_project_name: String,
    /// The name used outside the panel (cards, banners): a hook, registry or
    /// transcript title, else the project folder, never the first prompt
    /// (ClaudeHostProjections.publicTitle).
    pub public_title: String,
    /// The transcript's `summary` line.
    pub summary: Option<String>,
    /// The transcript's last message (80 characters), who wrote it
    /// (`user` | `assistant` | `tool`) and, for a tool, which.
    pub last_message: Option<String>,
    pub last_message_role: Option<String>,
    pub last_tool_name: Option<String>,
    /// Hooks report this session (not only the registry or the status line).
    pub is_hook_backed: bool,
    pub last_hook_event_at: Option<SystemTime>,
    /// The last hook event, status line update or registry change applied.
    pub last_event_at: SystemTime,
    /// Tool calls started and not finished, oldest first.
    pub running_tools: Vec<RunningTool>,
    /// When it started waiting for the user: the shown request's activation,
    /// else when the needs-input reason was set.
    pub waiting_since: Option<SystemTime>,
    /// A Stop not yet confirmed as the end of its turn.
    pub completion_pending_since: Option<SystemTime>,
    /// The completion isn't worth an alert (a /loop tick, a turn that left
    /// Claude waiting to be woken): it stays in the review queue silently.
    pub completion_quiet: bool,
    /// "1 workflow", "2 background agents" while the turn waits on them.
    pub background_wait_description: Option<String>,
    pub permission_mode: Option<String>,
    pub context_window_size: Option<u64>,
    /// The terminal variables the latest hook forwarded.
    pub terminal: Option<HookTerminal>,
    /// The Claude Desktop record's identity of a session Desktop hosts.
    pub desktop_identity: Option<IdentityId>,
    /// Claude Desktop hosts it (its entrypoint says so): it runs as
    /// Desktop's account, whatever its folder names.
    pub is_desktop_hosted: bool,
}

impl SessionView {
    /// An idle session with nothing known but its id, folder and the time it
    /// was first seen: what tests and fixtures start from (`..SessionView::new`).
    pub fn new(id: impl Into<SessionId>, cwd: impl Into<PathBuf>, at: SystemTime) -> SessionView {
        let cwd: PathBuf = cwd.into();
        let project_name =
            crate::sessions::tool_input::file_name(&cwd.to_string_lossy()).to_owned();
        SessionView {
            id: id.into(),
            account: None,
            ring: None,
            attribution: Attribution::Known(None),
            attribution_since: at,
            current_cwd: cwd.clone(),
            cwd,
            display_project_name: project_name.clone(),
            public_title: project_name.clone(),
            title: project_name.clone(),
            project_name,
            title_from_folder: true,
            state: SessionState::Idle,
            phase: Phase::Idle,
            pid: None,
            pid_started: None,
            entrypoint: None,
            config_dir_env: None,
            host_session_id: None,
            registry_status: None,
            first_seen_at: at,
            model: None,
            context_pct: None,
            tasks: None,
            background: BackgroundWait::default(),
            last_activity: at,
            turn_started_at: None,
            completed_at: None,
            reviewed_at: None,
            last_assistant_message: None,
            pending: Vec::new(),
            cost_usd: None,
            transcript_path: None,
            summary: None,
            last_message: None,
            last_message_role: None,
            last_tool_name: None,
            is_hook_backed: false,
            last_hook_event_at: None,
            last_event_at: at,
            running_tools: Vec::new(),
            waiting_since: None,
            completion_pending_since: None,
            completion_quiet: false,
            background_wait_description: None,
            permission_mode: None,
            context_window_size: None,
            terminal: None,
            desktop_identity: None,
            is_desktop_hosted: false,
        }
    }

    /// The request shown now: the first pending one.
    pub fn active_request(&self) -> Option<&PendingRequest> {
        self.pending.first()
    }

    /// The turn failed (StopFailure), as opposed to waiting on something the
    /// user can answer.
    pub fn has_failed_turn(&self) -> bool {
        matches!(self.state, SessionState::Failed(_))
    }
}

/// A tool call in progress (ToolTracker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningTool {
    pub id: String,
    pub name: String,
    pub started_at: SystemTime,
    /// The subagent that called it; `None` for the main session.
    pub agent_id: Option<String>,
    /// It waits for permission.
    pub pending_approval: bool,
}

/// Who wrote a transcript message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
}

/// One block of a transcript message (MessageBlock).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking(String),
    Image {
        media_type: String,
        data_base64: String,
    },
    /// "[Request interrupted by user".
    Interrupted,
}

impl MessageBlock {
    /// The kind word of chat item ids (`<uuid>-<kind>-<block index>`).
    pub fn type_prefix(&self) -> &'static str {
        match self {
            MessageBlock::Text(_) => "text",
            MessageBlock::ToolUse { .. } => "tool",
            MessageBlock::Thinking(_) => "thinking",
            MessageBlock::Image { .. } => "image",
            MessageBlock::Interrupted => "interrupted",
        }
    }
}

/// A user or assistant line of a transcript as the chat needs it
/// (ChatMessage): not meta, not a sidechain, not a command echo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// The line's `uuid`.
    pub id: String,
    pub role: ChatRole,
    /// The line's `timestamp`; `None` when it has none (the store uses the
    /// time it read it).
    pub at: Option<SystemTime>,
    pub blocks: Vec<MessageBlock>,
}

impl ChatMessage {
    /// The chat item ids this message produces.
    pub fn item_ids(&self) -> Vec<String> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(index, block)| match block {
                MessageBlock::ToolUse { id, .. } => id.clone(),
                other => format!("{}-{}-{index}", self.id, other.type_prefix()),
            })
            .collect()
    }
}

/// A tool result's detail for the chat: its text and the raw
/// `toolUseResult` (the store knows the call's name and parses it into a
/// `ToolResultView`; the sync job keeps no state between reads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub tool_use_id: String,
    /// The line's top-level `toolName`, when Claude Code wrote one.
    pub tool_name: Option<String>,
    pub is_error: bool,
    pub is_interrupted: bool,
    /// The result's text: `content` (a string, or the first text block).
    pub content: Option<String>,
    /// `toolUseResult.stdout` / `.stderr`.
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    /// `toolUseResult` verbatim.
    pub raw: Option<Value>,
}

/// A `review-state.json` record (HS§5.13), in model terms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub completed_at: Option<SystemTime>,
    pub reviewed_at: Option<SystemTime>,
    /// At most 1500 characters.
    pub last_assistant_message: Option<String>,
    pub stop_error: Option<String>,
    pub stop_error_code: Option<String>,
    pub failed_at: Option<SystemTime>,
    pub background_wait_since: Option<SystemTime>,
    pub background_agent_types: Vec<String>,
    pub updated_at: SystemTime,
}

impl ReviewItem {
    /// A record with nothing in it yet, last changed at `updated_at`.
    pub fn new(updated_at: SystemTime) -> ReviewItem {
        ReviewItem {
            completed_at: None,
            reviewed_at: None,
            last_assistant_message: None,
            stop_error: None,
            stop_error_code: None,
            failed_at: None,
            background_wait_since: None,
            background_agent_types: Vec::new(),
            updated_at,
        }
    }

    /// Nothing worth keeping.
    pub fn is_empty(&self) -> bool {
        self.completed_at.is_none()
            && self.reviewed_at.is_none()
            && self.last_assistant_message.is_none()
            && self.stop_error.is_none()
            && self.background_wait_since.is_none()
    }

    /// The same content, whenever it was written.
    pub fn has_same_content(&self, other: &ReviewItem) -> bool {
        ReviewItem {
            updated_at: self.updated_at,
            ..other.clone()
        } == *self
    }
}

/// One `sessions\<pid>.json` (SessionRegistryEntry, HS§10.1). Times are the
/// file's epoch milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub pid: u32,
    pub session_id: SessionId,
    pub cwd: Option<String>,
    /// `interactive` | `bg` | `daemon` | `daemon-worker` (missing in older versions).
    pub kind: Option<String>,
    pub entrypoint: Option<String>,
    pub name: Option<String>,
    /// `derived` when Claude Code made the name up from the folder.
    pub name_source: Option<String>,
    pub version: Option<String>,
    /// `busy` | `idle` | `shell` | `waiting`.
    pub status: Option<String>,
    /// `permission prompt` | `input needed` | `dialog open` | …
    pub waiting_for: Option<String>,
    pub started_at: Option<SystemTime>,
    pub updated_at: Option<SystemTime>,
    pub status_updated_at: Option<SystemTime>,
    /// `EEE MMM d HH:mm:ss yyyy` (UTC), when written.
    pub proc_start: Option<String>,
    /// Claude Desktop's `local_…` id, only with a Desktop entrypoint.
    pub host_session_id: Option<String>,
    /// The process's creation time (`Processes::start_time`), read by the job.
    pub process_started: Option<SystemTime>,
    /// The liveness rule held (§4.5: running, and its creation time matches
    /// `proc_start` within 2 s, else is ≤ `started_at` + 5 s).
    pub live: bool,
    /// The process's `CLAUDE_CONFIG_DIR`, read only when the folder was
    /// reached through a link (§4.2); `None` otherwise.
    pub process_config_dir: Option<EnvRead>,
}

impl RegistryEntry {
    /// An entry with only its process and session (tests, fixtures): an
    /// interactive `cli` session whose process runs.
    pub fn new(pid: u32, session_id: impl Into<SessionId>) -> RegistryEntry {
        RegistryEntry {
            pid,
            session_id: session_id.into(),
            cwd: None,
            kind: Some("interactive".into()),
            entrypoint: Some("cli".into()),
            name: None,
            name_source: None,
            version: None,
            status: None,
            waiting_for: None,
            started_at: None,
            updated_at: None,
            status_updated_at: None,
            proc_start: None,
            host_session_id: None,
            process_started: None,
            live: true,
            process_config_dir: None,
        }
    }

    /// Interactive sessions the app tracks: `bg`, `daemon` and
    /// `daemon-worker` kinds and SDK entrypoints are ignored.
    pub fn is_tracked(&self) -> bool {
        if self
            .kind
            .as_deref()
            .is_some_and(|kind| !kind.is_empty() && kind != "interactive")
        {
            return false;
        }
        !self
            .entrypoint
            .as_deref()
            .is_some_and(|entry| !entry.is_empty() && entry.to_lowercase().starts_with("sdk"))
    }

    /// Claude Code made the name up rather than the user choosing it.
    pub fn is_name_derived(&self) -> bool {
        self.name_source.as_deref() == Some("derived")
    }

    /// When the status last changed (the last update when the file says no more).
    pub fn status_changed_at(&self) -> Option<SystemTime> {
        self.status_updated_at.or(self.updated_at)
    }
}

/// One read of a physical `sessions` folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub sessions_dir: PathBuf,
    /// Reached through a link (shared history): entries are attributed by
    /// their process's `CLAUDE_CONFIG_DIR`.
    pub via_link: bool,
    pub entries: Vec<RegistryEntry>,
    pub read_at: SystemTime,
    pub error: Option<String>,
}

/// A session's attention changed (AttentionTracker, HS§7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttentionTransition {
    pub session: SessionView,
    /// `None` when the session just appeared.
    pub from: Option<SessionState>,
    pub to: SessionState,
}

impl AttentionTransition {
    /// Newly needs the user, or for a different reason.
    pub fn became_needs_you(&self) -> bool {
        let Some(reason) = self.to.reason() else {
            return false;
        };
        self.from.as_ref().and_then(SessionState::reason) != Some(reason)
    }

    pub fn became_ready_for_review(&self) -> bool {
        self.to == SessionState::ReadyForReview && self.from != Some(SessionState::ReadyForReview)
    }

    /// The turn failed (rate limit, overload, sign-in, billing): blocked on
    /// the user, but with nothing to answer. Worth a different sound and
    /// banner, and one banner per account rather than one per session.
    pub fn is_failure(&self) -> bool {
        self.to.reason().is_some_and(NeedsInputReason::is_error)
    }
}

/// A known identity as Claude Desktop's folders name it, for finding whose
/// a Desktop-hosted session is (`sessions::desktop`, AU§13).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DesktopCandidate {
    pub identity_id: IdentityId,
    pub account_uuid: String,
    /// `None` when not known: every organization folder of the account is
    /// looked in.
    pub organization_uuid: Option<String>,
}

/// An image block of a transcript, kept by the chat history and fetched by
/// the panel one at a time (`Call::ChatImage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatImage {
    pub media_type: String,
    pub data_base64: String,
}

impl ChatImage {
    /// The longest data URL `Call::ChatImage` answers with (§4.8).
    pub const MAX_DATA_URL_BYTES: usize = 2 * 1024 * 1024;

    /// The decoded size, from the base64 length alone (nothing is decoded).
    pub fn byte_count(&self) -> u64 {
        let data = self.data_base64.trim_end();
        let padding = data.bytes().rev().take_while(|b| *b == b'=').count().min(2);
        ((data.len() / 4) * 3
            + match data.len() % 4 {
                2 => 1,
                3 => 2,
                _ => 0,
            })
        .saturating_sub(padding) as u64
    }

    /// `data:<media type>;base64,…`, or `None` when it would be longer than
    /// [`ChatImage::MAX_DATA_URL_BYTES`] or the media type isn't an image's.
    pub fn data_url(&self) -> Option<String> {
        let media = self.media_type.trim();
        // The type goes into a URL the page hands to an <img>: only a plain
        // `image/<subtype>` is let through.
        let plain = media.strip_prefix("image/").is_some_and(|sub| {
            !sub.is_empty()
                && sub
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        });
        if !plain {
            return None;
        }
        let url = format!("data:{media};base64,{}", self.data_base64.trim());
        (url.len() <= Self::MAX_DATA_URL_BYTES).then_some(url)
    }
}

/// A page of a session's conversation, read from its transcript
/// (ConversationParser, HS§5.11): the `LoadChat` job's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatPage {
    pub session: SessionId,
    pub path: PathBuf,
    /// Oldest first, at most 150.
    pub items: Vec<ChatItem>,
    /// Items before the first one.
    pub has_earlier: u32,
    /// The page ends before this item id (`None`: the newest page).
    pub before: Option<String>,
    pub images: BTreeMap<String, ChatImage>,
    pub error: Option<String>,
    /// When each item's transcript line was written, in `items`' order
    /// (`None`: the line carried no timestamp). The store orders hook
    /// placeholders among transcript items by it.
    pub times: Vec<Option<SystemTime>>,
}

/// An open chat's history as the session store keeps it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatHistory {
    pub items: Vec<ChatItem>,
    pub has_earlier: u32,
    pub images: BTreeMap<String, ChatImage>,
    /// Bumped on every change; the panel drops out-of-order updates.
    pub revision: u64,
}
