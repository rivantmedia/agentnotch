//! Hook installation (HS§3, DESIGN-WIN §4.3): consent, targets, the hook
//! copy, command forms, recognisers, settings.json safety, status line
//! takeover, passes, uninstall.
//!
//! Owner: WP2. WP0 stub: the §3.4 signatures; it never writes anything.

pub mod facts;

use crate::core::settings::ControlSettings;
use crate::model::{Account, AccountId, RunFolder};
use crate::persist::hook_install::HookInstallRecord;
use crate::platform::{Clock, SecureFiles};
use crate::runtime_types::{FolderHookStatus, InstallOutcome, InstallPlan, VersionSighting};
use facts::ClaudeCodeFacts;

#[derive(Default)]
pub struct HookManager {
    _statuses: Vec<(AccountId, FolderHookStatus)>,
}

impl HookManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plan(
        &self,
        accounts: &[Account],
        folders: &[RunFolder],
        settings: &ControlSettings,
        versions: &[VersionSighting],
        facts: &ClaudeCodeFacts,
    ) -> Vec<InstallPlan> {
        let _ = (accounts, folders, settings, versions, facts);
        Vec::new()
    }

    pub fn folder_status(&self, folder: &AccountId) -> FolderHookStatus {
        let _ = folder;
        FolderHookStatus::default()
    }
}

/// A Job on `an-io`.
pub fn apply_install(
    plan: &InstallPlan,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> InstallOutcome {
    let _ = (files, clock);
    InstallOutcome {
        folder: plan.folder.clone(),
        settings_path: plan.settings_path.clone(),
        result: Err("Installing hooks isn't in this build yet.".into()),
        backup: None,
    }
}

pub fn uninstall_everything(
    record: &HookInstallRecord,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> Vec<InstallOutcome> {
    let _ = (record, files, clock);
    Vec::new()
}
