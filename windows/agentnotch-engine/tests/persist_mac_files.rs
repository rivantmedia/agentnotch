//! Every persisted file round-trips through its `persist::*` types: a
//! Mac-written fixture (`tests/fixtures/mac-files/`) is read and written back
//! JSON-equivalent, and the model conversions keep the values.

use agentnotch_engine::core::settings::ControlSettings;
use agentnotch_engine::core::time;
use agentnotch_engine::model::{UsageSource, UsageWindow};
use agentnotch_engine::persist::accounts::AccountsFile;
use agentnotch_engine::persist::hook_install::HookInstallFile;
use agentnotch_engine::persist::json_equivalent;
use agentnotch_engine::persist::review::ReviewStateFile;
use agentnotch_engine::persist::settings::SettingsFile;
use agentnotch_engine::persist::usage::{PersistedAccountUsage, UsageStateFile};
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mac-files")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_equivalent(original: &[u8], written: &[u8], name: &str) {
    let a: Value = serde_json::from_slice(original).unwrap();
    let b: Value = serde_json::from_slice(written).unwrap();
    assert!(
        json_equivalent(&a, &b),
        "{name} changed:\n{a:#}\n---\n{b:#}"
    );
}

#[test]
fn accounts_json() {
    let bytes = fixture("accounts.json");
    let file = AccountsFile::parse(&bytes).expect("parses");
    assert_eq!(file.version, 2);
    assert_eq!(file.accounts.len(), 3);
    assert_eq!(file.accounts[1].custom_label.as_deref(), Some("Work"));
    assert_eq!(
        file.accounts[0]
            .last_seen_at
            .map(|d| time::iso8601(d.0))
            .as_deref(),
        Some("2026-09-20T08:15:00Z")
    );
    let timeline = file.default_identity_timeline.as_ref().unwrap();
    assert!(timeline.spans[1].is_login);
    assert_equivalent(&bytes, &file.encode(), "accounts.json");
    assert!(AccountsFile::parse(b"{\"accounts\": 3}").is_none());
}

#[test]
fn usage_state_json() {
    let bytes = fixture("usage-state.json");
    let file = UsageStateFile::parse(&bytes).expect("parses");
    assert_equivalent(&bytes, &file.encode(), "usage-state.json");

    let personal = &file.accounts["uuid:5f0c3a1e-0000-4000-8000-000000000001"];
    let usage = personal.last_full_reading.as_ref().unwrap().to_model();
    assert_eq!(usage.source, UsageSource::Probe);
    assert_eq!(
        usage.five_hour,
        Some(UsageWindow {
            utilization: 34.0,
            resets_at: time::parse_iso8601("2026-09-21T16:23:20Z"),
            duration_s: UsageWindow::SESSION_DURATION_S
        })
    );
    assert_eq!(usage.scoped[0].0, "Opus");
    assert_eq!(
        usage.extra_usage.as_ref().unwrap().used_credits,
        Some(1234.0)
    );
    // The model converts back to the same record.
    let back = PersistedAccountUsage::from_model(&usage);
    assert_eq!(&back, personal.last_full_reading.as_ref().unwrap());

    let line = &personal.status_lines.as_ref().unwrap()[0];
    assert_eq!(line.key, "pid:48213@1790000000");
    let reading = line.readings.five_hour.as_ref().unwrap().to_model();
    assert_eq!(reading.window.utilization, 35.0);
    assert!(reading.not_before.is_some());

    let work = &file.accounts["uuid:8a7b6c5d-0000-4000-8000-000000000002/org-work-0002"];
    assert_eq!(work.failure_count, 2);
    assert_eq!(
        work.last_full_reading.as_ref().unwrap().to_model().source,
        UsageSource::Cache
    );

    // A newer version is only a cache: start fresh.
    assert!(UsageStateFile::parse(br#"{"version": 2, "accounts": {}}"#).is_none());
}

#[test]
fn review_state_json() {
    let bytes = fixture("review-state.json");
    let file = ReviewStateFile::parse(&bytes).expect("parses");
    assert_eq!(file.version, 2);
    // Compact, sorted keys, the very numbers read: byte for byte.
    assert_eq!(
        String::from_utf8(file.encode()).unwrap(),
        String::from_utf8(bytes.clone()).unwrap()
    );

    let waiting = file.sessions["5d1e0a7b-3c21-4f7e-9a0b-1c2d3e4f5a6b"]
        .to_model()
        .unwrap();
    assert_eq!(waiting.background_agent_types, ["subagent", "workflow"]);
    assert_eq!(
        time::to_ms(waiting.completed_at.unwrap()),
        1_789_999_000_125
    );
    let failed = file.sessions["9f8e7d6c-5b4a-4321-8fed-cba987654321"]
        .to_model()
        .unwrap();
    assert_eq!(failed.stop_error_code.as_deref(), Some("rate_limit"));
    assert!(failed.background_agent_types.is_empty());
}

#[test]
fn superpowered_vibe_notchs_bare_review_file_is_read() {
    let file = ReviewStateFile::parse(&fixture("review-state-spvn.json")).expect("parses");
    assert_eq!(file.last_alive_at, None);
    assert_eq!(file.sessions.len(), 1);
    assert!(ReviewStateFile::parse(b"[1]").is_none());
}

#[test]
fn control_settings_json() {
    let bytes = fixture("control-settings.json");
    let file = SettingsFile::parse(&bytes).expect("parses");
    let settings = file.settings();
    assert_eq!(settings.hook_consent, Some(true));
    assert_eq!(settings.auto_open, "needsInput");
    assert_eq!(settings.hot_key, "ctrlAltSpace");
    assert_eq!(settings.peek_seconds, 10);
    assert_eq!(settings.usage_probe_interval_minutes, 10);
    assert!(!settings.sound && !settings.notify_ready_for_review && settings.cloud_sync_enabled);
    assert_eq!(
        settings.cloud_device_id.as_deref(),
        Some("3F2504E0-4F89-41D3-9A0C-0305E82C3301")
    );

    // Written back with every setting: the unknown key survives.
    let mut written = file.clone();
    written.apply(&settings);
    assert_equivalent(&bytes, &written.encode(), "control-settings.json");
}

#[test]
fn bad_settings_fall_back_one_by_one() {
    let file = SettingsFile::parse(
        br#"{"version": 1, "autoOpen": "sometimes", "peekSeconds": 7, "hooksEnabled": "yes", "hookConsent": false,
            "usageProbeIntervalMinutes": -5, "ringClick": "refreshUsage", "claudeBinaryPath": ""}"#,
    )
    .unwrap();
    let settings = file.settings();
    let defaults = ControlSettings::default();
    assert_eq!(settings.auto_open, defaults.auto_open);
    assert_eq!(settings.peek_seconds, defaults.peek_seconds);
    assert_eq!(settings.hooks_enabled, defaults.hooks_enabled);
    assert_eq!(
        settings.usage_probe_interval_minutes,
        defaults.usage_probe_interval_minutes
    );
    assert_eq!(settings.claude_binary_path, None);
    // The good ones are kept.
    assert_eq!(settings.hook_consent, Some(false));
    assert_eq!(settings.ring_click, "refreshUsage");
    assert!(SettingsFile::parse(b"[]").is_none());
    assert_eq!(SettingsFile::default().settings(), defaults);
}

#[test]
fn hook_install_json() {
    let bytes = fixture("hook-install.json");
    let file = HookInstallFile::parse(&bytes).expect("parses");
    assert_equivalent(&bytes, &file.encode(), "hook-install.json");
    let record = file.to_model();
    assert_eq!(record.files.len(), 2);
    assert_eq!(
        record.files[1].folder.as_str(),
        r"C:\Users\John Smith\.claude-work"
    );
    assert!(!record.files[1].status_line);
    assert_eq!(HookInstallFile::from_model(&record), file);
}
