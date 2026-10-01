//! Every cloud file in `<support>` round-trips in the Mac's format: a
//! Mac-written fixture (`tests/fixtures/mac-files/cloud-*.json`) is read
//! through its Rust type and written back JSON-equivalent (CL§4.2). Later
//! steps add the ledger, scan state, outbox, summaries, sync state and folder
//! logins.

use agentnotch_engine::cloud::auth::AuthSession;
use agentnotch_engine::cloud::contract::date;
use agentnotch_engine::persist::json_equivalent;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mac-files")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_equivalent(original: &[u8], written: &[u8], name: &str) {
    let a: Value = serde_json::from_slice(original).unwrap();
    let b: Value = serde_json::from_slice(written).unwrap();
    assert!(
        json_equivalent(&a, &b),
        "{name} changed:\n{a:#}\n---\n{b:#}"
    );
}

#[test]
fn cloud_session_json() {
    let bytes = fixture("cloud-session.json");
    let session = AuthSession::decode(&bytes).expect("parses");
    assert_eq!(session.access_token, "made-up-access-token-for-a-test");
    assert_eq!(session.refresh_token, "made-up-refresh-token");
    assert_eq!(
        date::to_string(session.expires_at),
        "2026-09-25T09:47:03.120Z"
    );
    assert_eq!(
        session.user_id.as_deref(),
        Some("5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d")
    );
    assert_eq!(session.email.as_deref(), Some("me@example.com"));
    assert_eq!(session.supabase_url, "https://abcdefghijklmnop.supabase.co");
    assert_eq!(
        session.publishable_key,
        "sb_publishable_example0000000000000000"
    );
    assert_eq!(session.website_url, "https://agentnotch.example.com");
    let written = session.encode();
    assert_equivalent(&bytes, &written, "cloud-session.json");
    // The Mac's encoder writes exactly this: compact, keys sorted.
    assert_eq!(written, bytes);

    // Older whole-second dates still read; anything else is no session.
    let whole = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("09:47:03.120Z", "09:47:03Z");
    assert!(AuthSession::decode(whole.as_bytes()).is_some());
    let missing = String::from_utf8(bytes)
        .unwrap()
        .replace("\"refreshToken\"", "\"refresh\"");
    assert!(AuthSession::decode(missing.as_bytes()).is_none());
}
