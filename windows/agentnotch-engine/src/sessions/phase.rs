//! The phase state machine (SessionPhase.swift) and what each hook event
//! means for it (SessionEvent.swift's `HookEvent` extensions, HS§5.2).

use crate::model::{HookEvent, PermissionContext, Phase};
use crate::sessions::tool_input;
use serde_json::Value;
use std::mem::discriminant;

/// Whether `from` may move to `to`.
pub fn can_transition(from: &Phase, to: &Phase) -> bool {
    use Phase::*;
    match (from, to) {
        // Terminal state: no transitions out.
        (Ended, _) => false,
        // Any state can end.
        (_, Ended) => true,
        (Idle, Processing | WaitingForApproval(_) | Compacting | WaitingForInput) => true,
        (Processing, WaitingForInput | WaitingForApproval(_) | Compacting | Idle) => true,
        // A request can outlive a missed or late Stop.
        (WaitingForInput, Processing | Idle | Compacting | WaitingForApproval(_)) => true,
        // Approved (tool runs), denied, denied and Claude stopped, another request.
        (WaitingForApproval(_), Processing | Idle | WaitingForInput | WaitingForApproval(_)) => {
            true
        }
        (Compacting, Processing | Idle | WaitingForInput | WaitingForApproval(_)) => true,
        // Staying in a phase is a no-op.
        _ => discriminant(from) == discriminant(to),
    }
}

pub fn is_active(phase: &Phase) -> bool {
    matches!(phase, Phase::Processing | Phase::Compacting)
}

pub fn is_waiting_for_approval(phase: &Phase) -> bool {
    matches!(phase, Phase::WaitingForApproval(_))
}

/// The approval a PermissionRequest carries.
pub fn permission_context(event: &HookEvent) -> PermissionContext {
    PermissionContext {
        tool_use_id: event.tool_use_id.clone().unwrap_or_default(),
        tool_name: event.tool.clone().unwrap_or_else(|| "unknown".into()),
        tool_input: event
            .tool_input
            .clone()
            .map(Value::Object)
            .unwrap_or(Value::Null),
        received_at: event.received_at,
        permission_suggestions: event.permission_suggestions.clone().unwrap_or_default(),
        has_synthetic_tool_use_id: event.has_synthetic_tool_use_id,
        agent_id: if event.is_subagent_event() {
            event.agent_id.clone()
        } else {
            None
        },
        activated_at: None,
    }
}

/// Target phase for an event, or `None` when it must not change the phase
/// (informational notifications, SessionEnd which removes the session).
/// Late-event protection and completion tracking live in the store.
pub fn determine_phase(event: &HookEvent) -> Option<Phase> {
    match event.event.as_str() {
        "PreCompact" => return Some(Phase::Compacting),
        // A manual /compact runs while the prompt is idle; an automatic one mid-turn.
        "PostCompact" => {
            return Some(if event.trigger.as_deref() == Some("manual") {
                Phase::WaitingForInput
            } else {
                Phase::Processing
            })
        }
        "PermissionRequest" => return Some(Phase::WaitingForApproval(permission_context(event))),
        // idle_prompt fires ~60 s after Claude went idle: the turn is over.
        // Every other notification only sets or clears a needs-input reason.
        "Notification" => {
            return (event.notification_type.as_deref() == Some("idle_prompt"))
                .then_some(Phase::WaitingForInput)
        }
        // Fired after every compaction, including an automatic one in the
        // middle of a turn: the turn goes on, so the phase must not move to
        // waitingForInput (the turn's Stop would then not count as a
        // completion). PreCompact / PostCompact drive the phase instead.
        "SessionStart" if event.source.as_deref() == Some("compact") => return None,
        "SessionEnd" => return None,
        _ => {}
    }
    match event.status.as_str() {
        "waiting_for_input" => Some(Phase::WaitingForInput),
        "running_tool" | "processing" | "starting" => Some(Phase::Processing),
        "compacting" => Some(Phase::Compacting),
        // Only PermissionRequest carries an approval context.
        _ => None,
    }
}

/// Events that start or continue a turn of the main session. Every other
/// event that maps to processing (the PostToolUse of a backgrounded Bash, a
/// SubagentStop, TaskCompleted, events from background subagents) can land
/// after Stop and must not drag a finished session back to processing. An
/// automatic compaction never starts a turn: it happens inside one, or in a
/// background agent or teammate after the main Stop (PreCompact/PostCompact
/// carry no agent_id).
pub fn resumes_turn(event: &HookEvent) -> bool {
    if event.is_subagent_event() {
        return false;
    }
    match event.event.as_str() {
        "UserPromptSubmit" | "PreToolUse" => true,
        "PreCompact" | "PostCompact" => event.trigger.as_deref() != Some("auto"),
        _ => false,
    }
}

/// Events after which the transcript is read again.
pub fn should_sync_file(event: &HookEvent) -> bool {
    matches!(
        event.event.as_str(),
        "UserPromptSubmit"
            | "PreToolUse"
            | "PostToolUse"
            | "PostToolUseFailure"
            | "Stop"
            | "StopFailure"
            | "SubagentStop"
    )
}

/// The main session's turn ended (Stop or StopFailure without agent_id).
pub fn ends_main_turn(event: &HookEvent) -> bool {
    !event.is_subagent_event() && (event.event == "Stop" || event.event == "StopFailure")
}

/// Whether this UserPromptSubmit is the user typing (or sending from an
/// editor), which means they have seen the previous result.
/// - `user` (terminal) and a missing source (older Claude Code) count.
/// - `sdk` counts only from an attended editor or desktop host
///   (`claude-vscode`, `claude-desktop`, …): that is how VS Code submits
///   what the user typed. Background-task wake-ups arrive as `sdk` there
///   too, so a prompt that is an injected notification or command echo
///   never counts.
/// - `system`, `loop_wakeup`, `schedule_wakeup`, `poll_event` never count.
pub fn is_user_authored_prompt(event: &HookEvent) -> bool {
    if event.event != "UserPromptSubmit" {
        return false;
    }
    match event.source.as_deref() {
        None | Some("user") => !tool_input::is_injected_prompt(event.prompt.as_deref()),
        Some("sdk") => {
            let editor = event
                .entrypoint
                .as_deref()
                .is_some_and(|e| e.to_lowercase().starts_with("claude-"));
            let prompt = event.prompt.as_deref().filter(|p| !p.is_empty());
            editor && prompt.is_some() && !tool_input::is_injected_prompt(prompt)
        }
        _ => false,
    }
}

/// Agent view's own announcements ("<label> needs your input: …", "<label>
/// finished") fire on the session hosting agent view, but are about another
/// (background) session; they must not flag this one.
pub fn is_agent_view_announcement(event: &HookEvent) -> bool {
    if event.event != "Notification" {
        return false;
    }
    match event.notification_type.as_deref() {
        Some("agent_completed") => true,
        Some("agent_needs_input") => event
            .message
            .as_deref()
            .is_some_and(names_another_session_needing_input),
        _ => false,
    }
}

/// `^.+ needs your input(:|$)`.
fn names_another_session_needing_input(message: &str) -> bool {
    const PHRASE: &str = " needs your input";
    let mut from = 0;
    while let Some(found) = message[from..].find(PHRASE) {
        let at = from + found;
        let rest = &message[at + PHRASE.len()..];
        // `.` never matches a line break.
        if at > 0 && !message[..at].contains('\n') && (rest.is_empty() || rest.starts_with(':')) {
            return true;
        }
        from = at + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn event(name: &str, status: &str) -> HookEvent {
        let mut e = HookEvent::new("s1", name, UNIX_EPOCH);
        e.status = status.into();
        e
    }

    #[test]
    fn transitions() {
        use Phase::*;
        assert!(!can_transition(&Ended, &Processing));
        assert!(can_transition(&Processing, &Ended));
        assert!(can_transition(&Idle, &WaitingForInput));
        assert!(can_transition(&Idle, &Idle));
        assert!(can_transition(&Processing, &Processing));
        assert!(can_transition(&Compacting, &Compacting));
        let approval = WaitingForApproval(permission_context(&event("PermissionRequest", "")));
        assert!(can_transition(&approval, &approval));
        assert!(can_transition(&WaitingForInput, &approval));
    }

    #[test]
    fn phases_by_event() {
        assert_eq!(
            determine_phase(&event("PreCompact", "compacting")),
            Some(Phase::Compacting)
        );
        let mut manual = event("PostCompact", "processing");
        manual.trigger = Some("manual".into());
        assert_eq!(determine_phase(&manual), Some(Phase::WaitingForInput));
        let mut idle = event("Notification", "notification");
        idle.notification_type = Some("idle_prompt".into());
        assert_eq!(determine_phase(&idle), Some(Phase::WaitingForInput));
        idle.notification_type = Some("permission_prompt".into());
        assert_eq!(determine_phase(&idle), None);
        let mut compact = event("SessionStart", "waiting_for_input");
        compact.source = Some("compact".into());
        assert_eq!(determine_phase(&compact), None);
        assert_eq!(determine_phase(&event("SessionEnd", "ended")), None);
        assert_eq!(
            determine_phase(&event("Stop", "waiting_for_input")),
            Some(Phase::WaitingForInput)
        );
        assert_eq!(
            determine_phase(&event("PreToolUse", "running_tool")),
            Some(Phase::Processing)
        );
        assert_eq!(determine_phase(&event("X", "waiting_for_approval")), None);
    }

    #[test]
    fn user_authored_prompts() {
        let mut vscode = event("UserPromptSubmit", "processing");
        vscode.entrypoint = Some("claude-vscode".into());
        vscode.source = Some("sdk".into());
        vscode.prompt = Some("fix the build".into());
        assert!(is_user_authored_prompt(&vscode));
        vscode.prompt = Some("<task-notification>x</task-notification>".into());
        assert!(!is_user_authored_prompt(&vscode));
        let mut sdk = event("UserPromptSubmit", "processing");
        sdk.entrypoint = Some("sdk-ts".into());
        sdk.source = Some("sdk".into());
        sdk.prompt = Some("hi".into());
        assert!(!is_user_authored_prompt(&sdk));
        let mut system = event("UserPromptSubmit", "processing");
        system.source = Some("system".into());
        assert!(!is_user_authored_prompt(&system));
        let plain = event("UserPromptSubmit", "processing");
        assert!(is_user_authored_prompt(&plain));
    }

    #[test]
    fn agent_view_announcements() {
        let mut e = event("Notification", "notification");
        e.notification_type = Some("agent_needs_input".into());
        e.message = Some("worker-3 needs your input: pick a db".into());
        assert!(is_agent_view_announcement(&e));
        e.message = Some("worker-3 needs your input".into());
        assert!(is_agent_view_announcement(&e));
        e.message = Some("Choose how to set up teammates".into());
        assert!(!is_agent_view_announcement(&e));
        e.message = Some(" needs your input".into());
        assert!(!is_agent_view_announcement(&e));
        e.notification_type = Some("agent_completed".into());
        assert!(is_agent_view_announcement(&e));
    }
}
