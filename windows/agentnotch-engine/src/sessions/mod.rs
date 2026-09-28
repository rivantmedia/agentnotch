//! The session pipeline (HS§5): phases, the five states, turn completion,
//! background waits, registry reconciliation, tasks, context, titles,
//! transcripts and chat history. All pure: `SessionStore::apply(input, now)`.
//!
//! Owner: WP5. WP0 stub: the §3.4 signatures; it keeps no sessions.

use crate::model::{ChatHistory, SessionId, SessionView};
use crate::runtime_types::{SessionEffects, SessionInput};
use std::time::SystemTime;

/// The engine's per-session record (SessionState.swift's port). Its fields
/// are WP5's; everything else reads a session through [`SessionView`].
#[derive(Debug, Clone, Default)]
pub struct Session {
    _private: (),
}

#[derive(Default)]
pub struct SessionStore {
    _sessions: Vec<Session>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, input: SessionInput, now: SystemTime) -> SessionEffects {
        let _ = (input, now);
        SessionEffects::default()
    }

    pub fn views(&self) -> Vec<SessionView> {
        Vec::new()
    }

    pub fn view(&self, id: &SessionId) -> Option<SessionView> {
        let _ = id;
        None
    }

    pub fn chat(&self, id: &SessionId) -> Option<ChatHistory> {
        let _ = id;
        None
    }
}
