//! The store's review queue and attention transitions (wp5-9): marks,
//! persistence of the review file, the restore of a session's record and
//! account removal. The input arms call these; they do nothing until wp5-9.

use crate::runtime_types::{AccountsChanged, ReviewAction};
use crate::sessions::session::Session;
use crate::sessions::store::SessionStore;
use std::time::SystemTime;

impl SessionStore {
    /// A review mark, a dismissed failure, a reset or hooks turned off
    /// (wp5-9).
    pub(super) fn apply_review(&mut self, action: ReviewAction, now: SystemTime) {
        let _ = (action, now);
    }

    /// Accounts were removed: their sessions go (wp5-9).
    pub(super) fn apply_accounts_changed(&mut self, change: AccountsChanged, now: SystemTime) {
        let _ = (change, now);
    }

    /// A new session restores what the review file kept of it: completion,
    /// review, last reply, a background wait, a failed turn (wp5-9).
    pub(super) fn restore_from_review(&self, session: &mut Session, now: SystemTime) {
        let _ = (session, now);
    }
}
