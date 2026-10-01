//! One sync pass (CL§5.4-5.7, §7.3; the Mac's `CloudSync.swift` 54-660):
//! what the website has, the stores, the payloads and their costs, the
//! batches, the backfill, summaries, and the mappings the engine's
//! `CloudDeps` is made from. No HTTP: the service sends what a pass
//! prepares.

mod cloud_support;

use agentnotch_engine::cloud::backfill::{self, Folder};
use agentnotch_engine::cloud::contract::{
    self, SessionSource, SyncAccount, SyncDevice, SyncProject, SyncSession, SyncTokens, SyncWindow,
    UsageSourceName,
};
use agentnotch_engine::cloud::environment;
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::ledger::{CloudLedgerEntry, Origin};
use agentnotch_engine::cloud::pass::{
    allowed_accounts, search_transcript, Batch, BatchLimits, CloudAccountInfo, CloudBatcher,
    CloudStores, CloudSyncMemory, CloudSyncPass, Input, Record, TranscriptStamp,
};
use agentnotch_engine::cloud::recorder::RecordedUsageReading;
use agentnotch_engine::cloud::summary::run::Summary;
use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{
    Account, AccountId, FolderKind, FolderSource, Identity, IdentityId, LiveSessionObservation,
    RingId, RunFolder,
};
use agentnotch_engine::runtime_types::CloudAccount;
use cloud_support::{ids, live_for, CloudFixture as F, Lines as L};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const A: &str = F::SESSION_A;
const B: &str = F::SESSION_B;
const C: &str = F::SESSION_C;
const D: &str = "22222222-3333-4444-8555-666666666666";
const E: &str = "33333333-4444-4555-8666-777777777777";

fn device() -> SyncDevice {
    SyncDevice {
        id: "5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d".to_owned(),
        name: "PC".to_owned(),
        app_version: "1".to_owned(),
    }
}

fn infos() -> Vec<CloudAccountInfo> {
    environment::account_infos(&[F::account(), F::work_account()])
}

/// A temporary home with a `.claude` folder and the stores kept in memory.
struct Harness {
    _root: tempfile::TempDir,
    home: PathBuf,
    support: PathBuf,
    stores: CloudStores,
}

impl Harness {
    fn new() -> Self {
        Self::with(false)
    }

    fn with(persist: bool) -> Self {
        let root = tempfile::tempdir().expect("a temporary folder");
        // The real path: macOS's temporary folder is reached through a
        // link, which the backfill refuses to read history through.
        let base = cloud_support::real_path(root.path());
        let home = base.join("home");
        let support = base.join("support");
        std::fs::create_dir_all(home.join(".claude").join("projects")).unwrap();
        let stores = CloudStores::new(&support, Arc::new(StdSecureFiles), persist, &home);
        Harness {
            _root: root,
            home,
            support,
            stores,
        }
    }

    fn transcript(&self, id: &str) -> PathBuf {
        self.home
            .join(".claude")
            .join("projects")
            .join("-Users-me-code-app")
            .join(format!("{id}.jsonl"))
    }

    fn write(&self, id: &str, lines: &[String]) {
        L::write(lines, &self.transcript(id), false);
    }

    fn append(&self, id: &str, lines: &[String]) {
        L::write(lines, &self.transcript(id), true);
    }

    /// The hub's sighting of a running session whose transcript is ours.
    fn observation(
        &self,
        id: &str,
        account: &CloudAccount,
        seconds: f64,
    ) -> LiveSessionObservation {
        let mut observation = live_for(id, account).active(seconds).build();
        observation.transcript_path = Some(self.transcript(id).to_string_lossy().into_owned());
        observation.config_dir = Some(self.home.join(".claude").to_string_lossy().into_owned());
        observation
    }

    fn see(&self, observation: LiveSessionObservation, seconds: f64) {
        let account = infos()
            .into_iter()
            .find(|a| a.account_key == observation.account_key)
            .expect("a known account");
        let accounts = BTreeMap::from([(account.account_key.clone(), account.ledger_account())]);
        let id = observation.session_id.clone();
        self.stores.ledger.observe(
            &[observation],
            &ids(&[&id]),
            &ids(&[]),
            &ids(&[]),
            &accounts,
            F::at(seconds),
        );
    }

    /// Seen running as the first account at `seconds`.
    fn run(&self, id: &str, seconds: f64) {
        self.see(self.observation(id, &F::account(), seconds), seconds);
    }

    /// Not seen since `last_seen`: noticed gone ten seconds on, ended a
    /// minute after that.
    fn end(&self, last_seen: f64) {
        assert!(self
            .stores
            .ledger
            .settle(&ids(&[]), F::at(last_seen + 10.0))
            .is_empty());
        let ended = self
            .stores
            .ledger
            .settle(&ids(&[]), F::at(last_seen + 80.0));
        assert!(!ended.is_empty(), "something ended");
    }

    fn input(&self) -> Input {
        Input::new(infos(), device(), F::at(10_000.0))
    }

    fn prepare(&self) -> agentnotch_engine::cloud::pass::Prepared {
        CloudSyncPass::prepare(&self.input(), &self.stores)
    }

    fn entry(&self, id: &str) -> CloudLedgerEntry {
        self.stores.ledger.entry(id).expect("in the ledger")
    }

    fn payload(&self, id: &str) -> Option<SyncSession> {
        CloudSyncPass::payload(&self.entry(id), &self.stores, false, &[], F::at(10_000.0))
            .map(|p| p.session)
    }
}

/// `n` responses of the session as the first account, the first at
/// `first` seconds, 10 s apart.
fn responses(id: &str, model: &str, input: i64, output: i64, first: f64) -> Vec<String> {
    vec![
        L::user("MY SECRET PROMPT", id, first - 5.0),
        L::assistant(&format!("msg_{id}_1"), &format!("req_{id}_1"), id)
            .model(model)
            .usage(input, output)
            .at(first)
            .tool_use()
            .line(),
        L::ai_title("Fix it", id),
    ]
}

fn records_of(batch: &Batch) -> BTreeMap<String, Record> {
    batch.records.clone()
}

// ---- The contract's limits ----

#[test]
fn requests_keep_to_the_contracts_limits() {
    let device = device();
    let mut accounts: BTreeMap<String, CloudAccountInfo> = BTreeMap::new();
    for index in 0..60 {
        let key = keys::sha256_hex(format!("account-{index}"));
        accounts.insert(
            key.clone(),
            CloudAccountInfo {
                identity_id: IdentityId::from(format!("uuid:{index}")),
                account_key: key,
                account_uuid: index.to_string(),
                email: None,
                organization_name: None,
                plan: None,
                label: None,
            },
        );
    }
    let account_keys: Vec<String> = accounts.keys().cloned().collect();
    let sessions: Vec<(SyncSession, Record)> = (0..450)
        .map(|index| {
            let key = account_keys[index % account_keys.len()].clone();
            (
                SyncSession {
                    account_key: key,
                    session_id: format!("00000000-0000-4000-8000-{index:012x}"),
                    project: SyncProject {
                        key: keys::sha256_hex("p"),
                        name: "p".to_owned(),
                    },
                    title: None,
                    source: SessionSource::Cli,
                    models: Vec::new(),
                    started_at: F::base(),
                    last_activity_at: F::base(),
                    ended_at: None,
                    message_count: 1,
                    tokens: SyncTokens::default(),
                    cost_usd: None,
                    summary: None,
                },
                Record::new(index.to_string()),
            )
        })
        .collect();
    let readings: Vec<RecordedUsageReading> = (0..1_200)
        .map(|index| RecordedUsageReading {
            account_key: account_keys[(index * 7) % account_keys.len()].clone(),
            identity_id: IdentityId::from("x"),
            source: UsageSourceName::Probe,
            observed_at: F::at(index as f64),
            windows: vec![SyncWindow {
                id: "session".to_owned(),
                utilization: 1.0,
                resets_at: None,
            }],
        })
        .collect();
    let batches = CloudBatcher::batches(
        sessions,
        readings,
        &accounts,
        &device,
        contract::limit::SESSIONS,
    );
    assert_eq!(
        batches
            .iter()
            .map(|b| b.request.sessions.len())
            .sum::<usize>(),
        450
    );
    assert_eq!(
        batches.iter().map(|b| b.request.usage.len()).sum::<usize>(),
        1_200
    );
    assert_eq!(
        batches.iter().map(|b| b.readings.len()).sum::<usize>(),
        1_200
    );
    for batch in &batches {
        let request = &batch.request;
        assert!(
            request.sessions.len() <= 200
                && request.usage.len() <= 500
                && request.accounts.len() <= 50
        );
        let listed: BTreeSet<&str> = request.accounts.iter().map(|a| a.key.as_str()).collect();
        assert!(request
            .sessions
            .iter()
            .all(|s| listed.contains(s.account_key.as_str())));
        assert!(request
            .usage
            .iter()
            .all(|u| listed.contains(u.account_key.as_str())));
        assert_eq!(&request.clamped(F::base()), request);
        let record_keys: BTreeSet<String> = batch.records.keys().cloned().collect();
        let session_keys: BTreeSet<String> = request
            .sessions
            .iter()
            .map(|s| CloudLedgerEntry::key_of(&s.session_id, &s.account_key))
            .collect();
        assert_eq!(record_keys, session_keys);
        // Accounts are listed sorted by key.
        let keys_listed: Vec<&str> = request.accounts.iter().map(|a| a.key.as_str()).collect();
        let mut sorted = keys_listed.clone();
        sorted.sort_unstable();
        assert_eq!(keys_listed, sorted);
    }
}

#[test]
fn a_batcher_leaves_out_what_has_no_account_and_makes_no_empty_request() {
    let accounts = allowed_accounts(&infos());
    let stranger = SyncSession {
        account_key: keys::sha256_hex("stranger"),
        session_id: A.to_owned(),
        project: SyncProject {
            key: keys::sha256_hex("p"),
            name: "p".to_owned(),
        },
        title: None,
        source: SessionSource::Cli,
        models: Vec::new(),
        started_at: F::base(),
        last_activity_at: F::base(),
        ended_at: None,
        message_count: 1,
        tokens: SyncTokens::default(),
        cost_usd: None,
        summary: None,
    };
    let none = CloudBatcher::batches(
        vec![(stranger, Record::new("x"))],
        Vec::new(),
        &accounts,
        &device(),
        200,
    );
    assert!(none.is_empty());
    // Smaller limits split sooner (the website's 413 answer does that).
    let account = &infos()[0];
    let make = |n: usize| SyncSession {
        account_key: account.account_key.clone(),
        session_id: format!("00000000-0000-4000-8000-{n:012x}"),
        project: SyncProject {
            key: keys::sha256_hex("p"),
            name: "p".to_owned(),
        },
        title: None,
        source: SessionSource::Cli,
        models: Vec::new(),
        started_at: F::base(),
        last_activity_at: F::base(),
        ended_at: None,
        message_count: 1,
        tokens: SyncTokens::default(),
        cost_usd: None,
        summary: None,
    };
    let sessions: Vec<_> = (0..5)
        .map(|n| (make(n), Record::new(n.to_string())))
        .collect();
    let split = CloudBatcher::batches_within(
        sessions,
        Vec::new(),
        &accounts,
        &device(),
        BatchLimits {
            sessions: 2,
            ..BatchLimits::default()
        },
    );
    assert_eq!(
        split
            .iter()
            .map(|b| b.request.sessions.len())
            .collect::<Vec<_>>(),
        [2, 2, 1]
    );
    assert!(split.iter().all(|b| b.request.accounts.len() == 1));
}

// ---- What a pass prepares ----

#[test]
fn an_unchanged_session_is_not_prepared_again() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    h.run(A, 10.0);
    let first = h.prepare();
    assert_eq!(first.session_count, 1);
    assert_eq!(first.batches.len(), 1);
    assert!(!first.has_more);
    let key = h.entry(A).key();
    assert_eq!(first.batches[0].records.keys().collect::<Vec<_>>(), [&key]);
    // The service sends it, then remembers it.
    h.stores
        .memory
        .mark_sent(&records_of(&first.batches[0]), F::at(10_000.0));
    let second = h.prepare();
    assert_eq!(second.session_count, 0);
    assert!(second.batches.is_empty());
    // It grows: it is sent again.
    h.append(
        A,
        &[L::assistant("msg_more", "req_more", A)
            .usage(10, 10)
            .at(40.0)
            .line()],
    );
    let third = h.prepare();
    assert_eq!(third.session_count, 1);
    assert_eq!(third.batches[0].request.sessions[0].message_count, 2);
}

#[test]
fn a_session_with_no_response_of_its_account_is_not_reported() {
    let h = Harness::new();
    h.write(A, &[L::user("hello", A, 0.0)]);
    h.run(A, 10.0);
    let prepared = h.prepare();
    assert_eq!(prepared.session_count, 0);
    assert!(prepared.batches.is_empty());
    assert!(h.payload(A).is_none());
}

#[test]
fn a_payload_carries_names_and_numbers_only() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    h.run(A, 10.0);
    let prepared = h.prepare();
    let request = &prepared.batches[0].request;
    let text = String::from_utf8(contract::to_json(request)).unwrap();
    for secret in [
        "MY SECRET PROMPT",
        "Done.",
        "cat secrets.txt",
        "/Users/me",
        "code/app",
        h.home.to_str().unwrap(),
        h.support.to_str().unwrap(),
        ".jsonl",
    ] {
        assert!(!text.contains(secret), "{secret} leaked into {text}");
    }
    let session = &request.sessions[0];
    assert_eq!(session.account_key, F::ACCOUNT_KEY);
    assert_eq!(session.session_id, A);
    assert_eq!(session.title.as_deref(), Some("Fix it"));
    assert_eq!(session.project.name, "app");
    // The key is the HMAC of the account and the project's path with this
    // install's secret: never the path.
    let entry = h.entry(A);
    assert_eq!(
        session.project.key,
        keys::project_key(F::ACCOUNT_KEY, &entry.project_path, &h.stores.secret())
    );
    assert_eq!(session.source, SessionSource::Vscode);
    assert_eq!(session.models, ["claude-opus-4-5"]);
    assert_eq!(session.message_count, 1);
    assert_eq!(session.tokens.input, 1000);
    assert_eq!(session.tokens.output, 500);
    assert_eq!(session.started_at, F::at(5.0).min(session.started_at));
    assert_eq!(request.accounts.len(), 1);
    assert_eq!(request.accounts[0].key, F::ACCOUNT_KEY);
    assert_eq!(request.accounts[0].email.as_deref(), Some("me@example.com"));
    assert_eq!(request.device, device());
    assert_eq!(request.clamped(F::at(10_000.0)), *request);
}

// ---- Costs ----

/// The cost a session of `model` with `usage` has when Claude Code reported
/// `reported`, built as the first account's whole session.
fn cost_of(
    h: &Harness,
    id: &str,
    model: &str,
    usage: (i64, i64),
    reported: Option<f64>,
) -> Option<f64> {
    h.write(id, &responses(id, model, usage.0, usage.1, 10.0));
    let mut observation = h.observation(id, &F::account(), 10.0);
    observation.cost_usd = reported;
    h.see(observation, 10.0);
    h.payload(id).expect("a payload").cost_usd
}

#[test]
fn the_cost_is_claude_codes_figure_unless_the_estimate_is_larger() {
    let h = Harness::new();
    // The estimate of 1000 in and 500 out on Opus 4.5: 5 and 25 dollars a
    // million tokens.
    let estimate = 0.0175;
    // Reported only: the model has no known price.
    assert_eq!(
        cost_of(&h, A, "mystery-model-9", (1000, 500), Some(2.5)),
        Some(2.5)
    );
    // The estimate only: nothing reported.
    let only = cost_of(&h, B, "claude-opus-4-5", (1000, 500), None).expect("priced");
    assert!((only - estimate).abs() < 1e-9, "{only}");
    // Both: the larger. Claude Code counts calls the transcript doesn't
    // record, so its figure normally wins...
    assert_eq!(
        cost_of(&h, C, "claude-opus-4-5", (1000, 500), Some(5.0)),
        Some(5.0)
    );
    // ...unless it no longer covers the session.
    let outgrown = cost_of(&h, D, "claude-opus-4-5", (1000, 500), Some(0.001)).expect("priced");
    assert!((outgrown - estimate).abs() < 1e-9, "{outgrown}");
    // Neither: unknown, never a number made up.
    assert_eq!(cost_of(&h, E, "mystery-model-9", (1000, 500), None), None);
}

#[test]
fn a_split_session_is_priced_per_part_and_never_carries_the_status_lines_cost() {
    let h = Harness::new();
    let (personal, work) = (F::account(), F::work_account());
    // The first account ran it until 900 s, the second resumed it.
    h.write(
        A,
        &[
            L::user("hello", A, 0.0),
            L::assistant("msg_p", "req_p", A)
                .model("claude-opus-4-5")
                .usage(1000, 500)
                .at(500.0)
                .line(),
            L::assistant("msg_w", "req_w", A)
                .model("claude-opus-4-5")
                .usage(2000, 1000)
                .at(950.0)
                .line(),
        ],
    );
    let mut first = h.observation(A, &personal, 600.0);
    first.cost_usd = Some(9.0);
    h.see(first, 600.0);
    let mut resumed = h.observation(A, &work, 960.0);
    resumed.started_at = F::at(950.0);
    resumed.process_started_at = Some(F::at(900.0));
    resumed.cost_usd = Some(9.0);
    h.see(resumed, 960.0);
    let segments = h.stores.ledger.segments(A);
    assert_eq!(segments.len(), 2);
    let mut costs = BTreeMap::new();
    for entry in &segments {
        let payload =
            CloudSyncPass::payload(entry, &h.stores, false, &[], F::at(10_000.0)).expect("part");
        assert_eq!(payload.session.message_count, 1);
        costs.insert(entry.account_key.clone(), payload.session.cost_usd.unwrap());
    }
    // Each part's own responses at list prices, not the 9 dollars the
    // status line gave for the whole process.
    assert!((costs[F::ACCOUNT_KEY] - 0.0175).abs() < 1e-9, "{costs:?}");
    assert!(
        (costs[F::WORK_ACCOUNT_KEY] - 0.035).abs() < 1e-9,
        "{costs:?}"
    );

    let totals = h.stores.scanner.cached_summary(A).expect("scanned");
    assert!(CloudSyncPass::is_split(&totals, F::ACCOUNT_KEY));
    assert!(CloudSyncPass::is_split(&totals, F::WORK_ACCOUNT_KEY));
    // Against anyone else, both parts are other people's.
    assert!(CloudSyncPass::is_split(&totals, "nobody"));
}

#[test]
fn a_part_that_only_nobody_ran_does_not_make_a_session_split() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    let mut observation = h.observation(A, &F::account(), 10.0);
    observation.cost_usd = Some(3.0);
    h.see(observation, 10.0);
    let totals = h
        .stores
        .scanner
        .scan(
            A,
            h.transcript(A).to_str().unwrap(),
            &h.stores.ledger.owners(A),
        )
        .expect("scanned");
    assert!(!CloudSyncPass::is_split(&totals, F::ACCOUNT_KEY));
    assert_eq!(h.payload(A).unwrap().cost_usd, Some(3.0));
}

// ---- When a session is built again ----

#[test]
fn an_ended_session_sent_the_way_it_is_built_now_costs_a_stat() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    h.run(A, 10.0);
    h.end(10.0);
    let key = h.entry(A).key();
    let path = h.transcript(A);
    let stamp = TranscriptStamp::of(&StdSecureFiles, path.to_str().unwrap()).expect("a stamp");
    // Sent as ended, in this version, from this transcript, with a hash
    // that is not what building it gives: building it again would send it
    // again, so a session left alone proves nothing was built.
    let stale = Record {
        base: "stale".to_owned(),
        summary: None,
        ended: true,
        transcript: Some(stamp),
        version: Some(CloudSyncPass::PAYLOAD_VERSION),
    };
    h.stores
        .memory
        .mark_sent(&BTreeMap::from([(key.clone(), stale)]), F::at(200.0));
    assert_eq!(h.prepare().session_count, 0);
    // The transcript moved since: built again.
    h.append(
        A,
        &[L::assistant("msg_2", "req_2", A)
            .usage(1, 1)
            .at(50.0)
            .line()],
    );
    assert_eq!(h.prepare().session_count, 1);
}

#[test]
fn a_payload_version_bump_rebuilds_an_ended_session_once() {
    let h = Harness::new();
    h.write(A, &responses(A, "mystery-model-9", 1000, 500, 10.0));
    h.write(B, &responses(B, "mystery-model-9", 1000, 500, 10.0));
    h.run(A, 10.0);
    h.run(B, 10.0);
    h.end(10.0);
    let first = h.prepare();
    assert_eq!(first.session_count, 2);
    let mut sent = records_of(&first.batches[0]);
    assert_eq!(sent.len(), 2);
    for record in sent.values() {
        assert!(record.ended);
        assert_eq!(record.version, Some(CloudSyncPass::PAYLOAD_VERSION));
        assert!(record.transcript.is_some());
    }
    // Sent by an earlier way (version 1): A's website copy had no cost, but
    // is the same as it is built now; B's differs.
    let key_a = h.entry(A).key();
    let key_b = h.entry(B).key();
    for record in sent.values_mut() {
        record.version = Some(CloudSyncPass::PAYLOAD_VERSION - 1);
    }
    sent.get_mut(&key_b).unwrap().base = "built the old way".to_owned();
    h.stores.memory.mark_sent(&sent, F::at(200.0));
    let rebuilt = h.prepare();
    // Only the one that comes out different goes again.
    assert_eq!(rebuilt.session_count, 1);
    assert_eq!(rebuilt.batches[0].request.sessions[0].session_id, B);
    // The one that came out the same is remembered as built the new way,
    // so from now on a stat is enough.
    assert_eq!(
        h.stores.memory.sent(&key_a).unwrap().version,
        Some(CloudSyncPass::PAYLOAD_VERSION)
    );
    h.stores
        .memory
        .mark_sent(&records_of(&rebuilt.batches[0]), F::at(300.0));
    assert_eq!(h.prepare().session_count, 0);
    assert_eq!(
        h.stores.memory.sent(&key_b).unwrap().version,
        Some(CloudSyncPass::PAYLOAD_VERSION)
    );
}

#[test]
fn at_most_five_requests_go_in_a_pass_and_the_rest_waits() {
    let h = Harness::new();
    let ids_made: Vec<String> = (0..7)
        .map(|n| format!("00000000-0000-4000-8000-{:012x}", 0x100 + n))
        .collect();
    for (n, id) in ids_made.iter().enumerate() {
        h.write(
            id,
            &responses(id, "claude-opus-4-5", 10, 10, 10.0 + n as f64),
        );
        h.run(id, 10.0 + n as f64);
    }
    let mut input = h.input();
    input.max_sessions_per_request = 1;
    let prepared = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(prepared.session_count, 7);
    assert_eq!(prepared.batches.len(), CloudSyncPass::MAX_REQUESTS_PER_PASS);
    assert!(prepared.has_more);
    // Oldest first.
    let sent: Vec<&str> = prepared
        .batches
        .iter()
        .map(|b| b.request.sessions[0].session_id.as_str())
        .collect();
    assert_eq!(
        sent,
        ids_made[..5].iter().map(String::as_str).collect::<Vec<_>>()
    );
    // What went is remembered; the next pass gets the rest.
    for batch in &prepared.batches {
        h.stores.memory.mark_sent(&batch.records, F::at(10_000.0));
    }
    let next = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(next.session_count, 2);
    assert_eq!(next.batches.len(), 2);
    assert!(!next.has_more);
}

#[test]
fn readings_of_accounts_that_may_not_be_mentioned_are_dropped_and_the_rest_ride_along() {
    let h = Harness::new();
    let reading = |key: &str, at: f64| RecordedUsageReading {
        account_key: key.to_owned(),
        identity_id: IdentityId::from("x"),
        source: UsageSourceName::StatusLine,
        observed_at: F::at(at),
        windows: vec![SyncWindow {
            id: "session".to_owned(),
            utilization: 12.0,
            resets_at: None,
        }],
    };
    let stranger = keys::sha256_hex("stranger");
    h.stores.recorder.record(reading(F::ACCOUNT_KEY, 1.0));
    h.stores.recorder.record(reading(&stranger, 2.0));
    let prepared = h.prepare();
    assert_eq!(prepared.usage_count, 1);
    assert_eq!(prepared.session_count, 0);
    assert_eq!(prepared.batches.len(), 1);
    assert_eq!(prepared.batches[0].request.usage.len(), 1);
    assert_eq!(prepared.batches[0].readings.len(), 1);
    assert_eq!(h.stores.recorder.pending_count(), 1);
}

// ---- The backfill ----

fn own_folder(h: &Harness, since: f64) -> Folder {
    Folder {
        config_dir: h.home.join(".claude-own").to_string_lossy().into_owned(),
        identity_id: Some(IdentityId::from(F::IDENTITY_ID)),
        account_key: Some(F::ACCOUNT_KEY.to_owned()),
        login: backfill::login(Some(F::ACCOUNT_UUID), None, Some("me@example.com")),
        signed_in_since: Some(F::at(since)),
    }
}

fn own_transcript(h: &Harness, id: &str) -> PathBuf {
    h.home
        .join(".claude-own")
        .join("projects")
        .join("-Users-me-code-app")
        .join(format!("{id}.jsonl"))
}

fn user_in(id: &str, seconds: f64, entrypoint: &str) -> String {
    L::user_in("hello", id, seconds, "/Users/me/code/app", entrypoint)
}

#[test]
fn the_backfill_dates_and_orders_what_it_finds() {
    let h = Harness::new();
    let response = |id: &str, msg: &str, at: f64| {
        L::assistant(msg, &format!("req_{msg}"), id)
            .model("claude-opus-4-5")
            .usage(100, 50)
            .at(at)
            .line()
    };
    // The original: two responses, quiet for hours.
    let original = [
        user_in(A, 0.0, "cli"),
        response(A, "msg_1", 20.0),
        response(A, "msg_2", 100.0),
    ];
    L::write(&original, &own_transcript(&h, A), false);
    // A copy, resumed later under another id: it holds the original's
    // lines (still naming the original) and one of its own, still going.
    let copy = [
        user_in(B, 500.0, "cli"),
        response(A, "msg_1", 20.0),
        response(A, "msg_2", 100.0),
        response(B, "msg_3", 9_900.0),
    ];
    L::write(&copy, &own_transcript(&h, B), false);
    // Begun before the folder was seen signed in as the account: not its.
    L::write(
        &[user_in(C, -5_000.0, "cli"), response(C, "msg_c", -4_990.0)],
        &own_transcript(&h, C),
        false,
    );
    // Claude Desktop runs its sessions as whoever it is signed in as.
    L::write(
        &[
            user_in(D, 600.0, "claude-desktop"),
            response(D, "msg_d", 610.0),
        ],
        &own_transcript(&h, D),
        false,
    );
    // Nothing of its own in it.
    L::write(&[user_in(E, 700.0, "cli")], &own_transcript(&h, E), false);

    let mut input = h.input();
    input.now = F::at(10_000.0);
    input.backfill_folders = Some(vec![own_folder(&h, -1_000.0)]);
    let prepared = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(prepared.backfilled, 2);
    assert_eq!(prepared.session_count, 2);

    let a = h.entry(A);
    assert_eq!(a.origin, Origin::Backfill);
    assert_eq!(a.identity_id.as_str(), F::IDENTITY_ID);
    assert_eq!(a.account_key, F::ACCOUNT_KEY);
    assert_eq!(a.started_at, F::at(0.0));
    assert_eq!(a.last_activity_at, F::at(100.0));
    // Quiet for more than half an hour: it ended where it stopped.
    assert_eq!(a.ended_at, Some(F::at(100.0)));
    assert_eq!(a.project_name, "app");
    assert_eq!(a.model.as_deref(), Some("claude-opus-4-5"));
    assert_eq!(a.cost_usd, None);
    assert_eq!(
        a.config_dir.as_deref(),
        Some(h.home.join(".claude-own").to_str().unwrap())
    );
    assert_eq!(a.source, SessionSource::Cli);
    assert_eq!(a.title, None);
    assert_eq!(
        a.project_path,
        keys::project_path("/Users/me/code/app", &h.home, &StdSecureFiles)
    );
    // The copy still looked open when found: no end.
    let b = h.entry(B);
    assert_eq!(b.origin, Origin::Backfill);
    assert_eq!(b.started_at, F::at(500.0));
    assert_eq!(b.ended_at, None);
    // The original claimed its responses first: the copy holds only its own.
    let part = |id: &str| {
        h.stores
            .scanner
            .cached_summary(id)
            .unwrap()
            .part(F::ACCOUNT_KEY)
            .message_count
    };
    assert_eq!(part(A), 2);
    assert_eq!(part(B), 1);
    // Not found: before the sign-in, Claude Desktop's, and empty ones.
    for id in [C, D, E] {
        assert!(!h.stores.ledger.knows(id), "{id}");
    }

    // Oldest first in what goes: the original, then the copy.
    let sent: Vec<&str> = prepared.batches[0]
        .request
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(sent, [A, B]);
    assert_eq!(prepared.batches[0].request.sessions[0].message_count, 2);
    assert_eq!(
        prepared.batches[0].request.sessions[0].ended_at,
        Some(F::at(100.0))
    );

    // A second pass finds nothing new, and reads nothing again.
    for batch in &prepared.batches {
        h.stores.memory.mark_sent(&batch.records, F::at(10_000.0));
    }
    let again = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(again.backfilled, 0);
    assert_eq!(again.session_count, 0);
}

#[test]
fn a_copy_that_repeats_the_originals_responses_leaves_them_to_the_original() {
    let h = Harness::new();
    let response = |id: &str, msg: &str, at: f64| {
        L::assistant(msg, &format!("req_{msg}"), id)
            .usage(100, 50)
            .at(at)
            .line()
    };
    // The copy's file is written first (so it is the older file), but the
    // original began first: it is read first and keeps its responses.
    L::write(
        &[
            user_in(B, 500.0, "cli"),
            response(B, "msg_1", 600.0),
            response(B, "msg_3", 700.0),
        ],
        &own_transcript(&h, B),
        false,
    );
    L::write(
        &[user_in(A, 0.0, "cli"), response(A, "msg_1", 20.0)],
        &own_transcript(&h, A),
        false,
    );
    let mut input = h.input();
    input.backfill_folders = Some(vec![own_folder(&h, -1_000.0)]);
    let prepared = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(prepared.backfilled, 2);
    let count = |id: &str| {
        h.stores
            .scanner
            .cached_summary(id)
            .unwrap()
            .part(F::ACCOUNT_KEY)
            .message_count
    };
    assert_eq!(count(A), 1);
    assert_eq!(count(B), 1);
    let sent: Vec<&str> = prepared.batches[0]
        .request
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(sent, [A, B]);
}

#[test]
fn a_backfilled_session_that_was_still_going_ends_when_it_goes_quiet() {
    let h = Harness::new();
    let lines = [
        user_in(A, 0.0, "cli"),
        L::assistant("msg_1", "req_1", A)
            .usage(10, 10)
            .at(9_900.0)
            .line(),
    ];
    L::write(&lines, &own_transcript(&h, A), false);
    let mut input = h.input();
    input.now = F::at(10_000.0);
    input.backfill_folders = Some(vec![own_folder(&h, -1_000.0)]);
    let prepared = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(prepared.backfilled, 1);
    assert_eq!(h.entry(A).ended_at, None);
    assert_eq!(prepared.batches[0].request.sessions[0].ended_at, None);
    // Half an hour and a bit later it has not moved: it ended at its last
    // response, and the payload says so.
    let mut later = input.clone();
    later.now = F::at(9_900.0 + 31.0 * 60.0);
    later.backfill_folders = None;
    for batch in &prepared.batches {
        h.stores.memory.mark_sent(&batch.records, F::at(10_000.0));
    }
    let after = CloudSyncPass::prepare(&later, &h.stores);
    assert_eq!(after.session_count, 1);
    assert_eq!(h.entry(A).ended_at, Some(F::at(9_900.0)));
    assert_eq!(
        after.batches[0].request.sessions[0].ended_at,
        Some(F::at(9_900.0))
    );
}

#[test]
fn the_backfill_reads_at_most_its_budget_of_new_transcripts_a_pass() {
    let h = Harness::new();
    let total = CloudSyncPass::BACKFILL_FILES_PER_PASS + 3;
    for n in 0..total {
        let id = format!("00000000-0000-4000-8000-{:012x}", 0x1000 + n);
        L::write(
            &[
                user_in(&id, 10.0 + n as f64, "cli"),
                L::assistant(&format!("msg_{n}"), &format!("req_{n}"), &id)
                    .usage(1, 1)
                    .at(20.0 + n as f64)
                    .line(),
            ],
            &own_transcript(&h, &id),
            false,
        );
    }
    let mut input = h.input();
    input.backfill_folders = Some(vec![own_folder(&h, -1_000.0)]);
    let first = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(first.backfilled, CloudSyncPass::BACKFILL_FILES_PER_PASS);
    let second = CloudSyncPass::prepare(&input, &h.stores);
    assert_eq!(second.backfilled, 3);
    assert_eq!(h.stores.ledger.count(), total);
}

#[test]
fn a_folder_that_shares_its_history_is_never_backfilled() {
    let h = Harness::new();
    let lines = [
        user_in(A, 0.0, "cli"),
        L::assistant("msg_1", "req_1", A)
            .usage(1, 1)
            .at(10.0)
            .line(),
    ];
    L::write(&lines, &own_transcript(&h, A), false);
    let mut folder = own_folder(&h, -1_000.0);
    // Not offered for an account, or with no login history.
    folder.account_key = None;
    let mut input = h.input();
    input.backfill_folders = Some(vec![folder]);
    assert_eq!(CloudSyncPass::prepare(&input, &h.stores).backfilled, 0);
    let mut unknown = own_folder(&h, -1_000.0);
    unknown.signed_in_since = None;
    input.backfill_folders = Some(vec![unknown]);
    assert_eq!(CloudSyncPass::prepare(&input, &h.stores).backfilled, 0);
    // An account the website may not hear of (any more): nothing either.
    input.backfill_folders = Some(vec![own_folder(&h, -1_000.0)]);
    input.accounts = environment::account_infos(&[F::work_account()]);
    assert_eq!(CloudSyncPass::prepare(&input, &h.stores).backfilled, 0);
    assert!(!h.stores.ledger.knows(A));
}

#[test]
fn a_transcript_the_registry_names_but_the_ledger_has_no_path_for_is_looked_for() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 10, 10, 10.0));
    let mut observation = h.observation(A, &F::account(), 10.0);
    observation.transcript_path = None;
    h.see(observation, 10.0);
    assert_eq!(h.entry(A).transcript_path, None);
    let paths = Paths::native(&h.home);
    let found = search_transcript(&paths, A, h.home.join(".claude").to_str().unwrap())
        .expect("found in a project folder");
    assert_eq!(Path::new(&found), h.transcript(A));
    assert_eq!(
        search_transcript(&paths, B, h.home.join(".claude").to_str().unwrap()),
        None
    );
    assert_eq!(
        search_transcript(&paths, "../x", h.home.join(".claude").to_str().unwrap()),
        None
    );
    assert_eq!(
        search_transcript(&paths, "", h.home.join(".claude").to_str().unwrap()),
        None
    );
    let prepared = h.prepare();
    assert_eq!(prepared.session_count, 1);
    // Found, so the ledger now knows it.
    assert_eq!(
        h.entry(A).transcript_path.as_deref(),
        Some(h.transcript(A).to_str().unwrap())
    );
}

// ---- Summaries ----

fn made_up(text: &str) -> Summary {
    Summary {
        text: text.to_owned(),
        model: "haiku".to_owned(),
        cost_usd: Some(0.002),
    }
}

#[test]
fn summaries_ride_along_only_when_asked_and_only_when_new() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    h.run(A, 10.0);
    h.end(10.0);
    let key = h.entry(A).key();
    h.stores
        .summaries
        .record(&key, &made_up("Fixed the parser."), 1, F::at(200.0));

    // Off: the session goes without it.
    let mut off = h.input();
    off.include_summaries = false;
    let plain = CloudSyncPass::prepare(&off, &h.stores);
    assert_eq!(plain.session_count, 1);
    assert_eq!(plain.batches[0].request.sessions[0].summary, None);
    assert_eq!(plain.batches[0].records[&key].summary, None);
    h.stores
        .memory
        .mark_sent(&plain.batches[0].records, F::at(300.0));

    // On: the summary is new, so the session goes again, with it.
    let mut on = h.input();
    on.include_summaries = true;
    let with = CloudSyncPass::prepare(&on, &h.stores);
    assert_eq!(with.session_count, 1);
    let summary = with.batches[0].request.sessions[0]
        .summary
        .clone()
        .expect("with its summary");
    assert_eq!(summary.text, "Fixed the parser.");
    assert_eq!(summary.model, "haiku");
    assert_eq!(summary.generated_at, F::at(200.0));
    let record = &with.batches[0].records[&key];
    assert!(record.summary.is_some());
    // The summary changes nothing in the session's own hash.
    assert_eq!(record.base, plain.batches[0].records[&key].base);
    let stored = h.stores.summaries.summary(&key).unwrap();
    assert!(!CloudSyncPass::was_sent(&stored, &key, &h.stores, &[]));

    // Switched off after the pass was built: the batch goes without it, and
    // is remembered without it.
    let stripped = with.batches[0].without_summaries();
    assert_eq!(stripped.request.sessions[0].summary, None);
    assert_eq!(stripped.records[&key].summary, None);
    assert_eq!(stripped.records[&key].base, record.base);

    h.stores
        .memory
        .mark_sent(&records_of(&with.batches[0]), F::at(400.0));
    assert!(CloudSyncPass::was_sent(&stored, &key, &h.stores, &[]));
    assert!(!CloudSyncPass::has_new_summary(
        &key,
        &h.stores.memory.sent(&key).unwrap(),
        &h.stores,
        true,
        &[]
    ));
    // Sent: nothing more, with or without them.
    assert_eq!(CloudSyncPass::prepare(&on, &h.stores).session_count, 0);
    assert_eq!(CloudSyncPass::prepare(&off, &h.stores).session_count, 0);

    // Summarised again: new, so sent again, only when they are on.
    h.stores
        .summaries
        .record(&key, &made_up("Fixed it, with a test."), 3, F::at(500.0));
    assert_eq!(CloudSyncPass::prepare(&off, &h.stores).session_count, 0);
    let newer = CloudSyncPass::prepare(&on, &h.stores);
    assert_eq!(newer.session_count, 1);
    assert_eq!(
        newer.batches[0].request.sessions[0]
            .summary
            .as_ref()
            .unwrap()
            .text,
        "Fixed it, with a test."
    );
    // The website keeps the old summary until a new one comes: a record
    // without one keeps the hash of the last sent.
    h.stores
        .memory
        .mark_sent(&records_of(&newer.batches[0]), F::at(600.0));
    let mut bare = records_of(&newer.batches[0]);
    bare.get_mut(&key).unwrap().summary = None;
    let before = h.stores.memory.sent(&key).unwrap().summary;
    h.stores.memory.mark_sent(&bare, F::at(700.0));
    assert_eq!(h.stores.memory.sent(&key).unwrap().summary, before);
}

#[test]
fn a_summary_is_scrubbed_of_this_pcs_names_as_it_is_built_and_hashed() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    h.run(A, 10.0);
    let key = h.entry(A).key();
    h.stores.summaries.record(
        &key,
        &made_up("Edited /Users/Some Person/code/app/main.rs today."),
        1,
        F::at(200.0),
    );
    let mut input = h.input();
    input.include_summaries = true;
    input.known_names = vec!["Some Person".to_owned()];
    let prepared = CloudSyncPass::prepare(&input, &h.stores);
    let text = &prepared.batches[0].request.sessions[0]
        .summary
        .as_ref()
        .unwrap()
        .text;
    assert!(
        !text.contains("Person") && !text.contains("/Users"),
        "{text}"
    );
    let stored = h.stores.summaries.summary(&key).unwrap();
    // The hash is of what is sent, so a names-aware pass agrees with itself.
    assert_eq!(
        prepared.batches[0].records[&key].summary,
        CloudSyncPass::summary_hash(&stored, &input.known_names)
    );
    assert_eq!(*text, stored.contract(&input.known_names).text);
}

#[test]
fn summary_candidates_are_ended_sessions_of_allowed_accounts_since_the_switch() {
    let h = Harness::new();
    h.write(A, &responses(A, "claude-opus-4-5", 1000, 500, 10.0));
    h.write(B, &responses(B, "claude-opus-4-5", 1000, 500, 10.0));
    h.run(A, 10.0);
    h.run(B, 10.0);
    h.end(10.0);
    // Read once, as a pass reads them.
    h.prepare();
    let both = infos();
    assert!(CloudSyncPass::summary_candidates(&both, &h.stores, None).is_empty());
    let all = CloudSyncPass::summary_candidates(&both, &h.stores, Some(F::at(0.0)));
    assert_eq!(all.len(), 2);
    let a = all.iter().find(|c| c.session_id == A).expect("A");
    assert_eq!(a.key, h.entry(A).key());
    assert_eq!(a.identity_id.as_str(), F::IDENTITY_ID);
    assert_eq!(a.account_key, F::ACCOUNT_KEY);
    assert_eq!(a.transcript_path, h.transcript(A).to_str().unwrap());
    assert_eq!(a.message_count, 1);
    assert_eq!(a.ended_at, h.entry(A).ended_at.unwrap());
    // Ended before summaries were turned on: never.
    assert!(CloudSyncPass::summary_candidates(&both, &h.stores, Some(F::at(5_000.0))).is_empty());
    // An account the caller left out (its 5-hour window is nearly used up).
    let only_work = environment::account_infos(&[F::work_account()]);
    assert!(CloudSyncPass::summary_candidates(&only_work, &h.stores, Some(F::at(0.0))).is_empty());
}

// ---- The memory ----

#[test]
fn the_memory_forgets_what_was_sent_to_another_user_or_website() {
    let memory = CloudSyncMemory::new(agentnotch_engine::cloud::files::StateFile::memory());
    let mut records = BTreeMap::new();
    records.insert("k".to_owned(), Record::new("one"));
    memory.adopt(
        Some("user-1"),
        "https://a.example.com",
        Some("https://a.example.com/d"),
    );
    memory.mark_sent(&records, F::at(1.0));
    assert_eq!(memory.sent_count(), 1);
    assert_eq!(memory.last_sync_at(), Some(F::at(1.0)));
    // The same user and website: kept, and the dashboard may change.
    memory.adopt(
        Some("user-1"),
        "https://a.example.com",
        Some("https://a.example.com/e"),
    );
    assert_eq!(memory.sent_count(), 1);
    assert_eq!(
        memory.dashboard_url().as_deref(),
        Some("https://a.example.com/e")
    );
    memory.adopt(Some("user-1"), "https://a.example.com", None);
    assert_eq!(
        memory.dashboard_url().as_deref(),
        Some("https://a.example.com/e")
    );
    // Another user: starts over.
    memory.adopt(Some("user-2"), "https://a.example.com", None);
    assert_eq!(memory.sent_count(), 0);
    assert_eq!(memory.last_sync_at(), None);
    assert_eq!(memory.dashboard_url(), None);
    assert_eq!(memory.user_id().as_deref(), Some("user-2"));
    // Another website: starts over too.
    memory.mark_sent(&records, F::at(2.0));
    memory.adopt(Some("user-2"), "https://b.example.com", None);
    assert_eq!(memory.sent_count(), 0);
}

#[test]
fn the_memory_says_what_needs_sending() {
    let memory = CloudSyncMemory::new(agentnotch_engine::cloud::files::StateFile::memory());
    let sent = Record {
        base: "base".to_owned(),
        summary: Some("s1".to_owned()),
        ended: false,
        transcript: None,
        version: Some(1),
    };
    // Never sent.
    assert!(memory.needs_sending("k", &sent));
    memory.mark_sent(
        &BTreeMap::from([("k".to_owned(), sent.clone())]),
        F::at(1.0),
    );
    assert!(!memory.needs_sending("k", &sent));
    // The session changed.
    assert!(memory.needs_sending("k", &Record::new("other")));
    // No summary in the new record: the website keeps its own.
    assert!(!memory.needs_sending("k", &Record::new("base")));
    // A different summary.
    let newer = Record {
        summary: Some("s2".to_owned()),
        ..sent.clone()
    };
    assert!(memory.needs_sending("k", &newer));
    // Built again the same way from a moved transcript: remembered, not sent.
    let stamp = TranscriptStamp {
        bytes: 5,
        modified: 12.5,
    };
    let rebuilt = Record {
        transcript: Some(stamp),
        version: Some(2),
        ..Record::new("base")
    };
    assert!(!memory.needs_sending("k", &rebuilt));
    memory.note_unchanged("k", &rebuilt);
    let kept = memory.sent("k").unwrap();
    assert_eq!(kept.transcript, Some(stamp));
    assert_eq!(kept.version, Some(2));
    assert_eq!(kept.summary.as_deref(), Some("s1"));
    // A session never sent is not invented by it.
    memory.note_unchanged("never", &rebuilt);
    assert_eq!(memory.sent("never"), None);
    memory.note_synced(F::at(9.0));
    assert_eq!(memory.last_sync_at(), Some(F::at(9.0)));
}

#[test]
fn a_transcript_stamp_tells_a_moved_file_from_a_quiet_one() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("t.jsonl");
    std::fs::write(&file, "one\n").unwrap();
    let path = file.to_str().unwrap();
    let stamp = TranscriptStamp::of(&StdSecureFiles, path).expect("a stamp");
    assert_eq!(stamp.bytes, 4);
    assert!(TranscriptStamp::of(&StdSecureFiles, path)
        .unwrap()
        .matches(&stamp));
    // A saved double may come back a step off: still the same file.
    assert!(TranscriptStamp {
        modified: stamp.modified + 1e-7,
        ..stamp
    }
    .matches(&stamp));
    assert!(!TranscriptStamp {
        modified: stamp.modified + 0.001,
        ..stamp
    }
    .matches(&stamp));
    assert!(!TranscriptStamp { bytes: 5, ..stamp }.matches(&stamp));
    std::fs::write(&file, "one\ntwo\n").unwrap();
    assert!(!TranscriptStamp::of(&StdSecureFiles, path)
        .unwrap()
        .matches(&stamp));
    assert!(
        TranscriptStamp::of(&StdSecureFiles, root.path().join("gone").to_str().unwrap()).is_none()
    );
}

// ---- The stores ----

#[test]
fn the_install_secret_is_made_when_a_pass_needs_a_project_key_and_not_before() {
    let h = Harness::with(true);
    let file = h.support.join("cloud-install-secret");
    // A pass with nothing to send never needs it.
    let empty = h.prepare();
    assert_eq!(empty.session_count, 0);
    assert!(!file.exists());
    h.write(A, &responses(A, "claude-opus-4-5", 10, 10, 10.0));
    h.run(A, 10.0);
    let prepared = h.prepare();
    assert_eq!(prepared.session_count, 1);
    let secret = std::fs::read(&file).expect("made by the pass");
    assert_eq!(secret.len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(h.stores.secret(), secret);
    // Another run reads the same one: project keys stay stable.
    let again = CloudStores::new(&h.support, Arc::new(StdSecureFiles), true, &h.home);
    assert_eq!(again.secret(), secret);
}

#[test]
fn a_run_that_keeps_nothing_makes_its_own_secret_and_writes_nothing() {
    let h = Harness::new();
    let secret = h.stores.secret();
    assert_eq!(secret.len(), 32);
    assert_eq!(h.stores.secret(), secret);
    h.stores.save_now();
    h.stores.flush();
    assert!(!h.support.exists(), "nothing was written");
    let other = Harness::new();
    assert_ne!(other.stores.secret(), secret);
}

#[test]
fn the_stores_write_their_files_when_asked_and_read_them_back() {
    let h = Harness::with(true);
    h.write(A, &responses(A, "claude-opus-4-5", 10, 10, 10.0));
    h.run(A, 10.0);
    let prepared = h.prepare();
    h.stores
        .memory
        .adopt(Some("user-1"), "https://agentnotch.example.com", None);
    h.stores
        .memory
        .mark_sent(&prepared.batches[0].records, F::at(100.0));
    h.stores.save_now();
    h.stores.flush();
    for name in [
        "cloud-ledger.json",
        "cloud-scan-state.json",
        "cloud-sync-state.json",
        "cloud-install-secret",
    ] {
        assert!(h.support.join(name).exists(), "{name}");
    }
    let reread = CloudStores::new(&h.support, Arc::new(StdSecureFiles), true, &h.home);
    assert_eq!(reread.memory.contents(), h.stores.memory.contents());
    assert_eq!(reread.ledger.entry(A), h.stores.ledger.entry(A));
    assert_eq!(reread.memory.user_id().as_deref(), Some("user-1"));
    // The reread stores prepare nothing: what was sent is remembered.
    let input = h.input();
    assert_eq!(CloudSyncPass::prepare(&input, &reread).session_count, 0);
}

// ---- The engine's mappings (CloudLiveEnvironment) ----

fn identity(uuid: &str, email: &str, organization: Option<&str>) -> Identity {
    Identity {
        account_uuid: Some(uuid.to_owned()),
        email: Some(email.to_owned()),
        organization_uuid: organization.map(str::to_owned),
        organization_name: organization.map(|_| "Company".to_owned()),
        ..Identity::default()
    }
}

fn run_folder(dir: &str, kind: FolderKind, identity: Option<Identity>) -> RunFolder {
    RunFolder {
        id: AccountId::from(dir),
        config_dir: PathBuf::from(dir),
        config_dir_env: Some(dir.to_owned()),
        custom_label: None,
        seen_config_dir_envs: Vec::new(),
        identity,
        subscription_type: None,
        color_index: 0,
        source: FolderSource::Discovered,
        last_seen_at: None,
        is_hidden: false,
        kind,
    }
}

fn account_of(id: &str, label: &str, run: &[&RunFolder], store: &[&RunFolder]) -> Account {
    Account {
        identity_id: IdentityId::from(id),
        ring_id: RingId::from("claude-acct-000000000000"),
        label: label.to_owned(),
        own_label: None,
        monogram: "M".to_owned(),
        color_index: 0,
        email: Some("me@example.com".to_owned()),
        plan_name: Some("Max 20x".to_owned()),
        organization_uuid: None,
        run_dirs: run.iter().map(|f| f.id.clone()).collect(),
        store_dirs: store.iter().map(|f| f.id.clone()).collect(),
        includes_default: false,
        is_tracked: true,
        ring_shown: true,
        is_signed_in: true,
        launch_command: None,
        can_forget: true,
    }
}

#[test]
fn the_accounts_the_website_may_hear_of_are_signed_in_tracked_ones_with_an_account_uuid() {
    let own = run_folder(
        "/Users/me/.claude-own",
        FolderKind::Run,
        Some(identity(F::ACCOUNT_UUID, "me@example.com", None)),
    );
    let work = run_folder(
        "/Users/me/.claude-work",
        FolderKind::Run,
        Some(identity(
            F::WORK_UUID,
            "me@company.com",
            Some(F::WORK_ORGANIZATION),
        )),
    );
    let mirrored = run_folder(
        "/Users/me/.claude-mirror",
        FolderKind::Run,
        Some(identity(
            F::ACCOUNT_UUID,
            "me@example.com",
            Some("stale-org"),
        )),
    );
    let by_email = run_folder("/Users/me/.claude-email", FolderKind::Run, None);
    let hidden = run_folder(
        "/Users/me/.claude-hidden",
        FolderKind::Run,
        Some(identity(
            "77777777-0000-4000-8000-000000000000",
            "h@example.com",
            None,
        )),
    );
    let signed_out = run_folder("/Users/me/.claude-out", FolderKind::Run, None);
    let mut accounts = vec![
        account_of(F::IDENTITY_ID, "Personal", &[&own, &mirrored], &[]),
        account_of(F::WORK_IDENTITY_ID, "Work", &[&work], &[]),
        account_of("email:me@example.com", "Email only", &[&by_email], &[]),
        account_of(
            "uuid:77777777-0000-4000-8000-000000000000",
            "Hidden",
            &[&hidden],
            &[],
        ),
        account_of(
            "dir:/Users/me/.claude-out",
            "Signed out",
            &[&signed_out],
            &[],
        ),
    ];
    accounts[3].is_tracked = false;
    accounts[4].is_signed_in = false;
    let folders = [
        own.clone(),
        work,
        mirrored.clone(),
        by_email,
        hidden,
        signed_out,
    ];
    let corrected = BTreeSet::from([mirrored.id.clone()]);
    let paths = Paths::new(PathStyle::Posix, "/Users/me");
    let engine = environment::cloud_accounts(&accounts, &folders, &corrected, true, &paths);
    assert_eq!(
        engine
            .iter()
            .map(|a| a.identity_id.as_str())
            .collect::<Vec<_>>(),
        [F::IDENTITY_ID, F::WORK_IDENTITY_ID, "email:me@example.com"]
    );
    let personal = &engine[0];
    assert_eq!(personal.label.as_deref(), Some("Personal"));
    assert_eq!(personal.plan.as_deref(), Some("Max 20x"));
    assert_eq!(personal.folders.len(), 2);
    assert!(!personal.folders[0].corrected);
    assert!(personal.folders[1].corrected);
    assert_eq!(
        personal.folders[1].organization_uuid.as_deref(),
        Some("stale-org")
    );
    // The mirrored copy's organization never makes the key.
    let infos = environment::account_infos(&engine);
    assert_eq!(
        infos
            .iter()
            .map(|a| a.account_key.as_str())
            .collect::<Vec<_>>(),
        [F::ACCOUNT_KEY, F::WORK_ACCOUNT_KEY]
    );
    assert_eq!(infos[0].account_uuid, F::ACCOUNT_UUID);
    assert_eq!(infos[0].identity_id.as_str(), F::IDENTITY_ID);
    assert_eq!(infos[0].email.as_deref(), Some("me@example.com"));
    assert_eq!(infos[0].label.as_deref(), Some("Personal"));
    assert_eq!(infos[1].organization_name.as_deref(), Some("Company"));
    // What the website is told of an account.
    assert_eq!(
        infos[1].contract(),
        SyncAccount {
            key: F::WORK_ACCOUNT_KEY.to_owned(),
            email: Some("me@example.com".to_owned()),
            organization_name: Some("Company".to_owned()),
            plan: Some("Max 20x".to_owned()),
            label: Some("Work".to_owned()),
        }
    );
    assert_eq!(
        infos[1].ledger_account().identity_id.as_str(),
        F::WORK_IDENTITY_ID
    );
}

#[test]
fn an_account_split_by_organization_keeps_its_organization() {
    let id = format!("uuid:{}/{}", F::WORK_UUID, F::WORK_ORGANIZATION);
    let folder = run_folder(
        "/Users/me/.claude-work",
        FolderKind::Run,
        Some(identity(
            F::WORK_UUID,
            "me@company.com",
            Some("someone-else"),
        )),
    );
    let accounts = [account_of(&id, "Work", &[&folder], &[])];
    let paths = Paths::new(PathStyle::Posix, "/Users/me");
    let engine = environment::cloud_accounts(&accounts, &[folder], &BTreeSet::new(), false, &paths);
    let infos = environment::account_infos(&engine);
    assert_eq!(infos[0].account_key, F::WORK_ACCOUNT_KEY);
    assert_eq!(infos[0].account_uuid, F::WORK_UUID);
}

#[test]
fn folder_logins_are_a_digest_of_who_is_signed_in_to_each_folder() {
    let own = run_folder(
        "/Users/me/.claude-own",
        FolderKind::Run,
        Some(identity(F::ACCOUNT_UUID, "me@example.com", None)),
    );
    let empty = run_folder("/Users/me/.claude-empty", FolderKind::Run, None);
    let nobody = run_folder(
        "/Users/me/.claude-nobody",
        FolderKind::Run,
        Some(Identity {
            organization_uuid: Some("o".to_owned()),
            ..Identity::default()
        }),
    );
    let logins = environment::folder_logins(&[own.clone(), empty, nobody]);
    assert_eq!(
        logins.keys().map(String::as_str).collect::<Vec<_>>(),
        ["/Users/me/.claude-own"]
    );
    assert_eq!(
        logins["/Users/me/.claude-own"],
        backfill::login(Some(F::ACCOUNT_UUID), None, Some("me@example.com")).unwrap()
    );
    // The login is who the folder's own login names (case and spaces don't
    // matter; another organization or email does).
    let a = backfill::login(Some("U"), Some("O"), Some("me@example.com"));
    assert_eq!(
        a,
        backfill::login(Some("u "), Some("o"), Some("ME@example.com"))
    );
    assert_ne!(
        a,
        backfill::login(Some("u"), Some("other"), Some("me@example.com"))
    );
    assert_eq!(backfill::login(None, None, None), None);
    // The digest never holds the email.
    assert!(!logins["/Users/me/.claude-own"].contains("example"));
}

#[test]
fn backfill_folders_are_run_folders_of_their_own_account() {
    let signed_in = |dir: &str, kind: FolderKind, uuid: &str| {
        run_folder(dir, kind, Some(identity(uuid, "me@example.com", None)))
    };
    let own = signed_in("/Users/me/.claude-own", FolderKind::Run, F::ACCOUNT_UUID);
    let mirrored = signed_in("/Users/me/.claude-mirror", FolderKind::Run, "stale");
    let store = signed_in(
        "/Users/me/.claude-store",
        FolderKind::Store,
        F::ACCOUNT_UUID,
    );
    let default = signed_in("/Users/me/.claude", FolderKind::Run, F::ACCOUNT_UUID);
    let folders = [
        own.clone(),
        mirrored.clone(),
        store.clone(),
        default.clone(),
    ];
    let accounts = [account_of(
        F::IDENTITY_ID,
        "Personal",
        &[&own, &mirrored, &default],
        &[&store],
    )];
    let infos = infos();
    let corrected = BTreeSet::from([mirrored.id.clone()]);
    let paths = Paths::new(PathStyle::Posix, "/Users/me");
    let result = environment::backfill_folders(
        &folders,
        &infos,
        &environment::identity_of_folder(&accounts),
        true,
        &corrected,
        &["/Users/me/.claude-shared".to_owned()],
        &paths,
    );
    // Only the account's own run folder: not a corrected copy, not a
    // store, and not `~/.claude` while accounts are mirrored into it.
    let owned: Vec<&str> = result
        .iter()
        .filter(|f| f.account_key.is_some())
        .map(|f| f.config_dir.as_str())
        .collect();
    assert_eq!(owned, ["/Users/me/.claude-own"]);
    assert_eq!(
        result[0].identity_id.as_ref().unwrap().as_str(),
        F::IDENTITY_ID
    );
    assert_eq!(result[0].account_key.as_deref(), Some(F::ACCOUNT_KEY));
    assert_eq!(result.len(), 5);
    let shared = result.last().unwrap();
    assert_eq!(shared.config_dir, "/Users/me/.claude-shared");
    assert!(shared.identity_id.is_none() && shared.account_key.is_none());
    // Nothing mirrors into `~/.claude` now: it is its account's.
    let plain = environment::backfill_folders(
        &folders,
        &infos,
        &environment::identity_of_folder(&accounts),
        false,
        &corrected,
        &[],
        &paths,
    );
    let owned: Vec<&str> = plain
        .iter()
        .filter(|f| f.account_key.is_some())
        .map(|f| f.config_dir.as_str())
        .collect();
    assert_eq!(owned, ["/Users/me/.claude-own", "/Users/me/.claude"]);
    // An account the website may not hear of owns nothing.
    let nobody = environment::backfill_folders(
        &folders,
        &[],
        &environment::identity_of_folder(&accounts),
        false,
        &BTreeSet::new(),
        &[],
        &paths,
    );
    assert!(nobody.iter().all(|f| f.account_key.is_none()));
}

#[test]
fn the_default_folder_is_found_by_windows_rules_too() {
    let identity_ = identity(F::ACCOUNT_UUID, "me@example.com", None);
    let default = run_folder(
        r"C:\Users\Me\.claude",
        FolderKind::Run,
        Some(identity_.clone()),
    );
    let own = run_folder(r"C:\Users\Me\.claude-own", FolderKind::Run, Some(identity_));
    let accounts = [account_of(
        F::IDENTITY_ID,
        "Personal",
        &[&default, &own],
        &[],
    )];
    let paths = Paths::new(PathStyle::Windows, r"c:\users\me");
    let folders = environment::backfill_folders(
        &[default.clone(), own.clone()],
        &infos(),
        &environment::identity_of_folder(&accounts),
        true,
        &BTreeSet::new(),
        &[],
        &paths,
    );
    assert_eq!(folders[0].account_key, None);
    assert_eq!(folders[1].account_key.as_deref(), Some(F::ACCOUNT_KEY));
    let engine =
        environment::cloud_accounts(&accounts, &[default, own], &BTreeSet::new(), true, &paths);
    assert!(engine[0].folders[0].is_default && engine[0].folders[0].mirrored_default);
    assert!(!engine[0].folders[1].is_default && !engine[0].folders[1].mirrored_default);
}
