//! Control through the live hub (design §4.8-§4.10; HS§7-§9; UI§3.8): typed
//! replies and their two phases, the jump to the terminal, banners, chimes,
//! peeks, the panel that opens by itself, and "is the user looking at it".
//!
//! Everything runs against the testkit's fakes: `FakeConsole` records what
//! would have been typed and whether Return was pressed, `FakeTerminals`
//! answers the host, console, focus and visibility lookups. Nothing is ever
//! typed into a real terminal. The clock is the testkit's fake one, so the
//! hub's deadlines come only when a test moves it.

mod accounts_support;
mod hub_support;

use accounts_support::{Home, PARAS, PARAS_UUID};
use agentnotch_engine::control::messaging::TYPING_OFF;
use agentnotch_engine::control::notifications;
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, HubEvent};
use agentnotch_engine::model::{HubSnapshot, SessionRow};
use agentnotch_engine::platform::{
    Chime, Clock, ConnId, ConsoleInfo, ConsoleInput, ConsoleTarget, FocusOutcome, FocusStep,
    Foreground, HostApp, HostKind, Platform, ToastKind, TypeOutcome,
};
use agentnotch_engine::runtime_types::{Input, PanelState};
use agentnotch_engine::testkit::FakeConsole;
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Claude Code's pid in every frame (the hook's own is another).
const PID: u32 = 4242;
/// Its console window.
const WINDOW: u64 = 0x50_0A12;

struct World {
    home: Home,
    hub: TestHub,
}

/// A home signed in as one account, Claude Code running in a console
/// window of its own (a classic console host), the hub started.
fn world() -> World {
    world_with(|_| {})
}

fn world_with(customise: impl FnOnce(&mut Platform)) -> World {
    let home = Home::new();
    home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
    home.mkdir(".claude/sessions");
    home.mkdir(".claude/projects");
    let base: PathBuf = home
        .roots
        .home
        .parent()
        .expect("the test's root")
        .to_path_buf();
    let hub = TestHub::over(&base, RuntimeOptions::default(), |_| {}, customise);
    let started = hub.handles.clock.now() - Duration::from_secs(60);
    hub.handles.processes.add(PID, 1, "claude.exe", started);
    hub.handles.terminals.set_host(
        PID,
        HostApp {
            kind: HostKind::Conhost,
            window: Some(WINDOW),
            host_pid: None,
            exe_path: None,
        },
    );
    hub.handles.terminals.set_console(PID, console());
    hub.hub.start().expect("the hub starts");
    assert!(
        eventually(|| hub.logs().iter().any(|l| l == "pipe listening")),
        "{:?}",
        hub.logs()
    );
    let w = World { home, hub };
    // The launch baseline: nothing seen before it is news.
    w.advance(Duration::from_secs(3));
    w
}

/// Claude's own console: attached, in raw mode, Claude alone in it.
fn console() -> ConsoleInfo {
    ConsoleInfo {
        attached: true,
        window: Some(WINDOW),
        title: Some("Refactor the parser".into()),
        processes: vec![PID],
        line_input: Some(false),
        elevated_target: false,
        error: None,
    }
}

impl World {
    fn frame(&self, event: &str, session: &str, extra: Value) -> Vec<u8> {
        let mut message = json!({
            "protocol": 1, "event": event, "session_id": session,
            "cwd": self.home.path("code/proj"),
            "transcript_path": self.home.path(&format!(".claude/projects/proj/{session}.jsonl")),
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

    /// The frames of a tool call that needs approval (its PreToolUse, then
    /// the held PermissionRequest).
    fn ask_frames(&self, session: &str) -> [Vec<u8>; 2] {
        let input = json!({"command": "npm test"});
        [
            self.frame(
                "PreToolUse",
                session,
                json!({"tool": "Bash", "tool_input": input, "tool_use_id": format!("toolu_{session}")}),
            ),
            self.frame(
                "PermissionRequest",
                session,
                json!({"tool": "Bash", "tool_input": input, "permission_suggestions": [],
                       "status": "waiting_for_approval"}),
            ),
        ]
    }

    /// A tool call that needs approval; returns once the row shows it.
    fn ask(&self, session: &str) {
        self.send("UserPromptSubmit", session, json!({"prompt": "go"}));
        let now = self.hub.handles.clock.now();
        for frame in self.ask_frames(session) {
            self.hub.handles.transport.inject(frame, now);
        }
        assert!(
            eventually(|| self.row(session).is_some_and(|r| r.pending.is_some())),
            "the request of {session} shows: {:?}",
            self.shown().sessions
        );
    }

    /// A turn that finished and waits for review.
    fn finished(&self, session: &str) {
        self.send("UserPromptSubmit", session, json!({"prompt": "go"}));
        assert!(eventually(|| self.row(session).is_some()));
        self.advance(Duration::from_secs(1));
        self.send("Stop", session, json!({"status": "waiting_for_input"}));
        // No registry entry confirms the Stop: the fallback settles it.
        self.advance(Duration::from_secs(95));
        assert!(
            eventually(|| self.row(session).is_some_and(|r| r.reviewable)),
            "{:?}",
            self.row(session)
        );
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

    fn call(&self, call: Call) -> Value {
        self.hub.hub.call(call).expect("the hub answers")
    }

    fn set(&self, key: &str, value: Value) {
        self.call(Call::SetSetting {
            key: key.into(),
            value,
        });
    }

    fn send_message(&self, session: &str, text: &str) -> Value {
        self.call(Call::SendMessage {
            session_id: session.into(),
            text: text.into(),
        })
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

    /// The burst's window has passed and its visibility scan is back: what
    /// it decided is carried out (every burst here chimes, and says so).
    fn settle_burst(&self) {
        self.advance(Duration::from_millis(600));
        assert!(
            eventually(|| {
                self.hub.sync();
                self.hub.logs().iter().any(|l| l.starts_with("attention: "))
            }),
            "{:?}",
            self.hub.logs()
        );
    }

    fn events(&self) -> Vec<HubEvent> {
        self.hub.events().into_iter().map(|(_, e)| e).collect()
    }

    fn panel_requests(&self) -> Vec<agentnotch_engine::model::PanelRequest> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                HubEvent::Panel(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    fn panel_closes(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                HubEvent::PanelClose { reason } => Some(reason),
                _ => None,
            })
            .collect()
    }

    fn peeks(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                HubEvent::Peek { ring_id, .. } => Some(ring_id),
                _ => None,
            })
            .collect()
    }
}

// ---- typing replies ----

/// Typing replies is opt-in: off, a send is refused and the chat is told why.
#[test]
fn typing_replies_off_refuses_and_says_why() {
    let w = world();
    w.finished("s1");
    let sent = w.send_message("s1", "go on");
    assert_eq!(sent["outcome"], "refused", "{sent}");
    assert_eq!(sent["reason"], TYPING_OFF);
    assert_eq!(
        w.call(Call::MessageRoute {
            session_id: "s1".into()
        }),
        json!({"available": false, "reason": "Typing replies is off. Turn it on in Settings › Claude Code."})
    );
    assert!(w.hub.handles.console.typed().is_empty());
    assert!(!w.row("s1").unwrap().can_message);
}

/// On, a reply to a session at its prompt is typed and submitted, into the
/// console window it was first seen with.
#[test]
fn a_reply_is_typed_and_submitted_when_nothing_changed() {
    let w = world();
    w.set("typeReplies", json!(true));
    w.finished("s1");
    assert_eq!(
        w.call(Call::MessageRoute {
            session_id: "s1".into()
        }),
        json!({"available": true})
    );
    assert!(w.row("s1").unwrap().can_message);
    let sent = w.send_message("s1", "  go on\nplease ");
    assert_eq!(sent, json!({"outcome": "delivered"}));
    let typed = w.hub.handles.console.typed();
    assert_eq!(typed.len(), 1, "{typed:?}");
    let (target, text, submitted) = &typed[0];
    assert_eq!(text, "go on please");
    assert!(*submitted);
    assert_eq!(target.claude_pid, PID);
    assert_eq!(target.expected_window, Some(WINDOW));
}

/// A reply is never typed while the session waits on a dialog.
#[test]
fn a_reply_is_refused_while_a_request_is_open() {
    let w = world();
    w.set("typeReplies", json!(true));
    w.ask("s1");
    let sent = w.send_message("s1", "go on");
    assert_eq!(sent["outcome"], "refused", "{sent}");
    assert_eq!(sent["reason"], "Answer the permission prompt first");
    assert!(w.hub.handles.console.typed().is_empty());
}

/// What runs between the text and Return, once set.
type Gap = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// Records like `FakeConsole`, and runs `gap` between the text and Return.
struct GapConsole {
    inner: Arc<FakeConsole>,
    gap: Gap,
}

impl ConsoleInput for GapConsole {
    fn type_text(
        &self,
        target: &ConsoleTarget,
        text: &str,
        recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome {
        let gap = self.gap.clone();
        self.inner.type_text(target, text, &mut || {
            if let Some(gap) = gap.lock().unwrap().as_ref() {
                gap();
            }
            recheck()
        })
    }
}

/// The contract's safety core: a PermissionRequest raised between the text
/// and Return (a background agent, say) means Return is never pressed, so
/// it can't confirm a dialog the user never saw. The request stays held for
/// the user to answer.
#[test]
fn a_request_raised_between_typed_and_submit_keeps_return_unpressed() {
    let gap: Gap = Arc::default();
    let recorder = Arc::new(FakeConsole::default());
    let (inner, slot) = (recorder.clone(), gap.clone());
    let w = world_with(move |platform| {
        platform.console = Arc::new(GapConsole { inner, gap: slot });
    });
    w.set("typeReplies", json!(true));
    w.finished("s1");
    let frames = w.ask_frames("s1");
    let (hub, transport, clock) = (
        w.hub.hub.clone(),
        w.hub.handles.transport.clone(),
        w.hub.handles.clock.clone(),
    );
    *gap.lock().unwrap() = Some(Box::new(move || {
        for frame in &frames {
            transport.inject(frame.clone(), clock.now());
        }
        // `an-core` has taken the request before the checkpoint is asked.
        assert!(eventually(|| {
            let shown: HubSnapshot =
                serde_json::from_value(hub.call(Call::Snapshot).unwrap()).unwrap();
            shown
                .sessions
                .iter()
                .any(|row| row.session_id == "s1" && row.pending.is_some())
        }));
    }));
    let sent = w.send_message("s1", "go on");
    assert_eq!(sent["outcome"], "typed_not_submitted", "{sent}");
    let typed = recorder.typed();
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(!typed[0].2, "Return was pressed");
    // The request is still the user's to answer.
    assert!(w.row("s1").unwrap().pending.is_some());
    assert!(w.hub.handles.transport.responses().is_empty());
}

/// A reply sent while Claude works is held, checked again every 250 ms, and
/// refused with the draft kept once it has waited 10 s.
#[test]
fn a_reply_sent_while_claude_works_is_held_then_refused() {
    let w = world();
    w.set("typeReplies", json!(true));
    w.finished("s1");
    w.send("UserPromptSubmit", "s1", json!({"prompt": "again"}));
    assert!(eventually(|| w
        .row("s1")
        .is_some_and(|r| r.bucket == "working")));
    let hub = w.hub.hub.clone();
    let sending = std::thread::spawn(move || {
        hub.call(Call::SendMessage {
            session_id: "s1".into(),
            text: "and then".into(),
        })
    });
    w.advance(Duration::from_secs(9));
    assert!(!sending.is_finished(), "held while Claude works");
    w.advance(Duration::from_millis(1500));
    let sent = sending.join().unwrap().unwrap();
    assert_eq!(
        sent,
        json!({"outcome": "refused", "reason": "Claude is working: send when it's done"})
    );
    assert!(w.hub.handles.console.typed().is_empty());
}

// ---- the jump ----

/// A jump that worked marks the session reviewed and closes the panel
/// unless it is pinned.
#[test]
fn focus_success_marks_reviewed_and_closes_an_unpinned_panel() {
    let w = world();
    w.finished("s1");
    w.hub
        .handles
        .terminals
        .set_focus_outcome(FocusOutcome::Focused);
    let focused = w.call(Call::Focus {
        session_id: "s1".into(),
    });
    assert_eq!(focused, json!({"outcome": "focused"}));
    assert_eq!(
        w.hub.handles.terminals.focus_steps(),
        vec![FocusStep::RaiseWindow { window: WINDOW }]
    );
    assert!(eventually(|| w.row("s1").is_some_and(|r| !r.reviewable)));
    assert_eq!(w.panel_closes(), vec!["jump".to_owned()]);

    // Pinned: the panel stays.
    w.finished("s2");
    w.set("panelPinned", json!(true));
    w.call(Call::Focus {
        session_id: "s2".into(),
    });
    assert_eq!(w.panel_closes(), vec!["jump".to_owned()]);
}

/// A jump that found nothing says so and marks nothing.
#[test]
fn focus_that_finds_nothing_marks_nothing() {
    let w = world();
    w.finished("s1");
    let focused = w.call(Call::Focus {
        session_id: "s1".into(),
    });
    assert_eq!(focused["outcome"], "not_found");
    assert!(w.row("s1").unwrap().reviewable);
    assert!(w.panel_closes().is_empty());
    let unknown = w.call(Call::Focus {
        session_id: "nobody".into(),
    });
    assert_eq!(unknown["outcome"], "not_found");
}

// ---- banners, chime, peek, auto-open ----

/// A session that starts needing you posts its banner (silent: the app
/// chimes itself) and the blocked chime; answering takes the banner back.
#[test]
fn a_needs_you_banner_and_chime_and_its_withdrawal() {
    let w = world();
    w.ask("s1");
    w.settle_burst();
    let posted = w.hub.handles.notifier.posted();
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(posted[0].kind, ToastKind::NeedsInput);
    assert_eq!(posted[0].group, notifications::group("s1"));
    assert!(
        posted[0].title.ends_with("needs you"),
        "{}",
        posted[0].title
    );
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Blocked]);
    // Auto-open is Never by default on Windows: the notch peeks instead.
    assert!(w.panel_requests().is_empty());
    assert_eq!(w.peeks().len(), 1, "{:?}", w.events());

    w.call(Call::Answer {
        session_id: "s1".into(),
        tool_use_id: "toolu_s1".into(),
        answer: agentnotch_engine::model::Answer::Allow { always: false },
    });
    assert!(eventually(|| w
        .hub
        .handles
        .notifier
        .withdrawn()
        .contains(&("needs".to_owned(), notifications::group("s1")))));
}

/// Banners off: none is posted (the chime still plays).
#[test]
fn no_banner_with_needs_you_banners_off() {
    let w = world();
    w.set("notifyNeedsInput", json!(false));
    w.ask("s1");
    w.settle_burst();
    assert!(w.hub.handles.notifier.posted().is_empty());
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Blocked]);
}

/// Auto-open on: the panel opens on the list with the session highlighted,
/// and closes by itself at its deadline when nobody touched it.
#[test]
fn an_auto_opened_panel_closes_at_its_deadline() {
    let w = world();
    w.set("autoOpen", json!("needsInput"));
    w.ask("s1");
    w.settle_burst();
    let requests = w.panel_requests();
    assert_eq!(requests.len(), 1, "{:?}", w.events());
    assert_eq!(requests[0].reason, "auto");
    assert_eq!(requests[0].highlight.as_deref(), Some("s1"));
    assert!(w.peeks().is_empty());
    w.call(Call::PanelState(PanelState {
        open: true,
        route: Some("sessions".into()),
        reason: Some("auto".into()),
        ..PanelState::default()
    }));
    // max(peek 5 s, the 8 s minimum) after it opened.
    w.advance(Duration::from_secs(6));
    assert!(w.panel_closes().is_empty());
    w.advance(Duration::from_secs(3));
    assert_eq!(w.panel_closes(), vec!["auto".to_owned()]);
}

/// A panel the user took over (the pointer went in) stays open.
#[test]
fn an_auto_opened_panel_the_user_touched_stays() {
    let w = world();
    w.set("autoOpen", json!("needsInput"));
    w.ask("s1");
    w.settle_burst();
    assert_eq!(w.panel_requests().len(), 1);
    let open = PanelState {
        open: true,
        reason: Some("auto".into()),
        ..PanelState::default()
    };
    w.call(Call::PanelState(PanelState {
        engaged: true,
        ..open.clone()
    }));
    w.call(Call::PanelState(open));
    w.advance(Duration::from_secs(12));
    assert!(w.panel_closes().is_empty());
}

/// Never over a panel the user has open.
#[test]
fn no_auto_open_over_an_open_panel() {
    let w = world();
    w.set("autoOpen", json!("needsInput"));
    w.call(Call::PanelState(PanelState {
        open: true,
        reason: Some("click".into()),
        ..PanelState::default()
    }));
    w.ask("s1");
    w.settle_burst();
    assert!(w.panel_requests().is_empty(), "{:?}", w.events());
    // The banner and the chime don't depend on the panel.
    assert_eq!(w.hub.handles.notifier.posted().len(), 1);
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Blocked]);
}

/// The visibility scan fills `any_terminal_visible`: with a terminal on
/// screen the panel doesn't open by itself, the notch peeks.
#[test]
fn a_visible_terminal_turns_the_auto_open_into_a_peek() {
    let w = world();
    w.set("autoOpen", json!("needsInput"));
    w.hub.handles.terminals.set_visible(true);
    w.ask("s1");
    w.settle_burst();
    assert!(w.panel_requests().is_empty(), "{:?}", w.events());
    assert_eq!(w.peeks().len(), 1, "{:?}", w.events());
}

// ---- looking at it ----

/// A switch to the session's own console window that stays 1.5 s marks its
/// completion viewed; a shorter one, or another window, doesn't.
#[test]
fn a_foreground_held_for_one_and_a_half_seconds_marks_viewed() {
    let w = world();
    w.finished("s1");
    let fg = |window: u64| Foreground {
        pid: 77,
        window,
        title: "Refactor the parser".into(),
        fullscreen: false,
    };
    w.hub.inputs.send(Input::Foreground(fg(0xBEEF)));
    w.advance(Duration::from_secs(2));
    assert!(w.row("s1").unwrap().reviewable, "another window");

    w.hub.inputs.send(Input::Foreground(fg(WINDOW)));
    w.hub.sync();
    w.advance(Duration::from_secs(1));
    assert!(w.row("s1").unwrap().reviewable, "not long enough yet");
    w.advance(Duration::from_millis(500));
    assert!(eventually(|| w.row("s1").is_some_and(|r| !r.reviewable)));
}

/// The hot key's report reaches Settings.
#[test]
fn the_hotkey_status_reaches_settings() {
    let w = world();
    w.call(Call::HotkeyStatus {
        ok: false,
        message: Some("Ctrl+Shift+Space is taken".into()),
    });
    let settings = w.call(Call::Settings);
    assert_eq!(settings["attention"]["hotkey_ok"], false, "{settings}");
    assert_eq!(
        settings["attention"]["hotkey_message"],
        "Ctrl+Shift+Space is taken"
    );
}
