//! Where an account's usage check runs (UsageProbePlanner.swift, AU§9.9).
//! The check (`claude -p`, see `probe`) runs Claude Code itself, which may
//! refresh the folder's login and rewrite its `.claude.json`, so it runs
//! once per account, and only in a folder Claude Code runs in as that
//! account anyway: never in a Claude Parallel Profiles store (the extension
//! keeps stores as its own copy of an account; a store must not change
//! behind its back). Session summaries run in the same folder. Pure.

use crate::model::{Account, ExpectedLogin, FolderKind, Identity, IdentityId, RunFolder};
use std::path::Path;
use std::time::SystemTime;

/// The marker Claude Parallel Profiles leaves in a store it made.
pub const STORE_MARKER: &str = ".parallel-accounts-store";

/// The folder Claude Code uses with `CLAUDE_CONFIG_DIR` unset: `.claude`,
/// known without the variable.
pub fn is_default_folder(folder: &RunFolder) -> bool {
    folder.config_dir_env.as_deref().is_none_or(str::is_empty)
        && folder
            .config_dir
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(".claude"))
}

/// The run folder to check an identity's usage in: the most recently active
/// one (a session seen there, or Claude Code writing its config), else
/// `~\.claude` when it runs as this identity, else the first. `None` when
/// the identity runs nowhere (only its store holds it), and never a store.
///
/// - `activity`: when each of `run_dirs` was last active, if known.
/// - `prefers_own_folders`: Claude Parallel Profiles mirrors the focused
///   window's account into `~/.claude`, so `~/.claude` may change hands at
///   any moment: its VS Code windows and standalone folders come first,
///   `~/.claude` only when it has none. (Never set on native Windows.)
pub fn probe_folder_among(
    run_dirs: &[RunFolder],
    activity: &[Option<SystemTime>],
    is_default: &dyn Fn(&RunFolder) -> bool,
    prefers_own_folders: bool,
) -> Option<RunFolder> {
    let mut candidates: Vec<(usize, &RunFolder)> = run_dirs
        .iter()
        .enumerate()
        .filter(|(_, folder)| folder.kind == FolderKind::Run)
        .collect();
    if prefers_own_folders && candidates.iter().any(|(_, folder)| !is_default(folder)) {
        candidates.retain(|(_, folder)| !is_default(folder));
    }
    if candidates.is_empty() {
        return None;
    }
    let mut newest: Option<(&RunFolder, SystemTime)> = None;
    for (index, folder) in &candidates {
        let Some(at) = activity.get(*index).copied().flatten() else {
            continue;
        };
        let better = match newest {
            None => true,
            Some((_, best_at)) if at != best_at => at > best_at,
            // Equal times: the default folder, then the first by path.
            Some((best, _)) => {
                let (folder_default, best_default) = (is_default(folder), is_default(best));
                if folder_default != best_default {
                    folder_default
                } else {
                    folder.id.as_str() < best.id.as_str()
                }
            }
        };
        if better {
            newest = Some((folder, at));
        }
    }
    if let Some((folder, _)) = newest {
        return Some(folder.clone());
    }
    candidates
        .iter()
        .find(|(_, folder)| is_default(folder))
        .or(candidates.first())
        .map(|(_, folder)| (*folder).clone())
}

/// The run folder a probe (or a summary) for `identity` runs in, from what
/// the account registry knows: each folder's activity is when a session was
/// last seen in it.
pub fn probe_folder(
    accounts: &[Account],
    folders: &[RunFolder],
    identity: &IdentityId,
) -> Option<RunFolder> {
    let account = accounts.iter().find(|a| a.identity_id == *identity)?;
    let run_dirs: Vec<RunFolder> = account
        .run_dirs
        .iter()
        .filter_map(|id| folders.iter().find(|folder| folder.id == *id).cloned())
        .collect();
    let activity: Vec<Option<SystemTime>> = run_dirs.iter().map(|f| f.last_seen_at).collect();
    probe_folder_among(&run_dirs, &activity, &is_default_folder, false)
}

/// The login an account's folders must show for a check to run there.
pub fn expected_login(account: &Account) -> ExpectedLogin {
    ExpectedLogin {
        email: account.email.clone(),
        account_uuid: account.identity_id.account_uuid().map(str::to_owned),
        organization_scope: account.identity_id.organization().map(str::to_owned),
    }
}

/// Whether a folder's login (as its `.claude.json` names it now) is the
/// expected one: by email first, since a mirrored `.claude.json` keeps
/// another account's UUID; by UUID when there is no email. One login in two
/// organizations is two accounts: the same login in the other organization
/// is not it.
pub fn folder_runs(login: Option<&Identity>, expected: &ExpectedLogin) -> bool {
    let Some(login) = login else {
        return false;
    };
    let lower = |text: &Option<String>| text.as_deref().map(str::to_lowercase);
    let (email, wanted_email) = (lower(&login.email), lower(&expected.email));
    let (uuid, wanted_uuid) = (lower(&login.account_uuid), lower(&expected.account_uuid));
    if let (Some(scope), Some(organization)) = (
        lower(&expected.organization_scope),
        lower(&login.organization_uuid),
    ) {
        if organization != scope && email == wanted_email && uuid == wanted_uuid {
            return false;
        }
    }
    if let (Some(email), Some(wanted)) = (&email, &wanted_email) {
        return email == wanted;
    }
    if let (Some(uuid), Some(wanted)) = (&uuid, &wanted_uuid) {
        return uuid == wanted;
    }
    false
}

/// The identity a folder's login is, as far as one file tells: `expected_id`
/// when the login is the expected one ([`folder_runs`] decides), else a key
/// of the login's own (`uuid:<account>[/<organization>]` or
/// `email:<address>`) that is never `expected_id`; `None` when nobody is
/// signed in. What a probe reports as who its folder ran as.
pub fn identity_of_login(
    login: Option<&Identity>,
    expected: &ExpectedLogin,
    expected_id: &IdentityId,
) -> Option<IdentityId> {
    let login = login?;
    if folder_runs(Some(login), expected) {
        return Some(expected_id.clone());
    }
    let part = |text: &Option<String>| {
        text.as_deref()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
    };
    let (uuid, organization, email) = (
        part(&login.account_uuid),
        part(&login.organization_uuid),
        part(&login.email),
    );
    let mut keys: Vec<String> = Vec::new();
    if let Some(uuid) = &uuid {
        keys.push(format!("{}{uuid}", IdentityId::UUID_PREFIX));
        if let Some(organization) = &organization {
            keys.push(format!("{}{uuid}/{organization}", IdentityId::UUID_PREFIX));
        }
    }
    if let Some(email) = &email {
        keys.push(format!("{}{email}", IdentityId::EMAIL_PREFIX));
    }
    keys.into_iter()
        .map(IdentityId)
        .find(|key| key != expected_id)
}

/// A folder Claude Parallel Profiles made as an account store carries its
/// marker: never run in, whatever the last classification said.
pub fn has_store_marker(config_dir: &Path) -> bool {
    config_dir.join(STORE_MARKER).exists()
}
