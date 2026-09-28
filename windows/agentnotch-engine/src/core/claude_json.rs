//! Reads the parts of an account's `.claude.json` the engine needs: the
//! signed-in identity (`oauthAccount`) and Claude Code's cached usage
//! (`cachedUsageUtilization`). Nothing else in the file is kept
//! (ClaudeGlobalConfigReader.swift, AU§4).
//!
//! The file can be several MB and Claude Code rewrites it often, so a parse
//! is cached per path and redone only when the file's modification time or
//! size changes; a reparse that fails (a read caught mid-write) keeps the
//! last good parse. The file is read whole into memory with the default
//! share modes, never memory-mapped: a mapping would make Claude Code's
//! replace of the file fail on Windows (AU§4.4).

use crate::core::json_scan;
use crate::core::time;
use crate::model::Identity;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

/// The only top-level keys of `.claude.json` the engine reads.
pub const READ_KEYS: [&str; 2] = ["oauthAccount", "cachedUsageUtilization"];

/// `cachedUsageUtilization` as the file holds it; `usage` parses the body
/// (UsageParser's rules, WP4).
#[derive(Debug, Clone, PartialEq)]
pub struct CachedUsageRaw {
    /// `fetchedAtMs`.
    pub fetched_at_ms: Option<f64>,
    /// Whose reading it is: a `/login` to another account leaves the old one
    /// behind.
    pub account_uuid: Option<String>,
    /// The usage body (`utilization`).
    pub utilization: Value,
}

impl CachedUsageRaw {
    pub fn fetched_at(&self) -> Option<SystemTime> {
        self.fetched_at_ms
            .and_then(|ms| time::from_secs_f64(ms / 1000.0))
    }
}

/// The subset of `.claude.json` the engine uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeGlobalConfig {
    /// `None` when nobody is signed in to claude.ai (or with an API key).
    pub identity: Option<Identity>,
    pub cached_usage: Option<CachedUsageRaw>,
    /// The file's modification time when it was parsed.
    pub modified_at: Option<SystemTime>,
}

impl ClaudeGlobalConfig {
    /// Only [`READ_KEYS`] are parsed; `None` when the bytes aren't a JSON
    /// object.
    pub fn parse(bytes: &[u8], modified_at: Option<SystemTime>) -> Option<ClaudeGlobalConfig> {
        let fields = json_scan::objects(bytes, &READ_KEYS)?;
        Some(ClaudeGlobalConfig {
            identity: fields
                .get("oauthAccount")
                .and_then(identity_from_oauth_account),
            cached_usage: fields.get("cachedUsageUtilization").and_then(cached_usage),
            modified_at,
        })
    }

    /// The cached usage, only when it belongs to the signed-in account.
    pub fn matching_cached_usage(&self) -> Option<&CachedUsageRaw> {
        let cached = self.cached_usage.as_ref()?;
        let expected = self.identity.as_ref()?.account_uuid.as_deref()?;
        (cached.account_uuid.as_deref() == Some(expected)).then_some(cached)
    }
}

/// An `oauthAccount` object as an identity; `None` when it names neither an
/// account UUID nor an email. Empty strings count as absent.
pub fn identity_from_oauth_account(value: &Value) -> Option<Identity> {
    let object = value.as_object()?;
    let string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let identity = Identity {
        account_uuid: string("accountUuid"),
        email: string("emailAddress"),
        display_name: string("displayName"),
        organization_name: string("organizationName"),
        organization_uuid: string("organizationUuid"),
        organization_type: string("organizationType"),
        rate_limit_tier: string("organizationRateLimitTier")
            .or_else(|| string("userRateLimitTier")),
        billing_type: string("billingType"),
        has_extra_usage_enabled: object.get("hasExtraUsageEnabled").and_then(lenient_bool),
    };
    (identity.account_uuid.is_some() || identity.email.is_some()).then_some(identity)
}

fn cached_usage(value: &Value) -> Option<CachedUsageRaw> {
    let object = value.as_object()?;
    let utilization = object.get("utilization").filter(|u| u.is_object())?.clone();
    Some(CachedUsageRaw {
        fetched_at_ms: object.get("fetchedAtMs").and_then(lenient_number),
        account_uuid: object
            .get("accountUuid")
            .and_then(Value::as_str)
            .map(str::to_owned),
        utilization,
    })
}

/// UsageParser's `bool`: a JSON bool or number, or `true`/`1`/`yes`,
/// `false`/`0`/`no` as text.
pub fn lenient_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => match s.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// UsageParser's `number`: a finite JSON number (not a bool) or numeric text.
pub fn lenient_number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

/// Whether a `.claude.json`'s bytes hold an `oauthAccount` object with
/// something in it: a byte scan (the file can be megabytes; a full parse
/// isn't needed to know). `{}` is no login.
pub fn has_login(bytes: &[u8]) -> bool {
    const KEY: &[u8] = b"\"oauthAccount\"";
    let is_space = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
    let mut from = 0;
    while let Some(found) = find(&bytes[from..], KEY).map(|at| from + at) {
        let mut index = found + KEY.len();
        while bytes.get(index).copied().is_some_and(is_space) {
            index += 1;
        }
        if bytes.get(index) == Some(&b':') {
            index += 1;
            while bytes.get(index).copied().is_some_and(is_space) {
                index += 1;
            }
            if bytes.get(index) == Some(&b'{') {
                index += 1;
                while bytes.get(index).copied().is_some_and(is_space) {
                    index += 1;
                }
                return bytes.get(index).is_some_and(|&b| b != b'}');
            }
        }
        from = found + KEY.len();
    }
    false
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// What a file looked like when it was parsed: (mtime ns, size).
pub type Stamp = (i128, u64);

struct Entry {
    stamp: Stamp,
    config: ClaudeGlobalConfig,
}

/// A shared reader: the registry and the usage store read the same files,
/// and a change is parsed once. Blocking; runs on the `an-io` lanes.
#[derive(Default)]
pub struct ClaudeJsonReader {
    cache: Mutex<HashMap<String, Entry>>,
}

impl ClaudeJsonReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// The parsed file, or `None` when it doesn't exist or has never parsed.
    pub fn read(&self, path: &Path) -> Option<ClaudeGlobalConfig> {
        self.read_stamped(path).map(|(config, _)| config)
    }

    /// [`ClaudeJsonReader::read`] with the stamp of the parse returned.
    pub fn read_stamped(&self, path: &Path) -> Option<(ClaudeGlobalConfig, Stamp)> {
        let key = path.to_string_lossy().into_owned();
        let Ok(metadata) = std::fs::metadata(path) else {
            self.lock().remove(&key);
            return None;
        };
        let modified = metadata.modified().ok();
        let stamp: Stamp = (modified.map_or(i128::MIN, time::to_ns), metadata.len());
        if let Some(entry) = self.lock().get(&key).filter(|entry| entry.stamp == stamp) {
            return Some((entry.config.clone(), entry.stamp));
        }
        // Only the two keys are parsed; the rest of the file is skipped over.
        let parsed = std::fs::read(path)
            .ok()
            .and_then(|bytes| ClaudeGlobalConfig::parse(&bytes, modified));
        let mut cache = self.lock();
        match parsed {
            Some(config) => {
                cache.insert(
                    key,
                    Entry {
                        stamp,
                        config: config.clone(),
                    },
                );
                Some((config, stamp))
            }
            // Caught mid-write: the last good parse, and the next call tries again.
            None => cache
                .get(&key)
                .map(|entry| (entry.config.clone(), entry.stamp)),
        }
    }

    /// Forget every cached parse.
    pub fn invalidate(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        // A panic while holding the lock can't leave the map half-written.
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
