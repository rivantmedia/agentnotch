//! Held requests, sessions, review and chat through the live hub (design
//! §1.4, §4.4, §4.5, §4.7; HS§4, HS§6): a hook frame on the pipe
//! (`MemoryTransport`) becomes a session row that needs you; `Call::Answer`
//! writes exactly HS§1.7's response frame to the connection that asked, once;
//! every held request is released (closed with no frame) at a stop, at the
//! updater's exit, when its process is gone and when the hooks are turned
//! off; control frames are answered; review marks, the session state text
//! and the chat go through the store.
//!
//! The clock is the testkit's fake one, so the store's deadlines come only
//! when a test moves it.

mod accounts_support;
mod hub_support;

use accounts_support::{Home, BIIOS, BIIOS_UUID, PARAS, PARAS_UUID};
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, HubEvent, RevealKind};
use agentnotch_engine::model::{Answer, HubSnapshot, SessionRow};
use agentnotch_engine::platform::{Clock, ConnId, Platform};
use agentnotch_engine::runtime_types::Input;
use agentnotch_proto::{ControlResponse, KEEP_PLANNING_REASON};
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Claude Code's pid in every frame (the hook's own is another).
const PID: u32 = 4242;

struct World {
    home: Home,
    hub: TestHub,
}

/// A home signed in as one account (`~\.claude`), its hub started with
/// Claude Code's process running.
fn world() -> World {
    world_with(|_| {})
}

/// [`world`], with more written into the home before the hub starts.
fn world_with(build: impl FnOnce(&Home)) -> World {
    world_over(build, |_| {})
}

/// [`world_with`], with platform services replaced.
fn world_over(build: impl FnOnce(&Home), customise: impl FnOnce(&mut Platform)) -> World {
    let home = Home::new();
    home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
    home.mkdir(".claude/sessions");
    home.mkdir(".claude/projects");
    build(&home);
    let base: PathBuf = home
        .roots
        .home
        .parent()
        .expect("the test's root")
        .to_path_buf();
    let hub = TestHub::over(&base, RuntimeOptions::default(), |_| {}, customise);
    let started = hub.handles.clock.now() - Duration::from_secs(60);
    hub.handles.processes.add(PID, 1, "claude.exe", started);
    hub.hub.start().expect("the hub starts");
    assert!(
        eventually(|| hub.logs().iter().any(|l| l == "pipe listening")),
        "{:?}",
        hub.logs()
    );
    World { home, hub }
}

impl World {
    fn transcript(&self, session: &str) -> String {
        self.home
            .path(&format!(".claude/projects/proj/{session}.jsonl"))
    }

    /// A hook message as the hook exe sends it.
    fn frame(&self, event: &str, session: &str, extra: Value) -> Vec<u8> {
        let mut message = json!({
            "protocol": 1, "event": event, "session_id": session,
            "cwd": self.home.path("code/proj"),
            "transcript_path": self.transcript(session),
            "pid": PID, "hook_pid": 9_999, "status": "processing",
            "entrypoint": "cli", "permission_mode": "default",
            "terminal": {"wt_session": null, "term_program": null},
        });
        if let (Some(message), Some(extra)) = (message.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                message.insert(key.clone(), value.clone());
            }
        }
        serde_json::to_vec(&message).unwrap()
    }

    fn send(&self, event: &str, session: &str, extra: Value) -> ConnId {
        let bytes = self.frame(event, session, extra);
        let now = self.hub.handles.clock.now();
        self.hub.handles.transport.inject(bytes, now)
    }

    /// A tool call that needs approval: its PreToolUse, then the held
    /// PermissionRequest (same tool and input, no id). The connection.
    fn ask(&self, session: &str, tool: &str, input: Value, suggestions: Value) -> ConnId {
        self.send("UserPromptSubmit", session, json!({"prompt": "go"}));
        self.send(
            "PreToolUse",
            session,
            json!({"tool": tool, "tool_input": input, "tool_use_id": format!("toolu_{session}")}),
        );
        let conn = self.send(
            "PermissionRequest",
            session,
            json!({"tool": tool, "tool_input": input, "permission_suggestions": suggestions,
                   "status": "waiting_for_approval"}),
        );
        assert!(
            eventually(|| self.row(session).is_some_and(|r| r.pending.is_some())),
            "the request of {session} shows: {:?}",
            self.shown().sessions
        );
        conn
    }

    fn shown(&self) -> HubSnapshot {
        serde_json::from_value(self.hub.hub.call(Call::Snapshot).unwrap()).unwrap()
    }

    fn row(&self, session: &str) -> Option<SessionRow> {
        self.shown()
            .sessions
            .into_iter()
            .find(|row| row.session_id == session)
    }

    fn answer(&self, session: &str, answer: Answer) -> Result<Value, String> {
        self.hub
            .hub
            .call(Call::Answer {
                session_id: session.into(),
                tool_use_id: format!("toolu_{session}"),
                answer,
            })
            .map_err(|e| format!("{}: {}", e.code, e.message))
    }

    /// What the connection was answered, as JSON.
    fn response(&self, conn: ConnId) -> Option<Value> {
        self.hub
            .handles
            .transport
            .response(conn)
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
    }

    /// `control status` as last published, after a publish was let out
    /// (the fake clock holds the coalescing gap open until it moves).
    fn status(&self) -> agentnotch_proto::ControlStatus {
        self.hub.handles.clock.advance(Duration::from_millis(60));
        self.hub.sync();
        self.hub.sync();
        self.hub.hub.control_status()
    }

    /// Moves the clock on in small steps and lets `an-core` act on each.
    fn advance(&self, by: Duration) {
        let step = Duration::from_millis(500);
        let mut left = by;
        while !left.is_zero() {
            let now = step.min(left);
            self.hub.handles.clock.advance(now);
            left -= now;
            self.hub.sync();
            self.hub.sync();
        }
    }
}

fn bash() -> Value {
    json!({"command": "npm test"})
}

fn narrow_rule() -> Value {
    json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test"}],
            "behavior": "allow", "destination": "localSettings"}])
}

fn question_input() -> Value {
    json!({"questions": [{"question": "Which database should we use?", "header": "DB",
        "multiSelect": false, "options": [{"label": "Postgres"}, {"label": "SQLite"}]}]})
}

// ---- the contract: frame → row → answer → frame ----

#[test]
fn a_held_request_shows_and_each_answer_is_hs_1_7s_frame() {
    let w = world();
    let cases: Vec<(&str, &str, Value, Value, Answer, Value)> = vec![
        (
            "allow",
            "Bash",
            bash(),
            narrow_rule(),
            Answer::Allow { always: false },
            json!({"decision": "allow"}),
        ),
        (
            "always",
            "Bash",
            bash(),
            narrow_rule(),
            Answer::Allow { always: true },
            json!({"decision": "allow", "updated_permissions": narrow_rule()}),
        ),
        (
            "deny",
            "Bash",
            bash(),
            json!([]),
            Answer::Deny { reason: None },
            json!({"decision": "deny"}),
        ),
        (
            "denywhy",
            "Bash",
            bash(),
            json!([]),
            Answer::Deny {
                reason: Some("not now".into()),
            },
            json!({"decision": "deny", "reason": "not now"}),
        ),
        (
            "question",
            "AskUserQuestion",
            question_input(),
            json!([]),
            Answer::Questions {
                answers: BTreeMap::from([(
                    "Which database should we use?".to_owned(),
                    "Postgres".to_owned(),
                )]),
            },
            json!({"decision": "allow",
                   "updated_input": {"answers": {"Which database should we use?": "Postgres"}}}),
        ),
        (
            "plan",
            "ExitPlanMode",
            json!({"plan": "1. Do it"}),
            json!([]),
            Answer::ApprovePlan,
            json!({"decision": "allow", "updated_input": {}}),
        ),
        (
            "keep",
            "ExitPlanMode",
            json!({"plan": "1. Do it"}),
            json!([]),
            Answer::KeepPlanning,
            json!({"decision": "deny", "reason": KEEP_PLANNING_REASON}),
        ),
    ];
    for (session, tool, input, suggestions, answer, expected) in cases {
        let conn = w.ask(session, tool, input, suggestions);
        let row = w.row(session).unwrap();
        assert_eq!(row.bucket, "needs_you", "{session}: {row:?}");
        let pending = row.pending.as_ref().unwrap();
        assert_eq!(pending.tool_use_id, format!("toolu_{session}"));
        assert_eq!(pending.tool_name, tool);
        let reply = w.answer(session, answer.clone()).unwrap();
        assert_eq!(reply, json!({"result": "delivered"}), "{session}");
        assert_eq!(w.response(conn), Some(expected.clone()), "{session}");
        assert!(!w.hub.handles.transport.is_closed(conn), "{session}");
        assert!(
            eventually(|| w.row(session).is_some_and(|r| r.pending.is_none())),
            "{session}: the answered request is gone"
        );
    }
    assert_eq!(w.hub.handles.transport.responses().len(), 7);
    assert_eq!(w.status().held, 0);
}

/// An answer that doesn't fit the request is refused, and the request
/// stays held for the right one.
#[test]
fn a_question_is_not_approved_without_its_answers() {
    let w = world();
    let conn = w.ask("q", "AskUserQuestion", question_input(), json!([]));
    let error = w.answer("q", Answer::Allow { always: false }).unwrap_err();
    assert!(error.starts_with("invalid"), "{error}");
    assert_eq!(w.response(conn), None);
    assert_eq!(w.status().held, 1);
    assert!(w.row("q").unwrap().pending.is_some());
}

// ---- never answered twice ----

#[test]
fn a_request_is_never_answered_twice() {
    let w = world();
    let conn = w.ask("s1", "Bash", bash(), json!([]));
    assert_eq!(
        w.answer("s1", Answer::Allow { always: false }).unwrap(),
        json!({"result": "delivered"})
    );
    assert_eq!(
        w.answer("s1", Answer::Deny { reason: None }).unwrap(),
        json!({"result": "not_pending"})
    );
    let responses = w.hub.handles.transport.responses();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].0, conn);
    assert_eq!(w.response(conn), Some(json!({"decision": "allow"})));
}

/// A request the terminal answered first (the hook went away) can't be
/// answered from the panel.
#[test]
fn a_hook_that_went_away_is_not_answered() {
    let w = world();
    let conn = w.ask("s1", "Bash", bash(), json!([]));
    w.hub.handles.transport.peer_closed(conn);
    assert!(eventually(|| w.status().held == 0));
    assert!(eventually(|| w
        .row("s1")
        .is_some_and(|r| r.pending.is_none())));
    assert_eq!(
        w.answer("s1", Answer::Allow { always: false }).unwrap(),
        json!({"result": "not_pending"})
    );
    assert!(w.hub.handles.transport.responses().is_empty());
}

/// An id the hub never held is not pending.
#[test]
fn an_unknown_request_is_not_pending() {
    let w = world();
    assert_eq!(
        w.answer("nobody", Answer::Allow { always: false }).unwrap(),
        json!({"result": "not_pending"})
    );
}

// ---- releases ----

/// A stop (quit, the updater's exit, the self-test's end all call it)
/// closes every held connection with no frame: each hook exits with no
/// output and Claude Code's own prompt decides.
#[test]
fn a_stop_releases_every_held_request() {
    let w = world();
    let first = w.ask("s1", "Bash", bash(), json!([]));
    let second = w.ask("s2", "AskUserQuestion", question_input(), json!([]));
    assert_eq!(w.status().held, 2);
    w.hub.hub.stop();
    let transport = &w.hub.handles.transport;
    assert!(transport.is_closed(first));
    assert!(transport.is_closed(second));
    assert!(transport.responses().is_empty());
    assert!(transport.is_stopped());
    // An answer after the stop finds nothing to answer.
    assert_eq!(
        w.answer("s1", Answer::Allow { always: false }).unwrap(),
        json!({"result": "not_pending"})
    );
}

/// The platform's clock, whose next reading on `an-core` once `armed`
/// panics (a bug anywhere in what `an-core` runs).
struct PanickingClock {
    clock: Arc<dyn Clock>,
    armed: Arc<AtomicBool>,
}

impl Clock for PanickingClock {
    fn now(&self) -> SystemTime {
        let on_core = std::thread::current().name() == Some("an-core");
        if on_core && self.armed.swap(false, Ordering::SeqCst) {
            panic!("a bug on an-core");
        }
        self.clock.now()
    }

    fn monotonic(&self) -> Instant {
        self.clock.monotonic()
    }
}

/// A panic on `an-core` ends the engine as a crash ends the Mac app: every
/// held request is closed with no frame (its hook fails open instead of
/// waiting a day), the pipe stops, calls are answered at once, and a stop
/// and a start listen again from the saved files.
#[test]
fn a_panic_on_an_core_releases_every_held_request() {
    let armed = Arc::new(AtomicBool::new(false));
    let bomb = armed.clone();
    let w = world_over(
        |_| {},
        move |platform| {
            platform.clock = Arc::new(PanickingClock {
                clock: platform.clock.clone(),
                armed: bomb,
            });
        },
    );
    let held = w.ask("s1", "Bash", bash(), json!([]));
    armed.store(true, Ordering::SeqCst);
    w.hub.inputs.send(Input::Tick);
    let transport = &w.hub.handles.transport;
    assert!(eventually(|| transport.is_closed(held)));
    assert!(transport.is_stopped());
    assert!(transport.responses().is_empty());
    assert!(w.hub.logs().iter().any(|l| l.contains("internal error")));

    let asked = Instant::now();
    let refused = w.hub.hub.call(Call::Snapshot).unwrap_err();
    assert_eq!(refused.code, "failed");
    assert!(asked.elapsed() < Duration::from_secs(2));

    let stopping = Instant::now();
    w.hub.hub.stop();
    assert!(stopping.elapsed() < Duration::from_secs(5));
    w.hub.hub.start().unwrap();
    assert!(eventually(|| w.status().transport == "listening"));
    let again = w.ask("s2", "Bash", bash(), json!([]));
    assert_eq!(
        w.answer("s2", Answer::Allow { always: false }).unwrap(),
        json!({"result": "delivered"})
    );
    assert_eq!(w.response(again), Some(json!({"decision": "allow"})));
}

/// The updater's exit: the glue stops the hub, then starts nothing; a
/// request held before is released, and one made after a restart is held
/// on the new start.
#[test]
fn an_update_exit_releases_and_a_restart_listens_again() {
    let w = world();
    let held = w.ask("s1", "Bash", bash(), json!([]));
    w.hub.hub.stop();
    assert!(w.hub.handles.transport.is_closed(held));
    w.hub.hub.start().unwrap();
    assert!(eventually(|| w.status().transport == "listening"));
    let again = w.ask("s3", "Bash", bash(), json!([]));
    assert_eq!(
        w.answer("s3", Answer::Allow { always: false }).unwrap(),
        json!({"result": "delivered"})
    );
    assert_eq!(w.response(again), Some(json!({"decision": "allow"})));
}

/// A session whose Claude Code process is gone is dropped at the periodic
/// check, and its held request with it.
#[test]
fn a_session_whose_process_is_gone_releases_its_requests() {
    let w = world();
    let conn = w.ask("s1", "Bash", bash(), json!([]));
    w.hub.handles.processes.remove(PID);
    w.advance(Duration::from_secs(4));
    assert!(eventually(|| w.hub.handles.transport.is_closed(conn)));
    assert!(eventually(|| w.row("s1").is_none()));
    assert_eq!(w.status().held, 0);
    assert_eq!(w.response(conn), None);
}

/// `reveal_target {session_cwd}` names the folder a known session started
/// in (the page's "Show in Explorer"), and nothing for an id it doesn't know.
#[test]
fn a_known_sessions_folder_is_revealed() {
    let w = world();
    w.send("UserPromptSubmit", "s1", json!({"prompt": "go"}));
    assert!(eventually(|| w.row("s1").is_some()));
    let reveal = |id: &str| {
        w.hub.hub.call(Call::RevealTarget {
            kind: RevealKind::SessionCwd,
            id: id.into(),
        })
    };
    assert_eq!(
        reveal("s1").unwrap(),
        json!({ "path": w.home.path("code/proj") })
    );
    assert_eq!(reveal("s2").unwrap_err().code, "not_found");
}

/// The session's end releases its requests (SessionEnd).
#[test]
fn a_session_end_releases_its_requests() {
    let w = world();
    let conn = w.ask("s1", "Bash", bash(), json!([]));
    w.send("SessionEnd", "s1", json!({"reason": "exit"}));
    assert!(eventually(|| w.hub.handles.transport.is_closed(conn)));
    assert_eq!(w.response(conn), None);
}

/// Turning the hooks off lets every waiting hook go.
#[test]
fn turning_the_hooks_off_releases_every_held_request() {
    let w = world();
    let conn = w.ask("s1", "Bash", bash(), json!([]));
    w.hub.hub.call(Call::HooksEnabled { on: false }).unwrap();
    assert!(w.hub.handles.transport.is_closed(conn));
    assert_eq!(w.status().held, 0);
}

// ---- control frames ----

#[test]
fn control_status_and_quit_are_answered() {
    let w = world();
    w.ask("s1", "Bash", bash(), json!([]));
    let transport = &w.hub.handles.transport;
    let now = w.hub.handles.clock.now();
    let status = transport.inject(
        serde_json::to_vec(&json!({"protocol": 1, "event": "AgentNotchControl", "op": "status"}))
            .unwrap(),
        now,
    );
    assert!(eventually(|| transport.response(status).is_some()));
    let answer: ControlResponse =
        serde_json::from_slice(&transport.response(status).unwrap()).unwrap();
    assert!(answer.ok);
    let status = answer.status.unwrap();
    assert_eq!(status.transport, "listening");
    assert_eq!(status.held, 1);
    assert_eq!(status.sessions, 1);
    assert_eq!(status.hook_consent, "unasked");
    assert_eq!(status.version, "1.1.0");

    let quit = transport.inject(
        serde_json::to_vec(&json!({"protocol": 1, "event": "AgentNotchControl", "op": "quit"}))
            .unwrap(),
        now,
    );
    assert!(eventually(|| transport.response(quit).is_some()));
    let answer: Value = serde_json::from_slice(&transport.response(quit).unwrap()).unwrap();
    assert_eq!(answer, json!({"ok": true}));
    assert!(eventually(|| w
        .hub
        .events()
        .iter()
        .any(|(_, e)| matches!(e, HubEvent::Quit))));
}

/// Before the pipe listens, and after a stop, `control status` says so.
#[test]
fn the_pipe_state_is_reported() {
    let w = world();
    assert_eq!(w.status().transport, "listening");
    w.hub
        .handles
        .transport
        .send(agentnotch_engine::platform::TransportEvent::Error(
            "The hook pipe is in use by another program".into(),
        ));
    assert!(eventually(|| w.status().transport == "in_use"));
    assert!(w
        .hub
        .logs()
        .iter()
        .any(|l| l.contains("in use by another program")));
}

// ---- sessions, review, chat ----

/// Integration_EngineTests at hub level: a failed turn is its own state,
/// never "needs you", and sorts after an answerable request.
#[test]
fn a_failed_turn_is_not_needs_you_and_sorts_after_a_question() {
    let w = world();
    w.send("UserPromptSubmit", "failed", json!({"prompt": "go"}));
    w.send(
        "StopFailure",
        "failed",
        json!({"stop_error": "overloaded", "status": "waiting_for_input"}),
    );
    w.ask("asked", "AskUserQuestion", question_input(), json!([]));
    assert!(eventually(|| w.row("failed").is_some_and(|r| r.failed)));
    let shown = w.shown();
    let order: Vec<&str> = shown
        .sessions
        .iter()
        .map(|row| row.session_id.as_str())
        .collect();
    let asked = order.iter().position(|id| *id == "asked").unwrap();
    let failed = order.iter().position(|id| *id == "failed").unwrap();
    assert!(asked < failed, "{order:?}");
    assert_eq!(shown.tray_badge, 1, "only the question needs you");
}

/// Review marks go through the store; the queue survives in
/// `review-state.json`, written at the stop.
#[test]
fn review_marks_reach_the_store_and_the_file() {
    let w = world();
    w.send("UserPromptSubmit", "s1", json!({"prompt": "go"}));
    // The reply comes a moment after the prompt (a prompt marks what came
    // before it reviewed).
    assert!(eventually(|| w.row("s1").is_some()));
    w.advance(Duration::from_secs(1));
    w.send(
        "Stop",
        "s1",
        json!({"status": "waiting_for_input", "last_assistant_message": "All done."}),
    );
    // No registry entry confirms the Stop: the fallback settles it.
    w.advance(Duration::from_secs(95));
    assert!(
        eventually(|| w.row("s1").is_some_and(|r| r.reviewable)),
        "{:?}",
        w.row("s1")
    );
    let text: Value = w.hub.hub.call(Call::SessionStateText).unwrap();
    let text = text["text"].as_str().unwrap().to_owned();
    assert!(text.starts_with("[agentnotch-state] s1 "), "{text}");
    assert!(text.contains("attn=readyForReview"), "{text}");
    assert!(text.contains("review=\"All done.\""), "{text}");

    let now_ms = w.hub.handles.clock.now_ms();
    w.hub
        .hub
        .call(Call::MarkReviewed {
            session_id: "s1".into(),
            at_ms: now_ms,
        })
        .unwrap();
    assert!(eventually(|| w.row("s1").is_some_and(|r| !r.reviewable)));
    w.hub.hub.stop();
    let file: Value = serde_json::from_slice(
        &std::fs::read(w.hub.roots.support.join("review-state.json")).unwrap(),
    )
    .unwrap();
    assert!(file["sessions"]["s1"]["reviewedAt"].is_number(), "{file}");
}

/// A moment from the page later than now is taken as now.
#[test]
fn a_review_mark_from_the_future_is_taken_as_now() {
    let w = world();
    w.send("UserPromptSubmit", "s1", json!({"prompt": "go"}));
    assert!(eventually(|| w.row("s1").is_some()));
    w.advance(Duration::from_secs(1));
    w.send("Stop", "s1", json!({"status": "waiting_for_input"}));
    w.advance(Duration::from_secs(95));
    assert!(eventually(|| w.row("s1").is_some_and(|r| r.reviewable)));
    w.hub
        .hub
        .call(Call::MarkReviewed {
            session_id: "s1".into(),
            at_ms: u64::MAX,
        })
        .unwrap();
    assert!(eventually(|| w.row("s1").is_some_and(|r| !r.reviewable)));
    // A turn finished after the mark is unreviewed again.
    w.advance(Duration::from_secs(2));
    w.send("UserPromptSubmit", "s1", json!({"prompt": "again"}));
    assert!(eventually(|| w
        .row("s1")
        .is_some_and(|r| r.bucket == "working")));
    w.advance(Duration::from_secs(1));
    w.send("Stop", "s1", json!({"status": "waiting_for_input"}));
    w.advance(Duration::from_secs(95));
    assert!(eventually(|| w.row("s1").is_some_and(|r| r.reviewable)));
}

/// Opening a chat reads the transcript and pushes it as a reset.
#[test]
fn opening_a_chat_pushes_its_history() {
    let w = world();
    let path = w.transcript("s1");
    std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    let lines = [
        json!({"type": "user", "uuid": "u1", "timestamp": "2026-09-21T14:13:20Z",
               "message": {"role": "user", "content": "Fix the tests"}}),
        json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-09-21T14:13:25Z",
               "message": {"id": "m1", "role": "assistant", "model": "claude-opus",
                           "content": [{"type": "text", "text": "On it."}]}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, text).unwrap();
    w.send("UserPromptSubmit", "s1", json!({"prompt": "Fix the tests"}));
    assert!(eventually(|| w.row("s1").is_some()));
    w.hub
        .hub
        .call(Call::ChatOpen {
            session_id: "s1".into(),
        })
        .unwrap();
    // A reset first (loading), then the page read.
    let chats = || -> Vec<agentnotch_engine::model::ChatUpdate> {
        w.hub
            .events()
            .into_iter()
            .filter_map(|(_, e)| match e {
                HubEvent::Chat(update) if update.session_id == "s1" => Some(update),
                _ => None,
            })
            .collect()
    };
    assert!(
        eventually(|| {
            let chats = chats();
            chats.first().is_some_and(|first| first.reset)
                && chats.iter().any(|update| {
                    update
                        .order
                        .iter()
                        .map(String::as_str)
                        .eq(["u1-text-0", "a1-text-0"])
                })
        }),
        "{:?}",
        w.hub
            .events()
            .iter()
            .filter(|(_, e)| matches!(e, HubEvent::Chat(_)))
            .collect::<Vec<_>>()
    );
    w.hub
        .hub
        .call(Call::ChatClose {
            session_id: "s1".into(),
        })
        .unwrap();
    let missing = w
        .hub
        .hub
        .call(Call::ChatOpen {
            session_id: "nobody".into(),
        })
        .unwrap_err();
    assert_eq!(missing.code, "not_found");
    let image = w
        .hub
        .hub
        .call(Call::ChatImage {
            session_id: "s1".into(),
            image_id: "none".into(),
        })
        .unwrap_err();
    assert_eq!(image.code, "not_found");
}

/// A status line's rate limits reach the account's ring.
#[test]
fn a_status_line_reaches_the_usage_and_the_session() {
    let w = world();
    w.send("UserPromptSubmit", "s1", json!({"prompt": "go"}));
    let resets = w.hub.handles.clock.now_ms() / 1000 + 3_600;
    let line = json!({
        "protocol": 1, "event": "StatusLine", "session_id": "s1",
        "transcript_path": w.transcript("s1"), "cwd": w.home.path("code/proj"),
        "config_dir_env": null, "pid": PID,
        "status_line": {
            "rate_limits": {"five_hour": {"used_percentage": 42, "resets_at": resets},
                            "seven_day": {"used_percentage": 10, "resets_at": resets + 86_400}},
            "context_window": {"used_percentage": 33, "context_window_size": 200_000},
            "model": {"id": "claude-opus", "display_name": "Opus"},
            "cost": {"total_cost_usd": 0.5}, "session_name": null, "version": "2.1.280",
        },
    });
    let now = w.hub.handles.clock.now();
    w.hub
        .handles
        .transport
        .inject(serde_json::to_vec(&line).unwrap(), now);
    assert!(
        eventually(|| w.row("s1").is_some_and(|r| r.context_pct == Some(33.0))),
        "{:?}",
        w.row("s1")
    );
    assert!(eventually(|| {
        w.hub.sync();
        w.status().readings == 1
    }));
}

// ---- Claude Desktop-hosted sessions ----

/// A host session id in Claude Desktop's form.
const HOST: &str = "local_0123abcd-89ef-4a5b-8c6d-001122334455";
/// The work account's organization.
const WORK_ORG: &str = "6f5e4d3c-2b1a-4a9f-8e8d-7c6b5a493827";
/// The Claude Desktop-hosted session's process.
const HOSTED_PID: u32 = 5151;

/// DesktopHostedSessionsTests at hub level
/// (`theHubAttributesDesktopSessionsByTheirRecordOnly`,
/// `lookupsAreRememberedAndRetried`): a session Claude Desktop hosts in
/// `~\.claude` counts for the account whose Desktop record names it, never
/// for the folder's; until the record exists it is unsure (shown under the
/// folder's account), and the lookup is tried again after a miss.
#[test]
fn a_desktop_hosted_session_counts_for_the_account_its_record_names() {
    let w = world_with(|home| {
        // Desktop names its folders by UUIDs.
        let mut login = home.login(BIIOS_UUID, BIIOS, None);
        login["oauthAccount"]["organizationUuid"] = json!(WORK_ORG);
        home.write_json(".claude-work/.claude.json", &login);
        home.mkdir(".claude-work/sessions");
        home.mkdir(".claude-work/projects");
    });
    let started = w.hub.handles.clock.now() - Duration::from_secs(30);
    w.hub
        .handles
        .processes
        .add(HOSTED_PID, 1, "claude.exe", started);
    let started_ms = agentnotch_engine::core::time::to_ms(started);
    w.home.write_json(
        &format!(".claude/sessions/{HOSTED_PID}.json"),
        &json!({"pid": HOSTED_PID, "sessionId": "hosted", "cwd": w.home.path("code/proj"),
                "startedAt": started_ms, "version": "2.1.282", "kind": "interactive",
                "entrypoint": "claude-desktop", "hostSessionId": HOST, "status": "busy",
                "updatedAt": started_ms, "statusUpdatedAt": started_ms}),
    );
    // A session of each account, through the hooks: their rings.
    w.send("UserPromptSubmit", "plain", json!({"prompt": "go"}));
    let work_transcript = w.home.path(".claude-work/projects/proj/work.jsonl");
    w.send(
        "UserPromptSubmit",
        "work",
        json!({"prompt": "go", "transcript_path": work_transcript}),
    );
    let ring = |id: &str| w.row(id).and_then(|row| row.ring_id);
    w.advance(Duration::from_secs(4));
    assert!(eventually(
        || ring("hosted").is_some() && ring("work").is_some()
    ));
    let personal = ring("plain").unwrap();
    let work = ring("work").unwrap();
    assert_ne!(personal, work);
    // No record yet: unsure, shown under the folder's account.
    assert_eq!(ring("hosted").unwrap(), personal);

    let records = w
        .hub
        .roots
        .claude_desktop
        .first()
        .unwrap()
        .join("claude-code-sessions")
        .join(BIIOS_UUID)
        .join(WORK_ORG);
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join(format!("{HOST}.json")), "{}").unwrap();
    // Looked for again after a miss.
    w.advance(Duration::from_secs(20));
    assert!(
        eventually(|| {
            w.advance(Duration::from_millis(500));
            ring("hosted").as_deref() == Some(work.as_str())
        }),
        "{:?}",
        w.row("hosted")
    );
}
