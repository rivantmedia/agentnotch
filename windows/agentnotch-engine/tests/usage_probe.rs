//! The usage probe (UsageProbeTests.swift): its stream-json lines, its
//! conversation with a scripted `claude` (never a real one), its time limits,
//! and the Windows specifics: `\r\n` output, a refused npm shim, and the
//! folder-changed-hands checks around the run.

use agentnotch_engine::core::claude_json::ClaudeJsonReader;
use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::model::{AccountId, ExpectedLogin, IdentityId, UsageSource};
use agentnotch_engine::platform::{CommandRunner, CommandSpec, Exit};
use agentnotch_engine::runtime_types::{
    ClaudeBinary, ProbeOutcome, ProbePlan, ProbeResult, RefreshReason,
};
use agentnotch_engine::testkit::runner::Conversation;
use agentnotch_engine::testkit::{FakeClock, Script, ScriptedRunner, TEST_START_MS};
use agentnotch_engine::usage::env::scrubbed_env_in;
use agentnotch_engine::usage::planner::STORE_MARKER;
use agentnotch_engine::usage::probe::{
    arguments, converse, outcome_for_error_message, parse_line, run_probe_with, LineEvent, Outcome,
    ProbeTiming, ARGUMENTS, INITIALIZE_REQUEST, INITIALIZE_REQUEST_ID, SHIM_REFUSED_TEXT,
    USAGE_REQUEST, USAGE_REQUEST_ID,
};
use agentnotch_engine::usage::schedule::{NOT_SIGNED_IN_TEXT, NO_RUN_FOLDER_TEXT};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// MARK: - Lines (UsageProbeParsingTests)

#[test]
fn initialize_response() {
    assert_eq!(
        parse_line(br#"{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-init","response":{"account":{"email":"x"}}}}"#),
        LineEvent::Initialized { error: None }
    );
    assert_eq!(
        parse_line(br#"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-init","error":"boom"}}"#),
        LineEvent::Initialized {
            error: Some("boom".into())
        }
    );
}

#[test]
fn usage_response() {
    let event = parse_line(br#"{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-usage","response":{"subscription_type":"max","rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":23,"resets_at":"2026-09-24T12:50:00.257626+00:00"},"seven_day":{"utilization":41,"resets_at":"2026-09-29T06:00:00+00:00"},"model_scoped":[]},"behaviors":null}}}"#);
    let LineEvent::Usage(Outcome::Usage(usage)) = event else {
        panic!("expected usage, got {event:?}");
    };
    assert_eq!(usage.five_hour.map(|w| w.utilization), Some(23.0));
    assert_eq!(usage.subscription_type.as_deref(), Some("max"));
}

#[test]
fn usage_errors() {
    assert_eq!(
        parse_line(r#"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-usage","error":"Not logged in · Please run /login"}}"#.as_bytes()),
        LineEvent::Usage(Outcome::Unavailable("Not signed in to Claude".into()))
    );
    assert_eq!(
        parse_line(br#"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-usage","error":"Request failed with status 429"}}"#),
        LineEvent::Usage(Outcome::RateLimited)
    );
    assert_eq!(
        parse_line(br#"{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-usage","response":{"rate_limits_available":false}}}"#),
        LineEvent::Usage(Outcome::Unavailable(
            "Usage limits aren't available for this login".into()
        ))
    );

    // Every rule of the error text, whatever its case.
    assert_eq!(
        outcome_for_error_message("Rate Limit exceeded"),
        Outcome::RateLimited
    );
    for text in [
        "NOT LOGGED IN",
        "please run /login first",
        "This organization uses Claude.ai",
    ] {
        assert_eq!(
            outcome_for_error_message(text),
            Outcome::Unavailable(NOT_SIGNED_IN_TEXT.into()),
            "{text}"
        );
    }
    let long = "é".repeat(250);
    assert_eq!(
        outcome_for_error_message(&long),
        Outcome::Failed("é".repeat(200))
    );
    // An error without text, and a success without a body.
    assert_eq!(
        parse_line(br#"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-usage"}}"#),
        LineEvent::Usage(Outcome::Failed("Request failed".into()))
    );
    assert_eq!(
        parse_line(br#"{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-usage"}}"#),
        LineEvent::Usage(Outcome::Failed("Empty usage response".into()))
    );
}

#[test]
fn other_lines_are_ignored() {
    assert_eq!(
        parse_line(br#"{"type":"system","subtype":"init","session_id":"x"}"#),
        LineEvent::Other
    );
    assert_eq!(
        parse_line(
            br#"{"type":"control_response","response":{"subtype":"success","request_id":"other"}}"#
        ),
        LineEvent::Other
    );
    assert_eq!(parse_line(b"not json"), LineEvent::Other);
}

#[test]
fn requests_are_single_lines() {
    for request in [INITIALIZE_REQUEST, USAGE_REQUEST] {
        assert!(request.ends_with('\n'));
        assert!(!request[..request.len() - 1].contains('\n'));
        // Keys sorted, as the Mac's JSONSerialization writes them.
        let value: Value = serde_json::from_str(request).unwrap();
        assert_eq!(serde_json::to_string(&value).unwrap() + "\n", request);
    }
    let usage: Value = serde_json::from_str(USAGE_REQUEST).unwrap();
    assert_eq!(usage["request"]["subtype"], "get_usage");
    assert_eq!(usage["request"]["skip_behaviors"], true);
    assert_eq!(usage["request_id"], USAGE_REQUEST_ID);
    let initialize: Value = serde_json::from_str(INITIALIZE_REQUEST).unwrap();
    assert_eq!(initialize["request"]["subtype"], "initialize");
    assert_eq!(initialize["request_id"], INITIALIZE_REQUEST_ID);

    assert_eq!(
        ARGUMENTS,
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--settings",
            r#"{"disableAllHooks":true}"#,
        ]
    );
    assert_eq!(arguments(&[]), os(&ARGUMENTS));
    // Node runs the npm package's cli.js first.
    let cli = OsString::from(r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js");
    let with_node = arguments(std::slice::from_ref(&cli));
    assert_eq!(with_node[0], cli);
    assert_eq!(with_node[1..], os(&ARGUMENTS)[..]);
}

// MARK: - A scripted claude

fn os(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn line(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

fn success(id: &str, payload: Value) -> Vec<u8> {
    line(json!({"type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": payload}}))
}

fn failure(id: &str, error: &str) -> Vec<u8> {
    line(json!({"type": "control_response",
        "response": {"subtype": "error", "request_id": id, "error": error}}))
}

/// The Mac stub's answer (UsageProbeProcessTests).
fn stub_usage() -> Value {
    json!({"subscription_type": "pro", "rate_limits_available": true, "rate_limits": {
        "five_hour": {"utilization": 55, "resets_at": "2026-09-24T12:50:00.257626+00:00"},
        "seven_day": {"utilization": 12, "resets_at": "2026-09-29T06:00:00+00:00"},
        "model_scoped": [{"display_name": "Opus", "utilization": 4, "resets_at": "2026-09-29T06:00:00+00:00"}]}})
}

/// A fresh answer (it lists `limits`, so it isn't Claude Code's fallback).
fn fresh_usage() -> Value {
    let mut usage = stub_usage();
    usage["rate_limits"]["limits"] = json!([]);
    usage
}

/// A `claude` that prints its init line, answers `initialize` with a large
/// body (like the real one) and then `get_usage` with `usage_line`.
fn talking_claude(usage_line: Vec<u8>) -> Conversation {
    Conversation::new()
        .print(line(json!({"type": "system", "subtype": "init"})))
        .on_request(
            INITIALIZE_REQUEST_ID,
            vec![success(
                INITIALIZE_REQUEST_ID,
                json!({"commands": ["x".repeat(70_000)]}),
            )],
        )
        .on_request(USAGE_REQUEST_ID, vec![usage_line])
}

/// Everything the app's own environment could hand a child that runs as
/// another login.
fn app_env() -> Vec<(OsString, OsString)> {
    [
        ("Path", r"C:\Windows\system32"),
        ("SystemRoot", r"C:\Windows"),
        ("CLAUDECODE", "1"),
        ("CLAUDE_PID", "123"),
        ("CLAUDE_EFFORT", "high"),
        ("CLAUDE_CODE_SESSION_ID", "s"),
        ("CLAUDE_CODE_MESSAGING_TOKEN", "secret"),
        ("CLAUDE_AGENT_SDK_VERSION", "1"),
        ("AI_AGENT", "claude"),
        ("CLAUDE_CONFIG_DIR", r"C:\Users\me\.claude-other"),
        ("ANTHROPIC_API_KEY", "sk-ant-secret"),
    ]
    .iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    .collect()
}

const RAW_DIR: &str = r"C:\Users\me\.claude-work\";

fn spec(dir: &Path, config_dir_env: Option<&str>) -> CommandSpec {
    let bin = dir.join("bin");
    CommandSpec {
        program: bin.join("claude.exe"),
        args: arguments(&[]),
        env: scrubbed_env_in(PathStyle::Windows, &app_env(), config_dir_env, &bin),
        cwd: dir.join("support").join("usage-probe"),
    }
}

fn timing() -> ProbeTiming {
    ProbeTiming {
        timeout: Duration::from_secs(10),
        exit_grace: Duration::from_millis(300),
        close_grace: Duration::from_millis(200),
        kill_wait: Duration::from_secs(1),
    }
}

// MARK: - The conversation (UsageProbeProcessTests)

#[test]
fn probe_returns_usage_and_scrubs_env() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    runner.push_conversation(talking_claude(success(USAGE_REQUEST_ID, stub_usage())));

    let outcome = converse(spec(dir.path(), Some(RAW_DIR)), false, &runner, &timing());
    let Outcome::Usage(usage) = outcome else {
        panic!("expected usage, got {outcome:?}");
    };
    assert_eq!(usage.five_hour.map(|w| w.utilization), Some(55.0));
    assert_eq!(usage.seven_day.map(|w| w.utilization), Some(12.0));
    assert_eq!(
        usage
            .scoped
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Opus"]
    );
    assert_eq!(usage.subscription_type.as_deref(), Some("pro"));

    let spawned = runner.spawned();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].args, os(&ARGUMENTS));
    let bin = dir.path().join("bin");
    let expected_path = format!(r"{};C:\Windows\system32", bin.display());
    assert_eq!(
        spawned[0].env,
        vec![
            (OsString::from("Path"), OsString::from(expected_path)),
            ("SystemRoot".into(), r"C:\Windows".into()),
            ("CLAUDE_CONFIG_DIR".into(), RAW_DIR.into()),
        ]
    );
    // Both requests, in order; then stdin closed and the child left by
    // itself, so nothing was killed.
    assert_eq!(
        runner.stdin_of(0).unwrap(),
        format!("{INITIALIZE_REQUEST}{USAGE_REQUEST}").into_bytes()
    );
    assert_eq!(runner.stdin_closed(0), Some(true));
    assert_eq!(runner.kills_of(0), Some(0));
    // Claude Code ran in the app's own folder, made for it.
    assert!(spawned[0].cwd.is_dir());
}

#[test]
fn the_usage_request_waits_for_initialize() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    // Never answers initialize, but would answer get_usage at once.
    runner.push_conversation(Conversation::hanging().on_request(
        USAGE_REQUEST_ID,
        vec![success(USAGE_REQUEST_ID, stub_usage())],
    ));
    let short = ProbeTiming {
        timeout: Duration::from_millis(300),
        close_grace: Duration::from_millis(50),
        ..timing()
    };
    let outcome = converse(spec(dir.path(), None), false, &runner, &short);
    assert!(
        matches!(&outcome, Outcome::Failed(text) if text.starts_with("Claude Code didn't answer within")),
        "{outcome:?}"
    );
    assert_eq!(runner.stdin_of(0).unwrap(), INITIALIZE_REQUEST.as_bytes());
}

#[test]
fn error_response_becomes_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    runner.push_conversation(talking_claude(failure(
        USAGE_REQUEST_ID,
        "Not logged in · Please run /login",
    )));
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Unavailable("Not signed in to Claude".into())
    );
}

#[test]
fn early_exit_is_reported_with_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    runner.push(Script {
        stdout: Vec::new(),
        stderr: b"Error: Invalid API key\n".to_vec(),
        exit: Exit::Code(1),
    });
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Failed("Claude Code exited (1): Error: Invalid API key".into())
    );
}

#[test]
fn an_exit_after_initialize_names_the_last_stderr_line() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    // Answers initialize, then exits before get_usage is answered.
    let mut stdout = success(INITIALIZE_REQUEST_ID, json!({}));
    stdout.push(b'\n');
    runner.push(Script {
        stdout,
        stderr: b"warming up\n  Error: something broke \t\n\n".to_vec(),
        exit: Exit::Code(3),
    });
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Failed("Claude Code exited (3): Error: something broke".into())
    );
    // Silent: only the status.
    runner.push(Script {
        stdout: Vec::new(),
        stderr: b" \n".to_vec(),
        exit: Exit::Code(9),
    });
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Failed("Claude Code exited with status 9".into())
    );
}

#[test]
fn hanging_child_is_killed_and_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    runner.push_conversation(Conversation::hanging());
    let short = ProbeTiming {
        timeout: Duration::from_millis(300),
        exit_grace: Duration::from_millis(100),
        close_grace: Duration::from_millis(100),
        kill_wait: Duration::from_secs(2),
    };
    let started = Instant::now();
    let outcome = converse(spec(dir.path(), None), false, &runner, &short);
    let took = started.elapsed();
    assert_eq!(
        outcome,
        Outcome::Failed("Claude Code didn't answer within 0s".into())
    );
    // It waited the whole timeout and the close grace, then ended the tree
    // once, which the killed child's wait reports at once.
    assert!(took >= Duration::from_millis(400), "{took:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert_eq!(runner.kills_of(0), Some(1));
    assert_eq!(runner.stdin_closed(0), Some(true));
}

#[test]
fn the_default_time_limits() {
    let defaults = ProbeTiming::default();
    assert_eq!(defaults.timeout, Duration::from_secs(20));
    assert_eq!(defaults.close_grace, Duration::from_secs(2));
    assert_eq!(defaults.exit_grace, Duration::from_millis(300));
    assert_eq!(defaults.kill_wait, Duration::from_secs(3));
}

#[test]
fn missing_binary_fails() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    runner.push_spawn_error(std::io::ErrorKind::NotFound, "program not found");
    let outcome = converse(spec(dir.path(), None), false, &runner, &timing());
    assert_eq!(
        outcome,
        Outcome::Failed("Couldn't start Claude Code: program not found".into())
    );
    // With nothing planned at all, the same.
    let outcome = converse(spec(dir.path(), None), false, &runner, &timing());
    assert!(
        matches!(&outcome, Outcome::Failed(text) if text.starts_with("Couldn't start Claude Code: ")),
        "{outcome:?}"
    );
}

// MARK: - Windows specifics

#[test]
fn a_trailing_carriage_return_is_stripped() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    let crlf = |mut bytes: Vec<u8>| {
        bytes.push(b'\r');
        bytes
    };
    runner.push_conversation(
        Conversation::new()
            .print(crlf(line(json!({"type": "system", "subtype": "init"}))))
            .on_request(
                INITIALIZE_REQUEST_ID,
                vec![crlf(success(INITIALIZE_REQUEST_ID, json!({})))],
            )
            .on_request(
                USAGE_REQUEST_ID,
                vec![crlf(success(USAGE_REQUEST_ID, stub_usage()))],
            ),
    );
    let outcome = converse(spec(dir.path(), None), false, &runner, &timing());
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");
}

#[test]
fn a_refused_npm_shim_says_to_choose_claude_exe() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    let refusal = "batch file arguments are invalid";
    runner.push_spawn_error(std::io::ErrorKind::InvalidInput, refusal);
    assert_eq!(
        converse(spec(dir.path(), None), true, &runner, &timing()),
        Outcome::Failed(SHIM_REFUSED_TEXT.into())
    );
    // The same refusal of a real exe is only a failure to start.
    runner.push_spawn_error(std::io::ErrorKind::InvalidInput, refusal);
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Failed(format!("Couldn't start Claude Code: {refusal}"))
    );
    // A shim that can't be found is not a refusal.
    runner.push_spawn_error(std::io::ErrorKind::NotFound, "not found");
    assert_eq!(
        converse(spec(dir.path(), None), true, &runner, &timing()),
        Outcome::Failed("Couldn't start Claude Code: not found".into())
    );
}

#[test]
fn an_initialize_error_ends_the_probe_without_asking_for_usage() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::default();
    runner.push_conversation(
        Conversation::new()
            .on_request(
                INITIALIZE_REQUEST_ID,
                vec![failure(INITIALIZE_REQUEST_ID, "boom")],
            )
            .on_request(
                USAGE_REQUEST_ID,
                vec![success(USAGE_REQUEST_ID, stub_usage())],
            ),
    );
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Failed("boom".into())
    );
    assert_eq!(runner.stdin_of(0).unwrap(), INITIALIZE_REQUEST.as_bytes());
    assert_eq!(runner.stdin_closed(0), Some(true));

    // A login error at initialize is the same verdict as at get_usage.
    runner.push_conversation(Conversation::new().on_request(
        INITIALIZE_REQUEST_ID,
        vec![failure(
            INITIALIZE_REQUEST_ID,
            "Not logged in · Please run /login",
        )],
    ));
    assert_eq!(
        converse(spec(dir.path(), None), false, &runner, &timing()),
        Outcome::Unavailable(NOT_SIGNED_IN_TEXT.into())
    );
}

#[test]
fn the_scripted_runner_hangs_until_killed() {
    let runner = ScriptedRunner::default();
    runner.push_conversation(Conversation::hanging());
    let dir = tempfile::tempdir().unwrap();
    let mut child = runner.spawn(spec(dir.path(), None)).unwrap();
    let mut stdout = child.take_stdout().unwrap();
    let reader = std::thread::spawn(move || {
        let mut all = Vec::new();
        stdout.read_to_end(&mut all).map(|_| all)
    });
    let started = Instant::now();
    assert_eq!(child.wait_timeout(Duration::from_millis(80)).unwrap(), None);
    assert!(started.elapsed() >= Duration::from_millis(80));
    // Closing its input doesn't end a hanging child.
    drop(child.take_stdin());
    assert_eq!(child.wait_timeout(Duration::ZERO).unwrap(), None);
    assert!(!reader.is_finished());

    child.kill_tree();
    child.kill_tree();
    assert_eq!(
        child.wait_timeout(Duration::from_secs(5)).unwrap(),
        Some(Exit::Killed)
    );
    assert_eq!(reader.join().unwrap().unwrap(), Vec::<u8>::new());
    assert_eq!(runner.kills_of(0), Some(2));
    assert_eq!(runner.stdin_closed(0), Some(true));
}

// MARK: - The run: who the folder is signed in as, before and after

const EXPECTED_ID: &str = "uuid:acc-1";

fn write_login(path: &Path, account_uuid: &str, email: &str) {
    let text = json!({"numStartups": 1,
        "oauthAccount": {"accountUuid": account_uuid, "emailAddress": email}});
    std::fs::write(path, serde_json::to_vec(&text).unwrap()).unwrap();
}

struct Setup {
    _dir: tempfile::TempDir,
    config_dir: PathBuf,
    plan: ProbePlan,
    clock: FakeClock,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".claude-work");
    std::fs::create_dir_all(&config_dir).unwrap();
    let identity_file = config_dir.join(".claude.json");
    write_login(&identity_file, "acc-1", "me@example.com");
    let clock = FakeClock::at_ms(TEST_START_MS);
    let spec = spec(dir.path(), Some(RAW_DIR));
    let plan = ProbePlan {
        identity: IdentityId::from(EXPECTED_ID),
        folder: AccountId::from("dir:c:\\users\\me\\.claude-work"),
        config_dir: config_dir.clone(),
        config_dir_env: Some(RAW_DIR.to_owned()),
        binary: ClaudeBinary {
            program: spec.program.clone(),
            prefix_args: Vec::new(),
            version: None,
            shim: false,
        },
        spec,
        reason: RefreshReason::Manual,
        planned_at: agentnotch_engine::platform::Clock::now(&clock),
        identity_file,
        expected: ExpectedLogin {
            email: Some("me@example.com".into()),
            account_uuid: Some("acc-1".into()),
            organization_scope: None,
        },
    };
    Setup {
        _dir: dir,
        config_dir,
        plan,
        clock,
    }
}

fn run(setup: &Setup, runner: &ScriptedRunner) -> ProbeResult {
    run_probe_with(
        &setup.plan,
        runner,
        &setup.clock,
        &timing(),
        &ClaudeJsonReader::new(),
    )
}

#[test]
fn a_run_returns_a_reading_dated_by_the_probe() {
    let setup = setup();
    let runner = ScriptedRunner::default();
    runner.push_conversation(talking_claude(success(USAGE_REQUEST_ID, fresh_usage())));
    let result = run(&setup, &runner);
    let ProbeOutcome::Reading(usage) = &result.outcome else {
        panic!("expected a reading, got {:?}", result.outcome);
    };
    let now = agentnotch_engine::platform::Clock::now(&setup.clock);
    assert_eq!(usage.account_id, IdentityId::from(EXPECTED_ID));
    assert_eq!(usage.source, UsageSource::Probe);
    assert_eq!(usage.updated_at, now);
    assert_eq!(usage.taken_after, Some(now));
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(55.0));
    assert_eq!(
        result.folder_identity_after,
        Some(IdentityId::from(EXPECTED_ID))
    );
    assert_eq!(result.plan, setup.plan);
    assert_eq!(runner.spawned(), vec![setup.plan.spec.clone()]);
}

#[test]
fn a_run_maps_the_outcomes() {
    let setup = setup();
    let runner = ScriptedRunner::default();
    runner.push_conversation(talking_claude(failure(
        USAGE_REQUEST_ID,
        "Not logged in · Please run /login",
    )));
    assert_eq!(run(&setup, &runner).outcome, ProbeOutcome::SignedOut);

    runner.push_conversation(talking_claude(failure(
        USAGE_REQUEST_ID,
        "Request failed with status 429",
    )));
    assert_eq!(
        run(&setup, &runner).outcome,
        ProbeOutcome::RateLimited { retry_after: None }
    );

    runner.push_conversation(talking_claude(success(
        USAGE_REQUEST_ID,
        json!({"rate_limits_available": false}),
    )));
    assert_eq!(
        run(&setup, &runner).outcome,
        ProbeOutcome::Unavailable("Usage limits aren't available for this login".into())
    );

    // A possibly seeded answer that `.claude.json` holds no copy of can't be
    // dated: it counts as rate limited.
    runner.push_conversation(talking_claude(success(USAGE_REQUEST_ID, stub_usage())));
    assert_eq!(
        run(&setup, &runner).outcome,
        ProbeOutcome::RateLimited { retry_after: None }
    );
}

#[test]
fn a_store_is_never_run() {
    let setup = setup();
    std::fs::write(setup.config_dir.join(STORE_MARKER), b"").unwrap();
    let runner = ScriptedRunner::default();
    runner.push_conversation(talking_claude(success(USAGE_REQUEST_ID, fresh_usage())));
    let result = run(&setup, &runner);
    assert_eq!(
        result.outcome,
        ProbeOutcome::Unavailable(NO_RUN_FOLDER_TEXT.into())
    );
    assert_eq!(
        result.folder_identity_after,
        Some(IdentityId::from(EXPECTED_ID))
    );
    assert!(runner.spawned().is_empty());
}

#[test]
fn a_folder_signed_in_as_another_login_is_not_run() {
    let setup = setup();
    write_login(&setup.plan.identity_file, "acc-2", "other@example.com");
    let runner = ScriptedRunner::default();
    runner.push_conversation(talking_claude(success(USAGE_REQUEST_ID, fresh_usage())));
    let result = run(&setup, &runner);
    assert_eq!(
        result.outcome,
        ProbeOutcome::Failed(format!(
            "{} is signed in as another account now",
            setup.config_dir.display()
        ))
    );
    assert_eq!(
        result.folder_identity_after,
        Some(IdentityId::from("uuid:acc-2"))
    );
    assert!(runner.spawned().is_empty());

    // Signed out: nobody to report.
    std::fs::remove_file(&setup.plan.identity_file).unwrap();
    let result = run(&setup, &runner);
    assert!(matches!(result.outcome, ProbeOutcome::Failed(_)));
    assert_eq!(result.folder_identity_after, None);
    assert!(runner.spawned().is_empty());
}

#[test]
fn a_folder_that_changed_hands_during_the_run_is_reported() {
    let setup = setup();
    let identity_file = setup.plan.identity_file.clone();
    let runner = ScriptedRunner::default();
    // A /login to another account while Claude Code answers.
    runner.push_conversation(
        Conversation::new()
            .on_request(
                INITIALIZE_REQUEST_ID,
                vec![success(INITIALIZE_REQUEST_ID, json!({}))],
            )
            .on_request_with(USAGE_REQUEST_ID, move |_| {
                write_login(&identity_file, "acc-2", "someone.else@example.com");
                vec![success(USAGE_REQUEST_ID, fresh_usage())]
            }),
    );
    let result = run(&setup, &runner);
    assert_eq!(
        result.outcome,
        ProbeOutcome::Failed(format!(
            "{} changed hands during the check",
            setup.config_dir.display()
        ))
    );
    assert_eq!(
        result.folder_identity_after,
        Some(IdentityId::from("uuid:acc-2"))
    );
    assert_eq!(runner.spawned().len(), 1);
}
