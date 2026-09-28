//! Sessions as the pipeline sees them (HS§5; SessionState.swift,
//! SessionPhase.swift, SessionAttention.swift).

use crate::model::{AccountId, ChatItem, IdentityId, PendingRequest, RingId, SessionId};
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
    pub title: String,
    /// The title was derived from the folder name (the cloud then sends the
    /// conversation summary instead).
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
}

/// An image block of a transcript, kept by the chat history and fetched by
/// the panel one at a time (`Call::ChatImage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatImage {
    pub media_type: String,
    pub data_base64: String,
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
