//! The sync service end to end, over the stand-in website and engine (the
//! Mac's `CloudSyncTests`: consent, what is sent, failures, summaries):
//! what may be sent, when, and what is never sent. Nothing here reaches a
//! real website or runs `claude`.

mod cloud_support;

use agentnotch_engine::cloud::contract::{date, SessionSource};
use agentnotch_engine::cloud::files::install_secret;
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::ledger::{CloudLedgerEntry, Origin};
use agentnotch_engine::cloud::pass::{CloudSyncPass, Record};
use agentnotch_engine::cloud::service::{
    backoff, INITIAL_BACKOFF, MAX_BACKOFF, SIGNED_OUT_OF_WEBSITE, SIGN_IN_NOT_ACCEPTED,
};
use agentnotch_engine::hub::DeepLinkOutcome;
use agentnotch_engine::model::{BackfillFolder, CloudAuthState, IdentityId, UsageSource};
use agentnotch_engine::platform::{HttpRequest, HttpResponse};
use agentnotch_engine::runtime_types::UsageObservation;
use agentnotch_engine::testkit::http::{body_json, header, json_response, path_of};
use agentnotch_engine::usage::scrubbed_env;
use cloud_support::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

const A: &str = CloudFixture::SESSION_A;
const B: &str = CloudFixture::SESSION_B;
const C: &str = CloudFixture::SESSION_C;

fn key_a() -> String {
    CloudLedgerEntry::key_of(A, CloudFixture::ACCOUNT_KEY)
}

fn options(f: impl FnOnce(&mut HarnessOptions)) -> HarnessOptions {
    let mut options = HarnessOptions::default();
    f(&mut options);
    options
}

fn started(options: HarnessOptions) -> Harness {
    let h = Harness::with(options);
    h.start();
    h
}

fn is_signed_in(h: &Harness) -> bool {
    matches!(h.service.state().auth, CloudAuthState::SignedIn { .. })
}

fn text_of(request: &HttpRequest) -> String {
    String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).into_owned()
}

/// The first session of the last sync request.
fn last_sent(h: &Harness) -> Value {
    let request = h.sync_requests().last().cloned().expect("a sync request");
    Harness::sessions(&request)
        .first()
        .cloned()
        .expect("a session in it")
}

fn tokens(input: i64, output: i64, creation: i64, read: i64) -> Value {
    json!({"input": input, "output": output, "cacheCreation": creation, "cacheRead": read})
}

fn with_header(mut response: HttpResponse, name: &str, value: &str) -> HttpResponse {
    response.headers.push((name.to_owned(), value.to_owned()));
    response
}

fn error_body(code: &str, message: &str) -> String {
    json!({"error": {"code": code, "message": message}}).to_string()
}

/// `path` in every form a request body could carry it.
fn spellings(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy().into_owned();
    vec![text.clone(), text.replace('\\', "\\\\")]
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- Consent ----

#[test]
fn nothing_is_captured_or_sent_while_sync_is_off() {
    let h = started(options(|o| o.sync_on = false));
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.service.record_usage(h.usage());
    h.tick();
    h.sync_now();
    assert!(h.requests().is_empty(), "sent while sync is off");
    assert_eq!(h.ledger_count(), 0);
    assert_eq!(h.pending_usage(), 0);
    assert!(is_signed_in(&h) && !h.service.state().sync_enabled);
}

#[test]
fn nothing_is_sent_while_signed_out() {
    let h = started(options(|o| o.signed_in = false));
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.service.record_usage(h.usage());
    h.tick();
    h.sync_now();
    assert!(h.requests().is_empty(), "sent while signed out");
    assert_eq!(h.service.state().auth, CloudAuthState::SignedOut);
    assert!(!h.service.can_upload());
    // Nor captured: the switch was agreed to for a sign-in.
    assert_eq!(h.ledger_count(), 0);
    assert_eq!(h.pending_usage(), 0);
}

#[test]
fn no_website_no_sync() {
    let h = started(options(|o| o.website = None));
    h.tick();
    assert!(h.requests().is_empty(), "sent with no website");
    // A session made through another website isn't used, and isn't deleted
    // either (a dev run pointed elsewhere shares the folder).
    assert_eq!(h.service.state().auth, CloudAuthState::SignedOut);
    assert!(h.saved_session().is_some());
}

// ---- What is sent ----

#[test]
fn a_sync_sends_names_and_numbers_only() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.service.record_usage(h.usage());
    h.tick();

    let requests = h.sync_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(header(request, "Authorization"), Some("Bearer access-1"));
    let text = text_of(request);
    let stores = h.service.stores().expect("stores");
    let secret = stores.secret();
    let mut leaks = vec![
        "MY SECRET PROMPT".to_owned(),
        "TOOL OUTPUT".to_owned(),
        "/Users/me".to_owned(),
        "Looking into it".to_owned(),
        "Fixed it.".to_owned(),
        CloudFixture::ACCOUNT_UUID.to_owned(),
        ".jsonl".to_owned(),
        hex(&secret),
        r"C:\Users".to_owned(),
        r"C:\\Users".to_owned(),
    ];
    leaks.extend(spellings(&std::fs::canonicalize(h.root.path()).unwrap()));
    leaks.extend(spellings(&h.handles.roots.home));
    for leaked in &leaks {
        assert!(!text.contains(leaked.as_str()), "{leaked} was sent");
    }
    // The install secret is kept beside the rest, privately.
    let secret_file = h.support().join(install_secret::FILE_NAME);
    assert_eq!(std::fs::read(&secret_file).unwrap(), secret);
    if cfg!(unix) {
        assert_eq!(
            agentnotch_engine::testkit::mode_of(&secret_file),
            Some(0o600)
        );
    }

    let body = body_json(request);
    assert_eq!(body["schemaVersion"], json!(1));
    assert!(keys::is_uuid(body["device"]["id"].as_str().unwrap_or("")));
    assert_eq!(body["device"]["name"], json!("TEST-PC"));
    let accounts = body["accounts"].as_array().expect("accounts");
    let account_keys: Vec<&str> = accounts.iter().filter_map(|a| a["key"].as_str()).collect();
    assert_eq!(account_keys, [CloudFixture::ACCOUNT_KEY]);
    assert_eq!(accounts[0]["email"], json!("me@example.com"));
    assert_eq!(accounts[0]["label"], json!("Personal"));
    let session = &body["sessions"][0];
    assert_eq!(session["sessionId"], json!(A));
    assert_eq!(session["project"]["name"], json!("app"));
    let path = keys::project_path(
        "/Users/me/code/app",
        &h.handles.roots.home,
        &*h.platform.files,
    );
    assert_eq!(
        session["project"]["key"],
        json!(keys::project_key(CloudFixture::ACCOUNT_KEY, &path, &secret))
    );
    assert_ne!(
        session["project"]["key"],
        json!(keys::sha256_hex(format!(
            "{}:{path}",
            CloudFixture::ACCOUNT_KEY
        )))
    );
    assert_eq!(session["title"], json!("Fix the login bug"));
    assert_eq!(session["source"], json!("cli"));
    assert_eq!(
        session["models"],
        json!(["claude-haiku-4-5", "claude-opus-4-5"])
    );
    assert_eq!(session["messageCount"], json!(2));
    assert_eq!(session["tokens"], tokens(110, 55, 1000, 5000));
    assert_eq!(session["costUsd"].as_f64(), Some(0.37));
    assert_eq!(session["endedAt"], Value::Null);
    assert!(session.get("summary").is_none());
    assert_eq!(
        session["startedAt"],
        json!(date::to_string(CloudFixture::base()))
    );
    let usage = &body["usage"][0];
    assert_eq!(usage["source"], json!("desktop"));
    assert_eq!(usage["accountKey"], json!(CloudFixture::ACCOUNT_KEY));

    let state = h.service.state();
    assert!(state.last_sync_at_ms.is_some() && state.last_error.is_none());
    assert_eq!((state.pending_usage, state.pending_sessions), (0, 0));
}

#[test]
fn an_unchanged_session_is_not_sent_again() {
    let mut h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 1);
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 1);

    // It grew: sent again, with its new totals.
    h.write_session(A, 3);
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 2);
    assert_eq!(last_sent(&h)["messageCount"], json!(5));

    // Remembered across a relaunch.
    h.relaunch();
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 2);
}

#[test]
fn only_allowed_accounts_are_ever_mentioned() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    let stranger = CloudFixture::work_account();
    h.observe_one(h.observation_for(A, &stranger));
    h.service.record_usage(UsageObservation {
        identity: stranger.identity_id.clone(),
        source: UsageSource::Probe,
        observed_at: CloudFixture::base(),
        windows: vec![("session".into(), 1.0, None)],
    });
    assert_eq!(h.ledger_count(), 0);
    assert_eq!(h.pending_usage(), 0);
    h.sync_now();
    assert!(h.sync_requests().is_empty());
}

/// While the hub can't attribute a running session for certain, the
/// responses it makes are never sent for any account; once a new process
/// of it is certain, only that process's count.
#[test]
fn responses_made_while_unsure_are_never_sent() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    let sent = |index: usize| -> Value {
        Harness::sessions(&h.sync_requests()[index])
            .first()
            .cloned()
            .expect("a session")
    };
    assert_eq!(sent(0)["messageCount"], json!(2));

    // Unsure (a mirrored folder switching accounts); it goes on answering.
    h.advance(Duration::from_secs(60));
    h.observe(Vec::new(), &[A], &[A]);
    h.append(
        A,
        &[
            Lines::assistant("u1", "ru1", A).usage(7, 7).at(30.0).line(),
            Lines::assistant("u2", "ru2", A).usage(7, 7).at(31.0).line(),
        ],
    );
    h.advance(Duration::from_secs(60));
    h.sync_now();
    for index in 0..h.sync_requests().len() {
        assert_eq!(sent(index)["messageCount"], json!(2), "request {index}");
        assert_eq!(
            sent(index)["tokens"]["output"],
            json!(55),
            "request {index}"
        );
    }

    // Certain again, in a new process: its own responses count.
    let mut resumed = h.observation(A);
    resumed.process_started_at = Some(CloudFixture::at(40.0));
    resumed.last_activity_at = CloudFixture::at(50.0);
    h.observe_one(resumed);
    h.append(
        A,
        &[Lines::assistant("c1", "rc1", A).usage(1, 1).at(50.0).line()],
    );
    h.advance(Duration::from_secs(60));
    h.sync_now();
    let last = last_sent(&h);
    assert_eq!(last["messageCount"], json!(3));
    assert_eq!(last["tokens"]["output"], json!(56));
    // Its stretch of nobody has responses: really split, so not Claude
    // Code's figure but its own responses at list prices (Opus 4.5's two,
    // Haiku 4.5's one).
    assert_eq!(sent(0)["costUsd"].as_f64(), Some(0.37));
    assert_eq!(last["costUsd"].as_f64(), Some(0.010565));
}

/// A session reported unsure only until the hub placed it was never really
/// split: its stretch of nobody holds no response, so it is sent whole,
/// with its cost.
#[test]
fn a_session_placed_a_moment_late_keeps_its_cost() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe(Vec::new(), &[A], &[A]);
    let mut placed = h.observation(A);
    placed.process_started_at = Some(CloudFixture::at(-5.0));
    h.observe_one(placed);
    assert_eq!(h.service.stores().unwrap().ledger.owners(A).len(), 2);
    h.sync_now();
    let session = last_sent(&h);
    assert_eq!(session["messageCount"], json!(2));
    assert_eq!(session["costUsd"].as_f64(), Some(0.37));
}

// ---- A shared history: only what the app saw running counts ----

/// Regression (double counting): a conversation in Claude Parallel Profiles'
/// shared history, first seen when an account continues it, is sent with only
/// that process's responses. The ones before it ran unseen (before sync, or
/// as another account): counting them for this account would credit it with
/// another's, or count them twice.
#[cfg(unix)]
#[test]
fn a_session_first_seen_in_a_shared_history_is_sent_from_its_process() {
    let h = started(HarnessOptions::default());
    let (window, transcript) = h.shared_window(A);
    Lines::write(
        &[
            // Earlier processes: two responses.
            Lines::user("start", A, 0.0),
            Lines::assistant("old-1", "ro1", A)
                .usage(100, 50)
                .at(10.0)
                .line(),
            Lines::assistant("old-2", "ro2", A)
                .usage(100, 50)
                .at(20.0)
                .line(),
            // This process: one.
            Lines::user("go on", A, 1000.0),
            Lines::assistant("new-1", "rn1", A)
                .usage(1, 2)
                .at(1010.0)
                .line(),
        ],
        &transcript,
        false,
    );
    let mut observation = h.observation(A);
    observation.config_dir = Some(window);
    observation.transcript_path = Some(transcript.to_string_lossy().into_owned());
    observation.process_started_at = Some(CloudFixture::at(990.0));
    observation.started_at = CloudFixture::at(995.0);
    observation.last_activity_at = CloudFixture::at(1010.0);
    h.observe_one(observation);
    h.sync_now();
    let session = last_sent(&h);
    assert_eq!(session["messageCount"], json!(1));
    assert_eq!(session["tokens"]["output"], json!(2));
    assert_eq!(session["startedAt"], json!(CloudFixture::stamp(1000.0)));
    // Not the process's status-line total (it may carry the earlier ones),
    // but its own response at list prices: Opus 4.5, 1 in, 2 out.
    assert_eq!(session["costUsd"].as_f64(), Some(0.000055));

    // The same layout, as a folder's own history: counted whole.
    let own = started(HarnessOptions::default());
    own.write_session(A, 0);
    let mut whole = own.observation(A);
    whole.process_started_at = Some(CloudFixture::at(15.0));
    own.observe_one(whole);
    own.sync_now();
    assert_eq!(last_sent(&own)["messageCount"], json!(2));
}

/// A new session in a shared history is all its process's: sent whole, with
/// Claude Code's own cost (no response is no one's).
#[cfg(unix)]
#[test]
fn a_new_session_in_a_shared_history_keeps_claude_codes_cost() {
    let h = started(HarnessOptions::default());
    let (window, transcript) = h.shared_window(A);
    h.write_session_to(A, 0, &transcript);
    let mut observation = h.observation(A);
    observation.config_dir = Some(window);
    observation.transcript_path = Some(transcript.to_string_lossy().into_owned());
    observation.process_started_at = Some(CloudFixture::at(-5.0));
    h.observe_one(observation);
    assert_eq!(h.service.stores().unwrap().ledger.owners(A).len(), 2);
    h.sync_now();
    let session = last_sent(&h);
    assert_eq!(session["messageCount"], json!(2));
    assert_eq!(session["costUsd"].as_f64(), Some(0.37));
}

/// A folder's history counts as shared when another folder the registry
/// knows reaches it too (profiles relinked after Claude Parallel Profiles
/// was removed, say), though no link leads to it.
#[cfg(unix)]
#[test]
fn a_history_another_folder_reaches_is_shared() {
    let h = Harness::new();
    let projects = h.projects().parent().unwrap().to_path_buf();
    let other = h.handles.roots.home.join(".claude-work");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::os::unix::fs::symlink(&projects, other.join("projects")).unwrap();
    *h.deps.backfill_folders.lock().unwrap() = vec![BackfillFolder {
        config_dir: other.to_string_lossy().into_owned(),
        identity_id: None,
        account_key: None,
        signed_in_since: None,
    }];
    h.start();
    h.write_session(A, 0);
    let mut observation = h.observation(A);
    observation.process_started_at = Some(CloudFixture::at(15.0));
    h.observe_one(observation);
    h.sync_now();
    // Only the response after its process started.
    assert_eq!(last_sent(&h)["messageCount"], json!(1));
}

/// Regression (double counting): a shared history's session the app saw
/// end, continued while the app wasn't capturing (as another account, say),
/// then resumed in a process the app sees: what was written in between
/// counts for no one, not for the account it ran as.
#[cfg(unix)]
#[test]
fn what_a_shared_session_did_unseen_counts_for_no_one() {
    let h = started(HarnessOptions::default());
    let (window, transcript) = h.shared_window(A);
    h.write_session_to(A, 0, &transcript);
    let mut observation = h.observation(A);
    observation.config_dir = Some(window);
    observation.transcript_path = Some(transcript.to_string_lossy().into_owned());
    observation.process_started_at = Some(CloudFixture::at(-5.0));
    h.observe_one(observation.clone());
    h.sync_now();
    assert_eq!(last_sent(&h)["messageCount"], json!(2));
    // It ends, and the app sees it go.
    h.observe(Vec::new(), &[], &[]);
    h.advance(Duration::from_secs(61));
    h.service.tick(h.now());
    // Continued unseen (sync off meanwhile, say): two responses.
    h.service.set_sync(false, h.now());
    Lines::write(
        &[
            Lines::user("other account", A, 4000.0),
            Lines::assistant("gap-1", "rg1", A)
                .usage(500, 500)
                .at(4010.0)
                .line(),
            Lines::assistant("gap-2", "rg2", A)
                .usage(500, 500)
                .at(4020.0)
                .line(),
        ],
        &transcript,
        true,
    );
    h.service.set_sync(true, h.now());
    // Resumed as its account in a new process the app sees.
    Lines::write(
        &[Lines::assistant("back-1", "rb1", A)
            .usage(1, 3)
            .at(5010.0)
            .line()],
        &transcript,
        true,
    );
    h.handles.clock.set(CloudFixture::at(5020.0));
    let mut resumed = observation;
    resumed.process_started_at = Some(CloudFixture::at(5000.0));
    resumed.started_at = CloudFixture::at(5001.0);
    resumed.last_activity_at = CloudFixture::at(5010.0);
    h.observe_one(resumed);
    h.sync_now();
    let session = last_sent(&h);
    assert_eq!(session["messageCount"], json!(3));
    assert_eq!(session["tokens"]["output"], json!(58));
}

/// Regression (double counting): an account whose key changes while a
/// session of it runs (its organization became known) hands the session over
/// in the same process. The old key's row is never sent again, so its last
/// response mustn't count for the new key as well.
#[test]
fn a_key_change_mid_session_counts_the_last_response_once() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    let mut observation = h.observation(A);
    observation.process_started_at = Some(CloudFixture::at(-5.0));
    h.observe_one(observation.clone());
    h.sync_now();
    assert_eq!(last_sent(&h)["messageCount"], json!(2));
    // The same Claude account, keyed with its organization from now on.
    let rekeyed = CloudAccountBuilder::new(CloudFixture::IDENTITY_ID)
        .email("me@example.com")
        .plan("Max 20x")
        .label("Personal")
        .folder(CloudAccountBuilder::run_folder(
            "/Users/me/.claude",
            Some(CloudFixture::WORK_ORGANIZATION),
        ))
        .build();
    let new_key = keys::account_key_of(&rekeyed).expect("a key");
    assert_ne!(new_key, CloudFixture::ACCOUNT_KEY);
    *h.deps.accounts.lock().unwrap() = vec![rekeyed];
    h.append(
        A,
        &[Lines::assistant("after", "ra", A)
            .usage(1, 1)
            .at(30.0)
            .line()],
    );
    observation.last_activity_at = CloudFixture::at(30.0);
    h.observe_one(observation);
    h.sync_now();
    let session = last_sent(&h);
    assert_eq!(session["accountKey"], json!(new_key));
    assert_eq!(session["messageCount"], json!(1));
}

/// The website counts a session once however many people's copies it holds:
/// a PC signed in as someone else is sent what it captured before, under the
/// same session id and project key.
#[test]
fn another_website_user_is_sent_what_was_captured_before() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    let first = last_sent(&h);

    h.service.sign_out(h.now());
    h.handles.http.set_handler(|request| {
        if path_of(request) == "/api/app/v1/me" {
            return Ok(json_response(
                200,
                json!({"user": {"id": "22222222-3333-4444-8555-666666666666",
                                "email": "other@example.com", "name": null},
                       "dashboardUrl": "https://agentnotch.example.com/dashboard"})
                .to_string(),
            ));
        }
        Ok(website_answer(request))
    });
    assert_eq!(h.sign_in(), DeepLinkOutcome::SignInCompleted);
    assert_eq!(
        h.service.stores().unwrap().memory.user_id().as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    h.service.set_sync(true, h.now());
    let before = h.sync_requests().len();
    // No sighting since: the session comes from the ledger.
    h.sync_now();
    assert_eq!(h.sync_requests().len(), before + 1);
    let again = last_sent(&h);
    assert_eq!(again["sessionId"], json!(A));
    assert_eq!(again["project"]["key"], first["project"]["key"]);
    assert_eq!(again["messageCount"], first["messageCount"]);
}

/// A session no status line reported a cost for (the VS Code extension's
/// chat panel, Claude Desktop, the SDK) is sent with its responses at list
/// prices; Claude Code's own figure wins once it gives one.
#[test]
fn a_session_without_claude_codes_cost_is_priced_from_its_transcript() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    let mut panel = h.observation(A);
    panel.entrypoint = Some("claude-vscode".into());
    panel.cost_usd = None;
    h.observe_one(panel.clone());
    h.sync_now();
    assert_eq!(last_sent(&h)["source"], json!("vscode"));
    // Opus 4.5: 100 in, 50 out, 1000 written, 5000 read; Haiku 4.5: 10 in, 5 out.
    assert_eq!(last_sent(&h)["costUsd"].as_f64(), Some(0.010535));

    panel.cost_usd = Some(0.37);
    h.observe_one(panel.clone());
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 2);
    assert_eq!(last_sent(&h)["costUsd"].as_f64(), Some(0.37));

    // A figure the session outgrew: the estimate, which is larger.
    panel.cost_usd = Some(0.004);
    h.observe_one(panel);
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 3);
    assert_eq!(last_sent(&h)["costUsd"].as_f64(), Some(0.010535));
}

/// Sessions sent as ended before costs were estimated are built again
/// once: sent again when that gives them a cost, only remembered as built
/// the new way when it doesn't change them.
#[test]
fn sessions_sent_before_costs_were_estimated_are_built_again_once() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    let mut panel = h.observation(A);
    panel.entrypoint = Some("claude-vscode".into());
    panel.cost_usd = None;
    h.observe_one(panel);
    h.observe(Vec::new(), &[], &[]);
    h.advance(Duration::from_secs(61));
    h.tick();
    h.sync_now();
    let stores = h.service.stores().expect("stores");
    let key = stores.ledger.entry(A).expect("in the ledger").key();
    let sent = stores.memory.sent(&key).expect("sent");
    assert!(sent.ended && sent.version == Some(CloudSyncPass::PAYLOAD_VERSION));
    let requests = h.sync_requests().len();

    // As an earlier version remembers it: sent as ended, with no cost.
    let earlier = |base: &str| Record {
        base: base.to_owned(),
        summary: None,
        ended: true,
        transcript: sent.transcript,
        version: None,
    };
    stores.memory.mark_sent(
        &BTreeMap::from([(key.clone(), earlier("sent without a cost"))]),
        h.now(),
    );
    h.sync_now();
    assert_eq!(h.sync_requests().len(), requests + 1);
    let session = last_sent(&h);
    assert_eq!(session["costUsd"].as_f64(), Some(0.010535));
    assert!(session["endedAt"].is_string());
    h.sync_now();
    assert_eq!(h.sync_requests().len(), requests + 1);

    // One the new way leaves as it was: not sent, remembered as built anew.
    stores.memory.mark_sent(
        &BTreeMap::from([(key.clone(), earlier(&sent.base))]),
        h.now(),
    );
    h.sync_now();
    assert_eq!(h.sync_requests().len(), requests + 1);
    assert_eq!(
        stores.memory.sent(&key).and_then(|r| r.version),
        Some(CloudSyncPass::PAYLOAD_VERSION)
    );
}

/// A session Claude Desktop hosts is recorded as the account the hub found
/// Desktop's record of it under, and only then; one the hub couldn't
/// attribute (reported unsure) never is.
#[test]
fn desktop_sessions_are_recorded_as_the_hub_attributes_them() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe(Vec::new(), &[A], &[A]);
    h.sync_now();
    assert_eq!(h.ledger_count(), 0);
    assert!(h.sync_requests().is_empty());
    // Its record found: the hub attributes it, and it is recorded as that
    // account's.
    h.advance(Duration::from_secs(120));
    let mut desktop = h.observation(A);
    desktop.entrypoint = Some("claude-desktop".into());
    h.observe_one(desktop);
    let entry = h
        .service
        .stores()
        .unwrap()
        .ledger
        .entry(A)
        .expect("recorded");
    assert_eq!(entry.source, SessionSource::Desktop);
    assert_eq!(entry.account_key, CloudFixture::ACCOUNT_KEY);
    // `local-agent` is Claude Desktop's too.
    assert_eq!(
        SessionSource::from_entrypoint(Some("local-agent")),
        SessionSource::Desktop
    );
}

#[test]
fn a_folders_own_history_is_backfilled() {
    let h = started(HarnessOptions::default());
    let own = h.handles.roots.home.join(".claude-own");
    let slug = own.join("projects").join("-Users-me-work-billing");
    let billing = "/Users/me/work/billing";
    Lines::write(
        &[
            Lines::user_in("MY SECRET PROMPT", B, 0.0, billing, "cli"),
            Lines::assistant("b1", "rb1", B)
                .usage(3, 4)
                .at(60.0)
                .cwd(billing)
                .line(),
        ],
        &slug.join(format!("{B}.jsonl")),
        false,
    );
    // Hosted by Claude Desktop, whose account can't be told here: left out.
    Lines::write(
        &[
            Lines::user_in("x", C, 0.0, billing, "claude-desktop"),
            Lines::assistant("c1", "rc1", C).usage(1, 1).at(5.0).line(),
        ],
        &slug.join(format!("{C}.jsonl")),
        false,
    );
    let own_text = own.to_string_lossy().into_owned();
    *h.deps.backfill_folders.lock().unwrap() = vec![BackfillFolder {
        config_dir: own_text.clone(),
        identity_id: Some(IdentityId::from(CloudFixture::IDENTITY_ID)),
        account_key: Some(CloudFixture::ACCOUNT_KEY.to_owned()),
        signed_in_since: None,
    }];
    // The folder was seen signed in as this account from before these
    // sessions.
    *h.deps.folder_logins.lock().unwrap() =
        Some(BTreeMap::from([(own_text, "login-1".to_owned())]));
    let now = h.now();
    h.handles.clock.set(CloudFixture::at(-60.0));
    h.service.set_sync(false, h.now());
    h.tick();
    h.service.set_sync(true, h.now());
    h.handles.clock.set(now);
    h.sync_now();

    let request = h.sync_requests().first().cloned().expect("a sync");
    let sessions = Harness::sessions(&request);
    let ids: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s["sessionId"].as_str())
        .collect();
    assert_eq!(ids, [B]);
    let session = &sessions[0];
    assert_eq!(session["project"]["name"], json!("billing"));
    let secret = h.service.stores().unwrap().secret();
    let path = keys::project_path(billing, &h.handles.roots.home, &*h.platform.files);
    assert_eq!(
        session["project"]["key"],
        json!(keys::project_key(CloudFixture::ACCOUNT_KEY, &path, &secret))
    );
    assert_eq!(session["endedAt"], json!(CloudFixture::stamp(60.0)));
    assert_eq!(session["source"], json!("cli"));
    assert_eq!(session["title"], Value::Null);
    let text = text_of(&request);
    assert!(!text.contains("MY SECRET PROMPT") && !text.contains("/Users/me"));
    let ledger = &h.service.stores().unwrap().ledger;
    assert_eq!(ledger.entry(B).map(|e| e.origin), Some(Origin::Backfill));
    assert!(ledger.entry(C).is_none());
}

// ---- Failures ----

#[test]
fn failures_back_off_and_honour_retry_after() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.handles.http.set_handler(|request| {
        if path_of(request) == "/api/app/v1/sync" {
            return Ok(json_response(
                500,
                error_body("INTERNAL", "Something went wrong. Try again later."),
            ));
        }
        Ok(website_answer(request))
    });
    h.tick();
    assert_eq!(h.sync_requests().len(), 1);
    assert_eq!(
        h.service.state().last_error.as_deref(),
        Some("Something went wrong. Try again later.")
    );
    h.tick();
    assert_eq!(h.sync_requests().len(), 1);
    h.advance(INITIAL_BACKOFF + Duration::from_secs(1));
    h.tick();
    assert_eq!(h.sync_requests().len(), 2);
    // The second failure waits twice as long.
    h.advance(INITIAL_BACKOFF + Duration::from_secs(1));
    h.tick();
    assert_eq!(h.sync_requests().len(), 2);

    h.handles.http.set_handler(|request| {
        if path_of(request) == "/api/app/v1/sync" {
            let answer = json_response(
                429,
                error_body("RATE_LIMITED", "Too many syncs. Try again in 600 s."),
            );
            return Ok(with_header(answer, "Retry-After", "600"));
        }
        Ok(website_answer(request))
    });
    h.advance(INITIAL_BACKOFF * 2);
    h.tick();
    assert_eq!(h.sync_requests().len(), 3);
    h.advance(Duration::from_secs(300));
    h.tick();
    assert_eq!(h.sync_requests().len(), 3);

    h.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));
    h.advance(Duration::from_secs(301));
    h.tick();
    assert_eq!(h.sync_requests().len(), 4);
    assert_eq!(h.service.state().last_error, None);
    assert_eq!(backoff(1), Duration::from_secs(30));
    assert_eq!(backoff(3), Duration::from_secs(120));
    assert_eq!(backoff(40), MAX_BACKOFF);
}

/// The website refusing a token the sign-in service just refreshed (its
/// key set couldn't be fetched, say) backs off; the sign-in is kept.
#[test]
fn a_refusal_after_a_refresh_backs_off_and_keeps_the_sign_in() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.handles.http.set_handler(|request| {
        if path_of(request) == "/api/app/v1/sync" {
            return Ok(json_response(401, contract_fixture("error.json")));
        }
        Ok(website_answer(request))
    });
    h.sync_now();
    // Once, a refresh, once more: then a backoff, still signed in.
    assert_eq!(h.sync_requests().len(), 2);
    assert_eq!(h.requests_to("/auth/v1/token").len(), 1);
    assert!(is_signed_in(&h) && h.service.state().sync_enabled);
    assert_eq!(
        h.service.state().last_error.as_deref(),
        Some(SIGN_IN_NOT_ACCEPTED)
    );
    assert_eq!(
        SIGN_IN_NOT_ACCEPTED,
        "The website didn't accept this PC's sign-in. Trying again later."
    );
    assert_eq!(
        h.saved_session().map(|s| s.refresh_token).as_deref(),
        Some("refresh-2")
    );
    h.tick();
    assert_eq!(h.sync_requests().len(), 2);
    // The website is fine again: the same sign-in works.
    h.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));
    h.advance(INITIAL_BACKOFF + Duration::from_secs(1));
    h.tick();
    assert_eq!(h.sync_requests().len(), 3);
    assert!(h.service.state().last_error.is_none() && is_signed_in(&h));
}

/// Only Supabase refusing the refresh token itself ends the sign-in; the
/// sync switch goes off with it.
#[test]
fn a_refused_refresh_token_signs_out() {
    let h = started(HarnessOptions::default());
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    // Supabase down: a 503 from its token endpoint is tried again later.
    h.handles
        .http
        .set_handler(|request| match path_of(request).as_str() {
            "/api/app/v1/sync" => Ok(json_response(401, contract_fixture("error.json"))),
            "/auth/v1/token" => Ok(json_response(
                503,
                json!({"msg": "Service Unavailable"}).to_string(),
            )),
            _ => Ok(website_answer(request)),
        });
    h.sync_now();
    assert!(is_signed_in(&h));
    assert_eq!(
        h.saved_session().map(|s| s.refresh_token).as_deref(),
        Some("refresh-1")
    );

    h.handles
        .http
        .set_handler(|request| match path_of(request).as_str() {
            "/api/app/v1/sync" => Ok(json_response(401, contract_fixture("error.json"))),
            "/auth/v1/token" => Ok(json_response(
                400,
                json!({"code": 400, "error_code": "refresh_token_already_used",
                   "msg": "Invalid Refresh Token: Already Used"})
                .to_string(),
            )),
            _ => Ok(website_answer(request)),
        });
    h.advance(MAX_BACKOFF);
    h.sync_now();
    let state = h.service.state();
    assert_eq!(state.auth, CloudAuthState::SignedOut);
    assert_eq!(state.last_error.as_deref(), Some(SIGNED_OUT_OF_WEBSITE));
    assert!(h.saved_session().is_none());
    assert!(!state.sync_enabled);
    assert_eq!(h.deps.setting("cloudSyncEnabled"), Some(json!(false)));
    let sent = h.sync_requests().len();
    h.tick();
    assert_eq!(h.sync_requests().len(), sent);
}

// ---- Summaries ----

/// A session seen running, synced once (so its totals are known), then
/// gone for good: ended.
fn ended_session(h: &Harness) {
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    h.observe(Vec::new(), &[], &[]);
    h.advance(Duration::from_secs(61));
    h.tick();
    assert!(h
        .service
        .stores()
        .unwrap()
        .ledger
        .entry(A)
        .and_then(|e| e.ended_at)
        .is_some());
}

fn summaries_on() -> HarnessOptions {
    options(|o| o.summaries_on = true)
}

#[test]
fn summaries_never_run_while_off() {
    let h = started(HarnessOptions::default());
    ended_session(&h);
    h.answer_summary(SUMMARY_ANSWER);
    h.advance(Duration::from_secs(11 * 60));
    h.tick();
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());
    assert!(!h.service.can_summarize(h.now()));

    // On, but this run may not launch Claude Code (sealed, a dev run…).
    // A session that ends after the switch went on could be summarised.
    h.service.set_summaries(true, h.now());
    h.write_session(C, 0);
    h.observe_one(h.observation(C));
    h.sync_now();
    h.observe(Vec::new(), &[], &[]);
    h.advance(Duration::from_secs(61));
    h.tick();
    h.advance(Duration::from_secs(11 * 60));
    let mut cfg = h.config();
    cfg.summaries_allowed_by_default = false;
    h.service.update_config(cfg, h.now());
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());
    // On, but sync off.
    h.service.update_config(h.config(), h.now());
    h.service.set_sync(false, h.now());
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());
    // Something else is launching Claude Code right now.
    h.service.set_sync(true, h.now());
    h.deps.launching_claude.store(true, Ordering::SeqCst);
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());
    // Nothing held it back but those: it runs once they are gone, for the
    // session that ended after the switch went on only.
    h.deps.launching_claude.store(false, Ordering::SeqCst);
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
fn a_summary_is_written_once_in_the_right_folder_and_sent() {
    let h = started(summaries_on());
    ended_session(&h);
    h.answer_summary(SUMMARY_ANSWER);
    // Not ten minutes yet.
    h.summarize_next();
    assert!(h.handles.runner.spawned().is_empty());

    h.advance(Duration::from_secs(10 * 60));
    h.summarize_next();
    let spawned = h.handles.runner.spawned();
    assert_eq!(spawned.len(), 1);
    assert_eq!(
        *h.deps.folder_requests.lock().unwrap(),
        [IdentityId::from(CloudFixture::IDENTITY_ID)]
    );
    // In the account's folder: its login, the probe's environment.
    let folder = summary_folder(&h.handles.roots.home);
    let binary = h.deps.claude_binary.lock().unwrap().clone().unwrap();
    let base: Vec<_> = std::env::vars_os().collect();
    assert_eq!(
        spawned[0].env,
        scrubbed_env(
            &base,
            folder.config_dir_env.as_deref(),
            binary.program.parent().unwrap_or(Path::new(""))
        )
    );
    assert_eq!(spawned[0].program, binary.program);
    assert_eq!(
        spawned[0].cwd.file_name().and_then(|n| n.to_str()),
        Some("session-summary")
    );
    assert_eq!(spawned[0].cwd, h.support().join("session-summary"));
    let input = String::from_utf8(h.handles.runner.stdin_of(0).unwrap()).unwrap();
    assert!(input.contains("User: MY SECRET PROMPT about the login bug"));
    assert!(input.contains("Claude: Fixed it."));
    assert!(!input.contains("TOOL OUTPUT"));

    // Once only.
    h.answer_summary(SUMMARY_ANSWER);
    h.summarize_next();
    assert_eq!(h.handles.runner.spawned().len(), 1);

    // Sent with the session (the only change since it was last sent).
    h.sync_now();
    let session = last_sent(&h);
    assert_eq!(
        session["summary"]["text"],
        json!("Fixed the token refresh.")
    );
    assert_eq!(
        session["summary"]["model"],
        json!("claude-haiku-4-5-20251001")
    );
    assert_eq!(h.service.state().summarized_sessions, 1);

    // Off again: not sent any more, and nothing resent for it.
    let sent = h.sync_requests().len();
    h.service.set_summaries(false, h.now());
    h.sync_now();
    assert_eq!(h.sync_requests().len(), sent);
}

#[test]
fn a_summary_from_a_folder_that_changed_hands_is_dropped() {
    let h = started(summaries_on());
    ended_session(&h);
    h.answer_summary(SUMMARY_ANSWER);
    h.advance(Duration::from_secs(11 * 60));
    h.deps
        .summary_folder_still_runs
        .store(false, Ordering::SeqCst);
    h.summarize_next();
    assert_eq!(h.handles.runner.spawned().len(), 1);
    let summaries = &h.service.stores().unwrap().summaries;
    assert!(summaries.summary(&key_a()).is_none());
    assert!(summaries.attempt(&key_a()).is_some());

    // No folder signed in as the account: not run at all.
    let other = started(summaries_on());
    ended_session(&other);
    other.answer_summary(SUMMARY_ANSWER);
    other.advance(Duration::from_secs(11 * 60));
    *other.deps.summary_folder.lock().unwrap() = None;
    other.summarize_next();
    assert!(other.handles.runner.spawned().is_empty());
    let attempt = other
        .service
        .stores()
        .unwrap()
        .summaries
        .attempt(&key_a())
        .expect("it waits");
    assert!(attempt.next_attempt_at >= other.now() + Duration::from_secs(60 * 60));
}

/// The tick runs a summary on a thread of its own: a `claude` that takes
/// its time holds up neither the schedule nor the calls, and turning
/// summaries off stops it at once.
#[test]
fn a_summary_runs_beside_the_schedule_and_stops_when_turned_off() {
    let h = started(summaries_on());
    ended_session(&h);
    h.advance(Duration::from_secs(11 * 60));
    let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
    let hanging = std::sync::Arc::new(HangingRunner::new(started_tx));
    // The same files, and a runner whose `claude` never ends by itself.
    h.service.stop(h.now());
    let service = std::sync::Arc::new(agentnotch_engine::cloud::service::CloudSync::new(
        h.config(),
        h.deps.clone(),
        &agentnotch_engine::platform::Platform {
            runner: hanging.clone(),
            ..h.platform.clone()
        },
    ));
    service.start(h.now());
    service.tick(h.now());
    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the summary started");
    assert!(service.is_summarizing());
    // The schedule goes on meanwhile.
    service.sync_now(h.now(), true);
    service.set_summaries(false, h.now());
    assert!(
        eventually(Duration::from_secs(10), || !service.is_summarizing()),
        "the summary was stopped"
    );
    assert!(hanging.killed());
    assert!(service
        .stores()
        .unwrap()
        .summaries
        .summary(&key_a())
        .is_none());
}
