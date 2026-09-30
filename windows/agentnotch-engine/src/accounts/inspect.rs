//! What the app would make of a home folder's Claude Code folders, printed
//! for a person: `agentnotch.exe inspect-accounts` (ClaudeAccountInspection
//! .swift, AU§16). The same discovery, classification and grouping the app
//! runs, against the real home folder, strictly read-only:
//!
//! - directory listings (the home folder, `~\.claude-windows`, each folder's
//!   `sessions\`), and link targets;
//! - `~\.claude-windows\.manifest.json`, and whether a folder holds the
//!   `.parallel-accounts-store` marker;
//! - from each `.claude.json`, `oauthAccount` and, of
//!   `cachedUsageUtilization`, only `accountUuid` and `fetchedAtMs` (the rest
//!   of the file is skipped over byte by byte, never parsed).
//!
//! Nothing is written, no process is asked whether it runs, no credential,
//! settings.json or session file is read, and `claude` is never run.

use super::classify;
use super::folder::Folder;
use super::identities::{self, IdentityPrefs};
use super::snapshot::read_snapshot;
use crate::core::claude_json::{identity_from_oauth_account, lenient_number, ClaudeJsonReader};
use crate::core::json_scan;
use crate::core::time;
use crate::model::{FolderKind, FolderSource, Identity};
use crate::platform::{EnvRead, Liveness, ProcessTable, Processes, Roots, SecureFiles};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::SystemTime;

/// No process is asked anything.
struct NoProcesses;

impl Processes for NoProcesses {
    fn liveness(&self, _pid: u32) -> Liveness {
        Liveness::Unknown
    }
    fn start_time(&self, _pid: u32) -> Option<SystemTime> {
        None
    }
    fn table(&self) -> ProcessTable {
        ProcessTable::default()
    }
    fn config_dir_env(&self, _pid: u32) -> EnvRead {
        EnvRead::Unreadable
    }
    fn same_user(&self, _pid: u32) -> Option<bool> {
        None
    }
    fn elevated(&self, _pid: u32) -> Option<bool> {
        None
    }
    fn exe_path(&self, _pid: u32) -> Option<PathBuf> {
        None
    }
}

/// One folder's login, as its `.claude.json` names it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Login {
    pub identity: Option<Identity>,
    /// `cachedUsageUtilization.accountUuid` and `.fetchedAtMs`.
    pub cached_usage_account_uuid: Option<String>,
    pub cached_usage_fetched_at: Option<SystemTime>,
}

/// 9999-12-31T23:59:59Z in epoch milliseconds: the last moment the report
/// can write out.
const LATEST_FETCHED_AT_MS: f64 = 253_402_300_799_000.0;

/// Only the allowed fields of a `.claude.json`.
pub fn read_login(bytes: &[u8]) -> Option<Login> {
    let fields = json_scan::values(bytes, &["oauthAccount", "cachedUsageUtilization"])?;
    let mut login = Login::default();
    if let Some(raw) = fields.get("oauthAccount") {
        login.identity = serde_json::from_slice(raw)
            .ok()
            .as_ref()
            .and_then(identity_from_oauth_account);
    }
    if let Some(inner) = fields
        .get("cachedUsageUtilization")
        .and_then(|raw| json_scan::objects(raw, &["accountUuid", "fetchedAtMs"]))
    {
        login.cached_usage_account_uuid = inner
            .get("accountUuid")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        // A number that is no date anyone can print (before 1970, after the
        // year 9999) is a broken file, not a reading.
        login.cached_usage_fetched_at = inner
            .get("fetchedAtMs")
            .and_then(lenient_number)
            .filter(|ms| (0.0..=LATEST_FETCHED_AT_MS).contains(ms))
            .and_then(|ms| time::from_secs_f64(ms / 1000.0));
    }
    Some(login)
}

/// One account the inspection found.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectedAccount {
    pub label: String,
    pub email: Option<String>,
    pub account_uuid: Option<String>,
    pub plan: Option<String>,
    pub ring_id: String,
    pub run_dirs: Vec<String>,
    pub store_dirs: Vec<String>,
    /// Where Claude Code's freshest cached usage (of this identity) is.
    pub cached_usage_folder: Option<String>,
    pub cached_usage_fetched_at: Option<SystemTime>,
}

/// One folder found, with what it is and who its `.claude.json` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedFolder {
    pub path: String,
    /// `run` | `store`.
    pub kind: &'static str,
    pub email: Option<String>,
    pub account_uuid_prefix: Option<String>,
    /// Its UUID belongs to another account (a mirrored `.claude.json`): its
    /// email decided.
    pub corrected: bool,
}

/// What the inspection found. Paths are written `~\…`.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub home: String,
    pub extension_detected: bool,
    pub manifest_stores: Vec<String>,
    /// Profiles the extension adopted (in `stores`, not made by it): run folders.
    pub adopted_folders: Vec<String>,
    /// Who `~\.claude`'s own `accountUuid` names, when the extension has
    /// mirrored another account into it.
    pub default_owner: Option<String>,
    pub accounts: Vec<InspectedAccount>,
    pub infrastructure: Vec<String>,
    pub unsigned_folders: Vec<String>,
    /// Where "Turn on" installs: every run folder of a tracked account.
    pub install_targets: Vec<String>,
    pub folders: Vec<InspectedFolder>,
}

/// Discovers, classifies and groups the Claude Code folders of `roots.home`.
pub fn inspect(roots: &Roots, files: &dyn SecureFiles) -> Inspection {
    let paths = roots.paths();
    // A reader of its own: nothing the inspection parsed outlives it.
    let snapshot = read_snapshot(&paths, &[], files, &NoProcesses, &ClaudeJsonReader::new());
    let discovery = classify::discover(&snapshot, &[], &[], &paths);
    let layout = &discovery.layout;

    let mut logins: BTreeMap<String, Login> = BTreeMap::new();
    let mut folders: Vec<Folder> = Vec::new();
    for dir in &discovery.accounts {
        let mut folder = Folder::new(&paths, dir);
        let is_default = paths.is_default_config_dir(dir);
        folder.config_dir_env = (!is_default).then(|| folder.dir().to_owned());
        folder.source = FolderSource::Discovered;
        folder.kind = layout.kind(&paths, dir).unwrap_or(FolderKind::Run);
        let identity_file = folder.global_config_file(&paths);
        if let Some(login) = std::fs::read(&identity_file)
            .ok()
            .and_then(|bytes| read_login(&bytes))
        {
            folder.apply_identity(login.identity.clone());
            logins.insert(folder.dir().to_owned(), login);
        }
        folders.push(folder);
    }
    let grouping = identities::group(
        &folders,
        &BTreeMap::<String, IdentityPrefs>::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        layout.extension_detected,
        &paths,
    );

    let display = |path: &str| paths.abbreviate(path);
    let accounts = grouping
        .identities
        .iter()
        .map(|identity| {
            let uuid = identity.account_uuid.as_deref().map(str::to_lowercase);
            let cached = identity
                .folders()
                .filter_map(|folder| {
                    let login = logins.get(folder.dir())?;
                    let fetched = login.cached_usage_fetched_at?;
                    let matches = login
                        .cached_usage_account_uuid
                        .as_deref()
                        .map(str::to_lowercase)
                        == uuid;
                    matches.then(|| (folder.dir().to_owned(), fetched))
                })
                .max_by_key(|(_, fetched)| *fetched);
            InspectedAccount {
                label: identity.label(&paths),
                email: identity.email.clone(),
                account_uuid: identity.account_uuid.clone(),
                plan: identity.plan_name(),
                ring_id: identity.ring_id.as_str().to_owned(),
                run_dirs: identity.run_dirs.iter().map(|f| display(f.dir())).collect(),
                store_dirs: identity
                    .store_dirs
                    .iter()
                    .map(|f| display(f.dir()))
                    .collect(),
                cached_usage_folder: cached.as_ref().map(|(dir, _)| display(dir)),
                cached_usage_fetched_at: cached.map(|(_, fetched)| fetched),
            }
        })
        .collect();

    let install_targets = grouping
        .identities
        .iter()
        .filter(|identity| !identity.is_hidden)
        .flat_map(|identity| identity.run_dirs.iter())
        .chain(&grouping.unsigned_folders)
        .filter(|folder| folder.kind == FolderKind::Run)
        .map(|folder| display(folder.dir()))
        .collect();

    let default_dir = paths.default_config_dir();
    let default_is_mirrored = grouping.corrected_folders.contains(&default_dir);
    Inspection {
        home: paths.home().to_owned(),
        extension_detected: layout.extension_detected,
        manifest_stores: snapshot
            .manifest
            .iter()
            .flat_map(|manifest| &manifest.stores)
            .map(|dir| display(dir))
            .collect(),
        adopted_folders: layout.adopted().iter().map(|dir| display(dir)).collect(),
        default_owner: default_is_mirrored.then(|| {
            grouping
                .identities
                .iter()
                .find(|identity| Some(identity.id.as_str()) == grouping.default_owner.as_deref())
                .map_or_else(
                    || "nobody known".to_owned(),
                    |identity| identity.label(&paths),
                )
        }),
        accounts,
        infrastructure: layout
            .infrastructure()
            .iter()
            .map(|dir| display(dir))
            .collect(),
        unsigned_folders: grouping
            .unsigned_folders
            .iter()
            .map(|folder| display(folder.dir()))
            .collect(),
        install_targets,
        folders: folders
            .iter()
            .map(|folder| InspectedFolder {
                path: display(folder.dir()),
                kind: match folder.kind {
                    FolderKind::Store => "store",
                    _ => "run",
                },
                email: folder.email().map(str::to_owned),
                account_uuid_prefix: folder
                    .account_uuid()
                    .map(|uuid| uuid.chars().take(8).collect()),
                corrected: grouping.corrected_folders.contains(folder.dir()),
            })
            .collect(),
    }
}

/// The inspection as text.
pub fn report(roots: &Roots, files: &dyn SecureFiles) -> String {
    let result = inspect(roots, files);
    let paths = roots.paths();
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    };
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "Claude accounts in {} (read-only inspection)",
        result.home
    ));
    if result.extension_detected {
        lines.push(format!(
            "Claude Parallel Profiles: detected; manifest stores: {}",
            list(&result.manifest_stores)
        ));
        if !result.adopted_folders.is_empty() {
            lines.push(format!(
                "Adopted by it (your own profiles, run folders): {}",
                result.adopted_folders.join(", ")
            ));
        }
    } else {
        lines.push("Claude Parallel Profiles: not detected".to_owned());
    }
    lines.push(String::new());
    lines.push(format!("Accounts: {}", result.accounts.len()));
    for (index, account) in result.accounts.iter().enumerate() {
        let uuid = account
            .account_uuid
            .as_deref()
            .map_or_else(String::new, |uuid| {
                format!(
                    " (accountUuid {}…)",
                    uuid.chars().take(8).collect::<String>()
                )
            });
        let plan = account
            .plan
            .as_deref()
            .map_or_else(String::new, |plan| format!(" · {plan}"));
        lines.push(format!(
            "{}. {} — {}{uuid}{plan}",
            index + 1,
            account.label,
            account.email.as_deref().unwrap_or("not signed in")
        ));
        lines.push(format!("   ring:   {}", account.ring_id));
        lines.push(format!(
            "   runs:   {}",
            if account.run_dirs.is_empty() {
                "nowhere now".to_owned()
            } else {
                account.run_dirs.join(", ")
            }
        ));
        lines.push(format!("   stores: {}", list(&account.store_dirs)));
        match (
            &account.cached_usage_folder,
            account.cached_usage_fetched_at,
        ) {
            (Some(folder), Some(fetched)) => lines.push(format!(
                "   cached usage: freshest in {folder}, fetched {}",
                time::iso8601(fetched)
            )),
            _ => lines.push("   cached usage: none matching this account".to_owned()),
        }
    }
    lines.push(String::new());
    lines.push("Folders (each one's own oauthAccount):".to_owned());
    for folder in &result.folders {
        let mut line = format!(
            "   {} [{}] {}",
            folder.path,
            folder.kind,
            folder.email.as_deref().unwrap_or("not signed in")
        );
        if let Some(uuid) = &folder.account_uuid_prefix {
            line.push_str(&format!(" ({uuid}…)"));
        }
        if folder.corrected {
            line.push_str(
                " — its accountUuid is another account's (a mirrored .claude.json): its email decides",
            );
        }
        lines.push(line);
    }
    lines.push(String::new());
    lines.push(format!(
        "Infrastructure (never accounts): {}",
        list(&result.infrastructure)
    ));
    lines.push(format!(
        "Not signed in (no ring): {}",
        list(&result.unsigned_folders)
    ));
    lines.push(String::new());
    lines.push(format!(
        "Install targets after consent (hooks + status line): {}",
        result.install_targets.len()
    ));
    for target in &result.install_targets {
        // Written as the rest: `~\.claude\settings.json`.
        let separator = paths.style().separator();
        lines.push(format!("   {target}{separator}settings.json"));
    }
    if let Some(owner) = &result.default_owner {
        lines.push(String::new());
        lines.push(format!(
            "{}'s own accountUuid belongs to: {owner} (its saved choices go there)",
            paths.abbreviate(&paths.default_config_dir())
        ));
    }
    lines.join("\n")
}
