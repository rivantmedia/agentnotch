//! `core::claude_json`: the Mac's PP_JSONFieldScannerTests
//! (theGlobalConfigReaderUsesOnlyTheTwoKeys), ClaudeAccountIdentity's rules,
//! `hasLogin`, and the reader's (mtime, size) cache with its last-good-parse
//! rule (ClaudeGlobalConfigReader), against temporary files.

use agentnotch_engine::core::claude_json::{
    has_login, identity_from_oauth_account, lenient_bool, ClaudeGlobalConfig, ClaudeJsonReader,
    READ_KEYS,
};
use serde_json::json;
use std::time::Duration;

#[test]
fn the_reader_uses_only_the_two_keys() {
    let data = br#"{"primaryApiKey": "sk-secret", "oauthAccount": {"accountUuid": "u", "emailAddress": "e@x.dev"}, "cachedUsageUtilization": {"accountUuid": "u", "fetchedAtMs": 1790000000000, "utilization": {"five_hour": {"utilization": 3, "resets_at": "2099-01-01T00:00:00Z"}}}}"#;
    let config = ClaudeGlobalConfig::parse(data, None).unwrap();
    assert_eq!(
        config.identity.as_ref().unwrap().email.as_deref(),
        Some("e@x.dev")
    );
    let cached = config.cached_usage.as_ref().unwrap();
    assert_eq!(cached.account_uuid.as_deref(), Some("u"));
    assert_eq!(cached.fetched_at_ms, Some(1_790_000_000_000.0));
    assert_eq!(cached.utilization["five_hour"]["utilization"], 3);
    assert_eq!(READ_KEYS, ["oauthAccount", "cachedUsageUtilization"]);
    assert!(config.matching_cached_usage().is_some());
}

#[test]
fn cached_usage_belongs_to_the_signed_in_login_only() {
    let other = br#"{"oauthAccount": {"accountUuid": "new"}, "cachedUsageUtilization": {"accountUuid": "old", "fetchedAtMs": 1, "utilization": {}}}"#;
    let config = ClaudeGlobalConfig::parse(other, None).unwrap();
    assert!(config.cached_usage.is_some());
    assert!(config.matching_cached_usage().is_none());
    let no_body = br#"{"oauthAccount": {"accountUuid": "u"}, "cachedUsageUtilization": {"accountUuid": "u", "fetchedAtMs": 1}}"#;
    assert!(ClaudeGlobalConfig::parse(no_body, None)
        .unwrap()
        .cached_usage
        .is_none());
    assert!(ClaudeGlobalConfig::parse(b"[]", None).is_none());
    assert_eq!(
        ClaudeGlobalConfig::parse(b"{}", None).unwrap(),
        ClaudeGlobalConfig::default()
    );
}

#[test]
fn identity_fields() {
    let identity = identity_from_oauth_account(&json!({
        "accountUuid": "5f0c", "emailAddress": "me@work.com", "displayName": "Me", "organizationName": "Work",
        "organizationUuid": "org-1", "organizationType": "claude_max", "userRateLimitTier": "default_claude_max_5x",
        "billingType": "stripe_subscription", "hasExtraUsageEnabled": "yes"
    }))
    .unwrap();
    assert_eq!(identity.account_uuid.as_deref(), Some("5f0c"));
    assert_eq!(
        identity.rate_limit_tier.as_deref(),
        Some("default_claude_max_5x")
    );
    assert_eq!(identity.subscription_type().as_deref(), Some("max"));
    assert_eq!(identity.has_extra_usage_enabled, Some(true));
    // The organization's tier wins over the user's.
    let org_tier = identity_from_oauth_account(&json!({"emailAddress": "a@b.c", "organizationRateLimitTier": "org", "userRateLimitTier": "user"})).unwrap();
    assert_eq!(org_tier.rate_limit_tier.as_deref(), Some("org"));
    // Nobody: neither a UUID nor an email (empty strings count as absent).
    assert!(identity_from_oauth_account(&json!({})).is_none());
    assert!(identity_from_oauth_account(
        &json!({"accountUuid": "", "emailAddress": "", "displayName": "x"})
    )
    .is_none());
    assert!(identity_from_oauth_account(&json!("text")).is_none());
    assert_eq!(lenient_bool(&json!(0)), Some(false));
    assert_eq!(lenient_bool(&json!("maybe")), None);
}

#[test]
fn has_login_scans_bytes() {
    assert!(has_login(
        br#"{"x": 1, "oauthAccount" : { "accountUuid": "u" }}"#
    ));
    assert!(!has_login(br#"{"oauthAccount": {}}"#));
    assert!(!has_login(br#"{"oauthAccount": { }}"#));
    assert!(!has_login(br#"{"oauthAccount": null}"#));
    assert!(!has_login(br#"{"other": "\"oauthAccount\""}"#));
    assert!(!has_login(b""));
    // The first mention can be a string; a later key still counts.
    assert!(has_login(
        br#"{"a": "\"oauthAccount\" text", "oauthAccount": {"emailAddress": "e"}}"#
    ));
}

#[test]
fn the_reader_caches_and_keeps_the_last_good_parse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    let reader = ClaudeJsonReader::new();
    assert!(reader.read(&path).is_none(), "no file");

    std::fs::write(
        &path,
        br#"{"oauthAccount": {"accountUuid": "first", "emailAddress": "a@x.dev"}}"#,
    )
    .unwrap();
    let (first, stamp) = reader.read_stamped(&path).unwrap();
    assert_eq!(
        first.identity.unwrap().account_uuid.as_deref(),
        Some("first")
    );
    assert!(first.modified_at.is_some());
    // Unchanged stamp: the cached parse.
    assert_eq!(reader.read_stamped(&path).unwrap().1, stamp);

    // Caught mid-write (a different size): the last good parse stays.
    std::fs::write(&path, br#"{"oauthAccount": {"accountUuid": "sec"#).unwrap();
    assert_eq!(
        reader
            .read(&path)
            .unwrap()
            .identity
            .unwrap()
            .account_uuid
            .as_deref(),
        Some("first")
    );

    // The write finished: the new login.
    std::thread::sleep(Duration::from_millis(5));
    std::fs::write(&path, br#"{"oauthAccount": {"accountUuid": "second", "emailAddress": "b@x.dev"}, "big": [1, 2, 3]}"#).unwrap();
    assert_eq!(
        reader
            .read(&path)
            .unwrap()
            .identity
            .unwrap()
            .account_uuid
            .as_deref(),
        Some("second")
    );

    // Gone: nothing, and the cache entry with it.
    std::fs::remove_file(&path).unwrap();
    assert!(reader.read(&path).is_none());
    std::fs::write(&path, b"not json at all").unwrap();
    assert!(
        reader.read(&path).is_none(),
        "no earlier good parse to fall back on"
    );

    reader.invalidate();
    assert!(reader.read(&path).is_none());
}
