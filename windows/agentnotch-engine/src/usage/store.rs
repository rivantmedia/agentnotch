//! Latest plan usage per account (UsageStore.swift, AU§9): per signed-in
//! identity, whatever folders it lives in, merged from read-only sources,
//! none of which involves a token:
//!
//! 1. `.claude.json` -> `cachedUsageUtilization`, Claude Code's own snapshot,
//!    from every folder of the identity, stores included (the freshest one
//!    whose account UUID is the identity's wins);
//! 2. status line `rate_limits` of the account's live sessions (5-hour and
//!    7-day windows only), dated per Claude Code process (`merge::advance`);
//! 3. Claude Desktop's HTTP cache, with the "Also read Claude Desktop's
//!    cached usage" setting on, for accounts whose claude.ai organization is
//!    known: at most once a minute, every 5 minutes after a miss;
//! 4. the usage probe (`store_probes.rs`).
//!
//! Per window the most current reading wins (`merge`). The Swift store is a
//! main-actor object with timers; this one is plain data driven with an
//! explicit `now`. Its owner (the hub, WP7) runs the loops and does the I/O:
//! it feeds `set_accounts` when the registry changes, `apply_cache_reads`
//! every 20 s with the folders' `.claude.json` reads, `desktop_due` /
//! `accept_desktop` for Claude Desktop, `prune_status_lines` with each cycle,
//! and writes `usage-state.json` whenever `save_due_at` has passed
//! (`take_state_to_save`). Nothing here reads a file, a clock or a process.
//!
//! The probe half (what to probe and when, requested refreshes, finishing a
//! probe) is a second `impl UsageStore` in `store_probes.rs`: the fields it
//! needs are `pub(super)`.

use crate::core::paths::Paths;
use crate::core::time::IsoSeconds;
use crate::model::RingStatus;
use crate::model::{
    distant_past, Account, AccountId, AccountUsage, Attribution, DesktopReading, IdentityId,
    RunFolder, StatusLineMessage, StatusLineRecord, UsageFetchState, UsageReading, UsageSource,
};
use crate::persist::usage::{
    PersistedAccountUsage, PersistedReading, PersistedStatusLine, PersistedStatusLineReadings,
    PersistedUsageAccount, UsageStateFile,
};
use crate::runtime_types::{
    ClaudeJsonRead, IngestContext, RefreshReason, RingReading, UsageObservation,
};
use crate::usage::merge::{self, StatusWindow};
use crate::usage::ring_windows::{self, SESSION_ID, WEEKLY_ID};
use crate::usage::schedule;
use crate::usage::store_probes::{PendingRefresh, ProbeEnvironment};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What the store is told once, and what the settings change.
#[derive(Debug, Clone)]
pub struct UsageStoreConfig {
    /// `%USERPROFILE%`: the default config folder `~\.claude` is under it.
    pub home: PathBuf,
    /// Probes run on their own schedule (off when sealed, and in dev runs
    /// unless `AGENTNOTCH_USAGE_PROBE=1`; `DevFlags::probes_allowed`).
    pub probes_allowed: bool,
    /// Claude Code is never launched, not even for a requested refresh
    /// (a sealed run); the account then reads "Usage probes are off".
    pub probes_disabled: bool,
    /// `usageProbeIntervalMinutes`: 0 turns the probe off.
    pub probe_interval_minutes: u32,
    /// "Also read Claude Desktop's cached usage".
    pub reads_desktop: bool,
    /// Claude Parallel Profiles mirrors accounts into `~\.claude`
    /// (AU§0.2: never on native Windows, kept so the Mac's rules stay tested).
    pub mirrors_default: bool,
}

impl Default for UsageStoreConfig {
    fn default() -> Self {
        UsageStoreConfig {
            home: PathBuf::new(),
            probes_allowed: true,
            probes_disabled: false,
            probe_interval_minutes: 5,
            reads_desktop: true,
            mirrors_default: false,
        }
    }
}

/// Per Claude Code process (`merge::status_line_key`).
pub(super) type Processes = BTreeMap<String, StatusLineRecord>;
/// Per folder the sessions ran in (a `Paths::key`).
pub(super) type Folders = BTreeMap<String, Processes>;

/// The usage store's state. Every key is an identity id, except for status
/// lines and snapshots that came in under a folder id (a status line from a
/// folder the registry had not grouped yet), which `set_accounts` moves to
/// the folder's identity once it has one.
pub struct UsageStore {
    pub(super) config: UsageStoreConfig,
    pub(super) paths: Paths,
    /// The registry as last told (`set_accounts`).
    pub(super) accounts: Vec<Account>,
    /// Merged usage per identity.
    pub(super) usage: BTreeMap<IdentityId, AccountUsage>,
    /// Probe status per identity (absent = never probed).
    pub(super) fetch_state: BTreeMap<IdentityId, UsageFetchState>,
    /// When each identity was last probed (success or not).
    pub(super) last_probe_at: BTreeMap<IdentityId, SystemTime>,
    /// The claude.ai organization of each signed-in identity, kept while
    /// Claude Desktop readings are on (its cache is keyed by it).
    pub(super) organizations: BTreeMap<IdentityId, String>,
    /// Latest full snapshot (cache, Claude Desktop or probe) per identity.
    pub(super) full_snapshots: BTreeMap<IdentityId, AccountUsage>,
    /// Per identity, per folder the sessions ran in, per Claude Code process.
    pub(super) status_line: BTreeMap<IdentityId, Folders>,
    /// Who each folder ran as when last seen (a reading from a folder that
    /// changed hands since is dropped).
    pub(super) identity_of_folder: BTreeMap<String, IdentityId>,
    /// Whether the identity's `.claude.json` shows a claude.ai login; absent
    /// until the first cache poll has looked.
    pub(super) signed_in: BTreeMap<IdentityId, bool>,
    pub(super) failure_count: BTreeMap<IdentityId, u32>,
    pub(super) next_attempt_at: BTreeMap<IdentityId, SystemTime>,
    /// Identities whose ring is switched off: never probed or read from
    /// Claude Desktop.
    pub(super) paused: BTreeSet<IdentityId>,
    pub(super) last_external_poll_at: BTreeMap<IdentityId, SystemTime>,
    pub(super) external_misses: BTreeSet<IdentityId>,
    /// Between `start` and `stop`: saves are only scheduled while running.
    pub(super) started: bool,
    pub(super) save_due_at: Option<SystemTime>,
    // ---- the probe half (store_probes.rs) ----
    /// The registry's folders as last told (`set_accounts`): a probe plan
    /// needs the run folders' paths, kinds and activity.
    pub(super) folders: Vec<RunFolder>,
    /// When each folder's `.claude.json` last changed, as the hub's reads
    /// stamped it (a `Paths::key`): a sign of Claude Code running there.
    pub(super) config_mtime: BTreeMap<String, SystemTime>,
    /// Requested probes, first in first out.
    pub(super) forced_queue: VecDeque<(IdentityId, RefreshReason)>,
    /// The identity a probe is running for (one at a time).
    pub(super) probing: Option<IdentityId>,
    /// The `claude` file that probe was planned with, remembered if it works.
    pub(super) probing_binary: Option<PathBuf>,
    /// Requests waiting for Claude Desktop's cache to be read (deciding
    /// whether to ask Claude Code comes after).
    pub(super) pending_refresh: BTreeMap<IdentityId, PendingRefresh>,
    /// `stop()` was called: nothing is probed any more.
    pub(super) stopped: bool,
    pub(super) probe_env: Option<ProbeEnvironment>,
    /// The `claude` that last answered a probe, tried right after the
    /// Settings choice.
    pub(super) remembered_binary: Option<PathBuf>,
}

impl Default for UsageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageStore {
    /// A store with the default settings (see [`UsageStoreConfig`]).
    pub fn new() -> Self {
        Self::with_config(UsageStoreConfig::default())
    }

    /// A store for `config`.
    pub fn with_config(config: UsageStoreConfig) -> Self {
        let paths = Paths::native(&config.home);
        UsageStore {
            config,
            paths,
            accounts: Vec::new(),
            usage: BTreeMap::new(),
            fetch_state: BTreeMap::new(),
            last_probe_at: BTreeMap::new(),
            organizations: BTreeMap::new(),
            full_snapshots: BTreeMap::new(),
            status_line: BTreeMap::new(),
            identity_of_folder: BTreeMap::new(),
            signed_in: BTreeMap::new(),
            failure_count: BTreeMap::new(),
            next_attempt_at: BTreeMap::new(),
            paused: BTreeSet::new(),
            last_external_poll_at: BTreeMap::new(),
            external_misses: BTreeSet::new(),
            started: false,
            save_due_at: None,
            folders: Vec::new(),
            config_mtime: BTreeMap::new(),
            forced_queue: VecDeque::new(),
            probing: None,
            probing_binary: None,
            pending_refresh: BTreeMap::new(),
            stopped: false,
            probe_env: None,
            remembered_binary: None,
        }
    }

    // ---- lifecycle and settings ----

    /// The store runs: changes schedule a save from now on.
    pub fn start(&mut self) {
        self.started = true;
        self.stopped = false;
    }

    /// The store stops (quitting): no more saves are scheduled and nothing
    /// is probed any more, not even a request queued behind the probe that
    /// is still running (which still finishes through `finish_probe`). The
    /// caller writes [`UsageStore::save_now`] once.
    pub fn stop(&mut self) {
        self.started = false;
        self.stopped = true;
        self.pending_refresh.clear();
        for (id, _) in std::mem::take(&mut self.forced_queue) {
            self.forget_request(&id);
        }
    }

    /// The settings the store was built with, as they stand.
    pub fn config(&self) -> &UsageStoreConfig {
        &self.config
    }

    /// The probe interval setting changed (minutes, 0 = off).
    pub fn set_probe_interval_minutes(&mut self, minutes: u32) {
        self.config.probe_interval_minutes = minutes;
    }

    /// The "Also read Claude Desktop's cached usage" setting changed.
    pub fn set_reads_desktop(&mut self, on: bool) {
        self.config.reads_desktop = on;
    }

    /// Automatic probes are allowed or not (a dev run, sealed).
    pub fn set_probes_allowed(&mut self, allowed: bool) {
        self.config.probes_allowed = allowed;
    }

    /// The identities whose ring is switched off: no probe and no Claude
    /// Desktop read runs for them until they are back. Requests still queued
    /// for them are dropped, and with them their "checking..." state (the
    /// one being probed finishes).
    pub fn set_paused(&mut self, ids: BTreeSet<IdentityId>) {
        self.paused = ids;
        self.drop_requests_of_paused();
    }

    /// Whether an identity's ring is switched off.
    pub fn is_paused(&self, id: &IdentityId) -> bool {
        self.paused.contains(id)
    }

    // ---- queries ----

    /// The merged usage of every identity that has any.
    pub fn usages(&self) -> &BTreeMap<IdentityId, AccountUsage> {
        &self.usage
    }

    /// The merged usage of one identity.
    pub fn usage_of(&self, id: &IdentityId) -> Option<&AccountUsage> {
        self.usage.get(id)
    }

    /// The latest full snapshot (cache, Claude Desktop or probe) of an identity.
    pub fn full_snapshot(&self, id: &IdentityId) -> Option<&AccountUsage> {
        self.full_snapshots.get(id)
    }

    /// The probe status of an identity; `Idle` when never probed.
    pub fn fetch_state_of(&self, id: &IdentityId) -> UsageFetchState {
        self.fetch_state
            .get(id)
            .cloned()
            .unwrap_or(UsageFetchState::Idle)
    }

    /// Any identity is being probed, or waits for a requested probe.
    pub fn is_fetching(&self) -> bool {
        self.fetch_state
            .values()
            .any(|state| *state == UsageFetchState::Fetching)
    }

    /// Set an identity's probe status (the probe half and the hub use it).
    pub fn set_fetch_state(&mut self, id: &IdentityId, state: UsageFetchState) {
        self.fetch_state.insert(id.clone(), state);
    }

    /// The claude.ai organization of an identity, known while Claude
    /// Desktop readings are on.
    pub fn organization_of(&self, id: &IdentityId) -> Option<&str> {
        self.organizations.get(id).map(String::as_str)
    }

    /// When an identity was last probed.
    pub fn last_probe_of(&self, id: &IdentityId) -> Option<SystemTime> {
        self.last_probe_at.get(id).copied()
    }

    /// When the identity's newest reading of any kind was taken.
    pub fn newest_data_at(&self, id: &IdentityId) -> Option<SystemTime> {
        [
            self.full_snapshots.get(id).map(|full| full.updated_at),
            self.status_reading(id, StatusWindow::FiveHour)
                .map(|reading| reading.at),
            self.status_reading(id, StatusWindow::SevenDay)
                .map(|reading| reading.at),
        ]
        .into_iter()
        .flatten()
        .max()
    }

    /// How old a reading may get before it shows as stale, for the current
    /// probe setting (`AccountUsage::stale_threshold`).
    pub fn stale_threshold(&self) -> Duration {
        let minutes = if self.config.probes_allowed {
            self.config.probe_interval_minutes
        } else {
            0
        };
        AccountUsage::stale_threshold(minutes)
    }

    /// The ring's reading for an identity (AU§9.11): signed out wins even
    /// over old windows (they belong to the last login); then windows; then
    /// why there are none.
    pub fn ring_reading(&self, id: &IdentityId, now: SystemTime) -> RingReading {
        let state = self.fetch_state.get(id);
        if state == Some(&schedule::not_signed_in()) {
            return RingReading::SignInNeeded;
        }
        if let Some(usage) = self.usage.get(id) {
            if !ring_windows::windows(usage, now).is_empty() {
                let threshold = self.stale_threshold();
                let status = if usage.is_stale(now, threshold) {
                    RingStatus::Stale
                } else {
                    RingStatus::Ok
                };
                return RingReading::Reading {
                    usage: usage.clone(),
                    status,
                    stale_after: usage.updated_at + threshold,
                };
            }
        }
        match state {
            Some(UsageFetchState::Unavailable(text)) => RingReading::Unavailable(text.clone()),
            Some(UsageFetchState::Failed(text)) => RingReading::Failed(text.clone()),
            _ => RingReading::Waiting,
        }
    }

    /// The identity's session window as a percentage, as merged.
    pub fn five_hour(&self, id: &IdentityId) -> Option<f64> {
        self.usage
            .get(id)?
            .five_hour
            .as_ref()
            .map(|window| window.utilization)
    }

    // ---- the registry ----

    /// The registry changed (the Mac debounces it 1 s, `accountsChanged`):
    /// state kept under a folder id moves to the folder's identity, ids that
    /// are no identity any more are pruned, and a folder that changed hands
    /// takes the status lines it gave its old account with it. A first call
    /// with the launch's registry prunes what was fed before it, so call it
    /// before feeding anything.
    pub fn set_accounts(&mut self, accounts: &[Account], folders: &[RunFolder], now: SystemTime) {
        self.accounts = accounts.to_vec();
        self.folders = folders.to_vec();
        self.adopt_folder_keyed_state();
        let known: BTreeSet<IdentityId> = self
            .accounts
            .iter()
            .map(|account| account.identity_id.clone())
            .collect();
        self.usage.retain(|id, _| known.contains(id));
        self.fetch_state.retain(|id, _| known.contains(id));
        self.last_probe_at.retain(|id, _| known.contains(id));
        self.full_snapshots.retain(|id, _| known.contains(id));
        self.status_line.retain(|id, _| known.contains(id));
        self.drop_readings_of_folders_that_changed_hands();
        self.signed_in.retain(|id, _| known.contains(id));
        self.failure_count.retain(|id, _| known.contains(id));
        self.next_attempt_at.retain(|id, _| known.contains(id));
        self.last_external_poll_at
            .retain(|id, _| known.contains(id));
        self.external_misses.retain(|id| known.contains(id));
        self.organizations.retain(|id, _| known.contains(id));
        self.paused.retain(|id| known.contains(id));
        self.schedule_save(now, schedule::STATE_SAVE_DELAY);
    }

    /// The accounts as last told.
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    pub(super) fn account(&self, id: &IdentityId) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|account| account.identity_id == *id)
    }

    pub(super) fn is_identity(&self, id: &IdentityId) -> bool {
        self.account(id).is_some()
    }

    /// The identity whose folders (run or store) include `folder`.
    pub(super) fn identity_id_for(&self, folder: &str) -> Option<IdentityId> {
        let key = self.paths.key(folder);
        self.accounts
            .iter()
            .find(|account| {
                self.folders_of(account)
                    .any(|id| self.paths.key(id.as_str()) == key)
            })
            .map(|account| account.identity_id.clone())
    }

    fn folders_of<'a>(&self, account: &'a Account) -> impl Iterator<Item = &'a AccountId> {
        account.run_dirs.iter().chain(account.store_dirs.iter())
    }

    fn default_folder_key(&self) -> String {
        self.paths.key(&self.paths.default_config_dir())
    }

    /// A folder that changed hands (a VS Code window switched account; a
    /// login in a standalone folder) takes the readings it gave its old
    /// account with it: they were read under a mapping that no longer holds.
    /// `~\.claude`, while mirrored into, is attributed per session instead
    /// (see `ingest_status_line`), so its readings stay.
    fn drop_readings_of_folders_that_changed_hands(&mut self) {
        let mut now: BTreeMap<String, IdentityId> = BTreeMap::new();
        for account in &self.accounts {
            for folder in self.folders_of(account) {
                now.insert(self.paths.key(folder.as_str()), account.identity_id.clone());
            }
        }
        let default_folder = self.default_folder_key();
        for (folder, identity) in &now {
            let Some(before) = self.identity_of_folder.get(folder) else {
                continue;
            };
            if before == identity {
                continue;
            }
            if *folder == default_folder && self.config.mirrors_default {
                continue;
            }
            let accounts: Vec<IdentityId> = self
                .status_line
                .iter()
                .filter(|(account, folders)| *account != identity && folders.contains_key(folder))
                .map(|(account, _)| account.clone())
                .collect();
            for account in accounts {
                if let Some(folders) = self.status_line.get_mut(&account) {
                    folders.remove(folder);
                }
                self.publish(&account);
            }
        }
        self.identity_of_folder = now;
    }

    /// State recorded under a folder id (a status line from a folder the
    /// registry had not grouped yet) moves to the folder's identity.
    fn adopt_folder_keyed_state(&mut self) {
        let orphans: Vec<IdentityId> = self
            .status_line
            .keys()
            .filter(|id| !self.is_identity(id))
            .cloned()
            .collect();
        for id in orphans {
            let Some(identity) = self.identity_id_for(id.as_str()) else {
                continue;
            };
            let Some(folders) = self.status_line.remove(&id) else {
                continue;
            };
            for (folder, processes) in folders {
                let mine = self
                    .status_line
                    .entry(identity.clone())
                    .or_default()
                    .entry(folder)
                    .or_default();
                for (key, moved) in processes {
                    // A process heard from under both keeps its latest record.
                    match mine.get(&key) {
                        Some(kept) if moved.last_report_at <= kept.last_report_at => {}
                        _ => {
                            mine.insert(key, moved);
                        }
                    }
                }
            }
            self.publish(&identity);
        }
        let orphans: Vec<IdentityId> = self
            .full_snapshots
            .keys()
            .filter(|id| !self.is_identity(id))
            .cloned()
            .collect();
        for id in orphans {
            let Some(identity) = self.identity_id_for(id.as_str()) else {
                continue;
            };
            let Some(mut moved) = self.full_snapshots.remove(&id) else {
                continue;
            };
            moved.account_id = identity.clone();
            if merge::is_newer_full_snapshot(&moved, self.full_snapshots.get(&identity)) {
                self.full_snapshots.insert(identity.clone(), moved);
                self.publish(&identity);
            }
        }
        let orphans: Vec<(IdentityId, SystemTime)> = self
            .last_probe_at
            .iter()
            .filter(|(id, _)| !self.is_identity(id))
            .map(|(id, at)| (id.clone(), *at))
            .collect();
        for (id, at) in orphans {
            let Some(identity) = self.identity_id_for(id.as_str()) else {
                continue;
            };
            let newest = self
                .last_probe_at
                .get(&identity)
                .map_or(at, |existing| (*existing).max(at));
            self.last_probe_at.insert(identity, newest);
        }
    }

    // ---- status lines ----

    /// A live terminal session reported its account's rate limits (AU§9.5).
    /// `ctx` is the hub's attribution of the session: known -> that identity
    /// (a folder not grouped yet is kept under its folder id, moved to its
    /// identity once it has one); unsure -> the reading is left out (`~\.claude`
    /// changed hands around when the session started). A pid counts only when
    /// the hub trusts it (`ctx.trusted_pid`, the wrapper's `CLAUDE_PID`
    /// checked against the hooks' pid): a `CLAUDE_PID` inherited from the
    /// Claude Code that started this one would mix two processes' limits.
    /// Returns the history's observation when the line brought a reading that
    /// is new for its process and the account shows it.
    pub fn ingest_status_line(
        &mut self,
        m: &StatusLineMessage,
        ctx: IngestContext,
        now: SystemTime,
    ) -> Option<UsageObservation> {
        if m.five_hour.is_none() && m.seven_day.is_none() {
            return None;
        }
        let folder = self.folder_of_status_line(m, &ctx);
        let folder_key = self.paths.key(&folder);
        let account_id = match &ctx.attribution {
            Attribution::Known(Some(identity)) => identity.clone(),
            Attribution::Known(None) => self
                .identity_id_for(&folder)
                .unwrap_or_else(|| IdentityId(self.paths.normalize(&folder))),
            Attribution::Unsure(_) | Attribution::Waiting => return None,
        };

        let pid = trusted_pid(m, &ctx);
        let started_at = pid.and(ctx.pid_started);
        let process = merge::status_line_key(m.session_id.as_str(), pid, started_at);
        let previous = self
            .status_line
            .get(&account_id)
            .and_then(|folders| folders.get(&folder_key))
            .and_then(|processes| processes.get(&process))
            .cloned();
        let readings = merge::advance(
            previous.as_ref(),
            m.five_hour.as_ref(),
            m.seven_day.as_ref(),
            m.received_at,
            if previous.is_none() { started_at } else { None },
        );
        self.status_line
            .entry(account_id.clone())
            .or_default()
            .entry(folder_key)
            .or_default()
            .insert(process, readings.clone());
        self.publish(&account_id);
        if readings.five_hour != previous.as_ref().and_then(|p| p.five_hour.clone())
            || readings.seven_day != previous.as_ref().and_then(|p| p.seven_day.clone())
        {
            // Kept for the next run, so a relaunch doesn't take a process's
            // old numbers for news; not urgent.
            self.schedule_save(now, schedule::STATUS_LINE_SAVE_DELAY);
        }
        // For the history: the windows this line brought (not a repeat) that
        // the account now shows.
        let mut taken: Vec<(String, f64, Option<SystemTime>)> = Vec::new();
        for (window, id) in [
            (StatusWindow::FiveHour, SESSION_ID),
            (StatusWindow::SevenDay, WEEKLY_ID),
        ] {
            let Some(reading) = window.of(&readings) else {
                continue;
            };
            if reading.at == m.received_at
                && reading.window.utilization.is_finite()
                && self.is_shown(reading, &account_id, window)
            {
                taken.push((
                    id.to_owned(),
                    reading.window.utilization,
                    reading.window.resets_at,
                ));
            }
        }
        (!taken.is_empty()).then_some(UsageObservation {
            identity: account_id,
            source: UsageSource::StatusLine,
            observed_at: m.received_at,
            windows: taken,
        })
    }

    /// The folder a status line ran in: its transcript's, else
    /// `CLAUDE_CONFIG_DIR`, else what the hub resolved, else `~\.claude`.
    fn folder_of_status_line(&self, m: &StatusLineMessage, ctx: &IngestContext) -> String {
        let named = |text: &str| (!text.is_empty()).then(|| self.paths.normalize(text));
        m.account_id
            .as_ref()
            .and_then(|id| named(id.as_str()))
            .or_else(|| m.config_dir_env.as_deref().and_then(named))
            .or_else(|| ctx.account.as_ref().and_then(|id| named(id.as_str())))
            .unwrap_or_else(|| self.paths.default_config_dir())
    }

    /// An identity's status line window over every folder its sessions ran
    /// in.
    fn status_reading(&self, id: &IdentityId, window: StatusWindow) -> Option<UsageReading> {
        merge::most_current(&self.status_candidates(id, window)).cloned()
    }

    /// The status line readings of an identity's window that count
    /// (`merge::status_candidates`).
    pub(super) fn status_candidates(
        &self,
        id: &IdentityId,
        window: StatusWindow,
    ) -> Vec<UsageReading> {
        let Some(folders) = self.status_line.get(id) else {
            return Vec::new();
        };
        let by_folder: BTreeMap<String, Vec<UsageReading>> = folders
            .iter()
            .map(|(folder, processes)| (folder.clone(), merge::readings(processes, window)))
            .collect();
        merge::status_candidates(
            &by_folder,
            &self.default_folder_key(),
            self.config.mirrors_default,
        )
    }

    /// Whether `reading` is what the account shows for the window now.
    fn is_shown(&self, reading: &UsageReading, id: &IdentityId, window: StatusWindow) -> bool {
        let snapshot = self.full_snapshots.get(id).and_then(|full| {
            window
                .of_usage(full)
                .map(|w| UsageReading::of_snapshot(w.clone(), full))
        });
        merge::winning_status(&self.status_candidates(id, window), snapshot.as_ref()).as_ref()
            == Some(reading)
    }

    /// Recompute the published usage for one identity from its sources.
    pub(super) fn publish(&mut self, id: &IdentityId) {
        let merged = merge::merge(
            id,
            self.full_snapshots.get(id),
            &self.status_candidates(id, StatusWindow::FiveHour),
            &self.status_candidates(id, StatusWindow::SevenDay),
        );
        match merged {
            Some(usage) => {
                if self.usage.get(id) != Some(&usage) {
                    self.usage.insert(id.clone(), usage);
                }
            }
            None => {
                self.usage.remove(id);
            }
        }
    }

    /// Forget the Claude Code processes not heard from for a week: their
    /// weekly window has reset since. Part of every 20-second cycle.
    pub fn prune_status_lines(&mut self, now: SystemTime) {
        let retention = schedule::STATUS_LINE_RETENTION.as_secs_f64();
        let mut changed: Vec<IdentityId> = Vec::new();
        for (id, folders) in &mut self.status_line {
            let before: usize = folders.values().map(BTreeMap::len).sum();
            for processes in folders.values_mut() {
                processes.retain(|_, record| {
                    crate::usage::parser::seconds_between(now, record.last_report_at) < retention
                });
            }
            folders.retain(|_, processes| !processes.is_empty());
            let after: usize = folders.values().map(BTreeMap::len).sum();
            if before != after {
                changed.push(id.clone());
            }
        }
        self.status_line.retain(|_, folders| !folders.is_empty());
        for id in changed {
            self.publish(&id);
        }
    }

    // ---- full snapshots ----

    /// A full snapshot (a probe's answer, a cache, Claude Desktop) was taken
    /// in: the usage history is told about it whether or not it is the
    /// newest, and it becomes the identity's full snapshot when it is newer
    /// than the one held (they replace each other by when they were taken,
    /// whichever source they came from).
    pub fn accept_snapshot(
        &mut self,
        u: AccountUsage,
        now: SystemTime,
    ) -> Option<UsageObservation> {
        let observation = observation_of(&u);
        self.accept_full_snapshot(u, now);
        observation
    }

    /// Keep `snapshot` as the identity's full snapshot if it is newer than
    /// the one held. Returns whether it was taken.
    pub(super) fn accept_full_snapshot(&mut self, snapshot: AccountUsage, now: SystemTime) -> bool {
        if !merge::is_newer_full_snapshot(&snapshot, self.full_snapshots.get(&snapshot.account_id))
        {
            return false;
        }
        let id = snapshot.account_id.clone();
        self.full_snapshots.insert(id.clone(), snapshot);
        self.publish(&id);
        self.schedule_save(now, schedule::STATE_SAVE_DELAY);
        true
    }

    // ---- caches ----

    /// One pass over every folder's `.claude.json` (stores included: an
    /// identity with no open window still has Claude Code's cached snapshot
    /// in its store), as the hub read them: per identity, whether it is signed
    /// in and what that does to its fetch state, its organization (while
    /// Claude Desktop readings are on) and Claude Code's freshest cached
    /// snapshot of any of its folders. `Account::is_signed_in` and
    /// `organization_uuid` are the registry's, which already applied these
    /// reads' identities. Returns the observations for the usage history.
    pub fn apply_cache_reads(
        &mut self,
        reads: &[ClaudeJsonRead],
        now: SystemTime,
    ) -> Vec<UsageObservation> {
        let wants_organizations = self.config.reads_desktop;
        let mut observations = Vec::new();
        for read in reads {
            let key = self.paths.key(read.folder.as_str());
            match read.stamp {
                Some((mtime_ns, _)) => {
                    self.config_mtime.insert(key, time_of_ns(mtime_ns));
                }
                None => {
                    self.config_mtime.remove(&key);
                }
            }
        }
        let accounts = self.accounts.clone();
        for account in &accounts {
            let id = &account.identity_id;
            let was_signed_in = self.signed_in.get(id).copied();
            self.signed_in.insert(id.clone(), account.is_signed_in);
            match schedule::fetch_state_after_cache_poll(
                self.fetch_state.get(id),
                was_signed_in,
                account.is_signed_in,
            ) {
                Some(state) => {
                    self.fetch_state.insert(id.clone(), state);
                }
                None => {
                    self.fetch_state.remove(id);
                }
            }
            if wants_organizations {
                match &account.organization_uuid {
                    Some(organization) => {
                        self.organizations.insert(id.clone(), organization.clone());
                    }
                    None => {
                        self.organizations.remove(id);
                    }
                }
            }
            if let Some(snapshot) = self.freshest_cached_usage(account, reads) {
                observations.extend(observation_of(&snapshot));
                self.accept_full_snapshot(snapshot, now);
            }
        }
        if !wants_organizations {
            self.organizations.clear();
        }
        observations
    }

    /// Claude Code's cached snapshot for an identity: of its folders'
    /// `cachedUsageUtilization`s (stores included), the freshest whose
    /// account UUID is the identity's. `reads` are the hub's `.claude.json`
    /// reads, whose `cached_usage` already belongs to the folder's own
    /// login; this keeps the ones whose login is this identity's, and, for a
    /// login in two organizations, only the folders of this one.
    pub fn freshest_cached_usage(
        &self,
        account: &Account,
        reads: &[ClaudeJsonRead],
    ) -> Option<AccountUsage> {
        let uuid = account.identity_id.account_uuid()?.to_lowercase();
        let scope = account.identity_id.organization().map(str::to_lowercase);
        let mut best: Option<&AccountUsage> = None;
        for folder in self.folders_of(account) {
            let key = self.paths.key(folder.as_str());
            let Some(read) = reads
                .iter()
                .find(|r| self.paths.key(r.folder.as_str()) == key)
            else {
                continue;
            };
            let (Some(identity), Some(cached)) = (&read.identity, &read.cached_usage) else {
                continue;
            };
            if identity.account_uuid.as_deref().map(str::to_lowercase) != Some(uuid.clone()) {
                continue;
            }
            if let Some(scope) = &scope {
                if identity
                    .organization_uuid
                    .as_deref()
                    .map(str::to_lowercase)
                    .as_ref()
                    != Some(scope)
                {
                    continue;
                }
            }
            // Of equally fresh ones the first stays, as Swift's `max(by:)`.
            if best.is_none_or(|current| current.updated_at < cached.updated_at) {
                best = Some(cached);
            }
        }
        best.map(|cached| {
            let mut snapshot = cached.clone();
            snapshot.account_id = account.identity_id.clone();
            snapshot.source = UsageSource::Cache;
            snapshot
        })
    }

    // ---- Claude Desktop ----

    /// The identities whose Claude Desktop cache is due a read, with the
    /// organization to read it for, and marks them polled at `now`: visible,
    /// not paused, signed in, organization known, and (unless `force`, which
    /// still leaves Desktop alone for 5 s) not read within the cadence (a
    /// minute; 5 minutes after a miss). Empty while the setting is off.
    /// `only` limits it to those identities.
    pub fn desktop_due(
        &mut self,
        now: SystemTime,
        only: Option<&BTreeSet<IdentityId>>,
        force: bool,
    ) -> Vec<(IdentityId, String)> {
        if !self.config.reads_desktop {
            return Vec::new();
        }
        let mut due = Vec::new();
        let ids: Vec<IdentityId> = self
            .accounts
            .iter()
            .map(|account| account.identity_id.clone())
            .collect();
        for id in ids {
            if only.is_some_and(|only| !only.contains(&id))
                || self.paused.contains(&id)
                || self.signed_in.get(&id) != Some(&true)
            {
                continue;
            }
            let Some(organization) = self.organizations.get(&id).cloned() else {
                continue;
            };
            let last = self.last_external_poll_at.get(&id).copied();
            let is_due = if force {
                crate::usage::parser::seconds_between(now, last.unwrap_or_else(distant_past))
                    >= schedule::EXTERNAL_FORCED_INTERVAL.as_secs_f64()
            } else {
                schedule::is_external_poll_due(last, self.external_misses.contains(&id), now)
            };
            if !is_due {
                continue;
            }
            self.last_external_poll_at.insert(id.clone(), now);
            due.push((id, organization));
        }
        due
    }

    /// What reading Claude Desktop's cache gave for an identity. A reading
    /// is a full snapshot dated when Desktop saw it (known newer only than a
    /// minute before that: its date is the server's, to the second), so an
    /// older one never replaces a newer probe. Anything else is a miss,
    /// remembered for the slower cadence.
    pub fn accept_desktop(
        &mut self,
        id: &IdentityId,
        reading: &DesktopReading,
        now: SystemTime,
    ) -> Option<UsageObservation> {
        self.note_desktop_answered(id);
        if let DesktopReading::Reading {
            windows,
            observed_at,
            ..
        } = reading
        {
            if let Some(mut snapshot) =
                ring_windows::account_usage_from_desktop(id.clone(), windows, *observed_at)
            {
                snapshot.taken_after = Some(
                    observed_at
                        .checked_sub(schedule::EXTERNAL_CLOCK_ALLOWANCE)
                        .unwrap_or(*observed_at),
                );
                self.external_misses.remove(id);
                return self.accept_snapshot(snapshot, now);
            }
        }
        self.external_misses.insert(id.clone());
        None
    }

    // ---- persistence ----

    /// Save at `now + delay`, or sooner if a save is already due sooner (a
    /// probe's backoff must not wait behind a status line's 30 s). Only
    /// while the store runs.
    pub(super) fn schedule_save(&mut self, now: SystemTime, delay: Duration) {
        if !self.started {
            return;
        }
        let due = now + delay;
        if self.save_due_at.is_some_and(|pending| pending <= due) {
            return;
        }
        self.save_due_at = Some(due);
    }

    /// When the next save of `usage-state.json` is due: 2 s after a change,
    /// 30 s when only a status line changed.
    pub fn save_due_at(&self) -> Option<SystemTime> {
        self.save_due_at
    }

    /// The state to write when the deadline has passed (it is then cleared).
    pub fn take_state_to_save(&mut self, now: SystemTime) -> Option<UsageStateFile> {
        let due = self.save_due_at?;
        if due > now {
            return None;
        }
        self.save_due_at = None;
        Some(self.state_file())
    }

    /// The state to write right now (quitting, tests); clears the deadline.
    pub fn save_now(&mut self) -> UsageStateFile {
        self.save_due_at = None;
        self.state_file()
    }

    /// The state worth keeping, as it stands.
    pub fn state_file(&self) -> UsageStateFile {
        let mut ids: BTreeSet<&IdentityId> = BTreeSet::new();
        ids.extend(self.last_probe_at.keys());
        ids.extend(self.failure_count.keys());
        ids.extend(self.next_attempt_at.keys());
        ids.extend(self.full_snapshots.keys());
        ids.extend(self.status_line.keys());
        let mut file = UsageStateFile::default();
        for id in ids {
            let account = PersistedUsageAccount {
                last_probe_at: self.last_probe_at.get(id).copied().map(IsoSeconds),
                failure_count: i64::from(self.failure_count.get(id).copied().unwrap_or(0)),
                next_attempt_at: self.next_attempt_at.get(id).copied().map(IsoSeconds),
                last_full_reading: self
                    .full_snapshots
                    .get(id)
                    .map(PersistedAccountUsage::from_model),
                status_lines: self.saved_status_lines(id),
            };
            if !account.is_empty() {
                file.accounts.insert(id.0.clone(), account);
            }
        }
        file
    }

    /// An identity's process records worth keeping: those whose process is
    /// known by pid and start time (see `restore_status_lines`).
    fn saved_status_lines(&self, id: &IdentityId) -> Option<Vec<PersistedStatusLine>> {
        if !self.is_identity(id) {
            return None;
        }
        let folders = self.status_line.get(id)?;
        let lines: Vec<PersistedStatusLine> = folders
            .iter()
            .flat_map(|(folder, processes)| {
                processes
                    .iter()
                    .filter(|(key, _)| merge::is_process_key_with_start(key))
                    .map(move |(key, record)| PersistedStatusLine {
                        folder: folder.clone(),
                        key: key.clone(),
                        readings: PersistedStatusLineReadings {
                            last_report_at: IsoSeconds(record.last_report_at),
                            five_hour: record.five_hour.as_ref().map(PersistedReading::from_model),
                            seven_day: record.seven_day.as_ref().map(PersistedReading::from_model),
                        },
                    })
            })
            .collect();
        (!lines.is_empty()).then_some(lines)
    }

    /// Take back the last run's probe times, backoff and readings (AU§9.10).
    /// Call it after `set_accounts`. A reading older than 8 days is dropped.
    /// State saved per folder (before accounts were identities) goes to the
    /// identity the folder belongs to, the first one winning; an id nobody
    /// knows stays under its own key until the next `set_accounts` prunes it.
    pub fn restore(&mut self, file: &UsageStateFile, now: SystemTime) {
        let state = file.restored(None, now);
        let default_folder = self.default_folder_key();
        let mut by_identity: BTreeMap<IdentityId, &PersistedUsageAccount> = BTreeMap::new();
        for (id, saved) in &state.accounts {
            let id = IdentityId(id.clone());
            let key = if self.is_identity(&id) {
                Some(id.clone())
            } else if self.paths.key(id.as_str()) == default_folder && self.config.mirrors_default {
                // A mirrored `~\.claude`'s state belongs to its owner, not to
                // whoever the extension mirrored in: nobody's here.
                None
            } else if let Some(owner) = self.identity_id_for(id.as_str()) {
                Some(owner)
            } else {
                Some(id.clone())
            };
            let Some(key) = key else {
                continue;
            };
            if !by_identity.contains_key(&key) || id == key {
                by_identity.insert(key, saved);
            }
        }
        for (id, saved) in &by_identity {
            if let Some(at) = saved.last_probe_at {
                self.last_probe_at.insert(id.clone(), at.0);
            }
            if saved.failure_count > 0 {
                self.failure_count.insert(
                    id.clone(),
                    u32::try_from(saved.failure_count).unwrap_or(u32::MAX),
                );
            }
            if let Some(next) = saved.next_attempt_at {
                self.next_attempt_at.insert(id.clone(), next.0);
            }
            if let Some(reading) = &saved.last_full_reading {
                let mut reading = reading.to_model();
                reading.account_id = id.clone();
                if merge::is_newer_full_snapshot(&reading, self.full_snapshots.get(id)) {
                    self.full_snapshots.insert(id.clone(), reading);
                    self.publish(id);
                }
            }
        }
        self.restore_status_lines(&by_identity, now);
    }

    /// The last run's status line records, in folders still the identity's.
    /// Each is keyed by its process's pid and start time: a process still
    /// running goes on from its record (its old numbers stay a repeat), and
    /// an ended one's readings still count until retention ends, as they
    /// would have without the relaunch.
    fn restore_status_lines(
        &mut self,
        saved: &BTreeMap<IdentityId, &PersistedUsageAccount>,
        now: SystemTime,
    ) {
        let default_folder = self.default_folder_key();
        let retention = schedule::STATUS_LINE_RETENTION.as_secs_f64();
        for (id, account) in saved {
            if !self.is_identity(id) {
                continue;
            }
            let mut restored = 0;
            for line in account.status_lines.iter().flatten() {
                let folder = self.paths.key(&line.folder);
                let report = line.readings.last_report_at.0;
                if !merge::is_process_key_with_start(&line.key)
                    || crate::usage::parser::seconds_between(now, report) >= retention
                    || !(self.identity_id_for(&line.folder).as_ref() == Some(id)
                        || (folder == default_folder && self.config.mirrors_default))
                {
                    continue;
                }
                self.status_line
                    .entry(id.clone())
                    .or_default()
                    .entry(folder)
                    .or_default()
                    .insert(
                        line.key.clone(),
                        StatusLineRecord {
                            last_report_at: report,
                            five_hour: line
                                .readings
                                .five_hour
                                .as_ref()
                                .map(PersistedReading::to_model),
                            seven_day: line
                                .readings
                                .seven_day
                                .as_ref()
                                .map(PersistedReading::to_model),
                        },
                    );
                restored += 1;
            }
            if restored > 0 {
                self.publish(id);
            }
        }
    }
}

/// A file's modification stamp (nanoseconds since the epoch) as a time.
fn time_of_ns(ns: i128) -> SystemTime {
    let magnitude = Duration::from_nanos(ns.unsigned_abs().min(u128::from(u64::MAX)) as u64);
    if ns >= 0 {
        UNIX_EPOCH + magnitude
    } else {
        UNIX_EPOCH - magnitude
    }
}

/// The status line's process: the hub's trusted pid, when the line names the
/// same one (`ProcessID.valid`: positive, fits an `i32`).
fn trusted_pid(m: &StatusLineMessage, ctx: &IngestContext) -> Option<u32> {
    let pid = ctx.trusted_pid?;
    (m.pid == Some(pid) && pid > 0 && pid <= i32::MAX as u32).then_some(pid)
}

/// The usage history's observation of a full snapshot.
fn observation_of(snapshot: &AccountUsage) -> Option<UsageObservation> {
    let windows = ring_windows::observation_windows(snapshot);
    (!windows.is_empty()).then(|| UsageObservation {
        identity: snapshot.account_id.clone(),
        source: snapshot.source,
        observed_at: snapshot.updated_at,
        windows,
    })
}
