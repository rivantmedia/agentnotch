//! Regressions for the cloud review's findings (the Mac's
//! `CloudSyncRegressionTests`), over the same stand-in website and engine
//! as `cloud_sync.rs`. Each test names its finding. (The rest of that
//! file, the findings about what is sent while a request is out, is
//! `cloud_sync_binding.rs`.)

mod cloud_support;

use agentnotch_engine::cloud::contract::{
    date, SessionSource, SyncAccount, SyncDevice, SyncProject, SyncRequest, SyncSession,
    SyncSummary, SyncTokens,
};
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::ledger::CloudLedgerEntry;
use agentnotch_engine::cloud::pass::{Batch, CloudSyncPass};
use agentnotch_engine::cloud::service::CloudSync;
use agentnotch_engine::model::{BackfillFolder, IdentityId};
use agentnotch_engine::platform::{HttpResponse, Platform};
use agentnotch_engine::runtime_types::UsageObservation;
use agentnotch_engine::testkit::http::{json_response, path_of};
use cloud_support::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const A: &str = CloudFixture::SESSION_A;
const B: &str = CloudFixture::SESSION_B;
const C: &str = CloudFixture::SESSION_C;

fn key_a() -> String {
    CloudLedgerEntry::key_of(A, CloudFixture::ACCOUNT_KEY)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Every session of every sync request, in order.
fn all_sessions(h: &Harness) -> Vec<Value> {
    h.sync_requests()
        .iter()
        .flat_map(Harness::sessions)
        .collect()
}

fn session_ids(sessions: &[Value]) -> Vec<String> {
    sessions
        .iter()
        .filter_map(|s| s["sessionId"].as_str().map(str::to_owned))
        .collect()
}

fn summaries_on() -> Harness {
    let h = Harness::with(HarnessOptions {
        summaries_on: true,
        ..HarnessOptions::default()
    });
    h.start();
    h
}

fn started() -> Harness {
    let h = Harness::new();
    h.start();
    h
}

/// The own folder the backfill tests read history from.
fn own_folder(h: &Harness) -> String {
    h.handles
        .roots
        .home
        .join(".claude-own")
        .to_string_lossy()
        .into_owned()
}

fn own_backfill(own: &str) -> BackfillFolder {
    BackfillFolder {
        config_dir: own.to_owned(),
        identity_id: Some(IdentityId::from(CloudFixture::IDENTITY_ID)),
        account_key: Some(CloudFixture::ACCOUNT_KEY.to_owned()),
        signed_in_since: None,
    }
}

// ---- Finding 0: backfill never guesses ----

#[test]
fn backfill_reads_only_what_began_after_the_folder_was_signed_in_as_its_account() {
    let mut h = started();
    let own = own_folder(&h);
    let slug = std::path::Path::new(&own)
        .join("projects")
        .join("-Users-me-work-billing");
    let billing = "/Users/me/work/billing";
    let write = |id: &str, at: f64| {
        Lines::write(
            &[
                Lines::user_in("prompt", id, at, billing, "cli"),
                Lines::assistant(&format!("{id}-m"), &format!("{id}-r"), id)
                    .usage(1, 1)
                    .at(at + 5.0)
                    .cwd(billing)
                    .line(),
            ],
            &slug.join(format!("{id}.jsonl")),
            false,
        );
    };
    // Someone was signed in here before: their session. Then ours.
    write(A, 0.0);
    write(B, 7200.0);
    *h.deps.backfill_folders.lock().unwrap() = vec![own_backfill(&own)];

    // The login there was never seen: nothing is backfilled.
    h.sync_now();
    assert!(all_sessions(&h).is_empty());
    assert_eq!(h.ledger_count(), 0);

    // First seen signed in as the account an hour in: only what began after.
    *h.deps.folder_logins.lock().unwrap() =
        Some(BTreeMap::from([(own.clone(), "login-now".to_owned())]));
    h.relaunch();
    h.tick();
    assert_eq!(session_ids(&all_sessions(&h)), [B]);
    assert!(!h.service.stores().unwrap().ledger.knows(A));
}

// ---- Findings 4 and 25: summaries of new sessions only, within limits ----

#[test]
fn summaries_skip_what_ended_before_they_were_turned_on() {
    let h = started();
    h.answer_summary(SUMMARY_ANSWER);
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(61));
    h.tick();
    assert!(h
        .service
        .stores()
        .unwrap()
        .ledger
        .entry(A)
        .unwrap()
        .ended_at
        .is_some());

    // Turned on after it ended: history is never summarised.
    h.advance(secs(60));
    h.service.set_summaries(true, h.now());
    h.advance(secs(24 * 3600));
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());

    // A session that ends from now on is.
    h.write_session(C, 0);
    h.observe_one(h.observation(C));
    h.sync_now();
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(61));
    h.tick();
    h.advance(secs(11 * 60));
    h.summarize_next();
    assert_eq!(h.handles.runner.spawned().len(), 1);
    let stores = h.service.stores().unwrap();
    assert!(stores
        .summaries
        .summary(&CloudLedgerEntry::key_of(C, CloudFixture::ACCOUNT_KEY))
        .is_some());
    assert!(stores.summaries.summary(&key_a()).is_none());
}

#[test]
fn an_account_near_its_five_hour_limit_gets_no_summaries() {
    let h = summaries_on();
    ended_session(&h);
    h.answer_summary(SUMMARY_ANSWER);
    h.advance(secs(11 * 60));
    let identity = IdentityId::from(CloudFixture::IDENTITY_ID);
    h.deps
        .five_hour
        .lock()
        .unwrap()
        .insert(identity.clone(), 80.0);
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());
    h.deps.five_hour.lock().unwrap().insert(identity, 79.5);
    h.summarize_next();
    assert_eq!(h.handles.runner.spawned().len(), 1);
}

// ---- Findings 5 and 8: a session is read again until its end is sent ----

#[test]
fn a_session_sent_while_running_is_sent_again_when_it_ends() {
    let h = started();
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    assert_eq!(all_sessions(&h)[0]["endedAt"], Value::Null);

    // It went away just before the lid closed; the next look is 8 hours on.
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(8 * 3600));
    h.tick();
    h.tick();
    assert_eq!(h.sync_requests().len(), 2);
    let last = all_sessions(&h).last().cloned().unwrap();
    assert_eq!(
        last["endedAt"],
        json!(date::to_string(h.now() - secs(8 * 3600)))
    );
    assert_eq!(last["messageCount"], json!(2));

    // Sent as ended, transcript unchanged: not read or sent again.
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 2);
    // It grew afterwards (a late write): sent again.
    h.write_session(A, 1);
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 3);
    assert_eq!(all_sessions(&h).last().unwrap()["messageCount"], json!(3));
}

#[test]
fn a_session_never_sent_is_read_whatever_its_age() {
    let h = started();
    h.write_session(A, 0);
    // Captured, then gone, and no pass ran before it was long over.
    h.observe_one(h.observation(A));
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(61));
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(3 * 24 * 3600));
    h.sync_now();
    let sessions = all_sessions(&h);
    let session = sessions.first().expect("a session was sent");
    assert_eq!(session["sessionId"], json!(A));
    assert_eq!(session["messageCount"], json!(2));
    assert!(session["endedAt"].is_string());
}

// ---- Finding 6: the key is the account's own ----

#[test]
fn live_sessions_are_keyed_as_their_account_is() {
    let h = Harness::new();
    *h.deps.accounts.lock().unwrap() = vec![CloudFixture::work_account()];
    h.start();
    h.write_session(A, 0);
    // The hub's key for the identity without its organization: re-keyed
    // from the account (its own UUID and organization).
    let mut observation = h.observation_for(A, &CloudFixture::work_account());
    observation.account_key = keys::account_key(CloudFixture::WORK_UUID, None);
    h.observe_one(observation);
    h.sync_now();
    let sessions = all_sessions(&h);
    assert_eq!(
        sessions.first().expect("a session")["accountKey"],
        json!(CloudFixture::WORK_ACCOUNT_KEY)
    );
    let request = agentnotch_engine::testkit::http::body_json(&h.sync_requests()[0]);
    let accounts: Vec<&Value> = request["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["key"])
        .collect();
    assert_eq!(accounts, [&json!(CloudFixture::WORK_ACCOUNT_KEY)]);
}

// ---- Finding 7: a session resumed under another account ----

#[test]
fn a_resumed_session_is_sent_once_per_account_and_summarised_as_its() {
    let h = Harness::new();
    let personal = CloudFixture::account();
    let work = CloudFixture::work_account();
    *h.deps.accounts.lock().unwrap() = vec![personal.clone(), work.clone()];
    h.start();
    h.answer_summary(SUMMARY_ANSWER);
    let id = A;
    Lines::write(
        &[
            Lines::user("START WITH PERSONAL", id, 0.0),
            Lines::assistant("m1", "r1", id)
                .usage(100, 10)
                .at(10.0)
                .text("Personal reply.")
                .line(),
            Lines::user("SECOND ACCOUNT PROMPT", id, 1000.0),
            Lines::assistant("m2", "r2", id)
                .model("claude-sonnet-4-5")
                .usage(7, 3)
                .at(1010.0)
                .text("Work reply.")
                .line(),
            Lines::assistant("m3", "r3", id)
                .model("claude-sonnet-4-5")
                .usage(3, 1)
                .at(1020.0)
                .text("Done.")
                .line(),
        ],
        &h.transcript(id),
        false,
    );
    h.observe_one(h.observation_for(id, &personal));
    let mut resumed = h.observation_for(id, &work);
    resumed.process_started_at = Some(CloudFixture::at(900.0));
    resumed.last_activity_at = CloudFixture::at(1010.0);
    h.observe_one(resumed);
    h.sync_now();

    let sessions = all_sessions(&h);
    let by_account: BTreeMap<String, &Value> = sessions
        .iter()
        .map(|s| (s["accountKey"].as_str().unwrap_or("").to_owned(), s))
        .collect();
    assert_eq!(by_account.len(), 2);
    let mine = by_account[&key_of(&personal)];
    let theirs = by_account[&key_of(&work)];
    assert_eq!(mine["sessionId"], json!(id));
    assert_eq!(theirs["sessionId"], json!(id));
    assert_eq!(mine["messageCount"], json!(1));
    assert_eq!(mine["tokens"]["input"], json!(100));
    assert_eq!(theirs["messageCount"], json!(2));
    assert_eq!(theirs["tokens"]["input"], json!(10));
    assert_eq!(
        mine["endedAt"],
        json!(date::to_string(CloudFixture::at(900.0)))
    );
    assert_eq!(theirs["endedAt"], Value::Null);
    assert_eq!(
        theirs["startedAt"],
        json!(date::to_string(CloudFixture::at(1000.0)))
    );
    assert_eq!(theirs["models"], json!(["claude-sonnet-4-5"]));

    // The work part's summary runs as the work account, from its own part.
    h.service.set_summaries(true, h.now());
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(61));
    h.tick();
    h.advance(secs(11 * 60));
    h.summarize_next();
    assert_eq!(h.handles.runner.spawned().len(), 1);
    assert_eq!(
        *h.deps.folder_requests.lock().unwrap(),
        std::slice::from_ref(&work.identity_id)
    );
    let input = String::from_utf8(h.handles.runner.stdin_of(0).unwrap()).unwrap();
    assert!(input.contains("SECOND ACCOUNT PROMPT") && input.contains("Work reply."));
    assert!(!input.contains("START WITH PERSONAL") && !input.contains("Personal reply."));
}

// ---- Findings 9 and 12: turning things off stops them at once ----

/// Holds the first sync request until the test lets it go. The stand-in
/// network runs the handler on the sending thread, so the thread waits.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    /// Waits (at most 30 s: a test never hangs) for [`open`](Self::open).
    fn wait(&self) {
        let guard = self.open.lock().unwrap_or_else(|p| p.into_inner());
        let _ = self
            .changed
            .wait_timeout_while(guard, secs(30), |open| !*open);
    }

    fn open(&self) {
        *self.open.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.changed.notify_all();
    }
}

#[test]
fn turning_sync_off_stops_a_pass_in_flight() {
    let h = started();
    // 600 readings: two requests.
    for index in 0..600u32 {
        h.service.record_usage(UsageObservation {
            identity: IdentityId::from(CloudFixture::IDENTITY_ID),
            source: agentnotch_engine::model::UsageSource::Probe,
            observed_at: CloudFixture::at(f64::from(index)),
            windows: vec![("session".into(), f64::from(index % 100) + 0.5, None)],
        });
    }
    assert_eq!(h.pending_usage(), 600);
    let gate = Arc::new(Gate::default());
    let first = Arc::new(FirstOnly::default());
    let (held, once) = (gate.clone(), first.clone());
    h.handles.http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/sync" && once.take() {
            held.wait();
        }
        Ok(website_answer(request))
    });
    let (service, now) = (h.service.clone(), h.now());
    let pass = std::thread::spawn(move || service.sync_now(now, true));
    assert!(
        eventually(secs(30), || h.sync_requests().len() == 1),
        "the first request went out"
    );
    h.service.set_sync(false, h.now());
    gate.open();
    pass.join().expect("the pass ended");
    assert_eq!(h.sync_requests().len(), 1);
}

#[test]
fn summaries_turned_off_during_a_pass_are_left_out() {
    let session = SyncSession {
        account_key: CloudFixture::ACCOUNT_KEY.to_owned(),
        session_id: A.to_owned(),
        project: SyncProject {
            key: CloudFixture::ACCOUNT_KEY.to_owned(),
            name: "p".to_owned(),
        },
        title: None,
        source: SessionSource::Cli,
        models: Vec::new(),
        started_at: CloudFixture::base(),
        last_activity_at: CloudFixture::base(),
        ended_at: None,
        message_count: 1,
        tokens: SyncTokens::default(),
        cost_usd: None,
        summary: Some(SyncSummary {
            text: "Did it.".to_owned(),
            model: "m".to_owned(),
            generated_at: CloudFixture::base(),
        }),
    };
    let record = CloudSyncPass::record(&session, None);
    let batch = Batch {
        request: SyncRequest {
            schema_version: 1,
            device: SyncDevice {
                id: "5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d".into(),
                name: "PC".into(),
                app_version: "1".into(),
            },
            accounts: Vec::<SyncAccount>::new(),
            sessions: vec![session],
            usage: Vec::new(),
        },
        records: BTreeMap::from([(key_a(), record.clone())]),
        readings: Vec::new(),
    };
    assert!(record.summary.is_some());
    let stripped = batch.without_summaries();
    assert!(stripped
        .request
        .sessions
        .iter()
        .all(|s| s.summary.is_none()));
    assert!(stripped.records[&key_a()].summary.is_none());
    assert_eq!(stripped.records[&key_a()].base, record.base);
}

#[test]
fn turning_summaries_off_stops_a_running_summary() {
    let h = summaries_on();
    ended_session(&h);
    h.advance(secs(11 * 60));
    let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
    let hanging = Arc::new(HangingRunner::new(started_tx));
    // The same files, and a runner whose `claude` never ends by itself.
    h.service.stop(h.now());
    let service = Arc::new(CloudSync::new(
        h.config(),
        h.deps.clone(),
        &Platform {
            runner: hanging.clone(),
            ..h.platform.clone()
        },
    ));
    service.start(h.now());
    service.tick(h.now());
    started_rx
        .recv_timeout(secs(10))
        .expect("the summary started");
    assert!(service.is_summarizing());
    service.set_summaries(false, h.now());
    assert!(
        eventually(secs(10), || !service.is_summarizing()),
        "the summary was stopped"
    );
    assert!(hanging.killed());
    let summaries = &service.stores().unwrap().summaries;
    assert!(summaries.summary(&key_a()).is_none());
    assert!(summaries.attempt(&key_a()).is_none());
}

// ---- Finding 22: a paused capture never dates an end ----

#[test]
fn turning_sync_back_on_never_dates_an_end_by_it() {
    let h = started();
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    h.service.set_sync(false, h.now());
    // It ends while sync is off; two days later sync is back on.
    h.advance(secs(2 * 24 * 3600));
    h.service.set_sync(true, h.now());
    h.tick();
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(61));
    h.tick();
    h.advance(secs(31));
    h.tick();
    let last = all_sessions(&h)
        .last()
        .cloned()
        .expect("a session was sent");
    // Its transcript's last line, not the day sync came back.
    assert_eq!(last["endedAt"], json!(CloudFixture::stamp(20.0)));
    assert_eq!(
        h.service
            .stores()
            .unwrap()
            .ledger
            .entry(A)
            .unwrap()
            .ended_at,
        Some(CloudFixture::at(20.0))
    );
}

// ---- Finding 23: nothing but "Sync now" comes before a backoff ends ----

fn sync_unavailable(request: &agentnotch_engine::platform::HttpRequest) -> HttpResponse {
    if path_of(request) == "/api/app/v1/sync" {
        let mut response = json_response(503, r#"{"error":{"code":"INTERNAL","message":"Down."}}"#);
        response
            .headers
            .push(("Retry-After".to_owned(), "600".to_owned()));
        return response;
    }
    website_answer(request)
}

#[test]
fn a_session_ending_during_a_backoff_does_not_bring_the_retry_forward() {
    let h = started();
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.handles
        .http
        .set_handler(|request| Ok(sync_unavailable(request)));
    h.tick();
    assert_eq!(h.sync_requests().len(), 1);
    // The session ends a minute later: a sync would be due in 30 s.
    h.observe(Vec::new(), &[], &[]);
    h.advance(secs(61));
    h.tick();
    assert!(h
        .service
        .stores()
        .unwrap()
        .ledger
        .entry(A)
        .unwrap()
        .ended_at
        .is_some());
    for _ in 0..10 {
        h.advance(secs(50));
        h.tick();
    }
    assert_eq!(h.sync_requests().len(), 1);
    h.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));
    h.advance(secs(50));
    h.tick();
    assert_eq!(h.sync_requests().len(), 2);
}

// ---- Finding 24: originals first ----

#[test]
fn backfill_reads_the_original_before_a_copy_of_it() {
    let h = started();
    let own = own_folder(&h);
    let slug = std::path::Path::new(&own)
        .join("projects")
        .join("-Users-me-code-app");
    let (original, copy) = (A, B);
    assert!(copy < original);
    Lines::write(
        &[
            Lines::user("start", original, 4000.0),
            Lines::assistant("m1", "r1", original)
                .usage(10, 1)
                .at(4010.0)
                .line(),
            Lines::assistant("m2", "r2", original)
                .usage(20, 2)
                .at(4020.0)
                .line(),
        ],
        &slug.join(format!("{original}.jsonl")),
        false,
    );
    // An older Claude Code resumed it into a new file, copying its last
    // response as the new session's.
    Lines::write(
        &[
            Lines::assistant("m2", "r2", copy)
                .usage(20, 2)
                .at(4020.0)
                .line(),
            Lines::user("go on", copy, 5000.0),
            Lines::assistant("m3", "r3", copy)
                .usage(5, 5)
                .at(5010.0)
                .line(),
        ],
        &slug.join(format!("{copy}.jsonl")),
        false,
    );
    *h.deps.backfill_folders.lock().unwrap() = vec![own_backfill(&own)];
    *h.deps.folder_logins.lock().unwrap() = Some(BTreeMap::from([(own, "login-1".to_owned())]));
    h.service.set_sync(false, h.now());
    h.tick();
    h.service.set_sync(true, h.now());
    h.advance(secs(2 * 3600));
    h.sync_now();
    let sessions = all_sessions(&h);
    let rows: BTreeMap<String, &Value> = sessions
        .iter()
        .map(|s| (s["sessionId"].as_str().unwrap_or("").to_owned(), s))
        .collect();
    assert_eq!(rows[original]["messageCount"], json!(2));
    assert_eq!(rows[copy]["messageCount"], json!(1));
    assert_eq!(rows[copy]["tokens"]["input"], json!(5));
}
