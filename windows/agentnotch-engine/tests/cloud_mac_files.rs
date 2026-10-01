//! Every cloud file in `<support>` round-trips in the Mac's format: a
//! Mac-written fixture (`tests/fixtures/mac-files/cloud-*.json`) is read
//! through its Rust type and written back JSON-equivalent (CL§4.2). Later
//! steps add the scan state, outbox, summaries and sync state.

use agentnotch_engine::cloud::auth::AuthSession;
use agentnotch_engine::cloud::contract::{date, to_json, SessionSource};
use agentnotch_engine::cloud::folder_logins::Contents as FolderLogins;
use agentnotch_engine::cloud::ledger::{Contents as Ledger, Origin, SessionOwner};
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

#[test]
fn cloud_ledger_json() {
    let bytes = fixture("cloud-ledger.json");
    let ledger: Ledger = serde_json::from_slice(&bytes).expect("parses");
    assert_eq!(ledger.version, 2);
    assert_eq!(ledger.sessions.len(), 4);
    let split = "0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b";
    let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let work = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";
    let first = &ledger.sessions[&format!("a1b2c3d4-e5f6-4789-8abc-def012345678|{personal}")];
    assert_eq!(first.source, SessionSource::Vscode);
    assert_eq!(first.origin, Origin::Live);
    assert_eq!(first.cost_usd, Some(1.5));
    assert_eq!(first.title.as_deref(), Some("Fix the notch"));
    assert_eq!(first.ended_at, None);
    let part = &ledger.sessions[&format!("{split}|{personal}")];
    assert_eq!(
        part.ended_at.map(date::to_string).as_deref(),
        Some("2026-09-25T08:15:00.250Z")
    );
    assert_eq!(
        ledger.sessions[&format!("{split}|{work}")].project_name,
        "api"
    );
    let old = &ledger.sessions[&format!("11111111-2222-4333-8444-555555555555|{personal}")];
    assert_eq!(old.origin, Origin::Backfill);
    assert_eq!(old.source, SessionSource::Other);
    // Owners: from the start (no date), a stretch of nobody, the work account.
    let owners = &ledger.owners[split];
    assert_eq!(owners.len(), 3);
    assert_eq!(owners[0], SessionOwner::new(None, personal));
    assert_eq!(owners[1].account_key, "");
    assert_eq!(
        owners[1].from.map(date::to_string).as_deref(),
        Some("2026-09-25T08:14:00.001Z")
    );
    assert_eq!(owners[2].account_key, work);
    assert_eq!(
        ledger.accounts[work].organization_name.as_deref(),
        Some("Company")
    );
    assert_eq!(ledger.accounts[personal].plan.as_deref(), Some("Max 20x"));
    // Sessions seen running with no account to give them.
    let unattributed = ledger.unattributed.as_ref().expect("unattributed");
    assert_eq!(
        date::to_string(unattributed["22222222-3333-4444-8555-666666666666"].0),
        "2026-09-25T08:20:00.000Z"
    );
    let written = to_json(&ledger);
    assert_equivalent(&bytes, &written, "cloud-ledger.json");
    // The Mac's encoder writes exactly this: compact, keys sorted, absent
    // optionals left out.
    assert_eq!(written, bytes);

    // A ledger from before `unattributed` still reads.
    let mut older: Value = serde_json::from_slice(&bytes).unwrap();
    older.as_object_mut().unwrap().remove("unattributed");
    let older: Ledger = serde_json::from_value(older).expect("an older ledger");
    assert!(older.unattributed.is_none());
}

#[test]
fn cloud_folder_logins_json() {
    let bytes = fixture("cloud-folder-logins.json");
    let logins: FolderLogins = serde_json::from_slice(&bytes).expect("parses");
    assert_eq!(logins.version, 1);
    assert_eq!(logins.folders.len(), 2);
    let work = &logins.folders["/Users/me/.claude-work"];
    assert_eq!(
        work.login,
        "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8"
    );
    assert_eq!(date::to_string(work.since.0), "2026-09-25T08:00:00.000Z");
    assert_eq!(
        date::to_string(logins.folders["/Users/me/.claude-personal"].since.0),
        "2026-09-26T09:30:15.500Z"
    );
    let written = to_json(&logins);
    assert_equivalent(&bytes, &written, "cloud-folder-logins.json");
    assert_eq!(written, bytes);
}
