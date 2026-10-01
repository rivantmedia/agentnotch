//! Session rows and hover-card rows (WP7): ports of SessionRowContentTests,
//! A3_ActivityRowsTests and the privacy rule of A3_ReviewTests, plus the
//! proof that rows built from the UI contract's snapshot inputs say what the
//! fixture says. The row-model, key-router and meter tests of the Mac's
//! SessionRowContentTests are page behaviour (panel-list.js) and are covered
//! by the node tests; the question parser is covered by sessions_record.

use agentnotch_engine::attention::rows::{
    accessibility_label, age_text, card_row, compact_detail, detail, duration_text, elapsed,
    pending_view, plain_text, reset_phrase, session_row, spoken_state, state_word, task_summary,
    RateLimitReset, ResetClock, RowContext, TERMINAL_WAIT,
};
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::core::time::from_ms;
use agentnotch_engine::model::ui::{RowDetail, UpstreamWindow};
use agentnotch_engine::model::{
    NeedsInputReason, PermissionContext, Phase, RingId, SessionState, SessionView, TaskProgress,
};
use agentnotch_engine::sessions::session::{Session, SessionTitleSource};
use agentnotch_engine::sessions::summary::ConversationInfo;
use agentnotch_engine::usage::ring_windows::RingWindow;
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn ago(secs: u64) -> SystemTime {
    now() - Duration::from_secs(secs)
}

fn clock() -> ResetClock {
    ResetClock::default()
}

/// SessionRowContentTests.session: an idle session with the last message
/// given, last active two hours ago.
fn session(
    phase: Phase,
    reason: Option<NeedsInputReason>,
    last_message: Option<&str>,
    role: Option<&str>,
    tool: Option<&str>,
) -> Session {
    let mut session = Session::new("s1", "/Users/me/code/acme", ago(7_200));
    session.phase = phase;
    session.conversation_info = ConversationInfo {
        last_message: last_message.map(str::to_owned),
        last_message_role: role.map(str::to_owned),
        last_tool_name: tool.map(str::to_owned),
        ..ConversationInfo::default()
    };
    session.set_needs_input(reason, ago(7_200));
    session
}

fn plain() -> Session {
    session(Phase::Idle, None, None, None, None)
}

fn approval(tool: &str, input: Value, suggestions: Vec<Value>, received_ago: u64) -> Phase {
    Phase::WaitingForApproval(PermissionContext {
        tool_use_id: "toolu_1".into(),
        tool_name: tool.into(),
        tool_input: input,
        received_at: ago(received_ago),
        permission_suggestions: suggestions,
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: None,
    })
}

fn permission_reason(tool: &str) -> NeedsInputReason {
    NeedsInputReason::Permission {
        tool: Some(tool.into()),
    }
}

fn error(text: &str) -> NeedsInputReason {
    NeedsInputReason::Error {
        text: text.into(),
        code: None,
    }
}

fn rate_limited() -> NeedsInputReason {
    NeedsInputReason::Error {
        text: "Rate limited".into(),
        code: Some("rate_limit".into()),
    }
}

fn view_of(session: &Session) -> SessionView {
    session.to_view()
}

fn detail_of(session: &Session) -> RowDetail {
    detail(&view_of(session), None, now(), clock())
}

fn working_with_task(task: &str) -> Session {
    let mut working = session(
        Phase::Processing,
        None,
        Some("ls"),
        Some("tool"),
        Some("Bash"),
    );
    working.tasks.todos_replaced(vec![
        ("Write tests".into(), "completed".into(), None),
        ("Run tests".into(), "in_progress".into(), Some(task.into())),
    ]);
    working
}

// ---- SessionRowContentTests ----

#[test]
fn glyph_follows_the_state() {
    let words = |session: &Session| {
        let view = view_of(session);
        (state_word(&view.state), spoken_state(&view.state))
    };
    assert_eq!(
        words(&session(
            approval("Bash", Value::Null, vec![], 90),
            None,
            None,
            None,
            None
        )),
        ("needs you", "Needs you")
    );
    assert_eq!(
        words(&session(
            Phase::Idle,
            Some(error("Overloaded")),
            None,
            None,
            None
        )),
        ("failed", "Failed")
    );
    let mut review = session(Phase::WaitingForInput, None, None, None, None);
    review.completed_at = Some(now());
    assert_eq!(words(&review), ("done", "Ready for review"));
    assert_eq!(
        words(&session(Phase::Processing, None, None, None, None)),
        ("working", "Working")
    );
    assert_eq!(words(&plain()), ("idle", "Idle"));
}

#[test]
fn elapsed_depends_on_the_bucket() {
    // Needs you: time since the request arrived.
    let waiting = session(
        approval("Bash", Value::Null, vec![], 150),
        None,
        None,
        None,
        None,
    );
    assert_eq!(elapsed(&view_of(&waiting), now()).as_deref(), Some("2m"));

    // Working: time since the prompt.
    let mut working = session(Phase::Processing, None, None, None, None);
    working.turn_started_at = Some(ago(3_600 + 12 * 60));
    assert_eq!(
        elapsed(&view_of(&working), now()).as_deref(),
        Some("1h 12m")
    );
    let unstarted = session(Phase::Processing, None, None, None, None);
    assert_eq!(elapsed(&view_of(&unstarted), now()), None);

    // Review: when it finished.
    let mut review = session(Phase::WaitingForInput, None, None, None, None);
    review.completed_at = Some(ago(5 * 60));
    assert_eq!(elapsed(&view_of(&review), now()).as_deref(), Some("5m ago"));
    review.completed_at = Some(ago(20));
    assert_eq!(
        elapsed(&view_of(&review), now()).as_deref(),
        Some("just now")
    );

    // Idle: last activity.
    assert_eq!(
        elapsed(&view_of(&plain()), now()).as_deref(),
        Some("2h ago")
    );
}

#[test]
fn durations_and_ages() {
    let secs = Duration::from_secs;
    assert_eq!(duration_text(secs(0)), "<1m");
    assert_eq!(duration_text(secs(59)), "<1m");
    assert_eq!(duration_text(secs(45 * 60)), "45m");
    assert_eq!(duration_text(secs(2 * 3600 + 13 * 60)), "2h 13m");
    assert_eq!(duration_text(secs(2 * 3600)), "2h");
    assert_eq!(duration_text(secs(3 * 86_400 + 5 * 3600)), "3d 5h");
    assert_eq!(duration_text(secs(3 * 86_400)), "3d");
    // Bad data is held at 10 000 days instead of overflowing.
    assert_eq!(duration_text(Duration::MAX), "10000d");
    assert_eq!(age_text(ago(10), now()), "just now");
    assert_eq!(age_text(ago(60), now()), "1m ago");
    assert_eq!(age_text(ago(3 * 3600), now()), "3h ago");
    assert_eq!(age_text(ago(2 * 86_400), now()), "2d ago");
    // A later date reads "just now".
    assert_eq!(
        age_text(now() + Duration::from_secs(500), now()),
        "just now"
    );
}

#[test]
fn permission_shows_tool_and_the_whole_input() {
    let mut s = session(
        approval(
            "Bash",
            json!({"command": "npm test\n  --watch=false"}),
            vec![],
            90,
        ),
        None,
        None,
        None,
        None,
    );
    s.set_needs_input(None, now());
    // The whole command, line breaks kept: nothing hides past the edge.
    assert_eq!(
        detail_of(&s),
        RowDetail::Permission {
            tool: "Bash".into(),
            request: "npm test\n  --watch=false".into(),
            waiting_in_terminal: false,
        }
    );
}

#[test]
fn permission_preview_shows_full_paths_under_home() {
    let mut s = session(
        approval(
            "Edit",
            json!({"file_path": "/Users/me/code/acme/src/app.ts"}),
            vec![],
            90,
        ),
        None,
        None,
        None,
        None,
    );
    s.paths = Some(Paths::new(PathStyle::Posix, "/Users/me"));
    assert_eq!(
        detail_of(&s),
        RowDetail::Permission {
            tool: "Edit".into(),
            request: "~/code/acme/src/app.ts".into(),
            waiting_in_terminal: false,
        }
    );
    // A request without an input says only the tool.
    let bare = session(
        approval("Bash", Value::Null, vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        plain_text(&detail_of(&bare)),
        "Bash",
        "no input: just the tool"
    );
}

#[test]
fn requests_too_long_for_the_row_go_to_the_chat() {
    let long = session(
        approval(
            "Bash",
            json!({"command": "rm -rf build && ".repeat(20)}),
            vec![],
            90,
        ),
        None,
        None,
        None,
        None,
    );
    let view = view_of(&long);
    assert!(pending_view(view.active_request().unwrap()).needs_review);
    let short = session(
        approval("Bash", json!({"command": "npm test"}), vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert!(!pending_view(view_of(&short).active_request().unwrap()).needs_review);
    let tall = session(
        approval("Bash", json!({"command": "a\nb\nc\nd\ne"}), vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert!(pending_view(view_of(&tall).active_request().unwrap()).needs_review);
}

#[test]
fn a_permission_seen_only_in_the_terminal_says_so() {
    let named = session(
        Phase::Idle,
        Some(permission_reason("Edit")),
        None,
        None,
        None,
    );
    assert_eq!(
        detail_of(&named),
        RowDetail::Permission {
            tool: "Edit".into(),
            request: TERMINAL_WAIT.into(),
            waiting_in_terminal: true,
        }
    );
    let unnamed = session(
        Phase::Idle,
        Some(NeedsInputReason::Permission {
            tool: Some(String::new()),
        }),
        None,
        None,
        None,
    );
    assert_eq!(
        detail_of(&unnamed),
        RowDetail::Permission {
            tool: "Permission".into(),
            request: TERMINAL_WAIT.into(),
            waiting_in_terminal: true,
        }
    );
    // Nothing for the bar to answer.
    assert!(view_of(&named).active_request().is_none());
}

#[test]
fn mcp_tool_names_are_formatted() {
    let s = session(
        approval("mcp__github__create_issue", Value::Null, vec![], 90),
        None,
        None,
        None,
        None,
    );
    match detail_of(&s) {
        RowDetail::Permission { tool, .. } => assert_eq!(tool, "Github - Create Issue"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_question_shows_its_first_question() {
    let input = json!({"questions": [{"question": "Which library?", "options": [{"label": "A"}, {"label": "B"}]}]});
    let asks = session(
        approval("AskUserQuestion", input, vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        detail_of(&asks),
        RowDetail::Question {
            text: "Which library?".into()
        }
    );
    assert_eq!(plain_text(&detail_of(&asks)), "Asks Which library?");
    let empty = session(
        approval("AskUserQuestion", Value::Null, vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        detail_of(&empty),
        RowDetail::Question {
            text: "A question for you".into()
        }
    );
}

#[test]
fn plan_dialog_and_elicitation() {
    let plan = session(
        approval("ExitPlanMode", Value::Null, vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert_eq!(detail_of(&plan), RowDetail::Plan);
    let dialog = session(
        Phase::Idle,
        Some(NeedsInputReason::Dialog {
            detail: "permission prompt".into(),
        }),
        None,
        None,
        None,
    );
    assert_eq!(
        detail_of(&dialog),
        RowDetail::Dialog {
            text: "Permission prompt".into()
        }
    );
    let elicitation = session(
        Phase::Idle,
        Some(NeedsInputReason::Elicitation {
            message: "Pick a repo".into(),
        }),
        None,
        None,
        None,
    );
    assert_eq!(
        detail_of(&elicitation),
        RowDetail::Dialog {
            text: "Pick a repo".into()
        }
    );
}

#[test]
fn a_rate_limit_names_the_window_that_is_spent() {
    let limited = view_of(&session(
        Phase::Idle,
        Some(rate_limited()),
        None,
        None,
        None,
    ));
    let five_hour = RateLimitReset {
        window: "5-hour limit".into(),
        resets_at: now() + Duration::from_secs(47 * 60),
    };
    assert_eq!(
        detail(&limited, Some(&five_hour), now(), clock()),
        RowDetail::Failed {
            text: "Rate limited · 5-hour limit resets in 47m".into()
        }
    );
    // Nothing known to be spent: just the error.
    assert_eq!(
        detail(&limited, None, now(), clock()),
        RowDetail::Failed {
            text: "Rate limited".into()
        }
    );
    // Other errors never get a reset time.
    let overloaded = view_of(&session(
        Phase::Idle,
        Some(error("Overloaded")),
        None,
        None,
        None,
    ));
    assert_eq!(
        detail(&overloaded, Some(&five_hour), now(), clock()),
        RowDetail::Failed {
            text: "Overloaded".into()
        }
    );
}

fn window(id: &str, label: Option<&str>, used: f64, resets: Option<SystemTime>) -> RingWindow {
    RingWindow {
        id: id.into(),
        label: label.map(str::to_owned),
        used_fraction: used,
        resets_at: resets,
        duration_s: None,
        money: None,
    }
}

#[test]
fn the_spent_window_is_the_week_when_the_week_is_out() {
    let week = now() + Duration::from_secs(3 * 86_400);
    let reading = [
        window(
            "session",
            None,
            0.62,
            Some(now() + Duration::from_secs(3_600)),
        ),
        window("weekly_all", None, 1.0, Some(week)),
    ];
    assert_eq!(
        RateLimitReset::current(&reading, now()),
        Some(RateLimitReset {
            window: "weekly limit".into(),
            resets_at: week
        })
    );

    // Both spent: the later reset is the real wait.
    let both = [
        window("session", None, 1.1, Some(now() + Duration::from_secs(600))),
        window("weekly_opus", None, 1.0, Some(week)),
    ];
    assert_eq!(
        RateLimitReset::current(&both, now()),
        Some(RateLimitReset {
            window: "Opus weekly limit".into(),
            resets_at: week
        })
    );

    // Nothing at 100%, a reset already past, or money windows: no claim.
    let fine = [window("session", None, 0.92, Some(week))];
    assert_eq!(RateLimitReset::current(&fine, now()), None);
    let past = [window("session", None, 1.0, Some(ago(1)))];
    assert_eq!(RateLimitReset::current(&past, now()), None);
    assert_eq!(RateLimitReset::current(&[], now()), None);
    let mut money = window("extra_usage", Some("Extra usage"), 1.0, Some(week));
    money.money = Some(agentnotch_engine::usage::ring_windows::Money {
        currency: "USD".into(),
        spent: 5.0,
        remaining: 0.0,
    });
    assert_eq!(RateLimitReset::current(&[money], now()), None);
}

#[test]
fn the_spent_window_from_the_snapshots_windows() {
    let ms = |at: SystemTime| Some(agentnotch_engine::core::time::to_ms(at));
    let week = now() + Duration::from_secs(3 * 86_400);
    let windows = [
        UpstreamWindow {
            id: "session".into(),
            label: "5-hour".into(),
            used: 0.4,
            resets_at: ms(now() + Duration::from_secs(600)),
            ..UpstreamWindow::default()
        },
        UpstreamWindow {
            id: "weekly_all".into(),
            label: "Weekly".into(),
            used: 1.0,
            resets_at: ms(week),
            ..UpstreamWindow::default()
        },
        UpstreamWindow {
            id: "extra_usage".into(),
            label: "Extra usage".into(),
            used: 1.0,
            resets_at: ms(week + Duration::from_secs(86_400)),
            ..UpstreamWindow::default()
        },
    ];
    assert_eq!(
        RateLimitReset::current_of_snapshot(&windows, now()),
        Some(RateLimitReset {
            window: "weekly limit".into(),
            resets_at: week
        })
    );
    assert_eq!(
        RateLimitReset::current_of_snapshot(&windows[..1], now()),
        None
    );
}

#[test]
fn reset_phrases() {
    let c = clock();
    assert_eq!(reset_phrase(None, now(), c), None);
    assert_eq!(
        reset_phrase(Some(ago(1)), now(), c).as_deref(),
        Some("has reset")
    );
    assert_eq!(
        reset_phrase(Some(now() + Duration::from_secs(47 * 60)), now(), c).as_deref(),
        Some("resets in 47m")
    );
    // 2027-01-15 08:00 UTC is 1_800_000_000 + 3 days + 8h-ish: Thursday.
    let later = now() + Duration::from_secs(3 * 86_400);
    let text = reset_phrase(Some(later), now(), c).unwrap();
    assert!(text.starts_with("resets "), "{text}");
    assert!(text.ends_with("AM") || text.ends_with("PM"), "{text}");
    // 24-hour clock, an hour east of UTC.
    let c24 = ResetClock {
        utc_offset_seconds: 3_600,
        hour12: false,
    };
    let text = reset_phrase(Some(later), now(), c24).unwrap();
    assert!(!text.ends_with("AM") && !text.ends_with("PM"), "{text}");
    assert!(text.contains(':'), "{text}");
    // A week out on today's weekday is "next <weekday>".
    let week = reset_phrase(Some(now() + Duration::from_secs(7 * 86_400)), now(), c).unwrap();
    assert!(week.starts_with("resets next "), "{week}");
}

#[test]
fn working_prefers_the_active_task_then_the_tool_then_thinking() {
    let with_task = working_with_task("Running tests");
    assert_eq!(
        detail_of(&with_task),
        RowDetail::Working {
            text: "Running tests".into(),
            secondary: false
        }
    );
    let with_tool = session(
        Phase::Processing,
        None,
        Some("src/**/*.swift"),
        Some("tool"),
        Some("Glob"),
    );
    assert_eq!(
        detail_of(&with_tool),
        RowDetail::Working {
            text: "Glob src/**/*.swift".into(),
            secondary: true
        }
    );
    let thinking = session(
        Phase::Processing,
        None,
        Some("Sure, let me look"),
        Some("assistant"),
        None,
    );
    assert_eq!(
        detail_of(&thinking),
        RowDetail::Working {
            text: "Thinking…".into(),
            secondary: true
        }
    );
    assert_eq!(
        detail_of(&session(Phase::Compacting, None, None, None, None)),
        RowDetail::Working {
            text: "Compacting context…".into(),
            secondary: true
        }
    );
}

#[test]
fn review_shows_the_last_reply() {
    let mut review = session(
        Phase::WaitingForInput,
        None,
        Some("short"),
        Some("assistant"),
        None,
    );
    review.completed_at = Some(now());
    review.last_assistant_message = Some("All done.\n\nTests pass.".into());
    let text = |s: &Session| match detail_of(s) {
        RowDetail::Review { text } => text,
        other => panic!("{other:?}"),
    };
    assert_eq!(text(&review), "All done. Tests pass.");
    review.last_assistant_message = Some("   ".into());
    assert_eq!(text(&review), "short");
    review.conversation_info = ConversationInfo::default();
    assert_eq!(text(&review), "Finished");
}

#[test]
fn idle_shows_the_last_message() {
    let idle = |message: Option<&str>, role: Option<&str>, tool: Option<&str>| match detail_of(
        &session(Phase::Idle, None, message, role, tool),
    ) {
        RowDetail::Idle { text } => text,
        other => panic!("{other:?}"),
    };
    assert_eq!(idle(Some("thanks"), Some("user"), None), "You: thanks");
    assert_eq!(
        idle(Some("npm outdated"), Some("tool"), Some("Bash")),
        "Bash npm outdated"
    );
    assert_eq!(idle(Some("Done."), Some("assistant"), None), "Done.");
    assert_eq!(idle(None, None, None), "No messages yet");
}

#[test]
fn the_request_a_bar_answers_carries_its_rule_only_when_it_is_narrow() {
    let local = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}],
                       "behavior": "allow", "destination": "localSettings"});
    let with_rule = session(
        approval("Bash", json!({"command": "npm test"}), vec![local], 90),
        None,
        None,
        None,
        None,
    );
    let pending = pending_view(view_of(&with_rule).active_request().unwrap());
    assert_eq!(pending.kind, "permission");
    assert_eq!(pending.tool_use_id, "toolu_1");
    assert_eq!(
        pending.always.as_deref(),
        Some("Don't ask again for Bash(npm test:*) in this project (just you)")
    );
    assert!(pending.inline_always);

    // A rule for every project, or a mode switch, is answered where it can
    // be read in full: no Always on the row.
    let everywhere =
        json!({"type": "addRules", "rules": [{"toolName": "Bash"}], "destination": "userSettings"});
    let wide = session(
        approval("Bash", json!({"command": "ls"}), vec![everywhere], 90),
        None,
        None,
        None,
        None,
    );
    let pending = pending_view(view_of(&wide).active_request().unwrap());
    assert_eq!((pending.always, pending.inline_always), (None, false));
    let none = session(
        approval("Bash", json!({"command": "ls"}), vec![], 90),
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        pending_view(view_of(&none).active_request().unwrap()).always,
        None
    );
}

#[test]
fn plans_failed_turns_and_working_sessions_have_nothing_inline_to_answer() {
    let plan = session(
        approval(
            "ExitPlanMode",
            json!({"plan": "## Plan\n\n1. Do it"}),
            vec![],
            90,
        ),
        None,
        None,
        None,
        None,
    );
    let pending = pending_view(view_of(&plan).active_request().unwrap());
    assert_eq!(pending.kind, "plan");
    assert_eq!(pending.request, "Plan ready for approval");
    assert_eq!(
        pending.plan_markdown.as_deref(),
        Some("## Plan\n\n1. Do it")
    );
    assert!(view_of(&session(Phase::Processing, None, None, None, None))
        .active_request()
        .is_none());
    // A failed turn and a prompt seen only in the terminal: nothing to answer.
    assert!(view_of(&session(
        Phase::Idle,
        Some(error("Rate limited")),
        None,
        None,
        None
    ))
    .active_request()
    .is_none());
    assert!(view_of(&session(
        Phase::Idle,
        Some(permission_reason("Bash")),
        None,
        None,
        None
    ))
    .active_request()
    .is_none());
}

#[test]
fn simple_questions_answer_with_one_tap_others_open_the_chat() {
    let pending = |input: Value| {
        pending_view(
            view_of(&session(
                approval("AskUserQuestion", input, vec![], 90),
                None,
                None,
                None,
                None,
            ))
            .active_request()
            .unwrap(),
        )
    };
    let simple = pending(
        json!({"questions": [{"question": "Which?", "header": "Lib", "multiSelect": false,
        "options": [{"label": "A", "description": "first"}, {"label": "B"}]}]}),
    );
    assert!(simple.single_tap);
    assert_eq!(simple.request, "Which?");
    let question = &simple.questions.as_ref().unwrap()[0];
    assert_eq!(question.header.as_deref(), Some("Lib"));
    assert_eq!(question.options[0].description.as_deref(), Some("first"));

    let multi = pending(
        json!({"questions": [{"question": "Which?", "multiSelect": true,
        "options": [{"label": "A"}, {"label": "B"}]}]}),
    );
    assert!(!multi.single_tap);
    let two = pending(json!({"questions": [
        {"question": "One?", "options": [{"label": "A"}]},
        {"question": "Two?", "options": [{"label": "B"}]}]}));
    assert!(!two.single_tap);
    let many = pending(
        json!({"questions": [{"question": "Which?", "options": ["A", "B", "C", "D", "E"]}]}),
    );
    assert!(!many.single_tap);
    let four =
        pending(json!({"questions": [{"question": "Which?", "options": ["A", "B", "C", "D"]}]}));
    assert!(four.single_tap);
    let none = pending(json!({"questions": [{"question": "Free text?", "options": []}]}));
    assert!(!none.single_tap);
}

#[test]
fn rows_say_what_voice_over_reads() {
    let mut titled = session(Phase::Processing, None, None, None, None);
    titled.apply_title(Some("Fix the bug"), SessionTitleSource::Hook);
    titled.context_used_percent = Some(40.0);
    let view = view_of(&titled);
    let mut ctx = RowContext::new(now());
    ctx.account_label = Some("Work");
    ctx.can_focus = true;
    let row = session_row(&view, &ctx);
    assert!(row.a11y.contains("Fix the bug"));
    assert!(row.a11y.contains("account Work"));
    assert_eq!(row.focus_label.as_deref(), Some("Show terminal"));
    assert_eq!(row.project.as_deref(), Some("acme"));
    assert_eq!(row.context_pct, Some(40.0));
    // No window to jump to: no label.
    ctx.can_focus = false;
    assert_eq!(session_row(&view, &ctx).focus_label, None);
    // VS Code sessions open the editor.
    let mut editor = view.clone();
    editor.entrypoint = Some("claude-vscode".into());
    ctx.can_focus = true;
    assert_eq!(
        session_row(&editor, &ctx).focus_label.as_deref(),
        Some("Show in editor")
    );
}

#[test]
fn voice_over_never_reads_the_assistants_message() {
    let mut review = session(Phase::WaitingForInput, None, None, None, None);
    review.completed_at = Some(ago(60));
    review.last_assistant_message = Some("SECRET reply text".into());
    let label = accessibility_label(&view_of(&review), None, None, now(), clock());
    assert!(!label.contains("SECRET"), "{label}");
    assert!(label.contains("Ready for review"), "{label}");
    assert!(label.contains("finished 1m ago"), "{label}");
}

#[test]
fn voice_over_orders_title_state_detail_time_tasks_account() {
    let mut working = working_with_task("Running tests");
    working.apply_title(Some("Fix CI"), SessionTitleSource::Hook);
    working.turn_started_at = Some(ago(125));
    let view = view_of(&working);
    assert_eq!(
        accessibility_label(&view, Some("Work"), None, now(), clock()),
        "Fix CI, Working, Running tests, running 2m, 1 of 2 tasks done, now: Running tests, account Work"
    );
    assert_eq!(
        task_summary(&TaskProgress {
            done: 3,
            total: 7,
            active_label: None,
            items: vec![]
        }),
        "3 of 7 tasks done"
    );
    // A waiting session says how long and what it asks.
    let waiting = session(
        approval("Bash", json!({"command": "npm test"}), vec![], 150),
        None,
        None,
        None,
        None,
    );
    let text = accessibility_label(&view_of(&waiting), None, None, now(), clock());
    assert!(
        text.contains("Needs you, Bash npm test, waiting 2m"),
        "{text}"
    );
    // A one-line row names the project when there is nothing to say.
    let idle = view_of(&plain());
    assert_eq!(compact_detail(&idle, None, now(), clock()), "acme");
    assert_eq!(compact_detail(&view, None, now(), clock()), "Running tests");
}

// ---- A3_ActivityRowsTests, as the Windows card ----

/// A3_ActivityRowsTests.state: private text that must never reach a card.
fn card_session(phase: Phase, reason: Option<NeedsInputReason>) -> Session {
    let mut s = Session::new("s1", "/Users/me/code/acme-web", ago(60));
    s.apply_title(Some("Fix the login loop"), SessionTitleSource::Hook);
    s.phase = phase;
    s.set_needs_input(reason, ago(60));
    s.turn_started_at = Some(ago(600));
    s.pid = Some(4242);
    s.last_assistant_message = Some("SECRET-ASSISTANT".into());
    s.conversation_info = ConversationInfo {
        last_message: Some("SECRET-LAST".into()),
        last_message_role: Some("assistant".into()),
        first_user_message: Some("SECRET-PROMPT".into()),
        ..ConversationInfo::default()
    };
    s
}

fn card_of(s: &Session, host: Option<&str>) -> agentnotch_engine::model::ui::CardRow {
    let mut ctx = RowContext::new(now());
    ctx.host_app = host;
    card_row(&view_of(s), &ctx)
}

fn expect_private(card: &agentnotch_engine::model::ui::CardRow) {
    for text in [
        card.name.as_str(),
        card.detail.as_deref().unwrap_or_default(),
        card.waiting_for.as_deref().unwrap_or_default(),
    ] {
        assert!(!text.contains("SECRET"), "{text}");
    }
}

#[test]
fn a_permission_card_names_the_tool_and_never_its_input() {
    let command = "npm run test -- --watch=false auth/redirect.spec.ts --reporter=dot";
    let s = card_session(
        approval("Bash", json!({"command": command}), vec![], 120),
        None,
    );
    let card = card_of(&s, Some("iTerm2"));
    assert_eq!(card.state, "needs_you");
    assert_eq!(card.waiting_for.as_deref(), Some("Approve Bash"));
    assert_eq!(card.name, "iTerm2 · acme-web");
    assert_eq!(
        card.since_ms,
        agentnotch_engine::core::time::to_ms(ago(120))
    );
    assert!(!card.waiting_for.unwrap().contains("npm"));
    expect_private(&card_of(&s, Some("iTerm2")));
}

#[test]
fn mcp_tools_and_unnamed_prompts_read_well() {
    let mcp = card_session(
        approval(
            "mcp__github__create_issue",
            json!({"title": "Bug"}),
            vec![],
            120,
        ),
        None,
    );
    assert_eq!(
        card_of(&mcp, None).waiting_for.as_deref(),
        Some("Approve Github - Create Issue")
    );
    // A permission prompt seen only as a notification has no tool.
    let bare = card_session(
        Phase::Processing,
        Some(NeedsInputReason::Permission {
            tool: Some(String::new()),
        }),
    );
    assert_eq!(
        card_of(&bare, None).waiting_for.as_deref(),
        Some("Needs permission")
    );
}

#[test]
fn a_question_card_shows_its_header_never_the_question() {
    let input = json!({"questions": [{"question": "SECRET-QUESTION-TEXT?", "header": "Charts",
        "options": [{"label": "A"}]}]});
    let s = card_session(approval("AskUserQuestion", input, vec![], 120), None);
    let card = card_of(&s, Some("iTerm2"));
    assert_eq!(card.state, "needs_you");
    assert_eq!(card.waiting_for.as_deref(), Some("Question · Charts"));
    expect_private(&card);
    let bare = card_session(approval("AskUserQuestion", Value::Null, vec![], 120), None);
    assert_eq!(
        card_of(&bare, None).waiting_for.as_deref(),
        Some("Question")
    );
}

#[test]
fn plan_elicitation_and_dialog_cards() {
    let plan = card_session(
        approval("ExitPlanMode", json!({"plan": "SECRET-PLAN"}), vec![], 120),
        None,
    );
    let card = card_of(&plan, None);
    assert_eq!(card.waiting_for.as_deref(), Some("Plan ready for approval"));
    expect_private(&card);
    let elicitation = card_session(
        Phase::Processing,
        Some(NeedsInputReason::Elicitation {
            message: "Figma needs you to pick a file".into(),
        }),
    );
    assert_eq!(
        card_of(&elicitation, None).waiting_for.as_deref(),
        Some("Figma needs you to pick a file")
    );
    let dialog = card_session(
        Phase::Processing,
        Some(NeedsInputReason::Dialog {
            detail: "worker permission".into(),
        }),
    );
    let card = card_of(&dialog, None);
    assert_eq!(card.waiting_for.as_deref(), Some("Worker permission"));
    // Without an approval the last hook event is when it started waiting.
    assert_eq!(card.since_ms, agentnotch_engine::core::time::to_ms(ago(60)));
}

/// GUX-2: a failed turn is not an amber wait: its own state, and only the
/// reason in words.
#[test]
fn a_failed_turn_is_its_own_state() {
    let limited = card_session(Phase::WaitingForInput, Some(rate_limited()));
    let card = card_of(&limited, Some("iTerm2"));
    assert_eq!(card.state, "failed");
    assert_eq!(card.waiting_for.as_deref(), Some("Rate limited"));
    let overloaded = card_session(Phase::WaitingForInput, Some(error("Overloaded")));
    assert_eq!(
        card_of(&overloaded, None).waiting_for.as_deref(),
        Some("Overloaded")
    );
}

#[test]
fn a_working_card_shows_tasks_tool_or_just_the_context() {
    let mut with_tasks = card_session(Phase::Processing, None);
    with_tasks.tasks.todos_replaced(
        std::iter::repeat_n(("done".to_owned(), "completed".to_owned(), None), 3)
            .chain([("Writing tests".to_owned(), "in_progress".to_owned(), None)])
            .chain(std::iter::repeat_n(
                ("later".to_owned(), "pending".to_owned(), None),
                4,
            ))
            .collect(),
    );
    with_tasks.context_used_percent = Some(42.4);
    let card = card_of(&with_tasks, Some("iTerm2"));
    assert_eq!(card.state, "working");
    assert_eq!(card.detail.as_deref(), Some("3/8 Writing tests · ctx 42%"));
    assert_eq!(card.waiting_for, None);
    assert_eq!(
        card.since_ms,
        agentnotch_engine::core::time::to_ms(ago(600))
    );
    expect_private(&card);

    // The newest tool started, in its display name.
    let mut tool = card_session(Phase::Processing, None);
    tool.tool_tracker.start_tool("a", "Read", None, ago(20));
    tool.tool_tracker
        .start_tool("b", "mcp__github__list_issues", None, ago(10));
    assert_eq!(
        agentnotch_engine::attention::rows::running_tool(&view_of(&tool)).as_deref(),
        Some("Github - List Issues")
    );
    assert_eq!(
        card_of(&tool, None).detail.as_deref(),
        Some("Github - List Issues…")
    );
    // Neither tasks nor a tool: only the context (the state word says the rest).
    let mut thinking = card_session(Phase::Processing, None);
    thinking.context_used_percent = Some(7.0);
    assert_eq!(card_of(&thinking, None).detail.as_deref(), Some("ctx 7%"));
    assert_eq!(
        card_of(&card_session(Phase::Processing, None), None).detail,
        None
    );
}

#[test]
fn a_card_waiting_on_background_agents_says_so() {
    let mut s = card_session(Phase::Idle, None);
    s.background_wait_since = Some(ago(30));
    s.background_agent_count = 1;
    s.background_agent_types = vec!["workflow".into()];
    s.background_task_count = 1;
    s.tasks.todos_replaced(vec![
        ("a".into(), "completed".into(), None),
        ("b".into(), "pending".into(), None),
    ]);
    s.context_used_percent = Some(42.0);
    let view = view_of(&s);
    assert_eq!(view.state, SessionState::Working);
    let card = card_row(&view, &RowContext::new(now()));
    let detail = card.detail.unwrap();
    assert!(detail.starts_with("Waiting on "), "{detail}");
    assert!(detail.ends_with("1/2 · ctx 42%"), "{detail}");
}

#[test]
fn a_review_card_counts_background_work_and_waits_until_reviewed() {
    let mut done = card_session(Phase::WaitingForInput, None);
    done.completed_at = Some(ago(30));
    done.background_task_count = 2;
    let card = card_of(&done, Some("iTerm2"));
    assert_eq!(card.state, "review");
    assert_eq!(card.detail.as_deref(), Some("2 background"));
    assert_eq!(card.since_ms, agentnotch_engine::core::time::to_ms(ago(30)));
    expect_private(&card);

    done.reviewed_at = Some(now());
    assert_eq!(card_of(&done, None).state, "idle");
}

#[test]
fn an_idle_card_says_where_it_runs() {
    let idle = card_session(Phase::Idle, None);
    let card = card_of(&idle, Some("VS Code"));
    assert_eq!(card.state, "idle");
    assert_eq!(card.name, "VS Code · acme-web");
    assert_eq!(card.since_ms, agentnotch_engine::core::time::to_ms(ago(60)));
    // The host unknown: the public title, never the first prompt.
    assert_eq!(card_of(&idle, None).name, "Fix the login loop");
    let untitled = Session::new("s2", "/Users/me/code/acme-web", ago(60));
    assert_eq!(card_of(&untitled, None).name, "acme-web");
}

/// A3_ReviewTests.rowsAndBannersCarryNoPromptOrAssistantText: the card never
/// carries a prompt or a reply, whatever the state.
#[test]
fn cards_carry_no_prompt_or_assistant_text() {
    let mut review = card_session(Phase::WaitingForInput, None);
    review.completed_at = Some(ago(30));
    let states = [
        card_session(Phase::Idle, None),
        card_session(Phase::Processing, None),
        review,
        card_session(approval("Bash", json!({"command": "ls"}), vec![], 10), None),
        card_session(Phase::Idle, Some(rate_limited())),
    ];
    for s in &states {
        expect_private(&card_of(s, Some("iTerm2")));
        expect_private(&card_of(s, None));
        let row = session_row(&view_of(s), &RowContext::new(now()));
        assert!(!row.card.name.contains("SECRET"));
    }
}

// ---- the UI contract's snapshot ----

struct Spec {
    id: &'static str,
    ring: &'static str,
    label: &'static str,
    color: u8,
    host: &'static str,
    vscode: bool,
    can_message: bool,
    can_focus: bool,
}

const PERSONAL: (&str, &str, u8) = ("claude-acct-1e41d94e802a", "Personal", 0);
const WORK: (&str, &str, u8) = ("claude-acct-5688209c6cfb", "Work", 3);

fn spec(id: &'static str, account: (&'static str, &'static str, u8), host: &'static str) -> Spec {
    Spec {
        id,
        ring: account.0,
        label: account.1,
        color: account.2,
        host,
        vscode: host == "VS Code",
        can_message: id != "needs-question",
        can_focus: true,
    }
}

/// A session of the fixture: its record, from the inputs the fixture's texts
/// came from (the Mac's SampleSessions).
fn fixture_session(
    spec: &Spec,
    title: &str,
    project: &str,
    ctx: Option<f64>,
    apply: impl FnOnce(&mut Session),
) -> (Spec, SessionView) {
    let mut s = Session::new(spec.id, format!("/home/me/{project}"), ago(0));
    s.apply_title(Some(title), SessionTitleSource::Hook);
    s.context_used_percent = ctx;
    s.ring = Some(RingId::new(spec.ring));
    if spec.vscode {
        s.entrypoint = Some("claude-vscode".into());
    }
    apply(&mut s);
    let view = s.to_view();
    (
        Spec {
            id: spec.id,
            ring: spec.ring,
            label: spec.label,
            color: spec.color,
            host: spec.host,
            vscode: spec.vscode,
            can_message: spec.can_message,
            can_focus: spec.can_focus,
        },
        view,
    )
}

fn generated_at() -> SystemTime {
    from_ms(1_790_000_000_000)
}

fn before(secs: u64) -> SystemTime {
    generated_at() - Duration::from_secs(secs)
}

fn bash_approval(received: SystemTime) -> Phase {
    Phase::WaitingForApproval(PermissionContext {
        tool_use_id: "toolu_sample_bash".into(),
        tool_name: "Bash".into(),
        tool_input: json!({"command": "npm run test -- --watch=false auth/redirect.spec.ts"}),
        received_at: received,
        permission_suggestions: vec![json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash", "ruleContent": "npm run test:*"}],
            "behavior": "allow",
            "destination": "localSettings"
        })],
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: None,
    })
}

fn special_approval(id: &str, tool: &str, input: Value, received: SystemTime) -> Phase {
    Phase::WaitingForApproval(PermissionContext {
        tool_use_id: id.into(),
        tool_name: tool.into(),
        tool_input: input,
        received_at: received,
        permission_suggestions: vec![],
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: None,
    })
}

fn fixture_views() -> Vec<(Spec, SessionView)> {
    let last = |s: &mut Session, secs: u64| s.last_activity = before(secs);
    vec![
        fixture_session(
            &spec("needs-permission", WORK, "Windows Terminal"),
            "Fix the login redirect loop",
            "acme-web",
            Some(42.0),
            |s| {
                s.phase = bash_approval(before(120));
                last(s, 120);
            },
        ),
        fixture_session(
            &spec("needs-question", PERSONAL, "VS Code"),
            "Pick a charting library",
            "dashboard",
            Some(18.0),
            |s| {
                s.phase = special_approval(
                    "toolu_sample_question",
                    "AskUserQuestion",
                    json!({"questions": [{
                    "question": "Which charting library should the dashboard use?",
                    "header": "Charts", "multiSelect": false,
                    "options": [
                        {"label": "Recharts", "description": "Composable React components"},
                        {"label": "Chart.js", "description": "Canvas, small bundle"},
                        {"label": "ECharts", "description": "Feature-rich, larger bundle"}]}]}),
                    before(360),
                );
                last(s, 360);
            },
        ),
        fixture_session(
            &spec("needs-plan", PERSONAL, "Console"),
            "Migrate settings storage to SQLite",
            "notes-app",
            Some(61.0),
            |s| {
                s.phase = special_approval(
                    "toolu_sample_plan",
                    "ExitPlanMode",
                    json!({"plan": "## Plan\n\n1. Add the models\n2. Migrate the stored settings\n3. Remove the old store"}),
                    before(660),
                );
                last(s, 660);
            },
        ),
        fixture_session(
            &spec("needs-elicitation", PERSONAL, "Windows Terminal"),
            "Sync the design tokens",
            "design-system",
            Some(33.0),
            |s| {
                s.phase = Phase::WaitingForInput;
                s.set_needs_input(
                    Some(NeedsInputReason::Elicitation {
                        message: "Figma needs you to pick a file".into(),
                    }),
                    before(300),
                );
                last(s, 300);
            },
        ),
        fixture_session(
            &spec("needs-ratelimit", WORK, "Windows Terminal"),
            "Refactor the billing webhooks",
            "billing-service",
            Some(57.0),
            |s| {
                s.phase = Phase::WaitingForInput;
                s.set_needs_input(Some(rate_limited()), before(540));
                last(s, 540);
            },
        ),
        fixture_session(
            &spec("review-just-finished", PERSONAL, "Windows Terminal"),
            "Fix the flaky date test",
            "billing-service",
            Some(27.0),
            |s| {
                s.phase = Phase::WaitingForInput;
                s.completed_at = Some(before(20));
                s.last_assistant_message = Some("The test pinned the time zone to UTC; it now uses a fixed calendar and passes 50 runs in a row.".into());
                last(s, 20);
            },
        ),
        fixture_session(
            &spec("review-darkmode", PERSONAL, "Console"),
            "Add a dark mode toggle to settings",
            "acme-web",
            Some(38.0),
            |s| {
                s.phase = Phase::WaitingForInput;
                s.completed_at = Some(before(300));
                s.last_assistant_message = Some("Added a Dark mode toggle under Settings › Appearance. It follows the system by default, persists the choice, and all 42 tests pass.".into());
                last(s, 300);
            },
        ),
        fixture_session(
            &spec("review-devserver", WORK, "Windows Terminal"),
            "Set up the local dev server",
            "acme-web",
            Some(22.0),
            |s| {
                s.phase = Phase::WaitingForInput;
                s.completed_at = Some(before(720));
                s.background_task_count = 2;
                s.last_assistant_message = Some("The dev server is up on http://localhost:5173 with hot reload. I left the type checker watching in the background.".into());
                last(s, 720);
            },
        ),
        fixture_session(
            &spec("work-migration", WORK, "Windows Terminal"),
            "Write migration tests for the v2 schema",
            "billing-service",
            Some(84.0),
            |s| {
                s.phase = Phase::Processing;
                s.turn_started_at = Some(before(840));
                last(s, 840);
            },
        ),
        fixture_session(
            &spec("work-ci", PERSONAL, "VS Code"),
            "Investigate the flaky CI job",
            "infra",
            Some(93.0),
            |s| {
                s.phase = Phase::Processing;
                s.turn_started_at = Some(before(480));
                s.conversation_info.last_message_role = Some("tool".into());
                s.conversation_info.last_tool_name = Some("Grep".into());
                s.conversation_info.last_message = Some("ETIMEDOUT|socket hang up".into());
                last(s, 480);
            },
        ),
        fixture_session(
            &spec("work-summary", WORK, "Windows Terminal"),
            "Summarize the PR review feedback",
            "acme-web",
            Some(12.0),
            |s| {
                s.phase = Phase::Processing;
                s.turn_started_at = Some(before(40));
                last(s, 40);
            },
        ),
        fixture_session(
            &spec("idle-notch", PERSONAL, "Windows Terminal"),
            "Explore the notch APIs",
            "agent-notch",
            Some(7.0),
            |s| {
                s.conversation_info.last_message_role = Some("assistant".into());
                s.conversation_info.last_message =
                    Some("The work area excludes the taskbar, so the notch sits below it.".into());
                last(s, 3_000);
            },
        ),
        fixture_session(
            &spec("idle-readme", WORK, "Windows Terminal"),
            "Tidy up the README",
            "acme-web",
            None,
            |s| {
                s.conversation_info.last_message_role = Some("user".into());
                s.conversation_info.last_message = Some("thanks, that's all for now".into());
                last(s, 10_800);
            },
        ),
        fixture_session(
            &spec("idle-deps", WORK, "Windows Terminal"),
            "Bump dependencies",
            "billing-service",
            None,
            |s| {
                s.conversation_info.last_message_role = Some("tool".into());
                s.conversation_info.last_tool_name = Some("Bash".into());
                s.conversation_info.last_message = Some("npm outdated".into());
                last(s, 18_000);
            },
        ),
        {
            let mut idle_logo = spec("idle-logo", PERSONAL, "Windows Terminal");
            idle_logo.can_message = false;
            idle_logo.can_focus = false;
            fixture_session(&idle_logo, "Draft a new logo brief", "brand", None, |s| {
                last(s, 93_600);
            })
        },
    ]
}

fn fixture() -> Value {
    serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/ui-contract/snapshot.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

/// Rows built from the fixture's inputs reproduce the fixture: every field of
/// every row, down to the card and the VoiceOver sentence.
#[test]
fn rows_built_from_the_snapshot_inputs_reproduce_the_snapshot() {
    let fixture = fixture();
    assert_eq!(fixture["generated_at_ms"], 1_790_000_000_000u64);
    let expected = fixture["sessions"].as_array().unwrap();
    let views = fixture_views();
    assert_eq!(views.len(), expected.len());

    // The one limit the fixture shows: Work's 5-hour window, spent, resetting
    // in 40 minutes.
    let work_limit = RateLimitReset {
        window: "5-hour limit".into(),
        resets_at: generated_at() + Duration::from_secs(2_400),
    };

    let mut mismatches = Vec::new();
    for (spec, mut view) in views {
        let want = expected
            .iter()
            .find(|row| row["session_id"] == spec.id)
            .unwrap_or_else(|| panic!("{} is not in the fixture", spec.id));
        // The task list comes from the fixture: its ids and labels are the
        // sample's own.
        if !want["tasks"].is_null() {
            view.tasks = Some(serde_json::from_value(want["tasks"].clone()).unwrap());
        }
        let mut ctx = RowContext::new(generated_at());
        ctx.account_label = Some(spec.label);
        ctx.account_color = Some(spec.color);
        ctx.host_app = Some(spec.host);
        ctx.can_focus = spec.can_focus;
        ctx.can_message = spec.can_message;
        ctx.rate_limit = Some(&work_limit);
        let row = session_row(&view, &ctx);
        assert_eq!(view.ring.as_ref().map(|r| r.as_str()), Some(spec.ring));
        let got = serde_json::to_value(&row).unwrap();
        for (key, value) in want.as_object().unwrap() {
            if &got[key] != value {
                mismatches.push(format!(
                    "{} {key}:\n  fixture {value}\n  rows    {}",
                    spec.id, got[key]
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
