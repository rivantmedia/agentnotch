//! The hub's public surface (§3.5): [`Hub`], the [`Call`]s the pages make
//! through `an_call`, and the [`HubEvent`]s the glue turns into `an:*`
//! events. The glue deserialises `{method, args}` with [`Call::from_parts`]
//! and gates it by the calling window with [`allowed_from_window`].
//!
//! Owner: WP0, then WP7.

use crate::core::flags::DevFlags;
use crate::model::*;
use crate::platform::{Clock, Platform, Roots};
use crate::runtime_types::{AnswerResult, PanelState};
use agentnotch_proto::ControlStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// What a hub is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubConfig {
    pub roots: Roots,
    /// Tauri's `package_info().version` (= `VERSION`).
    pub app_version: String,
    /// From `app-config.json`, validated (CL§3.2).
    pub website: Option<String>,
    pub flags: DevFlags,
    /// `<install dir>\agentnotch-hook.exe`.
    pub hook_exe: PathBuf,
    /// `proto::pipe_name(sid)` or the dev override.
    pub pipe_name: String,
}

/// An error answered to a call. `code`: `not_found` | `refused` | `invalid`
/// | `sealed` | `busy` | `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallError {
    pub code: String,
    pub message: String,
}

impl CallError {
    fn with(code: &str, message: impl Into<String>) -> Self {
        CallError {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::with("not_found", message)
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self::with("refused", message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::with("invalid", message)
    }

    pub fn sealed(message: impl Into<String>) -> Self {
        Self::with("sealed", message)
    }

    pub fn busy(message: impl Into<String>) -> Self {
        Self::with("busy", message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::with("failed", message)
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CallError {}

/// What an `agentnotch://` link did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeepLinkOutcome {
    SignInCompleted,
    SignInIgnored(String),
    Opened,
    Reviewed,
    Ignored(String),
}

/// What the doctor adds from outside the engine (§4.14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorExtras {
    pub exe: PathBuf,
    /// The doctor's `updates:` line.
    pub updates: String,
    pub deep_link: String,
    pub autostart: bool,
    pub shortcut_present: bool,
    /// Upstream's provider probe lines.
    pub providers: Vec<String>,
    /// A running instance's `control status`, when one answered.
    pub running: Option<ControlStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRefreshTrigger {
    RingClick,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudAction {
    SignIn,
    CancelSignIn,
    SignOut,
    SetSync,
    SetSummaries,
    SyncNow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudUrlTarget {
    Dashboard,
    Pools,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealKind {
    ConfigDir,
    SessionCwd,
    Backup,
}

/// Everything the pages (and the glue) can ask of the engine. JSON:
/// `{"method": "<snake_case name>", "args": {…}}`; methods without
/// arguments carry none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "args", rename_all = "snake_case")]
pub enum Call {
    /// → `HubSnapshot` (also pushed as `an:snapshot`).
    Snapshot,
    /// → `SettingsSnapshot` (also pushed as `an:settings`).
    Settings,
    /// → `AnswerReply`.
    Answer {
        session_id: SessionId,
        tool_use_id: String,
        answer: Answer,
    },
    /// → `OutcomeReply` (HS§8, two-phase, §4.8); refused while `typeReplies` is off.
    SendMessage {
        session_id: SessionId,
        text: String,
    },
    /// → `RouteReply`.
    MessageRoute {
        session_id: SessionId,
    },
    /// → `OutcomeReply`; success marks the session reviewed.
    Focus {
        session_id: SessionId,
    },
    MarkReviewed {
        session_id: SessionId,
        at_ms: u64,
    },
    MarkViewed {
        session_id: SessionId,
        completed_at_ms: u64,
    },
    /// The UI commits after its 5 s undo.
    MarkAllReviewed {
        session_ids: Vec<SessionId>,
        at_ms: u64,
    },
    DismissFailure {
        session_id: SessionId,
    },
    ResetReviewQueue,
    /// Opening marks reviewed; the chat is pushed as `an:chat`.
    ChatOpen {
        session_id: SessionId,
    },
    ChatClose {
        session_id: SessionId,
    },
    /// The next page (150 items).
    ChatMore {
        session_id: SessionId,
        before_id: String,
    },
    /// → `DataUrlReply` (≤ 2 MiB).
    ChatImage {
        session_id: SessionId,
        image_id: String,
    },
    /// → `ComingReply`.
    RefreshUsage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ring_id: Option<String>,
        reason: UsageRefreshTrigger,
    },
    /// "Turn on" / "Not now".
    HookConsent {
        grant: bool,
    },
    /// Off uninstalls everywhere and restores status lines.
    HooksEnabled {
        on: bool,
    },
    StatusLineEnabled {
        on: bool,
    },
    HooksReinstall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
    },
    /// → `RemovedReply`: the official app's entries, explicit only.
    RemoveCodenotchHooks {
        folder: String,
    },
    AcknowledgeScope,
    /// → `{}` or `ErrorReply` (`create` → `CreatedReply`).
    Account {
        action: AccountAction,
    },
    /// Keys: §4.12.
    SetSetting {
        key: String,
        value: Value,
    },
    /// → `VersionReply`; `None` = find automatically.
    ChooseClaudeBinary {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// All no-ops when sealed.
    Cloud {
        action: CloudAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
    },
    /// → `UrlReply`; the page asks the glue to open it.
    CloudUrl {
        target: CloudUrlTarget,
    },
    /// → `TextReply` (Advanced › Copy).
    SessionStateText,
    /// → `CommandReply` (the PowerShell form, §4.2).
    LaunchCommand {
        account_id: String,
    },
    /// → `PathReply`: the engine resolves a folder it knows; a page never
    /// supplies a path.
    RevealTarget {
        kind: RevealKind,
        id: String,
    },
    /// From the glue only.
    PanelState(PanelState),
    /// From the glue only, after each registration attempt.
    HotkeyStatus {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

/// Calls only the glue may make (refused from every page).
pub const GLUE_ONLY_METHODS: [&str; 2] = ["panel_state", "hotkey_status"];

/// Methods the glue answers itself (`calls.rs`); they never reach the engine.
pub const GLUE_METHODS: [&str; 14] = [
    "panel_toggle",
    "panel_open",
    "panel_close",
    "panel_route",
    "panel_report_size",
    "panel_take_focus",
    "panel_engaged",
    "open_settings",
    "open_url",
    "open_notification_settings",
    "copy_text",
    "reveal",
    "pick_folder",
    "log",
];

/// §3.7: which window may call which method (engine or glue method). The
/// panel may call everything but the glue-only calls; the notch reads the
/// snapshot, refreshes usage, jumps, marks reviewed, drives the panel, opens
/// Settings and logs; Settings may call everything but answering, typing
/// and the glue-only calls; drop zones nothing.
pub fn allowed_from_window(window_label: &str, method: &str) -> bool {
    if GLUE_ONLY_METHODS.contains(&method) {
        return false;
    }
    match window_label {
        "agentnotch-panel" => true,
        "notch" => {
            matches!(
                method,
                "snapshot" | "refresh_usage" | "focus" | "mark_reviewed" | "open_settings" | "log"
            ) || method.starts_with("panel_")
        }
        "settings" => !matches!(method, "answer" | "send_message"),
        _ => false,
    }
}

impl Call {
    /// `{method, args}` as the glue receives them. Arguments that are
    /// `null` or `{}` also fit a method that takes none.
    pub fn from_parts(method: &str, args: Value) -> Result<Call, CallError> {
        let with_args = serde_json::json!({ "method": method, "args": args });
        match serde_json::from_value::<Call>(with_args) {
            Ok(call) => Ok(call),
            Err(first) => {
                let empty = args.is_null() || args.as_object().is_some_and(|m| m.is_empty());
                if empty {
                    if let Ok(call) =
                        serde_json::from_value::<Call>(serde_json::json!({ "method": method }))
                    {
                        return Ok(call);
                    }
                }
                Err(CallError::invalid(format!("{method}: {first}")))
            }
        }
    }

    /// The method's name in JSON.
    pub fn method(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default()
    }
}

/// `answer` → `{result}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerReply {
    pub result: AnswerResult,
}

/// `send_message`, `focus` → `{outcome, reason?}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeReply {
    /// send_message: `delivered` | `refused` | `typed_not_submitted` |
    /// `failed`; focus: `focused` | `raised_only` | `not_found` | `failed`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `message_route` → `{available, reason?}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReply {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `refresh_usage` → `{coming}`: a fresher reading is on its way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComingReply {
    pub coming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedReply {
    pub removed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlReply {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextReply {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReply {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathReply {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataUrlReply {
    pub data_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionReply {
    pub version: Option<String>,
}

/// `account {create}` → the folder made and the command to sign in there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedReply {
    pub created: String,
    pub command: String,
}

/// `account` → `{error}` when it was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReply {
    pub error: String,
}

/// Engine → glue. The glue turns most into `an:*` events
/// ([`HubEvent::tauri_event`]); the rest are its own work.
// Moved once through a channel or a call; boxing variants would change the
// §3 signatures every package codes against.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubEvent {
    /// `an:snapshot` (coalesced ≥ 50 ms, only on change) → notch, panel.
    Snapshot(HubSnapshot),
    /// `an:settings` (only while the settings window exists).
    Settings(SettingsSnapshot),
    /// `an:cloud` → settings.
    Cloud(CloudState),
    /// `an:chat` (a reset or a patch) → panel.
    Chat(ChatUpdate),
    /// The glue opens and places the panel, then `an:panel`.
    Panel(PanelRequest),
    /// The glue hides the panel (auto-close, a jump unless pinned).
    PanelClose { reason: String },
    /// `an:peek` → notch.
    Peek { ring_id: String, seconds: u32 },
    /// `AppState.usage` replaced and upstream's `usage` emitted to all.
    UpstreamUsage(UpstreamUsage),
    /// The tray icon's dot on (> 0) or off.
    TrayBadge(u32),
    /// `an:notice` → notch.
    Notice(String),
    /// A line for upstream's run.log; the glue adds the `an: ` prefix. Never prompts, inputs,
    /// replies, tokens.
    Log(String),
    /// A graceful `app.exit(0)` (control op `quit`).
    Quit,
}

/// The glue's own event: the panel's keyboard focus was confirmed (or lost).
pub const PANEL_FOCUS_EVENT: &str = "an:panel_focus";
/// The glue's own event: an open panel moved or changed width (a [`PanelPlace`]).
///
/// [`PanelPlace`]: crate::model::PanelPlace
pub const PANEL_PLACE_EVENT: &str = "an:panel_place";
/// The glue's own event: the panel window's state (a `PanelState`), to the notch's page.
pub const PANEL_STATE_EVENT: &str = "an:panel_state";

impl HubEvent {
    /// The Tauri event the glue emits for it, when it is one.
    pub fn tauri_event(&self) -> Option<&'static str> {
        match self {
            HubEvent::Snapshot(_) => Some("an:snapshot"),
            HubEvent::Settings(_) => Some("an:settings"),
            HubEvent::Cloud(_) => Some("an:cloud"),
            HubEvent::Chat(_) => Some("an:chat"),
            HubEvent::Panel(_) => Some("an:panel"),
            HubEvent::Peek { .. } => Some("an:peek"),
            HubEvent::Notice(_) => Some("an:notice"),
            HubEvent::UpstreamUsage(_) => Some("usage"),
            HubEvent::PanelClose { .. }
            | HubEvent::TrayBadge(_)
            | HubEvent::Log(_)
            | HubEvent::Quit => None,
        }
    }

    /// The event's payload as the pages receive it.
    pub fn payload(&self) -> Value {
        let value = match self {
            HubEvent::Snapshot(s) => serde_json::to_value(s),
            HubEvent::Settings(s) => serde_json::to_value(s),
            HubEvent::Cloud(c) => serde_json::to_value(c),
            HubEvent::Chat(c) => serde_json::to_value(c),
            HubEvent::Panel(p) => serde_json::to_value(p),
            HubEvent::PanelClose { reason } => Ok(serde_json::json!({ "reason": reason })),
            HubEvent::Peek { ring_id, seconds } => {
                Ok(serde_json::json!({ "ring_id": ring_id, "seconds": seconds }))
            }
            HubEvent::UpstreamUsage(u) => serde_json::to_value(u),
            HubEvent::TrayBadge(n) => Ok(Value::from(*n)),
            HubEvent::Notice(text) | HubEvent::Log(text) => Ok(Value::String(text.clone())),
            HubEvent::Quit => Ok(Value::Null),
        };
        value.unwrap_or(Value::Null)
    }
}

/// Receives the hub's events (called from `an-core`).
pub type EventSink = Box<dyn Fn(&HubEvent) + Send + Sync>;

/// What runs behind a [`Hub`]: the sealed fixture hub, or the runtime.
pub(crate) trait HubBackend: Send + Sync {
    fn start(&self) -> Result<(), String>;
    fn stop(&self);
    fn on_event(&self, sink: EventSink);
    fn call(&self, call: Call) -> Result<Value, CallError>;
    fn snapshot(&self) -> HubSnapshot;
    fn settings_snapshot(&self) -> SettingsSnapshot;
    fn launch_rings(&self) -> Vec<RingSummary>;
    fn upstream_usage(&self) -> UpstreamUsage;
    fn handle_deep_link(&self, url: &str) -> DeepLinkOutcome;
    fn control_status(&self) -> ControlStatus;
    fn doctor_report(&self, extra: &DoctorExtras) -> String;
}

/// The engine, as the glue holds it. Cheap to clone.
#[derive(Clone)]
pub struct Hub {
    backend: Arc<dyn HubBackend>,
}

impl Hub {
    /// The live engine. Sealed flags give the sealed hub instead: a sealed
    /// run reaches nothing real whatever the caller passes.
    pub fn new(cfg: HubConfig, platform: Platform) -> Hub {
        if cfg.flags.sealed {
            return Hub::sealed(cfg, platform.clock);
        }
        super::runtime::live_hub(cfg, platform, super::runtime::RuntimeOptions::default()).0
    }

    pub(crate) fn with_backend(backend: Arc<dyn HubBackend>) -> Hub {
        Hub { backend }
    }

    /// Fixtures only; nothing is read, written, spawned or sent.
    pub fn sealed(cfg: HubConfig, clock: Arc<dyn Clock>) -> Hub {
        Hub {
            backend: Arc::new(super::sealed_fixture::SealedFixture::new(cfg, clock)),
        }
    }

    /// Spawns `an-core`, the workers, `an-cloud` and the transport.
    pub fn start(&self) -> Result<(), String> {
        self.backend.start()
    }

    /// Saves the stores now, releases held requests, kills children.
    pub fn stop(&self) {
        self.backend.stop()
    }

    pub fn on_event(&self, sink: EventSink) {
        self.backend.on_event(sink)
    }

    /// Blocking; at most 25 s.
    pub fn call(&self, call: Call) -> Result<Value, CallError> {
        self.backend.call(call)
    }

    pub fn snapshot(&self) -> HubSnapshot {
        self.backend.snapshot()
    }

    pub fn settings_snapshot(&self) -> SettingsSnapshot {
        self.backend.settings_snapshot()
    }

    /// Synchronous discovery at launch, so the rings exist from the start.
    pub fn launch_rings(&self) -> Vec<RingSummary> {
        self.backend.launch_rings()
    }

    /// `AppState.usage`'s shape (§4.6).
    pub fn upstream_usage(&self) -> UpstreamUsage {
        self.backend.upstream_usage()
    }

    /// `auth-callback` | `open` | `review`.
    pub fn handle_deep_link(&self, url: &str) -> DeepLinkOutcome {
        self.backend.handle_deep_link(url)
    }

    pub fn control_status(&self) -> ControlStatus {
        self.backend.control_status()
    }

    /// The doctor's text (§4.14).
    pub fn doctor_report(&self, extra: &DoctorExtras) -> String {
        self.backend.doctor_report(extra)
    }

    /// Read-only account inspection; no hub needed.
    pub fn inspect_accounts(roots: &Roots, platform: &Platform) -> String {
        crate::accounts::inspect::report(roots, platform.files.as_ref())
    }

    /// Removes this app's hooks from every folder it wrote (the
    /// `hook-install.json` record plus discovery).
    pub fn uninstall_hooks(roots: &Roots, platform: &Platform) -> Result<String, String> {
        let _ = (roots, platform);
        Err("Removing hooks isn't in this build yet.".to_owned())
    }
}
