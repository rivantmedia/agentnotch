//! Signing in to the website: PKCE, the Supabase exchange and refresh, the
//! 401 retry, signing out, and where the session is kept (the Mac's
//! `CloudAuthTests`). A stand-in network only.

mod cloud_support;

use agentnotch_engine::cloud::api::{ApiError, CloudApi};
use agentnotch_engine::cloud::auth::{
    self, pkce, Auth, AuthError, AuthSession, FileSessionStore, MemorySessionStore, SessionStore,
    StoreError,
};
use agentnotch_engine::platform::{
    Browser, Clock, HttpError, HttpRequest, HttpResponse, SecureFiles,
};
use agentnotch_engine::testkit::http::{
    body_json, header, json_response, path_of, query_of, FixtureHttp,
};
use agentnotch_engine::testkit::{FakeClock, RecordingBrowser, StdSecureFiles, TEST_START_MS};
use cloud_support::{contract_fixture, AuthFixture};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Rig {
    http: Arc<FixtureHttp>,
    store: Arc<MemorySessionStore>,
    clock: Arc<FakeClock>,
    auth: Arc<Auth>,
}

/// An `Auth` over a fixture network and a memory store holding `session`
/// (expiring that many seconds from the clock's start), if any.
fn make_rig(expires_in: Option<i64>) -> Rig {
    let clock = Arc::new(FakeClock::at_ms(TEST_START_MS));
    let http = Arc::new(FixtureHttp::default());
    let store = Arc::new(MemorySessionStore::new(
        expires_in.map(|s| AuthFixture::session(s, clock.now())),
    ));
    let auth = Arc::new(Auth::new(http.clone(), store.clone(), clock.clone()));
    Rig {
        http,
        store,
        clock,
        auth,
    }
}

fn error_response(status: u16, body: serde_json::Value) -> HttpResponse {
    json_response(status, body.to_string())
}

// ---- PKCE ----

#[test]
fn pkce_matches_rfc7636_appendix_b() {
    let octets: [u8; 32] = [
        116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37,
        77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
    ];
    let verifier = pkce::verifier_from_bytes(&octets);
    assert_eq!(verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(
        pkce::challenge(&verifier),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn verifiers_are_random_and_well_formed() {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "-._~".contains(c);
    let first = pkce::make_verifier().expect("randomness");
    let second = pkce::make_verifier().expect("randomness");
    assert_ne!(first, second);
    for verifier in [&first, &second] {
        assert!((43..=128).contains(&verifier.len()), "{verifier}");
        assert!(verifier.chars().all(allowed), "{verifier}");
    }
    assert!(!pkce::challenge(&first).contains('='));
}

#[test]
fn authorize_url_asks_google_with_the_challenge() {
    let url = auth::authorize_url(AuthFixture::SUPABASE, "abc").expect("a URL");
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some("abcdefghijklmnop.supabase.co"));
    assert_eq!(parsed.path(), "/auth/v1/authorize");
    let items: BTreeMap<String, String> = parsed.query_pairs().into_owned().collect();
    let expected: BTreeMap<String, String> = [
        ("provider", "google"),
        ("redirect_to", "agentnotch://auth-callback"),
        ("code_challenge", "abc"),
        ("code_challenge_method", "s256"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(items, expected);
}

#[test]
fn callbacks_give_the_code_or_the_error() {
    let code = auth::authorization_code;
    assert_eq!(
        code("agentnotch://auth-callback?code=abc-123").as_deref(),
        Ok("abc-123")
    );
    assert_eq!(
        code("agentnotch://auth-callback#code=frag").as_deref(),
        Ok("frag")
    );
    // Some browsers hand the link over with a slash after the host.
    assert_eq!(
        code("agentnotch://auth-callback/?code=slash").as_deref(),
        Ok("slash")
    );
    assert_eq!(
        code("AgentNotch://Auth-Callback?code=case").as_deref(),
        Ok("case")
    );
    assert_eq!(
        code("agentnotch://auth-callback#error=access_denied&error_description=Email+link+is+invalid+or+has+expired"),
        Err(AuthError::Provider("Email link is invalid or has expired".into()))
    );
    assert_eq!(
        code("agentnotch://auth-callback?error=access_denied"),
        Err(AuthError::Provider("access_denied".into()))
    );
    assert_eq!(
        code("agentnotch://auth-callback?error_code=otp_expired&code=late"),
        Err(AuthError::Provider("otp_expired".into()))
    );
    let long = "x".repeat(400);
    assert_eq!(
        code(&format!("agentnotch://auth-callback?error={long}")),
        Err(AuthError::Provider("x".repeat(300)))
    );
    for invalid in [
        "https://evil.example.com/auth-callback?code=x",
        "agentnotch://evil.example.com?code=x",
        "other://auth-callback?code=x",
        "agentnotch:auth-callback?code=x",
        "agentnotch://auth-callback",
        "agentnotch://auth-callback?code=",
        "not a url",
        "",
    ] {
        assert_eq!(code(invalid), Err(AuthError::InvalidCallback), "{invalid}");
    }
    // A malformed escape is someone else's text, never a crash.
    assert_eq!(
        code("agentnotch://auth-callback?x=%zz&code=ok").as_deref(),
        Ok("ok")
    );
    let values: Vec<String> = auth::form_pairs(Some("a=%zz&b=two+words&c"))
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    assert_eq!(values, ["%zz", "two words", ""]);
}

// ---- Exchange ----

#[test]
fn sign_in_exchanges_the_code_and_saves_the_session() {
    let rig = make_rig(None);
    rig.http.set_handler(|request| {
        if path_of(request) != "/auth/v1/token" {
            return Ok(json_response(404, "{}"));
        }
        Ok(AuthFixture::token_answer("access-new", "refresh-new"))
    });
    let browser = RecordingBrowser::default();
    let pending = auth::begin(&AuthFixture::config(), AuthFixture::WEBSITE).expect("begins");
    auth::open_browser(&browser, &pending).expect("opens");
    let session = rig
        .auth
        .finish("agentnotch://auth-callback?code=the-code", &pending)
        .expect("exchanged");
    assert_eq!(session.access_token, "access-new");
    assert_eq!(session.refresh_token, "refresh-new");
    assert_eq!(session.email.as_deref(), Some("me@example.com"));
    assert_eq!(session.user_id.as_deref(), Some("user-1"));
    assert_eq!(session.website_url, AuthFixture::WEBSITE);
    assert_eq!(
        session.expires_at,
        rig.clock.now() + Duration::from_secs(3600)
    );
    // Not used until the caller knows it is still wanted.
    assert!(rig.store.load().is_none() && rig.auth.current_session().is_none());
    rig.auth.adopt(session.clone());
    assert_eq!(rig.store.load(), Some(session));
    assert_eq!(
        rig.auth
            .valid_access_token_for(AuthFixture::WEBSITE)
            .as_deref(),
        Ok("access-new")
    );
    assert_eq!(
        rig.auth.valid_access_token_for("https://other.example.com"),
        Err(AuthError::OtherWebsite)
    );

    // The browser got the challenge; the exchange carried its verifier.
    let opened = browser.opened();
    assert_eq!(opened, std::slice::from_ref(&pending.authorize_url));
    let authorize = url::Url::parse(&opened[0]).unwrap();
    let challenge = authorize
        .query_pairs()
        .find(|(name, _)| name == "code_challenge")
        .map(|(_, value)| value.into_owned())
        .expect("a challenge");
    let exchanges = rig.http.requests_to("/auth/v1/token");
    assert_eq!(exchanges.len(), 1);
    let exchange = &exchanges[0];
    assert_eq!(exchange.method, "POST");
    assert_eq!(query_of(exchange).as_deref(), Some("grant_type=pkce"));
    assert_eq!(
        header(exchange, "apikey"),
        Some(AuthFixture::PUBLISHABLE_KEY)
    );
    assert_eq!(header(exchange, "Content-Type"), Some("application/json"));
    assert_eq!(header(exchange, "Authorization"), None);
    let body = body_json(exchange);
    assert_eq!(body["auth_code"], "the-code");
    let verifier = body["code_verifier"].as_str().expect("a verifier");
    assert_eq!(verifier, pending.verifier());
    assert_eq!(pkce::challenge(verifier), challenge);
}

#[test]
fn a_closed_sign_in_window_is_cancelled() {
    // Windows can't tell when a browser tab closes: a sign-in given up on
    // (cancelled, timed out, restarted) is a dropped `PendingSignIn`.
    let rig = make_rig(None);
    let pending = auth::begin(&AuthFixture::config(), AuthFixture::WEBSITE).expect("begins");
    drop(pending);
    assert!(rig.auth.current_session().is_none());
    assert!(rig.store.load().is_none() && rig.store.save_count() == 0);
    assert!(rig.http.requests().is_empty());
    assert_eq!(rig.auth.valid_access_token(), Err(AuthError::SignedOut));

    let mut config = AuthFixture::config();
    config.supabase_url = "http://evil.example.com".into();
    assert_eq!(
        auth::begin(&config, AuthFixture::WEBSITE).err(),
        Some(AuthError::BadConfig("supabaseUrl".into()))
    );
    let mut config = AuthFixture::config();
    config.supabase_publishable_key = String::new();
    assert_eq!(
        auth::begin(&config, AuthFixture::WEBSITE).err(),
        Some(AuthError::BadConfig("supabasePublishableKey".into()))
    );
    let mut config = AuthFixture::config();
    config.redirect_url = "https://evil.example.com/callback".into();
    assert_eq!(
        auth::begin(&config, AuthFixture::WEBSITE).err(),
        Some(AuthError::BadConfig(
            "redirectUrl is https://evil.example.com/callback".into()
        ))
    );
    // A local Supabase for development is fine.
    let mut config = AuthFixture::config();
    config.supabase_url = "http://127.0.0.1:54321".into();
    let local = auth::begin(&config, "http://localhost:3000").expect("local");
    assert!(local
        .authorize_url
        .starts_with("http://127.0.0.1:54321/auth/v1/authorize?"));
}

/// A browser that can't open says so.
struct BrokenBrowser;

impl Browser for BrokenBrowser {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("This app can't open the website's sign-in.".into())
    }
}

/// A browser that can't open the sign-in page fails the sign-in with its
/// reason, not as a cancellation.
#[test]
fn a_host_with_no_browser_step_says_so() {
    let pending = auth::begin(&AuthFixture::config(), AuthFixture::WEBSITE).expect("begins");
    assert_eq!(
        auth::open_browser(&BrokenBrowser, &pending),
        Err(AuthError::Provider(
            "This app can't open the website's sign-in.".into()
        ))
    );
}

// ---- Refresh ----

#[test]
fn an_expiring_token_is_refreshed_once_and_saved_first() {
    let rig = make_rig(Some(30));
    let store = rig.store.clone();
    let saved_before_answer = Arc::new(Mutex::new(Vec::new()));
    let seen = saved_before_answer.clone();
    // The refresh is held open until the test lets it go, so the second
    // caller certainly arrives while it is out.
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let (started_in, release_in) = (started.clone(), release.clone());
    rig.http.set_handler(move |request| {
        if query_of(request).as_deref() != Some("grant_type=refresh_token") {
            return Ok(json_response(404, "{}"));
        }
        started_in.store(true, Ordering::SeqCst);
        while !release_in.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        seen.lock().unwrap().push(store.save_count());
        Ok(AuthFixture::token_answer("access-2", "refresh-2"))
    });
    // Two callers at once: one request (the refresh token is single-use).
    let spawn = || {
        let auth = rig.auth.clone();
        std::thread::spawn(move || auth.valid_access_token())
    };
    let first = spawn();
    while !started.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let second = spawn();
    std::thread::sleep(Duration::from_millis(100));
    release.store(true, Ordering::SeqCst);
    let tokens = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(tokens, [Ok("access-2".into()), Ok("access-2".into())]);
    assert_eq!(rig.http.requests_to("/auth/v1/token").len(), 1);
    assert_eq!(
        body_json(&rig.http.requests()[0])["refresh_token"],
        "refresh-1"
    );
    assert_eq!(
        header(&rig.http.requests()[0], "apikey"),
        Some(AuthFixture::PUBLISHABLE_KEY)
    );
    assert_eq!(*saved_before_answer.lock().unwrap(), [0]);
    assert_eq!(
        rig.store.load().map(|s| s.refresh_token).as_deref(),
        Some("refresh-2")
    );
    assert_eq!(rig.store.save_count(), 1);
    // The user and email are kept.
    assert_eq!(
        rig.auth.current_session().and_then(|s| s.email).as_deref(),
        Some("me@example.com")
    );
    // A fresh token needs no request.
    assert_eq!(rig.auth.valid_access_token().as_deref(), Ok("access-2"));
    assert_eq!(rig.http.requests().len(), 1);
    // A minute or less left is refreshed first.
    rig.clock.advance(Duration::from_secs(3600 - 60));
    rig.http.clear_handler();
    rig.http
        .push(Ok(AuthFixture::token_answer("access-3", "refresh-3")));
    assert_eq!(rig.auth.valid_access_token().as_deref(), Ok("access-3"));
    assert_eq!(rig.http.requests().len(), 2);
}

#[test]
fn a_refresh_answer_without_the_user_keeps_who_it_was() {
    let rig = make_rig(Some(-10));
    rig.http.push(Ok(json_response(
        200,
        json!({"access_token": "a2", "refresh_token": "r2", "expires_at": 1_790_003_600})
            .to_string(),
    )));
    let fresh = rig.auth.refresh().expect("refreshed");
    assert_eq!(fresh.user_id.as_deref(), Some("user-1"));
    assert_eq!(fresh.email.as_deref(), Some("me@example.com"));
    assert_eq!(
        agentnotch_engine::core::time::to_ms(fresh.expires_at),
        1_790_003_600_000
    );
    // An answer without tokens is no session: the old one is kept.
    rig.http
        .push(Ok(json_response(200, r#"{"access_token": ""}"#)));
    assert_eq!(
        rig.auth.refresh(),
        Err(AuthError::BadResponse("no tokens".into()))
    );
    assert_eq!(
        rig.store.load().map(|s| s.refresh_token).as_deref(),
        Some("r2")
    );
}

#[test]
fn a_refused_refresh_signs_out() {
    let rig = make_rig(Some(-10));
    rig.http.push(Ok(error_response(
        400,
        json!({"error": "invalid_grant", "error_description": "Invalid Refresh Token: Already Used"}),
    )));
    assert_eq!(rig.auth.valid_access_token(), Err(AuthError::SignedOut));
    assert!(rig.store.load().is_none());
    assert!(rig.auth.current_session().is_none());
}

/// Only Supabase refusing the refresh token itself ends the session; its
/// being down doesn't.
#[test]
fn only_a_refused_refresh_token_ends_the_session() {
    let kept = [
        (503, json!({"msg": "Service Unavailable"})),
        (500, json!({})),
        (429, json!({"msg": "slow down"})),
        (
            400,
            json!({"error_code": "validation_failed", "msg": "Unsupported content type"}),
        ),
        (403, json!({"error_code": "bad_jwt", "msg": "invalid JWT"})),
    ];
    for (status, body) in kept {
        let rig = make_rig(Some(-10));
        rig.http.push(Ok(error_response(status, body.clone())));
        assert!(rig.auth.valid_access_token().is_err());
        assert_eq!(
            rig.store.load().map(|s| s.refresh_token).as_deref(),
            Some("refresh-1"),
            "{status} {body}"
        );
    }
    let refused = [
        json!({"error": "invalid_grant", "error_description": "Invalid Refresh Token: Already Used"}),
        json!({"code": 400, "error_code": "refresh_token_not_found", "msg": "Invalid Refresh Token: Refresh Token Not Found"}),
        json!({"error_code": "session_not_found", "msg": "Session from session_id claim in JWT does not exist"}),
    ];
    for body in refused {
        let rig = make_rig(Some(-10));
        rig.http.push(Ok(error_response(400, body.clone())));
        assert_eq!(
            rig.auth.valid_access_token(),
            Err(AuthError::SignedOut),
            "{body}"
        );
        assert!(rig.store.load().is_none(), "{body}");
    }
    assert!(auth::refuses_refresh_token(
        401,
        Some("refresh_token_already_used"),
        None
    ));
    assert!(!auth::refuses_refresh_token(
        503,
        Some("invalid_grant"),
        None
    ));
    assert!(!auth::refuses_refresh_token(400, None, Some("Bad request")));
    assert!(auth::refuses_refresh_token(
        400,
        None,
        Some("Invalid Refresh Token: Expired")
    ));
}

#[test]
fn a_refusal_after_a_new_sign_in_leaves_the_new_session() {
    // The user signed in again while the refused refresh was out: the new
    // session isn't the refused one.
    let rig = make_rig(Some(-10));
    let auth = rig.auth.clone();
    let now = rig.clock.now();
    rig.http.set_handler(move |_| {
        auth.adopt(AuthFixture::session_with(
            3600,
            now,
            "access-9",
            "refresh-9",
        ));
        Ok(error_response(400, json!({"error": "invalid_grant"})))
    });
    assert_eq!(rig.auth.refresh(), Err(AuthError::SignedOut));
    assert_eq!(
        rig.store.load().map(|s| s.refresh_token).as_deref(),
        Some("refresh-9")
    );
}

#[test]
fn a_network_failure_keeps_the_session() {
    let rig = make_rig(Some(-10));
    rig.http.push(Err(HttpError::Connect("offline".into())));
    assert_eq!(
        rig.auth.valid_access_token(),
        Err(AuthError::Transport("offline".into()))
    );
    assert_eq!(
        rig.store.load().map(|s| s.refresh_token).as_deref(),
        Some("refresh-1")
    );
    rig.http.push(Err(HttpError::Timeout));
    assert!(rig.auth.valid_access_token().is_err());
    assert!(rig.auth.current_session().is_some());
}

#[test]
fn a_stored_session_with_an_unusable_supabase_address_is_forgotten() {
    let rig = make_rig(None);
    let mut session = AuthFixture::session(-10, rig.clock.now());
    session.supabase_url = "http://evil.example.com".into();
    rig.store.save(&session).unwrap();
    assert_eq!(rig.auth.valid_access_token(), Err(AuthError::SignedOut));
    assert!(rig.store.load().is_none());
    assert!(rig.http.requests().is_empty());
}

#[test]
fn a_401_is_retried_once_after_a_refresh() {
    let rig = make_rig(Some(3600));
    let handler = |next_access: &'static str, next_refresh: &'static str| {
        move |request: &HttpRequest| {
            if path_of(request) == "/auth/v1/token" {
                return Ok(AuthFixture::token_answer(next_access, next_refresh));
            }
            if path_of(request) == "/api/app/v1/me" {
                if header(request, "Authorization") == Some("Bearer access-2") {
                    return Ok(json_response(200, contract_fixture("me.json")));
                }
                return Ok(json_response(401, contract_fixture("error.json")));
            }
            Ok(json_response(404, "{}"))
        }
    };
    rig.http.set_handler(handler("access-2", "refresh-2"));
    let api = CloudApi::new(
        AuthFixture::WEBSITE,
        rig.http.clone(),
        Some(rig.auth.clone()),
        "1.2.3",
    );
    let me = api.me().expect("me");
    assert_eq!(me.user.email.as_deref(), Some("me@example.com"));
    let paths: Vec<String> = rig.http.requests().iter().map(path_of).collect();
    assert_eq!(
        paths,
        ["/api/app/v1/me", "/auth/v1/token", "/api/app/v1/me"]
    );
    let first = &rig.http.requests()[0];
    assert_eq!(header(first, "Authorization"), Some("Bearer access-1"));
    assert_eq!(header(first, "Accept"), Some("application/json"));
    assert_eq!(header(first, "User-Agent"), Some("AgentNotch/1.2.3"));
    assert_eq!(header(first, "Content-Type"), None);
    assert_eq!(first.timeout, Duration::from_secs(30));
    assert_eq!(first.url, "https://agentnotch.example.com/api/app/v1/me");

    // Refused again after the refresh: not retried a second time, and the
    // session is kept.
    rig.http.set_handler(|request| {
        if path_of(request) == "/auth/v1/token" {
            return Ok(AuthFixture::token_answer("access-3", "refresh-3"));
        }
        Ok(json_response(401, contract_fixture("error.json")))
    });
    let error = api.me().expect_err("refused");
    assert_eq!(
        error,
        ApiError::Server {
            status: 401,
            code: Some("UNAUTHORIZED".into()),
            message: Some("Sign in again.".into()),
            retry_after: None
        }
    );
    assert!(error.is_unauthorized() && !error.ends_sign_in());
    assert_eq!(rig.http.requests_to("/api/app/v1/me").len(), 4);
    assert_eq!(
        rig.store.load().map(|s| s.refresh_token).as_deref(),
        Some("refresh-3")
    );
}

#[test]
fn a_retry_never_sends_another_sign_ins_token() {
    // The user signed out and in again (through another website) while the
    // request was out: its retry gets no token at all.
    let rig = make_rig(Some(3600));
    let auth = rig.auth.clone();
    let now = rig.clock.now();
    rig.http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/me" {
            let mut other = AuthFixture::session_with(3600, now, "access-x", "refresh-x");
            other.website_url = "https://other.example.com".into();
            auth.adopt(other);
            return Ok(json_response(401, contract_fixture("error.json")));
        }
        Ok(json_response(404, "{}"))
    });
    let api = CloudApi::new(
        AuthFixture::WEBSITE,
        rig.http.clone(),
        Some(rig.auth.clone()),
        "1",
    );
    let error = api.me().expect_err("no retry");
    assert_eq!(error, ApiError::Auth(AuthError::OtherWebsite));
    assert!(error.ends_sign_in());
    assert_eq!(rig.http.requests().len(), 1);

    // Signed in again through the same website: another sign-in's token
    // isn't sent either.
    let rig = make_rig(Some(3600));
    let auth = rig.auth.clone();
    rig.http.set_handler(move |request| {
        if header(request, "Authorization") == Some("Bearer access-1") {
            auth.adopt(AuthFixture::session_with(
                3600,
                now,
                "access-y",
                "refresh-y",
            ));
        }
        Ok(json_response(401, contract_fixture("error.json")))
    });
    let api = CloudApi::new(
        AuthFixture::WEBSITE,
        rig.http.clone(),
        Some(rig.auth.clone()),
        "1",
    );
    assert_eq!(
        api.me().expect_err("no retry"),
        ApiError::Auth(AuthError::SignedOut)
    );
    assert_eq!(rig.http.requests().len(), 1);
}

#[test]
fn config_needs_no_sign_in() {
    let http = Arc::new(FixtureHttp::default());
    http.set_handler(|_| Ok(json_response(200, contract_fixture("config.json"))));
    let api = CloudApi::new(AuthFixture::WEBSITE, http.clone(), None, "1");
    let config = api.config().expect("config");
    assert_eq!(config.redirect_url, "agentnotch://auth-callback");
    let first = &http.requests()[0];
    assert_eq!(header(first, "Authorization"), None);
    assert_eq!(first.method, "GET");
    assert_eq!(
        first.url,
        "https://agentnotch.example.com/api/app/v1/config"
    );
    assert_eq!(api.me().expect_err("no auth"), ApiError::NotSignedIn);

    // A failure is the contract's error, with its Retry-After.
    http.set_handler(|_| {
        let mut response = json_response(
            429,
            r#"{"error":{"code":"RATE_LIMITED","message":"Too many requests."}}"#,
        );
        response.headers.push(("retry-after".into(), "12".into()));
        Ok(response)
    });
    let error = api.config().expect_err("limited");
    assert!(error.is_rate_limited());
    assert_eq!(error.retry_after(), Some(12.0));
    http.set_handler(|_| Err(HttpError::Timeout));
    assert_eq!(
        api.config().expect_err("offline"),
        ApiError::Transport("The request timed out.".into())
    );
    http.set_handler(|_| Ok(json_response(200, "{}")));
    assert!(matches!(
        api.config().expect_err("not the contract"),
        ApiError::BadResponse(_)
    ));
}

#[test]
fn sync_sends_the_clamped_request_with_sorted_keys() {
    use agentnotch_engine::cloud::contract::{self, SyncRequest};
    let rig = make_rig(Some(3600));
    rig.http
        .set_handler(|_| Ok(json_response(200, contract_fixture("sync-response.json"))));
    let api = CloudApi::new(
        AuthFixture::WEBSITE,
        rig.http.clone(),
        Some(rig.auth.clone()),
        "1",
    )
    .with_clock(rig.clock.clone());
    let request: SyncRequest =
        serde_json::from_slice(&contract_fixture("sync-request.json")).expect("the fixture");
    let answer = api.sync(&request).expect("synced");
    assert!(answer.accepted.sessions >= 0);
    let sent = &rig.http.requests()[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.url, "https://agentnotch.example.com/api/app/v1/sync");
    assert_eq!(header(sent, "Content-Type"), Some("application/json"));
    assert_eq!(header(sent, "Authorization"), Some("Bearer access-1"));
    let expected = contract::to_json(&request.clamped(rig.clock.now()));
    assert_eq!(sent.body.as_deref(), Some(expected.as_slice()));
}

// ---- Signing out ----

#[test]
fn sign_out_ends_this_session_only_and_forgets_it() {
    let rig = make_rig(Some(3600));
    rig.http.push(Ok(json_response(500, "{}")));
    let epoch = rig.auth.sign_in_epoch();
    rig.auth.sign_out();
    let logout = &rig.http.requests()[0];
    assert_eq!(logout.method, "POST");
    assert_eq!(path_of(logout), "/auth/v1/logout");
    assert_eq!(query_of(logout).as_deref(), Some("scope=local"));
    assert_eq!(header(logout, "Authorization"), Some("Bearer access-1"));
    assert_eq!(header(logout, "apikey"), Some(AuthFixture::PUBLISHABLE_KEY));
    // Forgotten even though the website answered 500.
    assert!(rig.store.load().is_none());
    assert_eq!(rig.auth.valid_access_token(), Err(AuthError::SignedOut));
    assert!(rig.auth.sign_in_epoch() > epoch);
    // Signing out again sends nothing.
    rig.auth.sign_out();
    assert_eq!(rig.http.requests().len(), 1);
}

#[test]
fn a_set_aside_session_stays_on_disk_unused() {
    let rig = make_rig(Some(3600));
    rig.auth.set_aside();
    assert_eq!(rig.auth.valid_access_token(), Err(AuthError::SignedOut));
    assert!(rig.store.load().is_some());
    // A revoked session that was never adopted leaves the saved one alone.
    let unwanted = AuthFixture::session_with(3600, rig.clock.now(), "access-u", "refresh-u");
    rig.auth.revoke(&unwanted);
    assert_eq!(
        header(&rig.http.requests()[0], "Authorization"),
        Some("Bearer access-u")
    );
    assert_eq!(
        rig.store.load().map(|s| s.access_token).as_deref(),
        Some("access-1")
    );
}

// ---- Where the session is kept ----

fn file_store(root: &std::path::Path, allowed: bool) -> FileSessionStore {
    FileSessionStore::new(root, Arc::new(StdSecureFiles), allowed)
}

#[test]
fn the_file_store_writes_atomically_and_privately() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("support");
    let store = file_store(&root, true);
    let now = agentnotch_engine::core::time::from_ms(TEST_START_MS);
    assert!(store.load().is_none());
    store.save(&AuthFixture::session(3600, now)).unwrap();
    store
        .save(&AuthFixture::session_with(
            3600,
            now,
            "access-2",
            "refresh-2",
        ))
        .unwrap();
    let file = root.join(FileSessionStore::FILE_NAME);
    assert_eq!(store.path(), file);
    if cfg!(unix) {
        // Plain std can't read an ACL; agentnotch-win's files prove it there.
        assert!(StdSecureFiles.is_private(&file).unwrap());
    }
    assert_eq!(
        store.load().map(|s| s.refresh_token).as_deref(),
        Some("refresh-2")
    );
    // Only the file itself: no temporary file left beside it.
    let names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [FileSessionStore::FILE_NAME]);
    // A damaged file is no session.
    std::fs::write(&file, b"{\"accessToken\": 3").unwrap();
    assert!(store.load().is_none());
    store.clear();
    assert!(!file.exists());
}

#[test]
fn the_file_store_does_nothing_sealed_or_before_bootstrap() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("support");
    let now = agentnotch_engine::core::time::from_ms(TEST_START_MS);
    let store = file_store(&root, false);
    assert_eq!(
        store.save(&AuthFixture::session(60, now)),
        Err(StoreError::NotAllowed)
    );
    assert!(!root.exists());
    // A file put there by someone else isn't read either.
    let other = file_store(&root, true);
    other.save(&AuthFixture::session(60, now)).unwrap();
    assert!(store.load().is_none());
    store.clear();
    assert!(root.join(FileSessionStore::FILE_NAME).exists());

    // The service's store follows the run: sealed means closed.
    let mut cfg = agentnotch_engine::runtime_types::CloudConfig {
        support: root.clone(),
        website: Some(AuthFixture::WEBSITE.into()),
        website_is_overridden: false,
        app_version: "1".into(),
        device_name: "TEST-PC".into(),
        device_id: String::new(),
        sync_enabled: false,
        summaries_enabled: false,
        summaries_allowed_by_default: false,
        sealed: true,
        system_users: None,
        home: temp.path().to_path_buf(),
    };
    let sealed = FileSessionStore::for_config(&cfg, Arc::new(StdSecureFiles));
    assert!(sealed.load().is_none());
    cfg.sealed = false;
    let live = FileSessionStore::for_config(&cfg, Arc::new(StdSecureFiles));
    assert!(live.load().is_some());

    // An Auth over a closed store signs in for the run only.
    let clock = Arc::new(FakeClock::at_ms(TEST_START_MS));
    let closed: Arc<dyn SessionStore> = Arc::new(file_store(&temp.path().join("closed"), false));
    let auth = Auth::new(Arc::new(FixtureHttp::default()), closed, clock);
    auth.adopt(AuthFixture::session(3600, now));
    assert_eq!(auth.valid_access_token().as_deref(), Ok("access-1"));
    assert!(!temp.path().join("closed").exists());
}

#[test]
fn the_session_file_leaves_out_what_it_doesnt_know() {
    let now = agentnotch_engine::core::time::from_ms(TEST_START_MS);
    let mut session: AuthSession = AuthFixture::session(3600, now);
    session.user_id = None;
    session.email = None;
    let text = String::from_utf8(session.encode()).unwrap();
    assert_eq!(
        text,
        "{\"accessToken\":\"access-1\",\"expiresAt\":\"2026-09-21T15:13:20.000Z\",\
         \"publishableKey\":\"sb_publishable_test\",\"refreshToken\":\"refresh-1\",\
         \"supabaseUrl\":\"https://abcdefghijklmnop.supabase.co\",\
         \"websiteURL\":\"https://agentnotch.example.com\"}"
    );
    assert_eq!(AuthSession::decode(text.as_bytes()), Some(session));
    // `null` reads as unknown, as Foundation's decoder does.
    let with_nulls = text.replace("{", "{\"email\":null,\"userId\":null,");
    assert!(AuthSession::decode(with_nulls.as_bytes()).is_some());
}
