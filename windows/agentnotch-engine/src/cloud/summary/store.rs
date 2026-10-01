//! What was summarised (CL§9.2; the Mac's `SessionSummaryStore`): the
//! summaries made so far (by ledger entry: one per session and account),
//! the failures to wait out, when each run started (the hourly and daily
//! caps), and when summaries were last turned on (only sessions that ended
//! after it are summarised). Kept in `<support>\cloud-summaries.json` (v2);
//! in memory only when sealed.
//!
//! Every method takes its time explicitly: the store never reads a clock.

use super::run::Summary;
use crate::cloud::contract::{date, seconds_between, Stamp, SyncSummary};
use crate::cloud::files::{lock, StateFile};
use crate::model::IdentityId;
use crate::platform::SecureFiles;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub const FILE_NAME: &str = "cloud-summaries.json";

/// Most runs in any hour.
pub const MAX_PER_HOUR: usize = 20;
/// Most runs in any day.
pub const MAX_PER_DAY: usize = 60;
/// A session is summarised this long after it ended at the earliest.
pub const QUIET_PERIOD: Duration = Duration::from_secs(10 * 60);
/// Fewer responses than this: nothing to summarise.
pub const MINIMUM_RESPONSES: i64 = 2;
/// Summarised again once it has this many times the responses it had.
pub const REGROWTH_FACTOR: f64 = 1.5;
/// Waits after failures: doubling from 15 minutes up to a day.
pub const INITIAL_RETRY: Duration = Duration::from_secs(15 * 60);
pub const MAX_RETRY: Duration = Duration::from_secs(24 * 60 * 60);

const HOUR_S: f64 = 3600.0;
const DAY_S: f64 = 24.0 * 3600.0;

/// A summary as kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "model")]
    pub model: String,
    #[serde(rename = "generatedAt", with = "date")]
    pub generated_at: SystemTime,
    /// Responses the session had when it was summarised.
    #[serde(rename = "messageCount")]
    pub message_count: i64,
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Entry {
    /// As sent: scrubbed again (with this PC's names), whatever wrote it.
    pub fn contract(&self, known_names: &[String]) -> SyncSummary {
        SyncSummary {
            text: super::text::scrub(&self.text, known_names),
            model: self.model.clone(),
            generated_at: self.generated_at,
        }
    }
}

/// A run that gave no summary, and when the session may be tried again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    #[serde(rename = "failures")]
    pub failures: u32,
    #[serde(rename = "nextAttemptAt", with = "date")]
    pub next_attempt_at: SystemTime,
    #[serde(rename = "reason")]
    pub reason: String,
}

/// `cloud-summaries.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contents {
    /// 2: by ledger entry key, and `enabledAt`.
    #[serde(rename = "version")]
    pub version: u32,
    /// By `CloudLedgerEntry` key.
    #[serde(rename = "summaries")]
    pub summaries: BTreeMap<String, Entry>,
    #[serde(rename = "attempts")]
    pub attempts: BTreeMap<String, Attempt>,
    /// Runs started in the last day.
    #[serde(rename = "runs")]
    pub runs: Vec<Stamp>,
    /// When summaries were last turned on; `None`: never (nothing is due).
    #[serde(
        rename = "enabledAt",
        with = "date::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled_at: Option<SystemTime>,
}

impl Contents {
    pub const CURRENT_VERSION: u32 = 2;
}

impl Default for Contents {
    fn default() -> Self {
        Contents {
            version: Self::CURRENT_VERSION,
            summaries: BTreeMap::new(),
            attempts: BTreeMap::new(),
            runs: Vec::new(),
            enabled_at: None,
        }
    }
}

/// A session (one account's part of it) that may be summarised now.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The ledger entry's key.
    pub key: String,
    pub session_id: String,
    pub identity_id: IdentityId,
    pub account_key: String,
    pub transcript_path: String,
    pub message_count: i64,
    pub ended_at: SystemTime,
}

pub struct SessionSummaryStore {
    // Shared with the background write, which encodes the newest contents
    // when it runs (a burst of changes is written once).
    contents: Arc<Mutex<Contents>>,
    file: StateFile<Contents>,
}

impl SessionSummaryStore {
    /// A saved file of another version, or one that can't be read, starts
    /// fresh.
    pub fn new(file: StateFile<Contents>) -> Self {
        let contents = file
            .load()
            .filter(|saved| saved.version == Contents::CURRENT_VERSION)
            .unwrap_or_default();
        SessionSummaryStore {
            contents: Arc::new(Mutex::new(contents)),
            file,
        }
    }

    /// The store of `<support>\cloud-summaries.json` (`persist` false:
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

    pub fn summary(&self, key: &str) -> Option<Entry> {
        lock(&self.contents).summaries.get(key).cloned()
    }

    pub fn count(&self) -> usize {
        lock(&self.contents).summaries.len()
    }

    pub fn attempt(&self, key: &str) -> Option<Attempt> {
        lock(&self.contents).attempts.get(key).cloned()
    }

    /// When summaries were last turned on.
    pub fn enabled_at(&self) -> Option<SystemTime> {
        lock(&self.contents).enabled_at
    }

    /// A copy of everything kept (tests, the fixture round trip).
    pub fn contents(&self) -> Contents {
        lock(&self.contents).clone()
    }

    /// Summaries were turned on at `at`: sessions that ended before it are
    /// never summarised.
    pub fn note_enabled(&self, at: SystemTime) {
        lock(&self.contents).enabled_at = Some(at);
        self.save();
    }

    fn runs_within(&self, now: SystemTime, seconds: f64) -> usize {
        lock(&self.contents)
            .runs
            .iter()
            .filter(|run| seconds_between(now, run.0) < seconds && run.0 <= now)
            .count()
    }

    /// Runs started in the hour before `now`.
    pub fn runs_in_last_hour(&self, now: SystemTime) -> usize {
        self.runs_within(now, HOUR_S)
    }

    /// Runs started in the day before `now`.
    pub fn runs_in_last_day(&self, now: SystemTime) -> usize {
        self.runs_within(now, DAY_S)
    }

    pub fn note_run(&self, at: SystemTime) {
        {
            let mut contents = lock(&self.contents);
            contents
                .runs
                .retain(|run| seconds_between(at, run.0) < DAY_S);
            contents.runs.push(Stamp(at));
        }
        self.save();
    }

    pub fn record(&self, key: &str, summary: &Summary, message_count: i64, at: SystemTime) {
        {
            let mut contents = lock(&self.contents);
            contents.summaries.insert(
                key.to_owned(),
                Entry {
                    text: summary.text.clone(),
                    model: summary.model.clone(),
                    generated_at: at,
                    message_count,
                    cost_usd: summary.cost_usd,
                },
            );
            contents.attempts.remove(key);
        }
        self.save();
    }

    /// Delete the summaries `is_kept` says no to (by ledger entry key).
    /// Returns how many went.
    pub fn remove_all(&self, is_kept: impl Fn(&str, &Entry) -> bool) -> usize {
        let snapshot = lock(&self.contents).summaries.clone();
        // Decided outside the lock (the caller may look at other stores).
        let doomed: Vec<(String, Entry)> = snapshot
            .into_iter()
            .filter(|(key, entry)| !is_kept(key, entry))
            .collect();
        if doomed.is_empty() {
            return 0;
        }
        let removed = {
            let mut contents = lock(&self.contents);
            let mut removed = 0;
            // One replaced meanwhile is a new summary, not the one judged.
            for (key, entry) in doomed {
                if contents.summaries.get(&key) == Some(&entry) {
                    contents.summaries.remove(&key);
                    removed += 1;
                }
            }
            removed
        };
        if removed > 0 {
            self.save();
        }
        removed
    }

    /// A run that didn't give a summary: wait before trying this session
    /// again (at least `minimum_wait`).
    pub fn record_failure(&self, key: &str, reason: &str, at: SystemTime, minimum_wait: Duration) {
        {
            let mut contents = lock(&self.contents);
            let failures = contents.attempts.get(key).map_or(0, |a| a.failures) + 1;
            let backoff = INITIAL_RETRY.as_secs_f64() * 2f64.powi(failures.min(11) as i32 - 1);
            let wait = minimum_wait
                .as_secs_f64()
                .max(backoff.min(MAX_RETRY.as_secs_f64()));
            contents.attempts.insert(
                key.to_owned(),
                Attempt {
                    failures,
                    next_attempt_at: at + Duration::from_secs_f64(wait),
                    reason: reason.to_owned(),
                },
            );
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

    /// Whether a session is due a summary: ended at least [`QUIET_PERIOD`]
    /// ago and not before summaries were turned on (`since`), at least
    /// [`MINIMUM_RESPONSES`], not summarised yet (or grown past
    /// [`REGROWTH_FACTOR`] times what it had then), and not waiting out a
    /// failure. Pure.
    pub fn is_due(
        ended_at: Option<SystemTime>,
        message_count: i64,
        existing: Option<&Entry>,
        attempt: Option<&Attempt>,
        since: Option<SystemTime>,
        now: SystemTime,
    ) -> bool {
        let (Some(ended_at), Some(since)) = (ended_at, since) else {
            return false;
        };
        if ended_at < since
            || seconds_between(now, ended_at) < QUIET_PERIOD.as_secs_f64()
            || message_count < MINIMUM_RESPONSES
        {
            return false;
        }
        if attempt.is_some_and(|a| a.next_attempt_at > now) {
            return false;
        }
        match existing {
            Some(existing) => {
                message_count as f64 > existing.message_count as f64 * REGROWTH_FACTOR
            }
            None => true,
        }
    }

    /// The next session to summarise: of the due ones, the one that ended
    /// last (ties: the smaller key). `None` when none is, or the hour's or
    /// the day's runs are used up.
    pub fn next_candidate(&self, candidates: &[Candidate], now: SystemTime) -> Option<Candidate> {
        if self.runs_in_last_hour(now) >= MAX_PER_HOUR || self.runs_in_last_day(now) >= MAX_PER_DAY
        {
            return None;
        }
        let (summaries, attempts, since) = {
            let contents = lock(&self.contents);
            (
                contents.summaries.clone(),
                contents.attempts.clone(),
                contents.enabled_at,
            )
        };
        candidates
            .iter()
            .filter(|c| {
                Self::is_due(
                    Some(c.ended_at),
                    c.message_count,
                    summaries.get(&c.key),
                    attempts.get(&c.key),
                    since,
                    now,
                )
            })
            .max_by(|a, b| a.ended_at.cmp(&b.ended_at).then_with(|| b.key.cmp(&a.key)))
            .cloned()
    }
}
