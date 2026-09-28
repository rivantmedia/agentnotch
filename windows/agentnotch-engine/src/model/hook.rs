//! What arrives over the hook pipe, decoded (HS§1.4, §2, §4; HookEvent.swift
//! field for field, plus the Windows fields). `ingress` builds these from
//! frames with the Mac's lenient rules (HS§4.2); nothing else decodes frames.

use crate::model::{AccountId, SessionId, UsageWindow};
use crate::platform::ConnId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::SystemTime;

/// The two terminal variables a hook forwards (never the environment).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookTerminal {
    /// `WT_SESSION`: Windows Terminal's id of the tab's session.
    pub wt_session: Option<String>,
    /// `TERM_PROGRAM` (`vscode` in VS Code's integrated terminal).
    pub term_program: Option<String>,
}

/// One hook event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookEvent {
    pub session_id: SessionId,
    pub event: String,
    /// `""` when the hook sent none.
    pub cwd: String,
    /// The hook's coarse status (`running_tool`, `waiting_for_input`, …;
    /// `unknown` when missing).
    pub status: String,
    /// Claude Code's pid (`CLAUDE_PID`, else the hook's walk), 1..=2³¹−1.
    pub pid: Option<u32>,
    pub transcript_path: Option<String>,
    /// `CLAUDE_CONFIG_DIR` verbatim.
    pub config_dir_env: Option<String>,
    /// `CLAUDE_CODE_SESSION_ATTENDED`: false marks unattended sessions.
    pub attended: Option<bool>,
    /// `CLAUDE_CODE_ENTRYPOINT` (`cli`, `claude-vscode`, `sdk-ts`, …).
    pub entrypoint: Option<String>,
    /// Present only for subagent events.
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub permission_mode: Option<String>,

    // Tool events.
    pub tool: Option<String>,
    pub tool_input: Option<Map<String, Value>>,
    /// Absent on PermissionRequest until ingress matched it to a PreToolUse.
    pub tool_use_id: Option<String>,
    /// Ingress made the id up (`permission-<uuid>`): no PreToolUse matched.
    pub has_synthetic_tool_use_id: bool,
    pub tool_error: Option<String>,
    pub is_interrupt: Option<bool>,
    pub permission_suggestions: Option<Vec<Value>>,
    pub denial_reason: Option<String>,

    // Tasks (TaskCreate's PostToolUse, TaskCreated, TaskCompleted).
    pub task_id: Option<String>,
    pub task_subject: Option<String>,

    // Notification.
    pub notification_type: Option<String>,
    pub message: Option<String>,
    pub title: Option<String>,

    // Stop, StopFailure, SubagentStop.
    pub last_assistant_message: Option<String>,
    pub background_task_count: Option<u32>,
    /// The `type` of each running background task (at most 64).
    pub background_task_types: Option<Vec<String>>,
    pub session_cron_count: Option<u32>,
    pub stop_hook_active: Option<bool>,
    pub stop_error: Option<String>,
    pub stop_error_details: Option<String>,
    pub agent_transcript_path: Option<String>,

    // SessionStart, UserPromptSubmit, SessionEnd, compaction.
    pub source: Option<String>,
    pub model: Option<String>,
    pub session_title: Option<String>,
    pub prompt: Option<String>,
    pub reason: Option<String>,
    pub trigger: Option<String>,

    // Windows.
    /// The frame's `protocol` (1 when missing).
    pub protocol: u32,
    /// The hook exe's own pid.
    pub hook_pid: Option<u32>,
    pub terminal: Option<HookTerminal>,

    /// When the server read the frame; every store time uses it.
    pub received_at: SystemTime,
}

impl HookEvent {
    /// An event with only its session, name and arrival time (tests, and
    /// events the engine synthesises).
    pub fn new(
        session_id: impl Into<SessionId>,
        event: impl Into<String>,
        received_at: SystemTime,
    ) -> HookEvent {
        HookEvent {
            session_id: session_id.into(),
            event: event.into(),
            cwd: String::new(),
            status: "unknown".into(),
            pid: None,
            transcript_path: None,
            config_dir_env: None,
            attended: None,
            entrypoint: None,
            agent_id: None,
            agent_type: None,
            permission_mode: None,
            tool: None,
            tool_input: None,
            tool_use_id: None,
            has_synthetic_tool_use_id: false,
            tool_error: None,
            is_interrupt: None,
            permission_suggestions: None,
            denial_reason: None,
            task_id: None,
            task_subject: None,
            notification_type: None,
            message: None,
            title: None,
            last_assistant_message: None,
            background_task_count: None,
            background_task_types: None,
            session_cron_count: None,
            stop_hook_active: None,
            stop_error: None,
            stop_error_details: None,
            agent_transcript_path: None,
            source: None,
            model: None,
            session_title: None,
            prompt: None,
            reason: None,
            trigger: None,
            protocol: agentnotch_proto::PROTOCOL,
            hook_pid: None,
            terminal: None,
            received_at,
        }
    }

    /// A PermissionRequest waits for an answer.
    pub fn expects_response(&self) -> bool {
        self.event == "PermissionRequest"
    }

    /// Fired inside a subagent (Agent/Task tool), not by the main session.
    pub fn is_subagent_event(&self) -> bool {
        self.agent_id.as_deref().is_some_and(|id| !id.is_empty())
    }

    /// Unattended sessions and non-interactive SDK entrypoints (`sdk-ts`,
    /// `sdk-py`, `sdk-cli` for `claude -p`) are ignored; the terminal CLI and
    /// the VS Code extension (`claude-vscode`) are kept (HS§4.3).
    pub fn is_from_ignored_session(&self) -> bool {
        if self.attended == Some(false) {
            return true;
        }
        self.entrypoint
            .as_deref()
            .is_some_and(|entry| !entry.is_empty() && entry.to_lowercase().starts_with("sdk"))
    }
}

/// A status line message (`StatusLineMessage` + `StatusLineUpdate`): the
/// raw rate limits plus what ingress and the usage parser read from them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusLineMessage {
    pub session_id: SessionId,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub config_dir_env: Option<String>,
    /// The folder derived from `transcript_path` (preferred) or
    /// `config_dir_env` (`core::paths::Paths::session_config_dir`).
    pub account_id: Option<AccountId>,
    pub received_at: SystemTime,
    /// `rate_limits` verbatim; parsed by `usage` (UsageParser's rules).
    pub rate_limits: Option<Value>,
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    /// `context_window.used_percentage`, 0..=100.
    pub context_used_percent: Option<f64>,
    pub context_window_size: Option<u64>,
    pub model_id: Option<String>,
    pub model_display_name: Option<String>,
    pub cost_usd: Option<f64>,
    pub session_name: Option<String>,
    pub claude_code_version: Option<String>,
    /// `CLAUDE_PID` from the wrapper (no parent-pid fallback). Rate limits
    /// are per process.
    pub pid: Option<u32>,
}

/// A PermissionRequest ingress holds open until it is answered or released
/// (the Mac's `PendingPermission`, HS§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeldPermission {
    pub conn: ConnId,
    pub session_id: SessionId,
    pub tool_use_id: String,
    pub has_synthetic_tool_use_id: bool,
    /// The subagent that asked; `None` for the main session.
    pub agent_id: Option<String>,
    /// The request as it arrived, `tool_use_id` filled in.
    pub event: HookEvent,
    pub received_at: SystemTime,
}
