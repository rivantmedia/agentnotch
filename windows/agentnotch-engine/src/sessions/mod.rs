//! The session pipeline (HS§5): phases, the five states, turn completion,
//! background waits, registry reconciliation, tasks, context, titles,
//! transcripts and chat history. All pure: `SessionStore::apply(input, now)`.
//!
//! Owner: WP5. In so far: the pure modules (`attention`, `background`,
//! `chat`, `completion`, `locator`, `phase`, `summary`, `tasks`, `tool_input`, `tool_results`,
//! `transcript`, `registry`, `desktop`, `session`).
//! The store (`store` and its `store_*` siblings, impl blocks of one
//! struct): hook and status line inputs are real (wp5-7), and so are the
//! registry, Desktop-hosted and periodic-check arms (wp5-8), and the review
//! queue with the attention transitions (wp5-9), and the transcript syncs,
//! the chat calls and the interrupt watcher (wp5-10: `store_transcript`,
//! `interrupt`).

pub mod attention;
pub mod background;
pub mod chat;
pub mod completion;
pub mod desktop;
pub mod interrupt;
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

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The last moment a date read from a file may name: 9999-12-31T23:59:59Z.
/// Windows' clock ends in the year 30828, chrono turns a date into a time by
/// adding without a check, and the store adds its delays (5 s, 90 s, 30
/// min…) to the dates it keeps: a later date from a transcript, a registry
/// entry or the review file would panic there, so it is taken for none.
pub const LATEST_FILE_DATE: Duration = Duration::from_secs(253_402_300_799);

/// `time` when it is no later than [`LATEST_FILE_DATE`].
pub fn file_time(time: SystemTime) -> Option<SystemTime> {
    UNIX_EPOCH
        .checked_add(LATEST_FILE_DATE)
        .filter(|latest| time <= *latest)
        .map(|_| time)
}

/// A year past 9999 (or before year 0) is spelt with a sign: refused before
/// chrono converts it.
pub(crate) fn has_signed_year(text: &str) -> bool {
    text.trim_start().starts_with(['+', '-'])
}
