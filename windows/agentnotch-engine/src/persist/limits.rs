//! `limit-announcements.json` v1: which usage limits were announced, per
//! ring, so a relaunch doesn't tell a limit again (`control::limits`).
//! Compact, sorted keys, dates as ISO 8601 whole seconds (Swift's
//! `.iso8601`), as the Mac writes it.

use crate::core::time::IsoSeconds;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FILE_NAME: &str = "limit-announcements.json";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitAnnouncementsFile {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub incidents: BTreeMap<String, Vec<PersistedIncident>>,
}

fn version() -> u32 {
    VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedIncident {
    pub window: String,
    #[serde(rename = "resetsAt", default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<IsoSeconds>,
    #[serde(rename = "startedAt")]
    pub started_at: IsoSeconds,
    /// `notification` and `reaction`.
    pub channels: Vec<String>,
}

impl LimitAnnouncementsFile {
    /// `None` when the bytes aren't the file.
    pub fn parse(bytes: &[u8]) -> Option<LimitAnnouncementsFile> {
        serde_json::from_slice(bytes).ok()
    }

    pub fn encode(&self) -> Vec<u8> {
        super::encode_compact_sorted(self)
    }
}
