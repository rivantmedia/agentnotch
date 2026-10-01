//! What the cloud reads from the running engine, as pure mappings over the
//! engine's own model types (CL§5.9, §7.2; the Mac's `CloudLiveEnvironment`
//! 46-117). The hub implements [`CloudDeps`](crate::runtime_types::CloudDeps)
//! with these: the accounts the website may hear of, who is signed in to
//! each folder, and the folders the backfill may read.

use super::backfill;
use super::keys;
use super::pass::CloudAccountInfo;
use crate::core::paths::Paths;
use crate::model::{Account, AccountId, BackfillFolder, FolderKind, IdentityId, RunFolder};
use crate::runtime_types::{CloudAccount, CloudFolder};
use std::collections::{BTreeMap, BTreeSet};

/// The identities of `accounts` as the cloud gets them: signed in, tracked
/// (the Mac's "visible": a hidden one is not polled, hooked or sent), with
/// the folders it runs in and is stored in (for its organization). `folders`
/// is every folder the registry knows; an account names its own by id.
/// `corrected` are folders whose `.claude.json` names another account's
/// details (a Claude Parallel Profiles mirror): their organization is stale
/// and never used for the key. `mirrors_default`: the extension mirrors
/// accounts into `~\.claude`, so its history can't be told apart. Pure.
pub fn cloud_accounts(
    accounts: &[Account],
    folders: &[RunFolder],
    corrected: &BTreeSet<AccountId>,
    mirrors_default: bool,
    paths: &Paths,
) -> Vec<CloudAccount> {
    let by_id: BTreeMap<&AccountId, &RunFolder> = folders.iter().map(|f| (&f.id, f)).collect();
    accounts
        .iter()
        .filter(|account| account.is_tracked && account.is_signed_in)
        .map(|account| {
            let own: Vec<&RunFolder> = account
                .run_dirs
                .iter()
                .chain(&account.store_dirs)
                .filter_map(|id| by_id.get(id).copied())
                .collect();
            let cloud_folders: Vec<CloudFolder> = own
                .iter()
                .map(|folder| {
                    let dir = folder.config_dir.to_string_lossy();
                    let is_default = paths.is_default_config_dir(&dir);
                    CloudFolder {
                        config_dir: folder.config_dir.clone(),
                        config_dir_env: folder.config_dir_env.clone(),
                        organization_uuid: folder
                            .identity
                            .as_ref()
                            .and_then(|i| i.organization_uuid.clone()),
                        corrected: corrected.contains(&folder.id),
                        kind: folder.kind,
                        is_default,
                        mirrored_default: is_default && mirrors_default,
                    }
                })
                .collect();
            // The organization's name, from a folder that is the account's
            // own (a mirrored copy's is another account's) when there is
            // one.
            let organization_name = own
                .iter()
                .filter(|f| !corrected.contains(&f.id))
                .chain(own.iter())
                .find_map(|f| {
                    f.identity
                        .as_ref()
                        .and_then(|i| i.organization_name.clone())
                        .filter(|name| !name.trim().is_empty())
                });
            CloudAccount {
                identity_id: account.identity_id.clone(),
                email: account.email.clone(),
                organization_name,
                plan: account.plan_name.clone(),
                label: Some(account.label.clone()).filter(|l| !l.is_empty()),
                folders: cloud_folders,
            }
        })
        .collect()
}

/// The accounts the website may hear of: those with an account UUID (an
/// email-only or unsigned folder has no key that is the same on every
/// computer), keyed by their own account UUID and organization
/// ([`keys::account_key_of`]), never by how this PC happens to group them.
/// Pure.
pub fn account_infos(accounts: &[CloudAccount]) -> Vec<CloudAccountInfo> {
    accounts
        .iter()
        .filter_map(|account| {
            let uuid = account.identity_id.account_uuid()?;
            let key = keys::account_key_of(account)?;
            Some(CloudAccountInfo {
                identity_id: account.identity_id.clone(),
                account_key: key,
                account_uuid: uuid.to_owned(),
                email: account.email.clone(),
                organization_name: account.organization_name.clone(),
                plan: account.plan.clone(),
                label: account.label.clone(),
            })
        })
        .collect()
}

/// Folder → login for every folder someone is signed in to
/// ([`backfill::login`]: a digest of its own account UUID, organization and
/// email). Pure.
pub fn folder_logins(folders: &[RunFolder]) -> BTreeMap<String, String> {
    let mut logins = BTreeMap::new();
    for folder in folders {
        let identity = folder.identity.as_ref();
        let login = backfill::login(
            identity.and_then(|i| i.account_uuid.as_deref()),
            identity.and_then(|i| i.organization_uuid.as_deref()),
            identity.and_then(|i| i.email.as_deref()),
        );
        if let Some(login) = login {
            logins.insert(folder.config_dir.to_string_lossy().into_owned(), login);
        }
    }
    logins
}

/// Which identity each of `accounts` runs or stores in a folder: the
/// accounts' own lists, by folder.
pub fn identity_of_folder(accounts: &[Account]) -> BTreeMap<AccountId, IdentityId> {
    let mut map = BTreeMap::new();
    for account in accounts {
        for id in account.run_dirs.iter().chain(&account.store_dirs) {
            map.entry(id.clone())
                .or_insert_with(|| account.identity_id.clone());
        }
    }
    map
}

/// Every folder the registry knows, with the account of those the backfill
/// may read: a run folder of an allowed account whose own `.claude.json`
/// names that account (not a mirrored or corrected copy, and not `~\.claude`
/// while Claude Parallel Profiles mirrors accounts into it: whose its
/// history is can't be told), then the `infrastructure` folders (shared
/// history), which belong to no account. Who is signed in where, and since
/// when, is added by [`backfill::folders`]. Pure.
pub fn backfill_folders(
    folders: &[RunFolder],
    accounts: &[CloudAccountInfo],
    identity_of_folder: &BTreeMap<AccountId, IdentityId>,
    mirrors_default: bool,
    corrected: &BTreeSet<AccountId>,
    infrastructure: &[String],
    paths: &Paths,
) -> Vec<BackfillFolder> {
    let allowed: BTreeMap<&IdentityId, &CloudAccountInfo> =
        accounts.iter().rev().map(|a| (&a.identity_id, a)).collect();
    let mut result: Vec<BackfillFolder> = folders
        .iter()
        .map(|folder| {
            let dir = folder.config_dir.to_string_lossy().into_owned();
            let mut account = None;
            if folder.kind == FolderKind::Run {
                account = identity_of_folder
                    .get(&folder.id)
                    .and_then(|identity| allowed.get(identity).copied());
            }
            if corrected.contains(&folder.id)
                || (mirrors_default && paths.is_default_config_dir(&dir))
            {
                account = None;
            }
            BackfillFolder {
                config_dir: dir,
                identity_id: account.map(|a| a.identity_id.clone()),
                account_key: account.map(|a| a.account_key.clone()),
                signed_in_since: None,
            }
        })
        .collect();
    result.extend(infrastructure.iter().map(|dir| BackfillFolder {
        config_dir: dir.clone(),
        identity_id: None,
        account_key: None,
        signed_in_since: None,
    }));
    result
}
