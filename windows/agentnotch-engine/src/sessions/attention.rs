//! What a session needs from the user right now (SessionAttention.swift):
//! input (a permission, a question, an error), a review of finished work,
//! nothing because Claude is working, or nothing at all. Derived purely from
//! the session's fields so the store, the tracker and the tests share one
//! definition.

use crate::model::{NeedsInputReason, Phase, SessionState};
use std::time::SystemTime;

/// The five states, in priority order (HS§5.3):
/// 1. a pending approval (question / plan / permission),
/// 2. an explicit needs-input reason (notification, registry, StopFailure;
///    a failed turn is `Failed`),
/// 3. processing or compacting, or a Stop not yet confirmed as the end of
///    the turn (`completion_pending`: Claude Code still runs its Stop hooks,
///    and a blocking one such as /goal continues the turn), or a turn that
///    ended waiting on background agents or workflows that will wake Claude
///    when they finish (`awaiting_background`),
/// 4. a completed turn not reviewed since it completed,
/// 5. idle.
pub fn derive(
    phase: &Phase,
    needs_input: Option<&NeedsInputReason>,
    completed_at: Option<SystemTime>,
    reviewed_at: Option<SystemTime>,
    completion_pending: bool,
    awaiting_background: bool,
) -> SessionState {
    if let Phase::WaitingForApproval(context) = phase {
        return SessionState::NeedsYou(NeedsInputReason::for_approval(&context.tool_name));
    }
    if let Some(reason) = needs_input {
        return if reason.is_error() {
            SessionState::Failed(reason.clone())
        } else {
            SessionState::NeedsYou(reason.clone())
        };
    }
    if matches!(phase, Phase::Processing | Phase::Compacting) {
        return SessionState::Working;
    }
    if completion_pending || awaiting_background {
        return SessionState::Working;
    }
    // A turn that ended with only shells or monitors still running (a dev
    // server, a watcher) is finished work to look at: treating it as
    // "working" would hide it from the review queue for as long as the task
    // lives. The rows show the running count as a detail instead.
    let turn_ended = matches!(phase, Phase::WaitingForInput | Phase::Idle);
    if turn_ended {
        if let Some(completed_at) = completed_at {
            if reviewed_at.is_none_or(|reviewed| reviewed < completed_at) {
                return SessionState::ReadyForReview;
            }
        }
    }
    SessionState::Idle
}

/// Why a turn failed (StopFailure `error`), grouped by what the user can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopErrorKind {
    /// The account hit a usage limit; the session can go on after the reset.
    RateLimit,
    /// Anthropic's side was busy or failed; retrying may work.
    Overloaded,
    ServerError,
    /// The account needs /login, or can't be used (org not allowed, on hold).
    Authentication,
    Billing,
    /// The request itself was refused (invalid, unknown model).
    InvalidRequest,
    MaxOutputTokens,
    Other,
}

impl StopErrorKind {
    pub const ALL: [StopErrorKind; 8] = [
        StopErrorKind::RateLimit,
        StopErrorKind::Overloaded,
        StopErrorKind::ServerError,
        StopErrorKind::Authentication,
        StopErrorKind::Billing,
        StopErrorKind::InvalidRequest,
        StopErrorKind::MaxOutputTokens,
        StopErrorKind::Other,
    ];

    /// `None` for a missing or empty code; an unknown code is `Other`.
    pub fn from_code(code: Option<&str>) -> Option<StopErrorKind> {
        let code = code?.to_lowercase();
        if code.is_empty() {
            return None;
        }
        Some(match code.as_str() {
            "rate_limit" => StopErrorKind::RateLimit,
            "overloaded" => StopErrorKind::Overloaded,
            "server_error" => StopErrorKind::ServerError,
            "authentication_failed"
            | "oauth_org_not_allowed"
            | "account_on_hold"
            | "cloud_credential_error" => StopErrorKind::Authentication,
            "billing_error" => StopErrorKind::Billing,
            "invalid_request" | "model_not_found" => StopErrorKind::InvalidRequest,
            "max_output_tokens" => StopErrorKind::MaxOutputTokens,
            _ => StopErrorKind::Other,
        })
    }

    pub fn display_text(self) -> &'static str {
        match self {
            StopErrorKind::RateLimit => "Rate limited",
            StopErrorKind::Overloaded => "Overloaded",
            StopErrorKind::ServerError => "Server error",
            StopErrorKind::Authentication => "Sign-in failed",
            StopErrorKind::Billing => "Billing problem",
            StopErrorKind::InvalidRequest => "Invalid request",
            StopErrorKind::MaxOutputTokens => "Output limit reached",
            StopErrorKind::Other => "Turn failed",
        }
    }

    /// Goes away by itself (a limit resets, a busy API recovers): retrying
    /// later works without the user fixing anything.
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            StopErrorKind::RateLimit | StopErrorKind::Overloaded | StopErrorKind::ServerError
        )
    }
}

/// Human text for a StopFailure `error` code: the kind's text, else the
/// code itself made readable ("some_new_error" → "Some new error"), else
/// "Turn failed".
pub fn humanized_stop_error(code: Option<&str>) -> String {
    if let Some(kind) = StopErrorKind::from_code(code) {
        if kind != StopErrorKind::Other {
            return kind.display_text().to_owned();
        }
    }
    match code.map(str::to_lowercase) {
        Some(other) if !other.is_empty() && other != "unknown" => {
            let words = other.replace('_', " ");
            let mut chars = words.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => words,
            }
        }
        _ => "Turn failed".to_owned(),
    }
}

/// The needs-input reason of a failed turn.
pub fn failure_reason(code: Option<&str>) -> NeedsInputReason {
    NeedsInputReason::Error {
        text: humanized_stop_error(code),
        code: code.filter(|c| !c.is_empty()).map(str::to_owned),
    }
}

/// Sort key inside "Needs you": answerable reasons first, errors last.
pub fn sort_rank(reason: &NeedsInputReason) -> u8 {
    u8::from(reason.is_error())
}

/// "Claude needs your permission to use Bash" → "Bash".
pub fn tool_name_from_permission_prompt(message: Option<&str>) -> Option<String> {
    let message = message?;
    let start = message.find("permission to use ")? + "permission to use ".len();
    let name: String = message[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ',' && *c != '.')
        .collect();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PermissionContext;
    use std::time::{Duration, UNIX_EPOCH};

    fn t0() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn approval(tool: &str) -> Phase {
        Phase::WaitingForApproval(PermissionContext {
            tool_use_id: "toolu_1".into(),
            tool_name: tool.into(),
            tool_input: serde_json::Value::Null,
            received_at: t0(),
            permission_suggestions: vec![],
            has_synthetic_tool_use_id: false,
            agent_id: None,
            activated_at: None,
        })
    }

    fn state(
        phase: Phase,
        reason: Option<NeedsInputReason>,
        completed: Option<SystemTime>,
        reviewed: Option<SystemTime>,
    ) -> SessionState {
        derive(&phase, reason.as_ref(), completed, reviewed, false, false)
    }

    fn permission(tool: &str) -> NeedsInputReason {
        NeedsInputReason::Permission {
            tool: Some(tool.into()),
        }
    }

    fn error(text: &str) -> NeedsInputReason {
        NeedsInputReason::Error {
            text: text.into(),
            code: None,
        }
    }

    // SessionAttentionTests
    #[test]
    fn approvals_map_to_their_reason() {
        assert_eq!(
            state(approval("Bash"), None, None, None),
            SessionState::NeedsYou(permission("Bash"))
        );
        assert_eq!(
            state(approval("AskUserQuestion"), None, None, None),
            SessionState::NeedsYou(NeedsInputReason::Question)
        );
        assert_eq!(
            state(approval("ExitPlanMode"), None, None, None),
            SessionState::NeedsYou(NeedsInputReason::PlanApproval)
        );
    }

    #[test]
    fn approval_wins_over_everything_else() {
        assert_eq!(
            state(
                approval("Edit"),
                Some(error("Rate limited")),
                Some(t0()),
                None
            ),
            SessionState::NeedsYou(permission("Edit"))
        );
    }

    #[test]
    fn explicit_reason_wins_over_working_and_review() {
        let dialog = NeedsInputReason::Dialog {
            detail: "input needed".into(),
        };
        assert_eq!(
            state(Phase::Processing, Some(dialog.clone()), None, None),
            SessionState::NeedsYou(dialog)
        );
        assert_eq!(
            state(
                Phase::WaitingForInput,
                Some(error("Rate limited")),
                Some(t0()),
                None
            ),
            SessionState::Failed(error("Rate limited"))
        );
        let elicitation = NeedsInputReason::Elicitation {
            message: "Pick a repo".into(),
        };
        assert_eq!(
            state(Phase::Idle, Some(elicitation.clone()), None, None),
            SessionState::NeedsYou(elicitation)
        );
    }

    #[test]
    fn processing_and_compacting_are_working() {
        assert_eq!(
            state(Phase::Processing, None, None, None),
            SessionState::Working
        );
        assert_eq!(
            state(Phase::Compacting, None, Some(t0()), None),
            SessionState::Working
        );
    }

    #[test]
    fn pending_completions_and_background_waits_are_working() {
        assert_eq!(
            derive(&Phase::WaitingForInput, None, Some(t0()), None, true, false),
            SessionState::Working
        );
        assert_eq!(
            derive(&Phase::Idle, None, Some(t0()), None, false, true),
            SessionState::Working
        );
    }

    #[test]
    fn completed_and_unreviewed_is_ready_for_review() {
        assert_eq!(
            state(Phase::WaitingForInput, None, Some(t0()), None),
            SessionState::ReadyForReview
        );
        assert_eq!(
            state(Phase::Idle, None, Some(t0()), None),
            SessionState::ReadyForReview
        );
        assert_eq!(
            state(
                Phase::WaitingForInput,
                None,
                Some(t0()),
                Some(t0() - Duration::from_secs(60))
            ),
            SessionState::ReadyForReview
        );
    }

    #[test]
    fn reviewed_after_completion_is_idle() {
        assert_eq!(
            state(
                Phase::WaitingForInput,
                None,
                Some(t0()),
                Some(t0() + Duration::from_secs(1))
            ),
            SessionState::Idle
        );
        assert_eq!(
            state(Phase::WaitingForInput, None, Some(t0()), Some(t0())),
            SessionState::Idle
        );
        assert_eq!(
            state(Phase::WaitingForInput, None, None, None),
            SessionState::Idle
        );
        assert_eq!(state(Phase::Idle, None, None, None), SessionState::Idle);
    }

    #[test]
    fn buckets_sort_needs_input_first() {
        let mut states = [
            SessionState::Idle,
            SessionState::Working,
            SessionState::ReadyForReview,
            SessionState::NeedsYou(NeedsInputReason::Question),
        ];
        states.sort_by_key(|s| s.bucket());
        assert_eq!(
            states,
            [
                SessionState::NeedsYou(NeedsInputReason::Question),
                SessionState::ReadyForReview,
                SessionState::Working,
                SessionState::Idle
            ]
        );
    }

    #[test]
    fn stop_errors_are_humanized() {
        assert_eq!(humanized_stop_error(Some("rate_limit")), "Rate limited");
        assert_eq!(humanized_stop_error(Some("overloaded")), "Overloaded");
        assert_eq!(
            humanized_stop_error(Some("some_new_error")),
            "Some new error"
        );
        assert_eq!(humanized_stop_error(Some("unknown")), "Turn failed");
        assert_eq!(humanized_stop_error(None), "Turn failed");
        assert_eq!(
            humanized_stop_error(Some("oauth_org_not_allowed")),
            "Sign-in failed"
        );
        assert_eq!(
            StopErrorKind::from_code(Some("billing_error")),
            Some(StopErrorKind::Billing)
        );
        assert_eq!(StopErrorKind::from_code(Some("")), None);
        assert!(StopErrorKind::RateLimit.is_transient());
        assert!(!StopErrorKind::Billing.is_transient());
    }

    #[test]
    fn permission_prompt_tool_name_is_extracted() {
        assert_eq!(
            tool_name_from_permission_prompt(Some("Claude needs your permission to use Bash")),
            Some("Bash".into())
        );
        assert_eq!(
            tool_name_from_permission_prompt(Some(
                "Claude needs your permission to use mcp__github__create_issue."
            )),
            Some("mcp__github__create_issue".into())
        );
        assert_eq!(
            tool_name_from_permission_prompt(Some("Waiting for input")),
            None
        );
    }
}
