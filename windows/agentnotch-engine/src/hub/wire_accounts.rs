//! The accounts, wired into the hub (design §4.2, AU§3.1): the registry
//! made at load from `accounts.json`, a discovery read synchronously before
//! the first rings and the first hook pass (so neither ever mistakes a store
//! for a run folder), then one every five minutes and soon after a session
//! names a folder no read covered; `.claude.json` reads come with the usage
//! cycle (`wire_usage`). What the user asks of an account in Settings, the
//! launch command and the folders Settings reveals are answered here.
//!
//! Owner: WP7.

use super::api::{CallError, CommandReply, CreatedReply, ErrorReply, PathReply, RevealKind};
use super::core_state::{to_value, Core};
use super::jobs::is_failed_folder_read;
use super::wire_hooks::hook_accounts;
use crate::accounts::{read_folder_snapshot, DISCOVERY_INTERVAL, SAVE_DELAY};
use crate::hooks::manager::is_install_target;
use crate::model::{AccountAction, AccountId, FolderKind, FolderSnapshot, RunFolder};
use crate::runtime_types::{AccountsChanged, Job, JobId, PersistFile};
use serde_json::{json, Value};
use std::time::SystemTime;

/// The accounts' schedule.
#[derive(Debug, Default)]
pub(crate) struct AccountsWiring {
    /// When the home folder is read again.
    next_discovery_at: Option<SystemTime>,
    /// The read of the home folder in flight.
    discovery_job: Option<JobId>,
    /// When `accounts.json` is written (changes come in bursts).
    save_at: Option<SystemTime>,
}

impl Core {
    /// The launch discovery, on the caller's thread, once: the rings at
    /// launch and the first hook pass need the folders classified. Reads
    /// only; what is worth saving is saved by the running hub.
    pub(crate) fn ensure_discovered(&mut self, now: SystemTime) {
        if self.registry.has_classified() {
            return;
        }
        let explicit = self.registry.explicit_dirs();
        let snapshot = read_folder_snapshot(
            &self.cfg.roots,
            &explicit,
            self.platform.files.as_ref(),
            self.platform.processes.as_ref(),
        );
        self.discovered(snapshot, now);
    }

    pub(crate) fn accounts_on_start(&mut self, now: SystemTime) {
        self.ensure_discovered(now);
        self.accounts_w.next_discovery_at = Some(now + DISCOVERY_INTERVAL);
    }

    /// The accounts' work that became due.
    pub(crate) fn drive_accounts(&mut self, now: SystemTime) {
        let due = self.accounts_w.next_discovery_at.is_none_or(|at| at <= now)
            || self.registry.needs_discovery();
        if due && self.accounts_w.discovery_job.is_none() {
            let explicit = self.registry.explicit_dirs();
            let id = self.schedule(Job::ReadFolders { explicit }, None);
            self.accounts_w.discovery_job = Some(id);
        }
        if self.registry.needs_save() && self.accounts_w.save_at.is_none() {
            self.accounts_w.save_at = Some(now + SAVE_DELAY);
        }
        if self.accounts_w.save_at.is_some_and(|at| at <= now) {
            self.accounts_w.save_at = None;
            if let Some(bytes) = self.registry.file_bytes() {
                self.persist(PersistFile::Accounts, bytes);
            }
            self.registry.mark_saved();
        }
    }

    pub(crate) fn accounts_deadline(&self) -> Option<SystemTime> {
        let discovery = self
            .accounts_w
            .discovery_job
            .is_none()
            .then_some(self.accounts_w.next_discovery_at)
            .flatten();
        [discovery, self.accounts_w.save_at]
            .into_iter()
            .flatten()
            .min()
    }

    /// A read of the home folder came back.
    pub(crate) fn folders_read(&mut self, id: JobId, snapshot: FolderSnapshot, now: SystemTime) {
        if self.accounts_w.discovery_job == Some(id) {
            self.accounts_w.discovery_job = None;
            self.accounts_w.next_discovery_at = Some(now + DISCOVERY_INTERVAL);
        }
        self.discovered(snapshot, now);
    }

    fn discovered(&mut self, snapshot: FolderSnapshot, now: SystemTime) {
        if is_failed_folder_read(&snapshot) {
            // Nothing learnt: the folders known stay as they are.
            self.log("the Claude folders couldn't be read; trying again later".into());
            return;
        }
        let changed = self.registry.discover(snapshot, now);
        self.accounts_changed(&changed, now);
    }

    /// The registry changed (or may have): every store that reads it is
    /// told. Called after each discovery, `.claude.json` read and account
    /// action.
    pub(crate) fn accounts_changed(&mut self, changed: &AccountsChanged, now: SystemTime) {
        if !changed.any() && self.usage_w.restored {
            return;
        }
        self.usage_accounts_changed(now);
        // A pass a second after the last change (when passes run at all).
        self.hooks.accounts_changed(now);
        self.hooks.retain_folders(&self.registry.folders());
        self.hooks_w.status_due = true;
        self.sessions_accounts_changed(Some(changed), now);
    }

    // ---- calls ----

    /// `account {action}`: `{}`, `{error}` when refused, `{created, command}`
    /// for a new account.
    pub(crate) fn account_call(&mut self, action: AccountAction) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        let paths = self.registry.paths().clone();
        let refused = |error: String| to_value(&ErrorReply { error });
        match action {
            AccountAction::Create { name } => match self.registry.create_account(&name) {
                Ok(created) => {
                    self.accounts_changed(&created.changed, now);
                    self.hooks_w.status_due = true;
                    to_value(&CreatedReply {
                        created: created.folder.0,
                        command: created.launch_command,
                    })
                }
                Err(error) => refused(error.message(&paths)),
            },
            AccountAction::Forget { id } => {
                // Ours comes out first, while the folders are known (the
                // registry keeps a forgotten identity's folders, untracked).
                let folders: Vec<RunFolder> = self
                    .registry
                    .folders_for(&id)
                    .into_iter()
                    .filter(|folder| folder.kind == FolderKind::Run)
                    .map(|folder| folder.to_run_folder())
                    .collect();
                match self.registry.apply_user(AccountAction::Forget { id }) {
                    Ok(changed) => {
                        // A mirrored `~\.claude` keeps ours while another
                        // account is tracked.
                        let accounts = hook_accounts(&self.registry);
                        let folders: Vec<RunFolder> = folders
                            .into_iter()
                            .filter(|folder| {
                                let default = paths
                                    .is_default_config_dir(&folder.config_dir.to_string_lossy());
                                !(default && is_install_target(&accounts, folder))
                            })
                            .collect();
                        let plans = self.hooks.forget_plans(&folders, &self.settings);
                        self.queue_removals(plans);
                        self.accounts_changed(&changed, now);
                        Ok(json!({}))
                    }
                    Err(error) => refused(error),
                }
            }
            action => {
                // Tracking is the user's click: the hooks follow at once.
                let tracking = matches!(action, AccountAction::Track { .. });
                match self.registry.apply_user(action) {
                    Ok(changed) => {
                        self.accounts_changed(&changed, now);
                        if tracking {
                            self.hooks.request_pass(now);
                        }
                        Ok(json!({}))
                    }
                    Err(error) => refused(error),
                }
            }
        }
    }

    /// `launch_command {account_id}`: the PowerShell line that starts Claude
    /// Code as that account.
    pub(crate) fn launch_command_call(&self, account_id: &str) -> Result<Value, CallError> {
        match self.registry.launch_command(account_id) {
            Some(command) => to_value(&CommandReply { command }),
            None => Err(CallError::not_found(
                "That account can't be started from a terminal.",
            )),
        }
    }

    /// `reveal_target`: a folder the engine knows, never a path a page
    /// supplies. `config_dir` takes a folder or an account; `backup` a
    /// folder (or an account's main folder), whose newest settings.json
    /// backup it names.
    pub(crate) fn reveal_target_call(
        &self,
        kind: RevealKind,
        id: &str,
    ) -> Result<Value, CallError> {
        let folder = self
            .registry
            .folder(id)
            .map(|folder| folder.id.clone())
            .or_else(|| self.registry.primary_dir(id));
        let Some(folder) = folder else {
            return Err(CallError::not_found("That account isn't known."));
        };
        let path = match kind {
            RevealKind::ConfigDir => self
                .registry
                .folder(folder.as_str())
                .map(|known| known.dir().to_owned()),
            RevealKind::Backup => self
                .hooks
                .folder_status(&folder)
                .newest_backup
                .map(|path| path.to_string_lossy().into_owned()),
            RevealKind::SessionCwd => None,
        };
        match path {
            Some(path) => to_value(&PathReply { path }),
            None => Err(CallError::not_found(match kind {
                RevealKind::Backup => "No backup of that settings.json yet.",
                _ => "That folder isn't known.",
            })),
        }
    }

    /// The folder ids the registry lists now, with their config folders
    /// (what the hook status read looks at).
    pub(crate) fn folder_dirs(&self) -> Vec<(AccountId, std::path::PathBuf)> {
        self.registry
            .folders()
            .into_iter()
            .map(|folder| (folder.id, folder.config_dir))
            .collect()
    }
}
