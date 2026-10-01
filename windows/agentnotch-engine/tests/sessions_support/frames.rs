//! A test-only decoder for the frames the hook exe and the status line
//! wrapper send (what WP1's `HookIngress` does in production).
//!
//! Ingress is another package's and a stub on this branch, so the
//! end-to-end tests decode `TransportEvent`s themselves, with the same
//! shape of rules: the frame is a little-endian length and a JSON object,
//! a `PermissionRequest` takes its `tool_use_id` from the preceding
//! `PreToolUse` of the same session, tool and agent (the `ToolUseIdCache`;
//! a made-up `permission-<n>` when none matches) and becomes a held
//! permission, and unattended or SDK sessions are dropped. After the merge,
//! `tests/sessions_e2e.rs` should switch to the real `HookIngress`.
#![allow(dead_code)]

use agentnotch_engine::core::paths::Paths;
use agentnotch_engine::model::{
    AccountId, HeldPermission, HookEvent, HookTerminal, SessionId, StatusLineMessage,
};
use agentnotch_engine::platform::TransportEvent;
use serde_json::Value;
use std::collections::HashMap;
use std::time::SystemTime;

/// What one transport event decoded to.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    Hook(HookEvent),
    StatusLine(StatusLineMessage),
    Held(HeldPermission),
    /// A frame the ingress drops (not a JSON object, an ignored session).
    Dropped(&'static str),
    /// `Listening`, `PeerClosed` and `Error` events carry no session input.
    Other,
}

/// The JSON of a frame (`u32` little endian length, then the bytes).
pub fn unframe(bytes: &[u8]) -> Result<Value, &'static str> {
    let (length, body) = bytes
        .split_first_chunk::<4>()
        .ok_or("a frame shorter than its header")?;
    if u32::from_le_bytes(*length) as usize != body.len() {
        return Err("a frame whose length is not its body's");
    }
    serde_json::from_slice(body).map_err(|_| "a frame that is not JSON")
}

pub struct FrameDecoder {
    /// (session, tool, agent) -> the `tool_use_id` of its last PreToolUse.
    cache: HashMap<(String, String, Option<String>), String>,
    synthetic: u64,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        FrameDecoder::new()
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        FrameDecoder {
            cache: HashMap::new(),
            synthetic: 0,
        }
    }

    pub fn decode(&mut self, event: TransportEvent, paths: &Paths) -> Decoded {
        let TransportEvent::Frame(frame) = event else {
            return Decoded::Other;
        };
        let Ok(value) = unframe(&frame.bytes) else {
            return Decoded::Dropped("not a frame");
        };
        let Some(object) = value.as_object() else {
            return Decoded::Dropped("not an object");
        };
        if object.get("event").and_then(Value::as_str) == Some("StatusLine") {
            return match status_line(&value, frame.received_at, paths) {
                Some(message) => Decoded::StatusLine(message),
                None => Decoded::Dropped("a status line without a session"),
            };
        }
        let Some(mut event) = hook_event(&value, frame.received_at) else {
            return Decoded::Dropped("a hook message without a session");
        };
        if event.is_from_ignored_session() {
            return Decoded::Dropped("an ignored session");
        }
        let key = |event: &HookEvent| {
            (
                event.session_id.to_string(),
                event.tool.clone().unwrap_or_default(),
                event.agent_id.clone(),
            )
        };
        match event.event.as_str() {
            "PreToolUse" => {
                if let Some(id) = &event.tool_use_id {
                    self.cache.insert(key(&event), id.clone());
                }
                Decoded::Hook(event)
            }
            "PermissionRequest" => {
                let cached = self.cache.remove(&key(&event));
                let synthetic = cached.is_none();
                let tool_use_id = cached.unwrap_or_else(|| {
                    self.synthetic += 1;
                    format!("permission-{}", self.synthetic)
                });
                event.tool_use_id = Some(tool_use_id.clone());
                event.has_synthetic_tool_use_id = synthetic;
                Decoded::Held(HeldPermission {
                    conn: frame.conn,
                    session_id: event.session_id.clone(),
                    tool_use_id,
                    has_synthetic_tool_use_id: synthetic,
                    agent_id: event.agent_id.clone(),
                    received_at: frame.received_at,
                    event,
                })
            }
            _ => Decoded::Hook(event),
        }
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn count(value: &Value, key: &str) -> Option<u32> {
    value.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// A hook message (`build_hook_message`'s output) as an event; none without
/// a session id.
pub fn hook_event(value: &Value, received_at: SystemTime) -> Option<HookEvent> {
    let session = text(value, "session_id").filter(|id| !id.is_empty())?;
    let mut event = HookEvent::new(
        SessionId::from(session),
        text(value, "event").unwrap_or_default(),
        received_at,
    );
    event.cwd = text(value, "cwd").unwrap_or_default();
    event.status = text(value, "status").unwrap_or_else(|| "unknown".into());
    event.pid = count(value, "pid");
    event.transcript_path = text(value, "transcript_path");
    event.config_dir_env = text(value, "config_dir_env");
    event.attended = value.get("attended").and_then(Value::as_bool);
    event.entrypoint = text(value, "entrypoint");
    event.agent_id = text(value, "agent_id");
    event.agent_type = text(value, "agent_type");
    event.permission_mode = text(value, "permission_mode");
    event.tool = text(value, "tool");
    event.tool_input = value.get("tool_input").and_then(Value::as_object).cloned();
    event.tool_use_id = text(value, "tool_use_id");
    event.tool_error = text(value, "tool_error");
    event.is_interrupt = value.get("is_interrupt").and_then(Value::as_bool);
    event.permission_suggestions = value
        .get("permission_suggestions")
        .and_then(Value::as_array)
        .cloned();
    event.denial_reason = text(value, "denial_reason");
    event.task_id = text(value, "task_id");
    event.task_subject = text(value, "task_subject");
    event.notification_type = text(value, "notification_type");
    event.message = text(value, "message");
    event.title = text(value, "title");
    event.last_assistant_message = text(value, "last_assistant_message");
    event.background_task_count = count(value, "background_task_count");
    event.background_task_types = value
        .get("background_task_types")
        .and_then(Value::as_array)
        .map(|types| {
            types
                .iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        });
    event.session_cron_count = count(value, "session_cron_count");
    event.stop_hook_active = value.get("stop_hook_active").and_then(Value::as_bool);
    event.stop_error = text(value, "stop_error");
    event.stop_error_details = text(value, "stop_error_details");
    event.agent_transcript_path = text(value, "agent_transcript_path");
    event.source = text(value, "source");
    event.model = text(value, "model");
    event.session_title = text(value, "session_title");
    event.prompt = text(value, "prompt");
    event.reason = text(value, "reason");
    event.trigger = text(value, "trigger");
    event.protocol = count(value, "protocol").unwrap_or(1);
    event.hook_pid = count(value, "hook_pid");
    event.terminal = value.get("terminal").map(|terminal| HookTerminal {
        wt_session: text(terminal, "wt_session"),
        term_program: text(terminal, "term_program"),
    });
    Some(event)
}

/// A status line message (`build_statusline_message`'s output). The usage
/// windows are WP4's: only the raw `rate_limits` is carried.
pub fn status_line(
    value: &Value,
    received_at: SystemTime,
    paths: &Paths,
) -> Option<StatusLineMessage> {
    let session = text(value, "session_id").filter(|id| !id.is_empty())?;
    let line = value.get("status_line").cloned().unwrap_or(Value::Null);
    let context = line.get("context_window").cloned().unwrap_or(Value::Null);
    let transcript_path = text(value, "transcript_path");
    let config_dir_env = text(value, "config_dir_env");
    let account_id = Some(AccountId::from(paths.session_config_dir(
        transcript_path.as_deref(),
        config_dir_env.as_deref(),
        |_| false,
    )));
    Some(StatusLineMessage {
        session_id: SessionId::from(session),
        cwd: text(value, "cwd"),
        transcript_path,
        config_dir_env,
        account_id,
        received_at,
        rate_limits: line.get("rate_limits").filter(|r| !r.is_null()).cloned(),
        five_hour: None,
        seven_day: None,
        context_used_percent: context.get("used_percentage").and_then(Value::as_f64),
        context_window_size: context.get("context_window_size").and_then(Value::as_u64),
        model_id: line
            .get("model")
            .and_then(|m| text(m, "id"))
            .filter(|m| !m.is_empty()),
        model_display_name: line
            .get("model")
            .and_then(|m| text(m, "display_name"))
            .filter(|m| !m.is_empty()),
        cost_usd: line
            .get("cost")
            .and_then(|c| c.get("total_cost_usd"))
            .and_then(Value::as_f64),
        session_name: text(&line, "session_name").filter(|n| !n.is_empty()),
        claude_code_version: text(&line, "version"),
        pid: count(value, "pid"),
    })
}
