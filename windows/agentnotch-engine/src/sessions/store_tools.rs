//! The store's approvals, tool tracking and subagent tracking (the Approvals,
//! Tool Tracking and Subagent sections of SessionStore.swift): the queue of
//! pending PermissionRequests, what each answer does to it, and the calls
//! and subagent tasks hook events show.

use crate::model::{Answer, HookEvent, PermissionContext, Phase, SessionId};
use crate::runtime_types::Release;
use crate::sessions::chat::{self, ERROR, INTERRUPTED, RUNNING, SUCCESS, WAITING_FOR_APPROVAL};
use crate::sessions::phase;
use crate::sessions::session::{Session, SubagentToolCall, ToolPhase};
use crate::sessions::store::SessionStore;
use crate::sessions::tool_input;
use std::time::SystemTime;

impl SessionStore {
    // ---- approvals ----

    /// A PermissionRequest arrived: its call waits for the user.
    pub(super) fn mark_waiting_for_approval(&mut self, event: &HookEvent, session: &mut Session) {
        if let Some(id) = &event.tool_use_id {
            session
                .chat
                .set_tool_status(id, WAITING_FOR_APPROVAL, false);
            session
                .tool_tracker
                .set_phase(ToolPhase::PendingApproval, id);
        }
    }

    /// Removes an approval that was answered; the next queued one becomes
    /// active, else the session returns to the phase it had before (see
    /// `Session::phase_after_approvals`).
    pub(super) fn resolve_approval(session: &mut Session, tool_use_id: &str, at: SystemTime) {
        session
            .queued_approvals
            .retain(|queued| queued.tool_use_id != tool_use_id);
        match &session.phase {
            Phase::WaitingForApproval(active) if active.tool_use_id == tool_use_id => {}
            _ => return,
        }
        let after = session.phase_after_approvals.clone();
        Self::activate_next_approval(session, after, at);
    }

    /// Shows the next queued request (clicks right after the swap are for the
    /// old one: the UI ignores them for a moment after `activated_at`), else
    /// moves to `phase`.
    fn activate_next_approval(session: &mut Session, phase: Phase, at: SystemTime) {
        if !session.queued_approvals.is_empty() {
            let mut next = session.queued_approvals.remove(0);
            next.activated_at = Some(at);
            session.phase = Phase::WaitingForApproval(next);
        } else if phase::can_transition(&session.phase, &phase) {
            session.phase = phase;
        }
    }

    /// The main turn is over (Stop, StopFailure, a new prompt, an interrupt,
    /// the registry going idle): the main session's requests go, their held
    /// pipes are closed (so a hook never waits on a request nobody sees), and
    /// background agents' requests stay. With none left the session moves to
    /// `then`; else the agents' requests stay shown and the session goes to
    /// `then` once they are answered.
    ///
    /// `whole_session` is a StopFailure or SessionStart: with no agent
    /// request kept, everything the session holds is released
    /// (`Release::Session`), else only the main agent's (`Release::MainAgent`).
    pub(super) fn drop_main_approvals(
        &mut self,
        session: &mut Session,
        then: Phase,
        now: SystemTime,
        whole_session: bool,
    ) {
        let (kept, dropped): (Vec<PermissionContext>, Vec<PermissionContext>) = session
            .pending_permissions()
            .into_iter()
            .cloned()
            .partition(PermissionContext::is_from_subagent);
        if !dropped.is_empty() {
            self.release(if whole_session && kept.is_empty() {
                Release::Session(session.id.clone())
            } else {
                Release::MainAgent(session.id.clone())
            });
        }
        for context in &dropped {
            session
                .chat
                .set_tool_status(&context.tool_use_id, INTERRUPTED, true);
        }

        let Some(mut first) = kept.first().cloned() else {
            session.queued_approvals.clear();
            if phase::can_transition(&session.phase, &then)
                || phase::is_waiting_for_approval(&session.phase)
            {
                session.phase = then;
            }
            return;
        };
        let still_active = session
            .active_permission()
            .is_some_and(|active| active.tool_use_id == first.tool_use_id);
        if !still_active {
            first.activated_at = Some(now);
        }
        session.queued_approvals = kept[1..].to_vec();
        session.phase = Phase::WaitingForApproval(first);
        session.phase_after_approvals = match then {
            Phase::Processing | Phase::Compacting => then,
            Phase::Idle => Phase::Idle,
            _ => Phase::WaitingForInput,
        };
    }

    /// A subagent stopped: whatever it still asked is stale, and the pipe
    /// holding it is closed.
    pub(super) fn drop_agent_approvals(
        &mut self,
        session: &mut Session,
        agent_id: &str,
        now: SystemTime,
    ) {
        let ids: Vec<String> = session
            .pending_permissions()
            .into_iter()
            .filter(|context| context.agent_id.as_deref() == Some(agent_id))
            .map(|context| context.tool_use_id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        self.release(Release::Agent {
            session: session.id.clone(),
            agent_id: agent_id.to_owned(),
        });
        for id in ids {
            session.chat.set_tool_status(&id, INTERRUPTED, true);
            session.tool_tracker.complete_tool(&id);
            Self::resolve_approval(session, &id, now);
        }
    }

    /// The user's answer went out (allow, deny, an answer to a question, a
    /// plan decision): the call runs, or Claude goes on with the denial.
    pub(super) fn permission_resolved(
        &mut self,
        id: &SessionId,
        tool_use_id: &str,
        answer: &Answer,
        now: SystemTime,
    ) {
        let Some(mut session) = self.sessions.remove(id) else {
            return;
        };
        let approved = !matches!(answer, Answer::Deny { .. } | Answer::KeepPlanning);
        // A late outcome must not flip a tool that already finished (its
        // PostToolUse may have been processed first) back to running.
        session
            .chat
            .set_tool_status(tool_use_id, if approved { RUNNING } else { ERROR }, true);
        if approved {
            session
                .tool_tracker
                .set_phase(ToolPhase::Running, tool_use_id);
        } else {
            session.tool_tracker.complete_tool(tool_use_id);
        }
        Self::resolve_approval(&mut session, tool_use_id, now);
        // Settles any prompt; a failed turn's error stays (the request can be
        // a background agent's, after StopFailure).
        if !session.has_failed_turn() {
            session.set_needs_input(None, now);
        }
        session.last_event_at = now;
        self.sessions.insert(id.clone(), session);
    }

    /// The hook holding the request went away (the terminal answered first,
    /// or it timed out): the app can no longer answer it. Claude either
    /// continues or is still asking in the terminal; the registry and the
    /// permission_prompt notification tell which.
    pub(super) fn permission_socket_failed(
        &mut self,
        id: &SessionId,
        tool_use_id: &str,
        now: SystemTime,
    ) {
        if let Some(session) = self.sessions.get_mut(id) {
            Self::resolve_approval(session, tool_use_id, now);
        }
    }

    // ---- tools ----

    pub(super) fn track_tools(&mut self, event: &HookEvent, session: &mut Session, at: SystemTime) {
        match event.event.as_str() {
            "PreToolUse" => {
                let (Some(tool_use_id), Some(tool)) = (&event.tool_use_id, &event.tool) else {
                    return;
                };
                session.tool_tracker.start_tool(
                    tool_use_id,
                    tool,
                    if event.is_subagent_event() {
                        event.agent_id.as_deref()
                    } else {
                        None
                    },
                    at,
                );
                // A subagent's own calls show under their Agent item, not as
                // top-level placeholders.
                let is_subagent_tool = event.is_subagent_event()
                    || (session.subagent_state.has_active_subagent()
                        && !tool_input::is_subagent_container(tool));
                if is_subagent_tool {
                    return;
                }
                let input = event
                    .tool_input
                    .as_ref()
                    .map(tool_input::flatten)
                    .unwrap_or_default();
                session.chat.place_tool(tool_use_id, tool, &input, at);
            }
            "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
                let Some(tool_use_id) = &event.tool_use_id else {
                    return;
                };
                let status = match event.event.as_str() {
                    "PostToolUse" => SUCCESS,
                    "PostToolUseFailure" if event.is_interrupt == Some(true) => INTERRUPTED,
                    _ => ERROR,
                };
                // The tool completed (possibly approved in the terminal).
                session.tool_tracker.complete_tool(tool_use_id);
                session.chat.set_tool_status(tool_use_id, status, true);
                // A request for this call was answered in the terminal (or
                // denied by a rule): its held pipe, if any, closes.
                if session.pending_permission(tool_use_id).is_some() {
                    self.release(Release::Request {
                        session: session.id.clone(),
                        tool_use_id: tool_use_id.clone(),
                    });
                }
                Self::resolve_approval(session, tool_use_id, at);
            }
            _ => {}
        }
    }

    // ---- subagents ----

    pub(super) fn track_subagents(
        &mut self,
        event: &HookEvent,
        session: &mut Session,
        at: SystemTime,
    ) {
        match event.event.as_str() {
            "PreToolUse" => {
                let (Some(tool), Some(tool_use_id)) = (&event.tool, &event.tool_use_id) else {
                    return;
                };
                if tool_input::is_subagent_container(tool) && !event.is_subagent_event() {
                    let description = event
                        .tool_input
                        .as_ref()
                        .and_then(|input| input.get("description"))
                        .and_then(|value| value.as_str());
                    session
                        .subagent_state
                        .start_task(tool_use_id, description, at);
                } else if session.subagent_state.has_active_subagent() {
                    // A subagent's inner tool is starting: show it under its
                    // Task/Agent live (not only after the Agent completes).
                    session.subagent_state.add_subagent_tool(SubagentToolCall {
                        id: tool_use_id.clone(),
                        name: tool.clone(),
                        input: event
                            .tool_input
                            .as_ref()
                            .map(tool_input::flatten)
                            .unwrap_or_default(),
                        status: RUNNING.to_owned(),
                        timestamp: at,
                    });
                    sync_subagent_tools(session);
                }
            }
            "PostToolUse" => {
                let Some(tool_use_id) = &event.tool_use_id else {
                    return;
                };
                let is_agent_call = event
                    .tool
                    .as_deref()
                    .is_some_and(tool_input::is_subagent_container);
                if is_agent_call && !event.is_subagent_event() {
                    // The Agent tool returned: the subagent has finished. Stop
                    // tracking so later tools of the parent turn aren't
                    // attached to this dead task.
                    session.subagent_state.stop_task(tool_use_id);
                } else if session.subagent_state.has_active_subagent() {
                    session
                        .subagent_state
                        .update_subagent_tool_status(tool_use_id, SUCCESS);
                    sync_subagent_tools(session);
                }
            }
            "SubagentStop" => {
                if let Some(agent_id) = event.agent_id.as_deref().filter(|id| !id.is_empty()) {
                    self.drop_agent_approvals(session, agent_id, at);
                }
            }
            _ => {}
        }
    }
}

/// Pushes the live subagent tool lists into the chat items of their Agent
/// calls, so the panel shows them as they start (not only after the Agent
/// completes).
fn sync_subagent_tools(session: &mut Session) {
    let lists: Vec<(String, Vec<_>)> = session
        .subagent_state
        .active_tasks
        .iter()
        .filter(|(_, task)| !task.subagent_tools.is_empty())
        .map(|(id, task)| {
            let views = task
                .subagent_tools
                .iter()
                .map(|tool| crate::model::SubagentToolView {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    summary: chat::tool_summary(&tool.name, &tool.input),
                    status: tool.status.clone(),
                })
                .collect();
            (id.clone(), views)
        })
        .collect();
    for (task_tool_id, views) in lists {
        session.chat.set_subagent_tools(&task_tool_id, views);
    }
}
