//! The store's turn completion, background waits, registry reconciliation,
//! Desktop-hosted sessions and periodic check (SessionStore.swift's Turn
//! completion, Background wait, Session Registry and recheck sections).
//!
//! The Mac sets a timer per pending completion, per background wait and per
//! registry rescan; the store is pure, so those are deadlines instead:
//! [`SessionStore::next_deadline`] says when the clock alone can change an
//! answer and the runtime sends `Tick` then.

use crate::model::{
    AccountId, IdentityId, NeedsInputReason, Phase, RegistryEntry, RegistrySnapshot, SessionId,
};
use crate::platform::Liveness;
use crate::runtime_types::{Job, Release};
use crate::sessions::background::{self, WaitDecision};
use crate::sessions::completion::{self, Decision};
use crate::sessions::desktop::{self, HostedLookup};
use crate::sessions::registry::{self, QUICK_RESCAN_DELAYS, SCAN_INTERVAL};
use crate::sessions::session::Session;
use crate::sessions::store::{SessionStore, ENDED_SESSION_MEMORY};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A session known only from its status line, or whose hooks sent no pid,
/// and silent this long, is gone.
pub const PIDLESS_SESSION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A session the registry shows idle since this long after the launch
/// (or before it) may have finished a turn while the app wasn't running.
pub const LAUNCH_GRACE: Duration = Duration::from_secs(5);

/// A pid whose process started this much later than the session's own is a
/// different process.
const PID_REUSE_TOLERANCE: Duration = Duration::from_secs(1);

/// The registry reads that follow a Stop: where they are, and which next.
#[derive(Debug, Clone)]
pub(super) struct QuickRescan {
    stop_at: SystemTime,
    next: usize,
}

impl QuickRescan {
    fn due(&self) -> Option<SystemTime> {
        QUICK_RESCAN_DELAYS
            .get(self.next)
            .map(|delay| self.stop_at + *delay)
    }
}

impl SessionStore {
    // ---- turn completion ----

    /// Confirms a pending Stop as the end of the turn once the session
    /// registry agrees (see `completion::decide`).
    pub(super) fn settle_pending_completion(&self, session: &mut Session, now: SystemTime) {
        let Some(stop_at) = session.completion_pending_since else {
            return;
        };
        let decision = completion::decide(
            stop_at,
            session.turn_started_at,
            session.registry_status.as_deref(),
            session.registry_status_changed_at,
            now,
            self.completion_timing,
        );
        if decision == Decision::Confirm {
            Self::confirm_pending_completion(session);
        }
    }

    /// The pending Stop was the end of the turn.
    pub(super) fn confirm_pending_completion(session: &mut Session) {
        if let Some(stop_at) = session.completion_pending_since.take() {
            session.completed_at = Some(stop_at);
        }
    }

    // ---- background wait ----

    /// Ends the wait of a turn that left background agents running once none
    /// is left, when Claude wasn't woken to say so (see `background::decide`).
    /// While it keeps waiting, the clock alone can change the answer: the
    /// store's next deadline says when.
    pub(super) fn settle_background_wait(&self, session: &mut Session, now: SystemTime) {
        let Some(since) = session.background_wait_since else {
            return;
        };
        let decision = background::decide(
            since,
            session.registry_status.as_deref(),
            session.registry_status_changed_at,
            session.last_hook_event_at,
            now,
            self.wait_timing,
        );
        if let WaitDecision::End { at } = decision {
            Self::end_background_wait(session, at);
        }
    }

    /// The agents are gone without waking Claude (stopped, or their
    /// notification never came): the turn's work was done at `at`.
    pub(super) fn end_background_wait(session: &mut Session, at: SystemTime) {
        session.background_wait_since = None;
        session.background_task_count = session
            .background_task_count
            .saturating_sub(session.background_agent_count);
        session.background_agent_count = 0;
        session.background_agent_types.clear();
        session.known_waking_agents = 0;
        // The work is done now, whatever became of the turn that waited: its
        // Stop may never have been confirmed (the agents kept the registry
        // busy), or a later turn was interrupted.
        session.completion_pending_since = None;
        session.completed_at = Some(session.completed_at.map_or(at, |done| done.max(at)));
    }

    // ---- the registry ----

    /// Asks for the folder's registry to be read again at 0.3 s and 1.2 s
    /// after a Stop (the registry goes idle moments after our hook ran). A
    /// Stop while those are pending adds nothing.
    pub(super) fn request_quick_rescan(&mut self, account: AccountId, stop_at: SystemTime) {
        self.rescan_after_stop
            .entry(account)
            .or_insert(QuickRescan { stop_at, next: 0 });
    }

    /// One read of a physical `sessions` folder: its entries are attributed
    /// to their config folders (a folder that is shared, or reached through
    /// a link, by the process's own `CLAUDE_CONFIG_DIR`) and each folder's
    /// entries create, reconcile and replace sessions. An unreadable folder
    /// says nothing; one with no entries changes nothing (a session whose
    /// process ended is dropped by the periodic check, not by its file
    /// vanishing).
    pub(super) fn apply_registry(&mut self, snapshot: RegistrySnapshot, now: SystemTime) {
        if snapshot.error.is_some() {
            return;
        }
        let sessions_dir = snapshot.sessions_dir.to_string_lossy().into_owned();
        let aliases = self.registry_aliases(&sessions_dir, snapshot.via_link);
        let is_shared = snapshot.via_link || aliases.len() > 1;
        let entries: Vec<RegistryEntry> = snapshot
            .entries
            .into_iter()
            .filter(|entry| entry.is_tracked() && entry.live)
            .collect();
        let default_dir = self.paths.default_config_dir();
        let by_folder = registry::attribute(
            &entries,
            &aliases,
            is_shared,
            &default_dir,
            &self.paths,
            &|folder| folder.to_owned(),
        );
        for (folder, entries) in by_folder {
            self.apply_registry_folder(&folder, entries, now);
        }
    }

    /// The config folders that lead to `sessions_dir`: the scan plan's group
    /// when the hub gave one, else the folder it is in (a link nobody
    /// described leads to the default folder).
    fn registry_aliases(&self, sessions_dir: &str, via_link: bool) -> Vec<String> {
        if let Some(group) = self.registry_folders.iter().find(|group| {
            self.paths
                .same(&group.folder.to_string_lossy(), sessions_dir)
        }) {
            return group.aliases.clone();
        }
        let parent = (!via_link)
            .then(|| self.paths.parent(sessions_dir))
            .flatten();
        vec![match parent {
            Some(parent) => self.paths.normalize(&parent),
            None => self.paths.default_config_dir(),
        }]
    }

    fn apply_registry_folder(
        &mut self,
        folder: &str,
        entries: Vec<RegistryEntry>,
        now: SystemTime,
    ) {
        // One pid, one registry file, one current session: a session of this
        // folder whose pid now names another session was left by /clear or
        // /resume without a SessionEnd reaching us.
        let mut by_pid: std::collections::BTreeMap<u32, &RegistryEntry> = Default::default();
        for entry in &entries {
            by_pid.entry(entry.pid).or_insert(entry);
        }
        let replaced: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .account
                    .as_ref()
                    .is_some_and(|account| self.paths.same(&account.0, folder))
            })
            .filter_map(|(id, session)| {
                let entry = by_pid.get(&session.pid?)?;
                if entry.session_id == *id {
                    return None;
                }
                // Hooks spoke after the registry changed: don't fight a race.
                match (session.last_hook_event_at, entry.status_changed_at()) {
                    (Some(hook), Some(changed)) if hook > changed => None,
                    _ => Some(id.clone()),
                }
            })
            .collect();
        for id in replaced {
            self.remove_session(&id, now);
        }

        for entry in &entries {
            if self.recently_ended_at(&entry.session_id, now).is_some() {
                continue;
            }
            let id = entry.session_id.clone();
            let Some(mut session) = self.sessions.remove(&id) else {
                let created = self.create_session_from_registry(entry, folder, now);
                self.sessions.insert(id.clone(), created);
                self.needs_task_reconstruction.insert(id.clone());
                self.schedule_sync(&id, now);
                self.look_up_host(&id, now);
                continue;
            };

            if session.pid != Some(entry.pid) {
                self.adopt_pid(&mut session, entry);
            }
            // Both the current process's: a session resumed in another host
            // (Claude Desktop, the terminal) is that host's from now on, and
            // one resumed outside Claude Desktop has no Desktop id.
            if let Some(entrypoint) = entry.entrypoint.as_deref().filter(|e| !e.is_empty()) {
                session.entrypoint = Some(entrypoint.to_owned());
            }
            session.host_session_id = entry.host_session_id.clone();
            if session.account.is_none() {
                session.account = Some(AccountId(folder.to_owned()));
            }
            session.apply_name(entry.name.as_deref(), entry.is_name_derived());

            if let Some(changed_at) = entry.status_changed_at() {
                if Some(changed_at) != session.registry_status_changed_at
                    || entry.status != session.registry_status
                {
                    session.registry_status = entry.status.clone();
                    session.registry_status_changed_at = Some(changed_at);
                    // No hook can settle a request matched to no call, so
                    // this one doesn't wait for the hooks to go quiet.
                    self.settle_answered_synthetic_request(
                        &mut session,
                        entry.status.as_deref(),
                        changed_at,
                        now,
                    );
                    // Hooks are the primary source; the registry only
                    // corrects state no hook reported (interrupts, dialogs,
                    // sessions without hooks). Status line updates don't
                    // count as hooks.
                    if changed_at > session.last_hook_event_at.unwrap_or(UNIX_EPOCH) {
                        self.reconcile(&mut session, entry, changed_at, now);
                        session.last_event_at = session.last_event_at.max(changed_at);
                    }
                    self.settle_pending_completion(&mut session, now);
                    self.settle_background_wait(&mut session, now);
                }
            }
            self.sessions.insert(id.clone(), session);
            self.look_up_host(&id, now);
        }
    }

    /// Records a (new) Claude process: its start time tells a reused pid
    /// apart.
    fn adopt_pid(&self, session: &mut Session, entry: &RegistryEntry) {
        session.pid = Some(entry.pid);
        session.pid_started_at = entry.process_started.or_else(|| {
            self.processes
                .as_ref()
                .and_then(|processes| processes.start_time(entry.pid))
        });
    }

    /// Corrects what no hook reported with what the registry says.
    fn reconcile(
        &mut self,
        session: &mut Session,
        entry: &RegistryEntry,
        changed_at: SystemTime,
        now: SystemTime,
    ) {
        match entry.status.as_deref() {
            Some("idle" | "shell") => match &session.phase {
                Phase::Processing | Phase::Compacting => {
                    if session.is_hook_backed() {
                        // Idle without a Stop: the turn was interrupted.
                        session.phase = Phase::Idle;
                        session.set_needs_input(None, changed_at);
                    } else {
                        // No hooks report this session: the registry going
                        // idle is how its turns end. The transcript tells a
                        // finished turn from an interrupted one.
                        session.phase = Phase::WaitingForInput;
                        session.completion_check_since =
                            Some(session.turn_started_at.unwrap_or(changed_at));
                        self.schedule_sync(&session.id.clone(), now);
                    }
                }
                Phase::WaitingForApproval(_) => {
                    // The dialog is gone and Claude is idle.
                    self.drop_main_approvals(session, Phase::Idle, now, false);
                }
                _ => clear_dialog(session, changed_at),
            },
            Some("waiting") => {
                if matches!(session.phase, Phase::Processing | Phase::Compacting) {
                    session.set_needs_input(
                        Some(NeedsInputReason::Dialog {
                            detail: entry.waiting_for.clone().unwrap_or_default(),
                        }),
                        changed_at,
                    );
                }
            }
            Some("busy") => {
                if matches!(session.phase, Phase::Idle | Phase::WaitingForInput) {
                    // Busy with the agents a background wait is on isn't a
                    // turn.
                    if session.completion_pending_since.is_none()
                        && session.background_wait_since.is_none()
                    {
                        session.phase = Phase::Processing;
                        if !session.is_hook_backed() {
                            session.turn_started_at = Some(changed_at);
                        }
                    }
                }
                clear_dialog(session, changed_at);
            }
            _ => {}
        }
    }

    /// A main-session request matched to no call (another hook rewrote its
    /// input while several calls of that tool ran) that the registry says was
    /// answered in the terminal: Claude Code's dialog closed and it went on
    /// after the request arrived. Its hook may live on, so it is closed too.
    /// Background agents' requests show no dialog while our hook waits, so
    /// the registry says nothing about them.
    fn settle_answered_synthetic_request(
        &mut self,
        session: &mut Session,
        registry_status: Option<&str>,
        changed_at: SystemTime,
        now: SystemTime,
    ) {
        let Phase::WaitingForApproval(active) = &session.phase else {
            return;
        };
        if registry_status != Some("busy")
            || !active.has_synthetic_tool_use_id
            || active.is_from_subagent()
            || changed_at <= active.received_at
        {
            return;
        }
        let tool_use_id = active.tool_use_id.clone();
        self.release(Release::Request {
            session: session.id.clone(),
            tool_use_id: tool_use_id.clone(),
        });
        Self::resolve_approval(session, &tool_use_id, now);
    }

    /// A session first seen in the registry. Its attribution is the
    /// folder's, certain of no one: the hub attributes it (there is no
    /// ingest context for a registry entry).
    fn create_session_from_registry(
        &mut self,
        entry: &RegistryEntry,
        folder: &str,
        now: SystemTime,
    ) -> Session {
        let changed_at = entry.status_changed_at().unwrap_or(now);
        let mut session = self.create_session(
            &entry.session_id,
            entry.cwd.as_deref().unwrap_or(""),
            changed_at,
            now,
        );
        self.adopt_pid(&mut session, entry);
        session.account = Some(AccountId(folder.to_owned()));
        session.entrypoint = entry.entrypoint.clone();
        session.host_session_id = entry.host_session_id.clone();
        session.apply_name(entry.name.as_deref(), entry.is_name_derived());
        session.last_event_at = changed_at;
        session.registry_status = entry.status.clone();
        session.registry_status_changed_at = entry.status_changed_at();
        match entry.status.as_deref() {
            Some("busy") if session.background_wait_since.is_some() => {
                // Busy with the agents it was waiting on; a turn of its own
                // would be reported by its hooks.
                session.phase = Phase::WaitingForInput;
            }
            Some("busy") => {
                session.phase = Phase::Processing;
                session.turn_started_at = Some(changed_at);
            }
            Some("waiting") => {
                session.phase = Phase::WaitingForInput;
                session.set_needs_input(
                    Some(NeedsInputReason::Dialog {
                        detail: entry.waiting_for.clone().unwrap_or_default(),
                    }),
                    changed_at,
                );
            }
            _ => {
                session.phase = Phase::Idle;
                // A turn may have finished while the app wasn't running; the
                // first sync checks the transcript. Not for a session that
                // went idle while this run was up and simply wasn't followed
                // (an account tracked only now): its replies were never
                // "while the app was down", and flagging them would flood the
                // review queue with work the user has seen.
                if let (Some(changed), Some(started)) = (entry.status_changed_at(), self.started_at)
                {
                    if changed < started + LAUNCH_GRACE {
                        session.completion_check_since = self.previous_run_alive_at;
                    }
                }
            }
        }
        self.settle_background_wait(&mut session, now);
        session
    }

    // ---- Claude Desktop-hosted sessions ----

    /// Looks for the identity a Desktop-hosted session belongs to: a job when
    /// nobody asked yet (or the answer is stale), the remembered answer
    /// otherwise.
    fn look_up_host(&mut self, id: &SessionId, now: SystemTime) {
        let Some(session) = self.sessions.get_mut(id) else {
            return;
        };
        let Some(host) = session
            .host_session_id
            .clone()
            .filter(|_| desktop::is_desktop_hosted(session.entrypoint.as_deref()))
        else {
            session.desktop_identity = None;
            return;
        };
        if self.desktop_roots.is_empty() || self.desktop_candidates.is_empty() {
            return;
        }
        match self.desktop.lookup(&host, &self.desktop_candidates, now) {
            HostedLookup::Invalid => session.desktop_identity = None,
            HostedLookup::Known(identity) => session.desktop_identity = identity,
            HostedLookup::Ask => self.effects.jobs.push(Job::DesktopHosted {
                roots: self.desktop_roots.clone(),
                host_session_id: host,
                candidates: self.desktop_candidates.clone(),
            }),
        }
    }

    /// Claude Desktop's own record named the identity of a hosted session, or
    /// none (a miss is looked at again later, never taken for anyone's).
    pub(super) fn apply_hosted(
        &mut self,
        session: &SessionId,
        identity: Option<IdentityId>,
        now: SystemTime,
    ) {
        let Some(record) = self.sessions.get_mut(session) else {
            return;
        };
        let Some(host) = record.host_session_id.clone() else {
            return;
        };
        self.desktop
            .record(&host, &self.desktop_candidates, identity.clone(), now);
        record.desktop_identity = identity;
    }

    // ---- the periodic check ----

    /// The runtime's clock reached [`SessionStore::next_deadline`]: pending
    /// completions and waits settle, the quick registry reads after a Stop
    /// are asked for and, every 3 s, the sessions are checked.
    pub(super) fn tick(&mut self, now: SystemTime) {
        let due = self
            .last_check
            .is_none_or(|last| now >= last + SCAN_INTERVAL);
        if due {
            self.recheck_all_sessions(now);
            self.last_check = Some(now);
        }

        let ids: Vec<SessionId> = self.sessions.keys().cloned().collect();
        for id in ids {
            let Some(mut session) = self.sessions.remove(&id) else {
                continue;
            };
            self.settle_pending_completion(&mut session, now);
            self.settle_background_wait(&mut session, now);
            self.sessions.insert(id.clone(), session);
            self.look_up_host(&id, now);
        }

        self.ask_for_quick_rescans(now);
    }

    /// Drops sessions whose process is gone (or whose pid now belongs to
    /// another process) and re-syncs the working ones.
    fn recheck_all_sessions(&mut self, now: SystemTime) {
        self.recently_ended.retain(|_, ended| {
            now.duration_since(*ended).unwrap_or_default() < ENDED_SESSION_MEMORY
        });
        let ids: Vec<SessionId> = self.sessions.keys().cloned().collect();
        for id in ids {
            let Some(session) = self.sessions.get(&id) else {
                continue;
            };
            if matches!(session.phase, Phase::Ended) {
                self.remove_session(&id, now);
                continue;
            }
            match session.pid {
                Some(pid) => {
                    if !self.same_process_running(pid, session.pid_started_at) {
                        self.remove_session(&id, now);
                        continue;
                    }
                }
                None => {
                    // Known only from the status line (or a hook that sent no
                    // pid) and silent since: gone.
                    let silent = now
                        .duration_since(session.last_event_at)
                        .unwrap_or_default();
                    if silent > PIDLESS_SESSION_TIMEOUT {
                        self.remove_session(&id, now);
                        continue;
                    }
                }
            }
            if matches!(
                session.phase,
                Phase::Processing | Phase::WaitingForApproval(_)
            ) {
                self.schedule_sync(&id, now);
            }
        }
    }

    /// The process runs and is the one first seen with this pid (not a pid
    /// reused by an unrelated process started later). A store with no
    /// process access never finds one gone; a probe that can't tell doesn't
    /// either.
    fn same_process_running(&self, pid: u32, started_at: Option<SystemTime>) -> bool {
        let Some(processes) = &self.processes else {
            return true;
        };
        if processes.liveness(pid) == Liveness::Gone {
            return false;
        }
        let (Some(started_at), Some(current)) = (started_at, processes.start_time(pid)) else {
            return true;
        };
        let apart = match current.duration_since(started_at) {
            Ok(apart) => apart,
            Err(earlier) => earlier.duration(),
        };
        apart < PID_REUSE_TOLERANCE
    }

    /// The registry reads 0.3 s and 1.2 s after a Stop, once each (one read
    /// serves both when a tick comes late).
    fn ask_for_quick_rescans(&mut self, now: SystemTime) {
        let accounts: Vec<AccountId> = self.rescan_after_stop.keys().cloned().collect();
        for account in accounts {
            let Some(rescan) = self.rescan_after_stop.get_mut(&account) else {
                continue;
            };
            let mut asked = false;
            while rescan.due().is_some_and(|due| now >= due) {
                rescan.next += 1;
                asked = true;
            }
            if rescan.due().is_none() {
                self.rescan_after_stop.remove(&account);
            }
            if asked {
                let folder = self.paths.join(&account.0, "sessions");
                self.effects.jobs.push(Job::ReadRegistry {
                    sessions_dir: self.paths.to_path_buf(&folder),
                    via_link: false,
                });
            }
        }
    }

    /// When the clock alone next changes an answer: a pending completion's
    /// fallback or timeout, the end of a background wait, a quick registry
    /// read after a Stop, the 3 s check. The runtime sends `Tick` then; a
    /// time in the past means now. `None` when nothing waits.
    pub fn next_deadline(&self) -> Option<SystemTime> {
        let mut deadlines: Vec<SystemTime> = Vec::new();
        for session in self.sessions.values() {
            if let Some(stop_at) = session.completion_pending_since {
                deadlines.push(completion::check_at(
                    stop_at,
                    session.turn_started_at,
                    session.registry_status.as_deref(),
                    session.registry_status_changed_at,
                    UNIX_EPOCH,
                    self.completion_timing,
                ));
            }
            if let Some(since) = session.background_wait_since {
                deadlines.push(background::check_at(
                    since,
                    session.registry_status.as_deref(),
                    session.registry_status_changed_at,
                    session.last_hook_event_at,
                    self.wait_timing,
                ));
            }
        }
        deadlines.extend(self.rescan_after_stop.values().filter_map(QuickRescan::due));
        if let (Some(last), false) = (self.last_check, self.sessions.is_empty()) {
            deadlines.push(last + SCAN_INTERVAL);
        }
        deadlines.into_iter().min()
    }
}

/// The registry's dialog is over: so is the needs-input it showed.
fn clear_dialog(session: &mut Session, at: SystemTime) {
    if matches!(
        session.needs_input_reason(),
        Some(NeedsInputReason::Dialog { .. })
    ) {
        session.set_needs_input(None, at);
    }
}
