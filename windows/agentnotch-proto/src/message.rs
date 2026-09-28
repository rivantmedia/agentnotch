//! The messages a hook sends: the Mac hook script's `build_message` and the
//! status line wrapper's, field for field (HS§1.4, §2), with the same
//! truncation limits and the same coarse `status`, plus the Windows fields
//! `protocol`, `hook_pid` and `terminal`.
//!
//! The builders take Claude Code's stdin as parsed JSON and what the exe
//! knows besides ([`HookEnv`]); they never touch the environment or the OS
//! themselves, so the exact output is testable everywhere.

use crate::limits::*;
use crate::pyjson;
use crate::PROTOCOL;
use serde_json::{json, Map, Value};

/// The `event` of a status line message.
pub const STATUS_LINE_EVENT: &str = "StatusLine";

/// What the hook exe knows besides stdin. Each variable is passed raw
/// (`None` when unset); `pid_guess` is what DESIGN-WIN §1.4's walk found
/// when `CLAUDE_PID` is absent (the exe computes it with [`crate::pid_guess`];
/// the builder only applies the precedence).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HookEnv {
    /// `CLAUDE_PID`.
    pub claude_pid: Option<String>,
    /// `CLAUDE_CONFIG_DIR`, forwarded verbatim.
    pub claude_config_dir: Option<String>,
    /// `CLAUDE_CODE_SESSION_ATTENDED`.
    pub attended: Option<String>,
    /// `CLAUDE_CODE_ENTRYPOINT`.
    pub entrypoint: Option<String>,
    pub pid_guess: Option<u32>,
    /// The hook exe's own pid.
    pub hook_pid: u32,
    /// `WT_SESSION` (Windows Terminal's tab id).
    pub wt_session: Option<String>,
    /// `TERM_PROGRAM` (`vscode` in VS Code's terminal).
    pub term_program: Option<String>,
}

impl HookEnv {
    /// The environment variables read, from a getter (the exe passes
    /// `std::env::var`); `pid_guess` and `hook_pid` are filled by the caller.
    pub fn from_env(get: impl Fn(&str) -> Option<String>, hook_pid: u32) -> HookEnv {
        HookEnv {
            claude_pid: get("CLAUDE_PID"),
            claude_config_dir: get("CLAUDE_CONFIG_DIR"),
            attended: get("CLAUDE_CODE_SESSION_ATTENDED"),
            entrypoint: get("CLAUDE_CODE_ENTRYPOINT"),
            pid_guess: None,
            hook_pid,
            wt_session: get("WT_SESSION"),
            term_program: get("TERM_PROGRAM"),
        }
    }

    /// `CLAUDE_PID` when it parses (as Python's `int()`) to 1..=2³¹−1.
    pub fn claude_pid_value(&self) -> Option<u32> {
        self.claude_pid.as_deref().and_then(python_int).and_then(valid_pid)
    }

    /// The hook event's `pid`: `CLAUDE_PID`, else the walk's guess, else none.
    fn hook_pid_field(&self) -> Value {
        match self.claude_pid_value().or(self.pid_guess.and_then(|p| valid_pid(p as i64))) {
            Some(pid) => json!(pid),
            None => Value::Null,
        }
    }

    fn attended_field(&self) -> Value {
        match self.attended.as_deref() {
            Some("1") => Value::Bool(true),
            Some("0") => Value::Bool(false),
            _ => Value::Null,
        }
    }

    fn terminal_field(&self) -> Value {
        json!({ "wt_session": self.wt_session, "term_program": self.term_program })
    }
}

/// The Mac's `status_for`: the coarse session status of an event.
pub fn status_for(event: &str, data: &Map<String, Value>) -> &'static str {
    match event {
        "PreToolUse" => "running_tool",
        "PermissionRequest" => "waiting_for_approval",
        "Stop" | "StopFailure" | "SessionStart" => "waiting_for_input",
        "SessionEnd" => "ended",
        "PreCompact" => "compacting",
        "Notification" => {
            if data.get("notification_type").and_then(Value::as_str) == Some("idle_prompt") {
                "waiting_for_input"
            } else {
                "notification"
            }
        }
        "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" | "SubagentStart"
        | "SubagentStop" | "PostCompact" | "TaskCreated" | "TaskCompleted" => "processing",
        _ => "unknown",
    }
}

/// The hook event message for Claude Code's stdin, or `None` when stdin is
/// not a JSON object.
pub fn build_hook_message(stdin: &Value, env: &HookEnv) -> Option<Value> {
    let data = stdin.as_object()?;
    let get = |key: &str| data.get(key).cloned().unwrap_or(Value::Null);
    let event = match data.get("hook_event_name") {
        Some(value) if truthy(value) => value.clone(),
        _ => Value::String(String::new()),
    };
    let event_name = event.as_str().unwrap_or("").to_owned();

    let mut m = Map::new();
    m.insert("protocol".into(), json!(PROTOCOL));
    m.insert("event".into(), event);
    m.insert("session_id".into(), or_default(data.get("session_id"), "unknown"));
    m.insert("cwd".into(), or_default(data.get("cwd"), ""));
    m.insert("transcript_path".into(), get("transcript_path"));
    m.insert("pid".into(), env.hook_pid_field());
    m.insert("status".into(), json!(status_for(&event_name, data)));
    m.insert("config_dir_env".into(), json!(env.claude_config_dir));
    m.insert("attended".into(), env.attended_field());
    m.insert("entrypoint".into(), json!(env.entrypoint));
    m.insert("agent_id".into(), get("agent_id"));
    m.insert("agent_type".into(), get("agent_type"));
    m.insert("permission_mode".into(), get("permission_mode"));
    m.insert("hook_pid".into(), json!(env.hook_pid));
    m.insert("terminal".into(), env.terminal_field());

    let is_tool_event = matches!(
        event_name.as_str(),
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" | "PermissionDenied"
    );
    if is_tool_event {
        m.insert("tool".into(), get("tool_name"));
        let tool_input = match data.get("tool_input") {
            Some(input @ Value::Object(_)) => truncate_deep(input, MAX_TOOL_INPUT_STRING, None),
            _ => Value::Object(Map::new()),
        };
        m.insert("tool_input".into(), tool_input);
        // PermissionRequest has no tool_use_id: the app matches it to the
        // preceding PreToolUse.
        if let Some(id) = data.get("tool_use_id").filter(|v| truthy(v)) {
            m.insert("tool_use_id".into(), id.clone());
        }
    }

    if event_name == "PostToolUse" && data.get("tool_name").and_then(Value::as_str) == Some("TaskCreate") {
        let task = data.get("tool_response").and_then(Value::as_object).and_then(|r| r.get("task"));
        if let Some(Value::Object(task)) = task {
            if let Some(id) = task.get("id").filter(|v| !v.is_null()) {
                m.insert("task_id".into(), json!(python_str(id)));
            }
            m.insert("task_subject".into(), text_or_none(task.get("subject"), MAX_TEXT));
        }
    }

    match event_name.as_str() {
        "PostToolUseFailure" => {
            m.insert("tool_error".into(), text_or_none(dumped(data.get("error")).as_ref(), MAX_TOOL_ERROR));
            if let Some(flag @ Value::Bool(_)) = data.get("is_interrupt") {
                m.insert("is_interrupt".into(), flag.clone());
            }
        }
        "PermissionRequest" => {
            if let Some(list @ Value::Array(_)) = data.get("permission_suggestions") {
                m.insert("permission_suggestions".into(), list.clone());
            }
        }
        "PermissionDenied" => {
            let reason = python_or(data.get("reason"), data.get("message"));
            m.insert("denial_reason".into(), text_or_none(reason, MAX_TEXT));
        }
        "Notification" => {
            m.insert("notification_type".into(), get("notification_type"));
            m.insert("message".into(), text_or_none(data.get("message"), MAX_TEXT));
            m.insert("title".into(), text_or_none(data.get("title"), MAX_TEXT));
        }
        "Stop" | "StopFailure" | "SubagentStop" => {
            m.insert(
                "last_assistant_message".into(),
                text_or_none(data.get("last_assistant_message"), MAX_LAST_ASSISTANT_MESSAGE),
            );
            match event_name.as_str() {
                "Stop" => {
                    let background = data.get("background_tasks").and_then(Value::as_array);
                    m.insert("background_task_count".into(), json!(background.map_or(0, Vec::len)));
                    if let Some(tasks) = background {
                        let types: Vec<Value> = tasks
                            .iter()
                            .take(MAX_BACKGROUND_TYPES)
                            .filter_map(|task| task.as_object()?.get("type").filter(|t| t.is_string()).cloned())
                            .collect();
                        m.insert("background_task_types".into(), Value::Array(types));
                    }
                    if let Some(crons) = data.get("session_crons").and_then(Value::as_array) {
                        m.insert("session_cron_count".into(), json!(crons.len()));
                    }
                    if let Some(flag @ Value::Bool(_)) = data.get("stop_hook_active") {
                        m.insert("stop_hook_active".into(), flag.clone());
                    }
                }
                "StopFailure" => {
                    let error = match data.get("error") {
                        Some(Value::String(s)) => json!(s),
                        _ => json!("unknown"),
                    };
                    m.insert("stop_error".into(), error);
                    let details = dumped(data.get("error_details"));
                    m.insert("stop_error_details".into(), text_or_none(details.as_ref(), MAX_TOOL_ERROR));
                }
                _ => {
                    m.insert("agent_transcript_path".into(), get("agent_transcript_path"));
                }
            }
        }
        "TaskCreated" | "TaskCompleted" => {
            if let Some(id) = data.get("task_id").filter(|v| !v.is_null()) {
                m.insert("task_id".into(), json!(python_str(id)));
            }
            m.insert("task_subject".into(), text_or_none(data.get("task_subject"), MAX_TEXT));
        }
        "SessionStart" => {
            m.insert("source".into(), get("source"));
            let model = data.get("model").filter(|v| v.is_string()).cloned().unwrap_or(Value::Null);
            m.insert("model".into(), model);
            m.insert("session_title".into(), text_or_none(data.get("session_title"), MAX_SESSION_TITLE));
        }
        "UserPromptSubmit" => {
            m.insert("source".into(), get("source"));
            m.insert("session_title".into(), text_or_none(data.get("session_title"), MAX_SESSION_TITLE));
            m.insert("prompt".into(), text_or_none(data.get("prompt"), MAX_PROMPT));
        }
        "SessionEnd" => {
            m.insert("reason".into(), get("reason"));
        }
        "PreCompact" | "PostCompact" => {
            m.insert("trigger".into(), get("trigger"));
        }
        _ => {}
    }

    Some(Value::Object(m))
}

/// The message as frame bytes, at most [`MAX_CLIENT_MESSAGE`]: a tool input
/// still too big after the first clamp is cut down again (strings to 500,
/// lists to 50 items), then dropped. The app only shows it; a permission
/// decision is merged onto the original stdin input, never onto this copy.
pub fn encode_hook_message(msg: &Value) -> Vec<u8> {
    let payload = to_bytes(msg);
    let has_input = msg.get("tool_input").is_some_and(truthy);
    if payload.len() <= MAX_CLIENT_MESSAGE || !has_input {
        return payload;
    }
    let mut smaller = msg.clone();
    let reduced = truncate_deep(&msg["tool_input"], MAX_TOOL_INPUT_STRING_REDUCED, Some(MAX_LIST_ITEMS));
    smaller["tool_input"] = reduced;
    let payload = to_bytes(&smaller);
    if payload.len() <= MAX_CLIENT_MESSAGE {
        return payload;
    }
    smaller["tool_input"] = Value::Object(Map::new());
    to_bytes(&smaller)
}

/// The status line message, or `None` when stdin is not a JSON object. No
/// parent-pid fallback for `pid`: the wrapper runs under a shell.
pub fn build_statusline_message(stdin: &Value, env: &HookEnv) -> Option<Value> {
    let data = stdin.as_object()?;
    let get = |key: &str| data.get(key).cloned().unwrap_or(Value::Null);
    let empty = Map::new();
    let context = data.get("context_window").and_then(Value::as_object).unwrap_or(&empty);
    let cost = data.get("cost").and_then(Value::as_object).unwrap_or(&empty);
    let field = |map: &Map<String, Value>, key: &str| map.get(key).cloned().unwrap_or(Value::Null);
    Some(json!({
        "protocol": PROTOCOL,
        "event": STATUS_LINE_EVENT,
        "session_id": get("session_id"),
        "transcript_path": get("transcript_path"),
        "cwd": get("cwd"),
        "config_dir_env": env.claude_config_dir,
        "pid": env.claude_pid_value(),
        "status_line": {
            "rate_limits": get("rate_limits"),
            "context_window": {
                "used_percentage": field(context, "used_percentage"),
                "context_window_size": field(context, "context_window_size"),
            },
            "model": get("model"),
            "cost": {"total_cost_usd": field(cost, "total_cost_usd")},
            "session_name": get("session_name"),
            "version": get("version"),
        },
    }))
}

fn to_bytes(value: &Value) -> Vec<u8> {
    // Serialising a Value cannot fail: its keys are strings.
    serde_json::to_vec(value).unwrap_or_default()
}

fn valid_pid(pid: i64) -> Option<u32> {
    (1..=i32::MAX as i64).contains(&pid).then_some(pid as u32)
}

/// Python's `int(str)`: surrounding whitespace, a sign, and underscores
/// between digits are allowed; nothing else.
fn python_int(raw: &str) -> Option<i64> {
    let text = raw.trim_matches(|c: char| c.is_whitespace());
    let (negative, digits) = match text.as_bytes().first()? {
        b'+' => (false, &text[1..]),
        b'-' => (true, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut value: i64 = 0;
    for byte in digits.bytes() {
        match byte {
            b'_' => continue,
            b'0'..=b'9' => value = value.checked_mul(10)?.checked_add((byte - b'0') as i64)?,
            _ => return None,
        }
    }
    Some(if negative { -value } else { value })
}

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `data.get(key) or default`.
fn or_default(value: Option<&Value>, default: &str) -> Value {
    match value {
        Some(v) if truthy(v) => v.clone(),
        _ => Value::String(default.to_owned()),
    }
}

/// `a or b` for two optional values.
fn python_or<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    match a {
        Some(v) if truthy(v) => Some(v),
        _ => b,
    }
}

/// A non-string, non-null value as `json.dumps` writes it; strings and null
/// pass through.
fn dumped(value: Option<&Value>) -> Option<Value> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(Value::String(s.clone())),
        Some(other) => Some(Value::String(pyjson::dumps_value(other))),
    }
}

/// Python's `str(value)` for the scalars a task id can be.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// A non-empty string clamped to `limit` characters, else null.
fn text_or_none(value: Option<&Value>, limit: usize) -> Value {
    match value {
        Some(Value::String(s)) if !s.is_empty() => Value::String(clamp(s, limit)),
        _ => Value::Null,
    }
}

/// The first `limit` Unicode scalar values (Python's `s[:limit]`).
fn clamp(s: &str, limit: usize) -> String {
    match s.char_indices().nth(limit) {
        Some((end, _)) => s[..end].to_owned(),
        None => s.to_owned(),
    }
}

/// Clamp every nested string; with `max_items`, cut lists too.
fn truncate_deep(value: &Value, limit: usize, max_items: Option<usize>) -> Value {
    match value {
        Value::String(s) => Value::String(clamp(s, limit)),
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), truncate_deep(v, limit, max_items))).collect())
        }
        Value::Array(items) => {
            let kept = max_items.map_or(items.len(), |max| max.min(items.len()));
            Value::Array(items[..kept].iter().map(|v| truncate_deep(v, limit, max_items)).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_int_rules() {
        assert_eq!(python_int("123"), Some(123));
        assert_eq!(python_int(" 42\n"), Some(42));
        assert_eq!(python_int("+7"), Some(7));
        assert_eq!(python_int("-3"), Some(-3));
        assert_eq!(python_int("1_000"), Some(1000));
        for bad in ["", " ", "12a", "_1", "1_", "1__0", "0x10", "1.5", "+"] {
            assert_eq!(python_int(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn clamp_counts_characters() {
        assert_eq!(clamp("héllo", 2), "hé");
        assert_eq!(clamp("😀😀😀", 2), "😀😀");
        assert_eq!(clamp("ab", 5), "ab");
    }

    #[test]
    fn truthiness() {
        for (value, expected) in [
            (json!(null), false),
            (json!(0), false),
            (json!(0.0), false),
            (json!(""), false),
            (json!([]), false),
            (json!({}), false),
            (json!(false), false),
            (json!("x"), true),
            (json!(2), true),
            (json!([0]), true),
        ] {
            assert_eq!(truthy(&value), expected, "{value}");
        }
    }
}
