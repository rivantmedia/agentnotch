//! Usage through the live hub (design §4.6, AU§9): the 20-second cycle
//! reads every folder's `.claude.json`, then the probe that is due runs on
//! the probe lane against a scripted `claude` and its reading reaches the
//! rings, the snapshot and `control status`; a probe job that fails (a
//! panicking body) still finishes its plan, so the next one runs; the
//! interval setting is followed; `refresh_usage` answers whether a reading
//! is coming; the launch rings come from a synchronous read-only
//! discovery; `usage-state.json` is kept across runs.
//!
//! Smoke phase 5 in miniature: before consent the only children are probes
//! (the exact probe argv, in `<support>\usage-probe`), and nothing in the
//! home folder changes.

mod accounts_support;
mod hub_support;

use accounts_support::{Home, BIIOS, BIIOS_UUID, PARAS, PARAS_UUID};
use agentnotch_engine::core::time::iso8601;
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, UsageRefreshTrigger};
use agentnotch_engine::model::HubSnapshot;
use agentnotch_engine::platform::{CommandRunner, CommandSpec, Platform, RunningCommand};
use agentnotch_engine::testkit::runner::Conversation;
use agentnotch_engine::testkit::{snapshot_dir, ScriptedRunner, TEST_START_MS};
use agentnotch_engine::usage::probe::{arguments, INITIALIZE_REQUEST_ID, USAGE_REQUEST_ID};
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// A home with two accounts and no cached usage anywhere: `~\.claude`
/// (paras) and `~\.claude-work` (biios), and `claude.exe` where the
/// installer puts it (never run: the runner is scripted).
fn build(home: &Home) {
    home.write(".claude/sessions/1.json", "{}");
    home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
    home.write(".claude-work/sessions/1.json", "{}");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, None),
    );
    home.write(".local/bin/claude.exe", "");
}

fn base_of(home: &Home) -> PathBuf {
    home.roots
        .home
        .parent()
        .expect("the test's root")
        .to_path_buf()
}

fn write_settings(home: &Home, settings: &Value) {
    std::fs::create_dir_all(&home.roots.support).unwrap();
    std::fs::write(
        home.roots.support.join("control-settings.json"),
        serde_json::to_vec(settings).unwrap(),
    )
    .unwrap();
}

fn hub_over(home: &Home, customise: impl FnOnce(&mut Platform)) -> TestHub {
    TestHub::over(&base_of(home), RuntimeOptions::default(), |_| {}, customise)
}

fn success(id: &str, payload: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": payload}}))
    .unwrap()
}

/// `claude` answering `initialize` and `get_usage`: 37 % of the session,
/// 61 % of the week.
fn usage_answer() -> Conversation {
    let start = UNIX_EPOCH + Duration::from_millis(TEST_START_MS);
    let session_resets = iso8601(start + Duration::from_secs(3 * 3600));
    let week_resets = iso8601(start + Duration::from_secs(3 * 86_400));
    // A fresh answer carries `limits` (a seeded one, Claude Code's copy of
    // its cache, doesn't).
    let body = json!({
        "five_hour": {"utilization": 37, "resets_at": session_resets},
        "seven_day": {"utilization": 61, "resets_at": week_resets},
        "limits": [
            {"kind": "session", "percent": 37, "resets_at": session_resets},
            {"kind": "weekly_all", "percent": 61, "resets_at": week_resets},
        ],
    });
    Conversation::new()
        .on_request(
            INITIALIZE_REQUEST_ID,
            vec![success(INITIALIZE_REQUEST_ID, json!({}))],
        )
        .on_request(
            USAGE_REQUEST_ID,
            vec![success(
                USAGE_REQUEST_ID,
                json!({"subscription_type": "max", "rate_limits_available": true,
                       "rate_limits": body}),
            )],
        )
}

fn shown(hub: &TestHub) -> HubSnapshot {
    let value = hub.hub.call(Call::Snapshot).expect("a snapshot");
    serde_json::from_value(value).expect("a snapshot's shape")
}

/// Waits for `control status` (what the pages were last shown) to count
/// `readings`; the clock moves a little each time so the coalesced
/// publishing goes on.
fn wait_for_readings(hub: &TestHub, readings: u32) -> bool {
    eventually(|| {
        hub.handles.clock.advance(Duration::from_millis(100));
        hub.sync();
        hub.hub.control_status().readings == readings
    })
}

fn probes_of(spawned: &[CommandSpec]) -> usize {
    spawned
        .iter()
        .filter(|spec| spec.args == arguments(&[]))
        .count()
}

#[test]
fn a_probe_reading_reaches_the_rings_before_consent() {
    let home = Home::new();
    build(&home);
    let before = snapshot_dir(&home.roots.home).unwrap();
    let hub = hub_over(&home, |_| {});
    hub.handles.runner.push_conversation(usage_answer());
    hub.handles.runner.push_conversation(usage_answer());
    hub.hub.start().expect("the hub starts");

    assert!(wait_for_readings(&hub, 2), "{:?}", hub.hub.control_status());
    let status = hub.hub.control_status();
    assert_eq!(status.accounts, 2);
    assert_eq!(status.hook_consent, "unasked");

    let snapshot = shown(&hub);
    assert_eq!(snapshot.rings.len(), 2);
    for ring in &snapshot.rings {
        assert_eq!(ring.usage.status, "ok", "{ring:?}");
        let session = ring
            .usage
            .windows
            .iter()
            .find(|window| window.id.starts_with("session"))
            .unwrap_or_else(|| panic!("a session window: {ring:?}"));
        assert!((session.used - 0.37).abs() < 1e-9, "{session:?}");
    }
    assert_eq!(hub.hub.upstream_usage().status, "ok");

    // Only probes ran: the exact argv, in `<support>\usage-probe`; no
    // `claude --version` before the yes.
    let spawned = hub.handles.runner.spawned();
    assert_eq!(spawned.len(), 2, "{spawned:?}");
    assert_eq!(probes_of(&spawned), 2, "{spawned:?}");
    for spec in &spawned {
        assert_eq!(spec.cwd, home.roots.usage_probe_dir());
        assert_eq!(spec.program, home.roots.home.join(".local/bin/claude.exe"));
    }
    // Nothing in the home folder changed.
    assert_eq!(snapshot_dir(&home.roots.home).unwrap(), before);
}

/// The next spawn panics, then the runner given takes over: a probe job
/// whose body panics.
struct PanicsOnce {
    inner: Arc<ScriptedRunner>,
    panics: AtomicUsize,
}

impl CommandRunner for PanicsOnce {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        if self.panics.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("a probe that panics");
        }
        self.inner.spawn(spec)
    }
}

#[test]
fn a_failing_probe_job_still_lets_the_next_one_run() {
    let home = Home::new();
    build(&home);
    let inner = Arc::new(ScriptedRunner::default());
    inner.push_conversation(usage_answer());
    let runner = Arc::new(PanicsOnce {
        inner: inner.clone(),
        panics: AtomicUsize::new(0),
    });
    let given = runner.clone();
    let hub = hub_over(&home, move |platform| platform.runner = given);
    hub.hub.start().expect("the hub starts");

    // The panicking probe finished its plan: the other account's ran.
    assert!(wait_for_readings(&hub, 1), "{:?}", hub.hub.control_status());
    assert_eq!(runner.panics.load(Ordering::SeqCst), 2);
    assert_eq!(probes_of(&inner.spawned()), 1);
    let snapshot = shown(&hub);
    let failed: Vec<_> = snapshot
        .rings
        .iter()
        .filter(|ring| ring.usage.windows.is_empty())
        .collect();
    assert_eq!(failed.len(), 1, "{:?}", snapshot.rings);
    assert_ne!(failed[0].usage.status, "ok", "{:?}", failed[0]);
}

#[test]
fn the_probe_interval_setting_is_followed() {
    let home = Home::new();
    build(&home);
    write_settings(&home, &json!({"usageProbeIntervalMinutes": 0}));
    let hub = hub_over(&home, |_| {});
    hub.handles.runner.push_conversation(usage_answer());
    hub.handles.runner.push_conversation(usage_answer());
    hub.hub.start().expect("the hub starts");
    for _ in 0..4 {
        hub.handles.clock.advance(Duration::from_secs(21));
        hub.sync();
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(hub.handles.runner.spawned().is_empty(), "probes are off");

    hub.hub
        .call(Call::SetSetting {
            key: "usageProbeIntervalMinutes".into(),
            value: json!(5),
        })
        .expect("a page may set it");
    assert!(eventually(|| {
        hub.handles.clock.advance(Duration::from_secs(21));
        hub.sync();
        probes_of(&hub.handles.runner.spawned()) >= 1
    }));
    assert!(wait_for_readings(&hub, 2));

    // The Claude Desktop switch reaches the store (and the settings).
    hub.hub
        .call(Call::SetSetting {
            key: "readsDesktopUsageCache".into(),
            value: json!(false),
        })
        .expect("a page may set it");
    let value = hub.hub.call(Call::Settings).expect("the settings");
    assert_eq!(value["usage"]["desktop_cache"], json!(false));
}

#[test]
fn refresh_usage_says_whether_a_reading_is_coming() {
    let home = Home::new();
    build(&home);
    write_settings(&home, &json!({"usageProbeIntervalMinutes": 0}));
    let hub = hub_over(&home, |_| {});

    // A hub that isn't running checks nothing.
    let reply = hub
        .hub
        .call(Call::RefreshUsage {
            ring_id: None,
            reason: UsageRefreshTrigger::Manual,
        })
        .expect("answered");
    assert_eq!(reply, json!({"coming": false}));
    assert!(hub.handles.runner.spawned().is_empty());

    hub.handles.runner.push_conversation(usage_answer());
    hub.handles.runner.push_conversation(usage_answer());
    hub.hub.start().expect("the hub starts");
    hub.sync();
    let ring = shown(&hub).rings[0].ring_id.clone();
    let reply = hub
        .hub
        .call(Call::RefreshUsage {
            ring_id: Some(ring),
            reason: UsageRefreshTrigger::Manual,
        })
        .expect("answered");
    let coming = reply["coming"].as_bool().expect("{coming}");
    if coming {
        assert!(wait_for_readings(&hub, 1));
        assert_eq!(probes_of(&hub.handles.runner.spawned()), 1);
    }
    // An unknown ring: nothing is coming.
    let reply = hub
        .hub
        .call(Call::RefreshUsage {
            ring_id: Some("claude-acct-000000000000".into()),
            reason: UsageRefreshTrigger::RingClick,
        })
        .expect("answered");
    assert_eq!(reply, json!({"coming": false}));
}

/// The rings at launch come from a discovery that only reads; the
/// readings of the last run come back with `usage-state.json`.
#[test]
fn launch_rings_read_only_and_readings_survive_a_restart() {
    let home = Home::new();
    build(&home);
    {
        let hub = hub_over(&home, |_| {});
        let rings = hub.hub.launch_rings();
        assert_eq!(rings.len(), 2, "{rings:?}");
        assert!(rings.iter().all(|ring| ring.usage.windows.is_empty()));
        assert!(hub.handles.runner.spawned().is_empty());
        assert!(hub.files.writes.lock().unwrap().is_empty());

        hub.handles.runner.push_conversation(usage_answer());
        hub.handles.runner.push_conversation(usage_answer());
        hub.hub.start().expect("the hub starts");
        assert!(wait_for_readings(&hub, 2));
        hub.hub.stop();
        assert!(home.roots.support.join("usage-state.json").exists());
        assert!(home.roots.support.join("accounts.json").exists());
    }
    let again = hub_over(&home, |_| {});
    let rings = again.hub.launch_rings();
    assert_eq!(rings.len(), 2);
    for ring in &rings {
        assert!(!ring.usage.windows.is_empty(), "{ring:?}");
    }
    assert!(again.handles.runner.spawned().is_empty());
}

/// Smoke phase 5 counts two readings, one per account, from the fake
/// `claude`, which answers `get_usage` with the smoke profile's
/// `usage.json`: that file is a reading the probe keeps (a `limits` list,
/// so never Claude Code's seeded fallback, which a profile with no cached
/// usage couldn't date).
#[test]
fn the_smoke_usage_fixture_is_a_probe_reading() {
    use agentnotch_engine::usage::parser::{parse_get_usage_response, GetUsageResult};
    use agentnotch_engine::usage::probe::interpret_probe_answer;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scripts/smoke/profile/usage.json"
    );
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).expect("usage.json"))
        .expect("usage.json is JSON");
    let GetUsageResult::Usage(parsed) =
        parse_get_usage_response(fixture.as_object().expect("an object"))
    else {
        panic!("not a usage answer: {fixture}");
    };
    assert!(!parsed.is_possibly_seeded);
    let now = UNIX_EPOCH + Duration::from_millis(TEST_START_MS);
    let id = agentnotch_engine::model::IdentityId::from("uuid:smoke");
    let (reading, rate_limited) = interpret_probe_answer(&parsed, &id, now, None, Some(now));
    assert!(!rate_limited);
    assert!(reading.is_some(), "the probe keeps a reading");
}
