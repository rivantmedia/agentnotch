//! The hook manager over throwaway homes: the Mac's `ConsentGateTests`
//! (A2_ConsentGateTests.swift) and `PP_HooksTests`, with what DESIGN-WIN
//! §4.3 adds: one write per physical settings.json, the install record, and
//! when passes run.
//!
//! Folders and accounts are built by hand: finding and grouping them is the
//! registry's work. Each pass is the hub's: `plan`, the Io job
//! (`apply_installs`), then the outcomes and what is on disk go back in.

use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::core::settings::ControlSettings;
use agentnotch_engine::core::settings_doc::Json;
use agentnotch_engine::hooks::apply::{apply_installs_with, read_status, Setup};
use agentnotch_engine::hooks::commands::{
    exec_form, hook_copy_path, not_hookable_reason, string_command, Subcommand, HOOK_EXE_NAME,
};
use agentnotch_engine::hooks::events::hook_events;
use agentnotch_engine::hooks::facts::ClaudeCodeFacts;
use agentnotch_engine::hooks::manager::{
    changed_folders, consent_files, consent_line, is_install_target, removal_plans, uninstall_all,
    HookManager, ACCOUNTS_DEBOUNCE, RECHECK_INTERVAL,
};
use agentnotch_engine::model::{
    Account, AccountId, FolderKind, FolderSource, Identity, IdentityId, RingId, RunFolder,
};
use agentnotch_engine::persist::hook_install::{HookInstallFile, HookInstallRecord};
use agentnotch_engine::platform::{Clock, SecureFiles};
use agentnotch_engine::runtime_types::{
    CommandForm, InstallChange, InstallOutcome, InstallPlan, StatusLineIntent, VersionSighting,
    VersionSource,
};
use agentnotch_engine::testkit::{snapshot_dir, write_file, FakeClock, StdSecureFiles};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 2026-09-21T14:13:20Z.
const START_MS: u64 = 1_790_000_000_000;

/// Git Bash is there: someone else's status line may be wrapped.
const SETUP: Setup = Setup { git_bash: true };

const UPSTREAM_COMMAND: &str = "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe";

/// A settings.json as users have them: other tools' hooks (the official
/// app's among them) and a status line.
const REALISTIC: &str = r#"{
  "model": "opus",
  "permissions": {"allow": ["Bash(npm test:*)"], "deny": []},
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "~/bin/guard-bash.sh", "timeout": 30}]},
      {"matcher": "*", "hooks": [{"type": "command", "command": "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe"}]}
    ]
  },
  "statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2}
}
"#;

struct Home {
    _temp: tempfile::TempDir,
    /// Resolved, so paths built from it are the ones the installer reports.
    root: PathBuf,
    clock: FakeClock,
    manager: HookManager,
    settings: ControlSettings,
    versions: Vec<VersionSighting>,
    facts: ClaudeCodeFacts,
}

fn home() -> Home {
    home_with(&DevFlags::default())
}

fn home_with(flags: &DevFlags) -> Home {
    let temp = tempfile::tempdir().unwrap();
    let root = StdSecureFiles.canonical(temp.path()).unwrap();
    let source = root.join("app").join(HOOK_EXE_NAME);
    write_file(&source, b"MZ not really an exe");
    Home {
        _temp: temp,
        root,
        clock: FakeClock::at_ms(START_MS),
        manager: HookManager::configured(source, flags),
        settings: ControlSettings::default(),
        versions: vec![sighting(VersionSource::Binary, Some("2.1.280"))],
        facts: ClaudeCodeFacts::compiled_in(),
    }
}

impl Home {
    /// A config folder under the home, with a `projects` folder as Claude
    /// Code leaves one, and the settings.json given.
    fn folder(&self, name: &str, kind: FolderKind, settings: Option<&str>) -> RunFolder {
        let dir = self.root.join(name);
        fs::create_dir_all(dir.join("projects")).unwrap();
        if let Some(settings) = settings {
            write_file(&dir.join("settings.json"), settings);
        }
        run_folder(&dir, kind)
    }

    fn grant_consent(&mut self) {
        self.settings.hook_consent = Some(true);
        self.settings.hooks_enabled = true;
    }

    fn plan(&self, accounts: &[Account], folders: &[RunFolder]) -> Vec<InstallPlan> {
        self.manager.plan(
            accounts,
            folders,
            &self.settings,
            &self.versions,
            &self.facts,
        )
    }

    /// One pass, as the hub runs it.
    fn pass(
        &mut self,
        accounts: &[Account],
        folders: &[RunFolder],
    ) -> (Vec<InstallPlan>, Vec<InstallOutcome>) {
        let plans = self.plan(accounts, folders);
        let outcomes = self.carry_out(&plans, folders);
        (plans, outcomes)
    }

    fn carry_out(&mut self, plans: &[InstallPlan], folders: &[RunFolder]) -> Vec<InstallOutcome> {
        let outcomes = apply_installs_with(plans, &StdSecureFiles, &self.clock, &SETUP);
        self.manager.absorb_outcomes(plans, &outcomes);
        for folder in folders {
            let disk = read_status(&folder.config_dir, &StdSecureFiles, &SETUP);
            self.manager.absorb_status(&folder.id, disk, &self.settings);
        }
        self.clock.advance(Duration::from_secs(2));
        outcomes
    }

    fn snapshot(&self) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        snapshot_dir(&self.root).unwrap()
    }
}

fn run_folder(dir: &Path, kind: FolderKind) -> RunFolder {
    RunFolder {
        id: id(dir),
        config_dir: dir.to_path_buf(),
        config_dir_env: None,
        custom_label: None,
        seen_config_dir_envs: Vec::new(),
        identity: None,
        subscription_type: None,
        color_index: 0,
        source: FolderSource::Discovered,
        last_seen_at: None,
        is_hidden: false,
        kind,
    }
}

fn id(dir: &Path) -> AccountId {
    AccountId(dir.to_string_lossy().into_owned())
}

/// An identity running in `run` and stored in `stores`.
fn account(email: &str, run: &[&RunFolder], stores: &[&RunFolder]) -> Account {
    Account {
        identity_id: IdentityId(format!("email:{email}")),
        ring_id: RingId(format!("claude-acct-{email}")),
        label: email.to_owned(),
        own_label: None,
        monogram: email[..1].to_uppercase(),
        color_index: 0,
        email: Some(email.to_owned()),
        plan_name: None,
        organization_uuid: None,
        run_dirs: run.iter().map(|folder| folder.id.clone()).collect(),
        store_dirs: stores.iter().map(|folder| folder.id.clone()).collect(),
        includes_default: false,
        is_tracked: true,
        ring_shown: true,
        is_signed_in: true,
        launch_command: None,
        can_forget: true,
    }
}

fn sighting(source: VersionSource, version: Option<&str>) -> VersionSighting {
    VersionSighting {
        source,
        path: None,
        version: version.map(str::to_owned),
    }
}

fn status(folder: &RunFolder) -> agentnotch_engine::runtime_types::FolderHookStatus {
    read_status(&folder.config_dir, &StdSecureFiles, &SETUP)
}

fn settings_path(folder: &RunFolder) -> PathBuf {
    folder.config_dir.join("settings.json")
}

fn json(path: &Path) -> Json {
    Json::parse(&fs::read(path).unwrap()).unwrap()
}

fn events(folder: &RunFolder) -> Vec<String> {
    json(&settings_path(folder))
        .get("hooks")
        .and_then(Json::members)
        .unwrap_or_default()
        .iter()
        .map(|member| member.key.clone())
        .collect()
}

/// The commands registered for `event`, in file order.
fn commands(folder: &RunFolder, event: &str) -> Vec<String> {
    json(&settings_path(folder))
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Json::items)
        .unwrap_or_default()
        .iter()
        .flat_map(|group| group.get("hooks").and_then(Json::items).unwrap_or_default())
        .filter_map(|entry| entry.get("command").and_then(Json::as_str))
        .map(str::to_owned)
        .collect()
}

fn status_line_command(folder: &RunFolder) -> Option<String> {
    json(&settings_path(folder))
        .get("statusLine")?
        .get("command")?
        .as_str()
        .map(str::to_owned)
}

fn hook_command(folder: &RunFolder) -> String {
    string_command(
        &hook_copy_path(&folder.config_dir).to_string_lossy(),
        Subcommand::Hook,
    )
    .expect("a temporary folder's path needs no quotes")
}

// ---- The consent gate (A2_ConsentGateTests) ----

#[test]
fn nothing_is_written_before_consent() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let work = home.folder(".claude-work", FolderKind::Run, None);
    let folders = [main.clone(), work.clone()];
    let accounts = [
        account("me@home.com", &[&main], &[]),
        account("me@work.com", &[&work], &[]),
    ];
    let before = home.snapshot();

    home.manager.start(home.clock.now());
    let (plans, outcomes) = home.pass(&accounts, &folders);
    assert!(plans.is_empty() && outcomes.is_empty());
    assert_eq!(home.snapshot(), before);
    // Statuses are still read back, so the card can say what is there.
    assert!(home.manager.folder_status(&main.id).config_dir_exists);
    assert!(!home.manager.folder_status(&main.id).hooks_registered);
    assert_eq!(
        home.manager.codenotch_hooks_folders(),
        vec![main.id.0.clone()]
    );
    // The card names both files.
    let paths = Paths::native(&home.root);
    assert_eq!(consent_files(&accounts, &folders, &paths).len(), 2);

    // "Not now" is remembered and still writes nothing.
    home.settings.hook_consent = Some(false);
    let (plans, _) = home.pass(&accounts, &folders);
    assert!(plans.is_empty());
    assert_eq!(home.snapshot(), before);
    assert!(home.manager.record().files.is_empty());
}

/// Before the yes not even a removal is planned, whatever a folder holds:
/// until then this app wrote nothing, so there is nothing of its to remove.
#[test]
fn without_consent_not_even_removals_are_planned() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let folders = [main.clone()];
    let accounts = [account("me@home.com", &[&main], &[])];
    home.grant_consent();
    home.pass(&accounts, &folders);
    assert!(status(&main).hooks_installed);
    let installed = home.snapshot();

    for consent in [None, Some(false)] {
        home.settings.hook_consent = consent;
        for hooks_enabled in [true, false] {
            home.settings.hooks_enabled = hooks_enabled;
            let (plans, _) = home.pass(&accounts, &folders);
            assert!(plans.is_empty(), "{consent:?} {hooks_enabled}");
            assert!(home
                .manager
                .forget_plans(&folders, &home.settings)
                .is_empty());
        }
    }
    assert_eq!(home.snapshot(), installed);
}

#[test]
fn consent_installs_into_every_tracked_account() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let work = home.folder(".claude-work", FolderKind::Run, None);
    let folders = [main.clone(), work.clone()];
    let accounts = [
        account("me@home.com", &[&main], &[]),
        account("me@work.com", &[&work], &[]),
    ];

    home.grant_consent();
    let (plans, outcomes) = home.pass(&accounts, &folders);
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|plan| !plan.remove_only
        && plan.status_line == StatusLineIntent::Wrap
        && plan.events
            == hook_events(agentnotch_engine::hooks::version::ClaudeCodeVersion::parse(
                "2.1.280"
            ))));
    for folder in &folders {
        let on_disk = status(folder);
        assert!(
            on_disk.hooks_installed && on_disk.status_line_installed,
            "{}",
            folder.id
        );
        let known = home.manager.folder_status(&folder.id);
        assert!(known.hooks_installed && known.status_line_installed);
        assert_eq!(known.last_outcome, Some(InstallChange::Written));
        assert_eq!(known.last_error, None);
        assert_eq!(known.form.as_deref(), Some("string"));
        assert_eq!(known.hook_command, Some(hook_command(folder)));
    }
    assert_eq!(
        changed_folders(&outcomes),
        vec![main.id.clone(), work.id.clone()]
    );
    // The official app's entries stay, and are reported.
    assert_eq!(
        home.manager.codenotch_hooks_folders(),
        vec![main.id.0.clone()]
    );
    assert!(commands(&main, "PreToolUse").contains(&UPSTREAM_COMMAND.to_owned()));
    assert!(home
        .manager
        .folders_missing_hooks(&accounts, &folders)
        .is_empty());

    // A second pass writes nothing.
    let installed = home.snapshot();
    let (_, outcomes) = home.pass(&accounts, &folders);
    assert!(outcomes
        .iter()
        .all(|outcome| outcome.result == Ok(InstallChange::Unchanged)));
    assert!(changed_folders(&outcomes).is_empty());
    assert_eq!(home.snapshot(), installed);
}

#[test]
fn untracking_and_forgetting_take_our_hooks_out() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(r#"{"model":"opus"}"#));
    let work = home.folder(
        ".claude-work",
        FolderKind::Run,
        Some(r#"{"statusLine":{"type":"command","command":"echo hi"}}"#),
    );
    let folders = [main.clone(), work.clone()];
    let mut accounts = [
        account("me@home.com", &[&main], &[]),
        account("me@work.com", &[&work], &[]),
    ];
    home.grant_consent();
    home.pass(&accounts, &folders);
    assert!(status(&work).hooks_installed && status(&work).status_line_installed);
    assert_eq!(home.manager.record().files.len(), 2);

    // Untrack: ours come out, the status line is restored.
    accounts[1].is_tracked = false;
    let (plans, _) = home.pass(&accounts, &folders);
    assert_eq!(plans.iter().filter(|plan| plan.remove_only).count(), 1);
    let on_disk = status(&work);
    assert!(!on_disk.hooks_registered && !on_disk.status_line_installed);
    assert_eq!(status_line_command(&work).as_deref(), Some("echo hi"));
    assert!(!hook_copy_path(&work.config_dir).exists());
    assert!(home.manager.record().entry_for(&work.id).is_none());
    assert!(home.manager.record().entry_for(&main.id).is_some());
    // Another pass leaves it alone: not even a removal is planned.
    let cleaned = fs::read(settings_path(&work)).unwrap();
    let (plans, _) = home.pass(&accounts, &folders);
    assert!(plans.iter().all(|plan| plan.folder != work.id));
    assert_eq!(fs::read(settings_path(&work)).unwrap(), cleaned);

    // Track again: back in.
    accounts[1].is_tracked = true;
    home.pass(&accounts, &folders);
    assert!(status(&work).hooks_installed);

    // Forget: out, while the folder is still known, then gone for good.
    let plans = home
        .manager
        .forget_plans(std::slice::from_ref(&work), &home.settings);
    assert_eq!(plans.len(), 1);
    assert!(plans[0].remove_only);
    home.carry_out(&plans, &folders);
    assert!(!status(&work).hooks_registered);
    let left = [main.clone()];
    home.manager.retain_folders(&left);
    assert_eq!(home.manager.folder_status(&work.id), Default::default());
    assert!(home.manager.record().entry_for(&work.id).is_none());
    home.pass(&accounts[..1], &left);
    assert!(status(&main).hooks_installed);
    assert!(!status(&work).hooks_registered);
}

/// A status line from an older Claude Code rewrites the hooks without the
/// events it doesn't know.
#[test]
fn an_older_client_lowers_the_registered_events() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, None);
    let folders = [main.clone()];
    let accounts = [account("me@home.com", &[&main], &[])];
    home.grant_consent();
    let now = home.clock.now();
    home.manager.start(now);
    home.manager
        .pass_started(&home.settings, &home.versions, &home.facts, now);
    home.pass(&accounts, &folders);
    assert!(events(&main).contains(&"PermissionDenied".to_owned()));
    assert!(!home.manager.pass_due(home.clock.now()));

    home.versions
        .push(sighting(VersionSource::StatusLine, Some("2.1.50")));
    let now = home.clock.now();
    assert!(home.manager.note_versions(&home.versions, &home.facts, now));
    assert!(home.manager.pass_due(now));
    home.manager
        .pass_started(&home.settings, &home.versions, &home.facts, now);
    home.pass(&accounts, &folders);
    let lowered = events(&main);
    assert!(!lowered.contains(&"PermissionDenied".to_owned()));
    assert!(!lowered.contains(&"TaskCreated".to_owned()));
    assert!(lowered.contains(&"TaskCompleted".to_owned()));

    // A newer one changes nothing.
    let before = fs::read(settings_path(&main)).unwrap();
    home.versions
        .push(sighting(VersionSource::StatusLine, Some("2.1.300")));
    let now = home.clock.now();
    assert!(!home.manager.note_versions(&home.versions, &home.facts, now));
    assert!(!home.manager.pass_due(now));
    home.pass(&accounts, &folders);
    assert_eq!(fs::read(settings_path(&main)).unwrap(), before);
}

#[test]
fn no_install_plans_nothing_even_after_consent() {
    let mut home = home_with(&DevFlags {
        no_install: true,
        ..DevFlags::default()
    });
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let folders = [main.clone()];
    let accounts = [account("me@home.com", &[&main], &[])];
    home.grant_consent();
    let before = home.snapshot();
    assert!(home.manager.installs_disabled());
    for hooks_enabled in [true, false] {
        home.settings.hooks_enabled = hooks_enabled;
        let (plans, _) = home.pass(&accounts, &folders);
        assert!(plans.is_empty());
        assert!(home
            .manager
            .forget_plans(&folders, &home.settings)
            .is_empty());
    }
    assert_eq!(home.snapshot(), before);

    let sealed = HookManager::configured(
        home.root.join("app").join(HOOK_EXE_NAME),
        &DevFlags {
            sealed: true,
            ..DevFlags::default()
        },
    );
    home.settings.hooks_enabled = true;
    assert!(sealed
        .plan(
            &accounts,
            &folders,
            &home.settings,
            &home.versions,
            &home.facts
        )
        .is_empty());
}

// ---- Run folders, stores and the shared history (PP_HooksTests) ----

/// Two accounts over four run folders (one of them run by both, as
/// `~\.claude` is while accounts are mirrored into it), three stores and
/// the shared history.
struct Layout {
    default: RunFolder,
    paras_a: RunFolder,
    paras_b: RunFolder,
    biios: RunFolder,
    stores: Vec<RunFolder>,
    shared: RunFolder,
}

impl Layout {
    fn build(home: &Home) -> Layout {
        let store = |name: &str| {
            let folder = home.folder(name, FolderKind::Store, None);
            write_file(
                &folder.config_dir.join(".claude.json"),
                br#"{"oauthAccount":{}}"#,
            );
            write_file(&folder.config_dir.join(".parallel-accounts-store"), b"");
            folder
        };
        let layout = Layout {
            default: home.folder(".claude", FolderKind::Run, Some(REALISTIC)),
            paras_a: home.folder(".claude-windows/801f9dd51396", FolderKind::Run, None),
            paras_b: home.folder(
                ".claude-windows/b9fbb9ecd7cb",
                FolderKind::Run,
                Some("{}\n"),
            ),
            biios: home.folder(".claude-windows/1bf3e8f92b11", FolderKind::Run, None),
            stores: vec![
                store(".claude-paras"),
                store(".claude-paras-rivant-in"),
                store(".claude-claude"),
            ],
            shared: home.folder(".claude-shared", FolderKind::Infrastructure, None),
        };
        write_file(
            &layout.shared.config_dir.join("projects/x/session.jsonl"),
            b"{}\n",
        );
        layout
    }

    fn run_folders(&self) -> Vec<&RunFolder> {
        vec![&self.default, &self.paras_a, &self.paras_b, &self.biios]
    }

    fn folders(&self) -> Vec<RunFolder> {
        let mut folders: Vec<RunFolder> = self.run_folders().into_iter().cloned().collect();
        folders.extend(self.stores.iter().cloned());
        folders.push(self.shared.clone());
        folders
    }

    /// paras first, then biios.
    fn accounts(&self) -> Vec<Account> {
        vec![
            account(
                "paras@example.com",
                &[&self.default, &self.paras_a, &self.paras_b],
                &[&self.stores[0], &self.stores[1]],
            ),
            account(
                "biios@example.com",
                &[&self.default, &self.biios],
                &[&self.stores[2]],
            ),
        ]
    }

    /// Everything in a store or the shared history.
    fn untouchable(&self, home: &Home) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut files = std::collections::BTreeMap::new();
        for folder in self.stores.iter().chain([&self.shared]) {
            for (path, bytes) in snapshot_dir(&folder.config_dir).unwrap() {
                files.insert(folder.config_dir.join(path), bytes);
            }
        }
        assert!(!files.is_empty());
        let _ = home;
        files
    }
}

#[test]
fn consent_installs_into_the_four_run_folders_only() {
    let mut home = home();
    let layout = Layout::build(&home);
    let (accounts, folders) = (layout.accounts(), layout.folders());
    let paths = Paths::native(&home.root);
    // The card names the four files, from the home folder, and whose each is.
    let home_name = match paths.style() {
        PathStyle::Windows => "%USERPROFILE%",
        PathStyle::Posix => "~",
    };
    let expected: Vec<String> = layout
        .run_folders()
        .into_iter()
        .map(|folder| {
            let file = settings_path(folder).to_string_lossy().into_owned();
            let below = paths.strip_prefix(paths.home(), &file).unwrap();
            format!("{home_name}{}{below}", paths.style().separator())
        })
        .collect();
    let card = consent_files(&accounts, &folders, &paths);
    let listed: Vec<&str> = card.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(listed, expected);
    assert_eq!(card[0].account.as_deref(), Some("paras@example.com"));
    assert_eq!(card[3].account.as_deref(), Some("biios@example.com"));
    let before = layout.untouchable(&home);

    home.grant_consent();
    let (plans, _) = home.pass(&accounts, &folders);
    assert_eq!(plans.len(), 4);
    for folder in layout.run_folders() {
        let on_disk = status(folder);
        assert!(
            on_disk.hooks_installed && on_disk.status_line_installed,
            "{}",
            folder.id
        );
    }
    // Stores and the shared history: not a byte changed or added.
    assert_eq!(layout.untouchable(&home), before);
    assert!(!settings_path(&layout.stores[0]).exists());
    assert!(!layout.shared.config_dir.join("hooks").exists());
    assert_eq!(home.manager.record().files.len(), 4);

    // And they stay so through untracking and the switch going off.
    let mut untracked = accounts.clone();
    untracked[0].is_tracked = false;
    home.pass(&untracked, &folders);
    home.settings.hooks_enabled = false;
    home.pass(&accounts, &folders);
    assert_eq!(layout.untouchable(&home), before);
}

#[test]
fn a_new_window_gets_the_hooks_on_the_next_pass() {
    let mut home = home();
    let layout = Layout::build(&home);
    let (mut accounts, mut folders) = (layout.accounts(), layout.folders());
    home.grant_consent();
    let now = home.clock.now();
    home.manager.start(now);
    home.manager
        .pass_started(&home.settings, &home.versions, &home.facts, now);
    home.pass(&accounts, &folders);

    let window = home.folder(".claude-windows/0a1b2c3d4e5f", FolderKind::Run, None);
    folders.push(window.clone());
    accounts[1].run_dirs.push(window.id.clone());
    let changed = home.clock.now();
    home.manager.accounts_changed(changed);
    assert!(!home.manager.pass_due(changed));
    assert!(home.manager.pass_due(changed + ACCOUNTS_DEBOUNCE));
    let (_, outcomes) = home.pass(&accounts, &folders);
    assert!(status(&window).hooks_installed);
    assert_eq!(changed_folders(&outcomes), vec![window.id.clone()]);
    let hooked = [&layout.biios, &window]
        .iter()
        .filter(|folder| home.manager.folder_status(&folder.id).hooks_installed)
        .count();
    assert_eq!(hooked, 2);
}

#[test]
fn untracking_an_account_takes_ours_out_of_all_its_run_folders() {
    let mut home = home();
    let layout = Layout::build(&home);
    let (mut accounts, folders) = (layout.accounts(), layout.folders());
    home.grant_consent();
    home.pass(&accounts, &folders);

    accounts[0].is_tracked = false;
    home.pass(&accounts, &folders);
    for folder in [&layout.paras_a, &layout.paras_b] {
        assert!(!status(folder).hooks_registered, "{}", folder.id);
    }
    // The folder both run in keeps our hooks while the other is tracked.
    assert!(status(&layout.default).hooks_installed);
    assert!(status(&layout.biios).hooks_installed);

    // Every account untracked: nothing of ours anywhere.
    accounts[1].is_tracked = false;
    home.pass(&accounts, &folders);
    for folder in layout.run_folders() {
        assert!(!status(folder).hooks_registered, "{}", folder.id);
    }
    assert!(home.manager.record().files.is_empty());
    // The user's own settings are what they were, entry for entry.
    assert_eq!(
        json(&settings_path(&layout.default)),
        Json::parse(REALISTIC.as_bytes()).unwrap()
    );

    accounts[1].is_tracked = true;
    home.pass(&accounts, &folders);
    assert!(status(&layout.default).hooks_installed);

    home.settings.hooks_enabled = false;
    home.pass(&accounts, &folders);
    assert!(!status(&layout.biios).hooks_registered);
    assert!(!status(&layout.default).hooks_registered);
}

/// Whatever the accounts say, nothing is installed into a store or the
/// shared history.
#[test]
fn stores_and_the_shared_history_are_never_install_targets() {
    let mut home = home();
    let layout = Layout::build(&home);
    let folders = layout.folders();
    // An account that (wrongly) lists a store and the shared history as
    // folders it runs in.
    let accounts = [account(
        "paras@example.com",
        &[&layout.default, &layout.stores[0], &layout.shared],
        &[],
    )];
    assert!(is_install_target(&accounts, &layout.default));
    assert!(!is_install_target(&accounts, &layout.stores[0]));
    assert!(!is_install_target(&accounts, &layout.shared));
    // Nor one nobody lists.
    assert!(!is_install_target(&[], &layout.stores[2]));
    assert!(is_install_target(&[], &layout.biios));
    let mut hidden = layout.biios.clone();
    hidden.is_hidden = true;
    assert!(!is_install_target(&[], &hidden));

    let before = layout.untouchable(&home);
    home.grant_consent();
    let (plans, _) = home.pass(&accounts, &folders);
    let planned: Vec<&AccountId> = plans.iter().map(|plan| &plan.folder).collect();
    for folder in layout.stores.iter().chain([&layout.shared]) {
        assert!(!planned.contains(&&folder.id), "{}", folder.id);
    }
    assert_eq!(layout.untouchable(&home), before);
    assert!(!settings_path(&layout.stores[0]).exists());
    assert!(!layout.stores[0].config_dir.join("hooks").exists());

    // Not by the uninstall either: a store is only cleaned when the record
    // says ours went in.
    let plans = removal_plans(&HookInstallRecord::default(), &folders);
    assert_eq!(plans.len(), 4);
    assert!(plans.iter().all(|plan| plan.remove_only));
}

/// A folder hooked while it was a run folder and a store since: ours come
/// out, and the settings.json that holds the user's own entries stays.
#[test]
fn a_store_that_holds_ours_is_cleaned_and_its_settings_stay() {
    let mut home = home();
    let mut folder = home.folder(".claude-claude", FolderKind::Run, Some(REALISTIC));
    let mut accounts = [account("me@home.com", &[&folder], &[])];
    home.grant_consent();
    home.pass(&accounts, std::slice::from_ref(&folder));
    assert!(status(&folder).hooks_installed);

    // Claude Parallel Profiles adopts it as a store.
    folder.kind = FolderKind::Store;
    accounts[0].run_dirs.clear();
    accounts[0].store_dirs.push(folder.id.clone());
    let (plans, _) = home.pass(&accounts, std::slice::from_ref(&folder));
    assert_eq!(plans.len(), 1);
    assert!(plans[0].remove_only);
    let on_disk = status(&folder);
    assert!(!on_disk.hooks_registered && !on_disk.status_line_installed);
    assert_eq!(
        json(&settings_path(&folder)),
        Json::parse(REALISTIC.as_bytes()).unwrap()
    );
    assert!(commands(&folder, "PreToolUse").contains(&"~/bin/guard-bash.sh".to_owned()));
    assert!(home.manager.record().files.is_empty());

    // From then on it is left alone like any store.
    let cleaned = home.snapshot();
    let (plans, _) = home.pass(&accounts, std::slice::from_ref(&folder));
    assert!(plans.is_empty());
    assert_eq!(home.snapshot(), cleaned);
}

#[test]
fn the_consent_card_says_where_in_plain_words() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\me");
    let folder = |dir: &str| run_folder(Path::new(dir), FolderKind::Run);
    let main = folder(r"C:\Users\me\.claude");
    let work = folder(r"C:\Users\me\.claude-work");
    let elsewhere = folder(r"D:\claude\client");
    let mut signed_in = folder(r"C:\Users\me\.claude-side");
    signed_in.identity = Some(Identity {
        email: Some("side@example.com".to_owned()),
        ..Identity::default()
    });
    let nobody = folder(r"C:\Users\me\.claude-new");
    let store = run_folder(Path::new(r"C:\Users\me\.claude-store"), FolderKind::Store);
    let mut untracked = account("old@example.com", &[&work], &[]);
    untracked.is_tracked = false;
    let accounts = [
        account("me@work.com", &[&main, &elsewhere], &[&store]),
        untracked,
    ];
    let folders = [main, work, elsewhere, signed_in, nobody, store];

    let lines: Vec<String> = consent_files(&accounts, &folders, &paths)
        .iter()
        .map(consent_line)
        .collect();
    assert_eq!(
        lines,
        [
            r"%USERPROFILE%\.claude\settings.json (me@work.com)",
            r"D:\claude\client\settings.json (me@work.com)",
            r"%USERPROFILE%\.claude-side\settings.json (side@example.com)",
            r"%USERPROFILE%\.claude-new\settings.json",
        ]
    );
}

// ---- What the design adds ----

/// Two folders whose settings.json is one file: it is written once, and
/// keeps our hooks while either folder is tracked.
#[cfg(unix)]
#[test]
fn a_shared_settings_file_is_written_once_and_kept_while_a_tracked_folder_uses_it() {
    let mut home = home();
    let first = home.folder(".claude", FolderKind::Run, Some(r#"{"model":"opus"}"#));
    let second = home.folder(".claude-work", FolderKind::Run, None);
    std::os::unix::fs::symlink(settings_path(&first), settings_path(&second)).unwrap();
    let folders = [first.clone(), second.clone()];
    let mut accounts = [
        account("me@home.com", &[&first], &[]),
        account("me@work.com", &[&second], &[]),
    ];

    home.grant_consent();
    let (plans, outcomes) = home.pass(&accounts, &folders);
    assert_eq!(plans.len(), 2);
    assert_eq!(outcomes[0].result, Ok(InstallChange::Written));
    assert_eq!(outcomes[1].result, Ok(InstallChange::Unchanged));
    assert!(outcomes[1].entry.is_none());
    // One set of entries, the first folder's.
    assert_eq!(commands(&first, "Stop"), [hook_command(&first)]);
    assert!(fs::symlink_metadata(settings_path(&second))
        .unwrap()
        .file_type()
        .is_symlink());
    // One file, one record entry.
    assert_eq!(home.manager.record().files.len(), 1);
    assert_eq!(
        home.manager.record().files[0].settings_path,
        settings_path(&first)
    );
    assert!(home.manager.folder_status(&second.id).hooks_registered);
    // And the next pass doesn't take turns rewriting it.
    let shared = fs::read(settings_path(&first)).unwrap();
    let (_, outcomes) = home.pass(&accounts, &folders);
    assert!(changed_folders(&outcomes).is_empty());
    assert_eq!(fs::read(settings_path(&first)).unwrap(), shared);

    // The first folder untracked: the file keeps our hooks, for the second.
    accounts[0].is_tracked = false;
    let (plans, outcomes) = home.pass(&accounts, &folders);
    assert_eq!(plans.iter().filter(|plan| plan.remove_only).count(), 1);
    assert_eq!(outcomes[0].result, Ok(InstallChange::Unchanged));
    assert_eq!(commands(&second, "Stop"), [hook_command(&second)]);
    assert!(status(&second).hooks_installed);
    let record = home.manager.record();
    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].folder, second.id);
    assert_eq!(record.files[0].settings_path, settings_path(&first));

    // Both untracked: out, and the link stays a link.
    accounts[1].is_tracked = false;
    home.pass(&accounts, &folders);
    assert!(!status(&first).hooks_registered);
    assert_eq!(
        json(&settings_path(&first)),
        Json::parse(br#"{"model":"opus"}"#).unwrap()
    );
    assert!(fs::symlink_metadata(settings_path(&second))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(home.manager.record().files.is_empty());
}

#[test]
fn hooks_off_after_consent_removes_everywhere() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let work = home.folder(".claude-work", FolderKind::Run, None);
    let gone = home.folder(".claude-gone", FolderKind::Run, None);
    let all = [main.clone(), work.clone(), gone.clone()];
    let mut accounts = [
        account("me@home.com", &[&main], &[]),
        account("me@work.com", &[&work], &[]),
        account("me@gone.com", &[&gone], &[]),
    ];
    // An untracked account's folder is cleaned like the others.
    accounts[1].is_tracked = false;
    home.grant_consent();
    home.pass(&accounts, &all);
    assert_eq!(home.manager.record().files.len(), 2);

    // The registry forgot a folder without taking ours out: with hooks on
    // it is left alone, since a folder may merely not be listed yet.
    let known = [main.clone(), work.clone()];
    home.manager.retain_folders(&known);
    let (plans, _) = home.pass(&accounts[..2], &known);
    assert!(plans.iter().all(|plan| plan.folder != gone.id));
    assert!(status(&gone).hooks_installed);

    // Off: every run folder, and what only the record still names.
    home.settings.hooks_enabled = false;
    let (plans, outcomes) = home.pass(&accounts[..2], &known);
    assert!(plans.iter().all(|plan| plan.remove_only));
    let planned: Vec<&AccountId> = plans.iter().map(|plan| &plan.folder).collect();
    assert_eq!(planned, [&main.id, &gone.id]);
    assert_eq!(
        changed_folders(&outcomes),
        vec![main.id.clone(), gone.id.clone()]
    );
    for folder in &all {
        let on_disk = status(folder);
        assert!(
            !on_disk.hooks_registered && !on_disk.status_line_installed,
            "{}",
            folder.id
        );
        assert!(!hook_copy_path(&folder.config_dir).exists());
    }
    assert_eq!(
        json(&settings_path(&main)),
        Json::parse(REALISTIC.as_bytes()).unwrap()
    );
    assert!(home.manager.record().files.is_empty());
    assert_eq!(
        home.manager.folder_status(&main.id).last_outcome,
        Some(InstallChange::Removed)
    );

    // Nothing is left to plan, and nothing is written again.
    let off = home.snapshot();
    let (plans, _) = home.pass(&accounts[..2], &known);
    assert!(plans.is_empty());
    assert_eq!(home.snapshot(), off);

    // On again: back in.
    home.settings.hooks_enabled = true;
    home.pass(&accounts[..2], &known);
    assert!(status(&main).hooks_installed);
    assert!(!status(&work).hooks_registered);
}

/// The status line switch: off gives the user's own back and leaves the
/// hooks in.
#[test]
fn the_status_line_switch_is_the_plans_wish() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let folders = [main.clone()];
    let accounts = [account("me@home.com", &[&main], &[])];
    home.grant_consent();
    home.pass(&accounts, &folders);
    assert!(status(&main).status_line_installed);

    home.settings.status_line_integration = false;
    let (plans, _) = home.pass(&accounts, &folders);
    assert_eq!(plans[0].status_line, StatusLineIntent::Unwrap);
    let on_disk = status(&main);
    assert!(on_disk.hooks_installed && !on_disk.status_line_installed);
    assert_eq!(
        status_line_command(&main).as_deref(),
        Some("~/.claude/statusline.sh")
    );
    let known = home.manager.folder_status(&main.id);
    assert!(!known.status_line_installed);
    assert_eq!(known.status_line_left_alone, None);
    assert!(!home.manager.record().files[0].status_line);
}

/// A folder whose path no hook command can carry is not hooked, and says so.
#[test]
fn a_folder_no_command_can_carry_is_not_hooked() {
    let mut home = home();
    let spaced = home.folder("my claude", FolderKind::Run, Some(r#"{"model":"opus"}"#));
    let folders = [spaced.clone()];
    let accounts = [account("me@home.com", &[&spaced], &[])];
    home.grant_consent();
    let before = home.snapshot();

    let (plans, outcomes) = home.pass(&accounts, &folders);
    let reason = not_hookable_reason(&home.facts);
    assert_eq!(plans[0].form, CommandForm::NotPossible(reason.clone()));
    assert_eq!(outcomes[0].result, Err(reason.clone()));
    assert_eq!(home.snapshot(), before);
    let known = home.manager.folder_status(&spaced.id);
    assert_eq!(known.not_hookable, Some(reason));
    assert_eq!(known.last_error, None);
    assert!(!known.hooks_registered);
    assert!(home.manager.record().files.is_empty());
    assert_eq!(
        home.manager.folders_missing_hooks(&accounts, &folders),
        vec![spaced.id.clone()]
    );
}

/// The form follows the versions seen: exec only when every Claude Code
/// here is known to run it.
#[test]
fn the_exec_form_is_planned_only_when_every_version_allows_it() {
    let mut home = home();
    home.facts = ClaudeCodeFacts {
        exec_form_min: Some("2.1.150".to_owned()),
        versions: Vec::new(),
    };
    let main = home.folder(".claude", FolderKind::Run, None);
    let spaced = home.folder("my claude", FolderKind::Run, None);
    let folders = [main.clone(), spaced.clone()];
    let accounts = [account("me@home.com", &[&main, &spaced], &[])];
    home.grant_consent();
    let now = home.clock.now();
    home.manager.start(now);

    let plans = home.plan(&accounts, &folders);
    assert_eq!(plans[0].form, exec_form(&hook_copy_path(&main.config_dir)));
    // The exec form carries any path.
    assert_eq!(
        plans[1].form,
        exec_form(&hook_copy_path(&spaced.config_dir))
    );
    home.manager
        .pass_started(&home.settings, &home.versions, &home.facts, now);
    home.carry_out(&plans, &folders);
    assert_eq!(
        home.manager.folder_status(&spaced.id).form.as_deref(),
        Some("exec")
    );
    assert!(status(&spaced).hooks_installed);
    assert_eq!(
        home.manager.record().entry_for(&spaced.id).unwrap().form,
        "exec"
    );

    // A copy whose version can't be told puts everyone back on strings, at
    // once.
    home.versions
        .push(sighting(VersionSource::BundledVsCode, None));
    let now = home.clock.now();
    assert!(home.manager.note_versions(&home.versions, &home.facts, now));
    assert!(home.manager.pass_due(now));
    let plans = home.plan(&accounts, &folders);
    assert_eq!(plans[0].form, CommandForm::Text(hook_command(&main)));
    assert!(matches!(plans[1].form, CommandForm::NotPossible(_)));
    assert_eq!(
        plans[1].form,
        CommandForm::NotPossible(
            "Can't be hooked here: its path needs Claude Code 2.1.150 or later everywhere on this PC"
                .to_owned()
        )
    );
}

// ---- When passes run ----

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(START_MS) + Duration::from_secs(seconds)
}

#[test]
fn passes_run_at_start_after_changes_settle_and_every_ten_minutes() {
    let facts = ClaudeCodeFacts::compiled_in();
    let settings = ControlSettings {
        hook_consent: Some(true),
        ..ControlSettings::default()
    };
    let versions = [sighting(VersionSource::Binary, Some("2.1.280"))];
    let mut manager = HookManager::new();

    // Nothing before the start.
    assert_eq!(manager.next_pass_at(), None);
    manager.accounts_changed(at(0));
    manager.request_pass(at(0));
    assert!(!manager.note_versions(&versions, &facts, at(0)));
    assert!(!manager.pass_due(at(1_000)));

    // The start: a pass at once, then every ten minutes.
    manager.start(at(0));
    assert_eq!(manager.next_pass_at(), Some(at(0)));
    assert!(manager.pass_due(at(0)));
    manager.pass_started(&settings, &versions, &facts, at(0));
    assert!(!manager.pass_due(at(599)));
    assert_eq!(manager.next_pass_at(), Some(at(0) + RECHECK_INTERVAL));
    assert!(manager.pass_due(at(600)));
    manager.pass_started(&settings, &versions, &facts, at(601));
    assert_eq!(manager.next_pass_at(), Some(at(1_201)));
    // Starting twice asks for nothing new.
    manager.start(at(602));
    assert_eq!(manager.next_pass_at(), Some(at(1_201)));

    // Account changes: one pass, a second after the last of a burst.
    manager.accounts_changed(at(700));
    assert_eq!(manager.next_pass_at(), Some(at(701)));
    manager.accounts_changed(at(700) + Duration::from_millis(900));
    assert!(!manager.pass_due(at(701)));
    assert!(!manager.pass_due(at(701) + Duration::from_millis(899)));
    assert!(manager.pass_due(at(701) + Duration::from_millis(900)));
    manager.pass_started(&settings, &versions, &facts, at(702));
    assert!(!manager.pass_due(at(703)));
    assert_eq!(manager.next_pass_at(), Some(at(1_302)));

    // A change during a pass that is due anyway is covered by it.
    manager.accounts_changed(at(1_301) + Duration::from_millis(500));
    assert!(manager.pass_due(at(1_302)));
    manager.pass_started(&settings, &versions, &facts, at(1_302));
    assert!(!manager.pass_due(at(1_303)));

    // The user's own switches: at once.
    manager.request_pass(at(1_400));
    assert!(manager.pass_due(at(1_400)));
    manager.pass_started(&settings, &versions, &facts, at(1_400));

    // Stopped: nothing is due any more.
    manager.stop();
    assert_eq!(manager.next_pass_at(), None);
    assert!(!manager.pass_due(at(9_999)));
}

#[test]
fn a_lower_version_asks_for_a_pass_only_when_hooks_were_written_for_a_newer_one() {
    let facts = ClaudeCodeFacts::compiled_in();
    let on = ControlSettings {
        hook_consent: Some(true),
        ..ControlSettings::default()
    };
    let binary = sighting(VersionSource::Binary, Some("2.1.280"));
    let older = sighting(VersionSource::StatusLine, Some("2.1.50"));
    let newer = sighting(VersionSource::Registry, Some("2.1.300"));
    let unknown = sighting(VersionSource::BundledDesktop, None);

    let mut manager = HookManager::new();
    manager.start(at(0));
    // Before any pass wrote hooks there is nothing to take back.
    assert!(!manager.note_versions(&[binary.clone(), older.clone()], &facts, at(0)));

    manager.pass_started(&on, std::slice::from_ref(&binary), &facts, at(0));
    // The same, a newer and an unknown version: nothing.
    assert!(!manager.note_versions(std::slice::from_ref(&binary), &facts, at(1)));
    assert!(!manager.note_versions(&[binary.clone(), newer.clone()], &facts, at(1)));
    assert!(!manager.note_versions(&[binary.clone(), unknown.clone()], &facts, at(1)));
    assert!(!manager.pass_due(at(1)));
    // A lower one: a pass now.
    assert!(manager.note_versions(&[binary.clone(), older.clone()], &facts, at(2)));
    assert_eq!(manager.next_pass_at(), Some(at(2)));
    manager.pass_started(&on, &[binary.clone(), older.clone()], &facts, at(2));
    // Once written for it, the same sighting asks for nothing.
    assert!(!manager.note_versions(&[binary.clone(), older.clone()], &facts, at(3)));
    assert!(!manager.pass_due(at(3)));

    // Hooks written for the baseline (no version known): nothing lower.
    let mut baseline = HookManager::new();
    baseline.start(at(0));
    baseline.pass_started(&on, &[], &facts, at(0));
    assert!(!baseline.note_versions(std::slice::from_ref(&older), &facts, at(1)));

    // A pass that wrote nothing (no consent, hooks off, --no-install).
    for (settings, flags) in [
        (ControlSettings::default(), DevFlags::default()),
        (
            ControlSettings {
                hook_consent: Some(true),
                hooks_enabled: false,
                ..ControlSettings::default()
            },
            DevFlags::default(),
        ),
        (
            on.clone(),
            DevFlags {
                no_install: true,
                ..DevFlags::default()
            },
        ),
    ] {
        let mut idle = HookManager::configured(PathBuf::from("hook.exe"), &flags);
        idle.start(at(0));
        idle.pass_started(&settings, std::slice::from_ref(&binary), &facts, at(0));
        assert!(!idle.note_versions(&[binary.clone(), older.clone()], &facts, at(1)));
    }
}

// ---- The install record ----

#[test]
fn the_record_is_built_from_outcomes_and_round_trips() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let work = home.folder(".claude-work", FolderKind::Run, None);
    let folders = [main.clone(), work.clone()];
    let accounts = [
        account("me@home.com", &[&main], &[]),
        account("me@work.com", &[&work], &[]),
    ];
    home.grant_consent();

    let plans = home.plan(&accounts, &folders);
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &home.clock, &SETUP);
    let installed_at = home.clock.now();
    assert!(home.manager.absorb_outcomes(&plans, &outcomes));
    let record = home.manager.record().clone();
    assert_eq!(record.files.len(), 2);
    let entry = record.entry_for(&main.id).unwrap();
    assert_eq!(entry.settings_path, settings_path(&main));
    assert_eq!(entry.form, "string");
    assert_eq!(entry.command, hook_command(&main));
    assert_eq!(entry.hook_copy, Some(hook_copy_path(&main.config_dir)));
    assert!(entry.status_line);
    assert_eq!(entry.installed_at, installed_at);

    // Through hook-install.json and back.
    let bytes = HookInstallFile::from_model(&record).encode();
    let read = HookInstallFile::parse(&bytes).unwrap();
    assert_eq!(read.version, 1);
    assert_eq!(read.to_model(), record);
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains("\"settingsPath\"") && text.contains("\"installedAt\""),
        "{text}"
    );

    // A pass that changed nothing leaves the record, dates included.
    home.clock.advance(Duration::from_secs(600));
    let plans = home.plan(&accounts, &folders);
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &home.clock, &SETUP);
    assert!(!home.manager.absorb_outcomes(&plans, &outcomes));
    assert_eq!(home.manager.record(), &record);

    // A failed pass keeps the entry: ours may still be in the file.
    write_file(&settings_path(&work), b"not json");
    let plans = home.plan(&accounts, &folders);
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &home.clock, &SETUP);
    assert!(outcomes[1].result.is_err());
    assert!(!home.manager.absorb_outcomes(&plans, &outcomes));
    let known = home.manager.folder_status(&work.id);
    assert_eq!(known.last_outcome, None);
    assert_eq!(known.last_error, outcomes[1].result.clone().err());
    assert_eq!(known.not_hookable, None);

    // A manager started from the record alone (a new launch) knows what to
    // remove when hooks go off, before any folder was read.
    let mut next = HookManager::configured(
        home.root.join("app").join(HOOK_EXE_NAME),
        &DevFlags::default(),
    );
    next.set_record(
        HookInstallFile::parse(&HookInstallFile::from_model(&record).encode())
            .unwrap()
            .to_model(),
    );
    home.settings.hooks_enabled = false;
    let plans = next.plan(&[], &[], &home.settings, &home.versions, &home.facts);
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|plan| plan.remove_only));
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &home.clock, &SETUP);
    assert_eq!(outcomes[0].result, Ok(InstallChange::Removed));
    // The unparseable file is refused and stays in the record.
    assert!(outcomes[1].result.is_err());
    assert!(next.absorb_outcomes(&plans, &outcomes));
    assert_eq!(next.record().files.len(), 1);
    assert_eq!(next.record().files[0].folder, work.id);
    assert!(!status(&main).hooks_registered);
}

/// `uninstall-hooks` and `Job::Uninstall`: the record plus the folders
/// found, whatever the settings say.
#[test]
fn uninstall_all_removes_from_the_record_and_the_folders_found() {
    let mut home = home();
    let main = home.folder(".claude", FolderKind::Run, Some(REALISTIC));
    let work = home.folder(".claude-work", FolderKind::Run, None);
    let store = home.folder(".claude-store", FolderKind::Store, None);
    let folders = [main.clone(), work.clone()];
    let accounts = [
        account("me@home.com", &[&main], &[]),
        account("me@work.com", &[&work], &[]),
    ];
    home.grant_consent();
    home.pass(&accounts, &folders);
    let record = home.manager.record().clone();
    let store_before = snapshot_dir(&store.config_dir).unwrap();

    // Discovery finds `main` and the store; `work` is only in the record,
    // and its folder is gone from a third entry.
    let mut stale = record.clone();
    let mut missing = record.files[0].clone();
    missing.folder = AccountId("gone".to_owned());
    missing.settings_path = home.root.join("gone").join("settings.json");
    missing.hook_copy = Some(hook_copy_path(&home.root.join("gone")));
    stale.files.push(missing);
    let found = [main.clone(), store.clone()];
    let (outcomes, left) = uninstall_all(&stale, &found, &StdSecureFiles, &home.clock);
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].result, Ok(InstallChange::Removed));
    assert_eq!(outcomes[1].result, Ok(InstallChange::Removed));
    assert!(outcomes[2].result.is_err());
    assert!(left.files.is_empty());
    for folder in &folders {
        assert!(!status(folder).hooks_registered, "{}", folder.id);
        assert!(!hook_copy_path(&folder.config_dir).exists());
    }
    assert_eq!(
        json(&settings_path(&main)),
        Json::parse(REALISTIC.as_bytes()).unwrap()
    );
    assert_eq!(snapshot_dir(&store.config_dir).unwrap(), store_before);
}
