//! `usage-state.json` v1 (AU§9.10): per identity, when Claude Code was last
//! asked, the failure backoff, the last full reading and each running
//! process's status line readings. ISO 8601 in whole seconds.
//!
//! Owner after WP0: WP4 (restoring: 8-day reading age, 7-day status lines).

use crate::core::time::IsoSeconds;
use crate::model::{AccountUsage, ExtraUsage, IdentityId, UsageReading, UsageSource, UsageWindow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

pub const FILE_NAME: &str = "usage-state.json";
pub const VERSION: u32 = 1;

/// The whole file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageStateFile {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub accounts: BTreeMap<String, PersistedUsageAccount>,
}

fn version() -> u32 {
    VERSION
}

impl Default for UsageStateFile {
    fn default() -> Self {
        UsageStateFile {
            version: VERSION,
            accounts: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PersistedUsageAccount {
    #[serde(
        rename = "lastProbeAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_probe_at: Option<IsoSeconds>,
    /// Consecutive failed or rate-limited probes.
    #[serde(rename = "failureCount", default)]
    pub failure_count: i64,
    #[serde(
        rename = "nextAttemptAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_attempt_at: Option<IsoSeconds>,
    #[serde(
        rename = "lastFullReading",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_full_reading: Option<PersistedAccountUsage>,
    #[serde(
        rename = "statusLines",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub status_lines: Option<Vec<PersistedStatusLine>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedWindow {
    /// 0..=100.
    pub utilization: f64,
    #[serde(rename = "resetsAt", default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<IsoSeconds>,
    /// Seconds.
    pub duration: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedScoped {
    pub name: String,
    pub window: PersistedWindow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedExtraUsage {
    #[serde(rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(
        rename = "monthlyLimit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub monthly_limit: Option<f64>,
    #[serde(
        rename = "usedCredits",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub used_credits: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// The Mac's `AccountUsage` as it encodes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedAccountUsage {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "fiveHour", default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<PersistedWindow>,
    #[serde(rename = "sevenDay", default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<PersistedWindow>,
    #[serde(default)]
    pub scoped: Vec<PersistedScoped>,
    #[serde(
        rename = "extraUsage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub extra_usage: Option<PersistedExtraUsage>,
    #[serde(
        rename = "subscriptionType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub subscription_type: Option<String>,
    /// `probe` | `statusLine` | `cache` (the Mac's three; `desktop` is
    /// written by Windows for Claude Desktop's readings).
    pub source: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: IsoSeconds,
    #[serde(
        rename = "takenAfter",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub taken_after: Option<IsoSeconds>,
}

/// One Claude Code process's status line record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedStatusLine {
    pub folder: String,
    /// `pid:<pid>@<epoch s of its start>`.
    pub key: String,
    pub readings: PersistedStatusLineReadings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedStatusLineReadings {
    #[serde(rename = "lastReportAt")]
    pub last_report_at: IsoSeconds,
    #[serde(rename = "fiveHour", default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<PersistedReading>,
    #[serde(rename = "sevenDay", default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<PersistedReading>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedReading {
    pub window: PersistedWindow,
    pub at: IsoSeconds,
    #[serde(rename = "notBefore", default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<IsoSeconds>,
}

impl PersistedUsageAccount {
    /// Nothing worth keeping.
    pub fn is_empty(&self) -> bool {
        self.last_probe_at.is_none()
            && self.failure_count == 0
            && self.next_attempt_at.is_none()
            && self.last_full_reading.is_none()
            && self.status_lines.as_ref().is_none_or(Vec::is_empty)
    }
}

impl UsageStateFile {
    /// Readings older than this aren't worth restoring: every window they
    /// describe has reset since.
    pub const MAX_RESTORED_READING_AGE: Duration = Duration::from_secs(8 * 24 * 60 * 60);

    /// The state as a new run should start from it: accounts that are gone
    /// dropped (when `known_account_ids` is given), and readings too old to
    /// describe any current window dropped.
    pub fn restored(
        &self,
        known_account_ids: Option<&BTreeSet<String>>,
        now: SystemTime,
    ) -> UsageStateFile {
        let mut copy = self.clone();
        if let Some(known) = known_account_ids {
            copy.accounts.retain(|id, _| known.contains(id));
        }
        for account in copy.accounts.values_mut() {
            let ancient = account.last_full_reading.as_ref().is_some_and(|reading| {
                now.duration_since(reading.updated_at.0)
                    .is_ok_and(|age| age > Self::MAX_RESTORED_READING_AGE)
            });
            if ancient {
                account.last_full_reading = None;
            }
        }
        copy.accounts.retain(|_, account| !account.is_empty());
        copy
    }

    /// `None` when it doesn't parse or is from a newer version: it is only a
    /// cache, the store starts fresh.
    pub fn parse(bytes: &[u8]) -> Option<UsageStateFile> {
        serde_json::from_slice::<UsageStateFile>(bytes)
            .ok()
            .filter(|file| file.version <= VERSION)
    }

    pub fn encode(&self) -> Vec<u8> {
        super::encode_pretty_sorted(self)
    }
}

impl PersistedWindow {
    pub fn from_model(window: &UsageWindow) -> Self {
        PersistedWindow {
            utilization: window.utilization,
            resets_at: window.resets_at.map(IsoSeconds),
            duration: window.duration_s as f64,
        }
    }

    pub fn to_model(&self) -> UsageWindow {
        UsageWindow {
            utilization: self.utilization,
            resets_at: self.resets_at.map(|d| d.0),
            duration_s: if self.duration.is_finite() && self.duration > 0.0 {
                self.duration.round() as u64
            } else {
                0
            },
        }
    }
}

impl PersistedReading {
    pub fn from_model(reading: &UsageReading) -> Self {
        PersistedReading {
            window: PersistedWindow::from_model(&reading.window),
            at: IsoSeconds(reading.at),
            not_before: reading.not_before.map(IsoSeconds),
        }
    }

    pub fn to_model(&self) -> UsageReading {
        UsageReading {
            window: self.window.to_model(),
            at: self.at.0,
            not_before: self.not_before.map(|d| d.0),
        }
    }
}

/// The file's name for a source.
pub fn source_name(source: UsageSource) -> &'static str {
    match source {
        UsageSource::Probe => "probe",
        UsageSource::StatusLine => "statusLine",
        UsageSource::Cache => "cache",
        UsageSource::Desktop => "desktop",
    }
}

/// A source from the file; an unknown name is a cache reading.
pub fn source_from_name(name: &str) -> UsageSource {
    match name {
        "probe" => UsageSource::Probe,
        "statusLine" => UsageSource::StatusLine,
        "desktop" => UsageSource::Desktop,
        _ => UsageSource::Cache,
    }
}

impl PersistedAccountUsage {
    pub fn from_model(usage: &AccountUsage) -> Self {
        PersistedAccountUsage {
            account_id: usage.account_id.0.clone(),
            five_hour: usage.five_hour.as_ref().map(PersistedWindow::from_model),
            seven_day: usage.seven_day.as_ref().map(PersistedWindow::from_model),
            scoped: usage
                .scoped
                .iter()
                .map(|(name, window)| PersistedScoped {
                    name: name.clone(),
                    window: PersistedWindow::from_model(window),
                })
                .collect(),
            extra_usage: usage.extra_usage.as_ref().map(|e| PersistedExtraUsage {
                is_enabled: e.is_enabled,
                monthly_limit: e.monthly_limit,
                used_credits: e.used_credits,
                utilization: e.utilization,
                currency: e.currency.clone(),
            }),
            subscription_type: usage.subscription_type.clone(),
            source: source_name(usage.source).into(),
            updated_at: IsoSeconds(usage.updated_at),
            taken_after: usage.taken_after.map(IsoSeconds),
        }
    }

    pub fn to_model(&self) -> AccountUsage {
        AccountUsage {
            account_id: IdentityId(self.account_id.clone()),
            five_hour: self.five_hour.as_ref().map(PersistedWindow::to_model),
            seven_day: self.seven_day.as_ref().map(PersistedWindow::to_model),
            scoped: self
                .scoped
                .iter()
                .map(|s| (s.name.clone(), s.window.to_model()))
                .collect(),
            extra_usage: self.extra_usage.as_ref().map(|e| ExtraUsage {
                is_enabled: e.is_enabled,
                monthly_limit: e.monthly_limit,
                used_credits: e.used_credits,
                utilization: e.utilization,
                currency: e.currency.clone(),
            }),
            subscription_type: self.subscription_type.clone(),
            source: source_from_name(&self.source),
            updated_at: self.updated_at.0,
            taken_after: self.taken_after.map(|d| d.0),
        }
    }
}
