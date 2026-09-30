//! The panel's window on the real desktop: `panel`'s [`WindowService`], over Tauri and
//! `agentnotch_win::window` (DESIGN-WIN §5.3). It decides nothing about what the panel shows or
//! who may type into it; it carries out what the rules ask and tells them what the desktop did.
//!
//! - **Order.** Window work is queued on one thread, which posts each piece to the main thread:
//!   that keeps the order the rules asked in, and it is never run inline (a window built inside
//!   a synchronous command deadlocks WebView2; see `settings_window::open`).
//! - **Where.** The engine's `geometry::panel` places the panel. This file only collects what it
//!   asks for: the notch window's frame and edge, the ring the notch's page measured (or the
//!   middle of the notch when nothing measured one), and the work area and scale of the monitor
//!   the panel lands on. With no notch to hang off (hidden, the ring off, no window) the panel
//!   floats on the pointer's monitor. It hangs there again 0.35 s after the notch moves, and
//!   closes when the monitor it hung on is gone.
//! - **Foreground.** The panel is never given the foreground through Tauri's `set_focus`: tao
//!   falls back to a synthetic Alt key press when Windows refuses, which is forcing it.
//!   `window::request_foreground` is the plain `SetForegroundWindow`, and its answer only
//!   decides whether the window flashes: the rules look at who has the foreground instead.
//! - **Above the notch.** Upstream's topmost watchdog raises the notch whenever another window
//!   comes to the front, and knows nothing of the panel, so the panel raises itself after it:
//!   on every notch move and every foreground change while it is open.
//! - **Outside clicks.** A panel that isn't the foreground window hears of no click elsewhere,
//!   so while it is open a thread looks at the mouse buttons every 50 ms; one that is loses the
//!   keyboard to whatever was clicked, which is how that case is seen.
//!
//! Every decision goes to run.log (rects, styles, reasons; never a route or anything a page
//! wrote): none of this can run on the Mac the port is written on.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use agentnotch_engine::geometry::panel::{
    self as geometry, PanelEdge, PanelInput, PanelMode, PanelPlacement, PxRect,
    UPRIGHT_TIP_INSET_CSS,
};
use agentnotch_win::window;
use serde_json::{json, Value};
use tauri::{
    AppHandle, Emitter, Listener, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use super::panel::{self, Place, RingRect, Timer, WindowService, LABEL};

const PAGE: &str = "agentnotch/panel.html";
/// Upstream's notch window.
const NOTCH: &str = "notch";
/// The content height (CSS px) the panel is placed for until its page reports its own.
const FIRST_HEIGHT: f64 = 560.0;
/// How long after the notch moved the panel hangs off it again: upstream places the notch in
/// several steps (size, zoom, position, then once more on a monitor at another scale).
const REANCHOR_AFTER: Duration = Duration::from_millis(350);
/// How often the mouse buttons and the foreground window are looked at while the panel is open.
const WATCH_EVERY: Duration = Duration::from_millis(50);
/// How often the foreground window is looked at all the time, to know which app to give the
/// keyboard back to.
const TRACK_EVERY: Duration = Duration::from_millis(250);
/// How long after the panel lost the keyboard the new foreground window is read: Windows names
/// it a moment after the old one hears it is no longer in front.
const BLUR_SETTLES_AFTER: Duration = Duration::from_millis(60);
/// Where the page learns that an open panel moved or changed width (`an:panel` itself is a
/// request to show a route, so it isn't sent again for this).
const PLACED_EVENT: &str = "an:panel_place";

type Work = Box<dyn FnOnce(&AppHandle) + Send>;

/// The real window service. It has no state of its own: there is one panel per process, and
/// what is known about it lives in this file's statics, where the main thread's work finds it.
pub(super) struct PanelWindow;

static QUEUE: OnceLock<Mutex<Sender<Work>>> = OnceLock::new();
/// The panel's `HWND` once the window is built (0 before): read from any thread, so the rules
/// never wait on the main thread for it.
static HANDLE: AtomicIsize = AtomicIsize::new(0);
/// The notch window's `HWND` (0 when unknown).
static NOTCH_HANDLE: AtomicIsize = AtomicIsize::new(0);
/// The last foreground window that belonged to another app (0 when none was seen).
static LAST_OTHER: AtomicIsize = AtomicIsize::new(0);
/// Whether the panel is on screen, as the main thread last left it.
static OPEN: AtomicBool = AtomicBool::new(false);
/// Numbers the watch threads: one that finds a newer number stops.
static WATCH: AtomicU64 = AtomicU64::new(0);
/// Numbers the notch's moves: only the latest one's timer re-anchors.
static MOVES: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<State> = Mutex::new(State::new());

fn atom(cell: &AtomicIsize) -> Option<isize> {
    match cell.load(Ordering::Relaxed) {
        0 => None,
        handle => Some(handle),
    }
}

fn handle() -> Option<isize> {
    atom(&HANDLE)
}

/// What the main thread knows about the panel on screen.
struct State {
    /// What the open panel was placed by.
    place: Option<Place>,
    /// The content's height in CSS px.
    height: f64,
    placed: Option<Placed>,
    /// The placement the page was last told.
    told: Option<Value>,
}

impl State {
    const fn new() -> Self {
        State {
            place: None,
            height: FIRST_HEIGHT,
            placed: None,
            told: None,
        }
    }
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A monitor: its whole area, the part the taskbar leaves free, and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Area {
    pub(super) monitor: PxRect,
    pub(super) work: PxRect,
    pub(super) scale: f64,
}

/// What the panel hangs off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Anchor {
    /// The notch's edge; `None` when the panel floats.
    pub(super) edge: Option<PanelEdge>,
    /// The ring (or the point standing in for it) on the screen; `None` when the panel floats.
    pub(super) ring: Option<PxRect>,
    /// The monitor the panel lands on.
    pub(super) area: Area,
    /// The notch window's frame when the ring was placed in it.
    notch: Option<PxRect>,
    /// Whether `ring` is the rect the notch's page measured (not the stand-in point).
    measured: bool,
}

/// Where the panel is, and what it was placed by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Placed {
    pub(super) anchor: Anchor,
    pub(super) placement: PanelPlacement,
}

/// Where the open panel was last put; `None` while it is closed.
pub(super) fn placed() -> Option<Placed> {
    state().placed
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
        let _ = QUEUE.set(Mutex::new(queue));
        PanelWindow
    }
}

/// Queues window work for the main thread, after everything queued before it. Before the panel
/// was ever asked for there is no queue, and nothing to do the work on.
fn post(work: impl FnOnce(&AppHandle) + Send + 'static) {
    if let Some(queue) = QUEUE.get() {
        let _ = queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(Box::new(work));
    }
}

impl WindowService for PanelWindow {
    fn show(&self, request: &Value, place: Place, activate: bool) {
        let request = request.clone();
        post(move |app| show_now(app, &request, place, activate));
    }

    fn hide(&self) {
        post(|app| {
            if let Some(window) = app.get_webview_window(LABEL) {
                let _ = window.hide();
            }
            OPEN.store(false, Ordering::Relaxed);
            // Stops the watch thread at its next look.
            WATCH.fetch_add(1, Ordering::Relaxed);
            *state() = State::new();
            super::log("panel hidden");
        });
    }

    fn set_focusable(&self, focusable: bool) {
        // Through Tauri, never SetWindowLongPtrW: tao rewrites the whole extended style from its
        // own flags, so a style set behind its back is lost at its next change.
        post(move |app| {
            if let Some(window) = app.get_webview_window(LABEL) {
                let result = window.set_focusable(focusable);
                super::log(&format!(
                    "panel focusable={focusable}: {}",
                    outcome(&result)
                ));
            }
        });
    }

    fn request_foreground(&self) {
        post(|_| {
            let Some(panel) = handle() else {
                return;
            };
            let accepted = window::request_foreground(panel);
            super::log(&format!(
                "panel asked for the foreground: accepted={accepted} foreground={} panel={}",
                hex(window::foreground()),
                hex(Some(panel))
            ));
            // Refused: the window says so the way Windows has windows say it. Whether the panel
            // has the keyboard is the rules' to find out, by looking.
            if !accepted {
                window::flash(panel);
            }
        });
    }

    fn restore_foreground(&self, saved: isize) {
        post(move |_| {
            // Looked at again here: the window may have closed since the rules checked.
            let exists = window::exists(saved);
            let accepted = exists && window::request_foreground(saved);
            super::log(&format!(
                "panel gave the foreground back to {}: exists={exists} accepted={accepted}",
                hex(Some(saved))
            ));
        });
    }

    fn foreground(&self) -> Option<isize> {
        window::foreground()
    }

    fn keyboard_owner(&self) -> Option<isize> {
        match window::foreground() {
            Some(front) if !is_own(front) => {
                // Noted at once: the tracker only looks four times a second.
                LAST_OTHER.store(front, Ordering::Relaxed);
                Some(front)
            }
            // One of the app's own windows (the notch, after a click on a ring), or none at all.
            _ => atom(&LAST_OTHER).filter(|other| window::exists(*other)),
        }
    }

    fn panel(&self) -> Option<isize> {
        handle()
    }

    fn exists(&self, handle: isize) -> bool {
        window::exists(handle)
    }

    fn raise_topmost(&self) {
        post(|_| raise_now());
    }

    fn set_content(&self, mode: PanelMode, height: Option<f64>) {
        post(move |app| {
            let placed = {
                let mut st = state();
                if let Some(height) = height {
                    st.height = height;
                }
                if let Some(place) = st.place.as_mut() {
                    place.mode = mode;
                }
                // The anchor stays as it was found: only the card's size follows the content.
                st.placed.map(|old| Placed {
                    anchor: old.anchor,
                    placement: placement(&old.anchor, mode, st.height),
                })
            };
            if let (true, Some(placed)) = (OPEN.load(Ordering::Relaxed), placed) {
                place_again(app, placed, "content");
            }
        });
    }

    fn emit(&self, event: &str, payload: Value) {
        // Queued like window work, so an event never overtakes the window it is for.
        let event = event.to_string();
        post(move |app| {
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

/// Follows the notch and the foreground window from the app's start (`setup`).
pub(super) fn start(app: &AppHandle) {
    // Upstream emits both whenever it has placed the notch: an edge change, a drag along the
    // edge, another size, another monitor, a taskbar that moved.
    for event in ["notch_edge", "notch_insets"] {
        app.listen_any(event, |_| notch_moved());
    }
    // Which app had the keyboard has to be known before the panel opens: by then a click on a
    // ring may already have made the notch the foreground window.
    let _ = std::thread::Builder::new()
        .name("an-panel-foreground".into())
        .spawn(|| {
            let mut last = None;
            loop {
                let front = window::foreground();
                if front != last {
                    last = front;
                    if let Some(front) = front.filter(|front| !is_own(*front)) {
                        LAST_OTHER.store(front, Ordering::Relaxed);
                    }
                }
                std::thread::sleep(TRACK_EVERY);
            }
        });
}

fn is_own(handle: isize) -> bool {
    window::process_of(handle) == Some(window::current_process_id())
}

fn raise_now() {
    if let Some(panel) = handle() {
        let _ = window::raise_topmost(panel);
    }
}

/// Upstream placed the notch again.
fn notch_moved() {
    if !OPEN.load(Ordering::Relaxed) {
        return;
    }
    // Its watchdog may have raised the notch over the panel.
    post(|_| raise_now());
    let mine = MOVES.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(REANCHOR_AFTER);
        // A later move has its own timer: only the last one places the panel.
        if MOVES.load(Ordering::Relaxed) == mine && OPEN.load(Ordering::Relaxed) {
            post(reanchor);
        }
    });
}

// ---- on the main thread ----

fn show_now(app: &AppHandle, request: &Value, place: Place, activate: bool) {
    remember_notch(app);
    // A panel already open keeps the height its content has; a closed one starts over.
    let height = state().height;
    let (anchor, why) = find_anchor(app, &place, None);
    let placed = anchor.map(|anchor| Placed {
        anchor,
        placement: placement(&anchor, place.mode, height),
    });
    let fields = placed.as_ref().map(place_fields);
    let payload = with_fields(request, fields.as_ref());
    let window = match app.get_webview_window(LABEL) {
        Some(window) => window,
        None => match build(app, &payload, activate) {
            Ok(window) => window,
            Err(e) => {
                super::log(&format!("panel window: {e}"));
                return;
            }
        },
    };
    if let Some(placed) = &placed {
        set_frame(&window, placed.placement.window);
    }
    {
        let mut st = state();
        st.place = Some(place.clone());
        st.placed = placed;
        st.told = fields;
    }
    OPEN.store(true, Ordering::Relaxed);
    if !activate {
        // tao shows with SW_SHOW after a window's first show, which would activate it. Shown
        // this way first, tao's own show below finds it visible and only notes that it is.
        let shown = handle().is_some_and(window::show_no_activate);
        super::log(&format!("panel shown without activation: {shown}"));
    }
    let result = window.show();
    let _ = app.emit_to(LABEL, "an:panel", payload);
    watch();
    super::log(&format!(
        "panel open: reason={} activate={activate} mode={:?} ring_known={} measured={} {} show={} \
         styles={} foreground={} panel={}",
        word(request.get("reason").and_then(Value::as_str)),
        place.mode,
        place.ring_id.is_some(),
        place.ring.is_some(),
        describe(placed.as_ref(), why),
        outcome(&result),
        styles(handle()),
        hex(window::foreground()),
        hex(handle()),
    ));
}

/// The notch moved, was hidden or shown: hang off it again. A panel that hung off a notch whose
/// monitor is gone closes (its display was unplugged); a floating one stays on its monitor while
/// that is attached.
fn reanchor(app: &AppHandle) {
    if !OPEN.load(Ordering::Relaxed) {
        return;
    }
    remember_notch(app);
    let (place, height, old) = {
        let st = state();
        (st.place.clone(), st.height, st.placed)
    };
    let Some(place) = place else {
        return;
    };
    let old = old.map(|placed| placed.anchor);
    if let Some(old) = old.filter(|old| old.edge.is_some()) {
        if !monitors(app).iter().any(|a| a.monitor == old.area.monitor) {
            super::log(&format!(
                "panel closes: the monitor it hung on is gone ({})",
                rect(old.area.monitor)
            ));
            panel::close_unanchored();
            return;
        }
    }
    let (anchor, why) = find_anchor(app, &place, old.as_ref());
    match anchor {
        Some(anchor) => {
            let placed = Placed {
                anchor,
                placement: placement(&anchor, place.mode, height),
            };
            super::log(&format!(
                "panel re-anchors: {}",
                describe(Some(&placed), why)
            ));
            place_again(app, placed, "re-anchor");
        }
        None => super::log(&format!("panel re-anchors: no monitor ({why})")),
    }
    raise_now();
}

/// Moves the open panel to `placed` and tells its page when what it draws changed.
fn place_again(app: &AppHandle, placed: Placed, cause: &str) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let fields = place_fields(&placed);
    let (moved, told) = {
        let mut st = state();
        let moved = st.placed.map(|old| old.placement.window) != Some(placed.placement.window);
        let told = st.told.as_ref() != Some(&fields);
        st.placed = Some(placed);
        if told {
            st.told = Some(fields.clone());
        }
        (moved, told)
    };
    if moved {
        set_frame(&window, placed.placement.window);
        super::log(&format!(
            "panel placed ({cause}): window={} card={} max_height={}",
            rect(placed.placement.window),
            rect(placed.placement.card),
            placed.placement.max_height_css
        ));
    }
    if told {
        let _ = app.emit_to(LABEL, PLACED_EVENT, fields);
    }
}

/// Puts the window's frame where the placement says, in physical pixels.
fn set_frame(window: &WebviewWindow, frame: PxRect) {
    let size = PhysicalSize::new(frame.w.max(1) as u32, frame.h.max(1) as u32);
    let position = PhysicalPosition::new(frame.x, frame.y);
    let _ = window.set_position(position);
    let _ = window.set_size(size);
    // Arriving on a monitor at another scale, Windows resizes the window by the ratio of the two
    // scales: pinned once more, as upstream does for the notch.
    if window.outer_size().map(|now| now != size).unwrap_or(false) {
        let _ = window.set_size(size);
        let _ = window.set_position(position);
        super::log(&format!(
            "panel frame pinned again: wanted {}x{}, now {:?}",
            size.width,
            size.height,
            window.outer_size().map(|s| (s.width, s.height)).ok()
        ));
    }
}

/// What the panel hangs off for `place`, and the reason when it floats. `previous` is the anchor
/// an open panel has now (a re-anchor); `None` when no monitor is known at all.
fn find_anchor(
    app: &AppHandle,
    place: &Place,
    previous: Option<&Anchor>,
) -> (Option<Anchor>, &'static str) {
    let monitors = monitors(app);
    match notch_frame(app, place.ring_id.as_deref()) {
        Ok(notch) => {
            let measured = measured_ring(place.ring, previous, notch.edge, notch.frame);
            let ring = match measured {
                Some(ring) => ring_on_screen((notch.frame.x, notch.frame.y), ring),
                // Nothing measured a ring (a menu, the hot key), or the notch has changed shape
                // since: the middle of the notch, as deep in as a ring's inner side.
                None => geometry::fallback_ring(
                    notch.edge,
                    notch.frame,
                    notch.scale,
                    UPRIGHT_TIP_INSET_CSS * notch.zoom,
                ),
            };
            let centre = (
                notch.frame.x + notch.frame.w / 2,
                notch.frame.y + notch.frame.h / 2,
            );
            let live = notch.handle.and_then(window::monitor_of);
            let anchor = landing(&monitors, live, centre, notch.scale).map(|area| Anchor {
                edge: Some(notch.edge),
                ring: Some(ring),
                area,
                notch: Some(notch.frame),
                measured: measured.is_some(),
            });
            (anchor, "anchored")
        }
        Err(why) => {
            // A panel already floating stays on its monitor; otherwise the pointer's.
            let stays = previous
                .filter(|p| p.edge.is_none())
                .map(|p| p.area.monitor)
                .filter(|monitor| monitors.iter().any(|a| a.monitor == *monitor));
            let point = match stays {
                Some(monitor) => (monitor.x + monitor.w / 2, monitor.y + monitor.h / 2),
                None => pointer(app),
            };
            let live = window::monitor_at(point.0, point.1);
            let anchor = landing(&monitors, live, point, 1.0).map(|area| Anchor {
                edge: None,
                ring: None,
                area,
                notch: None,
                measured: false,
            });
            (anchor, why)
        }
    }
}

/// The notch window as the panel needs it.
struct NotchFrame {
    frame: PxRect,
    edge: PanelEdge,
    /// Its monitor's scale.
    scale: f64,
    /// The notch Size setting.
    zoom: f64,
    handle: Option<isize>,
}

/// The notch the panel can hang off, or why there is none.
fn notch_frame(app: &AppHandle, ring_id: Option<&str>) -> Result<NotchFrame, &'static str> {
    let notch = app
        .get_webview_window(NOTCH)
        .ok_or("floating: no notch window")?;
    let (shown, edge) = {
        let st = app.state::<crate::AppState>();
        let c = st.cfg.lock().unwrap_or_else(|e| e.into_inner());
        (c.notch_visible, crate::config::edge_or_right(&c.notch_edge))
    };
    if !shown || !notch.is_visible().unwrap_or(false) {
        return Err("floating: the notch is hidden");
    }
    // "Ring in notch" off for this account: there is nothing on the notch to point at.
    let off = ring_id.is_some_and(|id| {
        super::hub().is_some_and(|hub| {
            hub.snapshot()
                .rings
                .iter()
                .any(|ring| ring.ring_id == id && !ring.shown)
        })
    });
    if off {
        return Err("floating: the ring is off");
    }
    let position = notch
        .outer_position()
        .map_err(|_| "floating: the notch's position is unknown")?;
    let size = notch
        .outer_size()
        .map_err(|_| "floating: the notch's size is unknown")?;
    Ok(NotchFrame {
        frame: PxRect::new(
            position.x,
            position.y,
            size.width as i32,
            size.height as i32,
        ),
        edge: edge_of(&edge),
        scale: notch.scale_factor().unwrap_or(1.0),
        zoom: crate::ui_scale(app),
        handle: hwnd_of(&notch),
    })
}

fn remember_notch(app: &AppHandle) {
    let handle = app
        .get_webview_window(NOTCH)
        .and_then(|notch| hwnd_of(&notch))
        .unwrap_or(0);
    NOTCH_HANDLE.store(handle, Ordering::Relaxed);
}

fn monitors(app: &AppHandle) -> Vec<Area> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let monitor = PxRect::new(
                m.position().x,
                m.position().y,
                m.size().width as i32,
                m.size().height as i32,
            );
            let wa = m.work_area();
            let work = PxRect::new(
                wa.position.x,
                wa.position.y,
                wa.size.width as i32,
                wa.size.height as i32,
            );
            Area {
                monitor,
                work: usable(work, monitor),
                scale: m.scale_factor(),
            }
        })
        .collect()
}

/// The pointer in physical screen pixels; the origin when nobody can say.
fn pointer(app: &AppHandle) -> (i32, i32) {
    window::cursor()
        .or_else(|| {
            app.cursor_position()
                .ok()
                .map(|p| (p.x.round() as i32, p.y.round() as i32))
        })
        .unwrap_or((0, 0))
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
        // Until it is placed, which happens before it is first shown.
        .inner_size(geometry::width_css(None, PanelMode::List), FIRST_HEIGHT)
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
        .theme(crate::theme_choice(app))
        .initialization_script(script)
        .build()?;
    match hwnd_of(&window) {
        Some(handle) => HANDLE.store(handle, Ordering::Relaxed),
        None => super::log("panel window: no handle (the keyboard gate stays shut)"),
    }
    accelerator_keys_off(&window);
    window.on_window_event(|event| {
        if let WindowEvent::Focused(focused) = event {
            let held = panel::status().is_some_and(|status| status.focused);
            super::log(&format!(
                "panel focus event: focused={focused} held={held} foreground={} panel={}",
                hex(window::foreground()),
                hex(handle())
            ));
            panel::foreground_changed();
            if !*focused && held {
                blurred();
            }
        }
    });
    super::log(&format!(
        "panel window built: handle={} focusable={focusable} styles={}",
        hex(handle()),
        styles(handle())
    ));
    Ok(window)
}

/// The panel had the keyboard and lost it: that is a click (or a switch) to another window,
/// which closes it, unless the window is the notch, whose rings answer for themselves.
fn blurred() {
    std::thread::spawn(|| {
        let mut front = None;
        // The new foreground window is named a moment later; an empty answer is asked again.
        for _ in 0..4 {
            std::thread::sleep(BLUR_SETTLES_AFTER);
            front = window::foreground();
            if front.is_some() {
                break;
            }
        }
        if !OPEN.load(Ordering::Relaxed) {
            return;
        }
        let closes = blur_closes(front, handle(), atom(&NOTCH_HANDLE));
        super::log(&format!(
            "panel lost the keyboard: foreground={} notch={} closes={closes} (unless pinned)",
            hex(front),
            hex(atom(&NOTCH_HANDLE))
        ));
        if closes {
            panel::outside_click();
        }
    });
}

/// Looks at the desktop while the panel is open: a foreground change (the rules shut the gate,
/// and the panel goes back above the notch) and a mouse button going down somewhere else.
fn watch() {
    let mine = WATCH.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = std::thread::Builder::new()
        .name("an-panel-watch".into())
        .spawn(move || {
            // A button held while the panel opened (the click that opened it) isn't a press.
            let mut down = window::mouse_button_down();
            let mut front = window::foreground();
            loop {
                std::thread::sleep(WATCH_EVERY);
                if WATCH.load(Ordering::Relaxed) != mine || !OPEN.load(Ordering::Relaxed) {
                    return;
                }
                let Some(panel) = handle() else {
                    continue;
                };
                let now_front = window::foreground();
                let below_notch = atom(&NOTCH_HANDLE)
                    .is_some_and(|notch| window::is_above(panel, notch) == Some(false));
                if now_front != front || below_notch {
                    front = now_front;
                    panel::foreground_changed();
                }
                let now_down = window::mouse_button_down();
                let pressed = now_down && !down;
                down = now_down;
                // In the foreground, the panel hears of a click elsewhere by losing the keyboard.
                if !pressed || now_front == Some(panel) {
                    continue;
                }
                let Some(cursor) = window::cursor() else {
                    continue;
                };
                let pinned = panel::status().is_some_and(|status| status.pinned);
                let frame = window::window_rect(panel).map(px_rect);
                let notch = notch_rects();
                let closes = outside_click(frame, &notch, cursor, pinned);
                super::log(&format!(
                    "panel saw a press at ({},{}): panel={} notch_rects={} pinned={pinned} \
                     closes={closes}",
                    cursor.0,
                    cursor.1,
                    frame.map(rect).unwrap_or_else(|| "?".into()),
                    notch.len()
                ));
                if closes {
                    panel::outside_click();
                }
            }
        });
}

/// Where a click lands on the notch: the rects its page reports to upstream's click-through
/// watchdog (the rest of the notch window lets clicks through to whatever is under it).
fn notch_rects() -> Vec<PxRect> {
    let Some(notch) = atom(&NOTCH_HANDLE) else {
        return Vec::new();
    };
    if !window::styles(notch).is_some_and(|s| s.visible) {
        return Vec::new();
    }
    let Some(frame) = window::window_rect(notch) else {
        return Vec::new();
    };
    let hot = crate::HOT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    hot_on_screen((frame.left, frame.top), &hot, crate::HOT_PAD)
}

// ---- the pieces that decide nothing about Windows, and run in the host tests ----

/// The ring the notch's page measured, while it still says where the ring is: always for a fresh
/// open, and at a re-anchor only when the notch has the edge and size it was measured in (the
/// page lays its rings out again for another edge or size, and nobody has measured since).
fn measured_ring(
    ring: Option<RingRect>,
    previous: Option<&Anchor>,
    edge: PanelEdge,
    notch: PxRect,
) -> Option<RingRect> {
    let still = match previous {
        None => true,
        Some(p) => {
            p.measured
                && p.edge == Some(edge)
                && p.notch.map(|n| (n.w, n.h)) == Some((notch.w, notch.h))
        }
    };
    ring.filter(|_| still)
}

/// A rect the notch's page measured (physical px from the notch window's top-left corner) as a
/// rect on the screen.
fn ring_on_screen(notch_origin: (i32, i32), ring: RingRect) -> PxRect {
    let [x, y, w, h] = ring;
    let left = (f64::from(notch_origin.0) + x).round();
    let top = (f64::from(notch_origin.1) + y).round();
    // The far sides are rounded too, not the size: the ring's centre stays where it is.
    let right = (f64::from(notch_origin.0) + x + w).round();
    let bottom = (f64::from(notch_origin.1) + y + h).round();
    PxRect::new(
        left as i32,
        top as i32,
        (right - left) as i32,
        (bottom - top) as i32,
    )
}

/// The monitor the panel lands on. `live` is what Windows says about the monitor at `point`
/// right now (its work area follows a taskbar that moved at once); the scale is Tauri's for that
/// monitor. Without it (the host build) the monitor holding `point`, else the first.
fn landing(
    monitors: &[Area],
    live: Option<window::MonitorArea>,
    point: (i32, i32),
    scale: f64,
) -> Option<Area> {
    let holding = monitors.iter().find(|a| holds(&a.monitor, point));
    match live {
        Some(live) => {
            let monitor = px_rect(live.monitor);
            let scale = monitors
                .iter()
                .find(|a| a.monitor == monitor)
                .or(holding)
                .map(|a| a.scale)
                .unwrap_or(scale);
            Some(Area {
                monitor,
                work: usable(px_rect(live.work), monitor),
                scale,
            })
        }
        None => holding.or(monitors.first()).copied(),
    }
}

/// The work area, or the whole monitor when the platform reports none worth the name.
fn usable(work: PxRect, monitor: PxRect) -> PxRect {
    if work.w > 0 && work.h > 0 {
        work
    } else {
        monitor
    }
}

fn placement(anchor: &Anchor, mode: PanelMode, height: f64) -> PanelPlacement {
    geometry::place(&PanelInput {
        edge: anchor.edge,
        ring: anchor.ring,
        work_area: anchor.area.work,
        scale: anchor.area.scale,
        mode,
        content_height_css: height,
    })
}

/// What the page draws by: the side its tail is on, how wide the card is, where the tail sits.
fn place_fields(placed: &Placed) -> Value {
    let floating = placed.placement.floating;
    json!({
        "edge": placed.anchor.edge.filter(|_| !floating).map(edge_name),
        "floating": floating,
        "width": placed.placement.width_css,
        "tail_offset": placed.placement.tail_offset,
    })
}

/// `request` with the placement's fields added; as it came when nothing could be placed.
fn with_fields(request: &Value, fields: Option<&Value>) -> Value {
    let mut payload = request.clone();
    if let (Some(payload), Some(fields)) =
        (payload.as_object_mut(), fields.and_then(Value::as_object))
    {
        for (key, value) in fields {
            payload.insert(key.clone(), value.clone());
        }
    }
    payload
}

/// Whether a mouse button going down at `cursor` closes a panel that isn't the foreground
/// window: anywhere but on the panel and on the notch, unless "Keep open" is on (the port of
/// `ClaudePanelPolicy.closesOnMouseDown`). A panel whose frame can't be read is left alone.
fn outside_click(
    panel: Option<PxRect>,
    notch: &[PxRect],
    cursor: (i32, i32),
    pinned: bool,
) -> bool {
    match panel {
        Some(panel) if !pinned => !holds(&panel, cursor) && !notch.iter().any(|r| holds(r, cursor)),
        _ => false,
    }
}

/// Whether a panel that lost the keyboard to `foreground` closes: not when it has it back, and
/// not when the notch took it (a click on a ring is the ring's to answer).
fn blur_closes(foreground: Option<isize>, panel: Option<isize>, notch: Option<isize>) -> bool {
    match foreground {
        Some(front) => Some(front) != panel && Some(front) != notch,
        // Nothing in front at all: the keyboard went somewhere that isn't the panel.
        None => true,
    }
}

/// The notch's clickable rects on the screen: each one the page reported, grown by upstream's
/// pad, and, with more than one, the box around them all (upstream counts the gap between the
/// pill and its card as the notch too).
fn hot_on_screen(origin: (i32, i32), hot: &[[f64; 4]], pad: f64) -> Vec<PxRect> {
    let on_screen = |x0: f64, y0: f64, x1: f64, y1: f64| {
        let left = (f64::from(origin.0) + x0 - pad).floor();
        let top = (f64::from(origin.1) + y0 - pad).floor();
        let right = (f64::from(origin.0) + x1 + pad).ceil();
        let bottom = (f64::from(origin.1) + y1 + pad).ceil();
        PxRect::new(
            left as i32,
            top as i32,
            (right - left) as i32,
            (bottom - top) as i32,
        )
    };
    let sane: Vec<&[f64; 4]> = hot
        .iter()
        .filter(|r| r.iter().all(|n| n.is_finite()) && r[2] >= 0.0 && r[3] >= 0.0)
        .collect();
    let mut rects: Vec<PxRect> = sane
        .iter()
        .map(|r| on_screen(r[0], r[1], r[0] + r[2], r[1] + r[3]))
        .collect();
    if sane.len() > 1 {
        let least = |at: usize| sane.iter().map(|r| r[at]).fold(f64::MAX, f64::min);
        let most = |at: usize| {
            sane.iter()
                .map(|r| r[at] + r[at + 2])
                .fold(f64::MIN, f64::max)
        };
        rects.push(on_screen(least(0), least(1), most(0), most(1)));
    }
    rects
}

/// Whether `point` is inside `rect` (its right and bottom sides are outside, as Win32 has them).
fn holds(rect: &PxRect, point: (i32, i32)) -> bool {
    point.0 >= rect.x && point.0 < rect.right() && point.1 >= rect.y && point.1 < rect.bottom()
}

fn px_rect(r: window::Rect) -> PxRect {
    PxRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// Upstream's edge names; anything else is its default, the right.
fn edge_of(name: &str) -> PanelEdge {
    match name {
        "left" => PanelEdge::Left,
        "top" => PanelEdge::Top,
        "bottom" => PanelEdge::Bottom,
        _ => PanelEdge::Right,
    }
}

fn edge_name(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Right => "right",
        PanelEdge::Left => "left",
        PanelEdge::Top => "top",
        PanelEdge::Bottom => "bottom",
    }
}

// ---- words for run.log ----

fn rect(r: PxRect) -> String {
    format!("({},{} {}x{})", r.x, r.y, r.w, r.h)
}

fn hex(handle: Option<isize>) -> String {
    match handle {
        Some(handle) => format!("{handle:#x}"),
        None => "none".into(),
    }
}

fn outcome<T>(result: &tauri::Result<T>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(e) => format!("failed ({e})"),
    }
}

fn styles(handle: Option<isize>) -> String {
    match handle.and_then(window::styles) {
        Some(s) => format!(
            "(ex={:#x} topmost={} no_activate={} visible={})",
            s.ex_style, s.topmost, s.no_activate, s.visible
        ),
        None => "unknown".into(),
    }
}

fn describe(placed: Option<&Placed>, why: &str) -> String {
    match placed {
        Some(p) => format!(
            "{why} edge={} ring={} window={} card={} work={} scale={} tail_offset={} \
             tail_limit={}",
            p.anchor.edge.map(edge_name).unwrap_or("none"),
            p.anchor.ring.map(rect).unwrap_or_else(|| "none".into()),
            rect(p.placement.window),
            rect(p.placement.card),
            rect(p.anchor.area.work),
            p.anchor.area.scale,
            p.placement.tail_offset,
            p.placement.tail_limit
        ),
        None => format!("{why}; no monitor is known, the window stays where it was"),
    }
}

/// A reason as a log word: the hub's and the pages' reasons are short identifiers, and nothing
/// longer or stranger that a page might send instead is written down.
fn word(text: Option<&str>) -> String {
    text.unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(24)
        .collect()
}

// ---- what only exists on Windows ----

#[cfg(windows)]
pub(super) fn hwnd_of(window: &WebviewWindow) -> Option<isize> {
    match window.hwnd() {
        // As a number: this crate's `windows`, Tauri's and agentnotch-win's each have their own
        // HWND type.
        Ok(hwnd) => Some(hwnd.0 as isize),
        Err(e) => {
            super::log(&format!("window handle of {}: {e}", window.label()));
            None
        }
    }
}

/// Off Windows (the host build that runs the glue's tests) there is no handle to keep, so the
/// panel is never seen in the foreground and the keyboard gate stays shut.
#[cfg(not(windows))]
pub(super) fn hwnd_of(_window: &WebviewWindow) -> Option<isize> {
    None
}

/// The browser's own shortcuts (F5 and Ctrl+R reload, Ctrl+P print, Ctrl+F find, Ctrl+J
/// downloads, ...) are turned off in the panel: a reload would drop what the user typed.
#[cfg(windows)]
fn accelerator_keys_off(window: &WebviewWindow) {
    let asked = window.with_webview(|webview| match keys_off(&webview.controller()) {
        Ok(()) => super::log("panel: browser accelerator keys off"),
        Err(e) => super::log(&format!("panel: browser accelerator keys stay on: {e}")),
    });
    if let Err(e) = asked {
        super::log(&format!("panel: browser accelerator keys stay on: {e}"));
    }
}

#[cfg(not(windows))]
fn accelerator_keys_off(_window: &WebviewWindow) {}

#[cfg(windows)]
fn keys_off(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
) -> Result<(), String> {
    use agentnotch_win::capture::ComInterface;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;

    // SAFETY: COM calls on the panel's own WebView2 controller, on the thread that owns it
    // (`with_webview` runs its closure there); each returns an HRESULT that is checked.
    unsafe {
        let settings = controller
            .CoreWebView2()
            .and_then(|webview| webview.Settings())
            .map_err(|e| e.to_string())?;
        // Settings3 came with WebView2 runtime 92; an older runtime answers the cast with an
        // error, and the page's own key handler is then the only guard.
        settings
            .cast::<ICoreWebView2Settings3>()
            .and_then(|settings| settings.SetAreBrowserAcceleratorKeysEnabled(false))
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: PxRect = PxRect::new(0, 0, 1920, 1040);
    const MONITOR: PxRect = PxRect::new(0, 0, 1920, 1080);

    fn area(scale: f64) -> Area {
        Area {
            monitor: MONITOR,
            work: WORK,
            scale,
        }
    }

    fn anchored(edge: PanelEdge, ring: PxRect, scale: f64) -> Anchor {
        Anchor {
            edge: Some(edge),
            ring: Some(ring),
            area: area(scale),
            notch: Some(PxRect::new(1560, 215, 360, 650)),
            measured: true,
        }
    }

    fn floating(scale: f64) -> Anchor {
        Anchor {
            edge: None,
            ring: None,
            area: area(scale),
            notch: None,
            measured: false,
        }
    }

    #[test]
    fn a_measured_ring_is_the_notch_windows_position_plus_the_pages_rect() {
        assert_eq!(
            ring_on_screen((1560, 215), [300.0, 120.0, 44.0, 44.0]),
            PxRect::new(1860, 335, 44, 44)
        );
        // A second monitor left of and above the primary one: negative screen coordinates.
        assert_eq!(
            ring_on_screen((-1920, -200), [12.0, 40.0, 44.0, 44.0]),
            PxRect::new(-1908, -160, 44, 44)
        );
        // Fractions (a page at 125 %): both sides are rounded, so the centre doesn't drift.
        assert_eq!(
            ring_on_screen((100, 100), [10.4, 20.6, 55.0, 55.0]),
            PxRect::new(110, 121, 55, 55)
        );
        assert_eq!(
            ring_on_screen((100, 100), [10.5, 20.5, 54.8, 54.8]),
            PxRect::new(111, 121, 54, 54)
        );
        // A point stays a point.
        assert_eq!(
            ring_on_screen((7, 9), [1.0, 2.0, 0.0, 0.0]),
            PxRect::new(8, 11, 0, 0)
        );
    }

    #[test]
    fn a_press_closes_the_panel_only_outside_the_panel_and_the_notch() {
        let panel = Some(PxRect::new(1100, 300, 432, 400));
        let notch = [
            PxRect::new(1850, 300, 60, 300),
            PxRect::new(1560, 400, 280, 120),
        ];
        // Inside the panel: no. Its left and top sides are inside, its right and bottom aren't.
        assert!(!outside_click(panel, &notch, (1300, 500), false));
        assert!(!outside_click(panel, &notch, (1100, 300), false));
        assert!(!outside_click(panel, &notch, (1531, 699), false));
        assert!(outside_click(panel, &notch, (1532, 500), false));
        assert!(outside_click(panel, &notch, (1300, 700), false));
        // Inside the notch (either of its rects): no.
        assert!(!outside_click(panel, &notch, (1880, 450), false));
        assert!(!outside_click(panel, &notch, (1600, 450), false));
        // Elsewhere: close. That includes the notch window's see-through part.
        assert!(outside_click(panel, &notch, (200, 200), false));
        assert!(outside_click(panel, &notch, (1700, 250), false));
        assert!(outside_click(panel, &[], (1880, 450), false));
        // Pinned: never.
        for cursor in [(200, 200), (1300, 500), (1880, 450), (1700, 250)] {
            assert!(!outside_click(panel, &notch, cursor, true));
        }
        // A panel whose frame can't be read isn't closed on a guess.
        assert!(!outside_click(None, &notch, (200, 200), false));
    }

    #[test]
    fn a_panel_that_lost_the_keyboard_closes_unless_the_notch_or_itself_has_it() {
        let (panel, notch, terminal) = (Some(0x700), Some(0x600), Some(0x100));
        assert!(blur_closes(terminal, panel, notch));
        assert!(!blur_closes(notch, panel, notch));
        assert!(!blur_closes(panel, panel, notch));
        assert!(blur_closes(None, panel, notch));
        // No notch window: every other window is elsewhere.
        assert!(blur_closes(terminal, panel, None));
        assert!(blur_closes(None, panel, None));
    }

    #[test]
    fn the_notchs_clickable_rects_are_its_pages_rects_on_the_screen_with_upstreams_pad() {
        assert_eq!(hot_on_screen((1560, 215), &[], 10.0), vec![]);
        assert_eq!(
            hot_on_screen((1560, 215), &[[300.0, 100.0, 50.0, 400.0]], 10.0),
            vec![PxRect::new(1850, 305, 70, 420)]
        );
        // The pill and its card: each, and the box around both (the gap between them counts).
        assert_eq!(
            hot_on_screen(
                (1560, 215),
                &[[300.0, 100.0, 50.0, 400.0], [20.0, 200.0, 250.0, 120.0]],
                10.0
            ),
            vec![
                PxRect::new(1850, 305, 70, 420),
                PxRect::new(1570, 405, 270, 140),
                PxRect::new(1570, 305, 350, 420),
            ]
        );
        // A rect that isn't one is left out.
        assert_eq!(
            hot_on_screen(
                (0, 0),
                &[[f64::NAN, 0.0, 10.0, 10.0], [0.0, 0.0, -5.0, 10.0]],
                10.0
            ),
            vec![]
        );
    }

    #[test]
    fn the_pages_rect_is_used_again_only_while_the_notch_keeps_its_edge_and_size() {
        let ring = Some([300.0, 120.0, 44.0, 44.0]);
        let notch = PxRect::new(1560, 215, 360, 650);
        let was = anchored(PanelEdge::Right, PxRect::new(1860, 335, 44, 44), 1.0);

        // A fresh open trusts the page.
        assert_eq!(measured_ring(ring, None, PanelEdge::Right, notch), ring);
        assert_eq!(measured_ring(None, None, PanelEdge::Right, notch), None);
        // Slid along its edge: the same rect, from the new position.
        let slid = PxRect::new(1560, 40, 360, 650);
        assert_eq!(
            measured_ring(ring, Some(&was), PanelEdge::Right, slid),
            ring
        );
        // Another edge, or another size: the rings are laid out anew, and nobody measured.
        assert_eq!(measured_ring(ring, Some(&was), PanelEdge::Top, notch), None);
        let larger = PxRect::new(1488, 150, 432, 780);
        assert_eq!(
            measured_ring(ring, Some(&was), PanelEdge::Right, larger),
            None
        );
        // It hung off the stand-in point, or floated: there never was a measurement to trust.
        let mut guessed = was;
        guessed.measured = false;
        assert_eq!(
            measured_ring(ring, Some(&guessed), PanelEdge::Right, notch),
            None
        );
        assert_eq!(
            measured_ring(ring, Some(&floating(1.0)), PanelEdge::Right, notch),
            None
        );
    }

    #[test]
    fn the_landing_monitor_is_windows_work_area_with_tauris_scale() {
        let left = Area {
            monitor: PxRect::new(-2560, 0, 2560, 1440),
            work: PxRect::new(-2560, 0, 2560, 1392),
            scale: 1.5,
        };
        let monitors = [area(1.0), left];
        let live = |monitor: (i32, i32, i32, i32), work: (i32, i32, i32, i32)| {
            let rect = |(left, top, right, bottom)| window::Rect {
                left,
                top,
                right,
                bottom,
            };
            Some(window::MonitorArea {
                monitor: rect(monitor),
                work: rect(work),
            })
        };
        // Windows' own answer wins for the work area (a taskbar that just moved to the left).
        assert_eq!(
            landing(
                &monitors,
                live((-2560, 0, 0, 1440), (-2500, 0, 0, 1440)),
                (-100, 700),
                1.0
            ),
            Some(Area {
                monitor: PxRect::new(-2560, 0, 2560, 1440),
                work: PxRect::new(-2500, 0, 2500, 1440),
                scale: 1.5,
            })
        );
        // A monitor Tauri doesn't list (just plugged in): the caller's scale.
        assert_eq!(
            landing(
                &monitors,
                live((1920, 0, 3840, 1080), (1920, 0, 3840, 1040)),
                (2000, 500),
                1.25
            )
            .map(|a| a.scale),
            Some(1.25)
        );
        // An empty work area falls back to the monitor.
        assert_eq!(
            landing(
                &monitors,
                live((0, 0, 1920, 1080), (0, 0, 0, 0)),
                (5, 5),
                1.0
            )
            .map(|a| a.work),
            Some(MONITOR)
        );
        // Without Windows' answer: the monitor holding the point, else the first, else none.
        assert_eq!(landing(&monitors, None, (-100, 700), 1.0), Some(left));
        assert_eq!(
            landing(&monitors, None, (9_000, 9_000), 1.0),
            Some(area(1.0))
        );
        assert_eq!(landing(&[], None, (0, 0), 1.0), None);
    }

    // The clamp `panel_report_size` is held to: the engine's caps for the route, and the floor.
    #[test]
    fn the_cards_height_follows_the_content_within_the_placements_cap() {
        let ring = PxRect::new(1860, 500, 44, 44);
        let beside = anchored(PanelEdge::Right, ring, 1.0);
        let card = |anchor: &Anchor, mode, height| placement(anchor, mode, height).card.h;

        assert_eq!(card(&beside, PanelMode::List, 10.0), 220);
        assert_eq!(card(&beside, PanelMode::List, 431.0), 431);
        assert_eq!(card(&beside, PanelMode::List, 5_000.0), 680);
        assert_eq!(card(&beside, PanelMode::Chat, 5_000.0), 780);
        assert_eq!(card(&beside, PanelMode::List, f64::NAN), 220);
        // The cap the page is told, and the widths: the list's beside a side notch, the chat's.
        let list = placement(&beside, PanelMode::List, FIRST_HEIGHT);
        assert_eq!((list.width_css, list.max_height_css), (400.0, 680.0));
        let chat = placement(&beside, PanelMode::Chat, FIRST_HEIGHT);
        assert_eq!((chat.width_css, chat.max_height_css), (440.0, 780.0));
        // At 150 % the window is in physical pixels, the cap still in CSS px.
        let mut large = anchored(PanelEdge::Right, PxRect::new(2_790, 800, 66, 66), 1.5);
        large.area.work = PxRect::new(0, 0, 2_880, 1_752);
        let scaled = placement(&large, PanelMode::List, 5_000.0);
        assert_eq!(scaled.card.h, 1_020);
        assert_eq!(scaled.max_height_css, 680.0);
        // A short work area caps it below the route's cap (margin 8 on both sides).
        let mut short = beside;
        short.area.work = PxRect::new(0, 0, 1024, 728);
        short.ring = Some(PxRect::new(964, 300, 44, 44));
        assert_eq!(card(&short, PanelMode::Chat, 5_000.0), 712);
        // Every frame stays inside the work area.
        for (anchor, mode) in [
            (beside, PanelMode::List),
            (beside, PanelMode::Chat),
            (short, PanelMode::Chat),
            (floating(1.25), PanelMode::Chat),
        ] {
            let placed = placement(&anchor, mode, 5_000.0);
            assert!(anchor.area.work.contains(&placed.card), "{placed:?}");
        }
    }

    #[test]
    fn the_page_is_told_the_edge_the_width_and_the_tail_beside_its_request() {
        let request = json!({
            "route": "sessions", "ring_id": "claude-acct-5f3e1d2c0b9a",
            "highlight": null, "reason": "ring_click",
        });
        let anchor = anchored(PanelEdge::Right, PxRect::new(1860, 500, 44, 44), 1.0);
        let placed = Placed {
            anchor,
            placement: placement(&anchor, PanelMode::List, 400.0),
        };
        assert_eq!(
            with_fields(&request, Some(&place_fields(&placed))),
            json!({
                "route": "sessions", "ring_id": "claude-acct-5f3e1d2c0b9a",
                "highlight": null, "reason": "ring_click",
                "edge": "right", "floating": false, "width": 400.0, "tail_offset": 0.0,
            })
        );
        // Near the bottom of the screen the card stops at the work area and the tail slides.
        let low = anchored(PanelEdge::Right, PxRect::new(1860, 960, 44, 44), 1.0);
        let placed = Placed {
            anchor: low,
            placement: placement(&low, PanelMode::List, 400.0),
        };
        let fields = place_fields(&placed);
        assert_eq!(fields["edge"], "right");
        assert!(fields["tail_offset"].as_f64().unwrap() > 0.0);

        // Floating: no edge, no tail.
        let anchor = floating(1.0);
        let placed = Placed {
            anchor,
            placement: placement(&anchor, PanelMode::Chat, 400.0),
        };
        assert_eq!(
            place_fields(&placed),
            json!({ "edge": null, "floating": true, "width": 520.0, "tail_offset": 0.0 })
        );
        // Nothing could be placed: the request goes as it came.
        assert_eq!(with_fields(&request, None), request);
    }

    #[test]
    fn edges_are_upstreams_names_and_an_unknown_one_is_the_right() {
        for (name, edge) in [
            ("right", PanelEdge::Right),
            ("left", PanelEdge::Left),
            ("top", PanelEdge::Top),
            ("bottom", PanelEdge::Bottom),
        ] {
            assert_eq!(edge_of(name), edge);
            assert_eq!(edge_name(edge), name);
        }
        assert_eq!(edge_of(""), PanelEdge::Right);
        assert_eq!(edge_of("Top"), PanelEdge::Right);
    }

    #[test]
    fn a_reason_is_logged_as_a_short_word_and_nothing_else() {
        assert_eq!(word(Some("ring_click")), "ring_click");
        assert_eq!(word(None), "");
        assert_eq!(
            word(Some("please run: rm -rf / && echo 'done' #########")),
            "pleaserunrmrfechodone"
        );
        assert_eq!(word(Some(&"a".repeat(200))).len(), 24);
    }

    // The accelerator-keys call against webview2-com 0.38: that this compiles is the proof here
    // (what it does to F5 is the sealed self-test's to show).
    #[cfg(windows)]
    #[test]
    fn the_accelerator_keys_call_type_checks_against_webview2() {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
        let call: fn(&ICoreWebView2Controller) -> Result<(), String> = keys_off;
        let _ = call;
    }
}
