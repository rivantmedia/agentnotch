//! End to end: the Mac simulator's scenarios (`Packages/ClaudeControl/
//! DevTools/simulate-sessions.py`) played as JSON over `MemoryTransport`.
//!
//! Every hook event and status line is built the way the Windows hook exe
//! builds it (`build_hook_message`, `encode_hook_message`,
//! `build_statusline_message`, `encode_frame`), injected into a
//! `MemoryTransport`, drained as `TransportEvent`s, handled by WP1's real
//! `HookIngress` (decode, the ToolUseIdCache, held requests) and fed to the
//! `SessionStore`; answers are the hub's mapping
//! (`control::answers::permission_response`) written through
//! `HookIngress::answer`, and the store's releases close held connections
//! through `HookIngress::release`. Every job the store asks for is run synchronously: the
//! registry reads (against `FakeProcesses` stand-ins for the fake Claude
//! processes), the transcript syncs, chat loads, Desktop lookups and the
//! review file's write. The clock is the store's own: the harness sends
//! `Tick` at `next_deadline`, as the runtime does.
//!
//! The scenario files are in `tests/fixtures/sessions/sim/` (see its
//! README.md for the step format). The Python's `EXPECT` lines are `expect`
//! steps; nothing here ran the Python, a hook script or `claude`.
//!
//! Not covered, by design: usage (the `ratelimit` scenario checks only the
//! session side, StopFailure `rate_limit` -> Failed "Rate limited"; usage
//! readings are WP4's).

mod sessions_support;

use agentnotch_engine::control::answers::permission_response;
use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::{project_slug, Paths};
use agentnotch_engine::core::time::{iso8601, to_ms};
use agentnotch_engine::ingress::{fill_status_line_account, HookIngress};
use agentnotch_engine::model::{
    AccountId, Answer, AttentionTransition, Attribution, IdentityId, NeedsInputReason, Phase,
    SessionId, SessionState, SessionView,
};
use agentnotch_engine::persist::review::ReviewStateFile;
use agentnotch_engine::platform::{ConnId, HookTransport, Processes, TransportEvent};
use agentnotch_engine::runtime_types::{
    AnswerResult, IngestContext, IngressConfig, IngressOut, Job, PersistFile, Release,
    SessionEffects, SessionInput,
};
use agentnotch_engine::sessions::background::WaitTiming;
use agentnotch_engine::sessions::chat::{load_chat, PAGE_SIZE};
use agentnotch_engine::sessions::completion::CompletionTiming;
use agentnotch_engine::sessions::desktop::find_hosted;
use agentnotch_engine::sessions::registry::read_registry;
use agentnotch_engine::sessions::transcript::{is_agent_transcript, sync_transcript};
use agentnotch_engine::sessions::SessionStore;
use agentnotch_engine::testkit::process::FakeProcesses;
use agentnotch_engine::testkit::transport::MemoryTransport;
use agentnotch_proto::frame::encode_frame;
use agentnotch_proto::message::{
    build_hook_message, build_statusline_message, encode_hook_message, HookEnv,
};
use agentnotch_proto::permission::permission_output_for_stdin;
use serde_json::{json, Map, Value};
use sessions_support::frames::unframe;
use sessions_support::Harness;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The scenarios the Python runs for `--scenario all`, in its order, and the
/// extra one.
const SCENARIOS: [&str; 10] = [
    "permission",
    "question",
    "tasks",
    "review",
    "ratelimit",
    "statusline",
    "registry",
    "goal",
    "bgagent",
    "bgworkflow",
];
const EXTRA_SCENARIOS: [&str; 1] = ["burst"];

/// How often the hub reads every registry (`registry::SCAN_INTERVAL`).
const SCAN: Duration = Duration::from_secs(3);

/// The two fake accounts (the Python's `ACCOUNTS`).
const ACCOUNTS: [(&str, &str); 2] = [("personal", ".claude"), ("work", ".claude-work")];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions/sim")
}

fn load_scenario(name: &str) -> Value {
    let path = fixtures().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("scenario {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("scenario {name}: {error}"))
}

/// One fake Claude session: its stand-in process, transcript and registry
/// file (the Python's `FakeSession`).
struct Fake {
    name: String,
    group: Option<String>,
    id: SessionId,
    folder: PathBuf,
    cwd: String,
    title: String,
    pid: u32,
    transcript: PathBuf,
    registry: PathBuf,
    started_ms: u64,
    registry_status: Option<String>,
}

/// A PermissionRequest the hook exe holds open, as ingress tracks it.
struct Held {
    conn: ConnId,
    session: SessionId,
    tool_use_id: String,
    tool: String,
    agent_id: Option<String>,
    /// Claude Code's stdin for it (the answer is merged onto its input).
    stdin: Vec<u8>,
}

struct Sim {
    h: Harness,
    _root: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    procs: Arc<FakeProcesses>,
    transport: Arc<MemoryTransport>,
    rx: crossbeam_channel::Receiver<TransportEvent>,
    ingress: HookIngress,
    support: PathBuf,
    fakes: Vec<Fake>,
    held: Vec<Held>,
    /// What the hook exe printed for the last answer of each session.
    outputs: BTreeMap<String, String>,
    transitions: Vec<AttentionTransition>,
    next_scan: std::time::SystemTime,
    played: BTreeSet<String>,
    next_pid: u32,
    counter: u64,
    last_stdin: Option<Vec<u8>>,
    // What proves the frames travelled through the transport.
    injected: usize,
    drained: usize,
    applied_from_frames: usize,
    dropped: Vec<&'static str>,
    answers_sent: usize,
    persisted: usize,
}

impl Sim {
    fn new() -> Sim {
        let dir = tempfile::tempdir().expect("a temporary root");
        let root = dir.path().to_path_buf();
        let paths = Paths::native(&root);
        let procs = Arc::new(FakeProcesses::default());
        let mut h = Harness::reading(&root, CompletionTiming::STANDARD, WaitTiming::STANDARD);
        h.store = SessionStore::new()
            .with_paths(paths.clone())
            .with_timing(CompletionTiming::STANDARD, WaitTiming::STANDARD)
            .with_processes(procs.clone());
        let transport = Arc::new(MemoryTransport::default());
        let (sink, rx) = crossbeam_channel::unbounded();
        transport
            .start("agentnotch-e2e", sink)
            .expect("the transport starts");
        let support = root.join("support");
        std::fs::create_dir_all(&support).unwrap();
        for (_, folder) in ACCOUNTS {
            std::fs::create_dir_all(root.join(folder).join("sessions")).unwrap();
        }
        h.store.load_review(None, h.now);
        let ingress =
            HookIngress::with_transport(IngressConfig::new("agentnotch-e2e"), transport.clone());
        let mut sim = Sim {
            next_scan: h.now + SCAN,
            h,
            _root: dir,
            root,
            paths,
            procs,
            transport,
            rx,
            ingress,
            support,
            fakes: Vec::new(),
            held: Vec::new(),
            outputs: BTreeMap::new(),
            transitions: Vec::new(),
            played: BTreeSet::new(),
            next_pid: 41_000,
            counter: 0,
            last_stdin: None,
            injected: 0,
            drained: 0,
            applied_from_frames: 0,
            dropped: Vec::new(),
            answers_sent: 0,
            persisted: 0,
        };
        // The launch: every registry read once, then the baseline settles.
        sim.scan();
        sim.h.store.initial_scan_completed(sim.h.now);
        sim.advance(2_500);
        sim
    }

    // ---- the account folders ----

    fn folder_of(&self, account: &str) -> PathBuf {
        let (_, folder) = ACCOUNTS
            .iter()
            .find(|(name, _)| *name == account)
            .unwrap_or_else(|| panic!("no account {account}"));
        self.root.join(folder)
    }

    fn account_id(&self, account: &str) -> AccountId {
        AccountId::from(
            self.paths
                .normalize(&self.folder_of(account).to_string_lossy()),
        )
    }

    /// What the hub says about a frame: the folder it ran in and whose it
    /// is (each fake account is its own identity).
    fn context_for(
        &self,
        transcript: Option<&str>,
        env: Option<&str>,
        pid: Option<u32>,
    ) -> IngestContext {
        let folder = self.paths.session_config_dir(transcript, env, |_| false);
        let identity = ACCOUNTS.iter().find_map(|(name, _)| {
            self.paths
                .same(&folder, self.account_id(name).as_str())
                .then(|| IdentityId::from(format!("uuid:acct-{name}")))
        });
        IngestContext {
            attribution: Attribution::Known(identity),
            account: Some(AccountId::from(folder)),
            trusted_pid: pid,
            pid_started: pid.and_then(|pid| self.procs.start_time(pid)),
        }
    }

    // ---- the fake sessions ----

    fn create_sessions(&mut self, spec: &Value) {
        let count = spec.get("count").and_then(Value::as_u64);
        let base = spec["name"].as_str().expect("a session name").to_owned();
        for i in 0..count.unwrap_or(1) {
            let sub = |text: &str| text.replace("{i}", &i.to_string());
            let account = match spec.get("accounts").and_then(Value::as_array) {
                Some(list) => list[i as usize % list.len()].as_str().unwrap(),
                None => spec["account"].as_str().expect("an account"),
            };
            let name = if count.is_some() {
                format!("{base}{i}")
            } else {
                base.clone()
            };
            let group = count.map(|_| base.clone());
            self.create_session(
                &name,
                group,
                account,
                &sub(spec["cwd"].as_str().expect("a cwd")),
                &sub(spec["title"].as_str().expect("a title")),
                spec["prefix"].as_str().expect("an id prefix"),
            );
        }
    }

    fn create_session(
        &mut self,
        name: &str,
        group: Option<String>,
        account: &str,
        cwd: &str,
        title: &str,
        prefix: &str,
    ) {
        assert!(
            self.fakes.iter().all(|fake| fake.name != name),
            "session {name} is made twice"
        );
        let (account, _) = ACCOUNTS
            .iter()
            .find(|(candidate, _)| *candidate == account)
            .unwrap_or_else(|| panic!("no account {account}"));
        let folder = self.folder_of(account);
        self.counter += 1;
        let id = SessionId::from(format!("{prefix}0000-0000-4000-8000-{:012x}", self.counter));
        let project = folder.join("projects").join(project_slug(cwd));
        std::fs::create_dir_all(&project).unwrap();
        let pid = self.next_pid;
        self.next_pid += 1;
        // A stand-in "Claude process" that started a second ago.
        self.procs
            .add(pid, 1, "claude.exe", self.h.now - Duration::from_secs(1));
        let fake = Fake {
            name: name.to_owned(),
            group,
            transcript: project.join(format!("{id}.jsonl")),
            registry: folder.join("sessions").join(format!("{pid}.json")),
            id,
            folder,
            cwd: cwd.to_owned(),
            title: title.to_owned(),
            pid,
            started_ms: to_ms(self.h.now),
            registry_status: None,
        };
        let lines = vec![
            json!({"type": "ai-title", "aiTitle": title, "sessionId": fake.id.as_str()}),
            json!({"type": "user", "uuid": self.uuid(), "sessionId": fake.id.as_str(),
                   "timestamp": iso8601(self.h.now),
                   "message": {"role": "user", "content": format!("Please work on: {title}")}}),
        ];
        self.fakes.push(fake);
        let last = self.fakes.len() - 1;
        self.append_transcript(last, &lines);
    }

    fn uuid(&mut self) -> String {
        self.counter += 1;
        format!("{:08x}-0000-4000-8000-{:012x}", self.counter, self.counter)
    }

    fn index(&self, name: &str) -> Result<usize, String> {
        self.fakes
            .iter()
            .position(|fake| fake.name == name)
            .ok_or_else(|| format!("no session named {name}"))
    }

    fn append_transcript(&mut self, index: usize, lines: &[Value]) {
        sessions_support::append_lines(&self.fakes[index].transcript, lines);
    }

    // ---- the registry files (what Claude Code writes) ----

    #[allow(clippy::too_many_arguments)]
    fn write_registry(
        &mut self,
        index: usize,
        status: &str,
        waiting_for: Option<&str>,
        kind: &str,
        entrypoint: &str,
        name: Option<&str>,
        name_source: &str,
    ) {
        let now_ms = to_ms(self.h.now);
        let fake = &mut self.fakes[index];
        let base = fake.cwd.rsplit('/').next().unwrap_or("").to_owned();
        let mut entry = json!({
            "pid": fake.pid, "sessionId": fake.id.as_str(), "cwd": fake.cwd,
            "startedAt": fake.started_ms, "version": "2.1.280", "kind": kind,
            "entrypoint": entrypoint,
            // Claude Code derives "<folder>-<n>" unless the session was named.
            "name": name.map(str::to_owned).unwrap_or_else(|| format!("{base}-{}", fake.pid % 97)),
            "nameSource": if name.is_some() { name_source } else { "derived" },
            "status": status, "updatedAt": now_ms, "statusUpdatedAt": now_ms,
        });
        if let Some(waiting) = waiting_for {
            entry["waitingFor"] = json!(waiting);
        }
        // Atomically, as a reader may poll it at any moment.
        let temporary = fake.registry.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec(&entry).unwrap()).unwrap();
        std::fs::rename(&temporary, &fake.registry).unwrap();
        fake.registry_status = Some(status.to_owned());
    }

    /// Claude Code updates its registry after the hooks of a status change
    /// ran (`FakeSession.mirror_registry`).
    fn mirror_registry(&mut self, index: usize, event: &str, fields: &Map<String, Value>) {
        if fields.get("agent_id").is_some_and(|id| !id.is_null()) {
            return;
        }
        let status = match event {
            "SessionStart" | "StopFailure" => "idle",
            "UserPromptSubmit" => "busy",
            "Stop" => {
                // Claude Code 2.1.x stays busy while agents, workflows, cloud
                // sessions or teammates run, and says shell for shells and
                // monitors only.
                let types: Vec<&str> = fields
                    .get("background_tasks")
                    .and_then(Value::as_array)
                    .map(|tasks| tasks.iter().filter_map(|t| t["type"].as_str()).collect())
                    .unwrap_or_default();
                if types
                    .iter()
                    .any(|t| ["subagent", "workflow", "teammate", "cloud session"].contains(t))
                {
                    "busy"
                } else if types.iter().any(|t| ["shell", "monitor"].contains(t)) {
                    "shell"
                } else {
                    "idle"
                }
            }
            _ => return,
        };
        if self.fakes[index].registry_status.as_deref() != Some(status) {
            self.write_registry(index, status, None, "interactive", "cli", None, "derived");
        }
    }

    // ---- frames over the transport ----

    /// Injects the message as a frame and applies what the store gets of it.
    fn send(&mut self, message_bytes: &[u8], stdin: Vec<u8>) {
        self.last_stdin = Some(stdin);
        let frame = encode_frame(message_bytes).expect("a frame");
        // The pipe server reads the length and hands the engine the body.
        let body = unframe(&frame).expect("a well-formed frame");
        self.transport
            .inject(serde_json::to_vec(&body).unwrap(), self.h.now);
        self.injected += 1;
        self.pump();
    }

    /// Drains the transport's events through the ingress and feeds the
    /// store, in order.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            let is_frame = matches!(event, TransportEvent::Frame(_));
            if is_frame {
                self.drained += 1;
            }
            let outs = self.ingress.on_transport(event, self.h.now);
            let mut reached = false;
            for out in outs {
                let input = match out {
                    IngressOut::Hook(event) => {
                        let ctx = self.context_for(
                            event.transcript_path.as_deref(),
                            event.config_dir_env.as_deref(),
                            event.pid,
                        );
                        SessionInput::Hook { event, ctx }
                    }
                    IngressOut::StatusLine(mut message) => {
                        fill_status_line_account(&mut message, &self.paths, |_| false);
                        let ctx = self.context_for(
                            message.transcript_path.as_deref(),
                            message.config_dir_env.as_deref(),
                            message.pid,
                        );
                        SessionInput::StatusLine { message, ctx }
                    }
                    IngressOut::PermissionHeld(held) => {
                        self.held.push(Held {
                            conn: held.conn,
                            session: held.session_id.clone(),
                            tool_use_id: held.tool_use_id.clone(),
                            tool: held.event.tool.clone().unwrap_or_default(),
                            agent_id: held.agent_id.clone(),
                            stdin: self.last_stdin.clone().unwrap_or_default(),
                        });
                        SessionInput::Held(held)
                    }
                    IngressOut::PermissionFailed {
                        session,
                        tool_use_id,
                    } => SessionInput::PermissionFailed {
                        session,
                        tool_use_id,
                    },
                    IngressOut::Control { .. } | IngressOut::TransportStatus(_) => continue,
                };
                reached = true;
                self.apply(input);
            }
            if is_frame {
                if reached {
                    self.applied_from_frames += 1;
                } else {
                    self.dropped.push("a frame the ingress kept from the store");
                }
            }
            self.forget_closed();
        }
    }

    /// The held requests the ingress closed itself (a PostToolUse, the
    /// main Stop, SessionEnd) are no longer held.
    fn forget_closed(&mut self) {
        let ingress = &self.ingress;
        self.held
            .retain(|held| ingress.is_pending(&held.session, &held.tool_use_id));
    }

    // ---- the store and its jobs ----

    fn apply(&mut self, input: SessionInput) {
        let effects = self.h.apply(input);
        let mut queue = VecDeque::new();
        self.take(effects, &mut queue);
        self.run_queue(queue);
    }

    /// Notes what an input caused: the attention news, the held hooks to
    /// close and the jobs to run.
    fn take(&mut self, effects: SessionEffects, queue: &mut VecDeque<Job>) {
        self.transitions.extend(effects.transitions);
        queue.extend(effects.jobs);
        for release in self.h.take_releases() {
            self.release(release);
        }
    }

    fn release(&mut self, release: Release) {
        self.ingress.release(release);
        self.forget_closed();
    }

    /// Runs jobs the way the runtime's `an-io` lane does, and feeds their
    /// results back, until none is left.
    fn run_queue(&mut self, mut queue: VecDeque<Job>) {
        for _ in 0..100_000 {
            let Some(job) = queue.pop_front() else {
                return;
            };
            let now = self.h.now;
            let effects = match job {
                Job::SyncTranscript {
                    session,
                    path,
                    cursor,
                } => {
                    let delta = sync_transcript(
                        &session,
                        &path,
                        cursor,
                        is_agent_transcript(&path),
                        &StdSecureFiles,
                    );
                    self.h.apply(SessionInput::TranscriptSynced(delta))
                }
                Job::LoadChat {
                    session,
                    path,
                    before,
                } => {
                    let page = load_chat(&session, &path, before.as_deref(), PAGE_SIZE);
                    let effects = self.h.store.chat_loaded(page, now);
                    self.h.releases.extend(effects.release.iter().cloned());
                    effects
                }
                Job::ReadRegistry {
                    sessions_dir,
                    via_link,
                } => {
                    let snapshot = read_registry(&sessions_dir, via_link, &*self.procs, now);
                    self.h.apply(SessionInput::Registry(snapshot))
                }
                Job::DesktopHosted {
                    roots,
                    host_session_id,
                    candidates,
                } => {
                    let identity = find_hosted(
                        &roots,
                        &host_session_id,
                        &candidates,
                        &StdSecureFiles,
                        false,
                    );
                    let sessions: Vec<SessionId> = self
                        .h
                        .store
                        .views()
                        .into_iter()
                        .filter(|view| view.host_session_id.as_deref() == Some(&host_session_id))
                        .map(|view| view.id)
                        .collect();
                    let mut last = SessionEffects::default();
                    for session in sessions {
                        let effects = self.h.apply(SessionInput::Hosted {
                            session,
                            identity: identity.clone(),
                        });
                        self.take(effects, &mut queue);
                        last = SessionEffects::default();
                    }
                    last
                }
                Job::Persist { file, bytes } => {
                    assert_eq!(file, PersistFile::Review, "only the review file is written");
                    std::fs::write(self.support.join(file.file_name()), bytes).unwrap();
                    self.persisted += 1;
                    SessionEffects::default()
                }
                other => panic!("the sessions store asked for a job nobody here runs: {other:?}"),
            };
            self.take(effects, &mut queue);
        }
        panic!("the job runner did not settle");
    }

    /// The hub reads every account's registry.
    fn scan(&mut self) {
        let queue = ACCOUNTS
            .iter()
            .map(|(_, folder)| Job::ReadRegistry {
                sessions_dir: self.root.join(folder).join("sessions"),
                via_link: false,
            })
            .collect();
        self.run_queue(queue);
    }

    /// Moves the clock `ms` on, sending `Tick` at every deadline the store
    /// names and reading the registries every 3 s, as the runtime does.
    fn advance(&mut self, ms: u64) {
        let until = self.h.now + Duration::from_millis(ms);
        for _ in 0..200_000 {
            let deadline = self.h.store.next_deadline();
            let next = deadline.map_or(self.next_scan, |d| d.min(self.next_scan));
            if next > until {
                self.h.now = self.h.now.max(until);
                return;
            }
            self.h.now = self.h.now.max(next);
            if self.next_scan <= self.h.now {
                while self.next_scan <= self.h.now {
                    self.next_scan += SCAN;
                }
                self.scan();
            }
            if self
                .h
                .store
                .next_deadline()
                .is_some_and(|d| d <= self.h.now)
            {
                self.apply(SessionInput::Tick);
            }
        }
        panic!("the clock did not settle");
    }

    // ---- the scenarios ----

    fn run_file(&mut self, name: &str) {
        if !self.played.insert(name.to_owned()) {
            return;
        }
        let doc = load_scenario(name);
        for earlier in doc["after"].as_array().into_iter().flatten() {
            self.run_file(earlier.as_str().expect("a scenario name"));
        }
        for spec in doc["sessions"].as_array().into_iter().flatten() {
            self.create_sessions(spec);
        }
        let latency = doc["hook_latency_ms"].as_u64().unwrap_or(100);
        for (number, step) in doc["steps"].as_array().expect("steps").iter().enumerate() {
            if let Err(problem) = self.step(step, None, latency) {
                panic!(
                    "scenario {name}, step {number} ({}): {problem}",
                    step["op"].as_str().unwrap_or("?")
                );
            }
        }
    }

    fn step(&mut self, step: &Value, forced: Option<&str>, latency: u64) -> Result<(), String> {
        let op = step["op"].as_str().ok_or("a step without an op")?;
        let session = forced.or_else(|| step["session"].as_str());
        let need = || session.ok_or_else(|| "the step names no session".to_owned());
        match op {
            "hook" => {
                let index = self.index(need()?)?;
                self.hook(index, step, latency)?;
            }
            "status_line" => {
                let index = self.index(need()?)?;
                self.status_line(index, step);
                self.advance(latency);
            }
            "transcript" => {
                let index = self.index(need()?)?;
                self.transcript(index, step)?;
            }
            "registry" => {
                let index = self.index(need()?)?;
                self.write_registry(
                    index,
                    step["status"].as_str().ok_or("a registry status")?,
                    step["waiting_for"].as_str(),
                    step["kind"].as_str().unwrap_or("interactive"),
                    step["entrypoint"].as_str().unwrap_or("cli"),
                    step["name"].as_str(),
                    step["name_source"].as_str().unwrap_or("derived"),
                );
            }
            "answer" => {
                let index = self.index(need()?)?;
                self.answer(index, step, latency)?;
            }
            "advance" => {}
            "expect" => self.expect(step, session)?,
            "each" => {
                let group = step["group"].as_str().ok_or("a group")?.to_owned();
                let members: Vec<String> = self
                    .fakes
                    .iter()
                    .filter(|fake| fake.group.as_deref() == Some(&group))
                    .map(|fake| fake.name.clone())
                    .collect();
                for turn in 0..step["turns"].as_u64().unwrap_or(1) {
                    for (i, member) in members.iter().enumerate() {
                        for sub in step["steps"].as_array().ok_or("steps")? {
                            let sub = substitute(sub, turn, i);
                            self.step(&sub, Some(member), latency)?;
                        }
                    }
                }
            }
            other => return Err(format!("unknown op {other}")),
        }
        if let Some(ms) = step["advance_ms"].as_u64() {
            self.advance(ms);
        }
        Ok(())
    }

    fn hook(&mut self, index: usize, step: &Value, latency: u64) -> Result<(), String> {
        let event = step["event"].as_str().ok_or("a hook event")?;
        let fields = step["fields"].as_object().cloned().unwrap_or_default();
        let fake = &self.fakes[index];
        // Claude Code's stdin (FakeSession.payload).
        let mut payload = json!({
            "hook_event_name": event, "session_id": fake.id.as_str(),
            "transcript_path": fake.transcript.to_string_lossy(), "cwd": fake.cwd,
            "permission_mode": "default",
        });
        for (key, value) in &fields {
            payload[key] = value.clone();
        }
        // The environment Claude Code gives hooks (FakeSession.hook_env).
        let pid = fake.pid.to_string();
        let config_dir = fake.folder.to_string_lossy().into_owned();
        let env = HookEnv::from_env(
            |key| match key {
                "CLAUDE_PID" => Some(pid.clone()),
                "CLAUDE_CONFIG_DIR" => Some(config_dir.clone()),
                "CLAUDE_CODE_SESSION_ATTENDED" => Some("1".into()),
                "CLAUDE_CODE_ENTRYPOINT" => Some("cli".into()),
                _ => None,
            },
            7_000 + self.counter as u32,
        );
        let message = build_hook_message(&payload, &env).ok_or("stdin is not an object")?;
        let bytes = encode_hook_message(&message);
        let stdin = serde_json::to_vec(&payload).unwrap();
        self.send(&bytes, stdin);

        let mirror = step["mirror"].as_bool().unwrap_or(true);
        let is_main = !fields.get("agent_id").is_some_and(|id| !id.is_null());
        if event == "PermissionRequest" {
            // The terminal shows its permission dialog at the same time.
            if mirror && is_main {
                self.advance(200);
                self.write_registry(
                    index,
                    "waiting",
                    Some("permission prompt"),
                    "interactive",
                    "cli",
                    None,
                    "derived",
                );
            }
        } else if mirror {
            self.mirror_registry(index, event, &fields);
        }
        self.advance(latency);
        Ok(())
    }

    fn status_line(&mut self, index: usize, step: &Value) {
        let fake = &self.fakes[index];
        let now = self
            .h
            .now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let stdin = json!({
            "session_id": fake.id.as_str(),
            "transcript_path": fake.transcript.to_string_lossy(),
            "cwd": fake.cwd,
            "rate_limits": {
                "five_hour": {"used_percentage": step["used_5h"], "resets_at": now + 2 * 3600 + 13 * 60},
                "seven_day": {"used_percentage": step["used_7d"], "resets_at": now + 3 * 86_400},
            },
            "context_window": {"used_percentage": step["context_pct"], "context_window_size": 200_000},
            "model": {"id": "claude-opus-4-5", "display_name": "Opus 4.5"},
            "cost": {"total_cost_usd": step["cost"]},
            "session_name": fake.title,
            "version": "2.1.280",
        });
        let pid = fake.pid.to_string();
        let config_dir = fake.folder.to_string_lossy().into_owned();
        let env = HookEnv::from_env(
            |key| match key {
                "CLAUDE_PID" => Some(pid.clone()),
                "CLAUDE_CONFIG_DIR" => Some(config_dir.clone()),
                _ => None,
            },
            0,
        );
        let message = build_statusline_message(&stdin, &env).expect("an object");
        self.send(&serde_json::to_vec(&message).unwrap(), Vec::new());
    }

    /// Appends lines to the session's transcript (FakeSession's writers).
    fn transcript(&mut self, index: usize, step: &Value) -> Result<(), String> {
        for line in step["lines"].as_array().ok_or("lines")? {
            let id = self.fakes[index].id.as_str().to_owned();
            let at = iso8601(self.h.now);
            if let Some(turn) = line.get("assistant_turn") {
                let context = turn["context_tokens"].as_i64().ok_or("context_tokens")?;
                self.counter += 1;
                let message_id = format!("msg_{:x}", self.counter);
                let mut blocks = vec![json!({"type": "text", "text": turn["text"]})];
                blocks.extend(
                    turn["tool_uses"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut tool| {
                            tool["type"] = json!("tool_use");
                            tool
                        }),
                );
                // Repeated lines share the message id and usage, like
                // Claude Code's streamed blocks.
                let usage = json!({
                    "input_tokens": 12, "output_tokens": 180,
                    "cache_read_input_tokens": context - 2012, "cache_creation_input_tokens": 2000,
                });
                let lines: Vec<Value> = blocks
                    .into_iter()
                    .map(|block| {
                        json!({
                            "type": "assistant", "uuid": self.uuid(), "sessionId": id,
                            "requestId": format!("req_{message_id}"), "isSidechain": false,
                            "timestamp": at,
                            "message": {"id": message_id, "role": "assistant",
                                        "model": "claude-opus-4-5", "content": [block],
                                        "usage": usage},
                        })
                    })
                    .collect();
                self.append_transcript(index, &lines);
            } else if let Some(result) = line.get("tool_result") {
                let mut entry = json!({
                    "type": "user", "uuid": self.uuid(), "sessionId": id, "isSidechain": false,
                    "timestamp": at,
                    "message": {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": result["id"], "content": result["text"]},
                    ]},
                });
                if let Some(structured) = result.get("structured").filter(|s| !s.is_null()) {
                    entry["toolUseResult"] = structured.clone();
                }
                self.append_transcript(index, &[entry]);
            } else {
                return Err(format!("unknown transcript line {line}"));
            }
        }
        Ok(())
    }

    /// The user answers a held request: the store hears it, the hook exe
    /// gets its response frame over the transport and prints its output.
    fn answer(&mut self, index: usize, step: &Value, latency: u64) -> Result<(), String> {
        let (name, id) = {
            let fake = &self.fakes[index];
            (fake.name.clone(), fake.id.clone())
        };
        let tool = step["tool"].as_str();
        let agent = step["agent"].as_str();
        let position = self
            .held
            .iter()
            .position(|held| {
                held.session == id
                    && tool.is_none_or(|tool| held.tool == tool)
                    && (agent.is_none() || held.agent_id.as_deref() == agent)
            })
            .ok_or("no held request matches")?;
        let held = self.held.remove(position);
        let answer: Answer =
            serde_json::from_value(step["answer"].clone()).map_err(|e| e.to_string())?;
        let request = self
            .h
            .store
            .view(&id)
            .and_then(|view| {
                view.pending
                    .into_iter()
                    .find(|request| request.tool_use_id == held.tool_use_id)
            })
            .ok_or("the store shows no such request")?;
        // What the hub sends the hook: the control package's mapping of the
        // request it showed, written once through the ingress, then the
        // store hears of it (the answer can't overtake a PostToolUse).
        let response = permission_response(&request, &answer)?;
        match self
            .ingress
            .answer(&id, &held.tool_use_id, response.clone())
        {
            AnswerResult::Delivered => {}
            other => return Err(format!("the answer was {other:?}")),
        }
        self.apply(SessionInput::PermissionResolved {
            session: id.clone(),
            tool_use_id: held.tool_use_id.clone(),
            answer: answer.clone(),
        });
        self.answers_sent += 1;
        let output = permission_output_for_stdin(&held.stdin, &response).unwrap_or_default();
        for needle in step["hook_output_contains"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let needle = needle.as_str().ok_or("a string")?;
            if !output.contains(needle) {
                return Err(format!("the hook printed {output:?}, not {needle:?}"));
            }
        }
        self.outputs.insert(name, output);
        // The dialog closes: Claude Code goes back to busy.
        if self.fakes[index].registry_status.as_deref() == Some("waiting") {
            self.write_registry(index, "busy", None, "interactive", "cli", None, "derived");
        }
        self.advance(latency);
        Ok(())
    }

    // ---- the Python's EXPECT lines ----

    fn expect(&mut self, step: &Value, session: Option<&str>) -> Result<(), String> {
        let targets: Vec<usize> = match step["group"].as_str() {
            Some(group) => {
                let members: Vec<usize> = (0..self.fakes.len())
                    .filter(|i| self.fakes[*i].group.as_deref() == Some(group))
                    .collect();
                if let Some(count) = step["count"].as_u64() {
                    if members.len() as u64 != count {
                        return Err(format!(
                            "{} sessions in {group}, not {count}",
                            members.len()
                        ));
                    }
                }
                members
            }
            None => vec![self.index(session.ok_or("the expect names no session")?)?],
        };
        let mut problems = Vec::new();
        for index in targets {
            let fake_name = self.fakes[index].name.clone();
            for problem in self.check(index, step) {
                problems.push(format!("{fake_name}: {problem}"));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn check(&self, index: usize, expect: &Value) -> Vec<String> {
        let fake = &self.fakes[index];
        let view = self.h.store.view(&fake.id);
        let mut problems = Vec::new();
        // What is read, by key; `diff` holds each against what the step wants.
        let mut readings: Vec<(&str, Value)> = Vec::new();
        let mut compare = |key: &'static str, actual: Value| readings.push((key, actual));
        compare("tracked", json!(view.is_some()));
        compare(
            "held",
            json!(self
                .held
                .iter()
                .filter(|held| held.session == fake.id)
                .count()),
        );
        compare(
            "ready_for_review_news",
            json!(self
                .transitions
                .iter()
                .filter(|t| t.session.id == fake.id && t.became_ready_for_review())
                .count()),
        );
        compare(
            "needs_you_news",
            json!(self
                .transitions
                .iter()
                .filter(|t| t.session.id == fake.id && t.became_needs_you())
                .count()),
        );
        compare(
            "hook_output",
            self.outputs
                .get(&fake.name)
                .map_or(Value::Null, |output| json!(output)),
        );
        if let Some(wanted) = expect.get("review_file") {
            let record = std::fs::read(self.support.join(PersistFile::Review.file_name()))
                .ok()
                .and_then(|bytes| ReviewStateFile::parse(&bytes))
                .and_then(|file| file.sessions.get(fake.id.as_str()).cloned());
            if wanted.as_bool() != Some(record.is_some()) {
                problems.push(format!("review_file has a record: {}", record.is_some()));
            }
            if let (Some(prefix), Some(record)) = (
                expect["review_file_message_starts_with"].as_str(),
                record.as_ref(),
            ) {
                let message = record.last_assistant_message.clone().unwrap_or_default();
                if !message.starts_with(prefix) {
                    problems.push(format!("the review file keeps {message:?}"));
                }
            }
        }
        let Some(view) = view else {
            diff(expect, readings, &mut problems);
            if expect.as_object().is_some_and(|keys| {
                keys.keys().any(|key| {
                    ![
                        "op",
                        "session",
                        "group",
                        "count",
                        "tracked",
                        "held",
                        "review_file",
                        "review_file_message_starts_with",
                        "ready_for_review_news",
                        "needs_you_news",
                        "hook_output",
                        "advance_ms",
                    ]
                    .contains(&key.as_str())
                })
            }) {
                problems.push("the session is not tracked".into());
            }
            return problems;
        };
        compare("state", json!(state_name(&view.state)));
        compare("bucket", json!(bucket_name(&view)));
        compare(
            "reason",
            view.state
                .reason()
                .map_or(Value::Null, |r| json!(reason_text(r))),
        );
        compare("phase", json!(phase_name(&view.phase)));
        compare(
            "account",
            json!(view.account.as_ref().map(|account| {
                ACCOUNTS
                    .iter()
                    .find(|(name, _)| {
                        self.paths
                            .same(account.as_str(), self.account_id(name).as_str())
                    })
                    .map_or("other", |(name, _)| *name)
            })),
        );
        compare(
            "tasks",
            json!(view
                .tasks
                .as_ref()
                .map(|t| format!("{}/{}", t.done, t.total))),
        );
        compare(
            "active_label",
            json!(view.tasks.as_ref().and_then(|t| t.active_label.clone())),
        );
        compare("context_pct", json!(view.context_pct));
        compare("model", json!(view.model));
        compare("title", json!(view.title));
        compare("registry_status", json!(view.registry_status));
        compare("last_assistant_message", json!(view.last_assistant_message));
        compare("last_message", json!(view.last_message));
        compare(
            "background_description",
            json!(view.background_wait_description),
        );
        compare(
            "completion_pending",
            json!(view.completion_pending_since.is_some()),
        );
        compare(
            "pending",
            json!(view
                .pending
                .iter()
                .map(|request| {
                    format!(
                        "{}:{}",
                        format!("{:?}", request.kind).to_lowercase(),
                        request.tool_name
                    )
                })
                .collect::<Vec<_>>()),
        );
        compare(
            "always_allow",
            json!(view
                .pending
                .first()
                .is_some_and(|request| request.always.is_some())),
        );
        if let Some(prefix) = expect["last_assistant_message_starts_with"].as_str() {
            let message = view.last_assistant_message.clone().unwrap_or_default();
            if !message.starts_with(prefix) {
                problems.push(format!("last_assistant_message is {message:?}"));
            }
        }
        diff(expect, readings, &mut problems);
        problems
    }

    /// The proof that everything went through the transport.
    fn finish(&self, frames_expected: bool) {
        assert!(
            self.dropped.is_empty(),
            "frames dropped: {:?}",
            self.dropped
        );
        assert_eq!(
            self.injected > 0,
            frames_expected,
            "frames injected: {}",
            self.injected
        );
        assert_eq!(
            self.injected, self.drained,
            "every injected frame is drained"
        );
        assert_eq!(
            self.drained, self.applied_from_frames,
            "every drained frame reached the store"
        );
        assert_eq!(self.transport.responses().len(), self.answers_sent);
        assert!(!self.transport.is_stopped());
    }
}

/// Each reading the step names must equal what the step wants of it.
fn diff(expect: &Value, readings: Vec<(&str, Value)>, problems: &mut Vec<String>) {
    for (key, actual) in readings {
        if let Some(wanted) = expect.get(key) {
            if *wanted != actual {
                problems.push(format!("{key} is {actual}, wanted {wanted}"));
            }
        }
    }
}

fn substitute(value: &Value, turn: u64, i: usize) -> Value {
    match value {
        Value::String(text) => Value::String(
            text.replace("{turn}", &turn.to_string())
                .replace("{i}", &i.to_string()),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| substitute(v, turn, i)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute(v, turn, i)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn state_name(state: &SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou(_) => "needs_you",
        SessionState::Failed(_) => "failed",
        SessionState::ReadyForReview => "ready_for_review",
        SessionState::Working => "working",
        SessionState::Idle => "idle",
    }
}

fn bucket_name(view: &SessionView) -> &'static str {
    use agentnotch_engine::model::Bucket;
    match view.state.bucket() {
        Bucket::NeedsYou => "needs_you",
        Bucket::ReadyForReview => "ready_for_review",
        Bucket::Working => "working",
        Bucket::Idle => "idle",
    }
}

fn reason_text(reason: &NeedsInputReason) -> String {
    match reason {
        NeedsInputReason::Permission { tool: Some(tool) } => format!("permission:{tool}"),
        NeedsInputReason::Permission { tool: None } => "permission".into(),
        NeedsInputReason::Question => "question".into(),
        NeedsInputReason::PlanApproval => "plan".into(),
        NeedsInputReason::Elicitation { message } => format!("elicitation:{message}"),
        NeedsInputReason::Dialog { detail } => format!("dialog:{detail}"),
        NeedsInputReason::Error { text, .. } => format!("error:{text}"),
    }
}

fn phase_name(phase: &Phase) -> String {
    match phase {
        Phase::Idle => "idle".into(),
        Phase::Processing => "processing".into(),
        Phase::WaitingForInput => "waiting_for_input".into(),
        Phase::WaitingForApproval(context) => format!("waiting_for_approval:{}", context.tool_name),
        Phase::Compacting => "compacting".into(),
        Phase::Ended => "ended".into(),
    }
}

fn play(names: &[&str]) -> Sim {
    let mut sim = Sim::new();
    for name in names {
        sim.run_file(name);
    }
    sim.finish(true);
    sim
}

#[test]
fn permission() {
    play(&["permission"]);
}

#[test]
fn question() {
    play(&["question"]);
}

#[test]
fn tasks() {
    play(&["tasks"]);
}

#[test]
fn review() {
    play(&["review"]);
}

#[test]
fn ratelimit() {
    play(&["ratelimit"]);
}

#[test]
fn statusline() {
    play(&["statusline"]);
}

/// No hook ever reports these sessions: the registry files alone make them.
#[test]
fn registry() {
    let mut sim = Sim::new();
    sim.run_file("registry");
    sim.finish(false);
}

#[test]
fn goal() {
    play(&["goal"]);
}

#[test]
fn bgagent() {
    play(&["bgagent"]);
}

#[test]
fn bgworkflow() {
    play(&["bgworkflow"]);
}

#[test]
fn burst() {
    play(&["burst"]);
}

/// `--scenario all`: the ten scenarios, one after the other, in one app.
#[test]
fn all_ten_scenarios_run_in_one_app() {
    let sim = play(&SCENARIOS);
    assert_eq!(sim.played.len(), SCENARIOS.len());
}

/// Every scenario file is played by a test above and none is missing.
#[test]
fn every_fixture_is_one_of_the_simulators_scenarios() {
    let mut files: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|entry| entry.ok()?.path().file_stem()?.to_str().map(str::to_owned))
        .collect();
    files.retain(|name| name != "README");
    files.sort();
    let mut wanted: Vec<String> = SCENARIOS
        .iter()
        .chain(EXTRA_SCENARIOS.iter())
        .map(|s| s.to_string())
        .collect();
    wanted.sort();
    assert_eq!(files, wanted);
}

/// The frames really are the transport's: the hook's answer is written back
/// to its own connection and a PostToolUse closes nothing already answered.
#[test]
fn answers_go_back_over_the_connection_that_asked() {
    let sim = play(&["permission", "question"]);
    let responses = sim.transport.responses();
    assert_eq!(responses.len(), 2);
    let first: Value = serde_json::from_slice(&responses[0].1).unwrap();
    assert_eq!(first["decision"], "allow");
    let second: Value = serde_json::from_slice(&responses[1].1).unwrap();
    assert_eq!(second["decision"], "allow");
    assert_eq!(
        second["updated_input"]["answers"]["Which database should we use?"],
        "Postgres"
    );
    assert_ne!(responses[0].0, responses[1].0);
    // Every other connection is closed at once (fire and forget); an
    // answered one is never closed unanswered as well.
    let closed = sim.transport.closed();
    assert!(
        responses.iter().all(|(conn, _)| !closed.contains(conn)),
        "an answered request was also closed"
    );
    assert!(sim.held.is_empty());
    assert_eq!(sim.ingress.held_count(), 0);
}

/// The agent's request survives the main Stop, and the end of the session's
/// turn closes it only when the store says so (the Release path).
#[test]
fn a_release_closes_the_held_connection_unanswered() {
    let mut sim = play(&["bgagent"]);
    assert_eq!(sim.held.len(), 1);
    let conn = sim.held[0].conn;
    assert!(!sim.transport.is_closed(conn));
    let session = sim.fakes[sim.index("bgagent").unwrap()].id.clone();
    sim.release(Release::Session(session));
    assert!(sim.held.is_empty());
    assert!(sim.transport.is_closed(conn));
    assert_eq!(sim.ingress.held_count(), 0);
    assert!(sim.transport.responses().is_empty());
}

/// The review file reaches `<support>` through the Persist job and parses.
#[test]
fn the_review_scenario_writes_a_review_file() {
    let sim = play(&["review"]);
    assert!(sim.persisted > 0);
    let bytes = std::fs::read(sim.support.join("review-state.json")).unwrap();
    assert!(ReviewStateFile::parse(&bytes).is_some());
}
