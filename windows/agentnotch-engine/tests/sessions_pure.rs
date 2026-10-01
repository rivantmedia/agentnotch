//! The pure session vectors the modules' unit tests don't hold, ported from
//! the Mac's BackgroundWaitTests and A1_AttentionAndReviewTests
//! (TurnCompletion's decisionTable is already `completion::tests`, and
//! SessionTaskListTests / SessionAttentionTests are `tasks::tests` /
//! `attention::tests`).

use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{
    Bucket, NeedsInputReason, PermissionContext, PermissionSuggestion, Phase, SessionState,
};
use agentnotch_engine::sessions::attention::derive;
use agentnotch_engine::sessions::background::{
    decide, is_awaited, phrase, WaitDecision, WaitTiming,
};
use agentnotch_engine::sessions::tasks::{created_task_id, TaskList};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---- BackgroundWaitTests ----

#[test]
fn awaited_types() {
    for kind in [
        "subagent",
        "workflow",
        "teammate",
        "cloud session",
        "local_agent",
        "local_workflow",
        "in_process_teammate",
        "remote_agent",
    ] {
        assert!(is_awaited(kind), "{kind}");
    }
    for kind in [
        "shell",
        "monitor",
        "MCP task",
        "dream",
        "auto-mode scan",
        "local_bash",
        "monitor_mcp",
        "mcp_task",
    ] {
        assert!(!is_awaited(kind), "{kind}");
    }
}

#[test]
fn phrases() {
    let types = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    assert_eq!(phrase(&types(&[])), None);
    assert_eq!(phrase(&types(&["workflow"])), Some("1 workflow".into()));
    assert_eq!(
        phrase(&types(&["local_workflow", "workflow"])),
        Some("2 workflows".into())
    );
    assert_eq!(
        phrase(&types(&["subagent", "cloud session", "local_agent"])),
        Some("3 background agents".into())
    );
    assert_eq!(
        phrase(&types(&["workflow", "subagent"])),
        Some("1 workflow and 1 background agent".into())
    );
}

#[test]
fn decisions() {
    let stop = UNIX_EPOCH + Duration::from_secs(1000);
    let timing = WaitTiming {
        registry_grace: Duration::from_secs(10),
        quiet_timeout: Duration::from_secs(600),
    };
    // Offsets from the Stop, in seconds (may be negative).
    let at = |s: i64| {
        if s >= 0 {
            stop + Duration::from_secs(s as u64)
        } else {
            stop - Duration::from_secs(-s as u64)
        }
    };
    let decide = |status: Option<&str>, changed: Option<i64>, hook: Option<i64>, now: i64| {
        decide(stop, status, changed.map(at), hook.map(at), at(now), timing)
    };
    let keep = |s: u64| WaitDecision::Keep {
        recheck_in: Some(Duration::from_secs(s)),
    };
    // Idle or shell: after the grace, counted from the Stop at the earliest.
    assert_eq!(decide(Some("idle"), Some(2), None, 5), keep(7));
    assert_eq!(
        decide(Some("shell"), Some(2), None, 12),
        WaitDecision::End { at: at(2) }
    );
    assert_eq!(decide(Some("idle"), Some(-30), None, 4), keep(6));
    // Busy (or a dialog): until nothing, agents included, has been heard
    // from for a long time (a paused workflow keeps the registry busy).
    assert_eq!(decide(Some("busy"), Some(-60), Some(100), 650), keep(50));
    assert_eq!(
        decide(Some("waiting"), Some(5), Some(100), 700),
        WaitDecision::End { at: at(100) }
    );
    // No registry: the same silence.
    assert_eq!(decide(None, None, Some(-5), 599), keep(1));
    assert_eq!(
        decide(None, None, None, 600),
        WaitDecision::End { at: stop }
    );
}

#[test]
fn attention_derivation() {
    let done = SystemTime::now();
    // A turn that ended waiting on background agents is still working...
    for phase in [Phase::WaitingForInput, Phase::Idle] {
        assert_eq!(
            derive(&phase, None, Some(done), None, false, true),
            SessionState::Working
        );
    }
    // ...and without them it is finished work to review.
    assert_eq!(
        derive(
            &Phase::WaitingForInput,
            None,
            Some(done),
            None,
            false,
            false
        ),
        SessionState::ReadyForReview
    );
    // A background agent's question still comes first.
    let dialog = NeedsInputReason::Dialog {
        detail: "permission prompt".into(),
    };
    assert_eq!(
        derive(
            &Phase::WaitingForInput,
            Some(&dialog),
            Some(done),
            None,
            false,
            true
        )
        .bucket(),
        Bucket::NeedsYou
    );
}

// ---- A1_SessionStoreRegressionTests.decisionTable: completion::tests ----

// ---- A1_AttentionAndReviewTests: task batches ----

#[test]
fn a_new_batch_starts_after_every_task_is_done() {
    let mut list = TaskList::new();
    for id in 1..=3 {
        list.task_create_started(&format!("t{id}"), format!("Task {id}"), None, None);
        list.task_create_finished(&format!("t{id}"), &id.to_string(), None);
    }
    for id in 1..=3 {
        list.task_updated(&id.to_string(), Some("completed"), None, None);
    }
    assert_eq!(list.completed_count(), 3); // the review row still reads 3/3

    list.task_create_started("t4", "Task 4".into(), None, None);
    list.task_create_finished("t4", "4", None);
    list.task_created("5", Some("Task 5"));
    assert_eq!(list.total_count(), 2);
    assert_eq!(list.completed_count(), 0);

    // Unfinished lists keep growing.
    list.task_created("6", Some("Task 6"));
    assert_eq!(list.total_count(), 3);
}

#[test]
fn reconstruction_sees_the_same_batches() {
    // The Mac writes these as transcript lines and reads the file; the
    // transcript job (wp5-2) feeds the same tool_use / tool_result pairs to
    // the list, which is what this drives.
    let mut list = TaskList::new();
    for id in 1..=2 {
        list.apply_transcript_tool_use(
            &format!("c{id}"),
            "TaskCreate",
            &json!({"subject": format!("Old {id}")}),
        );
        list.apply_transcript_tool_result(
            &format!("c{id}"),
            false,
            created_task_id(&format!("Task #{id} created successfully: Old {id}")).as_deref(),
        );
        list.apply_transcript_tool_use(
            &format!("u{id}"),
            "TaskUpdate",
            &json!({"taskId": id.to_string(), "status": "completed"}),
        );
    }
    list.apply_transcript_tool_use("c3", "TaskCreate", &json!({"subject": "New"}));
    list.apply_transcript_tool_result(
        "c3",
        false,
        created_task_id("Task #3 created successfully: New").as_deref(),
    );
    let subjects: Vec<&str> = list.items().iter().map(|t| t.subject.as_str()).collect();
    assert_eq!(subjects, ["New"]);
}

// ---- A1_AttentionAndReviewTests.previewsSayWhenTheyHideSomething ----

fn context(
    tool: &str,
    input: serde_json::Value,
    suggestions: Vec<serde_json::Value>,
) -> PermissionContext {
    PermissionContext {
        tool_use_id: "t".into(),
        tool_name: tool.into(),
        tool_input: input,
        received_at: SystemTime::now(),
        permission_suggestions: suggestions,
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: None,
    }
}

#[test]
fn previews_say_when_they_hide_something() {
    let paths = Paths::new(PathStyle::Posix, "/Users/me");
    let long = "a".repeat(150) + " && git push --force";
    let bash = context("Bash", json!({ "command": long }), vec![]);
    assert_eq!(
        bash.formatted_input().unwrap().chars().count(),
        PermissionContext::PREVIEW_LENGTH + 3
    );
    assert_eq!(bash.full_input(&paths).as_deref(), Some(long.as_str()));
    assert!(bash.is_preview_truncated(&paths));

    let edit = context("Edit", json!({"file_path": "/etc/hosts"}), vec![]);
    assert_eq!(edit.formatted_input().as_deref(), Some("hosts"));
    assert_eq!(edit.full_input(&paths).as_deref(), Some("/etc/hosts"));
    assert!(edit.is_preview_truncated(&paths));

    let short = context("Bash", json!({"command": "ls"}), vec![]);
    assert!(!short.is_preview_truncated(&paths));

    let rule = json!({"type": "addRules", "destination": "userSettings", "rules": []});
    let mode = json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"});
    let session = json!({"type": "addRules", "destination": "session", "rules": []});
    let suggestion = |raw: serde_json::Value| -> Option<PermissionSuggestion> {
        context("Bash", json!({}), vec![raw]).always_allow_suggestion()
    };
    assert!(!suggestion(rule).unwrap().is_narrow());
    assert!(suggestion(mode).unwrap().changes_permission_mode());
    assert!(suggestion(session).unwrap().is_narrow());
}
