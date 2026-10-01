//! The sealed hub: it serves the ui-contract fixtures
//! (`tests/ui-contract/*.json`), answers every [`Call`] from them and emits
//! the `an:*` events, so the glue and the pages can be built and self-tested
//! before the engine exists (milestone M1). Nothing is read, written,
//! spawned or sent: the fixtures are compiled in, and calls change only the
//! in-memory copy (answering a request, marking reviewed, a setting, …) so a
//! self-test sees its clicks take effect.
//!
//! The fixtures' times are shifted to "now" when the hub is made, so the
//! elapsed labels read as they were written ("2m", "5m ago").
//!
//! Replaced by WP7's sealed demo (ports of the Mac's `SampleLayout`,
//! `SampleSessions`, `SampleData`), whose snapshot must equal the fixture.

use super::api::*;
use crate::core::settings::ControlSettings;
use crate::core::time;
use crate::model::*;
use crate::platform::Clock;
use crate::runtime_types::{AnswerResult, PanelState};
use agentnotch_proto::ControlStatus;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard};

pub const SNAPSHOT_FIXTURE: &str = include_str!("../../tests/ui-contract/snapshot.json");
pub const SETTINGS_FIXTURE: &str = include_str!("../../tests/ui-contract/settings.json");
pub const CHAT_FIXTURE: &str = include_str!("../../tests/ui-contract/chat.json");

const SEALED_TYPING: &str = "Sealed: nothing is typed into a terminal.";
const TYPING_OFF: &str = "Typing replies is off. Turn it on in Settings › Claude Code.";
/// A 1×1 transparent PNG: every chat image of the fixture.
const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

struct State {
    snapshot: HubSnapshot,
    settings: SettingsSnapshot,
    control: ControlSettings,
    chat: ChatUpdate,
    chat_revision: u64,
    panel: PanelState,
    started: bool,
}

pub(crate) struct SealedFixture {
    cfg: HubConfig,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    sinks: Mutex<Vec<Arc<EventSink>>>,
}

/// Moves every time in a fixture by `delta_ms`: fields named `*_at_ms` and
/// `since_ms`, and upstream's `resets_at` / `fetched_at` (epoch ms too).
/// Durations (`duration_ms`, …) stay as they are.
fn shift_times(value: &mut Value, delta_ms: i64) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                let is_time = key.ends_with("_at_ms")
                    || key == "since_ms"
                    || key == "resets_at"
                    || key == "fetched_at";
                match item {
                    Value::Number(n) if is_time => {
                        if let Some(ms) = n.as_u64() {
                            let moved = (ms as i64).saturating_add(delta_ms).max(0);
                            *item = Value::from(moved as u64);
                        }
                    }
                    other => shift_times(other, delta_ms),
                }
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| shift_times(item, delta_ms)),
        _ => {}
    }
}

fn load<T: serde::de::DeserializeOwned>(text: &str, delta_ms: i64, name: &str) -> T {
    let mut value: Value =
        serde_json::from_str(text).unwrap_or_else(|e| panic!("ui-contract {name}: {e}"));
    shift_times(&mut value, delta_ms);
    serde_json::from_value(value).unwrap_or_else(|e| panic!("ui-contract {name}: {e}"))
}

/// The fixture as a sealed run shows it at `now_ms` (times shifted).
pub fn fixture_snapshot(now_ms: u64) -> HubSnapshot {
    let generated: HubSnapshot =
        serde_json::from_str(SNAPSHOT_FIXTURE).expect("ui-contract snapshot.json");
    let delta = now_ms as i64 - generated.generated_at_ms as i64;
    load(SNAPSHOT_FIXTURE, delta, "snapshot.json")
}

impl SealedFixture {
    pub(crate) fn new(cfg: HubConfig, clock: Arc<dyn Clock>) -> SealedFixture {
        let now = time::to_ms(clock.now());
        let raw: HubSnapshot =
            serde_json::from_str(SNAPSHOT_FIXTURE).expect("ui-contract snapshot.json");
        let delta = now as i64 - raw.generated_at_ms as i64;
        let mut snapshot: HubSnapshot = load(SNAPSHOT_FIXTURE, delta, "snapshot.json");
        let mut settings: SettingsSnapshot = load(SETTINGS_FIXTURE, delta, "settings.json");
        let chat: ChatUpdate = load(CHAT_FIXTURE, delta, "chat.json");
        snapshot.sealed = true;
        settings.sealed = true;
        settings.hooks.pipe_name = cfg.pipe_name.clone();
        let control = control_from_ui(&snapshot.ui, &settings);
        let state = State {
            snapshot,
            settings,
            control,
            chat,
            chat_revision: 1,
            panel: PanelState::default(),
            started: false,
        };
        SealedFixture {
            cfg,
            clock,
            state: Mutex::new(state),
            sinks: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit(&self, events: Vec<HubEvent>) {
        let sinks: Vec<Arc<EventSink>> =
            self.sinks.lock().unwrap_or_else(|p| p.into_inner()).clone();
        for event in &events {
            for sink in &sinks {
                sink(event);
            }
        }
    }

    fn now_ms(&self) -> u64 {
        time::to_ms(self.clock.now())
    }

    /// The snapshot and settings after a change.
    fn changed(&self, state: &mut State) -> Vec<HubEvent> {
        recount(&mut state.snapshot);
        state.snapshot.generated_at_ms = self.now_ms();
        state.settings.setup = state.snapshot.setup.clone();
        state.settings.attention = state.snapshot.ui.clone();
        vec![
            HubEvent::Snapshot(state.snapshot.clone()),
            HubEvent::Settings(state.settings.clone()),
            HubEvent::TrayBadge(state.snapshot.tray_badge),
        ]
    }

    fn handle(&self, call: Call) -> (Result<Value, CallError>, Vec<HubEvent>) {
        let mut state = self.lock();
        let ok = |value: Value| Ok(value);
        match call {
            Call::Snapshot => {
                state.snapshot.generated_at_ms = self.now_ms();
                (to_value(&state.snapshot), Vec::new())
            }
            Call::Settings => (to_value(&state.settings), Vec::new()),
            Call::Answer {
                session_id,
                tool_use_id,
                answer: _,
            } => {
                let Some(row) = state.snapshot.sessions.iter_mut().find(|r| {
                    r.session_id == session_id.0
                        && r.pending
                            .as_ref()
                            .is_some_and(|p| p.tool_use_id == tool_use_id)
                }) else {
                    return (
                        to_value(&AnswerReply {
                            result: AnswerResult::NotPending,
                        }),
                        Vec::new(),
                    );
                };
                row.pending = None;
                become_working(row);
                let events = self.changed(&mut state);
                (
                    to_value(&AnswerReply {
                        result: AnswerResult::Delivered,
                    }),
                    events,
                )
            }
            Call::SendMessage { .. } => {
                let reason = if state.control.type_replies {
                    SEALED_TYPING
                } else {
                    TYPING_OFF
                };
                (
                    to_value(&OutcomeReply {
                        outcome: "refused".into(),
                        reason: Some(reason.into()),
                    }),
                    Vec::new(),
                )
            }
            Call::MessageRoute { .. } => {
                let reason = if state.control.type_replies {
                    SEALED_TYPING
                } else {
                    TYPING_OFF
                };
                (
                    to_value(&RouteReply {
                        available: false,
                        reason: Some(reason.into()),
                    }),
                    Vec::new(),
                )
            }
            Call::Focus { .. } => (
                to_value(&OutcomeReply {
                    outcome: "not_found".into(),
                    reason: Some("Sealed: no terminal to show.".into()),
                }),
                Vec::new(),
            ),
            Call::MarkReviewed { session_id, .. } | Call::MarkViewed { session_id, .. } => {
                let found = mark_reviewed(&mut state.snapshot, &session_id.0);
                let events = if found {
                    self.changed(&mut state)
                } else {
                    Vec::new()
                };
                (ok(json!({})), events)
            }
            Call::MarkAllReviewed { session_ids, .. } => {
                let mut any = false;
                for id in &session_ids {
                    any |= mark_reviewed(&mut state.snapshot, &id.0);
                }
                let events = if any {
                    self.changed(&mut state)
                } else {
                    Vec::new()
                };
                (ok(json!({})), events)
            }
            Call::DismissFailure { session_id } => {
                let mut found = false;
                if let Some(row) = state
                    .snapshot
                    .sessions
                    .iter_mut()
                    .find(|r| r.session_id == session_id.0 && r.failed)
                {
                    become_idle(row);
                    found = true;
                }
                let events = if found {
                    self.changed(&mut state)
                } else {
                    Vec::new()
                };
                (ok(json!({})), events)
            }
            Call::ResetReviewQueue => {
                let ids: Vec<String> = state
                    .snapshot
                    .sessions
                    .iter()
                    .filter(|r| r.bucket == Bucket::ReadyForReview.as_str())
                    .map(|r| r.session_id.clone())
                    .collect();
                for id in &ids {
                    mark_reviewed(&mut state.snapshot, id);
                }
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::ChatOpen { session_id } | Call::ChatMore { session_id, .. } => {
                if !state
                    .snapshot
                    .sessions
                    .iter()
                    .any(|r| r.session_id == session_id.0)
                {
                    return (Err(CallError::not_found("No such session.")), Vec::new());
                }
                state.chat_revision += 1;
                let mut chat = state.chat.clone();
                chat.session_id = session_id.0.clone();
                chat.revision = state.chat_revision;
                chat.reset = true;
                let mut events = Vec::new();
                if mark_reviewed(&mut state.snapshot, &session_id.0) {
                    events = self.changed(&mut state);
                }
                events.push(HubEvent::Chat(chat));
                (ok(json!({})), events)
            }
            Call::ChatClose { .. } => (ok(json!({})), Vec::new()),
            Call::ChatImage { .. } => (
                to_value(&DataUrlReply {
                    data_url: format!("data:image/png;base64,{PIXEL_PNG}"),
                }),
                Vec::new(),
            ),
            Call::RefreshUsage { .. } => (to_value(&ComingReply { coming: false }), Vec::new()),
            Call::HookConsent { grant } => {
                state.control.hook_consent = Some(grant);
                state.snapshot.setup.hook_consent = Some(grant);
                state.snapshot.setup.needs_hook_consent = false;
                state.snapshot.setup.control_off = grant && !state.control.hooks_enabled;
                state.settings.hooks.consent = Some(grant);
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::HooksEnabled { on } => {
                state.control.hooks_enabled = on;
                state.settings.hooks.enabled = on;
                state.snapshot.setup.control_off = !on && state.control.hook_consent == Some(true);
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::StatusLineEnabled { on } => {
                state.control.status_line_integration = on;
                state.settings.hooks.status_line = on;
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::HooksReinstall { .. } => (ok(json!({})), Vec::new()),
            Call::RemoveCodenotchHooks { .. } => {
                (to_value(&RemovedReply { removed: 0 }), Vec::new())
            }
            Call::AcknowledgeScope => {
                state.snapshot.setup.new_install_folders.clear();
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::Account { action } => self.account(&mut state, action),
            Call::SetSetting { key, value } => {
                match state.control.validated_from_page(&key, &value) {
                    Ok(next) => {
                        state.control = next;
                        // What the hot key service reported stays as it was.
                        let hotkey_ok = state.snapshot.ui.hotkey_ok;
                        let hotkey_message = state.snapshot.ui.hotkey_message.take();
                        state.snapshot.ui = state.control.ui();
                        state.snapshot.ui.hotkey_ok = hotkey_ok;
                        state.snapshot.ui.hotkey_message = hotkey_message;
                        state.settings.usage.interval_minutes =
                            state.control.usage_probe_interval_minutes;
                        state.settings.usage.desktop_cache =
                            state.control.reads_desktop_usage_cache;
                        state.settings.notifications.notify_needs_input =
                            state.control.notify_needs_input;
                        state.settings.notifications.notify_ready_for_review =
                            state.control.notify_ready_for_review;
                        let events = self.changed(&mut state);
                        (ok(json!({})), events)
                    }
                    Err(error) => (Err(error), Vec::new()),
                }
            }
            Call::ChooseClaudeBinary { .. } => {
                (to_value(&VersionReply { version: None }), Vec::new())
            }
            Call::Cloud { .. } => (ok(json!({})), Vec::new()),
            Call::CloudUrl { target } => {
                let cloud = &state.settings.cloud;
                let url = match target {
                    CloudUrlTarget::Dashboard => cloud.dashboard_url.clone(),
                    CloudUrlTarget::Pools => cloud.pools_url.clone(),
                    CloudUrlTarget::Settings => cloud.settings_url.clone(),
                };
                match url {
                    Some(url) => (to_value(&UrlReply { url }), Vec::new()),
                    None => (
                        Err(CallError::not_found(
                            "The website hasn't said where that is yet.",
                        )),
                        Vec::new(),
                    ),
                }
            }
            Call::SessionStateText => (
                to_value(&TextReply {
                    text: state_text(&state.snapshot),
                }),
                Vec::new(),
            ),
            Call::LaunchCommand { account_id } => {
                let command = state
                    .settings
                    .accounts
                    .iter()
                    .find(|a| a.identity_id == account_id || a.ring_id == account_id)
                    .and_then(|a| a.launch_command.clone());
                match command {
                    Some(command) => (to_value(&CommandReply { command }), Vec::new()),
                    None => (
                        Err(CallError::not_found("No launch command for that account.")),
                        Vec::new(),
                    ),
                }
            }
            Call::RevealTarget { .. } => (
                Err(CallError::sealed("Sealed: there is no folder to show.")),
                Vec::new(),
            ),
            Call::PanelState(panel) => {
                state.panel = panel;
                (ok(json!({})), Vec::new())
            }
            Call::HotkeyStatus { .. } => (ok(json!({})), Vec::new()),
        }
    }

    fn account(
        &self,
        state: &mut State,
        action: AccountAction,
    ) -> (Result<Value, CallError>, Vec<HubEvent>) {
        match action {
            AccountAction::Rename { id, label } => {
                let Some(row) = state
                    .settings
                    .accounts
                    .iter_mut()
                    .find(|a| a.identity_id == id)
                else {
                    return (
                        to_value(&ErrorReply {
                            error: "No such account.".into(),
                        }),
                        Vec::new(),
                    );
                };
                let name = label
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.trim().to_owned());
                row.has_custom_label = name.is_some();
                row.label = name.unwrap_or_else(|| row.default_label.clone());
                let (ring_id, label) = (row.ring_id.clone(), row.label.clone());
                for ring in state
                    .snapshot
                    .rings
                    .iter_mut()
                    .filter(|r| r.ring_id == ring_id)
                {
                    ring.label = label.clone();
                }
                for session in state
                    .snapshot
                    .sessions
                    .iter_mut()
                    .filter(|s| s.ring_id.as_deref() == Some(&ring_id))
                {
                    session.account_label = Some(label.clone());
                }
                let events = self.changed(state);
                (Ok(json!({})), events)
            }
            AccountAction::Track { id, on } => {
                let Some(row) = state
                    .settings
                    .accounts
                    .iter_mut()
                    .find(|a| a.identity_id == id)
                else {
                    return (
                        to_value(&ErrorReply {
                            error: "No such account.".into(),
                        }),
                        Vec::new(),
                    );
                };
                row.is_tracked = on;
                let events = self.changed(state);
                (Ok(json!({})), events)
            }
            AccountAction::RingShown { ring_id, on } => {
                for row in state
                    .settings
                    .accounts
                    .iter_mut()
                    .filter(|a| a.ring_id == ring_id)
                {
                    row.ring_shown = on;
                }
                for ring in state
                    .snapshot
                    .rings
                    .iter_mut()
                    .filter(|r| r.ring_id == ring_id)
                {
                    ring.shown = on;
                }
                let mut events = self.changed(state);
                events.push(HubEvent::UpstreamUsage(upstream_usage_from_rings(
                    &state.snapshot.rings,
                )));
                (Ok(json!({})), events)
            }
            AccountAction::Forget { .. }
            | AccountAction::AddFolder { .. }
            | AccountAction::Create { .. }
            | AccountAction::SuggestionDismiss { .. }
            | AccountAction::SuggestionAdd { .. } => (
                Err(CallError::sealed(
                    "Sealed: no folder is added, created or forgotten.",
                )),
                Vec::new(),
            ),
        }
    }
}

impl HubBackend for SealedFixture {
    fn start(&self) -> Result<(), String> {
        let events = {
            let mut state = self.lock();
            state.started = true;
            state.snapshot.generated_at_ms = self.now_ms();
            vec![
                HubEvent::Log("hub started (sealed fixtures)".into()),
                HubEvent::Snapshot(state.snapshot.clone()),
                HubEvent::Settings(state.settings.clone()),
                HubEvent::Cloud(state.settings.cloud.clone()),
                HubEvent::UpstreamUsage(upstream_usage_from_rings(&state.snapshot.rings)),
                HubEvent::TrayBadge(state.snapshot.tray_badge),
            ]
        };
        self.emit(events);
        Ok(())
    }

    fn stop(&self) {
        self.lock().started = false;
    }

    fn on_event(&self, sink: EventSink) {
        self.sinks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(sink));
    }

    fn call(&self, call: Call) -> Result<Value, CallError> {
        let (reply, events) = self.handle(call);
        self.emit(events);
        reply
    }

    fn snapshot(&self) -> HubSnapshot {
        let mut state = self.lock();
        state.snapshot.generated_at_ms = self.now_ms();
        state.snapshot.clone()
    }

    fn settings_snapshot(&self) -> SettingsSnapshot {
        self.lock().settings.clone()
    }

    fn launch_rings(&self) -> Vec<RingSummary> {
        self.lock().snapshot.rings.clone()
    }

    fn upstream_usage(&self) -> UpstreamUsage {
        upstream_usage_from_rings(&self.lock().snapshot.rings)
    }

    fn handle_deep_link(&self, _url: &str) -> DeepLinkOutcome {
        DeepLinkOutcome::Ignored("Sealed: deep links are ignored.".into())
    }

    fn control_status(&self) -> ControlStatus {
        let state = self.lock();
        let snapshot = &state.snapshot;
        let shown: Vec<&RingSummary> = snapshot.rings.iter().filter(|r| r.shown).collect();
        ControlStatus {
            version: self.cfg.app_version.clone(),
            sealed: true,
            elevated: false,
            accounts: state.settings.accounts.len() as u32,
            rings: shown.len() as u32,
            readings: shown.iter().filter(|r| !r.usage.windows.is_empty()).count() as u32,
            sessions: snapshot.sessions.len() as u32,
            held: snapshot
                .sessions
                .iter()
                .filter(|r| r.pending.is_some())
                .count() as u32,
            hook_consent: consent_word(snapshot.setup.hook_consent).into(),
            transport: "off".into(),
            cloud: match state.settings.cloud.auth {
                CloudAuthState::SignedIn { .. } => "signed_in",
                CloudAuthState::SigningIn => "signing_in",
                _ => "signed_out",
            }
            .into(),
            sync: state.settings.cloud.sync_enabled,
        }
    }

    fn doctor_report(&self, extra: &DoctorExtras) -> String {
        let status = self.control_status();
        let state = self.lock();
        let roots = &self.cfg.roots;
        let mut lines = vec![
            format!(
                "Agent Notch doctor v{} ({})",
                self.cfg.app_version,
                crate::core::roots::IDENTIFIER
            ),
            format!("exe: {}", extra.exe.display()),
            "sealed: yes".to_owned(),
            "elevated: app=no running=no sessions-elevated=0".to_owned(),
            "smart-app-control: unknown".to_owned(),
            format!(
                "data: {}   support: {} (private: sealed, not used)",
                roots.data.display(),
                roots.support.display()
            ),
            extra.updates.clone(),
            format!("pipe: {} sealed (no server)", self.cfg.pipe_name),
            format!("hook exe: {}", self.cfg.hook_exe.display()),
            format!("accounts: {}", status.accounts),
        ];
        for account in &state.settings.accounts {
            lines.push(format!(
                "account: {} \"{}\" signed-in={} folders={}",
                account.ring_id,
                account.label,
                if account.is_signed_in { "yes" } else { "no" },
                account
                    .folders
                    .iter()
                    .map(|f| f.title.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        lines.push(format!(
            "hooks: consent={} installed=0/0 form=none exec-form-min=unset",
            status.hook_consent
        ));
        lines.push("claude-versions: none (sealed)".into());
        lines.push("status-line: wrapped=0 left-alone=0".into());
        lines.push("claude: not looked for (sealed)".into());
        lines.push("desktop-cache: absent".into());
        lines.push(format!("deep-link: {}", extra.deep_link));
        lines.push(format!(
            "autostart: {}",
            if extra.autostart { "on" } else { "off" }
        ));
        lines.push(format!(
            "notifications: {}",
            if extra.shortcut_present {
                "shortcut present (AUMID com.rivantmedia.agentnotch)"
            } else {
                "shortcut missing"
            }
        ));
        lines.push(format!("providers: {}", extra.providers.join("; ")));
        lines.join("\n") + "\n"
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, CallError> {
    serde_json::to_value(value).map_err(|e| CallError::failed(e.to_string()))
}

fn consent_word(consent: Option<bool>) -> &'static str {
    match consent {
        Some(true) => "granted",
        Some(false) => "declined",
        None => "unasked",
    }
}

/// The settings as the fixture shows them.
fn control_from_ui(ui: &UiSettings, settings: &SettingsSnapshot) -> ControlSettings {
    ControlSettings {
        hook_consent: settings.hooks.consent,
        hooks_enabled: settings.hooks.enabled,
        status_line_integration: settings.hooks.status_line,
        usage_probe_interval_minutes: settings.usage.interval_minutes,
        reads_desktop_usage_cache: settings.usage.desktop_cache,
        claude_binary_path: None,
        notify_needs_input: settings.notifications.notify_needs_input,
        notify_ready_for_review: settings.notifications.notify_ready_for_review,
        auto_open: ui.panel_open_mode.clone(),
        hold_open_while_needs_you: ui.hold_open.clone(),
        ring_badges: ui.ring_badges,
        resting_marks: ui.resting_marks,
        tray_badge: ui.tray_badge,
        ring_click: ui.ring_click.clone(),
        session_click: ui.hover_click.clone(),
        hot_key: ui.hotkey.clone(),
        panel_pinned: ui.panel_pinned,
        sound: ui.sound,
        peek: ui.peek,
        peek_seconds: ui.peek_seconds,
        type_replies: ui.type_replies,
        ..ControlSettings::default()
    }
}

fn become_working(row: &mut SessionRow) {
    row.bucket = Bucket::Working.as_str().into();
    row.failed = false;
    row.state_word = "working".into();
    row.detail = RowDetail::Working {
        text: "Thinking…".into(),
        secondary: true,
    };
    row.reviewable = false;
    row.card.state = "working".into();
    row.card.waiting_for = None;
}

fn become_idle(row: &mut SessionRow) {
    let text = match &row.detail {
        RowDetail::Review { text } | RowDetail::Failed { text } => text.clone(),
        _ => "No messages yet".into(),
    };
    row.bucket = Bucket::Idle.as_str().into();
    row.failed = false;
    row.state_word = "idle".into();
    row.detail = RowDetail::Idle { text };
    row.pending = None;
    row.reviewable = false;
    row.card.state = "idle".into();
    row.card.waiting_for = None;
}

/// A review row becomes idle; `false` when there was none to mark.
fn mark_reviewed(snapshot: &mut HubSnapshot, session_id: &str) -> bool {
    match snapshot
        .sessions
        .iter_mut()
        .find(|r| r.session_id == session_id && r.bucket == Bucket::ReadyForReview.as_str())
    {
        Some(row) => {
            become_idle(row);
            true
        }
        None => false,
    }
}

/// Totals, sections, ring counts and badges, the tray and the folded marks
/// from the rows (the sections keep their fixture order and folding).
fn recount(snapshot: &mut HubSnapshot) {
    let count = |rows: &[&SessionRow]| {
        let mut counts = Counts::default();
        for row in rows {
            match (row.bucket.as_str(), row.failed) {
                ("needs_you", true) => counts.failed += 1,
                ("needs_you", false) => counts.needs_you += 1,
                ("ready_for_review", _) => counts.review += 1,
                ("working", _) => counts.working += 1,
                _ => counts.idle += 1,
            }
        }
        counts
    };
    let all: Vec<&SessionRow> = snapshot.sessions.iter().collect();
    snapshot.totals = count(&all);
    // The list shows each section's rows together, in section order.
    let order = |bucket: &str| {
        Bucket::ALL
            .iter()
            .position(|b| b.as_str() == bucket)
            .unwrap_or(Bucket::ALL.len())
    };
    snapshot.sessions.sort_by_key(|row| order(&row.bucket));
    for ring in &mut snapshot.rings {
        let rows: Vec<&SessionRow> = snapshot
            .sessions
            .iter()
            .filter(|r| r.ring_id.as_deref() == Some(ring.ring_id.as_str()))
            .collect();
        ring.counts = count(&rows);
        ring.badges = Badges {
            needs_you: ring.counts.needs_you + ring.counts.failed,
            review: ring.counts.review,
        };
        ring.activity = if ring.counts.needs_you + ring.counts.failed > 0 {
            RingActivity::Waiting
        } else if ring.counts.working > 0 {
            RingActivity::Working
        } else if ring.counts.review > 0 {
            RingActivity::Success
        } else {
            RingActivity::Idle
        };
    }
    let totals = snapshot.totals;
    for section in &mut snapshot.sections {
        section.count = match section.bucket.as_str() {
            "needs_you" => totals.needs_you + totals.failed,
            "ready_for_review" => totals.review,
            "working" => totals.working,
            _ => totals.idle,
        };
    }
    snapshot.sections.retain(|s| s.count > 0);
    for bucket in Bucket::ALL {
        let count = match bucket {
            Bucket::NeedsYou => totals.needs_you + totals.failed,
            Bucket::ReadyForReview => totals.review,
            Bucket::Working => totals.working,
            Bucket::Idle => totals.idle,
        };
        if count > 0
            && !snapshot
                .sections
                .iter()
                .any(|s| s.bucket == bucket.as_str())
        {
            snapshot.sections.push(SectionInfo {
                bucket: bucket.as_str().into(),
                title: bucket.title().into(),
                count,
                fold_by_default: bucket == Bucket::Idle && count > 3,
            });
        }
    }
    snapshot.sections.sort_by_key(|s| order(&s.bucket));
    snapshot.tray_badge = totals.needs_you + totals.failed;
    snapshot.resting_marks.needs_you = totals.needs_you + totals.failed > 0;
    snapshot.resting_marks.review = totals.review > 0;
    snapshot.resting_marks.working = totals.working > 0;
}

/// One line per session, for bug reports.
fn state_text(snapshot: &HubSnapshot) -> String {
    let mut lines = Vec::new();
    for row in &snapshot.sessions {
        let mut line = format!(
            "{} [{}] {}",
            row.title,
            row.state_word,
            row.project.clone().unwrap_or_default()
        );
        if let Some(tasks) = &row.tasks {
            line.push_str(&format!(" tasks {}/{}", tasks.done, tasks.total));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// §4.6's projection for upstream code: every shown ring's windows; the
/// default ring (the one holding `~\.claude`, else the first shown) keeps
/// bare ids, the others get `<id>@<ring_id>`; `group` is the ring's label
/// when more than one ring is shown. Never `needsAuth`.
pub fn upstream_usage_from_rings(rings: &[RingSummary]) -> UpstreamUsage {
    let shown: Vec<&RingSummary> = rings.iter().filter(|r| r.shown).collect();
    let Some(default) = shown
        .iter()
        .find(|r| r.is_default)
        .or(shown.first())
        .copied()
    else {
        return UpstreamUsage {
            status: "none".into(),
            note: "No Claude account yet.".into(),
            ..UpstreamUsage::default()
        };
    };
    let several = shown.len() > 1;
    let mut windows = Vec::new();
    for ring in &shown {
        for window in &ring.usage.windows {
            let mut window = window.clone();
            if ring.ring_id != default.ring_id {
                window.id = format!("{}@{}", window.id, ring.ring_id);
            }
            window.group = several.then(|| ring.label.clone());
            windows.push(window);
        }
    }
    let (status, note) = match default.usage.status.as_str() {
        "ok" => ("ok", default.usage.note.clone()),
        "stale" => ("stale", default.usage.note.clone()),
        "waiting" => ("none", "Waiting for the first reading…".to_owned()),
        "sign_in_needed" => (
            "none",
            "Not signed in to Claude. Run claude, then /login.".to_owned(),
        ),
        "failed" => ("error", default.usage.note.clone()),
        _ => ("none", default.usage.note.clone()),
    };
    UpstreamUsage {
        status: status.into(),
        windows,
        fetched_at: shown
            .iter()
            .map(|r| r.usage.fetched_at_ms)
            .max()
            .unwrap_or(0),
        note,
        backoff_until: 0,
    }
}
