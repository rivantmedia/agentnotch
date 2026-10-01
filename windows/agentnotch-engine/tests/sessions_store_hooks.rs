//! The session store's hook and status line inputs: SessionStoreFlowTests,
//! A1_SessionStoreRegressionTests, SessionCoreRegressionTests and
//! A1_ReviewFixesTests ported to `SessionStore::apply`, plus the Windows
//! contract (pid-less sessions, one account sighting per session per minute,
//! the releases of held requests).
//!
//! The Mac tests that need a confirmed completion after a delay or the
//! registry (turnCompletesIntoReviewAndPromptReviews,
//! stopWithBackgroundTasksIsReadyForReview, ...) belong to wp5-8; the review
//! marks of the first one to wp5-9.

mod sessions_support;

use agentnotch_engine::model::{
    AccountId, Answer, Attribution, HeldPermission, HookEvent, IdentityId, NeedsInputReason, Phase,
    SessionId, SessionState, StatusLineMessage,
};
use agentnotch_engine::runtime_types::{Release, SessionInput};
use agentnotch_engine::sessions::attention::StopErrorKind;
use agentnotch_engine::sessions::phase::is_user_authored_prompt;
use agentnotch_engine::sessions::session::ToolPhase;
use sessions_support::{t0, Harness, TRANSCRIPT};
use std::time::Duration;

fn permission(tool: &str) -> SessionState {
    SessionState::NeedsYou(NeedsInputReason::Permission {
        tool: Some(tool.to_owned()),
    })
}

fn is_failed_with(state: &SessionState, expected: &str) -> bool {
    matches!(state, SessionState::Failed(NeedsInputReason::Error { text, .. }) if text == expected)
}

fn s1() -> SessionId {
    SessionId::from("s1")
}

fn status_line(percent: f64) -> StatusLineMessage {
    StatusLineMessage {
        session_id: s1(),
        cwd: Some("/tmp/proj".into()),
        transcript_path: Some(TRANSCRIPT.into()),
        config_dir_env: None,
        account_id: None,
        received_at: t0(),
        rate_limits: None,
        five_hour: None,
        seven_day: None,
        context_used_percent: Some(percent),
        context_window_size: Some(200_000),
        model_id: None,
        model_display_name: Some("Opus".into()),
        cost_usd: Some(0.5),
        session_name: None,
        claude_code_version: None,
        pid: None,
    }
}

// ---- SessionStoreFlowTests ----

#[test]
fn late_events_dont_resurrect_a_finished_session() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook("Stop", "waiting_for_input");
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg".into());
    });
    h.hook("SubagentStop", "processing");
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.agent_id = Some("agent-1".into());
        b.tool = Some("Read".into());
        b.tool_use_id = Some("toolu_sub".into());
    });
    assert_eq!(h.session().unwrap().phase, Phase::WaitingForInput);
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

#[test]
fn context_resume_stop_is_not_a_completion() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some(
            "This session is being continued from a previous conversation that ran out of context."
                .into(),
        );
    });
    assert!(h.session().unwrap().completed_at.is_none());
    assert_eq!(h.state(), SessionState::Idle);
}

#[test]
fn session_start_does_not_complete_but_allows_idle_to_waiting() {
    let mut h = Harness::new();
    h.hook_with("SessionStart", "waiting_for_input", |b| {
        b.source = Some("startup".into());
        b.session_title = Some("Refactor".into());
    });
    let started = h.session().unwrap();
    assert_eq!(started.phase, Phase::WaitingForInput);
    assert!(started.completed_at.is_none());
    assert_eq!(started.display_title(), "Refactor");
}

#[test]
fn stop_failure_needs_input_until_next_prompt() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into());
    });
    assert!(
        is_failed_with(&h.state(), "Rate limited"),
        "{:?}",
        h.state()
    );
    h.hook("UserPromptSubmit", "processing");
    assert_eq!(h.state(), SessionState::Working);
}

#[test]
fn notifications_set_and_clear_needs_input() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("elicitation_dialog".into());
        b.message = Some("Pick a repo".into());
    });
    assert_eq!(
        h.state(),
        SessionState::NeedsYou(NeedsInputReason::Elicitation {
            message: "Pick a repo".into()
        })
    );
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("elicitation_complete".into());
    });
    assert_eq!(h.state(), SessionState::Working);

    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("permission_prompt".into());
        b.message = Some("Claude needs your permission to use Bash".into());
    });
    assert_eq!(h.state(), permission("Bash"));
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    assert_eq!(h.state(), SessionState::Working);
}

#[test]
fn parallel_approvals_queue_and_other_tools_dont_hide_them() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_a".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Edit".into());
        b.tool_use_id = Some("toolu_b".into());
    });
    assert_eq!(h.state(), permission("Bash"));

    // An unrelated auto-allowed tool finishing doesn't answer anything.
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Read".into());
        b.tool_use_id = Some("toolu_c".into());
    });
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "toolu_a"
    );

    h.approve("toolu_a");
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "toolu_b"
    );

    // Answered in the terminal: its PostToolUse resolves it.
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Edit".into());
        b.tool_use_id = Some("toolu_b".into());
    });
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
}

#[test]
fn dead_permission_socket_leaves_approval() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_a".into());
    });
    h.socket_failed("toolu_a");
    let current = h.session().unwrap();
    assert_eq!(current.phase, Phase::Processing);
    assert!(current.active_permission().is_none());
    // The pipe is gone already: nothing is left to release.
    assert!(h.releases.is_empty());
}

#[test]
fn status_line_creates_session_and_sets_context() {
    let mut h = Harness::new();
    h.status_line(status_line(64.0));
    let created = h.session().unwrap();
    assert_eq!(created.phase, Phase::Idle);
    assert_eq!(created.context_used_percent, Some(64.0));
    assert_eq!(created.context_window_size, Some(200_000));
    assert_eq!(created.model.as_deref(), Some("Opus"));
    assert_eq!(created.cost_usd, Some(0.5));
    // The folder comes from the transcript path.
    assert_eq!(
        created.account.as_ref().map(AccountId::as_str),
        Some("/home/me/.claude")
    );
    // A status line says nothing about the turn.
    assert!(created.last_hook_event_at.is_none());
    assert!(!h.view().unwrap().is_hook_backed);
}

#[test]
fn status_line_context_is_clamped() {
    let mut h = Harness::new();
    h.status_line(status_line(250.0));
    assert_eq!(h.session().unwrap().context_used_percent, Some(100.0));
    h.status_line(status_line(-5.0));
    assert_eq!(h.session().unwrap().context_used_percent, Some(0.0));
    assert_eq!(h.view().unwrap().context_pct, Some(0.0));
}

#[test]
fn session_end_removes_and_blocks_late_status_lines() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook("SessionEnd", "ended");
    assert!(h.session().is_none());
    assert_eq!(h.take_releases(), vec![Release::Session(s1())]);

    h.status_line(StatusLineMessage {
        transcript_path: None,
        context_used_percent: None,
        ..status_line(0.0)
    });
    assert!(h.session().is_none());

    // Ten minutes later it is forgotten: a status line is a live session.
    h.advance(10 * 60 + 1);
    h.status_line(status_line(10.0));
    assert!(h.session().is_some());
}

#[test]
fn a_hook_revives_an_ended_session_id() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook("SessionEnd", "ended");
    h.hook("UserPromptSubmit", "processing");
    assert_eq!(h.state(), SessionState::Working);
}

// ---- A1_SessionStoreRegressionTests ----

#[test]
fn background_agent_requests_outlive_the_main_stop() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("bg-1".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Edit".into());
        b.tool_use_id = Some("toolu_main".into());
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(vec!["subagent".into()]);
    });

    // The main request is over and its pipe closed; the agent's stays.
    assert_eq!(h.take_releases(), vec![Release::MainAgent(s1())]);
    let current = h.session().unwrap();
    assert_eq!(current.active_permission().unwrap().tool_use_id, "toolu_bg");
    assert!(current.queued_approvals.is_empty());
    assert_eq!(h.state(), permission("Bash"));
    assert!(h.session().unwrap().completed_at.is_some());

    // A wake-up turn (a sibling agent finished) keeps it too...
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("system".into())
    });
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "toolu_bg"
    );
    // ...and so does an interrupt of the main turn.
    let at = h.now;
    h.interrupt(at);
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "toolu_bg"
    );
    // The agent's request is not the main agent's to release.
    assert!(h.take_releases().is_empty());

    // Answered: back to the finished turn.
    h.approve("toolu_bg");
    let current = h.session().unwrap();
    assert!(current.active_permission().is_none());
    assert_eq!(current.phase, Phase::Idle);
}

#[test]
fn main_requests_end_with_the_turn_and_close_their_pipes() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    let effects = h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("overloaded".into());
    });
    // A failed turn leaves nothing the session holds open.
    assert_eq!(effects.release, vec![Release::Session(s1())]);
    let current = h.session().unwrap();
    assert!(current.active_permission().is_none());
    assert!(is_failed_with(&h.state(), "Overloaded"), "{:?}", h.state());
    assert_eq!(current.stop_error_kind(), Some(StopErrorKind::Overloaded));
    // The call that was waiting on the user was interrupted.
    assert_eq!(current.chat.tool_status("toolu_1"), Some("interrupted"));
}

#[test]
fn a_stop_closes_only_the_main_agents_pipes() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    let effects = h.hook("Stop", "waiting_for_input");
    assert_eq!(effects.release, vec![Release::MainAgent(s1())]);
    // A Stop with nothing pending releases nothing.
    let effects = h.hook("Stop", "waiting_for_input");
    assert!(effects.release.is_empty());
}

#[test]
fn running_tools_end_with_their_call_or_the_main_turn() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.at(0);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.at(1).hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Read".into());
        b.tool_use_id = Some("toolu_read".into());
    });
    h.at(2).hook_with("PreToolUse", "running_tool", |b| {
        b.agent_id = Some("bg-1".into());
        b.tool = Some("Grep".into());
        b.tool_use_id = Some("toolu_agent".into());
    });
    h.at(3).hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bash".into());
    });
    let tracker = &h.session().unwrap().tool_tracker;
    assert_eq!(tracker.newest().unwrap().name, "Bash");
    let mut ids: Vec<&str> = tracker
        .in_progress()
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["toolu_agent", "toolu_bash", "toolu_read"]);

    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bash".into());
    });
    assert_eq!(
        h.session()
            .unwrap()
            .tool_tracker
            .get("toolu_bash")
            .unwrap()
            .phase,
        ToolPhase::PendingApproval
    );
    h.approve("toolu_bash");
    assert_eq!(
        h.session()
            .unwrap()
            .tool_tracker
            .get("toolu_bash")
            .unwrap()
            .phase,
        ToolPhase::Running
    );
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bash".into());
    });
    assert_eq!(
        h.session().unwrap().tool_tracker.newest().unwrap().name,
        "Grep"
    );

    // The main turn ends: its calls are over (the Read never reported back);
    // the background agent's call goes on.
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(vec!["subagent".into()]);
    });
    let tracker = &h.session().unwrap().tool_tracker;
    let ids: Vec<&str> = tracker
        .in_progress()
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids, ["toolu_agent"]);
    // The view lists the running tool.
    let running = h.view().unwrap().running_tools;
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].name, "Grep");
    assert_eq!(running[0].agent_id.as_deref(), Some("bg-1"));
}

#[test]
fn answers_name_the_request_the_user_saw() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.at(0).hook("UserPromptSubmit", "processing");
    h.at(1)
        .hook_with("PermissionRequest", "waiting_for_approval", |b| {
            b.tool = Some("Bash".into());
            b.tool_use_id = Some("toolu_a".into());
        });
    h.at(2)
        .hook_with("PermissionRequest", "waiting_for_approval", |b| {
            b.tool = Some("Bash".into());
            b.tool_use_id = Some("toolu_b".into());
        });
    let current = h.session().unwrap();
    assert_eq!(
        current.active_permission().unwrap().activated_at,
        Some(t0() + Duration::from_secs(1))
    );
    assert!(current.pending_permission("toolu_b").is_some());

    // Allow A, then a second click on A (it landed after the row swapped).
    h.at(10);
    h.approve("toolu_a");
    h.approve("toolu_a");
    let current = h.session().unwrap();
    assert_eq!(current.active_permission().unwrap().tool_use_id, "toolu_b");
    assert!(current.pending_permission("toolu_a").is_none());
    // B was promoted when A was answered (not when B arrived): the UI ignores
    // clicks for a moment after this.
    let active = current.active_permission().unwrap();
    assert_eq!(active.activated_at, Some(t0() + Duration::from_secs(10)));
    assert_ne!(active.activated_at, Some(active.received_at));
}

#[test]
fn a_late_outcome_does_not_revive_a_finished_tool() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.approve("toolu_1");
    assert_eq!(
        h.session().unwrap().chat.tool_status("toolu_1"),
        Some("success")
    );
}

#[test]
fn a_denial_ends_the_call_and_promotes_the_next_request() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    for (tool, id) in [("Bash", "toolu_a"), ("Edit", "toolu_b")] {
        h.hook_with("PreToolUse", "running_tool", |b| {
            b.tool = Some(tool.into());
            b.tool_use_id = Some(id.into());
        });
        h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
            b.tool = Some(tool.into());
            b.tool_use_id = Some(id.into());
        });
    }
    h.deny("toolu_a");
    let current = h.session().unwrap();
    assert_eq!(current.chat.tool_status("toolu_a"), Some("error"));
    assert!(current.tool_tracker.get("toolu_a").is_none());
    assert_eq!(current.active_permission().unwrap().tool_use_id, "toolu_b");
    assert_eq!(
        current.chat.tool_status("toolu_b"),
        Some("waiting_for_approval")
    );

    // A plan the user keeps planning on is a denial; an answered question or
    // an approved plan lets the call run.
    h.answer("toolu_b", Answer::KeepPlanning);
    assert_eq!(
        h.session().unwrap().chat.tool_status("toolu_b"),
        Some("error")
    );
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("AskUserQuestion".into());
        b.tool_use_id = Some("toolu_q".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("AskUserQuestion".into());
        b.tool_use_id = Some("toolu_q".into());
    });
    assert_eq!(
        h.state(),
        SessionState::NeedsYou(NeedsInputReason::Question)
    );
    h.answer(
        "toolu_q",
        Answer::Questions {
            answers: [("Which?".to_owned(), "A".to_owned())].into(),
        },
    );
    assert_eq!(
        h.session().unwrap().chat.tool_status("toolu_q"),
        Some("running")
    );
    assert_eq!(h.state(), SessionState::Working);
}

#[test]
fn a_vscode_prompt_reviews_but_a_task_notification_does_not() {
    let mut h = Harness::new();
    let vscode = |b: &mut sessions_support::HookEventBuilder, prompt: &str| {
        b.entrypoint = "claude-vscode".into();
        b.source = Some("sdk".into());
        b.prompt = Some(prompt.into());
    };
    h.hook_with("UserPromptSubmit", "processing", |b| {
        vscode(b, "fix the build")
    });
    h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // A background agent's result wakes Claude: not the user looking.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        vscode(
            b,
            "<task-notification><task-id>a1</task-id> completed</task-notification>",
        )
    });
    let at = h.now;
    h.interrupt(at);
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // The user types the next prompt in VS Code: reviewed.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        vscode(b, "now the docs")
    });
    let at = h.now;
    h.interrupt(at);
    assert_eq!(h.state(), SessionState::Idle);

    // An SDK script (not an editor) never counts.
    let mut sdk = HookEvent::new("s1", "UserPromptSubmit", t0());
    sdk.status = "processing".into();
    sdk.entrypoint = Some("sdk-ts".into());
    sdk.source = Some("sdk".into());
    sdk.prompt = Some("hi".into());
    assert!(!is_user_authored_prompt(&sdk));
    let mut system = HookEvent::new("s1", "UserPromptSubmit", t0());
    system.source = Some("system".into());
    assert!(!is_user_authored_prompt(&system));
}

#[test]
fn background_compaction_does_not_reopen_a_finished_turn() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook("Stop", "waiting_for_input");
    h.hook_with("PreCompact", "compacting", |b| {
        b.trigger = Some("auto".into())
    });
    h.hook_with("PostCompact", "processing", |b| {
        b.trigger = Some("auto".into())
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // The user's own /compact at the prompt still shows.
    h.hook_with("PreCompact", "compacting", |b| {
        b.trigger = Some("manual".into())
    });
    assert_eq!(h.session().unwrap().phase, Phase::Compacting);
    h.hook_with("PostCompact", "waiting_for_input", |b| {
        b.trigger = Some("manual".into())
    });
    assert_eq!(h.session().unwrap().phase, Phase::WaitingForInput);
}

#[test]
fn agent_view_announcements_are_about_other_sessions() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("agent_needs_input".into());
        b.message = Some("worker-3 needs your input: pick a db".into());
    });
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("agent_completed".into());
        b.message = Some("worker-3 finished".into());
    });
    assert!(h.session().unwrap().needs_input_reason().is_none());
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("agent_needs_input".into());
        b.message = Some("Choose how to set up teammates".into());
    });
    assert_eq!(
        h.session().unwrap().needs_input_reason(),
        Some(&NeedsInputReason::Dialog {
            detail: "Choose how to set up teammates".into()
        })
    );
}

// ---- SessionCoreRegressionTests ----

#[test]
fn background_agent_approval_after_stop_does_not_restart_the_turn() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_count = Some(1)
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);

    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("agent-bg".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg1".into());
    });
    assert_eq!(h.state(), permission("Bash"));
    h.approve("toolu_bg1");
    assert_eq!(h.session().unwrap().phase, Phase::WaitingForInput);

    // Answered in the terminal instead: the agent's PostToolUse resolves it.
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("agent-bg".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg2".into());
    });
    h.take_releases();
    h.hook_with("PostToolUse", "processing", |b| {
        b.agent_id = Some("agent-bg".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg2".into());
    });
    // The hook held for that request is over.
    assert_eq!(
        h.take_releases(),
        vec![Release::Request {
            session: s1(),
            tool_use_id: "toolu_bg2".into()
        }]
    );
    let session = h.session().unwrap();
    assert_eq!(session.phase, Phase::WaitingForInput);
    // Still unreviewed; the running agent shows as a detail.
    assert_eq!(session.attention(), SessionState::ReadyForReview);
}

#[test]
fn idle_notification_does_not_drop_a_pending_approval() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("Notification", "waiting_for_input", |b| {
        b.notification_type = Some("idle_prompt".into())
    });
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "toolu_1"
    );
    assert!(h.releases.is_empty());
}

#[test]
fn background_agent_activity_keeps_a_failed_turns_error() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("overloaded".into())
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("agent-bg".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg".into());
    });
    assert_eq!(h.state(), permission("Bash"));
    h.approve("toolu_bg");
    h.hook_with("PostToolUse", "processing", |b| {
        b.agent_id = Some("agent-bg".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_bg".into());
    });
    assert!(is_failed_with(&h.state(), "Overloaded"), "{:?}", h.state());

    // A terminal permission prompt answered for the agent is settled by its
    // activity.
    h.hook_with("Notification", "notification", |b| {
        b.notification_type = Some("permission_prompt".into());
        b.message = Some("Claude needs your permission to use Edit".into());
    });
    assert_eq!(h.state(), permission("Edit"));
    h.hook_with("PostToolUse", "processing", |b| {
        b.agent_id = Some("agent-bg".into());
        b.tool = Some("Edit".into());
        b.tool_use_id = Some("toolu_bg2".into());
    });
    assert!(h.session().unwrap().needs_input_reason().is_none());
}

#[test]
fn main_session_approval_still_resumes_processing() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.approve("toolu_1");
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
}

// ---- A1_ReviewFixesTests ----

#[test]
fn a_subagents_stop_does_not_end_the_main_turn() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.at(0).hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.at(1).hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_main".into());
    });
    h.at(2).hook_with("Stop", "waiting_for_input", |b| {
        b.agent_id = Some("teammate-1".into())
    });
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
    assert_eq!(h.state(), SessionState::Working);

    // Nor does it drop the main session's request.
    h.at(3)
        .hook_with("PermissionRequest", "waiting_for_approval", |b| {
            b.tool = Some("Bash".into());
            b.tool_use_id = Some("toolu_main".into());
        });
    h.at(4).hook_with("StopFailure", "waiting_for_input", |b| {
        b.agent_id = Some("teammate-1".into());
        b.stop_error = Some("rate_limit".into());
    });
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "toolu_main"
    );
    assert!(h.releases.is_empty());

    // The main Stop does end it.
    h.at(5).hook("Stop", "waiting_for_input");
    assert!(h.session().unwrap().active_permission().is_none());
    assert_eq!(h.take_releases(), vec![Release::MainAgent(s1())]);
}

// ---- the hooks' own contract ----

#[test]
fn a_subagent_that_stops_loses_the_requests_it_still_asked() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("a1".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_a1".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("a2".into());
        b.tool = Some("Edit".into());
        b.tool_use_id = Some("toolu_a2".into());
    });
    let effects = h.hook_with("SubagentStop", "processing", |b| {
        b.agent_id = Some("a1".into())
    });
    assert_eq!(
        effects.release,
        vec![Release::Agent {
            session: s1(),
            agent_id: "a1".into()
        }]
    );
    let current = h.session().unwrap();
    assert_eq!(current.active_permission().unwrap().tool_use_id, "toolu_a2");
    assert!(current.pending_permission("toolu_a1").is_none());
}

#[test]
fn a_held_request_is_its_permission_request_event() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    let mut event = HookEvent::new("s1", "PermissionRequest", h.now);
    event.status = "waiting_for_approval".into();
    event.cwd = "/tmp/proj".into();
    event.tool = Some("Bash".into());
    event.tool_input = Some(serde_json::Map::new());
    event.attended = Some(true);
    event.entrypoint = Some("cli".into());
    h.apply(SessionInput::Held(HeldPermission {
        conn: 7,
        session_id: s1(),
        tool_use_id: "toolu_held".into(),
        has_synthetic_tool_use_id: true,
        agent_id: None,
        event,
        received_at: h.now,
    }));
    let current = h.session().unwrap();
    let active = current.active_permission().unwrap();
    assert_eq!(active.tool_use_id, "toolu_held");
    assert!(active.has_synthetic_tool_use_id);
    assert_eq!(h.view().unwrap().pending.len(), 1);
}

#[test]
fn ignored_sessions_are_not_tracked_and_their_requests_are_released() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.entrypoint = "sdk-ts".into()
    });
    assert!(h.session().is_none());
    let effects = h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.attended = Some(false);
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_x".into());
    });
    assert!(h.session().is_none());
    assert_eq!(effects.release, vec![Release::Session(s1())]);
    assert!(effects.sightings.is_empty());
    // The VS Code extension is kept.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.entrypoint = "claude-vscode".into()
    });
    assert!(h.session().is_some());
}

#[test]
fn hook_frames_with_a_null_pid_make_a_pid_less_session() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    let view = h.view().unwrap();
    assert_eq!(view.pid, None);
    assert_eq!(view.pid_started, None);
    assert!(view.is_hook_backed);
    // A later frame that names Claude's process gives the session a pid, and
    // the hub's start time with it.
    let started = t0() - Duration::from_secs(3600);
    h.ctx.pid_started = Some(started);
    h.hook_with("PreToolUse", "running_tool", |b| b.pid = Some(4242));
    let view = h.view().unwrap();
    assert_eq!(view.pid, Some(4242));
    assert_eq!(view.pid_started, Some(started));
    // A frame without a pid does not forget it.
    h.hook("PostToolUse", "processing");
    assert_eq!(h.view().unwrap().pid, Some(4242));
    // The hub's trusted pid wins over the hook's; a new process, a new start.
    let restarted = t0() - Duration::from_secs(60);
    h.ctx.trusted_pid = Some(777);
    h.ctx.pid_started = Some(restarted);
    h.hook_with("PostToolUse", "processing", |b| b.pid = Some(4242));
    let view = h.view().unwrap();
    assert_eq!(view.pid, Some(777));
    assert_eq!(view.pid_started, Some(restarted));
}

#[test]
fn sightings_are_one_per_session_per_minute() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.at(0).hook("UserPromptSubmit", "processing");
    assert_eq!(h.sightings.len(), 1);
    let first = &h.sightings[0];
    assert_eq!(first.config_dir.as_str(), "/home/me/.claude");
    assert_eq!(first.config_dir_env, None);
    assert_eq!(first.session_id, s1());
    assert_eq!(first.at, t0());

    // Within the minute: none, for hooks and status lines alike.
    h.at(30).hook("PreToolUse", "running_tool");
    h.at(59).status_line(status_line(10.0));
    assert_eq!(h.sightings.len(), 1);

    h.at(60).hook("PostToolUse", "processing");
    assert_eq!(h.sightings.len(), 2);

    // The account changing is sighted at once.
    h.at(61).hook_with("PostToolUse", "processing", |b| {
        b.transcript_path = Some("/home/me/.claude-work/projects/-tmp-proj/s1.jsonl".into());
        b.config_dir_env = Some("/home/me/.claude-work".into());
    });
    assert_eq!(h.sightings.len(), 3);
    assert_eq!(h.sightings[2].config_dir.as_str(), "/home/me/.claude-work");
    assert_eq!(
        h.sightings[2].config_dir_env.as_deref(),
        Some("/home/me/.claude-work")
    );

    // Another session has its own minute.
    h.at(62).hook_with("UserPromptSubmit", "processing", |b| {
        b.session_id = "s2".into()
    });
    assert_eq!(h.sightings.len(), 4);

    // An event that names no folder says nothing about one.
    h.at(200).hook_with("PostToolUse", "processing", |b| {
        b.transcript_path = None;
        b.session_id = "s3".into();
    });
    assert_eq!(h.sightings.len(), 4);
}

#[test]
fn the_account_is_the_hubs_answer_else_the_transcripts_folder() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    assert_eq!(
        h.session().unwrap().account.as_ref().unwrap().as_str(),
        "/home/me/.claude"
    );

    // The hub's answer wins.
    h.ctx.account = Some(AccountId::from("/home/me/.claude-work"));
    h.hook("PreToolUse", "running_tool");
    assert_eq!(
        h.session().unwrap().account.as_ref().unwrap().as_str(),
        "/home/me/.claude-work"
    );

    // Without it, a transcript through the shared history names no account:
    // the environment decides.
    let mut other = Harness::new();
    other.hook_with("UserPromptSubmit", "processing", |b| {
        b.transcript_path = Some("/home/me/.claude-shared/projects/-tmp-proj/s1.jsonl".into());
        b.config_dir_env = Some("/home/me/.claude-b".into());
    });
    assert_eq!(
        other.session().unwrap().account.as_ref().unwrap().as_str(),
        "/home/me/.claude-b"
    );
}

#[test]
fn the_attribution_follows_the_hub_and_remembers_since_when() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.at(0).hook("UserPromptSubmit", "processing");
    let view = h.view().unwrap();
    assert_eq!(view.attribution, Attribution::Known(None));
    assert_eq!(view.attribution_since, t0());

    let identity = IdentityId::from("uuid:acct-1");
    h.ctx.attribution = Attribution::Known(Some(identity.clone()));
    h.at(10).hook("PreToolUse", "running_tool");
    let view = h.view().unwrap();
    assert_eq!(view.attribution, Attribution::Known(Some(identity.clone())));
    assert_eq!(view.attribution_since, t0() + Duration::from_secs(10));

    // The same answer again keeps the time.
    h.at(20).hook("PostToolUse", "processing");
    assert_eq!(
        h.view().unwrap().attribution_since,
        t0() + Duration::from_secs(10)
    );
    // And so does the status line's.
    h.ctx.attribution = Attribution::Unsure(Some(identity));
    h.status_line(StatusLineMessage {
        received_at: t0() + Duration::from_secs(30),
        ..status_line(5.0)
    });
    assert_eq!(
        h.view().unwrap().attribution,
        Attribution::Unsure(Some(IdentityId::from("uuid:acct-1")))
    );
    assert_eq!(
        h.view().unwrap().attribution_since,
        t0() + Duration::from_secs(30)
    );
}

#[test]
fn metadata_comes_from_the_events() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.cwd = "/work/alpha".into();
        b.config_dir_env = Some("/home/me/.claude-work".into());
        b.session_title = Some("Fix login".into());
    });
    let session = h.session().unwrap();
    assert_eq!(session.cwd, "/work/alpha");
    assert_eq!(session.project_name, "alpha");
    assert_eq!(session.entrypoint.as_deref(), Some("cli"));
    assert_eq!(
        session.config_dir_env.as_deref(),
        Some("/home/me/.claude-work")
    );
    assert_eq!(session.transcript_path.as_deref(), Some(TRANSCRIPT));
    assert_eq!(session.display_title(), "Fix login");

    // The folder a session started in never changes; the latest one shows.
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.cwd = "/work/beta".into()
    });
    let session = h.session().unwrap();
    assert_eq!(session.cwd, "/work/alpha");
    assert_eq!(session.current_cwd, "/work/beta");
    assert_eq!(h.view().unwrap().display_project_name, "beta");

    // The main session sets the permission mode and the model; a subagent
    // never sets the mode, nor replaces the transcript path.
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.permission_mode = Some("plan".into());
        b.model = Some("claude-opus".into());
    });
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.agent_id = Some("a1".into());
        b.permission_mode = Some("bypassPermissions".into());
        b.transcript_path = Some("/home/me/.claude/projects/-tmp-proj/agent-a1.jsonl".into());
    });
    let session = h.session().unwrap();
    assert_eq!(session.permission_mode.as_deref(), Some("plan"));
    assert_eq!(session.model.as_deref(), Some("claude-opus"));
    assert_eq!(session.transcript_path.as_deref(), Some(TRANSCRIPT));
}

#[test]
fn the_stop_that_leaves_agents_running_is_a_wait_not_a_review() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(vec!["workflow".into(), "shell".into()]);
    });
    let session = h.session().unwrap();
    assert_eq!(session.background_task_count, 2);
    assert_eq!(session.background_agent_count, 1);
    assert_eq!(session.background_agent_types, ["workflow"]);
    assert!(session.background_wait_since.is_some());
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(
        session.background_wait_description().as_deref(),
        Some("1 workflow")
    );
    // The turn counts as finished work at the Stop (quiet while it waits).
    assert!(session.completed_at.is_some());
    assert!(session.completion_is_quiet());
}

#[test]
fn views_are_sorted_by_project_then_session() {
    let mut h = Harness::new();
    for (id, cwd) in [
        ("b", "/x/zeta"),
        ("c", "/x/alpha"),
        ("a", "/x/zeta"),
        ("d", "/x/alpha"),
    ] {
        h.hook_with("UserPromptSubmit", "processing", |builder| {
            builder.session_id = id.into();
            builder.cwd = cwd.into();
        });
    }
    let order: Vec<String> = h
        .store
        .views()
        .iter()
        .map(|v| format!("{}:{}", v.project_name, v.id))
        .collect();
    assert_eq!(order, ["alpha:c", "alpha:d", "zeta:a", "zeta:b"]);
}

#[test]
fn changed_says_whether_anything_is_new() {
    let mut h = Harness::new();
    assert!(h.hook("UserPromptSubmit", "processing").changed);
    let tick = h.apply(SessionInput::Tick);
    assert!(!tick.changed);
}

#[test]
fn tool_calls_show_in_the_chat_and_subagent_tools_under_their_agent() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Agent".into());
        b.tool_use_id = Some("toolu_agent".into());
    });
    // A subagent's call is not a top-level item.
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.agent_id = Some("a1".into());
        b.tool = Some("Read".into());
        b.tool_use_id = Some("toolu_inner".into());
    });
    let session = h.session().unwrap();
    assert_eq!(session.chat.tool_status("toolu_agent"), Some("running"));
    assert!(session.chat.item("toolu_inner").is_none());
    assert!(session.subagent_state.has_active_subagent());
    assert_eq!(
        session.subagent_state.active_tasks["toolu_agent"].subagent_tools[0].id,
        "toolu_inner"
    );
    // The Agent returned: the task is no longer followed.
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Agent".into());
        b.tool_use_id = Some("toolu_agent".into());
    });
    let session = h.session().unwrap();
    assert!(!session.subagent_state.has_active_subagent());
    assert_eq!(session.chat.tool_status("toolu_agent"), Some("success"));
}
