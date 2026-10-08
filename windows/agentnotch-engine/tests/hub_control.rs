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
    Foreground, HostApp, HostKind, Liveness, NotifyPermission, Platform, ProcessTable, Terminals,
    ToastKind, TypeOutcome,
};
use agentnotch_engine::runtime_types::{Input, PanelState};
use agentnotch_engine::testkit::{FakeConsole, FakeProcesses, RecordingNotifier};
use hub_support::live::{eventually, TestHub};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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
    world_prepared(customise, |_| {})
}

/// `world_with`, with `prepare` run on the fakes before the hub starts.
fn world_prepared(customise: impl FnOnce(&mut Platform), prepare: impl FnOnce(&TestHub)) -> World {
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
    prepare(&hub);
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

/// Return goes only to the process the session recorded: when, between the
/// text and Return, the pid belongs to a process started later, or to one
/// whose start time can't be read (which the session store still counts as
/// running), Return is never pressed.
#[test]
fn return_is_never_pressed_into_a_process_not_confirmed_as_claude() {
    type Change = fn(&FakeProcesses, SystemTime);
    let cases: [(&str, Change); 2] = [
        ("an unreadable start time", |processes, _| {
            processes.remove(PID);
            processes.set_liveness(PID, Liveness::Alive);
        }),
        ("a reused pid", |processes, now| {
            processes.add(PID, 1, "pwsh.exe", now);
        }),
    ];
    for (case, change) in cases {
        let gap: Gap = Arc::default();
        let recorder = Arc::new(FakeConsole::default());
        let (inner, slot) = (recorder.clone(), gap.clone());
        let w = world_with(move |platform| {
            platform.console = Arc::new(GapConsole { inner, gap: slot });
        });
        w.set("typeReplies", json!(true));
        w.finished("s1");
        let (processes, clock) = (w.hub.handles.processes.clone(), w.hub.handles.clock.clone());
        *gap.lock().unwrap() = Some(Box::new(move || change(&processes, clock.now())));
        let sent = w.send_message("s1", "go on");
        assert_eq!(sent["outcome"], "typed_not_submitted", "{case}: {sent}");
        let typed = recorder.typed();
        assert_eq!(typed.len(), 1, "{case}: {typed:?}");
        assert!(!typed[0].2, "{case}: Return was pressed");
    }
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
    // The 10 s count from when an-core took the call, which a busy runner
    // may reach only after the clock began to move (Windows CI 37310257609):
    // go on in steps until it answers, never more than 10 s past the first 9.
    let mut more = Duration::ZERO;
    loop {
        w.advance(Duration::from_millis(500));
        more += Duration::from_millis(500);
        let deadline = std::time::Instant::now() + Duration::from_millis(200);
        while !sending.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if sending.is_finished() {
            break;
        }
        assert!(more < Duration::from_secs(10), "still held 19 s on");
    }
    assert!(
        more >= Duration::from_secs(1),
        "answered before 10 s: {more:?}"
    );
    let sent = sending.join().unwrap().unwrap();
    assert_eq!(
        sent,
        json!({"outcome": "refused", "reason": "Claude is working: send when it's done"})
    );
    assert!(w.hub.handles.console.typed().is_empty());
}

/// A console lookup that waits until the test lets it go (`an-ui` held by
/// a terminal that doesn't answer).
struct HangingConsoles {
    inner: Arc<dyn Terminals>,
    open: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl Terminals for HangingConsoles {
    fn classify_host(&self, claude_pid: u32, table: &ProcessTable) -> HostApp {
        self.inner.classify_host(claude_pid, table)
    }
    fn console_info(&self, claude_pid: u32) -> ConsoleInfo {
        let (open, changed) = &*self.open;
        let mut is_open = open.lock().unwrap();
        while !*is_open {
            is_open = changed.wait(is_open).unwrap();
        }
        drop(is_open);
        self.inner.console_info(claude_pid)
    }
    fn run_focus(&self, step: &FocusStep) -> FocusOutcome {
        self.inner.run_focus(step)
    }
    fn foreground(&self) -> Option<Foreground> {
        self.inner.foreground()
    }
    fn window_title(&self, window: u64) -> Option<String> {
        self.inner.window_title(window)
    }
    fn wt_tab_titles(&self, window: u64) -> Option<Vec<(String, bool)>> {
        self.inner.wt_tab_titles(window)
    }
    fn any_terminal_visible(&self) -> bool {
        self.inner.any_terminal_visible()
    }
    fn watch_foreground(&self, sink: crossbeam_channel::Sender<Foreground>) {
        self.inner.watch_foreground(sink)
    }
}

/// A reply whose terminal doesn't answer the lookups is refused within 10 s,
/// well inside the call's 25 s, and is not typed when the lookup comes back
/// later: the page gave up on it, and the user may send it again.
#[test]
fn a_reply_whose_terminal_hangs_is_refused_in_time_and_never_typed_later() {
    let open: Arc<(Mutex<bool>, std::sync::Condvar)> = Arc::default();
    let gate = open.clone();
    let w = world_with(move |platform| {
        let inner = platform.terminals.clone();
        platform.terminals = Arc::new(HangingConsoles { inner, open: gate });
    });
    w.set("typeReplies", json!(true));
    w.finished("s1");
    let hub = w.hub.hub.clone();
    let sending = std::thread::spawn(move || {
        hub.call(Call::SendMessage {
            session_id: "s1".into(),
            text: "go on".into(),
        })
    });
    w.advance(Duration::from_secs(9));
    assert!(!sending.is_finished(), "waits for the lookups first");
    let mut more = Duration::ZERO;
    while !sending.is_finished() {
        assert!(more < Duration::from_secs(10), "still waiting 19 s on");
        w.advance(Duration::from_millis(500));
        more += Duration::from_millis(500);
        let deadline = std::time::Instant::now() + Duration::from_millis(200);
        while !sending.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let sent = sending.join().unwrap().unwrap();
    assert_eq!(
        sent,
        json!({"outcome": "refused", "reason": "The terminal didn't answer in time"})
    );
    // The terminal answers at last: nothing is typed for the refused call.
    let (lock, changed) = &*open;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    w.advance(Duration::from_secs(1));
    assert!(w.hub.handles.console.typed().is_empty());
}

/// Return is never pressed 20 s or more after the call: by then the page
/// may have been told `busy` and the user may have sent the reply again.
#[test]
fn a_reply_is_not_submitted_once_its_call_may_have_given_up() {
    let gap: Gap = Arc::default();
    let recorder = Arc::new(FakeConsole::default());
    let (inner, slot) = (recorder.clone(), gap.clone());
    let w = world_with(move |platform| {
        platform.console = Arc::new(GapConsole { inner, gap: slot });
    });
    w.set("typeReplies", json!(true));
    w.finished("s1");
    let clock = w.hub.handles.clock.clone();
    *gap.lock().unwrap() = Some(Box::new(move || {
        clock.advance(Duration::from_secs(21));
    }));
    let sent = w.send_message("s1", "go on");
    assert_eq!(sent["outcome"], "typed_not_submitted", "{sent}");
    let typed = recorder.typed();
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(!typed[0].2, "Return was pressed");
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

/// Windows' banner switch is read again before banners are decided and
/// whenever Settings asks: turned on in Windows Settings while the app runs,
/// the next request posts its banner and Settings stops warning (the Mac
/// asks before every post and when its pane appears).
#[test]
fn the_windows_banner_switch_is_read_again() {
    let notifier = Arc::new(RecordingNotifier::default());
    notifier.set_permission(NotifyPermission::DisabledForUser);
    let mine = notifier.clone();
    let w = world_with(move |platform| platform.notifier = mine);
    let settings = w.call(Call::Settings);
    assert_eq!(settings["notifications"]["permission"], "disabled_for_user");
    assert_eq!(settings["notifications"]["permission_warning"], true);
    w.ask("s1");
    w.settle_burst();
    assert!(notifier.posted().is_empty(), "{:?}", notifier.posted());

    notifier.set_permission(NotifyPermission::Allowed);
    w.ask("s2");
    assert!(
        eventually(|| notifier
            .posted()
            .iter()
            .any(|t| t.group == notifications::group("s2"))),
        "{:?}",
        notifier.posted()
    );
    let settings = w.call(Call::Settings);
    assert_eq!(settings["notifications"]["permission"], "allowed");
    assert_eq!(settings["notifications"]["permission_warning"], false);

    // Turned off again: the page that asks is told, through `an:settings`
    // too.
    notifier.set_permission(NotifyPermission::DisabledForApp);
    w.call(Call::Settings);
    assert!(eventually(|| {
        w.advance(Duration::from_millis(100));
        w.events().iter().rev().find_map(|e| match e {
            HubEvent::Settings(s) => Some(s.notifications.permission == "disabled_for_app"),
            _ => None,
        }) == Some(true)
    }));
}

/// A session stopped by its usage limit while a full-screen app is in front
/// posts no limit banner, as no other banner is posted then; out of full
/// screen the next one does.
#[test]
fn a_limit_banner_is_held_back_in_full_screen() {
    let w = world();
    let limits = || {
        w.hub
            .handles
            .notifier
            .posted()
            .into_iter()
            .filter(|t| t.kind == ToastKind::Limit)
            .count()
    };
    let fg = |fullscreen: bool| Foreground {
        pid: 77,
        window: 0xBEEF,
        title: "A game".into(),
        fullscreen,
    };
    let limited = |session: &str| {
        w.send("UserPromptSubmit", session, json!({"prompt": "go"}));
        w.send(
            "StopFailure",
            session,
            json!({"stop_error": "rate_limit", "status": "waiting_for_input"}),
        );
        assert!(eventually(|| w.row(session).is_some_and(|r| r.failed)));
        w.advance(Duration::from_secs(1));
    };
    w.hub.handles.terminals.set_foreground(Some(fg(true)));
    limited("s1");
    assert_eq!(limits(), 0, "{:?}", w.hub.handles.notifier.posted());

    w.hub.handles.terminals.set_foreground(Some(fg(false)));
    limited("s2");
    assert!(
        eventually(|| limits() == 1),
        "{:?}",
        w.hub.handles.notifier.posted()
    );
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

// ---- a banner clicked while the app wasn't running ----

/// The real process table, except that the launch's registry read (on an
/// `an-io` worker) waits until the test lets it go.
struct HeldRegistry {
    inner: Arc<dyn agentnotch_engine::platform::Processes>,
    open: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl HeldRegistry {
    fn wait(&self) {
        let on_io = std::thread::current()
            .name()
            .is_some_and(|name| name.starts_with("an-io"));
        if !on_io {
            return;
        }
        let (open, turn) = &*self.open;
        let mut open = open.lock().unwrap();
        while !*open {
            open = turn.wait(open).unwrap();
        }
    }
}

impl agentnotch_engine::platform::Processes for HeldRegistry {
    fn liveness(&self, pid: u32) -> Liveness {
        self.wait();
        self.inner.liveness(pid)
    }
    fn start_time(&self, pid: u32) -> Option<SystemTime> {
        self.inner.start_time(pid)
    }
    fn table(&self) -> ProcessTable {
        self.inner.table()
    }
    fn config_dir_env(&self, pid: u32) -> agentnotch_engine::platform::EnvRead {
        self.inner.config_dir_env(pid)
    }
    fn same_user(&self, pid: u32) -> Option<bool> {
        self.inner.same_user(pid)
    }
    fn elevated(&self, pid: u32) -> Option<bool> {
        self.inner.elevated(pid)
    }
    fn exe_path(&self, pid: u32) -> Option<PathBuf> {
        self.inner.exe_path(pid)
    }
}

/// DESIGN-WIN §4.10: a cold start hands its link to the hub in setup, right
/// after the start, before the launch's registry read is back and so before
/// any session is listed. The link waits for that read instead of being
/// ignored as "no such session" (the Mac's `openSession` has no such check).
#[test]
fn a_banner_link_that_started_the_app_waits_for_the_launch_scan() {
    let open: Arc<(Mutex<bool>, std::sync::Condvar)> = Arc::default();
    let held = open.clone();
    let w = world_prepared(
        move |platform| {
            platform.processes = Arc::new(HeldRegistry {
                inner: platform.processes.clone(),
                open: held,
            });
        },
        |hub| {
            let started = hub.handles.clock.now() - Duration::from_secs(60);
            let started_ms = agentnotch_engine::core::time::to_ms(started);
            let home = hub.roots.home.clone();
            std::fs::write(
                home.join(format!(".claude/sessions/{PID}.json")),
                serde_json::to_vec(&json!({
                    "pid": PID, "sessionId": "cold", "cwd": home.join("code/proj"),
                    "startedAt": started_ms, "version": "2.1.282", "kind": "interactive",
                    "entrypoint": "cli", "status": "waiting", "waitingFor": "approve Bash",
                    "updatedAt": started_ms, "statusUpdatedAt": started_ms,
                }))
                .unwrap(),
            )
            .unwrap();
        },
    );
    assert!(w.row("cold").is_none(), "the launch read is still out");
    let hub = w.hub.hub.clone();
    let clicked =
        std::thread::spawn(move || hub.handle_deep_link("agentnotch://open?session=cold"));
    std::thread::sleep(Duration::from_millis(200));
    *open.0.lock().unwrap() = true;
    open.1.notify_all();
    assert_eq!(
        clicked.join().unwrap(),
        agentnotch_engine::hub::DeepLinkOutcome::Opened
    );
    assert!(w
        .panel_requests()
        .iter()
        .any(|request| request.highlight.as_deref() == Some("cold")));
    // Once the scan is back, an unknown session is ignored at once.
    let asked = std::time::Instant::now();
    assert!(matches!(
        w.hub
            .hub
            .handle_deep_link("agentnotch://open?session=nobody"),
        agentnotch_engine::hub::DeepLinkOutcome::Ignored(_)
    ));
    assert!(asked.elapsed() < Duration::from_secs(1));
}

// ---- a usage limit is announced once per account ----

impl World {
    /// A turn that fails on the account's usage limit; returns once the row
    /// shows it.
    fn hit_limit(&self, session: &str) {
        self.send("UserPromptSubmit", session, json!({"prompt": "go"}));
        self.send(
            "StopFailure",
            session,
            json!({"stop_error": "rate_limit", "status": "waiting_for_input"}),
        );
        assert!(eventually(|| self.row(session).is_some_and(|r| r.failed)));
        self.advance(Duration::from_secs(1));
    }

    /// A status line of `session` saying the 5-hour window is `used` percent
    /// used, resetting in an hour.
    fn status_line(&self, session: &str, used: u32) {
        let resets = self.hub.handles.clock.now_ms() / 1000 + 3_600;
        let line = json!({
            "protocol": 1, "event": "StatusLine", "session_id": session,
            "transcript_path": self.home.path(&format!(".claude/projects/proj/{session}.jsonl")),
            "cwd": self.home.path("code/proj"), "config_dir_env": null, "pid": PID,
            "status_line": {
                "rate_limits": {"five_hour": {"used_percentage": used, "resets_at": resets}},
                "model": {"id": "claude-opus", "display_name": "Opus"},
                "version": "2.1.280",
            },
        });
        let now = self.hub.handles.clock.now();
        self.hub
            .handles
            .transport
            .inject(serde_json::to_vec(&line).unwrap(), now);
        self.hub.sync();
    }

    fn limit_toasts(&self) -> usize {
        self.hub
            .handles
            .notifier
            .posted()
            .into_iter()
            .filter(|t| t.kind == ToastKind::Limit)
            .count()
    }

    /// Lets every burst the last events began close and be carried out.
    fn quiet(&self) {
        self.advance(Duration::from_secs(3));
        self.hub.sync();
    }

    fn announcements(&self) -> Option<String> {
        std::fs::read_to_string(self.hub.roots.support.join("limit-announcements.json")).ok()
    }
}

/// The limit banner, the chime and the peek each come once per limit: not on
/// the retry that fails again, nor for another session stopped by the same
/// limit. The record is written to `limit-announcements.json`, and the banner
/// is taken back when no session is stopped by the limit any more.
#[test]
fn a_usage_limit_is_announced_once_per_account() {
    let w = world();
    w.hit_limit("s1");
    w.settle_burst();
    assert_eq!(w.limit_toasts(), 1, "{:?}", w.hub.handles.notifier.posted());
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Finished]);
    assert_eq!(w.peeks().len(), 1, "{:?}", w.events());
    // Written after each claim, the second one a moment after the first.
    assert!(
        eventually(|| w.announcements().is_some_and(
            |record| record.contains("\"notification\"") && record.contains("\"reaction\"")
        )),
        "the record is written: {:?}",
        w.announcements()
    );
    let record = w.announcements().unwrap();
    assert!(record.contains("\"window\":\"unknown\""), "{record}");

    // The retry (a wake-up, a /loop tick) fails on the limit again.
    w.hit_limit("s1");
    w.quiet();
    // Another session of the account is stopped by it too.
    w.hit_limit("s2");
    w.quiet();
    assert_eq!(w.limit_toasts(), 1, "{:?}", w.hub.handles.notifier.posted());
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Finished]);
    assert_eq!(w.peeks().len(), 1, "{:?}", w.events());

    // No session is stopped by it any more: the banner goes.
    for session in ["s1", "s2"] {
        w.call(Call::DismissFailure {
            session_id: session.into(),
        });
    }
    assert!(eventually(|| w
        .hub
        .handles
        .notifier
        .withdrawn()
        .iter()
        .any(|(tag, _)| tag == "limit")));
    // And hitting it again before the window resets is still the same limit.
    w.hit_limit("s3");
    w.quiet();
    assert_eq!(w.limit_toasts(), 1);
}

/// A banner that macOS' counterpart would not show claims nothing: with
/// Windows' notifications off the chime and peek announce the limit, and
/// turned back on, the next session stopped by it still gets its banner (no
/// card falls in for it on Windows), though never a second chime.
#[test]
fn a_limit_whose_banner_could_not_show_is_left_to_the_chime() {
    let notifier = Arc::new(RecordingNotifier::default());
    notifier.set_permission(NotifyPermission::DisabledForUser);
    let mine = notifier.clone();
    let w = world_with(move |platform| platform.notifier = mine);
    w.hit_limit("s1");
    w.settle_burst();
    assert!(notifier.posted().is_empty(), "{:?}", notifier.posted());
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Finished]);
    assert_eq!(w.peeks().len(), 1);

    notifier.set_permission(NotifyPermission::Allowed);
    w.hit_limit("s2");
    w.quiet();
    let limits = || {
        notifier
            .posted()
            .into_iter()
            .filter(|t| t.kind == ToastKind::Limit)
            .count()
    };
    assert_eq!(limits(), 1, "{:?}", notifier.posted());
    // The reaction was announced already.
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Finished]);
    assert_eq!(w.peeks().len(), 1);
}

/// A relaunch doesn't announce the same limit again: the record comes back
/// from `limit-announcements.json`.
#[test]
fn an_announced_limit_survives_a_relaunch() {
    let first = world();
    first.hit_limit("s1");
    first.settle_burst();
    assert_eq!(first.limit_toasts(), 1);
    assert!(eventually(|| first
        .announcements()
        .is_some_and(|record| record.contains("\"notification\""))));
    let record = first.announcements().unwrap();

    let again = world_prepared(
        |_| {},
        |hub| {
            std::fs::create_dir_all(&hub.roots.support).unwrap();
            std::fs::write(hub.roots.support.join("limit-announcements.json"), &record).unwrap();
        },
    );
    again.hit_limit("s1");
    again.quiet();
    assert_eq!(
        again.limit_toasts(),
        0,
        "{:?}",
        again.hub.handles.notifier.posted()
    );
    assert!(again.hub.handles.sounds.played().is_empty());
}

/// A session reopened with the failure read back from `review-state.json`
/// shows it, and announces nothing: no banner, no chime, no peek.
#[test]
fn a_failure_restored_from_disk_announces_nothing() {
    let failed_at = 1_800_000_000.0 - 3600.0;
    let w = world_prepared(
        |_| {},
        |hub| {
            std::fs::create_dir_all(&hub.roots.support).unwrap();
            let file = format!(
                r#"{{"version":2,"lastAliveAt":{alive},"sessions":{{"s1":{{"stopError":"Rate limited","stopErrorCode":"rate_limit","failedAt":{failed_at},"updatedAt":{failed_at}}}}}}}"#,
                alive = failed_at + 60.0
            );
            std::fs::write(hub.roots.support.join("review-state.json"), file).unwrap();
        },
    );
    w.send(
        "SessionStart",
        "s1",
        json!({"source": "resume", "status": "waiting_for_input"}),
    );
    assert!(eventually(|| w.row("s1").is_some_and(|r| r.failed)));
    w.quiet();
    assert_eq!(w.limit_toasts(), 0, "{:?}", w.hub.handles.notifier.posted());
    assert!(w.hub.handles.notifier.posted().is_empty());
    assert!(w.hub.handles.sounds.played().is_empty());
    assert!(w.peeks().is_empty());
    assert!(w.announcements().is_none(), "nothing was announced");

    // Tried again and failed again, live: that is news.
    w.hit_limit("s1");
    w.settle_burst();
    assert_eq!(w.limit_toasts(), 1, "{:?}", w.hub.handles.notifier.posted());
    assert_eq!(w.hub.handles.sounds.played(), vec![Chime::Finished]);
}

/// With the readings showing which window ran out, the limit is announced
/// under it; the window back under its limit before it resets (an early
/// reset) makes using it up again a new limit.
#[test]
fn a_limit_that_lifted_early_is_announced_again() {
    let w = world();
    w.send("UserPromptSubmit", "s1", json!({"prompt": "go"}));
    w.status_line("s1", 100);
    assert!(eventually(|| {
        w.hub.sync();
        w.shown()
            .rings
            .iter()
            .any(|ring| ring.usage.windows.iter().any(|win| win.used >= 1.0))
    }));
    w.hit_limit("s1");
    w.settle_burst();
    assert_eq!(w.limit_toasts(), 1, "{:?}", w.hub.handles.notifier.posted());
    assert!(eventually(|| w.announcements().is_some_and(
        |record| record.contains("\"window\":\"session\"") && record.contains("\"notification\"")
    )));
    let chimes = w.hub.handles.sounds.played().len();
    assert_eq!(chimes, 1);

    // Still the same limit while the window stays used up.
    w.hit_limit("s1");
    w.quiet();
    assert_eq!(w.limit_toasts(), 1);

    // An early reset: a later status line reads the window far lower.
    w.advance(Duration::from_secs(60));
    w.status_line("s1", 20);
    assert!(eventually(|| {
        w.hub.sync();
        w.announcements()
            .is_some_and(|record| !record.contains("\"window\":\"session\""))
    }));
    w.hit_limit("s2");
    w.quiet();
    assert_eq!(w.limit_toasts(), 2, "{:?}", w.hub.handles.notifier.posted());
    assert_eq!(w.hub.handles.sounds.played().len(), 2);
}

/// A turn fails on the limit before any reading says which window ran out;
/// the reading that comes after names it, and the announcement then lasts
/// until that window resets, not an hour.
#[test]
fn a_limit_announced_before_the_readings_learns_its_window() {
    let w = world();
    w.hit_limit("s1");
    w.settle_burst();
    assert!(eventually(|| {
        w.announcements()
            .is_some_and(|record| record.contains("\"window\":\"unknown\""))
    }));
    w.status_line("s1", 100);
    assert!(
        eventually(|| {
            w.hub.sync();
            w.announcements().is_some_and(|record| {
                record.contains("\"window\":\"session\"") && record.contains("\"resetsAt\"")
            })
        }),
        "{:?}",
        w.announcements()
    );
    // Still one limit: nothing more is announced.
    w.hit_limit("s2");
    w.quiet();
    assert_eq!(w.limit_toasts(), 1);
    assert_eq!(w.hub.handles.sounds.played().len(), 1);
}

/// A later reading of the same window whose reset time is a few seconds off
/// (two sources rounding it differently) shows the reset time the ring had:
/// one window, one reset. A new window passes through.
#[test]
fn a_ring_window_keeps_the_reset_time_it_was_shown_with() {
    let w = world();
    let session_reset = || {
        w.hub.sync();
        w.shown()
            .rings
            .iter()
            .flat_map(|ring| ring.usage.windows.clone())
            .find(|win| win.id == "session")
            .and_then(|win| win.resets_at)
    };
    w.send("UserPromptSubmit", "s1", json!({"prompt": "go"}));
    let first = (w.hub.handles.clock.now_ms() / 1000 + 3_600) * 1000;
    w.status_line("s1", 40);
    assert!(eventually(|| session_reset() == Some(first)));

    // Thirty seconds on, the same window read as resetting thirty seconds
    // later.
    w.advance(Duration::from_secs(30));
    w.status_line("s1", 50);
    assert!(eventually(|| {
        w.hub.sync();
        w.shown()
            .rings
            .iter()
            .flat_map(|ring| ring.usage.windows.clone())
            .any(|win| win.id == "session" && (win.used - 0.5).abs() < 1e-9)
    }));
    assert_eq!(session_reset(), Some(first));
}
