//! The registry driven with Windows paths and hand-built snapshots, so it
//! runs the same on the Mac: spellings of one folder collapse by
//! `Paths::key` (no "login conflict" on Windows, where a login is a file in
//! the folder itself), discovery of a Windows home's `.claude-*` folders,
//! `~\` in what people read, and `AGENTNOTCH_EXTRA_CONFIG_DIRS` split on `;`.

mod accounts_support;

use accounts_support::{after, win};
use agentnotch_engine::accounts::AccountRegistry;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{
    AccountId, AccountSighting, ConfigRead, FolderFacts, FolderKind, FolderSnapshot, Identity,
    SessionId,
};
use std::collections::BTreeSet;

const HOME: &str = r"C:\Users\me";
const WORK: &str = r"C:\Users\me\.claude-work";

fn identity(email: &str, uuid: &str) -> Identity {
    Identity {
        account_uuid: Some(uuid.to_owned()),
        email: Some(email.to_owned()),
        ..Identity::default()
    }
}

fn config(identity: Option<Identity>) -> ConfigRead {
    ConfigRead {
        identity,
        modified_at: Some(after(0)),
    }
}

/// What a read of `path` found: a Claude folder with history, signed in as
/// `login` when there is one.
fn facts(path: &str, login: Option<Identity>) -> FolderFacts {
    FolderFacts {
        path: path.to_owned(),
        has_global_config: login.is_some(),
        is_signed_in: login.is_some(),
        has_projects: true,
        own_config: login.map(|login| config(Some(login))),
        canonical: Some(path.to_owned()),
        ..FolderFacts::default()
    }
}

/// One read of a Windows home: `folders`, and who `~\.claude.json` names.
fn snapshot(folders: Vec<FolderFacts>, home_login: Option<Identity>) -> FolderSnapshot {
    FolderSnapshot {
        home: HOME.to_owned(),
        home_config: home_login.map(|login| config(Some(login))),
        home_canonical: Some(HOME.to_owned()),
        requested: folders.iter().map(|f| f.path.clone()).collect(),
        folders,
        ..FolderSnapshot::default()
    }
}

fn sighting(dir: &str, env: Option<&str>, seconds: u64) -> AccountSighting {
    AccountSighting {
        config_dir: AccountId::new(dir),
        config_dir_env: env.map(str::to_owned),
        session_id: SessionId::new("s"),
        at: after(seconds),
    }
}

fn registry() -> AccountRegistry {
    AccountRegistry::new(win())
}

/// Nothing an account or a folder says is about a login conflict.
fn assert_no_login_conflict(registry: &AccountRegistry) {
    let said = serde_json::to_string(&(registry.accounts(), registry.folders()))
        .expect("accounts serialize")
        .to_lowercase();
    assert!(!said.contains("conflict"), "{said}");
}

// The three spellings a Windows session can name one folder by: the same
// folder, one entry, the first spelling kept, and no change after the first.
#[test]
fn spellings_of_one_folder_collapse() {
    let mut registry = registry();
    registry.discover(
        snapshot(
            vec![facts(WORK, Some(identity("me@work.dev", "u-work")))],
            None,
        ),
        after(0),
    );
    let first = WORK;
    let second = "c:/users/ME/.claude-work/";
    let third = r"\\?\C:\Users\me\.claude-work";

    let changed = registry.record(sighting(first, Some(first), 10), after(10));
    assert!(changed.any(), "the first spelling is news");
    // Within a minute of the first: `last_seen_at` is written once per 60 s
    // and is not what is being looked at.
    for (n, spelling) in [(1, second), (2, third), (3, first)] {
        let changed = registry.record(sighting(spelling, Some(spelling), 10 + n), after(10 + n));
        assert!(!changed.any(), "{spelling}: {changed:?}");
    }

    assert_eq!(registry.known_folders().len(), 1);
    let folder = registry.folder(WORK).expect("the folder");
    assert_eq!(folder.seen_config_dir_envs, vec![first.to_owned()]);
    assert_eq!(folder.config_dir_env.as_deref(), Some(first));
    // Any spelling finds it.
    for spelling in [second, third, r"C:\USERS\ME\.CLAUDE-WORK\"] {
        assert_eq!(
            registry.folder(spelling).map(|f| f.dir()),
            Some(folder.dir()),
            "{spelling}"
        );
    }
    // A read of the disk changes nothing either.
    let settled = registry.discover(
        snapshot(
            vec![facts(WORK, Some(identity("me@work.dev", "u-work")))],
            None,
        ),
        after(500),
    );
    assert!(!settled.any(), "{settled:?}");
    assert_eq!(registry.accounts().len(), 1);
    assert_no_login_conflict(&registry);
}

// A folder first met through a spelling nobody wrote in a hook: still one.
#[test]
fn a_sighting_before_any_read_is_one_folder_too() {
    let mut registry = registry();
    let created = registry.record(sighting(r"\\?\C:\Users\me\.claude-work", None, 0), after(0));
    assert!(created.any());
    registry.record(
        sighting(
            "c:/users/ME/.claude-work/",
            Some("c:/users/ME/.claude-work/"),
            100,
        ),
        after(100),
    );
    registry.record(sighting(WORK, Some(WORK), 200), after(200));
    assert_eq!(registry.known_folders().len(), 1);
    let folder = &registry.known_folders()[0];
    assert_eq!(folder.dir(), WORK);
    // The raw variable, verbatim, of the first sighting that had one.
    assert_eq!(
        folder.seen_config_dir_envs,
        vec!["c:/users/ME/.claude-work/".to_owned()]
    );
    assert_no_login_conflict(&registry);
}

// `~\.claude` reads `~\.claude.json` when CLAUDE_CONFIG_DIR is unset and
// `~\.claude\.claude.json` when it is set to it: two variants, kept apart;
// every spelling of "set" is one.
#[test]
fn the_default_folder_keeps_set_and_unset_apart() {
    let paths = win();
    let dir = paths.default_config_dir();
    assert_eq!(dir, r"C:\Users\me\.claude");
    assert_eq!(
        paths.global_config_file(&dir, None),
        r"C:\Users\me\.claude.json"
    );
    assert_eq!(
        paths.global_config_file(&dir, Some(&dir)),
        r"C:\Users\me\.claude\.claude.json"
    );
    // The same file however the variable was spelled.
    assert_eq!(
        paths.key(&paths.global_config_file(&dir, Some("c:/USERS/me/.claude/"))),
        paths.key(&paths.global_config_file(&dir, Some(&dir)))
    );

    let mut registry = registry();
    registry.discover(
        snapshot(
            vec![facts(&dir, None)],
            Some(identity("me@personal.dev", "u-personal")),
        ),
        after(0),
    );
    registry.record(sighting(&dir, None, 10), after(10));
    registry.record(sighting(&dir, Some(&dir), 80), after(80));
    registry.record(
        sighting(&dir, Some("c:/Users/ME/.claude/"), 150),
        after(150),
    );
    registry.record(
        sighting(&dir, Some(r"\\?\C:\Users\me\.claude"), 220),
        after(220),
    );
    let folder = registry.folder(&dir).expect("the default folder");
    assert_eq!(
        folder.seen_config_dir_envs,
        vec![String::new(), dir.clone()],
        "unset and set are two variants; the spellings of set are one"
    );
    assert_no_login_conflict(&registry);
}

// Only the unset spelling has a login (`~\.claude.json`): the folder moves
// there once, then a settled read reports nothing.
#[test]
fn the_default_folder_moves_to_the_spelling_that_is_signed_in() {
    let paths = win();
    let dir = paths.default_config_dir();
    let read = || {
        // `~\.claude\.claude.json` does not exist; `~\.claude.json` does.
        snapshot(
            vec![facts(&dir, None)],
            Some(identity("me@personal.dev", "u-personal")),
        )
    };
    let mut registry = registry();
    registry.discover(read(), after(0));
    registry.record(sighting(&dir, Some(&dir), 10), after(10));
    registry.record(sighting(&dir, None, 80), after(80));
    assert_eq!(
        registry.folder(&dir).unwrap().config_dir_env.as_deref(),
        Some(dir.as_str()),
        "sightings alone never move it"
    );
    registry.discover(read(), after(200));
    let settled = registry.folder(&dir).unwrap();
    assert_eq!(settled.config_dir_env, None);
    assert_eq!(
        settled.identity.as_ref().and_then(|i| i.email.as_deref()),
        Some("me@personal.dev")
    );
    assert_eq!(settled.seen_config_dir_envs.len(), 2);
    for n in 0..3 {
        registry.record(sighting(&dir, Some(&dir), 300 + n), after(300 + n));
        let changed = registry.discover(read(), after(400 + n));
        assert_eq!(registry.folder(&dir).unwrap().config_dir_env, None);
        assert!(!changed.any(), "{changed:?}");
    }
    assert_no_login_conflict(&registry);
}

// A Windows home's `.claude-*` children are discovered when they look like
// Claude Code's; ids and map keys are by `Paths::key`.
#[test]
fn a_windows_home_is_discovered_by_its_claude_children() {
    let mut registry = registry();
    let work = r"C:\Users\me\.claude-Work";
    let pending = r"C:\Users\me\.claude-pending";
    let backup = r"C:\Users\me\.claude-backup";
    let unrelated = r"C:\Users\me\.claudette";
    registry.discover(
        snapshot(
            vec![
                facts(work, Some(identity("me@work.dev", "u-work"))),
                facts(pending, None),
                facts(backup, Some(identity("me@work.dev", "u-work"))),
                facts(unrelated, Some(identity("me@work.dev", "u-work"))),
                facts(r"C:\Users\me", None),
            ],
            Some(identity("me@personal.dev", "u-personal")),
        ),
        after(0),
    );
    let known: Vec<String> = registry
        .known_folders()
        .iter()
        .map(|f| f.dir().to_owned())
        .collect();
    // `~\.claude.json` names a login, but `~\.claude` itself is only an
    // account when the folder exists: it doesn't here.
    assert_eq!(known, vec![work.to_owned()]);
    // Ids are the normalized path in display case; any spelling finds it.
    let paths = win();
    assert_eq!(registry.folder(work).unwrap().id.as_str(), work);
    assert_eq!(
        registry.folder("c:/users/me/.CLAUDE-work").map(|f| &f.id),
        Some(&registry.folder(work).unwrap().id)
    );
    // Map keys are by `key()`: lower case on Windows.
    let logins = registry.folder_logins().expect("identities were read");
    assert_eq!(
        logins.keys().cloned().collect::<Vec<_>>(),
        vec![paths.key(work)]
    );
    assert_eq!(paths.key(work), r"c:\users\me\.claude-work");
    // A backup copy is only suggested, as is a folder nobody is signed in to.
    let suggested: BTreeSet<String> = registry
        .suggestions()
        .iter()
        .map(|s| paths.key(&s.config_dir))
        .collect();
    assert_eq!(
        suggested,
        BTreeSet::from([paths.key(pending), paths.key(backup)])
    );
    assert_eq!(registry.folder(work).unwrap().kind, FolderKind::Run);
}

// What people read: `~\…`, whatever the spelling; outside home, as written.
#[test]
fn paths_are_shown_with_a_tilde() {
    let paths = win();
    for spelling in [
        WORK,
        "c:/users/ME/.claude-work/",
        r"\\?\C:\Users\me\.claude-work",
        r"~\.claude-work",
    ] {
        assert_eq!(paths.abbreviate(spelling), r"~\.claude-work", "{spelling}");
    }
    assert_eq!(paths.abbreviate(HOME), "~");
    assert_eq!(
        paths.abbreviate(r"C:\Users\me\.claude-windows\1bf3e8f92b11"),
        r"~\.claude-windows\1bf3e8f92b11"
    );
    assert_eq!(paths.abbreviate(r"D:\Work\claude"), r"D:\Work\claude");
    assert_eq!(
        paths.abbreviate(r"C:\Users\meow\.claude"),
        r"C:\Users\meow\.claude"
    );
}

// AGENTNOTCH_EXTRA_CONFIG_DIRS is split on `;` (a `:` is part of a drive),
// trimmed, normalized, empties dropped.
#[test]
fn extra_config_dirs_are_split_on_semicolons() {
    let paths = win();
    let listed = paths.split_list(r" D:\Work\claude-alt ;;C:/Tools/claude ; ~\.claude-extra;");
    assert_eq!(
        listed,
        vec![
            r"D:\Work\claude-alt".to_owned(),
            r"C:\Tools\claude".to_owned(),
            r"C:\Users\me\.claude-extra".to_owned(),
        ]
    );
    // The same list on a POSIX home splits on `:` instead.
    let mac = Paths::new(PathStyle::Posix, "/Users/me");
    assert_eq!(
        mac.split_list("/opt/claude-a: /opt/claude-b::~/.claude-c"),
        vec![
            "/opt/claude-a".to_owned(),
            "/opt/claude-b".to_owned(),
            "/Users/me/.claude-c".to_owned(),
        ]
    );

    // Folders named there are accounts although their names don't say so.
    let mut registry = AccountRegistry::new(paths.clone()).with_extra_config_dirs(&listed);
    let alt = identity("alt@work.dev", "u-alt");
    let mut folders: Vec<FolderFacts> = listed
        .iter()
        .map(|dir| facts(dir, Some(alt.clone())))
        .collect();
    // Home itself is never an account, even when someone lists it.
    folders.push(facts(HOME, Some(alt)));
    registry.discover(snapshot(folders, None), after(0));
    let known: BTreeSet<String> = registry
        .known_folders()
        .iter()
        .map(|f| paths.key(f.dir()))
        .collect();
    assert_eq!(
        known,
        listed
            .iter()
            .map(|dir| paths.key(dir))
            .collect::<BTreeSet<_>>()
    );
    // One login in three folders is one account with three run folders.
    let accounts = registry.accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].run_dirs.len(), 3);
}
