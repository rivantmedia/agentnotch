//! Regressions for the fix check's open items (the Mac's
//! `CloudSyncBindingTests`), over the same stand-in website and engine as
//! `cloud_sync.rs`: a pass and each request stay with the sign-in and
//! website they started with, a split session's parts are each priced by
//! their own responses, a pass sends at most five requests, the install
//! secret is made only when a project key is needed, and turning summaries
//! off deletes the ones never sent.
//!
//! "In flight" is reproduced by the stand-in network itself: its handler
//! signs out and in again while it answers the request it holds, on the
//! thread that sent it, exactly between the send and the answer.

mod cloud_support;

use agentnotch_engine::cloud::api::{ApiError, CloudApi};
use agentnotch_engine::cloud::auth::{Auth, AuthError, MemorySessionStore};
use agentnotch_engine::cloud::contract::{SyncDevice, SyncRequest};
use agentnotch_engine::cloud::files::install_secret;
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::ledger::CloudLedgerEntry;
use agentnotch_engine::cloud::pass::CloudSyncPass;
use agentnotch_engine::cloud::service::{CloudSync, SOON_DELAY};
use agentnotch_engine::hub::DeepLinkOutcome;
use agentnotch_engine::model::{CloudAuthState, IdentityId, UsageSource};
use agentnotch_engine::platform::{Clock, HttpRequest, HttpResponse};
use agentnotch_engine::runtime_types::UsageObservation;
use agentnotch_engine::testkit::http::{header, json_response, path_of, FixtureHttp};
use agentnotch_engine::testkit::FakeClock;
use agentnotch_engine::testkit::TEST_START_MS;
use cloud_support::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

const A: &str = CloudFixture::SESSION_A;
const C: &str = CloudFixture::SESSION_C;
const D: &str = "44444444-5555-4666-8777-888888888888";

fn key(session: &str) -> String {
    CloudLedgerEntry::key_of(session, CloudFixture::ACCOUNT_KEY)
}

fn empty_request() -> SyncRequest {
    SyncRequest {
        schema_version: 1,
        device: SyncDevice {
            id: "5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d".into(),
            name: "PC".into(),
            app_version: "1".into(),
        },
        accounts: Vec::new(),
        sessions: Vec::new(),
        usage: Vec::new(),
    }
}

fn host_of(request: &HttpRequest) -> String {
    url::Url::parse(&request.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn started(options: HarnessOptions) -> Harness {
    let h = Harness::with(options);
    h.start();
    h
}

fn authorizations(requests: &[HttpRequest]) -> Vec<Option<String>> {
    requests
        .iter()
        .map(|r| header(r, "Authorization").map(str::to_owned))
        .collect()
}

// ---- A request's 401 retry keeps to its sign-in and website ----

/// An `Auth` holding the fixture's session (access-1), over its own
/// stand-in network.
fn auth_rig() -> (Arc<FixtureHttp>, Arc<Auth>, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::at_ms(TEST_START_MS));
    let http = Arc::new(FixtureHttp::default());
    let store = Arc::new(MemorySessionStore::new(Some(AuthFixture::session(
        3600,
        clock.now(),
    ))));
    let auth = Arc::new(Auth::new(http.clone(), store, clock.clone()));
    (http, auth, clock)
}

/// The first send goes out with website A's token; before A answers 401 a
/// sign-in made through website B replaces it (a run pointed at B with
/// `AGENTNOTCH_WEB_URL` shares the session file). The retry must not carry
/// B's token to A.
#[test]
fn a_retry_never_carries_another_websites_token() {
    let (http, auth, clock) = auth_rig();
    let held: Weak<Auth> = Arc::downgrade(&auth);
    let first = Arc::new(FirstOnly::default());
    http.set_handler(
        move |request| match (host_of(request).as_str(), path_of(request).as_str()) {
            ("agentnotch.example.com", "/api/app/v1/sync") => {
                if first.take() {
                    if let Some(auth) = held.upgrade() {
                        auth.sign_out();
                        let mut other =
                            AuthFixture::session_with(3600, clock.now(), "b-access", "b-refresh");
                        other.website_url = "https://other.example.com".into();
                        auth.adopt(other);
                    }
                }
                Ok(json_response(401, contract_fixture("error.json")))
            }
            (_, "/auth/v1/token") => Ok(AuthFixture::token_answer("b-refreshed", "b-refresh-2")),
            _ => Ok(json_response(204, "")),
        },
    );
    let api = CloudApi::new(AuthFixture::WEBSITE, http.clone(), Some(auth.clone()), "1");
    assert_eq!(
        api.sync(&empty_request()).unwrap_err(),
        ApiError::Auth(AuthError::OtherWebsite)
    );
    let sent_to_a: Vec<HttpRequest> = http
        .requests()
        .into_iter()
        .filter(|r| host_of(r) == "agentnotch.example.com")
        .collect();
    assert_eq!(
        authorizations(&sent_to_a),
        [Some("Bearer access-1".to_owned())]
    );
    // B's session wasn't refreshed for A's request either.
    assert!(http.requests_to("/auth/v1/token").is_empty());
    assert_eq!(
        auth.current_session().map(|s| s.access_token).as_deref(),
        Some("b-access")
    );
}

/// The same, on one website: signed out and in again (as anyone) while the
/// request was out. The old request's retry is not the new sign-in's.
#[test]
fn a_retry_never_carries_a_later_sign_ins_token() {
    let (http, auth, clock) = auth_rig();
    let held: Weak<Auth> = Arc::downgrade(&auth);
    let first = Arc::new(FirstOnly::default());
    http.set_handler(move |request| match path_of(request).as_str() {
        "/api/app/v1/sync" => {
            if first.take() {
                if let Some(auth) = held.upgrade() {
                    auth.sign_out();
                    auth.adopt(AuthFixture::session_with(
                        3600,
                        clock.now(),
                        "new-access",
                        "new-refresh",
                    ));
                }
            }
            Ok(json_response(401, contract_fixture("error.json")))
        }
        "/auth/v1/token" => Ok(AuthFixture::token_answer("new-refreshed", "new-refresh-2")),
        _ => Ok(json_response(204, "")),
    });
    let api = CloudApi::new(AuthFixture::WEBSITE, http.clone(), Some(auth.clone()), "1");
    assert_eq!(
        api.sync(&empty_request()).unwrap_err(),
        ApiError::Auth(AuthError::SignedOut)
    );
    assert_eq!(
        authorizations(&http.requests_to("/api/app/v1/sync")),
        [Some("Bearer access-1".to_owned())]
    );
    assert!(http.requests_to("/auth/v1/token").is_empty());
    // A 401 for the sign-in in use is still retried once after a refresh.
    let grant = auth.access_grant(AuthFixture::WEBSITE).expect("a grant");
    assert_eq!(grant.token, "new-access");
    let fresh = auth
        .refresh_after_unauthorized(&grant.token, &grant.website, grant.sign_in)
        .expect("refreshed");
    assert_eq!(fresh, "new-refreshed");
}

// ---- A pass's result belongs to its sign-in ----

/// Holds the pass's first request, signs out and in again meanwhile (the
/// website user changes: `me.json`'s id, where the pass began with none),
/// then lets the request answer with `answer`.
fn pass_across_a_new_sign_in(
    h: &Harness,
    answer: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
) {
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    let service: Weak<CloudSync> = Arc::downgrade(&h.service);
    let clock = h.handles.clock.clone();
    let first = Arc::new(FirstOnly::default());
    let outcome = Arc::new(Mutex::new(None));
    let seen = outcome.clone();
    h.handles.http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/sync" && first.take() {
            if let Some(service) = service.upgrade() {
                service.sign_out(clock.now());
                let started = service.sign_in(clock.now()).is_ok();
                let finished =
                    service.deep_link("agentnotch://auth-callback?code=again", clock.now());
                *seen.lock().unwrap() = Some((started, finished));
            }
            return Ok(answer(request));
        }
        Ok(website_answer(request))
    });
    h.sync_now();
    assert_eq!(
        *outcome.lock().unwrap(),
        Some((true, DeepLinkOutcome::SignInCompleted))
    );
    assert!(matches!(
        h.service.state().auth,
        CloudAuthState::SignedIn { .. }
    ));
    h.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));
}

#[test]
fn a_pass_answered_after_a_new_sign_in_marks_nothing_sent() {
    let h = started(HarnessOptions::default());
    pass_across_a_new_sign_in(&h, |_| {
        json_response(200, contract_fixture("sync-response.json"))
    });
    // The old sign-in's batch isn't remembered as the new one's.
    let stores = h.service.stores().unwrap();
    assert!(stores.memory.sent(&key(A)).is_none());
    assert!(stores.memory.last_sync_at().is_none());

    // So once the user turns sync on for the new sign-in, it is sent to it.
    h.service.set_sync(true, h.now());
    h.observe_one(h.observation(A));
    h.sync_now();
    let requests = h.sync_requests();
    assert_eq!(requests.len(), 2);
    let last = requests.last().unwrap();
    assert_eq!(header(last, "Authorization"), Some("Bearer access-2"));
    assert_eq!(Harness::sessions(last)[0]["sessionId"], json!(A));
}

/// A 401 to the old sign-in's request: no retry with the new token, and the
/// new sign-in isn't signed out for it.
#[test]
fn a_refusal_after_a_new_sign_in_is_not_held_against_it() {
    let h = started(HarnessOptions::default());
    pass_across_a_new_sign_in(&h, |_| json_response(401, contract_fixture("error.json")));
    assert_eq!(h.sync_requests().len(), 1);
    let state = h.service.state();
    assert_eq!(
        state.auth,
        CloudAuthState::SignedIn {
            email: Some("me@example.com".into())
        }
    );
    assert_eq!(state.last_error, None);
    assert_eq!(
        h.saved_session().map(|s| s.access_token).as_deref(),
        Some("access-2")
    );
}

/// A server error to the old sign-in's request: no error shown and no
/// backoff for the new sign-in.
#[test]
fn a_failure_after_a_new_sign_in_is_not_held_against_it() {
    let h = started(HarnessOptions::default());
    pass_across_a_new_sign_in(&h, |_| {
        let mut answer = json_response(
            503,
            json!({"error": {"code": "INTERNAL", "message": "Down."}}).to_string(),
        );
        answer
            .headers
            .push(("Retry-After".to_owned(), "600".to_owned()));
        answer
    });
    let state = h.service.state();
    assert!(state.last_error.is_none() && matches!(state.auth, CloudAuthState::SignedIn { .. }));
    h.service.set_sync(true, h.now());
    h.observe_one(h.observation(A));
    h.tick();
    assert_eq!(h.sync_requests().len(), 2);
    assert_eq!(h.service.state().last_error, None);
}

// ---- A split session: each part priced on its own ----

#[test]
fn a_session_split_across_accounts_prices_each_part_on_its_own() {
    let h = Harness::new();
    let (personal, work) = (CloudFixture::account(), CloudFixture::work_account());
    *h.deps.accounts.lock().unwrap() = vec![personal.clone(), work.clone()];
    h.start();
    Lines::write(
        &[
            Lines::user("start", A, 0.0),
            Lines::assistant("m1", "r1", A)
                .usage(100, 10)
                .at(10.0)
                .line(),
        ],
        &h.transcript(A),
        false,
    );
    // Run by one account so far: its cost goes.
    let mut mine0 = h.observation_for(A, &personal);
    mine0.last_activity_at = CloudFixture::at(10.0);
    h.observe_one(mine0);
    h.sync_now();
    let first = Harness::sessions(&h.sync_requests()[0]);
    assert_eq!(first[0]["costUsd"].as_f64(), Some(0.37));

    // Resumed as the work account: Claude Code restored the total so far
    // and adds to it, so its figure can't be divided. Each part is its own
    // responses at list prices instead.
    h.append(
        A,
        &[
            Lines::user("go on", A, 1000.0),
            Lines::assistant("m2", "r2", A)
                .usage(7, 3)
                .at(1010.0)
                .line(),
            Lines::assistant("m3", "r3", A)
                .usage(3, 1)
                .at(1020.0)
                .line(),
        ],
    );
    let mut resumed = h.observation_for(A, &work);
    resumed.process_started_at = Some(CloudFixture::at(900.0));
    resumed.last_activity_at = CloudFixture::at(1010.0);
    resumed.cost_usd = Some(0.52);
    h.observe_one(resumed);
    h.sync_now();
    let last = h.sync_requests().last().cloned().unwrap();
    let rows: BTreeMap<String, Value> = Harness::sessions(&last)
        .into_iter()
        .map(|s| (s["accountKey"].as_str().unwrap_or("").to_owned(), s))
        .collect();
    let mine = &rows[CloudFixture::ACCOUNT_KEY];
    let theirs = &rows[CloudFixture::WORK_ACCOUNT_KEY];
    // Opus 4.5: 100 in and 10 out; 10 in and 4 out.
    assert_eq!(mine["costUsd"].as_f64(), Some(0.00075));
    assert_eq!(theirs["costUsd"].as_f64(), Some(0.00015));
    // Tokens stay split exactly.
    assert_eq!(
        mine["tokens"],
        json!({"input": 100, "output": 10, "cacheCreation": 0, "cacheRead": 0})
    );
    assert_eq!(
        theirs["tokens"],
        json!({"input": 10, "output": 4, "cacheCreation": 0, "cacheRead": 0})
    );
}

// ---- At most five requests a pass ----

#[test]
fn a_pass_sends_at_most_five_requests_and_the_rest_soon_after() {
    assert_eq!(CloudSyncPass::MAX_REQUESTS_PER_PASS, 5);
    let h = started(HarnessOptions::default());
    // 3,000 readings: six requests of 500.
    for index in 0..3_000u32 {
        h.service.record_usage(UsageObservation {
            identity: IdentityId::from(CloudFixture::IDENTITY_ID),
            source: UsageSource::Probe,
            observed_at: CloudFixture::at(f64::from(index)),
            windows: vec![("session".into(), f64::from(index % 100) + 0.5, None)],
        });
    }
    assert_eq!(h.pending_usage(), 3_000);
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 5);
    let state = h.service.state();
    assert_eq!(state.pending_usage, 500);
    assert_eq!(state.last_error, None);
    // Not before the next pass, half a minute on.
    h.tick();
    assert_eq!(h.sync_requests().len(), 5);
    h.advance(SOON_DELAY + Duration::from_secs(1));
    h.tick();
    assert_eq!(h.sync_requests().len(), 6);
    assert_eq!(h.service.state().pending_usage, 0);
}

// ---- The install secret is made when a project key is needed ----

#[test]
fn the_install_secret_is_made_only_when_a_sync_needs_a_project_key() {
    // Signed out, sync off: launched and ticking, no secret.
    let idle = started(HarnessOptions {
        signed_in: false,
        sync_on: false,
        ..HarnessOptions::default()
    });
    idle.tick();
    assert!(!idle.support().join(install_secret::FILE_NAME).exists());

    // Signed in with sync on, nothing to send yet: still none.
    let mut h = started(HarnessOptions::default());
    let file = h.support().join(install_secret::FILE_NAME);
    h.service.record_usage(h.usage());
    h.tick();
    assert_eq!(h.sync_requests().len(), 1);
    assert!(!file.exists());

    // The first session to send: made, private, and kept.
    h.write_session(A, 0);
    h.observe_one(h.observation(A));
    h.sync_now();
    let secret = std::fs::read(&file).expect("the secret");
    assert_eq!(secret.len(), install_secret::LENGTH);
    if cfg!(unix) {
        assert_eq!(agentnotch_engine::testkit::mode_of(&file), Some(0o600));
    }
    let with_session = h.sync_requests().last().cloned().unwrap();
    let path = keys::project_path(
        "/Users/me/code/app",
        &h.handles.roots.home,
        &*h.platform.files,
    );
    assert_eq!(
        Harness::sessions(&with_session)[0]["project"]["key"],
        json!(keys::project_key(CloudFixture::ACCOUNT_KEY, &path, &secret))
    );

    h.relaunch();
    assert_eq!(h.service.stores().unwrap().secret(), secret);
}

// ---- Turning summaries off deletes the ones never sent ----

/// A session seen running, synced (so its totals are known), gone, and then
/// summarised once it has been quiet for ten minutes. Not synced after that.
fn summarised_session(h: &Harness, id: &str) {
    h.write_session(id, 0);
    h.observe_one(h.observation(id));
    h.sync_now();
    h.observe(Vec::new(), &[], &[]);
    h.advance(Duration::from_secs(61));
    h.tick();
    h.advance(Duration::from_secs(11 * 60));
    h.answer_summary(SUMMARY_ANSWER);
    h.summarize_next();
    let stores = h.service.stores().unwrap();
    assert!(
        eventually(Duration::from_secs(10), || stores
            .summaries
            .summary(&key(id))
            .is_some()),
        "{id} was summarised"
    );
}

#[test]
fn turning_summaries_off_deletes_only_the_ones_never_sent() {
    let h = started(HarnessOptions {
        summaries_on: true,
        ..HarnessOptions::default()
    });
    let stores = h.service.stores().unwrap();

    // A's summary was sent; C's was made but never sent.
    summarised_session(&h, A);
    h.sync_now();
    let sent_a = h.sync_requests().last().cloned().unwrap();
    assert!(Harness::sessions(&sent_a)[0]["summary"].is_object());
    summarised_session(&h, C);
    assert_eq!(h.service.state().summarized_sessions, 2);

    h.service.set_summaries(false, h.now());
    assert!(stores.summaries.summary(&key(A)).is_some());
    assert!(stores.summaries.summary(&key(C)).is_none());
    assert_eq!(h.service.state().summarized_sessions, 1);

    // Signing out turns them off too.
    h.service.set_summaries(true, h.now());
    summarised_session(&h, D);
    h.service.sign_out(h.now());
    assert!(stores.summaries.summary(&key(D)).is_none());
    assert!(stores.summaries.summary(&key(A)).is_some());
}
