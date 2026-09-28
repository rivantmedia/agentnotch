//! Every size and time limit of the protocol, in one place so the hook exe,
//! the pipe server and the engine cannot drift apart. The string limits are
//! the Mac hook script's (HS§1.2), counted in Unicode scalar values as Python
//! counts `len(str)`.

/// Largest message a hook sends after its own truncation (HS§1.5).
pub const MAX_CLIENT_MESSAGE: usize = 1 << 20;
/// Largest frame the server reads; a bigger one closes the connection.
pub const MAX_SERVER_MESSAGE: usize = 8 << 20;
/// Largest response frame the hook reads (the permission decision).
pub const MAX_RESPONSE: usize = 4 << 20;

/// Every string nested in `tool_input` is clamped to this.
pub const MAX_TOOL_INPUT_STRING: usize = 20_000;
/// When a message is still above [`MAX_CLIENT_MESSAGE`], `tool_input`'s
/// strings are clamped again to this…
pub const MAX_TOOL_INPUT_STRING_REDUCED: usize = 500;
/// …and its lists to this many items.
pub const MAX_LIST_ITEMS: usize = 50;
/// `tool_error` and `stop_error_details`.
pub const MAX_TOOL_ERROR: usize = 500;
/// `last_assistant_message` (Stop, StopFailure, SubagentStop).
pub const MAX_LAST_ASSISTANT_MESSAGE: usize = 1500;
/// `prompt` (UserPromptSubmit).
pub const MAX_PROMPT: usize = 300;
/// `session_title` (SessionStart, UserPromptSubmit).
pub const MAX_SESSION_TITLE: usize = 200;
/// Every other free text: messages, titles, task subjects, denial reasons.
pub const MAX_TEXT: usize = 2000;
/// Background task types forwarded at Stop.
pub const MAX_BACKGROUND_TYPES: usize = 64;

/// The watchdog ends a hook with exit 0 this long after stdin was read when
/// connecting and writing have not finished (every hook event; a
/// PermissionRequest disarms it once its frame is written).
pub const HOOK_WATCHDOG_MS: u64 = 1200;
/// The status line's send, connect included, runs on its own thread for at
/// most this long.
pub const STATUS_LINE_SEND_BUDGET_MS: u64 = 300;
/// Matches the installer's `"timeout": 86400` for PermissionRequest.
pub const DECISION_TIMEOUT_S: u64 = 86_400;
/// The server closes a connection that has not sent a whole frame by then.
pub const SERVER_READ_DEADLINE_MS: u64 = 5000;
/// The server writes a response within this, then closes.
pub const RESPONSE_WRITE_TIMEOUT_MS: u64 = 2000;
/// Above this many open connections a new one is closed at once (the hook
/// fails open).
pub const MAX_CONNECTIONS: usize = 512;
/// In and out buffer of each pipe instance.
pub const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
