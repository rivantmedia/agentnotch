//! Which account a Claude Code session that Claude Desktop hosts runs as
//! (DesktopHostedSessions.swift, AU§13).
//!
//! Claude Desktop runs the sessions it hosts as whoever Desktop is signed in
//! as, whatever config folder they run in, so the folder says nothing about
//! them. Claude Code marks such a session in its registry entry (an
//! entrypoint of `claude-desktop`, `claude-desktop-3p` or `local-agent`, and
//! `hostSessionId`, Desktop's own id for it), and Desktop keeps a record of
//! each session it hosts under the account and organization it ran as:
//!
//! ```text
//! <Claude Desktop folder>\claude-code-sessions\<accountUuid>\<organizationUuid>\<hostSessionId>.json
//! ```
//!
//! So the session is the known identity whose record exists: a few metadata
//! reads per identity (its account UUID and organization; for an identity
//! whose organization isn't known, a listing of its account's folder), and
//! no record is ever opened. Desktop writes plain files and folders, so a
//! link anywhere from the root down (symbolic link or junction: any reparse
//! point) is never followed: it is no record. No record, records under more
//! than one identity, or a Desktop-hosted entry without a usable
//! `hostSessionId`: the session can't be attributed, and nothing is guessed.
//! Nothing is looked at when sealed.
//!
//! **[ASSUMPTION]** the folder names are the Mac bundle's; the Windows
//! install (`%APPDATA%\Claude`, or an MSIX package's copy) may differ, in
//! which case no record is found and every hosted session stays unsure.

use crate::model::{Attribution, DesktopCandidate, IdentityId};
use crate::platform::SecureFiles;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The folder of Desktop's records inside a Claude Desktop folder.
pub const RECORDS_DIR: &str = "claude-code-sessions";

/// The entrypoints Claude Code writes a `hostSessionId` for.
pub const ENTRYPOINTS: [&str; 3] = ["claude-desktop", "claude-desktop-3p", "local-agent"];

/// A registry entry's (or hook's) entrypoint of a session Claude Desktop
/// hosts. Any other entrypoint naming Desktop counts too: it carries no
/// `hostSessionId`, so such a session is never attributed.
pub fn is_desktop_hosted(entrypoint: Option<&str>) -> bool {
    let Some(value) = entrypoint else {
        return false;
    };
    let value = value.trim_matches([' ', '\t']).to_lowercase();
    !value.is_empty() && (ENTRYPOINTS.contains(&value.as_str()) || value.contains("desktop"))
}

/// Claude Code's own check of the id (`^local_[0-9a-f-]{8,72}$`), which also
/// keeps it a plain file name.
pub fn valid_host_session_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("local_") else {
        return false;
    };
    (8..=72).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

/// A UUID as Desktop names its folders: 36 characters, hex digits and dashes.
fn is_uuid(text: &str) -> bool {
    text.len() == 36 && uuid::Uuid::parse_str(text).is_ok()
}

/// A UUID as Desktop names its folder (lowercase), and as given if that
/// differs (a case-sensitive volume).
fn spellings(uuid: &str) -> Vec<String> {
    let lower = uuid.to_lowercase();
    if lower == uuid {
        vec![uuid.to_owned()]
    } else {
        vec![lower, uuid.to_owned()]
    }
}

/// `<Claude Desktop folder>\claude-code-sessions`.
pub fn records_root(desktop_root: &Path) -> PathBuf {
    desktop_root.join(RECORDS_DIR)
}

/// The identities whose record of `host_session_id` exists under `root` (the
/// records folder). Asks `exists` for each record's path and `list` for an
/// account folder's entries; nothing else is looked at.
fn holders(
    host_session_id: &str,
    candidates: &[DesktopCandidate],
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
    list: &dyn Fn(&Path) -> Vec<String>,
) -> Vec<IdentityId> {
    let file = format!("{host_session_id}.json");
    let mut found: Vec<IdentityId> = Vec::new();
    for candidate in candidates {
        let account = candidate.account_uuid.trim_matches([' ', '\t']);
        if !is_uuid(account) {
            continue;
        }
        let has_record = spellings(account).iter().any(|account_name| {
            let account_folder = root.join(account_name);
            let organizations: Vec<String> = match candidate
                .organization_uuid
                .as_deref()
                .map(|o| o.trim_matches([' ', '\t']))
                .filter(|o| is_uuid(o))
            {
                Some(organization) => spellings(organization),
                None => {
                    let mut names: Vec<String> = list(&account_folder)
                        .into_iter()
                        .filter(|name| is_uuid(name))
                        .collect();
                    names.sort();
                    names
                }
            };
            organizations
                .iter()
                .any(|organization| exists(&account_folder.join(organization).join(&file)))
        });
        if has_record && !found.contains(&candidate.identity_id) {
            found.push(candidate.identity_id.clone());
        }
    }
    found
}

/// The identity whose Desktop record of `host_session_id` exists under
/// `root` (the records folder); `None` when none does, when more than one
/// does, or when the id isn't one. Pure apart from `exists` and `list`,
/// which a test replaces.
pub fn identity(
    host_session_id: Option<&str>,
    candidates: &[DesktopCandidate],
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
    list: &dyn Fn(&Path) -> Vec<String>,
) -> Option<IdentityId> {
    let id = host_session_id.filter(|id| valid_host_session_id(id))?;
    let mut holders = holders(id, candidates, root, exists, list);
    if holders.len() == 1 {
        holders.pop()
    } else {
        None
    }
}

/// What is at `path` itself (a link is not what it points to) is a plain
/// folder or file: not a reparse point, and a directory (`want_dir`) or a
/// regular file by `symlink_metadata`. Never opened.
fn is_plain(files: &dyn SecureFiles, path: &Path, want_dir: bool) -> bool {
    if files.is_reparse(path).unwrap_or(true) {
        return false;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let kind = meta.file_type();
            if want_dir {
                kind.is_dir()
            } else {
                kind.is_file()
            }
        }
        Err(_) => false,
    }
}

/// Desktop's record `<root>\<account>\<organization>\<id>.json` is a regular
/// file reached through real folders: the root, then the account's and the
/// organization's folders, then the record, each checked in that order and
/// only while the ones above are real, so a link is never followed (not
/// even to look further down). Never opened.
pub fn is_record(files: &dyn SecureFiles, path: &Path) -> bool {
    let Some(organization) = path.parent() else {
        return false;
    };
    let Some(account) = organization.parent() else {
        return false;
    };
    let Some(root) = account.parent() else {
        return false;
    };
    is_plain(files, root, true)
        && is_plain(files, account, true)
        && is_plain(files, organization, true)
        && is_plain(files, path, false)
}

/// An account folder's entries, when the root and the folder are real
/// folders (never a link, which isn't followed); empty otherwise, or when it
/// can't be listed.
pub fn listing(files: &dyn SecureFiles, path: &Path) -> Vec<String> {
    let Some(root) = path.parent() else {
        return Vec::new();
    };
    if !is_plain(files, root, true) || !is_plain(files, path, true) {
        return Vec::new();
    }
    match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// `Job::DesktopHosted`: the identity whose Desktop record of
/// `host_session_id` exists under any of `roots` (Claude Desktop's folders,
/// `Roots::claude_desktop`), or `None`: no record, records under more than
/// one identity (also across roots), an id that isn't Claude Code's form.
/// Metadata reads and folder listings only, every link refused top-down,
/// no file opened. Sealed: `None` without touching the disk.
pub fn find_hosted(
    roots: &[PathBuf],
    host_session_id: &str,
    candidates: &[DesktopCandidate],
    files: &dyn SecureFiles,
    sealed: bool,
) -> Option<IdentityId> {
    if sealed || !valid_host_session_id(host_session_id) {
        return None;
    }
    let exists = |path: &Path| is_record(files, path);
    let list = |path: &Path| listing(files, path);
    let mut all: Vec<IdentityId> = Vec::new();
    for root in roots {
        for holder in holders(
            host_session_id,
            candidates,
            &records_root(root),
            &exists,
            &list,
        ) {
            if !all.contains(&holder) {
                all.push(holder);
            }
        }
    }
    if all.len() == 1 {
        all.pop()
    } else {
        None
    }
}

/// The folder's best guess of whose a session is, for an unsure one.
fn best_guess(folder: &Attribution) -> Option<IdentityId> {
    match folder {
        Attribution::Known(identity) | Attribution::Unsure(identity) => identity.clone(),
        Attribution::Waiting => None,
    }
}

/// A session Claude Desktop hosts, once its record was looked for: its
/// record's identity when found, else unsure (shown under the folder's
/// account, never recorded as anyone's). Pure.
pub fn hosted_attribution(
    found: Option<IdentityId>,
    folder_best_guess: Option<IdentityId>,
) -> Attribution {
    match found {
        Some(identity) => Attribution::Known(Some(identity)),
        None => Attribution::Unsure(folder_best_guess),
    }
}

/// A session's attribution once Claude Desktop hosting it is taken into
/// account: [`hosted_attribution`] for a hosted one, the folder's own for
/// any other. Pure.
pub fn attribution(
    folder: &Attribution,
    is_desktop_hosted: bool,
    desktop_identity: Option<IdentityId>,
) -> Attribution {
    if !is_desktop_hosted {
        return folder.clone();
    }
    hosted_attribution(desktop_identity, best_guess(folder))
}

/// What [`DesktopAttributor::lookup`] says to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostedLookup {
    /// Not a Claude Code host session id: nothing to ask, ever.
    Invalid,
    /// Remembered (an in-flight or recent miss is `Known(None)`).
    Known(Option<IdentityId>),
    /// Issue `Job::DesktopHosted` and call [`DesktopAttributor::record`]
    /// with its answer. Asking again within `RETRY_AFTER` answers `Known(None)`
    /// until the answer is recorded.
    Ask,
}

/// The runtime's memory of its Desktop lookups: a record found holds for as
/// long as the session runs (Desktop doesn't move it) and the known
/// identities stay the same; a miss is looked at again after [`RETRY_AFTER`]
/// (the record may be written just after the session starts).
#[derive(Debug, Default)]
pub struct DesktopAttributor {
    answers: Vec<Answer>,
}

#[derive(Debug)]
struct Answer {
    host_session_id: String,
    identity: Option<IdentityId>,
    candidates: Vec<DesktopCandidate>,
    at: SystemTime,
}

/// How long a miss stands before the record is looked for again.
pub const RETRY_AFTER: Duration = Duration::from_secs(15);

impl DesktopAttributor {
    pub fn new() -> Self {
        Self::default()
    }

    /// What is known of `host_session_id`'s identity, or that it should be
    /// asked for. Marks the question as asked at `now`.
    pub fn lookup(
        &mut self,
        host_session_id: &str,
        candidates: &[DesktopCandidate],
        now: SystemTime,
    ) -> HostedLookup {
        if !valid_host_session_id(host_session_id) {
            return HostedLookup::Invalid;
        }
        if let Some(known) = self
            .answers
            .iter()
            .find(|a| a.host_session_id == host_session_id)
        {
            let fresh = now
                .duration_since(known.at)
                .map(|elapsed| elapsed < RETRY_AFTER)
                .unwrap_or(true);
            if known.candidates == candidates && (known.identity.is_some() || fresh) {
                return HostedLookup::Known(known.identity.clone());
            }
        }
        self.store(host_session_id, None, candidates, now);
        HostedLookup::Ask
    }

    /// The answer of a `Job::DesktopHosted` for these candidates.
    pub fn record(
        &mut self,
        host_session_id: &str,
        candidates: &[DesktopCandidate],
        identity: Option<IdentityId>,
        now: SystemTime,
    ) {
        if valid_host_session_id(host_session_id) {
            self.store(host_session_id, identity, candidates, now);
        }
    }

    fn store(
        &mut self,
        host_session_id: &str,
        identity: Option<IdentityId>,
        candidates: &[DesktopCandidate],
        at: SystemTime,
    ) {
        self.answers
            .retain(|a| a.host_session_id != host_session_id);
        self.answers.push(Answer {
            host_session_id: host_session_id.to_owned(),
            identity,
            candidates: candidates.to_vec(),
            at,
        });
    }

    /// Forgets the sessions no longer running.
    pub fn retain(&mut self, host_session_ids: &[String]) {
        self.answers
            .retain(|a| host_session_ids.contains(&a.host_session_id));
    }

    pub fn len(&self) -> usize {
        self.answers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.answers.is_empty()
    }
}
