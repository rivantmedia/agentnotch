//! Finding the Claude Code a usage check or a session summary runs
//! (ClaudeBinaryLocator.swift, §4.6). Looked up in this order: the binary
//! chosen in Settings, the one found earlier in this run, the native
//! installer's launcher, Bun's and Volta's, the npm and pnpm shims, then
//! `PATH` (`claude.exe` before `claude.cmd` in each folder).
//!
//! A copy that belongs to Claude Desktop is never used, from whichever of
//! those it came: Desktop's bundled Claude Code runs as Desktop's own
//! account whatever folder it is pointed at, and the `WindowsApps` aliases
//! start a packaged app, not a command-line tool. Asking it would put
//! another account's usage on the ring.
//!
//! Pure over `exists`: the caller says what is on disk, so the rules are
//! tested against temporary trees on every OS.

use crate::platform::Roots;
use crate::runtime_types::ClaudeBinary;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Where the npm package keeps its entry point, below the shim's folder.
const CLI_JS: [&str; 4] = ["node_modules", "@anthropic-ai", "claude-code", "cli.js"];

/// A path Claude Desktop (or a packaged app) owns: never run.
pub fn is_desktop_owned(path: &Path) -> bool {
    let folded = path.to_string_lossy().to_lowercase().replace('/', "\\");
    folded.contains("\\anthropicclaude\\")
        || folded.contains("\\claude\\claude-code\\")
        || folded.contains("\\windowsapps\\")
}

/// A batch shim (`claude.cmd`), which can't be started like an exe.
pub fn is_shim(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    })
}

/// `%APPDATA%` as far as the roots tell: the parent of upstream's config
/// folder (the Known Folder), then the default place under the profile.
fn roaming_folders(roots: &Roots) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(parent) = roots.data.parent().filter(|p| !p.as_os_str().is_empty()) {
        folders.push(parent.to_path_buf());
    }
    folders.push(roots.home.join("AppData").join("Roaming"));
    folders
}

/// `%LOCALAPPDATA%` as far as the roots tell: the folder that holds the
/// app's own `com.rivantmedia.agentnotch\Claude` (unless the support folder
/// was overridden), then the default place under the profile.
fn local_folders(roots: &Roots) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    let identifier_folder = roots.support.parent().filter(|folder| {
        folder
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(crate::core::roots::IDENTIFIER))
    });
    if let Some(parent) = identifier_folder
        .and_then(Path::parent)
        .filter(|p| !p.as_os_str().is_empty())
    {
        folders.push(parent.to_path_buf());
    }
    folders.push(roots.home.join("AppData").join("Local"));
    folders
}

/// The well-known install locations, in order (§4.6).
pub fn fixed_candidates(roots: &Roots) -> Vec<PathBuf> {
    let home = &roots.home;
    let mut candidates = vec![
        // The native installer's launcher.
        home.join(".local").join("bin").join("claude.exe"),
        home.join(".bun").join("bin").join("claude.exe"),
        home.join(".volta").join("bin").join("claude.exe"),
    ];
    for roaming in roaming_folders(roots) {
        candidates.push(roaming.join("npm").join("claude.cmd"));
    }
    for local in local_folders(roots) {
        candidates.push(local.join("pnpm").join("claude.cmd"));
    }
    candidates
}

/// `claude.exe`, then `claude.cmd`, in each folder of `PATH`.
pub fn path_candidates(env_path: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(env_path)
        .filter(|folder| !folder.as_os_str().is_empty())
        .flat_map(|folder| [folder.join("claude.exe"), folder.join("claude.cmd")])
        .collect()
}

/// Every place Claude Code is looked for, in order, without the ones Claude
/// Desktop owns and without repeats.
pub fn candidates(
    roots: &Roots,
    settings_choice: Option<&Path>,
    remembered: Option<&Path>,
    env_path: &OsStr,
) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = Vec::new();
    all.extend(settings_choice.map(Path::to_path_buf));
    all.extend(remembered.map(Path::to_path_buf));
    all.extend(fixed_candidates(roots));
    all.extend(path_candidates(env_path));

    let mut seen: Vec<String> = Vec::new();
    all.retain(|path| {
        if path.as_os_str().is_empty() || is_desktop_owned(path) {
            return false;
        }
        let key = path.to_string_lossy().to_lowercase();
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
    all
}

/// Every Claude Code installed in those places (the hook installer writes
/// for the oldest of them, §4.3).
pub fn installed(
    roots: &Roots,
    settings_choice: Option<&Path>,
    env_path: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    candidates(roots, settings_choice, None, env_path)
        .into_iter()
        .filter(|path| exists(path))
        .collect()
}

/// The Claude Code to run, or `None` when there is none.
pub fn locate_claude(
    roots: &Roots,
    settings_choice: Option<&Path>,
    env_path: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<ClaudeBinary> {
    locate_claude_from(roots, settings_choice, None, env_path, exists)
}

/// [`locate_claude`] with the path found earlier in this run, tried right
/// after the Settings choice so an install that hasn't moved is one check.
pub fn locate_claude_from(
    roots: &Roots,
    settings_choice: Option<&Path>,
    remembered: Option<&Path>,
    env_path: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<ClaudeBinary> {
    candidates(roots, settings_choice, remembered, env_path)
        .into_iter()
        .find(|path| exists(path))
        .map(|path| binary_for(&path, env_path, exists))
}

/// How the file at `path` is run. An npm shim becomes Node with the
/// package's `cli.js` when both are there: a batch file goes through
/// `cmd.exe`, whose quoting Node's own command line avoids. Otherwise the
/// shim runs as it is (`shim: true`).
pub fn binary_for(path: &Path, env_path: &OsStr, exists: &dyn Fn(&Path) -> bool) -> ClaudeBinary {
    let plain = |shim: bool| ClaudeBinary {
        program: path.to_path_buf(),
        prefix_args: Vec::new(),
        version: None,
        shim,
    };
    if !is_shim(path) {
        return plain(false);
    }
    let folder = path.parent().unwrap_or(Path::new(""));
    let cli = CLI_JS
        .iter()
        .fold(folder.to_path_buf(), |path, part| path.join(part));
    if exists(&cli) {
        if let Some(node) = node_for(folder, env_path, exists) {
            return ClaudeBinary {
                program: node,
                prefix_args: vec![cli.into_os_string()],
                version: None,
                shim: false,
            };
        }
    }
    plain(true)
}

/// The Node the shim itself would run: `node.exe` beside it, else the first
/// on `PATH`.
fn node_for(folder: &Path, env_path: &OsStr, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    std::iter::once(folder.to_path_buf())
        .chain(std::env::split_paths(env_path))
        .filter(|folder| !folder.as_os_str().is_empty())
        .map(|folder| folder.join("node.exe"))
        .find(|node| !is_desktop_owned(node) && exists(node))
}
