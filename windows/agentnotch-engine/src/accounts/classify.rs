//! What each Claude Code config folder is for, and which folders discovery
//! adds by itself (ConfigDirClassifier.swift and `AccountRegistry.discover`,
//! AU§2.5, §3.2). Pure functions of a [`FolderSnapshot`].
//!
//! Claude Parallel Profiles (the VS Code extension) makes many folders per
//! account where it runs: account stores (`~/.claude-<name>` with a
//! `.parallel-accounts-store` marker, or listed under the manifest's
//! `created`), a working copy per VS Code window (`~/.claude-windows/<12
//! hex>`), and the shared history (`~/.claude-shared`) linked into all of
//! them. So a folder is one of three kinds: Claude Code *runs* there, it is
//! a *store* (its identity is read; nothing is ever written or run there),
//! or it is *infrastructure* (never an account). The extension is inert on
//! native Windows (AU§0.2): there every folder is a run folder, and these
//! rules are kept, pure and tested, for the setups they describe.

use crate::core::paths::Paths;
use crate::model::{FolderFacts, FolderKind, FolderSnapshot, ParallelProfilesManifest};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// `~\.claude-windows`: the extension's VS Code window copies.
pub const WINDOWS_FOLDER_NAME: &str = ".claude-windows";
/// `~\.claude-shared`: the shared history, linked into every folder.
pub const SHARED_FOLDER_NAME: &str = ".claude-shared";
/// Marks an account store the extension made.
pub const STORE_MARKER_NAME: &str = ".parallel-accounts-store";
/// `~\.claude-windows\.manifest.json`.
pub const MANIFEST_NAME: &str = ".manifest.json";
/// Folders whose links make another folder the shared history.
pub const SHARED_ENTRY_NAMES: [&str; 2] = ["projects", "sessions"];

pub fn windows_root(paths: &Paths) -> String {
    paths.join(paths.home(), WINDOWS_FOLDER_NAME)
}

pub fn shared_store(paths: &Paths) -> String {
    paths.join(paths.home(), SHARED_FOLDER_NAME)
}

pub fn manifest_path(paths: &Paths) -> String {
    paths.join(&windows_root(paths), MANIFEST_NAME)
}

/// A VS Code window's working copy: a folder right inside
/// `~\.claude-windows` whose name doesn't start with a dot.
pub fn is_window_dir(paths: &Paths, path: &str) -> bool {
    let Some(name) = paths.file_name(path) else {
        return false;
    };
    let Some(parent) = paths.parent(path) else {
        return false;
    };
    paths.same(&parent, &windows_root(paths)) && !name.starts_with('.') && !name.is_empty()
}

/// The extension's manifest: only its folder lists, each normalized.
/// `None` when the bytes aren't a JSON object.
pub fn parse_manifest(bytes: &[u8], paths: &Paths) -> Option<ParallelProfilesManifest> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object()?;
    let list = |key: &str| -> Vec<String> {
        object
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| paths.normalize(s))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(ParallelProfilesManifest {
        stores: list("stores"),
        created: list("created"),
    })
}

/// The classification of every folder in a snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// By `Paths::key`: the folder as written, and its kind.
    kinds: BTreeMap<String, (String, FolderKind)>,
    /// The manifest or a store marker was found.
    pub extension_detected: bool,
    /// Run folders the extension adopted as account sources (in its
    /// manifest's `stores`, not made by it): the user's own profiles.
    adopted: BTreeMap<String, String>,
}

impl Layout {
    pub fn kind(&self, paths: &Paths, path: &str) -> Option<FolderKind> {
        self.kinds.get(&paths.key(path)).map(|(_, kind)| *kind)
    }

    fn with(&self, kind: FolderKind) -> Vec<String> {
        let mut dirs: Vec<String> = self
            .kinds
            .values()
            .filter(|(_, k)| *k == kind)
            .map(|(dir, _)| dir.clone())
            .collect();
        dirs.sort();
        dirs
    }

    pub fn run_dirs(&self) -> Vec<String> {
        self.with(FolderKind::Run)
    }

    pub fn stores(&self) -> Vec<String> {
        self.with(FolderKind::Store)
    }

    pub fn infrastructure(&self) -> Vec<String> {
        self.with(FolderKind::Infrastructure)
    }

    pub fn window_dirs(&self, paths: &Paths) -> Vec<String> {
        self.run_dirs()
            .into_iter()
            .filter(|dir| is_window_dir(paths, dir))
            .collect()
    }

    pub fn is_adopted(&self, paths: &Paths, path: &str) -> bool {
        self.adopted.contains_key(&paths.key(path))
    }

    pub fn adopted(&self) -> Vec<String> {
        self.adopted.values().cloned().collect()
    }

    /// Sealed fixtures: a layout written out by hand.
    pub fn from_kinds(
        paths: &Paths,
        kinds: &[(&str, FolderKind)],
        extension_detected: bool,
    ) -> Layout {
        Layout {
            kinds: kinds
                .iter()
                .map(|(dir, kind)| (paths.key(dir), (paths.normalize(dir), *kind)))
                .collect(),
            extension_detected,
            adopted: BTreeMap::new(),
        }
    }
}

/// Classifies every folder of `snapshot` (pure):
/// 1. `~\.claude-windows` and `~\.claude-shared` are infrastructure;
/// 2. a folder holding the store marker, or listed in the manifest's
///    `created`, is a store; one only in its `stores` is a profile it
///    adopted, which stays a run folder;
/// 3. a folder other folders' `projects`/`sessions` link into is
///    infrastructure, unless it is `~\.claude` or signed in itself;
/// 4. everything else is a run folder.
///
/// `previous_stores` are the stores of the last classification: while the
/// manifest can't be read (caught mid-write) they stay stores, so an
/// unreadable manifest never turns a store into a folder Claude Code could
/// be run in.
pub fn classify(snapshot: &FolderSnapshot, previous_stores: &[String], paths: &Paths) -> Layout {
    let home = paths.home();
    let default_dir = paths.default_config_dir();
    let fixed: BTreeSet<String> = [windows_root(paths), shared_store(paths)]
        .iter()
        .map(|dir| paths.key(dir))
        .collect();
    let keys = |list: Option<&Vec<String>>| -> BTreeSet<String> {
        list.map(|dirs| dirs.iter().map(|d| paths.key(d)).collect())
            .unwrap_or_default()
    };
    let created = keys(snapshot.manifest.as_ref().map(|m| &m.created));
    let listed = keys(snapshot.manifest.as_ref().map(|m| &m.stores));
    let sticky: BTreeSet<String> = if snapshot.manifest_unreadable {
        previous_stores.iter().map(|d| paths.key(d)).collect()
    } else {
        BTreeSet::new()
    };
    // Owners of link targets: `~\.claude-shared\projects` → `~\.claude-shared`.
    let mut link_owners = BTreeSet::new();
    for folder in &snapshot.folders {
        for target in &folder.link_targets {
            let normalized = paths.normalize(target);
            let is_shared_entry = paths.file_name(&normalized).is_some_and(|name| {
                SHARED_ENTRY_NAMES
                    .iter()
                    .any(|entry| paths.names_equal(&name, entry))
            });
            if !is_shared_entry {
                continue;
            }
            if let Some(owner) = paths.parent(&normalized) {
                if !paths.same(&owner, &folder.path) {
                    link_owners.insert(paths.key(&owner));
                }
            }
        }
    }

    let mut layout = Layout::default();
    let mut marker_found = false;
    for folder in &snapshot.folders {
        let key = paths.key(&folder.path);
        if paths.same(&folder.path, home) {
            continue;
        }
        if folder.has_store_marker {
            marker_found = true;
        }
        let kind = if fixed.contains(&key) {
            FolderKind::Infrastructure
        } else if folder.has_store_marker || created.contains(&key) || sticky.contains(&key) {
            FolderKind::Store
        } else if link_owners.contains(&key)
            && !paths.same(&folder.path, &default_dir)
            && !folder.is_signed_in
        {
            FolderKind::Infrastructure
        } else {
            if listed.contains(&key)
                && !paths.same(&folder.path, &default_dir)
                && !is_window_dir(paths, &folder.path)
            {
                layout
                    .adopted
                    .insert(key.clone(), paths.normalize(&folder.path));
            }
            FolderKind::Run
        };
        layout
            .kinds
            .insert(key, (paths.normalize(&folder.path), kind));
    }
    layout.extension_detected =
        snapshot.manifest.is_some() || snapshot.manifest_unreadable || marker_found;
    layout
}

/// Why a folder was suggested rather than added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionReason {
    /// Has Claude Code's folders but no login and no live session.
    Found,
    /// Named like a backup copy (`-backup`, `-old`, a date, …).
    LooksLikeBackup,
    /// Forgotten earlier, and a session ran there since.
    SeenAgain,
}

impl SuggestionReason {
    /// Settings' sentence for it.
    pub fn text(self) -> &'static str {
        match self {
            SuggestionReason::Found => {
                "Has Claude Code's folders, but nobody is signed in and no session runs there."
            }
            SuggestionReason::LooksLikeBackup => "Named like a backup copy.",
            SuggestionReason::SeenAgain => "You removed it, and a session ran there since.",
        }
    }
}

/// A folder that looks like a Claude config folder but wasn't added by
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSuggestion {
    pub config_dir: String,
    pub reason: SuggestionReason,
}

/// What a scan of the home folder found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    /// Add these without asking (run folders and stores), default first.
    pub accounts: Vec<String>,
    /// Offer these.
    pub suggestions: Vec<FolderSuggestion>,
    /// What every folder found is for.
    pub layout: Layout,
}

/// Discovery from a snapshot (pure): `~\.claude` when it exists; each
/// `~\.claude-*` or `~\.claude_*` folder that is signed in or has a live
/// session, unless named like a backup; `extra_dirs` that exist; window
/// copies once stocked; stores holding an account. Other folders with
/// Claude Code's markers become suggestions. Never the home folder, nor
/// anything holding it (also through a link).
pub fn discover(
    snapshot: &FolderSnapshot,
    extra_dirs: &[String],
    previous_stores: &[String],
    paths: &Paths,
) -> Discovery {
    let layout = classify(snapshot, previous_stores, paths);
    let default_dir = paths.default_config_dir();
    let mut discovery = Discovery::default();
    if find(snapshot, paths, &default_dir).is_some() {
        discovery.accounts.push(default_dir.clone());
    }
    let extras: BTreeSet<String> = extra_dirs.iter().map(|d| paths.key(d)).collect();
    let (mut others, mut windows, mut stores) = (Vec::new(), Vec::new(), Vec::new());
    for folder in &snapshot.folders {
        let path = paths.normalize(&folder.path);
        if paths.same(&path, &default_dir) {
            continue;
        }
        if !can_be_account(
            paths,
            &path,
            folder.canonical.as_deref(),
            snapshot.home_canonical.as_deref(),
        ) {
            continue;
        }
        match layout.kind(paths, &path) {
            None | Some(FolderKind::Infrastructure) => continue,
            Some(FolderKind::Store) => {
                // A store with an account in it; its identity is read.
                if folder.has_global_config {
                    stores.push(path);
                }
            }
            Some(FolderKind::Run) => {
                if is_window_dir(paths, &path) {
                    // A VS Code window's working copy, once it has been stocked.
                    if folder.has_global_config {
                        windows.push(path);
                    }
                    continue;
                }
                if extras.contains(&paths.key(&path)) {
                    others.push(path);
                    continue;
                }
                let Some(name) = paths.file_name(&path) else {
                    continue;
                };
                let in_home = paths
                    .parent(&path)
                    .is_some_and(|p| paths.same(&p, paths.home()));
                let is_home_lookalike = in_home
                    && (paths.name_has_prefix(&name, ".claude-")
                        || paths.name_has_prefix(&name, ".claude_"));
                // Folders already known (by hand, from a hook) are classified
                // but not rediscovered.
                if !is_home_lookalike {
                    continue;
                }
                let is_clearly_config_dir = folder.has_projects || folder.has_sessions;
                if !is_clearly_config_dir && !folder.has_global_config {
                    continue;
                }
                if looks_like_backup(&name) {
                    discovery.suggestions.push(FolderSuggestion {
                        config_dir: path,
                        reason: SuggestionReason::LooksLikeBackup,
                    });
                } else if folder.is_signed_in || folder.has_live_session {
                    others.push(path);
                } else {
                    discovery.suggestions.push(FolderSuggestion {
                        config_dir: path,
                        reason: SuggestionReason::Found,
                    });
                }
            }
        }
    }
    for group in [&mut others, &mut windows, &mut stores] {
        group.sort();
        discovery.accounts.append(group);
    }
    discovery.layout = layout;
    discovery
}

/// The snapshot's facts for a folder, by `Paths::key`.
pub fn find<'a>(
    snapshot: &'a FolderSnapshot,
    paths: &Paths,
    path: &str,
) -> Option<&'a FolderFacts> {
    let key = paths.key(path);
    snapshot.folders.iter().find(|f| paths.key(&f.path) == key)
}

/// Words a backup copy's folder name usually has: `backup`, `bak`, `old`,
/// `copy`, `orig`, `archive`, `tmp`, or a year or date.
pub fn looks_like_backup(folder_name: &str) -> bool {
    let lowered = folder_name.to_lowercase();
    let mut name = lowered.as_str();
    for prefix in [".claude-", ".claude_", "."] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest;
            break;
        }
    }
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    const BACKUP_WORDS: [&str; 16] = [
        "backup", "backups", "bak", "bkp", "old", "copy", "orig", "original", "archive",
        "archived", "tmp", "temp", "save", "saved", "prev", "previous",
    ];
    if words.iter().any(|w| BACKUP_WORDS.contains(w)) {
        return true;
    }
    if words
        .iter()
        .any(|w| w.starts_with("backup") || w.ends_with("backup") || w.ends_with("bak"))
    {
        return true;
    }
    // A year (2019…2099) or a yyyymmdd stamp.
    words.iter().any(|word| {
        if !word.chars().all(char::is_numeric) {
            return false;
        }
        match word.chars().count() {
            8 => true,
            4 => word
                .parse::<u32>()
                .is_ok_and(|year| (2019..=2099).contains(&year)),
            _ => false,
        }
    })
}

/// The home folder, and anything holding it, can never be an account: not
/// as written, and not through a link (`~\.claude-x` → `~`). The resolved
/// forms are compared when both are known.
pub fn can_be_account(
    paths: &Paths,
    dir: &str,
    dir_canonical: Option<&str>,
    home_canonical: Option<&str>,
) -> bool {
    let home = paths.home().to_owned();
    let as_written = !paths.same(dir, &home) && !paths.is_ancestor(dir, &home);
    let resolved = match (dir_canonical, home_canonical) {
        (Some(folder), Some(home)) => !paths.same(folder, home) && !paths.is_ancestor(folder, home),
        (Some(folder), None) => !paths.same(folder, &home) && !paths.is_ancestor(folder, &home),
        _ => true,
    };
    as_written && resolved
}
