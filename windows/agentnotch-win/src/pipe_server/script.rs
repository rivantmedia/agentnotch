//! What `pipe-test-server.exe` does, apart from owning a real pipe: its command line, the
//! scripted answers, the record it writes and its loop (test-only; the exe is never bundled).
//!
//! The test server is the real `PipeServer` feeding the real engine `HookIngress`, scripted from
//! the command line, so a test (the integrity-level tests, the hermetic Claude Code job) can run
//! the app's side of the hook pipe in a process of its own and read afterwards what arrived.
//! Everything here is plain code over a `HookTransport`, so it runs on every system against the
//! engine's in-memory transport; only the exe's `main` names the real server.
//!
//! The record holds states and identifiers: which event, which session, which pid and config
//! folder the hook forwarded, which tool. Never a prompt, a tool's input, a message or a
//! working folder.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agentnotch_engine::ingress::HookIngress;
use agentnotch_engine::model::{HeldPermission, HookEvent, StatusLineMessage};
use agentnotch_engine::platform::{HookTransport, TransportEvent};
use agentnotch_engine::runtime_types::{AnswerResult, IngressConfig, IngressOut, Release};
use agentnotch_proto::{ControlOp, ControlResponse, ControlStatus, PermissionResponse};
use crossbeam_channel::RecvTimeoutError;
use serde_json::{json, Value};

use super::client::is_pipe_path;
use super::PIPE_IN_USE;

/// Asked to stop: `control quit`, the stop file, or the time limit.
pub const EXIT_OK: u8 = 0;
/// The server could not start, lost its pipe, or could not write its record.
pub const EXIT_FAILED: u8 = 1;
/// The command line was wrong.
pub const EXIT_USAGE: u8 = 2;
/// Another program already has the pipe's name.
pub const EXIT_PIPE_IN_USE: u8 = 3;

/// How long the server runs when `--max-seconds` is not given: a forgotten one must not outlive
/// the job that started it.
pub const DEFAULT_MAX_SECONDS: u64 = 3600;
/// What `control status` reports as the version.
pub const STATUS_VERSION: &str = "pipe-test-server";

pub const USAGE: &str =
    "usage: pipe-test-server --pipe <\\\\.\\pipe\\name> [--record <file.jsonl>] [--ready <file>]
       [--answer <Tool>=<verb>]... [--default <verb>] [--stop-file <path>] [--max-seconds <n>]
       [--include-sdk]
verbs: allow | always | deny | deny:<reason> | ask | close | plan | keep-planning | answer:<label>";

/// How a held PermissionRequest is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    /// Allow.
    Allow,
    /// Allow, handing back the request's first permission suggestion ("always allow").
    Always,
    /// Deny, with the user's reason when there is one.
    Deny(Option<String>),
    /// The frame that says "no decision": the hook prints nothing.
    Ask,
    /// Close the connection without a frame: also "no decision".
    Close,
    /// Approve an ExitPlanMode.
    Plan,
    /// "Keep planning" on an ExitPlanMode.
    KeepPlanning,
    /// Answer every question of an AskUserQuestion with this label.
    Answer(String),
}

impl Verb {
    pub fn parse(text: &str) -> Result<Verb, String> {
        let verb = match text {
            "allow" => Verb::Allow,
            "always" => Verb::Always,
            "deny" => Verb::Deny(None),
            "ask" => Verb::Ask,
            "close" => Verb::Close,
            "plan" => Verb::Plan,
            "keep-planning" => Verb::KeepPlanning,
            _ => match text.split_once(':') {
                Some(("deny", reason)) if !reason.is_empty() => Verb::Deny(Some(reason.into())),
                Some(("answer", label)) if !label.is_empty() => Verb::Answer(label.into()),
                _ => return Err(format!("unknown answer: {text}")),
            },
        };
        Ok(verb)
    }

    /// The verb as it is written on the command line (and in the record).
    pub fn text(&self) -> String {
        match self {
            Verb::Allow => "allow".into(),
            Verb::Always => "always".into(),
            Verb::Deny(None) => "deny".into(),
            Verb::Deny(Some(reason)) => format!("deny:{reason}"),
            Verb::Ask => "ask".into(),
            Verb::Close => "close".into(),
            Verb::Plan => "plan".into(),
            Verb::KeepPlanning => "keep-planning".into(),
            Verb::Answer(label) => format!("answer:{label}"),
        }
    }

    /// The response frame for `request`, built by the constructors the app's own UI uses;
    /// `None` for `close` (no frame at all).
    ///
    /// `always` on a request without suggestions and `answer:` on one without questions have
    /// nothing to hand back: they are a plain allow.
    pub fn response(&self, request: &HookEvent) -> Option<PermissionResponse> {
        let response = match self {
            Verb::Allow => PermissionResponse::allow(),
            Verb::Always => match request
                .permission_suggestions
                .as_ref()
                .and_then(|list| list.first())
            {
                Some(suggestion) => PermissionResponse::always_allow(suggestion.clone()),
                None => PermissionResponse::allow(),
            },
            Verb::Deny(reason) => PermissionResponse::deny(reason.clone()),
            Verb::Ask => PermissionResponse::ask(),
            Verb::Close => return None,
            Verb::Plan => PermissionResponse::approve_plan(),
            Verb::KeepPlanning => PermissionResponse::keep_planning(),
            Verb::Answer(label) => {
                let questions = questions(request);
                if questions.is_empty() {
                    PermissionResponse::allow()
                } else {
                    PermissionResponse::answers(questions.into_iter().map(|q| (q, label.clone())))
                }
            }
        };
        Some(response)
    }
}

/// The question texts of an AskUserQuestion request, exactly as they were asked.
fn questions(request: &HookEvent) -> Vec<String> {
    request
        .tool_input
        .as_ref()
        .and_then(|input| input.get("questions"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("question").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// `\\.\pipe\<name>`. Required, so the server never sits on a real pipe by accident.
    pub pipe: String,
    pub record: Option<PathBuf>,
    pub ready: Option<PathBuf>,
    /// `--answer <Tool>=<verb>`, in the order given.
    pub answers: Vec<(String, Verb)>,
    /// The answer for a tool no `--answer` names.
    pub default: Verb,
    pub stop_file: Option<PathBuf>,
    pub max_seconds: u64,
    /// Also take the events of `sdk*` entrypoints (`claude -p`), which the app ignores: a job
    /// that can only run Claude Code that way still sees its hooks arrive.
    pub include_sdk: bool,
}

/// Reads the arguments after the program's name. Matched by hand: every flag but
/// `--include-sdk` takes one value, only `--answer` may repeat, and anything unknown is an error rather than ignored (a typo in a
/// test's script must fail the test, not change what it proves).
pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut pipe = None;
    let mut record = None;
    let mut ready = None;
    let mut answers = Vec::new();
    let mut default = None;
    let mut stop_file = None;
    let mut max_seconds = None;
    let mut include_sdk = None;

    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        if flag == "--include-sdk" {
            once(&mut include_sdk, flag, true)?;
            continue;
        }
        let known = matches!(
            flag.as_str(),
            "--pipe"
                | "--record"
                | "--ready"
                | "--answer"
                | "--default"
                | "--stop-file"
                | "--max-seconds"
        );
        if !known {
            return Err(format!("unknown argument: {flag}"));
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?
            .as_str();
        match flag.as_str() {
            "--pipe" => {
                if !is_pipe_path(value) {
                    return Err(format!("--pipe must be \\\\.\\pipe\\<name>, not {value}"));
                }
                once(&mut pipe, flag, value.to_owned())?;
            }
            "--record" => once(&mut record, flag, path(flag, value)?)?,
            "--ready" => once(&mut ready, flag, path(flag, value)?)?,
            "--stop-file" => once(&mut stop_file, flag, path(flag, value)?)?,
            "--answer" => {
                let (tool, verb) = value
                    .split_once('=')
                    .filter(|(tool, _)| !tool.is_empty())
                    .ok_or_else(|| format!("--answer must be <Tool>=<verb>, not {value}"))?;
                answers.push((tool.to_owned(), Verb::parse(verb)?));
            }
            "--default" => once(&mut default, flag, Verb::parse(value)?)?,
            _ => {
                let seconds = value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| *seconds > 0)
                    .ok_or_else(|| {
                        format!("--max-seconds must be a number above 0, not {value}")
                    })?;
                once(&mut max_seconds, flag, seconds)?;
            }
        }
    }

    Ok(Args {
        pipe: pipe.ok_or("--pipe is required")?,
        record,
        ready,
        answers,
        default: default.unwrap_or(Verb::Close),
        stop_file,
        max_seconds: max_seconds.unwrap_or(DEFAULT_MAX_SECONDS),
        include_sdk: include_sdk.unwrap_or(false),
    })
}

fn once<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} was given twice"));
    }
    *slot = Some(value);
    Ok(())
}

fn path(flag: &str, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err(format!("{flag} needs a path"));
    }
    Ok(PathBuf::from(value))
}

/// Which answer each held request gets.
///
/// A tool named by several `--answer`s gets them in turn, one per request, and the last one
/// from then on; a tool none names gets the default.
#[derive(Debug, Clone)]
pub struct Script {
    by_tool: BTreeMap<String, VecDeque<Verb>>,
    default: Verb,
}

impl Script {
    pub fn new(answers: &[(String, Verb)], default: Verb) -> Self {
        let mut by_tool: BTreeMap<String, VecDeque<Verb>> = BTreeMap::new();
        for (tool, verb) in answers {
            by_tool
                .entry(tool.clone())
                .or_default()
                .push_back(verb.clone());
        }
        Script { by_tool, default }
    }

    /// The answer for the next request of `tool`.
    pub fn next(&mut self, tool: Option<&str>) -> Verb {
        let Some(queue) = tool.and_then(|tool| self.by_tool.get_mut(tool)) else {
            return self.default.clone();
        };
        if queue.len() > 1 {
            queue.pop_front().unwrap_or_else(|| self.default.clone())
        } else {
            queue
                .front()
                .cloned()
                .unwrap_or_else(|| self.default.clone())
        }
    }
}

/// What `control status` answers: fixed, apart from the number of requests held right now.
pub fn control_status(held: usize) -> ControlStatus {
    ControlStatus {
        version: STATUS_VERSION.into(),
        sealed: false,
        elevated: false,
        accounts: 0,
        rings: 0,
        readings: 0,
        sessions: 0,
        held: u32::try_from(held).unwrap_or(u32::MAX),
        hook_consent: "unasked".into(),
        transport: "listening".into(),
        cloud: "signed_out".into(),
        sync: false,
    }
}

// ---- the record ----

fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// What the record keeps of an event: what the hook forwarded about where it ran, and the
/// event's identifiers. None of its text.
fn event_line(kind: &str, at_ms: u64, event: &HookEvent) -> Value {
    json!({
        "kind": kind,
        "at_ms": at_ms,
        "event": event.event,
        "session_id": event.session_id.as_str(),
        "pid": event.pid,
        "config_dir_env": event.config_dir_env,
        "entrypoint": event.entrypoint,
        "attended": event.attended,
        "agent_id": event.agent_id,
        "tool": event.tool,
        "tool_use_id": event.tool_use_id,
        "protocol": event.protocol,
        "hook_pid": event.hook_pid,
        "terminal": event.terminal.as_ref().map(|terminal| json!({
            "wt_session": terminal.wt_session,
            "term_program": terminal.term_program,
        })),
    })
}

/// An event that expects no answer.
pub fn hook_line(at_ms: u64, event: &HookEvent) -> Value {
    event_line("hook", at_ms, event)
}

/// A PermissionRequest, with the answer the script gave (`answer`, a verb as written on the
/// command line) and what became of it (`result`: `delivered`, `peer_gone`, `not_pending`, or
/// `closed` for the verb `close`).
pub fn held_line(at_ms: u64, held: &HeldPermission, answer: &Verb, result: &str) -> Value {
    let mut line = event_line("permission_held", at_ms, &held.event);
    if let Some(fields) = line.as_object_mut() {
        fields.insert("tool_use_id".into(), json!(held.tool_use_id));
        fields.insert(
            "synthetic_tool_use_id".into(),
            json!(held.has_synthetic_tool_use_id),
        );
        fields.insert("answer".into(), json!(answer.text()));
        fields.insert("result".into(), json!(result));
    }
    line
}

/// A status line message: who sent it and whether it carried rate limits, not the numbers.
pub fn status_line_line(at_ms: u64, message: &StatusLineMessage) -> Value {
    json!({
        "kind": "status_line",
        "at_ms": at_ms,
        "session_id": message.session_id.as_str(),
        "pid": message.pid,
        "config_dir_env": message.config_dir_env,
        "has_rate_limits": message.rate_limits.is_some(),
        "model_id": message.model_id,
        "claude_code_version": message.claude_code_version,
    })
}

/// A held request whose hook went away before it was answered.
pub fn failed_line(at_ms: u64, session_id: &str, tool_use_id: &str) -> Value {
    json!({
        "kind": "permission_failed",
        "at_ms": at_ms,
        "session_id": session_id,
        "tool_use_id": tool_use_id,
    })
}

/// A control request, and whether its answer was written.
pub fn control_line(at_ms: u64, op: ControlOp, replied: bool) -> Value {
    json!({
        "kind": "control",
        "at_ms": at_ms,
        "op": op,
        "replied": replied,
    })
}

/// The pipe's state: `listening` with the name, or `error` with the reason.
pub fn transport_line(at_ms: u64, status: &Result<String, String>) -> Value {
    match status {
        Ok(name) => json!({"kind": "transport", "at_ms": at_ms, "listening": name}),
        Err(why) => json!({"kind": "transport", "at_ms": at_ms, "error": why}),
    }
}

// ---- the server ----

/// What one transport event led to.
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    /// The record's new lines, in order.
    pub lines: Vec<Value>,
    /// The pipe is listening: time to write the ready file.
    pub listening: bool,
    /// The server must end with this exit code.
    pub exit: Option<u8>,
}

/// The engine's ingress with the script answering in place of the hub and the user.
pub struct Scripted {
    ingress: HookIngress,
    script: Script,
}

impl Scripted {
    pub fn new(args: &Args, transport: Arc<dyn HookTransport>) -> Self {
        let mut config = IngressConfig::new(args.pipe.clone());
        config.ignore_sdk_entrypoints = !args.include_sdk;
        Scripted {
            ingress: HookIngress::with_transport(config, transport),
            script: Script::new(&args.answers, args.default.clone()),
        }
    }

    pub fn start(&self, sink: crossbeam_channel::Sender<TransportEvent>) -> Result<(), String> {
        self.ingress.start(sink)
    }

    /// Releases whatever is held and stops the transport.
    pub fn stop(&mut self) {
        self.ingress.stop();
    }

    /// One transport event: every request it holds is answered at once from the script.
    pub fn on_transport(&mut self, event: TransportEvent, now: SystemTime) -> Step {
        let at_ms = unix_ms(now);
        let mut step = Step::default();
        for out in self.ingress.on_transport(event, now) {
            match out {
                IngressOut::TransportStatus(status) => {
                    step.lines.push(transport_line(at_ms, &status));
                    match status {
                        Ok(_) => step.listening = true,
                        // The app would show a banner and try again; a test wants to know now.
                        Err(why) if why == PIPE_IN_USE => step.exit = Some(EXIT_PIPE_IN_USE),
                        Err(_) => step.exit = Some(EXIT_FAILED),
                    }
                }
                IngressOut::Hook(event) => step.lines.push(hook_line(at_ms, &event)),
                IngressOut::StatusLine(message) => {
                    step.lines.push(status_line_line(at_ms, &message));
                }
                IngressOut::PermissionHeld(held) => {
                    let verb = self.script.next(held.event.tool.as_deref());
                    let result = self.answer(&held, &verb);
                    step.lines.push(held_line(at_ms, &held, &verb, result));
                }
                IngressOut::PermissionFailed {
                    session,
                    tool_use_id,
                } => step
                    .lines
                    .push(failed_line(at_ms, session.as_str(), &tool_use_id)),
                IngressOut::Control { conn, op } => {
                    let response = match op {
                        ControlOp::Status => {
                            ControlResponse::status(control_status(self.ingress.held_count()))
                        }
                        ControlOp::Quit => ControlResponse::ok(),
                    };
                    let replied = self.ingress.reply_control(conn, &response);
                    step.lines.push(control_line(at_ms, op, replied));
                    if op == ControlOp::Quit {
                        step.exit = Some(EXIT_OK);
                    }
                }
            }
        }
        step
    }

    fn answer(&mut self, held: &HeldPermission, verb: &Verb) -> &'static str {
        match verb.response(&held.event) {
            Some(response) => {
                match self
                    .ingress
                    .answer(&held.session_id, &held.tool_use_id, response)
                {
                    AnswerResult::Delivered => "delivered",
                    AnswerResult::NotPending => "not_pending",
                    AnswerResult::PeerGone => "peer_gone",
                }
            }
            None => {
                self.ingress.release(Release::Request {
                    session: held.session_id.clone(),
                    tool_use_id: held.tool_use_id.clone(),
                });
                "closed"
            }
        }
    }
}

/// The record file: one JSON object per line, appended and flushed line by line, so a reader
/// sees every event as soon as it was handled.
struct Recorder(Option<File>);

impl Recorder {
    fn open(path: Option<&Path>) -> io::Result<Recorder> {
        path.map(|path| OpenOptions::new().create(true).append(true).open(path))
            .transpose()
            .map(Recorder)
    }

    fn write(&mut self, line: &Value) -> io::Result<()> {
        let Some(file) = &mut self.0 else {
            return Ok(());
        };
        let mut text = line.to_string();
        text.push('\n');
        file.write_all(text.as_bytes())?;
        file.flush()
    }
}

/// Writes the ready file (the pipe's name) under another name first, so whoever waits for it
/// never reads half of it.
fn write_ready(path: &Path, pipe_name: &str) -> io::Result<()> {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".tmp");
    let staging = PathBuf::from(staging);
    fs::write(&staging, format!("{pipe_name}\n"))?;
    fs::rename(&staging, path)
}

/// How often the loop looks at the stop file and the clock while nothing arrives.
const POLL: Duration = Duration::from_millis(100);

/// Runs the test server on `transport` until it is asked to stop; the exit code.
///
/// `complain` gets one line for whatever went wrong (the exe writes it to stderr).
pub fn serve(args: &Args, transport: Arc<dyn HookTransport>, complain: &mut dyn FnMut(&str)) -> u8 {
    let mut recorder = match Recorder::open(args.record.as_deref()) {
        Ok(recorder) => recorder,
        Err(error) => {
            complain(&format!("the record file can't be opened: {error}"));
            return EXIT_FAILED;
        }
    };
    let mut server = Scripted::new(args, transport);
    let (sink, events) = crossbeam_channel::unbounded();
    if let Err(why) = server.start(sink) {
        complain(&why);
        return EXIT_FAILED;
    }
    let deadline = Instant::now() + Duration::from_secs(args.max_seconds);
    let mut ready_written = false;
    let code = loop {
        if Instant::now() >= deadline {
            break EXIT_OK;
        }
        if args.stop_file.as_deref().is_some_and(Path::exists) {
            break EXIT_OK;
        }
        let event = match events.recv_timeout(POLL) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                complain("the pipe server ended by itself");
                break EXIT_FAILED;
            }
        };
        let step = server.on_transport(event, SystemTime::now());
        if let Some(error) = step
            .lines
            .iter()
            .find_map(|line| recorder.write(line).err())
        {
            complain(&format!("the record file can't be written: {error}"));
            break EXIT_FAILED;
        }
        if step.listening && !ready_written {
            ready_written = true;
            if let Some(ready) = &args.ready {
                if let Err(error) = write_ready(ready, &args.pipe) {
                    complain(&format!("the ready file can't be written: {error}"));
                    break EXIT_FAILED;
                }
            }
        }
        if let Some(code) = step.exit {
            if code == EXIT_PIPE_IN_USE {
                complain(PIPE_IN_USE);
            }
            break code;
        }
    };
    server.stop();
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentnotch_engine::ingress::{decode, Decoded};
    use agentnotch_engine::testkit::transport::MemoryTransport;
    use agentnotch_proto::permission_output_for_frames;

    const PIPE: &str = r"\\.\pipe\agentnotch-test-script";
    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentnotch-proto/tests/fixtures"
    );

    fn args(list: &[&str]) -> Result<Args, String> {
        let list: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
        parse_args(&list)
    }

    fn fixture(path: &str) -> Vec<u8> {
        let path = format!("{FIXTURES}/{path}");
        fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    fn fixture_json(path: &str) -> Value {
        serde_json::from_slice(&fixture(path)).unwrap()
    }

    /// The v1 frame `name` as the engine decodes it.
    fn request(name: &str) -> HookEvent {
        match decode(&fixture(&format!("v1/{name}.json")), SystemTime::now()) {
            Decoded::Hook(event) => *event,
            other => panic!("{name} is not a hook event: {other:?}"),
        }
    }

    // ---- argv ----

    #[test]
    fn the_pipe_is_required() {
        assert_eq!(args(&[]), Err("--pipe is required".into()));
        assert_eq!(
            args(&["--record", "frames.jsonl", "--default", "allow"]),
            Err("--pipe is required".into())
        );
    }

    #[test]
    fn the_pipe_alone_is_enough() {
        assert_eq!(
            args(&["--pipe", PIPE]),
            Ok(Args {
                pipe: PIPE.into(),
                record: None,
                ready: None,
                answers: Vec::new(),
                // No script: no decision, so Claude Code's own prompt decides.
                default: Verb::Close,
                stop_file: None,
                max_seconds: 3600,
                include_sdk: false,
            })
        );
    }

    #[test]
    fn every_flag_is_read() {
        let parsed = args(&[
            "--record",
            "frames.jsonl",
            "--answer",
            "Bash=allow",
            "--pipe",
            PIPE,
            "--ready",
            "ready.txt",
            "--default",
            "deny:Not in this test",
            "--stop-file",
            "stop",
            "--max-seconds",
            "90",
            "--include-sdk",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Args {
                pipe: PIPE.into(),
                record: Some("frames.jsonl".into()),
                ready: Some("ready.txt".into()),
                answers: vec![("Bash".into(), Verb::Allow)],
                default: Verb::Deny(Some("Not in this test".into())),
                stop_file: Some("stop".into()),
                max_seconds: 90,
                include_sdk: true,
            }
        );
    }

    #[test]
    fn answers_repeat_and_keep_their_order() {
        let parsed = args(&[
            "--pipe",
            PIPE,
            "--answer",
            "Bash=allow",
            "--answer",
            "AskUserQuestion=answer:Chart.js",
            "--answer",
            "Bash=deny",
            "--answer",
            "ExitPlanMode=plan",
            // The tool's name ends at the first `=`; the rest is the verb, whatever it holds.
            "--answer",
            "mcp__db__query=deny:a=b: no",
        ])
        .unwrap();
        assert_eq!(
            parsed.answers,
            vec![
                ("Bash".into(), Verb::Allow),
                ("AskUserQuestion".into(), Verb::Answer("Chart.js".into())),
                ("Bash".into(), Verb::Deny(None)),
                ("ExitPlanMode".into(), Verb::Plan),
                ("mcp__db__query".into(), Verb::Deny(Some("a=b: no".into()))),
            ]
        );
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        for (list, why) in [
            (
                vec!["--pipe", PIPE, "--verbose"],
                "unknown argument: --verbose",
            ),
            (vec!["--pipe", PIPE, "extra"], "unknown argument: extra"),
            (
                vec!["--pipe", PIPE, "--answers", "Bash=allow"],
                "unknown argument: --answers",
            ),
            (vec!["-p", PIPE], "unknown argument: -p"),
            (vec!["--pipe"], "--pipe needs a value"),
            (vec!["--pipe", PIPE, "--record"], "--record needs a value"),
            (
                vec!["--pipe", PIPE, "--record", ""],
                "--record needs a path",
            ),
            (
                vec!["--pipe", PIPE, "--pipe", PIPE],
                "--pipe was given twice",
            ),
            (
                vec!["--pipe", PIPE, "--include-sdk", "--include-sdk"],
                "--include-sdk was given twice",
            ),
            (
                vec!["--pipe", PIPE, "--include-sdk", "yes"],
                "unknown argument: yes",
            ),
            (
                vec!["--pipe", PIPE, "--default", "ask", "--default", "ask"],
                "--default was given twice",
            ),
            (
                vec!["--pipe", PIPE, "--answer", "Bash"],
                "--answer must be <Tool>=<verb>, not Bash",
            ),
            (
                vec!["--pipe", PIPE, "--answer", "=allow"],
                "--answer must be <Tool>=<verb>, not =allow",
            ),
            (
                vec!["--pipe", PIPE, "--answer", "Bash=yes"],
                "unknown answer: yes",
            ),
            (
                vec!["--pipe", PIPE, "--default", "deny:"],
                "unknown answer: deny:",
            ),
            (
                vec!["--pipe", PIPE, "--default", "answer:"],
                "unknown answer: answer:",
            ),
            (
                vec!["--pipe", PIPE, "--max-seconds", "0"],
                "--max-seconds must be a number above 0, not 0",
            ),
            (
                vec!["--pipe", PIPE, "--max-seconds", "soon"],
                "--max-seconds must be a number above 0, not soon",
            ),
        ] {
            assert_eq!(args(&list), Err(why.to_owned()), "{list:?}");
        }
    }

    #[test]
    fn only_a_pipe_name_is_served() {
        for name in [
            "agentnotch-test",
            r"C:\pipe\name",
            r"\\.\pipe\",
            r"\\.\pipe\a\b",
            "",
        ] {
            assert_eq!(
                args(&["--pipe", name]),
                Err(format!(r"--pipe must be \\.\pipe\<name>, not {name}")),
            );
        }
    }

    #[test]
    fn every_verb_is_written_the_way_it_is_read() {
        for text in [
            "allow",
            "always",
            "deny",
            "deny:Not on main",
            "ask",
            "close",
            "plan",
            "keep-planning",
            "answer:Recharts",
            "answer:a: b",
        ] {
            assert_eq!(Verb::parse(text).unwrap().text(), text);
        }
        for text in ["", "Allow", "allow:x", "plan:x", "approve", "deny :x"] {
            assert_eq!(Verb::parse(text), Err(format!("unknown answer: {text}")));
        }
    }

    // ---- verb → response frame ----

    /// The frame `verb` gives for the request `stdin`, checked against the proto fixture
    /// `v1-responses/<name>`: the frame itself, and what the hook prints for it.
    fn assert_frame(verb: &str, stdin: &str, name: &str) {
        let response = Verb::parse(verb)
            .unwrap()
            .response(&request(stdin))
            .unwrap_or_else(|| panic!("{verb} gave no frame"));
        let frame = response.to_json();
        let expected = fixture_json(&format!("v1-responses/{name}.json"));
        assert_eq!(expected["stdin"], stdin, "{name}");
        assert_eq!(
            serde_json::from_slice::<Value>(&frame).unwrap(),
            expected["response"],
            "{verb}"
        );

        let printed =
            permission_output_for_frames(&fixture(&format!("stdin/{stdin}.json")), &frame);
        let stdout = String::from_utf8(fixture(&format!("v1-responses/{name}.stdout"))).unwrap();
        match printed {
            Some(printed) => assert_eq!(
                serde_json::from_str::<Value>(&printed).unwrap(),
                serde_json::from_str::<Value>(&stdout).unwrap(),
                "{verb}"
            ),
            None => assert_eq!(stdout.trim(), "", "{verb}"),
        }
    }

    #[test]
    fn each_verb_gives_the_frame_the_apps_ui_gives() {
        assert_frame("allow", "permission_request_bash", "allow");
        assert_frame("always", "permission_request_bash", "always");
        assert_frame("deny", "permission_request_bash", "deny");
        assert_frame("ask", "permission_request_bash", "ask");
        assert_frame("plan", "permission_request_plan", "plan");
        assert_frame("keep-planning", "permission_request_plan", "keep_planning");
        assert_frame("answer:Recharts", "permission_request_question", "question");
        // Whatever the label holds (the fixture's is not ASCII), it goes out as it was given.
        let unicode = fixture_json("v1-responses/unicode.json");
        let label = unicode["response"]["updated_input"]["answers"]
            .as_object()
            .and_then(|answers| answers.values().next())
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        assert!(!label.is_ascii());
        assert_frame(
            &format!("answer:{label}"),
            "permission_request_question",
            "unicode",
        );
    }

    #[test]
    fn a_deny_with_a_reason_carries_it() {
        let frame = Verb::parse("deny:Not on main")
            .unwrap()
            .response(&request("permission_request_bash"))
            .unwrap()
            .to_json();
        // The fixture's deny also interrupts, which the script has no verb for.
        let mut expected = fixture_json("v1-responses/deny_reason.json")["response"].clone();
        expected.as_object_mut().unwrap().remove("interrupt");
        assert_eq!(serde_json::from_slice::<Value>(&frame).unwrap(), expected);
        assert_eq!(
            String::from_utf8(frame).unwrap(),
            r#"{"decision":"deny","reason":"Not on main"}"#
        );
    }

    #[test]
    fn close_gives_no_frame() {
        let verb = Verb::parse("close").unwrap();
        assert_eq!(verb.response(&request("permission_request_bash")), None);
    }

    #[test]
    fn nothing_to_hand_back_is_a_plain_allow() {
        // No suggestions on a question, no questions on a Bash call.
        let question = request("permission_request_question");
        let bash = request("permission_request_bash");
        assert_eq!(
            Verb::Always.response(&question),
            Some(PermissionResponse::allow())
        );
        assert_eq!(
            Verb::Answer("Recharts".into()).response(&bash),
            Some(PermissionResponse::allow())
        );
    }

    #[test]
    fn a_tools_answers_are_given_in_turn_and_the_last_one_stays() {
        let answers = args(&[
            "--pipe",
            PIPE,
            "--answer",
            "Bash=allow",
            "--answer",
            "Edit=plan",
            "--answer",
            "Bash=deny",
            "--default",
            "ask",
        ])
        .unwrap();
        let mut script = Script::new(&answers.answers, answers.default);
        assert_eq!(script.next(Some("Bash")), Verb::Allow);
        assert_eq!(script.next(Some("Edit")), Verb::Plan);
        assert_eq!(script.next(Some("Bash")), Verb::Deny(None));
        assert_eq!(script.next(Some("Bash")), Verb::Deny(None));
        assert_eq!(script.next(Some("Edit")), Verb::Plan);
        // Names are compared as they are.
        assert_eq!(script.next(Some("bash")), Verb::Ask);
        assert_eq!(script.next(Some("Write")), Verb::Ask);
        assert_eq!(script.next(None), Verb::Ask);
    }

    // ---- the record ----

    const SESSION: &str = "5d1e0a7b-3c21-4f7e-9a0b-1c2d3e4f5a6b";

    /// A scripted server on the in-memory transport, started, with its event channel.
    fn scripted(
        list: &[&str],
    ) -> (
        Scripted,
        Arc<MemoryTransport>,
        crossbeam_channel::Receiver<TransportEvent>,
    ) {
        let mut all = vec!["--pipe", PIPE];
        all.extend_from_slice(list);
        let transport = Arc::new(MemoryTransport::default());
        let server = Scripted::new(&args(&all).unwrap(), transport.clone());
        let (sink, events) = crossbeam_channel::unbounded();
        server.start(sink).unwrap();
        (server, transport, events)
    }

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    /// Feeds the v1 frame `name` to the server as a new connection.
    fn feed(server: &mut Scripted, transport: &MemoryTransport, name: &str) -> (u64, Step) {
        let (conn, event) = transport.frame(&fixture_json(&format!("v1/{name}.json")), at(1_000));
        (conn, server.on_transport(event, at(1_234)))
    }

    #[test]
    fn a_hook_event_is_one_line_of_states_and_identifiers() {
        let (mut server, transport, _events) = scripted(&[]);
        let (conn, step) = feed(&mut server, &transport, "session_start");
        assert_eq!(
            step,
            Step {
                lines: vec![json!({
                    "kind": "hook",
                    "at_ms": 1_234,
                    "event": "SessionStart",
                    "session_id": SESSION,
                    "pid": 4242,
                    "config_dir_env": r"C:\Users\me\.claude-work",
                    "entrypoint": "cli",
                    "attended": true,
                    "agent_id": null,
                    "tool": null,
                    "tool_use_id": null,
                    "protocol": 1,
                    "hook_pid": 6789,
                    "terminal": {
                        "wt_session": "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0",
                        "term_program": null,
                    },
                })],
                listening: false,
                exit: None,
            }
        );
        // Nothing to answer: closed at once.
        assert!(transport.is_closed(conn));
    }

    #[test]
    fn the_record_holds_no_text_of_the_session() {
        let (mut server, transport, _events) = scripted(&["--default", "allow"]);
        let mut text = String::new();
        for name in [
            "session_start",
            "user_prompt_submit",
            "pre_tool_use",
            "permission_request_bash",
            "permission_request_question",
            "permission_request_plan",
            "post_tool_use_failure",
            "notification_idle",
            "stop",
            "status_line",
            "session_end",
        ] {
            let (_, step) = feed(&mut server, &transport, name);
            assert_eq!(step.lines.len(), 1, "{name}");
            text.push_str(&step.lines[0].to_string());
            text.push('\n');
        }
        for secret in [
            "npm run test",           // a tool input
            "Run the auth suite",     // its description
            "Which charting library", // a question
            "Add models",             // a plan
            "login redirect",         // the session's title and prompt
            "code\\\\app",            // the working folder
            ".jsonl",                 // the transcript's path
            "used_percentage",        // the rate limits' numbers
        ] {
            assert!(!text.contains(secret), "{secret} is in the record:\n{text}");
        }
        // What the job does need is there.
        assert!(text.contains(r#""config_dir_env":"C:\\Users\\me\\.claude-work""#));
        assert!(text.contains(r#""pid":4242"#));
    }

    #[test]
    fn a_held_request_is_answered_from_the_script_and_recorded_with_its_answer() {
        let (mut server, transport, _events) = scripted(&["--answer", "Bash=allow"]);
        let (_, earlier) = feed(&mut server, &transport, "pre_tool_use");
        let tool_use_id = earlier.lines[0]["tool_use_id"].clone();
        assert!(tool_use_id.is_string());

        let (conn, step) = feed(&mut server, &transport, "permission_request_bash");
        assert_eq!(step.exit, None);
        assert_eq!(
            step.lines,
            vec![json!({
                "kind": "permission_held",
                "at_ms": 1_234,
                "event": "PermissionRequest",
                "session_id": SESSION,
                "pid": 4242,
                "config_dir_env": r"C:\Users\me\.claude-work",
                "entrypoint": "cli",
                "attended": true,
                "agent_id": null,
                "tool": "Bash",
                // The id of the PreToolUse before it: the request itself carries none.
                "tool_use_id": tool_use_id,
                "synthetic_tool_use_id": false,
                "protocol": 1,
                "hook_pid": 6789,
                "terminal": {
                    "wt_session": "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0",
                    "term_program": null,
                },
                "answer": "allow",
                "result": "delivered",
            })]
        );
        assert_eq!(
            transport.response(conn),
            Some(br#"{"decision":"allow"}"#.to_vec())
        );
    }

    #[test]
    fn every_scripted_answer_reaches_its_connection() {
        let (mut server, transport, _events) = scripted(&[
            "--answer",
            "AskUserQuestion=answer:Recharts",
            "--answer",
            "ExitPlanMode=keep-planning",
            "--default",
            "deny:Not in this test",
        ]);
        let (question, step) = feed(&mut server, &transport, "permission_request_question");
        assert_eq!(step.lines[0]["answer"], "answer:Recharts");
        assert_eq!(step.lines[0]["synthetic_tool_use_id"], true);
        assert_eq!(
            serde_json::from_slice::<Value>(&transport.response(question).unwrap()).unwrap(),
            fixture_json("v1-responses/question.json")["response"]
        );

        let (plan, step) = feed(&mut server, &transport, "permission_request_plan");
        assert_eq!(step.lines[0]["answer"], "keep-planning");
        assert_eq!(
            serde_json::from_slice::<Value>(&transport.response(plan).unwrap()).unwrap(),
            fixture_json("v1-responses/keep_planning.json")["response"]
        );

        let (bash, step) = feed(&mut server, &transport, "permission_request_bash");
        assert_eq!(step.lines[0]["answer"], "deny:Not in this test");
        assert_eq!(step.lines[0]["result"], "delivered");
        assert_eq!(
            transport.response(bash),
            Some(br#"{"decision":"deny","reason":"Not in this test"}"#.to_vec())
        );
    }

    /// The v1 frame `name` as `claude -p` would send it.
    fn from_sdk(transport: &MemoryTransport, name: &str) -> (u64, TransportEvent) {
        let mut frame = fixture_json(&format!("v1/{name}.json"));
        frame["entrypoint"] = json!("sdk-cli");
        transport.frame(&frame, at(1_000))
    }

    #[test]
    fn an_sdk_sessions_events_are_dropped_as_the_app_drops_them() {
        let (mut server, transport, _events) = scripted(&["--default", "allow"]);
        for name in ["session_start", "permission_request_bash"] {
            let (conn, event) = from_sdk(&transport, name);
            assert_eq!(server.on_transport(event, at(1_234)), Step::default());
            assert!(transport.is_closed(conn));
        }
    }

    #[test]
    fn include_sdk_takes_an_sdk_sessions_events() {
        let (mut server, transport, _events) = scripted(&["--default", "allow", "--include-sdk"]);
        let (_, event) = from_sdk(&transport, "session_start");
        let step = server.on_transport(event, at(1_234));
        assert_eq!(step.lines[0]["kind"], "hook");
        assert_eq!(step.lines[0]["entrypoint"], "sdk-cli");

        let (conn, event) = from_sdk(&transport, "permission_request_bash");
        let step = server.on_transport(event, at(1_234));
        assert_eq!(step.lines[0]["result"], "delivered");
        assert_eq!(
            transport.response(conn),
            Some(br#"{"decision":"allow"}"#.to_vec())
        );

        // An unattended session stays ignored: nothing may hold its request.
        let mut frame = fixture_json("v1/permission_request_bash.json");
        frame["attended"] = json!(false);
        let (conn, event) = transport.frame(&frame, at(1_000));
        assert_eq!(server.on_transport(event, at(1_234)), Step::default());
        assert!(transport.is_closed(conn));
    }

    #[test]
    fn close_releases_the_request_without_a_frame() {
        // `close` is also what a server without a script does.
        let (mut server, transport, _events) = scripted(&[]);
        let (conn, step) = feed(&mut server, &transport, "permission_request_bash");
        assert_eq!(step.lines[0]["kind"], "permission_held");
        assert_eq!(step.lines[0]["answer"], "close");
        assert_eq!(step.lines[0]["result"], "closed");
        assert!(transport.is_closed(conn));
        assert_eq!(transport.response(conn), None);
    }

    #[test]
    fn an_answer_to_a_hook_that_went_away_says_so() {
        let (mut server, transport, _events) = scripted(&["--default", "allow"]);
        let (conn, event) =
            transport.frame(&fixture_json("v1/permission_request_bash.json"), at(1));
        transport.mark_gone(conn);
        let step = server.on_transport(event, at(2));
        assert_eq!(step.lines[0]["result"], "peer_gone");
    }

    #[test]
    fn a_status_line_is_recorded_without_its_numbers() {
        let (mut server, transport, _events) = scripted(&[]);
        let (conn, step) = feed(&mut server, &transport, "status_line");
        assert_eq!(
            step.lines,
            vec![json!({
                "kind": "status_line",
                "at_ms": 1_234,
                "session_id": SESSION,
                "pid": 4242,
                "config_dir_env": r"C:\Users\me\.claude-work",
                "has_rate_limits": true,
                "model_id": "claude-opus-4-5",
                "claude_code_version": "2.1.282",
            })]
        );
        assert!(transport.is_closed(conn));
    }

    #[test]
    fn control_status_is_answered_and_the_server_goes_on() {
        let (mut server, transport, _events) = scripted(&[]);
        let (conn, step) = feed(&mut server, &transport, "control_status");
        assert_eq!(
            step,
            Step {
                lines: vec![json!({
                    "kind": "control",
                    "at_ms": 1_234,
                    "op": "status",
                    "replied": true,
                })],
                listening: false,
                exit: None,
            }
        );
        let answer: ControlResponse =
            serde_json::from_slice(&transport.response(conn).unwrap()).unwrap();
        assert_eq!(answer, ControlResponse::status(control_status(0)));
        let status = answer.status.unwrap();
        assert_eq!(status.version, "pipe-test-server");
        assert_eq!(status.transport, "listening");
        assert_eq!(status.hook_consent, "unasked");
        assert_eq!(status.cloud, "signed_out");
    }

    #[test]
    fn control_quit_is_answered_ok_and_ends_the_server_with_0() {
        let (mut server, transport, _events) = scripted(&[]);
        let (conn, step) = feed(&mut server, &transport, "control_quit");
        assert_eq!(
            step.lines,
            vec![json!({"kind": "control", "at_ms": 1_234, "op": "quit", "replied": true})]
        );
        assert_eq!(step.exit, Some(EXIT_OK));
        assert_eq!(transport.response(conn), Some(br#"{"ok":true}"#.to_vec()));
    }

    #[test]
    fn the_transports_state_is_recorded_and_a_taken_name_ends_with_3() {
        let (mut server, transport, events) = scripted(&[]);
        // The in-memory transport reports listening as the real one does.
        let listening = server.on_transport(events.try_recv().unwrap(), at(5));
        assert_eq!(
            listening,
            Step {
                lines: vec![json!({"kind": "transport", "at_ms": 5, "listening": PIPE})],
                listening: true,
                exit: None,
            }
        );
        assert_eq!(transport.pipe_name().as_deref(), Some(PIPE));

        let taken = server.on_transport(TransportEvent::Error(PIPE_IN_USE.into()), at(6));
        assert_eq!(
            taken.lines,
            vec![json!({
                "kind": "transport",
                "at_ms": 6,
                "error": "The hook pipe is in use by another program",
            })]
        );
        assert_eq!(taken.exit, Some(EXIT_PIPE_IN_USE));
        assert_eq!(EXIT_PIPE_IN_USE, 3);

        let broken = server.on_transport(TransportEvent::Error("anything else".into()), at(7));
        assert_eq!(broken.exit, Some(EXIT_FAILED));
    }

    #[test]
    fn a_vanished_hook_of_a_held_request_is_recorded() {
        assert_eq!(
            failed_line(9, SESSION, "toolu_01"),
            json!({
                "kind": "permission_failed",
                "at_ms": 9,
                "session_id": SESSION,
                "tool_use_id": "toolu_01",
            })
        );
    }

    // ---- the loop ----

    /// Waits for `path` to exist; the loop under test runs on another thread.
    fn wait_for(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn record_lines(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_server_writes_ready_records_each_event_and_quits_with_0() {
        let folder = tempfile::tempdir().unwrap();
        let record = folder.path().join("frames.jsonl");
        let ready = folder.path().join("ready");
        let parsed = args(&[
            "--pipe",
            PIPE,
            "--record",
            record.to_str().unwrap(),
            "--ready",
            ready.to_str().unwrap(),
            "--answer",
            "Bash=allow",
        ])
        .unwrap();
        let transport = Arc::new(MemoryTransport::default());
        let served = {
            let transport = transport.clone();
            std::thread::spawn(move || {
                let mut complaints = Vec::new();
                let code = serve(&parsed, transport, &mut |why| {
                    complaints.push(why.to_owned())
                });
                (code, complaints)
            })
        };

        wait_for(&ready);
        assert_eq!(fs::read_to_string(&ready).unwrap(), format!("{PIPE}\n"));
        transport.inject(fixture("v1/session_start.json"), at(1));
        let held = transport.inject(fixture("v1/permission_request_bash.json"), at(2));
        let quit = transport.inject(fixture("v1/control_quit.json"), at(3));

        let (code, complaints) = served.join().unwrap();
        assert_eq!((code, complaints), (EXIT_OK, Vec::new()));
        assert_eq!(
            transport.response(held),
            Some(br#"{"decision":"allow"}"#.to_vec())
        );
        assert_eq!(transport.response(quit), Some(br#"{"ok":true}"#.to_vec()));
        assert!(transport.is_stopped());

        let kinds: Vec<String> = record_lines(&record)
            .iter()
            .map(|line| line["kind"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(kinds, ["transport", "hook", "permission_held", "control"]);
    }

    #[test]
    fn the_stop_file_ends_the_server_with_0() {
        let folder = tempfile::tempdir().unwrap();
        let ready = folder.path().join("ready");
        let stop = folder.path().join("stop");
        let parsed = args(&[
            "--pipe",
            PIPE,
            "--ready",
            ready.to_str().unwrap(),
            "--stop-file",
            stop.to_str().unwrap(),
        ])
        .unwrap();
        let transport = Arc::new(MemoryTransport::default());
        let served = {
            let transport = transport.clone();
            std::thread::spawn(move || serve(&parsed, transport, &mut |_| {}))
        };
        wait_for(&ready);
        fs::write(&stop, "").unwrap();
        assert_eq!(served.join().unwrap(), EXIT_OK);
        assert!(transport.is_stopped());
    }

    #[test]
    fn the_time_limit_ends_the_server_with_0() {
        let parsed = args(&["--pipe", PIPE, "--max-seconds", "1"]).unwrap();
        let started = Instant::now();
        let code = serve(&parsed, Arc::new(MemoryTransport::default()), &mut |_| {});
        assert_eq!(code, EXIT_OK);
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[test]
    fn a_record_file_that_cant_be_opened_ends_the_server_with_1() {
        let folder = tempfile::tempdir().unwrap();
        // A folder is not a file to append to.
        let parsed = args(&["--pipe", PIPE, "--record", folder.path().to_str().unwrap()]).unwrap();
        let transport = Arc::new(MemoryTransport::default());
        let mut complaints = Vec::new();
        let code = serve(&parsed, transport.clone(), &mut |why| {
            complaints.push(why.to_owned());
        });
        assert_eq!(code, EXIT_FAILED);
        assert_eq!(complaints.len(), 1);
        assert!(complaints[0].starts_with("the record file can't be opened: "));
        // The pipe was never claimed.
        assert_eq!(transport.pipe_name(), None);
    }
}
