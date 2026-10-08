//! `core::time` is total: a date from a file or the WebView that no clock
//! holds is written as the nearest end of 1601..9999, never a panic.

use agentnotch_engine::core::time::{
    from_filetime, from_ms, from_secs_f64, iso8601, iso8601_ms, parse_iso8601, to_filetime, to_ms,
    EpochSeconds, IsoSeconds, MAX_MS,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_TEXT: &str = "9999-12-31T23:59:59Z";
const MAX_TEXT_MS: &str = "9999-12-31T23:59:59.999Z";
const MIN_TEXT: &str = "1601-01-01T00:00:00Z";

/// `secs` after (negative: before) the epoch, when this platform's clock holds
/// it: the Mac's reaches far past both ends, Windows' runs from 1601 to 30828.
fn at(secs: i64) -> Option<SystemTime> {
    let span = Duration::from_secs(secs.unsigned_abs());
    if secs >= 0 {
        UNIX_EPOCH.checked_add(span)
    } else {
        UNIX_EPOCH.checked_sub(span)
    }
}

#[test]
fn from_ms_is_total() {
    assert_eq!(to_ms(from_ms(u64::MAX)), MAX_MS);
    assert_eq!(to_ms(from_ms(MAX_MS + 1)), MAX_MS);
    assert_eq!(to_ms(from_ms(MAX_MS)), MAX_MS);
    assert_eq!(to_ms(from_ms(0)), 0);
    assert_eq!(to_ms(from_ms(1_790_000_000_123)), 1_790_000_000_123);
    // A timestamp the page sent in microseconds is a far-future date, held.
    assert_eq!(iso8601(from_ms(1_790_000_000_123_000)), MAX_TEXT);
    assert_eq!(iso8601_ms(from_ms(u64::MAX)), MAX_TEXT_MS);
}

#[test]
fn a_time_after_9999_is_written_as_the_last_second() {
    // Year 30000 on Windows' clock, year 286000 on the Mac's.
    for secs in [900_000_000_000, 9_000_000_000_000] {
        let Some(far) = at(secs) else { continue };
        assert_eq!(iso8601(far), MAX_TEXT, "{secs}");
        assert_eq!(iso8601_ms(far), MAX_TEXT_MS, "{secs}");
    }
    let after = UNIX_EPOCH + Duration::from_millis(MAX_MS + 5_000);
    assert_eq!(iso8601(after), MAX_TEXT);
    assert_eq!(iso8601_ms(after), MAX_TEXT_MS);
}

#[test]
fn a_time_before_1601_is_written_as_the_first_second() {
    // The Mac's `SystemTime` holds these; Windows' starts in 1601.
    for secs in [-11_644_473_601, -20_000_000_000, -9_000_000_000_000] {
        let Some(before) = at(secs) else { continue };
        assert_eq!(iso8601(before), MIN_TEXT, "{secs}");
        assert_eq!(iso8601_ms(before), "1601-01-01T00:00:00.000Z", "{secs}");
    }
}

#[test]
fn times_inside_the_range_keep_their_text() {
    assert_eq!(iso8601(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601(from_filetime(0)), MIN_TEXT);
    assert_eq!(
        iso8601_ms(UNIX_EPOCH - Duration::from_millis(1)),
        "1969-12-31T23:59:59.999Z"
    );
    assert_eq!(
        iso8601(UNIX_EPOCH - Duration::from_secs(86_400)),
        "1969-12-31T00:00:00Z"
    );
    assert_eq!(iso8601_ms(from_ms(MAX_MS)), MAX_TEXT_MS);
    assert_eq!(iso8601(from_ms(1_790_000_000_987)), "2026-09-21T14:13:20Z");
}

#[test]
fn a_cached_usage_style_number_is_held() {
    // `CachedUsageRaw` reads 1e13 seconds as a date: past the range's end
    // (and past Windows' clock, where it is no time at all).
    let t = from_secs_f64(1.0e13).unwrap_or_else(|| from_ms(MAX_MS));
    assert_eq!(iso8601(t), MAX_TEXT);
    assert_eq!(iso8601_ms(t), MAX_TEXT_MS);
    assert_eq!(from_secs_f64(1.0e13 + 1.0), None);
    if let Some(t) = from_secs_f64(-1.0e13) {
        assert_eq!(iso8601(t), MIN_TEXT);
    }
    // Serde paths: a huge epoch number and a huge ISO year don't panic either.
    let huge: EpochSeconds = serde_json::from_str("1e13").unwrap();
    if let Some(t) = huge.to_time() {
        assert_eq!(iso8601(t), MAX_TEXT);
    }
    assert!(serde_json::from_str::<IsoSeconds>("\"+262143-12-31T23:59:59Z\"").is_err());
}

#[test]
fn filetimes_out_of_range_are_held() {
    // The largest FILETIME (in nanoseconds, a `Duration`'s limit) is the year 2185.
    assert!(iso8601(from_filetime(u64::MAX)).starts_with("2185-"));
    assert_eq!(
        from_filetime(0),
        at(-11_644_473_600).expect("1601 is the first instant of both clocks")
    );
    if let Some(before) = at(-20_000_000_000) {
        assert_eq!(to_filetime(before), 0);
    }
}

#[test]
fn the_extremes_never_panic() {
    let mut extremes: Vec<SystemTime> = vec![
        UNIX_EPOCH,
        from_ms(u64::MAX),
        from_filetime(u64::MAX),
        from_filetime(0),
    ];
    extremes.extend(at(i64::from(u32::MAX) * 1000));
    extremes.extend(at(-i64::from(u32::MAX) * 100));
    for t in extremes {
        let seconds = iso8601(t);
        let millis = iso8601_ms(t);
        assert!(seconds.len() == 20 && seconds.ends_with('Z'), "{seconds}");
        assert!(millis.len() == 24 && millis.ends_with('Z'), "{millis}");
        assert!(parse_iso8601(&seconds).is_some(), "{seconds}");
        let _ = to_ms(t);
    }
}
