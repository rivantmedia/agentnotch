//! `usage::parser`: the Mac's UsageParserTests, vector for vector. Fixtures
//! follow the shapes Claude Code 2.1.280 produces: the account-usage body (as
//! cached in `.claude.json` and returned by `get_usage`), the `get_usage`
//! control response, and the status line. Doubles are compared exactly, as
//! the Swift tests compare them.

use agentnotch_engine::core::claude_json::{identity_from_oauth_account, ClaudeGlobalConfig};
use agentnotch_engine::core::time;
use agentnotch_engine::model::{ExtraUsage, UsageWindow};
use agentnotch_engine::usage::parser::{
    cached_usage_from_raw, number, offset, parse_cached_usage, parse_date,
    parse_get_usage_response, parse_status_line_rate_limits, parse_usage_body, GetUsageResult,
};
use serde_json::{json, Map, Value};
use std::time::SystemTime;

fn object(text: &str) -> Map<String, Value> {
    match serde_json::from_str::<Value>(text).expect("fixture is JSON") {
        Value::Object(map) => map,
        other => panic!("fixture is not an object: {other}"),
    }
}

/// An instant from an ISO 8601 fixture, through the same reader the parser uses.
fn date(iso: &str) -> SystemTime {
    time::parse_iso8601(iso).expect("fixture date")
}

fn epoch(seconds: f64) -> SystemTime {
    time::from_secs_f64(seconds).expect("in range")
}

fn secs(t: Option<SystemTime>) -> Option<f64> {
    t.map(time::to_secs_f64)
}

fn window(utilization: f64, resets_at: Option<SystemTime>, duration_s: u64) -> UsageWindow {
    UsageWindow::new(utilization, resets_at, duration_s)
}

fn names(scoped: &[(String, UsageWindow)]) -> Vec<&str> {
    scoped.iter().map(|(name, _)| name.as_str()).collect()
}

/// A full usage body as the endpoint returns it today.
const LIVE_BODY: &str = r#"
{"five_hour":{"utilization":9,"resets_at":"2026-09-24T12:50:00.257626+00:00","limit_dollars":null,"used_dollars":null,"remaining_dollars":null,"locked_reason":null},
 "seven_day":{"utilization":3.0,"resets_at":"2026-09-29T06:00:00+00:00","limit_dollars":null,"used_dollars":null,"remaining_dollars":null,"locked_reason":null},
 "seven_day_oauth_apps":null,"seven_day_opus":null,"seven_day_sonnet":null,
 "seven_day_cowork":null,"seven_day_omelette":null,"cinder_cove":null,
 "iguana_necktie":{"utilization":0,"resets_at":null,"limit_dollars":250},
 "extra_usage":{"is_enabled":false,"monthly_limit":null,"used_credits":null,"utilization":null,"currency":null,"disabled_reason":null},
 "limits":[
   {"kind":"session","group":"session","percent":9,"severity":"normal","resets_at":"2026-09-24T12:50:00.257626+00:00","scope":null,"is_active":true},
   {"kind":"weekly_all","group":"weekly","percent":3,"severity":"normal","resets_at":"2026-09-29T06:00:00+00:00","scope":null,"is_active":true},
   {"kind":"weekly_scoped","group":"weekly","percent":0,"severity":"normal","resets_at":"2026-09-29T06:00:00+00:00","scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}
 ],
 "spend":{"limit_dollars":null},
 "seven_day_breakdown":{"rows":[{"key":"claude_code","percent":92}]}}
"#;

#[test]
fn parses_live_body() {
    let usage = parse_usage_body(&object(LIVE_BODY));

    let five_hour = usage.five_hour.as_ref().expect("five_hour");
    assert_eq!(five_hour.utilization, 9.0);
    assert_eq!(five_hour.duration_s, UsageWindow::SESSION_DURATION_S);
    assert_eq!(
        five_hour.resets_at,
        Some(date("2026-09-24T12:50:00.257626+00:00"))
    );

    let seven_day = usage.seven_day.as_ref().expect("seven_day");
    assert_eq!(seven_day.utilization, 3.0);
    assert_eq!(seven_day.duration_s, UsageWindow::WEEKLY_DURATION_S);
    assert_eq!(seven_day.resets_at, Some(date("2026-09-29T06:00:00Z")));

    assert_eq!(names(&usage.scoped), ["Fable"]);
    assert_eq!(usage.scoped[0].1.utilization, 0.0);
    assert_eq!(usage.scoped[0].1.duration_s, UsageWindow::WEEKLY_DURATION_S);

    let extra = usage.extra_usage.as_ref().expect("extra_usage");
    assert!(!extra.is_enabled);
    assert_eq!(extra.monthly_limit, None);
    assert_eq!(usage.subscription_type, None);
}

#[test]
fn fractional_seconds_are_kept() {
    let with_fraction = parse_date(Some(&json!("2026-09-24T12:50:00.257626+00:00")));
    let without = parse_date(Some(&json!("2026-09-24T12:50:00+00:00")));
    let zulu = parse_date(Some(&json!("2026-09-24T12:50:00Z")));
    let millis = parse_date(Some(&json!("2026-09-24T12:50:00.257Z")));
    assert_eq!(without, zulu);
    assert_eq!(secs(without), Some(1_790_254_200.0));
    assert!((secs(with_fraction).unwrap() - 1_790_254_200.257626).abs() < 0.001);
    assert!((secs(millis).unwrap() - 1_790_254_200.257).abs() < 0.001);
}

#[test]
fn parses_offsets_and_missing_zones() {
    assert_eq!(
        secs(parse_date(Some(&json!("2026-09-24T18:20:00+05:30")))),
        Some(1_790_254_200.0)
    );
    // No zone designator: read as UTC rather than dropped.
    assert_eq!(
        secs(parse_date(Some(&json!("2026-09-24T12:50:00")))),
        Some(1_790_254_200.0)
    );
}

#[test]
fn parses_epoch_dates() {
    // Seconds, milliseconds (anything above 1e12), and a fractional second.
    assert_eq!(
        secs(parse_date(Some(&json!(1_790_254_200)))),
        Some(1_790_254_200.0)
    );
    assert_eq!(
        secs(parse_date(Some(&json!(1_790_254_200_000_i64)))),
        Some(1_790_254_200.0)
    );
    assert_eq!(
        secs(parse_date(Some(&json!(1_790_254_200.5)))),
        Some(1_790_254_200.5)
    );
}

#[test]
fn rejects_bad_dates() {
    for text in ["", "soon", "2026-13-45T99:00:00Z"] {
        assert_eq!(parse_date(Some(&json!(text))), None, "{text:?}");
    }
}

/// Dates outside what Windows' `SystemTime` holds (1601 to 30828) are no
/// date there, never a panic (chrono's own conversion panics); elsewhere
/// they read as written. In a body such a window is left out or read, the
/// rest of the body stays.
#[test]
fn dates_beyond_the_platforms_clock_never_panic() {
    for (text, seconds) in [
        ("0001-01-01T00:00:00Z", -62_135_596_800.0),
        ("1600-12-31T23:59:59Z", -11_644_473_601.0),
        ("1600-12-31T23:59:59", -11_644_473_601.0),
    ] {
        if let Some(at) = parse_date(Some(&json!(text))) {
            assert_eq!(secs(Some(at)), Some(seconds), "{text}");
        }
        if let Some(at) = time::parse_iso8601(text) {
            assert_eq!(secs(Some(at)), Some(seconds), "{text}");
        }
    }
    let usage = parse_usage_body(&object(
        r#"{"five_hour":{"utilization":9,"resets_at":"0001-01-01T00:00:00Z"},
            "seven_day":{"utilization":3,"resets_at":"2026-09-29T06:00:00Z"}}"#,
    ));
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(3.0));
    // Shifting by a span no `Duration` holds leaves the time as it was.
    let at = epoch(1_790_254_200.0);
    for seconds in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300] {
        assert_eq!(offset(at, seconds), at, "{seconds}");
    }
    assert_eq!(offset(at, 1.5), epoch(1_790_254_201.5));
}

#[test]
fn rejects_non_dates() {
    assert_eq!(parse_date(None), None);
    assert_eq!(parse_date(Some(&Value::Null)), None);
    assert_eq!(parse_date(Some(&json!(true))), None);
}

#[test]
fn utilization_accepts_int_double_and_string() {
    let body = object(
        r#"{"five_hour":{"utilization":"42.5","resets_at":null},
            "seven_day":{"utilization":7,"resets_at":"2026-09-29T06:00:00Z"},
            "seven_day_opus":{"utilization":12.25,"resets_at":"2026-09-29T06:00:00.000Z"}}"#,
    );
    let usage = parse_usage_body(&body);
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(42.5));
    assert_eq!(usage.five_hour.as_ref().and_then(|w| w.resets_at), None);
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(7.0));
    assert_eq!(
        usage.scoped,
        vec![(
            "Opus".to_owned(),
            window(
                12.25,
                Some(date("2026-09-29T06:00:00Z")),
                UsageWindow::WEEKLY_DURATION_S
            )
        )]
    );
}

#[test]
fn booleans_are_not_numbers() {
    assert_eq!(number(Some(&json!(true))), None);
    assert_eq!(number(Some(&json!(false))), None);
    assert_eq!(number(Some(&json!(3))), Some(3.0));
    assert_eq!(number(Some(&json!(" 4.5 "))), Some(4.5));
    assert_eq!(number(Some(&json!("nan"))), None);
}

#[test]
fn null_windows_are_none() {
    let usage = parse_usage_body(&object(
        r#"{"five_hour":null,"seven_day":{"utilization":null,"resets_at":null},"seven_day_opus":null,"seven_day_sonnet":null,"extra_usage":null}"#,
    ));
    assert_eq!(usage.five_hour, None);
    assert_eq!(usage.seven_day, None);
    assert!(usage.scoped.is_empty());
    assert_eq!(usage.extra_usage, None);
    assert!(!usage.has_windows());
}

#[test]
fn limits_fill_missing_top_level_windows() {
    let usage = parse_usage_body(&object(
        r#"{"five_hour":null,"seven_day":null,
            "limits":[{"kind":"session","percent":61,"resets_at":"2026-09-24T12:50:00Z"},
                      {"kind":"weekly_all","percent":"18","resets_at":"2026-09-29T06:00:00Z"}]}"#,
    ));
    let five = usage.five_hour.as_ref().unwrap();
    assert_eq!(five.utilization, 61.0);
    assert_eq!(five.duration_s, UsageWindow::SESSION_DURATION_S);
    let seven = usage.seven_day.as_ref().unwrap();
    assert_eq!(seven.utilization, 18.0);
    assert_eq!(seven.duration_s, UsageWindow::WEEKLY_DURATION_S);
}

#[test]
fn scoped_limits_merge_and_dedupe_by_name() {
    let usage = parse_usage_body(&object(
        r#"{"five_hour":{"utilization":20,"resets_at":"2026-09-24T12:50:00Z"},
            "seven_day":{"utilization":30,"resets_at":"2026-09-29T06:00:00Z"},
            "seven_day_opus":{"utilization":99,"resets_at":"2026-09-29T06:00:00Z"},
            "seven_day_sonnet":{"utilization":14,"resets_at":"2026-09-29T06:00:00Z"},
            "limits":[{"kind":"weekly_scoped","percent":41,"resets_at":"2026-09-29T06:00:00Z","scope":{"model":{"id":null,"display_name":"Opus"}}},
                      {"kind":"weekly_scoped","percent":5,"resets_at":"2026-09-29T06:00:00Z","scope":{"model":{"display_name":"Fable"}}},
                      {"kind":"weekly_scoped","percent":8,"scope":null}],
            "model_scoped":[{"display_name":"fable","utilization":6,"resets_at":"2026-09-29T06:00:00Z"},
                            {"display_name":"Haiku","utilization":2.5,"resets_at":"2026-09-29T06:00:00.5Z"}]}"#,
    ));
    // limits[] first, then model_scoped, then the legacy per-model keys;
    // names compare case-insensitively and the first reading wins.
    assert_eq!(names(&usage.scoped), ["Opus", "Fable", "Haiku", "Sonnet"]);
    let utilizations: Vec<f64> = usage.scoped.iter().map(|(_, w)| w.utilization).collect();
    assert_eq!(utilizations, [41.0, 5.0, 2.5, 14.0]);
}

#[test]
fn extra_usage_when_enabled() {
    let usage = parse_usage_body(&object(
        r#"{"five_hour":{"utilization":100,"resets_at":"2026-09-24T12:50:00Z"},
            "extra_usage":{"is_enabled":true,"monthly_limit":5000,"used_credits":1234.5,"utilization":24.69,"currency":"USD"}}"#,
    ));
    assert_eq!(
        usage.extra_usage,
        Some(ExtraUsage {
            is_enabled: true,
            monthly_limit: Some(5000.0),
            used_credits: Some(1234.5),
            utilization: Some(24.69),
            currency: Some("USD".to_owned()),
        })
    );
}

// MARK: - get_usage

#[test]
fn get_usage_response() {
    let response = object(
        r#"{"session":{"id":"x"},"subscription_type":"max","rate_limits_available":true,
            "rate_limits":{"five_hour":{"utilization":23,"resets_at":"2026-09-24T12:50:00.257626+00:00"},
                           "seven_day":{"utilization":41,"resets_at":"2026-09-29T06:00:00+00:00"},
                           "seven_day_sonnet":null,
                           "limits":[],
                           "model_scoped":[{"display_name":"Opus","utilization":12,"resets_at":"2026-09-29T06:00:00+00:00"}]},
            "behaviors":null}"#,
    );
    let GetUsageResult::Usage(usage) = parse_get_usage_response(&response) else {
        panic!("expected usage");
    };
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(23.0));
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(41.0));
    assert_eq!(names(&usage.scoped), ["Opus"]);
    assert_eq!(usage.subscription_type.as_deref(), Some("max"));
    // A `limits` array is present, so the answer is a fresh fetch.
    assert!(!usage.is_possibly_seeded);
}

#[test]
fn get_usage_unavailable() {
    let response =
        object(r#"{"subscription_type":null,"rate_limits_available":false,"rate_limits":null}"#);
    assert_eq!(
        parse_get_usage_response(&response),
        GetUsageResult::Unavailable("Usage limits aren't available for this login".to_owned())
    );
}

#[test]
fn get_usage_rate_limited() {
    let response = object(
        r#"{"rate_limits_available":true,"rate_limits":{"error":{"type":"rate_limit_error","message":"Rate limited"}}}"#,
    );
    assert_eq!(
        parse_get_usage_response(&response),
        GetUsageResult::RateLimited
    );
}

/// Claude Code 2.1.280 answers `rate_limits: null` with availability true
/// when its own fetch failed (429, network): a retryable failure.
#[test]
fn get_usage_when_claude_codes_fetch_failed() {
    let response = object(
        r#"{"session":{"total_cost_usd":0},"subscription_type":"max","rate_limits_available":true,"rate_limits":null,"behaviors":null}"#,
    );
    assert_eq!(
        parse_get_usage_response(&response),
        GetUsageResult::Malformed("Claude Code couldn't load usage right now".to_owned())
    );
}

#[test]
fn get_usage_without_windows_is_malformed() {
    let response = object(
        r#"{"rate_limits_available":true,"rate_limits":{"five_hour":null,"seven_day":null}}"#,
    );
    assert!(matches!(
        parse_get_usage_response(&response),
        GetUsageResult::Malformed(_)
    ));
}

// MARK: - .claude.json

#[test]
fn cached_usage_from_global_config() {
    let text = format!(
        r#"{{"numStartups":12,"projects":{{"/Users/me/p":{{"allowedTools":[]}}}},
         "oauthAccount":{{"accountUuid":"acc-1","emailAddress":"me@example.com","organizationType":"claude_max","organizationRateLimitTier":"default_claude_max_20x","displayName":"Me","organizationName":"Me's Org","hasExtraUsageEnabled":false}},
         "cachedUsageUtilization":{{"fetchedAtMs":1790250000123,"accountUuid":"acc-1","utilization":{LIVE_BODY}}}}}"#
    );
    let snapshot = parse_cached_usage(&object(&text)).expect("cached usage");
    assert_eq!(snapshot.account_uuid.as_deref(), Some("acc-1"));
    assert!((time::to_secs_f64(snapshot.fetched_at) - 1_790_250_000.123).abs() < 0.0001);
    assert_eq!(
        snapshot.usage.five_hour.as_ref().map(|w| w.utilization),
        Some(9.0)
    );

    // The engine's own reader of the file agrees with the parser.
    let extracted = ClaudeGlobalConfig::parse(text.as_bytes(), None).expect("config");
    let identity = extracted.identity.as_ref().expect("identity");
    assert_eq!(identity.email.as_deref(), Some("me@example.com"));
    assert_eq!(identity.subscription_type().as_deref(), Some("max"));
    assert_eq!(
        identity.rate_limit_tier.as_deref(),
        Some("default_claude_max_20x")
    );
    let matching = extracted.matching_cached_usage().expect("matching");
    assert_eq!(cached_usage_from_raw(matching), Some(snapshot));
}

#[test]
fn cached_usage_from_another_login_is_ignored() {
    let text = format!(
        r#"{{"oauthAccount":{{"accountUuid":"acc-2","emailAddress":"other@example.com"}},
         "cachedUsageUtilization":{{"fetchedAtMs":1790250000123,"accountUuid":"acc-1","utilization":{LIVE_BODY}}}}}"#
    );
    let extracted = ClaudeGlobalConfig::parse(text.as_bytes(), None).expect("config");
    assert!(extracted.cached_usage.is_some());
    assert!(extracted.matching_cached_usage().is_none());
}

#[test]
fn signed_out_config_has_no_identity() {
    let extracted =
        ClaudeGlobalConfig::parse(br#"{"numStartups":1,"oauthAccount":{}}"#, None).expect("config");
    assert!(extracted.identity.is_none());
    assert!(extracted.cached_usage.is_none());
}

#[test]
fn user_tier_is_the_fallback() {
    let identity = identity_from_oauth_account(&json!({
        "accountUuid": "a", "userRateLimitTier": "default_claude_max_5x", "organizationType": "claude_pro"
    }))
    .expect("identity");
    assert_eq!(
        identity.rate_limit_tier.as_deref(),
        Some("default_claude_max_5x")
    );
    assert_eq!(identity.subscription_type().as_deref(), Some("pro"));
}

// MARK: - Status line

#[test]
fn status_line_rate_limits() {
    let rate_limits = json!({
        "five_hour": {"used_percentage": 23.5, "resets_at": 1_738_425_600},
        "seven_day": {"used_percentage": 41.2, "resets_at": 1_738_857_600}
    });
    let (five, seven) = parse_status_line_rate_limits(Some(&rate_limits));
    assert_eq!(
        five,
        Some(window(
            23.5,
            Some(epoch(1_738_425_600.0)),
            UsageWindow::SESSION_DURATION_S
        ))
    );
    assert_eq!(
        seven,
        Some(window(
            41.2,
            Some(epoch(1_738_857_600.0)),
            UsageWindow::WEEKLY_DURATION_S
        ))
    );
}

#[test]
fn status_line_with_only_one_window() {
    let (five, seven) = parse_status_line_rate_limits(Some(
        &json!({"seven_day": {"used_percentage": 0, "resets_at": 1_738_857_600}}),
    ));
    assert_eq!(five, None);
    assert_eq!(seven.map(|w| w.utilization), Some(0.0));
    assert_eq!(parse_status_line_rate_limits(None).0, None);
}
