//! What the cloud thread reads of the accounts (CloudLiveEnvironment.swift,
//! SessionLedgerTests' `CloudBackfill.login` and
//! `backfillFoldersAreRunFoldersOfTheirOwnAccount`; CL§1.6, §7.2): the login
//! digest, the website's account key (against the contract's fixture, read
//! in place), who is signed in to each folder, the folders the backfill may
//! read, and the accounts the website may hear of.
//!
//! Hand-built Windows snapshots, no disk: it runs the same on the Mac.

mod accounts_support;

use accounts_support::{after, win};
use agentnotch_engine::accounts::for_cloud::{account_key, login_digest, organization_of};
use agentnotch_engine::accounts::AccountRegistry;
use agentnotch_engine::model::{
    ConfigRead, FolderFacts, FolderKind, FolderSnapshot, Identity, IdentityId,
    ParallelProfilesManifest,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::PathBuf;

const HOME: &str = r"C:\Users\me";

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---- hand-built reads ----

fn login(email: Option<&str>, uuid: Option<&str>, organization: Option<&str>) -> Identity {
    Identity {
        account_uuid: uuid.map(str::to_owned),
        email: email.map(str::to_owned),
        organization_uuid: organization.map(str::to_owned),
        ..Identity::default()
    }
}

fn config(identity: Identity) -> ConfigRead {
    ConfigRead {
        identity: Some(identity),
        modified_at: Some(after(0)),
    }
}

/// A Claude folder with history, signed in as `who` when given.
fn facts(path: &str, who: Option<Identity>) -> FolderFacts {
    FolderFacts {
        path: path.to_owned(),
        has_global_config: who.is_some(),
        is_signed_in: who.is_some(),
        has_projects: true,
        own_config: who.map(config),
        canonical: Some(path.to_owned()),
        ..FolderFacts::default()
    }
}

fn stored(path: &str, who: Identity) -> FolderFacts {
    FolderFacts {
        has_store_marker: true,
        ..facts(path, Some(who))
    }
}

fn snapshot(folders: Vec<FolderFacts>, home_login: Option<Identity>) -> FolderSnapshot {
    FolderSnapshot {
        home: HOME.to_owned(),
        home_config: home_login.map(config),
        home_canonical: Some(HOME.to_owned()),
        requested: folders.iter().map(|f| f.path.clone()).collect(),
        folders,
        ..FolderSnapshot::default()
    }
}

fn registry_over(snapshot: FolderSnapshot) -> AccountRegistry {
    let mut registry = AccountRegistry::new(win());
    registry.discover(snapshot, after(0));
    registry
}

fn identity_id(uuid: &str) -> IdentityId {
    IdentityId::new(format!("uuid:{uuid}"))
}

// ---- login_digest (CloudBackfill.login) ----

#[test]
fn a_login_is_who_the_folders_own_oauth_account_names() {
    let a = login_digest(Some("U"), Some("O"), Some("me@example.com"));
    // Case-insensitive, and spaces around a field don't matter.
    assert_eq!(
        a,
        login_digest(Some("u"), Some("o"), Some("ME@example.com"))
    );
    assert_eq!(
        a,
        login_digest(Some(" u "), Some("o"), Some("me@example.com "))
    );
    // Another organization, another email, another account: another login.
    assert_ne!(
        a,
        login_digest(Some("u"), Some("other"), Some("me@example.com"))
    );
    assert_ne!(a, login_digest(Some("u"), None, Some("me@example.com")));
    assert_ne!(
        a,
        login_digest(Some("u"), Some("o"), Some("you@example.com"))
    );
    assert_ne!(
        a,
        login_digest(Some("v"), Some("o"), Some("me@example.com"))
    );
    // Nobody is signed in: none. An organization alone isn't a login.
    assert_eq!(login_digest(None, None, None), None);
    assert_eq!(login_digest(Some(""), Some(" "), Some("  ")), None);
    assert_eq!(login_digest(None, Some("o"), None), None);
    // An email alone, or an account UUID alone, is one.
    assert!(login_digest(None, None, Some("me@example.com")).is_some());
    assert!(login_digest(Some("u"), None, None).is_some());
}

#[test]
fn the_login_digest_is_a_sha256_of_the_cleaned_fields() {
    let digest = login_digest(Some("U"), Some("O"), Some("Me@Example.com")).unwrap();
    assert_eq!(digest, sha256_hex("u|o|me@example.com"));
    let email_only = login_digest(None, None, Some("me@example.com")).unwrap();
    assert_eq!(email_only, sha256_hex("||me@example.com"));
    // Local only: what it is made of is not in it.
    assert!(!digest.contains("example"));
}

// ---- account_key against the contract's fixture ----

fn fixture() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/contract/fixtures/keys.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("the contract's keys.json at {path:?}: {error}"));
    serde_json::from_str(&text).expect("keys.json parses")
}

/// The identity a single folder signed in as `who` makes.
fn account_of(who: Identity) -> (AccountRegistry, BTreeSet<String>) {
    let registry = registry_over(snapshot(
        vec![facts(r"C:\Users\me\.claude-a", Some(who))],
        None,
    ));
    let corrected = registry.corrected_folders().clone();
    (registry, corrected)
}

#[test]
fn account_keys_are_the_contracts() {
    let fixture = fixture();
    let accounts = fixture["accounts"].as_array().expect("accounts");
    assert_eq!(
        accounts.len(),
        2,
        "the fixture has an account with and without an organization"
    );
    for account in accounts {
        let uuid = account["accountUuid"].as_str().expect("accountUuid");
        let organization = account["organizationUuid"].as_str();
        let expected = account["key"].as_str().expect("key");
        for (uuid, organization) in [
            (uuid.to_owned(), organization.map(str::to_owned)),
            // Upper-casing the inputs gives the same key.
            (uuid.to_uppercase(), organization.map(str::to_uppercase)),
        ] {
            let (registry, corrected) = account_of(login(
                Some("me@example.com"),
                Some(&uuid),
                organization.as_deref(),
            ));
            let identity = &registry.identities()[0];
            assert_eq!(
                account_key(identity, &corrected).as_deref(),
                Some(expected),
                "{uuid} / {organization:?}"
            );
        }
    }
    // The rule the fixture's comment states.
    assert_eq!(
        accounts[0]["key"].as_str().unwrap(),
        sha256_hex(&accounts[0]["accountUuid"].as_str().unwrap().to_lowercase())
    );
    assert_eq!(
        accounts[1]["key"].as_str().unwrap(),
        sha256_hex(&format!(
            "{}/{}",
            accounts[1]["accountUuid"].as_str().unwrap(),
            accounts[1]["organizationUuid"].as_str().unwrap()
        ))
    );
}

// The identity's own organization when it was split by one; the folders'
// otherwise; a folder that names none leaves the UUID alone.
#[test]
fn the_organization_comes_from_the_identity_or_its_own_folders() {
    let (registry, corrected) =
        account_of(login(Some("me@example.com"), Some("u1"), Some("org-1")));
    assert_eq!(
        organization_of(&registry.identities()[0], &corrected).as_deref(),
        Some("org-1")
    );
    let (registry, corrected) = account_of(login(Some("me@example.com"), Some("u1"), None));
    assert_eq!(organization_of(&registry.identities()[0], &corrected), None);
    // One login in two organizations is two identities, each keyed by its own.
    let registry = registry_over(snapshot(
        vec![
            facts(
                r"C:\Users\me\.claude-x",
                Some(login(Some("me@example.com"), Some("u1"), Some("org-x"))),
            ),
            facts(
                r"C:\Users\me\.claude-y",
                Some(login(Some("me@example.com"), Some("u1"), Some("org-y"))),
            ),
        ],
        None,
    ));
    let corrected = registry.corrected_folders().clone();
    let keys: BTreeSet<Option<String>> = registry
        .identities()
        .iter()
        .map(|identity| account_key(identity, &corrected))
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([Some(sha256_hex("u1/org-x")), Some(sha256_hex("u1/org-y"))])
    );
}

// A mirrored (corrected) folder's organization is stale: never used.
#[test]
fn a_corrected_folders_stale_organization_never_names_the_key() {
    // `.claude-mirror` was rewritten to say this email, but kept another
    // account's UUID and organization; that UUID also has its own email.
    let registry = registry_over(snapshot(
        vec![
            facts(
                r"C:\Users\me\.claude-real",
                Some(login(Some("me@example.com"), Some("real"), None)),
            ),
            facts(
                r"C:\Users\me\.claude-mirror",
                Some(login(
                    Some("me@example.com"),
                    Some("stale"),
                    Some("org-stale"),
                )),
            ),
            facts(
                r"C:\Users\me\.claude-other",
                Some(login(
                    Some("other@example.com"),
                    Some("stale"),
                    Some("org-stale"),
                )),
            ),
        ],
        None,
    ));
    let mirror = r"C:\Users\me\.claude-mirror";
    assert!(registry.corrected_folders().contains(mirror));
    let corrected = registry.corrected_folders().clone();
    let real = registry
        .identities()
        .iter()
        .find(|identity| identity.id.as_str() == "uuid:real")
        .expect("the real account holds the mirror");
    assert_eq!(real.run_dirs.len(), 2);
    assert_eq!(organization_of(real, &corrected), None);
    assert_eq!(
        account_key(real, &corrected),
        Some(sha256_hex("real")),
        "the UUID alone: the organization only the mirror names is stale"
    );
    // The other account is keyed by its own organization.
    let other = registry
        .identities()
        .iter()
        .find(|identity| identity.id.as_str() == "uuid:stale")
        .expect("the stale uuid's own account");
    assert_eq!(
        account_key(other, &corrected),
        Some(sha256_hex("stale/org-stale"))
    );
}

// An email-only login and a folder nobody signed in to have no key that is
// the same on every computer.
#[test]
fn email_only_and_dir_identities_have_no_key() {
    let (registry, corrected) = account_of(login(Some("me@example.com"), None, None));
    let identity = registry.identities()[0].clone();
    assert!(identity.id.as_str().starts_with("email:"));
    assert_eq!(account_key(&identity, &corrected), None);
    // A `dir:` identity (a folder added by hand): none either, and an empty
    // UUID isn't one.
    let mut dir = identity.clone();
    dir.id = IdentityId::new(r"dir:C:\Users\me\.claude-empty");
    assert_eq!(account_key(&dir, &corrected), None);
    dir.id = IdentityId::new("uuid:");
    assert_eq!(account_key(&dir, &corrected), None);
}

// ---- folder_logins ----

#[test]
fn folder_logins_wait_for_the_first_read_then_key_by_the_paths_key() {
    let paths = win();
    let mut registry = AccountRegistry::new(win());
    assert_eq!(
        registry.folder_logins(),
        None,
        "before identities were read"
    );

    let signed = login(Some("me@example.com"), Some("u1"), Some("o1"));
    let work = r"C:\Users\me\.claude-Work";
    registry.discover(
        snapshot(
            vec![
                facts(work, Some(signed.clone())),
                // Signed out: left out, not an empty login.
                facts(r"C:\Users\me\.claude-empty", None),
                facts(
                    r"C:\Users\me\.claude-b",
                    Some(login(None, Some("u2"), None)),
                ),
            ],
            None,
        ),
        after(0),
    );
    let logins = registry.folder_logins().expect("identities were read");
    assert_eq!(logins.len(), 2, "{logins:?}");
    // Keyed by `Paths::key`: lower case, whatever the display case.
    assert_eq!(paths.key(work), r"c:\users\me\.claude-work");
    assert_eq!(
        logins.get(&paths.key(work)).map(String::as_str),
        login_digest(Some("u1"), Some("o1"), Some("me@example.com")).as_deref()
    );
    assert_eq!(
        logins
            .get(&paths.key(r"C:\Users\me\.claude-b"))
            .map(String::as_str),
        login_digest(Some("u2"), None, None).as_deref()
    );
    assert!(!logins.contains_key(&paths.key(r"C:\Users\me\.claude-empty")));
    // A `/login` as someone else changes the folder's digest.
    registry.discover(
        snapshot(
            vec![facts(
                work,
                Some(login(Some("you@example.com"), Some("u9"), Some("o1"))),
            )],
            None,
        ),
        after(100),
    );
    let after_login = registry.folder_logins().expect("read");
    assert_ne!(
        after_login.get(&paths.key(work)),
        logins.get(&paths.key(work))
    );
}

// ---- backfill_folders (backfillFoldersAreRunFoldersOfTheirOwnAccount) ----

fn own_dirs(folders: &[agentnotch_engine::model::BackfillFolder]) -> Vec<String> {
    folders
        .iter()
        .filter(|f| f.account_key.is_some())
        .map(|f| f.config_dir.clone())
        .collect()
}

#[test]
fn backfill_folders_are_run_folders_of_their_own_account() {
    let paths = win();
    let me = login(Some("me@example.com"), Some("real"), Some("org-1"));
    let registry = registry_over(snapshot(
        vec![
            // `~\.claude`, `.claude-own`: run folders of the account.
            facts(r"C:\Users\me\.claude", Some(me.clone())),
            facts(r"C:\Users\me\.claude-own", Some(me.clone())),
            // Another account, not allowed.
            facts(
                r"C:\Users\me\.claude-b",
                Some(login(Some("b@example.com"), Some("bee"), None)),
            ),
            // Nobody is signed in.
            facts(r"C:\Users\me\.claude-empty", None),
            // A copy whose accountUuid is another account's: corrected.
            facts(
                r"C:\Users\me\.claude-mirror",
                Some(login(
                    Some("me@example.com"),
                    Some("stale"),
                    Some("org-stale"),
                )),
            ),
            facts(
                r"C:\Users\me\.claude-third",
                Some(login(
                    Some("third@example.com"),
                    Some("stale"),
                    Some("org-stale"),
                )),
            ),
            // The shared history.
            facts(r"C:\Users\me\.claude-shared", None),
        ],
        Some(me.clone()),
    ));
    assert!(registry
        .corrected_folders()
        .contains(r"C:\Users\me\.claude-mirror"));
    assert!(!registry.mirrors_default());

    let allowed = BTreeSet::from([identity_id("real"), identity_id("stale")]);
    let folders = registry.backfill_folders(&allowed);
    // Not the mirror (corrected), not `.claude-b`, not the unsigned one.
    assert_eq!(
        own_dirs(&folders)
            .iter()
            .map(|dir| paths.key(dir))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            paths.key(r"C:\Users\me\.claude"),
            paths.key(r"C:\Users\me\.claude-own"),
            paths.key(r"C:\Users\me\.claude-third"),
        ])
    );
    let own = folders
        .iter()
        .find(|f| paths.same(&f.config_dir, r"C:\Users\me\.claude-own"))
        .expect("the folder");
    assert_eq!(own.identity_id, Some(identity_id("real")));
    assert_eq!(own.account_key, Some(sha256_hex("real/org-1")));
    // The cloud's to fill, from its own record.
    assert_eq!(own.signed_in_since, None);
    let mirror = folders
        .iter()
        .find(|f| paths.same(&f.config_dir, r"C:\Users\me\.claude-mirror"))
        .expect("present, with no account");
    assert_eq!((&mirror.identity_id, &mirror.account_key), (&None, &None));

    // The shared history's folder follows with no account, last.
    let last = folders.last().expect("folders");
    assert!(paths.same(&last.config_dir, r"C:\Users\me\.claude-shared"));
    assert_eq!((&last.identity_id, &last.account_key), (&None, &None));

    // Nothing allowed, nothing to read.
    assert!(own_dirs(&registry.backfill_folders(&BTreeSet::new())).is_empty());
}

// Claude Parallel Profiles: a store is identity only; `~\.claude` is a
// mirror whose history can't be told; a window's working copy is a run folder.
#[test]
fn backfill_skips_stores_and_the_mirrored_default() {
    let paths = win();
    let me = login(Some("me@example.com"), Some("real"), None);
    let store = r"C:\Users\me\.claude-paras";
    let window = r"C:\Users\me\.claude-windows\801f9dd51396";
    let mut read = snapshot(
        vec![
            facts(r"C:\Users\me\.claude", Some(me.clone())),
            stored(store, me.clone()),
            facts(window, Some(me.clone())),
            facts(r"C:\Users\me\.claude-shared", None),
            facts(r"C:\Users\me\.claude-windows", None),
        ],
        Some(me.clone()),
    );
    read.manifest = Some(ParallelProfilesManifest {
        stores: vec![store.to_owned()],
        created: vec![store.to_owned()],
    });
    let registry = registry_over(read);
    assert!(registry.mirrors_default());
    let stores = registry
        .known_folders()
        .iter()
        .filter(|f| f.kind == FolderKind::Store)
        .count();
    assert_eq!(stores, 1);

    let allowed = BTreeSet::from([identity_id("real")]);
    let folders = registry.backfill_folders(&allowed);
    assert_eq!(
        own_dirs(&folders)
            .iter()
            .map(|dir| paths.key(dir))
            .collect::<Vec<_>>(),
        vec![paths.key(window)],
        "only the window's copy: not the store, not the mirrored default"
    );
    // Every folder is there; the store and the default just have no account.
    for dir in [store, r"C:\Users\me\.claude"] {
        let folder = folders
            .iter()
            .find(|f| paths.same(&f.config_dir, dir))
            .unwrap_or_else(|| panic!("{dir} is listed"));
        assert_eq!(folder.account_key, None, "{dir}");
    }
    // The shared history and the windows folder come with no account.
    for dir in [
        r"C:\Users\me\.claude-shared",
        r"C:\Users\me\.claude-windows",
    ] {
        let folder = folders
            .iter()
            .find(|f| paths.same(&f.config_dir, dir))
            .unwrap_or_else(|| panic!("{dir} is listed"));
        assert_eq!((&folder.identity_id, &folder.account_key), (&None, &None));
    }
}

// A history that folders link into is seen as shared: its folder is listed
// with no account, whatever it is called.
#[test]
fn a_folder_that_others_link_their_history_from_is_listed_without_an_account() {
    let paths = win();
    let me = login(Some("me@example.com"), Some("real"), None);
    let histories = r"C:\Users\me\histories";
    let mut linked = facts(r"C:\Users\me\.claude-a", Some(me.clone()));
    linked.link_targets = vec![format!(r"{histories}\projects")];
    let registry = registry_over(snapshot(
        vec![
            linked,
            facts(r"C:\Users\me\.claude-b", Some(me.clone())),
            facts(histories, None),
        ],
        None,
    ));
    let allowed = BTreeSet::from([identity_id("real")]);
    let folders = registry.backfill_folders(&allowed);
    let shared = folders
        .iter()
        .find(|f| paths.same(&f.config_dir, histories))
        .expect("the shared history is listed");
    assert_eq!((&shared.identity_id, &shared.account_key), (&None, &None));
    assert_eq!(
        registry
            .infrastructure_dirs()
            .iter()
            .map(|dir| paths.key(dir))
            .collect::<Vec<_>>(),
        vec![paths.key(histories)]
    );
    // The two account folders are still the account's own.
    assert_eq!(own_dirs(&folders).len(), 2);
}

// ---- cloud_accounts ----

fn cloud_registry() -> AccountRegistry {
    let read =
        |email: Option<&str>, uuid: Option<&str>, org: Option<&str>| Some(login(email, uuid, org));
    registry_over(snapshot(
        vec![
            facts(
                r"C:\Users\me\.claude",
                read(Some("me@example.com"), Some("u-me"), Some("o-me")),
            ),
            facts(
                r"C:\Users\me\.claude-me2",
                read(Some("me@example.com"), Some("u-me"), Some("o-me")),
            ),
            // Only an email: no key that is the same on every computer.
            facts(
                r"C:\Users\me\.claude-mail",
                read(Some("mail@example.com"), None, None),
            ),
            facts(
                r"C:\Users\me\.claude-hid",
                read(Some("hid@example.com"), Some("u-hid"), None),
            ),
            facts(
                r"C:\Users\me\.claude-gone",
                read(Some("gone@example.com"), Some("u-gone"), None),
            ),
            facts(r"C:\Users\me\.claude-off", None),
        ],
        // `~\.claude` runs unset: it reads `~\.claude.json`.
        read(Some("me@example.com"), Some("u-me"), Some("o-me")),
    ))
}

#[test]
fn cloud_accounts_are_tracked_remembered_and_signed_in_with_a_uuid() {
    let paths = win();
    let mut registry = cloud_registry();
    let ids = |registry: &AccountRegistry| -> Vec<String> {
        registry
            .cloud_accounts()
            .iter()
            .map(|account| account.identity_id.as_str().to_owned())
            .collect()
    };
    let mut everyone = ids(&registry);
    everyone.sort();
    assert_eq!(
        everyone,
        ["uuid:u-gone", "uuid:u-hid", "uuid:u-me"],
        "not the email-only or unsigned ones"
    );

    // Untracked ("Track sessions and hooks" off): not heard of.
    registry.set_hidden("uuid:u-hid", true);
    // Forgotten: not heard of either.
    registry.remove("uuid:u-gone");
    assert_eq!(ids(&registry), ["uuid:u-me"]);

    let accounts = registry.cloud_accounts();
    let me = &accounts[0];
    assert_eq!(me.email.as_deref(), Some("me@example.com"));
    assert!(me.label.as_deref().is_some_and(|label| !label.is_empty()));
    assert_eq!(me.folders.len(), 2);
    let default = me
        .folders
        .iter()
        .find(|f| f.is_default)
        .expect("`~\\.claude` is one of its folders");
    assert_eq!(
        paths.key(&default.config_dir.to_string_lossy()),
        paths.key(r"C:\Users\me\.claude")
    );
    assert_eq!(default.kind, FolderKind::Run);
    assert!(!default.mirrored_default, "no extension here");
    assert!(!default.corrected);
    assert_eq!(default.organization_uuid.as_deref(), Some("o-me"));
    // A discovered custom folder is used as CLAUDE_CONFIG_DIR=<path>; the
    // default one is used unset.
    assert_eq!(default.config_dir_env, None);
    let second = me.folders.iter().find(|f| !f.is_default).unwrap();
    assert_eq!(
        second.config_dir_env.as_deref(),
        Some(r"C:\Users\me\.claude-me2")
    );
    let me_identity = registry.identity("uuid:u-me").expect("the account");
    assert_eq!(
        account_key(me_identity, registry.corrected_folders()),
        Some(sha256_hex("u-me/o-me"))
    );

    // Added back: heard of again.
    registry.set_hidden("uuid:u-hid", false);
    assert_eq!(ids(&registry), ["uuid:u-hid", "uuid:u-me"]);
}

#[test]
fn cloud_folders_say_which_are_mirrored_or_corrected() {
    let me = login(Some("me@example.com"), Some("real"), None);
    let store = r"C:\Users\me\.claude-paras";
    let mut read = snapshot(
        vec![
            facts(r"C:\Users\me\.claude", Some(me.clone())),
            stored(store, me.clone()),
        ],
        Some(me.clone()),
    );
    read.manifest = Some(ParallelProfilesManifest {
        stores: vec![store.to_owned()],
        created: vec![store.to_owned()],
    });
    let registry = registry_over(read);
    let accounts = registry.cloud_accounts();
    assert_eq!(accounts.len(), 1);
    let folders = &accounts[0].folders;
    let default = folders.iter().find(|f| f.is_default).expect("default");
    assert!(default.mirrored_default);
    let store = folders
        .iter()
        .find(|f| f.kind == FolderKind::Store)
        .expect("the store");
    assert!(!store.is_default && !store.mirrored_default);
}
