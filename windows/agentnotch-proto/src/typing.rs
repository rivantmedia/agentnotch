//! The two-phase typing protocol between the engine and
//! `agentnotch-hook.exe type` (DESIGN-WIN §4.8). Stdin line 1 is a
//! [`TypeRequest`]; the helper types the text, prints `{"phase":"typed"}`
//! and waits up to 2 s for line 2, [`TYPE_SUBMIT`] or [`TYPE_ABORT`]; it
//! then prints the final `{"outcome":…,"reason"?:…}` line and exits 0.
//! Return is pressed only after the engine re-checked the session on fresh
//! state, so a prompt that appeared in the gap is never confirmed by it.

use serde::{Deserialize, Serialize};

/// Line 2: press Return.
pub const TYPE_SUBMIT: &str = "submit";
/// Line 2: leave the text typed, without Return.
pub const TYPE_ABORT: &str = "abort";

/// Stdin line 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeRequest {
    pub text: String,
}

/// One stdout line of the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypePhase {
    /// The text is in the console's input buffer; waiting for line 2.
    Typed,
    /// Final: `delivered`, `refused`, `typed_not_submitted` or `failed`.
    Outcome {
        outcome: String,
        reason: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Wire {
    Typed {
        phase: String,
    },
    Outcome {
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl TypePhase {
    pub const DELIVERED: &'static str = "delivered";
    pub const REFUSED: &'static str = "refused";
    pub const TYPED_NOT_SUBMITTED: &'static str = "typed_not_submitted";
    pub const FAILED: &'static str = "failed";

    /// The line (without its newline).
    pub fn to_line(&self) -> String {
        let wire = match self {
            TypePhase::Typed => Wire::Typed {
                phase: "typed".into(),
            },
            TypePhase::Outcome { outcome, reason } => Wire::Outcome {
                outcome: outcome.clone(),
                reason: reason.clone(),
            },
        };
        serde_json::to_string(&wire).unwrap_or_default()
    }

    /// Parses one line; `None` for anything else.
    pub fn from_line(line: &str) -> Option<TypePhase> {
        match serde_json::from_str::<Wire>(line.trim()).ok()? {
            Wire::Typed { phase } if phase == "typed" => Some(TypePhase::Typed),
            Wire::Typed { .. } => None,
            Wire::Outcome { outcome, reason } => Some(TypePhase::Outcome { outcome, reason }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        assert_eq!(TypePhase::Typed.to_line(), r#"{"phase":"typed"}"#);
        let refused = TypePhase::Outcome {
            outcome: TypePhase::REFUSED.into(),
            reason: Some("Another program is reading this console".into()),
        };
        assert_eq!(
            refused.to_line(),
            r#"{"outcome":"refused","reason":"Another program is reading this console"}"#
        );
        let delivered = TypePhase::Outcome {
            outcome: TypePhase::DELIVERED.into(),
            reason: None,
        };
        assert_eq!(delivered.to_line(), r#"{"outcome":"delivered"}"#);
        for phase in [TypePhase::Typed, refused, delivered] {
            assert_eq!(TypePhase::from_line(&phase.to_line()), Some(phase));
        }
        assert_eq!(TypePhase::from_line(r#"{"phase":"other"}"#), None);
        assert_eq!(TypePhase::from_line("garbage"), None);
    }
}
