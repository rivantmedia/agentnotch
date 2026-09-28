//! The sessions panel's window, `agentnotch-panel` (DESIGN-WIN §2.4, §5.3).
//!
//! Created on first use and then kept, hidden between uses. Window work always runs on the main
//! thread, posted from another thread (a window built inside a synchronous command deadlocks
//! WebView2; see `settings_window::open`). Every change of what the panel shows is reported to
//! the hub as `panel_state`, which its reactions and auto-close rule read.
//!
//! What this build leaves to the panel's package (WP9): placing the panel beside its ring
//! (`geometry::panel`), `WS_EX_NOACTIVATE` for an auto-open, and the foreground confirmation
//! behind `an:panel_focus`. Until then the panel opens centred and no focus confirmation is ever
//! sent, so the page's keyboard gate stays shut (its safe state, §5.3).

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use agentnotch_engine::model::ui::PanelRequest;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};

pub(super) const LABEL: &str = "agentnotch-panel";
const PAGE: &str = "agentnotch/panel.html";
/// The panel's width until the geometry port places it (the list's side-edge width).
const WIDTH: f64 = 440.0;
/// The page reports its content height; the window follows within these bounds (§5.3).
const MIN_HEIGHT: f64 = 220.0;
const MAX_HEIGHT: f64 = 780.0;

/// What the panel shows, as the hub is told it (`PanelState`, §3.4).
struct Status {
    open: bool,
    route: Option<String>,
    ring_id: Option<String>,
    reason: Option<String>,
    engaged: bool,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    open: false,
    route: None,
    ring_id: None,
    reason: None,
    engaged: false,
});

fn status() -> std::sync::MutexGuard<'static, Status> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The hub asked for the panel (`HubEvent::Panel`).
pub(super) fn open_request(app: &AppHandle, request: PanelRequest) {
    let payload = serde_json::to_value(&request).unwrap_or(Value::Null);
    show(app, request.route, request.ring_id, request.reason, payload);
}

/// A page or the notch menu asked for the panel.
pub(super) fn open_route(app: &AppHandle, route: String, ring_id: Option<String>, reason: String) {
    let payload =
        json!({ "route": route, "ring_id": ring_id, "highlight": null, "reason": reason });
    show(app, route, ring_id, reason, payload);
}

/// A ring clicked: the same ring again closes the panel, another ring moves it there.
pub(super) fn toggle(app: &AppHandle, ring_id: Option<String>, reason: String) {
    let same = {
        let s = status();
        s.open && s.ring_id == ring_id
    };
    if same {
        close(app);
    } else {
        open_route(app, "sessions".into(), ring_id, reason);
    }
}

pub(super) fn close(app: &AppHandle) {
    {
        let mut s = status();
        if !s.open {
            return;
        }
        s.open = false;
        s.engaged = false;
    }
    on_main(app, |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
    report();
}

pub(super) fn set_route(route: String) {
    status().route = Some(route);
    report();
}

pub(super) fn set_engaged(on: bool) {
    status().engaged = on;
    report();
}

/// The page's natural content height (CSS px): the window follows it, within bounds.
pub(super) fn report_height(app: &AppHandle, height: f64) {
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    let height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    on_main(app, move |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.set_size(LogicalSize::new(WIDTH, height));
        }
    });
}

/// A click on the composer asks for the keyboard.
pub(super) fn take_focus(app: &AppHandle) {
    on_main(app, |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.set_focus();
        }
    });
}

fn show(app: &AppHandle, route: String, ring_id: Option<String>, reason: String, payload: Value) {
    {
        let mut s = status();
        s.open = true;
        s.route = Some(route);
        s.ring_id = ring_id;
        s.reason = Some(reason.clone());
    }
    // An automatic open never takes the keyboard from whatever the user is typing into (§5.3).
    let activate = reason != "auto";
    on_main(app, move |app| {
        let window = match app.get_webview_window(LABEL) {
            Some(window) => window,
            None => match build(app, &payload) {
                Ok(window) => window,
                Err(e) => {
                    super::log(&format!("panel window: {e}"));
                    return;
                }
            },
        };
        let _ = window.show();
        if activate {
            let _ = window.set_focus();
        }
        let _ = window.emit_to(LABEL, "an:panel", &payload);
    });
    report();
}

fn build(app: &AppHandle, request: &Value) -> tauri::Result<tauri::WebviewWindow> {
    // The page learns the request it was opened for before its first script runs; later opens
    // arrive as `an:panel`. The theme comes the same way upstream's windows get it, so the
    // first frame is already in the right appearance.
    let script = format!(
        "{}\nwindow.__AGENTNOTCH_PANEL__ = {};",
        crate::theme_script(crate::resolved_theme(app)),
        request
    );
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title(super::PANEL_TITLE)
        .inner_size(WIDTH, 560.0)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .visible(false)
        .center()
        .theme(crate::theme_choice(app))
        .initialization_script(script)
        .build()
}

/// Runs window work on the main thread, posted from another thread (never inline).
fn on_main(app: &AppHandle, work: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || work(&app));
    });
}

/// Tells the hub what the panel shows now.
///
/// Through one reporter thread, in order: this is reached from the hub's own event callback
/// too, where a call back into the hub would wait on itself, and reports sent from separate
/// threads could arrive out of order (a "closed" overtaken by the "open" before it).
fn report() {
    let state = {
        let s = status();
        json!({
            "open": s.open,
            "route": s.route,
            "ring_id": s.ring_id,
            "reason": s.reason,
            "engaged": s.engaged,
            // No foreground confirmation exists in this build (see the module note).
            "focused": false,
            "pinned": false,
        })
    };
    let _ = reporter()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(state);
}

fn reporter() -> &'static Mutex<Sender<Value>> {
    static REPORTER: OnceLock<Mutex<Sender<Value>>> = OnceLock::new();
    REPORTER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Value>();
        let _ = std::thread::Builder::new()
            .name("an-panel-state".into())
            .spawn(move || {
                for state in receiver {
                    if let Some(hub) = super::hub() {
                        if let Err(e) = super::calls::engine_call(&hub, "panel_state", state) {
                            super::log(&format!("panel_state: {}", e.message));
                        }
                    }
                }
            });
        Mutex::new(sender)
    })
}
