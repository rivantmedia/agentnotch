//! Carrying a plan out on disk (the IO half of HookInstaller.swift:
//! `install`, `uninstall`, `applyChange`, `readStatus`; DESIGN-WIN §4.3).
//!
//! settings.json is the user's file and Claude Code rewrites it whenever it
//! likes, so a write here goes:
//!
//! 1. resolve the path once: a linked settings.json is written through to
//!    its target, and everything after uses the resolved path, so the stage
//!    and the backups sit beside the real file;
//! 2. read its identity, then its bytes; the first read decides whether the
//!    file existed;
//! 3. plan (`plan.rs`); a refusal ends here, before anything is written;
//! 4. the hook copy, before settings.json may point at it;
//! 5. the saved status line, before settings.json stops pointing at the
//!    status line being taken over; if that can't be saved, nothing is;
//! 6. a backup of the bytes that were read;
//! 7. one atomic replace that only goes through while the file is still the
//!    one that was read. When Claude Code saved in between, the plan is made
//!    again from its version. When the file is gone, the pass stops: a file
//!    that was there is never replaced by one planned from nothing.
//!
//! So at every moment settings.json holds its old bytes or its new bytes.

use super::backups::{back_up_settings, newest_backup, newest_status_line};
use super::commands::{
    display_command, form_name, git_bash_candidates, hook_copy_path, is_upstream_hook,
    string_command_for, takeover, Recogniser, Subcommand, Takeover, HOOKS_DIR_NAME, HOOK_EXE_NAME,
    STATUS_LINE_NOT_AVAILABLE,
};
use super::copy::{install_hook_copy, remove_hook_copy};
use super::plan::{
    first_hook_entry, is_a_wrapper, plan_install, plan_uninstall, plan_upstream_removal,
    PreviousStatusLineChange, Refusal, Saved, SettingsPlan, SettingsWrite, StatusLineWish,
    CHAINS_NOTHING,
};
use crate::core::paths::PathStyle;
use crate::core::settings_doc::{is_blank, Json, SettingsDocument};
use crate::model::AccountId;
use crate::persist::hook_install::{HookInstallEntry, HookInstallRecord};
use crate::platform::{Clock, Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use crate::runtime_types::{
    CommandForm, FolderHookStatus, InstallChange, InstallOutcome, InstallPlan, StatusLineIntent,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SETTINGS_FILE_NAME: &str = "settings.json";
/// The `statusLine` object ours replaced, beside the hook copy. The wrapper
/// reads it to know what to chain to.
pub const PREVIOUS_STATUS_LINE_FILE_NAME: &str = "agentnotch-statusline.previous.json";

/// How many times a plan is made again because settings.json changed under
/// it, after the first.
pub const MAX_REPLANS: usize = 5;

pub const VANISHED: &str =
    "settings.json disappeared while Agent Notch was writing it; nothing was changed";
pub const KEPT_CHANGING: &str = "settings.json kept changing while we tried to update it";
pub const READ_ONLY: &str = "settings.json is read-only";
pub const CONFIG_DIR_MISSING: &str = "The config folder doesn't exist.";

/// What this PC offers an install, found once per pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    /// Git Bash is installed: someone else's status line may be wrapped.
    pub git_bash: bool,
}

impl Setup {
    /// Looks where Claude Code itself looks for Git Bash.
    pub fn detect() -> Setup {
        let git_bash = git_bash_candidates(|name| std::env::var(name).ok())
            .iter()
            .any(|candidate| candidate.is_file());
        Setup { git_bash }
    }
}

/// `<config folder>\hooks\agentnotch-statusline.previous.json`.
pub fn previous_status_line_path(config_dir: &Path) -> PathBuf {
    config_dir
        .join(HOOKS_DIR_NAME)
        .join(PREVIOUS_STATUS_LINE_FILE_NAME)
}

// ---- Resolving and reading ----

/// Why a settings.json isn't touched, before any plan.
enum Unusable {
    /// A link to a target that doesn't exist (a dotfiles repo not cloned
    /// yet): it stays a link.
    LinkBroken(String),
    /// There, but it can't be read: we have no idea what we would replace.
    Unreadable,
    FolderMissing,
}

impl Unusable {
    fn message(&self) -> String {
        match self {
            Unusable::LinkBroken(target) => format!(
                "settings.json links to {target}, which doesn't exist, so it was left alone."
            ),
            Unusable::Unreadable => Refusal::Unreadable.message().to_owned(),
            Unusable::FolderMissing => CONFIG_DIR_MISSING.to_owned(),
        }
    }
}

/// The file a settings.json path names, through any links. One that doesn't
/// exist yet resolves to its name in its (resolved) folder.
fn resolve(settings_path: &Path, files: &dyn SecureFiles) -> Result<PathBuf, Unusable> {
    if let Ok(resolved) = files.canonical(settings_path) {
        return Ok(resolved);
    }
    if files.is_reparse(settings_path).unwrap_or(false) {
        let target = fs::read_link(settings_path)
            .map(|target| match settings_path.parent() {
                Some(parent) if target.is_relative() => parent.join(target),
                _ => target,
            })
            .unwrap_or_else(|_| settings_path.to_path_buf());
        return Err(Unusable::LinkBroken(target.to_string_lossy().into_owned()));
    }
    if fs::symlink_metadata(settings_path).is_ok() {
        return Err(Unusable::Unreadable);
    }
    let (Some(parent), Some(name)) = (settings_path.parent(), settings_path.file_name()) else {
        return Err(Unusable::FolderMissing);
    };
    files
        .canonical(parent)
        .map(|folder| folder.join(name))
        .map_err(|_| Unusable::FolderMissing)
}

/// A settings.json at one moment: `None` when there is no file.
type FileState = Option<(Vec<u8>, FileIdentity)>;

/// The identity is taken before the bytes: a save that lands between the two
/// leaves an identity older than the bytes, which the write then refuses.
fn read_state(path: &Path, files: &dyn SecureFiles) -> Result<FileState, Unusable> {
    let missing = |error: &io::Error| error.kind() == io::ErrorKind::NotFound;
    let identity = match files.identity(path) {
        Ok(identity) => identity,
        Err(error) if missing(&error) => return Ok(None),
        Err(_) => return Err(Unusable::Unreadable),
    };
    match fs::read(path) {
        Ok(bytes) => Ok(Some((bytes, identity))),
        Err(error) if missing(&error) => Ok(None),
        Err(_) => Err(Unusable::Unreadable),
    }
}

/// The statusLine object saved by an earlier install, if any.
pub fn read_saved_status_line(path: &Path) -> Option<Json> {
    let value = Json::parse(&fs::read(path).ok()?).ok()?;
    value.is_object().then_some(value)
}

/// Where the status line our wrapper chains to is saved: beside the exe the
/// current `statusLine` runs, which is this folder's unless the settings.json
/// came from (or is shared with) another folder. That is the file the running
/// wrapper reads, so it is the one to restore from.
fn saved_status_line_path(
    existing: Option<&Json>,
    config_dir: &Path,
    recogniser: &Recogniser,
) -> PathBuf {
    let own = previous_status_line_path(config_dir);
    let Some(ours) = existing
        .and_then(|settings| settings.get("statusLine"))
        .and_then(|status_line| recogniser.entry(status_line))
        .filter(|found| found.subcommand == Subcommand::StatusLine)
    else {
        return own;
    };
    match recogniser.exe_path(&ours).parent() {
        Some(folder) if !folder.as_os_str().is_empty() => {
            folder.join(PREVIOUS_STATUS_LINE_FILE_NAME)
        }
        _ => own,
    }
}

/// A file only the user can read, replaced atomically: a saved status line
/// can hold what settings.json holds.
fn write_owner_only(bytes: &[u8], path: &Path, files: &dyn SecureFiles) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        let _ = fs::create_dir_all(folder);
    }
    match files.write_atomic(path, bytes, WriteMode::Private, Expect::Nothing) {
        Ok(WriteResult::Written) => Ok(()),
        Ok(WriteResult::ReadOnly) => Err("the file is read-only".to_owned()),
        Ok(WriteResult::Changed | WriteResult::Vanished) => Err("the file changed".to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

/// Puts back a lost saved status line (`PreviousStatusLineChange::Recover`).
/// Ours is the status line either way; the chain is a bonus, so a failure
/// here stops nothing.
fn recover_saved_status_line(bytes: &[u8], path: &Path, files: &dyn SecureFiles) {
    if fs::read(path).is_ok_and(|held| held == bytes) {
        return;
    }
    let _ = write_owner_only(bytes, path, files);
}

// ---- The write ----

/// What a pass over one settings.json came to.
struct Applied {
    /// The bytes written, `None` when the file was already as wanted.
    written: Option<Vec<u8>>,
    /// The bytes the last plan was made from (`None`: no file).
    read: Option<Vec<u8>>,
    backup: Option<PathBuf>,
    status_line: StatusLineIntent,
}

impl Applied {
    /// What settings.json holds now.
    fn bytes(&self) -> Option<&[u8]> {
        self.written.as_deref().or(self.read.as_deref())
    }
}

enum Failure {
    /// Left alone, with what Settings shows.
    Refused(String),
    /// The file went away mid-pass: nothing was written.
    Vanished,
}

impl Failure {
    fn result(self) -> Result<InstallChange, String> {
        match self {
            Failure::Refused(message) => Err(message),
            Failure::Vanished => Ok(InstallChange::Aborted(VANISHED.to_owned())),
        }
    }

    fn message(self) -> String {
        match self {
            Failure::Refused(message) => message,
            Failure::Vanished => VANISHED.to_owned(),
        }
    }
}

impl From<Unusable> for Failure {
    fn from(unusable: Unusable) -> Failure {
        Failure::Refused(unusable.message())
    }
}

/// One settings.json and the folder whose hook copy and saved status line go
/// with it.
struct Target<'a> {
    config_dir: &'a Path,
    /// Resolved.
    settings: &'a Path,
    files: &'a dyn SecureFiles,
    clock: &'a dyn Clock,
    recogniser: &'a Recogniser<'a>,
}

impl Target<'_> {
    /// Reads, plans and carries the plan out, planning again from a fresh
    /// read when the file changed in between. `first` is a read already
    /// made; `seen` says the pass has seen the file exist, after which a
    /// missing file is never "no file yet".
    fn apply(
        &self,
        first: FileState,
        mut seen: bool,
        planner: &mut dyn FnMut(Option<&[u8]>, &Saved) -> SettingsPlan,
    ) -> Result<Applied, Failure> {
        let previous_path = previous_status_line_path(self.config_dir);
        let mut pending = Some(first);

        for _ in 0..=MAX_REPLANS {
            let state = match pending.take() {
                Some(state) => state,
                None => read_state(self.settings, self.files)?,
            };
            let (existing, identity) = match state {
                Some((bytes, identity)) => (Some(bytes), Some(identity)),
                None if seen => return Err(Failure::Vanished),
                None => (None, None),
            };
            seen |= existing.is_some();

            let parsed =
                SettingsDocument::new(existing.as_deref()).map(|document| document.value().clone());
            let saved_status_line = read_saved_status_line(&saved_status_line_path(
                parsed.as_ref(),
                self.config_dir,
                self.recogniser,
            ));
            let from_backups = || {
                newest_status_line(self.settings, &|status_line| {
                    is_a_wrapper(status_line, self.recogniser)
                })
            };
            let plan = planner(
                existing.as_deref(),
                &Saved {
                    previous_status_line: saved_status_line.as_ref(),
                    backup_status_line: &from_backups,
                },
            );

            let new_bytes = match plan.settings {
                SettingsWrite::Refuse(refusal) => {
                    return Err(Failure::Refused(refusal.message().to_owned()))
                }
                SettingsWrite::AlreadyCurrent => {
                    match &plan.previous_status_line {
                        Some(PreviousStatusLineChange::Remove) => {
                            let _ = fs::remove_file(&previous_path);
                        }
                        Some(PreviousStatusLineChange::ChainNothing) => {
                            recover_saved_status_line(CHAINS_NOTHING, &previous_path, self.files);
                        }
                        Some(PreviousStatusLineChange::Recover(bytes)) => {
                            recover_saved_status_line(bytes, &previous_path, self.files);
                        }
                        Some(PreviousStatusLineChange::Save(_)) | None => {}
                    }
                    return Ok(Applied {
                        written: None,
                        read: existing,
                        backup: None,
                        status_line: plan.status_line,
                    });
                }
                SettingsWrite::Write(bytes) => bytes,
            };

            // Known before anything else is written for this file.
            if fs::metadata(self.settings).is_ok_and(|meta| meta.permissions().readonly()) {
                return Err(Failure::Refused(READ_ONLY.to_owned()));
            }

            // Save the status line being taken over BEFORE settings.json
            // stops pointing at it; if that fails, don't take it over.
            match &plan.previous_status_line {
                Some(PreviousStatusLineChange::Save(bytes)) => {
                    write_owner_only(bytes, &previous_path, self.files).map_err(|error| {
                        Failure::Refused(format!("Couldn't save the current status line: {error}"))
                    })?;
                }
                // Only a guard against an older status line coming back;
                // without it a lost file falls back to the backups.
                Some(PreviousStatusLineChange::ChainNothing) => {
                    let _ = write_owner_only(CHAINS_NOTHING, &previous_path, self.files);
                }
                Some(PreviousStatusLineChange::Recover(bytes)) => {
                    recover_saved_status_line(bytes, &previous_path, self.files);
                }
                Some(PreviousStatusLineChange::Remove) | None => {}
            }

            // As on the Mac, a backup that couldn't be made doesn't stop the
            // write: the replace is atomic, so the old bytes are only
            // without a second copy, never half written.
            let backup = existing
                .as_deref()
                .filter(|bytes| !is_blank(bytes))
                .and_then(|bytes| {
                    back_up_settings(bytes, self.settings, self.files, self.clock)
                        .path()
                        .map(Path::to_path_buf)
                });

            let expect = identity.map_or(Expect::Absent, Expect::Same);
            let result = self.files.write_atomic(
                self.settings,
                &new_bytes,
                WriteMode::KeepTargetSecurity,
                expect,
            );
            match result {
                Ok(WriteResult::Written) => {
                    if plan.previous_status_line == Some(PreviousStatusLineChange::Remove) {
                        let _ = fs::remove_file(&previous_path);
                    }
                    return Ok(Applied {
                        written: Some(new_bytes),
                        read: existing,
                        backup,
                        status_line: plan.status_line,
                    });
                }
                // Claude Code saved meanwhile: plan again from its version.
                Ok(WriteResult::Changed) => continue,
                Ok(WriteResult::Vanished) => return Err(Failure::Vanished),
                Ok(WriteResult::ReadOnly) => return Err(Failure::Refused(READ_ONLY.to_owned())),
                Err(error) => {
                    return Err(Failure::Refused(format!(
                        "Couldn't write settings.json: {error}"
                    )))
                }
            }
        }
        Err(Failure::Refused(KEPT_CHANGING.to_owned()))
    }

    /// Whether the `statusLine` in `settings` (a settings.json's bytes) runs
    /// our wrapper.
    fn status_line_is_ours(&self, settings: Option<&[u8]>) -> bool {
        settings
            .and_then(|bytes| SettingsDocument::new(Some(bytes)))
            .is_some_and(|document| {
                self.recogniser
                    .is_our_status_line(document.get("statusLine"))
            })
    }
}

fn is_a_folder(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// The folder a plan's hook copy and saved status line belong to: the one
/// holding the copy, else the one holding settings.json.
fn config_dir_of(hook_copy: Option<&Path>, settings_path: &Path) -> PathBuf {
    hook_copy
        .and_then(Path::parent)
        .and_then(Path::parent)
        .or_else(|| settings_path.parent())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn outcome(
    folder: &AccountId,
    settings_path: &Path,
    result: Result<InstallChange, String>,
) -> InstallOutcome {
    InstallOutcome {
        folder: folder.clone(),
        settings_path: settings_path.to_path_buf(),
        result,
        backup: None,
        entry: None,
        status_line: None,
    }
}

// ---- Install ----

/// Carries one plan out: the hook copy, then settings.json. Idempotent: a
/// second run with nothing changed writes nothing. A `remove_only` plan
/// removes ours instead. A Job on `an-io`.
pub fn apply_install(
    plan: &InstallPlan,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> InstallOutcome {
    apply_install_with(plan, files, clock, &Setup::detect())
}

pub fn apply_install_with(
    plan: &InstallPlan,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
    setup: &Setup,
) -> InstallOutcome {
    let config_dir = config_dir_of(
        plan.hook_copy.as_ref().map(|(_, copy)| copy.as_path()),
        &plan.settings_path,
    );
    if plan.remove_only {
        return uninstall(
            &plan.folder,
            &config_dir,
            &plan.settings_path,
            plan.existed || matches!(plan.expected, Expect::Same(_)),
            files,
            clock,
        );
    }
    let refused =
        |settings_path: &Path, message: String| outcome(&plan.folder, settings_path, Err(message));
    if !is_a_folder(&config_dir) {
        return refused(&plan.settings_path, CONFIG_DIR_MISSING.to_owned());
    }

    // A path no string command can carry as it is may still have an 8.3
    // spelling that can; only the file system knows, so it is asked here.
    let exe = hook_copy_path(&config_dir);
    let short_path = |path: &Path| files.short_path(path);
    let form = match &plan.form {
        CommandForm::NotPossible(reason) => {
            match string_command_for(&exe, Subcommand::Hook, &short_path) {
                Some(command) => CommandForm::Text(command),
                None => return refused(&plan.settings_path, reason.clone()),
            }
        }
        form => form.clone(),
    };

    let resolved = match resolve(&plan.settings_path, files) {
        Ok(resolved) => resolved,
        Err(unusable) => return refused(&plan.settings_path, unusable.message()),
    };
    let first = match read_state(&resolved, files) {
        Ok(state) => state,
        Err(unusable) => return refused(&resolved, unusable.message()),
    };
    // A plan made from a file that was there is never carried out on none.
    let seen = plan.existed || matches!(plan.expected, Expect::Same(_));
    if first.is_none() && seen {
        return outcome(&plan.folder, &resolved, Failure::Vanished.result());
    }

    // Don't leave a hook copy behind for a settings.json we'd refuse anyway.
    // (The plan checks again right before the write.)
    let existing = first.as_ref().map(|(bytes, _)| bytes.as_slice());
    let refusal = match SettingsDocument::new(existing) {
        None => Some(Refusal::Unreadable),
        Some(document) => document
            .get("hooks")
            .is_some_and(|hooks| !hooks.is_object())
            .then_some(Refusal::HooksNotAnObject),
    };
    if let Some(refusal) = refusal {
        return refused(&resolved, refusal.message().to_owned());
    }

    // The copy first: settings.json must never point at an exe that isn't
    // there.
    match &plan.hook_copy {
        Some((source, _)) => {
            if let Err(error) = install_hook_copy(source, &config_dir, clock) {
                return refused(&resolved, error.to_string());
            }
        }
        None if exe.is_file() => {}
        None => return refused(&resolved, format!("{HOOK_EXE_NAME} is not available")),
    }

    // The status line is always a string command, under the same rule as
    // the hooks'. Where none can carry the path, it is left as it is.
    let status_command = string_command_for(&exe, Subcommand::StatusLine, &short_path);
    let (wish, not_available) = match (&plan.status_line, &status_command) {
        (StatusLineIntent::Wrap | StatusLineIntent::UpdateCommand, Some(command)) => (
            StatusLineWish::Wrap {
                command,
                git_bash: setup.git_bash,
            },
            false,
        ),
        (StatusLineIntent::Wrap | StatusLineIntent::UpdateCommand, None) => {
            (StatusLineWish::Leave, true)
        }
        (StatusLineIntent::Unwrap, _) => (StatusLineWish::Unwrap, false),
        (StatusLineIntent::LeaveAlone(_) | StatusLineIntent::Nothing, _) => {
            (StatusLineWish::Leave, false)
        }
    };

    let long_path = |path: &Path| files.long_path(path);
    let recogniser = Recogniser::with_long_paths(PathStyle::native(), &long_path);
    let target = Target {
        config_dir: &config_dir,
        settings: &resolved,
        files,
        clock,
        recogniser: &recogniser,
    };
    let applied = target.apply(first, seen, &mut |existing, saved| {
        plan_install(existing, &form, &plan.events, wish, saved, &recogniser)
    });
    let applied = match applied {
        Ok(applied) => applied,
        Err(failure) => return outcome(&plan.folder, &resolved, failure.result()),
    };

    let status_line_ours = target.status_line_is_ours(applied.bytes());
    // With the integration off and our wrapper no longer in settings.json,
    // a saved status line has nothing left to say.
    if wish == StatusLineWish::Unwrap && !status_line_ours {
        let _ = fs::remove_file(previous_status_line_path(&config_dir));
    }

    InstallOutcome {
        folder: plan.folder.clone(),
        settings_path: resolved.clone(),
        result: Ok(if applied.written.is_some() {
            InstallChange::Written
        } else {
            InstallChange::Unchanged
        }),
        backup: applied.backup,
        entry: Some(HookInstallEntry {
            settings_path: resolved,
            folder: plan.folder.clone(),
            form: form_name(&form).unwrap_or_default().to_owned(),
            command: match &form {
                CommandForm::Exec { command, .. } => command.to_string_lossy().into_owned(),
                form => display_command(form).unwrap_or_default(),
            },
            hook_copy: Some(exe),
            status_line: status_line_ours,
            installed_at: clock.now(),
        }),
        status_line: Some(if not_available {
            StatusLineIntent::LeaveAlone(STATUS_LINE_NOT_AVAILABLE.to_owned())
        } else {
            applied.status_line
        }),
    }
}

/// Carries a whole pass out, writing each physical settings.json once: two
/// folders whose settings.json is one file (a link) would otherwise replace
/// each other's entries on every pass. Installs come first, so a folder that
/// is only being cleaned never strips what a tracked folder sharing its file
/// just wrote. The outcomes are in the plans' order; a plan whose file
/// another plan wrote reports `Unchanged` and no entry.
pub fn apply_installs(
    plans: &[InstallPlan],
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> Vec<InstallOutcome> {
    apply_installs_with(plans, files, clock, &Setup::detect())
}

pub fn apply_installs_with(
    plans: &[InstallPlan],
    files: &dyn SecureFiles,
    clock: &dyn Clock,
    setup: &Setup,
) -> Vec<InstallOutcome> {
    let keys: Vec<PhysicalFile> = plans
        .iter()
        .map(|plan| PhysicalFile::of(&plan.settings_path, files))
        .collect();
    let mut outcomes: Vec<Option<InstallOutcome>> = plans.iter().map(|_| None).collect();
    let mut done: Vec<usize> = Vec::new();
    let installs = (0..plans.len()).filter(|&index| !plans[index].remove_only);
    let removals = (0..plans.len()).filter(|&index| plans[index].remove_only);
    for index in installs.chain(removals) {
        let plan = &plans[index];
        outcomes[index] = Some(if done.iter().any(|&other| keys[other] == keys[index]) {
            outcome(
                &plan.folder,
                &keys[index].path,
                Ok(InstallChange::Unchanged),
            )
        } else {
            done.push(index);
            apply_install_with(plan, files, clock, setup)
        });
    }
    outcomes.into_iter().flatten().collect()
}

/// Which file a settings.json path comes to.
struct PhysicalFile {
    path: PathBuf,
    /// (volume, file index), where the file exists and the OS says.
    id: Option<(u64, u128)>,
}

impl PhysicalFile {
    fn of(settings_path: &Path, files: &dyn SecureFiles) -> PhysicalFile {
        let path = resolve(settings_path, files).unwrap_or_else(|_| settings_path.to_path_buf());
        let id = files
            .identity(&path)
            .ok()
            .map(|identity| (identity.volume, identity.index))
            // An implementation that can't tell files apart says zero.
            .filter(|id| *id != (0, 0));
        PhysicalFile { path, id }
    }
}

impl PartialEq for PhysicalFile {
    fn eq(&self, other: &PhysicalFile) -> bool {
        self.path == other.path || (self.id.is_some() && self.id == other.id)
    }
}

// ---- Uninstall ----

/// Removes our hooks from one settings.json and gives the status line back,
/// then deletes what the folder holds of ours. Other tools' entries are
/// never touched.
fn uninstall(
    folder: &AccountId,
    config_dir: &Path,
    settings_path: &Path,
    seen: bool,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> InstallOutcome {
    if !is_a_folder(config_dir) {
        return outcome(folder, settings_path, Err(CONFIG_DIR_MISSING.to_owned()));
    }
    let resolved = match resolve(settings_path, files) {
        Ok(resolved) => resolved,
        Err(unusable) => return outcome(folder, settings_path, Err(unusable.message())),
    };
    let first = match read_state(&resolved, files) {
        Ok(state) => state,
        Err(unusable) => return outcome(folder, &resolved, Err(unusable.message())),
    };

    let long_path = |path: &Path| files.long_path(path);
    let recogniser = Recogniser::with_long_paths(PathStyle::native(), &long_path);
    let target = Target {
        config_dir,
        settings: &resolved,
        files,
        clock,
        recogniser: &recogniser,
    };
    let applied = target.apply(first, seen, &mut |existing, saved| {
        plan_uninstall(existing, saved, &recogniser)
    });
    let applied = match applied {
        Ok(applied) => applied,
        Err(failure) => return outcome(folder, &resolved, failure.result()),
    };

    // Only once settings.json no longer references them.
    let _ = fs::remove_file(previous_status_line_path(config_dir));
    // A copy a hook is still running is renamed aside; the next sweep (or
    // the uninstaller) deletes it.
    let _ = remove_hook_copy(config_dir, clock);
    // The folder ours went into, when nothing else is in it (and it isn't a
    // link to someone's folder).
    let hooks = config_dir.join(HOOKS_DIR_NAME);
    if !files.is_reparse(&hooks).unwrap_or(true) {
        let _ = fs::remove_dir(&hooks);
    }

    InstallOutcome {
        folder: folder.clone(),
        settings_path: resolved,
        result: Ok(if applied.written.is_some() {
            InstallChange::Removed
        } else {
            InstallChange::Unchanged
        }),
        backup: applied.backup,
        entry: None,
        status_line: Some(applied.status_line),
    }
}

/// Removes everything this app wrote, from the record alone (no hub, no
/// folder discovery): our entries, the wrapped status lines, the saved
/// status lines, the hook copies and their leftovers.
pub fn uninstall_everything(
    record: &HookInstallRecord,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> Vec<InstallOutcome> {
    record
        .files
        .iter()
        .map(|entry| {
            let config_dir = config_dir_of(entry.hook_copy.as_deref(), &entry.settings_path);
            uninstall(
                &entry.folder,
                &config_dir,
                &entry.settings_path,
                false,
                files,
                clock,
            )
        })
        .collect()
}

// ---- The official app's hooks ----

/// Removes the official Codenotch's hook entries from one settings.json, on
/// the user's say-so, and says how many went. Ours and everyone else's stay
/// as they are; its files are left alone. With none to remove the file isn't
/// written.
pub fn remove_codenotch_hooks(
    settings_path: &Path,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> Result<u32, String> {
    let config_dir = settings_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if !is_a_folder(&config_dir) {
        return Err(CONFIG_DIR_MISSING.to_owned());
    }
    let resolved = resolve(settings_path, files).map_err(|unusable| unusable.message())?;
    let first = read_state(&resolved, files).map_err(|unusable| unusable.message())?;

    let recogniser = Recogniser::new(PathStyle::native());
    let target = Target {
        config_dir: &config_dir,
        settings: &resolved,
        files,
        clock,
        recogniser: &recogniser,
    };
    let mut removed = 0;
    let applied = target.apply(first, false, &mut |existing, _| {
        let (plan, count) = plan_upstream_removal(existing);
        removed = count;
        // The plan also drops events left holding nothing; with none of its
        // entries there, that alone is no reason to write the user's file.
        if count == 0 && matches!(plan.settings, SettingsWrite::Write(_)) {
            return SettingsPlan {
                settings: SettingsWrite::AlreadyCurrent,
                previous_status_line: None,
                status_line: StatusLineIntent::Nothing,
            };
        }
        plan
    });
    match applied {
        Ok(_) => Ok(removed),
        Err(failure) => Err(failure.message()),
    }
}

// ---- Status ----

/// Reads back what one folder's settings.json registers (HS§3.8). Read-only:
/// safe whatever the consent. `last_outcome`, `last_error` and
/// `not_hookable` are the manager's to fill in; `status_line_left_alone` is
/// why the status line there would not be taken over, which only matters
/// while the integration is on.
pub fn read_status(config_dir: &Path, files: &dyn SecureFiles, setup: &Setup) -> FolderHookStatus {
    let mut status = FolderHookStatus {
        settings_readable: true,
        ..FolderHookStatus::default()
    };
    status.config_dir_exists = is_a_folder(config_dir);
    if !status.config_dir_exists {
        return status;
    }
    let settings_path = config_dir.join(SETTINGS_FILE_NAME);
    let resolved = match resolve(&settings_path, files) {
        Ok(resolved) => resolved,
        Err(_) => {
            status.newest_backup = newest_backup(&settings_path);
            status.settings_readable = false;
            return status;
        }
    };
    status.newest_backup = newest_backup(&resolved);
    let bytes = match read_state(&resolved, files) {
        Ok(Some((bytes, _))) => bytes,
        Ok(None) => return status,
        Err(_) => {
            status.settings_readable = false;
            return status;
        }
    };
    let Some(document) = SettingsDocument::new(Some(&bytes)) else {
        status.settings_readable = false;
        return status;
    };
    if document
        .get("hooks")
        .is_some_and(|hooks| !hooks.is_object())
    {
        status.settings_readable = false;
    }
    let settings = document.value();
    let long_path = |path: &Path| files.long_path(path);
    let recogniser = Recogniser::with_long_paths(PathStyle::native(), &long_path);

    // The exe the entries actually run: normally this folder's copy, but a
    // settings.json shared with another folder (a link, or a copy) runs that
    // folder's, which works as well.
    if let Some(entry) = first_hook_entry(settings, &|entry| recogniser.is_our_hook(entry)) {
        status.hooks_registered = true;
        if let Some(found) = recogniser.entry(entry) {
            status.hooks_installed = recogniser.exe_path(&found).is_file();
            status.form = Some(if found.exec_form { "exec" } else { "string" }.to_owned());
        }
        status.hook_command = entry.get("command").and_then(Json::as_str).map(|command| {
            let args = entry.get("args").and_then(Json::items).unwrap_or_default();
            args.iter()
                .filter_map(Json::as_str)
                .fold(command.to_owned(), |text, arg| format!("{text} {arg}"))
        });
    }

    let status_line = document.get("statusLine");
    if let Some(found) = status_line
        .and_then(|value| recogniser.entry(value))
        .filter(|found| found.subcommand == Subcommand::StatusLine)
    {
        status.status_line_installed = recogniser.exe_path(&found).is_file();
    } else {
        let short_path = |path: &Path| files.short_path(path);
        let command = string_command_for(
            &hook_copy_path(config_dir),
            Subcommand::StatusLine,
            &short_path,
        );
        status.status_line_left_alone = match takeover(status_line, &recogniser, setup.git_bash) {
            _ if command.is_none() => Some(STATUS_LINE_NOT_AVAILABLE.to_owned()),
            Takeover::LeaveAlone(reason) => Some(reason),
            Takeover::Install | Takeover::Update | Takeover::Wrap => None,
        };
    }
    status.codenotch_hooks = first_hook_entry(settings, &is_upstream_hook).is_some();
    status
}
