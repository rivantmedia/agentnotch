//! The Job-object runner (src/job.rs) against `fake-claude.exe` (D 2705): a whole tree ends on
//! `kill_tree` and on drop, stdio is piped, argv and the scrubbed environment reach the child as
//! they were given, and start failures are std's own errors.

#![cfg(windows)]

use agentnotch_engine::core::claude_json::ClaudeJsonReader;
use agentnotch_engine::model::{AccountId, ExpectedLogin, IdentityId, UsageSource};
use agentnotch_engine::platform::{Clock, CommandRunner, CommandSpec, Exit, RunningCommand};
use agentnotch_engine::runtime_types::{ClaudeBinary, ProbeOutcome, ProbePlan, RefreshReason};
use agentnotch_engine::testkit::{FakeClock, TEST_START_MS};
use agentnotch_engine::usage::probe::{run_probe_with, ProbeTiming, ARGUMENTS};
use agentnotch_engine::usage::scrubbed_env;
use agentnotch_win::job::JobRunner;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

/// What a Windows program needs to start at all, from the test's own environment.
fn system_env() -> Vec<(OsString, OsString)> {
    ["SystemRoot", "Path", "ComSpec"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.into(), value)))
        .collect()
}

fn spec(program: &Path, args: &[&str], extra_env: &[(&str, &OsString)], cwd: &Path) -> CommandSpec {
    let mut env = system_env();
    env.extend(
        extra_env
            .iter()
            .map(|(name, value)| (OsString::from(name), (*value).clone())),
    );
    CommandSpec {
        program: program.to_owned(),
        args: args.iter().map(OsString::from).collect(),
        env,
        cwd: cwd.to_owned(),
    }
}

fn start(spec: CommandSpec) -> Box<dyn RunningCommand> {
    JobRunner::new().spawn(spec).expect("the fake starts")
}

fn read_all(pipe: Option<Box<dyn Read + Send>>) -> String {
    let mut text = String::new();
    pipe.expect("a piped stream")
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// A handle that can be waited on, opened while the process surely still exists (so a reused
/// pid can't be mistaken for it later).
fn open_process(pid: u32) -> OwnedHandle {
    // SAFETY: plain call; the wrapper checks the handle, which is owned right below.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
        .unwrap_or_else(|error| panic!("process {pid} is not running: {error}"));
    // SAFETY: `raw` is a fresh, valid handle that nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw.0) }
}

fn ends_within(process: &OwnedHandle, limit: Duration) -> bool {
    // SAFETY: the handle is live and was opened with SYNCHRONIZE.
    let waited =
        unsafe { WaitForSingleObject(HANDLE(process.as_raw_handle()), limit.as_millis() as u32) };
    waited == WAIT_OBJECT_0
}

fn grandchild_pid(pid_file: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "the fake wrote no grandchild pid"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The fake started with a sleeping grandchild; returns it with handles to both processes.
fn tree(dir: &Path) -> (Box<dyn RunningCommand>, OwnedHandle, OwnedHandle) {
    let pid_file = dir.join("grandchild.pid");
    let pid_file_arg = pid_file.to_str().unwrap();
    let child = start(spec(
        Path::new(FAKE),
        &["--spawn-grandchild", pid_file_arg],
        &[],
        dir,
    ));
    let grandchild = grandchild_pid(&pid_file);
    assert_ne!(grandchild, child.pid());
    let child_process = open_process(child.pid());
    let grandchild_process = open_process(grandchild);
    (child, child_process, grandchild_process)
}

#[test]
fn kill_tree_ends_the_child_and_its_grandchild() {
    let dir = tempfile::tempdir().unwrap();
    let (mut child, child_process, grandchild_process) = tree(dir.path());

    assert_eq!(child.wait_timeout(Duration::ZERO).unwrap(), None);
    child.kill_tree();
    assert!(ends_within(&child_process, Duration::from_secs(5)));
    assert!(
        ends_within(&grandchild_process, Duration::from_secs(5)),
        "the grandchild outlived kill_tree"
    );
    assert_eq!(
        child.wait_timeout(Duration::from_secs(5)).unwrap(),
        Some(Exit::Killed)
    );
    // Idempotent: a second kill changes nothing and doesn't panic.
    child.kill_tree();
    assert_eq!(
        child.wait_timeout(Duration::ZERO).unwrap(),
        Some(Exit::Killed)
    );
}

#[test]
fn dropping_a_running_command_ends_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (child, child_process, grandchild_process) = tree(dir.path());

    // No kill_tree: closing the job's only handle is what ends them (KILL_ON_JOB_CLOSE).
    drop(child);
    assert!(
        ends_within(&child_process, Duration::from_secs(5)),
        "the child outlived its runner"
    );
    assert!(
        ends_within(&grandchild_process, Duration::from_secs(5)),
        "the grandchild outlived its runner"
    );
}

#[test]
fn wait_timeout_answers_none_while_the_child_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = start(spec(Path::new(FAKE), &["--sleep", "30"], &[], dir.path()));
    let started = Instant::now();
    assert_eq!(
        child.wait_timeout(Duration::from_millis(200)).unwrap(),
        None
    );
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert_eq!(child.wait_timeout(Duration::ZERO).unwrap(), None);
    child.kill_tree();
    assert_eq!(
        child.wait_timeout(Duration::from_secs(5)).unwrap(),
        Some(Exit::Killed)
    );
}

#[test]
fn exit_codes_and_stderr_come_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut failing = start(spec(
        Path::new(FAKE),
        &["--exit", "7", "--stderr", "Invalid API key"],
        &[],
        dir.path(),
    ));
    let stderr = read_all(failing.take_stderr());
    let stdout = read_all(failing.take_stdout());
    assert_eq!(
        failing.wait_timeout(Duration::from_secs(10)).unwrap(),
        Some(Exit::Code(7))
    );
    assert_eq!(stderr.trim(), "Invalid API key");
    assert!(stdout.is_empty());

    let mut fine = start(spec(Path::new(FAKE), &["--version"], &[], dir.path()));
    assert_eq!(read_all(fine.take_stdout()), "2.1.282 (Claude Code)\n");
    assert_eq!(
        fine.wait_timeout(Duration::from_secs(10)).unwrap(),
        Some(Exit::Code(0))
    );
    // A child that ended by itself before kill_tree keeps its own code.
    fine.kill_tree();
    assert_eq!(
        fine.wait_timeout(Duration::ZERO).unwrap(),
        Some(Exit::Code(0))
    );
}

#[test]
fn argv_comes_back_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let args = ["--echo-argv", r#"{"disableAllHooks":true}"#, "", "ünï cödé"];
    let mut child = start(spec(Path::new(FAKE), &args, &[], dir.path()));
    let out = read_all(child.take_stdout());
    assert_eq!(
        child.wait_timeout(Duration::from_secs(10)).unwrap(),
        Some(Exit::Code(0))
    );
    let argv: Vec<String> = serde_json::from_str(&out).unwrap();
    assert_eq!(argv, args);
}

fn log_lines(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn the_scrubbed_environment_reaches_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("fake-claude.log");
    let folder = r"C:\Users\me\.claude-work";

    // The app's own environment, as a run started from a Claude Code terminal would have it.
    let mut base: Vec<(OsString, OsString)> = vec![
        (
            "CLAUDE_CONFIG_DIR".into(),
            r"C:\Users\me\.claude-other".into(),
        ),
        (
            "ANTHROPIC_API_KEY".into(),
            "sk-ant-not-for-the-child".into(),
        ),
        ("CLAUDECODE".into(), "1".into()),
        ("claude_code_entrypoint".into(), "cli".into()),
        ("FAKE_CLAUDE_LOG".into(), log.clone().into_os_string()),
    ];
    base.extend(system_env());
    let binary_dir = Path::new(FAKE).parent().unwrap();
    let env = scrubbed_env(&base, Some(folder), binary_dir);

    let config_dirs: Vec<&OsString> = env
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .map(|(_, value)| value)
        .collect();
    assert_eq!(config_dirs, [&OsString::from(folder)]);

    let mut child = start(CommandSpec {
        program: PathBuf::from(FAKE),
        args: vec!["--version".into()],
        env,
        cwd: dir.path().to_owned(),
    });
    let _ = read_all(child.take_stdout());
    assert_eq!(
        child.wait_timeout(Duration::from_secs(10)).unwrap(),
        Some(Exit::Code(0))
    );
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["env"], json!(["CLAUDE_CONFIG_DIR"]));
    assert_eq!(lines[0]["argv"], json!(["--version"]));
}

#[test]
fn a_missing_program_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-claude.exe");
    let error = JobRunner::new()
        .spawn(spec(&missing, &["--version"], &[], dir.path()))
        .err()
        .expect("nothing to start");
    assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
}

#[test]
fn a_batch_shim_gets_escaped_arguments_and_refuses_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("claude.cmd");
    // It leaves a mark beside itself, so the test knows cmd.exe really ran it.
    std::fs::write(&shim, "@echo ran> \"%~dp0ran.txt\"\r\n@exit /b 3\r\n").unwrap();

    // std escapes what a batch file can take...
    let mut child = start(spec(
        &shim,
        &["--settings", r#"{"disableAllHooks":true}"#, ""],
        &[],
        dir.path(),
    ));
    assert_eq!(
        child.wait_timeout(Duration::from_secs(10)).unwrap(),
        Some(Exit::Code(3))
    );
    assert!(dir.path().join("ran.txt").exists(), "the shim did not run");

    // ...and refuses what it can't: its own error, unchanged, never a panic. The probe shows
    // SHIM_REFUSED_TEXT for exactly this kind.
    let error = JobRunner::new()
        .spawn(spec(&shim, &["say \"hi\"\nexit"], &[], dir.path()))
        .err()
        .expect("std refuses the argument");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
}

// MARK: - The probe conversation through the runner

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-claude-usage.json")
}

#[test]
fn stdin_and_stdout_carry_the_probe_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".claude-work");
    std::fs::create_dir_all(&config_dir).unwrap();
    let identity_file = config_dir.join(".claude.json");
    std::fs::write(
        &identity_file,
        serde_json::to_vec(&json!({"numStartups": 1, "oauthAccount": {
            "accountUuid": "acc-1", "emailAddress": "me@example.com"}}))
        .unwrap(),
    )
    .unwrap();
    let cwd = dir.path().join("usage-probe");
    std::fs::create_dir_all(&cwd).unwrap();
    let log = dir.path().join("fake-claude.log");
    let usage = fixture().into_os_string();
    let log_value = log.clone().into_os_string();
    let config_value = config_dir.clone().into_os_string();
    let spec = spec(
        Path::new(FAKE),
        &ARGUMENTS,
        &[
            ("CLAUDE_CONFIG_DIR", &config_value),
            ("FAKE_CLAUDE_LOG", &log_value),
            ("FAKE_CLAUDE_USAGE", &usage),
        ],
        &cwd,
    );
    let clock = FakeClock::at_ms(TEST_START_MS);
    let plan = ProbePlan {
        identity: IdentityId::from("uuid:acc-1"),
        folder: AccountId::from("dir:c:\\users\\me\\.claude-work"),
        config_dir,
        config_dir_env: None,
        binary: ClaudeBinary {
            program: PathBuf::from(FAKE),
            prefix_args: Vec::new(),
            version: None,
            shim: false,
        },
        spec,
        reason: RefreshReason::Manual,
        planned_at: clock.now(),
        identity_file,
        expected: ExpectedLogin {
            email: Some("me@example.com".into()),
            account_uuid: Some("acc-1".into()),
            organization_scope: None,
        },
    };
    let timing = ProbeTiming {
        timeout: Duration::from_secs(20),
        exit_grace: Duration::from_millis(300),
        close_grace: Duration::from_secs(5),
        kill_wait: Duration::from_secs(2),
    };

    let result = run_probe_with(
        &plan,
        &JobRunner::new(),
        &clock,
        &timing,
        &ClaudeJsonReader::new(),
    );
    let ProbeOutcome::Reading(usage) = result.outcome else {
        panic!("expected a reading, got {:?}", result.outcome);
    };
    assert_eq!(usage.source, UsageSource::Probe);
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(9.0));
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(3.0));

    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1, "one run");
    assert_eq!(lines[0]["argv"], json!(ARGUMENTS.to_vec()));
    assert_eq!(lines[0]["env"], json!(["CLAUDE_CONFIG_DIR"]));
}
