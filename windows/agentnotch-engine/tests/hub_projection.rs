//! The hub's snapshot projection (`hub::project`, WP7): ports of
//! A3_HubProjectionTests (counts, "just finished", paused rings),
//! PP_ProjectionTests (the ring a session lands on), A3_RingReadingTests (the
//! ring reading's windows and statuses as the pages and upstream read them),
//! Fix_AccountOwnLabelTests, Fix_ForgottenAccountTests, and the projection
//! rules of A3_ReviewTests; plus the upstream usage projection and the
//! snapshot clock.
//!
//! The Swift attention transitions (A3_TransitionsBaselineTests, the
//! crossings of A3_ReviewTests) are `attention::news`/`tracker` (WP5/WP6,
//! `tests/attention_tracker.rs`); `leaving_a_wait_is_resolved_before_the_next_crossing`
//! below pins the A3_ReviewTests case on the rule itself.

mod accounts_support;
mod hub_support;
mod usage_support;

use agentnotch_engine::attention::news::{kinds_between, NewsKind};
use agentnotch_engine::hub::project::{
    default_ring_id, fresh_success_until, next_boundary, paused_account_ids, ring_counts, ring_id,
    upstream_usage, SnapshotClock, FRESH_SUCCESS_WINDOW, SIGN_IN_NOTE, WAITING_NOTE,
};
use agentnotch_engine::model::{
    AccountUsage, Attribution, Counts, ExtraUsage, IdentityId, NeedsInputReason, RingActivity,
    RingStatus, SessionState, UsageSource, UsageWindow,
};
use agentnotch_engine::runtime_types::RingReading;
use agentnotch_engine::usage::{UsageStore, UsageStoreConfig};
use hub_support::{account, at, attributed, now, now_ms, on_ring, session, Kind, World};
use std::collections::BTreeSet;
use std::time::Duration;

fn counts(needs_you: u32, review: u32, working: u32, idle: u32, failed: u32) -> Counts {
    Counts {
        needs_you,
        failed,
        review,
        working,
        idle,
    }
}

// ---- A3_HubProjectionTests: counts, "just finished", paused rings ----

#[test]
fn counts_per_ring_and_total() {
    let views = vec![
        on_ring(session("a", "/h", Kind::Permission), "claude"),
        on_ring(session("b", "/h", Kind::Working), "claude"),
        on_ring(session("c", "/h", Kind::Review(600)), "claude"),
        on_ring(session("d", "/h", Kind::Failed), "claude-work"),
        on_ring(session("e", "/h", Kind::Idle), "claude-work"),
        on_ring(session("f", "/h", Kind::Working), "claude-work"),
    ];
    let rings: Vec<String> = ["claude", "claude-work", "claude-side"]
        .map(String::from)
        .to_vec();
    let by_ring = ring_counts(&views, &rings);
    assert_eq!(by_ring["claude"], counts(1, 1, 1, 0, 0));
    // A failed turn counts as failed, not as needing you (GUX-2).
    assert_eq!(by_ring["claude-work"], counts(0, 0, 1, 1, 1));
    // A ring with no sessions is there, at zero, so its badges clear.
    assert_eq!(by_ring["claude-side"], Counts::default());
    let total = agentnotch_engine::attention::policy::total(by_ring.values());
    assert_eq!(total, counts(1, 1, 2, 1, 1));
}

#[test]
fn fresh_success_lasts_ninety_seconds_per_ring() {
    let views = vec![
        on_ring(session("new", "/h", Kind::Review(20)), "claude"),
        on_ring(session("older", "/h", Kind::Review(80)), "claude"),
        on_ring(session("old", "/h", Kind::Review(91)), "claude-work"),
        on_ring(session("working", "/h", Kind::Working), "claude-side"),
    ];
    let fresh = fresh_success_until(&views, now());
    // The newest completion on the ring decides.
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh["claude"], at(70));
    assert_eq!(FRESH_SUCCESS_WINDOW, Duration::from_secs(90));
    // The boundary: exactly 90 s after the completion it has settled.
    let boundary = at(70);
    let just_before = boundary - Duration::from_millis(1);
    assert_eq!(fresh_success_until(&views, just_before)["claude"], boundary);
    assert!(fresh_success_until(&views, boundary).is_empty());
}

#[test]
fn rings_switched_off_pause_their_accounts() {
    let mut off = account("uuid:b", "claude-acct-b", "B", false);
    off.ring_shown = false;
    let mut untracked = account("uuid:c", "claude-acct-c", "C", false);
    untracked.is_tracked = false;
    let on = account("uuid:a", "claude-acct-a", "A", true);
    let paused = paused_account_ids(&[on.clone(), off, untracked]);
    let expected: BTreeSet<IdentityId> = ["uuid:b", "uuid:c"].map(IdentityId::from).into();
    assert_eq!(paused, expected);
    assert!(paused_account_ids(&[on]).is_empty());
}

// ---- PP_ProjectionTests / A3_ReviewTests: the ring a session lands on ----

#[test]
fn the_default_ring_is_the_account_the_default_folder_runs_as() {
    let mut off = account("uuid:b", "claude-acct-b", "B", false);
    off.is_tracked = false;
    let side = account("uuid:s", "claude-acct-s", "S", false);
    let main = account("uuid:a", "claude-acct-a", "A", true);
    // The default account when it is tracked.
    assert_eq!(
        default_ring_id(&[side.clone(), main.clone()]),
        "claude-acct-a"
    );
    // Else the first tracked one.
    assert_eq!(default_ring_id(&[off.clone(), side]), "claude-acct-s");
    // Else the fallback.
    assert_eq!(default_ring_id(&[off]), "claude");
    assert_eq!(default_ring_id(&[]), "claude");
}

#[test]
fn sessions_of_unknown_accounts_go_to_the_default_ring() {
    let known: BTreeSet<String> = ["claude", "claude-work"].map(String::from).into();
    assert_eq!(ring_id("claude-work", &known, "claude"), "claude-work");
    assert_eq!(ring_id("claude-new", &known, "claude"), "claude");
    assert_eq!(
        ring_id("claude-dir-0badf00d", &BTreeSet::new(), "claude"),
        "claude"
    );
}

#[test]
fn a_folder_the_registry_does_not_know_lists_on_the_default_ring() {
    let mut world = World::new(vec![
        account("uuid:a", "claude-acct-a", "Main", true),
        account("uuid:w", "claude-acct-w", "Work", false),
    ]);
    world.directory.default_folder = "/h/.claude".into();
    world
        .directory
        .identities
        .insert("/h/.claude-work".into(), "uuid:w".into());
    world.sessions = vec![
        attributed(
            session("known", "/h/.claude-work", Kind::Working),
            Attribution::Known(Some(IdentityId::from("uuid:w"))),
        ),
        // A folder seen in a hook before the registry knows it.
        attributed(
            session("new", "/h/.claude-new", Kind::Working),
            Attribution::Known(None),
        ),
        // No folder at all: where `~\.claude` runs.
        attributed(
            {
                let mut view = session("bare", "/h/.claude", Kind::Idle);
                view.account = None;
                view
            },
            Attribution::Known(None),
        ),
    ];
    let snapshot = world.project();
    let ring_of = |id: &str| {
        snapshot
            .sessions
            .iter()
            .find(|row| row.session_id == id)
            .and_then(|row| row.ring_id.clone())
    };
    assert_eq!(ring_of("known").as_deref(), Some("claude-acct-w"));
    assert_eq!(ring_of("new").as_deref(), Some("claude-acct-a"));
    assert_eq!(ring_of("bare").as_deref(), Some("claude-acct-a"));
    let ring = |id: &str| snapshot.rings.iter().find(|r| r.ring_id == id).unwrap();
    assert_eq!(ring("claude-acct-a").counts, counts(0, 0, 1, 1, 0));
    assert_eq!(ring("claude-acct-w").counts, counts(0, 0, 1, 0, 0));
    assert_eq!(snapshot.totals, counts(0, 0, 2, 1, 0));
}

#[test]
fn a_guess_places_an_unsure_session_and_waiting_ones_fall_back_to_their_folder() {
    let mut world = World::new(vec![
        account("uuid:a", "claude-acct-a", "Main", true),
        account("uuid:w", "claude-acct-w", "Work", false),
    ]);
    world
        .directory
        .identities
        .insert("/h/.claude-x".into(), "uuid:a".into());
    world.sessions = vec![
        attributed(
            session("guessed", "/h/.claude-x", Kind::Idle),
            Attribution::Unsure(Some(IdentityId::from("uuid:w"))),
        ),
        attributed(
            session("waiting", "/h/.claude-x", Kind::Idle),
            Attribution::Waiting,
        ),
    ];
    let snapshot = world.project();
    let ring_of = |id: &str| {
        snapshot
            .sessions
            .iter()
            .find(|row| row.session_id == id)
            .and_then(|row| row.ring_id.clone())
    };
    assert_eq!(ring_of("guessed").as_deref(), Some("claude-acct-w"));
    assert_eq!(ring_of("waiting").as_deref(), Some("claude-acct-a"));
}

#[test]
fn the_panel_lists_no_session_of_a_forgotten_account() {
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "Main", true)]);
    world.directory.default_folder = "/h/.claude".into();
    world
        .directory
        .identities
        .insert("/h/.claude".into(), "uuid:a".into());
    world.directory.forgotten.insert("/h/.claude-temp".into());
    world.sessions = vec![
        attributed(
            session("kept", "/h/.claude", Kind::Working),
            Attribution::Known(Some(IdentityId::from("uuid:a"))),
        ),
        attributed(
            session("gone", "/h/.claude-temp", Kind::Working),
            Attribution::Known(None),
        ),
    ];
    let snapshot = world.project();
    let ids: Vec<&str> = snapshot
        .sessions
        .iter()
        .map(|row| row.session_id.as_str())
        .collect();
    assert_eq!(ids, ["kept"]);
    // Nor does it count anywhere.
    assert_eq!(snapshot.totals, counts(0, 0, 1, 0, 0));
    // A forgotten identity too (certain attribution).
    world.directory.forgotten.insert("uuid:a".into());
    assert!(world.project().sessions.is_empty());
}

#[test]
fn sessions_of_a_switched_off_account_are_listed_nowhere() {
    let mut off = account("uuid:b", "claude-acct-b", "B", false);
    off.is_tracked = false;
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true), off]);
    world.sessions = vec![attributed(
        session("hidden", "/h/.claude-b", Kind::Permission),
        Attribution::Known(Some(IdentityId::from("uuid:b"))),
    )];
    let snapshot = world.project();
    assert!(snapshot.sessions.is_empty());
    assert_eq!(snapshot.rings.len(), 1, "no ring for an untracked account");
    assert_eq!(snapshot.tray_badge, 0);
}

// The Claude Parallel Profiles layout through the real registry.
#[test]
fn sessions_run_in_a_window_are_on_its_accounts_ring() {
    let home = accounts_support::Home::new();
    home.build_user_layout(true, true);
    let registry = home.registry();
    let accounts = registry.accounts();
    assert_eq!(accounts.len(), 2);
    let paras = accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("paras@rivant.in"))
        .expect("paras");
    let biios = accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("claude@biios.in"))
        .expect("biios");
    assert_ne!(paras.ring_id, biios.ring_id);

    let mut world = World::new(accounts.clone());
    let place = |relative: &str, id: &str| {
        let folder = home.id(relative);
        let mut view = session(id, folder.as_str(), Kind::Working);
        view.attribution = registry.attribution(&folder, None);
        view
    };
    world.sessions = vec![
        place(".claude-windows/801f9dd51396", "paras-window"),
        place(".claude-windows/1bf3e8f92b11", "biios-window"),
        place(".claude", "default"),
        place(".claude-unseen", "unseen"),
    ];
    // The same registry answers the projection's questions.
    let extras = |_: &agentnotch_engine::model::SessionView| Default::default();
    let snapshot = agentnotch_engine::hub::project::project(
        &agentnotch_engine::hub::project::ProjectionInput {
            now: now(),
            accounts: &accounts,
            directory: &registry,
            readings: &world.readings,
            sessions: &world.sessions,
            extras: &extras,
            clock: Default::default(),
            ui: &world.ui,
            setup: &world.setup,
            sealed: false,
        },
        now_ms(),
    );
    let ring_of = |id: &str| {
        snapshot
            .sessions
            .iter()
            .find(|row| row.session_id == id)
            .and_then(|row| row.ring_id.clone())
            .unwrap()
    };
    assert_eq!(ring_of("paras-window"), paras.ring_id.as_str());
    assert_eq!(ring_of("biios-window"), biios.ring_id.as_str());
    assert_eq!(ring_of("default"), paras.ring_id.as_str());
    // A folder nobody has seen: the default ring (`~\.claude` runs as paras).
    assert_eq!(ring_of("unseen"), paras.ring_id.as_str());
    assert_eq!(default_ring_id(&accounts), paras.ring_id.as_str());
    assert!(snapshot.accounts_multi);

    // A forgotten folder's sessions are listed nowhere.
    home.mkdir(".claude-temp");
    let mut registry = home.registry();
    let (temp, _) = registry.add_folder(&home.path(".claude-temp")).unwrap();
    registry.remove(temp.as_str());
    let accounts = registry.accounts();
    let sessions = vec![
        place(".claude", "kept"),
        attributed(
            session("gone", temp.as_str(), Kind::Working),
            registry.attribution(&temp, None),
        ),
    ];
    let snapshot = agentnotch_engine::hub::project::project(
        &agentnotch_engine::hub::project::ProjectionInput {
            now: now(),
            accounts: &accounts,
            directory: &registry,
            readings: &world.readings,
            sessions: &sessions,
            extras: &extras,
            clock: Default::default(),
            ui: &world.ui,
            setup: &world.setup,
            sealed: false,
        },
        now_ms(),
    );
    let listed: Vec<&str> = snapshot
        .sessions
        .iter()
        .map(|row| row.session_id.as_str())
        .collect();
    assert_eq!(listed, ["kept"]);
}

// ---- Fix_AccountOwnLabelTests ----

#[test]
fn a_ring_carries_the_accounts_label_and_the_nickname_wins() {
    let mut named = account("uuid:a", "claude-acct-a", "research", true);
    named.own_label = Some("research".into());
    let world = World::new(vec![named.clone()]);
    assert_eq!(world.project().rings[0].label, "research");
    // A nickname replaces `label`; the own name stays on the account.
    named.label = "Lab".into();
    let world = World::new(vec![named]);
    let ring = &world.project().rings[0];
    assert_eq!(ring.label, "Lab");
    assert!(ring.a11y.starts_with("Lab"));
}

// ---- A3_ReviewTests: when the projection changes by itself ----

fn reading_with(usage: AccountUsage, status: RingStatus) -> RingReading {
    RingReading::Reading {
        stale_after: usage.updated_at + Duration::from_secs(900),
        usage,
        status,
    }
}

fn window(utilization: f64, resets_in: i64, duration: u64) -> UsageWindow {
    UsageWindow::new(utilization, Some(at(resets_in)), duration)
}

fn full_usage(id: &str) -> AccountUsage {
    let mut usage = AccountUsage::new(IdentityId::from(id), UsageSource::Probe, at(-90));
    usage.five_hour = Some(window(34.0, 3600, UsageWindow::SESSION_DURATION_S));
    usage.seven_day = Some(window(41.0, 3 * 86_400, UsageWindow::WEEKLY_DURATION_S));
    usage.scoped = vec![
        (
            "Sonnet 4.5".into(),
            window(12.0, 3 * 86_400, UsageWindow::WEEKLY_DURATION_S),
        ),
        (
            "Opus".into(),
            window(25.0, 3 * 86_400, UsageWindow::WEEKLY_DURATION_S),
        ),
    ];
    usage.extra_usage = Some(ExtraUsage {
        is_enabled: true,
        monthly_limit: Some(5_000.0),
        used_credits: Some(1_240.0),
        utilization: Some(24.8),
        currency: Some("usd".into()),
    });
    usage.subscription_type = Some("max".into());
    usage
}

#[test]
fn the_hub_republishes_when_a_window_resets_or_an_arc_settles() {
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    let mut usage = AccountUsage::new(IdentityId::from("uuid:a"), UsageSource::Probe, now());
    usage.five_hour = Some(window(100.0, 600, UsageWindow::SESSION_DURATION_S));
    usage.seven_day = Some(window(40.0, 86_400, UsageWindow::WEEKLY_DURATION_S));
    // A model's week that reset 5 s ago reads 0% and is no boundary.
    usage.scoped = vec![(
        "Opus".into(),
        window(10.0, -5, UsageWindow::WEEKLY_DURATION_S),
    )];
    world.readings.insert(
        IdentityId::from("uuid:a"),
        reading_with(usage, RingStatus::Ok),
    );
    // A used-up session window lifts in 10 minutes: republish then.
    let snapshot = world.project();
    assert_eq!(next_boundary(&snapshot, now()), Some(at(600)));

    // A ring settling sooner comes first.
    world
        .directory
        .identities
        .insert("/h".into(), "uuid:a".into());
    world.sessions = vec![attributed(
        session("done", "/h", Kind::Review(60)),
        Attribution::Known(Some(IdentityId::from("uuid:a"))),
    )];
    let snapshot = world.project();
    assert_eq!(snapshot.rings[0].activity, RingActivity::Success);
    assert_eq!(
        snapshot.rings[0].success_settles_at_ms,
        Some(now_ms() + 30_000)
    );
    assert_eq!(next_boundary(&snapshot, now()), Some(at(30)));
    // Once it settled (the arc is steady) it is no boundary any more.
    assert_eq!(next_boundary(&snapshot, at(31)), Some(at(600)));

    // Nothing ahead: nothing scheduled.
    let mut quiet = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    quiet
        .readings
        .insert(IdentityId::from("uuid:a"), RingReading::Waiting);
    assert_eq!(next_boundary(&quiet.project(), now()), None);
}

#[test]
fn leaving_a_wait_is_resolved_before_the_next_crossing() {
    let question = SessionState::NeedsYou(NeedsInputReason::Question);
    let bash = SessionState::NeedsYou(NeedsInputReason::Permission {
        tool: Some("Bash".into()),
    });
    let kinds = |from: Option<&SessionState>, to: Option<&SessionState>| {
        kinds_between(from, to, false, None, None)
    };
    assert_eq!(
        kinds(Some(&SessionState::ReadyForReview), Some(&question)),
        [NewsKind::Resolved, NewsKind::NeedsInput]
    );
    assert_eq!(
        kinds(Some(&question), Some(&SessionState::Working)),
        [NewsKind::Resolved]
    );
    assert_eq!(
        kinds(Some(&bash), Some(&SessionState::Idle)),
        [NewsKind::Resolved]
    );
}

// ---- A3_RingReadingTests: a reading as the ring and upstream read it ----

#[test]
fn ids_labels_and_order_mirror_upstream() {
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    world.readings.insert(
        IdentityId::from("uuid:a"),
        reading_with(full_usage("uuid:a"), RingStatus::Ok),
    );
    let ring = &world.project().rings[0];
    let ids: Vec<&str> = ring.usage.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "session",
            "weekly_all",
            "extra_usage",
            "weekly_opus",
            "weekly_sonnet_4_5"
        ]
    );
    let label = |id: &str| {
        ring.usage
            .windows
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.label.clone())
            .unwrap()
    };
    // Upstream names the two standard windows itself.
    assert_eq!(label("session"), "Current session");
    assert_eq!(label("weekly_all"), "All models");
    assert_eq!(label("weekly_opus"), "Opus");
    assert_eq!(label("weekly_sonnet_4_5"), "Sonnet 4.5");
    assert_eq!(label("extra_usage"), "Extra usage");
    let session = &ring.usage.windows[0];
    assert!((session.used - 0.34).abs() < 1e-9);
    assert_eq!(session.resets_at, Some(now_ms() + 3_600_000));
    let extra = &ring.usage.windows[2];
    assert!((extra.used - 0.248).abs() < 1e-9);
    assert_eq!(extra.resets_at, None);
    assert_eq!(ring.usage.status, "ok");
    assert!(!ring.usage.stale);
    assert_eq!(ring.usage.fetched_at_ms, now_ms() - 90_000);
}

#[test]
fn a_reset_window_reads_zero_and_a_used_up_one_is_clamped() {
    let mut usage = full_usage("uuid:a");
    usage.five_hour = Some(window(88.0, -60, UsageWindow::SESSION_DURATION_S));
    usage.seven_day = Some(window(104.0, 86_400, UsageWindow::WEEKLY_DURATION_S));
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    world.readings.insert(
        IdentityId::from("uuid:a"),
        reading_with(usage, RingStatus::Ok),
    );
    let ring = &world.project().rings[0];
    assert_eq!(ring.usage.windows[0].used, 0.0);
    assert_eq!(ring.usage.windows[1].used, 1.0);
}

#[test]
fn the_five_statuses_become_the_rings_usage() {
    let status = |reading: Option<RingReading>| {
        let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
        if let Some(reading) = reading {
            world.readings.insert(IdentityId::from("uuid:a"), reading);
        }
        world.project().rings.remove(0).usage
    };
    let usage = status(Some(reading_with(full_usage("uuid:a"), RingStatus::Stale)));
    assert_eq!((usage.status.as_str(), usage.stale), ("stale", true));
    assert!(!usage.windows.is_empty());
    for waiting in [status(None), status(Some(RingReading::Waiting))] {
        assert_eq!(waiting.status, "waiting");
        assert!(waiting.windows.is_empty() && !waiting.stale);
        assert_eq!(waiting.fetched_at_ms, 0);
    }
    let signed_out = status(Some(RingReading::SignInNeeded));
    assert_eq!(
        (signed_out.status.as_str(), signed_out.note.as_str()),
        ("sign_in_needed", "Not signed in to Claude")
    );
    let off = status(Some(RingReading::Unavailable(
        "Usage probes are off".into(),
    )));
    assert_eq!(
        (off.status.as_str(), off.note.as_str()),
        ("unavailable", "Usage probes are off")
    );
    let failed = status(Some(RingReading::Failed("Claude Code not found".into())));
    assert_eq!(
        (failed.status.as_str(), failed.note.as_str()),
        ("failed", "Claude Code not found")
    );
}

/// A store holding one reading, and the ring it projects to.
fn project_store_reading(
    config: UsageStoreConfig,
    usage: AccountUsage,
) -> agentnotch_engine::model::RingUsage {
    let home = usage_support::join(&usage_support::fake_drive(), &["home"]);
    let dir = home.join(".claude-work");
    let folder = usage_support::run_folder(&dir, Some(&dir.to_string_lossy()));
    let account_model = usage_support::account("uuid:acc-1", Some("me@x.dev"), &[&folder], &[]);
    let mut store = UsageStore::with_config(UsageStoreConfig { home, ..config });
    store.start();
    store.set_accounts(
        std::slice::from_ref(&account_model),
        std::slice::from_ref(&folder),
        now(),
    );
    store.accept_snapshot(usage, now());
    let id = IdentityId::from("uuid:acc-1");
    let mut world = World::new(vec![account_model]);
    world
        .readings
        .insert(id.clone(), store.ring_reading(&id, now()));
    world.project().rings.remove(0).usage
}

fn aged_usage(age: Duration, seven_day: f64) -> AccountUsage {
    let mut usage = AccountUsage::new(
        IdentityId::from("uuid:acc-1"),
        UsageSource::Probe,
        now() - age,
    );
    usage.five_hour = Some(window(10.0, 3600, UsageWindow::SESSION_DURATION_S));
    usage.seven_day = Some(window(
        seven_day,
        2 * 86_400,
        UsageWindow::WEEKLY_DURATION_S,
    ));
    usage
}

#[test]
fn stale_follows_the_engines_rule() {
    let probing = |minutes| UsageStoreConfig {
        probes_allowed: true,
        probe_interval_minutes: minutes,
        reads_desktop: false,
        ..UsageStoreConfig::default()
    };
    let minutes = |m: u64| Duration::from_secs(m * 60);
    // Probes every 5 minutes: 15 minutes is the threshold.
    assert!(project_store_reading(probing(5), aged_usage(minutes(20), 30.0)).stale);
    assert!(!project_store_reading(probing(5), aged_usage(minutes(10), 30.0)).stale);
    // At the 30-minute setting the threshold is 45: 20 minutes is fresh.
    assert!(!project_store_reading(probing(30), aged_usage(minutes(20), 30.0)).stale);
    assert!(project_store_reading(probing(30), aged_usage(minutes(50), 30.0)).stale);
    // With probes off only an hour without news is stale.
    let off = UsageStoreConfig {
        probes_allowed: false,
        reads_desktop: false,
        ..UsageStoreConfig::default()
    };
    assert!(!project_store_reading(off.clone(), aged_usage(minutes(50), 30.0)).stale);
    assert!(project_store_reading(off.clone(), aged_usage(minutes(70), 30.0)).stale);
    // A used-up window never goes stale before its reset.
    let exhausted =
        project_store_reading(probing(5), aged_usage(Duration::from_secs(3 * 3600), 100.0));
    assert_eq!(exhausted.status, "ok");
    assert!(!exhausted.stale);
}

// ---- badges, marks, totals, activity ----

fn two_rings() -> World {
    let mut world = World::new(vec![
        account("uuid:a", "claude-acct-a", "Personal", true),
        account("uuid:w", "claude-acct-w", "Work", false),
    ]);
    world
        .directory
        .identities
        .insert("/h/.claude".into(), "uuid:a".into());
    world
        .directory
        .identities
        .insert("/h/.claude-work".into(), "uuid:w".into());
    world
}

fn on(world: &mut World, id: &str, folder: &str, kind: Kind, identity: &str) {
    world.sessions.push(attributed(
        session(id, folder, kind),
        Attribution::Known(Some(IdentityId::from(identity))),
    ));
}

#[test]
fn badges_marks_totals_and_activity() {
    let mut world = two_rings();
    on(&mut world, "p1", "/h/.claude", Kind::Permission, "uuid:a");
    on(&mut world, "p2", "/h/.claude", Kind::Question, "uuid:a");
    on(&mut world, "r1", "/h/.claude", Kind::Review(30), "uuid:a");
    on(&mut world, "f1", "/h/.claude-work", Kind::Failed, "uuid:w");
    on(&mut world, "w1", "/h/.claude-work", Kind::Working, "uuid:w");
    on(&mut world, "i1", "/h/.claude-work", Kind::Idle, "uuid:w");
    let snapshot = world.project();

    assert_eq!(snapshot.version, 1);
    assert_eq!(snapshot.generated_at_ms, now_ms());
    assert!(snapshot.accounts_multi);
    assert_eq!(snapshot.totals, counts(2, 1, 1, 1, 1));
    // The tray counts what can be answered; a failed turn is no amber.
    assert_eq!(snapshot.tray_badge, 2);
    assert_eq!(snapshot.resting_marks.needs_you_key, 2);
    assert!(snapshot.resting_marks.needs_you);
    assert!(snapshot.resting_marks.review && snapshot.resting_marks.working);

    let personal = &snapshot.rings[0];
    assert_eq!(personal.ring_id, "claude-acct-a");
    assert!(personal.is_default && personal.shown);
    assert_eq!(personal.badges.needs_you, 2);
    assert_eq!(personal.badges.review, 1);
    // Needs you outranks everything else.
    assert_eq!(personal.activity, RingActivity::Waiting);
    assert_eq!(personal.success_settles_at_ms, None);
    assert_eq!(
        personal.a11y,
        "Personal: waiting for the first reading; 2 need you, 1 to review"
    );

    let work = &snapshot.rings[1];
    // A failed turn is idle for the ring (nothing to answer), the working
    // session decides.
    assert_eq!(work.activity, RingActivity::Working);
    assert_eq!(work.badges.needs_you, 0);
    assert_eq!(work.counts, counts(0, 0, 1, 1, 1));
    assert!(work.a11y.ends_with("1 working, 1 failed"), "{}", work.a11y);

    // Rows carry their account's name once there are several.
    let row = snapshot
        .sessions
        .iter()
        .find(|r| r.session_id == "w1")
        .unwrap();
    assert_eq!(row.account_label.as_deref(), Some("Work"));
    // Section order: needs you, review, working, idle; answerable first.
    let order: Vec<&str> = snapshot
        .sessions
        .iter()
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(order[2], "f1", "{order:?}");
    assert_eq!(order[3], "r1", "{order:?}");
    assert_eq!(snapshot.sections.iter().map(|s| s.count).sum::<u32>(), 6);
}

#[test]
fn a_finished_session_pulses_green_until_it_settles() {
    let mut world = two_rings();
    on(&mut world, "r1", "/h/.claude", Kind::Review(20), "uuid:a");
    let fresh = world.project();
    assert_eq!(fresh.rings[0].activity, RingActivity::Success);
    assert_eq!(
        fresh.rings[0].success_settles_at_ms,
        Some(now_ms() + 70_000)
    );
    // After the window the arc is still green (work waits for review) but its
    // settle time is in the past: the page shows it steady.
    let settled = world.project_at(at(100), now_ms() + 100_000);
    assert_eq!(settled.rings[0].activity, RingActivity::Success);
    assert!(settled.rings[0].success_settles_at_ms.unwrap() < now_ms() + 100_000);
    assert_eq!(settled.rings[1].activity, RingActivity::Idle);
}

#[test]
fn a_ring_switched_off_shows_nothing_but_lists_its_sessions() {
    let mut world = two_rings();
    world.accounts[1].ring_shown = false;
    on(
        &mut world,
        "p",
        "/h/.claude-work",
        Kind::Permission,
        "uuid:w",
    );
    let snapshot = world.project();
    assert_eq!(snapshot.sessions.len(), 1);
    let work = &snapshot.rings[1];
    assert!(!work.shown);
    assert_eq!(work.counts, counts(1, 0, 0, 0, 0));
    assert_eq!(work.badges.needs_you, 0);
    assert_eq!(work.activity, RingActivity::Idle);
    // The folded notch has no bar for a ring it doesn't show; the tray does.
    assert!(!snapshot.resting_marks.needs_you);
    assert_eq!(snapshot.tray_badge, 1);
}

#[test]
fn the_tray_badge_follows_its_setting() {
    let mut world = two_rings();
    on(&mut world, "p", "/h/.claude", Kind::Permission, "uuid:a");
    world.ui.tray_badge = false;
    assert_eq!(world.project().tray_badge, 0);
}

#[test]
fn rows_say_which_limit_a_rate_limited_session_waits_for() {
    let mut world = two_rings();
    let mut usage = AccountUsage::new(IdentityId::from("uuid:w"), UsageSource::Probe, at(-30));
    usage.five_hour = Some(window(104.0, 47 * 60, UsageWindow::SESSION_DURATION_S));
    usage.seven_day = Some(window(41.0, 86_400, UsageWindow::WEEKLY_DURATION_S));
    world.readings.insert(
        IdentityId::from("uuid:w"),
        reading_with(usage, RingStatus::Ok),
    );
    on(&mut world, "f", "/h/.claude-work", Kind::Failed, "uuid:w");
    let snapshot = world.project();
    let row = &snapshot.sessions[0];
    let text = format!("{:?}", row.detail);
    assert!(text.contains("5-hour limit"), "{text}");
}

// ---- upstream's usage ----

fn rings_of(world: &World) -> Vec<agentnotch_engine::model::RingSummary> {
    world.project().rings
}

#[test]
fn upstream_never_hears_of_needs_auth() {
    // Signed out: the honest status, plus how to sign in; never `needsAuth`.
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    world
        .readings
        .insert(IdentityId::from("uuid:a"), RingReading::SignInNeeded);
    let usage = upstream_usage(&rings_of(&world));
    assert_eq!(usage.status, "none");
    assert_eq!(usage.note, SIGN_IN_NOTE);
    assert!(usage.windows.is_empty());

    let cases: Vec<(Option<RingReading>, &str, &str)> = vec![
        (None, "none", WAITING_NOTE),
        (Some(RingReading::Waiting), "none", WAITING_NOTE),
        (
            Some(RingReading::Unavailable("Usage probes are off".into())),
            "none",
            "Usage probes are off",
        ),
        (
            Some(RingReading::Failed("Claude Code not found".into())),
            "error",
            "Claude Code not found",
        ),
        (
            Some(reading_with(full_usage("uuid:a"), RingStatus::Ok)),
            "ok",
            "",
        ),
        (
            Some(reading_with(full_usage("uuid:a"), RingStatus::Stale)),
            "stale",
            "",
        ),
    ];
    for (reading, status, note) in cases {
        let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
        if let Some(reading) = &reading {
            world
                .readings
                .insert(IdentityId::from("uuid:a"), reading.clone());
        }
        let usage = upstream_usage(&rings_of(&world));
        assert_eq!(
            (usage.status.as_str(), usage.note.as_str()),
            (status, note),
            "{reading:?}"
        );
        assert_ne!(usage.status, "needsAuth");
        assert_ne!(usage.status, "needs_auth");
    }
    // No account at all.
    let none = upstream_usage(&[]);
    assert_eq!(none.status, "none");
}

#[test]
fn upstream_gets_every_shown_rings_windows() {
    let mut world = two_rings();
    world.readings.insert(
        IdentityId::from("uuid:a"),
        reading_with(full_usage("uuid:a"), RingStatus::Ok),
    );
    let mut other = AccountUsage::new(IdentityId::from("uuid:w"), UsageSource::Probe, at(-10));
    other.five_hour = Some(window(72.0, 3600, UsageWindow::SESSION_DURATION_S));
    world.readings.insert(
        IdentityId::from("uuid:w"),
        reading_with(other, RingStatus::Ok),
    );
    let usage = upstream_usage(&rings_of(&world));
    assert_eq!(usage.status, "ok");
    let ids: Vec<&str> = usage.windows.iter().map(|w| w.id.as_str()).collect();
    // The default ring keeps bare ids, the others `<id>@<ring>`.
    assert_eq!(
        ids,
        [
            "session",
            "weekly_all",
            "extra_usage",
            "weekly_opus",
            "weekly_sonnet_4_5",
            "session@claude-acct-w"
        ]
    );
    // With several rings shown each window names its ring.
    assert!(usage.windows[..5]
        .iter()
        .all(|w| w.group.as_deref() == Some("Personal")));
    assert_eq!(usage.windows[5].group.as_deref(), Some("Work"));
    assert_eq!(usage.fetched_at, now_ms() - 10_000);
    assert_eq!(usage.backoff_until, 0);

    // A ring switched off sends nothing, and one ring needs no group.
    world.accounts[1].ring_shown = false;
    let usage = upstream_usage(&rings_of(&world));
    assert_eq!(usage.windows.len(), 5);
    assert!(usage.windows.iter().all(|w| w.group.is_none()));
}

#[test]
fn upstream_takes_its_status_from_the_default_ring() {
    // The default ring is the one holding `~\.claude`, wherever it sits.
    let mut world = World::new(vec![
        account("uuid:w", "claude-acct-w", "Work", false),
        account("uuid:a", "claude-acct-a", "Main", true),
    ]);
    world
        .readings
        .insert(IdentityId::from("uuid:w"), RingReading::Failed("x".into()));
    world.readings.insert(
        IdentityId::from("uuid:a"),
        reading_with(full_usage("uuid:a"), RingStatus::Stale),
    );
    let usage = upstream_usage(&rings_of(&world));
    assert_eq!(usage.status, "stale");
    // No default ring: the first shown one.
    world.accounts[1].includes_default = false;
    assert_eq!(upstream_usage(&rings_of(&world)).status, "error");
}

// ---- launch, the clock ----

#[test]
fn launch_rings_are_the_rings_before_any_session() {
    let accounts = vec![account("uuid:a", "claude-acct-a", "Main", true), {
        let mut hidden = account("uuid:b", "claude-acct-b", "Off", false);
        hidden.is_tracked = false;
        hidden
    }];
    let mut readings = std::collections::BTreeMap::new();
    readings.insert(
        IdentityId::from("uuid:a"),
        reading_with(full_usage("uuid:a"), RingStatus::Ok),
    );
    let rings = agentnotch_engine::hub::project::launch_rings(&accounts, &readings, now());
    assert_eq!(rings.len(), 1);
    assert_eq!(rings[0].ring_id, "claude-acct-a");
    assert_eq!(rings[0].activity, RingActivity::Idle);
    assert_eq!(rings[0].counts, Counts::default());
    assert_eq!(rings[0].usage.windows.len(), 5);
}

#[test]
fn generated_at_never_decreases_when_the_clock_steps_back() {
    let mut clock = SnapshotClock::default();
    assert_eq!(clock.next(1_000), 1_000);
    assert_eq!(clock.next(1_000), 1_001, "two snapshots never tie");
    assert_eq!(clock.next(500), 1_002, "a clock that stepped back");
    assert_eq!(clock.next(5_000), 5_000);
    assert_eq!(clock.last(), 5_000);
    // And the projection carries what it is given.
    let world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    assert_eq!(world.project_at(now(), 42).generated_at_ms, 42);
}

#[test]
fn a_sealed_projection_says_so_and_carries_the_setup() {
    let mut world = World::new(vec![account("uuid:a", "claude-acct-a", "A", true)]);
    world.sealed = true;
    world.setup.install_disabled = true;
    let snapshot = world.project();
    assert!(snapshot.sealed);
    assert!(snapshot.setup.install_disabled);
    assert_eq!(snapshot.ui, world.ui);
    assert!(!snapshot.accounts_multi);
}
