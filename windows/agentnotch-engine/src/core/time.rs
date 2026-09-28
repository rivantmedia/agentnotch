//! Time conversions and the date encodings of the files the engine shares
//! with the Mac: ISO 8601 in whole seconds (`JSONEncoder.dateEncodingStrategy
//! = .iso8601`), epoch seconds as a double (`.secondsSince1970`), and the
//! cloud's ISO 8601 with milliseconds. UI JSON carries epoch milliseconds.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds between 1601-01-01 (FILETIME's epoch) and 1970-01-01.
const FILETIME_UNIX_OFFSET_S: u64 = 11_644_473_600;

/// Epoch milliseconds (0 before 1970).
pub fn to_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64)
}

pub fn from_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// Epoch seconds, negative before 1970.
pub fn to_secs_f64(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// `None` for NaN, infinities and values out of `SystemTime`'s range.
pub fn from_secs_f64(secs: f64) -> Option<SystemTime> {
    if !secs.is_finite() || secs.abs() > 1.0e13 {
        return None;
    }
    let magnitude = Duration::from_secs_f64(secs.abs());
    if secs >= 0.0 {
        UNIX_EPOCH.checked_add(magnitude)
    } else {
        UNIX_EPOCH.checked_sub(magnitude)
    }
}

/// Nanoseconds since the epoch (file modification stamps).
pub fn to_ns(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

/// A Windows FILETIME (100 ns since 1601) as a time.
pub fn from_filetime(filetime: u64) -> SystemTime {
    let since_1601 = Duration::from_nanos(filetime.saturating_mul(100));
    let offset = Duration::from_secs(FILETIME_UNIX_OFFSET_S);
    match since_1601.checked_sub(offset) {
        Some(after) => UNIX_EPOCH + after,
        None => UNIX_EPOCH - (offset - since_1601),
    }
}

pub fn to_filetime(t: SystemTime) -> u64 {
    let ns = to_ns(t) + (FILETIME_UNIX_OFFSET_S as i128) * 1_000_000_000;
    (ns.max(0) / 100) as u64
}

fn utc(t: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(t)
}

/// `2026-09-28T12:34:56Z`: whole seconds, as the Mac writes `accounts.json`
/// and `usage-state.json` (sub-second parts are dropped).
pub fn iso8601(t: SystemTime) -> String {
    utc(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `2026-09-28T12:34:56.120Z`: rounded to the millisecond, as the cloud
/// contract writes dates (CloudJSON).
pub fn iso8601_ms(t: SystemTime) -> String {
    utc(from_ms(
        to_ms(t) + u64::from(to_ns(t).rem_euclid(1_000_000) >= 500_000),
    ))
    .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// ISO 8601 with or without fractional seconds, `Z` or an offset; no zone
/// means UTC. `None` for anything else.
pub fn parse_iso8601(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    if let Ok(date) = DateTime::parse_from_rfc3339(text) {
        return Some(date.with_timezone(&Utc).into());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Some(naive.and_utc().into());
        }
    }
    None
}

/// A date written as whole-second ISO 8601 (`Z`), read leniently. Round
/// trips a Mac-written file exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IsoSeconds(pub SystemTime);

impl Serialize for IsoSeconds {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&iso8601(self.0))
    }
}

impl<'de> Deserialize<'de> for IsoSeconds {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_iso8601(&text)
            .map(IsoSeconds)
            .ok_or_else(|| serde::de::Error::custom(format!("not an ISO 8601 date: {text}")))
    }
}

/// A date written as epoch seconds (a double), kept as the exact number read
/// so a file round-trips unchanged. A whole number is written without a
/// fraction, as Swift's `JSONEncoder` writes it (`1789998000`, not
/// `1789998000.0`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Deserialize)]
#[serde(transparent)]
pub struct EpochSeconds(pub f64);

impl Serialize for EpochSeconds {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let whole = self.0.fract() == 0.0 && self.0.abs() < 9_007_199_254_740_992.0;
        if whole {
            serializer.serialize_i64(self.0 as i64)
        } else {
            serializer.serialize_f64(self.0)
        }
    }
}

impl EpochSeconds {
    pub fn from_time(t: SystemTime) -> Self {
        EpochSeconds(to_secs_f64(t))
    }

    pub fn to_time(self) -> Option<SystemTime> {
        from_secs_f64(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_round_trip() {
        let t = from_ms(1_790_000_000_123);
        assert_eq!(to_ms(t), 1_790_000_000_123);
        assert_eq!(to_ms(UNIX_EPOCH - Duration::from_secs(5)), 0);
    }

    #[test]
    fn iso_whole_seconds() {
        let t = from_ms(1_790_000_000_987);
        assert_eq!(iso8601(t), "2026-09-21T14:13:20Z");
        assert_eq!(
            parse_iso8601("2026-09-21T14:13:20Z"),
            Some(from_ms(1_790_000_000_000))
        );
        assert_eq!(
            parse_iso8601("2026-09-21T14:13:20.5Z"),
            Some(from_ms(1_790_000_000_500))
        );
        assert_eq!(
            parse_iso8601("2026-09-21T16:13:20+02:00"),
            Some(from_ms(1_790_000_000_000))
        );
        assert_eq!(
            parse_iso8601("2026-09-21T14:13:20"),
            Some(from_ms(1_790_000_000_000))
        );
        assert_eq!(parse_iso8601("yesterday"), None);
    }

    #[test]
    fn iso_milliseconds_round() {
        let t = UNIX_EPOCH + Duration::from_nanos(1_790_000_000_481_600_000);
        assert_eq!(iso8601_ms(t), "2026-09-21T14:13:20.482Z");
        assert_eq!(
            iso8601_ms(from_ms(1_790_000_000_000)),
            "2026-09-21T14:13:20.000Z"
        );
    }

    #[test]
    fn iso_seconds_serde() {
        let json = "\"2026-09-21T14:13:20Z\"";
        let date: IsoSeconds = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&date).unwrap(), json);
        assert!(serde_json::from_str::<IsoSeconds>("\"soon\"").is_err());
    }

    #[test]
    fn epoch_seconds_keep_their_number() {
        let parsed: EpochSeconds = serde_json::from_str("1790000000.123456").unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "1790000000.123456");
        assert_eq!(
            serde_json::to_string(&EpochSeconds(1_789_998_000.0)).unwrap(),
            "1789998000"
        );
        let t = parsed.to_time().unwrap();
        assert_eq!(to_ms(t), 1_790_000_000_123);
        assert_eq!(from_secs_f64(f64::NAN), None);
        assert_eq!(
            from_secs_f64(-1.5),
            Some(UNIX_EPOCH - Duration::from_millis(1500))
        );
    }

    #[test]
    fn filetime() {
        // 2026-09-21T14:13:20Z as a FILETIME.
        let ft = (1_790_000_000u64 + FILETIME_UNIX_OFFSET_S) * 10_000_000;
        assert_eq!(from_filetime(ft), from_ms(1_790_000_000_000));
        assert_eq!(to_filetime(from_ms(1_790_000_000_000)), ft);
        assert!(from_filetime(0) < UNIX_EPOCH);
    }
}
