//! AttentionTracker and AttentionNews (HS§7): the launch baseline, quiet and
//! pre-launch completions, and the one rule for what a change announces.
//!
//! Ported from Fix_AttentionNewsTests (theRule, bothStreamsAgree: the hub's
//! half, `ClaudeControlHub.transitions`, is WP7's; here the tracker's banners
//! are checked against the rule itself) and A1_AttentionAndReviewTests
//! (accountsFoundAtLaunchDoNotAlert, completionsFromBeforeLaunchAndQuiet
//! OnesAreSilent, failuresAreFlagged).

use agentnotch_engine::attention::news::{is_news, kinds, kinds_between, NewsKind};
use agentnotch_engine::attention::tracker::{
    AttentionTracker, MAX_BASELINE_INTERVAL, SETTLE_INTERVAL,
};
use agentnotch_engine::model::{
    AttentionTransition, NeedsInputReason, SessionId, SessionState, SessionView,
};
use agentnotch_engine::sessions::attention::{sort_rank, StopErrorKind};
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use NewsKind::{NeedsInput, ReadyForReview, Resolved};

fn launch() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn secs(n: i64) -> Duration {
    Duration::from_secs(n.unsigned_abs())
}

fn permission(tool: &str) -> SessionState {
    SessionState::NeedsYou(NeedsInputReason::Permission {
        tool: Some(tool.into()),
    })
}

fn failure(text: &str) -> SessionState {
    SessionState::Failed(NeedsInputReason::Error {
        text: text.into(),
        code: None,
    })
}

fn session(
    id: &str,
    state: SessionState,
    completed_at: Option<SystemTime>,
    quiet: bool,
) -> SessionView {
    let mut view = SessionView::new(id, format!("/tmp/{id}"), launch());
    view.state = state;
    view.completed_at = completed_at;
    view.completion_quiet = quiet;
    view
}

fn plain(id: &str, state: SessionState, completed_at: Option<SystemTime>) -> SessionView {
    session(id, state, completed_at, false)
}

fn ids(changes: &[AttentionTransition]) -> BTreeSet<String> {
    changes.iter().map(|c| c.session.id.to_string()).collect()
}

// ---- Fix_AttentionNewsTests ----

fn rule(
    from: Option<&SessionState>,
    to: Option<&SessionState>,
    quiet: bool,
    completed_at: Option<SystemTime>,
) -> Vec<NewsKind> {
    kinds_between(
        from,
        to,
        quiet,
        Some(completed_at.unwrap_or(launch() + secs(60))),
        Some(launch()),
    )
}

#[test]
fn the_rule() {
    use SessionState::{Idle, ReadyForReview as Ready, Working};
    let bash = permission("Bash");
    let edit = permission("Edit");
    fn r(from: Option<&SessionState>, to: Option<&SessionState>) -> Vec<NewsKind> {
        rule(from, to, false, None)
    }
    assert_eq!(r(Some(&Working), Some(&bash)), [NeedsInput]);
    assert_eq!(r(None, Some(&bash)), [NeedsInput], "first seen, waiting");
    assert_eq!(r(Some(&bash), Some(&edit)), [NeedsInput], "another request");
    assert_eq!(r(Some(&bash), Some(&bash)), []);
    assert_eq!(r(Some(&bash), Some(&Working)), [Resolved]);
    assert_eq!(r(Some(&bash), None), [Resolved], "went away");
    assert_eq!(r(Some(&bash), Some(&Ready)), [Resolved, ReadyForReview]);
    assert_eq!(r(Some(&Working), Some(&Ready)), [ReadyForReview]);
    assert_eq!(
        r(None, Some(&Ready)),
        [ReadyForReview],
        "first seen, finished now"
    );
    assert_eq!(rule(Some(&Working), Some(&Ready), true, None), []);
    assert_eq!(
        rule(
            Some(&Working),
            Some(&Ready),
            false,
            Some(launch() - secs(1))
        ),
        []
    );
    assert_eq!(r(Some(&Ready), Some(&Idle)), [Resolved]);
    assert_eq!(r(Some(&Ready), None), [Resolved]);
    assert_eq!(r(Some(&Ready), Some(&Ready)), []);
    assert_eq!(r(None, Some(&Working)), []);
    assert_eq!(r(Some(&Idle), Some(&Working)), []);
    assert_eq!(r(Some(&Working), None), []);

    // A failed turn is needing the user: with its own error as the reason.
    let rate = failure("Rate limited");
    assert_eq!(r(Some(&Working), Some(&rate)), [NeedsInput]);
    assert_eq!(
        r(Some(&bash), Some(&rate)),
        [NeedsInput],
        "another reason, same bucket"
    );
    assert_eq!(r(Some(&rate), Some(&failure("Overloaded"))), [NeedsInput]);
    assert_eq!(r(Some(&rate), Some(&Working)), [Resolved]);
    // No completion time or no launch time: the pre-launch rule can't apply.
    assert_eq!(
        kinds_between(Some(&Working), Some(&Ready), false, None, Some(launch())),
        [ReadyForReview]
    );
    assert_eq!(
        kinds_between(
            Some(&Working),
            Some(&Ready),
            false,
            Some(launch() - secs(1)),
            None
        ),
        [ReadyForReview]
    );
}

#[test]
fn a_transitions_kinds_use_its_sessions_completion() {
    let ready = plain("a", SessionState::ReadyForReview, Some(launch() + secs(5)));
    let transition = AttentionTransition {
        session: ready.clone(),
        from: Some(SessionState::Working),
        to: SessionState::ReadyForReview,
    };
    assert_eq!(kinds(&transition, Some(launch()), false), [ReadyForReview]);
    assert_eq!(kinds(&transition, Some(launch()), true), []);
    assert!(is_news(&transition, Some(launch())));
    assert!(!is_news(
        &AttentionTransition {
            session: session(
                "a",
                SessionState::ReadyForReview,
                Some(launch() + secs(5)),
                true
            ),
            ..transition.clone()
        },
        Some(launch())
    ));
    // Every change but a rejected completion is passed on.
    let working = AttentionTransition {
        session: plain("a", SessionState::Working, None),
        from: Some(SessionState::ReadyForReview),
        to: SessionState::Working,
    };
    assert!(is_news(&working, Some(launch())));
}

#[test]
fn both_streams_agree() {
    use SessionState::{ReadyForReview as Ready, Working};
    let mut tracker = AttentionTracker::new(launch());
    tracker.initial_scan_completed(launch());

    let first = [plain("a", Working, None), plain("b", Working, None)];
    let second = [
        plain(
            "a",
            SessionState::NeedsYou(NeedsInputReason::Question),
            None,
        ),
        plain("b", Ready, Some(launch() + secs(120))),
        plain("new", permission("Bash"), None),
    ];
    let after_baseline = launch() + SETTLE_INTERVAL + secs(1);
    tracker.update(&first, after_baseline);
    let banners = tracker.update(&second, after_baseline);

    let mut announced = BTreeSet::new();
    for change in &banners {
        if change.became_needs_you() {
            announced.insert(format!("{}:needsInput", change.session.id));
        }
        if change.became_ready_for_review() {
            announced.insert(format!("{}:readyForReview", change.session.id));
        }
    }
    // What the rule says for the same changes (the hub's stream uses it).
    let mut by_rule = BTreeSet::new();
    for (view, before) in second.iter().zip([Some(&first[0]), Some(&first[1]), None]) {
        for kind in kinds_between(
            before.map(|v| &v.state),
            Some(&view.state),
            view.completion_quiet,
            view.completed_at,
            Some(launch()),
        ) {
            match kind {
                NeedsInput => by_rule.insert(format!("{}:needsInput", view.id)),
                ReadyForReview => by_rule.insert(format!("{}:readyForReview", view.id)),
                Resolved => false,
            };
        }
    }
    assert_eq!(announced, by_rule);
    let expected: BTreeSet<String> = ["a:needsInput", "b:readyForReview", "new:needsInput"]
        .map(String::from)
        .into();
    assert_eq!(announced, expected);
}

// ---- A1_AttentionAndReviewTests ----

#[test]
fn accounts_found_at_launch_do_not_alert() {
    use SessionState::{Idle, ReadyForReview as Ready, Working};
    let t = launch();
    let mut tracker = AttentionTracker::new(t);
    let dialog = || {
        SessionState::NeedsYou(NeedsInputReason::Dialog {
            detail: "permission prompt".into(),
        })
    };
    let mut received = Vec::new();

    // Account A's registry, then account B's, each its own snapshot.
    received.extend(tracker.update(&[plain("a1", Idle, None)], t));
    received.extend(tracker.update(
        &[
            plain("a1", Idle, None),
            plain("b1", dialog(), None),
            plain("b2", Ready, Some(t - secs(86_400))),
        ],
        t + Duration::from_millis(500),
    ));
    tracker.initial_scan_completed(t + secs(1));
    // Late first syncs within the settle window are still baseline.
    received.extend(tracker.update(
        &[
            plain("a1", Ready, Some(t - secs(60))),
            plain("b1", dialog(), None),
            plain("b2", Ready, Some(t - secs(86_400))),
        ],
        t + Duration::from_millis(1500),
    ));
    assert!(received.is_empty());

    // After the baseline, a real change is news.
    let later = tracker.update(
        &[
            plain("a1", Ready, Some(t - secs(60))),
            plain("b1", Working, None),
            plain("b2", Ready, Some(t - secs(86_400))),
            plain(
                "c1",
                SessionState::NeedsYou(NeedsInputReason::Question),
                None,
            ),
        ],
        t + secs(10),
    );
    assert_eq!(ids(&later), ["b1", "c1"].map(String::from).into());
}

#[test]
fn completions_from_before_launch_and_quiet_ones_are_silent() {
    use SessionState::{Idle, ReadyForReview as Ready, Working};
    let now = launch() + secs(100);
    let mut tracker = AttentionTracker::new(launch());
    tracker.initial_scan_completed(launch());

    tracker.update(
        &[
            plain("inferred", Idle, None),
            plain("quiet", Working, None),
            plain("done", Working, None),
        ],
        now,
    );
    let received = tracker.update(
        &[
            // Found finished on its first sync: it finished while the app was down.
            plain("inferred", Ready, Some(launch() - secs(30))),
            // A /loop tick: the loop's last turn is announced instead.
            session("quiet", Ready, Some(now), true),
            plain("done", Ready, Some(now)),
        ],
        now,
    );
    let ready: Vec<_> = received
        .iter()
        .filter(|c| c.became_ready_for_review())
        .map(|c| c.session.id.to_string())
        .collect();
    assert_eq!(ready, ["done"]);
    // The silent ones are recorded all the same: no second look at them.
    assert_eq!(tracker.attention(&SessionId::from("quiet")), Some(&Ready));
}

#[test]
fn failures_are_flagged() {
    let view = SessionView::new("f", "/tmp", launch());
    let failed = AttentionTransition {
        session: view.clone(),
        from: Some(SessionState::Working),
        to: failure("Rate limited"),
    };
    let question = AttentionTransition {
        session: view,
        from: Some(SessionState::Working),
        to: SessionState::NeedsYou(NeedsInputReason::Question),
    };
    assert!(failed.is_failure());
    assert!(!question.is_failure());
    // The pieces of the Mac test that live in the pure session modules.
    let error = NeedsInputReason::Error {
        text: "x".into(),
        code: None,
    };
    assert!(sort_rank(&error) > sort_rank(&NeedsInputReason::Question));
    assert!(error.is_error());
    assert!(!NeedsInputReason::Question.is_error());
    assert!(StopErrorKind::from_code(Some("rate_limit"))
        .unwrap()
        .is_transient());
    assert!(!StopErrorKind::from_code(Some("billing_error"))
        .unwrap()
        .is_transient());
    assert_eq!(
        StopErrorKind::from_code(Some("oauth_org_not_allowed")),
        Some(StopErrorKind::Authentication)
    );
}

// ---- the tracker's own rules ----

#[test]
fn the_baseline_closes_two_seconds_after_the_registries_or_fifteen_after_start() {
    let t = launch();
    let mut tracker = AttentionTracker::new(t);
    // Before `start`, and before the scan: baseline.
    assert!(tracker.is_in_baseline(t + secs(1000)));
    tracker.start(t);
    assert!(tracker.is_in_baseline(t + MAX_BASELINE_INTERVAL - Duration::from_millis(1)));
    assert!(!tracker.is_in_baseline(t + MAX_BASELINE_INTERVAL));
    assert_eq!(MAX_BASELINE_INTERVAL, secs(15));
    assert_eq!(SETTLE_INTERVAL, secs(2));

    // Start is idempotent: a second call doesn't move the cap.
    tracker.start(t + secs(10));
    assert!(!tracker.is_in_baseline(t + secs(15)));

    // The scan's end plus the settle wins over the cap, and the first
    // report of it counts.
    let mut tracker = AttentionTracker::new(t);
    tracker.start(t);
    tracker.initial_scan_completed(t + secs(3));
    tracker.initial_scan_completed(t + secs(4));
    assert!(tracker.is_in_baseline(t + Duration::from_millis(4999)));
    assert!(!tracker.is_in_baseline(t + secs(5)));

    // Stopping forgets both: a new baseline on the next start.
    tracker.stop();
    assert!(tracker.is_in_baseline(t + secs(100)));
    tracker.start(t + secs(100));
    assert!(tracker.is_in_baseline(t + secs(110)));
    assert!(!tracker.is_in_baseline(t + secs(115)));
}

#[test]
fn a_session_that_went_away_is_new_when_it_comes_back() {
    use SessionState::Working;
    let t = launch();
    let mut tracker = AttentionTracker::new(t);
    tracker.initial_scan_completed(t);
    let now = t + secs(10);
    let waiting = || plain("s", permission("Bash"), None);
    assert_eq!(tracker.update(&[waiting()], now).len(), 1);
    assert!(
        tracker.update(&[waiting()], now).is_empty(),
        "no change, no news"
    );
    assert!(
        tracker.update(&[], now).is_empty(),
        "going away is not announced"
    );
    assert_eq!(tracker.attention(&SessionId::from("s")), None);
    let back = tracker.update(&[waiting()], now);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].from, None);
    let resolved = tracker.update(&[plain("s", Working, None)], now);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].from, Some(permission("Bash")));
}

#[test]
fn a_first_seen_session_after_the_baseline_is_news() {
    let t = launch();
    let mut tracker = AttentionTracker::new(t);
    tracker.initial_scan_completed(t);
    let now = t + secs(10);
    let changes = tracker.update(&[plain("n", SessionState::ReadyForReview, Some(now))], now);
    assert_eq!(changes.len(), 1);
    assert!(changes[0].became_ready_for_review());
    assert_eq!(tracker.launched_at(), t);
}
