//! The hook exe's copy in a run folder: `<cfg>\hooks\agentnotch-hook.exe`
//! (DESIGN-WIN §4.3). Claude Code may be running the copy at any moment, and
//! a hook that fails to start must stay the rare exception, so the copy is
//! replaced in an order that never leaves the folder without an exe: stage
//! beside it, verify, one rename over it, and only when that is refused (a
//! running image can't be replaced, but it can be renamed) move the current
//! copy aside and the stage into place at once.
//!
//! Plain `std::fs` throughout: `std::fs::rename` replaces atomically on
//! every OS the engine runs on, and the copy needs no private security (it
//! holds no user data, and Claude Code must be able to run it).

use super::commands::{hook_copy_path, HOOK_EXE_NAME};
use crate::platform::Clock;
use chrono::{DateTime, Local};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// `agentnotch-hook.new-<yyyyMMddHHmmss>.exe`: a copy being written.
pub const STAGE_MARK: &str = "new";
/// `agentnotch-hook.old-<yyyyMMddHHmmss>.exe`: a copy that was running when
/// it was replaced or removed.
pub const ASIDE_MARK: &str = "old";

/// How many later seconds are tried for a stage or set-aside name that is
/// taken (an earlier copy still running under it).
const NAME_TRIES: u64 = 120;

/// The file operations a replacement is made of. Injected so the branch
/// Windows takes for a running image can be tested anywhere.
pub struct FileOps<'a> {
    pub rename: &'a (dyn Fn(&Path, &Path) -> io::Result<()> + 'a),
    pub remove: &'a (dyn Fn(&Path) -> io::Result<()> + 'a),
}

impl FileOps<'static> {
    /// The real ones.
    pub const STD: FileOps<'static> = FileOps {
        rename: &|from, to| fs::rename(from, to),
        remove: &|path| fs::remove_file(path),
    };
}

/// What installing the copy did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    /// Same size and SHA-256 already: nothing written.
    AlreadyCurrent,
    /// Written, or renamed over the copy that was there.
    Written,
    /// The copy in place was running: it was renamed to `aside` and the new
    /// one put in its place. A later sweep deletes `aside`.
    ReplacedRunning { aside: PathBuf },
}

/// Why the copy isn't there. Whatever was in the folder is as it was.
#[derive(Debug)]
pub enum CopyError {
    /// The exe to copy from can't be read (a dev run without it, say).
    SourceMissing(io::Error),
    /// The `hooks` folder or the stage couldn't be written.
    Stage(io::Error),
    /// The stage didn't read back as the source.
    Corrupt,
    /// Neither rename was allowed.
    Replace(io::Error),
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::SourceMissing(_) => write!(f, "{HOOK_EXE_NAME} is not available"),
            CopyError::Stage(error) | CopyError::Replace(error) => {
                write!(f, "Couldn't write {HOOK_EXE_NAME}: {error}")
            }
            CopyError::Corrupt => write!(
                f,
                "Couldn't write {HOOK_EXE_NAME}: the copy didn't read back the same"
            ),
        }
    }
}

impl std::error::Error for CopyError {}

/// `yyyyMMddHHmmss` in local time, as the names beside the copy carry it.
pub fn stamp(at: SystemTime) -> String {
    DateTime::<Local>::from(at)
        .format("%Y%m%d%H%M%S")
        .to_string()
}

/// `agentnotch-hook.<mark>-<stamp>.exe`.
pub fn marked_name(mark: &str, at: SystemTime) -> String {
    let stem = HOOK_EXE_NAME.strip_suffix(".exe").unwrap_or(HOOK_EXE_NAME);
    format!("{stem}.{mark}-{}.exe", stamp(at))
}

/// Whether `name` is one of the copy's leftovers: a stage or a set-aside copy.
pub fn is_leftover(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let stem = HOOK_EXE_NAME.strip_suffix(".exe").unwrap_or(HOOK_EXE_NAME);
    [STAGE_MARK, ASIDE_MARK].iter().any(|mark| {
        name.strip_prefix(&format!("{stem}.{mark}-"))
            .and_then(|rest| rest.strip_suffix(".exe"))
            .is_some_and(|stamp| !stamp.is_empty())
    })
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A name for `mark` that nothing in `dir` has: this second's, else a later
/// one's (an earlier leftover of this very second may still be running).
fn free_name(dir: &Path, mark: &str, now: SystemTime) -> PathBuf {
    (0..NAME_TRIES)
        .map(|bump| dir.join(marked_name(mark, now + Duration::from_secs(bump))))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .unwrap_or_else(|| dir.join(marked_name(mark, now)))
}

/// Deletes every stage and set-aside copy in `config_dir`'s `hooks` folder,
/// and nothing else. One that is still running can't be deleted: it stays for
/// a later pass. Returns how many went.
pub fn sweep(config_dir: &Path) -> usize {
    sweep_with(config_dir, &FileOps::STD)
}

pub fn sweep_with(config_dir: &Path, ops: &FileOps) -> usize {
    let Some(dir) = hook_copy_path(config_dir).parent().map(Path::to_path_buf) else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(is_leftover))
        // Never a folder or a link someone gave one of these names.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| (ops.remove)(&entry.path()).is_ok())
        .count()
}

/// Whether the copy in place is `source`'s bytes (size, then SHA-256).
fn is_current(destination: &Path, source: &[u8]) -> bool {
    let same_size = fs::metadata(destination)
        .is_ok_and(|meta| meta.is_file() && meta.len() == source.len() as u64);
    same_size && fs::read(destination).is_ok_and(|bytes| sha256(&bytes) == sha256(source))
}

/// Writes `bytes` to a new file at `stage` and proves it reads back the same.
fn write_stage(stage: &Path, bytes: &[u8], source: &Path) -> Result<(), CopyError> {
    let written = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // Where running a file takes a permission bit, the copy has the
        // source's. Windows has no such bit, and copying its attributes
        // would carry a read-only flag onto a file we must replace later.
        #[cfg(unix)]
        fs::set_permissions(stage, fs::metadata(source)?.permissions())?;
        #[cfg(not(unix))]
        let _ = source;
        Ok(())
    })();
    if let Err(error) = written {
        // Only what this call made: a stage someone else holds stays theirs.
        if error.kind() != io::ErrorKind::AlreadyExists {
            let _ = fs::remove_file(stage);
        }
        return Err(CopyError::Stage(error));
    }
    match fs::read(stage) {
        Ok(read) if sha256(&read) == sha256(bytes) => Ok(()),
        Ok(_) => {
            let _ = fs::remove_file(stage);
            Err(CopyError::Corrupt)
        }
        Err(error) => {
            let _ = fs::remove_file(stage);
            Err(CopyError::Stage(error))
        }
    }
}

/// Puts `source` (the exe beside the app) at `<config_dir>\hooks\` when the
/// copy there differs, after sweeping earlier leftovers. Call it before
/// settings.json is pointed at the copy; on an error settings.json must not be.
pub fn install_hook_copy(
    source: &Path,
    config_dir: &Path,
    clock: &dyn Clock,
) -> Result<CopyOutcome, CopyError> {
    install_hook_copy_with(source, config_dir, clock, &FileOps::STD)
}

pub fn install_hook_copy_with(
    source: &Path,
    config_dir: &Path,
    clock: &dyn Clock,
    ops: &FileOps,
) -> Result<CopyOutcome, CopyError> {
    // First, and before anything is touched: no source, no change at all.
    let bytes = fs::read(source).map_err(CopyError::SourceMissing)?;
    let destination = hook_copy_path(config_dir);
    let dir = destination
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_dir.to_path_buf());

    sweep_with(config_dir, ops);
    if is_current(&destination, &bytes) {
        return Ok(CopyOutcome::AlreadyCurrent);
    }

    fs::create_dir_all(&dir).map_err(CopyError::Stage)?;
    let now = clock.now();
    let stage = free_name(&dir, STAGE_MARK, now);
    write_stage(&stage, &bytes, source)?;

    let refused = match (ops.rename)(&stage, &destination) {
        Ok(()) => return Ok(CopyOutcome::Written),
        Err(error) => error,
    };
    // Refused with nothing in the way: moving a copy aside can't help.
    if fs::symlink_metadata(&destination).is_err() {
        let _ = (ops.remove)(&stage);
        return Err(CopyError::Replace(refused));
    }

    // A hook is running the copy. Nothing between these two renames: a hook
    // spawned in that gap fails to start, which Claude Code doesn't block on.
    let aside = free_name(&dir, ASIDE_MARK, now);
    if let Err(error) = (ops.rename)(&destination, &aside) {
        let _ = (ops.remove)(&stage);
        return Err(CopyError::Replace(error));
    }
    if let Err(error) = (ops.rename)(&stage, &destination) {
        // Put the old copy back rather than leave the folder without one.
        let _ = (ops.rename)(&aside, &destination);
        let _ = (ops.remove)(&stage);
        return Err(CopyError::Replace(error));
    }
    Ok(CopyOutcome::ReplacedRunning { aside })
}

/// What removing the copy did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    /// There was none.
    Absent,
    Deleted,
    /// It was running: renamed to `aside`, for a later sweep (or the
    /// uninstaller) to delete.
    SetAside {
        aside: PathBuf,
    },
}

/// Removes the copy and its leftovers (hooks turned off, uninstall). A copy a
/// hook is running can't be deleted; renamed aside, no settings.json command
/// can start it again. The error is the rename's when even that is refused.
pub fn remove_hook_copy(config_dir: &Path, clock: &dyn Clock) -> io::Result<RemoveOutcome> {
    remove_hook_copy_with(config_dir, clock, &FileOps::STD)
}

pub fn remove_hook_copy_with(
    config_dir: &Path,
    clock: &dyn Clock,
    ops: &FileOps,
) -> io::Result<RemoveOutcome> {
    sweep_with(config_dir, ops);
    let destination = hook_copy_path(config_dir);
    if fs::symlink_metadata(&destination).is_err() {
        return Ok(RemoveOutcome::Absent);
    }
    match (ops.remove)(&destination) {
        Ok(()) => return Ok(RemoveOutcome::Deleted),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(RemoveOutcome::Absent),
        Err(_) => {}
    }
    let dir = destination
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_dir.to_path_buf());
    let aside = free_name(&dir, ASIDE_MARK, clock.now());
    (ops.rename)(&destination, &aside)?;
    Ok(RemoveOutcome::SetAside { aside })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time;

    #[test]
    fn leftover_names() {
        let at = time::from_ms(1_790_000_000_000);
        let stage = marked_name(STAGE_MARK, at);
        let aside = marked_name(ASIDE_MARK, at);
        assert!(stage.starts_with("agentnotch-hook.new-") && stage.ends_with(".exe"));
        assert_eq!(
            stage.len(),
            "agentnotch-hook.new-".len() + 14 + ".exe".len()
        );
        assert!(is_leftover(&stage));
        assert!(is_leftover(&aside));
        assert!(is_leftover("AgentNotch-Hook.OLD-20260930101500.EXE"));
        for other in [
            HOOK_EXE_NAME,
            "agentnotch-hook.new-.exe",
            "agentnotch-hook.new-20260930101500.txt",
            "agentnotch-hook.older.exe",
            "agentnotch-statusline.previous.json",
            "someone-else.old-20260930101500.exe",
        ] {
            assert!(!is_leftover(other), "{other}");
        }
    }
}
