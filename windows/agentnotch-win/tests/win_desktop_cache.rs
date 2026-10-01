//! Claude Desktop's cache reader on a real Windows file system (AU§12.2):
//! Chromium may hold a cache entry open, and an entry that cannot be opened is
//! a miss for that entry (a sharing violation, Windows error 32), never a
//! failure of the read. The engine's own tests can't hold a file with no
//! sharing (engine sources and tests carry no `cfg(windows)`), so this one lives
//! here. Everything else about the reader is in the engine's
//! `usage_desktop_cache.rs`.
#![cfg(windows)]

use agentnotch_engine::usage::desktop::read_folder;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ORGANIZATION: &str = "11111111-2222-3333-4444-555555555555";

/// A body written by libzstd: session 30 percent, weekly 74 percent, both
/// resetting in 2099.
const BODY: &[u8] = &[
    0x28, 0xb5, 0x2f, 0xfd, 0x64, 0x4e, 0x00, 0x7d, 0x04, 0x00, 0x22, 0x08, 0x1b, 0x19, 0x50, 0x77,
    0x0e, 0x3c, 0xeb, 0x6c, 0xaa, 0xd6, 0xf7, 0x72, 0xec, 0x6b, 0x41, 0x54, 0x35, 0x28, 0xbb, 0x5f,
    0xa3, 0x35, 0xa3, 0xaa, 0x6c, 0x2a, 0x04, 0xc0, 0xe7, 0x20, 0xca, 0x9c, 0x11, 0x72, 0x73, 0xea,
    0xde, 0x0e, 0x7e, 0xad, 0xfa, 0xc1, 0xfa, 0xba, 0x40, 0x69, 0xd3, 0xf3, 0xa1, 0x4a, 0x4e, 0xf7,
    0x1a, 0x3e, 0xad, 0x92, 0x92, 0x73, 0x97, 0x49, 0x10, 0x85, 0x8b, 0x10, 0xac, 0x5a, 0x6b, 0x7a,
    0xe1, 0x7b, 0xb0, 0xc4, 0x2e, 0x27, 0xcb, 0x14, 0x3b, 0x2c, 0xcb, 0x1a, 0xe7, 0xd7, 0x08, 0x69,
    0x6d, 0xda, 0xd8, 0x85, 0x25, 0xf6, 0xe0, 0x57, 0x96, 0xac, 0xb1, 0x93, 0x53, 0xd6, 0xec, 0x1e,
    0x1f, 0xcb, 0x2c, 0x09, 0xb6, 0x26, 0xa3, 0x7b, 0x01, 0x0c, 0x00, 0xb9, 0x2c, 0xc2, 0x55, 0x70,
    0x94, 0xa2, 0x00, 0xb3, 0x2c, 0x12, 0x2a, 0x58, 0x0d, 0xd4, 0x04, 0x16, 0x6c, 0x0a, 0xf3, 0x61,
    0x0a, 0x73, 0xaf, 0x06, 0x00, 0x58, 0x02, 0x6e, 0x0f, 0x69, 0x09, 0x6c, 0xc4,
];

/// One Simple Cache entry: header (magic, version, key length, hash, 4 bytes
/// of padding), key, body, then a stand-in header block.
fn entry() -> Vec<u8> {
    let key = format!("1/0/https://claude.ai/api/organizations/{ORGANIZATION}/usage?skip_spend=1");
    let mut out = Vec::new();
    out.extend_from_slice(&0xfcfb_6d1b_a772_5c30_u64.to_le_bytes());
    out.extend_from_slice(&5_u32.to_le_bytes());
    out.extend_from_slice(&(key.len() as u32).to_le_bytes());
    out.extend_from_slice(&0xdead_beef_u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(BODY);
    out.extend_from_slice(b"\x01\0\0\0HTTP/1.1 200\0content-encoding:zstd\0\0");
    out
}

fn write_entry(dir: &Path, name: &str, modified: u64) {
    let path = dir.join(name);
    std::fs::write(&path, entry()).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(modified))
        .unwrap();
}

fn newest(dir: &Path) -> Option<String> {
    read_folder(dir, ORGANIZATION, SystemTime::now()).map(|reading| {
        reading
            .entry
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    })
}

/// Hold the newest entry open with no sharing: the scan goes on to the next
/// one, and finds the locked one again when it is let go.
#[test]
fn an_entry_locked_against_sharing_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    write_entry(dir.path(), "locked_0", 2_000_000);
    write_entry(dir.path(), "other_0", 1_000_000);
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.path().join("locked_0"))
        .unwrap();
    assert_eq!(newest(dir.path()).as_deref(), Some("other_0"));
    drop(holder);
    assert_eq!(newest(dir.path()).as_deref(), Some("locked_0"));
}

/// A file Chromium has open for reading and writing is read all the same: Rust
/// opens with the full share mode, so a writer does not get in the way.
#[test]
fn an_entry_open_for_writing_elsewhere_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    write_entry(dir.path(), "busy_0", 2_000_000);
    let _writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0x1 | 0x2 | 0x4)
        .open(dir.path().join("busy_0"))
        .unwrap();
    assert_eq!(newest(dir.path()).as_deref(), Some("busy_0"));
}
