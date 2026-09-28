//! The UI contract's fixtures (`tests/ui-contract/`), shared with the node
//! tests of the pages: each deserialises into its Rust type and serialises
//! back with the same keys and values; every documented call parses and the
//! sealed hub answers it as recorded; every event's payload is its type.

use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::hub::{Call, Hub, HubConfig, HubEvent};
use agentnotch_engine::model::*;
use agentnotch_engine::persist::json_equivalent;
use agentnotch_engine::platform::Roots;
use agentnotch_engine::testkit::{FakeClock, TEST_START_MS};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn contract_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/ui-contract")
}

fn fixture(name: &str) -> Value {
    let path = contract_dir().join(name);
    serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Deserialises `value` as `T` and back; the result must say the same.
fn round_trip<T: Serialize + DeserializeOwned>(value: &Value, name: &str) -> T {
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let back = serde_json::to_value(&typed).unwrap();
    assert!(
        json_equivalent(value, &back),
        "{name} changed in a round trip:\n{value:#}\n---\n{back:#}"
    );
    typed
}

#[test]
fn ui_contract_fixtures_roundtrip() {
    let snapshot: HubSnapshot = round_trip(&fixture("snapshot.json"), "snapshot.json");
    assert_eq!(snapshot.version, HubSnapshot::VERSION);
    let settings: SettingsSnapshot = round_trip(&fixture("settings.json"), "settings.json");
    let chat: ChatUpdate = round_trip(&fixture("chat.json"), "chat.json");
    assert!(chat.reset);
    assert_eq!(
        chat.order,
        chat.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>()
    );

    // The fixture shows one of each thing the pages render.
    let kinds: std::collections::BTreeSet<&str> = snapshot
        .sessions
        .iter()
        .map(|r| match r.detail {
            RowDetail::Permission { .. } => "permission",
            RowDetail::Question { .. } => "question",
            RowDetail::Plan => "plan",
            RowDetail::Dialog { .. } => "dialog",
            RowDetail::Failed { .. } => "failed",
            RowDetail::Working { .. } => "working",
            RowDetail::Review { .. } => "review",
            RowDetail::Idle { .. } => "idle",
        })
        .collect();
    assert_eq!(kinds.len(), 8, "{kinds:?}");
    assert!(settings.accounts.len() >= 2);
    assert!(matches!(
        settings.cloud.auth,
        CloudAuthState::SignedIn { .. }
    ));
}

#[test]
fn the_snapshot_fixture_is_consistent() {
    let snapshot: HubSnapshot = serde_json::from_value(fixture("snapshot.json")).unwrap();
    let mut totals = Counts::default();
    for row in &snapshot.sessions {
        match (row.bucket.as_str(), row.failed) {
            ("needs_you", true) => totals.failed += 1,
            ("needs_you", false) => totals.needs_you += 1,
            ("ready_for_review", _) => totals.review += 1,
            ("working", _) => totals.working += 1,
            ("idle", _) => totals.idle += 1,
            (other, _) => panic!("bucket {other}"),
        }
        assert!(
            snapshot
                .rings
                .iter()
                .any(|r| Some(&r.ring_id) == row.ring_id.as_ref()),
            "{}",
            row.session_id
        );
        if let Some(pending) = &row.pending {
            assert_eq!(row.bucket, "needs_you", "{}", row.session_id);
            assert!(["permission", "question", "plan"].contains(&pending.kind.as_str()));
        }
    }
    assert_eq!(snapshot.totals, totals);
    assert_eq!(snapshot.tray_badge, totals.needs_you + totals.failed);
    let section_total: u32 = snapshot.sections.iter().map(|s| s.count).sum();
    assert_eq!(section_total as usize, snapshot.sessions.len());
    for ring in &snapshot.rings {
        let n: u32 = ring.counts.needs_you
            + ring.counts.failed
            + ring.counts.review
            + ring.counts.working
            + ring.counts.idle;
        assert_eq!(
            n as usize,
            snapshot
                .sessions
                .iter()
                .filter(|r| r.ring_id.as_ref() == Some(&ring.ring_id))
                .count()
        );
        assert_eq!(
            ring.badges.needs_you,
            ring.counts.needs_you + ring.counts.failed
        );
        for window in &ring.usage.windows {
            assert!((0.0..=1.0).contains(&window.used), "{}", window.id);
        }
    }
}

fn sealed_hub() -> (Hub, Arc<Mutex<Vec<HubEvent>>>) {
    let root = std::env::temp_dir().join("agentnotch-ui-contract-never-created");
    let cfg = HubConfig {
        roots: Roots::under(&root),
        app_version: "1.1.0".into(),
        website: Some("https://agentnotch.rivant.in".into()),
        flags: DevFlags {
            sealed: true,
            ..DevFlags::default()
        },
        hook_exe: root.join("install").join("agentnotch-hook.exe"),
        pipe_name: agentnotch_proto::pipe_name("S-1-5-21-1000000001-1000000002-1000000003-1001"),
    };
    let hub = Hub::sealed(cfg, Arc::new(FakeClock::at_ms(TEST_START_MS)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    hub.on_event(Box::new(move |event| {
        sink.lock().unwrap().push(event.clone())
    }));
    (hub, events)
}

#[test]
fn every_call_parses_and_the_sealed_hub_answers_as_recorded() {
    let calls = fixture("calls.json");
    let calls = calls.as_array().unwrap();
    let mut methods = std::collections::BTreeSet::new();
    for case in calls {
        let raw = &case["call"];
        let call: Call = round_trip(raw, &raw.to_string());
        methods.insert(call.method());
        // The glue's path: method and args apart.
        let method = raw["method"].as_str().unwrap();
        let via_parts =
            Call::from_parts(method, raw.get("args").cloned().unwrap_or(Value::Null)).unwrap();
        assert_eq!(via_parts, call);

        let (hub, _) = sealed_hub();
        hub.start().unwrap();
        let answer = hub.call(call);
        if let Some(code) = case.get("error").and_then(Value::as_str) {
            assert_eq!(answer.map_err(|e| e.code), Err(code.to_owned()), "{raw}");
        } else if let Some(name) = case.get("reply_fixture").and_then(Value::as_str) {
            let reply = answer.unwrap_or_else(|e| panic!("{raw}: {e}"));
            // Same keys as the fixture (times are shifted to the clock).
            let expected = fixture(name);
            assert_eq!(key_paths(&reply), key_paths(&expected), "{raw}");
        } else if let Some(text) = case.get("reply_contains").and_then(Value::as_str) {
            let reply = answer.unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert!(
                reply["text"].as_str().unwrap().contains(text),
                "{raw}: {reply}"
            );
        } else {
            let reply = answer.unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert!(json_equivalent(&reply, &case["reply"]), "{raw}: {reply}");
        }
    }
    // Every method of `Call` is documented.
    let all = [
        "snapshot",
        "settings",
        "answer",
        "send_message",
        "message_route",
        "focus",
        "mark_reviewed",
        "mark_viewed",
        "mark_all_reviewed",
        "dismiss_failure",
        "reset_review_queue",
        "chat_open",
        "chat_close",
        "chat_more",
        "chat_image",
        "refresh_usage",
        "hook_consent",
        "hooks_enabled",
        "status_line_enabled",
        "hooks_reinstall",
        "remove_codenotch_hooks",
        "acknowledge_scope",
        "account",
        "set_setting",
        "choose_claude_binary",
        "cloud",
        "cloud_url",
        "session_state_text",
        "launch_command",
        "reveal_target",
        "panel_state",
        "hotkey_status",
    ];
    for method in all {
        assert!(methods.contains(method), "calls.json lacks {method}");
    }
}

/// Every key path of a JSON value (array items merged), to compare shapes.
fn key_paths(value: &Value) -> std::collections::BTreeSet<String> {
    fn walk(value: &Value, prefix: &str, out: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    let path = format!("{prefix}.{key}");
                    out.insert(path.clone());
                    walk(item, &path, out);
                }
            }
            Value::Array(items) => items
                .iter()
                .for_each(|item| walk(item, &format!("{prefix}[]"), out)),
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(value, "", &mut out);
    out
}

#[test]
fn every_event_payload_is_its_type() {
    let events = fixture("events.json");
    let mut names = std::collections::BTreeSet::new();
    for event in events.as_array().unwrap() {
        let name = event["event"].as_str().unwrap();
        names.insert(name.to_owned());
        let payload = match event.get("payload_fixture").and_then(Value::as_str) {
            Some(file) => fixture(file),
            None => event["payload"].clone(),
        };
        let hub_event = match name {
            "an:snapshot" => HubEvent::Snapshot(round_trip(&payload, name)),
            "an:settings" => HubEvent::Settings(round_trip(&payload, name)),
            "an:cloud" => HubEvent::Cloud(round_trip(&payload, name)),
            "an:chat" => HubEvent::Chat(round_trip(&payload, name)),
            "an:panel" => HubEvent::Panel(round_trip(&payload, name)),
            "an:peek" => {
                let ring_id = payload["ring_id"].as_str().unwrap().to_owned();
                let seconds = payload["seconds"].as_u64().unwrap() as u32;
                HubEvent::Peek { ring_id, seconds }
            }
            "an:notice" => HubEvent::Notice(payload.as_str().unwrap().to_owned()),
            "usage" => HubEvent::UpstreamUsage(round_trip(&payload, name)),
            "an:panel_focus" => {
                assert!(payload["focused"].is_boolean());
                continue;
            }
            other => panic!("unknown event {other}"),
        };
        assert_eq!(hub_event.tauri_event(), Some(name));
        assert!(json_equivalent(&hub_event.payload(), &payload), "{name}");
    }
    for name in [
        "an:snapshot",
        "an:settings",
        "an:cloud",
        "an:chat",
        "an:panel",
        "an:panel_focus",
        "an:peek",
        "an:notice",
        "usage",
    ] {
        assert!(names.contains(name), "events.json lacks {name}");
    }
}

#[test]
fn rebrand_vectors_are_well_formed() {
    let vectors = fixture("rebrand-vectors.json");
    for vector in vectors.as_array().unwrap() {
        assert!(
            vector["input"].is_string() && vector["expected"].is_string(),
            "{vector}"
        );
    }
}

#[test]
fn the_sealed_hub_emits_and_reacts() {
    let (hub, events) = sealed_hub();
    hub.start().unwrap();
    let first: Vec<Option<&'static str>> = events
        .lock()
        .unwrap()
        .iter()
        .map(HubEvent::tauri_event)
        .collect();
    for name in ["an:snapshot", "an:settings", "an:cloud", "usage"] {
        assert!(first.contains(&Some(name)), "{first:?}");
    }
    let snapshot = hub.snapshot();
    assert!(snapshot.sealed);
    // Times follow the clock: the fixture was made at TEST_START_MS too.
    assert_eq!(snapshot.generated_at_ms, TEST_START_MS);
    let before = snapshot.totals;

    events.lock().unwrap().clear();
    let reply = hub
        .call(Call::from_parts("answer", serde_json::json!({"session_id": "needs-permission", "tool_use_id": "toolu_sample_bash", "answer": {"allow": {"always": true}}})).unwrap())
        .unwrap();
    assert_eq!(reply["result"], "delivered");
    let after = hub.snapshot();
    assert_eq!(after.totals.needs_you, before.needs_you - 1);
    assert_eq!(after.totals.working, before.working + 1);
    let row = after
        .sessions
        .iter()
        .find(|r| r.session_id == "needs-permission")
        .unwrap();
    assert!(row.pending.is_none());
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, HubEvent::Snapshot(_))));
    // Answering the same request again: no longer pending.
    let again = hub
        .call(Call::from_parts("answer", serde_json::json!({"session_id": "needs-permission", "tool_use_id": "toolu_sample_bash", "answer": "approve_plan"})).unwrap())
        .unwrap();
    assert_eq!(again["result"], "not_pending");

    // Opening a chat pushes it and marks the session reviewed.
    events.lock().unwrap().clear();
    hub.call(Call::ChatOpen {
        session_id: "review-darkmode".into(),
    })
    .unwrap();
    let pushed = events.lock().unwrap().clone();
    assert!(pushed
        .iter()
        .any(|e| matches!(e, HubEvent::Chat(c) if c.session_id == "review-darkmode" && c.reset)));
    assert_eq!(hub.snapshot().totals.review, before.review - 1);

    // A setting goes through the file's rules.
    hub.call(Call::SetSetting {
        key: "typeReplies".into(),
        value: Value::Bool(true),
    })
    .unwrap();
    assert!(hub.snapshot().ui.type_replies);
    let route = hub
        .call(Call::MessageRoute {
            session_id: "work-ci".into(),
        })
        .unwrap();
    assert_eq!(route["reason"], "Sealed: nothing is typed into a terminal.");

    // Sealed: deep links do nothing, the status says so.
    assert!(matches!(
        hub.handle_deep_link("agentnotch://auth-callback?code=x"),
        agentnotch_engine::hub::DeepLinkOutcome::Ignored(_)
    ));
    let status = hub.control_status();
    assert!(status.sealed);
    assert_eq!(status.transport, "off");
    assert_eq!(status.sessions as usize, after.sessions.len());
    let usage = hub.upstream_usage();
    assert_ne!(usage.status, "needsAuth");
    assert!(usage.windows.iter().any(|w| w.id == "session"));
    assert!(usage
        .windows
        .iter()
        .any(|w| w.id.starts_with("session@claude-acct-")));
}

#[test]
fn optional_arguments_may_be_null_or_missing() {
    let null = Call::from_parts("choose_claude_binary", serde_json::json!({"path": null})).unwrap();
    let missing = Call::from_parts("choose_claude_binary", serde_json::json!({})).unwrap();
    assert_eq!(null, missing);
    // A method without arguments takes none, null or {}.
    for args in [Value::Null, serde_json::json!({})] {
        assert_eq!(Call::from_parts("snapshot", args).unwrap(), Call::Snapshot);
    }
    assert_eq!(
        Call::from_parts("no_such_method", Value::Null)
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        Call::from_parts("answer", serde_json::json!({"session_id": 5}))
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[test]
fn window_labels_gate_methods() {
    use agentnotch_engine::hub::allowed_from_window;
    assert!(allowed_from_window("agentnotch-panel", "answer"));
    assert!(!allowed_from_window("agentnotch-panel", "panel_state"));
    assert!(allowed_from_window("notch", "panel_toggle"));
    assert!(allowed_from_window("notch", "refresh_usage"));
    assert!(!allowed_from_window("notch", "answer"));
    assert!(!allowed_from_window("settings", "send_message"));
    assert!(allowed_from_window("settings", "hook_consent"));
    assert!(!allowed_from_window("settings", "hotkey_status"));
    assert!(!allowed_from_window("dropzones", "snapshot"));
}
