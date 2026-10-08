//! The settings snapshot and the setup state (`hub::project_settings`, WP7):
//! ports of PP_HooksTests.theConsentCardSaysWhereInPlainWords and
//! PPFix_SettingsCopyTests (the copy the engine produces: consent files,
//! folder names, summaries), the Hooks switch's summary, the usage lines, and
//! the Windows additions: the setup state read through `absorb_status` before
//! any consent, `install_disabled` for sealed and `--no-install` runs, a status
//! line left alone with its reason, the Desktop cache format caption, and the
//! shortcut's report. The registry and the hook manager are the real stores,
//! over Windows paths and hand-built discovery reads; nothing touches a disk.

mod hub_support;

use agentnotch_engine::accounts::AccountRegistry;
use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::hooks::HookManager;
use agentnotch_engine::hub::project_settings::{
    join_list, scope_message, usage_check_caption, usage_line, ConsentScope, FOOTNOTES,
    INTERVAL_OPTIONS,
};
use agentnotch_engine::model::{
    AccountId, AccountUsage, ConfigRead, DesktopCacheFormat, FolderFacts, FolderSnapshot, Identity,
    IdentityId, UsageSource, UsageWindow,
};
use agentnotch_engine::platform::{Expect, NotifyPermission};
use agentnotch_engine::runtime_types::{
    CommandForm, FolderHookStatus, InstallChange, InstallOutcome, InstallPlan, RingReading,
    StatusLineIntent, VersionSighting, VersionSource,
};
use agentnotch_engine::usage::desktop::UNSUPPORTED_FORMAT_TEXT;
use hub_support::{at, now, SettingsWorld};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HOME: &str = r"C:\Users\me";
const MAIN: &str = r"C:\Users\me\.claude";
const WORK: &str = r"C:\Users\me\.claude-work";
const OLD: &str = r"C:\Users\me\.claude-old";

fn paths() -> Paths {
    Paths::new(PathStyle::Windows, HOME)
}

fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

fn identity(email: &str, uuid: &str, tier: Option<&str>) -> Identity {
    Identity {
        account_uuid: Some(uuid.to_owned()),
        email: Some(email.to_owned()),
        rate_limit_tier: tier.map(str::to_owned),
        ..Identity::default()
    }
}

fn read_of(login: Option<Identity>) -> ConfigRead {
    ConfigRead {
        identity: login,
        modified_at: Some(t0()),
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
        own_config: login.map(|login| read_of(Some(login))),
        canonical: Some(path.to_owned()),
        ..FolderFacts::default()
    }
}

fn snapshot(folders: Vec<FolderFacts>, home_login: Option<Identity>) -> FolderSnapshot {
    FolderSnapshot {
        home: HOME.to_owned(),
        home_config: home_login.map(|login| read_of(Some(login))),
        home_canonical: Some(HOME.to_owned()),
        requested: folders.iter().map(|f| f.path.clone()).collect(),
        folders,
        ..FolderSnapshot::default()
    }
}

fn personal() -> Identity {
    identity(
        "me@personal.example",
        "u-personal",
        Some("default_claude_max_5x"),
    )
}

fn work() -> Identity {
    identity("me@work.example", "u-work", None)
}

/// `~\.claude` (Personal) and `~\.claude-work` (Work), as discovery finds them.
fn two_accounts() -> AccountRegistry {
    let mut registry = AccountRegistry::new(paths());
    registry.discover(
        snapshot(
            vec![facts(MAIN, Some(personal())), facts(WORK, Some(work()))],
            Some(personal()),
        ),
        t0(),
    );
    registry
}

fn world() -> SettingsWorld {
    SettingsWorld::new(two_accounts(), HookManager::new())
}

fn folder_id(path: &str) -> AccountId {
    AccountId::new(paths().normalize(path))
}

/// What a folder's settings.json says once our hooks are in.
fn installed(status_line: bool) -> FolderHookStatus {
    FolderHookStatus {
        config_dir_exists: true,
        settings_readable: true,
        hooks_registered: true,
        hooks_installed: true,
        status_line_installed: status_line,
        ..FolderHookStatus::default()
    }
}

fn bare() -> FolderHookStatus {
    FolderHookStatus {
        config_dir_exists: true,
        settings_readable: true,
        ..FolderHookStatus::default()
    }
}

fn read(world: &mut SettingsWorld, path: &str, status: FolderHookStatus) {
    world
        .hooks
        .absorb_status(&folder_id(path), status, &world.settings);
}

fn plan_for(path: &str, form: CommandForm) -> InstallPlan {
    InstallPlan {
        folder: folder_id(path),
        settings_path: PathBuf::from(format!(r"{path}\settings.json")),
        expected: Expect::Absent,
        existed: false,
        hook_copy: None,
        form,
        events: Vec::new(),
        status_line: StatusLineIntent::Nothing,
        remove_only: false,
    }
}

fn outcome_of(plan: &InstallPlan, result: Result<InstallChange, String>) -> InstallOutcome {
    InstallOutcome {
        folder: plan.folder.clone(),
        settings_path: plan.settings_path.clone(),
        result,
        backup: None,
        entry: None,
        status_line: None,
    }
}

fn consent(world: &mut SettingsWorld) {
    world.settings.hook_consent = Some(true);
    world.settings.hooks_enabled = true;
}

fn account<'a>(
    snapshot: &'a agentnotch_engine::model::SettingsSnapshot,
    email: &str,
) -> &'a agentnotch_engine::model::AccountRow {
    snapshot
        .accounts
        .iter()
        .find(|row| row.identity_line.starts_with(email))
        .unwrap_or_else(|| panic!("no account row for {email}"))
}

fn usage(five: f64, seven: f64, age: u64) -> AccountUsage {
    let mut usage = AccountUsage::new(IdentityId::from("x"), UsageSource::Probe, at(-(age as i64)));
    usage.five_hour = Some(UsageWindow::new(
        five,
        Some(at(3600)),
        UsageWindow::SESSION_DURATION_S,
    ));
    usage.seven_day = Some(UsageWindow::new(
        seven,
        Some(at(3 * 86_400)),
        UsageWindow::WEEKLY_DURATION_S,
    ));
    usage
}

fn reading(usage: AccountUsage, stale: bool) -> RingReading {
    RingReading::Reading {
        stale_after: usage.updated_at + Duration::from_secs(900),
        usage,
        status: if stale {
            agentnotch_engine::model::RingStatus::Stale
        } else {
            agentnotch_engine::model::RingStatus::Ok
        },
    }
}

// ---- PP_HooksTests.theConsentCardSaysWhereInPlainWords ----

#[test]
fn the_consent_card_says_where_in_plain_words() {
    let scope = ConsentScope {
        includes_default: true,
        window_count: 3,
        store_count: 3,
        parallel_profiles: true,
        ..ConsentScope::default()
    };
    assert_eq!(scope.folder_count(), 4);
    assert_eq!(
        scope.sentence().as_deref(),
        Some("Installs into ~\\.claude and your VS Code workspaces' folders (3 now; new ones are set up automatically). Claude Parallel Profiles' account stores never get hooks.")
    );
    let plain = ConsentScope {
        includes_default: true,
        ..ConsentScope::default()
    };
    assert_eq!(plain.sentence(), None);
}

#[test]
fn standalone_folders_join_the_sentence() {
    let scope = ConsentScope {
        includes_default: true,
        window_count: 1,
        standalone_folders: vec!["~\\.claude-work".into()],
        parallel_profiles: true,
        ..ConsentScope::default()
    };
    assert_eq!(scope.folder_count(), 3);
    let sentence = scope.sentence().unwrap();
    assert!(
        sentence.contains("~\\.claude, your VS Code workspaces' folders (1 now; new ones are set up automatically), and ~\\.claude-work."),
        "{sentence}"
    );
}

#[test]
fn the_scope_of_what_turn_on_covers_comes_from_the_tracked_run_folders() {
    let world = world();
    let accounts = world.registry.accounts();
    let folders = world.registry.folders();
    let scope = ConsentScope::of(&world.registry, &accounts, &folders);
    assert!(scope.includes_default);
    assert_eq!(scope.window_count, 0);
    assert_eq!(scope.standalone_folders, vec![r"~\.claude-work".to_owned()]);
    assert_eq!(scope.folder_count(), 2);
    // No extension on this PC: the sentence is left out.
    assert_eq!(scope.sentence(), None);
}

// ---- PPFix_SettingsCopyTests ----

#[test]
fn the_scope_notice_names_the_folders_it_adds() {
    assert!(scope_message(3).contains("3 VS Code workspaces' folders"));
    assert!(scope_message(1).contains("1 VS Code workspace's folder"));
    assert!(scope_message(3).contains("Account stores never get hooks."));
    let scope = ConsentScope {
        includes_default: true,
        window_count: 3,
        parallel_profiles: true,
        ..ConsentScope::default()
    };
    let sentence = scope.sentence().unwrap();
    assert!(sentence.contains("never get hooks") && !sentence.contains("untouched"));
}

#[test]
fn lists_read_the_way_foundation_formats_them() {
    let items = |texts: &[&str]| texts.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
    assert_eq!(join_list(&items(&["a"])), "a");
    assert_eq!(join_list(&items(&["a", "b"])), "a and b");
    assert_eq!(join_list(&items(&["a", "b", "c"])), "a, b, and c");
}

#[test]
fn the_consent_card_lists_each_file_with_whose_it_is() {
    let world = world();
    let setup = world.setup();
    assert_eq!(setup.hook_consent, None);
    assert!(setup.needs_hook_consent);
    let files: Vec<(String, Option<String>)> = setup
        .consent_files
        .iter()
        .map(|file| (file.path.clone(), file.account.clone()))
        .collect();
    assert_eq!(
        files,
        vec![
            (
                r"%USERPROFILE%\.claude\settings.json".to_owned(),
                Some("me@personal.example".to_owned())
            ),
            (
                r"%USERPROFILE%\.claude-work\settings.json".to_owned(),
                Some("me@work.example".to_owned())
            ),
        ]
    );
}

#[test]
fn workspace_folders_are_named_where_they_are_listed() {
    // A VS Code window's working copy, once stocked, runs as an account.
    let window = r"C:\Users\me\.claude-windows\801f9dd51396";
    let mut registry = AccountRegistry::new(paths());
    registry.discover(
        snapshot(
            vec![
                facts(MAIN, Some(personal())),
                facts(window, Some(personal())),
            ],
            Some(personal()),
        ),
        t0(),
    );
    let mut world = SettingsWorld::new(registry, HookManager::new());
    world.window_names.insert(
        folder_id(window).to_string(),
        "superpowered-vibe-notch".into(),
    );
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    let titles: Vec<(&str, &str)> = row
        .folders
        .iter()
        .map(|f| (f.title.as_str(), f.role.as_str()))
        .collect();
    assert!(
        titles.contains(&(
            "VS Code · superpowered-vibe-notch",
            r"~\.claude-windows\801f9dd51396"
        )),
        "{titles:?}"
    );
    assert!(titles.contains(&(r"~\.claude", "Default")), "{titles:?}");
    assert_eq!(
        row.folder_summary,
        "Runs in ~\\.claude and 1 VS Code workspace"
    );

    // Unnamed: the path leads and the role says what it is.
    world.window_names.clear();
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert!(row.folders.iter().any(|f| {
        f.title == r"~\.claude-windows\801f9dd51396" && f.role == "VS Code workspace"
    }));

    // The last-change notice names a workspace by its project too.
    world.window_names.insert(
        folder_id(window).to_string(),
        "superpowered-vibe-notch".into(),
    );
    world.changed = vec![folder_id(window)];
    let notice = world.snapshot().hooks.last_change.unwrap();
    assert!(
        notice.contains("(VS Code · superpowered-vibe-notch)"),
        "{notice}"
    );
}

#[test]
fn prompts_wait_where_claude_code_runs() {
    let mut world = world();
    consent(&mut world);
    read(&mut world, MAIN, bare());
    read(&mut world, WORK, bare());
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert!(row
        .hook_problem
        .as_deref()
        .unwrap()
        .contains("VS Code or the terminal"));
    // The panel's banner says the same for the accounts without hooks.
    assert_eq!(snapshot.setup.missing_hooks_accounts.len(), 2);
}

// ---- the setup state ----

#[test]
fn the_setup_state_lists_the_official_apps_folders_before_any_consent() {
    let mut world = world();
    assert_eq!(world.settings.hook_consent, None);
    // Read at launch through `absorb_status`, before the user answers.
    let mut official = bare();
    official.codenotch_hooks = true;
    read(&mut world, MAIN, official);
    read(&mut world, WORK, bare());
    let setup = world.setup();
    assert!(setup.needs_hook_consent);
    assert_eq!(
        setup.codenotch_hooks_folders,
        vec![folder_id(MAIN).to_string()]
    );
    // The same string goes back in `remove_codenotch_hooks`, so it is the id.
    let snapshot = world.snapshot();
    let main = account(&snapshot, "me@personal.example");
    assert!(main.folders[0].codenotch_hooks);
    assert!(!account(&snapshot, "me@work.example").folders[0].codenotch_hooks);
    assert_eq!(snapshot.setup, setup);
}

#[test]
fn install_is_disabled_when_sealed_or_with_no_install() {
    let world = world();
    assert!(!world.setup().install_disabled);
    assert!(world.snapshot().hooks.install_allowed);

    let mut no_install = SettingsWorld::new(
        two_accounts(),
        HookManager::configured(
            PathBuf::from("agentnotch-hook.exe"),
            &DevFlags {
                no_install: true,
                ..DevFlags::default()
            },
        ),
    );
    consent(&mut no_install);
    let snapshot = no_install.snapshot();
    assert!(snapshot.setup.install_disabled);
    assert!(!snapshot.hooks.install_allowed && snapshot.hooks.enabled_locked);
    assert_eq!(
        snapshot.hooks.summary,
        "Installing is off for this run (--no-install)."
    );
    assert!(snapshot.accounts.iter().all(|row| !row.can_install));
    // The banner about missing hooks has nothing to offer in such a run.
    assert!(snapshot.setup.missing_hooks_accounts.is_empty());

    let mut sealed = SettingsWorld::new(
        two_accounts(),
        HookManager::configured(
            PathBuf::from("agentnotch-hook.exe"),
            &DevFlags {
                sealed: true,
                ..DevFlags::default()
            },
        ),
    );
    sealed.sealed = true;
    sealed.settings.hook_consent = Some(true);
    // Sealed: the Mac's empty setup state (no banner, no card); the pane
    // says nothing is installed.
    let setup = sealed.setup();
    assert!(!setup.install_disabled && !setup.needs_hook_consent);
    assert!(setup.consent_files.is_empty() && setup.codenotch_hooks_folders.is_empty());
    let pane = sealed.snapshot();
    assert!(pane.sealed && !pane.hooks.install_allowed && pane.hooks.enabled_locked);
    assert!(pane.accounts.iter().all(|row| !row.can_install));
}

#[test]
fn the_setup_state_follows_the_consent_answer() {
    let mut world = world();
    assert!(world.setup().needs_hook_consent && !world.setup().control_off);
    world.settings.hook_consent = Some(false);
    let declined = world.setup();
    assert_eq!(declined.hook_consent, Some(false));
    assert!(!declined.needs_hook_consent && !declined.control_off);

    consent(&mut world);
    world.settings.hooks_enabled = false;
    let off = world.setup();
    assert!(off.control_off && !off.needs_hook_consent);
    assert!(
        off.missing_hooks_accounts.is_empty(),
        "nothing is missing while it is off"
    );
}

#[test]
fn a_transport_error_is_carried() {
    let mut world = world();
    world.transport_error = Some("The hook pipe couldn't be opened (access denied).".into());
    assert_eq!(
        world.setup().transport_error.as_deref(),
        Some("The hook pipe couldn't be opened (access denied).")
    );
}

#[test]
fn a_yes_given_before_workspaces_were_covered_is_said_once() {
    let window = r"C:\Users\me\.claude-windows\5d1e0a7b3c21";
    let mut registry = AccountRegistry::new(paths());
    registry.discover(
        snapshot(
            vec![
                facts(MAIN, Some(personal())),
                facts(window, Some(personal())),
            ],
            Some(personal()),
        ),
        t0(),
    );
    let mut world = SettingsWorld::new(registry, HookManager::new());
    consent(&mut world);
    // A yes given by an earlier build (scope 0) did not cover the windows.
    world.settings.hook_consent_scope = 0;
    let setup = world.setup();
    assert_eq!(
        setup.new_install_folders,
        vec![r"~\.claude-windows\5d1e0a7b3c21".to_owned()]
    );
    // Named by its project once a session told.
    world
        .window_names
        .insert(folder_id(window).to_string(), "dotfiles".into());
    assert_eq!(
        world.setup().new_install_folders,
        vec!["VS Code · dotfiles"]
    );
    // Acknowledged (the scope is current again): gone. Likewise with hooks off.
    world.settings.hook_consent_scope = 2;
    assert!(world.setup().new_install_folders.is_empty());
    world.settings.hook_consent_scope = 0;
    world.settings.hooks_enabled = false;
    assert!(world.setup().new_install_folders.is_empty());
}

// ---- the accounts ----

#[test]
fn an_account_row_says_who_where_and_how_it_is_hooked() {
    let mut world = world();
    world.settings.hook_consent = Some(true);
    read(&mut world, MAIN, installed(true));
    read(&mut world, WORK, installed(false));
    let snapshot = world.snapshot();
    assert_eq!(snapshot.accounts.len(), 2);

    let main = account(&snapshot, "me@personal.example");
    assert_eq!(main.identity_line, "me@personal.example · Max 5x");
    assert_eq!(main.folder_summary, r"Runs in ~\.claude");
    assert!(main.is_default && main.is_tracked && main.ring_shown && main.is_signed_in);
    assert_eq!(main.hook_state, "Hooks installed");
    assert_eq!(main.hook_state_tone, "ok");
    assert!(main.live_status_line);
    assert_eq!(main.hook_problem, None);
    assert!(main.can_install);
    assert_eq!(main.install_label, "Reinstall hooks");
    assert_eq!(main.launch_command.as_deref(), Some("claude"));
    assert!(!main.can_forget);
    assert_eq!(main.folders.len(), 1);
    assert_eq!(main.folders[0].role, "Default");
    assert_eq!(main.folders[0].title, r"~\.claude");
    assert_eq!(main.folders[0].state, "Hooks and live status line");
    assert_eq!(main.ring_id, main.ring_id.to_lowercase());
    assert!(main.ring_id.starts_with("claude-acct-"));

    let other = account(&snapshot, "me@work.example");
    assert!(!other.is_default && other.can_forget);
    assert!(!other.live_status_line);
    assert_eq!(other.folders[0].role, "Folder");
    assert_eq!(other.folders[0].state, "Hooks installed");
    assert_eq!(
        other.launch_command.as_deref(),
        Some(r"$env:CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work'; claude")
    );
    assert_eq!(
        other.forget_caption,
        "This app's hooks are removed from its settings.json and it stops being tracked. The folder and its sessions are left alone."
    );
}

#[test]
fn names_have_a_default_and_a_custom_form() {
    let mut registry = two_accounts();
    let work = registry
        .identities()
        .iter()
        .find(|i| i.email.as_deref() == Some("me@work.example"))
        .unwrap()
        .id
        .to_string();
    registry.rename(&work, Some("Lab"));
    let world = SettingsWorld::new(registry, HookManager::new());
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@work.example");
    assert_eq!(row.label, "Lab");
    assert!(row.has_custom_label);
    assert_ne!(row.default_label, "Lab");
    let main = account(&snapshot, "me@personal.example");
    assert!(!main.has_custom_label);
    assert_eq!(main.label, main.default_label);
}

#[test]
fn the_hook_chip_says_what_is_true() {
    let mut world = world();
    // Before consent: off, not a problem, and no install button.
    read(&mut world, MAIN, bare());
    read(&mut world, WORK, bare());
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.hook_state, "Hooks off");
    assert_eq!(row.hook_state_tone, "neutral");
    assert_eq!(row.hook_problem, None);
    assert!(!row.can_install);

    // Consent given, not in place: amber, with what the user loses.
    consent(&mut world);
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.hook_state, "Hooks not installed");
    assert_eq!(row.hook_state_tone, "warning");
    assert_eq!(row.install_label, "Install hooks");
    assert!(row.can_install && !row.hook_problem_critical);

    // A pass that failed says why (the outcome is the manager's to keep).
    read(&mut world, MAIN, bare());
    let plan = plan_for(MAIN, CommandForm::Text("x".into()));
    world.hooks.absorb_outcomes(
        std::slice::from_ref(&plan),
        &[outcome_of(&plan, Err("settings.json is read-only".into()))],
    );
    assert_eq!(
        account(&world.snapshot(), "me@personal.example")
            .hook_problem
            .as_deref(),
        Some("settings.json is read-only")
    );

    // A folder no hook command can run from says that.
    let why =
        "Can't be hooked here: its path needs Claude Code 2.1.101 or later everywhere on this PC";
    let plan = plan_for(MAIN, CommandForm::NotPossible(why.into()));
    world.hooks.absorb_outcomes(
        std::slice::from_ref(&plan),
        &[outcome_of(&plan, Err(why.into()))],
    );
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert!(row
        .hook_problem
        .as_deref()
        .unwrap()
        .starts_with("Can't be hooked here"));
    assert_eq!(row.folders[0].not_hookable.as_deref(), Some(why));

    // settings.json that doesn't parse is critical; a missing folder neutral.
    let unreadable = FolderHookStatus {
        config_dir_exists: true,
        settings_readable: false,
        ..FolderHookStatus::default()
    };
    read(&mut world, MAIN, unreadable);
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.hook_state, "settings.json unreadable");
    assert_eq!(row.hook_state_tone, "critical");
    assert!(row.hook_problem_critical);
    assert_eq!(row.folders[0].state, "settings.json unreadable");

    // One another program holds is busy, not broken: it says so instead of
    // "isn't valid JSON", and installing stays offered for a retry.
    let held = FolderHookStatus {
        config_dir_exists: true,
        settings_readable: false,
        settings_in_use: true,
        ..FolderHookStatus::default()
    };
    read(&mut world, MAIN, held);
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.hook_state, "settings.json in use");
    assert_eq!(row.hook_state_tone, "warning");
    assert!(!row.hook_problem_critical);
    assert_eq!(
        row.hook_problem.as_deref(),
        Some(agentnotch_engine::hooks::apply::IN_USE)
    );
    assert!(row.can_install);
    assert_eq!(row.folders[0].state, "settings.json in use");

    read(&mut world, MAIN, FolderHookStatus::default());
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.hook_state, "Folder missing");
    assert!(!row.can_install, "nothing to install into");
    assert_eq!(row.folders[0].state, "Folder missing");
}

#[test]
fn a_folder_not_read_yet_says_so() {
    let mut world = world();
    consent(&mut world);
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.folders[0].state, "Checking…");
    // Unread is not "Folder missing".
    assert_ne!(row.hook_state, "Folder missing");
}

#[test]
fn a_status_line_left_alone_shows_its_reason() {
    let mut world = world();
    consent(&mut world);
    read(&mut world, MAIN, installed(true));
    let mut left = installed(false);
    left.status_line_left_alone =
        Some("Status line left alone: its command uses Windows paths".into());
    read(&mut world, WORK, left);
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@work.example");
    assert!(!row.live_status_line);
    assert_eq!(
        row.folders[0].status_line_note.as_deref(),
        Some("Status line left alone: its command uses Windows paths")
    );
    assert_eq!(
        account(&snapshot, "me@personal.example").folders[0].status_line_note,
        None
    );
    // The reason only matters while the integration is on.
    world.settings.status_line_integration = false;
    let mut again = installed(false);
    again.status_line_left_alone = Some("Status line left alone: whatever".into());
    read(&mut world, WORK, again);
    assert_eq!(
        account(&world.snapshot(), "me@work.example").folders[0].status_line_note,
        None
    );
}

#[test]
fn an_untracked_account_is_not_hooked_or_checked() {
    let mut registry = two_accounts();
    let work = registry
        .identities()
        .iter()
        .find(|i| i.email.as_deref() == Some("me@work.example"))
        .unwrap()
        .id
        .to_string();
    registry.set_hidden(&work, true);
    let mut world = SettingsWorld::new(registry, HookManager::new());
    consent(&mut world);
    read(&mut world, MAIN, installed(true));
    read(&mut world, WORK, bare());
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@work.example");
    assert!(!row.is_tracked && !row.ring_shown);
    assert_eq!(row.hook_state, "Hooks off");
    assert_eq!(row.folders[0].state, "Not tracked");
    assert_eq!(row.usage_line, "Not checked while it isn't tracked");
    assert!(!row.can_install);
    assert_eq!(row.hook_problem, None);
    // It has no line in Usage, and is no missing hook.
    assert_eq!(snapshot.usage.accounts.len(), 1);
    assert!(snapshot.setup.missing_hooks_accounts.is_empty());
    assert_eq!(snapshot.hooks.summary, "Installed in the tracked account.");
}

// ---- the Hooks section ----

#[test]
fn the_hooks_summary_says_what_is_true_not_what_was_asked() {
    let mut world = world();
    assert_eq!(
        world.snapshot().hooks.summary,
        "Turn on Claude Code control first."
    );
    assert!(world.snapshot().hooks.enabled_locked);
    assert!(!world.snapshot().hooks.enabled);

    consent(&mut world);
    read(&mut world, MAIN, installed(true));
    read(&mut world, WORK, bare());
    let hooks = world.snapshot().hooks;
    assert_eq!(hooks.summary, "Installed in 1 of 2 tracked accounts.");
    assert!(hooks.summary_warning && hooks.enabled && !hooks.enabled_locked);

    read(&mut world, WORK, installed(false));
    let hooks = world.snapshot().hooks;
    assert_eq!(hooks.summary, "Installed in all 2 tracked accounts.");
    assert!(!hooks.summary_warning);

    world.busy = true;
    let hooks = world.snapshot().hooks;
    assert!(hooks.busy && hooks.enabled_locked);
    assert!(!account(&world.snapshot(), "me@work.example").can_install);
    world.busy = false;

    world.settings.hooks_enabled = false;
    let hooks = world.snapshot().hooks;
    assert_eq!(hooks.summary, "Off: no account has this app's hooks.");
    assert!(!hooks.enabled && !hooks.enabled_locked && !hooks.summary_warning);
    assert_eq!(hooks.consent, Some(true));
}

#[test]
fn an_account_in_several_folders_counts_folders() {
    let extra = r"C:\Users\me\.claude-two";
    let mut registry = AccountRegistry::new(paths());
    registry.discover(
        snapshot(
            vec![
                facts(MAIN, Some(personal())),
                facts(extra, Some(personal())),
            ],
            Some(personal()),
        ),
        t0(),
    );
    let mut world = SettingsWorld::new(registry, HookManager::new());
    consent(&mut world);
    read(&mut world, MAIN, installed(true));
    read(&mut world, extra, bare());
    let snapshot = world.snapshot();
    assert_eq!(snapshot.accounts.len(), 1);
    let row = &snapshot.accounts[0];
    assert_eq!(row.hook_state, "Hooks in 1 of 2 folders");
    assert_eq!(row.folder_summary, r"Runs in ~\.claude and ~\.claude-two");
    assert_eq!(
        snapshot.hooks.summary,
        "Installed in 1 of 2 folders of the tracked account."
    );
    assert!(snapshot.hooks.summary_warning);
    assert!(row
        .forget_caption
        .starts_with("This app's hooks are removed from every folder it runs in"));
    assert_eq!(row.folders.len(), 2);

    read(&mut world, extra, installed(true));
    let snapshot = world.snapshot();
    assert_eq!(
        snapshot.hooks.summary,
        "Installed in all 2 folders of the tracked account."
    );
    assert_eq!(snapshot.accounts[0].hook_state, "Hooks in 2 of 2 folders");
}

#[test]
fn the_claude_code_found_and_its_caption() {
    let mut world = world();
    world.versions = vec![
        VersionSighting {
            source: VersionSource::Binary,
            path: Some(PathBuf::from(r"C:\Users\me\.local\bin\claude.exe")),
            version: Some("2.1.282".into()),
        },
        VersionSighting {
            source: VersionSource::Binary,
            path: Some(PathBuf::from(r"C:\tools\old\claude.exe")),
            version: Some("2.1.200".into()),
        },
    ];
    let hooks = world.snapshot().hooks;
    assert_eq!(hooks.claude_version.as_deref(), Some("2.1.200"));
    assert_eq!(
        hooks.claude_path.as_deref(),
        Some(r"C:\tools\old\claude.exe")
    );
    assert!(!hooks.claude_path_chosen);
    assert_eq!(
        hooks.claude_caption,
        "Hooks are written for the oldest claude found, so every version reads them."
    );

    world.settings.claude_binary_path = Some(r"C:\Users\me\.local\bin\claude.exe".into());
    let hooks = world.snapshot().hooks;
    assert_eq!(hooks.claude_version.as_deref(), Some("2.1.282"));
    assert_eq!(
        hooks.claude_path.as_deref(),
        Some(r"C:\Users\me\.local\bin\claude.exe")
    );
    assert!(hooks.claude_path_chosen);
    assert_eq!(
        hooks.claude_caption,
        r"Hooks are written for C:\Users\me\.local\bin\claude.exe."
    );

    world.versions.clear();
    world.settings.claude_binary_path = None;
    let hooks = world.snapshot().hooks;
    assert_eq!((hooks.claude_version, hooks.claude_path), (None, None));
}

#[test]
fn the_pipe_name_and_footnotes_are_carried() {
    let mut world = world();
    world.pipe_name = r"\\.\pipe\agentnotch-hook-S-1-5-21-1".to_owned();
    let hooks = world.snapshot().hooks;
    assert_eq!(hooks.pipe_name, r"\\.\pipe\agentnotch-hook-S-1-5-21-1");
    assert_eq!(hooks.footnotes, FOOTNOTES.to_vec());
    assert!(hooks.footnotes[0].contains("WSL"));
    assert!(hooks.status_line);
    world.settings.status_line_integration = false;
    assert!(!world.snapshot().hooks.status_line);
}

#[test]
fn the_last_change_notice_names_the_file_and_its_backup() {
    let mut world = world();
    assert_eq!(world.snapshot().hooks.last_change, None);
    world.changed = vec![folder_id(WORK)];
    let mut status = installed(false);
    status.newest_backup = Some(PathBuf::from(
        r"C:\Users\me\.claude-work\settings.json.agentnotch-20260921-141000-000.bak",
    ));
    read(&mut world, WORK, status);
    assert_eq!(
        world.snapshot().hooks.last_change.as_deref(),
        Some("Last change: settings.json in ~\\.claude-work. The previous version is kept as settings.json.agentnotch-20260921-141000-000.bak.")
    );
    world.changed = vec![folder_id(WORK), folder_id(MAIN)];
    assert_eq!(
        world.snapshot().hooks.last_change.as_deref(),
        Some("Last change: settings.json in ~\\.claude-work and ~\\.claude. The previous version is kept beside each file.")
    );
}

// ---- Usage ----

#[test]
fn the_usage_section_explains_the_probe_where_it_can_be_turned_off() {
    let mut world = world();
    let usage = world.snapshot().usage;
    assert_eq!(usage.interval_minutes, 5);
    assert_eq!(usage.interval_options, INTERVAL_OPTIONS.to_vec());
    assert_eq!(usage.interval_options, vec![0, 5, 10, 15, 30]);
    assert_eq!(usage.interval_caption, usage_check_caption(5));
    assert!(usage
        .interval_caption
        .starts_with("Every 5 min, only when nothing fresher has arrived"));
    assert!(usage
        .interval_caption
        .contains("never reads your login token"));
    assert!(usage.desktop_cache && !usage.refreshing);
    assert_eq!(usage.accounts.len(), 2);

    world.settings.usage_probe_interval_minutes = 15;
    assert!(world
        .snapshot()
        .usage
        .interval_caption
        .starts_with("Every 15 min"));
    // Never more often than the engine probes.
    world.settings.usage_probe_interval_minutes = 2;
    assert!(world
        .snapshot()
        .usage
        .interval_caption
        .starts_with("Every 5 min"));
    assert_eq!(world.snapshot().usage.interval_minutes, 2);
    world.settings.usage_probe_interval_minutes = 0;
    assert_eq!(
        world.snapshot().usage.interval_caption,
        "Off: readings come only from live status lines and Claude Code's own cache."
    );
    world.refreshing = true;
    assert!(world.snapshot().usage.refreshing);
}

#[test]
fn the_desktop_caption_says_when_the_cache_cannot_be_read() {
    let mut world = world();
    let ordinary = "Claude Desktop keeps the limits it last saw on disk; no token is involved.";
    assert_eq!(world.snapshot().usage.desktop_caption, ordinary);
    assert_eq!(world.snapshot().usage.desktop_format, None);
    for (format, word) in [
        (DesktopCacheFormat::Simple, "simple"),
        (DesktopCacheFormat::Absent, "absent"),
    ] {
        world.desktop_format = Some(format);
        let usage = world.snapshot().usage;
        assert_eq!(usage.desktop_format.as_deref(), Some(word));
        assert_eq!(usage.desktop_caption, ordinary);
    }
    world.desktop_format = Some(DesktopCacheFormat::Blockfile);
    let usage = world.snapshot().usage;
    assert_eq!(usage.desktop_format.as_deref(), Some("blockfile"));
    assert_eq!(usage.desktop_caption, UNSUPPORTED_FORMAT_TEXT);
    assert_eq!(
        usage.desktop_caption,
        "Claude Desktop's cache on this PC uses a format this version can't read."
    );
    world.settings.reads_desktop_usage_cache = false;
    assert!(!world.snapshot().usage.desktop_cache);
}

#[test]
fn usage_lines_say_the_numbers_or_why_there_are_none() {
    let line = |reading: Option<&RingReading>, shown: bool, tracked: bool| {
        usage_line(reading, shown, tracked, now())
    };
    let ok = reading(usage(34.0, 41.0, 240), false);
    assert_eq!(
        line(Some(&ok), true, true),
        "5-hour 34% · weekly 41% · 4m ago"
    );
    let stale = reading(usage(34.0, 41.0, 240), true);
    assert_eq!(
        line(Some(&stale), true, true),
        "5-hour 34% · weekly 41% · 4m ago, stale"
    );
    // Rounded down, and a fraction of a hundred never reads one short.
    let odd = reading(usage(29.0, 99.9, 30), false);
    assert_eq!(
        line(Some(&odd), true, true),
        "5-hour 29% · weekly 99% · just now"
    );
    let over = reading(usage(110.0, 5000.0, 30), false);
    assert_eq!(
        line(Some(&over), true, true),
        "5-hour 110% · weekly 999% · just now"
    );

    assert_eq!(line(None, true, true), "No reading yet");
    assert_eq!(
        line(Some(&RingReading::Waiting), true, true),
        "Waiting for the first reading"
    );
    assert_eq!(
        line(Some(&RingReading::SignInNeeded), true, true),
        "Not signed in to Claude"
    );
    assert_eq!(
        line(
            Some(&RingReading::Unavailable(
                "Usage isn't available for this login".into()
            )),
            true,
            true
        ),
        "Usage isn't available for this login"
    );
    assert_eq!(
        line(
            Some(&RingReading::Failed("claude exited with 1".into())),
            true,
            true
        ),
        "Usage check failed: claude exited with 1"
    );
    // Tracking off, or the ring off, takes the checks with it.
    assert_eq!(
        line(Some(&ok), true, false),
        "Not checked while it isn't tracked"
    );
    assert_eq!(
        line(Some(&ok), false, true),
        "Not checked while its ring is off"
    );

    // Without a window of either kind there is nothing to say.
    let mut empty = usage(0.0, 0.0, 30);
    empty.five_hour = None;
    empty.seven_day = None;
    assert_eq!(
        line(Some(&reading(empty, false)), true, true),
        "No limits reported"
    );
}

#[test]
fn each_account_row_and_usage_line_carry_the_reading() {
    let mut world = world();
    let main_id = world
        .registry
        .identities()
        .iter()
        .find(|i| i.email.as_deref() == Some("me@personal.example"))
        .unwrap()
        .id
        .clone();
    world
        .readings
        .insert(main_id, reading(usage(34.0, 41.0, 240), true));
    let snapshot = world.snapshot();
    let row = account(&snapshot, "me@personal.example");
    assert_eq!(row.usage_line, "5-hour 34% · weekly 41% · 4m ago, stale");
    assert!(row.usage_stale);
    let line = snapshot
        .usage
        .accounts
        .iter()
        .find(|line| line.ring_id == row.ring_id)
        .unwrap();
    assert_eq!(line.line, row.usage_line);
    assert_eq!(line.label, row.label);
    let work = account(&snapshot, "me@work.example");
    assert_eq!(work.usage_line, "No reading yet");
    assert!(!work.usage_stale);
}

// ---- notifications, attention, the rest ----

#[test]
fn the_windows_notification_permission_reads_in_words() {
    let mut world = world();
    let cases = [
        (NotifyPermission::Allowed, "allowed", "Allowed", false),
        (
            NotifyPermission::DisabledForApp,
            "disabled_for_app",
            "Off in Windows Settings",
            true,
        ),
        (
            NotifyPermission::DisabledForUser,
            "disabled_for_user",
            "Off in Windows Settings",
            true,
        ),
        (
            NotifyPermission::DisabledByPolicy,
            "disabled_by_policy",
            "Off in Windows Settings",
            true,
        ),
        (
            NotifyPermission::Unavailable,
            "unavailable",
            "Banners need the installed app",
            false,
        ),
    ];
    for (permission, word, text, warning) in cases {
        world.notify = permission;
        let section = world.snapshot().notifications;
        assert_eq!(section.permission, word);
        assert_eq!(section.permission_text, text);
        assert_eq!(section.permission_warning, warning, "{word}");
        assert!(section.notify_needs_input && section.notify_ready_for_review);
    }
    // Windows turned banners off, and they are off here too: nothing to warn of.
    world.notify = NotifyPermission::DisabledForApp;
    world.settings.notify_needs_input = false;
    world.settings.notify_ready_for_review = false;
    let section = world.snapshot().notifications;
    assert!(!section.permission_warning);
    assert!(!section.notify_needs_input && !section.notify_ready_for_review);
}

#[test]
fn the_hot_key_report_is_carried_with_the_attention_settings() {
    let mut world = world();
    world.settings.hot_key = "ctrlAltSpace".into();
    let attention = world.snapshot().attention;
    assert_eq!(attention.hotkey, "ctrlAltSpace");
    assert!(attention.hotkey_ok);
    assert_eq!(attention.hotkey_message, None);

    world.hotkey_ok = false;
    world.hotkey_message = Some("Another app already uses Ctrl+Alt+Space.".into());
    let attention = world.snapshot().attention;
    assert!(!attention.hotkey_ok);
    assert_eq!(
        attention.hotkey_message.as_deref(),
        Some("Another app already uses Ctrl+Alt+Space.")
    );
    // The rest is the settings' own.
    assert_eq!(attention.panel_open_mode, world.settings.auto_open);
    assert_eq!(attention.peek_seconds, world.settings.peek_seconds);
    assert_eq!(attention.type_replies, world.settings.type_replies);
}

#[test]
fn cloud_and_advanced_are_passed_through() {
    let mut world = world();
    world.cloud.website_url = Some("https://agentnotch.example".into());
    world.cloud.sync_enabled = true;
    world.session_count = 15;
    world.review_count = 3;
    let snapshot = world.snapshot();
    assert_eq!(snapshot.cloud, world.cloud);
    assert_eq!(
        (
            snapshot.advanced.session_count,
            snapshot.advanced.review_count
        ),
        (15, 3)
    );
    assert!(!snapshot.sealed);
}

// ---- suggestions and folders nobody is signed in to ----

#[test]
fn suggestions_and_unsigned_folders_list_by_their_short_paths() {
    let unsigned = r"C:\Users\me\.claude-new";
    let mut registry = AccountRegistry::new(paths());
    registry.discover(
        snapshot(
            vec![
                facts(MAIN, Some(personal())),
                facts(OLD, Some(work())),
                FolderFacts {
                    has_live_session: true,
                    ..facts(unsigned, None)
                },
            ],
            Some(personal()),
        ),
        t0(),
    );
    let world = SettingsWorld::new(registry, HookManager::new());
    let snapshot = world.snapshot();
    // Named like a backup: offered, not added.
    assert_eq!(snapshot.suggestions.len(), 1, "{:?}", snapshot.suggestions);
    assert_eq!(snapshot.suggestions[0].path, r"~\.claude-old");
    assert_eq!(snapshot.suggestions[0].reason, "Named like a backup copy.");
    // A folder Claude Code runs in with nobody signed in: listed as such.
    assert_eq!(snapshot.unsigned_folders, vec![r"~\.claude-new".to_owned()]);
    assert_eq!(snapshot.accounts.len(), 1);
}

// ---- the contract ----

fn key_paths(value: &Value) -> BTreeSet<String> {
    fn walk(value: &Value, prefix: &str, out: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    let path = format!("{prefix}.{key}");
                    out.insert(path.clone());
                    walk(inner, &path, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, &format!("{prefix}[]"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(value, "", &mut out);
    out
}

#[test]
fn the_snapshot_has_the_keys_of_the_ui_contract_fixture() {
    let fixture: Value = serde_json::from_str(include_str!("ui-contract/settings.json")).unwrap();
    let mut registry = AccountRegistry::new(paths());
    registry.discover(
        snapshot(
            vec![
                facts(MAIN, Some(personal())),
                facts(WORK, Some(work())),
                facts(OLD, Some(work())),
            ],
            Some(personal()),
        ),
        t0(),
    );
    let mut world = SettingsWorld::new(registry, HookManager::new());
    consent(&mut world);
    read(&mut world, MAIN, installed(true));
    let mut left = installed(false);
    left.status_line_left_alone =
        Some("Status line left alone: its command uses Windows paths".into());
    left.not_hookable = Some("x".into());
    read(&mut world, WORK, left);
    world.changed = vec![folder_id(WORK)];
    world.versions = vec![VersionSighting {
        source: VersionSource::Binary,
        path: Some(PathBuf::from(r"C:\Users\me\.local\bin\claude.exe")),
        version: Some("2.1.282".into()),
    }];
    world.desktop_format = Some(DesktopCacheFormat::Absent);
    world.hotkey_message = Some("taken".into());
    world.cloud.last_error = Some("x".into());
    let mine = serde_json::to_value(world.snapshot()).unwrap();
    // The fixture is the sealed demo, which finds no folder to suggest and
    // asks no consent: its lists are empty, so their items have no keys.
    let keys: BTreeSet<String> = key_paths(&mine)
        .into_iter()
        .filter(|k| !k.starts_with(".setup.consent_files[]") && !k.starts_with(".suggestions[]"))
        .collect();
    let expected = key_paths(&fixture);
    // `cloud.auth` is a variant: the fixture shows the signed-in one.
    let strip = |set: &BTreeSet<String>| -> BTreeSet<String> {
        set.iter()
            .filter(|k| !k.starts_with(".cloud.auth."))
            .cloned()
            .collect()
    };
    assert_eq!(strip(&keys), strip(&expected));
}
