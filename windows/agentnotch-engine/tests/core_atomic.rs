//! `core::atomic::StdSecureFiles`: the testkit's `SecureFiles`. The real
//! Windows one has the same contract (`agentnotch-win/tests/win_files.rs`).

use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::platform::{Expect, SecureFiles, WriteMode, WriteResult};
use agentnotch_engine::testkit::mode_of;
use std::path::Path;

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.contains(".agentnotch-") && name.ends_with(".tmp"))
        .collect()
}

#[test]
fn private_dir_and_private_file() {
    let root = tempfile::tempdir().unwrap();
    let support = root.path().join("support").join("Claude");
    let files = StdSecureFiles;
    files.ensure_private_dir(&support).unwrap();
    let path = support.join("accounts.json");
    assert_eq!(
        files
            .write_atomic(&path, b"{}", WriteMode::Private, Expect::Absent)
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    if cfg!(unix) {
        assert_eq!(mode_of(&support), Some(0o700));
        assert_eq!(mode_of(&path), Some(0o600));
        assert!(files.is_private(&path).unwrap());
    }
    assert!(leftovers(&support).is_empty());
}

#[test]
fn expectations_are_checked() {
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{\"old\":1}").unwrap();
    let identity = files.identity(&path).unwrap();

    // Absent expected, but it exists.
    assert_eq!(
        files
            .write_atomic(&path, b"{}", WriteMode::KeepTargetSecurity, Expect::Absent)
            .unwrap(),
        WriteResult::Changed
    );
    // Changed since it was read.
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(&path, b"{\"other\":22}").unwrap();
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"{}",
                WriteMode::KeepTargetSecurity,
                Expect::Same(identity)
            )
            .unwrap(),
        WriteResult::Changed
    );
    // Gone since it was read: nothing is written in its place.
    let identity = files.identity(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"{}",
                WriteMode::KeepTargetSecurity,
                Expect::Same(identity)
            )
            .unwrap(),
        WriteResult::Vanished
    );
    assert!(!path.exists());
    // Still what was read: written.
    std::fs::write(&path, b"{\"v\":1}").unwrap();
    let identity = files.identity(&path).unwrap();
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"{\"v\":2}",
                WriteMode::KeepTargetSecurity,
                Expect::Same(identity)
            )
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"v\":2}");
    assert!(leftovers(root.path()).is_empty());
}

#[test]
fn keep_target_security_keeps_the_mode() {
    if !cfg!(unix) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    files
        .write_atomic(
            &path,
            b"{\"a\":1}",
            WriteMode::KeepTargetSecurity,
            Expect::Nothing,
        )
        .unwrap();
    assert_eq!(mode_of(&path), Some(0o640));
}

#[test]
fn a_read_only_target_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{}").unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"{\"x\":1}",
                WriteMode::KeepTargetSecurity,
                Expect::Nothing
            )
            .unwrap(),
        WriteResult::ReadOnly
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
}

#[test]
fn create_exclusive_has_one_winner() {
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    let path = root.path().join("cloud-install-secret");
    assert!(files.create_exclusive(&path, &[7u8; 32]).unwrap());
    assert!(!files.create_exclusive(&path, &[9u8; 32]).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), vec![7u8; 32]);
    if cfg!(unix) {
        assert_eq!(mode_of(&path), Some(0o600));
    }
    assert!(leftovers(root.path()).is_empty());
}

#[test]
fn identity_links_and_canonical() {
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    let path = root.path().join("a.json");
    std::fs::write(&path, b"1").unwrap();
    let before = files.identity(&path).unwrap();
    let moved = root.path().join("b.json");
    std::fs::rename(&path, &moved).unwrap();
    let after = files.identity(&moved).unwrap();
    if cfg!(unix) {
        assert_eq!(
            (before.volume, before.index),
            (after.volume, after.index),
            "stable across a rename"
        );
    }
    assert_eq!(before.size, after.size);
    assert!(!files.is_reparse(&moved).unwrap());
    #[cfg(unix)]
    {
        let link = root.path().join("link.json");
        std::os::unix::fs::symlink(&moved, &link).unwrap();
        assert!(files.is_reparse(&link).unwrap());
        assert_eq!(
            files.canonical(&link).unwrap(),
            files.canonical(&moved).unwrap()
        );
    }
}
