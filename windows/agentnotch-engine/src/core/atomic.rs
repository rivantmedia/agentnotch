//! [`StdSecureFiles`]: `SecureFiles` over the standard library, for tests on
//! every OS and for the engine's own unit tests. The real Windows
//! implementation (protected DACLs, `FileRenameInfoEx`, file ids, reparse
//! points) is `agentnotch-win::files`; this one gives the same guarantees
//! with POSIX means where it can: owner-only modes (0700/0600), `O_EXCL`
//! stages, fsync, one `rename(2)`, identities from dev/inode.

use crate::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct StdSecureFiles;

/// `.<name>.agentnotch-<8 hex>.tmp` beside `path`.
pub fn stage_path(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a file path is needed"))?
        .to_string_lossy()
        .into_owned();
    let mut random = [0u8; 4];
    getrandom::getrandom(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
    Ok(path.with_file_name(format!(".{name}.agentnotch-{hex}.tmp")))
}

fn open_new(path: &Path, private: bool, like: Option<&fs::Metadata>) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if private {
            options.mode(0o600);
        } else if let Some(meta) = like {
            options.mode(meta.permissions().mode() & 0o7777);
        }
    }
    #[cfg(not(unix))]
    let _ = (private, like);
    let file = options.open(path)?;
    // The umask may have filtered the mode: set it exactly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if private {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        } else if let Some(meta) = like {
            file.set_permissions(fs::Permissions::from_mode(
                meta.permissions().mode() & 0o7777,
            ))?;
        }
    }
    Ok(file)
}

fn identity_of(meta: &fs::Metadata) -> FileIdentity {
    let modified_ns = meta.modified().map(crate::core::time::to_ns).unwrap_or(0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            volume: meta.dev(),
            index: u128::from(meta.ino()),
            modified_ns,
            size: meta.len(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            volume: 0,
            index: 0,
            modified_ns,
            size: meta.len(),
        }
    }
}

/// Whether `path` is still what `expect` says. `Some(result)` when it isn't.
fn check(path: &Path, expect: Expect) -> io::Result<Option<WriteResult>> {
    let current = match fs::metadata(path) {
        Ok(meta) => Some(identity_of(&meta)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    Ok(match (expect, current) {
        (Expect::Nothing, _) => None,
        (Expect::Absent, None) => None,
        (Expect::Absent, Some(_)) => Some(WriteResult::Changed),
        (Expect::Same(_), None) => Some(WriteResult::Vanished),
        (Expect::Same(expected), Some(found)) if found == expected => None,
        (Expect::Same(_), Some(_)) => Some(WriteResult::Changed),
    })
}

impl SecureFiles for StdSecureFiles {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        let target = fs::metadata(path).ok();
        if target
            .as_ref()
            .is_some_and(|meta| meta.permissions().readonly())
        {
            return Ok(WriteResult::ReadOnly);
        }
        if let Some(result) = check(path, expect)? {
            return Ok(result);
        }
        let stage = stage_path(path)?;
        let written = (|| -> io::Result<Option<WriteResult>> {
            let like = match mode {
                WriteMode::Private => None,
                WriteMode::KeepTargetSecurity => target.as_ref(),
            };
            let mut file = open_new(&stage, mode == WriteMode::Private, like)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            // The last moment: someone may have written or removed it meanwhile.
            if let Some(result) = check(path, expect)? {
                return Ok(Some(result));
            }
            fs::rename(&stage, path)?;
            Ok(None)
        })();
        match written {
            Ok(None) => {
                sync_parent(path);
                Ok(WriteResult::Written)
            }
            Ok(Some(result)) => {
                let _ = fs::remove_file(&stage);
                Ok(result)
            }
            Err(error) => {
                let _ = fs::remove_file(&stage);
                Err(error)
            }
        }
    }

    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        let stage = stage_path(path)?;
        let result = (|| -> io::Result<bool> {
            let mut file = open_new(&stage, true, None)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            // A hard link never replaces: exactly one writer wins.
            match fs::hard_link(&stage, path) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
                Err(e) => Err(e),
            }
        })();
        let _ = fs::remove_file(&stage);
        if result.as_ref().is_ok_and(|created| *created) {
            sync_parent(path);
        }
        result
    }

    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        fs::metadata(path).map(|meta| identity_of(&meta))
    }

    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        fs::symlink_metadata(path).map(|meta| meta.file_type().is_symlink())
    }

    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        let canonical = fs::canonicalize(path)?;
        let text = canonical.to_string_lossy();
        Ok(match text.strip_prefix(r"\\?\") {
            Some(rest)
                if rest.len() >= 4
                    && rest.is_char_boundary(4)
                    && rest[..4].eq_ignore_ascii_case(r"UNC\") =>
            {
                PathBuf::from(format!(r"\\{}", &rest[4..]))
            }
            Some(rest) => PathBuf::from(rest),
            None => canonical,
        })
    }

    fn is_private(&self, path: &Path) -> io::Result<bool> {
        let meta = fs::metadata(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok(meta.permissions().mode() & 0o077 == 0)
        }
        #[cfg(not(unix))]
        {
            // Plain std can't read an ACL; only agentnotch-win can say.
            let _ = meta;
            Ok(false)
        }
    }
}

/// Makes the rename itself durable where the OS allows syncing a folder.
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
