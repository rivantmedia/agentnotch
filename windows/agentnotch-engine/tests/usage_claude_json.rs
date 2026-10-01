//! `usage::claude_json::read_claude_json_with` over temporary `.claude.json`
//! files (Job::ReadClaudeJson): who is signed in, and Claude Code's cached
//! usage only when it belongs to that login. Each test has its own reader, so
//! nothing is shared between them.

use agentnotch_engine::core::claude_json::ClaudeJsonReader;
use agentnotch_engine::core::time;
use agentnotch_engine::model::{AccountId, UsageSource};
use agentnotch_engine::usage::claude_json::read_claude_json_with;
use std::path::Path;

const BODY: &str = r#"{"five_hour":{"utilization":9,"resets_at":"2026-09-24T12:50:00.257626+00:00"},
 "seven_day":{"utilization":3.0,"resets_at":"2026-09-29T06:00:00+00:00"}}"#;

fn write_config(path: &Path, login: &str, cached_for: &str) {
    let text = format!(
        r#"{{"numStartups":3,"oauthAccount":{{"accountUuid":"{login}","emailAddress":"me@example.com","organizationType":"claude_max"}},
         "cachedUsageUtilization":{{"fetchedAtMs":1790250000123,"accountUuid":"{cached_for}","utilization":{BODY}}}}}"#
    );
    std::fs::write(path, text).unwrap();
}

fn folder() -> AccountId {
    AccountId::from("dir:c:\\users\\me\\.claude")
}

#[test]
fn a_signed_in_file_with_matching_cached_usage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    write_config(&path, "acc-1", "acc-1");
    let read = read_claude_json_with(&ClaudeJsonReader::new(), &folder(), &path);
    assert_eq!(read.folder, folder());
    assert_eq!(read.error, None);
    assert!(read.stamp.is_some());
    assert_eq!(
        read.identity.as_ref().and_then(|i| i.email.as_deref()),
        Some("me@example.com")
    );
    let usage = read.cached_usage.expect("cached usage");
    assert_eq!(usage.source, UsageSource::Cache);
    // Dated when Claude Code fetched it.
    assert_eq!(
        usage.updated_at,
        time::from_secs_f64(1_790_250_000.123).unwrap()
    );
    assert_eq!(usage.account_id.0, "uuid:acc-1");
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(9.0));
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(3.0));
}

#[test]
fn cached_usage_of_another_login_is_not_returned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    // A /login to acc-2 leaves acc-1's snapshot behind.
    write_config(&path, "acc-2", "acc-1");
    let read = read_claude_json_with(&ClaudeJsonReader::new(), &folder(), &path);
    assert_eq!(read.error, None);
    assert!(read.identity.is_some(), "the login itself is still read");
    assert_eq!(read.cached_usage, None);
}

#[test]
fn an_unparsable_file_is_an_error_and_the_last_good_parse_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    let reader = ClaudeJsonReader::new();

    // Nothing there yet: a folder nobody ran Claude Code in; no error.
    let missing = read_claude_json_with(&reader, &folder(), &path);
    assert_eq!(missing.error, None);
    assert!(missing.identity.is_none() && missing.cached_usage.is_none());

    // Never parsed and unparsable: an error, nothing else.
    std::fs::write(&path, b"{\"oauthAccount\": {\"accountUuid\": \"acc").unwrap();
    let broken = read_claude_json_with(&reader, &folder(), &path);
    assert!(broken.error.is_some());
    assert!(broken.identity.is_none() && broken.cached_usage.is_none());
    assert!(broken.stamp.is_none());

    // A good parse, then a read caught mid-write: the good parse stays.
    write_config(&path, "acc-1", "acc-1");
    let good = read_claude_json_with(&reader, &folder(), &path);
    assert_eq!(good.error, None);
    assert!(good.cached_usage.is_some());
    std::fs::write(&path, b"{\"oauthAccount\": {\"accountUuid\": \"acc").unwrap();
    let kept = read_claude_json_with(&reader, &folder(), &path);
    assert_eq!(kept.error, None);
    assert_eq!(kept.identity, good.identity);
    assert_eq!(kept.cached_usage, good.cached_usage);
}
