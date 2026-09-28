//! `review-state.json` v2 (HS§5.13): the review queue and failed turns, so
//! they survive restarts, and the heartbeat `lastAliveAt`. Compact, sorted
//! keys, dates as epoch seconds (doubles). Superpowered Vibe Notch's bare
//! `{sessionId: record}` shape is read too.
//!
//! Owner after WP0: WP5 (debounce, heartbeat, 7-day prune).

use crate::core::time::EpochSeconds;
use crate::model::ReviewItem;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FILE_NAME: &str = "review-state.json";
pub const VERSION: u32 = 2;
/// The longest `lastAssistantMessage` kept on disk.
pub const MAX_MESSAGE_LENGTH: usize = 1500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewStateFile {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(
        rename = "lastAliveAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_alive_at: Option<EpochSeconds>,
    pub sessions: BTreeMap<String, PersistedReviewRecord>,
}

fn version() -> u32 {
    VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedReviewRecord {
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<EpochSeconds>,
    #[serde(
        rename = "reviewedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reviewed_at: Option<EpochSeconds>,
    #[serde(
        rename = "lastAssistantMessage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_assistant_message: Option<String>,
    #[serde(rename = "stopError", default, skip_serializing_if = "Option::is_none")]
    pub stop_error: Option<String>,
    #[serde(
        rename = "stopErrorCode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_error_code: Option<String>,
    #[serde(rename = "failedAt", default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<EpochSeconds>,
    #[serde(
        rename = "backgroundWaitSince",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub background_wait_since: Option<EpochSeconds>,
    #[serde(
        rename = "backgroundAgentTypes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub background_agent_types: Option<Vec<String>>,
    #[serde(rename = "updatedAt")]
    pub updated_at: EpochSeconds,
}

impl ReviewStateFile {
    /// The file, or Superpowered Vibe Notch's bare map (read as version 2
    /// without a heartbeat). `None` when neither parses.
    pub fn parse(bytes: &[u8]) -> Option<ReviewStateFile> {
        if let Ok(file) = serde_json::from_slice::<ReviewStateFile>(bytes) {
            return Some(file);
        }
        let sessions =
            serde_json::from_slice::<BTreeMap<String, PersistedReviewRecord>>(bytes).ok()?;
        Some(ReviewStateFile {
            version: VERSION,
            last_alive_at: None,
            sessions,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        super::encode_compact_sorted(self)
    }
}

impl PersistedReviewRecord {
    /// `None` when `updatedAt` isn't a usable date.
    pub fn to_model(&self) -> Option<ReviewItem> {
        let time = |d: Option<EpochSeconds>| d.and_then(EpochSeconds::to_time);
        Some(ReviewItem {
            completed_at: time(self.completed_at),
            reviewed_at: time(self.reviewed_at),
            last_assistant_message: self.last_assistant_message.clone(),
            stop_error: self.stop_error.clone(),
            stop_error_code: self.stop_error_code.clone(),
            failed_at: time(self.failed_at),
            background_wait_since: time(self.background_wait_since),
            background_agent_types: self.background_agent_types.clone().unwrap_or_default(),
            updated_at: self.updated_at.to_time()?,
        })
    }

    pub fn from_model(item: &ReviewItem) -> Self {
        let date = |t: Option<std::time::SystemTime>| t.map(EpochSeconds::from_time);
        let message = item.last_assistant_message.as_ref().map(|m| {
            match m.char_indices().nth(MAX_MESSAGE_LENGTH) {
                Some((end, _)) => m[..end].to_owned(),
                None => m.clone(),
            }
        });
        PersistedReviewRecord {
            completed_at: date(item.completed_at),
            reviewed_at: date(item.reviewed_at),
            last_assistant_message: message,
            stop_error: item.stop_error.clone(),
            stop_error_code: item.stop_error_code.clone(),
            failed_at: date(item.failed_at),
            background_wait_since: date(item.background_wait_since),
            background_agent_types: (!item.background_agent_types.is_empty())
                .then(|| item.background_agent_types.clone()),
            updated_at: EpochSeconds::from_time(item.updated_at),
        }
    }
}
