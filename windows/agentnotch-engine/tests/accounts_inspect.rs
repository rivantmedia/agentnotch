//! The read-only account inspection (`agentnotch.exe inspect-accounts`,
//! ClaudeAccountInspection.swift, AU§16) over a temporary home: a sample home
//! with two plain profiles (copied from `tests/fixtures/home-two-profiles`)
//! and Claude Parallel Profiles' layout. It names accounts, rings, run and
//! store folders, reads only `accountUuid` and `fetchedAtMs` of the cached
//! usage, and changes nothing. Also the fake process table's scripted
//! answers, which the discovery under it depends on.

mod accounts_support;

use accounts_support::*;
use agentnotch_engine::accounts::inspect::{inspect, read_login, report};
use agentnotch_engine::core::time;
use agentnotch_engine::hub::Hub;
use agentnotch_engine::platform::{EnvRead, Liveness, Processes, Roots};
use agentnotch_engine::testkit::{self, FakeProcesses, StdSecureFiles};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const WORK_UUID: &str = "5b1c2d3e-4f50-4a61-8b72-93a4b5c6d7e8";
const PERSONAL_UUID: &str = "c7d8e9f0-a1b2-4c3d-8e4f-5a6b7c8d9e0f";

/// `claude-acct-` and the first 12 hex digits of sha256(lower-cased UUID).
fn ring_id(uuid: &str) -> String {
    let digest = Sha256::digest(uuid.to_lowercase().as_bytes());
    let hex: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    format!("claude-acct-{hex}")
}

fn at_ms(ms: u64) -> String {
    time::iso8601(UNIX_EPOCH + Duration::from_millis(ms))
}

fn report_of(roots: &Roots) -> String {
    report(roots, &StdSecureFiles)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create a folder");
    for entry in std::fs::read_dir(from).expect("read the sample home") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a file");
        }
    }
}

/// Every file and folder under `root`, with the bytes of files and the
/// target of links: what a read-only tool must leave as it was.
fn tree(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(dir).expect("read a folder") {
            let path = entry.expect("an entry").path();
            let relative = path.strip_prefix(root).expect("inside").to_owned();
            let meta = std::fs::symlink_metadata(&path).expect("metadata");
            if meta.file_type().is_symlink() {
                into.insert(relative, format!("link {:?}", std::fs::read_link(&path)));
            } else if meta.is_dir() {
                into.insert(relative, "dir".to_owned());
                walk(root, &path, into);
            } else {
                let modified = meta.modified().ok();
                let bytes = std::fs::read(&path).expect("read a file");
                into.insert(relative, format!("file {modified:?} {bytes:?}"));
            }
        }
    }
    let mut into = BTreeMap::new();
    walk(root, root, &mut into);
    into
}

// ---- the committed sample home ----

#[test]
fn a_sample_home_with_two_plain_profiles() {
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/home-two-profiles");
    let temp = tempfile::tempdir().expect("a temporary folder");
    let roots = Roots::under(temp.path());
    copy_tree(&sample, &roots.home);
    let before = tree(&roots.home);
    let paths = roots.paths();
    let work = paths.abbreviate(&paths.join(paths.home(), ".claude-work"));
    let personal = paths.abbreviate(&paths.join(paths.home(), ".claude-personal"));

    let text = report_of(&roots);
    assert!(text.starts_with(&format!(
        "Claude accounts in {} (read-only inspection)\nClaude Parallel Profiles: not detected\n\nAccounts: 2\n",
        paths.home()
    )), "{text}");
    // Both accounts, with their emails, UUID prefixes and rings.
    assert!(
        text.contains("sam.work@example.com (accountUuid 5b1c2d3e…)"),
        "{text}"
    );
    assert!(
        text.contains("sam.home@example.com (accountUuid c7d8e9f0…)"),
        "{text}"
    );
    assert!(
        text.contains(&format!("ring:   {}", ring_id(WORK_UUID))),
        "{text}"
    );
    assert!(
        text.contains(&format!("ring:   {}", ring_id(PERSONAL_UUID))),
        "{text}"
    );
    assert!(text.contains("· Team"), "the plan: {text}");
    assert!(text.contains("· Max"), "the plan: {text}");
    // Run folders shown with `~`; no stores.
    assert!(text.contains(&format!("runs:   {work}\n")), "{text}");
    assert!(text.contains(&format!("runs:   {personal}\n")), "{text}");
    assert!(text.contains("stores: none"), "{text}");
    // Its own cached usage counts; the other's names someone else.
    assert!(
        text.contains(&format!(
            "cached usage: freshest in {work}, fetched {}",
            at_ms(1_790_000_100_000)
        )),
        "{text}"
    );
    assert!(
        text.contains("cached usage: none matching this account"),
        "{text}"
    );
    // Nothing but the two fields of the cache is read: no sentinel and no
    // field name from it reaches the text.
    assert!(!text.contains("SENTINEL"), "{text}");
    assert!(!text.contains("resets_at"), "{text}");
    // Install targets after consent: both run folders.
    let separator = paths.style().separator();
    assert!(
        text.contains("Install targets after consent (hooks + status line): 2"),
        "{text}"
    );
    assert!(
        text.contains(&format!("   {work}{separator}settings.json")),
        "{text}"
    );
    assert!(
        text.contains(&format!("   {personal}{separator}settings.json")),
        "{text}"
    );
    assert!(text.contains("Not signed in (no ring): none"), "{text}");
    assert!(
        text.contains("Infrastructure (never accounts): none"),
        "{text}"
    );

    // Read-only: not one byte, timestamp or entry changed, and no folder made.
    assert_eq!(tree(&roots.home), before);
    assert!(!roots.support.exists(), "nothing of the app's is created");

    // The structure behind the text.
    let found = inspect(&roots, &StdSecureFiles);
    assert!(!found.extension_detected);
    assert_eq!(found.accounts.len(), 2);
    assert_eq!(found.folders.len(), 2);
    assert!(found
        .folders
        .iter()
        .all(|f| f.kind == "run" && !f.corrected));
    assert_eq!(
        found.folders[0].account_uuid_prefix.as_deref(),
        Some("c7d8e9f0")
    );
}

#[test]
fn an_empty_home_has_no_accounts() {
    let home = Home::new();
    let text = report_of(&home.roots);
    assert!(text.contains("Accounts: 0"), "{text}");
    assert!(
        text.contains("Claude Parallel Profiles: not detected"),
        "{text}"
    );
    assert!(
        text.contains("Install targets after consent (hooks + status line): 0"),
        "{text}"
    );
    assert!(!text.contains("settings.json"), "{text}");
}

#[test]
fn a_home_that_does_not_exist_is_not_created() {
    let temp = tempfile::tempdir().expect("a temporary folder");
    let roots = Roots::under(temp.path());
    let text = report_of(&roots);
    assert!(text.contains("Accounts: 0"), "{text}");
    assert!(!roots.home.exists());
}

// ---- Claude Parallel Profiles' layout ----

#[test]
fn the_parallel_profiles_layout_is_told_apart() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let before = tree(&home.roots.home);
    let text = report_of(&home.roots);
    let show = |relative: &str| home.paths.abbreviate(&home.path(relative));
    let separator = home.paths.style().separator();

    assert!(
        text.contains("Claude Parallel Profiles: detected; manifest stores: "),
        "{text}"
    );
    for store in CREATED_STORES {
        assert!(text.contains(&show(store)), "{store}: {text}");
    }
    // One ring per identity, however many folders it has.
    assert!(text.contains("Accounts: 2"), "{text}");
    assert!(
        text.contains(&format!("{PARAS} (accountUuid 29638aea…)")),
        "{text}"
    );
    assert!(
        text.contains(&format!("{BIIOS} (accountUuid 3d93ede5…)")),
        "{text}"
    );
    assert!(
        text.contains(&format!("ring:   {}", ring_id(PARAS_UUID))),
        "{text}"
    );
    assert!(
        text.contains(&format!("ring:   {}", ring_id(BIIOS_UUID))),
        "{text}"
    );

    let found = inspect(&home.roots, &StdSecureFiles);
    assert!(found.extension_detected);
    assert_eq!(found.accounts.len(), 2);
    let paras = found
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some(PARAS))
        .expect("paras");
    let biios = found
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some(BIIOS))
        .expect("biios");
    // Run folders: the default first, then the VS Code windows; stores apart.
    assert_eq!(
        paras.run_dirs,
        vec![
            "~".to_owned() + &separator.to_string() + ".claude",
            show(".claude-windows/801f9dd51396"),
            show(".claude-windows/b9fbb9ecd7cb"),
        ]
    );
    assert_eq!(
        paras.store_dirs,
        vec![show(".claude-paras"), show(".claude-paras-rivant-in")]
    );
    assert_eq!(biios.run_dirs, vec![show(".claude-windows/1bf3e8f92b11")]);
    assert_eq!(biios.store_dirs, vec![show(".claude-claude")]);
    assert_eq!(paras.ring_id, ring_id(PARAS_UUID));
    assert_eq!(biios.ring_id, ring_id(BIIOS_UUID));
    // Each account's freshest cached usage, wherever it is.
    assert_eq!(
        paras.cached_usage_folder.as_deref(),
        Some(show(".claude-windows/801f9dd51396").as_str())
    );
    assert_eq!(
        biios.cached_usage_folder.as_deref(),
        Some(show(".claude-windows/1bf3e8f92b11").as_str())
    );
    assert!(
        text.contains(&format!("fetched {}", at_ms(1_790_000_200_000))),
        "{text}"
    );
    assert!(
        text.contains(&format!("fetched {}", at_ms(1_790_000_300_000))),
        "{text}"
    );
    // The shared history is infrastructure; nothing is an unsigned folder.
    assert!(
        found.infrastructure.contains(&show(".claude-shared")),
        "{found:?}"
    );
    assert!(found.unsigned_folders.is_empty());
    // Hooks go only to run folders: the default and the three windows, never a store.
    assert_eq!(
        found.install_targets.len(),
        4,
        "{:?}",
        found.install_targets
    );
    for store in CREATED_STORES {
        assert!(
            !found.install_targets.contains(&show(store)),
            "{store} is a store"
        );
        assert!(!text.contains(&format!("   {}{separator}settings.json", show(store))));
    }
    // Both spellings of who the default folder is, and no mirror yet.
    assert_eq!(found.default_owner, None);
    assert!(
        found.folders.iter().all(|f| !f.corrected),
        "{:?}",
        found.folders
    );
    // Nothing changed on disk.
    assert_eq!(tree(&home.roots.home), before);
    assert!(!home.roots.support.exists());
}

// The extension mirrors another account into `~\.claude`: its email decides
// and the report says so.
#[test]
fn a_mirrored_default_folder_is_reported() {
    let home = Home::new();
    home.build_user_layout(true, true);
    home.mirror_into_default(BIIOS_UUID, PARAS);
    let found = inspect(&home.roots, &StdSecureFiles);
    let default = found
        .folders
        .iter()
        .find(|f| f.path.ends_with(".claude") && !f.path.contains("claude-"))
        .expect("the default folder");
    assert!(default.corrected, "{found:?}");
    assert!(found.default_owner.is_some());
    let text = report_of(&home.roots);
    assert!(
        text.contains("its accountUuid is another account's"),
        "{text}"
    );
    assert!(text.contains("belongs to"), "{text}");
}

// ---- what is read of a `.claude.json` ----

#[test]
fn only_the_allowed_fields_of_the_cache_are_read() {
    let login = read_login(
        br#"{
            "oauthAccount": {"accountUuid": "U-1", "emailAddress": "a@example.com"},
            "cachedUsageUtilization": {
                "accountUuid": "U-1", "fetchedAtMs": 1790000100000,
                "utilization": {"five_hour": {"utilization": 41, "note": "SENTINEL"}}
            },
            "projects": {"x": {"history": [{"display": "secret prompt"}]}}
        }"#,
    )
    .expect("a login");
    assert_eq!(login.cached_usage_account_uuid.as_deref(), Some("U-1"));
    assert_eq!(
        login.cached_usage_fetched_at,
        Some(UNIX_EPOCH + Duration::from_millis(1_790_000_100_000))
    );
    assert_eq!(
        login.identity.and_then(|i| i.email).as_deref(),
        Some("a@example.com")
    );
    // Not there, or not what it should be: nothing invented.
    let bare = read_login(br#"{"numStartups": 3}"#).expect("a file");
    assert_eq!(bare.identity, None);
    assert_eq!(bare.cached_usage_account_uuid, None);
    assert_eq!(bare.cached_usage_fetched_at, None);
    assert!(read_login(b"not json").is_none());
}

// ---- Hub::inspect_accounts ----

// A `fetchedAtMs` that is no date (a broken or hand-edited file) is not a
// reading: the report is printed without it, never brought down by it.
#[test]
fn a_cache_dated_outside_the_calendar_is_no_reading() {
    for ms in [
        "9.9e15",
        "8.64e15",
        "253402300800000",
        "-1",
        "-9.9e15",
        "1e300",
        "\"9900000000000000\"",
    ] {
        let file = format!(
            r#"{{"oauthAccount":{{"accountUuid":"u-1","emailAddress":"me@example.com"}},
                "cachedUsageUtilization":{{"accountUuid":"u-1","fetchedAtMs":{ms},"utilization":{{}}}}}}"#
        );
        let login = read_login(file.as_bytes()).expect("a file");
        assert_eq!(login.cached_usage_fetched_at, None, "{ms}");
        assert_eq!(login.cached_usage_account_uuid.as_deref(), Some("u-1"));

        let home = Home::new();
        home.mkdir(".claude-work/projects");
        home.write(".claude-work/.claude.json", &file);
        let text = report_of(&home.roots);
        assert!(text.contains("me@example.com"), "{ms}: {text}");
        assert!(
            text.contains("cached usage: none matching this account"),
            "{ms}: {text}"
        );
    }
    // The last second of the year 9999 is still a date.
    let last = read_login(
        br#"{"cachedUsageUtilization":{"accountUuid":"u-1","fetchedAtMs":253402300799000}}"#,
    )
    .expect("a file");
    assert_eq!(
        last.cached_usage_fetched_at.map(time::iso8601).as_deref(),
        Some("9999-12-31T23:59:59Z")
    );
}

#[test]
fn the_hub_prints_the_inspection() {
    let temp = tempfile::tempdir().expect("a temporary folder");
    let (platform, handles) = testkit::platform(temp.path());
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/home-two-profiles");
    copy_tree(&sample, &handles.roots.home);
    let text = Hub::inspect_accounts(&handles.roots, &platform);
    assert_eq!(text, report(&handles.roots, &StdSecureFiles));
    assert!(text.contains("Accounts: 2"), "{text}");
}

// ---- the fake process table's scripted answers ----

#[test]
fn a_scripted_liveness_beats_the_table_and_can_be_unknown() {
    let processes = FakeProcesses::default();
    // Unlisted: gone.
    assert_eq!(processes.liveness(10), Liveness::Gone);
    processes.add(10, 1, "claude.exe", t0());
    assert_eq!(processes.liveness(10), Liveness::Alive);
    // Could not be asked (no access): not "gone".
    processes.set_liveness(10, Liveness::Unknown);
    assert_eq!(processes.liveness(10), Liveness::Unknown);
    processes.set_liveness(11, Liveness::Alive);
    assert_eq!(processes.liveness(11), Liveness::Alive);
    processes.clear_liveness(10);
    assert_eq!(processes.liveness(10), Liveness::Alive);
    processes.remove(10);
    assert_eq!(processes.liveness(10), Liveness::Gone);
}

#[test]
fn another_users_process_is_alive_but_not_ours_to_read() {
    let processes = FakeProcesses::default();
    processes.add(20, 1, "claude.exe", t0());
    processes.set_config_dir_env(20, EnvRead::Set("X".to_owned()));
    assert_eq!(processes.same_user(20), Some(true));
    assert_eq!(processes.config_dir_env(20), EnvRead::Set("X".to_owned()));
    processes.set_other_user(20, true);
    assert_eq!(processes.liveness(20), Liveness::Alive);
    assert_eq!(processes.same_user(20), Some(false));
    assert_eq!(processes.config_dir_env(20), EnvRead::Unreadable);
    processes.set_other_user(20, false);
    assert_eq!(processes.same_user(20), Some(true));
    // A process nobody listed says nothing about its user.
    assert_eq!(processes.same_user(99), None);
}

// A session file whose process could not be asked about is no live session
// (only a process known to run makes a folder "running"): the folder with no
// login is then a suggestion, as with a dead one.
#[test]
fn an_unknown_process_makes_no_live_session() {
    let home = Home::new();
    home.write(".claude-idle/sessions/4242.json", "{}");
    home.mkdir(".claude-idle/projects");
    let facts_of = |home: &Home| {
        let snapshot = home.read(&[]);
        snapshot
            .folders
            .iter()
            .find(|f| home.paths.same(&f.path, &home.path(".claude-idle")))
            .expect("the folder")
            .clone()
    };
    assert!(!facts_of(&home).has_live_session, "nobody listed: gone");
    home.processes.add(4242, 1, "claude.exe", t0());
    assert!(facts_of(&home).has_live_session);
    home.processes.set_liveness(4242, Liveness::Unknown);
    assert!(!facts_of(&home).has_live_session, "unknown is not alive");
    home.processes.set_liveness(4242, Liveness::Alive);
    assert!(facts_of(&home).has_live_session);
}
