//! Sessions and transitions for the control tests, in the shape the session
//! store hands them on.
#![allow(dead_code)]

use agentnotch_engine::core::time::from_ms;
use agentnotch_engine::model::*;
use agentnotch_engine::platform::{ConsoleInfo, HostApp, HostKind, NotifyPermission};
use agentnotch_engine::runtime_types::{PanelState, ReactionContext, ToastContext};
use agentnotch_engine::testkit::TEST_START_MS;
use serde_json::Value;
use std::path::PathBuf;
use std::time::SystemTime;

/// When the test sessions were last active.
pub fn t0() -> SystemTime {
    from_ms(TEST_START_MS)
}

pub const RING: &str = "claude-acct-1a2b3c4d5e6f";

/// An idle session in a terminal, with a title of its own.
pub fn view(id: &str) -> SessionView {
    SessionView {
        id: SessionId::from(id),
        account: None,
        ring: Some(RingId::from(RING)),
        attribution: Attribution::Known(None),
        attribution_since: t0(),
        cwd: PathBuf::from(r"C:\Users\me\code\app"),
        project_name: "app".into(),
        title: "Refactor the parser".into(),
        title_from_folder: false,
        state: SessionState::Idle,
        phase: Phase::WaitingForInput,
        pid: Some(4242),
        pid_started: Some(t0()),
        entrypoint: Some("cli".into()),
        config_dir_env: None,
        host_session_id: None,
        registry_status: None,
        first_seen_at: t0(),
        model: None,
        context_pct: None,
        tasks: None,
        background: BackgroundWait::default(),
        last_activity: t0(),
        turn_started_at: None,
        completed_at: None,
        reviewed_at: None,
        last_assistant_message: None,
        pending: Vec::new(),
        cost_usd: None,
        transcript_path: None,
    }
}

pub fn request_kind(tool: &str) -> RequestKind {
    match tool {
        "AskUserQuestion" => RequestKind::Question,
        "ExitPlanMode" => RequestKind::Plan,
        _ => RequestKind::Permission,
    }
}

/// A held request for `tool` as the session store builds it.
pub fn request(session: &str, tool: &str, input: Value, suggestions: &[Value]) -> PendingRequest {
    PendingRequest {
        session_id: SessionId::from(session),
        tool_use_id: "toolu_1".into(),
        kind: request_kind(tool),
        tool_name: tool.into(),
        received_at: t0(),
        activated_at: Some(t0()),
        input_preview: String::new(),
        input,
        always: suggestions.first().map(|suggestion| AlwaysRule {
            description: "Don't ask again".into(),
            suggestion: suggestion.clone(),
            inline: true,
        }),
        needs_review: false,
        questions: None,
        plan_markdown: None,
        agent_id: None,
    }
}

/// A held request with every field the answer rules read given outright,
/// so a test can make the kind disagree with the tool. `always` is the
/// suggestion "Always allow" sends back, if any.
pub fn pending(
    tool: &str,
    kind: RequestKind,
    input: Value,
    always: Option<Value>,
) -> PendingRequest {
    PendingRequest {
        kind,
        always: always.map(|suggestion| AlwaysRule {
            description: "Don't ask again".into(),
            suggestion,
            inline: true,
        }),
        ..request("s1", tool, input, &[])
    }
}

/// `view`, waiting on a request for Bash.
pub fn waiting_on(mut view: SessionView, tool: &str, input: Value) -> SessionView {
    view.state = SessionState::NeedsYou(NeedsInputReason::for_approval(tool));
    view.phase = Phase::WaitingForApproval(PermissionContext {
        tool_use_id: "toolu_1".into(),
        tool_name: tool.into(),
        tool_input: input.clone(),
        received_at: t0(),
        permission_suggestions: Vec::new(),
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: Some(t0()),
    });
    view.pending = vec![request(view.id.as_str(), tool, input, &[])];
    view
}

pub fn in_state(mut view: SessionView, state: SessionState) -> SessionView {
    view.state = state;
    view
}

pub fn permission(tool: &str) -> SessionState {
    SessionState::NeedsYou(NeedsInputReason::Permission {
        tool: Some(tool.into()),
    })
}

pub fn failed(text: &str, code: &str) -> SessionState {
    SessionState::Failed(NeedsInputReason::Error {
        text: text.into(),
        code: Some(code.into()),
    })
}

/// `view` arriving at its state from `from`.
pub fn transition(view: SessionView, from: Option<SessionState>) -> AttentionTransition {
    AttentionTransition {
        to: view.state.clone(),
        session: view,
        from,
    }
}

pub fn host(kind: HostKind) -> HostApp {
    HostApp {
        kind,
        window: None,
        host_pid: None,
        exe_path: None,
    }
}

/// Claude's own console: attached, in raw mode, Claude alone in it.
pub fn console(pid: u32) -> ConsoleInfo {
    ConsoleInfo {
        attached: true,
        window: Some(0x50_0A12),
        title: Some("✳ Refactor the parser".into()),
        processes: vec![pid],
        line_input: Some(false),
        elevated_target: false,
        error: None,
    }
}

/// Banners allowed and wanted, a single account, `view`'s own title and
/// project as the hub would hand them.
pub fn toast_ctx() -> ToastContext {
    ToastContext {
        now: t0(),
        notify_needs_input: true,
        notify_ready_for_review: true,
        permission: NotifyPermission::Allowed,
        suppressed: false,
        looking_at: None,
        account_label: None,
        multi_account: false,
        title: "Refactor the parser".into(),
        project: "app".into(),
    }
}

/// Auto-open on for needs-you, a sound, peeks of 5 s, the panel closed, no
/// terminal on screen, the user not at that session, the ring shown.
pub fn reaction_ctx() -> ReactionContext {
    ReactionContext {
        now: t0(),
        auto_open: "needsInput".into(),
        sound: true,
        peek: true,
        peek_seconds: 5,
        panel: PanelState::default(),
        full_screen: false,
        any_terminal_visible: false,
        looking_at: Some(false),
        ring_shown: true,
        notch_hidden: false,
    }
}

/// `reaction_ctx` under another auto-open setting.
pub fn reaction_ctx_with(auto_open: &str) -> ReactionContext {
    ReactionContext {
        auto_open: auto_open.into(),
        ..reaction_ctx()
    }
}

/// A session that just began waiting on a permission for Bash.
pub fn needs_you_now(id: &str) -> AttentionTransition {
    transition(
        in_state(view(id), permission("Bash")),
        Some(SessionState::Working),
    )
}

/// A session that just finished and awaits review.
pub fn review_now(id: &str) -> AttentionTransition {
    transition(
        in_state(view(id), SessionState::ReadyForReview),
        Some(SessionState::Working),
    )
}

/// A session whose turn just failed.
pub fn failed_now(id: &str) -> AttentionTransition {
    transition(
        in_state(view(id), failed("Rate limited", "rate_limit")),
        Some(SessionState::Working),
    )
}
