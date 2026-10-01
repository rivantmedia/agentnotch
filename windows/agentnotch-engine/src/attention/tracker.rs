//! AttentionTracker (HS§7.1): turns the session list into per-session
//! transitions ("session X now needs input", "session Y is ready for
//! review") for banners, sounds and auto-open.
//!
//! What is not news:
//! - anything seen before the launch baseline closes: that is once every
//!   account's session registry has been read (plus a short settle), so the
//!   sessions of the second, third, ... account found at launch don't all
//!   alert (each registry arrives as its own snapshot);
//! - a completion from before this launch (restored from
//!   `review-state.json`, or inferred from a transcript that finished while
//!   the app was down);
//! - a quiet completion: a /loop or cron tick, or a turn that scheduled a
//!   wake-up. They stay in the review queue; the final turn is announced.
//!   (A turn that ended waiting for background agents isn't a completion at
//!   all: it stays working until they are done.)
//!
//! Owner: WP5. Pure: the caller passes the time and the store wires it
//! (wp5-9).

use super::news;
use crate::model::{AttentionTransition, SessionId, SessionState, SessionView};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// After the registries were first read, changes are still recorded
/// silently this long (late snapshots, first transcript syncs).
pub const SETTLE_INTERVAL: Duration = Duration::from_secs(2);
/// The baseline closes by itself this long after `start`, even if the
/// registry scanner never reports its first pass.
pub const MAX_BASELINE_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct AttentionTracker {
    last: HashMap<SessionId, SessionState>,
    /// Completions from before this are not news (the hub uses it too).
    launched_at: SystemTime,
    started_at: Option<SystemTime>,
    baseline_ends_at: Option<SystemTime>,
}

impl AttentionTracker {
    pub fn new(launched_at: SystemTime) -> AttentionTracker {
        AttentionTracker {
            last: HashMap::new(),
            launched_at,
            started_at: None,
            baseline_ends_at: None,
        }
    }

    pub fn launched_at(&self) -> SystemTime {
        self.launched_at
    }

    /// The tracker begins following the session list. Idempotent.
    pub fn start(&mut self, now: SystemTime) {
        if self.started_at.is_none() {
            self.started_at = Some(now);
        }
    }

    /// The hub stopped. A later `start` takes a new launch baseline.
    pub fn stop(&mut self) {
        self.started_at = None;
        self.baseline_ends_at = None;
    }

    /// Every account's session registry has been read once.
    pub fn initial_scan_completed(&mut self, now: SystemTime) {
        if self.baseline_ends_at.is_none() {
            self.baseline_ends_at = Some(now + SETTLE_INTERVAL);
        }
    }

    /// The state of a session as last seen by the tracker.
    pub fn attention(&self, id: &SessionId) -> Option<&SessionState> {
        self.last.get(id)
    }

    /// Whether changes are still recorded as the launch baseline.
    pub fn is_in_baseline(&self, now: SystemTime) -> bool {
        if let Some(ends) = self.baseline_ends_at {
            return now < ends;
        }
        match self.started_at {
            Some(started) => now < started + MAX_BASELINE_INTERVAL,
            None => true,
        }
    }

    /// Records the session list and returns the news in it: every change,
    /// except a completion `news::is_news` rejects. Sessions that went away
    /// are forgotten (they come back as new). Nothing is returned while the
    /// baseline is open.
    pub fn update(&mut self, views: &[SessionView], now: SystemTime) -> Vec<AttentionTransition> {
        let mut next = HashMap::with_capacity(views.len());
        let mut changes = Vec::new();
        for view in views {
            let previous = self.last.get(&view.id);
            if previous != Some(&view.state) {
                changes.push(AttentionTransition {
                    session: view.clone(),
                    from: previous.cloned(),
                    to: view.state.clone(),
                });
            }
            next.insert(view.id.clone(), view.state.clone());
        }
        self.last = next;

        if self.is_in_baseline(now) {
            return Vec::new();
        }
        changes.retain(|change| news::is_news(change, Some(self.launched_at)));
        changes
    }
}
