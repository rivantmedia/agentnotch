//! The store's transcript syncs, chat and interrupts (wp5-10). Hook events
//! only ask for a sync (`schedule_sync`); the jobs, the deltas and the chat
//! come with wp5-10. The Interrupt input is here because the hook tests need
//! it; wp5-10 completes the watcher side.

use crate::model::{Phase, SessionId};
use crate::runtime_types::TranscriptDelta;
use crate::sessions::phase;
use crate::sessions::session::SubagentState;
use crate::sessions::store::SessionStore;
use std::time::{Duration, SystemTime};

/// Hook events that change the transcript are read after this quiet moment,
/// so a burst costs one read.
pub const SYNC_DEBOUNCE: Duration = Duration::from_millis(100);

impl SessionStore {
    /// Asks for the session's transcript to be read again soon (debounced).
    pub(super) fn schedule_sync(&mut self, id: &SessionId, now: SystemTime) {
        self.sync_due
            .entry(id.clone())
            .or_insert(now + SYNC_DEBOUNCE);
    }

    /// New lines of a transcript were read (wp5-10); for now nothing is done.
    pub(super) fn apply_transcript_synced(&mut self, delta: TranscriptDelta, now: SystemTime) {
        let _ = (delta, now);
    }

    /// The transcript watcher saw Esc: the main turn stopped (HS§5.8). Ignored
    /// when a newer turn started since (the interrupt, seen late, is about
    /// the old one); the background agents a wait is on keep running.
    pub(super) fn apply_interrupt(&mut self, id: &SessionId, at: SystemTime, now: SystemTime) {
        let Some(mut session) = self.sessions.remove(id) else {
            return;
        };
        if session.turn_started_at.is_some_and(|started| started > at) {
            self.sessions.insert(id.clone(), session);
            return;
        }
        session.subagent_state = SubagentState::new();
        session.tool_tracker.end_main_turn();
        session.completion_pending_since = None;
        if !session.has_failed_turn() {
            session.set_needs_input(None, now);
        }
        // Calls still running were interrupted; those waiting for approval are
        // settled with the requests below.
        session.chat.interrupt_running();
        if phase::is_waiting_for_approval(&session.phase) {
            self.drop_main_approvals(&mut session, Phase::Idle, now, false);
        } else if phase::can_transition(&session.phase, &Phase::Idle) {
            session.phase = Phase::Idle;
        }
        // Esc stops the turn, not the agents a background wait is on.
        self.settle_background_wait(&mut session, now);
        self.sessions.insert(id.clone(), session);
    }
}
