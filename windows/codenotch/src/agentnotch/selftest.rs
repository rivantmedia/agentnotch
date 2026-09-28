//! Sealed-only launch switches (DESIGN-WIN §2.4 `selftest.rs`, §4.13, §7.4).
//!
//! - `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH=sessions|session:<id>|settings` opens that view once the
//!   app is up, for looking at the fixture UI;
//! - `AGENTNOTCH_PANEL_SELF_TEST=1` (+ `AGENTNOTCH_SELF_TEST_OUT=<json>`) and
//!   `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>` are the smoke test's layout self-test and snapshots. They
//!   come with the panel's package (WP9); until then a run that asks for them writes a failing
//!   report and exits 1, so no check can mistake a missing self-test for a passing one.
//!
//! None of them does anything unless the run is sealed.

use std::time::Duration;

use tauri::AppHandle;

pub(super) fn start(app: &AppHandle) {
    if !super::sealed() {
        return;
    }
    if let Some(route) = env("AGENTNOTCH_OPEN_PANEL_ON_LAUNCH") {
        let app = app.clone();
        std::thread::spawn(move || {
            // After the notch's first frame, so the view opens over a settled desktop.
            std::thread::sleep(Duration::from_secs(1));
            if route == "settings" {
                crate::settings_window::open(&app);
            } else {
                super::panel::open_route(&app, route, None, "settings".into());
            }
        });
    }
    let self_test = env("AGENTNOTCH_PANEL_SELF_TEST").is_some_and(|v| v != "0");
    let snapshots = env("AGENTNOTCH_SNAPSHOT_CLAUDE").is_some();
    if self_test || snapshots {
        let reason = "the sealed self-test and snapshots aren't available in this build";
        if let Some(out) = env("AGENTNOTCH_SELF_TEST_OUT") {
            let report = serde_json::json!({ "ok": false, "error": reason });
            let _ = std::fs::write(out, report.to_string());
        }
        super::log(reason);
        app.exit(1);
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
