//! `usage::schedule` and the probe's answer dating: the scheduling tests of
//! the Mac's UsageStoreLogicTests (UsageProbeTests.swift) and the pure ones
//! of A3_UsageStoreTests (desktopPollCadence, probeAnswersAreDatedHonestly,
//! getUsageWithoutLimitsMayBeSeeded, pausedText,
//! scheduledProbesPickTheStalestDueAccount). The merge rules of the same
//! suite are in `usage_merge.rs`.

use agentnotch_engine::core::time::from_ms;
use agentnotch_engine::model::{
    distant_past, IdentityId, UsageFetchState, UsageSource, UsageWindow,
};
use agentnotch_engine::usage::parser::{
    parse_get_usage_response, CachedUsageSnapshot, GetUsageResult, ParsedUsage,
};
use agentnotch_engine::usage::probe::interpret_probe_answer;
use agentnotch_engine::usage::schedule::{
    backoff, effective_probe_interval, fetch_state_after_cache_poll, is_external_poll_due,
    next_scheduled_probe, not_signed_in, paused_text, ProbeSchedule,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn id(text: &str) -> IdentityId {
    IdentityId::from(text)
}

#[test]
fn cache_poll_keeps_the_probes_verdict() {
    // UsageStoreLogicTests.cachePollKeepsTheProbesVerdict
    let signed_out = not_signed_in();
    let failed = UsageFetchState::Failed("x".into());
    // Signed out per .claude.json: always shown.
    assert_eq!(
        fetch_state_after_cache_poll(Some(&UsageFetchState::Idle), Some(true), false),
        Some(signed_out.clone())
    );
    // Signed back in: cleared.
    assert_eq!(
        fetch_state_after_cache_poll(Some(&signed_out), Some(false), true),
        Some(UsageFetchState::Idle)
    );
    assert_eq!(
        fetch_state_after_cache_poll(Some(&signed_out), None, true),
        Some(UsageFetchState::Idle)
    );
    // Still signed in per the file, but the probe found the login dead: keep that.
    assert_eq!(
        fetch_state_after_cache_poll(Some(&signed_out), Some(true), true),
        Some(signed_out)
    );
    assert_eq!(
        fetch_state_after_cache_poll(Some(&failed), Some(true), true),
        Some(failed)
    );
    assert_eq!(fetch_state_after_cache_poll(None, None, true), None);
}

#[test]
fn backoff_doubles_and_caps() {
    // UsageStoreLogicTests.backoffDoublesAndCaps
    assert_eq!(backoff(0), Duration::ZERO);
    assert_eq!(backoff(1), secs(120));
    assert_eq!(backoff(2), secs(240));
    assert_eq!(backoff(4), secs(960));
    assert_eq!(backoff(5), secs(1800));
    assert_eq!(backoff(50), secs(1800));
    assert_eq!(backoff(u32::MAX), secs(1800));
}

#[test]
fn probe_interval_has_a_floor() {
    // UsageStoreLogicTests.probeIntervalHasAFloor
    assert_eq!(effective_probe_interval(0), None);
    assert_eq!(effective_probe_interval(1), Some(secs(300)));
    assert_eq!(effective_probe_interval(5), Some(secs(300)));
    assert_eq!(effective_probe_interval(30), Some(secs(1800)));
}

#[test]
fn desktop_poll_cadence() {
    // A3_ExternalUsageMergeTests.desktopPollCadence
    let now = from_ms(1_800_000_000_000);
    assert!(is_external_poll_due(None, false, now));
    assert!(!is_external_poll_due(Some(now - secs(30)), false, now));
    assert!(is_external_poll_due(Some(now - secs(60)), false, now));
    assert!(!is_external_poll_due(Some(now - secs(120)), true, now));
    assert!(is_external_poll_due(Some(now - secs(300)), true, now));
}

#[test]
fn paused_text_rounds_up_to_minutes() {
    // A3_UsageRefreshPolicyTests.pausedText
    let now = from_ms(1_800_000_000_000);
    assert_eq!(
        paused_text(now + secs(300), now),
        "Usage check paused (too many requests), retrying in 5 min"
    );
    assert!(paused_text(now + secs(61), now).ends_with("in 2 min"));
    assert!(paused_text(now, now).ends_with("in 1 min"));
}

#[test]
fn scheduled_probes_pick_the_stalest_due_account() {
    // A3_UsageRefreshPolicyTests.scheduledProbesPickTheStalestDueAccount
    let now = from_ms(1_800_000_000_000);
    let candidates: Vec<IdentityId> = [
        "fresh",
        "stale",
        "staler",
        "signedOut",
        "paused",
        "backingOff",
        "justProbed",
    ]
    .into_iter()
    .map(id)
    .collect();
    let signed_in: BTreeMap<IdentityId, bool> = candidates
        .iter()
        .map(|c| (c.clone(), c.as_str() != "signedOut"))
        .collect();
    let paused: BTreeSet<IdentityId> = [id("paused")].into();
    let next_attempt_at: BTreeMap<_, _> = [(id("backingOff"), now + secs(60))].into();
    let last_probe_at: BTreeMap<_, _> = [(id("justProbed"), now - secs(60))].into();
    let newest_data_at: BTreeMap<_, _> = [
        (id("fresh"), now - secs(10)),
        (id("stale"), now - secs(600)),
        (id("staler"), now - secs(900)),
        (id("paused"), distant_past()),
        (id("backingOff"), distant_past()),
        (id("justProbed"), distant_past()),
        (id("signedOut"), distant_past()),
    ]
    .into();
    let newest_full_at: BTreeMap<_, _> = [(id("fresh"), now - secs(10))].into();
    let next = next_scheduled_probe(
        &candidates,
        secs(300),
        now,
        &ProbeSchedule {
            signed_in: Some(&signed_in),
            paused: Some(&paused),
            next_attempt_at: Some(&next_attempt_at),
            last_probe_at: Some(&last_probe_at),
            newest_data_at: Some(&newest_data_at),
            newest_full_at: Some(&newest_full_at),
        },
    );
    assert_eq!(next, Some(id("staler")));

    // Fresh 5h/7d from a status line, but no full snapshot for 15 minutes:
    // probe for the scoped limits.
    let signed_in: BTreeMap<_, _> = [(id("a"), true)].into();
    let newest_data_at: BTreeMap<_, _> = [(id("a"), now - secs(5))].into();
    let newest_full_at: BTreeMap<_, _> = [(id("a"), now - secs(16 * 60))].into();
    let scoped = next_scheduled_probe(
        &[id("a")],
        secs(300),
        now,
        &ProbeSchedule {
            signed_in: Some(&signed_in),
            newest_data_at: Some(&newest_data_at),
            newest_full_at: Some(&newest_full_at),
            ..ProbeSchedule::default()
        },
    );
    assert_eq!(scoped, Some(id("a")));
}

#[test]
fn equally_old_accounts_keep_the_first_candidate() {
    // Swift's `min(by:)` keeps the first of equal elements; Rust's `min_by`
    // would keep the last, so the port compares strictly.
    let now = from_ms(1_800_000_000_000);
    let candidates = [id("one"), id("two"), id("three")];
    let signed_in: BTreeMap<_, _> = candidates.iter().map(|c| (c.clone(), true)).collect();
    let stale = now - secs(3600);
    let newest_data_at: BTreeMap<_, _> = candidates.iter().map(|c| (c.clone(), stale)).collect();
    let next = next_scheduled_probe(
        &candidates,
        secs(300),
        now,
        &ProbeSchedule {
            signed_in: Some(&signed_in),
            newest_data_at: Some(&newest_data_at),
            ..ProbeSchedule::default()
        },
    );
    assert_eq!(next, Some(id("one")));
}

/// Usage bodies for the pure tests (the Swift `UsageBodies`).
struct UsageBodies {
    now: SystemTime,
}

impl UsageBodies {
    fn parsed(&self, session: f64, seeded: bool) -> ParsedUsage {
        ParsedUsage {
            five_hour: Some(UsageWindow::new(
                session,
                Some(self.now + secs(3600)),
                18_000,
            )),
            seven_day: Some(UsageWindow::new(
                10.0,
                Some(self.now + secs(86_400)),
                604_800,
            )),
            is_possibly_seeded: seeded,
            ..ParsedUsage::default()
        }
    }
}

fn copy_of(fetched_at: SystemTime, usage: ParsedUsage) -> CachedUsageSnapshot {
    CachedUsageSnapshot {
        fetched_at,
        account_uuid: Some("x".into()),
        usage,
    }
}

#[test]
fn probe_answers_are_dated_honestly() {
    // A3_UsageRefreshPolicyTests.probeAnswersAreDatedHonestly
    let now = from_ms(1_800_000_000_000);
    let bodies = UsageBodies { now };
    let a = id("a");
    // A normal answer: fetched now.
    let (snapshot, rate_limited) =
        interpret_probe_answer(&bodies.parsed(20.0, false), &a, now, None, None);
    let snapshot = snapshot.expect("a fresh answer is a snapshot");
    assert_eq!(snapshot.updated_at, now);
    assert_eq!(snapshot.source, UsageSource::Probe);
    assert!(!rate_limited);
    // Seeded, matching a 40-minute-old copy: that date, and back off.
    let copy = copy_of(now - secs(2400), bodies.parsed(20.0, false));
    let (seeded, rate_limited) =
        interpret_probe_answer(&bodies.parsed(20.0, true), &a, now, Some(&copy), None);
    assert_eq!(
        seeded.expect("dated by the copy").updated_at,
        now - secs(2400)
    );
    assert!(rate_limited);
    // Seeded but the copy is Claude Code's fresh cache: fine.
    let recent = copy_of(now - secs(30), bodies.parsed(20.0, false));
    let (_, rate_limited) =
        interpret_probe_answer(&bodies.parsed(20.0, true), &a, now, Some(&recent), None);
    assert!(!rate_limited);
    // Seeded and nothing to date it by: dropped.
    let other = copy_of(now, bodies.parsed(77.0, false));
    let (undated, rate_limited) =
        interpret_probe_answer(&bodies.parsed(20.0, true), &a, now, Some(&other), None);
    assert!(undated.is_none() && rate_limited);
    let (none, _) = interpret_probe_answer(&bodies.parsed(20.0, true), &a, now, None, None);
    assert!(none.is_none());
}

#[test]
fn get_usage_without_limits_may_be_seeded() {
    // A3_UsageRefreshPolicyTests.getUsageWithoutLimitsMayBeSeeded
    fn answer(rate_limits: Value) -> ParsedUsage {
        let mut body = Map::new();
        body.insert("rate_limits_available".into(), json!(true));
        body.insert("rate_limits".into(), rate_limits);
        match parse_get_usage_response(&body) {
            GetUsageResult::Usage(usage) => usage,
            other => panic!("not usage: {other:?}"),
        }
    }
    let window = json!({"utilization": 5, "resets_at": "2026-09-24T12:50:00Z"});
    assert!(answer(json!({"five_hour": window})).is_possibly_seeded);
    assert!(
        !answer(json!({"five_hour": window, "limits": [{"kind": "session", "percent": 5}]}))
            .is_possibly_seeded
    );
    assert!(!answer(json!({"five_hour": window, "limits": []})).is_possibly_seeded);
}
