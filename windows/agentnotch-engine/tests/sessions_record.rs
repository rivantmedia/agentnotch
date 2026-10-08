//! The session record (SessionState.swift's port): titles, the tool
//! tracker, bounded agent bookkeeping, quiet completions, pending requests
//! and the view, ported from SessionCoreRegressionTests,
//! A1_SessionStoreRegressionTests, A1_ReviewFixesTests, A3_ReviewTests and
//! Fix_QuietCompletionTests (the rule itself; the store drives it in wp5-7).

use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{
    AccountId, Attribution, HookTerminal, IdentityId, NeedsInputReason, PermissionContext, Phase,
    RequestKind, RingId, SessionState, SessionView,
};
use agentnotch_engine::sessions::session::{
    describe_suggestion, is_too_long_to_review_inline, parse_questions, permission_preview,
    Session, SessionTitleSource, SettledAgents, SubagentState, SubagentToolCall, ToolPhase,
    ToolTracker,
};
use agentnotch_engine::sessions::summary::ConversationInfo;
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_000_000 + secs)
}

fn session() -> Session {
    Session::new("s1", "/tmp/proj", at(0))
}

fn info(first_user_message: Option<&str>, summary: Option<&str>) -> ConversationInfo {
    ConversationInfo {
        first_user_message: first_user_message.map(str::to_owned),
        summary: summary.map(str::to_owned),
        ..ConversationInfo::default()
    }
}

fn context(tool: &str, input: Value, suggestions: Vec<Value>) -> PermissionContext {
    PermissionContext {
        tool_use_id: format!("toolu_{tool}"),
        tool_name: tool.into(),
        tool_input: input,
        received_at: at(5),
        permission_suggestions: suggestions,
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: None,
    }
}

// ---- SessionCoreRegressionTests ----

#[test]
fn derived_registry_names_rank_below_transcript_titles() {
    let mut state = session();
    state.apply_name(Some("proj-3"), true);
    assert_eq!(state.display_title(), "proj-3");

    // A first prompt says more than a made-up name...
    state.conversation_info = info(Some("Fix the login flow"), None);
    assert_eq!(state.display_title(), "Fix the login flow");
    // ...and an AI title replaces it.
    state.apply_title(Some("Login flow fix"), SessionTitleSource::Transcript);
    assert_eq!(state.display_title(), "Login flow fix");
    // The status line repeating the derived name changes nothing.
    state.apply_name(Some("proj-3"), false);
    assert_eq!(state.display_title(), "Login flow fix");
    // A name the user chose wins.
    state.apply_name(Some("Auth rewrite"), false);
    assert_eq!(state.display_title(), "Auth rewrite");
}

#[test]
fn derived_name_seen_first_on_the_status_line_is_downgraded() {
    let mut state = session();
    state.apply_name(Some("proj-3"), false); // status line, source unknown yet
    assert_eq!(state.title_source, Some(SessionTitleSource::Registry));
    state.apply_name(Some("proj-3"), true); // registry: it was derived
    assert_eq!(state.title_source, Some(SessionTitleSource::DerivedName));
    state.apply_title(Some("Login flow fix"), SessionTitleSource::Transcript);
    assert_eq!(state.display_title(), "Login flow fix");
}

#[test]
fn title_sources_rank_and_blank_titles_change_nothing() {
    let mut state = session();
    assert!(state.apply_title(Some("From the hook"), SessionTitleSource::Hook));
    // A lower source never overwrites a higher one.
    assert!(!state.apply_title(Some("From the registry"), SessionTitleSource::Registry));
    assert!(!state.apply_title(Some("From the transcript"), SessionTitleSource::Transcript));
    assert_eq!(state.display_title(), "From the hook");
    // The same title from the same source is no change; blanks are ignored.
    assert!(!state.apply_title(Some("  From the hook \n"), SessionTitleSource::Hook));
    assert!(!state.apply_title(Some("   "), SessionTitleSource::Hook));
    assert!(!state.apply_title(None, SessionTitleSource::Hook));
    assert!(!state.apply_name(Some(" "), false));
    // An equal or higher source replaces.
    assert!(state.apply_title(Some("Renamed"), SessionTitleSource::Hook));
    assert_eq!(state.session_title.as_deref(), Some("Renamed"));
    assert!(SessionTitleSource::DerivedName < SessionTitleSource::Transcript);
    assert!(SessionTitleSource::Transcript < SessionTitleSource::Registry);
    assert!(SessionTitleSource::Registry < SessionTitleSource::Hook);
}

// ---- A1_SessionStoreRegressionTests ----

#[test]
fn the_tool_tracker_keeps_only_the_newest_calls() {
    let mut tracker = ToolTracker::new();
    for index in 0..(ToolTracker::MAX_IN_PROGRESS + 10) {
        tracker.start_tool(&format!("t{index}"), "Read", Some("a"), at(index as u64));
    }
    assert_eq!(tracker.len(), ToolTracker::MAX_IN_PROGRESS);
    assert!(!tracker.contains("t0"));
    assert_eq!(
        tracker.newest().map(|tool| tool.id.as_str()),
        Some(format!("t{}", ToolTracker::MAX_IN_PROGRESS + 9).as_str())
    );
    // A repeated start doesn't move a call.
    tracker.start_tool("t20", "Edit", None, at(999));
    assert_eq!(
        tracker.get("t20").map(|tool| tool.name.as_str()),
        Some("Read")
    );
}

#[test]
fn the_tool_tracker_ends_a_main_turn_but_not_the_background_agents() {
    let mut tracker = ToolTracker::new();
    tracker.start_tool("main", "Bash", None, at(1));
    tracker.start_tool("agent", "Read", Some("agent-1"), at(2));
    tracker.set_phase(ToolPhase::PendingApproval, "main");
    assert_eq!(
        tracker.get("main").unwrap().phase,
        ToolPhase::PendingApproval
    );
    tracker.set_phase(ToolPhase::Running, "unknown"); // no such call: nothing
    tracker.end_main_turn();
    assert!(!tracker.contains("main"));
    assert!(tracker.contains("agent"));
    tracker.complete_tool("agent");
    assert!(tracker.is_empty());
    assert!(tracker.newest().is_none());
}

// ---- A1_ReviewFixesTests ----

#[test]
fn settled_agents_are_bounded() {
    let mut settled = SettledAgents::new();
    for index in 0..5000 {
        settled.settle(&format!("toolu_agent_{index}"));
    }
    assert!(settled.len() <= SettledAgents::MAX * 2);
    assert!(settled.contains("toolu_agent_4999"));
    assert!(!settled.contains("toolu_agent_0"));
    // Settling twice is one entry.
    let before = settled.len();
    settled.settle("toolu_agent_4999");
    assert_eq!(settled.len(), before);
}

#[test]
fn subagent_state_tracks_tasks_and_their_tools() {
    let tool = |id: &str, secs: u64| SubagentToolCall {
        id: id.into(),
        name: "Read".into(),
        input: Default::default(),
        status: "running".into(),
        timestamp: at(secs),
    };
    let mut state = SubagentState::new();
    // No active task: a subagent tool has nowhere to go.
    state.add_subagent_tool(tool("orphan", 1));
    assert!(!state.has_active_subagent());

    state.start_task("task-a", Some("first"), at(10));
    state.start_task("task-b", None, at(20));
    assert!(state.has_active_subagent());
    // The most recent active Task takes the tool.
    state.add_subagent_tool(tool("t1", 21));
    assert_eq!(state.active_tasks["task-b"].subagent_tools.len(), 1);
    assert!(state.active_tasks["task-a"].subagent_tools.is_empty());
    // Its status changes wherever it is.
    state.update_subagent_tool_status("t1", "success");
    assert_eq!(
        state.active_tasks["task-b"].subagent_tools[0].status,
        "success"
    );
    state.stop_task("task-b");
    state.add_subagent_tool(tool("t2", 22));
    assert_eq!(state.active_tasks["task-a"].subagent_tools[0].id, "t2");
    state.stop_task("task-a");
    assert!(!state.has_active_subagent());
}

// ---- A3_ReviewTests: titlesNeverFallBackToThePrompt ----

fn titled(title: Option<&str>, source: SessionTitleSource, summary: Option<&str>) -> Session {
    let mut state = Session::new("s1", "/Users/me/code/acme-web", at(0));
    state.phase = Phase::WaitingForInput;
    if let Some(title) = title {
        state.apply_title(Some(title), source);
    }
    state.conversation_info = ConversationInfo {
        summary: summary.map(str::to_owned),
        last_message: Some("SECRET-LAST".into()),
        last_message_role: Some("assistant".into()),
        first_user_message: Some("SECRET-PROMPT please fix my login".into()),
        ..ConversationInfo::default()
    };
    state.last_assistant_message = Some("SECRET-ASSISTANT all done".into());
    state
}

#[test]
fn titles_never_fall_back_to_the_prompt() {
    let hook = SessionTitleSource::Hook;
    // What the panel would show before Claude Code names the session.
    assert!(titled(None, hook, None)
        .display_title()
        .contains("SECRET-PROMPT"));
    assert_eq!(titled(None, hook, None).public_title(), "acme-web");
    assert_eq!(
        titled(Some("Fix the login loop"), hook, None).public_title(),
        "Fix the login loop"
    );
    assert_eq!(
        titled(None, hook, Some("Login redirect fix")).public_title(),
        "Login redirect fix"
    );
    // A name derived from the folder loses to a transcript title, and still
    // beats the bare folder.
    let derived = SessionTitleSource::DerivedName;
    assert_eq!(
        titled(Some("acme"), derived, Some("Login fix")).public_title(),
        "Login fix"
    );
    assert_eq!(titled(Some("acme"), derived, None).public_title(), "acme");
    assert_eq!(
        titled(Some("  Two\n lines "), hook, None).public_title(),
        "Two lines"
    );
}

#[test]
fn a_title_made_from_the_folder_is_flagged_for_the_cloud() {
    let hook = SessionTitleSource::Hook;
    assert!(titled(None, hook, None).title_from_folder());
    assert!(titled(Some("acme"), SessionTitleSource::DerivedName, None).title_from_folder());
    assert!(!titled(Some("Fix the login loop"), hook, None).title_from_folder());
    assert!(!titled(None, hook, Some("Login redirect fix")).title_from_folder());
    assert!(!titled(
        Some("acme"),
        SessionTitleSource::DerivedName,
        Some("Login fix")
    )
    .title_from_folder());
    // The view carries it, and no secret leaves through the public fields.
    let view = titled(None, hook, None).to_view();
    assert!(view.title_from_folder);
    assert_eq!(view.public_title, "acme-web");
}

#[test]
fn the_display_title_follows_its_order() {
    let mut state = session();
    assert_eq!(state.display_title(), "proj");
    state.conversation_info = info(Some("First prompt"), None);
    assert_eq!(state.display_title(), "First prompt");
    state.conversation_info = info(Some("First prompt"), Some("The summary"));
    assert_eq!(state.display_title(), "The summary");
    state.apply_title(Some("The hook title"), SessionTitleSource::Hook);
    assert_eq!(state.display_title(), "The hook title");
}

// ---- quiet completions (HS 548-555; Fix_QuietCompletionTests) ----

struct Turn {
    source: &'static str,
    user_authored: bool,
    wakeups_at_start: u32,
    wakeups: u32,
    agents_at_start: u32,
    agents: u32,
}

fn quiet(turn: Turn) -> bool {
    let mut state = session();
    state.last_prompt_source = Some(turn.source.into());
    state.last_prompt_was_user_authored = turn.user_authored;
    state.wakeups_at_turn_start = turn.wakeups_at_start;
    state.scheduled_wakeup_count = turn.wakeups;
    state.agents_at_turn_start = turn.agents_at_start;
    state.background_agent_count = turn.agents;
    state.completion_is_quiet()
}

const TYPED: Turn = Turn {
    source: "user",
    user_authored: true,
    wakeups_at_start: 0,
    wakeups: 0,
    agents_at_start: 0,
    agents: 0,
};

#[test]
fn the_quiet_completion_table() {
    // Nothing waiting: announced.
    assert!(!quiet(TYPED));
    // 1. A loop or schedule tick is always quiet.
    for source in ["loop_wakeup", "schedule_wakeup"] {
        assert!(quiet(Turn {
            source,
            user_authored: false,
            ..TYPED
        }));
    }
    // 2. The turn scheduled a wake-up of its own.
    assert!(quiet(Turn {
        wakeups: 1,
        ..TYPED
    }));
    assert!(quiet(Turn {
        wakeups_at_start: 1,
        wakeups: 2,
        ..TYPED
    }));
    // A cron that was already there makes nothing quiet (a typed turn in a
    // /loop session is announced).
    assert!(!quiet(Turn {
        wakeups_at_start: 1,
        wakeups: 1,
        ..TYPED
    }));
    assert!(!quiet(Turn {
        wakeups_at_start: 2,
        wakeups: 0,
        ..TYPED
    }));
    // 3. A typed turn is quiet when it started agents.
    assert!(quiet(Turn { agents: 2, ..TYPED }));
    assert!(!quiet(Turn {
        agents_at_start: 2,
        agents: 2,
        ..TYPED
    }));
    // 4. A turn the system started is quiet while any waking agent is out.
    let system = Turn {
        source: "system",
        user_authored: false,
        ..TYPED
    };
    assert!(quiet(Turn {
        agents: 1,
        ..system
    }));
    assert!(!quiet(Turn {
        source: "system",
        user_authored: false,
        ..TYPED
    }));
}

#[test]
fn a_loop_session_across_turns() {
    // aTypedTurnInALoopSessionIsAnnounced, as the fields the store sets.
    assert!(quiet(Turn {
        wakeups: 1,
        ..TYPED
    })); // "/loop 5m check the build": the cron is new
    assert!(quiet(Turn {
        source: "schedule_wakeup",
        user_authored: false,
        wakeups_at_start: 1,
        wakeups: 1,
        ..TYPED
    })); // a tick
    assert!(!quiet(Turn {
        wakeups_at_start: 1,
        wakeups: 1,
        ..TYPED
    })); // "fix the lint error"
    assert!(quiet(Turn {
        wakeups_at_start: 1,
        wakeups: 2,
        ..TYPED
    })); // "check back in 10 min"
    assert!(!quiet(Turn {
        wakeups_at_start: 2,
        wakeups: 0,
        ..TYPED
    })); // "stop the loop"
}

// ---- needs input, attention, waiting ----

#[test]
fn needs_input_keeps_its_first_time() {
    let mut state = session();
    assert_eq!(state.needs_input_since(), None);
    state.set_needs_input(Some(NeedsInputReason::Question), at(10));
    assert_eq!(state.needs_input_since(), Some(at(10)));
    // A reason that only changes keeps the original time.
    state.set_needs_input(Some(NeedsInputReason::PlanApproval), at(20));
    assert_eq!(state.needs_input_since(), Some(at(10)));
    assert_eq!(state.waiting_since(), Some(at(10)));
    // Clearing forgets it; the next one starts again.
    state.set_needs_input(None, at(30));
    assert_eq!(state.needs_input_since(), None);
    assert!(state.needs_input_reason().is_none());
    state.set_needs_input(Some(NeedsInputReason::Question), at(40));
    assert_eq!(state.needs_input_since(), Some(at(40)));
}

#[test]
fn failed_turns_and_their_kind() {
    let mut state = session();
    assert!(!state.has_failed_turn());
    assert_eq!(state.stop_error_kind(), None);
    state.stop_error = Some("Rate limited".into());
    state.stop_error_code = Some("rate_limit".into());
    // The error text alone is not a blocked session.
    assert_eq!(state.stop_error_kind(), None);
    state.set_needs_input(
        Some(NeedsInputReason::Error {
            text: "Rate limited".into(),
            code: Some("rate_limit".into()),
        }),
        at(3),
    );
    assert!(state.has_failed_turn());
    assert!(state.stop_error_kind().is_some());
    assert!(matches!(state.attention(), SessionState::Failed(_)));
    assert!(matches!(state.to_view().state, SessionState::Failed(_)));
    // An unknown code is still a kind.
    state.stop_error_code = Some("something_new".into());
    assert!(state.stop_error_kind().is_some());
}

#[test]
fn attention_follows_the_record() {
    let mut state = session();
    assert_eq!(state.attention(), SessionState::Idle);
    state.phase = Phase::Processing;
    assert_eq!(state.attention(), SessionState::Working);
    state.phase = Phase::WaitingForInput;
    state.completed_at = Some(at(10));
    assert!(state.is_ready_for_review());
    state.reviewed_at = Some(at(11));
    assert_eq!(state.attention(), SessionState::Idle);
    // A Stop not yet confirmed, and a wait on agents, both read as working.
    state.completion_pending_since = Some(at(12));
    assert_eq!(state.attention(), SessionState::Working);
    state.completion_pending_since = None;
    state.background_wait_since = Some(at(12));
    assert_eq!(state.attention(), SessionState::Working);
}

#[test]
fn the_background_wait_reads_only_between_turns() {
    let mut state = session();
    state.background_wait_since = Some(at(5));
    state.background_agent_types = vec!["workflow".into()];
    state.phase = Phase::WaitingForInput;
    assert!(state.is_awaiting_background_work());
    assert_eq!(
        state.background_wait_description().as_deref(),
        Some("1 workflow")
    );
    state.background_agent_types = vec!["subagent".into(), "teammate".into()];
    assert_eq!(
        state.background_wait_description().as_deref(),
        Some("1 background agent and 1 teammate")
    );
    // While Claude works on a turn again, that turn's own progress shows.
    for phase in [Phase::Processing, Phase::Compacting] {
        state.phase = phase;
        assert!(!state.is_awaiting_background_work());
        assert_eq!(state.background_wait_description(), None);
    }
    state.phase = Phase::WaitingForInput;
    state.background_wait_since = None;
    assert!(!state.is_awaiting_background_work());
}

#[test]
fn the_waiting_since_of_a_shown_request() {
    let mut state = session();
    let mut active = context("Bash", json!({"command": "ls"}), vec![]);
    state.phase = Phase::WaitingForApproval(active.clone());
    // Not yet activated: when it was received.
    assert_eq!(state.waiting_since(), Some(at(5)));
    active.activated_at = Some(at(9));
    state.phase = Phase::WaitingForApproval(active);
    assert_eq!(state.waiting_since(), Some(at(9)));
}

// ---- pending requests ----

#[test]
fn pending_requests_list_the_active_one_then_the_queue() {
    let mut state = session();
    let first = context("Bash", json!({"command": "npm test"}), vec![]);
    let mut second = context("Read", json!({"file_path": "/tmp/proj/a.txt"}), vec![]);
    second.agent_id = Some("agent-7".into());
    state.phase = Phase::WaitingForApproval(first.clone());
    state.queued_approvals = vec![second.clone()];

    let requests = state.pending_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tool_use_id, first.tool_use_id);
    assert_eq!(requests[0].session_id.as_str(), "s1");
    assert_eq!(requests[0].kind, RequestKind::Permission);
    assert_eq!(requests[0].input_preview, "npm test");
    assert_eq!(requests[0].input, json!({"command": "npm test"}));
    assert_eq!(requests[0].agent_id, None);
    assert_eq!(requests[1].tool_use_id, second.tool_use_id);
    assert_eq!(requests[1].agent_id.as_deref(), Some("agent-7"));
    assert_eq!(requests[1].received_at, at(5));
    assert_eq!(
        state.pending_permission("toolu_Read").unwrap().tool_name,
        "Read"
    );
    assert!(state.pending_permission("nope").is_none());
    assert_eq!(state.to_view().pending, requests);

    // The queue is the store's to keep empty outside an approval; the
    // record lists whatever it holds after the active request.
    state.phase = Phase::Processing;
    assert_eq!(state.pending_requests().len(), 1);
    state.queued_approvals.clear();
    assert!(state.pending_requests().is_empty());
}

#[test]
fn a_permission_is_always_allowable_only_for_a_narrow_rule() {
    let rule = |destination: &str| {
        json!({"type": "addRules", "destination": destination,
               "rules": [{"toolName": "Bash", "ruleContent": "npm run test:*"}]})
    };
    let request = |suggestion: Value| {
        let mut state = session();
        state.phase = Phase::WaitingForApproval(context(
            "Bash",
            json!({"command": "npm run test:unit"}),
            vec![suggestion],
        ));
        state.pending_requests().remove(0)
    };

    let narrow = request(rule("localSettings"));
    let always = narrow.always.expect("narrow rule");
    assert_eq!(
        always.description,
        "Don't ask again for Bash(npm run test:*) in this project (just you)"
    );
    assert!(always.inline);
    assert_eq!(always.suggestion, rule("localSettings"));

    let session_rule = request(rule("session")).always.expect("session rule");
    assert_eq!(
        session_rule.description,
        "Don't ask again for Bash(npm run test:*) for this session"
    );

    // Wider rules, mode changes, no suggestion, or no description: none.
    assert!(request(rule("userSettings")).always.is_none());
    assert!(request(rule("projectSettings")).always.is_none());
    assert!(
        request(json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"}))
            .always
            .is_none()
    );
    assert!(
        request(json!({"type": "addRules", "destination": "session", "rules": []}))
            .always
            .is_none()
    );
    assert!(request(json!("not an object")).always.is_none());
    let mut state = session();
    state.phase = Phase::WaitingForApproval(context("Bash", json!({"command": "ls"}), vec![]));
    assert!(state.pending_requests()[0].always.is_none());
}

#[test]
fn suggestions_are_described_in_words() {
    assert_eq!(
        describe_suggestion(
            &json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})
        )
        .as_deref(),
        Some("Switch to accept-edits mode for this session")
    );
    assert_eq!(
        describe_suggestion(&json!({"type": "setMode", "mode": "custom"})).as_deref(),
        Some("Switch to custom mode")
    );
    assert_eq!(
        describe_suggestion(
            &json!({"type": "addDirectories", "destination": "userSettings",
                                    "directories": ["/home/me/code/acme", "C:\\work\\site"]})
        )
        .as_deref(),
        Some("Allow access to acme, site in all projects")
    );
    assert_eq!(
        describe_suggestion(&json!({"type": "addRules", "rules": [{"toolName": "Read"}]}))
            .as_deref(),
        Some("Don't ask again for Read")
    );
    assert_eq!(describe_suggestion(&json!({"type": "removeRules"})), None);
    assert_eq!(
        describe_suggestion(&json!({"destination": "session"})),
        None
    );
}

#[test]
fn a_question_request_carries_its_questions() {
    let input = json!({"questions": [
        {"question": " Which screens should onboarding include? ", "header": " Screens ",
         "multiSelect": true,
         "options": [
             {"label": "Welcome", "description": "A greeting"},
             {"label": "  Profile  "},
             "Bare option",
             {"label": ""},
             {"description": "no label"},
             "   "
         ]},
        {"question": "Single?", "multiSelect": "TRUE", "options": []},
        {"question": "Numeric?", "multiSelect": 1},
        {"question": "   "},
        {"header": "no question"},
        "junk"
    ]});
    let parsed = parse_questions(&input);
    assert_eq!(parsed.len(), 3);
    // The answer key is the text exactly as sent.
    assert_eq!(parsed[0].text, " Which screens should onboarding include? ");
    assert_eq!(parsed[0].header.as_deref(), Some("Screens"));
    assert!(parsed[0].multi_select);
    let labels: Vec<_> = parsed[0].options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Welcome", "Profile", "Bare option"]);
    assert_eq!(
        parsed[0].options[0].description.as_deref(),
        Some("A greeting")
    );
    assert_eq!(parsed[0].options[1].description, None);
    assert!(parsed[1].multi_select);
    assert!(parsed[2].multi_select);
    assert!(parse_questions(&json!({})).is_empty());
    assert!(parse_questions(&json!({"questions": "no"})).is_empty());

    let mut state = session();
    state.phase = Phase::WaitingForApproval(context("AskUserQuestion", input, vec![]));
    let request = state.pending_requests().remove(0);
    assert_eq!(request.kind, RequestKind::Question);
    assert_eq!(request.questions.as_ref().map(Vec::len), Some(3));
    assert_eq!(
        request.input_preview,
        "Which screens should onboarding include?"
    );
    assert!(!request.needs_review);
    assert!(request.always.is_none());
    assert!(request.plan_markdown.is_none());

    // No usable question: no list, the raw input stays.
    state.phase =
        Phase::WaitingForApproval(context("AskUserQuestion", json!({"questions": []}), vec![]));
    let request = state.pending_requests().remove(0);
    assert_eq!(request.kind, RequestKind::Question);
    assert!(request.questions.is_none());
    assert_eq!(request.input_preview, "");
}

#[test]
fn a_plan_request_carries_its_markdown() {
    let mut state = session();
    state.phase = Phase::WaitingForApproval(context(
        "ExitPlanMode",
        json!({"plan": "# Plan\n1. Do it\n2. Test it"}),
        vec![],
    ));
    let request = state.pending_requests().remove(0);
    assert_eq!(request.kind, RequestKind::Plan);
    assert_eq!(
        request.plan_markdown.as_deref(),
        Some("# Plan\n1. Do it\n2. Test it")
    );
    assert!(request.questions.is_none());
    assert!(!request.needs_review);
    // The state derived from it.
    assert_eq!(
        state.attention(),
        SessionState::NeedsYou(NeedsInputReason::PlanApproval)
    );

    state.phase = Phase::WaitingForApproval(context("ExitPlanMode", json!({}), vec![]));
    assert!(state.pending_requests()[0].plan_markdown.is_none());
}

#[test]
fn request_text_is_the_whole_command_and_a_full_path() {
    let paths = Paths::new(PathStyle::Posix, "/home/me");
    let preview = |tool: &str, input: Value| permission_preview(tool, &input, Some(&paths));
    assert_eq!(
        preview(
            "Bash",
            json!({"command": "  npm test  \n", "description": "run"})
        )
        .as_deref(),
        Some("npm test")
    );
    assert_eq!(
        preview(
            "Write",
            json!({"file_path": "/home/me/code/a.rs", "content": "x"})
        )
        .as_deref(),
        Some("~/code/a.rs")
    );
    assert_eq!(
        preview("NotebookEdit", json!({"notebook_path": "/srv/n.ipynb"})).as_deref(),
        Some("/srv/n.ipynb")
    );
    assert_eq!(
        preview("WebFetch", json!({"url": "https://example.com/x"})).as_deref(),
        Some("https://example.com/x")
    );
    assert_eq!(
        preview("Grep", json!({"pattern": "TODO", "path": ""})).as_deref(),
        Some("TODO")
    );
    // Else the first string field that isn't "description", in key order.
    assert_eq!(
        preview(
            "mcp__x__y",
            json!({"description": "d", "zeta": "z", "alpha": "a", "n": 3})
        )
        .as_deref(),
        Some("a")
    );
    assert_eq!(preview("mcp__x__y", json!({"description": "only"})), None);
    assert_eq!(preview("Bash", Value::Null), None);
    // Without home rules the path is shown as sent.
    assert_eq!(
        permission_preview("Read", &json!({"file_path": "/home/me/a"}), None).as_deref(),
        Some("/home/me/a")
    );
}

#[test]
fn a_long_request_must_be_reviewed_before_allowing() {
    assert!(!is_too_long_to_review_inline(None));
    assert!(!is_too_long_to_review_inline(Some("one\ntwo\nthree\nfour")));
    assert!(is_too_long_to_review_inline(Some("1\n2\n3\n4\n5")));
    assert!(!is_too_long_to_review_inline(Some(&"x".repeat(200))));
    assert!(is_too_long_to_review_inline(Some(&"x".repeat(201))));
    // Characters, not bytes.
    assert!(!is_too_long_to_review_inline(Some(&"é".repeat(200))));

    let mut state = session();
    let long = "x".repeat(300);
    state.phase = Phase::WaitingForApproval(context("Bash", json!({ "command": long }), vec![]));
    let request = state.pending_requests().remove(0);
    assert!(request.needs_review);
    assert_eq!(request.input_preview.len(), 300);
    state.phase = Phase::WaitingForApproval(context("Bash", json!({"command": "ls"}), vec![]));
    assert!(!state.pending_requests()[0].needs_review);
}

// ---- the view ----

#[test]
fn a_new_session_is_an_idle_view() {
    let state = Session::new("s9", "C:\\Users\\me\\code\\acme", at(3));
    let view = state.to_view();
    assert_eq!(view.id.as_str(), "s9");
    assert_eq!(view.project_name, "acme");
    assert_eq!(view.display_project_name, "acme");
    assert_eq!(view.title, "acme");
    assert!(view.title_from_folder);
    assert_eq!(view.state, SessionState::Idle);
    assert_eq!(view.phase, Phase::Idle);
    assert_eq!(view.first_seen_at, at(3));
    assert_eq!(view.last_activity, at(3));
    assert_eq!(view.last_event_at, at(3));
    assert_eq!(view.attribution, Attribution::Known(None));
    assert!(view.pending.is_empty() && view.running_tools.is_empty());
    assert!(!view.is_hook_backed && !view.is_desktop_hosted && !view.completion_quiet);
    assert!(view.tasks.is_none() && view.waiting_since.is_none());
    // The same as the view's own idle constructor, which tests start from.
    let expected = SessionView::new("s9", "C:\\Users\\me\\code\\acme", at(3));
    assert_eq!(view, expected);
}

#[test]
fn to_view_of_a_populated_session_fills_every_field() {
    let mut s = Session::new("sess-1", "/work/acme-web", at(0));
    s.current_cwd = "/work/acme-web/packages/ui".into();
    s.pid = Some(4242);
    s.pid_started_at = Some(at(1));
    s.terminal = Some(HookTerminal {
        wt_session: Some("wt-1".into()),
        term_program: Some("vscode".into()),
    });
    s.transcript_path = Some("/home/me/.claude/projects/p/sess-1.jsonl".into());
    s.account = Some(AccountId::from("/home/me/.claude"));
    s.ring = Some(RingId::from("claude-acct-1a2b3c4d5e6f"));
    let identity = IdentityId::from("uuid:abc/org");
    s.attribution = Attribution::Unsure(Some(identity.clone()));
    s.attribution_since = at(2);
    s.config_dir_env = Some("/home/me/.claude-work".into());
    s.entrypoint = Some("claude-desktop".into());
    s.host_session_id = Some("local_0123abcd-4567".into());
    s.desktop_identity = Some(identity.clone());
    s.apply_title(Some("Fix the login loop"), SessionTitleSource::Hook);
    s.model = Some("claude-opus-4".into());
    s.permission_mode = Some("acceptEdits".into());
    s.last_assistant_message = Some("All done".into());
    s.turn_started_at = Some(at(10));
    s.completed_at = Some(at(20));
    s.reviewed_at = Some(at(15));
    s.completion_pending_since = Some(at(21));
    s.background_task_count = 3;
    s.background_agent_count = 1;
    s.background_agent_types = vec!["workflow".into()];
    s.background_wait_since = Some(at(22));
    s.last_prompt_source = Some("user".into());
    s.last_prompt_was_user_authored = true;
    s.agents_at_turn_start = 0;
    s.tasks.task_created("1", Some("Write tests"));
    s.context_used_percent = Some(42.5);
    s.context_window_size = Some(200_000);
    s.cost_usd = Some(1.25);
    s.phase = Phase::WaitingForApproval(context("Bash", json!({"command": "ls"}), vec![]));
    s.conversation_info = ConversationInfo {
        summary: Some("Login redirect fix".into()),
        last_message: Some("Did the thing".into()),
        last_message_role: Some("tool".into()),
        last_tool_name: Some("Bash".into()),
        ..ConversationInfo::default()
    };
    s.tool_tracker.start_tool("tu-b", "Bash", None, at(31));
    s.tool_tracker
        .start_tool("tu-a", "Read", Some("agent-2"), at(30));
    s.tool_tracker.set_phase(ToolPhase::PendingApproval, "tu-b");
    s.last_activity = at(40);
    s.last_event_at = at(41);
    s.last_hook_event_at = Some(at(39));
    s.registry_status = Some("busy".into());
    s.registry_status_changed_at = Some(at(38));

    // Every field of the view is named: a field added to SessionView without
    // a line here stops this test from compiling.
    let SessionView {
        id,
        account,
        ring,
        attribution,
        attribution_since,
        cwd,
        project_name,
        title,
        title_from_folder,
        state,
        phase,
        pid,
        pid_started,
        entrypoint,
        config_dir_env,
        host_session_id,
        registry_status,
        first_seen_at,
        model,
        context_pct,
        tasks,
        background,
        last_activity,
        turn_started_at,
        completed_at,
        reviewed_at,
        last_assistant_message,
        pending,
        cost_usd,
        transcript_path,
        current_cwd,
        display_project_name,
        public_title,
        summary,
        last_message,
        last_message_role,
        last_tool_name,
        is_hook_backed,
        last_hook_event_at,
        last_event_at,
        running_tools,
        waiting_since,
        completion_pending_since,
        completion_quiet,
        stop_error_is_restored,
        background_wait_description,
        permission_mode,
        context_window_size,
        terminal,
        desktop_identity,
        is_desktop_hosted,
    } = s.to_view();

    assert_eq!(id.as_str(), "sess-1");
    assert_eq!(account, Some(AccountId::from("/home/me/.claude")));
    assert_eq!(ring, Some(RingId::from("claude-acct-1a2b3c4d5e6f")));
    assert_eq!(attribution, Attribution::Unsure(Some(identity.clone())));
    assert_eq!(attribution_since, at(2));
    assert_eq!(cwd, std::path::PathBuf::from("/work/acme-web"));
    assert_eq!(project_name, "acme-web");
    assert_eq!(title, "Fix the login loop");
    assert!(!title_from_folder);
    assert_eq!(
        state,
        SessionState::NeedsYou(NeedsInputReason::Permission {
            tool: Some("Bash".into())
        })
    );
    assert!(matches!(phase, Phase::WaitingForApproval(_)));
    assert_eq!(pid, Some(4242));
    assert_eq!(pid_started, Some(at(1)));
    assert_eq!(entrypoint.as_deref(), Some("claude-desktop"));
    assert_eq!(config_dir_env.as_deref(), Some("/home/me/.claude-work"));
    assert_eq!(host_session_id.as_deref(), Some("local_0123abcd-4567"));
    assert_eq!(registry_status.as_deref(), Some("busy"));
    assert_eq!(first_seen_at, at(0));
    assert_eq!(model.as_deref(), Some("claude-opus-4"));
    assert_eq!(context_pct, Some(42.5));
    let tasks = tasks.expect("task progress");
    assert_eq!((tasks.done, tasks.total), (0, 1));
    assert_eq!(background.since, Some(at(22)));
    assert_eq!(background.agent_types, ["workflow"]);
    assert_eq!(background.task_count, 3);
    assert_eq!(last_activity, at(40));
    assert_eq!(turn_started_at, Some(at(10)));
    assert_eq!(completed_at, Some(at(20)));
    assert_eq!(reviewed_at, Some(at(15)));
    assert_eq!(last_assistant_message.as_deref(), Some("All done"));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool_name, "Bash");
    assert_eq!(cost_usd, Some(1.25));
    assert_eq!(
        transcript_path,
        Some(std::path::PathBuf::from(
            "/home/me/.claude/projects/p/sess-1.jsonl"
        ))
    );
    assert_eq!(
        current_cwd,
        std::path::PathBuf::from("/work/acme-web/packages/ui")
    );
    assert_eq!(display_project_name, "ui");
    assert_eq!(public_title, "Fix the login loop");
    assert_eq!(summary.as_deref(), Some("Login redirect fix"));
    assert_eq!(last_message.as_deref(), Some("Did the thing"));
    assert_eq!(last_message_role.as_deref(), Some("tool"));
    assert_eq!(last_tool_name.as_deref(), Some("Bash"));
    assert!(is_hook_backed);
    assert_eq!(last_hook_event_at, Some(at(39)));
    assert_eq!(last_event_at, at(41));
    // Oldest first.
    let tools: Vec<_> = running_tools
        .iter()
        .map(|t| (t.id.as_str(), t.pending_approval))
        .collect();
    assert_eq!(tools, [("tu-a", false), ("tu-b", true)]);
    assert_eq!(running_tools[0].agent_id.as_deref(), Some("agent-2"));
    assert_eq!(running_tools[1].started_at, at(31));
    assert_eq!(waiting_since, Some(at(5)));
    assert_eq!(completion_pending_since, Some(at(21)));
    // The typed turn started a workflow: quiet.
    assert!(completion_quiet);
    // Seen happen in this sighting, not read back from disk.
    assert!(!stop_error_is_restored);
    // Waiting on agents is shown only between turns; here a request is shown
    // (phase waiting for approval), so the wait reads.
    assert_eq!(background_wait_description.as_deref(), Some("1 workflow"));
    assert_eq!(permission_mode.as_deref(), Some("acceptEdits"));
    assert_eq!(context_window_size, Some(200_000));
    assert_eq!(
        terminal,
        Some(HookTerminal {
            wt_session: Some("wt-1".into()),
            term_program: Some("vscode".into())
        })
    );
    assert_eq!(desktop_identity, Some(identity));
    assert!(is_desktop_hosted);
}

#[test]
fn the_view_clamps_a_wild_context_percent() {
    let mut state = session();
    state.context_used_percent = Some(250.0);
    assert_eq!(state.to_view().context_pct, Some(100.0));
    state.context_used_percent = Some(-3.0);
    assert_eq!(state.to_view().context_pct, Some(0.0));
    state.context_used_percent = Some(f64::NAN);
    assert_eq!(state.to_view().context_pct, None);
}

#[test]
fn requests_show_paths_under_home_when_the_session_knows_it() {
    let mut state = session();
    state.paths = Some(Paths::new(PathStyle::Posix, "/home/me"));
    state.phase = Phase::WaitingForApproval(context(
        "Edit",
        json!({"file_path": "/home/me/code/main.rs"}),
        vec![],
    ));
    assert_eq!(state.pending_requests()[0].input_preview, "~/code/main.rs");
}

#[test]
fn the_record_clones_with_its_chat_and_trackers() {
    // The store keeps whole records in maps and hands copies to its tests.
    let mut state = session();
    state.tool_tracker.start_tool("t", "Bash", None, at(1));
    let copy = state.clone();
    state.tool_tracker.complete_tool("t");
    assert_eq!(copy.tool_tracker.len(), 1);
    assert_eq!(state.chat.len(), 0);
    assert_eq!(copy.chat_history().items.len(), 0);
}
