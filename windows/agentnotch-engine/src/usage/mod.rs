//! Usage (AU§8-12): the parser, window ids, the store's merge rules and
//! dated readings, probe scheduling, the probe itself, the binary locator,
//! the environment scrub and Claude Desktop's cache.
//!
//! Owner: WP4. In: the parser, window ids, merge rules, schedule, probe
//! planner, probe, locator, environment scrub, versions and the
//! `.claude.json` reader and Claude Desktop's cache reader (re-exported below
//! under their §3.4 paths). Still a WP0 stub: [`UsageStore`].

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
pub mod versions;

use crate::model::{AccountUsage, IdentityId, StatusLineMessage};
use crate::persist::usage::UsageStateFile;
use crate::runtime_types::{IngestContext, ProbePlan, ProbeResult, RingReading, UsageObservation};
use std::time::SystemTime;

#[derive(Default)]
pub struct UsageStore {
    _readings: Vec<AccountUsage>,
}

impl UsageStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest_status_line(
        &mut self,
        m: &StatusLineMessage,
        ctx: IngestContext,
        now: SystemTime,
    ) -> Option<UsageObservation> {
        let _ = (m, ctx, now);
        None
    }

    pub fn accept_snapshot(
        &mut self,
        u: AccountUsage,
        now: SystemTime,
    ) -> Option<UsageObservation> {
        let _ = (u, now);
        None
    }

    pub fn due_probe(&mut self, now: SystemTime) -> Option<ProbePlan> {
        let _ = now;
        None
    }

    pub fn finish_probe(&mut self, r: ProbeResult, now: SystemTime) {
        let _ = (r, now);
    }

    pub fn is_probing(&self) -> bool {
        false
    }

    pub fn ring_reading(&self, id: &IdentityId, now: SystemTime) -> RingReading {
        let _ = (id, now);
        RingReading::Waiting
    }

    pub fn state_file(&self) -> UsageStateFile {
        UsageStateFile::default()
    }

    pub fn five_hour(&self, id: &IdentityId) -> Option<f64> {
        let _ = id;
        None
    }
}

pub use desktop::{desktop_cache_format, read_desktop_cache};
pub use env::scrubbed_env;
pub use locator::locate_claude;
pub use planner::probe_folder;
pub use probe::run_probe;
