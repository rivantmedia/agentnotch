//! What the notch does when a Claude session starts needing you or finishes:
//! the full matrix of auto-open policy × focused terminal × visible terminal
//! × full screen × panel open (× the sound and peek switches), then the
//! burst merge, the hover-row click, the tray dot and the folded-notch marks.
//!
//! Ported from the Mac's `C_AttentionPolicyTests`.

mod control_support;

use agentnotch_engine::attention::policy::*;
use agentnotch_engine::model::*;
use control_support::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

fn needs_input(id: &str, pid: Option<u32>) -> AttentionTransition {
    let mut session = in_state(view(id), permission("Bash"));
    session.pid = pid;
    transition(session, Some(SessionState::Working))
}

fn ready_for_review(id: &str, pid: Option<u32>) -> AttentionTransition {
    let mut session = in_state(view(id), SessionState::ReadyForReview);
    session.pid = pid;
    transition(session, Some(SessionState::Working))
}

fn resolved(id: &str) -> AttentionTransition {
    transition(
        in_state(view(id), SessionState::Working),
        Some(permission("Bash")),
    )
}

fn of_kind(kind: TransitionKind) -> AttentionTransition {
    match kind {
        TransitionKind::NeedsInput => needs_input("s1", Some(4242)),
        TransitionKind::ReadyForReview => ready_for_review("s1", Some(4242)),
        TransitionKind::Resolved => resolved("s1"),
    }
}

fn context(auto_open: AutoOpenPolicy) -> PolicyContext {
    PolicyContext {
        auto_open,
        chimes: true,
        peeks: true,
        terminal_focused: false,
        any_terminal_visible: false,
        full_screen: false,
        panel_open: false,
        ring_shown: true,
    }
}

fn peek(id: &str, pid: Option<u32>, kind: TransitionKind) -> Decision {
    Decision::Peek {
        session: SessionId::from(id),
        ring: Some(RingId::from(RING)),
        pid,
        kind,
    }
}

fn auto_open(id: &str, kind: TransitionKind) -> Decision {
    Decision::AutoOpen {
        session: SessionId::from(id),
        kind,
    }
}

#[test]
fn transitions_have_a_kind() {
    assert_eq!(
        kind_of(&needs_input("s", None)),
        Some(TransitionKind::NeedsInput)
    );
    assert_eq!(
        kind_of(&ready_for_review("s", None)),
        Some(TransitionKind::ReadyForReview)
    );
    assert_eq!(kind_of(&resolved("s")), Some(TransitionKind::Resolved));
    // Another reason on a session that already needed you is news again.
    let question = in_state(
        view("s"),
        SessionState::NeedsYou(NeedsInputReason::Question),
    );
    assert_eq!(
        kind_of(&transition(question.clone(), Some(permission("Bash")))),
        Some(TransitionKind::NeedsInput)
    );
    // The same reason, or a change between working and idle, is none.
    assert_eq!(
        kind_of(&transition(
            question,
            Some(SessionState::NeedsYou(NeedsInputReason::Question))
        )),
        None
    );
    assert_eq!(
        kind_of(&transition(view("s"), Some(SessionState::Working))),
        None
    );
    // A session first seen already waiting is news.
    assert_eq!(
        kind_of(&transition(in_state(view("s"), permission("Edit")), None)),
        Some(TransitionKind::NeedsInput)
    );
}

/// Every combination, checked against the rules one at a time.
#[test]
fn matrix() {
    let flags = [false, true];
    for kind in [
        TransitionKind::NeedsInput,
        TransitionKind::ReadyForReview,
        TransitionKind::Resolved,
    ] {
        for auto in AutoOpenPolicy::ALL {
            for focused in flags {
                for visible in flags {
                    for full_screen in flags {
                        for panel_open in flags {
                            for chimes in flags {
                                for peeks in flags {
                                    for ring_shown in flags {
                                        let ctx = PolicyContext {
                                            auto_open: auto,
                                            chimes,
                                            peeks,
                                            terminal_focused: focused,
                                            any_terminal_visible: visible,
                                            full_screen,
                                            panel_open,
                                            ring_shown,
                                        };
                                        check(kind, &ctx);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn check(kind: TransitionKind, ctx: &PolicyContext) {
    let decisions = decide(&of_kind(kind), ctx);
    let label = format!("{kind:?} {ctx:?}");

    // Resolved, or already looking at that terminal: nothing at all.
    if kind == TransitionKind::Resolved || ctx.terminal_focused {
        assert!(decisions.is_empty(), "{label}");
        return;
    }

    // Exactly one chime when sounds are on, of the right kind.
    let chimes_made = decisions
        .iter()
        .filter(|decision| matches!(decision, Decision::Chime(_)))
        .count();
    assert_eq!(chimes_made, usize::from(ctx.chimes), "{label}");
    if ctx.chimes {
        let sound = if kind == TransitionKind::NeedsInput {
            Sound::NeedsInput
        } else {
            Sound::Finished
        };
        assert_eq!(decisions.first(), Some(&Decision::Chime(sound)), "{label}");
    }

    // Auto-open only when the policy covers the kind and nothing is in the
    // way; it replaces the peek.
    let covers = if kind == TransitionKind::NeedsInput {
        ctx.auto_open != AutoOpenPolicy::Never
    } else {
        ctx.auto_open == AutoOpenPolicy::NeedsInputOrDone
    };
    let opens = covers && !ctx.any_terminal_visible && !ctx.full_screen && !ctx.panel_open;
    assert_eq!(decisions.contains(&auto_open("s1", kind)), opens, "{label}");

    // Otherwise a peek, when peeks are on, the panel is closed and the ring
    // is in the notch.
    let peeks_here = !opens && ctx.peeks && !ctx.panel_open && ctx.ring_shown;
    assert_eq!(
        decisions.contains(&peek("s1", Some(4242), kind)),
        peeks_here,
        "{label}"
    );

    assert_eq!(
        decisions.len(),
        usize::from(ctx.chimes) + usize::from(opens) + usize::from(peeks_here),
        "{label}"
    );
}

// ---- The cases the design names ----

#[test]
fn needs_input_with_nothing_on_screen_opens_the_panel() {
    assert_eq!(
        decide(
            &needs_input("s1", Some(4242)),
            &context(AutoOpenPolicy::NeedsInput)
        ),
        [
            Decision::Chime(Sound::NeedsInput),
            auto_open("s1", TransitionKind::NeedsInput)
        ]
    );
}

#[test]
fn needs_input_beside_a_visible_terminal_peeks_instead() {
    let ctx = PolicyContext {
        any_terminal_visible: true,
        ..context(AutoOpenPolicy::NeedsInput)
    };
    assert_eq!(
        decide(&needs_input("s1", Some(4242)), &ctx),
        [
            Decision::Chime(Sound::NeedsInput),
            peek("s1", Some(4242), TransitionKind::NeedsInput)
        ]
    );
}

#[test]
fn needs_input_over_a_full_screen_app_peeks_instead() {
    let ctx = PolicyContext {
        full_screen: true,
        ..context(AutoOpenPolicy::NeedsInputOrDone)
    };
    assert_eq!(
        decide(&needs_input("s1", Some(4242)), &ctx),
        [
            Decision::Chime(Sound::NeedsInput),
            peek("s1", Some(4242), TransitionKind::NeedsInput)
        ]
    );
}

#[test]
fn ready_for_review_peeks_unless_auto_open_covers_it() {
    assert_eq!(
        decide(
            &ready_for_review("s1", Some(4242)),
            &context(AutoOpenPolicy::NeedsInput)
        ),
        [
            Decision::Chime(Sound::Finished),
            peek("s1", Some(4242), TransitionKind::ReadyForReview)
        ]
    );
    assert_eq!(
        decide(
            &ready_for_review("s1", Some(4242)),
            &context(AutoOpenPolicy::NeedsInputOrDone)
        ),
        [
            Decision::Chime(Sound::Finished),
            auto_open("s1", TransitionKind::ReadyForReview)
        ]
    );
}

#[test]
fn an_open_panel_only_chimes() {
    let ctx = PolicyContext {
        panel_open: true,
        ..context(AutoOpenPolicy::NeedsInputOrDone)
    };
    assert_eq!(
        decide(&needs_input("s1", Some(4242)), &ctx),
        [Decision::Chime(Sound::NeedsInput)]
    );
}

#[test]
fn a_focused_terminal_silences_everything() {
    let ctx = PolicyContext {
        terminal_focused: true,
        ..context(AutoOpenPolicy::NeedsInputOrDone)
    };
    assert!(decide(&needs_input("s1", Some(4242)), &ctx).is_empty());
    assert!(decide(&ready_for_review("s1", Some(4242)), &ctx).is_empty());
}

#[test]
fn a_session_without_a_pid_still_peeks() {
    assert_eq!(
        decide(
            &ready_for_review("s1", None),
            &context(AutoOpenPolicy::Never)
        ),
        [
            Decision::Chime(Sound::Finished),
            peek("s1", None, TransitionKind::ReadyForReview)
        ]
    );
}

/// A failed turn (rate limit, overload, sign-in): one soft cue, the
/// finished chime and a peek, never the blocked chime or an auto-open for
/// something the panel can't answer.
#[test]
fn a_failed_turn_never_opens_the_panel() {
    let stopped = transition(
        in_state(view("s1"), failed("Overloaded", "overloaded")),
        Some(SessionState::Working),
    );
    assert!(is_failure(&stopped));
    assert_eq!(kind_of(&stopped), Some(TransitionKind::NeedsInput));
    assert_eq!(
        decide(&stopped, &context(AutoOpenPolicy::NeedsInputOrDone)),
        [
            Decision::Chime(Sound::Finished),
            peek("s1", Some(4242), TransitionKind::NeedsInput)
        ]
    );
    assert!(!is_failure(&needs_input("s1", None)));
}

// ---- Bursts ----

#[test]
fn a_burst_makes_one_chime_and_one_peek() {
    let merged = merge(&[
        Decision::Chime(Sound::Finished),
        peek("a", Some(1), TransitionKind::ReadyForReview),
        Decision::Chime(Sound::Finished),
        peek("b", Some(2), TransitionKind::ReadyForReview),
    ]);
    // The newest of equals is the one offered.
    assert_eq!(
        merged,
        [
            Decision::Chime(Sound::Finished),
            peek("b", Some(2), TransitionKind::ReadyForReview)
        ]
    );
}

#[test]
fn the_blocked_sound_and_session_win_a_burst() {
    let merged = merge(&[
        Decision::Chime(Sound::Finished),
        peek("a", Some(1), TransitionKind::ReadyForReview),
        Decision::Chime(Sound::NeedsInput),
        peek("b", Some(2), TransitionKind::NeedsInput),
        Decision::Chime(Sound::Finished),
        peek("c", Some(3), TransitionKind::ReadyForReview),
    ]);
    assert_eq!(
        merged,
        [
            Decision::Chime(Sound::NeedsInput),
            peek("b", Some(2), TransitionKind::NeedsInput)
        ]
    );
}

#[test]
fn an_auto_open_in_a_burst_replaces_every_peek() {
    let merged = merge(&[
        peek("p", Some(1), TransitionKind::NeedsInput),
        auto_open("b", TransitionKind::ReadyForReview),
        auto_open("a", TransitionKind::NeedsInput),
        auto_open("c", TransitionKind::ReadyForReview),
    ]);
    assert_eq!(merged, [auto_open("a", TransitionKind::NeedsInput)]);
}

#[test]
fn merging_nothing_does_nothing() {
    assert!(merge(&[]).is_empty());
    assert_eq!(
        merge(&[Decision::Chime(Sound::Finished)]),
        [Decision::Chime(Sound::Finished)]
    );
}

/// Whatever a burst holds, the result never has two chimes or two ways of
/// opening the notch.
#[test]
fn merged_bursts_are_always_single() {
    let kinds = [TransitionKind::NeedsInput, TransitionKind::ReadyForReview];
    let mut decisions = Vec::new();
    for index in 0..40u32 {
        let kind = kinds[index as usize % 2];
        decisions.push(if index % 3 == 0 {
            Decision::Chime(Sound::Finished)
        } else {
            Decision::Chime(Sound::NeedsInput)
        });
        decisions.push(if index % 5 == 0 {
            auto_open(&format!("s{index}"), kind)
        } else {
            peek(&format!("s{index}"), Some(index), kind)
        });
        let merged = merge(&decisions);
        let chimes = merged
            .iter()
            .filter(|decision| matches!(decision, Decision::Chime(_)))
            .count();
        assert_eq!(chimes, 1);
        assert_eq!(merged.len() - chimes, 1);
    }
}

/// A burst opens with its first transition, and closes once its window has
/// passed and nothing in it is still being decided.
#[test]
fn a_burst_waits_for_its_window_and_its_pending_decisions() {
    let now = t0();
    let mut burst = Burst::new();
    assert_eq!(burst.close(now), None, "nothing is open");

    assert_eq!(burst.begin(now), Some(now + BURST_WINDOW));
    // A second transition joins the open burst.
    assert_eq!(burst.begin(now + Duration::from_millis(100)), None);
    assert_eq!(burst.pending(), 2);
    assert_eq!(burst.opened_at(), Some(now));

    burst.finish(vec![
        Decision::Chime(Sound::Finished),
        peek("a", Some(1), TransitionKind::ReadyForReview),
    ]);
    // Its window has not passed.
    assert_eq!(burst.close(now + Duration::from_millis(200)), None);
    // The window passed, but one decision is still out (a slow terminal
    // check): closing now would let it chime a second time.
    assert_eq!(burst.close(now + BURST_WINDOW), None);

    burst.finish(vec![
        Decision::Chime(Sound::NeedsInput),
        peek("b", Some(2), TransitionKind::NeedsInput),
    ]);
    assert_eq!(burst.decisions().len(), 4);
    assert_eq!(
        burst.close(now + Duration::from_millis(900)),
        Some(vec![
            Decision::Chime(Sound::NeedsInput),
            peek("b", Some(2), TransitionKind::NeedsInput)
        ])
    );

    // Closing starts afresh: the next transition opens a new burst.
    let later = now + Duration::from_secs(5);
    assert_eq!(burst.begin(later), Some(later + BURST_WINDOW));
    burst.finish(Vec::new());
    assert_eq!(burst.close(later + BURST_WINDOW), Some(Vec::new()));
}

// ---- Settings ----

#[test]
fn auto_open_settings_default_to_never() {
    assert_eq!(AutoOpenPolicy::from_setting("never"), AutoOpenPolicy::Never);
    assert_eq!(
        AutoOpenPolicy::from_setting("needsInput"),
        AutoOpenPolicy::NeedsInput
    );
    assert_eq!(
        AutoOpenPolicy::from_setting("needsInputOrDone"),
        AutoOpenPolicy::NeedsInputOrDone
    );
    // Anything unreadable is the safe choice.
    assert_eq!(
        AutoOpenPolicy::from_setting("always"),
        AutoOpenPolicy::Never
    );
    assert_eq!(AutoOpenPolicy::from_setting(""), AutoOpenPolicy::Never);
}

// ---- Hover-card rows ----

#[test]
fn smart_click_sends_what_needs_you_to_the_panel() {
    let needs = SessionState::NeedsYou(NeedsInputReason::Question);
    assert_eq!(session_click(&needs, "smart"), SessionClickTarget::Panel);
    for other in [
        SessionState::Working,
        SessionState::ReadyForReview,
        SessionState::Idle,
    ] {
        assert_eq!(session_click(&other, "smart"), SessionClickTarget::Terminal);
    }
    for state in [
        needs,
        SessionState::Working,
        SessionState::ReadyForReview,
        SessionState::Idle,
    ] {
        assert_eq!(session_click(&state, "panel"), SessionClickTarget::Panel);
        assert_eq!(
            session_click(&state, "terminal"),
            SessionClickTarget::Terminal
        );
    }
}

// ---- Tray and folded notch ----

#[test]
fn the_tray_dot_shows_the_needs_you_count() {
    assert_eq!(tray_badge(3, true), 3);
    assert_eq!(tray_badge(0, true), 0);
    assert_eq!(tray_badge(3, false), 0);
}

#[test]
fn resting_marks_in_order() {
    let counts = |needs_you, review, working, idle| Counts {
        needs_you,
        failed: 0,
        review,
        working,
        idle,
    };
    assert!(resting_marks(&Counts::default()).is_empty());
    assert_eq!(
        resting_marks(&counts(2, 1, 4, 3)),
        [
            RestingMark::NeedsYou,
            RestingMark::Review,
            RestingMark::Working
        ]
    );
    assert_eq!(resting_marks(&counts(0, 1, 0, 3)), [RestingMark::Review]);
    assert_eq!(
        resting_marks(&counts(1, 0, 1, 0)),
        [RestingMark::NeedsYou, RestingMark::Working]
    );
    assert_eq!(RESTING_BREATHS % 2, 1);
}

// ---- What the notch counts ----

#[test]
fn counts_land_on_the_rings_the_notch_shows() {
    let counts = |needs_you| Counts {
        needs_you,
        ..Counts::default()
    };
    let by_ring = BTreeMap::from([
        ("claude-acct-aaa".to_owned(), counts(1)),
        ("claude-acct-bbb".to_owned(), counts(2)),
        // A session whose account isn't known yet.
        ("claude-dir-1234".to_owned(), counts(4)),
    ]);
    let rings = vec!["claude-acct-aaa".to_owned(), "claude-acct-bbb".to_owned()];

    // Nothing has said which rings are shown: everything passes through.
    assert_eq!(
        notch_values(&by_ring, &rings, None, "claude-acct-aaa", combined),
        by_ring
    );

    // The unknown ring's sessions count on the default ring, shown with it.
    let shown = BTreeSet::from(["claude-acct-aaa".to_owned(), "claude-dir-1234".to_owned()]);
    let shown_values = notch_values(&by_ring, &rings, Some(&shown), "claude-acct-aaa", combined);
    assert_eq!(
        shown_values,
        BTreeMap::from([("claude-acct-aaa".to_owned(), counts(5))])
    );
    assert_eq!(total(shown_values.values()).needs_you, 5);

    // A ring switched off has no ring to badge.
    let only_b = BTreeSet::from(["claude-acct-bbb".to_owned()]);
    assert_eq!(
        notch_values(&by_ring, &rings, Some(&only_b), "claude-acct-aaa", combined),
        BTreeMap::from([("claude-acct-bbb".to_owned(), counts(2))])
    );
    assert_eq!(total(by_ring.values()).needs_you, 7);
}
