//! Which Claude Code versions are on this PC (§4.3's version sources that
//! need a look at the disk or a child process; `Job::Versions`):
//!
//! - `claude --version` of every installed binary (`locator::installed`);
//! - the folder names of bundled copies the app never runs: the VS Code
//!   family's `extensions\anthropic.claude-code-<ver>-*`, and Claude
//!   Desktop's `claude-code\<ver>`.
//!
//! The hook installer writes the command form every one of them reads, so a
//! copy whose version can't be told is reported as unknown (`version:
//! None`), never left out.

use crate::platform::{CommandRunner, CommandSpec, Exit, Roots};
use crate::runtime_types::{VersionSighting, VersionSource};
use crate::usage::env::scrubbed_env;
use crate::usage::locator::binary_for;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// The profile folders of the VS Code family, each with an `extensions`
/// folder.
const EDITOR_FOLDERS: [&str; 4] = [".vscode", ".vscode-insiders", ".cursor", ".windsurf"];
/// How Claude Code's extension folders start.
const EXTENSION_PREFIX: &str = "anthropic.claude-code-";
/// Claude Desktop keeps its bundled copies in `<userData>\claude-code\<ver>`.
const DESKTOP_FOLDER: &str = "claude-code";

/// How long one `claude --version` may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// More output than this isn't a version.
const MAX_VERSION_OUTPUT: u64 = 64 * 1024;

/// The first `X.Y.Z` in `text` ("2.1.88", "v2.1.88", "2.1.280 (Claude Code)").
pub fn parse_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    (0..bytes.len())
        // Where a run of digits starts.
        .filter(|&at| bytes[at].is_ascii_digit() && (at == 0 || !bytes[at - 1].is_ascii_digit()))
        .find_map(|at| leading_version(&text[at..]))
}

/// The `X.Y.Z` that `text` starts with, without leading zeros.
fn leading_version(text: &str) -> Option<String> {
    let mut parts = [0u64; 3];
    let mut rest = text;
    for (index, part) in parts.iter_mut().enumerate() {
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        *part = rest[..end].parse().ok()?;
        rest = &rest[end..];
        if index < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    Some(format!("{}.{}.{}", parts[0], parts[1], parts[2]))
}

/// The folders bundled copies live in: each editor's `extensions` and each
/// Claude Desktop root's `claude-code`. Missing ones are fine.
pub fn bundled_roots(roots: &Roots) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = EDITOR_FOLDERS
        .iter()
        .map(|editor| roots.home.join(editor).join("extensions"))
        .collect();
    folders.extend(
        roots
            .claude_desktop
            .iter()
            .map(|root| root.join(DESKTOP_FOLDER)),
    );
    folders
}

/// The bundled copies found in `folders`, by folder name only: nothing in
/// them is opened or run.
pub fn bundled_versions(folders: &[PathBuf]) -> Vec<VersionSighting> {
    let mut sightings = Vec::new();
    for folder in folders {
        let desktop = folder
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(DESKTOP_FOLDER));
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .collect();
        found.sort();
        for (name, path) in found {
            let version = if desktop {
                leading_version(&name)
            } else {
                match strip_prefix_ignore_case(&name, EXTENSION_PREFIX) {
                    Some(rest) => leading_version(rest),
                    // Another extension.
                    None => continue,
                }
            };
            sightings.push(VersionSighting {
                source: if desktop {
                    VersionSource::BundledDesktop
                } else {
                    VersionSource::BundledVsCode
                },
                path: Some(path),
                version,
            });
        }
    }
    sightings
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// `claude --version` of the binary at `path`: `None` on any failure (the
/// caller then treats the copy as unknown).
pub fn binary_version(
    path: &Path,
    runner: &dyn CommandRunner,
    base_env: &[(OsString, OsString)],
    env_path: &OsStr,
    timeout: Duration,
) -> Option<String> {
    let binary = binary_for(path, env_path, &|candidate| candidate.is_file());
    let folder = binary
        .program
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let mut args = binary.prefix_args.clone();
    args.push("--version".into());
    let mut child = runner
        .spawn(CommandSpec {
            program: binary.program.clone(),
            args,
            env: scrubbed_env(base_env, None, &folder),
            cwd: folder,
        })
        .ok()?;
    // Nothing is asked on stdin.
    drop(child.take_stdin());
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = child.take_stdout() {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stdout.take(MAX_VERSION_OUTPUT).read_to_end(&mut text);
            let _ = sender.send(text);
        });
    }
    let exit = child.wait_timeout(timeout).ok().flatten();
    if exit.is_none() {
        child.kill_tree();
        let _ = child.wait_timeout(Duration::from_secs(1));
    }
    let output = receiver.recv_timeout(Duration::from_secs(1)).ok()?;
    match exit {
        Some(Exit::Code(0)) => parse_version(&String::from_utf8_lossy(&output)),
        _ => None,
    }
}

/// `Job::Versions`: the version of every binary in `binaries` (its
/// `--version`) and of every bundled copy under `bundled` (its folder name).
pub fn read_versions(
    binaries: &[PathBuf],
    bundled: &[PathBuf],
    runner: &dyn CommandRunner,
    base_env: &[(OsString, OsString)],
) -> Vec<VersionSighting> {
    let env_path = base_env
        .iter()
        .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let mut sightings: Vec<VersionSighting> = binaries
        .iter()
        .map(|path| VersionSighting {
            source: VersionSource::Binary,
            path: Some(path.clone()),
            version: binary_version(path, runner, base_env, &env_path, VERSION_TIMEOUT),
        })
        .collect();
    sightings.extend(bundled_versions(bundled));
    sightings
}
