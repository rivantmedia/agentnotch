//! Hook ingress, wired into the hub (design §1.4, §4.4, §4.7; HS§4, HS§6):
//! the platform's hook pipe is attached to WP1's `HookIngress` at a start,
//! and its events reach `an-core` in arrival order as `Input::Transport`.
//! What the ingress makes of them goes on: hook events and status lines to
//! the session store (and status lines to the usage store), held
//! PermissionRequests to the session store unless their account is
//! untracked (then they are released at once, with no decision), a hook
//! that went away as a failed request, and control frames are answered.
//!
//! Held requests are the sharp edge: a PermissionRequest hook blocks Claude
//! Code until it is answered or its connection is closed. Each is answered
//! at most once (`Call::Answer` goes through `HookIngress::answer`, which
//! refuses a second answer), and every one is released when its session
//! ends or its process is gone (the session store's releases), when the
//! hooks are turned off, and at every stop (quit, the updater's exit, the
//! self-test's end).
//!
//! Owner: WP7.

use super::api::{AnswerReply, CallError, HubEvent};
use super::core_state::{to_value, Core};
use crate::control::answers::permission_response;
use crate::ingress::{fill_status_line_account, HookIngress};
use crate::model::{AccountId, Answer, HookEvent, SessionId, StatusLineMessage};
use crate::platform::TransportEvent;
use crate::runtime_types::{
    AnswerResult, IngestContext, IngressConfig, IngressOut, Input, Release, SessionInput,
    VersionSighting, VersionSource,
};
use crate::usage::parser::parse_status_line_rate_limits;
use agentnotch_proto::{ControlOp, ControlResponse};
use crossbeam_channel::Sender;
use serde_json::Value;
use std::time::SystemTime;

/// The log line the smoke test looks for once the pipe listens.
pub(crate) const PIPE_LISTENING: &str = "pipe listening";

/// The pipe as `control status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PipeState {
    /// Never started (a hub that never ran, a sealed run).
    Off,
    Listening,
    /// Not listening, and why.
    Failed(String),
}

pub(crate) struct IngressWiring {
    pub(crate) ingress: HookIngress,
    pub(crate) pipe: PipeState,
    /// Where the transport's events are forwarded to (`an-core`'s queue);
    /// set by the runtime before a start.
    pub(crate) inputs: Option<Sender<Input>>,
    /// The transport was started (it is stopped at a stop).
    started: bool,
}

impl IngressWiring {
    pub(crate) fn new(pipe_name: &str) -> IngressWiring {
        IngressWiring {
            ingress: HookIngress::new(IngressConfig::new(pipe_name)),
            pipe: PipeState::Off,
            inputs: None,
            started: false,
        }
    }

    /// `control status`'s `transport`: `listening`, `in_use` (another
    /// program holds the pipe's name), `error` or `off`.
    pub(crate) fn status_word(&self) -> &'static str {
        match &self.pipe {
            PipeState::Off => "off",
            PipeState::Listening => "listening",
            PipeState::Failed(why) if why.contains("in use") => "in_use",
            PipeState::Failed(_) => "error",
        }
    }
}

impl Core {
    /// The hub starts: the pipe is attached and starts listening (never
    /// when sealed). Its events come back through `an-core`'s queue.
    pub(crate) fn ingress_on_start(&mut self) {
        if self.cfg.flags.sealed || self.ingress_w.started {
            return;
        }
        let Some(inputs) = self.ingress_w.inputs.clone() else {
            return;
        };
        self.ingress_w
            .ingress
            .attach(self.platform.transport.clone());
        let (sink, events) = crossbeam_channel::unbounded::<TransportEvent>();
        // The transport hands its events to a channel of its own; this
        // thread passes them on in order. It ends when the transport lets
        // its sink go (at its stop).
        let forwarder = std::thread::Builder::new()
            .name("an-pipe-events".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if inputs.send(Input::Transport(event)).is_err() {
                        return;
                    }
                }
            });
        if let Err(e) = forwarder {
            self.pipe_failed(format!("the hook pipe's events can't be read: {e}"));
            return;
        }
        match self.ingress_w.ingress.start(sink) {
            Ok(()) => self.ingress_w.started = true,
            Err(why) => self.pipe_failed(why),
        }
    }

    /// The hub stops: every held request is closed with no answer (each
    /// waiting hook exits with no output and Claude Code's own prompt
    /// decides), and the pipe stops listening.
    pub(crate) fn ingress_on_stop(&mut self) {
        self.ingress_w.ingress.release(Release::All);
        if self.ingress_w.started {
            self.ingress_w.ingress.stop();
            self.ingress_w.started = false;
            self.ingress_w.pipe = PipeState::Off;
        }
    }

    fn pipe_failed(&mut self, why: String) {
        if self.ingress_w.pipe != PipeState::Failed(why.clone()) {
            self.log(format!("pipe not listening: {why}"));
        }
        self.transport_error = Some(why.clone());
        self.ingress_w.pipe = PipeState::Failed(why);
    }

    /// One event of the pipe, in arrival order.
    pub(crate) fn transport_event(&mut self, event: TransportEvent) {
        let now = self.platform.clock.now();
        let outs = self.ingress_w.ingress.on_transport(event, now);
        for out in outs {
            self.ingress_out(out, now);
        }
    }

    fn ingress_out(&mut self, out: IngressOut, now: SystemTime) {
        match out {
            IngressOut::TransportStatus(Ok(_)) => {
                if self.ingress_w.pipe != PipeState::Listening {
                    self.log(PIPE_LISTENING.into());
                }
                self.ingress_w.pipe = PipeState::Listening;
                self.transport_error = None;
            }
            IngressOut::TransportStatus(Err(why)) => self.pipe_failed(why),
            IngressOut::Hook(event) => {
                let ctx = self.ingest_context(
                    event.transcript_path.as_deref(),
                    event.config_dir_env.as_deref(),
                    event.pid,
                );
                self.sessions_input(SessionInput::Hook { event, ctx }, now);
            }
            IngressOut::StatusLine(message) => self.status_line(message, now),
            IngressOut::PermissionHeld(held) => {
                if self.passes_through(&held.event) {
                    // An untracked or forgotten account: nothing shows the
                    // session, so nothing may hold its request. The
                    // terminal asks.
                    self.ingress_w.ingress.release(Release::Request {
                        session: held.session_id.clone(),
                        tool_use_id: held.tool_use_id.clone(),
                    });
                    return;
                }
                self.sessions_input(SessionInput::Held(held), now);
            }
            IngressOut::PermissionFailed {
                session,
                tool_use_id,
            } => {
                self.sessions_input(
                    SessionInput::PermissionFailed {
                        session,
                        tool_use_id,
                    },
                    now,
                );
            }
            IngressOut::Control { conn, op } => match op {
                ControlOp::Status => {
                    let status = self.project(now).status;
                    self.ingress_w
                        .ingress
                        .reply_control(conn, &ControlResponse::status(status));
                }
                ControlOp::Quit => {
                    self.ingress_w
                        .ingress
                        .reply_control(conn, &ControlResponse::ok());
                    self.log("quit asked for over the pipe".into());
                    self.push_event(HubEvent::Quit);
                }
            },
        }
    }

    /// What the hub knows about where a frame came from: the folder it ran
    /// in, whose that folder is (as of the process's start), and Claude
    /// Code's own pid (the frame's `pid`, never the hook's `hook_pid`).
    pub(crate) fn ingest_context(
        &self,
        transcript_path: Option<&str>,
        config_dir_env: Option<&str>,
        pid: Option<u32>,
    ) -> IngestContext {
        let folder = AccountId::new(self.session_folder(transcript_path, config_dir_env));
        let pid_started = pid.and_then(|pid| self.platform.processes.start_time(pid));
        IngestContext {
            attribution: self.registry.attribution(&folder, pid_started),
            account: Some(folder),
            trusted_pid: pid,
            pid_started,
        }
    }

    /// The config folder a session runs in: its transcript's, else
    /// `CLAUDE_CONFIG_DIR`, else `~\.claude` (shared-history folders are
    /// never one).
    fn session_folder(
        &self,
        transcript_path: Option<&str>,
        config_dir_env: Option<&str>,
    ) -> String {
        let paths = self.registry.paths();
        let infrastructure = self.registry.infrastructure_dirs();
        paths.session_config_dir(transcript_path, config_dir_env, |dir| {
            infrastructure.iter().any(|infra| paths.same(infra, dir))
        })
    }

    /// Whether a request of this session's account is released at once
    /// (the Mac's `passesThrough`).
    fn passes_through(&self, event: &HookEvent) -> bool {
        let ctx = self.ingest_context(
            event.transcript_path.as_deref(),
            event.config_dir_env.as_deref(),
            event.pid,
        );
        ctx.account
            .as_ref()
            .is_some_and(|folder| self.registry.is_untracked(folder, ctx.pid_started))
    }

    /// A status line: its account filled in, its rate limits read; the
    /// usage store takes the readings, the session store the context,
    /// model, title and cost, the hooks the Claude Code version.
    fn status_line(&mut self, mut message: StatusLineMessage, now: SystemTime) {
        {
            let paths = self.registry.paths();
            let infrastructure = self.registry.infrastructure_dirs();
            fill_status_line_account(&mut message, paths, |dir| {
                infrastructure.iter().any(|infra| paths.same(infra, dir))
            });
        }
        let (five_hour, seven_day) = parse_status_line_rate_limits(message.rate_limits.as_ref());
        message.five_hour = five_hour;
        message.seven_day = seven_day;
        let mut ctx = self.ingest_context(
            message.transcript_path.as_deref(),
            message.config_dir_env.as_deref(),
            message.pid,
        );
        // The wrapper's `CLAUDE_PID` may be inherited from the Claude Code
        // that started this one: it counts only when the hooks don't name
        // another process for the session.
        if let Some(hooks_pid) = self
            .sessions
            .session(&message.session_id)
            .and_then(|session| session.pid)
        {
            if message.pid != Some(hooks_pid) {
                ctx.trusted_pid = Some(hooks_pid);
                ctx.pid_started = self.platform.processes.start_time(hooks_pid);
            }
        }
        if let Some(observation) = self.usage.ingest_status_line(&message, ctx.clone(), now) {
            self.usage_observed([observation]);
        }
        if let Some(version) = message.claude_code_version.clone() {
            self.note_version_sighting(
                VersionSighting {
                    source: VersionSource::StatusLine,
                    path: None,
                    version: Some(version),
                },
                now,
            );
        }
        self.sessions_input(SessionInput::StatusLine { message, ctx }, now);
    }

    /// `answer {session_id, tool_use_id, answer}` → `{result}`: the
    /// request the user saw, answered exactly once. An id that is no longer
    /// held (answered, released, or never held) is `not_pending` and sends
    /// nothing; an answer that doesn't fit the request (approving a
    /// question) is refused and the request stays held.
    pub(crate) fn answer_call(
        &mut self,
        session: &SessionId,
        tool_use_id: &str,
        answer: Answer,
    ) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        let request = self.sessions.view(session).and_then(|view| {
            view.pending
                .into_iter()
                .find(|request| request.tool_use_id == tool_use_id)
        });
        let Some(request) =
            request.filter(|_| self.ingress_w.ingress.is_pending(session, tool_use_id))
        else {
            return to_value(&AnswerReply {
                result: AnswerResult::NotPending,
            });
        };
        let response = permission_response(&request, &answer).map_err(CallError::invalid)?;
        let result = self
            .ingress_w
            .ingress
            .answer(session, tool_use_id, response);
        // After the write, through the pipeline: the answer can't overtake
        // a PostToolUse that came in before it.
        match result {
            AnswerResult::Delivered => self.sessions_input(
                SessionInput::PermissionResolved {
                    session: session.clone(),
                    tool_use_id: tool_use_id.to_owned(),
                    answer,
                },
                now,
            ),
            // The hook is gone: the request can't be answered any more.
            AnswerResult::PeerGone => self.sessions_input(
                SessionInput::PermissionFailed {
                    session: session.clone(),
                    tool_use_id: tool_use_id.to_owned(),
                },
                now,
            ),
            AnswerResult::NotPending => {}
        }
        to_value(&AnswerReply { result })
    }

    /// Held requests the session store says are over (a Stop, a new
    /// prompt, the session gone…): closed with no answer.
    pub(crate) fn release_held(&mut self, release: Release) {
        self.ingress_w.ingress.release(release);
    }

    /// `control status`'s `held`.
    pub(crate) fn held_count(&self) -> u32 {
        u32::try_from(self.ingress_w.ingress.held_count()).unwrap_or(u32::MAX)
    }
}
