//! The test stand-in for `claude.exe` (src/bin/fake-claude.rs), proven on every OS, and the
//! usage probe run end to end against it. WP11's smoke test points `FAKE_CLAUDE_USAGE` at
//! `tests/fixtures/fake-claude-usage.json`.

use agentnotch_engine::core::claude_json::ClaudeJsonReader;
use agentnotch_engine::model::{AccountId, ExpectedLogin, IdentityId, UsageSource};
use agentnotch_engine::platform::{CommandRunner, CommandSpec, Exit, RunningCommand};
use agentnotch_engine::runtime_types::{ClaudeBinary, ProbeOutcome, ProbePlan, RefreshReason};
use agentnotch_engine::testkit::{FakeClock, TEST_START_MS};
use agentnotch_engine::usage::probe::{run_probe_with, ProbeTiming, ARGUMENTS};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-claude-usage.json")
}

fn fake() -> Command {
    let mut command = Command::new(FAKE);
    // The host's own variables must not leak into what a test reads back.
    command
        .env_remove("FAKE_CLAUDE_VERSION")
        .env_remove("FAKE_CLAUDE_ECHO_ARGV")
        .env_remove("FAKE_CLAUDE_LOG")
        .env_remove("FAKE_CLAUDE_USAGE");
    command
}

fn stdout_of(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn version_has_a_default_and_an_override() {
    assert_eq!(
        stdout_of(fake().arg("--version")),
        "2.1.282 (Claude Code)\n"
    );
    assert_eq!(
        stdout_of(
            fake()
                .arg("--version")
                .env("FAKE_CLAUDE_VERSION", "3.0.1 (Claude Code)")
        ),
        "3.0.1 (Claude Code)\n"
    );
}

#[test]
fn argv_comes_back_as_one_json_line() {
    let out =
        stdout_of(fake().args(["--echo-argv", r#"{"disableAllHooks":true}"#, "", "ünï cödé"]));
    assert_eq!(out.matches('\n').count(), 1);
    let argv: Vec<String> = serde_json::from_str(&out).unwrap();
    assert_eq!(
        argv,
        ["--echo-argv", r#"{"disableAllHooks":true}"#, "", "ünï cödé"]
    );

    // The environment switch works without the flag, and the answer is still argv.
    let out = stdout_of(
        fake()
            .env("FAKE_CLAUDE_ECHO_ARGV", "1")
            .args(["-p", "--verbose"]),
    );
    let argv: Vec<String> = serde_json::from_str(&out).unwrap();
    assert_eq!(argv, ["-p", "--verbose"]);
}

#[test]
fn unknown_arguments_are_ignored() {
    let status = fake()
        .args(["--model", "haiku", "--whatever"])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn exit_code_and_stderr_text() {
    let output = fake()
        .args(["--exit", "7", "--stderr", "Invalid API key"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "Invalid API key"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn sleeping_waits_then_exits() {
    let started = Instant::now();
    let status = fake()
        .args(["--sleep", "0.2", "--exit", "3"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(3));
    assert!(started.elapsed() >= Duration::from_millis(200));
}

#[test]
fn a_grandchild_is_started_and_its_pid_written() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let mut parent = fake()
        .arg("--spawn-grandchild")
        .arg(&pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "no pid file");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(pid, parent.id());
    // Both sleep for a bounded time; end the parent now. The grandchild outlives it by design
    // (a runner's kill_tree is what ends it) and dies by itself within the sleep cap.
    parent.kill().unwrap();
    let _ = parent.wait();
}

fn log_lines(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn the_log_names_variables_but_never_their_values() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("fake-claude.log");
    let secret = "sk-ant-SECRET-VALUE-1234";
    let config = "C:\\Users\\me\\.claude-work-VALUE";
    for _ in 0..2 {
        fake()
            .current_dir(dir.path())
            .env("FAKE_CLAUDE_LOG", &log)
            .env("ANTHROPIC_API_KEY", secret)
            .env("CLAUDE_CONFIG_DIR", config)
            .env("claude_code_entrypoint", "vscode")
            .env("UNRELATED", "kept out")
            .args(["--version", "--settings", r#"{"a":1}"#])
            .output()
            .unwrap();
    }
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains(secret) && !text.contains(config) && !text.contains("kept out"));
    assert!(!text.contains("vscode"));
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 2, "one line per run");
    let names: Vec<&str> = lines[0]["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(names.contains(&"CLAUDE_CONFIG_DIR"));
    assert!(names.contains(&"ANTHROPIC_API_KEY"));
    assert!(names.contains(&"claude_code_entrypoint"));
    assert!(!names.contains(&"UNRELATED"));
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(
        lines[0]["argv"],
        json!(["--version", "--settings", r#"{"a":1}"#])
    );
    assert_eq!(
        std::fs::canonicalize(lines[0]["cwd"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

// MARK: - The probe, end to end

/// A `CommandRunner` over `std::process`: enough for a fake that starts no grandchild.
struct StdRunner;

struct StdChild {
    child: Child,
}

impl CommandRunner for StdRunner {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(Box::new(StdChild {
            child: command.spawn()?,
        }))
    }
}

impl RunningCommand for StdChild {
    fn pid(&self) -> u32 {
        self.child.id()
    }
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.child.stdin.take().map(|s| Box::new(s) as _)
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child.stdout.take().map(|s| Box::new(s) as _)
    }
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child.stderr.take().map(|s| Box::new(s) as _)
    }
    fn wait_timeout(&mut self, d: Duration) -> io::Result<Option<Exit>> {
        let deadline = Instant::now() + d;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status.code().map_or(Exit::Killed, Exit::Code)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn kill_tree(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for StdChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Probe {
    _dir: tempfile::TempDir,
    cwd: PathBuf,
    log: PathBuf,
    plan: ProbePlan,
    clock: FakeClock,
}

fn probe_setup(usage: Option<&Path>) -> Probe {
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
    let log = dir.path().join("fake-claude.log");
    let mut env: Vec<(OsString, OsString)> = vec![
        (
            "CLAUDE_CONFIG_DIR".into(),
            config_dir.clone().into_os_string(),
        ),
        ("FAKE_CLAUDE_LOG".into(), log.clone().into_os_string()),
    ];
    if let Some(usage) = usage {
        env.push(("FAKE_CLAUDE_USAGE".into(), usage.as_os_str().to_owned()));
    }
    // Windows programs may not start without their system folder.
    for name in ["SystemRoot", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            env.push((name.into(), value));
        }
    }
    let spec = CommandSpec {
        program: PathBuf::from(FAKE),
        args: ARGUMENTS.iter().map(OsString::from).collect(),
        env,
        cwd: cwd.clone(),
    };
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
        planned_at: agentnotch_engine::platform::Clock::now(&clock),
        identity_file,
        expected: ExpectedLogin {
            email: Some("me@example.com".into()),
            account_uuid: Some("acc-1".into()),
            organization_scope: None,
        },
    };
    Probe {
        _dir: dir,
        cwd,
        log,
        plan,
        clock,
    }
}

fn run(probe: &Probe) -> ProbeOutcome {
    let timing = ProbeTiming {
        timeout: Duration::from_secs(20),
        exit_grace: Duration::from_millis(300),
        close_grace: Duration::from_secs(5),
        kill_wait: Duration::from_secs(2),
    };
    run_probe_with(
        &probe.plan,
        &StdRunner,
        &probe.clock,
        &timing,
        &ClaudeJsonReader::new(),
    )
    .outcome
}

#[test]
fn the_probe_reads_the_fixtures_usage_from_the_fake() {
    let probe = probe_setup(Some(&fixture()));
    let ProbeOutcome::Reading(usage) = run(&probe) else {
        panic!("expected a reading");
    };
    assert_eq!(usage.source, UsageSource::Probe);
    assert_eq!(usage.five_hour.as_ref().map(|w| w.utilization), Some(9.0));
    assert_eq!(usage.seven_day.as_ref().map(|w| w.utilization), Some(3.0));

    let lines = log_lines(&probe.log);
    assert_eq!(lines.len(), 1, "one run");
    let expected: Vec<&str> = ARGUMENTS.to_vec();
    assert_eq!(lines[0]["argv"], json!(expected));
    assert_eq!(
        std::fs::canonicalize(lines[0]["cwd"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&probe.cwd).unwrap()
    );
    assert_eq!(lines[0]["env"], json!(["CLAUDE_CONFIG_DIR"]));
}

#[test]
fn a_missing_fixture_is_an_error_answer_not_a_hang() {
    let probe = probe_setup(None);
    let ProbeOutcome::Failed(reason) = run(&probe) else {
        panic!("expected a failure");
    };
    assert!(reason.contains("fake-claude: no usage fixture"), "{reason}");
}
