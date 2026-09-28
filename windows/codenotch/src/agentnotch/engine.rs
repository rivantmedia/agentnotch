//! The engine names the glue uses beyond DESIGN-WIN §3's written signatures, in one place, so
//! that a name the engine settles differently is a one-file change here.
//!
//! - `DevFlags` (`core::flags`, §4.13) is built from this process's argv and environment by
//!   `DevFlags::from_env`, and says whether the run is sealed in its `sealed` field;
//! - the rebrand of upstream copy is `core::rebrand` (WP7); until it lands, the name swap below
//!   stands in for it, which is exact for the strings the glue rebrands (the "Quit …" items in
//!   every language, which carry no article or compound);
//! - the website comes from `app-config.json` through the engine's validation
//!   (`cloud::website::from_app_config`, WP8); until it lands the app has no sync website, which
//!   Settings shows as such.

pub use agentnotch_engine::core::flags::DevFlags;

/// The switches of this process (argv and environment), read once by `super::flags`.
pub fn dev_flags() -> DevFlags {
    let args: Vec<String> = std::env::args().collect();
    DevFlags::from_env(&args, &|name: &str| std::env::var(name).ok())
}

pub fn is_sealed(flags: &DevFlags) -> bool {
    flags.sealed
}

/// Upstream's name for the app, as its copy spells it.
const UPSTREAM_NAME: &str = "Codenotch";

pub fn rebrand(text: &str) -> String {
    text.replace(UPSTREAM_NAME, super::DISPLAY_NAME)
}

/// The sync website named by `app-config.json`, once the engine has checked it.
pub fn website(_app_config: &str) -> Option<String> {
    None
}
