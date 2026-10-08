//! `Core`: everything `an-core` owns (design §1.2) — the settings, the
//! packages' stores, the jobs in flight and the calls waiting on them — and
//! what one input does to it. Only `an-core` touches a `Core` while the hub
//! runs (a stopped hub lends it to the caller of a call); nothing here
//! blocks on IO: blocking work leaves as a [`Job`] through the outbox and
//! comes back through [`Core::job_done`].
//!
//! The packages are wired in their own files: accounts, usage and hooks
//! (`wire_accounts`, `wire_usage`, `wire_hooks`), the hook pipe and held
//! requests (`wire_ingress`), sessions, review and chat (`wire_sessions`),
//! the jump, typed replies and the attention reactions (`wire_control`),
//! cloud sync (`wire_cloud`, over the view of `cloud_view`).
//!
//! Owner: WP7.

use super::api::{Call, CallError, HubConfig, HubEvent};
use super::project::{self, Directory, ProjectionInput};
use super::project_settings::{settings_snapshot, setup_state, SettingsInput, SetupInput};
use super::wire_accounts::AccountsWiring;
use super::wire_cloud::CloudWiring;
use super::wire_control::{row_extras, ControlWiring};
use super::wire_hooks::HooksWiring;
use super::wire_ingress::IngressWiring;
use super::wire_sessions::{session_store, SessionsWiring};
use super::wire_usage::UsageWiring;
use crate::accounts::{AccountRegistry, DiskProbe};
use crate::attention::rows::ResetClock;
use crate::core::settings::ControlSettings;
use crate::hooks::HookManager;
use crate::model::*;
use crate::persist::hook_install::HookInstallFile;
use crate::persist::settings::SettingsFile;
use crate::persist::usage::UsageStateFile;
use crate::platform::{Expect, NotifyPermission, Platform, WriteMode};
use crate::runtime_types::*;
use crate::usage::store::{UsageStore, UsageStoreConfig};
use crate::usage::ProbeEnvironment;
use agentnotch_proto::ControlStatus;
use crossbeam_channel::Sender;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

/// Where a call's answer goes.
pub(crate) type Reply = Sender<Result<Value, CallError>>;

/// Why calls still waiting are answered at a stop.
const STOPPING: &str = "The app is stopping.";

/// A job handed to a lane and not back yet.
pub(crate) struct PendingJob {
    pub(crate) lane: Lane,
    /// The call answered when the job is back (focus, typing, a probe the
    /// user asked for): long calls never hold `an-core`.
    pub(crate) reply: Option<Reply>,
}

/// What the pages are shown, as one projection made it.
#[derive(Debug, Clone)]
pub(crate) struct Projection {
    pub(crate) snapshot: HubSnapshot,
    pub(crate) settings: SettingsSnapshot,
    pub(crate) status: ControlStatus,
}

pub(crate) struct Core {
    pub(crate) cfg: HubConfig,
    pub(crate) platform: Platform,
    pub(crate) settings: ControlSettings,
    /// The file as read, so a key this build doesn't know survives a write.
    settings_file: SettingsFile,
    /// The settings changed since the last write was handed out.
    settings_dirty: bool,
    /// The one settings write in flight: writes never overtake each other.
    settings_write: Option<JobId>,
    pub(crate) registry: AccountRegistry,
    pub(crate) hooks: HookManager,
    pub(crate) usage: UsageStore,
    pub(crate) sessions: crate::sessions::SessionStore,
    pub(crate) cloud: CloudState,
    pub(crate) panel: PanelState,
    pub(crate) hotkey_ok: bool,
    pub(crate) hotkey_message: Option<String>,
    pub(crate) transport_error: Option<String>,
    pub(crate) versions: Vec<VersionSighting>,
    pub(crate) changed_folders: Vec<AccountId>,
    pub(crate) window_names: BTreeMap<String, String>,
    pub(crate) notify_permission: NotifyPermission,
    snapshot_clock: project::SnapshotClock,
    jobs: BTreeMap<JobId, PendingJob>,
    next_job: u64,
    /// Jobs to hand to the lanes, in the order they were made.
    outbox: Vec<(JobId, Job)>,
    /// Inputs a stop set aside while it waited for a write; they go first
    /// at the next start, in their order.
    pub(crate) backlog: VecDeque<Input>,
    /// Events that aren't projections (log lines, notices, panel requests),
    /// in order.
    events: Vec<HubEvent>,
    /// Between a start and a stop: the stores' own schedules run (a stopped
    /// hub only does what a call asks).
    pub(crate) live: bool,
    /// The engine's files in `<support>`: one write per file in flight.
    persists: Persisting,
    pub(crate) accounts_w: AccountsWiring,
    pub(crate) usage_w: UsageWiring,
    pub(crate) hooks_w: HooksWiring,
    pub(crate) ingress_w: IngressWiring,
    pub(crate) sessions_w: SessionsWiring,
    pub(crate) control_w: ControlWiring,
    pub(crate) cloud_w: CloudWiring,
}

/// The writes of `accounts.json`, `usage-state.json` and
/// `hook-install.json`: never two of one file at once (an older write
/// landing last would undo the newer), and what was last written, so a
/// stop writes only what is newer.
#[derive(Default)]
struct Persisting {
    in_flight: HashMap<PersistFile, (JobId, Vec<u8>)>,
    next: HashMap<PersistFile, Vec<u8>>,
    saved: HashMap<PersistFile, Vec<u8>>,
}

impl Core {
    /// The core of a hub that hasn't run yet: the settings from
    /// `control-settings.json` (the defaults when it is missing or doesn't
    /// parse), empty stores.
    pub(crate) fn load(cfg: HubConfig, platform: Platform) -> Core {
        let path = cfg.roots.support.join(crate::persist::settings::FILE_NAME);
        let mut events = Vec::new();
        let settings_file = match std::fs::read(&path) {
            Ok(bytes) => SettingsFile::parse(&bytes).unwrap_or_else(|| {
                events.push(HubEvent::Log(format!(
                    "{} didn't parse; the defaults apply until a setting changes",
                    crate::persist::settings::FILE_NAME
                )));
                SettingsFile::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SettingsFile::default(),
            Err(e) => {
                events.push(HubEvent::Log(format!(
                    "{} unreadable ({:?}); the defaults apply",
                    crate::persist::settings::FILE_NAME,
                    e.kind()
                )));
                SettingsFile::default()
            }
        };
        let settings = settings_file.settings();
        let mut persists = Persisting::default();
        let mut hooks = HookManager::configured(cfg.hook_exe.clone(), &cfg.flags);
        if let Some(file) = read_support(&cfg.roots.support, PersistFile::HookInstall, &mut events)
            .and_then(|bytes| HookInstallFile::parse(&bytes))
        {
            hooks.set_record(file.to_model());
        }
        persists.saved.insert(
            PersistFile::HookInstall,
            HookInstallFile::from_model(hooks.record()).encode(),
        );
        let paths = cfg.roots.paths();
        let mut registry = AccountRegistry::new(paths.clone())
            .with_extra_config_dirs(&cfg.flags.extra_config_dirs)
            .with_probe(Arc::new(DiskProbe::new(
                paths,
                platform.files.clone(),
                platform.processes.clone(),
            )));
        if let Some(bytes) = read_support(&cfg.roots.support, PersistFile::Accounts, &mut events) {
            if !registry.load(&bytes) {
                events.push(HubEvent::Log(format!(
                    "{} didn't parse; the accounts are found again",
                    PersistFile::Accounts.file_name()
                )));
            }
        }
        if let Some(bytes) = registry.file_bytes() {
            persists.saved.insert(PersistFile::Accounts, bytes);
        }
        let mut usage = UsageStore::with_config(UsageStoreConfig::for_run(
            cfg.roots.home.clone(),
            &cfg.flags,
            &settings,
        ));
        let base_env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os().collect();
        let env_path = base_env
            .iter()
            .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case("PATH"))
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        usage.set_probe_environment(ProbeEnvironment {
            roots: cfg.roots.clone(),
            base_env,
            env_path,
            claude_binary_path: settings.claude_binary_path.as_ref().map(Into::into),
        });
        let saved_usage = read_support(&cfg.roots.support, PersistFile::Usage, &mut events)
            .and_then(|bytes| UsageStateFile::parse(&bytes));
        let saved_review = read_support(&cfg.roots.support, PersistFile::Review, &mut events);
        let sessions = session_store(
            cfg.roots.paths(),
            &platform,
            &cfg.roots.claude_desktop,
            cfg.flags.sealed,
        );
        let ingress_w = IngressWiring::new(&cfg.pipe_name);
        let cloud = CloudState {
            website_url: cfg.website.clone(),
            ..CloudState::default()
        };
        let notify_permission = if cfg.flags.no_notifications {
            NotifyPermission::Unavailable
        } else {
            platform.notifier.permission()
        };
        Core {
            cfg,
            platform,
            settings,
            settings_file,
            settings_dirty: false,
            settings_write: None,
            registry,
            hooks,
            usage,
            sessions,
            cloud,
            panel: PanelState::default(),
            hotkey_ok: true,
            hotkey_message: None,
            transport_error: None,
            versions: Vec::new(),
            changed_folders: Vec::new(),
            window_names: BTreeMap::new(),
            notify_permission,
            snapshot_clock: project::SnapshotClock::default(),
            jobs: BTreeMap::new(),
            next_job: 1,
            outbox: Vec::new(),
            backlog: VecDeque::new(),
            events,
            live: false,
            persists,
            accounts_w: AccountsWiring::default(),
            usage_w: UsageWiring::new(saved_usage),
            hooks_w: HooksWiring::default(),
            ingress_w,
            sessions_w: SessionsWiring::new(saved_review),
            control_w: ControlWiring::default(),
            cloud_w: CloudWiring::default(),
        }
    }

    /// The hub starts: the launch discovery (when no call made it yet),
    /// then each store's schedule from now.
    pub(crate) fn on_start(&mut self) {
        let now = self.platform.clock.now();
        self.live = true;
        self.accounts_on_start(now);
        self.usage_on_start(now);
        self.hooks_on_start(now);
        self.sessions_on_start(now);
        self.ingress_on_start();
        self.control_on_start();
        self.cloud_on_start(now);
        self.after_input();
    }

    // ---- inputs ----

    /// One input, applied. `Stop` is the runtime's (it needs the lanes).
    pub(crate) fn handle(&mut self, input: Input) {
        match input {
            Input::Call { call, reply } => self.handle_call(call, reply),
            Input::JobDone { id, result } => self.job_done(id, result),
            Input::TypeCheckpoint { job, reply } => {
                let _ = reply.send(self.type_checkpoint(job));
            }
            Input::SetSetting { key, value } => {
                if let Err(e) = self.set_setting(&key, &value, false) {
                    self.log(format!("setting not changed: {}", e.message));
                }
            }
            Input::Transport(event) => self.transport_event(event),
            Input::Foreground(fg) => {
                let now = self.platform.clock.now();
                self.foreground_changed(fg, now);
            }
            Input::CloudState(state) => self.cloud_state_changed(state),
            // Deadlines are checked after every input.
            Input::Tick => {}
            Input::Stop { done } => {
                // Only the runtime's loop stops a core; one reaching here
                // (a stopped hub's inline call) has nothing to stop.
                let _ = done.send(());
            }
        }
    }

    /// A call: answered now, or kept with the job it waits for.
    pub(crate) fn handle_call(&mut self, call: Call, reply: Reply) {
        let answer = match call {
            Call::Snapshot => {
                let now = self.platform.clock.now();
                let projection = self.project(now);
                to_value(&projection.snapshot)
            }
            Call::Settings => {
                let now = self.platform.clock.now();
                to_value(&self.project_settings(now))
            }
            Call::SetSetting { key, value } => {
                self.set_setting(&key, &value, true).map(|()| json!({}))
            }
            Call::PanelState(state) => {
                self.panel_reported(state);
                Ok(json!({}))
            }
            Call::HotkeyStatus { ok, message } => {
                self.hotkey_ok = ok;
                self.hotkey_message = if ok { None } else { message };
                Ok(json!({}))
            }
            // Calls whose answer may wait for a job keep their reply.
            Call::Focus { session_id } => return self.focus_call(session_id, reply),
            Call::SendMessage { session_id, text } => {
                return self.send_message_call(session_id, &text, reply)
            }
            Call::MessageRoute { session_id } => return self.message_route_call(session_id, reply),
            Call::RefreshUsage { ring_id, reason } => {
                return self.refresh_usage_call(ring_id, reason, reply)
            }
            Call::HooksReinstall { account_id } => {
                return self.hooks_reinstall_call(account_id.as_deref(), reply)
            }
            Call::RemoveCodenotchHooks { folder } => {
                return self.remove_codenotch_call(&folder, reply)
            }
            Call::ChooseClaudeBinary { path } => {
                return self.choose_claude_binary_call(path.as_deref(), reply)
            }
            Call::HookConsent { grant } => self.hook_consent_call(grant),
            Call::HooksEnabled { on } => self.hooks_enabled_call(on),
            Call::StatusLineEnabled { on } => self.status_line_call(on),
            Call::AcknowledgeScope => self.acknowledge_scope_call(),
            Call::Account { action } => self.account_call(action),
            Call::LaunchCommand { account_id } => self.launch_command_call(&account_id),
            Call::RevealTarget { kind, id } => self.reveal_target_call(kind, &id),
            Call::Answer {
                session_id,
                tool_use_id,
                answer,
            } => self.answer_call(&session_id, &tool_use_id, answer),
            Call::MarkReviewed { session_id, at_ms } => self.mark_reviewed_call(session_id, at_ms),
            Call::MarkViewed {
                session_id,
                completed_at_ms,
            } => self.mark_viewed_call(session_id, completed_at_ms),
            Call::MarkAllReviewed { session_ids, at_ms } => self.mark_all_call(session_ids, at_ms),
            Call::DismissFailure { session_id } => self.dismiss_failure_call(session_id),
            Call::ResetReviewQueue => self.reset_review_call(),
            Call::SessionStateText => self.session_state_text_call(),
            Call::ChatOpen { session_id } => self.chat_open_call(&session_id),
            Call::ChatClose { session_id } => self.chat_close_call(&session_id),
            Call::ChatMore {
                session_id,
                before_id,
            } => self.chat_more_call(&session_id, &before_id),
            Call::ChatImage {
                session_id,
                image_id,
            } => self.chat_image_call(&session_id, &image_id),
            Call::Cloud { action, on } => self.cloud_call(action, on),
            Call::CloudUrl { target } => self.cloud_url_call(target),
        };
        let _ = reply.send(answer);
    }

    /// `an-core`'s fresh check before a typed reply is sent (§4.8).
    fn type_checkpoint(&mut self, job: JobId) -> bool {
        self.control_checkpoint(job)
    }

    // ---- settings ----

    /// Sets one setting: `from_page` holds it to the keys a page may set;
    /// the cloud thread's `Input::SetSetting` may set any. An unchanged
    /// value writes nothing.
    pub(crate) fn set_setting(
        &mut self,
        key: &str,
        value: &Value,
        from_page: bool,
    ) -> Result<(), CallError> {
        let next = if from_page {
            self.settings.validated_from_page(key, value)?
        } else {
            self.settings.validated(key, value)?
        };
        self.replace_settings(next);
        Ok(())
    }

    /// Settings `an-core` made up itself (the cloud's device id): kept, and
    /// in the file the next write makes, but no write of their own.
    pub(crate) fn adopt_settings_quietly(&mut self, next: ControlSettings) {
        self.settings = next;
        self.settings_file.apply(&self.settings);
    }

    /// The settings after a change any package made: saved (through
    /// `an-core`'s one writer) when they differ.
    pub(crate) fn replace_settings(&mut self, next: ControlSettings) {
        if next == self.settings {
            return;
        }
        let old = std::mem::replace(&mut self.settings, next);
        self.settings_file.apply(&self.settings);
        self.settings_dirty = true;
        self.usage_settings_changed(&old);
    }

    /// Hands the settings write out when one is due and none is in flight.
    fn schedule_settings_write(&mut self) {
        if !self.settings_dirty || self.settings_write.is_some() {
            return;
        }
        self.settings_dirty = false;
        let bytes = self.settings_file.encode();
        let id = self.schedule(
            Job::Persist {
                file: PersistFile::Settings,
                bytes,
            },
            None,
        );
        self.settings_write = Some(id);
    }

    /// The settings write in flight, if any (a stop waits for it, so the
    /// last write is the newest).
    pub(crate) fn settings_write_in_flight(&self) -> Option<JobId> {
        self.settings_write
    }

    // ---- jobs ----

    /// Queues `job`; `reply` is answered when it is back.
    pub(crate) fn schedule(&mut self, job: Job, reply: Option<Reply>) -> JobId {
        let id = JobId(self.next_job);
        self.next_job += 1;
        self.jobs.insert(
            id,
            PendingJob {
                lane: job.lane(),
                reply,
            },
        );
        self.outbox.push((id, job));
        id
    }

    /// A job came back.
    pub(crate) fn job_done(&mut self, id: JobId, result: JobResult) {
        let Some(pending) = self.jobs.remove(&id) else {
            return;
        };
        let mut reply = pending.reply;
        let now = self.platform.clock.now();
        match result {
            JobResult::Persisted(outcome) if self.settings_write == Some(id) => {
                self.settings_write = None;
                if let Err(why) = outcome {
                    // Written again with the next change (never in a loop).
                    self.log(format!("settings not saved: {why}"));
                }
            }
            JobResult::Persisted(outcome) => self.persisted(id, outcome),
            JobResult::Folders(snapshot) => self.folders_read(id, snapshot, now),
            JobResult::ClaudeJson(read) => self.claude_json_read(id, read, now),
            JobResult::Desktop(reading) => self.desktop_read(id, reading, now),
            JobResult::Probe(result) => self.probe_done(id, result, now),
            JobResult::Installed(outcomes) => self.installed(id, outcomes, &mut reply),
            JobResult::HookStatus(statuses) => self.hook_statuses_read(id, statuses),
            JobResult::Versions(sightings) => self.versions_read(id, sightings, now, &mut reply),
            JobResult::CodenotchRemoved { folder, result } => {
                self.codenotch_removed(id, &folder, result, &mut reply)
            }
            JobResult::Registry(snapshot) => self.registry_read(id, snapshot, now),
            JobResult::Transcript(delta) => self.transcript_synced(delta, now),
            JobResult::Chat(page) => self.chat_page_read(page, now),
            JobResult::Hosted(identity) => self.hosted_read(id, identity, now),
            JobResult::Host(host) => self.host_read(id, host, now),
            JobResult::Console(info) => self.console_read(id, info, now),
            JobResult::Focus(outcome) => self.focus_done(id, outcome, &mut reply, now),
            JobResult::Typed(outcome) => self.typed(id, outcome, &mut reply),
            JobResult::Visible {
                any_terminal,
                full_screen,
            } => self.visible_read(id, any_terminal, full_screen, now),
        }
        // A call whose job came back with nothing to answer it (a result its
        // wiring no longer expected) is still answered, never left to time out.
        if let Some(reply) = reply {
            let _ = reply.send(Err(CallError::failed("That didn't finish. Try again.")));
        }
    }

    // ---- the engine's files ----

    /// Writes one of the engine's files (not the settings, which have their
    /// own writer): now, or after the write of it in flight.
    pub(crate) fn persist(&mut self, file: PersistFile, bytes: Vec<u8>) {
        let newest = self
            .persists
            .next
            .get(&file)
            .or_else(|| self.persists.in_flight.get(&file).map(|(_, bytes)| bytes))
            .or_else(|| self.persists.saved.get(&file));
        if newest == Some(&bytes) {
            return;
        }
        if self.persists.in_flight.contains_key(&file) {
            self.persists.next.insert(file, bytes);
            return;
        }
        let id = self.schedule(
            Job::Persist {
                file,
                bytes: bytes.clone(),
            },
            None,
        );
        self.persists.in_flight.insert(file, (id, bytes));
    }

    fn persisted(&mut self, id: JobId, outcome: Result<(), String>) {
        let Some(file) = self
            .persists
            .in_flight
            .iter()
            .find(|(_, (job, _))| *job == id)
            .map(|(file, _)| *file)
        else {
            if let Err(why) = outcome {
                self.log(format!("not saved: {why}"));
            }
            return;
        };
        let bytes = self
            .persists
            .in_flight
            .remove(&file)
            .map(|(_, bytes)| bytes);
        match outcome {
            Ok(()) => {
                if let Some(bytes) = bytes {
                    self.persists.saved.insert(file, bytes);
                }
            }
            Err(why) => self.log(format!("not saved: {why}")),
        }
        if let Some(next) = self.persists.next.remove(&file) {
            self.persist(file, next);
        }
    }

    /// Jobs not back yet, by lane (the doctor's and the tests' view).
    pub(crate) fn jobs_in_flight(&self, lane: Lane) -> usize {
        self.jobs.values().filter(|job| job.lane == lane).count()
    }

    /// After every input: writes that became due, and while the hub runs,
    /// the stores' own work that became due.
    pub(crate) fn after_input(&mut self) {
        self.schedule_settings_write();
        if self.live {
            let now = self.platform.clock.now();
            self.drive_accounts(now);
            self.drive_usage(now);
            self.drive_sessions(now);
            self.drive_control(now);
            self.drive_cloud(now);
        }
        // Calls waiting on lookups are answered on a stopped hub too.
        let now = self.platform.clock.now();
        self.resolve_waiters(now);
        self.publish_chats();
        // Hook writes asked for by a call run on a stopped hub too.
        let now = self.platform.clock.now();
        self.drive_hooks(now);
    }

    pub(crate) fn take_outbox(&mut self) -> Vec<(JobId, Job)> {
        std::mem::take(&mut self.outbox)
    }

    pub(crate) fn take_events(&mut self) -> Vec<HubEvent> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn log(&mut self, line: String) {
        self.events.push(HubEvent::Log(line));
    }

    pub(crate) fn push_event(&mut self, event: HubEvent) {
        self.events.push(event);
    }

    /// When the stores next need `an-core` without an input (their own
    /// deadlines; the projection's label boundaries are the runtime's).
    pub(crate) fn next_deadline(&self) -> Option<SystemTime> {
        if !self.live {
            return None;
        }
        [
            self.accounts_deadline(),
            self.usage_deadline(),
            self.hooks_deadline(),
            self.sessions_deadline(),
            self.control_deadline(),
            self.cloud_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    // ---- stop ----

    /// The hub stops: held requests are released first (each waiting hook
    /// exits with no output), calls still waiting are answered, and every
    /// store saved now, on this thread.
    pub(crate) fn stop(&mut self) {
        self.ingress_on_stop();
        for pending in self.jobs.values_mut() {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err(CallError::failed(STOPPING)));
            }
        }
        self.control_on_stop(STOPPING);
        self.usage_w.answer_waiting(STOPPING);
        self.hooks_w.answer_waiting(STOPPING);
        self.save_now();
        self.live = false;
        self.hooks.stop();
        self.usage.stop();
        self.cloud_on_stop();
    }

    /// Writes what is unsaved now, synchronously (a stop; the process may
    /// end right after).
    pub(crate) fn save_now(&mut self) {
        if self.settings_dirty || self.settings_write.is_some() {
            let bytes = self.settings_file.encode();
            match self.write_support_file(PersistFile::Settings, &bytes) {
                Ok(()) => self.settings_dirty = false,
                Err(why) => self.log(format!("settings not saved: {why}")),
            }
        }
        if self.live {
            if let Some(bytes) = self.registry.file_bytes() {
                self.save_if_newer(PersistFile::Accounts, bytes);
                self.registry.mark_saved();
            }
            if self.usage_w.restored {
                let bytes = self.usage.save_now().encode();
                self.save_if_newer(PersistFile::Usage, bytes);
            }
        }
        let record = HookInstallFile::from_model(self.hooks.record()).encode();
        self.save_if_newer(PersistFile::HookInstall, record);
        let now = self.platform.clock.now();
        if let Some(bytes) = self.review_bytes_now(now) {
            self.save_if_newer(PersistFile::Review, bytes);
        }
    }

    /// Writes `bytes` now unless they are what the file was last written
    /// with (a write still in flight is overtaken: it carries older bytes
    /// or the same).
    fn save_if_newer(&mut self, file: PersistFile, bytes: Vec<u8>) {
        if self.persists.saved.get(&file) == Some(&bytes) {
            return;
        }
        match self.write_support_file(file, &bytes) {
            Ok(()) => {
                self.persists.next.remove(&file);
                self.persists.saved.insert(file, bytes);
            }
            Err(why) => self.log(format!("not saved: {why}")),
        }
    }

    fn write_support_file(&self, file: PersistFile, bytes: &[u8]) -> Result<(), String> {
        let files = self.platform.files.as_ref();
        let support = &self.cfg.roots.support;
        files
            .ensure_private_dir(support)
            .map_err(|e| format!("{}: {e}", support.display()))?;
        files
            .write_atomic(
                &support.join(file.file_name()),
                bytes,
                WriteMode::Private,
                Expect::Nothing,
            )
            .map(|_| ())
            .map_err(|e| format!("{}: {e}", file.file_name()))
    }

    // ---- projections ----

    /// Each identity's ring reading now.
    pub(crate) fn readings(&self, now: SystemTime) -> BTreeMap<IdentityId, RingReading> {
        self.registry
            .accounts()
            .iter()
            .map(|a| {
                (
                    a.identity_id.clone(),
                    self.usage.ring_reading(&a.identity_id, now),
                )
            })
            .collect()
    }

    fn setup_input(&self) -> SetupInput<'_> {
        SetupInput {
            registry: &self.registry,
            hooks: &self.hooks,
            settings: &self.settings,
            window_names: &self.window_names,
            transport_error: self.transport_error.as_deref(),
            sealed: false,
        }
    }

    fn ui(&self) -> UiSettings {
        UiSettings {
            hotkey_ok: self.hotkey_ok,
            hotkey_message: self.hotkey_message.clone(),
            ..self.settings.ui()
        }
    }

    /// Everything the pages are shown now.
    pub(crate) fn project(&mut self, now: SystemTime) -> Projection {
        let snapshot = self.project_snapshot(now);
        let settings = self.project_settings(now);
        let status = self.status(&snapshot, &settings);
        self.cloud_projected(now);
        Projection {
            snapshot,
            settings,
            status,
        }
    }

    fn project_snapshot(&mut self, now: SystemTime) -> HubSnapshot {
        let generated = self.snapshot_clock.next(crate::core::time::to_ms(now));
        self.snapshot_at(now, generated)
    }

    /// The snapshot at `now`, dated `generated_at_ms` (the reactions place
    /// sessions through it too, outside the snapshot clock).
    pub(crate) fn snapshot_at(&self, now: SystemTime, generated: u64) -> HubSnapshot {
        let accounts = self.registry.accounts();
        let readings = self.readings(now);
        let views = self.session_views();
        let ui = self.ui();
        let setup = setup_state(&self.setup_input());
        let directory: &dyn Directory = &self.registry;
        let (control, type_replies) = (&self.control_w, self.settings.type_replies);
        let extras = |view: &SessionView| row_extras(control, view, type_replies);
        project::project(
            &ProjectionInput {
                now,
                accounts: &accounts,
                directory,
                readings: &readings,
                sessions: &views,
                extras: &extras,
                clock: reset_clock(now),
                ui: &ui,
                setup: &setup,
                sealed: false,
            },
            generated,
        )
    }

    fn project_settings(&self, now: SystemTime) -> SettingsSnapshot {
        let readings = self.readings(now);
        let views = self.sessions.views();
        settings_snapshot(&SettingsInput {
            now,
            setup: self.setup_input(),
            readings: &readings,
            versions: &self.versions,
            changed_folders: &self.changed_folders,
            pipe_name: &self.cfg.pipe_name,
            busy: self.jobs_in_flight_of_install(),
            refreshing: self.jobs_in_flight(Lane::Probe) > 0,
            desktop_format: None,
            notify_permission: self.notify_permission,
            hotkey_ok: self.hotkey_ok,
            hotkey_message: self.hotkey_message.as_deref(),
            cloud: &self.cloud,
            session_count: views.len() as u32,
            review_count: views
                .iter()
                .filter(|v| v.state == SessionState::ReadyForReview)
                .count() as u32,
        })
    }

    /// An install or removal is running or waiting to.
    fn jobs_in_flight_of_install(&self) -> bool {
        self.hooks_w.busy()
    }

    /// `control status` (§4.14), from what the pages are shown.
    fn status(&self, snapshot: &HubSnapshot, settings: &SettingsSnapshot) -> ControlStatus {
        let shown: Vec<&RingSummary> = snapshot.rings.iter().filter(|r| r.shown).collect();
        ControlStatus {
            version: self.cfg.app_version.clone(),
            sealed: false,
            elevated: self.platform.device.elevated(),
            accounts: settings.accounts.len() as u32,
            rings: shown.len() as u32,
            readings: shown.iter().filter(|r| !r.usage.windows.is_empty()).count() as u32,
            sessions: snapshot.sessions.len() as u32,
            held: self.held_count(),
            hook_consent: consent_word(self.settings.hook_consent).into(),
            transport: self.ingress_w.status_word().into(),
            cloud: match self.cloud.auth {
                CloudAuthState::SignedIn { .. } => "signed_in",
                CloudAuthState::SigningIn => "signing_in",
                _ => "signed_out",
            }
            .into(),
            sync: self.cloud.sync_enabled,
        }
    }

    /// `--dump-state`: one line per session, never a prompt, reply, input
    /// or title (SessionDebugTools.swift's summary, minus the title).
    pub(crate) fn dump_lines(&self) -> Vec<String> {
        let mut views = self.session_views();
        views.sort_by(|a, b| {
            (a.state.bucket(), a.id.as_str()).cmp(&(b.state.bucket(), b.id.as_str()))
        });
        views.iter().map(dump_line).collect()
    }
}

/// The user's clock for reset times.
fn reset_clock(now: SystemTime) -> ResetClock {
    let offset = i64::try_from(crate::core::time::to_ms(now))
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or(0, |utc| {
            utc.with_timezone(&chrono::Local).offset().local_minus_utc()
        });
    ResetClock {
        utc_offset_seconds: offset,
        ..ResetClock::default()
    }
}

pub(crate) fn dump_line(view: &SessionView) -> String {
    let id: String = view.id.as_str().chars().take(8).collect();
    let account = view
        .account
        .as_ref()
        .and_then(|a| {
            std::path::Path::new(a.as_str())
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "-".into());
    let tasks = view
        .tasks
        .as_ref()
        .filter(|t| t.total > 0)
        .map_or("-".into(), |t| format!("{}/{}", t.done, t.total));
    let context = view
        .context_pct
        .map_or("-".into(), |pct| format!("{pct:.0}%"));
    let mut line = format!(
        "[agentnotch-state] {id} acct={account} attn={} phase={} tasks={tasks} ctx={context}",
        state_word(&view.state),
        phase_word(&view.phase),
    );
    if !view.pending.is_empty() {
        line.push_str(&format!(" pending={}", view.pending.len()));
    }
    line
}

fn state_word(state: &SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou(_) => "needsYou",
        SessionState::Failed(_) => "failed",
        SessionState::ReadyForReview => "readyForReview",
        SessionState::Working => "working",
        SessionState::Idle => "idle",
    }
}

fn phase_word(phase: &Phase) -> &'static str {
    match phase {
        Phase::Idle => "idle",
        Phase::Processing => "processing",
        Phase::WaitingForInput => "waitingForInput",
        Phase::WaitingForApproval(_) => "waitingForApproval",
        Phase::Compacting => "compacting",
        Phase::Ended => "ended",
    }
}

/// One of the engine's files in `<support>`, as read at load: `None` when
/// it isn't there (or can't be read, which is logged).
fn read_support(support: &Path, file: PersistFile, events: &mut Vec<HubEvent>) -> Option<Vec<u8>> {
    match std::fs::read(support.join(file.file_name())) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            events.push(HubEvent::Log(format!(
                "{} unreadable ({:?})",
                file.file_name(),
                e.kind()
            )));
            None
        }
    }
}

pub(crate) fn consent_word(consent: Option<bool>) -> &'static str {
    match consent {
        Some(true) => "granted",
        Some(false) => "declined",
        None => "unasked",
    }
}

pub(crate) fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, CallError> {
    serde_json::to_value(value).map_err(|e| CallError::failed(e.to_string()))
}
