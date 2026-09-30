//! How readings of one window are weighed against each other
//! (UsageStore.swift "Merging", AU§9.4). Per window the most current reading
//! wins: one known to have been taken after the others, even when lower (an
//! early reset), else the likeliest. Everything here is pure.
//!
//! What the rules rest on (Claude Code 2.1.282, CLAUDE.md "Status line rate
//! limits"): a process's status line repeats its own latest API response's
//! rate limits however old; a response's numbers are applied when it ends,
//! so a process's changed numbers can be older than its previous report; an
//! early reset lowers usage but keeps the reset time.

use crate::model::{
    distant_past, AccountUsage, IdentityId, StatusLineRecord, UsageReading, UsageSource,
    UsageWindow,
};
use crate::usage::parser::seconds_between;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A reading of the same window known to be later, but lower, counts only
/// when lower by more than this many points (what the website's chart takes
/// for a reset too). A smaller drop is rounding (the usage endpoint's 9
/// against a status line's 9.4), or a response that started before the
/// other reading and ended after it.
pub const RESET_DROP_MINIMUM: f64 = 5.0;

/// The prefix of a status line key that names a Claude Code process.
pub const PROCESS_KEY_PREFIX: &str = "pid:";

/// Which of a status line's two windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusWindow {
    FiveHour,
    SevenDay,
}

impl StatusWindow {
    pub fn of(self, record: &StatusLineRecord) -> Option<&UsageReading> {
        match self {
            StatusWindow::FiveHour => record.five_hour.as_ref(),
            StatusWindow::SevenDay => record.seven_day.as_ref(),
        }
    }

    pub fn of_usage(self, usage: &AccountUsage) -> Option<&UsageWindow> {
        match self {
            StatusWindow::FiveHour => usage.five_hour.as_ref(),
            StatusWindow::SevenDay => usage.seven_day.as_ref(),
        }
    }
}

/// Two readings describe the same window when their reset times are close:
/// consecutive windows reset at least a full window length apart, while
/// sources disagree by at most rounding (epoch seconds against fractional
/// ISO 8601).
pub fn is_same_window(lhs: &UsageWindow, rhs: &UsageWindow) -> bool {
    let (Some(left), Some(right)) = (lhs.resets_at, rhs.resets_at) else {
        return false;
    };
    seconds_between(left, right).abs() < lhs.duration_s.min(rhs.duration_s) as f64 / 4.0
}

/// `lower` is the same window as `higher`, lower by no more than
/// [`RESET_DROP_MINIMUM`]: rounding or a late response, not a reset.
pub fn is_small_drop(higher: &UsageWindow, lower: &UsageWindow) -> bool {
    if !is_same_window(higher, lower) {
        return false;
    }
    let drop = higher.utilization - lower.utilization;
    drop > 0.0 && drop <= RESET_DROP_MINIMUM
}

/// Whether `candidate` is known to have been taken after `other`, and so
/// wins whatever its numbers say. Usage can come down within a window:
/// Anthropic resets limits early and keeps the reset time, and only the
/// reading after the reset is right. Not, though, for a reading of a window
/// that had already reset when it was taken (the usage endpoint can still
/// answer with the ended window): that says nothing about a later one. Nor
/// for a small drop ([`is_small_drop`]).
pub fn supersedes(candidate: &UsageReading, other: &UsageReading) -> bool {
    let Some(not_before) = candidate.not_before else {
        return false;
    };
    if not_before < other.at || candidate.at <= other.at {
        return false;
    }
    if let (Some(reset), Some(other_reset)) = (candidate.window.resets_at, other.window.resets_at) {
        if reset <= not_before && other_reset > reset {
            return false;
        }
    }
    !is_small_drop(&other.window, &candidate.window)
}

/// Whether `candidate` is a more current reading of a window than `other`
/// when neither is known to have been taken after the other (see
/// [`supersedes`]).
///
/// Arrival time alone can't decide: a status line re-run repeats whatever
/// that process's last API response said, possibly hours ago. So the data
/// decides first: a later reset means a newer window, and within one window
/// usage only grows short of an early reset, so the higher reading is the
/// likelier the more recent. Arrival time breaks ties and covers readings
/// without a reset time.
pub fn is_more_current(candidate: &UsageReading, other: &UsageReading) -> bool {
    if let (Some(candidate_reset), Some(other_reset)) =
        (candidate.window.resets_at, other.window.resets_at)
    {
        if !is_same_window(&candidate.window, &other.window) {
            return candidate_reset > other_reset;
        }
        if candidate.window.utilization != other.window.utilization {
            return candidate.window.utilization > other.window.utilization;
        }
    }
    candidate.at > other.at
}

/// Where in `readings` the most current one is: any known to have been
/// taken before another is out, and [`is_more_current`] picks among the
/// rest, whose order is unknown (of two even that can't tell apart, the one
/// listed first). The latest `at` is never out, so a non-empty list always
/// has an answer.
pub fn most_current_index(readings: &[UsageReading]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, reading) in readings.iter().enumerate() {
        if readings.iter().any(|other| supersedes(other, reading)) {
            continue;
        }
        best = match best {
            Some(current) if !is_more_current(reading, &readings[current]) => Some(current),
            _ => Some(index),
        };
    }
    best
}

/// The most current of several readings of a window ([`most_current_index`]).
pub fn most_current(readings: &[UsageReading]) -> Option<&UsageReading> {
    most_current_index(readings).map(|index| &readings[index])
}

/// The same numbers again. Reset times within a second are the same
/// (`usage-state.json` keeps whole seconds; Claude Code sends whole seconds).
pub fn is_repeat(lhs: &UsageWindow, rhs: &UsageWindow) -> bool {
    if lhs.utilization != rhs.utilization || lhs.duration_s != rhs.duration_s {
        return false;
    }
    match (lhs.resets_at, rhs.resets_at) {
        (None, None) => true,
        (Some(left), Some(right)) => seconds_between(left, right).abs() < 1.0,
        _ => false,
    }
}

/// A Claude Code process's status line record after it reported
/// `five_hour`/`seven_day` at `received_at`. Its rate limits are its own
/// latest API response's (one set per process, replaced by each newer
/// response, even a lower one; none before its first response), and a
/// status line re-run repeats them however old. So:
/// - the same window again is no news: the reading keeps its time, so a
///   process repeating stale rate limits neither looks fresh nor holds off
///   the probe;
/// - a different one came from a response applied after the previous report
///   (its numbers may still be older: see [`RESET_DROP_MINIMUM`]), so a
///   small drop is not taken, and the higher reading keeps its time;
/// - a process's first report is newer than the process (`started_at`, from
///   the kernel), when that is known.
pub fn advance(
    record: Option<&StatusLineRecord>,
    five_hour: Option<&UsageWindow>,
    seven_day: Option<&UsageWindow>,
    received_at: SystemTime,
    started_at: Option<SystemTime>,
) -> StatusLineRecord {
    let not_before = record
        .map(|r| r.last_report_at)
        .or(started_at)
        .filter(|bound| *bound < received_at);
    let next = |stored: Option<&UsageReading>, window: Option<&UsageWindow>| {
        let Some(window) = window else {
            return stored.cloned();
        };
        if let Some(stored) = stored {
            if is_repeat(&stored.window, window) || is_small_drop(&stored.window, window) {
                return Some(stored.clone());
            }
        }
        Some(UsageReading::new(window.clone(), received_at, not_before))
    };
    StatusLineRecord {
        last_report_at: record.map_or(received_at, |r| r.last_report_at.max(received_at)),
        five_hour: next(record.and_then(|r| r.five_hour.as_ref()), five_hour),
        seven_day: next(record.and_then(|r| r.seven_day.as_ref()), seven_day),
    }
}

/// Whose status line an update is: the Claude Code process's when it is
/// known (rate limits belong to the process, and outlive a `/clear` into a
/// new session), with its start time when that is known (Windows reuses
/// pids quickly), else the session's.
pub fn status_line_key(
    session_id: &str,
    pid: Option<u32>,
    started_at: Option<SystemTime>,
) -> String {
    let Some(pid) = pid else {
        return format!("session:{session_id}");
    };
    match started_at {
        None => format!("{PROCESS_KEY_PREFIX}{pid}"),
        Some(started) => {
            // Whole seconds, truncated toward zero, as the Mac writes them.
            let seconds = match started.duration_since(UNIX_EPOCH) {
                Ok(since) => since.as_secs() as i128,
                Err(before) => -(before.duration().as_secs() as i128),
            };
            format!("{PROCESS_KEY_PREFIX}{pid}@{seconds}")
        }
    }
}

/// A process's key that names its start time too ([`status_line_key`]).
pub fn is_process_key_with_start(key: &str) -> bool {
    key.starts_with(PROCESS_KEY_PREFIX) && key.contains('@')
}

/// One window of a folder's processes, in a fixed order.
pub fn readings(
    processes: &BTreeMap<String, StatusLineRecord>,
    window: StatusWindow,
) -> Vec<UsageReading> {
    processes
        .values()
        .filter_map(|record| window.of(record).cloned())
        .collect()
}

/// The readings of a window from the folders an account's sessions ran in
/// (one per Claude Code process) that count, in a fixed order. While Claude
/// Parallel Profiles mirrors accounts into `~/.claude`, a reading that came
/// through `~/.claude` counts only until one from the account's own folders
/// arrives after it. (Inert on native Windows, where nothing mirrors.)
pub fn status_candidates(
    by_folder: &BTreeMap<String, Vec<UsageReading>>,
    default_folder: &str,
    mirrors_default: bool,
) -> Vec<UsageReading> {
    let newest_own = by_folder
        .iter()
        .filter(|(folder, _)| folder.as_str() != default_folder)
        .flat_map(|(_, readings)| readings.iter().map(|r| r.at))
        .max();
    let mut candidates = Vec::new();
    for (folder, readings) in by_folder {
        match newest_own {
            Some(newest_own) if mirrors_default && folder == default_folder => {
                candidates.extend(readings.iter().filter(|r| r.at > newest_own).cloned());
            }
            _ => candidates.extend(readings.iter().cloned()),
        }
    }
    candidates
}

/// The most current of those readings.
pub fn combined_status(
    by_folder: &BTreeMap<String, Vec<UsageReading>>,
    default_folder: &str,
    mirrors_default: bool,
) -> Option<UsageReading> {
    let candidates = status_candidates(by_folder, default_folder, mirrors_default);
    most_current(&candidates).cloned()
}

/// The status line reading that wins a window over the full snapshot's,
/// `None` when the snapshot's stays (it goes first: of two readings nothing
/// tells apart, it is kept) or there is none.
pub fn winning_status(
    readings: &[UsageReading],
    snapshot: Option<&UsageReading>,
) -> Option<UsageReading> {
    let candidates: Vec<UsageReading> = snapshot
        .into_iter()
        .chain(readings.iter())
        .cloned()
        .collect();
    let index = most_current_index(&candidates)?;
    (snapshot.is_none() || index > 0).then(|| candidates[index].clone())
}

/// The most current reading wins per window, the full snapshot's and the
/// status lines' together (one status line known to be newer than the
/// snapshot is enough, whichever of them shows); scoped limits, extra usage
/// and plan come from the full snapshot. `updated_at`/`source` describe the
/// newest reading used.
pub fn merge(
    account_id: &IdentityId,
    full: Option<&AccountUsage>,
    status_five_hour: &[UsageReading],
    status_seven_day: &[UsageReading],
) -> Option<AccountUsage> {
    if full.is_none() && status_five_hour.is_empty() && status_seven_day.is_empty() {
        return None;
    }
    let mut result = full.cloned().unwrap_or_else(|| {
        AccountUsage::new(account_id.clone(), UsageSource::StatusLine, distant_past())
    });
    let mut newest = full.map_or_else(distant_past, |f| f.updated_at);
    let mut newest_source = full.map_or(UsageSource::StatusLine, |f| f.source);

    let mut take = |readings: &[UsageReading], current: Option<UsageWindow>| {
        let snapshot = match (&current, full) {
            (Some(window), Some(full)) => Some(UsageReading::of_snapshot(window.clone(), full)),
            _ => None,
        };
        let Some(reading) = winning_status(readings, snapshot.as_ref()) else {
            return current;
        };
        if reading.at > newest {
            newest = reading.at;
            newest_source = UsageSource::StatusLine;
        }
        Some(reading.window)
    };
    result.five_hour = take(status_five_hour, result.five_hour.take());
    result.seven_day = take(status_seven_day, result.seven_day.take());
    result.updated_at = newest;
    result.source = newest_source;
    Some(result)
}

/// Full snapshots replace each other by when they were taken, whichever
/// source they came from (Claude Desktop, `.claude.json`, the probe).
pub fn is_newer_full_snapshot(snapshot: &AccountUsage, current: Option<&AccountUsage>) -> bool {
    current.is_none_or(|current| snapshot.updated_at > current.updated_at)
}
