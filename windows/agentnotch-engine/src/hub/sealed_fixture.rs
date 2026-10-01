//! The sealed hub: it serves the sealed demo (`hub::sealed_demo`: the
//! fixture accounts, sessions and usage through the hub's own projections),
//! answers every [`Call`] on the demo's in-memory stores and emits the `an:*`
//! events, so the glue's sealed self-test and snapshots, and a sealed run,
//! show the pages working. Nothing is read, written, spawned or sent; calls
//! change only the demo (answering a request, marking reviewed, a setting,
//! renaming an account, …) so a self-test sees its clicks take effect.
//!
//! The demo is built at the clock's "now" when the hub is made and stays at
//! that instant: its elapsed labels ("2m", "5m ago") read as written however
//! long the run lasts (a snapshot run's pictures must not depend on how long
//! it took), and only `generated_at_ms` follows the clock. The ui-contract
//! fixtures are this demo made at their `generated_at_ms`.

use super::api::*;
use super::project;
use super::sealed_demo::{self, SealedDemo};
use crate::core::time;
use crate::model::*;
use crate::platform::Clock;
use crate::runtime_types::{AnswerResult, PanelState};
use agentnotch_proto::ControlStatus;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

const SEALED_TYPING: &str = "Sealed: nothing is typed into a terminal.";
const TYPING_OFF: &str = "Typing replies is off. Turn it on in Settings › Claude Code.";
/// A 1×1 transparent PNG: every chat image of the fixture.
const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

struct State {
    demo: SealedDemo,
    /// The demo's instant (see the module comment).
    at: SystemTime,
    snapshot: HubSnapshot,
    settings: SettingsSnapshot,
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

/// What a sealed run shows at `now_ms` (the demo made then).
pub fn fixture_snapshot(now_ms: u64) -> HubSnapshot {
    let at = time::from_ms(now_ms);
    SealedDemo::new(at, "").snapshot(at, now_ms)
}

impl SealedFixture {
    pub(crate) fn new(cfg: HubConfig, clock: Arc<dyn Clock>) -> SealedFixture {
        let at = clock.now();
        let demo = SealedDemo::new(at, &cfg.pipe_name);
        let snapshot = demo.snapshot(at, time::to_ms(at));
        let settings = demo.settings_snapshot(at);
        let state = State {
            demo,
            at,
            snapshot,
            settings,
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

    /// `generated_at_ms`: the clock, never before the last snapshot's.
    fn generated_at(&self, state: &State) -> u64 {
        time::to_ms(self.clock.now()).max(state.snapshot.generated_at_ms)
    }

    /// The snapshot and settings after a change, projected again.
    fn changed(&self, state: &mut State) -> Vec<HubEvent> {
        let generated = self.generated_at(state).saturating_add(1);
        state.snapshot = state.demo.snapshot(state.at, generated);
        state.settings = state.demo.settings_snapshot(state.at);
        vec![
            HubEvent::Snapshot(state.snapshot.clone()),
            HubEvent::Settings(state.settings.clone()),
            HubEvent::TrayBadge(state.snapshot.tray_badge),
            HubEvent::UpstreamUsage(project::upstream_usage(&state.snapshot.rings)),
        ]
    }

    fn handle(&self, call: Call) -> (Result<Value, CallError>, Vec<HubEvent>) {
        let mut state = self.lock();
        let at = state.at;
        let ok = |value: Value| Ok(value);
        match call {
            Call::Snapshot => {
                state.snapshot.generated_at_ms = self.generated_at(&state);
                (to_value(&state.snapshot), Vec::new())
            }
            Call::Settings => (to_value(&state.settings), Vec::new()),
            Call::Answer {
                session_id,
                tool_use_id,
                answer: _,
            } => {
                if !state.demo.answer(&session_id.0, &tool_use_id, at) {
                    let reply = AnswerReply {
                        result: AnswerResult::NotPending,
                    };
                    return (to_value(&reply), Vec::new());
                }
                let events = self.changed(&mut state);
                let reply = AnswerReply {
                    result: AnswerResult::Delivered,
                };
                (to_value(&reply), events)
            }
            Call::SendMessage { .. } => {
                let reason = typing_refusal(&state);
                let reply = OutcomeReply {
                    outcome: "refused".into(),
                    reason: Some(reason.into()),
                };
                (to_value(&reply), Vec::new())
            }
            Call::MessageRoute { .. } => {
                let reason = typing_refusal(&state);
                let reply = RouteReply {
                    available: false,
                    reason: Some(reason.into()),
                };
                (to_value(&reply), Vec::new())
            }
            Call::Focus { .. } => (
                to_value(&OutcomeReply {
                    outcome: "not_found".into(),
                    reason: Some("Sealed: no terminal to show.".into()),
                }),
                Vec::new(),
            ),
            Call::MarkReviewed { session_id, .. } | Call::MarkViewed { session_id, .. } => {
                let events = if state.demo.mark_reviewed(&session_id.0, at) {
                    self.changed(&mut state)
                } else {
                    Vec::new()
                };
                (ok(json!({})), events)
            }
            Call::MarkAllReviewed { session_ids, .. } => {
                let mut any = false;
                for id in &session_ids {
                    any |= state.demo.mark_reviewed(&id.0, at);
                }
                let events = if any {
                    self.changed(&mut state)
                } else {
                    Vec::new()
                };
                (ok(json!({})), events)
            }
            Call::DismissFailure { session_id } => {
                let events = if state.demo.dismiss_failure(&session_id.0, at) {
                    self.changed(&mut state)
                } else {
                    Vec::new()
                };
                (ok(json!({})), events)
            }
            Call::ResetReviewQueue => {
                for id in state.demo.reviewable() {
                    state.demo.mark_reviewed(&id, at);
                }
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::ChatOpen { session_id } | Call::ChatMore { session_id, .. } => {
                if !state.demo.has_session(&session_id.0) {
                    return (Err(CallError::not_found("No such session.")), Vec::new());
                }
                state.chat_revision += 1;
                let chat = sealed_demo::chat(&session_id.0, state.chat_revision);
                let mut events = Vec::new();
                if state.demo.mark_reviewed(&session_id.0, at) {
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
            // The answers are kept in memory: the setup state and the
            // Settings switches follow them, and nothing is installed.
            Call::HookConsent { grant } => {
                state.demo.control_mut().hook_consent = Some(grant);
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::HooksEnabled { on } => {
                state.demo.control_mut().hooks_enabled = on;
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::StatusLineEnabled { on } => {
                state.demo.control_mut().status_line_integration = on;
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::HooksReinstall { .. } => (ok(json!({})), Vec::new()),
            Call::RemoveCodenotchHooks { .. } => {
                (to_value(&RemovedReply { removed: 0 }), Vec::new())
            }
            Call::AcknowledgeScope => {
                let events = self.changed(&mut state);
                (ok(json!({})), events)
            }
            Call::Account { action } => self.account(&mut state, action),
            Call::SetSetting { key, value } => {
                match state.demo.control().validated_from_page(&key, &value) {
                    Ok(next) => {
                        *state.demo.control_mut() = next;
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
        let at = state.at;
        let unknown = || {
            let reply = ErrorReply {
                error: "No such account.".into(),
            };
            (to_value(&reply), Vec::new())
        };
        match action {
            AccountAction::Rename { id, label } => {
                if !state.demo.has_account(&id) {
                    return unknown();
                }
                state.demo.rename(&id, label.as_deref(), at);
            }
            AccountAction::Track { id, on } => {
                if !state.demo.has_account(&id) {
                    return unknown();
                }
                state.demo.track(&id, on, at);
            }
            AccountAction::RingShown { ring_id, on } => {
                state.demo.ring_shown(&ring_id, on, at);
            }
            AccountAction::Forget { .. }
            | AccountAction::AddFolder { .. }
            | AccountAction::Create { .. }
            | AccountAction::SuggestionDismiss { .. }
            | AccountAction::SuggestionAdd { .. } => {
                return (
                    Err(CallError::sealed(
                        "Sealed: no folder is added, created or forgotten.",
                    )),
                    Vec::new(),
                );
            }
        }
        let events = self.changed(state);
        (Ok(json!({})), events)
    }
}

/// Why nothing is typed: typing is off, or the run is sealed.
fn typing_refusal(state: &State) -> &'static str {
    if state.demo.control().type_replies {
        SEALED_TYPING
    } else {
        TYPING_OFF
    }
}

impl HubBackend for SealedFixture {
    fn start(&self) -> Result<(), String> {
        let events = {
            let mut state = self.lock();
            state.started = true;
            state.snapshot.generated_at_ms = self.generated_at(&state);
            vec![
                HubEvent::Log("hub started (sealed demo)".into()),
                HubEvent::Snapshot(state.snapshot.clone()),
                HubEvent::Settings(state.settings.clone()),
                HubEvent::Cloud(state.settings.cloud.clone()),
                HubEvent::UpstreamUsage(project::upstream_usage(&state.snapshot.rings)),
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
        state.snapshot.generated_at_ms = self.generated_at(&state);
        state.snapshot.clone()
    }

    fn settings_snapshot(&self) -> SettingsSnapshot {
        self.lock().settings.clone()
    }

    fn launch_rings(&self) -> Vec<RingSummary> {
        self.lock().snapshot.rings.clone()
    }

    fn upstream_usage(&self) -> UpstreamUsage {
        project::upstream_usage(&self.lock().snapshot.rings)
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
