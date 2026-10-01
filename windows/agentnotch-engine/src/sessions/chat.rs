//! Chat history (HS§5.11, D§4.8): the `LoadChat` job, the items the store
//! keeps per session, the open-chat policy and the patches sent to the panel.
//!
//! - [`load_chat`] reads a whole transcript and returns the newest page of
//!   its conversation as [`ChatItem`]s (ConversationParser's `fullHistory`
//!   plus the paging the Mac never needed): a /clear drops what came before,
//!   tool results are joined to their calls, and an Agent/Task call carries
//!   the tool list of its subagent's own transcript.
//! - [`ChatState`] is one session's items as the store keeps them (SessionStore's
//!   `mergeMessages`, `applySubagentTools`, `updateToolStatus`, `trimChat`):
//!   transcript messages merged with the placeholders hooks create, ordered by
//!   time. A closed chat keeps its 40 newest items plus running tools; an open
//!   one keeps everything.
//! - [`OpenChats`] is ChatHistoryManager's policy: at most two chats keep a
//!   full history, the least recently touched is released.
//! - [`chat_update`] turns two histories into the panel's patch, and
//!   [`chat_image`] answers one image on request.
//!
//! Everything here is pure apart from [`load_chat`], which reads files (a
//! `Job::LoadChat` body, run on the I/O thread).

use crate::core::atomic::StdSecureFiles;
use crate::core::paths::Paths;
use crate::model::{
    ChatBody, ChatHistory, ChatImage, ChatItem, ChatMessage, ChatPage, ChatRole, ChatUpdate,
    MessageBlock, SessionId, SubagentToolView, SubagentView, ToolOutput, ToolResultView,
};
use crate::runtime_types::{TranscriptCursor, TranscriptEntry};
use crate::sessions::locator::TranscriptLocator;
use crate::sessions::tool_input;
use crate::sessions::tool_results;
use crate::sessions::transcript::sync_transcript;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Items per page of a loaded chat.
pub const PAGE_SIZE: usize = 150;
/// Items a closed chat keeps (SessionStore.retainedChatItems), plus any tool
/// still running or waiting for approval.
pub const RETAINED_ITEMS: usize = 40;
/// Chats whose whole history is kept at once (ChatHistoryManager).
pub const MAX_OPEN_HISTORIES: usize = 2;
/// Hook placeholders younger than this survive a /clear: they belong to
/// whatever Claude does next (SessionStore.processFileUpdate).
const CLEAR_GRACE: Duration = Duration::from_secs(2);
/// The longest one-line tool summary, in characters.
const SUMMARY_LENGTH: usize = 200;

/// A tool item's `status`.
pub const RUNNING: &str = "running";
pub const WAITING_FOR_APPROVAL: &str = "waiting_for_approval";
pub const SUCCESS: &str = "success";
pub const ERROR: &str = "error";
pub const INTERRUPTED: &str = "interrupted";

/// Still to finish: a result or an answer is awaited.
pub fn is_pending(status: &str) -> bool {
    status == RUNNING || status == WAITING_FOR_APPROVAL
}

// ---- items ----

/// The one-line summary of a call: the file name, command, pattern, URL,
/// query or description a tool has (`tool_input::preview`).
pub fn tool_summary(name: &str, flat_input: &BTreeMap<String, String>) -> String {
    tool_input::preview(name, flat_input, Some(SUMMARY_LENGTH)).unwrap_or_default()
}

fn flat_value(flat: &BTreeMap<String, String>) -> Value {
    Value::Object(
        flat.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

fn tool_item(id: &str, name: &str, flat: &BTreeMap<String, String>, status: &str) -> ChatItem {
    ChatItem {
        id: id.to_owned(),
        body: ChatBody::Tool {
            name: name.to_owned(),
            summary: tool_summary(name, flat),
            status: status.to_owned(),
            input: flat_value(flat),
            result: None,
            subagent: None,
        },
    }
}

/// One item of a message, with what goes beside it.
struct Built {
    item: ChatItem,
    image: Option<ChatImage>,
}

/// The items of a message's blocks (`createChatItem`): empty text and
/// thinking blocks make none, a tool call starts out running.
fn build_items(message: &ChatMessage) -> Vec<Built> {
    let ids = message.item_ids();
    let mut built = Vec::new();
    for (block, id) in message.blocks.iter().zip(ids) {
        let (body, image) = match block {
            MessageBlock::Text(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                let body = match message.role {
                    ChatRole::User => ChatBody::User { text: text.clone() },
                    ChatRole::Assistant => ChatBody::Assistant { text: text.clone() },
                };
                (body, None)
            }
            MessageBlock::ToolUse { name, input, .. } => {
                let flat = tool_input::flatten_value(input);
                built.push(Built {
                    item: tool_item(&id, name, &flat, RUNNING),
                    image: None,
                });
                continue;
            }
            MessageBlock::Thinking(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                (ChatBody::Thinking { text: text.clone() }, None)
            }
            MessageBlock::Image {
                media_type,
                data_base64,
            } => {
                let image = ChatImage {
                    media_type: media_type.clone(),
                    data_base64: data_base64.clone(),
                };
                (
                    ChatBody::Image {
                        media_type: media_type.clone(),
                        image_id: id.clone(),
                        bytes: image.byte_count(),
                    },
                    Some(image),
                )
            }
            MessageBlock::Interrupted => (ChatBody::Interrupted, None),
        };
        built.push(Built {
            item: ChatItem { id, body },
            image,
        });
    }
    built
}

/// A finished call's status and result (ToolCompletionResult.from): the
/// structured result when the line carried one, else its text (stdout, else
/// stderr, else the content; none for an interrupt).
pub fn completion(
    tool_name: &str,
    output: &ToolOutput,
    flat_input: &BTreeMap<String, String>,
) -> (&'static str, Option<ToolResultView>) {
    let status = if output.is_interrupted {
        INTERRUPTED
    } else if output.is_error {
        ERROR
    } else {
        SUCCESS
    };
    let text = if output.is_interrupted {
        None
    } else {
        [&output.stdout, &output.stderr, &output.content]
            .into_iter()
            .flatten()
            .find(|text| !text.is_empty())
            .map(|text| tool_results::clamp(text))
    };
    let view = match output.raw.as_ref().and_then(Value::as_object) {
        Some(data) => {
            let name = output.tool_name.as_deref().unwrap_or(tool_name);
            let mut view = tool_results::parse(name, data, flat_input);
            if let ToolResultView::Generic { text: slot, .. } = &mut view {
                if slot.is_none() {
                    *slot = text;
                }
            }
            Some(view)
        }
        None => text.map(|text| ToolResultView::Generic {
            text: Some(text),
            raw: None,
        }),
    };
    (status, view)
}

/// What [`apply_output`] did.
#[derive(Default)]
struct Applied {
    changed: bool,
    /// A pending call finished.
    finished: bool,
}

/// Joins a tool result to its call. A call still pending takes the status
/// and the result; one a hook already finished keeps its status and gets the
/// result only when it has none yet (the hooks' PostToolUse usually arrives
/// before the transcript line, and carries no result).
fn apply_output(body: &mut ChatBody, output: &ToolOutput) -> Applied {
    let ChatBody::Tool {
        name,
        status,
        input,
        result,
        ..
    } = body
    else {
        return Applied::default();
    };
    let pending = is_pending(status);
    if !pending && result.is_some() {
        return Applied::default();
    }
    let flat = tool_input::flatten_value(input);
    let (new_status, view) = completion(name, output, &flat);
    let mut applied = Applied::default();
    if pending {
        *status = new_status.to_owned();
        applied.changed = true;
        applied.finished = true;
    }
    if view.is_some() && (pending || result.is_none()) {
        *result = view;
        applied.changed = true;
    }
    applied
}

// ---- subagents ----

/// A tool call of a subagent, read from its own transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentTool {
    pub id: String,
    pub name: String,
    pub input: BTreeMap<String, String>,
    pub completed: bool,
}

/// A subagent's transcript read incrementally (SubagentTranscript): its tool
/// calls and which of them finished. Fed the entries of an `agent` sync.
#[derive(Debug, Clone, Default)]
pub struct SubagentTranscript {
    tools: Vec<SubagentTool>,
    index: HashMap<String, usize>,
    /// Results seen before their call (defensive; not expected).
    early_results: HashSet<String>,
}

impl SubagentTranscript {
    pub fn tools(&self) -> &[SubagentTool] {
        &self.tools
    }

    /// Starts over (the file was rewritten).
    pub fn reset(&mut self) {
        *self = SubagentTranscript::default();
    }

    /// Folds entries in; whether the tool list changed.
    pub fn fold(&mut self, entries: &[TranscriptEntry]) -> bool {
        let mut changed = false;
        for entry in entries {
            match entry {
                TranscriptEntry::ToolUse { id, name, input } => {
                    if self.index.contains_key(id) {
                        continue;
                    }
                    self.index.insert(id.clone(), self.tools.len());
                    self.tools.push(SubagentTool {
                        id: id.clone(),
                        name: name.clone(),
                        input: tool_input::flatten_value(input),
                        completed: self.early_results.remove(id),
                    });
                    changed = true;
                }
                TranscriptEntry::ToolResult { tool_use_id, .. } => {
                    match self.index.get(tool_use_id) {
                        Some(&position) => {
                            if !self.tools[position].completed {
                                self.tools[position].completed = true;
                                changed = true;
                            }
                        }
                        None => {
                            self.early_results.insert(tool_use_id.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        changed
    }
}

/// A subagent's tools as the panel lists them: a finished one succeeded,
/// the rest run.
pub fn subagent_tool_views(tools: &[SubagentTool]) -> Vec<SubagentToolView> {
    tools
        .iter()
        .map(|tool| SubagentToolView {
            id: tool.id.clone(),
            name: tool.name.clone(),
            summary: tool_summary(&tool.name, &tool.input),
            status: if tool.completed { SUCCESS } else { RUNNING }.to_owned(),
        })
        .collect()
}

/// The agent a finished Agent/Task call ran as (its structured result).
fn agent_id_of(body: &ChatBody) -> Option<String> {
    match body {
        ChatBody::Tool {
            name,
            result: Some(ToolResultView::Task { agent_id, .. }),
            ..
        } if tool_input::is_subagent_container(name) && !agent_id.is_empty() => {
            Some(agent_id.clone())
        }
        _ => None,
    }
}

/// An Agent/Task item's subagent view for a tool list: the agent from its
/// result (or its earlier view), the description from its input.
fn subagent_view(body: &ChatBody, views: Vec<SubagentToolView>) -> Option<SubagentView> {
    let ChatBody::Tool {
        name,
        input,
        subagent,
        ..
    } = body
    else {
        return None;
    };
    if !tool_input::is_subagent_container(name) || views.is_empty() {
        return None;
    }
    Some(SubagentView {
        agent_id: agent_id_of(body).or_else(|| subagent.as_ref().and_then(|s| s.agent_id.clone())),
        description: input
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tools: views,
    })
}

/// Tool calls a subagent made show under their Agent item, not on their own
/// (ChatHistoryManager.filterOutSubagentTools).
pub fn filter_out_subagent_tools(items: &[ChatItem]) -> Vec<ChatItem> {
    let mut hidden: HashSet<&str> = HashSet::new();
    for item in items {
        if let ChatBody::Tool {
            name,
            subagent: Some(view),
            ..
        } = &item.body
        {
            if tool_input::is_subagent_container(name) {
                hidden.extend(view.tools.iter().map(|tool| tool.id.as_str()));
            }
        }
    }
    if hidden.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| !hidden.contains(item.id.as_str()))
        .cloned()
        .collect()
}

// ---- LoadChat ----

const UNREADABLE: &str = "The transcript could not be read.";

/// The `Job::LoadChat` body: the newest `page` items of the conversation
/// (before the item `before`, when given). Subagent transcripts are looked
/// for next to the transcript.
pub fn load_chat(session: &SessionId, path: &Path, before: Option<&str>, page: usize) -> ChatPage {
    // Only existence is asked of the files: the std implementation does.
    let paths = Paths::native(Path::new(""));
    let locator = TranscriptLocator::new(&paths, &StdSecureFiles);
    load_chat_with(&locator, session, path, before, page)
}

/// [`load_chat`] with the locator given (the runtime's platform files).
pub fn load_chat_with(
    locator: &TranscriptLocator<'_>,
    session: &SessionId,
    path: &Path,
    before: Option<&str>,
    page: usize,
) -> ChatPage {
    let mut out = ChatPage {
        session: session.clone(),
        path: path.to_path_buf(),
        items: Vec::new(),
        has_earlier: 0,
        before: None,
        images: BTreeMap::new(),
        error: None,
        times: Vec::new(),
    };
    if std::fs::File::open(path).is_err() {
        out.error = Some(UNREADABLE.to_owned());
        return out;
    }

    let mut fold = Fold::default();
    let mut cursor = TranscriptCursor::default();
    loop {
        let delta = sync_transcript(session, path, cursor, false, locator.files);
        if delta.reset {
            // Rewritten while it was being read: what was folded is stale.
            fold.clear_all();
        }
        fold.apply(&delta.entries);
        let advanced = delta.cursor.offset > cursor.offset || delta.reset;
        cursor = delta.cursor;
        if cursor.offset >= cursor.size || !advanced {
            break;
        }
    }

    let total = fold.items.len();
    let end = before
        .and_then(|id| fold.index.get(id).copied())
        .unwrap_or(total);
    if before.is_some() && end < total {
        out.before = before.map(str::to_owned);
    }
    let start = end.saturating_sub(page.max(1));
    out.has_earlier = u32::try_from(start).unwrap_or(u32::MAX);
    let transcript_path = path.to_string_lossy();
    for entry in &fold.items[start..end] {
        let mut item = entry.item.clone();
        if let Some(agent_id) = agent_id_of(&item.body) {
            let tools = read_subagent(locator, session, &transcript_path, &agent_id);
            if let Some(view) = subagent_view(&item.body, subagent_tool_views(&tools)) {
                if let ChatBody::Tool { subagent, .. } = &mut item.body {
                    *subagent = Some(view);
                }
            }
        }
        if matches!(item.body, ChatBody::Image { .. }) {
            if let Some(image) = fold.images.get(&item.id) {
                out.images.insert(item.id.clone(), image.clone());
            }
        }
        out.items.push(item);
        out.times.push(entry.at);
    }
    out
}

/// The tools a subagent's own transcript lists (empty when it has none yet).
fn read_subagent(
    locator: &TranscriptLocator<'_>,
    session: &SessionId,
    transcript_path: &str,
    agent_id: &str,
) -> Vec<SubagentTool> {
    // The id comes from a transcript: never let it name another folder.
    if agent_id.contains(['/', '\\', '\0']) || agent_id == "." || agent_id == ".." {
        return Vec::new();
    }
    let agent_path = locator.subagent_transcript_path(transcript_path, agent_id);
    let agent_path = Path::new(&agent_path);
    let mut transcript = SubagentTranscript::default();
    let mut cursor = TranscriptCursor::default();
    loop {
        let delta = sync_transcript(session, agent_path, cursor, true, locator.files);
        if delta.reset {
            transcript.reset();
        }
        transcript.fold(&delta.entries);
        let advanced = delta.cursor.offset > cursor.offset || delta.reset;
        cursor = delta.cursor;
        if cursor.offset >= cursor.size || !advanced {
            break;
        }
    }
    transcript.tools
}

struct FoldItem {
    item: ChatItem,
    at: Option<SystemTime>,
}

/// A whole transcript folded into items, in file order.
#[derive(Default)]
struct Fold {
    items: Vec<FoldItem>,
    index: HashMap<String, usize>,
    images: BTreeMap<String, ChatImage>,
    /// Tool calls turned into items; Claude Code may write one twice while
    /// streaming. Kept across a /clear, as the Mac keeps it.
    seen_tools: HashSet<String>,
}

impl Fold {
    fn clear_all(&mut self) {
        *self = Fold::default();
    }

    fn apply(&mut self, entries: &[TranscriptEntry]) {
        for entry in entries {
            match entry {
                TranscriptEntry::Message(message) => self.message(message),
                TranscriptEntry::ToolOutput(output) => {
                    if let Some(&position) = self.index.get(&output.tool_use_id) {
                        apply_output(&mut self.items[position].item.body, output);
                    }
                }
                TranscriptEntry::Clear => {
                    // Everything before the /clear is gone from the conversation.
                    self.items.clear();
                    self.index.clear();
                    self.images.clear();
                }
                _ => {}
            }
        }
    }

    fn message(&mut self, message: &ChatMessage) {
        let blocks: Vec<MessageBlock> = message
            .blocks
            .iter()
            .filter(|block| match block {
                MessageBlock::ToolUse { id, .. } => self.seen_tools.insert(id.clone()),
                _ => true,
            })
            .cloned()
            .collect();
        if blocks.is_empty() {
            return;
        }
        let message = ChatMessage {
            blocks,
            ..message.clone()
        };
        for built in build_items(&message) {
            if self.index.contains_key(&built.item.id) {
                continue;
            }
            if let Some(image) = built.image {
                self.images.insert(built.item.id.clone(), image);
            }
            self.index.insert(built.item.id.clone(), self.items.len());
            self.items.push(FoldItem {
                item: built.item,
                at: message.at,
            });
        }
    }
}

// ---- the store's per-session items ----

#[derive(Debug, Clone)]
struct Entry {
    item: ChatItem,
    at: SystemTime,
}

/// What [`ChatState::apply_entries`] learned beyond the items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatOutcome {
    /// Something in the items changed.
    pub changed: bool,
    /// Tools that were pending and whose transcript result arrived, in order
    /// (the store completes them in its tracker and drops their approvals).
    pub completed_tools: Vec<String>,
    /// A /clear was seen.
    pub cleared: bool,
}

/// One session's chat items as the store keeps them.
#[derive(Debug, Clone, Default)]
pub struct ChatState {
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    images: BTreeMap<String, ChatImage>,
    has_earlier: u32,
    /// An earlier page was merged in: a newer page's own `has_earlier` no
    /// longer says how much lies before what is kept.
    paged_back: bool,
    revision: u64,
    /// The chat is open: everything is kept (see [`OpenChats`]).
    open: bool,
}

impl ChatState {
    pub fn new() -> ChatState {
        ChatState::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn has_earlier(&self) -> u32 {
        self.has_earlier
    }

    pub fn items(&self) -> impl Iterator<Item = &ChatItem> {
        self.entries.iter().map(|entry| &entry.item)
    }

    pub fn ids(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.item.id.as_str())
            .collect()
    }

    pub fn item(&self, id: &str) -> Option<&ChatItem> {
        self.index.get(id).map(|&i| &self.entries[i].item)
    }

    /// One image as a data URL (at most 2 MiB), without copying the history.
    pub fn image_data_url(&self, image_id: &str) -> Option<String> {
        self.images.get(image_id)?.data_url()
    }

    /// A tool item's status.
    pub fn tool_status(&self, id: &str) -> Option<&str> {
        match &self.item(id)?.body {
            ChatBody::Tool { status, .. } => Some(status),
            _ => None,
        }
    }

    /// Opens or closes the chat. Closing releases the history down to the
    /// retention rule.
    pub fn set_open(&mut self, open: bool) {
        if self.open == open {
            return;
        }
        self.open = open;
        if !open {
            self.paged_back = false;
            self.trim_if_closed();
        }
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    fn rebuild_index(&mut self) {
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.item.id.clone(), position))
            .collect();
    }

    fn push(&mut self, item: ChatItem, at: SystemTime) {
        self.index.insert(item.id.clone(), self.entries.len());
        self.entries.push(Entry { item, at });
    }

    fn sort_by_time(&mut self) {
        // Stable: items of one instant keep their order.
        self.entries.sort_by_key(|entry| entry.at);
        self.rebuild_index();
    }

    fn drop_unused_images(&mut self) {
        if self.images.is_empty() {
            return;
        }
        let index = &self.index;
        self.images.retain(|id, _| index.contains_key(id));
    }

    /// A closed chat keeps the newest [`RETAINED_ITEMS`] items plus any tool
    /// still running or waiting for approval (approvals and completions need
    /// them). An open chat keeps everything. Whether anything was dropped.
    pub fn trim_if_closed(&mut self) -> bool {
        if self.open || self.entries.len() <= RETAINED_ITEMS {
            return false;
        }
        let cut = self.entries.len() - RETAINED_ITEMS;
        let mut position = 0;
        self.entries.retain(|entry| {
            let keep = position >= cut
                || matches!(&entry.item.body, ChatBody::Tool { status, .. } if is_pending(status));
            position += 1;
            keep
        });
        self.rebuild_index();
        self.drop_unused_images();
        self.bump();
        true
    }

    // ---- hooks ----

    /// PreToolUse: the placeholder of a call, running. Nothing when the call
    /// has an item already. (The store leaves a subagent's own calls out: they
    /// show under their Agent item.)
    pub fn place_tool(
        &mut self,
        id: &str,
        name: &str,
        input: &BTreeMap<String, String>,
        at: SystemTime,
    ) -> bool {
        if self.index.contains_key(id) {
            return false;
        }
        self.push(tool_item(id, name, input, RUNNING), at);
        self.bump();
        self.trim_if_closed();
        true
    }

    /// Sets a tool item's status. With `only_if_pending`, a tool that
    /// already finished keeps its final status. Whether it changed.
    pub fn set_tool_status(&mut self, id: &str, status: &str, only_if_pending: bool) -> bool {
        let Some(&position) = self.index.get(id) else {
            return false;
        };
        let ChatBody::Tool {
            status: current, ..
        } = &mut self.entries[position].item.body
        else {
            return false;
        };
        if only_if_pending && !is_pending(current) {
            return false;
        }
        if current == status {
            return false;
        }
        *current = status.to_owned();
        self.bump();
        true
    }

    /// Esc stopped the turn: every call still running was interrupted
    /// (calls waiting for approval are the store's to settle).
    pub fn interrupt_running(&mut self) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if let ChatBody::Tool { status, .. } = &mut entry.item.body {
                if status == RUNNING {
                    *status = INTERRUPTED.to_owned();
                    changed = true;
                }
            }
        }
        if changed {
            self.bump();
        }
        changed
    }

    /// Live subagent tools from hooks, shown under the Agent call.
    pub fn set_subagent_tools(&mut self, task_tool_id: &str, tools: Vec<SubagentToolView>) -> bool {
        let Some(&position) = self.index.get(task_tool_id) else {
            return false;
        };
        let Some(view) = subagent_view(&self.entries[position].item.body, tools) else {
            return false;
        };
        let ChatBody::Tool { subagent, .. } = &mut self.entries[position].item.body else {
            return false;
        };
        if subagent.as_ref() == Some(&view) {
            return false;
        }
        *subagent = Some(view);
        self.bump();
        true
    }

    /// Tools read from a subagent's own transcript, attached to its Agent
    /// call (an unchanged list leaves the item alone).
    pub fn apply_subagent_tools(&mut self, task_tool_id: &str, tools: &[SubagentTool]) -> bool {
        self.set_subagent_tools(task_tool_id, subagent_tool_views(tools))
    }

    // ---- transcript ----

    /// A /clear (or a rewritten transcript): drops what the transcript no
    /// longer has, but keeps the hook placeholders of the last moments.
    pub fn clear_history(&mut self, now: SystemTime) {
        let cutoff = now
            .checked_sub(CLEAR_GRACE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.entries.retain(|entry| entry.at > cutoff);
        self.rebuild_index();
        self.drop_unused_images();
        self.has_earlier = 0;
        self.paged_back = false;
        self.bump();
    }

    /// Merges transcript messages (`mergeMessages`). A message with no time
    /// takes `now`. A call that has an item (a hook's placeholder) keeps its
    /// status and result and only learns its name and input.
    pub fn merge_messages(
        &mut self,
        messages: &[ChatMessage],
        now: SystemTime,
        sorts_by_time: bool,
    ) -> bool {
        let mut changed = false;
        let mut appended = false;
        for message in messages {
            let at = message.at.unwrap_or(now);
            for built in build_items(message) {
                if let Some(&position) = self.index.get(&built.item.id) {
                    changed |= self.refresh_call(position, &built.item, at);
                    continue;
                }
                if let Some(image) = built.image {
                    self.images.insert(built.item.id.clone(), image);
                }
                self.push(built.item, at);
                appended = true;
            }
        }
        if appended && sorts_by_time {
            self.sort_by_time();
        }
        if appended || changed {
            self.bump();
            self.trim_if_closed();
        }
        appended || changed
    }

    /// A transcript call meeting the item a hook made: the name and (unless
    /// empty) the input are the transcript's.
    fn refresh_call(&mut self, position: usize, built: &ChatItem, at: SystemTime) -> bool {
        let (
            ChatBody::Tool {
                name: new_name,
                input: new_input,
                summary: new_summary,
                ..
            },
            ChatBody::Tool {
                name,
                input,
                summary,
                ..
            },
        ) = (&built.body, &mut self.entries[position].item.body)
        else {
            return false;
        };
        let input_empty = new_input.as_object().is_none_or(|object| object.is_empty());
        let before = (name.clone(), input.clone());
        *name = new_name.clone();
        if !input_empty {
            *input = new_input.clone();
            *summary = new_summary.clone();
        } else {
            *summary = tool_summary(name, &tool_input::flatten_value(input));
        }
        if before == (name.clone(), input.clone()) {
            return false;
        }
        self.entries[position].at = at;
        true
    }

    /// Folds what a transcript sync read, in file order: messages, tool
    /// results (joined to their calls) and /clear. `sorts_by_time` re-orders
    /// appended items by their time (a history load); a /clear sorts too.
    pub fn apply_entries(
        &mut self,
        entries: &[TranscriptEntry],
        now: SystemTime,
        sorts_by_time: bool,
    ) -> ChatOutcome {
        let mut outcome = ChatOutcome::default();
        let mut sorts = sorts_by_time;
        let mut appended = false;
        for entry in entries {
            match entry {
                TranscriptEntry::Clear => {
                    self.clear_history(now);
                    outcome.cleared = true;
                    outcome.changed = true;
                    sorts = true;
                }
                TranscriptEntry::Message(message) => {
                    let before = self.entries.len();
                    // Sorting waits for the end of the batch.
                    if self.merge_messages(std::slice::from_ref(message), now, false) {
                        outcome.changed = true;
                    }
                    appended |= self.entries.len() != before;
                }
                TranscriptEntry::ToolOutput(output) => {
                    let Some(&position) = self.index.get(&output.tool_use_id) else {
                        continue;
                    };
                    let applied = apply_output(&mut self.entries[position].item.body, output);
                    if applied.changed {
                        outcome.changed = true;
                        self.bump();
                    }
                    if applied.finished {
                        outcome.completed_tools.push(output.tool_use_id.clone());
                    }
                }
                _ => {}
            }
        }
        if sorts && appended {
            self.sort_by_time();
            self.bump();
        }
        if outcome.changed {
            self.trim_if_closed();
        }
        outcome
    }

    /// Merges a loaded page (`processHistoryLoaded`): items the state lacks
    /// are added and ordered by time; a call a hook placed learns the result
    /// the transcript has. Whether anything changed.
    pub fn merge_page(&mut self, page: &ChatPage, now: SystemTime) -> bool {
        if page.error.is_some() {
            return false;
        }
        let times = filled_times(&page.times, page.items.len(), now);
        let mut changed = false;
        let mut appended = false;
        for (item, at) in page.items.iter().zip(times) {
            if let Some(&position) = self.index.get(&item.id) {
                changed |= merge_existing(&mut self.entries[position].item.body, &item.body);
                continue;
            }
            if let Some(image) = page.images.get(&item.id) {
                self.images.insert(item.id.clone(), image.clone());
            }
            self.push(item.clone(), at);
            appended = true;
        }
        if appended {
            self.sort_by_time();
        }
        // A page that names what it lies before is an earlier page; after
        // one, the newest page's own count no longer says what lies before
        // what is kept.
        let has_earlier = if page.before.is_some() {
            self.paged_back = true;
            page.has_earlier
        } else if self.paged_back {
            self.has_earlier.min(page.has_earlier)
        } else {
            page.has_earlier
        };
        if has_earlier != self.has_earlier {
            self.has_earlier = has_earlier;
            changed = true;
        }
        if appended || changed {
            self.bump();
            self.trim_if_closed();
        }
        appended || changed
    }

    /// The history as the panel sees it: subagent calls filtered out.
    pub fn history(&self) -> ChatHistory {
        let items: Vec<ChatItem> = self.entries.iter().map(|e| e.item.clone()).collect();
        ChatHistory {
            items: filter_out_subagent_tools(&items),
            has_earlier: self.has_earlier,
            images: self.images.clone(),
            revision: self.revision,
        }
    }
}

/// A page item's time: its own, else the nearest known one before it, else
/// after it, else `now`.
fn filled_times(times: &[Option<SystemTime>], count: usize, now: SystemTime) -> Vec<SystemTime> {
    let known = |i: usize| times.get(i).copied().flatten();
    let mut filled: Vec<Option<SystemTime>> = Vec::with_capacity(count);
    let mut last = None;
    for i in 0..count {
        last = known(i).or(last);
        filled.push(last);
    }
    let first = filled.iter().flatten().next().copied();
    filled
        .into_iter()
        .map(|at| at.or(first).unwrap_or(now))
        .collect()
}

/// A loaded call meeting an item the state has: a pending one takes the
/// loaded status; a finished one takes a result it lacks; the subagent view
/// and an empty input come from the page.
fn merge_existing(existing: &mut ChatBody, loaded: &ChatBody) -> bool {
    let (
        ChatBody::Tool {
            status,
            input,
            summary,
            result,
            subagent,
            ..
        },
        ChatBody::Tool {
            status: loaded_status,
            input: loaded_input,
            summary: loaded_summary,
            result: loaded_result,
            subagent: loaded_subagent,
            ..
        },
    ) = (existing, loaded)
    else {
        return false;
    };
    let mut changed = false;
    if is_pending(status) && !is_pending(loaded_status) {
        *status = loaded_status.clone();
        changed = true;
    }
    if result.is_none() && loaded_result.is_some() {
        *result = loaded_result.clone();
        changed = true;
    }
    if loaded_subagent.is_some() && subagent != loaded_subagent {
        *subagent = loaded_subagent.clone();
        changed = true;
    }
    if input.as_object().is_none_or(|o| o.is_empty())
        && loaded_input.as_object().is_some_and(|o| !o.is_empty())
    {
        *input = loaded_input.clone();
        *summary = loaded_summary.clone();
        changed = true;
    }
    changed
}

// ---- open chats ----

/// The sessions whose chat is open (ChatHistoryManager): only they keep a
/// whole history, so at most [`MAX_OPEN_HISTORIES`] are held and opening
/// another releases the one touched longest ago.
#[derive(Debug, Clone)]
pub struct OpenChats {
    limit: usize,
    /// Least recently touched first.
    order: Vec<SessionId>,
    /// Open chats whose transcript has been read in full.
    loaded: HashSet<SessionId>,
}

impl Default for OpenChats {
    fn default() -> Self {
        OpenChats::new()
    }
}

impl OpenChats {
    pub fn new() -> OpenChats {
        OpenChats::with_limit(MAX_OPEN_HISTORIES)
    }

    pub fn with_limit(limit: usize) -> OpenChats {
        OpenChats {
            limit: limit.max(1),
            order: Vec::new(),
            loaded: HashSet::new(),
        }
    }

    /// Marks `session` most recently touched (opening it if need be) and
    /// returns the chats released beyond the limit, oldest first.
    pub fn touch(&mut self, session: &SessionId) -> Vec<SessionId> {
        self.order.retain(|id| id != session);
        self.order.push(session.clone());
        let mut released = Vec::new();
        while self.order.len() > self.limit {
            let oldest = self.order.remove(0);
            self.loaded.remove(&oldest);
            released.push(oldest);
        }
        released
    }

    /// The chat closed, or its session ended: its history is released.
    /// Whether it was open.
    pub fn close(&mut self, session: &SessionId) -> bool {
        self.loaded.remove(session);
        match self.order.iter().position(|id| id == session) {
            Some(position) => {
                self.order.remove(position);
                true
            }
            None => false,
        }
    }

    pub fn is_open(&self, session: &SessionId) -> bool {
        self.order.contains(session)
    }

    /// The whole transcript was read for this open chat.
    pub fn mark_loaded(&mut self, session: &SessionId) {
        if self.is_open(session) {
            self.loaded.insert(session.clone());
        }
    }

    pub fn is_loaded(&self, session: &SessionId) -> bool {
        self.loaded.contains(session)
    }

    /// Open chats, least recently touched first.
    pub fn ids(&self) -> &[SessionId] {
        &self.order
    }
}

// ---- patches ----

/// SHA-256 of an item's JSON: what tells a changed item from an unchanged one.
pub fn item_hash(item: &ChatItem) -> [u8; 32] {
    let json = serde_json::to_vec(item).unwrap_or_default();
    Sha256::digest(&json).into()
}

/// The panel's update (D§3.6): a reset (every item) when there is no
/// `previous` (the chat opened, or paged), else only the items that are new
/// or whose content hash changed, the ids removed, and the full order. The
/// revision is `next`'s, and always above `previous`'s. Images never travel
/// inline: an image item carries `{image_id, bytes}` and the page asks for
/// the data with [`chat_image`].
pub fn chat_update(
    session_id: &str,
    previous: Option<&ChatHistory>,
    next: &ChatHistory,
    working: Option<String>,
    ended: bool,
    loading: bool,
) -> ChatUpdate {
    let order: Vec<String> = next.items.iter().map(|item| item.id.clone()).collect();
    let (reset, revision, items, removed) = match previous {
        None => (true, next.revision.max(1), next.items.clone(), Vec::new()),
        Some(previous) => {
            let before: HashMap<&str, [u8; 32]> = previous
                .items
                .iter()
                .map(|item| (item.id.as_str(), item_hash(item)))
                .collect();
            let items = next
                .items
                .iter()
                .filter(|item| before.get(item.id.as_str()) != Some(&item_hash(item)))
                .cloned()
                .collect();
            let kept: HashSet<&str> = order.iter().map(String::as_str).collect();
            let removed = previous
                .items
                .iter()
                .filter(|item| !kept.contains(item.id.as_str()))
                .map(|item| item.id.clone())
                .collect();
            (
                false,
                next.revision.max(previous.revision + 1),
                items,
                removed,
            )
        }
    };
    ChatUpdate {
        session_id: session_id.to_owned(),
        revision,
        reset,
        items,
        removed,
        order,
        has_earlier: next.has_earlier,
        working,
        ended,
        loading,
    }
}

/// One image of a history as a data URL, for `chat_image`: at most 2 MiB,
/// image media types only.
pub fn chat_image(history: &ChatHistory, image_id: &str) -> Option<String> {
    history.images.get(image_id)?.data_url()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_times_borrow_the_nearest_known_one() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(500);
        let t = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(
            filled_times(&[None, Some(t(10)), None, Some(t(20)), None], 5, now),
            vec![t(10), t(10), t(10), t(20), t(20)]
        );
        assert_eq!(filled_times(&[None, None], 2, now), vec![now, now]);
        assert!(filled_times(&[], 0, now).is_empty());
    }
}
