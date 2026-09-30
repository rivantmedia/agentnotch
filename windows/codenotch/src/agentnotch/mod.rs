//! Agent Notch's glue inside upstream's app crate (DESIGN-WIN §2.4). It **holds no logic**: it
//! copies fields between the hub (`agentnotch_engine`) and Tauri, and calls `agentnotch_win` for
//! window work. Everything that decides something lives in the engine.
//!
//! Upstream's files reach it only through the one-line seams of DESIGN-WIN §2.5 (each tagged
//! `// Fork: <id>` and listed in `Scripts/fork-seams.txt`); the entry points they call are the
//! public items of this module.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use agentnotch_engine::hub::{CallError, Hub};
use serde_json::Value;
use tauri::{AppHandle, WebviewWindow};

mod calls;
pub mod cli;
mod deeplink;
mod emit;
mod engine;
mod hotkey;
mod menu;
mod panel;
mod panel_window;
mod selftest;
mod setup;
mod tray;
mod update;
mod website;

pub use menu::{notch_menu_event, notch_menu_items};
pub use setup::setup;
pub use update::updater;

/// This app's name wherever upstream's copy says "Codenotch".
pub const DISPLAY_NAME: &str = "Agent Notch";
/// The Tauri identifier (`tauri.conf.json`; check-seams.sh pins both).
pub const IDENTIFIER: &str = "com.rivantmedia.agentnotch";
pub const SETTINGS_TITLE: &str = "Agent Notch Settings";
pub const DROPZONES_TITLE: &str = "Agent Notch drop zones";
pub const PANEL_TITLE: &str = "Agent Notch sessions";
/// Upstream's "Sign in" button on a Claude ring would start Claude Code's own browser login from
/// the app, part of the token path the fork never takes (seam WU1). The fork's projection never
/// offers the button; this is the answer should anything still invoke the command.
pub const SIGN_IN_REFUSED: &str =
    "Agent Notch never signs in to Claude itself. Run claude in a terminal, then /login.";
/// What upstream's own hooks switch says when Claude Code control hasn't been turned on (WH7).
const TURN_ON_FIRST: &str = "Turn on Claude Code control in Settings › Claude Code first.";

/// The version this build calls itself before Tauri is running (the CLI, the tray's first
/// tooltip). `tauri.conf.json`'s `"version"` is the same value: check-seams.sh and the build
/// script both require it to equal `VERSION`.
pub fn app_version() -> &'static str {
    include_str!("../../../../VERSION").trim()
}

/// Upstream copy with this app's name in it (tray and notch menu "Quit …", window titles).
pub fn rebrand(text: &str) -> String {
    engine::rebrand(text)
}

/// The tray icon's hover text when upstream has no readings to show (WR-TRAY), with the fork's
/// needs-you count when there is one (the tray dot's words, DESIGN-WIN §4.10).
pub fn tray_tooltip() -> String {
    match NEEDS_YOU.load(Ordering::Relaxed) {
        0 => format!("{DISPLAY_NAME} v{}", app_version()),
        n => format!("{DISPLAY_NAME} — {n} need you"),
    }
}

/// Sessions needing the user, as the hub last reported (`HubEvent::TrayBadge`).
static NEEDS_YOU: AtomicU32 = AtomicU32::new(0);

/// The weekly ring's place on a fresh install (WC): outside the session ring, the Mac fork's
/// default. Upstream's own fallback for an unknown value stays "off" (its test pins that).
pub fn default_weekly_ring() -> String {
    "outside".into()
}

/// Whether this run is sealed (`AGENTNOTCH_SAFE_MODE`, fails closed), decided once per process.
pub fn sealed() -> bool {
    static SEALED: OnceLock<bool> = OnceLock::new();
    *SEALED.get_or_init(engine::is_sealed)
}

/// Upstream's config folder name (WR-DIR): a sealed run keeps its config and logs apart, so it
/// never edits the real app's `config.json`.
pub fn data_folder_name() -> &'static str {
    engine::data_folder_name(sealed())
}

/// The running hub, once `setup` has started it.
pub fn hub() -> Option<Hub> {
    HUB.get().cloned()
}

static HUB: OnceLock<Hub> = OnceLock::new();

/// The one Tauri command of the fork (DESIGN-WIN §3.5, §3.7): `invoke('an_call', {method, args})`.
///
/// Async, so a slow call (focus, typing, a probe) runs off WebView2's thread.
#[tauri::command]
pub async fn an_call(
    app: AppHandle,
    window: WebviewWindow,
    method: String,
    args: Option<Value>,
) -> Result<Value, CallError> {
    calls::dispatch(
        app,
        window.label().to_string(),
        method,
        args.unwrap_or(Value::Null),
    )
    .await
}

/// A second launch forwarded by the single-instance plugin (WSI). Returns `true` when it was a
/// deep link (handled here), `false` to let upstream bring Settings forward as it always did.
pub fn second_instance(_app: &AppHandle, args: &[String]) -> bool {
    match deeplink::from_args(args) {
        Some(url) => {
            deeplink::handle(url.to_string());
            true
        }
        // Anything that merely looks like one is swallowed too: an `agentnotch:` argument is
        // never a request to open Settings, and never a subcommand (cli::run).
        None => args.iter().skip(1).any(|a| deeplink::names_scheme(a)),
    }
}

/// Upstream's refresh path for a Claude ring (WU1d): the notch's ring click, the tray's and the
/// notch menu's "Refresh". Answers whether a reading is on its way, as upstream's did.
///
/// Upstream reaches this from synchronous commands and menu handlers, which run on the main
/// thread, so the hub gets a short while to answer: the refresh goes ahead either way, only the
/// ring's press animation waits on the answer.
pub fn refresh_claude(_app: &AppHandle, provider: &str) -> bool {
    let Some(hub) = hub() else {
        return false;
    };
    // Upstream's own id ("claude") comes from "Refresh all": every ring, on request. A ring id
    // ("claude-acct-…") comes from a click on that ring.
    let args = if provider == "claude" {
        serde_json::json!({ "reason": "manual" })
    } else {
        serde_json::json!({ "ring_id": provider, "reason": "ring_click" })
    };
    calls::engine_call_within(hub, "refresh_usage", args, MAIN_THREAD_WAIT)
        .and_then(Result::ok)
        .and_then(|reply| reply.get("coming").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Upstream's "Let Claude Code notify …" switch (WH6): on only while Claude Code control is on.
pub fn hooks_switch_get() -> bool {
    hub().is_some_and(|hub| {
        let setup = hub.snapshot().setup;
        setup.hook_consent == Some(true) && !setup.control_off
    })
}

/// Upstream's switch turned on or off (WH7). Turning on needs the consent the Claude Code pane
/// asks for; upstream's switch never grants it on its own. A synchronous command (the main
/// thread): the hub gets a short while to answer, and the change goes ahead either way.
pub fn hooks_switch_set(on: bool) -> Result<String, String> {
    let hub =
        hub().ok_or_else(|| format!("{DISPLAY_NAME}'s Claude Code control isn't running."))?;
    if on && hub.snapshot().setup.hook_consent != Some(true) {
        return Err(TURN_ON_FIRST.into());
    }
    let state = if on { "on" } else { "off" };
    match calls::engine_call_within(
        hub,
        "hooks_enabled",
        serde_json::json!({ "on": on }),
        MAIN_THREAD_WAIT,
    ) {
        Some(Ok(_)) => Ok(format!("Claude Code hooks are {state}.")),
        Some(Err(e)) => Err(e.message),
        None => Ok(format!("Claude Code hooks are being turned {state}.")),
    }
}

/// How long a call made on the main thread waits for the hub before the UI moves on.
const MAIN_THREAD_WAIT: std::time::Duration = std::time::Duration::from_millis(750);

fn log(line: &str) {
    crate::applog(&format!("an: {line}"));
}
