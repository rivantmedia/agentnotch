//! `usage::merge`: the Mac's UsageStoreLogicTests (UsageProbeTests.swift), the
//! rules that weigh a full snapshot and every Claude Code process's status line
//! against each other. `cachePollKeepsTheProbesVerdict`, `backoffDoublesAndCaps`
//! and `probeIntervalHasAFloor` of that suite are in `usage_schedule.rs`, with
//! the scheduling code they test. Doubles are compared exactly, as the Swift
//! tests compare them.

use agentnotch_engine::core::time;
use agentnotch_engine::model::{
    AccountUsage, ExtraUsage, IdentityId, StatusLineRecord, UsageReading, UsageSource, UsageWindow,
};
use agentnotch_engine::usage::merge::{
    advance, combined_status, is_repeat, is_same_window, merge, most_current, readings,
    status_line_key, supersedes, winning_status, StatusWindow, RESET_DROP_MINIMUM,
};
use agentnotch_engine::usage::parser::ParsedUsage;
use agentnotch_engine::usage::probe::interpret_probe_answer;
use std::collections::BTreeMap;
use std::time::SystemTime;

const SESSION: u64 = UsageWindow::SESSION_DURATION_S;
const WEEK: u64 = UsageWindow::WEEKLY_DURATION_S;

fn now() -> SystemTime {
    time::from_secs_f64(1_790_000_000.0).unwrap()
}

/// `now` plus `seconds`.
fn after(seconds: f64) -> SystemTime {
    time::from_secs_f64(1_790_000_000.0 + seconds).unwrap()
}

fn id(text: &str) -> IdentityId {
    IdentityId::from(text)
}

/// A session window that resets an hour from `now`.
fn window(utilization: f64) -> UsageWindow {
    window_of(utilization, SESSION)
}

fn window_of(utilization: f64, duration_s: u64) -> UsageWindow {
    UsageWindow::new(utilization, Some(after(3600.0)), duration_s)
}

/// A weekly window that resets `reset_in` from `now`.
fn week_in(utilization: f64, reset_in: f64) -> UsageWindow {
    UsageWindow::new(utilization, Some(after(reset_in)), WEEK)
}

fn week(utilization: f64) -> UsageWindow {
    week_in(utilization, 3.0 * 86400.0)
}

/// A status line reading that arrived `at` seconds from `now`, taken after
/// `taken_after` seconds from `now` when that is known.
fn reading(window: UsageWindow, at: f64, taken_after: Option<f64>) -> UsageReading {
    UsageReading::new(window, after(at), taken_after.map(after))
}

fn snapshot(source: UsageSource) -> AccountUsage {
    AccountUsage::new(id("a"), source, now())
}

#[test]
fn status_line_alone_makes_usage() {
    let merged = merge(&id("a"), None, &[reading(window(20.0), 0.0, None)], &[])
        .expect("a status line is usage");
    assert_eq!(merged.five_hour, Some(window(20.0)));
    assert_eq!(merged.seven_day, None);
    assert_eq!(merged.source, UsageSource::StatusLine);
    assert_eq!(merged.updated_at, now());
    assert_eq!(merge(&id("a"), None, &[], &[]), None);
}

#[test]
fn newest_reading_wins_per_window() {
    let mut full = snapshot(UsageSource::Probe);
    full.five_hour = Some(window(10.0));
    full.seven_day = Some(window_of(30.0, WEEK));
    full.scoped = vec![("Opus".into(), window_of(5.0, WEEK))];
    full.extra_usage = Some(ExtraUsage {
        is_enabled: true,
        monthly_limit: Some(10.0),
        used_credits: Some(1.0),
        utilization: Some(10.0),
        currency: Some("USD".into()),
    });
    full.subscription_type = Some("max".into());
    // Newer status line 5h, older status line 7d.
    let merged = merge(
        &id("a"),
        Some(&full),
        &[reading(window(25.0), 60.0, None)],
        &[reading(window_of(20.0, WEEK), -60.0, None)],
    )
    .unwrap();
    assert_eq!(merged.five_hour, Some(window(25.0)));
    assert_eq!(merged.seven_day, Some(window_of(30.0, WEEK)));
    assert_eq!(merged.scoped, full.scoped);
    assert_eq!(merged.extra_usage, full.extra_usage);
    assert_eq!(merged.subscription_type.as_deref(), Some("max"));
    assert_eq!(merged.source, UsageSource::StatusLine);
    assert_eq!(merged.updated_at, after(60.0));
}

#[test]
fn status_line_fills_a_window_the_snapshot_lacks() {
    let mut full = snapshot(UsageSource::Cache);
    full.seven_day = Some(window_of(30.0, WEEK));
    let merged = merge(
        &id("a"),
        Some(&full),
        &[reading(window(7.0), -600.0, None)],
        &[],
    )
    .unwrap();
    assert_eq!(merged.five_hour, Some(window(7.0)));
    assert_eq!(merged.source, UsageSource::Cache);
    assert_eq!(merged.updated_at, now());
}

// A status line re-run repeats the session's last API response, however old:
// it must not beat a fresher probe just by arriving later.
#[test]
fn stale_status_line_does_not_beat_a_fresher_snapshot() {
    let mut full = snapshot(UsageSource::Probe);
    full.five_hour = Some(window(55.0));
    full.seven_day = Some(window_of(30.0, WEEK));
    let merged = merge(
        &id("a"),
        Some(&full),
        &[reading(window(40.0), 60.0, None)],
        &[reading(window_of(28.0, WEEK), 60.0, None)],
    )
    .unwrap();
    assert_eq!(merged.five_hour, Some(window(55.0)));
    assert_eq!(merged.seven_day, Some(window_of(30.0, WEEK)));
    assert_eq!(merged.source, UsageSource::Probe);
    assert_eq!(merged.updated_at, now());
    // Even one that changed, when the change may predate the probe.
    let straddling = merge(
        &id("a"),
        Some(&full),
        &[reading(window(40.0), 60.0, Some(-30.0))],
        &[],
    )
    .unwrap();
    assert_eq!(straddling.five_hour, Some(window(55.0)));
}

#[test]
fn a_newer_window_wins_even_when_lower() {
    let expired = UsageWindow::new(80.0, Some(after(-600.0)), SESSION);
    let fresh = UsageWindow::new(5.0, Some(after(4.0 * 3600.0)), SESSION);
    let mut full = snapshot(UsageSource::Cache);
    full.five_hour = Some(expired.clone());
    let merged = merge(
        &id("a"),
        Some(&full),
        &[reading(fresh.clone(), -60.0, None)],
        &[],
    )
    .unwrap();
    assert_eq!(merged.five_hour, Some(fresh.clone()));
    // ... and an older window never beats a newer one, whenever it arrived.
    let mut reversed = snapshot(UsageSource::Probe);
    reversed.five_hour = Some(fresh.clone());
    let merged = merge(
        &id("a"),
        Some(&reversed),
        &[reading(expired, 60.0, None)],
        &[],
    )
    .unwrap();
    assert_eq!(merged.five_hour, Some(fresh));
}

#[test]
fn same_window_tolerates_rounding() {
    // The endpoint's ISO reset has fractional seconds; the status line's is whole epoch seconds.
    let endpoint = UsageWindow::new(9.0, time::from_secs_f64(1_790_254_200.257626), SESSION);
    let status_line = UsageWindow::new(9.4, time::from_secs_f64(1_790_254_200.0), SESSION);
    let next_window = UsageWindow::new(
        1.0,
        time::from_secs_f64(1_790_254_200.0 + 5.0 * 3600.0),
        SESSION,
    );
    assert!(is_same_window(&endpoint, &status_line));
    assert!(!is_same_window(&endpoint, &next_window));
    assert!(!is_same_window(
        &endpoint,
        &UsageWindow::new(1.0, None, SESSION)
    ));
    // A later reading a rounding step lower is no reset: the higher stays.
    let mut full = snapshot(UsageSource::Probe);
    full.five_hour = Some(endpoint);
    let merged = merge(
        &id("a"),
        Some(&full),
        &[reading(status_line.clone(), -60.0, None)],
        &[],
    )
    .unwrap();
    assert_eq!(merged.five_hour, Some(status_line));
}

#[test]
fn repeated_readings_keep_their_time() {
    let first = advance(None, Some(&window(20.0)), None, now(), None);
    assert_eq!(first.five_hour, Some(reading(window(20.0), 0.0, None)));
    // The same numbers again (an idle session re-rendering): not news.
    let repeated = advance(Some(&first), Some(&window(20.0)), None, after(300.0), None);
    assert_eq!(repeated.five_hour, first.five_hour);
    assert_eq!(repeated.last_report_at, after(300.0));
    // A small step back (a response that started before the last one ended
    // after it): the higher reading stays, with its time.
    let step_back = advance(
        Some(&repeated),
        Some(&window(20.0 - RESET_DROP_MINIMUM)),
        None,
        after(350.0),
        None,
    );
    assert_eq!(step_back.five_hour, first.five_hour);
    assert_eq!(step_back.last_report_at, after(350.0));
    // Different numbers, even lower by a reset: the process's own newer
    // response, taken after the report before.
    let lower = advance(
        Some(&step_back),
        Some(&window(12.0)),
        Some(&week(40.0)),
        after(400.0),
        None,
    );
    assert_eq!(
        lower.five_hour,
        Some(reading(window(12.0), 400.0, Some(350.0)))
    );
    assert_eq!(
        lower.seven_day,
        Some(reading(week(40.0), 400.0, Some(350.0)))
    );
    // A reset time that lost its fraction of a second (a relaunch) is the same.
    let mut rounded = window(12.0);
    let reset = time::to_secs_f64(rounded.resets_at.unwrap());
    rounded.resets_at = time::from_secs_f64(reset.floor() + 0.4);
    assert_eq!(
        advance(
            Some(&lower),
            Some(&rounded),
            Some(&week(40.0)),
            after(450.0),
            None
        )
        .five_hour,
        lower.five_hour
    );
    assert!(!is_repeat(&window(15.0), &window(16.0)));
    // A window the line leaves out keeps what it had.
    assert_eq!(
        advance(Some(&lower), None, Some(&week(40.0)), after(500.0), None).five_hour,
        lower.five_hour
    );
}

/// Claude Code has no rate limits before a process's first API response, so a
/// process's first report is newer than the process.
#[test]
fn a_first_report_is_newer_than_its_process() {
    let first = advance(None, None, Some(&week(3.0)), now(), Some(after(-60.0)));
    assert_eq!(first.seven_day, Some(reading(week(3.0), 0.0, Some(-60.0))));
    // A start time that isn't before the report is no bound.
    let odd = advance(None, None, Some(&week(3.0)), now(), Some(now()));
    assert_eq!(odd.seven_day.unwrap().not_before, None);
}

#[test]
fn readings_in_unknown_order_keep_the_old_rules() {
    // Lower within the same window from a process whose data may be older: the higher stays.
    let kept = [
        reading(window(20.0), 0.0, None),
        reading(window(15.0), 300.0, None),
    ];
    assert_eq!(most_current(&kept).map(|r| &r.window), Some(&window(20.0)));
    // Higher: fresh.
    let higher = [
        reading(window(20.0), 0.0, None),
        reading(window(25.0), 300.0, None),
    ];
    assert_eq!(
        most_current(&higher).map(|r| &r.window),
        Some(&window(25.0))
    );
    // No reset times to compare: arrival order decides.
    let bare = |value: f64| UsageWindow::new(value, None, SESSION);
    let by_arrival = [
        reading(bare(20.0), 0.0, None),
        reading(bare(10.0), 1.0, None),
    ];
    assert_eq!(
        most_current(&by_arrival).map(|r| &r.window),
        Some(&bare(10.0))
    );
    assert_eq!(most_current(&[]), None);
}

// MARK: Early resets

/// Anthropic reset the week early and kept the reset time: the probe after it
/// reads low, a status line from before it still says high.
#[test]
fn a_later_snapshot_wins_after_an_early_reset() {
    // The reset time kept, moved a day later (inside the same-window
    // tolerance), or moved two days earlier.
    for shift in [0.0, 86400.0, -2.0 * 86400.0] {
        let reset_in = 3.0 * 86400.0 + shift;
        let mut after_reset = snapshot(UsageSource::Probe);
        after_reset.seven_day = Some(week_in(3.0, reset_in));
        let merged = merge(
            &id("a"),
            Some(&after_reset),
            &[],
            &[reading(week(62.0), -600.0, Some(-900.0))],
        )
        .unwrap();
        assert_eq!(
            merged.seven_day,
            Some(week_in(3.0, reset_in)),
            "reset moved by {shift} s"
        );
        assert_eq!(merged.source, UsageSource::Probe);
    }
}

/// ...and the other way round: the reading kept from before the reset
/// (usage-state.json, an old cache) loses to a status line taken after it.
#[test]
fn a_later_status_line_wins_over_an_older_snapshot() {
    let mut before = snapshot(UsageSource::Cache);
    before.seven_day = Some(week(62.0));
    // A process that changed after the snapshot, or started after it.
    let merged = merge(
        &id("a"),
        Some(&before),
        &[],
        &[reading(week(3.0), 120.0, Some(60.0))],
    )
    .unwrap();
    assert_eq!(merged.seven_day, Some(week(3.0)));
    assert_eq!(merged.updated_at, after(120.0));
    // One that may have been taken before it (a re-run, a straddling change) doesn't.
    let rerun = merge(
        &id("a"),
        Some(&before),
        &[],
        &[reading(week(3.0), 120.0, None)],
    )
    .unwrap();
    assert_eq!(rerun.seven_day, Some(week(62.0)));
    let straddling = merge(
        &id("a"),
        Some(&before),
        &[],
        &[reading(week(3.0), 120.0, Some(-60.0))],
    )
    .unwrap();
    assert_eq!(straddling.seven_day, Some(week(62.0)));
}

/// Two sessions: A keeps working through the reset, B went idle before it and
/// keeps re-rendering its old numbers.
#[test]
fn an_idle_sessions_old_numbers_dont_outlive_the_reset() {
    fn seven(record: &StatusLineRecord) -> UsageReading {
        record.seven_day.clone().unwrap()
    }
    let mut a = advance(None, None, Some(&week(61.0)), now(), None);
    let b = advance(None, None, Some(&week(62.0)), after(10.0), None);
    a = advance(Some(&a), None, Some(&week(61.0)), after(20.0), None);
    let shown = |a: &StatusLineRecord| {
        let processes: BTreeMap<String, StatusLineRecord> = [
            ("pid:1".to_owned(), a.clone()),
            ("pid:2".to_owned(), b.clone()),
        ]
        .into();
        let by_folder: BTreeMap<String, Vec<UsageReading>> = [(
            "/w".to_owned(),
            readings(&processes, StatusWindow::SevenDay),
        )]
        .into();
        combined_status(&by_folder, "/h/.claude", false).map(|r| r.window)
    };
    assert_eq!(shown(&a), Some(week(62.0)));
    // The reset; A's next response reads 3%.
    a = advance(Some(&a), None, Some(&week(3.0)), after(600.0), None);
    assert_eq!(shown(&a), Some(week(3.0)));
    // B re-renders the same old numbers: still 3%.
    let b_again = advance(Some(&b), None, Some(&week(62.0)), after(700.0), None);
    let folders =
        |a: &StatusLineRecord, b: &StatusLineRecord| -> BTreeMap<String, Vec<UsageReading>> {
            [
                ("/w".to_owned(), vec![seven(a)]),
                ("/v".to_owned(), vec![seven(b)]),
            ]
            .into()
        };
    assert_eq!(
        combined_status(&folders(&a, &b_again), "/h/.claude", false).map(|r| r.window),
        Some(week(3.0))
    );
    // Usage grows again after the reset.
    a = advance(Some(&a), None, Some(&week(4.0)), after(800.0), None);
    assert_eq!(
        combined_status(&folders(&a, &b_again), "/h/.claude", false).map(|r| r.window),
        Some(week(4.0))
    );
}

/// A reset that moves the reset time earlier: the reading known to be later
/// still wins, though its window resets first.
#[test]
fn a_reset_that_moves_the_reset_time_earlier_is_followed() {
    let before = reading(week_in(62.0, 5.0 * 86400.0), 0.0, Some(-60.0));
    let after_reset = reading(week_in(1.0, 2.0 * 86400.0), 600.0, Some(300.0));
    assert_eq!(
        most_current(&[before.clone(), after_reset.clone()]),
        Some(&after_reset)
    );
    assert_eq!(
        most_current(&[after_reset.clone(), before.clone()]),
        Some(&after_reset)
    );
    // Order unknown: the later reset still wins, as before.
    let unknown = reading(week_in(1.0, 2.0 * 86400.0), 600.0, None);
    assert_eq!(most_current(&[before.clone(), unknown]), Some(&before));
}

#[test]
fn supersedes_is_strict_and_never_mutual() {
    let snapshot = UsageReading::new(week(3.0), now(), Some(now()));
    // A point in time doesn't supersede itself, nor a reading from the same moment.
    assert!(!supersedes(&snapshot, &snapshot));
    assert!(!supersedes(&snapshot, &reading(week(62.0), 0.0, None)));
    assert!(supersedes(&snapshot, &reading(week(62.0), -1.0, None)));
    // The reading with the latest arrival is never out; a later one a point
    // lower is no reset, so the higher stays.
    let all = [
        snapshot,
        reading(week(62.0), -1.0, None),
        reading(week(5.0), 30.0, Some(-10.0)),
        reading(week(4.0), 60.0, Some(30.0)),
    ];
    assert_eq!(
        most_current(&all),
        Some(&reading(week(5.0), 30.0, Some(-10.0)))
    );
    for x in &all {
        for y in &all {
            if supersedes(x, y) {
                assert!(!supersedes(y, x));
            }
        }
    }
}

#[test]
fn status_line_key_is_the_process_when_known() {
    assert_eq!(status_line_key("s1", Some(4242), None), "pid:4242");
    assert_eq!(status_line_key("s1", None, None), "session:s1");
    // Windows reuses pids quickly: the start time (whole epoch seconds) tells them apart.
    assert_eq!(
        status_line_key("s1", Some(4242), Some(after(0.9))),
        "pid:4242@1790000000"
    );
}

/// A response that started before a probe and ended after it: its process
/// reports numbers a little older than the probe's, as news. Only a
/// reset-sized drop overrides the higher reading.
#[test]
fn a_small_drop_is_no_reset_even_when_later() {
    let mut probe = snapshot(UsageSource::Probe);
    probe.seven_day = Some(week(42.0));
    let seven = |value: f64| {
        merge(
            &id("a"),
            Some(&probe),
            &[],
            &[reading(week(value), 60.0, Some(30.0))],
        )
        .unwrap()
        .seven_day
    };
    assert_eq!(seven(40.0), Some(week(42.0)));
    assert_eq!(seven(42.0 - RESET_DROP_MINIMUM), Some(week(42.0)));
    assert_eq!(seven(36.0), Some(week(36.0)));
}

/// The snapshot and every process's reading are weighed together: one process
/// known to be newer than the snapshot puts it out, even when another
/// process's reading is what shows.
#[test]
fn the_snapshot_and_every_status_line_are_weighed_together() {
    let mut full = snapshot(UsageSource::Probe);
    full.seven_day = Some(week(10.0));
    let lines = [
        reading(week(5.0), 100.0, Some(-50.0)),
        reading(week(3.0), 60.0, Some(30.0)),
    ];
    let merged = merge(&id("a"), Some(&full), &[], &lines).unwrap();
    assert_eq!(merged.seven_day, Some(week(5.0)));
    let dated = UsageReading::of_snapshot(week(10.0), &full);
    assert_eq!(winning_status(&lines, Some(&dated)), Some(lines[0].clone()));
    // Neither line alone is known newer than the snapshot... than 5 is.
    assert_eq!(winning_status(&lines[..1], Some(&dated)), None);
}

/// A snapshot is known newer only than its `taken_after` (a probe's launch, a
/// margin for Claude Desktop's date), never after its `updated_at`.
#[test]
fn a_snapshot_is_dated_from_its_taken_after() {
    let mut full = snapshot(UsageSource::Probe);
    full.seven_day = Some(week(3.0));
    assert_eq!(
        UsageReading::of_snapshot(week(3.0), &full),
        UsageReading::new(week(3.0), now(), Some(now()))
    );
    full.taken_after = Some(after(-5.0));
    assert_eq!(
        UsageReading::of_snapshot(week(3.0), &full).not_before,
        Some(after(-5.0))
    );
    full.taken_after = Some(after(5.0));
    assert_eq!(
        UsageReading::of_snapshot(week(3.0), &full).not_before,
        Some(now())
    );
    // A probe's answer came after its launch.
    let parsed = ParsedUsage {
        seven_day: Some(week(3.0)),
        ..ParsedUsage::default()
    };
    let (answer, _) = interpret_probe_answer(&parsed, &id("a"), now(), None, Some(after(-4.0)));
    let answer = answer.expect("a fresh answer is a snapshot");
    assert_eq!(answer.taken_after, Some(after(-4.0)));
    // A status line that arrived while the probe ran is not known older than it.
    let during = reading(week(62.0), -2.0, Some(-30.0));
    let probe = UsageReading::of_snapshot(week(3.0), &answer);
    assert!(!supersedes(&probe, &during));
    assert!(supersedes(&probe, &reading(week(62.0), -5.0, None)));
}
