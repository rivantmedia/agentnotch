//! AttentionNews (HS§7): the one rule for what a change in a session's
//! state announces, used by both streams built on it: the hub's transitions
//! (chime, peek, auto-open, "viewed") and the tracker's (banners). Both
//! share the tracker's launch baseline (`AttentionTracker::is_in_baseline`):
//! nothing is news until every account's registry has been read, and after
//! that a session seen for the first time is news like any other change.
//!
//! - resolved: it needed input or waited for review, and now does something
//!   else, or it went away while it did;
//! - needs input: it didn't need input (or wasn't known), or now needs it
//!   for another reason;
//! - ready for review: it wasn't ready for review, the completion isn't
//!   quiet, and it didn't finish before this launch (restored from
//!   `review-state.json`, or inferred from a transcript that ended while the
//!   app was down).
//!
//! Owner: WP5. Pure.

use crate::model::{AttentionTransition, Bucket, SessionState};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NewsKind {
    NeedsInput,
    ReadyForReview,
    Resolved,
}

/// What `from` -> `to` announces, in order (resolved first, so "the wait is
/// over" is never implied). `to == None` is a session that went away.
/// A failed turn counts as needing input, with its error as the reason.
pub fn kinds_between(
    from: Option<&SessionState>,
    to: Option<&SessionState>,
    is_quiet_completion: bool,
    completed_at: Option<SystemTime>,
    launched_at: Option<SystemTime>,
) -> Vec<NewsKind> {
    let mut kinds = Vec::new();
    if let Some(from) = from {
        let was = from.bucket();
        if matches!(was, Bucket::NeedsYou | Bucket::ReadyForReview)
            && to.map(SessionState::bucket) != Some(was)
        {
            kinds.push(NewsKind::Resolved);
        }
    }
    let Some(to) = to else {
        return kinds;
    };
    if let Some(reason) = to.reason() {
        match from.and_then(SessionState::reason) {
            Some(was) if was == reason => {}
            _ => kinds.push(NewsKind::NeedsInput),
        }
    } else if *to == SessionState::ReadyForReview && from != Some(&SessionState::ReadyForReview) {
        let before_launch = matches!((launched_at, completed_at), (Some(l), Some(c)) if c < l);
        if !is_quiet_completion && !before_launch {
            kinds.push(NewsKind::ReadyForReview);
        }
    }
    kinds
}

/// What a transition announces. `quiet` is the session's completion being
/// quiet (a /loop tick, a turn that left Claude waiting to be woken); the
/// completion time is the session's own.
pub fn kinds(
    transition: &AttentionTransition,
    launched_at: Option<SystemTime>,
    quiet: bool,
) -> Vec<NewsKind> {
    kinds_between(
        transition.from.as_ref(),
        Some(&transition.to),
        quiet,
        transition.session.completed_at,
        launched_at,
    )
}

/// Every change is passed on (banners are withdrawn on any of them), except
/// a completion that isn't news: from before this launch, or quiet.
pub fn is_news(transition: &AttentionTransition, launched_at: Option<SystemTime>) -> bool {
    if !transition.became_ready_for_review() {
        return true;
    }
    kinds(transition, launched_at, transition.session.completion_quiet)
        .contains(&NewsKind::ReadyForReview)
}
