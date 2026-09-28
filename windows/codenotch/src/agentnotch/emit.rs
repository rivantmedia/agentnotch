//! The hub's events, delivered to Tauri (DESIGN-WIN §3.5 `HubEvent`, §3.7).
//!
//! Called on the hub's own thread (`an-core`): `emit`/`emit_to` are thread-safe, and anything
//! that touches a window hops to the main thread inside `panel`. Nothing here may block, and
//! nothing may call back into the hub synchronously (the hub is the caller).

use agentnotch_engine::hub::{HubEvent, UpstreamUsage};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::panel;

/// Upstream's notch window.
const NOTCH: &str = "notch";
/// Upstream's settings window.
const SETTINGS: &str = "settings";

pub(super) fn forward(app: &AppHandle, event: &HubEvent) {
    match event {
        HubEvent::Snapshot(snapshot) => {
            emit_to(app, NOTCH, "an:snapshot", snapshot);
            emit_to(app, panel::LABEL, "an:snapshot", snapshot);
        }
        HubEvent::Settings(settings) => emit_to(app, SETTINGS, "an:settings", settings),
        HubEvent::Cloud(cloud) => emit_to(app, SETTINGS, "an:cloud", cloud),
        HubEvent::Chat(update) => emit_to(app, panel::LABEL, "an:chat", update),
        HubEvent::Panel(request) => panel::open_request(app, request.clone()),
        HubEvent::PanelClose { reason } => {
            super::log(&format!("panel closed by the hub ({reason:?})"));
            panel::close(app);
        }
        HubEvent::Peek { ring_id, seconds } => emit_to(
            app,
            NOTCH,
            "an:peek",
            serde_json::json!({ "ring_id": ring_id, "seconds": seconds }),
        ),
        HubEvent::UpstreamUsage(usage) => publish_usage(app, usage),
        HubEvent::TrayBadge(count) => super::tray::set_badge(app, *count),
        HubEvent::Notice(text) => emit_to(app, NOTCH, "an:notice", text),
        HubEvent::Log(line) => super::log(line),
        HubEvent::Quit => {
            // The hub asked (control op `quit`): stop it first so stores are saved and held
            // requests are released, then exit. Never on this thread: stopping waits for the hub,
            // which is waiting for this callback to return.
            let app = app.clone();
            std::thread::spawn(move || {
                if let Some(hub) = super::hub() {
                    hub.stop();
                }
                app.exit(0);
            });
        }
    }
}

/// The fork's usage projection in upstream's shape (DESIGN-WIN §4.6): upstream's tray, menu and
/// notch code read `AppState.usage` and listen to `usage`, exactly as they did when upstream's
/// own Claude poller filled them.
pub(super) fn publish_usage(app: &AppHandle, usage: &UpstreamUsage) {
    let snapshot = crate::usage::UsageSnapshot {
        status: usage.status.clone(),
        windows: usage
            .windows
            .iter()
            .map(|w| crate::usage::LimitWindow {
                id: w.id.clone(),
                label: w.label.clone(),
                used: w.used,
                resets_at: w.resets_at,
                count: w.count,
                derived: w.derived,
                group: w.group.clone(),
            })
            .collect(),
        fetched_at: usage.fetched_at,
        note: usage.note.clone(),
        backoff_until: usage.backoff_until,
    };
    {
        let state = app.state::<crate::AppState>();
        // A poisoned lock only means another thread panicked mid-update; the reading replaces
        // whatever it left, so take the lock back rather than lose the notch over it.
        let mut current = state.usage.lock().unwrap_or_else(|e| e.into_inner());
        *current = snapshot.clone();
    }
    let _ = app.emit("usage", &snapshot);
}

/// An event for one window; nothing happens when that window doesn't exist (yet).
fn emit_to<S: Serialize + Clone>(app: &AppHandle, label: &str, event: &str, payload: S) {
    let _ = app.emit_to(label, event, payload);
}
