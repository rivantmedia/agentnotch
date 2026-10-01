//! When a sessions panel that opened by itself goes away again, and what a
//! banner click or an auto-open shows (the Mac's `ClaudePanelPolicy`, the
//! part the engine decides). The glue reports the panel as a [`PanelState`];
//! the hub keeps an [`AutoOpenWatch`] for a panel it opened itself and emits
//! `HubEvent::PanelClose` when its deadline passes.

use crate::model::{PanelRequest, SessionId, SessionState};
use crate::runtime_types::PanelState;
use std::time::{Duration, SystemTime};

/// How long after its session is resolved an auto-opened panel lingers.
pub const AUTO_CLOSE_AFTER_RESOLVED: Duration = Duration::from_secs(1);
/// The least an untouched auto-opened panel stays up.
pub const AUTO_CLOSE_MINIMUM_TIMEOUT: Duration = Duration::from_secs(8);

/// `PanelRequest::reason` / `PanelState::reason` of a panel the engine
/// opened by itself.
pub const REASON_AUTO: &str = "auto";
/// …of one opened by a click on a banner.
pub const REASON_NOTIFICATION: &str = "notification";
/// The list of every account's sessions.
pub const ROUTE_SESSIONS: &str = "sessions";

/// A panel that opened by itself, until the user has touched it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoOpen {
    pub opened_at: SystemTime,
    /// When the attention that opened it (needs you, or ready for review)
    /// was resolved, or its session went away.
    pub resolved_at: Option<SystemTime>,
    /// The pointer has entered the panel, or it took the keyboard. From then
    /// on it is the user's panel and closes only the ordinary ways.
    pub engaged: bool,
}

impl AutoOpen {
    pub fn new(opened_at: SystemTime) -> AutoOpen {
        AutoOpen {
            opened_at,
            resolved_at: None,
            engaged: false,
        }
    }

    /// When it closes by itself: [`AUTO_CLOSE_AFTER_RESOLVED`] after its
    /// session is resolved, or max(peek, [`AUTO_CLOSE_MINIMUM_TIMEOUT`])
    /// after it opened if nobody touched it, whichever comes first. `None`
    /// once the user has engaged.
    pub fn deadline(&self, peek: Duration) -> Option<SystemTime> {
        if self.engaged {
            return None;
        }
        let timeout = self.opened_at + peek.max(AUTO_CLOSE_MINIMUM_TIMEOUT);
        Some(match self.resolved_at {
            Some(resolved_at) => timeout.min(resolved_at + AUTO_CLOSE_AFTER_RESOLVED),
            None => timeout,
        })
    }
}

/// The user is in the panel: the pointer is inside or a field has focus, or
/// the glue confirmed it holds the keyboard.
fn is_engaged(panel: &PanelState) -> bool {
    panel.engaged || panel.focused
}

fn is_auto_opened(panel: &PanelState) -> bool {
    panel.open && panel.reason.as_deref() == Some(REASON_AUTO)
}

/// `ClaudePanelPolicy.autoCloseDeadline` on the panel the glue reports:
/// `None` unless it is open, opened by itself and untouched right now.
///
/// This sees only the present report, so a pointer that went in and out
/// again no longer counts; [`AutoOpenWatch`] remembers it, as the Mac does.
pub fn auto_close_deadline(
    panel: &PanelState,
    cause_resolved_at: Option<SystemTime>,
    opened_at: SystemTime,
    peek_seconds: u32,
) -> Option<SystemTime> {
    if !is_auto_opened(panel) || is_engaged(panel) {
        return None;
    }
    AutoOpen {
        opened_at,
        resolved_at: cause_resolved_at,
        engaged: false,
    }
    .deadline(Duration::from_secs(u64::from(peek_seconds)))
}

/// What an auto-opened panel was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoOpenCause {
    NeedsInput,
    ReadyForReview,
}

/// The cause a session's state can auto-open the panel for; `None` for
/// working and idle, which never do.
pub fn auto_open_cause(state: &SessionState) -> Option<AutoOpenCause> {
    match state {
        SessionState::NeedsYou(_) | SessionState::Failed(_) => Some(AutoOpenCause::NeedsInput),
        SessionState::ReadyForReview => Some(AutoOpenCause::ReadyForReview),
        SessionState::Working | SessionState::Idle => None,
    }
}

/// Whether what auto-opened the panel is over: the session has left that
/// attention (answered, reviewed, went back to work), or it is gone (`None`).
/// A different question on the same session still needs the user.
pub fn is_resolved(cause: AutoOpenCause, state: Option<&SessionState>) -> bool {
    match state {
        None => true,
        Some(state) => auto_open_cause(state) != Some(cause),
    }
}

/// An auto-opened panel as the hub follows it: the session it opened for,
/// when that was resolved, and whether the user has touched it since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoOpenWatch {
    pub session: SessionId,
    /// `None` when the session was already past the state that opened the
    /// panel: only the timeout closes it then.
    pub cause: Option<AutoOpenCause>,
    pub state: AutoOpen,
}

impl AutoOpenWatch {
    /// The panel was opened by itself for `session`, whose state is `state`.
    pub fn opened(session: SessionId, state: Option<&SessionState>, now: SystemTime) -> Self {
        AutoOpenWatch {
            session,
            cause: state.and_then(auto_open_cause),
            state: AutoOpen::new(now),
        }
    }

    /// The glue reported the panel. False when this watch is over: the panel
    /// closed, or was opened again another way (a click, the shortcut).
    pub fn panel_reported(&mut self, panel: &PanelState) -> bool {
        if !is_auto_opened(panel) {
            return false;
        }
        if is_engaged(panel) {
            self.state.engaged = true;
        }
        true
    }

    /// The session's state now (`None`: it is gone). Notes the first moment
    /// its cause was resolved.
    pub fn session_is(&mut self, state: Option<&SessionState>, now: SystemTime) {
        let Some(cause) = self.cause else { return };
        if self.state.resolved_at.is_none() && is_resolved(cause, state) {
            self.state.resolved_at = Some(now);
        }
    }

    pub fn deadline(&self, peek_seconds: u32) -> Option<SystemTime> {
        self.state
            .deadline(Duration::from_secs(u64::from(peek_seconds)))
    }
}

/// What the panel shows for a banner click or an auto-open that names a
/// session: the list of every account with that session's row highlighted,
/// not the session's chat. Its answer is one keystroke away there, and
/// opening a chat marks the session reviewed, which would resolve (and so
/// close) a panel auto-opened for a finished session the moment it appeared.
pub fn lands_on_list(session: &SessionId, reason: &str) -> PanelRequest {
    PanelRequest {
        route: ROUTE_SESSIONS.to_owned(),
        ring_id: None,
        highlight: Some(session.as_str().to_owned()),
        reason: reason.to_owned(),
    }
}
