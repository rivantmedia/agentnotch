//! Frame decoding (DESIGN-WIN §1.4 "Server dispatch", HS§4.2): ported from
//! the Mac's HookEventDecodingTests (the parts that are decoding; phases are
//! the session store's), plus the Windows fields.

use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::ingress::{decode, fill_status_line_account, Decoded};
use agentnotch_engine::model::{HookEvent, StatusLineMessage};
use agentnotch_proto::{ControlOp, Decision, PermissionResponse};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

fn decoded(value: Value) -> Decoded {
    decode(&serde_json::to_vec(&value).unwrap(), at())
}

fn hook(value: Value) -> HookEvent {
    match decoded(value) {
        Decoded::Hook(event) => *event,
        other => panic!("not a hook event: {other:?}"),
    }
}

fn status(value: Value) -> StatusLineMessage {
    match decoded(value) {
        Decoded::StatusLine(message) => *message,
        other => panic!("not a status line: {other:?}"),
    }
}

/// HookEventDecodingTests.decodesBaseAndNewFields.
#[test]
fn decodes_base_and_new_fields() {
    let event = hook(json!({
        "event": "Stop",
        "session_id": "abc",
        "cwd": "/Users/me/proj",
        "transcript_path": "/Users/me/.claude-work/projects/-Users-me-proj/abc.jsonl",
        "pid": 4242,
        "tty": "/dev/ttys003",
        "status": "waiting_for_input",
        "config_dir_env": "/Users/me/.claude-work",
        "attended": true,
        "entrypoint": "cli",
        "agent_id": null,
        "permission_mode": "default",
        "last_assistant_message": "All done.",
        "background_task_count": 2,
    }));
    assert_eq!(event.session_id.as_str(), "abc");
    assert_eq!(event.pid, Some(4242));
    assert!(event
        .transcript_path
        .as_deref()
        .is_some_and(|p| p.ends_with("abc.jsonl")));
    assert_eq!(
        event.config_dir_env.as_deref(),
        Some("/Users/me/.claude-work")
    );
    assert_eq!(event.attended, Some(true));
    assert_eq!(event.entrypoint.as_deref(), Some("cli"));
    assert_eq!(event.agent_id, None);
    assert!(!event.is_subagent_event());
    assert_eq!(event.last_assistant_message.as_deref(), Some("All done."));
    assert_eq!(event.background_task_count, Some(2));
    assert_eq!(event.received_at, at());
}

/// HookEventDecodingTests.decodesPermissionRequestWithNestedInputAndSuggestions.
#[test]
fn decodes_permission_request_with_nested_input_and_suggestions() {
    let event = hook(json!({
        "event": "PermissionRequest",
        "session_id": "abc",
        "cwd": "/tmp",
        "status": "waiting_for_approval",
        "tool": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which DB?", "options": [{"label": "Postgres"}, {"label": "SQLite"}]}]},
        "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test"}], "behavior": "allow", "destination": "session"}],
    }));
    assert!(event.expects_response());
    assert_eq!(event.tool_use_id, None);
    let questions = event.tool_input.as_ref().unwrap()["questions"]
        .as_array()
        .unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(event.permission_suggestions.as_ref().map(Vec::len), Some(1));
    assert_eq!(event.tool.as_deref(), Some("AskUserQuestion"));
}

/// HookEventDecodingTests.decodesTaskAndFailureFields.
#[test]
fn decodes_task_and_failure_fields() {
    let created = hook(
        json!({"event": "TaskCreated", "session_id": "s", "task_id": 3, "task_subject": "Write docs"}),
    );
    assert_eq!(created.task_id.as_deref(), Some("3"));
    assert_eq!(created.task_subject.as_deref(), Some("Write docs"));
    assert_eq!(created.cwd, "");
    assert_eq!(created.status, "unknown");

    let failure = hook(
        json!({"event": "StopFailure", "session_id": "s", "status": "waiting_for_input", "stop_error": "rate_limit", "stop_error_details": "429"}),
    );
    assert_eq!(failure.stop_error.as_deref(), Some("rate_limit"));
    assert_eq!(failure.stop_error_details.as_deref(), Some("429"));

    let post = hook(
        json!({"event": "PostToolUseFailure", "session_id": "s", "tool": "Bash", "tool_use_id": "toolu_1", "tool_error": "boom", "is_interrupt": true}),
    );
    assert_eq!(post.tool_error.as_deref(), Some("boom"));
    assert_eq!(post.is_interrupt, Some(true));
    assert_eq!(post.tool_use_id.as_deref(), Some("toolu_1"));
}

/// HookEventDecodingTests.lenientTypesAndUnknownEvents.
#[test]
fn lenient_types_and_unknown_events() {
    let event = hook(json!({
        "event": "SomeFutureEvent",
        "session_id": "s",
        "pid": "123",
        "attended": "0",
        "unexpected_key": {"nested": true},
    }));
    assert_eq!(event.pid, Some(123));
    assert_eq!(event.attended, Some(false));
    assert!(event.is_from_ignored_session());

    assert!(matches!(
        decoded(json!({"session_id": "s"})),
        Decoded::Unreadable(_)
    ));
    assert!(matches!(
        decoded(json!({"event": "Stop"})),
        Decoded::Unreadable(_)
    ));
    // session_id and event must be strings (the Swift decoder's non-lossy fields).
    assert!(matches!(
        decoded(json!({"event": "Stop", "session_id": 7})),
        Decoded::Unreadable(_)
    ));
    assert!(matches!(
        decoded(json!({"event": 1, "session_id": "s"})),
        Decoded::Unreadable(_)
    ));
    assert!(matches!(decode(b"not json", at()), Decoded::Unreadable(_)));
    assert!(matches!(decode(b"[1,2]", at()), Decoded::Unreadable(_)));
    assert!(matches!(decode(b"", at()), Decoded::Unreadable(_)));
}

#[test]
fn loose_values_are_coerced_or_absent() {
    let event = hook(json!({
        "event": "Stop",
        "session_id": "s",
        "cwd": 12,
        "status": null,
        "pid": 0,
        "attended": "yes",
        "entrypoint": false,
        "tool_input": "not an object",
        "permission_suggestions": {"not": "a list"},
        "background_task_count": -1,
        "background_task_types": ["shell", 3, null, "subagent"],
        "session_cron_count": "2",
        "stop_hook_active": 1,
    }));
    assert_eq!(event.cwd, "12");
    assert_eq!(event.status, "unknown");
    assert_eq!(event.pid, None);
    assert_eq!(event.attended, Some(true));
    assert_eq!(event.entrypoint, None);
    assert_eq!(event.tool_input, None);
    assert_eq!(event.permission_suggestions, None);
    assert_eq!(event.background_task_count, None);
    assert_eq!(
        event.background_task_types,
        Some(vec!["shell".to_string(), "subagent".to_string()])
    );
    assert_eq!(event.session_cron_count, Some(2));
    assert_eq!(event.stop_hook_active, Some(true));

    // pid is kept only in 1..=Int32.max.
    for (raw, expected) in [
        (json!(1), Some(1)),
        (json!(2_147_483_647u64), Some(2_147_483_647)),
        (json!(2_147_483_648u64), None),
        (json!(-5), None),
        (json!("77"), Some(77)),
        (json!(" 77"), None),
        (json!(77.0), Some(77)),
        (json!(77.5), None),
        (json!(true), None),
    ] {
        let event = hook(json!({"event": "Stop", "session_id": "s", "pid": raw}));
        assert_eq!(event.pid, expected, "pid {raw}");
    }
}

/// HookEventDecodingTests.sessionFilterKeepsInteractiveEntrypoints (the
/// hook-event half; registry kinds are the session store's).
#[test]
fn session_filter_keeps_interactive_entrypoints() {
    let with = |attended: Value, entrypoint: Value| {
        hook(
            json!({"event": "Stop", "session_id": "s", "attended": attended, "entrypoint": entrypoint}),
        )
    };
    assert!(!with(json!(true), json!("cli")).is_from_ignored_session());
    assert!(!with(json!(null), json!("claude-vscode")).is_from_ignored_session());
    assert!(!with(json!(null), json!("claude-desktop")).is_from_ignored_session());
    assert!(with(json!(null), json!("sdk-ts")).is_from_ignored_session());
    assert!(with(json!(null), json!("SDK-cli")).is_from_ignored_session());
    assert!(with(json!(false), json!("cli")).is_from_ignored_session());
    assert!(!with(json!(null), json!("")).is_from_ignored_session());
}

/// HookEventDecodingTests.parsesStatusLine (rate-limit windows are the usage
/// parser's; here they must arrive raw).
#[test]
fn parses_status_line() {
    let rate_limits = json!({
        "five_hour": {"used_percentage": 42.5, "resets_at": 1_800_000_000},
        "seven_day": {"used_percentage": "12", "resets_at": 1_800_300_000},
    });
    let mut message = status(json!({
        "event": "StatusLine",
        "session_id": "abc",
        "cwd": "/Users/me/proj",
        "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/abc.jsonl",
        "config_dir_env": null,
        "pid": 4242,
        "status_line": {
            "rate_limits": rate_limits,
            "context_window": {"used_percentage": 37, "context_window_size": 200_000},
            "model": {"id": "claude-opus-4-5", "display_name": "Opus 4.5"},
            "cost": {"total_cost_usd": 1.25},
            "session_name": "Fix login",
            "version": "2.1.280",
        },
    }));
    assert_eq!(message.session_id.as_str(), "abc");
    assert_eq!(message.rate_limits.as_ref(), Some(&rate_limits));
    assert_eq!(message.context_used_percent, Some(37.0));
    assert_eq!(message.context_window_size, Some(200_000));
    assert_eq!(message.model_id.as_deref(), Some("claude-opus-4-5"));
    assert_eq!(message.model_display_name.as_deref(), Some("Opus 4.5"));
    assert_eq!(message.cost_usd, Some(1.25));
    assert_eq!(message.session_name.as_deref(), Some("Fix login"));
    assert_eq!(message.claude_code_version.as_deref(), Some("2.1.280"));
    assert_eq!(message.pid, Some(4242));
    assert_eq!(message.config_dir_env, None);
    assert_eq!(message.cwd.as_deref(), Some("/Users/me/proj"));
    assert_eq!(message.received_at, at());

    // The folder the transcript names (AccountPaths.normalize("/Users/me/.claude")).
    let paths = Paths::new(PathStyle::Posix, "/Users/me");
    fill_status_line_account(&mut message, &paths, |_| false);
    assert_eq!(
        message.account_id.as_ref().map(|a| a.as_str()),
        Some("/Users/me/.claude")
    );
}

#[test]
fn status_line_account_on_windows() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\me");
    let mut from_transcript = status(json!({
        "event": "StatusLine", "session_id": "s",
        "transcript_path": r"C:\Users\me\.claude-work\PROJECTS\C--x\s.jsonl",
        "config_dir_env": r"C:\elsewhere",
        "status_line": {},
    }));
    fill_status_line_account(&mut from_transcript, &paths, |_| false);
    assert_eq!(
        from_transcript.account_id.map(|a| a.0),
        Some(r"C:\Users\me\.claude-work".to_string())
    );

    let mut from_env = status(json!({
        "event": "StatusLine", "session_id": "s",
        "config_dir_env": "c:/Users/me/.claude-b/",
        "status_line": {},
    }));
    fill_status_line_account(&mut from_env, &paths, |_| false);
    assert_eq!(
        from_env.account_id.map(|a| a.0),
        Some(r"C:\Users\me\.claude-b".to_string())
    );

    let mut default = status(json!({"event": "StatusLine", "session_id": "s"}));
    fill_status_line_account(&mut default, &paths, |_| false);
    assert_eq!(
        default.account_id.map(|a| a.0),
        Some(r"C:\Users\me\.claude".to_string())
    );
}

/// HookEventDecodingTests.statusLineWithoutRateLimits.
#[test]
fn status_line_without_rate_limits() {
    let message = status(json!({"event": "StatusLine", "session_id": "abc", "status_line": {}}));
    assert_eq!(message.rate_limits, None);
    assert_eq!(message.context_used_percent, None);
    // No pid (an older wrapper, or no CLAUDE_PID): keyed by session instead.
    assert_eq!(message.pid, None);
    // 2^63 once trapped converting to Int; above pid_t's range is no pid either.
    for bad in [
        json!(null),
        json!(0),
        json!(-3),
        json!("x"),
        json!(true),
        json!(9_223_372_036_854_775_808.0),
        json!(1u64 << 63),
        json!(i32::MAX as i64 + 1),
    ] {
        let message = status(
            json!({"event": "StatusLine", "session_id": "abc", "pid": bad, "status_line": {}}),
        );
        assert_eq!(message.pid, None, "pid {bad}");
    }
    // A missing or empty session is no status line at all.
    assert!(matches!(
        decoded(json!({"event": "StatusLine", "session_id": ""})),
        Decoded::Unreadable(_)
    ));
    assert!(matches!(
        decoded(json!({"event": "StatusLine"})),
        Decoded::Unreadable(_)
    ));
}

#[test]
fn status_line_loose_values() {
    let message = status(json!({
        "event": "StatusLine", "session_id": "s", "pid": "4242", "cwd": "",
        "status_line": {
            "context_window": {"used_percentage": "8.5", "context_window_size": -1},
            "model": "not an object",
            "cost": {"total_cost_usd": true},
            "version": 2,
        },
    }));
    assert_eq!(message.pid, Some(4242));
    assert_eq!(message.cwd, None);
    assert_eq!(message.context_used_percent, Some(8.5));
    assert_eq!(message.context_window_size, None);
    assert_eq!(message.model_id, None);
    assert_eq!(message.cost_usd, None);
    assert_eq!(message.claude_code_version.as_deref(), Some("2"));
}

/// HookEventDecodingTests.permissionResponseEncodesNewShape.
#[test]
fn permission_response_encodes_new_shape() {
    let mut allow = PermissionResponse::allow();
    allow.updated_input = json!({"answers": {"Which DB?": "Postgres"}})
        .as_object()
        .cloned();
    allow.updated_permissions = Some(vec![json!({"type": "addRules", "behavior": "allow"})]);
    let object: Value = serde_json::from_slice(&allow.to_json()).unwrap();
    assert_eq!(object["decision"], "allow");
    assert_eq!(object["updated_input"]["answers"]["Which DB?"], "Postgres");
    assert_eq!(
        object["updated_permissions"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(object.get("reason").is_none());

    let mut deny = PermissionResponse::deny(Some("No".into()));
    deny.interrupt = Some(true);
    let object: Value = serde_json::from_slice(&deny.to_json()).unwrap();
    assert_eq!(object["decision"], "deny");
    assert_eq!(object["reason"], "No");
    assert_eq!(object["interrupt"], true);
    assert_eq!(deny.decision, Decision::Deny);
}

#[test]
fn windows_fields() {
    let event = hook(json!({
        "protocol": 1, "event": "PreToolUse", "session_id": "s",
        "hook_pid": 6789,
        "terminal": {"wt_session": "0b1c", "term_program": null},
    }));
    assert_eq!(event.protocol, 1);
    assert_eq!(event.hook_pid, Some(6789));
    let terminal = event.terminal.unwrap();
    assert_eq!(terminal.wt_session.as_deref(), Some("0b1c"));
    assert_eq!(terminal.term_program, None);

    // A Mac-shaped frame (no Windows fields) and a newer protocol both read.
    let old = hook(json!({"event": "Stop", "session_id": "s"}));
    assert_eq!(old.protocol, 1);
    assert_eq!(old.hook_pid, None);
    assert_eq!(old.terminal, None);
    let newer = hook(json!({"protocol": 7, "event": "Stop", "session_id": "s", "terminal": "?"}));
    assert_eq!(newer.protocol, 7);
    assert_eq!(newer.terminal, None);
    let odd = hook(json!({"protocol": 0, "event": "Stop", "session_id": "s"}));
    assert_eq!(odd.protocol, 1);
}

#[test]
fn control_frames() {
    assert_eq!(
        decoded(json!({"protocol": 1, "event": "AgentNotchControl", "op": "status"})),
        Decoded::Control(ControlOp::Status)
    );
    assert_eq!(
        decoded(json!({"event": "AgentNotchControl", "op": "quit", "later": 1})),
        Decoded::Control(ControlOp::Quit)
    );
    assert!(matches!(
        decoded(json!({"event": "AgentNotchControl", "op": "reboot"})),
        Decoded::Unreadable(_)
    ));
}
