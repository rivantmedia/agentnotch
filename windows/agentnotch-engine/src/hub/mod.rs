//! The hub: the one facade the Tauri glue talks to (DESIGN-WIN §3.5).
//! `api` fixes its surface; `project` and `project_settings` make what the
//! pages draw; `sealed_fixture` is the sealed hub, serving `sealed_demo` (the
//! fixture stores through those projections). WP7 adds the runtime behind
//! the same `Hub`.

pub mod api;
pub mod project;
pub mod project_settings;
pub mod sealed_demo;
pub mod sealed_fixture;

pub use api::*;
