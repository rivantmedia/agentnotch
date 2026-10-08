//! Which `projects\` folders hold only one account's history (CL§7.2; the
//! Mac's `CloudBackfill`).
//!
//! A folder is read by the backfill only when its `projects\` is a real
//! folder of its own: no link or junction anywhere on the way (a linked
//! history, `~\.claude-shared` and the like, can only be attributed by the
//! ledger), reached by no other known folder, and its login is known and
//! dated. The Mac compares `realpath` with the lexical path, which is
//! case-sensitive and fails on Windows; here every component of the path is
//! asked whether it is a link (`SecureFiles::is_reparse`: symlinks and
//! junctions), and canonical paths are grouped by `Paths::key`.
//!
//! [`is_shared`] asks the same of one running session's history, with the
//! Mac's full rule (a link, or another known folder reaching the same
//! folder, compared by file identity: volume and file index, never by
//! spelling), for the ledger's "only what the app saw running counts".

use super::folder_logins::CloudFolderLogins;
use super::keys::sha256_hex;
use crate::core::paths::Paths;
use crate::model::{BackfillFolder, IdentityId};
use crate::platform::SecureFiles;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::SystemTime;

use super::contract::trim_spaces;

/// A folder to look in, and whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The physical `projects` folder (on-disk case).
    pub projects: String,
    pub identity_id: IdentityId,
    pub account_key: String,
    pub config_dir: String,
    /// Signed in as that account since (as far as the app saw): only
    /// transcripts begun after it are its.
    pub signed_in_since: SystemTime,
}

/// One folder as the backfill sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub config_dir: String,
    /// The identity it runs as, when it is a run folder of a visible,
    /// signed-in account that can be backfilled.
    pub identity_id: Option<IdentityId>,
    pub account_key: Option<String>,
    /// Who is signed in there, as `CloudFolderLogins` tells logins apart;
    /// `None` for nobody (or unknown).
    pub login: Option<String>,
    /// Since when the app has seen `login` there; `None` when unknown.
    pub signed_in_since: Option<SystemTime>,
}

/// The folders the hub offers ([`BackfillFolder`]) with who is signed in to
/// each (`CloudDeps::folder_logins`, by folder) and since when (the cloud's
/// own record: what the hub put in `signed_in_since` is not used).
pub fn folders(
    offered: &[BackfillFolder],
    logins: &BTreeMap<String, String>,
    history: &CloudFolderLogins,
    paths: &Paths,
) -> Vec<Folder> {
    let by_key: BTreeMap<String, &String> = logins
        .iter()
        .map(|(folder, login)| (paths.key(folder), login))
        .collect();
    offered
        .iter()
        .map(|folder| {
            let login = by_key
                .get(&paths.key(&folder.config_dir))
                .map(|l| (*l).clone());
            let signed_in_since = history.since(&folder.config_dir, login.as_deref());
            Folder {
                config_dir: folder.config_dir.clone(),
                identity_id: folder.identity_id.clone(),
                account_key: folder.account_key.clone(),
                login,
                signed_in_since,
            }
        })
        .collect()
}

/// The roots to backfill, over the real file system.
pub fn roots(folders: &[Folder], paths: &Paths, files: &dyn SecureFiles) -> Vec<Root> {
    roots_with(folders, paths, files, &|path| Path::new(path).is_dir())
}

/// [`roots`] with the folder test injected (tests with Windows-style paths).
pub fn roots_with(
    folders: &[Folder],
    paths: &Paths,
    files: &dyn SecureFiles,
    is_directory: &dyn Fn(&str) -> bool,
) -> Vec<Root> {
    // Real projects folder (as a key) → the config folders that reach it.
    let mut reached_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut candidates: Vec<(String, &Folder, String)> = Vec::new();
    for folder in folders {
        let config_dir = paths.normalize(&folder.config_dir);
        let projects = paths.join(&config_dir, "projects");
        if !is_directory(&projects) {
            continue;
        }
        let real = match files.canonical(Path::new(&projects)) {
            Ok(real) => paths.normalize(&real.to_string_lossy()),
            Err(_) => continue,
        };
        reached_by
            .entry(paths.key(&real))
            .or_default()
            .insert(paths.key(&config_dir));
        // A link anywhere on the way: shared (or at least not provably its
        // own).
        if has_link_on_the_way(&projects, paths, files)
            || folder.identity_id.is_none()
            || folder.account_key.is_none()
            || folder.signed_in_since.is_none()
        {
            continue;
        }
        candidates.push((real, folder, config_dir));
    }
    candidates
        .into_iter()
        .filter_map(|(real, folder, config_dir)| {
            if reached_by.get(&paths.key(&real))?.len() != 1 {
                return None;
            }
            Some(Root {
                projects: real,
                identity_id: folder.identity_id.clone()?,
                account_key: folder.account_key.clone()?,
                config_dir,
                signed_in_since: folder.signed_in_since?,
            })
        })
        .collect()
}

/// Whether any component of `path` (the drive or root aside) is a symlink
/// or junction; a component that can't be asked counts as one.
fn has_link_on_the_way(path: &str, paths: &Paths, files: &dyn SecureFiles) -> bool {
    let mut so_far = String::new();
    for (index, component) in paths.components(path).into_iter().enumerate() {
        if index == 0 {
            so_far = component.clone();
            if paths.is_root(&component) {
                continue;
            }
        } else {
            so_far = paths.join(&so_far, &component);
        }
        match files.is_reparse(Path::new(&so_far)) {
            Ok(false) => {}
            _ => return true,
        }
    }
    false
}

/// Whether a session's history is shared with other folders, so other
/// accounts may have written its transcript and only the ledger can tell
/// whose a line is: the transcript's project folder, or the `projects\` it
/// is in (before a transcript is known, `config_dir`'s), is a link (Claude
/// Parallel Profiles links every folder's `projects\` to `~\.claude-shared`),
/// or another of `folders` reaches the same physical `projects`. Links
/// further up (a config folder kept with dotfiles) share nothing by
/// themselves, and folders are compared as files (`SecureFiles::identity`:
/// volume and file index), not as spellings (letter case, 8.3 names,
/// `\\?\`). Unlike a backfill root's test, a folder the app knows nothing
/// else about is its own. A folder that can't be asked counts as its own
/// (nothing is guessed shared); a link that can't be asked as no link.
pub fn is_shared(
    transcript_path: Option<&str>,
    config_dir: &str,
    folders: &[BackfillFolder],
    paths: &Paths,
    files: &dyn SecureFiles,
) -> bool {
    let is_link = |path: &str| files.is_reparse(Path::new(path)).unwrap_or(false);
    // Which folder it is, however it is spelled or reached. The file's
    // times and size say nothing about that.
    let physical = |path: &str| {
        files
            .identity(Path::new(path))
            .ok()
            .map(|id| (id.volume, id.index))
    };
    let config_dir = paths.normalize(config_dir);
    let mut projects = paths.join(&config_dir, "projects");
    if let Some(transcript) = transcript_path.filter(|path| !path.is_empty()) {
        let folder = paths.parent(&paths.normalize(transcript));
        if let Some(folder) = &folder {
            if is_link(folder) {
                return true;
            }
            if let Some(above) = paths.parent(folder) {
                projects = above;
            }
        }
    }
    if is_link(&projects) {
        return true;
    }
    let Some(shared) = physical(&projects) else {
        return false;
    };
    let own_folder = physical(&config_dir);
    folders.iter().any(|other| {
        let other_dir = paths.normalize(&other.config_dir);
        // The same folder, however it is spelled, is not another.
        if paths.same(&other_dir, &config_dir) {
            return false;
        }
        if own_folder.is_some() && physical(&other_dir) == own_folder {
            return false;
        }
        physical(&paths.join(&other_dir, "projects")) == Some(shared)
    })
}

/// Who is signed in to a folder, as `CloudFolderLogins` tells logins apart:
/// a digest of its own `oauthAccount` account UUID, organization and email
/// (a `/login` as someone else changes it). `None` when nobody is.
pub fn login(
    account_uuid: Option<&str>,
    organization_uuid: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    let clean = |value: Option<&str>| trim_spaces(value.unwrap_or("")).to_lowercase();
    let parts = [clean(account_uuid), clean(organization_uuid), clean(email)];
    if parts[0].is_empty() && parts[2].is_empty() {
        return None;
    }
    Some(sha256_hex(parts.join("|")))
}
