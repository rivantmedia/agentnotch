//! `an_call` (DESIGN-WIN §3.5, §3.7): the pages' one way into the fork.
//!
//! Each call is first checked against the window it came from (the engine's
//! `allowed_from_window`, so the rule lives in one place), then either handled here (the
//! glue-level methods: windows, clipboard, Explorer, logs) or turned into the engine's `Call` by
//! `Call::from_parts` and run by the hub on a blocking thread. The JSON shape `{method, args}` is
//! the contract; the glue never builds a `Call` any other way, so it depends on the table in
//! §3.5, not on how the engine spells its variants.

use agentnotch_engine::hub::{allowed_from_window, Call, CallError, Hub};
use serde_json::{json, Value};
use tauri::AppHandle;

use super::panel;

/// Whether the window labelled `label` may make the call `method` (§3.7).
pub(super) fn allowed(label: &str, method: &str) -> bool {
    allowed_from_window(label, method)
}

pub(super) async fn dispatch(
    app: AppHandle,
    label: String,
    method: String,
    args: Value,
) -> Result<Value, CallError> {
    if !allowed(&label, &method) {
        return Err(error(
            "refused",
            format!("{method} isn't available to this window"),
        ));
    }
    if let Some(reply) = glue_method(&app, &label, &method, &args) {
        return reply;
    }
    let hub = super::hub().ok_or_else(|| {
        error(
            "failed",
            format!(
                "{}'s Claude Code control isn't running",
                super::DISPLAY_NAME
            ),
        )
    })?;
    tauri::async_runtime::spawn_blocking(move || engine_call(&hub, &method, args))
        .await
        .map_err(|e| error("failed", e.to_string()))?
}

/// Runs `{method, args}` on the hub (blocking; at most the hub's call timeout).
pub(super) fn engine_call(hub: &Hub, method: &str, args: Value) -> Result<Value, CallError> {
    hub.call(Call::from_parts(method, args)?)
}

/// `engine_call` on its own thread, waited for at most `wait`: `None` when the hub hasn't answered
/// by then (the call still completes). For callers on the main thread, which must not stall.
pub(super) fn engine_call_within(
    hub: Hub,
    method: &'static str,
    args: Value,
    wait: std::time::Duration,
) -> Option<Result<Value, CallError>> {
    let (reply, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = reply.send(engine_call(&hub, method, args));
    });
    answer.recv_timeout(wait).ok()
}

/// The methods that never reach the engine. `None` = not one of them.
fn glue_method(
    app: &AppHandle,
    label: &str,
    method: &str,
    args: &Value,
) -> Option<Result<Value, CallError>> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
    let reply = match method {
        "log" => {
            // Pages log their own events only (never prompts, replies or paths); a runaway line
            // is cut so it can't fill run.log.
            let msg: String = text("msg").unwrap_or_default().chars().take(500).collect();
            super::log(&format!("page {label}: {msg}"));
            Ok(json!({}))
        }
        "panel_open" => {
            let route = text("route").unwrap_or_else(|| "sessions".into());
            let reason = text("reason").unwrap_or_else(|| "settings".into());
            panel::open_route(app, route, text("ring_id"), reason);
            Ok(json!({}))
        }
        "panel_toggle" => {
            let reason = text("reason").unwrap_or_else(|| "ring_click".into());
            panel::toggle(app, text("ring_id"), reason);
            Ok(json!({}))
        }
        "panel_close" => {
            panel::close(app);
            Ok(json!({}))
        }
        "panel_route" => {
            if let Some(route) = text("route") {
                panel::set_route(route);
            }
            Ok(json!({}))
        }
        "panel_report_size" => {
            let height = args.get("h").and_then(Value::as_f64).unwrap_or(0.0);
            panel::report_height(app, height);
            Ok(json!({}))
        }
        "panel_take_focus" => {
            panel::take_focus(app);
            Ok(json!({}))
        }
        "panel_engaged" => {
            panel::set_engaged(args.get("on").and_then(Value::as_bool).unwrap_or(false));
            Ok(json!({}))
        }
        "open_settings" => {
            crate::settings_window::open(app);
            Ok(json!({}))
        }
        // Each of these reaches the OS through agentnotch-win (a URL through ShellExecuteW, never
        // a command line built from page data); none is wired up in this build.
        "open_url" | "open_notification_settings" | "copy_text" | "reveal" | "pick_folder" => Err(
            error("failed", format!("{method} isn't available in this build")),
        ),
        _ => return None,
    };
    Some(reply)
}

pub(super) fn error(code: &str, message: impl Into<String>) -> CallError {
    CallError {
        code: code.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[test]
    fn glue_only_calls_are_refused_to_every_page() {
        for label in ["agentnotch-panel", "notch", "settings", "dropzones"] {
            assert!(!allowed(label, "panel_state"), "{label}");
            assert!(!allowed(label, "hotkey_status"), "{label}");
        }
    }

    #[test]
    fn the_panel_may_call_the_rest() {
        for method in [
            "answer",
            "send_message",
            "snapshot",
            "chat_open",
            "panel_close",
            "log",
        ] {
            assert!(allowed("agentnotch-panel", method), "{method}");
        }
    }

    #[test]
    fn the_notch_calls_only_its_own_list() {
        // `open_settings`: the notch's "Turn on in Settings" chip (§7.5 step 7).
        for method in [
            "snapshot",
            "refresh_usage",
            "focus",
            "mark_reviewed",
            "panel_toggle",
            "open_settings",
            "log",
        ] {
            assert!(allowed("notch", method), "{method}");
        }
        for method in [
            "answer",
            "send_message",
            "cloud",
            "hook_consent",
            "set_setting",
            "open_url",
        ] {
            assert!(!allowed("notch", method), "{method}");
        }
    }

    #[test]
    fn settings_never_answers_or_types() {
        assert!(!allowed("settings", "answer"));
        assert!(!allowed("settings", "send_message"));
        for method in [
            "settings",
            "hook_consent",
            "cloud",
            "set_setting",
            "panel_open",
            "open_url",
        ] {
            assert!(allowed("settings", method), "{method}");
        }
    }

    #[test]
    fn other_windows_call_nothing() {
        assert!(!allowed("dropzones", "snapshot"));
        assert!(!allowed("", "snapshot"));
        assert!(!allowed("agentnotch-panel-2", "answer"));
    }
}
