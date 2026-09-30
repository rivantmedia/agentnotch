//! Sealed-only launch switches (DESIGN-WIN §2.4 `selftest.rs`, §4.13, §7.4).
//!
//! - `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH=sessions|session:<id>|settings` opens that view once the
//!   app is up, for looking at the fixture UI;
//! - `AGENTNOTCH_PANEL_SELF_TEST=1` (+ `AGENTNOTCH_SELF_TEST_OUT=<json>`) is the smoke test's
//!   self-test: the app drives its own panel over every edge, looks at what Windows made of it
//!   and into its three pages, writes a report and exits 0 (nothing wrong) or 1;
//! - `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>` is the snapshot run (`snapshots.rs`): PNGs of the
//!   pages in their states and a manifest. Beside the self-test it runs after it, and the two
//!   share one exit code.
//!
//! None of them does anything unless the run is sealed.
//!
//! The self-test sends no input to any window and never forces the foreground. A hosted runner
//! has a desktop but no user, and Windows may refuse the panel the foreground there, so whether
//! a click's open really got the keyboard is only logged. What is asserted is the gate: shut
//! after the panel opened by itself, open once the rules were told the panel is in front (through
//! the path a real confirmation takes).
//!
//! It judges nothing itself: `selftest_report.rs` holds the checks and the list of steps. This
//! file moves the notch through upstream's own switches (and puts them back), opens and closes
//! the panel through the panel's own entries, and reads windows and pages. Every step goes to
//! run.log: on CI that and the report are all there is to see.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agentnotch_engine::geometry::panel::{PanelEdge, PxRect};
use agentnotch_win::window;
use serde_json::{json, Value};
use tauri::{AppHandle, Listener, Manager, WebviewWindow};

use super::panel_window::{self, Placed};
use super::selftest_report::{
    self as report, EdgeReport, Rect, Report, Step, FLOATING, NOTCH, PANEL, SETTINGS,
};
use super::{panel, snapshots, webview};

/// The whole run. Past it the watchdog writes what there is and ends the process.
const DEADLINE: Duration = Duration::from_secs(120);
/// How long the notch window and its page get to come up.
const NOTCH_SETTLES: Duration = Duration::from_secs(20);
/// A window's first build, WebView2 included.
const WINDOW_BUILDS: Duration = Duration::from_secs(15);
const CLOSES: Duration = Duration::from_secs(3);
/// After an open: the rules' two looks at the foreground (50 and 250 ms) and the raise above the
/// notch have happened. The simulated confirmation must come after them, or a real look
/// overrules it.
const LOOKS_ARE_OVER: Duration = Duration::from_millis(400);
/// After a close: a panel that had the keyboard asks up to 240 ms later where it went, and
/// would close a panel opened in the meantime.
const CLOSE_SETTLES: Duration = Duration::from_millis(400);
/// Upstream says `notch_edge` when it has placed the notch.
const NOTCH_MOVES: Duration = Duration::from_secs(5);
/// ... and the notch's page lays itself out for the edge after that.
const EDGE_SETTLES: Duration = Duration::from_millis(500);
const MAIN_THREAD_ANSWERS: Duration = Duration::from_secs(10);
const PAGE_ANSWERS: Duration = Duration::from_secs(10);
const QUICK: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);
/// The sealed hub has nothing to save; stopping it gets this long before the process ends.
const HUB_STOPS: Duration = Duration::from_secs(2);
/// After the verdict the app is asked to quit; should its event loop not get to it, the process
/// ends itself with the same code.
const QUITS: Duration = Duration::from_secs(5);

/// How often upstream has said `notch_edge`.
static NOTCH_EDGE_EVENTS: AtomicU64 = AtomicU64::new(0);
/// Set by whoever ends the run, the self-test or its watchdog: only one of them writes.
static ENDED: AtomicBool = AtomicBool::new(false);
/// The step the run is in and the report so far, for the watchdog.
static PROGRESS: Mutex<Option<(Step, Report)>> = Mutex::new(None);

// ---- what runs inside the pages ----

/// Goes into the panel's page beside the collector: keeps every `an:panel_focus` the page is
/// sent, through the page's own `window.__TAURI__`. WP0's stub page listens to nothing itself,
/// and a page that can't listen (no `__TAURI__`, or the `agentnotch` capability without
/// `core:default`) could never be told the gate's state: that is reported, not worked around.
const FOCUS_LISTENER: &str = r#"(function () {
  var s = window.__anSelfTest = window.__anSelfTest || Object.create(null);
  if (s.focus) { return; }
  s.focus = [];
  s.focusMark = 0;
  s.focusListener = "pending";
  function why(e) { return "refused: " + String(e && e.message ? e.message : e); }
  function attach() {
    if (s.focusListener !== "pending") { return; }
    var t = window.__TAURI__;
    if (!t || !t.event || typeof t.event.listen !== "function") { return; }
    s.focusListener = "asked";
    try {
      t.event.listen("an:panel_focus", function (e) {
        s.focus.push(!!(e && e.payload && e.payload.focused === true));
      }).then(
        function () { s.focusListener = "ready"; },
        function (e) { s.focusListener = why(e); });
    } catch (e) { s.focusListener = why(e); }
  }
  attach();
  document.addEventListener("DOMContentLoaded", function () {
    attach();
    if (s.focusListener === "pending") { s.focusListener = "missing"; }
  });
})();"#;

/// The listener's state: `ready` once the page can hear `an:panel_focus`.
const FOCUS_STATE: &str = r#"(function () {
  var s = window.__anSelfTest;
  return s && s.focusListener ? s.focusListener : "missing";
})()"#;

/// Forgets what the page heard so far. Run while the panel is open and before it is closed:
/// closing only ever says "not focused".
const FOCUS_MARK: &str = r#"(function () {
  var s = window.__anSelfTest;
  if (!s || !s.focus) { return false; }
  s.focusMark = s.focus.length;
  return true;
})()"#;

/// Whether the page was told since the mark that the panel has the keyboard.
const FOCUS_SEEN: &str = r#"(function () {
  var s = window.__anSelfTest;
  if (!s || !s.focus) { return null; }
  return s.focus.slice(s.focusMark || 0).indexOf(true) >= 0;
})()"#;

/// The generic invariant of every page: no text is cut off by its box. How many are.
const TEXT_OVERFLOWS: &str = r#"(function () {
  var all = document.querySelectorAll("[data-an-text]"), cut = 0;
  for (var i = 0; i < all.length; i++) {
    if (all[i].scrollWidth > all[i].clientWidth) { cut++; }
  }
  return cut;
})()"#;

/// The round trip, through the page's own bridge.
const ROUND_TRIP: &str = r#"window.__TAURI__.core.invoke("an_call", { method: "snapshot" })"#;
const PAGE_LOADED: &str = r#"document.readyState === "complete""#;
const INNER_WIDTH: &str = "window.innerWidth";

/// Each page's object that may carry a `selfTest` hook (the fork's UI package defines them).
const HOOKS: [(&str, &str); 3] = [
    (NOTCH, "agentnotch"),
    (SETTINGS, "agentnotchSettings"),
    (PANEL, "agentnotchPanel"),
];

// ---- the switches ----

pub(super) fn start(app: &AppHandle) {
    let switches = report::switches(super::sealed(), |name| std::env::var(name).ok());
    if let Some(route) = switches.open_route.clone() {
        let app = app.clone();
        std::thread::spawn(move || {
            // After the notch's first frame, so the view opens over a settled desktop.
            std::thread::sleep(Duration::from_secs(1));
            if route == "settings" {
                crate::settings_window::open(&app);
            } else {
                panel::open_route(&app, route, None, "settings".into());
            }
        });
    }
    if !switches.self_test && switches.snapshots.is_none() {
        return;
    }
    app.listen_any("notch_edge", |_| {
        NOTCH_EDGE_EVENTS.fetch_add(1, Ordering::Relaxed);
    });
    if switches.self_test {
        start_self_test(app, switches.out, switches.snapshots);
    } else if let Some(dir) = switches.snapshots {
        start_snapshots(app, dir);
    }
}

/// The snapshot run alone: its own thread (page probes never run on the main one), then the
/// process ends with its verdict.
fn start_snapshots(app: &AppHandle, dir: PathBuf) {
    super::log(&format!(
        "snapshots: start (at most {} s)",
        snapshots::DEADLINE.as_secs()
    ));
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("an-snapshots".into())
        .spawn(move || {
            let code = panic::catch_unwind(AssertUnwindSafe(|| snapshots::run(&app, &dir)))
                .unwrap_or_else(|_| {
                    super::log("snapshots: the run panicked");
                    1
                });
            quit(Some(&app), code);
        });
    if spawned.is_err() {
        super::log("snapshots: the thread could not be started");
        std::process::exit(1);
    }
}

fn version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

fn start_self_test(app: &AppHandle, out: Option<PathBuf>, snapshots: Option<PathBuf>) {
    *progress() = Some((Step::Settle, Report::new(&version(app))));
    super::log(&format!(
        "self-test: starts (report file: {}, at most {} s)",
        if out.is_some() { "given" } else { "none" },
        DEADLINE.as_secs()
    ));

    let watchdog_out = out.clone();
    let _ = std::thread::Builder::new()
        .name("an-selftest-watchdog".into())
        .spawn(move || {
            std::thread::sleep(DEADLINE);
            let (step, so_far) = progress()
                .take()
                .unwrap_or((Step::Settle, Report::new(super::app_version())));
            // The run is stuck somewhere, maybe on the main thread: nothing here may wait on it.
            end(
                None,
                watchdog_out,
                so_far.stopped(report::timed_out_at(step)),
                None,
            );
        });

    // Its own thread, and not one named "main": the page probes wait for answers that arrive
    // on the main thread.
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("an-selftest".into())
        .spawn(move || {
            let mut run = Run {
                app: &app,
                report: Report::new(&version(&app)),
                step: Step::Settle,
                found: read_found(&app),
                ring: None,
                session: None,
            };
            let finished = panic::catch_unwind(AssertUnwindSafe(|| run.all())).is_ok();
            let (step, mut report) = (run.step, run.report);
            if finished {
                report.seal();
            } else {
                report = report.stopped(format!("the self-test panicked at {}", step.name()));
            }
            end(Some(&app), out, report, snapshots);
        });
}

fn progress() -> MutexGuard<'static, Option<(Step, Report)>> {
    PROGRESS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Writes the report, says how it went and ends the process with the verdict. Only the first
/// caller does; `app` is `None` for the watchdog, which must not wait on the app. `snapshots`
/// is the snapshot run the same launch asked for: it goes after the self-test (so the watchdog,
/// which has the self-test's 120 s, is already out of the way) and its verdict joins the code.
fn end(app: Option<&AppHandle>, out: Option<PathBuf>, report: Report, snapshots: Option<PathBuf>) {
    if ENDED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(e) = report::write(out.as_deref(), &report) {
        super::log(&format!("self-test: the report could not be written: {e}"));
    }
    for line in &report.failures {
        super::log(&format!("self-test: failed: {line}"));
    }
    super::log(&report::summary(&report));
    let mut code = report::exit_code(&report);
    if let (Some(app), Some(dir)) = (app, snapshots) {
        super::log(&format!(
            "snapshots: start (at most {} s)",
            snapshots::DEADLINE.as_secs()
        ));
        let taken = panic::catch_unwind(AssertUnwindSafe(|| snapshots::run(app, &dir)));
        code = code.max(taken.unwrap_or_else(|_| {
            super::log("snapshots: the run panicked");
            1
        }));
    }
    quit(app, code);
}

/// Stops the hub and ends the process with `code`. `app` is `None` for the watchdog.
fn quit(app: Option<&AppHandle>, code: i32) -> ! {
    // Off this thread: the verdict is out, and the process ends whether or not the hub answers.
    let (stopped, wait) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(hub) = super::hub() {
            hub.stop();
        }
        let _ = stopped.send(());
    });
    let _ = wait.recv_timeout(HUB_STOPS);
    if let Some(app) = app {
        app.exit(code);
        std::thread::sleep(QUITS);
    }
    std::process::exit(code);
}

// ---- the run ----

/// Upstream's settings the self-test changes, as it found them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) edge: String,
    pub(super) notch_visible: bool,
    tray_visible: bool,
    notch_on_hover: bool,
}

struct Run<'a> {
    app: &'a AppHandle,
    report: Report,
    step: Step,
    found: Found,
    /// The first ring and the first session of the sealed hub.
    ring: Option<String>,
    session: Option<String>,
}

impl Run<'_> {
    fn all(&mut self) {
        for step in report::steps() {
            self.enter(step);
            match step {
                Step::Settle => self.settle(),
                Step::Collectors => self.collectors(),
                Step::Edge(edge) => {
                    let moved = if edge == FLOATING {
                        show_notch(self.app, false, &self.found)
                    } else {
                        move_notch(self.app, edge)
                    };
                    let case = self.panel_case(edge, moved);
                    self.report.edges.push(case);
                }
                Step::Widths => self.widths(),
                Step::Restore => self.restore(),
                Step::Pages => self.pages(),
                // The thread that ran this writes it.
                Step::Report => {}
            }
        }
    }

    fn enter(&mut self, step: Step) {
        self.step = step;
        super::log(&format!("self-test: {}", step.name()));
        let mut progress = progress();
        // Taken by the watchdog: the run is over, whatever this thread still does.
        if progress.is_some() {
            *progress = Some((step, self.report.clone()));
        }
    }

    fn fail(&mut self, line: String) {
        super::log(&format!("self-test: {line}"));
        self.report.fail(line);
    }

    fn window(&self, label: &str) -> Option<WebviewWindow> {
        self.app.get_webview_window(label)
    }

    fn settle(&mut self) {
        let settled = wait_for(NOTCH_SETTLES, || {
            self.window(NOTCH).is_some_and(|notch| {
                notch.is_visible().unwrap_or(false)
                    && webview::eval(&notch, PAGE_LOADED, QUICK) == Ok(Value::Bool(true))
            })
        });
        if !settled {
            self.fail(format!(
                "notch: the window and its page were not up within {} s",
                NOTCH_SETTLES.as_secs()
            ));
        }
        self.found = read_found(self.app);
        super::log(&format!("self-test: {:?}", self.found));
        if let Some(hub) = super::hub() {
            let snapshot = hub.snapshot();
            self.ring = snapshot.rings.first().map(|ring| ring.ring_id.clone());
            self.session = snapshot.sessions.first().map(|s| s.session_id.clone());
        }
        if self.ring.is_none() {
            self.fail("the sealed hub has no ring to open the panel on".into());
        }
        if !self.found.notch_visible {
            // The edges need a notch to hang off; it is hidden again at the end.
            if let Err(e) = show_notch(self.app, true, &self.found) {
                self.fail(format!("the notch could not be shown: {e}"));
            }
        }
    }

    /// The collector goes into each page, which reloads it: before anything drives the pages.
    fn collectors(&mut self) {
        let app = self.app;
        self.collector(NOTCH);

        crate::settings_window::open(app);
        if !wait_for(WINDOW_BUILDS, || self.window(SETTINGS).is_some()) {
            self.fail("settings: the window never came up".into());
        }
        self.collector(SETTINGS);

        // The panel's window is built when it is first asked for. Opened the way it opens by
        // itself, so nothing is activated for this.
        panel::open_route(app, "sessions".into(), self.ring.clone(), "auto".into());
        if !wait_for(WINDOW_BUILDS, || self.window(PANEL).is_some()) {
            self.fail("agentnotch-panel: the window never came up".into());
        }
        if let Some(page) = self.window(PANEL) {
            if let Err(e) = webview::add_document_script(&page, FOCUS_LISTENER) {
                self.fail(format!(
                    "agentnotch-panel: the focus listener could not be added: {e}"
                ));
            }
        }
        self.collector(PANEL);
        if let Some(page) = self.window(PANEL) {
            let state = || match webview::eval(&page, FOCUS_STATE, QUICK) {
                Ok(Value::String(state)) => state,
                Ok(_) => "missing".into(),
                Err(e) => e,
            };
            if !wait_for(QUICK, || state() == "ready") {
                self.fail(format!(
                    "agentnotch-panel: the page can't listen to an:panel_focus ({}): it needs \
                     window.__TAURI__ and core:default in the agentnotch capability",
                    state()
                ));
            }
        }
        self.close_panel();
    }

    fn collector(&mut self, label: &str) {
        let Some(page) = self.window(label) else {
            return self.fail(format!("{label}: no window to look into"));
        };
        match webview::install_collector(&page) {
            Ok(()) => super::log(&format!("self-test: collector in {label}")),
            Err(e) => self.fail(format!(
                "{label}: the collector could not be installed: {e}"
            )),
        }
    }

    /// One edge: the panel as a click opens it, then as it opens by itself.
    fn panel_case(&mut self, edge: &str, moved: Result<(), String>) -> EdgeReport {
        let app = self.app;
        let mut case = EdgeReport::new(edge);
        if let Err(e) = moved {
            case.failures
                .push(format!("the notch did not get there: {e}"));
        }

        // As a click on the ring opens it.
        panel::open_route(
            app,
            "sessions".into(),
            self.ring.clone(),
            "ring_click".into(),
        );
        let Some((handle, placed)) = self.opened() else {
            case.failures
                .push("the panel did not open for a ring click".into());
            self.close_panel();
            return case;
        };
        let (card, mismatch) = card_on_screen(handle, &placed);
        case.failures.extend(mismatch);
        case.panel = card;
        case.work_area = work_area(handle, &placed);
        case.tail_offset = placed.placement.tail_offset;
        case.tail_limit = placed.placement.tail_limit;
        let wanted = (edge != FLOATING).then_some(edge);
        let hangs_off = placed
            .anchor
            .edge
            .filter(|_| !placed.placement.floating)
            .map(edge_word);
        if hangs_off != wanted {
            case.failures.push(format!(
                "the panel should {}; it is placed to {}",
                describe(wanted),
                describe(hangs_off)
            ));
        }
        let styles = window::styles(handle);
        case.topmost = styles.is_some_and(|s| s.topmost);
        let notch = self
            .window(NOTCH)
            .and_then(|notch| panel_window::hwnd_of(&notch));
        let above = notch.and_then(|notch| window::is_above(handle, notch));
        // With the notch hidden there is nothing to be above.
        case.above_notch = case.floating || above == Some(true);
        // Information only: Windows may refuse a desktop without a user the foreground.
        let activated = panel::status().is_some_and(|s| s.focused);
        super::log(&format!(
            "self-test: {edge} ring_click: window={:?} card={card:?} work={:?} tail_offset={} \
             tail_limit={} styles={styles:?} above_notch={above:?} activated={activated}",
            window::window_rect(handle),
            case.work_area,
            case.tail_offset,
            case.tail_limit
        ));
        // While the page can still be asked: a closing panel only ever says "not focused".
        let marked = self
            .window(PANEL)
            .is_some_and(|page| webview::eval(&page, FOCUS_MARK, QUICK) == Ok(Value::Bool(true)));
        self.close_panel();

        // As it opens by itself.
        panel::open_route(app, "sessions".into(), self.ring.clone(), "auto".into());
        let Some((handle, _)) = self.opened() else {
            case.failures
                .push("the panel did not open by itself".into());
            case.check();
            self.close_panel();
            return case;
        };
        let page = self.window(PANEL);
        let seen = || {
            page.as_ref()
                .and_then(|page| webview::eval(page, FOCUS_SEEN, QUICK).ok())
                .and_then(|seen| seen.as_bool())
        };
        let styles = window::styles(handle);
        case.auto.no_activate = styles.is_some_and(|s| s.no_activate);
        let core_before = panel::status().is_some_and(|s| s.focused);
        let page_before = seen();
        case.auto.gate_shut = marked && !core_before && page_before == Some(false);
        // "The panel is the foreground window", fed into the path the real confirmation takes.
        let taken = panel::confirm_for_self_test();
        // Read at once: the next real look at the foreground shuts the gate again.
        let core_after = panel::status().is_some_and(|s| s.focused);
        let page_after = wait_for(CLOSES, || seen() == Some(true));
        case.auto.gate_opens_on_confirmation = taken && core_after && page_after;
        super::log(&format!(
            "self-test: {edge} auto: styles={styles:?} marked={marked} core_before={core_before} \
             page_before={page_before:?} taken={taken} core_after={core_after} \
             page_after={page_after}"
        ));
        case.check();
        self.close_panel();
        case
    }

    /// The narrowest card and the widest: the list beside a side notch (400) and a chat under
    /// a flat one (520).
    fn widths(&mut self) {
        // Back from the floating case first.
        if let Err(e) = show_notch(self.app, true, &self.found) {
            self.fail(format!("the notch could not be shown again: {e}"));
        }
        self.width_case("list", "right", "sessions".into(), "ring_click");
        match self.session.clone() {
            Some(session) => {
                self.width_case("chat", "top", format!("session:{session}"), "hover_row")
            }
            None => self.fail("the sealed hub has no session to open a chat on".into()),
        }
    }

    /// The panel at one width: inside the work area, nothing cut off, and the page's own hook.
    fn width_case(&mut self, name: &str, edge: &str, route: String, reason: &str) {
        let app = self.app;
        if let Err(e) = move_notch(app, edge) {
            self.fail(format!("the notch did not move to the {edge} edge: {e}"));
        }
        panel::open_route(app, route, self.ring.clone(), reason.into());
        let Some((handle, placed)) = self.opened() else {
            self.fail(format!("agentnotch-panel: the {name} did not open"));
            self.close_panel();
            return;
        };
        let (card, mismatch) = card_on_screen(handle, &placed);
        if let Some(mismatch) = mismatch {
            self.fail(format!("agentnotch-panel: {name}: {mismatch}"));
        }
        let work = work_area(handle, &placed);
        let fits = report::inside(card, work);
        super::log(&format!(
            "self-test: {name} on {edge}: card={card:?} work={work:?} width_css={} fits={fits}",
            placed.placement.width_css
        ));
        self.report
            .pages
            .panel
            .invariants
            .insert(format!("{name}_inside_work_area"), fits);
        self.page_invariants(PANEL, &format!("{name}_no_text_overflow"), edge);
        self.close_panel();
    }

    /// Upstream's edge and visibility go back to what they were.
    fn restore(&mut self) {
        let found = self.found.clone();
        if let Err(e) = move_notch(self.app, &found.edge) {
            self.fail(format!(
                "the notch could not be put back on the {} edge: {e}",
                found.edge
            ));
        }
        if !found.notch_visible {
            if let Err(e) = show_notch(self.app, false, &found) {
                self.fail(format!("the notch could not be hidden again: {e}"));
            }
        }
        let left = read_found(self.app);
        if left != found {
            self.fail(format!(
                "upstream's settings were {found:?} and are left as {left:?}"
            ));
        }
    }

    /// The generic invariant (under `name`) and the page's own hook, for the page as it is now.
    fn page_invariants(&mut self, label: &str, name: &str, edge: &str) {
        let Some(page) = self.window(label) else {
            return self.fail(format!("{label}: no window to look into"));
        };
        let cut = webview::eval(&page, TEXT_OVERFLOWS, QUICK);
        let width = webview::eval(&page, INNER_WIDTH, QUICK)
            .ok()
            .and_then(|width| width.as_f64())
            .unwrap_or(0.0);
        super::log(&format!(
            "self-test: {label} {name}: cut_off={cut:?} inner_width={width}"
        ));
        let holds = cut.ok().and_then(|cut| cut.as_u64()) == Some(0);
        let global = HOOKS
            .iter()
            .find(|(page, _)| *page == label)
            .map(|(_, global)| *global)
            .unwrap_or_default();
        let hook = page_hook(&page, global, edge, width);
        if let Some(entry) = self.report.pages.named(label) {
            entry.invariants.insert(name.to_string(), holds);
            entry.page_hook |= !matches!(hook, Ok(None));
            if let Ok(Some(answers)) = &hook {
                for (name, holds) in answers {
                    // Asked more than once (the panel, at each width): it must hold each time.
                    let all = entry.invariants.entry(name.clone()).or_insert(true);
                    *all = *all && *holds;
                }
            }
        }
        if let Err(e) = hook {
            self.fail(format!("{label}: the page's selfTest hook failed: {e}"));
        }
    }

    /// Once per run: each page's round trip, invariants, and what its collector kept.
    fn pages(&mut self) {
        let edge = self.found.edge.clone();
        // The panel's page is asked while its window is on screen.
        panel::open_route(
            self.app,
            "sessions".into(),
            self.ring.clone(),
            "ring_click".into(),
        );
        if self.opened().is_none() {
            self.fail("agentnotch-panel: the panel did not open for the page checks".into());
        }
        self.report.scale = [PANEL, NOTCH]
            .into_iter()
            .filter_map(|label| self.window(label))
            .find_map(|page| webview::device_scale(&page).ok());
        if self.report.scale.is_none() {
            self.fail("no page could say its device scale".into());
        }
        for label in [NOTCH, SETTINGS, PANEL] {
            let Some(page) = self.window(label) else {
                self.fail(format!("{label}: no window to look into"));
                continue;
            };
            let answer = webview::eval_async(&page, ROUND_TRIP, PAGE_ANSWERS);
            let round_trip = answer.as_ref().is_ok_and(report::is_snapshot);
            super::log(&format!(
                "self-test: {label} round trip: {}",
                match &answer {
                    Ok(_) if round_trip => "a snapshot".to_string(),
                    Ok(_) => "an answer without rings".to_string(),
                    Err(e) => e.clone(),
                }
            ));
            // The panel's invariants were read at each of its widths.
            if label != PANEL {
                self.page_invariants(label, "no_text_overflow", &edge);
            }
            let kept = webview::collected(&page);
            if let Some(entry) = self.report.pages.named(label) {
                entry.round_trip = round_trip;
                if let Ok(kept) = &kept {
                    entry.csp_violations = kept.csp.clone();
                    entry.errors = kept.errors.clone();
                }
            }
            match kept {
                Ok(kept) => super::log(&format!(
                    "self-test: {label} collected {} csp violations, {} errors",
                    kept.csp.len(),
                    kept.errors.len()
                )),
                Err(e) => self.fail(format!(
                    "{label}: what the page collected could not be read: {e}"
                )),
            }
        }
        self.close_panel();
    }

    fn opened(&self) -> Option<(isize, Placed)> {
        opened(self.app)
    }

    fn close_panel(&mut self) {
        if !close_panel(self.app) {
            self.fail("agentnotch-panel: the panel did not close".into());
        }
    }
}

/// Waits for the panel asked for just now: its window, and where it was put.
pub(super) fn opened(app: &AppHandle) -> Option<(isize, Placed)> {
    let look = || {
        let open = panel::status().is_some_and(|s| s.open);
        let handle = app
            .get_webview_window(PANEL)
            .and_then(|page| panel_window::hwnd_of(&page))?;
        let placed = panel_window::placed()?;
        (open && window::styles(handle).is_some_and(|s| s.visible)).then_some((handle, placed))
    };
    if !wait_for(WINDOW_BUILDS, || look().is_some()) {
        return None;
    }
    std::thread::sleep(LOOKS_ARE_OVER);
    // Again: the frame the panel settled on, not the first one it had.
    look()
}

/// Closes the panel and waits until it is gone; whether it went.
pub(super) fn close_panel(app: &AppHandle) -> bool {
    panel::close(app);
    let closed = wait_for(CLOSES, || {
        !panel::status().is_some_and(|s| s.open) && panel_window::placed().is_none()
    });
    std::thread::sleep(CLOSE_SETTLES);
    closed
}

// ---- upstream's switches ----

pub(super) fn read_found(app: &AppHandle) -> Found {
    let state = app.state::<crate::AppState>();
    let config = state.cfg.lock().unwrap_or_else(|e| e.into_inner());
    Found {
        edge: crate::config::edge_or_right(&config.notch_edge),
        notch_visible: config.notch_visible,
        tray_visible: config.tray_visible,
        notch_on_hover: config.notch_on_hover,
    }
}

/// Runs `work` where upstream's own commands run, and waits for it.
fn on_main(app: &AppHandle, work: impl FnOnce(AppHandle) + Send + 'static) -> Result<(), String> {
    let (done, wait) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        work(handle);
        let _ = done.send(());
    })
    .map_err(|e| e.to_string())?;
    wait.recv_timeout(MAIN_THREAD_ANSWERS)
        .map_err(|_| "the main thread did not get to it".to_string())
}

/// Waits until upstream has placed the notch again (it says `notch_edge` each time) and the
/// page has laid itself out.
fn notch_placed(since: u64) -> Result<(), String> {
    let placed = wait_for(NOTCH_MOVES, || {
        NOTCH_EDGE_EVENTS.load(Ordering::Relaxed) > since
    });
    std::thread::sleep(EDGE_SETTLES);
    if placed {
        Ok(())
    } else {
        Err("upstream never said notch_edge".into())
    }
}

/// Moves the notch through upstream's own command, which saves the edge and places the notch.
pub(super) fn move_notch(app: &AppHandle, edge: &str) -> Result<(), String> {
    let since = NOTCH_EDGE_EVENTS.load(Ordering::Relaxed);
    let to = edge.to_string();
    on_main(app, move |app| {
        let _ = crate::set_notch_edge(app, to);
    })?;
    notch_placed(since)
}

/// Shows or hides the notch through upstream's own switch (Settings › Show), leaving the other
/// two flags as they were found.
pub(super) fn show_notch(app: &AppHandle, visible: bool, found: &Found) -> Result<(), String> {
    let since = NOTCH_EDGE_EVENTS.load(Ordering::Relaxed);
    let (tray, on_hover) = (found.tray_visible, found.notch_on_hover);
    on_main(app, move |app| {
        let _ = crate::set_ui_flags(app, visible, tray, Some(on_hover));
    })?;
    if visible {
        return notch_placed(since);
    }
    let hidden = wait_for(CLOSES, || {
        !app.get_webview_window(NOTCH)
            .is_some_and(|notch| notch.is_visible().unwrap_or(false))
    });
    std::thread::sleep(EDGE_SETTLES);
    if hidden {
        Ok(())
    } else {
        Err("the notch window is still on screen".into())
    }
}

// ---- reading windows and pages ----

pub(super) fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

fn rect(r: PxRect) -> Rect {
    [r.x, r.y, r.w, r.h]
}

/// The panel's card where the window really is, and a line when the window isn't where its
/// placement put it. The card is the window less its tail strip.
fn card_on_screen(handle: isize, placed: &Placed) -> (Rect, Option<String>) {
    match window::window_rect(handle) {
        Some(frame) => card_in(
            [
                frame.left,
                frame.top,
                frame.right - frame.left,
                frame.bottom - frame.top,
            ],
            placed.placement.window,
            placed.placement.card,
        ),
        None => (
            rect(placed.placement.card),
            Some("the panel window's frame could not be read".into()),
        ),
    }
}

/// The card inside a window whose frame is `frame`, when the placement wanted the window at
/// `wanted` with the card at `card`.
fn card_in(frame: Rect, wanted: PxRect, card: PxRect) -> (Rect, Option<String>) {
    let on_screen = [
        frame[0] + (card.x - wanted.x),
        frame[1] + (card.y - wanted.y),
        card.w,
        card.h,
    ];
    let mismatch = (frame != rect(wanted)).then(|| {
        format!(
            "the panel window is at {frame:?}, its placement says {:?}",
            rect(wanted)
        )
    });
    (on_screen, mismatch)
}

/// The work area of the monitor the panel is on, as Windows says now; what it was placed by
/// when Windows can't say.
fn work_area(handle: isize, placed: &Placed) -> Rect {
    match window::monitor_of(handle) {
        Some(area) => [
            area.work.left,
            area.work.top,
            area.work.right - area.work.left,
            area.work.bottom - area.work.top,
        ],
        None => rect(placed.anchor.area.work),
    }
}

fn edge_word(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Right => "right",
        PanelEdge::Left => "left",
        PanelEdge::Top => "top",
        PanelEdge::Bottom => "bottom",
    }
}

fn describe(edge: Option<&str>) -> String {
    match edge {
        Some(edge) => format!("hang off the {edge} edge"),
        None => "float".into(),
    }
}

/// Calls the page's `selfTest({edge, width})` when it has one: its answers, or `None` for a
/// page without a hook.
fn page_hook(
    page: &WebviewWindow,
    global: &str,
    edge: &str,
    width: f64,
) -> Result<Option<BTreeMap<String, bool>>, String> {
    let answer = webview::eval_async(page, &hook_call(global, edge, width), PAGE_ANSWERS)?;
    if answer.is_null() {
        return Ok(None);
    }
    report::hook_invariants(&answer)
        .map(Some)
        .ok_or_else(|| "it answered something that is no object".to_string())
}

/// The expression that calls a page's hook. `global` is one of [`HOOKS`]' names; the arguments
/// go in as JSON.
fn hook_call(global: &str, edge: &str, width: f64) -> String {
    let arguments = json!({ "edge": edge, "width": width });
    format!(
        "(function () {{ var o = window.{global}; \
         if (!o || typeof o.selfTest !== \"function\") {{ return null; }} \
         return o.selfTest({arguments}); }})()"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentnotch::selftest_report::EDGES;

    #[test]
    fn the_hook_is_called_with_the_edge_and_the_width_as_json() {
        assert_eq!(
            hook_call("agentnotchPanel", "top", 520.0),
            "(function () { var o = window.agentnotchPanel; \
             if (!o || typeof o.selfTest !== \"function\") { return null; } \
             return o.selfTest({\"edge\":\"top\",\"width\":520.0}); })()"
        );
        // Whatever the edge's text, it arrives as a string and nothing else.
        let odd = hook_call("agentnotch", "\"); alert(1); (\"", 0.0);
        assert!(odd.contains(r#"{"edge":"\"); alert(1); (\"","width":0.0}"#));
        // Each page has one hook object, under the names the UI package uses.
        assert_eq!(
            HOOKS,
            [
                ("notch", "agentnotch"),
                ("settings", "agentnotchSettings"),
                ("agentnotch-panel", "agentnotchPanel"),
            ]
        );
    }

    #[test]
    fn the_card_is_read_from_where_the_window_really_is() {
        // A panel left of a right notch: the tail strip is the window's last 32 px.
        let wanted = PxRect::new(560, 24, 432, 680);
        let card = PxRect::new(560, 24, 400, 680);
        assert_eq!(
            card_in([560, 24, 432, 680], wanted, card),
            ([560, 24, 400, 680], None)
        );
        // Under a top notch the strip is above the card.
        let wanted = PxRect::new(252, 12, 520, 712);
        let card = PxRect::new(252, 44, 520, 680);
        assert_eq!(
            card_in([252, 12, 520, 712], wanted, card),
            ([252, 44, 520, 680], None)
        );
        // A window Windows put somewhere else: the card goes with it, and it is said.
        let (moved, mismatch) = card_in([300, 60, 520, 712], wanted, card);
        assert_eq!(moved, [300, 92, 520, 680]);
        assert!(mismatch.is_some_and(|line| line.contains("[300, 60, 520, 712]")));
        assert!(!report::inside(moved, [0, 0, 1024, 728]));
    }

    #[test]
    fn the_waits_come_after_the_panels_own_looks_and_the_run_has_its_deadline() {
        // The simulated confirmation must come after the rules' own looks at the foreground
        // (50 and 250 ms after an open), and the next open after a closed panel's late look.
        assert!(LOOKS_ARE_OVER > Duration::from_millis(250));
        assert!(CLOSE_SETTLES > Duration::from_millis(240));
        assert_eq!(NOTCH_SETTLES, Duration::from_secs(20));
        assert_eq!(DEADLINE, Duration::from_secs(120));
        // The words the report and the panel's placement use for an edge are the same.
        let words = [
            PanelEdge::Right,
            PanelEdge::Left,
            PanelEdge::Top,
            PanelEdge::Bottom,
        ]
        .map(edge_word);
        assert_eq!(words, EDGES[..4]);
        assert_eq!(describe(Some("left")), "hang off the left edge");
        assert_eq!(describe(None), "float");
    }
}
