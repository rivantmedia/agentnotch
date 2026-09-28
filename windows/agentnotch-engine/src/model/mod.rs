//! The engine's data types (DESIGN-WIN §3.3 and §3.6), one file per owning
//! package: `hook` (WP1), `accounts` (WP3), `usage` (WP4), `sessions` and
//! `requests` (WP5), `cloud` (WP8), `ui` (WP7). A package may add fields to
//! its own file, never rename or remove one without the lead.
//!
//! JSON is snake_case (serde's default) for everything the fork's pages see;
//! every enum is externally tagged with snake_case variant names unless it
//! names a `tag`. Times are `SystemTime` inside Rust and `*_ms: u64` (epoch
//! milliseconds) in UI JSON. The files the Mac also writes are never
//! (de)serialised from these types: `persist::*` has a data-transfer type per
//! file with the Mac's own names and date encodings.

pub mod accounts;
pub mod cloud;
pub mod hook;
pub mod ids;
pub mod requests;
pub mod sessions;
pub mod ui;
pub mod usage;

pub use accounts::*;
pub use cloud::*;
pub use hook::*;
pub use ids::*;
pub use requests::*;
pub use sessions::*;
pub use ui::*;
pub use usage::*;
