//! Cloud sync through the live hub (design §3.4, §4.11; CL§5.1, §5.9,
//! §6.1; the Mac's `ClaudeControlHub+Cloud.swift` and
//! `DesktopHostedSessionsTests.onlyRealUncertaintyIsUnsure`):
//!
//! - the cloud thread's switch writes (`Input::SetSetting`) are written by
//!   `an-core` and the config handed back to the cloud, and its published
//!   state reaches the settings page, `an:cloud` and `control status`;
//! - the ledger's batch: certain sessions with their account's key, the
//!   unplaced waiting at most `PLACEMENT_GRACE`, the unsure for nobody;
//! - consent before uploads: no `/sync` before a sign-in and sync on, and a
//!   sign-in and a sign-out turn both switches off;
//! - deep links: a callback with no sign-in waiting is ignored (smoke
//!   phase 6), a sealed hub ignores every link, banner links open the panel
//!   only for what the pages show, ten a minute;
//! - `stop()` stops the cloud: a sign-in still waiting can't complete.
//!
//! The website is `FixtureHttp` answering as the contract's fixtures; the
//! browser only records. Nothing reaches the network or the real home.

mod accounts_support;
mod cloud_support;
mod hub_support;

use accounts_support::{Home, BIIOS, BIIOS_UUID, PARAS, PARAS_UUID};
use agentnotch_engine::cloud::service::NO_SIGN_IN_PENDING;
use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::hub::cloud_view::running_batch;
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, CloudAction, CloudUrlTarget, DeepLinkOutcome, Hub, HubEvent};
use agentnotch_engine::model::{
    Attribution, CloudAuthState, IdentityId, Phase, SettingsSnapshot, PLACEMENT_GRACE,
};
use agentnotch_engine::platform::Clock;
use agentnotch_engine::runtime_types::Input;
use agentnotch_engine::testkit::http::path_of;
use cloud_support::{website_answer, AuthFixture, CloudFixture};
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

const SYNC: &str = "/api/app/v1/sync";
const CALLBACK: &str = "agentnotch://auth-callback?code=the-code";

/// A started hub whose build names the stand-in website, over a home with
/// two signed-in accounts (`~\.claude` and `~\.claude-work`).
fn hub_with_accounts(home: &Home) -> TestHub {
    home.write(".claude/sessions/1.json", "{}");
    home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
    home.write(".claude-work/sessions/1.json", "{}");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, None),
    );
    let base: PathBuf = home.roots.home.parent().expect("the root").to_path_buf();
    let hub = TestHub::over(
        &base,
        RuntimeOptions::default(),
        |cfg| cfg.website = Some(AuthFixture::WEBSITE.into()),
        |_| {},
    );
    hub.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));
    hub.hub.start().expect("the hub starts");
    hub
}

/// A started hub with the website and no accounts.
fn hub_with_website() -> TestHub {
    let hub = TestHub::with_config(
        RuntimeOptions::default(),
        |cfg| cfg.website = Some(AuthFixture::WEBSITE.into()),
        |_| {},
    );
    hub.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));
    hub.hub.start().expect("the hub starts");
    hub
}

fn cloud(hub: &TestHub, action: CloudAction, on: Option<bool>) -> Result<Value, String> {
    hub.hub
        .call(Call::Cloud { action, on })
        .map_err(|e| e.to_string())
}

fn settings(hub: &TestHub) -> SettingsSnapshot {
    serde_json::from_value(hub.hub.call(Call::Settings).expect("settings")).expect("its shape")
}

fn sync_requests(hub: &TestHub) -> usize {
    hub.handles.http.requests_to(SYNC).len()
}

fn requests_to(hub: &TestHub, path: &str) -> usize {
    hub.handles
        .http
        .requests()
        .iter()
        .filter(|r| path_of(r) == path)
        .count()
}

/// `control status` as the pages were last shown it, after letting the
/// coalesced publishing go on (its gap runs on the fake clock).
fn status(hub: &TestHub) -> agentnotch_proto::ControlStatus {
    hub.handles.clock.advance(Duration::from_millis(100));
    hub.sync();
    hub.hub.control_status()
}

/// Signs in as a user would: "Sign in" opens the browser, the website
/// calls back.
fn sign_in(hub: &TestHub) {
    cloud(hub, CloudAction::SignIn, None).expect("the sign-in starts");
    assert!(eventually(|| !hub.handles.browser.opened().is_empty()));
    assert!(eventually(
        || settings(hub).cloud.auth == CloudAuthState::SigningIn
    ));
    assert_eq!(
        hub.hub.handle_deep_link(CALLBACK),
        DeepLinkOutcome::SignInCompleted
    );
    assert!(eventually(|| status(hub).cloud == "signed_in"));
}

// ---- the cloud thread's settings ----

/// `SetSetting` from the cloud thread (`cloudSyncEnabled`) is written by
/// `an-core` (on a file lane, with the device id it minted) and
/// republished: the settings page, `an:cloud`, and the cloud's own config,
/// which follows a later write too.
#[test]
fn the_cloud_threads_switch_is_written_by_an_core_and_republished() {
    let hub = hub_with_website();
    // Nothing is written at start: the device id waits for a real write.
    assert!(hub.files.writes_of("control-settings.json").is_empty());

    cloud(&hub, CloudAction::SetSync, Some(true)).expect("queued");
    assert!(eventually(
        || hub.settings_file()["cloudSyncEnabled"] == json!(true)
    ));
    let writes = hub.files.writes_of("control-settings.json");
    assert!(
        writes.iter().all(|w| w.thread.starts_with("an-io-")),
        "{writes:?}"
    );
    let device = hub.settings_file()["cloudDeviceId"].clone();
    let device = device.as_str().expect("a device id is saved with it");
    assert!(uuid::Uuid::parse_str(device).is_ok(), "{device}");
    assert_eq!(device, device.to_uppercase());

    assert!(eventually(|| settings(&hub).cloud.sync_enabled));
    assert!(eventually(|| {
        status(&hub);
        hub.events().iter().any(|(_, e)| {
            matches!(e, HubEvent::Cloud(state) if state.sync_enabled
                && state.website_url.as_deref() == Some(AuthFixture::WEBSITE))
        })
    }));

    // Another writer's change reaches the cloud through its config.
    hub.inputs.send(Input::SetSetting {
        key: "cloudSyncEnabled".into(),
        value: json!(false),
    });
    assert!(eventually(|| !settings(&hub).cloud.sync_enabled));
    assert!(eventually(
        || hub.settings_file()["cloudSyncEnabled"] == json!(false)
    ));
    // The device id stays what it was.
    assert_eq!(hub.settings_file()["cloudDeviceId"], json!(device));
}

/// Sync already on in a settings file that lost its device id (edited by
/// hand): the id minted at start is written at once, so nothing is ever sent
/// under an id that wasn't saved.
#[test]
fn a_device_id_minted_while_sync_is_on_is_saved_at_once() {
    let hub = TestHub::with_config(
        RuntimeOptions::default(),
        |cfg| cfg.website = Some(AuthFixture::WEBSITE.into()),
        |_| {},
    );
    std::fs::create_dir_all(&hub.roots.support).unwrap();
    std::fs::write(hub.settings_path(), br#"{"cloudSyncEnabled": true}"#).unwrap();
    hub.hub.start().expect("the hub starts");
    assert!(eventually(
        || hub.settings_file()["cloudDeviceId"].is_string()
    ));
    let device = hub.settings_file()["cloudDeviceId"].clone();
    assert!(
        uuid::Uuid::parse_str(device.as_str().unwrap()).is_ok(),
        "{device}"
    );
    assert_eq!(hub.settings_file()["cloudSyncEnabled"], json!(true));
}

/// A switch call without its value is refused; cancelling with no sign-in
/// waiting does nothing; a link the website hasn't given is `not_found`.
#[test]
fn calls_are_checked_and_cancel_only_cancels_a_waiting_sign_in() {
    let hub = hub_with_website();
    let error = hub
        .hub
        .call(Call::Cloud {
            action: CloudAction::SetSummaries,
            on: None,
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    cloud(&hub, CloudAction::CancelSignIn, None).expect("a no-op");
    hub.sync();
    assert_eq!(settings(&hub).cloud.auth, CloudAuthState::SignedOut);
    assert_eq!(settings(&hub).cloud.last_error, None);
    let error = hub
        .hub
        .call(Call::CloudUrl {
            target: CloudUrlTarget::Dashboard,
        })
        .unwrap_err();
    assert_eq!(error.code, "not_found");

    // While a sign-in waits, Cancel ends it, quietly.
    cloud(&hub, CloudAction::SignIn, None).expect("the sign-in starts");
    assert!(eventually(
        || settings(&hub).cloud.auth == CloudAuthState::SigningIn
    ));
    cloud(&hub, CloudAction::CancelSignIn, None).expect("cancelled");
    assert!(eventually(
        || settings(&hub).cloud.auth == CloudAuthState::SignedOut
    ));
    // Its callback, arriving late, finds nothing waiting.
    assert_eq!(
        hub.hub.handle_deep_link(CALLBACK),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert_eq!(requests_to(&hub, "/auth/v1/token"), 0);
}

// ---- consent before uploads ----

/// Nothing reaches `/sync` before a sign-in and sync on (sync on while
/// signed out, and "Sync now", send nothing); the sign-in turns both
/// switches off; sync on then sends; the sign-out turns both off again and
/// ends the session on the website. The dashboard's links come from it.
#[test]
fn nothing_is_uploaded_before_a_sign_in_and_sync_on() {
    let home = Home::new();
    let hub = hub_with_accounts(&home);
    assert_eq!(settings(&hub).accounts.len(), 2);
    // Smoke phase 5: a started cloud leaves no file behind before a sign-in
    // but the record of since when each folder has been signed in as its
    // account, which is local only and kept whether or not sync is on (the
    // Mac's CloudSync.tick: a later backfill needs it).
    let cloud_files = || {
        std::fs::read_dir(&hub.roots.support)
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with("cloud-") && name != "cloud-folder-logins.json")
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    assert_eq!(cloud_files(), Vec::<String>::new());

    cloud(&hub, CloudAction::SetSync, Some(true)).expect("queued");
    cloud(&hub, CloudAction::SetSummaries, Some(true)).expect("queued");
    cloud(&hub, CloudAction::SyncNow, None).expect("queued");
    assert!(eventually(
        || hub.settings_file()["cloudSyncEnabled"] == json!(true)
    ));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sync_requests(&hub), 0, "signed out: nothing is sent");

    sign_in(&hub);
    // A sign-in starts with both switches off (consent is per sign-in).
    assert!(eventually(|| {
        let file = hub.settings_file();
        file["cloudSyncEnabled"] == json!(false) && file["cloudSummariesEnabled"] == json!(false)
    }));
    let state = settings(&hub).cloud;
    assert!(!state.sync_enabled && !state.summaries_enabled);
    cloud(&hub, CloudAction::SyncNow, None).expect("queued");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sync_requests(&hub), 0, "sync off: nothing is sent");
    let dashboard = hub
        .hub
        .call(Call::CloudUrl {
            target: CloudUrlTarget::Dashboard,
        })
        .expect("the website said where its dashboard is");
    assert!(dashboard["url"]
        .as_str()
        .is_some_and(|url| url.starts_with(AuthFixture::WEBSITE)));

    cloud(&hub, CloudAction::SetSync, Some(true)).expect("queued");
    assert!(eventually(|| settings(&hub).cloud.sync_enabled));
    // A reading taken in from now on is recorded (Claude Code's cache,
    // read by the next 20-second cycle), and sent.
    let fetched = agentnotch_engine::core::time::to_ms(hub.handles.clock.now());
    home.write_json(
        ".claude-work/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, Some(fetched as f64)),
    );
    assert!(eventually(|| {
        hub.handles.clock.advance(Duration::from_secs(5));
        hub.sync();
        settings(&hub).cloud.pending_usage > 0
    }));
    cloud(&hub, CloudAction::SyncNow, None).expect("queued");
    assert!(eventually(|| sync_requests(&hub) > 0));
    assert_eq!(status(&hub).cloud, "signed_in");
    assert!(eventually(|| status(&hub).sync));

    cloud(&hub, CloudAction::SignOut, None).expect("queued");
    assert!(eventually(|| status(&hub).cloud == "signed_out"));
    assert!(eventually(|| requests_to(&hub, "/auth/v1/logout") == 1));
    assert!(eventually(|| {
        let file = hub.settings_file();
        file["cloudSyncEnabled"] == json!(false) && file["cloudSummariesEnabled"] == json!(false)
    }));
    assert!(!hub.roots.support.join("cloud-session.json").exists());
    let sent = sync_requests(&hub);
    cloud(&hub, CloudAction::SyncNow, None).expect("queued");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sync_requests(&hub), sent, "signed out again: nothing more");
}

/// Signed in with sync on and a pass on the cloud thread held in its
/// `/sync` request until the returned sender lets it go (or drops).
fn hub_held_in_a_pass(home: &Home) -> (TestHub, crossbeam_channel::Sender<()>) {
    let hub = hub_with_accounts(home);
    sign_in(&hub);
    cloud(&hub, CloudAction::SetSync, Some(true)).expect("queued");
    cloud(&hub, CloudAction::SetSummaries, Some(true)).expect("queued");
    assert!(eventually(|| {
        let file = hub.settings_file();
        file["cloudSyncEnabled"] == json!(true) && file["cloudSummariesEnabled"] == json!(true)
    }));
    assert!(hub.roots.support.join("cloud-session.json").exists());
    let fetched = agentnotch_engine::core::time::to_ms(hub.handles.clock.now());
    home.write_json(
        ".claude-work/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, Some(fetched as f64)),
    );
    assert!(eventually(|| {
        hub.handles.clock.advance(Duration::from_secs(5));
        hub.sync();
        settings(&hub).cloud.pending_usage > 0
    }));
    let (release, gate) = crossbeam_channel::bounded::<()>(0);
    let (entered, held) = crossbeam_channel::unbounded::<()>();
    hub.handles.http.set_handler(move |request| {
        if path_of(request) == SYNC {
            let _ = entered.send(());
            let _ = gate.recv_timeout(Duration::from_secs(20));
        }
        Ok(website_answer(request))
    });
    cloud(&hub, CloudAction::SyncNow, None).expect("queued");
    held.recv_timeout(Duration::from_secs(10))
        .expect("the pass sends its request");
    (hub, release)
}

/// Sync turned off while a pass holds the cloud thread, then a quit: the
/// switch is saved off at once (by `an-core`, before the call's turn), so
/// the next launch doesn't sync again. The summaries switch stays as it
/// was (the Mac's `setSyncEnabled`).
#[test]
fn sync_off_is_saved_even_when_the_app_quits_during_a_pass() {
    let home = Home::new();
    let (hub, release) = hub_held_in_a_pass(&home);
    cloud(&hub, CloudAction::SetSync, Some(false)).expect("queued");
    hub.hub.stop();
    let file = hub.settings_file();
    assert_eq!(file["cloudSyncEnabled"], json!(false));
    assert_eq!(file["cloudSummariesEnabled"], json!(true));
    drop(release);
}

/// The same for summaries off.
#[test]
fn summaries_off_is_saved_even_when_the_app_quits_during_a_pass() {
    let home = Home::new();
    let (hub, release) = hub_held_in_a_pass(&home);
    cloud(&hub, CloudAction::SetSummaries, Some(false)).expect("queued");
    hub.hub.stop();
    let file = hub.settings_file();
    assert_eq!(file["cloudSummariesEnabled"], json!(false));
    assert_eq!(file["cloudSyncEnabled"], json!(true));
    drop(release);
}

/// A sign-out while a pass holds the cloud thread, then a quit: both
/// switches are saved off and the session's file is gone at once, so the
/// next launch is signed out. The `/logout` is still sent once the cloud
/// thread gets to the call.
#[test]
fn a_sign_out_holds_even_when_the_app_quits_during_a_pass() {
    let home = Home::new();
    let (hub, release) = hub_held_in_a_pass(&home);
    cloud(&hub, CloudAction::SignOut, None).expect("queued");
    assert!(!hub.roots.support.join("cloud-session.json").exists());
    hub.hub.stop();
    let file = hub.settings_file();
    assert_eq!(file["cloudSyncEnabled"], json!(false));
    assert_eq!(file["cloudSummariesEnabled"], json!(false));
    assert!(!hub.roots.support.join("cloud-session.json").exists());
    assert_eq!(requests_to(&hub, "/auth/v1/logout"), 0);
    drop(release);
    assert!(eventually(|| requests_to(&hub, "/auth/v1/logout") == 1));
    assert!(!hub.roots.support.join("cloud-session.json").exists());
}

/// Summaries turned on while a pass holds the cloud thread, and the pass
/// let go only once the quit has stopped `an-core` (the hook pipe is
/// closed): the cloud writes the switch as it stops, after `an-core` took
/// its last input, and the stop still saves it.
#[test]
fn a_switch_the_cloud_writes_as_the_app_quits_is_saved() {
    let home = Home::new();
    let hub = hub_with_accounts(&home);
    sign_in(&hub);
    cloud(&hub, CloudAction::SetSync, Some(true)).expect("queued");
    assert!(eventually(
        || hub.settings_file()["cloudSyncEnabled"] == json!(true)
    ));
    let fetched = agentnotch_engine::core::time::to_ms(hub.handles.clock.now());
    home.write_json(
        ".claude-work/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, Some(fetched as f64)),
    );
    assert!(eventually(|| {
        hub.handles.clock.advance(Duration::from_secs(5));
        hub.sync();
        settings(&hub).cloud.pending_usage > 0
    }));
    let (entered, held) = crossbeam_channel::unbounded::<()>();
    let transport = hub.handles.transport.clone();
    hub.handles.http.set_handler(move |request| {
        if path_of(request) == SYNC {
            let _ = entered.send(());
            let until = std::time::Instant::now() + Duration::from_secs(20);
            while !transport.is_stopped() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(website_answer(request))
    });
    cloud(&hub, CloudAction::SyncNow, None).expect("queued");
    held.recv_timeout(Duration::from_secs(10))
        .expect("the pass sends its request");
    cloud(&hub, CloudAction::SetSummaries, Some(true)).expect("queued");
    assert_eq!(hub.settings_file()["cloudSummariesEnabled"], json!(false));
    hub.hub.stop();
    assert_eq!(hub.settings_file()["cloudSummariesEnabled"], json!(true));
    assert_eq!(hub.settings_file()["cloudSyncEnabled"], json!(true));
}

fn ids(set: &std::collections::BTreeSet<String>) -> Vec<&str> {
    set.iter().map(String::as_str).collect()
}

// ---- the ledger's batch ----

/// The hub's side of `onlyRealUncertaintyIsUnsure`: a session certain of a
/// known account is attributed with that account's key; one not placed
/// yet waits, for at most `PLACEMENT_GRACE`, and the batch says when that
/// runs out; really unsure ones and those of an account the website may
/// not hear of (hidden, forgotten) count for nobody; ended sessions aren't
/// running. Every running session is live.
#[test]
fn the_live_batch_places_sessions_with_the_grace() {
    let account = CloudFixture::account();
    let seen = CloudFixture::base();
    let soon = seen + Duration::from_secs(3);
    let late = seen + PLACEMENT_GRACE + Duration::from_secs(1);
    let view = |id: &str, attribution: Attribution| {
        let mut view = cloud_support::session_view(id);
        view.attribution = attribution;
        view.attribution_since = seen;
        view
    };
    let certain = view(
        "certain",
        Attribution::Known(Some(account.identity_id.clone())),
    );
    let ungrouped = view("ungrouped", Attribution::Known(None));
    let waiting = view("waiting", Attribution::Waiting);
    let unsure = view(
        "unsure",
        Attribution::Unsure(Some(account.identity_id.clone())),
    );
    let mut desktop = view("desktop", Attribution::Unsure(None));
    desktop.entrypoint = Some("claude-desktop".into());
    let hidden = view(
        "hidden",
        Attribution::Known(Some(IdentityId::from("uuid:someone-else"))),
    );
    let mut ended = view(
        "ended",
        Attribution::Known(Some(account.identity_id.clone())),
    );
    ended.phase = Phase::Ended;
    let views = vec![certain, ungrouped, waiting, unsure, desktop, hidden, ended];

    let (batch, due) = running_batch(&views, std::slice::from_ref(&account), soon);
    assert_eq!(batch.at, soon);
    assert_eq!(batch.attributed.len(), 1);
    assert_eq!(batch.attributed[0].session_id, "certain");
    assert_eq!(
        Some(batch.attributed[0].account_key.clone()),
        agentnotch_engine::cloud::keys::account_key_of(&account)
    );
    assert_eq!(ids(&batch.waiting), ["desktop", "ungrouped", "waiting"]);
    assert_eq!(ids(&batch.unsure), ["unsure"]);
    assert_eq!(
        ids(&batch.live_ids),
        [
            "certain",
            "desktop",
            "hidden",
            "ungrouped",
            "unsure",
            "waiting"
        ]
    );
    assert_eq!(due, Some(seen + PLACEMENT_GRACE));

    // Past the grace, not knowing is not being able to tell.
    let (batch, due) = running_batch(&views, std::slice::from_ref(&account), late);
    assert!(batch.waiting.is_empty());
    assert_eq!(
        ids(&batch.unsure),
        ["desktop", "ungrouped", "unsure", "waiting"]
    );
    assert_eq!(due, None);

    // A Desktop-hosted session whose registry entry was read can't be told.
    let mut read = view("desktop", Attribution::Unsure(None));
    read.entrypoint = Some("claude-desktop".into());
    read.registry_status = Some("busy".into());
    let (batch, _) = running_batch(&[read], &[account], soon);
    assert_eq!(ids(&batch.unsure), ["desktop"]);
}

// ---- deep links ----

/// Smoke phase 6: a callback with no sign-in waiting is ignored as "no
/// sign-in pending" (the glue logs it, shows nothing), on a running hub and
/// on one never started; a link that is neither a callback nor a banner's
/// is ignored too.
#[test]
fn a_callback_with_nothing_pending_is_ignored() {
    let hub = hub_with_website();
    assert_eq!(
        hub.hub
            .handle_deep_link("agentnotch://auth-callback?code=smoke"),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert!(matches!(
        hub.hub.handle_deep_link("agentnotch://somewhere-else"),
        DeepLinkOutcome::Ignored(_)
    ));
    assert_eq!(requests_to(&hub, "/auth/v1/token"), 0);

    let unstarted = TestHub::new();
    assert_eq!(
        unstarted
            .hub
            .handle_deep_link("agentnotch://auth-callback?code=smoke"),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
}

/// A sealed hub follows no link and its cloud calls do nothing.
#[test]
fn a_sealed_hub_ignores_links() {
    let dir = tempfile::tempdir().expect("a temporary root");
    let (platform, handles) = agentnotch_engine::testkit::platform(dir.path());
    let mut cfg = hub_support::live::config(&handles.roots);
    cfg.flags = DevFlags {
        sealed: true,
        ..DevFlags::default()
    };
    let hub = Hub::new(cfg, platform);
    hub.start().expect("the sealed hub starts");
    for link in [CALLBACK, "agentnotch://open?ring=claude-acct-1e41d94e802a"] {
        assert!(
            matches!(hub.handle_deep_link(link), DeepLinkOutcome::Ignored(_)),
            "{link}"
        );
    }
    hub.call(Call::Cloud {
        action: CloudAction::SignIn,
        on: None,
    })
    .expect("a no-op");
    assert!(handles.browser.opened().is_empty());
    assert!(handles.http.requests().is_empty());
    hub.stop();
}

/// A banner's links open the panel at a ring the pages show (never one
/// they don't, never a session they don't list) and at most ten a minute.
#[test]
fn banner_links_open_the_panel_for_what_is_shown() {
    let home = Home::new();
    let hub = hub_with_accounts(&home);
    let ring = hub.hub.snapshot().rings[0].ring_id.clone();
    let open = format!("agentnotch://open?ring={ring}");
    assert_eq!(hub.hub.handle_deep_link(&open), DeepLinkOutcome::Opened);
    assert!(hub.events().iter().any(|(_, e)| matches!(e,
        HubEvent::Panel(request) if request.ring_id.as_deref() == Some(ring.as_str())
            && request.reason == "notification" && request.route == "sessions")));
    assert!(matches!(
        hub.hub
            .handle_deep_link("agentnotch://open?ring=claude-acct-000000000000"),
        DeepLinkOutcome::Ignored(_)
    ));
    assert!(matches!(
        hub.hub.handle_deep_link("agentnotch://open?session=nobody"),
        DeepLinkOutcome::Ignored(_)
    ));
    assert!(matches!(
        hub.hub
            .handle_deep_link("agentnotch://review?session=nobody&completed=1"),
        DeepLinkOutcome::Ignored(_)
    ));
    // Four so far; six more are followed, then none for the minute.
    for _ in 0..6 {
        assert_eq!(hub.hub.handle_deep_link(&open), DeepLinkOutcome::Opened);
    }
    assert!(matches!(
        hub.hub.handle_deep_link(&open),
        DeepLinkOutcome::Ignored(_)
    ));
    hub.handles.clock.advance(Duration::from_secs(61));
    assert_eq!(hub.hub.handle_deep_link(&open), DeepLinkOutcome::Opened);
}

// ---- stop ----

/// `stop()` halts the cloud thread: a sign-in that waited can't complete
/// afterwards (no code is traded), and its calls say it stopped. A new
/// start runs a new cloud.
#[test]
fn stop_halts_the_cloud() {
    let hub = hub_with_website();
    cloud(&hub, CloudAction::SignIn, None).expect("the sign-in starts");
    assert!(eventually(
        || settings(&hub).cloud.auth == CloudAuthState::SigningIn
    ));
    hub.hub.stop();
    assert_eq!(
        hub.hub.handle_deep_link(CALLBACK),
        DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
    );
    assert_eq!(requests_to(&hub, "/auth/v1/token"), 0);
    let error = hub
        .hub
        .call(Call::Cloud {
            action: CloudAction::SyncNow,
            on: None,
        })
        .unwrap_err();
    assert_eq!(error.code, "failed");

    hub.hub.start().expect("starts again");
    cloud(&hub, CloudAction::SyncNow, None).expect("a cloud again");
    assert!(eventually(
        || settings(&hub).cloud.auth == CloudAuthState::SignedOut
    ));
    assert_eq!(sync_requests(&hub), 0);
}
