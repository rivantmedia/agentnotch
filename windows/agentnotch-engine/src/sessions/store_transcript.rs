//! The store's transcript syncs, chat and interrupts (SessionStore.swift's
//! File Update, Chat retention, Interrupt and History sections, and
//! ConversationParser's per-session state).
//!
//! The syncs are jobs: a hook event only asks for one (`schedule_sync`, 100
//! ms debounced), `tick` turns the due ones into `Job::SyncTranscript` (one
//! per session in flight; a request that comes meanwhile is asked again when
//! the job is back), and the runtime hands each result back as
//! `SessionInput::TranscriptSynced`. The job keeps no state, so everything
//! stateful is here: the summary fold, the task reconstruction of a session
//! first seen mid-flight, the agents whose own transcripts are followed (each
//! its own job, settled once finished or after 10 minutes without change,
//! at most 64) and the chat items.
//!
//! Chat calls for the panel: `open_chat` (marks reviewed, asks for the
//! newest page), `chat_loaded` (the page came back; `JobResult::Chat` has no
//! `SessionInput`), `chat_more`, `close_chat`, `chat_image` and
//! `take_chat_updates` (a reset when a chat opens or pages, then patches of
//! the changed items).

use crate::model::{
    ChatBody, ChatHistory, ChatPage, ChatUpdate, Phase, SessionId, ToolOutput, ToolResultView,
};
use crate::persist::review::MAX_MESSAGE_LENGTH;
use crate::runtime_types::{Job, Release, SessionEffects, TranscriptDelta, TranscriptEntry};
use crate::sessions::chat::{self, SubagentTranscript};
use crate::sessions::interrupt::InterruptWatch;
use crate::sessions::locator::TranscriptLocator;
use crate::sessions::session::{
    AgentTrack, Session, SessionTitleSource, SettledAgents, SubagentState, ToolTracker,
};
use crate::sessions::store::{clear_failure, SessionStore};
use crate::sessions::summary::{self, TranscriptTurn};
use crate::sessions::tasks::TaskList;
use crate::sessions::tool_input;
use crate::sessions::transcript::is_agent_transcript;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Hook events that change the transcript are read after this quiet moment,
/// so a burst costs one read.
pub const SYNC_DEBOUNCE: Duration = Duration::from_millis(100);

/// Agents followed per session (the newest; older ones are settled).
pub const MAX_TRACKED_AGENTS: usize = 64;

/// A running agent whose transcript hasn't changed for this long is settled.
pub const AGENT_IDLE_SETTLE: Duration = Duration::from_secs(10 * 60);

/// A finished agent whose transcript never shows up is given up on after
/// this many syncs.
pub const MISSING_AGENT_CHECKS: u32 = 3;

/// What the panel was last sent of one open chat.
#[derive(Debug, Clone, Default)]
pub(super) struct ChatWatch {
    /// The history as last sent; none until the opening reset goes out (and
    /// again after paging, which is sent as a reset).
    pub(super) last: Option<ChatHistory>,
    pub(super) last_working: Option<String>,
    pub(super) last_loading: bool,
    /// A page is being read.
    pub(super) loading: bool,
}

impl SessionStore {
    // ---- scheduling ----

    /// Asks for the session's transcript to be read again soon (debounced).
    pub(super) fn schedule_sync(&mut self, id: &SessionId, now: SystemTime) {
        if !self.reads_transcripts {
            return;
        }
        self.sync_due
            .entry(id.clone())
            .or_insert(now + SYNC_DEBOUNCE);
    }

    /// The reads whose debounce is over become jobs.
    pub(super) fn start_due_syncs(&mut self, now: SystemTime) {
        let due: Vec<SessionId> = self
            .sync_due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.sync_due.remove(&id);
            if self.sessions.contains_key(&id) {
                self.start_sync(&id);
            }
        }
    }

    fn start_sync(&mut self, id: &SessionId) {
        if self.sync_in_flight.contains(id) {
            self.sync_again.insert(id.clone());
            return;
        }
        let Some(path) = self.resolve_transcript_path(id) else {
            return;
        };
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        self.effects.jobs.push(Job::SyncTranscript {
            session: id.clone(),
            path: PathBuf::from(path),
            cursor: session.cursor,
        });
        self.sync_in_flight.insert(id.clone());
    }

    /// The session's transcript: the hook's path while it exists, else the
    /// one located from the cwd and account (its own folder first, then
    /// `~\.claude`; a history shared between them, Claude Parallel Profiles,
    /// is read once).
    pub(super) fn resolve_transcript_path(&mut self, id: &SessionId) -> Option<String> {
        let session = self.sessions.get(id)?;
        let locator = TranscriptLocator::new(&self.paths, self.files.as_ref());
        let known = session.transcript_path.clone();
        if let Some(path) = known.as_deref().filter(|path| !path.is_empty()) {
            if self.files.identity(Path::new(path)).is_ok() {
                return Some(path.to_owned());
            }
        }
        let default_dir = self.paths.join(self.paths.home(), ".claude");
        let config_dir = session
            .account
            .as_ref()
            .map_or_else(|| default_dir.clone(), |account| account.0.clone());
        let cwd = if session.cwd.is_empty() {
            session.current_cwd.as_str()
        } else {
            session.cwd.as_str()
        };
        let located = locator.transcript_path_in(
            id.as_str(),
            Some(cwd),
            &[config_dir, default_dir],
            known.as_deref(),
        )?;
        let changed = known
            .as_deref()
            .is_none_or(|known| !locator.is_same_file(known, &located));
        if changed {
            if let Some(session) = self.sessions.get_mut(id) {
                session.transcript_path = Some(located.clone());
            }
        }
        Some(located)
    }

    // ---- deltas ----

    /// New lines of a transcript were read: the session's own, or one of its
    /// agents'.
    pub(super) fn apply_transcript_synced(&mut self, delta: TranscriptDelta, now: SystemTime) {
        let id = delta.session.clone();
        let Some(mut session) = self.sessions.remove(&id) else {
            self.sync_in_flight.remove(&id);
            return;
        };
        if is_agent_transcript(&delta.path) {
            apply_agent_delta(&mut session, delta, now);
            self.sessions.insert(id, session);
            return;
        }
        self.sync_in_flight.remove(&id);
        let previous_offset = session.cursor.offset;
        session.cursor = delta.cursor;
        let reset = delta.reset;
        if reset {
            // Rewritten: everything is read again from the top.
            session.fold = summary::TranscriptSummary::new();
            session.agents.clear();
            session.settled_agents = SettledAgents::new();
            session.tool_tracker = ToolTracker::new();
            session.subagent_state = SubagentState::new();
            session.tasks.reset();
            session.chat.clear_history(now);
            if self.needs_task_reconstruction.contains(&id) {
                self.reconstruction.insert(id.clone(), TaskList::new());
            }
        }
        let reconstructing = self.needs_task_reconstruction.contains(&id);
        for entry in &delta.entries {
            session.fold.apply(entry);
            if reconstructing {
                fold_task(self.reconstruction.entry(id.clone()).or_default(), entry);
            }
        }
        if session.transcript_path.is_none() {
            session.transcript_path = Some(delta.path.to_string_lossy().into_owned());
        }
        apply_conversation_info(&mut session);

        // Read in full, or no more to read right now (a half-written last
        // line waits for its newline).
        let progressed = reset || delta.cursor.offset > previous_offset;
        let caught_up = delta.cursor.offset >= delta.cursor.size || !progressed;
        // An empty or missing file says nothing yet: the checks wait for it.
        if caught_up && delta.cursor.size > 0 {
            let turn = session.fold.turn().clone();
            if reconstructing {
                // History from the transcript, then everything hooks
                // reported since.
                if let Some(rebuilt) = self.reconstruction.remove(&id) {
                    session.tasks = session.tasks.merged_into_reconstructed(&rebuilt);
                }
                self.needs_task_reconstruction.remove(&id);
                review_restored_state(&mut session, &turn, now);
            }
            infer_completion(&mut session, &turn);
        }

        let outcome = session.chat.apply_entries(&delta.entries, now, reset);
        if outcome.cleared {
            // /clear: the conversation starts over.
            session.tool_tracker = ToolTracker::new();
            session.subagent_state = SubagentState::new();
            session.tasks.reset();
        }
        // Tools shown as running whose result the transcript now has: they
        // finished (possibly approved in the terminal).
        for tool_use_id in &outcome.completed_tools {
            session.tool_tracker.complete_tool(tool_use_id);
            if session.pending_permission(tool_use_id).is_some() {
                self.release(Release::Request {
                    session: id.clone(),
                    tool_use_id: tool_use_id.clone(),
                });
            }
            Self::resolve_approval(&mut session, tool_use_id, now);
        }
        for entry in &delta.entries {
            if let TranscriptEntry::ToolOutput(output) = entry {
                follow_agent(&mut session, output, now);
            }
        }
        self.sessions.insert(id.clone(), session);

        self.refresh_agents(&id, now);
        if progressed && delta.cursor.offset < delta.cursor.size {
            // More lines than one read takes.
            self.start_sync(&id);
        } else if self.sync_again.remove(&id) {
            self.schedule_sync(&id, now);
        }
    }

    /// Reads what every followed agent wrote since last time (a job each;
    /// a finished agent is read once) and settles the ones that are done.
    fn refresh_agents(&mut self, id: &SessionId, now: SystemTime) {
        let Some(mut session) = self.sessions.remove(id) else {
            return;
        };
        let main = session.transcript_path.clone().unwrap_or_default();
        let locator = TranscriptLocator::new(&self.paths, self.files.as_ref());
        let tool_use_ids: Vec<String> = session.agents.keys().cloned().collect();
        for tool_use_id in tool_use_ids {
            let Some(track) = session.agents.get_mut(&tool_use_id) else {
                continue;
            };
            if track.in_flight {
                continue;
            }
            if track.path.is_none() {
                let path = locator.subagent_transcript_path(&main, &track.agent_id);
                if self.files.identity(Path::new(&path)).is_ok() {
                    track.path = Some(PathBuf::from(path));
                } else {
                    track.missing_checks += 1;
                }
            }
            match &track.path {
                Some(path) => {
                    self.effects.jobs.push(Job::SyncTranscript {
                        session: id.clone(),
                        path: path.clone(),
                        cursor: track.cursor,
                    });
                    track.in_flight = true;
                }
                None => {
                    if is_settled(track, now) {
                        settle(&mut session, &tool_use_id);
                    }
                }
            }
        }
        trim_agents(&mut session);
        self.sessions.insert(id.clone(), session);
    }

    /// The transcript watcher saw Esc: the main turn stopped (HS§5.8). Ignored
    /// when a newer turn started since (the interrupt, seen late, is about
    /// the old one); the background agents a wait is on keep running.
    pub(super) fn apply_interrupt(&mut self, id: &SessionId, at: SystemTime, now: SystemTime) {
        let Some(mut session) = self.sessions.remove(id) else {
            return;
        };
        if session.turn_started_at.is_some_and(|started| started > at) {
            self.sessions.insert(id.clone(), session);
            return;
        }
        session.watching_turn = false;
        session.subagent_state = SubagentState::new();
        session.tool_tracker.end_main_turn();
        session.completion_pending_since = None;
        if !session.has_failed_turn() {
            session.set_needs_input(None, now);
        }
        // Calls still running were interrupted; those waiting for approval are
        // settled with the requests below.
        session.chat.interrupt_running();
        if crate::sessions::phase::is_waiting_for_approval(&session.phase) {
            self.drop_main_approvals(&mut session, Phase::Idle, now, false);
        } else if crate::sessions::phase::can_transition(&session.phase, &Phase::Idle) {
            session.phase = Phase::Idle;
        }
        // Esc stops the turn, not the agents a background wait is on.
        self.settle_background_wait(&mut session, now);
        self.sessions.insert(id.clone(), session);
    }

    /// The transcripts to watch for an interrupt: those of the sessions
    /// whose main turn runs (a processing event with a transcript path since
    /// the last Stop). The runtime gives them to the
    /// [`InterruptWatcher`](crate::sessions::interrupt::InterruptWatcher).
    pub fn interrupt_watches(&self) -> Vec<InterruptWatch> {
        self.sessions
            .values()
            .filter(|session| session.watching_turn)
            .filter_map(|session| {
                let path = session
                    .transcript_path
                    .as_deref()
                    .filter(|path| !path.is_empty())?;
                Some(InterruptWatch {
                    session: session.id.clone(),
                    path: PathBuf::from(path),
                })
            })
            .collect()
    }

    // ---- chat ----

    /// The panel opened a session's chat: the session counts as reviewed, its
    /// history is kept whole from now on and the newest page is read. The
    /// opening reset is in `take_chat_updates`.
    pub fn open_chat(&mut self, id: &SessionId, now: SystemTime) -> SessionEffects {
        self.run(now, false, |store| {
            if !store.sessions.contains_key(id) {
                return;
            }
            store.mark_reviewed(id, now);
            for released in store.open_chats.touch(id) {
                if let Some(session) = store.sessions.get_mut(&released) {
                    session.chat.set_open(false);
                }
                store.chat_watches.remove(&released);
            }
            if let Some(session) = store.sessions.get_mut(id) {
                session.chat.set_open(true);
            }
            let mut watch = ChatWatch::default();
            let path = store
                .reads_transcripts
                .then(|| store.resolve_transcript_path(id))
                .flatten();
            if let Some(path) = path {
                watch.loading = true;
                store.effects.jobs.push(Job::LoadChat {
                    session: id.clone(),
                    path: PathBuf::from(path),
                    before: None,
                });
            }
            store.chat_watches.insert(id.clone(), watch);
        })
    }

    /// The page a `LoadChat` job read: merged into the open chat. The panel
    /// gets what changed (a reset when it was an earlier page).
    pub fn chat_loaded(&mut self, page: ChatPage, now: SystemTime) -> SessionEffects {
        self.run(now, false, |store| {
            let id = page.session.clone();
            if !store.open_chats.is_open(&id) {
                return;
            }
            let Some(mut session) = store.sessions.remove(&id) else {
                return;
            };
            let earlier = page.before.is_some();
            let read = page.error.is_none();
            if read {
                session.chat.merge_page(&page, now);
                follow_running_agents(&mut session, &page, now);
                if !earlier {
                    store.open_chats.mark_loaded(&id);
                }
            }
            store.sessions.insert(id.clone(), session);
            if let Some(watch) = store.chat_watches.get_mut(&id) {
                watch.loading = false;
                if earlier && read {
                    // Paging is sent as a reset.
                    watch.last = None;
                }
            }
            store.refresh_agents(&id, now);
        })
    }

    /// The next page of items before `before_id` is wanted (the panel
    /// scrolled to the top): the job that reads it.
    pub fn chat_more(&mut self, id: &SessionId, before_id: &str) -> Vec<Job> {
        if !self.open_chats.is_open(id) {
            return Vec::new();
        }
        let Some(path) = self.resolve_transcript_path(id) else {
            return Vec::new();
        };
        if let Some(watch) = self.chat_watches.get_mut(id) {
            watch.loading = true;
        }
        vec![Job::LoadChat {
            session: id.clone(),
            path: PathBuf::from(path),
            before: Some(before_id.to_owned()),
        }]
    }

    /// The chat was closed: its history is released down to the retention
    /// rule.
    pub fn close_chat(&mut self, id: &SessionId) {
        self.open_chats.close(id);
        self.chat_watches.remove(id);
        if let Some(session) = self.sessions.get_mut(id) {
            session.chat.set_open(false);
        }
    }

    /// One image of a session's chat as a data URL (at most 2 MiB).
    pub fn chat_image(&self, id: &SessionId, image_id: &str) -> Option<String> {
        self.sessions.get(id)?.chat.image_data_url(image_id)
    }

    /// Whether any chat is open (or just ended): the runtime then asks for
    /// [`SessionStore::take_chat_updates`] after every input.
    pub fn has_open_chats(&self) -> bool {
        !self.chat_watches.is_empty() || !self.ended_chats.is_empty()
    }

    /// What the open chats gained since the last call: a reset for a chat
    /// just opened or paged, else only the items that are new or changed,
    /// the ids removed and the order; a last update with `ended` for a
    /// session that went away.
    pub fn take_chat_updates(&mut self) -> Vec<ChatUpdate> {
        let mut updates = Vec::new();
        for (id, last) in std::mem::take(&mut self.ended_chats) {
            updates.push(chat::chat_update(
                id.as_str(),
                Some(&last),
                &last,
                None,
                true,
                false,
            ));
        }
        for (id, watch) in &mut self.chat_watches {
            let Some(session) = self.sessions.get(id) else {
                continue;
            };
            let next = session.chat.history();
            let working = working_label(session);
            let Some(previous) = watch.last.as_ref() else {
                updates.push(chat::chat_update(
                    id.as_str(),
                    None,
                    &next,
                    working.clone(),
                    false,
                    watch.loading,
                ));
                watch.last = Some(next);
                watch.last_working = working;
                watch.last_loading = watch.loading;
                continue;
            };
            if previous.revision == next.revision
                && watch.last_working == working
                && watch.last_loading == watch.loading
            {
                continue;
            }
            let update = chat::chat_update(
                id.as_str(),
                Some(previous),
                &next,
                working.clone(),
                false,
                watch.loading,
            );
            let previous_order: Vec<&str> =
                previous.items.iter().map(|item| item.id.as_str()).collect();
            let meaningful = !update.items.is_empty()
                || !update.removed.is_empty()
                || update.order.iter().map(String::as_str).ne(previous_order)
                || update.has_earlier != previous.has_earlier
                || watch.last_working != working
                || watch.last_loading != watch.loading;
            if meaningful {
                updates.push(update);
            }
            watch.last = Some(next);
            watch.last_working = working;
            watch.last_loading = watch.loading;
        }
        updates
    }
}

// ---- helpers ----

/// What the session is doing, for the chat's "working" line.
fn working_label(session: &Session) -> Option<String> {
    match session.phase {
        Phase::Compacting => Some("Compacting context…".to_owned()),
        Phase::Processing => Some(
            session
                .tasks
                .active_item()
                .map(|task| task.active_label().to_owned())
                .or_else(|| {
                    session
                        .tool_tracker
                        .newest()
                        .map(|tool| format!("{}…", tool.name))
                })
                .unwrap_or_else(|| "Thinking…".to_owned()),
        ),
        _ => None,
    }
}

/// Transcript-derived fields: conversation info, title fallback, context
/// estimate.
fn apply_conversation_info(session: &mut Session) {
    let info = session.fold.info();
    session.apply_title(
        info.title.as_deref().or(info.summary.as_deref()),
        SessionTitleSource::Transcript,
    );
    if session.model.is_none() {
        session.model = info.last_model.clone();
    }
    // The status line is exact; estimate only for sessions without one.
    if session.status_line_updated_at.is_none() {
        if let Some(tokens) = info.last_context_tokens.filter(|tokens| *tokens > 0) {
            session.context_used_percent = Some(summary::context::percent(
                tokens,
                session.context_window_size,
                &[session.model.as_deref(), info.last_model.as_deref()],
            ));
        }
    }
    session.conversation_info = info;
}

/// What the task list sees of one transcript entry.
fn fold_task(list: &mut TaskList, entry: &TranscriptEntry) {
    match entry {
        TranscriptEntry::ToolUse { id, name, input } => {
            list.apply_transcript_tool_use(id, name, input)
        }
        TranscriptEntry::ToolResult {
            tool_use_id,
            status,
            task_id,
        } => {
            list.apply_transcript_tool_result(tool_use_id, status != "success", task_id.as_deref())
        }
        TranscriptEntry::Clear => list.reset(),
        _ => {}
    }
}

/// A review state restored from disk predates whatever happened while the
/// app wasn't watching: a prompt typed after the recorded completion means
/// the user already saw that result (and moved on), and one typed after a
/// recorded failure means it was dealt with.
fn review_restored_state(session: &mut Session, turn: &TranscriptTurn, now: SystemTime) {
    if let (Some(failed_at), true) = (session.failed_at, session.has_failed_turn()) {
        let moved_on = [turn.human_prompt_at, turn.reply_at]
            .into_iter()
            .flatten()
            .any(|at| at > failed_at);
        if moved_on {
            clear_failure(session);
            session.set_needs_input(None, now);
        }
    }
    if let (Some(completed_at), Some(prompt_at)) = (session.completed_at, turn.human_prompt_at) {
        if prompt_at > completed_at
            && session
                .reviewed_at
                .is_none_or(|reviewed| reviewed < prompt_at)
        {
            session.reviewed_at = Some(prompt_at);
        }
    }
}

/// A turn that ended where no hook said so: a session without hooks whose
/// registry went idle, or one that finished while the app wasn't running
/// (its registry entry idle at discovery, its reply newer than the last
/// time the app was alive). Only a transcript that ends with Claude's reply
/// counts: an interrupt, a prompt or a tool call after it doesn't.
fn infer_completion(session: &mut Session, turn: &TranscriptTurn) {
    let Some(since) = session.completion_check_since.take() else {
        return;
    };
    // Only Claude Code's own registry saying "idle" tells a finished turn
    // from a reply written mid-turn (text before the next tool call).
    if !matches!(session.registry_status.as_deref(), Some("idle" | "shell")) {
        return;
    }
    if !matches!(session.phase, Phase::Idle | Phase::WaitingForInput)
        || session.completion_pending_since.is_some()
    {
        return;
    }
    let bound = [Some(since), session.completed_at, session.reviewed_at]
        .into_iter()
        .flatten()
        .max();
    let Some((text, at)) = turn.finished_turn(bound) else {
        return;
    };
    session.completed_at = Some(at);
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        session.last_assistant_message = Some(text.chars().take(MAX_MESSAGE_LENGTH).collect());
    }
}

// ---- agents ----

/// A finished Agent/Task call whose result names its agent: the agent's own
/// transcript is followed from here.
fn follow_agent(session: &mut Session, output: &ToolOutput, now: SystemTime) {
    let Some(raw) = output.raw.as_ref().and_then(Value::as_object) else {
        return;
    };
    let Some(agent_id) = raw
        .get("agentId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let name =
        output
            .tool_name
            .clone()
            .or_else(|| match &session.chat.item(&output.tool_use_id)?.body {
                ChatBody::Tool { name, .. } => Some(name.clone()),
                _ => None,
            });
    if name.is_some_and(|name| !tool_input::is_subagent_container(&name)) {
        return;
    }
    let tool_use_id = &output.tool_use_id;
    if session.agents.contains_key(tool_use_id) || session.settled_agents.contains(tool_use_id) {
        return;
    }
    let finished = raw.get("status").and_then(Value::as_str) == Some("completed");
    session.agents.insert(
        tool_use_id.clone(),
        AgentTrack::new(agent_id, finished, now),
    );
}

/// Agents the loaded page shows still running (and nobody follows): they
/// are followed from here. Finished ones came with their tool lists.
fn follow_running_agents(session: &mut Session, page: &ChatPage, now: SystemTime) {
    for item in &page.items {
        let ChatBody::Tool {
            name,
            result: Some(ToolResultView::Task {
                agent_id, status, ..
            }),
            ..
        } = &item.body
        else {
            continue;
        };
        if !tool_input::is_subagent_container(name)
            || agent_id.is_empty()
            || status == "completed"
            || session.agents.contains_key(&item.id)
            || session.settled_agents.contains(&item.id)
        {
            continue;
        }
        session.agents.insert(
            item.id.clone(),
            AgentTrack::new(agent_id.clone(), false, now),
        );
    }
}

/// An agent's transcript was read: its tool list goes under its Agent call,
/// and the agent is settled when it can write no more.
fn apply_agent_delta(session: &mut Session, delta: TranscriptDelta, now: SystemTime) {
    let Some(tool_use_id) = session
        .agents
        .iter()
        .find(|(_, track)| track.path.as_deref() == Some(delta.path.as_path()))
        .map(|(id, _)| id.clone())
    else {
        return;
    };
    let Some(track) = session.agents.get_mut(&tool_use_id) else {
        return;
    };
    track.in_flight = false;
    track.cursor = delta.cursor;
    let transcript = track
        .transcript
        .get_or_insert_with(SubagentTranscript::default);
    if delta.reset {
        transcript.reset();
    }
    if transcript.fold(&delta.entries) {
        track.last_change_at = Some(now);
        let tools = transcript.tools().to_vec();
        session.chat.apply_subagent_tools(&tool_use_id, &tools);
    }
    // A transcript longer than one read is read on by the next sync.
    let more = delta.cursor.offset < delta.cursor.size && delta.cursor.offset > 0;
    if !more && is_settled(&session.agents[&tool_use_id], now) {
        settle(session, &tool_use_id);
    }
}

/// A finished agent wrote everything it will once its transcript was read
/// (or never showed one); a running one is given up on after 10 minutes of
/// silence.
fn is_settled(track: &AgentTrack, now: SystemTime) -> bool {
    if track.is_finished {
        return track.transcript.is_some() || track.missing_checks >= MISSING_AGENT_CHECKS;
    }
    let last_change = track.last_change_at.unwrap_or(track.added_at);
    now.duration_since(last_change).unwrap_or_default() > AGENT_IDLE_SETTLE
}

fn settle(session: &mut Session, tool_use_id: &str) {
    session.agents.remove(tool_use_id);
    session.settled_agents.settle(tool_use_id);
}

/// At most [`MAX_TRACKED_AGENTS`] are followed: the oldest are settled.
fn trim_agents(session: &mut Session) {
    if session.agents.len() <= MAX_TRACKED_AGENTS {
        return;
    }
    let mut by_age: Vec<(SystemTime, String)> = session
        .agents
        .iter()
        .map(|(id, track)| (track.added_at, id.clone()))
        .collect();
    by_age.sort();
    let extra = session.agents.len() - MAX_TRACKED_AGENTS;
    for (_, tool_use_id) in by_age.into_iter().take(extra) {
        settle(session, &tool_use_id);
    }
}
