//! Small builders the usage integration tests share (`mod usage_support;`):
//! folders, accounts, logins and Windows-shaped roots. Each test file uses a
//! few of them, hence the `dead_code` allowance.
//!
//! Paths are built with `Path::join`, so they carry the host's separator: on
//! the Mac the tests check the rules the functions guarantee on the host, and
//! on windows-2025 the same tests run with real backslashes. Where a rule is
//! about backslashes (the Desktop exclusions) the tests also pass literal
//! Windows strings, which `locator::is_desktop_owned` folds itself.
#![allow(dead_code)]

use agentnotch_engine::model::{
    Account, AccountId, ExpectedLogin, FolderKind, FolderSource, Identity, IdentityId, RingId,
    RunFolder,
};
use agentnotch_engine::platform::Roots;
use std::path::{Path, PathBuf};

/// A made-up drive (or root) nothing exists under: pure tests never touch it.
pub fn fake_drive() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" })
}

/// `base` with each of `parts` joined on.
pub fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(base.to_path_buf(), |path, p| path.join(p))
}

/// Roots in the Windows layout under `base` (a temporary folder, or a made-up
/// one for pure tests): `home` is `%USERPROFILE%`, `data` is
/// `%APPDATA%\Agent Notch`, `support` is
/// `%LOCALAPPDATA%\com.rivantmedia.agentnotch\Claude` and Claude Desktop's
/// root is `%APPDATA%\Claude`.
pub fn windows_roots(base: &Path) -> Roots {
    let home = join(base, &["Users", "me"]);
    let roaming = join(&home, &["AppData", "Roaming"]);
    let local = join(&home, &["AppData", "Local"]);
    Roots {
        data: roaming.join("Agent Notch"),
        support: join(&local, &["com.rivantmedia.agentnotch", "Claude"]),
        claude_desktop: vec![roaming.join("Claude")],
        system_users: Some(join(base, &["Users"])),
        install_dir: Some(join(&local, &["Programs", "Agent Notch"])),
        home,
    }
}

/// `%APPDATA%` of [`windows_roots`].
pub fn appdata(roots: &Roots) -> PathBuf {
    roots.data.parent().unwrap().to_path_buf()
}

/// `%LOCALAPPDATA%` of [`windows_roots`].
pub fn localappdata(roots: &Roots) -> PathBuf {
    join(&roots.home, &["AppData", "Local"])
}

/// A `PATH` value of `folders` in the host's list syntax.
pub fn path_list(folders: &[PathBuf]) -> std::ffi::OsString {
    std::env::join_paths(folders).expect("folders without the list separator")
}

/// A folder's id is its path as a string (`core::paths::Paths::normalize`'s
/// output in the real registry; the tests name the folders already normal).
pub fn folder_id(path: &Path) -> AccountId {
    AccountId::from(path.to_string_lossy().as_ref())
}

/// A folder Claude Code runs in. `env` is its `CLAUDE_CONFIG_DIR` as first
/// seen (`None` for the default folder).
pub fn run_folder(config_dir: &Path, env: Option<&str>) -> RunFolder {
    RunFolder {
        id: folder_id(config_dir),
        config_dir: config_dir.to_path_buf(),
        config_dir_env: env.map(str::to_owned),
        custom_label: None,
        seen_config_dir_envs: Vec::new(),
        identity: None,
        subscription_type: None,
        color_index: 0,
        source: FolderSource::Discovered,
        last_seen_at: None,
        is_hidden: false,
        kind: FolderKind::Run,
    }
}

/// A Claude Parallel Profiles account store: identity only, never run in.
pub fn store_folder(config_dir: &Path) -> RunFolder {
    RunFolder {
        kind: FolderKind::Store,
        config_dir_env: Some(config_dir.to_string_lossy().into_owned()),
        ..run_folder(config_dir, None)
    }
}

/// `run_folder` seen at `last_seen_at`.
pub fn seen(mut folder: RunFolder, last_seen_at: std::time::SystemTime) -> RunFolder {
    folder.last_seen_at = Some(last_seen_at);
    folder
}

/// What `.claude.json` says about who is signed in.
pub fn login(email: Option<&str>, account_uuid: Option<&str>, org: Option<&str>) -> Identity {
    Identity {
        email: email.map(str::to_owned),
        account_uuid: account_uuid.map(str::to_owned),
        organization_uuid: org.map(str::to_owned),
        ..Identity::default()
    }
}

/// An account (one ring) with the given folders; everything else plain.
pub fn account(
    identity_id: &str,
    email: Option<&str>,
    run_dirs: &[&RunFolder],
    store_dirs: &[&RunFolder],
) -> Account {
    Account {
        identity_id: IdentityId::from(identity_id),
        ring_id: RingId::from("claude-acct-000000000000"),
        label: email.unwrap_or(identity_id).to_owned(),
        own_label: None,
        monogram: "A".into(),
        color_index: 0,
        email: email.map(str::to_owned),
        plan_name: None,
        organization_uuid: None,
        run_dirs: run_dirs.iter().map(|f| f.id.clone()).collect(),
        store_dirs: store_dirs.iter().map(|f| f.id.clone()).collect(),
        includes_default: false,
        is_tracked: true,
        ring_shown: true,
        is_signed_in: true,
        launch_command: None,
        can_forget: false,
    }
}

/// The login a check must find in a folder.
pub fn expected(
    email: Option<&str>,
    account_uuid: Option<&str>,
    organization_scope: Option<&str>,
) -> ExpectedLogin {
    ExpectedLogin {
        email: email.map(str::to_owned),
        account_uuid: account_uuid.map(str::to_owned),
        organization_scope: organization_scope.map(str::to_owned),
    }
}

// ---- times and status lines (the store's tests) ----

use agentnotch_engine::model::{Attribution, SessionId, StatusLineMessage, UsageWindow};
use agentnotch_engine::runtime_types::{IngestContext, UsageObservation};
use agentnotch_engine::usage::UsageStore;
use std::time::{Duration, UNIX_EPOCH};

/// Whole seconds from the epoch (negative: before it).
pub fn t(seconds: i64) -> std::time::SystemTime {
    if seconds >= 0 {
        UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

/// The fixtures' "now": the Swift `UsageFixture.now`.
pub fn now() -> std::time::SystemTime {
    t(1_800_000_000)
}

pub fn ago(seconds: i64) -> std::time::SystemTime {
    t(1_800_000_000 - seconds)
}

pub fn plus(base: std::time::SystemTime, seconds: f64) -> std::time::SystemTime {
    base + Duration::from_secs_f64(seconds)
}

/// One status line update, like the Swift `line(...)`.
pub struct Line {
    pub five: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    pub process: u32,
    pub session: String,
    pub at: std::time::SystemTime,
    /// The kernel's start time of the process.
    pub start: Option<std::time::SystemTime>,
    pub folder: String,
    pub attribution: Attribution,
}

impl Line {
    pub fn started(mut self, start: std::time::SystemTime) -> Line {
        self.start = Some(start);
        self
    }

    pub fn session(mut self, session: &str) -> Line {
        self.session = session.to_owned();
        self
    }
}

/// Hands the store a status line the way the hub does: the pid is trusted.
pub fn feed(store: &mut UsageStore, l: Line) -> Option<UsageObservation> {
    let message = StatusLineMessage {
        session_id: SessionId::from(l.session.as_str()),
        cwd: None,
        transcript_path: None,
        config_dir_env: Some(l.folder.clone()),
        account_id: Some(l.folder.as_str().into()),
        received_at: l.at,
        rate_limits: None,
        five_hour: l.five.clone(),
        seven_day: l.weekly.clone(),
        context_used_percent: None,
        context_window_size: None,
        model_id: None,
        model_display_name: None,
        cost_usd: None,
        session_name: None,
        claude_code_version: None,
        pid: Some(l.process),
    };
    let ctx = IngestContext {
        attribution: l.attribution.clone(),
        account: None,
        trusted_pid: Some(l.process),
        pid_started: l.start,
    };
    store.ingest_status_line(&message, ctx, l.at)
}
