//! What the cloud thread reads of the accounts (CloudLiveEnvironment.swift;
//! CL§0.1, §5.2, §7.2): the accounts the website may hear of, who is signed
//! in to each folder (for the backfill's "signed in since" record), and the
//! folders the backfill may read. All of it is derived from what the
//! registry already holds; nothing here reads the disk.

use super::folder::Folder;
use super::identities::{trim_spaces, IdentityAccount, ORGANIZATION_SEPARATOR};
use super::registry::AccountRegistry;
use crate::model::{BackfillFolder, FolderKind, IdentityId};
use crate::runtime_types::{CloudAccount, CloudFolder};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn clean(value: Option<&str>) -> Option<String> {
    let value = trim_spaces(value?).to_lowercase();
    (!value.is_empty()).then_some(value)
}

/// Who is signed in to a folder, as the "signed in since" record tells logins
/// apart: a digest of its own `oauthAccount`'s account UUID, organization and
/// email (a `/login` as someone else changes it). `None` when nobody is.
/// Local only; never sent.
pub fn login_digest(
    account_uuid: Option<&str>,
    organization_uuid: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    let parts = [
        clean(account_uuid).unwrap_or_default(),
        clean(organization_uuid).unwrap_or_default(),
        clean(email).unwrap_or_default(),
    ];
    if parts[0].is_empty() && parts[2].is_empty() {
        return None;
    }
    Some(sha256_hex(&parts.join("|")))
}

/// The organization an identity is signed in to: the one it was split by,
/// else the one its own folders' `oauthAccount` names (they name at most one,
/// or the identity would have been split). A mirrored copy's is never used:
/// it is stale. An identity with no folders has its own field.
pub fn organization_of(identity: &IdentityAccount, corrected: &BTreeSet<String>) -> Option<String> {
    if let Some(scope) = clean(identity.organization_scope.as_deref()) {
        return Some(scope);
    }
    if identity.folders().next().is_none() {
        return clean(identity.organization_uuid.as_deref());
    }
    identity
        .folders()
        .filter(|folder| !corrected.contains(folder.dir()))
        .find_map(|folder| clean(folder.organization_uuid()))
}

/// The website's key for an identity (the contract's `accountKey`): SHA-256
/// of the lower-cased `<accountUuid>/<organizationUuid>`, or of the account
/// UUID alone when no folder names an organization. It never depends on how
/// this PC groups identities. `None` for `email:` and `dir:` identities,
/// which have no account UUID to share across computers.
pub fn account_key(identity: &IdentityAccount, corrected: &BTreeSet<String>) -> Option<String> {
    let account = identity.id.account_uuid().map(trim_spaces)?;
    if account.is_empty() {
        return None;
    }
    let base = match organization_of(identity, corrected) {
        Some(organization) => format!("{account}{ORGANIZATION_SEPARATOR}{organization}"),
        None => account.to_owned(),
    };
    Some(sha256_hex(&base.to_lowercase()))
}

impl AccountRegistry {
    /// The accounts the website may hear of: tracked, remembered and signed
    /// in with an account UUID (an email-only or unsigned folder has no key
    /// that is the same on every computer), named as the app names them.
    pub fn cloud_accounts(&self) -> Vec<CloudAccount> {
        let paths = self.paths();
        self.identities()
            .iter()
            .filter(|identity| {
                !identity.is_hidden
                    && identity.is_signed_in()
                    && !self.is_forgotten(identity.id.as_str())
                    && account_key(identity, self.corrected_folders()).is_some()
            })
            .map(|identity| CloudAccount {
                identity_id: identity.id.clone(),
                email: identity.email.clone(),
                organization_name: identity.organization_name.clone(),
                plan: identity.plan_name(),
                label: Some(identity.label(paths)),
                folders: identity
                    .folders()
                    .map(|folder| {
                        let is_default = folder.is_default(paths);
                        CloudFolder {
                            config_dir: PathBuf::from(folder.dir()),
                            config_dir_env: folder
                                .config_dir_env
                                .clone()
                                .filter(|env| !env.is_empty()),
                            organization_uuid: folder.organization_uuid().map(str::to_owned),
                            corrected: self.corrected_folders().contains(folder.dir()),
                            kind: folder.kind,
                            is_default,
                            mirrored_default: is_default && self.mirrors_default(),
                        }
                    })
                    .collect(),
            })
            .collect()
    }

    /// Who is signed in to each folder, by `Paths::key`, for every folder
    /// someone is signed in to. `None` until identities have been read:
    /// before, every folder would look signed out (or unchanged since the
    /// last launch).
    pub fn folder_logins(&self) -> Option<BTreeMap<String, String>> {
        if !self.identities_read() {
            return None;
        }
        Some(folder_logins(self.known_folders(), |dir| {
            self.paths().key(dir)
        }))
    }

    /// Every folder the registry knows, with the account of those the
    /// backfill may read: a run folder of an allowed account whose own
    /// `.claude.json` names that account. Not a mirrored or corrected copy,
    /// and not `~\.claude` while Claude Parallel Profiles mirrors accounts
    /// into it: whose its history is can't be told. The shared history's
    /// folders follow with no account, so a history linked into several
    /// folders is seen to be shared. `signed_in_since` is the cloud's to fill
    /// (from its own record, by [`AccountRegistry::folder_logins`]).
    pub fn backfill_folders(&self, allowed: &BTreeSet<IdentityId>) -> Vec<BackfillFolder> {
        let paths = self.paths();
        let mut result: Vec<BackfillFolder> = self
            .known_folders()
            .iter()
            .map(|folder| {
                let mirrored_default =
                    paths.is_default_config_dir(folder.dir()) && self.mirrors_default();
                let identity = self.identity_of_folder(folder.dir()).filter(|identity| {
                    folder.kind == FolderKind::Run
                        && allowed.contains(&identity.id)
                        && !self.corrected_folders().contains(folder.dir())
                        && !mirrored_default
                });
                BackfillFolder {
                    config_dir: folder.dir().to_owned(),
                    identity_id: identity.map(|identity| identity.id.clone()),
                    account_key: identity
                        .and_then(|identity| account_key(identity, self.corrected_folders())),
                    signed_in_since: None,
                }
            })
            .collect();
        result.extend(
            self.infrastructure_dirs()
                .into_iter()
                .map(|config_dir| BackfillFolder {
                    config_dir,
                    identity_id: None,
                    account_key: None,
                    signed_in_since: None,
                }),
        );
        result
    }
}

/// Folder → login for every folder someone is signed in to.
pub fn folder_logins(folders: &[Folder], key: impl Fn(&str) -> String) -> BTreeMap<String, String> {
    folders
        .iter()
        .filter_map(|folder| {
            login_digest(
                folder.account_uuid(),
                folder.organization_uuid(),
                folder.email(),
            )
            .map(|login| (key(folder.dir()), login))
        })
        .collect()
}
