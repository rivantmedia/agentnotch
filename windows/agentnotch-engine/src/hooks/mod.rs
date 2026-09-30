//! Hook installation (HS§3, DESIGN-WIN §4.3): consent, targets, the hook
//! copy, command forms, recognisers, settings.json safety, status line
//! takeover, passes, uninstall.
//!
//! Owner: WP2. `apply` carries plans out on disk; `HookManager` is still
//! WP0's stub (it plans nothing).

pub mod apply;
pub mod backups;
pub mod commands;
pub mod copy;
pub mod events;
pub mod facts;
pub mod plan;
pub mod shell_words;
pub mod version;

use crate::core::settings::ControlSettings;
use crate::model::{Account, AccountId, RunFolder};
use crate::runtime_types::{FolderHookStatus, InstallPlan, VersionSighting};
use facts::ClaudeCodeFacts;

pub use apply::{
    apply_install, apply_installs, read_status, remove_codenotch_hooks, uninstall_everything,
};

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
