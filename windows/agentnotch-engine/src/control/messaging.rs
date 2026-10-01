//! Typing a chat reply into a session's terminal (HS§8, §4.8).
//!
//! The reply goes into the input buffer of the console Claude is attached
//! to (`agentnotch-hook.exe type`), never as keystrokes to whatever window
//! is in front. Typing is only safe when Claude Code's prompt is what reads
//! the text: Return answers whatever dialog the terminal shows (a permission
//! the app doesn't hold, a sandbox prompt, an elicitation) with its
//! highlighted option, and a shell sharing the console would run the text as
//! a command. So:
//!
//! - typing is opt-in (`typeReplies`, off by default on Windows);
//! - nothing is typed while the session waits on a dialog, of any agent;
//! - the console must be Claude's own: only Claude, its children and the
//!   shells that launched it may be attached, and it must read in raw mode
//!   (a shell prompt reads in line mode);
//! - a reply is held while a tool is in flight (dialogs open mid-tool), for
//!   a few seconds, then refused with the draft kept;
//! - and the whole check runs again between the text and Return
//!   (`Input::TypeCheckpoint`), on the session as it is then.

use super::hosts::is_shell;
use super::text::single_line;
use crate::model::{NeedsInputReason, Phase, RequestKind, SessionState, SessionView};
use crate::platform::{ConsoleInfo, ConsoleTarget, HostApp, HostKind, ProcessTable, TypeOutcome};
use std::collections::BTreeSet;
use std::time::Duration;

// ---- Copy ----

/// `message_route` and `send_message` while `typeReplies` is off.
pub const TYPING_OFF: &str = "Typing replies is off. Turn it on in Settings › Claude Code.";
/// The chat's note when nothing more specific is known.
pub const ROUTE_SENTENCE: &str =
    "Replies can be typed from here for sessions in Windows Terminal, VS Code's terminal and console windows.";
/// Sealed fixtures have no terminal to type into.
pub const SEALED: &str = "Not available with sample sessions";
pub const SESSION_ENDED: &str = "The session has ended";
pub const PROCESS_UNKNOWN: &str = "The session's process isn't known";
pub const NOTHING_TO_SEND: &str = "Nothing to send";
/// The VS Code extension, an SDK host, a Git Bash (mintty) window.
pub const NO_CONSOLE: &str = "This session has no console to type into";
pub const ELEVATED: &str = "Claude runs as administrator";
pub const NOT_CONFIRMED: &str = "The session's terminal can't be confirmed";
pub const OTHER_READER: &str = "Another program is reading this console";
pub const NOT_AT_PROMPT: &str = "The terminal isn't at Claude Code's prompt";
/// The text was typed and Return was not pressed.
pub const TYPED_NOT_SUBMITTED: &str =
    "Claude asked for something while your reply was typed; it's in the terminal, not sent.";

const ANSWER_QUESTION: &str = "Answer the question in the terminal or above";
const ANSWER_PLAN: &str = "Approve or reject the plan first";
const ANSWER_PERMISSION: &str = "Answer the permission prompt first";
const ANSWER_DIALOG: &str = "Answer in the terminal: Claude is showing a prompt";
const WORKING: &str = "Claude is working: send when it's done";
const RUNNING_TOOL: &str = "Claude is running a tool: send when it's done";

/// How long a send made while a tool runs waits for it to finish.
pub const HOLD_LIMIT: Duration = Duration::from_secs(10);
pub const HOLD_POLL: Duration = Duration::from_millis(250);
/// How long a session's console facts are reused (per pid and start time).
pub const CONSOLE_INFO_LIFETIME: Duration = Duration::from_secs(30);

// ---- The message ----

/// The reply as it is typed: trimmed, on one line, control characters gone.
/// There is no escaping of a leading `/`, `!` or `#`: they act as Claude
/// Code's own commands, as they would typed by hand.
pub fn prepare(text: &str) -> Result<String, String> {
    let line = single_line(text.trim());
    if line.is_empty() {
        return Err(NOTHING_TO_SEND.into());
    }
    Ok(line)
}

// ---- Safety ----

/// What the engine knows about the tools a session runs. `SessionView`
/// doesn't carry it; without it a reply waits out the whole turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolActivity {
    /// Hooks have spoken for this session, so every tool is announced.
    pub hook_backed: bool,
    /// A tool of the main session is in flight.
    pub main_tool_in_flight: bool,
    /// A tool of the main session or of any agent is in flight.
    pub any_tool_in_flight: bool,
}

/// Why a reply was not typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypingRefusal {
    /// Short and user-facing, without a final stop (the chat appends one).
    pub reason: String,
    /// Claude is only busy: a send is held and asked again, for up to
    /// [`HOLD_LIMIT`]. Anything else is final.
    pub busy: bool,
}

fn dialog_of(reason: &NeedsInputReason) -> Option<&'static str> {
    match reason {
        NeedsInputReason::Question => Some(ANSWER_QUESTION),
        NeedsInputReason::PlanApproval => Some(ANSWER_PLAN),
        NeedsInputReason::Permission { .. } => Some(ANSWER_PERMISSION),
        NeedsInputReason::Elicitation { .. } | NeedsInputReason::Dialog { .. } => {
            Some(ANSWER_DIALOG)
        }
        // A failed turn has nothing open: the prompt is back.
        NeedsInputReason::Error { .. } => None,
    }
}

/// The dialog the terminal shows, if any: the session's own, a request any
/// of its agents holds, or one only the registry knows about.
fn open_dialog(view: &SessionView) -> Option<&'static str> {
    if let Some(dialog) = view.state.reason().and_then(dialog_of) {
        return Some(dialog);
    }
    if let Phase::WaitingForApproval(context) = &view.phase {
        return dialog_of(&NeedsInputReason::for_approval(&context.tool_name));
    }
    if let Some(request) = view.pending.first() {
        return Some(match request.kind {
            RequestKind::Question => ANSWER_QUESTION,
            RequestKind::Plan => ANSWER_PLAN,
            RequestKind::Permission => ANSWER_PERMISSION,
        });
    }
    (view.registry_status.as_deref() == Some("waiting")).then_some(ANSWER_DIALOG)
}

/// Why typing into the terminal would not reach Claude's prompt, or `None`
/// when it would.
pub fn block_reason(view: &SessionView, info: &ConsoleInfo, host: &HostApp) -> Option<String> {
    if let Some(dialog) = open_dialog(view) {
        return Some(dialog.into());
    }
    if view.phase == Phase::Ended {
        return Some(SESSION_ENDED.into());
    }
    let Some(pid) = view.pid else {
        return Some(PROCESS_UNKNOWN.into());
    };
    if info.elevated_target {
        return Some(ELEVATED.into());
    }
    if host.kind == HostKind::NoConsole || !info.attached {
        return Some(NO_CONSOLE.into());
    }
    if !info.processes.contains(&pid) {
        return Some(NOT_CONFIRMED.into());
    }
    input_mode_refusal(info).map(str::to_owned)
}

/// Claude Code reads its console in raw mode; a shell prompt reads in line
/// mode. A mode that couldn't be read is not taken for raw mode: Return
/// could then run the text as a command.
fn input_mode_refusal(info: &ConsoleInfo) -> Option<&'static str> {
    match info.line_input {
        Some(false) => None,
        Some(true) => Some(NOT_AT_PROMPT),
        None => Some(NOT_CONFIRMED),
    }
}

/// Why typing must wait for Claude, or `None`. A permission, question or
/// plan dialog only ever opens while a tool call is in flight (its
/// PreToolUse has already been heard), so with hooks a reply waits out the
/// main session's tool calls, and any call while the turn runs. Without
/// hooks (or without knowing the tools) nothing announces one, so it waits
/// out the whole turn.
pub fn busy_reason(state: &SessionState, tools: Option<&ToolActivity>) -> Option<String> {
    let working = *state == SessionState::Working;
    match tools.filter(|tools| tools.hook_backed) {
        None => working.then(|| WORKING.to_owned()),
        Some(tools) => (tools.main_tool_in_flight || (working && tools.any_tool_in_flight))
            .then(|| RUNNING_TOOL.to_owned()),
    }
}

/// Both checks, on the session as the engine has it now: what the
/// last-moment re-check asks between the text and Return.
pub fn typing_check(
    view: &SessionView,
    info: &ConsoleInfo,
    host: &HostApp,
    tools: Option<&ToolActivity>,
) -> Result<(), TypingRefusal> {
    if let Some(reason) = block_reason(view, info, host) {
        return Err(TypingRefusal {
            reason,
            busy: false,
        });
    }
    match busy_reason(&view.state, tools) {
        Some(reason) => Err(TypingRefusal { reason, busy: true }),
        None => Ok(()),
    }
}

/// [`typing_check`] for a session whose running tools aren't known: a reply
/// then waits out the whole turn.
pub fn message_safety(
    view: &SessionView,
    info: &ConsoleInfo,
    host: &HostApp,
) -> Result<(), String> {
    typing_check(view, info, host, None).map_err(|refusal| refusal.reason)
}

/// Whether the chat offers its composer (`message_route`): typing is on, and
/// nothing blocks it now. Not "busy": the composer stays while a tool runs,
/// and a send made then is held.
///
/// `view` is `None` for a session the engine no longer tracks.
pub fn availability(
    type_replies: bool,
    sealed: bool,
    view: Option<&SessionView>,
    info: &ConsoleInfo,
    host: &HostApp,
) -> Result<(), String> {
    // Sealed first, as on the Mac: sample sessions have no terminal, whatever
    // the setting says.
    if sealed {
        return Err(SEALED.into());
    }
    if !type_replies {
        return Err(TYPING_OFF.into());
    }
    let Some(view) = view else {
        return Err(SESSION_ENDED.into());
    };
    match block_reason(view, info, host) {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

// ---- Holding a send ----

/// What to do with a send that was just checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// Type it.
    Go,
    /// Claude is busy: ask again after [`HOLD_POLL`].
    Wait,
    /// Not typed, for this reason; the draft is kept.
    Refuse(String),
}

/// One look at a held send: `waited` is how long it has been held.
pub fn hold(check: Result<(), TypingRefusal>, waited: Duration) -> Hold {
    match check {
        Ok(()) => Hold::Go,
        // Anything but a running tool (a dialog, a lost console) is final.
        Err(refusal) if refusal.busy && waited < HOLD_LIMIT => Hold::Wait,
        Err(refusal) => Hold::Refuse(refusal.reason),
    }
}

// ---- The console ----

/// Claude's direct parent chain of known shells, nearest first: the only
/// other processes that may be attached to its console. It stops at the
/// first ancestor that isn't a shell, and a parent link counts only when
/// the parent is older than its child.
pub fn allowed_shells(table: &ProcessTable, claude_pid: u32) -> Vec<u32> {
    // Deep enough for `cmd /c` wrappers around a shell around a shell.
    const MAX_SHELLS: usize = 8;
    table
        .ancestors(claude_pid, MAX_SHELLS)
        .into_iter()
        .take_while(|ancestor| is_shell(&ancestor.exe_name))
        .map(|ancestor| ancestor.pid)
        .collect()
}

fn descendants(table: &ProcessTable, root: u32) -> BTreeSet<u32> {
    let mut found = BTreeSet::from([root]);
    // Parents can come after their children in the table: repeat until
    // nothing new joins.
    loop {
        let before = found.len();
        for entry in &table.entries {
            if !found.contains(&entry.pid)
                && table
                    .parent(entry.pid)
                    .is_some_and(|parent| found.contains(&parent.pid))
            {
                found.insert(entry.pid);
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Where a reply is typed, or why it can't be: the console must hold only
/// Claude, its descendants and the shells that launched it. A shell that
/// reads the same console at its own prompt (Claude started in the
/// background of it) would run the reply as a command.
///
/// The helper is told to expect the console window `info` names.
pub fn console_target(
    view: &SessionView,
    info: &ConsoleInfo,
    table: &ProcessTable,
) -> Result<ConsoleTarget, String> {
    console_target_with(view, info, table, None)
}

/// [`console_target`] for a session whose console window was recorded
/// earlier (when the session was first seen): `info` must still name that
/// window, the Windows counterpart of the Mac's "the process's TTY is the
/// session's". The helper is then told to expect the recorded window.
pub fn console_target_with(
    view: &SessionView,
    info: &ConsoleInfo,
    table: &ProcessTable,
    recorded_window: Option<u64>,
) -> Result<ConsoleTarget, String> {
    let pid = view.pid.ok_or(PROCESS_UNKNOWN)?;
    if info.elevated_target {
        return Err(ELEVATED.into());
    }
    if !info.attached {
        return Err(NO_CONSOLE.into());
    }
    if recorded_window.is_some() && info.window != recorded_window {
        return Err(NOT_CONFIRMED.into());
    }
    // The start time pairs with the pid everywhere: Windows reuses pids.
    let claude_started = view
        .pid_started
        .or_else(|| table.get(pid).and_then(|entry| entry.started))
        .ok_or(NOT_CONFIRMED)?;
    // The table's process under that pid must be the session's own, or its
    // parents and children are some other program's.
    let reused = table
        .get(pid)
        .and_then(|entry| entry.started)
        .is_some_and(|started| started != claude_started);
    if reused || !info.processes.contains(&pid) {
        return Err(NOT_CONFIRMED.into());
    }
    let allowed_shells = allowed_shells(table, pid);
    let family = descendants(table, pid);
    let stranger = info
        .processes
        .iter()
        .any(|attached| !family.contains(attached) && !allowed_shells.contains(attached));
    if stranger {
        return Err(OTHER_READER.into());
    }
    if let Some(refusal) = input_mode_refusal(info) {
        return Err(refusal.into());
    }
    Ok(ConsoleTarget {
        claude_pid: pid,
        claude_started,
        expected_window: recorded_window.or(info.window),
        allowed_shells,
    })
}

/// The `send_message` call's outcome name and reason.
pub fn outcome_reply(outcome: &TypeOutcome) -> (&'static str, Option<String>) {
    match outcome {
        TypeOutcome::Delivered => ("delivered", None),
        TypeOutcome::Refused(reason) => ("refused", Some(reason.clone())),
        TypeOutcome::TypedNotSubmitted(reason) => ("typed_not_submitted", Some(reason.clone())),
        TypeOutcome::Failed(reason) => ("failed", Some(reason.clone())),
    }
}

/// What the chat shows under the composer after a send, `None` once it was
/// delivered (ChatComposerCopy on the Mac). Reasons carry no final stop
/// here; the helper's own may, and it is dropped so the copy reads once.
pub fn outcome_note(outcome: &TypeOutcome) -> Option<String> {
    let bare = |reason: &str| reason.trim_end().trim_end_matches('.').to_owned();
    match outcome {
        TypeOutcome::Delivered => None,
        TypeOutcome::Refused(reason) => {
            Some(format!("Not sent: {}. Your message is kept.", bare(reason)))
        }
        TypeOutcome::TypedNotSubmitted(reason) => Some(format!(
            "Typed but not submitted: {}. Press Enter in the terminal when it's safe.",
            bare(reason)
        )),
        TypeOutcome::Failed(_) => Some(FAILED_NOTE.to_owned()),
    }
}

/// The helper couldn't be started or the console couldn't be reached.
pub const FAILED_NOTE: &str = "Couldn't reach the session's console. Type in the terminal instead.";
