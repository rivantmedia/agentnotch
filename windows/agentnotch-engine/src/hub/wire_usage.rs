//! Usage, wired into the hub (design §4.6, AU§9): the store made at load
//! for this run (`UsageStoreConfig::for_run`), told every account of the
//! registry before `usage-state.json` is restored into it, then the Mac's
//! 20-second cycle: forget week-old status lines, read every folder's
//! `.claude.json` (the registry takes the logins, the store Claude Code's
//! cached usage), read Claude Desktop's cache where it is due, then start
//! the probe that is due, if any. One probe runs at a time, on the probe
//! lane; every probe job comes back (a failed or panicking one as a failed
//! probe), and `finish_probe` hears of each exactly once, so the next one
//! can run. `refresh_usage` re-reads the caches first and answers whether a
//! fresher reading is coming.
//!
//! Owner: WP7.

use super::api::{CallError, ComingReply, UsageRefreshTrigger};
use super::core_state::{to_value, Core, Reply};
use super::project;
use crate::core::settings::ControlSettings;
use crate::model::{DesktopReading, IdentityId};
use crate::persist::usage::UsageStateFile;
use crate::runtime_types::{
    ClaudeJsonRead, Job, JobId, PersistFile, ProbeResult, RefreshReason, UsageObservation,
};
use crate::usage::schedule::{CACHE_POLL_INTERVAL, REFRESH_READ_WAIT};
use crate::usage::RefreshRequest;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// The usage store's schedule and the jobs it waits on.
pub(crate) struct UsageWiring {
    /// `usage-state.json` as read at load, until the first accounts are in.
    saved: Option<UsageStateFile>,
    /// The saved state went in (after the first accounts): from then on the
    /// store's state is the one to keep.
    pub(crate) restored: bool,
    /// When the next cycle starts.
    cycle_at: Option<SystemTime>,
    /// The cycle running: its `.claude.json` reads not back yet, and those in.
    cycle: Option<Cycle>,
    /// Claude Desktop reads in flight, by the identity they are for.
    desktop_jobs: BTreeMap<JobId, IdentityId>,
    /// The probe in flight.
    probe_job: Option<JobId>,
    /// When a refresh waiting on Claude Desktop's cache decides anyway.
    probe_check_at: Option<SystemTime>,
    /// `refresh_usage` calls, answered when the running cycle's reads are in.
    refreshes: Vec<(Refresh, Reply)>,
}

#[derive(Debug, Default)]
struct Cycle {
    waiting: BTreeSet<JobId>,
    reads: Vec<ClaudeJsonRead>,
}

/// What a `refresh_usage` asked for.
enum Refresh {
    All,
    One(IdentityId, RefreshReason),
}

impl UsageWiring {
    pub(crate) fn new(saved: Option<UsageStateFile>) -> UsageWiring {
        UsageWiring {
            saved,
            restored: false,
            cycle_at: None,
            cycle: None,
            desktop_jobs: BTreeMap::new(),
            probe_job: None,
            probe_check_at: None,
            refreshes: Vec::new(),
        }
    }

    /// Calls still waiting are answered (a stop).
    pub(crate) fn answer_waiting(&mut self, why: &str) {
        for (_, reply) in self.refreshes.drain(..) {
            let _ = reply.send(Err(CallError::failed(why)));
        }
    }
}

impl Core {
    /// The registry's accounts changed: the store is told all of them
    /// (tracked or not); the first time, the saved state goes in after them.
    pub(crate) fn usage_accounts_changed(&mut self, now: SystemTime) {
        if !self.registry.has_classified() {
            // Restoring before the accounts are known would prune it all.
            return;
        }
        let accounts = self.registry.accounts();
        self.usage
            .set_accounts(&accounts, &self.registry.folders(), now);
        if !self.usage_w.restored {
            if let Some(file) = self.usage_w.saved.take() {
                self.usage.restore(&file, now);
            }
            self.usage_w.restored = true;
        }
        // An account whose ring is off (or untracked) is neither probed nor
        // read from Claude Desktop until it is back.
        self.usage
            .set_paused(project::paused_account_ids(&accounts));
    }

    pub(crate) fn usage_on_start(&mut self, now: SystemTime) {
        self.usage_accounts_changed(now);
        self.usage.start();
        self.usage_w.cycle_at = Some(now);
    }

    /// A setting the store reads changed (a page's or the cloud thread's).
    pub(crate) fn usage_settings_changed(&mut self, old: &ControlSettings) {
        let new = &self.settings;
        if old.usage_probe_interval_minutes != new.usage_probe_interval_minutes {
            self.usage
                .set_probe_interval_minutes(new.usage_probe_interval_minutes);
        }
        if old.reads_desktop_usage_cache != new.reads_desktop_usage_cache {
            self.usage
                .set_reads_desktop(new.reads_desktop_usage_cache && !self.cfg.flags.sealed);
        }
        if old.claude_binary_path != new.claude_binary_path {
            self.usage
                .set_claude_binary_path(new.claude_binary_path.as_ref().map(Into::into));
        }
    }

    /// The usage work that became due.
    pub(crate) fn drive_usage(&mut self, now: SystemTime) {
        if self.usage_w.cycle.is_none() && self.usage_w.cycle_at.is_some_and(|at| at <= now) {
            self.start_cycle(now);
        }
        if self.usage_w.probe_check_at.is_some_and(|at| at <= now) {
            self.usage_w.probe_check_at = None;
            self.start_due_probe(now);
        }
        if self.usage_w.restored {
            if let Some(file) = self.usage.take_state_to_save(now) {
                self.persist(PersistFile::Usage, file.encode());
            }
        }
    }

    pub(crate) fn usage_deadline(&self) -> Option<SystemTime> {
        let cycle = self
            .usage_w
            .cycle
            .is_none()
            .then_some(self.usage_w.cycle_at)
            .flatten();
        [cycle, self.usage_w.probe_check_at, self.usage.save_due_at()]
            .into_iter()
            .flatten()
            .min()
    }

    /// One cycle: status lines pruned, then every folder's `.claude.json`
    /// read (the rest follows when the reads are in).
    fn start_cycle(&mut self, now: SystemTime) {
        self.usage_w.cycle_at = Some(now + CACHE_POLL_INTERVAL);
        self.usage.prune_status_lines(now);
        let mut cycle = Cycle::default();
        for (folder, path) in self.registry.identity_files() {
            let id = self.schedule(Job::ReadClaudeJson { folder, path }, None);
            cycle.waiting.insert(id);
        }
        if cycle.waiting.is_empty() {
            self.finish_cycle(Vec::new(), now);
        } else {
            self.usage_w.cycle = Some(cycle);
        }
    }

    /// A `.claude.json` read came back.
    pub(crate) fn claude_json_read(&mut self, id: JobId, read: ClaudeJsonRead, now: SystemTime) {
        let Some(cycle) = self.usage_w.cycle.as_mut() else {
            return;
        };
        if !cycle.waiting.remove(&id) {
            return;
        }
        cycle.reads.push(read);
        if cycle.waiting.is_empty() {
            let reads = self
                .usage_w
                .cycle
                .take()
                .map(|cycle| cycle.reads)
                .unwrap_or_default();
            self.finish_cycle(reads, now);
        }
    }

    /// The reads are in: the registry takes the logins, the store the
    /// cached usage; Claude Desktop is read where due; waiting refreshes
    /// decide; the probe that is due starts.
    fn finish_cycle(&mut self, reads: Vec<ClaudeJsonRead>, now: SystemTime) {
        let mut changed = crate::runtime_types::AccountsChanged::default();
        for read in &reads {
            let one = self.registry.apply_claude_json(read, now);
            changed.rings |= one.rings;
            changed.folders |= one.folders;
            changed.identities |= one.identities;
            changed.new_run_folders.extend(one.new_run_folders);
            changed.removed_folders.extend(one.removed_folders);
        }
        // The store reads who is signed in from the registry's accounts.
        self.accounts_changed(&changed, now);
        let observations = self.usage.apply_cache_reads(&reads, now);
        self.usage_observed(observations);
        let due = self.usage.desktop_due(now, None, false);
        self.read_desktop(due);
        for (refresh, reply) in std::mem::take(&mut self.usage_w.refreshes) {
            let request = match refresh {
                Refresh::All => self.usage.refresh_all(now),
                Refresh::One(id, reason) => self.usage.refresh(&id, reason, now),
            };
            let coming = self.requested(request, now);
            let _ = reply.send(to_value(&ComingReply { coming }));
        }
        self.start_due_probe(now);
    }

    /// Carries out the part of a refresh that is the hub's: Claude
    /// Desktop's reads now, and a decision once they are in (or their wait
    /// is over). Says whether a fresher reading is coming.
    fn requested(&mut self, request: RefreshRequest, now: SystemTime) -> bool {
        if !request.desktop_reads.is_empty() {
            let decide_at = now + REFRESH_READ_WAIT;
            self.usage_w.probe_check_at = Some(
                self.usage_w
                    .probe_check_at
                    .map_or(decide_at, |at| at.min(decide_at)),
            );
        }
        self.read_desktop(request.desktop_reads);
        request.wait_until.is_some()
    }

    fn read_desktop(&mut self, due: Vec<(IdentityId, String)>) {
        for (identity, organization_uuid) in due {
            let id = self.schedule(Job::ReadDesktopCache { organization_uuid }, None);
            self.usage_w.desktop_jobs.insert(id, identity);
        }
    }

    /// Claude Desktop's cache was read for an identity.
    pub(crate) fn desktop_read(&mut self, id: JobId, reading: DesktopReading, now: SystemTime) {
        let Some(identity) = self.usage_w.desktop_jobs.remove(&id) else {
            return;
        };
        let observation = self.usage.accept_desktop(&identity, &reading, now);
        self.usage_observed(observation);
        // A refresh that waited for this read decides now.
        self.start_due_probe(now);
    }

    /// Starts the probe that is due, unless one runs.
    fn start_due_probe(&mut self, now: SystemTime) {
        if self.usage_w.probe_job.is_some() {
            return;
        }
        if let Some(plan) = self.usage.due_probe(now) {
            let id = self.schedule(Job::Probe(plan), None);
            self.usage_w.probe_job = Some(id);
        }
    }

    /// A probe came back (also a failed or panicking one): the store hears
    /// of it once, and the next one may start.
    pub(crate) fn probe_done(&mut self, id: JobId, result: ProbeResult, now: SystemTime) {
        if self.usage_w.probe_job == Some(id) {
            self.usage_w.probe_job = None;
        }
        let identity = result.plan.identity.clone();
        let folder = result.plan.folder.clone();
        let observation = self.usage.finish_probe(result, now);
        self.usage_observed(observation);
        // The plan `get_usage` named, when `.claude.json` names none.
        if let Some(plan) = self
            .usage
            .subscription_type_of(&identity)
            .map(str::to_owned)
        {
            let changed = self.registry.note_subscription_type(&folder, &plan);
            self.accounts_changed(&changed, now);
        }
        if self.live {
            self.start_due_probe(now);
        }
    }

    /// Readings for the usage history. The cloud records them (wp7-10);
    /// until it runs, nothing keeps them.
    fn usage_observed(&mut self, _observations: impl IntoIterator<Item = UsageObservation>) {}

    // ---- calls ----

    /// `refresh_usage {ring_id?, reason}` → `{coming}`, once the caches were
    /// read again. A hub that isn't running checks nothing.
    pub(crate) fn refresh_usage_call(
        &mut self,
        ring_id: Option<String>,
        reason: UsageRefreshTrigger,
        reply: Reply,
    ) {
        let not_coming = || to_value(&ComingReply { coming: false });
        if !self.live {
            let _ = reply.send(not_coming());
            return;
        }
        let reason = match reason {
            UsageRefreshTrigger::RingClick => RefreshReason::RingClick,
            UsageRefreshTrigger::Manual => RefreshReason::Manual,
        };
        let refresh = match ring_id {
            None => Refresh::All,
            Some(ring) => {
                let identity = self
                    .registry
                    .accounts()
                    .into_iter()
                    .find(|account| account.ring_id.as_str() == ring)
                    .map(|account| account.identity_id);
                match identity {
                    Some(identity) => Refresh::One(identity, reason),
                    None => {
                        let _ = reply.send(not_coming());
                        return;
                    }
                }
            }
        };
        self.usage_w.refreshes.push((refresh, reply));
        // The caches first: the newest reading of any source decides.
        if self.usage_w.cycle.is_none() {
            let now = self.platform.clock.now();
            self.start_cycle(now);
        }
    }
}
