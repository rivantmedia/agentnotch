//! Typing a reply into a session's console: what may be typed, when, and
//! into which console. Ported from `Fix_MessagingSafetyTests`,
//! `A3_FocusAndMessagingTests` (`dialogsBlockTyping`,
//! `theProcessMustOwnItsTerminalsForeground`) and `TerminalFocusTests`
//! (`singleLineCollapsesLineBreaksAndDropsControls`,
//! `sendRejectsEmptyMessagesAndBadTTYs`), plus the Windows console vectors of
//! design §4.8.
//!
//! Mapped, not ported one to one:
//! - `SessionMessenger.waitUntilFree` is the hub's loop around `hold`: one
//!   look per `HOLD_POLL`, given how long the send has been held. The tests
//!   walk that loop by hand instead of sleeping.
//! - `aQueuedScriptIsCheckedAgainOnceItsTurnComes`: Windows has no script
//!   queue; its counterpart is the two-phase `type`, whose Return waits on a
//!   fresh `message_safety` (`Input::TypeCheckpoint`).
//! - `theProcessMustOwnItsTerminalsForeground`: a TTY and a foreground
//!   process group become the console Claude is attached to, its window and
//!   the processes sharing it, and its input mode.
//!
//! Skipped (no Windows counterpart):
//! - `tmuxEndsTheOptionsBeforeTheMessage`: there is no tmux route; the text
//!   goes into the console input buffer, never through a command line.
//! - The TTY half of `sendRejectsEmptyMessagesAndBadTTYs` and the "suspended
//!   (Ctrl+Z)" vector: Windows has no TTY paths and no job control.

mod control_support;

use agentnotch_engine::control::message_safety as reexported_safety;
use agentnotch_engine::control::messaging::*;
use agentnotch_engine::control::text::single_line;
use agentnotch_engine::model::*;
use agentnotch_engine::platform::{
    ConsoleInfo, ConsoleInput, HostApp, HostKind, ProcessTable, TypeOutcome,
};
use agentnotch_engine::testkit::terminal::FakeConsole;
use control_support::*;
use serde_json::json;
use std::time::Duration;

const QUESTION: &str = "Answer the question in the terminal or above";
const PLAN: &str = "Approve or reject the plan first";
const PERMISSION: &str = "Answer the permission prompt first";
const DIALOG: &str = "Answer in the terminal: Claude is showing a prompt";
const WORKING_COPY: &str = "Claude is working: send when it's done";
const TOOL_COPY: &str = "Claude is running a tool: send when it's done";

fn hooks(main: bool, any: bool) -> ToolActivity {
    ToolActivity {
        hook_backed: true,
        main_tool_in_flight: main,
        any_tool_in_flight: main || any,
    }
}

fn no_hooks() -> ToolActivity {
    ToolActivity {
        hook_backed: false,
        main_tool_in_flight: false,
        any_tool_in_flight: false,
    }
}

fn terminal() -> HostApp {
    host(HostKind::WindowsTerminal)
}

/// Why typing into `view` would be refused now, in a console of its own.
fn blocked(view: &SessionView) -> Option<String> {
    block_reason(view, &console(4242), &terminal())
}

fn target(info: &ConsoleInfo, table: &ProcessTable) -> Result<Vec<u32>, String> {
    console_target(&view("s1"), info, table).map(|target| target.allowed_shells)
}

/// A request held for an agent of the session, not the session itself.
fn agent_request(tool: &str) -> PendingRequest {
    PendingRequest {
        agent_id: Some("agent-1".into()),
        ..request("s1", tool, json!({}), &[])
    }
}

// ---- Fix_MessagingSafetyTests ----

#[test]
fn a_main_session_tool_in_flight_holds_typing() {
    let working = SessionState::Working;
    assert_eq!(
        busy_reason(&working, Some(&hooks(true, false))).as_deref(),
        Some(TOOL_COPY)
    );
    // Between tools (thinking), with hooks, typing may go ahead.
    assert_eq!(busy_reason(&working, Some(&hooks(false, false))), None);
    // A subagent's tool while the turn runs holds it too.
    assert_eq!(
        busy_reason(&working, Some(&hooks(false, true))).as_deref(),
        Some(TOOL_COPY)
    );
    // A background agent's call after the turn ended does not.
    assert_eq!(
        busy_reason(&SessionState::ReadyForReview, Some(&hooks(false, true))),
        None
    );
    assert_eq!(
        busy_reason(&SessionState::Idle, Some(&hooks(false, false))),
        None
    );
    // A main-session tool holds it whatever the state says.
    assert_eq!(
        busy_reason(&SessionState::Idle, Some(&hooks(true, false))).as_deref(),
        Some(TOOL_COPY)
    );
}

#[test]
fn without_hooks_the_whole_turn_holds_typing() {
    for tools in [None, Some(no_hooks())] {
        let tools = tools.as_ref();
        assert_eq!(
            busy_reason(&SessionState::Working, tools).as_deref(),
            Some(WORKING_COPY)
        );
        assert_eq!(busy_reason(&SessionState::ReadyForReview, tools), None);
        assert_eq!(busy_reason(&SessionState::Idle, tools), None);
    }
    // Tools that aren't hook-backed say nothing: the turn is what counts.
    let unannounced = ToolActivity {
        hook_backed: false,
        main_tool_in_flight: true,
        any_tool_in_flight: true,
    };
    assert_eq!(busy_reason(&SessionState::Idle, Some(&unannounced)), None);
    // `message_safety` doesn't know the tools: it waits out the turn.
    let working = in_state(view("s1"), SessionState::Working);
    assert_eq!(
        message_safety(&working, &console(4242), &terminal()),
        Err(WORKING_COPY.to_owned())
    );
    assert_eq!(
        reexported_safety(&view("s1"), &console(4242), &terminal()),
        Ok(())
    );
}

/// The Windows counterpart of the queued script checked again once its turn
/// comes: the text is typed, then a fresh `message_safety` on the session as
/// it is by then decides whether Return is pressed.
#[test]
fn a_queued_script_is_checked_again_once_its_turn_comes() {
    let info = console(4242);
    let host = terminal();
    let before = view("s1");
    assert_eq!(message_safety(&before, &info, &host), Ok(()));
    let target = console_target(&before, &info, &shell_chain()).unwrap();

    // A background agent asked for a permission between the text and Return.
    let mut after = before.clone();
    after.pending = vec![agent_request("Bash")];
    let fake = FakeConsole::default();
    let outcome = fake.type_text(&target, "fix it", &mut || {
        message_safety(&after, &info, &host).is_ok()
    });
    assert_eq!(
        outcome,
        TypeOutcome::TypedNotSubmitted(TYPED_NOT_SUBMITTED.into())
    );
    assert_eq!(fake.typed(), vec![(target.clone(), "fix it".into(), false)]);

    // Nothing new: Return goes.
    let clear = FakeConsole::default();
    let outcome = clear.type_text(&target, "fix it", &mut || {
        message_safety(&before, &info, &host).is_ok()
    });
    assert_eq!(outcome, TypeOutcome::Delivered);
    assert_eq!(clear.typed(), vec![(target, "fix it".into(), true)]);

    // Every way a dialog can show up in the gap refuses Return.
    let mut registry = before.clone();
    registry.registry_status = Some("waiting".into());
    let approval = waiting_on(before.clone(), "Bash", json!({"command": "ls"}));
    let mut ended = before.clone();
    ended.phase = Phase::Ended;
    for fresh in [registry, approval, ended] {
        assert!(message_safety(&fresh, &info, &host).is_err(), "{fresh:?}");
    }
}

/// One look per poll, as the hub's loop around `hold` takes them.
fn walk(checks: &[Result<(), TypingRefusal>]) -> Vec<Hold> {
    checks
        .iter()
        .enumerate()
        .map(|(index, check)| hold(check.clone(), HOLD_POLL * index as u32))
        .collect()
}

fn busy() -> Result<(), TypingRefusal> {
    Err(TypingRefusal {
        reason: TOOL_COPY.into(),
        busy: true,
    })
}

#[test]
fn a_send_waits_out_a_tool_then_goes_ahead() {
    assert_eq!(HOLD_POLL, Duration::from_millis(250));
    assert_eq!(
        walk(&[busy(), busy(), Ok(())]),
        [Hold::Wait, Hold::Wait, Hold::Go]
    );
    // On the session itself: a tool in flight, then done.
    let session = in_state(view("s1"), SessionState::Working);
    let (info, host) = (console(4242), terminal());
    let running = typing_check(&session, &info, &host, Some(&hooks(true, false)));
    assert_eq!(running, busy());
    assert_eq!(hold(running, Duration::ZERO), Hold::Wait);
    let between = typing_check(&session, &info, &host, Some(&hooks(false, false)));
    assert_eq!(hold(between, HOLD_POLL), Hold::Go);
}

#[test]
fn a_send_that_stays_busy_is_refused_after_the_limit() {
    assert_eq!(HOLD_LIMIT, Duration::from_secs(10));
    assert_eq!(hold(busy(), Duration::ZERO), Hold::Wait);
    assert_eq!(hold(busy(), Duration::from_millis(9_750)), Hold::Wait);
    assert_eq!(
        hold(busy(), Duration::from_secs(10)),
        Hold::Refuse(TOOL_COPY.into())
    );
    assert_eq!(hold(Ok(()), Duration::from_secs(10)), Hold::Go);
    // Held all the way: 40 looks wait, the 41st (at 10 s) refuses.
    let looks = walk(&vec![busy(); 41]);
    assert!(looks[..40].iter().all(|look| *look == Hold::Wait));
    assert_eq!(looks[40], Hold::Refuse(TOOL_COPY.into()));
}

#[test]
fn a_dialog_refuses_at_once() {
    // A permission dialog follows a tool already in flight: the dialog wins,
    // and it is final.
    let session = waiting_on(view("s1"), "Bash", json!({"command": "ls"}));
    let check = typing_check(
        &session,
        &console(4242),
        &terminal(),
        Some(&hooks(true, true)),
    );
    assert_eq!(
        check,
        Err(TypingRefusal {
            reason: PERMISSION.into(),
            busy: false
        })
    );
    assert_eq!(hold(check, Duration::ZERO), Hold::Refuse(PERMISSION.into()));
}

#[test]
fn an_unknown_session_is_never_typed_into() {
    let (info, host) = (console(4242), terminal());
    // No longer tracked.
    assert_eq!(
        availability(true, false, None, &info, &host),
        Err(SESSION_ENDED.into())
    );
    // No pid.
    let mut no_pid = view("s1");
    no_pid.pid = None;
    assert_eq!(blocked(&no_pid), Some(PROCESS_UNKNOWN.into()));
    assert_eq!(
        console_target(&no_pid, &info, &shell_chain()),
        Err(PROCESS_UNKNOWN.into())
    );
    // Ended.
    let mut ended = view("s1");
    ended.phase = Phase::Ended;
    assert_eq!(blocked(&ended), Some(SESSION_ENDED.into()));
    assert_eq!(
        availability(true, false, Some(&ended), &info, &host),
        Err(SESSION_ENDED.into())
    );
    // Its process is gone from the console it had.
    assert_eq!(
        block_reason(&view("s1"), &console_with(9999, &[]), &host),
        Some(NOT_CONFIRMED.into())
    );
}

// ---- A3_FocusAndMessagingTests ----

#[test]
fn dialogs_block_typing() {
    let blocked_by = [
        (permission("Bash"), PERMISSION),
        (
            SessionState::NeedsYou(NeedsInputReason::Permission { tool: None }),
            PERMISSION,
        ),
        (SessionState::NeedsYou(NeedsInputReason::Question), QUESTION),
        (SessionState::NeedsYou(NeedsInputReason::PlanApproval), PLAN),
        (
            SessionState::NeedsYou(NeedsInputReason::Elicitation {
                message: String::new(),
            }),
            DIALOG,
        ),
        (
            SessionState::NeedsYou(NeedsInputReason::Dialog {
                detail: "waiting".into(),
            }),
            DIALOG,
        ),
    ];
    for (state, copy) in blocked_by {
        let session = in_state(view("s1"), state.clone());
        assert_eq!(blocked(&session).as_deref(), Some(copy), "{state:?}");
    }
    // A failed turn has nothing open: the prompt is back.
    for state in [
        failed("Rate limited", "rate_limit"),
        SessionState::NeedsYou(NeedsInputReason::Error {
            text: "Rate limited".into(),
            code: None,
        }),
        SessionState::ReadyForReview,
        SessionState::Idle,
    ] {
        assert_eq!(
            blocked(&in_state(view("s1"), state.clone())),
            None,
            "{state:?}"
        );
    }

    // A pending request of a background agent blocks too, after the turn.
    for (tool, copy) in [
        ("Bash", PERMISSION),
        ("AskUserQuestion", QUESTION),
        ("ExitPlanMode", PLAN),
    ] {
        let mut session = in_state(view("s1"), SessionState::ReadyForReview);
        session.pending = vec![agent_request(tool)];
        assert_eq!(blocked(&session).as_deref(), Some(copy), "{tool}");
    }
    // An approval the phase holds while the state lags behind.
    let mut lagging = waiting_on(view("s1"), "AskUserQuestion", json!({}));
    lagging.state = SessionState::Working;
    lagging.pending.clear();
    assert_eq!(blocked(&lagging).as_deref(), Some(QUESTION));
    // A dialog only the registry knows about.
    let mut registry = view("s1");
    registry.registry_status = Some("waiting".into());
    assert_eq!(blocked(&registry).as_deref(), Some(DIALOG));
    registry.registry_status = Some("busy".into());
    assert_eq!(blocked(&registry), None);
    // A dialog outranks everything else: nothing about the console is asked.
    let mut gone = waiting_on(view("s1"), "Bash", json!({}));
    gone.phase = Phase::Ended;
    gone.pid = None;
    assert_eq!(blocked(&gone).as_deref(), Some(PERMISSION));
}

/// The Windows vectors of `theProcessMustOwnItsTerminalsForeground`: the
/// console must be attached, Claude's, shared with nobody but Claude's own
/// processes and the shells that launched it, and read in raw mode.
#[test]
fn the_process_must_own_its_terminals_foreground() {
    let session = view("s1");
    let wt = terminal();
    let chain = shell_chain();

    // Not attached: an extension or SDK session, a Git Bash window.
    let detached = ConsoleInfo {
        attached: false,
        processes: Vec::new(),
        window: None,
        ..console(4242)
    };
    assert_eq!(
        block_reason(&session, &detached, &wt),
        Some(NO_CONSOLE.into())
    );
    assert_eq!(
        console_target(&session, &detached, &chain),
        Err(NO_CONSOLE.into())
    );
    assert_eq!(
        block_reason(&session, &console(4242), &host(HostKind::NoConsole)),
        Some(NO_CONSOLE.into())
    );
    // Not attached because Claude runs elevated.
    let elevated = ConsoleInfo {
        elevated_target: true,
        ..detached.clone()
    };
    assert_eq!(
        block_reason(&session, &elevated, &wt),
        Some(ELEVATED.into())
    );
    assert_eq!(
        console_target(&session, &elevated, &chain),
        Err(ELEVATED.into())
    );

    // The console window isn't the one recorded for the session.
    let recorded = console(4242).window;
    let moved = ConsoleInfo {
        window: Some(0x60_0B34),
        ..console(4242)
    };
    assert_eq!(
        console_target_with(&session, &moved, &chain, recorded),
        Err(NOT_CONFIRMED.into())
    );
    let unnamed = ConsoleInfo {
        window: None,
        ..console(4242)
    };
    assert_eq!(
        console_target_with(&session, &unnamed, &chain, recorded),
        Err(NOT_CONFIRMED.into())
    );
    assert!(console_target_with(&session, &console(4242), &chain, recorded).is_ok());

    // Claude and its descendants and its direct shell chain: fine.
    let family = console_with(4242, &[300, 200, 5000, 5001]);
    assert_eq!(block_reason(&session, &family, &wt), None);
    assert_eq!(target(&family, &chain), Ok(vec![300, 200]));
    // Plus a foreign process.
    let foreign = console_with(4242, &[300, 777]);
    assert_eq!(target(&foreign, &chain), Err(OTHER_READER.into()));
    // Plus a non-shell ancestor.
    // (Explorer, which opened the console window.)
    let explorer = console_with(4242, &[300, 200, 100]);
    assert_eq!(target(&explorer, &chain), Err(OTHER_READER.into()));
    // Claude run by node.exe from a shell: the chain stops at node, so
    // neither node nor the shell above it may share the console.
    let wrapped = table(vec![
        proc_entry(200, 100, "cmd.exe", -500),
        proc_entry(250, 200, "node.exe", -100),
        proc_entry(4242, 250, "claude.exe", 0),
    ]);
    assert_eq!(allowed_shells(&wrapped, 4242), Vec::<u32>::new());
    assert_eq!(
        target(&console_with(4242, &[250]), &wrapped),
        Err(OTHER_READER.into())
    );
    assert_eq!(
        target(&console_with(4242, &[200]), &wrapped),
        Err(OTHER_READER.into())
    );
    // A shell above a non-shell link above a shell: only the nearest counts.
    let broken = table(vec![
        proc_entry(200, 100, "cmd.exe", -500),
        proc_entry(250, 200, "node.exe", -300),
        proc_entry(300, 250, "pwsh.exe", -100),
        proc_entry(4242, 300, "claude.exe", 0),
    ]);
    assert_eq!(allowed_shells(&broken, 4242), vec![300]);
    assert_eq!(target(&console_with(4242, &[300]), &broken), Ok(vec![300]));
    assert_eq!(
        target(&console_with(4242, &[300, 200]), &broken),
        Err(OTHER_READER.into())
    );
    // A "parent" younger than Claude holds a reused pid: not Claude's shell.
    let stale = table(vec![
        proc_entry(300, 200, "pwsh.exe", 60),
        proc_entry(4242, 300, "claude.exe", 0),
    ]);
    assert_eq!(allowed_shells(&stale, 4242), Vec::<u32>::new());
    assert_eq!(
        target(&console_with(4242, &[300]), &stale),
        Err(OTHER_READER.into())
    );
    // Likewise a "child" older than Claude.
    let mut old_child = chain.clone();
    old_child
        .entries
        .push(proc_entry(6000, 4242, "cmd.exe", -10));
    assert_eq!(
        target(&console_with(4242, &[6000]), &old_child),
        Err(OTHER_READER.into())
    );
    // Shell names match exactly, in any case.
    let shouting = table(vec![
        proc_entry(200, 100, "CMD.EXE", -500),
        proc_entry(300, 200, "PowerShell.exe", -400),
        proc_entry(4242, 300, "claude.exe", 0),
    ]);
    assert_eq!(allowed_shells(&shouting, 4242), vec![300, 200]);
    let lookalike = table(vec![
        proc_entry(300, 200, "mycmd.exe", -400),
        proc_entry(4242, 300, "claude.exe", 0),
    ]);
    assert_eq!(allowed_shells(&lookalike, 4242), Vec::<u32>::new());

    // A console reading in line mode is at a shell's prompt.
    let cooked = ConsoleInfo {
        line_input: Some(true),
        ..console(4242)
    };
    assert_eq!(
        block_reason(&session, &cooked, &wt),
        Some(NOT_AT_PROMPT.into())
    );
    assert_eq!(target(&cooked, &chain), Err(NOT_AT_PROMPT.into()));
    // A mode that couldn't be read is not raw mode.
    let unknown = ConsoleInfo {
        line_input: None,
        ..console(4242)
    };
    assert_eq!(
        block_reason(&session, &unknown, &wt),
        Some(NOT_CONFIRMED.into())
    );
    assert_eq!(target(&unknown, &chain), Err(NOT_CONFIRMED.into()));
}

#[test]
fn availability_is_opt_in_and_never_with_samples() {
    let (session, info, host) = (view("s1"), console(4242), terminal());
    assert_eq!(
        availability(false, false, Some(&session), &info, &host),
        Err(TYPING_OFF.into())
    );
    assert_eq!(
        availability(true, true, Some(&session), &info, &host),
        Err(SEALED.into())
    );
    // Sample sessions never point at Settings.
    assert_eq!(
        availability(false, true, Some(&session), &info, &host),
        Err(SEALED.into())
    );
    assert_eq!(
        availability(true, false, Some(&session), &info, &host),
        Ok(())
    );
    // A tool in flight keeps the composer: the send is held, not refused.
    let working = in_state(session, SessionState::Working);
    assert_eq!(
        availability(true, false, Some(&working), &info, &host),
        Ok(())
    );
    // The route sentence of design §4.8, word for word.
    assert_eq!(
        ROUTE_SENTENCE,
        "Replies can be typed from here for sessions in Windows Terminal, VS Code's terminal and console windows."
    );
    assert_eq!(
        TYPING_OFF,
        "Typing replies is off. Turn it on in Settings › Claude Code."
    );
}

// ---- The console target ----

#[test]
fn the_console_target_names_claude_its_window_and_its_shells() {
    let session = view("s1");
    let info = console_with(4242, &[300, 200]);
    let made = console_target(&session, &info, &shell_chain()).unwrap();
    assert_eq!(made.claude_pid, 4242);
    assert_eq!(made.claude_started, t0());
    assert_eq!(made.expected_window, info.window);
    assert_eq!(made.allowed_shells, vec![300, 200]);

    // A recorded window is what the helper expects.
    let recorded = console_target_with(&session, &info, &shell_chain(), info.window).unwrap();
    assert_eq!(recorded, made);
    // No window known at all: the helper is told none.
    let windowless = ConsoleInfo {
        window: None,
        ..info.clone()
    };
    let made = console_target(&session, &windowless, &shell_chain()).unwrap();
    assert_eq!(made.expected_window, None);

    // The start time comes from the session only. The table knows when the
    // process under that pid now started, but that is no proof it is the
    // session's: a reused pid would be checked against itself and the reply
    // typed into another program, maybe another Claude at its prompt.
    let mut unstarted = view("s1");
    unstarted.pid_started = None;
    assert_eq!(
        console_target(&unstarted, &info, &shell_chain()),
        Err(NOT_CONFIRMED.into())
    );
    assert_eq!(
        console_target_with(&unstarted, &info, &shell_chain(), info.window),
        Err(NOT_CONFIRMED.into())
    );
    // Nobody knows it: refused all the more.
    assert_eq!(
        console_target(&unstarted, &info, &ProcessTable::default()),
        Err(NOT_CONFIRMED.into())
    );
    // The table's process under Claude's pid started at another time: the
    // pid was reused, and its family is someone else's.
    let mut reused = shell_chain();
    for entry in &mut reused.entries {
        if entry.pid == 4242 {
            *entry = proc_entry(4242, 300, "notepad.exe", 30);
        }
    }
    assert_eq!(
        console_target(&session, &info, &reused),
        Err(NOT_CONFIRMED.into())
    );
    // Claude isn't attached to the console read.
    assert_eq!(
        console_target(&session, &console_with(9999, &[]), &shell_chain()),
        Err(NOT_CONFIRMED.into())
    );
}

// ---- Outcomes ----

#[test]
fn outcomes_name_themselves_and_keep_the_draft() {
    assert_eq!(outcome_reply(&TypeOutcome::Delivered), ("delivered", None));
    assert_eq!(
        outcome_reply(&TypeOutcome::Refused(OTHER_READER.into())),
        ("refused", Some(OTHER_READER.into()))
    );
    assert_eq!(
        outcome_reply(&TypeOutcome::TypedNotSubmitted(TYPED_NOT_SUBMITTED.into())),
        ("typed_not_submitted", Some(TYPED_NOT_SUBMITTED.into()))
    );
    assert_eq!(
        outcome_reply(&TypeOutcome::Failed("helper missing".into())),
        ("failed", Some("helper missing".into()))
    );

    assert_eq!(outcome_note(&TypeOutcome::Delivered), None);
    assert_eq!(
        outcome_note(&TypeOutcome::Refused(PERMISSION.into())).as_deref(),
        Some("Not sent: Answer the permission prompt first. Your message is kept.")
    );
    assert_eq!(
        outcome_note(&TypeOutcome::TypedNotSubmitted(TYPED_NOT_SUBMITTED.into())).as_deref(),
        Some(
            "Typed but not submitted: Claude asked for something while your reply was typed; \
             it's in the terminal, not sent. Press Enter in the terminal when it's safe."
        )
    );
    assert_eq!(
        outcome_note(&TypeOutcome::Failed("helper missing".into())).as_deref(),
        Some(FAILED_NOTE)
    );
}

// ---- TerminalFocusTests ----

#[test]
fn single_line_collapses_line_breaks_and_drops_controls() {
    assert_eq!(single_line("fix the\nbug\r\nnow"), "fix the bug  now");
    assert_eq!(single_line("tab\there"), "tab here");
    assert_eq!(single_line("bell\u{07}ring\u{1B}[0m"), "bellring[0m");
    assert_eq!(single_line("  padded  "), "padded");
    // Invisible format characters (a right-to-left override, a zero-width
    // space, a byte order mark) would hide what is sent.
    assert_eq!(single_line("rm \u{202E}fdp.x"), "rm fdp.x");
    assert_eq!(single_line("a\u{200B}b\u{FEFF}c"), "abc");
    assert_eq!(single_line("héllo wörld ✓"), "héllo wörld ✓");
}

#[test]
fn send_rejects_empty_messages() {
    for empty in ["", "   \n", " \r\n\t ", "\u{07}\u{1B}", " \u{200B} "] {
        assert_eq!(prepare(empty), Err(NOTHING_TO_SEND.into()), "{empty:?}");
    }
    // Claude Code's own commands pass through as typed.
    for command in ["/compact", "!ls -la", "# remember this"] {
        assert_eq!(prepare(command).as_deref(), Ok(command));
    }
    assert_eq!(prepare("  /clear \n").as_deref(), Ok("/clear"));
    assert_eq!(
        prepare("first\r\nsecond\tthird").as_deref(),
        Ok("first  second third")
    );
    assert_eq!(prepare("ok\u{07}\u{0}").as_deref(), Ok("ok"));
    // Characters outside the Basic Multilingual Plane stay whole.
    assert_eq!(prepare("ship it 🚀𝄞").as_deref(), Ok("ship it 🚀𝄞"));
}
