//! The review queue (HS§5.13): marks, the heartbeat, pruning, and
//! `review-state.json` (through `persist::review`).
//!
//! Owner: WP5.

pub mod store;

pub use store::{ReviewSnapshot, ReviewStore, StopFailure};
