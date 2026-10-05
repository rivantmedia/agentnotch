//! The wire protocol between `agentnotch-hook.exe` and Agent Notch for Windows.
//!
//! Claude Code runs the hook exe for every registered hook event (and as the
//! account's status line); the exe turns what Claude Code wrote on stdin into
//! one message and sends it to the app over a per-user named pipe
//! (`\\.\pipe\agentnotch-hook-<SID>`, DESIGN-WIN §1.4). A PermissionRequest
//! then waits for the app's decision and prints Claude Code's hook output.
//!
//! Everything here is pure so it can be tested on every OS: framing, the
//! messages (the Mac hook script's `build_message`, field for field, plus
//! three Windows fields), the permission output, the hand-parsed argv of the
//! exe, the parent-pid walk over a process list, and the control and typing
//! messages. Nothing here may panic on hostile input: the hook exe must fail
//! open (exit 0, no output) whatever Claude Code or the pipe hands it.
//!
//! Compatibility: hook copies in run folders can be older or newer than the
//! app. Every field added later is optional, every decoder ignores unknown
//! fields, and the committed `tests/fixtures/v1/*.json` frames stay accepted
//! forever.

pub mod console;
pub mod control;
pub mod frame;
pub mod invocation;
pub mod limits;
pub mod message;
pub mod permission;
pub mod pid;
pub mod pipe;
mod pyjson;
pub mod statusline;
pub mod typing;

pub use console::ConsoleInfo;
pub use control::{ControlOp, ControlRequest, ControlResponse, ControlStatus, CONTROL_EVENT};
pub use frame::{encode_frame, read_frame, write_frame, FrameError};
pub use invocation::{parse_invocation, Invocation, TypeArgs};
pub use message::{
    build_hook_message, build_statusline_message, encode_hook_message, status_for, HookEnv,
    STATUS_LINE_EVENT,
};
pub use permission::{
    permission_output, permission_output_for_frames, permission_output_for_stdin, Decision,
    PermissionResponse, DEFAULT_DENY_MESSAGE, KEEP_PLANNING_REASON,
};
pub use pid::{pid_guess, ProcLink};
pub use pipe::{
    dev_pipe_override, pipe_name, pipe_sddl, PipeAce, PipeSecurity, MEDIUM_INTEGRITY_RID,
    PIPE_NAME_PREFIX, SYSTEM_SID,
};
pub use typing::{TypePhase, TypeRequest, TYPE_ABORT, TYPE_SUBMIT};

/// The protocol version every message carries (`"protocol": 1`). A server
/// decodes any version ≥ 1 leniently and never refuses a frame for its
/// version; a change that cannot be an optional field gets a new `event`
/// name instead of a new version.
pub const PROTOCOL: u32 = 1;
