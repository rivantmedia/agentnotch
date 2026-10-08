//! The UI contract (DESIGN-WIN §3.6): everything the fork's pages render,
//! pushed whole as `an:snapshot` / `an:settings` / `an:chat` and answered by
//! `an_call`. Every field is always present in the JSON (`null` when absent),
//! so the pages never guess; the fixtures in `tests/ui-contract/` hold one
//! full example of each and are shared with the node tests.

use crate::model::{CloudState, Question, TaskProgress};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The notch's and the panel's whole state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubSnapshot {
    /// 1.
    pub version: u32,
    pub generated_at_ms: u64,
    pub sealed: bool,
    pub rings: Vec<RingSummary>,
    /// In display order (sections, then the order inside each, UI§5.3).
    pub sessions: Vec<SessionRow>,
    pub sections: Vec<SectionInfo>,
    pub totals: Counts,
    pub resting_marks: RestingMarks,
    pub tray_badge: u32,
    pub setup: SetupState,
    pub ui: UiSettings,
    /// More than one tracked account: rows carry account tags.
    pub accounts_multi: bool,
}

impl HubSnapshot {
    pub const VERSION: u32 = 1;
}

/// What a ring shows besides usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingActivity {
    /// 3/4 arc spinning.
    Working,
    /// Amber full ring pulsing: a session needs you.
    Waiting,
    /// Green full ring pulsing until `success_settles_at_ms`, then steady.
    Success,
    Idle,
}

/// One Claude ring (one identity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingSummary {
    pub ring_id: String,
    pub label: String,
    pub monogram: String,
    pub color_index: u8,
    /// "Ring in notch".
    pub shown: bool,
    /// Holds `~\.claude`: its window ids go to upstream unsuffixed.
    pub is_default: bool,
    pub usage: RingUsage,
    pub activity: RingActivity,
    pub success_settles_at_ms: Option<u64>,
    pub badges: Badges,
    pub counts: Counts,
    /// The ring's accessible label.
    pub a11y: String,
}

/// A ring's usage in upstream's cell shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingUsage {
    /// `ok` | `stale` | `waiting` | `sign_in_needed` | `unavailable` | `failed`.
    pub status: String,
    /// Ids `session`, `weekly_all`, `weekly_<slug>`, `extra_usage` (AU§8.5);
    /// `used` 0..=1; `resets_at` epoch ms.
    pub windows: Vec<UpstreamWindow>,
    pub fetched_at_ms: u64,
    pub note: String,
    /// The engine's rule (AU§8.4: 1 h with probes off, else
    /// max(15 min, 1.5 × interval), never for an exhausted window);
    /// `decorateCell` sets the cell's `stale` class from it, overriding
    /// upstream's 15-minute rule.
    pub stale: bool,
}

/// Upstream's `usage::UsageSnapshot`, the shape `AppState.usage` and the
/// `usage` event carry (§4.6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpstreamUsage {
    /// `ok` | `stale` | `none` | `error` (never `needsAuth` for Claude).
    pub status: String,
    pub windows: Vec<UpstreamWindow>,
    /// Epoch ms.
    pub fetched_at: u64,
    pub note: String,
    pub backoff_until: u64,
}

/// Upstream's `usage::LimitWindow`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpstreamWindow {
    pub id: String,
    pub label: String,
    /// 0.0..=1.0.
    pub used: f64,
    /// Epoch ms.
    pub resets_at: Option<u64>,
    pub count: Option<i64>,
    pub derived: bool,
    pub group: Option<String>,
}

/// Count badges on a ring (the pages label them "", "1".."9", "9+").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badges {
    pub needs_you: u32,
    pub review: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Answerable (failed not included).
    pub needs_you: u32,
    pub failed: u32,
    pub review: u32,
    pub working: u32,
    pub idle: u32,
}

/// The folded notch's marks (UI§3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestingMarks {
    pub needs_you: bool,
    pub review: bool,
    pub working: bool,
    /// Changes whenever a new request arrives: the bar breathes again.
    pub needs_you_key: u32,
}

/// One session in the panel's list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRow {
    pub session_id: String,
    pub ring_id: Option<String>,
    pub account_label: Option<String>,
    pub account_color: Option<u8>,
    pub title: String,
    pub project: Option<String>,
    /// `needs_you` | `ready_for_review` | `working` | `idle`.
    pub bucket: String,
    pub failed: bool,
    /// "needs you", "failed", "done", "working", "idle".
    pub state_word: String,
    /// What the elapsed label counts from (UI§5.4).
    pub since_ms: u64,
    pub detail: RowDetail,
    pub tasks: Option<TaskProgress>,
    pub context_pct: Option<f64>,
    pub background_count: u32,
    pub pending: Option<PendingRequestView>,
    /// "Show terminal" | "Show in editor"; `None`: nowhere to jump.
    pub focus_label: Option<String>,
    pub can_message: bool,
    pub reviewable: bool,
    pub a11y: String,
    /// What the chat shows under its last message when Claude is not working
    /// (ChatStatusLine.swift); `None` while it works or waits on an answer.
    /// Absent from a payload made before it existed.
    #[serde(default)]
    pub chat_status: Option<ChatStatusLine>,
    /// The notch hover card's row (UI§3.5), a port of `activityRow`.
    pub card: CardRow,
}

/// The one line a chat shows under its last message once Claude has stopped:
/// the turn failed, it is ready for review, or the session is idle. It speaks
/// with the row's words, so the row and the chat never disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatStatusLine {
    /// The row's mark: `error` | `review` | `idle` (`agentnotchCommon.glyphKind`).
    pub glyph: String,
    pub text: String,
    /// A failed turn can be dismissed from the chat as from the row.
    pub can_dismiss: bool,
}

/// A hover-card row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardRow {
    /// "<host app> · <project>", or the title.
    pub name: String,
    /// "3/7 … · ctx 42%".
    pub detail: Option<String>,
    /// `needs_you` | `failed` | `review` | `working` | `idle`.
    pub state: String,
    pub waiting_for: Option<String>,
    pub since_ms: u64,
}

/// A row's second line (SessionRowContent, UI§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowDetail {
    Permission {
        tool: String,
        request: String,
        waiting_in_terminal: bool,
    },
    Question {
        text: String,
    },
    Plan,
    Dialog {
        text: String,
    },
    Failed {
        text: String,
    },
    /// `secondary`: a tool line or "Thinking…", drawn in secondary ink.
    Working {
        text: String,
        secondary: bool,
    },
    Review {
        text: String,
    },
    Idle {
        text: String,
    },
}

/// The active request of a row, as its action bar needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRequestView {
    pub tool_use_id: String,
    /// `permission` | `question` | `plan`.
    pub kind: String,
    pub tool_name: String,
    pub received_at_ms: u64,
    pub request: String,
    pub needs_review: bool,
    /// The "Always" description.
    pub always: Option<String>,
    pub inline_always: bool,
    pub questions: Option<Vec<Question>>,
    /// A single single-choice question with 1-4 options: one tap answers.
    pub single_tap: bool,
    pub plan_markdown: Option<String>,
    pub diff: Option<Vec<DiffLine>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Hunk,
    Context,
    Add,
    Remove,
}

/// One line of an Edit/Write/MultiEdit diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}

/// A list section's header (UI§5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionInfo {
    pub bucket: String,
    pub title: String,
    pub count: u32,
    /// Idle folds by default above 3 sessions; the others only when folded.
    pub fold_by_default: bool,
}

/// First-run and problem banners (UI§5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupState {
    /// `None` until "Turn on" / "Not now".
    pub hook_consent: Option<bool>,
    pub needs_hook_consent: bool,
    /// Every settings.json "Turn on" would write.
    pub consent_files: Vec<ConsentFile>,
    /// Folders whose settings.json holds the official Codenotch's hooks.
    pub codenotch_hooks_folders: Vec<String>,
    /// Folders the consent now also covers (the scope notice).
    pub new_install_folders: Vec<String>,
    /// The pipe couldn't be opened (banner (d)).
    pub transport_error: Option<String>,
    /// Consent given, hooks turned off.
    pub control_off: bool,
    /// Tracked accounts whose hooks are missing.
    pub missing_hooks_accounts: Vec<String>,
    /// `--no-install`: nothing is written in this run.
    pub install_disabled: bool,
}

/// One file of the consent card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentFile {
    /// `~\.claude\settings.json`.
    pub path: String,
    /// Whose it is: the account's email or name.
    pub account: Option<String>,
}

/// The attention and panel settings the pages need (§4.12 values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSettings {
    /// `autoOpen`: `never` | `needsInput` | `needsInputOrDone`.
    pub panel_open_mode: String,
    /// `holdOpenWhileNeedsYou`: `never` | `always`.
    pub hold_open: String,
    pub ring_badges: bool,
    pub resting_marks: bool,
    /// `openPanel` | `refreshUsage`.
    pub ring_click: String,
    /// `sessionClick`: `smart` | `panel` | `terminal`.
    pub hover_click: String,
    /// `off` | `ctrlAltSpace` | `ctrlAltJ`.
    pub hotkey: String,
    pub panel_pinned: bool,
    pub peek: bool,
    pub peek_seconds: u32,
    pub type_replies: bool,
    pub sound: bool,
    pub tray_badge: bool,
    /// The global shortcut is registered (or none is wanted). `false` while the
    /// chosen one is taken or refused: settings then says so. Added after the
    /// first fixtures, so a payload without it reads as fine.
    #[serde(default = "default_true")]
    pub hotkey_ok: bool,
    /// Windows' own words for a refusal, when it gave any.
    #[serde(default)]
    pub hotkey_message: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A chat over IPC: a reset (the whole page) when a chat opens or pages,
/// then patches (only new or changed items, the ids removed, and the page's
/// id order). Images travel by reference (`chat_image`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatUpdate {
    pub session_id: String,
    pub revision: u64,
    pub reset: bool,
    pub items: Vec<ChatItem>,
    pub removed: Vec<String>,
    pub order: Vec<String>,
    pub has_earlier: u32,
    /// The active task or "Thinking…" while the session works.
    pub working: Option<String>,
    pub ended: bool,
    pub loading: bool,
}

/// One transcript item. Ids: the tool_use id, or
/// `<uuid>-<text|tool|thinking|image|interrupted>-<block index>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatItem {
    pub id: String,
    #[serde(flatten)]
    pub body: ChatBody,
}

// Moved once through a channel or a call; boxing variants would change the
// §3 signatures every package codes against.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatBody {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tool {
        name: String,
        summary: String,
        /// `running` | `waiting_for_approval` | `success` | `error` | `interrupted`.
        status: String,
        input: Value,
        result: Option<ToolResultView>,
        subagent: Option<SubagentView>,
    },
    Image {
        media_type: String,
        image_id: String,
        bytes: u64,
    },
    Interrupted,
}

/// A structured tool result (ToolResultData.swift), per tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolResultView {
    Read {
        file_path: String,
        content: String,
        num_lines: u32,
        start_line: u32,
        total_lines: u32,
    },
    Edit {
        file_path: String,
        replace_all: bool,
        user_modified: bool,
        diff: Vec<DiffLine>,
    },
    Write {
        file_path: String,
        created: bool,
        content: String,
        diff: Vec<DiffLine>,
    },
    Bash {
        stdout: String,
        stderr: String,
        interrupted: bool,
        return_code_interpretation: Option<String>,
        background_task_id: Option<String>,
    },
    Grep {
        mode: String,
        filenames: Vec<String>,
        num_files: u32,
        content: Option<String>,
        num_lines: Option<u32>,
    },
    Glob {
        filenames: Vec<String>,
        num_files: u32,
        truncated: bool,
    },
    Todo {
        items: Vec<TodoView>,
    },
    Task {
        agent_id: String,
        status: String,
        content: String,
        total_duration_ms: Option<u64>,
        total_tokens: Option<u64>,
        total_tool_use_count: Option<u32>,
    },
    WebFetch {
        url: String,
        code: u32,
        code_text: String,
        bytes: u64,
        duration_ms: u64,
        result: String,
    },
    WebSearch {
        query: String,
        results: Vec<SearchResultView>,
    },
    AskUserQuestion {
        questions: Vec<Question>,
        answers: std::collections::BTreeMap<String, String>,
    },
    BashOutput {
        shell_id: String,
        status: String,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
    },
    KillShell {
        shell_id: String,
        message: String,
    },
    ExitPlanMode {
        plan: Option<String>,
        file_path: Option<String>,
    },
    Mcp {
        server_name: String,
        tool_name: String,
        raw: Value,
    },
    Generic {
        text: Option<String>,
        raw: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoView {
    pub content: String,
    /// `pending` | `in_progress` | `completed`.
    pub status: String,
    pub active_form: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResultView {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// A subagent (Agent/Task tool) followed inside its container item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentView {
    pub agent_id: Option<String>,
    pub description: Option<String>,
    pub tools: Vec<SubagentToolView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentToolView {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub status: String,
}

/// Open or route the panel (`an:panel`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelRequest {
    /// `sessions` | `session:<id>`.
    pub route: String,
    pub ring_id: Option<String>,
    /// A session to highlight in the list.
    pub highlight: Option<String>,
    /// `ring_click` | `hover_row` | `peek_click` | `notification` | `hotkey` |
    /// `settings` | `auto`.
    pub reason: String,
}

/// Where the glue put an open panel (`an:panel_place`, and these four fields
/// beside the request's own in `an:panel`): what the page draws its tail and
/// width by. The glue builds it from `geometry::panel`'s placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelPlace {
    /// The notch's edge (`left` | `right` | `top` | `bottom`); `null` when
    /// the panel floats.
    pub edge: Option<String>,
    pub floating: bool,
    /// The card's width in CSS pixels.
    pub width: f64,
    /// The tail's offset from the card's centre along its side.
    pub tail_offset: f64,
}

/// The Claude Code settings pane's whole state (UI§7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsSnapshot {
    pub accounts: Vec<AccountRow>,
    pub suggestions: Vec<Suggestion>,
    /// "<list>: Claude Code runs here, but nobody is signed in yet."
    pub unsigned_folders: Vec<String>,
    pub hooks: HooksSection,
    pub usage: UsageSection,
    pub cloud: CloudState,
    pub attention: UiSettings,
    pub notifications: NotificationsSection,
    pub advanced: AdvancedSection,
    pub sealed: bool,
    pub setup: SetupState,
}

/// One account in Settings › Accounts (UI§7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountRow {
    pub identity_id: String,
    pub ring_id: String,
    pub label: String,
    /// The rename field's placeholder: the name without a custom one.
    pub default_label: String,
    pub has_custom_label: bool,
    pub color_index: u8,
    pub is_default: bool,
    /// "me@work.com · Max 20x", or "Not signed in".
    pub identity_line: String,
    /// "Runs in ~\.claude", or "Runs nowhere now".
    pub folder_summary: String,
    pub folders: Vec<FolderRow>,
    /// "5-hour 34% · weekly 41% · 4m ago", or why there's none.
    pub usage_line: String,
    pub usage_stale: bool,
    /// "Hooks installed", "Hooks in 1 of 2 folders", "Hooks not installed", …
    pub hook_state: String,
    /// `ok` | `warning` | `critical` | `neutral`: the chip's ink.
    pub hook_state_tone: String,
    pub live_status_line: bool,
    /// "Its sessions still show; answer their prompts where …".
    pub hook_problem: Option<String>,
    pub hook_problem_critical: bool,
    pub is_tracked: bool,
    pub ring_shown: bool,
    pub is_signed_in: bool,
    pub launch_command: Option<String>,
    pub can_install: bool,
    /// "Install hooks" | "Reinstall hooks".
    pub install_label: String,
    pub can_forget: bool,
    pub forget_caption: String,
}

/// One folder of an account, expanded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRow {
    /// The folder's `AccountId` (what `reveal_target` takes).
    pub id: String,
    /// `~\.claude-work`.
    pub title: String,
    /// "Default" | "Folder" | "Account store" | …
    pub role: String,
    /// "Hooks and live status line" | "Hooks installed" | "Not tracked" | …
    pub state: String,
    /// "Status line left alone: its command uses Windows paths", …
    pub status_line_note: Option<String>,
    /// Why no hook command can run from this folder, when none can.
    pub not_hookable: Option<String>,
    pub codenotch_hooks: bool,
}

/// A folder discovery found but didn't add.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// `~\.claude-old`.
    pub path: String,
    /// "Named like a backup copy.", …
    pub reason: String,
}

/// Settings › Hooks and status line (UI§7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HooksSection {
    pub consent: Option<bool>,
    pub enabled: bool,
    /// The switch can't be used (no consent, `--no-install`, busy).
    pub enabled_locked: bool,
    /// "Installed in all 2 tracked accounts.", …
    pub summary: String,
    pub summary_warning: bool,
    pub status_line: bool,
    /// The hook pipe, shown where the Mac shows its socket path.
    pub pipe_name: String,
    pub claude_version: Option<String>,
    pub claude_path: Option<String>,
    /// The path was chosen in Settings ("Find automatically" then shows).
    pub claude_path_chosen: bool,
    /// "Hooks are written for the oldest claude found, so every version reads them."
    pub claude_caption: String,
    /// "Last change: settings.json in ~\.claude. The previous version is kept as …".
    pub last_change: Option<String>,
    pub install_allowed: bool,
    pub busy: bool,
    pub footnotes: Vec<String>,
}

/// Settings › Usage (UI§7.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSection {
    pub interval_minutes: u32,
    /// `[0, 5, 10, 15, 30]`; 0 is Off.
    pub interval_options: Vec<u32>,
    pub interval_caption: String,
    pub desktop_cache: bool,
    pub desktop_caption: String,
    /// `simple` | `blockfile` | `absent`, when read.
    pub desktop_format: Option<String>,
    pub accounts: Vec<UsageAccountLine>,
    pub refreshing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAccountLine {
    pub ring_id: String,
    pub label: String,
    pub color_index: u8,
    pub line: String,
}

/// Settings › Notifications (UI§7.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationsSection {
    pub notify_needs_input: bool,
    pub notify_ready_for_review: bool,
    /// `allowed` | `disabled_for_app` | `disabled_for_user` | `disabled_by_policy` | `unavailable`.
    pub permission: String,
    /// "Allowed" | "Off in Windows Settings" | "Banners need the installed app".
    pub permission_text: String,
    /// Banners are on but Windows turned them off.
    pub permission_warning: bool,
}

/// Settings › Advanced (UI§7.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvancedSection {
    pub session_count: u32,
    pub review_count: u32,
}
