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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use agentnotch_engine::hub::Hub;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Error, Updater, UpdaterExt};

/// The hook below stopped the hub for an installer that has not started yet.
static STOPPED_FOR_UPDATE: AtomicBool = AtomicBool::new(false);
/// The windows that were showing when the hook's cleanup hid them all.
static SHOWN_BEFORE_UPDATE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Upstream's `app.updater()`, with the hub stopped before the installer runs.
pub fn updater(app: &AppHandle) -> Result<Updater, Error> {
    let exiting = app.clone();
    app.updater_builder()
        .on_before_exit(move || {
            stop_for_update(super::hub().as_ref(), &STOPPED_FOR_UPDATE);
            let shown: Vec<String> = exiting
                .webview_windows()
                .into_iter()
                .filter(|(_, w)| w.is_visible().unwrap_or(false))
                .map(|(label, _)| label)
                .collect();
            *lock(&SHOWN_BEFORE_UPDATE) = shown;
            exiting.cleanup_before_exit();
        })
        .build()
}

/// The installer did not start after the hook ran (tauri-plugin-updater 2.12.0 runs the hook,
/// then `ShellExecuteW`, and returns its error instead of exiting: a declined UAC prompt,
/// AppLocker, an antivirus that blocks the unsigned installer in %TEMP%). The app keeps running,
/// so what the hook undid comes back: the hub (without it hooks find no app and Claude Code
/// control, usage and sync stay frozen until a relaunch), and what `cleanup_before_exit` took
/// away (it drops the tray icon and hides every window). Resources the pages held in Tauri's
/// resource tables are not restored; the fork's pages keep none.
pub fn install_failed(app: &AppHandle) {
    match restart_after_failed_install(super::hub().as_ref(), &STOPPED_FOR_UPDATE) {
        Some(Ok(())) => super::log("updater: the installer didn't start; the hub runs again"),
        Some(Err(e)) => super::log(&format!(
            "updater: the installer didn't start; the hub didn't start again ({e})"
        )),
        None => {}
    }
    let shown = std::mem::take(&mut *lock(&SHOWN_BEFORE_UPDATE));
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if handle.tray_by_id("main").is_none() {
            match crate::tray::setup(&handle) {
                Ok(()) => super::tray::rebuilt(&handle),
                Err(e) => super::log(&format!("updater: the tray didn't come back ({e})")),
            }
        }
        for label in shown {
            if let Some(window) = handle.get_webview_window(&label) {
                let _ = window.show();
            }
        }
    });
}

/// The hook's half: stops the hub and remembers that it did.
fn stop_for_update(hub: Option<&Hub>, stopped: &AtomicBool) {
    if let Some(hub) = hub {
        hub.stop();
        stopped.store(true, Ordering::SeqCst);
    }
}

/// The failed install's half: starts the hub again when the hook stopped it (`None`: it didn't).
fn restart_after_failed_install(
    hub: Option<&Hub>,
    stopped: &AtomicBool,
) -> Option<Result<(), String>> {
    if !stopped.swap(false, Ordering::SeqCst) {
        return None;
    }
    Some(hub?.start())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The doctor's `updates:` line (§4.14). Only release builds carry an update key (the release
/// workflow's config overlay); a copy built from source never updates itself.
pub(super) fn doctor_line() -> String {
    updates_line(&merged_updater_config(), super::sealed())
}

/// The line for a given `plugins.updater` config. Pure, so the format the release's smoke test
/// matches (`^updates: on .*key=<id> signed-version=required`) is tested on any OS.
fn updates_line(config: &serde_json::Map<String, serde_json::Value>, sealed: bool) -> String {
    let pubkey = config.get("pubkey").and_then(|v| v.as_str()).unwrap_or("");
    if pubkey.is_empty() || sealed {
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
    format!(
        "updates: on feed={feed} key={} signed-version={}",
        key_id(pubkey),
        if signed_version {
            "required"
        } else {
            "not required"
        }
    )
}

/// The key id the build's update key names, decoded by the release tool's own function (the one
/// release.yml checks the published key and feed with), so the doctor can never disagree with the
/// release about which key a build trusts. A key the updater could not use reads `invalid`, which
/// no smoke test's key pattern (16 upper-case hex digits) matches: a broken release build fails
/// there instead of shipping an app that refuses every update.
fn key_id(pubkey: &str) -> String {
    // The updater decodes the value with a strict base64 decoder and no trimming
    // (tauri-plugin-updater's `base64_to_string`); the release tool is more lenient (it trims and
    // also takes a bare minisign `RW…` line), so what only it would accept is refused here first.
    if pubkey.trim() != pubkey || pubkey.starts_with("RW") {
        return "invalid".into();
    }
    match agentnotch_release::key_id_of_pubkey(pubkey) {
        Ok(id) => id.to_string(),
        Err(_) => "invalid".into(),
    }
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use agentnotch_engine::core::flags::DevFlags;
    use agentnotch_engine::hub::{Hub, HubConfig, HubEvent};
    use agentnotch_engine::platform::Roots;

    use super::{key_id, restart_after_failed_install, stop_for_update, updates_line};

    /// A sealed hub that counts its starts.
    fn counted_hub() -> (Hub, Arc<Mutex<u32>>) {
        let root = std::env::temp_dir().join("agentnotch-glue-tests-never-created");
        let hub = Hub::sealed(
            HubConfig {
                roots: Roots::under(&root),
                app_version: "1.1.0".into(),
                website: None,
                flags: DevFlags {
                    sealed: true,
                    ..DevFlags::default()
                },
                hook_exe: root.join("agentnotch-hook.exe"),
                pipe_name: String::new(),
            },
            Arc::new(agentnotch_win::SystemClock),
        );
        let starts = Arc::new(Mutex::new(0));
        let seen = starts.clone();
        hub.on_event(Box::new(move |event: &HubEvent| {
            if matches!(event, HubEvent::Log(line) if line.starts_with("hub started")) {
                *seen.lock().unwrap() += 1;
            }
        }));
        (hub, starts)
    }

    #[test]
    fn a_hub_stopped_for_an_installer_that_never_ran_starts_again() {
        let (hub, starts) = counted_hub();
        hub.start().unwrap();
        let stopped = AtomicBool::new(false);
        stop_for_update(Some(&hub), &stopped);
        assert!(stopped.load(Ordering::SeqCst));
        assert_eq!(
            restart_after_failed_install(Some(&hub), &stopped),
            Some(Ok(()))
        );
        assert_eq!(*starts.lock().unwrap(), 2);
        // Once: a second failure report starts nothing.
        assert_eq!(restart_after_failed_install(Some(&hub), &stopped), None);
        assert_eq!(*starts.lock().unwrap(), 2);
    }

    #[test]
    fn nothing_starts_when_the_hook_never_stopped_the_hub() {
        // A failure before the download finished (a bad signature, no network): the hook never
        // ran, so a hub that is stopped for another reason stays stopped.
        let (hub, starts) = counted_hub();
        let stopped = AtomicBool::new(false);
        assert_eq!(restart_after_failed_install(Some(&hub), &stopped), None);
        assert_eq!(*starts.lock().unwrap(), 0);
        stop_for_update(None, &stopped);
        assert!(!stopped.load(Ordering::SeqCst), "no hub, nothing stopped");
    }

    const FEED: &str =
        "https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json";

    /// `plugins.updater` as agentnotch-build.ps1's release overlay writes it.
    fn release_config(pubkey: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "pubkey": pubkey,
            "endpoints": [FEED],
            "requireSignedVersion": true,
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    /// agentnotch-smoke.ps1's `Test-DoctorReport` pattern for the updates line, as a release run
    /// (`-Updates on -KeyId <id>`) and a run without `-KeyId` build it.
    fn smoke_pattern(key_id: Option<&str>) -> regex::Regex {
        let key = key_id.map_or_else(|| "[0-9A-F]{16}".to_string(), regex::escape);
        regex::Regex::new(&format!(
            "(?m)^updates: on .*key={key} signed-version=required"
        ))
        .unwrap()
    }

    #[test]
    fn the_design_vector_shows_its_key_id() {
        // DESIGN-WIN Appendix B's pubkey (a test seed, never a real key) and §4.14's line.
        let pubkey = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI1QTU2MzgzNjFGQkQwMTkKUldRWjBQdGhnMk9sdGN3UFAralJPTnNJUnh6bjFOZUVGamloRlNMUWZTako4Qzk4RlRGZi9qdjQK";
        let line = updates_line(&release_config(pubkey), false);
        assert_eq!(
            line,
            format!("updates: on feed={FEED} key=B5A5638361FBD019 signed-version=required")
        );
        assert!(smoke_pattern(Some("B5A5638361FBD019")).is_match(&line));
        assert!(smoke_pattern(None).is_match(&line));
        assert!(!smoke_pattern(Some("0000000000000000")).is_match(&line));
    }

    #[test]
    fn keys_made_by_the_release_tool_show_the_id_it_derived() {
        // The id is compared with the one `UpdateKey::derive` produced (HKDF output), not with
        // a second decoding: what release.yml passes as expected_key_id.
        for seed in [
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
            "//////////////////////////////////////////8=",
            "q83vEjRWeJCrze8SNFZ4kKvN7xI0VniQq83vEjRWeJA=",
        ] {
            let seed = agentnotch_release::Seed::from_base64(seed.as_bytes()).unwrap();
            let key = agentnotch_release::UpdateKey::derive(&seed);
            let id = key.key_id().to_string();
            assert_eq!(key_id(&key.tauri_pubkey()), id);
            let line = updates_line(&release_config(&key.tauri_pubkey()), false);
            assert!(smoke_pattern(Some(&id)).is_match(&line), "{line}");
            assert!(smoke_pattern(None).is_match(&line), "{line}");
        }
    }

    #[test]
    fn a_key_the_updater_could_not_use_never_passes_the_smoke_test() {
        let key = agentnotch_release::UpdateKey::derive(
            &agentnotch_release::Seed::from_base64(b"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=")
                .unwrap(),
        );
        use base64::Engine as _;
        let b64 = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
        let short_blob = b64("untrusted comment: minisign public key: 0\nRWQZ0Pthg2Ol\n");
        for pubkey in [
            "not base64 at all".to_string(),
            format!("{}\n", key.tauri_pubkey()),
            format!(" {}", key.tauri_pubkey()),
            key.minisign_public_key(),
            b64("hello"),
            b64("untrusted comment: minisign public key: 0\n"),
            short_blob,
        ] {
            assert_eq!(key_id(&pubkey), "invalid", "{pubkey:?}");
            let line = updates_line(&release_config(&pubkey), false);
            assert!(line.contains(" key=invalid "), "{line}");
            assert!(!smoke_pattern(None).is_match(&line), "{line}");
            assert!(
                !smoke_pattern(Some("B5A5638361FBD019")).is_match(&line),
                "{line}"
            );
        }
    }

    #[test]
    fn no_key_or_a_sealed_run_means_updates_off() {
        let key_text = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI1QTU2MzgzNjFGQkQwMTkKUldRWjBQdGhnMk9sdGN3UFAralJPTnNJUnh6bjFOZUVGamloRlNMUWZTako4Qzk4RlRGZi9qdjQK";
        let off = "updates: off (built from source)";
        assert_eq!(updates_line(&serde_json::Map::new(), false), off);
        assert_eq!(updates_line(&release_config(""), false), off);
        assert_eq!(updates_line(&release_config(key_text), true), off);
        let mut unsigned = release_config(key_text);
        unsigned.insert("requireSignedVersion".into(), false.into());
        let line = updates_line(&unsigned, false);
        assert!(line.ends_with("key=B5A5638361FBD019 signed-version=not required"));
        assert!(!smoke_pattern(Some("B5A5638361FBD019")).is_match(&line));
    }
}
