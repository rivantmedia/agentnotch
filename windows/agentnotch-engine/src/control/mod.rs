//! Control and OS integration (HS§6-9): answers to requests, typing replies
//! (message safety), jumping to terminals, "is the user looking at it",
//! toasts, chimes, peek, auto-open and the panel's auto-close.
//!
//! Owner: WP6. WP0 stub: the §3.4 signatures; it refuses to type, plans no
//! jump and posts nothing.

use crate::model::AttentionTransition;
use crate::model::{Answer, PendingRequest, SessionView};
use crate::platform::{ConsoleInfo, FocusStep, Foreground, HostApp, Toast};
use crate::runtime_types::{PanelState, ReactionContext, Reactions, ToastContext};
use agentnotch_proto::PermissionResponse;
use std::time::SystemTime;

pub fn permission_response(req: &PendingRequest, a: &Answer) -> Result<PermissionResponse, String> {
    let _ = (req, a);
    Err("Answering isn't in this build yet.".into())
}

/// HS§8.1's rules with the Windows mapping (HS§8.2, §4.8).
pub fn message_safety(
    view: &SessionView,
    info: &ConsoleInfo,
    host: &HostApp,
) -> Result<(), String> {
    let _ = (view, info, host);
    Err("Typing replies isn't in this build yet.".into())
}

pub fn focus_plan(view: &SessionView, host: &HostApp, info: &ConsoleInfo) -> Vec<FocusStep> {
    let _ = (view, host, info);
    Vec::new()
}

pub fn looking_at(
    view: &SessionView,
    host: &HostApp,
    fg: &Foreground,
    sessions_in_window: usize,
) -> Option<bool> {
    let _ = (view, host, fg, sessions_in_window);
    None
}

pub fn toast_for(tr: &AttentionTransition, ctx: &ToastContext) -> Option<Toast> {
    let _ = (tr, ctx);
    None
}

/// Chime, peek and auto-open for a burst of transitions.
pub fn reactions(burst: &[AttentionTransition], ctx: &ReactionContext) -> Reactions {
    let _ = (burst, ctx);
    Reactions::default()
}

/// ClaudePanelPolicy.autoCloseDeadline.
pub fn auto_close_deadline(
    panel: &PanelState,
    cause_resolved_at: Option<SystemTime>,
    opened_at: SystemTime,
    peek_seconds: u32,
) -> Option<SystemTime> {
    let _ = (panel, cause_resolved_at, opened_at, peek_seconds);
    None
}
