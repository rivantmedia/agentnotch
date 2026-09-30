//! When Claude Code is asked for usage, and the usage store's tuning
//! (UsageStore.swift "Tuning" and "Probe Scheduling", AU§9.1, §9.8). Probes
//! run one at a time, at most every 5 minutes per account on their own
//! schedule (the usage endpoint answers 429 to tighter polling), with
//! exponential backoff on failure. Pure.

use crate::model::{distant_past, IdentityId, UsageFetchState, UsageWindow};
use crate::usage::parser::seconds_between;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

/// How often `.claude.json` is checked for a new cached snapshot.
pub const CACHE_POLL_INTERVAL: Duration = Duration::from_secs(20);
/// Scheduled probes never run more often than this per account.
pub const MINIMUM_PROBE_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// A requested refresh probes at most this often per account.
pub const FORCED_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// A ring click probes only when the newest reading is older than this.
pub const RING_CLICK_FRESHNESS: Duration = Duration::from_secs(120);
/// Backoff after the first failure; doubles per failure up to [`MAX_BACKOFF`].
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(2 * 60);
pub const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// Model-scoped limits only come with full snapshots; probe for one when the
/// last is older than this, even while status lines keep 5h/7d fresh.
pub const FULL_SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// Claude Desktop's cache is read at most this often per account…
pub const EXTERNAL_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// …and this often after a read that found nothing.
pub const EXTERNAL_MISS_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// A requested read still leaves Claude Desktop's cache alone for this long.
pub const EXTERNAL_FORCED_INTERVAL: Duration = Duration::from_secs(5);
/// A seeded probe answer dated by `.claude.json` within this long of the
/// probe is Claude Code's own fresh cache, not a fallback.
pub const SEEDED_FRESH_WINDOW: Duration = Duration::from_secs(90);
/// Persisted state is written this long after the last change…
pub const STATE_SAVE_DELAY: Duration = Duration::from_secs(2);
/// …or this long after one that only a status line brought.
pub const STATUS_LINE_SAVE_DELAY: Duration = Duration::from_secs(30);
/// Claude Desktop's readings are dated by the server's `Date:` (whole
/// seconds, the server's clock) or the cache file: known newer only than
/// this much before that.
pub const EXTERNAL_CLOCK_ALLOWANCE: Duration = Duration::from_secs(60);
/// A check thrown away because its folder changed hands is tried again
/// after this long at the earliest.
pub const DISCARD_RETRY_DELAY: Duration = Duration::from_secs(60);
/// A Claude Code process not heard from for this long is forgotten: its
/// weekly window has reset since.
pub const STATUS_LINE_RETENTION: Duration = Duration::from_secs(UsageWindow::WEEKLY_DURATION_S);
/// A requested refresh re-reads the caches first (they cost nothing) and
/// decides about asking Claude Code once they are in, or after this long.
pub const REFRESH_READ_WAIT: Duration = Duration::from_secs(5);

/// Shown for accounts whose `.claude.json` has no claude.ai login.
pub const NOT_SIGNED_IN_TEXT: &str = "Not signed in to Claude";
/// An account only a Claude Parallel Profiles store holds: never checked there.
pub const NO_RUN_FOLDER_TEXT: &str =
    "Not checked: no VS Code workspace or terminal folder runs this account now";
/// Sealed runs never launch Claude Code, not even on request.
pub const PROBES_OFF_TEXT: &str = "Usage probes are off";
pub const CLAUDE_NOT_FOUND_TEXT: &str = "Claude Code not found";

/// The state of an account nobody is signed in to.
pub fn not_signed_in() -> UsageFetchState {
    UsageFetchState::Unavailable(NOT_SIGNED_IN_TEXT.to_owned())
}

/// The state of an account no run folder holds.
pub fn no_run_folder() -> UsageFetchState {
    UsageFetchState::Unavailable(NO_RUN_FOLDER_TEXT.to_owned())
}

/// The effective automatic probe interval, or `None` when probing is off.
pub fn effective_probe_interval(minutes: u32) -> Option<Duration> {
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60).max(MINIMUM_PROBE_INTERVAL))
}

/// Delay before retrying after `failures` consecutive failures.
pub fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let exponent = (failures - 1).min(10);
    INITIAL_BACKOFF
        .saturating_mul(1u32 << exponent)
        .min(MAX_BACKOFF)
}

/// "Usage check paused (too many requests), retrying in 5 min". A 429 on
/// the usage check says nothing about the account's own rate limits.
pub fn paused_text(retry_at: SystemTime, now: SystemTime) -> String {
    let minutes = (seconds_between(retry_at, now) / 60.0).ceil().max(1.0) as u64;
    format!("Usage check paused (too many requests), retrying in {minutes} min")
}

/// The fetch state after a `.claude.json` read. Signed out shows as
/// unavailable; signing (back) in clears that. A signed-in account keeps
/// whatever the probe last reported (including a probe's own "not signed
/// in", a revoked login the file doesn't know about yet).
pub fn fetch_state_after_cache_poll(
    current: Option<&UsageFetchState>,
    was_signed_in: Option<bool>,
    is_signed_in: bool,
) -> Option<UsageFetchState> {
    if !is_signed_in {
        return Some(not_signed_in());
    }
    if was_signed_in != Some(true) && current == Some(&not_signed_in()) {
        return Some(UsageFetchState::Idle);
    }
    current.cloned()
}

/// Whether Claude Desktop's cache is due a read for an account.
pub fn is_external_poll_due(
    last_poll_at: Option<SystemTime>,
    last_was_miss: bool,
    now: SystemTime,
) -> bool {
    let Some(last_poll_at) = last_poll_at else {
        return true;
    };
    let interval = if last_was_miss {
        EXTERNAL_MISS_INTERVAL
    } else {
        EXTERNAL_POLL_INTERVAL
    };
    seconds_between(now, last_poll_at) >= interval.as_secs_f64()
}

/// What [`next_scheduled_probe`] looks at.
#[derive(Debug, Clone, Default)]
pub struct ProbeSchedule<'a> {
    pub signed_in: Option<&'a BTreeMap<IdentityId, bool>>,
    pub paused: Option<&'a BTreeSet<IdentityId>>,
    pub next_attempt_at: Option<&'a BTreeMap<IdentityId, SystemTime>>,
    pub last_probe_at: Option<&'a BTreeMap<IdentityId, SystemTime>>,
    pub newest_data_at: Option<&'a BTreeMap<IdentityId, SystemTime>>,
    pub newest_full_at: Option<&'a BTreeMap<IdentityId, SystemTime>>,
}

/// Which account to probe next on the schedule, if any: the signed-in,
/// visible, unpaused account with the oldest data, among those whose newest
/// reading is older than the interval (or whose last full snapshot is older
/// than 15 minutes), outside their backoff and not probed in the last 5
/// minutes. Of equally old ones the first candidate.
pub fn next_scheduled_probe(
    candidates: &[IdentityId],
    interval: Duration,
    now: SystemTime,
    schedule: &ProbeSchedule<'_>,
) -> Option<IdentityId> {
    fn at<'a>(
        map: Option<&'a BTreeMap<IdentityId, SystemTime>>,
        id: &IdentityId,
    ) -> Option<&'a SystemTime> {
        map.and_then(|m| m.get(id))
    }
    let newest = |id: &IdentityId| {
        at(schedule.newest_data_at, id)
            .copied()
            .unwrap_or_else(distant_past)
    };
    let mut best: Option<&IdentityId> = None;
    for id in candidates {
        let signed_in = schedule.signed_in.and_then(|m| m.get(id)).copied();
        if signed_in != Some(true) || schedule.paused.is_some_and(|p| p.contains(id)) {
            continue;
        }
        if at(schedule.next_attempt_at, id).is_some_and(|next| *next > now) {
            continue;
        }
        if at(schedule.last_probe_at, id)
            .is_some_and(|last| seconds_between(now, *last) < MINIMUM_PROBE_INTERVAL.as_secs_f64())
        {
            continue;
        }
        let newest_full = at(schedule.newest_full_at, id)
            .copied()
            .unwrap_or_else(distant_past);
        let due = seconds_between(now, newest(id)) >= interval.as_secs_f64()
            || seconds_between(now, newest_full)
                >= interval.max(FULL_SNAPSHOT_MAX_AGE).as_secs_f64();
        if due && best.is_none_or(|current| newest(id) < newest(current)) {
            best = Some(id);
        }
    }
    best.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::from_ms;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
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
    fn scheduled_probes_pick_the_stalest_due_account() {
        // A3_UsageRefreshPolicyTests.scheduledProbesPickTheStalestDueAccount
        let now = from_ms(1_800_000_000_000);
        let id = |s: &str| IdentityId::from(s);
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

        // Fresh 5h/7d from a status line, but no full snapshot for 15
        // minutes: probe for the scoped limits.
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
}
