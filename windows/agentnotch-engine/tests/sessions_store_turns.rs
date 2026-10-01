//! The session store's turn completion, background waits, session registry,
//! Desktop-hosted sessions and periodic check: BackgroundWaitTests (the
//! store ones), SessionStoreFlowTests, A1_SessionStoreRegressionTests,
//! A1_ReviewFixesTests, DesktopHostedSessionsTests and
//! SessionCoreRegressionTests ported to `SessionStore::apply`, plus the
//! Windows contract (deadlines instead of timers, pid-less sessions).
//!
//! Not ported here, with their owner:
//! - the review halves (wp5-9): `aFailedTurnKeepsWaitingBehindItsError`'s
//!   dismissal, `turnCompletesIntoReviewAndPromptReviews`' last click and
//!   `theWaitSurvivesARelaunch` need the review file and `ReviewAction`;
//! - `aSessionWithoutHooksCompletesFromItsTranscript` (inferCompletion) and
//!   the transcript-driven registry tests are the transcript half (wp5-10);
//! - `theRowsSayWhatTheSessionWaitsOn`'s row text (WP7); only the phrase is
//!   checked here;
//! - `processFactsComeFromTheKernel` (WP3).

mod sessions_support;

use agentnotch_engine::model::{
    AccountId, Attribution, DesktopCandidate, IdentityId, NeedsInputReason, Phase, SessionId,
    SessionState, StatusLineMessage,
};
use agentnotch_engine::platform::Processes;
use agentnotch_engine::runtime_types::{Job, Release, SessionInput};
use agentnotch_engine::sessions::attention::humanized_stop_error;
use agentnotch_engine::sessions::background::WaitTiming;
use agentnotch_engine::sessions::completion::CompletionTiming;
use agentnotch_engine::sessions::registry::SessionsFolderGroup;
use agentnotch_engine::testkit::process::FakeProcesses;
use sessions_support::{
    registry_entry, registry_snapshot, t0, Harness, HookEventBuilder, FOLDER, PID, TRANSCRIPT,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const WAKE: &str = "<task-notification>\n<task-id>w1</task-id>\n<status>completed</status>\n<summary>Dynamic workflow \"sweep\" completed</summary>\n</task-notification>";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn s1() -> SessionId {
    SessionId::from("s1")
}

/// The Mac's `BackgroundWaitTests` store: stops complete at once unless the
/// registry follows the turn.
fn waiting(registry_grace: u64) -> Harness {
    Harness::with_timing(
        CompletionTiming::IMMEDIATE,
        WaitTiming {
            registry_grace: secs(registry_grace),
            quiet_timeout: secs(30 * 60),
        },
    )
}

/// A store whose Stops wait `fallback` for a registry that never comes.
fn slow(fallback: Duration) -> Harness {
    Harness::with_timing(
        CompletionTiming {
            fallback_delay: fallback,
            registry_timeout: secs(60),
            clock_tolerance: secs(1),
        },
        WaitTiming::STANDARD,
    )
}

/// A hook event of the harness's Claude process.
fn hook(h: &mut Harness, name: &str, status: &str, configure: impl FnOnce(&mut HookEventBuilder)) {
    h.hook_with(name, status, |b| {
        b.pid = Some(PID);
        configure(b);
    });
}

/// A typed prompt, the registry going busy with it, then a Stop.
fn turn(h: &mut Harness, prompt: &str, background: &[&str]) {
    let start = h.now;
    hook(h, "UserPromptSubmit", "processing", |b| {
        b.prompt = Some(prompt.into())
    });
    h.registry("busy", start);
    stop_with(h, background);
}

fn stop_with(h: &mut Harness, background: &[&str]) {
    hook(h, "Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(background.iter().map(|t| (*t).to_owned()).collect());
    });
}

fn is_failed_with(state: &SessionState, expected: &str) -> bool {
    matches!(state, SessionState::Failed(NeedsInputReason::Error { text, .. }) if text == expected)
}

fn status_line(percent: f64, at: SystemTime) -> StatusLineMessage {
    StatusLineMessage {
        session_id: s1(),
        cwd: Some("/tmp/proj".into()),
        transcript_path: Some(TRANSCRIPT.into()),
        config_dir_env: None,
        account_id: None,
        received_at: at,
        rate_limits: None,
        five_hour: None,
        seven_day: None,
        context_used_percent: Some(percent),
        context_window_size: Some(200_000),
        model_id: None,
        model_display_name: None,
        cost_usd: None,
        session_name: None,
        claude_code_version: None,
        pid: None,
    }
}

// ---- BackgroundWaitTests: the wait ----

#[test]
fn a_turn_waiting_on_a_workflow_is_working_until_its_result_wakes_claude() {
    let mut h = waiting(10);
    turn(&mut h, "sweep the repo", &["workflow", "shell"]);
    let session = h.session().unwrap();
    // Busy registry: the Stop isn't confirmed yet, and the wait is on.
    assert!(session.completion_pending_since.is_some());
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(
        session.background_wait_description().as_deref(),
        Some("1 workflow")
    );

    // A minute later Claude Code says it waits for input: that confirms the
    // Stop, but the workflow still runs.
    hook(&mut h, "Notification", "waiting_for_input", |b| {
        b.notification_type = Some("idle_prompt".into())
    });
    let session = h.session().unwrap();
    assert!(session.completion_pending_since.is_none());
    assert!(session.completed_at.is_some());
    assert_eq!(session.phase, Phase::WaitingForInput);
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(
        session.background_wait_description().as_deref(),
        Some("1 workflow")
    );

    // The workflow's own agents keep working after the Stop.
    hook(&mut h, "PreToolUse", "running_tool", |b| {
        b.agent_id = Some("a1".into());
        b.tool = Some("Grep".into());
    });
    assert!(h.session().unwrap().is_awaiting_background_work());

    // Its result wakes Claude: a turn of its own (the wait stands until that
    // turn's Stop says what is left).
    hook(&mut h, "UserPromptSubmit", "processing", |b| {
        b.prompt = Some(WAKE.into())
    });
    let session = h.session().unwrap();
    assert_eq!(h.state(), SessionState::Working);
    assert!(!session.is_awaiting_background_work());
    assert_eq!(session.background_wait_description(), None);

    stop_with(&mut h, &["shell"]);
    let now = h.now;
    h.registry("shell", now);
    let session = h.session().unwrap();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert!(session.background_wait_since.is_none());
    assert!(!session.completion_is_quiet());
    assert_eq!(session.background_task_count, 1);
}

/// The SDK (VS Code) may wake Claude without a UserPromptSubmit: the next
/// Stop still ends the wait, and the completion is that Stop's.
#[test]
fn a_wake_without_a_prompt_still_ends_the_wait() {
    let mut h = waiting(10);
    h.at(0);
    hook(&mut h, "UserPromptSubmit", "processing", |b| {
        b.prompt = Some("go".into())
    });
    h.at(1);
    stop_with(&mut h, &["subagent"]);
    assert_eq!(h.state(), SessionState::Working);

    h.at(30);
    stop_with(&mut h, &[]);
    let session = h.session().unwrap();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert!(session.completed_at.unwrap() >= t0() + secs(30));
}

/// A follow-up typed while a workflow runs is answered, but the session's
/// work isn't done until the workflow is.
#[test]
fn a_typed_turn_during_the_wait_keeps_waiting() {
    let mut h = waiting(10);
    turn(&mut h, "sweep the repo", &["workflow"]);
    turn(&mut h, "what does foo() do?", &["workflow"]);
    let session = h.session().unwrap();
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(
        session.background_wait_description().as_deref(),
        Some("1 workflow")
    );
    // Announced once the workflow's result has been dealt with.
    assert!(!session.completion_is_quiet());
}

/// Esc during a turn doesn't stop background agents: the wait stands.
#[test]
fn an_interrupted_turn_keeps_waiting_on_agents_still_running() {
    let mut h = waiting(0);
    turn(&mut h, "go", &["workflow", "workflow"]);
    // The first workflow's result wakes Claude; the user presses Esc.
    hook(&mut h, "UserPromptSubmit", "processing", |b| {
        b.prompt = Some(WAKE.into())
    });
    let now = h.now;
    h.interrupt(now);
    let session = h.session().unwrap();
    assert_eq!(session.phase, Phase::Idle);
    assert_eq!(h.state(), SessionState::Working);
    assert!(session.background_wait_description().is_some());

    // The second one is stopped too, without a report: the registry shows
    // nothing left.
    let now = h.now;
    h.registry("idle", now);
    let session = h.session().unwrap();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert!(session.background_wait_since.is_none());
}

/// A failed turn (the API refused) doesn't stop the agents either. The
/// dismissal that follows in the Mac test is a review action (wp5-9).
#[test]
fn a_failed_turn_keeps_waiting_behind_its_error() {
    let mut h = waiting(10);
    turn(&mut h, "go", &["workflow"]);
    hook(&mut h, "StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    });
    assert!(is_failed_with(
        &h.state(),
        &humanized_stop_error(Some("rate_limit"))
    ));
    assert!(h.session().unwrap().background_wait_since.is_some());
}

// ---- BackgroundWaitTests: ending without a wake-up ----

/// Agents stopped (or a report that never came): the registry shows no agent
/// left, and after a short grace the work is done and announced.
#[test]
fn the_registry_ends_a_wait_no_wake_up_ends() {
    let mut h = waiting(0);
    turn(&mut h, "go", &["subagent", "subagent", "shell"]);
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(
        h.session()
            .unwrap()
            .background_wait_description()
            .as_deref(),
        Some("2 background agents")
    );

    // Only the shell is left.
    let now = h.now;
    h.registry("shell", now);
    let session = h.session().unwrap();
    assert_eq!(session.phase, Phase::WaitingForInput);
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(session.background_agent_count, 0);
    assert_eq!(session.background_task_count, 1);
    assert!(!session.completion_is_quiet());
}

#[test]
fn the_registry_waits_out_its_grace() {
    let mut h = waiting(1);
    turn(&mut h, "go", &["workflow"]);
    let idle_at = h.now;
    h.registry("idle", idle_at);
    assert_eq!(h.state(), SessionState::Working);
    h.run_until(idle_at + ms(900));
    assert_eq!(h.state(), SessionState::Working);
    // The wait is the next thing the clock settles.
    assert_eq!(h.store.next_deadline(), Some(idle_at + secs(1)));
    h.run_until(idle_at + secs(1));
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

/// Idle teammates don't keep the registry busy: a lead's typed turn is
/// announced after the grace, not held until the teammates report.
#[test]
fn idle_teammates_end_the_wait_through_the_registry() {
    let mut h = waiting(0);
    turn(&mut h, "spawn a team", &["teammate", "teammate"]);
    h.advance(1);
    let busy_at = h.now - ms(500);
    h.registry("busy", busy_at);
    assert_eq!(h.state(), SessionState::Working);
    let now = h.now;
    h.registry("idle", now);
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert!(!h.session().unwrap().completion_is_quiet());
}

#[test]
fn shells_monitors_and_housekeeping_do_not_hold_the_session() {
    let mut h = waiting(10);
    hook(&mut h, "UserPromptSubmit", "processing", |b| {
        b.prompt = Some("start the dev server".into())
    });
    stop_with(
        &mut h,
        &["shell", "monitor", "dream", "MCP task", "auto-mode scan"],
    );
    let session = h.session().unwrap();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert!(session.background_wait_since.is_none());
    assert_eq!(session.background_task_count, 5);
}

#[test]
fn the_rows_say_what_the_session_waits_on() {
    let mut h = waiting(10);
    turn(&mut h, "go", &["workflow", "subagent", "teammate"]);
    let view = h.view().unwrap();
    assert_eq!(view.state, SessionState::Working);
    assert_eq!(
        view.background_wait_description.as_deref(),
        Some("1 workflow, 1 background agent and 1 teammate")
    );
}

/// Woken for a turn of its own, the row shows that turn's progress.
#[test]
fn a_woken_turn_shows_its_own_progress() {
    let mut h = waiting(10);
    turn(&mut h, "go", &["workflow"]);
    hook(&mut h, "UserPromptSubmit", "processing", |b| {
        b.prompt = Some(WAKE.into())
    });
    assert_eq!(h.view().unwrap().background_wait_description, None);
    assert_eq!(h.state(), SessionState::Working);
}

// ---- SessionStoreFlowTests ----

#[test]
fn a_turn_completes_into_review_and_a_user_prompt_is_the_user_looking() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    assert_eq!(h.state(), SessionState::Working);

    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Done: tests pass".into())
    });
    let finished = h.session().unwrap();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(
        finished.last_assistant_message.as_deref(),
        Some("Done: tests pass")
    );
    assert!(finished.completed_at.is_some());

    // idle_prompt must not downgrade the review state.
    h.hook_with("Notification", "waiting_for_input", |b| {
        b.notification_type = Some("idle_prompt".into())
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // A loop/cron turn is not the user looking.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("loop_wakeup".into())
    });
    h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // The user's own prompt is (the click that marks it reviewed is wp5-9's).
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    assert_eq!(h.state(), SessionState::Working);
}

#[test]
fn a_stop_with_background_tasks_is_ready_for_review() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_count = Some(1)
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(h.session().unwrap().background_task_count, 1);
}

#[test]
fn the_registry_reconciles_interrupts_and_dialogs() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");

    // Waiting in the registry while we think it's working: a dialog is open.
    let mut waiting = registry_entry("s1", PID, "waiting", h.now + secs(1));
    waiting.waiting_for = Some("input needed".into());
    h.registry_entries(FOLDER, false, vec![waiting]);
    assert_eq!(
        h.state(),
        SessionState::NeedsYou(NeedsInputReason::Dialog {
            detail: "input needed".into()
        })
    );

    // Idle without a Stop: interrupted.
    let idle_at = h.now + secs(2);
    h.registry("idle", idle_at);
    let interrupted = h.session().unwrap();
    assert_eq!(interrupted.phase, Phase::Idle);
    assert!(interrupted.completed_at.is_none());

    // Older registry data never overrides newer hook state.
    h.hook("UserPromptSubmit", "processing");
    let stale_at = h.now - secs(60);
    h.registry("idle", stale_at);
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
}

#[test]
fn the_registry_creates_unknown_sessions() {
    let mut h = Harness::new();
    let mut entry = registry_entry("s1", PID, "busy", h.now);
    entry.entrypoint = Some("claude-vscode".into());
    entry.name = Some("Registry name".into());
    h.registry_entries("/home/me/.claude-work", false, vec![entry]);
    let created = h.session().unwrap();
    assert_eq!(created.phase, Phase::Processing);
    assert_eq!(created.entrypoint.as_deref(), Some("claude-vscode"));
    assert_eq!(
        created.account,
        Some(AccountId("/home/me/.claude-work".into()))
    );
    assert_eq!(created.display_title(), "Registry name");
    // No hook or status line gave it an identity: the hub attributes it.
    assert_eq!(created.attribution, Attribution::Known(None));
    assert_eq!(created.pid, Some(PID));

    // A hook title outranks the registry name.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.session_title = Some("Hook title".into())
    });
    assert_eq!(h.session().unwrap().display_title(), "Hook title");
}

// ---- A1_SessionStoreRegressionTests: Stop hooks that continue the turn ----

#[test]
fn a_stop_is_done_only_when_the_registry_says_the_turn_ended() {
    let mut h = slow(secs(60));
    let at = |ms_after: u64| t0() + ms(ms_after);
    h.now = at(0);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.now = at(50);
    h.registry("busy", at(50));

    // Our Stop hook ran; a blocking Stop hook (/goal) is still deciding.
    h.now = at(2000);
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Step one done".into())
    });
    let current = h.session().unwrap();
    assert_eq!(h.state(), SessionState::Working);
    assert!(current.completed_at.is_none());
    // The registry is read again soon (0.3 s after the Stop).
    assert_eq!(h.store.next_deadline(), Some(at(2300)));

    // It blocked: Claude continues without a prompt.
    h.now = at(3000);
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_c1".into());
    });
    assert!(h.session().unwrap().completion_pending_since.is_none());
    assert_eq!(h.state(), SessionState::Working);

    // The continuation ends; this time nothing blocks and Claude Code goes
    // idle.
    let final_stop = at(5000);
    h.now = final_stop;
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("All steps done".into());
        b.stop_hook_active = Some(true);
    });
    assert_eq!(h.state(), SessionState::Working);
    h.now = at(5100);
    h.registry("idle", at(5100));
    let current = h.session().unwrap();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(current.completed_at, Some(final_stop));
    assert_eq!(
        current.last_assistant_message.as_deref(),
        Some("All steps done")
    );
}

#[test]
fn a_text_only_continuation_ends_with_the_last_stop() {
    let mut h = slow(secs(60));
    let at = |ms_after: u64| t0() + ms(ms_after);
    h.now = at(0);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.now = at(50);
    h.registry("busy", at(50));
    h.now = at(1000);
    h.hook("Stop", "waiting_for_input");
    // A blocking Stop hook made Claude write more text, then stop again: no
    // event in between, the second Stop carries stop_hook_active.
    let second = at(4000);
    h.now = second;
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Goal met".into());
        b.stop_hook_active = Some(true);
    });
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(h.session().unwrap().completion_pending_since, Some(second));

    h.now = at(4200);
    h.registry("idle", at(4200));
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(h.session().unwrap().completed_at, Some(second));
}

#[test]
fn without_a_registry_a_stop_completes_after_a_quiet_moment() {
    let mut h = slow(ms(200));
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    let stop_at = h.now;
    h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(h.store.next_deadline(), Some(stop_at + ms(200)));
    h.run_until(stop_at + ms(199));
    assert_eq!(h.state(), SessionState::Working);
    h.run_until(stop_at + ms(200));
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(h.session().unwrap().completed_at, Some(stop_at));
}

#[test]
fn an_idle_notification_confirms_a_pending_stop() {
    let mut h = slow(secs(60));
    h.hook("UserPromptSubmit", "processing");
    h.hook("Stop", "waiting_for_input");
    assert_eq!(h.state(), SessionState::Working);
    h.hook_with("Notification", "waiting_for_input", |b| {
        b.notification_type = Some("idle_prompt".into())
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

#[test]
fn a_registry_that_follows_the_turn_confirms_after_its_timeout() {
    let mut h = slow(secs(4));
    h.hook("UserPromptSubmit", "processing");
    h.registry("busy", t0());
    let stop_at = h.now;
    h.hook("Stop", "waiting_for_input");
    // The registry follows the turn: the 4 s fallback doesn't apply.
    h.run_until(stop_at + secs(30));
    assert_eq!(h.state(), SessionState::Working);
    h.run_until(stop_at + secs(60));
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

// ---- A1_SessionStoreRegressionTests: quiet completions ----

#[test]
fn turns_waiting_on_agents_or_loops_are_quiet() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(vec!["subagent".into(), "shell".into()])
    });
    let current = h.session().unwrap();
    // The subagent will wake Claude: not done yet (see the wait tests).
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(current.background_task_count, 2);
    assert_eq!(current.background_agent_count, 1);
    assert!(current.completion_is_quiet());

    // A dev server alone isn't waited on: that turn is done.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(vec!["shell".into()])
    });
    assert!(!h.session().unwrap().completion_is_quiet());
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // A /loop tick.
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("loop_wakeup".into())
    });
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.session_cron_count = Some(1)
    });
    let current = h.session().unwrap();
    assert!(current.completion_is_quiet());
    assert_eq!(current.scheduled_wakeup_count, 1);
}

// ---- A1_SessionStoreRegressionTests: registry reconciliation ----

#[test]
fn status_line_updates_do_not_hide_a_registry_correction() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.hook("UserPromptSubmit", "processing");
    h.registry("busy", t0() + ms(100));
    let interrupted_at = t0() + secs(1);
    h.now = t0() + secs(2);
    h.status_line(status_line(12.0, t0() + secs(2)));
    h.registry("idle", interrupted_at);
    assert_eq!(h.session().unwrap().phase, Phase::Idle);
}

#[test]
fn a_synthetic_request_answered_in_the_terminal_is_resolved_by_the_registry() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.hook("UserPromptSubmit", "processing");
    h.registry("busy", t0() + ms(100));
    h.now = t0() + secs(1);
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("permission-x".into());
        b.synthetic = true;
    });
    let mut waiting = registry_entry("s1", PID, "waiting", t0() + ms(1200));
    waiting.waiting_for = Some("permission prompt".into());
    h.registry_entries(FOLDER, false, vec![waiting]);
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "permission-x"
    );

    h.registry("busy", t0() + secs(5));
    assert!(h.session().unwrap().active_permission().is_none());
    assert_eq!(h.state(), SessionState::Working);
    assert!(h.releases.contains(&Release::Request {
        session: s1(),
        tool_use_id: "permission-x".into()
    }));
}

/// Answered in the terminal while parallel calls kept firing hooks: the
/// registry's "busy" arrives after those hooks, and still settles it.
#[test]
fn a_synthetic_request_is_settled_by_the_registry_even_after_later_hooks() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.registry("busy", t0() + ms(100));
    h.now = t0() + ms(500);
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_ls".into());
    });
    h.now = t0() + secs(1);
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("permission-x".into());
        b.synthetic = true;
    });
    // The parallel call finishes after the user answered at t0+2.
    h.now = t0() + secs(3);
    h.hook_with("PostToolUse", "processing", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_ls".into());
    });
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "permission-x"
    );

    h.registry("busy", t0() + secs(2));
    assert!(h.session().unwrap().active_permission().is_none());
    assert_eq!(h.state(), SessionState::Working);
    assert!(h.releases.contains(&Release::Request {
        session: s1(),
        tool_use_id: "permission-x".into()
    }));
}

/// A background agent's request shows no dialog while our hook waits, so the
/// main session's registry status says nothing about it.
#[test]
fn a_background_agents_synthetic_request_ignores_the_registry() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.now = t0() + secs(1);
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.agent_id = Some("bg-1".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("permission-bg".into());
        b.synthetic = true;
    });
    h.registry("busy", t0() + secs(4));
    assert_eq!(
        h.session()
            .unwrap()
            .active_permission()
            .unwrap()
            .tool_use_id,
        "permission-bg"
    );
    assert!(!h
        .releases
        .iter()
        .any(|release| matches!(release, Release::Request { .. })));
}

#[test]
fn a_pid_now_naming_another_session_drops_the_old_one() {
    let mut h = Harness::new();
    let t = h.now;
    h.registry_of("old", PID, "idle", t);
    assert!(h.store.session(&SessionId::from("old")).is_some());
    h.registry_of("new", PID, "idle", t + secs(1));
    assert!(h.store.session(&SessionId::from("old")).is_none());
    assert!(h.store.session(&SessionId::from("new")).is_some());
    // The old one can't come back from a late snapshot.
    h.registry_of("old", PID + 1, "idle", t + secs(2));
    assert!(h.store.session(&SessionId::from("old")).is_none());
}

#[test]
fn a_hook_session_is_not_dropped_by_a_stale_registry_entry() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.registry_of("s1", PID, "idle", t0());
    assert!(h.session().is_some());
    // /clear: the new session's hooks arrive before the registry catches up.
    h.now = t0() + secs(2);
    h.hook_with("SessionStart", "waiting_for_input", |b| {
        b.session_id = "s2".into();
        b.pid = Some(PID);
        b.source = Some("clear".into());
    });
    h.registry_of("s1", PID, "idle", t0() + ms(500));
    assert!(h.store.session(&SessionId::from("s2")).is_some());
}

#[test]
fn registry_entries_of_other_kinds_and_dead_processes_are_not_sessions() {
    let mut h = Harness::new();
    let at = h.now;
    let mut daemon = registry_entry("d1", 11, "idle", at);
    daemon.kind = Some("daemon".into());
    let mut sdk = registry_entry("d2", 12, "idle", at);
    sdk.entrypoint = Some("sdk-ts".into());
    let mut dead = registry_entry("d3", 13, "idle", at);
    dead.live = false;
    let good = registry_entry("d4", 14, "idle", at);
    h.registry_entries(FOLDER, false, vec![daemon, sdk, dead, good]);
    let views = h.store.views();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, SessionId::from("d4"));
}

#[test]
fn a_new_session_is_made_from_each_status() {
    let mut h = Harness::new();
    let at = h.now;
    let mut dialog = registry_entry("w1", 21, "waiting", at);
    dialog.waiting_for = Some("input needed".into());
    h.registry_entries(
        FOLDER,
        false,
        vec![
            registry_entry("b1", 20, "busy", at),
            dialog,
            registry_entry("i1", 22, "idle", at),
        ],
    );
    let session = |id: &str| h.store.session(&SessionId::from(id)).unwrap();
    assert_eq!(session("b1").phase, Phase::Processing);
    assert_eq!(session("b1").turn_started_at, Some(at));
    assert_eq!(session("w1").phase, Phase::WaitingForInput);
    assert_eq!(
        session("w1").needs_input_reason(),
        Some(&NeedsInputReason::Dialog {
            detail: "input needed".into()
        })
    );
    assert_eq!(session("i1").phase, Phase::Idle);
    assert_eq!(session("i1").registry_status.as_deref(), Some("idle"));
    assert_eq!(session("i1").registry_status_changed_at, Some(at));
}

#[test]
fn an_idle_session_from_before_the_launch_may_have_finished_a_turn() {
    let mut h = Harness::new();
    let launch = t0() + secs(100);
    let previous = t0() + secs(40);
    h.store.set_launch(launch, Some(previous));
    h.now = launch;
    // Idle since just before the launch (within the grace): checked.
    let before = registry_entry("old", 31, "idle", launch - secs(30));
    let edge = registry_entry("edge", 32, "idle", launch + secs(4));
    // Went idle while this run was up: not "while the app was down".
    let after = registry_entry("late", 33, "idle", launch + secs(6));
    h.registry_entries(FOLDER, false, vec![before, edge, after]);
    let check = |id: &str| {
        h.store
            .session(&SessionId::from(id))
            .unwrap()
            .completion_check_since
    };
    assert_eq!(check("old"), Some(previous));
    assert_eq!(check("edge"), Some(previous));
    assert_eq!(check("late"), None);
}

#[test]
fn a_session_without_hooks_that_goes_idle_waits_for_the_transcript() {
    let mut h = Harness::new();
    let at = h.now;
    h.registry("busy", at);
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
    h.registry("idle", at + secs(5));
    let session = h.session().unwrap();
    assert_eq!(session.phase, Phase::WaitingForInput);
    // The turn began when the registry first said busy.
    assert_eq!(session.completion_check_since, Some(at));
    assert!(!session.is_hook_backed());
}

#[test]
fn a_dialog_the_registry_closes_is_cleared() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    let mut waiting = registry_entry("s1", PID, "waiting", h.now + secs(1));
    waiting.waiting_for = Some("dialog open".into());
    h.registry_entries(FOLDER, false, vec![waiting]);
    assert!(matches!(h.state(), SessionState::NeedsYou(_)));
    let at = h.now + secs(2);
    h.registry("busy", at);
    assert_eq!(h.state(), SessionState::Working);
}

#[test]
fn the_registry_idle_drops_the_approvals_of_a_session_whose_dialog_is_gone() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.hook("UserPromptSubmit", "processing");
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_a".into());
    });
    assert!(matches!(h.state(), SessionState::NeedsYou(_)));
    h.registry("idle", t0() + secs(2));
    assert_eq!(h.session().unwrap().phase, Phase::Idle);
    assert!(h.releases.contains(&Release::MainAgent(s1())));
}

// ---- attribution of the registry's entries ----

#[test]
fn a_shared_folders_entries_go_to_the_folder_their_process_runs_in() {
    let mut h = Harness::new();
    h.store.set_registry_folders(vec![SessionsFolderGroup {
        folder: PathBuf::from("/home/me/.claude-shared/sessions"),
        aliases: vec!["/home/me/.claude".into(), "/home/me/.claude-work".into()],
    }]);
    let at = h.now;
    let mut a = registry_entry("a", 41, "idle", at);
    a.process_config_dir = Some(agentnotch_engine::platform::EnvRead::Set(
        "/home/me/.claude-work".into(),
    ));
    let mut b = registry_entry("b", 42, "idle", at);
    b.process_config_dir = Some(agentnotch_engine::platform::EnvRead::Unset);
    let mut c = registry_entry("c", 43, "idle", at);
    c.process_config_dir = Some(agentnotch_engine::platform::EnvRead::Unreadable);
    h.registry_entries("/home/me/.claude-shared", true, vec![a, b, c]);
    let account = |id: &str| {
        h.store
            .session(&SessionId::from(id))
            .unwrap()
            .account
            .clone()
            .unwrap()
            .0
    };
    assert_eq!(account("a"), "/home/me/.claude-work");
    assert_eq!(account("b"), "/home/me/.claude");
    assert_eq!(account("c"), "/home/me/.claude");
}

#[test]
fn an_unreadable_folder_says_nothing() {
    let mut h = Harness::new();
    let at = h.now;
    h.registry("idle", at);
    let mut snapshot = registry_snapshot(FOLDER, false, Vec::new(), at);
    snapshot.error = Some("PermissionDenied".into());
    h.apply(SessionInput::Registry(snapshot));
    assert!(h.session().is_some());
    // Nor does an empty read: the process check drops a gone session.
    h.apply(SessionInput::Registry(registry_snapshot(
        FOLDER,
        false,
        Vec::new(),
        at,
    )));
    assert!(h.session().is_some());
}

// ---- DesktopHostedSessionsTests ----

const HOST: &str = "local_0123abcd-4567-89ef";

fn hosted_entry(
    entrypoint: &str,
    status: &str,
    at: SystemTime,
    host: bool,
) -> agentnotch_engine::model::RegistryEntry {
    let mut entry = registry_entry("s1", PID, status, at);
    entry.entrypoint = Some(entrypoint.into());
    entry.host_session_id = host.then(|| HOST.to_owned());
    entry
}

/// The session follows its current process's entry: resumed outside Claude
/// Desktop, it has no Desktop id any more.
#[test]
fn the_session_keeps_its_processs_desktop_id() {
    let mut h = Harness::new();
    let at = h.now;
    h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("claude-desktop", "idle", at, true)],
    );
    assert_eq!(h.session().unwrap().host_session_id.as_deref(), Some(HOST));
    h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("cli", "busy", at + secs(1), false)],
    );
    assert_eq!(h.session().unwrap().host_session_id, None);
}

/// The session takes its current process's entrypoint too, as it takes its
/// Desktop id: resumed by Claude Desktop in a new process, it is
/// Desktop-hosted; resumed in a terminal, it isn't.
#[test]
fn the_session_takes_its_new_processs_entrypoint() {
    let mut h = Harness::new();
    let at = h.now;
    h.registry_entries(FOLDER, false, vec![hosted_entry("cli", "idle", at, false)]);
    assert_eq!(h.session().unwrap().entrypoint.as_deref(), Some("cli"));
    let mut desktop = hosted_entry("claude-desktop", "busy", at + secs(1), true);
    desktop.pid = PID + 1;
    h.registry_entries(FOLDER, false, vec![desktop]);
    let hosted = h.session().unwrap();
    assert_eq!(hosted.entrypoint.as_deref(), Some("claude-desktop"));
    assert_eq!(hosted.host_session_id.as_deref(), Some(HOST));
    assert!(h.view().unwrap().is_desktop_hosted);

    h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("cli", "idle", at + secs(2), false)],
    );
    let resumed = h.session().unwrap();
    assert_eq!(resumed.entrypoint.as_deref(), Some("cli"));
    assert_eq!(resumed.host_session_id, None);
    assert!(!h.view().unwrap().is_desktop_hosted);

    // An entry that names no entrypoint keeps the one known.
    let mut unnamed = registry_entry("s1", PID, "busy", at + secs(3));
    unnamed.entrypoint = None;
    h.registry_entries(FOLDER, false, vec![unnamed]);
    assert_eq!(h.session().unwrap().entrypoint.as_deref(), Some("cli"));
}

fn candidate(account: &str) -> DesktopCandidate {
    DesktopCandidate {
        identity_id: IdentityId::from(format!("identity-{account}").as_str()),
        account_uuid: account.into(),
        organization_uuid: None,
    }
}

#[test]
fn a_hosted_session_asks_for_its_identity_once_and_keeps_the_answer() {
    let mut h = Harness::new();
    let roots = vec![PathBuf::from("/home/me/desktop/Claude")];
    h.store.set_desktop_roots(roots.clone());
    h.store.set_desktop_candidates(vec![candidate("acct-1")]);
    let at = h.now;
    let effects = h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("claude-desktop", "idle", at, true)],
    );
    let asked: Vec<&Job> = effects
        .jobs
        .iter()
        .filter(|job| matches!(job, Job::DesktopHosted { .. }))
        .collect();
    assert_eq!(
        asked,
        [&Job::DesktopHosted {
            roots,
            host_session_id: HOST.into(),
            candidates: vec![candidate("acct-1")],
        }]
    );
    // A snapshot while the question is out asks nothing more.
    let effects = h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("claude-desktop", "idle", at, true)],
    );
    assert!(!effects
        .jobs
        .iter()
        .any(|job| matches!(job, Job::DesktopHosted { .. })));
    assert_eq!(h.session().unwrap().desktop_identity, None);

    // The answer names the identity, and stays.
    let identity = IdentityId::from("identity-acct-1");
    h.apply(SessionInput::Hosted {
        session: s1(),
        identity: Some(identity.clone()),
    });
    assert_eq!(
        h.session().unwrap().desktop_identity,
        Some(identity.clone())
    );
    assert_eq!(h.view().unwrap().desktop_identity, Some(identity.clone()));
    let effects = h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("claude-desktop", "busy", at + secs(60), true)],
    );
    assert!(effects.jobs.is_empty());
    assert_eq!(h.session().unwrap().desktop_identity, Some(identity));

    // New candidates are looked at again; a session no longer hosted has no
    // identity.
    h.store
        .set_desktop_candidates(vec![candidate("acct-1"), candidate("acct-2")]);
    let effects = h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("claude-desktop", "busy", at + secs(61), true)],
    );
    assert_eq!(
        effects
            .jobs
            .iter()
            .filter(|job| matches!(job, Job::DesktopHosted { .. }))
            .count(),
        1
    );
    h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("cli", "idle", at + secs(62), false)],
    );
    assert_eq!(h.session().unwrap().desktop_identity, None);
}

#[test]
fn a_miss_is_asked_again_after_a_while_and_a_bad_id_never() {
    let mut h = Harness::new();
    h.store
        .set_desktop_roots(vec![PathBuf::from("/home/me/desktop/Claude")]);
    h.store.set_desktop_candidates(vec![candidate("acct-1")]);
    let at = h.now;
    h.registry_entries(
        FOLDER,
        false,
        vec![hosted_entry("claude-desktop", "idle", at, true)],
    );
    h.apply(SessionInput::Hosted {
        session: s1(),
        identity: None,
    });
    h.now = at + secs(10);
    let asked = h.tick().jobs;
    assert!(!asked.iter().any(|j| matches!(j, Job::DesktopHosted { .. })));
    h.now = at + secs(20);
    let asked = h.tick().jobs;
    assert!(asked.iter().any(|j| matches!(j, Job::DesktopHosted { .. })));

    let mut bad = hosted_entry("claude-desktop", "idle", at, true);
    bad.host_session_id = Some("../x".into());
    bad.session_id = "s9".into();
    bad.pid = 77;
    let effects = h.registry_entries(FOLDER, false, vec![bad]);
    assert!(!effects.jobs.iter().any(
        |j| matches!(j, Job::DesktopHosted { host_session_id, .. } if host_session_id == "../x")
    ));
}

// ---- SessionCoreRegressionTests ----

#[test]
fn automatic_compaction_mid_turn_keeps_working_and_still_completes() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("PreCompact", "compacting", |b| {
        b.trigger = Some("auto".into())
    });
    h.hook_with("PostCompact", "processing", |b| {
        b.trigger = Some("auto".into())
    });
    // Claude Code runs SessionStart(compact) after every compaction.
    h.hook_with("SessionStart", "waiting_for_input", |b| {
        b.source = Some("compact".into())
    });
    assert_eq!(h.session().unwrap().phase, Phase::Processing);
    assert_eq!(h.state(), SessionState::Working);

    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Migrated all 40 files.".into())
    });
    let finished = h.session().unwrap();
    assert!(finished.completed_at.is_some());
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

// ---- the periodic check ----

fn with_processes(h: Harness) -> (Harness, Arc<FakeProcesses>) {
    let processes = Arc::new(FakeProcesses::default());
    let shared: Arc<dyn Processes> = processes.clone();
    let mut h = h;
    h.store = h.store.with_processes(shared);
    (h, processes)
}

#[test]
fn a_reused_pid_ends_the_session() {
    let (mut h, processes) = with_processes(Harness::new());
    let current_start = t0() - secs(500);
    processes.add(PID, 1, "claude.exe", current_start);
    // "reused" was first seen with another start time; "current" with this
    // process's own.
    h.ctx.trusted_pid = Some(PID);
    h.ctx.pid_started = Some(t0() - secs(3600));
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.session_id = "reused".into();
        b.pid = Some(PID);
    });
    h.ctx.pid_started = Some(current_start);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.session_id = "current".into();
        b.pid = Some(PID);
    });
    h.advance(3);
    h.tick();
    assert!(h.store.session(&SessionId::from("reused")).is_none());
    assert!(h.store.session(&SessionId::from("current")).is_some());
    // Late data can't bring the reused one back.
    assert!(h
        .releases
        .contains(&Release::Session(SessionId::from("reused"))));
}

#[test]
fn a_start_time_within_a_second_is_the_same_process() {
    let (mut h, processes) = with_processes(Harness::new());
    let start = t0() - secs(500);
    processes.add(PID, 1, "claude.exe", start + ms(900));
    h.ctx.trusted_pid = Some(PID);
    h.ctx.pid_started = Some(start);
    h.hook("UserPromptSubmit", "processing");
    h.advance(3);
    h.tick();
    assert!(h.session().is_some());
    processes.add(PID, 1, "claude.exe", start + ms(1000));
    h.advance(3);
    h.tick();
    assert!(h.session().is_none());
}

#[test]
fn a_session_whose_process_is_gone_ends() {
    let (mut h, processes) = with_processes(Harness::new());
    processes.add(PID, 1, "claude.exe", t0() - secs(10));
    h.ctx.trusted_pid = Some(PID);
    h.hook("UserPromptSubmit", "processing");
    h.advance(3);
    h.tick();
    assert!(h.session().is_some());
    processes.remove(PID);
    // Not before the 3 s are up.
    h.advance(1);
    h.tick();
    assert!(h.session().is_some());
    h.advance(3);
    let effects = h.tick();
    assert!(h.session().is_none());
    assert!(effects.changed);
    assert!(h.releases.contains(&Release::Session(s1())));
}

#[test]
fn a_store_with_no_process_access_never_finds_one_gone() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| b.pid = Some(PID));
    h.advance(3600);
    h.tick();
    assert!(h.session().is_some());
}

#[test]
fn a_status_line_only_session_is_dropped_after_15_minutes_of_silence() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.status_line(status_line(10.0, t0()));
    assert!(h.session().is_some());
    assert_eq!(h.session().unwrap().pid, None);
    h.now = t0() + secs(14 * 60);
    h.tick();
    assert!(h.session().is_some());
    h.now = t0() + secs(15 * 60) + secs(1);
    h.tick();
    assert!(h.session().is_none());
    // Its late status lines don't bring it back.
    h.status_line(status_line(11.0, h.now));
    assert!(h.session().is_none());
}

#[test]
fn a_hook_session_with_a_null_pid_is_dropped_the_same_way() {
    let mut h = Harness::new();
    h.step = Duration::ZERO;
    h.now = t0();
    h.hook("UserPromptSubmit", "processing");
    assert_eq!(h.session().unwrap().pid, None);
    h.now = t0() + secs(14 * 60);
    h.tick();
    assert!(h.session().is_some());
    // Any hook counts as life.
    h.hook("PreToolUse", "running_tool");
    h.now = t0() + secs(14 * 60) + secs(14 * 60);
    h.tick();
    assert!(h.session().is_some());
    h.now += secs(2 * 60);
    h.tick();
    assert!(h.session().is_none());
}

#[test]
fn ended_sessions_are_forgotten_after_ten_minutes() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    h.hook("SessionEnd", "ended");
    assert!(h.session().is_none());
    h.status_line(status_line(10.0, h.now));
    assert!(h.session().is_none());
    h.advance(11 * 60);
    h.tick();
    h.status_line(status_line(10.0, h.now));
    assert!(h.session().is_some());
}

// ---- quick rescans, deadlines ----

#[test]
fn a_stop_has_the_registry_read_again_at_a_third_and_at_one_and_a_fifth_seconds() {
    let mut h = slow(secs(60));
    h.hook("UserPromptSubmit", "processing");
    let stop_at = h.now;
    h.hook("Stop", "waiting_for_input");
    // A second Stop while the reads are pending changes nothing.
    h.now = stop_at + ms(100);
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.stop_hook_active = Some(true)
    });
    let jobs: Vec<Job> = h
        .run_until(stop_at + secs(2))
        .into_iter()
        .filter(|job| matches!(job, Job::ReadRegistry { .. }))
        .collect();
    let read = Job::ReadRegistry {
        sessions_dir: PathBuf::from(format!("{FOLDER}/sessions")),
        via_link: false,
    };
    assert_eq!(jobs, [read.clone(), read.clone()]);
    // Another Stop after they are done starts them again.
    let again = h.now;
    h.hook("Stop", "waiting_for_input");
    let jobs: Vec<Job> = h
        .run_until(again + ms(310))
        .into_iter()
        .filter(|job| matches!(job, Job::ReadRegistry { .. }))
        .collect();
    assert_eq!(jobs, [read]);
}

#[test]
fn a_late_tick_reads_the_registry_once_for_both() {
    let mut h = slow(secs(60));
    h.hook("UserPromptSubmit", "processing");
    let stop_at = h.now;
    h.hook("Stop", "waiting_for_input");
    h.now = stop_at + secs(5);
    let jobs = h.tick().jobs;
    assert_eq!(
        jobs.iter()
            .filter(|job| matches!(job, Job::ReadRegistry { .. }))
            .count(),
        1
    );
    assert!(h.tick().jobs.is_empty());
}

#[test]
fn next_deadline_is_the_earliest_pending_check() {
    let mut h = slow(secs(60));
    assert_eq!(h.store.next_deadline(), None);
    h.now = t0();
    hook(&mut h, "UserPromptSubmit", "processing", |_| {});
    // Only the 3 s check: nothing pends.
    assert_eq!(h.store.next_deadline(), Some(t0() + secs(3)));

    // A Stop no registry follows: the registry is read again at 0.3 s and
    // 1.2 s, ahead of the 3 s check and of the fallback.
    h.now = t0() + secs(1);
    hook(&mut h, "Stop", "waiting_for_input", |_| {});
    assert_eq!(h.store.next_deadline(), Some(t0() + secs(1) + ms(300)));
    h.run_until(t0() + secs(1) + ms(500));
    assert_eq!(h.store.next_deadline(), Some(t0() + secs(2) + ms(200)));
    h.run_until(t0() + secs(2) + ms(200));
    assert_eq!(h.store.next_deadline(), Some(t0() + secs(3)));
    h.run_until(t0() + secs(3));
    // The check ran: the next one is at 6 s, ahead of the fallback's 61 s.
    assert_eq!(h.store.next_deadline(), Some(t0() + secs(6)));

    // A wait on agents adds its own: 30 minutes without a hook.
    hook(&mut h, "UserPromptSubmit", "processing", |_| {});
    hook(&mut h, "Stop", "waiting_for_input", |b| {
        b.background_task_types = Some(vec!["workflow".into()])
    });
    h.run_until(t0() + secs(10 * 60));
    assert_eq!(h.state(), SessionState::Working);
    h.run_until(t0() + secs(31 * 60));
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

#[test]
fn removing_the_last_session_stops_the_deadlines() {
    let mut h = Harness::new();
    h.hook("UserPromptSubmit", "processing");
    assert!(h.store.next_deadline().is_some());
    h.hook("SessionEnd", "ended");
    assert_eq!(h.store.next_deadline(), None);
}
