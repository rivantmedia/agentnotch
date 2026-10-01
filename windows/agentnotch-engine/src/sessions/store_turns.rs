//! The store's turn completion, background waits, registry reconciliation and
//! periodic check (SessionStore.swift's Turn completion, Background wait,
//! Session Registry and recheck sections).
//!
//! wp5-7 holds what hook events need: confirming a Stop as the end of its
//! turn and ending a wait on background agents. The registry, the Desktop
//! answer and the periodic check are wp5-8's; their input arms call the
//! methods at the end of this file, which do nothing until then.

use crate::model::{IdentityId, RegistrySnapshot, SessionId};
use crate::sessions::background::{self, WaitDecision};
use crate::sessions::completion::{self, Decision};
use crate::sessions::session::Session;
use crate::sessions::store::SessionStore;
use std::time::SystemTime;

impl SessionStore {
    // ---- turn completion ----

    /// Confirms a pending Stop as the end of the turn once the session
    /// registry agrees (see `completion::decide`).
    pub(super) fn settle_pending_completion(&self, session: &mut Session, now: SystemTime) {
        let Some(stop_at) = session.completion_pending_since else {
            return;
        };
        let decision = completion::decide(
            stop_at,
            session.turn_started_at,
            session.registry_status.as_deref(),
            session.registry_status_changed_at,
            now,
            self.completion_timing,
        );
        if decision == Decision::Confirm {
            Self::confirm_pending_completion(session);
        }
    }

    /// The pending Stop was the end of the turn.
    pub(super) fn confirm_pending_completion(session: &mut Session) {
        if let Some(stop_at) = session.completion_pending_since.take() {
            session.completed_at = Some(stop_at);
        }
    }

    // ---- background wait ----

    /// Ends the wait of a turn that left background agents running once none
    /// is left, when Claude wasn't woken to say so (see `background::decide`).
    /// While it keeps waiting, the clock alone can change the answer: the
    /// store's next deadline says when.
    pub(super) fn settle_background_wait(&self, session: &mut Session, now: SystemTime) {
        let Some(since) = session.background_wait_since else {
            return;
        };
        let decision = background::decide(
            since,
            session.registry_status.as_deref(),
            session.registry_status_changed_at,
            session.last_hook_event_at,
            now,
            self.wait_timing,
        );
        if let WaitDecision::End { at } = decision {
            Self::end_background_wait(session, at);
        }
    }

    /// The agents are gone without waking Claude (stopped, or their
    /// notification never came): the turn's work was done at `at`.
    pub(super) fn end_background_wait(session: &mut Session, at: SystemTime) {
        session.background_wait_since = None;
        session.background_task_count = session
            .background_task_count
            .saturating_sub(session.background_agent_count);
        session.background_agent_count = 0;
        session.background_agent_types.clear();
        session.known_waking_agents = 0;
        // The work is done now, whatever became of the turn that waited: its
        // Stop may never have been confirmed (the agents kept the registry
        // busy), or a later turn was interrupted.
        session.completion_pending_since = None;
        session.completed_at = Some(session.completed_at.map_or(at, |done| done.max(at)));
    }

    // ---- wp5-8 ----

    /// A snapshot of one `sessions` folder (wp5-8): entries create, reconcile
    /// and end sessions; for now nothing is done.
    pub(super) fn apply_registry(&mut self, snapshot: RegistrySnapshot, now: SystemTime) {
        let _ = (snapshot, now);
    }

    /// Claude Desktop's own record named the identity of a hosted session
    /// (wp5-8); for now nothing is done.
    pub(super) fn apply_hosted(
        &mut self,
        session: &SessionId,
        identity: Option<IdentityId>,
        now: SystemTime,
    ) {
        let _ = (session, identity, now);
    }

    /// The periodic check (wp5-8): ended, dead and pid-less sessions go;
    /// completion and wait deadlines settle. For now nothing is done.
    pub(super) fn tick(&mut self, now: SystemTime) {
        let _ = now;
    }
}
