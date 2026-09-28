//! Usage readings (AU§8, UsageModels.swift).

use crate::model::IdentityId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// One limit window: how much of it is used (0..=100), when it resets, and
/// how long it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub utilization: f64,
    pub resets_at: Option<SystemTime>,
    pub duration_s: u64,
}

impl UsageWindow {
    /// The 5-hour session window.
    pub const SESSION_DURATION_S: u64 = 5 * 60 * 60;
    /// The weekly windows.
    pub const WEEKLY_DURATION_S: u64 = 7 * 24 * 60 * 60;

    /// Its reset time has passed.
    pub fn has_reset(&self, now: SystemTime) -> bool {
        self.resets_at.is_some_and(|resets| resets <= now)
    }

    /// What it reads now: 0 once it reset.
    pub fn effective_utilization(&self, now: SystemTime) -> f64 {
        if self.has_reset(now) {
            0.0
        } else {
            self.utilization
        }
    }
}

/// Where a reading came from. The cloud contract's names come only from
/// [`UsageSource::contract_name`], never from serde's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// Claude Code's own `get_usage`.
    Probe,
    /// A live session's status line `rate_limits`.
    StatusLine,
    /// `cachedUsageUtilization` in the account's `.claude.json`.
    Cache,
    /// Claude Desktop's HTTP cache.
    Desktop,
}

impl UsageSource {
    /// The name `web/contract` uses for `usage[].source`.
    pub fn contract_name(self) -> &'static str {
        match self {
            UsageSource::Probe => "probe",
            UsageSource::StatusLine => "statusLine",
            UsageSource::Cache => "claudeJson",
            UsageSource::Desktop => "desktop",
        }
    }
}

/// Pay-as-you-go credits beyond the plan (credits in minor units).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

/// A full usage snapshot for one identity (AU§8.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUsage {
    pub account_id: IdentityId,
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    /// Per-model weekly windows, by display name ("Opus", "Sonnet 4.5").
    pub scoped: Vec<(String, UsageWindow)>,
    pub extra_usage: Option<ExtraUsage>,
    pub subscription_type: Option<String>,
    pub source: UsageSource,
    pub updated_at: SystemTime,
    /// The data is newer than this (a probe's launch time).
    pub taken_after: Option<SystemTime>,
}

/// One dated reading of a window (AU§9.3): `at` is the latest the data can
/// be from, `not_before` the earliest when known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageReading {
    pub window: UsageWindow,
    pub at: SystemTime,
    pub not_before: Option<SystemTime>,
}

/// A ring's usage state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingStatus {
    Ok,
    Stale,
    Waiting,
    SignInNeeded,
    Unavailable,
    Failed,
}

impl RingStatus {
    /// The name the UI uses (`RingUsage::status`).
    pub fn as_str(self) -> &'static str {
        match self {
            RingStatus::Ok => "ok",
            RingStatus::Stale => "stale",
            RingStatus::Waiting => "waiting",
            RingStatus::SignInNeeded => "sign_in_needed",
            RingStatus::Unavailable => "unavailable",
            RingStatus::Failed => "failed",
        }
    }
}

/// Which disk cache backend Claude Desktop's `Cache_Data` uses (AU§12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopCacheFormat {
    /// Chromium's Simple Cache (`<16 hex>_0` files): readable.
    Simple,
    /// Chromium's blockfile cache (`index`, `data_0..3`): not read.
    Blockfile,
    /// No cache folder.
    Absent,
    Unknown,
}

/// One window of a Claude Desktop reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopWindow {
    /// `session`, `weekly_all`, `weekly_<slug>`.
    pub id: String,
    /// The model it is scoped to, when not the standard label.
    pub label: Option<String>,
    pub utilization: f64,
    pub resets_at: Option<SystemTime>,
    pub duration_s: u64,
}

/// What reading Claude Desktop's cache for one organization found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopReading {
    Reading {
        organization_uuid: String,
        windows: Vec<DesktopWindow>,
        observed_at: SystemTime,
    },
    /// No usable entry for the organization in a readable cache.
    NotFound {
        format: DesktopCacheFormat,
    },
    /// The cache is Chromium's blockfile format (DEGRADE, AU§12.2).
    UnsupportedFormat,
    Unavailable(String),
}
