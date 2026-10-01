//! The app's answer to a PermissionRequest and what the hook prints for it
//! (HS§1.7): Claude Code reads `hookSpecificOutput.decision`, where
//! `behavior == "allow"` allows (taking `updatedInput`) and anything else
//! denies. "ask", no answer, or anything unreadable prints nothing, and
//! Claude Code's own terminal dialog, shown in parallel, decides.

use crate::pyjson::{self, Ordered};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The message a deny carries when the user gave no reason.
pub const DEFAULT_DENY_MESSAGE: &str = "Denied by user via Agent Notch";
/// The deny reason of "Keep planning" on an ExitPlanMode request.
pub const KEEP_PLANNING_REASON: &str =
    "The user reviewed the plan and wants to keep planning. Stay in plan mode and ask what to change before implementing.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

/// The response frame's JSON (the Mac's `PermissionResponse`, HS§4.4), with
/// absent options omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionResponse {
    pub decision: Decision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Only the fields to change; merged onto the ORIGINAL stdin input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_permissions: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
}

impl PermissionResponse {
    pub fn allow() -> Self {
        PermissionResponse {
            decision: Decision::Allow,
            reason: None,
            updated_input: None,
            updated_permissions: None,
            interrupt: None,
        }
    }

    pub fn deny(reason: Option<String>) -> Self {
        PermissionResponse {
            decision: Decision::Deny,
            reason,
            ..PermissionResponse::allow()
        }
    }

    /// "Always allow": allow, and hand Claude Code back one of the request's
    /// own `permission_suggestions`, verbatim.
    pub fn always_allow(suggestion: Value) -> Self {
        PermissionResponse {
            updated_permissions: Some(vec![suggestion]),
            ..PermissionResponse::allow()
        }
    }

    /// An AskUserQuestion answered: allow with `{"answers": {question text,
    /// exactly as it was asked: the chosen label or labels}}`, which the
    /// hook merges onto the original input.
    pub fn answers<Q, A>(answers: impl IntoIterator<Item = (Q, A)>) -> Self
    where
        Q: Into<String>,
        A: Into<String>,
    {
        let answers: Map<String, Value> = answers
            .into_iter()
            .map(|(question, answer)| (question.into(), Value::String(answer.into())))
            .collect();
        let mut input = Map::new();
        input.insert("answers".into(), Value::Object(answers));
        PermissionResponse {
            updated_input: Some(input),
            ..PermissionResponse::allow()
        }
    }

    /// An ExitPlanMode approved. A plain allow is ignored for tools that need
    /// the user, so the original input is echoed: an empty update.
    pub fn approve_plan() -> Self {
        PermissionResponse {
            updated_input: Some(Map::new()),
            ..PermissionResponse::allow()
        }
    }

    /// "Keep planning" on an ExitPlanMode: a deny that tells Claude why.
    pub fn keep_planning() -> Self {
        PermissionResponse::deny(Some(KEEP_PLANNING_REASON.into()))
    }

    /// No decision: the hook prints nothing and Claude Code's own prompt
    /// decides. Closing the connection without a frame says the same.
    pub fn ask() -> Self {
        PermissionResponse {
            decision: Decision::Ask,
            ..PermissionResponse::allow()
        }
    }

    /// The frame's bytes.
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Reads a response frame the way the Mac hook script does: it must be
    /// a JSON object; a field of the wrong type is ignored rather than
    /// failing the whole answer, and an unknown `decision` is "ask" (print
    /// nothing). `None` for anything that isn't an object.
    pub fn from_frame(bytes: &[u8]) -> Option<PermissionResponse> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let object = value.as_object()?;
        let decision = match object.get("decision").and_then(Value::as_str) {
            Some("allow") => Decision::Allow,
            Some("deny") => Decision::Deny,
            _ => Decision::Ask,
        };
        Some(PermissionResponse {
            decision,
            reason: object
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned),
            updated_input: object
                .get("updated_input")
                .and_then(Value::as_object)
                .cloned(),
            updated_permissions: object
                .get("updated_permissions")
                .and_then(Value::as_array)
                .cloned(),
            interrupt: object.get("interrupt").and_then(Value::as_bool),
        })
    }
}

/// The exact stdout text (without a trailing newline) for `r`, or `None` to
/// print nothing. The original input's keys come out sorted; the hook exe
/// uses [`permission_output_for_frames`] to keep Claude Code's own order.
pub fn permission_output(original_tool_input: &Value, r: &PermissionResponse) -> Option<String> {
    output(
        &Ordered::from_value(original_tool_input),
        &Answer::from_response(r),
    )
}

/// [`permission_output`] with the original `tool_input` taken from Claude
/// Code's stdin bytes in document order.
pub fn permission_output_for_stdin(stdin: &[u8], r: &PermissionResponse) -> Option<String> {
    output(&original_input(stdin), &Answer::from_response(r))
}

/// What the hook prints, from the two byte buffers it holds: Claude Code's
/// stdin and the app's response frame. Both keep their key order, so the
/// text is byte for byte what the Mac hook script prints (`json.dumps`) for
/// the same bytes. A response that isn't a JSON object prints nothing.
pub fn permission_output_for_frames(stdin: &[u8], response_frame: &[u8]) -> Option<String> {
    let response = Ordered::from_slice(response_frame)?;
    output(&original_input(stdin), &Answer::from_ordered(&response)?)
}

fn original_input(stdin: &[u8]) -> Ordered {
    Ordered::from_slice(stdin)
        .and_then(|data| data.get("tool_input").cloned())
        .unwrap_or(Ordered::Null)
}

/// A response with its parts in the order they arrived.
struct Answer {
    decision: Decision,
    reason: Option<String>,
    updated_input: Option<Vec<(String, Ordered)>>,
    updated_permissions: Option<Vec<Ordered>>,
    interrupt: Option<bool>,
}

impl Answer {
    fn from_response(r: &PermissionResponse) -> Answer {
        let object = |map: &Map<String, Value>| {
            map.iter()
                .map(|(k, v)| (k.clone(), Ordered::from_value(v)))
                .collect()
        };
        Answer {
            decision: r.decision,
            reason: r.reason.clone(),
            updated_input: r.updated_input.as_ref().map(object),
            updated_permissions: r
                .updated_permissions
                .as_ref()
                .map(|list| list.iter().map(Ordered::from_value).collect()),
            interrupt: r.interrupt,
        }
    }

    /// The Mac script's `isinstance` checks: a field of the wrong type is
    /// ignored; an unknown decision prints nothing.
    fn from_ordered(response: &Ordered) -> Option<Answer> {
        let Ordered::Object(_) = response else {
            return None;
        };
        let decision = match response.get("decision") {
            Some(Ordered::String(s)) if s == "allow" => Decision::Allow,
            Some(Ordered::String(s)) if s == "deny" => Decision::Deny,
            _ => Decision::Ask,
        };
        Some(Answer {
            decision,
            reason: match response.get("reason") {
                Some(Ordered::String(s)) => Some(s.clone()),
                _ => None,
            },
            updated_input: match response.get("updated_input") {
                Some(Ordered::Object(entries)) => Some(entries.clone()),
                _ => None,
            },
            updated_permissions: match response.get("updated_permissions") {
                Some(Ordered::Array(items)) => Some(items.clone()),
                _ => None,
            },
            interrupt: match response.get("interrupt") {
                Some(Ordered::Bool(b)) => Some(*b),
                _ => None,
            },
        })
    }
}

fn output(original: &Ordered, r: &Answer) -> Option<String> {
    let mut result: Vec<(String, Ordered)> = Vec::new();
    match r.decision {
        Decision::Allow => {
            result.push(("behavior".into(), Ordered::String("allow".into())));
            if let Some(updates) = &r.updated_input {
                // Onto the ORIGINAL input: an empty update echoes it, which
                // tools needing user interaction (ExitPlanMode) require.
                let mut merged = match original {
                    Ordered::Object(entries) => entries.clone(),
                    _ => Vec::new(),
                };
                for (key, value) in updates {
                    Ordered::insert(&mut merged, key.clone(), value.clone());
                }
                result.push(("updatedInput".into(), Ordered::Object(merged)));
            }
            if let Some(permissions) = r
                .updated_permissions
                .as_ref()
                .filter(|list| !list.is_empty())
            {
                result.push((
                    "updatedPermissions".into(),
                    Ordered::Array(permissions.clone()),
                ));
            }
        }
        Decision::Deny => {
            let message = r
                .reason
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .unwrap_or(DEFAULT_DENY_MESSAGE);
            result.push(("behavior".into(), Ordered::String("deny".into())));
            result.push(("message".into(), Ordered::String(message.into())));
            if let Some(interrupt) = r.interrupt {
                result.push(("interrupt".into(), Ordered::Bool(interrupt)));
            }
        }
        Decision::Ask => return None,
    }
    let output = Ordered::Object(vec![(
        "hookSpecificOutput".into(),
        Ordered::Object(vec![
            (
                "hookEventName".into(),
                Ordered::String("PermissionRequest".into()),
            ),
            ("decision".into(), Ordered::Object(result)),
        ]),
    )]);
    Some(pyjson::dumps(&output))
}
