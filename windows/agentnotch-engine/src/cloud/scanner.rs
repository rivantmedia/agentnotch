//! Per-session token totals from Claude Code's transcripts, for sync (the
//! Mac's `SessionTokenScanner.swift`, state v4): input, output, cache writes
//! and cache reads, subagents included; what they cost at list prices
//! ([`super::pricing`]), for sessions whose cost from Claude Code is missing
//! or can't be used; how many responses; the models used, most used first;
//! the first and last timestamp; and, for sessions found only on disk, the
//! working directory, entrypoint and title the transcript records.
//!
//! What counts, and once:
//! - A session is its transcript `<project>/<sessionId>.jsonl` plus its
//!   subagents: `<project>/<sessionId>/subagents/**/agent-*.jsonl`, and the
//!   older flat `<project>/agent-*.jsonl`, whose session is the one its
//!   lines name.
//! - One API response is written as a line per content block, each
//!   repeating the same `message.usage`: responses are keyed by
//!   `message.id|requestId`, and a repeat replaces the earlier value.
//! - A response is counted by the first file that claims it. Older
//!   transcripts repeat subagent lines inside the parent, and a resumed or
//!   forked session may copy earlier lines into its new file: neither is
//!   counted twice. Lines in a session's own transcript that name another
//!   session (copied from it), or that `/branch` copied from another one
//!   (`forkedFrom`), are left to that session altogether, their timestamps
//!   and titles too.
//! - `<synthetic>` messages (local errors) aren't responses.
//! - Two config folders that share `projects/` (Claude Parallel Profiles'
//!   links or junctions) name one file two ways: files are kept by their
//!   real path.
//! - A session more than one account ran (resumed under another account)
//!   is split by its owners ([`SessionOwner`], from the ledger): each line
//!   counts for the account that ran the session at its timestamp. Every
//!   session's totals are kept per account (`parts`).
//!
//! Incremental: per file a watermark {size, mtime, file identity, offset},
//! its share of the totals per account and short hashes of the responses it
//! counted (the claims; the index from response to file is rebuilt from them
//! in memory). An unchanged file is not opened; a grown one is read from the
//! offset; one that shrank or was replaced (a new file identity), or whose
//! owners changed for lines already counted, is recounted from the start,
//! its claims released first. State is kept in `cloud-scan-state.json`, the
//! Mac's format; on Windows the 64-bit `inode` holds the file index (folded
//! when it has 128 bits) and an added `volume` the volume serial.
//!
//! Only `.jsonl` files under a `projects` folder are ever opened; nothing in
//! a config folder's `sessions/` is, and neither is a link that leads out of
//! `projects`: the real path is checked as well as the one given. Reads use
//! `std::fs` and never write into a Claude folder. Synchronous and
//! thread-safe: call it off the UI thread.

use super::contract::{date, limit};
use super::files::{lock, StateFile};
use super::ledger::{SessionOwner, SessionOwners};
use super::pricing::{self, NanoUsd};
use crate::core::paths::PathStyle;
use crate::core::time::{parse_iso8601, EpochSeconds};
use crate::platform::{FileIdentity, SecureFiles};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The state file's name in `<support>`.
pub const FILE_NAME: &str = "cloud-scan-state.json";

/// Responses a file remembers for replacing repeats (a response's lines are
/// written together, so a few are enough).
pub const REMEMBERED_RESPONSES: usize = 32;

/// Bytes read per chunk of a transcript.
pub const CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// How far into a transcript the first timestamp is looked for.
pub const FIRST_TIMESTAMP_LIMIT: usize = 1024 * 1024;

// ---- What a scan answers ----

/// Token totals in the contract's terms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTokenTotals {
    pub input: i64,
    pub output: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
}

impl CloudTokenTotals {
    pub fn add(&mut self, other: &CloudTokenTotals) {
        self.input += other.input;
        self.output += other.output;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
    }

    pub fn subtract(&mut self, other: &CloudTokenTotals) {
        self.input -= other.input;
        self.output -= other.output;
        self.cache_creation -= other.cache_creation;
        self.cache_read -= other.cache_read;
    }

    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }

    /// The contract's tokens (never negative).
    pub fn contract(&self) -> super::contract::SyncTokens {
        super::contract::SyncTokens {
            input: self.input.max(0),
            output: self.output.max(0),
            cache_creation: self.cache_creation.max(0),
            cache_read: self.cache_read.max(0),
        }
    }
}

/// One account's share of a session (or the whole of it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTokenPart {
    pub tokens: CloudTokenTotals,
    /// Responses counted once.
    pub message_count: i64,
    /// Most used first.
    pub models: Vec<String>,
    pub first_timestamp: Option<SystemTime>,
    pub last_timestamp: Option<SystemTime>,
    /// What its responses cost at list prices; `None` when a model that
    /// made one has no known price.
    pub cost: Option<NanoUsd>,
}

impl SessionTokenPart {
    /// The cost in dollars, as the website gets it.
    pub fn estimated_cost_usd(&self) -> Option<f64> {
        self.cost.map(pricing::dollars)
    }
}

/// One session's totals over its files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTokenSummary {
    pub session_id: String,
    pub tokens: CloudTokenTotals,
    /// Responses counted once.
    pub message_count: i64,
    /// Most used first.
    pub models: Vec<String>,
    pub first_timestamp: Option<SystemTime>,
    pub last_timestamp: Option<SystemTime>,
    /// What its responses cost at list prices; `None` when a model that
    /// made one has no known price.
    pub cost: Option<NanoUsd>,
    /// The working directory the transcript's first line records.
    pub cwd: Option<String>,
    pub entrypoint: Option<String>,
    /// The transcript's own title (custom, else AI-generated, else
    /// summary). Never a prompt.
    pub title: Option<String>,
    /// Bytes of the main transcript, and when it was last written (epoch
    /// seconds).
    pub transcript_bytes: u64,
    pub transcript_modified: f64,
    /// By the account key of the owner each line was counted for ("" when
    /// scanned with no owner).
    pub parts: BTreeMap<String, SessionTokenPart>,
}

impl SessionTokenSummary {
    /// What `account_key` ran of it (nothing when it ran none).
    pub fn part(&self, account_key: &str) -> SessionTokenPart {
        self.parts.get(account_key).cloned().unwrap_or_default()
    }
}

// ---- The saved state (cloud-scan-state.json) ----

/// One response a file counted, kept to replace its repeats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub key: String,
    pub model: String,
    pub usage: CloudTokenTotals,
    /// The account it was counted for.
    pub owner: String,
    /// At list prices; `None` when its model has no known price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<NanoUsd>,
}

/// A file's share of one account's part.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default)]
    pub totals: CloudTokenTotals,
    #[serde(default)]
    pub responses: i64,
    #[serde(default)]
    pub model_counts: BTreeMap<String, i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub first: Option<SystemTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub last: Option<SystemTime>,
    /// What the priced responses cost, and how many had no known price.
    #[serde(default)]
    pub cost: NanoUsd,
    #[serde(default)]
    pub unpriced: i64,
}

impl Part {
    fn note(&mut self, stamp: SystemTime) {
        self.first = Some(self.first.map_or(stamp, |first| first.min(stamp)));
        self.last = Some(self.last.map_or(stamp, |last| last.max(stamp)));
    }

    // Wrapping, so a damaged transcript's sums can't overflow and a removal
    // always undoes its add; `known_cost` refuses what wrapped.
    fn add_cost(&mut self, response: Option<NanoUsd>) {
        match response {
            Some(cost) => self.cost = self.cost.wrapping_add(cost),
            None => self.unpriced += 1,
        }
    }

    fn remove_cost(&mut self, response: Option<NanoUsd>) {
        match response {
            Some(cost) => self.cost = self.cost.wrapping_sub(cost),
            None => self.unpriced -= 1,
        }
    }

    /// `None` while any response has no known price, or when the sum is
    /// beyond what the website takes.
    fn known_cost(&self) -> Option<NanoUsd> {
        (self.unpriced <= 0 && self.cost >= 0 && (self.cost as f64) < limit::COST_USD * 1e9)
            .then_some(self.cost)
    }
}

/// What the scanner remembers about one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileState {
    /// The session the file belongs to ("" until a flat agent file names it).
    pub session_id: String,
    #[serde(default)]
    pub size: u64,
    /// Epoch seconds.
    pub mtime: EpochSeconds,
    /// The file's index (Mac: the inode; Windows: the file index, folded to
    /// 64 bits). 0: unknown.
    #[serde(default)]
    pub inode: u64,
    /// The volume's serial (Windows) or device (Mac); absent in files the
    /// Mac wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u64>,
    #[serde(default)]
    pub offset: u64,
    /// The owners the file was counted with.
    #[serde(default)]
    pub owners: Vec<SessionOwner>,
    /// By owner account key.
    #[serde(default)]
    pub parts: BTreeMap<String, Part>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub first: Option<SystemTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub last: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_title: Option<String>,
    #[serde(default)]
    pub recent: Vec<Response>,
    /// Hashes of the responses this file counted.
    #[serde(default)]
    pub claimed: Vec<String>,
    /// Whether this is the session's own transcript (not a subagent's).
    #[serde(default)]
    pub is_main: bool,
}

impl FileState {
    fn new(session_id: &str, owners: &[SessionOwner]) -> Self {
        FileState {
            session_id: session_id.to_owned(),
            size: 0,
            mtime: EpochSeconds(0.0),
            inode: 0,
            volume: None,
            offset: 0,
            owners: owners.to_vec(),
            parts: BTreeMap::new(),
            first: None,
            last: None,
            cwd: None,
            entrypoint: None,
            custom_title: None,
            ai_title: None,
            summary_title: None,
            recent: Vec::new(),
            claimed: Vec::new(),
            is_main: false,
        }
    }
}

/// `cloud-scan-state.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub version: u32,
    /// By real path.
    pub files: BTreeMap<String, FileState>,
}

impl State {
    /// 3: totals per owner account. 4: and what they cost (every transcript
    /// is read again once). A price change bumps this with
    /// `CloudSyncPass.payloadVersion`.
    pub const CURRENT_VERSION: u32 = 4;
}

impl Default for State {
    fn default() -> Self {
        State {
            version: Self::CURRENT_VERSION,
            files: BTreeMap::new(),
        }
    }
}

// ---- Pure helpers ----

fn components(path: &str, style: PathStyle) -> Vec<&str> {
    let is_separator = |c: char| c == '/' || (style == PathStyle::Windows && c == '\\');
    path.split(is_separator).filter(|c| !c.is_empty()).collect()
}

fn same_name(style: PathStyle, a: &str, b: &str) -> bool {
    match style {
        PathStyle::Windows => a.eq_ignore_ascii_case(b),
        PathStyle::Posix => a == b,
    }
}

/// A transcript the scanner may open: a `.jsonl` file under a `projects`
/// folder, with no `sessions` folder below that and no `.` or `..`. Windows
/// paths split on both separators and compare the folder names without
/// regard to case. Pure.
pub fn is_safe_transcript_path(path: &str, style: PathStyle) -> bool {
    if !path.ends_with(".jsonl") {
        return false;
    }
    let parts = components(path, style);
    let Some(projects) = parts.iter().rposition(|c| same_name(style, c, "projects")) else {
        return false;
    };
    let below = &parts[projects + 1..];
    below.len() >= 2
        && !below
            .iter()
            .any(|c| same_name(style, c, "sessions") || *c == ".." || *c == ".")
}

/// Claude Code's session ids are UUIDs.
pub fn is_session_id(id: &str) -> bool {
    super::keys::is_uuid(id)
}

/// A line of a session's own transcript that belongs to another session:
/// one naming it, or one `/branch` copied from it. Pure.
pub fn is_copied(json: &Value, session_id: &str) -> bool {
    if let Some(owner) = json.get("sessionId").and_then(Value::as_str) {
        if !owner.is_empty() && owner != session_id {
            return true;
        }
    }
    if let Some(origin) = json
        .get("forkedFrom")
        .and_then(|f| f.get("sessionId"))
        .and_then(Value::as_str)
    {
        if !origin.is_empty() && origin != session_id {
            return true;
        }
    }
    false
}

/// A response's key as the claims keep it: 16 hex digits of its SHA-256
/// (short, and a collision among one PC's responses is out of reach). Pure.
pub fn claim_key(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn decode(line: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(line)
        .ok()
        .filter(Value::is_object)
}

fn text<'a>(json: &'a Value, key: &str) -> Option<&'a str> {
    json.get(key).and_then(Value::as_str)
}

fn stamp_of(json: &Value) -> Option<SystemTime> {
    text(json, "timestamp").and_then(parse_iso8601)
}

fn non_negative(value: Option<&Value>) -> i64 {
    pricing::integer(value).unwrap_or(0).max(0)
}

fn seconds_of(nanoseconds: i128) -> f64 {
    let whole = nanoseconds.div_euclid(1_000_000_000);
    let rest = nanoseconds.rem_euclid(1_000_000_000);
    whole as f64 + rest as f64 / 1e9
}

/// The 64 bits the state keeps of a file index (a 128-bit ReFS index folds).
fn inode_of(identity: &FileIdentity) -> u64 {
    (identity.index as u64) ^ ((identity.index >> 64) as u64)
}

fn volume_of(identity: &FileIdentity) -> Option<u64> {
    (identity.volume != 0).then_some(identity.volume)
}

/// Whether the file at a path is no longer the one that was counted.
fn replaced(existing: &FileState, inode: u64, volume: Option<u64>) -> bool {
    existing.inode != 0
        && (existing.inode != inode
            || matches!((existing.volume, volume), (Some(a), Some(b)) if a != b))
}

// ---- Reading complete lines ----

/// What reading from an offset found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadOutcome {
    /// The file shrank (it was rewritten): reading restarted at 0.
    pub did_reset: bool,
    /// Complete lines handed to the body.
    pub line_count: usize,
}

/// Calls `body` with every complete, non-empty line after `offset`, in
/// order, and moves `offset` past the last newline, so a half-written final
/// line is read again next time. `None` when the file can't be opened or
/// read.
pub fn for_each_line(
    path: &Path,
    offset: &mut u64,
    chunk_size: usize,
    mut body: impl FnMut(&[u8]),
) -> Option<ReadOutcome> {
    let mut file = File::open(path).ok()?;
    let size = file.seek(SeekFrom::End(0)).ok()?;
    let mut outcome = ReadOutcome::default();
    if size < *offset {
        *offset = 0;
        outcome.did_reset = true;
    }
    if size <= *offset || file.seek(SeekFrom::Start(*offset)).is_err() {
        return Some(outcome);
    }
    let chunk_size = chunk_size.max(1);
    let mut carry: Vec<u8> = Vec::new();
    let mut position = *offset;
    while position < size {
        let wanted = (size - position).min(chunk_size as u64) as usize;
        let mut chunk = vec![0u8; wanted];
        let read = read_some(&mut file, &mut chunk);
        if read == 0 {
            break;
        }
        chunk.truncate(read);
        position += read as u64;
        let mut buffer = if carry.is_empty() {
            chunk
        } else {
            let mut joined = std::mem::take(&mut carry);
            joined.extend_from_slice(&chunk);
            joined
        };
        let Some(last_newline) = buffer.iter().rposition(|b| *b == b'\n') else {
            carry = buffer;
            continue;
        };
        outcome.line_count += for_each_line_in(&buffer[..=last_newline], &mut body);
        *offset += (last_newline + 1) as u64;
        carry = buffer.split_off(last_newline + 1);
    }
    Some(outcome)
}

/// A read that fills what it can (a short read is not the end of the file).
fn read_some(file: &mut File, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    filled
}

/// Splits `data` (which ends with a newline) into non-empty lines.
fn for_each_line_in(data: &[u8], body: &mut impl FnMut(&[u8])) -> usize {
    let mut count = 0;
    for line in data.split(|b| *b == b'\n') {
        if !line.is_empty() {
            body(line);
            count += 1;
        }
    }
    count
}

/// Reads lines from the start until one of the session's own carries a
/// timestamp (giving up at a line longer than `limit` bytes). Pure apart
/// from the read.
pub fn read_first_timestamp(path: &Path, session_id: &str, limit: usize) -> Option<SystemTime> {
    let mut file = File::open(path).ok()?;
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < limit {
        let mut chunk = vec![0u8; 64 * 1024];
        let read = read_some(&mut file, &mut chunk);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        let Some(last_newline) = buffer.iter().rposition(|b| *b == b'\n') else {
            continue;
        };
        let mut found: Option<SystemTime> = None;
        for_each_line_in(&buffer[..=last_newline], &mut |line| {
            if found.is_some() {
                return;
            }
            if let Some(json) = decode(line) {
                if !is_copied(&json, session_id) {
                    found = stamp_of(&json);
                }
            }
        });
        if found.is_some() {
            return found;
        }
        buffer.drain(..=last_newline);
    }
    None
}

/// When the file was created (for ordering ties); `None` when unknown.
pub fn birth_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.created().ok()
}

/// Main transcripts under a physical `projects` folder:
/// `<root>/<slug>/<sessionId>.jsonl`, where the id looks like a UUID.
/// `(session id, path)`.
pub fn session_files(root: &Path, style: PathStyle) -> Vec<(String, String)> {
    let named = root
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| same_name(style, n, "projects"));
    if !named {
        return Vec::new();
    }
    let mut found = Vec::new();
    for slug in sorted_names(root) {
        if slug.starts_with('.') || same_name(style, &slug, "sessions") {
            continue;
        }
        let folder = root.join(&slug);
        for name in sorted_names(&folder) {
            let Some(id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if is_session_id(id) {
                found.push((id.to_owned(), path_text(&folder.join(&name))));
            }
        }
    }
    found
}

fn sorted_names(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `<projectDir>/<sessionId>/subagents/**/agent-*.jsonl`, sorted.
pub fn nested_subagent_files(
    project_dir: &Path,
    session_id: &str,
    style: PathStyle,
) -> Vec<String> {
    if !is_session_id(session_id) {
        return Vec::new();
    }
    let mut found = Vec::new();
    collect_agent_files(&project_dir.join(session_id).join("subagents"), &mut found);
    let mut files: Vec<String> = found
        .iter()
        .map(|p| path_text(p))
        .filter(|p| is_safe_transcript_path(p, style))
        .collect();
    files.sort();
    files
}

fn collect_agent_files(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // Folders that are links are not walked into.
        if kind.is_dir() {
            collect_agent_files(&path, found);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("agent-") && name.ends_with(".jsonl") {
            found.push(path);
        }
    }
}

/// The legacy flat `<projectDir>/agent-*.jsonl`, sorted.
pub fn flat_agent_files(project_dir: &Path, style: PathStyle) -> Vec<String> {
    sorted_names(project_dir)
        .into_iter()
        .filter(|n| n.starts_with("agent-") && n.ends_with(".jsonl"))
        .map(|n| path_text(&project_dir.join(n)))
        .filter(|p| is_safe_transcript_path(p, style))
        .collect()
}

// ---- The scanner ----

struct Inner {
    state: State,
    /// Response hash -> real path of the file that counted it (from `claimed`).
    claims: HashMap<String, String>,
    files_of_session: HashMap<String, BTreeSet<String>>,
    dirty: bool,
    /// A transcript's first own timestamp, by real path and inode (a
    /// transcript is only ever appended to), for ordering the backfill.
    heads: HashMap<String, (u64, Option<SystemTime>)>,
}

impl Inner {
    fn from(state: State) -> Self {
        let mut inner = Inner {
            state,
            claims: HashMap::new(),
            files_of_session: HashMap::new(),
            dirty: false,
            heads: HashMap::new(),
        };
        inner.rebuild_index();
        inner
    }

    fn rebuild_index(&mut self) {
        self.files_of_session.clear();
        self.claims.clear();
        for (path, file) in &self.state.files {
            if !file.session_id.is_empty() {
                self.files_of_session
                    .entry(file.session_id.clone())
                    .or_default()
                    .insert(path.clone());
            }
            for key in &file.claimed {
                self.claims
                    .entry(key.clone())
                    .or_insert_with(|| path.clone());
            }
        }
    }
}

/// Totals per session from the transcripts, kept in `cloud-scan-state.json`.
pub struct SessionTokenScanner {
    inner: Arc<Mutex<Inner>>,
    file: StateFile<State>,
    files: Arc<dyn SecureFiles>,
    style: PathStyle,
}

impl SessionTokenScanner {
    /// `file` keeps the watermarks (memory only for sealed runs and tests);
    /// `files` gives each file's identity and its real path.
    pub fn new(file: StateFile<State>, files: Arc<dyn SecureFiles>, style: PathStyle) -> Self {
        let state = match file.load() {
            Some(saved) if saved.version == State::CURRENT_VERSION => saved,
            _ => State::default(),
        };
        SessionTokenScanner {
            inner: Arc::new(Mutex::new(Inner::from(state))),
            file,
            files,
            style,
        }
    }

    /// The scanner of `<support>\cloud-scan-state.json` (`persist` false:
    /// memory only).
    pub fn in_support(support: &Path, files: Arc<dyn SecureFiles>, persist: bool) -> Self {
        let file = if persist {
            StateFile::new(
                Some(support.join(FILE_NAME)),
                Some(files.clone()),
                Duration::ZERO,
            )
        } else {
            StateFile::memory()
        };
        Self::new(file, files, PathStyle::native())
    }

    fn is_safe(&self, path: &str) -> bool {
        is_safe_transcript_path(path, self.style)
    }

    /// The path a transcript really is, when that is a transcript the
    /// scanner may open too: a link out of `projects` leads nowhere.
    fn real_path(&self, path: &str) -> Option<String> {
        if !self.is_safe(path) {
            return None;
        }
        let real = path_text(&self.files.canonical(Path::new(path)).ok()?);
        self.is_safe(&real).then_some(real)
    }

    /// Bring the session's files up to date and return its totals, split by
    /// `owners` (who ran it from when; empty: one unnamed owner, ""). `None`
    /// when its transcript can't be read.
    pub fn scan(
        &self,
        session_id: &str,
        transcript_path: &str,
        owners: &[SessionOwner],
    ) -> Option<SessionTokenSummary> {
        if !is_session_id(session_id) {
            return None;
        }
        let main = self.real_path(transcript_path)?;
        if !Path::new(&main).is_file() {
            return None;
        }
        let project_dir = Path::new(&main).parent()?.to_path_buf();
        let mut inner = lock(&self.inner);
        // Files of this session that went away take their counts with them.
        let gone: Vec<String> = inner
            .files_of_session
            .get(session_id)
            .into_iter()
            .flatten()
            .filter(|p| **p != main && !Path::new(p.as_str()).exists())
            .cloned()
            .collect();
        for path in gone {
            self.drop_file(&mut inner, &path);
        }
        self.update(&mut inner, &main, Some(session_id), true, owners);
        for path in nested_subagent_files(&project_dir, session_id, self.style) {
            if let Some(real) = self.real_path(&path) {
                self.update(&mut inner, &real, Some(session_id), false, owners);
            }
        }
        // Flat agent files name their session in their lines: this
        // session's, and ones not read yet (another session's are read when
        // that one is scanned).
        for path in flat_agent_files(&project_dir, self.style) {
            let known = inner.state.files.get(&path).map(|f| f.session_id.as_str());
            if !matches!(known, None | Some("")) && known != Some(session_id) {
                continue;
            }
            if let Some(real) = self.real_path(&path) {
                self.update(&mut inner, &real, None, false, owners);
            }
        }
        self.summary(&inner, session_id, &main)
    }

    /// The first timestamp of a transcript's own lines (not those copied
    /// from another session), reading lines until one is found (giving up at
    /// a line longer than a megabyte); the counted value when the file was
    /// scanned. For ordering and dating the backfill without scanning.
    /// `None` when none is found. Like `scan`, the path a link resolves to
    /// is checked too: a `<id>.jsonl` link to a file outside `projects` (one
    /// in a config folder's `sessions/`, a credential file) is never opened.
    pub fn first_timestamp(&self, transcript_path: &str, session_id: &str) -> Option<SystemTime> {
        let real = self.real_path(transcript_path)?;
        let identity = self.files.identity(Path::new(&real)).ok()?;
        let inode = inode_of(&identity);
        {
            let inner = lock(&self.inner);
            if let Some(file) = inner.state.files.get(&real) {
                if file.inode == inode && file.is_main {
                    return file.first;
                }
            }
            if let Some((head_inode, first)) = inner.heads.get(&real) {
                if *head_inode == inode {
                    return *first;
                }
            }
        }
        let first = read_first_timestamp(Path::new(&real), session_id, FIRST_TIMESTAMP_LIMIT);
        lock(&self.inner).heads.insert(real, (inode, first));
        first
    }

    /// The session's totals as last scanned (no file is read).
    pub fn cached_summary(&self, session_id: &str) -> Option<SessionTokenSummary> {
        let inner = lock(&self.inner);
        let main = inner
            .files_of_session
            .get(session_id)?
            .iter()
            .find(|p| inner.state.files.get(*p).is_some_and(|f| f.is_main))?
            .clone();
        self.summary(&inner, session_id, &main)
    }

    /// Forget files that are gone (Claude Code deletes old transcripts):
    /// their totals were sent, and their claims no longer need keeping.
    /// Returns how many were forgotten.
    pub fn prune_missing_files(&self) -> usize {
        let mut inner = lock(&self.inner);
        let gone: Vec<String> = inner
            .state
            .files
            .keys()
            .filter(|p| !Path::new(p.as_str()).exists())
            .cloned()
            .collect();
        for path in &gone {
            self.drop_file(&mut inner, path);
        }
        gone.len()
    }

    /// Write the watermarks if anything changed (in the background).
    pub fn save(&self) {
        let snapshot = {
            let mut inner = lock(&self.inner);
            if !inner.dirty {
                return;
            }
            inner.dirty = false;
            inner.state.clone()
        };
        self.file.save(move || snapshot);
    }

    /// Write the watermarks now.
    pub fn save_now(&self) {
        let snapshot = {
            let mut inner = lock(&self.inner);
            inner.dirty = false;
            inner.state.clone()
        };
        self.file.save_now(&snapshot);
    }

    /// Waits until every background write has landed (tests).
    pub fn flush(&self) {
        self.file.flush();
    }

    /// A copy of the saved state (tests, diagnostics).
    pub fn state(&self) -> State {
        lock(&self.inner).state.clone()
    }

    // ---- One file (lock held) ----

    fn update(
        &self,
        inner: &mut Inner,
        path: &str,
        session_id: Option<&str>,
        is_main: bool,
        owners: &[SessionOwner],
    ) {
        let Ok(identity) = self.files.identity(Path::new(path)) else {
            if inner.state.files.contains_key(path) {
                self.drop_file(inner, path);
            }
            return;
        };
        let size = identity.size;
        let mtime = seconds_of(identity.modified_ns);
        let inode = inode_of(&identity);
        let volume = volume_of(&identity);

        let existing = inner.state.files.get(path).cloned();
        let mut current = existing
            .clone()
            .unwrap_or_else(|| FileState::new(session_id.unwrap_or(""), owners));
        if let Some(existing) = &existing {
            // Compared to the microsecond: a saved double may come back one
            // step off.
            if existing.size == size
                && (existing.mtime.0 - mtime).abs() < 1e-6
                && existing.inode == inode
                && existing.volume == volume
                && existing.owners == owners
            {
                return;
            }
            let owners_hold = SessionOwners::agree(&existing.owners, owners, existing.last);
            if size < existing.offset || replaced(existing, inode, volume) || !owners_hold {
                // Rewritten or replaced, or lines already counted belong to
                // another account now: counted again from the start.
                self.release_claims(inner, path);
                current = FileState::new(session_id.unwrap_or(&existing.session_id), owners);
            }
            current.owners = owners.to_vec();
        }
        if let Some(session_id) = session_id {
            if current.session_id != session_id {
                current.session_id = session_id.to_owned();
            }
        }
        current.is_main = current.is_main || is_main;

        let mut offset = current.offset;
        let mut outcome = for_each_line(Path::new(path), &mut offset, CHUNK_SIZE, |line| {
            Self::consume(inner, line, &mut current, path)
        });
        if outcome.is_some_and(|o| o.did_reset) {
            // It shrank between the stat and the read: count it again from
            // the start, releasing what the saved state and this read
            // claimed.
            self.release_claims(inner, path);
            let claimed = std::mem::take(&mut current.claimed);
            Self::release(inner, &claimed, path);
            let kept_session = current.session_id.clone();
            current = FileState::new(&kept_session, owners);
            current.is_main = is_main;
            offset = 0;
            outcome = for_each_line(Path::new(path), &mut offset, CHUNK_SIZE, |line| {
                Self::consume(inner, line, &mut current, path)
            });
        }
        if outcome.is_none() {
            return;
        }
        current.offset = offset;
        current.size = size;
        current.mtime = EpochSeconds(mtime);
        current.inode = inode;
        current.volume = volume;

        let previous_session = inner
            .state
            .files
            .get(path)
            .map(|f| f.session_id.clone())
            .filter(|previous| *previous != current.session_id);
        if let Some(previous) = previous_session {
            if let Some(paths) = inner.files_of_session.get_mut(&previous) {
                paths.remove(path);
            }
        }
        if !current.session_id.is_empty() {
            inner
                .files_of_session
                .entry(current.session_id.clone())
                .or_default()
                .insert(path.to_owned());
        }
        inner.state.files.insert(path.to_owned(), current);
        inner.dirty = true;
    }

    fn consume(inner: &mut Inner, line: &[u8], file: &mut FileState, path: &str) {
        let Some(json) = decode(line) else {
            return;
        };
        let line_session = text(&json, "sessionId").filter(|s| !s.is_empty());
        if file.session_id.is_empty() {
            if let Some(line_session) = line_session {
                file.session_id = line_session.to_owned();
            }
        }
        // A line a resumed or forked session copied from another one: that
        // session's, counted (and dated) from its own transcript.
        if file.is_main && is_copied(&json, &file.session_id) {
            return;
        }
        let stamp = stamp_of(&json);
        // Whose it is: the account that ran the session then (a line with no
        // time goes with the latest one seen).
        let owner = SessionOwners::owner_at(stamp.or(file.last), &file.owners);
        if let Some(stamp) = stamp {
            file.first = Some(file.first.map_or(stamp, |first| first.min(stamp)));
            file.last = Some(file.last.map_or(stamp, |last| last.max(stamp)));
            file.parts.entry(owner.clone()).or_default().note(stamp);
        }
        if file.cwd.is_none() {
            if let Some(cwd) = text(&json, "cwd").filter(|c| !c.is_empty()) {
                file.cwd = Some(cwd.to_owned());
            }
        }
        if file.entrypoint.is_none() {
            if let Some(entrypoint) = text(&json, "entrypoint").filter(|e| !e.is_empty()) {
                file.entrypoint = Some(entrypoint.to_owned());
            }
        }
        match text(&json, "type") {
            Some("summary") => {
                if let Some(title) = text(&json, "summary").filter(|t| !t.is_empty()) {
                    file.summary_title = Some(title.to_owned());
                }
            }
            Some("ai-title") => {
                if let Some(title) = text(&json, "aiTitle").filter(|t| !t.is_empty()) {
                    file.ai_title = Some(title.to_owned());
                }
            }
            Some("custom-title") => {
                if let Some(title) = text(&json, "customTitle").filter(|t| !t.is_empty()) {
                    file.custom_title = Some(title.to_owned());
                }
            }
            Some("assistant") => Self::consume_response(inner, &json, file, path, &owner),
            _ => {}
        }
    }

    fn consume_response(
        inner: &mut Inner,
        json: &Value,
        file: &mut FileState,
        path: &str,
        owner: &str,
    ) {
        let Some(message) = json.get("message") else {
            return;
        };
        let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
            return;
        };
        let model = text(message, "model").unwrap_or("");
        if model == "<synthetic>" {
            return;
        }
        let raw = if let Some(id) = text(message, "id").filter(|i| !i.is_empty()) {
            format!("{id}|{}", text(json, "requestId").unwrap_or(""))
        } else if let Some(uuid) = text(json, "uuid").filter(|u| !u.is_empty()) {
            format!("uuid:{uuid}")
        } else {
            return;
        };
        let key = claim_key(&raw);
        let entry = CloudTokenTotals {
            input: non_negative(usage.get("input_tokens")),
            output: non_negative(usage.get("output_tokens")),
            cache_creation: non_negative(usage.get("cache_creation_input_tokens")),
            cache_read: non_negative(usage.get("cache_read_input_tokens")),
        };
        match inner.claims.get(&key).map(String::as_str) {
            None => {
                let cost = pricing::cost(model, usage);
                inner.claims.insert(key.clone(), path.to_owned());
                file.claimed.push(key.clone());
                let part = file.parts.entry(owner.to_owned()).or_default();
                part.totals.add(&entry);
                part.responses += 1;
                part.add_cost(cost);
                if !model.is_empty() {
                    *part.model_counts.entry(model.to_owned()).or_insert(0) += 1;
                }
                file.recent.push(Response {
                    key,
                    model: model.to_owned(),
                    usage: entry,
                    owner: owner.to_owned(),
                    cost,
                });
                if file.recent.len() > REMEMBERED_RESPONSES {
                    let extra = file.recent.len() - REMEMBERED_RESPONSES;
                    file.recent.drain(..extra);
                }
            }
            Some(claimed_by) if claimed_by == path => {
                // Another line of a response this file counted: its latest
                // usage wins, in the part it was first counted in.
                let Some(index) = file.recent.iter().rposition(|r| r.key == key) else {
                    return;
                };
                let counted = file.recent[index].owner.clone();
                let priced_as = if model.is_empty() {
                    file.recent[index].model.as_str()
                } else {
                    model
                };
                let cost = pricing::cost(priced_as, usage);
                let previous = file.recent[index].clone();
                let part = file.parts.entry(counted.clone()).or_default();
                part.totals.subtract(&previous.usage);
                part.totals.add(&entry);
                part.remove_cost(previous.cost);
                part.add_cost(cost);
                file.recent[index].cost = cost;
                if previous.model != model && !model.is_empty() {
                    if !previous.model.is_empty() {
                        let count = part.model_counts.entry(previous.model.clone()).or_insert(1);
                        *count -= 1;
                        if *count <= 0 {
                            part.model_counts.remove(&previous.model);
                        }
                    }
                    *part.model_counts.entry(model.to_owned()).or_insert(0) += 1;
                    file.recent[index].model = model.to_owned();
                }
                file.recent[index].usage = entry;
            }
            // Counted by another file (a subagent's own file, the session a
            // fork copied it from).
            Some(_) => {}
        }
    }

    fn release_claims(&self, inner: &mut Inner, path: &str) {
        let Some(claimed) = inner.state.files.get(path).map(|f| f.claimed.clone()) else {
            return;
        };
        Self::release(inner, &claimed, path);
    }

    fn release(inner: &mut Inner, keys: &[String], path: &str) {
        for key in keys {
            if inner.claims.get(key).is_some_and(|p| p == path) {
                inner.claims.remove(key);
            }
        }
    }

    fn drop_file(&self, inner: &mut Inner, path: &str) {
        self.release_claims(inner, path);
        if let Some(session_id) = inner.state.files.get(path).map(|f| f.session_id.clone()) {
            if let Some(paths) = inner.files_of_session.get_mut(&session_id) {
                paths.remove(path);
            }
        }
        inner.state.files.remove(path);
        inner.dirty = true;
    }

    // ---- Totals (lock held) ----

    fn summary(&self, inner: &Inner, session_id: &str, main: &str) -> Option<SessionTokenSummary> {
        let main_file = inner.state.files.get(main)?;
        let mut whole = Part::default();
        let mut parts: BTreeMap<String, Part> = BTreeMap::new();
        fn add(part: &Part, total: &mut Part) {
            total.totals.add(&part.totals);
            total.responses += part.responses;
            let (cost, overflowed) = total.cost.overflowing_add(part.cost);
            total.cost = cost;
            total.unpriced += part.unpriced + i64::from(overflowed);
            for (model, count) in &part.model_counts {
                *total.model_counts.entry(model.clone()).or_insert(0) += count;
            }
            if let Some(first) = part.first {
                total.first = Some(total.first.map_or(first, |t| t.min(first)));
            }
            if let Some(last) = part.last {
                total.last = Some(total.last.map_or(last, |t| t.max(last)));
            }
        }
        if let Some(paths) = inner.files_of_session.get(session_id) {
            for path in paths {
                let Some(file) = inner.state.files.get(path) else {
                    continue;
                };
                for (owner, part) in &file.parts {
                    add(part, &mut whole);
                    add(part, parts.entry(owner.clone()).or_default());
                }
                // Timestamps of lines outside any part (none today) still
                // date the session.
                if let Some(first) = file.first {
                    whole.first = Some(whole.first.map_or(first, |t| t.min(first)));
                }
                if let Some(last) = file.last {
                    whole.last = Some(whole.last.map_or(last, |t| t.max(last)));
                }
            }
        }
        fn models(counts: &BTreeMap<String, i64>) -> Vec<String> {
            let mut used: Vec<(&String, i64)> = counts
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(m, n)| (m, *n))
                .collect();
            used.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            used.into_iter().map(|(m, _)| m.clone()).collect()
        }
        Some(SessionTokenSummary {
            session_id: session_id.to_owned(),
            tokens: whole.totals,
            message_count: whole.responses,
            models: models(&whole.model_counts),
            first_timestamp: whole.first,
            last_timestamp: whole.last,
            cost: whole.known_cost(),
            cwd: main_file.cwd.clone(),
            entrypoint: main_file.entrypoint.clone(),
            title: main_file
                .custom_title
                .clone()
                .or_else(|| main_file.ai_title.clone())
                .or_else(|| main_file.summary_title.clone()),
            transcript_bytes: main_file.size,
            transcript_modified: main_file.mtime.0,
            parts: parts
                .iter()
                .map(|(owner, part)| {
                    (
                        owner.clone(),
                        SessionTokenPart {
                            tokens: part.totals,
                            message_count: part.responses,
                            models: models(&part.model_counts),
                            first_timestamp: part.first,
                            last_timestamp: part.last,
                            cost: part.known_cost(),
                        },
                    )
                })
                .collect(),
        })
    }
}

/// An epoch-seconds stamp as a time (for callers that compare with a
/// summary's `transcript_modified`).
pub fn time_of_seconds(seconds: f64) -> Option<SystemTime> {
    crate::core::time::from_secs_f64(seconds).or_else(|| {
        (seconds.is_finite() && seconds >= 0.0)
            .then(|| UNIX_EPOCH + Duration::from_secs_f64(seconds))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_is_sixteen_hex_digits() {
        let key = claim_key("msg_1|req_1");
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(key, claim_key("msg_1|req_2"));
    }

    #[test]
    fn lines_are_read_whatever_the_chunk_size() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "aa\nbbbb\n\ncc\nhalf").expect("written");
        for chunk in [1, 2, 3, 5, 64] {
            let mut offset = 0;
            let mut lines = Vec::new();
            let outcome = for_each_line(&path, &mut offset, chunk, |l| {
                lines.push(String::from_utf8_lossy(l).into_owned())
            })
            .expect("readable");
            assert_eq!(lines, ["aa", "bbbb", "cc"], "chunk {chunk}");
            assert_eq!(offset, 12, "chunk {chunk}");
            assert_eq!(outcome.line_count, 3);
            assert!(!outcome.did_reset);
        }
    }
}
