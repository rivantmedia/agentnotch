//! Helpers the sessions integration tests share: transcript line builders
//! (A1_TestSupport.swift's `TranscriptLines`), writing and appending
//! transcripts, folding decoded entries the way the store will, and fake
//! `SecureFiles` (links and file identities a Windows runner can't create).
#![allow(dead_code)]

use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::time::iso8601;
use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use agentnotch_engine::runtime_types::TranscriptEntry;
use agentnotch_engine::sessions::summary::{ConversationInfo, TranscriptSummary};
use agentnotch_engine::sessions::transcript::decode_line;
use serde_json::{json, Map, Value};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 2027-01-15, a fixed instant for lines that need a time.
pub fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn next_uuid() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("uuid-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn merged(mut line: Value, extra: Value) -> Value {
    if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            line.insert(key.clone(), value.clone());
        }
    }
    line
}

pub fn user(text: &str, at: SystemTime, extra: Value) -> Value {
    merged(
        json!({
            "type": "user", "uuid": next_uuid(), "timestamp": iso8601(at),
            "message": {"role": "user", "content": text},
        }),
        extra,
    )
}

pub fn assistant_text(text: &str, at: SystemTime) -> Value {
    json!({
        "type": "assistant", "uuid": next_uuid(), "timestamp": iso8601(at),
        "message": {"id": next_uuid(), "role": "assistant", "content": [{"type": "text", "text": text}]},
    })
}

/// An assistant line with usage (TranscriptTests' `assistant(...)`).
#[allow(clippy::too_many_arguments)]
pub fn assistant_usage(
    id: Option<&str>,
    model: &str,
    sidechain: bool,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    text: &str,
) -> Value {
    let mut message = json!({
        "model": model,
        "content": [{"type": "text", "text": text}],
        "usage": {
            "input_tokens": input, "output_tokens": output,
            "cache_read_input_tokens": cache_read, "cache_creation_input_tokens": cache_creation,
        },
    });
    if let Some(id) = id {
        message["id"] = json!(id);
    }
    json!({"type": "assistant", "isSidechain": sidechain, "uuid": next_uuid(),
           "requestId": "req_1", "message": message})
}

pub fn tool_use(id: &str, name: &str, input: Value, at: SystemTime) -> Value {
    json!({
        "type": "assistant", "uuid": next_uuid(), "timestamp": iso8601(at),
        "message": {"id": format!("msg-{id}"), "role": "assistant",
                    "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]},
    })
}

pub fn tool_result(id: &str, text: &str, at: SystemTime, tool_use_result: Option<Value>) -> Value {
    let mut line = json!({
        "type": "user", "uuid": next_uuid(), "timestamp": iso8601(at),
        "message": {"role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]},
    });
    if let Some(result) = tool_use_result {
        line["toolUseResult"] = result;
    }
    line
}

pub fn line_bytes(line: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(line).unwrap();
    bytes.push(b'\n');
    bytes
}

/// Writes (replacing) a transcript of these lines.
pub fn write_lines(path: &Path, lines: &[Value]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let bytes: Vec<u8> = lines.iter().flat_map(line_bytes).collect();
    std::fs::write(path, bytes).unwrap();
}

pub fn append_bytes(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

pub fn append_lines(path: &Path, lines: &[Value]) {
    let bytes: Vec<u8> = lines.iter().flat_map(line_bytes).collect();
    append_bytes(path, &bytes);
}

/// Decodes lines and folds them into a summary, as the store does.
pub fn summarize(lines: &[Value]) -> (ConversationInfo, TranscriptSummary) {
    let mut summary = TranscriptSummary::new();
    for line in lines {
        for entry in decode_line(line.to_string().as_bytes()) {
            summary.apply(&entry);
        }
    }
    (summary.info(), summary)
}

pub fn entries_of(lines: &[Value]) -> Vec<TranscriptEntry> {
    lines
        .iter()
        .flat_map(|line| decode_line(line.to_string().as_bytes()))
        .collect()
}

/// `StdSecureFiles` where chosen folders are links to others: paths under a
/// link resolve under its target for `identity` and `canonical`, as a real
/// symbolic link or junction would.
pub struct LinkedFiles {
    pub links: Vec<(PathBuf, PathBuf)>,
}

impl LinkedFiles {
    fn through(&self, path: &Path) -> PathBuf {
        for (link, target) in &self.links {
            if let Ok(rest) = path.strip_prefix(link) {
                return target.join(rest);
            }
        }
        path.to_path_buf()
    }
}

impl SecureFiles for LinkedFiles {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        StdSecureFiles.ensure_private_dir(&self.through(dir))
    }
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        StdSecureFiles.write_atomic(&self.through(path), bytes, mode, expect)
    }
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        StdSecureFiles.create_exclusive(&self.through(path), bytes)
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        StdSecureFiles.identity(&self.through(path))
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        if self.links.iter().any(|(link, _)| link == path) {
            return Ok(true);
        }
        StdSecureFiles.is_reparse(&self.through(path))
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        StdSecureFiles.canonical(&self.through(path))
    }
    fn is_private(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_private(&self.through(path))
    }
}

/// `StdSecureFiles` whose identity answers are chosen by the test (a Windows
/// runner reports volume 0 and index 0 for every file, production reports
/// real ones).
pub struct FixedIdentity(pub std::sync::Mutex<Option<(u64, u128)>>);

impl FixedIdentity {
    pub fn new(volume: u64, index: u128) -> Self {
        FixedIdentity(std::sync::Mutex::new(Some((volume, index))))
    }
    pub fn set(&self, volume: u64, index: u128) {
        *self.0.lock().unwrap() = Some((volume, index));
    }
}

impl SecureFiles for FixedIdentity {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        StdSecureFiles.ensure_private_dir(dir)
    }
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        StdSecureFiles.write_atomic(path, bytes, mode, expect)
    }
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        StdSecureFiles.create_exclusive(path, bytes)
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        let mut identity = StdSecureFiles.identity(path)?;
        if let Some((volume, index)) = *self.0.lock().unwrap() {
            identity.volume = volume;
            identity.index = index;
        }
        Ok(identity)
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_reparse(path)
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        StdSecureFiles.canonical(path)
    }
    fn is_private(&self, path: &Path) -> io::Result<bool> {
        StdSecureFiles.is_private(path)
    }
}

/// A JSON object from key/value pairs, for `extra` arguments.
pub fn object(pairs: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_owned(), value.clone());
    }
    Value::Object(map)
}
