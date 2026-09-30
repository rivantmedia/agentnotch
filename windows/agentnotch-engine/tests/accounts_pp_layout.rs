//! Claude Parallel Profiles' layout on a temporary home: which folders
//! Claude Code runs in, which are account stores, which are only
//! infrastructure (PP_LayoutTests' PP_ClassificationTests and
//! PP_WindowWatchTests, and the classifier half of PPFix_ReviewTests), and
//! how a new VS Code window is noticed. The scanner-level suites at the top
//! of the Swift file live in `core_json_scan.rs` and `core_claude_json.rs`.
//!
//! Links are symbolic links on Unix and junctions on Windows (through
//! `Home::link_dir`); no test looks at a raw link target, and every
//! expected path is built through `Home::path`.

mod accounts_support;

use accounts_support::*;
use agentnotch_engine::accounts::classify::{
    classify, discover, is_window_dir, parse_manifest, SuggestionReason, STORE_MARKER_NAME,
};
use agentnotch_engine::accounts::watch::{change, fingerprint, Change};
use agentnotch_engine::model::FolderKind;

fn sorted(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths
}

// PP_ClassificationTests.theUsersLayout
#[test]
fn the_users_layout() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let snapshot = home.read(&[]);
    let layout = classify(&snapshot, &[], &home.paths);
    assert!(layout.extension_detected);
    assert_eq!(
        layout.run_dirs(),
        sorted(home.paths_of(&[
            ".claude",
            ".claude-windows/1bf3e8f92b11",
            ".claude-windows/801f9dd51396",
            ".claude-windows/b9fbb9ecd7cb",
        ]))
    );
    assert_eq!(
        layout.stores(),
        sorted(home.paths_of(&[".claude-claude", ".claude-paras", ".claude-paras-rivant-in"]))
    );
    assert_eq!(
        layout.infrastructure(),
        sorted(home.paths_of(&[".claude-shared", ".claude-windows"]))
    );
    assert_eq!(layout.window_dirs(&home.paths).len(), 3);
    assert!(layout.adopted().is_empty());

    // Links are read as links, and the default's login is ~/.claude.json.
    let default_folder = snapshot
        .folder(&home.path(".claude"))
        .expect("the default folder is in the snapshot");
    assert!(default_folder.is_signed_in);
    let shared_projects = home.paths.normalize(&home.path(".claude-shared/projects"));
    assert!(
        default_folder
            .link_targets
            .iter()
            .any(|target| home.paths.same(target, &shared_projects)),
        "link targets: {:?}",
        default_folder.link_targets
    );
    // The shared history's own folder is not linked to anything.
    let shared = snapshot
        .folder(&home.path(".claude-shared"))
        .expect("shared");
    assert!(shared.link_targets.is_empty());
}

// PP_ClassificationTests.discoveryAddsDefaultStandaloneWindowsAndStoresNeverTheSharedHistory
#[test]
fn discovery_adds_default_standalone_windows_and_stores_never_the_shared_history() {
    let home = Home::new();
    home.build_user_layout(true, true);
    // A window that has not been stocked yet (no .claude.json) is not an account.
    home.link_shared(".claude-windows/0000aaaa1111");
    let snapshot = home.read(&[]);
    let found = discover(&snapshot, &[], &[], &home.paths);
    assert_eq!(
        found.accounts,
        home.paths_of(&[
            ".claude",
            ".claude-windows/1bf3e8f92b11",
            ".claude-windows/801f9dd51396",
            ".claude-windows/b9fbb9ecd7cb",
            ".claude-claude",
            ".claude-paras",
            ".claude-paras-rivant-in",
        ])
    );
    assert!(found.suggestions.is_empty());
    // The unstocked window is still a run folder, just not an account yet.
    assert_eq!(
        found
            .layout
            .kind(&home.paths, &home.path(".claude-windows/0000aaaa1111")),
        Some(FolderKind::Run)
    );

    // A standalone folder of the user's own is added when it is signed in,
    // and offered when it only has Claude Code's folders.
    home.mkdir(".claude-work/projects");
    home.sign_in(".claude-work/.claude.json", "me@work.dev", "u-work", None);
    home.mkdir(".claude-idle/projects");
    let found = discover(&home.read(&[]), &[], &[], &home.paths);
    assert!(found.accounts.contains(&home.path(".claude-work")));
    assert!(!found.accounts.contains(&home.path(".claude-idle")));
    assert_eq!(found.suggestions.len(), 1);
    assert_eq!(found.suggestions[0].config_dir, home.path(".claude-idle"));
    assert_eq!(found.suggestions[0].reason, SuggestionReason::Found);
    for never in [".claude-shared", ".claude-windows"] {
        assert!(!found.accounts.contains(&home.path(never)));
        assert!(found
            .suggestions
            .iter()
            .all(|s| s.config_dir != home.path(never)));
    }
    // The standalone folders come before the windows and the stores.
    let position = |relative: &str| {
        found
            .accounts
            .iter()
            .position(|dir| *dir == home.path(relative))
            .expect("in the list")
    };
    assert_eq!(position(".claude"), 0);
    assert!(position(".claude-work") < position(".claude-windows/1bf3e8f92b11"));
    assert!(position(".claude-windows/b9fbb9ecd7cb") < position(".claude-claude"));
}

// PP_ClassificationTests.storesAreFoundByMarkerOrManifestAlone
#[test]
fn stores_are_found_by_marker_or_manifest_alone() {
    let home = Home::new();
    home.link_shared(".claude");
    // A marker, not in the manifest.
    home.add_store("a", "u-a", "a@x.dev", None, true);
    // In the manifest only (`created`, no marker).
    home.add_store("b", "u-b", "b@x.dev", None, false);
    home.write_manifest(&[".claude-b"], &[".claude-b"]);
    let layout = classify(&home.read(&[]), &[], &home.paths);
    assert_eq!(
        layout.kind(&home.paths, &home.path(".claude-a")),
        Some(FolderKind::Store)
    );
    assert_eq!(
        layout.kind(&home.paths, &home.path(".claude-b")),
        Some(FolderKind::Store)
    );
    assert!(layout.extension_detected);
}

// PP_ClassificationTests.withoutTheExtensionEveryFolderRuns
/// Without the extension everything works as before: every folder is a run
/// folder (grouped by identity later). This is also the native Windows case.
#[test]
fn without_the_extension_every_folder_runs() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.write_json(".claude.json", &home.login("u-1", "me@x.dev", None));
    home.mkdir(".claude-work/projects");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login("u-2", "me@work.dev", None),
    );
    let layout = classify(&home.read(&[]), &[], &home.paths);
    assert!(!layout.extension_detected);
    assert_eq!(
        layout.run_dirs(),
        sorted(home.paths_of(&[".claude", ".claude-work"]))
    );
    assert!(layout.stores().is_empty() && layout.infrastructure().is_empty());
}

// PP_ClassificationTests.aLinkTargetIsInfrastructureUnlessItRunsItself
/// A user who links a profile's history into `~/.claude` by hand still runs
/// Claude Code in `~/.claude`; an unsigned link target is history.
#[test]
fn a_link_target_is_infrastructure_unless_it_runs_itself() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.mkdir(".claude/sessions");
    home.mkdir(".claude-work");
    home.link_dir(".claude-work/projects", ".claude/projects");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login("u-2", "me@work.dev", None),
    );
    home.mkdir(".claude-history/projects");
    home.mkdir(".claude-other");
    // A relative link, resolved against the folder that holds it.
    home.mkdir(".claude-history/sessions");
    home.link_dir(
        ".claude-other/sessions",
        "../.claude-history/projects/../sessions",
    );
    let layout = classify(&home.read(&[]), &[], &home.paths);
    let kind = |relative: &str| layout.kind(&home.paths, &home.path(relative));
    assert_eq!(kind(".claude"), Some(FolderKind::Run));
    assert_eq!(kind(".claude-work"), Some(FolderKind::Run));
    assert_eq!(kind(".claude-history"), Some(FolderKind::Infrastructure));
}

// PP_ClassificationTests.windowFoldersAreRecognised
#[test]
fn window_folders_are_recognised() {
    let paths = mac();
    let home = "/Users/me";
    assert!(is_window_dir(
        &paths,
        &format!("{home}/.claude-windows/801f9dd51396")
    ));
    assert!(is_window_dir(
        &paths,
        &format!("{home}/.claude-windows/801f9dd51396/")
    ));
    assert!(!is_window_dir(&paths, &format!("{home}/.claude-windows")));
    assert!(!is_window_dir(
        &paths,
        &format!("{home}/.claude-windows/.manifest.json")
    ));
    assert!(!is_window_dir(
        &paths,
        &format!("{home}/.claude-windows/a/b")
    ));
    let manifest = parse_manifest(
        br#"{"stores": ["/Users/me/.claude-a/"], "created": []}"#,
        &paths,
    )
    .expect("a manifest");
    assert_eq!(manifest.stores, ["/Users/me/.claude-a"]);
    assert!(parse_manifest(b"[]", &paths).is_none());

    let paths = win();
    assert!(is_window_dir(
        &paths,
        r"C:\Users\me\.claude-windows\801f9dd51396"
    ));
    assert!(is_window_dir(
        &paths,
        r"c:/users/me/.claude-windows/801f9dd51396"
    ));
    assert!(!is_window_dir(&paths, r"C:\Users\me\.claude-windows"));
    assert!(!is_window_dir(&paths, r"C:\Users\me\.claude-windows\a\b"));
}

// ---- PPFix_ReviewTests, the classifier half ----

// PPFix_AdoptedProfileTests.anAdoptedProfileIsARunFolderAndAMadeStoreIsAStore
#[test]
fn an_adopted_profile_is_a_run_folder_and_a_made_store_is_a_store() {
    let home = Home::new();
    home.build_user_layout(false, true);
    home.mkdir(".claude-work/projects");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login("u-work", "me@work.dev", None),
    );
    home.write(
        ".claude-work/settings.json",
        "{\n  \"model\": \"opus\"\n}\n",
    );
    let mut listed: Vec<&str> = CREATED_STORES.to_vec();
    listed.push(".claude-work");
    home.write_manifest(&listed, &CREATED_STORES);

    let layout = classify(&home.read(&[]), &[], &home.paths);
    assert_eq!(
        layout.kind(&home.paths, &home.path(".claude-work")),
        Some(FolderKind::Run)
    );
    assert_eq!(layout.adopted(), home.paths_of(&[".claude-work"]));
    assert!(layout.is_adopted(&home.paths, &home.path(".claude-work")));
    assert!(!layout.is_adopted(&home.paths, &home.path(".claude-paras")));
    assert_eq!(layout.stores(), sorted(home.paths_of(&CREATED_STORES)));
    // Discovery adds it as an account of its own.
    let found = discover(&home.read(&[]), &[], &[], &home.paths);
    assert!(found.accounts.contains(&home.path(".claude-work")));

    // A made store without its marker is still a store (listed in `created`).
    home.remove(&format!(".claude-paras/{STORE_MARKER_NAME}"));
    let layout = classify(&home.read(&[]), &[], &home.paths);
    assert_eq!(
        layout.kind(&home.paths, &home.path(".claude-paras")),
        Some(FolderKind::Store)
    );
    // A listed window or the default is never "adopted".
    listed.push(".claude");
    listed.push(".claude-windows/801f9dd51396");
    home.write_manifest(&listed, &CREATED_STORES);
    let layout = classify(&home.read(&[]), &[], &home.paths);
    assert_eq!(layout.adopted(), home.paths_of(&[".claude-work"]));
}

// PPFix_AdoptedProfileTests.anUnreadableManifestKeepsTheStoresStores
#[test]
fn an_unreadable_manifest_keeps_the_stores_stores() {
    let home = Home::new();
    home.build_user_layout(false, false);
    // No marker and no manifest: nothing says they are stores yet, so make
    // the manifest, read once, then break it.
    home.write_manifest(&CREATED_STORES, &CREATED_STORES);
    let mut registry = home.registry();
    let kind = |registry: &agentnotch_engine::accounts::AccountRegistry, relative: &str| {
        registry.layout().kind(&home.paths, &home.path(relative))
    };
    assert_eq!(kind(&registry, ".claude-paras"), Some(FolderKind::Store));

    // The extension is caught mid-write.
    home.write(
        ".claude-windows/.manifest.json",
        &format!("{{\"stores\": [\"{}", home.path(".claude-paras")),
    );
    let snapshot = home.snapshot(&registry);
    assert!(snapshot.manifest_unreadable && snapshot.manifest.is_none());
    home.discover(&mut registry);
    assert_eq!(kind(&registry, ".claude-paras"), Some(FolderKind::Store));
    assert_eq!(kind(&registry, ".claude-claude"), Some(FolderKind::Store));
    assert_eq!(
        kind(&registry, ".claude-paras-rivant-in"),
        Some(FolderKind::Store)
    );
    assert!(registry.layout().extension_detected);
    // Without the last classification they would have been run folders.
    let fresh = classify(&snapshot, &[], &home.paths);
    assert_eq!(
        fresh.kind(&home.paths, &home.path(".claude-paras")),
        Some(FolderKind::Run)
    );
    // ~/.claude and the windows are never stores.
    assert_eq!(kind(&registry, ".claude"), Some(FolderKind::Run));
    assert_eq!(
        kind(&registry, ".claude-windows/801f9dd51396"),
        Some(FolderKind::Run)
    );

    // A readable manifest again: stores stay, by the manifest this time.
    home.write_manifest(&CREATED_STORES, &CREATED_STORES);
    home.discover(&mut registry);
    assert_eq!(kind(&registry, ".claude-paras"), Some(FolderKind::Store));
    // A manifest that no longer lists one lets it go.
    home.write_manifest(
        &[".claude-claude", ".claude-paras"],
        &[".claude-claude", ".claude-paras"],
    );
    home.discover(&mut registry);
    assert_eq!(
        kind(&registry, ".claude-paras-rivant-in"),
        Some(FolderKind::Run)
    );
}

// ---- PP_WindowWatchTests ----

// PP_WindowWatchTests.aNewWindowOrAnAccountSwitchIsNoticed
#[test]
fn a_new_window_or_an_account_switch_is_noticed() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let first = fingerprint(&home.paths);
    assert_eq!(first.windows.len(), 3);
    assert!(first.manifest.is_some());
    assert_eq!(change(None, &first), Change::None);
    assert_eq!(
        change(Some(&first), &fingerprint(&home.paths)),
        Change::None
    );

    // A workspace opened: its folder appears, then gets its config.
    home.link_shared(".claude-windows/0a1b2c3d4e5f");
    let appeared = fingerprint(&home.paths);
    assert_eq!(change(Some(&first), &appeared), Change::Folders);
    home.write_json(
        ".claude-windows/0a1b2c3d4e5f/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, None),
    );
    let stocked = fingerprint(&home.paths);
    assert_eq!(change(Some(&appeared), &stocked), Change::Folders);

    // The window switched account: only who is signed in changed (the file
    // is longer now, which is what its stamp compares).
    home.write_json(
        ".claude-windows/0a1b2c3d4e5f/.claude.json",
        &home.login(PARAS_UUID, PARAS, Some(1.0)),
    );
    let switched = fingerprint(&home.paths);
    assert_eq!(change(Some(&stocked), &switched), Change::Identities);

    // The extension mirrors another account into ~/.claude.
    home.mirror_into_default(BIIOS_UUID, "someone.else@rivant.in");
    let mirrored = fingerprint(&home.paths);
    assert_eq!(change(Some(&switched), &mirrored), Change::Identities);

    // A window went.
    home.remove(".claude-windows/0a1b2c3d4e5f");
    assert_eq!(
        change(Some(&mirrored), &fingerprint(&home.paths)),
        Change::Folders
    );
}

#[test]
fn a_changed_manifest_reads_the_folders_again() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let before = fingerprint(&home.paths);
    // Longer than before, so its stamp differs whatever the clock says.
    home.write_manifest(
        &[
            ".claude-claude",
            ".claude-paras",
            ".claude-paras-rivant-in",
            ".claude-work",
        ],
        &CREATED_STORES,
    );
    assert_eq!(
        change(Some(&before), &fingerprint(&home.paths)),
        Change::Folders
    );
}

#[test]
fn without_a_manifest_only_the_windows_are_watched() {
    let home = Home::new();
    home.build_user_layout(false, true);
    let first = fingerprint(&home.paths);
    assert!(first.manifest.is_none() && first.default_config.is_none());
    // ~/.claude.json is not watched while there is no manifest.
    home.mirror_into_default(BIIOS_UUID, "someone.else@rivant.in");
    assert_eq!(
        change(Some(&first), &fingerprint(&home.paths)),
        Change::None
    );
    // A window is.
    home.add_window("0a1b2c3d4e5f", BIIOS_UUID, BIIOS, None);
    assert_eq!(
        change(Some(&first), &fingerprint(&home.paths)),
        Change::Folders
    );

    // No windows folder at all: nothing to see.
    let empty = Home::new();
    let none = fingerprint(&empty.paths);
    assert!(none.windows.is_empty() && none.manifest.is_none());
    assert_eq!(
        change(Some(&none), &fingerprint(&empty.paths)),
        Change::None
    );
}
