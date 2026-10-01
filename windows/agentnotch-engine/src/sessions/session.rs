//! The engine's per-session record (SessionState.swift's port, HS§5): what
//! the pipeline knows of one Claude session, the derived facts the rest of
//! the app reads through [`SessionView`], and the small trackers it owns
//! (tools in progress, subagent tasks, the agents whose transcripts are
//! followed).
//!
//! The record holds data and pure rules only. The store (`store.rs` and its
//! siblings) decides *when* each field changes; every time is passed in,
//! never read from a clock here.

use crate::core::paths::Paths;
use crate::model::{
    AccountId, AlwaysRule, Attribution, BackgroundWait, ChatHistory, HookTerminal, IdentityId,
    NeedsInputReason, PendingRequest, PermissionContext, Phase, Question, QuestionOption,
    RequestKind, RingId, RunningTool, SessionId, SessionState, SessionView,
};
use crate::runtime_types::TranscriptCursor;
use crate::sessions::attention;
use crate::sessions::background;
use crate::sessions::chat::{ChatState, SubagentTranscript};
use crate::sessions::desktop;
use crate::sessions::summary::{ConversationInfo, TranscriptSummary};
use crate::sessions::tasks::TaskList;
use crate::sessions::tool_input;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::SystemTime;

// ---- titles ----

/// Where a session title came from, lowest priority first (a lower source
/// never overwrites a higher one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionTitleSource {
    /// A registry `name` Claude Code derived from the project folder
    /// (`nameSource: "derived"`): less telling than any transcript title.
    DerivedName,
    /// `summary` / `ai-title` / `custom-title` lines in the transcript.
    Transcript,
    /// A chosen `name` in the session registry or the status line's
    /// `session_name`.
    Registry,
    /// `session_title` from a SessionStart / UserPromptSubmit hook.
    Hook,
}

// ---- tools in progress ----

/// Phase of a tool in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPhase {
    Starting,
    Running,
    PendingApproval,
}

/// A tool call started (PreToolUse) and not finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInProgress {
    pub id: String,
    pub name: String,
    pub start_time: SystemTime,
    pub phase: ToolPhase,
    /// The subagent that called it; `None` for the main session.
    pub agent_id: Option<String>,
}

/// Tool calls started (PreToolUse) and not finished (PostToolUse,
/// PostToolUseFailure, PermissionDenied). The main session's calls are
/// dropped when its turn ends, so a call whose end never came (an interrupt,
/// a crash) doesn't linger; background agents' calls stay until they finish,
/// and at most [`ToolTracker::MAX_IN_PROGRESS`] are kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolTracker {
    in_progress: BTreeMap<String, ToolInProgress>,
}

impl ToolTracker {
    /// Most calls followed at once (the newest are kept).
    pub const MAX_IN_PROGRESS: usize = 64;

    pub fn new() -> ToolTracker {
        ToolTracker::default()
    }

    pub fn len(&self) -> usize {
        self.in_progress.len()
    }

    pub fn is_empty(&self) -> bool {
        self.in_progress.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&ToolInProgress> {
        self.in_progress.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.in_progress.contains_key(id)
    }

    /// Every call in progress, oldest first (ties by id).
    pub fn in_progress(&self) -> Vec<&ToolInProgress> {
        let mut tools: Vec<&ToolInProgress> = self.in_progress.values().collect();
        tools.sort_by(|a, b| (a.start_time, &a.id).cmp(&(b.start_time, &b.id)));
        tools
    }

    /// The most recently started tool still in progress.
    pub fn newest(&self) -> Option<&ToolInProgress> {
        self.in_progress
            .values()
            .max_by(|a, b| (a.start_time, &a.id).cmp(&(b.start_time, &b.id)))
    }

    /// A tool started. `agent_id` is the subagent that called it (`None`: the
    /// main session). A call already followed is not moved.
    pub fn start_tool(&mut self, id: &str, name: &str, agent_id: Option<&str>, at: SystemTime) {
        if self.in_progress.contains_key(id) {
            return;
        }
        self.in_progress.insert(
            id.to_owned(),
            ToolInProgress {
                id: id.to_owned(),
                name: name.to_owned(),
                start_time: at,
                phase: ToolPhase::Running,
                agent_id: agent_id.map(str::to_owned),
            },
        );
        if self.in_progress.len() <= Self::MAX_IN_PROGRESS {
            return;
        }
        let excess = self.in_progress.len() - Self::MAX_IN_PROGRESS;
        let oldest: Vec<String> = self
            .in_progress()
            .into_iter()
            .take(excess)
            .map(|tool| tool.id.clone())
            .collect();
        for id in oldest {
            self.in_progress.remove(&id);
        }
    }

    /// The tool finished (successfully or not).
    pub fn complete_tool(&mut self, id: &str) {
        self.in_progress.remove(id);
    }

    /// The tool waits for permission, or runs again once it was granted.
    pub fn set_phase(&mut self, phase: ToolPhase, id: &str) {
        if let Some(tool) = self.in_progress.get_mut(id) {
            tool.phase = phase;
        }
    }

    /// The main turn ended: its calls are over; background agents' go on.
    pub fn end_main_turn(&mut self) {
        self.in_progress.retain(|_, tool| tool.agent_id.is_some());
    }
}

// ---- subagents ----

/// A tool call of a subagent seen live through hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentToolCall {
    pub id: String,
    pub name: String,
    pub input: BTreeMap<String, String>,
    /// One of the `chat` status words (`running`, `success`, `error`, …).
    pub status: String,
    pub timestamp: SystemTime,
}

/// An active Task (subagent) tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContext {
    pub task_tool_id: String,
    pub start_time: SystemTime,
    pub agent_id: Option<String>,
    pub description: Option<String>,
    pub subagent_tools: Vec<SubagentToolCall>,
}

/// State for Task (subagent) tools and their nested tools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentState {
    /// Active Task tools, keyed by task tool_use_id.
    pub active_tasks: BTreeMap<String, TaskContext>,
    /// Active task ids, most recent last (insertion order is how parallel
    /// Tasks tell their tools apart).
    pub task_stack: Vec<String>,
    /// agentId → the Task's description.
    pub agent_descriptions: BTreeMap<String, String>,
}

impl SubagentState {
    pub fn new() -> SubagentState {
        SubagentState::default()
    }

    /// There is an active subagent.
    pub fn has_active_subagent(&self) -> bool {
        !self.active_tasks.is_empty()
    }

    /// Start tracking a Task tool.
    pub fn start_task(&mut self, task_tool_id: &str, description: Option<&str>, at: SystemTime) {
        self.active_tasks.insert(
            task_tool_id.to_owned(),
            TaskContext {
                task_tool_id: task_tool_id.to_owned(),
                start_time: at,
                agent_id: None,
                description: description.map(str::to_owned),
                subagent_tools: Vec::new(),
            },
        );
    }

    /// Stop tracking a Task tool.
    pub fn stop_task(&mut self, task_tool_id: &str) {
        self.active_tasks.remove(task_tool_id);
    }

    /// Add a subagent tool to the most recent active Task (the latest start;
    /// the larger id wins a tie).
    pub fn add_subagent_tool(&mut self, tool: SubagentToolCall) {
        let newest = self
            .active_tasks
            .iter()
            .max_by(|a, b| (a.1.start_time, a.0).cmp(&(b.1.start_time, b.0)))
            .map(|(id, _)| id.clone());
        if let Some(task) = newest.and_then(|id| self.active_tasks.get_mut(&id)) {
            task.subagent_tools.push(tool);
        }
    }

    /// Update the status of a subagent tool across all active Tasks.
    pub fn update_subagent_tool_status(&mut self, tool_id: &str, status: &str) {
        for task in self.active_tasks.values_mut() {
            if let Some(tool) = task.subagent_tools.iter_mut().find(|t| t.id == tool_id) {
                tool.status = status.to_owned();
                return;
            }
        }
    }
}

/// Agent calls read to the end for good (the newest
/// [`SettledAgents::MAX`]: an Agent result is read once, so only a
/// whole-transcript read, such as a chat opening, meets an old one again).
#[derive(Debug, Clone, Default)]
pub struct SettledAgents {
    set: BTreeSet<String>,
    order: VecDeque<String>,
}

impl SettledAgents {
    pub const MAX: usize = 512;

    pub fn new() -> SettledAgents {
        SettledAgents::default()
    }

    pub fn contains(&self, tool_use_id: &str) -> bool {
        self.set.contains(tool_use_id)
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Stops following an agent for good. The oldest are forgotten in
    /// batches once the list is twice the bound.
    pub fn settle(&mut self, tool_use_id: &str) {
        if !self.set.insert(tool_use_id.to_owned()) {
            return;
        }
        self.order.push_back(tool_use_id.to_owned());
        if self.order.len() > Self::MAX * 2 {
            while self.order.len() > Self::MAX {
                if let Some(forgotten) = self.order.pop_front() {
                    self.set.remove(&forgotten);
                }
            }
        }
    }
}

/// One Agent (Task) call whose subagent transcript is followed.
#[derive(Debug, Clone)]
pub struct AgentTrack {
    pub agent_id: String,
    /// Its result says it completed (not launched in the background).
    pub is_finished: bool,
    pub added_at: SystemTime,
    pub transcript: Option<SubagentTranscript>,
    pub last_change_at: Option<SystemTime>,
    /// Syncs that found no transcript file yet.
    pub missing_checks: u32,
}

// ---- the record ----

/// The engine's per-session record. Every other package reads a session
/// through [`SessionView`] (see [`Session::to_view`]).
#[derive(Debug, Clone)]
pub struct Session {
    // Identity.
    pub id: SessionId,
    /// Working directory the session started in (identity; never changes).
    pub cwd: String,
    pub project_name: String,
    /// Latest working directory an event reported, for display.
    pub current_cwd: String,

    // Instance.
    pub pid: Option<u32>,
    /// When `pid` started (from the kernel), so a reused pid isn't taken for
    /// the same Claude process.
    pub pid_started_at: Option<SystemTime>,
    pub terminal: Option<HookTerminal>,

    // Account and origin.
    /// Absolute transcript path, from the hook when available.
    pub transcript_path: Option<String>,
    /// The account (normalized config folder) the session runs under.
    pub account: Option<AccountId>,
    /// Set by the hub's projection; the engine never derives a ring.
    pub ring: Option<RingId>,
    pub attribution: Attribution,
    /// When the current attribution began.
    pub attribution_since: SystemTime,
    /// Raw `CLAUDE_CONFIG_DIR` of the Claude process; `None` for the default.
    pub config_dir_env: Option<String>,
    pub entrypoint: Option<String>,
    /// Claude Desktop's id for the session, when Desktop hosts it.
    pub host_session_id: Option<String>,
    /// The identity Claude Desktop's own record names for a hosted session.
    pub desktop_identity: Option<IdentityId>,

    // Details.
    pub session_title: Option<String>,
    pub title_source: Option<SessionTitleSource>,
    /// The registry's `name` when Claude Code made it up, so the same string
    /// from the status line isn't mistaken for a name the user chose.
    pub derived_name: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,

    // Turn and review tracking.
    /// Text of Claude's final reply in the last completed turn (Stop).
    pub last_assistant_message: Option<String>,
    /// When the current (or last) turn started (UserPromptSubmit).
    pub turn_started_at: Option<SystemTime>,
    /// When Claude last finished a turn the user asked for.
    pub completed_at: Option<SystemTime>,
    /// When the user last looked at the session.
    pub reviewed_at: Option<SystemTime>,
    /// A main-session Stop not yet confirmed as the end of its turn: Claude
    /// Code runs Stop hooks after ours, and a blocking one makes Claude
    /// continue. `completed_at` is set from this once confirmed.
    pub completion_pending_since: Option<SystemTime>,
    /// A turn may have ended where no hook said so: the next transcript sync
    /// decides whether it completed. The earliest the completion can be.
    pub completion_check_since: Option<SystemTime>,
    /// Humanised StopFailure error of the last turn.
    pub stop_error: Option<String>,
    /// StopFailure's raw `error` code.
    pub stop_error_code: Option<String>,
    /// When the last turn failed.
    pub failed_at: Option<SystemTime>,
    needs_input_reason: Option<NeedsInputReason>,
    needs_input_since: Option<SystemTime>,
    /// Background tasks still running after the turn ended.
    pub background_task_count: u32,
    /// Of those, the ones the turn waits for (subagents, workflows,
    /// teammates, cloud sessions).
    pub background_agent_count: u32,
    /// Their `type` labels, for "Waiting on 1 workflow".
    pub background_agent_types: Vec<String>,
    /// Set by a Stop that left such agents running: the session shows as
    /// working, not ready for review, until they are done.
    pub background_wait_since: Option<SystemTime>,
    /// Crons and wake-ups scheduled at the last Stop.
    pub scheduled_wakeup_count: u32,
    /// `source` of the prompt that started the current or last turn.
    pub last_prompt_source: Option<String>,
    /// The user typed the prompt that started the current or last turn.
    pub last_prompt_was_user_authored: bool,
    /// Waking agents and crons as the last Stop left them (a prompt does not
    /// reset them, so a turn interrupted before its Stop keeps them)...
    pub known_waking_agents: u32,
    pub known_wakeups: u32,
    /// ...and as they stood when the current or last turn started: a
    /// completion is quiet only when its own turn added one.
    pub agents_at_turn_start: u32,
    pub wakeups_at_turn_start: u32,

    // Progress and usage.
    pub tasks: TaskList,
    /// Context window used, 0...100: the status line's, else estimated from
    /// the transcript.
    pub context_used_percent: Option<f64>,
    /// Context window size in tokens, when the status line reported it.
    pub context_window_size: Option<u64>,
    /// When the status line last reported the context window (it then wins
    /// over estimates).
    pub status_line_updated_at: Option<SystemTime>,
    /// Session cost in USD from the status line.
    pub cost_usd: Option<f64>,

    // State machine.
    pub phase: Phase,
    /// Further permission requests waiting behind the one in `phase`, oldest
    /// first, each with its full context.
    pub queued_approvals: Vec<PermissionContext>,
    /// Phase to return to once the pending approvals are answered.
    pub phase_after_approvals: Phase,

    // Chat, tools, subagents, transcript.
    pub chat: ChatState,
    pub tool_tracker: ToolTracker,
    pub subagent_state: SubagentState,
    pub conversation_info: ConversationInfo,
    /// How far this session's transcript was read.
    pub cursor: TranscriptCursor,
    /// The fold of every transcript entry read so far.
    pub fold: TranscriptSummary,
    /// Agent calls whose subagent transcript is followed, by tool_use_id.
    pub agents: BTreeMap<String, AgentTrack>,
    pub settled_agents: SettledAgents,

    // Timestamps.
    pub last_activity: SystemTime,
    pub created_at: SystemTime,
    /// The last hook event, status line update or registry change applied.
    pub last_event_at: SystemTime,
    /// The last hook event only: registry corrections apply when the
    /// registry changed after it; status line updates don't move it.
    pub last_hook_event_at: Option<SystemTime>,
    /// The session registry's last status (`busy`, `idle`, `shell`,
    /// `waiting`) and when it changed.
    pub registry_status: Option<String>,
    pub registry_status_changed_at: Option<SystemTime>,

    /// The home folder rules for paths shown in requests (`~`); `None` shows
    /// them as sent.
    pub paths: Option<Paths>,
}

impl Session {
    /// A new, idle session first seen at `at`.
    pub fn new(id: impl Into<SessionId>, cwd: impl Into<String>, at: SystemTime) -> Session {
        let cwd = cwd.into();
        let project_name = tool_input::file_name(&cwd).to_owned();
        Session {
            id: id.into(),
            current_cwd: cwd.clone(),
            cwd,
            project_name,
            pid: None,
            pid_started_at: None,
            terminal: None,
            transcript_path: None,
            account: None,
            ring: None,
            attribution: Attribution::Known(None),
            attribution_since: at,
            config_dir_env: None,
            entrypoint: None,
            host_session_id: None,
            desktop_identity: None,
            session_title: None,
            title_source: None,
            derived_name: None,
            model: None,
            permission_mode: None,
            last_assistant_message: None,
            turn_started_at: None,
            completed_at: None,
            reviewed_at: None,
            completion_pending_since: None,
            completion_check_since: None,
            stop_error: None,
            stop_error_code: None,
            failed_at: None,
            needs_input_reason: None,
            needs_input_since: None,
            background_task_count: 0,
            background_agent_count: 0,
            background_agent_types: Vec::new(),
            background_wait_since: None,
            scheduled_wakeup_count: 0,
            last_prompt_source: None,
            last_prompt_was_user_authored: false,
            known_waking_agents: 0,
            known_wakeups: 0,
            agents_at_turn_start: 0,
            wakeups_at_turn_start: 0,
            tasks: TaskList::new(),
            context_used_percent: None,
            context_window_size: None,
            status_line_updated_at: None,
            cost_usd: None,
            phase: Phase::Idle,
            queued_approvals: Vec::new(),
            phase_after_approvals: Phase::Processing,
            chat: ChatState::new(),
            tool_tracker: ToolTracker::new(),
            subagent_state: SubagentState::new(),
            conversation_info: ConversationInfo::default(),
            cursor: TranscriptCursor::default(),
            fold: TranscriptSummary::new(),
            agents: BTreeMap::new(),
            settled_agents: SettledAgents::new(),
            last_activity: at,
            created_at: at,
            last_event_at: at,
            last_hook_event_at: None,
            registry_status: None,
            registry_status_changed_at: None,
            paths: None,
        }
    }

    // ---- needs input ----

    /// The explicit reason the session is blocked on the user, beyond a
    /// pending approval.
    pub fn needs_input_reason(&self) -> Option<&NeedsInputReason> {
        self.needs_input_reason.as_ref()
    }

    /// When the reason was set (`None` while there is none).
    pub fn needs_input_since(&self) -> Option<SystemTime> {
        self.needs_input_since
    }

    /// Sets the needs-input reason as of `at` (when the event saying so
    /// arrived, not when it was processed). A reason that only changes keeps
    /// its original time; clearing it forgets the time.
    pub fn set_needs_input(&mut self, reason: Option<NeedsInputReason>, at: SystemTime) {
        let was_waiting = self.needs_input_reason.is_some();
        match &reason {
            None => self.needs_input_since = None,
            Some(_) if !was_waiting => self.needs_input_since = Some(at),
            Some(_) => {}
        }
        self.needs_input_reason = reason;
    }

    // ---- derived ----

    /// The active permission context, if any.
    pub fn active_permission(&self) -> Option<&PermissionContext> {
        match &self.phase {
            Phase::WaitingForApproval(context) => Some(context),
            _ => None,
        }
    }

    /// When the session started waiting for the user: its shown request's
    /// activation, else when the needs-input reason was set.
    pub fn waiting_since(&self) -> Option<SystemTime> {
        match self.active_permission() {
            Some(active) => Some(active.activated_at.unwrap_or(active.received_at)),
            None => self.needs_input_since,
        }
    }

    /// Every request waiting for an answer: the active one first, then the
    /// queue.
    pub fn pending_permissions(&self) -> Vec<&PermissionContext> {
        self.active_permission()
            .into_iter()
            .chain(self.queued_approvals.iter())
            .collect()
    }

    /// A pending request by tool_use_id, active or queued.
    pub fn pending_permission(&self, tool_use_id: &str) -> Option<&PermissionContext> {
        self.pending_permissions()
            .into_iter()
            .find(|context| context.tool_use_id == tool_use_id)
    }

    /// Hooks report this session (not only the registry or status line).
    pub fn is_hook_backed(&self) -> bool {
        self.last_hook_event_at.is_some()
    }

    /// The turn failed (StopFailure), as opposed to waiting on something the
    /// user can answer.
    pub fn has_failed_turn(&self) -> bool {
        self.needs_input_reason
            .as_ref()
            .is_some_and(NeedsInputReason::is_error)
    }

    /// StopFailure's error, grouped by what the user can do about it; `None`
    /// once the failure no longer blocks the session.
    pub fn stop_error_kind(&self) -> Option<attention::StopErrorKind> {
        if self.stop_error.is_none() || !self.has_failed_turn() {
            return None;
        }
        Some(
            attention::StopErrorKind::from_code(self.stop_error_code.as_deref())
                .unwrap_or(attention::StopErrorKind::Other),
        )
    }

    /// A finished turn that is not worth an alert (it stays in the review
    /// queue): a /loop or cron tick, or a turn that left Claude paused,
    /// waiting to be woken: it scheduled a new wake-up or started agents that
    /// will wake it. Its later, final turn is announced instead.
    ///
    /// A cron or teammate that was already there when the turn began does not
    /// make it quiet: a /loop stays in the session between ticks, and every
    /// turn the user types there must still be announced. A turn the system
    /// started (an agent's result woke Claude) stays quiet while any waking
    /// agent is still out (HS§5.7).
    pub fn completion_is_quiet(&self) -> bool {
        if matches!(
            self.last_prompt_source.as_deref(),
            Some("loop_wakeup" | "schedule_wakeup")
        ) {
            return true;
        }
        if self.scheduled_wakeup_count > self.wakeups_at_turn_start {
            return true;
        }
        if self.last_prompt_was_user_authored {
            self.background_agent_count > self.agents_at_turn_start
        } else {
            self.background_agent_count > 0
        }
    }

    /// What the session needs from the user (see [`attention::derive`]).
    pub fn attention(&self) -> SessionState {
        attention::derive(
            &self.phase,
            self.needs_input_reason.as_ref(),
            self.completed_at,
            self.reviewed_at,
            self.completion_pending_since.is_some(),
            self.background_wait_since.is_some(),
        )
    }

    /// Finished work the user hasn't looked at yet.
    pub fn is_ready_for_review(&self) -> bool {
        self.attention() == SessionState::ReadyForReview
    }

    /// The turn is over and the session waits on the background agents or
    /// workflows it started (not while Claude works on a turn again).
    pub fn is_awaiting_background_work(&self) -> bool {
        self.background_wait_since.is_some()
            && !matches!(self.phase, Phase::Processing | Phase::Compacting)
    }

    /// "1 workflow", "2 background agents and 1 teammate": what the session
    /// waits on while [`Self::is_awaiting_background_work`], else `None`.
    pub fn background_wait_description(&self) -> Option<String> {
        if !self.is_awaiting_background_work() {
            return None;
        }
        background::phrase(&self.background_agent_types)
    }

    /// The panel's title: session title > transcript summary > first user
    /// message > derived registry name > project name.
    pub fn display_title(&self) -> String {
        if let Some(title) = &self.session_title {
            if !title.is_empty() && self.title_source != Some(SessionTitleSource::DerivedName) {
                return title.clone();
            }
        }
        self.conversation_info
            .summary
            .clone()
            .or_else(|| self.conversation_info.first_user_message.clone())
            .or_else(|| self.session_title.clone())
            .unwrap_or_else(|| self.project_name.clone())
    }

    /// Project folder name of the latest working directory, for display.
    pub fn display_project_name(&self) -> String {
        tool_input::file_name(&self.current_cwd).to_owned()
    }

    /// The title a person chose or Claude Code made, if the session has one
    /// that isn't only the folder's name.
    fn own_title(&self) -> Option<String> {
        if self.title_source != Some(SessionTitleSource::DerivedName) {
            if let Some(title) = usable(self.session_title.as_deref()) {
                return Some(title);
            }
        }
        usable(self.conversation_info.summary.as_deref())
    }

    /// A session's name outside the panel (hover rows and the phone link,
    /// banners, the cloud): its title from a hook, the registry or the
    /// transcript, else the project folder. Never the first prompt, which
    /// [`Self::display_title`] falls back to until Claude Code names the
    /// session: rows travel to the phone over the network, and banners show
    /// on screen and in the notification centre.
    pub fn public_title(&self) -> String {
        if let Some(title) = self.own_title() {
            return title;
        }
        if let Some(title) = usable(self.session_title.as_deref()) {
            return title;
        }
        usable(Some(&self.project_name)).unwrap_or_else(|| self.display_project_name())
    }

    /// `public_title` is only a name made from the folder: the session has no
    /// title and no summary of its own, so the cloud sends none.
    pub fn title_from_folder(&self) -> bool {
        self.own_title().is_none()
    }

    /// Sets the title unless a higher-priority source already provided one.
    /// Returns true if the title changed.
    pub fn apply_title(&mut self, title: Option<&str>, source: SessionTitleSource) -> bool {
        let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) else {
            return false;
        };
        if self.title_source.is_some_and(|current| current > source) {
            return false;
        }
        if self.session_title.as_deref() == Some(title) && self.title_source == Some(source) {
            return false;
        }
        self.session_title = Some(title.to_owned());
        self.title_source = Some(source);
        true
    }

    /// Applies a registry or status line name. A name equal to the registry's
    /// derived name ranks below transcript titles, whichever source reported
    /// it.
    pub fn apply_name(&mut self, name: Option<&str>, is_derived: bool) -> bool {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
            return false;
        };
        if is_derived {
            self.derived_name = Some(name.to_owned());
            // The status line may have reported it first, as a chosen name.
            if self.title_source == Some(SessionTitleSource::Registry)
                && self.session_title.as_deref() == Some(name)
            {
                self.title_source = Some(SessionTitleSource::DerivedName);
                return true;
            }
        }
        let derived = is_derived || self.derived_name.as_deref() == Some(name);
        self.apply_title(
            Some(name),
            if derived {
                SessionTitleSource::DerivedName
            } else {
                SessionTitleSource::Registry
            },
        )
    }

    // ---- requests ----

    /// Every request waiting for an answer, the active one first, as the UI
    /// sees them.
    pub fn pending_requests(&self) -> Vec<PendingRequest> {
        self.pending_permissions()
            .into_iter()
            .map(|context| self.request_of(context))
            .collect()
    }

    fn request_of(&self, context: &PermissionContext) -> PendingRequest {
        let kind = match context.tool_name.as_str() {
            "AskUserQuestion" => RequestKind::Question,
            "ExitPlanMode" => RequestKind::Plan,
            _ => RequestKind::Permission,
        };
        let questions = (kind == RequestKind::Question)
            .then(|| parse_questions(&context.tool_input))
            .filter(|questions| !questions.is_empty());
        let plan_markdown = (kind == RequestKind::Plan)
            .then(|| {
                context
                    .tool_input
                    .get("plan")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten();
        let preview = match kind {
            RequestKind::Permission => {
                permission_preview(&context.tool_name, &context.tool_input, self.paths.as_ref())
            }
            // The first question is what the row says.
            RequestKind::Question => questions
                .as_ref()
                .and_then(|questions| questions.first())
                .map(|question| question.text.trim().to_owned()),
            RequestKind::Plan => None,
        };
        PendingRequest {
            session_id: self.id.clone(),
            tool_use_id: context.tool_use_id.clone(),
            kind,
            tool_name: context.tool_name.clone(),
            received_at: context.received_at,
            activated_at: context.activated_at,
            needs_review: kind == RequestKind::Permission
                && is_too_long_to_review_inline(preview.as_deref()),
            input_preview: preview.unwrap_or_default(),
            input: context.tool_input.clone(),
            always: always_rule(context),
            questions,
            plan_markdown,
            agent_id: context.agent_id.clone(),
        }
    }

    // ---- the view ----

    /// The record as every other package reads it.
    pub fn to_view(&self) -> SessionView {
        let info = &self.conversation_info;
        SessionView {
            id: self.id.clone(),
            account: self.account.clone(),
            ring: self.ring.clone(),
            attribution: self.attribution.clone(),
            attribution_since: self.attribution_since,
            cwd: PathBuf::from(&self.cwd),
            project_name: self.project_name.clone(),
            title: self.display_title(),
            title_from_folder: self.title_from_folder(),
            state: self.attention(),
            phase: self.phase.clone(),
            pid: self.pid,
            pid_started: self.pid_started_at,
            entrypoint: self.entrypoint.clone(),
            config_dir_env: self.config_dir_env.clone(),
            host_session_id: self.host_session_id.clone(),
            registry_status: self.registry_status.clone(),
            first_seen_at: self.created_at,
            model: self.model.clone(),
            context_pct: self
                .context_used_percent
                .filter(|pct| pct.is_finite())
                .map(|pct| pct.clamp(0.0, 100.0)),
            tasks: self.tasks.progress(),
            background: BackgroundWait {
                since: self.background_wait_since,
                agent_types: self.background_agent_types.clone(),
                task_count: self.background_task_count,
            },
            last_activity: self.last_activity,
            turn_started_at: self.turn_started_at,
            completed_at: self.completed_at,
            reviewed_at: self.reviewed_at,
            last_assistant_message: self.last_assistant_message.clone(),
            pending: self.pending_requests(),
            cost_usd: self.cost_usd,
            transcript_path: self.transcript_path.as_ref().map(PathBuf::from),
            current_cwd: PathBuf::from(&self.current_cwd),
            display_project_name: self.display_project_name(),
            public_title: self.public_title(),
            summary: info.summary.clone(),
            last_message: info.last_message.clone(),
            last_message_role: info.last_message_role.clone(),
            last_tool_name: info.last_tool_name.clone(),
            is_hook_backed: self.is_hook_backed(),
            last_hook_event_at: self.last_hook_event_at,
            last_event_at: self.last_event_at,
            running_tools: self
                .tool_tracker
                .in_progress()
                .into_iter()
                .map(|tool| RunningTool {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    started_at: tool.start_time,
                    agent_id: tool.agent_id.clone(),
                    pending_approval: tool.phase == ToolPhase::PendingApproval,
                })
                .collect(),
            waiting_since: self.waiting_since(),
            completion_pending_since: self.completion_pending_since,
            completion_quiet: self.completion_is_quiet(),
            background_wait_description: self.background_wait_description(),
            permission_mode: self.permission_mode.clone(),
            context_window_size: self.context_window_size,
            terminal: self.terminal.clone(),
            desktop_identity: self.desktop_identity.clone(),
            is_desktop_hosted: desktop::is_desktop_hosted(self.entrypoint.as_deref()),
        }
    }

    /// The session's chat as the panel shows it.
    pub fn chat_history(&self) -> ChatHistory {
        self.chat.history()
    }
}

/// The text collapsed to one line (any run of white space is one space);
/// `None` when nothing is left.
fn usable(text: Option<&str>) -> Option<String> {
    let line = text?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

// ---- request text ----

/// Lines the row gives a request before sending it to the chat.
pub const INLINE_LINE_LIMIT: usize = 4;
/// About as many characters as [`INLINE_LINE_LIMIT`] lines hold at the
/// narrowest panel width.
pub const INLINE_CHARACTER_LIMIT: usize = 200;

/// A permission request as the row shows it (PermissionPreview): the whole
/// command, not the engine's 100-character cut, and a full path (under `~`)
/// rather than the file name alone, so nothing that matters hides past the
/// edge.
pub fn permission_preview(tool_name: &str, input: &Value, paths: Option<&Paths>) -> Option<String> {
    let input = input.as_object()?;
    let string = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    };
    if tool_name == "Bash" {
        if let Some(command) = string("command") {
            return Some(command.trim().to_owned());
        }
    }
    for key in ["file_path", "notebook_path", "path"] {
        if let Some(path) = string(key) {
            return Some(match paths {
                Some(paths) => paths.abbreviate(path),
                None => path.to_owned(),
            });
        }
    }
    for key in ["command", "url", "query", "pattern"] {
        if let Some(value) = string(key) {
            return Some(value.to_owned());
        }
    }
    let mut keys: Vec<&String> = input.keys().collect();
    keys.sort();
    keys.into_iter()
        .filter(|key| key.as_str() != "description")
        .find_map(|key| string(key))
        .map(str::to_owned)
}

/// Whether the request is too long to approve from the row.
pub fn is_too_long_to_review_inline(text: Option<&str>) -> bool {
    let Some(text) = text else {
        return false;
    };
    text.split('\n').count() > INLINE_LINE_LIMIT || text.chars().count() > INLINE_CHARACTER_LIMIT
}

/// What the first permission suggestion ("Always allow") would do, in words:
/// "Don't ask again for Bash(npm run test:*) in this project (just you)".
pub fn describe_suggestion(suggestion: &Value) -> Option<String> {
    let kind = suggestion.get("type")?.as_str()?;
    let destination = match suggestion.get("destination").and_then(Value::as_str) {
        Some("session") => Some("for this session"),
        Some("localSettings") => Some("in this project (just you)"),
        Some("projectSettings") => Some("in this project (shared)"),
        Some("userSettings") => Some("in all projects"),
        _ => None,
    };
    let suffix = destination.map(|d| format!(" {d}")).unwrap_or_default();
    match kind {
        "addRules" | "replaceRules" => {
            let rules: Vec<String> = suggestion
                .get("rules")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|rule| {
                    let tool = rule
                        .get("toolName")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())?;
                    Some(
                        match rule
                            .get("ruleContent")
                            .and_then(Value::as_str)
                            .filter(|c| !c.is_empty())
                        {
                            Some(content) => format!("{tool}({content})"),
                            None => tool.to_owned(),
                        },
                    )
                })
                .collect();
            (!rules.is_empty()).then(|| format!("Don't ask again for {}{suffix}", rules.join(", ")))
        }
        "setMode" => {
            let mode = suggestion.get("mode")?.as_str()?;
            let name = match mode {
                "acceptEdits" => "accept-edits mode".to_owned(),
                "bypassPermissions" => "bypass-permissions mode".to_owned(),
                "plan" => "plan mode".to_owned(),
                "default" => "default mode".to_owned(),
                other => format!("{other} mode"),
            };
            Some(format!("Switch to {name}{suffix}"))
        }
        "addDirectories" => {
            let names: Vec<&str> = suggestion
                .get("directories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(tool_input::file_name)
                .collect();
            (!names.is_empty()).then(|| format!("Allow access to {}{suffix}", names.join(", ")))
        }
        _ => None,
    }
}

/// "Always allow" for the request, only for a narrow rule (addRules or
/// replaceRules to the session or localSettings) that has a description
/// (D§4.7): anything wider, or a mode change, is answered where the user can
/// read it in full, in the terminal.
fn always_rule(context: &PermissionContext) -> Option<AlwaysRule> {
    let suggestion = context.permission_suggestions.first()?;
    let narrow = context.always_allow_suggestion()?.is_narrow();
    let description = describe_suggestion(suggestion)?;
    narrow.then(|| AlwaysRule {
        description,
        suggestion: suggestion.clone(),
        inline: true,
    })
}

/// Questions from AskUserQuestion's input:
/// `{"questions": [{"question", "header", "multiSelect", "options":
/// [{"label", "description"}]}]}`. Malformed entries are skipped; options
/// without a label are dropped. The answer key is the question's text
/// exactly as sent.
pub fn parse_questions(input: &Value) -> Vec<Question> {
    let Some(raw) = input.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    let trimmed = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    let mut questions = Vec::new();
    for entry in raw {
        let Some(text) = entry.get("question").and_then(Value::as_str) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let options = entry
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                // Bare strings are options too.
                if let Some(label) = option.as_str() {
                    let label = label.trim();
                    return (!label.is_empty()).then(|| QuestionOption {
                        label: label.to_owned(),
                        description: None,
                    });
                }
                Some(QuestionOption {
                    label: trimmed(option.get("label"))?,
                    description: trimmed(option.get("description")),
                })
            })
            .collect();
        let multi_select = match entry.get("multiSelect") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
            Some(Value::String(text)) => text.to_lowercase() == "true",
            _ => false,
        };
        questions.push(Question {
            text: text.to_owned(),
            header: trimmed(entry.get("header")),
            multi_select,
            options,
        });
    }
    questions
}
