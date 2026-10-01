//! The usage store's probe half: what is probed when, requested refreshes,
//! paused rings, stopping, and what a probe's answer means
//! (A3_ExternalUsageMergeTests, A3_UsageStatePersistenceTests,
//! A3_UsageRefreshPolicyTests, A3_EarlyResetTests and PP_UsageTests: every
//! test that has a probe in it; the rest is `usage_store.rs`).
//!
//! The Swift fixture's stand-in probe runner is the test itself: it takes
//! `due_probe`'s plan (that is "Claude Code was asked", the Swift
//! `probe.count`), and answers with `finish_probe` and a `ProbeResult` built by
//! hand, or by `probe::run_probe_with` over a scripted `claude` where the
//! `.claude.json` copy of a seeded answer matters. Nothing runs a process and
//! no test waits: the Swift 20 s `refreshWaitLimit` is the deadline `refresh`
//! returns, and its sleeps are explicit times. A "poll cycle" is what the
//! hub's 20-second loop does: `prune_status_lines`, `apply_cache_reads`,
//! `desktop_due` / `accept_desktop`, then `due_probe`.

mod usage_support;

use agentnotch_engine::core::claude_json::ClaudeJsonReader;
use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::core::settings::ControlSettings;
use agentnotch_engine::core::time::{iso8601, to_ns, IsoSeconds};
use agentnotch_engine::model::{
    Account, AccountUsage, Attribution, DesktopCacheFormat, DesktopReading, DesktopWindow,
    FolderKind, IdentityId, RunFolder, UsageFetchState, UsageSource, UsageWindow,
};
use agentnotch_engine::persist::usage::{
    PersistedAccountUsage, PersistedUsageAccount, PersistedWindow, UsageStateFile,
};
use agentnotch_engine::platform::Roots;
use agentnotch_engine::runtime_types::{
    ClaudeJsonRead, ProbeOutcome, ProbePlan, ProbeResult, RefreshReason,
};
use agentnotch_engine::testkit::runner::Conversation;
use agentnotch_engine::testkit::{FakeClock, ScriptedRunner};
use agentnotch_engine::usage::planner::STORE_MARKER;
use agentnotch_engine::usage::probe::{
    arguments, run_probe_with, ProbeTiming, INITIALIZE_REQUEST_ID, USAGE_REQUEST_ID,
};
use agentnotch_engine::usage::ring_windows::WEEKLY_ID;
use agentnotch_engine::usage::schedule::{
    self, CLAUDE_NOT_FOUND_TEXT, NO_RUN_FOLDER_TEXT, PROBES_OFF_TEXT,
};
use agentnotch_engine::usage::store_probes::NOT_SET_UP_TEXT;
use agentnotch_engine::usage::{ProbeEnvironment, UsageStore, UsageStoreConfig};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use usage_support::{
    account, ago, feed, join, login, now, plus, run_folder, seen as seen_at, store_folder, t,
    windows_roots, Line,
};

const DAY: u64 = 86_400;

// MARK: - Fixture

/// A throwaway home with one signed-in account (`uuid:acc-1`, in organization
/// `org-1`, folder `~\.claude-work`) and a `claude.exe` where the installer
/// puts it. Nothing outside the temporary folder is touched; the file is
/// never run.
struct Fx {
    _dir: tempfile::TempDir,
    roots: Roots,
    dir: PathBuf,
    folder: RunFolder,
    account: Account,
    id: IdentityId,
    claude: PathBuf,
}

const SIDE_ID: &str = "uuid:acc-2";

impl Fx {
    fn new() -> Fx {
        let base = tempfile::tempdir().unwrap();
        let roots = windows_roots(base.path());
        let dir = roots.home.join(".claude-work");
        let folder = run_folder(&dir, Some(&dir.to_string_lossy()));
        let mut account = account("uuid:acc-1", Some("me@x.dev"), &[&folder], &[]);
        account.organization_uuid = Some("org-1".into());
        let claude = join(&roots.home, &[".local", "bin", "claude.exe"]);
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::fs::write(&claude, b"").unwrap();
        Fx {
            _dir: base,
            roots,
            dir,
            folder,
            account,
            id: IdentityId::from("uuid:acc-1"),
            claude,
        }
    }

    fn environment(&self) -> ProbeEnvironment {
        ProbeEnvironment {
            roots: self.roots.clone(),
            base_env: vec![
                ("Path".into(), r"C:\Windows\system32".into()),
                ("SystemRoot".into(), r"C:\Windows".into()),
                ("CLAUDE_PID".into(), "1234".into()),
            ],
            env_path: OsString::new(),
            claude_binary_path: None,
        }
    }

    fn config(&self, desktop: bool) -> UsageStoreConfig {
        UsageStoreConfig {
            home: self.roots.home.clone(),
            reads_desktop: desktop,
            ..UsageStoreConfig::default()
        }
    }

    /// A started store of the given registry, set up the way the hub does.
    fn store_of(
        &self,
        config: UsageStoreConfig,
        accounts: &[Account],
        folders: &[RunFolder],
    ) -> UsageStore {
        let mut store = UsageStore::with_config(config);
        store.start();
        store.set_accounts(accounts, folders, now());
        store.set_probe_environment(self.environment());
        store
    }

    /// The Swift `makeStore(desktopOn:)`.
    fn store(&self, desktop: bool) -> UsageStore {
        self.store_of(
            self.config(desktop),
            std::slice::from_ref(&self.account),
            std::slice::from_ref(&self.folder),
        )
    }

    /// A second signed-in account (`~\.claude-side`), next to the first.
    fn side(&self) -> (Account, RunFolder) {
        let dir = self.roots.home.join(".claude-side");
        let folder = run_folder(&dir, Some(&dir.to_string_lossy()));
        let account = account(SIDE_ID, Some("me@y.dev"), &[&folder], &[]);
        (account, folder)
    }

    fn store_with_side(&self) -> (UsageStore, IdentityId) {
        let (side, side_folder) = self.side();
        let store = self.store_of(
            self.config(false),
            &[self.account.clone(), side],
            &[self.folder.clone(), side_folder],
        );
        (store, IdentityId::from(SIDE_ID))
    }

    /// `.claude.json` read of the fixture's folder: signed in, in `org-1`,
    /// optionally with Claude Code's cached usage (`writeGlobalConfig`).
    fn read(&self, cached_session: Option<f64>, fetched_at: SystemTime) -> ClaudeJsonRead {
        ClaudeJsonRead {
            folder: self.folder.id.clone(),
            identity: Some(login(Some("me@x.dev"), Some("acc-1"), Some("org-1"))),
            cached_usage: cached_session.map(|session| {
                let mut usage = AccountUsage::new(self.id.clone(), UsageSource::Cache, fetched_at);
                usage.five_hour = Some(five(session));
                usage.seven_day = Some(week(10.0));
                usage
            }),
            stamp: None,
            error: None,
        }
    }

    /// The read of the second account's folder.
    fn side_read(&self, folder: &RunFolder) -> ClaudeJsonRead {
        ClaudeJsonRead {
            folder: folder.id.clone(),
            identity: Some(login(Some("me@y.dev"), Some("acc-2"), None)),
            cached_usage: None,
            stamp: None,
            error: None,
        }
    }

    fn line(&self, weekly_utilization: f64, process: u32, at: SystemTime) -> Line {
        Line {
            five: None,
            weekly: Some(week(weekly_utilization)),
            process,
            session: format!("s{process}"),
            at,
            start: None,
            folder: self.dir.to_string_lossy().into_owned(),
            attribution: Attribution::Known(Some(self.id.clone())),
        }
    }
}

/// The fixture's session window (`UsageFixture.body` resets it in 3 hours).
fn five(utilization: f64) -> UsageWindow {
    UsageWindow::new(
        utilization,
        Some(now() + Duration::from_secs(3 * 3600)),
        UsageWindow::SESSION_DURATION_S,
    )
}

/// The fixture's weekly window (`UsageFixture.body` resets it in 3 days).
fn week(utilization: f64) -> UsageWindow {
    UsageWindow::new(
        utilization,
        Some(now() + Duration::from_secs(3 * DAY)),
        UsageWindow::WEEKLY_DURATION_S,
    )
}

/// What Claude Desktop's cache says (`desktopReading`).
fn desktop_reading(session: f64, observed_at: SystemTime) -> DesktopReading {
    DesktopReading::Reading {
        organization_uuid: "org-1".into(),
        windows: vec![
            DesktopWindow {
                id: "session".into(),
                label: None,
                utilization: session,
                resets_at: Some(now() + Duration::from_secs(3 * 3600)),
                duration_s: 18_000,
            },
            DesktopWindow {
                id: "weekly_all".into(),
                label: None,
                utilization: 10.0,
                resets_at: Some(now() + Duration::from_secs(3 * DAY)),
                duration_s: 604_800,
            },
        ],
        observed_at,
        resets: None,
    }
}

fn desktop_miss() -> DesktopReading {
    DesktopReading::NotFound {
        format: DesktopCacheFormat::Simple,
    }
}

/// Stands in for Claude Desktop's cache, and counts the reads.
struct Desktop {
    reading: DesktopReading,
    asked: Vec<String>,
}

impl Desktop {
    fn with(reading: DesktopReading) -> Desktop {
        Desktop {
            reading,
            asked: Vec::new(),
        }
    }

    fn nothing() -> Desktop {
        Desktop::with(desktop_miss())
    }
}

/// The hub's poll cycle at `at`; the probe it would run, if any.
fn cycle(
    store: &mut UsageStore,
    reads: &[ClaudeJsonRead],
    desktop: &mut Desktop,
    at: SystemTime,
) -> Option<ProbePlan> {
    store.prune_status_lines(at);
    store.apply_cache_reads(reads, at);
    for (id, organization) in store.desktop_due(at, None, false) {
        desktop.asked.push(organization);
        store.accept_desktop(&id, &desktop.reading.clone(), at);
    }
    store.due_probe(at)
}

/// A cycle with no Claude Desktop in it (the setting is off).
fn quiet_cycle(
    store: &mut UsageStore,
    reads: &[ClaudeJsonRead],
    at: SystemTime,
) -> Option<ProbePlan> {
    cycle(store, reads, &mut Desktop::nothing(), at)
}

fn result(plan: &ProbePlan, outcome: ProbeOutcome, finished: SystemTime) -> ProbeResult {
    ProbeResult {
        plan: plan.clone(),
        outcome,
        started: plan.planned_at,
        finished,
        folder_identity_after: Some(plan.identity.clone()),
    }
}

/// A fresh answer: Claude Code asked its server after the probe launched.
fn reading(
    plan: &ProbePlan,
    session: Option<f64>,
    weekly: Option<f64>,
    answered: SystemTime,
) -> ProbeResult {
    let mut usage = AccountUsage::new(plan.identity.clone(), UsageSource::Probe, answered);
    usage.five_hour = session.map(five);
    usage.seven_day = weekly.map(week);
    usage.taken_after = Some(plan.planned_at);
    result(plan, ProbeOutcome::Reading(usage), answered)
}

fn five_of(store: &UsageStore, id: &IdentityId) -> Option<f64> {
    store.five_hour(id)
}

fn weekly_of(store: &UsageStore, id: &IdentityId) -> Option<f64> {
    store
        .usage_of(id)
        .and_then(|usage| usage.seven_day.as_ref())
        .map(|window| window.utilization)
}

/// A request for one identity as the hub makes it: the caches are re-read
/// first, then the store is asked.
fn ask(
    store: &mut UsageStore,
    reads: &[ClaudeJsonRead],
    id: &IdentityId,
    reason: RefreshReason,
    at: SystemTime,
) -> agentnotch_engine::usage::RefreshRequest {
    store.apply_cache_reads(reads, at);
    store.refresh(id, reason, at)
}

fn persisted_reading(id: &IdentityId, session: f64, at: SystemTime) -> PersistedAccountUsage {
    PersistedAccountUsage {
        account_id: id.0.clone(),
        five_hour: Some(PersistedWindow {
            utilization: session,
            resets_at: Some(IsoSeconds(ago(0) + Duration::from_secs(3600))),
            duration: 18_000.0,
        }),
        seven_day: None,
        scoped: Vec::new(),
        extra_usage: None,
        subscription_type: None,
        source: "probe".into(),
        updated_at: IsoSeconds(at),
        taken_after: None,
    }
}

fn state_of(id: &IdentityId, account: PersistedUsageAccount) -> UsageStateFile {
    let mut file = UsageStateFile::default();
    file.accounts.insert(id.0.clone(), account);
    file
}

// MARK: - Claude Desktop merge (A3_ExternalUsageMergeTests)

#[test]
fn a_newer_desktop_reading_wins_and_holds_off_the_probe() {
    let fx = Fx::new();
    let reads = [fx.read(Some(20.0), ago(20 * 60))];
    let mut desktop = Desktop::with(desktop_reading(50.0, ago(10)));
    let mut store = fx.store(true);

    let plan = cycle(&mut store, &reads, &mut desktop, now());

    let usage = store.usage_of(&fx.id).expect("a reading");
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(50.0));
    assert_eq!(usage.updated_at, ago(10));
    assert_eq!(store.organization_of(&fx.id), Some("org-1"));
    assert_eq!(desktop.asked, ["org-1"]);
    // Fresh from Desktop: Claude Code isn't asked.
    assert!(plan.is_none());
}

#[test]
fn an_older_desktop_reading_loses() {
    let fx = Fx::new();
    let reads = [fx.read(Some(20.0), ago(60))];
    let mut desktop = Desktop::with(desktop_reading(50.0, ago(600)));
    let mut store = fx.store(true);

    let plan = cycle(&mut store, &reads, &mut desktop, now());

    let usage = store.usage_of(&fx.id).expect("a reading");
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(20.0));
    assert_eq!(usage.updated_at, ago(60));
    assert!(plan.is_none());
}

#[test]
fn with_nothing_fresh_the_probe_runs() {
    let fx = Fx::new();
    let reads = [fx.read(Some(20.0), ago(20 * 60))];
    let mut store = fx.store(true);

    let plan = cycle(&mut store, &reads, &mut Desktop::nothing(), now()).expect("a probe is due");

    // What was asked: this account's folder, Claude Code's own command line,
    // the probe's working folder, and nothing of the app's own Claude env.
    assert_eq!(plan.identity, fx.id);
    assert_eq!(plan.folder, fx.folder.id);
    assert_eq!(plan.config_dir, fx.dir);
    assert_eq!(
        plan.config_dir_env.as_deref(),
        Some(fx.dir.to_string_lossy().as_ref())
    );
    assert_eq!(plan.spec.program, fx.claude);
    assert_eq!(plan.spec.args, arguments(&[]));
    assert_eq!(plan.spec.cwd, fx.roots.usage_probe_dir());
    assert_eq!(plan.identity_file, fx.dir.join(".claude.json"));
    assert_eq!(plan.expected.email.as_deref(), Some("me@x.dev"));
    assert_eq!(plan.expected.account_uuid.as_deref(), Some("acc-1"));
    assert_eq!(plan.reason, RefreshReason::Interval);
    assert_eq!(plan.planned_at, now());
    let names: Vec<String> = plan
        .spec
        .env
        .iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|name| name == "CLAUDE_PID"));
    let config_dir = plan
        .spec
        .env
        .iter()
        .find(|(name, _)| name == "CLAUDE_CONFIG_DIR")
        .map(|(_, value)| value.clone());
    assert_eq!(config_dir, Some(fx.dir.clone().into_os_string()));
    assert!(store.is_probing());
    assert_eq!(store.fetch_state_of(&fx.id), UsageFetchState::Fetching);

    store.finish_probe(
        reading(&plan, Some(33.0), Some(10.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    assert_eq!(five_of(&store, &fx.id), Some(33.0));
    assert_eq!(
        store.usage_of(&fx.id).map(|usage| usage.source),
        Some(UsageSource::Probe)
    );
    assert!(!store.is_probing());
}

#[test]
fn desktop_is_left_alone_when_switched_off_or_paused() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut desktop = Desktop::with(desktop_reading(50.0, now()));
    let mut plans = Vec::new();

    let mut off = fx.store(false);
    plans.extend(cycle(&mut off, &reads, &mut desktop, now()));
    assert!(desktop.asked.is_empty());
    // Without the Desktop setting the organization isn't read at all.
    assert_eq!(off.organization_of(&fx.id), None);

    let mut paused = fx.store(true);
    paused.set_paused([fx.id.clone()].into());
    plans.extend(cycle(&mut paused, &reads, &mut desktop, now()));
    assert!(desktop.asked.is_empty());
    assert_eq!(
        plans.len(),
        1,
        "only the unpaused store with Desktop off probed"
    );
}

// MARK: - usage-state.json with probes (A3_UsageStatePersistenceTests)

#[test]
fn a_relaunch_neither_probes_early_nor_starts_empty() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    // The last run probed a minute ago; its reading is 20 minutes old.
    let file = state_of(
        &fx.id,
        PersistedUsageAccount {
            last_probe_at: Some(IsoSeconds(ago(60))),
            last_full_reading: Some(persisted_reading(&fx.id, 44.0, ago(20 * 60))),
            ..PersistedUsageAccount::default()
        },
    );
    let mut store = fx.store(false);
    store.restore(&file, now());
    assert_eq!(five_of(&store, &fx.id), Some(44.0));

    // Stale, but asked a minute ago: 5 minutes between probes.
    assert!(quiet_cycle(&mut store, &reads, now()).is_none());
    // The same state, long enough later, is due.
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 240.0)).is_some());
}

#[test]
fn a_backoff_survives_a_relaunch() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let file = state_of(
        &fx.id,
        PersistedUsageAccount {
            last_probe_at: Some(IsoSeconds(ago(3600))),
            failure_count: 3,
            next_attempt_at: Some(IsoSeconds(plus(now(), 600.0))),
            ..PersistedUsageAccount::default()
        },
    );
    let mut store = fx.store(false);
    store.restore(&file, now());

    assert!(quiet_cycle(&mut store, &reads, now()).is_none());
    // Until its time has come.
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 601.0)).is_some());
}

#[test]
fn probes_are_recorded_for_the_next_run() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    let request = ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    assert_eq!(request.wait_until, Some(plus(now(), 20.0)));
    let plan = store.due_probe(now()).expect("a requested probe");
    store.finish_probe(
        reading(&plan, Some(61.0), Some(10.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );

    let file = store.save_now();
    let saved = &file.accounts[fx.id.as_str()];
    assert_eq!(saved.last_probe_at, Some(IsoSeconds(plus(now(), 1.0))));
    assert_eq!(saved.failure_count, 0);
    assert_eq!(
        saved
            .last_full_reading
            .as_ref()
            .and_then(|reading| reading.five_hour.as_ref())
            .map(|window| window.utilization),
        Some(61.0)
    );
}

// MARK: - Refresh policy (A3_UsageRefreshPolicyTests)

#[test]
fn a_ring_click_probes_only_stale_data() {
    // 60 s old: fresh enough.
    let fresh = Fx::new();
    let reads = [fresh.read(Some(20.0), ago(60))];
    let mut store = fresh.store(false);
    let request = ask(
        &mut store,
        &reads,
        &fresh.id,
        RefreshReason::RingClick,
        now(),
    );
    assert_eq!(request.wait_until, None);
    assert!(store.due_probe(now()).is_none());

    // 130 s old: asked, and the answer ends the wait.
    let stale = Fx::new();
    let reads = [stale.read(Some(20.0), ago(130))];
    let mut store = stale.store(false);
    let request = ask(
        &mut store,
        &reads,
        &stale.id,
        RefreshReason::RingClick,
        now(),
    );
    assert_eq!(request.wait_until, Some(plus(now(), 20.0)));
    let plan = store.due_probe(now()).expect("asked for stale data");
    assert_eq!(plan.reason, RefreshReason::RingClick);
    assert!(store.is_waiting_on(&stale.id));
    store.finish_probe(
        reading(&plan, Some(5.0), Some(10.0), plus(now(), 2.0)),
        plus(now(), 2.0),
    );
    assert!(!store.is_waiting_on(&stale.id));
    assert_eq!(five_of(&store, &stale.id), Some(5.0));
    assert_eq!(store.fetch_state_of(&stale.id), UsageFetchState::Idle);
}

#[test]
fn forced_refreshes_are_spaced_by_a_minute() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("the first is asked");
    store.finish_probe(
        reading(&plan, Some(5.0), Some(10.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );

    // A second one 30 s later is skipped, and nothing waits for it.
    let again = ask(
        &mut store,
        &reads,
        &fx.id,
        RefreshReason::Manual,
        plus(now(), 30.0),
    );
    assert_eq!(again.wait_until, None);
    assert!(store.due_probe(plus(now(), 30.0)).is_none());
    assert_eq!(store.fetch_state_of(&fx.id), UsageFetchState::Idle);
    // A minute after the probe it is asked again.
    ask(
        &mut store,
        &reads,
        &fx.id,
        RefreshReason::Manual,
        plus(now(), 61.0),
    );
    assert!(store.due_probe(plus(now(), 61.0)).is_some());
}

#[test]
fn a_ring_click_respects_the_backoff_and_forced_clears_it() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let file = state_of(
        &fx.id,
        PersistedUsageAccount {
            last_probe_at: Some(IsoSeconds(ago(600))),
            failure_count: 1,
            next_attempt_at: Some(IsoSeconds(plus(now(), 300.0))),
            ..PersistedUsageAccount::default()
        },
    );
    let mut store = fx.store(false);
    store.restore(&file, now());

    let click = ask(&mut store, &reads, &fx.id, RefreshReason::RingClick, now());
    assert_eq!(click.wait_until, None);
    assert!(store.due_probe(now()).is_none());

    let forced = ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    assert!(forced.wait_until.is_some());
    let plan = store.due_probe(now()).expect("forced clears the backoff");
    // Rate limited again: paused, and said so in words that aren't the
    // account's own limit.
    store.finish_probe(
        result(
            &plan,
            ProbeOutcome::RateLimited { retry_after: None },
            plus(now(), 1.0),
        ),
        plus(now(), 1.0),
    );
    let UsageFetchState::Failed(text) = store.fetch_state_of(&fx.id) else {
        panic!("expected a failure state");
    };
    assert!(
        text.starts_with("Usage check paused (too many requests), retrying in"),
        "{text}"
    );
}

#[test]
fn a_refresh_waits_at_most_the_limit() {
    // Swift refreshWaitsAtMostTheLimit: the call returned after the limit
    // although Claude Code hadn't answered. Here the deadline is returned,
    // and an answer before it closes the request.
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    let request = ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let deadline = request.wait_until.expect("a probe will be asked for");
    assert!(deadline <= now() + Duration::from_secs(20));
    assert_eq!(deadline, now() + schedule::REFRESH_WAIT_LIMIT);

    let plan = store.due_probe(now()).expect("a probe");
    assert!(store.is_probing());
    assert!(store.is_waiting_on(&fx.id));
    assert!(store.usage_of(&fx.id).is_none());
    // Still running at the deadline: the caller stops waiting, the probe goes on.
    assert!(store.is_probing());
    // A finish before the deadline resolves the request.
    store.finish_probe(
        reading(&plan, Some(5.0), Some(10.0), plus(now(), 8.0)),
        plus(now(), 8.0),
    );
    assert!(!store.is_waiting_on(&fx.id));
    assert_eq!(five_of(&store, &fx.id), Some(5.0));
}

/// An account the user stopped tracking (the Mac's hidden identity, outside
/// `visibleIdentities`) keeps its state and its cached usage, but Claude
/// Code is never run for it on the schedule or by "Check now", and Claude
/// Desktop's cache isn't read for it. Asked for by name it is checked, as
/// the Mac's `refresh(accountId:reason:)` does for any identity.
#[test]
fn untracked_accounts_are_neither_scheduled_nor_read_from_desktop() {
    let fx = Fx::new();
    let mut untracked = fx.account.clone();
    untracked.is_tracked = false;
    let mut store = fx.store_of(
        fx.config(true),
        &[untracked],
        std::slice::from_ref(&fx.folder),
    );
    // Old enough that a tracked account would be probed now
    // (`with_nothing_fresh_the_probe_runs`).
    let reads = [fx.read(Some(20.0), ago(20 * 60))];
    let mut desktop = Desktop::with(desktop_reading(50.0, now()));

    assert!(cycle(&mut store, &reads, &mut desktop, now()).is_none());
    assert!(
        desktop.asked.is_empty(),
        "Desktop read for an untracked account"
    );
    assert_eq!(
        five_of(&store, &fx.id),
        Some(20.0),
        "its cached usage stays"
    );
    assert!(!store.is_probing());

    let all = store.refresh_all(plus(now(), 1.0));
    assert!(all.desktop_reads.is_empty());
    assert_eq!(all.wait_until, None);
    assert!(store.due_probe(plus(now(), 1.0)).is_none());
    assert_eq!(store.fetch_state_of(&fx.id), UsageFetchState::Idle);

    let asked = ask(
        &mut store,
        &reads,
        &fx.id,
        RefreshReason::Manual,
        plus(now(), 2.0),
    );
    assert!(asked.desktop_reads.is_empty());
    let plan = store
        .due_probe(plus(now(), 2.0))
        .expect("asked for by name, it is checked");
    assert_eq!(plan.identity, fx.id);

    // Tracked again, the schedule takes it.
    let mut store = fx.store(true);
    assert!(cycle(&mut store, &reads, &mut Desktop::nothing(), now()).is_some());
}

#[test]
fn paused_accounts_are_not_probed_until_shown_again() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    store.set_paused([fx.id.clone()].into());
    assert!(quiet_cycle(&mut store, &reads, now()).is_none());
    store.set_paused(BTreeSet::new());
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 20.0)).is_some());
}

/// Switching a ring off drops its queued request: no probe later, no
/// "checking..." left behind, and whoever waits on it is let go.
#[test]
fn pausing_a_ring_drops_its_queued_request() {
    let fx = Fx::new();
    let (mut store, side) = fx.store_with_side();
    let (_, side_folder) = fx.side();
    let reads = [fx.read(None, now()), fx.side_read(&side_folder)];
    store.apply_cache_reads(&reads, now());

    // Both asked for; the first is being probed, the second waits its turn.
    let all = store.refresh_all(now());
    assert_eq!(all.wait_until, Some(plus(now(), 20.0)));
    let plan = store.due_probe(now()).expect("the first probe");
    assert_eq!(plan.identity, fx.id);
    assert_eq!(store.fetch_state_of(&side), UsageFetchState::Fetching);
    // A second request for the one in the queue joins it.
    let waiter = store.refresh(&side, RefreshReason::Manual, plus(now(), 0.3));
    assert_eq!(
        waiter.wait_until,
        Some(plus(now(), 0.3) + Duration::from_secs(20))
    );
    assert!(store.is_waiting_on(&side));
    // One at a time: nothing else while the first runs.
    assert!(store.due_probe(plus(now(), 0.5)).is_none());

    store.set_paused([side.clone()].into());
    assert!(!store.is_waiting_on(&side), "whoever waits is let go");
    assert_eq!(store.fetch_state_of(&side), UsageFetchState::Idle);
    assert_eq!(store.fetch_state_of(&fx.id), UsageFetchState::Fetching);
    assert!(store.is_fetching());

    store.finish_probe(
        reading(&plan, Some(5.0), Some(10.0), plus(now(), 2.0)),
        plus(now(), 2.0),
    );
    assert!(
        store.due_probe(plus(now(), 2.0)).is_none(),
        "only the first was probed"
    );
    assert!(!store.is_fetching());

    // Asked for while off: nothing reaches Claude Code.
    let off = ask(
        &mut store,
        &reads,
        &side,
        RefreshReason::Manual,
        plus(now(), 30.0),
    );
    assert_eq!(off.wait_until, None);
    assert!(store.due_probe(plus(now(), 30.0)).is_none());
    assert_eq!(store.fetch_state_of(&side), UsageFetchState::Idle);
}

/// Quitting: nothing new reaches Claude Code after `stop()`, not even the
/// request queued behind the probe that is still running.
#[test]
fn nothing_is_probed_after_stop() {
    let fx = Fx::new();
    let (mut store, side) = fx.store_with_side();
    let (_, side_folder) = fx.side();
    let reads = [fx.read(None, now()), fx.side_read(&side_folder)];
    store.apply_cache_reads(&reads, now());
    store.refresh_all(now());
    let plan = store.due_probe(now()).expect("the first probe");

    store.stop();
    assert_ne!(store.fetch_state_of(&side), UsageFetchState::Fetching);
    // The running one still finishes and is recorded.
    store.finish_probe(
        reading(&plan, Some(5.0), Some(10.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    assert_eq!(five_of(&store, &fx.id), Some(5.0));
    assert!(store.due_probe(plus(now(), 2.0)).is_none());
    assert!(
        store.due_probe(plus(now(), 3600.0)).is_none(),
        "not on the schedule either"
    );
    assert!(!store.is_probing() && !store.is_fetching());
}

#[test]
fn a_seeded_answer_keeps_its_real_date() {
    let fx = Fx::new();
    std::fs::create_dir_all(&fx.dir).unwrap();
    for (age, rate_limited) in [(40 * 60, true), (30, false)] {
        let fetched_at = ago(age);
        // Claude Code's fallback: the same numbers as its cache (what a
        // reading's own `limits` key would say, it lacks).
        let body = json!({
            "five_hour": {"utilization": 20, "resets_at": iso8601(now() + Duration::from_secs(3 * 3600))},
            "seven_day": {"utilization": 10, "resets_at": iso8601(now() + Duration::from_secs(3 * DAY))},
        });
        std::fs::write(
            fx.dir.join(".claude.json"),
            serde_json::to_vec(&json!({
                "oauthAccount": {"accountUuid": "acc-1", "emailAddress": "me@x.dev", "organizationUuid": "org-1"},
                "cachedUsageUtilization": {"accountUuid": "acc-1",
                    "fetchedAtMs": fetched_at.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
                    "utilization": body},
            }))
            .unwrap(),
        )
        .unwrap();

        let mut store = fx.store(false);
        store.apply_cache_reads(&[fx.read(None, now())], now());
        store.refresh(&fx.id, RefreshReason::Manual, now());
        let plan = store.due_probe(now()).expect("a probe");

        let runner = ScriptedRunner::default();
        runner.push_conversation(
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
                ),
        );
        let clock = FakeClock::at_ms(1_800_000_000_000);
        let timing = ProbeTiming {
            timeout: Duration::from_secs(10),
            exit_grace: Duration::from_millis(300),
            close_grace: Duration::from_millis(200),
            kill_wait: Duration::from_secs(1),
        };
        let probed = run_probe_with(&plan, &runner, &clock, &timing, &ClaudeJsonReader::new());
        assert_eq!(runner.spawned().len(), 1);
        store.finish_probe(probed, now());

        let usage = store.usage_of(&fx.id).expect("a reading");
        assert_eq!(usage.updated_at, fetched_at, "not stamped as fetched now");
        assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(20.0));
        assert_eq!(
            usage.is_stale(now(), store.stale_threshold()),
            age > 15 * 60
        );
        assert_eq!(store.subscription_type_of(&fx.id), Some("max"));
        match store.fetch_state_of(&fx.id) {
            UsageFetchState::Failed(text) => {
                assert!(
                    rate_limited,
                    "a seeded answer is rate limited only when old"
                );
                assert!(text.contains("too many requests"), "{text}");
            }
            UsageFetchState::Idle => assert!(!rate_limited, "fresh enough to count"),
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn success(id: &str, payload: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": payload}}))
    .unwrap()
}

// MARK: - Early resets with checks (A3_EarlyResetTests)

#[test]
fn a_check_after_the_reset_replaces_the_status_lines_old_numbers() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    store.apply_cache_reads(&reads, now());

    // Before the reset: 62% of the week, from a terminal session.
    feed(&mut store, fx.line(62.0, 101, ago(600)));
    assert_eq!(weekly_of(&store, &fx.id), Some(62.0));

    // The reset; the next check reads 3%, same reset time.
    ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a check");
    let seen = store.finish_probe(
        reading(&plan, None, Some(3.0), plus(now(), 2.0)),
        plus(now(), 2.0),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    assert_eq!(seen.map(|o| o.source), Some(UsageSource::Probe));

    // The session, idle, re-renders its old numbers: still 3%, and nothing
    // for the history.
    let seen = feed(&mut store, fx.line(62.0, 101, plus(now(), 3.0)));
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    assert!(seen.is_none());

    // Its next response after the reset: shown and recorded, though the
    // process said more before.
    let seen = feed(&mut store, fx.line(4.0, 101, plus(now(), 4.0))).expect("news");
    assert_eq!(weekly_of(&store, &fx.id), Some(4.0));
    assert_eq!(seen.source, UsageSource::StatusLine);
    let recorded = seen.windows.iter().find(|(id, _, _)| id == WEEKLY_ID);
    assert_eq!(recorded.map(|(_, utilization, _)| *utilization), Some(4.0));
}

/// A relaunch keeps what each running process said, so an idle one
/// re-rendering its pre-reset numbers is still a repeat.
#[test]
fn a_relaunch_remembers_what_each_process_said_after_a_check() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let started = ago(3600);
    let mut store = fx.store(false);
    store.apply_cache_reads(&reads, now());
    feed(&mut store, fx.line(62.0, 101, ago(600)).started(started));
    ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a check");
    store.finish_probe(
        reading(&plan, None, Some(3.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    let file = store.save_now();

    // The next run, the same process still running: a repeat, not news.
    let mut next = fx.store(false);
    next.restore(&file, plus(now(), 5.0));
    assert_eq!(weekly_of(&next, &fx.id), Some(3.0));
    let seen = feed(
        &mut next,
        fx.line(62.0, 101, plus(now(), 6.0)).started(started),
    );
    assert_eq!(weekly_of(&next, &fx.id), Some(3.0));
    assert!(seen.is_none());

    // A new process that got the same pid is another process: its line is news.
    let mut reused = fx.store(false);
    reused.restore(&file, plus(now(), 5.0));
    let seen = feed(
        &mut reused,
        fx.line(62.0, 101, plus(now(), 6.0))
            .started(started + Duration::from_secs(30)),
    );
    assert!(seen.is_some_and(|o| o.source == UsageSource::StatusLine));
}

/// A line that is news for its process but that the check outranks is not
/// recorded: the history gets what the ring shows.
#[test]
fn a_line_the_check_outranks_is_not_recorded() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    store.apply_cache_reads(&reads, now());
    ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a check");
    store.finish_probe(
        reading(&plan, None, Some(42.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    let base = plus(now(), 2.0);
    feed(&mut store, fx.line(42.0, 101, base));

    // A process's own small step back (a late response): not news.
    let seen = feed(&mut store, fx.line(40.0, 101, plus(base, 1.0)));
    assert!(seen.is_none());
    // Another process's numbers, a little behind: news for it, not shown.
    let seen = feed(&mut store, fx.line(40.0, 202, plus(base, 2.0)));
    assert_eq!(weekly_of(&store, &fx.id), Some(42.0));
    assert!(seen.is_none());

    // A reset-sized drop from a process that reported after both: shown
    // and recorded.
    feed(&mut store, fx.line(40.0, 101, plus(base, 2.5)));
    let seen = feed(&mut store, fx.line(30.0, 101, plus(base, 3.0))).expect("news");
    assert_eq!(weekly_of(&store, &fx.id), Some(30.0));
    assert_eq!(seen.source, UsageSource::StatusLine);
    assert_eq!(
        seen.windows.first().map(|(_, utilization, _)| *utilization),
        Some(30.0)
    );
}

/// A check's answer came some time after it was launched: a line that
/// arrived while it ran is not known to be older than the answer.
#[test]
fn a_line_that_arrives_during_a_check_is_not_older_than_its_answer() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a check");
    assert_eq!(plan.planned_at, now());

    // While Claude Code answers (launched at `now`, answers at +3 s).
    feed(&mut store, fx.line(62.0, 101, plus(now(), 1.0)));
    store.finish_probe(
        reading(&plan, None, Some(3.0), plus(now(), 3.0)),
        plus(now(), 3.0),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(62.0));
}

/// A status line's slow save doesn't hold back the quick one a check asks for.
#[test]
fn a_checks_save_doesnt_wait_behind_a_status_lines() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    store.apply_cache_reads(&reads, now());
    store.save_now();
    feed(&mut store, fx.line(30.0, 101, now()).started(ago(60)));
    assert_eq!(
        store.save_due_at(),
        Some(now() + schedule::STATUS_LINE_SAVE_DELAY)
    );

    ask(
        &mut store,
        &reads,
        &fx.id,
        RefreshReason::Manual,
        plus(now(), 1.0),
    );
    let plan = store.due_probe(plus(now(), 1.0)).expect("a check");
    store.finish_probe(
        reading(&plan, None, Some(31.0), plus(now(), 2.0)),
        plus(now(), 2.0),
    );
    assert_eq!(
        store.save_due_at(),
        Some(plus(now(), 2.0) + schedule::STATE_SAVE_DELAY)
    );
    let file = store
        .take_state_to_save(plus(now(), 4.0))
        .expect("due 2 s after the check");
    assert!(file.accounts[fx.id.as_str()].last_probe_at.is_some());
}

// MARK: - Over the user's layout (PP_UsageTests)

/// The user's layout: `paras` runs in `~\.claude` and a window folder and
/// keeps a store; `biios` runs in one window folder and keeps a store.
struct Layout {
    fx: Fx,
    accounts: Vec<Account>,
    folders: Vec<RunFolder>,
    paras: IdentityId,
    biios: IdentityId,
    main: RunFolder,
    paras_window: RunFolder,
    biios_window: RunFolder,
    paras_store: RunFolder,
    biios_store: RunFolder,
}

fn layout(with_biios_window: bool) -> Layout {
    let fx = Fx::new();
    let home = fx.roots.home.clone();
    let main = run_folder(&home.join(".claude"), None);
    let window = |name: &str| {
        let dir = join(&home, &[".claude-windows", name]);
        run_folder(&dir, Some(&dir.to_string_lossy()))
    };
    let paras_window = window("801f9dd51396");
    let biios_window = window("1bf3e8f92b11");
    let paras_store = store_folder(&home.join(".claude-paras"));
    let biios_store = store_folder(&home.join(".claude-biios"));
    assert_eq!(paras_store.kind, FolderKind::Store);
    let paras = account(
        "uuid:paras",
        Some("paras@x.dev"),
        &[&main, &paras_window],
        &[&paras_store],
    );
    let biios = if with_biios_window {
        account(
            "uuid:biios",
            Some("biios@x.dev"),
            &[&biios_window],
            &[&biios_store],
        )
    } else {
        account("uuid:biios", Some("biios@x.dev"), &[], &[&biios_store])
    };
    let mut folders = vec![
        main.clone(),
        paras_window.clone(),
        paras_store.clone(),
        biios_store.clone(),
    ];
    if with_biios_window {
        folders.push(biios_window.clone());
    }
    Layout {
        fx,
        paras: paras.identity_id.clone(),
        biios: biios.identity_id.clone(),
        accounts: vec![paras, biios],
        folders,
        main,
        paras_window,
        biios_window,
        paras_store,
        biios_store,
    }
}

/// Over the user's layout: one check per account, each in a run folder of its
/// own; an account only a store holds is not checked.
#[test]
fn one_check_per_account_never_in_a_store() {
    let l = layout(true);
    let mut store = l.fx.store_of(l.fx.config(false), &l.accounts, &l.folders);
    store.apply_cache_reads(&[], now());

    store.refresh_all(now());
    let mut plans = Vec::new();
    let mut at = now();
    while let Some(plan) = store.due_probe(at) {
        at = plus(at, 1.0);
        store.finish_probe(reading(&plan, Some(40.0), Some(10.0), at), at);
        plans.push(plan);
    }
    assert_eq!(plans.len(), 2);
    let probed: BTreeSet<&IdentityId> = plans.iter().map(|plan| &plan.identity).collect();
    assert_eq!(probed, BTreeSet::from([&l.paras, &l.biios]));
    for plan in &plans {
        assert_ne!(
            plan.config_dir, l.paras_store.config_dir,
            "checked in a store"
        );
        assert_ne!(
            plan.config_dir, l.biios_store.config_dir,
            "checked in a store"
        );
        if plan.identity == l.paras {
            assert!([&l.main, &l.paras_window]
                .iter()
                .any(|f| f.id == plan.folder));
        }
        if plan.identity == l.biios {
            assert_eq!(plan.config_dir, l.biios_window.config_dir);
        }
    }
    assert_eq!(five_of(&store, &l.paras), Some(40.0));
    assert_eq!(five_of(&store, &l.biios), Some(40.0));
}

#[test]
fn an_account_only_a_store_holds_shows_its_cached_usage_and_is_not_checked() {
    let l = layout(false);
    let mut store = l.fx.store_of(l.fx.config(false), &l.accounts, &l.folders);
    // The store's cached snapshot, passive, with its date.
    let mut cached = AccountUsage::new(
        IdentityId::from("uuid:biios"),
        UsageSource::Cache,
        t(1_790_000_050),
    );
    cached.five_hour = Some(five(7.0));
    let reads = [ClaudeJsonRead {
        folder: l.biios_store.id.clone(),
        identity: Some(login(Some("biios@x.dev"), Some("biios"), None)),
        cached_usage: Some(cached),
        stamp: None,
        error: None,
    }];
    store.apply_cache_reads(&reads, now());
    let usage = store.usage_of(&l.biios).expect("the store's cache shows");
    assert_eq!(usage.source, UsageSource::Cache);
    assert_eq!(usage.updated_at, t(1_790_000_050));

    // Asked for, and never launched: the state says why.
    let request = store.refresh(&l.biios, RefreshReason::Manual, now());
    assert!(request.wait_until.is_some());
    let plan = store.due_probe(now());
    assert!(plan.as_ref().is_none_or(|plan| plan.identity != l.biios));
    assert_eq!(store.fetch_state_of(&l.biios), schedule::no_run_folder());
    assert!(!store.is_waiting_on(&l.biios));
    // And not on the schedule either, however old.
    assert!(store
        .due_probe(plus(now(), 7200.0))
        .is_none_or(|plan| plan.identity != l.biios));
}

// MARK: - What the store does beyond the Swift tests

#[test]
fn a_probe_runs_in_the_most_recently_active_run_folder() {
    let fx = Fx::new();
    let other_dir = fx.roots.home.join(".claude-other");
    let other = run_folder(&other_dir, Some(&other_dir.to_string_lossy()));
    let mut account = account("uuid:acc-1", Some("me@x.dev"), &[&fx.folder, &other], &[]);
    account.organization_uuid = Some("org-1".into());
    let folders = [
        seen_at(fx.folder.clone(), ago(600)),
        seen_at(other.clone(), ago(60)),
    ];
    let mut store = fx.store_of(fx.config(false), &[account], &folders);
    store.refresh(&fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a probe");
    assert_eq!(plan.folder, other.id, "a session was seen there last");
    assert_eq!(plan.identity_file, other_dir.join(".claude.json"));
    store.finish_probe(
        result(&plan, ProbeOutcome::Failed("no".into()), plus(now(), 1.0)),
        plus(now(), 1.0),
    );

    // Claude Code writing its config in the other folder is a sign of life,
    // too: the newer `.claude.json` wins over a session seen earlier.
    let stamped = ClaudeJsonRead {
        stamp: Some((to_ns(ago(5)), 10)),
        ..fx.read(None, now())
    };
    store.apply_cache_reads(&[stamped], now());
    store.refresh(&fx.id, RefreshReason::Manual, plus(now(), 90.0));
    let plan = store.due_probe(plus(now(), 90.0)).expect("a probe");
    assert_eq!(plan.folder, fx.folder.id);
    assert_eq!(plan.identity_file, fx.dir.join(".claude.json"));
}

#[test]
fn the_default_folder_runs_with_the_variable_unset() {
    let fx = Fx::new();
    let main = run_folder(&fx.roots.home.join(".claude"), None);
    let account = account("uuid:acc-1", Some("me@x.dev"), &[&main], &[]);
    let mut store = fx.store_of(fx.config(false), &[account], std::slice::from_ref(&main));
    store.refresh(&fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a probe");
    assert_eq!(plan.config_dir_env, None);
    assert!(!plan.spec.env.iter().any(|(name, _)| name
        .to_string_lossy()
        .eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")));
    // `~\.claude.json`, beside the folder rather than in it.
    assert_eq!(plan.identity_file, fx.roots.home.join(".claude.json"));
}

/// Any other folder runs with `CLAUDE_CONFIG_DIR` naming it, even when the
/// registry recorded no raw value for it: unset, Claude Code would run as
/// `~\.claude`'s login while the checks read this folder's file.
#[test]
fn another_folder_always_names_itself() {
    let fx = Fx::new();
    let bare = run_folder(&fx.dir, None);
    let account = account("uuid:acc-1", Some("me@x.dev"), &[&bare], &[]);
    let mut store = fx.store_of(fx.config(false), &[account], std::slice::from_ref(&bare));
    store.refresh(&fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a probe");
    let dir = fx.dir.to_string_lossy().into_owned();
    assert_eq!(plan.config_dir_env.as_deref(), Some(dir.as_str()));
    let named: Vec<OsString> = plan
        .spec
        .env
        .iter()
        .filter(|(name, _)| {
            name.to_string_lossy()
                .eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")
        })
        .map(|(_, value)| value.clone())
        .collect();
    assert_eq!(named, [OsString::from(&dir)]);
    assert_eq!(plan.identity_file, fx.dir.join(".claude.json"));
}

#[test]
fn requests_are_answered_in_order_one_at_a_time() {
    let fx = Fx::new();
    let (mut store, side) = fx.store_with_side();
    let (_, side_folder) = fx.side();
    let reads = [fx.read(None, now()), fx.side_read(&side_folder)];
    store.apply_cache_reads(&reads, now());
    // Asked in the other order than the registry's.
    store.refresh(&side, RefreshReason::Manual, now());
    store.refresh(&fx.id, RefreshReason::Manual, now());
    let first = store.due_probe(now()).expect("first");
    assert_eq!(first.identity, side);
    assert!(store.due_probe(now()).is_none(), "one at a time");
    store.finish_probe(
        reading(&first, Some(1.0), Some(1.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    let second = store.due_probe(plus(now(), 1.0)).expect("second");
    assert_eq!(second.identity, fx.id);
    store.finish_probe(
        reading(&second, Some(2.0), Some(1.0), plus(now(), 2.0)),
        plus(now(), 2.0),
    );
    assert!(store.due_probe(plus(now(), 2.0)).is_none());
}

#[test]
fn the_schedule_picks_the_stalest_account_and_waits_five_minutes() {
    let fx = Fx::new();
    let (mut store, side) = fx.store_with_side();
    let (_, side_folder) = fx.side();
    let mut side_read = fx.side_read(&side_folder);
    let mut cached = AccountUsage::new(side.clone(), UsageSource::Cache, ago(3600));
    cached.five_hour = Some(five(9.0));
    side_read.cached_usage = Some(cached);
    let reads = [fx.read(Some(20.0), ago(1200)), side_read];

    let plan = quiet_cycle(&mut store, &reads, now()).expect("both are stale");
    assert_eq!(plan.identity, side, "the older reading goes first");
    store.finish_probe(
        reading(&plan, Some(9.0), Some(1.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    let plan = quiet_cycle(&mut store, &reads, plus(now(), 20.0)).expect("the other");
    assert_eq!(plan.identity, fx.id);
    store.finish_probe(
        reading(&plan, Some(9.0), Some(1.0), plus(now(), 21.0)),
        plus(now(), 21.0),
    );
    // Both fresh now: nothing for 5 minutes.
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 120.0)).is_none());
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 330.0)).is_some());
}

#[test]
fn the_interval_and_the_allowance_gate_the_schedule() {
    let fx = Fx::new();
    let reads = [fx.read(Some(20.0), ago(3600))];
    let mut store = fx.store(false);
    store.set_probe_interval_minutes(0);
    assert!(
        quiet_cycle(&mut store, &reads, now()).is_none(),
        "probing is off"
    );
    store.set_probe_interval_minutes(60);
    // An hour and a minute: stale by the interval.
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 60.0)).is_some());
    let mut again = fx.store(false);
    again.set_probe_interval_minutes(60);
    // Not an hour old yet.
    assert!(quiet_cycle(&mut again, &[fx.read(Some(20.0), ago(1800))], now()).is_none());

    let mut dev = fx.store(false);
    dev.set_probes_allowed(false);
    assert!(
        quiet_cycle(&mut dev, &reads, now()).is_none(),
        "dev runs don't probe on their own"
    );
    // A request still goes through (only a sealed run refuses it).
    dev.refresh(&fx.id, RefreshReason::Manual, now());
    assert!(dev.due_probe(now()).is_some());
    // Nobody signed in, nothing to ask.
    let mut out = fx.store(false);
    let mut signed_out = fx.read(None, now());
    signed_out.identity = None;
    let mut account = fx.account.clone();
    account.is_signed_in = false;
    out.set_accounts(&[account], std::slice::from_ref(&fx.folder), now());
    assert!(quiet_cycle(&mut out, &[signed_out], now()).is_none());
    out.refresh(&fx.id, RefreshReason::Manual, now());
    assert!(out.due_probe(now()).is_none());
    assert_eq!(out.fetch_state_of(&fx.id), schedule::not_signed_in());
}

/// What a run starts with (`UsageStoreConfig::for_run`): a sealed run never
/// launches Claude Code, not even on request, and never reads Claude
/// Desktop's cache whatever the settings say; a dev run (`--no-install`)
/// probes only on request unless `AGENTNOTCH_USAGE_PROBE=1`.
#[test]
fn a_runs_configuration_follows_its_flags() {
    let fx = Fx::new();
    let settings = ControlSettings::default();
    let flags = |sealed: bool, no_install: bool, usage_probe: bool| DevFlags {
        sealed,
        no_install,
        usage_probe_on_dev_run: usage_probe,
        ..DevFlags::default()
    };
    let config =
        |flags: &DevFlags| UsageStoreConfig::for_run(fx.roots.home.clone(), flags, &settings);

    let live = config(&flags(false, false, false));
    assert!(live.probes_allowed && !live.probes_disabled && live.reads_desktop);
    assert_eq!(live.probe_interval_minutes, 5);
    assert_eq!(live.home, fx.roots.home);
    assert!(!live.mirrors_default);
    let dev = config(&flags(false, true, false));
    assert!(!dev.probes_allowed && !dev.probes_disabled && dev.reads_desktop);
    assert!(config(&flags(false, true, true)).probes_allowed);
    let off = UsageStoreConfig::for_run(
        fx.roots.home.clone(),
        &flags(false, false, false),
        &ControlSettings {
            usage_probe_interval_minutes: 0,
            reads_desktop_usage_cache: false,
            ..ControlSettings::default()
        },
    );
    assert_eq!(off.probe_interval_minutes, 0);
    assert!(!off.reads_desktop);

    let sealed = config(&flags(true, false, true));
    assert!(!sealed.probes_allowed && sealed.probes_disabled && !sealed.reads_desktop);
    let mut store = fx.store_of(
        sealed,
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
    );
    let reads = [fx.read(Some(20.0), ago(20 * 60))];
    let mut desktop = Desktop::with(desktop_reading(50.0, now()));
    assert!(cycle(&mut store, &reads, &mut desktop, now()).is_none());
    assert!(desktop.asked.is_empty(), "a sealed run read Claude Desktop");
    let asked = ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    assert!(asked.desktop_reads.is_empty());
    assert!(store.due_probe(now()).is_none());
    assert_eq!(
        store.fetch_state_of(&fx.id),
        UsageFetchState::Unavailable(PROBES_OFF_TEXT.into())
    );
}

#[test]
fn refusals_say_why_and_a_missing_claude_backs_off() {
    // A sealed run never launches Claude Code, not even on request.
    let fx = Fx::new();
    let mut sealed = fx.store_of(
        UsageStoreConfig {
            probes_disabled: true,
            probes_allowed: false,
            ..fx.config(false)
        },
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
    );
    sealed.refresh(&fx.id, RefreshReason::Manual, now());
    assert!(sealed.due_probe(now()).is_none());
    assert_eq!(
        sealed.fetch_state_of(&fx.id),
        UsageFetchState::Unavailable(PROBES_OFF_TEXT.into())
    );

    // No Claude Code anywhere: a failure, with a backoff, not a retry every cycle.
    std::fs::remove_file(&fx.claude).unwrap();
    let mut store = fx.store(false);
    let reads = [fx.read(None, now())];
    assert!(quiet_cycle(&mut store, &reads, now()).is_none());
    assert_eq!(
        store.fetch_state_of(&fx.id),
        UsageFetchState::Failed(CLAUDE_NOT_FOUND_TEXT.into())
    );
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 20.0)).is_none());
    assert_eq!(store.last_probe_of(&fx.id), Some(now()));
    // It turns up (the user installed it): asked again once the wait is over.
    std::fs::write(&fx.claude, b"").unwrap();
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 330.0)).is_some());

    // The engine was never told where things are.
    let mut bare = UsageStore::with_config(fx.config(false));
    bare.set_accounts(
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
        now(),
    );
    bare.refresh(&fx.id, RefreshReason::Manual, now());
    assert!(bare.due_probe(now()).is_none());
    assert_eq!(
        bare.fetch_state_of(&fx.id),
        UsageFetchState::Failed(NOT_SET_UP_TEXT.into())
    );
}

#[test]
fn claude_is_found_where_settings_and_memory_say() {
    let fx = Fx::new();
    let chosen = fx.roots.home.join("tools").join("claude.exe");
    std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
    std::fs::write(&chosen, b"").unwrap();
    let mut store = fx.store(false);
    let reads = [fx.read(None, now())];
    store.apply_cache_reads(&reads, now());

    store.set_claude_binary_path(Some(chosen.clone()));
    store.refresh(&fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a probe");
    assert_eq!(plan.spec.program, chosen, "the Settings choice comes first");
    assert_eq!(
        store.remembered_binary(),
        None,
        "only a working run is remembered"
    );
    store.finish_probe(
        reading(&plan, Some(1.0), Some(1.0), plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    assert_eq!(store.remembered_binary(), Some(chosen.as_path()));

    // The choice is cleared: the one that worked is tried next.
    store.set_claude_binary_path(None);
    store.refresh(&fx.id, RefreshReason::Manual, plus(now(), 90.0));
    let plan = store.due_probe(plus(now(), 90.0)).expect("a probe");
    assert_eq!(plan.spec.program, chosen);
    // A failed run isn't remembered over it.
    store.finish_probe(
        result(
            &plan,
            ProbeOutcome::Failed("boom".into()),
            plus(now(), 91.0),
        ),
        plus(now(), 91.0),
    );
    assert_eq!(store.remembered_binary(), Some(chosen.as_path()));
}

/// Runs one requested probe to its answer; when it finished.
fn answer_next(
    store: &mut UsageStore,
    fx: &Fx,
    reads: &[ClaudeJsonRead],
    at: SystemTime,
    outcome: impl Fn(&ProbePlan) -> ProbeOutcome,
) -> SystemTime {
    ask(store, reads, &fx.id, RefreshReason::Manual, at);
    let plan = store.due_probe(at).expect("a probe");
    let finished = plus(at, 1.0);
    store.finish_probe(result(&plan, outcome(&plan), finished), finished);
    finished
}

fn saved(store: &UsageStore, id: &IdentityId) -> PersistedUsageAccount {
    store.state_file().accounts[id.as_str()].clone()
}

#[test]
fn what_each_kind_of_answer_means() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    let failed = |_: &ProbePlan| ProbeOutcome::Failed("boom".into());

    // Failed: backoff(n), doubling while the schedule keeps asking, and the
    // reason shown. (A requested probe clears the count first.)
    let mut at = plus(now(), 400.0);
    for failures in 1..=3u32 {
        let plan = quiet_cycle(&mut store, &reads, at).expect("due on the schedule");
        let finished = plus(at, 1.0);
        store.finish_probe(result(&plan, failed(&plan), finished), finished);
        assert_eq!(
            store.fetch_state_of(&fx.id),
            UsageFetchState::Failed("boom".into())
        );
        let kept = saved(&store, &fx.id);
        assert_eq!(kept.failure_count, i64::from(failures));
        assert_eq!(
            kept.next_attempt_at,
            Some(IsoSeconds(finished + schedule::backoff(failures)))
        );
        // Not before its time, and not within five minutes of the last probe.
        assert!(quiet_cycle(&mut store, &reads, plus(finished, 60.0)).is_none());
        at = plus(finished, 301.0 + 480.0 * f64::from(failures));
    }
    assert_eq!(schedule::backoff(1), Duration::from_secs(120));
    assert_eq!(schedule::backoff(3), Duration::from_secs(480));

    // Rate limited: at least five minutes, whatever the count says, and not
    // sooner than the server asked.
    let limited = |_: &ProbePlan| ProbeOutcome::RateLimited {
        retry_after: Some(Duration::from_secs(900)),
    };
    let finished = answer_next(&mut store, &fx, &reads, plus(at, 100.0), limited);
    let three = saved(&store, &fx.id);
    assert_eq!(
        three.failure_count, 1,
        "a forced request starts the count over"
    );
    assert_eq!(
        three.next_attempt_at,
        Some(IsoSeconds(finished + Duration::from_secs(900)))
    );
    assert_eq!(
        store.fetch_state_of(&fx.id),
        UsageFetchState::Failed(
            "Usage check paused (too many requests), retrying in 15 min".into()
        )
    );
    let finished = answer_next(&mut store, &fx, &reads, plus(at, 500.0), |_| {
        ProbeOutcome::RateLimited { retry_after: None }
    });
    let again = saved(&store, &fx.id);
    assert_eq!(
        again.next_attempt_at,
        Some(IsoSeconds(finished + Duration::from_secs(300)))
    );
    assert_eq!(
        store.fetch_state_of(&fx.id),
        UsageFetchState::Failed("Usage check paused (too many requests), retrying in 5 min".into())
    );

    // Unavailable: tried again in half an hour, the count starts over.
    let reason = "Usage limits aren't available for this login";
    let finished = answer_next(&mut store, &fx, &reads, plus(at, 900.0), |_| {
        ProbeOutcome::Unavailable(reason.into())
    });
    let unavailable = saved(&store, &fx.id);
    assert_eq!(unavailable.failure_count, 0);
    assert_eq!(
        unavailable.next_attempt_at,
        Some(IsoSeconds(finished + schedule::MAX_BACKOFF))
    );
    assert_eq!(
        store.fetch_state_of(&fx.id),
        UsageFetchState::Unavailable(reason.into())
    );

    // Signed out: the ring says so.
    let finished = answer_next(&mut store, &fx, &reads, plus(at, 1300.0), |_| {
        ProbeOutcome::SignedOut
    });
    assert_eq!(store.fetch_state_of(&fx.id), schedule::not_signed_in());
    assert_eq!(
        store.ring_reading(&fx.id, finished),
        agentnotch_engine::runtime_types::RingReading::SignInNeeded
    );

    // A reading ends all of it.
    answer_next(&mut store, &fx, &reads, plus(at, 1700.0), |plan| {
        reading(plan, Some(12.0), Some(3.0), plan.planned_at).outcome
    });
    assert_eq!(store.fetch_state_of(&fx.id), UsageFetchState::Idle);
    let fine = saved(&store, &fx.id);
    assert_eq!(fine.failure_count, 0);
    assert_eq!(fine.next_attempt_at, None);
    assert_eq!(five_of(&store, &fx.id), Some(12.0));
}

#[test]
fn a_store_the_probe_found_is_unavailable_for_half_an_hour() {
    // The probe's own check (the marker file) answers for a folder the
    // classification took for a run folder.
    let fx = Fx::new();
    std::fs::create_dir_all(&fx.dir).unwrap();
    std::fs::write(
        fx.dir.join(".claude.json"),
        br#"{"oauthAccount":{"accountUuid":"acc-1","emailAddress":"me@x.dev"}}"#,
    )
    .unwrap();
    std::fs::write(fx.dir.join(STORE_MARKER), b"").unwrap();
    let mut store = fx.store(false);
    store.refresh(&fx.id, RefreshReason::Manual, now());
    let plan = store
        .due_probe(now())
        .expect("planned from what the registry says");
    let runner = ScriptedRunner::default();
    let clock = FakeClock::at_ms(1_800_000_000_000);
    let probed = run_probe_with(
        &plan,
        &runner,
        &clock,
        &ProbeTiming::default(),
        &ClaudeJsonReader::new(),
    );
    assert!(runner.spawned().is_empty(), "never run in a store");
    store.finish_probe(probed, plus(now(), 1.0));
    assert_eq!(
        store.fetch_state_of(&fx.id),
        UsageFetchState::Unavailable(NO_RUN_FOLDER_TEXT.into())
    );
    let saved = &store.state_file().accounts[fx.id.as_str()];
    assert_eq!(
        saved.next_attempt_at,
        Some(IsoSeconds(plus(now(), 1.0) + schedule::MAX_BACKOFF))
    );
}

/// An answer from a folder that changed hands, thrown away at `at`.
fn thrown_away(
    store: &mut UsageStore,
    fx: &Fx,
    plan: &ProbePlan,
    after: Option<IdentityId>,
    at: SystemTime,
) {
    let mut answer = reading(plan, Some(99.0), Some(99.0), at);
    answer.folder_identity_after = after.clone();
    assert!(store.finish_probe(answer, at).is_none());
    assert!(!store.is_probing());
    assert!(store.usage_of(&fx.id).is_none(), "{after:?}: not recorded");
    assert_eq!(store.fetch_state_of(&fx.id), UsageFetchState::Idle);
    assert_eq!(
        store.last_probe_of(&fx.id),
        None,
        "not a probe of this account"
    );
    let kept = saved(store, &fx.id);
    assert_eq!(kept.failure_count, 0, "no backoff");
    assert_eq!(kept.last_probe_at, None);
    // Not straight away: the folder may still be changing hands.
    assert_eq!(kept.next_attempt_at, Some(IsoSeconds(plus(at, 60.0))));
}

/// The probe never speaks for another account: an answer from a folder that
/// changed hands is thrown away, however good it looks.
#[test]
fn a_folder_that_changed_hands_is_discarded_without_a_backoff() {
    let fx = Fx::new();
    let reads = [fx.read(None, now())];
    let mut store = fx.store(false);
    ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    let plan = store.due_probe(now()).expect("a probe");
    // Another login now, and nobody signed in.
    thrown_away(
        &mut store,
        &fx,
        &plan,
        Some(IdentityId::from("uuid:acc-2")),
        plus(now(), 1.0),
    );

    // The schedule waits a minute; a request clears the wait.
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 30.0)).is_none());
    ask(
        &mut store,
        &reads,
        &fx.id,
        RefreshReason::Manual,
        plus(now(), 70.0),
    );
    let again = store
        .due_probe(plus(now(), 70.0))
        .expect("a request clears the wait");
    assert_eq!(again.planned_at, plus(now(), 70.0));
    thrown_away(&mut store, &fx, &again, None, plus(now(), 71.0));
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 90.0)).is_none());
    assert!(quiet_cycle(&mut store, &reads, plus(now(), 132.0)).is_some());
}

/// A request waits for Claude Desktop's cache (it costs nothing and may be
/// fresh enough), for up to 5 s.
#[test]
fn a_request_waits_for_claude_desktop_before_it_asks_claude_code() {
    let fx = Fx::new();
    // Older than a ring click tolerates (2 minutes), not yet due on the schedule.
    let reads = [fx.read(Some(20.0), ago(130))];

    // A fresh Desktop reading holds the ring click off.
    let mut store = fx.store(true);
    let request = ask(&mut store, &reads, &fx.id, RefreshReason::RingClick, now());
    assert_eq!(
        request.desktop_reads,
        vec![(fx.id.clone(), "org-1".to_owned())]
    );
    assert_eq!(
        request.wait_until,
        Some(plus(now(), 20.0)),
        "waiting for the reads"
    );
    assert!(store.is_waiting_on(&fx.id));
    assert!(
        store.due_probe(plus(now(), 1.0)).is_none(),
        "still reading Desktop"
    );
    store.accept_desktop(
        &fx.id,
        &desktop_reading(40.0, plus(now(), 1.0)),
        plus(now(), 1.0),
    );
    assert!(
        store.due_probe(plus(now(), 1.0)).is_none(),
        "Desktop's reading is fresh"
    );
    assert!(!store.is_waiting_on(&fx.id));
    assert_eq!(five_of(&store, &fx.id), Some(40.0));

    // A miss leaves the cache's two minutes and more: asked.
    let mut store = fx.store(true);
    let request = ask(&mut store, &reads, &fx.id, RefreshReason::RingClick, now());
    assert_eq!(request.desktop_reads.len(), 1);
    store.accept_desktop(&fx.id, &desktop_miss(), plus(now(), 1.0));
    assert!(store.due_probe(plus(now(), 1.0)).is_some());

    // No answer at all: decided after 5 s.
    let mut store = fx.store(true);
    ask(&mut store, &reads, &fx.id, RefreshReason::RingClick, now());
    assert!(store.due_probe(plus(now(), 4.0)).is_none());
    assert!(store.due_probe(plus(now(), 5.0)).is_some());

    // A switched-off ring is never read for, nor asked.
    let mut store = fx.store(true);
    store.set_paused([fx.id.clone()].into());
    let request = ask(&mut store, &reads, &fx.id, RefreshReason::Manual, now());
    assert_eq!(request, agentnotch_engine::usage::RefreshRequest::default());
    assert!(store.due_probe(plus(now(), 10.0)).is_none());
}

#[test]
fn a_forgotten_account_leaves_no_spinner_behind() {
    let fx = Fx::new();
    let (mut store, side) = fx.store_with_side();
    let (_, side_folder) = fx.side();
    let reads = [fx.read(None, now()), fx.side_read(&side_folder)];
    store.apply_cache_reads(&reads, now());
    store.refresh_all(now());
    let plan = store.due_probe(now()).expect("the first");

    // The second account is forgotten while it waits its turn.
    store.set_accounts(
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
        plus(now(), 1.0),
    );
    store.finish_probe(
        reading(&plan, Some(1.0), Some(1.0), plus(now(), 2.0)),
        plus(now(), 2.0),
    );
    assert!(store.due_probe(plus(now(), 2.0)).is_none());
    assert!(!store.is_fetching());
    assert_eq!(store.fetch_state_of(&side), UsageFetchState::Idle);
    // An answer for an account the registry dropped is nobody's.
    let mut ghost = reading(&plan, Some(1.0), Some(1.0), plus(now(), 3.0));
    ghost.plan.identity = side.clone();
    ghost.folder_identity_after = Some(side.clone());
    assert!(store.finish_probe(ghost, plus(now(), 3.0)).is_none());
    assert!(store.usage_of(&side).is_none());
}
