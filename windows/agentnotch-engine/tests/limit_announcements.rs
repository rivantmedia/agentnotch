//! A usage limit is announced once per account until it lifts (Release 1.0.2,
//! `LimitAnnouncementTests`): not again on every retry, wake-up or /loop tick
//! that fails on it, nor for another session stopped by it, a session
//! reopened with its old failure, a relaunch, or a reset time that two usage
//! sources round differently.
//!
//! Ported: the pure rule (`LimitAnnouncementRuleTests`), the record's file
//! (`LimitAnnouncementStoreTests`), `RestoredFailureTests`,
//! `LimitWindowTests`, `LimitReactionTests` and `RingResetTimeTests`, plus the
//! toast rules the Mac's `NotificationService` applies. The hub's side (the
//! banner posted once, the chime once, the file written) is in `hub_control.rs`.
//!
//! Not ported, for want of a Windows counterpart: upstream's "limit reached"
//! card and its 100% banner (seams LIM1/LIM2, `LimitAlertTests`): the Windows
//! app has no usage-limit watcher, so the reaction is the limit's only
//! announcement when no banner shows.

mod sessions_support;

use agentnotch_engine::attention::news::{kinds_between_restoring, NewsKind};
use agentnotch_engine::attention::policy::repeats_limit_reaction;
use agentnotch_engine::control::limits::{
    window_key, Channel, Incident, LimitAnnouncementStore, LimitAnnouncements, UNKNOWN_ABSORBS_FOR,
    UNKNOWN_LIFETIME, UNKNOWN_WINDOW,
};
use agentnotch_engine::control::notifications::{
    toast_for, LimitBanners, LimitChange, LimitContext, LimitedSession,
};
use agentnotch_engine::model::RingStatus;
use agentnotch_engine::model::{
    AccountUsage, AttentionTransition, IdentityId, LimitHit, LimitWindowKind, NeedsInputReason,
    RingId, SessionId, SessionState, SessionView, UsageSource, UsageWindow,
};
use agentnotch_engine::platform::NotifyPermission;
use agentnotch_engine::runtime_types::{RingReading, ToastContext};
use agentnotch_engine::usage::ring_windows::{
    keeping_reset_times, lifted_limit_windows, windows, with_reset_times, RingWindow,
};
use sessions_support::Harness;
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn resets() -> SystemTime {
    now() + secs(3 * 3600)
}

/// A claim of `channel` with nothing else marked.
fn claim(
    state: &mut LimitAnnouncements,
    channel: Channel,
    ring: &str,
    window: &str,
    resets_at: Option<SystemTime>,
    at: SystemTime,
) -> bool {
    state.claim(channel, ring, window, resets_at, at, &[])
}

// ---- LimitAnnouncementRuleTests ----

#[test]
fn a_limit_is_announced_once_per_channel_until_its_window_resets() {
    let mut gate = LimitAnnouncements::default();
    let n = Channel::Notification;
    assert!(claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        now()
    ));
    // The retry five minutes later, the /loop tick after that.
    assert!(!claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        now() + secs(300)
    ));
    assert!(!claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        now() + secs(3600)
    ));
    // The chime and peek are a channel of their own: once too.
    let r = Channel::Reaction;
    assert!(claim(
        &mut gate,
        r,
        "work",
        "session",
        Some(resets()),
        now() + secs(1)
    ));
    assert!(!claim(
        &mut gate,
        r,
        "work",
        "session",
        Some(resets()),
        now() + secs(400)
    ));
    // The window reset, and the next one ran out: a new limit.
    let next = resets() + secs(5 * 3600);
    assert!(claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(next),
        resets() + secs(60)
    ));
}

#[test]
fn accounts_and_windows_are_apart() {
    let mut gate = LimitAnnouncements::default();
    let n = Channel::Notification;
    assert!(claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        now()
    ));
    assert!(claim(
        &mut gate,
        n,
        "personal",
        "session",
        Some(resets()),
        now()
    ));
    // The week running out too is news of its own.
    assert!(claim(
        &mut gate,
        n,
        "work",
        "weekly",
        Some(now() + secs(4 * 86_400)),
        now()
    ));
}

/// A turn fails on the limit before the readings say which window ran out:
/// that incident covers whichever window is named next, and takes its reset
/// time.
#[test]
fn a_limit_of_an_unknown_window_learns_its_window() {
    let mut gate = LimitAnnouncements::default();
    let n = Channel::Notification;
    assert!(claim(&mut gate, n, "work", UNKNOWN_WINDOW, None, now()));
    // The window named seconds later.
    assert!(!claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        now() + secs(5)
    ));
    assert_eq!(
        gate.incidents["work"],
        vec![Incident {
            window: "session".into(),
            resets_at: Some(resets()),
            started_at: now(),
            channels: [Channel::Notification].into(),
        }]
    );
    // It lasts until that reset now, not the unknown lifetime.
    assert!(!claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        now() + secs(2 * 3600)
    ));
    // A failure that names no window is covered by a known incident.
    assert!(!claim(
        &mut gate,
        n,
        "work",
        UNKNOWN_WINDOW,
        None,
        now() + secs(2 * 3600)
    ));
}

#[test]
fn a_limit_no_reading_shows_lasts_an_hour() {
    let mut gate = LimitAnnouncements::default();
    let n = Channel::Notification;
    assert!(claim(&mut gate, n, "work", UNKNOWN_WINDOW, None, now()));
    assert!(!claim(
        &mut gate,
        n,
        "work",
        UNKNOWN_WINDOW,
        None,
        now() + secs(59 * 60)
    ));
    assert!(claim(
        &mut gate,
        n,
        "work",
        UNKNOWN_WINDOW,
        None,
        now() + UNKNOWN_LIFETIME + secs(1)
    ));
    // A reset time already past says nothing about how long it lasts.
    let mut other = LimitAnnouncements::default();
    assert!(claim(
        &mut other,
        n,
        "work",
        "session",
        Some(now() - secs(60)),
        now()
    ));
    assert_eq!(other.incidents["work"][0].resets_at, None);
}

/// A banner that brings its own sound takes the reaction with it (the Mac's
/// card does; Windows' banner is silent, so nothing there marks it).
#[test]
fn a_notification_can_take_the_reaction_with_it() {
    let mut gate = LimitAnnouncements::default();
    assert!(gate.claim(
        Channel::Notification,
        "work",
        "session",
        Some(resets()),
        now(),
        &[Channel::Reaction]
    ));
    assert!(!claim(
        &mut gate,
        Channel::Reaction,
        "work",
        "session",
        Some(resets()),
        now()
    ));
    assert!(!claim(
        &mut gate,
        Channel::Notification,
        "work",
        "session",
        Some(resets()),
        now()
    ));
}

/// A failure that came before any reading named a window stands for the next
/// window named only while the readings may still be catching up; a window
/// that runs out later is a limit of its own.
#[test]
fn an_unknown_limit_absorbs_only_while_the_readings_catch_up() {
    let mut gate = LimitAnnouncements::default();
    let n = Channel::Notification;
    assert!(claim(&mut gate, n, "work", UNKNOWN_WINDOW, None, now()));
    let later = now() + UNKNOWN_ABSORBS_FOR + secs(60);
    let reset = resets() + secs(3600);
    assert!(claim(&mut gate, n, "work", "session", Some(reset), later));
    assert!(!claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(reset),
        later + secs(60)
    ));
}

/// The readings name the window of a limit announced before they could: it
/// lasts until that window resets, not an hour.
#[test]
fn the_readings_teach_an_unknown_limit_its_window() {
    let mut gate = LimitAnnouncements::default();
    let (n, r) = (Channel::Notification, Channel::Reaction);
    assert!(claim(&mut gate, n, "work", UNKNOWN_WINDOW, None, now()));
    assert!(claim(&mut gate, r, "work", UNKNOWN_WINDOW, None, now()));
    gate.learn("work", "session", Some(resets()), now() + secs(300));
    // Past the unknown lifetime, before the reset: still the same limit.
    let retry = now() + UNKNOWN_LIFETIME + secs(20 * 60);
    assert!(!claim(
        &mut gate,
        n,
        "work",
        "session",
        Some(resets()),
        retry
    ));
    assert!(!claim(&mut gate, r, "work", UNKNOWN_WINDOW, None, retry));
    // Too late to learn: a reading long after the failure names a limit of
    // its own.
    let mut other = LimitAnnouncements::default();
    assert!(claim(&mut other, n, "work", UNKNOWN_WINDOW, None, now()));
    other.learn(
        "work",
        "session",
        Some(resets()),
        now() + UNKNOWN_ABSORBS_FOR + secs(60),
    );
    let windows: Vec<&str> = other.incidents["work"]
        .iter()
        .map(|i| i.window.as_str())
        .collect();
    assert_eq!(windows, [UNKNOWN_WINDOW]);
}

/// An early reset (a reset credit, `/limit-reset`) keeps the reset time: using
/// the window up again is announced again.
#[test]
fn an_early_reset_makes_the_next_hit_news() {
    let mut gate = LimitAnnouncements::default();
    let n = Channel::Notification;
    let friday = now() + secs(4 * 86_400);
    assert!(claim(&mut gate, n, "work", "weekly", Some(friday), now()));
    assert!(!claim(&mut gate, n, "work", "weekly", Some(friday), now()));
    gate.end("work", "weekly");
    assert!(claim(
        &mut gate,
        n,
        "work",
        "weekly",
        Some(friday),
        now() + secs(86_400)
    ));
}

#[test]
fn a_banner_without_a_sound_leaves_the_chime() {
    let mut gate = LimitAnnouncements::default();
    assert!(gate.claim(
        Channel::Notification,
        "work",
        "session",
        Some(resets()),
        now(),
        &[]
    ));
    assert!(claim(
        &mut gate,
        Channel::Reaction,
        "work",
        "session",
        Some(resets()),
        now()
    ));
}

#[test]
fn expired_incidents_are_pruned() {
    let mut gate = LimitAnnouncements::default();
    claim(
        &mut gate,
        Channel::Notification,
        "work",
        "session",
        Some(resets()),
        now(),
    );
    gate.prune(resets());
    assert!(gate.incidents.is_empty());
}

#[test]
fn only_so_many_incidents_are_kept_per_ring() {
    let mut gate = LimitAnnouncements::default();
    for n in 0..12 {
        claim(
            &mut gate,
            Channel::Notification,
            "work",
            &format!("scoped:model-{n}"),
            Some(resets()),
            now(),
        );
    }
    let list = &gate.incidents["work"];
    assert_eq!(list.len(), 8);
    assert_eq!(list[0].window, "scoped:model-4");
}

#[test]
fn windows_have_keys() {
    assert_eq!(window_key(Some(&LimitWindowKind::Session)), "session");
    assert_eq!(window_key(Some(&LimitWindowKind::Weekly)), "weekly");
    assert_eq!(
        window_key(Some(&LimitWindowKind::Scoped("Opus".into()))),
        "scoped:Opus"
    );
    assert_eq!(window_key(None), UNKNOWN_WINDOW);
}

// ---- LimitAnnouncementStoreTests ----

#[test]
fn a_relaunch_does_not_announce_the_same_limit_again() {
    let reset = now() + secs(3600);
    let mut first = LimitAnnouncementStore::load(None, true, now());
    assert!(first.claim(
        Channel::Notification,
        "work",
        Some(&LimitWindowKind::Session),
        Some(reset),
        now(),
        &[]
    ));
    let bytes = first.take_bytes().expect("a claim is written");
    // Nothing changed since: nothing to write.
    assert_eq!(first.take_bytes(), None);
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("\"channels\":[\"notification\"]"), "{text}");
    assert!(
        text.contains("\"resetsAt\":\"2027-01-15T09:00:00Z\""),
        "{text}"
    );

    let mut second = LimitAnnouncementStore::load(Some(&bytes), true, now() + secs(60));
    assert!(!second.claim(
        Channel::Notification,
        "work",
        Some(&LimitWindowKind::Session),
        Some(reset),
        now() + secs(60),
        &[]
    ));
    // Once the window has reset, the next launch has nothing left.
    let later = LimitAnnouncementStore::load(Some(&bytes), true, reset + secs(1));
    assert!(later.state().is_empty());
}

#[test]
fn an_unreadable_file_starts_afresh() {
    let mut store = LimitAnnouncementStore::load(Some(b"not json"), true, now());
    assert!(store.was_unreadable());
    assert_eq!(store.state(), &LimitAnnouncements::default());
    assert!(store.claim(Channel::Notification, "work", None, None, now(), &[]));
    // A channel this knows nothing of is as unreadable as any other damage.
    let odd = br#"{"version":1,"incidents":{"work":[{"window":"session","startedAt":"2027-01-15T08:00:00Z","channels":["carrier-pigeon"]}]}}"#;
    assert!(LimitAnnouncementStore::load(Some(odd), true, now()).was_unreadable());
}

/// A store that doesn't persist (a sealed run) keeps its announcements in
/// memory only.
#[test]
fn a_store_that_does_not_persist_writes_nothing() {
    let mut store = LimitAnnouncementStore::load(None, false, now());
    assert!(store.claim(
        Channel::Notification,
        "work",
        Some(&LimitWindowKind::Weekly),
        None,
        now(),
        &[]
    ));
    assert_eq!(store.take_bytes(), None);
    assert_eq!(store.bytes_now(), None);
    // And it reads nothing either.
    let file = LimitAnnouncementStore::load(None, true, now());
    let mut writer = file.clone();
    writer.claim(Channel::Notification, "work", None, None, now(), &[]);
    let bytes = writer.take_bytes().unwrap();
    assert!(LimitAnnouncementStore::load(Some(&bytes), false, now())
        .state()
        .is_empty());
}

#[test]
fn a_run_that_changed_nothing_leaves_the_file_alone() {
    let store = LimitAnnouncementStore::load(None, true, now());
    assert_eq!(store.bytes_now(), None);
}

// ---- RestoredFailureTests ----

fn limited() -> SessionState {
    SessionState::Failed(NeedsInputReason::Error {
        text: "Rate limited".into(),
        code: Some("rate_limit".into()),
    })
}

fn kinds(from: Option<&SessionState>, restored: bool) -> Vec<NewsKind> {
    kinds_between_restoring(from, Some(&limited()), false, None, Some(now()), restored)
}

#[test]
fn a_restored_failure_is_not_news() {
    assert_eq!(kinds(None, true), []);
    assert_eq!(kinds(Some(&SessionState::Working), true), []);
    let question = SessionState::NeedsYou(NeedsInputReason::Question);
    assert_eq!(kinds(Some(&question), true), []);
    assert_eq!(
        kinds(Some(&SessionState::ReadyForReview), true),
        [NewsKind::Resolved]
    );
    // Seen happen: news as before.
    assert_eq!(kinds(None, false), [NewsKind::NeedsInput]);
    assert_eq!(
        kinds(Some(&SessionState::Working), false),
        [NewsKind::NeedsInput]
    );
    // Only failures: anything else restored is still news.
    assert_eq!(
        kinds_between_restoring(
            None,
            Some(&SessionState::NeedsYou(NeedsInputReason::Question)),
            false,
            None,
            Some(now()),
            true
        ),
        [NewsKind::NeedsInput]
    );
}

fn launched() -> Harness {
    let mut h = Harness::new();
    h.store.load_review(None, h.now);
    h
}

fn failed_live(h: &mut Harness) -> agentnotch_engine::runtime_types::SessionEffects {
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    })
}

fn review_file(h: &mut Harness) -> Vec<u8> {
    use agentnotch_engine::runtime_types::{Job, PersistFile};
    h.tick()
        .jobs
        .iter()
        .find_map(|job| match job {
            Job::Persist {
                file: PersistFile::Review,
                bytes,
            } => Some(bytes.clone()),
            _ => None,
        })
        .expect("the tick asks for the review file")
}

/// A failed turn read back from `review-state.json` (the session reopened,
/// resumed, or seen again after a relaunch) shows, but isn't news.
#[test]
fn a_session_seen_again_keeps_its_failure_but_not_as_news() {
    let mut first = launched();
    let live = failed_live(&mut first);
    assert!(live.transitions.is_empty(), "inside the launch baseline");
    let seen = first.session().unwrap();
    assert!(seen.has_failed_turn() && !seen.stop_error_is_restored);
    let bytes = review_file(&mut first);

    // Seen again (the next launch, or a resumed session): restored.
    let mut second = Harness::new();
    second.now = first.now + secs(60);
    second.store.load_review(Some(&bytes), second.now);
    second.store.initial_scan_completed(second.now);
    second.now += secs(3);
    let found = second.registry("idle", second.now);
    let restored = second.session().unwrap();
    assert!(restored.has_failed_turn() && restored.stop_error_is_restored);
    assert!(
        found.transitions.iter().all(|t| !t.became_needs_you()),
        "{:?}",
        found.transitions
    );
    assert!(second.view().unwrap().stop_error_is_restored);

    // The same session tried again and failed again, live: news, and no
    // longer restored.
    second.now += secs(1);
    let again = failed_live(&mut second);
    let session = second.session().unwrap();
    assert!(session.has_failed_turn() && !session.stop_error_is_restored);
    let news: Vec<_> = again
        .transitions
        .iter()
        .filter(|t| t.became_needs_you())
        .collect();
    assert_eq!(news.len(), 1, "{:?}", again.transitions);
    assert!(!news[0].session.stop_error_is_restored);
}

/// A restored failure that ends (the user moved on) still withdraws what it
/// had posted: a resolution is always passed on.
#[test]
fn a_restored_failure_that_is_resolved_still_passes() {
    let mut first = launched();
    failed_live(&mut first);
    let bytes = review_file(&mut first);
    let mut second = Harness::new();
    second.now = first.now + secs(60);
    second.store.load_review(Some(&bytes), second.now);
    second.store.initial_scan_completed(second.now);
    second.now += secs(3);
    second.registry("idle", second.now);
    assert!(second.session().unwrap().stop_error_is_restored);
    let moved_on = second.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    assert!(!second.session().unwrap().stop_error_is_restored);
    assert!(!moved_on.transitions.is_empty());
}

// ---- the toast rules ----

fn view(id: &str, state: SessionState, restored: bool) -> SessionView {
    let mut view = SessionView::new(id, format!("/tmp/{id}"), now());
    view.ring = Some(RingId::from("claude-acct-1a2b3c4d5e6f"));
    view.state = state;
    view.stop_error_is_restored = restored;
    view
}

fn transition(session: SessionView, from: Option<SessionState>) -> AttentionTransition {
    AttentionTransition {
        to: session.state.clone(),
        session,
        from,
    }
}

fn toast_ctx() -> ToastContext {
    ToastContext {
        now: now(),
        notify_needs_input: true,
        notify_ready_for_review: true,
        permission: NotifyPermission::Allowed,
        suppressed: false,
        looking_at: None,
        account_label: None,
        multi_account: false,
        title: "Refactor the parser".into(),
        project: "app".into(),
    }
}

fn overloaded() -> SessionState {
    SessionState::Failed(NeedsInputReason::Error {
        text: "Overloaded".into(),
        code: Some("overloaded".into()),
    })
}

/// A failure read back from disk was announced when it happened: no banner,
/// the failed kind included.
#[test]
fn a_restored_failure_posts_no_banner() {
    let live = transition(view("s", overloaded(), false), None);
    assert!(toast_for(&live, &toast_ctx()).is_some());
    let restored = transition(view("s", overloaded(), true), None);
    assert!(toast_for(&restored, &toast_ctx()).is_none());
    // Only failures: a restored question still posts.
    let question = SessionState::NeedsYou(NeedsInputReason::Question);
    assert!(toast_for(&transition(view("s", question, true), None), &toast_ctx()).is_some());
}

fn limit_ctx() -> LimitContext {
    LimitContext {
        notify_needs_input: true,
        permission: NotifyPermission::Allowed,
        suppressed: false,
        account_label: None,
        limit_reset: None,
    }
}

fn session(id: &str, restored: bool) -> LimitedSession {
    LimitedSession {
        id: SessionId::from(id),
        title: format!("Session {id}"),
        restored,
    }
}

/// The limit banner is due for a session newly stopped by the limit, not for
/// one that comes back with its old failure.
#[test]
fn a_restored_session_does_not_make_the_limit_news() {
    let ring = RingId::from("claude-acct-1a2b3c4d5e6f");
    let mut banners = LimitBanners::new();
    assert_eq!(
        banners.update(
            &ring,
            &[session("a", true), session("b", true)],
            &limit_ctx()
        ),
        LimitChange::Nothing
    );
    // A live one joins the restored: due.
    assert!(matches!(
        banners.update(
            &ring,
            &[session("a", true), session("b", true), session("c", false)],
            &limit_ctx()
        ),
        LimitChange::Post(_)
    ));
    // The transition of a restored failure doesn't ask for the banner.
    let restored = transition(view("s", limited(), true), Some(SessionState::Working));
    assert!(!LimitBanners::concerns(&restored));
    let live = transition(view("s", limited(), false), Some(SessionState::Working));
    assert!(LimitBanners::concerns(&live));
}

// ---- LimitWindowTests ----

fn window(utilization: f64, resets_in: Duration, duration: u64) -> UsageWindow {
    UsageWindow::new(utilization, Some(now() + resets_in), duration)
}

/// A model's week used up days ago doesn't stand in for today's 5-hour limit:
/// a failed turn is announced under the window that stops every model.
#[test]
fn the_five_hour_or_weekly_window_comes_before_a_models_own() {
    let week = UsageWindow::WEEKLY_DURATION_S;
    let session_s = UsageWindow::SESSION_DURATION_S;
    let mut usage = AccountUsage::new(IdentityId::from("a"), UsageSource::Probe, now());
    usage.five_hour = Some(window(100.0, secs(7200), session_s));
    usage.seven_day = Some(window(60.0, secs(3 * 86_400), week));
    usage.scoped = vec![("Opus".to_owned(), window(100.0, secs(4 * 86_400), week))];
    assert_eq!(
        usage.limit_hit(now()).map(|hit| hit.window),
        Some(LimitWindowKind::Scoped("Opus".into()))
    );
    assert_eq!(
        usage.announced_limit_hit(now()),
        Some(LimitHit {
            window: LimitWindowKind::Session,
            resets_at: Some(now() + secs(7200)),
        })
    );
    // Only a model's own window used up: that one.
    usage.five_hour.as_mut().unwrap().utilization = 40.0;
    assert_eq!(
        usage.announced_limit_hit(now()).map(|hit| hit.window),
        Some(LimitWindowKind::Scoped("Opus".into()))
    );
    // Both general windows: the one that lifts last.
    usage.five_hour.as_mut().unwrap().utilization = 100.0;
    usage.seven_day.as_mut().unwrap().utilization = 100.0;
    assert_eq!(
        usage.announced_limit_hit(now()).map(|hit| hit.window),
        Some(LimitWindowKind::Weekly)
    );
    // Nothing used up: nothing.
    usage.five_hour.as_mut().unwrap().utilization = 10.0;
    usage.seven_day.as_mut().unwrap().utilization = 10.0;
    usage.scoped.clear();
    assert_eq!(usage.announced_limit_hit(now()), None);
}

fn reading(session_used: f64, weekly_used: f64, status: RingStatus) -> RingReading {
    let mut usage = AccountUsage::new(IdentityId::from("a"), UsageSource::Probe, now());
    usage.five_hour = Some(window(
        session_used * 100.0,
        secs(3600),
        UsageWindow::SESSION_DURATION_S,
    ));
    usage.seven_day = Some(window(
        weekly_used * 100.0,
        secs(86_400),
        UsageWindow::WEEKLY_DURATION_S,
    ));
    usage.scoped = vec![(
        "Opus".to_owned(),
        window(10.0, secs(86_400), UsageWindow::WEEKLY_DURATION_S),
    )];
    RingReading::Reading {
        usage,
        status,
        stale_after: now() + secs(900),
    }
}

#[test]
fn a_fresh_reading_under_the_limit_lifts_its_window() {
    use LimitWindowKind::{Session, Weekly};
    let lifted = |reading: &RingReading| lifted_limit_windows(reading, now());
    assert_eq!(lifted(&reading(0.2, 1.0, RingStatus::Ok)), [Session]);
    assert_eq!(lifted(&reading(0.96, 0.5, RingStatus::Ok)), [Weekly]);
    assert_eq!(
        lifted(&reading(0.2, 0.2, RingStatus::Ok)),
        [Session, Weekly]
    );
    // A stale reading, or none yet, proves nothing.
    assert!(lifted(&reading(0.2, 0.2, RingStatus::Stale)).is_empty());
    assert!(lifted(&RingReading::Waiting).is_empty());
    assert!(lifted(&RingReading::SignInNeeded).is_empty());
}

// ---- LimitReactionTests ----

fn placed(id: &str, ring: &str, state: SessionState) -> AttentionTransition {
    let mut session = SessionView::new(id, format!("/tmp/{id}"), now());
    session.ring = Some(RingId::from(ring));
    session.state = state.clone();
    AttentionTransition {
        session,
        from: Some(SessionState::Working),
        to: state,
    }
}

/// The chime and peek for a turn stopped by the limit: once per account and
/// limit.
#[test]
fn only_the_first_failure_of_a_limit_chimes() {
    let hit = LimitHit {
        window: LimitWindowKind::Session,
        resets_at: Some(now() + secs(3600)),
    };
    let transitions = vec![
        (placed("a", "work", limited()), Some(hit.clone())),
        (placed("b", "work", limited()), Some(hit.clone())),
        (placed("c", "personal", limited()), None),
        (
            placed(
                "d",
                "work",
                SessionState::NeedsYou(NeedsInputReason::Question),
            ),
            None,
        ),
    ];
    let mut gate = LimitAnnouncements::default();
    let mut filtered = |transitions: &[(AttentionTransition, Option<LimitHit>)]| -> Vec<String> {
        transitions
            .iter()
            .filter(|(tr, hit)| {
                !repeats_limit_reaction(tr, hit.as_ref(), |ring, hit| {
                    gate.claim(
                        Channel::Reaction,
                        ring.as_str(),
                        &window_key(hit.map(|hit| &hit.window)),
                        hit.and_then(|hit| hit.resets_at),
                        now(),
                        &[],
                    )
                })
            })
            .map(|(tr, _)| tr.session.id.to_string())
            .collect()
    };
    // One per account; a question is never held back.
    assert_eq!(filtered(&transitions), ["a", "c", "d"]);
    // The retry that fails again: nothing, the question still comes through.
    assert_eq!(filtered(&transitions), ["d"]);
    // Resolutions always pass.
    let resolved = AttentionTransition {
        from: Some(limited()),
        to: SessionState::Working,
        ..placed("a", "work", SessionState::Working)
    };
    assert!(!repeats_limit_reaction(&resolved, None, |_, _| false));
}

// ---- RingResetTimeTests ----

fn base() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_254_200)
}

fn ring_window(id: &str, resets_at: Option<SystemTime>, duration_s: Option<u64>) -> RingWindow {
    RingWindow {
        id: id.to_owned(),
        label: None,
        used_fraction: 1.0,
        resets_at,
        duration_s,
        money: None,
    }
}

fn resets_of(windows: &[RingWindow]) -> Vec<Option<SystemTime>> {
    windows.iter().map(|w| w.resets_at).collect()
}

#[test]
fn rounding_between_sources_keeps_the_first_reset_time() {
    let previous = [
        ring_window("session", Some(base()), Some(18_000)),
        ring_window("weekly_all", Some(base() + secs(86_400)), Some(604_800)),
    ];
    // The usage endpoint's fractional seconds after the status line's whole
    // ones, and back.
    let later = vec![
        ring_window(
            "session",
            Some(base() + Duration::from_millis(257)),
            Some(18_000),
        ),
        ring_window(
            "weekly_all",
            Some(base() + secs(86_399) + Duration::from_millis(600)),
            Some(604_800),
        ),
    ];
    let kept = keeping_reset_times(later, &previous);
    assert_eq!(
        resets_of(&kept),
        [Some(base()), Some(base() + secs(86_400))]
    );
    let earlier = vec![ring_window(
        "session",
        Some(base() - Duration::from_millis(400)),
        Some(18_000),
    )];
    assert_eq!(
        keeping_reset_times(earlier, &previous)[0].resets_at,
        Some(base())
    );
}

#[test]
fn a_new_window_passes_through() {
    let previous = [ring_window("session", Some(base()), Some(18_000))];
    let next = vec![ring_window(
        "session",
        Some(base() + secs(18_000)),
        Some(18_000),
    )];
    assert_eq!(keeping_reset_times(next.clone(), &previous), next);
    // Nothing to keep: no previous reading, a window it didn't have, no reset
    // time.
    assert_eq!(keeping_reset_times(next.clone(), &[]), next);
    let other = vec![ring_window(
        "weekly_opus",
        Some(base() + Duration::from_millis(300)),
        Some(604_800),
    )];
    assert_eq!(keeping_reset_times(other.clone(), &previous), other);
    let unknown = vec![ring_window("session", None, Some(18_000))];
    assert_eq!(keeping_reset_times(unknown.clone(), &previous), unknown);
}

#[test]
fn a_window_of_unknown_length_uses_a_minute() {
    let previous = [ring_window("session", Some(base()), None)];
    let jitter = vec![ring_window(
        "session",
        Some(base() + Duration::from_millis(500)),
        None,
    )];
    assert_eq!(
        keeping_reset_times(jitter, &previous)[0].resets_at,
        Some(base())
    );
    let moved = vec![ring_window("session", Some(base() + secs(120)), None)];
    assert_eq!(keeping_reset_times(moved.clone(), &previous), moved);
}

/// The kept times go back into the usage the ring is drawn from, window by
/// window, the ids `windows` makes.
#[test]
fn kept_times_go_back_into_the_usage() {
    let mut usage = AccountUsage::new(IdentityId::from("a"), UsageSource::Probe, now());
    usage.five_hour = Some(window(100.0, secs(3600), 18_000));
    usage.seven_day = Some(window(50.0, secs(86_400), 604_800));
    usage.scoped = vec![("Opus".to_owned(), window(10.0, secs(86_400), 604_800))];
    let previous = windows(&usage, now());
    let mut jittered = usage.clone();
    let wobble = Duration::from_millis(300);
    jittered.five_hour.as_mut().unwrap().resets_at = Some(now() + secs(3600) + wobble);
    jittered.seven_day.as_mut().unwrap().resets_at = Some(now() + secs(86_400) - wobble);
    jittered.scoped[0].1.resets_at = Some(now() + secs(86_400) + wobble);
    let kept = keeping_reset_times(windows(&jittered, now()), &previous);
    assert_eq!(with_reset_times(&jittered, &kept), usage);
    // A real new window keeps its own time.
    let mut fresh = usage.clone();
    fresh.five_hour.as_mut().unwrap().resets_at = Some(now() + secs(3600 + 18_000));
    let kept = keeping_reset_times(windows(&fresh, now()), &previous);
    assert_eq!(with_reset_times(&fresh, &kept), fresh);
}

// Keeps `BTreeSet` imported for the sealed-store test above if it grows.
#[test]
fn the_incidents_of_a_ring_have_their_channels_as_a_set() {
    let mut gate = LimitAnnouncements::default();
    gate.claim(
        Channel::Reaction,
        "work",
        "session",
        Some(resets()),
        now(),
        &[Channel::Notification],
    );
    assert_eq!(
        gate.incidents["work"][0].channels,
        BTreeSet::from([Channel::Notification, Channel::Reaction])
    );
}
