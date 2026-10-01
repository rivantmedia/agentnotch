//! The app ⇄ website API, version 1, as Rust values (CL§1; the Mac's
//! `CloudModels.swift`). The contract is `web/contract/README.md`; its
//! fixtures are what the tests encode and decode against, so a field renamed
//! here without the website breaks a test.
//!
//! - Dates are ISO 8601 in UTC with a `Z`. The app writes milliseconds,
//!   rounded (not truncated), and reads either form ([`date`]).
//! - Nullable fields are sent as an explicit `null` (the website's schemas
//!   require the key); `sessions[].summary` is left out when there is none,
//!   which tells the website to keep the summary it has.
//! - Keys that name a Claude account or a project are digests made here
//!   ([`super::keys`]): the website never sees an account UUID or a path.
//! - Every date in a request lies between 2023-01-01 and a day after now; a
//!   session or reading with a date outside is left out ([`SyncRequest::clamped`]).
//!   A window's reset time may lie up to 32 days ahead; one outside that is
//!   sent as null.
//! - What is encoded goes through [`to_json`], which sorts every object's
//!   keys itself: serde_json's map keeps insertion order when any crate in
//!   the app's build turns on its `preserve_order` feature, and the bytes
//!   are hashed ([`super::sync`]'s "what was sent" records).

use super::keys;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use unicode_segmentation::UnicodeSegmentation;

pub const SCHEMA_VERSION: u32 = 1;
/// Where Supabase sends the browser back after Google sign-in.
pub const REDIRECT_URL: &str = "agentnotch://auth-callback";
pub const CALLBACK_SCHEME: &str = "agentnotch";
pub const CALLBACK_HOST: &str = "auth-callback";

/// Upper bounds the website enforces (lengths in UTF-16 units, as its
/// schemas count them).
pub mod limit {
    pub const DEVICE_NAME: usize = 120;
    pub const APP_VERSION: usize = 40;
    pub const ACCOUNTS: usize = 50;
    pub const EMAIL: usize = 320;
    pub const ORGANIZATION_NAME: usize = 200;
    pub const PLAN: usize = 60;
    pub const LABEL: usize = 80;
    pub const SESSIONS: usize = 200;
    pub const PROJECT_NAME: usize = 120;
    pub const TITLE: usize = 200;
    pub const MODELS: usize = 10;
    pub const USAGE: usize = 500;
    pub const WINDOWS: usize = 20;
    pub const SUMMARY_TEXT: usize = 2000;
    pub const SUMMARY_MODEL: usize = 80;
    /// Not in the contract's table: what the website's schemas and columns
    /// hold (a model id, Postgres `integer`, JavaScript's largest exact
    /// integer, `numeric(14, 6)`).
    pub const MODEL_ID: usize = 200;
    pub const MESSAGE_COUNT: i64 = 2_147_483_647;
    pub const TOKEN_COUNT: i64 = 9_007_199_254_740_991;
    pub const COST_USD: f64 = 100_000_000.0;
}

/// Paths under the website's address.
pub mod path {
    pub const CONFIG: &str = "api/app/v1/config";
    pub const ME: &str = "api/app/v1/me";
    pub const SYNC: &str = "api/app/v1/sync";
}

/// 2023-01-01T00:00:00Z, the earliest date a request may carry.
pub const EARLIEST_DATE_S: u64 = 1_672_531_200;
/// How far past the clock a date may lie.
pub const FUTURE_ALLOWANCE: Duration = Duration::from_secs(24 * 60 * 60);
/// How far past the clock a usage window's `resetsAt` may lie (a weekly
/// window resets within a week).
pub const RESETS_AT_ALLOWANCE: Duration = Duration::from_secs(32 * 24 * 60 * 60);

pub fn earliest_date() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(EARLIEST_DATE_S)
}

/// A date the website takes: from 2023-01-01 to a day after `now`.
pub fn accepts(date: SystemTime, now: SystemTime) -> bool {
    date >= earliest_date() && date <= now + FUTURE_ALLOWANCE
}

/// A window's reset time the website takes: from 2023-01-01 to 32 days
/// after `now`.
pub fn accepts_resets_at(date: SystemTime, now: SystemTime) -> bool {
    date >= earliest_date() && date <= now + RESETS_AT_ALLOWANCE
}

/// Dates as the contract and the cloud's files write them.
pub mod date {
    use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Milliseconds since the epoch, rounded half away from zero (the Mac's
    /// `(t * 1000).rounded()`; a formatter would truncate a parsed `.482`
    /// into `.481`).
    pub fn rounded_ms(t: SystemTime) -> i64 {
        let ns: i128 = match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        };
        let ms = if ns >= 0 {
            (ns + 500_000) / 1_000_000
        } else {
            -((-ns + 500_000) / 1_000_000)
        };
        ms.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// `2026-09-25T09:47:03.120Z`.
    pub fn to_string(t: SystemTime) -> String {
        let ms = rounded_ms(t);
        match DateTime::<Utc>::from_timestamp_millis(ms) {
            Some(dt) => dt.to_rfc3339_opts(SecondsFormat::Millis, true),
            None => DateTime::<Utc>::UNIX_EPOCH.to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }

    /// ISO 8601 in UTC with a `Z`, with or without fractional seconds.
    /// Offsets are refused (the website refuses them too).
    pub fn parse(text: &str) -> Option<SystemTime> {
        let body = text.strip_suffix('Z')?;
        let naive = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"]
            .iter()
            .find_map(|format| NaiveDateTime::parse_from_str(body, format).ok())?;
        Some(naive.and_utc().into())
    }

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&to_string(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(d)?;
        parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("not an ISO 8601 date: {text}")))
    }

    /// The same for an optional date: `null` when there is none.
    pub mod option {
        use serde::{Deserialize, Deserializer, Serializer};
        use std::time::SystemTime;

        pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
            match t {
                Some(t) => s.serialize_str(&super::to_string(*t)),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<SystemTime>, D::Error> {
            match Option::<String>::deserialize(d)? {
                None => Ok(None),
                Some(text) => super::parse(&text).map(Some).ok_or_else(|| {
                    serde::de::Error::custom(format!("not an ISO 8601 date: {text}"))
                }),
            }
        }
    }
}

/// A date inside a list or a map of a cloud file (single fields use the
/// [`date`] adapters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp(pub SystemTime);

impl Serialize for Stamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        date::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        date::deserialize(d).map(Stamp)
    }
}

// ---- JSON ----

/// `value` as JSON with every object's keys sorted and slashes not escaped:
/// the Mac's `JSONEncoder` with `.sortedKeys, .withoutEscapingSlashes`.
pub fn encode_sorted(value: &Value) -> String {
    let mut out = String::new();
    write_sorted(value, &mut out);
    out
}

fn write_sorted(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()))
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_sorted(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
                out.push(':');
                write_sorted(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// A value's JSON bytes, keys sorted ([`encode_sorted`]).
pub fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    match serde_json::to_value(value) {
        Ok(value) => encode_sorted(&value).into_bytes(),
        Err(_) => b"null".to_vec(),
    }
}

// ---- Responses ----

/// `GET /api/app/v1/config`: what a sign-in needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResponse {
    pub supabase_url: String,
    pub supabase_publishable_key: String,
    pub redirect_url: String,
    pub dashboard_url: String,
}

/// `GET /api/app/v1/me`: the signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub user: MeUser,
    pub dashboard_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeUser {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /api/app/v1/sync` answered 200.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResponse {
    pub accepted: Accepted,
    #[serde(with = "date")]
    pub server_time: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    pub sessions: i64,
    pub usage: i64,
}

/// The website's error body: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

/// The error codes the contract names.
pub mod error_code {
    pub const UNAUTHORIZED: &str = "UNAUTHORIZED";
    pub const FORBIDDEN: &str = "FORBIDDEN";
    pub const BAD_REQUEST: &str = "BAD_REQUEST";
    pub const PAYLOAD_TOO_LARGE: &str = "PAYLOAD_TOO_LARGE";
    pub const RATE_LIMITED: &str = "RATE_LIMITED";
    pub const INTERNAL: &str = "INTERNAL";
}

// ---- The sync request ----

/// Where a session ran: `sessions[].source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionSource {
    Cli,
    Vscode,
    Desktop,
    Sdk,
    Other,
}

/// The entrypoints Claude Code writes a `hostSessionId` for: the sessions
/// Claude Desktop hosts (DesktopHostedSessions.swift).
pub const DESKTOP_HOSTED_ENTRYPOINTS: [&str; 3] =
    ["claude-desktop", "claude-desktop-3p", "local-agent"];

/// A session Claude Desktop hosts, by its entrypoint: any entrypoint naming
/// Desktop counts too (such a session carries no host id, so it is never
/// attributed).
pub fn is_desktop_hosted(entrypoint: Option<&str>) -> bool {
    let Some(value) = entrypoint.map(trim_spaces).map(str::to_lowercase) else {
        return false;
    };
    !value.is_empty()
        && (DESKTOP_HOSTED_ENTRYPOINTS.contains(&value.as_str()) || value.contains("desktop"))
}

impl SessionSource {
    /// From Claude Code's entrypoint (`CLAUDE_CODE_ENTRYPOINT`, the
    /// registry's or a transcript line's `entrypoint`): `cli`,
    /// `claude-vscode`, `claude-desktop`/`claude-desktop-3p`/`local-agent`
    /// (Claude Desktop hosts Claude Code sessions too), `sdk-ts`/`sdk-py`/
    /// `sdk-cli`. Unknown or missing is `other`.
    pub fn from_entrypoint(entrypoint: Option<&str>) -> SessionSource {
        let Some(value) = entrypoint.map(trim_spaces).map(str::to_lowercase) else {
            return SessionSource::Other;
        };
        if value.is_empty() {
            return SessionSource::Other;
        }
        if value == "cli" {
            return SessionSource::Cli;
        }
        if value.contains("vscode") {
            return SessionSource::Vscode;
        }
        if value.contains("desktop") || is_desktop_hosted(Some(&value)) {
            return SessionSource::Desktop;
        }
        if value.starts_with("sdk") {
            return SessionSource::Sdk;
        }
        SessionSource::Other
    }
}

/// Where a usage reading came from: `usage[].source`. Claude Desktop's
/// cache and Claude Code's `.claude.json` cache are told apart here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSourceName {
    Probe,
    StatusLine,
    ClaudeJson,
    Desktop,
}

impl UsageSourceName {
    /// The contract's name (what serde writes too).
    pub fn as_str(self) -> &'static str {
        match self {
            UsageSourceName::Probe => "probe",
            UsageSourceName::StatusLine => "statusLine",
            UsageSourceName::ClaudeJson => "claudeJson",
            UsageSourceName::Desktop => "desktop",
        }
    }

    /// The engine's source, through its explicit contract name (never
    /// serde's, DESIGN-WIN §3).
    pub fn from_model(source: crate::model::UsageSource) -> UsageSourceName {
        match source.contract_name() {
            "probe" => UsageSourceName::Probe,
            "statusLine" => UsageSourceName::StatusLine,
            "claudeJson" => UsageSourceName::ClaudeJson,
            _ => UsageSourceName::Desktop,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub schema_version: u32,
    pub device: SyncDevice,
    pub accounts: Vec<SyncAccount>,
    pub sessions: Vec<SyncSession>,
    pub usage: Vec<SyncReading>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDevice {
    pub id: String,
    pub name: String,
    pub app_version: String,
}

/// An account, with every nullable field written as an explicit null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAccount {
    pub key: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub organization_name: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncProject {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTokens {
    pub input: i64,
    pub output: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub text: String,
    pub model: String,
    #[serde(with = "date")]
    pub generated_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSession {
    pub account_key: String,
    pub session_id: String,
    pub project: SyncProject,
    #[serde(default)]
    pub title: Option<String>,
    pub source: SessionSource,
    pub models: Vec<String>,
    #[serde(with = "date")]
    pub started_at: SystemTime,
    #[serde(with = "date")]
    pub last_activity_at: SystemTime,
    #[serde(with = "date::option", default)]
    pub ended_at: Option<SystemTime>,
    pub message_count: i64,
    pub tokens: SyncTokens,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// Absent unless session summaries are on and one was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SyncSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncWindow {
    /// `session`, `weekly_all`, `weekly_<model>` or `extra_usage`.
    pub id: String,
    /// 0–100, may exceed 100.
    pub utilization: f64,
    #[serde(with = "date::option", default)]
    pub resets_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReading {
    pub account_key: String,
    pub source: UsageSourceName,
    /// When Claude Code or Claude Desktop took the reading.
    #[serde(with = "date")]
    pub observed_at: SystemTime,
    pub windows: Vec<SyncWindow>,
}

impl SyncRequest {
    pub fn new(device: SyncDevice) -> SyncRequest {
        SyncRequest {
            schema_version: SCHEMA_VERSION,
            device,
            accounts: Vec::new(),
            sessions: Vec::new(),
            usage: Vec::new(),
        }
    }

    /// The request with every string and list cut to the contract's
    /// limits, counts and token totals within what the website stores,
    /// numbers JSON can't carry (NaN, infinity) dropped, and any session or
    /// reading the website's schemas would refuse left out (one bad item
    /// fails the whole request there): an id that isn't a UUID, a key that
    /// isn't 64 hex digits, an account missing from `accounts`, a date
    /// before 2023 or more than a day after `now` (a summary with such a
    /// date goes; its session stays). What is sent is always this. A
    /// window's `resetsAt` may be up to 32 days ahead; one before 2023 or
    /// further ahead (a zero or a garbled value) is sent as null, and the
    /// reading goes with its utilization.
    pub fn clamped(&self, now: SystemTime) -> SyncRequest {
        let date_ok = |d: SystemTime| accepts(d, now);
        let mut copy = self.clone();
        copy.device.name = clamp_utf16(&self.device.name, limit::DEVICE_NAME);
        copy.device.app_version = clamp_utf16(&self.device.app_version, limit::APP_VERSION);
        copy.accounts = self
            .accounts
            .iter()
            .filter(|a| keys::is_key(&a.key))
            .take(limit::ACCOUNTS)
            .map(|a| SyncAccount {
                key: a.key.clone(),
                email: a.email.as_deref().map(|v| clamp_utf16(v, limit::EMAIL)),
                organization_name: a
                    .organization_name
                    .as_deref()
                    .map(|v| clamp_utf16(v, limit::ORGANIZATION_NAME)),
                plan: a.plan.as_deref().map(|v| clamp_utf16(v, limit::PLAN)),
                label: a.label.as_deref().map(|v| clamp_utf16(v, limit::LABEL)),
            })
            .collect();
        let known: std::collections::BTreeSet<&str> =
            copy.accounts.iter().map(|a| a.key.as_str()).collect();
        copy.sessions = self
            .sessions
            .iter()
            .filter(|s| {
                known.contains(s.account_key.as_str())
                    && keys::is_key(&s.project.key)
                    && keys::is_uuid(&s.session_id)
                    && date_ok(s.started_at)
                    && date_ok(s.last_activity_at)
                    && s.ended_at.is_none_or(date_ok)
            })
            .take(limit::SESSIONS)
            .map(|s| {
                let mut s = s.clone();
                s.project.name = clamp_utf16(&s.project.name, limit::PROJECT_NAME);
                s.title = s.title.as_deref().map(|t| clamp_utf16(t, limit::TITLE));
                let mut models: Vec<String> = Vec::new();
                for model in s.models.iter().map(|m| clamp_utf16(m, limit::MODEL_ID)) {
                    if !model.is_empty() && !models.contains(&model) {
                        models.push(model);
                    }
                }
                models.truncate(limit::MODELS);
                s.models = models;
                s.message_count = s.message_count.clamp(0, limit::MESSAGE_COUNT);
                let count = |v: i64| v.clamp(0, limit::TOKEN_COUNT);
                s.tokens = SyncTokens {
                    input: count(s.tokens.input),
                    output: count(s.tokens.output),
                    cache_creation: count(s.tokens.cache_creation),
                    cache_read: count(s.tokens.cache_read),
                };
                if let Some(cost) = s.cost_usd {
                    if !cost.is_finite() || cost < 0.0 || cost >= limit::COST_USD {
                        s.cost_usd = None;
                    }
                }
                if let Some(mut summary) = s.summary.take() {
                    summary.text = clamp_utf16(&summary.text, limit::SUMMARY_TEXT);
                    summary.model = clamp_utf16(&summary.model, limit::SUMMARY_MODEL);
                    s.summary = date_ok(summary.generated_at).then_some(summary);
                }
                if s.last_activity_at < s.started_at {
                    s.last_activity_at = s.started_at;
                }
                if let Some(ended) = s.ended_at {
                    if ended < s.last_activity_at {
                        s.ended_at = Some(s.last_activity_at);
                    }
                }
                s
            })
            .collect();
        copy.usage = self
            .usage
            .iter()
            .filter(|r| known.contains(r.account_key.as_str()) && date_ok(r.observed_at))
            .take(limit::USAGE)
            .filter_map(|r| {
                let mut r = r.clone();
                r.windows = r
                    .windows
                    .iter()
                    .filter(|w| w.utilization.is_finite() && keys::is_window_id(&w.id))
                    .map(|w| SyncWindow {
                        id: w.id.clone(),
                        utilization: w.utilization.max(0.0),
                        resets_at: w.resets_at.filter(|d| accepts_resets_at(*d, now)),
                    })
                    .take(limit::WINDOWS)
                    .collect();
                (!r.windows.is_empty()).then_some(r)
            })
            .collect();
        copy
    }
}

// ---- Text ----

/// Seconds from `earlier` to `later`, negative when `later` is the earlier
/// one (the Mac's `timeIntervalSince`).
pub fn seconds_between(later: SystemTime, earlier: SystemTime) -> f64 {
    match later.duration_since(earlier) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// At most `limit` UTF-16 units (what the website's schemas count), never
/// splitting a character (a grapheme cluster).
pub fn clamp_utf16(text: &str, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let size = grapheme.encode_utf16().count();
        if used + size > limit {
            break;
        }
        result.push_str(grapheme);
        used += size;
    }
    result
}

/// Spaces and tabs off both ends, never a newline (the Mac's
/// `trimmingCharacters(in: .whitespaces)`).
pub fn trim_spaces(text: &str) -> &str {
    text.trim_matches(|c: char| {
        c.is_whitespace()
            && !matches!(
                c,
                '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_to_the_millisecond() {
        let t = date::parse("2026-09-25T08:02:11.482Z").unwrap();
        assert_eq!(date::to_string(t), "2026-09-25T08:02:11.482Z");
        let whole = date::parse("2026-09-25T09:47:03Z").unwrap();
        assert_eq!(date::to_string(whole), "2026-09-25T09:47:03.000Z");
        assert_eq!(date::parse("yesterday"), None);
        assert_eq!(date::parse("2026-09-25T12:00:00+01:00"), None);
        let half = UNIX_EPOCH + Duration::from_nanos(1_790_000_000_000_500_000);
        assert_eq!(date::rounded_ms(half), 1_790_000_000_001);
    }

    #[test]
    fn sorted_json() {
        let value = serde_json::json!({"b": 1, "a": {"d": null, "c": "x/y"}});
        assert_eq!(encode_sorted(&value), r#"{"a":{"c":"x/y","d":null},"b":1}"#);
    }

    #[test]
    fn clamps_never_split_a_character() {
        assert_eq!(clamp_utf16("héllo", 3), "hél");
        // A flag is one character of four UTF-16 units.
        assert_eq!(clamp_utf16("ab🇫🇷", 5), "ab");
        assert_eq!(clamp_utf16("ab🇫🇷", 6), "ab🇫🇷");
    }
}
