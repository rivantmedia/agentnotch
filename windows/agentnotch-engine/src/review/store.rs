//! The review queue ("Claude finished, you haven't looked yet") and failed
//! turns, kept in `review-state.json` so they survive restarts: when a
//! session is rediscovered (hook, registry, status line) its completion,
//! review and failure are restored (HS§5.13).
//!
//! It also keeps a heartbeat, `lastAliveAt`, refreshed while the app runs: a
//! turn whose reply is newer than the previous run's heartbeat finished
//! while nobody was watching (`inferCompletion`).
//!
//! Pure, like the rest of the pipeline: the store holds the records and says
//! *when* a write is due (`update` returns the delay, `next_heartbeat` the
//! time); the caller writes the bytes `encode` makes through a `Persist` job
//! (atomic, private). Completions and failures are written at once (a crash
//! right after one must not lose it); review marks after a 1 s debounce.
//! Entries untouched for 7 days are pruned. The file of Superpowered Vibe
//! Notch (a bare `{sessionId: record}` object) is read as well.
//!
//! Owner: WP5. Port of `ReviewStateStore`.

use crate::core::time::EpochSeconds;
use crate::model::ReviewItem;
use crate::persist::review::{
    file_date, PersistedReviewRecord, ReviewStateFile,
    MAX_MESSAGE_LENGTH as FILE_MAX_MESSAGE_LENGTH, VERSION,
};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// Records untouched for longer than this are dropped.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Longest preview kept, in characters.
pub const MAX_MESSAGE_LENGTH: usize = FILE_MAX_MESSAGE_LENGTH;
/// How often the running app refreshes `lastAliveAt`.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// How long a review mark waits before it is written.
pub const WRITE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ReviewStore {
    records: BTreeMap<String, ReviewItem>,
    last_alive_at: Option<SystemTime>,
    /// The previous run's last heartbeat, read at launch (`None` on a first
    /// run, or from a file without one).
    previous_run_alive_at: Option<SystemTime>,
    /// When the bytes were last made (every write carries the heartbeat).
    last_beat_at: Option<SystemTime>,
}

impl ReviewStore {
    /// An empty store (a first run, or a sealed one that never reads a file).
    pub fn empty() -> ReviewStore {
        ReviewStore {
            records: BTreeMap::new(),
            last_alive_at: None,
            previous_run_alive_at: None,
            last_beat_at: None,
        }
    }

    /// The store of the file's bytes (`None`: no file), with records
    /// untouched for 7 days left out. Unreadable bytes are an empty store.
    pub fn load(bytes: Option<&[u8]>, now: SystemTime) -> ReviewStore {
        let mut store = ReviewStore::empty();
        let Some(file) = bytes.and_then(ReviewStateFile::parse) else {
            return store;
        };
        store.records = file
            .sessions
            .iter()
            .filter_map(|(id, record)| Some((id.clone(), record.to_model()?)))
            .collect();
        store.prune(now);
        store.last_alive_at = file_date(file.last_alive_at);
        store.previous_run_alive_at = store.last_alive_at;
        store
    }

    /// The last heartbeat: the previous run's until this run writes its own.
    pub fn last_alive_at(&self) -> Option<SystemTime> {
        self.last_alive_at
    }

    /// The previous run's last heartbeat, fixed at launch.
    pub fn previous_run_alive_at(&self) -> Option<SystemTime> {
        self.previous_run_alive_at
    }

    pub fn record(&self, session_id: &str) -> Option<&ReviewItem> {
        self.records.get(session_id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Stores the session's record if its content changed. `Some(delay)`:
    /// a write is due after it (`ZERO` for `urgent`: completions and
    /// failures; a second for review marks). `None`: nothing changed.
    pub fn update(
        &mut self,
        session_id: &str,
        mut item: ReviewItem,
        urgent: bool,
    ) -> Option<Duration> {
        if let Some(message) = item.last_assistant_message.as_mut() {
            if let Some((end, _)) = message.char_indices().nth(MAX_MESSAGE_LENGTH) {
                message.truncate(end);
            }
        }
        match self.records.get(session_id) {
            Some(existing) if existing.has_same_content(&item) => return None,
            // Nothing to keep and nothing kept: not a change (the Mac wrote
            // the file again for it; the bytes are the same).
            None if item.is_empty() => return None,
            _ => {}
        }
        if item.is_empty() {
            self.records.remove(session_id);
        } else {
            self.records.insert(session_id.to_owned(), item);
        }
        Some(if urgent { Duration::ZERO } else { WRITE_DELAY })
    }

    /// Stores the record of a session's snapshot (`persistReviewIfChanged`):
    /// nothing when it equals `before`, a write at once when the completion
    /// or the failure changed, after the debounce otherwise.
    pub fn persist_if_changed(
        &mut self,
        session_id: &str,
        before: &ReviewSnapshot,
        after: &ReviewSnapshot,
        now: SystemTime,
    ) -> Option<Duration> {
        if after == before {
            return None;
        }
        let urgent =
            after.completed_at != before.completed_at || after.stop_error != before.stop_error;
        self.update(session_id, after.to_item(now), urgent)
    }

    /// Forgets a session (its account was dropped). `Some(delay)` when it
    /// had a record.
    pub fn remove(&mut self, session_id: &str) -> Option<Duration> {
        self.records.remove(session_id).map(|_| WRITE_DELAY)
    }

    /// Forgets every record (the heartbeat stays).
    pub fn reset(&mut self) -> Option<Duration> {
        if self.records.is_empty() {
            return None;
        }
        self.records.clear();
        Some(WRITE_DELAY)
    }

    /// When the next heartbeat write is due: at once when none was made yet
    /// (the Mac starts its timer when the store starts), else 30 s after the
    /// last write.
    pub fn next_heartbeat(&self, now: SystemTime) -> SystemTime {
        match self.last_beat_at {
            Some(last) => last + HEARTBEAT_INTERVAL,
            None => now,
        }
    }

    /// The file's bytes as of `now`: records untouched for 7 days are
    /// pruned, `lastAliveAt` is `now`. Compact, sorted keys, epoch seconds.
    /// Making them counts as the heartbeat.
    pub fn encode(&mut self, now: SystemTime) -> Vec<u8> {
        self.prune(now);
        self.last_alive_at = Some(now);
        self.last_beat_at = Some(now);
        ReviewStateFile {
            version: VERSION,
            last_alive_at: Some(EpochSeconds::from_time(now)),
            sessions: self
                .records
                .iter()
                .map(|(id, item)| (id.clone(), PersistedReviewRecord::from_model(item)))
                .collect(),
        }
        .encode()
    }

    fn prune(&mut self, now: SystemTime) {
        let cutoff = now.checked_sub(RETENTION);
        if let Some(cutoff) = cutoff {
            self.records.retain(|_, item| item.updated_at >= cutoff);
        }
    }
}

/// The fields of a session that `review-state.json` keeps. Failures are kept
/// only while they still count: the caller says whether the session has a
/// failed turn, so a failure the user answered or a later turn replaced is
/// not restored at the next launch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReviewSnapshot {
    pub completed_at: Option<SystemTime>,
    pub reviewed_at: Option<SystemTime>,
    pub last_assistant_message: Option<String>,
    pub stop_error: Option<String>,
    pub stop_error_code: Option<String>,
    pub failed_at: Option<SystemTime>,
    pub background_wait_since: Option<SystemTime>,
    pub background_agent_types: Vec<String>,
}

/// A session's last StopFailure, as the session holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct StopFailure {
    pub text: String,
    pub code: Option<String>,
    pub at: Option<SystemTime>,
}

impl ReviewSnapshot {
    /// `failure` is kept only when `has_failed_turn`; the background agent
    /// types only while a wait stands.
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        completed_at: Option<SystemTime>,
        reviewed_at: Option<SystemTime>,
        last_assistant_message: Option<String>,
        background_wait_since: Option<SystemTime>,
        background_agent_types: &[String],
        failure: Option<StopFailure>,
        has_failed_turn: bool,
    ) -> ReviewSnapshot {
        let failure = failure.filter(|_| has_failed_turn);
        ReviewSnapshot {
            completed_at,
            reviewed_at,
            last_assistant_message,
            stop_error: failure.as_ref().map(|f| f.text.clone()),
            stop_error_code: failure.as_ref().and_then(|f| f.code.clone()),
            failed_at: failure.and_then(|f| f.at),
            background_wait_since,
            background_agent_types: if background_wait_since.is_some() {
                background_agent_types.to_vec()
            } else {
                Vec::new()
            },
        }
    }

    pub fn to_item(&self, updated_at: SystemTime) -> ReviewItem {
        ReviewItem {
            completed_at: self.completed_at,
            reviewed_at: self.reviewed_at,
            last_assistant_message: self.last_assistant_message.clone(),
            stop_error: self.stop_error.clone(),
            stop_error_code: self.stop_error_code.clone(),
            failed_at: self.failed_at,
            background_wait_since: self.background_wait_since,
            background_agent_types: self.background_agent_types.clone(),
            updated_at,
        }
    }
}
