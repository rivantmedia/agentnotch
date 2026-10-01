//! The usage store's probe half (UsageStore.swift "Requests", "Probe
//! Scheduling" and `startProbe`/`finishProbe`, AU§9.8-9.9): what to ask
//! Claude Code, when, and what its answer means. A second `impl UsageStore`,
//! pure like the first: nothing here runs a process, reads a clock or waits.
//!
//! How the hub (WP7) drives it:
//!
//! - Every cycle (and after each of the calls below) it asks
//!   [`UsageStore::due_probe`]. A plan comes back when a probe should run
//!   now; the hub runs `Job::Probe(plan)` on the Probe lane and hands the
//!   `ProbeResult` to [`UsageStore::finish_probe`]. One probe runs at a
//!   time: while [`UsageStore::is_probing`] no further plan is made.
//! - A user's request (ring click, "Check now", a new account) goes through
//!   [`UsageStore::refresh`] / [`UsageStore::refresh_all`]. The hub has just
//!   re-read the `.claude.json` files (`apply_cache_reads`); what comes back
//!   are the Claude Desktop reads to run (`Job::ReadDesktopCache`, each
//!   answer through `accept_desktop`) and the deadline up to which the caller
//!   waits for the answer: the Swift `refresh` awaited the probe for 20 s at
//!   most, here the wait is the hub's, and `is_waiting_on` says whether the
//!   request is still open (a `finish_probe` before the deadline closes it).
//! - Settings changes: `set_probe_interval_minutes`, `set_reads_desktop`,
//!   `set_probes_allowed`, `set_claude_binary_path`; rings switched off:
//!   `set_paused`; quitting: `stop`.

use crate::model::{
    distant_past, Account, AccountUsage, IdentityId, RunFolder, UsageFetchState, UsageSource,
};
use crate::platform::{CommandSpec, Roots};
use crate::runtime_types::{ProbeOutcome, ProbePlan, ProbeResult, RefreshReason, UsageObservation};
use crate::usage::parser::seconds_between;
use crate::usage::planner::{expected_login, is_default_folder, probe_folder_among};
use crate::usage::schedule::{
    self, backoff, effective_probe_interval, ProbeSchedule, CLAUDE_NOT_FOUND_TEXT,
    DISCARD_RETRY_DELAY, FORCED_REFRESH_INTERVAL, MAX_BACKOFF, MINIMUM_PROBE_INTERVAL,
    NO_RUN_FOLDER_TEXT, PROBES_OFF_TEXT, REFRESH_READ_WAIT, REFRESH_WAIT_LIMIT,
    RING_CLICK_FRESHNESS, STATE_SAVE_DELAY,
};
use crate::usage::store::UsageStore;
use crate::usage::{env, locator, probe};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Shown when a probe is asked for before the engine was told where Claude
/// Code lives (`set_probe_environment`).
pub const NOT_SET_UP_TEXT: &str = "Usage checks need a bootstrapped engine";

/// What a probe plan needs from the machine, given once by the hub (the
/// Mac's `ClaudeBinaryLocator` and `UsageProbe.environment` read these
/// themselves).
#[derive(Debug, Clone)]
pub struct ProbeEnvironment {
    /// Where Claude Code is looked for, and the probe's working folder
    /// (`<support>\usage-probe`).
    pub roots: Roots,
    /// The app's own environment, once (`std::env::vars_os`); the probe's is
    /// this, scrubbed (`usage::env`).
    pub base_env: Vec<(OsString, OsString)>,
    /// The app's `PATH`, searched for `claude.exe` and `claude.cmd` after the
    /// usual install places.
    pub env_path: OsString,
    /// "Claude Code location" in Settings (`claudeBinaryPath`), tried first.
    pub claude_binary_path: Option<PathBuf>,
}

/// A requested refresh the hub has to carry out part of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RefreshRequest {
    /// Claude Desktop's cache is to be read for these identities (and the
    /// organization to read it for) before Claude Code is asked: run
    /// `Job::ReadDesktopCache` for each and hand the answer to
    /// `accept_desktop`. Already marked as read.
    pub desktop_reads: Vec<(IdentityId, String)>,
    /// The caller waits for the answer up to this time (20 s after the
    /// request); `None` when nothing is being waited for.
    pub wait_until: Option<SystemTime>,
}

/// A request that waits for Claude Desktop's cache before it decides whether
/// to ask Claude Code (the cache costs nothing and may be fresh enough).
#[derive(Debug, Clone)]
pub struct PendingRefresh {
    reason: RefreshReason,
    /// Decided at the latest then, answered or not.
    until: SystemTime,
    /// `accept_desktop` has been told.
    answered: bool,
}

impl UsageStore {
    // ---- settings ----

    /// Where Claude Code is, what its environment starts from and the
    /// Settings choice. Without it no probe can be planned.
    pub fn set_probe_environment(&mut self, environment: ProbeEnvironment) {
        self.probe_env = Some(environment);
    }

    /// The "Claude Code location" setting changed (`None`: look for it).
    pub fn set_claude_binary_path(&mut self, path: Option<PathBuf>) {
        if let Some(environment) = self.probe_env.as_mut() {
            environment.claude_binary_path = path;
        }
    }

    /// The `claude` that last answered a probe (the hub may keep it for the
    /// next run; it is tried right after the Settings choice).
    pub fn remembered_binary(&self) -> Option<&Path> {
        self.remembered_binary.as_deref()
    }

    // ---- state ----

    /// A probe is running now.
    pub fn is_probing(&self) -> bool {
        self.probing.is_some()
    }

    /// The identity being probed, if any.
    pub fn probing_identity(&self) -> Option<&IdentityId> {
        self.probing.as_ref()
    }

    /// A requested refresh of `id` is still open: waiting for Claude
    /// Desktop's cache, queued behind another probe, or being probed. The
    /// hub's wait for a refresh ends when this turns false or at the
    /// deadline `refresh` gave.
    pub fn is_waiting_on(&self, id: &IdentityId) -> bool {
        self.pending_refresh.contains_key(id)
            || self.probing.as_ref() == Some(id)
            || self.forced_queue.iter().any(|(queued, _)| queued == id)
    }

    // ---- requests ----

    /// Fresh usage for every tracked identity whose ring is on, on request
    /// (the Settings "Check now"): each is probed (queued behind the running
    /// one), clearing its backoff, unless probed less than a minute ago.
    /// Claude Desktop's cache is to be re-read at once for all of them.
    pub fn refresh_all(&mut self, now: SystemTime) -> RefreshRequest {
        let targets: BTreeSet<IdentityId> = self
            .tracked_ids()
            .into_iter()
            .filter(|id| !self.paused.contains(id))
            .collect();
        for id in &targets {
            self.enqueue_forced(id, true, RefreshReason::Manual, now);
        }
        let desktop_reads = self.desktop_due(now, Some(&targets), true);
        RefreshRequest {
            desktop_reads,
            wait_until: targets
                .iter()
                .any(|id| self.is_waiting_on(id))
                .then(|| now + REFRESH_WAIT_LIMIT),
        }
    }

    /// Fresh usage for one identity, on request. Call it right after the
    /// caches were re-read (`apply_cache_reads`): the newest reading of any
    /// source decides.
    ///
    /// - `RingClick` and `Launch` ask Claude Code only when the newest
    ///   reading is more than 2 minutes old, and respect a running backoff;
    /// - `Manual` and `NewAccount` ask unless Claude Code was asked in the
    ///   last minute, and clear the backoff;
    /// - `Interval` is the schedule's own: it asks nothing (`due_probe`
    ///   decides).
    ///
    /// None asks for an identity whose ring is switched off. When Claude
    /// Desktop's cache is due a read, the decision waits for it (each answer
    /// goes through `accept_desktop`) for up to 5 s: a fresh Desktop reading
    /// holds the request off, as a fresh `.claude.json` does.
    pub fn refresh(
        &mut self,
        id: &IdentityId,
        reason: RefreshReason,
        now: SystemTime,
    ) -> RefreshRequest {
        if !self.is_identity(id) {
            return RefreshRequest::default();
        }
        let only: BTreeSet<IdentityId> = BTreeSet::from([id.clone()]);
        let desktop_reads = self.desktop_due(now, Some(&only), true);
        // Claude Code is never asked about an account whose ring is off.
        if !self.paused.contains(id) {
            if desktop_reads.is_empty() {
                self.decide_refresh(id, reason, now);
            } else {
                self.pending_refresh.insert(
                    id.clone(),
                    PendingRefresh {
                        reason,
                        until: now + REFRESH_READ_WAIT,
                        answered: false,
                    },
                );
            }
        }
        RefreshRequest {
            desktop_reads,
            wait_until: self.is_waiting_on(id).then(|| now + REFRESH_WAIT_LIMIT),
        }
    }

    /// Claude Desktop answered a read: a request waiting for it decides now.
    pub(super) fn note_desktop_answered(&mut self, id: &IdentityId) {
        if let Some(pending) = self.pending_refresh.get_mut(id) {
            pending.answered = true;
        }
    }

    /// The requests whose reads are in (or past their wait) decide.
    fn decide_pending(&mut self, now: SystemTime) {
        let ready: Vec<IdentityId> = self
            .pending_refresh
            .iter()
            .filter(|(_, pending)| pending.answered || pending.until <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            if let Some(pending) = self.pending_refresh.remove(&id) {
                if !self.paused.contains(&id) {
                    self.decide_refresh(&id, pending.reason, now);
                }
            }
        }
    }

    fn decide_refresh(&mut self, id: &IdentityId, reason: RefreshReason, now: SystemTime) {
        match reason {
            RefreshReason::RingClick | RefreshReason::Launch => {
                if let Some(newest) = self.newest_data_at(id) {
                    if seconds_between(now, newest) <= RING_CLICK_FRESHNESS.as_secs_f64() {
                        return;
                    }
                }
                if self.next_attempt_at.get(id).is_some_and(|next| *next > now) {
                    return;
                }
                self.enqueue_forced(id, false, reason, now);
            }
            RefreshReason::Manual | RefreshReason::NewAccount => {
                self.enqueue_forced(id, true, reason, now);
            }
            RefreshReason::Interval => {}
        }
    }

    fn enqueue_forced(
        &mut self,
        id: &IdentityId,
        clearing_backoff: bool,
        reason: RefreshReason,
        now: SystemTime,
    ) {
        if clearing_backoff {
            self.failure_count.remove(id);
            self.next_attempt_at.remove(id);
        }
        if self
            .last_probe_at
            .get(id)
            .is_some_and(|last| seconds_between(now, *last) < FORCED_REFRESH_INTERVAL.as_secs_f64())
        {
            return;
        }
        if self.probing.as_ref() == Some(id) || self.forced_queue.iter().any(|(q, _)| q == id) {
            return;
        }
        self.forced_queue.push_back((id.clone(), reason));
        if self.fetch_state.get(id) != Some(&UsageFetchState::Fetching) {
            self.fetch_state
                .insert(id.clone(), UsageFetchState::Fetching);
        }
    }

    /// A request that will not run: no "checking..." left behind.
    pub(super) fn forget_request(&mut self, id: &IdentityId) {
        if self.fetch_state.get(id) == Some(&UsageFetchState::Fetching)
            && self.probing.as_ref() != Some(id)
        {
            self.fetch_state.insert(id.clone(), UsageFetchState::Idle);
        }
    }

    /// Switching a ring off drops its queued request: no probe later, no
    /// "checking..." left behind.
    pub(super) fn drop_requests_of_paused(&mut self) {
        let paused = &self.paused;
        let (dropped, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.forced_queue)
            .into_iter()
            .partition(|(id, _)| paused.contains(id) && self.probing.as_ref() != Some(id));
        self.forced_queue = kept.into();
        self.pending_refresh.retain(|id, _| !paused.contains(id));
        for (id, _) in dropped {
            self.forget_request(&id);
        }
    }

    // ---- what to probe ----

    /// The probe to run now, if any (the hub then runs `Job::Probe`):
    /// nothing while one runs or after `stop`; else the requested ones first
    /// in order, then the schedule's pick. An identity nobody is signed in
    /// to, or without a run folder, or when probes are off, is not probed:
    /// its state says why.
    pub fn due_probe(&mut self, now: SystemTime) -> Option<ProbePlan> {
        if self.stopped || self.probing.is_some() {
            return None;
        }
        self.decide_pending(now);
        while let Some((id, reason)) = self.forced_queue.pop_front() {
            if !self.is_identity(&id) {
                // Forgotten while it waited; don't leave a spinner behind.
                self.fetch_state.remove(&id);
                continue;
            }
            if self.signed_in.get(&id) == Some(&false) {
                self.fetch_state.insert(id, schedule::not_signed_in());
                continue;
            }
            if let Some(plan) = self.plan_probe(&id, reason, now) {
                return Some(plan);
            }
        }
        self.scheduled_probe(now)
    }

    /// The most out-of-date identity that is due on the schedule
    /// (`schedule::next_scheduled_probe`), among the tracked ones a run
    /// folder holds: a store is never probed, nor an account the user
    /// stopped tracking.
    fn scheduled_probe(&mut self, now: SystemTime) -> Option<ProbePlan> {
        if !self.config.probes_allowed || self.config.probes_disabled {
            return None;
        }
        let interval = effective_probe_interval(self.config.probe_interval_minutes)?;
        let candidates: Vec<IdentityId> = self
            .accounts
            .iter()
            .filter(|account| account.is_tracked && !self.run_folders_of(account).is_empty())
            .map(|account| account.identity_id.clone())
            .collect();
        let mut newest: BTreeMap<IdentityId, SystemTime> = BTreeMap::new();
        let mut newest_full: BTreeMap<IdentityId, SystemTime> = BTreeMap::new();
        for id in &candidates {
            if let Some(at) = self.newest_data_at(id) {
                newest.insert(id.clone(), at);
            }
            if let Some(full) = self.full_snapshots.get(id) {
                newest_full.insert(id.clone(), full.updated_at);
            }
        }
        let next = schedule::next_scheduled_probe(
            &candidates,
            interval,
            now,
            &ProbeSchedule {
                signed_in: Some(&self.signed_in),
                paused: Some(&self.paused),
                next_attempt_at: Some(&self.next_attempt_at),
                last_probe_at: Some(&self.last_probe_at),
                newest_data_at: Some(&newest),
                newest_full_at: Some(&newest_full),
            },
        )?;
        self.plan_probe(&next, RefreshReason::Interval, now)
    }

    /// The account's run folders (never a store), as the registry listed
    /// them.
    fn run_folders_of(&self, account: &Account) -> Vec<RunFolder> {
        account
            .run_dirs
            .iter()
            .filter_map(|id| self.folders.iter().find(|folder| folder.id == *id))
            .filter(|folder| folder.kind == crate::model::FolderKind::Run)
            .cloned()
            .collect()
    }

    /// Plans the probe of `id`: the folder, the `claude` to run, its command
    /// line and environment. `None` (with the state saying why) when it
    /// can't run; the folder's login is checked by the probe itself, right
    /// before and after it runs.
    fn plan_probe(
        &mut self,
        id: &IdentityId,
        reason: RefreshReason,
        now: SystemTime,
    ) -> Option<ProbePlan> {
        // Sealed runs never launch Claude Code, not even on request.
        if self.config.probes_disabled {
            self.fetch_state.insert(
                id.clone(),
                UsageFetchState::Unavailable(PROBES_OFF_TEXT.to_owned()),
            );
            return None;
        }
        let account = self.account(id)?.clone();
        // Never in a store: only a folder Claude Code runs in as this identity.
        let run_dirs = self.run_folders_of(&account);
        let no_run_folder = UsageFetchState::Unavailable(NO_RUN_FOLDER_TEXT.to_owned());
        if run_dirs.is_empty() {
            self.fetch_state.insert(id.clone(), no_run_folder);
            return None;
        }
        // Where: the most recently active run folder.
        let activity: Vec<Option<SystemTime>> = run_dirs
            .iter()
            .map(|folder| {
                // `~\.claude.json` is rewritten by every mirror and every
                // Claude Code in `~\.claude`: not a sign of this account.
                let mirrored = self.config.mirrors_default && is_default_folder(folder);
                let modified = (!mirrored)
                    .then(|| self.config_mtime.get(&self.paths.key(folder.id.as_str())))
                    .flatten()
                    .copied();
                [folder.last_seen_at, modified].into_iter().flatten().max()
            })
            .collect();
        let Some(folder) = probe_folder_among(
            &run_dirs,
            &activity,
            &is_default_folder,
            self.config.mirrors_default,
        ) else {
            self.fetch_state.insert(id.clone(), no_run_folder);
            return None;
        };

        let Some(environment) = self.probe_env.clone() else {
            self.fail_planning(id, NOT_SET_UP_TEXT, now);
            return None;
        };
        // The same search as `locator::locate_claude_from`, keeping which
        // file it found: that is what a working run remembers.
        let found = locator::candidates(
            &environment.roots,
            environment.claude_binary_path.as_deref(),
            self.remembered_binary.as_deref(),
            &environment.env_path,
        )
        .into_iter()
        .find(|path| path.exists());
        let Some(found) = found else {
            self.fail_planning(id, CLAUDE_NOT_FOUND_TEXT, now);
            return None;
        };
        let binary = locator::binary_for(&found, &environment.env_path, &|path| path.exists());

        // Unset only for `~\.claude`. Any other folder runs with the variable
        // naming it (the registry records it so, as the Mac's does); were it
        // missing, Claude Code would run as `~\.claude`'s login while the
        // checks before and after read this folder's: another account's
        // answer on this ring.
        let config_dir_env = folder
            .config_dir_env
            .clone()
            .filter(|raw| !raw.is_empty())
            .or_else(|| {
                let dir = folder.config_dir.to_string_lossy();
                (!self.paths.is_default_config_dir(&dir)).then(|| dir.into_owned())
            });
        let binary_dir = binary.program.parent().unwrap_or(Path::new(""));
        let spec = CommandSpec {
            program: binary.program.clone(),
            args: probe::arguments(&binary.prefix_args),
            env: env::scrubbed_env(&environment.base_env, config_dir_env.as_deref(), binary_dir),
            cwd: environment.roots.usage_probe_dir(),
        };
        let identity_file = PathBuf::from(self.paths.global_config_file(
            &folder.config_dir.to_string_lossy(),
            config_dir_env.as_deref(),
        ));
        self.probing = Some(id.clone());
        self.probing_binary = Some(found);
        self.fetch_state
            .insert(id.clone(), UsageFetchState::Fetching);
        Some(ProbePlan {
            identity: id.clone(),
            folder: folder.id.clone(),
            config_dir: folder.config_dir.clone(),
            config_dir_env,
            binary,
            spec,
            reason,
            planned_at: now,
            identity_file,
            expected: expected_login(&account),
        })
    }

    /// A probe that couldn't even start (no Claude Code): a failure like any
    /// other, so the schedule backs off instead of looking again at once.
    fn fail_planning(&mut self, id: &IdentityId, reason: &str, now: SystemTime) {
        self.last_probe_at.insert(id.clone(), now);
        self.note_failure(id, reason, now);
        self.schedule_save(now, STATE_SAVE_DELAY);
    }

    fn note_failure(&mut self, id: &IdentityId, reason: &str, now: SystemTime) {
        let failures = self.failure_count.get(id).copied().unwrap_or(0) + 1;
        self.failure_count.insert(id.clone(), failures);
        self.next_attempt_at
            .insert(id.clone(), now + backoff(failures));
        self.fetch_state
            .insert(id.clone(), UsageFetchState::Failed(reason.to_owned()));
    }

    // ---- what a probe found ----

    /// A probe's result. Returns the reading for the usage history when it
    /// brought one.
    ///
    /// - The folder changed hands (`folder_identity_after` is not the
    ///   probed identity): thrown away, whatever it says, as it may be
    ///   another account's. Nothing is recorded against the account, no
    ///   backoff; it is tried again after a minute at the earliest.
    /// - A reading is kept (a seeded one, Claude Code's own fallback, under
    ///   the date its `.claude.json` copy has, and counted as rate limited
    ///   when that is older than 90 s), the backoff reset and the `claude`
    ///   that ran remembered.
    /// - Unavailable (also a signed-out login): next attempt in 30 minutes.
    /// - Rate limited: back off at least 5 minutes, said so in words that
    ///   aren't the account's own limit.
    /// - Failed: back off, and say why.
    pub fn finish_probe(&mut self, r: ProbeResult, now: SystemTime) -> Option<UsageObservation> {
        let id = r.plan.identity.clone();
        let binary = if self.probing.as_ref() == Some(&id) {
            self.probing = None;
            self.probing_binary.take()
        } else {
            None
        };
        if !self.is_identity(&id) {
            return None;
        }
        if r.folder_identity_after.as_ref() != Some(&id) {
            self.discard_probe(&id, now);
            return None;
        }
        self.last_probe_at.insert(id.clone(), now);

        let mut observation = None;
        match r.outcome {
            ProbeOutcome::Reading(mut usage) => {
                // Never another account's answer.
                usage.account_id = id.clone();
                let rate_limited = usage.source == UsageSource::Cache
                    && probe::is_stale_seed(usage.updated_at, now);
                observation = self.accept_snapshot(usage, now);
                if rate_limited {
                    self.note_rate_limited(&id, now, None);
                } else {
                    self.failure_count.remove(&id);
                    self.next_attempt_at.remove(&id);
                    self.fetch_state.insert(id.clone(), UsageFetchState::Idle);
                }
                if binary.is_some() {
                    self.remembered_binary = binary;
                }
            }
            ProbeOutcome::SignedOut => {
                self.note_unavailable(&id, schedule::NOT_SIGNED_IN_TEXT.to_owned(), now);
            }
            ProbeOutcome::Unavailable(reason) => self.note_unavailable(&id, reason, now),
            ProbeOutcome::RateLimited { retry_after } => {
                self.note_rate_limited(&id, now, retry_after);
            }
            ProbeOutcome::Failed(reason) => self.note_failure(&id, &reason, now),
        }
        self.schedule_save(now, STATE_SAVE_DELAY);
        observation
    }

    fn note_unavailable(&mut self, id: &IdentityId, reason: String, now: SystemTime) {
        self.failure_count.remove(id);
        self.next_attempt_at.insert(id.clone(), now + MAX_BACKOFF);
        self.fetch_state
            .insert(id.clone(), UsageFetchState::Unavailable(reason));
    }

    fn note_rate_limited(
        &mut self,
        id: &IdentityId,
        now: SystemTime,
        retry_after: Option<Duration>,
    ) {
        let failures = self.failure_count.get(id).copied().unwrap_or(0) + 1;
        self.failure_count.insert(id.clone(), failures);
        let wait = backoff(failures)
            .max(MINIMUM_PROBE_INTERVAL)
            .max(retry_after.unwrap_or(Duration::ZERO));
        let retry_at = now + wait;
        self.next_attempt_at.insert(id.clone(), retry_at);
        self.fetch_state.insert(
            id.clone(),
            UsageFetchState::Failed(schedule::paused_text(retry_at, now)),
        );
    }

    /// A probe whose folder changed hands: its answer (if any) would be
    /// another account's. Not a failure (no backoff), but not straight away
    /// either: the folder may still be changing hands.
    fn discard_probe(&mut self, id: &IdentityId, now: SystemTime) {
        let retry = now + DISCARD_RETRY_DELAY;
        let at = self
            .next_attempt_at
            .get(id)
            .copied()
            .unwrap_or_else(distant_past)
            .max(retry);
        self.next_attempt_at.insert(id.clone(), at);
        if self.fetch_state.get(id) == Some(&UsageFetchState::Fetching) {
            self.fetch_state.insert(id.clone(), UsageFetchState::Idle);
        }
    }

    /// The identity's latest subscription plan ("max", "pro"), as the last
    /// full reading named it (the hub tells the registry, which shows it).
    pub fn subscription_type_of(&self, id: &IdentityId) -> Option<&str> {
        self.full_snapshots
            .get(id)
            .and_then(|snapshot: &AccountUsage| snapshot.subscription_type.as_deref())
    }
}
