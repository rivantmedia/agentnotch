//! The panel's window on the real desktop: `panel`'s [`WindowService`], over Tauri and
//! `agentnotch_win::window` (DESIGN-WIN §5.3). It decides nothing.
//!
//! Window work is queued on one thread, which posts each piece to the main thread: that keeps
//! the order the rules asked in, and it is never run inline (a window built inside a synchronous
//! command deadlocks WebView2; see `settings_window::open`).
//!
//! The panel is never given the foreground through Tauri's `set_focus`: tao falls back to a
//! synthetic Alt key press when Windows refuses, which is forcing it. `window::request_foreground`
//! is the plain `SetForegroundWindow`, and its answer is dropped: the rules look at who has the
//! foreground instead.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use agentnotch_win::window;
use serde_json::Value;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

use super::panel::{self, Timer, WindowService, LABEL, WIDTH};

const PAGE: &str = "agentnotch/panel.html";
/// The window's height until the page reports its content's.
const FIRST_HEIGHT: f64 = 560.0;

type Work = Box<dyn FnOnce(&AppHandle) + Send>;

pub(super) struct PanelWindow {
    queue: Mutex<Sender<Work>>,
}

/// The panel's `HWND` once the window is built (0 before): read from any thread, so the rules
/// never wait on the main thread for it.
static HANDLE: AtomicIsize = AtomicIsize::new(0);

fn handle() -> Option<isize> {
    match HANDLE.load(Ordering::Relaxed) {
        0 => None,
        handle => Some(handle),
    }
}

impl PanelWindow {
    pub(super) fn new(app: AppHandle) -> Self {
        let (queue, work) = mpsc::channel::<Work>();
        let _ = std::thread::Builder::new()
            .name("an-panel-window".into())
            .spawn(move || {
                for work in work {
                    let on_main = app.clone();
                    let _ = app.run_on_main_thread(move || work(&on_main));
                }
            });
        Self {
            queue: Mutex::new(queue),
        }
    }

    /// Queues window work for the main thread, after everything queued before it.
    fn post(&self, work: impl FnOnce(&AppHandle) + Send + 'static) {
        let _ = self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(Box::new(work));
    }
}

impl WindowService for PanelWindow {
    fn show(&self, request: &Value, activate: bool) {
        let request = request.clone();
        self.post(move |app| {
            let window = match app.get_webview_window(LABEL) {
                Some(window) => window,
                None => match build(app, &request, activate) {
                    Ok(window) => window,
                    Err(e) => {
                        super::log(&format!("panel window: {e}"));
                        return;
                    }
                },
            };
            let _ = window.show();
        });
    }

    fn hide(&self) {
        self.post(|app| {
            if let Some(window) = app.get_webview_window(LABEL) {
                let _ = window.hide();
            }
        });
    }

    fn set_focusable(&self, focusable: bool) {
        // Through Tauri, never SetWindowLongPtrW: tao rewrites the whole extended style from its
        // own flags, so a style set behind its back is lost at its next change.
        self.post(move |app| {
            if let Some(window) = app.get_webview_window(LABEL) {
                let _ = window.set_focusable(focusable);
            }
        });
    }

    fn request_foreground(&self) {
        self.post(|_| {
            if let Some(panel) = handle() {
                let _ = window::request_foreground(panel);
            }
        });
    }

    fn restore_foreground(&self, saved: isize) {
        self.post(move |_| {
            // Looked at again here: the window may have closed since the rules checked.
            if window::exists(saved) {
                let _ = window::request_foreground(saved);
            }
        });
    }

    fn foreground(&self) -> Option<isize> {
        window::foreground()
    }

    fn panel(&self) -> Option<isize> {
        handle()
    }

    fn exists(&self, handle: isize) -> bool {
        window::exists(handle)
    }

    fn raise_topmost(&self) {
        self.post(|_| {
            if let Some(panel) = handle() {
                let _ = window::raise_topmost(panel);
            }
        });
    }

    fn set_size(&self, width: f64, height: f64) {
        self.post(move |app| {
            if let Some(window) = app.get_webview_window(LABEL) {
                let _ = window.set_size(LogicalSize::new(width, height));
            }
        });
    }

    fn emit(&self, event: &str, payload: Value) {
        // Queued like window work, so an event never overtakes the window it is for.
        let event = event.to_string();
        self.post(move |app| {
            let _ = app.emit_to(LABEL, &event, payload);
        });
    }

    fn after(&self, ms: u64, timer: Timer) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            panel::timer(timer);
        });
    }
}

fn build(app: &AppHandle, request: &Value, focusable: bool) -> tauri::Result<WebviewWindow> {
    // The page learns the request it was opened for before its first script runs; later opens
    // arrive as `an:panel`. The theme comes the same way upstream's windows get it, so the
    // first frame is already in the right appearance.
    let script = format!(
        "{}\nwindow.__AGENTNOTCH_PANEL__ = {};",
        crate::theme_script(crate::resolved_theme(app)),
        request
    );
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title(super::PANEL_TITLE)
        .inner_size(WIDTH, FIRST_HEIGHT)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        // The "set focusable" asked for before this window existed reached nothing.
        .focusable(focusable)
        .focused(false)
        .visible(false)
        .center()
        .theme(crate::theme_choice(app))
        .initialization_script(script)
        .build()?;
    remember_handle(&window);
    window.on_window_event(|event| {
        if let WindowEvent::Focused(_) = event {
            panel::foreground_changed();
        }
    });
    Ok(window)
}

#[cfg(windows)]
fn remember_handle(window: &WebviewWindow) {
    match window.hwnd() {
        Ok(hwnd) => HANDLE.store(hwnd.0 as isize, Ordering::Relaxed),
        Err(e) => super::log(&format!("panel window handle: {e}")),
    }
}

/// Off Windows (the host build that runs the glue's tests) there is no handle to keep, so the
/// panel is never seen in the foreground and the keyboard gate stays shut.
#[cfg(not(windows))]
fn remember_handle(_window: &WebviewWindow) {}
