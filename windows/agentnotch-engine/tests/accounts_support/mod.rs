//! Shared by the account suites: a throwaway home folder with the engine
//! over it, and folders written out by hand for the pure rules.
//!
//! The on-disk suites run with the build's own path style (Windows paths on
//! Windows CI, POSIX ones on the Mac and Linux), so every expected path is
//! built through [`Home::path`]. Nothing outside the temporary folder is
//! read or written.

// Each suite uses its own part of this.
#![allow(dead_code)]

use agentnotch_engine::accounts::classify::STORE_MARKER_NAME;
use agentnotch_engine::accounts::snapshot::read_snapshot;
use agentnotch_engine::accounts::{AccountRegistry, DiskProbe, Folder};
use agentnotch_engine::core::claude_json::ClaudeJsonReader;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{
    AccountId, AccountSighting, FolderKind, FolderSnapshot, FolderSource, Identity, SessionId,
};
use agentnotch_engine::platform::Roots;
use agentnotch_engine::runtime_types::AccountsChanged;
use agentnotch_engine::testkit::{FakeProcesses, StdSecureFiles};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A moment every suite can count from: 2026-09-21T14:13:20Z.
pub fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

pub fn after(seconds: u64) -> SystemTime {
    t0() + Duration::from_secs(seconds)
}

/// A temporary home folder with the engine over it.
pub struct Home {
    // Removed with everything in it when the test ends.
    _root: tempfile::TempDir,
    pub roots: Roots,
    pub paths: Paths,
    pub processes: Arc<FakeProcesses>,
    /// Kept for the test's life: a parse is reused only while the file's
    /// modification time and size are unchanged.
    pub reader: ClaudeJsonReader,
}

impl Home {
    pub fn new() -> Home {
        let root = tempfile::tempdir().expect("a temporary folder");
        let roots = Roots::under(root.path());
        std::fs::create_dir_all(&roots.home).expect("the home folder");
        Home {
            paths: roots.paths(),
            roots,
            _root: root,
            processes: Arc::new(FakeProcesses::default()),
            reader: ClaudeJsonReader::new(),
        }
    }

    /// The home folder, normalized.
    pub fn home(&self) -> String {
        self.paths.home().to_owned()
    }

    /// `relative` (written with `/`) inside the home folder, as the engine
    /// writes paths.
    pub fn path(&self, relative: &str) -> String {
        if relative.is_empty() {
            self.home()
        } else {
            self.paths.join(self.paths.home(), relative)
        }
    }

    pub fn paths_of(&self, relatives: &[&str]) -> Vec<String> {
        relatives.iter().map(|r| self.path(r)).collect()
    }

    pub fn id(&self, relative: &str) -> AccountId {
        AccountId::new(self.path(relative))
    }

    pub fn mkdir(&self, relative: &str) {
        std::fs::create_dir_all(self.path(relative)).expect("create a folder");
    }

    pub fn write(&self, relative: &str, text: &str) {
        let path = PathBuf::from(self.path(relative));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the file's folders");
        }
        std::fs::write(&path, text).expect("write a file");
    }

    pub fn write_json(&self, relative: &str, value: &Value) {
        self.write(relative, &serde_json::to_string(value).expect("json"));
    }

    pub fn remove(&self, relative: &str) {
        let path = PathBuf::from(self.path(relative));
        if path.is_dir() {
            std::fs::remove_dir_all(&path).expect("remove a folder");
        } else {
            std::fs::remove_file(&path).expect("remove a file");
        }
    }

    pub fn exists(&self, relative: &str) -> bool {
        Path::new(&self.path(relative)).exists()
    }

    /// A `.claude.json` naming a login (AccountRegistryTests' `signIn`).
    pub fn sign_in(&self, relative: &str, email: &str, uuid: &str, organization: Option<&str>) {
        let mut account = json!({"accountUuid": uuid, "emailAddress": email});
        if let Some(organization) = organization {
            account["organizationName"] = json!(organization);
            account["organizationUuid"] = json!(format!("org-{organization}"));
        }
        self.write_json(
            relative,
            &json!({"numStartups": 3, "oauthAccount": account}),
        );
    }

    /// A directory link at `link` pointing to `target` (both inside the
    /// home folder unless `target` is relative to the link's folder): a
    /// symbolic link, or on Windows a junction, which needs no privilege.
    pub fn link_dir(&self, link: &str, target: &str) {
        let link = PathBuf::from(self.path(link));
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).expect("create the link's folder");
        }
        make_dir_link(&link, target, &self.path(target));
    }

    /// The registry's file in the (temporary) support folder.
    pub fn accounts_file(&self) -> PathBuf {
        self.roots.support_file("accounts.json")
    }

    /// A registry the way the hub makes one at launch: `accounts.json` loaded
    /// when there is one, then a discovery.
    pub fn registry(&self) -> AccountRegistry {
        self.registry_with(&[])
    }

    pub fn registry_with(&self, extra_dirs: &[String]) -> AccountRegistry {
        let mut registry = self.bare_registry(extra_dirs);
        if let Ok(bytes) = std::fs::read(self.accounts_file()) {
            registry.load(&bytes);
        }
        self.discover(&mut registry);
        registry
    }

    /// A registry that has read nothing yet.
    pub fn bare_registry(&self, extra_dirs: &[String]) -> AccountRegistry {
        AccountRegistry::new(self.paths.clone())
            .with_extra_config_dirs(extra_dirs)
            .with_probe(Arc::new(DiskProbe::new(
                self.paths.clone(),
                Arc::new(StdSecureFiles),
                self.processes.clone(),
            )))
    }

    /// One read of the home folder, asked about what the registry knows.
    pub fn snapshot(&self, registry: &AccountRegistry) -> FolderSnapshot {
        let explicit: Vec<String> = registry
            .explicit_dirs()
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        self.read(&explicit)
    }

    /// One read of the home folder, asked about `explicit`.
    pub fn read(&self, explicit: &[String]) -> FolderSnapshot {
        read_snapshot(
            &self.paths,
            explicit,
            &StdSecureFiles,
            self.processes.as_ref(),
            &self.reader,
        )
    }

    /// A discovery now (the Mac's `discoverNow` and `refreshIdentities`).
    pub fn discover(&self, registry: &mut AccountRegistry) -> AccountsChanged {
        self.discover_at(registry, SystemTime::now())
    }

    pub fn discover_at(&self, registry: &mut AccountRegistry, now: SystemTime) -> AccountsChanged {
        let snapshot = self.snapshot(registry);
        registry.discover(snapshot, now)
    }

    /// Writes `accounts.json` as the hub would (the Mac's `saveNow`).
    pub fn save(&self, registry: &mut AccountRegistry) {
        let bytes = registry.file_bytes().expect("a live registry saves");
        std::fs::create_dir_all(&self.roots.support).expect("the support folder");
        std::fs::write(self.accounts_file(), bytes).expect("write accounts.json");
        registry.mark_saved();
    }

    pub fn sighting(&self, relative: &str, env: Option<&str>, at: SystemTime) -> AccountSighting {
        AccountSighting {
            config_dir: self.id(relative),
            config_dir_env: env.map(str::to_owned),
            session_id: SessionId::new("s"),
            at,
        }
    }

    // ---- Claude Parallel Profiles' layout (PP_TestSupport.swift) ----

    /// A `.claude.json` with project state (skipped by the scanner), the
    /// login, and Claude Code's cached usage.
    pub fn login(&self, uuid: &str, email: &str, cached_fetched_at_ms: Option<f64>) -> Value {
        let organization: String = uuid.chars().take(8).collect();
        let mut value = json!({
            "numStartups": 7,
            "projects": {"/Users/x/repo": {"allowedTools": ["Bash(npm test)"], "history": [{"display": "hi \"there\" {"}]}},
            "oauthAccount": {
                "accountUuid": uuid, "emailAddress": email, "organizationUuid": format!("org-{organization}"),
                "organizationType": "claude_max", "organizationRateLimitTier": "default_claude_max_20x",
            },
        });
        if let Some(fetched) = cached_fetched_at_ms {
            value["cachedUsageUtilization"] = json!({
                "accountUuid": uuid,
                "fetchedAtMs": fetched,
                "utilization": {
                    "five_hour": {"utilization": 12, "resets_at": "2099-01-01T00:00:00Z"},
                    "seven_day": {"utilization": 20, "resets_at": "2099-01-05T00:00:00Z"},
                },
            });
        }
        value
    }

    /// Links the shared history into a folder, as the extension does.
    pub fn link_shared(&self, folder: &str) {
        self.mkdir(folder);
        for entry in SHARED_ENTRIES {
            let shared = format!(".claude-shared/{entry}");
            self.mkdir(&shared);
            let link = format!("{folder}/{entry}");
            if std::fs::read_link(self.path(&link)).is_err() {
                self.link_dir(&link, &shared);
            }
        }
    }

    pub fn add_store(
        &self,
        name: &str,
        uuid: &str,
        email: &str,
        cached: Option<f64>,
        marker: bool,
    ) {
        let folder = format!(".claude-{name}");
        self.link_shared(&folder);
        if marker {
            self.write(&format!("{folder}/{STORE_MARKER_NAME}"), "");
        }
        self.write_json(
            &format!("{folder}/.claude.json"),
            &self.login(uuid, email, cached),
        );
    }

    pub fn add_window(&self, id: &str, uuid: &str, email: &str, cached: Option<f64>) {
        let folder = format!(".claude-windows/{id}");
        self.link_shared(&folder);
        self.write_json(
            &format!("{folder}/.claude.json"),
            &self.login(uuid, email, cached),
        );
    }

    /// The manifest with `created` apart from `stores` (the rest adopted).
    pub fn write_manifest(&self, stores: &[&str], created: &[&str]) {
        self.write_json(
            ".claude-windows/.manifest.json",
            &json!({
                "stores": self.paths_of(stores), "created": self.paths_of(created),
                "customOAuth": false, "defaultConfigDir": null,
            }),
        );
    }

    /// The maintainer's layout (PP_TestSupport's table):
    ///
    /// | folder                          | what                        | identity        |
    /// |---------------------------------|-----------------------------|-----------------|
    /// | ~/.claude (+ ~/.claude.json)    | default, mirrored last-used | paras@rivant.in |
    /// | ~/.claude-paras                 | store (marker, manifest)    | paras@rivant.in |
    /// | ~/.claude-paras-rivant-in       | store                       | paras@rivant.in |
    /// | ~/.claude-claude                | store                       | claude@biios.in |
    /// | ~/.claude-windows/1bf3e8f92b11  | VS Code window              | claude@biios.in |
    /// | ~/.claude-windows/801f9dd51396  | VS Code window              | paras@rivant.in |
    /// | ~/.claude-windows/b9fbb9ecd7cb  | VS Code window              | paras@rivant.in |
    /// | ~/.claude-windows/.manifest.json| the extension's manifest    | —               |
    /// | ~/.claude-shared                | shared history, linked in   | —               |
    pub fn build_user_layout(&self, manifest: bool, markers: bool) {
        self.link_shared(".claude");
        self.write_json(
            ".claude.json",
            &self.login(PARAS_UUID, PARAS, Some(1_790_000_000_000.0)),
        );
        self.add_store(
            "paras",
            PARAS_UUID,
            PARAS,
            Some(1_790_000_100_000.0),
            markers,
        );
        self.add_store("paras-rivant-in", PARAS_UUID, PARAS, None, markers);
        self.add_store(
            "claude",
            BIIOS_UUID,
            BIIOS,
            Some(1_790_000_050_000.0),
            markers,
        );
        self.add_window("1bf3e8f92b11", BIIOS_UUID, BIIOS, Some(1_790_000_300_000.0));
        self.add_window("801f9dd51396", PARAS_UUID, PARAS, Some(1_790_000_200_000.0));
        self.add_window("b9fbb9ecd7cb", PARAS_UUID, PARAS, None);
        if manifest {
            self.write_manifest(&CREATED_STORES, &CREATED_STORES);
        }
    }

    /// Claude Parallel Profiles mirrors `email`'s account into `~/.claude`:
    /// it rewrites `~/.claude.json`'s email and keeps its `accountUuid`.
    pub fn mirror_into_default(&self, keeping_uuid: &str, email: &str) {
        self.write_json(".claude.json", &self.login(keeping_uuid, email, None));
    }
}

pub const PARAS_UUID: &str = "29638aea-5c1e-4d2a-9b7f-1e0d3c4b5a69";
pub const BIIOS_UUID: &str = "3d93ede5-8a2b-4c6d-9e1f-7a5b3c2d1e08";
pub const PARAS: &str = "paras@rivant.in";
pub const BIIOS: &str = "claude@biios.in";
pub const SHARED_ENTRIES: [&str; 7] = [
    "projects",
    "sessions",
    "session-env",
    "shell-snapshots",
    "file-history",
    "plans",
    "todos",
];
pub const CREATED_STORES: [&str; 3] =
    [".claude-claude", ".claude-paras", ".claude-paras-rivant-in"];

#[cfg(unix)]
fn make_dir_link(link: &Path, target_as_written: &str, target_absolute: &str) {
    // A relative target stays relative, as the test wrote it.
    let target = if target_as_written.starts_with("..") {
        target_as_written
    } else {
        target_absolute
    };
    std::os::unix::fs::symlink(target, link).expect("create a symbolic link");
}

#[cfg(not(unix))]
fn make_dir_link(link: &Path, target_as_written: &str, target_absolute: &str) {
    // A junction's target is always absolute: a relative one is resolved
    // against the link's folder first.
    let target = if target_as_written.starts_with("..") {
        let parent = link.parent().expect("a link has a folder");
        parent.join(target_as_written.replace('/', "\\"))
    } else {
        PathBuf::from(target_absolute)
    };
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(&target)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run mklink");
    assert!(status.success(), "mklink /J {link:?} {target:?}");
}

// ---- folders written out by hand ----

pub fn mac() -> Paths {
    Paths::new(PathStyle::Posix, "/Users/me")
}

pub fn win() -> Paths {
    Paths::new(PathStyle::Windows, r"C:\Users\me")
}

/// A folder the way `ClaudeAccount(configDir:…)` makes one: nothing known.
pub fn folder(paths: &Paths, dir: &str) -> Folder {
    Folder::new(paths, dir)
}

/// A folder signed in as `email` / `uuid`.
pub fn signed(paths: &Paths, dir: &str, email: Option<&str>, uuid: Option<&str>) -> Folder {
    let mut folder = Folder::new(paths, dir);
    folder.identity = Some(Identity {
        account_uuid: uuid.map(str::to_owned),
        email: email.map(str::to_owned),
        ..Identity::default()
    });
    folder
}

pub trait FolderExt {
    fn env(self, env: &str) -> Self;
    fn organization(self, uuid: &str) -> Self;
    fn organization_name(self, name: &str) -> Self;
    fn plan(self, subscription: &str) -> Self;
    fn custom(self, label: &str) -> Self;
    fn kind(self, kind: FolderKind) -> Self;
    fn source(self, source: FolderSource) -> Self;
    fn hidden(self) -> Self;
}

impl FolderExt for Folder {
    fn env(mut self, env: &str) -> Self {
        self.config_dir_env = Some(env.to_owned());
        self
    }

    fn organization(mut self, uuid: &str) -> Self {
        self.identity
            .get_or_insert_with(Identity::default)
            .organization_uuid = Some(uuid.to_owned());
        self
    }

    fn organization_name(mut self, name: &str) -> Self {
        self.identity
            .get_or_insert_with(Identity::default)
            .organization_name = Some(name.to_owned());
        self
    }

    fn plan(mut self, subscription: &str) -> Self {
        self.subscription_type = Some(subscription.to_owned());
        self
    }

    fn custom(mut self, label: &str) -> Self {
        self.custom_label = Some(label.to_owned());
        self
    }

    fn kind(mut self, kind: FolderKind) -> Self {
        self.kind = kind;
        self
    }

    fn source(mut self, source: FolderSource) -> Self {
        self.source = source;
        self
    }

    fn hidden(mut self) -> Self {
        self.is_hidden = true;
        self
    }
}
