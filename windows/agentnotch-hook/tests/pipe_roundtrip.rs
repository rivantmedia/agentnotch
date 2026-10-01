//! The real hook exe against the real pipe server (DESIGN-WIN §1.4, §7.2): what the Mac's
//! `HookSocketIntegrationTests` prove with the hook script and a private socket, and what only
//! Windows can get wrong.
//!
//! Every test runs the app's side in-process (`agentnotch_win::pipe_server::PipeServer` feeding
//! the engine's `HookIngress`) on a pipe name of its own, and the exe as Claude Code would.
//!
//! The time bounds are the design's and are measured from just before the process is created:
//! no app, under 200 ms; a server that never reads, under 1.5 s (the 1.2 s watchdog); the server
//! stopping, or the app's process being killed, under a waiting hook, under 1 s. Only the first
//! is not taken from a single run: see `without_an_app_the_hook_is_gone_in_under_200_ms`.
//! For the kill, this binary runs again in a process of its own as the app
//! (`the_app_in_a_process_of_its_own`, ignored in a normal run).

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, Instant};

use agentnotch_engine::platform::{HookTransport, TransportEvent};
use agentnotch_engine::runtime_types::{AnswerResult, IngressOut, Release};
use agentnotch_proto::limits::{
    HOOK_WATCHDOG_MS, MAX_CLIENT_MESSAGE, MAX_TOOL_INPUT_STRING, SERVER_READ_DEADLINE_MS,
};
use agentnotch_proto::{
    build_hook_message, encode_hook_message, ControlOp, ControlResponse, ControlStatus,
    PermissionResponse, SYSTEM_SID,
};
use agentnotch_win::pipe_server::client::{self, ClientError};
use agentnotch_win::pipe_server::PIPE_IN_USE;
use common::{
    assert_silent_success, begin, fixture, hook_command, hook_env, open_client, own_sid,
    pipe_security, response_fixture, silent_server, spawn, temp_folder, trace_file, trace_lines,
    traced, unique_pipe, Harness, CONFIG_DIR, HARD_TIMEOUT, WAIT,
};
use serde_json::{json, Map, Value};

/// How long a PermissionRequest's hook is watched to make sure it waits.
const BLOCKED_FOR: Duration = Duration::from_millis(250);
/// The budget of the app's own client in these tests.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
/// A span in which something that must not happen would have happened.
const SHORTLY: Duration = Duration::from_millis(300);

const SESSION: &str = "sess-int";
const QUESTION: &str = "Which charting library should the dashboard use?";

/// What Claude Code writes on a hook's stdin: the fields every event has, plus `extra`.
fn payload(event: &str, extra: Value) -> Vec<u8> {
    let mut base = json!({
        "hook_event_name": event,
        "session_id": SESSION,
        "transcript_path": r"C:\Users\me\.claude-work\projects\C--tmp\sess-int.jsonl",
        "cwd": r"C:\tmp",
        "permission_mode": "default",
    });
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    serde_json::to_vec(&base).unwrap()
}

/// A PreToolUse whose message is about as large as a hook sends: 52 strings that each just fit
/// the 20 000 character clamp, a little under the 1 MiB a client may send.
fn large_pre_tool_use() -> Vec<u8> {
    let input: Map<String, Value> = (0..52)
        .map(|n| {
            (
                format!("field_{n:02}"),
                Value::String("x".repeat(MAX_TOOL_INPUT_STRING)),
            )
        })
        .collect();
    payload(
        "PreToolUse",
        json!({"tool_name": "Write", "tool_input": input, "tool_use_id": "toolu_large"}),
    )
}

/// The frame a hook started by `hook_command` sends for `stdin`.
fn expected_frame(stdin: &[u8], hook_pid: u32) -> (Value, Vec<u8>) {
    let event: Value = serde_json::from_slice(stdin).unwrap();
    let message = build_hook_message(&event, &hook_env(hook_pid)).expect("an event object");
    let frame = encode_hook_message(&message);
    (message, frame)
}

fn printed(stdout: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|error| {
        panic!(
            "the hook printed no JSON ({error}): {:?}",
            String::from_utf8_lossy(stdout)
        )
    })
}

fn join<T>(thread: std::thread::JoinHandle<T>) -> T {
    thread.join().expect("the client's thread ended")
}

// ---- fire and forget ----

#[test]
fn a_pre_tool_use_arrives_as_the_hook_built_it() {
    let _test = begin("a_pre_tool_use_arrives_as_the_hook_built_it");
    let mut app = Harness::start("pre");
    let stdin = fixture("stdin/pre_tool_use.json");

    let done = spawn(hook_command(&app.pipe), &stdin).finish();
    assert_silent_success(&done, "PreToolUse");

    let (frame, outs) = app.wait_frame();
    let (message, bytes) = expected_frame(&stdin, done.pid);
    assert_eq!(
        serde_json::from_slice::<Value>(&frame.bytes).unwrap(),
        message
    );
    assert_eq!(frame.bytes, bytes);
    assert_eq!(message["pid"], json!(std::process::id()));
    assert_eq!(message["hook_pid"], json!(done.pid));
    assert_eq!(frame.peer_pid, Some(done.pid));

    let [IngressOut::Hook(event)] = outs.as_slice() else {
        panic!("expected one hook event, got {outs:?}");
    };
    assert_eq!(event.event, "PreToolUse");
    assert_eq!(event.tool.as_deref(), Some("Bash"));
    assert_eq!(event.tool_use_id.as_deref(), Some("toolu_01ABCDEF"));
    assert_eq!(event.pid, Some(std::process::id()));
    assert_eq!(event.config_dir_env.as_deref(), Some(CONFIG_DIR));
    assert_eq!(event.attended, Some(true));
    assert_eq!(event.entrypoint.as_deref(), Some("cli"));
    assert_eq!(app.ingress.held_count(), 0);
}

/// HookSocketIntegrationTests.fireAndForgetEventsAndIgnoredSessions.
#[test]
fn fire_and_forget_events_and_ignored_sessions() {
    let _test = begin("fire_and_forget_events_and_ignored_sessions");
    let mut app = Harness::start("forget");

    let stop = payload(
        "Stop",
        json!({
            "last_assistant_message": "y".repeat(5000),
            "background_tasks": [{"id": "b1"}, {"id": "b2"}],
        }),
    );
    let done = spawn(hook_command(&app.pipe), &stop).finish();
    assert_silent_success(&done, "Stop");
    let event = app.wait_hook("Stop");
    let message = event.last_assistant_message.as_deref();
    assert_eq!(message.map(|text| text.chars().count()), Some(1500));
    assert_eq!(event.background_task_count, Some(2));
    assert_eq!(event.pid, Some(std::process::id()));
    assert_eq!(event.attended, Some(true));
    assert_eq!(event.entrypoint.as_deref(), Some("cli"));

    // An unattended session's event reaches the app and goes no further.
    let mut unattended = hook_command(&app.pipe);
    unattended.env("CLAUDE_CODE_SESSION_ATTENDED", "0");
    let done = spawn(unattended, &stop).finish();
    assert_silent_success(&done, "unattended Stop");
    let (_, outs) = app.wait_frame();
    assert!(outs.is_empty(), "{outs:?}");

    // Neither does `claude -p`'s, and its PermissionRequest is let go at once: nothing shows
    // the session, so nothing may hold its request.
    let mut sdk = hook_command(&app.pipe);
    sdk.env("CLAUDE_CODE_ENTRYPOINT", "sdk-cli");
    let request = spawn(sdk, &fixture("stdin/permission_request_bash.json"));
    let (_, outs) = app.wait_frame();
    assert!(outs.is_empty(), "{outs:?}");
    assert_eq!(app.ingress.held_count(), 0);
    assert_silent_success(&request.finish(), "sdk PermissionRequest");
}

/// The design's assumption that what a client wrote before it closed is still readable by the
/// server: the hook has exited before the last of its megabyte is read.
#[test]
fn a_one_mebibyte_message_arrives_whole_after_the_hook_is_gone() {
    let _test = begin("a_one_mebibyte_message_arrives_whole_after_the_hook_is_gone");
    let mut app = Harness::start("large");
    let stdin = large_pre_tool_use();

    let done = spawn(hook_command(&app.pipe), &stdin).finish();
    assert_silent_success(&done, "a large PreToolUse");

    let (frame, outs) = app.wait_frame();
    let (message, bytes) = expected_frame(&stdin, done.pid);
    assert!(
        (1_000_000..=MAX_CLIENT_MESSAGE).contains(&bytes.len()),
        "the message is {} bytes",
        bytes.len()
    );
    // Nothing of it was cut a second time to make it fit.
    assert_eq!(
        message["tool_input"]["field_51"].as_str().map(str::len),
        Some(MAX_TOOL_INPUT_STRING)
    );
    assert_eq!(frame.bytes.len(), bytes.len());
    assert!(frame.bytes == bytes, "the frame differs from the message");
    let [IngressOut::Hook(event)] = outs.as_slice() else {
        panic!("expected one hook event, got {} outs", outs.len());
    };
    assert_eq!(event.tool_use_id.as_deref(), Some("toolu_large"));
    assert_eq!(event.tool_input.as_ref().map(Map::len), Some(52));
}

#[test]
fn garbage_on_stdin_sends_nothing() {
    let _test = begin("garbage_on_stdin_sends_nothing");
    let mut app = Harness::start("garbage");
    let inputs: [&[u8]; 6] = [
        b"",
        b"not json",
        b"{",
        b"[1, 2, 3]",
        b"\"PreToolUse\"",
        b"\xff\xfe\x00\x00{\x00}",
    ];
    for input in inputs {
        let done = spawn(hook_command(&app.pipe), input).finish();
        assert_silent_success(&done, &format!("{:?}", String::from_utf8_lossy(input)));
    }
    // Each of them connected and left without a frame.
    assert_eq!(app.frames_during(SHORTLY).len(), 0);

    // The server is none the worse for it.
    let done = spawn(hook_command(&app.pipe), &fixture("stdin/pre_tool_use.json")).finish();
    assert_silent_success(&done, "PreToolUse");
    app.wait_hook("PreToolUse");
}

/// Claude Code runs the PreToolUse hooks of parallel tool calls at the same moment, and every
/// one of them must reach the app: the Mac's socket queues them in its backlog. A pipe instance
/// takes one client at a time; the server puts a new one in its place at once, and a hook that
/// finds every instance taken waits for the next one, within its budget.
#[test]
fn a_burst_of_hooks_all_arrive() {
    let _test = begin("a_burst_of_hooks_all_arrive");
    const HOOKS: usize = 16;
    let mut app = Harness::start("burst");
    let trace = trace_file();
    let id = |n: usize| format!("toolu_burst_{n:02}");

    // Every hook is started and blocked on its stdin first (starting a process is the slow
    // part); then all their stdins end at once, and they open the pipe together.
    let mut waiting = Vec::new();
    for n in 0..HOOKS {
        let mut command = hook_command(&app.pipe);
        command
            .env("AGENTNOTCH_HOOK_TRACE", &trace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("the hook exe starts");
        let stdin = child.stdin.take().expect("a piped stdin");
        let event = payload(
            "PreToolUse",
            json!({
                "tool_name": "Read",
                "tool_input": {"file_path": format!(r"C:\tmp\{n}.txt")},
                "tool_use_id": id(n),
            }),
        );
        waiting.push((child, stdin, event));
    }
    let go = Arc::new(Barrier::new(HOOKS));
    let (tell, ended) = mpsc::channel();
    for (child, mut stdin, event) in waiting {
        let (go, tell) = (go.clone(), tell.clone());
        std::thread::spawn(move || {
            go.wait();
            let _ = stdin.write_all(&event);
            drop(stdin);
            let _ = tell.send(child.wait_with_output());
        });
    }
    for _ in 0..HOOKS {
        let output = ended
            .recv_timeout(HARD_TIMEOUT)
            .expect("every hook of the burst ends")
            .expect("the hook's output");
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{output:?}"
        );
    }

    let mut arrived: Vec<String> = Vec::new();
    let deadline = Instant::now() + WAIT;
    while arrived.len() < HOOKS {
        let Some(step) = app.step(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        for out in step.outs {
            if let IngressOut::Hook(event) = out {
                arrived.extend(event.tool_use_id);
            }
        }
    }
    arrived.sort();
    let expected: Vec<String> = (0..HOOKS).map(id).collect();
    // On a loss, the trace says why each lost hook gave up.
    let gave_up: Vec<String> = trace_lines(&trace)
        .into_iter()
        .map(|(_, what)| what)
        .filter(|what| {
            !what.starts_with("invoked") && !what.starts_with("env ") && !what.starts_with("sent ")
        })
        .collect();
    assert_eq!(
        arrived, expected,
        "the hooks that gave up said: {gave_up:?}"
    );
}

// ---- PermissionRequest ----

/// Every `v1-responses` sample: the hook waits, and for the app's frame prints exactly what the
/// Mac hook script prints (nothing for `ask`). This also proves the design's other assumption:
/// what the server wrote before it closed its handle is still readable by the hook.
#[test]
fn every_answer_prints_the_macs_bytes() {
    let _test = begin("every_answer_prints_the_macs_bytes");
    let mut app = Harness::start("bytes");
    for name in [
        "allow",
        "always",
        "ask",
        "deny",
        "deny_reason",
        "keep_planning",
        "plan",
        "question",
        "unicode",
    ] {
        let case = response_fixture(name);
        let hook = spawn(hook_command(&app.pipe), &case.stdin);
        let held = app.wait_held();
        hook.assert_waiting(BLOCKED_FOR);

        // The frame's own bytes: the engine's answer types would write its keys in their order.
        assert!(
            app.server.respond(held.conn, case.response_frame.clone()),
            "{name}: the frame was not written"
        );
        let done = hook.finish();
        assert_eq!(done.code, Some(0), "{name}");
        assert_eq!(
            String::from_utf8_lossy(&done.stdout),
            String::from_utf8_lossy(&case.stdout),
            "{name}"
        );
        assert!(done.stdout == case.stdout, "{name}: not the same bytes");
        assert!(done.stderr.is_empty(), "{name}");
    }
}

/// The same answers as the app gives them: through the ingress, built by the constructors the
/// hub uses.
#[test]
fn the_apps_answers_reach_claude_code() {
    let _test = begin("the_apps_answers_reach_claude_code");
    let mut app = Harness::start("answers");
    for name in [
        "allow",
        "always",
        "deny",
        "deny_reason",
        "question",
        "unicode",
        "plan",
        "keep_planning",
    ] {
        let case = response_fixture(name);
        let hook = spawn(hook_command(&app.pipe), &case.stdin);
        let held = app.wait_held();
        hook.assert_waiting(BLOCKED_FOR);
        assert!(app.ingress.is_pending(&held.session_id, &held.tool_use_id));

        let response = match name {
            "allow" => PermissionResponse::allow(),
            "always" => {
                let suggestions = held.event.permission_suggestions.as_ref();
                PermissionResponse::always_allow(suggestions.expect("suggestions")[0].clone())
            }
            "deny" => PermissionResponse::deny(None),
            "deny_reason" => PermissionResponse {
                interrupt: Some(true),
                ..PermissionResponse::deny(Some("Not on main".into()))
            },
            "question" => PermissionResponse::answers([(QUESTION, "Recharts")]),
            "unicode" => {
                PermissionResponse::answers([(QUESTION, "Chart.js \u{2713} café \u{1F600}")])
            }
            "plan" => PermissionResponse::approve_plan(),
            "keep_planning" => PermissionResponse::keep_planning(),
            other => unreachable!("{other}"),
        };
        assert_eq!(
            app.ingress
                .answer(&held.session_id, &held.tool_use_id, response),
            AnswerResult::Delivered,
            "{name}"
        );
        let done = hook.finish();
        assert_eq!(done.code, Some(0), "{name}");
        assert!(done.stderr.is_empty(), "{name}");
        assert_eq!(
            printed(&done.stdout),
            printed(&case.stdout),
            "{name}: what the hook printed"
        );
        // A suggestion is handed back as the engine read it, its keys in the engine's order;
        // everything else is the Mac's bytes too.
        if name != "always" {
            assert!(done.stdout == case.stdout, "{name}: not the same bytes");
        }
        assert!(!app.ingress.is_pending(&held.session_id, &held.tool_use_id));
        assert_eq!(app.ingress.held_count(), 0, "{name}");
    }
}

/// HookSocketIntegrationTests.answersAskUserQuestionThroughTheHook.
#[test]
fn answers_ask_user_question_through_the_hook() {
    let _test = begin("answers_ask_user_question_through_the_hook");
    let mut app = Harness::start("ask-user");
    let input = json!({"questions": [{
        "question": "Which DB?", "header": "DB", "multiSelect": false,
        "options": [
            {"label": "Postgres", "description": "Relational"},
            {"label": "SQLite", "description": "x".repeat(30_000)},
        ],
    }]});

    let pre = payload(
        "PreToolUse",
        json!({"tool_name": "AskUserQuestion", "tool_input": input, "tool_use_id": "toolu_ask"}),
    );
    assert_silent_success(&spawn(hook_command(&app.pipe), &pre).finish(), "PreToolUse");
    app.wait_hook("PreToolUse");

    let request = payload(
        "PermissionRequest",
        json!({"tool_name": "AskUserQuestion", "tool_input": input, "permission_suggestions": []}),
    );
    let hook = spawn(hook_command(&app.pipe), &request);
    let held = app.wait_held();
    assert_eq!(held.tool_use_id, "toolu_ask");
    assert!(!held.has_synthetic_tool_use_id);
    assert_eq!(held.session_id.as_str(), SESSION);
    assert!(app.ingress.is_pending(&held.session_id, "toolu_ask"));
    hook.assert_waiting(BLOCKED_FOR);

    let answer = PermissionResponse::answers([("Which DB?", "Postgres")]);
    assert_eq!(
        app.ingress.answer(&held.session_id, "toolu_ask", answer),
        AnswerResult::Delivered
    );
    let done = hook.finish();
    assert_eq!(done.code, Some(0));

    let output = printed(&done.stdout);
    let specific = &output["hookSpecificOutput"];
    assert_eq!(specific["hookEventName"], "PermissionRequest");
    let decision = &specific["decision"];
    assert_eq!(decision["behavior"], "allow");
    let updated = &decision["updatedInput"];
    assert_eq!(updated["answers"], json!({"Which DB?": "Postgres"}));
    // Merged onto the ORIGINAL input: the 30k description is whole, not the copy the app got.
    let description = updated["questions"][0]["options"][1]["description"].as_str();
    assert_eq!(description.map(str::len), Some(30_000));
    assert!(!app.ingress.is_pending(&held.session_id, "toolu_ask"));
}

/// HookSocketIntegrationTests.denyAndAlwaysAllowShapes.
#[test]
fn deny_and_always_allow_shapes() {
    let _test = begin("deny_and_always_allow_shapes");
    let mut app = Harness::start("shapes");
    let suggestion = json!({
        "type": "addRules",
        "rules": [{"toolName": "Bash", "ruleContent": "rm -rf build"}],
        "behavior": "allow",
        "destination": "session",
    });
    let request = |command: &str| {
        payload(
            "PermissionRequest",
            json!({
                "tool_name": "Bash",
                "tool_input": {"command": command},
                "permission_suggestions": [suggestion],
            }),
        )
    };

    // No PreToolUse was seen: the engine makes up an id and still holds the request.
    let hook = spawn(hook_command(&app.pipe), &request("rm -rf build"));
    let held = app.wait_held();
    assert!(held.has_synthetic_tool_use_id);
    let first = held
        .event
        .permission_suggestions
        .as_ref()
        .expect("suggestions")[0]
        .clone();
    assert_eq!(
        app.ingress.answer(
            &held.session_id,
            &held.tool_use_id,
            PermissionResponse::always_allow(first)
        ),
        AnswerResult::Delivered
    );
    let done = hook.finish();
    assert_eq!(done.code, Some(0));
    let output = printed(&done.stdout);
    let decision = &output["hookSpecificOutput"]["decision"];
    assert_eq!(decision["behavior"], "allow");
    assert_eq!(decision.get("updatedInput"), None);
    assert_eq!(decision["updatedPermissions"], json!([suggestion]));

    // Deny.
    let hook = spawn(hook_command(&app.pipe), &request("git push --force"));
    app.wait_held();
    let pending = app.ingress.pending();
    let [(session, tool_use_id)] = pending.as_slice() else {
        panic!("expected one held request, got {pending:?}");
    };
    assert_eq!(
        app.ingress.answer(
            session,
            tool_use_id,
            PermissionResponse::deny(Some("Not on main".into()))
        ),
        AnswerResult::Delivered
    );
    let done = hook.finish();
    assert_eq!(done.code, Some(0));
    let output = printed(&done.stdout);
    let decision = &output["hookSpecificOutput"]["decision"];
    assert_eq!(decision["behavior"], "deny");
    assert_eq!(decision["message"], "Not on main");
}

#[test]
fn ask_and_a_close_without_a_frame_print_nothing() {
    let _test = begin("ask_and_a_close_without_a_frame_print_nothing");
    let mut app = Harness::start("no-decision");
    let stdin = fixture("stdin/permission_request_bash.json");
    let traced_hook = |pipe: &str| {
        let trace = trace_file();
        let mut command = hook_command(pipe);
        command.env("AGENTNOTCH_HOOK_TRACE", &trace);
        (spawn(command, &stdin), trace)
    };

    // "ask": the app leaves the decision to Claude Code's own prompt.
    let (hook, trace) = traced_hook(&app.pipe);
    let held = app.wait_held();
    hook.assert_waiting(BLOCKED_FOR);
    assert_eq!(
        app.ingress.answer(
            &held.session_id,
            &held.tool_use_id,
            PermissionResponse::ask()
        ),
        AnswerResult::Delivered
    );
    let done = hook.finish();
    assert_silent_success(&done, "ask");
    let lines = traced(&trace, done.pid);
    assert_eq!(lines.last().map(String::as_str), Some("decision: ask"));

    // Released (the tool ran, the session stopped, …): the connection closes with no frame.
    let (hook, trace) = traced_hook(&app.pipe);
    let held = app.wait_held();
    hook.assert_waiting(BLOCKED_FOR);
    app.ingress.release(Release::Request {
        session: held.session_id.clone(),
        tool_use_id: held.tool_use_id.clone(),
    });
    let done = hook.finish();
    assert_silent_success(&done, "released");
    let lines = traced(&trace, done.pid);
    assert_eq!(lines.last().map(String::as_str), Some("no decision"));
    assert_eq!(app.ingress.held_count(), 0);
    // Too late to answer it now.
    assert_eq!(
        app.ingress.answer(
            &held.session_id,
            &held.tool_use_id,
            PermissionResponse::allow()
        ),
        AnswerResult::NotPending
    );
}

/// HookSocketIntegrationTests.detectsAHookThatWentAway.
#[test]
fn a_killed_hook_is_reported_as_gone() {
    let _test = begin("a_killed_hook_is_reported_as_gone");
    let mut app = Harness::start("gone");
    let request = payload(
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"command": "make"}}),
    );
    let hook = spawn(hook_command(&app.pipe), &request);
    let held = app.wait_held();
    hook.assert_waiting(BLOCKED_FOR);

    // Claude Code kills the hook when the terminal dialog answers first.
    hook.kill();
    let done = hook.finish();
    assert_ne!(done.code, Some(0));
    assert!(done.stdout.is_empty());

    let outs = app.wait_for("the peer to be reported gone", |step| match &step.event {
        TransportEvent::PeerClosed(conn) if *conn == held.conn => Some(step.outs.clone()),
        _ => None,
    });
    assert_eq!(
        outs,
        [IngressOut::PermissionFailed {
            session: held.session_id.clone(),
            tool_use_id: held.tool_use_id.clone(),
        }]
    );
    assert!(!app.ingress.is_pending(&held.session_id, &held.tool_use_id));
    assert_eq!(
        app.ingress.answer(
            &held.session_id,
            &held.tool_use_id,
            PermissionResponse::allow()
        ),
        AnswerResult::NotPending
    );
    assert!(!app.server.respond(held.conn, b"{}".to_vec()));
}

#[test]
fn a_stopped_server_ends_a_waiting_hook_within_a_second() {
    let _test = begin("a_stopped_server_ends_a_waiting_hook_within_a_second");
    let mut app = Harness::start("stopped");
    let hook = spawn(
        hook_command(&app.pipe),
        &fixture("stdin/permission_request_bash.json"),
    );
    app.wait_held();
    hook.assert_waiting(BLOCKED_FOR);

    // The app quits, crashes or is updated: only the transport goes, nobody releases anything.
    let stopping = Instant::now();
    app.server.stop();
    let done = hook.finish();
    let took = stopping.elapsed();
    assert_silent_success(&done, "the server stopped");
    assert!(took < Duration::from_secs(1), "it took {took:?}");
}

/// Set on the copy of this test binary that plays the app for
/// `a_killed_app_ends_a_waiting_hook_within_a_second`: the pipe it serves…
const STAND_IN_PIPE: &str = "AGENTNOTCH_TEST_STAND_IN_PIPE";
/// …and the folder where it leaves a file for each step it got to.
const STAND_IN_FOLDER: &str = "AGENTNOTCH_TEST_STAND_IN_FOLDER";
/// Its name, for libtest to run it alone.
const STAND_IN_TEST: &str = "the_app_in_a_process_of_its_own";

/// Not a test of its own: the app's side (the real server feeding the real ingress) in a
/// process of its own, for `a_killed_app_ends_a_waiting_hook_within_a_second` to kill. Ignored
/// in a normal run; that test runs this binary again with `--ignored --exact` and the two
/// variables. Without them it does nothing.
#[test]
#[ignore = "run in a process of its own by a_killed_app_ends_a_waiting_hook_within_a_second"]
fn the_app_in_a_process_of_its_own() {
    let (Some(pipe), Some(folder)) = (
        std::env::var_os(STAND_IN_PIPE),
        std::env::var_os(STAND_IN_FOLDER),
    ) else {
        return;
    };
    let folder = PathBuf::from(folder);
    let mut app = Harness::on(pipe.to_str().expect("a Unicode pipe name"));
    app.wait_listening();
    std::fs::write(folder.join("listening"), b"").expect("the stand-in's first step");
    app.wait_held();
    std::fs::write(folder.join("held"), b"").expect("the stand-in's second step");
    // Held until the test kills this process; a stand-in whose test went away ends on its own.
    std::thread::sleep(Duration::from_secs(60));
}

/// The stand-in app; killed when the test ends, however it ends.
struct StandIn(Child);

impl StandIn {
    /// Waits until the stand-in left `step` in its folder; fails when it ended first or took
    /// too long.
    #[track_caller]
    fn wait_for(&mut self, step: &Path) {
        let until = Instant::now() + WAIT;
        while !step.exists() {
            if let Ok(Some(status)) = self.0.try_wait() {
                panic!(
                    "the stand-in app ended ({status}) before {}",
                    step.display()
                );
            }
            assert!(
                Instant::now() < until,
                "the stand-in app never got to {}",
                step.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The app is ended by Windows (a crash, Task Manager, the updater's kill) while a hook waits:
/// nothing in the app runs, and the kernel closing its handles is all the hook gets. It exits 0,
/// silent, within a second (DESIGN-WIN §7.3 "server killed mid-wait").
#[test]
fn a_killed_app_ends_a_waiting_hook_within_a_second() {
    let _test = begin("a_killed_app_ends_a_waiting_hook_within_a_second");
    let pipe = unique_pipe("killed");
    let folder = temp_folder("killed-app");
    let mut app = StandIn(
        Command::new(std::env::current_exe().expect("the test's own path"))
            .args([STAND_IN_TEST, "--exact", "--ignored", "--test-threads", "1"])
            .env(STAND_IN_PIPE, &pipe)
            .env(STAND_IN_FOLDER, folder.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the stand-in app starts"),
    );
    app.wait_for(&folder.path().join("listening"));
    let hook = spawn(
        hook_command(&pipe),
        &fixture("stdin/permission_request_bash.json"),
    );
    app.wait_for(&folder.path().join("held"));
    hook.assert_waiting(BLOCKED_FOR);

    let killing = Instant::now();
    app.0.kill().expect("the stand-in app is killed");
    let done = hook.finish();
    let took = killing.elapsed();
    assert_silent_success(&done, "the app was killed");
    assert!(took < Duration::from_secs(1), "it took {took:?}");
}

// ---- no app, or not an app ----

/// The design's bound is on the hook. A run is measured from before its process is created, so
/// it also holds what the runner needs to start a process, which at times is more than the
/// whole bound: after one unmeasured run, the fastest of three must be under 200 ms, and every
/// run must end with exit 0 and nothing printed.
#[test]
fn without_an_app_the_hook_is_gone_in_under_200_ms() {
    let _test = begin("without_an_app_the_hook_is_gone_in_under_200_ms");
    let nobody = unique_pipe("no-app");
    let request = fixture("stdin/permission_request_bash.json");

    // The unmeasured run also says why the others end: one failed open, nothing else.
    let trace = trace_file();
    let mut command = hook_command(&nobody);
    command.env("AGENTNOTCH_HOOK_TRACE", &trace);
    let warm_up = spawn(command, &request).finish();
    assert_silent_success(&warm_up, "PermissionRequest");
    assert_eq!(
        traced(&trace, warm_up.pid).last().map(String::as_str),
        Some("no app")
    );

    let mut fastest = Duration::MAX;
    for stdin in [&request, &fixture("stdin/pre_tool_use.json"), &request] {
        // A PermissionRequest is the kind that would wait if anyone were there.
        let done = spawn(hook_command(&nobody), stdin).finish();
        assert_silent_success(&done, "no app");
        fastest = fastest.min(done.elapsed);
    }
    assert!(
        fastest < Duration::from_millis(200),
        "the fastest run took {fastest:?}"
    );
}

/// A synchronous write of a megabyte into a 64 KiB pipe blocks for as long as the server does
/// not read. Nothing but the watchdog ends it.
#[test]
fn a_server_that_never_reads_costs_at_most_the_watchdog() {
    let _test = begin("a_server_that_never_reads_costs_at_most_the_watchdog");
    let pipe = unique_pipe("never-reads");
    // One instance with the app's owner and DACL, never read from. A client connects to a
    // listening instance whether or not its server ever asks who came.
    let server = silent_server(&pipe);

    let trace = trace_file();
    let mut command = hook_command(&pipe);
    command.env("AGENTNOTCH_HOOK_TRACE", &trace);
    let done = spawn(command, &large_pre_tool_use()).finish();
    assert_silent_success(&done, "a write nobody reads");

    let lines = traced(&trace, done.pid);
    // The pipe was opened and found to be the app's; the write never came back, and it was the
    // watchdog that ended the run.
    assert_eq!(
        lines,
        [
            "invoked hook exec=false",
            "env claude_pid=set config_dir=set",
            "watchdog: exit"
        ]
    );
    assert!(
        done.elapsed >= Duration::from_millis(HOOK_WATCHDOG_MS),
        "it ended after {:?}, before the watchdog",
        done.elapsed
    );
    assert!(
        done.elapsed < Duration::from_millis(1500),
        "it took {:?}",
        done.elapsed
    );
    drop(server);
}

/// The first-instance flag: a name somebody holds is never joined. (The Mac's socket path can
/// be taken over by a newer instance; a pipe name cannot, and must not be.)
#[test]
fn a_second_server_does_not_take_the_name() {
    let _test = begin("a_second_server_does_not_take_the_name");
    let mut first = Harness::start("twice");
    let pre = fixture("stdin/pre_tool_use.json");
    let refused = |harness: &mut Harness| {
        harness.wait_for("the name to be refused", |step| match &step.event {
            TransportEvent::Error(why) => Some((why.clone(), step.outs.clone())),
            TransportEvent::Listening(_) => panic!("a second server took the name"),
            _ => None,
        })
    };

    let mut second = Harness::on(&first.pipe);
    let (why, outs) = refused(&mut second);
    assert_eq!(why, "The hook pipe is in use by another program");
    assert_eq!(why, PIPE_IN_USE);
    assert_eq!(outs, [IngressOut::TransportStatus(Err(why))]);

    // The first one is unaffected, and hooks reach it alone.
    assert_silent_success(&spawn(hook_command(&first.pipe), &pre).finish(), "first");
    first.wait_hook("PreToolUse");
    assert_eq!(second.frames_during(SHORTLY).len(), 0);

    // The second one giving up takes nothing from the first.
    second.server.stop();
    assert_silent_success(&spawn(hook_command(&first.pipe), &pre).finish(), "second");
    first.wait_hook("PreToolUse");

    // A server that was refused keeps trying, and has the name soon after it came free.
    let mut third = Harness::on(&first.pipe);
    refused(&mut third);
    first.server.stop();
    third.wait_listening();
    assert_silent_success(&spawn(hook_command(&third.pipe), &pre).finish(), "third");
    third.wait_hook("PreToolUse");
    assert_eq!(first.frames_during(SHORTLY).len(), 0);
}

/// What the hook checks before it writes: read here from a client's own handle, as it does.
#[test]
fn the_pipe_is_this_users_and_closed_to_everyone_else() {
    let _test = begin("the_pipe_is_this_users_and_closed_to_everyone_else");
    let app = Harness::start("owner");
    let me = own_sid();

    // The first instance, and the one made to replace it once it was taken.
    let first = open_client(&app.pipe);
    let next = open_client(&app.pipe);
    for client in [&first, &next] {
        let security = pipe_security(client);
        let owner = security.owner.as_deref().expect("an owner");
        assert!(
            owner.eq_ignore_ascii_case(&me),
            "owned by {owner}, not {me}"
        );
        assert!(
            security.dacl_protected,
            "the DACL takes entries from a parent"
        );
        let aces = security.dacl.as_ref().expect("a DACL");
        assert_eq!(aces.len(), 2, "{aces:?}");
        assert!(aces.iter().all(|ace| ace.allow), "{aces:?}");
        let mut allowed: Vec<String> = aces
            .iter()
            .map(|ace| ace.sid.to_ascii_uppercase())
            .collect();
        allowed.sort();
        let mut expected = vec![me.to_ascii_uppercase(), SYSTEM_SID.to_owned()];
        expected.sort();
        assert_eq!(allowed, expected);
        assert!(security.is_ours(&me));
        assert!(!security.is_ours(SYSTEM_SID));
    }
}

// ---- other clients ----

/// A frame of protocol 1, exactly as the fixtures keep it, written by the app's own client.
#[test]
fn a_v1_frame_from_another_client_is_accepted() {
    let _test = begin("a_v1_frame_from_another_client_is_accepted");
    let mut app = Harness::start("v1");
    let send = |pipe: &str, name: &str| {
        let (pipe, frame) = (pipe.to_owned(), fixture(&format!("v1/{name}.json")));
        std::thread::spawn(move || client::request(&pipe, &frame, CLIENT_TIMEOUT))
    };

    // Fire and forget: taken, then closed with no frame.
    let sent = send(&app.pipe, "pre_tool_use");
    let event = app.wait_hook("PreToolUse");
    assert_eq!(event.pid, Some(4242));
    assert_eq!(event.tool_use_id.as_deref(), Some("toolu_01ABCDEF"));
    assert_eq!(event.config_dir_env.as_deref(), Some(CONFIG_DIR));
    assert_eq!(join(sent), Ok(None));

    // A request: held, and the answer is the frame the client reads.
    let sent = send(&app.pipe, "permission_request_bash");
    let held = app.wait_held();
    assert_eq!(held.event.tool.as_deref(), Some("Bash"));
    assert_eq!(
        app.ingress.answer(
            &held.session_id,
            &held.tool_use_id,
            PermissionResponse::allow()
        ),
        AnswerResult::Delivered
    );
    assert_eq!(join(sent), Ok(Some(PermissionResponse::allow().to_json())));
}

#[test]
fn control_requests_go_round() {
    let _test = begin("control_requests_go_round");
    let mut app = Harness::start("control");
    let ask = |pipe: &str, op: ControlOp| {
        let pipe = pipe.to_owned();
        std::thread::spawn(move || client::control(&pipe, op, CLIENT_TIMEOUT))
    };
    let asked = |app: &mut Harness, wanted: ControlOp| {
        app.wait_for("a control request", |step| {
            step.outs.iter().find_map(|out| match out {
                IngressOut::Control { conn, op } if *op == wanted => Some(*conn),
                _ => None,
            })
        })
    };

    let status = ControlStatus {
        version: "pipe-roundtrip".into(),
        sessions: 3,
        held: 1,
        hook_consent: "granted".into(),
        transport: "listening".into(),
        cloud: "signed_out".into(),
        ..ControlStatus::default()
    };
    let client = ask(&app.pipe, ControlOp::Status);
    let conn = asked(&mut app, ControlOp::Status);
    assert!(app
        .ingress
        .reply_control(conn, &ControlResponse::status(status.clone())));
    assert_eq!(join(client), Ok(ControlResponse::status(status)));

    let client = ask(&app.pipe, ControlOp::Quit);
    let conn = asked(&mut app, ControlOp::Quit);
    assert!(app.ingress.reply_control(conn, &ControlResponse::ok()));
    assert_eq!(join(client), Ok(ControlResponse::ok()));

    // A refusal is an answer too.
    let client = ask(&app.pipe, ControlOp::Quit);
    let conn = asked(&mut app, ControlOp::Quit);
    assert!(app
        .ingress
        .reply_control(conn, &ControlResponse::error("not now")));
    assert_eq!(join(client), Ok(ControlResponse::error("not now")));

    // An app that closes without one did not understand the request.
    let client = ask(&app.pipe, ControlOp::Status);
    let conn = asked(&mut app, ControlOp::Status);
    app.server.close(conn);
    assert!(matches!(join(client), Err(ClientError::Other(_))));

    // And with no app there is nobody to ask.
    assert_eq!(
        client::control(&unique_pipe("no-app"), ControlOp::Status, CLIENT_TIMEOUT),
        Err(ClientError::NotRunning)
    );
}

#[test]
fn a_client_that_sends_nothing_is_dropped_after_five_seconds() {
    let _test = begin("a_client_that_sends_nothing_is_dropped_after_five_seconds");
    let mut app = Harness::start("silent");

    let opened = Instant::now();
    let mut silent = open_client(&app.pipe);
    let (tell, closed) = mpsc::channel();
    std::thread::spawn(move || {
        // Returns when the server closes its end: nothing is ever sent this way.
        let mut byte = [0u8; 1];
        let read = silent.read(&mut byte).map_err(|error| error.raw_os_error());
        let _ = tell.send((read, opened.elapsed()));
    });

    // It does not stand in anyone's way meanwhile.
    let done = spawn(hook_command(&app.pipe), &fixture("stdin/pre_tool_use.json")).finish();
    assert_silent_success(&done, "PreToolUse");
    app.wait_hook("PreToolUse");

    let (read, after) = closed
        .recv_timeout(Duration::from_secs(15))
        .expect("the server dropped the silent client");
    // The end of the pipe: std reads a broken pipe as that; "no process on the other end" (233)
    // says the same.
    assert!(matches!(read, Ok(0) | Err(Some(109 | 233))), "{read:?}");
    let deadline = Duration::from_millis(SERVER_READ_DEADLINE_MS);
    assert!(after >= deadline, "dropped after only {after:?}");
    assert!(
        after < deadline + Duration::from_secs(3),
        "dropped after {after:?}"
    );
    // Nothing of it was ever delivered.
    assert_eq!(app.frames_during(SHORTLY).len(), 0);
}
