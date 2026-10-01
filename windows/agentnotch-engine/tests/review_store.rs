//! `ReviewStore` (ReviewStateStore's port): the Mac's files round-trip, the
//! legacy shape is read and rewritten as v2, failures persist with their
//! time, records expire after 7 days, and the delays of the writes.
//!
//! Ported from A1_AttentionAndReviewTests (readsSuperpoweredVibeNotchsFile
//! AndWritesTheNewShape, failuresPersistWithTheirTime); the file-mode check
//! (0600) belongs to the writer (`SecureFiles::write_atomic`, WP2).

use agentnotch_engine::core::time::{from_ms, to_secs_f64};
use agentnotch_engine::model::ReviewItem;
use agentnotch_engine::review::store::{
    HEARTBEAT_INTERVAL, MAX_MESSAGE_LENGTH, RETENTION, WRITE_DELAY,
};
use agentnotch_engine::review::{ReviewSnapshot, ReviewStore, StopFailure};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FIXTURE: &[u8] = include_bytes!("fixtures/mac-files/review-state.json");
const FIXTURE_SPVN: &[u8] = include_bytes!("fixtures/mac-files/review-state-spvn.json");

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn item(updated_at: SystemTime) -> ReviewItem {
    ReviewItem::new(updated_at)
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("json")
}

#[test]
fn the_macs_file_round_trips_byte_for_byte() {
    // The fixture's lastAliveAt is 1790000000.25.
    let now = UNIX_EPOCH + Duration::from_millis(1_790_000_000_250);
    let mut store = ReviewStore::load(Some(FIXTURE), now);
    assert_eq!(store.len(), 3);
    assert_eq!(store.previous_run_alive_at(), Some(now));
    assert_eq!(store.last_alive_at(), Some(now));
    assert_eq!(
        String::from_utf8(store.encode(now)).unwrap(),
        String::from_utf8(FIXTURE.to_vec()).unwrap()
    );

    let waiting = store
        .record("5d1e0a7b-3c21-4f7e-9a0b-1c2d3e4f5a6b")
        .unwrap();
    assert_eq!(waiting.background_agent_types, ["subagent", "workflow"]);
    assert_eq!(waiting.completed_at, Some(from_ms(1_789_999_000_125)));
    let failed = store
        .record("9f8e7d6c-5b4a-4321-8fed-cba987654321")
        .unwrap();
    assert_eq!(failed.stop_error_code.as_deref(), Some("rate_limit"));
}

#[test]
fn no_file_and_unreadable_files_are_an_empty_store() {
    let now = at(1_790_000_000);
    for bytes in [
        None,
        Some(&b"not json"[..]),
        Some(&b"[1]"[..]),
        Some(&b""[..]),
    ] {
        let store = ReviewStore::load(bytes, now);
        assert!(store.is_empty());
        assert_eq!(store.previous_run_alive_at(), None);
    }
}

#[test]
fn reads_superpowered_vibe_notchs_file_and_writes_the_new_shape() {
    let now = at(1_790_000_000);
    let legacy = format!(
        r#"{{"s1":{{"completedAt":{c},"lastAssistantMessage":"Shipped","updatedAt":{c}}}}}"#,
        c = 1_790_000_000 - 60
    );
    let mut store = ReviewStore::load(Some(legacy.as_bytes()), now);
    assert_eq!(
        store
            .record("s1")
            .unwrap()
            .last_assistant_message
            .as_deref(),
        Some("Shipped")
    );
    assert_eq!(store.previous_run_alive_at(), None);
    // The fixture shape too: the bare map of the Mac's SPVN file.
    let spvn = ReviewStore::load(Some(FIXTURE_SPVN), at(1_790_000_000));
    assert_eq!(spvn.len(), 1);

    let written = json(&store.encode(now));
    assert_eq!(written["version"], 2);
    assert_eq!(written["lastAliveAt"], 1_790_000_000);
    assert!(written["sessions"]["s1"].is_object());
    // A later run reads its predecessor's heartbeat.
    let bytes = store.encode(now);
    assert_eq!(
        ReviewStore::load(Some(&bytes), now).previous_run_alive_at(),
        Some(now)
    );
}

#[test]
fn failures_persist_with_their_time() {
    let now = at(1_790_000_000);
    let failed_at = at(1_800_000_000);
    let mut store = ReviewStore::empty();
    let mut failure = item(now);
    failure.stop_error = Some("Rate limited".into());
    failure.stop_error_code = Some("rate_limit".into());
    failure.failed_at = Some(failed_at);
    assert_eq!(store.update("s1", failure, true), Some(Duration::ZERO));

    let bytes = store.encode(now);
    let reloaded = ReviewStore::load(Some(&bytes), now);
    let record = reloaded.record("s1").unwrap();
    assert_eq!(record.stop_error.as_deref(), Some("Rate limited"));
    assert_eq!(record.stop_error_code.as_deref(), Some("rate_limit"));
    assert_eq!(record.failed_at, Some(failed_at));

    // Nothing left to keep: the record goes.
    assert!(store.update("s1", item(now), true).is_some());
    let bytes = store.encode(now);
    assert!(ReviewStore::load(Some(&bytes), now).record("s1").is_none());
    assert_eq!(json(&bytes)["sessions"], serde_json::json!({}));
}

#[test]
fn records_expire_after_seven_days() {
    let now = at(1_790_000_000);
    let mut file = String::from(r#"{"version":2,"sessions":{"#);
    let edge = 1_790_000_000 - RETENTION.as_secs();
    file.push_str(&format!(
        r#""old":{{"completedAt":1,"updatedAt":{}}},"edge":{{"completedAt":1,"updatedAt":{}}},"fresh":{{"completedAt":1,"updatedAt":{}}}}}}}"#,
        edge - 1,
        edge,
        edge + 1
    ));
    let store = ReviewStore::load(Some(file.as_bytes()), now);
    assert!(
        store.record("old").is_none(),
        "older than 7 days is dropped at load"
    );
    assert!(store.record("edge").is_some(), "exactly 7 days is kept");
    assert!(store.record("fresh").is_some());

    // And while the app runs: the next write prunes what grew old.
    let mut store = store;
    let later = now + Duration::from_secs(1);
    let written = json(&store.encode(later));
    assert!(written["sessions"].get("edge").is_none());
    assert!(written["sessions"].get("fresh").is_some());
    assert_eq!(store.len(), 1);
}

#[test]
fn an_unusable_date_drops_the_record() {
    let file = br#"{"version":2,"sessions":{"bad":{"updatedAt":1e300},"ok":{"completedAt":5,"updatedAt":1790000000}}}"#;
    let store = ReviewStore::load(Some(file), at(1_790_000_000));
    assert!(store.record("bad").is_none());
    assert!(store.record("ok").is_some());
}

#[test]
fn urgent_writes_are_at_once_marks_are_debounced_and_nothing_changed_is_no_write() {
    let t = at(1_790_000_000);
    let mut store = ReviewStore::empty();
    let mut done = item(t);
    done.completed_at = Some(t);
    done.last_assistant_message = Some("Done.".into());
    assert_eq!(store.update("s", done.clone(), true), Some(Duration::ZERO));

    // The same content written again, later: no write, whatever `urgent` says.
    let mut again = done.clone();
    again.updated_at = t + Duration::from_secs(60);
    assert_eq!(store.update("s", again, true), None);
    assert_eq!(store.record("s").unwrap().updated_at, t, "kept as it was");

    // A review mark: a second's debounce.
    let mut reviewed = done.clone();
    reviewed.reviewed_at = Some(t + Duration::from_secs(5));
    reviewed.updated_at = t + Duration::from_secs(5);
    assert_eq!(store.update("s", reviewed, false), Some(WRITE_DELAY));
    assert_eq!(WRITE_DELAY, Duration::from_secs(1));
    assert_eq!(
        store.record("s").unwrap().reviewed_at,
        Some(t + Duration::from_secs(5))
    );

    // An empty record for a session never kept is no change either.
    assert_eq!(store.update("other", item(t), true), None);
    assert_eq!(store.len(), 1);

    assert_eq!(store.remove("s"), Some(WRITE_DELAY));
    assert_eq!(store.remove("s"), None);
    assert!(store.update("a", done.clone(), false).is_some());
    assert!(store.update("b", done, false).is_some());
    assert_eq!(store.reset(), Some(WRITE_DELAY));
    assert!(store.is_empty());
    assert_eq!(store.reset(), None);
}

#[test]
fn snapshots_keep_a_failure_only_while_it_counts() {
    let t = at(1_790_000_000);
    let failure = || {
        Some(StopFailure {
            text: "Rate limited".into(),
            code: Some("rate_limit".into()),
            at: Some(t),
        })
    };
    let types = vec!["workflow".to_owned()];
    let failed = ReviewSnapshot::capture(Some(t), None, None, None, &types, failure(), true);
    assert_eq!(failed.stop_error.as_deref(), Some("Rate limited"));
    assert_eq!(failed.stop_error_code.as_deref(), Some("rate_limit"));
    assert_eq!(failed.failed_at, Some(t));
    assert!(
        failed.background_agent_types.is_empty(),
        "no wait, no agent types"
    );

    // The failure was answered or replaced by a later turn: not persisted.
    let answered = ReviewSnapshot::capture(Some(t), None, None, Some(t), &types, failure(), false);
    assert_eq!(answered.stop_error, None);
    assert_eq!(answered.stop_error_code, None);
    assert_eq!(answered.failed_at, None);
    assert_eq!(answered.background_agent_types, types);

    let mut store = ReviewStore::empty();
    let none = ReviewSnapshot::default();
    // A failure is written at once; the same snapshot again changes nothing.
    assert_eq!(
        store.persist_if_changed("s", &none, &failed, t),
        Some(Duration::ZERO)
    );
    assert_eq!(store.persist_if_changed("s", &failed, &failed, t), None);
    // A review mark alone is debounced; a new completion is urgent.
    let mut reviewed = failed.clone();
    reviewed.reviewed_at = Some(t);
    assert_eq!(
        store.persist_if_changed("s", &failed, &reviewed, t),
        Some(WRITE_DELAY)
    );
    let mut completed = reviewed.clone();
    completed.completed_at = Some(t + Duration::from_secs(9));
    assert_eq!(
        store.persist_if_changed("s", &reviewed, &completed, t),
        Some(Duration::ZERO)
    );
    // The failure is gone from the snapshot (answered): also urgent.
    let cleared = ReviewSnapshot {
        stop_error: None,
        stop_error_code: None,
        failed_at: None,
        ..completed.clone()
    };
    assert_eq!(
        store.persist_if_changed("s", &completed, &cleared, t),
        Some(Duration::ZERO)
    );
    assert_eq!(store.record("s").unwrap().stop_error, None);
}

#[test]
fn the_heartbeat_is_due_at_once_then_every_thirty_seconds() {
    let t = at(1_790_000_000);
    let mut store = ReviewStore::empty();
    assert_eq!(store.next_heartbeat(t), t);
    assert_eq!(store.last_alive_at(), None);

    let bytes = store.encode(t);
    assert_eq!(json(&bytes)["lastAliveAt"], 1_790_000_000);
    assert_eq!(store.last_alive_at(), Some(t));
    assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(30));
    // Whatever "now" is, the next one is 30 s after the last write.
    assert_eq!(
        store.next_heartbeat(t + Duration::from_secs(7)),
        t + HEARTBEAT_INTERVAL
    );

    // A write for another reason is a heartbeat as well.
    let later = t + Duration::from_secs(12);
    store.encode(later);
    assert_eq!(store.next_heartbeat(later), later + HEARTBEAT_INTERVAL);
    // The previous run's own heartbeat is not moved by this run's.
    assert_eq!(store.previous_run_alive_at(), None);
}

#[test]
fn the_message_cut_counts_characters_not_bytes() {
    let t = at(1_790_000_000);
    let mut store = ReviewStore::empty();
    // 3-byte and 4-byte characters: a byte cut at 1500 would split one.
    let long: String = "é€😀"
        .chars()
        .cycle()
        .take(MAX_MESSAGE_LENGTH + 40)
        .collect();
    let mut with_message = item(t);
    with_message.completed_at = Some(t);
    with_message.last_assistant_message = Some(long.clone());
    store.update("s", with_message, false);

    let kept = store
        .record("s")
        .unwrap()
        .last_assistant_message
        .clone()
        .unwrap();
    assert_eq!(kept.chars().count(), MAX_MESSAGE_LENGTH);
    assert!(long.starts_with(&kept));

    let bytes = store.encode(t);
    let reloaded = ReviewStore::load(Some(&bytes), t);
    let message = reloaded
        .record("s")
        .unwrap()
        .last_assistant_message
        .clone()
        .unwrap();
    assert_eq!(message, kept);

    // Exactly 1500 characters is kept whole; the cut is not applied twice.
    let exact: String = "😀".repeat(MAX_MESSAGE_LENGTH);
    let mut item = item(t);
    item.completed_at = Some(t);
    item.last_assistant_message = Some(exact.clone());
    store.update("exact", item, false);
    assert_eq!(
        store
            .record("exact")
            .unwrap()
            .last_assistant_message
            .as_deref(),
        Some(exact.as_str())
    );
}

#[test]
fn dates_are_epoch_second_doubles() {
    let t = UNIX_EPOCH + Duration::from_millis(1_789_999_000_125);
    let mut store = ReviewStore::empty();
    let mut record = item(t);
    record.completed_at = Some(t);
    store.update("s", record, false);
    let bytes = store.encode(t);
    let value = json(&bytes);
    assert_eq!(
        value["sessions"]["s"]["completedAt"].as_f64(),
        Some(to_secs_f64(t))
    );
    assert_eq!(
        value["sessions"]["s"]["completedAt"].as_f64(),
        Some(1_789_999_000.125)
    );
    // Compact: no whitespace outside strings.
    assert!(!String::from_utf8(bytes).unwrap().contains(['\n', ' ']));
}
