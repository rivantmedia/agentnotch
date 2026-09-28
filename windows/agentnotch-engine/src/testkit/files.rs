//! File helpers for tests: build a fake Claude setup, and prove that
//! nothing outside what a test allows was changed (byte for byte).

pub use crate::core::atomic::StdSecureFiles;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Writes `bytes` at `path`, creating its folders.
pub fn write_file(path: &Path, bytes: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the file's folders");
    }
    std::fs::write(path, bytes).expect("write a test file");
}

/// A `.claude.json` naming a login (made-up identities only).
pub fn claude_json(account_uuid: &str, email: &str, organization_uuid: Option<&str>) -> String {
    let org = organization_uuid
        .map(|o| format!(r#","organizationUuid":"{o}","organizationName":"Test Org""#))
        .unwrap_or_default();
    format!(
        r#"{{"numStartups":3,"oauthAccount":{{"accountUuid":"{account_uuid}","emailAddress":"{email}","organizationType":"claude_max"{org}}},"projects":{{}}}}"#
    )
}

/// Every regular file under `dir` with its bytes, keyed by the path relative
/// to `dir`: compare two of these to show a folder was left alone.
pub fn snapshot_dir(dir: &Path) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = BTreeMap::new();
    if !dir.exists() {
        return Ok(files);
    }
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(dir)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|_| entry.path());
                files.insert(relative, std::fs::read(entry.path())?);
            }
        }
    }
    Ok(files)
}

/// The file's permission bits (POSIX; `None` elsewhere).
pub fn mode_of(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
