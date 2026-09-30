//! One account per signed-in identity, however many folders it lives in,
//! over Claude Parallel Profiles' layout on a temporary home
//! (PP_IdentityTests, and the identity half of PPFix_ReviewTests). The pure
//! grouping rules (`identity_keys`) are in `accounts_rules.rs`.
//!
//! The Swift tests that ask the usage probe planner which folder to run in
//! belong to the usage package and are left to it. Every expected path is
//! built through `Home::path`, so the suite runs with either path style.

mod accounts_support;

use accounts_support::*;
use agentnotch_engine::accounts::identities::{
    identity_keys, ring_id_for_account_key, IdentityAccount,
};
use agentnotch_engine::accounts::timeline::FolderIdentityTimeline;
use agentnotch_engine::accounts::{AccountError, AccountRegistry, FolderRings};
use agentnotch_engine::model::FolderKind;
use serde_json::json;
use std::time::{Duration, UNIX_EPOCH};

fn find<'a>(registry: &'a AccountRegistry, email: &str) -> Option<&'a IdentityAccount> {
    registry
        .identities()
        .iter()
        .find(|identity| identity.email.as_deref() == Some(email))
}

fn run_dirs(identity: Option<&IdentityAccount>) -> Vec<String> {
    identity
        .map(|i| i.run_dirs.iter().map(|f| f.dir().to_owned()).collect())
        .unwrap_or_default()
}

fn store_dirs(identity: Option<&IdentityAccount>) -> Vec<String> {
    identity
        .map(|i| i.store_dirs.iter().map(|f| f.dir().to_owned()).collect())
        .unwrap_or_default()
}

fn labels(registry: &AccountRegistry, home: &Home) -> Vec<String> {
    registry
        .identities()
        .iter()
        .map(|i| i.label(&home.paths))
        .collect()
}

fn emails(registry: &AccountRegistry) -> Vec<Option<String>> {
    registry
        .identities()
        .iter()
        .map(|i| i.email.clone())
        .collect()
}

fn parasid() -> String {
    format!("uuid:{PARAS_UUID}")
}

fn biiosid() -> String {
    format!("uuid:{BIIOS_UUID}")
}

// PP_IdentityTests.theUsersMacHasExactlyTwoAccounts
#[test]
fn the_users_mac_has_exactly_two_accounts() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let registry = home.registry();

    assert_eq!(registry.identities().len(), 2);
    let paras = find(&registry, PARAS);
    let biios = find(&registry, BIIOS);
    assert_eq!(
        run_dirs(paras),
        home.paths_of(&[
            ".claude",
            ".claude-windows/801f9dd51396",
            ".claude-windows/b9fbb9ecd7cb"
        ])
    );
    assert_eq!(
        store_dirs(paras),
        home.paths_of(&[".claude-paras", ".claude-paras-rivant-in"])
    );
    assert_eq!(
        run_dirs(biios),
        home.paths_of(&[".claude-windows/1bf3e8f92b11"])
    );
    assert_eq!(store_dirs(biios), home.paths_of(&[".claude-claude"]));
    // The naming rule, no folder suffix: the names don't collide.
    let paras = paras.unwrap();
    let biios = biios.unwrap();
    assert_eq!(paras.label(&home.paths), "Claude Rivant");
    assert_eq!(biios.label(&home.paths), "Claude Biios");
    // By name, whichever one ~/.claude runs as.
    assert_eq!(labels(&registry, &home), ["Claude Biios", "Claude Rivant"]);
    assert!(paras.includes_default && !biios.includes_default);
    assert_eq!(paras.window_dir_ids.len(), 2);
    assert_eq!(biios.window_dir_ids.len(), 1);
    // Ring ids: from the account's UUID alone, the Claude family.
    assert_eq!(paras.ring_id, ring_id_for_account_key(PARAS_UUID));
    assert!(paras.ring_id.as_str().starts_with("claude-acct-"));
    assert_eq!(paras.ring_id.as_str().len(), "claude-acct-".len() + 12);
    assert!(agentnotch_engine::accounts::identities::is_claude_ring(
        paras.ring_id.as_str()
    ));
    // No phantom account for the shared history, nothing unsigned.
    assert!(registry.unsigned_folders().is_empty());
    assert!(registry
        .known_folders()
        .iter()
        .all(|f| f.dir() != home.path(".claude-shared")));
    assert_eq!(
        registry.infrastructure_dirs(),
        home.paths_of(&[".claude-shared", ".claude-windows"])
    );
    assert!(registry.layout().extension_detected);
    // Stores are folders of the account, but never run folders.
    let mut stores: Vec<String> = registry
        .known_folders()
        .iter()
        .filter(|f| f.kind == FolderKind::Store)
        .map(|f| f.dir().to_owned())
        .collect();
    stores.sort();
    let mut expected =
        home.paths_of(&[".claude-claude", ".claude-paras", ".claude-paras-rivant-in"]);
    expected.sort();
    assert_eq!(stores, expected);
    let of_window = registry
        .identity_of_folder(&home.path(".claude-windows/1bf3e8f92b11"))
        .unwrap();
    assert_eq!(of_window.email.as_deref(), Some(BIIOS));
    assert_eq!(
        registry
            .identity_id_for(&home.path(".claude-paras"))
            .map(|id| id.as_str().to_owned()),
        Some(parasid())
    );
}

// PP_IdentityTests.aWindowThatSwitchedAccountMovesToTheOtherAccount
#[test]
fn a_window_that_switched_account_moves_to_the_other_account() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let mut registry = home.registry();
    home.write_json(
        ".claude-windows/b9fbb9ecd7cb/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, None),
    );
    home.reader.invalidate();
    home.discover(&mut registry);
    let biios = find(&registry, BIIOS);
    assert_eq!(
        run_dirs(biios),
        home.paths_of(&[
            ".claude-windows/1bf3e8f92b11",
            ".claude-windows/b9fbb9ecd7cb"
        ])
    );
    assert_eq!(registry.identities().len(), 2);
}

// PP_IdentityTests.aDefaultMirroredToTheOtherAccountJoinsIt
/// The extension mirrors the last-used account into ~/.claude.
#[test]
fn a_default_mirrored_to_the_other_account_joins_it() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let mut registry = home.registry();
    home.write_json(".claude.json", &home.login(BIIOS_UUID, BIIOS, None));
    home.reader.invalidate();
    home.discover(&mut registry);
    let paras = find(&registry, PARAS);
    let biios = find(&registry, BIIOS);
    assert_eq!(
        run_dirs(biios),
        home.paths_of(&[".claude", ".claude-windows/1bf3e8f92b11"])
    );
    assert_eq!(
        run_dirs(paras),
        home.paths_of(&[
            ".claude-windows/801f9dd51396",
            ".claude-windows/b9fbb9ecd7cb"
        ])
    );
    // The rings keep their places.
    assert_eq!(
        emails(&registry),
        [Some(BIIOS.to_owned()), Some(PARAS.to_owned())]
    );
}

// PP_IdentityTests.aHalfMirroredDefaultFollowsItsEmail
/// The mirror rewrites the email and keeps the rest of `oauthAccount`: the
/// UUID left behind belongs to the other person, and the email wins.
#[test]
fn a_half_mirrored_default_follows_its_email() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let mut registry = home.registry();
    home.write_json(".claude.json", &home.login(PARAS_UUID, BIIOS, None));
    home.reader.invalidate();
    home.discover(&mut registry);
    assert_eq!(registry.identities().len(), 2);
    let biios = find(&registry, BIIOS).unwrap();
    assert_eq!(
        biios.run_dirs.first().map(|f| f.dir().to_owned()),
        Some(home.path(".claude"))
    );
    assert_eq!(biios.account_uuid.as_deref(), Some(BIIOS_UUID));

    let folders: Vec<_> = registry.known_folders().to_vec();
    let keys = identity_keys(&folders, None);
    let key_of = |relative: &str| keys.get(home.path(relative).as_str());
    let default = key_of(".claude").expect("the default has a key");
    assert_eq!(
        (default.key.as_str(), default.corrected),
        (biiosid().as_str(), true)
    );
    let store = key_of(".claude-paras").expect("the store has a key");
    assert_eq!(
        (store.key.as_str(), store.corrected),
        (parasid().as_str(), false)
    );
}

// PP_IdentityTests.anAccountOnlyItsStoreHoldsStillHasARingButRunsNowhere
#[test]
fn an_account_only_its_store_holds_still_has_a_ring_but_runs_nowhere() {
    let home = Home::new();
    home.build_user_layout(true, true);
    home.remove(".claude-windows/1bf3e8f92b11");
    let registry = home.registry();
    let biios = find(&registry, BIIOS).expect("the account still has a ring");
    assert!(biios.run_dirs.is_empty());
    assert_eq!(store_dirs(Some(biios)), home.paths_of(&[".claude-claude"]));
    // Nothing runs it, so the usage probe has no folder to run in: it is
    // told by `probe_folder` over its run folders, which are empty.
    assert!(biios
        .primary_dir()
        .is_some_and(|f| f.kind == FolderKind::Store));
}

// PP_IdentityTests.aNewWindowJoinsItsAccountAndAClosedOneLeaves
#[test]
fn a_new_window_joins_its_account_and_a_closed_one_leaves() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let mut registry = home.registry();
    home.add_window("0a1b2c3d4e5f", BIIOS_UUID, BIIOS, None);
    home.discover(&mut registry);
    assert!(run_dirs(find(&registry, BIIOS)).contains(&home.path(".claude-windows/0a1b2c3d4e5f")));
    // The extension deleted it (uninstalled, say): it goes by itself.
    home.remove(".claude-windows/0a1b2c3d4e5f");
    home.discover(&mut registry);
    assert!(registry
        .folder(&home.path(".claude-windows/0a1b2c3d4e5f"))
        .is_none());
}

// PP_IdentityTests.unsignedFolders
/// Only a folder added by hand keeps "Run ... then /login"; others that
/// nobody signed in to are listed, with no ring.
#[test]
fn unsigned_folders() {
    let home = Home::new();
    home.build_user_layout(true, true);
    home.mkdir(".claude-fresh/projects");
    home.write(".claude-windows/0a1b2c3d4e5f/.claude.json", "{}");
    let mut registry = home.registry();
    let (added, _) = registry
        .add_folder(&home.path(".claude-fresh"))
        .expect("a Claude folder");
    assert_eq!(registry.identities().len(), 3);
    let manual = registry
        .identities()
        .iter()
        .find(|i| i.id.as_str() == format!("dir:{}", added.as_str()))
        .expect("the folder added by hand has a ring");
    assert!(manual.is_standalone_unsigned());
    assert_eq!(manual.ring_id.as_str(), "claude-fresh");
    let unsigned: Vec<String> = registry
        .unsigned_folders()
        .iter()
        .map(|f| f.dir().to_owned())
        .collect();
    assert_eq!(unsigned, home.paths_of(&[".claude-windows/0a1b2c3d4e5f"]));
}

// PP_IdentityTests.aLoneUnsignedDefaultKeepsItsRing
/// With nobody signed in anywhere, `~/.claude` still has its ring.
#[test]
fn a_lone_unsigned_default_keeps_its_ring() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    let registry = home.registry();
    let rings: Vec<&str> = registry
        .identities()
        .iter()
        .map(|i| i.ring_id.as_str())
        .collect();
    assert_eq!(rings, ["claude"]);
    assert!(!registry.identities()[0].is_signed_in());
}

// PP_IdentityTests.plainFoldersWithOneLoginAreOneAccount
/// Without the extension, two folders with one login are one account.
#[test]
fn plain_folders_with_one_login_are_one_account() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.write_json(".claude.json", &home.login("u-1", "me@x.dev", None));
    home.mkdir(".claude-personal/projects");
    home.write_json(
        ".claude-personal/.claude.json",
        &home.login("u-1", "me@x.dev", None),
    );
    home.mkdir(".claude-work/projects");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login("u-2", "me@work.dev", None),
    );
    let registry = home.registry();
    assert_eq!(registry.identities().len(), 2);
    assert_eq!(
        run_dirs(find(&registry, "me@x.dev")),
        home.paths_of(&[".claude", ".claude-personal"])
    );
    assert!(registry
        .identities()
        .iter()
        .all(|i| i.store_dirs.is_empty()));
    assert!(!registry.layout().extension_detected);
}

// PP_IdentityTests.perFolderChoicesBecomePerIdentityAndPersist
/// accounts.json from before (a row per folder): name, colour and tracking go
/// to the identity from its first folder, then persist.
#[test]
fn per_folder_choices_become_per_identity_and_persist() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let folder_row = |relative: &str, extra: serde_json::Value| {
        let mut row = json!({
            "id": home.path(relative), "configDir": home.path(relative),
            "source": "discovered",
        });
        for (key, value) in extra.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    };
    let old = json!({"version": 1, "removedIds": [], "accounts": [
        folder_row(".claude", json!({"customLabel": "Paras", "colorIndex": 3, "isHidden": false})),
        folder_row(".claude-shared", json!({"colorIndex": 1, "isHidden": false})),
        folder_row(".claude-claude", json!({"configDirEnv": home.path(".claude-claude"), "colorIndex": 3, "isHidden": true})),
        folder_row(".claude-paras", json!({"configDirEnv": home.path(".claude-paras"), "customLabel": "Store name", "colorIndex": 5, "isHidden": true})),
    ]});
    std::fs::create_dir_all(&home.roots.support).unwrap();
    std::fs::write(home.accounts_file(), serde_json::to_vec(&old).unwrap()).unwrap();
    let mut registry = home.registry();
    let paras = find(&registry, PARAS).unwrap();
    let biios = find(&registry, BIIOS).unwrap();
    assert_eq!(paras.label(&home.paths), "Paras"); // ~/.claude's, the first folder
    assert_eq!(paras.color_index, 3);
    assert!(!paras.is_hidden);
    assert!(biios.is_hidden); // ~/.claude-claude's choice
    assert_ne!(biios.color_index, 3); // the colour was taken
                                      // Every folder follows its identity's tracking, windows included.
    let folders_of = |identity: &IdentityAccount| -> Vec<bool> {
        identity
            .folder_ids()
            .iter()
            .map(|id| registry.folder(id.as_str()).unwrap().is_hidden)
            .collect()
    };
    let biios_hidden = folders_of(biios);
    let paras_hidden = folders_of(paras);
    assert_eq!(biios_hidden.len(), 2);
    assert!(biios_hidden.iter().all(|h| *h));
    assert_eq!(paras_hidden.len(), 5);
    assert!(paras_hidden.iter().all(|h| !*h));
    // The shared history is no account, whatever the file said.
    assert!(registry.folder(&home.path(".claude-shared")).is_none());

    home.save(&mut registry);
    let saved = std::fs::read_to_string(home.accounts_file()).unwrap();
    assert!(saved.contains("\"version\" : 2"), "{saved}");
    assert!(saved.contains(&parasid()));
    let again = home.registry();
    assert_eq!(find(&again, PARAS).unwrap().label(&home.paths), "Paras");
    assert!(find(&again, BIIOS).unwrap().is_hidden);
}

// PP_IdentityTests.trackingAndForgettingApplyToTheWholeIdentity
#[test]
fn tracking_and_forgetting_apply_to_the_whole_identity() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let mut registry = home.registry();
    registry.set_hidden(&biiosid(), true);
    // A window opened later for it is untracked too.
    home.add_window("0a1b2c3d4e5f", BIIOS_UUID, BIIOS, None);
    home.discover(&mut registry);
    assert!(
        registry
            .folder(&home.path(".claude-windows/0a1b2c3d4e5f"))
            .expect("the new window is known")
            .is_hidden
    );
    // A folder of it: the identity.
    registry.set_hidden(&home.path(".claude-windows/1bf3e8f92b11"), false);
    assert!(!registry.identity(&biiosid()).unwrap().is_hidden);

    registry.remove(&biiosid());
    assert!(registry.identity(&biiosid()).is_none());
    assert_eq!(registry.identities().len(), 1);
    assert!(registry.is_forgotten(&home.path(".claude-windows/1bf3e8f92b11")));
    assert!(registry.is_forgotten(&biiosid()));
    // Its store stays on disk, untouched.
    assert!(home.exists(".claude-claude/.claude.json"));
    // Adding one of its folders back brings the account back.
    registry
        .add_folder(&home.path(".claude-windows/1bf3e8f92b11"))
        .expect("a forgotten folder can be added again");
    assert!(registry.identity(&biiosid()).is_some());
}

// PP_IdentityTests.theSharedHistoryCannotBeAddedByHand
#[test]
fn the_shared_history_cannot_be_added_by_hand() {
    let home = Home::new();
    home.build_user_layout(true, true);
    let mut registry = home.registry();
    for relative in [".claude-shared", ".claude-windows"] {
        assert_eq!(
            registry.add_folder(&home.path(relative)).unwrap_err(),
            AccountError::Infrastructure
        );
    }
    assert_eq!(registry.identities().len(), 2);
}

// PPFix_ReviewTests.anUntrackedAccountsSessionIsToldApartByWhenItStarted
#[test]
fn an_untracked_accounts_session_is_told_apart_by_when_it_started() {
    for paths in [mac(), win()] {
        let default = paths.key(&paths.join(paths.home(), ".claude"));
        let window = paths.key(&paths.join(paths.home(), ".claude-windows/w"));
        let other = paths.join(paths.home(), ".claude-other");
        let t0 = UNIX_EPOCH + Duration::from_secs(2_000_000);
        let plus = |seconds: u64| t0 + Duration::from_secs(seconds);
        let mut timeline = FolderIdentityTimeline::default();
        timeline.observe(Some("uuid:p"), Some("p"), Some(t0), plus(10), false);
        timeline.observe(Some("uuid:b"), Some("p"), Some(plus(20)), plus(30), false);
        let mut rings = FolderRings {
            default_folder: Some(default.clone()),
            mirrors_default: true,
            default_timeline: timeline,
            ..FolderRings::default()
        };
        rings
            .identity_of_folder
            .insert(default.clone(), "uuid:b".to_owned());
        rings
            .identity_of_folder
            .insert(window.clone(), "uuid:b".to_owned());
        rings.untracked_identities.insert("uuid:b".to_owned());
        rings.untracked_folders.insert(default.clone());
        rings.untracked_folders.insert(window.clone());
        // Started as the tracked account: not held back.
        assert!(!rings.is_untracked(&paths, &default, Some(plus(5))));
        // Started as the untracked one, or can't tell: its prompts go to the
        // terminal.
        assert!(rings.is_untracked(&paths, &default, Some(plus(25))));
        assert!(rings.is_untracked(&paths, &default, None));
        assert!(rings.is_untracked(&paths, &window, None));
        assert!(!rings.is_untracked(&paths, &other, None));
    }
}

// PPFix_ReviewTests.anExtensionAccountCanBeForgottenWhoeverHoldsTheDefault
#[test]
fn an_extension_account_can_be_forgotten_whoever_holds_the_default() {
    let home = Home::new();
    home.build_user_layout(true, true);
    home.mirror_into_default(PARAS_UUID, BIIOS);
    let registry = home.registry();
    let biios = registry.identity(&biiosid()).expect("the mirrored account");
    assert!(biios.includes_default && biios.can_be_forgotten());
    let paras = registry.identity(&parasid()).expect("the other account");
    assert!(paras.can_be_forgotten());
    // Without the extension, the account ~/.claude runs as stays.
    let plain_home = Home::new();
    plain_home.mkdir(".claude/projects");
    plain_home.write_json(".claude.json", &plain_home.login("u-1", "me@x.dev", None));
    let plain = plain_home.registry();
    let only = &plain.identities()[0];
    assert!(only.includes_default && !only.can_be_forgotten());
}

// PPFix_ReviewTests.savedFolderChoicesGoToTheDefaultsOwner
#[test]
fn saved_folder_choices_go_to_the_defaults_owner() {
    let home = Home::new();
    home.build_user_layout(true, true);
    home.mirror_into_default(PARAS_UUID, BIIOS);
    let saved = json!({"version": 1, "removedIds": [], "accounts": [
        {"id": home.path(".claude"), "configDir": home.path(".claude"), "customLabel": "Work (Rivant)",
         "colorIndex": 3, "isHidden": false, "source": "discovered"},
        {"id": home.path(".claude-claude"), "configDir": home.path(".claude-claude"),
         "configDirEnv": home.path(".claude-claude"), "colorIndex": 5, "isHidden": true, "source": "discovered"},
    ]});
    std::fs::create_dir_all(&home.roots.support).unwrap();
    std::fs::write(home.accounts_file(), serde_json::to_vec(&saved).unwrap()).unwrap();
    let registry = home.registry();
    let paras = registry.identity(&parasid()).unwrap();
    let biios = registry.identity(&biiosid()).unwrap();
    assert_eq!(paras.custom_label.as_deref(), Some("Work (Rivant)"));
    assert_eq!(paras.color_index, 3);
    assert!(!paras.is_hidden);
    assert_eq!(biios.custom_label, None);
    assert!(biios.is_hidden);
    // Who a mirrored default's saved choices belong to.
    assert_eq!(
        registry
            .owner_of_saved_folder(&home.path(".claude"))
            .map(|id| id.as_str().to_owned()),
        Some(parasid())
    );
}
