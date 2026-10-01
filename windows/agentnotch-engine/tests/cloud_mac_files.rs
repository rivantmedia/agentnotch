//! Every cloud file in `<support>` round-trips in the Mac's format: a
//! Mac-written fixture (`tests/fixtures/mac-files/cloud-*.json`) is read
//! through its Rust type and written back JSON-equivalent (CL§4.2). The
//! sync state is added with the sync service.

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

#[test]
fn cloud_scan_state_json() {
    use agentnotch_engine::cloud::scanner::State;
    let bytes = fixture("cloud-scan-state.json");
    let state: State = serde_json::from_slice(&bytes).expect("parses");
    assert_eq!(state.version, State::CURRENT_VERSION);
    assert_eq!(state.files.len(), 3);
    let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let main = &state.files
        ["/Users/me/.claude/projects/-Users-me-code-app/a1b2c3d4-e5f6-4789-8abc-def012345678.jsonl"];
    assert!(main.is_main);
    assert_eq!(main.inode, 8_590_012_345);
    // The Mac knows no volume; it stays absent when written back.
    assert_eq!(main.volume, None);
    assert_eq!((main.size, main.offset), (2048, 2048));
    assert_eq!(main.mtime.0, 1_790_323_240.5);
    assert_eq!(main.ai_title.as_deref(), Some("Fix the parser bug"));
    assert_eq!(main.entrypoint.as_deref(), Some("claude-vscode"));
    assert_eq!(main.claimed.len(), 2);
    assert_eq!(main.recent.len(), 2);
    assert_eq!(main.owners, [SessionOwner::new(None, personal)]);
    let part = &main.parts[personal];
    assert_eq!(
        (part.responses, part.cost, part.unpriced),
        (2, 2_215_000, 0)
    );
    assert_eq!(part.totals.input, 15);
    assert_eq!(part.model_counts["claude-haiku-4-5"], 1);
    assert_eq!(
        part.first.map(date::to_string).as_deref(),
        Some("2026-09-25T08:00:00.000Z")
    );
    // An unpriced response: no cost on it, and one counted unpriced.
    let agent = &state.files["/Users/me/.claude/projects/-Users-me-code-app/a1b2c3d4-e5f6-4789-8abc-def012345678/subagents/agent-one.jsonl"];
    assert!(!agent.is_main);
    assert_eq!(agent.recent[0].cost, None);
    assert_eq!(agent.parts[personal].unpriced, 1);
    // A session two accounts ran: the second owner starts at a time.
    let split = &state.files
        ["/Users/me/.claude/projects/-Users-me-code-app/0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b.jsonl"];
    assert_eq!(split.owners.len(), 2);
    assert_eq!(split.first, None);

    let written = serde_json::to_vec(&state).expect("encodes");
    assert_equivalent(&bytes, &written, "cloud-scan-state.json");
    // A file the engine wrote on Windows carries the volume, and the Mac's
    // reader (which ignores unknown keys) still takes the rest.
    let mut windows = state.clone();
    windows
        .files
        .values_mut()
        .for_each(|f| f.volume = Some(0xA1B2));
    let again: State =
        serde_json::from_slice(&serde_json::to_vec(&windows).expect("encodes")).expect("parses");
    assert_eq!(again, windows);
}

#[test]
fn cloud_summaries_json() {
    use agentnotch_engine::cloud::summary::store::{
        self, Contents as Summaries, SessionSummaryStore,
    };
    use agentnotch_engine::testkit::StdSecureFiles;
    use std::sync::Arc;
    let bytes = fixture("cloud-summaries.json");
    let contents: Summaries = serde_json::from_slice(&bytes).expect("parses");
    assert_eq!(contents.version, Summaries::CURRENT_VERSION);
    let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let work = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";
    let first = &contents.summaries[&format!("a1b2c3d4-e5f6-4789-8abc-def012345678|{personal}")];
    assert_eq!(first.model, "claude-haiku-4-5-20251001");
    assert_eq!(first.message_count, 14);
    assert_eq!(first.cost_usd, Some(0.0021));
    assert_eq!(
        date::to_string(first.generated_at),
        "2026-09-25T09:12:40.125Z"
    );
    // No cost known: the key is absent, and stays absent.
    let second = &contents.summaries[&format!("0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b|{work}")];
    assert_eq!(second.cost_usd, None);
    let attempt = &contents.attempts[&format!("11111111-2222-4333-8444-555555555555|{personal}")];
    assert_eq!(attempt.failures, 2);
    assert_eq!(
        date::to_string(attempt.next_attempt_at),
        "2026-09-25T10:30:00.500Z"
    );
    assert_eq!(contents.runs.len(), 2);
    assert_eq!(
        contents.enabled_at.map(date::to_string).as_deref(),
        Some("2026-09-25T08:00:00.000Z")
    );
    let written = to_json(&contents);
    assert_equivalent(&bytes, &written, "cloud-summaries.json");
    // The Mac's encoder writes exactly this: compact, keys sorted, absent
    // optionals left out.
    assert_eq!(written, bytes);

    // Through the store: read from <support>, written back the same.
    let support = tempfile::tempdir().unwrap();
    std::fs::write(support.path().join(store::FILE_NAME), &bytes).unwrap();
    let files = Arc::new(StdSecureFiles);
    let kept = SessionSummaryStore::in_support(support.path(), files.clone(), true);
    assert_eq!(kept.contents(), contents);
    kept.save_now();
    assert_eq!(
        std::fs::read(support.path().join(store::FILE_NAME)).unwrap(),
        bytes
    );
    // Never turned on: `enabledAt` absent.
    let mut never: Value = serde_json::from_slice(&bytes).unwrap();
    never.as_object_mut().unwrap().remove("enabledAt");
    let never: Summaries = serde_json::from_value(never).expect("parses");
    assert_eq!(never.enabled_at, None);
    assert!(!String::from_utf8(to_json(&never))
        .unwrap()
        .contains("enabledAt"));
}

#[test]
fn cloud_usage_outbox_json() {
    use agentnotch_engine::cloud::contract::UsageSourceName;
    use agentnotch_engine::cloud::recorder::{self, Contents as Outbox, UsageHistoryRecorder};
    use agentnotch_engine::testkit::StdSecureFiles;
    use std::sync::Arc;
    let bytes = fixture("cloud-usage-outbox.json");
    let outbox: Outbox = serde_json::from_slice(&bytes).expect("parses");
    assert_eq!(outbox.version, Outbox::CURRENT_VERSION);
    assert_eq!(outbox.pending.len(), 3);
    assert_eq!(outbox.last.len(), 4);
    let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let first = &outbox.pending[0];
    assert_eq!(first.account_key, personal);
    assert_eq!(first.source, UsageSourceName::Probe);
    assert_eq!(
        date::to_string(first.observed_at),
        "2026-09-25T08:05:00.250Z"
    );
    let ids: Vec<&str> = first.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["session", "weekly_all", "weekly_opus", "extra_usage"]);
    assert_eq!(first.windows[1].utilization, 61.5);
    // A window with no reset time: an explicit null, kept.
    assert_eq!(first.windows[2].resets_at, None);
    assert_eq!(
        first.dedupe_key(),
        format!("{personal}|probe|1790323500250")
    );
    assert_eq!(outbox.pending[2].source, UsageSourceName::Desktop);
    assert_eq!(outbox.last[&format!("{personal}|probe")], *first);
    let written = to_json(&outbox);
    assert_equivalent(&bytes, &written, "cloud-usage-outbox.json");
    assert!(String::from_utf8(written)
        .unwrap()
        .contains(r#""resetsAt":null"#));

    // Through the recorder: read from <support>, written back equivalent.
    let support = tempfile::tempdir().unwrap();
    let file = support.path().join(recorder::FILE_NAME);
    std::fs::write(&file, &bytes).unwrap();
    let kept = UsageHistoryRecorder::in_support(support.path(), Arc::new(StdSecureFiles), true);
    assert_eq!(kept.contents(), outbox);
    assert_eq!(kept.pending_count(), 3);
    kept.save_now();
    assert_equivalent(
        &bytes,
        &std::fs::read(&file).unwrap(),
        "cloud-usage-outbox.json",
    );
}

#[test]
fn cloud_sync_state_json() {
    use agentnotch_engine::cloud::pass::{CloudSyncMemory, Contents as SyncState, SYNC_STATE_FILE};
    use agentnotch_engine::testkit::StdSecureFiles;
    use std::sync::Arc;
    let bytes = fixture("cloud-sync-state.json");
    let state: SyncState = serde_json::from_slice(&bytes).expect("parses");
    assert_eq!(state.version, SyncState::CURRENT_VERSION);
    assert_eq!(
        state.user_id.as_deref(),
        Some("5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d")
    );
    assert_eq!(
        state.website.as_deref(),
        Some("https://agentnotch.example.com")
    );
    assert_eq!(
        state.dashboard_url.as_deref(),
        Some("https://agentnotch.example.com/dashboard")
    );
    assert_eq!(
        state.last_sync_at.map(date::to_string).as_deref(),
        Some("2026-09-25T10:00:30.250Z")
    );
    assert_eq!(state.sessions.len(), 3);
    let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let work = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";
    let ended = &state.sessions[&format!("0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b|{work}")];
    assert!(ended.ended);
    assert_eq!(ended.version, Some(2));
    assert_eq!(ended.transcript.unwrap().bytes, 48213);
    assert_eq!(ended.transcript.unwrap().modified, 1790326800.5);
    assert!(ended.summary.is_some());
    // Built before there was a payload version, a transcript or a summary:
    // all three absent, and they stay absent.
    let old = &state.sessions[&format!("11111111-2222-4333-8444-555555555555|{personal}")];
    assert!(!old.ended);
    assert_eq!(old.version, None);
    assert_eq!(old.transcript, None);
    assert_eq!(old.summary, None);
    let written = to_json(&state);
    assert_equivalent(&bytes, &written, "cloud-sync-state.json");
    // The Mac's encoder writes exactly this: compact, keys sorted, absent
    // optionals left out.
    assert_eq!(written, bytes);

    // Through the memory: read from <support>, written back the same.
    let support = tempfile::tempdir().unwrap();
    let file = support.path().join(SYNC_STATE_FILE);
    std::fs::write(&file, &bytes).unwrap();
    let kept = CloudSyncMemory::in_support(support.path(), Arc::new(StdSecureFiles), true);
    assert_eq!(kept.contents(), state);
    assert_eq!(kept.sent_count(), 3);
    kept.save_now();
    assert_eq!(std::fs::read(&file).unwrap(), bytes);

    // Another version starts over.
    let other = String::from_utf8(bytes)
        .unwrap()
        .replace(r#""version":2,"website""#, r#""version":1,"website""#);
    std::fs::write(&file, other).unwrap();
    let fresh = CloudSyncMemory::in_support(support.path(), Arc::new(StdSecureFiles), true);
    assert_eq!(fresh.sent_count(), 0);
    assert_eq!(fresh.user_id(), None);
}
