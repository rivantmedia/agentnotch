//! Sessions Claude Desktop hosts (DesktopHostedSessionsTests.swift): what
//! marks them in the registry, and whose they are, from Desktop's own record
//! of each (a metadata read, never opened). Temporary folders only.

mod sessions_support;

use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::model::{Attribution, DesktopCandidate, IdentityId};
use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use agentnotch_engine::sessions::desktop::{
    attribution, find_hosted, hosted_attribution, identity, is_desktop_hosted, is_record, listing,
    records_root, valid_host_session_id, DesktopAttributor, HostedLookup, RETRY_AFTER,
};
use agentnotch_engine::sessions::registry::parse_entry;
use serde_json::json;
use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

const HOST: &str = "local_0123abcd-89ef-4a5b-8c6d-001122334455";
const ACCOUNT_A: &str = "a1b2c3d4-1111-4222-8333-444455556666";
const ORG_A: &str = "5e4d3c2b-1a09-4f8e-9d7c-6b5a49382716";
const ACCOUNT_W: &str = "b2c3d4e5-2222-4333-8444-555566667777";
const ORG_W: &str = "6f5e4d3c-2b1a-4a9f-8e8d-7c6b5a493827";

fn identity_a() -> IdentityId {
    IdentityId::from(format!("uuid:{ACCOUNT_A}/{ORG_A}"))
}

fn identity_w() -> IdentityId {
    IdentityId::from(format!("uuid:{ACCOUNT_W}/{ORG_W}"))
}

fn candidates() -> Vec<DesktopCandidate> {
    vec![
        DesktopCandidate {
            identity_id: identity_a(),
            account_uuid: ACCOUNT_A.into(),
            organization_uuid: Some(ORG_A.into()),
        },
        DesktopCandidate {
            identity_id: identity_w(),
            account_uuid: ACCOUNT_W.into(),
            organization_uuid: Some(ORG_W.into()),
        },
    ]
}

fn work(organization: Option<&str>) -> Vec<DesktopCandidate> {
    vec![DesktopCandidate {
        identity_id: identity_w(),
        account_uuid: ACCOUNT_W.into(),
        organization_uuid: organization.map(str::to_owned),
    }]
}

/// A Claude Desktop folder in a temporary home; the records are under
/// `<desktop>/claude-code-sessions`.
struct Desktop {
    _home: TempDir,
    desktop: PathBuf,
}

impl Desktop {
    fn new() -> Desktop {
        let home = TempDir::new().unwrap();
        let desktop = home.path().join("Claude");
        std::fs::create_dir_all(records_root(&desktop)).unwrap();
        Desktop {
            _home: home,
            desktop,
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![self.desktop.clone()]
    }

    /// Desktop's record of the session, under the account and organization.
    fn record(&self, account: &str, organization: &str, id: &str) -> PathBuf {
        record_under(&records_root(&self.desktop), account, organization, id)
    }

    fn find(&self, candidates: &[DesktopCandidate]) -> Option<IdentityId> {
        find_hosted(&self.roots(), HOST, candidates, &StdSecureFiles, false)
    }
}

fn record_under(root: &Path, account: &str, organization: &str, id: &str) -> PathBuf {
    let folder = root.join(account).join(organization);
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join(format!("{id}.json"));
    std::fs::write(&file, br#"{"secret":"never read"}"#).unwrap();
    file
}

/// `StdSecureFiles` that says chosen paths are reparse points (a symbolic
/// link or junction a Windows runner may not be allowed to make), whatever
/// is really there. Counts every call.
struct ReparseAt {
    paths: Vec<PathBuf>,
    calls: AtomicUsize,
}

impl ReparseAt {
    fn new(paths: Vec<PathBuf>) -> ReparseAt {
        ReparseAt {
            paths,
            calls: AtomicUsize::new(0),
        }
    }

    fn touched(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

impl SecureFiles for ReparseAt {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        self.touched();
        StdSecureFiles.ensure_private_dir(dir)
    }
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        self.touched();
        StdSecureFiles.write_atomic(path, bytes, mode, expect)
    }
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        self.touched();
        StdSecureFiles.create_exclusive(path, bytes)
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        self.touched();
        StdSecureFiles.identity(path)
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        self.touched();
        if self.paths.iter().any(|p| p == path) {
            return Ok(true);
        }
        StdSecureFiles.is_reparse(path)
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        self.touched();
        StdSecureFiles.canonical(path)
    }
    fn is_private(&self, path: &Path) -> io::Result<bool> {
        self.touched();
        StdSecureFiles.is_private(path)
    }
}

#[test]
fn the_registry_entry_carries_desktops_session_id() {
    let entry = parse_entry(&json!({
        "pid": 4242, "sessionId": "s-a", "entrypoint": "claude-desktop", "hostSessionId": HOST,
    }))
    .unwrap();
    assert_eq!(entry.host_session_id.as_deref(), Some(HOST));
    assert!(is_desktop_hosted(entry.entrypoint.as_deref()));
    for entrypoint in ["claude-desktop-3p", "local-agent"] {
        let entry = parse_entry(&json!({"pid": 1, "sessionId": "s", "entrypoint": entrypoint}));
        assert!(is_desktop_hosted(entry.unwrap().entrypoint.as_deref()));
    }
    let cli = parse_entry(&json!({"pid": 1, "sessionId": "s", "entrypoint": "cli"})).unwrap();
    assert!(!is_desktop_hosted(cli.entrypoint.as_deref()));
    let none = parse_entry(&json!({"pid": 1, "sessionId": "s"})).unwrap();
    // No entrypoint (the entry's own default is not the file's).
    assert_eq!(none.entrypoint, None);
    assert!(!is_desktop_hosted(none.entrypoint.as_deref()));
    assert!(is_desktop_hosted(Some("remote_desktop")));
    assert!(is_desktop_hosted(Some(" Claude-Desktop ")));
    assert!(!is_desktop_hosted(Some("")));
    assert!(!is_desktop_hosted(None));
    // Only Claude Code's own form: never a path.
    let long = format!("local_{}", "a".repeat(73));
    for bad in [
        "local_../../../.ssh/id_rsa",
        "LOCAL_0123abcd",
        "local_0123ABCD-0000",
        "local_0123",
        "0123abcd-89ef",
        long.as_str(),
        r"local_0123abcd\..\x",
    ] {
        let entry = parse_entry(&json!({"pid": 1, "sessionId": "s", "hostSessionId": bad}));
        assert_eq!(entry.unwrap().host_session_id, None, "{bad}");
        assert!(!valid_host_session_id(bad), "{bad}");
    }
    let longest = format!("local_{}", "a".repeat(72));
    assert!(valid_host_session_id(&longest));
    assert!(valid_host_session_id("local_01234567"));
    assert!(!valid_host_session_id("local_0123456"));
}

#[test]
fn the_session_is_the_identity_whose_record_exists() {
    let desktop = Desktop::new();
    // No record yet: nobody's (not the only account, not the folder's).
    assert_eq!(desktop.find(&candidates()), None);
    let file = desktop.record(ACCOUNT_W, ORG_W, HOST);
    assert_eq!(desktop.find(&candidates()), Some(identity_w()));
    // Found by a metadata read alone: an unreadable record is still found.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(desktop.find(&candidates()), Some(identity_w()));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = file;
    // Another session's record, another organization's, or an unknown
    // account's: not this one.
    desktop.record(ACCOUNT_A, ORG_A, "local_ffffffff-0000");
    desktop.record(ACCOUNT_A, ORG_W, HOST);
    desktop.record("c0ffee00-0000-4000-8000-000000000000", ORG_A, HOST);
    assert_eq!(desktop.find(&candidates()), Some(identity_w()));
    // Under two identities: can't tell.
    desktop.record(ACCOUNT_A, ORG_A, HOST);
    assert_eq!(desktop.find(&candidates()), None);
}

#[test]
fn an_identity_with_no_known_organization_is_looked_up_in_each_of_its_own() {
    let desktop = Desktop::new();
    desktop.record(ACCOUNT_A, ORG_A, HOST);
    let candidates = vec![DesktopCandidate {
        identity_id: identity_a(),
        account_uuid: ACCOUNT_A.to_uppercase(),
        organization_uuid: None,
    }];
    assert_eq!(desktop.find(&candidates), Some(identity_a()));
    assert_eq!(
        find_hosted(&desktop.roots(), "", &candidates, &StdSecureFiles, false),
        None
    );
    // A stray non-UUID folder in the account's folder is not an organization.
    let stray = records_root(&desktop.desktop)
        .join(ACCOUNT_A)
        .join("not-an-org");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join(format!("{HOST}.json")), b"{}").unwrap();
    assert_eq!(desktop.find(&candidates), Some(identity_a()));
    let other = Desktop::new();
    let stray = records_root(&other.desktop)
        .join(ACCOUNT_A)
        .join("not-an-org");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join(format!("{HOST}.json")), b"{}").unwrap();
    assert_eq!(other.find(&candidates), None);
}

#[test]
fn records_under_any_root_count_and_two_identities_across_roots_are_unsure() {
    let first = Desktop::new();
    let second = Desktop::new();
    second.record(ACCOUNT_W, ORG_W, HOST);
    let roots = vec![first.desktop.clone(), second.desktop.clone()];
    assert_eq!(
        find_hosted(&roots, HOST, &candidates(), &StdSecureFiles, false),
        Some(identity_w())
    );
    first.record(ACCOUNT_A, ORG_A, HOST);
    assert_eq!(
        find_hosted(&roots, HOST, &candidates(), &StdSecureFiles, false),
        None
    );
    // The same identity under both roots is still one identity.
    first.record(ACCOUNT_W, ORG_W, HOST);
    std::fs::remove_file(
        records_root(&first.desktop)
            .join(ACCOUNT_A)
            .join(ORG_A)
            .join(format!("{HOST}.json")),
    )
    .unwrap();
    assert_eq!(
        find_hosted(&roots, HOST, &candidates(), &StdSecureFiles, false),
        Some(identity_w())
    );
    assert_eq!(
        find_hosted(&[], HOST, &candidates(), &StdSecureFiles, false),
        None
    );
}

/// Regression (review): a link is never followed. A record that is a link to
/// a real file, or one reached through a linked organization folder, account
/// folder or root, is no record, and a linked account folder isn't listed.
/// Each tree here is real folders and files with one component declared a
/// reparse point, so the check is `SecureFiles::is_reparse`'s alone.
#[test]
fn links_are_never_followed() {
    for organization in [Some(ORG_W), None] {
        let candidates = work(organization);
        let desktop = Desktop::new();
        let root = records_root(&desktop.desktop);
        let record = desktop.record(ACCOUNT_W, ORG_W, HOST);
        let account = root.join(ACCOUNT_W);
        let org = account.join(ORG_W);
        let find = |files: &dyn SecureFiles| {
            find_hosted(&desktop.roots(), HOST, &candidates, files, false)
        };
        assert_eq!(find(&StdSecureFiles), Some(identity_w()));
        assert_eq!(find(&ReparseAt::new(vec![])), Some(identity_w()));
        for (what, link) in [
            ("record", &record),
            ("organization", &org),
            ("account", &account),
            ("root", &root),
        ] {
            assert_eq!(
                find(&ReparseAt::new(vec![link.clone()])),
                None,
                "a linked {what} ({organization:?})"
            );
        }
        // A linked account folder isn't listed either.
        assert!(listing(&ReparseAt::new(vec![account.clone()]), &account).is_empty());
        assert!(!listing(&StdSecureFiles, &account).is_empty());
        assert!(is_record(&StdSecureFiles, &record));
        assert!(!is_record(&ReparseAt::new(vec![root.clone()]), &record));
    }
}

/// Only what is really there, and only plain files and folders: a folder
/// where the record should be, a record that is missing, or a file where a
/// folder should be is no record.
#[test]
fn only_a_regular_file_in_real_folders_is_a_record() {
    let desktop = Desktop::new();
    let root = records_root(&desktop.desktop);
    let folder_record = root
        .join(ACCOUNT_W)
        .join(ORG_W)
        .join(format!("{HOST}.json"));
    std::fs::create_dir_all(&folder_record).unwrap();
    assert_eq!(desktop.find(&work(Some(ORG_W))), None);
    assert_eq!(desktop.find(&work(None)), None);
    let other = Desktop::new();
    std::fs::create_dir_all(records_root(&other.desktop).join(ACCOUNT_W)).unwrap();
    std::fs::write(
        records_root(&other.desktop).join(ACCOUNT_W).join(ORG_W),
        b"{}",
    )
    .unwrap();
    assert_eq!(other.find(&work(Some(ORG_W))), None);
    // No Desktop at all.
    let missing = vec![desktop.desktop.join("nowhere")];
    assert_eq!(
        find_hosted(&missing, HOST, &work(Some(ORG_W)), &StdSecureFiles, false),
        None
    );
}

#[cfg(unix)]
#[test]
fn real_symbolic_links_are_never_followed() {
    use std::os::unix::fs::symlink;
    let home = TempDir::new().unwrap();
    let elsewhere = home.path().join("elsewhere").join("claude-code-sessions");
    let real_record = record_under(&elsewhere, ACCOUNT_W, ORG_W, HOST);
    let find = |desktop: &Path, organization: Option<&str>| {
        find_hosted(
            &[desktop.to_path_buf()],
            HOST,
            &work(organization),
            &StdSecureFiles,
            false,
        )
    };
    let elsewhere_desktop = home.path().join("elsewhere");
    for organization in [Some(ORG_W), None] {
        assert_eq!(find(&elsewhere_desktop, organization), Some(identity_w()));
        let make_root = |name: &str| {
            let desktop = home.path().join(name);
            let root = records_root(&desktop);
            std::fs::create_dir_all(&root).unwrap();
            (desktop, root)
        };
        // The record is a link to a real file.
        let (desktop, root) = make_root("record");
        let org = root.join(ACCOUNT_W).join(ORG_W);
        std::fs::create_dir_all(&org).unwrap();
        symlink(&real_record, org.join(format!("{HOST}.json"))).unwrap();
        assert_eq!(find(&desktop, organization), None);
        // The organization's folder is a link.
        let (desktop, root) = make_root("organization");
        std::fs::create_dir_all(root.join(ACCOUNT_W)).unwrap();
        symlink(
            real_record.parent().unwrap(),
            root.join(ACCOUNT_W).join(ORG_W),
        )
        .unwrap();
        assert_eq!(find(&desktop, organization), None);
        // The account's folder is a link: not listed either.
        let (desktop, root) = make_root("account");
        symlink(elsewhere.join(ACCOUNT_W), root.join(ACCOUNT_W)).unwrap();
        assert_eq!(find(&desktop, organization), None);
        assert!(listing(&StdSecureFiles, &root.join(ACCOUNT_W)).is_empty());
        // The records folder itself is a link.
        let desktop = home.path().join("root");
        std::fs::create_dir_all(&desktop).unwrap();
        symlink(&elsewhere, records_root(&desktop)).unwrap();
        assert_eq!(find(&desktop, organization), None);
        // Cleanup for the next round (names are reused).
        for name in ["record", "organization", "account", "root"] {
            std::fs::remove_dir_all(home.path().join(name)).unwrap();
        }
    }
}

/// Only metadata reads (and folder listings) under Desktop's folder, of the
/// session's own record; a malformed id asks nothing.
#[test]
fn only_the_records_place_is_looked_at() {
    let desktop = Desktop::new();
    let root = records_root(&desktop.desktop);
    let asked: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let found = identity(
        Some(HOST),
        &candidates(),
        &root,
        &|path| {
            asked.borrow_mut().push(path.to_path_buf());
            false
        },
        &|_| Vec::new(),
    );
    assert_eq!(found, None);
    let asked_now = asked.borrow().clone();
    assert_eq!(asked_now.len(), 2);
    for path in &asked_now {
        assert!(path.starts_with(&root));
        assert!(path.file_name().unwrap().to_string_lossy() == format!("{HOST}.json"));
    }
    asked.borrow_mut().clear();
    let none = identity(
        Some("local_../../x"),
        &candidates(),
        &root,
        &|path| {
            asked.borrow_mut().push(path.to_path_buf());
            true
        },
        &|_| vec!["x".into()],
    );
    assert_eq!(none, None);
    assert!(asked.borrow().is_empty());
    assert_eq!(
        identity(None, &candidates(), &root, &|_| true, &|_| vec![]),
        None
    );
    // A candidate whose account isn't a UUID asks nothing.
    let odd = vec![DesktopCandidate {
        identity_id: identity_a(),
        account_uuid: "../..".into(),
        organization_uuid: None,
    }];
    assert_eq!(
        identity(Some(HOST), &odd, &root, &|_| panic!("asked"), &|_| panic!(
            "listed"
        )),
        None
    );
    // The records folder's place inside Claude Desktop's.
    assert_eq!(
        records_root(Path::new("Claude")),
        Path::new("Claude").join("claude-code-sessions")
    );
}

/// The hub's rule: a session Desktop hosts is its record's identity's, else
/// unsure (shown under its folder's account, never recorded); any other
/// session keeps its folder's attribution.
#[test]
fn the_hub_attributes_desktop_sessions_by_their_record_only() {
    let folder = Attribution::Known(Some("uuid:folder".into()));
    let desktop = Some(IdentityId::from("uuid:desktop"));
    assert_eq!(
        attribution(&folder, true, desktop.clone()),
        Attribution::Known(desktop.clone())
    );
    assert_eq!(
        attribution(&folder, true, None),
        Attribution::Unsure(Some("uuid:folder".into()))
    );
    assert_eq!(attribution(&folder, false, None), folder);
    assert_eq!(attribution(&folder, false, desktop.clone()), folder);
    // A folder not grouped yet has no guess to show.
    assert_eq!(
        attribution(&Attribution::Known(None), true, None),
        Attribution::Unsure(None)
    );
    assert_eq!(
        attribution(&Attribution::Waiting, true, None),
        Attribution::Unsure(None)
    );
    assert_eq!(
        attribution(&Attribution::Unsure(Some("uuid:a".into())), true, None),
        Attribution::Unsure(Some("uuid:a".into()))
    );
    // The two pure halves.
    assert_eq!(
        hosted_attribution(desktop.clone(), Some("uuid:folder".into())),
        Attribution::Known(desktop)
    );
    assert_eq!(
        hosted_attribution(None, Some("uuid:folder".into())),
        Attribution::Unsure(Some("uuid:folder".into()))
    );
}

/// Regression (review): only real uncertainty is unsure. What the hub makes
/// of "not placed yet" (waiting, for `PLACEMENT_GRACE`) is WP7's; here the
/// facts it reads stay right: a Desktop-hosted session with no registry
/// entry read has no host id, one whose id's record isn't found is unsure,
/// and a session Desktop doesn't host is never made unsure by this module.
#[test]
fn only_real_uncertainty_is_unsure() {
    let desktop = Desktop::new();
    // A hosted session whose record isn't there (yet): unsure, with the folder's guess.
    let miss = desktop.find(&candidates());
    assert_eq!(
        attribution(&Attribution::Known(Some("uuid:folder".into())), true, miss),
        Attribution::Unsure(Some("uuid:folder".into()))
    );
    // Its record appears: certain.
    desktop.record(ACCOUNT_A, ORG_A, HOST);
    let found = desktop.find(&candidates());
    assert_eq!(
        attribution(&Attribution::Known(Some("uuid:folder".into())), true, found),
        Attribution::Known(Some(identity_a()))
    );
    // Not hosted: the folder's own, whatever it is.
    for folder in [
        Attribution::Known(None),
        Attribution::Waiting,
        Attribution::Unsure(Some("uuid:a".into())),
    ] {
        assert_eq!(attribution(&folder, false, None), folder);
    }
}

/// Looked up again after a miss (Desktop may write its record a moment after
/// the session starts), kept once found.
#[test]
fn lookups_are_remembered_and_retried() {
    let desktop = Desktop::new();
    let mut attributor = DesktopAttributor::new();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let candidates = candidates();
    // Not asked yet: ask. While that is in flight, a repeat question is a miss.
    assert_eq!(attributor.lookup(HOST, &candidates, now), HostedLookup::Ask);
    assert_eq!(
        attributor.lookup(HOST, &candidates, now + Duration::from_millis(10)),
        HostedLookup::Known(None)
    );
    attributor.record(HOST, &candidates, desktop.find(&candidates), now);
    let file = desktop.record(ACCOUNT_A, ORG_A, HOST);
    // The miss stands for RETRY_AFTER.
    assert_eq!(
        attributor.lookup(HOST, &candidates, now + Duration::from_secs(1)),
        HostedLookup::Known(None)
    );
    let later = now + RETRY_AFTER + Duration::from_secs(1);
    assert_eq!(
        attributor.lookup(HOST, &candidates, later),
        HostedLookup::Ask
    );
    attributor.record(HOST, &candidates, desktop.find(&candidates), later);
    assert_eq!(
        attributor.lookup(HOST, &candidates, later),
        HostedLookup::Known(Some(identity_a()))
    );
    // Found: kept for as long as the identities stay the same, whatever the disk says.
    std::fs::remove_file(file).unwrap();
    assert_eq!(
        attributor.lookup(HOST, &candidates, later + Duration::from_secs(600)),
        HostedLookup::Known(Some(identity_a()))
    );
    // Other known identities: asked again.
    assert_eq!(
        attributor.lookup(HOST, &candidates[..1], later + Duration::from_secs(601)),
        HostedLookup::Ask
    );
    // A malformed id is never asked.
    assert_eq!(
        attributor.lookup("", &candidates, later),
        HostedLookup::Invalid
    );
    assert_eq!(
        attributor.lookup("local_../x", &candidates, later),
        HostedLookup::Invalid
    );
    attributor.retain(&[]);
    assert!(attributor.is_empty());
}

/// Sealed: no answer and no disk access whatever is there, a fake that
/// counts every call proves it (and that an unsealed call does look).
#[test]
fn a_sealed_lookup_touches_nothing() {
    let desktop = Desktop::new();
    desktop.record(ACCOUNT_W, ORG_W, HOST);
    let files = ReparseAt::new(vec![]);
    assert_eq!(
        find_hosted(&desktop.roots(), HOST, &candidates(), &files, true),
        None
    );
    assert_eq!(files.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        find_hosted(&desktop.roots(), HOST, &candidates(), &files, false),
        Some(identity_w())
    );
    assert!(files.calls.load(Ordering::SeqCst) > 0);
    // A root that doesn't exist: sealed never even looks.
    let files = ReparseAt::new(vec![]);
    let roots = vec![PathBuf::from("/definitely/not/here/Claude")];
    assert_eq!(find_hosted(&roots, HOST, &candidates(), &files, true), None);
    assert_eq!(files.calls.load(Ordering::SeqCst), 0);
}
