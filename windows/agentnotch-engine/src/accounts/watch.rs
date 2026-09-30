//! Noticing a new VS Code window within seconds (WindowDirWatcher.swift,
//! AU§2.8). Claude Parallel Profiles makes a working copy for every VS Code
//! window (`~\.claude-windows\<id>`) the moment a workspace is opened, and
//! restocks it with another account when the window switches. A cheap look
//! every few seconds (a listing and a few stats, no reads) says when the
//! registry should read the folders again (a window appeared or went, or the
//! manifest changed) or only who is signed in (a window's `.claude.json`
//! changed; or `~\.claude.json`, which the extension rewrites whenever
//! another window is focused).
//!
//! The extension does nothing on native Windows (AU§0.2), so there the
//! fingerprint is always empty and nothing is ever noticed; the rule is kept
//! for the setups it describes.

use super::classify::{manifest_path, windows_root};
use crate::core::paths::Paths;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// How often the hub takes a fingerprint.
pub const INTERVAL: Duration = Duration::from_secs(5);

/// A file's modification time and size, or that it isn't there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub modified_at: Option<SystemTime>,
    pub size: u64,
    pub exists: bool,
}

/// What is watched, as comparable facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fingerprint {
    /// Every window folder's name, with its `.claude.json`'s stamp.
    pub windows: BTreeMap<String, FileStamp>,
    pub manifest: Option<FileStamp>,
    /// `~\.claude.json`, while the manifest exists.
    pub default_config: Option<FileStamp>,
}

pub fn stamp(path: &str) -> FileStamp {
    match std::fs::metadata(path) {
        Ok(meta) => FileStamp {
            modified_at: meta.modified().ok(),
            size: meta.len(),
            exists: true,
        },
        Err(_) => FileStamp {
            modified_at: None,
            size: 0,
            exists: false,
        },
    }
}

/// The windows folder as it is now (read-only).
pub fn fingerprint(paths: &Paths) -> Fingerprint {
    let root = windows_root(paths);
    let mut fingerprint = Fingerprint::default();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for name in entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
        {
            let config = paths.join(&paths.join(&root, &name), ".claude.json");
            fingerprint.windows.insert(name, stamp(&config));
        }
    }
    let manifest = stamp(&manifest_path(paths));
    if manifest.exists {
        fingerprint.manifest = Some(manifest);
        fingerprint.default_config = Some(stamp(&paths.default_identity_file()));
    }
    fingerprint
}

/// What changed between two looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    /// Only who is signed in to an existing folder may have: re-read identities.
    Identities,
    /// A window appeared or went, or the manifest changed: read the folders again.
    Folders,
}

pub fn change(old: Option<&Fingerprint>, new: &Fingerprint) -> Change {
    let Some(old) = old else {
        return Change::None;
    };
    if !old.windows.keys().eq(new.windows.keys()) || old.manifest != new.manifest {
        return Change::Folders;
    }
    let changed: Vec<(&FileStamp, &FileStamp)> = old
        .windows
        .values()
        .zip(new.windows.values())
        .filter(|(before, after)| before != after)
        .collect();
    // A window without a config yet that now has one is a new account folder.
    if changed
        .iter()
        .any(|(before, after)| before.exists != after.exists)
    {
        return Change::Folders;
    }
    // A window switched account, or the extension mirrored another account
    // into ~\.claude (or Claude Code wrote it).
    if !changed.is_empty() || old.default_config != new.default_config {
        return Change::Identities;
    }
    Change::None
}
