//! The sessions panel, `agentnotch-panel` (DESIGN-WIN §2.4, §5.3): what it shows, who has the
//! keyboard, and what the hub is told.
//!
//! Every rule here runs against [`WindowService`], the handful of things only the OS can do; the
//! real one is `panel_window`, and the tests drive a fake. The rules:
//!
//! - **Opening.** A click, the hot key, a banner or Settings saves the window that has the
//!   keyboard, shows the panel and asks for the foreground. An automatic open never asks, never
//!   replaces a panel already open, and lands on the list.
//! - **The keyboard gate.** The page shows a caret and takes shortcuts only after
//!   `an:panel_focus {focused: true}`, and that is sent only when the foreground window *is
//!   observed to be* the panel: 50 ms and 250 ms after every attempt, and on every focus event.
//!   Windows may refuse a foreground request, and an accepted request isn't a granted one, so a
//!   request's answer is never looked at ([`WindowService::request_foreground`] has none). A caret
//!   shown while keys still reach the terminal could answer a prompt there (§9, R6).
//! - **Closing.** The panel is hidden and the gate shut. The saved window gets the foreground
//!   back only if the panel held it (otherwise the user has moved on) and the window still exists.
//! - **Where it goes.** The rules only hand on what they were told (the ring, the rect the notch's
//!   page measured, list or chat, the content's height); the window service asks the engine's
//!   `geometry::panel` for the frame.
//! - **Reports.** Every change goes to the hub as `panel_state`, in order, through one thread.
//!
//! When an auto-opened panel closes by itself is the engine's rule (`control`, on the state
//! reported here); this file only obeys `HubEvent::PanelClose`.

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};

use agentnotch_engine::geometry::panel::PanelMode;
use agentnotch_engine::model::ui::PanelRequest;
use serde_json::{json, Value};
use tauri::AppHandle;

use super::panel_window::PanelWindow;

pub(super) const LABEL: &str = "agentnotch-panel";
/// The list, every account or one ring's.
const LIST: &str = "sessions";
/// A chat's route is this followed by the session's id.
const CHAT_PREFIX: &str = "session:";
/// How long after an attempt to take the foreground it is looked at: once for the usual case and
/// once for a desktop that was slow to switch. A later grant arrives as a focus event.
const CONFIRM_AFTER_MS: [u64; 2] = [50, 250];
/// The hub's reason for closing after a jump to the terminal, which a pinned panel ignores.
const JUMP: &str = "jump";

/// Something the panel's rules asked to be told about later ([`WindowService::after`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Timer {
    /// Look at the foreground window again.
    Confirm,
}

/// A ring as the notch's page measured it: `[x, y, w, h]` in physical pixels, from the top-left
/// of the notch window's client area.
pub(super) type RingRect = [f64; 4];

/// What the panel is placed by, as far as the rules know it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Place {
    /// The ring the panel hangs off; `None` for every account's list (the hot key, a menu).
    pub(super) ring_id: Option<String>,
    /// Where the page says that ring is; `None` when nothing measured it.
    pub(super) ring: Option<RingRect>,
    pub(super) mode: PanelMode,
}

/// What the OS does for the panel. Handles are `HWND`s as `isize`.
///
/// Everything that changes a window is carried out in the order it was asked for, some time
/// after the call returns (window work belongs to the main thread); the two reads answer at
/// once. Nothing here may call back into the panel's rules before it returns.
pub(super) trait WindowService {
    /// Shows the panel where `place` puts it, building it first when it doesn't exist yet, and
    /// sends the page `an:panel`: `request` plus where it was put (`edge`, `floating`, `width`,
    /// `tail_offset`). A page that isn't loaded yet reads the same value before its first script
    /// runs. `activate: false` must not take the foreground.
    fn show(&self, request: &Value, place: Place, activate: bool);
    fn hide(&self);
    /// Whether a click on the panel may make it the foreground window (`WS_EX_NOACTIVATE` off).
    fn set_focusable(&self, focusable: bool);
    /// Asks for the panel to become the foreground window. Deliberately without an answer: only
    /// [`WindowService::foreground`] says who has the keyboard.
    fn request_foreground(&self);
    /// Gives the foreground to another app's window (the one saved when the panel opened).
    fn restore_foreground(&self, window: isize);
    /// The foreground window right now.
    fn foreground(&self) -> Option<isize>;
    /// The window to give the keyboard back to: the foreground window, or, when that is one of
    /// this app's own (the notch, after a click on a ring), the last window of another app that
    /// had it.
    fn keyboard_owner(&self) -> Option<isize>;
    /// The panel's own window, once it has been built.
    fn panel(&self) -> Option<isize>;
    fn exists(&self, window: isize) -> bool;
    /// Puts the panel above the other topmost windows (the notch) without activating it.
    fn raise_topmost(&self);
    /// The open panel shows the list or a chat, and its content is `height` CSS px tall (`None`:
    /// as tall as before). The window follows, within what its placement allows.
    fn set_content(&self, mode: PanelMode, height: Option<f64>);
    /// An event for the panel's page.
    fn emit(&self, event: &str, payload: Value);
    /// Hands `timer` to the panel's rules after `ms` milliseconds.
    fn after(&self, ms: u64, timer: Timer);
}

/// What the panel shows, as the hub is told it (`PanelState`, §3.4), and what closing needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Status {
    pub(super) open: bool,
    pub(super) route: Option<String>,
    pub(super) ring_id: Option<String>,
    pub(super) reason: Option<String>,
    /// The pointer is inside, or a text field has focus (the page says).
    pub(super) engaged: bool,
    /// The panel was seen to be the foreground window: the keyboard gate is open.
    pub(super) focused: bool,
    /// "Keep open" (the setting `panelPinned`).
    pub(super) pinned: bool,
    /// The window that had the keyboard before the panel asked for it.
    saved_foreground: Option<isize>,
    /// The page's last content height, kept so a route change can clamp it again.
    content_height: Option<f64>,
}

/// The panel's rules over a window service.
pub(super) struct Core<S: WindowService> {
    service: S,
    /// Whether the run is sealed: the self-test's confirmation exists only then.
    sealed: bool,
    /// Where `panel_state` goes. Called with the status lock held, so reports leave in the order
    /// the changes happened; it must only hand the value on.
    report: Box<dyn Fn(Value) + Send + Sync>,
    status: Mutex<Status>,
}

impl<S: WindowService> Core<S> {
    pub(super) fn new(service: S, sealed: bool, report: Box<dyn Fn(Value) + Send + Sync>) -> Self {
        Self {
            service,
            sealed,
            report,
            status: Mutex::new(Status::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The hub, a page or a menu asked for the panel.
    pub(super) fn open(&self, request: PanelRequest) {
        let mut s = self.lock();
        self.open_locked(&mut s, request, None);
    }

    /// A ring clicked: the same ring again closes the panel, another ring moves it there.
    /// `ring` is where the notch's page measured that ring.
    pub(super) fn toggle(&self, ring_id: Option<String>, reason: String, ring: Option<RingRect>) {
        let mut s = self.lock();
        if s.open && s.ring_id == ring_id {
            self.close_locked(&mut s);
        } else {
            let request = PanelRequest {
                route: LIST.into(),
                ring_id,
                highlight: None,
                reason,
            };
            self.open_locked(&mut s, request, ring);
        }
    }

    /// The user closed it: Esc, the close button.
    pub(super) fn close(&self) {
        self.close_locked(&mut self.lock());
    }

    /// The hub closed it (`HubEvent::PanelClose`). A pinned panel stays after a jump to the
    /// terminal; the auto-close of a panel that opened by itself applies to a pinned one too.
    pub(super) fn close_by_hub(&self, reason: &str) {
        let mut s = self.lock();
        if s.pinned && reason == JUMP {
            return;
        }
        self.close_locked(&mut s);
    }

    /// A mouse button went down outside the panel and the notch, or the panel lost the keyboard
    /// to another window.
    pub(super) fn outside_click(&self) {
        let mut s = self.lock();
        if !s.pinned {
            self.close_locked(&mut s);
        }
    }

    /// The hot key: opens the list for every account; closes a panel that has the keyboard;
    /// gives the keyboard to one that hasn't.
    pub(super) fn hotkey(&self) {
        let mut s = self.lock();
        if !s.open {
            let request = PanelRequest {
                route: LIST.into(),
                ring_id: None,
                highlight: None,
                reason: "hotkey".into(),
            };
            self.open_locked(&mut s, request, None);
        } else if s.focused {
            self.close_locked(&mut s);
        } else {
            self.take_focus_locked(&mut s);
        }
    }

    /// The page moved between the list and a chat.
    pub(super) fn set_route(&self, route: String) {
        let mut s = self.lock();
        s.route = Some(route);
        // The two differ in width and in how tall they may get: the window is fitted again.
        if s.open {
            self.resize(&s);
        }
        self.report(&s);
    }

    pub(super) fn set_engaged(&self, on: bool) {
        let mut s = self.lock();
        s.engaged = on;
        self.report(&s);
    }

    /// The setting `panelPinned`, as the hub's snapshot has it.
    pub(super) fn set_pinned(&self, on: bool) {
        let mut s = self.lock();
        if s.pinned != on {
            s.pinned = on;
            self.report(&s);
        }
    }

    /// The page's natural content size (CSS px): the window follows the height, within what its
    /// placement allows (the engine's caps and the work area; the window service asks). The
    /// width is the placement's, not the page's.
    pub(super) fn report_size(&self, _width: f64, height: f64) {
        if !height.is_finite() || height <= 0.0 {
            return;
        }
        let mut s = self.lock();
        if !s.open {
            return;
        }
        s.content_height = Some(height);
        self.resize(&s);
    }

    /// A click on the composer asks for the keyboard. The gate opens only once the panel is seen
    /// to have it.
    pub(super) fn take_focus(&self) {
        self.take_focus_locked(&mut self.lock());
    }

    /// The panel's window gained or lost focus, or another window came to the front.
    pub(super) fn foreground_changed(&self) {
        let mut s = self.lock();
        if s.open {
            // Upstream's watchdog raises the notch on the same occasions; the panel ends above.
            self.service.raise_topmost();
        }
        let observed = self.observed(&s);
        self.apply_focus(&mut s, observed);
    }

    pub(super) fn timer(&self, timer: Timer) {
        match timer {
            Timer::Confirm => {
                let mut s = self.lock();
                let observed = self.observed(&s);
                self.apply_focus(&mut s, observed);
            }
        }
    }

    /// The sealed self-test's stand-in for "the panel is the foreground window", fed through the
    /// path a real confirmation takes (a hosted runner may refuse the panel the foreground, so
    /// the test can't wait for a real one). Answers whether it was taken: never outside a sealed
    /// run, never for a closed panel. The next real look at the foreground overrules it.
    // Reached from the self-test, which comes with `selftest.rs`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn confirm_for_self_test(&self) -> bool {
        if !self.sealed {
            return false;
        }
        let mut s = self.lock();
        if !s.open {
            return false;
        }
        self.apply_focus(&mut s, true);
        true
    }

    /// What the panel shows now.
    pub(super) fn status(&self) -> Status {
        self.lock().clone()
    }

    fn open_locked(&self, s: &mut Status, request: PanelRequest, ring: Option<RingRect>) {
        let auto = request.reason == "auto";
        // Something opening by itself never takes over a panel in use.
        if auto && s.open {
            return;
        }
        let was_open = s.open;
        // An automatic open shows the list with the session's row pointed out, not its chat: a
        // chat's composer is where a stray key would do harm.
        let (route, highlight) = match request.route.strip_prefix(CHAT_PREFIX) {
            Some(session) if auto => (
                LIST.to_string(),
                request.highlight.or_else(|| Some(session.to_string())),
            ),
            _ => (request.route, request.highlight),
        };
        let payload = json!({
            "route": route,
            "ring_id": request.ring_id,
            "highlight": highlight,
            "reason": request.reason,
        });
        let activate = takes_keyboard(&request.reason);
        let place = Place {
            ring_id: request.ring_id.clone(),
            ring,
            mode: mode_of(Some(&route)),
        };
        s.open = true;
        s.route = Some(route);
        s.ring_id = request.ring_id;
        s.reason = Some(request.reason);
        if activate {
            self.save_foreground(s);
            self.service.set_focusable(true);
            self.service.show(&payload, place, true);
            self.service.request_foreground();
        } else {
            if !was_open {
                self.service.set_focusable(false);
            }
            self.service.show(&payload, place, false);
        }
        self.service.raise_topmost();
        if activate {
            self.confirm_later();
        }
        self.report(s);
    }

    fn take_focus_locked(&self, s: &mut Status) {
        if !s.open {
            return;
        }
        self.save_foreground(s);
        self.service.set_focusable(true);
        self.service.request_foreground();
        self.confirm_later();
    }

    fn close_locked(&self, s: &mut Status) {
        if !s.open {
            return;
        }
        // Read before hiding: a hidden window holds nothing.
        let held = self.observed(s);
        s.open = false;
        s.engaged = false;
        s.content_height = None;
        self.service.hide();
        if s.focused {
            s.focused = false;
            self.service
                .emit("an:panel_focus", json!({ "focused": false }));
        }
        // The keyboard goes back to where it was, unless the user has already taken it elsewhere
        // (then nothing is taken from them) or that window is gone.
        if let Some(saved) = s.saved_foreground.take() {
            if held && self.service.exists(saved) {
                self.service.restore_foreground(saved);
            }
        }
        self.report(s);
    }

    /// Remembers who has the keyboard before the panel asks for it. The latest window that isn't
    /// the panel wins: a user who went to another window while the panel was open gets that one
    /// back.
    fn save_foreground(&self, s: &mut Status) {
        match self.service.keyboard_owner() {
            Some(window) if Some(window) != self.service.panel() => {
                s.saved_foreground = Some(window);
            }
            _ => {}
        }
    }

    fn confirm_later(&self) {
        for ms in CONFIRM_AFTER_MS {
            self.service.after(ms, Timer::Confirm);
        }
    }

    /// Whether the open panel is the foreground window, as the OS says right now.
    fn observed(&self, s: &Status) -> bool {
        s.open
            && match (self.service.panel(), self.service.foreground()) {
                (Some(panel), Some(foreground)) => panel == foreground,
                _ => false,
            }
    }

    /// The one place the gate moves: the page and the hub hear of a change, and only of a change.
    fn apply_focus(&self, s: &mut Status, focused: bool) {
        if s.focused == focused {
            return;
        }
        s.focused = focused;
        self.service
            .emit("an:panel_focus", json!({ "focused": focused }));
        self.report(s);
    }

    fn resize(&self, s: &Status) {
        self.service
            .set_content(mode_of(s.route.as_deref()), s.content_height);
    }

    fn report(&self, s: &Status) {
        (self.report)(json!({
            "open": s.open,
            "route": s.route,
            "ring_id": s.ring_id,
            "reason": s.reason,
            "engaged": s.engaged,
            "focused": s.focused,
            "pinned": s.pinned,
        }));
    }
}

/// The reasons that open the panel to be typed into. Anything else (an automatic open, a reason
/// this build doesn't know) leaves the keyboard where it is.
fn takes_keyboard(reason: &str) -> bool {
    matches!(
        reason,
        "ring_click" | "hover_row" | "peek_click" | "hotkey" | "notification" | "settings"
    )
}

fn mode_of(route: Option<&str>) -> PanelMode {
    match route {
        Some(route) if route.starts_with(CHAT_PREFIX) => PanelMode::Chat,
        _ => PanelMode::List,
    }
}

// ---- the running app's panel ----

static CORE: OnceLock<Core<PanelWindow>> = OnceLock::new();

fn core(app: &AppHandle) -> &'static Core<PanelWindow> {
    CORE.get_or_init(|| {
        Core::new(
            PanelWindow::new(app.clone()),
            super::sealed(),
            Box::new(send_report),
        )
    })
}

/// The hub asked for the panel (`HubEvent::Panel`).
pub(super) fn open_request(app: &AppHandle, request: PanelRequest) {
    core(app).open(request);
}

/// A page or the notch menu asked for the panel.
pub(super) fn open_route(app: &AppHandle, route: String, ring_id: Option<String>, reason: String) {
    core(app).open(PanelRequest {
        route,
        ring_id,
        highlight: None,
        reason,
    });
}

pub(super) fn toggle(
    app: &AppHandle,
    ring_id: Option<String>,
    reason: String,
    ring: Option<RingRect>,
) {
    core(app).toggle(ring_id, reason, ring);
}

/// The panel shortcut was pressed (`hotkey.rs`).
pub(super) fn hotkey(app: &AppHandle) {
    core(app).hotkey();
}

pub(super) fn close(app: &AppHandle) {
    core(app).close();
}

pub(super) fn close_by_hub(app: &AppHandle, reason: &str) {
    core(app).close_by_hub(reason);
}

pub(super) fn set_pinned(app: &AppHandle, on: bool) {
    core(app).set_pinned(on);
}

pub(super) fn report_size(app: &AppHandle, width: f64, height: f64) {
    core(app).report_size(width, height);
}

pub(super) fn take_focus(app: &AppHandle) {
    core(app).take_focus();
}

// The ones below come from places that hold no app handle. Before the first call that has one
// there is no panel, so there is nothing to tell.

/// The window service found nothing left to hang the panel off (its display was unplugged).
pub(super) fn close_unanchored() {
    if let Some(core) = CORE.get() {
        core.close();
    }
}

/// See [`Core::outside_click`].
pub(super) fn outside_click() {
    if let Some(core) = CORE.get() {
        core.outside_click();
    }
}

pub(super) fn set_route(route: String) {
    if let Some(core) = CORE.get() {
        core.set_route(route);
    }
}

pub(super) fn set_engaged(on: bool) {
    if let Some(core) = CORE.get() {
        core.set_engaged(on);
    }
}

/// The panel's window gained or lost focus (`WindowEvent::Focused`), or another window came to
/// the front while it was open (both from `panel_window`).
pub(super) fn foreground_changed() {
    if let Some(core) = CORE.get() {
        core.foreground_changed();
    }
}

/// A timer the window service was handed is due.
pub(super) fn timer(timer: Timer) {
    if let Some(core) = CORE.get() {
        core.timer(timer);
    }
}

/// See [`Core::confirm_for_self_test`].
// Reached from the self-test, which comes with `selftest.rs`.
#[allow(dead_code)]
pub(super) fn confirm_for_self_test() -> bool {
    CORE.get().is_some_and(Core::confirm_for_self_test)
}

/// What the panel shows now; `None` before it was ever asked for.
pub(super) fn status() -> Option<Status> {
    CORE.get().map(Core::status)
}

/// Hands a `panel_state` to the reporter thread.
///
/// Through one thread, in order: this is reached from the hub's own event callback too, where a
/// call back into the hub would wait on itself, and reports sent from separate threads could
/// arrive out of order (a "closed" overtaken by the "open" before it).
fn send_report(state: Value) {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;

    const PANEL: isize = 0x700;
    const TERMINAL: isize = 0x100;
    const EDITOR: isize = 0x200;
    /// The app's own notch window.
    const NOTCH: isize = 0x600;
    const RING_A: &str = "claude-acct-5f3e1d2c0b9a";
    const RING_B: &str = "claude-acct-8a7b6c5d4e3f";

    #[derive(Debug, Clone, PartialEq)]
    enum Op {
        Show { activate: bool },
        Hide,
        Focusable(bool),
        RequestForeground,
        RestoreForeground(isize),
        RaiseTopmost,
        Content(PanelMode, Option<f64>),
        Emit(String, Value),
    }

    /// A desktop with a terminal and an editor, and a panel that exists once it was shown.
    struct Desktop {
        ops: Vec<Op>,
        foreground: Option<isize>,
        /// The last window of another app that had the keyboard, as the service tracks it.
        other: Option<isize>,
        panel: Option<isize>,
        /// What each show was placed by, in order.
        places: Vec<Place>,
        visible: bool,
        windows: HashSet<isize>,
        /// Whether a foreground request is granted (Windows decides, not the asker).
        grants: bool,
        now: u64,
        timers: Vec<(u64, Timer)>,
    }

    #[derive(Clone)]
    struct Fake(Arc<Mutex<Desktop>>);

    impl Fake {
        fn new() -> Self {
            Fake(Arc::new(Mutex::new(Desktop {
                ops: Vec::new(),
                foreground: Some(TERMINAL),
                other: None,
                panel: None,
                places: Vec::new(),
                visible: false,
                windows: HashSet::from([TERMINAL, EDITOR, NOTCH]),
                grants: false,
                now: 0,
                timers: Vec::new(),
            })))
        }

        fn desk(&self) -> MutexGuard<'_, Desktop> {
            self.0.lock().unwrap()
        }

        fn ops(&self) -> Vec<Op> {
            self.desk().ops.clone()
        }

        fn clear(&self) {
            self.desk().ops.clear();
        }

        fn count(&self, op: &Op) -> usize {
            self.desk().ops.iter().filter(|o| *o == op).count()
        }

        /// Every `an:panel_focus` the page was sent, in order.
        fn focus_events(&self) -> Vec<bool> {
            self.desk()
                .ops
                .iter()
                .filter_map(|op| match op {
                    Op::Emit(event, payload) if event == "an:panel_focus" => {
                        payload["focused"].as_bool()
                    }
                    _ => None,
                })
                .collect()
        }
    }

    impl WindowService for Fake {
        fn show(&self, request: &Value, place: Place, activate: bool) {
            let mut d = self.desk();
            d.panel = Some(PANEL);
            d.windows.insert(PANEL);
            d.visible = true;
            d.places.push(place);
            d.ops.push(Op::Show { activate });
            // The real service adds where it put the panel; the request itself goes as it came.
            d.ops.push(Op::Emit("an:panel".into(), request.clone()));
        }
        fn hide(&self) {
            let mut d = self.desk();
            d.visible = false;
            // Windows hands the foreground to some other window; which one isn't the panel's
            // business.
            if d.foreground == Some(PANEL) {
                d.foreground = None;
            }
            d.ops.push(Op::Hide);
        }
        fn set_focusable(&self, focusable: bool) {
            self.desk().ops.push(Op::Focusable(focusable));
        }
        fn request_foreground(&self) {
            let mut d = self.desk();
            if d.grants {
                d.foreground = Some(PANEL);
            }
            d.ops.push(Op::RequestForeground);
        }
        fn restore_foreground(&self, window: isize) {
            let mut d = self.desk();
            d.foreground = Some(window);
            d.ops.push(Op::RestoreForeground(window));
        }
        fn foreground(&self) -> Option<isize> {
            self.desk().foreground
        }
        fn keyboard_owner(&self) -> Option<isize> {
            let d = self.desk();
            d.foreground.filter(|window| *window != NOTCH).or(d.other)
        }
        fn panel(&self) -> Option<isize> {
            self.desk().panel
        }
        fn exists(&self, window: isize) -> bool {
            self.desk().windows.contains(&window)
        }
        fn raise_topmost(&self) {
            self.desk().ops.push(Op::RaiseTopmost);
        }
        fn set_content(&self, mode: PanelMode, height: Option<f64>) {
            self.desk().ops.push(Op::Content(mode, height));
        }
        fn emit(&self, event: &str, payload: Value) {
            self.desk().ops.push(Op::Emit(event.to_string(), payload));
        }
        fn after(&self, ms: u64, timer: Timer) {
            let mut d = self.desk();
            let due = d.now + ms;
            d.timers.push((due, timer));
        }
    }

    struct Rig {
        core: Core<Fake>,
        fake: Fake,
        reports: Arc<Mutex<Vec<Value>>>,
    }

    impl Rig {
        fn new() -> Self {
            Self::with(false)
        }

        fn with(sealed: bool) -> Self {
            let fake = Fake::new();
            let reports = Arc::new(Mutex::new(Vec::new()));
            let sink = reports.clone();
            let core = Core::new(
                fake.clone(),
                sealed,
                Box::new(move |state| sink.lock().unwrap().push(state)),
            );
            Rig {
                core,
                fake,
                reports,
            }
        }

        fn open(&self, route: &str, ring: Option<&str>, reason: &str) {
            self.core.open(PanelRequest {
                route: route.into(),
                ring_id: ring.map(str::to_string),
                highlight: None,
                reason: reason.into(),
            });
        }

        /// Lets `ms` milliseconds pass, running the timers that fall due, in order. No sleeping.
        fn advance(&self, ms: u64) {
            let end = self.fake.desk().now + ms;
            loop {
                let next = {
                    let mut d = self.fake.desk();
                    let due = d
                        .timers
                        .iter()
                        .enumerate()
                        .filter(|(_, (at, _))| *at <= end)
                        .min_by_key(|(_, (at, _))| *at)
                        .map(|(index, _)| index);
                    due.map(|index| {
                        let (at, timer) = d.timers.remove(index);
                        d.now = at;
                        timer
                    })
                };
                match next {
                    Some(timer) => self.core.timer(timer),
                    None => break,
                }
            }
            self.fake.desk().now = end;
        }

        /// The foreground window changes, and the panel's window hears of it.
        fn foreground(&self, window: Option<isize>) {
            self.fake.desk().foreground = window;
            self.core.foreground_changed();
        }

        fn grants(&self, on: bool) {
            self.fake.desk().grants = on;
        }

        fn status(&self) -> Status {
            self.core.status()
        }

        fn reports(&self) -> Vec<Value> {
            self.reports.lock().unwrap().clone()
        }

        fn last_report(&self) -> Value {
            self.reports().last().cloned().expect("a report")
        }
    }

    // DESIGN-WIN §7.3: the panel keyboard gate stays shut until a foreground confirmation.
    #[test]
    fn the_gate_stays_shut_until_the_panel_is_seen_in_the_foreground() {
        let rig = Rig::new();
        // The request is made and nothing refuses it, but the terminal keeps the foreground.
        rig.open(LIST, Some(RING_A), "ring_click");
        assert_eq!(rig.fake.count(&Op::RequestForeground), 1);
        assert_eq!(rig.fake.count(&Op::Show { activate: true }), 1);
        assert!(!rig.status().focused);

        rig.advance(50);
        assert!(!rig.status().focused);
        rig.advance(200);
        assert!(!rig.status().focused);
        rig.advance(10_000);
        assert!(!rig.status().focused);
        assert_eq!(rig.fake.focus_events(), Vec::<bool>::new());
        assert!(rig.reports().iter().all(|r| r["focused"] == false));

        // Windows gives the panel the foreground after all.
        rig.foreground(Some(PANEL));
        assert!(rig.status().focused);
        assert_eq!(rig.fake.focus_events(), vec![true]);
        assert_eq!(rig.last_report()["focused"], true);

        // Hearing it again changes nothing and says nothing.
        rig.foreground(Some(PANEL));
        rig.advance(1_000);
        assert_eq!(rig.fake.focus_events(), vec![true]);
    }

    #[test]
    fn a_granted_request_opens_the_gate_at_the_first_look_not_before() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "hotkey");
        // Granted already, but nobody has looked: the gate follows observations only.
        assert!(!rig.status().focused);
        assert_eq!(rig.fake.focus_events(), Vec::<bool>::new());
        rig.advance(49);
        assert!(!rig.status().focused);
        rig.advance(1);
        assert!(rig.status().focused);
        rig.advance(200);
        assert_eq!(rig.fake.focus_events(), vec![true]);
    }

    #[test]
    fn a_slow_grant_is_seen_at_the_second_look() {
        let rig = Rig::new();
        rig.open(LIST, None, "settings");
        rig.advance(50);
        assert!(!rig.status().focused);
        // The desktop switches late, and no focus event arrives.
        rig.fake.desk().foreground = Some(PANEL);
        rig.advance(199);
        assert!(!rig.status().focused);
        rig.advance(1);
        assert!(rig.status().focused);
        assert_eq!(rig.fake.focus_events(), vec![true]);
    }

    #[test]
    fn every_asking_reason_saves_the_foreground_shows_and_asks() {
        for reason in [
            "ring_click",
            "hover_row",
            "peek_click",
            "hotkey",
            "notification",
            "settings",
        ] {
            let rig = Rig::new();
            rig.open(LIST, Some(RING_A), reason);
            let ops = rig.fake.ops();
            let at = |op: &Op| ops.iter().position(|o| o == op).expect(reason);
            // Focusable before it is shown, shown before the foreground is asked for.
            assert!(at(&Op::Focusable(true)) < at(&Op::Show { activate: true }));
            assert!(at(&Op::Show { activate: true }) < at(&Op::RequestForeground));
            assert_eq!(rig.status().saved_foreground, Some(TERMINAL), "{reason}");
            assert_eq!(rig.fake.desk().timers.len(), 2, "{reason}");
            assert_eq!(rig.status().reason.as_deref(), Some(reason));
        }
    }

    #[test]
    fn an_auto_open_never_asks_for_the_foreground_and_leaves_the_gate_shut() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open("session:abc", Some(RING_A), "auto");

        let ops = rig.fake.ops();
        assert!(ops.contains(&Op::Focusable(false)));
        assert!(ops.contains(&Op::Show { activate: false }));
        assert!(!ops.contains(&Op::RequestForeground));
        assert!(!ops.contains(&Op::Focusable(true)));
        assert!(rig.fake.desk().timers.is_empty());
        assert_eq!(rig.status().saved_foreground, None);
        rig.advance(10_000);
        assert!(!rig.status().focused);
        assert_eq!(rig.fake.focus_events(), Vec::<bool>::new());
        assert_eq!(rig.fake.foreground(), Some(TERMINAL));

        // It lands on the list, with the session's row pointed out.
        assert_eq!(rig.status().route.as_deref(), Some(LIST));
        let request = ops
            .iter()
            .find_map(|op| match op {
                Op::Emit(event, payload) if event == "an:panel" => Some(payload.clone()),
                _ => None,
            })
            .expect("an:panel");
        assert_eq!(
            request,
            json!({ "route": "sessions", "ring_id": RING_A, "highlight": "abc", "reason": "auto" })
        );
    }

    #[test]
    fn panel_take_focus_makes_it_focusable_and_the_gate_opens_only_on_confirmation() {
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "auto");
        rig.fake.clear();

        // The click on the composer: asked for, not yet given.
        rig.core.take_focus();
        let ops = rig.fake.ops();
        let at = |op: &Op| ops.iter().position(|o| o == op).expect("op");
        assert!(at(&Op::Focusable(true)) < at(&Op::RequestForeground));
        assert_eq!(rig.status().saved_foreground, Some(TERMINAL));
        assert!(!rig.status().focused);
        rig.advance(250);
        assert!(!rig.status().focused);
        assert_eq!(rig.fake.focus_events(), Vec::<bool>::new());

        // A second click, which Windows honours.
        rig.grants(true);
        rig.core.take_focus();
        assert!(!rig.status().focused);
        rig.advance(50);
        assert!(rig.status().focused);
        assert_eq!(rig.fake.focus_events(), vec![true]);
    }

    #[test]
    fn panel_take_focus_does_nothing_for_a_closed_panel() {
        let rig = Rig::new();
        rig.core.take_focus();
        assert!(rig.fake.ops().is_empty());
        assert!(rig.fake.desk().timers.is_empty());
    }

    #[test]
    fn an_auto_open_does_not_replace_an_open_panel() {
        let rig = Rig::new();
        rig.open("session:abc", Some(RING_A), "hover_row");
        let before = rig.status();
        let reports = rig.reports().len();
        rig.fake.clear();

        rig.open("session:other", Some(RING_B), "auto");
        assert_eq!(rig.status(), before);
        assert!(rig.fake.ops().is_empty());
        assert_eq!(rig.reports().len(), reports);
    }

    #[test]
    fn the_same_ring_closes_and_another_ring_retargets() {
        let rig = Rig::new();
        rig.core
            .toggle(Some(RING_A.into()), "ring_click".into(), None);
        assert!(rig.status().open);
        assert_eq!(rig.status().ring_id.as_deref(), Some(RING_A));
        assert_eq!(rig.status().route.as_deref(), Some(LIST));

        rig.core
            .toggle(Some(RING_B.into()), "ring_click".into(), None);
        assert!(rig.status().open);
        assert_eq!(rig.status().ring_id.as_deref(), Some(RING_B));
        assert_eq!(rig.fake.count(&Op::Hide), 0);
        assert_eq!(rig.fake.count(&Op::Show { activate: true }), 2);

        rig.core
            .toggle(Some(RING_B.into()), "ring_click".into(), None);
        assert!(!rig.status().open);
        assert_eq!(rig.fake.count(&Op::Hide), 1);

        // Closed, the same ring opens it again.
        rig.core
            .toggle(Some(RING_B.into()), "ring_click".into(), None);
        assert!(rig.status().open);
    }

    #[test]
    fn a_click_retargets_a_panel_that_opened_by_itself_and_asks_for_the_keyboard() {
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "auto");
        rig.fake.clear();
        rig.core
            .toggle(Some(RING_B.into()), "ring_click".into(), None);
        let ops = rig.fake.ops();
        assert!(ops.contains(&Op::Focusable(true)));
        assert!(ops.contains(&Op::RequestForeground));
        assert_eq!(rig.status().reason.as_deref(), Some("ring_click"));
    }

    // The port of ClaudePanelController.returnsFrontOnClose's cases: the front goes back only
    // when the panel took it and there is a window to give it to.
    #[test]
    fn close_gives_the_foreground_back_when_the_panel_held_it_and_the_window_exists() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "notification");
        rig.advance(50);
        assert!(rig.status().focused);

        rig.core.close();
        let ops = rig.fake.ops();
        let at = |op: &Op| ops.iter().position(|o| o == op).expect("op");
        assert!(at(&Op::Hide) < at(&Op::RestoreForeground(TERMINAL)));
        assert_eq!(rig.fake.foreground(), Some(TERMINAL));
        assert_eq!(rig.status().saved_foreground, None);
    }

    #[test]
    fn close_restores_nothing_when_the_saved_window_is_gone() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        rig.fake.desk().windows.remove(&TERMINAL);

        rig.core.close();
        assert_eq!(rig.fake.count(&Op::Hide), 1);
        assert!(!rig
            .fake
            .ops()
            .iter()
            .any(|op| matches!(op, Op::RestoreForeground(_))));
    }

    #[test]
    fn close_restores_nothing_when_the_panel_did_not_hold_the_foreground() {
        // Never granted: the terminal kept the keyboard all along.
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(250);
        rig.core.close();
        assert!(!rig
            .fake
            .ops()
            .iter()
            .any(|op| matches!(op, Op::RestoreForeground(_))));

        // Granted, but the user has gone to the editor since: nothing is taken from them.
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        rig.foreground(Some(EDITOR));
        rig.core.close();
        assert!(!rig
            .fake
            .ops()
            .iter()
            .any(|op| matches!(op, Op::RestoreForeground(_))));
        assert_eq!(rig.fake.foreground(), Some(EDITOR));
    }

    #[test]
    fn close_restores_nothing_when_nothing_was_saved() {
        // An automatic open saves nothing, even if the panel ends up in the foreground.
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "auto");
        rig.foreground(Some(PANEL));
        rig.core.close();
        assert!(!rig
            .fake
            .ops()
            .iter()
            .any(|op| matches!(op, Op::RestoreForeground(_))));

        // No window had the foreground when it opened.
        let rig = Rig::new();
        rig.fake.desk().foreground = None;
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        rig.core.close();
        assert!(!rig
            .fake
            .ops()
            .iter()
            .any(|op| matches!(op, Op::RestoreForeground(_))));
    }

    #[test]
    fn the_window_saved_is_the_last_one_that_had_the_keyboard() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        // The user works in the editor for a while, then clicks the panel's composer.
        rig.foreground(Some(EDITOR));
        rig.core.take_focus();
        rig.advance(50);
        assert!(rig.status().focused);
        // Asking again while the panel has the keyboard keeps the editor.
        rig.core.take_focus();
        rig.core.close();
        assert_eq!(rig.fake.count(&Op::RestoreForeground(EDITOR)), 1);
    }

    #[test]
    fn losing_the_foreground_shuts_the_gate_and_reports_it() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        assert_eq!(rig.last_report()["focused"], true);

        rig.foreground(Some(TERMINAL));
        assert!(!rig.status().focused);
        assert!(rig.status().open);
        assert_eq!(rig.fake.focus_events(), vec![true, false]);
        assert_eq!(rig.last_report()["focused"], false);
        assert_eq!(rig.last_report()["open"], true);

        // And back.
        rig.foreground(Some(PANEL));
        assert_eq!(rig.fake.focus_events(), vec![true, false, true]);
    }

    #[test]
    fn closing_shuts_the_gate_and_a_late_look_cannot_open_it() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        rig.core.close();
        assert!(!rig.status().focused);
        assert_eq!(rig.fake.focus_events(), vec![true, false]);
        assert_eq!(rig.last_report()["focused"], false);

        // The 250 ms look arrives after the close, with a stale foreground.
        rig.fake.desk().foreground = Some(PANEL);
        rig.advance(250);
        rig.core.foreground_changed();
        assert!(!rig.status().focused);
        assert_eq!(rig.fake.focus_events(), vec![true, false]);
    }

    #[test]
    fn panel_state_reports_arrive_in_order_with_pinned_engaged_and_focused() {
        let rig = Rig::new();
        rig.grants(true);
        rig.core.set_pinned(true);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.advance(50);
        rig.core.set_engaged(true);
        rig.core.set_route("session:abc".into());
        rig.core.set_pinned(false);
        rig.core.set_pinned(false);
        rig.core.close();

        let state = |open: bool,
                     route: Option<&str>,
                     ring: Option<&str>,
                     reason: Option<&str>,
                     engaged: bool,
                     focused: bool,
                     pinned: bool| {
            json!({
                "open": open, "route": route, "ring_id": ring, "reason": reason,
                "engaged": engaged, "focused": focused, "pinned": pinned,
            })
        };
        let click = Some("ring_click");
        assert_eq!(
            rig.reports(),
            vec![
                state(false, None, None, None, false, false, true),
                state(true, Some(LIST), Some(RING_A), click, false, false, true),
                state(true, Some(LIST), Some(RING_A), click, false, true, true),
                state(true, Some(LIST), Some(RING_A), click, true, true, true),
                state(
                    true,
                    Some("session:abc"),
                    Some(RING_A),
                    click,
                    true,
                    true,
                    true
                ),
                state(
                    true,
                    Some("session:abc"),
                    Some(RING_A),
                    click,
                    true,
                    true,
                    false
                ),
                state(
                    false,
                    Some("session:abc"),
                    Some(RING_A),
                    click,
                    false,
                    false,
                    false
                ),
            ]
        );
        // Each is the engine's `PanelState`.
        for report in rig.reports() {
            let parsed: agentnotch_engine::runtime_types::PanelState =
                serde_json::from_value(report.clone()).expect("PanelState");
            assert_eq!(serde_json::to_value(parsed).unwrap(), report);
        }
    }

    #[test]
    fn a_pinned_panel_ignores_outside_clicks_and_jump_closes() {
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.core.set_pinned(true);

        rig.core.outside_click();
        assert!(rig.status().open);
        rig.core.close_by_hub("jump");
        assert!(rig.status().open);
        assert_eq!(rig.fake.count(&Op::Hide), 0);

        // The engine's auto-close and the user's own close still apply.
        rig.core.close_by_hub("auto_close");
        assert!(!rig.status().open);
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.core.close();
        assert!(!rig.status().open);
    }

    #[test]
    fn an_unpinned_panel_closes_on_an_outside_click_and_on_a_jump() {
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.core.outside_click();
        assert!(!rig.status().open);

        rig.open(LIST, Some(RING_A), "ring_click");
        rig.core.close_by_hub("jump");
        assert!(!rig.status().open);

        // Closing a closed panel does nothing and reports nothing.
        let reports = rig.reports().len();
        rig.core.close();
        rig.core.outside_click();
        rig.core.close_by_hub("jump");
        assert_eq!(rig.reports().len(), reports);
        assert_eq!(rig.fake.count(&Op::Hide), 2);
    }

    // How tall the window gets is the placement's answer (the engine's caps and the work area;
    // `panel_window`'s tests pin it): the rules hand on what the page said, and only that.
    #[test]
    fn size_reports_reach_the_window_with_the_routes_mode() {
        let rig = Rig::new();
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.fake.clear();

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -40.0] {
            rig.core.report_size(440.0, bad);
        }
        assert!(rig.fake.ops().is_empty());

        rig.core.report_size(440.0, 10.0);
        rig.core.report_size(440.0, 431.5);
        rig.core.report_size(440.0, 5_000.0);
        assert_eq!(
            rig.fake.ops(),
            vec![
                Op::Content(PanelMode::List, Some(10.0)),
                Op::Content(PanelMode::List, Some(431.5)),
                Op::Content(PanelMode::List, Some(5_000.0)),
            ]
        );

        // A chat is wider and may be taller: the height already reported is fitted again.
        rig.fake.clear();
        rig.core.set_route("session:abc".into());
        assert_eq!(
            rig.fake.ops(),
            vec![Op::Content(PanelMode::Chat, Some(5_000.0))]
        );
        rig.core.report_size(520.0, 900.0);
        assert_eq!(
            rig.fake.ops()[1..],
            [Op::Content(PanelMode::Chat, Some(900.0))]
        );

        // A closed panel has no size to follow.
        rig.core.close();
        rig.fake.clear();
        rig.core.report_size(440.0, 300.0);
        rig.core.set_route(LIST.into());
        assert!(rig.fake.ops().is_empty());

        // Opened again, nothing of the old height is left: the route alone is handed on.
        rig.open(LIST, Some(RING_A), "ring_click");
        rig.fake.clear();
        rig.core.set_route("session:abc".into());
        assert_eq!(rig.fake.ops(), vec![Op::Content(PanelMode::Chat, None)]);
    }

    #[test]
    fn the_window_is_placed_by_the_ring_its_rect_and_the_route() {
        let rig = Rig::new();
        let rect = [12.0, 300.5, 44.0, 44.0];
        rig.core
            .toggle(Some(RING_A.into()), "ring_click".into(), Some(rect));
        // A chat opened by the hub: no page measured a ring for it.
        rig.open("session:abc", Some(RING_B), "notification");
        // One that opens by itself lands on the list, and is placed as the list.
        rig.core.close();
        rig.open("session:abc", Some(RING_B), "auto");
        // The hot key: every account's list.
        rig.core.close();
        rig.core.hotkey();

        let place = |ring_id: Option<&str>, ring, mode| Place {
            ring_id: ring_id.map(str::to_string),
            ring,
            mode,
        };
        assert_eq!(
            rig.fake.desk().places,
            vec![
                place(Some(RING_A), Some(rect), PanelMode::List),
                place(Some(RING_B), None, PanelMode::Chat),
                place(Some(RING_B), None, PanelMode::List),
                place(None, None, PanelMode::List),
            ]
        );
    }

    // After a click on a ring the notch window may be the foreground window: the keyboard goes
    // back to the app the user was in, never to the notch.
    #[test]
    fn a_ring_click_saves_the_app_the_user_was_in_not_the_notch() {
        let rig = Rig::new();
        rig.grants(true);
        {
            let mut d = rig.fake.desk();
            d.foreground = Some(NOTCH);
            d.other = Some(TERMINAL);
        }
        rig.core
            .toggle(Some(RING_A.into()), "ring_click".into(), None);
        assert_eq!(rig.status().saved_foreground, Some(TERMINAL));
        rig.advance(50);
        rig.core.close();
        assert_eq!(rig.fake.count(&Op::RestoreForeground(TERMINAL)), 1);

        // No other app's window was ever seen: nothing is saved, nothing restored.
        let rig = Rig::new();
        rig.grants(true);
        rig.fake.desk().foreground = Some(NOTCH);
        rig.core
            .toggle(Some(RING_A.into()), "ring_click".into(), None);
        assert_eq!(rig.status().saved_foreground, None);
    }

    #[test]
    fn the_hot_key_opens_takes_the_keyboard_then_closes() {
        let rig = Rig::new();
        rig.core.hotkey();
        assert!(rig.status().open);
        assert_eq!(rig.status().ring_id, None);
        assert_eq!(rig.status().reason.as_deref(), Some("hotkey"));
        rig.advance(250);
        assert!(!rig.status().focused);

        // Open without the keyboard: the key asks for it again instead of closing.
        rig.grants(true);
        rig.core.hotkey();
        assert!(rig.status().open);
        assert_eq!(rig.fake.count(&Op::RequestForeground), 2);
        rig.advance(50);
        assert!(rig.status().focused);

        rig.core.hotkey();
        assert!(!rig.status().open);
        assert_eq!(rig.fake.foreground(), Some(TERMINAL));
    }

    #[test]
    fn the_self_tests_confirmation_takes_the_real_path_and_only_when_sealed() {
        // A live run has no such entry.
        let live = Rig::new();
        live.open(LIST, Some(RING_A), "auto");
        assert!(!live.core.confirm_for_self_test());
        assert!(!live.status().focused);
        assert_eq!(live.fake.focus_events(), Vec::<bool>::new());

        let sealed = Rig::with(true);
        // Nothing to confirm for a closed panel.
        assert!(!sealed.core.confirm_for_self_test());
        sealed.open(LIST, Some(RING_A), "auto");
        assert!(!sealed.status().focused);
        assert!(sealed.core.confirm_for_self_test());
        assert!(sealed.status().focused);
        assert_eq!(sealed.fake.focus_events(), vec![true]);
        assert_eq!(sealed.last_report()["focused"], true);

        // A real look at the foreground overrules it: the terminal still has the keyboard.
        sealed.core.foreground_changed();
        assert!(!sealed.status().focused);
        assert_eq!(sealed.fake.focus_events(), vec![true, false]);
    }

    #[test]
    fn the_panel_is_raised_on_every_open_and_foreground_change_while_open() {
        let rig = Rig::new();
        rig.core.foreground_changed();
        assert_eq!(rig.fake.count(&Op::RaiseTopmost), 0);
        rig.open(LIST, Some(RING_A), "auto");
        assert_eq!(rig.fake.count(&Op::RaiseTopmost), 1);
        rig.foreground(Some(EDITOR));
        assert_eq!(rig.fake.count(&Op::RaiseTopmost), 2);
        rig.core.close();
        rig.foreground(Some(TERMINAL));
        assert_eq!(rig.fake.count(&Op::RaiseTopmost), 2);
    }

    #[test]
    fn an_unknown_reason_leaves_the_keyboard_where_it_is() {
        let rig = Rig::new();
        rig.grants(true);
        rig.open(LIST, Some(RING_A), "somethingNew");
        assert!(!rig.fake.ops().contains(&Op::RequestForeground));
        assert!(rig.fake.ops().contains(&Op::Show { activate: false }));
        assert_eq!(rig.fake.foreground(), Some(TERMINAL));
    }
}
