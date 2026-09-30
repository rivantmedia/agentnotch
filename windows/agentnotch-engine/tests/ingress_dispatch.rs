//! What ingress does with each frame (HS§4.3–4.4, DESIGN-WIN §1.4): the
//! Mac's HookSocketIntegrationTests (PermissionRequest round trip, deny and
//! always-allow shapes, a hook that went away, fire and forget, ignored
//! sessions) against the in-memory transport, plus the held-request rules:
//! releases, the cache's resolution events, stale ids, untracked accounts,
//! control requests.

use agentnotch_engine::ingress::HookIngress;
use agentnotch_engine::model::SessionId;
use agentnotch_engine::platform::{ConnId, HookTransport, TransportEvent};
use agentnotch_engine::runtime_types::{AnswerResult, IngressConfig, IngressOut, Release};
use agentnotch_engine::testkit::MemoryTransport;
use agentnotch_proto::{ControlOp, ControlResponse, ControlStatus, PermissionResponse};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const SESSION: &str = "sess-int";

struct Fixture {
    transport: Arc<MemoryTransport>,
    ingress: HookIngress,
    clock: u64,
}

impl Fixture {
    fn new() -> Fixture {
        let transport = Arc::new(MemoryTransport::default());
        let ingress = HookIngress::with_transport(
            IngressConfig::new(r"\\.\pipe\agentnotch-test"),
            transport.clone(),
        );
        Fixture {
            transport,
            ingress,
            clock: 0,
        }
    }

    fn now(&mut self) -> SystemTime {
        self.clock += 1;
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + self.clock)
    }

    /// Sends one frame as a new connection; its id and what ingress made of it.
    fn send(&mut self, frame: Value) -> (ConnId, Vec<IngressOut>) {
        let now = self.now();
        let (conn, event) = self.transport.frame(&frame, now);
        (conn, self.ingress.on_transport(event, now))
    }

    fn send_raw(&mut self, bytes: &[u8]) -> (ConnId, Vec<IngressOut>) {
        let now = self.now();
        let (conn, event) = self.transport.frame(&json!(null), now);
        let TransportEvent::Frame(mut frame) = event else {
            unreachable!()
        };
        frame.bytes = bytes.to_vec();
        (
            conn,
            self.ingress.on_transport(TransportEvent::Frame(frame), now),
        )
    }
}

/// A hook message as the hook exe sends it (the Mac script's base fields).
fn message(event: &str, extra: Value) -> Value {
    let mut base = json!({
        "protocol": 1,
        "event": event,
        "session_id": SESSION,
        "transcript_path": r"C:\Users\me\.claude\projects\C--tmp\sess-int.jsonl",
        "cwd": r"C:\tmp",
        "pid": 4242,
        "status": "unknown",
        "config_dir_env": null,
        "attended": true,
        "entrypoint": "cli",
        "agent_id": null,
        "agent_type": null,
        "permission_mode": "default",
        "hook_pid": 5555,
        "terminal": {"wt_session": null, "term_program": null},
    });
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

fn held(outs: &[IngressOut]) -> &agentnotch_engine::model::HeldPermission {
    match outs {
        [IngressOut::PermissionHeld(held)] => held,
        other => panic!("expected one held request, got {other:?}"),
    }
}

fn session() -> SessionId {
    SessionId::new(SESSION)
}

fn response_json(transport: &MemoryTransport, conn: ConnId) -> Value {
    serde_json::from_slice(&transport.response(conn).expect("an answer was written")).unwrap()
}

/// HookSocketIntegrationTests.answersAskUserQuestionThroughTheHook.
#[test]
fn answers_ask_user_question() {
    let mut f = Fixture::new();
    let input = json!({"questions": [{"question": "Which DB?", "header": "DB", "multiSelect": false,
        "options": [{"label": "Postgres", "description": "Relational"}, {"label": "SQLite", "description": "File"}]}]});
    let (pre_conn, outs) = f.send(message(
        "PreToolUse",
        json!({"tool": "AskUserQuestion", "tool_input": input, "tool_use_id": "toolu_ask"}),
    ));
    assert!(matches!(&outs[..], [IngressOut::Hook(e)] if e.event == "PreToolUse"));
    assert!(
        f.transport.is_closed(pre_conn),
        "fire and forget is closed at once"
    );

    let (conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "AskUserQuestion", "tool_input": input, "permission_suggestions": []}),
    ));
    let request = held(&outs);
    assert_eq!(request.tool_use_id, "toolu_ask");
    assert!(!request.has_synthetic_tool_use_id);
    assert_eq!(request.event.tool_use_id.as_deref(), Some("toolu_ask"));
    assert_eq!(request.conn, conn);
    assert_eq!(request.agent_id, None);
    assert!(!f.transport.is_closed(conn), "held open until answered");
    assert!(f.ingress.is_pending(&session(), "toolu_ask"));

    let mut answer = PermissionResponse::allow();
    answer.updated_input = json!({"answers": {"Which DB?": "Postgres"}})
        .as_object()
        .cloned();
    assert_eq!(
        f.ingress.answer(&session(), "toolu_ask", answer),
        AnswerResult::Delivered
    );
    let written = response_json(&f.transport, conn);
    assert_eq!(written["decision"], "allow");
    assert_eq!(written["updated_input"]["answers"]["Which DB?"], "Postgres");
    assert!(!f.ingress.is_pending(&session(), "toolu_ask"));
    // One answer per request.
    assert_eq!(
        f.ingress
            .answer(&session(), "toolu_ask", PermissionResponse::allow()),
        AnswerResult::NotPending
    );
}

/// HookSocketIntegrationTests.denyAndAlwaysAllowShapes.
#[test]
fn deny_and_always_allow_shapes() {
    let mut f = Fixture::new();
    let suggestion = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "rm -rf build"}], "behavior": "allow", "destination": "session"});
    // No PreToolUse was seen: ingress makes up an id and still holds the request.
    let (conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "rm -rf build"}, "permission_suggestions": [suggestion]}),
    ));
    let request = held(&outs).clone();
    assert!(request.has_synthetic_tool_use_id);
    let id = request.tool_use_id.clone();
    let uuid = id
        .strip_prefix("permission-")
        .expect("the Mac's synthetic form");
    assert_eq!(uuid.len(), 36);
    assert_eq!(uuid, uuid.to_uppercase());

    let mut always = PermissionResponse::allow();
    always.updated_permissions = request
        .event
        .permission_suggestions
        .as_ref()
        .map(|s| s[..1].to_vec());
    assert_eq!(
        f.ingress.answer(&session(), &id, always),
        AnswerResult::Delivered
    );
    let written = response_json(&f.transport, conn);
    assert_eq!(written["decision"], "allow");
    assert!(written.get("updated_input").is_none());
    assert_eq!(written["updated_permissions"][0]["destination"], "session");

    // Deny.
    let (deny_conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "git push --force"}}),
    ));
    let pending_id = held(&outs).tool_use_id.clone();
    assert_eq!(f.ingress.pending(), vec![(session(), pending_id.clone())]);
    assert_eq!(
        f.ingress.answer(
            &session(),
            &pending_id,
            PermissionResponse::deny(Some("Not on main".into()))
        ),
        AnswerResult::Delivered
    );
    let written = response_json(&f.transport, deny_conn);
    assert_eq!(written["decision"], "deny");
    assert_eq!(written["reason"], "Not on main");
}

/// HookSocketIntegrationTests.detectsAHookThatWentAway.
#[test]
fn detects_a_hook_that_went_away() {
    let mut f = Fixture::new();
    let (conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "make"}}),
    ));
    let id = held(&outs).tool_use_id.clone();

    // Claude Code kills the hook when the terminal dialog answers first.
    f.transport.mark_gone(conn);
    let now = f.now();
    let outs = f
        .ingress
        .on_transport(TransportEvent::PeerClosed(conn), now);
    assert_eq!(
        outs,
        vec![IngressOut::PermissionFailed {
            session: session(),
            tool_use_id: id.clone()
        }]
    );
    assert!(!f.ingress.is_pending(&session(), &id));
    assert_eq!(
        f.ingress
            .answer(&session(), &id, PermissionResponse::allow()),
        AnswerResult::NotPending
    );
    // A connection that was never held going away is nothing.
    let now = f.now();
    assert!(f
        .ingress
        .on_transport(TransportEvent::PeerClosed(999), now)
        .is_empty());
}

#[test]
fn an_answer_to_a_vanished_hook_is_peer_gone() {
    let mut f = Fixture::new();
    let (conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "x"}}),
    ));
    let id = held(&outs).tool_use_id.clone();
    // Gone between the engine's last look and the answer.
    f.transport.mark_gone(conn);
    assert_eq!(
        f.ingress
            .answer(&session(), &id, PermissionResponse::allow()),
        AnswerResult::PeerGone
    );
    assert!(f.ingress.pending().is_empty());
}

#[test]
fn an_answer_for_another_session_is_refused() {
    let mut f = Fixture::new();
    let (conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "x"}, "tool_use_id": "toolu_x"}),
    ));
    assert_eq!(held(&outs).tool_use_id, "toolu_x");
    assert_eq!(
        f.ingress.answer(
            &SessionId::new("other"),
            "toolu_x",
            PermissionResponse::allow()
        ),
        AnswerResult::NotPending
    );
    assert!(f.ingress.is_pending(&session(), "toolu_x"));
    assert!(f.transport.response(conn).is_none());
}

/// HookSocketIntegrationTests.fireAndForgetEventsAndIgnoredSessions.
#[test]
fn fire_and_forget_events_and_ignored_sessions() {
    let mut f = Fixture::new();
    let (conn, outs) = f.send(message(
        "Stop",
        json!({"status": "waiting_for_input", "last_assistant_message": "y".repeat(1500), "background_task_count": 2}),
    ));
    match &outs[..] {
        [IngressOut::Hook(event)] => {
            assert_eq!(
                event.last_assistant_message.as_ref().map(|s| s.len()),
                Some(1500)
            );
            assert_eq!(event.background_task_count, Some(2));
            assert_eq!(event.pid, Some(4242));
            assert_eq!(event.attended, Some(true));
            assert_eq!(event.entrypoint.as_deref(), Some("cli"));
        }
        other => panic!("{other:?}"),
    }
    assert!(f.transport.is_closed(conn));

    for (attended, entrypoint) in [
        (json!(false), json!("cli")),
        (json!(true), json!("sdk-ts")),
        (json!(null), json!("sdk-py")),
    ] {
        let (conn, outs) = f.send(message(
            "PermissionRequest",
            json!({"attended": attended, "entrypoint": entrypoint, "tool": "Bash", "tool_input": {}}),
        ));
        assert!(outs.is_empty(), "ignored: {attended} {entrypoint}");
        assert!(f.transport.is_closed(conn));
    }
    assert!(f.ingress.pending().is_empty());
}

#[test]
fn sdk_sessions_can_be_kept() {
    let transport = Arc::new(MemoryTransport::default());
    let mut cfg = IngressConfig::new("p");
    cfg.ignore_sdk_entrypoints = false;
    let mut ingress = HookIngress::with_transport(cfg, transport.clone());
    let now = SystemTime::UNIX_EPOCH;
    let (_, frame) = transport.frame(&message("Stop", json!({"entrypoint": "sdk-ts"})), now);
    assert_eq!(ingress.on_transport(frame, now).len(), 1);
    // Unattended sessions are never kept.
    let (_, frame) = transport.frame(&message("Stop", json!({"attended": false})), now);
    assert!(ingress.on_transport(frame, now).is_empty());
}

#[test]
fn status_lines_and_unreadable_frames_are_closed() {
    let mut f = Fixture::new();
    let (conn, outs) = f.send(json!({"protocol": 1, "event": "StatusLine", "session_id": "abc", "pid": 4242, "status_line": {"version": "2.1.282"}}));
    match &outs[..] {
        [IngressOut::StatusLine(message)] => {
            assert_eq!(message.session_id.as_str(), "abc");
            assert_eq!(message.claude_code_version.as_deref(), Some("2.1.282"));
        }
        other => panic!("{other:?}"),
    }
    assert!(f.transport.is_closed(conn));

    for bytes in [&b"garbage"[..], b"{}", b"[]", br#"{"event":"Stop"}"#, b""] {
        let (conn, outs) = f.send_raw(bytes);
        assert!(outs.is_empty());
        assert!(f.transport.is_closed(conn));
    }
}

#[test]
fn post_tool_use_resolves_a_held_request() {
    for resolution in ["PostToolUse", "PostToolUseFailure", "PermissionDenied"] {
        let mut f = Fixture::new();
        f.send(message(
            "PreToolUse",
            json!({"tool": "Bash", "tool_input": {"command": "ls"}, "tool_use_id": "toolu_1"}),
        ));
        let (conn, outs) = f.send(message(
            "PermissionRequest",
            json!({"tool": "Bash", "tool_input": {"command": "ls"}}),
        ));
        assert_eq!(held(&outs).tool_use_id, "toolu_1");
        // Answered in the terminal (or by a rule): closed with no decision.
        let (_, outs) = f.send(message(
            resolution,
            json!({"tool": "Bash", "tool_input": {"command": "ls"}, "tool_use_id": "toolu_1"}),
        ));
        assert!(matches!(&outs[..], [IngressOut::Hook(e)] if e.event == resolution));
        assert!(f.transport.is_closed(conn), "{resolution}");
        assert!(f.ingress.pending().is_empty());
    }
}

#[test]
fn an_auto_allowed_call_never_lends_its_id() {
    let mut f = Fixture::new();
    let input = json!({"command": "npm test"});
    // Allowed by a rule: PreToolUse then PostToolUse, no PermissionRequest.
    f.send(message(
        "PreToolUse",
        json!({"tool": "Bash", "tool_input": input, "tool_use_id": "toolu_auto"}),
    ));
    f.send(message(
        "PostToolUse",
        json!({"tool": "Bash", "tool_input": input, "tool_use_id": "toolu_auto"}),
    ));
    // A later identical call that does ask.
    f.send(message(
        "PreToolUse",
        json!({"tool": "Bash", "tool_input": input, "tool_use_id": "toolu_asks"}),
    ));
    let (_, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": input}),
    ));
    assert_eq!(held(&outs).tool_use_id, "toolu_asks");
}

#[test]
fn a_rewritten_input_matches_the_only_call_in_flight() {
    let mut f = Fixture::new();
    f.send(message(
        "PreToolUse",
        json!({"tool": "Bash", "tool_input": {"command": "make"}, "tool_use_id": "toolu_make"}),
    ));
    // Another hook rewrote the input before the PermissionRequest.
    let (_, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "make -j8"}}),
    ));
    let request = held(&outs);
    assert_eq!(request.tool_use_id, "toolu_make");
    assert!(!request.has_synthetic_tool_use_id);

    // Two in flight: which one can't be told, so the id is made up.
    f.send(message(
        "PreToolUse",
        json!({"tool": "Bash", "tool_input": {"command": "a"}, "tool_use_id": "toolu_a"}),
    ));
    f.send(message(
        "PreToolUse",
        json!({"tool": "Bash", "tool_input": {"command": "b"}, "tool_use_id": "toolu_b"}),
    ));
    let (_, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "c"}}),
    ));
    assert!(held(&outs).has_synthetic_tool_use_id);
}

#[test]
fn subagent_requests_carry_their_agent_and_survive_the_main_stop() {
    let mut f = Fixture::new();
    let (main_conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {"command": "main"}}),
    ));
    let main_id = held(&outs).tool_use_id.clone();
    f.send(message("PreToolUse", json!({"agent_id": "agent-7", "agent_type": "general", "tool": "Bash", "tool_input": {"command": "sub"}, "tool_use_id": "toolu_sub"})));
    let (sub_conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"agent_id": "agent-7", "tool": "Bash", "tool_input": {"command": "sub"}}),
    ));
    let sub = held(&outs);
    assert_eq!(sub.tool_use_id, "toolu_sub");
    assert_eq!(sub.agent_id.as_deref(), Some("agent-7"));

    // A subagent's Stop is not the main turn's.
    f.send(message("Stop", json!({"agent_id": "agent-7"})));
    assert_eq!(f.ingress.held_count(), 2);

    // The main turn stopped: the main agent's request is over; the background
    // subagent keeps running and keeps waiting for its answer.
    f.send(message("Stop", json!({})));
    assert!(f.transport.is_closed(main_conn));
    assert!(!f.transport.is_closed(sub_conn));
    assert!(!f.ingress.is_pending(&session(), &main_id));
    assert!(f.ingress.is_pending(&session(), "toolu_sub"));

    // The session ended: everything is over.
    f.send(message("SessionEnd", json!({"reason": "exit"})));
    assert!(f.transport.is_closed(sub_conn));
    assert!(f.ingress.pending().is_empty());
}

#[test]
fn a_stale_request_with_the_same_id_is_replaced() {
    let mut f = Fixture::new();
    let (old, _) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {}, "tool_use_id": "toolu_dup"}),
    ));
    let (new, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {}, "tool_use_id": "toolu_dup"}),
    ));
    assert_eq!(held(&outs).conn, new);
    assert!(f.transport.is_closed(old));
    assert_eq!(f.ingress.held_count(), 1);
    assert_eq!(
        f.ingress
            .answer(&session(), "toolu_dup", PermissionResponse::allow()),
        AnswerResult::Delivered
    );
    assert!(f.transport.response(new).is_some());
}

#[test]
fn untracked_accounts_pass_through() {
    let mut f = Fixture::new();
    f.ingress.set_pass_through(Box::new(|event| {
        event.config_dir_env.as_deref() == Some(r"C:\Users\me\.claude-untracked")
    }));
    let (conn, outs) = f.send(message(
        "PermissionRequest",
        json!({"config_dir_env": r"C:\Users\me\.claude-untracked", "tool": "Bash", "tool_input": {}}),
    ));
    assert!(outs.is_empty());
    assert!(f.transport.is_closed(conn));
    // Its other events still arrive (the session is shown nowhere, but the
    // hub decides that, not ingress).
    let (_, outs) = f.send(message(
        "Stop",
        json!({"config_dir_env": r"C:\Users\me\.claude-untracked"}),
    ));
    assert_eq!(outs.len(), 1);
    // A tracked one is held.
    let (_, outs) = f.send(message(
        "PermissionRequest",
        json!({"tool": "Bash", "tool_input": {}}),
    ));
    held(&outs);
}

#[test]
fn releases() {
    let mut f = Fixture::new();
    let mut conns = Vec::new();
    for (session_id, agent, id) in [
        ("a", None, "a-main"),
        ("a", Some("x"), "a-x"),
        ("a", Some("y"), "a-y"),
        ("b", None, "b-main"),
        ("c", None, "c-main"),
    ] {
        let mut frame = message(
            "PermissionRequest",
            json!({"tool": "Bash", "tool_input": {}, "tool_use_id": id}),
        );
        frame["session_id"] = json!(session_id);
        frame["agent_id"] = json!(agent);
        let (conn, outs) = f.send(frame);
        held(&outs);
        conns.push(conn);
    }
    let a = SessionId::new("a");
    // The wrong session releases nothing.
    f.ingress.release(Release::Request {
        session: SessionId::new("b"),
        tool_use_id: "a-x".into(),
    });
    assert_eq!(f.ingress.held_count(), 5);
    f.ingress.release(Release::Request {
        session: a.clone(),
        tool_use_id: "a-x".into(),
    });
    assert!(f.transport.is_closed(conns[1]));
    f.ingress.release(Release::Agent {
        session: a.clone(),
        agent_id: "y".into(),
    });
    assert!(f.transport.is_closed(conns[2]));
    assert!(!f.transport.is_closed(conns[0]));
    f.ingress.release(Release::MainAgent(a.clone()));
    assert!(f.transport.is_closed(conns[0]));
    f.ingress.release(Release::Session(SessionId::new("b")));
    assert!(f.transport.is_closed(conns[3]));
    assert_eq!(
        f.ingress.pending(),
        vec![(SessionId::new("c"), "c-main".to_string())]
    );
    f.ingress.release(Release::All);
    assert!(f.transport.is_closed(conns[4]));
    assert_eq!(f.ingress.held_count(), 0);
    // Released requests can't be answered.
    assert_eq!(
        f.ingress
            .answer(&SessionId::new("c"), "c-main", PermissionResponse::allow()),
        AnswerResult::NotPending
    );
}

#[test]
fn pending_is_oldest_first() {
    let mut f = Fixture::new();
    for id in ["zeta", "alpha", "mid"] {
        f.send(message(
            "PermissionRequest",
            json!({"tool": "Bash", "tool_input": {}, "tool_use_id": id}),
        ));
    }
    let ids: Vec<String> = f.ingress.pending().into_iter().map(|(_, id)| id).collect();
    assert_eq!(ids, ["zeta", "alpha", "mid"]);
}

#[test]
fn the_cap_fails_open() {
    let transport = Arc::new(MemoryTransport::default());
    let mut cfg = IngressConfig::new("p");
    cfg.max_connections = 2;
    let mut ingress = HookIngress::with_transport(cfg, transport.clone());
    let now = SystemTime::UNIX_EPOCH;
    let mut last = 0;
    for id in ["1", "2", "3"] {
        let (conn, frame) = transport.frame(
            &message(
                "PermissionRequest",
                json!({"tool": "Bash", "tool_input": {}, "tool_use_id": id}),
            ),
            now,
        );
        ingress.on_transport(frame, now);
        last = conn;
    }
    assert_eq!(ingress.held_count(), 2);
    assert!(transport.is_closed(last));
}

#[test]
fn control_requests_are_held_for_the_hub() {
    let mut f = Fixture::new();
    let (conn, outs) = f.send(json!({"protocol": 1, "event": "AgentNotchControl", "op": "status"}));
    assert_eq!(
        outs,
        vec![IngressOut::Control {
            conn,
            op: ControlOp::Status
        }]
    );
    assert!(!f.transport.is_closed(conn));
    let status = ControlStatus {
        version: "1.1.0".into(),
        sessions: 3,
        ..ControlStatus::default()
    };
    assert!(f
        .ingress
        .reply_control(conn, &ControlResponse::status(status)));
    let written = response_json(&f.transport, conn);
    assert_eq!(written["ok"], true);
    assert_eq!(written["status"]["sessions"], 3);
    // Control requests are never permission requests.
    assert_eq!(f.ingress.held_count(), 0);
}

#[test]
fn transport_status_and_lifecycle() {
    let transport = Arc::new(MemoryTransport::default());
    let mut ingress =
        HookIngress::with_transport(IngressConfig::new(r"\\.\pipe\x"), transport.clone());
    let (sink, events) = crossbeam_channel::unbounded();
    ingress.start(sink).unwrap();
    assert_eq!(transport.pipe_name().as_deref(), Some(r"\\.\pipe\x"));
    let listening = events.try_recv().unwrap();
    let now = SystemTime::UNIX_EPOCH;
    assert_eq!(
        ingress.on_transport(listening, now),
        vec![IngressOut::TransportStatus(Ok(r"\\.\pipe\x".into()))]
    );
    assert_eq!(
        ingress.on_transport(
            TransportEvent::Error("The hook pipe is in use by another program".into()),
            now
        ),
        vec![IngressOut::TransportStatus(Err(
            "The hook pipe is in use by another program".into()
        ))]
    );

    // Frames injected through the transport arrive on the sink in order.
    let first = transport.inject(
        serde_json::to_vec(&message("PreToolUse", json!({"tool_use_id": "t"}))).unwrap(),
        now,
    );
    let second = transport.inject(
        serde_json::to_vec(&message("PermissionRequest", json!({"tool_use_id": "t2"}))).unwrap(),
        now,
    );
    let mut outs = Vec::new();
    while let Ok(event) = events.try_recv() {
        outs.extend(ingress.on_transport(event, now));
    }
    assert!(
        matches!(&outs[..], [IngressOut::Hook(_), IngressOut::PermissionHeld(h)] if h.conn == second)
    );
    assert!(transport.is_closed(first));

    // Stopping releases what is held and stops the transport.
    ingress.stop();
    assert!(transport.is_closed(second));
    assert!(transport.is_stopped());
}

#[test]
fn without_a_transport_answers_are_peer_gone() {
    let mut ingress = HookIngress::new(IngressConfig::new("p"));
    let now = SystemTime::UNIX_EPOCH;
    let probe = MemoryTransport::default();
    let (_, frame) = probe.frame(
        &message("PermissionRequest", json!({"tool_use_id": "t"})),
        now,
    );
    ingress.on_transport(frame, now);
    assert_eq!(
        ingress.answer(&session(), "t", PermissionResponse::allow()),
        AnswerResult::PeerGone
    );
    assert!(ingress.start(crossbeam_channel::unbounded().0).is_err());
    // The memory transport really is a HookTransport the ingress can take.
    let _: &dyn HookTransport = &probe;
}
