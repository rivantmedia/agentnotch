//! The hub: the one facade the Tauri glue talks to (DESIGN-WIN §3.5).
//! `api` fixes its surface; `project` and `project_settings` make what the
//! pages draw; `sealed_fixture` is the sealed hub, serving `sealed_demo` (the
//! fixture stores through those projections). `runtime` is the live hub:
//! `an-core` over `core_state::Core`, the worker lanes of `jobs`.

pub mod api;
mod core_state;
pub mod jobs;
pub mod project;
pub mod project_settings;
pub mod runtime;
pub mod sealed_demo;
pub mod sealed_fixture;

pub use api::*;
