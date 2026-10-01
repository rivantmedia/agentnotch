//! When a Stop is the end of a turn (TurnCompletion.swift). Our Stop hook is
//! one of several Claude Code runs in parallel at the end of a turn; a
//! blocking one (the built-in /goal, plugin loops) makes Claude continue, up
//! to 8 times, with no new prompt. So a Stop is only a pending completion
//! until Claude Code's session registry (`<configDir>\sessions\<pid>.json`)
//! reports the session idle after it: it stays busy while Stop hooks and any
//! continuation run. Sessions the registry doesn't follow fall back to a
//! short quiet period. Pure: the store feeds it the registry status and the
//! clock.

use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The turn is over: record the completion.
    Confirm,
    /// Claude may still be running Stop hooks or continuing.
    Wait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionTiming {
    /// No registry follows this turn: confirm after this quiet period.
    pub fallback_delay: Duration,
    /// The registry follows the turn but never reported it idle.
    pub registry_timeout: Duration,
    /// Registry and hook timestamps come from different writers.
    pub clock_tolerance: Duration,
}

impl CompletionTiming {
    pub const STANDARD: CompletionTiming = CompletionTiming {
        fallback_delay: Duration::from_secs(4),
        registry_timeout: Duration::from_secs(90),
        clock_tolerance: Duration::from_secs(1),
    };

    /// A Stop with no registry following it completes at once (the Mac
    /// tests' `.immediate`).
    pub const IMMEDIATE: CompletionTiming = CompletionTiming {
        fallback_delay: Duration::ZERO,
        registry_timeout: Duration::from_secs(90),
        clock_tolerance: Duration::from_secs(1),
    };
}

impl Default for CompletionTiming {
    fn default() -> Self {
        CompletionTiming::STANDARD
    }
}

fn minus(t: SystemTime, d: Duration) -> SystemTime {
    t.checked_sub(d).unwrap_or(t)
}

/// Whether the Stop at `stop_at` ended the turn.
pub fn decide(
    stop_at: SystemTime,
    turn_started_at: Option<SystemTime>,
    registry_status: Option<&str>,
    registry_changed_at: Option<SystemTime>,
    now: SystemTime,
    timing: CompletionTiming,
) -> Decision {
    if let (Some(status), Some(changed_at)) = (registry_status, registry_changed_at) {
        let settled = status == "idle" || status == "shell";
        if settled && changed_at >= minus(stop_at, timing.clock_tolerance) {
            // The registry went idle after the Stop: every Stop hook ran and
            // none continued the turn.
            return Decision::Confirm;
        }
        if follows_turn(turn_started_at, stop_at, status, changed_at, timing) {
            return if now >= stop_at + timing.registry_timeout {
                Decision::Confirm
            } else {
                Decision::Wait
            };
        }
    }
    if now >= stop_at + timing.fallback_delay {
        Decision::Confirm
    } else {
        Decision::Wait
    }
}

/// When `decide` may change its answer by the clock alone (at least 50 ms
/// ahead, like the Mac's timer).
pub fn check_at(
    stop_at: SystemTime,
    turn_started_at: Option<SystemTime>,
    registry_status: Option<&str>,
    registry_changed_at: Option<SystemTime>,
    now: SystemTime,
    timing: CompletionTiming,
) -> SystemTime {
    let follows = match (registry_status, registry_changed_at) {
        (Some(status), Some(changed_at)) => {
            follows_turn(turn_started_at, stop_at, status, changed_at, timing)
        }
        _ => false,
    };
    let deadline = stop_at
        + if follows {
            timing.registry_timeout
        } else {
            timing.fallback_delay
        };
    deadline.max(now + Duration::from_millis(50))
}

/// The registry reported the session busy (or waiting on a dialog) since
/// this turn began, so its going idle will mark the end.
pub fn follows_turn(
    turn_started_at: Option<SystemTime>,
    stop_at: SystemTime,
    registry_status: &str,
    registry_changed_at: SystemTime,
    timing: CompletionTiming,
) -> bool {
    if registry_status != "busy" && registry_status != "waiting" {
        return false;
    }
    let turn_start = turn_started_at.unwrap_or(stop_at);
    registry_changed_at >= minus(turn_start, timing.clock_tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    // A1_SessionStoreRegressionTests.decisionTable
    #[test]
    fn decision_table() {
        let stop = UNIX_EPOCH + Duration::from_secs(1_000);
        let timing = CompletionTiming {
            fallback_delay: Duration::from_secs(4),
            registry_timeout: Duration::from_secs(90),
            clock_tolerance: Duration::from_secs(1),
        };
        let offset = |s: f64| {
            if s >= 0.0 {
                stop + Duration::from_secs_f64(s)
            } else {
                stop - Duration::from_secs_f64(-s)
            }
        };
        let decide = |status: Option<&str>, changed: Option<f64>, now: f64| {
            decide(
                stop,
                Some(stop - Duration::from_secs(10)),
                status,
                changed.map(offset),
                offset(now),
                timing,
            )
        };
        assert_eq!(decide(Some("idle"), Some(0.2), 0.2), Decision::Confirm); // idle after the Stop
        assert_eq!(decide(Some("busy"), Some(-9.9), 1.0), Decision::Wait); // busy since the turn began
        assert_eq!(decide(Some("busy"), Some(-9.9), 91.0), Decision::Confirm); // ...but not forever
        assert_eq!(decide(Some("waiting"), Some(0.5), 1.0), Decision::Wait); // a dialog after the Stop
        assert_eq!(decide(Some("idle"), Some(-60.0), 1.0), Decision::Wait); // stale idle: registry not following
        assert_eq!(decide(Some("idle"), Some(-60.0), 4.0), Decision::Confirm); // ...falls back to the quiet period
        assert_eq!(decide(None, None, 3.9), Decision::Wait);
        assert_eq!(decide(None, None, 4.0), Decision::Confirm);
    }

    #[test]
    fn check_times() {
        let stop = UNIX_EPOCH + Duration::from_secs(1_000);
        let timing = CompletionTiming::STANDARD;
        assert_eq!(
            check_at(stop, None, None, None, stop, timing),
            stop + Duration::from_secs(4)
        );
        assert_eq!(
            check_at(stop, Some(stop), Some("busy"), Some(stop), stop, timing),
            stop + Duration::from_secs(90)
        );
        let late = stop + Duration::from_secs(100);
        assert_eq!(
            check_at(stop, None, None, None, late, timing),
            late + Duration::from_millis(50)
        );
    }
}
