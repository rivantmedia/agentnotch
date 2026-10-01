//! `usage::planner` (UsageProbePlanner.swift, PP_UsageTests, AU§9.9): where an
//! account's usage check runs (one run folder, never a Claude Parallel
//! Profiles store), which login a folder must show for the check to run
//! there, and the store marker. Pure: folders are values, the disk is a
//! temporary folder.

mod usage_support;

use agentnotch_engine::model::{FolderKind, IdentityId};
use agentnotch_engine::usage::planner::{
    expected_login, folder_runs, has_store_marker, identity_of_login, is_default_folder,
    probe_folder, probe_folder_among, STORE_MARKER,
};
use std::time::{Duration, SystemTime};
use usage_support::*;

fn at(seconds_ago: u64, now: SystemTime) -> Option<SystemTime> {
    Some(now - Duration::from_secs(seconds_ago))
}

// MARK: - The folder the check runs in

/// PP_UsageTests.theCheckRunsInTheMostRecentlyActiveRunFolder, with the
/// Windows layout: `~\.claude`, a per-workspace copy in `~\.claude-windows`,
/// and a Claude Parallel Profiles store.
#[test]
fn the_check_runs_in_the_most_recently_active_run_folder() {
    let home = join(&fake_drive(), &["Users", "me"]);
    let main = run_folder(&home.join(".claude"), None);
    let window_dir = join(&home, &[".claude-windows", "801f9dd51396"]);
    let window = run_folder(&window_dir, Some(&window_dir.to_string_lossy()));
    let store = store_folder(&home.join(".claude-paras"));
    let now = SystemTime::now();
    let among = |folders: &[_], activity: &[Option<SystemTime>]| {
        probe_folder_among(folders, activity, &is_default_folder, false).map(|f| f.id)
    };

    let (main_id, window_id) = (main.id.clone(), window.id.clone());
    assert_eq!(
        among(&[main.clone(), window.clone()], &[at(60, now), Some(now)]),
        Some(window_id.clone())
    );
    assert_eq!(
        among(&[main.clone(), window.clone()], &[Some(now), at(60, now)]),
        Some(main_id.clone())
    );
    // No activity known: the default folder, even when listed second.
    assert_eq!(
        among(&[window.clone(), main.clone()], &[None, None]),
        Some(main_id.clone())
    );
    // Only one folder: that one.
    assert_eq!(
        among(std::slice::from_ref(&window), &[None]),
        Some(window_id)
    );
    // Never a store, even if handed one; nothing at all is nothing.
    assert_eq!(among(std::slice::from_ref(&store), &[Some(now)]), None);
    assert_eq!(among(&[], &[]), None);
    // A store beside a run folder is skipped, even when it is the newest.
    assert_eq!(
        among(&[store, main], &[Some(now), at(3600, now)]),
        Some(main_id)
    );
}

#[test]
fn a_folder_with_a_known_activity_beats_one_without() {
    let base = fake_drive();
    let (a, b) = (
        run_folder(&join(&base, &["a", ".claude-x"]), Some("a")),
        run_folder(&join(&base, &["b", ".claude-y"]), Some("b")),
    );
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let among = |folders: &[_], activity: &[Option<SystemTime>]| {
        probe_folder_among(folders, activity, &is_default_folder, false).map(|f| f.id)
    };
    assert_eq!(
        among(&[a.clone(), b.clone()], &[None, Some(old)]),
        Some(b.id.clone())
    );
    assert_eq!(
        among(&[a.clone(), b.clone()], &[Some(old), None]),
        Some(a.id.clone())
    );
    // A short activity list leaves the rest unknown rather than failing.
    assert_eq!(among(&[a.clone(), b.clone()], &[None]), Some(a.id.clone()));
    // Nothing known: the first folder.
    assert_eq!(among(&[a.clone(), b], &[None, None]), Some(a.id));
}

#[test]
fn equal_times_go_to_the_default_folder_then_the_smaller_path() {
    let home = join(&fake_drive(), &["Users", "me"]);
    let main = run_folder(&home.join(".claude"), None);
    let b = run_folder(&join(&home, &[".claude-windows", "b"]), Some("b"));
    let a = run_folder(&join(&home, &[".claude-windows", "a"]), Some("a"));
    let now = SystemTime::now();
    let among = |folders: &[_]| {
        let activity = vec![Some(now); folders.len()];
        probe_folder_among(folders, &activity, &is_default_folder, false).map(|f| f.id)
    };

    // The default folder wins a tie, wherever it is listed.
    assert_eq!(
        among(&[b.clone(), main.clone(), a.clone()]),
        Some(main.id.clone())
    );
    assert_eq!(among(&[main.clone(), a.clone()]), Some(main.id.clone()));
    // Otherwise the smaller path, in either order.
    assert_eq!(among(&[b.clone(), a.clone()]), Some(a.id.clone()));
    assert_eq!(among(&[a.clone(), b.clone()]), Some(a.id));
}

#[test]
fn a_mirrored_default_yields_to_the_identitys_own_folders() {
    // Claude Parallel Profiles mirrors the focused window's account into
    // ~/.claude, so with `prefers_own_folders` it is used only when it is
    // all there is (never set on native Windows, but the rule is kept).
    let home = join(&fake_drive(), &["Users", "me"]);
    let main = run_folder(&home.join(".claude"), None);
    let window = run_folder(
        &join(&home, &[".claude-windows", "801f9dd51396"]),
        Some("w"),
    );
    let now = SystemTime::now();
    let among = |folders: &[_], activity: &[Option<SystemTime>]| {
        probe_folder_among(folders, activity, &is_default_folder, true).map(|f| f.id)
    };

    // The default is newer, the own folder still wins.
    assert_eq!(
        among(&[main.clone(), window.clone()], &[Some(now), at(3600, now)]),
        Some(window.id.clone())
    );
    // The own folder has no activity: it is still the one (never the default).
    assert_eq!(
        among(&[main.clone(), window.clone()], &[Some(now), None]),
        Some(window.id)
    );
    // Only the default: it is used.
    assert_eq!(
        among(std::slice::from_ref(&main), &[Some(now)]),
        Some(main.id)
    );
}

#[test]
fn the_default_folder_is_config_dir_unset_and_named_dot_claude() {
    let home = join(&fake_drive(), &["Users", "me"]);
    assert!(is_default_folder(&run_folder(&home.join(".claude"), None)));
    assert!(is_default_folder(&run_folder(
        &home.join(".claude"),
        Some("")
    )));
    // Windows paths are case-insensitive.
    assert!(is_default_folder(&run_folder(&home.join(".CLAUDE"), None)));
    // Pointed at by CLAUDE_CONFIG_DIR it is an explicit folder, not the default.
    assert!(!is_default_folder(&run_folder(
        &home.join(".claude"),
        Some(&home.join(".claude").to_string_lossy())
    )));
    assert!(!is_default_folder(&run_folder(
        &home.join(".claude-work"),
        None
    )));
}

// MARK: - probe_folder over the registry

#[test]
fn probe_folder_reads_the_accounts_folders_and_their_last_seen_time() {
    let home = join(&fake_drive(), &["Users", "me"]);
    let now = SystemTime::now();
    let main = seen(
        run_folder(&home.join(".claude"), None),
        now - Duration::from_secs(600),
    );
    let window = seen(
        run_folder(
            &join(&home, &[".claude-windows", "801f9dd51396"]),
            Some("w"),
        ),
        now,
    );
    let store = seen(store_folder(&home.join(".claude-paras")), now);
    let other = seen(run_folder(&home.join(".claude-other"), Some("o")), now);
    let mine = account("uuid:u-1", Some("me@x.com"), &[&main, &window], &[&store]);
    let theirs = account("uuid:u-2", Some("you@x.com"), &[&other], &[]);
    let folders = [main.clone(), window.clone(), store, other.clone()];
    let accounts = [mine, theirs];

    let id = |text: &str| IdentityId::from(text);
    // The newest run folder of the account; its store is never considered.
    assert_eq!(
        probe_folder(&accounts, &folders, &id("uuid:u-1")).map(|f| f.id),
        Some(window.id.clone())
    );
    assert_eq!(
        probe_folder(&accounts, &folders, &id("uuid:u-2")).map(|f| f.id),
        Some(other.id)
    );
    // Not an account; a folder the registry no longer lists is skipped.
    assert_eq!(probe_folder(&accounts, &folders, &id("uuid:nobody")), None);
    assert_eq!(
        probe_folder(&accounts, std::slice::from_ref(&main), &id("uuid:u-1")).map(|f| f.id),
        Some(main.id)
    );
    assert_eq!(probe_folder(&accounts, &[], &id("uuid:u-1")), None);
}

#[test]
fn an_account_only_a_store_holds_has_no_probe_folder() {
    let home = join(&fake_drive(), &["Users", "me"]);
    let store = seen(store_folder(&home.join(".claude-paras")), SystemTime::now());
    // Even a registry that wrongly lists the store among the run folders
    // (a stale classification): the folder's own kind decides.
    let wrong = account("uuid:u-1", Some("me@x.com"), &[&store], &[]);
    let right = account("uuid:u-1", Some("me@x.com"), &[], &[&store]);
    let id = IdentityId::from("uuid:u-1");
    assert_eq!(store.kind, FolderKind::Store);
    assert_eq!(
        probe_folder(&[wrong], std::slice::from_ref(&store), &id),
        None
    );
    assert_eq!(probe_folder(&[right], &[store], &id), None);
}

#[test]
fn equal_last_seen_times_keep_the_default_folder() {
    let home = join(&fake_drive(), &["Users", "me"]);
    let now = SystemTime::now();
    let main = seen(run_folder(&home.join(".claude"), None), now);
    let window = seen(
        run_folder(&join(&home, &[".claude-windows", "a"]), Some("a")),
        now,
    );
    let acct = account("uuid:u-1", None, &[&window, &main], &[]);
    assert_eq!(
        probe_folder(
            &[acct],
            &[main.clone(), window],
            &IdentityId::from("uuid:u-1")
        )
        .map(|f| f.id),
        Some(main.id)
    );
}

// MARK: - Who a folder must be signed in as

#[test]
fn the_expected_login_comes_from_the_account() {
    let plain = account("uuid:Acct-1", Some("Me@Work.com"), &[], &[]);
    assert_eq!(
        expected_login(&plain),
        expected(Some("Me@Work.com"), Some("Acct-1"), None)
    );
    let split = account("uuid:acct-1/org-9", None, &[], &[]);
    assert_eq!(
        expected_login(&split),
        expected(None, Some("acct-1"), Some("org-9"))
    );
    let by_email = account("email:me@work.com", Some("me@work.com"), &[], &[]);
    assert_eq!(
        expected_login(&by_email),
        expected(Some("me@work.com"), None, None)
    );
    let by_folder = account("dir:c:\\users\\me\\.claude-x", None, &[], &[]);
    assert_eq!(expected_login(&by_folder), expected(None, None, None));
}

#[test]
fn a_folder_runs_as_the_account_by_email_first() {
    let wanted = expected(Some("me@work.com"), Some("u-1"), None);
    // The email decides, whatever the case, even when a mirrored
    // `.claude.json` kept another account's UUID.
    assert!(folder_runs(
        Some(&login(Some("ME@Work.com"), Some("u-1"), None)),
        &wanted
    ));
    assert!(folder_runs(
        Some(&login(Some("me@work.com"), Some("u-OTHER"), None)),
        &wanted
    ));
    assert!(!folder_runs(
        Some(&login(Some("you@work.com"), Some("u-1"), None)),
        &wanted
    ));
    // A different email with the same UUID is another login.
    assert!(!folder_runs(
        Some(&login(Some("x@y.com"), Some("U-1"), None)),
        &wanted
    ));
}

#[test]
fn without_an_email_the_uuids_decide() {
    let wanted = expected(None, Some("U-1"), None);
    assert!(folder_runs(Some(&login(None, Some("u-1"), None)), &wanted));
    assert!(!folder_runs(Some(&login(None, Some("u-2"), None)), &wanted));
    // The login has an email but the account doesn't: UUIDs again.
    assert!(folder_runs(
        Some(&login(Some("a@b.c"), Some("u-1"), None)),
        &wanted
    ));
    // Nothing to compare on one side or the other: not it.
    assert!(!folder_runs(
        Some(&login(Some("a@b.c"), None, None)),
        &wanted
    ));
    assert!(!folder_runs(Some(&login(None, None, None)), &wanted));
    assert!(!folder_runs(
        Some(&login(None, Some("u-1"), None)),
        &expected(None, None, None)
    ));
    // Nobody signed in.
    assert!(!folder_runs(None, &wanted));
}

#[test]
fn one_login_in_two_organizations_is_two_accounts() {
    let wanted = expected(Some("me@work.com"), Some("u-1"), Some("org-A"));
    assert!(folder_runs(
        Some(&login(Some("me@work.com"), Some("u-1"), Some("ORG-a"))),
        &wanted
    ));
    // The same login, the other organization: not this account.
    assert!(!folder_runs(
        Some(&login(Some("me@work.com"), Some("u-1"), Some("org-B"))),
        &wanted
    ));
    // The organization only separates the same person: another email is
    // judged by the email rule (and fails it) in any organization.
    assert!(!folder_runs(
        Some(&login(Some("you@work.com"), Some("u-1"), Some("org-A"))),
        &wanted
    ));
    // A login that names no organization can't be told apart: the email rule.
    assert!(folder_runs(
        Some(&login(Some("me@work.com"), Some("u-1"), None)),
        &wanted
    ));
    // An account without a scope ignores the login's organization.
    let unscoped = expected(Some("me@work.com"), Some("u-1"), None);
    assert!(folder_runs(
        Some(&login(Some("me@work.com"), Some("u-1"), Some("org-B"))),
        &unscoped
    ));
}

#[test]
fn a_probe_reports_who_its_folder_ran_as() {
    let ours = IdentityId::from("uuid:u-1");
    let wanted = expected(Some("me@work.com"), Some("u-1"), None);
    let who = |login: Option<agentnotch_engine::model::Identity>| {
        identity_of_login(login.as_ref(), &wanted, &ours)
    };
    // The expected login is the expected id.
    assert_eq!(
        who(Some(login(Some("me@work.com"), Some("u-1"), None))),
        Some(ours.clone())
    );
    // Another login is a key of its own, never the expected one.
    assert_eq!(
        who(Some(login(Some("you@x.com"), Some("u-2"), Some("o-1")))),
        Some(IdentityId::from("uuid:u-2"))
    );
    assert_eq!(
        who(Some(login(Some("you@x.com"), None, None))),
        Some(IdentityId::from("email:you@x.com"))
    );
    // Nobody is signed in: nobody.
    assert_eq!(who(None), None);
    assert_eq!(who(Some(login(None, None, None))), None);
}

// MARK: - The store marker

#[test]
fn a_folder_with_the_stores_marker_is_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".claude-paras");
    let plain = dir.path().join(".claude");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(STORE_MARKER, ".parallel-accounts-store");
    assert!(!has_store_marker(&store));

    std::fs::write(store.join(STORE_MARKER), b"").unwrap();
    assert!(has_store_marker(&store));
    assert!(!has_store_marker(&plain));
    // A folder that isn't there has no marker.
    assert!(!has_store_marker(&dir.path().join("missing")));
}
