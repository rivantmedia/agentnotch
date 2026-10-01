//! Asks Claude Code itself for an account's plan usage, so the app never
//! touches a login: Claude Code reads its own, refreshes it under its own
//! lock, and answers from its own 60-second cache (UsageProbe.swift, AU§10).
//!
//! ```text
//! CLAUDE_CONFIG_DIR=<raw, or unset for the default account> \
//! claude -p --input-format stream-json --output-format stream-json --verbose \
//!   --no-session-persistence --strict-mcp-config --settings {"disableAllHooks":true}
//! ```
//!
//! run from an empty working folder, with two control requests on stdin:
//! `initialize`, then (once initialized) `get_usage` with `skip_behaviors`.
//! The `get_usage` control response carries the usage body. No model request
//! is made and no transcript is written; hooks are off, so the probe never
//! shows up as a session.
//!
//! The child is bounded by a timeout and always ended and reaped (its whole
//! tree, through the runner's Job object). Who the folder is signed in as is
//! read right before and after it runs: a folder that changed hands is not
//! probed, and an answer from one that changed hands meanwhile is reported
//! as such (`ProbeResult::folder_identity_after`) and thrown away.

use crate::core::claude_json::ClaudeJsonReader;
use crate::model::{AccountUsage, IdentityId, UsageSource};
use crate::platform::{Clock, CommandRunner, CommandSpec, Exit, RunningCommand};
use crate::runtime_types::{ProbeOutcome, ProbePlan, ProbeResult};
use crate::usage::parser::{
    self, seconds_between, CachedUsageSnapshot, GetUsageResult, ParsedUsage,
};
use crate::usage::planner::{folder_runs, has_store_marker, identity_of_login};
use crate::usage::schedule::{NOT_SIGNED_IN_TEXT, NO_RUN_FOLDER_TEXT, SEEDED_FRESH_WINDOW};
use serde_json::Value;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const INITIALIZE_REQUEST_ID: &str = "agentnotch-init";
pub const USAGE_REQUEST_ID: &str = "agentnotch-usage";

/// The probe's arguments, after the binary's own (`node`'s `cli.js`).
pub const ARGUMENTS: [&str; 10] = [
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
];

/// The two control requests: one JSON line each, keys sorted, as the Mac
/// sends them.
pub const INITIALIZE_REQUEST: &str = concat!(
    r#"{"request":{"subtype":"initialize"},"request_id":"agentnotch-init","type":"control_request"}"#,
    "\n"
);
pub const USAGE_REQUEST: &str = concat!(
    r#"{"request":{"skip_behaviors":true,"subtype":"get_usage"},"request_id":"agentnotch-usage","type":"control_request"}"#,
    "\n"
);

/// Shown when an npm shim refuses the probe's arguments (a batch file can't
/// take every argument safely, and the runner refuses rather than guess).
pub const SHIM_REFUSED_TEXT: &str =
    "Claude Code's npm shim can't take the probe's arguments; choose claude.exe in Settings";

/// Bytes of stdout buffered without seeing a newline before giving up.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// How much of stderr is kept for the failure text.
const STDERR_TAIL_BYTES: usize = 4096;

/// The probe's arguments for `prefix` (the binary's own leading arguments).
pub fn arguments(prefix: &[OsString]) -> Vec<OsString> {
    prefix
        .iter()
        .cloned()
        .chain(ARGUMENTS.iter().map(OsString::from))
        .collect()
}

/// What one probe found, before it is tied to an account.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Usage(ParsedUsage),
    /// Usage can't be had for this login; don't retry soon.
    Unavailable(String),
    /// Claude Code's usage request was rate limited; back off.
    RateLimited,
    /// Anything else: launch failure, timeout, error response, bad output.
    Failed(String),
}

/// What one stdout line means to the probe.
#[derive(Debug, Clone, PartialEq)]
pub enum LineEvent {
    /// The `initialize` request was answered (successfully or not).
    Initialized { error: Option<String> },
    /// The `get_usage` request was answered.
    Usage(Outcome),
    /// Anything else Claude Code prints (system/init messages, …).
    Other,
}

/// Classifies one line of stream-json output.
pub fn parse_line(line: &[u8]) -> LineEvent {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(line) else {
        return LineEvent::Other;
    };
    if object.get("type").and_then(Value::as_str) != Some("control_response") {
        return LineEvent::Other;
    }
    let Some(response) = object.get("response").and_then(Value::as_object) else {
        return LineEvent::Other;
    };
    let Some(request_id) = response.get("request_id").and_then(Value::as_str) else {
        return LineEvent::Other;
    };
    let succeeded = response.get("subtype").and_then(Value::as_str) == Some("success");
    let error = (!succeeded).then(|| {
        response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Request failed")
            .to_owned()
    });

    match request_id {
        INITIALIZE_REQUEST_ID => LineEvent::Initialized { error },
        USAGE_REQUEST_ID => {
            if let Some(error) = error {
                return LineEvent::Usage(outcome_for_error_message(&error));
            }
            let Some(body) = response.get("response").and_then(Value::as_object) else {
                return LineEvent::Usage(Outcome::Failed("Empty usage response".into()));
            };
            LineEvent::Usage(match parser::parse_get_usage_response(body) {
                GetUsageResult::Usage(usage) => Outcome::Usage(usage),
                GetUsageResult::Unavailable(reason) => Outcome::Unavailable(reason),
                GetUsageResult::RateLimited => Outcome::RateLimited,
                GetUsageResult::Malformed(reason) => Outcome::Failed(reason),
            })
        }
        _ => LineEvent::Other,
    }
}

/// Maps a control-response error string to an outcome. Not-logged-in and
/// API-key setups are permanent for this login, not worth retrying soon.
pub fn outcome_for_error_message(message: &str) -> Outcome {
    let lower = message.to_lowercase();
    if lower.contains("rate limit") || lower.contains("429") {
        return Outcome::RateLimited;
    }
    if lower.contains("not logged in")
        || lower.contains("please run /login")
        || lower.contains("claude.ai")
    {
        return Outcome::Unavailable(NOT_SIGNED_IN_TEXT.to_owned());
    }
    Outcome::Failed(message.chars().take(200).collect())
}

/// What a probe's usage answer means: the snapshot to keep (dated by when
/// Claude Code fetched it, never "now" for an answer that may be its
/// hour-old fallback; a fresh one was fetched after `launched_at`), and
/// whether the probe counts as rate limited for the backoff.
pub fn interpret_probe_answer(
    parsed: &ParsedUsage,
    account_id: &IdentityId,
    now: SystemTime,
    cached_copy: Option<&CachedUsageSnapshot>,
    launched_at: Option<SystemTime>,
) -> (Option<AccountUsage>, bool) {
    if !parsed.is_possibly_seeded {
        let mut snapshot = parsed.account_usage(account_id.clone(), UsageSource::Probe, now);
        snapshot.taken_after = launched_at.map(|launched| launched.min(now));
        return (Some(snapshot), false);
    }
    let Some(copy) = cached_copy.filter(|copy| copy.usage.has_same_windows(parsed)) else {
        // Can't be dated: the cache poll takes the file's own copy.
        return (None, true);
    };
    let dated = parsed.account_usage(account_id.clone(), UsageSource::Cache, copy.fetched_at);
    (Some(dated), is_stale_seed(copy.fetched_at, now))
}

/// A seeded answer dated longer ago than [`SEEDED_FRESH_WINDOW`] is Claude
/// Code's fallback after its own request failed: the probe counts as rate
/// limited.
pub fn is_stale_seed(fetched_at: SystemTime, now: SystemTime) -> bool {
    seconds_between(now, fetched_at) > SEEDED_FRESH_WINDOW.as_secs_f64()
}

/// The probe's time limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTiming {
    /// Upper bound for one probe, launch to answer.
    pub timeout: Duration,
    /// After the child exited, how long its last output may still arrive.
    pub exit_grace: Duration,
    /// After stdin is closed, how long the child may take to exit by itself
    /// before its tree is ended.
    pub close_grace: Duration,
    /// How long the ended tree is waited for.
    pub kill_wait: Duration,
}

impl Default for ProbeTiming {
    fn default() -> Self {
        ProbeTiming {
            timeout: Duration::from_secs(20),
            exit_grace: Duration::from_millis(300),
            close_grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(3),
        }
    }
}

/// What the stdout reader hands over.
enum Output {
    Line(Vec<u8>),
    /// Too much output without a newline.
    Overflow,
}

/// Reads `stdout` into lines (a trailing `\r` dropped) until it ends.
fn read_lines(mut stdout: Box<dyn Read + Send>, sender: mpsc::Sender<Output>) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = match stdout.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
        while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = buffer.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() && sender.send(Output::Line(line)).is_err() {
                return;
            }
        }
        if buffer.len() > MAX_LINE_BYTES {
            let _ = sender.send(Output::Overflow);
            return;
        }
    }
}

/// Keeps the last [`STDERR_TAIL_BYTES`] of `stderr`. `_done` is dropped when
/// stderr ends, which tells the probe the tail is complete.
fn read_tail(mut stderr: Box<dyn Read + Send>, tail: Arc<Mutex<Vec<u8>>>, _done: mpsc::Sender<()>) {
    let mut chunk = [0u8; 4096];
    while let Ok(read) = stderr.read(&mut chunk) {
        if read == 0 {
            break;
        }
        let mut tail = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tail.extend_from_slice(&chunk[..read]);
        if tail.len() > STDERR_TAIL_BYTES {
            let excess = tail.len() - STDERR_TAIL_BYTES;
            tail.drain(..excess);
        }
    }
}

/// A short reason for an early exit: the last non-empty stderr line (Claude
/// Code prints errors such as "Invalid API key" there), capped.
fn exit_description(exit: Exit, stderr: &[u8]) -> String {
    let status = match exit {
        Exit::Code(code) => code.to_string(),
        Exit::Killed => "stopped".to_owned(),
    };
    let text = String::from_utf8_lossy(stderr);
    let last_line = text
        .lines()
        .map(|line| line.trim_matches([' ', '\t']))
        .rfind(|line| !line.is_empty());
    match last_line {
        Some(line) => format!(
            "Claude Code exited ({status}): {}",
            line.chars().take(160).collect::<String>()
        ),
        None => format!("Claude Code exited with status {status}"),
    }
}

/// Why the child couldn't be started.
fn start_failure(error: &io::Error, shim: bool) -> String {
    // The runner refuses a batch shim's arguments rather than pass them to
    // `cmd.exe` unsafely.
    if shim && error.kind() == io::ErrorKind::InvalidInput {
        return SHIM_REFUSED_TEXT.to_owned();
    }
    format!("Couldn't start Claude Code: {error}")
}

/// Runs one probe child and talks to it. Never fails: every failure becomes
/// an outcome. The child is gone (or its tree ended) when this returns.
pub fn converse(
    spec: CommandSpec,
    shim: bool,
    runner: &dyn CommandRunner,
    timing: &ProbeTiming,
) -> Outcome {
    // Claude Code runs in an empty folder of the app's own.
    let _ = std::fs::create_dir_all(&spec.cwd);
    let mut child = match runner.spawn(spec) {
        Ok(child) => child,
        Err(error) => return Outcome::Failed(start_failure(&error, shim)),
    };

    let mut stdin = child.take_stdin();
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = child.take_stdout() {
        std::thread::spawn(move || read_lines(stdout, sender));
    } else {
        drop(sender);
    }
    let stderr_tail = Arc::new(Mutex::new(Vec::new()));
    let (stderr_done, stderr_ended) = mpsc::channel();
    if let Some(stderr) = child.take_stderr() {
        let tail = stderr_tail.clone();
        std::thread::spawn(move || read_tail(stderr, tail, stderr_done));
    } else {
        drop(stderr_done);
    }
    let stderr = StderrTail {
        tail: stderr_tail,
        ended: stderr_ended,
    };

    let outcome = exchange(&mut *child, &mut stdin, &receiver, &stderr, timing);

    // Closing stdin lets Claude Code exit on its own; its tree is ended if
    // it lingers.
    drop(stdin);
    if !matches!(child.wait_timeout(timing.close_grace), Ok(Some(_))) {
        child.kill_tree();
        let _ = child.wait_timeout(timing.kill_wait);
    }
    outcome
}

/// The child's stderr as far as it was read, and word of its end.
struct StderrTail {
    tail: Arc<Mutex<Vec<u8>>>,
    /// Disconnected once the reader has seen the end of stderr.
    ended: mpsc::Receiver<()>,
}

impl StderrTail {
    /// The tail once stderr has ended, or as it stands after `grace` (a
    /// grandchild may hold the pipe open): the child's last words are
    /// written just before it exits and may still be on their way.
    fn after_exit(&self, grace: Duration) -> Vec<u8> {
        let _ = self.ended.recv_timeout(grace);
        self.tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// The two requests and their answers, within `timing.timeout`.
fn exchange(
    child: &mut dyn RunningCommand,
    stdin: &mut Option<Box<dyn Write + Send>>,
    receiver: &mpsc::Receiver<Output>,
    stderr: &StderrTail,
    timing: &ProbeTiming,
) -> Outcome {
    // A child that already exited or closed its input reports that through
    // its exit (or the timeout): a failed write is not the news.
    let mut send = |request: &str| {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin
                .write_all(request.as_bytes())
                .and_then(|()| stdin.flush());
        }
    };
    send(INITIALIZE_REQUEST);

    let exited = |exit: Exit| {
        Outcome::Failed(exit_description(
            exit,
            &stderr.after_exit(timing.exit_grace),
        ))
    };
    let started = Instant::now();
    let mut sent_usage_request = false;
    let mut output_open = true;
    // Once the child has exited, its last lines may still be on their way:
    // an answer written just before exiting must still count.
    let mut exit: Option<(Exit, Instant)> = None;
    loop {
        let remaining = timing.timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Outcome::Failed(format!(
                "Claude Code didn't answer within {}s",
                timing.timeout.as_secs()
            ));
        }
        let mut wait = remaining.min(Duration::from_millis(50));
        if let Some((status, since)) = exit {
            let grace = timing.exit_grace.saturating_sub(since.elapsed());
            if grace.is_zero() || !output_open {
                return exited(status);
            }
            wait = wait.min(grace);
        }
        if !output_open {
            // Nothing more to read: only its exit is left to wait for.
            if let Ok(Some(status)) = child.wait_timeout(wait) {
                return exited(status);
            }
            continue;
        }
        match receiver.recv_timeout(wait) {
            Ok(Output::Line(line)) => match parse_line(&line) {
                LineEvent::Initialized { error: Some(error) } => {
                    return outcome_for_error_message(&error);
                }
                LineEvent::Initialized { error: None } => {
                    if !sent_usage_request {
                        sent_usage_request = true;
                        send(USAGE_REQUEST);
                    }
                }
                LineEvent::Usage(outcome) => return outcome,
                LineEvent::Other => {}
            },
            Ok(Output::Overflow) => {
                return Outcome::Failed("Unexpected output from Claude Code".into());
            }
            Err(RecvTimeoutError::Disconnected) => output_open = false,
            Err(RecvTimeoutError::Timeout) => {
                if exit.is_none() {
                    if let Ok(Some(status)) = child.wait_timeout(Duration::ZERO) {
                        exit = Some((status, Instant::now()));
                    }
                }
            }
        }
    }
}

/// `Job::Probe`: one usage check, with Claude Code's real limits on time.
pub fn run_probe(plan: &ProbePlan, runner: &dyn CommandRunner, clock: &dyn Clock) -> ProbeResult {
    run_probe_with(
        plan,
        runner,
        clock,
        &ProbeTiming::default(),
        crate::usage::claude_json::reader(),
    )
}

/// [`run_probe`] with its time limits and `.claude.json` reader given
/// (tests shorten the one and isolate the other).
pub fn run_probe_with(
    plan: &ProbePlan,
    runner: &dyn CommandRunner,
    clock: &dyn Clock,
    timing: &ProbeTiming,
    reader: &ClaudeJsonReader,
) -> ProbeResult {
    let started = clock.now();
    let result = |outcome: ProbeOutcome, folder_identity_after: Option<IdentityId>| ProbeResult {
        plan: plan.clone(),
        outcome,
        started,
        finished: clock.now(),
        folder_identity_after,
    };
    let folder = plan.config_dir.display();

    // Right before it runs: the folder is still not a store (whatever the
    // last classification said), and still signed in as this account.
    if has_store_marker(&plan.config_dir) {
        return result(
            ProbeOutcome::Unavailable(NO_RUN_FOLDER_TEXT.to_owned()),
            Some(plan.identity.clone()),
        );
    }
    let before = reader
        .read(&plan.identity_file)
        .and_then(|config| config.identity);
    if !folder_runs(before.as_ref(), &plan.expected) {
        return result(
            ProbeOutcome::Failed(format!("{folder} is signed in as another account now")),
            identity_of_login(before.as_ref(), &plan.expected, &plan.identity),
        );
    }

    let launched_at = clock.now();
    let outcome = converse(plan.spec.clone(), plan.binary.shim, runner, timing);

    // A possibly seeded answer is dated from the copy Claude Code keeps in
    // `.claude.json`, read after the probe; and the folder must still be
    // this account's.
    let after = reader.read(&plan.identity_file);
    let login = after.as_ref().and_then(|config| config.identity.as_ref());
    if !folder_runs(login, &plan.expected) {
        return result(
            ProbeOutcome::Failed(format!("{folder} changed hands during the check")),
            identity_of_login(login, &plan.expected, &plan.identity),
        );
    }

    let outcome = match outcome {
        Outcome::Usage(parsed) => {
            let cached_copy = after
                .as_ref()
                .filter(|_| parsed.is_possibly_seeded)
                .and_then(|config| config.matching_cached_usage())
                .and_then(parser::cached_usage_from_raw);
            let answer = interpret_probe_answer(
                &parsed,
                &plan.identity,
                clock.now(),
                cached_copy.as_ref(),
                Some(launched_at),
            );
            match answer.0 {
                Some(snapshot) => ProbeOutcome::Reading(snapshot),
                None => ProbeOutcome::RateLimited { retry_after: None },
            }
        }
        Outcome::Unavailable(reason) if reason == NOT_SIGNED_IN_TEXT => ProbeOutcome::SignedOut,
        Outcome::Unavailable(reason) => ProbeOutcome::Unavailable(reason),
        Outcome::RateLimited => ProbeOutcome::RateLimited { retry_after: None },
        Outcome::Failed(reason) => ProbeOutcome::Failed(reason),
    };
    result(outcome, Some(plan.identity.clone()))
}
