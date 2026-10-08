//! The store's review queue and attention transitions (HS§5.13): marks, the
//! persistence of `review-state.json`, the restore of a session's record,
//! account removal, and the tracker that turns the session list into
//! transitions. The review file itself is the hub's to read and write: the
//! store takes its bytes at the start ([`SessionStore::load_review`]) and
//! asks for them to be written as a `Job::Persist` on a `Tick` once a write
//! is due (at once for a completion or a failure, after a second for a
//! review mark) and every 30 seconds as the heartbeat.

use crate::model::{NeedsInputReason, SessionId};
use crate::review::{ReviewSnapshot, ReviewStore, StopFailure};
use crate::runtime_types::{AccountsChanged, Job, PersistFile, ReviewAction};
use crate::sessions::session::Session;
use crate::sessions::store::{clear_failure, SessionStore};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// What a session keeps in the review file.
pub fn review_snapshot(session: &Session) -> ReviewSnapshot {
    ReviewSnapshot::capture(
        session.completed_at,
        session.reviewed_at,
        session.last_assistant_message.clone(),
        session.background_wait_since,
        &session.background_agent_types,
        session.stop_error.clone().map(|text| StopFailure {
            text,
            code: session.stop_error_code.clone(),
            at: session.failed_at,
        }),
        session.has_failed_turn(),
    )
}

impl SessionStore {
    // ---- the hub's calls ----

    /// The review file's bytes at the start (`None`: no file): records of
    /// the previous run are restored as its sessions are found again, the
    /// previous run's last heartbeat tells which turns finished while the
    /// app was down, and from now on the store asks for the file to be
    /// written. Also the launch: the attention baseline begins.
    pub fn load_review(&mut self, bytes: Option<&[u8]>, now: SystemTime) {
        self.review = ReviewStore::load(bytes, now);
        self.previous_run_alive_at = self.review.previous_run_alive_at();
        if self.started_at.is_none() {
            self.started_at = Some(now);
            self.tracker.set_launched_at(now);
        }
        self.review_loaded_at = Some(now);
        self.tracker.start(now);
    }

    /// Every account's session registry has been read once: the launch
    /// baseline closes after a short settle.
    pub fn initial_scan_completed(&mut self, now: SystemTime) {
        self.tracker.initial_scan_completed(now);
    }

    /// The review file's bytes now, whatever its debounce says (the hub's
    /// stop writes them at once: the process may end right after). `None`
    /// for a store never given the file. Added by WP7.
    pub fn review_file_now(&mut self, now: SystemTime) -> Option<Vec<u8>> {
        self.review_loaded_at?;
        self.review_persist_due = None;
        Some(self.review.encode(now))
    }

    /// The earliest moment the review file or the attention baseline needs
    /// the clock (only once the file was handed over).
    pub(super) fn review_deadline(&self) -> Option<SystemTime> {
        let loaded_at = self.review_loaded_at?;
        [
            self.review_persist_due,
            Some(self.review.next_heartbeat(loaded_at)),
            self.tracker.baseline_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    // ---- persistence ----

    /// The review-file fields of every session, before an input is applied.
    pub(super) fn review_snapshots(&self) -> BTreeMap<SessionId, ReviewSnapshot> {
        self.sessions
            .iter()
            .map(|(id, session)| (id.clone(), review_snapshot(session)))
            .collect()
    }

    /// Keeps the record of every session whose review fields changed, and
    /// notes when the file is due. A session first seen in this input starts
    /// from what its restored record holds.
    pub(super) fn persist_review_changes(
        &mut self,
        mut before: BTreeMap<SessionId, ReviewSnapshot>,
        now: SystemTime,
    ) {
        let mut delay: Option<Duration> = None;
        for (id, session) in &self.sessions {
            let after = review_snapshot(session);
            let previous = before.remove(id).unwrap_or_else(|| {
                self.review
                    .record(id.as_str())
                    .map(|item| ReviewSnapshot {
                        completed_at: item.completed_at,
                        reviewed_at: item.reviewed_at,
                        last_assistant_message: item.last_assistant_message.clone(),
                        stop_error: item.stop_error.clone(),
                        stop_error_code: item.stop_error_code.clone(),
                        failed_at: item.failed_at,
                        background_wait_since: item.background_wait_since,
                        background_agent_types: item.background_agent_types.clone(),
                    })
                    .unwrap_or_default()
            });
            if let Some(wait) = self
                .review
                .persist_if_changed(id.as_str(), &previous, &after, now)
            {
                delay = Some(delay.map_or(wait, |known| known.min(wait)));
            }
        }
        self.note_review_write(delay, now);
    }

    /// A write of the review file is due after `delay` (the earliest wins;
    /// zero is at once).
    fn note_review_write(&mut self, delay: Option<Duration>, now: SystemTime) {
        let Some(delay) = delay else {
            return;
        };
        let due = now + delay;
        self.review_persist_due = Some(self.review_persist_due.map_or(due, |known| known.min(due)));
        self.effects.persist_review = Some(
            self.effects
                .persist_review
                .map_or(delay, |known| known.min(delay)),
        );
    }

    /// On a tick: the review file's bytes, when a write is due or the 30 s
    /// heartbeat is.
    pub(super) fn flush_review(&mut self, now: SystemTime) {
        let Some(loaded_at) = self.review_loaded_at else {
            return;
        };
        let write_due = self.review_persist_due.is_some_and(|due| now >= due);
        let beat_due = now >= self.review.next_heartbeat(loaded_at);
        if !write_due && !beat_due {
            return;
        }
        self.review_persist_due = None;
        let bytes = self.review.encode(now);
        self.effects.jobs.push(Job::Persist {
            file: PersistFile::Review,
            bytes,
        });
    }

    // ---- actions ----

    /// A review mark, a dismissed failure, a reset or hooks turned off.
    pub(super) fn apply_review(&mut self, action: ReviewAction, now: SystemTime) {
        match action {
            ReviewAction::MarkReviewed { session, at } => {
                // The moment of the click, not of processing: a turn that
                // completes in between is still unreviewed.
                self.mark_reviewed(&session, at);
            }
            ReviewAction::MarkViewed {
                session,
                completed_at,
            } => {
                if let Some(record) = self.sessions.get_mut(&session) {
                    if record.completed_at == Some(completed_at)
                        && record
                            .reviewed_at
                            .is_none_or(|reviewed| reviewed < completed_at)
                    {
                        record.reviewed_at = Some(completed_at);
                    }
                }
            }
            ReviewAction::MarkAll { sessions, at } => {
                for id in sessions {
                    if self
                        .sessions
                        .get(&id)
                        .is_some_and(Session::is_ready_for_review)
                    {
                        self.mark_reviewed(&id, at);
                    }
                }
            }
            ReviewAction::DismissFailure(id) => self.dismiss_failure(&id, now),
            ReviewAction::Reset => {
                // Advanced > Review queue: every finished session is reviewed.
                let ready: Vec<SessionId> = self
                    .sessions
                    .iter()
                    .filter(|(_, session)| session.is_ready_for_review())
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ready {
                    self.mark_reviewed(&id, now);
                }
            }
            ReviewAction::HooksTurnedOff => {
                for session in self.sessions.values_mut() {
                    session.last_hook_event_at = None;
                }
            }
        }
    }

    /// Reviewed as of `at` (the click), however much later this runs; an
    /// older mark never takes a review back.
    pub(super) fn mark_reviewed(&mut self, id: &SessionId, at: SystemTime) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.reviewed_at = Some(session.reviewed_at.map_or(at, |known| known.max(at)));
        }
    }

    /// A failed turn the user dismissed: no longer failed (nor restored as
    /// failed after a relaunch, since the review record drops it), until the
    /// next StopFailure. A failure newer than the click (`now`, the click
    /// being applied in the order it arrived) stays.
    fn dismiss_failure(&mut self, id: &SessionId, now: SystemTime) {
        let Some(session) = self.sessions.get_mut(id) else {
            return;
        };
        if !session.has_failed_turn() || session.failed_at.is_some_and(|failed| failed > now) {
            return;
        }
        clear_failure(session);
        session.set_needs_input(None, now);
        session.reviewed_at = Some(session.reviewed_at.map_or(now, |known| known.max(now)));
    }

    /// Accounts were removed: their sessions go (the Mac's
    /// `dropAccountSessions`). Their review records stay until pruned, as on
    /// the Mac.
    pub(super) fn apply_accounts_changed(&mut self, change: AccountsChanged, now: SystemTime) {
        if change.removed_folders.is_empty() {
            return;
        }
        let gone: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .account
                    .as_ref()
                    .is_some_and(|account| change.removed_folders.contains(account))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.remove_session(&id, now);
        }
    }

    // ---- restore ----

    /// A new session restores what the review file kept of it: completion,
    /// review, last reply, a background wait, a failed turn.
    pub(super) fn restore_from_review(&self, session: &mut Session, now: SystemTime) {
        let Some(record) = self.review.record(session.id.as_str()) else {
            return;
        };
        session.completed_at = record.completed_at;
        session.reviewed_at = record.reviewed_at;
        session.last_assistant_message = record.last_assistant_message.clone();
        // Still waiting on background agents when we last looked; the
        // registry says whether they still run.
        session.background_wait_since = record.background_wait_since;
        session.background_agent_types = record.background_agent_types.clone();
        session.background_agent_count = session.background_agent_types.len() as u32;
        if let Some(error) = &record.stop_error {
            // The turn failed while we last looked; the first transcript sync
            // clears it if the user has moved on since.
            session.stop_error = Some(error.clone());
            session.stop_error_code = record.stop_error_code.clone();
            session.failed_at = record.failed_at;
            session.set_needs_input(
                Some(NeedsInputReason::Error {
                    text: error.clone(),
                    code: record.stop_error_code.clone(),
                }),
                record.failed_at.unwrap_or(now),
            );
        }
    }
}
