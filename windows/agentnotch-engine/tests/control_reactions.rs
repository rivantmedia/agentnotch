//! When the panel opens, closes or only peeks by itself, and what a burst of
//! attention transitions makes of the chime. Ported from
//! `ClaudePanelPolicyTests` (the auto-open half: lines 81-152) and the
//! reactions-with-`PanelState` rules of design §3.4 and UI§3.8.
//!
//! The attention policy's own matrix is `tests/attention_policy.rs`; here
//! the `ReactionContext` -> `PolicyContext` mapping and the carrying out are
//! tested, plus the auto-close deadline and `AutoOpenWatch`.
//!
//! Mapped, not ported one to one:
//! - `Policy.AutoOpen` + `autoCloseDeadline` is `AutoOpen::deadline` and, over
//!   what the glue reports, `auto_close_deadline(panel, ..)`.
//! - `presentation(for:landsOnList:)` is `lands_on_list`; its chat half
//!   (`landsOnList: false`) is the glue's.
//! - `anUntouchedAutoOpenNeverTakesKeyForItsContent`: the engine's part is
//!   that an auto-open's request carries reason `auto`, which the glue opens
//!   without activating the window. A panel that reports itself engaged or
//!   focused is the user's and never auto-closes.
//!
//! Not here: the rest of `ClaudePanelPolicyTests` (ring click, anchoring,
//! outside clicks, the hold matrix, edit keys, hot keys, launch requests) is
//! WP7's and WP9's.

mod control_support;

use agentnotch_engine::attention::policy::{self, Decision, Sound, TransitionKind};
use agentnotch_engine::control::auto_close_deadline as reexported_deadline;
use agentnotch_engine::control::notifications::{self, tag};
use agentnotch_engine::control::panel::*;
use agentnotch_engine::control::reactions::{
    carry_out, decide, policy_context, reactions, reactions_each,
};
use agentnotch_engine::model::*;
use agentnotch_engine::platform::{Chime, ToastKind};
use agentnotch_engine::runtime_types::{PanelState, ReactionContext};
use control_support::*;
use std::time::{Duration, SystemTime};

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

fn opened() -> SystemTime {
    t0()
}

/// A panel the engine opened by itself, untouched.
fn auto_panel() -> PanelState {
    PanelState {
        open: true,
        route: Some(ROUTE_SESSIONS.into()),
        reason: Some(REASON_AUTO.into()),
        ..PanelState::default()
    }
}

fn open_panel_ctx(auto_open: &str) -> ReactionContext {
    ReactionContext {
        panel: PanelState {
            open: true,
            route: Some(ROUTE_SESSIONS.into()),
            reason: Some("ring_click".into()),
            ..PanelState::default()
        },
        ..reaction_ctx_with(auto_open)
    }
}

fn on_ring(mut tr: AttentionTransition, ring: &str) -> AttentionTransition {
    tr.session.ring = Some(RingId::from(ring));
    tr
}

// ---- (a) the auto-close deadline ----

#[test]
fn untouched_auto_open_closes_after_the_peek_or_eight_seconds() {
    let state = AutoOpen::new(opened());
    assert_eq!(
        state.deadline(Duration::from_secs(5)),
        Some(opened() + secs(8.0))
    );
    assert_eq!(
        state.deadline(Duration::from_secs(10)),
        Some(opened() + secs(10.0))
    );
    // The same through what the glue reports.
    let panel = auto_panel();
    assert_eq!(
        auto_close_deadline(&panel, None, opened(), 5),
        Some(opened() + secs(8.0))
    );
    assert_eq!(
        auto_close_deadline(&panel, None, opened(), 10),
        Some(opened() + secs(10.0))
    );
}

#[test]
fn the_deadline_follows_the_peek_duration_of_three_five_and_ten_seconds() {
    let panel = auto_panel();
    for (peek, lasts) in [(3, 8.0), (5, 8.0), (8, 8.0), (10, 10.0), (30, 30.0)] {
        assert_eq!(
            auto_close_deadline(&panel, None, opened(), peek),
            Some(opened() + secs(lasts)),
            "peek {peek} s"
        );
    }
    // Resolved at once, the one-second linger beats every peek length.
    for peek in [3, 5, 10] {
        assert_eq!(
            auto_close_deadline(&panel, Some(opened()), opened(), peek),
            Some(opened() + secs(1.0)),
            "peek {peek} s"
        );
    }
}

#[test]
fn auto_open_closes_one_second_after_its_session_is_resolved() {
    let state = AutoOpen {
        opened_at: opened(),
        resolved_at: Some(opened() + secs(2.0)),
        engaged: false,
    };
    assert_eq!(
        state.deadline(Duration::from_secs(5)),
        Some(opened() + secs(3.0))
    );
    // Resolved late: the timeout still wins.
    let late = AutoOpen {
        resolved_at: Some(opened() + secs(7.5)),
        ..state
    };
    assert_eq!(
        late.deadline(Duration::from_secs(5)),
        Some(opened() + secs(8.0))
    );
    // Whichever is first, at the edge too: resolved at 7 s is due at 8 s, and
    // a long peek moves the timeout but not the linger.
    let edge = AutoOpen {
        resolved_at: Some(opened() + secs(7.0)),
        ..state
    };
    assert_eq!(
        edge.deadline(Duration::from_secs(5)),
        Some(opened() + secs(8.0))
    );
    assert_eq!(
        edge.deadline(Duration::from_secs(20)),
        Some(opened() + secs(8.0))
    );
    // The same through the glue's report.
    assert_eq!(
        auto_close_deadline(&auto_panel(), Some(opened() + secs(2.0)), opened(), 5),
        Some(opened() + secs(3.0))
    );
    assert_eq!(
        auto_close_deadline(&auto_panel(), Some(opened() + secs(7.5)), opened(), 5),
        Some(opened() + secs(8.0))
    );
}

#[test]
fn an_engaged_auto_open_stays() {
    let state = AutoOpen {
        opened_at: opened(),
        resolved_at: Some(opened()),
        engaged: true,
    };
    assert_eq!(state.deadline(Duration::from_secs(5)), None);
    // The pointer inside, or the keyboard in a field or confirmed: the user's.
    for panel in [
        PanelState {
            engaged: true,
            ..auto_panel()
        },
        PanelState {
            focused: true,
            ..auto_panel()
        },
    ] {
        assert_eq!(auto_close_deadline(&panel, None, opened(), 5), None);
        assert_eq!(
            auto_close_deadline(&panel, Some(opened()), opened(), 5),
            None
        );
    }
}

#[test]
fn only_an_open_panel_that_opened_itself_has_a_deadline() {
    let closed = PanelState {
        open: false,
        ..auto_panel()
    };
    assert_eq!(auto_close_deadline(&closed, None, opened(), 5), None);
    for reason in [
        "ring_click",
        "hover_row",
        "peek_click",
        "notification",
        "hotkey",
        "settings",
    ] {
        let panel = PanelState {
            reason: Some(reason.into()),
            ..auto_panel()
        };
        assert_eq!(
            auto_close_deadline(&panel, Some(opened()), opened(), 5),
            None,
            "reason {reason}"
        );
    }
    let no_reason = PanelState {
        reason: None,
        ..auto_panel()
    };
    assert_eq!(auto_close_deadline(&no_reason, None, opened(), 5), None);
}

#[test]
fn the_section_3_4_function_is_the_same_rule() {
    let panel = auto_panel();
    assert_eq!(
        reexported_deadline(&panel, Some(opened() + secs(2.0)), opened(), 5),
        auto_close_deadline(&panel, Some(opened() + secs(2.0)), opened(), 5)
    );
}

// ---- (a) what an auto-open is for, and when it is over ----

#[test]
fn auto_open_causes_are_needs_input_and_review() {
    assert_eq!(
        auto_open_cause(&permission("Bash")),
        Some(AutoOpenCause::NeedsInput)
    );
    assert_eq!(
        auto_open_cause(&SessionState::NeedsYou(NeedsInputReason::Question)),
        Some(AutoOpenCause::NeedsInput)
    );
    assert_eq!(
        auto_open_cause(&failed("Rate limited", "rate_limit")),
        Some(AutoOpenCause::NeedsInput)
    );
    assert_eq!(
        auto_open_cause(&SessionState::ReadyForReview),
        Some(AutoOpenCause::ReadyForReview)
    );
    assert_eq!(auto_open_cause(&SessionState::Working), None);
    assert_eq!(auto_open_cause(&SessionState::Idle), None);
}

#[test]
fn an_auto_open_is_resolved_when_its_session_leaves_that_attention_or_goes_away() {
    let permission = permission("Bash");
    let question = SessionState::NeedsYou(NeedsInputReason::Question);
    // Still waiting, even on a different question: the user is still needed.
    assert!(!is_resolved(AutoOpenCause::NeedsInput, Some(&permission)));
    assert!(!is_resolved(AutoOpenCause::NeedsInput, Some(&question)));
    // Answered: back to work, or done.
    assert!(is_resolved(
        AutoOpenCause::NeedsInput,
        Some(&SessionState::Working)
    ));
    assert!(is_resolved(
        AutoOpenCause::NeedsInput,
        Some(&SessionState::ReadyForReview)
    ));
    // Reviewed, or the session ended.
    assert!(is_resolved(
        AutoOpenCause::ReadyForReview,
        Some(&SessionState::Idle)
    ));
    assert!(!is_resolved(
        AutoOpenCause::ReadyForReview,
        Some(&SessionState::ReadyForReview)
    ));
    assert!(is_resolved(AutoOpenCause::NeedsInput, None));
    assert!(is_resolved(AutoOpenCause::ReadyForReview, None));
    // Needing the user is not what a review panel was opened for.
    assert!(is_resolved(
        AutoOpenCause::ReadyForReview,
        Some(&permission)
    ));
}

// ---- (a) the panel lands on the list ----

#[test]
fn a_banner_or_an_auto_open_lands_on_the_list_with_the_row_highlighted() {
    // Opening the chat would mark a finished session reviewed at once,
    // resolving (and closing) the panel that auto-opened for it.
    for reason in [REASON_AUTO, REASON_NOTIFICATION] {
        let request = lands_on_list(&SessionId::from("s1"), reason);
        assert_eq!(request.route, "sessions");
        assert_eq!(request.route, ROUTE_SESSIONS);
        assert_eq!(request.highlight.as_deref(), Some("s1"));
        assert_eq!(request.ring_id, None);
        assert_eq!(request.reason, reason);
    }
    assert_eq!(REASON_AUTO, "auto");
    assert_eq!(REASON_NOTIFICATION, "notification");
}

#[test]
fn an_auto_open_requests_reason_auto_which_the_glue_opens_without_activation() {
    let r = reactions(&[needs_you_now("s1")], &reaction_ctx());
    let request = r.open_panel.expect("opens by itself");
    assert_eq!(request.reason, REASON_AUTO);
    assert_eq!(request.route, ROUTE_SESSIONS);
    assert_eq!(request.highlight.as_deref(), Some("s1"));
    // Never a chat route: an auto-opened chat would mark the session read.
    assert!(!request.route.starts_with("session:"));
}

// ---- (a) AutoOpenWatch ----

#[test]
fn a_watch_follows_its_panel_from_opened_to_closed() {
    let id = SessionId::from("s1");
    let mut watch = AutoOpenWatch::opened(id.clone(), Some(&permission("Bash")), opened());
    assert_eq!(watch.session, id);
    assert_eq!(watch.cause, Some(AutoOpenCause::NeedsInput));
    assert_eq!(watch.deadline(5), Some(opened() + secs(8.0)));
    assert_eq!(watch.deadline(12), Some(opened() + secs(12.0)));

    // The glue reports the panel untouched, still ours.
    assert!(watch.panel_reported(&auto_panel()));
    assert!(!watch.state.engaged);

    // Still waiting: nothing resolved, whatever time passes.
    watch.session_is(Some(&permission("Bash")), opened() + secs(1.0));
    assert_eq!(watch.state.resolved_at, None);
    assert_eq!(watch.deadline(5), Some(opened() + secs(8.0)));

    // Answered at 2 s: closes at 3 s.
    watch.session_is(Some(&SessionState::Working), opened() + secs(2.0));
    assert_eq!(watch.state.resolved_at, Some(opened() + secs(2.0)));
    assert_eq!(watch.deadline(5), Some(opened() + secs(3.0)));

    // The first moment counts; a later look does not move it.
    watch.session_is(None, opened() + secs(2.5));
    assert_eq!(watch.state.resolved_at, Some(opened() + secs(2.0)));
    // Not even if it needs the user again.
    watch.session_is(Some(&permission("Bash")), opened() + secs(2.6));
    assert_eq!(watch.state.resolved_at, Some(opened() + secs(2.0)));

    // The panel closed, or opened again another way: the watch is over.
    let closed = PanelState::default();
    assert!(!watch.panel_reported(&closed));
    let clicked = PanelState {
        reason: Some("ring_click".into()),
        ..auto_panel()
    };
    assert!(!watch.panel_reported(&clicked));
}

#[test]
fn a_watch_remembers_that_the_pointer_went_in_and_out_again() {
    let mut watch = AutoOpenWatch::opened(
        SessionId::from("s1"),
        Some(&SessionState::ReadyForReview),
        opened(),
    );
    assert_eq!(watch.cause, Some(AutoOpenCause::ReadyForReview));
    let inside = PanelState {
        engaged: true,
        ..auto_panel()
    };
    assert!(watch.panel_reported(&inside));
    assert!(watch.state.engaged);
    // The pointer left: the report no longer says engaged, the watch does.
    assert!(watch.panel_reported(&auto_panel()));
    assert!(watch.state.engaged);
    assert_eq!(watch.deadline(5), None);
    // Resolving an engaged panel changes nothing: the user's panel stays.
    watch.session_is(Some(&SessionState::Idle), opened() + secs(1.0));
    assert_eq!(watch.deadline(5), None);

    // A key-holding panel engages too.
    let mut other = AutoOpenWatch::opened(SessionId::from("s2"), None, opened());
    assert!(other.panel_reported(&PanelState {
        focused: true,
        ..auto_panel()
    }));
    assert!(other.state.engaged);
}

#[test]
fn a_watch_for_a_session_already_past_its_attention_closes_on_the_timeout_only() {
    // Working or idle by the time the panel opened: no cause to resolve.
    for state in [Some(SessionState::Working), Some(SessionState::Idle), None] {
        let mut watch = AutoOpenWatch::opened(SessionId::from("s1"), state.as_ref(), opened());
        assert_eq!(watch.cause, None);
        watch.session_is(None, opened() + secs(1.0));
        assert_eq!(watch.state.resolved_at, None);
        assert_eq!(watch.deadline(5), Some(opened() + secs(8.0)));
    }
}

#[test]
fn a_watch_for_a_session_that_goes_away_closes_one_second_later() {
    let mut watch = AutoOpenWatch::opened(
        SessionId::from("s1"),
        Some(&SessionState::ReadyForReview),
        opened(),
    );
    watch.session_is(None, opened() + secs(4.0));
    assert_eq!(watch.deadline(5), Some(opened() + secs(5.0)));
}

// ---- (b) reactions with the panel and the settings ----

#[test]
fn a_session_needing_you_auto_opens_the_list_and_chimes_blocked() {
    let r = reactions(&[needs_you_now("s1")], &reaction_ctx());
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(
        r.open_panel,
        Some(lands_on_list(&SessionId::from("s1"), "auto"))
    );
    // An auto-open makes the peek pointless.
    assert_eq!(r.peek, None);
    assert!(r.toasts.is_empty());
    assert!(r.withdraw.is_empty());
}

#[test]
fn never_auto_open_over_an_open_panel() {
    for setting in ["needsInput", "needsInputOrDone"] {
        let ctx = open_panel_ctx(setting);
        for tr in [needs_you_now("s1"), review_now("s1")] {
            let r = reactions(&[tr], &ctx);
            assert_eq!(r.open_panel, None, "{setting}");
            // Chime only: no peek over a panel that already shows the list.
            assert_eq!(r.peek, None, "{setting}");
            assert!(r.chime.is_some(), "{setting}");
        }
    }
    let r = reactions(&[needs_you_now("s1")], &open_panel_ctx("needsInput"));
    assert_eq!(r.chime, Some(Chime::Blocked));
    // An auto-opened panel is still an open panel.
    let over_auto = ReactionContext {
        panel: auto_panel(),
        ..reaction_ctx()
    };
    let r = reactions(&[needs_you_now("s1")], &over_auto);
    assert_eq!(r.open_panel, None);
    assert_eq!(r.peek, None);
    assert_eq!(r.chime, Some(Chime::Blocked));
}

#[test]
fn never_auto_open_when_the_setting_is_never_peek_instead() {
    let ctx = reaction_ctx_with("never");
    let r = reactions(&[needs_you_now("s1")], &ctx);
    assert_eq!(r.open_panel, None);
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
}

#[test]
fn an_unknown_auto_open_setting_counts_as_never() {
    for setting in ["", "always", "needsinput", "NEVER", "needs_input", "true"] {
        let r = reactions(&[needs_you_now("s1")], &reaction_ctx_with(setting));
        assert_eq!(r.open_panel, None, "{setting:?}");
        assert_eq!(r.peek, Some((RingId::from(RING), 5)), "{setting:?}");
        assert_eq!(r.chime, Some(Chime::Blocked), "{setting:?}");
    }
}

#[test]
fn the_mapping_reads_the_three_settings() {
    use agentnotch_engine::attention::policy::AutoOpenPolicy;
    assert_eq!(
        policy_context(&reaction_ctx_with("never")).auto_open,
        AutoOpenPolicy::Never
    );
    assert_eq!(
        policy_context(&reaction_ctx_with("needsInput")).auto_open,
        AutoOpenPolicy::NeedsInput
    );
    assert_eq!(
        policy_context(&reaction_ctx_with("needsInputOrDone")).auto_open,
        AutoOpenPolicy::NeedsInputOrDone
    );
}

#[test]
fn needs_input_or_done_opens_for_review_too() {
    let r = reactions(&[review_now("s1")], &reaction_ctx_with("needsInputOrDone"));
    assert_eq!(r.chime, Some(Chime::Finished));
    assert_eq!(
        r.open_panel,
        Some(lands_on_list(&SessionId::from("s1"), "auto"))
    );
    assert_eq!(r.peek, None);
    // needsInput alone peeks for a review, and never for the other
    // policies' open.
    let r = reactions(&[review_now("s1")], &reaction_ctx_with("needsInput"));
    assert_eq!(r.chime, Some(Chime::Finished));
    assert_eq!(r.open_panel, None);
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
    let r = reactions(&[review_now("s1")], &reaction_ctx_with("never"));
    assert_eq!(r.open_panel, None);
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
    // needsInputOrDone also opens for needs-you.
    let r = reactions(
        &[needs_you_now("s1")],
        &reaction_ctx_with("needsInputOrDone"),
    );
    assert!(r.open_panel.is_some());
}

#[test]
fn full_screen_or_a_visible_terminal_peeks_instead_of_opening() {
    for ctx in [
        ReactionContext {
            full_screen: true,
            ..reaction_ctx()
        },
        ReactionContext {
            any_terminal_visible: true,
            ..reaction_ctx()
        },
        ReactionContext {
            full_screen: true,
            any_terminal_visible: true,
            ..reaction_ctx()
        },
    ] {
        let r = reactions(&[needs_you_now("s1")], &ctx);
        assert_eq!(r.open_panel, None);
        assert_eq!(r.chime, Some(Chime::Blocked));
        assert_eq!(r.peek, Some((RingId::from(RING), 5)));
    }
    // The same for review under needsInputOrDone.
    let ctx = ReactionContext {
        any_terminal_visible: true,
        ..reaction_ctx_with("needsInputOrDone")
    };
    let r = reactions(&[review_now("s1")], &ctx);
    assert_eq!(r.open_panel, None);
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
}

#[test]
fn looking_at_the_session_does_nothing_at_all() {
    let ctx = ReactionContext {
        looking_at: Some(true),
        ..reaction_ctx_with("needsInputOrDone")
    };
    for tr in [needs_you_now("s1"), review_now("s1"), failed_now("s1")] {
        let r = reactions(&[tr], &ctx);
        assert_eq!(r.chime, None);
        assert_eq!(r.peek, None);
        assert_eq!(r.open_panel, None);
    }
}

#[test]
fn not_knowing_whether_the_user_looks_is_not_looking() {
    for looking in [None, Some(false)] {
        let ctx = ReactionContext {
            looking_at: looking,
            ..reaction_ctx()
        };
        let r = reactions(&[needs_you_now("s1")], &ctx);
        assert_eq!(r.chime, Some(Chime::Blocked), "{looking:?}");
        assert!(r.open_panel.is_some(), "{looking:?}");
    }
    assert!(
        !policy_context(&ReactionContext {
            looking_at: None,
            ..reaction_ctx()
        })
        .terminal_focused
    );
}

#[test]
fn a_hidden_ring_or_notch_gets_no_peek_but_still_chimes() {
    for ctx in [
        ReactionContext {
            ring_shown: false,
            ..reaction_ctx_with("never")
        },
        ReactionContext {
            notch_hidden: true,
            ..reaction_ctx_with("never")
        },
    ] {
        let r = reactions(&[needs_you_now("s1")], &ctx);
        assert_eq!(r.peek, None);
        assert_eq!(r.chime, Some(Chime::Blocked));
        let r = reactions(&[review_now("s1")], &ctx);
        assert_eq!(r.peek, None);
        assert_eq!(r.chime, Some(Chime::Finished));
    }
    // A hidden ring does not stop the panel opening: it needs no ring.
    let ctx = ReactionContext {
        ring_shown: false,
        ..reaction_ctx()
    };
    assert!(reactions(&[needs_you_now("s1")], &ctx).open_panel.is_some());
}

#[test]
fn sound_off_means_no_chime_but_everything_else_stays() {
    let ctx = ReactionContext {
        sound: false,
        ..reaction_ctx()
    };
    let r = reactions(&[needs_you_now("s1")], &ctx);
    assert_eq!(r.chime, None);
    assert!(r.open_panel.is_some());
    let ctx = ReactionContext {
        sound: false,
        ..reaction_ctx_with("never")
    };
    let r = reactions(&[review_now("s1")], &ctx);
    assert_eq!(r.chime, None);
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
}

#[test]
fn peeks_off_means_no_peek_but_the_chime_and_the_open_stay() {
    let ctx = ReactionContext {
        peek: false,
        ..reaction_ctx_with("never")
    };
    for tr in [needs_you_now("s1"), review_now("s1"), failed_now("s1")] {
        let r = reactions(&[tr], &ctx);
        assert_eq!(r.peek, None);
        assert!(r.chime.is_some());
        assert_eq!(r.open_panel, None);
    }
    // Auto-open is its own switch.
    let ctx = ReactionContext {
        peek: false,
        ..reaction_ctx()
    };
    assert!(reactions(&[needs_you_now("s1")], &ctx).open_panel.is_some());
    // Peeks off, a visible terminal and no open: only the chime is left.
    let ctx = ReactionContext {
        peek: false,
        any_terminal_visible: true,
        ..reaction_ctx()
    };
    let r = reactions(&[needs_you_now("s1")], &ctx);
    assert_eq!((r.open_panel, r.peek), (None, None));
    assert_eq!(r.chime, Some(Chime::Blocked));
}

#[test]
fn a_failed_turn_never_auto_opens_it_chimes_finished_and_peeks() {
    for setting in ["never", "needsInput", "needsInputOrDone"] {
        let r = reactions(&[failed_now("s1")], &reaction_ctx_with(setting));
        assert_eq!(r.open_panel, None, "{setting}");
        assert_eq!(r.chime, Some(Chime::Finished), "{setting}");
        assert_eq!(r.peek, Some((RingId::from(RING), 5)), "{setting}");
    }
    // A needs-you whose reason is an error is a failure too.
    let tr = transition(
        in_state(
            view("s1"),
            SessionState::NeedsYou(NeedsInputReason::Error {
                text: "Overloaded".into(),
                code: Some("overloaded".into()),
            }),
        ),
        Some(SessionState::Working),
    );
    let r = reactions(&[tr], &reaction_ctx());
    assert_eq!(r.open_panel, None);
    assert_eq!(r.chime, Some(Chime::Finished));
}

#[test]
fn working_idle_and_resolved_transitions_react_with_nothing() {
    let ctx = reaction_ctx_with("needsInputOrDone");
    let nothing = |tr: AttentionTransition| {
        let r = reactions(&[tr], &ctx);
        (r.chime, r.peek, r.open_panel)
    };
    // Resolved by an answer.
    assert_eq!(
        nothing(transition(
            in_state(view("s1"), SessionState::Working),
            Some(permission("Bash"))
        )),
        (None, None, None)
    );
    // Reviewed.
    assert_eq!(
        nothing(transition(
            in_state(view("s1"), SessionState::Idle),
            Some(SessionState::ReadyForReview)
        )),
        (None, None, None)
    );
    // Not an attention state at all.
    assert_eq!(
        nothing(transition(
            in_state(view("s1"), SessionState::Idle),
            Some(SessionState::Working)
        )),
        (None, None, None)
    );
    // A state that does not change makes nothing new.
    assert_eq!(
        nothing(transition(
            in_state(view("s1"), permission("Bash")),
            Some(permission("Bash"))
        )),
        (None, None, None)
    );
}

#[test]
fn a_burst_makes_one_chime_blocked_wins_and_one_open_needs_you_outranking_review() {
    let ctx = reaction_ctx_with("needsInputOrDone");
    // Review arrives first, then a session that needs you.
    let r = reactions(&[review_now("done"), needs_you_now("blocked")], &ctx);
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(
        r.open_panel,
        Some(lands_on_list(&SessionId::from("blocked"), "auto"))
    );
    assert_eq!(r.peek, None);
    // Whatever the order.
    let r = reactions(&[needs_you_now("blocked"), review_now("done")], &ctx);
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(
        r.open_panel.and_then(|p| p.highlight),
        Some("blocked".into())
    );
    // The newest of equals.
    let r = reactions(&[needs_you_now("a"), needs_you_now("b")], &ctx);
    assert_eq!(r.open_panel.and_then(|p| p.highlight), Some("b".into()));
    let r = reactions(&[review_now("a"), review_now("b")], &ctx);
    assert_eq!(r.chime, Some(Chime::Finished));
    assert_eq!(r.open_panel.and_then(|p| p.highlight), Some("b".into()));
}

#[test]
fn a_burst_of_failures_and_reviews_chimes_finished_once_and_peeks_once() {
    let ctx = reaction_ctx_with("never");
    let r = reactions(&[failed_now("a"), review_now("b"), failed_now("c")], &ctx);
    assert_eq!(r.chime, Some(Chime::Finished));
    assert_eq!(r.open_panel, None);
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
}

#[test]
fn a_burst_peeks_the_ring_of_the_session_needing_you_then_the_newest() {
    let ctx = reaction_ctx_with("never");
    let ring_a = "claude-acct-aaaaaaaaaaaa";
    let ring_b = "claude-acct-bbbbbbbbbbbb";
    let ring_c = "claude-acct-cccccccccccc";
    let r = reactions(
        &[
            on_ring(needs_you_now("a"), ring_a),
            on_ring(review_now("b"), ring_b),
        ],
        &ctx,
    );
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(r.peek, Some((RingId::from(ring_a), 5)));
    let r = reactions(
        &[
            on_ring(needs_you_now("a"), ring_a),
            on_ring(needs_you_now("b"), ring_b),
            on_ring(review_now("c"), ring_c),
        ],
        &ctx,
    );
    assert_eq!(r.peek, Some((RingId::from(ring_b), 5)));
}

#[test]
fn a_burst_over_an_open_panel_chimes_once_and_shows_nothing() {
    let r = reactions(
        &[needs_you_now("a"), review_now("b")],
        &open_panel_ctx("needsInputOrDone"),
    );
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!((r.open_panel, r.peek), (None, None));
}

#[test]
fn a_burst_where_only_some_transitions_are_looked_at_reacts_to_the_rest() {
    let looked = ReactionContext {
        looking_at: Some(true),
        ..reaction_ctx()
    };
    let away = reaction_ctx();
    let blocked = needs_you_now("looked");
    let review = review_now("away");
    let r = reactions_each(&[(&blocked, &looked), (&review, &away)], None);
    // The one the user sees makes no chime; the review's finished chime stays.
    assert_eq!(r.chime, Some(Chime::Finished));
    assert_eq!(r.open_panel, None);
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
}

#[test]
fn the_peek_lasts_as_long_as_the_peek_seconds_say() {
    for seconds in [3, 5, 10] {
        let ctx = ReactionContext {
            peek_seconds: seconds,
            ..reaction_ctx_with("never")
        };
        let r = reactions(&[needs_you_now("s1")], &ctx);
        assert_eq!(r.peek, Some((RingId::from(RING), seconds)));
    }
    // With a context per transition, the last one's duration is used.
    let short = ReactionContext {
        peek_seconds: 3,
        ..reaction_ctx_with("never")
    };
    let long = ReactionContext {
        peek_seconds: 10,
        ..reaction_ctx_with("never")
    };
    let (a, b) = (needs_you_now("a"), needs_you_now("b"));
    let r = reactions_each(&[(&a, &short), (&b, &long)], None);
    assert_eq!(r.peek, Some((RingId::from(RING), 10)));
    let r = reactions_each(&[(&a, &long), (&b, &short)], None);
    assert_eq!(r.peek, Some((RingId::from(RING), 3)));
}

#[test]
fn a_session_without_an_account_peeks_the_default_ring_or_nothing() {
    let mut tr = needs_you_now("s1");
    tr.session.ring = None;
    let ctx = reaction_ctx_with("never");
    let default = RingId::from("claude-acct-dddddddddddd");
    let r = reactions_each(&[(&tr, &ctx)], Some(&default));
    assert_eq!(r.peek, Some((default.clone(), 5)));
    let r = reactions_each(&[(&tr, &ctx)], None);
    assert_eq!(r.peek, None);
    assert_eq!(r.chime, Some(Chime::Blocked));
    // Its own ring beats the default.
    let own = needs_you_now("s2");
    let r = reactions_each(&[(&own, &ctx)], Some(&default));
    assert_eq!(r.peek, Some((RingId::from(RING), 5)));
}

#[test]
fn an_empty_burst_reacts_with_nothing() {
    let r = reactions(&[], &reaction_ctx());
    assert_eq!(r, Default::default());
    let r = reactions_each(&[], None);
    assert_eq!(r, Default::default());
}

#[test]
fn resolved_transitions_withdraw_their_banners_once() {
    let ctx = reaction_ctx_with("needsInputOrDone");
    let answered = transition(
        in_state(view("s1"), SessionState::Working),
        Some(permission("Bash")),
    );
    let r = reactions(std::slice::from_ref(&answered), &ctx);
    let needs = (
        tag(ToastKind::NeedsInput).to_owned(),
        notifications::group("s1"),
    );
    assert_eq!(r.withdraw, vec![needs.clone()]);
    assert_eq!((r.chime, r.peek, r.open_panel), (None, None, None));
    // The same transition twice in a burst: one withdrawal.
    let r = reactions(&[answered.clone(), answered], &ctx);
    assert_eq!(r.withdraw, vec![needs.clone()]);

    // Reviewed.
    let reviewed = transition(
        in_state(view("s2"), SessionState::Idle),
        Some(SessionState::ReadyForReview),
    );
    let r = reactions(&[reviewed], &ctx);
    assert_eq!(
        r.withdraw,
        vec![(
            tag(ToastKind::Review).to_owned(),
            notifications::group("s2")
        )]
    );

    // A failure cleared.
    let recovered = transition(
        in_state(view("s3"), SessionState::Working),
        Some(failed("Rate limited", "rate_limit")),
    );
    let r = reactions(&[recovered], &ctx);
    assert!(r.withdraw.contains(&(
        tag(ToastKind::Failed).to_owned(),
        notifications::group("s3")
    )));
    assert_eq!(r.withdraw.len(), 2, "a failed state is also a wait: {r:?}");
}

#[test]
fn a_session_that_resolves_while_another_arrives_both_react() {
    let ctx = reaction_ctx();
    let answered = transition(
        in_state(view("old"), SessionState::Working),
        Some(permission("Bash")),
    );
    let r = reactions(&[answered, needs_you_now("new")], &ctx);
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(r.open_panel.and_then(|p| p.highlight), Some("new".into()));
    assert_eq!(
        r.withdraw,
        vec![(
            tag(ToastKind::NeedsInput).to_owned(),
            notifications::group("old")
        )]
    );
}

#[test]
fn withdrawals_come_even_while_the_user_looks() {
    // Looking at the session silences the chime, not the clean-up.
    let ctx = ReactionContext {
        looking_at: Some(true),
        ..reaction_ctx()
    };
    let answered = transition(
        in_state(view("s1"), SessionState::Working),
        Some(permission("Bash")),
    );
    let r = reactions(&[answered], &ctx);
    assert_eq!(r.withdraw.len(), 1);
}

// ---- carrying out ----

#[test]
fn carry_out_turns_merged_decisions_into_reactions() {
    let ring = RingId::from("claude-acct-eeeeeeeeeeee");
    let merged = [
        Decision::Chime(Sound::NeedsInput),
        Decision::AutoOpen {
            session: SessionId::from("s9"),
            kind: TransitionKind::NeedsInput,
        },
    ];
    let r = carry_out(&merged, 7, Some(&ring));
    assert_eq!(r.chime, Some(Chime::Blocked));
    assert_eq!(
        r.open_panel,
        Some(lands_on_list(&SessionId::from("s9"), "auto"))
    );
    assert_eq!(r.peek, None);

    let merged = [
        Decision::Chime(Sound::Finished),
        Decision::Peek {
            session: SessionId::from("s9"),
            ring: None,
            pid: Some(1),
            kind: TransitionKind::ReadyForReview,
        },
    ];
    let r = carry_out(&merged, 7, Some(&ring));
    assert_eq!(r.chime, Some(Chime::Finished));
    assert_eq!(r.peek, Some((ring, 7)));
    assert_eq!(carry_out(&[], 7, None), Default::default());
}

#[test]
fn deciding_one_transition_matches_the_policy_under_the_mapped_context() {
    for setting in ["never", "needsInput", "needsInputOrDone"] {
        for tr in [needs_you_now("s1"), review_now("s1"), failed_now("s1")] {
            let ctx = reaction_ctx_with(setting);
            assert_eq!(
                decide(&tr, &ctx),
                policy::decide(&tr, &policy_context(&ctx))
            );
        }
    }
}

#[test]
fn the_context_mapping_copies_each_switch() {
    let ctx = ReactionContext {
        sound: false,
        peek: false,
        full_screen: true,
        any_terminal_visible: true,
        looking_at: Some(true),
        panel: auto_panel(),
        ..reaction_ctx()
    };
    let mapped = policy_context(&ctx);
    assert!(!mapped.chimes);
    assert!(!mapped.peeks);
    assert!(mapped.full_screen);
    assert!(mapped.any_terminal_visible);
    assert!(mapped.terminal_focused);
    assert!(mapped.panel_open);
    assert!(mapped.ring_shown);
    // A hidden notch has no ring to unfold on.
    let mapped = policy_context(&ReactionContext {
        notch_hidden: true,
        ..reaction_ctx()
    });
    assert!(!mapped.ring_shown);
    let mapped = policy_context(&ReactionContext {
        ring_shown: false,
        ..reaction_ctx()
    });
    assert!(!mapped.ring_shown);
}
