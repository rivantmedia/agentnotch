//! Test-only stand-in for `claude.exe` (DESIGN-WIN §2.2, §7.3, §7.5; WP4); never shipped.
//!
//! Std plus `serde_json` only, so it builds and runs on every OS and the Mac can prove the
//! probe's conversation against it. What it does, in the order it looks at its arguments:
//!
//! - every run first appends one JSON line to `$FAKE_CLAUDE_LOG` when set: `argv` (after the
//!   program), `cwd`, and `env`, the sorted **names** (never values) of the variables that
//!   match `CLAUDE*` or `ANTHROPIC*`, case-insensitively;
//! - `--version` prints `$FAKE_CLAUDE_VERSION` (default `2.1.282 (Claude Code)`);
//! - `--echo-argv` (or `$FAKE_CLAUDE_ECHO_ARGV=1`) prints argv as one JSON array line;
//! - `--spawn-grandchild <pid file>` starts a copy of itself that sleeps, writes its pid to the
//!   file, then sleeps itself;
//! - `--sleep <s>` sleeps (never longer than [`MAX_SLEEP_SECS`], so a process a failed test
//!   leaves behind dies by itself), then `--exit <code>` exits with `--stderr <text>` printed;
//! - with the probe's arguments (`--input-format stream-json`) it reads stdin lines and answers
//!   the `initialize` control request, then `get_usage` with the object in the file named by
//!   `$FAKE_CLAUDE_USAGE`, and exits 0 at stdin EOF.
//!
//! Anything else Claude Code's real flags carry is ignored.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::{Command, ExitCode};
use std::time::Duration;

/// The longest any sleep lasts.
const MAX_SLEEP_SECS: u64 = 60;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    log_run(&args);

    if args.iter().any(|a| a == "--version") {
        let version =
            std::env::var("FAKE_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.282 (Claude Code)".into());
        println!("{version}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--echo-argv")
        || std::env::var("FAKE_CLAUDE_ECHO_ARGV").is_ok_and(|v| v == "1")
    {
        println!("{}", Value::from(args));
        return ExitCode::SUCCESS;
    }
    if let Some(pid_file) = value_after(&args, "--spawn-grandchild") {
        return spawn_grandchild(&pid_file);
    }

    let sleep = value_after(&args, "--sleep").and_then(|s| s.parse::<f64>().ok());
    let exit = value_after(&args, "--exit").and_then(|s| s.parse::<i32>().ok());
    if sleep.is_some() || exit.is_some() {
        if let Some(seconds) = sleep {
            nap(seconds);
        }
        if let Some(text) = value_after(&args, "--stderr") {
            eprintln!("{text}");
        }
        return ExitCode::from(exit.unwrap_or(0) as u8);
    }

    if value_after(&args, "--input-format").as_deref() == Some("stream-json") {
        answer_probe();
    }
    ExitCode::SUCCESS
}

/// The argument after `flag`, when there is one.
fn value_after(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).cloned()
}

fn nap(seconds: f64) {
    let seconds = if seconds.is_finite() { seconds } else { 0.0 };
    std::thread::sleep(Duration::from_secs_f64(
        seconds.clamp(0.0, MAX_SLEEP_SECS as f64),
    ));
}

/// One line per run in `$FAKE_CLAUDE_LOG`: what the process was given, never a secret.
fn log_run(args: &[String]) {
    let Some(path) = std::env::var_os("FAKE_CLAUDE_LOG") else {
        return;
    };
    let mut names: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            upper.starts_with("CLAUDE") || upper.starts_with("ANTHROPIC")
        })
        .collect();
    names.sort();
    names.dedup();
    let cwd = std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    let mut line = json!({"argv": args, "cwd": cwd, "env": names}).to_string();
    line.push('\n');
    // A failure to log must not change what the fake does: the test reads the log and fails
    // there with a clearer message.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// A copy of this program that sleeps, its pid written to `pid_file`, then sleeps as well, so a
/// test can end the parent's tree and look for the grandchild.
fn spawn_grandchild(pid_file: &str) -> ExitCode {
    let Ok(program) = std::env::current_exe() else {
        eprintln!("fake-claude: cannot find itself");
        return ExitCode::from(1);
    };
    let child = Command::new(program)
        .args(["--sleep", &MAX_SLEEP_SECS.to_string()])
        .spawn();
    match child {
        Ok(child) => {
            if std::fs::write(pid_file, child.id().to_string()).is_err() {
                eprintln!("fake-claude: cannot write {pid_file}");
                return ExitCode::from(1);
            }
            nap(MAX_SLEEP_SECS as f64);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fake-claude: cannot start the grandchild: {error}");
            ExitCode::from(1)
        }
    }
}

/// The probe's conversation: stream-json requests on stdin, control responses on stdout.
fn answer_probe() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(Value::Object(request)) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if request.get("type").and_then(Value::as_str) != Some("control_request") {
            continue;
        }
        let Some(id) = request.get("request_id").and_then(Value::as_str) else {
            continue;
        };
        let subtype = request
            .get("request")
            .and_then(|r| r.get("subtype"))
            .and_then(Value::as_str);
        let body = match subtype {
            Some("initialize") => success(id, json!({})),
            Some("get_usage") => match usage_fixture() {
                Some(usage) => success(id, usage),
                None => error(id, "fake-claude: no usage fixture"),
            },
            _ => continue,
        };
        if writeln!(stdout, "{body}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break;
        }
    }
}

fn usage_fixture() -> Option<Value> {
    let path = std::env::var_os("FAKE_CLAUDE_USAGE")?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()
        .filter(Value::is_object)
}

fn success(id: &str, response: Value) -> Value {
    json!({"type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": response}})
}

fn error(id: &str, message: &str) -> Value {
    json!({"type": "control_response",
        "response": {"subtype": "error", "request_id": id, "error": message}})
}
