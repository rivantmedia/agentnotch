//! The session store (SessionStore.swift's port, HS§5): every hook, status
//! line, registry, transcript, interrupt and review input goes through
//! [`SessionStore::apply`], strictly in order, and changes the sessions
//! only there. Pure: file work comes back as [`Job`]s in the effects, held
//! PermissionRequests to close as [`Release`]s, and every time the store
//! records (turn start, completion, review, failure) is the time the hook
//! server received the event, not the time it was processed, so a backlog
//! cannot reorder them against the user's actions.
//!
//! This file holds the store, the hook and status line handlers and the
//! phase rules. Its siblings are impl blocks of the same struct:
//! - `store_tools`: approvals, tool and subagent tracking, answers;
//! - `store_turns`: turn completion, background waits, the registry, the
//!   periodic check;
//! - `store_review`: the review queue and attention transitions;
//! - `store_transcript`: transcript syncs, chat, interrupts.

use crate::core::paths::{PathStyle, Paths};
use crate::model::{
    AccountId, AccountSighting, ChatHistory, HookEvent, NeedsInputReason, PermissionContext, Phase,
    SessionId, SessionView, StatusLineMessage,
};
use crate::runtime_types::{IngestContext, Release, SessionEffects, SessionInput};
use crate::sessions::attention;
use crate::sessions::background;
use crate::sessions::completion::CompletionTiming;
use crate::sessions::phase;
use crate::sessions::session::{Session, SessionTitleSource, SubagentState};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

/// Claude Code's user message after an automatic compaction; a Stop right
/// after it is not a finished task.
pub const CONTEXT_RESUME_PREFIX: &str =
    "This session is being continued from a previous conversation";

/// How long ended session ids are remembered: late status line or registry
/// data must not revive them.
pub const ENDED_SESSION_MEMORY: Duration = Duration::from_secs(10 * 60);

/// At most one account sighting per session per this long (and at once when
/// the session's account changes).
pub const SIGHTING_INTERVAL: Duration = Duration::from_secs(60);

/// Sightings remembered before the stale ones are dropped.
const MAX_SIGHTINGS: usize = 512;

/// The folder shared history lives in (`~\.claude-shared`): a transcript path
/// through it names no account.
const SHARED_HISTORY_FOLDER: &str = ".claude-shared";

/// All Claude sessions this run knows of.
pub struct SessionStore {
    pub(super) sessions: BTreeMap<SessionId, Session>,
    /// Sessions that ended recently; late status line data must not revive
    /// them.
    pub(super) recently_ended: BTreeMap<SessionId, SystemTime>,
    /// Sessions first seen mid-flight whose task list is rebuilt from the
    /// transcript (wp5-10).
    pub(super) needs_task_reconstruction: BTreeSet<SessionId>,
    /// Sessions whose transcript is read again, and when (the 100 ms
    /// debounce; the sync jobs are wp5-10's).
    pub(super) sync_due: BTreeMap<SessionId, SystemTime>,
    /// The folder each session was last sighted in, and when.
    sightings: BTreeMap<SessionId, (AccountId, SystemTime)>,
    /// Folders whose registry is rescanned soon, with the time of the Stop
    /// that asked (the quick rescans at 0.3 s and 1.2 s are wp5-8's).
    pub(super) rescan_after_stop: BTreeMap<AccountId, SystemTime>,
    pub(super) paths: Paths,
    pub(super) completion_timing: CompletionTiming,
    pub(super) wait_timing: background::WaitTiming,
    effects: SessionEffects,
}

impl Default for SessionStore {
    fn default() -> Self {
        SessionStore::new()
    }
}

impl SessionStore {
    /// A store with the standard timings and no home folder (the hub gives
    /// the real one through [`SessionStore::with_paths`]).
    pub fn new() -> Self {
        SessionStore {
            sessions: BTreeMap::new(),
            recently_ended: BTreeMap::new(),
            needs_task_reconstruction: BTreeSet::new(),
            sync_due: BTreeMap::new(),
            sightings: BTreeMap::new(),
            rescan_after_stop: BTreeMap::new(),
            paths: Paths::new(PathStyle::native(), ""),
            completion_timing: CompletionTiming::STANDARD,
            wait_timing: background::WaitTiming::STANDARD,
            effects: SessionEffects::default(),
        }
    }

    /// The path rules and home folder: config folders are derived with them
    /// and requests show `~` for the home.
    pub fn with_paths(mut self, paths: Paths) -> Self {
        self.paths = paths;
        self
    }

    /// How long a Stop waits to be confirmed as the end of its turn, and
    /// when a wait on background agents gives up (the tests' `.immediate`).
    pub fn with_timing(
        mut self,
        completion: CompletionTiming,
        wait: background::WaitTiming,
    ) -> Self {
        self.completion_timing = completion;
        self.wait_timing = wait;
        self
    }

    // ---- reading ----

    /// Every session, sorted by project name, then id.
    pub fn views(&self) -> Vec<SessionView> {
        let mut views: Vec<SessionView> = self.sessions.values().map(Session::to_view).collect();
        views.sort_by(|a, b| {
            (a.project_name.as_str(), a.id.as_str()).cmp(&(b.project_name.as_str(), b.id.as_str()))
        });
        views
    }

    pub fn view(&self, id: &SessionId) -> Option<SessionView> {
        self.sessions.get(id).map(Session::to_view)
    }

    pub fn chat(&self, id: &SessionId) -> Option<ChatHistory> {
        self.sessions.get(id).map(Session::chat_history)
    }

    /// The record itself (the engine's own code and its tests; everything
    /// else reads [`SessionView`]s).
    pub fn session(&self, id: &SessionId) -> Option<&Session> {
        self.sessions.get(id)
    }

    // ---- input ----

    /// Applies one input: the only way the sessions change. Inputs must be
    /// applied in the order they arrived (an answer must not overtake a
    /// PostToolUse).
    pub fn apply(&mut self, input: SessionInput, now: SystemTime) -> SessionEffects {
        let before = self.views();
        self.effects = SessionEffects::default();
        match input {
            SessionInput::Hook { event, ctx } => self.process_hook(event, Some(&ctx), now),
            SessionInput::Held(held) => {
                // The request as it arrived, with the id the hook is held
                // under (ingress matched it to the preceding PreToolUse).
                let mut event = held.event;
                event.session_id = held.session_id;
                event.tool_use_id = Some(held.tool_use_id);
                event.has_synthetic_tool_use_id = held.has_synthetic_tool_use_id;
                if held.agent_id.is_some() {
                    event.agent_id = held.agent_id;
                }
                event.received_at = held.received_at;
                self.process_hook(event, None, now);
            }
            SessionInput::PermissionFailed {
                session,
                tool_use_id,
            } => self.permission_socket_failed(&session, &tool_use_id, now),
            SessionInput::PermissionResolved {
                session,
                tool_use_id,
                answer,
            } => self.permission_resolved(&session, &tool_use_id, &answer, now),
            SessionInput::StatusLine { message, ctx } => {
                self.process_status_line(message, &ctx, now)
            }
            SessionInput::Registry(snapshot) => self.apply_registry(snapshot, now),
            SessionInput::TranscriptSynced(delta) => self.apply_transcript_synced(delta, now),
            SessionInput::Hosted { session, identity } => {
                self.apply_hosted(&session, identity, now)
            }
            SessionInput::Interrupt { session, at } => self.apply_interrupt(&session, at, now),
            SessionInput::Review(action) => self.apply_review(action, now),
            SessionInput::AccountsChanged(change) => self.apply_accounts_changed(change, now),
            SessionInput::Tick => self.tick(now),
        }
        let mut effects = std::mem::take(&mut self.effects);
        effects.changed = effects.changed || before != self.views();
        effects
    }

    /// Closes a held request without an answer (once per apply).
    pub(super) fn release(&mut self, release: Release) {
        if !self.effects.release.contains(&release) {
            self.effects.release.push(release);
        }
    }

    // ---- hook events ----

    fn process_hook(&mut self, event: HookEvent, ctx: Option<&IngestContext>, now: SystemTime) {
        if event.is_from_ignored_session() {
            // Unattended and SDK sessions are none of ours; a request they
            // hold open would wait for an answer nobody shows.
            if event.expects_response() {
                self.release(Release::Session(event.session_id.clone()));
            }
            return;
        }
        self.note_sighting(
            &event.session_id,
            event.transcript_path.as_deref(),
            event.config_dir_env.as_deref(),
            now,
        );

        let id = event.session_id.clone();
        let at = event.received_at;
        if event.event == "SessionEnd" {
            self.remove_session(&id, now);
            return;
        }

        let is_new = !self.sessions.contains_key(&id);
        let mut session = match self.sessions.remove(&id) {
            Some(session) => session,
            None => self.create_session(&id, &event.cwd, at, now),
        };
        if is_new {
            self.recently_ended.remove(&id);
            // A session first seen mid-flight: its progress so far is only
            // in the transcript.
            let starts_fresh = event.event == "SessionStart"
                && matches!(event.source.as_deref(), Some("startup" | "clear"));
            if !starts_fresh {
                self.needs_task_reconstruction.insert(id.clone());
            }
        }

        self.apply_metadata(&event, ctx, &mut session);
        session.last_activity = at;
        session.last_event_at = at;
        session.last_hook_event_at = Some(at);

        if !event.is_subagent_event()
            && (phase::resumes_turn(&event) || event.event == "StopFailure")
        {
            // Claude went on (a Stop hook continued the turn, a wake-up, a
            // new prompt): the Stop before this wasn't the end of the turn.
            session.completion_pending_since = None;
        }

        let previous_phase = session.phase.clone();
        if let Some(target) = phase::determine_phase(&event) {
            self.apply_phase(target, &event, &mut session, is_new, at);
        }

        self.apply_lifecycle(
            &event,
            (!is_new).then_some(previous_phase),
            &mut session,
            at,
        );
        apply_needs_input(&event, &mut session);

        if event.event == "PermissionRequest" {
            self.mark_waiting_for_approval(&event, &mut session);
        }

        session.tasks.apply(&event);
        self.track_tools(&event, &mut session, at);
        self.track_subagents(&event, &mut session, at);

        self.sessions.insert(id.clone(), session);

        if phase::should_sync_file(&event) || is_new {
            self.schedule_sync(&id, now);
        }
    }

    /// Identity, account and descriptive fields carried by every event.
    fn apply_metadata(
        &self,
        event: &HookEvent,
        ctx: Option<&IngestContext>,
        session: &mut Session,
    ) {
        let pid = ctx
            .and_then(|ctx| ctx.trusted_pid)
            .or(event.pid)
            .filter(|pid| *pid > 0);
        if let Some(pid) = pid {
            if session.pid != Some(pid) {
                // A new Claude process: its start time tells a reused pid
                // apart.
                session.pid = Some(pid);
                session.pid_started_at = ctx.and_then(|ctx| ctx.pid_started);
            } else if session.pid_started_at.is_none() {
                session.pid_started_at = ctx.and_then(|ctx| ctx.pid_started);
            }
        }
        if let Some(terminal) = &event.terminal {
            session.terminal = Some(terminal.clone());
        }
        if !event.cwd.is_empty() {
            session.current_cwd = event.cwd.clone();
        }
        if let Some(path) = event.transcript_path.as_deref().filter(|p| !p.is_empty()) {
            if !event.is_subagent_event() || session.transcript_path.is_none() {
                session.transcript_path = Some(path.to_owned());
            }
        }
        if event.transcript_path.is_some()
            || event.config_dir_env.is_some()
            || session.account.is_none()
        {
            session.account = Some(self.account_of(
                ctx,
                event.transcript_path.as_deref(),
                event.config_dir_env.as_deref(),
            ));
        }
        if let Some(env) = event.config_dir_env.as_deref().filter(|e| !e.is_empty()) {
            session.config_dir_env = Some(env.to_owned());
        }
        if let Some(entrypoint) = event.entrypoint.as_deref().filter(|e| !e.is_empty()) {
            session.entrypoint = Some(entrypoint.to_owned());
        }
        if !event.is_subagent_event() {
            if let Some(mode) = event.permission_mode.as_deref().filter(|m| !m.is_empty()) {
                session.permission_mode = Some(mode.to_owned());
            }
        }
        if let Some(model) = event.model.as_deref().filter(|m| !m.is_empty()) {
            session.model = Some(model.to_owned());
        }
        session.apply_title(event.session_title.as_deref(), SessionTitleSource::Hook);
        self.apply_attribution(ctx, session, event.received_at);
    }

    /// What the hub says the session counts for; the time it started saying
    /// so is kept for the cloud's placement grace.
    fn apply_attribution(
        &self,
        ctx: Option<&IngestContext>,
        session: &mut Session,
        at: SystemTime,
    ) {
        if let Some(ctx) = ctx {
            if session.attribution != ctx.attribution {
                session.attribution = ctx.attribution.clone();
                session.attribution_since = at;
            }
        }
    }

    /// The session's folder: the hub's answer, else the transcript path,
    /// then `CLAUDE_CONFIG_DIR`, then `~\.claude` (SessionFilter.configDir).
    fn account_of(
        &self,
        ctx: Option<&IngestContext>,
        transcript_path: Option<&str>,
        config_dir_env: Option<&str>,
    ) -> AccountId {
        ctx.and_then(|ctx| ctx.account.clone())
            .unwrap_or_else(|| self.config_dir_of(transcript_path, config_dir_env))
    }

    fn config_dir_of(
        &self,
        transcript_path: Option<&str>,
        config_dir_env: Option<&str>,
    ) -> AccountId {
        let shared = self
            .paths
            .key(&self.paths.join(self.paths.home(), SHARED_HISTORY_FOLDER));
        AccountId(
            self.paths
                .session_config_dir(transcript_path, config_dir_env, |dir| {
                    self.paths.key(dir) == shared
                }),
        )
    }

    /// Moves the phase, protecting finished sessions and pending approvals
    /// from events that don't concern them.
    fn apply_phase(
        &mut self,
        target: Phase,
        event: &HookEvent,
        session: &mut Session,
        is_new: bool,
        now: SystemTime,
    ) {
        if let Phase::WaitingForApproval(context) = target {
            enqueue_approval(context, session, is_new);
            return;
        }
        if event.is_subagent_event()
            && !is_new
            && matches!(target, Phase::WaitingForInput | Phase::Idle | Phase::Ended)
        {
            // A subagent (or teammate) finishing its own work doesn't end the
            // main turn, nor the main session's pending requests.
            return;
        }

        match (&session.phase, &target) {
            (Phase::WaitingForInput | Phase::Idle, Phase::Processing | Phase::Compacting) => {
                // A turn only resumes on a user prompt, a fresh main-session
                // tool call or a manual compaction. Late PostToolUse /
                // SubagentStop / Task* events, background subagents and their
                // automatic compactions land after Stop and would otherwise
                // leave the session "processing" forever.
                if !is_new && !phase::resumes_turn(event) {
                    return;
                }
            }
            (Phase::WaitingForApproval(_), Phase::Processing | Phase::Compacting) => {
                // Parallel tool calls: another tool finishing doesn't answer
                // this request. The request's own PostToolUse /
                // PermissionDenied is handled by `resolve_approval` (which
                // promotes the next queued one).
                if event.event != "UserPromptSubmit" || event.is_subagent_event() {
                    return;
                }
                // A new prompt: the main session's requests are over (the
                // user typed past them); background agents' still wait.
                self.drop_main_approvals(session, target, now, false);
                return;
            }
            (Phase::WaitingForApproval(_), Phase::WaitingForInput)
                if event.event == "Notification" =>
            {
                // An idle notification says nothing about a request whose
                // hook is still waiting; its own events (or the hook going
                // away) resolve it.
                return;
            }
            (Phase::WaitingForApproval(_), _) => {
                // The main turn ended (Stop, StopFailure, SessionStart): its
                // own requests are over; background agents' requests outlive
                // it.
                let whole_session = matches!(event.event.as_str(), "StopFailure" | "SessionStart");
                self.drop_main_approvals(session, target, now, whole_session);
                return;
            }
            _ => {}
        }

        if phase::can_transition(&session.phase, &target) {
            session.phase = target;
        }
    }

    /// Turn boundaries: completion, review, errors, background work.
    /// `previous_phase` is `None` for a session first seen with this event.
    fn apply_lifecycle(
        &mut self,
        event: &HookEvent,
        previous_phase: Option<Phase>,
        session: &mut Session,
        now: SystemTime,
    ) {
        if event.is_subagent_event() {
            return;
        }
        match event.event.as_str() {
            "UserPromptSubmit" => {
                session.turn_started_at = Some(now);
                session.tool_tracker.end_main_turn();
                clear_failure(session);
                // What was already out when this turn began.
                session.agents_at_turn_start = session.known_waking_agents;
                session.wakeups_at_turn_start = session.known_wakeups;
                session.background_task_count = 0;
                session.background_agent_count = 0;
                // A background wait stands until this turn's Stop says what
                // is still running: Esc or a failed turn leaves the agents
                // running.
                session.scheduled_wakeup_count = 0;
                session.last_prompt_source = event.source.clone();
                session.last_prompt_was_user_authored = phase::is_user_authored_prompt(event);
                session.completion_check_since = None;
                // Loop, cron, system and task-notification turns don't mean
                // the user looked at the result; a prompt they typed does
                // (including one sent from VS Code, whose source is "sdk").
                if session.last_prompt_was_user_authored {
                    session.reviewed_at = Some(now);
                }
            }
            "Stop" => self.apply_stop(event, previous_phase, session, now),
            "StopFailure" => {
                session.stop_error =
                    Some(attention::humanized_stop_error(event.stop_error.as_deref()));
                session.stop_error_code = event.stop_error.clone();
                session.failed_at = Some(now);
                // The last_assistant_message of a StopFailure is the API
                // error text; the preview keeps the last real reply.
                session.background_task_count = 0;
                session.background_agent_count = 0;
                // The turn failed; the agents it waited on didn't (the wait
                // stands, and the registry can end it).
                session.subagent_state = SubagentState::new();
                session.tool_tracker.end_main_turn();
                self.settle_background_wait(session, now);
            }
            "SessionStart" => {
                if event.source.as_deref() == Some("clear") {
                    session.reviewed_at = Some(now);
                }
            }
            "Notification" if event.notification_type.as_deref() == Some("idle_prompt") => {
                // Claude has sat idle for a minute: whatever the Stop hooks
                // did, the turn is over.
                Self::confirm_pending_completion(session);
            }
            _ => {}
        }
    }

    fn apply_stop(
        &mut self,
        event: &HookEvent,
        previous_phase: Option<Phase>,
        session: &mut Session,
        now: SystemTime,
    ) {
        // A Stop ends a turn Claude worked on. A session first seen with its
        // Stop (the app started mid-turn) finished work too, and so did a Stop
        // ending a continuation a blocking Stop hook forced
        // (`stop_hook_active`), even with no event in between. So did the turn
        // an agent's result woke Claude for, even when no prompt or tool of it
        // reached us (the SDK can wake Claude without a UserPromptSubmit).
        let was_working = match previous_phase {
            None | Some(Phase::Processing | Phase::Compacting | Phase::WaitingForApproval(_)) => {
                true
            }
            Some(_) => {
                event.stop_hook_active == Some(true) || session.background_wait_since.is_some()
            }
        };
        // The hook's last_assistant_message is exact. The transcript's last
        // message is only a fallback for hooks that don't send it: it can be
        // a sync behind, still showing the preamble of an automatic
        // compaction earlier in this very turn.
        let reply = event
            .last_assistant_message
            .as_deref()
            .filter(|m| !m.is_empty());
        let is_context_resume = reply
            .or(session.conversation_info.last_message.as_deref())
            .is_some_and(|message| message.starts_with(CONTEXT_RESUME_PREFIX));
        if let Some(message) = reply {
            session.last_assistant_message = Some(message.to_owned());
        }
        let types = event.background_task_types.clone().unwrap_or_default();
        let awaited: Vec<String> = types
            .iter()
            .filter(|task_type| background::is_awaited(task_type))
            .cloned()
            .collect();
        session.background_task_count = event.background_task_count.unwrap_or(types.len() as u32);
        session.background_agent_count = awaited.len() as u32;
        // Agents and workflows still running will wake Claude when they
        // finish: until then the work isn't done (the session shows as
        // working, not ready for review).
        session.background_wait_since = (!awaited.is_empty()).then_some(now);
        session.background_agent_types = awaited;
        session.scheduled_wakeup_count = event.session_cron_count.unwrap_or(0);
        session.known_waking_agents = session.background_agent_count;
        session.known_wakeups = session.scheduled_wakeup_count;
        clear_failure(session);
        session.subagent_state = SubagentState::new();
        session.tool_tracker.end_main_turn();
        if was_working && !is_context_resume {
            // Claude Code still runs the other Stop hooks; a blocking one
            // (/goal) continues the turn. The registry going idle, or a quiet
            // moment, confirms the turn is over.
            session.completion_pending_since = Some(now);
            self.settle_pending_completion(session, now);
        }
        if let Some(account) = &session.account {
            self.rescan_after_stop.insert(account.clone(), now);
        }
        self.settle_background_wait(session, now);
    }

    // ---- status line ----

    fn process_status_line(
        &mut self,
        message: StatusLineMessage,
        ctx: &IngestContext,
        now: SystemTime,
    ) {
        self.note_sighting(
            &message.session_id,
            message.transcript_path.as_deref(),
            message.config_dir_env.as_deref(),
            now,
        );
        let id = message.session_id.clone();
        if self.recently_ended_at(&id, now).is_some() {
            return;
        }
        let at = message.received_at;
        let is_new = !self.sessions.contains_key(&id);
        // The status line runs only in live terminal sessions, so an unknown
        // session is real: track it (idle until a hook says otherwise).
        let mut session = match self.sessions.remove(&id) {
            Some(session) => session,
            None => self.create_session(&id, message.cwd.as_deref().unwrap_or(""), at, now),
        };
        if is_new {
            self.needs_task_reconstruction.insert(id.clone());
        }

        if let Some(cwd) = message.cwd.as_deref().filter(|c| !c.is_empty()) {
            session.current_cwd = cwd.to_owned();
        }
        if let Some(path) = message.transcript_path.as_deref().filter(|p| !p.is_empty()) {
            session.transcript_path = Some(path.to_owned());
        }
        if message.transcript_path.is_some()
            || message.config_dir_env.is_some()
            || session.account.is_none()
        {
            session.account = Some(
                ctx.account
                    .clone()
                    .or_else(|| message.account_id.clone())
                    .unwrap_or_else(|| {
                        self.config_dir_of(
                            message.transcript_path.as_deref(),
                            message.config_dir_env.as_deref(),
                        )
                    }),
            );
        }
        if let Some(env) = message.config_dir_env.as_deref().filter(|e| !e.is_empty()) {
            session.config_dir_env = Some(env.to_owned());
        }
        if let Some(percent) = message.context_used_percent.filter(|p| p.is_finite()) {
            session.context_used_percent = Some(percent.clamp(0.0, 100.0));
            session.status_line_updated_at = Some(at);
        }
        if let Some(size) = message.context_window_size.filter(|size| *size > 0) {
            session.context_window_size = Some(size);
        }
        if let Some(model) = message
            .model_display_name
            .as_deref()
            .or(message.model_id.as_deref())
        {
            session.model = Some(model.to_owned());
        }
        if let Some(cost) = message.cost_usd {
            session.cost_usd = Some(cost);
        }
        session.apply_name(message.session_name.as_deref(), false);
        self.apply_attribution(Some(ctx), &mut session, at);
        // Not `last_hook_event_at`: a status line says nothing about the
        // turn, so it must not hide a later registry correction.
        session.last_event_at = session.last_event_at.max(at);

        self.sessions.insert(id.clone(), session);
        if is_new {
            self.schedule_sync(&id, now);
        }
    }

    // ---- creating and ending ----

    fn create_session(
        &mut self,
        id: &SessionId,
        cwd: &str,
        at: SystemTime,
        now: SystemTime,
    ) -> Session {
        let mut session = Session::new(id.clone(), cwd, at);
        session.paths = Some(self.paths.clone());
        self.restore_from_review(&mut session, now);
        session
    }

    /// Forgets the session; late status line data can't revive it for
    /// [`ENDED_SESSION_MEMORY`].
    pub(super) fn remove_session(&mut self, id: &SessionId, now: SystemTime) {
        self.sessions.remove(id);
        self.recently_ended.retain(|_, ended| {
            now.duration_since(*ended).unwrap_or_default() < ENDED_SESSION_MEMORY
        });
        self.recently_ended.insert(id.clone(), now);
        self.needs_task_reconstruction.remove(id);
        self.sync_due.remove(id);
        self.release(Release::Session(id.clone()));
    }

    /// When the session ended, if that was within [`ENDED_SESSION_MEMORY`].
    pub(super) fn recently_ended_at(&self, id: &SessionId, now: SystemTime) -> Option<SystemTime> {
        self.recently_ended
            .get(id)
            .copied()
            .filter(|ended| now.duration_since(*ended).unwrap_or_default() < ENDED_SESSION_MEMORY)
    }

    /// One sighting per session per [`SIGHTING_INTERVAL`], and at once when
    /// the session's account changes (HookEventPipeline's SightingThrottle).
    fn note_sighting(
        &mut self,
        id: &SessionId,
        transcript_path: Option<&str>,
        config_dir_env: Option<&str>,
        now: SystemTime,
    ) {
        let has_transcript = transcript_path.is_some_and(|path| !path.is_empty());
        let env = config_dir_env.filter(|env| !env.is_empty());
        if !has_transcript && env.is_none() {
            return;
        }
        let config_dir = self.config_dir_of(transcript_path, config_dir_env);
        if let Some((last_dir, last_at)) = self.sightings.get(id) {
            if *last_dir == config_dir
                && now.duration_since(*last_at).unwrap_or_default() < SIGHTING_INTERVAL
            {
                return;
            }
        }
        self.sightings.insert(id.clone(), (config_dir.clone(), now));
        if self.sightings.len() > MAX_SIGHTINGS {
            self.sightings.retain(|_, (_, at)| {
                now.duration_since(*at).unwrap_or_default() < SIGHTING_INTERVAL
            });
        }
        self.effects.sightings.push(AccountSighting {
            config_dir,
            config_dir_env: env.map(str::to_owned),
            session_id: id.clone(),
            at: now,
        });
    }
}

/// The turn's failure is over (a new prompt, a Stop).
pub(super) fn clear_failure(session: &mut Session) {
    session.stop_error = None;
    session.stop_error_code = None;
    session.failed_at = None;
}

/// Sets or clears the needs-input reason for this event.
fn apply_needs_input(event: &HookEvent, session: &mut Session) {
    let at = event.received_at;
    match event.event.as_str() {
        "StopFailure" if !event.is_subagent_event() => {
            session.set_needs_input(
                Some(attention::failure_reason(event.stop_error.as_deref())),
                at,
            );
        }
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "Stop" | "PermissionRequest" => {
            // Main-session activity means whatever was asked has been
            // answered. Background subagents keep working after a failed turn,
            // so their events only settle a terminal permission prompt
            // (theirs), never an error or a dialog of the main session.
            if !event.is_subagent_event()
                || matches!(
                    session.needs_input_reason(),
                    Some(NeedsInputReason::Permission { .. })
                )
            {
                session.set_needs_input(None, at);
            }
        }
        "Notification" => {
            // Agent view announces background sessions ("<label> needs your
            // input", "<label> finished") on the session hosting it; they are
            // about another session.
            if phase::is_agent_view_announcement(event) {
                return;
            }
            match event.notification_type.as_deref() {
                Some("elicitation_dialog" | "elicitation_url_dialog") => session.set_needs_input(
                    Some(NeedsInputReason::Elicitation {
                        message: event.message.clone().unwrap_or_default(),
                    }),
                    at,
                ),
                Some("agent_needs_input" | "worker_permission_prompt") => session.set_needs_input(
                    Some(NeedsInputReason::Dialog {
                        detail: event
                            .message
                            .clone()
                            .or_else(|| event.title.clone())
                            .unwrap_or_default(),
                    }),
                    at,
                ),
                Some("permission_prompt") => {
                    // The terminal is asking. With a pending PermissionRequest
                    // the approval already shows; otherwise (no hook pipe) flag
                    // it.
                    if !phase::is_waiting_for_approval(&session.phase) {
                        let tool =
                            attention::tool_name_from_permission_prompt(event.message.as_deref())
                                .or_else(|| event.title.clone());
                        session.set_needs_input(Some(NeedsInputReason::Permission { tool }), at);
                    }
                }
                Some("elicitation_complete" | "elicitation_response") => {
                    if matches!(
                        session.needs_input_reason(),
                        Some(NeedsInputReason::Elicitation { .. })
                    ) {
                        session.set_needs_input(None, at);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Makes `context` the active approval, or queues it behind the active one.
pub(super) fn enqueue_approval(
    mut context: PermissionContext,
    session: &mut Session,
    is_new: bool,
) {
    if let Phase::WaitingForApproval(active) = &session.phase {
        if active.tool_use_id == context.tool_use_id {
            context.activated_at = active.activated_at;
            session.phase = Phase::WaitingForApproval(context);
            return;
        }
        if !session
            .queued_approvals
            .iter()
            .any(|queued| queued.tool_use_id == context.tool_use_id)
        {
            session.queued_approvals.push(context);
        }
        return;
    }
    let target = Phase::WaitingForApproval(context.clone());
    if phase::can_transition(&session.phase, &target) {
        // A main-session request always follows its PreToolUse, which put the
        // session in processing. A finished session is being asked by a
        // background agent: answering must not restart the turn.
        session.phase_after_approvals = match &session.phase {
            Phase::WaitingForInput | Phase::Idle if !is_new => session.phase.clone(),
            _ => Phase::Processing,
        };
        context.activated_at = Some(context.received_at);
        session.phase = Phase::WaitingForApproval(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_store_has_no_sessions() {
        let store = SessionStore::new();
        assert!(store.views().is_empty());
        assert!(store.view(&SessionId::from("x")).is_none());
        assert!(store.chat(&SessionId::from("x")).is_none());
    }
}
