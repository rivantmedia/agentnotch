//! The usage store's first half: accounts, status lines, snapshots, Claude
//! Desktop readings, `.claude.json` caches, the ring reading and
//! `usage-state.json` (A3_UsageStoreTests, PP_UsageTests, A3_RingReadingTests:
//! every test that needs no probe; the probe half is `usage_store_probes.rs`).
//!
//! The Swift tests use `Date()` and async polls; these drive the store with
//! explicit times. A "poll cycle" is `prune_status_lines` + `apply_cache_reads`
//! (+ `desktop_due` / `accept_desktop`), exactly what the hub's 20-second loop
//! does. The Swift fixture's stand-in probe is `accept_snapshot` with the
//! snapshot a probe would have made.

mod usage_support;

use agentnotch_engine::core::time::IsoSeconds;
use agentnotch_engine::model::AccountUsage;
use agentnotch_engine::model::{
    Account, Attribution, DesktopReading, DesktopWindow, IdentityId, RingStatus, RunFolder,
    SessionId, StatusLineMessage, UsageFetchState, UsageSource, UsageWindow,
};
use agentnotch_engine::persist::usage::{
    PersistedAccountUsage, PersistedReading, PersistedStatusLine, PersistedStatusLineReadings,
    PersistedUsageAccount, PersistedWindow, UsageStateFile,
};
use agentnotch_engine::runtime_types::{
    ClaudeJsonRead, IngestContext, RingReading, UsageObservation,
};
use agentnotch_engine::usage::ring_windows::WEEKLY_ID;
use agentnotch_engine::usage::schedule;
use agentnotch_engine::usage::{UsageStore, UsageStoreConfig};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use usage_support::{account, fake_drive, folder_id, join, login, run_folder, store_folder};

const DAY: i64 = 86_400;
const WEEKLY: u64 = UsageWindow::WEEKLY_DURATION_S;

/// Whole seconds from the epoch (negative: before it).
fn t(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

/// The fixture's "now": the Swift `UsageFixture.now`.
fn now() -> SystemTime {
    t(1_800_000_000)
}

fn ago(seconds: i64) -> SystemTime {
    t(1_800_000_000 - seconds)
}

fn plus(base: SystemTime, seconds: f64) -> SystemTime {
    base + Duration::from_secs_f64(seconds)
}

/// One account (`uuid:acc-1`, in organization `org-1`) with one run folder,
/// the Swift `UsageFixture`.
struct Fx {
    home: PathBuf,
    dir: PathBuf,
    folder: RunFolder,
    account: Account,
    id: IdentityId,
}

impl Fx {
    fn new() -> Fx {
        let home = join(&fake_drive(), &["home"]);
        let dir = home.join(".claude-work");
        let folder = run_folder(&dir, Some(&dir.to_string_lossy()));
        let mut account = account("uuid:acc-1", Some("me@x.dev"), &[&folder], &[]);
        account.organization_uuid = Some("org-1".into());
        Fx {
            home,
            dir,
            folder,
            account,
            id: IdentityId::from("uuid:acc-1"),
        }
    }

    fn store_with(&self, config: UsageStoreConfig) -> UsageStore {
        let mut store = UsageStore::with_config(config);
        store.start();
        store.set_accounts(
            std::slice::from_ref(&self.account),
            std::slice::from_ref(&self.folder),
            now(),
        );
        store
    }

    /// The Swift `makeStore(desktopOn:)`.
    fn store_desktop(&self, desktop: bool) -> UsageStore {
        self.store_with(UsageStoreConfig {
            home: self.home.clone(),
            reads_desktop: desktop,
            ..UsageStoreConfig::default()
        })
    }

    /// A second signed-in account (`~/.claude-side`), next to the first.
    fn side(&self) -> (Account, RunFolder) {
        let dir = self.home.join(".claude-side");
        let folder = run_folder(&dir, Some(&dir.to_string_lossy()));
        let account = account("uuid:acc-2", Some("me@y.dev"), &[&folder], &[]);
        (account, folder)
    }

    /// The fixture's weekly window (`UsageFixture.body` resets it in 3 days).
    fn week(&self, utilization: f64) -> UsageWindow {
        UsageWindow::new(
            utilization,
            Some(now() + Duration::from_secs(3 * DAY as u64)),
            WEEKLY,
        )
    }

    /// `.claude.json` read of the fixture's folder: signed in, in `org-1`,
    /// optionally with Claude Code's cached usage (`writeGlobalConfig`).
    fn read(&self, cached_session: Option<f64>, fetched_at: SystemTime) -> ClaudeJsonRead {
        ClaudeJsonRead {
            folder: self.folder.id.clone(),
            identity: Some(login(Some("me@x.dev"), Some("acc-1"), Some("org-1"))),
            cached_usage: cached_session.map(|session| self.cached(session, fetched_at)),
            stamp: None,
            error: None,
        }
    }

    /// Claude Code's cached snapshot as `usage::claude_json` makes it:
    /// `body(session:)`, dated when it was fetched.
    fn cached(&self, session: f64, fetched_at: SystemTime) -> AccountUsage {
        let mut usage = AccountUsage::new(self.id.clone(), UsageSource::Cache, fetched_at);
        usage.five_hour = Some(UsageWindow::new(
            session,
            Some(now() + Duration::from_secs(3 * 3600)),
            UsageWindow::SESSION_DURATION_S,
        ));
        usage.seven_day = Some(self.week(10.0));
        usage
    }

    fn line(&self, weekly: f64, process: u32, at: SystemTime) -> L {
        L {
            five: None,
            weekly: Some(self.week(weekly)),
            process,
            session: format!("s{process}"),
            at,
            start: None,
            trusted: true,
            folder: self.dir.to_string_lossy().into_owned(),
            attribution: Attribution::Known(Some(self.id.clone())),
        }
    }
}

/// One status line update, like the Swift `line(...)`.
struct L {
    five: Option<UsageWindow>,
    weekly: Option<UsageWindow>,
    process: u32,
    session: String,
    at: SystemTime,
    /// The kernel's start time of the process.
    start: Option<SystemTime>,
    /// The hub trusts the pid (the hooks don't contradict it).
    trusted: bool,
    folder: String,
    attribution: Attribution,
}

impl L {
    fn started(mut self, start: SystemTime) -> L {
        self.start = Some(start);
        self
    }

    fn session(mut self, session: &str) -> L {
        self.session = session.to_owned();
        self
    }

    fn untrusted(mut self) -> L {
        self.trusted = false;
        self
    }

    fn five(mut self, window: UsageWindow) -> L {
        self.five = Some(window);
        self
    }
}

fn message(l: &L) -> StatusLineMessage {
    StatusLineMessage {
        session_id: SessionId::from(l.session.as_str()),
        cwd: None,
        transcript_path: None,
        config_dir_env: Some(l.folder.clone()),
        account_id: Some(l.folder.as_str().into()),
        received_at: l.at,
        rate_limits: None,
        five_hour: l.five.clone(),
        seven_day: l.weekly.clone(),
        context_used_percent: None,
        context_window_size: None,
        model_id: None,
        model_display_name: None,
        cost_usd: None,
        session_name: None,
        claude_code_version: None,
        pid: Some(l.process),
    }
}

fn feed(store: &mut UsageStore, l: L) -> Option<UsageObservation> {
    let ctx = IngestContext {
        attribution: l.attribution.clone(),
        account: None,
        trusted_pid: l.trusted.then_some(l.process),
        pid_started: l.start,
    };
    store.ingest_status_line(&message(&l), ctx, l.at)
}

fn weekly_of(store: &UsageStore, id: &IdentityId) -> Option<f64> {
    store
        .usage_of(id)
        .and_then(|usage| usage.seven_day.as_ref())
        .map(|window| window.utilization)
}

fn weekly_obs(observation: &UsageObservation) -> Option<f64> {
    observation
        .windows
        .iter()
        .find(|(id, _, _)| id == WEEKLY_ID)
        .map(|(_, utilization, _)| *utilization)
}

// MARK: - usage-state.json

#[test]
fn state_round_trips_through_the_file() {
    // A3_UsageStatePersistenceTests.stateRoundTripsThroughTheFile (the 0600
    // mode is the file layer's: core::atomic).
    let at = t(1_800_000_000);
    let mut file = UsageStateFile::default();
    file.accounts.insert(
        "a".into(),
        PersistedUsageAccount {
            last_probe_at: Some(IsoSeconds(at)),
            failure_count: 2,
            next_attempt_at: Some(IsoSeconds(at + Duration::from_secs(240))),
            last_full_reading: Some(PersistedAccountUsage {
                account_id: "a".into(),
                five_hour: Some(PersistedWindow {
                    utilization: 12.0,
                    resets_at: Some(IsoSeconds(at)),
                    duration: 18_000.0,
                }),
                seven_day: None,
                scoped: Vec::new(),
                extra_usage: None,
                subscription_type: None,
                source: "probe".into(),
                updated_at: IsoSeconds(at),
                taken_after: None,
            }),
            status_lines: None,
        },
    );
    assert_eq!(UsageStateFile::parse(&file.encode()), Some(file));
    // A damaged file is only a cache: start empty.
    assert_eq!(UsageStateFile::parse(b"{"), None);
    // So is one from a newer version.
    assert_eq!(
        UsageStateFile::parse(br#"{"version":2,"accounts":{}}"#),
        None
    );
}

#[test]
fn restoring_drops_gone_accounts_and_ancient_readings() {
    let now = t(1_800_000_000);
    let reading = |id: &str, updated_at: SystemTime| PersistedAccountUsage {
        account_id: id.into(),
        five_hour: None,
        seven_day: None,
        scoped: Vec::new(),
        extra_usage: None,
        subscription_type: None,
        source: "probe".into(),
        updated_at: IsoSeconds(updated_at),
        taken_after: None,
    };
    let mut file = UsageStateFile::default();
    file.accounts.insert(
        "kept".into(),
        PersistedUsageAccount {
            last_probe_at: Some(IsoSeconds(now)),
            last_full_reading: Some(reading("kept", now)),
            ..PersistedUsageAccount::default()
        },
    );
    file.accounts.insert(
        "old".into(),
        PersistedUsageAccount {
            last_full_reading: Some(reading("old", now - Duration::from_secs(9 * DAY as u64))),
            ..PersistedUsageAccount::default()
        },
    );
    file.accounts.insert(
        "gone".into(),
        PersistedUsageAccount {
            last_probe_at: Some(IsoSeconds(now)),
            ..PersistedUsageAccount::default()
        },
    );
    let known: BTreeSet<String> = ["kept".to_owned(), "old".to_owned()].into();
    let restored = file.restored(Some(&known), now);
    assert_eq!(restored.accounts.keys().collect::<Vec<_>>(), vec!["kept"]);
    assert!(file.restored(None, now).accounts.contains_key("gone"));
    // A reading of 9 days ago is dropped, and its account with it (nothing
    // else was in it).
    assert!(!file.restored(None, now).accounts.contains_key("old"));
}

// MARK: - Early resets

/// Without any check: a session working through the reset brings the ring
/// down, and another session's older numbers don't bring it back.
#[test]
fn a_working_session_brings_the_ring_down_by_itself() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    let start = ago(900);
    feed(&mut store, fx.line(60.0, 101, start));
    feed(&mut store, fx.line(62.0, 202, plus(start, 10.0)));
    feed(&mut store, fx.line(60.0, 101, plus(start, 20.0)));
    assert_eq!(weekly_of(&store, &fx.id), Some(62.0));

    feed(&mut store, fx.line(3.0, 101, plus(start, 600.0)));
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    feed(&mut store, fx.line(62.0, 202, plus(start, 700.0)));
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    // A new session in the same process (`/clear`) is still that process.
    feed(
        &mut store,
        fx.line(62.0, 202, plus(start, 800.0))
            .session("after-clear"),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
}

/// After a relaunch the reading kept from before the reset shows until
/// something known to be newer arrives: a session that started after it.
#[test]
fn a_session_started_after_the_reading_replaces_it() {
    // A3_EarlyResetTests.aSessionStartedAfterTheReadingReplacesIt (no probe).
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    // Claude Code's own cache, from before the reset: 10% of the week.
    store.apply_cache_reads(&[fx.read(Some(20.0), ago(3600))], now());
    assert_eq!(weekly_of(&store, &fx.id), Some(10.0));

    // A session older than that reading can't tell which came first.
    feed(
        &mut store,
        fx.line(3.0, 101, now()).session("old").started(ago(7200)),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(10.0));
    // One started after it can: Claude Code has no rate limits before a
    // process's first response.
    feed(
        &mut store,
        fx.line(3.0, 202, now()).session("new").started(ago(1800)),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
}

/// The history gets what the ring shows: another process's lagging, lower
/// numbers are not sent, and neither is a stale first report.
#[test]
fn only_what_the_ring_shows_is_recorded() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    let start = ago(600);
    let mut seen = Vec::new();

    seen.extend(feed(&mut store, fx.line(45.0, 101, start)));
    assert_eq!(seen.len(), 1);
    // Another process, its numbers a little behind: not shown, not sent.
    seen.extend(feed(&mut store, fx.line(43.0, 202, plus(start, 5.0))));
    assert_eq!(weekly_of(&store, &fx.id), Some(45.0));
    assert_eq!(seen.len(), 1);
    // Shown again when it moves ahead.
    seen.extend(feed(&mut store, fx.line(46.0, 202, plus(start, 10.0))));
    assert_eq!(seen.last().and_then(weekly_obs), Some(46.0));
    assert_eq!(seen[0].source, UsageSource::StatusLine);
    assert_eq!(seen[0].identity, fx.id);
}

/// A relaunch keeps what each running process said, so an idle one
/// re-rendering its pre-reset numbers is still a repeat. (The Swift test's
/// stand-in probe is the snapshot a probe would have made.)
#[test]
fn a_relaunch_remembers_what_each_process_said() {
    // A3_EarlyResetTests.aRelaunchRemembersWhatEachProcessSaid
    let fx = Fx::new();
    let started = ago(3600);
    let mut store = fx.store_desktop(false);
    feed(&mut store, fx.line(62.0, 101, ago(600)).started(started));
    let mut check = AccountUsage::new(fx.id.clone(), UsageSource::Probe, ago(60));
    check.seven_day = Some(fx.week(3.0));
    check.taken_after = Some(ago(61));
    store.accept_snapshot(check, ago(60));
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    let file = store.save_now();

    // The next run, the same process still running: a repeat, not news.
    let mut next = fx.store_desktop(false);
    next.restore(&file, now());
    assert_eq!(weekly_of(&next, &fx.id), Some(3.0));
    let seen = feed(&mut next, fx.line(62.0, 101, now()).started(started));
    assert_eq!(weekly_of(&next, &fx.id), Some(3.0));
    assert!(seen.is_none());

    // A new process that got the same pid is another process: its line is news.
    let mut reused = fx.store_desktop(false);
    reused.restore(&file, now());
    let seen = feed(
        &mut reused,
        fx.line(62.0, 101, now())
            .started(started + Duration::from_secs(30)),
    );
    assert!(seen.is_some_and(|o| o.source == UsageSource::StatusLine));
}

/// The reading that brought the ring down comes back after a relaunch even
/// when its process has ended since, rather than the older snapshot from
/// before the reset.
#[test]
fn an_ended_processs_reading_outlives_a_relaunch() {
    let fx = Fx::new();
    // Claude Code's own cache, from before the reset: 10% of the week.
    let reads = [fx.read(Some(20.0), ago(3600))];
    let mut store = fx.store_desktop(false);
    store.apply_cache_reads(&reads, now());
    assert_eq!(weekly_of(&store, &fx.id), Some(10.0));
    feed(&mut store, fx.line(3.0, 101, now()).started(ago(1800)));
    assert_eq!(weekly_of(&store, &fx.id), Some(3.0));
    let file = store.save_now();

    // The next run; the process has ended.
    let mut next = fx.store_desktop(false);
    next.restore(&file, now());
    assert_eq!(weekly_of(&next, &fx.id), Some(3.0));
    next.prune_status_lines(now());
    next.apply_cache_reads(&reads, now());
    assert_eq!(weekly_of(&next, &fx.id), Some(3.0));
}

#[test]
fn saved_records_come_back_only_to_their_accounts_folders_within_a_week() {
    let fx = Fx::new();
    let (side, side_folder) = fx.side();
    let key = "pid:101@1700000000";
    let record = |weekly: f64, age: i64| {
        let at = ago(age);
        PersistedStatusLineReadings {
            last_report_at: IsoSeconds(at),
            five_hour: None,
            seven_day: Some(PersistedReading::from_model(
                &agentnotch_engine::model::UsageReading::new(fx.week(weekly), at, None),
            )),
        }
    };
    let restored = |folder: &str, key: &str, readings: PersistedStatusLineReadings| {
        let mut file = UsageStateFile::default();
        file.accounts.insert(
            fx.id.0.clone(),
            PersistedUsageAccount {
                status_lines: Some(vec![PersistedStatusLine {
                    folder: folder.to_owned(),
                    key: key.to_owned(),
                    readings,
                }]),
                ..PersistedUsageAccount::default()
            },
        );
        let mut store = UsageStore::with_config(UsageStoreConfig {
            home: fx.home.clone(),
            ..UsageStoreConfig::default()
        });
        store.set_accounts(
            &[fx.account.clone(), side.clone()],
            &[fx.folder.clone(), side_folder.clone()],
            now(),
        );
        store.restore(&file, now());
        weekly_of(&store, &fx.id)
    };
    let folder = fx.folder.id.as_str().to_owned();
    assert_eq!(restored(&folder, key, record(30.0, 60)), Some(30.0));
    // Not heard from for a week: its window has reset since.
    let week = schedule::STATUS_LINE_RETENTION.as_secs() as i64;
    assert_eq!(restored(&folder, key, record(30.0, week + 60)), None);
    // A folder another account holds now.
    assert_eq!(
        restored(side_folder.id.as_str(), key, record(30.0, 60)),
        None
    );
    // A key without the process's start could be any process's.
    assert_eq!(restored(&folder, "pid:101", record(30.0, 60)), None);
}

#[test]
fn a_process_not_heard_from_for_a_week_is_forgotten() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    feed(&mut store, fx.line(30.0, 101, now()));
    store.prune_status_lines(now());
    store.apply_cache_reads(&[fx.read(None, now())], now());
    assert_eq!(weekly_of(&store, &fx.id), Some(30.0));
    let later = now() + schedule::STATUS_LINE_RETENTION + Duration::from_secs(60);
    store.prune_status_lines(later);
    store.apply_cache_reads(&[fx.read(None, later)], later);
    assert!(store.usage_of(&fx.id).is_none());
}

/// Claude Desktop's reading is dated by the server's clock, to the second: it
/// is not taken for newer than a line from the minute before it.
#[test]
fn claude_desktops_date_allows_for_clock_skew() {
    for (line_age, shown) in [(30.0, 62.0), (120.0, 10.0)] {
        let fx = Fx::new();
        let mut store = fx.store_desktop(true);
        let observed = ago(10);
        let reading = DesktopReading::Reading {
            organization_uuid: "org-1".into(),
            windows: vec![
                DesktopWindow {
                    id: "session".into(),
                    label: None,
                    utilization: 20.0,
                    resets_at: Some(now() + Duration::from_secs(3 * 3600)),
                    duration_s: 18_000,
                },
                DesktopWindow {
                    id: "weekly_all".into(),
                    label: None,
                    utilization: 10.0,
                    resets_at: Some(now() + Duration::from_secs(3 * DAY as u64)),
                    duration_s: 604_800,
                },
            ],
            observed_at: observed,
            resets: None,
        };
        feed(
            &mut store,
            fx.line(62.0, 101, observed - Duration::from_secs_f64(line_age)),
        );
        // The poll cycle: caches first, then Claude Desktop.
        store.apply_cache_reads(&[fx.read(None, now())], now());
        let due = store.desktop_due(now(), None, false);
        assert_eq!(due, vec![(fx.id.clone(), "org-1".to_owned())]);
        let seen = store.accept_desktop(&fx.id, &reading, now());
        assert_eq!(
            weekly_of(&store, &fx.id),
            Some(shown),
            "line {line_age} s before Desktop's reading"
        );
        assert_eq!(seen.map(|o| o.source), Some(UsageSource::Desktop));
    }
}

/// A pid the session's hooks contradict isn't trusted: the line counts for
/// the session, with nothing known of when it was taken.
#[test]
fn a_pid_the_hooks_contradict_is_not_trusted() {
    let fx = Fx::new();
    let started = ago(60);
    let snapshot_time = ago(120);
    let mut store = fx.store_desktop(false);
    // An older, higher reading from another process.
    feed(&mut store, fx.line(62.0, 202, snapshot_time));
    // Trusted, this process's start would make its first report newer than that.
    feed(
        &mut store,
        fx.line(3.0, 101, now()).started(started).untrusted(),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(62.0));

    // A trusted pid that is another process's than the line names counts
    // for nobody's process either.
    let mut mismatched = fx.store_desktop(false);
    feed(&mut mismatched, fx.line(62.0, 202, snapshot_time));
    let line = fx.line(3.0, 101, now()).started(started);
    let ctx = IngestContext {
        attribution: line.attribution.clone(),
        account: None,
        trusted_pid: Some(777),
        pid_started: line.start,
    };
    mismatched.ingest_status_line(&message(&line), ctx, now());
    assert_eq!(weekly_of(&mismatched, &fx.id), Some(62.0));

    let mut trusting = fx.store_desktop(false);
    feed(&mut trusting, fx.line(62.0, 202, snapshot_time));
    feed(&mut trusting, fx.line(3.0, 101, now()).started(started));
    assert_eq!(weekly_of(&trusting, &fx.id), Some(3.0));
}

// MARK: - Attribution and the registry

/// An unsure session counts for no account (the folder changed hands around
/// when it started).
#[test]
fn an_unsure_line_is_left_out() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    let mut line = fx.line(40.0, 101, now());
    line.attribution = Attribution::Unsure(Some(fx.id.clone()));
    assert!(feed(&mut store, line).is_none());
    assert!(store.usage_of(&fx.id).is_none());
    // Nor does a line with neither window do anything.
    let mut empty = fx.line(40.0, 101, now());
    empty.weekly = None;
    assert!(feed(&mut store, empty).is_none());
}

/// A folder the registry has not grouped yet keeps its readings under the
/// folder, and `set_accounts` moves them to the folder's identity.
#[test]
fn a_folder_not_grouped_yet_is_adopted_by_its_identity() {
    let fx = Fx::new();
    let mut store = UsageStore::with_config(UsageStoreConfig {
        home: fx.home.clone(),
        ..UsageStoreConfig::default()
    });
    store.start();
    let mut line = fx.line(33.0, 101, now()).started(ago(30));
    line.attribution = Attribution::Known(None);
    feed(&mut store, line);
    let folder_key = IdentityId::from(folder_id(&fx.dir).as_str());
    assert_eq!(weekly_of(&store, &folder_key), Some(33.0));
    assert!(store.usage_of(&fx.id).is_none());

    store.set_accounts(
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
        now(),
    );
    assert_eq!(weekly_of(&store, &fx.id), Some(33.0));
    assert!(store.usage_of(&folder_key).is_none());
    // Known now: a later line finds the identity by its folder.
    let mut line = fx.line(34.0, 101, plus(now(), 5.0));
    line.attribution = Attribution::Known(None);
    feed(&mut store, line);
    assert_eq!(weekly_of(&store, &fx.id), Some(34.0));
}

/// A folder that changed hands takes the readings it gave its old account
/// with it; an account that is gone takes everything.
#[test]
fn a_folder_that_changed_hands_takes_its_readings_with_it() {
    let fx = Fx::new();
    let (side, side_folder) = fx.side();
    let mut store = UsageStore::with_config(UsageStoreConfig {
        home: fx.home.clone(),
        ..UsageStoreConfig::default()
    });
    store.set_accounts(
        &[fx.account.clone(), side.clone()],
        &[fx.folder.clone(), side_folder.clone()],
        now(),
    );
    feed(&mut store, fx.line(30.0, 101, now()));
    assert_eq!(weekly_of(&store, &fx.id), Some(30.0));
    // The window's folder is now the other account's.
    let mut moved_side = side.clone();
    moved_side.run_dirs = vec![side_folder.id.clone(), fx.folder.id.clone()];
    let mut emptied = fx.account.clone();
    emptied.run_dirs = Vec::new();
    store.set_accounts(
        &[emptied, moved_side],
        &[fx.folder.clone(), side_folder.clone()],
        now(),
    );
    assert!(store.usage_of(&fx.id).is_none());
    assert!(store.usage_of(&side.identity_id).is_none());
    // And an identity that is gone is pruned.
    feed(&mut store, {
        let mut line = fx.line(31.0, 101, plus(now(), 1.0));
        line.attribution = Attribution::Known(Some(side.identity_id.clone()));
        line
    });
    assert_eq!(weekly_of(&store, &side.identity_id), Some(31.0));
    store.set_accounts(
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
        now(),
    );
    assert!(store.usages().is_empty());
}

// MARK: - Full snapshots and the save deadline

#[test]
fn full_snapshots_replace_each_other_only_by_a_newer_date() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    let mut newer = AccountUsage::new(fx.id.clone(), UsageSource::Probe, ago(10));
    newer.five_hour = Some(UsageWindow::new(40.0, Some(plus(now(), 3600.0)), 18_000));
    let mut older = AccountUsage::new(fx.id.clone(), UsageSource::Cache, ago(100));
    older.five_hour = Some(UsageWindow::new(90.0, Some(plus(now(), 3600.0)), 18_000));
    assert!(store.accept_snapshot(newer, now()).is_some());
    // The history is told of it, but it doesn't replace the newer one.
    assert!(store.accept_snapshot(older, now()).is_some());
    assert_eq!(store.five_hour(&fx.id), Some(40.0));
    assert_eq!(
        store.full_snapshot(&fx.id).map(|u| u.source),
        Some(UsageSource::Probe)
    );
    // A snapshot without windows says nothing to the history.
    let empty = AccountUsage::new(fx.id.clone(), UsageSource::Probe, now());
    assert!(store.accept_snapshot(empty, now()).is_none());
}

#[test]
fn saves_are_due_two_seconds_after_a_change_and_thirty_after_a_status_line() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(false);
    store.set_accounts(
        std::slice::from_ref(&fx.account),
        std::slice::from_ref(&fx.folder),
        now(),
    );
    // `store` already scheduled one for the registry change; start afresh.
    store.save_now();
    assert_eq!(store.save_due_at(), None);

    feed(&mut store, fx.line(30.0, 101, now()).started(ago(60)));
    assert_eq!(
        store.save_due_at(),
        Some(now() + schedule::STATUS_LINE_SAVE_DELAY)
    );
    // A snapshot's 2 s doesn't wait behind the status line's 30 s.
    let mut snapshot = AccountUsage::new(fx.id.clone(), UsageSource::Probe, now());
    snapshot.five_hour = Some(UsageWindow::new(40.0, Some(plus(now(), 3600.0)), 18_000));
    store.accept_snapshot(snapshot, now());
    assert_eq!(
        store.save_due_at(),
        Some(now() + schedule::STATE_SAVE_DELAY)
    );
    // ...and a later 30 s doesn't push the 2 s back.
    feed(
        &mut store,
        fx.line(31.0, 101, plus(now(), 1.0)).started(ago(60)),
    );
    assert_eq!(
        store.save_due_at(),
        Some(now() + schedule::STATE_SAVE_DELAY)
    );

    assert!(store.take_state_to_save(plus(now(), 1.0)).is_none());
    let file = store
        .take_state_to_save(plus(now(), 2.0))
        .expect("due after 2 s");
    assert_eq!(store.save_due_at(), None);
    let account = &file.accounts[fx.id.as_str()];
    assert!(account.last_full_reading.is_some());
    // The status line is kept with its process's pid and start time.
    let lines = account.status_lines.as_ref().expect("a record per process");
    assert_eq!(lines[0].key, "pid:101@1799999940");
    // A stopped store schedules nothing.
    store.stop();
    store.accept_snapshot(
        AccountUsage::new(fx.id.clone(), UsageSource::Probe, plus(now(), 9.0)),
        plus(now(), 9.0),
    );
    assert_eq!(store.save_due_at(), None);
}

// MARK: - Caches (PP_UsageTests)

fn cached_read(folder: &RunFolder, uuid: &str, fetched_at: i64, five: f64) -> ClaudeJsonRead {
    let mut usage = AccountUsage::new(
        IdentityId::from(format!("uuid:{uuid}").as_str()),
        UsageSource::Cache,
        t(fetched_at),
    );
    usage.five_hour = Some(UsageWindow::new(
        five,
        Some(t(4_070_908_800)),
        UsageWindow::SESSION_DURATION_S,
    ));
    ClaudeJsonRead {
        folder: folder.id.clone(),
        identity: Some(login(None, Some(uuid), None)),
        cached_usage: Some(usage),
        stamp: None,
        error: None,
    }
}

#[test]
fn the_freshest_matching_cache_of_any_folder_wins() {
    let home = join(&fake_drive(), &["h"]);
    let main = run_folder(&home.join(".claude"), None);
    let store_dir = store_folder(&home.join(".claude-u"));
    let other = run_folder(&home.join(".claude-other"), None);
    let mut account = account("uuid:U-1", None, &[&main], &[&store_dir]);
    account.identity_id = IdentityId::from("uuid:u-1");
    let store = UsageStore::new();
    let reads = [
        cached_read(&main, "u-1", 1_000_000, 1.0),
        cached_read(&store_dir, "u-1", 3_000_000, 3.0), // the store's is newer
        cached_read(&other, "u-1", 9_000_000, 9.0),     // not this identity's folder
    ];
    let freshest = store
        .freshest_cached_usage(&account, &reads)
        .expect("a cache");
    assert_eq!(freshest.updated_at, t(3_000_000));
    assert_eq!(freshest.account_id, account.identity_id);
    assert_eq!(freshest.source, UsageSource::Cache);
    // The identity's UUID matches whatever its case.
    let mut upper = account.clone();
    upper.identity_id = IdentityId::from("uuid:U-1");
    assert!(store.freshest_cached_usage(&upper, &reads).is_some());
    // Someone else's snapshot left in a folder (a mirrored default) is ignored.
    let mut only_main = account.clone();
    only_main.run_dirs = vec![main.id.clone()];
    only_main.store_dirs = Vec::new();
    assert!(store
        .freshest_cached_usage(&only_main, &[cached_read(&main, "u-2", 5_000_000, 5.0)])
        .is_none());
}

#[test]
fn a_login_in_two_organizations_reads_only_the_folders_of_its_own() {
    let home = join(&fake_drive(), &["h"]);
    let first = run_folder(&home.join(".claude-a"), None);
    let second = run_folder(&home.join(".claude-b"), None);
    let mut account = account("uuid:u-1/org-a", None, &[&first, &second], &[]);
    account.identity_id = IdentityId::from("uuid:u-1/org-a");
    let mut a = cached_read(&first, "u-1", 1_000, 1.0);
    a.identity = Some(login(None, Some("u-1"), Some("ORG-A")));
    let mut b = cached_read(&second, "u-1", 2_000, 2.0);
    b.identity = Some(login(None, Some("u-1"), Some("org-b")));
    let store = UsageStore::new();
    let freshest = store
        .freshest_cached_usage(&account, &[a, b])
        .expect("org a's");
    assert_eq!(freshest.updated_at, t(1_000));
}

/// Of the account's folders' caches the newest wins, the store's included;
/// the poll also tells the signed-in state and the organization.
#[test]
fn cached_usage_merges_across_the_accounts_folders() {
    let home = join(&fake_drive(), &["h"]);
    let main = run_folder(&home.join(".claude"), None);
    let paras_store = store_folder(&home.join(".claude-paras"));
    let paras_window = run_folder(&home.join(".claude-windows").join("801f9dd51396"), None);
    let biios_store = store_folder(&home.join(".claude-biios"));
    let biios_window = run_folder(&home.join(".claude-windows").join("1bf3e8f92b11"), None);
    let paras = account(
        "uuid:paras",
        Some("paras@x.dev"),
        &[&main, &paras_window],
        &[&paras_store],
    );
    let mut biios = account(
        "uuid:biios",
        Some("biios@x.dev"),
        &[&biios_window],
        &[&biios_store],
    );
    biios.organization_uuid = Some("org-b".into());
    let mut store = UsageStore::with_config(UsageStoreConfig {
        home: home.clone(),
        ..UsageStoreConfig::default()
    });
    store.set_accounts(
        &[paras.clone(), biios.clone()],
        &[
            main.clone(),
            paras_store.clone(),
            paras_window.clone(),
            biios_store.clone(),
            biios_window.clone(),
        ],
        now(),
    );
    let seen = store.apply_cache_reads(
        &[
            cached_read(&main, "paras", 1_790_000_000, 10.0),
            cached_read(&paras_store, "paras", 1_790_000_100, 20.0),
            cached_read(&paras_window, "paras", 1_790_000_200, 30.0),
            cached_read(&biios_store, "biios", 1_790_000_050, 40.0),
            cached_read(&biios_window, "biios", 1_790_000_300, 50.0),
        ],
        now(),
    );
    // Paras: ~/.claude 1_790_000_000, store ..100, window ..200.
    assert_eq!(
        store.usage_of(&paras.identity_id).map(|u| u.updated_at),
        Some(t(1_790_000_200))
    );
    // Biios: store ..050, window ..300.
    assert_eq!(
        store.usage_of(&biios.identity_id).map(|u| u.updated_at),
        Some(t(1_790_000_300))
    );
    assert_eq!(store.usages().len(), 2);
    assert_eq!(store.organization_of(&biios.identity_id), Some("org-b"));
    assert_eq!(store.organization_of(&paras.identity_id), None);
    // The history hears of each account's freshest cached snapshot, as a
    // `.claude.json` reading.
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|o| o.source == UsageSource::Cache));
}

/// An account only a store holds shows its cached usage, passively, with its
/// date (the probe half checks that it is not probed).
#[test]
fn an_account_only_a_store_holds_shows_its_cached_usage() {
    let home = join(&fake_drive(), &["h"]);
    let biios_store = store_folder(&home.join(".claude-biios"));
    let biios = account("uuid:biios", Some("biios@x.dev"), &[], &[&biios_store]);
    let mut store = UsageStore::new();
    store.set_accounts(
        std::slice::from_ref(&biios),
        std::slice::from_ref(&biios_store),
        now(),
    );
    store.apply_cache_reads(
        &[cached_read(&biios_store, "biios", 1_790_000_050, 12.0)],
        now(),
    );
    let usage = store
        .usage_of(&biios.identity_id)
        .expect("a passive reading");
    assert_eq!(usage.source, UsageSource::Cache);
    assert_eq!(usage.updated_at, t(1_790_000_050));
}

/// A status line from a window counts for the account the window runs.
#[test]
fn status_lines_count_for_their_windows_account() {
    let home = join(&fake_drive(), &["h"]);
    let main = run_folder(&home.join(".claude"), None);
    let biios_window = run_folder(&home.join(".claude-windows").join("1bf3e8f92b11"), None);
    let biios = account("uuid:biios", Some("biios@x.dev"), &[&biios_window], &[]);
    let paras = account("uuid:paras", Some("paras@x.dev"), &[&main], &[]);
    let mut store = UsageStore::with_config(UsageStoreConfig {
        home: home.clone(),
        ..UsageStoreConfig::default()
    });
    store.set_accounts(
        &[paras.clone(), biios.clone()],
        &[main.clone(), biios_window.clone()],
        now(),
    );
    let window = biios_window.config_dir.to_string_lossy().into_owned();
    let line = L {
        five: None,
        weekly: None,
        process: 4242,
        session: "s".into(),
        at: now(),
        start: None,
        trusted: false,
        folder: window.clone(),
        // What the hub's registry says of that folder: the window's account.
        attribution: Attribution::Known(None),
    }
    .five(UsageWindow::new(
        77.0,
        Some(plus(now(), 3600.0)),
        UsageWindow::SESSION_DURATION_S,
    ));
    feed(&mut store, line);
    assert_eq!(store.five_hour(&biios.identity_id), Some(77.0));
    assert!(store.usage_of(&IdentityId::from(window.as_str())).is_none());
    assert!(store.usage_of(&paras.identity_id).is_none());
}

// MARK: - Claude Desktop

#[test]
fn desktop_is_read_on_its_cadence_and_only_for_signed_in_visible_accounts() {
    let fx = Fx::new();
    let mut store = fx.store_desktop(true);
    // Not signed in as far as the store has looked: nothing is due.
    assert!(store.desktop_due(now(), None, false).is_empty());
    store.apply_cache_reads(&[fx.read(None, now())], now());
    assert_eq!(store.desktop_due(now(), None, false).len(), 1);
    // Marked polled: not again within the minute...
    assert!(store.desktop_due(plus(now(), 59.0), None, false).is_empty());
    assert_eq!(store.desktop_due(plus(now(), 60.0), None, false).len(), 1);
    // ...a miss slows it to 5 minutes...
    let miss = DesktopReading::NotFound {
        format: agentnotch_engine::model::DesktopCacheFormat::Simple,
    };
    assert!(store
        .accept_desktop(&fx.id, &miss, plus(now(), 60.0))
        .is_none());
    assert!(store
        .desktop_due(plus(now(), 300.0), None, false)
        .is_empty());
    assert_eq!(store.desktop_due(plus(now(), 360.0), None, false).len(), 1);
    // ...a forced read still leaves Desktop alone for 5 s...
    assert!(store.desktop_due(plus(now(), 362.0), None, true).is_empty());
    assert_eq!(store.desktop_due(plus(now(), 365.0), None, true).len(), 1);
    // ...and `only` and a switched-off ring both exclude.
    let nobody: BTreeSet<IdentityId> = BTreeSet::new();
    assert!(store
        .desktop_due(plus(now(), 900.0), Some(&nobody), false)
        .is_empty());
    store.set_paused([fx.id.clone()].into());
    assert!(store
        .desktop_due(plus(now(), 900.0), None, false)
        .is_empty());
    store.set_paused(BTreeSet::new());
    // The setting off: never.
    store.set_reads_desktop(false);
    assert!(store
        .desktop_due(plus(now(), 900.0), None, false)
        .is_empty());
}

// MARK: - Ring reading (A3_RingReadingTests)

#[test]
fn the_five_statuses() {
    let fx = Fx::new();
    let mut full = AccountUsage::new(fx.id.clone(), UsageSource::Probe, now());
    full.five_hour = Some(UsageWindow::new(
        40.0,
        Some(plus(now(), 3.0 * 3600.0)),
        18_000,
    ));
    full.seven_day = Some(fx.week(10.0));
    let status = |store: &UsageStore| match store.ring_reading(&fx.id, now()) {
        RingReading::Reading { status, .. } => format!("reading:{status:?}"),
        RingReading::Waiting => "waiting".into(),
        RingReading::SignInNeeded => "sign_in".into(),
        RingReading::Unavailable(text) => format!("unavailable:{text}"),
        RingReading::Failed(text) => format!("failed:{text}"),
    };
    // Windows win over a failed check.
    let mut store = fx.store_desktop(false);
    store.accept_snapshot(full.clone(), now());
    store.set_fetch_state(&fx.id, UsageFetchState::Failed("x".into()));
    assert_eq!(status(&store), "reading:Ok");
    // Nothing yet: waiting for the first reading, whatever the check is doing.
    let mut store = fx.store_desktop(false);
    assert_eq!(status(&store), "waiting");
    store.set_fetch_state(&fx.id, UsageFetchState::Fetching);
    assert_eq!(status(&store), "waiting");
    store.set_fetch_state(&fx.id, UsageFetchState::Idle);
    assert_eq!(status(&store), "waiting");
    // Signed out wins, even over old windows (they belong to the last login).
    let mut signed_out = fx.account.clone();
    signed_out.is_signed_in = false;
    store.set_accounts(&[signed_out], std::slice::from_ref(&fx.folder), now());
    store.apply_cache_reads(&[], now());
    assert_eq!(status(&store), "sign_in");
    store.accept_snapshot(full.clone(), now());
    assert_eq!(status(&store), "sign_in");
    // Unavailable and failed with no windows.
    let mut store = fx.store_desktop(false);
    store.set_fetch_state(
        &fx.id,
        UsageFetchState::Unavailable("Usage probes are off".into()),
    );
    assert_eq!(status(&store), "unavailable:Usage probes are off");
    store.set_fetch_state(
        &fx.id,
        UsageFetchState::Failed("Claude Code not found".into()),
    );
    assert_eq!(status(&store), "failed:Claude Code not found");
    // A fresh store knows no one.
    let store = UsageStore::new();
    assert!(matches!(
        store.ring_reading(&fx.id, now()),
        RingReading::Waiting
    ));
}

#[test]
fn a_reading_goes_stale_after_the_probe_setting_allows() {
    let fx = Fx::new();
    let mut usage = AccountUsage::new(fx.id.clone(), UsageSource::Probe, ago(20 * 60));
    usage.five_hour = Some(UsageWindow::new(
        40.0,
        Some(plus(now(), 3.0 * 3600.0)),
        18_000,
    ));
    let mut store = fx.store_desktop(false);
    store.accept_snapshot(usage, now());
    let stale = |store: &UsageStore| match store.ring_reading(&fx.id, now()) {
        RingReading::Reading {
            status,
            stale_after,
            ..
        } => (status, stale_after),
        other => panic!("{other:?}"),
    };
    // At the default 5-minute setting 20 minutes is stale (the threshold is 15).
    let (status, stale_after) = stale(&store);
    assert_eq!(status, RingStatus::Stale);
    assert_eq!(stale_after, ago(5 * 60));
    // At the 30-minute setting (45 minutes) it is not yet.
    store.set_probe_interval_minutes(30);
    let (status, stale_after) = stale(&store);
    assert_eq!(status, RingStatus::Ok);
    assert_eq!(stale_after, ago(20 * 60) + Duration::from_secs(45 * 60));
    // Probes off: an hour.
    store.set_probe_interval_minutes(0);
    assert_eq!(
        store.stale_threshold(),
        AccountUsage::STALE_AFTER_WITHOUT_PROBES
    );
}
