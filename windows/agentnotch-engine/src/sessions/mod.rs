//! The session pipeline (HS§5): phases, the five states, turn completion,
//! background waits, registry reconciliation, tasks, context, titles,
//! transcripts and chat history. All pure: `SessionStore::apply(input, now)`.
//!
//! Owner: WP5. In so far: the pure modules (`attention`, `background`,
//! `chat`, `completion`, `locator`, `phase`, `summary`, `tasks`, `tool_input`, `tool_results`,
//! `transcript`, `registry`, `desktop`, `session`).
//! The store (`store` and its `store_*` siblings, impl blocks of one
//! struct): hook and status line inputs are real (wp5-7); the registry,
//! review, transcript and periodic-check arms follow in wp5-8..wp5-10.

pub mod attention;
pub mod background;
pub mod chat;
pub mod completion;
pub mod desktop;
pub mod locator;
pub mod phase;
pub mod registry;
pub mod session;
pub mod store;
mod store_review;
mod store_tools;
mod store_transcript;
mod store_turns;
pub mod summary;
pub mod tasks;
pub mod tool_input;
pub mod tool_results;
pub mod transcript;

pub use session::Session;
pub use store::SessionStore;
