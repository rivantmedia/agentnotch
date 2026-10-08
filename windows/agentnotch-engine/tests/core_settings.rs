//! The engine's settings (§4.12): defaults, per-key fallbacks of the file,
//! unknown keys kept on save, and the rules `Call::SetSetting` is held to.

use agentnotch_engine::core::settings::{
    keys, ControlSettings, SettingError, MAX_PROBE_INTERVAL_MINUTES, PAGE_KEYS,
};
use agentnotch_engine::persist::settings::SettingsFile;
use serde_json::{json, Value};
use std::time::Duration;

fn file(json: Value) -> SettingsFile {
    SettingsFile::parse(json.to_string().as_bytes()).expect("an object")
}

/// One vector per key: its default as the file shows it, a valid value that
/// differs from it, and a value the file must not keep.
fn vectors() -> Vec<(&'static str, Value, Value, Vec<Value>)> {
    vec![
        (
            keys::HOOK_CONSENT,
            Value::Null,
            json!(true),
            vec![json!("yes"), json!(1)],
        ),
        (
            keys::HOOK_CONSENT_SCOPE,
            json!(2),
            json!(0),
            vec![json!(-1), json!("2"), json!(1.5), json!(1u64 << 40)],
        ),
        (
            keys::HOOKS_ENABLED,
            json!(true),
            json!(false),
            vec![json!("no"), Value::Null],
        ),
        (
            keys::STATUS_LINE_INTEGRATION,
            json!(true),
            json!(false),
            vec![json!(0)],
        ),
        (
            keys::USAGE_PROBE_INTERVAL_MINUTES,
            json!(5),
            json!(30),
            vec![
                json!(-5),
                json!(MAX_PROBE_INTERVAL_MINUTES + 1),
                json!("5"),
                json!(2.5),
            ],
        ),
        (
            keys::READS_DESKTOP_USAGE_CACHE,
            json!(true),
            json!(false),
            vec![json!("true")],
        ),
        (
            keys::CLAUDE_BINARY_PATH,
            Value::Null,
            json!("C:\\Tools\\claude.exe"),
            vec![json!(7), json!(["x"])],
        ),
        (
            keys::NOTIFY_NEEDS_INPUT,
            json!(true),
            json!(false),
            vec![json!(null)],
        ),
        (
            keys::NOTIFY_READY_FOR_REVIEW,
            json!(true),
            json!(false),
            vec![json!("off")],
        ),
        (
            keys::AUTO_OPEN,
            json!("never"),
            json!("needsInputOrDone"),
            vec![json!("sometimes"), json!(true)],
        ),
        (
            keys::HOLD_OPEN_WHILE_NEEDS_YOU,
            json!("never"),
            json!("always"),
            vec![json!("auto"), json!(1)],
        ),
        (
            keys::RING_BADGES,
            json!(true),
            json!(false),
            vec![json!("on")],
        ),
        (
            keys::RESTING_MARKS,
            json!(true),
            json!(false),
            vec![json!(2)],
        ),
        (keys::TRAY_BADGE, json!(true), json!(false), vec![json!([])]),
        (
            keys::RING_CLICK,
            json!("openPanel"),
            json!("refreshUsage"),
            vec![json!("refresh")],
        ),
        (
            keys::SESSION_CLICK,
            json!("smart"),
            json!("terminal"),
            vec![json!("popup")],
        ),
        (
            keys::HOT_KEY,
            json!("off"),
            json!("ctrlAltJ"),
            vec![json!("optionCommandJ")],
        ),
        (
            keys::PANEL_PINNED,
            json!(false),
            json!(true),
            vec![json!("pinned")],
        ),
        (keys::SOUND, json!(true), json!(false), vec![json!(0)]),
        (keys::PEEK, json!(true), json!(false), vec![json!({})]),
        (
            keys::PEEK_SECONDS,
            json!(5),
            json!(10),
            vec![json!(7), json!(0), json!("5"), json!(3.5)],
        ),
        (
            keys::CLOUD_SYNC_ENABLED,
            json!(false),
            json!(true),
            vec![json!("on")],
        ),
        (
            keys::CLOUD_SUMMARIES_ENABLED,
            json!(false),
            json!(true),
            vec![json!(1)],
        ),
        (
            keys::CLOUD_DEVICE_ID,
            Value::Null,
            json!("0F3A5C1E-7B2D-4E96-8A10-5D4C3B2A1908"),
            vec![
                json!("device-1"),
                json!(5),
                json!("0F3A5C1E7B2D4E968A105D4C3B2A1908"),
            ],
        ),
        (
            keys::TYPE_REPLIES,
            json!(false),
            json!(true),
            vec![json!("true")],
        ),
    ]
}

#[test]
fn every_key_has_a_vector() {
    let covered: Vec<&str> = vectors().iter().map(|v| v.0).collect();
    assert_eq!(covered, keys::ALL.to_vec());
}

#[test]
fn the_defaults_are_the_windows_ones() {
    let s = ControlSettings::default();
    assert_eq!(s.hook_consent, None);
    assert_eq!(s.hook_consent_scope, 2);
    assert!(s.hooks_enabled && s.status_line_integration && s.reads_desktop_usage_cache);
    assert_eq!(s.usage_probe_interval_minutes, 5);
    assert!(s.notify_needs_input && s.notify_ready_for_review);
    assert_eq!(s.auto_open, "never");
    assert_eq!(s.hold_open_while_needs_you, "never");
    assert!(s.ring_badges && s.resting_marks && s.tray_badge);
    assert_eq!(
        (
            s.ring_click.as_str(),
            s.session_click.as_str(),
            s.hot_key.as_str()
        ),
        ("openPanel", "smart", "off")
    );
    assert!(!s.panel_pinned && s.sound && s.peek);
    assert_eq!(s.peek_seconds, 5);
    assert!(!s.cloud_sync_enabled && !s.cloud_summaries_enabled && !s.type_replies);
    assert_eq!(s.cloud_device_id, None);
    assert_eq!(s.claude_binary_path, None);
}

#[test]
fn an_empty_file_reads_as_the_defaults_and_writes_every_key() {
    let settings = SettingsFile::default().settings();
    assert_eq!(settings, ControlSettings::default());
    let mut written = SettingsFile::default();
    written.apply(&settings);
    for (key, default, _, _) in vectors() {
        assert_eq!(written.values.get(key), Some(&default), "{key}");
    }
    assert_eq!(written.values.get("version"), Some(&json!(1)));
}

#[test]
fn a_valid_value_is_read_and_written_back() {
    for (key, _, valid, _) in vectors() {
        let read = file(json!({ key: valid.clone() })).settings();
        let mut back = SettingsFile::default();
        back.apply(&read);
        assert_eq!(back.values.get(key), Some(&valid), "{key}");
        assert_ne!(read, ControlSettings::default(), "{key} changed nothing");
    }
}

#[test]
fn a_bad_value_falls_back_to_its_default_alone() {
    for (key, default, _, bad) in vectors() {
        // Every other key holds a valid, non-default value; only `key` is bad.
        for bad in bad {
            let mut object = serde_json::Map::new();
            for (other, _, valid, _) in vectors() {
                if other != key {
                    object.insert(other.to_owned(), valid);
                }
            }
            object.insert(key.to_owned(), bad.clone());
            let read = SettingsFile::parse(Value::Object(object).to_string().as_bytes())
                .unwrap()
                .settings();
            let mut back = SettingsFile::default();
            back.apply(&read);
            assert_eq!(back.values.get(key), Some(&default), "{key} = {bad}");
            for (other, _, valid, _) in vectors() {
                if other != key {
                    assert_eq!(
                        back.values.get(other),
                        Some(&valid),
                        "{other} beside bad {key}"
                    );
                }
            }
        }
    }
}

#[test]
fn unknown_keys_survive_a_save() {
    let mut f =
        file(json!({ "autoOpen": "needsInput", "futureSwitch": {"a": [1, 2]}, "version": 1 }));
    let mut settings = f.settings();
    assert_eq!(settings.auto_open, "needsInput");
    settings.sound = false;
    f.apply(&settings);
    assert_eq!(f.values["futureSwitch"], json!({"a": [1, 2]}));
    assert_eq!(f.values["sound"], json!(false));
    let again = SettingsFile::parse(&f.encode()).unwrap();
    assert_eq!(again.values["futureSwitch"], json!({"a": [1, 2]}));
    assert_eq!(again.settings(), settings);
}

#[test]
fn a_file_that_is_not_an_object_is_not_a_file() {
    assert!(SettingsFile::parse(b"[1]").is_none());
    assert!(SettingsFile::parse(b"not json").is_none());
}

#[test]
fn an_empty_binary_path_is_none() {
    assert_eq!(
        file(json!({"claudeBinaryPath": ""}))
            .settings()
            .claude_binary_path,
        None
    );
}

#[test]
fn set_setting_takes_a_valid_value_for_every_key() {
    for (key, _, valid, _) in vectors() {
        let next = ControlSettings::default()
            .validated(key, &valid)
            .unwrap_or_else(|e| panic!("{key}: {e}"));
        assert_ne!(next, ControlSettings::default(), "{key}");
    }
}

#[test]
fn set_setting_refuses_a_bad_value_and_changes_nothing() {
    let start = ControlSettings::default();
    for (key, _, _, bad) in vectors() {
        for bad in bad {
            let error = start
                .validated(key, &bad)
                .expect_err(&format!("{key} = {bad}"));
            assert_eq!(error.code, "invalid", "{key} = {bad}");
            assert!(error.message.contains(key), "{}", error.message);
        }
    }
}

#[test]
fn set_setting_refuses_unknown_keys() {
    let start = ControlSettings::default();
    for key in ["noSuchSetting", "version", "", "AutoOpen", "autoopen"] {
        let error = start.validated(key, &json!(true)).unwrap_err();
        assert_eq!(error.code, "invalid", "{key}");
        assert!(
            error.message.starts_with("Unknown setting"),
            "{}",
            error.message
        );
    }
    let mut copy = start.clone();
    assert_eq!(
        copy.set_value("nope", &json!(1)),
        Err(SettingError::Unknown)
    );
    assert_eq!(copy, start);
}

#[test]
fn a_long_key_is_cut_in_the_message() {
    let key = "k".repeat(5000);
    let error = ControlSettings::default()
        .validated(&key, &json!(1))
        .unwrap_err();
    assert!(error.message.len() < 100, "{}", error.message.len());
}

#[test]
fn the_probe_interval_rule() {
    let start = ControlSettings::default();
    let set = |v: Value| start.validated(keys::USAGE_PROBE_INTERVAL_MINUTES, &v);
    assert_eq!(set(json!(0)).unwrap().probe_interval(), None);
    // Under five minutes the effective interval is still 300 s.
    assert_eq!(
        set(json!(1)).unwrap().probe_interval(),
        Some(Duration::from_secs(300))
    );
    assert_eq!(
        set(json!(30)).unwrap().probe_interval(),
        Some(Duration::from_secs(1800))
    );
    assert!(set(json!(MAX_PROBE_INTERVAL_MINUTES)).is_ok());
    for bad in [
        json!(MAX_PROBE_INTERVAL_MINUTES + 1),
        json!(-1),
        json!(u64::MAX),
        json!(1e3),
        json!(null),
    ] {
        assert_eq!(set(bad.clone()).unwrap_err().code, "invalid", "{bad}");
    }
}

#[test]
fn peek_seconds_is_three_five_or_ten() {
    let start = ControlSettings::default();
    for ok in [3, 5, 10] {
        assert_eq!(
            start
                .validated(keys::PEEK_SECONDS, &json!(ok))
                .unwrap()
                .peek_seconds,
            ok
        );
    }
    for bad in [
        json!(0),
        json!(4),
        json!(7),
        json!(11),
        json!(-3),
        json!("5"),
        json!(5.5),
    ] {
        assert_eq!(
            start.validated(keys::PEEK_SECONDS, &bad).unwrap_err().code,
            "invalid",
            "{bad}"
        );
    }
}

#[test]
fn consent_can_be_unanswered_again() {
    let granted = ControlSettings::default()
        .validated(keys::HOOK_CONSENT, &json!(true))
        .unwrap();
    assert!(granted.hooks_active());
    let reset = granted.validated(keys::HOOK_CONSENT, &Value::Null).unwrap();
    assert_eq!(reset.hook_consent, None);
    assert!(!reset.hooks_active());
}

#[test]
fn a_page_sets_only_its_own_keys() {
    let start = ControlSettings::default();
    for key in PAGE_KEYS {
        let (_, _, valid, _) = vectors().into_iter().find(|v| v.0 == key).unwrap();
        assert!(start.validated_from_page(key, &valid).is_ok(), "{key}");
    }
    // Consent, hooks, the status line, the binary and the cloud switches
    // have their own calls (the pages' contract); the cloud thread's
    // `Input::SetSetting` takes them through `validated`.
    for key in [
        keys::HOOK_CONSENT,
        keys::HOOK_CONSENT_SCOPE,
        keys::HOOKS_ENABLED,
        keys::STATUS_LINE_INTEGRATION,
        keys::CLAUDE_BINARY_PATH,
        keys::CLOUD_SYNC_ENABLED,
        keys::CLOUD_SUMMARIES_ENABLED,
        keys::CLOUD_DEVICE_ID,
    ] {
        let (_, _, valid, _) = vectors().into_iter().find(|v| v.0 == key).unwrap();
        let error = start.validated_from_page(key, &valid).unwrap_err();
        assert_eq!(error.code, "invalid", "{key}");
        assert!(start.validated(key, &valid).is_ok(), "{key}");
    }
    assert_eq!(PAGE_KEYS.len() + 8, keys::ALL.len());
}

#[test]
fn the_device_id_is_made_on_demand_and_kept() {
    let mut s = ControlSettings::default();
    assert_eq!(s.cloud_device_id, None);
    let made = s.device_id();
    assert_eq!(made.len(), 36);
    assert_eq!(made, made.to_uppercase());
    assert_eq!(made.as_bytes()[14], b'4', "a version 4 UUID: {made}");
    assert_eq!(s.cloud_device_id.as_deref(), Some(made.as_str()));
    assert_eq!(s.device_id(), made);
    assert_ne!(ControlSettings::default().device_id(), made);
}

#[test]
fn a_device_id_is_stored_in_uppercase() {
    let s = ControlSettings::default()
        .validated(
            keys::CLOUD_DEVICE_ID,
            &json!("0f3a5c1e-7b2d-4e96-8a10-5d4c3b2a1908"),
        )
        .unwrap();
    assert_eq!(
        s.cloud_device_id.as_deref(),
        Some("0F3A5C1E-7B2D-4E96-8A10-5D4C3B2A1908")
    );
    // A saved id that isn't a UUID is made again, not trusted.
    let read = file(json!({"cloudDeviceId": "device-1"})).settings();
    assert_eq!(read.cloud_device_id, None);
}

#[test]
fn the_pages_part_is_a_projection() {
    let s = ControlSettings {
        auto_open: "needsInput".into(),
        hot_key: "ctrlAltSpace".into(),
        type_replies: true,
        ..ControlSettings::default()
    };
    let ui = s.ui();
    assert_eq!(ui.panel_open_mode, "needsInput");
    assert_eq!(ui.hotkey, "ctrlAltSpace");
    assert!(ui.type_replies);
    // The hub fills in the hot key service's report; until then nothing is wrong.
    assert!(ui.hotkey_ok);
    assert_eq!(ui.hotkey_message, None);
}
