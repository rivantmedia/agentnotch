//! Discovery, logins, suggestions and adding folders against a throwaway
//! home folder: the first half of the Mac's AccountRegistryTests (sightings,
//! user actions and persistence are WP3's next sub-task).
//! Expected paths are built with `Home::path`, so the same file runs on
//! Windows CI with `C:\` paths and junctions.

mod accounts_support;

use accounts_support::{after, Home};
use agentnotch_engine::accounts::classify::{self, Discovery, FolderSuggestion, SuggestionReason};
use agentnotch_engine::accounts::snapshot::{has_live_session, SESSION_FILE_SLACK};
use agentnotch_engine::accounts::{AccountError, FolderMarkers};
use agentnotch_engine::core::claude_json::has_login;
use agentnotch_engine::model::{FolderKind, FolderSource};
use agentnotch_engine::platform::Liveness;
use std::collections::HashMap;
use std::time::Duration;

const T0: u64 = 0;

/// A `.claude.json` like `signIn` writes with the email only.
fn touch_config(home: &Home, relative: &str, text: &str) {
    home.write(relative, text);
}

/// Pure discovery over one read of the home folder (the Mac's
/// `AccountRegistry.discover(home:extraDirs:)`).
fn found(home: &Home, extra: &[String]) -> Discovery {
    let snapshot = home.read(extra);
    classify::discover(&snapshot, extra, &[], &home.paths)
}

/// A session registry entry for a process the fake table lists.
fn live_session(home: &Home, folder: &str, pid: u32) {
    home.processes.add(pid, 1, "claude.exe", after(T0));
    home.write(
        &format!("{folder}/sessions/{pid}.json"),
        &format!(r#"{{"pid":{pid}}}"#),
    );
}

// ---- Discovery rules ----

/// Only folders clearly in use are added by themselves: ~\.claude, and
/// look-alikes that are signed in or have session files. History alone, a
/// backup's name, ~\.config\claude and foreign folders are not.
#[test]
fn discovery_rules() {
    let home = Home::new();
    home.mkdir(".claude"); // default: always, if it exists
    home.mkdir(".claude-work/projects"); // signed in
    home.sign_in(".claude-work/.claude.json", "me@work.com", "u-work", None);
    home.mkdir(".claude_personal/sessions"); // a live session registry
    live_session(&home, ".claude_personal", 4242);
    home.mkdir(".claude-history/projects"); // history, no login, no sessions: suggest
    home.mkdir(".claude-fresh"); // only an empty .claude.json: suggest
    touch_config(&home, ".claude-fresh/.claude.json", "{}");
    home.mkdir(".claude-backup/projects"); // a backup copy, even signed in: suggest
    home.sign_in(".claude-backup/.claude.json", "me@work.com", "u-work", None);
    home.mkdir(".claude-old-2025/sessions"); // a dated copy: suggest
    home.write(".claude-old-2025/sessions/1.json", "{}");
    // Its session file names a process that runs (pid 1 always does on the
    // Mac): a backup's name still wins.
    home.processes.add(1, 0, "launchd", after(T0));
    home.mkdir(".claude-server-commander"); // an MCP server's folder: nothing
    home.write(".claude-server-commander/config.json", "{}");
    home.mkdir(".claude-mem"); // a look-alike with a config but no login: suggest at most
    touch_config(&home, ".claude-mem/.claude.json", r#"{"oauthAccount":{}}"#);
    home.mkdir(".config/claude/projects"); // XDG-style: only via a live session
    home.sign_in(".config/claude/.claude.json", "me@x.com", "u-x", None);
    home.mkdir(".claudette/projects"); // not our prefix
    home.write(".claude.json", "{}"); // a file, not a dir
    home.write(".claude-notes.json", "{}"); // a file with our prefix

    let found = found(&home, &[]);
    assert_eq!(
        found.accounts,
        home.paths_of(&[".claude", ".claude-work", ".claude_personal"])
    );
    let suggested: HashMap<String, SuggestionReason> = found
        .suggestions
        .iter()
        .map(|s| (s.config_dir.clone(), s.reason))
        .collect();
    let expected: HashMap<String, SuggestionReason> = [
        (".claude-history", SuggestionReason::Found),
        (".claude-fresh", SuggestionReason::Found),
        (".claude-mem", SuggestionReason::Found),
        (".claude-backup", SuggestionReason::LooksLikeBackup),
        (".claude-old-2025", SuggestionReason::LooksLikeBackup),
    ]
    .into_iter()
    .map(|(name, reason)| (home.path(name), reason))
    .collect();
    assert_eq!(suggested, expected);
    assert_eq!(found.suggestions.len(), 5, "each folder is suggested once");
}

#[test]
fn discovery_without_default_dir() {
    let home = Home::new();
    home.mkdir(".claude-work/sessions");
    // The registry lists a pid nobody runs, and the file is never opened.
    live_session(&home, ".claude-work", 77);
    assert_eq!(found(&home, &[]).accounts, home.paths_of(&[".claude-work"]));
}

/// A session file whose process is gone says nothing: the folder is only
/// suggested (history is there, but nobody is signed in).
#[test]
fn a_session_file_of_a_process_that_ended_does_not_add_a_folder() {
    let home = Home::new();
    home.mkdir(".claude-work/sessions");
    home.write(".claude-work/sessions/91.json", "{}");
    let found = found(&home, &[]);
    assert!(found.accounts.is_empty());
    assert_eq!(
        found.suggestions,
        vec![FolderSuggestion {
            config_dir: home.path(".claude-work"),
            reason: SuggestionReason::Found,
        }]
    );
}

#[test]
fn extra_config_dirs_are_added_but_never_home() {
    let home = Home::new();
    home.mkdir("elsewhere/claude-profile");
    let extra = home.paths_of(&["elsewhere/claude-profile", "", "missing"]);
    assert_eq!(
        found(&home, &extra).accounts,
        home.paths_of(&["elsewhere/claude-profile"])
    );
}

/// The registry made with extras adds them at its first discovery.
#[test]
fn a_registry_adds_its_extra_config_dirs() {
    let home = Home::new();
    home.mkdir("elsewhere/claude-profile");
    let extra = home.paths_of(&["elsewhere/claude-profile", ""]);
    let registry = home.registry_with(&extra);
    let dirs: Vec<&str> = registry.known_folders().iter().map(|f| f.dir()).collect();
    assert_eq!(dirs, vec![home.path("elsewhere/claude-profile")]);
}

// ---- The login byte scan ----

#[test]
fn login_detection_reads_only_for_an_account_object() {
    let home = Home::new();
    home.sign_in("a.json", "me@x.com", "u-a", None);
    home.write("b.json", r#"{"oauthAccount": null}"#);
    home.write("c.json", r#"{"oauthAccount" : { }}"#);
    home.write("d.json", r#"{"projects":{"x":{"note":"oauthAccount"}}}"#);
    home.write(
        "e.json",
        r#"{"x":1,"oauthAccount" :{"emailAddress":"a@b.c"}}"#,
    );
    let login = |name: &str| {
        std::fs::read(home.path(name))
            .map(|bytes| has_login(&bytes))
            .unwrap_or(false)
    };
    assert!(login("a.json"));
    assert!(!login("b.json"));
    assert!(!login("c.json"));
    assert!(!login("d.json"));
    assert!(login("e.json"));
    assert!(!login("missing.json"));
    assert!(!has_login(b"{}"));
}

// ---- Discovery through the registry ----

#[test]
fn discover_adds_accounts_with_stable_colours_and_suggests_the_rest() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.mkdir(".claude-work/projects");
    home.sign_in(".claude-work/.claude.json", "me@work.com", "u-work", None);
    home.mkdir(".claude-alt/sessions");
    live_session(&home, ".claude-alt", 4243);
    home.mkdir(".claude-copy/projects");

    let mut registry = home.registry();

    let folders = registry.known_folders().to_vec();
    assert_eq!(folders.len(), 3);
    assert_eq!(folders[0].dir(), home.path(".claude"));
    assert!(folders[0].is_default(&home.paths));
    assert_eq!(folders[0].config_dir_env, None);
    let mut colours: Vec<i64> = folders.iter().map(|f| f.color_index).collect();
    colours.sort_unstable();
    assert_eq!(colours, vec![0, 1, 2]);
    assert!(folders.iter().all(|f| f.source == FolderSource::Discovered));
    assert!(folders.iter().all(|f| f.kind == FolderKind::Run));
    assert_eq!(
        registry.suggestions(),
        [FolderSuggestion {
            config_dir: home.path(".claude-copy"),
            reason: SuggestionReason::LooksLikeBackup,
        }]
    );

    // A discovered custom folder runs as CLAUDE_CONFIG_DIR=<its path>; a
    // trailing separator names the same folder.
    let work = registry
        .folder(&format!("{}/", home.path(".claude-work")))
        .expect("the work folder");
    assert_eq!(work.config_dir_env, Some(home.path(".claude-work")));

    // A rescan changes nothing.
    let changed = home.discover(&mut registry);
    assert_eq!(registry.known_folders(), folders.as_slice());
    assert!(!changed.any(), "{changed:?}");

    // Accepting a suggestion adds it.
    registry
        .add_folder(&home.path(".claude-copy"))
        .expect("the suggestion is added");
    assert_eq!(
        registry
            .folder(&home.path(".claude-copy"))
            .map(|f| f.source),
        Some(FolderSource::Manual)
    );
    assert!(registry.suggestions().is_empty());
}

#[test]
fn dismissed_suggestions_stay_dismissed() {
    let home = Home::new();
    home.mkdir(".claude-history/projects");
    let mut registry = home.registry();
    let suggested: Vec<&str> = registry
        .suggestions()
        .iter()
        .map(|s| s.config_dir.as_str())
        .collect();
    assert_eq!(suggested, vec![home.path(".claude-history")]);
    registry.dismiss_suggestion(&home.path(".claude-history"));
    assert!(registry.suggestions().is_empty());
    home.save(&mut registry);

    let mut reloaded = home.registry();
    home.discover(&mut reloaded);
    assert!(reloaded.suggestions().is_empty());
    assert!(reloaded.known_folders().is_empty());
}

// ---- Identity ----

#[test]
fn identity_from_global_config() {
    let home = Home::new();
    home.mkdir(".claude-work/projects");
    home.write(
        ".claude-work/.claude.json",
        r#"{"oauthAccount":{"accountUuid":"u-1","emailAddress":"me@company.com","displayName":"Me",
          "organizationName":"Company","organizationUuid":"org-9","organizationType":"claude_team","organizationRateLimitTier":"default_claude_team"},
         "projects":{}}"#,
    );
    let mut registry = home.registry();

    let folder = registry
        .folder(&home.path(".claude-work"))
        .expect("the work folder")
        .clone();
    assert_eq!(folder.email(), Some("me@company.com"));
    assert_eq!(folder.account_uuid(), Some("u-1"));
    assert_eq!(folder.organization_name(), Some("Company"));
    assert_eq!(folder.organization_uuid(), Some("org-9"));
    assert_eq!(folder.subscription_type.as_deref(), Some("team"));
    assert_eq!(folder.rate_limit_tier(), Some("default_claude_team"));
    assert_eq!(folder.label(&home.paths), "Claude Company");
    let accounts = registry.accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "Claude Company");
    assert_eq!(accounts[0].email.as_deref(), Some("me@company.com"));
    assert_eq!(accounts[0].organization_uuid.as_deref(), Some("org-9"));

    // Signing out clears it (a different length: the reader's cache is keyed
    // by modification time and size).
    home.write(
        ".claude-work/.claude.json",
        r#"{"projects":{},"numStartups":3}"#,
    );
    home.reader.invalidate();
    home.discover(&mut registry);
    let signed_out = registry
        .folder(&home.path(".claude-work"))
        .expect("the work folder");
    assert_eq!(signed_out.email(), None);
    assert_eq!(signed_out.account_uuid(), None);
    assert_eq!(signed_out.organization_uuid(), None);
    assert_eq!(signed_out.subscription_type, None);
    assert_eq!(signed_out.label(&home.paths), "Claude (work)");
}

/// `~\.claude` is signed in through `~\.claude.json`, beside it.
#[test]
fn the_default_folders_login_is_read_from_the_home_claude_json() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.sign_in(".claude.json", "me@x.com", "u-def", None);
    let registry = home.registry();
    let default = registry
        .folder(&home.path(".claude"))
        .expect("the default folder");
    assert_eq!(default.email(), Some("me@x.com"));
    assert_eq!(default.account_uuid(), Some("u-def"));
}

#[test]
fn default_account_reads_home_claude_json() {
    let home = Home::new();
    let paths = &home.paths;
    let default = home.path(".claude");
    assert_eq!(
        paths.global_config_file(&default, None),
        home.path(".claude.json")
    );
    assert_eq!(
        paths.global_config_file(&default, Some(&default)),
        home.path(".claude/.claude.json")
    );
    let work = home.path(".claude-work");
    let spelled = format!("{work}/");
    assert_eq!(
        paths.global_config_file(&work, Some(&spelled)),
        home.path(".claude-work/.claude.json")
    );
}

#[test]
fn home_is_never_an_account() {
    let home = Home::new();
    let mut registry = home.registry();
    let root = std::path::Path::new(&home.home())
        .ancestors()
        .last()
        .expect("a root")
        .to_string_lossy()
        .into_owned();
    registry.record(home.sighting("", Some(&home.home()), after(1)), after(1));
    registry.record(
        agentnotch_engine::model::AccountSighting {
            config_dir: agentnotch_engine::model::AccountId::new(root.clone()),
            config_dir_env: Some(root),
            session_id: agentnotch_engine::model::SessionId::new("s2"),
            at: after(1),
        },
        after(1),
    );
    assert!(registry.known_folders().is_empty());
    assert!(registry.accounts().is_empty());
}

// ---- Adding folders ----

#[test]
fn add_folder_refuses_home_and_its_parents() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.write(
        ".claude.json",
        r#"{"oauthAccount":{"emailAddress":"me@x.com"}}"#,
    );
    let mut registry = home.registry();
    let refused = |registry: &mut agentnotch_engine::accounts::AccountRegistry, path: &str| {
        registry.add_folder(path).expect_err(path)
    };
    let parent = home.paths.parent(&home.home()).expect("home has a parent");
    let root = std::path::Path::new(&home.home())
        .ancestors()
        .last()
        .expect("a root")
        .to_string_lossy()
        .into_owned();

    assert_eq!(
        refused(&mut registry, &home.home()),
        AccountError::HomeFolder
    );
    assert_eq!(
        refused(&mut registry, &format!("{}/", home.home())),
        AccountError::HomeFolder
    );
    assert_eq!(
        refused(&mut registry, &parent),
        AccountError::ContainsAccounts
    );
    assert_eq!(
        refused(&mut registry, &root),
        AccountError::ContainsAccounts
    );
    assert_eq!(
        refused(&mut registry, &home.path("nope")),
        AccountError::Missing
    );
    assert_eq!(
        refused(&mut registry, &home.path(".claude.json")),
        AccountError::NotAFolder
    );
    // ~\.claude was found when the registry was made (it discovers at once);
    // inside it is inside that account.
    assert_eq!(
        refused(&mut registry, &home.path(".claude/projects")),
        AccountError::InsideAccount("Claude X".to_owned())
    );
    let dirs: Vec<&str> = registry.known_folders().iter().map(|f| f.dir()).collect();
    assert_eq!(dirs, vec![home.path(".claude")]);
}

/// A link to the home folder is the home folder.
#[test]
fn a_link_to_home_is_home() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.link_dir("homelink", "");
    let mut registry = home.registry();
    assert_eq!(
        registry.add_folder(&home.path("homelink")),
        Err(AccountError::HomeFolder)
    );
    assert!(registry.check_folder(&home.path("homelink")).is_err());
}

/// A `~\.claude-x` that is a link to the home folder looks signed in (it
/// shows `~\.claude.json` as its own), but it is the home folder: discovery
/// never adds or suggests it. A session can name it before any read of the
/// disk has looked; the read then drops it, and later sightings leave it out.
#[test]
fn a_link_to_home_is_never_discovered_and_a_sighted_one_is_dropped_for_good() {
    let home = Home::new();
    home.mkdir(".claude/projects");
    home.sign_in(".claude.json", "me@x.com", "u-def", None);
    home.link_dir(".claude-loop", "");
    let looped = home.path(".claude-loop");
    let dirs = |registry: &agentnotch_engine::accounts::AccountRegistry| -> Vec<String> {
        registry
            .known_folders()
            .iter()
            .map(|f| f.dir().to_owned())
            .collect()
    };

    let mut registry = home.registry();
    assert_eq!(dirs(&registry), home.paths_of(&[".claude"]));
    assert!(registry.suggestions().is_empty());
    // Known to be the home folder already: a session naming it adds nothing.
    let changed = registry.record(
        home.sighting(".claude-loop", Some(&looped), after(1)),
        after(1),
    );
    assert!(!changed.any(), "{changed:?}");
    assert_eq!(dirs(&registry), home.paths_of(&[".claude"]));

    // A registry that hears of it from a session first takes it at its word,
    // asks for a read, and drops it when the read shows what it is.
    let mut fresh = home.bare_registry(&[]);
    let heard = fresh.record(
        home.sighting(".claude-loop", Some(&looped), after(1)),
        after(1),
    );
    assert_eq!(heard.new_run_folders, vec![home.id(".claude-loop")]);
    assert!(fresh.needs_discovery());
    let read = home.discover_at(&mut fresh, after(2));
    assert_eq!(read.removed_folders, vec![home.id(".claude-loop")]);
    assert_eq!(dirs(&fresh), home.paths_of(&[".claude"]));
    assert!(!fresh.needs_discovery());
    let again = fresh.record(
        home.sighting(".claude-loop", Some(&looped), after(100)),
        after(100),
    );
    assert!(!again.any(), "{again:?}");
    assert_eq!(dirs(&fresh), home.paths_of(&[".claude"]));
    // It is not remembered as something the user forgot.
    assert!(!fresh.is_forgotten(&looped));
    assert!(fresh.suggestions().is_empty());

    // Once the link points at a folder of its own, it is a folder like any other.
    home.remove_link(".claude-loop");
    home.mkdir(".claude-loop/projects");
    home.sign_in(".claude-loop/.claude.json", "me@loop.io", "u-loop", None);
    home.discover_at(&mut fresh, after(200));
    assert_eq!(dirs(&fresh).len(), 2);
    assert!(fresh.folder(&looped).is_some());
}

/// A link to a folder that contains home holds every account too.
#[test]
fn a_link_to_a_folder_containing_home_is_refused() {
    let home = Home::new();
    let parent = home.paths.parent(&home.home()).expect("home has a parent");
    let link = home.path("upwards");
    std::fs::create_dir_all(home.path("")).expect("home");
    home.link_to("upwards", &parent);
    let mut registry = home.registry();
    assert_eq!(
        registry.add_folder(&link),
        Err(AccountError::ContainsAccounts)
    );
}

#[test]
fn add_folder_refuses_a_folder_inside_another_account() {
    let home = Home::new();
    home.mkdir(".claude-work/projects");
    home.sign_in(
        ".claude-work/.claude.json",
        "me@work.com",
        "u-work",
        Some("Work"),
    );
    home.mkdir(".claude-work/hooks");
    let mut registry = home.registry();
    let label = registry
        .folder(&home.path(".claude-work"))
        .expect("the work folder")
        .label(&home.paths);
    assert_eq!(
        registry.add_folder(&home.path(".claude-work/hooks")),
        Err(AccountError::InsideAccount(label))
    );
}

#[test]
fn add_folder_refuses_the_shared_history_and_the_windows_folder() {
    let home = Home::new();
    home.mkdir(".claude-shared/projects");
    home.mkdir(".claude-windows");
    let mut registry = home.registry();
    for folder in [".claude-shared", ".claude-windows"] {
        assert_eq!(
            registry.add_folder(&home.path(folder)),
            Err(AccountError::Infrastructure),
            "{folder}"
        );
    }
    assert!(registry.known_folders().is_empty());
}

#[test]
fn a_registry_without_a_probe_cannot_check_folders() {
    let home = Home::new();
    home.mkdir("somewhere");
    let mut registry = agentnotch_engine::accounts::AccountRegistry::new(home.paths.clone());
    assert_eq!(
        registry.add_folder(&home.path("somewhere")),
        Err(AccountError::Unavailable)
    );
}

#[test]
fn the_refusals_say_what_is_wrong() {
    let home = Home::new();
    let paths = &home.paths;
    let example = paths.abbreviate(&home.path(".claude-work"));
    assert_eq!(
        AccountError::Missing.message(paths),
        "That folder doesn't exist."
    );
    assert_eq!(
        AccountError::NotAFolder.message(paths),
        "That's a file, not a folder."
    );
    assert_eq!(
        AccountError::HomeFolder.message(paths),
        format!("That's your home folder, not a Claude Code config folder. Pick a folder like {example}.")
    );
    assert_eq!(
        AccountError::ContainsAccounts.message(paths),
        format!("That folder contains your home folder. Pick a Claude Code config folder like {example}.")
    );
    assert_eq!(
        AccountError::InsideAccount("Claude X".to_owned()).message(paths),
        "That folder is inside Claude X's config folder. Pick the config folder itself."
    );
    assert!(AccountError::Infrastructure
        .message(paths)
        .starts_with("That folder is Claude Parallel Profiles' shared history"));
}

#[test]
fn folder_markers_say_whether_to_ask() {
    let home = Home::new();
    home.mkdir(".claude-a/projects");
    home.mkdir(".claude-b");
    home.write(".claude-b/.claude.json", "{}");
    home.mkdir("somewhere");
    let registry = home.registry();
    let a = registry
        .check_folder(&home.path(".claude-a"))
        .expect("a folder");
    assert!(a.is_clearly_config_dir());
    let only_config = registry
        .check_folder(&home.path(".claude-b"))
        .expect("a folder");
    assert!(only_config.has_global_config && !only_config.is_clearly_config_dir());
    assert_eq!(
        registry
            .check_folder(&home.path("somewhere"))
            .expect("a folder"),
        FolderMarkers::default()
    );
}

// ---- has_live_session ----

#[test]
fn a_live_session_needs_a_positive_pid_the_table_lists() {
    let home = Home::new();
    let paths = &home.paths;
    let dir = home.path(".claude-a");
    home.mkdir(".claude-a/sessions");
    assert!(!has_live_session(paths, &dir, home.processes.as_ref()));

    // Not listed, not a pid, not a session file: nothing.
    home.write(".claude-a/sessions/500.json", "not even json");
    home.write(".claude-a/sessions/abc.json", "{}");
    home.write(".claude-a/sessions/-7.json", "{}");
    home.write(".claude-a/sessions/0.json", "{}");
    home.write(".claude-a/sessions/600.key", "secret");
    home.processes.add(0, 0, "system", after(T0));
    home.processes.add(600, 1, "claude.exe", after(T0));
    assert!(!has_live_session(paths, &dir, home.processes.as_ref()));

    // The process runs: the file is never opened, so its content is moot.
    home.processes.add(500, 1, "claude.exe", after(T0));
    assert!(has_live_session(paths, &dir, home.processes.as_ref()));
    home.processes.remove(500);
    assert!(!has_live_session(paths, &dir, home.processes.as_ref()));
}

/// Windows hands a pid out again soon after its process ends. A process
/// writes its own session file, so a file last written before the pid's
/// process started was left by an earlier process: no live session.
#[test]
fn a_session_file_older_than_its_pids_process_is_a_leftover() {
    let home = Home::new();
    let paths = &home.paths;
    let dir = home.path(".claude-work");
    home.write(".claude-work/sessions/700.json", "{}");
    let written = std::fs::metadata(home.path(".claude-work/sessions/700.json"))
        .and_then(|meta| meta.modified())
        .expect("the session file's time");
    let second = Duration::from_secs(1);

    // Started before the file was written (every real session), or no later
    // than the two clocks can differ: its own file.
    for started in [
        written - Duration::from_secs(3600),
        written,
        written + SESSION_FILE_SLACK,
    ] {
        home.processes.add(700, 1, "claude.exe", started);
        assert!(
            has_live_session(paths, &dir, home.processes.as_ref()),
            "{started:?} against {written:?}"
        );
    }
    // Started after the file's last write: the pid is another process's now.
    home.processes
        .add(700, 1, "notepad.exe", written + SESSION_FILE_SLACK + second);
    assert!(!has_live_session(paths, &dir, home.processes.as_ref()));
    // So discovery only suggests the folder (history, nobody signed in).
    let discovery = found(&home, &[]);
    assert!(discovery.accounts.is_empty(), "{:?}", discovery.accounts);
    assert_eq!(
        discovery.suggestions,
        vec![FolderSuggestion {
            config_dir: dir.clone(),
            reason: SuggestionReason::Found,
        }]
    );

    // A process whose start time can't be read (another user's, say) is
    // taken at its word, as before.
    home.processes.remove(700);
    assert!(!has_live_session(paths, &dir, home.processes.as_ref()));
    home.processes.set_liveness(700, Liveness::Alive);
    assert!(has_live_session(paths, &dir, home.processes.as_ref()));
    home.processes.clear_liveness(700);

    // One leftover beside a session that does run: the folder has a live one.
    home.processes
        .add(700, 1, "notepad.exe", written + Duration::from_secs(60));
    home.write(".claude-work/sessions/701.json", "{}");
    home.processes.add(701, 1, "claude.exe", after(T0));
    assert!(has_live_session(paths, &dir, home.processes.as_ref()));
    assert_eq!(found(&home, &[]).accounts, home.paths_of(&[".claude-work"]));
}

#[test]
fn a_linked_sessions_folder_says_nothing_about_the_folder() {
    let home = Home::new();
    home.mkdir(".claude-shared/sessions");
    live_session(&home, ".claude-shared", 4300);
    home.mkdir(".claude-a");
    home.link_dir(".claude-a/sessions", ".claude-shared/sessions");
    // The registry's own folder holds the same file: it is that one's.
    home.mkdir(".claude-b/sessions");
    home.write(".claude-b/sessions/4300.json", "{}");

    let snapshot = home.read(&[]);
    let facts = |name: &str| {
        classify::find(&snapshot, &home.paths, &home.path(name))
            .unwrap_or_else(|| panic!("{name} is in the snapshot"))
            .clone()
    };
    let shared_link = facts(".claude-a");
    assert!(shared_link.has_sessions);
    assert!(!shared_link.has_live_session);
    assert!(facts(".claude-b").has_live_session);
}
