//! Parsing Claude Code's plan-usage payloads (UsageParser.swift, AU§8). Three
//! shapes reach the app, all read-only and none involving a token:
//!
//! - the account-usage body, found in `.claude.json` →
//!   `cachedUsageUtilization.utilization` and in the `get_usage` control
//!   response → `rate_limits` (which adds `model_scoped`):
//!   `five_hour` / `seven_day` `{utilization 0-100, resets_at ISO 8601}`,
//!   `seven_day_opus` / `seven_day_sonnet` (older per-model windows),
//!   `limits[]` `{kind session|weekly_all|weekly_scoped, percent, resets_at,
//!   scope.model.display_name}`, `model_scoped[]` `{display_name, utilization,
//!   resets_at}`, `extra_usage` `{is_enabled, monthly_limit, used_credits,
//!   utilization, currency}`;
//! - the `get_usage` response wrapper: `subscription_type`,
//!   `rate_limits_available`;
//! - the status line's `rate_limits`: `{five_hour|seven_day: {used_percentage,
//!   resets_at (epoch seconds)}}`.
//!
//! Everything is tolerant: numbers may arrive as integers, doubles or numeric
//! strings, dates as ISO 8601 with or without fractional seconds (or epoch
//! numbers), and any window may be null. Pure.

use crate::core::claude_json::{lenient_bool, CachedUsageRaw};
use crate::core::time;
use crate::model::{AccountUsage, ExtraUsage, IdentityId, UsageSource, UsageWindow};
use serde_json::{Map, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A usage body parsed into model types, before it is tied to an account.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedUsage {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub scoped: Vec<(String, UsageWindow)>,
    pub extra_usage: Option<ExtraUsage>,
    /// `max`, `pro`, … when the payload says.
    pub subscription_type: Option<String>,
    /// The answer may be Claude Code's persisted fallback rather than a
    /// fresh fetch. When its own usage request fails (a 429, typically),
    /// Claude Code answers `get_usage` with the snapshot it saved last, up to
    /// an hour old, and strips `limits` from it on the way out; the answer
    /// carries no fetch time. So a `get_usage` body without a `limits` array
    /// can't be dated by the probe.
    pub is_possibly_seeded: bool,
}

impl ParsedUsage {
    /// At least one window (session, weekly or scoped) was present.
    pub fn has_windows(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some() || !self.scoped.is_empty()
    }

    /// The same readings of the same windows as `other` (plan, extra usage
    /// and where it came from aside): how a seeded probe answer is matched to
    /// the dated copy in `.claude.json`. Reset times may differ by rounding.
    pub fn has_same_windows(&self, other: &ParsedUsage) -> bool {
        fn same(lhs: Option<&UsageWindow>, rhs: Option<&UsageWindow>) -> bool {
            match (lhs, rhs) {
                (None, None) => true,
                (Some(l), Some(r)) => {
                    if (l.utilization - r.utilization).abs() >= 0.5 {
                        return false;
                    }
                    match (l.resets_at, r.resets_at) {
                        (None, None) => true,
                        (Some(a), Some(b)) => seconds_between(a, b).abs() < 60.0,
                        _ => false,
                    }
                }
                _ => false,
            }
        }
        if !same(self.five_hour.as_ref(), other.five_hour.as_ref())
            || !same(self.seven_day.as_ref(), other.seven_day.as_ref())
        {
            return false;
        }
        let by_name = |scoped: &[(String, UsageWindow)]| {
            // The first reading of a name wins, as `uniquingKeysWith: first`.
            let mut map: Vec<(String, UsageWindow)> = Vec::new();
            for (name, window) in scoped {
                let key = name.to_lowercase();
                if !map.iter().any(|(k, _)| *k == key) {
                    map.push((key, window.clone()));
                }
            }
            map
        };
        let mine = by_name(&self.scoped);
        let theirs = by_name(&other.scoped);
        if mine.len() != theirs.len() {
            return false;
        }
        mine.iter().all(|(name, window)| {
            theirs
                .iter()
                .find(|(k, _)| k == name)
                .is_some_and(|(_, w)| same(Some(window), Some(w)))
        })
    }

    /// Tied to an account, from `source`, taken at `updated_at`.
    pub fn account_usage(
        &self,
        account_id: IdentityId,
        source: UsageSource,
        updated_at: SystemTime,
    ) -> AccountUsage {
        AccountUsage {
            account_id,
            five_hour: self.five_hour.clone(),
            seven_day: self.seven_day.clone(),
            scoped: self.scoped.clone(),
            extra_usage: self.extra_usage.clone(),
            subscription_type: self.subscription_type.clone(),
            source,
            updated_at,
            taken_after: None,
        }
    }
}

/// `cachedUsageUtilization` from an account's `.claude.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedUsageSnapshot {
    /// When Claude Code fetched it (`fetchedAtMs`).
    pub fetched_at: SystemTime,
    /// The account it belongs to; compared with `oauthAccount.accountUuid`.
    pub account_uuid: Option<String>,
    pub usage: ParsedUsage,
}

impl CachedUsageSnapshot {
    /// As a full snapshot: source `.claude.json`, dated when Claude Code
    /// fetched it. The account id is its own account UUID (`uuid:<…>` in
    /// lower case, empty when it names none) until the usage store ties it
    /// to an identity.
    pub fn account_usage(&self) -> AccountUsage {
        let id = self
            .account_uuid
            .as_deref()
            .map(|uuid| uuid.trim().to_lowercase())
            .filter(|uuid| !uuid.is_empty())
            .map(|uuid| format!("{}{uuid}", IdentityId::UUID_PREFIX))
            .unwrap_or_default();
        self.usage
            .account_usage(IdentityId(id), UsageSource::Cache, self.fetched_at)
    }
}

/// Outcome of a `get_usage` control response.
#[derive(Debug, Clone, PartialEq)]
pub enum GetUsageResult {
    Usage(ParsedUsage),
    /// Usage can't be had for this login (API key, not claude.ai, …).
    Unavailable(String),
    /// Claude Code's usage request was rate limited (HTTP 429 or a
    /// `rate_limit_error` body): back off.
    RateLimited,
    /// The payload didn't have the expected shape.
    Malformed(String),
}

// MARK: - Usage body

/// Parses an account-usage-shaped body (cache or probe).
pub fn parse_usage_body(body: &Map<String, Value>) -> ParsedUsage {
    let limits: Vec<&Map<String, Value>> = body
        .get("limits")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();

    let five_hour = window(body.get("five_hour"), UsageWindow::SESSION_DURATION_S)
        .or_else(|| limit_window(&limits, "session", UsageWindow::SESSION_DURATION_S));
    let seven_day = window(body.get("seven_day"), UsageWindow::WEEKLY_DURATION_S)
        .or_else(|| limit_window(&limits, "weekly_all", UsageWindow::WEEKLY_DURATION_S));

    // Scoped weekly limits, most specific source first; later sources only
    // add model families not seen yet (compared case-insensitively).
    let mut scoped: Vec<(String, UsageWindow)> = Vec::new();
    let mut add = |name: Option<&str>, window: Option<UsageWindow>| {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
            return;
        };
        let Some(window) = window else {
            return;
        };
        let lower = name.to_lowercase();
        if scoped.iter().any(|(n, _)| n.to_lowercase() == lower) {
            return;
        }
        scoped.push((name.to_owned(), window));
    };

    for limit in &limits {
        if limit.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
            continue;
        }
        let name = limit
            .get("scope")
            .and_then(Value::as_object)
            .and_then(|s| s.get("model"))
            .and_then(Value::as_object)
            .and_then(|m| m.get("display_name"))
            .and_then(Value::as_str);
        add(
            name,
            window_from(
                present(limit.get("percent")).or_else(|| limit.get("utilization")),
                limit.get("resets_at"),
                UsageWindow::WEEKLY_DURATION_S,
            ),
        );
    }
    if let Some(entries) = body.get("model_scoped").and_then(Value::as_array) {
        for entry in entries.iter().filter_map(Value::as_object) {
            add(
                entry.get("display_name").and_then(Value::as_str),
                window_from(
                    present(entry.get("utilization")).or_else(|| entry.get("percent")),
                    entry.get("resets_at"),
                    UsageWindow::WEEKLY_DURATION_S,
                ),
            );
        }
    }
    add(
        Some("Opus"),
        window(body.get("seven_day_opus"), UsageWindow::WEEKLY_DURATION_S),
    );
    add(
        Some("Sonnet"),
        window(body.get("seven_day_sonnet"), UsageWindow::WEEKLY_DURATION_S),
    );

    ParsedUsage {
        five_hour,
        seven_day,
        scoped,
        extra_usage: extra_usage(body.get("extra_usage")),
        subscription_type: body
            .get("subscription_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        is_possibly_seeded: false,
    }
}

/// Swift's `a ?? b` over JSON: a key that is present with `null` still
/// counts as present (`NSNull` is not nil), so it stays the choice.
fn present(value: Option<&Value>) -> Option<&Value> {
    value
}

/// `{utilization, resets_at}`; `None` for null, a non-object, or no
/// utilization.
pub fn window(value: Option<&Value>, duration_s: u64) -> Option<UsageWindow> {
    let object = value?.as_object()?;
    window_from(
        object.get("utilization"),
        object.get("resets_at"),
        duration_s,
    )
}

fn window_from(
    value: Option<&Value>,
    resets_at: Option<&Value>,
    duration_s: u64,
) -> Option<UsageWindow> {
    let utilization = number(value)?;
    Some(UsageWindow {
        utilization,
        resets_at: parse_date(resets_at),
        duration_s,
    })
}

fn limit_window(
    limits: &[&Map<String, Value>],
    kind: &str,
    duration_s: u64,
) -> Option<UsageWindow> {
    let limit = limits
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some(kind))?;
    window_from(
        present(limit.get("percent")).or_else(|| limit.get("utilization")),
        limit.get("resets_at"),
        duration_s,
    )
}

fn extra_usage(value: Option<&Value>) -> Option<ExtraUsage> {
    let object = value?.as_object()?;
    Some(ExtraUsage {
        is_enabled: object
            .get("is_enabled")
            .and_then(lenient_bool)
            .unwrap_or(false),
        monthly_limit: number(object.get("monthly_limit")),
        used_credits: number(object.get("used_credits")),
        utilization: number(object.get("utilization")),
        currency: object
            .get("currency")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

// MARK: - get_usage

/// Parses the inner `response` of a successful `get_usage` control response.
pub fn parse_get_usage_response(response: &Map<String, Value>) -> GetUsageResult {
    let subscription = response
        .get("subscription_type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    if response.get("rate_limits_available").and_then(lenient_bool) == Some(false) {
        return GetUsageResult::Unavailable("Usage limits aren't available for this login".into());
    }
    // Claude Code answers `rate_limits: null` (with `rate_limits_available:
    // true`) when its own usage fetch failed: a 429, a network error, an
    // expired login it couldn't refresh. It doesn't say which.
    let Some(rate_limits) = response.get("rate_limits").and_then(Value::as_object) else {
        return GetUsageResult::Malformed("Claude Code couldn't load usage right now".into());
    };
    if let Some(error) = rate_limits.get("error").and_then(Value::as_object) {
        if error.get("type").and_then(Value::as_str) == Some("rate_limit_error") {
            return GetUsageResult::RateLimited;
        }
        return GetUsageResult::Malformed(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Usage request failed")
                .to_owned(),
        );
    }

    let mut usage = parse_usage_body(rate_limits);
    if let Some(subscription) = subscription {
        usage.subscription_type = Some(subscription.to_owned());
    }
    usage.is_possibly_seeded = !rate_limits.get("limits").is_some_and(Value::is_array);
    if !usage.has_windows() {
        return GetUsageResult::Malformed("No usage windows in the response".into());
    }
    GetUsageResult::Usage(usage)
}

// MARK: - .claude.json cache

/// `cachedUsageUtilization` from a parsed `.claude.json`, if present.
pub fn parse_cached_usage(global_config: &Map<String, Value>) -> Option<CachedUsageSnapshot> {
    let cached = global_config.get("cachedUsageUtilization")?.as_object()?;
    let utilization = cached.get("utilization")?.as_object()?;
    let fetched_at_ms = number(cached.get("fetchedAtMs"))?;
    cached_snapshot(
        utilization,
        fetched_at_ms,
        cached.get("accountUuid").and_then(Value::as_str),
    )
}

/// The same from `core::claude_json`'s raw read of the file.
pub fn cached_usage_from_raw(raw: &CachedUsageRaw) -> Option<CachedUsageSnapshot> {
    cached_snapshot(
        raw.utilization.as_object()?,
        raw.fetched_at_ms?,
        raw.account_uuid.as_deref(),
    )
}

fn cached_snapshot(
    utilization: &Map<String, Value>,
    fetched_at_ms: f64,
    account_uuid: Option<&str>,
) -> Option<CachedUsageSnapshot> {
    let usage = parse_usage_body(utilization);
    if !usage.has_windows() {
        return None;
    }
    Some(CachedUsageSnapshot {
        fetched_at: time::from_secs_f64(fetched_at_ms / 1000.0)?,
        account_uuid: account_uuid.map(str::to_owned),
        usage,
    })
}

// MARK: - Status line

/// The status line's `rate_limits` object → the 5-hour and 7-day windows.
pub fn parse_status_line_rate_limits(
    value: Option<&Value>,
) -> (Option<UsageWindow>, Option<UsageWindow>) {
    let Some(rate_limits) = value.and_then(Value::as_object) else {
        return (None, None);
    };
    (
        status_line_window(
            rate_limits.get("five_hour"),
            UsageWindow::SESSION_DURATION_S,
        ),
        status_line_window(rate_limits.get("seven_day"), UsageWindow::WEEKLY_DURATION_S),
    )
}

/// `{used_percentage 0-100, resets_at epoch seconds}`.
pub fn status_line_window(value: Option<&Value>, duration_s: u64) -> Option<UsageWindow> {
    let object = value?.as_object()?;
    let used =
        number(present(object.get("used_percentage")).or_else(|| object.get("utilization")))?;
    Some(UsageWindow {
        utilization: used,
        resets_at: parse_date(object.get("resets_at")),
        duration_s,
    })
}

// MARK: - Scalars

/// A number from an integer, a double or a numeric string. Booleans are not
/// numbers here, and neither are NaN or infinities.
pub fn number(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim_matches([' ', '\t']).parse::<f64>().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

/// ISO 8601 (with or without fractional seconds, `Z` or an offset; no zone
/// is read as UTC), or an epoch number in seconds or milliseconds (anything
/// above 1e12 is milliseconds: 1e12 s is the year 33658).
pub fn parse_date(value: Option<&Value>) -> Option<SystemTime> {
    if let Some(seconds) = number(value) {
        let seconds = if seconds > 1e12 {
            seconds / 1000.0
        } else {
            seconds
        };
        return time::from_secs_f64(seconds);
    }
    let raw = value?.as_str()?.trim_matches([' ', '\t']);
    if raw.is_empty() || !raw.contains('T') {
        // Swift's ISO 8601 style needs the date and the time.
        return None;
    }
    time::parse_iso8601(raw)
}

/// Seconds from `b` to `a` (negative when `a` is earlier).
pub fn seconds_between(a: SystemTime, b: SystemTime) -> f64 {
    match a.duration_since(b) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// `t` plus a signed number of seconds (`t` itself for NaN, an infinity or
/// a span no `Duration` holds: `Duration::from_secs_f64` would panic).
pub fn offset(t: SystemTime, seconds: f64) -> SystemTime {
    let Ok(magnitude) = Duration::try_from_secs_f64(seconds.abs()) else {
        return t;
    };
    if seconds >= 0.0 {
        t.checked_add(magnitude).unwrap_or(t)
    } else {
        t.checked_sub(magnitude).unwrap_or(UNIX_EPOCH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_lenient_but_not_booleans() {
        assert_eq!(number(Some(&json!(true))), None);
        assert_eq!(number(Some(&json!(3))), Some(3.0));
        assert_eq!(number(Some(&json!(" 4.5 "))), Some(4.5));
        assert_eq!(number(Some(&json!("nan"))), None);
        assert_eq!(number(Some(&json!("inf"))), None);
        assert_eq!(number(Some(&Value::Null)), None);
    }

    #[test]
    fn a_null_percent_is_still_the_choice() {
        // `limit["percent"] ?? limit["utilization"]`: an explicit null percent
        // doesn't fall through to utilization, as in Swift.
        let body = json!({"limits": [{"kind": "session", "percent": null, "utilization": 5}]});
        let parsed = parse_usage_body(body.as_object().unwrap());
        assert_eq!(parsed.five_hour, None);
        let body = json!({"limits": [{"kind": "session", "utilization": 5}]});
        let parsed = parse_usage_body(body.as_object().unwrap());
        assert_eq!(parsed.five_hour.map(|w| w.utilization), Some(5.0));
    }
}
