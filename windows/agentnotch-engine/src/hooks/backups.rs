//! Copies of settings.json kept beside it before every write of ours
//! (HS§3.4): timestamped ones, the newest five, and the bytes from before
//! this app first changed the file, kept forever. A backup can hold whatever
//! settings.json holds (`env` secrets included), so each is made owner-only
//! from its first byte, through [`SecureFiles`].
//!
//! The backups are also where a lost saved status line is looked for
//! ([`newest_status_line`]).

use crate::core::settings_doc::{is_blank, Json, SettingsDocument};
use crate::platform::{Clock, SecureFiles};
use chrono::{DateTime, Local};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const BACKUP_PREFIX: &str = "settings.json.agentnotch-";
pub const BACKUP_SUFFIX: &str = ".bak";
/// The copy made before this app first changed the file; never pruned.
pub const ORIGINAL_BACKUP_NAME: &str = "settings.json.agentnotch.original.bak";
/// How many of our own timestamped backups to keep beside each settings.json.
pub const MAX_BACKUPS: usize = 5;

/// Every backup of ours to look through for a lost saved status line, in the
/// order they are trusted: the timestamped ones, then the original.
const OWN_BACKUP_PREFIXES: [&str; 2] = [BACKUP_PREFIX, ORIGINAL_BACKUP_NAME];

/// How many later milliseconds are tried when a backup's name is taken.
const NAME_TRIES: u64 = 1000;

/// What backing up did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backup {
    /// A blank file holds nothing to keep.
    Blank,
    /// The newest backup already holds these bytes.
    AlreadyHeld(PathBuf),
    Written(PathBuf),
    /// Couldn't be written. As on the Mac the settings write still goes
    /// ahead: it replaces the file atomically, so the old bytes are only
    /// without a second copy, never half written.
    Failed(String),
}

impl Backup {
    /// The backup that holds the bytes, if one does.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Backup::AlreadyHeld(path) | Backup::Written(path) => Some(path),
            Backup::Blank | Backup::Failed(_) => None,
        }
    }
}

/// `settings.json.agentnotch-yyyyMMdd-HHmmss-SSS.bak`, in local time.
pub fn backup_name(at: SystemTime) -> String {
    let stamp = DateTime::<Local>::from(at).format("%Y%m%d-%H%M%S-%3f");
    format!("{BACKUP_PREFIX}{stamp}{BACKUP_SUFFIX}")
}

fn directory(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Copies settings.json's bytes aside before a write, owner-only. `data` is
/// what was read for the plan rather than a fresh read, so the backup is
/// exactly what the write is about to replace. Skipped when the newest backup
/// already holds these bytes. The first time, the bytes are also kept as the
/// original, which rotation never removes. `settings_path` is the resolved
/// file: the backups go beside the target, not beside a link to it.
pub fn back_up_settings(
    data: &[u8],
    settings_path: &Path,
    files: &dyn SecureFiles,
    clock: &dyn Clock,
) -> Backup {
    if is_blank(data) {
        return Backup::Blank;
    }
    let directory = directory(settings_path);
    // Never replaces one that is there: only the first bytes are the original.
    let _ = files.create_exclusive(&directory.join(ORIGINAL_BACKUP_NAME), data);

    if let Some(newest) = our_backups(&directory).last() {
        let newest = directory.join(newest);
        if fs::read(&newest).is_ok_and(|held| held == data) {
            return Backup::AlreadyHeld(newest);
        }
    }

    // Two writes within one millisecond (a re-plan, a test's still clock)
    // must not cost the earlier backup: the later one takes the next name,
    // which also keeps it the newest by name.
    let now = clock.now();
    let mut failure = String::from("every name is taken");
    for bump in 0..NAME_TRIES {
        let path = directory.join(backup_name(now + Duration::from_millis(bump)));
        match files.create_exclusive(&path, data) {
            Ok(true) => {
                prune_backups(&directory);
                return Backup::Written(path);
            }
            Ok(false) => continue,
            Err(error) => {
                failure = error.to_string();
                break;
            }
        }
    }
    Backup::Failed(failure)
}

/// Our timestamped backups in `directory`, oldest first (the timestamp format
/// sorts as text).
pub fn our_backups(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = names_in(directory)
        .into_iter()
        .filter(|name| name.starts_with(BACKUP_PREFIX) && name.ends_with(BACKUP_SUFFIX))
        .collect();
    names.sort();
    names
}

fn names_in(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Trims our own backups to the newest [`MAX_BACKUPS`]. Only ever removes
/// files matching both our prefix and suffix: nothing else in the config
/// folder is a candidate, and the original isn't one.
pub fn prune_backups(directory: &Path) {
    let ours = our_backups(directory);
    let excess = ours.len().saturating_sub(MAX_BACKUPS);
    for name in &ours[..excess] {
        let _ = fs::remove_file(directory.join(name));
    }
}

/// The newest timestamped backup of ours beside `settings_path`, if any.
pub fn newest_backup(settings_path: &Path) -> Option<PathBuf> {
    let directory = directory(settings_path);
    our_backups(&directory)
        .last()
        .map(|name| directory.join(name))
}

/// The status line the user had before our wrapper replaced it, from the
/// backups beside `settings_path`: the newest backup (by name) that wasn't
/// taken while a wrapper was the status line says what it was. `None` when
/// that backup has none (the user had no status line then; an older backup's
/// may be one they removed since). For restoring when the saved copy is gone.
/// `is_wrapper` says which status lines are a wrapper
/// (`plan::is_a_wrapper`). The timestamped backups are trusted before the
/// original, newest first.
pub fn newest_status_line(
    settings_path: &Path,
    is_wrapper: &dyn Fn(&Json) -> bool,
) -> Option<Json> {
    let directory = directory(settings_path);
    let rank = |name: &str| {
        OWN_BACKUP_PREFIXES
            .iter()
            .position(|prefix| name.starts_with(prefix))
    };
    let mut names: Vec<(usize, String)> = names_in(&directory)
        .into_iter()
        .filter(|name| name.ends_with(BACKUP_SUFFIX))
        .filter_map(|name| rank(&name).map(|rank| (rank, name)))
        .collect();
    names.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0).then_with(|| rhs.1.cmp(&lhs.1)));
    for (_, name) in names {
        let Some(document) = fs::read(directory.join(&name))
            .ok()
            .and_then(|data| SettingsDocument::new(Some(&data)))
        else {
            continue;
        };
        let status_line = document.get("statusLine")?;
        if is_wrapper(status_line) {
            continue;
        }
        let has_members = status_line
            .members()
            .is_some_and(|members| !members.is_empty());
        return has_members.then(|| status_line.clone());
    }
    None
}
