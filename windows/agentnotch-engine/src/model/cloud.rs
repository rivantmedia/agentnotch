//! What the cloud thread shows the UI, and what it reads from the rest of
//! the engine (CL§5.9, §6.1, §7.2). The contract's own types (camelCase,
//! `web/contract`) live in `cloud`, never here.

use crate::model::IdentityId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudAuthState {
    SignedOut,
    SigningIn,
    SignedIn { email: Option<String> },
    Error { message: String },
}

/// The cloud section's state (ClaudeCloudState).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudState {
    /// The website's host, or `None` when the build has none.
    pub website_url: Option<String>,
    /// Set by `AGENTNOTCH_WEB_URL` for this run.
    pub website_is_overridden: bool,
    pub auth: CloudAuthState,
    pub sync_enabled: bool,
    pub summaries_enabled: bool,
    /// Summaries can run in this run (a `claude` to run, not `--no-install`).
    pub summaries_available: bool,
    pub is_syncing: bool,
    pub last_sync_at_ms: Option<u64>,
    pub last_error: Option<String>,
    pub pending_sessions: u32,
    pub pending_usage: u32,
    pub summarized_sessions: u32,
    pub dashboard_url: Option<String>,
    pub pools_url: Option<String>,
    pub settings_url: Option<String>,
}

impl Default for CloudState {
    fn default() -> Self {
        CloudState {
            website_url: None,
            website_is_overridden: false,
            auth: CloudAuthState::SignedOut,
            sync_enabled: false,
            summaries_enabled: false,
            summaries_available: false,
            is_syncing: false,
            last_sync_at_ms: None,
            last_error: None,
            pending_sessions: 0,
            pending_usage: 0,
            summarized_sessions: 0,
            dashboard_url: None,
            pools_url: None,
            settings_url: None,
        }
    }
}

/// A running session as the hub attributed it (SessionLedger.swift).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveSessionObservation {
    pub session_id: String,
    pub identity_id: IdentityId,
    /// `CloudKeys.accountKey(identity)`, filled by the cloud thread.
    pub account_key: String,
    /// The folder the session started in.
    pub cwd: String,
    pub transcript_path: Option<String>,
    pub config_dir: Option<String>,
    pub entrypoint: Option<String>,
    /// When the app first saw it running.
    pub started_at: SystemTime,
    pub last_activity_at: SystemTime,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub title: Option<String>,
    /// When the Claude Code process running it started.
    pub process_started_at: Option<SystemTime>,
    /// Its transcript is in a history other folders share (Claude Parallel
    /// Profiles): its lines from before this process may be another
    /// account's. `CloudSyncService` looks it up for sessions the ledger
    /// hasn't been told about (`SessionLedger::shared_history`); `None`
    /// otherwise.
    #[serde(default)]
    pub in_shared_history: Option<bool>,
}

/// A folder the backfill may read (CloudBackfill.Folder, CL§7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackfillFolder {
    pub config_dir: String,
    /// `None` for infrastructure folders: they only "reach" shared projects.
    pub identity_id: Option<IdentityId>,
    pub account_key: Option<String>,
    /// When `CloudFolderLogins` first saw it signed in as its account.
    pub signed_in_since: Option<SystemTime>,
}
