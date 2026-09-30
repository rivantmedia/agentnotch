//! The hub's events, delivered to Tauri (DESIGN-WIN §3.5 `HubEvent`, §3.7).
//!
//! Called on the hub's own thread (`an-core`): `emit`/`emit_to` are thread-safe, and anything
//! that touches a window hops to the main thread inside `panel`. Nothing here may block, and
//! nothing may call back into the hub synchronously (the hub is the caller).

use agentnotch_engine::hub::HubEvent;
use agentnotch_engine::model::UpstreamUsage;
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
            // "Keep open" is a setting; the panel's window rules need its value.
            panel::set_pinned(app, snapshot.ui.panel_pinned);
        }
        HubEvent::Settings(settings) => {
            emit_to(app, SETTINGS, "an:settings", settings);
            // Two settings the glue itself acts on: the panel shortcut and the tray's dot.
            super::hotkey::apply_setting(app, &settings.attention.hotkey);
            super::tray::set_dot_enabled(app, settings.attention.tray_badge);
        }
        HubEvent::Cloud(cloud) => emit_to(app, SETTINGS, "an:cloud", cloud),
        HubEvent::Chat(update) => emit_to(app, panel::LABEL, "an:chat", update),
        HubEvent::Panel(request) => panel::open_request(app, request.clone()),
        HubEvent::PanelClose { reason } => {
            super::log(&format!("panel closed by the hub ({reason:?})"));
            panel::close_by_hub(app, reason);
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
    let snapshot = upstream_snapshot(usage);
    {
        let state = app.state::<crate::AppState>();
        // A poisoned lock only means another thread panicked mid-update; the reading replaces
        // whatever it left, so take the lock back rather than lose the notch over it.
        let mut current = state.usage.lock().unwrap_or_else(|e| e.into_inner());
        *current = snapshot.clone();
    }
    let _ = app.emit("usage", &snapshot);
}

/// Upstream's word for "no reading, and nothing wrong": what its pages show without a button.
const NO_READING: &str = "none";

/// The status as upstream's pages may see it. Upstream's `needsAuth` puts a "Sign in" button on
/// the ring, which starts Claude Code's browser login from the app: part of the token path the
/// fork never takes (seam WU1). The engine never says it (a signed-out account is `none` with a
/// note); should a status ever arrive spelled that way, it is shown as no reading.
fn upstream_status(status: &str) -> String {
    let word: String = status
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if word == "needsauth" {
        NO_READING.into()
    } else {
        status.into()
    }
}

fn upstream_snapshot(usage: &UpstreamUsage) -> crate::usage::UsageSnapshot {
    crate::usage::UsageSnapshot {
        status: upstream_status(&usage.status),
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
    }
}

/// An event for one window; nothing happens when that window doesn't exist (yet).
fn emit_to<S: Serialize + Clone>(app: &AppHandle, label: &str, event: &str, payload: S) {
    let _ = app.emit_to(label, event, payload);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agentnotch_engine::core::flags::DevFlags;
    use agentnotch_engine::hub::{Hub, HubConfig};
    use agentnotch_engine::model::UpstreamUsage;
    use agentnotch_engine::platform::Roots;

    use super::upstream_snapshot;

    fn fixture_hub() -> Hub {
        let root = std::env::temp_dir().join("agentnotch-glue-tests-never-created");
        let config = HubConfig {
            roots: Roots::under(&root),
            app_version: "1.1.0".into(),
            website: None,
            flags: DevFlags {
                sealed: true,
                ..DevFlags::default()
            },
            hook_exe: root.join("agentnotch-hook.exe"),
            pipe_name: String::new(),
        };
        Hub::sealed(config, Arc::new(agentnotch_win::SystemClock))
    }

    #[test]
    fn the_projection_copies_the_hubs_reading() {
        let usage = fixture_hub().upstream_usage();
        let shown = upstream_snapshot(&usage);
        assert_ne!(shown.status, "needsAuth");
        assert_eq!(shown.status, usage.status);
        assert_eq!(shown.windows.len(), usage.windows.len());
        assert!(!shown.windows.is_empty(), "the fixture has readings");
        for (shown, given) in shown.windows.iter().zip(&usage.windows) {
            assert_eq!(shown.id, given.id);
            assert_eq!(shown.used, given.used);
            assert_eq!(shown.group, given.group);
        }
        assert_eq!(shown.fetched_at, usage.fetched_at);
        assert_eq!(shown.note, usage.note);
    }

    #[test]
    fn the_projection_never_says_needs_auth() {
        // Upstream's pages answer `needsAuth` with a "Sign in" button (Claude's browser login).
        for hostile in [
            "needsAuth",
            "needsauth",
            "NEEDSAUTH",
            "needs_auth",
            " needs-auth ",
        ] {
            let usage = UpstreamUsage {
                status: hostile.into(),
                note: "Not signed in to Claude. Run claude, then /login.".into(),
                ..UpstreamUsage::default()
            };
            let shown = upstream_snapshot(&usage);
            assert_eq!(shown.status, "none", "{hostile:?}");
            assert_eq!(shown.note, usage.note);
        }
        // The engine's own statuses pass unchanged.
        for status in ["ok", "stale", "none", "error"] {
            let usage = UpstreamUsage {
                status: status.into(),
                ..UpstreamUsage::default()
            };
            assert_eq!(upstream_snapshot(&usage).status, status);
        }
    }
}
