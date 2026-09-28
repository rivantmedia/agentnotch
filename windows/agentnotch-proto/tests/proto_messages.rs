//! The hook's messages against the Mac hook script's behaviour (HS§1.4-1.5,
//! ported from the Python script and HookSocketIntegrationTests), and the
//! committed protocol-1 frames: each `tests/fixtures/v1/<name>.json` is what
//! the builder makes of `tests/fixtures/stdin/<name>.json` under
//! [`fixture_env`]. The frames are kept forever (later servers must accept
//! them, `ingress_protocol_compat.rs`), so the builder may add fields but
//! never change or drop one of theirs.

use agentnotch_proto::limits::*;
use agentnotch_proto::*;
use serde_json::{json, Value};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_json(path: PathBuf) -> Value {
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The environment the committed frames were built under.
fn fixture_env() -> HookEnv {
    HookEnv {
        claude_pid: Some("4242".into()),
        claude_config_dir: Some(r"C:\Users\me\.claude-work".into()),
        attended: Some("1".into()),
        entrypoint: Some("cli".into()),
        pid_guess: None,
        hook_pid: 6789,
        wt_session: Some("0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0".into()),
        term_program: None,
    }
}

/// Every key of `expected` is in `actual` with an equal value (recursively
/// for objects), so fields added after protocol 1 don't break the check.
fn assert_contains(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            for (key, value) in e {
                let child = format!("{path}.{key}");
                let got = a.get(key).unwrap_or_else(|| panic!("{child} is missing"));
                assert_contains(got, value, &child);
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn names(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures().join(dir))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".json").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn build(stdin: &Value) -> Value {
    if stdin.get("hook_event_name").is_some() {
        build_hook_message(stdin, &fixture_env()).expect("an object")
    } else {
        build_statusline_message(stdin, &fixture_env()).expect("an object")
    }
}

#[test]
fn the_v1_frames_are_what_the_builder_makes() {
    let stdin_names = names("stdin");
    assert!(stdin_names.len() >= 17, "{stdin_names:?}");
    for name in stdin_names {
        let stdin = read_json(fixtures().join(format!("stdin/{name}.json")));
        let frame = read_json(fixtures().join(format!("v1/{name}.json")));
        assert_contains(&build(&stdin), &frame, &name);
        assert_eq!(frame["protocol"], json!(1), "{name}");
    }
}

#[test]
fn every_v1_frame_is_a_protocol_1_object_with_an_event() {
    for name in names("v1") {
        let frame = read_json(fixtures().join(format!("v1/{name}.json")));
        assert_eq!(frame["protocol"], json!(1), "{name}");
        assert!(frame["event"].is_string(), "{name}");
        let bytes = serde_json::to_vec(&frame).unwrap();
        assert!(bytes.len() <= MAX_CLIENT_MESSAGE, "{name}");
    }
}

#[test]
fn base_fields() {
    let stdin = json!({"hook_event_name": "Stop", "session_id": "abc", "cwd": "/Users/me/proj",
        "transcript_path": "/t.jsonl", "agent_id": null, "permission_mode": "default",
        "last_assistant_message": "y".repeat(5000), "background_tasks": [{"id": "b1"}, {"id": "b2"}]});
    let m = build_hook_message(&stdin, &fixture_env()).unwrap();
    assert_eq!(m["event"], "Stop");
    assert_eq!(m["session_id"], "abc");
    assert_eq!(m["status"], "waiting_for_input");
    assert_eq!(m["pid"], 4242);
    assert_eq!(m["attended"], true);
    assert_eq!(m["entrypoint"], "cli");
    assert_eq!(m["config_dir_env"], r"C:\Users\me\.claude-work");
    assert_eq!(m["agent_id"], Value::Null);
    assert_eq!(m["hook_pid"], 6789);
    assert_eq!(
        m["terminal"],
        json!({"wt_session": "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0", "term_program": null})
    );
    assert_eq!(
        m["last_assistant_message"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1500
    );
    assert_eq!(m["background_task_count"], 2);
    // Entries without a string "type" are skipped, not sent as null.
    assert_eq!(m["background_task_types"], json!([]));
}

#[test]
fn missing_or_empty_ids_get_the_scripts_defaults() {
    let m =
        build_hook_message(&json!({"session_id": "", "cwd": null}), &HookEnv::default()).unwrap();
    assert_eq!(m["event"], "");
    assert_eq!(m["session_id"], "unknown");
    assert_eq!(m["cwd"], "");
    assert_eq!(m["status"], "unknown");
    assert_eq!(m["pid"], Value::Null);
    assert_eq!(m["attended"], Value::Null);
    assert!(build_hook_message(&json!([1, 2]), &HookEnv::default()).is_none());
    assert!(build_hook_message(&json!("text"), &HookEnv::default()).is_none());
}

#[test]
fn pid_precedence() {
    let stdin = json!({"hook_event_name": "PreToolUse", "session_id": "s"});
    let with = |pid: Option<&str>, guess: Option<u32>| {
        let env = HookEnv {
            claude_pid: pid.map(str::to_owned),
            pid_guess: guess,
            ..HookEnv::default()
        };
        build_hook_message(&stdin, &env).unwrap()["pid"].clone()
    };
    assert_eq!(with(Some("4242"), Some(7)), 4242);
    assert_eq!(with(Some(" 4242 "), None), 4242);
    // Unparsable or out of range: the walk's guess, else nothing.
    assert_eq!(with(Some("abc"), Some(7)), 7);
    assert_eq!(with(Some("0"), Some(7)), 7);
    assert_eq!(with(Some("2147483648"), None), Value::Null);
    assert_eq!(with(None, None), Value::Null);
    assert_eq!(with(None, Some(0)), Value::Null);
}

#[test]
fn attended_flag() {
    let stdin = json!({"hook_event_name": "Stop", "session_id": "s"});
    for (raw, expected) in [
        (Some("1"), json!(true)),
        (Some("0"), json!(false)),
        (Some("yes"), Value::Null),
        (None, Value::Null),
    ] {
        let env = HookEnv {
            attended: raw.map(str::to_owned),
            ..HookEnv::default()
        };
        assert_eq!(
            build_hook_message(&stdin, &env).unwrap()["attended"],
            expected,
            "{raw:?}"
        );
    }
}

#[test]
fn coarse_status_per_event() {
    for (event, extra, status) in [
        ("PreToolUse", json!({}), "running_tool"),
        ("PermissionRequest", json!({}), "waiting_for_approval"),
        ("Stop", json!({}), "waiting_for_input"),
        ("StopFailure", json!({}), "waiting_for_input"),
        ("SessionStart", json!({}), "waiting_for_input"),
        ("SessionEnd", json!({}), "ended"),
        ("PreCompact", json!({}), "compacting"),
        (
            "Notification",
            json!({"notification_type": "idle_prompt"}),
            "waiting_for_input",
        ),
        (
            "Notification",
            json!({"notification_type": "permission_prompt"}),
            "notification",
        ),
        ("UserPromptSubmit", json!({}), "processing"),
        ("PostToolUse", json!({}), "processing"),
        ("PostToolUseFailure", json!({}), "processing"),
        ("PermissionDenied", json!({}), "processing"),
        ("SubagentStart", json!({}), "processing"),
        ("SubagentStop", json!({}), "processing"),
        ("PostCompact", json!({}), "processing"),
        ("TaskCreated", json!({}), "processing"),
        ("TaskCompleted", json!({}), "processing"),
        ("SomeFutureEvent", json!({}), "unknown"),
    ] {
        let mut stdin = json!({"hook_event_name": event, "session_id": "s"});
        stdin
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            build_hook_message(&stdin, &HookEnv::default()).unwrap()["status"],
            status,
            "{event}"
        );
    }
}

#[test]
fn tool_fields() {
    let long = "x".repeat(30_000);
    let stdin = json!({"hook_event_name": "PermissionRequest", "session_id": "s", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which DB?", "options": [{"label": "SQLite", "description": long}]}]},
        "permission_suggestions": []});
    let m = build_hook_message(&stdin, &HookEnv::default()).unwrap();
    assert_eq!(m["tool"], "AskUserQuestion");
    // Every nested string is clamped; no tool_use_id on a PermissionRequest.
    assert_eq!(
        m["tool_input"]["questions"][0]["options"][0]["description"]
            .as_str()
            .unwrap()
            .len(),
        20_000
    );
    assert!(m.get("tool_use_id").is_none());
    assert_eq!(m["permission_suggestions"], json!([]));

    // A tool_input that isn't an object is sent as {}.
    let odd = json!({"hook_event_name": "PreToolUse", "session_id": "s", "tool_name": "X", "tool_input": "raw", "tool_use_id": ""});
    let m = build_hook_message(&odd, &HookEnv::default()).unwrap();
    assert_eq!(m["tool_input"], json!({}));
    assert!(m.get("tool_use_id").is_none(), "an empty id is not sent");
}

#[test]
fn failure_and_task_fields() {
    let failure = json!({"hook_event_name": "PostToolUseFailure", "session_id": "s", "tool_name": "Bash",
        "tool_use_id": "toolu_1", "error": {"code": 2}, "is_interrupt": true});
    let m = build_hook_message(&failure, &HookEnv::default()).unwrap();
    // A non-string error goes through json.dumps.
    assert_eq!(m["tool_error"], r#"{"code": 2}"#);
    assert_eq!(m["is_interrupt"], true);
    let not_bool = json!({"hook_event_name": "PostToolUseFailure", "session_id": "s", "is_interrupt": "yes", "error": ""});
    let m = build_hook_message(&not_bool, &HookEnv::default()).unwrap();
    assert!(m.get("is_interrupt").is_none());
    assert_eq!(m["tool_error"], Value::Null);

    let created = json!({"hook_event_name": "TaskCreated", "session_id": "s", "task_id": 3, "task_subject": "Write docs"});
    let m = build_hook_message(&created, &HookEnv::default()).unwrap();
    assert_eq!(m["task_id"], "3");
    assert_eq!(m["task_subject"], "Write docs");

    let post = json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_name": "TaskCreate",
        "tool_response": {"task": {"id": "12", "subject": ""}}});
    let m = build_hook_message(&post, &HookEnv::default()).unwrap();
    assert_eq!(m["task_id"], "12");
    assert_eq!(m["task_subject"], Value::Null);

    let stop_failure = json!({"hook_event_name": "StopFailure", "session_id": "s", "error": 5, "error_details": "d".repeat(900)});
    let m = build_hook_message(&stop_failure, &HookEnv::default()).unwrap();
    assert_eq!(m["stop_error"], "unknown");
    assert_eq!(
        m["stop_error_details"].as_str().unwrap().len(),
        MAX_TOOL_ERROR
    );

    let denied = json!({"hook_event_name": "PermissionDenied", "session_id": "s", "reason": "", "message": "Blocked"});
    assert_eq!(
        build_hook_message(&denied, &HookEnv::default()).unwrap()["denial_reason"],
        "Blocked"
    );
}

#[test]
fn prompt_and_title_limits() {
    let stdin = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "p".repeat(1000),
        "session_title": "t".repeat(500), "source": "user"});
    let m = build_hook_message(&stdin, &HookEnv::default()).unwrap();
    assert_eq!(m["prompt"].as_str().unwrap().len(), MAX_PROMPT);
    assert_eq!(
        m["session_title"].as_str().unwrap().len(),
        MAX_SESSION_TITLE
    );
    assert_eq!(m["source"], "user");

    let start = json!({"hook_event_name": "SessionStart", "session_id": "s", "model": {"id": "x"}, "source": "clear"});
    let m = build_hook_message(&start, &HookEnv::default()).unwrap();
    assert_eq!(m["model"], Value::Null, "only a string model is forwarded");
    assert_eq!(m["source"], "clear");
}

#[test]
fn a_huge_message_is_cut_to_one_mebibyte() {
    // Strings are already clamped to 20k; many of them still add up.
    let edits: Vec<Value> = (0..200)
        .map(|i| json!({"old_string": "o".repeat(20_000), "new_string": format!("{i}")}))
        .collect();
    let stdin = json!({"hook_event_name": "PreToolUse", "session_id": "s", "tool_name": "MultiEdit",
        "tool_input": {"file_path": "C:/x.rs", "edits": edits}, "tool_use_id": "toolu_big"});
    let message = build_hook_message(&stdin, &HookEnv::default()).unwrap();
    assert!(serde_json::to_vec(&message).unwrap().len() > MAX_CLIENT_MESSAGE);
    let bytes = encode_hook_message(&message);
    assert!(bytes.len() <= MAX_CLIENT_MESSAGE);
    let sent: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        sent["tool_input"]["edits"].as_array().unwrap().len(),
        MAX_LIST_ITEMS
    );
    assert_eq!(
        sent["tool_input"]["edits"][0]["old_string"]
            .as_str()
            .unwrap()
            .len(),
        500
    );
    assert_eq!(sent["tool_use_id"], "toolu_big");

    // Too big even then: the input is dropped, the event still goes.
    let wide: serde_json::Map<String, Value> = (0..4000)
        .map(|i| (format!("k{i:05}"), json!("v".repeat(400))))
        .collect();
    let stdin = json!({"hook_event_name": "PreToolUse", "session_id": "s", "tool_name": "X", "tool_input": wide});
    let message = build_hook_message(&stdin, &HookEnv::default()).unwrap();
    let sent: Value = serde_json::from_slice(&encode_hook_message(&message)).unwrap();
    assert_eq!(sent["tool_input"], json!({}));
    assert_eq!(sent["event"], "PreToolUse");

    // A small message is passed through untouched.
    let small = build_hook_message(
        &json!({"hook_event_name": "Stop", "session_id": "s"}),
        &HookEnv::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&encode_hook_message(&small)).unwrap(),
        small
    );
}

#[test]
fn status_line_message() {
    let stdin = read_json(fixtures().join("stdin/status_line.json"));
    let m = build_statusline_message(&stdin, &fixture_env()).unwrap();
    assert_eq!(m["event"], STATUS_LINE_EVENT);
    assert_eq!(m["pid"], 4242);
    assert_eq!(
        m["status_line"]["rate_limits"]["five_hour"]["used_percentage"],
        42.5
    );
    assert_eq!(
        m["status_line"]["context_window"],
        json!({"used_percentage": 37, "context_window_size": 200000})
    );
    assert_eq!(m["status_line"]["cost"], json!({"total_cost_usd": 1.25}));
    assert_eq!(m["status_line"]["model"]["display_name"], "Opus 4.5");
    // No parent-pid fallback: the wrapper runs under a shell.
    let env = HookEnv {
        claude_pid: None,
        pid_guess: Some(99),
        ..fixture_env()
    };
    assert_eq!(
        build_statusline_message(&stdin, &env).unwrap()["pid"],
        Value::Null
    );
    // Missing parts come through as nulls, not as a refusal.
    let bare =
        build_statusline_message(&json!({"session_id": "s", "cost": 3}), &HookEnv::default())
            .unwrap();
    assert_eq!(bare["status_line"]["cost"], json!({"total_cost_usd": null}));
    assert_eq!(bare["status_line"]["rate_limits"], Value::Null);
    assert!(build_statusline_message(&json!(null), &HookEnv::default()).is_none());
}
