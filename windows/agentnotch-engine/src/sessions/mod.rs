//! The session pipeline (HS§5): phases, the five states, turn completion,
//! background waits, registry reconciliation, tasks, context, titles,
//! transcripts and chat history. All pure: `SessionStore::apply(input, now)`.
//!
//! Owner: WP5. In so far: the pure modules (`attention`, `background`,
//! `chat`, `completion`, `locator`, `phase`, `summary`, `tasks`, `tool_input`, `tool_results`,
//! `transcript`, `registry`, `desktop`, `session`).
//! Still WP0's stub: [`SessionStore`] (the §3.4 signatures, keeps no
//! sessions); the real store comes in wp5-7..wp5-10.

pub mod attention;
pub mod background;
pub mod chat;
pub mod completion;
pub mod desktop;
pub mod locator;
pub mod phase;
pub mod registry;
pub mod session;
pub mod summary;
pub mod tasks;
pub mod tool_input;
pub mod tool_results;
pub mod transcript;

use crate::model::{ChatHistory, SessionId, SessionView};
use crate::runtime_types::{SessionEffects, SessionInput};
use std::time::SystemTime;

pub use session::Session;

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
