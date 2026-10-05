//! Which folders get this app's hooks, and when (AccountHookManager.swift;
//! DESIGN-WIN §4.3, HS§3.1).
//!
//! The manager is pure: it reads no file and writes none. It turns the
//! accounts, the settings and the Claude Code versions seen into
//! `InstallPlan`s, which an Io job carries out (`apply_installs`), and takes
//! the outcomes back to know what each folder holds and what
//! `hook-install.json` must say.
//!
//! Nothing is planned:
//! - before the user answers "Turn on Claude Code control" with yes
//!   (`hook_consent`), not even a removal: until then this app wrote nothing;
//! - when installs are disabled for the run (`--no-install`, sealed);
//! - for a Claude Parallel Profiles store or the shared history, which are
//!   never hooked. Ours is only taken out of one when it is known to be
//!   there (a folder that was a run folder when it was hooked).
//!
//! After the yes, tracked run folders get the hooks while the Hooks switch
//! is on, and every other folder that may hold ours has them taken out:
//! untracked run folders, and with the switch off every run folder and
//! every file `hook-install.json` names.

use super::apply::{apply_installs, CONFIG_DIR_MISSING, SETTINGS_FILE_NAME};
use super::commands::{
    display_command, effective_version, exec_form, exec_form_allowed, hook_copy_path,
    not_hookable_reason, string_command, Subcommand,
};
use super::events::hook_events;
use super::facts::ClaudeCodeFacts;
use super::version::ClaudeCodeVersion;
use crate::core::flags::DevFlags;
use crate::core::paths::{PathStyle, Paths};
use crate::core::settings::ControlSettings;
use crate::model::{Account, AccountId, ConsentFile, FolderKind, RunFolder};
use crate::persist::hook_install::{HookInstallEntry, HookInstallRecord};
use crate::platform::{Clock, Expect, SecureFiles};
use crate::runtime_types::{
    CommandForm, FolderHookStatus, InstallChange, InstallOutcome, InstallPlan, StatusLineIntent,
    VersionSighting,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often every folder is checked again (a folder moved, a hook copy
/// deleted, settings.json rewritten by a dotfiles sync). A pass writes only
/// where something differs.
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Accounts change in bursts (a scan finds several folders, a login lands in
/// two files): one pass, a second after the last change.
pub const ACCOUNTS_DEBOUNCE: Duration = Duration::from_secs(1);

/// What is known of one folder.
#[derive(Debug, Clone, Default)]
struct Known {
    status: FolderHookStatus,
    /// Whether the folder's settings.json holds ours: `None` until a read or
    /// an outcome said.
    ours: Option<bool>,
}

/// When hook passes run. Every method takes `now`, so the hub only drives it.
#[derive(Debug, Clone, Default)]
struct Timing {
    started: bool,
    /// A pass someone asked for: the start, a switch, a lower version.
    requested: Option<SystemTime>,
    /// The accounts changed: a pass once they have been quiet.
    settled: Option<SystemTime>,
    recheck: Option<SystemTime>,
    /// What the last pass wrote hooks for: `None` when it wrote none,
    /// `Some(None)` for the baseline events.
    written_for: Option<Option<ClaudeCodeVersion>>,
    wrote_exec_form: bool,
}

#[derive(Default)]
pub struct HookManager {
    /// `<install dir>\agentnotch-hook.exe`, copied into each hooked folder.
    hook_source: Option<PathBuf>,
    /// `--no-install` or sealed: nothing is planned in this run.
    installs_disabled: bool,
    known: Vec<(AccountId, Known)>,
    record: HookInstallRecord,
    timing: Timing,
}

impl HookManager {
    /// A manager that knows no hook exe to copy: its plans only work where a
    /// copy is already in place. The hub uses `configured`.
    pub fn new() -> Self {
        Self::default()
    }

    /// `hook_source` is the hook exe beside the app.
    pub fn configured(hook_source: PathBuf, flags: &DevFlags) -> Self {
        HookManager {
            hook_source: Some(hook_source),
            installs_disabled: !flags.installs_allowed(),
            ..Self::default()
        }
    }

    /// Installs are disabled for this run (`--no-install`, sealed).
    pub fn installs_disabled(&self) -> bool {
        self.installs_disabled
    }

    // ---- Planning ----

    /// What the next pass does. Empty before the yes and when installs are
    /// disabled. Installs into every tracked run folder while hooks are on;
    /// removals for the folders that may hold ours and shouldn't.
    pub fn plan(
        &self,
        accounts: &[Account],
        folders: &[RunFolder],
        settings: &ControlSettings,
        versions: &[VersionSighting],
        facts: &ClaudeCodeFacts,
    ) -> Vec<InstallPlan> {
        if self.installs_disabled || settings.hook_consent != Some(true) {
            return Vec::new();
        }
        let installing = settings.hooks_active();
        let events: Vec<String> = hook_events(effective_version(versions))
            .into_iter()
            .map(str::to_owned)
            .collect();
        let exec = exec_form_allowed(versions, facts);
        let status_line = if settings.status_line_integration {
            StatusLineIntent::Wrap
        } else {
            StatusLineIntent::Unwrap
        };

        let mut plans = Vec::new();
        for folder in folders {
            if installing && is_install_target(accounts, folder) {
                let exe = hook_copy_path(&folder.config_dir);
                plans.push(InstallPlan {
                    folder: folder.id.clone(),
                    settings_path: folder.config_dir.join(SETTINGS_FILE_NAME),
                    expected: Expect::Nothing,
                    existed: false,
                    hook_copy: self
                        .hook_source
                        .as_ref()
                        .map(|source| (source.clone(), exe.clone())),
                    form: command_form(&exe, exec, facts),
                    events: events.clone(),
                    status_line: status_line.clone(),
                    remove_only: false,
                });
            } else if self.may_hold_ours(folder) {
                plans.push(removal_plan(folder));
            }
        }
        // With hooks off, what the record names goes too, whether or not the
        // folder is still known. While they are on, a folder that is merely
        // not listed yet (a scan still running) keeps its hooks.
        if !installing {
            plans.extend(
                self.record
                    .files
                    .iter()
                    .filter(|entry| !folders.iter().any(|folder| folder.id == entry.folder))
                    .map(|entry| self.removal_plan_for_entry(entry)),
            );
        }
        plans
    }

    /// Removals for folders the user forgets (or untracks by hand): ours
    /// comes out while the folder is still known. Empty before the yes and
    /// when installs are disabled.
    pub fn forget_plans(
        &self,
        folders: &[RunFolder],
        settings: &ControlSettings,
    ) -> Vec<InstallPlan> {
        if self.installs_disabled || settings.hook_consent != Some(true) {
            return Vec::new();
        }
        folders
            .iter()
            .filter(|folder| self.may_hold_ours(folder))
            .map(removal_plan)
            .collect()
    }

    /// Whether a folder's settings.json may hold ours. A run folder does
    /// until a read or a removal said otherwise. A store or the shared
    /// history only when ours was seen there: they are never written
    /// otherwise, not even to look.
    fn may_hold_ours(&self, folder: &RunFolder) -> bool {
        if self.record.entry_for(&folder.id).is_some() {
            return true;
        }
        let ours = self.known(&folder.id).and_then(|known| known.ours);
        match folder.kind {
            FolderKind::Run => ours != Some(false),
            FolderKind::Store | FolderKind::Infrastructure => ours == Some(true),
        }
    }

    fn removal_plan_for_entry(&self, entry: &HookInstallEntry) -> InstallPlan {
        removal_plan_for_entry(entry, self.hook_source.as_deref())
    }

    // ---- What each folder holds ----

    fn known(&self, folder: &AccountId) -> Option<&Known> {
        self.known
            .iter()
            .find(|(id, _)| id == folder)
            .map(|(_, known)| known)
    }

    fn known_mut(&mut self, folder: &AccountId) -> &mut Known {
        let index = match self.known.iter().position(|(id, _)| id == folder) {
            Some(index) => index,
            None => {
                self.known.push((folder.clone(), Known::default()));
                self.known.len() - 1
            }
        };
        &mut self.known[index].1
    }

    /// What a folder's settings.json registers, with the last pass's result.
    /// The default (nothing known) for a folder never read.
    pub fn folder_status(&self, folder: &AccountId) -> FolderHookStatus {
        self.known(folder)
            .map(|known| known.status.clone())
            .unwrap_or_default()
    }

    /// Takes in what `read_status` found on disk. The last outcome, the last
    /// error and the not-hookable reason stay the manager's. Why the status
    /// line was left alone only matters while the integration is on.
    pub fn absorb_status(
        &mut self,
        folder: &AccountId,
        disk: FolderHookStatus,
        settings: &ControlSettings,
    ) {
        let known = self.known_mut(folder);
        let mut status = disk;
        status.last_outcome = known.status.last_outcome.take();
        status.last_error = known.status.last_error.take();
        status.not_hookable = known.status.not_hookable.take();
        if !settings.status_line_integration {
            status.status_line_left_alone = None;
        }
        known.ours = Some(status.hooks_registered || status.status_line_installed);
        known.status = status;
    }

    /// Takes in a pass's outcomes: each folder's last result, what it now
    /// holds, and the install record. `plans` are the ones the outcomes
    /// answer. Says whether the record changed, which is when
    /// `hook-install.json` must be written again (`record`).
    pub fn absorb_outcomes(&mut self, plans: &[InstallPlan], outcomes: &[InstallOutcome]) -> bool {
        for outcome in outcomes {
            let Some(plan) = plans.iter().find(|plan| plan.folder == outcome.folder) else {
                continue;
            };
            let known = self.known_mut(&outcome.folder);
            note_outcome(known, plan, outcome);
        }
        let updated = updated_record(&self.record, plans, outcomes);
        let changed = updated != self.record;
        self.record = updated;
        changed
    }

    /// Drops what is known of folders the registry no longer lists.
    pub fn retain_folders(&mut self, folders: &[RunFolder]) {
        self.known
            .retain(|(id, _)| folders.iter().any(|folder| &folder.id == id));
    }

    /// Folders whose settings.json holds the official app's hook entries
    /// (`setup.codenotch_hooks_folders`). Known only from `absorb_status`.
    pub fn codenotch_hooks_folders(&self) -> Vec<String> {
        self.known
            .iter()
            .filter(|(_, known)| known.status.codenotch_hooks)
            .map(|(id, _)| id.0.clone())
            .collect()
    }

    /// Tracked run folders whose hooks aren't in place, as far as is known.
    pub fn folders_missing_hooks(
        &self,
        accounts: &[Account],
        folders: &[RunFolder],
    ) -> Vec<AccountId> {
        folders
            .iter()
            .filter(|folder| is_install_target(accounts, folder))
            .filter(|folder| !self.folder_status(&folder.id).hooks_installed)
            .map(|folder| folder.id.clone())
            .collect()
    }

    // ---- The install record ----

    /// Every settings.json holding ours, for `hook-install.json`
    /// (`HookInstallFile::from_model`).
    pub fn record(&self) -> &HookInstallRecord {
        &self.record
    }

    /// The record read at launch (`HookInstallFile::to_model`).
    pub fn set_record(&mut self, record: HookInstallRecord) {
        self.record = record;
    }

    // ---- When passes run ----

    /// The first pass is due now, and one every `RECHECK_INTERVAL` after
    /// each pass. Idempotent.
    pub fn start(&mut self, now: SystemTime) {
        if self.timing.started {
            return;
        }
        self.timing.started = true;
        self.timing.requested = Some(now);
    }

    pub fn stop(&mut self) {
        self.timing = Timing::default();
    }

    /// A pass now: the user answered the consent card, flipped a switch, or
    /// tracked or untracked an account.
    pub fn request_pass(&mut self, now: SystemTime) {
        if self.timing.started {
            self.timing.requested = Some(now);
        }
    }

    /// The set of folders, or which of them are tracked, changed: a pass a
    /// second after the last such change.
    pub fn accounts_changed(&mut self, now: SystemTime) {
        if self.timing.started {
            self.timing.settled = Some(now + ACCOUNTS_DEBOUNCE);
        }
    }

    /// The Claude Code versions seen changed. A pass is due when the hooks
    /// were written for a newer version than the lowest now known (an older
    /// Claude Code may ignore a settings.json naming events it doesn't
    /// know), or in the exec form and a copy that can't be trusted with it
    /// showed up. Hooks written for the baseline have nothing to take back.
    /// Says whether it asked for a pass.
    pub fn note_versions(
        &mut self,
        versions: &[VersionSighting],
        facts: &ClaudeCodeFacts,
        now: SystemTime,
    ) -> bool {
        if !self.timing.started {
            return false;
        }
        let lower = match (self.timing.written_for, effective_version(versions)) {
            (Some(Some(written)), Some(seen)) => seen < written,
            _ => false,
        };
        let exec_lost = self.timing.wrote_exec_form && !exec_form_allowed(versions, facts);
        if lower || exec_lost {
            self.timing.requested = Some(now);
        }
        lower || exec_lost
    }

    /// When the next pass is due; `None` before `start`.
    pub fn next_pass_at(&self) -> Option<SystemTime> {
        [
            self.timing.requested,
            self.timing.settled,
            self.timing.recheck,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn pass_due(&self, now: SystemTime) -> bool {
        self.next_pass_at().is_some_and(|at| at <= now)
    }

    /// The hub planned a pass from `versions` at `now`: everything asked for
    /// so far is answered by it, and the next recheck is a full interval
    /// away.
    pub fn pass_started(
        &mut self,
        settings: &ControlSettings,
        versions: &[VersionSighting],
        facts: &ClaudeCodeFacts,
        now: SystemTime,
    ) {
        let writes = !self.installs_disabled && settings.hooks_active();
        self.timing.written_for = writes.then(|| effective_version(versions));
        self.timing.wrote_exec_form = writes && exec_form_allowed(versions, facts);
        self.timing.requested = None;
        self.timing.settled = None;
        if self.timing.started {
            self.timing.recheck = Some(now + RECHECK_INTERVAL);
        }
    }
}

// ---- Targets ----

/// Whether a folder gets our hooks: a run folder of a tracked account. A
/// folder several accounts run in (`~\.claude` while Claude Parallel
/// Profiles mirrors accounts into it) keeps them while any of those is
/// tracked. A run folder no account lists goes by its own switch. Never a
/// store or the shared history, whatever lists them.
pub fn is_install_target(accounts: &[Account], folder: &RunFolder) -> bool {
    if folder.kind != FolderKind::Run {
        return false;
    }
    let mut owners = accounts
        .iter()
        .filter(|account| account.run_dirs.contains(&folder.id))
        .peekable();
    if owners.peek().is_none() {
        return !folder.is_hidden;
    }
    owners.any(|account| account.is_tracked)
}

/// The hook command for the copy at `exe`, from its path alone: the exec
/// form when every Claude Code here runs it, else the path unquoted. A path
/// no string can carry is left to the install, which asks the file system
/// for an 8.3 spelling and refuses with this reason when there is none.
fn command_form(exe: &Path, exec: bool, facts: &ClaudeCodeFacts) -> CommandForm {
    if exec {
        return exec_form(exe);
    }
    match string_command(&exe.to_string_lossy(), Subcommand::Hook) {
        Some(command) => CommandForm::Text(command),
        None => CommandForm::NotPossible(not_hookable_reason(facts)),
    }
}

fn removal_plan(folder: &RunFolder) -> InstallPlan {
    InstallPlan {
        folder: folder.id.clone(),
        settings_path: folder.config_dir.join(SETTINGS_FILE_NAME),
        expected: Expect::Nothing,
        existed: false,
        hook_copy: None,
        form: CommandForm::NotPossible(String::new()),
        events: Vec::new(),
        status_line: StatusLineIntent::Unwrap,
        remove_only: true,
    }
}

/// A removal from what the record says alone. The recorded settings.json is
/// the resolved file, which may sit outside the folder (a link), so the
/// folder is named through its hook copy.
fn removal_plan_for_entry(entry: &HookInstallEntry, hook_source: Option<&Path>) -> InstallPlan {
    InstallPlan {
        folder: entry.folder.clone(),
        settings_path: entry.settings_path.clone(),
        expected: Expect::Nothing,
        existed: false,
        hook_copy: entry.hook_copy.as_ref().map(|copy| {
            (
                hook_source.map(Path::to_path_buf).unwrap_or_default(),
                copy.clone(),
            )
        }),
        form: CommandForm::NotPossible(String::new()),
        events: Vec::new(),
        status_line: StatusLineIntent::Unwrap,
        remove_only: true,
    }
}

/// Removals for everything this app may have written, whatever the settings
/// say: every run folder given and every file the record names. For
/// "remove the hooks" with no hub running (`uninstall-hooks`: the record plus
/// discovery) and for `Job::Uninstall`.
pub fn removal_plans(record: &HookInstallRecord, folders: &[RunFolder]) -> Vec<InstallPlan> {
    let run_folders = folders
        .iter()
        .filter(|folder| folder.kind == FolderKind::Run || record.entry_for(&folder.id).is_some());
    let mut plans: Vec<InstallPlan> = run_folders.map(removal_plan).collect();
    let unlisted: Vec<InstallPlan> = record
        .files
        .iter()
        .filter(|entry| !plans.iter().any(|plan| plan.folder == entry.folder))
        .map(|entry| removal_plan_for_entry(entry, None))
        .collect();
    plans.extend(unlisted);
    plans
}

/// Carries `removal_plans` out, each physical settings.json once, and says
/// what is left of the record (empty when everything came out) beside the
/// outcomes.
pub fn uninstall_all(
    record: &HookInstallRecord,
    folders: &[RunFolder],
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> (Vec<InstallOutcome>, HookInstallRecord) {
    let plans = removal_plans(record, folders);
    let outcomes = apply_installs(&plans, files, clock);
    let left = updated_record(record, &plans, &outcomes);
    (outcomes, left)
}

// ---- Outcomes ----

fn note_outcome(known: &mut Known, plan: &InstallPlan, outcome: &InstallOutcome) {
    let status = &mut known.status;
    match &outcome.result {
        Ok(change) => {
            status.last_outcome = Some(change.clone());
            status.last_error = match change {
                InstallChange::Aborted(why) => Some(why.clone()),
                _ => None,
            };
            status.not_hookable = None;
            if matches!(change, InstallChange::Aborted(_)) {
                return;
            }
            if outcome.backup.is_some() {
                status.newest_backup = outcome.backup.clone();
            }
            status.config_dir_exists = true;
            status.settings_readable = true;
            if plan.remove_only {
                known.ours = Some(false);
                status.hooks_registered = false;
                status.hooks_installed = false;
                status.status_line_installed = false;
                status.status_line_left_alone = None;
                status.form = None;
                status.hook_command = None;
                return;
            }
            known.ours = Some(true);
            status.hooks_registered = true;
            status.hooks_installed = true;
            // No entry: the file is shared with a folder whose plan wrote
            // it, and what it holds is that folder's to say.
            let Some(entry) = &outcome.entry else {
                return;
            };
            status.status_line_installed = entry.status_line;
            status.status_line_left_alone = match &outcome.status_line {
                Some(StatusLineIntent::LeaveAlone(reason)) => Some(reason.clone()),
                _ => None,
            };
            status.form = Some(entry.form.clone());
            status.hook_command = Some(match &plan.form {
                form @ CommandForm::Exec { .. } => {
                    display_command(form).unwrap_or_else(|| entry.command.clone())
                }
                _ => entry.command.clone(),
            });
        }
        Err(error) => {
            status.last_outcome = None;
            // A path no command can carry is how the folder is, not a
            // failure of this pass.
            let not_hookable = !plan.remove_only
                && matches!(&plan.form, CommandForm::NotPossible(reason) if reason == error);
            if not_hookable {
                status.not_hookable = Some(error.clone());
                status.last_error = None;
                // The pass took ours out of settings.json (`withdraw_hooks`);
                // a backup says it had to write to do so.
                if outcome.backup.is_some() {
                    status.newest_backup = outcome.backup.clone();
                }
                status.hooks_registered = false;
                status.hooks_installed = false;
                status.form = None;
                status.hook_command = None;
            } else {
                status.not_hookable = None;
                status.last_error = Some(error.clone());
            }
        }
    }
}

/// `record` after a pass: every install that left ours in a file is noted
/// under that file, and every file a removal cleaned is dropped, unless
/// another folder's install holds ours in the same file.
fn updated_record(
    record: &HookInstallRecord,
    plans: &[InstallPlan],
    outcomes: &[InstallOutcome],
) -> HookInstallRecord {
    let mut updated = record.clone();
    let installed: Vec<&HookInstallEntry> = outcomes
        .iter()
        .filter(|outcome| outcome.result.is_ok())
        .filter_map(|outcome| outcome.entry.as_ref())
        .collect();
    for outcome in outcomes {
        let removal = plans
            .iter()
            .find(|plan| plan.folder == outcome.folder)
            .is_some_and(|plan| plan.remove_only);
        let cleaned = match &outcome.result {
            Ok(InstallChange::Removed | InstallChange::Unchanged) => true,
            // No folder, nothing of ours in it.
            Err(error) => error == CONFIG_DIR_MISSING,
            Ok(_) => false,
        };
        if !removal || !cleaned {
            continue;
        }
        updated.remove_where(|entry| {
            (entry.settings_path == outcome.settings_path || entry.folder == outcome.folder)
                && !installed
                    .iter()
                    .any(|kept| kept.settings_path == entry.settings_path)
        });
    }
    for entry in installed {
        updated.upsert(entry.clone());
    }
    updated
}

/// The folders whose settings.json a pass changed, for the notice saying
/// what was changed and where the backups are. A folder no command can name
/// reports its reason, not a change, even when ours had to come out of its
/// file; its backup says that happened.
pub fn changed_folders(outcomes: &[InstallOutcome]) -> Vec<AccountId> {
    outcomes
        .iter()
        .filter(|outcome| match &outcome.result {
            Ok(change) => matches!(change, InstallChange::Written | InstallChange::Removed),
            Err(_) => outcome.backup.is_some(),
        })
        .map(|outcome| outcome.folder.clone())
        .collect()
}

// ---- The consent card ----

/// The settings.json files "Turn on" will edit, in the folders' order: the
/// tracked run folders', with whose each is.
pub fn consent_files(
    accounts: &[Account],
    folders: &[RunFolder],
    paths: &Paths,
) -> Vec<ConsentFile> {
    folders
        .iter()
        .filter(|folder| is_install_target(accounts, folder))
        .map(|folder| {
            let file = paths.join(&folder.config_dir.to_string_lossy(), SETTINGS_FILE_NAME);
            let home = match paths.style() {
                PathStyle::Windows => "%USERPROFILE%",
                PathStyle::Posix => "~",
            };
            let path = match paths.strip_prefix(paths.home(), &file) {
                Some(rest) => format!("{home}{}{rest}", paths.style().separator()),
                None => file,
            };
            let owner = accounts
                .iter()
                .filter(|account| account.run_dirs.contains(&folder.id))
                .find(|account| account.is_tracked);
            let account = match owner {
                Some(account) => account
                    .email
                    .clone()
                    .or_else(|| Some(account.label.clone())),
                None => folder.identity.as_ref().and_then(|identity| {
                    identity
                        .email
                        .clone()
                        .or_else(|| identity.display_name.clone())
                }),
            };
            ConsentFile {
                path,
                account: account.filter(|name| !name.is_empty()),
            }
        })
        .collect()
}

/// One line of the consent card:
/// `%USERPROFILE%\.claude\settings.json (me@work.com)`.
pub fn consent_line(file: &ConsentFile) -> String {
    match &file.account {
        Some(account) => format!("{} ({account})", file.path),
        None => file.path.clone(),
    }
}
