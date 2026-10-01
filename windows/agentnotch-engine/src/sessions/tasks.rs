//! A session's progress list (SessionTaskList.swift, HS§5.9): the task list
//! Claude Code shows under the prompt, rebuilt the way Claude Code rebuilds
//! it itself:
//! - TaskCreate calls are pending until their result names the new task id
//!   (keyed by tool_use_id, so duplicate subjects can't be confused),
//! - TaskUpdate changes status/subject/activeForm (`deleted` removes the task),
//! - TodoWrite replaces the legacy todo list, shown only when no Task* tasks exist,
//! - subagent calls (hooks with agent_id, sidechain transcript lines) are ignored,
//! - the list survives new prompts; /clear resets it, and so does a new batch:
//!   Claude Code deletes a list once every task in it is completed, so a
//!   TaskCreate arriving when all tasks are done starts a fresh list (task
//!   ids keep counting up, so nothing collides).

use crate::model::{HookEvent, TaskItem, TaskProgress, TaskStatus};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// One task (or todo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Claude Code's task id ("3"), or "todo-<index>" for TodoWrite items.
    pub id: String,
    pub subject: String,
    /// Present-continuous label shown while in progress ("Running tests").
    pub active_form: Option<String>,
    pub description: Option<String>,
    pub status: TaskStatus,
    /// The subject is the "Task #<id>" stand-in for a task seen only in an update.
    pub placeholder_subject: bool,
}

impl Task {
    /// Label for the in-progress row: the active form, else the subject.
    pub fn active_label(&self) -> &str {
        match self.active_form.as_deref() {
            Some(form) if !form.is_empty() => form,
            _ => &self.subject,
        }
    }
}

/// A TaskCreate call whose task id isn't known yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCreate {
    pub subject: String,
    pub description: Option<String>,
    pub active_form: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskList {
    /// Tasks from the Task* tools, in creation order.
    tasks: Vec<Task>,
    /// The latest TodoWrite list.
    todos: Vec<Task>,
    /// TaskCreate calls keyed by tool_use_id, waiting for their task id.
    pending_creates: BTreeMap<String, PendingCreate>,
    /// TaskCreate calls already resolved or failed (tool_use_ids).
    settled_creates: BTreeSet<String>,
    /// Task ids deleted with TaskUpdate status "deleted".
    deleted_ids: BTreeSet<String>,
    /// A TodoWrite list was seen (an empty list is still a replacement).
    has_todo_list: bool,
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty()).map(str::to_owned)
}

/// A loosely typed JSON scalar as text (`JSONValue.string`): a number
/// counts, an empty string doesn't.
pub(crate) fn json_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl TaskList {
    pub fn new() -> Self {
        Self::default()
    }

    /// What to display: Task* tasks when any exist, else the TodoWrite list.
    pub fn items(&self) -> &[Task] {
        if self.tasks.is_empty() {
            &self.todos
        } else {
            &self.tasks
        }
    }

    pub fn total_count(&self) -> usize {
        self.items().len()
    }

    pub fn completed_count(&self) -> usize {
        self.items()
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count()
    }

    /// The first in-progress item.
    pub fn active_item(&self) -> Option<&Task> {
        self.items()
            .iter()
            .find(|t| t.status == TaskStatus::InProgress)
    }

    /// Completed fraction 0..=1; 0 without items.
    pub fn fraction(&self) -> f64 {
        match self.total_count() {
            0 => 0.0,
            total => self.completed_count() as f64 / total as f64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items().is_empty()
    }

    pub fn pending_creates(&self) -> &BTreeMap<String, PendingCreate> {
        &self.pending_creates
    }

    /// The progress the rows show; `None` without items.
    pub fn progress(&self) -> Option<TaskProgress> {
        let items = self.items();
        if items.is_empty() {
            return None;
        }
        Some(TaskProgress {
            done: self.completed_count() as u32,
            total: items.len() as u32,
            active_label: self.active_item().map(|t| t.active_label().to_owned()),
            items: items
                .iter()
                .map(|t| TaskItem {
                    id: Some(t.id.clone()),
                    label: t.subject.clone(),
                    status: t.status,
                })
                .collect(),
        })
    }

    /// Forget everything (/clear, SessionStart source "clear").
    pub fn reset(&mut self) {
        *self = TaskList::default();
    }

    /// PreToolUse TaskCreate.
    pub fn task_create_started(
        &mut self,
        tool_use_id: &str,
        subject: String,
        description: Option<String>,
        active_form: Option<String>,
    ) {
        self.start_new_batch_if_done();
        self.pending_creates.insert(
            tool_use_id.to_owned(),
            PendingCreate {
                subject,
                description,
                active_form,
            },
        );
    }

    /// Claude Code resets the task list once all of it is completed (in the
    /// terminal, 5 s later). A new task after that belongs to a new list;
    /// until then, the finished one stays (so a review row still reads 3/3).
    fn start_new_batch_if_done(&mut self) {
        if !self.tasks.is_empty() && self.tasks.iter().all(|t| t.status == TaskStatus::Completed) {
            self.tasks.clear();
        }
    }

    /// PostToolUse TaskCreate (or its transcript result) with the new task id.
    pub fn task_create_finished(
        &mut self,
        tool_use_id: &str,
        task_id: &str,
        subject: Option<&str>,
    ) {
        let pending = self.pending_creates.remove(tool_use_id);
        self.settled_creates.insert(tool_use_id.to_owned());
        if pending.is_none() && !self.tasks.iter().any(|t| t.id == task_id) {
            self.start_new_batch_if_done();
        }
        let resolved = non_empty(subject)
            .or_else(|| pending.as_ref().map(|p| p.subject.clone()))
            .unwrap_or_else(|| format!("Task #{task_id}"));
        self.upsert(
            task_id,
            Some(resolved),
            pending.as_ref().and_then(|p| p.description.clone()),
            pending.as_ref().and_then(|p| p.active_form.clone()),
            None,
        );
    }

    /// PostToolUseFailure TaskCreate: the task was never created.
    pub fn task_create_failed(&mut self, tool_use_id: &str) {
        self.pending_creates.remove(tool_use_id);
        self.settled_creates.insert(tool_use_id.to_owned());
    }

    /// TaskCreated hook event.
    pub fn task_created(&mut self, task_id: &str, subject: Option<&str>) {
        if !self.tasks.iter().any(|t| t.id == task_id) {
            self.start_new_batch_if_done();
        }
        self.upsert(task_id, non_empty(subject), None, None, None);
    }

    /// PreToolUse TaskUpdate; `status` is Claude Code's raw value and
    /// "deleted" removes the task.
    pub fn task_updated(
        &mut self,
        task_id: &str,
        status: Option<&str>,
        subject: Option<&str>,
        active_form: Option<&str>,
    ) {
        if status == Some("deleted") {
            self.tasks.retain(|t| t.id != task_id);
            self.deleted_ids.insert(task_id.to_owned());
            return;
        }
        self.upsert(
            task_id,
            non_empty(subject),
            None,
            non_empty(active_form),
            status.and_then(parse_status),
        );
    }

    /// TaskCompleted hook event.
    pub fn task_completed(&mut self, task_id: &str, subject: Option<&str>) {
        self.upsert(
            task_id,
            non_empty(subject),
            None,
            None,
            Some(TaskStatus::Completed),
        );
    }

    /// PreToolUse TodoWrite: the full list is replaced.
    pub fn todos_replaced(&mut self, todos: Vec<(String, String, Option<String>)>) {
        self.has_todo_list = true;
        self.todos = todos
            .into_iter()
            .enumerate()
            .map(|(index, (content, status, active_form))| Task {
                id: format!("todo-{index}"),
                subject: content,
                active_form: non_empty(active_form.as_deref()),
                description: None,
                status: parse_status(&status).unwrap_or(TaskStatus::Pending),
                placeholder_subject: false,
            })
            .collect();
    }

    fn upsert(
        &mut self,
        id: &str,
        subject: Option<String>,
        description: Option<String>,
        active_form: Option<String>,
        status: Option<TaskStatus>,
    ) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            if let Some(subject) = subject {
                task.subject = subject;
                task.placeholder_subject = false;
            }
            if description.is_some() {
                task.description = description;
            }
            if active_form.is_some() {
                task.active_form = active_form;
            }
            if let Some(status) = status {
                task.status = status;
            }
        } else {
            self.tasks.push(Task {
                id: id.to_owned(),
                placeholder_subject: subject.is_none(),
                subject: subject.unwrap_or_else(|| format!("Task #{id}")),
                active_form,
                description,
                status: status.unwrap_or(TaskStatus::Pending),
            });
        }
    }

    /// Combines a list rebuilt from the transcript (history before the app
    /// saw the session) with `self`, built from hooks since then. Hook data
    /// is newer and wins, except that a placeholder subject never replaces a
    /// real one.
    pub fn merged_into_reconstructed(&self, reconstructed: &TaskList) -> TaskList {
        let mut result = reconstructed.clone();
        for id in &self.deleted_ids {
            result.tasks.retain(|t| &t.id != id);
        }
        result.deleted_ids.extend(self.deleted_ids.iter().cloned());
        for task in &self.tasks {
            let Some(index) = result.tasks.iter().position(|t| t.id == task.id) else {
                result.tasks.push(task.clone());
                continue;
            };
            let base = &result.tasks[index];
            let mut merged = task.clone();
            if task.placeholder_subject && !base.placeholder_subject {
                merged.subject = base.subject.clone();
                merged.placeholder_subject = false;
            }
            merged.active_form = task
                .active_form
                .clone()
                .or_else(|| base.active_form.clone());
            merged.description = task
                .description
                .clone()
                .or_else(|| base.description.clone());
            result.tasks[index] = merged;
        }
        result
            .settled_creates
            .extend(self.settled_creates.iter().cloned());
        let settled = &self.settled_creates;
        result.pending_creates.retain(|id, _| !settled.contains(id));
        for (id, pending) in &self.pending_creates {
            result.pending_creates.insert(id.clone(), pending.clone());
        }
        if self.has_todo_list {
            result.todos = self.todos.clone();
            result.has_todo_list = true;
        }
        result
    }

    /// Applies a hook event. Returns true when it was a task event of the
    /// main session (whether or not it changed anything); subagent events
    /// are ignored.
    pub fn apply(&mut self, event: &HookEvent) -> bool {
        if event.is_subagent_event() {
            return false;
        }
        let empty = Map::new();
        let input = event.tool_input.as_ref().unwrap_or(&empty);
        match event.event.as_str() {
            "PreToolUse" => match event.tool.as_deref() {
                Some("TaskCreate") => {
                    let Some(tool_use_id) = event.tool_use_id.as_deref() else {
                        return false;
                    };
                    self.task_create_started(
                        tool_use_id,
                        json_string(input.get("subject")).unwrap_or_else(|| "Untitled task".into()),
                        json_string(input.get("description")),
                        json_string(input.get("activeForm")),
                    );
                    true
                }
                Some("TaskUpdate") => {
                    let Some(task_id) = json_string(input.get("taskId")) else {
                        return false;
                    };
                    self.task_updated(
                        &task_id,
                        json_string(input.get("status")).as_deref(),
                        json_string(input.get("subject")).as_deref(),
                        json_string(input.get("activeForm")).as_deref(),
                    );
                    true
                }
                Some("TodoWrite") => {
                    self.todos_replaced(todos(input.get("todos")));
                    true
                }
                _ => false,
            },
            "PostToolUse" if event.tool.as_deref() == Some("TaskCreate") => {
                let Some(tool_use_id) = event.tool_use_id.as_deref() else {
                    return false;
                };
                if let Some(task_id) = event.task_id.as_deref() {
                    self.task_create_finished(tool_use_id, task_id, event.task_subject.as_deref());
                }
                true
            }
            "PostToolUseFailure" if event.tool.as_deref() == Some("TaskCreate") => {
                if let Some(tool_use_id) = event.tool_use_id.as_deref() {
                    self.task_create_failed(tool_use_id);
                }
                true
            }
            "TaskCreated" => match event.task_id.as_deref() {
                Some(task_id) => {
                    self.task_created(task_id, event.task_subject.as_deref());
                    true
                }
                None => false,
            },
            "TaskCompleted" => match event.task_id.as_deref() {
                Some(task_id) => {
                    self.task_completed(task_id, event.task_subject.as_deref());
                    true
                }
                None => false,
            },
            "SessionStart" if event.source.as_deref() == Some("clear") => {
                self.reset();
                true
            }
            _ => false,
        }
    }

    // ---- transcript reconstruction ----

    /// A main-thread (non-sidechain) `tool_use` block of the transcript.
    pub fn apply_transcript_tool_use(&mut self, id: &str, name: &str, input: &Value) {
        let empty = Map::new();
        let input = input.as_object().unwrap_or(&empty);
        match name {
            "TaskCreate" => self.task_create_started(
                id,
                json_string(input.get("subject")).unwrap_or_else(|| "Untitled task".into()),
                json_string(input.get("description")),
                json_string(input.get("activeForm")),
            ),
            "TaskUpdate" => {
                if let Some(task_id) = json_string(input.get("taskId")) {
                    self.task_updated(
                        &task_id,
                        json_string(input.get("status")).as_deref(),
                        json_string(input.get("subject")).as_deref(),
                        json_string(input.get("activeForm")).as_deref(),
                    );
                }
            }
            "TodoWrite" => self.todos_replaced(todos(input.get("todos"))),
            _ => {}
        }
    }

    /// A main-thread `tool_result` of the transcript: resolves a pending
    /// TaskCreate by the id its result names (`toolUseResult.task.id`, else
    /// "Task #<id> created successfully").
    pub fn apply_transcript_tool_result(
        &mut self,
        tool_use_id: &str,
        is_error: bool,
        task_id: Option<&str>,
    ) {
        if !self.pending_creates.contains_key(tool_use_id) {
            return;
        }
        if is_error {
            self.task_create_failed(tool_use_id);
        } else if let Some(task_id) = task_id {
            self.task_create_finished(tool_use_id, task_id, None);
        }
    }
}

fn parse_status(raw: &str) -> Option<TaskStatus> {
    match raw {
        "pending" => Some(TaskStatus::Pending),
        "in_progress" => Some(TaskStatus::InProgress),
        "completed" => Some(TaskStatus::Completed),
        _ => None,
    }
}

/// TodoWrite's `todos` array → (content, status, activeForm).
pub fn todos(raw: Option<&Value>) -> Vec<(String, String, Option<String>)> {
    let Some(Value::Array(array)) = raw else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|element| {
            let todo = element.as_object()?;
            let content = json_string(todo.get("content"))?;
            Some((
                content,
                json_string(todo.get("status")).unwrap_or_else(|| "pending".into()),
                json_string(todo.get("activeForm")),
            ))
        })
        .collect()
}

/// The task id of a `Task #<id> created successfully: <subject>` result.
pub fn created_task_id(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("Task #")?;
    let end = rest.find(char::is_whitespace)?;
    let (id, tail) = rest.split_at(end);
    if id.is_empty() || !tail.trim_start().starts_with("created successfully") {
        return None;
    }
    Some(id.strip_suffix(':').unwrap_or(id).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::UNIX_EPOCH;

    fn pre(tool: &str, id: &str, input: Value, agent: Option<&str>) -> HookEvent {
        let mut e = HookEvent::new("s1", "PreToolUse", UNIX_EPOCH);
        e.status = "running_tool".into();
        e.agent_id = agent.map(str::to_owned);
        e.tool = Some(tool.into());
        e.tool_input = input.as_object().cloned();
        e.tool_use_id = Some(id.into());
        e
    }

    fn post_create(id: &str, task_id: Option<&str>, subject: Option<&str>) -> HookEvent {
        let mut e = HookEvent::new("s1", "PostToolUse", UNIX_EPOCH);
        e.tool = Some("TaskCreate".into());
        e.tool_use_id = Some(id.into());
        e.task_id = task_id.map(str::to_owned);
        e.task_subject = subject.map(str::to_owned);
        e
    }

    fn event(
        name: &str,
        task_id: Option<&str>,
        subject: Option<&str>,
        source: Option<&str>,
    ) -> HookEvent {
        let mut e = HookEvent::new("s1", name, UNIX_EPOCH);
        e.task_id = task_id.map(str::to_owned);
        e.task_subject = subject.map(str::to_owned);
        e.source = source.map(str::to_owned);
        e
    }

    fn ids(list: &TaskList) -> Vec<&str> {
        list.items().iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn create_is_pending_until_its_id_is_known() {
        let mut list = TaskList::new();
        list.apply(&pre(
            "TaskCreate",
            "toolu_a",
            json!({"subject": "Write tests", "activeForm": "Writing tests"}),
            None,
        ));
        assert_eq!(list.total_count(), 0);
        assert_eq!(list.pending_creates().len(), 1);
        list.apply(&post_create("toolu_a", Some("1"), Some("Write tests")));
        assert_eq!(list.total_count(), 1);
        assert!(list.pending_creates().is_empty());
        assert_eq!(list.items()[0].id, "1");
        assert_eq!(
            list.items()[0].active_form.as_deref(),
            Some("Writing tests")
        );
        assert_eq!(list.items()[0].status, TaskStatus::Pending);
    }

    #[test]
    fn duplicate_subjects_resolve_by_tool_use_id() {
        let mut list = TaskList::new();
        list.apply(&pre(
            "TaskCreate",
            "toolu_a",
            json!({"subject": "Same", "activeForm": "First"}),
            None,
        ));
        list.apply(&pre(
            "TaskCreate",
            "toolu_b",
            json!({"subject": "Same", "activeForm": "Second"}),
            None,
        ));
        list.apply(&post_create("toolu_b", Some("2"), None));
        list.apply(&post_create("toolu_a", Some("1"), None));
        assert_eq!(ids(&list), ["2", "1"]);
        let form = |id: &str| {
            list.items()
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .active_form
                .clone()
        };
        assert_eq!(form("1").as_deref(), Some("First"));
        assert_eq!(form("2").as_deref(), Some("Second"));
    }

    #[test]
    fn updates_progress_and_counts_completion() {
        let mut list = TaskList::new();
        for (index, subject) in ["A", "B", "C"].iter().enumerate() {
            list.apply(&pre(
                "TaskCreate",
                &format!("toolu_{index}"),
                json!({"subject": subject}),
                None,
            ));
            list.apply(&post_create(
                &format!("toolu_{index}"),
                Some(&(index + 1).to_string()),
                None,
            ));
        }
        list.apply(&pre(
            "TaskUpdate",
            "toolu_u1",
            json!({"taskId": "1", "status": "completed"}),
            None,
        ));
        list.apply(&pre(
            "TaskUpdate",
            "toolu_u2",
            json!({"taskId": "2", "status": "in_progress", "activeForm": "Doing B"}),
            None,
        ));
        assert_eq!(list.completed_count(), 1);
        assert_eq!(list.total_count(), 3);
        assert_eq!(list.active_item().unwrap().id, "2");
        assert_eq!(list.active_item().unwrap().active_label(), "Doing B");
        assert!((list.fraction() - 1.0 / 3.0).abs() < 0.0001);
        let progress = list.progress().unwrap();
        assert_eq!((progress.done, progress.total), (1, 3));
        assert_eq!(progress.active_label.as_deref(), Some("Doing B"));
    }

    #[test]
    fn deleted_removes_and_unknown_id_creates_placeholder() {
        let mut list = TaskList::new();
        list.apply(&pre("TaskCreate", "toolu_a", json!({"subject": "A"}), None));
        list.apply(&post_create("toolu_a", Some("1"), None));
        list.apply(&pre(
            "TaskUpdate",
            "toolu_u",
            json!({"taskId": "1", "status": "deleted"}),
            None,
        ));
        assert!(list.is_empty());
        assert!(list.progress().is_none());
        list.apply(&pre(
            "TaskUpdate",
            "toolu_v",
            json!({"taskId": 7, "status": "in_progress"}),
            None,
        ));
        assert_eq!(list.items()[0].subject, "Task #7");
        assert_eq!(list.active_item().unwrap().id, "7");
    }

    #[test]
    fn task_hook_events_upsert_and_complete() {
        let mut list = TaskList::new();
        list.apply(&event("TaskCreated", Some("4"), Some("Ship it"), None));
        assert_eq!(list.items()[0].subject, "Ship it");
        list.apply(&event("TaskCompleted", Some("4"), None, None));
        assert_eq!(list.completed_count(), 1);
    }

    #[test]
    fn todo_write_is_the_fallback_list() {
        let mut list = TaskList::new();
        let todos = json!({"todos": [
            {"content": "Read code", "status": "completed", "activeForm": "Reading code"},
            {"content": "Fix bug", "status": "in_progress", "activeForm": "Fixing bug"},
            {"content": "Test", "status": "pending", "activeForm": "Testing"}
        ]});
        list.apply(&pre("TodoWrite", "toolu_t", todos, None));
        assert_eq!(list.total_count(), 3);
        assert_eq!(list.completed_count(), 1);
        assert_eq!(list.active_item().unwrap().active_label(), "Fixing bug");
        list.apply(&pre(
            "TaskCreate",
            "toolu_a",
            json!({"subject": "Real task"}),
            None,
        ));
        list.apply(&post_create("toolu_a", Some("1"), None));
        assert_eq!(
            list.items()
                .iter()
                .map(|t| t.subject.as_str())
                .collect::<Vec<_>>(),
            ["Real task"]
        );
    }

    #[test]
    fn subagent_events_are_ignored() {
        let mut list = TaskList::new();
        assert!(!list.apply(&pre(
            "TaskCreate",
            "toolu_a",
            json!({"subject": "Sub"}),
            Some("agent-1")
        )));
        assert!(list.pending_creates().is_empty());
    }

    #[test]
    fn survives_prompts_and_resets_on_clear() {
        let mut list = TaskList::new();
        list.apply(&pre("TaskCreate", "toolu_a", json!({"subject": "A"}), None));
        list.apply(&post_create("toolu_a", Some("1"), None));
        list.apply(&event("UserPromptSubmit", None, None, Some("user")));
        assert_eq!(list.total_count(), 1);
        list.apply(&event("SessionStart", None, None, Some("clear")));
        assert!(list.is_empty());
    }

    #[test]
    fn failed_create_is_dropped() {
        let mut list = TaskList::new();
        list.apply(&pre("TaskCreate", "toolu_a", json!({"subject": "A"}), None));
        let mut failed = HookEvent::new("s1", "PostToolUseFailure", UNIX_EPOCH);
        failed.tool = Some("TaskCreate".into());
        failed.tool_use_id = Some("toolu_a".into());
        list.apply(&failed);
        assert!(list.pending_creates().is_empty());
        assert!(list.is_empty());
        // A late result for the failed call creates nothing.
        list.apply_transcript_tool_result("toolu_a", false, Some("1"));
        assert!(list.is_empty());
    }

    #[test]
    fn a_new_batch_starts_once_every_task_is_done() {
        let mut list = TaskList::new();
        list.apply(&pre("TaskCreate", "toolu_a", json!({"subject": "A"}), None));
        list.apply(&post_create("toolu_a", Some("1"), None));
        list.apply(&pre(
            "TaskUpdate",
            "toolu_u",
            json!({"taskId": "1", "status": "completed"}),
            None,
        ));
        assert_eq!(list.completed_count(), 1);
        list.apply(&pre("TaskCreate", "toolu_b", json!({"subject": "B"}), None));
        list.apply(&post_create("toolu_b", Some("2"), None));
        assert_eq!(ids(&list), ["2"]);
    }

    #[test]
    fn reconstructs_from_transcript_entries() {
        let mut list = TaskList::new();
        list.apply_transcript_tool_use(
            "toolu_1",
            "TaskCreate",
            &json!({"subject": "Plan", "activeForm": "Planning"}),
        );
        list.apply_transcript_tool_use("toolu_2", "TaskCreate", &json!({"subject": "Build"}));
        list.apply_transcript_tool_result(
            "toolu_1",
            false,
            created_task_id("Task #1 created successfully: Plan").as_deref(),
        );
        list.apply_transcript_tool_result("toolu_2", false, Some("2"));
        list.apply_transcript_tool_use(
            "toolu_3",
            "TaskUpdate",
            &json!({"taskId": "1", "status": "completed"}),
        );
        list.apply_transcript_tool_use(
            "toolu_4",
            "TaskUpdate",
            &json!({"taskId": "2", "status": "in_progress"}),
        );
        assert_eq!(ids(&list), ["1", "2"]);
        assert_eq!(list.completed_count(), 1);
        assert_eq!(list.active_item().unwrap().subject, "Build");
        assert_eq!(list.items()[0].active_form.as_deref(), Some("Planning"));
    }

    #[test]
    fn merges_transcript_history_with_newer_hook_state() {
        let mut reconstructed = TaskList::new();
        reconstructed.task_create_started(
            "toolu_1",
            "Design".into(),
            None,
            Some("Designing".into()),
        );
        reconstructed.task_create_finished("toolu_1", "1", None);
        reconstructed.task_create_started("toolu_2", "Build".into(), None, Some("Building".into()));
        reconstructed.task_create_finished("toolu_2", "2", None);
        reconstructed.task_create_started("toolu_3", "Test".into(), None, None);
        reconstructed.task_updated("1", Some("in_progress"), None, None);

        let mut hooks = TaskList::new();
        hooks.task_create_finished("toolu_3", "3", Some("Test"));
        hooks.task_updated("1", Some("completed"), None, None);
        hooks.task_updated("2", Some("in_progress"), None, None);
        hooks.task_updated("9", Some("deleted"), None, None);

        let merged = hooks.merged_into_reconstructed(&reconstructed);
        assert_eq!(ids(&merged), ["1", "2", "3"]);
        assert_eq!(
            merged
                .items()
                .iter()
                .map(|t| t.subject.as_str())
                .collect::<Vec<_>>(),
            ["Design", "Build", "Test"]
        );
        assert_eq!(merged.completed_count(), 1);
        assert_eq!(merged.active_item().unwrap().active_label(), "Building");
        assert!(merged.pending_creates().is_empty());
    }

    #[test]
    fn created_result_text_is_parsed() {
        assert_eq!(
            created_task_id("Task #12 created successfully: Do it").as_deref(),
            Some("12")
        );
        assert_eq!(
            created_task_id("Task #12: created successfully").as_deref(),
            Some("12")
        );
        assert_eq!(created_task_id("Task 12 created successfully"), None);
        assert_eq!(created_task_id("nope"), None);
    }
}
