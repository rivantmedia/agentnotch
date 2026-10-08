//! The sealed demo (`hub::sealed_demo`, WP7) and the sealed hub serving it:
//! the demo through the real projections is the ui-contract fixture (at its
//! own time and shifted to any other), ports of A3_SealedHubTests,
//! C_SealedDemoScriptTests, A1_SessionStoreRegressionTests.
//! aSealedAnswerChangesTheFixtureOnly and A2_ConsentGateTests.
//! setupActionsDoNothingWhenSealed, and a sealed hub over a temporary folder
//! that takes every documented call and leaves the folder empty.
//!
//! Windows differences: the sample has no second terminal dialog (a Windows
//! row draws an elicitation and a dialog alike), so four sessions need an
//! answer where the Mac's five do; the third account's arrival is checked on
//! the demo's stores, and the transition baseline is the runtime's (the
//! sealed hub doesn't play the script by itself).

use agentnotch_engine::accounts::identities::ring_id_for_config_dir;
use agentnotch_engine::accounts::Folder;
use agentnotch_engine::attention::policy::BURST_WINDOW;
use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::core::time::{from_ms, to_ms};
use agentnotch_engine::hub::project::{fresh_success_until, FRESH_SUCCESS_WINDOW};
use agentnotch_engine::hub::sealed_demo::{
    self, apply_step, SealedDemo, SealedDemoStep, THIRD_ACCOUNT_REVIEW_AGE,
};
use agentnotch_engine::hub::{Call, DeepLinkOutcome, Hub, HubConfig, HubEvent};
use agentnotch_engine::model::*;
use agentnotch_engine::platform::Roots;
use agentnotch_engine::sessions::session::Session;
use agentnotch_engine::testkit::FakeClock;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// When the ui-contract fixtures were made.
const GENERATED_AT_MS: u64 = 1_790_000_000_000;
const PIPE: &str = r"\\.\pipe\agentnotch-hook-S-1-5-21-1000000001-1000000002-1000000003-1001";

fn generated_at() -> SystemTime {
    from_ms(GENERATED_AT_MS)
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/ui-contract/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Every place two values differ, as `path: fixture != demo`.
fn differences(path: &str, want: &Value, got: &Value, out: &mut Vec<String>) {
    match (want, got) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let item = b.get(key).unwrap_or(&Value::Null);
                differences(&format!("{path}.{key}"), value, item, out);
            }
            for key in b.keys().filter(|k| !a.contains_key(*k)) {
                out.push(format!("{path}.{key}: missing in the fixture"));
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                differences(&format!("{path}[{index}]"), x, y, out);
            }
        }
        _ if want != got => out.push(format!("{path}:\n  fixture {want}\n  demo    {got}")),
        _ => {}
    }
}

fn assert_same(name: &str, want: &Value, got: &impl serde::Serialize) {
    let mut out = Vec::new();
    differences(name, want, &serde_json::to_value(got).unwrap(), &mut out);
    assert!(out.is_empty(), "{}", out.join("\n"));
}

/// Moves every time of a fixture by `delta_ms` (the times the README names).
fn shift_times(value: &mut Value, delta_ms: u64) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                let is_time = key.ends_with("_at_ms")
                    || key == "since_ms"
                    || key == "resets_at"
                    || key == "fetched_at";
                match item {
                    Value::Number(n) if is_time => {
                        *item = Value::from(n.as_u64().unwrap() + delta_ms);
                    }
                    other => shift_times(other, delta_ms),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| shift_times(i, delta_ms)),
        _ => {}
    }
}

fn demo() -> SealedDemo {
    SealedDemo::new(generated_at(), PIPE)
}

// ---- the demo is the fixture ----

/// The demo's snapshot, settings pane and chat, through the hub's own
/// projections, are the committed ui-contract fixtures.
#[test]
fn the_demo_is_the_ui_contract_fixture() {
    let demo = demo();
    let now = generated_at();
    assert_same(
        "snapshot",
        &fixture("snapshot.json"),
        &demo.snapshot(now, GENERATED_AT_MS),
    );
    assert_same(
        "settings",
        &fixture("settings.json"),
        &demo.settings_snapshot(now),
    );
    assert_same(
        "chat",
        &fixture("chat.json"),
        &sealed_demo::chat("needs-permission", 1),
    );
}

/// Made at any other time, the demo is the fixture with its times moved:
/// every label is relative ("2m", "resets in 40m"), so nothing else changes.
#[test]
fn a_later_demo_is_the_fixture_shifted() {
    let delta: u64 = 86_400_000 + 37_123;
    let now_ms = GENERATED_AT_MS + delta;
    let now = from_ms(now_ms);
    let demo = SealedDemo::new(now, PIPE);
    let made = [
        (
            "snapshot.json",
            serde_json::to_value(demo.snapshot(now, now_ms)).unwrap(),
        ),
        (
            "settings.json",
            serde_json::to_value(demo.settings_snapshot(now)).unwrap(),
        ),
    ];
    for (name, got) in made {
        let mut want = fixture(name);
        shift_times(&mut want, delta);
        assert_same(name, &want, &got);
    }
}

// ---- a sealed hub over a temporary folder ----

/// A temporary folder the sealed hub is given as every root; removed after.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> TempRoot {
        let root = std::env::temp_dir().join(format!(
            "agentnotch-sealed-demo-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        TempRoot(root)
    }

    /// Everything inside, relative.
    fn contents(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                out.push(path.strip_prefix(base).unwrap().display().to_string());
                if path.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &self.0, &mut out);
        out
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sealed_hub(root: &Path, now_ms: u64) -> (Hub, Arc<Mutex<Vec<HubEvent>>>) {
    let cfg = HubConfig {
        roots: Roots::under(root),
        app_version: "1.1.0".into(),
        website: Some("https://agentnotch.rivant.in".into()),
        flags: DevFlags {
            sealed: true,
            ..DevFlags::default()
        },
        hook_exe: root.join("install").join("agentnotch-hook.exe"),
        pipe_name: PIPE.into(),
    };
    let hub = Hub::sealed(cfg, Arc::new(FakeClock::at_ms(now_ms)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    hub.on_event(Box::new(move |event| {
        sink.lock().unwrap().push(event.clone())
    }));
    (hub, events)
}

fn call(hub: &Hub, method: &str, args: Value) -> Value {
    hub.call(Call::from_parts(method, args).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

/// The sealed hub serves the demo, made at its clock's time.
#[test]
fn the_sealed_hub_serves_the_demo() {
    let root = TempRoot::new("serves");
    let (hub, events) = sealed_hub(&root.0, GENERATED_AT_MS);
    hub.start().unwrap();
    let snapshot = hub.snapshot();
    let mut want = fixture("snapshot.json");
    want["generated_at_ms"] = json!(snapshot.generated_at_ms);
    assert_same("snapshot", &want, &snapshot);
    assert_same(
        "settings",
        &fixture("settings.json"),
        &hub.settings_snapshot(),
    );
    let logs: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            HubEvent::Log(line) => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(logs, ["hub started (sealed demo)"]);
    hub.stop();
}

// ---- A3_SealedHubTests ----

fn rings() -> (String, String) {
    let accounts = demo().accounts();
    (
        accounts[0].ring_id.as_str().to_owned(),
        accounts[1].ring_id.as_str().to_owned(),
    )
}

/// What the Mac's test calls each attention state.
fn kind(state: &SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou(NeedsInputReason::Permission { .. }) => "permission",
        SessionState::NeedsYou(NeedsInputReason::Question) => "question",
        SessionState::NeedsYou(NeedsInputReason::PlanApproval) => "plan",
        SessionState::NeedsYou(NeedsInputReason::Elicitation { .. }) => "elicitation",
        SessionState::NeedsYou(NeedsInputReason::Dialog { .. }) => "dialog",
        SessionState::NeedsYou(NeedsInputReason::Error { .. }) | SessionState::Failed(_) => "error",
        SessionState::Working => "working",
        SessionState::ReadyForReview => "review",
        SessionState::Idle => "idle",
    }
}

/// A sealed hub shows the fixtures (every attention state on two accounts)
/// and touches nothing: nothing written under its roots, no terminal shown,
/// no usage check run.
#[test]
fn sealed_hub_shows_every_state_and_touches_nothing() {
    let root = TempRoot::new("states");
    let (hub, _) = sealed_hub(&root.0, GENERATED_AT_MS);
    hub.start().unwrap();
    let snapshot = hub.snapshot();
    let (personal, work) = rings();

    // Two accounts, as rings.
    let shown: Vec<(&str, &str)> = snapshot
        .rings
        .iter()
        .map(|r| (r.ring_id.as_str(), r.label.as_str()))
        .collect();
    assert_eq!(
        shown,
        [(personal.as_str(), "Personal"), (work.as_str(), "Work")]
    );

    // Every attention state, each row named by where it runs.
    let views = demo().views();
    assert_eq!(snapshot.sessions.len(), views.len());
    let kinds: BTreeSet<&str> = views.iter().map(|v| kind(&v.state)).collect();
    let want: BTreeSet<&str> = [
        "permission",
        "question",
        "plan",
        "elicitation",
        "error",
        "working",
        "review",
        "idle",
    ]
    .into();
    assert_eq!(kinds, want);
    for row in &snapshot.sessions {
        assert!(row.card.name.contains(" · "), "{}", row.session_id);
    }

    // Counts per ring and in total: the rate-limited one is failed, not
    // needs-you (GUX-2).
    let t = snapshot.totals;
    assert_eq!(
        (t.needs_you, t.failed, t.review, t.working, t.idle),
        (4, 1, 3, 3, 4)
    );
    let needs: u32 = snapshot.rings.iter().map(|r| r.counts.needs_you).sum();
    assert_eq!(needs, 4);
    assert_eq!(snapshot.tray_badge, 4);
    assert!(snapshot.resting_marks.working);

    // Readings: both rings drawn, the session window first; the work ring's
    // session window is used up.
    for ring in &snapshot.rings {
        assert_eq!(ring.usage.status, "ok", "{}", ring.ring_id);
        assert_eq!(ring.usage.windows[0].id, "session");
    }
    assert_eq!(snapshot.rings[1].usage.windows[0].used, 1.0);

    // The rate-limited session says when it can go on.
    let limited = snapshot
        .sessions
        .iter()
        .find(|r| r.session_id == "needs-ratelimit")
        .unwrap();
    assert!(limited.failed && limited.state_word == "failed");
    match &limited.detail {
        RowDetail::Failed { text } => {
            assert!(
                text.starts_with("Rate limited · 5-hour limit resets "),
                "{text}"
            )
        }
        other => panic!("{other:?}"),
    }

    // The just-finished review keeps its ring's arc pulsing for 90 s.
    let placed: Vec<SessionView> = views
        .iter()
        .map(|v| {
            let mut v = v.clone();
            v.ring = snapshot
                .sessions
                .iter()
                .find(|r| r.session_id == v.id.as_str())
                .and_then(|r| r.ring_id.as_deref())
                .map(RingId::from);
            v
        })
        .collect();
    let fresh = fresh_success_until(&placed, generated_at());
    let until = fresh[&personal];
    assert!(until > generated_at() && until <= generated_at() + FRESH_SUCCESS_WINDOW);
    assert!(!fresh.contains_key(&work));

    // A ring switched off gets no badges.
    let ring_shown =
        |on: bool| json!({"action": {"ring_shown": {"ring_id": work.as_str(), "on": on}}});
    call(&hub, "account", ring_shown(false));
    let hidden = hub.snapshot();
    let ring = hidden.rings.iter().find(|r| r.ring_id == work).unwrap();
    assert!(!ring.shown);
    assert_eq!((ring.badges.needs_you, ring.badges.review), (0, 0));
    call(&hub, "account", ring_shown(true));

    // Reviewing works on fixtures.
    call(
        &hub,
        "mark_reviewed",
        json!({"session_id": "review-darkmode", "at_ms": GENERATED_AT_MS}),
    );
    let reviewed = hub.snapshot();
    let row = reviewed
        .sessions
        .iter()
        .find(|r| r.session_id == "review-darkmode")
        .unwrap();
    assert_eq!(row.bucket, "idle");
    assert_eq!(reviewed.totals.review, 2);

    // Nothing real was touched.
    let focus = call(&hub, "focus", json!({"session_id": "review-darkmode"}));
    assert_eq!(focus["outcome"], "not_found");
    let refresh = call(
        &hub,
        "refresh_usage",
        json!({"ring_id": personal.as_str(), "reason": "manual"}),
    );
    assert_eq!(refresh["coming"], false);
    hub.stop();
    assert!(root.contents().is_empty(), "{:?}", root.contents());
}

/// A third account signs in while running (the script's second step): its
/// ring, its sessions and its reading arrive, and nothing else changes.
#[test]
fn a_third_account_arrives_later() {
    let now = generated_at();
    let mut demo = demo();
    let before = demo.snapshot(now, GENERATED_AT_MS);
    assert_eq!(before.rings.len(), 2);

    demo.apply(SealedDemoStep::AddThirdAccount, now);
    let after = demo.snapshot(now, GENERATED_AT_MS + 1);
    assert_eq!(after.rings.len(), 3);
    // Rings are listed by name: Personal, Side project, Work.
    let side = &after.rings[1];
    assert_eq!(side.label, "Side project");
    assert_eq!(side.usage.status, "ok");
    assert_eq!(after.sessions.len(), before.sessions.len() + 2);
    assert_eq!(
        (side.counts.review, side.counts.idle, side.counts.working),
        (1, 1, 0)
    );
    for old in &before.rings {
        let new = after
            .rings
            .iter()
            .find(|r| r.ring_id == old.ring_id)
            .unwrap();
        assert_eq!(old.counts, new.counts, "{}", old.ring_id);
    }
    // Its review finished 85 s ago: the arc settles five seconds later.
    assert_eq!(side.activity, RingActivity::Success);
    assert_eq!(
        side.success_settles_at_ms,
        Some(to_ms(now + FRESH_SUCCESS_WINDOW - THIRD_ACCOUNT_REVIEW_AGE))
    );
    assert_eq!(demo.settings_snapshot(now).accounts.len(), 3);
}

// ---- C_SealedDemoScriptTests ----

fn script_now() -> SystemTime {
    from_ms(1_800_000_000_000)
}

fn fixtures() -> (Vec<Folder>, Vec<Session>) {
    let demo = demo();
    (
        demo.registry().known_folders().to_vec(),
        demo.sessions().to_vec(),
    )
}

fn attention(sessions: &[Session]) -> BTreeMap<String, SessionState> {
    sessions
        .iter()
        .map(|s| (s.id.as_str().to_owned(), s.attention()))
        .collect()
}

/// The sessions whose attention changed, by id.
fn moved(before: &[Session], after: &[Session]) -> Vec<String> {
    let (a, b) = (attention(before), attention(after));
    a.keys().filter(|id| a[*id] != b[*id]).cloned().collect()
}

fn views(sessions: &[Session]) -> Vec<SessionView> {
    sessions.iter().map(Session::to_view).collect()
}

fn of(sessions: &[Session], id: &str) -> Session {
    sessions
        .iter()
        .find(|s| s.id.as_str() == id)
        .unwrap()
        .clone()
}

#[test]
fn the_third_account_arrives_with_a_review_and_an_idle_session() {
    let (folders, sessions) = fixtures();
    let now = script_now();
    let (new_folders, new_sessions) =
        apply_step(SealedDemoStep::AddThirdAccount, &folders, &sessions, now);
    assert_eq!(new_folders.len(), folders.len() + 1);
    let side = new_folders.last().unwrap();
    assert_eq!(side.dir(), sealed_demo::SIDE_DIR);
    assert!(side.identity.as_ref().unwrap().email.is_some());

    let added: Vec<&Session> = new_sessions
        .iter()
        .filter(|s| s.account.as_ref().map(|a| a.as_str()) == Some(side.dir()))
        .collect();
    let states: Vec<SessionState> = added.iter().map(|s| s.attention()).collect();
    assert_eq!(states, [SessionState::ReadyForReview, SessionState::Idle]);
    // Finished long enough ago to settle a few seconds after it appears.
    assert_eq!(added[0].completed_at, Some(now - THIRD_ACCOUNT_REVIEW_AGE));
    assert!(THIRD_ACCOUNT_REVIEW_AGE < FRESH_SUCCESS_WINDOW);
    assert!(FRESH_SUCCESS_WINDOW - THIRD_ACCOUNT_REVIEW_AGE <= Duration::from_secs(8));

    // Nobody else changed.
    assert_eq!(views(&new_sessions[..sessions.len()]), views(&sessions));

    // A second time is a no-op.
    let (again_folders, again_sessions) = apply_step(
        SealedDemoStep::AddThirdAccount,
        &new_folders,
        &new_sessions,
        now,
    );
    assert_eq!(again_folders, new_folders);
    assert_eq!(views(&again_sessions), views(&new_sessions));
}

#[test]
fn the_third_account_is_a_new_ring() {
    let paths = sealed_demo::paths();
    let ring = ring_id_for_config_dir(&paths, sealed_demo::SIDE_DIR);
    assert_eq!(ring.as_str(), "claude-side");
    for folder in sealed_demo::folders(&paths) {
        assert_ne!(ring_id_for_config_dir(&paths, folder.dir()), ring);
    }
    // Signed in, it is a ring of its own identity, beside the others'.
    let mut demo = demo();
    demo.apply(SealedDemoStep::AddThirdAccount, generated_at());
    let rings: Vec<String> = demo
        .accounts()
        .iter()
        .map(|a| a.ring_id.as_str().to_owned())
        .collect();
    let side = demo
        .accounts()
        .into_iter()
        .find(|a| a.label == "Side project")
        .unwrap();
    assert_eq!(rings.len(), 3);
    assert!(side.ring_id.as_str().starts_with("claude-acct-"));
    assert_eq!(
        rings.iter().filter(|r| *r == side.ring_id.as_str()).count(),
        1
    );
}

#[test]
fn two_sessions_finish_together() {
    let (folders, sessions) = fixtures();
    let now = script_now();
    let (_, after) = apply_step(SealedDemoStep::FinishTwoSessions, &folders, &sessions, now);
    let ids = moved(&sessions, &after);
    assert_eq!(ids, ["work-ci", "work-summary"]);
    for id in &ids {
        assert_eq!(of(&sessions, id).attention(), SessionState::Working);
        let finished = of(&after, id);
        assert_eq!(finished.attention(), SessionState::ReadyForReview);
        assert_eq!(finished.completed_at, Some(now));
    }
    // The two are on different rings: one burst across two rings.
    let accounts: BTreeSet<String> = ids
        .iter()
        .filter_map(|id| of(&after, id).account.map(|a| a.as_str().to_owned()))
        .collect();
    assert_eq!(accounts.len(), 2);
}

#[test]
fn a_working_session_asks_for_permission() {
    let (folders, sessions) = fixtures();
    let (_, after) = apply_step(
        SealedDemoStep::AskPermission,
        &folders,
        &sessions,
        script_now(),
    );
    assert_eq!(moved(&sessions, &after), ["work-migration"]);
    let asking = of(&after, "work-migration");
    assert_eq!(asking.attention().bucket(), Bucket::NeedsYou);
    assert_eq!(asking.active_permission().unwrap().tool_name, "Edit");
}

/// The work ring stops asking and gets back to work, so the demo shows an
/// asking ring (personal) beside a spinning one (work). Only the two
/// answered sessions move, and they move into working.
#[test]
fn answering_the_work_prompts_leaves_only_the_personal_ring_asking() {
    let (folders, sessions) = fixtures();
    let now = script_now();
    let (_, after) = apply_step(SealedDemoStep::AnswerWorkPrompts, &folders, &sessions, now);
    let ids = moved(&sessions, &after);
    assert_eq!(ids, ["needs-permission", "needs-ratelimit"]);
    for id in &ids {
        assert_eq!(of(&sessions, id).attention().bucket(), Bucket::NeedsYou);
        let answered = of(&after, id);
        assert_eq!(answered.attention(), SessionState::Working);
        assert_eq!(answered.turn_started_at, Some(now));
    }
    let work = sealed_demo::paths().normalize(sealed_demo::WORK_DIR);
    let of_work = |s: &&Session| s.account.as_ref().map(|a| a.as_str()) == Some(work.as_str());
    let asking: Vec<&Session> = after
        .iter()
        .filter(|s| s.attention().bucket() == Bucket::NeedsYou)
        .collect();
    assert!(!asking.is_empty());
    assert!(!asking.iter().any(of_work));
    assert!(after
        .iter()
        .filter(of_work)
        .any(|s| s.attention() == SessionState::Working));
}

/// Played in order, the last step early enough for a sealed run of ten
/// seconds, and the third ring at three seconds, settling before the run
/// ends.
#[test]
fn the_timeline_fits_a_ten_second_run() {
    let times: Vec<Duration> = SealedDemoStep::ALL
        .iter()
        .map(|s| s.after_launch())
        .collect();
    let mut sorted = times.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, times);
    assert_eq!(
        SealedDemoStep::AddThirdAccount.after_launch(),
        Duration::from_secs(3)
    );
    assert!(*times.last().unwrap() < Duration::from_secs(8));
    let settles = SealedDemoStep::AddThirdAccount.after_launch() + FRESH_SUCCESS_WINDOW
        - THIRD_ACCOUNT_REVIEW_AGE;
    assert!(settles < Duration::from_secs(10));
    // Further apart than a burst window, so each step is a burst of its own.
    for pair in times.windows(2) {
        assert!(pair[1] - pair[0] > BURST_WINDOW);
    }
}

/// Steps change nothing once applied, so a replayed step changes nothing.
#[test]
fn steps_do_not_repeat() {
    let (mut folders, mut sessions) = fixtures();
    let now = script_now();
    for step in SealedDemoStep::ALL {
        (folders, sessions) = apply_step(step, &folders, &sessions, now);
    }
    for step in SealedDemoStep::ALL {
        let (again_folders, again_sessions) = apply_step(step, &folders, &sessions, now);
        assert_eq!(again_folders, folders, "{step:?}");
        assert_eq!(views(&again_sessions), views(&sessions), "{step:?}");
    }
}

// ---- A1 / A2 ----

/// A sealed answer changes the fixture only: the session goes back to work
/// and nothing is written (no pipe, no file under the roots).
#[test]
fn a_sealed_answer_changes_the_fixture_only() {
    let root = TempRoot::new("answer");
    let (hub, _) = sealed_hub(&root.0, GENERATED_AT_MS);
    hub.start().unwrap();
    let reply = call(
        &hub,
        "answer",
        json!({"session_id": "needs-permission", "tool_use_id": "toolu_sample_bash",
               "answer": {"allow": {"always": false}}}),
    );
    assert_eq!(reply["result"], "delivered");
    let snapshot = hub.snapshot();
    let row = snapshot
        .sessions
        .iter()
        .find(|r| r.session_id == "needs-permission")
        .unwrap();
    assert!(row.pending.is_none());
    assert_eq!(row.bucket, "working");
    hub.stop();
    assert!(root.contents().is_empty(), "{:?}", root.contents());
}

/// Sealed: no consent card, nothing found, no folder to edit, no
/// suggestion, and every action on folders is refused as sealed.
#[test]
fn setup_actions_do_nothing_when_sealed() {
    let root = TempRoot::new("setup");
    let (hub, _) = sealed_hub(&root.0, GENERATED_AT_MS);
    hub.start().unwrap();
    let setup = hub.snapshot().setup;
    assert!(!setup.needs_hook_consent);
    assert!(setup.consent_files.is_empty());
    assert!(setup.codenotch_hooks_folders.is_empty());
    assert!(setup.new_install_folders.is_empty());
    assert!(setup.missing_hooks_accounts.is_empty());
    assert!(!setup.install_disabled, "no --no-install banner");
    let settings = hub.settings_snapshot();
    assert!(settings.suggestions.is_empty());
    assert!(!settings.hooks.install_allowed);
    for action in [
        json!({"add_folder": {"path": r"C:\Users\me\.claude-x"}}),
        json!({"create": {"name": "x"}}),
        json!({"forget": {"id": settings.accounts[1].identity_id}}),
        json!({"suggestion_add": {"path": r"~\.claude-old"}}),
    ] {
        let answer = hub.call(Call::from_parts("account", json!({ "action": action })).unwrap());
        assert_eq!(answer.map_err(|e| e.code), Err("sealed".to_owned()));
    }
    // A consent answer is kept in memory and still shows no card.
    call(&hub, "hook_consent", json!({"grant": true}));
    assert!(!hub.snapshot().setup.needs_hook_consent);
    hub.stop();
    assert!(root.contents().is_empty(), "{:?}", root.contents());
}

/// Every documented call, on one sealed hub over a temporary folder: the
/// folder stays empty through start, the calls and stop (twice), a deep
/// link is ignored, and every snapshot pushed is newer than the last.
#[test]
fn every_call_leaves_the_roots_empty() {
    let root = TempRoot::new("calls");
    let (hub, events) = sealed_hub(&root.0, GENERATED_AT_MS);
    hub.start().unwrap();
    for case in fixture("calls.json").as_array().unwrap() {
        let raw = &case["call"];
        let method = raw["method"].as_str().unwrap();
        let args = raw.get("args").cloned().unwrap_or(Value::Null);
        let _ = hub.call(Call::from_parts(method, args).unwrap());
    }
    assert!(matches!(
        hub.handle_deep_link("agentnotch://auth-callback?code=x"),
        DeepLinkOutcome::Ignored(_)
    ));
    hub.stop();
    hub.stop();
    assert!(root.contents().is_empty(), "{:?}", root.contents());
    let times: Vec<u64> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            HubEvent::Snapshot(s) => Some(s.generated_at_ms),
            _ => None,
        })
        .collect();
    assert!(times.len() > 2);
    assert!(times.windows(2).all(|pair| pair[0] < pair[1]), "{times:?}");
}
