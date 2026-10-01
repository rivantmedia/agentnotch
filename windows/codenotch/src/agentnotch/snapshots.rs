//! The sealed snapshot run (DESIGN-WIN §7.4): `AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>` drives the
//! pages through the fixture hub's states, captures each as a PNG through WebView2's
//! `CapturePreview` and writes `<dir>/manifest.json`, `[{name, file, width, height, bytes}]`.
//! The process then exits 0, or 1 when a capture failed or was empty or the run took longer than
//! [`DEADLINE`]. Baselines and their comparison belong to the smoke test, not to this file.
//!
//! States with the fixture hub and any page: the notch on each edge (`notch-<edge>`), the panel's
//! list on each edge and floating (`panel-list-<edge>`, `panel-floating`), a chat
//! (`panel-chat`) and the settings window on its Claude Code tab (`settings-claude`). A page
//! that has `window.<page>.snapshotStates()` and `.showSnapshotState(name)` (the UI package's
//! contract, UI§10) is then driven through each state it lists, at the panel's list width on
//! the right edge, into `<page>-<name>`, with `<page>` one of `notch`, `panel`, `settings`.
//!
//! Sealed runs only (through `webview`, which refuses otherwise), and on a worker thread: see
//! `webview.rs`. Every state that can't be captured is a failure line in run.log; the others
//! still are.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, WebviewWindow};

use super::selftest::{self, Found};
use super::selftest_report::{EDGES, FLOATING, NOTCH, PANEL, SETTINGS};
use super::{panel, webview};

/// The whole run. Past it the watchdog ends the process with 1.
pub(super) const DEADLINE: Duration = Duration::from_secs(120);
/// The file that lists what was captured.
const MANIFEST: &str = "manifest.json";
/// The longest file name stem a page's state may make.
const STEM_LIMIT: usize = 80;
/// A page's `snapshotStates()` is believed up to this many states.
const STATES_LIMIT: usize = 64;
/// A window's first build, WebView2 included.
const WINDOW_BUILDS: Duration = Duration::from_secs(15);
/// After the two animation frames: the last layout and image decodes of the state.
const SETTLE: Duration = Duration::from_millis(150);
const FRAMES: Duration = Duration::from_secs(5);
const CAPTURE: Duration = Duration::from_secs(10);
const PAGE_ANSWERS: Duration = Duration::from_secs(10);
const QUICK: Duration = Duration::from_secs(5);

/// The states every run captures, in this order (the smoke test requires each of these files).
pub(super) const BASE_STATES: [&str; 11] = [
    "notch-right",
    "notch-left",
    "notch-top",
    "notch-bottom",
    "panel-list-right",
    "panel-list-left",
    "panel-list-top",
    "panel-list-bottom",
    "panel-floating",
    "panel-chat",
    "settings-claude",
];

/// Set when the run is over, so its watchdog leaves the process alone.
static DONE: AtomicBool = AtomicBool::new(false);

/// Two animation frames: what the page drew after the last change is on screen after them.
const TWO_FRAMES: &str = "new Promise(function (done) { \
requestAnimationFrame(function () { requestAnimationFrame(function () { done(true); }); }); })";

/// Selects the Claude Code tab the way a click on it does.
const CLAUDE_TAB: &str = r#"(function () {
  var tab = document.getElementById("tab-claude");
  if (!tab) { return false; }
  tab.click();
  var pane = document.getElementById("pane-claude");
  return !!pane && !pane.hidden;
})()"#;

const PAGE_LOADED: &str = r#"document.readyState === "complete""#;

/// The pages that may carry the snapshot hooks: window label, the page's global, the name
/// its states carry in file names.
const PAGES: [(&str, &str, &str); 3] = [
    (NOTCH, "agentnotch", "notch"),
    (PANEL, "agentnotchPanel", "panel"),
    (SETTINGS, "agentnotchSettings", "settings"),
];

// ---- what a run writes (pure) ----

/// One line of `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) file: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bytes: u64,
}

pub(super) fn manifest_json(entries: &[Entry]) -> String {
    serde_json::to_string_pretty(entries).unwrap_or_else(|_| "[]".into())
}

/// A file name stem from a state's name, however odd: lowercase letters, digits and single
/// dashes only, at most [`STEM_LIMIT`] long, never empty. No separator, dot or drive letter can
/// get through, so the file can only land in the snapshots folder.
pub(super) fn file_stem(name: &str) -> String {
    let mut stem = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            stem.push(c);
        } else if !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem = stem.trim_matches('-');
    let stem: String = stem.chars().take(STEM_LIMIT).collect();
    let stem = stem.trim_end_matches('-');
    if stem.is_empty() {
        "state".into()
    } else {
        stem.to_string()
    }
}

/// `stem`, or `stem-2`, `stem-3`, … when the stem is taken (two page states may sanitise to the
/// same one).
pub(super) fn unique_stem(stem: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(stem) {
        return stem.to_string();
    }
    (2..)
        .map(|n| format!("{stem}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| stem.to_string())
}

/// The states a page lists, from what `snapshotStates()` answered: its strings, none twice.
pub(super) fn states_from(answer: &Value) -> Vec<String> {
    let mut seen = BTreeSet::new();
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| !name.is_empty() && seen.insert(name.to_string()))
        .take(STATES_LIMIT)
        .map(str::to_string)
        .collect()
}

/// The expression that asks a page for its states: an array, or `null` without the hook.
fn states_call(global: &str) -> String {
    format!(
        "(function () {{ var o = window.{global}; \
         if (!o || typeof o.snapshotStates !== \"function\") {{ return null; }} \
         return o.snapshotStates(); }})()"
    )
}

/// The expression that puts a page into one of its states; `name` goes in as a JSON string.
fn show_call(global: &str, name: &str) -> String {
    let name = Value::String(name.to_string());
    format!(
        "(function () {{ var o = window.{global}; \
         if (!o || typeof o.showSnapshotState !== \"function\") {{ \
         throw new Error(\"no showSnapshotState\"); }} \
         return o.showSnapshotState({name}); }})()"
    )
}

/// Whether a PNG has an alpha channel (colour type 4 or 6), from its header; `None` when the
/// bytes are no PNG. Logged so the transparent pages' captures can be told from opaque ones.
fn png_has_alpha(bytes: &[u8]) -> Option<bool> {
    webview::png_size(bytes).ok()?;
    bytes.get(25).map(|kind| matches!(kind, 4 | 6))
}

// ---- the run ----

/// Captures every state and writes the manifest. The verdict: 0, or 1 when anything failed.
pub(super) fn run(app: &AppHandle, dir: &Path) -> i32 {
    DONE.store(false, Ordering::SeqCst);
    let _ = std::thread::Builder::new()
        .name("an-snapshots-watchdog".into())
        .spawn(|| {
            std::thread::sleep(DEADLINE);
            if !DONE.load(Ordering::SeqCst) {
                super::log(&format!(
                    "snapshots: FAILED: the run took longer than {} s",
                    DEADLINE.as_secs()
                ));
                // Whatever was captured is in the manifest already: it is rewritten after each.
                std::process::exit(1);
            }
        });
    let mut run = Run {
        app,
        dir: dir.to_path_buf(),
        entries: Vec::new(),
        taken: BTreeSet::new(),
        failures: Vec::new(),
        ring: None,
        session: None,
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        run.fail(format!("{}: {e}", dir.display()));
    } else {
        run.all();
        run.check_base();
    }
    DONE.store(true, Ordering::SeqCst);
    for line in &run.failures {
        super::log(&format!("snapshots: failed: {line}"));
    }
    let code = verdict(run.entries.len(), run.failures.len());
    super::log(&format!(
        "snapshots: {} captured, {} failed",
        run.entries.len(),
        run.failures.len()
    ));
    code
}

/// 0 only when something was captured and nothing failed.
fn verdict(captured: usize, failed: usize) -> i32 {
    i32::from(captured == 0 || failed > 0)
}

struct Run<'a> {
    app: &'a AppHandle,
    dir: PathBuf,
    entries: Vec<Entry>,
    taken: BTreeSet<String>,
    failures: Vec<String>,
    /// The first ring and the first session of the sealed hub.
    ring: Option<String>,
    session: Option<String>,
}

impl Run<'_> {
    fn fail(&mut self, line: String) {
        super::log(&format!("snapshots: {line}"));
        self.failures.push(line);
    }

    /// Every state of [`BASE_STATES`] is in the manifest: a run that skipped one has failed
    /// even when nothing it did try went wrong.
    fn check_base(&mut self) {
        for name in BASE_STATES {
            if !self.entries.iter().any(|entry| entry.name == name) {
                self.fail(format!("{name}: it was not captured"));
            }
        }
    }

    fn window(&self, label: &str) -> Option<WebviewWindow> {
        self.app.get_webview_window(label)
    }

    fn all(&mut self) {
        let found = self.settle();
        self.edges(&found);
        self.floating(&found);
        self.chat();
        self.settings();
        self.page_states();
    }

    /// The notch up and shown, and the fixture hub's first ring and session.
    fn settle(&mut self) -> Found {
        let app = self.app;
        let up = selftest::wait_for(WINDOW_BUILDS + Duration::from_secs(5), || {
            self.window(NOTCH).is_some_and(|notch| {
                notch.is_visible().unwrap_or(false)
                    && webview::eval(&notch, PAGE_LOADED, QUICK) == Ok(Value::Bool(true))
            })
        });
        if !up {
            self.fail("notch: the window and its page were not up".into());
        }
        let found = selftest::read_found(app);
        if let Some(hub) = super::hub() {
            let snapshot = hub.snapshot();
            self.ring = snapshot.rings.first().map(|ring| ring.ring_id.clone());
            self.session = snapshot.sessions.first().map(|s| s.session_id.clone());
        }
        if self.ring.is_none() {
            self.fail("the sealed hub has no ring to open the panel on".into());
        }
        if !found.notch_visible {
            self.show_notch(true, &found);
        }
        found
    }

    fn show_notch(&mut self, visible: bool, found: &Found) {
        if let Err(e) = selftest::show_notch(self.app, visible, found) {
            self.fail(format!("the notch could not be shown or hidden: {e}"));
        }
    }

    fn move_notch(&mut self, edge: &str) {
        if let Err(e) = selftest::move_notch(self.app, edge) {
            self.fail(format!("the notch did not move to the {edge} edge: {e}"));
        }
    }

    /// The panel on the first ring; false when it did not open.
    fn open_panel(&mut self, route: String, reason: &str, what: &str) -> bool {
        panel::open_route(self.app, route, self.ring.clone(), reason.into());
        let opened = selftest::opened(self.app).is_some();
        if !opened {
            self.fail(format!("{what}: the panel did not open"));
        }
        opened
    }

    fn close_panel(&mut self) {
        if !selftest::close_panel(self.app) {
            self.fail("the panel did not close".into());
        }
    }

    /// The notch and the panel's list on each edge.
    fn edges(&mut self, found: &Found) {
        for edge in EDGES.into_iter().filter(|edge| *edge != FLOATING) {
            self.move_notch(edge);
            let notch = self.window(NOTCH);
            self.shoot(&format!("notch-{edge}"), notch);
            let name = format!("panel-list-{edge}");
            if self.open_panel("sessions".into(), "ring_click", &name) {
                let panel = self.window(PANEL);
                self.shoot(&name, panel);
            }
            self.close_panel();
        }
        // The notch goes back to where it was found, and the later states are read from there.
        self.move_notch(&found.edge);
    }

    /// The panel with no notch to hang off.
    fn floating(&mut self, found: &Found) {
        self.show_notch(false, found);
        if self.open_panel("sessions".into(), "ring_click", "panel-floating") {
            let panel = self.window(PANEL);
            self.shoot("panel-floating", panel);
        }
        self.close_panel();
        self.show_notch(true, found);
    }

    fn chat(&mut self) {
        let Some(session) = self.session.clone() else {
            return self.fail("the sealed hub has no session to open a chat on".into());
        };
        if self.open_panel(format!("session:{session}"), "hover_row", "panel-chat") {
            let panel = self.window(PANEL);
            self.shoot("panel-chat", panel);
        }
        self.close_panel();
    }

    fn settings(&mut self) {
        let app = self.app;
        crate::settings_window::open(app);
        if !selftest::wait_for(WINDOW_BUILDS, || self.window(SETTINGS).is_some()) {
            return self.fail("settings-claude: the window never came up".into());
        }
        let Some(page) = self.window(SETTINGS) else {
            return;
        };
        let loaded = selftest::wait_for(WINDOW_BUILDS, || {
            page.is_visible().unwrap_or(false)
                && webview::eval(&page, PAGE_LOADED, QUICK) == Ok(Value::Bool(true))
        });
        if !loaded {
            return self.fail("settings-claude: the page did not load".into());
        }
        // The click is the page's own tab switch: nothing else selects the tab.
        if webview::eval(&page, CLAUDE_TAB, QUICK) != Ok(Value::Bool(true)) {
            return self.fail("settings-claude: the Claude Code tab could not be selected".into());
        }
        self.shoot("settings-claude", Some(page));
    }

    /// Each page's own states, when it lists any.
    fn page_states(&mut self) {
        // Everything back to the right edge, where the panel is at its list width.
        self.move_notch("right");
        for (label, global, short) in PAGES {
            let Some(page) = self.window(label) else {
                self.fail(format!("{label}: no window to look into"));
                continue;
            };
            let listed = webview::eval_async(&page, &states_call(global), PAGE_ANSWERS);
            let states = match &listed {
                Ok(answer) => states_from(answer),
                Err(e) => {
                    self.fail(format!("{label}: snapshotStates failed: {e}"));
                    continue;
                }
            };
            if states.is_empty() {
                continue;
            }
            super::log(&format!("snapshots: {label} lists {} states", states.len()));
            for state in states {
                self.page_state(label, global, short, &state);
            }
            if label == PANEL {
                self.close_panel();
            }
        }
    }

    fn page_state(&mut self, label: &str, global: &str, short: &str, state: &str) {
        let name = format!("{short}-{state}");
        // The panel closes by itself now and then (a click elsewhere): it is opened again.
        let closed = label == PANEL && !panel::status().is_some_and(|s| s.open);
        if closed && !self.open_panel("sessions".into(), "ring_click", &name) {
            return;
        }
        let Some(page) = self.window(label) else {
            return self.fail(format!("{label}: no window to look into"));
        };
        if let Err(e) = webview::eval_async(&page, &show_call(global, state), PAGE_ANSWERS) {
            return self.fail(format!("{name}: the page could not show it: {e}"));
        }
        self.shoot(&name, Some(page));
    }

    /// Captures `window` as `<dir>/<stem of name>.png` once the page has drawn, and adds it to
    /// the manifest.
    fn shoot(&mut self, name: &str, window: Option<WebviewWindow>) {
        let Some(window) = window else {
            return self.fail(format!("{name}: no window to capture"));
        };
        match self.capture(name, &window) {
            Ok(entry) => {
                super::log(&format!(
                    "snapshots: {} {}x{} {} bytes",
                    entry.file, entry.width, entry.height, entry.bytes
                ));
                self.entries.push(entry);
                self.write_manifest();
            }
            Err(e) => self.fail(format!("{name}: {e}")),
        }
    }

    fn capture(&mut self, name: &str, window: &WebviewWindow) -> Result<Entry, String> {
        webview::eval_async(window, TWO_FRAMES, FRAMES)
            .map_err(|e| format!("the page drew no frames: {e}"))?;
        std::thread::sleep(SETTLE);
        let png = webview::capture_png(window, CAPTURE)?;
        let (width, height) = webview::png_size(&png)?;
        let stem = unique_stem(&file_stem(name), &self.taken);
        let file = format!("{stem}.png");
        std::fs::write(self.dir.join(&file), &png).map_err(|e| format!("{file}: {e}"))?;
        super::log(&format!(
            "snapshots: {file}: alpha channel {:?}",
            png_has_alpha(&png)
        ));
        self.taken.insert(stem);
        Ok(Entry {
            name: name.to_string(),
            file,
            width,
            height,
            bytes: png.len() as u64,
        })
    }

    fn write_manifest(&mut self) {
        let path = self.dir.join(MANIFEST);
        if let Err(e) = std::fs::write(&path, manifest_json(&self.entries)) {
            self.fail(format!("{}: {e}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn safe(file: &str) -> bool {
        !file.is_empty()
            && file
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn the_base_states_are_eleven_distinct_safe_names() {
        assert_eq!(BASE_STATES.len(), 11);
        let distinct: BTreeSet<_> = BASE_STATES.into_iter().collect();
        assert_eq!(distinct.len(), 11);
        for name in BASE_STATES {
            assert!(safe(name), "{name}");
            assert_eq!(file_stem(name), name, "a base name is its own file stem");
        }
        // The edges are the report's, in its order.
        let edges = EDGES.into_iter().filter(|edge| *edge != FLOATING);
        let notch: Vec<String> = edges.clone().map(|edge| format!("notch-{edge}")).collect();
        let list: Vec<String> = edges.map(|edge| format!("panel-list-{edge}")).collect();
        assert_eq!(notch, BASE_STATES[..4]);
        assert_eq!(list, BASE_STATES[4..8]);
        assert_eq!(
            BASE_STATES[8..],
            ["panel-floating", "panel-chat", "settings-claude"]
        );
    }

    #[test]
    fn a_page_states_name_becomes_a_file_name_that_stays_in_the_folder() {
        assert_eq!(file_stem("../x"), "x");
        assert_eq!(file_stem("a/b"), "a-b");
        assert_eq!(file_stem("a\\b"), "a-b");
        assert_eq!(file_stem("C:\\Windows\\x"), "c-windows-x");
        assert_eq!(file_stem(".."), "state");
        assert_eq!(file_stem(""), "state");
        assert_eq!(file_stem("  "), "state");
        assert_eq!(file_stem("Every State!"), "every-state");
        assert_eq!(file_stem("panel__needs you"), "panel-needs-you");
        assert_eq!(file_stem("a\0b"), "a-b");
        assert_eq!(file_stem("é"), "state");
        let long = "x".repeat(300);
        assert_eq!(file_stem(&long).len(), STEM_LIMIT);
        let dashed = format!("{}-{}", "y".repeat(STEM_LIMIT - 1), "z".repeat(50));
        assert!(!file_stem(&dashed).ends_with('-'));
        for name in [
            "../x",
            "a/b",
            "a\\b",
            "",
            long.as_str(),
            "..\\..\\evil",
            "/etc/passwd",
        ] {
            let stem = file_stem(name);
            assert!(safe(&stem), "{name:?} -> {stem:?}");
            let file = format!("{stem}.png");
            assert!(!file.contains(['/', '\\', ':']));
            assert_eq!(Path::new(&file).components().count(), 1);
        }
    }

    #[test]
    fn two_states_with_the_same_stem_get_two_files() {
        let mut taken = BTreeSet::new();
        for expected in ["a-b", "a-b-2", "a-b-3"] {
            let stem = unique_stem(&file_stem("a/b"), &taken);
            assert_eq!(stem, expected);
            taken.insert(stem);
        }
    }

    #[test]
    fn the_manifest_is_a_list_of_name_file_width_height_bytes() {
        let entries = vec![Entry {
            name: "notch-right".into(),
            file: "notch-right.png".into(),
            width: 320,
            height: 96,
            bytes: 1234,
        }];
        let text = manifest_json(&entries);
        let read: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            read,
            json!([{
                "name": "notch-right",
                "file": "notch-right.png",
                "width": 320,
                "height": 96,
                "bytes": 1234
            }])
        );
        assert_eq!(manifest_json(&[]), "[]");
    }

    #[test]
    fn a_pages_states_are_its_strings_once_each() {
        assert_eq!(
            states_from(&json!(["a", "b", "a", 3, null, "", "c"])),
            ["a", "b", "c"]
        );
        assert!(states_from(&Value::Null).is_empty());
        assert!(states_from(&json!({"a": 1})).is_empty());
        let many: Vec<String> = (0..200).map(|n| format!("s{n}")).collect();
        assert_eq!(states_from(&json!(many)).len(), STATES_LIMIT);
    }

    #[test]
    fn the_hook_expressions_carry_the_state_as_a_json_string() {
        assert_eq!(
            states_call("agentnotchPanel"),
            "(function () { var o = window.agentnotchPanel; \
             if (!o || typeof o.snapshotStates !== \"function\") { return null; } \
             return o.snapshotStates(); })()"
        );
        let call = show_call("agentnotch", "a\"); alert(1); (\"");
        assert!(call.contains(r#"o.showSnapshotState("a\"); alert(1); (\"");"#));
        // The pages are the three the self-test looks into, under their hook globals.
        assert_eq!(
            PAGES.map(|(label, global, _)| (label, global)),
            [
                ("notch", "agentnotch"),
                ("agentnotch-panel", "agentnotchPanel"),
                ("settings", "agentnotchSettings"),
            ]
        );
    }

    #[test]
    fn a_run_passes_only_with_something_captured_and_nothing_failed() {
        assert_eq!(verdict(11, 0), 0);
        assert_eq!(verdict(11, 1), 1);
        assert_eq!(verdict(0, 0), 1);
        assert_eq!(verdict(0, 3), 1);
        assert_eq!(DEADLINE, Duration::from_secs(120));
    }

    #[test]
    fn the_alpha_channel_is_read_from_the_png_header() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend([0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0]);
        assert_eq!(png_has_alpha(&png), Some(true));
        png[25] = 2;
        assert_eq!(png_has_alpha(&png), Some(false));
        assert_eq!(png_has_alpha(b"not a png"), None);
    }
}
