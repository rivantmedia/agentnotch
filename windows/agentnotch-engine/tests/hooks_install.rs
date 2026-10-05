//! The installer against throwaway config folders: the Mac's
//! `HookInstallerFileTests` (HookInstallerTests.swift), with the hook exe's
//! copy in place of the scripts, plus what DESIGN-WIN §4.3 adds: the abort
//! when settings.json disappears mid-pass, the limit on planning again, a
//! read-only file, the official app's entries, uninstall from the record,
//! and the 8.3 command form.
//!
//! Claude Code saving at the worst moment is played by a `SecureFiles` that
//! wraps the real one. The holds only Windows has (a file open without
//! delete sharing) are in `agentnotch-win/tests/win_install.rs`.

use agentnotch_engine::core::settings_doc::Json;
use agentnotch_engine::hooks::apply::{
    apply_install_with, apply_installs_with, previous_status_line_path, read_saved_status_line,
    read_status, remove_codenotch_hooks, uninstall_everything, Setup, CONFIG_DIR_MISSING, IN_USE,
    KEPT_CHANGING, MAX_REPLANS, NOT_AN_INSTALL_TARGET, READ_ONLY, STORE_MARKER_NAME, VANISHED,
};
use agentnotch_engine::hooks::backups::{our_backups, ORIGINAL_BACKUP_NAME};
use agentnotch_engine::hooks::commands::{
    exec_form, hook_copy_path, string_command, Subcommand, HOOK_EXE_NAME, STATUS_LINE_NOT_AVAILABLE,
};
use agentnotch_engine::hooks::copy::{marked_name, ASIDE_MARK, STAGE_MARK};
use agentnotch_engine::hooks::events::hook_events;
use agentnotch_engine::hooks::plan::{Refusal, CHAINS_NOTHING};
use agentnotch_engine::model::AccountId;
use agentnotch_engine::persist::hook_install::HookInstallRecord;
use agentnotch_engine::platform::{
    Clock, Expect, FileIdentity, SecureFiles, WriteMode, WriteResult,
};
use agentnotch_engine::runtime_types::{
    CommandForm, InstallChange, InstallOutcome, InstallPlan, StatusLineIntent,
};
use agentnotch_engine::testkit::{mode_of, snapshot_dir, write_file, FakeClock, StdSecureFiles};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 2026-09-21T14:13:20Z.
const START_MS: u64 = 1_790_000_000_000;

/// Git Bash is there: someone else's status line may be wrapped.
const SETUP: Setup = Setup { git_bash: true };

const HOOK_BYTES: &[u8] = b"MZ not really an exe";

/// A settings.json as users have them: other tools' hooks (the official
/// app's among them, and one that only mentions our exe), a stale entry of
/// ours on an event no longer registered, and a status line.
const REALISTIC: &str = r#"{
  "$schema": "https://json.schemastore.org/claude-code-settings.json",
  "model": "opus",
  "env": {"BASH_DEFAULT_TIMEOUT_MS": "300000"},
  "permissions": {"allow": ["Bash(npm test:*)", "Read(~/notes/**)"], "deny": [], "defaultMode": "default"},
  "enabledPlugins": {"superpowers@claude-plugins-official": true},
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "~/bin/guard-bash.sh", "timeout": 30}]},
      {"matcher": "*", "hooks": [{"type": "command", "command": "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe", "timeout": 5}]}
    ],
    "PermissionRequest": [
      {"matcher": "*", "hooks": [
        {"type": "command", "command": "node ~/tools/notify.js --skip agentnotch-hook.exe", "timeout": 86400}
      ]}
    ],
    "Stop": [{"hooks": [{"type": "command", "command": "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe"}]}],
    "TeammateIdle": [{"hooks": [{"type": "command", "command": "C:/old/path/hooks/agentnotch-hook.exe hook"}]}]
  },
  "statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2, "refreshInterval": 5}
}
"#;

const UPSTREAM_COMMAND: &str = "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe";
const MENTION_COMMAND: &str = "node ~/tools/notify.js --skip agentnotch-hook.exe";

struct Fixture {
    _temp: tempfile::TempDir,
    /// Resolved, so paths built from it are the ones the installer reports.
    root: PathBuf,
    /// The hook exe beside the app.
    source: PathBuf,
    clock: FakeClock,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = StdSecureFiles.canonical(temp.path()).unwrap();
    let source = root.join("app").join(HOOK_EXE_NAME);
    write_file(&source, HOOK_BYTES);
    Fixture {
        _temp: temp,
        root,
        source,
        clock: FakeClock::at_ms(START_MS),
    }
}

impl Fixture {
    /// A config folder with a `projects` folder, as Claude Code leaves one,
    /// and the settings.json given.
    fn config_dir(&self, name: &str, settings: Option<&str>) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(dir.join("projects")).unwrap();
        if let Some(settings) = settings {
            write_file(&dir.join("settings.json"), settings);
        }
        dir
    }

    fn plan(&self, dir: &Path, status_line: StatusLineIntent) -> InstallPlan {
        InstallPlan {
            folder: folder(dir),
            settings_path: dir.join("settings.json"),
            expected: Expect::Nothing,
            existed: false,
            hook_copy: Some((self.source.clone(), hook_copy_path(dir))),
            // As the manager plans it from the path alone.
            form: string_command(&hook_copy_path(dir).to_string_lossy(), Subcommand::Hook)
                .map_or_else(
                    || CommandForm::NotPossible("no command can carry this path".to_owned()),
                    CommandForm::Text,
                ),
            events: hook_events(None).into_iter().map(str::to_owned).collect(),
            status_line,
            remove_only: false,
        }
    }

    fn removal(&self, dir: &Path) -> InstallPlan {
        InstallPlan {
            hook_copy: None,
            form: CommandForm::NotPossible(String::new()),
            events: Vec::new(),
            remove_only: true,
            ..self.plan(dir, StatusLineIntent::Unwrap)
        }
    }

    fn apply_with(&self, plan: &InstallPlan, files: &dyn SecureFiles) -> InstallOutcome {
        apply_install_with(plan, files, &self.clock, &SETUP)
    }

    fn apply(&self, plan: &InstallPlan) -> InstallOutcome {
        self.apply_with(plan, &StdSecureFiles)
    }

    /// Hooks and the status line.
    fn install(&self, dir: &Path) -> InstallOutcome {
        self.apply(&self.plan(dir, StatusLineIntent::Wrap))
    }

    fn uninstall(&self, dir: &Path) -> InstallOutcome {
        self.apply(&self.removal(dir))
    }
}

fn folder(dir: &Path) -> AccountId {
    AccountId(dir.to_string_lossy().into_owned())
}

fn hook_command(dir: &Path) -> String {
    string_command(&hook_copy_path(dir).to_string_lossy(), Subcommand::Hook)
        .expect("a temporary folder's path needs no quotes")
}

fn status_line_command(dir: &Path) -> String {
    string_command(
        &hook_copy_path(dir).to_string_lossy(),
        Subcommand::StatusLine,
    )
    .expect("a temporary folder's path needs no quotes")
}

fn settings(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn json(path: &Path) -> Json {
    let value = Json::parse(&fs::read(path).unwrap()).unwrap();
    assert!(value.is_object());
    value
}

fn parsed(text: &str) -> Json {
    Json::parse(text.as_bytes()).unwrap()
}

/// The commands registered for `event`, in file order.
fn commands(settings: &Json, event: &str) -> Vec<String> {
    settings
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

fn status_line_of(settings: &Json) -> Option<&str> {
    settings.get("statusLine")?.get("command")?.as_str()
}

/// `REALISTIC` without our stale entry: what an uninstall gives back.
fn realistic_without_ours() -> Json {
    let mut expected = parsed(REALISTIC);
    let mut hooks = expected.get("hooks").cloned().unwrap();
    hooks.set("TeammateIdle", None);
    expected.set("hooks", Some(hooks));
    expected
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn no_stage_left(dir: &Path) {
    let stages: Vec<String> = names(dir)
        .into_iter()
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(stages.is_empty(), "{stages:?}");
}

fn is_owner_only(path: &Path) -> bool {
    // Plain std can't read a Windows ACL; win_install.rs checks it there.
    !cfg!(unix) || mode_of(path) == Some(0o600)
}

fn modified(path: &Path) -> std::time::SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

fn set_read_only(path: &Path, read_only: bool) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(read_only);
    fs::set_permissions(path, permissions).unwrap();
}

// ---- A `SecureFiles` that misbehaves on cue ----

type BeforeWrite = Box<dyn FnMut(&Path) -> Option<WriteResult> + Send>;

/// The real files, with a say before each settings.json write (return a
/// result to answer in the real one's place), private writes that can fail,
/// and an 8.3 name for one folder.
struct Faulty {
    before_settings_write: Mutex<BeforeWrite>,
    settings_writes: AtomicUsize,
    private_writes_fail: bool,
    short: Option<(PathBuf, PathBuf)>,
    /// Another program holds settings.json past the retries.
    settings_busy: bool,
}

impl Faulty {
    fn new(before: impl FnMut(&Path) -> Option<WriteResult> + Send + 'static) -> Faulty {
        Faulty {
            before_settings_write: Mutex::new(Box::new(before)),
            settings_writes: AtomicUsize::new(0),
            private_writes_fail: false,
            short: None,
            settings_busy: false,
        }
    }

    fn plain() -> Faulty {
        Faulty::new(|_| None)
    }

    fn settings_writes(&self) -> usize {
        self.settings_writes.load(Ordering::SeqCst)
    }
}

impl SecureFiles for Faulty {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        StdSecureFiles.ensure_private_dir(dir)
    }

    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        match mode {
            WriteMode::KeepTargetSecurity => {
                self.settings_writes.fetch_add(1, Ordering::SeqCst);
                let answer = (self.before_settings_write.lock().unwrap())(path);
                if let Some(result) = answer {
                    return Ok(result);
                }
            }
            WriteMode::Private if self.private_writes_fail => {
                return Err(io::Error::other("the disk is full"));
            }
            WriteMode::Private => {}
        }
        StdSecureFiles.write_atomic(path, bytes, mode, expect)
    }

    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        StdSecureFiles.create_exclusive(path, bytes)
    }

    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        StdSecureFiles.identity(path)
    }

    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_reparse(path)
    }

    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        StdSecureFiles.canonical(path)
    }

    fn is_private(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_private(path)
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        if self.settings_busy && path.file_name().is_some_and(|name| name == "settings.json") {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "The process cannot access the file because it is being used by another process.",
            ));
        }
        StdSecureFiles.read_file(path)
    }

    fn short_path(&self, path: &Path) -> Option<PathBuf> {
        let (long, short) = self.short.as_ref()?;
        (path == long).then(|| short.clone())
    }
}

// ---- The Mac's on-disk suite ----

#[test]
fn install_is_idempotent_and_backs_up() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));

    let outcome = fx.install(&dir);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(outcome.folder, folder(&dir));
    assert_eq!(outcome.settings_path, settings(&dir));
    assert_eq!(outcome.status_line, Some(StatusLineIntent::Wrap));

    // The copy in place; the previous status line saved, owner-only.
    let copy = hook_copy_path(&dir);
    assert_eq!(fs::read(&copy).unwrap(), HOOK_BYTES);
    let saved_path = previous_status_line_path(&dir);
    let saved = read_saved_status_line(&saved_path).expect("the saved status line");
    assert_eq!(
        saved.get("command").and_then(Json::as_str),
        Some("~/.claude/statusline.sh")
    );
    assert!(is_owner_only(&saved_path));

    // Backups hold the original bytes, owner-only; the original is kept apart.
    let first_backups = our_backups(&dir);
    assert_eq!(first_backups.len(), 1);
    let backup = dir.join(&first_backups[0]);
    assert_eq!(fs::read(&backup).unwrap(), REALISTIC.as_bytes());
    assert!(is_owner_only(&backup));
    let original = dir.join(ORIGINAL_BACKUP_NAME);
    assert_eq!(fs::read(&original).unwrap(), REALISTIC.as_bytes());
    assert!(is_owner_only(&original));
    assert_eq!(outcome.backup, Some(backup.clone()));
    no_stage_left(&dir);

    // What the record is given.
    let entry = outcome.entry.expect("an entry for hook-install.json");
    assert_eq!(entry.settings_path, settings(&dir));
    assert_eq!(entry.folder, folder(&dir));
    assert_eq!(entry.form, "string");
    assert_eq!(entry.command, hook_command(&dir));
    assert_eq!(entry.hook_copy, Some(copy));
    assert!(entry.status_line);
    assert_eq!(entry.installed_at, fx.clock.now());

    // Ours on every event; everyone else's where they were.
    let written = json(&settings(&dir));
    for event in hook_events(None) {
        assert!(
            commands(&written, event).contains(&hook_command(&dir)),
            "missing {event}"
        );
    }
    assert_eq!(
        commands(&written, "PreToolUse")[..2],
        [
            "~/bin/guard-bash.sh".to_owned(),
            UPSTREAM_COMMAND.to_owned()
        ]
    );
    assert_eq!(commands(&written, "PermissionRequest")[0], MENTION_COMMAND);
    assert_eq!(commands(&written, "Stop")[0], UPSTREAM_COMMAND);
    assert!(written.get("hooks").unwrap().get("TeammateIdle").is_none());
    assert_eq!(
        status_line_of(&written),
        Some(status_line_command(&dir).as_str())
    );
    let before = parsed(REALISTIC);
    for member in before.members().unwrap() {
        if !matches!(member.key.as_str(), "hooks" | "statusLine") {
            assert!(
                Json::equivalent(written.get(&member.key), Some(&member.value)),
                "changed {}",
                member.key
            );
        }
    }

    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(status.config_dir_exists && status.settings_readable);
    assert!(status.hooks_registered && status.hooks_installed);
    assert!(status.status_line_installed);
    assert_eq!(status.status_line_left_alone, None);
    assert!(status.codenotch_hooks);
    assert_eq!(status.form.as_deref(), Some("string"));
    assert_eq!(status.hook_command, Some(hook_command(&dir)));
    assert_eq!(status.newest_backup, Some(backup));

    // Second run: nothing written, no new backup.
    let bytes = fs::read(settings(&dir)).unwrap();
    let stamp = modified(&settings(&dir));
    let again = fx.install(&dir);
    assert_eq!(again.result, Ok(InstallChange::Unchanged));
    assert_eq!(again.backup, None);
    assert!(again.entry.is_some_and(|entry| entry.status_line));
    assert_eq!(again.status_line, Some(StatusLineIntent::UpdateCommand));
    assert_eq!(fs::read(settings(&dir)).unwrap(), bytes);
    assert_eq!(modified(&settings(&dir)), stamp);
    assert_eq!(our_backups(&dir), first_backups);
}

#[test]
fn keeps_the_settings_files_permissions() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(r#"{"env":{"API_KEY":"secret"}}"#));
    set_mode(&settings(&dir), 0o600);
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    if cfg!(unix) {
        assert_eq!(mode_of(&settings(&dir)), Some(0o600));
    }
    let backups = our_backups(&dir);
    assert_eq!(backups.len(), 1);
    for backup in backups {
        assert!(is_owner_only(&dir.join(backup)));
    }
}

#[test]
fn refuses_unparseable_settings_without_touching_anything() {
    let fx = fixture();
    let garbage = "{\"permissions\": {\"allow\": [\"Bash(ls)\"]";
    let dir = fx.config_dir(".claude-test", Some(garbage));
    let before = snapshot_dir(&dir).unwrap();
    let refused = Err(Refusal::Unreadable.message().to_owned());

    let outcome = fx.install(&dir);
    assert_eq!(outcome.result, refused);
    assert_eq!(outcome.entry, None);
    assert!(!dir.join("hooks").exists());
    assert!(our_backups(&dir).is_empty());
    assert!(!read_status(&dir, &StdSecureFiles, &SETUP).settings_readable);
    assert_eq!(fx.uninstall(&dir).result, refused);
    assert_eq!(snapshot_dir(&dir).unwrap(), before);
}

#[test]
fn refuses_a_hooks_value_that_is_not_an_object() {
    let fx = fixture();
    let text = r#"{"hooks": [], "model": "opus"}"#;
    let dir = fx.config_dir(".claude-test", Some(text));
    let before = snapshot_dir(&dir).unwrap();
    assert_eq!(
        fx.install(&dir).result,
        Err(Refusal::HooksNotAnObject.message().to_owned())
    );
    assert_eq!(snapshot_dir(&dir).unwrap(), before);
    assert!(!dir.join("hooks").exists());
    assert!(!read_status(&dir, &StdSecureFiles, &SETUP).settings_readable);
}

/// A settings.json that links to a file that isn't there (a dotfiles repo
/// not cloned yet) stays a link.
#[cfg(unix)]
#[test]
fn refuses_a_dangling_symlink() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", None);
    let target = fx.root.join("dotfiles/claude/settings.json");
    std::os::unix::fs::symlink(&target, settings(&dir)).unwrap();
    let refused = Err(format!(
        "settings.json links to {}, which doesn't exist, so it was left alone.",
        target.display()
    ));

    assert_eq!(fx.install(&dir).result, refused);
    assert_eq!(fx.uninstall(&dir).result, refused);
    assert_eq!(fs::read_link(settings(&dir)).unwrap(), target);
    assert!(!target.exists());
    assert!(!dir.join("hooks").exists());
    assert!(!read_status(&dir, &StdSecureFiles, &SETUP).settings_readable);
}

#[test]
fn uninstall_restores_the_original() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));

    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    let removed = fx.uninstall(&dir);
    assert_eq!(removed.result, Ok(InstallChange::Removed));
    assert_eq!(removed.entry, None);
    assert_eq!(removed.status_line, Some(StatusLineIntent::Unwrap));

    assert!(json(&settings(&dir)).is_equivalent(&realistic_without_ours()));
    assert!(!hook_copy_path(&dir).exists());
    assert!(!previous_status_line_path(&dir).exists());
    // The folder ours went into held nothing else.
    assert!(!dir.join("hooks").exists());
    no_stage_left(&dir);
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(!status.hooks_registered && !status.hooks_installed);
    assert!(!status.status_line_installed);
    // The official app's entries were not ours to remove.
    assert!(status.codenotch_hooks);

    assert_eq!(fx.uninstall(&dir).result, Ok(InstallChange::Unchanged));
}

/// previous.json deleted by hand: uninstall brings the status line back from
/// the newest backup rather than deleting it.
#[test]
fn uninstall_without_the_saved_status_line_uses_a_backup() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    fs::remove_file(previous_status_line_path(&dir)).unwrap();

    assert_eq!(fx.uninstall(&dir).result, Ok(InstallChange::Removed));
    assert!(Json::equivalent(
        json(&settings(&dir)).get("statusLine"),
        parsed(REALISTIC).get("statusLine")
    ));
}

/// The same loss while ours stays installed: the next pass puts the saved
/// copy back, though settings.json needs no change.
#[test]
fn a_lost_saved_status_line_is_recovered_by_the_next_pass() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    let saved_path = previous_status_line_path(&dir);
    fs::remove_file(&saved_path).unwrap();

    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Unchanged));
    assert!(Json::equivalent(
        read_saved_status_line(&saved_path).as_ref(),
        parsed(REALISTIC).get("statusLine")
    ));
    assert!(is_owner_only(&saved_path));
}

#[test]
fn disabling_the_integration_unwraps_and_removes_the_saved_status_line() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));

    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    let off = fx.apply(&fx.plan(&dir, StatusLineIntent::Unwrap));
    assert_eq!(off.result, Ok(InstallChange::Written));
    assert_eq!(off.status_line, Some(StatusLineIntent::Unwrap));
    assert!(off.entry.is_some_and(|entry| !entry.status_line));

    let written = json(&settings(&dir));
    assert!(Json::equivalent(
        written.get("statusLine"),
        parsed(REALISTIC).get("statusLine")
    ));
    assert!(!previous_status_line_path(&dir).exists());
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(status.hooks_installed && !status.status_line_installed);
}

#[test]
fn fresh_config_dir_without_settings() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", None);

    let outcome = fx.install(&dir);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(outcome.backup, None);
    assert!(our_backups(&dir).is_empty());
    assert!(!dir.join(ORIGINAL_BACKUP_NAME).exists());
    let written = json(&settings(&dir));
    assert_eq!(
        status_line_of(&written),
        Some(status_line_command(&dir).as_str())
    );
    // Nothing to chain, said explicitly.
    assert_eq!(
        fs::read(previous_status_line_path(&dir)).unwrap(),
        CHAINS_NOTHING
    );
    assert!(read_status(&dir, &StdSecureFiles, &SETUP).status_line_installed);
}

#[test]
fn missing_config_dir_is_reported() {
    let fx = fixture();
    let dir = fx.root.join(".claude-nope");
    let missing = Err(CONFIG_DIR_MISSING.to_owned());
    assert_eq!(fx.install(&dir).result, missing);
    assert_eq!(fx.uninstall(&dir).result, missing);
    assert!(!dir.exists());
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(!status.config_dir_exists && !status.hooks_registered);
}

/// `PP_HooksTests.theInstallerRefusesStoresAndTheSharedHistory`: whatever a
/// plan thinks, nothing is installed into a Claude Parallel Profiles store;
/// the marker alone refuses. Taking ours out of one still works.
#[test]
fn the_installer_refuses_a_store_whatever_the_plan_says() {
    let fx = fixture();
    let store = fx.config_dir(".claude-paras", None);
    write_file(&store.join(STORE_MARKER_NAME), "");
    let refused = Err(NOT_AN_INSTALL_TARGET.to_owned());
    assert_eq!(fx.install(&store).result, refused);
    assert!(!settings(&store).exists());
    assert!(!store.join("hooks").exists());

    // One with a settings.json of its own: its bytes stay as they are.
    let with_settings = fx.config_dir(".claude-work", Some(REALISTIC));
    write_file(&with_settings.join(STORE_MARKER_NAME), "");
    let before = snapshot_dir(&with_settings).unwrap();
    let outcome = fx.install(&with_settings);
    assert_eq!(outcome.result, refused);
    assert_eq!(outcome.entry, None);
    assert_eq!(snapshot_dir(&with_settings).unwrap(), before);

    // Ours put there before the folder became a store come out.
    fs::remove_file(with_settings.join(STORE_MARKER_NAME)).unwrap();
    assert_eq!(
        fx.install(&with_settings).result,
        Ok(InstallChange::Written)
    );
    write_file(&with_settings.join(STORE_MARKER_NAME), "");
    assert_eq!(
        fx.uninstall(&with_settings).result,
        Ok(InstallChange::Removed)
    );
    assert!(!read_status(&with_settings, &StdSecureFiles, &SETUP).hooks_registered);
    assert!(!with_settings.join("hooks").exists());
}

#[cfg(unix)]
#[test]
fn writes_through_a_symlinked_settings_file() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", None);
    let real = fx.root.join("dotfiles").join("claude-settings.json");
    write_file(&real, r#"{"model":"opus"}"#);
    set_mode(&real, 0o640);
    std::os::unix::fs::symlink(&real, settings(&dir)).unwrap();

    let outcome = fx.install(&dir);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    // The resolved file is the one reported and recorded.
    assert_eq!(outcome.settings_path, real);
    assert_eq!(
        outcome.entry.map(|entry| entry.settings_path),
        Some(real.clone())
    );
    assert_eq!(fs::read_link(settings(&dir)).unwrap(), real);
    assert!(fs::read_to_string(&real).unwrap().contains(HOOK_EXE_NAME));
    assert_eq!(mode_of(&real), Some(0o640));
    // The stage and the backups went beside the real file, not the link.
    no_stage_left(&dir);
    no_stage_left(real.parent().unwrap());
    assert!(our_backups(&dir).is_empty());
    assert_eq!(our_backups(real.parent().unwrap()).len(), 1);
}

/// A settings.json copied from another folder runs that folder's wrapper;
/// installing here must keep chaining to the same status line.
#[test]
fn copied_settings_keep_the_status_line_chain() {
    let fx = fixture();
    let first = fx.config_dir(".claude-first", Some(REALISTIC));
    let second = fx.config_dir(".claude-second", None);
    let original = parsed(REALISTIC);

    assert_eq!(fx.install(&first).result, Ok(InstallChange::Written));
    fs::copy(settings(&first), settings(&second)).unwrap();
    assert_eq!(fx.install(&second).result, Ok(InstallChange::Written));

    let written = json(&settings(&second));
    assert_eq!(
        status_line_of(&written),
        Some(status_line_command(&second).as_str())
    );
    assert!(!commands(&written, "Stop").contains(&hook_command(&first)));
    let saved = read_saved_status_line(&previous_status_line_path(&second));
    assert!(Json::equivalent(saved.as_ref(), original.get("statusLine")));

    assert_eq!(fx.uninstall(&second).result, Ok(InstallChange::Removed));
    assert!(Json::equivalent(
        json(&settings(&second)).get("statusLine"),
        original.get("statusLine")
    ));
}

/// Two folders whose settings.json is one file (a link): either one's status
/// reads true, and uninstalling through either restores the original status
/// line, whichever folder's copy is installed.
#[cfg(unix)]
#[test]
fn shared_settings_file_works_from_either_account() {
    let fx = fixture();
    let first = fx.config_dir(".claude-first", Some(REALISTIC));
    let second = fx.config_dir(".claude-second", None);
    std::os::unix::fs::symlink(settings(&first), settings(&second)).unwrap();
    let original = parsed(REALISTIC);

    assert_eq!(fx.install(&first).result, Ok(InstallChange::Written));
    let status = read_status(&second, &StdSecureFiles, &SETUP);
    assert!(status.hooks_installed && status.status_line_installed);
    assert!(!second.join("hooks").exists());

    let removed = fx.uninstall(&second);
    assert_eq!(removed.result, Ok(InstallChange::Removed));
    assert_eq!(removed.settings_path, settings(&first));
    assert!(Json::equivalent(
        json(&settings(&first)).get("statusLine"),
        original.get("statusLine")
    ));
    assert!(!read_status(&first, &StdSecureFiles, &SETUP).hooks_installed);
    assert_eq!(fx.uninstall(&first).result, Ok(InstallChange::Unchanged));
    assert!(!previous_status_line_path(&first).exists());
}

/// A pass writes each physical settings.json once, and an install wins over
/// a removal through another folder's link to the same file.
#[cfg(unix)]
#[test]
fn a_pass_writes_a_shared_settings_file_once() {
    let fx = fixture();
    let first = fx.config_dir(".claude-first", Some(REALISTIC));
    let second = fx.config_dir(".claude-second", None);
    let third = fx.config_dir(".claude-third", None);
    std::os::unix::fs::symlink(settings(&first), settings(&second)).unwrap();
    std::os::unix::fs::symlink(settings(&first), settings(&third)).unwrap();

    let plans = [
        fx.removal(&third),
        fx.plan(&first, StatusLineIntent::Wrap),
        fx.plan(&second, StatusLineIntent::Wrap),
    ];
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &fx.clock, &SETUP);
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.folder.clone())
            .collect::<Vec<_>>(),
        [folder(&third), folder(&first), folder(&second)]
    );
    assert_eq!(outcomes[1].result, Ok(InstallChange::Written));
    assert!(outcomes[1].entry.is_some());
    for shared in [&outcomes[0], &outcomes[2]] {
        assert_eq!(shared.result, Ok(InstallChange::Unchanged));
        assert_eq!(shared.settings_path, settings(&first));
        assert_eq!(shared.entry, None);
    }
    assert!(!second.join("hooks").exists());
    let written = json(&settings(&first));
    assert!(commands(&written, "Stop").contains(&hook_command(&first)));
    assert!(!commands(&written, "Stop").contains(&hook_command(&second)));
    assert_eq!(our_backups(&first).len(), 1);
}

/// A folder no command can name takes ours out of its file, so through a
/// link it would strip what a folder that can be hooked just wrote there:
/// it goes after that folder's install, and the file keeps its hooks.
#[cfg(unix)]
#[test]
fn a_folder_no_command_can_name_never_strips_a_shared_file() {
    let fx = fixture();
    let first = fx.config_dir(".claude-first", Some(REALISTIC));
    let unnamed = fx.config_dir("O'Brien", None);
    std::os::unix::fs::symlink(settings(&first), settings(&unnamed)).unwrap();

    let mut cannot = fx.plan(&unnamed, StatusLineIntent::Wrap);
    cannot.form = CommandForm::NotPossible("Can't be hooked here".to_owned());
    let plans = [cannot, fx.plan(&first, StatusLineIntent::Wrap)];
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &fx.clock, &SETUP);
    assert_eq!(outcomes[0].folder, folder(&unnamed));
    assert_eq!(outcomes[0].result, Ok(InstallChange::Unchanged));
    assert_eq!(outcomes[1].result, Ok(InstallChange::Written));
    let written = json(&settings(&first));
    assert!(commands(&written, "Stop").contains(&hook_command(&first)));
    assert!(read_status(&first, &StdSecureFiles, &SETUP).hooks_installed);
}

/// The same rule without a link: one file named by two plans of a pass.
#[test]
fn a_removal_never_undoes_an_install_of_the_same_file() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let other = fx.config_dir(".claude-other", None);

    let plans = [
        fx.removal(&dir),
        fx.plan(&other, StatusLineIntent::Nothing),
        fx.plan(&dir, StatusLineIntent::Wrap),
    ];
    let outcomes = apply_installs_with(&plans, &StdSecureFiles, &fx.clock, &SETUP);
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].result, Ok(InstallChange::Unchanged));
    assert_eq!(outcomes[0].entry, None);
    assert_eq!(outcomes[1].result, Ok(InstallChange::Written));
    assert_eq!(outcomes[1].folder, folder(&other));
    assert_eq!(outcomes[2].result, Ok(InstallChange::Written));
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(status.hooks_installed && status.status_line_installed);
    assert!(read_status(&other, &StdSecureFiles, &SETUP).hooks_installed);
}

/// A save by Claude Code that lands while we plan is kept: we plan again
/// from its bytes instead of writing over them.
#[test]
fn a_concurrent_save_is_not_lost() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(r#"{"model":"opus"}"#));
    let mut interfered = false;
    let files = Faulty::new(move |path| {
        if !interfered {
            interfered = true;
            // Claude Code saves a permission rule meanwhile.
            write_file(
                path,
                r#"{"model":"opus","permissions":{"allow":["Bash(ls)"]}}"#,
            );
        }
        None
    });

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Nothing), &files);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(files.settings_writes(), 2);
    let written = json(&settings(&dir));
    let allowed = written
        .get("permissions")
        .and_then(|permissions| permissions.get("allow"))
        .and_then(Json::items)
        .and_then(|items| items[0].as_str());
    assert_eq!(allowed, Some("Bash(ls)"));
    assert!(commands(&written, "Stop").contains(&hook_command(&dir)));
    assert!(written.get("statusLine").is_none());
    no_stage_left(&dir);
}

// ---- What DESIGN-WIN §4.3 adds ----

/// settings.json deleted between the read and the write: the pass stops.
/// A file that was there is never replaced by one planned from nothing.
#[test]
fn a_settings_file_that_disappears_aborts_the_pass() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let files = Faulty::new(|path| {
        fs::remove_file(path).unwrap();
        None
    });

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Wrap), &files);
    assert_eq!(
        outcome.result,
        Ok(InstallChange::Aborted(
            "settings.json disappeared while Agent Notch was writing it; nothing was changed"
                .to_owned()
        ))
    );
    assert_eq!(outcome.entry, None);
    assert_eq!(files.settings_writes(), 1);
    assert!(!settings(&dir).exists());
    no_stage_left(&dir);
    // What was there is still in the backup.
    assert_eq!(
        fs::read(dir.join(ORIGINAL_BACKUP_NAME)).unwrap(),
        REALISTIC.as_bytes()
    );
}

/// The same, noticed at the next read: a write refused as "changed", and no
/// file there when the plan is about to be made again.
#[test]
fn a_settings_file_gone_at_the_second_read_aborts_too() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(r#"{"model":"opus"}"#));
    let files = Faulty::new(|path| {
        let _ = fs::remove_file(path);
        Some(WriteResult::Changed)
    });

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Nothing), &files);
    assert_eq!(
        outcome.result,
        Ok(InstallChange::Aborted(VANISHED.to_owned()))
    );
    assert_eq!(files.settings_writes(), 1);
    assert!(!settings(&dir).exists());

    // A plan made from a file that existed is not carried out on none.
    let mut plan = fx.plan(&dir, StatusLineIntent::Nothing);
    plan.existed = true;
    assert_eq!(
        fx.apply(&plan).result,
        Ok(InstallChange::Aborted(VANISHED.to_owned()))
    );
    assert!(!settings(&dir).exists());
    // Without that, no file is simply no file yet.
    plan.existed = false;
    assert_eq!(fx.apply(&plan).result, Ok(InstallChange::Written));
}

/// A file that appears where there was none is planned from, not replaced.
#[test]
fn a_settings_file_that_appears_is_planned_from() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", None);
    let mut interfered = false;
    let files = Faulty::new(move |path| {
        if !interfered {
            interfered = true;
            write_file(path, r#"{"model":"opus"}"#);
        }
        None
    });

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Nothing), &files);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(files.settings_writes(), 2);
    let written = json(&settings(&dir));
    assert_eq!(written.get("model").and_then(Json::as_str), Some("opus"));
    assert!(commands(&written, "Stop").contains(&hook_command(&dir)));
}

#[test]
fn gives_up_when_the_settings_file_keeps_changing() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let files = Faulty::new(|_| Some(WriteResult::Changed));

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Wrap), &files);
    assert_eq!(outcome.result, Err(KEPT_CHANGING.to_owned()));
    assert_eq!(
        KEPT_CHANGING,
        "settings.json kept changing while we tried to update it"
    );
    // The first plan, then five made again.
    assert_eq!(MAX_REPLANS, 5);
    assert_eq!(files.settings_writes(), 1 + MAX_REPLANS);
    assert_eq!(fs::read(settings(&dir)).unwrap(), REALISTIC.as_bytes());
    assert_eq!(outcome.entry, None);
    no_stage_left(&dir);
}

#[test]
fn refuses_a_read_only_settings_file() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    assert_eq!(READ_ONLY, "settings.json is read-only");

    // As the file system says it.
    set_read_only(&settings(&dir), true);
    let outcome = fx.install(&dir);
    set_read_only(&settings(&dir), false);
    assert_eq!(outcome.result, Err(READ_ONLY.to_owned()));
    assert_eq!(fs::read(settings(&dir)).unwrap(), REALISTIC.as_bytes());
    // Refused before the status line was saved or a backup made.
    assert!(!previous_status_line_path(&dir).exists());
    assert!(our_backups(&dir).is_empty());

    // As the write itself reports it (an attribute set in between).
    let files = Faulty::new(|_| Some(WriteResult::ReadOnly));
    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Wrap), &files);
    assert_eq!(outcome.result, Err(READ_ONLY.to_owned()));
    assert_eq!(fs::read(settings(&dir)).unwrap(), REALISTIC.as_bytes());

    // A read-only file that needs no change is no failure.
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    set_read_only(&settings(&dir), true);
    let outcome = fx.install(&dir);
    set_read_only(&settings(&dir), false);
    assert_eq!(outcome.result, Ok(InstallChange::Unchanged));
}

/// The status line being taken over is saved before settings.json stops
/// pointing at it; when that fails, nothing is taken over.
#[test]
fn nothing_is_taken_over_when_the_status_line_cannot_be_saved() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let files = Faulty {
        private_writes_fail: true,
        ..Faulty::plain()
    };

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Wrap), &files);
    assert_eq!(
        outcome.result,
        Err("Couldn't save the current status line: the disk is full".to_owned())
    );
    assert_eq!(files.settings_writes(), 0);
    assert_eq!(fs::read(settings(&dir)).unwrap(), REALISTIC.as_bytes());
    assert!(!previous_status_line_path(&dir).exists());
}

/// No source exe (a dev run without it): settings.json must not be pointed
/// at a copy that was never written.
#[test]
fn nothing_is_registered_without_the_hook_copy() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let before = snapshot_dir(&dir).unwrap();
    let unavailable = Err(format!("{HOOK_EXE_NAME} is not available"));

    let mut plan = fx.plan(&dir, StatusLineIntent::Wrap);
    plan.hook_copy = Some((fx.root.join("nowhere.exe"), hook_copy_path(&dir)));
    assert_eq!(fx.apply(&plan).result, unavailable);
    plan.hook_copy = None;
    assert_eq!(fx.apply(&plan).result, unavailable);
    assert_eq!(snapshot_dir(&dir).unwrap(), before);

    // With the copy already in place, a plan that names no source goes ahead.
    write_file(&hook_copy_path(&dir), HOOK_BYTES);
    assert_eq!(fx.apply(&plan).result, Ok(InstallChange::Written));
}

#[test]
fn removes_the_official_apps_hooks_and_nothing_else() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Written));
    let installed = json(&settings(&dir));
    let backups = our_backups(&dir);
    fx.clock.advance(std::time::Duration::from_secs(60));

    assert_eq!(
        remove_codenotch_hooks(&settings(&dir), &StdSecureFiles, &fx.clock),
        Ok(2)
    );
    let written = json(&settings(&dir));
    for event in hook_events(None) {
        let mut expected = commands(&installed, event);
        expected.retain(|command| command != UPSTREAM_COMMAND);
        assert_eq!(commands(&written, event), expected, "{event}");
    }
    for member in installed.members().unwrap() {
        if member.key != "hooks" {
            assert!(Json::equivalent(
                written.get(&member.key),
                Some(&member.value)
            ));
        }
    }
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(!status.codenotch_hooks);
    assert!(status.hooks_installed && status.status_line_installed);
    // The bytes it replaced were backed up first.
    assert_eq!(our_backups(&dir).len(), backups.len() + 1);
    no_stage_left(&dir);

    // Nothing left of it: nothing written.
    let bytes = fs::read(settings(&dir)).unwrap();
    let stamp = modified(&settings(&dir));
    let backups = our_backups(&dir);
    assert_eq!(
        remove_codenotch_hooks(&settings(&dir), &StdSecureFiles, &fx.clock),
        Ok(0)
    );
    assert_eq!(fs::read(settings(&dir)).unwrap(), bytes);
    assert_eq!(modified(&settings(&dir)), stamp);
    assert_eq!(our_backups(&dir), backups);
}

#[test]
fn removing_the_official_apps_hooks_leaves_other_files_as_they_are() {
    let fx = fixture();
    // An emptied event is the user's to keep when nothing of the official
    // app's is there.
    let text = "{\r\n  \"hooks\": {\"Stop\": []},\r\n  \"model\": \"opus\"\r\n}\r\n";
    let dir = fx.config_dir(".claude-test", Some(text));
    let before = snapshot_dir(&dir).unwrap();
    assert_eq!(
        remove_codenotch_hooks(&settings(&dir), &StdSecureFiles, &fx.clock),
        Ok(0)
    );
    assert_eq!(snapshot_dir(&dir).unwrap(), before);

    let empty = fx.config_dir(".claude-empty", None);
    assert_eq!(
        remove_codenotch_hooks(&settings(&empty), &StdSecureFiles, &fx.clock),
        Ok(0)
    );
    assert!(!settings(&empty).exists());

    let garbage = fx.config_dir(".claude-garbage", Some("{\"hooks\": {"));
    assert_eq!(
        remove_codenotch_hooks(&settings(&garbage), &StdSecureFiles, &fx.clock),
        Err(Refusal::Unreadable.message().to_owned())
    );
    assert_eq!(
        remove_codenotch_hooks(
            &fx.root.join("nowhere").join("settings.json"),
            &StdSecureFiles,
            &fx.clock
        ),
        Err(CONFIG_DIR_MISSING.to_owned())
    );

    // The same safe write as an install: a file that goes away is not
    // written from nothing.
    let vanishing = fx.config_dir(".claude-vanishing", Some(REALISTIC));
    let files = Faulty::new(|path| {
        fs::remove_file(path).unwrap();
        None
    });
    assert_eq!(
        remove_codenotch_hooks(&settings(&vanishing), &files, &fx.clock),
        Err(VANISHED.to_owned())
    );
    assert!(!settings(&vanishing).exists());
}

#[test]
fn uninstalls_everything_from_the_record() {
    let fx = fixture();
    let first = fx.config_dir(".claude-first", Some(REALISTIC));
    let second = fx.config_dir(".claude-second", None);
    let gone = fx.config_dir(".claude-gone", None);

    let mut record = HookInstallRecord::default();
    for dir in [&first, &second, &gone] {
        let outcome = fx.install(dir);
        assert_eq!(outcome.result, Ok(InstallChange::Written));
        record.files.push(outcome.entry.unwrap());
    }
    // Leftovers of replaced copies, and something of the user's in `hooks`.
    let now = fx.clock.now();
    let leftovers = [
        first.join("hooks").join(marked_name(ASIDE_MARK, now)),
        first.join("hooks").join(marked_name(STAGE_MARK, now)),
        second.join("hooks").join(marked_name(ASIDE_MARK, now)),
    ];
    for leftover in &leftovers {
        write_file(leftover, "an earlier copy");
    }
    let theirs = second.join("hooks").join("notify.cmd");
    write_file(&theirs, "@echo off");
    fs::remove_dir_all(&gone).unwrap();

    let outcomes = uninstall_everything(&record, &StdSecureFiles, &fx.clock);
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].result, Ok(InstallChange::Removed));
    assert_eq!(outcomes[0].folder, folder(&first));
    assert_eq!(outcomes[0].settings_path, settings(&first));
    assert_eq!(outcomes[1].result, Ok(InstallChange::Removed));
    assert_eq!(outcomes[2].result, Err(CONFIG_DIR_MISSING.to_owned()));
    assert!(outcomes.iter().all(|outcome| outcome.entry.is_none()));

    assert!(json(&settings(&first)).is_equivalent(&realistic_without_ours()));
    assert!(json(&settings(&second)).is_equivalent(&parsed("{}")));
    for leftover in &leftovers {
        assert!(!leftover.exists(), "{}", leftover.display());
    }
    assert!(!first.join("hooks").exists());
    // A `hooks` folder holding someone else's file stays, with that file.
    assert_eq!(names(&second.join("hooks")), ["notify.cmd"]);
    assert!(!gone.exists());

    // Again: nothing left to do.
    let again = uninstall_everything(&record, &StdSecureFiles, &fx.clock);
    assert_eq!(again[0].result, Ok(InstallChange::Unchanged));
    assert_eq!(again[1].result, Ok(InstallChange::Unchanged));
}

/// A folder whose path no string command can carry, on a volume without 8.3
/// names: not hooked, nothing written, the plan's reason shown.
#[test]
fn a_folder_no_command_can_name_is_left_untouched() {
    let fx = fixture();
    // Nothing of ours in it (`REALISTIC`'s stale entry is, and would come
    // out: the next test).
    let none_of_ours = realistic_none_of_ours();
    let dir = fx.config_dir("my claude", Some(&none_of_ours));
    let before = snapshot_dir(&dir).unwrap();
    let reason = "Can't be hooked here: its path has characters a hook command can't carry";

    let mut plan = fx.plan(&dir, StatusLineIntent::Wrap);
    plan.form = CommandForm::NotPossible(reason.to_owned());
    let files = Faulty::plain();
    let outcome = fx.apply_with(&plan, &files);
    assert_eq!(outcome.result, Err(reason.to_owned()));
    assert_eq!(outcome.entry, None);
    assert_eq!(files.settings_writes(), 0);
    assert_eq!(snapshot_dir(&dir).unwrap(), before);
    assert!(!dir.join("hooks").exists());
}

/// A settings.json another program holds (past `WinFiles`' retries) is
/// busy, not broken: the user is told to try again, not to fix the file,
/// and nothing is written.
#[test]
fn a_settings_json_held_by_another_program_is_said_to_be_in_use() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let before = snapshot_dir(&dir).unwrap();
    let files = Faulty {
        settings_busy: true,
        ..Faulty::plain()
    };

    let outcome = fx.apply_with(&fx.plan(&dir, StatusLineIntent::Wrap), &files);
    assert_eq!(outcome.result, Err(IN_USE.to_owned()));
    assert_ne!(IN_USE, Refusal::Unreadable.message());
    assert_eq!(
        fx.apply_with(&fx.removal(&dir), &files).result,
        Err(IN_USE.to_owned())
    );
    assert_eq!(
        remove_codenotch_hooks(&settings(&dir), &files, &fx.clock),
        Err(IN_USE.to_owned())
    );
    assert_eq!(files.settings_writes(), 0);
    assert_eq!(snapshot_dir(&dir).unwrap(), before);

    let status = read_status(&dir, &files, &SETUP);
    assert!(status.settings_in_use && !status.settings_readable);
    // A file that really doesn't parse is not "in use".
    write_file(&settings(&dir), "{\"hooks\": ");
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(!status.settings_in_use && !status.settings_readable);
}

/// `REALISTIC` as text without our stale entry.
fn realistic_none_of_ours() -> String {
    let stale = ",\n    \"TeammateIdle\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"C:/old/path/hooks/agentnotch-hook.exe hook\"}]}]";
    assert!(REALISTIC.contains(stale));
    let text = REALISTIC.replace(stale, "");
    assert!(parsed(&text).is_equivalent(&realistic_without_ours()));
    text
}

/// Exec-form hooks were written while every Claude Code here could run
/// them; then an older one showed up, and this folder's path has no string
/// form (`(` or `'`, no 8.3 name). That copy would ignore `args` and hand
/// the bare path to Git Bash: a syntax error, exit 2, which blocks every tool
/// call, prompt and Stop. So ours come out, and only ours; the folder still
/// reports why it can't be hooked.
#[test]
fn exec_form_hooks_come_out_when_the_folder_can_no_longer_be_named() {
    let fx = fixture();
    for name in ["Claude (work)", "O'Brien"] {
        let dir = fx.config_dir(name, Some(REALISTIC));
        let exe = hook_copy_path(&dir);
        assert!(string_command(&exe.to_string_lossy(), Subcommand::Hook).is_none());

        let mut plan = fx.plan(&dir, StatusLineIntent::Wrap);
        plan.form = exec_form(&exe);
        assert_eq!(fx.apply(&plan).result, Ok(InstallChange::Written));
        let installed = fs::read(settings(&dir)).unwrap();
        assert!(read_status(&dir, &StdSecureFiles, &SETUP).hooks_registered);

        let reason = "Can't be hooked here: its path needs Claude Code 2.1.139 or later everywhere on this PC";
        plan.form = CommandForm::NotPossible(reason.to_owned());
        let files = Faulty::plain();
        let outcome = fx.apply_with(&plan, &files);
        assert_eq!(outcome.result, Err(reason.to_owned()), "{name}");
        assert_eq!(outcome.entry, None);
        assert_eq!(files.settings_writes(), 1);
        // The backup holds what was there, ours included.
        let backup = outcome.backup.expect("a backup of the file with ours");
        assert_eq!(fs::read(&backup).unwrap(), installed);

        // Nothing of ours left; everything else as the user had it.
        let after = json(&settings(&dir));
        assert!(after.is_equivalent(&realistic_without_ours()), "{name}");
        assert_eq!(
            commands(&after, "PreToolUse"),
            vec![
                "~/bin/guard-bash.sh".to_owned(),
                UPSTREAM_COMMAND.to_owned()
            ]
        );
        assert_eq!(
            commands(&after, "PermissionRequest"),
            vec![MENTION_COMMAND.to_owned()]
        );
        let status = read_status(&dir, &StdSecureFiles, &SETUP);
        assert!(!status.hooks_registered && status.settings_readable);
        // The copy stays for the record's removal to take.
        assert!(exe.is_file());

        // A second pass has nothing to do.
        let before = snapshot_dir(&dir).unwrap();
        let again = Faulty::plain();
        let outcome = fx.apply_with(&plan, &again);
        assert_eq!(outcome.result, Err(reason.to_owned()));
        assert_eq!(outcome.backup, None);
        assert_eq!(again.settings_writes(), 0);
        assert_eq!(snapshot_dir(&dir).unwrap(), before);
    }
}

/// Taking ours out of a folder no command can name is still a write to the
/// user's file, under the same refusals as any other.
#[test]
fn withdrawing_hooks_refuses_a_file_it_cannot_read() {
    let fx = fixture();
    let reason = "Can't be hooked here: its path has characters a hook command can't carry";
    for (name, text, refusal) in [
        ("bad (json)", "{\"hooks\": ", Refusal::Unreadable),
        ("bad (hooks)", "{\"hooks\": []}", Refusal::HooksNotAnObject),
    ] {
        let dir = fx.config_dir(name, Some(text));
        let before = snapshot_dir(&dir).unwrap();
        let mut plan = fx.plan(&dir, StatusLineIntent::Wrap);
        plan.form = CommandForm::NotPossible(reason.to_owned());
        let files = Faulty::plain();
        let outcome = fx.apply_with(&plan, &files);
        assert_eq!(outcome.result, Err(refusal.message().to_owned()), "{name}");
        assert_eq!(files.settings_writes(), 0);
        assert_eq!(snapshot_dir(&dir).unwrap(), before);
    }
}

/// The same folder where the volume has 8.3 names: the short spelling is
/// what goes into settings.json, for the hooks and the status line.
#[cfg(unix)]
#[test]
fn the_short_path_is_written_when_the_volume_has_one() {
    let fx = fixture();
    let dir = fx.config_dir("my claude", Some(REALISTIC));
    // A link stands in for the 8.3 name, so the command names a real file.
    let short = fx.root.join("MYCLAU~1");
    std::os::unix::fs::symlink(&dir, &short).unwrap();
    let files = Faulty {
        short: Some((dir.clone(), short.clone())),
        ..Faulty::plain()
    };

    let mut plan = fx.plan(&dir, StatusLineIntent::Wrap);
    plan.form = CommandForm::NotPossible("no command can carry this path".to_owned());
    let outcome = fx.apply_with(&plan, &files);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    let hook = format!("{}/hooks/{HOOK_EXE_NAME} hook", short.display());
    let status_line = format!("{}/hooks/{HOOK_EXE_NAME} statusline", short.display());
    let entry = outcome.entry.unwrap();
    assert_eq!(entry.form, "string");
    assert_eq!(entry.command, hook);
    assert!(entry.status_line);

    let written = json(&settings(&dir));
    assert!(commands(&written, "Stop").contains(&hook));
    assert_eq!(status_line_of(&written), Some(status_line.as_str()));
    assert_eq!(fs::read(hook_copy_path(&dir)).unwrap(), HOOK_BYTES);
    let status = read_status(&dir, &files, &SETUP);
    assert!(status.hooks_installed && status.status_line_installed);
    assert_eq!(status.hook_command, Some(hook));

    assert_eq!(
        fx.apply_with(&plan, &files).result,
        Ok(InstallChange::Unchanged)
    );
    assert_eq!(
        fx.apply_with(&fx.removal(&dir), &files).result,
        Ok(InstallChange::Removed)
    );
    assert!(json(&settings(&dir)).is_equivalent(&realistic_without_ours()));
}

/// Exec-form hooks need no string command, but the status line always is
/// one: where none can carry the path, the user's stays as it is.
#[test]
fn the_status_line_is_left_where_no_command_can_carry_the_path() {
    let fx = fixture();
    let dir = fx.config_dir("my claude", Some(REALISTIC));
    let exe = hook_copy_path(&dir);

    let mut plan = fx.plan(&dir, StatusLineIntent::Wrap);
    plan.form = exec_form(&exe);
    let outcome = fx.apply(&plan);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(
        outcome.status_line,
        Some(StatusLineIntent::LeaveAlone(
            "Live status line isn't available for this folder".to_owned()
        ))
    );
    let entry = outcome.entry.unwrap();
    assert_eq!(entry.form, "exec");
    assert_eq!(entry.command, exe.to_string_lossy());
    assert!(!entry.status_line);

    let written = json(&settings(&dir));
    assert!(Json::equivalent(
        written.get("statusLine"),
        parsed(REALISTIC).get("statusLine")
    ));
    assert!(!previous_status_line_path(&dir).exists());
    let status = read_status(&dir, &StdSecureFiles, &SETUP);
    assert!(status.hooks_installed && !status.status_line_installed);
    assert_eq!(status.form.as_deref(), Some("exec"));
    assert_eq!(
        status.hook_command,
        Some(format!("{} hook --exec", exe.to_string_lossy()))
    );
    assert_eq!(
        status.status_line_left_alone.as_deref(),
        Some(STATUS_LINE_NOT_AVAILABLE)
    );
}

/// Someone else's status line is wrapped only where Git Bash can run it
/// again; without it the line stays, and the reason is reported.
#[test]
fn the_status_line_is_left_alone_without_git_bash() {
    let fx = fixture();
    let dir = fx.config_dir(".claude-test", Some(REALISTIC));
    let no_git_bash = Setup { git_bash: false };
    let reason = "Status line left alone: Git Bash isn't installed";

    let outcome = apply_install_with(
        &fx.plan(&dir, StatusLineIntent::Wrap),
        &StdSecureFiles,
        &fx.clock,
        &no_git_bash,
    );
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(
        outcome.status_line,
        Some(StatusLineIntent::LeaveAlone(reason.to_owned()))
    );
    assert!(outcome.entry.is_some_and(|entry| !entry.status_line));
    assert!(Json::equivalent(
        json(&settings(&dir)).get("statusLine"),
        parsed(REALISTIC).get("statusLine")
    ));
    let status = read_status(&dir, &StdSecureFiles, &no_git_bash);
    assert!(status.hooks_installed && !status.status_line_installed);
    assert_eq!(status.status_line_left_alone.as_deref(), Some(reason));
}
