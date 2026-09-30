//! Sightings, spellings, user actions and `accounts.json` against a
//! throwaway home folder: the second half of the Mac's AccountRegistryTests,
//! the registry half of Fix_ForgottenAccountTests and
//! Fix_AccountOwnLabelTests. Expected paths are built with `Home::path`, so
//! the same file runs on Windows CI with `C:\` paths. Windows-style spelling
//! vectors (`C:\` against `c:/`) are the pure suites'.

mod accounts_support;

use accounts_support::{after, Home};
use agentnotch_engine::accounts::classify::{FolderSuggestion, SuggestionReason};
use agentnotch_engine::accounts::{AccountError, AccountRegistry};
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{AccountId, FolderSource};
use agentnotch_engine::persist::accounts::AccountsFile;
use std::collections::BTreeSet;
use std::path::MAIN_SEPARATOR;

/// A registry over `.claude-work` with a live session, so discovery adds it.
fn live_session(home: &Home, folder: &str, pid: u32) {
    home.processes.add(pid, 1, "claude.exe", after(0));
    home.write(
        &format!("{folder}/sessions/{pid}.json"),
        &format!(r#"{{"pid":{pid}}}"#),
    );
}

/// The line that starts Claude Code in `dir`, in the build's own shell.
fn launch_line(paths: &Paths, dir: &str) -> String {
    match paths.style() {
        PathStyle::Posix => format!("CLAUDE_CONFIG_DIR='{dir}' claude"),
        PathStyle::Windows => format!("$env:CLAUDE_CONFIG_DIR='{dir}'; claude"),
    }
}

fn labels(registry: &AccountRegistry) -> Vec<String> {
    registry.accounts().into_iter().map(|a| a.label).collect()
}

// ---- Sightings ----

// AccountRegistryTests.sightingsAddAccountsAndKeepTheRawEnv
#[test]
fn sightings_add_accounts_and_keep_the_raw_env() {
    let home = Home::new();
    let mut registry = home.registry();
    // A trailing separator: a different spelling, kept verbatim.
    let raw = format!("{}{}", home.path(".claude-work"), MAIN_SEPARATOR);
    let work = home.id(".claude-work");

    registry.record(
        home.sighting(".claude-work", Some(&raw), after(0)),
        after(0),
    );
    let folder = registry.folder(&raw).expect("the sighting added it");
    assert_eq!(folder.source, FolderSource::Hook);
    assert_eq!(folder.config_dir_env.as_deref(), Some(raw.as_str()));
    assert_eq!(folder.last_seen_at, Some(after(0)));
    assert_eq!(folder.id, work);

    // Within a minute: last_seen_at doesn't churn, and nothing is reported.
    let changed = registry.record(
        home.sighting(".claude-work", Some(&raw), after(10)),
        after(10),
    );
    assert!(!changed.any());
    assert_eq!(
        registry.folder(work.as_str()).unwrap().last_seen_at,
        Some(after(0))
    );
    registry.record(
        home.sighting(".claude-work", Some(&raw), after(59)),
        after(59),
    );
    assert_eq!(
        registry.folder(work.as_str()).unwrap().last_seen_at,
        Some(after(0))
    );
    registry.record(
        home.sighting(".claude-work", Some(&raw), after(90)),
        after(90),
    );
    assert_eq!(
        registry.folder(work.as_str()).unwrap().last_seen_at,
        Some(after(90))
    );

    // A session without the variable on a custom folder keeps the known one.
    registry.record(home.sighting(".claude-work", None, after(200)), after(200));
    let folder = registry.folder(work.as_str()).unwrap();
    assert_eq!(folder.config_dir_env.as_deref(), Some(raw.as_str()));
    // One spelling, seen three times: one entry (no login conflict here).
    assert_eq!(folder.seen_config_dir_envs, vec![raw]);
    assert_eq!(folder.last_seen_at, Some(after(200)));
    assert!(registry.needs_save());
}

// A sighting for a folder nobody knows: a hook-sourced account, whose
// identity comes with the next read of the disk.
#[test]
fn a_sighting_is_read_by_the_next_discovery() {
    let home = Home::new();
    home.mkdir(".claude-lab");
    home.sign_in(".claude-lab/.claude.json", "me@lab.io", "u-lab", None);
    let mut registry = home.registry_with(&[]);
    // The folder is signed in but only a session named it: discovery found
    // it too (signed-in look-alike), so a second sighting must not duplicate.
    let count = registry.known_folders().len();
    registry.record(home.sighting(".claude-lab", None, after(5)), after(5));
    assert_eq!(registry.known_folders().len(), count);

    home.mkdir(".claude-side");
    home.sign_in(".claude-side/.claude.json", "me@side.io", "u-side", None);
    // Not discovered as an account by itself (no session), but a hook saw it.
    registry.record(home.sighting(".claude-side", None, after(6)), after(6));
    assert!(registry.needs_discovery());
    home.discover_at(&mut registry, after(7));
    assert!(!registry.needs_discovery());
    let folder = registry.folder(&home.path(".claude-side")).unwrap();
    assert_eq!(folder.source, FolderSource::Hook);
    assert_eq!(
        folder.identity.as_ref().and_then(|i| i.email.as_deref()),
        Some("me@side.io")
    );
}

// AccountRegistryTests.twoSpellingsOfOneFolderDoNotFlipFlop, ported to the
// collapse rule (brief rule 9): one entry per `Paths::key`, unset apart from
// set for the default folder (`~\.claude.json` against `~\.claude\.claude.json`),
// no login conflict, and the folder moves to the spelling that is signed in
// once, then stays.
#[test]
fn two_spellings_of_one_folder_do_not_flip_flop() {
    let home = Home::new();
    home.mkdir(".claude");
    // The default login's file.
    home.sign_in(".claude.json", "me@personal.dev", "u-personal", None);
    let mut registry = home.registry();
    let dir = home.path(".claude");
    let trailing = format!("{dir}{MAIN_SEPARATOR}");

    registry.record(home.sighting(".claude", Some(&dir), after(0)), after(0));
    assert_eq!(
        registry.folder(&dir).unwrap().config_dir_env.as_deref(),
        Some(dir.as_str())
    );
    registry.record(home.sighting(".claude", None, after(1)), after(1));
    registry.record(home.sighting(".claude", Some(&dir), after(2)), after(2));
    // Another spelling of "set to itself" names the same login file: nothing new.
    registry.record(
        home.sighting(".claude", Some(&trailing), after(3)),
        after(3),
    );
    let folder = registry.folder(&dir).unwrap();
    assert_eq!(
        folder.seen_config_dir_envs,
        vec![dir.clone(), String::new()]
    );
    // Sightings alone never flip it...
    assert_eq!(folder.config_dir_env.as_deref(), Some(dir.as_str()));

    // ...but the explicit spelling's `~\.claude\.claude.json` has no login and
    // the default one's does, so it moves there, once.
    home.discover_at(&mut registry, after(100));
    let settled = registry.folder(&dir).unwrap();
    assert_eq!(settled.config_dir_env, None);
    assert_eq!(
        settled.identity.as_ref().and_then(|i| i.email.as_deref()),
        Some("me@personal.dev")
    );
    assert!(settled.is_default(registry.paths()));
    assert_eq!(settled.seen_config_dir_envs.len(), 2);

    // Neither more sightings nor more reads move it again, and once settled
    // a read of the disk reports nothing.
    for n in 0..3 {
        registry.record(
            home.sighting(".claude", Some(&dir), after(200 + n)),
            after(200 + n),
        );
        registry.record(
            home.sighting(".claude", None, after(300 + n)),
            after(300 + n),
        );
        home.discover_at(&mut registry, after(400 + n));
        let folder = registry.folder(&dir).unwrap();
        assert_eq!(folder.config_dir_env, None);
        assert_eq!(folder.seen_config_dir_envs.len(), 2);
    }
    let settled = home.discover_at(&mut registry, after(1000));
    assert!(!settled.any(), "{settled:?}");
    assert_eq!(registry.known_folders().len(), 1);
    // One login, one account.
    let accounts = registry.accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].email.as_deref(), Some("me@personal.dev"));
    assert!(accounts[0].includes_default);
}

// Two spellings of a folder that isn't the default: one entry at any case
// or separator the build treats alike, one folder.
#[test]
fn spellings_of_a_custom_folder_are_one_entry() {
    let home = Home::new();
    let mut registry = home.registry();
    let dir = home.path(".claude-work");
    let tilde = "~/.claude-work".to_owned();
    registry.record(
        home.sighting(".claude-work", Some(&dir), after(0)),
        after(0),
    );
    registry.record(
        home.sighting(".claude-work", Some(&tilde), after(1)),
        after(1),
    );
    let folder = registry.folder(&dir).unwrap();
    // `~/…` names the same folder as its expansion.
    assert_eq!(
        folder.seen_config_dir_envs.len(),
        1,
        "{:?}",
        folder.seen_config_dir_envs
    );
    assert_eq!(folder.config_dir_env.as_deref(), Some(dir.as_str()));
    assert_eq!(registry.known_folders().len(), 1);
    let again = home.discover_at(&mut registry, after(60));
    assert!(again.new_run_folders.is_empty());
    assert_eq!(registry.known_folders().len(), 1);
}

// AccountRegistryTests.forgottenAccountsComeBackOnlyAsASuggestion
// A forgotten folder is not re-added by a session, only suggested (and a
// sighting for one that is not forgotten adds it).
#[test]
fn forgotten_accounts_come_back_only_as_a_suggestion() {
    let home = Home::new();
    home.mkdir(".claude-work/sessions");
    live_session(&home, ".claude-work", 4321);
    let mut registry = home.registry();
    let work = home.id(".claude-work");
    assert!(
        registry.folder(work.as_str()).is_some(),
        "discovery adds it"
    );
    registry.remove(work.as_str());
    assert!(registry.folder(work.as_str()).is_none());

    let env = work.as_str().to_owned();
    registry.record(
        home.sighting(".claude-work", Some(&env), after(0)),
        after(0),
    );
    assert!(registry.folder(work.as_str()).is_none());
    assert!(registry.accounts().is_empty());
    assert_eq!(
        registry.suggestions(),
        [FolderSuggestion {
            config_dir: work.as_str().to_owned(),
            reason: SuggestionReason::SeenAgain,
        }]
    );
    // Discovery doesn't bring it back either.
    home.discover_at(&mut registry, after(120));
    assert!(registry.folder(work.as_str()).is_none());

    let (added, _) = registry.add_folder(work.as_str()).unwrap();
    assert_eq!(added, work);
    assert!(registry.folder(work.as_str()).is_some());
    assert!(registry.suggestions().is_empty());
}

// New: a sighting for a folder that is forgotten reports nothing else and
// keeps the account forgotten; a second sighting doesn't repeat the suggestion.
#[test]
fn a_sighting_for_a_forgotten_folder_changes_only_the_suggestions() {
    let home = Home::new();
    home.mkdir(".claude-temp");
    let mut registry = home.registry();
    let (temp, _) = registry.add_folder(&home.path(".claude-temp")).unwrap();
    registry.remove(temp.as_str());
    assert!(registry.is_forgotten(temp.as_str()));

    let changed = registry.record(home.sighting(".claude-temp", None, after(0)), after(0));
    assert!(changed.new_run_folders.is_empty() && !changed.identities && !changed.rings);
    assert!(registry.is_forgotten(temp.as_str()));
    assert_eq!(registry.suggestions().len(), 1);
    registry.record(home.sighting(".claude-temp", None, after(500)), after(500));
    assert_eq!(registry.suggestions().len(), 1);
    assert!(registry.folder(temp.as_str()).is_none());
}

// New: the shared history and the windows folder are never accounts, however
// a transcript path resolved.
#[test]
fn a_sighting_for_an_infrastructure_folder_is_ignored() {
    let home = Home::new();
    home.mkdir(".claude");
    home.mkdir(".claude-shared");
    home.add_window("aaaaaaaaaaaa", "u-win", "me@win.io", None);
    let mut registry = home.registry();
    let before: Vec<AccountId> = registry
        .known_folders()
        .iter()
        .map(|f| f.id.clone())
        .collect();
    registry.mark_saved();

    for relative in [".claude-shared", ".claude-windows"] {
        let changed = registry.record(home.sighting(relative, None, after(0)), after(0));
        assert!(!changed.any(), "{relative}");
    }
    let now: Vec<AccountId> = registry
        .known_folders()
        .iter()
        .map(|f| f.id.clone())
        .collect();
    assert_eq!(now, before);
    assert!(registry.suggestions().is_empty());
    assert!(!registry.needs_save());
}

// New: home itself and a parent of it are never accounts.
#[test]
fn a_sighting_for_home_adds_nothing() {
    let home = Home::new();
    let mut registry = home.registry();
    let changed = registry.record(home.sighting("", None, after(0)), after(0));
    assert!(!changed.any());
    assert!(registry.known_folders().is_empty());
}

// ---- User actions and persistence ----

// AccountRegistryTests.renameHideRemoveAndPersist
#[test]
fn rename_hide_remove_and_persist() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.mkdir(".claude-work/projects");
    home.sign_in(".claude-work/.claude.json", "me@work.com", "u-work", None);

    let mut registry = home.registry();
    let work_dir = home.path(".claude-work");
    let work = registry.folder(&work_dir).expect("discovered").clone();

    registry.rename(&work_dir, Some("  Work  "));
    assert_eq!(
        registry.folder(&work_dir).unwrap().custom_label.as_deref(),
        Some("Work")
    );
    let identity = registry.identity_of_folder(&work_dir).unwrap().clone();
    assert_eq!(identity.custom_label.as_deref(), Some("Work"));
    assert_eq!(registry.accounts()[0].label, "Work");
    registry.set_hidden(&work_dir, true);
    assert!(registry.folder(&work_dir).unwrap().is_hidden);
    let visible: Vec<&str> = registry.visible_folders().iter().map(|f| f.dir()).collect();
    assert_eq!(visible, vec![home.path(".claude")]);
    assert!(registry.visible_identities().is_empty());
    home.save(&mut registry);

    // A new registry on the same file restores the user's choices.
    let mut reloaded = home.registry();
    let restored = reloaded.folder(&work_dir).expect("restored").clone();
    assert_eq!(restored.custom_label.as_deref(), Some("Work"));
    assert!(restored.is_hidden);
    assert_eq!(restored.color_index, work.color_index);
    assert_eq!(restored.config_dir_env, work.config_dir_env);
    assert_eq!(reloaded.accounts()[0].label, "Work");
    assert!(!reloaded.accounts()[0].is_tracked);

    // Removing forgets it; discovery doesn't bring it back; the folder stays.
    reloaded.rename(&work_dir, None);
    assert_eq!(reloaded.folder(&work_dir).unwrap().custom_label, None);
    reloaded.remove(&work_dir);
    home.discover(&mut reloaded);
    assert!(reloaded.folder(&work_dir).is_none());
    // Only the default folder's own (signed-out) account is left.
    assert_eq!(labels(&reloaded), ["Claude"]);
    assert!(home.exists(".claude-work"));
    home.save(&mut reloaded);

    let mut again = home.registry();
    assert!(again.folder(&work_dir).is_none());
    assert_eq!(labels(&again), ["Claude"]);
    assert!(again.is_forgotten(&work_dir));

    // Adding it by hand brings it back.
    let (added, _) = again.add_folder(&work_dir).unwrap();
    assert_eq!(
        again.folder(added.as_str()).unwrap().source,
        FolderSource::Manual
    );
    assert_eq!(again.accounts().len(), 1);
    assert!(!again.is_forgotten(&work_dir));
}

// Track off is an identity's choice: it survives a new folder of the same
// account and is written once, on the identity.
#[test]
fn tracking_and_the_ring_are_kept_per_identity() {
    let home = Home::new();
    home.mkdir(".claude-work/projects");
    home.sign_in(".claude-work/.claude.json", "me@work.com", "u-work", None);
    let mut registry = home.registry();
    let ring = registry.accounts()[0].ring_id.clone();
    let key = registry.identities()[0].id.as_str().to_owned();

    registry.set_ring_shown(ring.as_str(), false);
    assert!(!registry.accounts()[0].ring_shown);
    assert!(registry.accounts()[0].is_tracked);
    home.save(&mut registry);
    let text = String::from_utf8(registry.file_bytes().unwrap()).unwrap();
    assert!(text.contains("\"ringHidden\" : true"), "{text}");

    let reloaded = home.registry();
    let account = &reloaded.accounts()[0];
    assert!(!account.ring_shown);
    assert!(account.is_tracked);
    assert_eq!(reloaded.identities()[0].id.as_str(), key);
}

// AccountRegistryTests.createAccountMakesTheFolder
#[test]
fn create_account_makes_the_folder() {
    let home = Home::new();
    let mut registry = home.registry();
    let created = registry.create_account("Side Project").unwrap();
    let expected = home.path(".claude-side-project");
    assert_eq!(created.folder.as_str(), expected);
    let folder = registry.folder(&expected).unwrap();
    assert_eq!(folder.config_dir_env.as_deref(), Some(expected.as_str()));
    assert_eq!(folder.custom_label.as_deref(), Some("Side Project"));
    assert_eq!(created.launch_command, launch_line(&home.paths, &expected));
    assert!(std::path::Path::new(&expected).is_dir());
    assert!(created.changed.any());
    assert_eq!(registry.accounts().len(), 1);
    assert_eq!(registry.accounts()[0].label, "Side Project");

    for bad in ["  /// ", "", "   "] {
        assert_eq!(
            registry.create_account(bad).unwrap_err(),
            AccountError::InvalidName,
            "{bad:?}"
        );
    }
}

// AccountRegistryTests.createAccountRefusesAForeignFolder: "New account..."
// never adopts another tool's folder that happens to have the same name; a
// real config folder is adopted.
#[test]
fn create_account_refuses_a_foreign_folder() {
    let home = Home::new();
    home.mkdir(".claude-server-commander");
    home.write(".claude-server-commander/config.json", "{}");
    home.mkdir(".claude-existing/projects");
    let mut registry = home.registry();
    let shown = home
        .paths
        .abbreviate(&home.path(".claude-server-commander"));
    assert_eq!(
        registry.create_account("Server Commander").unwrap_err(),
        AccountError::FolderExistsNotClaude(shown)
    );
    assert!(registry.accounts().is_empty());
    assert!(registry.known_folders().is_empty());
    let adopted = registry.create_account("existing").unwrap();
    assert_eq!(adopted.folder.as_str(), home.path(".claude-existing"));
}

// A file where the folder should go, and a registry with no way to look.
#[test]
fn create_account_needs_a_place_and_a_probe() {
    let home = Home::new();
    home.write(".claude-taken", "not a folder");
    let mut registry = home.registry();
    assert!(matches!(
        registry.create_account("taken"),
        Err(AccountError::CreateFailed(_))
    ));
    let mut bare = AccountRegistry::new(home.paths.clone());
    assert_eq!(
        bare.create_account("x").unwrap_err(),
        AccountError::Unavailable
    );
    assert!(!home.exists(".claude-x"));
}

// Fix_ForgottenAccountTests.aRemovedAccountIsForgottenUntilAddedBack (the
// store's and the panel's halves are WP5's and WP7's).
#[test]
fn a_removed_account_is_forgotten_until_added_back() {
    let home = Home::new();
    home.mkdir(".claude-temp");
    let mut registry = home.registry();
    let (id, _) = registry.add_folder(&home.path(".claude-temp")).unwrap();
    assert!(!registry.is_forgotten(id.as_str()));
    registry.remove(id.as_str());
    assert!(registry.is_forgotten(id.as_str()));
    assert_eq!(
        registry.forgotten_ids(),
        BTreeSet::from([id.as_str().to_owned()])
    );
    registry.add_folder(&home.path(".claude-temp")).unwrap();
    assert!(!registry.is_forgotten(id.as_str()));
    assert!(registry.forgotten_ids().is_empty());
}

// A signed-in account is forgotten as an identity: by its id, by any of its
// folders, and until a folder of it is added back.
#[test]
fn a_forgotten_identity_is_forgotten_in_every_folder() {
    let home = Home::new();
    home.mkdir(".claude-work/projects");
    home.sign_in(".claude-work/.claude.json", "me@work.com", "u-work", None);
    let mut registry = home.registry();
    let identity = registry.identities()[0].id.clone();
    let work = home.path(".claude-work");
    assert!(!registry.is_forgotten(identity.as_str()));
    registry
        .apply_user(agentnotch_engine::model::AccountAction::Forget {
            id: identity.as_str().to_owned(),
        })
        .unwrap();
    assert!(registry.is_forgotten(identity.as_str()));
    assert!(registry.is_forgotten(&work));
    assert!(registry.is_untracked(&AccountId::new(work.clone()), None));
    assert!(registry.accounts().is_empty());
    home.save(&mut registry);
    let reloaded = home.registry();
    assert!(reloaded.is_forgotten(identity.as_str()));
    assert!(reloaded.accounts().is_empty());

    let mut back = home.registry();
    back.add_folder(&work).unwrap();
    assert!(!back.is_forgotten(identity.as_str()));
    assert!(!back.is_forgotten(&work));
    assert_eq!(back.accounts().len(), 1);
}

// Fix_AccountOwnLabelTests.theOwnLabelIsTheCustomNameElseTheDefault
#[test]
fn the_own_label_is_the_custom_name_else_the_default() {
    let home = Home::new();
    home.mkdir(".claude-research");
    let mut registry = home.registry();
    let (research, _) = registry.add_folder(&home.path(".claude-research")).unwrap();
    registry.rename(research.as_str(), Some("research"));
    assert_eq!(
        registry.accounts()[0].own_label.as_deref(),
        Some("research")
    );
    assert_eq!(registry.accounts()[0].label, "research");

    registry.rename(research.as_str(), None);
    home.sign_in(".claude-research/.claude.json", "me@gmail.com", "u-g", None);
    home.discover(&mut registry);
    let account = &registry.accounts()[0];
    assert_eq!(account.own_label.as_deref(), Some("Claude Gmail"));
    assert_eq!(account.label, "Claude Gmail");
}

// ---- accounts.json ----

fn mac_registry() -> AccountRegistry {
    AccountRegistry::new(Paths::new(PathStyle::Posix, "/Users/me"))
}

fn fixture() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mac-files/accounts.json");
    std::fs::read(path).expect("the Mac fixture")
}

// A file the Mac wrote loads and writes back the same bytes: through the
// file's types byte for byte (Foundation's `"key" : value`), and through the
// registry, which lists the folders in its own order (the default first, then
// by label), as the Mac's does on load.
#[test]
fn the_mac_accounts_json_round_trips_through_the_registry() {
    let bytes = fixture();
    assert_eq!(AccountsFile::parse(&bytes).unwrap().encode(), bytes);

    let mut registry = mac_registry();
    assert!(registry.load(&bytes));
    let mut folders: Vec<&str> = registry.known_folders().iter().map(|f| f.dir()).collect();
    assert_eq!(folders[0], "/Users/me/.claude");
    folders.sort_unstable();
    assert_eq!(
        folders,
        vec![
            "/Users/me/.claude",
            "/Users/me/.claude-work",
            "/Users/me/claude-lab"
        ]
    );
    let work = registry.folder("/Users/me/.claude-work").unwrap();
    assert_eq!(work.source, FolderSource::Hook);
    assert_eq!(
        work.seen_config_dir_envs,
        vec![
            "/Users/me/.claude-work".to_owned(),
            "~/.claude-work".to_owned()
        ]
    );
    assert_eq!(work.custom_label.as_deref(), Some("Work"));
    assert!(registry.folder("/Users/me/claude-lab").unwrap().is_hidden);
    assert!(registry.is_forgotten("/Users/me/.claude-old"));
    assert!(registry.is_forgotten("email:old@example.com"));
    assert!(!registry.default_timeline().is_empty());
    assert!(!registry.needs_save());

    // Everything but the order of the folders is what the Mac wrote: the
    // file's own types, given the registry's order, write the same bytes.
    let written = registry.file_bytes().unwrap();
    let mut expected = AccountsFile::parse(&bytes).unwrap();
    let order: Vec<String> = AccountsFile::parse(&written)
        .unwrap()
        .accounts
        .into_iter()
        .map(|folder| folder.id)
        .collect();
    expected
        .accounts
        .sort_by_key(|folder| order.iter().position(|id| *id == folder.id));
    assert_eq!(written, expected.encode());
    // And it is a fixed point.
    let mut again = mac_registry();
    assert!(again.load(&written));
    assert_eq!(again.file_bytes().unwrap(), written);
}

// Ids are deduplicated by folder and `can_be_account` is checked again on
// load: home, a parent of it, a root and a nested folder never load.
#[test]
fn load_deduplicates_and_rechecks() {
    let text = r#"{"version":2,"accounts":[
      {"id":"/Users/me/.claude-work","configDir":"/Users/me/.claude-work","colorIndex":1,"isHidden":false,"source":"hook"},
      {"id":"/Users/me/.claude-work/","configDir":"/Users/me/.claude-work/","colorIndex":2,"isHidden":true,"source":"manual"},
      {"id":"/Users/me","configDir":"/Users/me","colorIndex":3,"isHidden":false,"source":"manual"},
      {"id":"/Users","configDir":"/Users","colorIndex":3,"isHidden":false,"source":"manual"},
      {"id":"/","configDir":"/","colorIndex":3,"isHidden":false,"source":"manual"},
      {"id":"/Users/me/.claude-shared","configDir":"/Users/me/.claude-shared","colorIndex":4,"isHidden":false,"source":"discovered"}
    ]}"#;
    let mut registry = mac_registry();
    assert!(registry.load(text.as_bytes()));
    let dirs: Vec<&str> = registry.known_folders().iter().map(|f| f.dir()).collect();
    assert!(dirs.contains(&"/Users/me/.claude-work"), "{dirs:?}");
    assert_eq!(
        dirs.iter().filter(|d| d.contains(".claude-work")).count(),
        1
    );
    assert!(!dirs.contains(&"/Users/me") && !dirs.contains(&"/Users") && !dirs.contains(&"/"));
    // The first of two spellings wins.
    assert_eq!(
        registry
            .folder("/Users/me/.claude-work")
            .unwrap()
            .color_index,
        1
    );
}

// Not an accounts.json: refused, and the registry stays empty.
#[test]
fn load_refuses_what_is_not_accounts_json() {
    let mut registry = mac_registry();
    assert!(!registry.load(b"not json"));
    assert!(!registry.load(b""));
    assert!(registry.known_folders().is_empty());
}

// New: a registry with identities, forgotten identities and a timeline
// writes bytes that load into a registry which writes the same bytes.
#[test]
fn a_registry_with_identities_and_a_timeline_round_trips() {
    let home = Home::new();
    home.mkdir(".claude");
    home.sign_in(".claude.json", "me@personal.dev", "u-personal", None);
    home.mkdir(".claude-work/projects");
    home.sign_in(
        ".claude-work/.claude.json",
        "me@work.com",
        "u-work",
        Some("Acme"),
    );
    home.mkdir(".claude-second/projects");
    home.sign_in(
        ".claude-second/.claude.json",
        "second@example.com",
        "u-second-login",
        None,
    );
    let mut registry = home.bare_registry(&[]);
    home.discover_at(&mut registry, after(0));
    // The default folder's login changes: a second span of the timeline.
    home.sign_in(
        ".claude.json",
        "someone.else@personal.dev",
        "u-second",
        None,
    );
    home.discover_at(&mut registry, after(3600));
    assert!(!registry.default_timeline().is_empty());

    let work = home.path(".claude-work");
    registry.rename(&work, Some("Work"));
    registry.set_hidden(&work, true);
    let old = registry
        .identity_of_folder(&home.path(".claude-second"))
        .unwrap()
        .id
        .clone();
    registry.remove(old.as_str());
    registry.dismiss_suggestion(&home.path(".claude-elsewhere"));
    registry.record(
        home.sighting(".claude-work", Some(&work), after(7200)),
        after(7200),
    );

    let bytes = registry.file_bytes().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    for needle in [
        "\"defaultIdentityTimeline\"",
        "\"forgottenIdentities\"",
        "\"identities\"",
        "\"removedIds\"",
        "\"customLabel\" : \"Work\"",
        "\"seenConfigDirEnvs\"",
    ] {
        assert!(text.contains(needle), "{needle} in {text}");
    }
    let mut loaded = home.bare_registry(&[]);
    assert!(loaded.load(&bytes));
    assert_eq!(loaded.file_bytes().unwrap(), bytes);
    // And through one more discovery the same choices come out.
    home.discover_at(&mut loaded, after(7300));
    assert_eq!(
        loaded.folder(&work).unwrap().custom_label.as_deref(),
        Some("Work")
    );
    assert!(loaded.folder(&work).unwrap().is_hidden);
    assert!(loaded.is_forgotten(old.as_str()));
    assert!(loaded.is_forgotten(&home.path(".claude-elsewhere")));
    let labels: Vec<String> = loaded.accounts().into_iter().map(|a| a.label).collect();
    assert_eq!(labels.len(), 2, "{labels:?}");
}

// A registry that holds fixtures (a sealed run) saves nothing and takes no
// sightings.
#[test]
fn a_sealed_registry_saves_nothing() {
    let home = Home::new();
    let mut registry = home.registry();
    registry.replace_all_with_fixtures(Vec::new(), None);
    assert!(registry.file_bytes().is_none());
    assert!(!registry.needs_save());
    let changed = registry.record(home.sighting(".claude-work", None, after(0)), after(0));
    assert!(!changed.any());
}

// Foundation writes an empty collection as an open line between its
// brackets (checked against `JSONEncoder` with `.prettyPrinted`).
#[test]
fn an_empty_registry_is_written_the_way_foundation_writes_it() {
    let registry = mac_registry();
    let text = String::from_utf8(registry.file_bytes().unwrap()).unwrap();
    assert_eq!(
        text,
        "{\n  \"accounts\" : [\n\n  ],\n  \"removedIds\" : [\n\n  ],\n  \"version\" : 2\n}\n"
    );
}
