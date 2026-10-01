//! Usage (AU§8-12): the parser, window ids, the store's merge rules and
//! dated readings, probe scheduling, the probe itself, the binary locator,
//! the environment scrub and Claude Desktop's cache.
//!
//! Owner: WP4. In: the parser, window ids, merge rules, schedule, probe
//! planner, probe, locator, environment scrub, versions and the
//! `.claude.json` reader and Claude Desktop's cache reader (re-exported below
//! under their §3.4 paths) and the [`UsageStore`]: status lines, caches, Claude
//! Desktop, persistence and the ring reading; its probe half (what is probed
//! when, requested refreshes, finishing a probe) is `store_probes`.

pub mod claude_json;
pub mod desktop;
pub mod env;
pub mod locator;
pub mod merge;
pub mod parser;
pub mod planner;
pub mod probe;
pub mod ring_windows;
pub mod schedule;
pub mod store;
pub mod store_probes;
pub mod versions;

pub use desktop::{desktop_cache_format, read_desktop_cache};
pub use env::scrubbed_env;
pub use locator::locate_claude;
pub use planner::probe_folder;
pub use probe::run_probe;
pub use store::{UsageStore, UsageStoreConfig};
pub use store_probes::{ProbeEnvironment, RefreshRequest};
