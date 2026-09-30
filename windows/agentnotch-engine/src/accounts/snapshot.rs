//! Reading the home folder for discovery (`ConfigDirClassifier.readSnapshot`,
//! AU§2.6), and looking at one folder the user picked. Both are read-only:
//! directory listings, link targets, the extension's manifest, whether the
//! store marker exists, and the login of each `.claude.json` (two of its
//! keys; never a credential, never a session file's contents).
//!
//! Everything the registry needs from the disk comes back in the
//! [`FolderSnapshot`], so the registry itself never blocks: the hub runs
//! [`read_folder_snapshot`] as a job on its file lanes and hands the result
//! to `AccountRegistry::discover`.

use super::classify::{self, SHARED_ENTRY_NAMES, STORE_MARKER_NAME};
use crate::core::claude_json::{self, ClaudeJsonReader};
use crate::core::paths::Paths;
use crate::model::{ConfigRead, FolderFacts, FolderSnapshot};
use crate::platform::{Liveness, Processes, Roots, SecureFiles};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One read of the home folder for discovery (a job on `an-io`): `~\.claude`,
/// every `~\.claude-*` / `~\.claude_*` folder, every VS Code window's working
/// copy, the manifest's stores, and `explicit` (folders already known: the
/// registry's, `AGENTNOTCH_EXTRA_CONFIG_DIRS`, the last classification's
/// stores).
pub fn read_folder_snapshot(
    roots: &Roots,
    explicit: &[PathBuf],
    files: &dyn SecureFiles,
    procs: &dyn Processes,
) -> FolderSnapshot {
    let explicit: Vec<String> = explicit
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    read_snapshot(
        &roots.paths(),
        &explicit,
        files,
        procs,
        ClaudeJsonReader::shared(),
    )
}

/// [`read_folder_snapshot`] with its path rules and `.claude.json` reader
/// given (tests keep their own reader, so no parse outlives them).
pub fn read_snapshot(
    paths: &Paths,
    explicit: &[String],
    files: &dyn SecureFiles,
    procs: &dyn Processes,
    reader: &ClaudeJsonReader,
) -> FolderSnapshot {
    let home = paths.home().to_owned();
    let mut found: Vec<String> = Vec::new();
    let add = |found: &mut Vec<String>, path: &str| {
        let normalized = paths.normalize(path);
        if !found.iter().any(|known| paths.same(known, &normalized)) {
            found.push(normalized);
        }
    };

    let default_dir = paths.default_config_dir();
    if is_dir(&default_dir) {
        add(&mut found, &default_dir);
    }
    for name in child_names(&home) {
        if paths.name_has_prefix(&name, ".claude-") || paths.name_has_prefix(&name, ".claude_") {
            let path = paths.join(&home, &name);
            if is_dir(&path) {
                add(&mut found, &path);
            }
        }
    }
    let windows_root = classify::windows_root(paths);
    for name in child_names(&windows_root) {
        if !name.starts_with('.') {
            let path = paths.join(&windows_root, &name);
            if is_dir(&path) {
                add(&mut found, &path);
            }
        }
    }

    // The extension writes its manifest in place, so a read can catch it
    // half-written: that is "unreadable", never "no manifest".
    let manifest_path = classify::manifest_path(paths);
    let manifest_bytes = std::fs::read(&manifest_path).ok();
    let manifest = manifest_bytes
        .as_deref()
        .and_then(|bytes| classify::parse_manifest(bytes, paths));
    let manifest_unreadable = manifest.is_none() && Path::new(&manifest_path).exists();
    for store in manifest.iter().flat_map(|m| &m.stores) {
        if is_dir(store) {
            add(&mut found, store);
        }
    }

    let requested: Vec<String> = {
        let mut requested: Vec<String> = Vec::new();
        for dir in explicit {
            let normalized = paths.normalize(dir);
            if !normalized.is_empty() && !requested.iter().any(|r| paths.same(r, &normalized)) {
                requested.push(normalized);
            }
        }
        requested
    };
    for dir in &requested {
        if is_dir(dir) {
            add(&mut found, dir);
        }
    }

    let folders = found
        .iter()
        .map(|path| {
            let mut facts = folder_facts(paths, path, files, procs, reader);
            facts.is_explicit = requested.iter().any(|r| paths.same(r, path));
            facts
        })
        .collect();

    FolderSnapshot {
        home_config: read_config(reader, &paths.default_identity_file()),
        home_canonical: canonical(paths, files, &home),
        home,
        folders,
        manifest,
        manifest_unreadable,
        requested,
    }
}

/// One folder's facts.
pub fn folder_facts(
    paths: &Paths,
    path: &str,
    files: &dyn SecureFiles,
    procs: &dyn Processes,
    reader: &ClaudeJsonReader,
) -> FolderFacts {
    let path = paths.normalize(path);
    let global_config = paths.join(&path, ".claude.json");
    let has_global_config = Path::new(&global_config).exists();
    // The default folder's login is in `~\.claude.json`, beside it.
    let identity_file = if paths.is_default_config_dir(&path) {
        paths.default_identity_file()
    } else {
        global_config.clone()
    };
    let is_signed_in = std::fs::read(&identity_file)
        .map(|bytes| claude_json::has_login(&bytes))
        .unwrap_or(false);

    let mut link_targets = Vec::new();
    let mut sessions_are_shared = false;
    for name in SHARED_ENTRY_NAMES {
        let entry = paths.join(&path, name);
        // A symbolic link or (on Windows) a junction; anything else errs.
        let Ok(destination) = std::fs::read_link(&entry) else {
            continue;
        };
        // A relative target is relative to the folder holding the link.
        link_targets.push(paths.resolve(&path, &destination.to_string_lossy()));
        if name == "sessions" {
            sessions_are_shared = true;
        }
    }
    let has_sessions = is_dir(&paths.join(&path, "sessions"));
    // A linked `sessions` holds every folder's sessions: it says nothing
    // about this one.
    let has_live_session =
        has_sessions && !sessions_are_shared && has_live_session(paths, &path, procs);

    FolderFacts {
        has_global_config,
        is_signed_in,
        has_store_marker: Path::new(&paths.join(&path, STORE_MARKER_NAME)).exists(),
        has_projects: is_dir(&paths.join(&path, "projects")),
        has_sessions,
        has_live_session,
        link_targets,
        is_explicit: false,
        own_config: if has_global_config {
            read_config(reader, &global_config)
        } else {
            None
        },
        canonical: canonical(paths, files, &path),
        path,
    }
}

/// `sessions\` holds a `<pid>.json` whose process runs. The file is never
/// opened, and the `.key` files beside it are never touched.
pub fn has_live_session(paths: &Paths, config_dir: &str, procs: &dyn Processes) -> bool {
    child_names(&paths.join(config_dir, "sessions"))
        .iter()
        .filter_map(|name| name.strip_suffix(".json"))
        .filter_map(|stem| stem.parse::<i32>().ok())
        .filter(|pid| *pid > 0)
        .any(|pid| procs.liveness(pid as u32) == Liveness::Alive)
}

/// Who a `.claude.json` names, when the file exists and has ever parsed.
fn read_config(reader: &ClaudeJsonReader, path: &str) -> Option<ConfigRead> {
    reader.read(Path::new(path)).map(|config| ConfigRead {
        identity: config.identity,
        modified_at: config.modified_at,
    })
}

fn canonical(paths: &Paths, files: &dyn SecureFiles, path: &str) -> Option<String> {
    files
        .canonical(Path::new(path))
        .ok()
        .map(|resolved| paths.normalize(&resolved.to_string_lossy()))
}

/// Follows links, as the OS does when Claude Code opens the folder.
fn is_dir(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// The names in a folder, sorted; none when it can't be listed. A name that
/// isn't Unicode can't be one of Claude Code's and is left out.
fn child_names(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// What a folder holds, for the "doesn't look like a Claude Code folder"
/// question.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderMarkers {
    pub has_projects: bool,
    pub has_sessions: bool,
    pub has_global_config: bool,
    pub is_signed_in: bool,
}

impl FolderMarkers {
    /// Session history or a session registry: clearly a config folder, no
    /// need to ask. A `.claude.json` alone could be anything (home has one).
    pub fn is_clearly_config_dir(&self) -> bool {
        self.has_projects || self.has_sessions
    }

    pub fn of(facts: &FolderFacts) -> FolderMarkers {
        FolderMarkers {
            has_projects: facts.has_projects,
            has_sessions: facts.has_sessions,
            has_global_config: facts.has_global_config,
            is_signed_in: facts.is_signed_in,
        }
    }
}

/// What is at a path the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Missing,
    /// A file, not a folder.
    NotAFolder,
    Folder(Box<PickedFolder>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedFolder {
    pub facts: FolderFacts,
    /// `settings.json` exists: with Claude Code's own markers, the sign of a
    /// config folder ("New account…" adopts only those).
    pub has_settings: bool,
    /// The home folder with its links resolved, when it could be.
    pub home_canonical: Option<String>,
}

/// The disk, as the registry asks of it when the user adds or creates a
/// folder: the only moments it can't wait for a snapshot, because the answer
/// ("That folder doesn't exist.") goes straight back to Settings. Tests of
/// the Windows rules put a map behind it.
pub trait FolderProbe: Send + Sync {
    fn look(&self, dir: &str) -> Picked;
    /// Makes the folder (and what is missing above it).
    fn create_dir(&self, dir: &str) -> Result<(), String>;
}

/// [`FolderProbe`] over the real disk.
pub struct DiskProbe {
    paths: Paths,
    files: Arc<dyn SecureFiles>,
    processes: Arc<dyn Processes>,
    reader: Arc<ClaudeJsonReader>,
}

impl DiskProbe {
    /// Reads `.claude.json` files through a reader of its own.
    pub fn new(paths: Paths, files: Arc<dyn SecureFiles>, processes: Arc<dyn Processes>) -> Self {
        DiskProbe {
            paths,
            files,
            processes,
            reader: Arc::new(ClaudeJsonReader::new()),
        }
    }
}

impl FolderProbe for DiskProbe {
    fn look(&self, dir: &str) -> Picked {
        let path = self.paths.normalize(dir);
        let Ok(meta) = std::fs::metadata(&path) else {
            return Picked::Missing;
        };
        if !meta.is_dir() {
            return Picked::NotAFolder;
        }
        Picked::Folder(Box::new(PickedFolder {
            facts: folder_facts(
                &self.paths,
                &path,
                self.files.as_ref(),
                self.processes.as_ref(),
                &self.reader,
            ),
            has_settings: Path::new(&self.paths.join(&path, "settings.json")).exists(),
            home_canonical: canonical(&self.paths, self.files.as_ref(), self.paths.home()),
        }))
    }

    fn create_dir(&self, dir: &str) -> Result<(), String> {
        let path = PathBuf::from(self.paths.normalize(dir));
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        // The user's profile already keeps others out on Windows (the folder
        // inherits its ACL); elsewhere the folder is made the owner's alone,
        // as the Mac makes it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
