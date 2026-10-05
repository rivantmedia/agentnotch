//! Hooks, wired into the hub (design §4.3, HS§3): the manager made at load
//! with `hook-install.json`'s record, passes when the manager says one is
//! due (at start, a second after the accounts change, every ten minutes, on
//! a lower Claude Code version, on the user's switches), what each folder's
//! settings.json holds read back (also before consent: the setup card shows
//! the official app's folders), and the Settings calls.
//!
//! Every write to a settings.json or a hooks folder is an install, removal
//! or official-app removal job, and all of them go through one queue here,
//! one at a time (two writers of one settings.json would undo each other):
//! - nothing is planned before "Turn on" (`HookManager::plan`);
//! - nothing at all is written while installs are disabled for the run
//!   (`--no-install`; a sealed run never builds this hub): the manager's
//!   write functions don't check, so the queue does;
//! - `claude --version` runs only after consent (before it, the only child
//!   the app starts is the usage probe).
//!
//! `uninstall-hooks` (no hub running) is [`uninstall_everywhere`].
//!
//! Owner: WP7.

use super::api::{CallError, RemovedReply, VersionReply};
use super::core_state::{to_value, Core, Reply};
use super::jobs::is_failed_folder_read;
use crate::accounts::{read_folder_snapshot, AccountRegistry};
use crate::core::flags::DevFlags;
use crate::core::settings::ControlSettings;
use crate::hooks::apply::{CONFIG_DIR_MISSING, SETTINGS_FILE_NAME};
use crate::hooks::facts::ClaudeCodeFacts;
use crate::hooks::manager::{changed_folders, removal_plans, uninstall_all};
use crate::model::{Account, AccountId, FolderKind};
use crate::persist::hook_install::HookInstallFile;
use crate::platform::{Expect, Platform, Roots, WriteMode};
use crate::runtime_types::{
    FolderHookStatus, InstallChange, InstallOutcome, InstallPlan, Job, JobId, PersistFile,
    VersionSighting, VersionSource,
};
use crate::usage::{locator, versions};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A write asked for, waiting its turn.
enum Write {
    /// A pass: planned when it starts, from what is known then.
    Pass(Option<Reply>),
    /// Removals planned already (forgetting an account).
    Removals(Vec<InstallPlan>),
    /// Hooks off: ours out of every folder that may hold them.
    Uninstall,
    /// The official app's entries out of one settings.json.
    Codenotch {
        folder: AccountId,
        settings_path: PathBuf,
        reply: Option<Reply>,
    },
}

/// The write running.
enum Running {
    /// Install or removal plans, which its outcomes answer.
    Plans(Vec<InstallPlan>, Option<Reply>),
    Codenotch(Option<Reply>),
}

/// The hooks' queue and what it waits on.
pub(crate) struct HooksWiring {
    facts: ClaudeCodeFacts,
    queue: VecDeque<Write>,
    running: Option<(JobId, Running)>,
    /// The read-back in flight; another is due when `status_due`.
    status_job: Option<JobId>,
    pub(crate) status_due: bool,
    /// The version check in flight: passes wait for it, so hooks are
    /// written for the versions on this PC.
    versions_job: Option<JobId>,
}

impl Default for HooksWiring {
    fn default() -> Self {
        HooksWiring {
            facts: ClaudeCodeFacts::compiled_in(),
            queue: VecDeque::new(),
            running: None,
            status_job: None,
            status_due: false,
            versions_job: None,
        }
    }
}

impl HooksWiring {
    /// A write is running or waiting.
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }

    fn pass_queued(&self) -> bool {
        self.queue
            .iter()
            .any(|write| matches!(write, Write::Pass(_)))
    }

    /// Calls still waiting are answered (a stop). The writes themselves
    /// stay queued for the next start.
    pub(crate) fn answer_waiting(&mut self, why: &str) {
        let mut waiting: Vec<Reply> = Vec::new();
        for write in &mut self.queue {
            match write {
                Write::Pass(reply) | Write::Codenotch { reply, .. } => waiting.extend(reply.take()),
                Write::Removals(_) | Write::Uninstall => {}
            }
        }
        match &mut self.running {
            Some((_, Running::Plans(_, reply) | Running::Codenotch(reply))) => {
                waiting.extend(reply.take())
            }
            None => {}
        }
        // The writes themselves wait for the next start.
        for reply in waiting {
            let _ = reply.send(Err(CallError::failed(why)));
        }
    }
}

impl Core {
    pub(crate) fn hooks_on_start(&mut self, now: SystemTime) {
        // The first pass is due now (it waits for the version check).
        self.hooks.start(now);
        self.hooks_w.status_due = true;
        if self.settings.hook_consent == Some(true) && !self.hooks.installs_disabled() {
            self.check_versions(None);
        }
    }

    /// The hooks' work that became due: the read-back, a pass the manager
    /// asks for (while the hub runs), the next write in the queue.
    pub(crate) fn drive_hooks(&mut self, now: SystemTime) {
        if self.hooks_w.status_due
            && self.hooks_w.status_job.is_none()
            && self.registry.has_classified()
        {
            self.hooks_w.status_due = false;
            let folders = self.folder_dirs();
            let id = self.schedule(Job::ReadHookStatus { folders }, None);
            self.hooks_w.status_job = Some(id);
        }
        if self.live
            && self.hooks.pass_due(now)
            && self.registry.has_classified()
            && !self.hooks_w.pass_queued()
        {
            self.hooks_w.queue.push_back(Write::Pass(None));
        }
        self.start_hook_writes(now);
    }

    pub(crate) fn hooks_deadline(&self) -> Option<SystemTime> {
        // A pass that is due but waits on a job is started by that job's
        // result, not by the clock.
        let waiting = self.hooks_w.busy()
            || self.hooks_w.versions_job.is_some()
            || !self.registry.has_classified();
        if waiting {
            None
        } else {
            self.hooks.next_pass_at()
        }
    }

    /// Starts queued writes until one runs.
    fn start_hook_writes(&mut self, now: SystemTime) {
        while self.hooks_w.running.is_none() {
            if matches!(self.hooks_w.queue.front(), Some(Write::Pass(_)))
                && self.hooks_w.versions_job.is_some()
            {
                return;
            }
            let Some(write) = self.hooks_w.queue.pop_front() else {
                return;
            };
            let disabled = self.hooks.installs_disabled();
            match write {
                Write::Pass(reply) => {
                    let plans = self.hooks.plan(
                        &hook_accounts(&self.registry),
                        &self.registry.folders(),
                        &self.settings,
                        &self.versions,
                        &self.hooks_w.facts,
                    );
                    self.hooks.pass_started(
                        &self.settings,
                        &self.versions,
                        &self.hooks_w.facts,
                        now,
                    );
                    self.start_plans(plans, None, reply);
                }
                Write::Removals(plans) => self.start_plans(plans, None, None),
                Write::Uninstall => {
                    if disabled {
                        continue;
                    }
                    let record = self.hooks.record().clone();
                    let folders = self.registry.folders();
                    let plans = removal_plans(&record, &folders);
                    self.start_plans(plans, Some(Job::Uninstall { record, folders }), None);
                }
                Write::Codenotch {
                    folder,
                    settings_path,
                    reply,
                } => {
                    if disabled {
                        if let Some(reply) = reply {
                            let _ = reply.send(Err(CallError::refused(INSTALLS_OFF)));
                        }
                        continue;
                    }
                    let id = self.schedule(
                        Job::RemoveCodenotchHooks {
                            folder,
                            settings_path,
                        },
                        None,
                    );
                    self.hooks_w.running = Some((id, Running::Codenotch(reply)));
                }
            }
        }
    }

    /// Runs `plans` (as `job` when given, else an install job), unless there
    /// is nothing to do or writing is off for the run.
    fn start_plans(&mut self, plans: Vec<InstallPlan>, job: Option<Job>, reply: Option<Reply>) {
        if plans.is_empty() || self.hooks.installs_disabled() {
            if let Some(reply) = reply {
                let _ = reply.send(Ok(json!({})));
            }
            return;
        }
        let job = job.unwrap_or_else(|| Job::Install {
            plans: plans.clone(),
        });
        let id = self.schedule(job, None);
        self.hooks_w.running = Some((id, Running::Plans(plans, reply)));
    }

    /// Removals the hub planned itself (forgetting an account), in turn.
    pub(crate) fn queue_removals(&mut self, plans: Vec<InstallPlan>) {
        if !plans.is_empty() && !self.hooks.installs_disabled() {
            self.hooks_w.queue.push_back(Write::Removals(plans));
        }
    }

    /// An install or removal came back: what each folder holds now, the
    /// record (saved when it changed), the folders whose settings.json
    /// changed (the notice), and the read-back.
    pub(crate) fn installed(
        &mut self,
        id: JobId,
        outcomes: Vec<InstallOutcome>,
        reply: &mut Option<Reply>,
    ) {
        let (plans, waiting) = match self.hooks_w.running.take() {
            Some((job, Running::Plans(plans, waiting))) if job == id => (plans, waiting),
            other => {
                self.hooks_w.running = other;
                return;
            }
        };
        if self.hooks.absorb_outcomes(&plans, &outcomes) {
            let bytes = HookInstallFile::from_model(self.hooks.record()).encode();
            self.persist(PersistFile::HookInstall, bytes);
        }
        // Folders the registry no longer lists aren't kept.
        self.hooks.retain_folders(&self.registry.folders());
        let changed = changed_folders(&outcomes);
        if !changed.is_empty() {
            self.changed_folders = changed;
        }
        self.hooks_w.status_due = true;
        let failures = failures(&outcomes);
        for failure in &failures {
            self.log(format!("hooks: {failure}"));
        }
        let answer = if failures.is_empty() {
            Ok(json!({}))
        } else {
            Err(CallError::failed(failures.join("\n")))
        };
        if let Some(waiting) = waiting.or_else(|| reply.take()) {
            let _ = waiting.send(answer);
        }
    }

    /// The folders' settings.json read back.
    pub(crate) fn hook_statuses_read(
        &mut self,
        id: JobId,
        statuses: Vec<(AccountId, FolderHookStatus)>,
    ) {
        if self.hooks_w.status_job == Some(id) {
            self.hooks_w.status_job = None;
        }
        for (folder, status) in statuses {
            // A folder forgotten while it was read isn't brought back.
            if self.registry.folder(folder.as_str()).is_some() {
                self.hooks.absorb_status(&folder, status, &self.settings);
            }
        }
    }

    /// The official app's entries came out (or didn't).
    pub(crate) fn codenotch_removed(
        &mut self,
        id: JobId,
        folder: &AccountId,
        result: Result<u32, String>,
        reply: &mut Option<Reply>,
    ) {
        let waiting = match self.hooks_w.running.take() {
            Some((job, Running::Codenotch(waiting))) if job == id => waiting,
            other => {
                self.hooks_w.running = other;
                None
            }
        };
        self.hooks_w.status_due = true;
        let answer = match result {
            Ok(removed) => {
                if removed > 0 {
                    self.changed_folders = vec![folder.clone()];
                }
                to_value(&RemovedReply { removed })
            }
            Err(why) => Err(CallError::failed(why)),
        };
        if let Some(waiting) = waiting.or_else(|| reply.take()) {
            let _ = waiting.send(answer);
        }
    }

    // ---- versions ----

    /// Asks every installed Claude Code its version and lists the bundled
    /// copies (after consent only). `reply` gets the version of the copy
    /// Settings chose (or of the one found first).
    fn check_versions(&mut self, reply: Option<Reply>) {
        let roots = &self.cfg.roots;
        let choice = self.settings.claude_binary_path.as_deref().map(Path::new);
        let env_path = std::env::var_os("PATH").unwrap_or_default();
        let binaries = locator::installed(roots, choice, &env_path, &|path| path.is_file());
        let bundled = versions::bundled_roots(roots);
        let id = self.schedule(Job::Versions { binaries, bundled }, reply);
        self.hooks_w.versions_job = Some(id);
    }

    /// The versions came back: the binaries' and bundled copies' replace
    /// the last ones (sessions' stay); hooks written for a newer version
    /// than the lowest now known are rewritten.
    pub(crate) fn versions_read(
        &mut self,
        id: JobId,
        sightings: Vec<VersionSighting>,
        now: SystemTime,
        reply: &mut Option<Reply>,
    ) {
        if self.hooks_w.versions_job == Some(id) {
            self.hooks_w.versions_job = None;
        }
        if let Some(reply) = reply.take() {
            let chosen = self
                .settings
                .claude_binary_path
                .as_deref()
                .map(PathBuf::from);
            let version = sightings
                .iter()
                .filter(|s| s.source == VersionSource::Binary)
                .find(|s| chosen.is_none() || s.path == chosen)
                .and_then(|s| s.version.clone());
            let _ = reply.send(to_value(&VersionReply { version }));
        }
        self.versions.retain(|seen| {
            matches!(
                seen.source,
                VersionSource::Registry | VersionSource::StatusLine
            )
        });
        self.versions.extend(sightings);
        self.hooks
            .note_versions(&self.versions, &self.hooks_w.facts, now);
    }

    /// A Claude Code version a session reported (its registry entry or
    /// status line). A lower one rewrites the hooks.
    #[allow(dead_code)] // The ingress's sightings come in with wp7-8.
    pub(crate) fn note_version_sighting(&mut self, sighting: VersionSighting, now: SystemTime) {
        if sighting.version.is_none() || self.versions.contains(&sighting) {
            return;
        }
        self.versions.push(sighting);
        self.hooks
            .note_versions(&self.versions, &self.hooks_w.facts, now);
    }

    // ---- calls ----

    /// "Turn on" (and the Hooks switch on, which is also a yes): consent
    /// for the current scope, the versions checked, a pass.
    fn turn_hooks_on(&mut self) {
        let mut next = self.settings.clone();
        next.hook_consent = Some(true);
        next.hooks_enabled = true;
        next.hook_consent_scope = ControlSettings::CURRENT_CONSENT_SCOPE;
        self.replace_settings(next);
        if self.hooks.installs_disabled() {
            return;
        }
        self.check_versions(None);
        self.queue_pass(None);
    }

    fn queue_pass(&mut self, reply: Option<Reply>) {
        if reply.is_none() && self.hooks_w.pass_queued() {
            return;
        }
        self.hooks_w.queue.push_back(Write::Pass(reply));
    }

    /// `hook_consent {grant}`: "Turn on" or "Not now" (remembered; nothing
    /// is written).
    pub(crate) fn hook_consent_call(&mut self, grant: bool) -> Result<Value, CallError> {
        if grant {
            self.turn_hooks_on();
        } else {
            let mut next = self.settings.clone();
            next.hook_consent = Some(false);
            self.replace_settings(next);
        }
        Ok(json!({}))
    }

    /// `hooks_enabled {on}`: off takes ours out of every folder that may
    /// hold them and restores the status lines.
    pub(crate) fn hooks_enabled_call(&mut self, on: bool) -> Result<Value, CallError> {
        if on {
            self.turn_hooks_on();
            return Ok(json!({}));
        }
        let mut next = self.settings.clone();
        next.hooks_enabled = false;
        self.replace_settings(next);
        // Before the yes nothing of ours was written, unless a record says so.
        let wrote =
            self.settings.hook_consent == Some(true) || !self.hooks.record().files.is_empty();
        if wrote && !self.hooks.installs_disabled() {
            self.hooks_w
                .queue
                .retain(|write| !matches!(write, Write::Uninstall));
            self.hooks_w.queue.push_back(Write::Uninstall);
        }
        Ok(json!({}))
    }

    /// `status_line_enabled {on}`: wrap each folder's status line, or put
    /// it back exactly.
    pub(crate) fn status_line_call(&mut self, on: bool) -> Result<Value, CallError> {
        let mut next = self.settings.clone();
        next.status_line_integration = on;
        self.replace_settings(next);
        self.queue_pass(None);
        Ok(json!({}))
    }

    /// `acknowledge_scope`: [OK] on the notice that "Turn on" covers more
    /// folders than the yes an earlier build was given.
    pub(crate) fn acknowledge_scope_call(&mut self) -> Result<Value, CallError> {
        let mut next = self.settings.clone();
        next.hook_consent_scope = ControlSettings::CURRENT_CONSENT_SCOPE;
        self.replace_settings(next);
        Ok(json!({}))
    }

    /// `hooks_reinstall {account_id?}`: a pass over every folder (it writes
    /// only where something differs), answered when it is done. Works on a
    /// hub that never started (`agentnotch.exe install-hooks`).
    pub(crate) fn hooks_reinstall_call(&mut self, account_id: Option<&str>, reply: Reply) {
        if self.hooks.installs_disabled() {
            let _ = reply.send(Err(CallError::refused(INSTALLS_OFF)));
            return;
        }
        if self.settings.hook_consent != Some(true) {
            let _ = reply.send(Err(CallError::refused(
                "Turn on Claude Code control in Settings › Claude Code first.",
            )));
            return;
        }
        let now = self.platform.clock.now();
        self.ensure_discovered(now);
        if account_id.is_some_and(|id| self.registry.folders_for(id).is_empty()) {
            let _ = reply.send(Err(CallError::not_found("That account isn't known.")));
            return;
        }
        if self.hooks_w.versions_job.is_none() {
            self.check_versions(None);
        }
        self.queue_pass(Some(reply));
    }

    /// `remove_codenotch_hooks {folder}`: the official app's entries out of
    /// a folder the engine knows, on the user's click.
    pub(crate) fn remove_codenotch_call(&mut self, folder: &str, reply: Reply) {
        if self.hooks.installs_disabled() {
            let _ = reply.send(Err(CallError::refused(INSTALLS_OFF)));
            return;
        }
        let Some(known) = self.registry.folder(folder) else {
            let _ = reply.send(Err(CallError::not_found("That folder isn't known.")));
            return;
        };
        let folder = known.id.clone();
        let settings_path = PathBuf::from(known.dir()).join(SETTINGS_FILE_NAME);
        self.hooks_w.queue.push_back(Write::Codenotch {
            folder,
            settings_path,
            reply: Some(reply),
        });
    }

    /// `choose_claude_binary {path?}`: the copy probes run (`None`: find
    /// it), answered with its version (after consent; before it no
    /// `claude --version` runs). The hooks follow its version.
    pub(crate) fn choose_claude_binary_call(&mut self, path: Option<&str>, reply: Reply) {
        let path = path.map(str::trim).filter(|path| !path.is_empty());
        if let Some(path) = path {
            let file = Path::new(path);
            if !file.is_absolute() || !file.is_file() {
                let _ = reply.send(Err(CallError::invalid("That file isn't there.")));
                return;
            }
            if locator::is_desktop_owned(file) {
                let _ = reply.send(Err(CallError::invalid(
                    "That copy belongs to Claude Desktop. Choose claude.exe from your own install.",
                )));
                return;
            }
        }
        let mut next = self.settings.clone();
        next.claude_binary_path = path.map(str::to_owned);
        self.replace_settings(next);
        if self.settings.hook_consent != Some(true) {
            let _ = reply.send(to_value(&VersionReply { version: None }));
            return;
        }
        self.check_versions(Some(reply));
        self.queue_pass(None);
    }
}

/// The accounts as the install targets see them. While Claude Parallel
/// Profiles mirrors accounts into `~\.claude`, that folder is no one
/// account's: whoever the focused VS Code window runs as is copied in, so
/// following one account's switch would add and remove our hooks on every
/// focus change. It counts as every signed-in account's run folder then,
/// and keeps our hooks while any of them is tracked (the Mac's
/// `AccountHookManager.isTracked`, PP-C5).
pub(crate) fn hook_accounts(registry: &AccountRegistry) -> Vec<Account> {
    let mut accounts = registry.accounts();
    if !registry.mirrors_default() {
        return accounts;
    }
    let paths = registry.paths();
    let default = registry.folders().into_iter().find(|folder| {
        folder.kind == FolderKind::Run
            && paths.is_default_config_dir(&folder.config_dir.to_string_lossy())
    });
    if let Some(default) = default {
        for account in accounts.iter_mut().filter(|account| account.is_signed_in) {
            if !account.run_dirs.contains(&default.id) {
                account.run_dirs.push(default.id.clone());
            }
        }
    }
    accounts
}

/// Refused writes while the run may not write (`--no-install`).
const INSTALLS_OFF: &str = "Installing and removing hooks is off for this run (--no-install).";

/// The outcomes that failed, as lines (a folder that isn't there has
/// nothing of ours to take out).
fn failures(outcomes: &[InstallOutcome]) -> Vec<String> {
    outcomes
        .iter()
        .filter_map(|outcome| match &outcome.result {
            Err(why) if why != CONFIG_DIR_MISSING => {
                Some(format!("{}: {why}", outcome.settings_path.display()))
            }
            _ => None,
        })
        .collect()
}

/// `agentnotch.exe uninstall-hooks`, with no hub running: this app's hooks
/// out of every folder `hook-install.json` names and every run folder a
/// discovery finds, status lines restored, the hook copies deleted. Never
/// while installs are off for the run.
pub(crate) fn uninstall_everywhere(
    roots: &Roots,
    platform: &Platform,
    flags: &DevFlags,
) -> Result<String, String> {
    if flags.sealed {
        return Ok("Sealed: nothing was removed (a sealed run touches no Claude folder).".into());
    }
    if !flags.installs_allowed() {
        return Err(format!("{INSTALLS_OFF} Nothing was removed."));
    }
    let files = platform.files.as_ref();
    let clock = platform.clock.as_ref();
    let record_path = roots.support_file(PersistFile::HookInstall.file_name());
    let record = std::fs::read(&record_path)
        .ok()
        .and_then(|bytes| HookInstallFile::parse(&bytes))
        .map(|file| file.to_model())
        .unwrap_or_default();
    let mut registry =
        AccountRegistry::new(roots.paths()).with_extra_config_dirs(&flags.extra_config_dirs);
    if let Ok(bytes) = std::fs::read(roots.support_file(PersistFile::Accounts.file_name())) {
        registry.load(&bytes);
    }
    let snapshot = read_folder_snapshot(
        roots,
        &registry.explicit_dirs(),
        files,
        platform.processes.as_ref(),
    );
    if !is_failed_folder_read(&snapshot) {
        registry.discover(snapshot, clock.now());
    }
    let (outcomes, left) = uninstall_all(&record, &registry.folders(), files, clock);
    let mut lines = Vec::new();
    if left != record {
        let bytes = HookInstallFile::from_model(&left).encode();
        let saved = files.ensure_private_dir(&roots.support).and_then(|()| {
            files.write_atomic(&record_path, &bytes, WriteMode::Private, Expect::Nothing)
        });
        if let Err(e) = saved {
            lines.push(format!("{}: {e}", record_path.display()));
        }
    }
    let removed = outcomes
        .iter()
        .filter(|outcome| matches!(outcome.result, Ok(InstallChange::Removed)))
        .count();
    let failed = failures(&outcomes);
    let summary = match removed {
        0 => "No Agent Notch hooks were found.".to_owned(),
        1 => "Removed Agent Notch's hooks from 1 folder.".to_owned(),
        n => format!("Removed Agent Notch's hooks from {n} folders."),
    };
    lines.extend(failed);
    if lines.is_empty() {
        Ok(summary)
    } else {
        Err(format!("{summary}\n{}", lines.join("\n")))
    }
}
