//! Frame → message, with the Mac's lenient rules (HookEvent.swift `init(from:)`,
//! `StatusLineMessage(json:)`, `HookSocketMessage.decode`; HS§4.2).
//!
//! Hook copies in run folders can be older or newer than the app, and Claude
//! Code's own values arrive loosely typed, so nothing but a missing
//! `session_id`/`event` makes a hook event unreadable: unknown keys are
//! ignored, a number sent as a string (or the other way round) is coerced,
//! and a value of the wrong shape is simply absent. The rules are the Swift
//! decoder's, field for field, so the engine's session rules see exactly what
//! the Mac's see.

use crate::model::{HookEvent, HookTerminal, SessionId, StatusLineMessage};
use agentnotch_proto::{ControlOp, CONTROL_EVENT, STATUS_LINE_EVENT};
use serde_json::{Map, Value};
use std::time::SystemTime;

/// What one frame holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    Hook(Box<HookEvent>),
    StatusLine(Box<StatusLineMessage>),
    Control(ControlOp),
    /// Not JSON, not an object, or a hook event without `session_id` or
    /// `event` (the reason, for the log; never shown).
    Unreadable(&'static str),
}

/// Decodes one frame's bytes; `received_at` is when the server read it.
pub fn decode(bytes: &[u8], received_at: SystemTime) -> Decoded {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Decoded::Unreadable("not JSON");
    };
    let Some(object) = value.as_object() else {
        return Decoded::Unreadable("not a JSON object");
    };
    match object.get("event").and_then(Value::as_str) {
        Some(STATUS_LINE_EVENT) => status_line(object, received_at)
            .map(|m| Decoded::StatusLine(Box::new(m)))
            .unwrap_or(Decoded::Unreadable("a status line without a session")),
        Some(CONTROL_EVENT) => match object.get("op").and_then(Value::as_str) {
            Some("status") => Decoded::Control(ControlOp::Status),
            Some("quit") => Decoded::Control(ControlOp::Quit),
            _ => Decoded::Unreadable("an unknown control operation"),
        },
        _ => hook_event(object, received_at)
            .map(|e| Decoded::Hook(Box::new(e)))
            .unwrap_or(Decoded::Unreadable(
                "a hook event without session_id or event",
            )),
    }
}

/// `HookEvent.init(from:)`: `session_id` and `event` must be strings; every
/// other field is lossy.
pub fn hook_event(object: &Map<String, Value>, received_at: SystemTime) -> Option<HookEvent> {
    let session_id = object.get("session_id")?.as_str()?;
    let event_name = object.get("event")?.as_str()?;
    let field = |key: &str| object.get(key);
    let string = |key: &str| field(key).and_then(lossy_string);
    let count = |key: &str| {
        field(key)
            .and_then(lossy_int)
            .and_then(|n| u32::try_from(n).ok())
    };

    let mut event = HookEvent::new(SessionId::new(session_id), event_name, received_at);
    event.cwd = string("cwd").unwrap_or_default();
    event.status = string("status").unwrap_or_else(|| "unknown".into());
    event.pid = field("pid").and_then(lossy_int).and_then(valid_pid);
    event.transcript_path = string("transcript_path");
    event.config_dir_env = string("config_dir_env");
    event.attended = field("attended").and_then(lossy_bool);
    event.entrypoint = string("entrypoint");
    event.agent_id = string("agent_id");
    event.agent_type = string("agent_type");
    event.permission_mode = string("permission_mode");

    event.tool = string("tool");
    event.tool_input = field("tool_input").and_then(Value::as_object).cloned();
    event.tool_use_id = string("tool_use_id");
    event.tool_error = string("tool_error");
    event.is_interrupt = field("is_interrupt").and_then(lossy_bool);
    event.permission_suggestions = field("permission_suggestions")
        .and_then(Value::as_array)
        .cloned();
    event.denial_reason = string("denial_reason");

    event.task_id = string("task_id");
    event.task_subject = string("task_subject");

    event.notification_type = string("notification_type");
    event.message = string("message");
    event.title = string("title");

    event.last_assistant_message = string("last_assistant_message");
    event.background_task_count = count("background_task_count");
    event.background_task_types = field("background_task_types")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        });
    event.session_cron_count = count("session_cron_count");
    event.stop_hook_active = field("stop_hook_active").and_then(lossy_bool);
    event.stop_error = string("stop_error");
    event.stop_error_details = string("stop_error_details");
    event.agent_transcript_path = string("agent_transcript_path");

    event.source = string("source");
    event.model = string("model");
    event.session_title = string("session_title");
    event.prompt = string("prompt");
    event.reason = string("reason");
    event.trigger = string("trigger");

    // Windows fields. A frame from a hook older than the field decodes to
    // the defaults; every version from 1 on is read the same way.
    event.protocol = field("protocol")
        .and_then(lossy_int)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p >= 1)
        .unwrap_or(agentnotch_proto::PROTOCOL);
    event.hook_pid = field("hook_pid").and_then(lossy_int).and_then(valid_pid);
    event.terminal = field("terminal")
        .and_then(Value::as_object)
        .map(|t| HookTerminal {
            wt_session: t.get("wt_session").and_then(lossy_string),
            term_program: t.get("term_program").and_then(lossy_string),
        });
    Some(event)
}

/// `StatusLineMessage(json:)`: a non-empty string `session_id` is required.
///
/// `rate_limits` stays raw: the usage parser (UsageParser's rules: booleans,
/// NaN and infinities are not percentages; `resets_at` in seconds,
/// milliseconds or ISO 8601) reads it, so the windows are left for it to
/// fill. `account_id` needs the home folder and the account classifier; see
/// [`super::fill_status_line_account`].
pub fn status_line(
    object: &Map<String, Value>,
    received_at: SystemTime,
) -> Option<StatusLineMessage> {
    let session_id = object
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let empty = Map::new();
    let status = object
        .get("status_line")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let context = status.get("context_window").and_then(Value::as_object);
    let model = status.get("model").and_then(Value::as_object);
    let cost = status.get("cost").and_then(Value::as_object);

    Some(StatusLineMessage {
        session_id: SessionId::new(session_id),
        cwd: json_string(object.get("cwd")),
        transcript_path: json_string(object.get("transcript_path")),
        config_dir_env: json_string(object.get("config_dir_env")),
        account_id: None,
        received_at,
        rate_limits: status.get("rate_limits").filter(|v| !v.is_null()).cloned(),
        five_hour: None,
        seven_day: None,
        context_used_percent: json_double(within(context, "used_percentage")),
        context_window_size: json_int(within(context, "context_window_size"))
            .and_then(|n| u64::try_from(n).ok()),
        model_id: json_string(within(model, "id")),
        model_display_name: json_string(within(model, "display_name")),
        cost_usd: json_double(within(cost, "total_cost_usd")),
        session_name: json_string(status.get("session_name")),
        claude_code_version: json_string(status.get("version")),
        pid: json_int(object.get("pid")).and_then(valid_pid),
    })
}

/// A key of a nested object that may itself be missing or of another shape.
fn within<'a>(map: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    map.and_then(|m| m.get(key))
}

/// `ProcessID.valid`: 1..=Int32.max; anything else is no pid.
fn valid_pid(pid: i64) -> Option<u32> {
    (1..=i32::MAX as i64).contains(&pid).then_some(pid as u32)
}

// ---- the Swift decoder's lossy coercions (HookEvent.swift, private extension) ----

/// `lossyString`: a string as it is (empty included), else an integer or a
/// double as text; `null`, booleans, arrays and objects are absent.
fn lossy_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(_) => {
            if let Some(int) = integral(value) {
                Some(int.to_string())
            } else {
                // Swift writes the everyday values (1.5, 0.25) the same way;
                // only exotic ones (1e+20) differ, which no hook sends.
                value
                    .as_f64()
                    .filter(|d| d.is_finite())
                    .map(|d| d.to_string())
            }
        }
        _ => None,
    }
}

/// `lossyInt`: an integer (an integral double counts, as Swift's decoder
/// reads `5.0` as 5), else a string Swift's `Int(_:)` parses.
fn lossy_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(_) => integral(value),
        Value::String(s) => swift_int(s),
        _ => None,
    }
}

/// `lossyBool`: a boolean, else "1"/"true"/"yes" and "0"/"false"/"no" in any
/// case, else an integer (non-zero is true).
fn lossy_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.to_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        },
        Value::Number(_) => integral(value).map(|n| n != 0),
        _ => None,
    }
}

/// A JSON number that is a whole number within `i64`.
fn integral(value: &Value) -> Option<i64> {
    if let Some(int) = value.as_i64() {
        return Some(int);
    }
    let double = value.as_f64()?;
    // Strictly below 2^63: `i64::MAX as f64` rounds up to it.
    (double.fract() == 0.0 && double >= i64::MIN as f64 && double < i64::MAX as f64)
        .then_some(double as i64)
}

/// Swift's `Int(String)`: an optional sign and ASCII digits, nothing else.
fn swift_int(text: &str) -> Option<i64> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

// ---- JSONValue (the status line's coercions) ----

/// `JSONValue.string`: a non-empty string, or a number as text (a JSON
/// boolean is an `NSNumber` there too, so it reads as "1" or "0").
fn json_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(match integral(&Value::Number(n.clone())) {
            Some(int) => int.to_string(),
            None => n.as_f64()?.to_string(),
        }),
        Value::Bool(b) => Some(if *b { "1".into() } else { "0".into() }),
        _ => None,
    }
}

/// `UsageParser.number`: a finite number (never a boolean), or a string
/// holding one (surrounding spaces allowed).
fn json_double(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|d| d.is_finite()),
        Value::String(s) => s
            .trim_matches(|c: char| c == ' ' || c == '\t')
            .parse::<f64>()
            .ok()
            .filter(|d| d.is_finite()),
        _ => None,
    }
}

/// `JSONValue.int`: `json_double` truncated toward zero, when it fits.
fn json_int(value: Option<&Value>) -> Option<i64> {
    let double = json_double(value)?;
    (double >= i64::MIN as f64 && double < i64::MAX as f64).then_some(double.trunc() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lossy_strings() {
        assert_eq!(lossy_string(&json!("x")), Some("x".into()));
        assert_eq!(lossy_string(&json!("")), Some(String::new()));
        assert_eq!(lossy_string(&json!(3)), Some("3".into()));
        assert_eq!(lossy_string(&json!(3.0)), Some("3".into()));
        assert_eq!(lossy_string(&json!(1.5)), Some("1.5".into()));
        for absent in [json!(null), json!(true), json!([1]), json!({"a": 1})] {
            assert_eq!(lossy_string(&absent), None, "{absent}");
        }
    }

    #[test]
    fn lossy_ints() {
        assert_eq!(lossy_int(&json!(42)), Some(42));
        assert_eq!(lossy_int(&json!(42.0)), Some(42));
        assert_eq!(lossy_int(&json!("42")), Some(42));
        assert_eq!(lossy_int(&json!("+42")), Some(42));
        assert_eq!(lossy_int(&json!("-4")), Some(-4));
        for absent in [
            json!(4.5),
            json!(" 42"),
            json!("4a"),
            json!(""),
            json!(true),
            json!(null),
            json!(9.3e18),
        ] {
            assert_eq!(lossy_int(&absent), None, "{absent}");
        }
    }

    #[test]
    fn lossy_bools() {
        assert_eq!(lossy_bool(&json!(true)), Some(true));
        assert_eq!(lossy_bool(&json!("YES")), Some(true));
        assert_eq!(lossy_bool(&json!("0")), Some(false));
        assert_eq!(lossy_bool(&json!("No")), Some(false));
        assert_eq!(lossy_bool(&json!(2)), Some(true));
        assert_eq!(lossy_bool(&json!(0)), Some(false));
        assert_eq!(lossy_bool(&json!("maybe")), None);
        assert_eq!(lossy_bool(&json!(0.5)), None);
    }

    #[test]
    fn status_line_numbers() {
        assert_eq!(json_double(Some(&json!(" 12.5 "))), Some(12.5));
        assert_eq!(json_double(Some(&json!(true))), None);
        assert_eq!(json_double(Some(&json!("inf"))), None);
        assert_eq!(json_int(Some(&json!(4242.9))), Some(4242));
        assert_eq!(json_int(Some(&json!(-3.7))), Some(-3));
        assert_eq!(json_int(Some(&json!(9_223_372_036_854_775_808.0))), None);
        assert_eq!(json_string(Some(&json!(""))), None);
        assert_eq!(json_string(Some(&json!(2))), Some("2".into()));
        assert_eq!(json_string(Some(&json!(false))), Some("0".into()));
    }
}
