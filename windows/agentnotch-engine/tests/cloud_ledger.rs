//! The session ledger, the backfill roots, folder logins and the hub's feed
//! (CL§6, §7.2, §7.4; the Mac's `SessionLedgerTests`, all 19, plus the
//! Windows path, junction, case and placement vectors).

mod cloud_support;

use agentnotch_engine::cloud::backfill::{self, Folder};
use agentnotch_engine::cloud::contract::SessionSource;
use agentnotch_engine::cloud::feed::{self, Placement};
use agentnotch_engine::cloud::files::StateFile;
use agentnotch_engine::cloud::folder_logins::CloudFolderLogins;
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::ledger::{
    path_resolver, CloudLedgerEntry, Origin, SessionLedger, SessionOwner, SessionOwners,
    MAX_OWNERS, UNCOUNTED_AFTER,
};
use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{Attribution, BackfillFolder, IdentityId, LiveSessionObservation};
use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use cloud_support::{ids, ledger_account, live, live_for, session_view, CloudFixture as F};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const HOME: &str = "/Users/me";

fn memory_ledger() -> SessionLedger {
    SessionLedger::new(
        StateFile::memory(),
        path_resolver(
            PathStyle::native(),
            Path::new(HOME),
            Arc::new(StdSecureFiles),
        ),
    )
}

fn file_ledger(support: &Path) -> SessionLedger {
    SessionLedger::in_support(support, Arc::new(StdSecureFiles), true, Path::new(HOME))
}

/// `observe` with the lists as slices and the time in seconds after the
/// fixture's base.
fn observe(
    ledger: &SessionLedger,
    live: Vec<LiveSessionObservation>,
    live_ids: &[&str],
    unsure: &[&str],
    waiting: &[&str],
    now: f64,
) -> Vec<String> {
    ledger.observe(
        &live,
        &ids(live_ids),
        &ids(unsure),
        &ids(waiting),
        &BTreeMap::new(),
        F::at(now),
    )
}

fn settle(ledger: &SessionLedger, live_ids: &[&str], now: f64) -> Vec<String> {
    ledger.settle(&ids(live_ids), F::at(now))
}

fn key(session: &str, account_key: &str) -> String {
    CloudLedgerEntry::key_of(session, account_key)
}

fn backfilled(id: &str) -> CloudLedgerEntry {
    CloudLedgerEntry {
        session_id: id.to_owned(),
        identity_id: IdentityId::from(F::IDENTITY_ID),
        account_key: F::ACCOUNT_KEY.to_owned(),
        project_name: "app".to_owned(),
        project_path: "/Users/me/code/app".to_owned(),
        transcript_path: None,
        config_dir: None,
        source: SessionSource::Cli,
        started_at: F::base(),
        last_activity_at: F::base(),
        ended_at: None,
        model: None,
        cost_usd: None,
        title: None,
        origin: Origin::Backfill,
        shared_history: None,
    }
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ---- The ledger ----

#[test]
fn captures_the_account_and_project_and_keeps_them() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("cloud-ledger.json");
    let ledger = file_ledger(root.path());
    let personal = F::account();
    let accounts = BTreeMap::from([(F::ACCOUNT_KEY.to_owned(), ledger_account(&personal))]);
    ledger.observe(
        &[live(F::SESSION_A).cost(1.5).title("Fix the notch").build()],
        &ids(&[F::SESSION_A]),
        &ids(&[]),
        &ids(&[]),
        &accounts,
        F::base(),
    );
    let entry = ledger.entry(F::SESSION_A).expect("captured");
    assert_eq!(entry.account_key, F::ACCOUNT_KEY);
    assert_eq!(entry.identity_id.as_str(), F::IDENTITY_ID);
    assert_eq!(entry.project_name, "app");
    assert_eq!(
        entry.project_path,
        keys::project_path("/Users/me/code/app", Path::new(HOME), &StdSecureFiles)
    );
    assert_eq!(entry.key(), format!("{}|{}", F::SESSION_A, F::ACCOUNT_KEY));
    assert_eq!(entry.source, SessionSource::Vscode);
    assert_eq!(entry.origin, Origin::Live);
    assert_eq!(entry.ended_at, None);
    assert_eq!(entry.cost_usd, Some(1.5));
    assert_eq!(entry.title.as_deref(), Some("Fix the notch"));
    assert_eq!(entry.model.as_deref(), Some("claude-opus-4-5"));

    ledger.save_now();
    #[cfg(unix)]
    assert_eq!(mode_of(&file), 0o600);
    let reloaded = file_ledger(root.path());
    assert_eq!(reloaded.entry(F::SESSION_A), Some(entry));
    assert_eq!(
        reloaded.account(F::ACCOUNT_KEY).unwrap().email.as_deref(),
        Some("me@example.com")
    );
    assert!(file.exists());
}

#[test]
fn a_session_ends_a_minute_after_it_goes_and_comes_back() {
    let ledger = memory_ledger();
    let a = F::SESSION_A;
    observe(
        &ledger,
        vec![live(a).active(100.0).build()],
        &[a],
        &[],
        &[],
        100.0,
    );
    // Gone: not ended at once...
    assert!(settle(&ledger, &[], 120.0).is_empty());
    assert_eq!(ledger.entry(a).unwrap().ended_at, None);
    // ...but a minute later, dated when it went.
    assert_eq!(settle(&ledger, &[], 190.0), vec![key(a, F::ACCOUNT_KEY)]);
    assert_eq!(ledger.entry(a).unwrap().ended_at, Some(F::at(120.0)));
    observe(
        &ledger,
        vec![live(a).active(300.0).build()],
        &[a],
        &[],
        &[],
        300.0,
    );
    assert_eq!(ledger.entry(a).unwrap().ended_at, None);
    // Still running but not attributable this time: its account's part stops
    // at its last activity (M2), and goes on once it is again.
    let k = key(a, F::ACCOUNT_KEY);
    assert_eq!(
        observe(&ledger, vec![], &[a], &[], &[], 320.0),
        vec![k.clone()]
    );
    assert_eq!(
        ledger.entry_by_key(&k).unwrap().ended_at,
        Some(F::at(300.0))
    );
    assert!(ledger.is_uncounted(a));
    assert!(settle(&ledger, &[a], 900.0).is_empty());
    observe(
        &ledger,
        vec![live(a).active(950.0).build()],
        &[a],
        &[],
        &[],
        950.0,
    );
    assert_eq!(ledger.entry(a).unwrap().ended_at, None);
    assert!(!ledger.is_uncounted(a));
}

/// Regression (M2): a session the hub can't attribute for certain any more (a
/// mirrored `~\.claude` switching accounts) stops counting for its account
/// from its last activity seen while it was certain; its responses from then
/// on are no one's until it is certain again, and then its new process's
/// account takes over from that process's start.
#[test]
fn a_session_the_hub_cant_attribute_counts_for_no_one() {
    let ledger = memory_ledger();
    let (personal, work) = (F::account(), F::work_account());
    let id = F::SESSION_A;
    observe(
        &ledger,
        vec![live_for(id, &personal).active(600.0).build()],
        &[id],
        &[],
        &[],
        600.0,
    );
    // Unsure: its new responses are nobody's; its part ends where it was last
    // certain.
    let personal_key = key(id, F::ACCOUNT_KEY);
    assert_eq!(
        observe(&ledger, vec![], &[id], &[id], &[], 700.0),
        vec![personal_key.clone()]
    );
    let nobody_from = F::at(600.0) + UNCOUNTED_AFTER;
    assert_eq!(
        ledger.owners(id),
        vec![
            SessionOwner::new(None, F::ACCOUNT_KEY),
            SessionOwner::new(Some(nobody_from), "")
        ]
    );
    assert_eq!(
        ledger.entry_by_key(&personal_key).unwrap().ended_at,
        Some(F::at(600.0))
    );
    assert!(ledger.entry(id).is_none() && ledger.is_uncounted(id));
    // Still unsure: nothing more changes.
    assert!(observe(&ledger, vec![], &[id], &[id], &[], 800.0).is_empty());
    assert_eq!(ledger.owners(id).len(), 2);
    // Certain again, in a new process as the other account: from its start.
    let resumed_at = F::at(1000.0);
    observe(
        &ledger,
        vec![live_for(id, &work)
            .active(1100.0)
            .started(1000.0)
            .process(1000.0)
            .build()],
        &[id],
        &[],
        &[],
        1100.0,
    );
    assert_eq!(
        ledger.owners(id),
        vec![
            SessionOwner::new(None, F::ACCOUNT_KEY),
            SessionOwner::new(Some(nobody_from), ""),
            SessionOwner::new(Some(resumed_at), F::WORK_ACCOUNT_KEY),
        ]
    );
    let entry = ledger.entry(id).unwrap();
    assert_eq!(entry.account_key, F::WORK_ACCOUNT_KEY);
    assert_eq!(entry.started_at, resumed_at);
    assert!(!ledger.is_uncounted(id));

    // An unsure session is never counted for any account, even when the hub
    // reports it among the live ones by mistake.
    let both = memory_ledger();
    observe(
        &both,
        vec![live_for(id, &personal).build()],
        &[id],
        &[id],
        &[],
        0.0,
    );
    assert_eq!(both.count(), 0);
    assert!(both.is_uncounted(id));
}

/// Regression (M2): the same process losing and regaining certainty (a moment
/// when the registry couldn't say) loses nothing: its responses in between
/// were its own all along.
#[test]
fn the_same_process_certain_again_keeps_its_responses() {
    let ledger = memory_ledger();
    let id = F::SESSION_A;
    observe(
        &ledger,
        vec![live(id).active(100.0).process(0.0).build()],
        &[id],
        &[],
        &[],
        100.0,
    );
    observe(&ledger, vec![], &[id], &[id], &[], 200.0);
    assert_eq!(ledger.owners(id).len(), 2);
    observe(
        &ledger,
        vec![live(id).active(300.0).process(0.0).build()],
        &[id],
        &[],
        &[],
        300.0,
    );
    assert_eq!(
        ledger.owners(id),
        vec![SessionOwner::new(None, F::ACCOUNT_KEY)]
    );
    let entry = ledger.entry(id).unwrap();
    assert_eq!(entry.ended_at, None);
    assert_eq!(entry.last_activity_at, F::at(300.0));
    assert_eq!(ledger.count(), 1);
}

/// Regression (M2): a session first seen unsure is no one's from its start:
/// when a certain process of it turns up, only that process's responses are
/// its account's; the backfill never adds it either.
#[test]
fn a_session_first_seen_unsure_counts_only_from_its_certain_process() {
    let root = tempfile::tempdir().unwrap();
    let id = F::SESSION_A;
    let ledger = file_ledger(root.path());
    observe(&ledger, vec![], &[id], &[id], &[], 100.0);
    assert!(ledger.count() == 0 && ledger.knows(id) && ledger.is_uncounted(id));
    assert_eq!(ledger.owners(id), vec![SessionOwner::new(None, "")]);
    // Remembered across a relaunch.
    ledger.save_now();
    let reloaded = file_ledger(root.path());
    assert!(reloaded.is_uncounted(id));
    // The backfill leaves it alone.
    assert_eq!(
        reloaded.record_backfill(&[backfilled(id)], &BTreeMap::new()),
        0
    );
    // A certain process of it: counted from that process's start.
    let resumed_at = F::at(500.0);
    observe(
        &reloaded,
        vec![live(id).active(600.0).started(500.0).process(500.0).build()],
        &[id],
        &[],
        &[],
        600.0,
    );
    assert_eq!(
        reloaded.owners(id),
        vec![
            SessionOwner::new(None, ""),
            SessionOwner::new(Some(resumed_at), F::ACCOUNT_KEY)
        ]
    );
    assert_eq!(reloaded.entry(id).unwrap().started_at, resumed_at);
    assert!(!reloaded.is_uncounted(id));
    // A session nobody reported unsure, merely not attributed, isn't
    // remembered.
    observe(&reloaded, vec![], &[F::SESSION_B], &[], &[], 700.0);
    assert!(!reloaded.knows(F::SESSION_B));
}

/// Regression (review): a session the hub hasn't placed yet (its state just
/// made again by a hook, a folder not grouped yet) waits: a known one isn't
/// paused (no stretch of nobody, its part not ended), a new one isn't
/// remembered as no one's, and once placed nothing was split.
#[test]
fn a_session_not_placed_yet_waits() {
    let ledger = memory_ledger();
    let id = F::SESSION_A;
    observe(
        &ledger,
        vec![live(id).active(100.0).process(0.0).build()],
        &[id],
        &[],
        &[],
        100.0,
    );
    assert!(observe(&ledger, vec![], &[id], &[], &[id], 110.0).is_empty());
    assert_eq!(
        ledger.owners(id),
        vec![SessionOwner::new(None, F::ACCOUNT_KEY)]
    );
    assert!(!ledger.is_uncounted(id) && ledger.entry(id).unwrap().ended_at.is_none());
    // Waiting longer than a missing session's grace: still running.
    assert!(observe(&ledger, vec![], &[], &[], &[id], 300.0).is_empty());
    assert!(settle(&ledger, &[id], 400.0).is_empty());
    assert_eq!(ledger.entry(id).unwrap().ended_at, None);
    // Placed: a new process of the same account. One owner, one part.
    observe(
        &ledger,
        vec![live(id).active(500.0).process(450.0).build()],
        &[id],
        &[],
        &[],
        500.0,
    );
    assert_eq!(
        ledger.owners(id),
        vec![SessionOwner::new(None, F::ACCOUNT_KEY)]
    );
    assert_eq!(ledger.count(), 1);
    assert_eq!(ledger.entry(id).unwrap().last_activity_at, F::at(500.0));
    // A new session not placed yet isn't remembered at all.
    observe(
        &ledger,
        vec![],
        &[F::SESSION_B],
        &[],
        &[F::SESSION_B],
        600.0,
    );
    assert!(!ledger.knows(F::SESSION_B));
    // Unsure wins over waiting.
    observe(&ledger, vec![], &[id], &[id], &[id], 700.0);
    assert!(ledger.is_uncounted(id));
}

/// Regression (review): a session that keeps losing its account (a new
/// process each time) is counted again whenever the hub is certain: the cap
/// on hand-overs never blocks one away from nobody. Stretches of nobody are
/// bounded on their own; past that, the session stays with the account it ran
/// as, as a capped session always did.
#[test]
fn a_session_is_counted_again_however_often_it_was_unsure() {
    let ledger = memory_ledger();
    let id = F::SESSION_A;
    let mut now = 0.0;
    let certain = |now: &mut f64| {
        *now += 100.0;
        observe(
            &ledger,
            vec![live(id).active(*now).started(*now).process(*now).build()],
            &[id],
            &[],
            &[],
            *now,
        );
    };
    let unsure = |now: &mut f64| {
        *now += 100.0;
        observe(&ledger, vec![], &[id], &[id], &[], *now);
    };
    let nobody_stretches = || {
        ledger
            .owners(id)
            .iter()
            .filter(|o| o.account_key.is_empty())
            .count()
    };
    certain(&mut now);
    for cycle in 1..=(MAX_OWNERS / 2) {
        unsure(&mut now);
        assert!(ledger.is_uncounted(id), "cycle {cycle}");
        certain(&mut now);
        assert!(!ledger.is_uncounted(id), "cycle {cycle}");
        let entry = ledger.entry(id).unwrap();
        assert!(entry.ended_at.is_none(), "cycle {cycle}");
        assert_eq!(entry.last_activity_at, F::at(now), "cycle {cycle}");
    }
    assert_eq!(ledger.owners(id).len(), 1 + MAX_OWNERS);
    // Bounded: at most `MAX_OWNERS` stretches of nobody, and never stuck in
    // one.
    for _ in 0..(MAX_OWNERS / 2 + 8) {
        unsure(&mut now);
        certain(&mut now);
    }
    assert_eq!(nobody_stretches(), MAX_OWNERS);
    assert_eq!(ledger.owners(id).len(), 1 + 2 * MAX_OWNERS);
    assert!(!ledger.is_uncounted(id) && ledger.entry(id).unwrap().ended_at.is_none());
    unsure(&mut now);
    assert!(!ledger.is_uncounted(id));
    assert_eq!(nobody_stretches(), MAX_OWNERS);
}

/// A ledger written before `unattributed` existed still loads.
#[test]
fn a_ledger_from_before_unsure_sessions_loads() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("cloud-ledger.json");
    let ledger = file_ledger(root.path());
    observe(
        &ledger,
        vec![live(F::SESSION_A).build()],
        &[F::SESSION_A],
        &[],
        &[],
        0.0,
    );
    ledger.save_now();
    let mut object: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    object.as_object_mut().unwrap().remove("unattributed");
    std::fs::write(&file, serde_json::to_vec(&object).unwrap()).unwrap();
    assert!(file_ledger(root.path()).entry(F::SESSION_A).is_some());
}

#[test]
fn owners_without_empty_or_repeated_stretches() {
    let (t1, t2) = (F::at(0.0), F::at(60.0));
    let o = |from: Option<SystemTime>, key: &str| SessionOwner::new(from, key);
    assert_eq!(
        SessionOwners::normalized(&[o(None, "a"), o(Some(t1), ""), o(Some(t1), "a")]),
        vec![o(None, "a")]
    );
    assert_eq!(
        SessionOwners::normalized(&[o(None, "a"), o(Some(t1), ""), o(Some(t2), "b")]),
        vec![o(None, "a"), o(Some(t1), ""), o(Some(t2), "b")]
    );
    assert_eq!(
        SessionOwners::normalized(&[o(None, "a"), o(Some(t1), "a")]),
        vec![o(None, "a")]
    );
}

/// Regression (review finding 22): capture paused (sync off) while a session
/// went away: when it resumes, the session ends at its last activity, not when
/// capture resumed.
#[test]
fn a_session_gone_while_capture_paused_ends_at_its_last_activity() {
    let ledger = memory_ledger();
    let a = F::SESSION_A;
    observe(
        &ledger,
        vec![live(a).active(100.0).build()],
        &[a],
        &[],
        &[],
        100.0,
    );
    ledger.forget_run_state();
    let resumed = 2.0 * 24.0 * 3600.0;
    observe(&ledger, vec![], &[], &[], &[], resumed);
    assert_eq!(
        settle(&ledger, &[], resumed + 61.0),
        vec![ledger.entry(a).unwrap().key()]
    );
    assert_eq!(ledger.entry(a).unwrap().ended_at, Some(F::at(100.0)));
}

/// Regression (review finding 7): a session resumed under another account is
/// split. The first account's part ends when the second's process started;
/// the owners say who ran it from when; each part keeps its own account.
#[test]
fn a_session_resumed_under_another_account_is_split() {
    let ledger = memory_ledger();
    let (personal, work) = (F::account(), F::work_account());
    let a = F::SESSION_A;
    observe(
        &ledger,
        vec![live_for(a, &personal).active(600.0).build()],
        &[a],
        &[],
        &[],
        600.0,
    );
    // Its limit hit, the window switches account and resumes the session.
    let resumed_at = F::at(900.0);
    observe(
        &ledger,
        vec![live_for(a, &work)
            .active(960.0)
            .started(950.0)
            .process(900.0)
            .build()],
        &[a],
        &[],
        &[],
        960.0,
    );
    let segments = ledger.segments(a);
    assert_eq!(
        segments
            .iter()
            .map(|s| s.account_key.as_str())
            .collect::<Vec<_>>(),
        vec![F::ACCOUNT_KEY, F::WORK_ACCOUNT_KEY]
    );
    assert_eq!(segments[0].ended_at, Some(resumed_at));
    assert_eq!(segments[0].identity_id, personal.identity_id);
    assert_eq!(segments[1].ended_at, None);
    assert_eq!(segments[1].started_at, resumed_at);
    assert_eq!(segments[1].identity_id, work.identity_id);
    assert_eq!(
        ledger.owners(a),
        vec![
            SessionOwner::new(None, F::ACCOUNT_KEY),
            SessionOwner::new(Some(resumed_at), F::WORK_ACCOUNT_KEY)
        ]
    );
    assert_eq!(ledger.entry(a).unwrap().account_key, F::WORK_ACCOUNT_KEY);
    // Seen under its account again: no new split.
    observe(
        &ledger,
        vec![live_for(a, &work).active(1000.0).build()],
        &[a],
        &[],
        &[],
        1000.0,
    );
    assert_eq!(ledger.owners(a).len(), 2);
    // Back to the first account: its part reopens, the owners go on.
    observe(
        &ledger,
        vec![live_for(a, &personal)
            .active(2000.0)
            .process(1990.0)
            .build()],
        &[a],
        &[],
        &[],
        2000.0,
    );
    assert_eq!(
        ledger
            .owners(a)
            .iter()
            .map(|o| o.account_key.as_str())
            .collect::<Vec<_>>(),
        vec![F::ACCOUNT_KEY, F::WORK_ACCOUNT_KEY, F::ACCOUNT_KEY]
    );
    assert_eq!(ledger.entry_of(a, F::ACCOUNT_KEY).unwrap().ended_at, None);
    assert_eq!(
        ledger.entry_of(a, F::WORK_ACCOUNT_KEY).unwrap().ended_at,
        Some(F::at(1990.0))
    );
    assert_eq!(ledger.count(), 2);

    // Running under both at once (two windows): which one a response came
    // from can't be told, so it stays with the one it ran as.
    let both = memory_ledger();
    observe(
        &both,
        vec![live_for(a, &personal).build()],
        &[F::SESSION_B],
        &[],
        &[],
        0.0,
    );
    observe(
        &both,
        vec![live_for(a, &personal).build(), live_for(a, &work).build()],
        &[a],
        &[],
        &[],
        10.0,
    );
    assert_eq!(
        both.segments(a)
            .iter()
            .map(|s| s.account_key.as_str())
            .collect::<Vec<_>>(),
        vec![F::ACCOUNT_KEY]
    );
    assert_eq!(both.owners(a).len(), 1);
    observe(
        &both,
        vec![
            live_for(F::SESSION_C, &personal).build(),
            live_for(F::SESSION_C, &work).build(),
        ],
        &[F::SESSION_C],
        &[],
        &[],
        20.0,
    );
    assert!(!both.knows(F::SESSION_C));
}

#[test]
fn owners_say_who_ran_the_session_when() {
    let (a, b) = ("key-a", "key-b");
    let (t1, t2) = (F::at(0.0), F::at(100.0));
    let o = |from: Option<SystemTime>, key: &str| SessionOwner::new(from, key);
    let owners = vec![o(None, a), o(Some(t1), b), o(Some(t2), a)];
    let ms = Duration::from_secs(1);
    assert_eq!(SessionOwners::owner_at(None, &owners), a);
    assert_eq!(SessionOwners::owner_at(Some(t1 - ms), &owners), a);
    assert_eq!(SessionOwners::owner_at(Some(t1), &owners), b);
    assert_eq!(
        SessionOwners::owner_at(Some(t2 + Duration::from_secs(5)), &owners),
        a
    );
    assert_eq!(SessionOwners::owner_at(Some(t1), &[]), "");
    let single = vec![o(None, a)];
    // Counted with one owner up to before the hand-over: still right.
    assert!(SessionOwners::agree(&single, &owners[..2], Some(t1 - ms)));
    assert!(!SessionOwners::agree(&single, &owners[..2], Some(t1)));
    assert!(!SessionOwners::agree(&single, &[o(None, b)], None));
    assert_eq!(SessionOwners::stretches(a, &single), None);
    let stretches = SessionOwners::stretches(a, &owners).unwrap();
    assert_eq!(stretches.len(), 2);
    assert!(SessionOwners::contains(&stretches, Some(t1 - ms)));
    assert!(!SessionOwners::contains(
        &stretches,
        Some(t1 + Duration::from_secs(50))
    ));
    assert!(SessionOwners::contains(&stretches, Some(t2)));
    assert!(!SessionOwners::contains(&stretches, None));
}

#[test]
fn something_that_is_not_a_session_is_never_captured() {
    let ledger = memory_ledger();
    let mut bad_id = live("not-a-uuid").build();
    bad_id.session_id = "not-a-uuid".to_owned();
    let mut bad_key = live(F::SESSION_B).build();
    bad_key.account_key = "email-account".to_owned();
    let no_cwd = live(F::SESSION_C).cwd("").build();
    observe(&ledger, vec![bad_id, bad_key, no_cwd], &[], &[], &[], 0.0);
    assert_eq!(ledger.count(), 0);
}

#[test]
fn live_capture_takes_over_a_backfilled_session() {
    let ledger = memory_ledger();
    let mut found = backfilled(F::SESSION_A);
    found.project_name = "old".to_owned();
    found.ended_at = Some(F::base());
    assert_eq!(
        ledger.record_backfill(&[found.clone()], &BTreeMap::new()),
        1
    );
    assert_eq!(ledger.record_backfill(&[found], &BTreeMap::new()), 0);
    observe(
        &ledger,
        vec![live(F::SESSION_A).build()],
        &[F::SESSION_A],
        &[],
        &[],
        0.0,
    );
    let entry = ledger.entry(F::SESSION_A).unwrap();
    assert_eq!(entry.origin, Origin::Live);
    assert_eq!(entry.project_name, "app");
    assert_eq!(entry.ended_at, None);
}

#[test]
fn writes_are_throttled_to_the_newest_value() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.json");
    let file: StateFile<Vec<String>> = StateFile::new(
        Some(path.clone()),
        Some(Arc::new(StdSecureFiles)),
        Duration::from_millis(200),
    );
    file.save(|| vec!["first".to_owned()]);
    file.save(|| vec!["second".to_owned()]);
    assert!(!path.exists());
    file.flush();
    assert_eq!(file.load(), Some(vec!["second".to_owned()]));
    #[cfg(unix)]
    assert_eq!(mode_of(&path), 0o600);
    file.save(|| vec!["third".to_owned()]);
    file.save_now(&vec!["now".to_owned()]);
    file.flush();
    assert_eq!(file.load(), Some(vec!["now".to_owned()]));
    // Not persisted: never touches the disk.
    let memory: StateFile<Vec<String>> =
        StateFile::new(Some(root.path().join("memory.json")), None, Duration::ZERO);
    memory.save_now(&vec!["x".to_owned()]);
    assert!(!root.path().join("memory.json").exists());
}

#[test]
fn the_ledger_keeps_its_capacity_dropping_the_oldest() {
    let ledger = memory_ledger();
    let uuid = |n: usize| format!("00000000-0000-4000-8000-{n:012}");
    let found: Vec<CloudLedgerEntry> = (0..10_003)
        .map(|n| {
            let mut entry = backfilled(&uuid(n));
            entry.last_activity_at = F::at(n as f64);
            entry
        })
        .collect();
    assert_eq!(ledger.record_backfill(&found, &BTreeMap::new()), 10_003);
    assert_eq!(ledger.count(), 10_000);
    assert!(!ledger.knows(&uuid(0)) && !ledger.knows(&uuid(2)));
    assert!(ledger.knows(&uuid(3)) && ledger.knows(&uuid(10_002)));
}

#[test]
fn refine_fills_in_what_a_later_look_learned() {
    let ledger = memory_ledger();
    let id = F::SESSION_A;
    ledger.record_backfill(&[backfilled(id)], &BTreeMap::new());
    let k = key(id, F::ACCOUNT_KEY);
    ledger.refine(
        &k,
        Some(F::at(50.0)),
        Some("A title"),
        Some("/t.jsonl"),
        Some(F::at(40.0)),
    );
    let entry = ledger.entry_by_key(&k).unwrap();
    assert_eq!(entry.last_activity_at, F::at(50.0));
    assert_eq!(entry.title.as_deref(), Some("A title"));
    assert_eq!(entry.transcript_path.as_deref(), Some("/t.jsonl"));
    // The end is never before the last activity; a title once set stays.
    assert_eq!(entry.ended_at, Some(F::at(50.0)));
    ledger.refine(&k, None, Some("Another"), None, None);
    assert_eq!(
        ledger.entry_by_key(&k).unwrap().title.as_deref(),
        Some("A title")
    );
    ledger.refine("nope|nope", Some(F::at(1.0)), None, None, None);
}

#[test]
fn windows_project_paths_keep_their_case() {
    let resolver = path_resolver(
        PathStyle::Windows,
        Path::new(r"C:\Users\me"),
        Arc::new(StdSecureFiles),
    );
    let ledger = SessionLedger::new(StateFile::memory(), resolver);
    let (a, b) = (F::SESSION_A, F::SESSION_B);
    observe(
        &ledger,
        vec![
            live(a).cwd(r"C:\Users\me\code\app").build(),
            live(b).cwd(r"~\Code\App/").build(),
        ],
        &[a, b],
        &[],
        &[],
        0.0,
    );
    let first = ledger.entry(a).unwrap();
    assert_eq!(first.project_path, r"C:\Users\me\code\app");
    assert_eq!(first.project_name, "app");
    // `~` is expanded against the home; the case stays as written.
    let second = ledger.entry(b).unwrap();
    assert_eq!(second.project_path, r"C:\Users\me\Code\App");
    assert_eq!(second.project_name, "App");
}

#[test]
fn a_ledger_round_trips_through_its_file_with_every_owner_kind() {
    let root = tempfile::tempdir().unwrap();
    let ledger = file_ledger(root.path());
    let (personal, work) = (F::account(), F::work_account());
    let a = F::SESSION_A;
    observe(
        &ledger,
        vec![live_for(a, &personal).active(600.0).build()],
        &[a],
        &[],
        &[],
        600.0,
    );
    observe(&ledger, vec![], &[a], &[a], &[], 700.0);
    observe(
        &ledger,
        vec![],
        &[F::SESSION_B],
        &[F::SESSION_B],
        &[],
        710.0,
    );
    observe(
        &ledger,
        vec![live_for(a, &work)
            .active(1100.0)
            .started(1000.0)
            .process(1000.0)
            .build()],
        &[a],
        &[],
        &[],
        1100.0,
    );
    ledger.save_now();
    let again = file_ledger(root.path());
    assert_eq!(again.entries(), ledger.entries());
    assert_eq!(again.owners(a), ledger.owners(a));
    assert!(again.is_uncounted(F::SESSION_B));
    assert_eq!(again.segments(a).len(), 2);
}

// ---- A shared history: only what the app saw running counts ----

/// Regression (double counting): a session first seen certain whose
/// transcript is in a shared history (Claude Parallel Profiles) may have been
/// run by another account before (a conversation from before sync, continued
/// after an account switch): only its process's responses are its account's,
/// as for a session first seen unsure. One in its folder's own history counts
/// whole, and a session the ledger knows goes on as it was.
#[test]
fn a_session_first_seen_in_a_shared_history_counts_from_its_process() {
    let ledger = memory_ledger();
    let (a, b, c) = (F::SESSION_A, F::SESSION_B, F::SESSION_C);
    let process_start = F::at(500.0);
    let shared = live(a)
        .active(600.0)
        .started(550.0)
        .process(500.0)
        .shared(true)
        .build();
    observe(&ledger, vec![shared.clone()], &[a], &[], &[], 600.0);
    assert_eq!(
        ledger.owners(a),
        vec![
            SessionOwner::new(None, ""),
            SessionOwner::new(Some(process_start), F::ACCOUNT_KEY)
        ]
    );
    assert_eq!(ledger.entry(a).unwrap().started_at, process_start);
    assert!(!ledger.is_uncounted(a) && ledger.count() == 1);
    // Seen again (marked or not): nothing more changes.
    observe(&ledger, vec![shared], &[a], &[], &[], 700.0);
    observe(
        &ledger,
        vec![live(a).active(800.0).build()],
        &[a],
        &[],
        &[],
        800.0,
    );
    assert!(ledger.owners(a).len() == 2 && ledger.count() == 1);
    assert_eq!(ledger.entry(a).unwrap().started_at, process_start);

    // Its process's start unknown: from when the app first saw it.
    observe(
        &ledger,
        vec![live(b).active(650.0).started(640.0).shared(true).build()],
        &[b],
        &[],
        &[],
        650.0,
    );
    assert_eq!(
        ledger.owners(b).last(),
        Some(&SessionOwner::new(Some(F::at(640.0)), F::ACCOUNT_KEY))
    );

    // A folder's own history: the whole session is its account's.
    observe(
        &ledger,
        vec![live(c).process(500.0).build()],
        &[c],
        &[],
        &[],
        700.0,
    );
    assert_eq!(
        ledger.owners(c),
        vec![SessionOwner::new(None, F::ACCOUNT_KEY)]
    );
}

/// Regression (double counting): when a shared history's session stops
/// running, what is written after it is no one's until the app sees a
/// process of it again (another account may have continued it while the app
/// wasn't capturing). The same process back after a moment's absence loses
/// nothing; a session in its folder's own history is left as it was.
#[test]
fn a_shared_session_counts_only_what_the_app_saw_running() {
    let ledger = memory_ledger();
    let a = F::SESSION_A;
    let k = key(a, F::ACCOUNT_KEY);
    let running = live(a).active(100.0).process(0.0).shared(true).build();
    observe(&ledger, vec![running.clone()], &[a], &[], &[], 100.0);
    // Gone: ended a minute later, at when it went.
    settle(&ledger, &[], 110.0);
    assert_eq!(settle(&ledger, &[], 171.0), vec![k.clone()]);
    let gone = F::at(110.0) + UNCOUNTED_AFTER;
    assert_eq!(
        ledger.entry_by_key(&k).unwrap().ended_at,
        Some(F::at(110.0))
    );
    assert_eq!(
        ledger.owners(a),
        vec![
            SessionOwner::new(None, ""),
            SessionOwner::new(Some(F::at(0.0)), F::ACCOUNT_KEY),
            SessionOwner::new(Some(gone), "")
        ]
    );
    // Whatever an unseen process wrote meanwhile is no one's.
    assert_eq!(
        SessionOwners::owner_at(Some(F::at(500.0)), &ledger.owners(a)),
        ""
    );
    // A new process of it: counted from its start, in the same entry.
    let resumed_at = F::at(900.0);
    observe(
        &ledger,
        vec![live(a).active(950.0).started(900.0).process(900.0).build()],
        &[a],
        &[],
        &[],
        950.0,
    );
    let owners = ledger.owners(a);
    assert_eq!(
        owners[owners.len() - 2..],
        [
            SessionOwner::new(Some(gone), ""),
            SessionOwner::new(Some(resumed_at), F::ACCOUNT_KEY)
        ]
    );
    assert!(ledger.entry_by_key(&k).unwrap().ended_at.is_none() && ledger.count() == 1);

    // Missing for a minute, then the same process again: nothing is lost.
    let flicker = memory_ledger();
    observe(&flicker, vec![running.clone()], &[a], &[], &[], 100.0);
    settle(&flicker, &[], 110.0);
    settle(&flicker, &[], 171.0);
    observe(
        &flicker,
        vec![live(a).active(200.0).process(0.0).build()],
        &[a],
        &[],
        &[],
        200.0,
    );
    assert_eq!(
        flicker.owners(a),
        vec![
            SessionOwner::new(None, ""),
            SessionOwner::new(Some(F::at(0.0)), F::ACCOUNT_KEY)
        ]
    );
    assert!(flicker.entry_by_key(&k).unwrap().ended_at.is_none());

    // Its folder's own history: its end changes no owner.
    let own = memory_ledger();
    let mut own_running = running;
    own_running.in_shared_history = Some(false);
    observe(&own, vec![own_running], &[a], &[], &[], 100.0);
    settle(&own, &[], 110.0);
    assert_eq!(settle(&own, &[], 171.0), vec![k]);
    assert_eq!(own.owners(a), vec![SessionOwner::new(None, F::ACCOUNT_KEY)]);
}

/// A shared session resumed again and again by one account (each end a
/// stretch of nobody, each resume its account back) can still change hands:
/// only hand-overs between accounts count toward the bound.
#[test]
fn resuming_a_shared_session_often_never_blocks_another_account() {
    let ledger = memory_ledger();
    let a = F::SESSION_A;
    let (personal, work) = (F::account(), F::work_account());
    let mut now = 0.0;
    for round in 0..(MAX_OWNERS + 4) {
        now += 100.0;
        let mut running = live_for(a, &personal)
            .active(now)
            .started(now)
            .process(now)
            .build();
        if round == 0 {
            running.in_shared_history = Some(true);
        }
        observe(&ledger, vec![running], &[a], &[], &[], now);
        settle(&ledger, &[], now + 1.0);
        now += 62.0;
        settle(&ledger, &[], now);
    }
    assert_eq!(
        ledger
            .owners(a)
            .iter()
            .filter(|o| o.account_key.is_empty())
            .count(),
        MAX_OWNERS
    );
    now += 100.0;
    observe(
        &ledger,
        vec![live_for(a, &work)
            .active(now)
            .started(now)
            .process(now)
            .build()],
        &[a],
        &[],
        &[],
        now,
    );
    assert_eq!(
        ledger.owners(a).last(),
        Some(&SessionOwner::new(Some(F::at(now)), F::WORK_ACCOUNT_KEY))
    );
    assert!(ledger.entry_of(a, F::WORK_ACCOUNT_KEY).is_some());
}

/// Whether a session's history is shared is told once and kept with its
/// entries: a part another account takes over inherits it, it survives a
/// relaunch, and a session from before the app looked learns it later (and
/// a ledger file written before the field still loads).
#[test]
fn whether_a_history_is_shared_is_kept_with_the_session() {
    let root = tempfile::tempdir().unwrap();
    let ledger = file_ledger(root.path());
    let (a, b) = (F::SESSION_A, F::SESSION_B);
    let (personal, work) = (F::account(), F::work_account());
    observe(
        &ledger,
        vec![live_for(a, &personal)
            .active(100.0)
            .process(0.0)
            .shared(true)
            .build()],
        &[a],
        &[],
        &[],
        100.0,
    );
    assert_eq!(ledger.shared_history(a), Some(true));
    // Taken over by another account (not looked up again): the same history.
    observe(
        &ledger,
        vec![live_for(a, &work).active(300.0).process(250.0).build()],
        &[a],
        &[],
        &[],
        300.0,
    );
    assert_eq!(
        ledger
            .entry_of(a, F::WORK_ACCOUNT_KEY)
            .unwrap()
            .shared_history,
        Some(true)
    );
    ledger.save_now();
    let reloaded = file_ledger(root.path());
    assert_eq!(reloaded.shared_history(a), Some(true));
    assert_eq!(
        reloaded
            .owners(a)
            .iter()
            .map(|o| o.account_key.as_str())
            .collect::<Vec<_>>(),
        vec!["", F::ACCOUNT_KEY, F::WORK_ACCOUNT_KEY]
    );

    // Not looked up (an entry from before): unknown until an observation says.
    observe(&reloaded, vec![live(b).build()], &[b], &[], &[], 400.0);
    assert_eq!(reloaded.shared_history(b), None);
    observe(
        &reloaded,
        vec![live(b).shared(false).build()],
        &[b],
        &[],
        &[],
        410.0,
    );
    assert_eq!(reloaded.shared_history(b), Some(false));
    assert_eq!(
        reloaded.owners(b),
        vec![SessionOwner::new(None, F::ACCOUNT_KEY)]
    );
    // An entry keeps what it was told first: a later answer changes nothing.
    observe(
        &reloaded,
        vec![live(b).shared(true).build()],
        &[b],
        &[],
        &[],
        420.0,
    );
    assert_eq!(reloaded.shared_history(b), Some(false));
    assert_eq!(
        reloaded.owners(b),
        vec![SessionOwner::new(None, F::ACCOUNT_KEY)]
    );

    // A file written before the field: no `sharedHistory` anywhere in it.
    reloaded.save_now();
    let text = std::fs::read_to_string(root.path().join("cloud-ledger.json")).unwrap();
    assert!(text.contains("sharedHistory"));
    let older = tempfile::tempdir().unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    for entry in doc["sessions"].as_object_mut().unwrap().values_mut() {
        entry.as_object_mut().unwrap().remove("sharedHistory");
    }
    std::fs::write(older.path().join("cloud-ledger.json"), doc.to_string()).unwrap();
    let before = file_ledger(older.path());
    assert_eq!(before.count(), reloaded.count());
    assert_eq!(before.shared_history(a), None);
    assert_eq!(before.owners(a), reloaded.owners(a));
}

/// A shared history's session the hub placed late, lost for a while and then
/// handed to another account: each stretch where it should be.
#[test]
fn a_shared_session_placed_late_then_unsure_then_handed_over() {
    let ledger = memory_ledger();
    let a = F::SESSION_A;
    let (personal, work) = (F::account(), F::work_account());
    // Not placed yet: nothing is remembered; placed: counted from its
    // process's start.
    observe(&ledger, vec![], &[a], &[], &[a], 5.0);
    assert!(!ledger.knows(a));
    observe(
        &ledger,
        vec![live_for(a, &personal)
            .active(100.0)
            .process(0.0)
            .shared(true)
            .build()],
        &[a],
        &[],
        &[],
        100.0,
    );
    let from_start = vec![
        SessionOwner::new(None, ""),
        SessionOwner::new(Some(F::at(0.0)), F::ACCOUNT_KEY),
    ];
    assert_eq!(ledger.owners(a), from_start);
    // Unsure for a while, then certain again in the same process: nothing lost.
    observe(&ledger, vec![], &[a], &[a], &[], 200.0);
    observe(
        &ledger,
        vec![live_for(a, &personal).active(300.0).process(0.0).build()],
        &[a],
        &[],
        &[],
        300.0,
    );
    assert_eq!(ledger.owners(a), from_start);
    // Another account's new process takes over from its start.
    let resumed_at = F::at(450.0);
    observe(
        &ledger,
        vec![live_for(a, &work)
            .active(500.0)
            .started(450.0)
            .process(450.0)
            .build()],
        &[a],
        &[],
        &[],
        500.0,
    );
    assert_eq!(
        ledger.owners(a),
        vec![
            SessionOwner::new(None, ""),
            SessionOwner::new(Some(F::at(0.0)), F::ACCOUNT_KEY),
            SessionOwner::new(Some(resumed_at), F::WORK_ACCOUNT_KEY)
        ]
    );
}

/// Regression (double counting): a session that changes hands while its
/// process runs (an account's key changed, a window switched) leaves the old
/// part's last response with it. The old part may never be sent again, so the
/// new part counting it too would count it twice.
#[test]
fn a_hand_over_leaves_the_old_parts_last_response_with_it() {
    let ledger = memory_ledger();
    let a = F::SESSION_A;
    let (personal, work) = (F::account(), F::work_account());
    let last_response = F::at(600.0);
    observe(
        &ledger,
        vec![live_for(a, &personal).active(600.0).process(0.0).build()],
        &[a],
        &[],
        &[],
        600.0,
    );
    observe(
        &ledger,
        vec![live_for(a, &work).active(700.0).process(0.0).build()],
        &[a],
        &[],
        &[],
        700.0,
    );
    let boundary = last_response + UNCOUNTED_AFTER;
    let owners = ledger.owners(a);
    assert_eq!(
        owners,
        vec![
            SessionOwner::new(None, F::ACCOUNT_KEY),
            SessionOwner::new(Some(boundary), F::WORK_ACCOUNT_KEY)
        ]
    );
    assert_eq!(
        SessionOwners::owner_at(Some(last_response), &owners),
        F::ACCOUNT_KEY
    );
    assert_eq!(
        ledger.entry_of(a, F::ACCOUNT_KEY).unwrap().ended_at,
        Some(boundary)
    );
    assert_eq!(
        ledger.entry_of(a, F::WORK_ACCOUNT_KEY).unwrap().started_at,
        boundary
    );
}

// ---- Backfill roots ----

/// A `SecureFiles` that knows only which paths are links and what each
/// canonicalises to: Windows-style paths on any host.
struct LinkFiles {
    links: BTreeSet<String>,
    canonical: BTreeMap<String, String>,
}

impl SecureFiles for LinkFiles {
    fn ensure_private_dir(&self, _: &Path) -> io::Result<()> {
        Err(io::Error::other("not used"))
    }
    fn write_atomic(&self, _: &Path, _: &[u8], _: WriteMode, _: Expect) -> io::Result<WriteResult> {
        Err(io::Error::other("not used"))
    }
    fn create_exclusive(&self, _: &Path, _: &[u8]) -> io::Result<bool> {
        Err(io::Error::other("not used"))
    }
    fn identity(&self, _: &Path) -> io::Result<FileIdentity> {
        Err(io::Error::other("not used"))
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        Ok(self.links.contains(path.to_string_lossy().as_ref()))
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        let text = path.to_string_lossy().into_owned();
        Ok(PathBuf::from(
            self.canonical.get(&text).cloned().unwrap_or(text),
        ))
    }
    fn is_private(&self, _: &Path) -> io::Result<bool> {
        Ok(true)
    }
}

fn folder(dir: &str, identity: &str, key: &str, since: Option<SystemTime>) -> Folder {
    Folder {
        config_dir: dir.to_owned(),
        identity_id: Some(IdentityId::from(identity)),
        account_key: Some(key.to_owned()),
        login: Some(identity.to_owned()),
        signed_in_since: since,
    }
}

fn infrastructure(dir: &str) -> Folder {
    Folder {
        config_dir: dir.to_owned(),
        identity_id: None,
        account_key: None,
        login: None,
        signed_in_since: None,
    }
}

#[test]
fn a_junctioned_projects_folder_is_not_a_backfill_root() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\Me");
    let since = Some(F::base());
    let own = r"C:\Users\Me\.claude-own";
    let linked = r"C:\Users\Me\.claude-linked";
    let solo = r"C:\Users\Me\.claude-solo";
    let files = LinkFiles {
        // `.claude-linked\projects` is a junction to another folder's history,
        // spelled in another case by the OS.
        links: BTreeSet::from([format!(r"{linked}\projects")]),
        canonical: BTreeMap::from([(
            format!(r"{linked}\projects"),
            r"c:\users\me\.CLAUDE-OWN\projects".to_owned(),
        )]),
    };
    let all_dirs = |_: &str| true;
    let folders = vec![
        folder(own, "uuid:a", "key-a", since),
        folder(linked, "uuid:b", "key-b", since),
        folder(solo, "uuid:c", "key-c", since),
        infrastructure(r"C:\Users\Me\.claude-shared"),
    ];
    // The linked folder is not a root, and it makes the folder it reaches
    // shared (case-insensitively); the solo folder is its own.
    let roots = backfill::roots_with(&folders, &paths, &files, &all_dirs);
    assert_eq!(
        roots
            .iter()
            .map(|r| r.identity_id.as_str())
            .collect::<Vec<_>>(),
        vec!["uuid:c"]
    );
    assert_eq!(roots[0].projects, format!(r"{solo}\projects"));
    assert_eq!(roots[0].config_dir, solo);
    assert_eq!(roots[0].signed_in_since, F::base());
    // Without the link, the first folder is a root too, in the case the disk
    // spells it.
    let files = LinkFiles {
        links: BTreeSet::new(),
        canonical: BTreeMap::from([(
            format!(r"{own}\projects"),
            r"C:\USERS\ME\.claude-own\projects".to_owned(),
        )]),
    };
    let roots = backfill::roots_with(&folders[..1], &paths, &files, &all_dirs);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].projects, r"C:\USERS\ME\.claude-own\projects");
    // A link anywhere on the way (the profile folder itself) shares it.
    let files = LinkFiles {
        links: BTreeSet::from([r"C:\Users\Me".to_owned()]),
        canonical: BTreeMap::new(),
    };
    assert!(backfill::roots_with(&folders[..1], &paths, &files, &all_dirs).is_empty());
    // Since when it is that account's isn't known: nothing is read.
    let undated = vec![folder(own, "uuid:a", "key-a", None)];
    let files = LinkFiles {
        links: BTreeSet::new(),
        canonical: BTreeMap::new(),
    };
    assert!(backfill::roots_with(&undated, &paths, &files, &all_dirs).is_empty());
    // Not a folder at all: skipped.
    assert!(backfill::roots_with(&folders[..1], &paths, &files, &|_| false).is_empty());
}

#[cfg(unix)]
#[test]
fn only_a_folders_own_history_is_backfilled() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    // The temporary folder itself may sit behind a link (macOS `/var`).
    let root = StdSecureFiles.canonical(temp.path()).unwrap();
    let at = |name: &str| root.join(name);
    let text = |name: &str| at(name).to_string_lossy().into_owned();
    // Its own history.
    std::fs::create_dir_all(at(".claude-own/projects")).unwrap();
    // Claude Parallel Profiles: projects/ linked to a shared history.
    std::fs::create_dir_all(at(".claude-shared/projects")).unwrap();
    std::fs::create_dir_all(at(".claude-a")).unwrap();
    symlink(at(".claude-shared/projects"), at(".claude-a/projects")).unwrap();
    // A folder with no account to give it (not signed in, or a store).
    std::fs::create_dir_all(at(".claude-store/projects")).unwrap();

    let paths = Paths::native(Path::new(HOME));
    let since = Some(F::base());
    let folders = vec![
        folder(&text(".claude-own"), "uuid:a", "key-a", since),
        folder(&text(".claude-a"), "uuid:b", "key-b", since),
        infrastructure(&text(".claude-shared")),
        infrastructure(&text(".claude-store")),
        folder(&text(".claude-missing"), "uuid:c", "key-c", since),
    ];
    let roots = backfill::roots(&folders, &paths, &StdSecureFiles);
    assert_eq!(
        roots
            .iter()
            .map(|r| r.identity_id.as_str())
            .collect::<Vec<_>>(),
        vec!["uuid:a"]
    );
    assert_eq!(roots[0].projects, text(".claude-own/projects"));
    assert_eq!(roots[0].signed_in_since, F::base());
    // Since when it is that account's isn't known: nothing is read (finding 0).
    let mut undated = folders[0].clone();
    undated.signed_in_since = None;
    assert!(backfill::roots(&[undated], &paths, &StdSecureFiles).is_empty());

    // A real projects folder that another known folder links to is shared too.
    std::fs::create_dir_all(at(".claude-b")).unwrap();
    symlink(at(".claude-own/projects"), at(".claude-b/projects")).unwrap();
    let linked_to = backfill::roots(
        &[folders[0].clone(), infrastructure(&text(".claude-b"))],
        &paths,
        &StdSecureFiles,
    );
    assert!(linked_to.is_empty());
}

// ---- Whether a running session's history is shared ----

/// A running session's history is shared when it is reached through a link,
/// or another known folder reaches it; then its lines from before its
/// process may be another account's (see the ledger's notes).
#[cfg(unix)]
#[test]
fn a_shared_history_is_one_reached_through_a_link_or_by_another_folder() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let paths = Paths::new(PathStyle::native(), HOME);
    let at = |name: &str| root.join(name);
    let text = |name: &str| at(name).to_string_lossy().into_owned();
    let slug = "-Users-me-code-app";
    let make = |name: &str| std::fs::create_dir_all(at(name)).unwrap();
    // Claude Parallel Profiles: a window folder's projects\ links to the
    // shared store.
    make(&format!(".claude-shared/projects/{slug}"));
    make(".claude-windows/a1b2c3d4e5f6");
    symlink(
        at(".claude-shared/projects"),
        at(".claude-windows/a1b2c3d4e5f6/projects"),
    )
    .unwrap();
    // A folder with a history of its own, and one whose project folder alone
    // is a link.
    make(&format!(".claude-own/projects/{slug}"));
    make(".claude-per-repo/projects");
    symlink(
        at(&format!(".claude-shared/projects/{slug}")),
        at(&format!(".claude-per-repo/projects/{slug}")),
    )
    .unwrap();
    let transcript = |dir: &str| text(&format!("{dir}/projects/{slug}/{}.jsonl", F::SESSION_A));
    let bare = |dir: &str| BackfillFolder {
        config_dir: text(dir),
        identity_id: None,
        account_key: None,
        signed_in_since: None,
    };
    let known = vec![
        bare(".claude-windows/a1b2c3d4e5f6"),
        bare(".claude-own"),
        bare(".claude-per-repo"),
    ];
    let shared = |transcript: Option<&str>, dir: &str, folders: &[BackfillFolder]| {
        backfill::is_shared(transcript, dir, folders, &paths, &StdSecureFiles)
    };

    // Through the link, with or without a transcript yet.
    let window = ".claude-windows/a1b2c3d4e5f6";
    assert!(shared(Some(&transcript(window)), &text(window), &known));
    assert!(shared(None, &text(window), &known));
    assert!(shared(
        Some(&transcript(".claude-per-repo")),
        &text(".claude-per-repo"),
        &known
    ));
    // Its own: the folder itself among the known ones doesn't count.
    assert!(!shared(
        Some(&transcript(".claude-own")),
        &text(".claude-own"),
        &known
    ));
    assert!(!shared(None, &format!("{}/", text(".claude-own")), &[]));
    // Nor do other spellings of the folder make it shared: a config folder
    // reached through a link (kept with dotfiles), or another letter case.
    make(&format!("dotfiles/claude/projects/{slug}"));
    symlink(at("dotfiles/claude"), at(".claude-dotfiles")).unwrap();
    assert!(!shared(
        Some(&transcript(".claude-dotfiles")),
        &text(".claude-dotfiles"),
        &known
    ));
    let mut with_alias = known.clone();
    with_alias.push(bare("dotfiles/claude"));
    assert!(!shared(
        Some(&transcript(".claude-dotfiles")),
        &text(".claude-dotfiles"),
        &with_alias
    ));
    let upper = text(".CLAUDE-OWN");
    if Path::new(&upper).exists() {
        assert!(!shared(None, &upper, &known));
    }
    // Nothing there at all: its own.
    assert!(!shared(None, &text(".claude-nowhere"), &known));

    // Until another known folder links to it.
    make(".claude-adopted");
    symlink(at(".claude-own/projects"), at(".claude-adopted/projects")).unwrap();
    let mut adopted = known;
    adopted.push(bare(".claude-adopted"));
    assert!(shared(
        Some(&transcript(".claude-own")),
        &text(".claude-own"),
        &adopted
    ));
}

/// A `SecureFiles` that knows which paths are links and which file each path
/// is (volume, index), compared without regard to case as NTFS does:
/// Windows-style paths on any host.
struct IdFiles {
    links: BTreeSet<String>,
    ids: BTreeMap<String, (u64, u128)>,
}

impl IdFiles {
    fn new(links: &[&str], ids: &[(&str, u128)]) -> Self {
        IdFiles {
            links: links.iter().map(|p| p.to_lowercase()).collect(),
            ids: ids
                .iter()
                .map(|(p, index)| (p.to_lowercase(), (7, *index)))
                .collect(),
        }
    }
}

impl SecureFiles for IdFiles {
    fn ensure_private_dir(&self, _: &Path) -> io::Result<()> {
        Err(io::Error::other("not used"))
    }
    fn write_atomic(&self, _: &Path, _: &[u8], _: WriteMode, _: Expect) -> io::Result<WriteResult> {
        Err(io::Error::other("not used"))
    }
    fn create_exclusive(&self, _: &Path, _: &[u8]) -> io::Result<bool> {
        Err(io::Error::other("not used"))
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        let (volume, index) = *self
            .ids
            .get(&path.to_string_lossy().to_lowercase())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        // The times and size change with the folder's contents: they must
        // not matter.
        Ok(FileIdentity {
            volume,
            index,
            modified_ns: index as i128 * 31,
            size: index as u64 * 17,
        })
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        Ok(self.links.contains(&path.to_string_lossy().to_lowercase()))
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
    fn is_private(&self, _: &Path) -> io::Result<bool> {
        Ok(true)
    }
}

/// The same rule over Windows paths: junctions, letter case and aliases of a
/// folder are told apart by the file each names (volume and file index), not
/// by how it is spelled.
#[test]
fn a_shared_history_is_told_by_file_identity_not_spelling() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\Me");
    let slug = "-Users-me-code-app";
    let window = r"C:\Users\Me\.claude-windows\a1b2c3d4e5f6";
    let own = r"C:\Users\Me\.claude-own";
    let per_repo = r"C:\Users\Me\.claude-per-repo";
    let dotfiles = r"C:\Users\Me\.claude-dotfiles";
    let adopted = r"C:\Users\Me\.claude-adopted";
    let transcript = |dir: &str| format!(r"{dir}\projects\{slug}\{}.jsonl", F::SESSION_A);
    let files = IdFiles::new(
        &[
            &format!(r"{window}\projects"),
            &format!(r"{per_repo}\projects\{slug}"),
            &format!(r"{adopted}\projects"),
        ],
        &[
            // The shared store, reached by the window folder's junction.
            (window, 1),
            (&format!(r"{window}\projects"), 2),
            (&format!(r"{window}\projects\{slug}"), 3),
            (own, 4),
            (&format!(r"{own}\projects"), 5),
            (&format!(r"{own}\projects\{slug}"), 6),
            (per_repo, 7),
            (&format!(r"{per_repo}\projects"), 8),
            (&format!(r"{per_repo}\projects\{slug}"), 3),
            // A folder kept elsewhere and reached under two names.
            (dotfiles, 9),
            (&format!(r"{dotfiles}\projects"), 10),
            (&format!(r"{dotfiles}\projects\{slug}"), 11),
            (r"C:\Users\Me\dotfiles\claude", 9),
            (r"C:\Users\Me\dotfiles\claude\projects", 10),
            // Linked to the own folder's history.
            (adopted, 12),
            (&format!(r"{adopted}\projects"), 5),
        ],
    );
    let bare = |dir: &str| BackfillFolder {
        config_dir: dir.to_owned(),
        identity_id: None,
        account_key: None,
        signed_in_since: None,
    };
    let known = vec![bare(window), bare(own), bare(per_repo)];
    let shared = |transcript_path: Option<&str>, dir: &str, folders: &[BackfillFolder]| {
        backfill::is_shared(transcript_path, dir, folders, &paths, &files)
    };

    // A junction to the shared store, with or without a transcript yet; a
    // project folder alone that is a link.
    assert!(shared(Some(&transcript(window)), window, &known));
    assert!(shared(None, window, &known));
    assert!(shared(Some(&transcript(per_repo)), per_repo, &known));
    // Its own, however the folder is spelled among the known ones.
    assert!(!shared(Some(&transcript(own)), own, &known));
    assert!(!shared(
        Some(&transcript(own)),
        r"c:\USERS\me\.CLAUDE-OWN",
        &known
    ));
    assert!(!shared(None, &format!(r"{own}\"), &[]));
    // Another name for the same folder (kept with dotfiles, reached through
    // a link) is not another folder; a changed size or time says nothing.
    let mut aliased = known.clone();
    aliased.push(bare(r"C:\Users\Me\dotfiles\claude"));
    assert!(!shared(Some(&transcript(dotfiles)), dotfiles, &known));
    assert!(!shared(Some(&transcript(dotfiles)), dotfiles, &aliased));
    // A folder the file system knows nothing of is its own.
    assert!(!shared(None, r"C:\Users\Me\.claude-nowhere", &known));
    // Until another known folder reaches its projects.
    let mut reached = known;
    reached.push(bare(adopted));
    assert!(shared(Some(&transcript(own)), own, &reached));
}

// ---- Folder logins ----

/// Regression (review finding 0): "signed in as <login> since <date>", set
/// when the app first sees a folder's login and again when it changes;
/// unknown before; unchanged by a sign-out.
#[test]
fn folders_remember_since_when_they_are_signed_in_as_whom() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(PathStyle::native(), HOME);
    let make = || {
        CloudFolderLogins::in_support(root.path(), Arc::new(StdSecureFiles), true, paths.clone())
    };
    let logins = make();
    let t0 = F::base();
    let work = "/Users/me/.claude-work";
    let one = |login: &str| BTreeMap::from([(work.to_owned(), login.to_owned())]);
    assert_eq!(logins.since(work, Some("a")), None);
    logins.observe(&one("a"), t0);
    logins.observe(
        &BTreeMap::from([("/Users/me/.claude-work/".to_owned(), "a".to_owned())]),
        F::at(60.0),
    );
    assert_eq!(logins.since(work, Some("a")), Some(t0));
    assert_eq!(logins.since(work, Some("b")), None);
    assert_eq!(logins.since(work, None), None);
    // Signed out for a while (left out of the logins), back as the same
    // account.
    logins.observe(&BTreeMap::new(), F::at(120.0));
    logins.observe(&one("a"), F::at(180.0));
    assert_eq!(logins.since(work, Some("a")), Some(t0));
    // /login as someone else: from then on.
    logins.observe(&one("b"), F::at(240.0));
    assert_eq!(logins.since(work, Some("b")), Some(F::at(240.0)));
    assert_eq!(logins.since(work, Some("a")), None);
    logins.save_now();
    let file = root.path().join("cloud-folder-logins.json");
    #[cfg(unix)]
    assert_eq!(mode_of(&file), 0o600);
    assert_eq!(make().since(work, Some("b")), Some(F::at(240.0)));
    assert!(!std::fs::read_to_string(&file)
        .unwrap()
        .contains("me@example.com"));

    // A login is who the folder's own oauthAccount names.
    let a = backfill::login(Some("U"), Some("O"), Some("me@example.com"));
    assert_eq!(
        a,
        backfill::login(Some("u"), Some("o"), Some("ME@example.com"))
    );
    assert_ne!(
        a,
        backfill::login(Some("u"), Some("other"), Some("me@example.com"))
    );
    assert_eq!(backfill::login(None, None, None), None);
    // Only an organization isn't a login; padding is ignored.
    assert_eq!(backfill::login(None, Some("o"), None), None);
    assert_eq!(
        backfill::login(Some(" u "), None, None),
        backfill::login(Some("u"), None, None)
    );
}

#[test]
fn folder_keys_are_case_insensitive_on_windows() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\Me");
    let logins = CloudFolderLogins::new(StateFile::memory(), paths);
    logins.observe(
        &BTreeMap::from([(r"C:\Users\Me\.claude".to_owned(), "a".to_owned())]),
        F::base(),
    );
    assert_eq!(
        logins.since(r"c:\users\me\.claude", Some("a")),
        Some(F::base())
    );
    assert_eq!(
        logins.since(r"C:/Users/Me/.claude/", Some("a")),
        Some(F::base())
    );
    assert_eq!(logins.since(r"C:\Users\Me\.claude-work", Some("a")), None);
    // Seen again in another spelling: not a change.
    logins.observe(
        &BTreeMap::from([(r"c:\USERS\me\.CLAUDE".to_owned(), "a".to_owned())]),
        F::at(60.0),
    );
    assert_eq!(
        logins.since(r"C:\Users\Me\.claude", Some("a")),
        Some(F::base())
    );
}

#[test]
fn backfill_folders_carry_their_login_and_date() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\Me");
    let history = CloudFolderLogins::new(StateFile::memory(), paths.clone());
    let login = backfill::login(Some(F::ACCOUNT_UUID), None, Some("me@example.com")).unwrap();
    history.observe(
        &BTreeMap::from([(r"C:\Users\Me\.claude-own".to_owned(), login.clone())]),
        F::at(-3600.0),
    );
    let offered = vec![
        BackfillFolder {
            config_dir: r"c:\users\me\.claude-own".to_owned(),
            identity_id: Some(IdentityId::from(F::IDENTITY_ID)),
            account_key: Some(F::ACCOUNT_KEY.to_owned()),
            signed_in_since: None,
        },
        BackfillFolder {
            config_dir: r"C:\Users\Me\.claude-shared".to_owned(),
            identity_id: None,
            account_key: None,
            signed_in_since: None,
        },
    ];
    let logins = BTreeMap::from([(r"C:\Users\Me\.claude-own".to_owned(), login.clone())]);
    let folders = backfill::folders(&offered, &logins, &history, &paths);
    assert_eq!(folders[0].login.as_deref(), Some(login.as_str()));
    assert_eq!(folders[0].signed_in_since, Some(F::at(-3600.0)));
    assert_eq!(folders[0].account_key.as_deref(), Some(F::ACCOUNT_KEY));
    assert_eq!(folders[1].login, None);
    assert_eq!(folders[1].signed_in_since, None);
    // Someone else signed in there now: no date until the app has seen it.
    let other = BTreeMap::from([(
        r"C:\Users\Me\.claude-own".to_owned(),
        "someone-else".to_owned(),
    )]);
    assert_eq!(
        backfill::folders(&offered, &other, &history, &paths)[0].signed_in_since,
        None
    );
}

// ---- From the hub ----

/// A running session is captured with its title, never its prompt.
#[test]
fn a_running_session_is_captured_with_its_title_never_its_prompt() {
    let identity = IdentityId::from(F::IDENTITY_ID);
    let account_key = F::ACCOUNT_KEY;
    let mut view = session_view(F::SESSION_A);
    view.transcript_path = Some(PathBuf::from(format!(
        "/Users/me/.claude/projects/-Users-me-code-app/{}.jsonl",
        F::SESSION_A
    )));
    view.cost_usd = Some(0.42);
    view.model = Some("claude-opus-4-5".to_owned());
    view.pid_started = Some(F::at(-30.0));
    let observed = feed::observation(&view, &identity, account_key).unwrap();
    assert_eq!(observed.account_key, F::ACCOUNT_KEY);
    assert_eq!(observed.process_started_at, Some(F::at(-30.0)));
    assert_eq!(observed.cost_usd, Some(0.42));
    assert_eq!(observed.entrypoint.as_deref(), Some("cli"));
    assert_eq!(observed.config_dir.as_deref(), Some("/Users/me/.claude"));
    assert_eq!(observed.cwd, "/Users/me/code/app");
    assert_eq!(observed.started_at, F::base());
    // A title made up from the folder name is not sent.
    assert_eq!(observed.title, None);
    view.title = "my-project-3".to_owned();
    assert_eq!(
        feed::observation(&view, &identity, account_key)
            .unwrap()
            .title,
        None
    );
    // The hub's `title` is the panel's: with no title of the session's own
    // (`title_from_folder`), it falls back to the first prompt, which never
    // leaves this PC.
    view.title = "MY SECRET PROMPT: fix the login for jane@example.com".to_owned();
    assert_eq!(
        feed::observation(&view, &identity, account_key)
            .unwrap()
            .title,
        None
    );
    view.title_from_folder = false;
    view.title = "  Fix the notch ".to_owned();
    assert_eq!(
        feed::observation(&view, &identity, account_key)
            .unwrap()
            .title
            .as_deref(),
        Some("Fix the notch")
    );
    view.title = "   ".to_owned();
    assert_eq!(
        feed::observation(&view, &identity, account_key)
            .unwrap()
            .title,
        None
    );
    // An account in an organization: its key is the caller's to give.
    let work = IdentityId::from(F::WORK_IDENTITY_ID);
    assert_eq!(
        feed::observation(&view, &work, F::WORK_ACCOUNT_KEY)
            .unwrap()
            .account_key,
        F::WORK_ACCOUNT_KEY
    );
    // No working directory: nothing to capture.
    view.cwd = PathBuf::new();
    assert!(feed::observation(&view, &identity, account_key).is_none());
    // The ledger takes it as it is.
    let ledger = memory_ledger();
    let mut view = session_view(F::SESSION_A);
    view.title_from_folder = false;
    view.title = "Fix the notch".to_owned();
    let batch = feed::live_batch(&[view], F::at(5.0), &|id| {
        (id.as_str() == F::IDENTITY_ID).then(|| F::ACCOUNT_KEY.to_owned())
    });
    ledger.observe_batch(&batch, &BTreeMap::new());
    assert_eq!(
        ledger.entry(F::SESSION_A).unwrap().title.as_deref(),
        Some("Fix the notch")
    );
}

#[test]
fn placement_waits_for_the_grace_then_is_unsure() {
    let grace = Duration::from_secs(30);
    let identity = IdentityId::from(F::IDENTITY_ID);
    let mut view = session_view(F::SESSION_A);
    let base = F::base();
    // Certain at any age.
    assert_eq!(
        feed::placement(&view, base + grace * 100),
        Placement::Certain
    );
    // A folder not grouped yet waits, then is unsure.
    view.attribution = Attribution::Known(None);
    assert_eq!(feed::placement(&view, base), Placement::Waiting);
    assert_eq!(
        feed::placement(&view, base + Duration::from_secs(29)),
        Placement::Waiting
    );
    assert_eq!(feed::placement(&view, base + grace), Placement::Unsure);
    // A clock that went back is still within the grace.
    assert_eq!(
        feed::placement(&view, base - Duration::from_secs(5)),
        Placement::Waiting
    );
    // An attribution that began later waits from then.
    view.attribution_since = base + Duration::from_secs(100);
    assert_eq!(
        feed::placement(&view, base + Duration::from_secs(120)),
        Placement::Waiting
    );
    assert_eq!(
        feed::placement(&view, base + Duration::from_secs(130)),
        Placement::Unsure
    );
    view.attribution_since = base;
    // Not placed yet, as the hub says.
    view.attribution = Attribution::Waiting;
    assert_eq!(
        feed::placement(&view, base + Duration::from_secs(1)),
        Placement::Waiting
    );
    assert_eq!(feed::placement(&view, base + grace), Placement::Unsure);
    // Unsure for real (a mirrored default folder around a switch).
    view.attribution = Attribution::Unsure(Some(identity.clone()));
    assert_eq!(feed::placement(&view, base), Placement::Unsure);
    // Desktop-hosted without a host session id and registry status: the
    // entry just hasn't been read.
    view.entrypoint = Some("claude-desktop".to_owned());
    assert_eq!(feed::placement(&view, base), Placement::Waiting);
    assert_eq!(feed::placement(&view, base + grace), Placement::Unsure);
    view.host_session_id = Some("local_0123abcd-0123".to_owned());
    assert_eq!(feed::placement(&view, base), Placement::Unsure);
    view.host_session_id = None;
    view.registry_status = Some("busy".to_owned());
    assert_eq!(feed::placement(&view, base), Placement::Unsure);
    view.registry_status = None;
    view.entrypoint = Some("cli".to_owned());
    assert_eq!(feed::placement(&view, base), Placement::Unsure);
}

#[test]
fn a_live_batch_sorts_sessions_into_its_lists() {
    let mut certain = session_view(F::SESSION_A);
    certain.last_activity = F::at(3.0);
    let mut hidden = session_view(F::SESSION_B);
    hidden.attribution = Attribution::Known(Some(IdentityId::from("uuid:hidden")));
    let mut ungrouped = session_view(F::SESSION_C);
    ungrouped.attribution = Attribution::Known(None);
    let mut unsure = session_view("33333333-4444-4555-8666-777777777777");
    unsure.attribution = Attribution::Unsure(None);
    let mut old = session_view("44444444-5555-4666-8777-888888888888");
    old.attribution = Attribution::Known(None);
    old.attribution_since = F::at(-120.0);
    let batch = feed::live_batch(
        &[certain, hidden, ungrouped, unsure, old],
        F::at(10.0),
        &|id| (id.as_str() == F::IDENTITY_ID).then(|| F::ACCOUNT_KEY.to_owned()),
    );
    assert_eq!(batch.at, F::at(10.0));
    assert_eq!(batch.attributed.len(), 1);
    assert_eq!(batch.attributed[0].session_id, F::SESSION_A);
    assert_eq!(batch.attributed[0].last_activity_at, F::at(3.0));
    assert_eq!(batch.waiting, ids(&[F::SESSION_C]));
    assert_eq!(
        batch.unsure,
        ids(&[
            "33333333-4444-4555-8666-777777777777",
            "44444444-5555-4666-8777-888888888888"
        ])
    );
    // Every running session, counted for an account or not.
    assert_eq!(batch.live_ids.len(), 5);
}
