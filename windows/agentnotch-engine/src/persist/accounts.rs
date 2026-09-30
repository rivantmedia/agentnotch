//! `accounts.json` v2 (AU§3.7): the folders, the per-identity preferences,
//! forgotten identities and the default folder's identity timeline. ISO 8601
//! dates in whole seconds; absent options left out.
//!
//! Owner after WP0: WP3 (`accounts::AccountRegistry` loads and writes it).

use crate::core::time::IsoSeconds;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FILE_NAME: &str = "accounts.json";
pub const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountsFile {
    #[serde(default = "version")]
    pub version: u32,
    pub accounts: Vec<PersistedFolder>,
    #[serde(rename = "removedIds", default)]
    pub removed_ids: Vec<String>,
    /// Per identity (`uuid:…`, `email:…`, `dir:…`): name, colour, tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identities: Option<BTreeMap<String, IdentityPrefs>>,
    #[serde(
        rename = "forgottenIdentities",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub forgotten_identities: Option<Vec<String>>,
    /// Who `~/.claude` ran as over time.
    #[serde(
        rename = "defaultIdentityTimeline",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub default_identity_timeline: Option<PersistedTimeline>,
}

fn version() -> u32 {
    VERSION
}

/// One config folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedFolder {
    pub id: String,
    #[serde(rename = "configDir")]
    pub config_dir: String,
    #[serde(
        rename = "configDirEnv",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub config_dir_env: Option<String>,
    #[serde(
        rename = "customLabel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_label: Option<String>,
    #[serde(rename = "colorIndex")]
    pub color_index: i64,
    #[serde(rename = "isHidden")]
    pub is_hidden: bool,
    /// `discovered` | `hook` | `manual`.
    pub source: String,
    #[serde(
        rename = "lastSeenAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen_at: Option<IsoSeconds>,
    #[serde(
        rename = "seenConfigDirEnvs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub seen_config_dir_envs: Option<Vec<String>>,
}

/// An identity's name, colour and "Track sessions and hooks".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityPrefs {
    #[serde(
        rename = "customLabel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_label: Option<String>,
    #[serde(rename = "colorIndex")]
    pub color_index: i64,
    #[serde(rename = "isHidden")]
    pub is_hidden: bool,
    /// "Ring in notch" off. The Mac keeps a ring's on/off in upstream's ring
    /// list; here the rings are the engine's, so it is the identity's choice
    /// like the rest. Written only when set, so a file the Mac wrote reads
    /// and writes back unchanged.
    #[serde(
        rename = "ringHidden",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub ring_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedTimeline {
    pub spans: Vec<PersistedSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedSpan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(rename = "rawUuid", default, skip_serializing_if = "Option::is_none")]
    pub raw_uuid: Option<String>,
    #[serde(rename = "isLogin", default)]
    pub is_login: bool,
    pub from: IsoSeconds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<IsoSeconds>,
    #[serde(rename = "lastSeen")]
    pub last_seen: IsoSeconds,
}

impl AccountsFile {
    /// `None` when the file doesn't parse (discovery then starts over).
    pub fn parse(bytes: &[u8]) -> Option<AccountsFile> {
        serde_json::from_slice(bytes).ok()
    }

    pub fn encode(&self) -> Vec<u8> {
        super::encode_pretty_sorted(self)
    }
}
