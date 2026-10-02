//! `Core`: everything `an-core` owns (design §1.2) — the settings, the
//! packages' stores, the jobs in flight and the calls waiting on them — and
//! what one input does to it. Only `an-core` touches a `Core` while the hub
//! runs (a stopped hub lends it to the caller of a call); nothing here
//! blocks on IO: blocking work leaves as a [`Job`] through the outbox and
//! comes back through [`Core::job_done`].
//!
//! The packages are wired in by the next sub-tasks, each at the seam named
//! for it below: accounts, usage and hooks (wp7-7), ingress, sessions,
//! review and chat (wp7-8), control (wp7-9), cloud (wp7-10).
//!
//! Owner: WP7.

use super::api::{Call, CallError, HubConfig, HubEvent};
use super::project::{self, Directory, ProjectionInput, RowExtras};
use super::project_settings::{settings_snapshot, setup_state, SettingsInput, SetupInput};
use crate::accounts::AccountRegistry;
use crate::attention::rows::ResetClock;
use crate::core::settings::ControlSettings;
use crate::hooks::HookManager;
use crate::model::*;
use crate::persist::settings::SettingsFile;
use crate::platform::{Expect, NotifyPermission, Platform, WriteMode};
use crate::runtime_types::*;
use crate::sessions::SessionStore;
use crate::usage::store::UsageStore;
use agentnotch_proto::ControlStatus;
use crossbeam_channel::Sender;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
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
    pub(crate) sessions: SessionStore,
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
        let hooks = HookManager::configured(cfg.hook_exe.clone(), &cfg.flags);
        let registry = AccountRegistry::new(cfg.roots.paths());
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
            usage: UsageStore::new(),
            sessions: SessionStore::new(),
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
        }
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
            // wp7-8: ingress → sessions; held requests.
            Input::Transport(_) => {}
            // wp7-9: "is the user looking at it" (mark viewed after 1.5 s).
            Input::Foreground(_) => {}
            // wp7-10: the cloud's published state.
            Input::CloudState(state) => self.cloud = state,
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
                // wp7-9 reads it for reactions and the auto-close tick.
                self.panel = state;
                Ok(json!({}))
            }
            Call::HotkeyStatus { ok, message } => {
                self.hotkey_ok = ok;
                self.hotkey_message = if ok { None } else { message };
                Ok(json!({}))
            }
            // Wired by the next sub-tasks (see the module doc).
            other => Err(CallError::failed(format!(
                "{} isn't available in this build yet.",
                method_name(&other)
            ))),
        };
        let _ = reply.send(answer);
    }

    /// `an-core`'s fresh check before a typed reply is sent (§4.8). Until
    /// typing is wired (wp7-9) nothing is ever submitted.
    fn type_checkpoint(&mut self, job: JobId) -> bool {
        let _ = job;
        false
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

    /// The settings after a change any package made: saved (through
    /// `an-core`'s one writer) when they differ.
    pub(crate) fn replace_settings(&mut self, next: ControlSettings) {
        if next == self.settings {
            return;
        }
        self.settings = next;
        self.settings_file.apply(&self.settings);
        self.settings_dirty = true;
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
        match result {
            JobResult::Persisted(outcome) if self.settings_write == Some(id) => {
                self.settings_write = None;
                if let Err(why) = outcome {
                    // Written again with the next change (never in a loop).
                    self.log(format!("settings not saved: {why}"));
                }
            }
            JobResult::Persisted(Err(why)) => self.log(format!("not saved: {why}")),
            // wp7-7..10 take their results here.
            _ => {}
        }
        if let Some(reply) = pending.reply {
            let _ = reply.send(Err(CallError::failed(
                "That isn't available in this build yet.",
            )));
        }
    }

    /// Jobs not back yet, by lane (the doctor's and the tests' view).
    pub(crate) fn jobs_in_flight(&self, lane: Lane) -> usize {
        self.jobs.values().filter(|job| job.lane == lane).count()
    }

    /// After every input: writes that became due.
    pub(crate) fn after_input(&mut self) {
        self.schedule_settings_write();
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

    /// When the stores next need `an-core` without an input (their own
    /// deadlines; the projection's label boundaries are the runtime's).
    pub(crate) fn next_deadline(&self) -> Option<SystemTime> {
        // wp7-7 (probe and discovery schedule, hook passes), wp7-8
        // (`SessionStore::next_deadline`, the interrupt watcher), wp7-9
        // (auto-close, typing checks).
        None
    }

    // ---- stop ----

    /// The hub stops: calls still waiting are answered, held requests
    /// released (wp7-8), and every store saved now, on this thread.
    pub(crate) fn stop(&mut self) {
        for pending in self.jobs.values_mut() {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err(CallError::failed(STOPPING)));
            }
        }
        self.save_now();
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
        // wp7-7 (accounts, usage `save_now`, hook record), wp7-8 (review).
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
    fn readings(&self, now: SystemTime) -> BTreeMap<IdentityId, RingReading> {
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
        Projection {
            snapshot,
            settings,
            status,
        }
    }

    fn project_snapshot(&mut self, now: SystemTime) -> HubSnapshot {
        let accounts = self.registry.accounts();
        let readings = self.readings(now);
        let views = self.sessions.views();
        let ui = self.ui();
        let setup = setup_state(&self.setup_input());
        let directory: &dyn Directory = &self.registry;
        // wp7-9 fills the host app, the jump and the typing availability.
        let extras = |_: &SessionView| RowExtras::default();
        let generated = self.snapshot_clock.next(crate::core::time::to_ms(now));
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

    /// An install or removal pass is running (wp7-7 marks those jobs).
    fn jobs_in_flight_of_install(&self) -> bool {
        false
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
            // wp7-8: the ingress's held requests.
            held: 0,
            hook_consent: consent_word(self.settings.hook_consent).into(),
            // wp7-8: the pipe's state.
            transport: "off".into(),
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
        let mut views = self.sessions.views();
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

fn dump_line(view: &SessionView) -> String {
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

pub(crate) fn consent_word(consent: Option<bool>) -> &'static str {
    match consent {
        Some(true) => "granted",
        Some(false) => "declined",
        None => "unasked",
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, CallError> {
    serde_json::to_value(value).map_err(|e| CallError::failed(e.to_string()))
}

/// A call's method name, as the pages spell it.
fn method_name(call: &Call) -> String {
    serde_json::to_value(call)
        .ok()
        .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "That".into())
}
