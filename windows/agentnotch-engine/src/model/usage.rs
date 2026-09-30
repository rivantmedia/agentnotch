//! Usage readings (AU§8, UsageModels.swift).

use crate::model::IdentityId;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    pub fn new(utilization: f64, resets_at: Option<SystemTime>, duration_s: u64) -> UsageWindow {
        UsageWindow {
            utilization,
            resets_at,
            duration_s,
        }
    }

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

    /// The fraction used, clamped to 0..=1, for drawing.
    pub fn fraction(&self, now: SystemTime) -> f64 {
        (self.effective_utilization(now) / 100.0).clamp(0.0, 1.0)
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

/// Which used-up window an account waits on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitWindowKind {
    /// The 5-hour session window.
    Session,
    /// The 7-day window across all models.
    Weekly,
    /// A 7-day window for one model family ("Opus").
    Scoped(String),
}

/// A limit the account has used up: which window, and when it lifts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LimitHit {
    pub window: LimitWindowKind,
    /// `None` when the source didn't say.
    pub resets_at: Option<SystemTime>,
}

impl AccountUsage {
    /// Data older than this is shown as stale, at the default probe interval.
    pub const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
    /// With automatic probes off only status lines and caches bring news; an
    /// hour without any is when the reading stops being trustworthy.
    pub const STALE_AFTER_WITHOUT_PROBES: Duration = Duration::from_secs(60 * 60);

    /// An empty snapshot (no windows) from `source`, taken at `updated_at`.
    pub fn new(
        account_id: IdentityId,
        source: UsageSource,
        updated_at: SystemTime,
    ) -> AccountUsage {
        AccountUsage {
            account_id,
            five_hour: None,
            seven_day: None,
            scoped: Vec::new(),
            extra_usage: None,
            subscription_type: None,
            source,
            updated_at,
            taken_after: None,
        }
    }

    /// How old a reading may get before it counts as stale, for a probe
    /// interval in minutes (0 = automatic probes off): never less than 15
    /// minutes, and never less than one and a half probe intervals, so a
    /// reading isn't stale merely because the next probe isn't due yet.
    pub fn stale_threshold(probe_interval_minutes: u32) -> Duration {
        if probe_interval_minutes == 0 {
            return Self::STALE_AFTER_WITHOUT_PROBES;
        }
        let interval = Duration::from_secs(u64::from(probe_interval_minutes) * 60);
        Self::STALE_AFTER.max(interval.mul_f64(1.5))
    }

    /// Whether the reading is too old to trust. A used-up window is never
    /// stale before its reset: it cannot come down until then, however old
    /// the reading is.
    pub fn is_stale(&self, now: SystemTime, threshold: Duration) -> bool {
        let age = now
            .duration_since(self.updated_at)
            .unwrap_or(Duration::ZERO);
        age > threshold && self.limit_hit(now).is_none()
    }

    /// The used-up window the account is waiting on, if any: of the windows
    /// at or over 100% that haven't reset yet, the one that resets last (a
    /// session limit hit during a used-up week lifts before the week does).
    /// A window at 100% with no known reset still counts.
    pub fn limit_hit(&self, now: SystemTime) -> Option<LimitHit> {
        let mut candidates: Vec<LimitHit> = Vec::new();
        let mut consider = |window: Option<&UsageWindow>, kind: LimitWindowKind| {
            if let Some(window) = window.filter(|w| w.effective_utilization(now) >= 100.0) {
                candidates.push(LimitHit {
                    window: kind,
                    resets_at: window.resets_at,
                });
            }
        };
        consider(self.five_hour.as_ref(), LimitWindowKind::Session);
        consider(self.seven_day.as_ref(), LimitWindowKind::Weekly);
        for (name, window) in &self.scoped {
            consider(Some(window), LimitWindowKind::Scoped(name.clone()));
        }
        // Of equal resets the first stays, as Swift's `max(by:)` keeps it
        // (Rust's `max_by_key` would take the last).
        let mut best: Option<LimitHit> = None;
        for hit in candidates {
            let key = hit.resets_at.map_or(FAR_FUTURE_KEY, sort_key);
            if best
                .as_ref()
                .is_none_or(|b| b.resets_at.map_or(FAR_FUTURE_KEY, sort_key) < key)
            {
                best = Some(hit);
            }
        }
        best
    }
}

/// A sort key for times (`None` sorts as the distant future).
fn sort_key(t: SystemTime) -> i128 {
    crate::core::time::to_ns(t)
}

const FAR_FUTURE_KEY: i128 = i128::MAX;

/// The distant future, for "no deadline".
pub fn distant_future() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(64_092_211_200) // 4001-01-01
}

/// The distant past, for "never".
pub fn distant_past() -> SystemTime {
    UNIX_EPOCH
        .checked_sub(Duration::from_secs(62_135_596_800)) // 0001-01-01
        .unwrap_or(UNIX_EPOCH)
}

/// One dated reading of a window (AU§9.3): `at` is the latest the data can
/// be from, `not_before` the earliest when known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageReading {
    pub window: UsageWindow,
    pub at: SystemTime,
    pub not_before: Option<SystemTime>,
}

impl UsageReading {
    pub fn new(window: UsageWindow, at: SystemTime, not_before: Option<SystemTime>) -> Self {
        UsageReading {
            window,
            at,
            not_before,
        }
    }

    /// A full snapshot's window, taken when the snapshot was: at its
    /// `updated_at`, and no earlier than its `taken_after` (never after its
    /// `updated_at`).
    pub fn of_snapshot(window: UsageWindow, snapshot: &AccountUsage) -> Self {
        let not_before = snapshot
            .taken_after
            .unwrap_or(snapshot.updated_at)
            .min(snapshot.updated_at);
        UsageReading::new(window, snapshot.updated_at, Some(not_before))
    }
}

/// What one Claude Code process last said in its status line: its windows,
/// dated by `usage::merge::advance`, and when it last reported
/// (`UsageStore.StatusLineReadings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusLineRecord {
    pub last_report_at: SystemTime,
    pub five_hour: Option<UsageReading>,
    pub seven_day: Option<UsageReading>,
}

/// Fetch status for one account, shown next to its usage (UsageFetchState).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFetchState {
    Idle,
    /// Being probed, or waiting for a requested probe.
    Fetching,
    /// The last attempt failed; the message is short and user-facing.
    Failed(String),
    /// Usage can't be fetched for this account (not signed in, an API-key
    /// login, no folder to check it in).
    Unavailable(String),
}

/// Who a folder must be signed in as for a usage check (or a session
/// summary) to run in it: the identity's login, as the folder's own
/// `.claude.json` would name it. Checked right before and after the run
/// (`usage::planner::folder_runs`), so a check never spends, or answers
/// for, another account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExpectedLogin {
    pub email: Option<String>,
    /// From a `uuid:` identity id.
    pub account_uuid: Option<String>,
    /// Set when one login has several organizations: the one this identity is.
    pub organization_scope: Option<String>,
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

impl DesktopCacheFormat {
    /// `simple` | `blockfile` | `absent` | `unknown` (Settings, the doctor).
    pub fn as_str(self) -> &'static str {
        match self {
            DesktopCacheFormat::Simple => "simple",
            DesktopCacheFormat::Blockfile => "blockfile",
            DesktopCacheFormat::Absent => "absent",
            DesktopCacheFormat::Unknown => "unknown",
        }
    }
}

/// One window of a Claude Desktop reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopWindow {
    /// `session`, `weekly_all`, `weekly_<slug>`.
    pub id: String,
    /// The model it is scoped to, when not the standard label.
    pub label: Option<String>,
    /// 0..=100 (can exceed 100 over the limit).
    pub utilization: f64,
    pub resets_at: Option<SystemTime>,
    /// 0 when the window's length isn't known.
    pub duration_s: u64,
}

/// One grant of unused resets (`cedar_ember.grants[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetGrant {
    pub id: String,
    pub resets_left: i64,
    pub starts_at: SystemTime,
    pub ends_at: SystemTime,
    pub paused: bool,
}

/// The `cedar_ember` block of Claude's usage response (ClaudeResetCredits):
/// the unused rate-limit resets an account has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetCredits {
    pub eligible: bool,
    pub ineligible_reason: Option<String>,
    pub grants: Vec<ResetGrant>,
}

/// One available credit, as the hover card lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetCredit {
    pub id: String,
    pub expires_at: Option<SystemTime>,
    pub count: i64,
}

/// What the hover card shows of unused resets (UsageResetCredits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableResets {
    pub available_count: i64,
    pub credits: Vec<ResetCredit>,
    /// When Claude Desktop saw them: a cached observation stays dated even
    /// when usage itself refreshes.
    pub checked_at: Option<SystemTime>,
}

impl AvailableResets {
    /// The soonest expiry of an available credit.
    pub fn next_expiry(&self) -> Option<SystemTime> {
        self.credits.iter().filter_map(|c| c.expires_at).min()
    }
}

impl ResetCredits {
    /// The resets usable at `now`. `None` when it can't be told: an account
    /// the surface isn't offered to (`ineligible_reason == "surface"`) is
    /// unknown, not "none left"; so is a total that overflows.
    pub fn credits(&self, now: SystemTime) -> Option<AvailableResets> {
        if !self.eligible && self.ineligible_reason.as_deref() == Some("surface") {
            return None;
        }
        let available: Vec<&ResetGrant> = if self.eligible {
            self.grants
                .iter()
                .filter(|g| g.resets_left > 0 && !g.paused && g.starts_at <= now && g.ends_at > now)
                .collect()
        } else {
            Vec::new()
        };
        let mut count: i64 = 0;
        for grant in &available {
            count = count.checked_add(grant.resets_left)?;
        }
        Some(AvailableResets {
            available_count: count,
            credits: available
                .iter()
                .map(|g| ResetCredit {
                    id: g.id.clone(),
                    expires_at: Some(g.ends_at),
                    count: g.resets_left,
                })
                .collect(),
            checked_at: None,
        })
    }
}

/// A `cedar_ember` block as Claude Desktop last saw it (`ResetReading`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopResets {
    /// `None`: the response carried a block that didn't parse, which still
    /// supersedes an older one.
    pub value: Option<ResetCredits>,
    pub captured_at: SystemTime,
}

impl DesktopResets {
    /// The resets to show at `now`, dated by when Desktop saw them. Desktop
    /// only refreshes grants while its Usage settings are open, so an old
    /// observation is kept (with its date) until a newer response replaces
    /// it or the grant expires; only a capture from the future (a clock that
    /// jumped) is refused.
    pub fn credits(&self, now: SystemTime) -> Option<AvailableResets> {
        let ahead = self
            .captured_at
            .duration_since(now)
            .unwrap_or(Duration::ZERO);
        if ahead >= Duration::from_secs(30 * 60) {
            return None;
        }
        let mut credits = self.value.as_ref()?.credits(now)?;
        credits.checked_at = Some(self.captured_at);
        Some(credits)
    }
}

/// What reading Claude Desktop's cache for one organization found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopReading {
    Reading {
        organization_uuid: String,
        windows: Vec<DesktopWindow>,
        observed_at: SystemTime,
        /// The newest `cedar_ember` block among the entries looked at, dated
        /// by its own entry.
        #[serde(default)]
        resets: Option<DesktopResets>,
    },
    /// No usable entry for the organization in a readable cache.
    NotFound {
        format: DesktopCacheFormat,
    },
    /// The cache is Chromium's blockfile format (DEGRADE, AU§12.2).
    UnsupportedFormat,
    Unavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::from_ms;

    #[test]
    fn distant_times_order() {
        assert!(distant_past() < UNIX_EPOCH);
        assert!(distant_future() > from_ms(4_000_000_000_000));
    }

    #[test]
    fn stale_threshold_follows_the_interval() {
        // A3_RingReadingTests.staleThresholdFollowsTheProbeInterval
        let minutes = |m: u32| AccountUsage::stale_threshold(m).as_secs_f64() / 60.0;
        assert_eq!(minutes(5), 15.0);
        assert_eq!(minutes(10), 15.0);
        assert_eq!(minutes(15), 22.5);
        assert_eq!(minutes(30), 45.0);
        assert_eq!(
            AccountUsage::stale_threshold(0),
            AccountUsage::STALE_AFTER_WITHOUT_PROBES
        );
        assert!(AccountUsage::STALE_AFTER_WITHOUT_PROBES > Duration::from_secs(45 * 60));
    }
}
