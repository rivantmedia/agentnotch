//! What the sealed self-test writes down and how it judges it (DESIGN-WIN §7.4): the report's
//! types, the checks that need no window, the launch switches and the list of steps.
//!
//! The report's field names are an interface: the smoke script asserts on them. Everything here
//! is plain data, so it is tested on every OS; `selftest.rs` fills it in from the real windows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// The notch's edges in the order the self-test visits them, then the panel with no notch to
/// hang off.
pub(super) const EDGES: [&str; 5] = ["right", "left", "top", "bottom", FLOATING];
pub(super) const FLOATING: &str = "floating";

/// The pages the self-test looks into, by window label.
pub(super) const NOTCH: &str = "notch";
pub(super) const SETTINGS: &str = "settings";
pub(super) const PANEL: &str = "agentnotch-panel";

/// `[x, y, w, h]` in physical screen pixels.
pub(super) type Rect = [i32; 4];

// ---- the launch switches ----

/// What a launch was asked to do through the sealed-only variables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Switches {
    /// `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH`.
    pub(super) open_route: Option<String>,
    /// `AGENTNOTCH_PANEL_SELF_TEST`, set and not `0`.
    pub(super) self_test: bool,
    /// `AGENTNOTCH_SNAPSHOT_CLAUDE`.
    pub(super) snapshots: Option<PathBuf>,
    /// `AGENTNOTCH_SELF_TEST_OUT`: where the report goes. Without it the run still happens and
    /// still exits with its verdict.
    pub(super) out: Option<PathBuf>,
    /// `AGENTNOTCH_SELF_TEST_SCALE`, as it was written: the scale the self-test's pages are
    /// to be drawn at ([`wanted_scale`] reads it). Without it they keep their monitor's.
    pub(super) scale: Option<String>,
}

/// Reads the switches. Only a sealed run has any: a live app started from a shell that still
/// exports them starts normally instead of driving its windows and quitting.
pub(super) fn switches(sealed: bool, env: impl Fn(&str) -> Option<String>) -> Switches {
    if !sealed {
        return Switches::default();
    }
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    Switches {
        open_route: set("AGENTNOTCH_OPEN_PANEL_ON_LAUNCH"),
        self_test: set("AGENTNOTCH_PANEL_SELF_TEST").is_some_and(|value| value != "0"),
        snapshots: set("AGENTNOTCH_SNAPSHOT_CLAUDE").map(PathBuf::from),
        out: set("AGENTNOTCH_SELF_TEST_OUT").map(PathBuf::from),
        scale: set("AGENTNOTCH_SELF_TEST_SCALE"),
    }
}

/// The scale a run asked for. One that can't be read is an error (the run fails) rather than
/// a run at the monitor's scale that would pass for the one asked for.
pub(super) fn wanted_scale(text: &str) -> Result<f64, String> {
    match text.trim().parse::<f64>() {
        Ok(scale) if scale.is_finite() && (1.0..=5.0).contains(&scale) => Ok(scale),
        _ => Err(format!(
            "AGENTNOTCH_SELF_TEST_SCALE is '{text}': not a scale between 1 and 5"
        )),
    }
}

/// Whether a page that says it draws at `drawn` draws at the scale asked for. A page's
/// `devicePixelRatio` is the scale exactly, give or take how a float is printed.
pub(super) fn scale_is(wanted: f64, drawn: f64) -> bool {
    (wanted - drawn).abs() <= 0.01
}

// ---- the steps ----

/// One stretch of the self-test. The watchdog names the one a run was in when it ran out of
/// time, which on a CI runner is all there is to go by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// Waiting for the notch window and its page.
    Settle,
    /// The collectors go into the three pages (each is reloaded).
    Collectors,
    /// The panel on one of [`EDGES`].
    Edge(&'static str),
    /// The panel at its narrowest (the list beside a side notch) and widest (a chat on a flat
    /// edge).
    Widths,
    /// Upstream's edge and visibility go back to what they were.
    Restore,
    /// `an_call('snapshot')` from each page, each page's invariants and what it collected.
    Pages,
    Report,
}

impl Step {
    pub(super) fn name(self) -> String {
        match self {
            Step::Settle => "waiting for the notch".into(),
            Step::Collectors => "installing the collectors".into(),
            Step::Edge(edge) => format!("edge {edge}"),
            Step::Widths => "panel widths".into(),
            Step::Restore => "restoring the notch".into(),
            Step::Pages => "page checks".into(),
            Step::Report => "writing the report".into(),
        }
    }
}

/// The whole run, in order.
pub(super) fn steps() -> Vec<Step> {
    let mut steps = vec![Step::Settle, Step::Collectors];
    steps.extend(EDGES.map(Step::Edge));
    steps.extend([Step::Widths, Step::Restore, Step::Pages, Step::Report]);
    steps
}

/// What the watchdog reports.
pub(super) fn timed_out_at(step: Step) -> String {
    format!("timed out at {}", step.name())
}

// ---- the report ----

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct Report {
    /// True only when no list of failures in the report has a line in it.
    pub(super) ok: bool,
    pub(super) version: String,
    /// The panel page's `devicePixelRatio`; null when no page could say.
    pub(super) scale: Option<f64>,
    /// Why the run could not finish; null when it did.
    pub(super) error: Option<String>,
    pub(super) edges: Vec<EdgeReport>,
    pub(super) pages: Pages,
    /// One human-readable line per failed check, the edges' and the pages' included.
    pub(super) failures: Vec<String>,
    /// Failures that belong to no edge and no page's own fields (a window that never came up).
    #[serde(skip)]
    notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub(super) struct Pages {
    pub(super) notch: PageReport,
    pub(super) settings: PageReport,
    #[serde(rename = "agentnotch-panel")]
    pub(super) panel: PageReport,
}

impl Pages {
    pub(super) fn named(&mut self, label: &str) -> Option<&mut PageReport> {
        match label {
            NOTCH => Some(&mut self.notch),
            SETTINGS => Some(&mut self.settings),
            PANEL => Some(&mut self.panel),
            _ => None,
        }
    }

    fn all(&self) -> [(&'static str, &PageReport); 3] {
        [
            (NOTCH, &self.notch),
            (SETTINGS, &self.settings),
            (PANEL, &self.panel),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub(super) struct PageReport {
    /// The page's own `invoke('an_call', {method: 'snapshot'})` came back with rings.
    pub(super) round_trip: bool,
    pub(super) csp_violations: Vec<Value>,
    pub(super) errors: Vec<Value>,
    pub(super) invariants: BTreeMap<String, bool>,
    /// Whether the page has a `selfTest` hook of its own (its answers are among `invariants`).
    pub(super) page_hook: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub(super) struct EdgeReport {
    pub(super) edge: String,
    pub(super) floating: bool,
    /// The panel's card (what the user sees of the window: the tail strip beside it is
    /// see-through and reaches to the ring, which may sit over a taskbar).
    pub(super) panel: Rect,
    pub(super) work_area: Rect,
    pub(super) inside_work_area: bool,
    pub(super) tail_offset: f64,
    pub(super) tail_limit: f64,
    pub(super) tail_inside_corners: bool,
    pub(super) topmost: bool,
    pub(super) above_notch: bool,
    pub(super) auto: AutoReport,
    pub(super) failures: Vec<String>,
}

/// The panel opened by itself (reason `auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub(super) struct AutoReport {
    /// `WS_EX_NOACTIVATE` is set.
    pub(super) no_activate: bool,
    /// Nobody was told the panel has the keyboard: not the rules, not the page.
    pub(super) gate_shut: bool,
    /// After the simulated confirmation both were.
    pub(super) gate_opens_on_confirmation: bool,
}

impl Report {
    pub(super) fn new(version: &str) -> Self {
        Report {
            ok: false,
            version: version.to_string(),
            scale: None,
            error: None,
            edges: Vec::new(),
            pages: Pages::default(),
            failures: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A failed check that is no edge's and shows in no page's fields.
    pub(super) fn fail(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    /// Draws the verdict from everything written down so far. Safe to call again after more
    /// was added.
    pub(super) fn seal(&mut self) {
        let mut failures = Vec::new();
        failures.extend(self.error.iter().cloned());
        failures.extend(self.notes.iter().cloned());
        for edge in &self.edges {
            failures.extend(
                edge.failures
                    .iter()
                    .map(|line| format!("edge {}: {line}", edge.edge)),
            );
        }
        for (label, page) in self.pages.all() {
            failures.extend(page_failures(label, page));
        }
        self.ok = failures.is_empty();
        self.failures = failures;
    }

    /// The report of a run that did not finish: what it had, and why it stopped.
    pub(super) fn stopped(mut self, error: String) -> Self {
        self.error = Some(error);
        self.seal();
        self
    }
}

impl EdgeReport {
    pub(super) fn new(edge: &str) -> Self {
        EdgeReport {
            edge: edge.to_string(),
            floating: edge == FLOATING,
            ..EdgeReport::default()
        }
    }

    /// Judges what was read from the windows: one line per check that failed.
    pub(super) fn check(&mut self) {
        self.inside_work_area = inside(self.panel, self.work_area);
        self.tail_inside_corners = tail_inside_corners(self.tail_offset, self.tail_limit);
        let checks = [
            (
                self.inside_work_area,
                format!(
                    "the panel {} is not inside the work area {}",
                    words(self.panel),
                    words(self.work_area)
                ),
            ),
            (
                self.tail_inside_corners,
                format!(
                    "the tail offset {} is outside the card's corners (limit {})",
                    self.tail_offset, self.tail_limit
                ),
            ),
            (self.topmost, "the panel is not topmost".to_string()),
            (
                self.above_notch,
                "the panel is not above the notch".to_string(),
            ),
            (
                self.auto.no_activate,
                "the panel that opened by itself can be activated (no WS_EX_NOACTIVATE)"
                    .to_string(),
            ),
            (
                self.auto.gate_shut,
                "the keyboard gate is open after the panel opened by itself".to_string(),
            ),
            (
                self.auto.gate_opens_on_confirmation,
                "the keyboard gate stayed shut after the confirmation".to_string(),
            ),
        ];
        for (passed, line) in checks {
            if !passed {
                self.failures.push(line);
            }
        }
    }
}

/// 0 for a run with nothing to report, 1 otherwise; the same with or without a report file.
pub(super) fn exit_code(report: &Report) -> i32 {
    if report.ok {
        0
    } else {
        1
    }
}

/// Writes the report where the run was told to; nowhere when it was told nothing. Through a
/// temporary file beside it, so whoever waits for the report never reads half of one.
pub(super) fn write(out: Option<&Path>, report: &Report) -> Result<(), String> {
    let Some(out) = out else {
        return Ok(());
    };
    let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let temporary = out.with_file_name(name);
    std::fs::write(&temporary, text).map_err(|e| format!("{}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, out).map_err(|e| format!("{}: {e}", out.display()))
}

/// The one line a run leaves in run.log.
pub(super) fn summary(report: &Report) -> String {
    match (report.ok, report.failures.first()) {
        (true, _) => format!("self-test passed ({} edges)", report.edges.len()),
        (false, Some(first)) => format!(
            "self-test FAILED: {} failures, the first: {first}",
            report.failures.len()
        ),
        (false, None) => "self-test FAILED".into(),
    }
}

// ---- the checks ----

/// Whether `rect` lies inside `area`; touching its sides is inside.
pub(super) fn inside(rect: Rect, area: Rect) -> bool {
    let [x, y, w, h] = rect.map(i64::from);
    let [ax, ay, aw, ah] = area.map(i64::from);
    w > 0 && h > 0 && x >= ax && y >= ay && x + w <= ax + aw && y + h <= ay + ah
}

/// Whether the tail sits on the card's straight side, clear of its rounded corners.
pub(super) fn tail_inside_corners(offset: f64, limit: f64) -> bool {
    offset.is_finite() && limit.is_finite() && offset.abs() <= limit
}

/// One line for each thing wrong with a page, each naming the page.
pub(super) fn page_failures(label: &str, page: &PageReport) -> Vec<String> {
    let mut lines = Vec::new();
    if !page.round_trip {
        lines.push(format!(
            "{label}: an_call('snapshot') did not come back with rings"
        ));
    }
    for (name, holds) in &page.invariants {
        if !holds {
            lines.push(format!("{label}: the invariant {name} does not hold"));
        }
    }
    let text = |entry: &Value, field: &str| {
        entry
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    };
    for violation in &page.csp_violations {
        lines.push(format!(
            "{label}: the content security policy blocked {} ({})",
            text(violation, "blockedURI"),
            text(violation, "violatedDirective")
        ));
    }
    for error in &page.errors {
        lines.push(format!(
            "{label}: {}: {}",
            text(error, "kind"),
            text(error, "message")
        ));
    }
    lines
}

/// What a page's `selfTest` hook answered, as invariants: only `true` holds.
pub(super) fn hook_invariants(answer: &Value) -> Option<BTreeMap<String, bool>> {
    let answers = answer.as_object()?;
    Some(
        answers
            .iter()
            .map(|(name, holds)| (name.clone(), holds == &Value::Bool(true)))
            .collect(),
    )
}

/// Whether an `an_call('snapshot')` answer is a snapshot: it has the rings, as a list.
pub(super) fn is_snapshot(answer: &Value) -> bool {
    answer.get("rings").is_some_and(Value::is_array)
}

fn words(rect: Rect) -> String {
    let [x, y, w, h] = rect;
    format!("({x},{y} {w}x{h})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn good_edge(edge: &str) -> EdgeReport {
        let mut report = EdgeReport::new(edge);
        report.panel = [584, 44, 400, 680];
        report.work_area = [0, 0, 1024, 728];
        report.tail_offset = -12.5;
        report.tail_limit = 306.0;
        report.topmost = true;
        report.above_notch = true;
        report.auto = AutoReport {
            no_activate: true,
            gate_shut: true,
            gate_opens_on_confirmation: true,
        };
        report.check();
        report
    }

    fn good_page() -> PageReport {
        PageReport {
            round_trip: true,
            invariants: BTreeMap::from([("no_text_overflow".to_string(), true)]),
            ..PageReport::default()
        }
    }

    fn good_report() -> Report {
        let mut report = Report::new("1.1.0");
        report.scale = Some(1.25);
        report.edges.push(good_edge("right"));
        report.pages = Pages {
            notch: good_page(),
            settings: good_page(),
            panel: good_page(),
        };
        report.seal();
        report
    }

    #[test]
    fn the_report_has_the_shape_the_smoke_script_reads() {
        let page = json!({
            "round_trip": true, "csp_violations": [], "errors": [],
            "invariants": { "no_text_overflow": true }, "page_hook": false,
        });
        assert_eq!(
            serde_json::to_value(good_report()).expect("a report"),
            json!({
                "ok": true, "version": "1.1.0", "scale": 1.25, "error": null,
                "edges": [ {
                    "edge": "right", "floating": false,
                    "panel": [584, 44, 400, 680], "work_area": [0, 0, 1024, 728],
                    "inside_work_area": true,
                    "tail_offset": -12.5, "tail_limit": 306.0, "tail_inside_corners": true,
                    "topmost": true, "above_notch": true,
                    "auto": { "no_activate": true, "gate_shut": true,
                              "gate_opens_on_confirmation": true },
                    "failures": [],
                } ],
                "pages": { "notch": page, "settings": page, "agentnotch-panel": page },
                "failures": [],
            })
        );
        // A run that knows nothing yet: the same fields, and not ok.
        let mut empty = Report::new("1.1.0");
        empty.seal();
        let empty = serde_json::to_value(empty).expect("a report");
        assert_eq!(empty["ok"], false);
        assert_eq!(empty["scale"], Value::Null);
        assert_eq!(empty["edges"], json!([]));
        assert_eq!(empty["pages"]["agentnotch-panel"]["round_trip"], false);
        // The floating case says so.
        assert!(EdgeReport::new("floating").floating);
        assert!(!EdgeReport::new("top").floating);
    }

    #[test]
    fn ok_is_false_whenever_any_list_of_failures_has_a_line() {
        assert!(good_report().ok);
        assert_eq!(exit_code(&good_report()), 0);

        // An edge's own line.
        let mut report = good_report();
        report.edges[0]
            .failures
            .push("the panel did not open".into());
        report.seal();
        assert!(!report.ok);
        assert_eq!(report.failures, ["edge right: the panel did not open"]);
        assert_eq!(exit_code(&report), 1);

        // A line of the run's own.
        let mut report = good_report();
        report.fail("settings: the window never came up");
        report.seal();
        assert!(!report.ok);
        assert_eq!(report.failures, ["settings: the window never came up"]);
        // Sealing again counts nothing twice.
        report.seal();
        assert_eq!(report.failures.len(), 1);

        // A page's.
        let mut report = good_report();
        report.pages.settings.round_trip = false;
        report.seal();
        assert!(!report.ok);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].starts_with("settings: "));

        // A run that stopped is never ok, whatever it had.
        let stopped = good_report().stopped(timed_out_at(Step::Edge("left")));
        assert!(!stopped.ok);
        assert_eq!(stopped.error.as_deref(), Some("timed out at edge left"));
        assert_eq!(stopped.failures, ["timed out at edge left"]);
        assert_eq!(stopped.edges.len(), 1);
        let json = serde_json::to_value(stopped).expect("a report");
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"], "timed out at edge left");
    }

    #[test]
    fn a_rect_is_inside_the_work_area_up_to_its_sides_and_not_a_pixel_beyond() {
        let work = [0, 0, 1024, 728];
        assert!(inside([8, 8, 400, 680], work));
        // Touching every side.
        assert!(inside([0, 0, 1024, 728], work));
        assert!(inside([624, 48, 400, 680], work));
        // One pixel out, on each side.
        assert!(!inside([-1, 8, 400, 680], work));
        assert!(!inside([8, -1, 400, 680], work));
        assert!(!inside([625, 8, 400, 680], work));
        assert!(!inside([8, 49, 400, 680], work));
        // A work area that doesn't start at the origin (a taskbar on the left, a second monitor).
        let beside = [-1920, 40, 1920, 1040];
        assert!(inside([-1920, 40, 440, 1040], beside));
        assert!(!inside([-1921, 40, 440, 1040], beside));
        assert!(!inside([-400, 40, 440, 680], beside));
        // A window that was never placed is inside nothing.
        assert!(!inside([0, 0, 0, 0], work));
        // Sizes near the type's end don't wrap round.
        assert!(!inside([i32::MAX, 0, i32::MAX, 10], work));
    }

    #[test]
    fn the_tail_is_inside_the_corners_up_to_the_limit() {
        assert!(tail_inside_corners(0.0, 0.0));
        assert!(tail_inside_corners(306.0, 306.0));
        assert!(tail_inside_corners(-306.0, 306.0));
        assert!(!tail_inside_corners(306.5, 306.0));
        assert!(!tail_inside_corners(-306.5, 306.0));
        assert!(!tail_inside_corners(f64::NAN, 306.0));
        assert!(!tail_inside_corners(0.0, f64::NAN));
    }

    #[test]
    fn every_failed_check_of_an_edge_is_one_line() {
        assert_eq!(good_edge("right").failures, Vec::<String>::new());

        let mut edge = good_edge("bottom");
        edge.failures.clear();
        edge.panel = [700, 100, 400, 680];
        edge.tail_offset = 400.0;
        edge.topmost = false;
        edge.above_notch = false;
        edge.auto = AutoReport::default();
        edge.check();
        assert!(!edge.inside_work_area && !edge.tail_inside_corners);
        assert_eq!(edge.failures.len(), 7, "{:?}", edge.failures);
        assert!(edge.failures[0].contains("(700,100 400x680)"));

        // Each on its own.
        let mut edge = good_edge("top");
        edge.auto.gate_shut = false;
        edge.check();
        assert_eq!(
            edge.failures,
            ["the keyboard gate is open after the panel opened by itself"]
        );
        let mut edge = good_edge("top");
        edge.auto.gate_opens_on_confirmation = false;
        edge.check();
        assert_eq!(
            edge.failures,
            ["the keyboard gate stayed shut after the confirmation"]
        );
    }

    #[test]
    fn a_false_invariant_a_csp_violation_and_an_error_are_each_one_line_naming_the_page() {
        assert_eq!(page_failures(PANEL, &good_page()), Vec::<String>::new());

        let mut page = good_page();
        page.invariants.insert("badges_inside_pill".into(), false);
        assert_eq!(
            page_failures(NOTCH, &page),
            ["notch: the invariant badges_inside_pill does not hold"]
        );

        let mut page = good_page();
        page.csp_violations.push(json!({
            "blockedURI": "inline", "violatedDirective": "script-src",
            "sourceFile": "", "lineNumber": 3,
        }));
        assert_eq!(
            page_failures(SETTINGS, &page),
            ["settings: the content security policy blocked inline (script-src)"]
        );

        let mut page = good_page();
        page.errors
            .push(json!({ "kind": "console.error", "message": "no such call" }));
        assert_eq!(
            page_failures(PANEL, &page),
            ["agentnotch-panel: console.error: no such call"]
        );

        // All at once, and a round trip that failed: one line each.
        page.round_trip = false;
        page.invariants.insert("no_text_overflow".into(), false);
        page.csp_violations.push(json!({}));
        page.errors.push(json!("not an entry"));
        let lines = page_failures(PANEL, &page);
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert!(lines
            .iter()
            .all(|line| line.starts_with("agentnotch-panel: ")));
    }

    #[test]
    fn a_page_hooks_answers_become_invariants_and_only_true_holds() {
        assert_eq!(hook_invariants(&Value::Null), None);
        assert_eq!(hook_invariants(&json!([true])), None);
        assert_eq!(
            hook_invariants(&json!({ "rows_fit": true, "tail_drawn": false, "odd": "yes" })),
            Some(BTreeMap::from([
                ("odd".to_string(), false),
                ("rows_fit".to_string(), true),
                ("tail_drawn".to_string(), false),
            ]))
        );
        assert!(is_snapshot(&json!({ "version": 1, "rings": [] })));
        assert!(!is_snapshot(&json!({ "rings": null })));
        assert!(!is_snapshot(&json!({ "code": "not_allowed" })));
        assert!(!is_snapshot(&Value::Null));
    }

    // The port of testSnapshotRequestsAreReadFromTheFlagAndOnlySealedFromTheEnvironment.
    #[test]
    fn the_switches_are_read_only_in_a_sealed_run() {
        let exported = |name: &str| match name {
            "AGENTNOTCH_OPEN_PANEL_ON_LAUNCH" => Some("sessions".to_string()),
            "AGENTNOTCH_PANEL_SELF_TEST" => Some("1".to_string()),
            "AGENTNOTCH_SELF_TEST_OUT" => Some("/tmp/report.json".to_string()),
            "AGENTNOTCH_SNAPSHOT_CLAUDE" => Some("/tmp/agentnotch-snapshots".to_string()),
            "AGENTNOTCH_SELF_TEST_SCALE" => Some("1.25".to_string()),
            _ => None,
        };
        // A live app started from a shell that still exports them starts normally.
        assert_eq!(switches(false, exported), Switches::default());
        assert!(!Switches::default().self_test);

        let sealed = switches(true, exported);
        assert_eq!(sealed.open_route.as_deref(), Some("sessions"));
        assert!(sealed.self_test);
        assert_eq!(sealed.out, Some(PathBuf::from("/tmp/report.json")));
        assert_eq!(
            sealed.snapshots,
            Some(PathBuf::from("/tmp/agentnotch-snapshots"))
        );
        assert_eq!(sealed.scale.as_deref(), Some("1.25"));

        // Nothing set, or set to nothing: nothing asked for.
        assert_eq!(switches(true, |_| None), Switches::default());
        assert_eq!(switches(true, |_| Some(String::new())), Switches::default());
        // 0 is off; anything else is on.
        let only = |value: &'static str| {
            switches(true, move |name| {
                (name == "AGENTNOTCH_PANEL_SELF_TEST").then(|| value.to_string())
            })
        };
        assert!(!only("0").self_test);
        assert!(only("1").self_test);
        assert!(only("yes").self_test);
        // Without a place for the report the run is still asked for.
        assert_eq!(only("1").out, None);
    }

    #[test]
    fn the_asked_scale_is_a_number_a_monitor_can_have_or_an_error() {
        assert_eq!(wanted_scale("1"), Ok(1.0));
        assert_eq!(wanted_scale("1.25"), Ok(1.25));
        assert_eq!(wanted_scale(" 1.5 "), Ok(1.5));
        assert_eq!(wanted_scale("5"), Ok(5.0));
        for bad in [
            "", "big", "0", "0.8", "-1.25", "5.5", "NaN", "inf", "1,25", "125%",
        ] {
            let refused = wanted_scale(bad).unwrap_err();
            assert!(refused.contains(&format!("'{bad}'")), "{refused}");
        }
    }

    #[test]
    fn a_page_draws_at_the_asked_scale_or_the_run_says_so() {
        assert!(scale_is(1.25, 1.25));
        assert!(scale_is(1.5, 1.5000000001));
        assert!(scale_is(1.25, 1.2549999));
        // What run 36716764131 had: asked for 1.25 and 1.5, drawn at 1.
        assert!(!scale_is(1.25, 1.0));
        assert!(!scale_is(1.5, 1.0));
        assert!(!scale_is(1.25, 1.5));
        assert!(!scale_is(1.25, f64::NAN));
    }

    #[test]
    fn a_run_without_a_report_file_still_has_its_exit_code() {
        let good = good_report();
        let bad = Report::new("1.1.0").stopped(timed_out_at(Step::Settle));
        assert_eq!(write(None, &good), Ok(()));
        assert_eq!(write(None, &bad), Ok(()));
        assert_eq!((exit_code(&good), exit_code(&bad)), (0, 1));

        // With one: the whole report, and no temporary file left beside it.
        let folder =
            std::env::temp_dir().join(format!("agentnotch-selftest-report-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a scratch folder");
        let out = folder.join("report.json");
        assert_eq!(write(Some(&out), &bad), Ok(()));
        assert_eq!(write(Some(&out), &good), Ok(()));
        let read: Value = serde_json::from_str(&std::fs::read_to_string(&out).expect("the report"))
            .expect("JSON");
        assert_eq!(read, serde_json::to_value(&good).expect("a report"));
        let left: Vec<_> = std::fs::read_dir(&folder)
            .expect("the folder")
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect();
        assert_eq!(left, ["report.json"]);
        // A folder that isn't there is an error, not a panic (the exit code stands).
        assert!(write(Some(&folder.join("missing").join("report.json")), &good).is_err());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_steps_are_data_and_the_watchdog_names_the_one_it_stopped_in() {
        assert_eq!(EDGES, ["right", "left", "top", "bottom", "floating"]);
        let steps = steps();
        let edges: Vec<&str> = steps
            .iter()
            .filter_map(|step| match step {
                Step::Edge(edge) => Some(*edge),
                _ => None,
            })
            .collect();
        assert_eq!(edges, EDGES);
        assert_eq!(steps.first(), Some(&Step::Settle));
        assert_eq!(steps.get(1), Some(&Step::Collectors));
        assert_eq!(steps.last(), Some(&Step::Report));
        // The notch is put back before the pages are judged, after the last panel case.
        let at = |step| steps.iter().position(|s| *s == step).expect("a step");
        assert!(at(Step::Edge(FLOATING)) < at(Step::Widths));
        assert!(at(Step::Widths) < at(Step::Restore));
        assert!(at(Step::Restore) < at(Step::Pages));

        let names: Vec<String> = steps.iter().map(|step| step.name()).collect();
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for step in steps {
            assert_eq!(timed_out_at(step), format!("timed out at {}", step.name()));
        }
        assert_eq!(
            timed_out_at(Step::Edge("bottom")),
            "timed out at edge bottom"
        );
        assert_eq!(
            summary(&good_report().stopped(timed_out_at(Step::Pages))),
            "self-test FAILED: 1 failures, the first: timed out at page checks"
        );
        assert_eq!(summary(&good_report()), "self-test passed (1 edges)");
    }
}
