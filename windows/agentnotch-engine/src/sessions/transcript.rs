//! The transcript sync job (`Job::SyncTranscript`): reads the complete lines
//! a JSONL transcript gained since a cursor and decodes each into
//! [`TranscriptEntry`]s (ConversationParser.swift's line rules and
//! TranscriptLineReader.swift). The job keeps no state between calls:
//! everything stateful (the summary fold, the task list, subagent follow-up,
//! the chat) is the store's, which feeds the entries in file order to
//! `TranscriptSummary::apply` and the `tasks::apply_transcript_*` hooks.
//!
//! Reading (Windows specifics, DESIGN-WIN 1774-1790):
//! - The file is opened with std's default share modes (read, write and
//!   delete), so Claude Code can keep appending to, or replace, a transcript
//!   being read. Its size is taken from the open handle.
//! - One call reads at most one chunk of 8 MiB (a longer line is read whole:
//!   chunks are added until a newline turns up), so a transcript of hundreds
//!   of MB reaches the store as a series of bounded deltas. The caller asks
//!   again while `cursor.offset < cursor.size` and the last delta advanced
//!   the offset; a half-written tail keeps the offset short of the size
//!   until its newline arrives.
//! - Only complete lines count (up to the last 0x0A). A trailing `\r` is JSON
//!   whitespace, so CRLF transcripts decode alike.
//! - A file shorter than the cursor's offset, or whose identity (volume and
//!   file index, when both are known and non-zero) changed, was rewritten or
//!   replaced: reading restarts at 0 and the delta says `reset`.

use crate::core::time::parse_iso8601;
use crate::model::{ChatMessage, ChatRole, MessageBlock, SessionId, ToolOutput};
use crate::platform::SecureFiles;
use crate::runtime_types::{TokenUsage, TranscriptCursor, TranscriptDelta, TranscriptEntry};
use crate::sessions::tasks::{created_task_id, json_string};
use crate::sessions::tool_input;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

/// Bytes read per chunk.
pub const CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// How many characters of a human prompt the summary needs.
pub const PROMPT_TEXT_LENGTH: usize = 200;

/// Whether `path` is a subagent's own transcript (`agent-<id>.jsonl`): the
/// runtime reads those with `agent` set (`Job::SyncTranscript` carries no
/// flag; a session's own transcript is named by its id).
pub fn is_agent_transcript(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("agent-") && name.ends_with(".jsonl"))
}

/// Reads what `path` gained since `cursor` (see the module doc). A missing or
/// unreadable file gives an empty delta with the cursor unchanged. With
/// `agent` (a subagent's own transcript, `agent-<id>.jsonl`) only `ToolUse`
/// and `ToolResult` entries are produced, sidechain lines included.
pub fn sync_transcript(
    session: &SessionId,
    path: &Path,
    cursor: TranscriptCursor,
    agent: bool,
    files: &dyn SecureFiles,
) -> TranscriptDelta {
    sync_transcript_chunked(session, path, cursor, agent, files, CHUNK_SIZE)
}

/// [`sync_transcript`] with the chunk size given (tests use small ones to put
/// chunk boundaries inside lines).
pub fn sync_transcript_chunked(
    session: &SessionId,
    path: &Path,
    cursor: TranscriptCursor,
    agent: bool,
    files: &dyn SecureFiles,
    chunk_size: usize,
) -> TranscriptDelta {
    let mut delta = TranscriptDelta {
        session: session.clone(),
        path: path.to_path_buf(),
        cursor,
        reset: false,
        entries: Vec::new(),
    };
    let Ok(mut file) = File::open(path) else {
        return delta;
    };
    let Ok(metadata) = file.metadata() else {
        return delta;
    };
    let size = metadata.len();
    let identity = files.identity(path).ok();

    let replaced = match (cursor.file, identity) {
        (Some(old), Some(new)) => {
            let known = |i: &crate::platform::FileIdentity| i.volume != 0 && i.index != 0;
            known(&old) && known(&new) && (old.volume != new.volume || old.index != new.index)
        }
        _ => false,
    };
    let mut offset = cursor.offset;
    if size < offset || replaced {
        offset = 0;
        delta.reset = true;
    }
    delta.cursor = TranscriptCursor {
        offset,
        size,
        file: identity.or(if delta.reset { None } else { cursor.file }),
    };
    if offset >= size || file.seek(SeekFrom::Start(offset)).is_err() {
        return delta;
    }

    let chunk_size = chunk_size.max(1);
    let mut buffer: Vec<u8> = Vec::new();
    let mut position = offset;
    while position < size {
        let wanted =
            usize::try_from((size - position).min(chunk_size as u64)).unwrap_or(chunk_size);
        let start = buffer.len();
        buffer.resize(start + wanted, 0);
        let got = read_fully(&mut file, &mut buffer[start..]);
        buffer.truncate(start + got);
        if got == 0 {
            break;
        }
        position += got as u64;
        if buffer[start..].contains(&b'\n') {
            break;
        }
    }
    let Some(last_newline) = buffer.iter().rposition(|byte| *byte == b'\n') else {
        // No complete line yet: the partial tail waits for its newline.
        return delta;
    };
    let complete = &buffer[..=last_newline];
    delta.cursor.offset = offset + complete.len() as u64;

    let mut decoder = Decoder::default();
    for line in complete.split(|byte| *byte == b'\n') {
        let line = trim_line(line);
        if line.is_empty() {
            continue;
        }
        if agent {
            decode_agent_line(line, &mut delta.entries);
        } else {
            decoder.decode(line, &mut delta.entries);
        }
    }
    delta
}

fn read_fully(file: &mut File, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    filled
}

/// A line without its `\r` and surrounding blanks (a whitespace-only line is
/// empty).
fn trim_line(line: &[u8]) -> &[u8] {
    let start = line
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &line[start..end]
}

// ---- decoding ----

/// One line as entries, in the order the store folds them (see
/// [`Decoder::decode`]).
pub fn decode_line(line: &[u8]) -> Vec<TranscriptEntry> {
    let mut entries = Vec::new();
    Decoder::default().decode(trim_line(line), &mut entries);
    entries
}

/// Claude Code's own test for words a person typed (`W6e`): a user entry that
/// isn't meta, carries no tool result, isn't a compact summary, and has no
/// origin or a human one. Task notifications (`origin.kind ==
/// "task-notification"`) and auto-continuations are not. Transcripts from
/// before `origin` existed are checked by their text instead.
pub fn is_human_prompt(json: &Value) -> bool {
    let Some(object) = json.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("user")
        || flag(object, "isMeta")
        || object.get("toolUseResult").is_some_and(|v| !v.is_null())
        || flag(object, "isCompactSummary")
    {
        return false;
    }
    if let Some(origin) = object.get("origin").and_then(Value::as_object) {
        if origin.get("kind").and_then(Value::as_str) != Some("human") {
            return false;
        }
    }
    let message = object.get("message").and_then(Value::as_object);
    !tool_input::is_injected_prompt(first_text(message).as_deref())
}

/// Slash-command echoes and caveats aren't conversation text.
pub fn is_command_text(text: &str) -> bool {
    text.starts_with("<command-name>")
        || text.starts_with("<local-command")
        || text.starts_with("Caveat:")
}

pub fn is_interrupt_marker(text: &str) -> bool {
    text.starts_with("[Request interrupted by user")
}

/// A user line echoing the /clear command.
pub fn is_clear_command(json: &Value) -> bool {
    let Some(object) = json.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let Some(message) = object.get("message").and_then(Value::as_object) else {
        return false;
    };
    first_text(Some(message))
        .is_some_and(|text| text.contains("<command-name>/clear</command-name>"))
}

/// A message's text: its `content` string, else the first text block's.
fn first_text(message: Option<&Map<String, Value>>) -> Option<String> {
    match message?.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => blocks
            .iter()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn flag(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool) == Some(true)
}

/// A line's `timestamp`. A date past the year 9999 is none: chrono reads one
/// spelt with a sign and turns it into a time without a check, which on
/// Windows (whose clock ends in the year 30828) would panic.
fn line_time(text: &str) -> Option<SystemTime> {
    if crate::sessions::has_signed_year(text) {
        return None;
    }
    parse_iso8601(text).and_then(crate::sessions::file_time)
}

fn text_of<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn count(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        _ => 0,
    }
}

/// `tool_result` content: a string, or the first text block of an array.
fn result_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => blocks
            .iter()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// Whether a failed result is the user's interrupt or refusal.
fn is_interruption(content: Option<&str>) -> bool {
    content.is_some_and(|text| {
        text.contains("Interrupted by user")
            || text.contains("interrupted by user")
            || text.contains("user doesn't want to proceed")
    })
}

/// A subagent's transcript line: its tool calls and their results, nothing
/// else (sidechain and meta lines included).
fn decode_agent_line(line: &[u8], out: &mut Vec<TranscriptEntry>) {
    let Ok(json) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    let Some(blocks) = json
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for block in blocks {
        let Some(block) = block.as_object() else {
            continue;
        };
        match text_of(block, "type") {
            Some("tool_use") => {
                if let (Some(id), Some(name)) = (text_of(block, "id"), text_of(block, "name")) {
                    out.push(TranscriptEntry::ToolUse {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        input: input_object(block),
                    });
                }
            }
            Some("tool_result") => {
                if let Some(id) = text_of(block, "tool_use_id") {
                    let (status, _) = result_status(block);
                    out.push(TranscriptEntry::ToolResult {
                        tool_use_id: id.to_owned(),
                        status,
                        task_id: None,
                    });
                }
            }
            _ => {}
        }
    }
}

fn input_object(block: &Map<String, Value>) -> Value {
    match block.get("input") {
        Some(input @ Value::Object(_)) => input.clone(),
        _ => json!({}),
    }
}

/// `success` | `error` | `interrupted`, and the result text.
fn result_status(block: &Map<String, Value>) -> (String, Option<String>) {
    let content = result_text(block.get("content"));
    let is_error = flag(block, "is_error");
    let status = if is_error && is_interruption(content.as_deref()) {
        "interrupted"
    } else if is_error {
        "error"
    } else {
        "success"
    };
    (status.to_owned(), content)
}

/// What one read remembers across its lines: the tool calls already turned
/// into chat blocks (Claude Code may write a call twice while streaming).
#[derive(Default)]
struct Decoder {
    seen_tool_ids: HashSet<String>,
}

impl Decoder {
    /// Decodes one main-transcript line. Order of the entries of one line:
    /// tool calls, an interrupt, then the `Assistant`/prompt entries, then
    /// the chat `Message`; a /clear line is `Injected` then `Clear`. Sidechain
    /// lines are skipped (their tool calls belong to a subagent's own
    /// transcript, read with `agent`).
    fn decode(&mut self, line: &[u8], out: &mut Vec<TranscriptEntry>) {
        let Ok(json) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let Some(object) = json.as_object() else {
            return;
        };
        let kind = text_of(object, "type");
        match kind {
            Some("summary") => return title(out, "summary", text_of(object, "summary")),
            Some("ai-title") => return title(out, "ai", text_of(object, "aiTitle")),
            Some("custom-title") => return title(out, "custom", text_of(object, "customTitle")),
            Some("user" | "assistant") => {}
            _ => return,
        }
        let at = text_of(object, "timestamp").and_then(line_time);
        if is_clear_command(&json) {
            // Everything before the /clear is gone from the conversation.
            out.push(TranscriptEntry::Injected { at });
            out.push(TranscriptEntry::Clear);
            return;
        }
        if flag(object, "isSidechain") {
            return;
        }
        let Some(message) = object.get("message").and_then(Value::as_object) else {
            return;
        };
        if kind == Some("assistant") {
            self.assistant(object, message, at, out);
        } else {
            self.user(&json, object, message, at, out);
        }
        if let Some(chat) = self.chat_message(object, message, kind == Some("user"), at) {
            out.push(TranscriptEntry::Message(chat));
        }
    }

    fn assistant(
        &mut self,
        object: &Map<String, Value>,
        message: &Map<String, Value>,
        at: Option<SystemTime>,
        out: &mut Vec<TranscriptEntry>,
    ) {
        let is_meta = flag(object, "isMeta");
        let content = message.get("content");
        let mut text = None;
        let mut interrupt = false;
        match content {
            Some(Value::String(content)) => {
                if !is_command_text(content) {
                    text = Some(content.clone());
                }
            }
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    let Some(block) = block.as_object() else {
                        continue;
                    };
                    if text_of(block, "type") == Some("tool_use") {
                        if let (Some(id), Some(name)) =
                            (text_of(block, "id"), text_of(block, "name"))
                        {
                            out.push(TranscriptEntry::ToolUse {
                                id: id.to_owned(),
                                name: name.to_owned(),
                                input: input_object(block),
                            });
                        }
                    }
                }
                // The last tool call or text block decides what the summary
                // shows: a call wins over text before it, text over calls
                // before it.
                for block in blocks.iter().rev() {
                    let Some(block) = block.as_object() else {
                        continue;
                    };
                    match text_of(block, "type") {
                        Some("tool_use") => break,
                        Some("text") => {
                            if let Some(block_text) = text_of(block, "text") {
                                if is_interrupt_marker(block_text) {
                                    interrupt = true;
                                } else {
                                    text = Some(block_text.to_owned());
                                }
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if is_meta {
            text = None;
            interrupt = false;
        }
        if interrupt {
            out.push(TranscriptEntry::Interrupt { at });
        }
        let usage = message
            .get("usage")
            .and_then(Value::as_object)
            .map(|usage| TokenUsage {
                input: count(usage.get("input_tokens")),
                output: count(usage.get("output_tokens")),
                cache_creation: count(usage.get("cache_creation_input_tokens")),
                cache_read: count(usage.get("cache_read_input_tokens")),
            });
        let model = text_of(message, "model").map(str::to_owned);
        out.push(TranscriptEntry::Assistant {
            uuid: text_of(object, "uuid").unwrap_or_default().to_owned(),
            at,
            text,
            synthetic: model.as_deref() == Some("<synthetic>"),
            model,
            usage,
            sidechain: false,
        });
    }

    fn user(
        &mut self,
        json: &Value,
        object: &Map<String, Value>,
        message: &Map<String, Value>,
        at: Option<SystemTime>,
        out: &mut Vec<TranscriptEntry>,
    ) {
        if let Some(blocks) = message.get("content").and_then(Value::as_array) {
            for block in blocks {
                if let Some(block) = block.as_object() {
                    if text_of(block, "type") == Some("tool_result") {
                        tool_result(object, block, out);
                    }
                }
            }
        }
        if flag(object, "isMeta") {
            return;
        }
        let human = is_human_prompt(json);
        let uuid = text_of(object, "uuid").unwrap_or_default();
        match message.get("content") {
            Some(Value::String(text)) => {
                if is_interrupt_marker(text) {
                    out.push(TranscriptEntry::Interrupt { at });
                } else if !human {
                    // A wake-up (task notification), compact summary or
                    // other injected entry: Claude works on, and it isn't
                    // anyone's words.
                    out.push(TranscriptEntry::Injected { at });
                } else if !is_command_text(text) {
                    human_prompt(out, uuid, at, text);
                }
            }
            Some(Value::Array(blocks)) => {
                let mut saw_text = false;
                let mut injected = false;
                for block in blocks {
                    let Some(block) = block.as_object() else {
                        continue;
                    };
                    if text_of(block, "type") != Some("text") {
                        continue;
                    }
                    let Some(text) = text_of(block, "text") else {
                        continue;
                    };
                    if is_interrupt_marker(text) {
                        out.push(TranscriptEntry::Interrupt { at });
                    } else if human && !saw_text && !is_command_text(text) {
                        saw_text = true;
                        human_prompt(out, uuid, at, text);
                    } else if !human && !injected {
                        injected = true;
                        out.push(TranscriptEntry::Injected { at });
                    }
                }
            }
            _ => {}
        }
    }

    /// The chat blocks of a line (`ChunkParser.parseMessage`): not meta, not
    /// a command echo, with a uuid.
    fn chat_message(
        &mut self,
        object: &Map<String, Value>,
        message: &Map<String, Value>,
        user: bool,
        at: Option<SystemTime>,
    ) -> Option<ChatMessage> {
        let uuid = text_of(object, "uuid")?;
        if flag(object, "isMeta") {
            return None;
        }
        let mut blocks = Vec::new();
        match message.get("content") {
            Some(Value::String(content)) => {
                if is_command_text(content) {
                    return None;
                }
                blocks.push(if is_interrupt_marker(content) {
                    MessageBlock::Interrupted
                } else {
                    MessageBlock::Text(content.clone())
                });
            }
            Some(Value::Array(content)) => {
                for block in content {
                    let Some(block) = block.as_object() else {
                        continue;
                    };
                    match text_of(block, "type") {
                        Some("text") => {
                            if let Some(text) = text_of(block, "text") {
                                blocks.push(if is_interrupt_marker(text) {
                                    MessageBlock::Interrupted
                                } else {
                                    MessageBlock::Text(text.to_owned())
                                });
                            }
                        }
                        Some("tool_use") => {
                            let (Some(id), Some(name)) =
                                (text_of(block, "id"), text_of(block, "name"))
                            else {
                                continue;
                            };
                            if !self.seen_tool_ids.insert(id.to_owned()) {
                                continue;
                            }
                            blocks.push(MessageBlock::ToolUse {
                                id: id.to_owned(),
                                name: name.to_owned(),
                                input: input_object(block),
                            });
                        }
                        Some("thinking") => {
                            if let Some(thinking) = text_of(block, "thinking") {
                                blocks.push(MessageBlock::Thinking(thinking.to_owned()));
                            }
                        }
                        // Claude Code stores inline images as base64 with
                        // their media type.
                        Some("image") => {
                            let source = block.get("source").and_then(Value::as_object);
                            if let (Some(media_type), Some(data)) = (
                                source.and_then(|s| text_of(s, "media_type")),
                                source.and_then(|s| text_of(s, "data")),
                            ) {
                                blocks.push(MessageBlock::Image {
                                    media_type: media_type.to_owned(),
                                    data_base64: data.to_owned(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if blocks.is_empty() {
            return None;
        }
        Some(ChatMessage {
            id: uuid.to_owned(),
            role: if user {
                ChatRole::User
            } else {
                ChatRole::Assistant
            },
            at,
            blocks,
        })
    }
}

fn title(out: &mut Vec<TranscriptEntry>, kind: &str, text: Option<&str>) {
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        out.push(TranscriptEntry::Title {
            kind: kind.to_owned(),
            text: text.to_owned(),
        });
    }
}

fn human_prompt(out: &mut Vec<TranscriptEntry>, uuid: &str, at: Option<SystemTime>, text: &str) {
    out.push(TranscriptEntry::HumanPrompt {
        uuid: uuid.to_owned(),
        at,
    });
    out.push(TranscriptEntry::PromptText {
        text: text.chars().take(PROMPT_TEXT_LENGTH).collect(),
    });
}

/// A `tool_result` block: its status (and the task it created), then its
/// text and raw structured result for the chat. Sidechain lines are not
/// reached (the caller skips them).
fn tool_result(
    line: &Map<String, Value>,
    block: &Map<String, Value>,
    out: &mut Vec<TranscriptEntry>,
) {
    let Some(id) = text_of(block, "tool_use_id") else {
        return;
    };
    let (status, content) = result_status(block);
    let tool_use_result = line.get("toolUseResult").and_then(Value::as_object);
    // Prefer the structured result, then the text.
    let task_id = if status == "success" {
        json_string(
            tool_use_result
                .and_then(|result| result.get("task"))
                .and_then(|task| task.get("id")),
        )
        .or_else(|| content.as_deref().and_then(created_task_id))
    } else {
        None
    };
    out.push(TranscriptEntry::ToolResult {
        tool_use_id: id.to_owned(),
        status: status.clone(),
        task_id,
    });
    out.push(TranscriptEntry::ToolOutput(ToolOutput {
        tool_use_id: id.to_owned(),
        tool_name: text_of(line, "toolName").map(str::to_owned),
        is_error: status != "success",
        is_interrupted: status == "interrupted",
        content,
        stdout: tool_use_result
            .and_then(|result| text_of(result, "stdout"))
            .map(str::to_owned),
        stderr: tool_use_result
            .and_then(|result| text_of(result, "stderr"))
            .map(str::to_owned),
        raw: tool_use_result.map(|result| Value::Object(result.clone())),
    }));
}
