//! Usage readings over time, with where they came from, until the website
//! has them (the Mac's `UsageHistoryRecorderTests`).

mod cloud_support;

use agentnotch_engine::cloud::contract::{SyncWindow, UsageSourceName};
use agentnotch_engine::cloud::files::StateFile;
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::recorder::{
    self, observation_windows, RecordedUsageReading, UsageHistoryRecorder, CAPACITY,
};
use agentnotch_engine::model::{AccountUsage, ExtraUsage, IdentityId, UsageSource, UsageWindow};
use agentnotch_engine::platform::SecureFiles;
use agentnotch_engine::runtime_types::UsageObservation;
use agentnotch_engine::testkit::StdSecureFiles;
use cloud_support::CloudFixture;
use std::sync::Arc;
use std::time::Duration;

fn window(id: &str, utilization: f64) -> SyncWindow {
    SyncWindow {
        id: id.into(),
        utilization,
        resets_at: None,
    }
}

fn reading(
    source: UsageSourceName,
    seconds: f64,
    session: f64,
    weekly: Option<f64>,
) -> RecordedUsageReading {
    let mut windows = vec![window("session", session)];
    if let Some(weekly) = weekly {
        windows.push(window("weekly_all", weekly));
    }
    RecordedUsageReading {
        account_key: CloudFixture::ACCOUNT_KEY.into(),
        identity_id: IdentityId::new(CloudFixture::IDENTITY_ID),
        source,
        observed_at: CloudFixture::at(seconds),
        windows,
    }
}

fn probe(seconds: f64, session: f64) -> RecordedUsageReading {
    reading(UsageSourceName::Probe, seconds, session, None)
}

fn memory() -> UsageHistoryRecorder {
    UsageHistoryRecorder::new(StateFile::memory())
}

#[test]
fn a_reading_is_kept_once_and_its_moments_windows_merge() {
    let recorder = memory();
    assert!(recorder.record(probe(0.0, 10.0)));
    assert!(!recorder.record(probe(0.0, 10.0)));
    // The same moment with another window: merged into it.
    assert!(recorder.record(reading(UsageSourceName::Probe, 0.0, 10.0, Some(30.0))));
    assert_eq!(recorder.pending_count(), 1);
    let ids: Vec<String> = recorder.pending(usize::MAX)[0]
        .windows
        .iter()
        .map(|w| w.id.clone())
        .collect();
    assert_eq!(ids, ["session", "weekly_all"]);
    // Older than the last taken: dropped.
    assert!(recorder.record(probe(900.0, 12.0)));
    assert!(!recorder.record(probe(600.0, 11.0)));
    assert_eq!(recorder.pending_count(), 2);
    let mut empty = probe(2000.0, 1.0);
    empty.windows.clear();
    assert!(!recorder.record(empty));
    // The same moment to the millisecond is the same reading.
    let mut close = reading(UsageSourceName::Probe, 900.0, 12.0, Some(40.0));
    close.observed_at += Duration::from_micros(200);
    assert!(recorder.record(close));
    assert_eq!(recorder.pending_count(), 2);
    assert_eq!(recorder.pending(usize::MAX)[1].windows.len(), 2);
}

#[test]
fn repeats_within_ten_minutes_are_dropped() {
    let recorder = memory();
    assert!(recorder.record(probe(0.0, 10.0)));
    assert!(!recorder.record(probe(300.0, 10.0)));
    assert!(recorder.record(probe(301.0, 11.0)));
    assert!(recorder.record(probe(301.0 + 600.0, 11.0)));
    assert_eq!(recorder.pending_count(), 3);
    assert_eq!(recorder::REPEAT_WINDOW, Duration::from_secs(600));
}

#[test]
fn claude_desktop_and_claude_json_are_separate_sources() {
    let recorder = memory();
    assert!(recorder.record(reading(UsageSourceName::Desktop, 0.0, 10.0, None)));
    assert!(recorder.record(reading(UsageSourceName::ClaudeJson, 0.0, 10.0, None)));
    assert!(recorder.record(reading(UsageSourceName::StatusLine, 0.0, 10.0, None)));
    let sources: Vec<UsageSourceName> = recorder
        .pending(usize::MAX)
        .iter()
        .map(|r| r.source)
        .collect();
    assert_eq!(
        sources,
        [
            UsageSourceName::Desktop,
            UsageSourceName::ClaudeJson,
            UsageSourceName::StatusLine
        ]
    );
    let contract: Vec<&str> = recorder
        .pending(usize::MAX)
        .iter()
        .map(|r| r.contract().source.as_str())
        .collect();
    assert_eq!(contract, ["desktop", "claudeJson", "statusLine"]);
}

#[test]
fn the_outbox_is_bounded_and_empties_as_the_website_takes_it() {
    let root = tempfile::tempdir().unwrap();
    let support = root.path().join("support");
    let files: Arc<dyn SecureFiles> = Arc::new(StdSecureFiles);
    let recorder = UsageHistoryRecorder::in_support(&support, files.clone(), true);
    for index in 0..(CAPACITY + 5) {
        recorder.record(probe(index as f64, index as f64));
    }
    assert_eq!(recorder.pending_count(), CAPACITY);
    assert_eq!(recorder.pending(usize::MAX)[0].windows[0].utilization, 5.0);
    recorder.mark_sent(&recorder.pending(100));
    assert_eq!(recorder.pending_count(), CAPACITY - 100);
    recorder.discard(|r| r.windows[0].utilization < 1000.0);
    recorder.save_now();
    let file = support.join(recorder::FILE_NAME);
    if cfg!(unix) {
        // Plain std can't read an ACL; agentnotch-win's files prove it there.
        assert!(StdSecureFiles.is_private(&file).unwrap());
    }
    assert_eq!(
        UsageHistoryRecorder::in_support(&support, files.clone(), true).pending_count(),
        CAPACITY - 995
    );
    recorder.clear();
    recorder.flush();
    recorder.save_now();
    assert_eq!(
        UsageHistoryRecorder::in_support(&support, files.clone(), true).pending_count(),
        0
    );
    // Not persisted (sealed): nothing on disk.
    let sealed_root = root.path().join("sealed");
    let sealed = UsageHistoryRecorder::in_support(&sealed_root, files, false);
    assert!(sealed.record(probe(0.0, 1.0)));
    sealed.save_now();
    assert!(!sealed_root.exists());
}

#[test]
fn a_snapshots_windows_as_recorded() {
    let weekly = |utilization: f64| UsageWindow {
        utilization,
        resets_at: None,
        duration_s: UsageWindow::WEEKLY_DURATION_S,
    };
    let usage = AccountUsage {
        account_id: IdentityId::new(CloudFixture::IDENTITY_ID),
        five_hour: Some(UsageWindow {
            utilization: 40.0,
            resets_at: Some(CloudFixture::base()),
            duration_s: UsageWindow::SESSION_DURATION_S,
        }),
        seven_day: Some(weekly(60.0)),
        scoped: vec![
            ("Opus".into(), weekly(5.0)),
            ("Very Long Model ".repeat(8), weekly(1.0)),
            // A second name for the same family: kept once.
            ("opus".into(), weekly(7.0)),
        ],
        extra_usage: Some(ExtraUsage {
            is_enabled: true,
            monthly_limit: Some(5000.0),
            used_credits: Some(1250.0),
            utilization: None,
            currency: Some("USD".into()),
        }),
        subscription_type: None,
        source: UsageSource::Cache,
        updated_at: CloudFixture::base(),
        taken_after: None,
    };
    let windows = observation_windows(&usage);
    let ids: Vec<&str> = windows.iter().map(|w| w.0.as_str()).collect();
    assert_eq!(ids[..3], ["session", "weekly_all", "weekly_opus"]);
    assert_eq!(windows.len(), 5);
    assert!(windows.iter().all(|w| keys::is_window_id(&w.0)));
    assert_eq!(windows[0].2, Some(CloudFixture::base()));
    assert_eq!(
        windows.last(),
        Some(&("extra_usage".to_owned(), 25.0, None))
    );

    // Not enabled, or nothing to go on: no extra_usage; below 0 reads 0;
    // not a number: left out.
    let mut other = usage.clone();
    other.extra_usage = Some(ExtraUsage {
        is_enabled: false,
        monthly_limit: Some(10.0),
        used_credits: Some(5.0),
        utilization: Some(50.0),
        currency: None,
    });
    other.five_hour.as_mut().unwrap().utilization = -3.0;
    other.seven_day.as_mut().unwrap().utilization = f64::NAN;
    other.scoped.clear();
    assert_eq!(
        observation_windows(&other),
        [("session".to_owned(), 0.0, Some(CloudFixture::base()))]
    );
}

/// Where a reading came from survives into the outbox: Claude Code's
/// `.claude.json` cache and Claude Desktop's cache are told apart. (The
/// usage store that dates and sends these observations is WP4's; its
/// half of the Mac's test lives with it.)
#[test]
fn the_usage_store_tells_claude_desktop_from_claude_json() {
    let observation = |source: UsageSource, utilization: f64| UsageObservation {
        identity: IdentityId::new(CloudFixture::IDENTITY_ID),
        source,
        observed_at: CloudFixture::at(-30.0),
        windows: vec![
            (
                "session".into(),
                utilization,
                Some(CloudFixture::at(3.0 * 3600.0)),
            ),
            // Shortened to what the website takes; one it would refuse is
            // left out, and so is a reading that isn't a number.
            (format!("weekly_{}", "a".repeat(80)), 1.0, None),
            ("Weird Id".into(), 2.0, None),
            ("weekly_all".into(), f64::INFINITY, None),
        ],
    };
    let cached = RecordedUsageReading::from_observation(
        &observation(UsageSource::Cache, 30.0),
        CloudFixture::ACCOUNT_KEY,
    );
    assert_eq!(cached.source, UsageSourceName::ClaudeJson);
    assert_eq!(cached.identity_id.as_str(), CloudFixture::IDENTITY_ID);
    assert_eq!(cached.account_key, CloudFixture::ACCOUNT_KEY);
    assert_eq!(cached.observed_at, CloudFixture::at(-30.0));
    assert_eq!(cached.windows.len(), 2);
    assert_eq!(cached.windows[0].utilization, 30.0);
    assert_eq!(cached.windows[1].id, format!("weekly_{}", "a".repeat(57)));
    let desktop = RecordedUsageReading::from_observation(
        &observation(UsageSource::Desktop, 35.0),
        CloudFixture::ACCOUNT_KEY,
    );
    assert_eq!(desktop.source, UsageSourceName::Desktop);
    assert_eq!(desktop.windows[0].utilization, 35.0);
    for (source, name) in [
        (UsageSource::Probe, "probe"),
        (UsageSource::StatusLine, "statusLine"),
    ] {
        let r = RecordedUsageReading::from_observation(&observation(source, 1.0), "k");
        assert_eq!(r.contract().source.as_str(), name);
    }
    // Two separate streams: both kept.
    let recorder = memory();
    assert!(recorder.record(cached));
    assert!(recorder.record(desktop));
    assert_eq!(recorder.pending_count(), 2);
}
