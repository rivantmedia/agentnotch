//! The session store's review queue, persistence and attention transitions:
//! the store halves of Fix_FailedTurnTests, SessionStoreFlowTests'
//! `reviewStateSurvivesRestart`, A1_SessionStoreRegressionTests,
//! A1_ReviewFixesTests and BackgroundWaitTests' `theWaitSurvivesARelaunch`,
//! ported to `SessionStore::apply`, plus the Windows contract (the review
//! file is asked for as a `Job::Persist` on a `Tick`).
//!
//! Not ported here, with their owner:
//! - the transcript halves of `aRestoredFailureClearsOnceTheUserMovedOn`,
//!   `sessionsFoundIdleLongAfterLaunchAreNotInferredDone` and
//!   `sessionsThatWentIdleWhileTheAppWasDownAreInferredDone` (the first
//!   transcript sync reconciles the restored state and infers a completion):
//!   wp5-10; only what the store itself decides is checked here;
//! - `aFailureGetsOneSoftCueAndNeverOpensThePanel` (the reaction policy,
//!   WP6) and `aFailedRowIsDismissedWithCommandR...` (the panel's keys,
//!   WP10); the counts of `aFailedTurnCountsAsFailedNotNeedsYou` are WP6's,
//!   the store's part is that a failure is its own state.

mod sessions_support;

use agentnotch_engine::model::{AccountId, NeedsInputReason, SessionId, SessionState};
use agentnotch_engine::review::ReviewStore;
use agentnotch_engine::runtime_types::{
    AccountsChanged, Job, PersistFile, Release, ReviewAction, SessionEffects, SessionInput,
};
use agentnotch_engine::sessions::attention::{humanized_stop_error, StopErrorKind};
use agentnotch_engine::sessions::background::WaitTiming;
use agentnotch_engine::sessions::completion::CompletionTiming;
use sessions_support::{t0, Harness, PID};
use std::time::{Duration, SystemTime};

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn s1() -> SessionId {
    SessionId::from("s1")
}

/// A store that was handed an empty review file at `t0`.
fn started() -> Harness {
    let mut h = Harness::new();
    h.store.load_review(None, h.now);
    h
}

/// The review file the effects ask for, if they ask for it.
fn persisted(effects: &SessionEffects) -> Option<Vec<u8>> {
    effects.jobs.iter().find_map(|job| match job {
        Job::Persist {
            file: PersistFile::Review,
            bytes,
        } => Some(bytes.clone()),
        _ => None,
    })
}

/// A tick at the current time; the review file it asks for.
fn flush(h: &mut Harness) -> Vec<u8> {
    let effects = h.tick();
    persisted(&effects).expect("the tick asks for the review file")
}

/// The next run of the app, handed the file the previous one wrote.
fn relaunch(bytes: &[u8], now: SystemTime) -> Harness {
    let mut h = Harness::new();
    h.now = now;
    h.store.load_review(Some(bytes), now);
    h
}

fn record_of(bytes: &[u8], now: SystemTime) -> Option<agentnotch_engine::model::ReviewItem> {
    ReviewStore::load(Some(bytes), now).record("s1").cloned()
}

fn review(h: &mut Harness, action: ReviewAction) {
    h.apply(SessionInput::Review(action));
}

fn is_failed_with(state: &SessionState, expected: &str) -> bool {
    matches!(state, SessionState::Failed(NeedsInputReason::Error { text, .. }) if text == expected)
}

fn finished_turn(h: &mut Harness, message: &str) {
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some(message.into())
    });
}

// ---- Fix_FailedTurnTests ----

/// A failure is its own state, never amber "needs you" (the counts and the
/// dock badge that follow from it are WP6's), and its transition says so.
#[test]
fn a_failed_turn_counts_as_failed_not_needs_you() {
    let mut h = started();
    h.store.initial_scan_completed(h.now);
    h.now += secs(3);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    let failure = h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    });
    assert!(is_failed_with(&h.state(), "Rate limited"));
    assert_eq!(failure.transitions.len(), 1);
    assert!(failure.transitions[0].is_failure());

    let question = needs_you(&mut h, "s2");
    assert!(matches!(
        h.store.view(&SessionId::from("s2")).unwrap().state,
        SessionState::NeedsYou(_)
    ));
    assert_eq!(question.transitions.len(), 1);
    assert!(!question.transitions[0].is_failure());
}

#[test]
fn a_dismissed_failure_stays_dismissed_across_a_relaunch() {
    let mut h = started();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    });
    assert!(h.session().unwrap().has_failed_turn());
    // The failure is on disk while it counts.
    let before = flush(&mut h);
    assert!(record_of(&before, h.now).unwrap().stop_error.is_some());

    review(&mut h, ReviewAction::DismissFailure(s1()));
    let dismissed = h.session().unwrap();
    assert!(!dismissed.has_failed_turn());
    assert!(!matches!(h.state(), SessionState::Failed(_)));
    h.now += secs(2);
    let bytes = flush(&mut h);
    assert!(record_of(&bytes, h.now).is_none_or(|record| record.stop_error.is_none()));

    // Next run: not restored as failed.
    let mut second = relaunch(&bytes, h.now + secs(1));
    second.registry("idle", second.now + secs(1));
    assert!(!second.session().unwrap().has_failed_turn());
}

#[test]
fn a_failure_newer_than_the_click_stays() {
    let mut h = started();
    h.hook("UserPromptSubmit", "processing");
    h.now = t0() + secs(5);
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("overloaded".into())
    });
    h.now = t0() + secs(1);
    review(&mut h, ReviewAction::DismissFailure(s1()));
    assert!(h.session().unwrap().has_failed_turn());
}

#[test]
fn mark_all_reviewed_commits_with_the_click_time() {
    let mut h = started();
    finished_turn(&mut h, "Done");
    assert_eq!(h.state(), SessionState::ReadyForReview);
    let click = t0() + secs(1_000);
    review(
        &mut h,
        ReviewAction::MarkAll {
            sessions: vec![s1()],
            at: click,
        },
    );
    assert_eq!(h.session().unwrap().reviewed_at, Some(click));
    assert_eq!(h.state(), SessionState::Idle);
}

// ---- SessionStoreFlowTests ----

#[test]
fn review_state_survives_restart() {
    let mut first = started();
    first.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    let stop = first.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Shipped".into())
    });
    assert_eq!(first.state(), SessionState::ReadyForReview);
    assert_eq!(stop.persist_review, Some(Duration::ZERO));
    let bytes = flush(&mut first);

    // A new app run discovers the session through the registry.
    let mut second = relaunch(&bytes, first.now + secs(60));
    second.registry("idle", second.now);
    assert_eq!(second.state(), SessionState::ReadyForReview);
    assert_eq!(
        second.session().unwrap().last_assistant_message.as_deref(),
        Some("Shipped")
    );
}

#[test]
fn a_queue_written_by_one_store_is_restored_by_a_new_one() {
    let mut first = started();
    // s1: finished, unreviewed. s2: finished and reviewed. s3: failed.
    // s4: waiting on a workflow.
    for id in ["s1", "s2", "s3", "s4"] {
        first.hook_with("UserPromptSubmit", "processing", |b| {
            b.session_id = id.into();
            b.source = Some("user".into());
        });
    }
    first.hook_with("Stop", "waiting_for_input", |b| {
        b.session_id = "s1".into();
        b.last_assistant_message = Some("One".into());
    });
    first.hook_with("Stop", "waiting_for_input", |b| {
        b.session_id = "s2".into();
        b.last_assistant_message = Some("Two".into());
    });
    let reviewed_at = first.now + secs(1);
    first.apply(SessionInput::Review(ReviewAction::MarkReviewed {
        session: "s2".into(),
        at: reviewed_at,
    }));
    first.hook_with("StopFailure", "waiting_for_input", |b| {
        b.session_id = "s3".into();
        b.stop_error = Some("rate_limit".into());
    });
    first.hook_with("Stop", "waiting_for_input", |b| {
        b.session_id = "s4".into();
        b.background_task_types = Some(vec!["workflow".into()]);
    });
    let bytes = flush(&mut first);

    let mut second = relaunch(&bytes, first.now + secs(60));
    for id in ["s1", "s2", "s3", "s4"] {
        let at = second.now;
        let entry =
            sessions_support::registry_entry(id, PID + id[1..].parse::<u32>().unwrap(), "idle", at);
        second.registry_entries(sessions_support::FOLDER, false, vec![entry.clone()]);
        second.now += ms(1);
    }
    let state = |h: &Harness, id: &str| h.store.view(&SessionId::from(id)).unwrap().state;
    assert_eq!(state(&second, "s1"), SessionState::ReadyForReview);
    assert_eq!(state(&second, "s2"), SessionState::Idle);
    assert!(is_failed_with(&state(&second, "s3"), "Rate limited"));
    let s4 = second.store.session(&SessionId::from("s4")).unwrap();
    assert_eq!(s4.background_agent_types, vec!["workflow".to_owned()]);
    assert!(s4.background_wait_since.is_some());
}

// ---- A1_SessionStoreRegressionTests ----

#[test]
fn a_failure_keeps_the_last_reply_and_survives_a_restart() {
    let mut first = started();
    first.hook("UserPromptSubmit", "processing");
    first.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Real answer".into())
    });
    first.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("loop_wakeup".into())
    });
    first.hook_with("StopFailure", "waiting_for_input", |b| {
        b.last_assistant_message = Some("API Error: Rate limit reached".into());
        b.stop_error = Some("rate_limit".into());
    });
    let failed = first.session().unwrap();
    assert_eq!(
        failed.last_assistant_message.as_deref(),
        Some("Real answer")
    );
    assert_eq!(failed.stop_error_kind(), Some(StopErrorKind::RateLimit));
    assert!(failed.has_failed_turn());
    assert!(matches!(first.state(), SessionState::Failed(_)));
    let bytes = flush(&mut first);

    // Next run: rediscovered through the registry, still failed.
    let mut second = relaunch(&bytes, first.now + secs(60));
    second.registry("idle", second.now);
    let restored = second.session().unwrap();
    assert!(is_failed_with(
        &second.state(),
        &humanized_stop_error(Some("rate_limit"))
    ));
    assert_eq!(restored.stop_error_code.as_deref(), Some("rate_limit"));
}

/// The store's part: what the record held is restored as a failure that
/// stands until the first transcript sync finds the user moved on (wp5-10).
#[test]
fn a_restored_failure_is_a_failure_until_the_user_moved_on() {
    let failed_at = t0()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let file = format!(
        r#"{{"version":2,"lastAliveAt":{alive},"sessions":{{"s1":{{"stopError":"Rate limited","stopErrorCode":"rate_limit","failedAt":{failed_at},"updatedAt":{failed_at}}}}}}}"#,
        alive = failed_at + 60.0
    );
    let mut h = relaunch(file.as_bytes(), t0() + secs(120));
    h.hook_with("SessionStart", "waiting_for_input", |b| {
        b.source = Some("resume".into())
    });
    let session = h.session().unwrap();
    assert!(session.has_failed_turn());
    assert_eq!(session.stop_error.as_deref(), Some("Rate limited"));
    assert_eq!(session.stop_error_code.as_deref(), Some("rate_limit"));
    assert!(is_failed_with(&h.state(), "Rate limited"));
    // Not rewritten for merely being restored.
    assert!(h.last.persist_review.is_none());
}

#[test]
fn completions_are_written_at_once() {
    let mut h = started();
    // The first tick is the launch heartbeat.
    flush(&mut h);
    h.hook("UserPromptSubmit", "processing");
    let stop = h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Saved".into())
    });
    assert_eq!(stop.persist_review, Some(Duration::ZERO));
    // The runtime ticks at the deadline, which is now.
    assert!(h.store.next_deadline().unwrap() <= h.now);
    let bytes = flush(&mut h);
    assert!(record_of(&bytes, h.now).unwrap().completed_at.is_some());
    // Written: nothing more is due.
    assert!(persisted(&h.tick()).is_none());
}

#[test]
fn review_marks_wait_for_the_debounce() {
    let mut h = started();
    finished_turn(&mut h, "Saved");
    flush(&mut h);
    let marked_at = h.now;
    h.step = Duration::ZERO;
    let effects = h.apply(SessionInput::Review(ReviewAction::MarkReviewed {
        session: s1(),
        at: marked_at,
    }));
    assert_eq!(effects.persist_review, Some(secs(1)));
    // (The quick registry read after the Stop may be due sooner.)
    assert!(h.store.next_deadline().unwrap() <= marked_at + secs(1));
    h.now = marked_at + ms(999);
    assert!(persisted(&h.tick()).is_none());
    h.now = marked_at + secs(1);
    let bytes = flush(&mut h);
    assert!(record_of(&bytes, h.now).unwrap().reviewed_at.is_some());
}

#[test]
fn the_heartbeat_comes_every_30_seconds() {
    let mut h = started();
    h.step = Duration::ZERO;
    let launch = h.now;
    // The file is written at the start, then every 30 seconds.
    flush(&mut h);
    assert_eq!(h.store.next_deadline(), Some(launch + secs(15)));
    h.now = launch + secs(15);
    assert!(persisted(&h.tick()).is_none());
    assert_eq!(h.store.next_deadline(), Some(launch + secs(30)));
    h.now = launch + secs(29);
    assert!(persisted(&h.tick()).is_none());
    h.now = launch + secs(30);
    let bytes = flush(&mut h);
    // Every file carries the heartbeat of the run that wrote it.
    let store = ReviewStore::load(Some(&bytes), h.now);
    assert_eq!(store.last_alive_at(), Some(launch + secs(30)));
    assert_eq!(h.store.next_deadline(), Some(launch + secs(60)));
}

#[test]
fn a_store_never_given_the_file_writes_nothing() {
    let mut h = Harness::new();
    finished_turn(&mut h, "Saved");
    // The record is kept and announced, but no job and no deadline asks for it.
    assert_eq!(h.last.persist_review, Some(Duration::ZERO));
    for _ in 0..3 {
        h.now += secs(60);
        assert!(h
            .tick()
            .jobs
            .iter()
            .all(|job| !matches!(job, Job::Persist { .. })));
    }
}

#[test]
fn mark_viewed_reviews_only_that_completion() {
    let mut h = started();
    h.step = Duration::ZERO;
    let t = t0();
    h.now = t;
    h.hook("UserPromptSubmit", "processing");
    h.now = t + secs(1);
    h.hook("Stop", "waiting_for_input");
    review(
        &mut h,
        ReviewAction::MarkViewed {
            session: s1(),
            completed_at: t,
        },
    );
    assert_eq!(h.state(), SessionState::ReadyForReview);
    review(
        &mut h,
        ReviewAction::MarkViewed {
            session: s1(),
            completed_at: t + secs(1),
        },
    );
    assert_eq!(h.state(), SessionState::Idle);
}

#[test]
fn a_wait_starts_when_its_event_arrived() {
    let mut h = started();
    h.step = Duration::ZERO;
    let t = t0();
    h.now = t;
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.now = t + secs(5);
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("elicitation_dialog".into());
        b.message = Some("Pick a file".into());
    });
    assert_eq!(h.view().unwrap().waiting_since, Some(t + secs(5)));
    // Another reason while still waiting keeps the original time.
    h.now = t + secs(9);
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("agent_needs_input".into());
        b.message = Some("Choose one".into());
    });
    assert_eq!(h.view().unwrap().waiting_since, Some(t + secs(5)));
    h.now = t + secs(20);
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    });
    assert_eq!(h.view().unwrap().waiting_since, Some(t + secs(5)));
}

#[test]
fn a_review_mark_covers_only_what_finished_before_the_click() {
    let mut h = started();
    h.step = Duration::ZERO;
    let t = t0();
    h.now = t;
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.now = t + secs(2);
    h.hook("Stop", "waiting_for_input");
    // Clicked before the Stop was processed: that turn is still unseen.
    review(
        &mut h,
        ReviewAction::MarkReviewed {
            session: s1(),
            at: t + secs(1),
        },
    );
    assert_eq!(h.state(), SessionState::ReadyForReview);
    review(
        &mut h,
        ReviewAction::MarkReviewed {
            session: s1(),
            at: t + secs(3),
        },
    );
    assert_eq!(h.state(), SessionState::Idle);
    // A later, earlier-dated mark never takes a review back.
    review(
        &mut h,
        ReviewAction::MarkReviewed {
            session: s1(),
            at: t,
        },
    );
    assert_eq!(h.session().unwrap().reviewed_at, Some(t + secs(3)));
}

// ---- A1_ReviewFixesTests ----

/// The review file of a previous run that was last alive `alive_at`.
fn previous_run(alive_at: SystemTime) -> Vec<u8> {
    let secs = alive_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    format!(r#"{{"version":2,"lastAliveAt":{secs},"sessions":{{}}}}"#).into_bytes()
}

/// The store's part of `sessionsFoundIdleLongAfterLaunchAreNotInferredDone`:
/// no transcript check is owed for a session that went idle hours into this
/// run, and nothing is made up (the transcript half is wp5-10's).
#[test]
fn sessions_found_idle_long_after_launch_are_not_inferred_done() {
    let alive_at = t0();
    let launched_at = alive_at + secs(60);
    let mut h = relaunch(&previous_run(alive_at), launched_at);
    h.now = launched_at + secs(901);
    h.registry("idle", launched_at + secs(901));
    let session = h.session().unwrap();
    assert!(session.completed_at.is_none());
    assert!(session.completion_check_since.is_none());
    assert_eq!(h.state(), SessionState::Idle);
}

/// The store's part of `sessionsThatWentIdleWhileTheAppWasDownAreInferredDone`:
/// the first transcript sync is asked to look for a finished turn newer than
/// the previous run's last heartbeat.
#[test]
fn sessions_that_went_idle_while_the_app_was_down_are_checked_for_a_finished_turn() {
    let alive_at = t0();
    let launched_at = alive_at + secs(3600);
    let mut h = relaunch(&previous_run(alive_at), launched_at);
    h.now = launched_at + secs(1);
    h.registry("idle", alive_at + secs(601));
    let since = h.session().unwrap().completion_check_since.unwrap();
    // The file keeps epoch seconds as doubles.
    let apart = since
        .duration_since(alive_at)
        .or_else(|error| Ok::<_, ()>(error.duration()))
        .unwrap();
    assert!(apart < ms(1));
}

#[test]
fn mark_all_reviewed_keeps_what_finished_after_the_click() {
    let mut h = started();
    h.step = Duration::ZERO;
    let t = t0();
    h.now = t;
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.now = t + secs(2);
    h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // Clicked before the turn finished; processed after.
    let mark_all = |at| ReviewAction::MarkAll {
        sessions: vec![s1()],
        at,
    };
    review(&mut h, mark_all(t + secs(1)));
    assert_eq!(h.state(), SessionState::ReadyForReview);
    review(&mut h, mark_all(t + secs(3)));
    assert_eq!(h.state(), SessionState::Idle);
    // An older click never moves a review back.
    review(&mut h, mark_all(t));
    assert_eq!(h.session().unwrap().reviewed_at, Some(t + secs(3)));
}

#[test]
fn the_error_kind_goes_with_the_failure() {
    let mut h = started();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    });
    assert_eq!(
        h.session().unwrap().stop_error_kind(),
        Some(StopErrorKind::RateLimit)
    );
    // A late main-session event settles the needs-you state; the kind goes
    // with it.
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_late".into());
    });
    let session = h.session().unwrap();
    assert!(!session.has_failed_turn());
    assert_eq!(session.stop_error_kind(), None);
    // And the failure is not kept for the next launch.
    let bytes = flush(&mut h);
    assert!(record_of(&bytes, h.now).is_none_or(|record| record.stop_error.is_none()));
}

// ---- BackgroundWaitTests, SessionStoreFlowTests: the review halves ----

fn waiting(registry_grace: u64) -> Harness {
    let mut h = Harness::with_timing(
        CompletionTiming::IMMEDIATE,
        WaitTiming {
            registry_grace: secs(registry_grace),
            quiet_timeout: secs(30 * 60),
        },
    );
    h.store.load_review(None, h.now);
    h
}

#[test]
fn a_failed_turn_keeps_waiting_behind_its_error_until_it_is_dismissed() {
    let mut h = waiting(10);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.pid = Some(PID);
        b.prompt = Some("go".into());
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.pid = Some(PID);
        b.background_task_types = Some(vec!["workflow".into()]);
    });
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.pid = Some(PID);
        b.stop_error = Some("rate_limit".into());
    });
    assert!(is_failed_with(&h.state(), "Rate limited"));
    assert!(h.session().unwrap().background_wait_since.is_some());

    review(&mut h, ReviewAction::DismissFailure(s1()));
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(
        h.session()
            .unwrap()
            .background_wait_description()
            .as_deref(),
        Some("1 workflow")
    );
}

/// The app quit and came back while a workflow ran.
#[test]
fn the_wait_survives_a_relaunch() {
    let mut first = waiting(0);
    let start = first.now;
    first.hook_with("UserPromptSubmit", "processing", |b| {
        b.pid = Some(PID);
        b.prompt = Some("go".into());
    });
    first.registry("busy", start);
    first.hook_with("Stop", "waiting_for_input", |b| {
        b.pid = Some(PID);
        b.background_task_types = Some(vec!["workflow".into()]);
    });
    first.hook_with("Notification", "waiting_for_input", |b| {
        b.pid = Some(PID);
        b.notification_type = Some("idle_prompt".into());
    });
    let bytes = flush(&mut first);

    let mut second = Harness::with_timing(
        CompletionTiming::IMMEDIATE,
        WaitTiming {
            registry_grace: Duration::ZERO,
            quiet_timeout: secs(30 * 60),
        },
    );
    second.now = first.now + secs(120);
    second.store.load_review(Some(&bytes), second.now);
    second.registry("busy", second.now - secs(60));
    assert_eq!(second.state(), SessionState::Working);
    assert_eq!(
        second
            .session()
            .unwrap()
            .background_wait_description()
            .as_deref(),
        Some("1 workflow")
    );

    second.registry("idle", second.now);
    assert_eq!(second.state(), SessionState::ReadyForReview);
}

#[test]
fn a_user_prompt_is_the_user_looking_and_a_click_marks_the_review() {
    let mut h = started();
    finished_turn(&mut h, "Done: tests pass");
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("loop_wakeup".into())
    });
    h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::ReadyForReview);

    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    let now = h.now;
    review(
        &mut h,
        ReviewAction::MarkReviewed {
            session: s1(),
            at: now,
        },
    );
    assert_eq!(h.state(), SessionState::Working);
}

// ---- the other review actions ----

#[test]
fn reset_marks_every_finished_session_reviewed() {
    let mut h = started();
    for id in ["s1", "s2"] {
        h.hook_with("UserPromptSubmit", "processing", |b| {
            b.session_id = id.into();
            b.source = Some("user".into());
        });
        h.hook_with("Stop", "waiting_for_input", |b| b.session_id = id.into());
    }
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.session_id = "s3".into();
        b.source = Some("user".into());
    });
    review(&mut h, ReviewAction::Reset);
    let state = |id: &str| h.store.view(&SessionId::from(id)).unwrap().state;
    assert_eq!(state("s1"), SessionState::Idle);
    assert_eq!(state("s2"), SessionState::Idle);
    assert_eq!(state("s3"), SessionState::Working);
}

#[test]
fn turning_hooks_off_forgets_that_hooks_report_the_sessions() {
    let mut h = started();
    h.hook("UserPromptSubmit", "processing");
    assert!(h.view().unwrap().is_hook_backed);
    review(&mut h, ReviewAction::HooksTurnedOff);
    assert!(!h.view().unwrap().is_hook_backed);
    assert!(h.session().unwrap().last_hook_event_at.is_none());
}

#[test]
fn removed_accounts_lose_their_sessions() {
    let mut h = started();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.config_dir_env = Some("/home/me/.claude-work".into());
        b.transcript_path = Some("/home/me/.claude-work/projects/-tmp-proj/s1.jsonl".into());
    });
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.session_id = "s2".into();
    });
    let work = h.session().unwrap().account.clone().unwrap();
    assert_ne!(
        Some(&work),
        h.store.session(&"s2".into()).unwrap().account.as_ref()
    );

    // Other changes leave sessions alone.
    h.apply(SessionInput::AccountsChanged(AccountsChanged {
        rings: true,
        folders: true,
        ..AccountsChanged::default()
    }));
    assert!(h.session().is_some());

    let effects = h.apply(SessionInput::AccountsChanged(AccountsChanged {
        removed_folders: vec![work, AccountId("/nowhere".into())],
        ..AccountsChanged::default()
    }));
    assert!(h.session().is_none());
    assert!(h.store.session(&"s2".into()).is_some());
    assert!(effects.changed);
    assert!(effects.release.contains(&Release::Session(s1())));
}

/// The record is not kept for a failure that no longer counts, and a record
/// restored as it was is not written again.
#[test]
fn nothing_is_written_for_a_session_that_only_was_found() {
    let mut first = started();
    finished_turn(&mut first, "Done");
    let bytes = flush(&mut first);

    let mut second = relaunch(&bytes, first.now + secs(60));
    second.registry("idle", second.now);
    assert_eq!(second.state(), SessionState::ReadyForReview);
    assert!(second.last.persist_review.is_none());
}

// ---- transitions ----

fn needs_you(h: &mut Harness, id: &str) -> SessionEffects {
    h.hook_with("Notification", "notification", |b| {
        b.session_id = id.into();
        b.notification_type = Some("permission_prompt".into());
        b.message = Some("Claude needs your permission to use Bash".into());
    })
}

#[test]
fn no_transitions_during_the_baseline_then_news() {
    let mut h = started();
    // The launch baseline is open: what is found now is not news.
    let early = needs_you(&mut h, "s1");
    assert!(early.transitions.is_empty());
    assert!(matches!(h.state(), SessionState::NeedsYou(_)));

    // Every registry was read: the baseline closes after the settle.
    let scanned = h.now;
    h.store.initial_scan_completed(scanned);
    h.now = scanned + ms(1_999);
    assert!(needs_you(&mut h, "s2").transitions.is_empty());
    h.now = scanned + secs(2);
    let late = needs_you(&mut h, "s3");
    assert_eq!(late.transitions.len(), 1);
    let transition = &late.transitions[0];
    assert_eq!(transition.session.id, SessionId::from("s3"));
    assert!(transition.became_needs_you());
    assert_eq!(transition.from, None);
}

#[test]
fn the_baseline_closes_by_itself_after_fifteen_seconds() {
    let mut h = started();
    let launch = h.now;
    flush(&mut h);
    assert_eq!(h.store.next_deadline(), Some(launch + secs(15)));
    h.now = launch + secs(14);
    assert!(needs_you(&mut h, "s1").transitions.is_empty());
    h.now = launch + secs(15);
    // The tick at the deadline closes it quietly; the next change is news.
    assert!(h.tick().transitions.is_empty());
    assert!(!needs_you(&mut h, "s2").transitions.is_empty());
}

#[test]
fn a_quiet_completion_is_never_news_and_a_real_one_is() {
    let mut h = started();
    h.store.initial_scan_completed(h.now);
    h.now += secs(3);

    // A /loop tick finishes: it stays in the review queue, unannounced.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("loop_wakeup".into())
    });
    let quiet = h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert!(h.view().unwrap().completion_quiet);
    assert!(quiet
        .transitions
        .iter()
        .all(|t| !t.became_ready_for_review()));

    // The user's own turn finishes: announced.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into());
        b.session_id = "s2".into();
    });
    let real = h.hook_with("Stop", "waiting_for_input", |b| b.session_id = "s2".into());
    assert!(real
        .transitions
        .iter()
        .any(|t| t.session.id == SessionId::from("s2") && t.became_ready_for_review()));
}

#[test]
fn a_completion_restored_from_before_the_launch_is_not_news() {
    let mut first = started();
    finished_turn(&mut first, "Done");
    let bytes = flush(&mut first);

    let mut second = relaunch(&bytes, first.now + secs(60));
    second.store.initial_scan_completed(second.now);
    second.now += secs(3);
    let found = second.registry("idle", second.now);
    assert_eq!(second.state(), SessionState::ReadyForReview);
    assert!(found
        .transitions
        .iter()
        .all(|t| !t.became_ready_for_review()));
}

#[test]
fn the_tracker_follows_from_the_first_input_without_a_review_file() {
    let mut h = Harness::new();
    assert!(needs_you(&mut h, "s1").transitions.is_empty());
    h.now += secs(16);
    assert!(!needs_you(&mut h, "s2").transitions.is_empty());
}

#[test]
fn views_after_the_session_ended_do_not_leave_transitions_behind() {
    let mut h = started();
    h.store.initial_scan_completed(h.now);
    h.now += secs(3);
    needs_you(&mut h, "s1");
    let ended = h.hook("SessionEnd", "ended");
    // The session went away: the tracker forgets it without a transition.
    assert!(ended.transitions.is_empty());
    assert!(h.session().is_none());
}
