//! Usage (AU§8-12): the parser, window ids, the store's merge rules and
//! dated readings, probe scheduling, the probe itself, the binary locator,
//! the environment scrub and Claude Desktop's cache.
//!
//! Owner: WP4. WP0 stub: the §3.4 signatures; it reads nothing and never
//! runs `claude`.

use crate::model::{
    Account, AccountUsage, DesktopReading, IdentityId, RunFolder, StatusLineMessage,
};
use crate::persist::usage::UsageStateFile;
use crate::platform::{Clock, CommandRunner, Roots};
use crate::runtime_types::{
    ClaudeBinary, IngestContext, ProbeOutcome, ProbePlan, ProbeResult, RingReading,
    UsageObservation,
};
use std::ffi::{OsStr, OsString};
use std::path::Path;
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

/// The run folder a probe (or a summary) for `identity` runs in.
pub fn probe_folder(
    accounts: &[Account],
    folders: &[RunFolder],
    identity: &IdentityId,
) -> Option<RunFolder> {
    let _ = (accounts, folders, identity);
    None
}

pub fn locate_claude(
    roots: &Roots,
    settings_choice: Option<&Path>,
    env_path: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<ClaudeBinary> {
    let _ = (roots, settings_choice, env_path, exists);
    None
}

/// The environment a probe or summary runs with (§4.6). Until WP4 lands it
/// is empty: nothing of the app's environment can leak into a child.
pub fn scrubbed_env(
    base: &[(OsString, OsString)],
    config_dir_env: Option<&str>,
    binary_dir: &Path,
) -> Vec<(OsString, OsString)> {
    let _ = (base, config_dir_env, binary_dir);
    Vec::new()
}

/// A Job on `an-probe`.
pub fn run_probe(plan: &ProbePlan, runner: &dyn CommandRunner, clock: &dyn Clock) -> ProbeResult {
    let _ = runner;
    let now = clock.now();
    ProbeResult {
        plan: plan.clone(),
        outcome: ProbeOutcome::Unavailable("Usage probes aren't in this build yet.".into()),
        started: now,
        finished: now,
        folder_identity_after: None,
    }
}

/// A Job on `an-io`.
pub fn read_desktop_cache(
    roots: &Roots,
    organization_uuid: &str,
    now: SystemTime,
) -> DesktopReading {
    let _ = (roots, organization_uuid, now);
    DesktopReading::Unavailable("Reading Claude Desktop's cache isn't in this build yet.".into())
}
