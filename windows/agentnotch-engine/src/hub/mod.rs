//! The hub: the one facade the Tauri glue talks to (DESIGN-WIN §3.5).
//! `api` fixes its surface; `sealed_fixture` serves the ui-contract fixtures
//! so the glue and the pages can be built and self-tested before the engine
//! exists. WP7 adds the runtime behind the same `Hub` and its sealed demo.

pub mod api;
pub mod project;
pub mod project_settings;
pub mod sealed_fixture;

pub use api::*;
