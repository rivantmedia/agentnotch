//! What the notch does when a Claude session starts needing you or finishes
//! work to review (HS§7, UI§3.8): chime, open the sessions panel by itself,
//! or peek the notch. A port of the Mac's `ClaudeAttentionPolicy`, pure so the
//! whole matrix is tested; `control::reactions` turns its decisions into the
//! hub's `Reactions`.
//!
//! Also the other small rules the notch surface shares: where a click on a
//! hover-card session row goes, the tray dot, the folded notch's marks, and
//! which ring a session's counts land on.
//!
//! Owner: WP6.

use crate::control::notifications;
use crate::model::{AttentionTransition, Counts, LimitHit, RingId, SessionId, SessionState};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

// ---- Transitions ----

/// What a transition means for the notch (the Mac's
/// `ClaudeAttentionTransition.Kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionKind {
    /// Newly needs the user, or for a different reason (a failed turn too:
    /// see [`is_failure`]).
    NeedsInput,
    /// Newly ready for review.
    ReadyForReview,
    /// Left needs-you or ready-for-review.
    Resolved,
}

fn is_attention(state: &SessionState) -> bool {
    matches!(
        state,
        SessionState::NeedsYou(_) | SessionState::Failed(_) | SessionState::ReadyForReview
    )
}

/// The kind of `tr`, or `None` when it is none of the three (working to
/// idle, say): such a transition makes no reaction at all.
pub fn kind_of(tr: &AttentionTransition) -> Option<TransitionKind> {
    // A failure read back from disk was announced when it happened
    // (`attention::news`): shown, never chimed or peeked at again.
    if tr.became_needs_you() && !is_restored_failure(tr) {
        return Some(TransitionKind::NeedsInput);
    }
    if tr.became_ready_for_review() {
        return Some(TransitionKind::ReadyForReview);
    }
    let was_attention = tr.from.as_ref().is_some_and(is_attention);
    (was_attention && !is_attention(&tr.to)).then_some(TransitionKind::Resolved)
}

/// `tr` shows a failed turn read back from `review-state.json`
/// (`SessionView::stop_error_is_restored`), not one seen happen.
fn is_restored_failure(tr: &AttentionTransition) -> bool {
    tr.session.stop_error_is_restored && tr.to.reason().is_some_and(|r| r.is_error())
}

/// A failed turn (rate limit, overload, sign-in): something to know about,
/// nothing to answer from the panel.
pub fn is_failure(tr: &AttentionTransition) -> bool {
    matches!(tr.to, SessionState::Failed(_)) || tr.to.reason().is_some_and(|r| r.is_error())
}

/// Whether the chime and peek for `tr` would repeat a usage limit already
/// announced: a turn stopped by the limit (needs input, by a rate limit) of an
/// account whose limit was announced, once per account and limit
/// (`control::limits`), not on every retry, wake-up or /loop tick that fails
/// again. `claim(ring, limit_hit)` says whether the limit's reaction is still
/// to come, and records it; it is asked only for such a transition, which must
/// be placed on a ring. Anything else is never held back.
pub fn repeats_limit_reaction(
    tr: &AttentionTransition,
    limit_hit: Option<&LimitHit>,
    claim: impl FnOnce(&RingId, Option<&LimitHit>) -> bool,
) -> bool {
    if kind_of(tr) != Some(TransitionKind::NeedsInput) {
        return false;
    }
    if !tr.to.reason().is_some_and(notifications::is_rate_limit) {
        return false;
    }
    match &tr.session.ring {
        Some(ring) => !claim(ring, limit_hit),
        None => false,
    }
}

// ---- Reactions ----

/// When the panel may open by itself (`autoOpen`). Windows defaults to
/// `Never` until foreground rights on real desktops are settled (§4.12, R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoOpenPolicy {
    Never,
    NeedsInput,
    NeedsInputOrDone,
}

impl AutoOpenPolicy {
    pub const ALL: [AutoOpenPolicy; 3] = [
        AutoOpenPolicy::Never,
        AutoOpenPolicy::NeedsInput,
        AutoOpenPolicy::NeedsInputOrDone,
    ];

    /// The settings value (`never` | `needsInput` | `needsInputOrDone`);
    /// anything else is `Never`, the safe choice.
    pub fn from_setting(value: &str) -> AutoOpenPolicy {
        match value {
            "needsInput" => AutoOpenPolicy::NeedsInput,
            "needsInputOrDone" => AutoOpenPolicy::NeedsInputOrDone,
            _ => AutoOpenPolicy::Never,
        }
    }
}

/// Everything a reaction depends on besides the transition itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PolicyContext {
    pub auto_open: AutoOpenPolicy,
    /// "Play a sound".
    pub chimes: bool,
    /// "Open the notch when a session ends".
    pub peeks: bool,
    /// The session's own terminal (its tab, where that can be told) is in front.
    pub terminal_focused: bool,
    /// Some terminal or editor window is on screen and not covered.
    pub any_terminal_visible: bool,
    /// The foreground app is full screen.
    pub full_screen: bool,
    /// The sessions panel is open.
    pub panel_open: bool,
    /// The session's ring is in the notch (a peek of a notch that does not
    /// show the session would point at nothing), and the notch isn't hidden.
    pub ring_shown: bool,
}

/// Which of the two chimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// You are the hold-up.
    NeedsInput,
    /// A turn is done.
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Decision {
    Chime(Sound),
    /// Open the panel at this session, without taking the keyboard.
    AutoOpen {
        session: SessionId,
        kind: TransitionKind,
    },
    /// Unfold the notch on this ring for the peek duration.
    Peek {
        session: SessionId,
        ring: Option<RingId>,
        pid: Option<u32>,
        kind: TransitionKind,
    },
}

/// Transitions this close together are one burst: one chime, one peek.
pub const BURST_WINDOW: Duration = Duration::from_millis(500);

/// What to do for one transition.
///
/// - Looking at that session's terminal already: nothing at all.
/// - Needs input: the blocked chime; then open the panel by itself when the
///   policy allows it, no terminal is on screen, nothing is full screen and
///   the panel is closed; otherwise peek.
/// - A failed turn: one soft cue, the finished chime and a peek, never the
///   blocked chime or an auto-open for something the panel can't answer.
/// - Ready for review: the finished chime; auto-open only for
///   `NeedsInputOrDone`, otherwise peek.
/// - Resolved: nothing (closing a panel after an answered prompt is the
///   panel's own business, `control::panel`).
/// - Never a peek while the panel is open, or for a ring that isn't shown.
pub fn decide(tr: &AttentionTransition, ctx: &PolicyContext) -> Vec<Decision> {
    if ctx.terminal_focused {
        return Vec::new();
    }
    let Some(kind) = kind_of(tr) else {
        return Vec::new();
    };
    let (sound, opens_by_itself) = match kind {
        TransitionKind::NeedsInput if is_failure(tr) => (Sound::Finished, false),
        TransitionKind::NeedsInput => (Sound::NeedsInput, ctx.auto_open != AutoOpenPolicy::Never),
        TransitionKind::ReadyForReview => (
            Sound::Finished,
            ctx.auto_open == AutoOpenPolicy::NeedsInputOrDone,
        ),
        TransitionKind::Resolved => return Vec::new(),
    };
    let mut decisions = Vec::new();
    if ctx.chimes {
        decisions.push(Decision::Chime(sound));
    }
    if opens_by_itself && !ctx.any_terminal_visible && !ctx.full_screen && !ctx.panel_open {
        decisions.push(Decision::AutoOpen {
            session: tr.session.id.clone(),
            kind,
        });
    } else if ctx.peeks && !ctx.panel_open && ctx.ring_shown {
        decisions.push(Decision::Peek {
            session: tr.session.id.clone(),
            ring: tr.session.ring.clone(),
            pid: tr.session.pid,
            kind,
        });
    }
    decisions
}

fn rank(kind: TransitionKind) -> u8 {
    match kind {
        TransitionKind::NeedsInput => 2,
        TransitionKind::ReadyForReview => 1,
        TransitionKind::Resolved => 0,
    }
}

fn decision_kind(decision: &Decision) -> TransitionKind {
    match decision {
        Decision::AutoOpen { kind, .. } | Decision::Peek { kind, .. } => *kind,
        Decision::Chime(_) => TransitionKind::Resolved,
    }
}

/// One burst's decisions, in arrival order, as what is actually done: at most
/// one chime (the blocked one wins) and at most one of auto-open or peek. An
/// auto-open makes a peek pointless, so any auto-open wins over every peek.
/// Within either, a needs-input session outranks a finished one, and the
/// newest of equals is the one offered.
pub fn merge(decisions: &[Decision]) -> Vec<Decision> {
    let mut sound: Option<Sound> = None;
    let mut open: Option<&Decision> = None;
    let mut peek: Option<&Decision> = None;
    for decision in decisions {
        match decision {
            Decision::Chime(next) => {
                if sound != Some(Sound::NeedsInput) {
                    sound = Some(*next);
                }
            }
            Decision::AutoOpen { kind, .. } => {
                if open.is_none_or(|kept| rank(*kind) >= rank(decision_kind(kept))) {
                    open = Some(decision);
                }
            }
            Decision::Peek { kind, .. } => {
                if peek.is_none_or(|kept| rank(*kind) >= rank(decision_kind(kept))) {
                    peek = Some(decision);
                }
            }
        }
    }
    let mut merged = Vec::new();
    if let Some(sound) = sound {
        merged.push(Decision::Chime(sound));
    }
    if let Some(open) = open {
        merged.push(open.clone());
    } else if let Some(peek) = peek {
        merged.push(peek.clone());
    }
    merged
}

/// Transitions that arrive together, gathered into one reaction.
///
/// A burst opens with its first transition and closes [`BURST_WINDOW`] later,
/// but never while one of its transitions is still being decided: deciding
/// waits on the terminal checks (UI Automation, the console helper), which can
/// outlast the window, and a decision landing after the close would be a
/// second chime.
///
/// ```text
/// let due = burst.begin(now);          // schedule `close` at `due`, if any
/// burst.finish(decide(&tr, &ctx));     // when its context is in
/// burst.close(now)                     // at `due`, and after each finish
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Burst {
    opened_at: Option<SystemTime>,
    pending: usize,
    decisions: Vec<Decision>,
}

impl Burst {
    pub fn new() -> Burst {
        Burst::default()
    }

    pub fn opened_at(&self) -> Option<SystemTime> {
        self.opened_at
    }

    /// Transitions begun and not yet finished.
    pub fn pending(&self) -> usize {
        self.pending
    }

    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// A transition arrived and is being decided. Returns when its burst is
    /// due to close if this transition opened it, `None` if it joined one
    /// already open.
    pub fn begin(&mut self, now: SystemTime) -> Option<SystemTime> {
        self.pending += 1;
        if self.opened_at.is_some() {
            return None;
        }
        self.opened_at = Some(now);
        Some(now + BURST_WINDOW)
    }

    /// One transition's decisions are in.
    pub fn finish(&mut self, made: Vec<Decision>) {
        self.pending = self.pending.saturating_sub(1);
        self.decisions.extend(made);
    }

    /// The burst's merged decisions once it is over: its window has passed
    /// and nothing in it is still being decided. `None` until then. Closing
    /// starts afresh, so the next transition opens a new burst.
    pub fn close(&mut self, now: SystemTime) -> Option<Vec<Decision>> {
        let opened_at = self.opened_at?;
        if self.pending > 0 || now < opened_at + BURST_WINDOW {
            return None;
        }
        let merged = merge(&self.decisions);
        *self = Burst::default();
        Some(merged)
    }
}

// ---- Hover-card session rows ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionClickTarget {
    /// The sessions panel, at that session.
    Panel,
    /// The session's terminal (which marks it reviewed).
    Terminal,
}

/// Where a click on a session row in a ring's hover card goes (`sessionClick`:
/// `smart` | `panel` | `terminal`). `smart` sends a session that needs you to
/// the panel, where it can be answered, and everything else to its terminal.
/// An unknown value acts as `smart`.
pub fn session_click(state: &SessionState, setting: &str) -> SessionClickTarget {
    match setting {
        "panel" => SessionClickTarget::Panel,
        "terminal" => SessionClickTarget::Terminal,
        _ => {
            if matches!(state, SessionState::NeedsYou(_) | SessionState::Failed(_)) {
                SessionClickTarget::Panel
            } else {
                SessionClickTarget::Terminal
            }
        }
    }
}

// ---- What the notch counts ----

/// Per-ring values (attention counts, "just finished" deadlines) as the notch
/// shows them, so a badge, a folded mark or a held-open notch never stands
/// for a session the notch does not show:
///
/// - only rings in `shown` count: a ring switched off has no ring to badge;
/// - a ring id that is no account's (a session whose account isn't known yet)
///   counts on `default_ring`, which is where the session rows go, and
///   `shown` lists it only while the default ring is on.
///
/// With `shown` `None` (nothing has said yet which rings are shown) every
/// value passes through unchanged.
pub fn notch_values<V: Clone>(
    by_ring: &BTreeMap<String, V>,
    rings: &[String],
    shown: Option<&BTreeSet<String>>,
    default_ring: &str,
    combine: impl Fn(&V, &V) -> V,
) -> BTreeMap<String, V> {
    let Some(shown) = shown else {
        return by_ring.clone();
    };
    let account_rings: BTreeSet<&str> = rings.iter().map(String::as_str).collect();
    let mut result: BTreeMap<String, V> = BTreeMap::new();
    for (ring, value) in by_ring {
        if !shown.contains(ring) {
            continue;
        }
        let target = if account_rings.contains(ring.as_str()) {
            ring.clone()
        } else {
            default_ring.to_owned()
        };
        let merged = match result.get(&target) {
            Some(existing) => combine(existing, value),
            None => value.clone(),
        };
        result.insert(target, merged);
    }
    result
}

/// Two sets of counts added together.
pub fn combined(a: &Counts, b: &Counts) -> Counts {
    Counts {
        needs_you: a.needs_you + b.needs_you,
        failed: a.failed + b.failed,
        review: a.review + b.review,
        working: a.working + b.working,
        idle: a.idle + b.idle,
    }
}

/// Every ring's counts added together.
pub fn total<'a>(counts: impl IntoIterator<Item = &'a Counts>) -> Counts {
    counts
        .into_iter()
        .fold(Counts::default(), |sum, next| combined(&sum, next))
}

// ---- Tray and folded notch ----

/// The tray dot's count (the Mac's Dock badge): the needs-you count when the
/// setting is on, else 0. Windows draws a dot, not the digit (§4.10), and puts
/// the count in the tooltip.
pub fn tray_badge(needs_you: u32, enabled: bool) -> u32 {
    if enabled {
        needs_you
    } else {
        0
    }
}

/// One mark on the folded notch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestingMark {
    NeedsYou,
    Review,
    Working,
}

/// The marks the folded notch shows, in drawing order: amber for needs you,
/// green for review, white for working. At most one of each.
pub fn resting_marks(counts: &Counts) -> Vec<RestingMark> {
    let mut marks = Vec::new();
    if counts.needs_you > 0 {
        marks.push(RestingMark::NeedsYou);
    }
    if counts.review > 0 {
        marks.push(RestingMark::Review);
    }
    if counts.working > 0 {
        marks.push(RestingMark::Working);
    }
    marks
}

/// How many half-cycles the amber mark breathes before resting bright. Odd,
/// so an alternating animation towards "bright" ends there.
pub const RESTING_BREATHS: u32 = 7;
