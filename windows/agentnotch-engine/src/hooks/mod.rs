//! Hook installation (HS§3, DESIGN-WIN §4.3): consent, targets, the hook
//! copy, command forms, recognisers, settings.json safety, status line
//! takeover, passes, uninstall.
//!
//! Owner: WP2. `manager` decides which folders get the hooks and when;
//! `apply` carries its plans out on disk.

pub mod apply;
pub mod backups;
pub mod commands;
pub mod copy;
pub mod events;
pub mod facts;
pub mod manager;
pub mod plan;
pub mod shell_words;
pub mod version;

pub use apply::{
    apply_install, apply_installs, read_status, remove_codenotch_hooks, uninstall_everything,
};
pub use manager::{
    changed_folders, consent_files, consent_line, is_install_target, removal_plans, uninstall_all,
    HookManager,
};
