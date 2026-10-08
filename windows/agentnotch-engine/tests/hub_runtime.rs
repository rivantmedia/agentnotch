//! The live hub's runtime (design §1.2): `an-core` applying inputs in
//! order across a stop and a start, calls and their 25 s bound, the
//! coalesced events, the one settings writer, the job lanes answering every
//! job (a panicking body too), and stop. Ports of A1_ReviewFixesTests'
//! pipeline restarts and A1_SessionStoreRegressionTests' burst, plus the
//! Windows runtime's own rules.
//!
//! Hook frames reach the hub with the ingress (wp7-8), so the restart tests
//! drive `an-core` through its other inputs (`Input::SetSetting`, what the
//! cloud thread sends); time moves only through the fake clock and
//! `Input::Tick`.

mod hub_support;

use agentnotch_engine::hub::jobs::{self, JobContext, TrackingRunner};
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, HubEvent};
use agentnotch_engine::model::HubSnapshot;
use agentnotch_engine::platform::{
    CommandRunner, CommandSpec, ConsoleInfo, FocusOutcome, FocusStep, Foreground, HostApp,
    ProcessTable, Terminals,
};
use agentnotch_engine::runtime_types::{Input, Job, JobResult};
use agentnotch_engine::testkit::{self, runner::Conversation};
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

fn set(key: &str, value: Value) -> Input {
    Input::SetSetting {
        key: key.into(),
        value,
    }
}

/// What a fresh projection shows (a call, so it follows every input sent
/// before it).
fn shown(hub: &TestHub) -> HubSnapshot {
    let value = hub.hub.call(Call::Snapshot).expect("a snapshot");
    serde_json::from_value(value).expect("a snapshot's shape")
}

// ---- restarts (A1_ReviewFixesTests) ----

/// A stopped and started hub still applies what it is sent.
#[test]
fn the_pipeline_applies_events_after_a_restart() {
    let hub = TestHub::started();
    hub.inputs.send(set("ringBadges", json!(false)));
    assert!(!shown(&hub).ui.ring_badges);

    hub.hub.stop();
    hub.hub.start().expect("starts again");
    hub.inputs.send(set("restingMarks", json!(false)));
    let after = shown(&hub);
    assert!(!after.ui.resting_marks);
    // The core kept what it had before the stop.
    assert!(!after.ui.ring_badges);
}

/// Inputs sent while the hub is stopped wait for the next start.
#[test]
fn inputs_yielded_while_stopped_wait_for_the_next_start() {
    let hub = TestHub::started();
    hub.hub.stop();
    hub.inputs.send(set("ringBadges", json!(false)));
    std::thread::sleep(Duration::from_millis(100));
    // A stopped hub answers on the caller's thread, from a core that hasn't
    // taken the queued input.
    assert!(shown(&hub).ui.ring_badges);

    hub.hub.start().expect("starts again");
    assert!(!shown(&hub).ui.ring_badges);
}

/// A hub stopped and started again runs its threads again: calls, inputs
/// and writes all work (the pipe comes back with the ingress, wp7-8).
#[test]
fn a_hub_stopped_and_started_again_still_listens() {
    let hub = TestHub::started();
    hub.hub.stop();
    hub.hub.start().expect("starts again");
    hub.inputs.send(set("cloudSyncEnabled", json!(true)));
    hub.sync();
    assert!(eventually(
        || hub.settings_file()["cloudSyncEnabled"] == json!(true)
    ));
}

// ---- the burst (A1_SessionStoreRegressionTests) ----

/// A burst goes out in a few coalesced snapshots, at least 50 ms apart, the
/// last one showing the end state; an unchanged state isn't sent again.
#[test]
fn a_burst_is_published_in_a_few_coalesced_updates() {
    let hub = TestHub::started();
    assert!(
        eventually(|| hub.snapshots().len() == 1),
        "the first snapshot at start"
    );
    for index in 0..300 {
        hub.inputs.send(set("ringBadges", json!(index % 2 == 0)));
    }
    hub.sync();
    // The clock hasn't moved: nothing more may go out yet.
    assert_eq!(hub.snapshots().len(), 1);

    hub.handles.clock.advance(Duration::from_millis(50));
    hub.inputs.send(Input::Tick);
    hub.sync();
    let published = hub.snapshots();
    assert!(
        published.len() >= 2 && published.len() < 60,
        "{}",
        published.len()
    );
    let (_, last) = published.last().unwrap();
    assert!(
        !last.ui.ring_badges,
        "the last snapshot shows the end state"
    );
    for pair in published.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= Duration::from_millis(50));
        assert!(pair[1].1.generated_at_ms > pair[0].1.generated_at_ms);
    }

    // The same value again changes nothing: no event.
    let settings_events = hub.settings_events();
    hub.inputs.send(set("ringBadges", json!(false)));
    hub.handles.clock.advance(Duration::from_millis(100));
    hub.inputs.send(Input::Tick);
    hub.sync();
    assert_eq!(hub.snapshots().len(), published.len());
    assert_eq!(hub.settings_events(), settings_events);
}

/// Nothing changed, nothing sent: ticks alone publish nothing.
#[test]
fn ticks_without_a_change_send_nothing() {
    let hub = TestHub::started();
    assert!(eventually(|| hub.snapshots().len() == 1));
    let before = hub.events().len();
    for _ in 0..5 {
        hub.handles.clock.advance(Duration::from_secs(1));
        hub.inputs.send(Input::Tick);
    }
    hub.sync();
    assert_eq!(hub.events().len(), before);
}

/// A start sends what the pages need at once.
#[test]
fn a_start_publishes_everything_once() {
    let hub = TestHub::started();
    assert!(eventually(|| hub.events().len() >= 5));
    let kinds: Vec<&str> = hub
        .events()
        .iter()
        .map(|(_, e)| match e {
            HubEvent::Snapshot(_) => "snapshot",
            HubEvent::Settings(_) => "settings",
            HubEvent::Cloud(_) => "cloud",
            HubEvent::UpstreamUsage(_) => "usage",
            HubEvent::TrayBadge(_) => "tray",
            _ => "other",
        })
        .collect();
    for kind in ["snapshot", "settings", "cloud", "usage", "tray"] {
        assert_eq!(
            kinds.iter().filter(|k| **k == kind).count(),
            1,
            "{kind}: {kinds:?}"
        );
    }
    assert!(!hub.hub.snapshot().sealed);
}

// ---- settings ----

/// `Input::SetSetting` from another thread (the cloud's) lands in
/// `control-settings.json`, written once, by a job `an-core` handed to a
/// file lane; a bad value is refused and writes nothing.
#[test]
fn set_setting_from_another_thread_is_written_once_by_an_core() {
    let hub = TestHub::started();
    let inputs = hub.inputs.clone();
    std::thread::spawn(move || inputs.send(set("cloudSyncEnabled", json!(true))))
        .join()
        .unwrap();
    hub.sync();
    assert!(eventually(
        || hub.settings_file()["cloudSyncEnabled"] == json!(true)
    ));
    let writes = hub.files.writes_of("control-settings.json");
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(writes[0].thread.starts_with("an-io-"), "{writes:?}");
    // Every key is written, unknown ones kept (persist::settings).
    assert_eq!(hub.settings_file()["version"], json!(1));
    assert_eq!(hub.settings_file()["hookConsent"], Value::Null);

    hub.inputs.send(set("peekSeconds", json!(7)));
    hub.inputs.send(set("noSuchSetting", json!(true)));
    hub.sync();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(hub.files.writes_of("control-settings.json").len(), 1);
    assert_eq!(hub.settings_file()["peekSeconds"], json!(5));
    let logs = hub.logs();
    assert!(logs.iter().any(|l| l.contains("peekSeconds")), "{logs:?}");
    assert!(logs.iter().any(|l| l.contains("noSuchSetting")), "{logs:?}");
}

/// A page sets only the keys a page may set; the reply says why not.
#[test]
fn a_page_sets_only_page_keys() {
    let hub = TestHub::started();
    let set = |key: &str, value: Value| {
        hub.hub.call(Call::SetSetting {
            key: key.into(),
            value,
        })
    };
    assert_eq!(set("peek", json!(false)).unwrap(), json!({}));
    let refused = set("hookConsent", json!(true)).unwrap_err();
    assert_eq!(refused.code, "invalid");
    let refused = set("peekSeconds", json!(4)).unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(!shown(&hub).ui.peek);
    assert!(eventually(|| hub.settings_file()["peek"] == json!(false)));
    assert_eq!(hub.settings_file()["hookConsent"], Value::Null);
}

/// Saved settings are read at start, unknown keys kept on the next write.
#[test]
fn the_settings_file_is_read_and_its_unknown_keys_kept() {
    let hub = TestHub::new();
    std::fs::create_dir_all(&hub.roots.support).unwrap();
    std::fs::write(
        hub.settings_path(),
        br#"{"version":1,"ringBadges":false,"peekSeconds":99,"futureKey":"kept"}"#,
    )
    .unwrap();
    hub.hub.start().unwrap();
    let snapshot = shown(&hub);
    assert!(!snapshot.ui.ring_badges);
    assert_eq!(
        snapshot.ui.peek_seconds, 5,
        "an invalid value reads as its default"
    );
    hub.inputs.send(set("sound", json!(false)));
    hub.sync();
    assert!(eventually(|| hub.settings_file()["sound"] == json!(false)));
    let file = hub.settings_file();
    assert_eq!(file["futureKey"], json!("kept"));
    assert_eq!(file["ringBadges"], json!(false));
}

/// A stop saves now: a change sent right before it is on disk after it.
#[test]
fn stop_saves_the_settings_now() {
    let hub = TestHub::started();
    for value in [false, true, false] {
        hub.inputs.send(set("trayBadge", json!(value)));
    }
    hub.hub.stop();
    assert_eq!(hub.settings_file()["trayBadge"], json!(false));
    for write in hub.files.writes_of("control-settings.json") {
        assert!(
            write.thread.starts_with("an-io-") || write.thread == "an-core",
            "{write:?}"
        );
    }
}

// ---- calls ----

/// A call `an-core` can't take within the bound is answered `busy`; the
/// hub answers again once `an-core` is free.
#[test]
fn a_call_an_core_cannot_take_in_time_is_busy() {
    let options = RuntimeOptions {
        call_timeout: Duration::from_millis(100),
        ..RuntimeOptions::default()
    };
    let hub = TestHub::with(options, |_| {});
    // A sink that holds `an-core` while the gate is shut.
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    hub.hub.on_event(Box::new(move |event| {
        if matches!(event, HubEvent::Log(line) if line.contains("hold")) {
            let (open, wake) = &*held;
            let mut open = open.lock().unwrap();
            while !*open {
                open = wake.wait(open).unwrap();
            }
        }
    }));
    hub.hub.start().unwrap();
    // A refused setting logs its key: the sink holds `an-core` there.
    hub.inputs.send(set("hold", json!(true)));
    let busy = hub.hub.call(Call::Snapshot).unwrap_err();
    assert_eq!(busy.code, "busy");

    let (open, wake) = &*gate;
    *open.lock().unwrap() = true;
    wake.notify_all();
    assert!(hub.hub.call(Call::Snapshot).is_ok());
}

/// A stopped hub answers on the caller's thread (the command line's
/// never-started hub), and its writes still reach the file.
#[test]
fn a_hub_never_started_answers_calls() {
    let hub = TestHub::new();
    assert!(hub.hub.snapshot().ui.ring_badges);
    hub.hub
        .call(Call::SetSetting {
            key: "ringBadges".into(),
            value: json!(false),
        })
        .unwrap();
    assert!(!shown(&hub).ui.ring_badges);
    assert_eq!(hub.settings_file()["ringBadges"], json!(false));
    let status = hub.hub.control_status();
    assert_eq!(status.hook_consent, "unasked");
    assert!(!status.sealed);
}

/// A session's folder is named only for a session the engine knows; a page
/// never names a path itself.
#[test]
fn an_unknown_sessions_folder_is_not_revealed() {
    let hub = TestHub::started();
    let error = hub
        .hub
        .call(Call::RevealTarget {
            kind: agentnotch_engine::hub::RevealKind::SessionCwd,
            id: "s-1".into(),
        })
        .unwrap_err();
    assert_eq!(error.code, "not_found", "{}", error.message);
}

/// The glue's own calls are taken (the hot key's report shows in Settings).
#[test]
fn the_glue_reports_the_hot_key() {
    let hub = TestHub::started();
    hub.hub
        .call(Call::HotkeyStatus {
            ok: false,
            message: Some("Ctrl+Alt+Space is taken by another app.".into()),
        })
        .unwrap();
    let snapshot = shown(&hub);
    assert!(!snapshot.ui.hotkey_ok);
    assert_eq!(
        snapshot.ui.hotkey_message.as_deref(),
        Some("Ctrl+Alt+Space is taken by another app.")
    );
}

// ---- start and stop ----

#[test]
fn stop_twice_is_fine_and_start_twice_too() {
    let hub = TestHub::new();
    hub.hub.stop();
    hub.hub.start().unwrap();
    hub.hub.start().unwrap();
    hub.hub.stop();
    hub.hub.stop();
    hub.hub.start().unwrap();
    hub.inputs.send(set("peek", json!(false)));
    assert!(!shown(&hub).ui.peek);
    hub.hub.stop();
    hub.hub.stop();
}

/// `--dump-state` writes a line per publish that changed what it shows.
#[test]
fn dump_state_logs_the_state() {
    let hub = TestHub::with_config(
        RuntimeOptions::default(),
        |cfg| cfg.flags.dump_state = true,
        |_| {},
    );
    hub.hub.start().unwrap();
    assert!(eventually(|| hub
        .logs()
        .iter()
        .any(|l| l == "[agentnotch-state] publish #1: 0 session(s)")));
}

// ---- jobs ----

/// A job body that panics still answers: the settings write comes back as
/// failed, the lane lives on, and the next change is written.
#[test]
fn a_panicking_job_body_still_yields_its_result() {
    let hub = TestHub::started();
    // The launch's own write (the review file's first heartbeat) first, so
    // the panicking write is the settings'.
    assert!(eventually(|| !hub
        .files
        .writes_of("review-state.json")
        .is_empty()));
    hub.files.panics.store(1, Ordering::SeqCst);
    hub.inputs.send(set("sound", json!(false)));
    hub.sync();
    assert!(eventually(|| hub.logs().iter().any(|l| l
        .contains("settings not saved")
        && l.contains("a disk that panics"))));
    hub.inputs.send(set("peek", json!(false)));
    hub.sync();
    assert!(eventually(|| hub.settings_file()["peek"] == json!(false)));
    assert_eq!(hub.settings_file()["sound"], json!(false));
}

struct PanickingTerminals;

impl Terminals for PanickingTerminals {
    fn classify_host(&self, _: u32, _: &ProcessTable) -> HostApp {
        panic!("classify")
    }
    fn console_info(&self, _: u32) -> ConsoleInfo {
        panic!("console")
    }
    fn run_focus(&self, _: &FocusStep) -> FocusOutcome {
        panic!("focus")
    }
    fn foreground(&self) -> Option<Foreground> {
        panic!("foreground")
    }
    fn window_title(&self, _: u64) -> Option<String> {
        panic!("title")
    }
    fn wt_tab_titles(&self, _: u64) -> Option<Vec<(String, bool)>> {
        panic!("tabs")
    }
    fn any_terminal_visible(&self) -> bool {
        panic!("visible")
    }
    fn watch_foreground(&self, _: crossbeam_channel::Sender<Foreground>) {}
}

/// Every job body's panic becomes that job's own failure.
#[test]
fn the_executor_turns_a_panic_into_the_jobs_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (mut platform, handles) = testkit::platform(dir.path());
    platform.terminals = Arc::new(PanickingTerminals);
    let ctx = JobContext {
        roots: handles.roots.clone(),
        platform,
        base_env: Vec::new(),
        sealed: false,
    };
    let mut no = || false;
    assert_eq!(
        jobs::run_guarded(&Job::Visibility, &ctx, &mut no),
        JobResult::Visible {
            any_terminal: true,
            full_screen: false
        }
    );
    match jobs::run_guarded(
        &Job::Focus {
            steps: vec![FocusStep::ActivatePid { pid: 4 }],
        },
        &ctx,
        &mut no,
    ) {
        JobResult::Focus(FocusOutcome::Failed(why)) => assert!(why.contains("focus"), "{why}"),
        other => panic!("{other:?}"),
    }
    match jobs::run_guarded(&Job::Classify { pid: 4 }, &ctx, &mut no) {
        JobResult::Host(host) => assert_eq!(host.window, None),
        other => panic!("{other:?}"),
    }
}

/// A console is asked about only for the process the session ran (pids
/// come back fast on Windows).
#[test]
fn console_info_needs_the_same_process() {
    let dir = tempfile::tempdir().unwrap();
    let (platform, handles) = testkit::platform(dir.path());
    let ctx = JobContext {
        roots: handles.roots.clone(),
        platform,
        base_env: Vec::new(),
        sealed: false,
    };
    let job = Job::ConsoleInfo {
        pid: 4242,
        started: std::time::UNIX_EPOCH,
    };
    match jobs::run_guarded(&job, &ctx, &mut || false) {
        JobResult::Console(info) => assert!(info.error.is_some()),
        other => panic!("{other:?}"),
    }
}

/// `an-core`'s check is asked before the first key too: a reply it no
/// longer allows (the session changed while the job waited for its lane) is
/// never typed at all.
#[test]
fn a_reply_the_check_refuses_is_never_typed() {
    let dir = tempfile::tempdir().unwrap();
    let (platform, handles) = testkit::platform(dir.path());
    let ctx = JobContext {
        roots: handles.roots.clone(),
        platform,
        base_env: Vec::new(),
        sealed: false,
    };
    let job = Job::Type {
        session: "s1".into(),
        target: agentnotch_engine::platform::ConsoleTarget {
            claude_pid: 4242,
            claude_started: std::time::UNIX_EPOCH,
            expected_window: None,
            allowed_shells: Vec::new(),
        },
        text: "go on".into(),
    };
    let mut asked = 0;
    let result = jobs::run_guarded(&job, &ctx, &mut || {
        asked += 1;
        false
    });
    assert_eq!(
        result,
        JobResult::Typed(agentnotch_engine::platform::TypeOutcome::Refused(
            jobs::NOT_TYPED.into()
        ))
    );
    assert_eq!(asked, 1);
    assert!(handles.console.typed().is_empty());
}

/// A stop ends every child at once and starts no new one until the hub
/// starts again.
#[test]
fn the_tracking_runner_ends_its_children() {
    let scripted = Arc::new(testkit::ScriptedRunner::default());
    scripted.push_conversation(Conversation::hanging());
    let runner = Arc::new(TrackingRunner::new(scripted.clone()));
    let spec = CommandSpec {
        program: "claude.exe".into(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: dir_of_test(),
    };
    let mut child = runner.spawn(spec.clone()).expect("spawned");
    let waiting = std::thread::spawn(move || child.wait_timeout(Duration::from_secs(30)));
    assert!(eventually(|| runner.running() == 1));
    runner.kill_all();
    let exit = waiting.join().unwrap().unwrap();
    assert!(exit.is_some(), "the wait ends with the kill");
    assert_eq!(scripted.kills_of(0), Some(1));
    assert!(
        runner.spawn(spec.clone()).is_err(),
        "nothing starts while stopping"
    );
    runner.resume();
    scripted.push_conversation(Conversation::new());
    assert!(runner.spawn(spec).is_ok());
}

fn dir_of_test() -> std::path::PathBuf {
    std::env::temp_dir()
}
