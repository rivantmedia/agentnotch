//! Config folders, identities and accounts (AU§2-7).

use crate::model::{AccountId, IdentityId, RingId, SessionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// What a config folder is for (AU§2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderKind {
    /// Claude Code runs here: hooks, probes, sessions.
    Run,
    /// A Claude Parallel Profiles account store: identity and cached usage
    /// only, never hooked, probed or written.
    Store,
    /// Shared history: never an account.
    Infrastructure,
}

/// Who is signed in, from `.claude.json` → `oauthAccount` (AU§4.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    /// Claude Desktop keys its usage cache by it.
    pub organization_uuid: Option<String>,
    /// `claude_max`, `claude_pro`, `claude_team`, …
    pub organization_type: Option<String>,
    /// The organization's rate-limit tier, else the user's.
    pub rate_limit_tier: Option<String>,
    pub billing_type: Option<String>,
    pub has_extra_usage_enabled: Option<bool>,
}

impl Identity {
    /// `max`, `pro`, `team`, `enterprise`: `organization_type` lowercased
    /// without its `claude_` prefix.
    pub fn subscription_type(&self) -> Option<String> {
        let kind = self.organization_type.as_deref()?.to_lowercase();
        let plan = kind.strip_prefix("claude_").unwrap_or(&kind);
        (!plan.is_empty()).then(|| plan.to_owned())
    }
}

/// How a folder became known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderSource {
    /// Found scanning the home folder.
    Discovered,
    /// Seen in a hook event or status line.
    Hook,
    /// Added by the user.
    Manual,
}

/// One Claude config folder (the Mac's `ClaudeAccount`, AU§2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFolder {
    pub id: AccountId,
    pub config_dir: PathBuf,
    /// `CLAUDE_CONFIG_DIR` as first seen; `None` for the default folder.
    pub config_dir_env: Option<String>,
    pub custom_label: Option<String>,
    /// Collapsed by `core::paths::Paths::key` on Windows: there the login
    /// is a file inside the folder itself (reached by a case-insensitive
    /// path), not a Keychain item per spelling, so Windows never shows the
    /// "login conflict" chip nor runs the spelling-switching identity
    /// refresh; only set-versus-unset matters for the default folder
    /// (`~\.claude.json` versus `~\.claude\.claude.json`).
    pub seen_config_dir_envs: Vec<String>,
    pub identity: Option<Identity>,
    pub subscription_type: Option<String>,
    pub color_index: u8,
    pub source: FolderSource,
    pub last_seen_at: Option<SystemTime>,
    pub is_hidden: bool,
    pub kind: FolderKind,
}

/// One identity, one ring (the Mac's `ClaudeIdentityAccount`, AU§5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub identity_id: IdentityId,
    pub ring_id: RingId,
    /// The name shown everywhere.
    pub label: String,
    /// The user's own name for it, when set.
    pub own_label: Option<String>,
    pub monogram: String,
    pub color_index: u8,
    pub email: Option<String>,
    /// "Max 20x", "Pro", …
    pub plan_name: Option<String>,
    pub organization_uuid: Option<String>,
    /// Default first, then other run folders.
    pub run_dirs: Vec<AccountId>,
    /// Claude Parallel Profiles stores holding it.
    pub store_dirs: Vec<AccountId>,
    /// One of its run folders is `~\.claude`.
    pub includes_default: bool,
    /// "Track sessions and hooks".
    pub is_tracked: bool,
    /// "Ring in notch".
    pub ring_shown: bool,
    pub is_signed_in: bool,
    /// The PowerShell line that starts Claude Code as it (§4.2).
    pub launch_command: Option<String>,
    pub can_forget: bool,
}

/// A session was seen running in a folder (AU§3.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSighting {
    /// From `transcript_path`, else `CLAUDE_CONFIG_DIR`, else `~\.claude`.
    pub config_dir: AccountId,
    /// `CLAUDE_CONFIG_DIR` verbatim; `None` when unset.
    pub config_dir_env: Option<String>,
    pub session_id: SessionId,
    pub at: SystemTime,
}

/// What discovery knows of one folder (`ConfigDirSnapshot.Folder`, AU§2.6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderFacts {
    /// Normalized, display case.
    pub path: String,
    /// `<folder>\.claude.json` exists.
    pub has_global_config: bool,
    /// Its identity file names a claude.ai login (`core::claude_json::has_login`).
    pub is_signed_in: bool,
    /// `<folder>\.parallel-accounts-store` exists.
    pub has_store_marker: bool,
    pub has_projects: bool,
    pub has_sessions: bool,
    /// `sessions\` holds a `<pid>.json` whose process runs (never opened).
    pub has_live_session: bool,
    /// Where `projects` / `sessions` point when they are links: resolved.
    pub link_targets: Vec<String>,
    /// Known already (the registry, a hook, `AGENTNOTCH_EXTRA_CONFIG_DIRS`).
    pub is_explicit: bool,
}

/// Claude Parallel Profiles' manifest: inert on native Windows (AU§0.2), its
/// classifier kept and tested.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParallelProfilesManifest {
    pub stores: Vec<String>,
    pub created: Vec<String>,
}

/// One read of the home folder for discovery (`ConfigDirSnapshot`, AU§3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSnapshot {
    pub home: String,
    pub folders: Vec<FolderFacts>,
    pub manifest: Option<ParallelProfilesManifest>,
    /// The manifest exists but didn't parse (a write in progress).
    pub manifest_unreadable: bool,
}

impl FolderSnapshot {
    pub fn folder(&self, normalized_path: &str) -> Option<&FolderFacts> {
        self.folders.iter().find(|f| f.path == normalized_path)
    }
}

/// What the user asked of an account in Settings (`Call::Account`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountAction {
    /// `label: None` reverts to the default name.
    Rename {
        id: String,
        label: Option<String>,
    },
    /// "Track sessions and hooks".
    Track {
        id: String,
        on: bool,
    },
    /// "Ring in notch".
    RingShown {
        ring_id: String,
        on: bool,
    },
    /// Stop tracking it (nothing on disk is deleted; its hooks are removed).
    Forget {
        id: String,
    },
    /// "Add existing folder…".
    AddFolder {
        path: String,
    },
    /// "New account…": `%USERPROFILE%\.claude-<slug>`.
    Create {
        name: String,
    },
    SuggestionDismiss {
        path: String,
    },
    SuggestionAdd {
        path: String,
    },
}
