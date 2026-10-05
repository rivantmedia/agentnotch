//! Cloud sync wired into the hub (design §3.4, §4.11; CL§5.1, §5.9, §6.1;
//! the Mac's `ClaudeControlHub+Cloud.swift`).
//!
//! - At start `an-core` starts [`CloudService`] with the config it makes
//!   from the settings and the [`HubConfig`](super::api::HubConfig): the
//!   build's website unless `AGENTNOTCH_WEB_URL` names one the app accepts,
//!   the switches as saved, the device id (minted here once, saved with the
//!   next settings write, which turning sync on always is: nothing is ever
//!   sent under an id that wasn't saved).
//! - The cloud reads the engine through [`LiveCloudDeps`], whose view is
//!   republished after every projection; its switch writes come back as
//!   `Input::SetSetting`, and the config is handed over again whenever what
//!   it is made of changed.
//! - The running sessions feed the ledger after every projection
//!   (`cloud::feed::live_batch`): only when the batch changed, again when
//!   the cloud's state changed (sync turned on wants to hear of them now),
//!   and when a session not placed yet runs out of
//!   [`PLACEMENT_GRACE`](crate::model::PLACEMENT_GRACE).
//! - Every usage reading the engine takes in is recorded (the service keeps
//!   it only while syncing).
//! - The service publishes its state without telling anyone; a small
//!   watcher (`an-cloud-state`) hands each change to `an-core` as
//!   `Input::CloudState`, so the settings page and `control status` follow.
//! - Nothing is sent before a sign-in and sync on: that is the service's
//!   rule; the hub only never calls it when sealed (the sealed hub isn't
//!   this one) and sends `CancelSignIn` only while a sign-in waits.
//!
//! Owner: WP7.

use super::api::{CallError, CloudAction, CloudUrlTarget, UrlReply};
use super::cloud_view::{running_batch, BinarySearch, CloudView, LiveCloudDeps};
use super::core_state::{to_value, Core};
use crate::cloud::service::STOPPED;
use crate::cloud::{website, CloudHandle, CloudService};
use crate::model::{CloudAuthState, CloudState};
use crate::runtime_types::{CloudCall, CloudConfig, Input, LiveBatch, UsageObservation};
use crossbeam_channel::{RecvTimeoutError, Sender};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the watcher looks at the cloud's published state.
const STATE_POLL: Duration = Duration::from_millis(100);

/// The cloud's side of `an-core`.
#[derive(Default)]
pub(crate) struct CloudWiring {
    /// `an-core`'s queue (set by the runtime before a start): the watcher's
    /// and the cloud's way back in.
    pub(crate) inputs: Option<Sender<Input>>,
    /// The running service; the runtime keeps a copy for deep links and
    /// for stopping it.
    pub(crate) handle: Option<Arc<CloudHandle>>,
    deps: Option<Arc<LiveCloudDeps>>,
    /// The config last handed to the service.
    config: Option<CloudConfig>,
    /// The batch last fed (its date cleared); `None` feeds the next one
    /// whatever it holds.
    fed: Option<LiveBatch>,
    /// When a session not placed yet runs out of its grace.
    feed_due: Option<SystemTime>,
    watcher: Option<(Sender<()>, JoinHandle<()>)>,
    /// The app's `PATH`, once (where `claude` is looked for).
    env_path: Option<OsString>,
}

impl Core {
    // ---- start and stop ----

    /// Starts the service (never in a sealed run, never twice, never on a
    /// hub without a queue: the command line's hub has no cloud).
    pub(crate) fn cloud_on_start(&mut self, now: SystemTime) {
        if self.cfg.flags.sealed || self.cloud_w.handle.is_some() {
            return;
        }
        let Some(inputs) = self.cloud_w.inputs.clone() else {
            return;
        };
        self.mint_device_id();
        self.cloud_w.env_path = Some(app_path());
        let config = self.cloud_config();
        let deps = Arc::new(LiveCloudDeps::new(self.cloud_view(now), inputs.clone()));
        let handle = Arc::new(CloudService::start(
            config.clone(),
            deps.clone(),
            &self.platform,
        ));
        self.cloud = handle.state();
        self.cloud_w.watcher = watch(handle.clone(), self.cloud.clone(), inputs);
        self.cloud_w.config = Some(config);
        self.cloud_w.deps = Some(deps);
        self.cloud_w.handle = Some(handle);
        self.cloud_w.fed = None;
    }

    /// The hub stops: the watcher ends here; the service itself is stopped
    /// by the runtime once the core has stopped (held requests are released
    /// first, and a request the cloud has out never holds them).
    pub(crate) fn cloud_on_stop(&mut self) {
        if let Some((quit, watcher)) = self.cloud_w.watcher.take() {
            drop(quit);
            let _ = watcher.join();
        }
        self.cloud_w.handle = None;
        self.cloud_w.deps = None;
        self.cloud_w.config = None;
        self.cloud_w.fed = None;
        self.cloud_w.feed_due = None;
    }

    /// A device id for the website: the saved one, else a new one, kept in
    /// the settings (and in the file the next write makes).
    fn mint_device_id(&mut self) {
        if self.settings.cloud_device_id.is_some() {
            return;
        }
        let mut next = self.settings.clone();
        next.device_id();
        self.adopt_settings_quietly(next);
    }

    /// What the service runs with, from the settings now.
    fn cloud_config(&self) -> CloudConfig {
        let flags = &self.cfg.flags;
        let (website, website_is_overridden) = website::effective(
            self.cfg.website.as_deref(),
            flags.web_url_override.as_deref(),
            flags.sealed,
        );
        CloudConfig {
            support: self.cfg.roots.support.clone(),
            website,
            website_is_overridden,
            app_version: self.cfg.app_version.clone(),
            device_name: self.platform.device.computer_name(),
            device_id: self.settings.cloud_device_id.clone().unwrap_or_default(),
            sync_enabled: self.settings.cloud_sync_enabled,
            summaries_enabled: self.settings.cloud_summaries_enabled,
            // Never in a `--no-install` run or one whose probes are off.
            summaries_allowed_by_default: flags.summaries_allowed() && flags.probes_allowed(),
            sealed: flags.sealed,
            system_users: self.cfg.roots.system_users.clone(),
            home: self.cfg.roots.home.clone(),
        }
    }

    /// The engine as the cloud reads it now.
    fn cloud_view(&self, now: SystemTime) -> CloudView {
        let accounts = self.registry.cloud_accounts();
        let allowed = accounts.iter().map(|a| a.identity_id.clone()).collect();
        let registry_accounts = self.registry.accounts();
        let folders = self.registry.folders();
        let five_hour = registry_accounts
            .iter()
            .filter_map(|account| {
                let window = self
                    .usage
                    .usage_of(&account.identity_id)?
                    .five_hour
                    .as_ref()?;
                Some((
                    account.identity_id.clone(),
                    window.effective_utilization(now),
                ))
            })
            .collect();
        CloudView {
            folder_logins: self.registry.folder_logins(),
            backfill_folders: self.registry.backfill_folders(&allowed),
            five_hour,
            summary: CloudView::summary_accounts(&registry_accounts, &folders),
            mirrors_default: self.registry.mirrors_default(),
            paths: Some(self.registry.paths().clone()),
            launching: self.usage.is_probing(),
            binary: BinarySearch {
                roots: Some(self.cfg.roots.clone()),
                choice: self.settings.claude_binary_path.as_ref().map(Into::into),
                remembered: self.usage.remembered_binary().map(Into::into),
                env_path: self.cloud_w.env_path.clone().unwrap_or_default(),
            },
            accounts,
        }
    }

    // ---- after every projection, and on its own deadline ----

    /// A projection was made: the cloud's view follows, and the ledger hears
    /// of the running sessions.
    pub(crate) fn cloud_projected(&mut self, now: SystemTime) {
        let Some(deps) = self.cloud_w.deps.clone() else {
            return;
        };
        deps.publish(self.cloud_view(now));
        self.feed_cloud(now);
    }

    /// Feeds the ledger the running sessions when the batch changed.
    fn feed_cloud(&mut self, now: SystemTime) {
        let Some(handle) = self.cloud_w.handle.clone() else {
            return;
        };
        let accounts = self.registry.cloud_accounts();
        let (batch, due) = running_batch(&self.session_views(), &accounts, now);
        self.cloud_w.feed_due = due;
        let undated = LiveBatch {
            at: UNIX_EPOCH,
            ..batch.clone()
        };
        if self.cloud_w.fed.as_ref() == Some(&undated) {
            return;
        }
        self.cloud_w.fed = Some(undated);
        handle.observe_live(batch);
    }

    /// After every input: the config when what it is made of changed, and
    /// the feed when a session's grace ran out.
    pub(crate) fn drive_cloud(&mut self, now: SystemTime) {
        let Some(handle) = self.cloud_w.handle.clone() else {
            return;
        };
        let config = self.cloud_config();
        if self.cloud_w.config.as_ref() != Some(&config) {
            self.cloud_w.config = Some(config.clone());
            handle.update_config(config);
        }
        if self.cloud_w.feed_due.is_some_and(|due| due <= now) {
            self.feed_cloud(now);
        }
    }

    pub(crate) fn cloud_deadline(&self) -> Option<SystemTime> {
        self.cloud_w.handle.as_ref()?;
        self.cloud_w.feed_due
    }

    /// The cloud published a new state (`Input::CloudState`).
    pub(crate) fn cloud_state_changed(&mut self, state: CloudState) {
        let resumed = state.sync_enabled != self.cloud.sync_enabled
            || std::mem::discriminant(&state.auth) != std::mem::discriminant(&self.cloud.auth);
        self.cloud = state;
        if resumed {
            // Sync on (or a sign-in) wants the sessions running now.
            self.cloud_w.fed = None;
        }
    }

    /// Usage readings the engine took in, for the website's history.
    pub(crate) fn cloud_record_usage(
        &self,
        observations: impl IntoIterator<Item = UsageObservation>,
    ) {
        if let Some(handle) = &self.cloud_w.handle {
            for observation in observations {
                handle.record_usage(observation);
            }
        }
    }

    // ---- calls ----

    /// `cloud {action, on?}`. Queued to the cloud thread; the state says how
    /// it went. All no-ops when sealed.
    pub(crate) fn cloud_call(
        &mut self,
        action: CloudAction,
        on: Option<bool>,
    ) -> Result<Value, CallError> {
        if self.cfg.flags.sealed {
            return Ok(json!({}));
        }
        let Some(handle) = self.cloud_w.handle.clone() else {
            return Err(CallError::failed(STOPPED));
        };
        let switch = |name: &str| {
            on.ok_or_else(|| CallError::invalid(format!("{name} needs \"on\" (true or false).")))
        };
        let call = match action {
            CloudAction::SignIn => CloudCall::SignIn,
            CloudAction::CancelSignIn => {
                // Only a sign-in that waits is cancelled: anything else
                // would bump the sign-in generation for nothing.
                if handle.state().auth != CloudAuthState::SigningIn {
                    return Ok(json!({}));
                }
                CloudCall::CancelSignIn
            }
            CloudAction::SignOut => CloudCall::SignOut,
            CloudAction::SetSync => CloudCall::SetSync(switch("set_sync")?),
            CloudAction::SetSummaries => CloudCall::SetSummaries(switch("set_summaries")?),
            CloudAction::SyncNow => CloudCall::SyncNow,
        };
        handle
            .call(call)
            .map(|()| json!({}))
            .map_err(CallError::failed)
    }

    /// `cloud_url {target}` → `{url}`, once the website has said where.
    pub(crate) fn cloud_url_call(&self, target: CloudUrlTarget) -> Result<Value, CallError> {
        let state = self
            .cloud_w
            .handle
            .as_ref()
            .map_or_else(|| self.cloud.clone(), |handle| handle.state());
        let url = match target {
            CloudUrlTarget::Dashboard => state.dashboard_url,
            CloudUrlTarget::Pools => state.pools_url,
            CloudUrlTarget::Settings => state.settings_url,
        };
        match url {
            Some(url) => to_value(&UrlReply { url }),
            None => Err(CallError::not_found(
                "The website hasn't said where that is yet.",
            )),
        }
    }
}

/// The app's own `PATH`.
fn app_path() -> OsString {
    std::env::vars_os()
        .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// `an-cloud-state`: hands each change of the cloud's published state to
/// `an-core`, until its sender is dropped.
fn watch(
    handle: Arc<CloudHandle>,
    shown: CloudState,
    inputs: Sender<Input>,
) -> Option<(Sender<()>, JoinHandle<()>)> {
    let (quit, quit_rx) = crossbeam_channel::bounded::<()>(0);
    let thread = std::thread::Builder::new()
        .name("an-cloud-state".into())
        .spawn(move || {
            let mut shown = shown;
            loop {
                let state = handle.state();
                if state != shown {
                    shown = state.clone();
                    if inputs.send(Input::CloudState(state)).is_err() {
                        return;
                    }
                }
                match quit_rx.recv_timeout(STATE_POLL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    _ => return,
                }
            }
        })
        .ok()?;
    Some((quit, thread))
}
