//! The engine names the glue uses beyond DESIGN-WIN §3's written signatures, in one place, so
//! that a name the engine settles differently is a one-file change here.
//!
//! - `DevFlags` (`core::flags`, §4.13) is built from this process's argv and environment by
//!   `DevFlags::from_env`, once the home folder is known (it expands `~` in
//!   `AGENTNOTCH_EXTRA_CONFIG_DIRS`);
//! - whether the run is sealed is `core::sealed::is_sealed`, the rule `DevFlags::sealed` is set
//!   from too: upstream's config folder (WR-DIR) depends on it, and that folder is needed before
//!   the hub's roots exist;
//! - the rebrand of upstream copy is `core::rebrand` (WP7); until it lands, the name swap below
//!   stands in for it, which is exact for the strings the glue rebrands (the "Quit …" items in
//!   every language, which carry no article or compound);
//! - the website comes from `app-config.json` through the engine's validation
//!   (`cloud::website::from_app_config`, WP8); until it lands the app has no sync website, which
//!   Settings shows as such.

use std::path::Path;

pub use agentnotch_engine::core::flags::DevFlags;
use agentnotch_engine::core::paths::Paths;
pub use agentnotch_engine::core::roots::data_folder_name;

/// The switches of this process (argv and environment); `home` is the hub's home folder.
pub fn dev_flags(home: &Path) -> DevFlags {
    let args: Vec<String> = std::env::args().collect();
    DevFlags::from_env(
        |name: &str| std::env::var(name).ok(),
        &args,
        &Paths::native(home),
    )
}

/// Whether this process's environment seals the run (fails closed).
pub fn is_sealed() -> bool {
    agentnotch_engine::core::sealed::is_sealed(|name: &str| std::env::var(name).ok())
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
