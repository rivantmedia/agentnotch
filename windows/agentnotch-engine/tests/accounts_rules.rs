//! The account rules that need no disk: the default folder's timeline, plan
//! and launch-command text, which folder can be an account, identity keys
//! and ring ids, colours and ordering. The Mac's PPFix_ReviewTests,
//! AccountRegistryTests, PP_LayoutTests, PP_IdentityTests and
//! A3_RingIdentityTests, each run under both path styles wherever a path is
//! involved, plus the Windows vectors (PowerShell quoting, drive and share
//! roots, two spellings of one folder).

mod accounts_support;

use accounts_support::{folder, mac, signed, win, FolderExt};
use agentnotch_engine::accounts::classify::{
    can_be_account, is_window_dir, looks_like_backup, parse_manifest,
};
use agentnotch_engine::accounts::folder::{launch_command_for, plan_name, Folder};
use agentnotch_engine::accounts::identities::{
    self, base_key, group, identity_keys, next_color_index, ring_id_for_account_key,
    ring_id_for_config_dir, ring_id_for_identity, Resolution,
};
use agentnotch_engine::accounts::registry::{sanitized_account_name, sorted};
use agentnotch_engine::accounts::timeline::{
    FolderAttribution, FolderIdentityTimeline, TimelineAnswer,
};
use agentnotch_engine::accounts::IdentityAccount;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{AccountId, FolderKind, IdentityId, RingId};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn both() -> [Paths; 2] {
    [mac(), win()]
}

fn dir(paths: &Paths, relative: &str) -> String {
    paths.join(paths.home(), relative)
}

// ---- the timeline ----

fn base() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000)
}

fn at(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        base() + Duration::from_secs(seconds as u64)
    } else {
        base() - Duration::from_secs(seconds.unsigned_abs())
    }
}

fn identity(id: &str) -> TimelineAnswer {
    TimelineAnswer::Identity(Some(id.to_owned()))
}

// PPFix_AttributionTests.theTimelineTellsWhoTheFolderRanAsAndWhenItCantTell
#[test]
fn the_timeline_tells_who_the_folder_ran_as_and_when_it_cant_tell() {
    let mut timeline = FolderIdentityTimeline::default();
    let first = timeline.observe(Some("uuid:p"), Some("p"), Some(at(0)), at(10), false);
    let same = timeline.observe(Some("uuid:p"), Some("p"), Some(at(15)), at(20), false);
    // The extension mirrors b in: same UUID, another email.
    let mirrored = timeline.observe(Some("uuid:b"), Some("p"), Some(at(25)), at(30), false);
    assert!(first && !same && mirrored);
    assert_eq!(timeline.identity_at(at(-1)), TimelineAnswer::Unknown);
    assert_eq!(timeline.identity_at(at(5)), identity("uuid:p"));
    assert_eq!(timeline.identity_at(at(22)), TimelineAnswer::Switching);
    assert_eq!(timeline.identity_at(at(26)), identity("uuid:b"));
    // A real login (the file's UUID changed) may take running sessions with it.
    timeline.observe(Some("uuid:x"), Some("x"), Some(at(40)), at(45), false);
    assert_eq!(timeline.identity_at(at(5)), TimelineAnswer::Switching);
    assert_eq!(timeline.identity_at(at(41)), identity("uuid:x"));

    // After a relaunch, a write nobody watched leaves the time between open.
    let mut resumed = FolderIdentityTimeline::default();
    resumed.observe(Some("uuid:p"), Some("p"), Some(at(0)), at(10), false);
    resumed.observe(Some("uuid:p"), Some("p"), Some(at(100)), at(200), true);
    assert_eq!(resumed.identity_at(at(50)), TimelineAnswer::Switching);
    assert_eq!(resumed.identity_at(at(150)), identity("uuid:p"));

    // Only the mirrored ~/.claude is attributed by time; others by who they
    // name now.
    assert_eq!(
        FolderAttribution::attribute(Some("uuid:b"), Some(&timeline), Some(at(5)), false),
        FolderAttribution::Known(Some("uuid:b".to_owned()))
    );
    assert_eq!(
        FolderAttribution::attribute(Some("uuid:b"), Some(&timeline), None, true),
        FolderAttribution::Unsure(Some("uuid:b".to_owned()))
    );
}

// FolderIdentityTimeline.observe/identity(at:) beyond the Swift test: what an
// attribution does with each answer, and the resumed look that changes nothing.
#[test]
fn a_mirrored_folder_is_attributed_by_time_and_unsure_when_time_cannot_tell() {
    let mut timeline = FolderIdentityTimeline::default();
    timeline.observe(Some("uuid:p"), Some("p"), Some(at(0)), at(10), false);
    timeline.observe(Some("uuid:b"), Some("p"), Some(at(25)), at(30), false);
    let attribute = |started| {
        FolderAttribution::attribute(Some("uuid:b"), Some(&timeline), Some(started), true)
    };
    assert_eq!(
        attribute(at(5)),
        FolderAttribution::Known(Some("uuid:p".to_owned()))
    );
    assert_eq!(
        attribute(at(22)),
        FolderAttribution::Unsure(Some("uuid:b".to_owned()))
    );
    assert_eq!(
        attribute(at(-5)),
        FolderAttribution::Unsure(Some("uuid:b".to_owned()))
    );
    assert_eq!(
        FolderAttribution::attribute(Some("uuid:b"), None, Some(at(5)), true),
        FolderAttribution::Unsure(Some("uuid:b".to_owned()))
    );
    assert_eq!(attribute(at(5)).best_guess(), Some("uuid:p"));
    assert_eq!(attribute(at(22)).best_guess(), Some("uuid:b"));

    // A relaunch where nothing was written since the last look changes nothing.
    let mut quiet = FolderIdentityTimeline::default();
    quiet.observe(Some("uuid:p"), Some("p"), Some(at(0)), at(10), false);
    assert!(!quiet.observe(Some("uuid:p"), Some("p"), Some(at(5)), at(200), true));
    assert_eq!(quiet.spans().len(), 1);
    assert_eq!(quiet.identity_at(at(150)), identity("uuid:p"));
    // The raw UUID is compared lower-cased.
    assert!(!quiet.observe(Some("uuid:p"), Some("P"), Some(at(5)), at(210), false));
    assert_eq!(quiet.current(), Some(Some("uuid:p")));
}

#[test]
fn a_timeline_keeps_at_most_24_spans_oldest_dropped_first() {
    let mut timeline = FolderIdentityTimeline::default();
    for n in 0..30_i64 {
        let id = format!("uuid:{n}");
        let raw = format!("{n}");
        assert!(timeline.observe(
            Some(&id),
            Some(&raw),
            Some(at(n * 10)),
            at(n * 10 + 5),
            false
        ));
    }
    assert_eq!(FolderIdentityTimeline::MAX_SPANS, 24);
    assert_eq!(timeline.spans().len(), 24);
    assert_eq!(timeline.spans()[0].identity.as_deref(), Some("uuid:6"));
    assert_eq!(timeline.current(), Some(Some("uuid:29")));
    // What the dropped spans covered is no longer known.
    assert_eq!(timeline.identity_at(at(20)), TimelineAnswer::Unknown);
}

#[test]
fn the_timeline_round_trips_through_its_persisted_form() {
    let mut timeline = FolderIdentityTimeline::default();
    timeline.observe(Some("uuid:p"), Some("P"), Some(at(0)), at(10), false);
    timeline.observe(Some("uuid:b"), Some("p"), Some(at(25)), at(30), false);
    timeline.observe(None, None, Some(at(40)), at(45), false);
    let saved = timeline.to_persisted();
    assert_eq!(saved.spans.len(), 3);
    assert_eq!(saved.spans[0].raw_uuid.as_deref(), Some("p"));
    assert!(saved.spans[2].is_login);
    assert_eq!(FolderIdentityTimeline::from_persisted(&saved), timeline);
    let json = serde_json::to_string(&saved).expect("encodes");
    let back = serde_json::from_str(&json).expect("decodes");
    assert_eq!(FolderIdentityTimeline::from_persisted(&back), timeline);
}

// ---- plans and launch commands ----

// ClaudeAccount.planName
#[test]
fn plan_names_come_from_the_tier_then_the_subscription() {
    assert_eq!(
        plan_name(Some("default_claude_max_20x"), Some("max")).as_deref(),
        Some("Max 20x")
    );
    assert_eq!(
        plan_name(Some("default_claude_max_5x"), None).as_deref(),
        Some("Max 5x")
    );
    assert_eq!(plan_name(None, Some("max")).as_deref(), Some("Max"));
    assert_eq!(
        plan_name(Some("other"), Some("pro")).as_deref(),
        Some("Pro")
    );
    assert_eq!(plan_name(None, Some("TEAM")).as_deref(), Some("Team"));
    assert_eq!(
        plan_name(None, Some("enterprise")).as_deref(),
        Some("Enterprise")
    );
    assert_eq!(plan_name(None, Some("free")).as_deref(), Some("Free"));
    assert_eq!(plan_name(None, Some("")), None);
    assert_eq!(plan_name(None, None), None);
}

// AccountRegistryTests.launchCommandQuotesTheFolder (Posix: the Mac's form)
#[test]
fn launch_command_quotes_the_folder_posix() {
    let paths = mac();
    let odd = folder(&paths, "/Users/u/my \"quoted\" $HOME dir").env("/Users/u/it's `x` $(y)");
    assert_eq!(
        odd.launch_command(&paths),
        r#"CLAUDE_CONFIG_DIR='/Users/u/it'\''s `x` $(y)' claude"#
    );
    assert_eq!(
        launch_command_for(PathStyle::Posix, "/Users/u/it's `x` $(y)"),
        odd.launch_command(&paths)
    );
    assert_eq!(
        folder(&paths, &paths.default_config_dir()).launch_command(&paths),
        "claude"
    );
    // A raw `~` is kept as the session spelled it (Claude Code keys the login by it).
    let tilde = folder(&paths, "/Users/me/.claude-w").env("~/.claude-w");
    assert_eq!(
        tilde.launch_command(&paths),
        "CLAUDE_CONFIG_DIR='~/.claude-w' claude"
    );
    // Without a spelling of its own the folder's path is used.
    assert_eq!(
        folder(&paths, "/Users/me/.claude-x").launch_command(&paths),
        "CLAUDE_CONFIG_DIR='/Users/me/.claude-x' claude"
    );
}

// New Windows vectors: PowerShell, `'` doubled, the typographic quotes too.
#[test]
fn launch_command_is_powershell_on_windows() {
    let paths = win();
    let work = folder(&paths, r"C:\Users\me\.claude-work").env(r"C:\Users\me\.claude-work");
    assert_eq!(
        work.launch_command(&paths),
        r"$env:CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work'; claude"
    );
    let quote = folder(&paths, r"C:\Users\O'Neil\.claude-w").env(r"C:\Users\O'Neil\.claude-w");
    assert_eq!(
        quote.launch_command(&paths),
        r"$env:CLAUDE_CONFIG_DIR='C:\Users\O''Neil\.claude-w'; claude"
    );
    assert_eq!(
        launch_command_for(PathStyle::Windows, "C:\\Users\\Paul\u{2019}s\\x"),
        "$env:CLAUDE_CONFIG_DIR='C:\\Users\\Paul\u{2019}\u{2019}s\\x'; claude"
    );
    assert_eq!(
        folder(&paths, r"C:\Users\me\.claude").launch_command(&paths),
        "claude"
    );
    // The default folder used WITH the variable set is not "the default".
    let set = folder(&paths, r"C:\Users\me\.claude").env(r"C:\Users\me\.claude");
    assert_eq!(
        set.launch_command(&paths),
        r"$env:CLAUDE_CONFIG_DIR='C:\Users\me\.claude'; claude"
    );
}

// ---- which folders are accounts ----

// AccountRegistryTests.backupNames (all arguments)
#[test]
fn backup_names() {
    let cases = [
        (".claude-backup", true),
        (".claude_bak", true),
        (".claude-old", true),
        (".claude-copy-2", true),
        (".claude-2024-01-01", true),
        (".claude-20250101", true),
        (".claude-work.bak", true),
        (".claude-orig", true),
        (".claude-work", false),
        (".claude-acme", false),
        (".claude_personal", false),
        (".claude-team2", false),
        (".claude-mem", false),
        (".claude-client-42", false),
    ];
    for (name, is_backup) in cases {
        assert_eq!(looks_like_backup(name), is_backup, "{name}");
    }
}

// AccountRegistryTests.homeIsNeverAnAccount (the pure part), and the Windows
// roots: a drive root and a share root are never accounts either.
#[test]
fn home_and_what_holds_it_is_never_an_account() {
    for paths in both() {
        let home = paths.home().to_owned();
        let parent = paths.parent(&home).expect("home has a parent");
        assert!(!can_be_account(&paths, &home, None, None), "{home}");
        assert!(
            !can_be_account(
                &paths,
                &format!("{home}{}", paths.style().separator()),
                None,
                None
            ),
            "{home} with a trailing separator"
        );
        assert!(!can_be_account(&paths, &parent, None, None), "{parent}");
        assert!(!can_be_account(&paths, &dir(&paths, ".."), None, None));
        assert!(can_be_account(&paths, &dir(&paths, ".claude"), None, None));
        assert!(can_be_account(
            &paths,
            &dir(&paths, ".claude-work"),
            None,
            None
        ));
        // Through a link: the resolved folder is the home folder or holds it.
        let linked = dir(&paths, ".claude-link");
        assert!(!can_be_account(&paths, &linked, Some(&home), None));
        assert!(!can_be_account(&paths, &linked, Some(&home), Some(&home)));
        assert!(!can_be_account(&paths, &linked, Some(&parent), Some(&home)));
        assert!(can_be_account(
            &paths,
            &linked,
            Some(&dir(&paths, "elsewhere")),
            Some(&home)
        ));
    }
    let posix = mac();
    assert!(!can_be_account(&posix, "/", None, None));
    assert!(!can_be_account(&posix, "/Users", None, None));
}

// New Windows vectors: drive and UNC share roots.
#[test]
fn a_drive_root_and_a_share_root_are_never_accounts() {
    let paths = win();
    for root in [r"C:\", "C:", "D:\\", r"\\server\share", r"\\server\share\"] {
        assert!(!can_be_account(&paths, root, None, None), "{root}");
    }
    // A folder on another drive or share is fine.
    for other in [r"D:\Work\.claude", r"\\server\share\me\.claude"] {
        assert!(can_be_account(&paths, other, None, None), "{other}");
    }
    // The case a Windows path is spelled in does not matter.
    assert!(!can_be_account(&paths, r"c:\users\ME", None, None));
    assert!(!can_be_account(&paths, r"C:\USERS", None, None));
}

// PP_LayoutTests.windowFoldersAreRecognised (Posix), and the same rules on Windows.
#[test]
fn window_folders_are_recognised() {
    let posix = Paths::new(PathStyle::Posix, "/Users/me");
    assert!(is_window_dir(
        &posix,
        "/Users/me/.claude-windows/801f9dd51396"
    ));
    assert!(is_window_dir(
        &posix,
        "/Users/me/.claude-windows/801f9dd51396/"
    ));
    assert!(!is_window_dir(&posix, "/Users/me/.claude-windows"));
    assert!(!is_window_dir(
        &posix,
        "/Users/me/.claude-windows/.manifest.json"
    ));
    assert!(!is_window_dir(&posix, "/Users/me/.claude-windows/a/b"));
    let manifest = parse_manifest(
        br#"{"stores": ["/Users/me/.claude-a/"], "created": []}"#,
        &posix,
    )
    .expect("a manifest");
    assert_eq!(manifest.stores, vec!["/Users/me/.claude-a".to_owned()]);
    assert!(parse_manifest(b"[]", &posix).is_none());

    let windows = win();
    assert!(is_window_dir(
        &windows,
        r"C:\Users\me\.claude-windows\801f9dd51396"
    ));
    assert!(is_window_dir(
        &windows,
        r"C:\Users\me\.claude-windows\801f9dd51396\"
    ));
    assert!(is_window_dir(
        &windows,
        r"c:/users/ME/.CLAUDE-WINDOWS/801f9dd51396"
    ));
    assert!(!is_window_dir(&windows, r"C:\Users\me\.claude-windows"));
    assert!(!is_window_dir(
        &windows,
        r"C:\Users\me\.claude-windows\.manifest.json"
    ));
    assert!(!is_window_dir(&windows, r"C:\Users\me\.claude-windows\a\b"));
    assert!(!is_window_dir(
        &windows,
        r"C:\Users\other\.claude-windows\801f9dd51396"
    ));
    let manifest = parse_manifest(
        br#"{"stores": ["C:\\Users\\me\\.claude-a\\"], "created": ["c:/Users/me/.claude-b"]}"#,
        &windows,
    )
    .expect("a manifest");
    assert_eq!(manifest.stores, vec![r"C:\Users\me\.claude-a".to_owned()]);
    assert_eq!(manifest.created, vec![r"C:\Users\me\.claude-b".to_owned()]);
    assert!(parse_manifest(b"[]", &windows).is_none());
}

// ---- identity keys ----

// PP_IdentityTests.identityKeysPreferTheUUIDAndJoinEmailOnlyLogins
#[test]
fn identity_keys_prefer_the_uuid_and_join_email_only_logins() {
    for paths in both() {
        let d = |name: &str| dir(&paths, name);
        let a =
            signed(&paths, &d(".claude-a"), Some("Me@X.dev"), Some("U-1")).kind(FolderKind::Store);
        let b = signed(&paths, &d(".claude-b"), Some("me@x.dev"), None);
        let c = signed(&paths, &d(".claude-c"), Some("other@x.dev"), None);
        let nobody = folder(&paths, &d(".claude-d"));
        let keys = identity_keys(&[a.clone(), b, c, nobody], None);
        assert_eq!(keys[&d(".claude-a")].key, "uuid:u-1");
        assert_eq!(keys[&d(".claude-b")].key, "uuid:u-1");
        assert_eq!(keys[&d(".claude-c")].key, "email:other@x.dev");
        assert!(!keys.contains_key(&d(".claude-d")));
        // Two logins with one email but two UUIDs (two organizations) stay apart.
        let e = signed(&paths, &d(".claude-e"), Some("me@x.dev"), Some("u-9"));
        assert_eq!(
            identity_keys(&[a, e], None)[&d(".claude-e")].key,
            "uuid:u-9"
        );
    }
}

#[test]
fn base_key_prefers_the_uuid_and_trims_and_lowercases() {
    assert_eq!(
        base_key(Some(" U-1 "), Some("a@b.co")).as_deref(),
        Some("uuid:u-1")
    );
    assert_eq!(
        base_key(None, Some(" A@B.co\t")).as_deref(),
        Some("email:a@b.co")
    );
    assert_eq!(
        base_key(Some("  "), Some("a@b.co")).as_deref(),
        Some("email:a@b.co")
    );
    assert_eq!(base_key(Some(""), None), None);
    assert_eq!(base_key(None, None), None);
}

// PPFix_AttributionTests.aStaleUuidOnlyTheDefaultHasIsNoSecondAccount
#[test]
fn a_stale_uuid_only_the_default_has_is_no_second_account() {
    for paths in both() {
        let d = |name: &str| dir(&paths, name);
        let main = signed(
            &paths,
            &d(".claude"),
            Some("claude@biios.in"),
            Some("u-gone"),
        );
        let window_dir = d(".claude-windows/1bf3e8f92b11");
        let window = signed(
            &paths,
            &window_dir,
            Some("claude@biios.in"),
            Some("u-biios"),
        )
        .env(&window_dir);
        let folders = [main, window];
        let mirrored = identity_keys(&folders, Some(&d(".claude")));
        assert_eq!(
            mirrored[&d(".claude")],
            Resolution {
                key: "uuid:u-biios".to_owned(),
                corrected: true
            }
        );
        let grouping = group(
            &folders,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            true,
            &paths,
        );
        assert_eq!(grouping.identities.len(), 1);
        assert_eq!(grouping.default_owner, None);
        // Without the extension nothing mirrors into ~/.claude: left as it is.
        assert_eq!(
            identity_keys(&folders, None)[&d(".claude")].key,
            "uuid:u-gone"
        );
    }
}

// PPFix_AttributionTests.oneLoginInTwoOrganizationsIsTwoAccounts
#[test]
fn one_login_in_two_organizations_is_two_accounts() {
    for paths in both() {
        let d = |name: &str| dir(&paths, name);
        let personal = signed(&paths, &d(".claude-me"), Some("me@x.dev"), Some("u-1"))
            .env(&d(".claude-me"))
            .organization("org-personal");
        let team = signed(&paths, &d(".claude-team"), Some("me@x.dev"), Some("u-1"))
            .env(&d(".claude-team"))
            .organization("org-team");
        let keys = identity_keys(&[personal.clone(), team.clone()], None);
        assert_eq!(keys[&d(".claude-me")].key, "uuid:u-1/org-personal");
        assert_eq!(keys[&d(".claude-team")].key, "uuid:u-1/org-team");
        let grouped = |folders: &[Folder], mirrors: bool| {
            group(
                folders,
                &BTreeMap::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                mirrors,
                &paths,
            )
        };
        let grouping = grouped(&[personal.clone(), team.clone()], false);
        assert_eq!(grouping.identities.len(), 2);
        let rings: BTreeSet<&RingId> = grouping.identities.iter().map(|i| &i.ring_id).collect();
        assert_eq!(rings.len(), 2);
        assert!(grouping
            .identities
            .iter()
            .all(|i| i.account_uuid.as_deref() == Some("u-1")));
        let scopes: BTreeSet<&str> = grouping
            .identities
            .iter()
            .filter_map(|i| i.organization_scope.as_deref())
            .collect();
        assert_eq!(scopes, BTreeSet::from(["org-personal", "org-team"]));

        // A mirrored ~/.claude keeps a stale UUID and organization: it joins
        // its email's owner, in the organization most of its folders have.
        let stale = signed(&paths, &d(".claude"), Some("me@x.dev"), Some("u-gone"))
            .organization("org-stale");
        let with_mirror = identity_keys(
            &[personal.clone(), team.clone(), stale.clone()],
            Some(&d(".claude")),
        );
        assert!(with_mirror[&d(".claude")].corrected);
        assert_eq!(with_mirror[&d(".claude")].key, "uuid:u-1/org-personal");
        assert_eq!(
            grouped(&[personal.clone(), team.clone(), stale], true)
                .identities
                .len(),
            2
        );

        // One organization (however many folders): one account, the ring id as always.
        let again = team.organization("org-personal");
        assert_eq!(
            identity_keys(&[personal, again], None)[&d(".claude-team")].key,
            "uuid:u-1"
        );
    }
}

// PPFix_AttributionTests.onlyATerminalFolderGetsALaunchCommand
#[test]
fn only_a_terminal_folder_gets_a_launch_command() {
    for paths in both() {
        let d = |name: &str| dir(&paths, name);
        let window_dir = d(".claude-windows/1bf3e8f92b11");
        let main = folder(&paths, &d(".claude"));
        let window = folder(&paths, &window_dir).env(&window_dir);
        let work = folder(&paths, &d(".claude-work")).env(&d(".claude-work"));
        let store = folder(&paths, &d(".claude-claude"))
            .env(&d(".claude-claude"))
            .kind(FolderKind::Store);
        let identity =
            |run: Vec<Folder>, stores: Vec<Folder>, windows: Vec<AccountId>, includes_default| {
                IdentityAccount {
                    id: IdentityId::from("uuid:x"),
                    ring_id: RingId::new("claude-acct-x"),
                    email: None,
                    display_name: None,
                    organization_name: None,
                    organization_uuid: None,
                    account_uuid: None,
                    subscription_type: None,
                    rate_limit_tier: None,
                    run_dirs: run,
                    store_dirs: stores,
                    custom_label: None,
                    color_index: 0,
                    is_hidden: false,
                    ring_hidden: false,
                    includes_default,
                    organization_scope: None,
                    window_dir_ids: windows,
                    default_label: None,
                    default_monogram: None,
                }
            };
        let windows = vec![window.id.clone()];
        assert_eq!(
            identity(vec![main, window.clone()], vec![], windows.clone(), true)
                .terminal_launch_command(&paths)
                .as_deref(),
            Some("claude")
        );
        assert_eq!(
            identity(
                vec![window.clone()],
                vec![store.clone()],
                windows.clone(),
                false
            )
            .terminal_launch_command(&paths),
            None
        );
        assert_eq!(
            identity(vec![], vec![store], vec![], false).terminal_launch_command(&paths),
            None
        );
        let command = identity(vec![window, work], vec![], windows, false)
            .terminal_launch_command(&paths)
            .expect("a terminal folder");
        assert!(command.contains(".claude-work"), "{command}");
        assert_eq!(
            command,
            launch_command_for(paths.style(), &d(".claude-work"))
        );
    }
}

// ---- ring ids ----

// A3_RingIdentityTests.defaultFolderIsTheClaudeRing
#[test]
fn default_folder_is_the_claude_ring() {
    for paths in both() {
        let sep = paths.style().separator();
        let default = dir(&paths, ".claude");
        assert_eq!(ring_id_for_config_dir(&paths, &default).as_str(), "claude");
        assert_eq!(
            ring_id_for_config_dir(&paths, "~/.claude").as_str(),
            "claude"
        );
        assert_eq!(
            ring_id_for_config_dir(&paths, &format!("{default}{sep}")).as_str(),
            "claude"
        );
        assert_eq!(
            ring_id_for_config_dir(&paths, &format!("{}{sep}.{sep}.claude", paths.home())).as_str(),
            "claude"
        );
    }
}

// A3_RingIdentityTests.dashFoldersKeepUpstreamsIds
#[test]
fn dash_folders_keep_upstreams_ids() {
    for paths in both() {
        let ring = |dir: &str| ring_id_for_config_dir(&paths, dir).as_str().to_owned();
        assert_eq!(ring(&dir(&paths, ".claude-work")), "claude-work");
        assert_eq!(ring("~/.claude-side"), "claude-side");
        assert_eq!(ring(&dir(&paths, ".claude-a.b")), "claude-a.b");
    }
}

fn is_hex(text: &str) -> bool {
    text.chars().all(|c| c.is_ascii_hexdigit())
}

// A3_RingIdentityTests.anythingElseIsHashedAndStable
#[test]
fn anything_else_is_hashed_and_stable() {
    for paths in both() {
        let ring = |dir: &str| ring_id_for_config_dir(&paths, dir).as_str().to_owned();
        let sep = paths.style().separator();
        let outside = |name: &str| match paths.style() {
            PathStyle::Posix => format!("/Volumes/{name}"),
            PathStyle::Windows => format!(r"D:\{name}"),
        };
        let elsewhere = ring(&format!("{}{sep}claude", outside("work")));
        assert!(elsewhere.starts_with("claude-dir-"), "{elsewhere}");
        assert_eq!(elsewhere.len(), "claude-dir-".len() + 8);
        assert!(is_hex(&elsewhere["claude-dir-".len()..]));
        // Stable across calls and path spellings.
        assert_eq!(
            ring(&format!("{}{sep}claude{sep}", outside("work"))),
            elsewhere
        );
        assert_eq!(
            ring(&format!("{}{sep}.{sep}claude", outside("work"))),
            elsewhere
        );
        // Different folders, different rings.
        assert_ne!(ring(&format!("{}{sep}claude", outside("other"))), elsewhere);
        // `~/.claude_x`, `~/.config/claude`, an empty slug and another user's
        // folder are not upstream ids.
        let other_user = paths.join(&paths.join(paths.home(), ".."), "other");
        for dir in [
            dir(&paths, ".claude_personal"),
            dir(&paths, ".config/claude"),
            format!("{}{sep}.claude-", paths.home()),
            paths.join(&other_user, ".claude-work"),
        ] {
            assert!(ring(&dir).starts_with("claude-dir-"), "{dir}");
        }
    }
}

// The exact hashes: sha256 of the normalized folder, first 4 bytes.
#[test]
fn a_claude_dir_ring_id_is_the_first_eight_hex_of_sha256_of_the_folder() {
    let posix = mac();
    assert_eq!(
        ring_id_for_config_dir(&posix, "/Volumes/work/claude").as_str(),
        "claude-dir-09fcc6a6"
    );
    assert_eq!(
        ring_id_for_config_dir(&posix, "/Users/me/.config/claude").as_str(),
        "claude-dir-a97604a1"
    );
    // On Windows the hash is over the lower-cased form (`Paths::key`).
    let windows = win();
    assert_eq!(
        ring_id_for_config_dir(&windows, r"C:\Work\x").as_str(),
        "claude-dir-8d62de09"
    );
}

// New Windows vector: two spellings of one folder are one ring.
#[test]
fn a_claude_dir_ring_id_is_the_same_for_two_spellings_of_one_windows_folder() {
    let windows = win();
    let ring = |dir: &str| ring_id_for_config_dir(&windows, dir);
    let one = ring(r"C:\Work\x");
    for spelling in [
        r"c:\work\X\",
        "C:/Work/x",
        r"c:\WORK\.\x",
        r"\\?\C:\Work\x",
        r"C:\Work\y\..\x",
    ] {
        assert_eq!(ring(spelling), one, "{spelling}");
    }
    assert_ne!(ring(r"C:\Work\y"), one);
    // The default and dash folders follow the same rule, whatever the case.
    assert_eq!(ring(r"c:\USERS\me\.CLAUDE").as_str(), "claude");
    assert_eq!(ring(r"C:\Users\ME\.Claude-Work").as_str(), "claude-work");
    // On the Mac the case is part of the name.
    let posix = mac();
    assert_ne!(
        ring_id_for_config_dir(&posix, "/Volumes/Work/x"),
        ring_id_for_config_dir(&posix, "/Volumes/work/x")
    );
}

// A3_RingIdentityTests.everyIdIsAClaudeRing
#[test]
fn every_id_is_a_claude_ring() {
    for paths in both() {
        let outside = match paths.style() {
            PathStyle::Posix => "/Volumes/x/claude",
            PathStyle::Windows => r"D:\x\claude",
        };
        for dir in [
            dir(&paths, ".claude"),
            dir(&paths, ".claude-work"),
            outside.to_owned(),
            dir(&paths, ".config/claude"),
        ] {
            let id = ring_id_for_config_dir(&paths, &dir);
            assert!(identities::is_claude_ring(id.as_str()), "{id}");
            // Upstream's `ClaudeProfile.isClaude(providerID:)`: "claude" or a "claude-" prefix.
            assert!(id.as_str() == "claude" || id.as_str().starts_with("claude-"));
        }
    }
    assert!(!identities::is_claude_ring("openai"));
    assert!(!identities::is_claude_ring("claudette"));
}

// ClaudeSummaries ring ids: `claude-acct-` + 12 hex of sha256 of the
// lower-cased value; the same in both path styles.
#[test]
fn identity_ring_ids_are_twelve_hex_of_the_sha256_of_the_lowercased_value() {
    assert_eq!(
        ring_id_for_account_key("u-1").as_str(),
        "claude-acct-a24a7f55f278"
    );
    assert_eq!(
        ring_id_for_account_key("U-1").as_str(),
        "claude-acct-a24a7f55f278"
    );
    assert_eq!(
        ring_id_for_account_key("u-1/org-personal").as_str(),
        "claude-acct-8c77a6bffa5a"
    );
    assert_eq!(
        ring_id_for_account_key("a@b.co").as_str(),
        "claude-acct-80305c9bb1bb"
    );
    for paths in both() {
        assert_eq!(
            ring_id_for_identity(&paths, "uuid:u-1").as_str(),
            "claude-acct-a24a7f55f278"
        );
        assert_eq!(
            ring_id_for_identity(&paths, "uuid:u-1/org-personal").as_str(),
            "claude-acct-8c77a6bffa5a"
        );
        assert_eq!(
            ring_id_for_identity(&paths, "email:A@B.co").as_str(),
            "claude-acct-80305c9bb1bb"
        );
        // A `dir:` identity keeps its folder's own ring id.
        let work = dir(&paths, ".claude-work");
        assert_eq!(
            ring_id_for_identity(&paths, &format!("dir:{work}")).as_str(),
            "claude-work"
        );
        let id = ring_id_for_account_key("u-1");
        assert!(id.as_str().starts_with("claude-acct-"));
        assert_eq!(id.as_str().len(), "claude-acct-".len() + 12);
    }
}

// ---- colours, names, order ----

// AccountRegistryTests.colourIndexPicksTheLeastUsed
#[test]
fn colour_index_picks_the_least_used() {
    assert_eq!(next_color_index(&[]), 0);
    assert_eq!(next_color_index(&[0, 1, 3]), 2);
    let all: Vec<i64> = (0..8).collect();
    assert_eq!(next_color_index(&all), 0);
    let mut more = all.clone();
    more.extend([0, 1]);
    assert_eq!(next_color_index(&more), 2);
    assert_eq!(next_color_index(&[9]), 0); // 9 wraps to 1
    assert_eq!(next_color_index(&[0, 9]), 2);
}

// AccountRegistryTests.sanitizedNames, plus Windows' trailing-dot rule
// (a folder named `work.` becomes `work` there).
#[test]
fn sanitized_names() {
    assert_eq!(sanitized_account_name("Work").as_deref(), Some("work"));
    assert_eq!(
        sanitized_account_name("my team_2").as_deref(),
        Some("my-team_2")
    );
    assert_eq!(sanitized_account_name("../../etc").as_deref(), Some("etc"));
    assert_eq!(sanitized_account_name("émile").as_deref(), Some("mile"));
    assert_eq!(sanitized_account_name("   "), None);
    assert_eq!(sanitized_account_name("work.").as_deref(), Some("work"));
    assert_eq!(sanitized_account_name("work...").as_deref(), Some("work"));
    assert_eq!(sanitized_account_name("a.b").as_deref(), Some("a.b"));
    assert_eq!(sanitized_account_name("..."), None);
    assert_eq!(sanitized_account_name("é."), None);
}

// AccountRegistryTests.sortingPutsDefaultFirst
#[test]
fn sorting_puts_default_first() {
    for paths in both() {
        let d = |name: &str| dir(&paths, name);
        let mut zeta = folder(&paths, &d(".claude-zeta")).env(&d(".claude-zeta"));
        zeta.custom_label = Some("Zeta".to_owned());
        let mut alpha = folder(&paths, &d(".claude-alpha")).env(&d(".claude-alpha"));
        alpha.custom_label = Some("beta".to_owned());
        let default = folder(&paths, &d(".claude"));
        let ordered = sorted(vec![zeta, alpha, default], &paths);
        let dirs: Vec<&str> = ordered.iter().map(Folder::dir).collect();
        assert_eq!(
            dirs,
            vec![d(".claude"), d(".claude-alpha"), d(".claude-zeta")]
        );
    }
}
