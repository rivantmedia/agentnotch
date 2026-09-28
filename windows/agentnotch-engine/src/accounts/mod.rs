//! Accounts (AU§2-7): the folder classifier and discovery, identities
//! ("the email decides"), naming, monograms, ring ids, `accounts.json`,
//! the default folder's timeline, process attribution.
//!
//! Owner: WP3. WP0 stub: the §3.4 signatures; it knows no account.

use crate::model::{
    Account, AccountAction, AccountId, AccountSighting, Attribution, BackfillFolder,
    FolderSnapshot, IdentityId, RunFolder,
};
use crate::platform::{Processes, Roots, SecureFiles};
use crate::runtime_types::AccountsChanged;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Default)]
pub struct AccountRegistry {
    _folders: Vec<RunFolder>,
}

impl AccountRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn discover(&mut self, snap: FolderSnapshot, now: SystemTime) -> AccountsChanged {
        let _ = (snap, now);
        AccountsChanged::default()
    }

    pub fn record(&mut self, s: AccountSighting, now: SystemTime) -> AccountsChanged {
        let _ = (s, now);
        AccountsChanged::default()
    }

    pub fn accounts(&self) -> Vec<Account> {
        Vec::new()
    }

    pub fn folders(&self) -> Vec<RunFolder> {
        Vec::new()
    }

    pub fn attribution(&self, folder: &AccountId, started: Option<SystemTime>) -> Attribution {
        let _ = (folder, started);
        Attribution::Known(None)
    }

    pub fn apply_user(&mut self, a: AccountAction) -> Result<AccountsChanged, String> {
        let _ = a;
        Err("Accounts can't be changed in this build yet.".into())
    }

    /// key() → login hash; `None` until the folders were read (CL§5.2).
    pub fn folder_logins(&self) -> Option<BTreeMap<String, String>> {
        None
    }

    /// CL§7.2's rules.
    pub fn backfill_folders(&self, allowed: &BTreeSet<IdentityId>) -> Vec<BackfillFolder> {
        let _ = allowed;
        Vec::new()
    }
}

/// One read of the home folder for discovery (a Job on `an-io`).
pub fn read_folder_snapshot(
    roots: &Roots,
    explicit: &[PathBuf],
    files: &dyn SecureFiles,
    procs: &dyn Processes,
) -> FolderSnapshot {
    let _ = (explicit, files, procs);
    FolderSnapshot {
        home: roots.home.to_string_lossy().into_owned(),
        ..FolderSnapshot::default()
    }
}
