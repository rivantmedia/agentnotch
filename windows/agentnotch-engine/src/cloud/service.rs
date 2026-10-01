//! The sync service (CloudSync.swift's `CloudSync`, CL§5): the consent
//! switches, the website sign-in with Windows' pending gate, the published
//! [`CloudState`], and what capture and the schedule may do.
//!
//! [`CloudSync`] is driven with an explicit `now` and takes `&self`
//! everywhere: its state sits behind one lock that is never held across a
//! request, so the `an-cloud` thread (calls, ticks), the thread a deep link
//! arrives on and a request still out interleave the way the Mac's main
//! actor interleaves at its `await`s. The Mac's three generation counters
//! do the same job here: a sign-in, a sync pass or a summary that started
//! before a sign-out, a switch-off or a stop finds its generation moved on
//! when its request comes back, and drops what came back.
//!
//! Switches: `cloudSyncEnabled` and `cloudSummariesEnabled` have one
//! writer, `an-core`. The service asks for a change through
//! [`CloudDeps::set_setting`] and uses the new value at once; the config
//! `an-core` republishes reaches it through [`CloudSync::update_config`].

use super::api::{ApiError, CloudApi};
use super::auth::{self, Auth, AuthError, AuthSession, FileSessionStore, PendingSignIn};
use super::backfill;
use super::contract::{limit, SyncDevice, CALLBACK_HOST, CALLBACK_SCHEME};
use super::environment;
use super::files::lock;
use super::ledger::SessionOwners;
use super::pass::{CloudAccountInfo, CloudStores, CloudSyncPass, Input};
use super::recorder::RecordedUsageReading;
use super::summary::run::{self as summary_run, Cancel, Outcome, Summarizer, SHIM_REFUSED};
use super::summary::store::MAX_RETRY;
use super::summary::text::{build_excerpt, LocalNames, MAX_EXCERPT_CHARACTERS};
use super::website;
use crate::core::paths::PathStyle;
use crate::core::settings::keys;
use crate::hub::DeepLinkOutcome;
use crate::model::{CloudAuthState, CloudState};
use crate::platform::{Browser, Clock, Http, Platform};
use crate::runtime_types::{CloudCall, CloudConfig, CloudDeps, LiveBatch, UsageObservation};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---- Tuning (CloudSync.swift 669-684, CL§5.3) ----

/// A sync pass every five minutes while there is something to send.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The `an-cloud` thread's tick.
pub const TICK_INTERVAL: Duration = Duration::from_secs(20);
/// After a session ends or a summary is written.
pub const SOON_DELAY: Duration = Duration::from_secs(30);
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(30);
pub const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
pub const BACKFILL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// A rate-limited summary pauses them all this long.
pub const SUMMARY_PAUSE: Duration = Duration::from_secs(30 * 60);
/// A summary whose account no folder is signed in as now waits this long.
pub const NO_FOLDER_WAIT: Duration = Duration::from_secs(60 * 60);
/// An account whose 5-hour window is at least this used (percent) gets no
/// summaries until it comes down: they would eat into real work.
pub const SUMMARY_USAGE_CEILING: f64 = 80.0;
/// How long a sign-in waits for its callback. Windows can't tell when the
/// user closes the browser tab (the Mac's sheet says so), so a sign-in
/// nobody finishes ends by itself, quietly, instead of leaving the section
/// stuck on "Signing in".
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// A summary whose run says this login can't make one waits this long.
pub const UNAVAILABLE_WAIT: Duration = Duration::from_secs(6 * 60 * 60);
/// After a 413, the next (smaller) pass goes this soon.
pub const TOO_LARGE_RETRY: Duration = Duration::from_secs(5);
/// A 413 halves the sessions per request, never below this.
pub const MIN_SESSIONS_PER_REQUEST: usize = 10;
/// The longest wait a website's Retry-After can ask for here: a larger one
/// (or one no clock can hold) waits this long.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Shown when the sign-in ended during a sync (Supabase refused the
/// refresh token, or the session is another website's).
pub const SIGNED_OUT_OF_WEBSITE: &str = "Signed out of the website. Sign in again.";
/// Shown when the website refused a token Supabase had just refreshed.
pub const SIGN_IN_NOT_ACCEPTED: &str =
    "The website didn't accept this PC's sign-in. Trying again later.";
/// Why a summary waits: no folder is signed in as its account now.
pub const NO_FOLDER_REASON: &str = "No folder is signed in as this account now";
/// Why a summary waits: its account's part holds no conversation.
pub const NO_CONVERSATION_REASON: &str = "No conversation to summarise";
/// Why a summary is dropped: its folder was signed in as someone else by
/// the time it came back.
pub const FOLDER_CHANGED_REASON: &str = "The folder changed accounts during the summary";
pub const RATE_LIMITED_REASON: &str = "Rate limited";

/// Why a deep link changed nothing: no sign-in waits for one (the app was
/// started by the link, the sign-in was cancelled, timed out or already
/// finished, or the link isn't this sign-in's).
pub const NO_SIGN_IN_PENDING: &str = "no sign-in pending";
/// Shown when a callback comes for a sign-in that is no longer waiting.
pub const SIGN_IN_EXPIRED: &str = "Sign-in expired; try again";
/// A sign-in thrown away because the user signed out, started another or
/// the service stopped while it was out.
pub const SIGN_IN_SUPERSEDED: &str = "the sign-in was cancelled";
/// A link for something else than the website sign-in.
pub const NOT_A_SIGN_IN_LINK: &str = "not a sign-in link";
/// A call after [`CloudSync::stop`].
pub const STOPPED: &str = "Cloud sync has stopped.";

/// 30 s, doubling per failure, up to 30 minutes. Pure.
pub fn backoff(after_failures: u32) -> Duration {
    if after_failures == 0 {
        return Duration::ZERO;
    }
    let factor = 2f64.powi(after_failures.saturating_sub(1).min(16) as i32);
    Duration::from_secs_f64((INITIAL_BACKOFF.as_secs_f64() * factor).min(MAX_BACKOFF.as_secs_f64()))
}

/// What a sealed run shows (ClaudeCloudState.sealedFixture): signed in to
/// an example website, sync on, the last sync three minutes ago. Nothing
/// behind it is real.
pub fn sealed_fixture(now: SystemTime) -> CloudState {
    let website = "https://agentnotch.example.com".to_owned();
    let dashboard = format!("{website}/dashboard");
    CloudState {
        website_url: Some(website.clone()),
        website_is_overridden: false,
        auth: CloudAuthState::SignedIn {
            email: Some("me@example.com".into()),
        },
        sync_enabled: true,
        summaries_enabled: false,
        summaries_available: false,
        is_syncing: false,
        last_sync_at_ms: Some(epoch_ms(
            now.checked_sub(Duration::from_secs(180)).unwrap_or(now),
        )),
        last_error: None,
        pending_sessions: 0,
        pending_usage: 0,
        summarized_sessions: 0,
        pools_url: pools_url(Some(&dashboard)),
        settings_url: settings_url(Some(&dashboard), Some(&website)),
        dashboard_url: Some(dashboard),
    }
}

/// Sharing accounts with other people happens on the website:
/// `<dashboard>/pools`.
pub fn pools_url(dashboard: Option<&str>) -> Option<String> {
    let mut url = url::Url::parse(dashboard?).ok()?;
    url.path_segments_mut().ok()?.pop_if_empty().push("pools");
    Some(url.to_string())
}

/// The website's settings page, where synced data is removed: beside the
/// dashboard (`<site>/dashboard` -> `<site>/settings`), else under the
/// website's address; `None` with neither (ClaudeControlHub+Cloud 99-105).
pub fn settings_url(dashboard: Option<&str>, website: Option<&str>) -> Option<String> {
    if let Some(mut url) = dashboard.and_then(|d| url::Url::parse(d).ok()) {
        let last = url
            .path_segments()
            .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
            .map(str::to_owned);
        if last.as_deref() == Some("dashboard") {
            let mut segments = url.path_segments_mut().ok()?;
            segments.pop_if_empty().pop().push("settings");
            drop(segments);
            return Some(url.to_string());
        }
    }
    let site = website::validated(website)?;
    let mut url = url::Url::parse(&site).ok()?;
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .push("settings");
    Some(url.to_string())
}

fn epoch_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn clamp_u32(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A Retry-After (seconds) as a wait, at most [`MAX_RETRY_AFTER`].
fn retry_wait(retry_after: Option<f64>) -> Duration {
    retry_after
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .map_or(Duration::ZERO, |wait| wait.min(MAX_RETRY_AFTER))
}

/// Whether `link` is the website sign-in's callback
/// (`agentnotch://auth-callback…`, a trailing slash allowed). Never panics
/// on odd text: it is whatever another program put on the command line.
pub fn is_sign_in_callback(link: &str) -> bool {
    match url::Url::parse(link) {
        Ok(url) => {
            url.scheme().eq_ignore_ascii_case(CALLBACK_SCHEME)
                && url
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case(CALLBACK_HOST))
        }
        Err(_) => false,
    }
}

// ---- Generations ----

/// The Mac's `authGeneration`, `syncGeneration` and `summaryGeneration`.
/// Shared with the handle, which moves them on before it queues a call that
/// stops something, so a request in flight sends nothing more even while
/// the call waits its turn.
#[derive(Debug, Default)]
pub struct Generations {
    /// Every sign-in, sign-out and cancelled or expired sign-in.
    auth: AtomicU64,
    /// Sync turned off or signed out: a pass in flight sends nothing more.
    sync: AtomicU64,
    /// Summaries (or sync) turned off: a run still going is dropped.
    summary: AtomicU64,
    /// The summary running now, if any: moving `summary` on stops its
    /// child at once, whichever thread does it (the handle's included,
    /// while the cloud thread is busy with a pass).
    running_summary: Mutex<Option<Cancel>>,
}

impl Generations {
    pub fn auth(&self) -> u64 {
        self.auth.load(Ordering::SeqCst)
    }

    pub fn sync(&self) -> u64 {
        self.sync.load(Ordering::SeqCst)
    }

    pub fn summary(&self) -> u64 {
        self.summary.load(Ordering::SeqCst)
    }

    fn bump_auth(&self) -> u64 {
        self.auth.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn bump_sync(&self) {
        self.sync.fetch_add(1, Ordering::SeqCst);
    }

    fn bump_summary(&self) {
        self.summary.fetch_add(1, Ordering::SeqCst);
        if let Some(cancel) = lock(&self.running_summary).as_ref() {
            cancel.cancel();
        }
    }

    fn track_summary(&self, cancel: Option<Cancel>) {
        *lock(&self.running_summary) = cancel;
    }

    /// What `call` will stop, stopped now: the handle calls this before it
    /// queues the call.
    pub fn interrupt(&self, call: CloudCall) {
        match call {
            CloudCall::SignOut => {
                self.bump_auth();
                self.bump_sync();
                self.bump_summary();
            }
            CloudCall::SetSync(false) => {
                self.bump_sync();
                self.bump_summary();
            }
            CloudCall::SetSummaries(false) => self.bump_summary(),
            CloudCall::CancelSignIn => {
                self.bump_auth();
            }
            CloudCall::SignIn
            | CloudCall::SetSync(true)
            | CloudCall::SetSummaries(true)
            | CloudCall::SyncNow => {}
        }
    }
}

// ---- State ----

/// A sign-in waiting for its callback (D 1972-1980): in memory only, taken
/// once, valid for [`SIGN_IN_TIMEOUT`].
struct PendingGate {
    sign_in: PendingSignIn,
    auth_generation: u64,
    started_at: SystemTime,
    /// The config's dashboard, until `me` says otherwise.
    dashboard: String,
}

/// A callback whose code was traded for a session that nobody has used
/// yet: [`CloudSync::finish_sign_in`] adopts it or ends it on Supabase.
pub struct AcceptedSignIn {
    session: AuthSession,
    website: String,
    auth_generation: u64,
    dashboard: String,
}

impl std::fmt::Debug for AcceptedSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptedSignIn")
            .field("website", &self.website)
            .field("auth_generation", &self.auth_generation)
            .finish_non_exhaustive()
    }
}

/// When the next pass may run (the schedule's own fields, for tests and the
/// schedule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub next_sync_at: Option<SystemTime>,
    /// After a failure: no pass before this, whatever asks for one sooner.
    pub not_before: Option<SystemTime>,
    pub failures: u32,
}

struct Inner {
    started: bool,
    /// The latest config, with the switches as this service last set or
    /// accepted them.
    cfg: CloudConfig,
    /// Switch values asked of `an-core` that its republished config hasn't
    /// shown yet: an older config arriving meanwhile doesn't undo them.
    written: BTreeMap<&'static str, bool>,
    auth_state: CloudAuthState,
    dashboard_url: Option<String>,
    last_error: Option<String>,
    is_syncing: bool,
    pending: Option<PendingGate>,
    /// A sign-in waited for its callback at some point in this run: a late
    /// callback then says it expired.
    had_pending: bool,
    schedule: Schedule,
    summaries_paused_until: Option<SystemTime>,
    pending_sessions: u32,
    last_live_ids: BTreeSet<String>,
    /// The hub reported running sessions since capture last (re)started:
    /// until it has, `last_live_ids` says nothing about what ended.
    live_observed_since_resume: bool,
    /// The summary running beside the schedule (claimed before its thread
    /// starts, released when it ends).
    summary_cancel: Option<Cancel>,
    /// The last pass that read the folders' own history.
    last_backfill_at: Option<SystemTime>,
    /// Halved by a 413 (never below [`MIN_SESSIONS_PER_REQUEST`]); never
    /// raised again in the run.
    max_sessions_per_request: usize,
}

/// A summary run's claim: the generation it began in and the handle that
/// stops its child.
struct SummaryClaim {
    cancel: Cancel,
    generation: u64,
}

/// Clears `is_syncing` and republishes when a pass ends, however it ends.
struct SyncingFlag<'a>(&'a CloudSync);

impl Drop for SyncingFlag<'_> {
    fn drop(&mut self) {
        lock(&self.0.inner).is_syncing = false;
        self.0.publish();
    }
}

/// Settings writes collected under the lock and sent after it is released,
/// so `an-core` is never called with the service locked.
type Writes = Vec<(&'static str, bool)>;

pub struct CloudSync {
    sealed: bool,
    /// The website this run uses, as [`website::validated`] keeps it; fixed
    /// for the run.
    website: Option<String>,
    website_is_overridden: bool,
    deps: Arc<dyn CloudDeps>,
    http: Arc<dyn Http>,
    browser: Arc<dyn Browser>,
    clock: Arc<dyn Clock>,
    /// `None` when sealed: no stores, no files.
    stores: Option<CloudStores>,
    auth: Option<Arc<Auth>>,
    names: LocalNames,
    summarizer: Summarizer,
    generations: Arc<Generations>,
    inner: Mutex<Inner>,
    published: Mutex<CloudState>,
}

impl CloudSync {
    /// The service for `cfg`. Nothing is read or written, and nothing sent,
    /// until [`start`](Self::start); a sealed run never reads or writes a
    /// file, sends a request or opens a browser.
    pub fn new(cfg: CloudConfig, deps: Arc<dyn CloudDeps>, platform: &Platform) -> CloudSync {
        let sealed = cfg.sealed;
        let website = if sealed {
            None
        } else {
            website::validated(cfg.website.as_deref())
        };
        let website_is_overridden = !sealed && cfg.website_is_overridden && website.is_some();
        let stores = (!sealed)
            .then(|| CloudStores::new(&cfg.support, platform.files.clone(), true, &cfg.home));
        let auth = (!sealed).then(|| {
            Arc::new(Auth::new(
                platform.http.clone(),
                Arc::new(FileSessionStore::for_config(&cfg, platform.files.clone())),
                platform.clock.clone(),
            ))
        });
        let names = LocalNames::new(
            cfg.system_users.clone(),
            &cfg.home.to_string_lossy(),
            sealed,
        );
        // The app's own environment, scrubbed per run (`usage::scrubbed_env`).
        let summarizer = Summarizer::for_config(
            &cfg,
            platform.runner.clone(),
            platform.files.clone(),
            std::env::vars_os().collect(),
        );
        CloudSync {
            sealed,
            website,
            website_is_overridden,
            deps,
            http: platform.http.clone(),
            browser: platform.browser.clone(),
            clock: platform.clock.clone(),
            stores,
            auth,
            names,
            summarizer,
            generations: Arc::new(Generations::default()),
            inner: Mutex::new(Inner {
                started: false,
                cfg,
                written: BTreeMap::new(),
                auth_state: CloudAuthState::SignedOut,
                dashboard_url: None,
                last_error: None,
                is_syncing: false,
                pending: None,
                had_pending: false,
                schedule: Schedule {
                    next_sync_at: None,
                    not_before: None,
                    failures: 0,
                },
                summaries_paused_until: None,
                pending_sessions: 0,
                last_live_ids: BTreeSet::new(),
                live_observed_since_resume: false,
                summary_cancel: None,
                last_backfill_at: None,
                max_sessions_per_request: limit::SESSIONS,
            }),
            published: Mutex::new(CloudState::default()),
        }
    }

    // ---- Lifecycle ----

    /// Load the saved session and state. A sealed run only shows the fixture.
    pub fn start(&self, now: SystemTime) {
        {
            let mut inner = lock(&self.inner);
            if inner.started {
                return;
            }
            inner.started = true;
            if self.sealed {
                drop(inner);
                *lock(&self.published) = sealed_fixture(now);
                return;
            }
            if let Some(stores) = &self.stores {
                // Summaries turned on before they kept a date: from now on.
                if inner.cfg.summaries_enabled && stores.summaries.enabled_at().is_none() {
                    stores.summaries.note_enabled(now);
                }
                inner.dashboard_url = stores
                    .memory
                    .dashboard_url()
                    .and_then(|d| website::validated_link(Some(&d)));
            }
        }
        self.publish();
        self.restore_session();
    }

    /// Stops what runs (a summary's `claude` is killed, a sign-in or pass
    /// still out is dropped when it comes back) and saves every store now.
    pub fn stop(&self, _now: SystemTime) {
        {
            let mut inner = lock(&self.inner);
            if !inner.started {
                return;
            }
            inner.started = false;
            inner.pending = None;
            self.stop_summarizing(&mut inner);
        }
        if let Some(stores) = &self.stores {
            stores.save_now();
        }
    }

    /// The saved session, if it is for the website this run uses. Another
    /// website's (a run pointed elsewhere with `AGENTNOTCH_WEB_URL`, or a
    /// build whose website changed) is set aside, not deleted: signing in
    /// replaces it, and a run on that website finds it again.
    fn restore_session(&self) {
        let Some(auth) = &self.auth else { return };
        let generation = self.generations.auth();
        let session = auth.current_session();
        {
            let mut inner = lock(&self.inner);
            if generation != self.generations.auth() || !inner.started {
                return;
            }
            inner.auth_state = match session {
                None => CloudAuthState::SignedOut,
                Some(session) if Some(&session.website_url) == self.website.as_ref() => {
                    CloudAuthState::SignedIn {
                        email: session.email,
                    }
                }
                Some(_) => {
                    auth.set_aside();
                    CloudAuthState::SignedOut
                }
            };
        }
        self.publish();
    }

    /// The published state, without waiting on anything that runs.
    pub fn state(&self) -> CloudState {
        lock(&self.published).clone()
    }

    pub fn generations(&self) -> Arc<Generations> {
        self.generations.clone()
    }

    /// The website this run uses: `AGENTNOTCH_WEB_URL`'s or the build's,
    /// none when sealed or when neither is one the app accepts.
    pub fn effective_website(&self) -> Option<&str> {
        self.website.as_deref()
    }

    /// `None` when sealed.
    pub fn stores(&self) -> Option<&CloudStores> {
        self.stores.as_ref()
    }

    pub fn schedule(&self) -> Schedule {
        lock(&self.inner).schedule
    }

    /// `an-core` republished the config (after a switch was written, or the
    /// device's name or id changed). The website, the folders and sealing
    /// are the run's and stay as they started. A switch value this service
    /// asked for and `an-core` hasn't shown yet is kept over an older one.
    pub fn update_config(&self, cfg: CloudConfig, now: SystemTime) {
        if self.sealed {
            return;
        }
        let mut writes = Writes::new();
        {
            let mut inner = lock(&self.inner);
            let sync = self.accepted_switch(&mut inner, keys::CLOUD_SYNC_ENABLED, cfg.sync_enabled);
            let summaries = self.accepted_switch(
                &mut inner,
                keys::CLOUD_SUMMARIES_ENABLED,
                cfg.summaries_enabled,
            );
            inner.cfg.app_version = cfg.app_version;
            inner.cfg.device_name = cfg.device_name;
            inner.cfg.device_id = cfg.device_id;
            inner.cfg.summaries_allowed_by_default = cfg.summaries_allowed_by_default;
            if let Some(enabled) = sync {
                self.apply_sync(&mut inner, &mut writes, enabled, false);
            }
            if let Some(enabled) = summaries {
                self.apply_summaries(&mut inner, &mut writes, enabled, false, now);
            }
        }
        self.send(writes);
        self.publish();
    }

    /// The value to switch to from a republished config, if any.
    fn accepted_switch(
        &self,
        inner: &mut Inner,
        key: &'static str,
        incoming: bool,
    ) -> Option<bool> {
        if let Some(&asked) = inner.written.get(key) {
            if asked == incoming {
                inner.written.remove(key);
            }
            return None;
        }
        let current = if key == keys::CLOUD_SYNC_ENABLED {
            inner.cfg.sync_enabled
        } else {
            inner.cfg.summaries_enabled
        };
        (current != incoming).then_some(incoming)
    }

    // ---- Calls ----

    /// The settings section's actions. Every one is a no-op when sealed.
    pub fn call(&self, call: CloudCall, now: SystemTime) -> Result<(), String> {
        match call {
            CloudCall::SignIn => self.sign_in(now),
            CloudCall::CancelSignIn => {
                self.cancel_sign_in(now);
                Ok(())
            }
            CloudCall::SignOut => {
                self.sign_out(now);
                Ok(())
            }
            CloudCall::SetSync(enabled) => {
                self.set_sync(enabled, now);
                Ok(())
            }
            CloudCall::SetSummaries(enabled) => {
                self.set_summaries(enabled, now);
                Ok(())
            }
            CloudCall::SyncNow => {
                self.sync_now(now, true);
                Ok(())
            }
        }
    }

    // ---- Switches ----

    /// The sync switch. Nothing is captured or sent while it is off.
    pub fn set_sync(&self, enabled: bool, _now: SystemTime) {
        if self.sealed {
            return;
        }
        let mut writes = Writes::new();
        {
            let mut inner = lock(&self.inner);
            self.apply_sync(&mut inner, &mut writes, enabled, true);
        }
        self.send(writes);
        self.publish();
    }

    /// The session-summaries switch (spends the account's usage; off by
    /// default).
    pub fn set_summaries(&self, enabled: bool, now: SystemTime) {
        if self.sealed {
            return;
        }
        let mut writes = Writes::new();
        {
            let mut inner = lock(&self.inner);
            self.apply_summaries(&mut inner, &mut writes, enabled, true, now);
        }
        self.send(writes);
        self.publish();
    }

    fn apply_sync(&self, inner: &mut Inner, writes: &mut Writes, enabled: bool, write: bool) {
        if write {
            Self::write(inner, writes, keys::CLOUD_SYNC_ENABLED, enabled);
        } else {
            inner.cfg.sync_enabled = enabled;
        }
        if enabled {
            inner.schedule.next_sync_at = None;
            inner.schedule.failures = 0;
        } else {
            self.stop_syncing(inner);
        }
    }

    fn apply_summaries(
        &self,
        inner: &mut Inner,
        writes: &mut Writes,
        enabled: bool,
        write: bool,
        now: SystemTime,
    ) {
        if write {
            Self::write(inner, writes, keys::CLOUD_SUMMARIES_ENABLED, enabled);
        } else {
            inner.cfg.summaries_enabled = enabled;
        }
        if enabled {
            // Sessions that ended before now are never summarised.
            if let Some(stores) = &self.stores {
                stores.summaries.note_enabled(now);
            }
        } else {
            self.stop_summarizing(inner);
            self.forget_unsent_summaries(now);
        }
    }

    fn write(inner: &mut Inner, writes: &mut Writes, key: &'static str, value: bool) {
        if key == keys::CLOUD_SYNC_ENABLED {
            inner.cfg.sync_enabled = value;
        } else {
            inner.cfg.summaries_enabled = value;
        }
        inner.written.insert(key, value);
        writes.push((key, value));
    }

    fn send(&self, writes: Writes) {
        for (key, value) in writes {
            self.deps.set_setting(key, serde_json::Value::Bool(value));
        }
    }

    /// Sync and summaries off, and whatever they were doing stopped: every
    /// successful sign-in (consent is per sign-in), sign-out and refusal of
    /// the sign-in.
    fn turn_switches_off(&self, inner: &mut Inner, writes: &mut Writes, now: SystemTime) {
        Self::write(inner, writes, keys::CLOUD_SYNC_ENABLED, false);
        Self::write(inner, writes, keys::CLOUD_SUMMARIES_ENABLED, false);
        self.stop_syncing(inner);
        self.forget_unsent_summaries(now);
    }

    /// Summaries are off: the ones the website this PC is signed in to
    /// doesn't have (made, never sent to it) are deleted here. Those it has
    /// stay there (the website keeps what was sent) and here.
    fn forget_unsent_summaries(&self, now: SystemTime) {
        let Some(stores) = &self.stores else { return };
        let names = self.names.current(now);
        stores
            .summaries
            .remove_all(|key, summary| CloudSyncPass::was_sent(summary, key, stores, &names));
    }

    /// Capture and uploads stop: a pass in flight sends nothing more, a
    /// summary in flight is stopped, the readings kept only to be sent go,
    /// and what capture saw no longer dates the end of a session (it may
    /// end during the gap: then it ends at its last activity).
    fn stop_syncing(&self, inner: &mut Inner) {
        self.generations.bump_sync();
        self.stop_summarizing(inner);
        if let Some(stores) = &self.stores {
            stores.ledger.forget_run_state();
            stores.recorder.clear();
        }
        inner.last_live_ids.clear();
        inner.live_observed_since_resume = false;
        inner.pending_sessions = 0;
    }

    /// A summary in flight is dropped and its `claude` stopped.
    fn stop_summarizing(&self, inner: &mut Inner) {
        self.generations.bump_summary();
        if let Some(cancel) = &inner.summary_cancel {
            cancel.cancel();
        }
    }

    // ---- Signing in ----

    /// Starts a Google sign-in through the website's Supabase project: asks
    /// the website for it, opens the authorize page in the default browser
    /// and waits for the callback ([`deep_link`](Self::deep_link)). A
    /// sign-in already waiting is replaced (its verifier is forgotten, so
    /// its callback can't finish). Refused when this run has no website.
    pub fn sign_in(&self, now: SystemTime) -> Result<(), String> {
        if self.sealed {
            return Ok(());
        }
        let Some(website) = self.website.clone().filter(|_| self.auth.is_some()) else {
            let message = ApiError::NoWebsite.to_string();
            lock(&self.inner).last_error = Some(message.clone());
            self.publish();
            return Err(message);
        };
        let generation = {
            let mut inner = lock(&self.inner);
            if !inner.started {
                return Err(STOPPED.into());
            }
            inner.pending = None;
            inner.auth_state = CloudAuthState::SigningIn;
            inner.last_error = None;
            self.generations.bump_auth()
        };
        self.publish();

        let config = match self.api(&website).config() {
            Ok(config) => config,
            Err(error) => {
                let message = error.to_string();
                self.sign_in_failed(generation, &AuthError::Provider(message.clone()));
                return Err(message);
            }
        };
        let pending = match auth::begin(&config, &website) {
            Ok(pending) => pending,
            Err(error) => {
                self.sign_in_failed(generation, &error);
                return Err(error.to_string());
            }
        };
        let authorize_url = pending.authorize_url.clone();
        {
            let mut inner = lock(&self.inner);
            if !self.is_current(&inner, generation) {
                return Ok(());
            }
            // Kept before the browser opens: a callback can't beat it.
            inner.pending = Some(PendingGate {
                sign_in: pending,
                auth_generation: generation,
                started_at: now,
                dashboard: config.dashboard_url,
            });
            inner.had_pending = true;
        }
        if let Err(message) = self.browser.open(&authorize_url) {
            self.sign_in_failed(generation, &AuthError::Provider(message.clone()));
            return Err(message);
        }
        Ok(())
    }

    /// The user gave up waiting ("Cancel"): signed out (or still signed in
    /// as before), no error, and a late callback finishes nothing.
    pub fn cancel_sign_in(&self, _now: SystemTime) {
        if self.sealed {
            return;
        }
        {
            let mut inner = lock(&self.inner);
            self.generations.bump_auth();
            if inner.pending.take().is_some() || inner.auth_state == CloudAuthState::SigningIn {
                inner.auth_state = self.resting_state();
            }
        }
        self.publish();
    }

    /// A sign-in nobody finished in [`SIGN_IN_TIMEOUT`] ends quietly.
    fn expire_sign_in(&self, now: SystemTime) {
        let expired = {
            let mut inner = lock(&self.inner);
            let generation = match &inner.pending {
                Some(gate) if Self::is_expired(gate, now) => Some(gate.auth_generation),
                _ => None,
            };
            if let Some(generation) = generation {
                inner.pending = None;
                if self.is_current(&inner, generation) {
                    self.generations.bump_auth();
                    inner.auth_state = self.resting_state();
                }
            }
            generation.is_some()
        };
        if expired {
            self.publish();
        }
    }

    fn is_expired(gate: &PendingGate, now: SystemTime) -> bool {
        now.duration_since(gate.started_at)
            .is_ok_and(|waited| waited >= SIGN_IN_TIMEOUT)
    }

    /// What the section shows when no sign-in runs: the saved session's
    /// account, if it is this website's.
    fn resting_state(&self) -> CloudAuthState {
        match self.auth.as_ref().and_then(|auth| auth.current_session()) {
            Some(session) if Some(&session.website_url) == self.website.as_ref() => {
                CloudAuthState::SignedIn {
                    email: session.email,
                }
            }
            _ => CloudAuthState::SignedOut,
        }
    }

    /// Still the sign-in the user is waiting for: the service runs and no
    /// sign-in, sign-out or cancel came since. (The website is the run's.)
    fn is_current(&self, inner: &Inner, generation: u64) -> bool {
        inner.started && generation == self.generations.auth()
    }

    fn sign_in_failed(&self, generation: u64, error: &AuthError) {
        {
            let mut inner = lock(&self.inner);
            if !self.is_current(&inner, generation) {
                return;
            }
            inner.pending = None;
            if *error == AuthError::Cancelled {
                inner.auth_state = self.resting_state();
            } else {
                let message = error.to_string();
                inner.auth_state = CloudAuthState::Error {
                    message: message.clone(),
                };
                inner.last_error = Some(message);
            }
        }
        self.publish();
    }

    /// An `agentnotch://` link this PC was handed: the sign-in's callback is
    /// accepted once, only while a sign-in waits, only within
    /// [`SIGN_IN_TIMEOUT`]; anything else changes nothing.
    pub fn deep_link(&self, link: &str, now: SystemTime) -> DeepLinkOutcome {
        match self.accept_callback(link, now) {
            Ok(accepted) => self.finish_sign_in(accepted, now),
            Err(outcome) => outcome,
        }
    }

    /// The first half of [`deep_link`](Self::deep_link), safe on any thread
    /// and bounded by the token request's own timeout: the gate taken and
    /// the code traded for a session, which nobody uses yet.
    pub fn accept_callback(
        &self,
        link: &str,
        now: SystemTime,
    ) -> Result<AcceptedSignIn, DeepLinkOutcome> {
        if self.sealed {
            return Err(DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into()));
        }
        if !is_sign_in_callback(link) {
            return Err(DeepLinkOutcome::Ignored(NOT_A_SIGN_IN_LINK.into()));
        }
        let (Some(auth), Some(website)) = (&self.auth, self.website.clone()) else {
            return Err(DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into()));
        };
        let gate = {
            let mut inner = lock(&self.inner);
            let taken = inner.pending.take();
            match taken {
                Some(gate)
                    if self.is_current(&inner, gate.auth_generation)
                        && !Self::is_expired(&gate, now) =>
                {
                    gate
                }
                Some(gate) => {
                    if self.is_current(&inner, gate.auth_generation) {
                        // Timed out: the quiet cancel, and the note.
                        self.generations.bump_auth();
                        inner.auth_state = self.resting_state();
                    }
                    inner.last_error = Some(SIGN_IN_EXPIRED.into());
                    drop(inner);
                    self.publish();
                    return Err(DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into()));
                }
                None => {
                    let signed_in = matches!(inner.auth_state, CloudAuthState::SignedIn { .. });
                    let note = inner.had_pending && !signed_in && inner.started;
                    if note {
                        inner.last_error = Some(SIGN_IN_EXPIRED.into());
                    }
                    drop(inner);
                    if note {
                        self.publish();
                    }
                    return Err(DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into()));
                }
            }
        };
        match auth.finish(link, &gate.sign_in) {
            Ok(session) => {
                let current = self.is_current(&lock(&self.inner), gate.auth_generation);
                if !current {
                    auth.revoke(&session);
                    return Err(DeepLinkOutcome::SignInIgnored(SIGN_IN_SUPERSEDED.into()));
                }
                Ok(AcceptedSignIn {
                    session,
                    website,
                    auth_generation: gate.auth_generation,
                    dashboard: gate.dashboard,
                })
            }
            Err(error) => {
                self.sign_in_failed(gate.auth_generation, &error);
                Err(DeepLinkOutcome::SignInIgnored(error.to_string()))
            }
        }
    }

    /// The second half: adopt the session if the user still waits for it
    /// (else end it on Supabase), learn who signed in, and start this
    /// sign-in with sync and summaries off, for the user to turn on.
    pub fn finish_sign_in(&self, accepted: AcceptedSignIn, now: SystemTime) -> DeepLinkOutcome {
        let Some(auth) = &self.auth else {
            return DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into());
        };
        let AcceptedSignIn {
            session,
            website,
            auth_generation: generation,
            dashboard,
        } = accepted;
        {
            // Checked and adopted under the lock: a sign-out either comes
            // first (and this session is ended, never saved) or after (and
            // ends the adopted one).
            let inner = lock(&self.inner);
            if !self.is_current(&inner, generation) {
                drop(inner);
                auth.revoke(&session);
                return DeepLinkOutcome::SignInIgnored(SIGN_IN_SUPERSEDED.into());
            }
            auth.adopt(session.clone());
        }
        let mut email = session.email.clone();
        let mut user_id = session.user_id.clone();
        let mut dashboard = dashboard;
        if let Ok(me) = self.api(&website).me() {
            email = me.user.email.or(email);
            user_id = Some(me.user.id);
            dashboard = me.dashboard_url;
        }
        let mut writes = Writes::new();
        {
            let mut inner = lock(&self.inner);
            // Signed out, or another sign-in began, while asking: whoever
            // did that ended this session already.
            if !self.is_current(&inner, generation) {
                return DeepLinkOutcome::SignInIgnored(SIGN_IN_SUPERSEDED.into());
            }
            inner.dashboard_url = website::validated_link(Some(&dashboard));
            if let Some(stores) = &self.stores {
                stores
                    .memory
                    .adopt(user_id.as_deref(), &website, inner.dashboard_url.as_deref());
            }
            // Consent is per sign-in: the user turns sync on for this one.
            self.turn_switches_off(&mut inner, &mut writes, now);
            inner.auth_state = CloudAuthState::SignedIn { email };
            inner.last_error = None;
            inner.schedule = Schedule {
                next_sync_at: None,
                not_before: None,
                failures: 0,
            };
        }
        self.send(writes);
        self.publish();
        DeepLinkOutcome::SignInCompleted
    }

    /// Sign out of the website (this PC only). Sync and summaries go off,
    /// and what waits to be sent goes.
    pub fn sign_out(&self, now: SystemTime) {
        let Some(auth) = &self.auth else { return };
        let mut writes = Writes::new();
        let generation = {
            let mut inner = lock(&self.inner);
            let generation = self.generations.bump_auth();
            inner.pending = None;
            self.turn_switches_off(&mut inner, &mut writes, now);
            generation
        };
        self.send(writes);
        auth.sign_out();
        {
            let mut inner = lock(&self.inner);
            if generation == self.generations.auth() {
                inner.auth_state = CloudAuthState::SignedOut;
                inner.dashboard_url = None;
            }
        }
        self.publish();
    }

    fn api(&self, website: &str) -> CloudApi {
        let version = lock(&self.inner).cfg.app_version.clone();
        CloudApi::new(website, self.http.clone(), self.auth.clone(), &version)
            .with_clock(self.clock.clone())
    }

    // ---- Feeding ----

    /// Signed in, sync on, a website: uploads may happen.
    pub fn can_upload(&self) -> bool {
        let inner = lock(&self.inner);
        self.can_upload_locked(&inner)
    }

    fn can_upload_locked(&self, inner: &Inner) -> bool {
        inner.started
            && !self.sealed
            && inner.cfg.sync_enabled
            && self.website.is_some()
            && matches!(inner.auth_state, CloudAuthState::SignedIn { .. })
    }

    /// Sessions are captured and readings recorded only while uploads may
    /// happen.
    pub fn is_capturing(&self) -> bool {
        self.can_upload()
    }

    /// This run may launch Claude Code for summaries: the engine allows it
    /// (bootstrapped, live, not a `--no-install` run) and Claude Code isn't
    /// a `.cmd` shim alone (its arguments can't be passed through one
    /// safely).
    pub fn summaries_allowed(&self) -> bool {
        let allowed = lock(&self.inner).cfg.summaries_allowed_by_default;
        allowed && !self.claude_is_shim()
    }

    fn claude_is_shim(&self) -> bool {
        self.deps.claude_binary().is_some_and(|binary| binary.shim)
    }

    /// Uploads may happen, summaries are on, this run may launch Claude
    /// Code, no usage probe is launching it, and no rate limit paused them.
    pub fn can_summarize(&self, now: SystemTime) -> bool {
        let allowed = self.summaries_allowed();
        {
            let inner = lock(&self.inner);
            if !self.can_upload_locked(&inner) || !inner.cfg.summaries_enabled || !allowed {
                return false;
            }
            if inner
                .summaries_paused_until
                .is_some_and(|until| until > now)
            {
                return false;
            }
        }
        !self.deps.is_launching_claude()
    }

    /// The hub's running sessions (see `cloud::feed`): only while
    /// capturing; only accounts the website may hear of, each keyed as
    /// `CloudDeps::accounts` keys it. A session the ledger knows that runs
    /// as no such account now counts for none.
    pub fn observe_live(&self, batch: LiveBatch, now: SystemTime) {
        let Some(stores) = &self.stores else { return };
        {
            let mut inner = lock(&self.inner);
            if !self.can_upload_locked(&inner) {
                return;
            }
            inner.last_live_ids = batch.live_ids.union(&batch.waiting).cloned().collect();
            inner.live_observed_since_resume = true;
        }
        let accounts = environment::account_infos(&self.deps.accounts());
        let mut by_identity = BTreeMap::new();
        for account in &accounts {
            by_identity
                .entry(account.identity_id.clone())
                .or_insert(account);
        }
        let mut ledger_accounts = BTreeMap::new();
        let kept: Vec<_> = batch
            .attributed
            .iter()
            .filter_map(|observation| {
                let account = by_identity.get(&observation.identity_id)?;
                ledger_accounts
                    .entry(account.account_key.clone())
                    .or_insert_with(|| account.ledger_account());
                let mut keyed = observation.clone();
                keyed.account_key = account.account_key.clone();
                Some(keyed)
            })
            .collect();
        let ended = stores.ledger.observe(
            &kept,
            &batch.live_ids,
            &batch.unsure,
            &batch.waiting,
            &ledger_accounts,
            batch.at,
        );
        if !ended.is_empty() {
            self.sync_soon(now);
        }
    }

    /// A usage reading the engine took in: only while capturing; only
    /// accounts the website may hear of.
    pub fn record_usage(&self, observation: UsageObservation) {
        let Some(stores) = &self.stores else { return };
        if !self.can_upload() {
            return;
        }
        let accounts = environment::account_infos(&self.deps.accounts());
        let Some(account) = accounts
            .iter()
            .find(|account| account.identity_id == observation.identity)
        else {
            return;
        };
        let reading = RecordedUsageReading::from_observation(&observation, &account.account_key);
        if stores.recorder.record(reading) {
            self.publish();
        }
    }

    // ---- Schedule ----

    /// The regular pass (every [`TICK_INTERVAL`]): a sign-in nobody finished
    /// in time ends, who is signed in where is noted, sessions gone for a
    /// minute end, a summary starts beside the schedule when allowed (on its
    /// own thread: up to 90 s of `claude` never holds up a pass or a call),
    /// and a pass runs when due.
    pub fn tick(self: &Arc<Self>, now: SystemTime) {
        let Some(stores) = &self.stores else { return };
        if !lock(&self.inner).started {
            return;
        }
        self.expire_sign_in(now);
        // Local only, whether or not sync is on: the backfill needs to know
        // since when each folder has been signed in as its account.
        if let Some(logins) = self.deps.folder_logins() {
            stores.folder_logins.observe(&logins, now);
        }
        let settle = {
            let inner = lock(&self.inner);
            (self.can_upload_locked(&inner) && inner.live_observed_since_resume)
                .then(|| inner.last_live_ids.clone())
        };
        if let Some(live_ids) = settle {
            if !stores.ledger.settle(&live_ids, now).is_empty() {
                self.sync_soon(now);
            }
        }
        if let Some(claim) = self.claim_summary(now) {
            let service = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("an-cloud-summary".into())
                .spawn(move || {
                    service.summarize_claimed(&claim, now);
                    service.release_summary();
                });
            if spawned.is_err() {
                self.release_summary();
            }
        }
        self.sync_now(now, false);
    }

    /// Sync within [`SOON_DELAY`] (unless one is due sooner), never before a
    /// failure's backoff or the website's Retry-After ends.
    pub fn sync_soon(&self, now: SystemTime) {
        let mut inner = lock(&self.inner);
        let mut soon = now + SOON_DELAY;
        if let Some(not_before) = inner.schedule.not_before {
            if not_before > soon {
                soon = not_before;
            }
        }
        if inner.schedule.next_sync_at.is_some_and(|next| next <= soon) {
            return;
        }
        inner.schedule.next_sync_at = Some(soon);
    }

    /// A pass is due: none is planned (the first), or the planned one's time
    /// came, and no backoff or Retry-After runs.
    fn is_due(schedule: &Schedule, now: SystemTime) -> bool {
        schedule.next_sync_at.is_none_or(|at| at <= now)
            && schedule.not_before.is_none_or(|at| at <= now)
    }

    /// One sync pass (CL§5.4): what changed since it was last sent, in
    /// batches. `by_hand` ("Sync now") runs it whatever the schedule says,
    /// a backoff included; otherwise only when it is due. Nothing runs
    /// unless uploads may happen and no other pass runs.
    pub fn sync_now(&self, now: SystemTime, by_hand: bool) {
        let (Some(stores), Some(website)) = (&self.stores, self.website.as_deref()) else {
            return;
        };
        {
            let mut inner = lock(&self.inner);
            if !self.can_upload_locked(&inner)
                || inner.is_syncing
                || !(by_hand || Self::is_due(&inner.schedule, now))
            {
                return;
            }
            inner.is_syncing = true;
        }
        self.publish();
        let _syncing = SyncingFlag(self);
        self.run_pass(stores, website, now);
    }

    /// The pass itself (CloudSync.swift 1163-1237). Turning sync off (or
    /// signing out) during it stops it before the next batch; turning
    /// summaries off leaves them out of the batches still to go. A pass
    /// belongs to the sign-in it started with: when that changed while a
    /// request was out (signed out, in again), what comes back is dropped,
    /// nothing is marked sent and no failure is held against the new
    /// sign-in. (The website is the run's, fixed: the Mac's other half of
    /// "bound" can't change here.)
    fn run_pass(&self, stores: &CloudStores, website: &str, now: SystemTime) {
        let generation = self.generations.sync();
        let pass_sign_in = self.generations.auth();
        let is_bound = || pass_sign_in == self.generations.auth();

        let (backfill_due, cfg, max_sessions) = {
            let mut inner = lock(&self.inner);
            let due = inner
                .last_backfill_at
                .is_none_or(|at| now.duration_since(at).is_ok_and(|d| d >= BACKFILL_INTERVAL));
            if due {
                inner.last_backfill_at = Some(now);
            }
            (due, inner.cfg.clone(), inner.max_sessions_per_request)
        };
        let backfill_folders = backfill_due.then(|| {
            backfill::folders(
                &self.deps.backfill_folders(),
                &self.deps.folder_logins().unwrap_or_default(),
                &stores.folder_logins,
                &stores.paths,
            )
        });
        let device = SyncDevice {
            id: cfg.device_id.clone(),
            name: cfg.device_name.clone(),
            app_version: cfg.app_version.clone(),
        };
        let mut input = Input::new(
            environment::account_infos(&self.deps.accounts()),
            device,
            now,
        );
        input.backfill_folders = backfill_folders;
        input.include_summaries = cfg.summaries_enabled;
        input.known_names = self.names.current(now);
        input.max_sessions_per_request = max_sessions;
        // Reads transcripts: never under the service's lock.
        let prepared = CloudSyncPass::prepare(&input, stores);
        {
            let mut inner = lock(&self.inner);
            if !is_bound() {
                return;
            }
            inner.pending_sessions = clamp_u32(prepared.session_count);
        }
        self.publish();

        let api = self.api(website);
        for batch in &prepared.batches {
            let summaries_on = {
                let inner = lock(&self.inner);
                if generation != self.generations.sync()
                    || !is_bound()
                    || !self.can_upload_locked(&inner)
                {
                    // Sync was switched off (or the sign-in changed) during
                    // the pass: the rest isn't sent.
                    return;
                }
                inner.cfg.summaries_enabled
            };
            let batch = if summaries_on {
                batch.clone()
            } else {
                batch.without_summaries()
            };
            match api.sync(&batch.request) {
                Ok(_) => {
                    // Checked and marked under the lock: a sign-out comes
                    // either before (nothing is marked) or after.
                    let mut inner = lock(&self.inner);
                    if !is_bound() {
                        return;
                    }
                    stores.memory.mark_sent(&batch.records, self.clock.now());
                    stores.recorder.mark_sent(&batch.readings);
                    inner.pending_sessions = inner
                        .pending_sessions
                        .saturating_sub(clamp_u32(batch.records.len()));
                }
                Err(error) => {
                    // The sign-in or website changed while it was out: its
                    // failure isn't the new sign-in's.
                    if is_bound() {
                        self.handle_sync_failure(&error);
                    }
                    return;
                }
            }
        }
        let mut inner = lock(&self.inner);
        if generation != self.generations.sync() || !is_bound() {
            return;
        }
        let at = self.clock.now();
        stores.memory.note_synced(at);
        inner.schedule.failures = 0;
        inner.schedule.not_before = None;
        inner.last_error = None;
        inner.schedule.next_sync_at = Some(
            at + if prepared.has_more {
                SOON_DELAY
            } else {
                SYNC_INTERVAL
            },
        );
    }

    /// A request of the pass failed (CL§5.8): the sign-in ended (Supabase
    /// refused the refresh token, or the session is another website's):
    /// signed out, the switches with it. A 413 shrinks the requests. Any
    /// other failure, a 401 after a good refresh included, keeps the
    /// session and backs off (honouring the website's Retry-After).
    fn handle_sync_failure(&self, error: &ApiError) {
        let now = self.clock.now();
        let mut writes = Writes::new();
        {
            let mut inner = lock(&self.inner);
            if error.ends_sign_in() {
                self.generations.bump_auth();
                self.turn_switches_off(&mut inner, &mut writes, now);
                inner.auth_state = CloudAuthState::SignedOut;
                inner.dashboard_url = None;
                inner.last_error = Some(SIGNED_OUT_OF_WEBSITE.into());
            } else if let ApiError::Server {
                status,
                retry_after,
                ..
            } = error
            {
                if *status == 413 && inner.max_sessions_per_request > MIN_SESSIONS_PER_REQUEST {
                    inner.max_sessions_per_request =
                        (inner.max_sessions_per_request / 2).max(MIN_SESSIONS_PER_REQUEST);
                    inner.schedule.next_sync_at = Some(now + TOO_LARGE_RETRY);
                } else {
                    inner.schedule.failures = inner.schedule.failures.saturating_add(1);
                    inner.last_error = Some(if *status == 401 {
                        // Refused even after a refresh Supabase accepted: the
                        // website's own check failed (its key set can't be
                        // fetched, say). The session is kept; try later.
                        SIGN_IN_NOT_ACCEPTED.into()
                    } else {
                        error.to_string()
                    });
                    let wait = backoff(inner.schedule.failures).max(retry_wait(*retry_after));
                    Self::back_off(&mut inner, now, wait);
                }
            } else {
                inner.schedule.failures = inner.schedule.failures.saturating_add(1);
                inner.last_error = Some(error.to_string());
                let wait = backoff(inner.schedule.failures);
                Self::back_off(&mut inner, now, wait);
            }
        }
        self.send(writes);
    }

    /// No pass before `now + wait`, whatever asks for one sooner.
    fn back_off(inner: &mut Inner, now: SystemTime, wait: Duration) {
        let until = now
            .checked_add(wait)
            .or_else(|| now.checked_add(MAX_RETRY_AFTER))
            .unwrap_or(now);
        inner.schedule.not_before = Some(until);
        inner.schedule.next_sync_at = Some(until);
    }

    // ---- Summaries ----

    /// Summarise the next due session, if any, here and now (CL§9.1): one
    /// that ended after summaries were turned on, of an account whose
    /// 5-hour window is under [`SUMMARY_USAGE_CEILING`], run in a folder of
    /// the account that ran it, from its own part of the conversation only.
    /// One at a time: nothing happens while another runs. The tick runs the
    /// same on a thread of its own.
    pub fn summarize_next(&self, now: SystemTime) {
        if let Some(claim) = self.claim_summary(now) {
            self.summarize_claimed(&claim, now);
            self.release_summary();
        }
    }

    /// A summary is running (tests wait for the tick's to end).
    pub fn is_summarizing(&self) -> bool {
        lock(&self.inner).summary_cancel.is_some()
    }

    /// The right to run the next summary, when summaries may run and none
    /// does. Its generation is read first: a switch-off from here on drops
    /// the run.
    fn claim_summary(&self, now: SystemTime) -> Option<SummaryClaim> {
        self.stores.as_ref()?;
        let generation = self.generations.summary();
        if !self.can_summarize(now) {
            return None;
        }
        let mut inner = lock(&self.inner);
        if inner.summary_cancel.is_some() || !inner.started {
            return None;
        }
        let cancel = Cancel::new();
        inner.summary_cancel = Some(cancel.clone());
        self.generations.track_summary(Some(cancel.clone()));
        if generation != self.generations.summary() {
            cancel.cancel();
        }
        Some(SummaryClaim { cancel, generation })
    }

    fn release_summary(&self) {
        let mut inner = lock(&self.inner);
        inner.summary_cancel = None;
        self.generations.track_summary(None);
    }

    /// The run of a claimed summary (CloudSync.swift 1296-1356).
    fn summarize_claimed(&self, claim: &SummaryClaim, now: SystemTime) {
        let Some(stores) = &self.stores else { return };
        let current =
            || claim.generation == self.generations.summary() && !claim.cancel.is_cancelled();
        let accounts: Vec<CloudAccountInfo> = environment::account_infos(&self.deps.accounts())
            .into_iter()
            .filter(|account| {
                self.deps.five_hour(&account.identity_id).unwrap_or(0.0) < SUMMARY_USAGE_CEILING
            })
            .collect();
        let candidates =
            CloudSyncPass::summary_candidates(&accounts, stores, stores.summaries.enabled_at());
        let Some(candidate) = stores.summaries.next_candidate(&candidates, now) else {
            return;
        };
        if !current() {
            return;
        }
        let Some(folder) = self.deps.summary_folder(&candidate.identity_id) else {
            stores
                .summaries
                .record_failure(&candidate.key, NO_FOLDER_REASON, now, NO_FOLDER_WAIT);
            return;
        };
        let owners = stores.ledger.owners(&candidate.session_id);
        let stretches = SessionOwners::stretches(&candidate.account_key, &owners);
        let excerpt = build_excerpt(
            &*stores.files,
            PathStyle::native(),
            &candidate.transcript_path,
            &candidate.session_id,
            stretches.as_deref(),
            MAX_EXCERPT_CHARACTERS,
        );
        let Some(excerpt) = excerpt else {
            stores
                .summaries
                .record_failure(&candidate.key, NO_CONVERSATION_REASON, now, MAX_RETRY);
            return;
        };
        // Switched off (or signed out) while it was being prepared: nothing
        // is launched.
        if !current() || !self.can_summarize(now) {
            return;
        }
        stores.summaries.note_run(now);
        let request = summary_run::Request {
            input: summary_run::input(&excerpt),
            config_dir_env: folder.config_dir_env.clone(),
            binary: self.deps.claude_binary(),
        };
        let outcome = self.summarizer.run(&request, &claim.cancel, now);
        // Turned off meanwhile: whatever came back isn't wanted.
        if !current() || !lock(&self.inner).cfg.summaries_enabled {
            return;
        }
        let at = self.clock.now();
        if !self
            .deps
            .summary_folder_still_runs(&folder, &candidate.identity_id)
        {
            stores.summaries.record_failure(
                &candidate.key,
                FOLDER_CHANGED_REASON,
                at,
                Duration::ZERO,
            );
            return;
        }
        match outcome {
            Outcome::Summary(summary) => {
                stores
                    .summaries
                    .record(&candidate.key, &summary, candidate.message_count, at);
                self.sync_soon(at);
            }
            Outcome::RateLimited => {
                lock(&self.inner).summaries_paused_until = Some(at + SUMMARY_PAUSE);
                stores.summaries.record_failure(
                    &candidate.key,
                    RATE_LIMITED_REASON,
                    at,
                    SUMMARY_PAUSE,
                );
            }
            Outcome::Unavailable(reason) => {
                stores
                    .summaries
                    .record_failure(&candidate.key, &reason, at, UNAVAILABLE_WAIT);
            }
            Outcome::Failed(reason) => {
                stores
                    .summaries
                    .record_failure(&candidate.key, &reason, at, Duration::ZERO);
            }
        }
        self.publish();
    }

    // ---- Publishing ----

    fn publish(&self) {
        if self.sealed {
            return;
        }
        let summaries_available = self.summaries_allowed();
        let shim = self.claude_is_shim();
        let next = {
            let inner = lock(&self.inner);
            let sync_enabled = inner.cfg.sync_enabled;
            let stores = self.stores.as_ref();
            let mut last_error = inner.last_error.clone();
            // A `.cmd`-only install can't run summaries: said where the
            // switch is, unless something more pressing is shown.
            if shim && inner.cfg.summaries_enabled && last_error.is_none() {
                last_error = Some(SHIM_REFUSED.into());
            }
            CloudState {
                website_url: self.website.clone(),
                website_is_overridden: self.website_is_overridden,
                auth: inner.auth_state.clone(),
                sync_enabled,
                summaries_enabled: inner.cfg.summaries_enabled,
                summaries_available,
                is_syncing: inner.is_syncing,
                last_sync_at_ms: stores.and_then(|s| s.memory.last_sync_at()).map(epoch_ms),
                last_error,
                pending_sessions: if sync_enabled {
                    inner.pending_sessions
                } else {
                    0
                },
                pending_usage: stores.map_or(0, |s| clamp_u32(s.recorder.pending_count())),
                summarized_sessions: stores.map_or(0, |s| clamp_u32(s.summaries.count())),
                pools_url: pools_url(inner.dashboard_url.as_deref()),
                settings_url: settings_url(inner.dashboard_url.as_deref(), self.website.as_deref()),
                dashboard_url: inner.dashboard_url.clone(),
            }
        };
        let mut published = lock(&self.published);
        if *published != next {
            *published = next;
        }
    }
}
