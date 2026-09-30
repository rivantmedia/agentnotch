//! `WinFiles`, the real `SecureFiles`, on a real NTFS volume (DESIGN-WIN §6.2): the DACLs it
//! gives and keeps are read back with `GetNamedSecurityInfoW`, and the replace is tried against
//! another thread that holds the target open the way a scanner or an editor does. Temporary
//! folders only. The same contract on other systems: `agentnotch-engine/tests/core_atomic.rs`.

#![cfg(windows)]

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agentnotch_engine::platform::{Expect, SecureFiles, WriteMode, WriteResult};
use agentnotch_win::files::{retry_delays, strip_verbatim, WinFiles, SYSTEM_SID};
use windows::core::{BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
    GetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SE_DACL_PROTECTED,
};
use windows::Win32::Storage::FileSystem::{
    SetFileAttributesW, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

// winnt.h: ACE types, ACE flags and the access masks the SDDL rights `FA` and `FR` stand for.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const OBJECT_INHERIT_ACE: u8 = 0x1;
const CONTAINER_INHERIT_ACE: u8 = 0x2;
const INHERITED_ACE: u8 = 0x10;
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;
const FILE_GENERIC_READ: u32 = 0x0012_0089;
const EVERYONE_SID: &str = "S-1-1-0";

// --- reading and setting security, without WinFiles ---------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Ace {
    kind: u8,
    flags: u8,
    mask: u32,
    sid: String,
}

/// A DACL as the system reports it: the protection flag and every entry, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dacl {
    protected: bool,
    aces: Vec<Ace>,
}

impl Dacl {
    fn sids(&self) -> Vec<&str> {
        self.aces.iter().map(|ace| ace.sid.as_str()).collect()
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn user_sid() -> String {
    agentnotch_win::sid::current_user_sid().expect("the user's SID")
}

fn dacl_of(path: &Path) -> Dacl {
    let name = wide(path);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `name` is NUL-terminated and alive for the call; `descriptor` is a valid out
    // pointer and receives a LocalAlloc'd descriptor, freed below.
    let result = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            &mut descriptor,
        )
    };
    assert_eq!(result, ERROR_SUCCESS, "{}", path.display());
    let dacl = read_dacl(descriptor);
    // SAFETY: the descriptor was allocated by the call above and is not used after this.
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    dacl
}

fn read_dacl(descriptor: PSECURITY_DESCRIPTOR) -> Dacl {
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: the descriptor is valid (the caller frees it after this returns); both out
    // pointers are valid locals.
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.unwrap();
    let mut present = BOOL(0);
    let mut defaulted = BOOL(0);
    let mut acl: *mut ACL = ptr::null_mut();
    // SAFETY: as above; the three out pointers are valid locals.
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
        .unwrap();
    assert!(present.as_bool() && !acl.is_null(), "the file has a DACL");
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `acl` points into the live descriptor; `size` is a valid out buffer of the
    // length passed.
    unsafe {
        GetAclInformation(
            acl,
            ptr::from_mut(&mut size).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    }
    .unwrap();
    let mut aces = Vec::new();
    for index in 0..size.AceCount {
        let mut entry: *mut c_void = ptr::null_mut();
        // SAFETY: `acl` is live and `index` is below its entry count.
        unsafe { GetAce(acl, index, &mut entry) }.unwrap();
        let entry = entry.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: GetAce succeeded, so `entry` points at an entry inside the DACL, and every
        // entry starts with an ACE_HEADER.
        let header = unsafe { (*entry).Header };
        assert!(
            matches!(
                header.AceType,
                ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE
            ),
            "an entry of a kind this test doesn't know: {}",
            header.AceType
        );
        // SAFETY: allow and deny entries share this layout: a mask, then the SID, which
        // starts at SidStart; only that field's address is taken.
        let (mask, sid) = unsafe { ((*entry).Mask, PSID((&raw mut (*entry).SidStart).cast())) };
        let mut text = PWSTR::null();
        // SAFETY: `sid` points at a SID inside the live DACL; `text` receives a LocalAlloc'd
        // string.
        unsafe { ConvertSidToStringSidW(sid, &mut text) }.unwrap();
        // SAFETY: `text` is the NUL-terminated string the call just returned.
        let sid = unsafe { text.to_string() }.unwrap();
        // SAFETY: the string was allocated by the call above and is not used after this.
        let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        aces.push(Ace {
            kind: header.AceType,
            flags: header.AceFlags,
            mask,
            sid,
        });
    }
    Dacl {
        protected: control & SE_DACL_PROTECTED.0 != 0,
        aces,
    }
}

/// Gives `path` the protected DACL an SDDL string describes.
fn set_protected_dacl(path: &Path, sddl: &str) {
    let name = wide(path);
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `text` is NUL-terminated and alive for the call; `descriptor` receives a
    // LocalAlloc'd descriptor, freed below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(text.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .unwrap();
    let mut present = BOOL(0);
    let mut defaulted = BOOL(0);
    let mut acl: *mut ACL = ptr::null_mut();
    // SAFETY: the descriptor is valid until it is freed below; the out pointers are locals.
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
        .unwrap();
    // SAFETY: `name` is NUL-terminated; `acl` points into the descriptor, which outlives the
    // call.
    let result = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl.cast_const()),
            None,
        )
    };
    // SAFETY: the descriptor was allocated by the conversion above and is not used after this.
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    assert_eq!(result, ERROR_SUCCESS, "{}", path.display());
}

/// The private DACL: protected, full access for the user and for SYSTEM, nobody else. A
/// folder's entries are handed down to what is made inside it; a file's have no such flags.
fn assert_private(path: &Path, folder: bool) {
    let dacl = dacl_of(path);
    let inherit = if folder {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        0
    };
    let entry = |sid: &str| Ace {
        kind: ACCESS_ALLOWED_ACE_TYPE,
        flags: inherit,
        mask: FILE_ALL_ACCESS,
        sid: sid.to_owned(),
    };
    assert_eq!(
        dacl,
        Dacl {
            protected: true,
            aces: vec![entry(&user_sid()), entry(SYSTEM_SID)],
        },
        "{}",
        path.display()
    );
}

// --- helpers --------------------------------------------------------------------------------------

/// Stage files left beside a target.
fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.contains(".agentnotch-") && name.ends_with(".tmp"))
        .collect()
}

/// Opens `path` on another thread the way a scanner or an editor does (others may read and
/// write it, nobody may delete or replace it), returns once it is open, and lets go after
/// `hold`. The flag turns true just before the handle is closed, so a replace that succeeded
/// while it is still false went through a held file.
fn hold_open(path: &Path, hold: Duration) -> (JoinHandle<()>, Arc<AtomicBool>) {
    let (opened, is_open) = mpsc::channel();
    let releasing = Arc::new(AtomicBool::new(false));
    let thread = {
        let path = path.to_owned();
        let releasing = Arc::clone(&releasing);
        thread::spawn(move || {
            let held = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                .open(&path)
                .unwrap();
            opened.send(()).unwrap();
            thread::sleep(hold);
            releasing.store(true, Ordering::SeqCst);
            drop(held);
        })
    };
    is_open.recv().unwrap();
    (thread, releasing)
}

/// `mklink /J`: a junction needs no privilege, unlike a symlink.
fn make_junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mklink /J failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// --- ensure_private_dir ---------------------------------------------------------------------------

/// A test that can't run its body here says so. On the CI runner (an administrator, 8.3 names on)
/// every body can run, so a skip there is a failure: a green run has to mean the test ran.
fn skipped(why: &str) {
    assert!(
        std::env::var_os("GITHUB_ACTIONS").is_none(),
        "skipped on the CI runner: {why}"
    );
    println!("skipped: {why}");
}

#[test]
fn a_private_folder_lets_in_only_the_user_and_system() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();

    // A new folder, parents included: born private.
    let support = root.path().join("Agent Notch").join("Claude");
    files.ensure_private_dir(&support).unwrap();
    assert!(support.is_dir());
    assert_private(&support, true);
    assert!(files.is_private(&support).unwrap());
    // Only the leaf is ours to lock down; the folder above keeps what it inherits.
    let parent = support.parent().unwrap();
    assert!(!dacl_of(parent).protected);
    assert!(!files.is_private(parent).unwrap());

    // Asking again changes nothing.
    files.ensure_private_dir(&support).unwrap();
    assert_private(&support, true);

    // What is made inside inherits exactly those two entries.
    let inside = support.join("accounts.json");
    std::fs::write(&inside, b"{}").unwrap();
    let inherited = dacl_of(&inside);
    assert_eq!(inherited.sids(), [user_sid().as_str(), SYSTEM_SID]);
    assert!(inherited
        .aces
        .iter()
        .all(|ace| ace.kind == ACCESS_ALLOWED_ACE_TYPE && ace.flags & INHERITED_ACE != 0));

    // A folder that was already there, open to whoever its parent lets in, is locked down.
    let existing = root.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("kept.json"), b"kept").unwrap();
    assert!(!files.is_private(&existing).unwrap());
    files.ensure_private_dir(&existing).unwrap();
    assert_private(&existing, true);
    assert!(files.is_private(&existing).unwrap());
    assert_eq!(std::fs::read(existing.join("kept.json")).unwrap(), b"kept");

    // A file where the folder should be is an error, and stays a file.
    let file = root.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    assert!(files.ensure_private_dir(&file).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"x");
}

// --- write_atomic: security -----------------------------------------------------------------------

#[test]
fn a_private_write_applies_the_private_dacl() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();

    // A new file in a folder anyone the parent lets in could read.
    let new = root.path().join("cloud-session.json");
    assert_eq!(
        files
            .write_atomic(&new, b"{\"a\":1}", WriteMode::Private, Expect::Absent)
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&new).unwrap(), b"{\"a\":1}");
    assert_private(&new, false);
    assert!(files.is_private(&new).unwrap());

    // A file that was there with an inherited DACL becomes private too.
    let open = root.path().join("usage-state.json");
    std::fs::write(&open, b"old").unwrap();
    assert!(!files.is_private(&open).unwrap());
    assert_eq!(
        files
            .write_atomic(&open, b"new", WriteMode::Private, Expect::Nothing)
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&open).unwrap(), b"new");
    assert_private(&open, false);

    // And it stays private when written again.
    files
        .write_atomic(&open, b"newer", WriteMode::Private, Expect::Nothing)
        .unwrap();
    assert_eq!(std::fs::read(&open).unwrap(), b"newer");
    assert_private(&open, false);
    assert!(leftovers(root.path()).is_empty());
}

#[test]
fn keeping_the_targets_security_keeps_its_dacl() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();

    // A DACL of the user's own making: protected, and Everyone may read.
    let own = root.path().join("settings.json");
    std::fs::write(&own, b"{\"old\":1}").unwrap();
    set_protected_dacl(&own, &format!("D:P(A;;FA;;;{})(A;;FR;;;WD)", user_sid()));
    let before = dacl_of(&own);
    assert!(before.protected);
    assert_eq!(before.sids(), [user_sid().as_str(), EVERYONE_SID]);
    assert_eq!(before.aces[1].mask, FILE_GENERIC_READ);
    assert_eq!(
        files
            .write_atomic(
                &own,
                b"{\"new\":2}",
                WriteMode::KeepTargetSecurity,
                Expect::Nothing
            )
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&own).unwrap(), b"{\"new\":2}");
    assert_eq!(dacl_of(&own), before);
    assert!(!files.is_private(&own).unwrap());

    // A plain file, whose DACL is all inherited: still unprotected, the same entries.
    let plain = root.path().join("plain.json");
    std::fs::write(&plain, b"1").unwrap();
    let before = dacl_of(&plain);
    assert!(!before.protected);
    files
        .write_atomic(&plain, b"2", WriteMode::KeepTargetSecurity, Expect::Nothing)
        .unwrap();
    assert_eq!(std::fs::read(&plain).unwrap(), b"2");
    assert_eq!(dacl_of(&plain), before);

    // A new file takes its folder's security.
    let private = root.path().join("private");
    files.ensure_private_dir(&private).unwrap();
    let new = private.join("settings.json");
    assert_eq!(
        files
            .write_atomic(&new, b"{}", WriteMode::KeepTargetSecurity, Expect::Absent)
            .unwrap(),
        WriteResult::Written
    );
    let inherited = dacl_of(&new);
    assert!(!inherited.protected);
    assert_eq!(inherited.sids(), [user_sid().as_str(), SYSTEM_SID]);
    assert!(leftovers(root.path()).is_empty());
    assert!(leftovers(&private).is_empty());
}

#[test]
fn keeping_the_targets_security_keeps_its_attributes() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"old").unwrap();
    let name = wide(&path);
    // SAFETY: `name` is NUL-terminated and alive for the call.
    unsafe {
        SetFileAttributesW(
            PCWSTR(name.as_ptr()),
            FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE,
        )
    }
    .unwrap();
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"new",
                WriteMode::KeepTargetSecurity,
                Expect::Nothing
            )
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    let attributes = std::fs::metadata(&path).unwrap().file_attributes();
    assert_ne!(attributes & FILE_ATTRIBUTE_HIDDEN.0, 0, "{attributes:#x}");
    assert_ne!(attributes & FILE_ATTRIBUTE_ARCHIVE.0, 0, "{attributes:#x}");
}

// --- write_atomic: someone holds the target -------------------------------------------------------

#[test]
fn a_replace_waits_out_a_short_hold() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{\"old\":1}").unwrap();

    let (holder, releasing) = hold_open(&path, Duration::from_millis(300));
    let result = files.write_atomic(
        &path,
        b"{\"new\":2}",
        WriteMode::KeepTargetSecurity,
        Expect::Nothing,
    );
    // The rename can't go through a file held without delete sharing, so it succeeded only
    // after the holder let go.
    let released_first = releasing.load(Ordering::SeqCst);
    holder.join().unwrap();
    assert_eq!(result.unwrap(), WriteResult::Written);
    assert!(released_first, "the replace went through a held file");
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"new\":2}");
    assert!(leftovers(root.path()).is_empty());
}

#[test]
fn a_replace_gives_up_cleanly_on_a_long_hold() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{\"old\":1}").unwrap();
    let before = files.identity(&path).unwrap();

    let (holder, releasing) = hold_open(&path, Duration::from_secs(2));
    let started = Instant::now();
    let result = files.write_atomic(
        &path,
        b"{\"new\":2}",
        WriteMode::KeepTargetSecurity,
        Expect::Same(before),
    );
    let waited = started.elapsed();
    let still_held = !releasing.load(Ordering::SeqCst);

    let error = result.expect_err("the target was held the whole time");
    assert!(
        matches!(error.raw_os_error(), Some(5 | 32)),
        "a sharing or access error: {error:?}"
    );
    assert!(still_held, "gave up only after the holder let go");
    // Every retry was used before giving up.
    assert!(
        waited >= retry_delays().iter().sum::<Duration>(),
        "{waited:?}"
    );
    // The target is exactly what it was, while still held and afterwards, and no stage is left.
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"old\":1}");
    assert_eq!(files.identity(&path).unwrap(), before);
    assert!(leftovers(root.path()).is_empty());
    holder.join().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"old\":1}");
    assert_eq!(files.identity(&path).unwrap(), before);

    // Once it is free the same write goes through.
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"{\"new\":2}",
                WriteMode::KeepTargetSecurity,
                Expect::Same(before),
            )
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"new\":2}");
    assert!(leftovers(root.path()).is_empty());
}

// --- write_atomic: expectations -------------------------------------------------------------------

#[test]
fn a_changed_or_vanished_target_is_not_written() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{\"old\":1}").unwrap();
    let identity = files.identity(&path).unwrap();

    // Changed since it was read.
    thread::sleep(Duration::from_millis(20));
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
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"other\":22}");

    // Replaced by another file with the same bytes and length: still a change.
    let identity = files.identity(&path).unwrap();
    let other = root.path().join("other.json");
    std::fs::write(&other, b"{\"other\":22}").unwrap();
    std::fs::rename(&other, &path).unwrap();
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
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"other\":22}");

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
fn absent_is_checked() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("settings.json");

    // Nothing there, as expected.
    assert_eq!(
        files
            .write_atomic(
                &path,
                b"{\"a\":1}",
                WriteMode::KeepTargetSecurity,
                Expect::Absent
            )
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}");
    // Someone made it meanwhile: theirs is kept.
    for mode in [WriteMode::KeepTargetSecurity, WriteMode::Private] {
        assert_eq!(
            files
                .write_atomic(&path, b"{}", mode, Expect::Absent)
                .unwrap(),
            WriteResult::Changed
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}");
    }
    assert!(leftovers(root.path()).is_empty());
}

#[test]
fn a_read_only_target_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{}").unwrap();
    let identity = files.identity(&path).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions.clone()).unwrap();

    for mode in [WriteMode::KeepTargetSecurity, WriteMode::Private] {
        assert_eq!(
            files
                .write_atomic(&path, b"{\"x\":1}", mode, Expect::Nothing)
                .unwrap(),
            WriteResult::ReadOnly
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        assert_eq!(files.identity(&path).unwrap(), identity);
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
        assert!(leftovers(root.path()).is_empty());
    }

    // So the temporary folder can be removed.
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
}

// --- create_exclusive -----------------------------------------------------------------------------

#[test]
fn create_exclusive_has_one_winner() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("cloud-install-secret");
    assert!(files.create_exclusive(&path, &[7u8; 32]).unwrap());
    assert!(!files.create_exclusive(&path, &[9u8; 32]).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), vec![7u8; 32]);
    assert_private(&path, false);
    assert!(leftovers(root.path()).is_empty());
}

#[test]
fn create_exclusive_has_one_winner_in_a_race() {
    const RACERS: u8 = 8;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cloud-install-secret");
    let start = Arc::new(Barrier::new(usize::from(RACERS)));
    let racers: Vec<JoinHandle<bool>> = (0..RACERS)
        .map(|racer| {
            let path = path.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                WinFiles::new()
                    .create_exclusive(&path, &[racer; 32])
                    .unwrap()
            })
        })
        .collect();
    let won: Vec<bool> = racers
        .into_iter()
        .map(|racer| racer.join().unwrap())
        .collect();
    let winners: Vec<usize> = (0..won.len()).filter(|&index| won[index]).collect();
    assert_eq!(winners.len(), 1, "{won:?}");
    // The file holds the winner's bytes, whole.
    assert_eq!(std::fs::read(&path).unwrap(), vec![winners[0] as u8; 32]);
    assert_private(&path, false);
    assert!(leftovers(root.path()).is_empty());
}

// --- identity -------------------------------------------------------------------------------------

#[test]
fn identity_survives_a_rename_and_changes_on_a_rewrite() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let path = root.path().join("a.json");
    std::fs::write(&path, b"one").unwrap();
    let first = files.identity(&path).unwrap();
    assert_eq!(first.size, 3);
    assert_eq!(
        files.identity(&path).unwrap(),
        first,
        "stable while untouched"
    );

    // The same file under another name.
    let moved = root.path().join("b.json");
    std::fs::rename(&path, &moved).unwrap();
    assert_eq!(files.identity(&moved).unwrap(), first);
    assert_eq!(
        files.identity(&path).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );

    // Rewritten in place: the same file on disk, other contents.
    thread::sleep(Duration::from_millis(20));
    std::fs::write(&moved, b"three").unwrap();
    let rewritten = files.identity(&moved).unwrap();
    assert_ne!(rewritten, first);
    assert_eq!(
        (rewritten.volume, rewritten.index),
        (first.volume, first.index)
    );
    assert_eq!(rewritten.size, 5);
    assert!(rewritten.modified_ns > first.modified_ns);

    // Replaced by our own write: another file, even with the same length.
    files
        .write_atomic(
            &moved,
            b"11111",
            WriteMode::KeepTargetSecurity,
            Expect::Same(rewritten),
        )
        .unwrap();
    let replaced = files.identity(&moved).unwrap();
    assert_eq!(replaced.size, rewritten.size);
    assert_eq!(replaced.volume, rewritten.volume);
    assert_ne!(replaced.index, rewritten.index);

    // Two files are never the same one.
    let twin = root.path().join("twin.json");
    std::fs::write(&twin, b"11111").unwrap();
    assert_ne!(files.identity(&twin).unwrap().index, replaced.index);

    // The modification time is a Unix time: between 2023 and a day from now.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128;
    let day = 86_400 * 1_000_000_000_i128;
    assert!(replaced.modified_ns > 1_672_531_200 * 1_000_000_000_i128);
    assert!(replaced.modified_ns < now + day);
}

// --- links ----------------------------------------------------------------------------------------

#[test]
fn a_junction_and_a_symlink_are_reparse_points() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let file = target.join("settings.json");
    std::fs::write(&file, b"{}").unwrap();

    // Plain things are not, and a missing one is an error rather than "no".
    assert!(!files.is_reparse(&target).unwrap());
    assert!(!files.is_reparse(&file).unwrap());
    assert_eq!(
        files
            .is_reparse(&root.path().join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );

    let junction = root.path().join("junction");
    make_junction(&junction, &target);
    assert!(files.is_reparse(&junction).unwrap());
    // What is reached through it is a plain file.
    assert!(!files.is_reparse(&junction.join("settings.json")).unwrap());

    // Symlinks need a privilege (or Developer Mode) the runner may lack.
    let dir_link = root.path().join("dir-link");
    match std::os::windows::fs::symlink_dir(&target, &dir_link) {
        Ok(()) => assert!(files.is_reparse(&dir_link).unwrap()),
        Err(error) => skipped(&format!("a directory symlink can't be made here ({error})")),
    }
    let file_link = root.path().join("file-link.json");
    match std::os::windows::fs::symlink_file(&file, &file_link) {
        Ok(()) => {
            assert!(files.is_reparse(&file_link).unwrap());
            // Never followed: a link whose target is gone is still a link.
            let dangling = root.path().join("dangling.json");
            std::os::windows::fs::symlink_file(root.path().join("nowhere.json"), &dangling)
                .unwrap();
            assert!(files.is_reparse(&dangling).unwrap());
            // Identity follows it.
            assert_eq!(
                files.identity(&file_link).unwrap(),
                files.identity(&file).unwrap()
            );
        }
        Err(error) => skipped(&format!("a file symlink can't be made here ({error})")),
    }
}

#[test]
fn canonical_strips_the_prefix_and_resolves_a_junction() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let file = target.join("settings.json");
    std::fs::write(&file, b"{}").unwrap();
    let junction = root.path().join("junction");
    make_junction(&junction, &target);

    let direct = files.canonical(&file).unwrap();
    let text = direct.to_str().unwrap();
    assert!(!text.starts_with(r"\\?\"), "{text}");
    assert!(direct.is_absolute());
    assert!(direct.ends_with(Path::new("target").join("settings.json")));
    // The system's own answer, which carries the prefix.
    let std_answer = std::fs::canonicalize(&file).unwrap();
    assert!(std_answer.to_str().unwrap().starts_with(r"\\?\"));
    assert_eq!(
        direct,
        PathBuf::from(strip_verbatim(std_answer.to_str().unwrap()))
    );

    // Through the junction it is the same file at the same place.
    let through = junction.join("settings.json");
    assert_eq!(files.canonical(&through).unwrap(), direct);
    assert_eq!(
        files.canonical(&junction).unwrap(),
        files.canonical(&target).unwrap()
    );
    assert_eq!(
        files.identity(&through).unwrap(),
        files.identity(&file).unwrap()
    );
    // A write to the resolved path lands in the target folder, the stage beside it.
    assert_eq!(
        files
            .write_atomic(
                &files.canonical(&through).unwrap(),
                b"{\"a\":1}",
                WriteMode::KeepTargetSecurity,
                Expect::Nothing
            )
            .unwrap(),
        WriteResult::Written
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"{\"a\":1}");
    assert!(files.is_reparse(&junction).unwrap());

    assert_eq!(
        files
            .canonical(&root.path().join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

// --- 8.3 names ------------------------------------------------------------------------------------

#[test]
fn short_and_long_paths_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let folder = root.path().join("A Folder With Spaces");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("agentnotch-hook.exe");
    std::fs::write(&file, b"x").unwrap();

    // The temporary folder itself may be spelled with 8.3 names; the long spelling of our
    // path is the reference.
    let long = files
        .long_path(&file)
        .expect("an existing path has a long spelling");
    assert!(long.ends_with(Path::new("A Folder With Spaces").join("agentnotch-hook.exe")));
    assert_eq!(files.long_path(&long).unwrap(), long);

    match files.short_path(&file) {
        // A volume can have 8.3 names turned off: then the answer is the long name again.
        Some(short) if short.to_str().is_some_and(|text| !text.contains(' ')) => {
            assert_ne!(short, long);
            assert!(short.to_str().unwrap().contains('~'), "{}", short.display());
            // It names the same file, and leads back to the long spelling.
            assert_eq!(
                files.identity(&short).unwrap(),
                files.identity(&file).unwrap()
            );
            assert_eq!(std::fs::read(&short).unwrap(), b"x");
            assert_eq!(files.long_path(&short).unwrap(), long);
            assert_eq!(
                files.short_path(&long).unwrap(),
                files.short_path(&short).unwrap()
            );
        }
        Some(same) => {
            skipped("this volume has no 8.3 names");
            assert_eq!(files.long_path(&same).unwrap(), long);
        }
        None => skipped("this volume gives no short paths"),
    }

    // Nothing there: no spelling at all.
    let missing = folder.join("missing.exe");
    assert_eq!(files.short_path(&missing), None);
    assert_eq!(files.long_path(&missing), None);
}
