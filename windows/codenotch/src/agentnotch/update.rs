//! Updates (seam WUP2, DESIGN-WIN §2.4 `update.rs`, §4.15).
//!
//! Upstream's updater UI and flow stay as they are; the fork only changes what happens right
//! before the installer takes over. The updater calls `std::process::exit(0)` as soon as it has
//! started the installer, so Tauri's exit events never fire: without the hook below, review
//! marks, usage and ledger writes still being debounced would be lost, and held PermissionRequests
//! would end only when the process died. The hub is stopped first (stores saved, held requests
//! closed without an answer so each hook exits 0 and Claude Code's own prompt decides, children
//! killed), then upstream's default cleanup runs.
//!
//! A cold-start deep link in argv would be replayed into the restarted app by the updater's
//! `/ARGS`; the plugin keeps that argument list to itself (`current_exe_args` is crate-private in
//! tauri-plugin-updater 2.12.0), so the engine's stale-callback rule is what ignores it (§4.11).

use tauri::AppHandle;
use tauri_plugin_updater::{Error, Updater, UpdaterExt};

/// Upstream's `app.updater()`, with the hub stopped before the installer runs.
pub fn updater(app: &AppHandle) -> Result<Updater, Error> {
    let exiting = app.clone();
    app.updater_builder()
        .on_before_exit(move || {
            if let Some(hub) = super::hub() {
                hub.stop();
            }
            exiting.cleanup_before_exit();
        })
        .build()
}

/// The doctor's `updates:` line (§4.14). Only release builds carry an update key (the release
/// workflow's config overlay); a copy built from source never updates itself.
pub(super) fn doctor_line() -> String {
    let config = merged_updater_config();
    let pubkey = config.get("pubkey").and_then(|v| v.as_str()).unwrap_or("");
    if pubkey.is_empty() || super::sealed() {
        return "updates: off (built from source)".into();
    }
    let feed = config
        .get("endpoints")
        .and_then(|v| v.get(0))
        .and_then(|v| v.as_str())
        .unwrap_or("none");
    let signed_version = config
        .get("requireSignedVersion")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // The key id is decoded from the key by the release tooling's rules (WP11); the doctor shows
    // the feed and whether the signed version is enforced.
    format!(
        "updates: on feed={feed} signed-version={}",
        if signed_version {
            "required"
        } else {
            "not required"
        }
    )
}

/// `plugins.updater` as this binary was built: the source `tauri.conf.json`, overlaid with the
/// `--config` the build passed (the Tauri CLI hands it to the compiler as `TAURI_CONFIG`; Tauri's
/// own build script reruns when it changes, and so does this crate, through `option_env!`).
fn merged_updater_config() -> serde_json::Map<String, serde_json::Value> {
    let updater = |text: &str| {
        serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v.get("plugins")?.get("updater")?.as_object().cloned())
            .unwrap_or_default()
    };
    let mut merged = updater(include_str!("../../tauri.conf.json"));
    if let Some(overlay) = option_env!("TAURI_CONFIG") {
        merged.extend(updater(overlay));
    }
    merged
}
