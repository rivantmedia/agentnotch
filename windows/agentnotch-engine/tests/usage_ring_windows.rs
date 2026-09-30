//! `usage::ring_windows` and the staleness / limit rules of `AccountUsage`:
//! the window- and staleness-related tests of the Mac's A3_RingReadingTests.
//! The test of the label wording lives beside the code (`ring_windows.rs`);
//! `theFiveStatuses` and the ring-reading projection belong to the usage
//! store's tests (`ring_reading`), as do `plan` and `status` of a reading.
//! Money-metered windows never block: the engine's `AccountUsage` carries extra
//! usage apart from its windows, so `limit_hit` cannot see one (that is the
//! Swift `moneyWindowsNeverBlock`, below).

use agentnotch_engine::core::time;
use agentnotch_engine::model::{
    AccountUsage, DesktopWindow, ExtraUsage, IdentityId, LimitHit, LimitWindowKind, UsageSource,
    UsageWindow,
};
use agentnotch_engine::usage::ring_windows::{
    account_usage_from_desktop, extra_usage_window, windows, Money,
};
use std::time::{Duration, SystemTime};

const WEEK: u64 = UsageWindow::WEEKLY_DURATION_S;
const SESSION: u64 = UsageWindow::SESSION_DURATION_S;

fn now() -> SystemTime {
    time::from_secs_f64(1_800_000_000.0).unwrap()
}

fn after(seconds: f64) -> SystemTime {
    let t = time::to_secs_f64(now()) + seconds;
    time::from_secs_f64(t).unwrap()
}

fn window(utilization: f64, resets_in: f64, duration_s: u64) -> UsageWindow {
    UsageWindow::new(utilization, Some(after(resets_in)), duration_s)
}

fn full_usage() -> AccountUsage {
    let mut usage = AccountUsage::new(IdentityId::from("a"), UsageSource::Probe, after(-90.0));
    usage.five_hour = Some(window(34.0, 3600.0, SESSION));
    usage.seven_day = Some(window(41.0, 3.0 * 86400.0, WEEK));
    usage.scoped = vec![
        ("Sonnet 4.5".to_owned(), window(12.0, 3.0 * 86400.0, WEEK)),
        ("Opus".to_owned(), window(25.0, 3.0 * 86400.0, WEEK)),
    ];
    usage.extra_usage = Some(ExtraUsage {
        is_enabled: true,
        monthly_limit: Some(5_000.0),
        used_credits: Some(1_240.0),
        utilization: Some(24.8),
        currency: Some("usd".to_owned()),
    });
    usage.subscription_type = Some("max".to_owned());
    usage
}

// MARK: Ids, labels, order, money

#[test]
fn ids_labels_and_order_mirror_codenotch() {
    let usage = full_usage();
    let windows = windows(&usage, now());
    let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
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
    // Codenotch labels the two standard windows itself.
    assert!(windows[0].label.is_none() && windows[1].label.is_none());
    let label = |id: &str| {
        windows
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.label.as_deref())
    };
    assert_eq!(label("weekly_opus"), Some("Opus"));
    assert_eq!(label("weekly_sonnet_4_5"), Some("Sonnet 4.5"));
    assert!((windows[0].used_fraction - 0.34).abs() < 1e-9);
    assert_eq!(windows[0].duration_s, Some(SESSION));
    assert_eq!(windows[1].resets_at, Some(after(3.0 * 86400.0)));
    assert_eq!(usage.subscription_type.as_deref(), Some("max"));
    assert_eq!(usage.updated_at, after(-90.0));
}

#[test]
fn extra_usage_is_money_in_major_units() {
    let usage = full_usage();
    let all = windows(&usage, now());
    let extra = all.iter().find(|w| w.id == "extra_usage").expect("extra");
    assert_eq!(extra.label.as_deref(), Some("Extra usage"));
    assert!((extra.used_fraction - 0.248).abs() < 1e-9);
    assert_eq!(
        extra.money,
        Some(Money {
            currency: "USD".to_owned(),
            spent: 12.40,
            remaining: 37.60
        })
    );
    // Switched off: no window.
    let mut off = full_usage();
    off.extra_usage.as_mut().unwrap().is_enabled = false;
    assert!(!windows(&off, now()).iter().any(|w| w.id == "extra_usage"));
    // Zero-decimal currencies are not divided.
    let yen = extra_usage_window(Some(&ExtraUsage {
        is_enabled: true,
        monthly_limit: Some(5000.0),
        used_credits: Some(1000.0),
        utilization: None,
        currency: Some("JPY".to_owned()),
    }))
    .expect("yen");
    assert_eq!(
        yen.money,
        Some(Money {
            currency: "JPY".to_owned(),
            spent: 1000.0,
            remaining: 4000.0
        })
    );
    assert!((yen.used_fraction - 0.2).abs() < 1e-9);
}

#[test]
fn a_reset_window_reads_zero() {
    let mut usage = full_usage();
    usage.five_hour = Some(window(88.0, -60.0, SESSION));
    let all = windows(&usage, now());
    let session = all.iter().find(|w| w.id == "session").expect("session");
    assert_eq!(session.used_fraction, 0.0);
}

// MARK: Staleness

#[test]
fn readings_go_stale_after_their_threshold() {
    let mut usage = full_usage();
    usage.updated_at = after(-20.0 * 60.0);
    let fifteen = Duration::from_secs(15 * 60);
    assert!(usage.is_stale(now(), fifteen));
    // At the 30-minute setting, 20 minutes is not stale yet.
    assert!(!usage.is_stale(now(), AccountUsage::stale_threshold(30)));
    // A minute after it was taken, a reading is fresh at the default.
    let just_taken = usage.updated_at + Duration::from_secs(60);
    assert!(!usage.is_stale(just_taken, AccountUsage::STALE_AFTER));
    // (Swift: only `.ok` readings are ever stale; a ring's status is the
    // store's business, tested with `ring_reading`.)
}

#[test]
fn a_used_up_window_never_goes_stale_before_its_reset() {
    let mut usage = full_usage();
    usage.seven_day = Some(window(100.0, 2.0 * 86400.0, WEEK));
    usage.updated_at = after(-3.0 * 3600.0);
    let fifteen = Duration::from_secs(15 * 60);
    assert!(!usage.is_stale(now(), fifteen));
    let hit = usage.limit_hit(now()).expect("a window is used up");
    assert_eq!(hit.window, LimitWindowKind::Weekly);
    // Once it has reset, the old reading is stale like any other.
    let after_reset = after(2.0 * 86400.0 + 60.0);
    assert_eq!(usage.limit_hit(after_reset), None);
    assert!(usage.is_stale(after_reset, fifteen));
}

// MARK: The limit a session waits for

#[test]
fn limit_hit_picks_the_window_that_lifts_last() {
    let mut usage = full_usage();
    assert_eq!(usage.limit_hit(now()), None);
    usage.five_hour = Some(window(104.0, 47.0 * 60.0, SESSION));
    assert_eq!(
        usage.limit_hit(now()),
        Some(LimitHit {
            window: LimitWindowKind::Session,
            resets_at: Some(after(47.0 * 60.0))
        })
    );
    // The week is used up too: waiting for the session isn't enough.
    usage.seven_day = Some(window(100.0, 2.0 * 86400.0, WEEK));
    assert_eq!(
        usage.limit_hit(now()),
        Some(LimitHit {
            window: LimitWindowKind::Weekly,
            resets_at: Some(after(2.0 * 86400.0))
        })
    );
    // A model's week that lifts even later.
    usage.scoped = vec![("Opus".to_owned(), window(100.0, 3.0 * 86400.0, WEEK))];
    assert_eq!(
        usage.limit_hit(now()).map(|hit| hit.window),
        Some(LimitWindowKind::Scoped("Opus".to_owned()))
    );
    // A window whose reset passed restarted at 0%.
    assert_eq!(usage.limit_hit(after(4.0 * 86400.0)), None);
    // The ring's weekly_opus is the window it waits for.
    let all = windows(&usage, now());
    assert!(all.iter().any(|w| w.id == "weekly_opus"));
}

#[test]
fn money_windows_never_block() {
    // Extra usage over its limit is money, not a wall: no window of it counts.
    let mut usage = AccountUsage::new(IdentityId::from("a"), UsageSource::Probe, now());
    usage.five_hour = Some(window(10.0, 3600.0, SESSION));
    usage.extra_usage = Some(ExtraUsage {
        is_enabled: true,
        monthly_limit: Some(5000.0),
        used_credits: Some(6000.0),
        utilization: Some(120.0),
        currency: Some("USD".to_owned()),
    });
    assert_eq!(usage.limit_hit(now()), None);
    let extra = extra_usage_window(usage.extra_usage.as_ref()).expect("extra");
    assert_eq!(extra.used_fraction, 1.2);
    assert!(extra.money.is_some());
}

// MARK: Claude Desktop readings

#[test]
fn desktop_windows_become_a_full_snapshot() {
    let observed = after(-30.0);
    let desktop = |id: &str, label: Option<&str>, utilization: f64, resets: Option<f64>, d: u64| {
        DesktopWindow {
            id: id.to_owned(),
            label: label.map(str::to_owned),
            utilization,
            resets_at: resets.map(after),
            duration_s: d,
        }
    };
    let input = vec![
        desktop("session", None, 50.0, Some(3600.0), 18_000),
        desktop("weekly_all", None, 20.0, Some(86400.0), 604_800),
        desktop("weekly_scoped", Some("Fable"), 10.0, Some(86400.0), 0),
        desktop("weekly_opus", None, 30.0, Some(86400.0), 0),
        desktop("something_new", None, 90.0, None, 0),
    ];
    let usage =
        account_usage_from_desktop(IdentityId::from("a"), &input, observed).expect("a snapshot");
    assert_eq!(usage.updated_at, observed);
    // Differs from the Mac on purpose: the reading keeps its own source, so
    // the website hears "desktop" (Swift said `.cache`).
    assert_eq!(usage.source, UsageSource::Desktop);
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(50.0));
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(20.0));
    let names: Vec<&str> = usage.scoped.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Fable", "Opus"]);
    // An unknown duration is the window's standard one.
    assert_eq!(usage.scoped[0].1.duration_s, WEEK);
    // Claude Desktop's windows carry no money, so there is no extra usage
    // (Swift's extra_usage window has no counterpart in a `DesktopWindow`).
    assert_eq!(usage.extra_usage, None);

    // And back: the same ring windows.
    let back = windows(&usage, now());
    let ids: Vec<&str> = back.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        ids,
        ["session", "weekly_all", "weekly_fable", "weekly_opus"]
    );

    // Nothing to draw a ring from: no snapshot.
    assert_eq!(
        account_usage_from_desktop(IdentityId::from("a"), &[], now()),
        None
    );
}
