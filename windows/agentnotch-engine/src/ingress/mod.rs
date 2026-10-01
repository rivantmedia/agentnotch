//! Hook ingress (HS§4, DESIGN-WIN §1.4 "Server dispatch"; the Mac's
//! `HookSocketServer.finishMessage` and its ToolUseIdCache): what `an-core`
//! does with every [`TransportEvent`] of the hook pipe.
//!
//! - Frames are decoded leniently ([`decode`]); an unreadable one, a status
//!   line, an event of an ignored session (unattended, or an `sdk*`
//!   entrypoint) and every event that expects no answer have their
//!   connection closed at once, in arrival order.
//! - PreToolUse ids go into the [`ToolUseIdCache`]; a PermissionRequest,
//!   which carries none, takes the id of its call (exact match, else the
//!   only call of that tool in flight, else a synthetic `permission-<UUID>`)
//!   and its connection is held until [`HookIngress::answer`] or a release.
//! - A PostToolUse/PostToolUseFailure/PermissionDenied for a held id means
//!   the terminal (or a rule) answered: the connection is closed without a
//!   decision. The main session's Stop/StopFailure closes the main agent's
//!   held requests (background subagents keep theirs); SessionEnd closes
//!   them all.
//! - A held request whose hook went away ([`TransportEvent::PeerClosed`])
//!   becomes [`IngressOut::PermissionFailed`]: the Mac's liveness timer,
//!   without the polling.
//!
//! Closing a connection without a frame is always "no decision": the hook
//! prints nothing and Claude Code's own terminal prompt decides.

mod cache;
mod decode;

pub use cache::{canonical, ToolUseIdCache, MAX_AGE as TOOL_USE_ID_MAX_AGE};
pub use decode::{decode, hook_event, status_line, Decoded};

use crate::core::paths::Paths;
use crate::model::{AccountId, HeldPermission, HookEvent, SessionId, StatusLineMessage};
use crate::platform::{ConnId, HookTransport, IncomingFrame, TransportEvent};
use crate::runtime_types::{AnswerResult, IngressConfig, IngressOut, Release};
use agentnotch_proto::{ControlResponse, PermissionResponse};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

/// Whether a PermissionRequest is released at once, with no decision: its
/// session belongs to an untracked or forgotten account (the Mac's
/// `passesThrough`). The hub supplies it: it holds the account registry.
pub type PassThrough = Box<dyn Fn(&HookEvent) -> bool + Send>;

/// One held PermissionRequest.
#[derive(Debug, Clone)]
struct Held {
    conn: ConnId,
    session: SessionId,
    /// The subagent that asked; `None` for the main session.
    agent_id: Option<String>,
    received_at: SystemTime,
}

pub struct HookIngress {
    cfg: IngressConfig,
    transport: Option<Arc<dyn HookTransport>>,
    cache: ToolUseIdCache,
    /// Held requests by `tool_use_id` (one per id: a newer request with the
    /// same id replaces a stale one, whose connection is closed).
    held: BTreeMap<String, Held>,
    pass_through: Option<PassThrough>,
}

impl HookIngress {
    pub fn new(cfg: IngressConfig) -> Self {
        let cache = ToolUseIdCache::new(cfg.tool_use_cache_ttl);
        HookIngress {
            cfg,
            transport: None,
            cache,
            held: BTreeMap::new(),
            pass_through: None,
        }
    }

    /// An ingress that closes and answers connections through `transport`.
    pub fn with_transport(cfg: IngressConfig, transport: Arc<dyn HookTransport>) -> Self {
        let mut ingress = HookIngress::new(cfg);
        ingress.attach(transport);
        ingress
    }

    /// The transport connections are closed and answered through. Without
    /// one, nothing is closed and every answer is [`AnswerResult::PeerGone`].
    pub fn attach(&mut self, transport: Arc<dyn HookTransport>) {
        self.transport = Some(transport);
    }

    /// Installs the untracked-account rule (see [`PassThrough`]).
    pub fn set_pass_through(&mut self, rule: PassThrough) {
        self.pass_through = Some(rule);
    }

    pub fn config(&self) -> &IngressConfig {
        &self.cfg
    }

    /// Starts the transport on the configured pipe name; its events go to
    /// `sink`, which `an-core` feeds back into [`HookIngress::on_transport`].
    pub fn start(&self, sink: crossbeam_channel::Sender<TransportEvent>) -> Result<(), String> {
        match &self.transport {
            Some(transport) => transport.start(&self.cfg.pipe_name, sink),
            None => Err("no hook transport".into()),
        }
    }

    /// Releases every held request and stops the transport (app quit, hub
    /// stop, the updater's exit): each waiting hook exits with no output.
    pub fn stop(&mut self) {
        self.release(Release::All);
        if let Some(transport) = &self.transport {
            transport.stop();
        }
    }

    /// One transport event, in arrival order. Every time the rules use is
    /// the frame's own read time, so `now` only matters for events without
    /// one.
    pub fn on_transport(&mut self, ev: TransportEvent, now: SystemTime) -> Vec<IngressOut> {
        let _ = now;
        match ev {
            TransportEvent::Listening(name) => vec![IngressOut::TransportStatus(Ok(name))],
            TransportEvent::Error(why) => vec![IngressOut::TransportStatus(Err(why))],
            TransportEvent::PeerClosed(conn) => self.peer_closed(conn),
            TransportEvent::Frame(frame) => self.frame(frame),
        }
    }

    /// Writes the answer to a held request and closes it. `NotPending` when
    /// no request with this id is held for this session (answered or
    /// released already): the UI's answer then does nothing.
    pub fn answer(
        &mut self,
        session: &SessionId,
        tool_use_id: &str,
        r: PermissionResponse,
    ) -> AnswerResult {
        if !self.is_pending(session, tool_use_id) {
            return AnswerResult::NotPending;
        }
        let Some(held) = self.held.remove(tool_use_id) else {
            return AnswerResult::NotPending;
        };
        let delivered = self
            .transport
            .as_ref()
            .is_some_and(|t| t.respond(held.conn, r.to_json()));
        if delivered {
            AnswerResult::Delivered
        } else {
            AnswerResult::PeerGone
        }
    }

    /// Answers a control request (`control status` / `quit`).
    pub fn reply_control(&self, conn: ConnId, response: &ControlResponse) -> bool {
        self.transport
            .as_ref()
            .is_some_and(|t| t.respond(conn, response.to_json()))
    }

    /// Closes held requests without answering.
    pub fn release(&mut self, which: Release) {
        let ids: Vec<String> = self
            .held
            .iter()
            .filter(|(id, held)| match &which {
                Release::Request {
                    session,
                    tool_use_id,
                } => *id == tool_use_id && held.session == *session,
                Release::MainAgent(session) => held.session == *session && held.agent_id.is_none(),
                Release::Agent { session, agent_id } => {
                    held.session == *session && held.agent_id.as_deref() == Some(agent_id.as_str())
                }
                Release::Session(session) => held.session == *session,
                Release::All => true,
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.close_held(&id);
        }
    }

    /// Held requests, oldest first.
    pub fn pending(&self) -> Vec<(SessionId, String)> {
        let mut pending: Vec<(&String, &Held)> = self.held.iter().collect();
        pending.sort_by(|a, b| a.1.received_at.cmp(&b.1.received_at).then(a.0.cmp(b.0)));
        pending
            .into_iter()
            .map(|(id, held)| (held.session.clone(), id.clone()))
            .collect()
    }

    /// Whether a request with this id is held for this session.
    pub fn is_pending(&self, session: &SessionId, tool_use_id: &str) -> bool {
        self.held
            .get(tool_use_id)
            .is_some_and(|held| held.session == *session)
    }

    /// How many requests are held (`control status`'s `held`).
    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    // ---- dispatch ----

    fn frame(&mut self, frame: IncomingFrame) -> Vec<IngressOut> {
        let conn = frame.conn;
        match decode(&frame.bytes, frame.received_at) {
            Decoded::Unreadable(_) => {
                self.close(conn);
                Vec::new()
            }
            Decoded::StatusLine(message) => {
                self.close(conn);
                vec![IngressOut::StatusLine(*message)]
            }
            // Held open: the hub answers it with `reply_control`.
            Decoded::Control(op) => vec![IngressOut::Control { conn, op }],
            Decoded::Hook(event) => self.hook(conn, *event),
        }
    }

    fn hook(&mut self, conn: ConnId, mut event: HookEvent) -> Vec<IngressOut> {
        if self.is_ignored(&event) {
            // Background, daemon and SDK sessions: no session, no held pipe.
            self.close(conn);
            return Vec::new();
        }
        self.update_cache(&event);

        if !event.expects_response() {
            self.close(conn);
            return vec![IngressOut::Hook(event)];
        }

        // A session of an untracked or forgotten account: nothing shows it,
        // so nothing may hold its request. Released at once with no
        // decision; the session asks in its own terminal.
        if self.pass_through.as_ref().is_some_and(|rule| rule(&event)) {
            self.close(conn);
            return Vec::new();
        }

        // Above the cap a request fails open, as the pipe server's own cap
        // does: the hook exits with no output.
        if self.held.len() >= self.cfg.max_connections {
            self.close(conn);
            return Vec::new();
        }

        let now = event.received_at;
        let session = event.session_id.as_str().to_owned();
        let agent = if event.is_subagent_event() {
            event.agent_id.clone()
        } else {
            None
        };
        if event.tool_use_id.is_none() {
            let tool = event.tool.as_deref();
            if let Some(exact) = self
                .cache
                .pop(&session, tool, event.tool_input.as_ref(), now)
            {
                event.tool_use_id = Some(exact);
            } else if let Some(only) =
                self.cache
                    .pop_only_in_flight(&session, tool, agent.as_deref(), now)
            {
                // Another hook rewrote the input, but exactly one call of
                // this tool is in flight: it is that one, and its own
                // PostToolUse will resolve the request.
                event.tool_use_id = Some(only);
            } else {
                // None or several candidates: still answerable through this
                // connection.
                event.tool_use_id = Some(synthetic_tool_use_id());
                event.has_synthetic_tool_use_id = true;
            }
        }
        let tool_use_id = event.tool_use_id.clone().unwrap_or_default();
        self.close_held(&tool_use_id);
        self.held.insert(
            tool_use_id.clone(),
            Held {
                conn,
                session: event.session_id.clone(),
                agent_id: agent.clone(),
                received_at: event.received_at,
            },
        );
        vec![IngressOut::PermissionHeld(HeldPermission {
            conn,
            session_id: event.session_id.clone(),
            tool_use_id,
            has_synthetic_tool_use_id: event.has_synthetic_tool_use_id,
            agent_id: agent,
            received_at: event.received_at,
            event,
        })]
    }

    fn is_ignored(&self, event: &HookEvent) -> bool {
        if event.attended == Some(false) {
            return true;
        }
        self.cfg.ignore_sdk_entrypoints && event.is_from_ignored_session()
    }

    /// Cache hygiene: ids are recorded on PreToolUse and dropped as soon as
    /// the tool resolved, so an auto-allowed call can't leave an id behind
    /// for a later identical call to pop.
    fn update_cache(&mut self, event: &HookEvent) {
        let session = event.session_id.as_str();
        match event.event.as_str() {
            "PreToolUse" => {
                if let Some(id) = &event.tool_use_id {
                    let agent = if event.is_subagent_event() {
                        event.agent_id.as_deref()
                    } else {
                        None
                    };
                    self.cache.record(
                        session,
                        event.tool.as_deref(),
                        event.tool_input.as_ref(),
                        id,
                        agent,
                        event.received_at,
                    );
                }
            }
            "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
                if let Some(id) = &event.tool_use_id {
                    self.cache.remove(id);
                    // Resolved in the terminal (or by a rule): nothing left
                    // to answer.
                    self.close_held(id);
                }
            }
            "Stop" | "StopFailure" => {
                // The main turn ended. Background subagents keep running
                // (and keep waiting for their answers), so only the main
                // session's calls and requests are over.
                if event.is_subagent_event() {
                    return;
                }
                self.cache.remove_session(session, true);
                self.release(Release::MainAgent(event.session_id.clone()));
            }
            "SessionEnd" => {
                self.cache.remove_session(session, false);
                self.release(Release::Session(event.session_id.clone()));
            }
            _ => {}
        }
    }

    fn peer_closed(&mut self, conn: ConnId) -> Vec<IngressOut> {
        let Some(id) = self
            .held
            .iter()
            .find(|(_, held)| held.conn == conn)
            .map(|(id, _)| id.clone())
        else {
            // Never held (fire and forget), or answered already.
            return Vec::new();
        };
        let Some(held) = self.held.remove(&id) else {
            return Vec::new();
        };
        // The hook is gone (Claude Code's own dialog answered first, or it
        // was killed): the UI must stop offering an answer.
        self.close(conn);
        vec![IngressOut::PermissionFailed {
            session: held.session,
            tool_use_id: id,
        }]
    }

    fn close_held(&mut self, tool_use_id: &str) {
        if let Some(held) = self.held.remove(tool_use_id) {
            self.close(held.conn);
        }
    }

    fn close(&self, conn: ConnId) {
        if let Some(transport) = &self.transport {
            transport.close(conn);
        }
    }
}

/// `permission-<UUID>` in the Mac's form (upper-case, hyphenated).
fn synthetic_tool_use_id() -> String {
    format!(
        "permission-{}",
        uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
    )
}

/// Fills a status line message's `account_id`: the folder its transcript
/// path names (unless that runs through shared-history infrastructure), else
/// its raw `CLAUDE_CONFIG_DIR`, else `~\.claude` (the Mac's
/// `SessionFilter.accountId`). The hub calls it: it holds the paths and the
/// account classifier.
pub fn fill_status_line_account(
    message: &mut StatusLineMessage,
    paths: &Paths,
    is_infrastructure: impl Fn(&str) -> bool,
) {
    let folder = paths.session_config_dir(
        message.transcript_path.as_deref(),
        message.config_dir_env.as_deref(),
        is_infrastructure,
    );
    message.account_id = Some(AccountId::new(folder));
}
