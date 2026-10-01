//! Helpers the sessions integration tests share: transcript line builders
//! (A1_TestSupport.swift's `TranscriptLines`), writing and appending
//! transcripts, folding decoded entries the way the store will, and fake
//! `SecureFiles` (links and file identities a Windows runner can't create).
#![allow(dead_code)]

use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::time::iso8601;
use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use agentnotch_engine::runtime_types::TranscriptEntry;
use agentnotch_engine::sessions::summary::{ConversationInfo, TranscriptSummary};
use agentnotch_engine::sessions::transcript::decode_line;
use serde_json::{json, Map, Value};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 2027-01-15, a fixed instant for lines that need a time.
pub fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn next_uuid() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("uuid-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn merged(mut line: Value, extra: Value) -> Value {
    if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            line.insert(key.clone(), value.clone());
        }
    }
    line
}

pub fn user(text: &str, at: SystemTime, extra: Value) -> Value {
    merged(
        json!({
            "type": "user", "uuid": next_uuid(), "timestamp": iso8601(at),
            "message": {"role": "user", "content": text},
        }),
        extra,
    )
}

pub fn assistant_text(text: &str, at: SystemTime) -> Value {
    json!({
        "type": "assistant", "uuid": next_uuid(), "timestamp": iso8601(at),
        "message": {"id": next_uuid(), "role": "assistant", "content": [{"type": "text", "text": text}]},
    })
}

/// An assistant line with usage (TranscriptTests' `assistant(...)`).
#[allow(clippy::too_many_arguments)]
pub fn assistant_usage(
    id: Option<&str>,
    model: &str,
    sidechain: bool,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    text: &str,
) -> Value {
    let mut message = json!({
        "model": model,
        "content": [{"type": "text", "text": text}],
        "usage": {
            "input_tokens": input, "output_tokens": output,
            "cache_read_input_tokens": cache_read, "cache_creation_input_tokens": cache_creation,
        },
    });
    if let Some(id) = id {
        message["id"] = json!(id);
    }
    json!({"type": "assistant", "isSidechain": sidechain, "uuid": next_uuid(),
           "requestId": "req_1", "message": message})
}

pub fn tool_use(id: &str, name: &str, input: Value, at: SystemTime) -> Value {
    json!({
        "type": "assistant", "uuid": next_uuid(), "timestamp": iso8601(at),
        "message": {"id": format!("msg-{id}"), "role": "assistant",
                    "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]},
    })
}

pub fn tool_result(id: &str, text: &str, at: SystemTime, tool_use_result: Option<Value>) -> Value {
    let mut line = json!({
        "type": "user", "uuid": next_uuid(), "timestamp": iso8601(at),
        "message": {"role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]},
    });
    if let Some(result) = tool_use_result {
        line["toolUseResult"] = result;
    }
    line
}

pub fn line_bytes(line: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(line).unwrap();
    bytes.push(b'\n');
    bytes
}

/// Writes (replacing) a transcript of these lines.
pub fn write_lines(path: &Path, lines: &[Value]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let bytes: Vec<u8> = lines.iter().flat_map(line_bytes).collect();
    std::fs::write(path, bytes).unwrap();
}

pub fn append_bytes(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

pub fn append_lines(path: &Path, lines: &[Value]) {
    let bytes: Vec<u8> = lines.iter().flat_map(line_bytes).collect();
    append_bytes(path, &bytes);
}

/// Decodes lines and folds them into a summary, as the store does.
pub fn summarize(lines: &[Value]) -> (ConversationInfo, TranscriptSummary) {
    let mut summary = TranscriptSummary::new();
    for line in lines {
        for entry in decode_line(line.to_string().as_bytes()) {
            summary.apply(&entry);
        }
    }
    (summary.info(), summary)
}

pub fn entries_of(lines: &[Value]) -> Vec<TranscriptEntry> {
    lines
        .iter()
        .flat_map(|line| decode_line(line.to_string().as_bytes()))
        .collect()
}

/// `StdSecureFiles` where chosen folders are links to others: paths under a
/// link resolve under its target for `identity` and `canonical`, as a real
/// symbolic link or junction would.
pub struct LinkedFiles {
    pub links: Vec<(PathBuf, PathBuf)>,
}

impl LinkedFiles {
    fn through(&self, path: &Path) -> PathBuf {
        for (link, target) in &self.links {
            if let Ok(rest) = path.strip_prefix(link) {
                return target.join(rest);
            }
        }
        path.to_path_buf()
    }
}

impl SecureFiles for LinkedFiles {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        StdSecureFiles.ensure_private_dir(&self.through(dir))
    }
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        StdSecureFiles.write_atomic(&self.through(path), bytes, mode, expect)
    }
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        StdSecureFiles.create_exclusive(&self.through(path), bytes)
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        StdSecureFiles.identity(&self.through(path))
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        if self.links.iter().any(|(link, _)| link == path) {
            return Ok(true);
        }
        StdSecureFiles.is_reparse(&self.through(path))
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        StdSecureFiles.canonical(&self.through(path))
    }
    fn is_private(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_private(&self.through(path))
    }
}

/// `StdSecureFiles` whose identity answers are chosen by the test (a Windows
/// runner reports volume 0 and index 0 for every file, production reports
/// real ones).
pub struct FixedIdentity(pub std::sync::Mutex<Option<(u64, u128)>>);

impl FixedIdentity {
    pub fn new(volume: u64, index: u128) -> Self {
        FixedIdentity(std::sync::Mutex::new(Some((volume, index))))
    }
    pub fn set(&self, volume: u64, index: u128) {
        *self.0.lock().unwrap() = Some((volume, index));
    }
}

impl SecureFiles for FixedIdentity {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        StdSecureFiles.ensure_private_dir(dir)
    }
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        StdSecureFiles.write_atomic(path, bytes, mode, expect)
    }
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        StdSecureFiles.create_exclusive(path, bytes)
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        let mut identity = StdSecureFiles.identity(path)?;
        if let Some((volume, index)) = *self.0.lock().unwrap() {
            identity.volume = volume;
            identity.index = index;
        }
        Ok(identity)
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_reparse(path)
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        StdSecureFiles.canonical(path)
    }
    fn is_private(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_private(path)
    }
}

/// A JSON object from key/value pairs, for `extra` arguments.
pub fn object(pairs: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_owned(), value.clone());
    }
    Value::Object(map)
}

// ---- the store harness (SessionStoreFlowTests' HookEventBuilder and the
// private SessionStore the Mac tests drive) ----

use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::model::{
    AccountSighting, Answer, Attribution, HookEvent, SessionId, SessionState, SessionView,
    StatusLineMessage,
};
use agentnotch_engine::runtime_types::{IngestContext, Release, SessionEffects, SessionInput};
use agentnotch_engine::sessions::background::WaitTiming;
use agentnotch_engine::sessions::completion::CompletionTiming;
use agentnotch_engine::sessions::session::Session;
use agentnotch_engine::sessions::SessionStore;

/// The transcript path every harness event carries (POSIX rules, so the
/// tests mean the same on every OS).
pub const TRANSCRIPT: &str = "/home/me/.claude/projects/-tmp-proj/s1.jsonl";

/// Builds HookEvents tersely for store tests.
#[derive(Clone)]
pub struct HookEventBuilder {
    pub event: String,
    pub status: String,
    pub transcript_path: Option<String>,
    pub session_id: String,
    pub cwd: String,
    pub entrypoint: String,
    pub attended: Option<bool>,
    pub agent_id: Option<String>,
    pub tool: Option<String>,
    pub tool_use_id: Option<String>,
    pub notification_type: Option<String>,
    pub message: Option<String>,
    pub title: Option<String>,
    pub last_assistant_message: Option<String>,
    pub background_task_count: Option<u32>,
    pub background_task_types: Option<Vec<String>>,
    pub session_cron_count: Option<u32>,
    pub stop_hook_active: Option<bool>,
    pub stop_error: Option<String>,
    pub source: Option<String>,
    pub session_title: Option<String>,
    pub trigger: Option<String>,
    pub prompt: Option<String>,
    pub config_dir_env: Option<String>,
    pub pid: Option<u32>,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub synthetic: bool,
    pub suggestions: Option<Vec<Value>>,
}

impl HookEventBuilder {
    pub fn new(event: &str, status: &str) -> Self {
        HookEventBuilder {
            event: event.into(),
            status: status.into(),
            transcript_path: Some(TRANSCRIPT.into()),
            session_id: "s1".into(),
            cwd: "/tmp/proj".into(),
            entrypoint: "cli".into(),
            attended: Some(true),
            agent_id: None,
            tool: None,
            tool_use_id: None,
            notification_type: None,
            message: None,
            title: None,
            last_assistant_message: None,
            background_task_count: None,
            background_task_types: None,
            session_cron_count: None,
            stop_hook_active: None,
            stop_error: None,
            source: None,
            session_title: None,
            trigger: None,
            prompt: None,
            config_dir_env: None,
            pid: None,
            permission_mode: None,
            model: None,
            synthetic: false,
            suggestions: None,
        }
    }

    pub fn build(&self, received_at: SystemTime) -> HookEvent {
        let mut event = HookEvent::new(self.session_id.as_str(), self.event.as_str(), received_at);
        event.status = self.status.clone();
        event.cwd = self.cwd.clone();
        event.transcript_path = self.transcript_path.clone();
        event.config_dir_env = self.config_dir_env.clone();
        event.attended = self.attended;
        event.entrypoint = Some(self.entrypoint.clone());
        event.pid = self.pid;
        event.permission_mode = self.permission_mode.clone();
        event.model = self.model.clone();
        event.agent_id = self.agent_id.clone();
        event.tool = self.tool.clone();
        event.tool_input = self.tool.as_ref().map(|_| Map::new());
        event.tool_use_id = self.tool_use_id.clone();
        event.has_synthetic_tool_use_id = self.synthetic;
        event.permission_suggestions = self.suggestions.clone();
        event.notification_type = self.notification_type.clone();
        event.message = self.message.clone();
        event.title = self.title.clone();
        event.last_assistant_message = self.last_assistant_message.clone();
        event.background_task_count = self
            .background_task_count
            .or(self.background_task_types.as_ref().map(|t| t.len() as u32));
        event.background_task_types = self.background_task_types.clone();
        event.session_cron_count = self.session_cron_count;
        event.stop_hook_active = self.stop_hook_active;
        event.stop_error = self.stop_error.clone();
        event.source = self.source.clone();
        event.session_title = self.session_title.clone();
        event.trigger = self.trigger.clone();
        event.prompt = self.prompt.clone();
        event
    }
}

/// A private SessionStore with a clock the test moves, the hub's context
/// for the frames, and every effect it returned.
pub struct Harness {
    pub store: SessionStore,
    pub now: SystemTime,
    pub ctx: IngestContext,
    /// Every release so far, in order (see `take_releases`).
    pub releases: Vec<Release>,
    pub sightings: Vec<AccountSighting>,
    pub last: SessionEffects,
    /// The clock moves this much after every input (1 ms: a prompt and the
    /// Stop that follows it never share an instant, as on the Mac); tests
    /// that read exact times set it to zero.
    pub step: Duration,
}

pub fn paths() -> Paths {
    Paths::new(PathStyle::Posix, "/home/me")
}

impl Harness {
    /// Stops complete at once (no registry to wait for), the Mac tests'
    /// `.immediate`.
    pub fn new() -> Self {
        Harness::with_timing(CompletionTiming::IMMEDIATE, WaitTiming::STANDARD)
    }

    pub fn with_timing(completion: CompletionTiming, wait: WaitTiming) -> Self {
        Harness {
            store: SessionStore::new()
                .with_paths(paths())
                .with_timing(completion, wait),
            now: t0(),
            ctx: IngestContext {
                attribution: Attribution::Known(None),
                account: None,
                trusted_pid: None,
                pid_started: None,
            },
            releases: Vec::new(),
            sightings: Vec::new(),
            last: SessionEffects::default(),
            step: Duration::from_millis(1),
        }
    }

    /// Moves the clock to `secs` after [`t0`].
    pub fn at(&mut self, secs: u64) -> &mut Self {
        self.now = t0() + Duration::from_secs(secs);
        self
    }

    pub fn advance(&mut self, secs: u64) -> &mut Self {
        self.now += Duration::from_secs(secs);
        self
    }

    pub fn apply(&mut self, input: SessionInput) -> SessionEffects {
        let effects = self.store.apply(input, self.now);
        self.releases.extend(effects.release.iter().cloned());
        self.sightings.extend(effects.sightings.iter().cloned());
        self.last = effects.clone();
        self.now += self.step;
        effects
    }

    pub fn send(&mut self, builder: &HookEventBuilder) -> SessionEffects {
        let event = builder.build(self.now);
        let ctx = self.ctx.clone();
        self.apply(SessionInput::Hook { event, ctx })
    }

    /// One hook event for session `s1` received now.
    pub fn hook(&mut self, name: &str, status: &str) -> SessionEffects {
        self.hook_with(name, status, |_| {})
    }

    pub fn hook_with(
        &mut self,
        name: &str,
        status: &str,
        configure: impl FnOnce(&mut HookEventBuilder),
    ) -> SessionEffects {
        let mut builder = HookEventBuilder::new(name, status);
        configure(&mut builder);
        self.send(&builder)
    }

    pub fn status_line(&mut self, message: StatusLineMessage) -> SessionEffects {
        let ctx = self.ctx.clone();
        self.apply(SessionInput::StatusLine { message, ctx })
    }

    pub fn answer(&mut self, tool_use_id: &str, answer: Answer) -> SessionEffects {
        self.apply(SessionInput::PermissionResolved {
            session: "s1".into(),
            tool_use_id: tool_use_id.into(),
            answer,
        })
    }

    pub fn approve(&mut self, tool_use_id: &str) -> SessionEffects {
        self.answer(tool_use_id, Answer::Allow { always: false })
    }

    pub fn deny(&mut self, tool_use_id: &str) -> SessionEffects {
        self.answer(tool_use_id, Answer::Deny { reason: None })
    }

    pub fn socket_failed(&mut self, tool_use_id: &str) -> SessionEffects {
        self.apply(SessionInput::PermissionFailed {
            session: "s1".into(),
            tool_use_id: tool_use_id.into(),
        })
    }

    pub fn interrupt(&mut self, at: SystemTime) -> SessionEffects {
        self.apply(SessionInput::Interrupt {
            session: "s1".into(),
            at,
        })
    }

    /// The releases since the last call.
    pub fn take_releases(&mut self) -> Vec<Release> {
        std::mem::take(&mut self.releases)
    }

    pub fn session(&self) -> Option<&Session> {
        self.store.session(&SessionId::from("s1"))
    }

    pub fn view(&self) -> Option<SessionView> {
        self.store.view(&SessionId::from("s1"))
    }

    pub fn state(&self) -> SessionState {
        self.view().expect("session s1 exists").state
    }
}

impl Default for Harness {
    fn default() -> Self {
        Harness::new()
    }
}
