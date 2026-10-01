//! Usage-limit readings over time, for sync (CL§8; the Mac's
//! `UsageHistoryRecorder.swift`). The usage store keeps only the latest
//! reading per account; this keeps each reading it takes in, with where it
//! came from, until the website has it:
//! - `probe`: Claude Code's `get_usage`;
//! - `statusLine`: a live session's `rate_limits` (5-hour and weekly only);
//! - `claudeJson`: Claude Code's cached copy in `.claude.json`;
//! - `desktop`: Claude Desktop's cache.
//!
//! Windows (the contract's ids): `session`, `weekly_all`, `weekly_<model>`,
//! `extra_usage`.
//!
//! A reading is dated when Claude Code or Claude Desktop took it. Readings
//! are deduplicated by (account, source, observedAt) with windows merged by
//! id; a reading older than the last one taken from the same account and
//! source is dropped, and so is one that repeats the previous values within
//! ten minutes (caches are re-read every 20 seconds). The outbox is
//! bounded: the oldest go first. Kept in `<support>\cloud-usage-outbox.json`
//! (v1); in memory only when sealed.

use crate::cloud::contract::{date, seconds_between, SyncReading, SyncWindow, UsageSourceName};
use crate::cloud::files::{lock, StateFile};
use crate::cloud::keys;
use crate::model::{AccountUsage, IdentityId, UsageWindow};
use crate::platform::SecureFiles;
use crate::runtime_types::UsageObservation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub const FILE_NAME: &str = "cloud-usage-outbox.json";
/// Readings kept waiting at most.
pub const CAPACITY: usize = 5_000;
/// A reading repeating the last values is dropped within this long.
pub const REPEAT_WINDOW: Duration = Duration::from_secs(10 * 60);

/// The windows of a full snapshot as the outbox records them: contract ids,
/// utilization as read (not reset to 0 for a window whose reset has passed
/// since), never below 0; `extra_usage` only when enabled, from its own
/// figure or used/limit. Pure (the Mac's `UsageObservation.windows(from:)`).
pub fn observation_windows(usage: &AccountUsage) -> Vec<(String, f64, Option<SystemTime>)> {
    let mut windows = Vec::new();
    let mut add = |id: &str, window: Option<&UsageWindow>| {
        let Some(window) = window else { return };
        if !window.utilization.is_finite() {
            return;
        }
        if let Some(id) = keys::contract_window_id(id) {
            windows.push((id, window.utilization.max(0.0), window.resets_at));
        }
    };
    add(keys::SESSION_WINDOW, usage.five_hour.as_ref());
    add(keys::WEEKLY_WINDOW, usage.seven_day.as_ref());
    let mut seen = vec![
        keys::SESSION_WINDOW.to_owned(),
        keys::WEEKLY_WINDOW.to_owned(),
    ];
    for (name, window) in &usage.scoped {
        let id = keys::scoped_id(name);
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        add(&id, Some(window));
    }
    if let Some(extra) = usage.extra_usage.as_ref().filter(|e| e.is_enabled) {
        let utilization = match (extra.utilization, extra.monthly_limit, extra.used_credits) {
            (Some(value), _, _) => Some(value),
            (None, Some(limit), Some(used)) if limit > 0.0 => Some(used / limit * 100.0),
            _ => None,
        };
        if let Some(utilization) = utilization.filter(|u| u.is_finite()) {
            windows.push((keys::EXTRA_USAGE_WINDOW.to_owned(), utilization, None));
        }
    }
    windows
}

/// A reading waiting to be sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedUsageReading {
    #[serde(rename = "accountKey")]
    pub account_key: String,
    #[serde(rename = "identityId")]
    pub identity_id: IdentityId,
    #[serde(rename = "source")]
    pub source: UsageSourceName,
    #[serde(rename = "observedAt", with = "date")]
    pub observed_at: SystemTime,
    #[serde(rename = "windows")]
    pub windows: Vec<SyncWindow>,
}

impl RecordedUsageReading {
    /// An observation for the account `account_key`: its windows under
    /// their contract ids (one the website would refuse, or a reading that
    /// isn't a number, is left out), its source by contract name.
    pub fn from_observation(observation: &UsageObservation, account_key: &str) -> Self {
        let windows = observation
            .windows
            .iter()
            .filter(|(_, utilization, _)| utilization.is_finite())
            .filter_map(|(id, utilization, resets_at)| {
                keys::contract_window_id(id).map(|id| SyncWindow {
                    id,
                    utilization: *utilization,
                    resets_at: *resets_at,
                })
            })
            .collect();
        RecordedUsageReading {
            account_key: account_key.to_owned(),
            identity_id: observation.identity.clone(),
            source: UsageSourceName::from_model(observation.source),
            observed_at: observation.observed_at,
            windows,
        }
    }

    /// (account, source, observedAt to the millisecond).
    pub fn dedupe_key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.account_key,
            self.source.as_str(),
            date::rounded_ms(self.observed_at)
        )
    }

    fn stream(&self) -> String {
        format!("{}|{}", self.account_key, self.source.as_str())
    }

    pub fn contract(&self) -> SyncReading {
        SyncReading {
            account_key: self.account_key.clone(),
            source: self.source,
            observed_at: self.observed_at,
            windows: self.windows.clone(),
        }
    }
}

/// `cloud-usage-outbox.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contents {
    #[serde(rename = "version")]
    pub version: u32,
    #[serde(rename = "pending")]
    pub pending: Vec<RecordedUsageReading>,
    /// Per account and source (`accountKey|source`): the newest reading
    /// taken in.
    #[serde(rename = "last")]
    pub last: BTreeMap<String, RecordedUsageReading>,
}

impl Contents {
    pub const CURRENT_VERSION: u32 = 1;
}

impl Default for Contents {
    fn default() -> Self {
        Contents {
            version: Self::CURRENT_VERSION,
            pending: Vec::new(),
            last: BTreeMap::new(),
        }
    }
}

/// The same set of (id, utilization, resets at), order and repeats aside.
fn same_values(lhs: &[SyncWindow], rhs: &[SyncWindow]) -> bool {
    lhs.iter().all(|w| rhs.contains(w)) && rhs.iter().all(|w| lhs.contains(w))
}

pub struct UsageHistoryRecorder {
    // Shared with the background write, which encodes the newest contents
    // when it runs (a burst of readings is written once).
    contents: Arc<Mutex<Contents>>,
    file: StateFile<Contents>,
}

impl UsageHistoryRecorder {
    /// A saved file of another version, or one that can't be read, starts
    /// fresh.
    pub fn new(file: StateFile<Contents>) -> Self {
        let contents = file
            .load()
            .filter(|saved| saved.version == Contents::CURRENT_VERSION)
            .unwrap_or_default();
        UsageHistoryRecorder {
            contents: Arc::new(Mutex::new(contents)),
            file,
        }
    }

    /// The outbox of `<support>\cloud-usage-outbox.json` (`persist` false:
    /// memory only, for a sealed run).
    pub fn in_support(support: &Path, files: Arc<dyn SecureFiles>, persist: bool) -> Self {
        let file = if persist {
            StateFile::new(Some(support.join(FILE_NAME)), Some(files), Duration::ZERO)
        } else {
            StateFile::memory()
        };
        Self::new(file)
    }

    fn save(&self) {
        let contents = self.contents.clone();
        self.file.save(move || lock(&contents).clone());
    }

    pub fn pending_count(&self) -> usize {
        lock(&self.contents).pending.len()
    }

    /// A copy of everything kept (tests, the fixture round trip).
    pub fn contents(&self) -> Contents {
        lock(&self.contents).clone()
    }

    /// Take a reading in. Returns whether it was kept (new, or merged into
    /// a waiting one with new windows).
    pub fn record(&self, reading: RecordedUsageReading) -> bool {
        if reading.windows.is_empty() {
            return false;
        }
        let kept = {
            let mut contents = lock(&self.contents);
            let stream = reading.stream();
            if let Some(last) = contents.last.get(&stream) {
                if reading.dedupe_key() == last.dedupe_key() {
                    let merged = merge(&mut contents, reading);
                    drop(contents);
                    if merged {
                        self.save();
                    }
                    return merged;
                }
                if reading.observed_at < last.observed_at {
                    return false;
                }
                if same_values(&reading.windows, &last.windows)
                    && seconds_between(reading.observed_at, last.observed_at)
                        < REPEAT_WINDOW.as_secs_f64()
                {
                    return false;
                }
            }
            contents.last.insert(stream, reading.clone());
            contents.pending.push(reading);
            let len = contents.pending.len();
            if len > CAPACITY {
                contents.pending.drain(..len - CAPACITY);
            }
            true
        };
        self.save();
        kept
    }

    /// The oldest waiting readings, at most `limit`.
    pub fn pending(&self, limit: usize) -> Vec<RecordedUsageReading> {
        lock(&self.contents)
            .pending
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    /// The website has these: stop sending them.
    pub fn mark_sent(&self, readings: &[RecordedUsageReading]) {
        let keys: std::collections::BTreeSet<String> =
            readings.iter().map(|r| r.dedupe_key()).collect();
        self.discard(|r| keys.contains(&r.dedupe_key()));
    }

    /// Drop what waits for accounts that must not be sent (forgotten,
    /// switched off).
    pub fn discard(&self, should_drop: impl Fn(&RecordedUsageReading) -> bool) {
        let changed = {
            let mut contents = lock(&self.contents);
            let before = contents.pending.len();
            contents.pending.retain(|r| !should_drop(r));
            contents.pending.len() != before
        };
        if changed {
            self.save();
        }
    }

    /// Forget every waiting reading (sync switched off, signed out): they
    /// were kept only to be sent.
    pub fn clear(&self) {
        {
            let mut contents = lock(&self.contents);
            if contents.pending.is_empty() && contents.last.is_empty() {
                return;
            }
            *contents = Contents::default();
        }
        self.save();
    }

    /// Write now, on this thread (quitting, tests).
    pub fn save_now(&self) {
        let snapshot = lock(&self.contents).clone();
        self.file.save_now(&snapshot);
    }

    /// Waits for background writes (tests).
    pub fn flush(&self) {
        self.file.flush();
    }
}

/// Add the windows a waiting reading of the same moment lacks.
fn merge(contents: &mut Contents, reading: RecordedUsageReading) -> bool {
    let key = reading.dedupe_key();
    let Some(waiting) = contents
        .pending
        .iter_mut()
        .rev()
        .find(|r| r.dedupe_key() == key)
    else {
        return false;
    };
    let added: Vec<SyncWindow> = reading
        .windows
        .into_iter()
        .filter(|w| !waiting.windows.iter().any(|k| k.id == w.id))
        .collect();
    if added.is_empty() {
        return false;
    }
    waiting.windows.extend(added);
    true
}
