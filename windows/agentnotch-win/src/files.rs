//! Files the app writes on Windows (DESIGN-WIN §3.2 `SecureFiles`, §4.3; WP2): private folders
//! with a protected DACL (the user and SYSTEM only), staged atomic replaces with one rename
//! (`FileRenameInfoEx`, `MoveFileExW` fallback) that keep the target's security or apply the
//! private one, exclusive create, file identity, reparse-point checks, canonical paths.
//!
//! Not implemented in this build: every call fails, so nothing is written anywhere (no
//! settings.json, no `<support>` file) and the hub reports what it could not save.

#![cfg(windows)]

use std::io;
use std::path::{Path, PathBuf};

use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};

use crate::NOT_IMPLEMENTED;

fn not_implemented() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, NOT_IMPLEMENTED)
}

#[derive(Debug, Default)]
pub struct WinFiles;

impl WinFiles {
    pub fn new() -> Self {
        WinFiles
    }
}

impl SecureFiles for WinFiles {
    fn ensure_private_dir(&self, _dir: &Path) -> io::Result<()> {
        Err(not_implemented())
    }
    fn write_atomic(
        &self,
        _path: &Path,
        _bytes: &[u8],
        _mode: WriteMode,
        _expect: Expect,
    ) -> io::Result<WriteResult> {
        Err(not_implemented())
    }
    fn create_exclusive(&self, _path: &Path, _bytes: &[u8]) -> io::Result<bool> {
        Err(not_implemented())
    }
    fn identity(&self, _path: &Path) -> io::Result<FileIdentity> {
        Err(not_implemented())
    }
    fn is_reparse(&self, _path: &Path) -> io::Result<bool> {
        Err(not_implemented())
    }
    fn canonical(&self, _path: &Path) -> io::Result<PathBuf> {
        Err(not_implemented())
    }
    fn is_private(&self, _path: &Path) -> io::Result<bool> {
        Err(not_implemented())
    }
}
