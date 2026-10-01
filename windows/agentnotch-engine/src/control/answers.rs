//! Answers to held PermissionRequests (HS§6): what the panel's buttons
//! become on the wire. The hook turns the response into Claude Code's
//! `hookSpecificOutput.decision` (`agentnotch_proto::permission_output`).
//!
//! An answer always names the request the user saw (its `tool_use_id`); the
//! hub looks that request up and passes it here, so a click that lands after
//! the request was replaced answers nothing.

use crate::model::{Answer, PendingRequest, RequestKind};
use agentnotch_proto::{PermissionResponse, KEEP_PLANNING_REASON};
use serde_json::{Map, Value};

/// AskUserQuestion has no "allow": it needs the answers.
const QUESTION_NEEDS_ANSWERS: &str = "A question is answered with its options, not approved.";
const NOT_A_QUESTION: &str = "That request isn't a question.";
const NOT_A_PLAN: &str = "That request isn't a plan.";
const NO_ANSWERS: &str = "No answer was chosen.";

/// The Mac keys every rule on the tool name alone (`ClaudeSessionMonitor`):
/// only AskUserQuestion takes answers.
const QUESTION_TOOL: &str = "AskUserQuestion";
/// Tools with `requiresUserInteraction` besides AskUserQuestion: Claude Code
/// takes their decision only with `updatedInput`, and an empty update makes
/// the hook echo the original input back.
const INPUT_ECHO_TOOLS: [&str; 1] = ["ExitPlanMode"];

/// Whether answers may be sent: by tool name only, so a request whose kind
/// disagrees with its tool never gets answers merged into its input.
fn takes_answers(req: &PendingRequest) -> bool {
    req.tool_name == QUESTION_TOOL
}

/// Whether a plain allow must be refused. Wider than [`takes_answers`] on
/// purpose: a request shown as a question is never allowed without its
/// answers, whatever its tool says.
fn refuses_plain_allow(req: &PendingRequest) -> bool {
    takes_answers(req) || req.kind == RequestKind::Question
}

/// Whether an allow echoes the original input (and plan answers apply): by
/// tool name only, so no other tool's input is ever echoed back.
fn needs_input_echo(req: &PendingRequest) -> bool {
    INPUT_ECHO_TOOLS.contains(&req.tool_name.as_str())
}

/// The response frame for answering `req` with `a`, or why that answer
/// doesn't fit the request (nothing is sent then; the request stays held).
///
/// - Allow: a plain allow; ExitPlanMode gets the input echo. "Always" also
///   applies Claude Code's first permission suggestion, verbatim (the
///   terminal's "Yes, and don't ask again"); with no suggestion it is a plain
///   allow.
/// - Deny: the reason, or none (the hook then says "Denied by user via
///   Agent Notch").
/// - Questions: allow with `{"answers": {question text: answer}}`, which the
///   hook merges onto the original input.
/// - Approve plan: allow with the input echo. Keep planning: deny with the
///   reason Claude reads back.
pub fn permission_response(req: &PendingRequest, a: &Answer) -> Result<PermissionResponse, String> {
    match a {
        Answer::Allow { always } => {
            if refuses_plain_allow(req) {
                return Err(QUESTION_NEEDS_ANSWERS.into());
            }
            let mut response = PermissionResponse::allow();
            if needs_input_echo(req) {
                response.updated_input = Some(Map::new());
            }
            if *always {
                response.updated_permissions = req
                    .always
                    .as_ref()
                    .map(|rule| vec![rule.suggestion.clone()]);
            }
            Ok(response)
        }
        Answer::Deny { reason } => Ok(PermissionResponse::deny(
            reason.clone().filter(|reason| !reason.trim().is_empty()),
        )),
        Answer::Questions { answers } => {
            if !takes_answers(req) {
                return Err(NOT_A_QUESTION.into());
            }
            if answers.is_empty() {
                return Err(NO_ANSWERS.into());
            }
            let chosen: Map<String, Value> = answers
                .iter()
                .map(|(question, answer)| (question.clone(), Value::String(answer.clone())))
                .collect();
            let mut update = Map::new();
            update.insert("answers".into(), Value::Object(chosen));
            let mut response = PermissionResponse::allow();
            response.updated_input = Some(update);
            Ok(response)
        }
        Answer::ApprovePlan => {
            if !needs_input_echo(req) {
                return Err(NOT_A_PLAN.into());
            }
            let mut response = PermissionResponse::allow();
            response.updated_input = Some(Map::new());
            Ok(response)
        }
        Answer::KeepPlanning => {
            if !needs_input_echo(req) {
                return Err(NOT_A_PLAN.into());
            }
            Ok(PermissionResponse::deny(Some(KEEP_PLANNING_REASON.into())))
        }
    }
}
