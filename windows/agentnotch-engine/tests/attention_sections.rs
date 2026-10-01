//! Sections of the sessions list (WP7): ports of SessionListTests
//! (SessionSectionsTests) and the layout tests of B_ReviewPass2Tests, plus
//! the sections of the UI contract's snapshot. The strip's VoiceOver text and
//! the row-model tests are page behaviour (node tests).

use agentnotch_engine::attention::sections::{
    build, collapsed_summary, infos, is_collapsed, is_compact, order_of, AttentionCounts,
    ListLayout, SessionSection,
};
use agentnotch_engine::model::{Bucket, NeedsInputReason, PermissionContext, Phase, SessionView};
use agentnotch_engine::sessions::session::{Session, SessionTitleSource};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

/// `offset` seconds from now (negative: before).
fn at(offset: i64) -> SystemTime {
    if offset < 0 {
        now() - Duration::from_secs(offset.unsigned_abs())
    } else {
        now() + Duration::from_secs(offset as u64)
    }
}

struct Make<'a> {
    id: &'a str,
    phase: Phase,
    reason: Option<NeedsInputReason>,
    turn_started: Option<i64>,
    completed: Option<i64>,
    reviewed: Option<i64>,
    last_activity: i64,
}

fn make(id: &str) -> Make<'_> {
    Make {
        id,
        phase: Phase::Idle,
        reason: None,
        turn_started: None,
        completed: None,
        reviewed: None,
        last_activity: -60,
    }
}

impl Make<'_> {
    fn phase(mut self, phase: Phase) -> Self {
        self.phase = phase;
        self
    }
    fn reason(mut self, reason: NeedsInputReason) -> Self {
        self.reason = Some(reason);
        self
    }
    fn turn(mut self, secs: i64) -> Self {
        self.turn_started = Some(secs);
        self
    }
    fn completed(mut self, secs: i64) -> Self {
        self.completed = Some(secs);
        self
    }
    fn active(mut self, secs: i64) -> Self {
        self.last_activity = secs;
        self
    }
    fn view(self) -> SessionView {
        let mut s = Session::new(self.id, format!("/tmp/{}", self.id), at(self.last_activity));
        s.phase = self.phase;
        s.set_needs_input(self.reason, at(self.last_activity));
        s.turn_started_at = self.turn_started.map(at);
        s.completed_at = self.completed.map(at);
        s.reviewed_at = self.reviewed.map(at);
        s.last_activity = at(self.last_activity);
        s.to_view()
    }
}

fn approval(tool: &str, received_ago: u64) -> Phase {
    Phase::WaitingForApproval(PermissionContext {
        tool_use_id: format!("toolu_{tool}"),
        tool_name: tool.into(),
        tool_input: Value::Null,
        received_at: now() - Duration::from_secs(received_ago),
        permission_suggestions: vec![],
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: None,
    })
}

fn dialog() -> NeedsInputReason {
    NeedsInputReason::Dialog {
        detail: String::new(),
    }
}

fn error(text: &str) -> NeedsInputReason {
    NeedsInputReason::Error {
        text: text.into(),
        code: None,
    }
}

fn ids(sections: &[SessionSection<'_>]) -> Vec<Vec<String>> {
    sections
        .iter()
        .map(|s| {
            s.sessions
                .iter()
                .map(|v| v.id.as_str().to_owned())
                .collect()
        })
        .collect()
}

fn strs(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|s| (*s).to_owned()).collect())
        .collect()
}

// ---- SessionSectionsTests ----

#[test]
fn sections_follow_bucket_order_and_skip_empty_ones() {
    let sessions = vec![
        make("idle").view(),
        make("working").phase(Phase::Processing).turn(-30).view(),
        make("review")
            .phase(Phase::WaitingForInput)
            .completed(-10)
            .view(),
        make("needs").phase(approval("Bash", 5)).view(),
    ];
    let sections = build(&sessions, None);
    assert_eq!(
        sections.iter().map(|s| s.bucket).collect::<Vec<_>>(),
        [
            Bucket::NeedsYou,
            Bucket::ReadyForReview,
            Bucket::Working,
            Bucket::Idle
        ]
    );
    assert_eq!(
        ids(&sections),
        strs(&[&["needs"], &["review"], &["working"], &["idle"]])
    );
    let few = vec![make("w").phase(Phase::Processing).view(), make("i").view()];
    assert_eq!(
        build(&few, None)
            .iter()
            .map(|s| s.bucket)
            .collect::<Vec<_>>(),
        [Bucket::Working, Bucket::Idle]
    );
    assert!(build(&[], None).is_empty());
}

#[test]
fn needs_you_is_oldest_waiting_first_with_failed_turns_after_answers() {
    let sessions = vec![
        make("recent").phase(approval("Bash", 30)).view(),
        make("oldest").phase(approval("Edit", 600)).view(),
        // Without a pending approval the last hook event is when it started waiting.
        make("dialog")
            .phase(Phase::WaitingForInput)
            .reason(dialog())
            .active(-120)
            .view(),
        // A failed turn waits on a retry, not an answer: it never pushes a
        // prompt below the fold, however long ago it failed.
        make("error")
            .phase(Phase::WaitingForInput)
            .reason(error("Rate limited"))
            .active(-900)
            .view(),
        make("error2")
            .phase(Phase::WaitingForInput)
            .reason(error("Overloaded"))
            .active(-60)
            .view(),
    ];
    assert_eq!(
        ids(&build(&sessions, None)),
        strs(&[&["oldest", "dialog", "recent", "error", "error2"]])
    );
}

#[test]
fn review_is_newest_completion_first() {
    let sessions = vec![
        make("old")
            .phase(Phase::WaitingForInput)
            .completed(-3_600)
            .view(),
        make("new")
            .phase(Phase::WaitingForInput)
            .completed(-60)
            .view(),
        make("mid").completed(-600).view(),
    ];
    assert_eq!(
        ids(&build(&sessions, None)),
        strs(&[&["new", "mid", "old"]])
    );
}

#[test]
fn working_is_longest_running_first_with_unknown_starts_last() {
    let sessions = vec![
        make("short").phase(Phase::Processing).turn(-30).view(),
        make("unknown").phase(Phase::Compacting).view(),
        make("long").phase(Phase::Processing).turn(-900).view(),
    ];
    assert_eq!(
        ids(&build(&sessions, None)),
        strs(&[&["long", "short", "unknown"]])
    );
}

#[test]
fn idle_is_most_recent_first() {
    let sessions = vec![
        make("yesterday").active(-86_400).view(),
        make("now").active(-5).view(),
        make("hour").active(-3_600).view(),
    ];
    assert_eq!(
        ids(&build(&sessions, None)),
        strs(&[&["now", "hour", "yesterday"]])
    );
}

#[test]
fn ties_are_broken_by_session_id_whatever_the_input_order() {
    let mk = |id: &str| make(id).phase(Phase::Processing).turn(-60).view();
    let (a, b, c) = (mk("a"), mk("b"), mk("c"));
    assert_eq!(
        ids(&build(&[c.clone(), a.clone(), b.clone()], None)),
        strs(&[&["a", "b", "c"]])
    );
    assert_eq!(ids(&build(&[b, c, a], None)), strs(&[&["a", "b", "c"]]));
}

#[test]
fn preserved_order_keeps_rows_put_and_appends_newcomers() {
    let first = make("first")
        .phase(Phase::WaitingForInput)
        .completed(-600)
        .view();
    let second = make("second")
        .phase(Phase::WaitingForInput)
        .completed(-300)
        .view();
    // Displayed while hovered: second (newest) above first.
    let displayed = order_of(&build(&[first.clone(), second.clone()], None));
    assert_eq!(displayed, ["second", "first"]);

    // "first" finishes again (now newest) and a new review arrives: under the
    // pointer nothing moves; the newcomer goes to the end.
    let refreshed = make("first")
        .phase(Phase::WaitingForInput)
        .completed(-10)
        .view();
    let newcomer = make("newcomer")
        .phase(Phase::WaitingForInput)
        .completed(-5)
        .view();
    let all = vec![refreshed, second, newcomer];
    assert_eq!(
        ids(&build(&all, Some(&displayed))),
        strs(&[&["second", "first", "newcomer"]])
    );
    // Without the pointer the natural order returns.
    assert_eq!(
        ids(&build(&all, None)),
        strs(&[&["newcomer", "first", "second"]])
    );
}

#[test]
fn preserved_order_still_moves_sessions_between_sections() {
    let working = make("w").phase(Phase::Processing).turn(-60).view();
    let displayed = order_of(&build(&[working], None));
    let finished = make("w").phase(Phase::WaitingForInput).completed(-1).view();
    let finished = [finished];
    let sections = build(&finished, Some(&displayed));
    assert_eq!(
        sections.iter().map(|s| s.bucket).collect::<Vec<_>>(),
        [Bucket::ReadyForReview]
    );
}

#[test]
fn counts_per_bucket_with_failed_turns_apart() {
    let sessions = vec![
        make("n1").phase(approval("Bash", 1)).view(),
        make("n2").reason(dialog()).view(),
        make("f")
            .phase(Phase::WaitingForInput)
            .reason(error("Rate limited"))
            .view(),
        make("r").phase(Phase::WaitingForInput).completed(-1).view(),
        make("w1").phase(Phase::Processing).view(),
        make("w2").phase(Phase::Compacting).view(),
        make("w3").phase(Phase::Processing).view(),
        make("i").view(),
    ];
    let counts = AttentionCounts::of(&sessions);
    assert_eq!(
        counts,
        AttentionCounts {
            needs_input: 3,
            ready_for_review: 1,
            working: 3,
            idle: 1,
            failed: 1,
        }
    );
    assert_eq!(counts.answerable(), 2);
    assert_eq!(counts.total(), 8);
}

#[test]
fn section_titles() {
    assert_eq!(
        Bucket::ALL.map(Bucket::title),
        ["Needs you", "Ready for review", "Working", "Idle"]
    );
    assert_eq!(
        Bucket::ALL.map(Bucket::as_str),
        ["needs_you", "ready_for_review", "working", "idle"]
    );
}

// ---- density ----

#[test]
fn only_a_long_idle_list_starts_folded_and_needs_you_never_folds() {
    let none = HashMap::new();
    assert!(!is_collapsed(Bucket::Idle, 3, &none));
    assert!(is_collapsed(Bucket::Idle, 4, &none));
    assert!(!is_collapsed(Bucket::Working, 40, &none));
    assert!(is_collapsed(
        Bucket::Working,
        2,
        &HashMap::from([(Bucket::Working, true)])
    ));
    assert!(!is_collapsed(
        Bucket::Idle,
        9,
        &HashMap::from([(Bucket::Idle, false)])
    ));
    assert!(!is_collapsed(
        Bucket::NeedsYou,
        9,
        &HashMap::from([(Bucket::NeedsYou, true)])
    ));
}

#[test]
fn rows_go_compact_past_eight_sessions() {
    assert!(!is_compact(8));
    assert!(is_compact(9));
}

#[test]
fn a_folded_summary_names_the_first_titles_and_counts_the_rest() {
    let titled = |id: &str, title: &str| {
        let mut s = Session::new(id, format!("/tmp/{id}"), now());
        s.phase = Phase::Processing;
        s.apply_title(Some(title), SessionTitleSource::Hook);
        s.to_view()
    };
    let views = [
        titled("a", "Write tests"),
        titled("b", "Fix CI"),
        titled("c", "Bump deps"),
    ];
    let all = SessionSection {
        bucket: Bucket::Working,
        sessions: views.iter().collect(),
    };
    assert_eq!(collapsed_summary(&all, 2), "Write tests, Fix CI and 1 more");
    let one = SessionSection {
        bucket: Bucket::Working,
        sessions: views.iter().take(1).collect(),
    };
    assert_eq!(collapsed_summary(&one, 2), "Write tests");
}

#[test]
fn the_layout_orders_the_keyboard_through_unfolded_rows_only() {
    let sessions = vec![
        make("needs").phase(approval("Bash", 5)).view(),
        make("w").phase(Phase::Processing).turn(-30).view(),
        make("i1").active(-10).view(),
        make("i2").active(-20).view(),
        make("i3").active(-30).view(),
        make("i4").active(-40).view(),
    ];
    let layout = ListLayout::make(&build(&sessions, None), &HashMap::new());
    // Four idle sessions fold by default.
    assert_eq!(
        layout
            .sections
            .iter()
            .map(|s| s.is_collapsed)
            .collect::<Vec<_>>(),
        [false, false, true]
    );
    assert_eq!(layout.visible_order(), ["needs", "w"]);
    assert_eq!(layout.bucket_of("i3"), Some(Bucket::Idle));
    assert!(!layout.is_compact);
}

// ---- B_ReviewPass2Tests ----

fn layout_of(sessions: &[SessionView], folds: &HashMap<Bucket, bool>) -> ListLayout {
    ListLayout::make(&build(sessions, None), folds)
}

#[test]
fn a_folded_idle_list_does_not_make_the_rows_above_it_compact() {
    let mut sessions: Vec<SessionView> = (0..5)
        .map(|i| make(&format!("w{i}")).phase(Phase::Processing).view())
        .collect();
    sessions.extend((0..20).map(|i| make(&format!("i{i}")).active(-((i + 1) * 60)).view()));
    // 25 sessions, but only the 5 working rows are drawn.
    assert!(!layout_of(&sessions, &HashMap::new()).is_compact);
    // Unfolding the idle list draws 25 rows: now they go to one line.
    assert!(layout_of(&sessions, &HashMap::from([(Bucket::Idle, false)])).is_compact);
}

#[test]
fn a_highlighted_row_names_the_folded_section_it_is_in() {
    let mut sessions: Vec<SessionView> = (0..2)
        .map(|i| make(&format!("w{i}")).phase(Phase::Processing).view())
        .collect();
    sessions.extend((0..5).map(|i| make(&format!("i{i}")).active(-((i + 1) * 60)).view()));
    let list = layout_of(&sessions, &HashMap::new());
    // Idle starts folded: a banner pointing at i3 must unfold it.
    assert_eq!(
        list.folded_bucket_containing(Some("i3")),
        Some(Bucket::Idle)
    );
    assert_eq!(
        list.folded_bucket_containing(Some("w1")),
        None,
        "already on screen"
    );
    assert_eq!(list.folded_bucket_containing(Some("gone")), None);
    assert_eq!(list.folded_bucket_containing(None), None);
}

// ---- the UI contract ----

#[test]
fn the_snapshots_sections_follow_from_its_rows() {
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/ui-contract/snapshot.json"
        ))
        .unwrap(),
    )
    .unwrap();
    // A session per fixture row, in its bucket.
    let views: Vec<SessionView> = fixture["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let id = row["session_id"].as_str().unwrap();
            match (
                row["bucket"].as_str().unwrap(),
                row["failed"].as_bool().unwrap(),
            ) {
                ("needs_you", true) => make(id)
                    .phase(Phase::WaitingForInput)
                    .reason(error("Rate limited"))
                    .view(),
                ("needs_you", false) => make(id).phase(approval("Bash", 60)).view(),
                ("ready_for_review", _) => {
                    make(id).phase(Phase::WaitingForInput).completed(-60).view()
                }
                ("working", _) => make(id).phase(Phase::Processing).turn(-60).view(),
                _ => make(id).view(),
            }
        })
        .collect();
    let sections = build(&views, None);
    let got = serde_json::to_value(infos(&sections)).unwrap();
    assert_eq!(got, fixture["sections"]);
}
