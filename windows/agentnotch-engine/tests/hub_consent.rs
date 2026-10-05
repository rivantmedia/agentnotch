//! Consent, install and removal through the live hub (design §4.3): the
//! Mac's `ConsentGateTests` (A2_ConsentGateTests.swift) and `PP_HooksTests`
//! over a throwaway home, driven only through the hub's calls and its own
//! schedule, plus what the Windows hub adds: nothing at all is written
//! while installs are off for the run (`--no-install`) or sealed,
//! `claude --version` runs only after the yes, `install-hooks` works on a
//! hub that never started, and `uninstall-hooks` puts every settings.json
//! back as it was.
//!
//! The Superpowered Vibe Notch cases have no Windows counterpart; where a
//! Mac test checks another app's entries, the official Codenotch app's
//! (`codenotch-hook.exe`) stand in for them.

mod accounts_support;
mod hub_support;

use accounts_support::{Home, BIIOS, BIIOS_UUID, PARAS, PARAS_UUID};
use agentnotch_engine::accounts::DISCOVERY_INTERVAL;
use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::hooks::apply::{read_status, Setup, STORE_MARKER_NAME};
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, CallError, Hub, HubConfig};
use agentnotch_engine::model::{AccountAction, SettingsSnapshot};
use agentnotch_engine::runtime_types::FolderHookStatus;
use agentnotch_engine::testkit::{self, snapshot_dir, StdSecureFiles};
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// The official app's hook, as its installer writes it.
const OFFICIAL_HOOK: &str = "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe";

/// A settings.json as users have them: another tool's hook, the official
/// app's, and a status line.
fn realistic() -> String {
    format!(
        r#"{{
  "model": "opus",
  "permissions": {{"allow": ["Bash(npm test:*)"], "deny": []}},
  "hooks": {{
    "PreToolUse": [
      {{"matcher": "Bash", "hooks": [{{"type": "command", "command": "~/bin/guard-bash.sh", "timeout": 30}}]}},
      {{"matcher": "*", "hooks": [{{"type": "command", "command": "{OFFICIAL_HOOK} PreToolUse"}}]}}
    ]
  }},
  "statusLine": {{"type": "command", "command": "~/.claude/statusline.sh", "padding": 2}}
}}
"#
    )
}

const RUN_FOLDERS: [&str; 4] = [
    ".claude",
    ".claude-windows/1bf3e8f92b11",
    ".claude-windows/801f9dd51396",
    ".claude-windows/b9fbb9ecd7cb",
];
const STORES: [&str; 3] = [".claude-paras", ".claude-paras-rivant-in", ".claude-claude"];

/// Settings that keep the probe off: the only child a hook test may see
/// is `claude --version`.
fn no_probes() -> Value {
    json!({"usageProbeIntervalMinutes": 0})
}

fn consented() -> Value {
    json!({"usageProbeIntervalMinutes": 0, "hookConsent": true, "hooksEnabled": true,
           "hookConsentScope": 2})
}

/// A home, the app's files, and a live hub over them (not started).
struct World {
    // Declared first: dropped (stopped) before the folders go.
    hub: TestHub,
    home: Home,
}

impl World {
    fn new(build: impl FnOnce(&Home), settings: Value) -> World {
        World::with(build, settings, |_| {})
    }

    fn with(
        build: impl FnOnce(&Home),
        settings: Value,
        configure: impl FnOnce(&mut HubConfig),
    ) -> World {
        let home = Home::new();
        build(&home);
        prepare(&home, &settings);
        let hub = TestHub::over(
            &base_of(&home),
            RuntimeOptions::default(),
            configure,
            |_| {},
        );
        World { hub, home }
    }

    fn started(build: impl FnOnce(&Home), settings: Value) -> World {
        let world = World::new(build, settings);
        world.hub.hub.start().expect("the hub starts");
        world.idle();
        world
    }

    fn dir(&self, relative: &str) -> PathBuf {
        PathBuf::from(self.home.path(relative))
    }

    fn status(&self, relative: &str) -> FolderHookStatus {
        read_status(&self.dir(relative), &StdSecureFiles, &Setup::detect())
    }

    fn settings_json(&self, relative: &str) -> Value {
        let bytes = std::fs::read(self.dir(relative).join("settings.json")).expect("settings.json");
        serde_json::from_slice(&bytes).expect("settings.json parses")
    }

    fn settings_bytes(&self, relative: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir(relative).join("settings.json")).ok()
    }

    /// A folder's settings.json as `files` names it.
    fn key(&self, relative: &str) -> PathBuf {
        self.dir(relative)
            .join("settings.json")
            .strip_prefix(&self.home.roots.home)
            .expect("inside the home")
            .to_path_buf()
    }

    /// Both settings.json files are back: `~\.claude-work`'s (where we
    /// only wrapped the status line) byte for byte; `~\.claude`'s, whose
    /// `PreToolUse` list we spliced into, as the same JSON with the other
    /// keys' bytes kept (the list itself is written out again).
    fn assert_restored(&self, before: &BTreeMap<PathBuf, Vec<u8>>) {
        assert_eq!(
            self.settings_bytes(".claude-work").as_ref(),
            before.get(&self.key(".claude-work"))
        );
        let now = self
            .settings_bytes(".claude")
            .expect("~\\.claude\\settings.json");
        let was = before.get(&self.key(".claude")).expect("it was there");
        let parse = |bytes: &[u8]| serde_json::from_slice::<Value>(bytes).expect("JSON");
        assert_eq!(parse(&now), parse(was));
        let text = String::from_utf8_lossy(&now);
        assert!(
            text.contains(r#""statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2}"#),
            "{text}"
        );
    }

    fn call(&self, call: Call) -> Result<Value, CallError> {
        self.hub.hub.call(call)
    }

    fn settings(&self) -> SettingsSnapshot {
        let value = self.call(Call::Settings).expect("the settings");
        serde_json::from_value(value).expect("the settings' shape")
    }

    /// Waits until no hook write runs or waits (and the read-back of what
    /// the last one wrote has been asked for).
    fn idle(&self) {
        assert!(
            eventually(|| !self.settings().hooks.busy),
            "the hooks' writes never finished"
        );
        self.hub.sync();
    }

    fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot_dir(&self.home.roots.home).expect("the home's files")
    }

    /// The identity id of the account signed in as `email`.
    fn account_id(&self, email: &str) -> String {
        self.settings()
            .accounts
            .into_iter()
            .find(|row| row.identity_line.contains(email))
            .map(|row| row.identity_id)
            .unwrap_or_else(|| panic!("no account for {email}"))
    }

    fn track(&self, email: &str, on: bool) {
        let id = self.account_id(email);
        self.call(Call::Account {
            action: AccountAction::Track { id, on },
        })
        .expect("tracking changes");
        self.idle();
    }

    /// Lets the hub's own schedule run: the clock moves on and `an-core`
    /// looks at it.
    fn advance(&self, by: Duration) {
        self.hub.handles.clock.advance(by);
        self.hub.sync();
    }

    /// `claude --version` runs the hub started.
    fn version_runs(&self) -> usize {
        self.hub
            .handles
            .runner
            .spawned()
            .iter()
            .filter(|spec| spec.args.iter().any(|arg| arg == "--version"))
            .count()
    }
}

fn base_of(home: &Home) -> PathBuf {
    home.roots
        .home
        .parent()
        .expect("the home sits in the test's root")
        .to_path_buf()
}

/// The hook exe beside the app, and `control-settings.json`.
fn prepare(home: &Home, settings: &Value) {
    let install = home.roots.install_dir.clone().expect("an install folder");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("agentnotch-hook.exe"), b"MZ not really an exe").unwrap();
    std::fs::create_dir_all(&home.roots.support).unwrap();
    std::fs::write(
        home.roots.support.join("control-settings.json"),
        serde_json::to_vec(settings).unwrap(),
    )
    .unwrap();
}

/// A config folder signed in as `email`, with a session as Claude Code
/// leaves one, and the settings.json given.
fn account(home: &Home, folder: &str, email: &str, uuid: &str, settings: Option<&str>) {
    home.write(&format!("{folder}/sessions/1.json"), "{}");
    home.write_json(
        &format!("{folder}/.claude.json"),
        &home.login(uuid, email, None),
    );
    if let Some(settings) = settings {
        home.write(&format!("{folder}/settings.json"), settings);
    }
}

/// `~\.claude` (with `~\.claude.json`) and `~\.claude-work`.
fn two_accounts(main_settings: Option<&str>, work_settings: Option<&str>) -> impl FnOnce(&Home) {
    let main = main_settings.map(str::to_owned);
    let work = work_settings.map(str::to_owned);
    move |home: &Home| {
        home.write(".claude/sessions/1.json", "{}");
        home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
        if let Some(main) = &main {
            home.write(".claude/settings.json", main);
        }
        account(home, ".claude-work", BIIOS, BIIOS_UUID, work.as_deref());
    }
}

fn user_layout(home: &Home) {
    home.build_user_layout(true, true);
}

fn events_of(settings: &Value) -> BTreeSet<String> {
    settings["hooks"]
        .as_object()
        .map(|hooks| hooks.keys().cloned().collect())
        .unwrap_or_default()
}

/// A Claude Code bundled with VS Code, known by its folder's name.
fn bundled(home: &Home, version: &str) {
    home.mkdir(&format!(
        ".vscode/extensions/anthropic.claude-code-{version}-win32-x64"
    ));
}

// ---- A2_ConsentGateTests ----

#[test]
fn nothing_is_written_before_consent() {
    let world = World::started(two_accounts(Some(&realistic()), None), no_probes());
    let before = world.files();

    // Every folder's settings.json was read for the setup card, the
    // official app's entries in `~\.claude` among them.
    assert!(eventually(|| {
        let setup = world.settings().setup;
        setup.codenotch_hooks_folders == vec![world.home.path(".claude")]
    }));
    let setup = world.settings().setup;
    assert!(setup.needs_hook_consent);
    assert_eq!(world.settings().accounts.len(), 2);
    let mut files: Vec<String> = setup.consent_files.iter().map(|f| f.path.clone()).collect();
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");

    // A reinstall asks first; switches and time write nothing either.
    let refused = world
        .call(Call::HooksReinstall { account_id: None })
        .unwrap_err();
    assert_eq!(refused.code, "refused");
    world
        .call(Call::StatusLineEnabled { on: false })
        .expect("the switch flips");
    world.advance(Duration::from_secs(15 * 60));
    world.idle();
    assert_eq!(world.files(), before);
    assert_eq!(
        world.version_runs(),
        0,
        "no claude --version before the yes"
    );

    // "Not now" is remembered and still writes nothing.
    world
        .call(Call::HookConsent { grant: false })
        .expect("not now");
    world.advance(Duration::from_secs(15 * 60));
    world.idle();
    assert_eq!(world.files(), before);
    assert!(eventually(
        || world.hub.settings_file()["hookConsent"] == json!(false)
    ));
    assert!(!world.settings().setup.needs_hook_consent);
    assert_eq!(world.version_runs(), 0);
}

#[test]
fn consent_installs_into_every_tracked_account() {
    let world = World::started(
        |home: &Home| {
            two_accounts(Some(&realistic()), None)(home);
            bundled(home, "2.1.280");
        },
        no_probes(),
    );
    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();

    for folder in [".claude", ".claude-work"] {
        let status = world.status(folder);
        assert!(status.hooks_installed, "{folder}: {status:?}");
        // Someone else's status line is wrapped only where Git Bash can
        // run the wrapper; else it is left alone, and says why.
        assert!(
            status.status_line_installed
                || (!Setup::detect().git_bash && status.status_line_left_alone.is_some()),
            "{folder}: {status:?}"
        );
    }
    assert!(world.status(".claude-work").status_line_installed);
    assert!(eventually(
        || world.hub.settings_file()["hookConsent"] == json!(true)
    ));
    let settings = world.settings();
    assert!(!settings.setup.needs_hook_consent);
    assert_eq!(settings.hooks.claude_version.as_deref(), Some("2.1.280"));
    // The official app's entries stay (they are removed only on a click).
    assert_eq!(
        settings.setup.codenotch_hooks_folders,
        vec![world.home.path(".claude")]
    );
    let kept = std::fs::read_to_string(world.dir(".claude").join("settings.json")).unwrap();
    assert!(kept.contains("codenotch-hook.exe") && kept.contains("guard-bash.sh"));
    // The notice names both folders.
    assert!(settings.hooks.last_change.is_some());
}

#[test]
fn untracking_and_forgetting_take_our_hooks_out() {
    let world = World::started(
        two_accounts(
            Some(r#"{"model":"opus"}"#),
            Some(r#"{"statusLine":{"type":"command","command":"echo hi"}}"#),
        ),
        no_probes(),
    );
    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();
    assert!(world.status(".claude-work").hooks_installed);

    // Untrack: ours come out, the status line is restored.
    world.track(BIIOS, false);
    let status = world.status(".claude-work");
    assert!(!status.hooks_registered && !status.status_line_installed);
    assert_eq!(
        world.settings_json(".claude-work")["statusLine"]["command"],
        json!("echo hi")
    );
    // Another pass leaves it alone.
    world
        .call(Call::HooksReinstall { account_id: None })
        .expect("a pass");
    world.idle();
    assert!(!world.status(".claude-work").hooks_registered);

    // Track again: back in. Forget: out, and gone for good.
    world.track(BIIOS, true);
    assert!(world.status(".claude-work").hooks_installed);
    let id = world.account_id(BIIOS);
    world
        .call(Call::Account {
            action: AccountAction::Forget { id },
        })
        .expect("forgotten");
    world.idle();
    assert!(!world.status(".claude-work").hooks_registered);
    let listed = |world: &World| {
        world
            .settings()
            .accounts
            .iter()
            .any(|row| row.identity_line.contains(BIIOS))
    };
    assert!(!listed(&world));
    assert!(world.status(".claude").hooks_installed);

    // A later discovery doesn't bring it back.
    world.advance(DISCOVERY_INTERVAL + Duration::from_secs(1));
    world.idle();
    assert!(!listed(&world));
    assert!(!world.status(".claude-work").hooks_registered);
}

#[test]
fn an_older_client_lowers_the_registered_events() {
    let world = World::started(
        |home: &Home| {
            home.write(".claude/sessions/1.json", "{}");
            home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
            bundled(home, "2.1.280");
        },
        no_probes(),
    );
    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();
    assert!(events_of(&world.settings_json(".claude")).contains("PermissionDenied"));

    // An older copy shows up: the hooks are written for it.
    bundled(&world.home, "2.1.50");
    world
        .call(Call::HooksReinstall { account_id: None })
        .expect("a pass");
    world.idle();
    let lowered = events_of(&world.settings_json(".claude"));
    assert!(!lowered.contains("PermissionDenied"), "{lowered:?}");
    assert!(!lowered.contains("TaskCreated"), "{lowered:?}");
    assert!(lowered.contains("TaskCompleted"), "{lowered:?}");

    // A newer one changes nothing.
    let before = std::fs::read(world.dir(".claude").join("settings.json")).unwrap();
    bundled(&world.home, "2.1.300");
    world
        .call(Call::HooksReinstall { account_id: None })
        .expect("a pass");
    world.idle();
    assert_eq!(
        std::fs::read(world.dir(".claude").join("settings.json")).unwrap(),
        before
    );
}

// ---- PP_HooksTests (the Windows layout of Claude Parallel Profiles) ----

#[test]
fn consent_installs_into_the_four_run_folders_only() {
    let world = World::started(user_layout, no_probes());
    let mut expected: Vec<String> = RUN_FOLDERS
        .iter()
        .map(|folder| {
            world
                .dir(folder)
                .join("settings.json")
                .display()
                .to_string()
        })
        .collect();
    expected.sort();
    let mut listed: Vec<String> = world
        .settings()
        .setup
        .consent_files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    listed.sort();
    assert_eq!(listed.len(), expected.len(), "{listed:?}");
    let before = world.files();

    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();
    for folder in RUN_FOLDERS {
        let status = world.status(folder);
        assert!(
            status.hooks_installed && status.status_line_installed,
            "{folder}: {status:?}"
        );
    }
    // Stores and the shared history: not a byte changed or added.
    let after = world.files();
    for (key, value) in &after {
        let path = key.to_string_lossy().replace('\\', "/");
        let protected = STORES
            .iter()
            .any(|store| path.starts_with(&format!("{store}/")))
            || path.starts_with(".claude-shared")
            || path.ends_with(".claude.json")
            || path.ends_with(STORE_MARKER_NAME)
            || path.ends_with(".manifest.json");
        if protected {
            assert_eq!(before.get(key), Some(value), "{path}");
        }
    }
    for store in STORES {
        assert!(!world.dir(store).join("settings.json").exists(), "{store}");
        assert!(!world.dir(store).join("hooks").exists(), "{store}");
    }
    assert!(!world.dir(".claude-shared/hooks").exists());
    assert!(!world.dir(".claude-shared/settings.json").exists());

    // Per account: hooks in every run folder (~\.claude is paras's).
    let paras = world
        .settings()
        .accounts
        .into_iter()
        .find(|row| row.identity_line.contains(PARAS))
        .expect("paras's account");
    let run_rows: Vec<_> = paras
        .folders
        .iter()
        .filter(|folder| folder.role != "Account store")
        .collect();
    assert_eq!(run_rows.len(), 3, "{:?}", paras.folders);
}

#[test]
fn a_new_window_gets_the_hooks_on_the_next_pass() {
    let world = World::started(user_layout, no_probes());
    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();

    world
        .home
        .add_window("0a1b2c3d4e5f", BIIOS_UUID, BIIOS, None);
    // The next discovery finds it; a pass follows once the accounts settle.
    world.advance(DISCOVERY_INTERVAL + Duration::from_secs(1));
    assert!(eventually(|| {
        world.advance(Duration::from_secs(2));
        world.status(".claude-windows/0a1b2c3d4e5f").hooks_installed
    }));
    world.idle();
    for folder in [
        ".claude-windows/1bf3e8f92b11",
        ".claude-windows/0a1b2c3d4e5f",
    ] {
        assert!(world.status(folder).hooks_installed, "{folder}");
    }
}

#[test]
fn untracking_an_account_takes_ours_out_of_all_its_run_folders() {
    let world = World::started(user_layout, no_probes());
    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();

    world.track(PARAS, false);
    for folder in [
        ".claude-windows/801f9dd51396",
        ".claude-windows/b9fbb9ecd7cb",
    ] {
        assert!(!world.status(folder).hooks_registered, "{folder}");
    }
    // ~\.claude is whoever the extension mirrored in last: it keeps our
    // hooks while another account is tracked (PP-C5, UX-4).
    assert!(world.status(".claude").hooks_installed);
    assert!(world.status(".claude-windows/1bf3e8f92b11").hooks_installed);

    // Every account untracked: nothing of ours anywhere.
    world.track(BIIOS, false);
    for folder in RUN_FOLDERS {
        assert!(!world.status(folder).hooks_registered, "{folder}");
    }
    world.track(BIIOS, true);
    assert!(world.status(".claude").hooks_installed);

    world
        .call(Call::HooksEnabled { on: false })
        .expect("hooks off");
    world.idle();
    assert!(
        !world
            .status(".claude-windows/1bf3e8f92b11")
            .hooks_registered
    );
    assert!(!world.status(".claude").hooks_registered);
}

/// Whatever a caller thinks, nothing is installed into a store or the
/// shared history: not one named as an extra config folder, not one the
/// manifest alone lists (its marker gone).
#[test]
fn the_installer_refuses_stores_and_the_shared_history() {
    let home_dirs: std::cell::RefCell<Vec<String>> = Default::default();
    let world = World::with(
        |home: &Home| {
            user_layout(home);
            home.remove(&format!(".claude-claude/{STORE_MARKER_NAME}"));
            home_dirs
                .borrow_mut()
                .extend([home.path(".claude-paras"), home.path(".claude-shared")]);
        },
        consented(),
        |cfg: &mut HubConfig| cfg.flags.extra_config_dirs = home_dirs.borrow().clone(),
    );
    world.hub.hub.start().expect("the hub starts");
    world.idle();
    world
        .call(Call::HooksReinstall { account_id: None })
        .expect("a pass");
    world.idle();
    assert!(world.status(".claude").hooks_installed);
    for folder in [".claude-paras", ".claude-claude", ".claude-shared"] {
        assert!(
            !world.dir(folder).join("settings.json").exists(),
            "{folder}"
        );
        assert!(!world.dir(folder).join("hooks").exists(), "{folder}");
    }
}

/// `aSettingsFileThatPredatesItStays`, with the official app's entries in
/// the place of Superpowered Vibe Notch's: taking them out keeps the file,
/// the user's own entries and its other keys.
#[test]
fn a_settings_file_that_predates_it_stays() {
    let world = World::started(two_accounts(Some(&realistic()), None), no_probes());
    assert!(eventually(|| !world
        .settings()
        .setup
        .codenotch_hooks_folders
        .is_empty()));
    let folder = world.home.path(".claude");
    let removed = world
        .call(Call::RemoveCodenotchHooks {
            folder: folder.clone(),
        })
        .expect("removed");
    assert_eq!(removed, json!({"removed": 1}));
    world.idle();
    let kept = std::fs::read_to_string(world.dir(".claude").join("settings.json")).unwrap();
    assert!(kept.contains("guard-bash.sh"), "{kept}");
    assert!(!kept.contains("codenotch-hook"), "{kept}");
    let json: Value = serde_json::from_str(&kept).unwrap();
    assert_eq!(json["model"], json!("opus"));
    assert_eq!(
        json["statusLine"]["command"],
        json!("~/.claude/statusline.sh")
    );
    // Nothing of ours went in (no consent), and the card no longer lists it.
    assert!(!world.status(".claude").hooks_registered);
    assert!(eventually(|| world
        .settings()
        .setup
        .codenotch_hooks_folders
        .is_empty()));

    // An unknown folder is refused, never guessed.
    let unknown = world
        .call(Call::RemoveCodenotchHooks {
            folder: world.home.path(".claude-nope"),
        })
        .unwrap_err();
    assert_eq!(unknown.code, "not_found");
}

// ---- Windows: runs that may not write, the command line ----

/// `--no-install`: no write function runs, whatever is asked; the
/// switches still answer.
#[test]
fn no_install_writes_nothing_whatever_is_asked() {
    let world = World::with(
        |home: &Home| {
            two_accounts(Some(&realistic()), None)(home);
            bundled(home, "2.1.280");
        },
        consented(),
        |cfg: &mut HubConfig| cfg.flags.no_install = true,
    );
    world.hub.hub.start().expect("the hub starts");
    world.idle();
    let before = world.files();
    let folder = world.home.path(".claude");

    world.call(Call::HookConsent { grant: true }).unwrap();
    world.call(Call::StatusLineEnabled { on: false }).unwrap();
    world.call(Call::HooksEnabled { on: false }).unwrap();
    world.call(Call::HooksEnabled { on: true }).unwrap();
    let refused = world
        .call(Call::HooksReinstall { account_id: None })
        .unwrap_err();
    assert_eq!(refused.code, "refused");
    let refused = world
        .call(Call::RemoveCodenotchHooks { folder })
        .unwrap_err();
    assert_eq!(refused.code, "refused");
    world.call(Call::HooksEnabled { on: false }).unwrap();
    world.advance(Duration::from_secs(15 * 60));
    world.idle();

    assert_eq!(world.files(), before);
    assert_eq!(world.version_runs(), 0);
    // Only the app's own settings file was written.
    let writes: Vec<PathBuf> = world
        .hub
        .files
        .writes
        .lock()
        .unwrap()
        .iter()
        .map(|w| w.path.clone())
        .collect();
    assert!(
        writes
            .iter()
            .all(|path| path.starts_with(&world.home.roots.support)),
        "{writes:?}"
    );
    assert!(world.settings().setup.install_disabled);
    // The command line's removal is refused too.
    let flags = DevFlags {
        no_install: true,
        ..DevFlags::default()
    };
    let (platform, _) = testkit::platform(&base_of(&world.home));
    assert!(Hub::uninstall_hooks_with(&world.home.roots, &platform, &flags).is_err());
    assert_eq!(world.files(), before);
}

/// A sealed hub touches no Claude folder, whatever the setup calls say.
#[test]
fn a_sealed_hub_writes_nothing() {
    let home = Home::new();
    two_accounts(Some(&realistic()), None)(&home);
    prepare(&home, &no_probes());
    let before = snapshot_dir(&home.roots.home).unwrap();
    let (platform, handles) = testkit::platform(&base_of(&home));
    let mut cfg = hub_support::live::config(&home.roots);
    cfg.flags.sealed = true;
    let hub = Hub::new(cfg, platform);
    hub.start().unwrap();
    let _ = hub.call(Call::HookConsent { grant: true });
    let _ = hub.call(Call::HooksReinstall { account_id: None });
    let _ = hub.call(Call::HooksEnabled { on: false });
    hub.stop();
    assert_eq!(snapshot_dir(&home.roots.home).unwrap(), before);
    assert!(handles.runner.spawned().is_empty());
    let flags = DevFlags {
        sealed: true,
        ..DevFlags::default()
    };
    let (platform, _) = testkit::platform(&base_of(&home));
    let said = Hub::uninstall_hooks_with(&home.roots, &platform, &flags).unwrap();
    assert!(said.starts_with("Sealed:"), "{said}");
    assert_eq!(snapshot_dir(&home.roots.home).unwrap(), before);
}

/// `claude --version` runs only after the yes (before it, the probe is the
/// only child the app starts).
#[test]
fn versions_are_checked_only_after_consent() {
    let world = World::started(
        |home: &Home| {
            two_accounts(None, None)(home);
            home.write(".local/bin/claude.exe", "");
        },
        no_probes(),
    );
    world.advance(Duration::from_secs(15 * 60));
    world.idle();
    assert_eq!(world.version_runs(), 0);

    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();
    assert_eq!(world.version_runs(), 1);
    assert!(world.status(".claude").hooks_installed);
}

/// `agentnotch.exe install-hooks`: a hub that never started installs (the
/// yes given earlier), and answers once it is done.
#[test]
fn hooks_reinstall_works_on_a_hub_never_started() {
    let world = World::new(two_accounts(Some(&realistic()), None), consented());
    world
        .call(Call::HooksReinstall { account_id: None })
        .expect("installed");
    for folder in [".claude", ".claude-work"] {
        assert!(world.status(folder).hooks_installed, "{folder}");
    }
    // The record was written for the command line's uninstall.
    assert!(world.home.roots.support.join("hook-install.json").exists());

    // Without the yes it is refused, and nothing is written.
    let other = World::new(two_accounts(None, None), no_probes());
    let before = other.files();
    let refused = other
        .call(Call::HooksReinstall { account_id: None })
        .unwrap_err();
    assert_eq!(refused.code, "refused");
    assert_eq!(other.files(), before);
}

/// Hooks off and `uninstall-hooks` put every settings.json back byte for
/// byte, and the hook copies go.
#[test]
fn hooks_off_and_uninstall_restore_every_settings_file() {
    let layout = two_accounts(
        Some(&realistic()),
        Some(r#"{"statusLine":{"type":"command","command":"echo hi"}}"#),
    );
    let world = World::started(layout, no_probes());
    let before = world.files();
    world
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    world.idle();
    assert!(world.status(".claude").hooks_installed);

    world
        .call(Call::HooksEnabled { on: false })
        .expect("hooks off");
    world.idle();
    world.assert_restored(&before);
    for folder in [".claude", ".claude-work"] {
        assert!(!world.dir(folder).join("hooks/agentnotch-hook.exe").exists());
    }

    // Again on, then the command line's uninstall with the app stopped.
    world
        .call(Call::HooksEnabled { on: true })
        .expect("hooks on");
    world.idle();
    assert!(world.status(".claude-work").hooks_installed);
    world.hub.hub.stop();
    let (platform, _) = testkit::platform(&base_of(&world.home));
    let said = Hub::uninstall_hooks_with(&world.home.roots, &platform, &DevFlags::default())
        .expect("removed");
    assert!(said.contains("2 folders"), "{said}");
    world.assert_restored(&before);
    for folder in [".claude", ".claude-work"] {
        assert!(!world.status(folder).hooks_registered, "{folder}");
    }
}
