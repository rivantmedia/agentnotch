//! Held PermissionRequests as the UI sees them, and the answers it can give
//! (HS§6).

use crate::model::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Permission,
    /// AskUserQuestion.
    Question,
    /// ExitPlanMode.
    Plan,
}

/// One held PermissionRequest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRequest {
    pub session_id: SessionId,
    pub tool_use_id: String,
    pub kind: RequestKind,
    pub tool_name: String,
    pub received_at: SystemTime,
    pub activated_at: Option<SystemTime>,
    /// The short request text (PermissionPreview, UI§5.4).
    pub input_preview: String,
    pub input: Value,
    /// "Always allow": the first suggestion, when there is one.
    pub always: Option<AlwaysRule>,
    /// More than 4 lines or 200 characters: review it whole first.
    pub needs_review: bool,
    pub questions: Option<Vec<Question>>,
    pub plan_markdown: Option<String>,
    pub agent_id: Option<String>,
}

/// An "Always allow" suggestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlwaysRule {
    /// "Don't ask again for Bash(npm run test:*) in this project (just you)".
    pub description: String,
    /// Sent back verbatim as `updated_permissions[0]`.
    pub suggestion: Value,
    /// A narrow rule (addRules/replaceRules to the session or localSettings):
    /// offered in the list, not only in the chat.
    pub inline: bool,
}

/// One AskUserQuestion question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// The answer key is this text, exactly as sent.
    pub text: String,
    pub header: Option<String>,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: Option<String>,
}

/// What the UI can answer. JSON: `{"allow":{"always":false}}`,
/// `{"deny":{"reason":null}}`, `{"questions":{"answers":{…}}}`,
/// `"approve_plan"`, `"keep_planning"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Allow {
        always: bool,
    },
    Deny {
        reason: Option<String>,
    },
    /// Question text → label, or labels joined ", " (multi-select), or the
    /// "Other" text.
    Questions {
        answers: BTreeMap<String, String>,
    },
    ApprovePlan,
    KeepPlanning,
}
