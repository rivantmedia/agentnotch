//! The sync service's consent switches, website binding and Windows
//! sign-in (the Mac's `CloudSyncTests` "Signing in and out", "The website"
//! and "Sealed" parts, `CloudSyncRegressionTests` findings 17 and 20), plus
//! the pending gate that replaces the Mac's sign-in sheet, and the
//! `an-cloud` thread's handle. Nothing here reaches a real website.

mod cloud_support;

use agentnotch_engine::cloud::api::{ApiError, CloudApi};
use agentnotch_engine::cloud::auth::{pkce, Auth, AuthError, MemorySessionStore};
use agentnotch_engine::cloud::service::{
    self, Generations, NOT_A_SIGN_IN_LINK, NO_SIGN_IN_PENDING, SIGN_IN_EXPIRED, SIGN_IN_TIMEOUT,
};
use agentnotch_engine::cloud::website;
use agentnotch_engine::cloud::CloudService;
use agentnotch_engine::hub::DeepLinkOutcome;
use agentnotch_engine::model::CloudAuthState;
use agentnotch_engine::platform::{Browser, HttpRequest};
use agentnotch_engine::runtime_types::{ClaudeBinary, CloudCall};
use agentnotch_engine::testkit::http::{body_json, header, path_of};
use cloud_support::*;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

const CALLBACK: &str = "agentnotch://auth-callback?code=the-code";

fn signed_in_as(email: &str) -> CloudAuthState {
    CloudAuthState::SignedIn {
        email: Some(email.into()),
    }
}

fn host_of(request: &HttpRequest) -> String {
    url::Url::parse(&request.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn query_value(link: &str, name: &str) -> Option<String> {
    url::Url::parse(link)
        .ok()?
        .query_pairs()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.into_owned())
}

fn signed_out() -> HarnessOptions {
    HarnessOptions {
        signed_in: false,
        ..HarnessOptions::default()
    }
}

/// A browser that can't open anything.
struct NoBrowser;

impl Browser for NoBrowser {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("The sign-in window couldn't open.".into())
    }
}

// ---- Signing in and out ----

#[test]
fn signing_in_goes_through_the_websites_supabase() {
    let h = Harness::with(signed_out());
    h.start();
    h.service.sign_in(h.now()).unwrap();
    assert_eq!(h.service.state().auth, CloudAuthState::SigningIn);
    let opened = h.opened();
    assert_eq!(opened.len(), 1);
    let authorize = url::Url::parse(&opened[0]).unwrap();
    assert_eq!(authorize.host_str(), Some("abcdefghijklmnop.supabase.co"));
    assert_eq!(
        query_value(&opened[0], "redirect_to").as_deref(),
        Some("agentnotch://auth-callback")
    );
    assert_eq!(
        query_value(&opened[0], "code_challenge_method").as_deref(),
        Some("s256")
    );

    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInCompleted
    );
    let state = h.service.state();
    assert_eq!(state.auth, signed_in_as("me@example.com"));
    // A new sign-in starts with sync off, for the user to turn on (finding 17).
    assert!(!state.sync_enabled && !state.summaries_enabled);
    assert_eq!(h.deps.setting("cloudSyncEnabled"), Some(json!(false)));
    assert_eq!(h.deps.setting("cloudSummariesEnabled"), Some(json!(false)));
    assert!(!h.service.can_upload());
    assert_eq!(
        state.dashboard_url.as_deref(),
        Some("https://agentnotch.example.com/dashboard")
    );
    assert_eq!(
        state.pools_url.as_deref(),
        Some("https://agentnotch.example.com/dashboard/pools")
    );
    assert_eq!(
        state.settings_url.as_deref(),
        Some("https://agentnotch.example.com/settings")
    );
    assert_eq!(
        h.paths(),
        ["/api/app/v1/config", "/auth/v1/token", "/api/app/v1/me"]
    );
    let token = &h.requests_to("/auth/v1/token")[0];
    assert_eq!(token.url.split('?').nth(1), Some("grant_type=pkce"));
    assert_eq!(body_json(token)["auth_code"], json!("the-code"));
    assert_eq!(
        h.saved_session().map(|s| s.website_url).as_deref(),
        Some(AuthFixture::WEBSITE)
    );

    h.service.sign_out(h.now());
    assert_eq!(h.service.state().auth, CloudAuthState::SignedOut);
    assert_eq!(h.service.state().dashboard_url, None);
    assert_eq!(h.requests_to("/auth/v1/logout").len(), 1);
    assert!(h.saved_session().is_none());
    assert!(!h.support().join("cloud-session.json").exists());
}

#[test]
fn a_failed_or_closed_sign_in_says_so() {
    let h = Harness::with(signed_out());
    h.start();
    // Closed: Windows can't tell, so the user cancels (or it times out).
    h.service.sign_in(h.now()).unwrap();
    h.service.cancel_sign_in(h.now());
    let state = h.service.state();
    assert!(state.auth == CloudAuthState::SignedOut && state.last_error.is_none());

    h.service.sign_in(h.now()).unwrap();
    let outcome = h.service.deep_link(
        "agentnotch://auth-callback?error=access_denied&error_description=Not+allowed",
        h.now(),
    );
    assert_eq!(
        outcome,
        DeepLinkOutcome::SignInIgnored("Not allowed".into())
    );
    assert_eq!(
        h.service.state().auth,
        CloudAuthState::Error {
            message: "Not allowed".into()
        }
    );
    assert_eq!(h.service.state().last_error.as_deref(), Some("Not allowed"));
    assert!(h.requests_to("/auth/v1/token").is_empty());

    // A browser that can't open: a failed sign-in with its reason.
    let h = Harness::with(HarnessOptions {
        browser: Some(Arc::new(NoBrowser)),
        ..signed_out()
    });
    h.start();
    let error = h.service.sign_in(h.now()).unwrap_err();
    assert_eq!(error, "The sign-in window couldn't open.");
    assert_eq!(
        h.service.state().auth,
        CloudAuthState::Error {
            message: error.clone()
        }
    );
    // Nothing waits for a callback.
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert!(h.requests_to("/auth/v1/token").is_empty());
}

// ---- The website ----

#[test]
fn the_builds_website_is_used() {
    let h = Harness::with(signed_out());
    h.start();
    let state = h.service.state();
    assert_eq!(state.website_url.as_deref(), Some(AuthFixture::WEBSITE));
    assert!(!state.website_is_overridden);
    assert_eq!(h.sign_in(), DeepLinkOutcome::SignInCompleted);
    assert_eq!(
        h.requests()[0].url,
        "https://agentnotch.example.com/api/app/v1/config"
    );
    assert_eq!(
        h.saved_session().map(|s| s.website_url).as_deref(),
        Some(AuthFixture::WEBSITE)
    );

    // Kept as the app keeps any address: lowercase, no trailing slash.
    let untidy = Harness::with(HarnessOptions {
        website: Some(" https://AgentNotch.example.com/ ".into()),
        ..signed_out()
    });
    untidy.start();
    assert_eq!(
        untidy.service.state().website_url.as_deref(),
        Some(AuthFixture::WEBSITE)
    );
}

/// On Windows the build's website comes from `app-config.json`, compiled
/// into the app (`website::from_app_config`), and reaches the service as
/// `CloudConfig.website`; a blank or refused address is no website.
#[test]
fn the_website_comes_from_the_app_config() {
    let from = |text: &str| website::from_app_config(text);
    assert_eq!(
        from(r#"{"websiteURL":"https://agentnotch.example.com"}"#).as_deref(),
        Some(AuthFixture::WEBSITE)
    );
    assert_eq!(from(r#"{"websiteURL":""}"#), None);
    assert_eq!(from(r#"{"websiteURL":42}"#), None);
    assert_eq!(from(r#"{"other":"https://a.example"}"#), None);
    assert_eq!(from("not json"), None);

    let h = Harness::with(HarnessOptions {
        website: from(r#"{"websiteURL":"https://agentnotch.example.com"}"#),
        ..signed_out()
    });
    h.start();
    assert_eq!(h.service.effective_website(), Some(AuthFixture::WEBSITE));
    assert_eq!(
        h.service.state().website_url.as_deref(),
        Some(AuthFixture::WEBSITE)
    );
}

/// `AGENTNOTCH_WEB_URL` (a development server) wins over the build's, and
/// says so; one the app doesn't accept counts for nothing.
#[test]
fn a_development_runs_override_wins() {
    let h = Harness::with(HarnessOptions {
        website_override: Some("http://localhost:3000".into()),
        ..signed_out()
    });
    h.start();
    let state = h.service.state();
    assert_eq!(state.website_url.as_deref(), Some("http://localhost:3000"));
    assert!(state.website_is_overridden);
    assert_eq!(h.sign_in(), DeepLinkOutcome::SignInCompleted);
    assert_eq!(
        h.requests()[0].url,
        "http://localhost:3000/api/app/v1/config"
    );
    assert!(h
        .requests()
        .iter()
        .all(|r| host_of(r) != "agentnotch.example.com"));
    assert_eq!(
        h.saved_session().map(|s| s.website_url).as_deref(),
        Some("http://localhost:3000")
    );

    let refused = Harness::with(HarnessOptions {
        website_override: Some("http://agentnotch.dev.example.com".into()),
        ..signed_out()
    });
    refused.start();
    let state = refused.service.state();
    assert_eq!(state.website_url.as_deref(), Some(AuthFixture::WEBSITE));
    assert!(!state.website_is_overridden);

    // Sealed: neither counts, and nothing is asked of any website.
    let sealed = Harness::with(HarnessOptions {
        sealed: true,
        website_override: Some("http://localhost:3000".into()),
        ..HarnessOptions::default()
    });
    sealed.start();
    assert_eq!(
        sealed.service.state(),
        service::sealed_fixture(sealed.now())
    );
    assert_eq!(sealed.service.sign_in(sealed.now()), Ok(()));
    assert!(sealed.requests().is_empty() && sealed.opened().is_empty());
}

/// A build with no website (or one the app doesn't accept): no sign-in, no
/// capture, no request; a sign-in saved earlier is set aside, not deleted.
#[test]
fn with_no_website_it_never_signs_in_or_syncs() {
    for website in [
        None,
        Some(""),
        Some("http://agentnotch.example.com"),
        Some("ftp://agentnotch.example.com"),
    ] {
        let h = Harness::with(HarnessOptions {
            website: website.map(str::to_owned),
            ..HarnessOptions::default()
        });
        h.start();
        let state = h.service.state();
        assert_eq!(state.website_url, None, "{website:?}");
        assert_eq!(state.auth, CloudAuthState::SignedOut);
        assert_eq!(state.settings_url, None);
        assert_eq!(
            h.saved_session().map(|s| s.website_url).as_deref(),
            Some(AuthFixture::WEBSITE)
        );
        let error = h.service.sign_in(h.now()).unwrap_err();
        assert_eq!(error, "This build has no website to sign in to.");
        assert_eq!(
            h.service.state().last_error.as_deref(),
            Some(error.as_str())
        );
        assert!(
            h.opened().is_empty(),
            "a browser was opened with no website"
        );
        h.service
            .observe_live(h.live(CloudFixture::SESSION_A), h.now());
        h.service.record_usage(h.usage());
        h.service.tick(h.now());
        h.service.sync_now(h.now(), true);
        assert!(!h.service.can_upload());
        assert!(h.requests().is_empty());
        assert_eq!((h.ledger_count(), h.pending_usage()), (0, 0));
    }
}

/// A sign-in saved for another website than this run's (a run pointed
/// elsewhere, or a build whose website changed) is set aside: kept, never
/// ended on Supabase, never sent anywhere; a run on that website finds it
/// again, and a new sign-in to this run's website replaces it. (The Mac's
/// retired typed-website setting doesn't exist on Windows.)
#[test]
fn a_sign_in_for_another_website_is_set_aside() {
    let mut h = Harness::with(HarnessOptions {
        website: Some("https://other.example.com".into()),
        ..HarnessOptions::default()
    });
    h.start();
    let state = h.service.state();
    assert_eq!(
        state.website_url.as_deref(),
        Some("https://other.example.com")
    );
    assert_eq!(state.auth, CloudAuthState::SignedOut);
    assert!(!h.service.can_upload());
    assert_eq!(
        h.saved_session().map(|s| s.website_url).as_deref(),
        Some(AuthFixture::WEBSITE)
    );
    h.service
        .observe_live(h.live(CloudFixture::SESSION_A), h.now());
    h.service.record_usage(h.usage());
    h.service.tick(h.now());
    assert!(h.requests().is_empty());
    assert_eq!(h.ledger_count(), 0);

    // Set aside, not deleted: a run on the old website finds it again.
    h.options.website_override = Some(AuthFixture::WEBSITE.into());
    h.relaunch();
    let state = h.service.state();
    assert_eq!(state.auth, signed_in_as("me@example.com"));
    assert!(state.website_is_overridden);
    h.options.website_override = None;
    h.relaunch();
    assert_eq!(h.service.state().auth, CloudAuthState::SignedOut);
    assert!(h.requests().is_empty());

    // Signing in to the build's website replaces it; the old one hears nothing.
    assert_eq!(h.sign_in(), DeepLinkOutcome::SignInCompleted);
    assert_eq!(
        h.saved_session().map(|s| s.website_url).as_deref(),
        Some("https://other.example.com")
    );
    let state = h.service.state();
    assert!(matches!(state.auth, CloudAuthState::SignedIn { .. }) && !state.sync_enabled);
    assert!(h
        .requests()
        .iter()
        .all(|r| host_of(r) != "agentnotch.example.com"));
    assert!(h.requests_to("/auth/v1/logout").is_empty());
}

// ---- Sealed ----

#[test]
fn a_sealed_run_shows_a_fixture_and_does_nothing() {
    let h = Harness::with(HarnessOptions {
        sealed: true,
        ..HarnessOptions::default()
    });
    h.start();
    let state = h.service.state();
    assert_eq!(state, service::sealed_fixture(h.now()));
    assert_eq!(state.auth, signed_in_as("me@example.com"));
    assert_eq!(state.website_url.as_deref(), Some(AuthFixture::WEBSITE));
    assert!(state.sync_enabled && !state.summaries_enabled && !state.summaries_available);
    assert_eq!(
        state.last_sync_at_ms,
        Some(agentnotch_engine::core::time::to_ms(h.now()) - 180_000)
    );
    assert_eq!(
        state.pools_url.as_deref(),
        Some("https://agentnotch.example.com/dashboard/pools")
    );
    assert_eq!(
        state.settings_url.as_deref(),
        Some("https://agentnotch.example.com/settings")
    );
    h.service
        .observe_live(h.live(CloudFixture::SESSION_A), h.now());
    h.service.record_usage(h.usage());
    for call in [
        CloudCall::SetSync(false),
        CloudCall::SetSummaries(true),
        CloudCall::SignIn,
        CloudCall::CancelSignIn,
        CloudCall::SyncNow,
        CloudCall::SignOut,
    ] {
        assert_eq!(h.service.call(call, h.now()), Ok(()));
    }
    h.service.tick(h.now());
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    h.service.stop(h.now());
    assert_eq!(h.service.state(), service::sealed_fixture(h.now()));
    assert!(h.requests().is_empty());
    assert!(h.opened().is_empty());
    assert!(h.handles.runner.spawned().is_empty());
    assert!(h.service.stores().is_none());
    assert!(!h.support().exists());
    assert!(h.deps.written().is_empty());
}

// ---- Consent is per sign-in (finding 17) ----

#[test]
fn signing_out_turns_sync_off_and_stops_capture() {
    let h = Harness::with(HarnessOptions {
        summaries_on: true,
        ..HarnessOptions::default()
    });
    h.start();
    assert!(h.service.can_upload());
    h.service.sign_out(h.now());
    let state = h.service.state();
    assert!(!state.sync_enabled && !state.summaries_enabled);
    assert_eq!(h.deps.setting("cloudSyncEnabled"), Some(json!(false)));
    assert_eq!(h.deps.setting("cloudSummariesEnabled"), Some(json!(false)));
    h.service
        .observe_live(h.live(CloudFixture::SESSION_A), h.now());
    h.service.record_usage(h.usage());
    assert_eq!((h.ledger_count(), h.pending_usage()), (0, 0));

    // Signed in again (as anyone): sync stays off until turned on.
    assert_eq!(h.sign_in(), DeepLinkOutcome::SignInCompleted);
    let state = h.service.state();
    assert!(matches!(state.auth, CloudAuthState::SignedIn { .. }) && !state.sync_enabled);
    h.service.tick(h.now());
    h.service
        .observe_live(h.live(CloudFixture::SESSION_A), h.now());
    h.service.record_usage(h.usage());
    assert!(h.requests_to("/api/app/v1/sync").is_empty());
    assert_eq!((h.ledger_count(), h.pending_usage()), (0, 0));

    // Turned on: capture starts.
    h.service.set_sync(true, h.now());
    assert_eq!(h.deps.setting("cloudSyncEnabled"), Some(json!(true)));
    h.service
        .observe_live(h.live(CloudFixture::SESSION_A), h.now());
    h.service.record_usage(h.usage());
    assert_eq!((h.ledger_count(), h.pending_usage()), (1, 1));
    assert_eq!(h.service.state().pending_usage, 1);
}

// ---- A sign-in is bound to its sign-in state (finding 20) ----

/// The website is fixed for the run; what a sign-in is still bound to is
/// the sign-in state it started in. On Windows the callback can't arrive
/// after a sign-out (the gate is gone), so the sign-out comes while the
/// code is being traded, and again while `me` is asked.
#[test]
fn a_sign_in_finished_after_signing_out_is_thrown_away() {
    let h = Harness::with(signed_out());
    h.start();
    let service: Weak<_> = Arc::downgrade(&h.service);
    let clock = h.handles.clock.clone();
    h.handles.http.set_handler(move |request| {
        if path_of(request) == "/auth/v1/token" {
            if let Some(service) = service.upgrade() {
                // While the code is traded, the app signs out (or quits).
                service.sign_out(agentnotch_engine::platform::Clock::now(&*clock));
            }
        }
        Ok(website_answer(request))
    });
    h.service.sign_in(h.now()).unwrap();
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(service::SIGN_IN_SUPERSEDED.into())
    );
    let state = h.service.state();
    assert_eq!(state.auth, CloudAuthState::SignedOut);
    assert_eq!(state.website_url.as_deref(), Some(AuthFixture::WEBSITE));
    assert!(h.saved_session().is_none());
    // The session it made was ended on Supabase with its own token, and the
    // website was never sent it.
    let logout = h.requests_to("/auth/v1/logout");
    let last = logout.last().expect("the session was ended");
    assert_eq!(header(last, "Authorization"), Some("Bearer access-2"));
    assert!(h.requests_to("/api/app/v1/me").is_empty());
    assert!(!h.service.can_upload());

    // Signed out while `me` is asked: the adopted session is ended by the
    // sign-out, and nothing is shown as signed in.
    let h = Harness::with(signed_out());
    h.start();
    let service: Weak<_> = Arc::downgrade(&h.service);
    let clock = h.handles.clock.clone();
    h.handles.http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/me" {
            if let Some(service) = service.upgrade() {
                service.sign_out(agentnotch_engine::platform::Clock::now(&*clock));
            }
        }
        Ok(website_answer(request))
    });
    h.service.sign_in(h.now()).unwrap();
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(service::SIGN_IN_SUPERSEDED.into())
    );
    assert_eq!(h.service.state().auth, CloudAuthState::SignedOut);
    assert_eq!(h.service.state().dashboard_url, None);
    assert!(h.saved_session().is_none());
    let logout = h.requests_to("/auth/v1/logout");
    assert_eq!(
        header(logout.last().unwrap(), "Authorization"),
        Some("Bearer access-2")
    );
    assert!(!h.service.can_upload());
}

#[test]
fn a_website_is_never_sent_another_websites_token() {
    let h = Harness::with(signed_out());
    h.handles.http.set_handler(|_| {
        Ok(agentnotch_engine::testkit::http::json_response(
            200,
            contract_fixture("me.json"),
        ))
    });
    let auth = Arc::new(Auth::new(
        h.platform.http.clone(),
        Arc::new(MemorySessionStore::new(Some(AuthFixture::session(
            3600,
            h.now(),
        )))),
        h.platform.clock.clone(),
    ));
    let other = CloudApi::new(
        "https://other.example.com",
        h.platform.http.clone(),
        Some(auth.clone()),
        "9.9",
    );
    assert_eq!(
        other.me().unwrap_err(),
        ApiError::Auth(AuthError::OtherWebsite)
    );
    assert!(h.requests().is_empty());
    let own = CloudApi::new(
        AuthFixture::WEBSITE,
        h.platform.http.clone(),
        Some(auth),
        "9.9",
    );
    assert_eq!(
        own.me().unwrap().user.email.as_deref(),
        Some("me@example.com")
    );
}

// ---- The pending gate (Windows) ----

#[test]
fn a_callback_with_nothing_pending_is_ignored() {
    // Cold start: the app was launched by the link.
    let h = Harness::with(signed_out());
    h.start();
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    let state = h.service.state();
    assert_eq!(state.auth, CloudAuthState::SignedOut);
    assert_eq!(state.last_error, None);
    assert!(h.requests().is_empty());

    // Odd links never panic and change nothing.
    for link in [
        "",
        "agentnotch://",
        "agentnotch:auth-callback?code=x",
        "agentnotch://open?session=1",
        "https://auth-callback/?code=x",
        "not a url at all",
        "agentnotch://auth-callback%00?code=x",
        "\u{0}\u{1}",
    ] {
        let outcome = h.service.deep_link(link, h.now());
        assert!(
            outcome == DeepLinkOutcome::Ignored(NOT_A_SIGN_IN_LINK.into())
                || outcome == DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into()),
            "{link:?}: {outcome:?}"
        );
    }
    assert!(h.requests().is_empty());
}

#[test]
fn a_callback_is_accepted_once() {
    let h = Harness::with(signed_out());
    h.start();
    assert_eq!(h.sign_in(), DeepLinkOutcome::SignInCompleted);
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert_eq!(h.requests_to("/auth/v1/token").len(), 1);
    let state = h.service.state();
    assert_eq!(state.auth, signed_in_as("me@example.com"));
    assert_eq!(state.last_error, None);
}

#[test]
fn a_trailing_slash_in_the_callback_is_the_same_callback() {
    let h = Harness::with(signed_out());
    h.start();
    h.service.sign_in(h.now()).unwrap();
    assert_eq!(
        h.service
            .deep_link("agentnotch://auth-callback/?code=x", h.now()),
        DeepLinkOutcome::SignInCompleted
    );
    assert_eq!(
        body_json(&h.requests_to("/auth/v1/token")[0])["auth_code"],
        json!("x")
    );
}

#[test]
fn a_callback_after_ten_minutes_is_ignored() {
    let h = Harness::with(signed_out());
    h.start();
    h.service.sign_in(h.now()).unwrap();
    h.advance(SIGN_IN_TIMEOUT + Duration::from_secs(1));
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    let state = h.service.state();
    assert_eq!(state.auth, CloudAuthState::SignedOut);
    assert_eq!(state.last_error.as_deref(), Some(SIGN_IN_EXPIRED));
    assert!(h.requests_to("/auth/v1/token").is_empty());

    // The tick ends a sign-in nobody finished quietly; a callback after
    // that says it expired.
    let h = Harness::with(signed_out());
    h.start();
    h.service.sign_in(h.now()).unwrap();
    h.advance(SIGN_IN_TIMEOUT - Duration::from_secs(1));
    h.service.tick(h.now());
    assert_eq!(h.service.state().auth, CloudAuthState::SigningIn);
    h.advance(Duration::from_secs(1));
    h.service.tick(h.now());
    let state = h.service.state();
    assert!(state.auth == CloudAuthState::SignedOut && state.last_error.is_none());
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert_eq!(
        h.service.state().last_error.as_deref(),
        Some(SIGN_IN_EXPIRED)
    );
    assert!(h.requests_to("/auth/v1/token").is_empty());

    // A new sign-in clears the note.
    h.service.sign_in(h.now()).unwrap();
    assert_eq!(h.service.state().last_error, None);
}

#[test]
fn cancelling_a_sign_in_is_quiet() {
    let h = Harness::with(signed_out());
    h.start();
    h.service.sign_in(h.now()).unwrap();
    h.service.call(CloudCall::CancelSignIn, h.now()).unwrap();
    let state = h.service.state();
    assert!(state.auth == CloudAuthState::SignedOut && state.last_error.is_none());
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert!(h.requests_to("/auth/v1/token").is_empty());

    // Cancelling another sign-in while signed in keeps the sign-in there was.
    let h = Harness::new();
    h.start();
    h.service.sign_in(h.now()).unwrap();
    assert_eq!(h.service.state().auth, CloudAuthState::SigningIn);
    h.service.cancel_sign_in(h.now());
    assert_eq!(h.service.state().auth, signed_in_as("me@example.com"));
    assert_eq!(
        h.saved_session().map(|s| s.access_token).as_deref(),
        Some("access-1")
    );
}

#[test]
fn signing_in_again_while_pending_restarts() {
    let h = Harness::with(signed_out());
    h.start();
    h.service.sign_in(h.now()).unwrap();
    h.service.sign_in(h.now()).unwrap();
    let opened = h.opened();
    assert_eq!(opened.len(), 2);
    let first = query_value(&opened[0], "code_challenge").unwrap();
    let second = query_value(&opened[1], "code_challenge").unwrap();
    assert_ne!(first, second);
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInCompleted
    );
    // Only the second sign-in's verifier was ever sent: the first one's is gone.
    let tokens = h.requests_to("/auth/v1/token");
    assert_eq!(tokens.len(), 1);
    let verifier = body_json(&tokens[0])["code_verifier"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(pkce::challenge(&verifier), second);
}

#[test]
fn a_sign_in_still_asking_the_website_when_cancelled_is_dropped() {
    let h = Harness::with(signed_out());
    h.start();
    let service: Weak<_> = Arc::downgrade(&h.service);
    let clock = h.handles.clock.clone();
    h.handles.http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/config" {
            if let Some(service) = service.upgrade() {
                service.cancel_sign_in(agentnotch_engine::platform::Clock::now(&*clock));
            }
        }
        Ok(website_answer(request))
    });
    assert_eq!(h.service.sign_in(h.now()), Ok(()));
    assert!(h.opened().is_empty());
    assert_eq!(h.service.state().auth, CloudAuthState::SignedOut);
    assert_eq!(
        h.service.deep_link(CALLBACK, h.now()),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
}

#[test]
fn a_website_that_cant_be_reached_fails_the_sign_in() {
    let h = Harness::with(signed_out());
    h.start();
    h.handles.http.set_handler(|request| {
        if path_of(request) == "/api/app/v1/config" {
            Err(agentnotch_engine::platform::HttpError::Connect(
                "refused".into(),
            ))
        } else {
            Ok(website_answer(request))
        }
    });
    let error = h.service.sign_in(h.now()).unwrap_err();
    assert!(error.starts_with("Couldn't reach the website"), "{error}");
    assert!(matches!(
        h.service.state().auth,
        CloudAuthState::Error { .. }
    ));
    assert!(h.opened().is_empty());
}

// ---- Switches ----

#[test]
fn switches_are_written_through_an_core_and_a_stale_config_doesnt_undo_them() {
    let h = Harness::new();
    h.start();
    h.service.set_sync(false, h.now());
    assert_eq!(
        h.deps.written(),
        [("cloudSyncEnabled".to_owned(), json!(false))]
    );
    assert!(!h.service.state().sync_enabled);
    // an-core republishes a config from before it took the write in.
    let mut stale = h.config();
    stale.sync_enabled = true;
    h.service.update_config(stale, h.now());
    assert!(!h.service.state().sync_enabled && !h.service.can_upload());
    // Then the one with the write.
    h.service.update_config(h.config(), h.now());
    assert!(!h.service.state().sync_enabled);
    // A later change of its own (nothing pending) is taken.
    let mut later = h.config();
    later.sync_enabled = true;
    h.service.update_config(later, h.now());
    assert!(h.service.state().sync_enabled && h.service.can_upload());

    // Summaries on: dated now, so sessions that ended before are never
    // summarised.
    h.service.set_summaries(true, h.now());
    assert_eq!(
        h.service.stores().unwrap().summaries.enabled_at(),
        Some(h.now())
    );
    assert!(h.service.state().summaries_enabled);
    assert!(h.service.can_summarize(h.now()));
    h.deps
        .launching_claude
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(!h.service.can_summarize(h.now()));
}

#[test]
fn a_cmd_shim_alone_makes_summaries_unavailable() {
    let h = Harness::with(HarnessOptions {
        summaries_on: true,
        ..HarnessOptions::default()
    });
    *h.deps.claude_binary.lock().unwrap() = Some(ClaudeBinary {
        program: PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude.cmd"),
        prefix_args: Vec::new(),
        version: None,
        shim: true,
    });
    h.start();
    let state = h.service.state();
    assert!(!state.summaries_available);
    assert_eq!(
        state.last_error.as_deref(),
        Some("Session summaries need claude.exe (or Node) on this PC.")
    );
    assert!(!h.service.can_summarize(h.now()));
    assert!(h.service.can_upload());
}

#[test]
fn starting_with_summaries_on_dates_them_from_now() {
    let h = Harness::with(HarnessOptions {
        summaries_on: true,
        ..HarnessOptions::default()
    });
    h.start();
    assert_eq!(
        h.service.stores().unwrap().summaries.enabled_at(),
        Some(h.now())
    );
}

#[test]
fn a_switch_off_stops_what_runs_before_it_is_queued() {
    let generations = Generations::default();
    generations.interrupt(CloudCall::SetSync(true));
    generations.interrupt(CloudCall::SetSummaries(true));
    generations.interrupt(CloudCall::SignIn);
    generations.interrupt(CloudCall::SyncNow);
    assert_eq!(
        (
            generations.auth(),
            generations.sync(),
            generations.summary()
        ),
        (0, 0, 0)
    );
    generations.interrupt(CloudCall::SetSummaries(false));
    assert_eq!(
        (
            generations.auth(),
            generations.sync(),
            generations.summary()
        ),
        (0, 0, 1)
    );
    generations.interrupt(CloudCall::SetSync(false));
    assert_eq!(
        (
            generations.auth(),
            generations.sync(),
            generations.summary()
        ),
        (0, 1, 2)
    );
    generations.interrupt(CloudCall::CancelSignIn);
    generations.interrupt(CloudCall::SignOut);
    assert_eq!(
        (
            generations.auth(),
            generations.sync(),
            generations.summary()
        ),
        (2, 2, 3)
    );
}

#[test]
fn the_tick_notes_folder_logins_whether_or_not_sync_is_on() {
    let h = Harness::with(HarnessOptions {
        sync_on: false,
        ..HarnessOptions::default()
    });
    h.start();
    let folder = h.handles.roots.home.join(".claude");
    let login = agentnotch_engine::cloud::backfill::login(
        Some(CloudFixture::ACCOUNT_UUID),
        None,
        Some("me@example.com"),
    )
    .expect("a login");
    *h.deps.folder_logins.lock().unwrap() = Some(
        [(folder.to_string_lossy().into_owned(), login.clone())]
            .into_iter()
            .collect(),
    );
    h.service.tick(h.now());
    let logins = &h.service.stores().unwrap().folder_logins;
    assert_eq!(
        logins.since(&folder.to_string_lossy(), Some(&login)),
        Some(h.now())
    );
    assert!(h.requests().is_empty());
}

#[test]
fn urls_beside_the_dashboard() {
    assert_eq!(
        service::settings_url(Some("https://a.example/dashboard"), None).as_deref(),
        Some("https://a.example/settings")
    );
    assert_eq!(
        service::settings_url(Some("https://a.example/app/dashboard/"), None).as_deref(),
        Some("https://a.example/app/settings")
    );
    assert_eq!(
        service::settings_url(Some("https://a.example/home"), Some("https://b.example")).as_deref(),
        Some("https://b.example/settings")
    );
    assert_eq!(
        service::settings_url(Some("https://a.example/home"), None),
        None
    );
    assert_eq!(
        service::pools_url(Some("https://a.example/dashboard/")).as_deref(),
        Some("https://a.example/dashboard/pools")
    );
    assert_eq!(service::pools_url(None), None);
    assert_eq!(service::backoff(0), Duration::ZERO);
    assert_eq!(service::backoff(1), Duration::from_secs(30));
    assert_eq!(service::backoff(3), Duration::from_secs(120));
    assert_eq!(service::backoff(7), Duration::from_secs(1800));
    assert_eq!(service::backoff(u32::MAX), Duration::from_secs(1800));
}

// ---- The an-cloud thread ----

#[test]
fn the_handle_runs_the_service_on_its_thread() {
    let h = Harness::with(signed_out());
    let handle = CloudService::start(h.config(), h.deps.clone(), &h.platform);
    handle.flush();
    assert_eq!(handle.state().auth, CloudAuthState::SignedOut);
    assert_eq!(
        handle.state().website_url.as_deref(),
        Some(AuthFixture::WEBSITE)
    );

    handle.call(CloudCall::SignIn).unwrap();
    handle.flush();
    assert_eq!(handle.state().auth, CloudAuthState::SigningIn);
    assert_eq!(h.opened().len(), 1);
    assert_eq!(handle.deep_link(CALLBACK), DeepLinkOutcome::SignInCompleted);
    handle.flush();
    assert_eq!(handle.state().auth, signed_in_as("me@example.com"));
    assert!(!handle.state().sync_enabled);

    handle.call(CloudCall::SetSync(true)).unwrap();
    handle.flush();
    assert!(handle.state().sync_enabled);
    handle.observe_live(h.live(CloudFixture::SESSION_A));
    handle.record_usage(h.usage());
    handle.update_config(h.config());
    handle.flush();
    assert_eq!(handle.state().pending_usage, 1);

    handle.call(CloudCall::SignOut).unwrap();
    handle.stop();
    // Stopped after what was queued, and joined.
    assert_eq!(handle.state().auth, CloudAuthState::SignedOut);
    assert_eq!(h.requests_to("/auth/v1/logout").len(), 1);
    assert_eq!(
        handle.call(CloudCall::SyncNow),
        Err(service::STOPPED.to_owned())
    );
    handle.stop();
    handle.flush();
    // A callback after the stop finishes nothing.
    assert_eq!(
        handle.deep_link(CALLBACK),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert!(h.saved_session().is_none());
}

#[test]
fn a_sealed_handle_answers_and_does_nothing() {
    let h = Harness::with(HarnessOptions {
        sealed: true,
        ..HarnessOptions::default()
    });
    let handle = CloudService::start(h.config(), h.deps.clone(), &h.platform);
    assert_eq!(handle.state(), service::sealed_fixture(h.now()));
    assert_eq!(handle.call(CloudCall::SignIn), Ok(()));
    assert_eq!(
        handle.deep_link(CALLBACK),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    handle.flush();
    handle.stop();
    assert!(h.requests().is_empty() && h.opened().is_empty());
    assert!(!h.support().exists());
}
