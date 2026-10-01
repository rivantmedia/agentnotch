//! Claude Code's session registry, `<config dir>\sessions\<pid>.json`
//! (SessionRegistryScanner.swift, HS§10.1). Each running Claude Code process
//! keeps its file current with its status (busy / idle / shell / waiting), so
//! the registry covers sessions started before the app, VS Code sessions and
//! interrupts that no hook reports.
//!
//! Only `<digits>.json` files under 1 MB are opened. The key files beside
//! them are secrets and are never opened, nor is any other name: the folder
//! is listed and a name that doesn't match is skipped unseen.
//!
//! Claude Parallel Profiles links every config folder's `sessions` to one
//! shared folder (`~\.claude-shared\sessions`). Each physical folder is read
//! once however many config folders lead to it ([`grouped_by_sessions_folder`]),
//! and when it is reached through a link each entry is attributed to the
//! config folder its process actually runs with: the process's
//! `CLAUDE_CONFIG_DIR` (`Processes::config_dir_env`), unset meaning
//! `~\.claude` ([`attribute`]).
//!
//! **[ASSUMPTION]** Claude Code writes `sessions\<pid>.json` on Windows too
//! (the same JS bundle); without the folder every read is an empty snapshot
//! and sessions come from hooks alone.

use crate::core::paths::Paths;
use crate::core::time::from_secs_f64;
use crate::model::{RegistryEntry, RegistrySnapshot, SessionId};
use crate::platform::{EnvRead, Liveness, Processes, SecureFiles};
use crate::sessions::desktop::valid_host_session_id;
use crate::sessions::tasks::json_string;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A registry file at or over this size is not read.
pub const MAX_ENTRY_BYTES: u64 = 1_000_000;
/// Claude Code's `procStart` has one-second resolution.
const PROC_START_TOLERANCE: Duration = Duration::from_secs(2);
/// A reused pid belongs to a process started after the entry was written.
const STARTED_AT_TOLERANCE: Duration = Duration::from_secs(5);

/// How often the registries are read, and how soon after a Stop (Claude Code
/// marks the session idle once every Stop hook ran), then once more. The
/// runtime's timers (`SessionStore::next_deadline`) use these.
pub const SCAN_INTERVAL: Duration = Duration::from_secs(3);
pub const QUICK_RESCAN_DELAYS: [Duration; 2] =
    [Duration::from_millis(300), Duration::from_millis(1200)];

// ---- Parsing ----

/// A JSON number (or numeric string) as a finite float; never a boolean.
fn number(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim_matches([' ', '\t']).parse::<f64>().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

/// Epoch milliseconds as a time; missing, zero and negative are none.
fn time_from_ms(value: Option<&Value>) -> Option<SystemTime> {
    let ms = number(value).filter(|ms| *ms > 0.0)?;
    from_secs_f64(ms / 1000.0)
}

/// A process id: 1 up to `i32::MAX`, anything else (out of range, zero,
/// negative, not a number) is dropped instead of trapping.
fn valid_pid(value: Option<&Value>) -> Option<u32> {
    let value = number(value)?;
    if value < i64::MIN as f64 || value >= i64::MAX as f64 {
        return None;
    }
    let pid = value as i64;
    (1..=i64::from(i32::MAX))
        .contains(&pid)
        .then_some(pid as u32)
}

/// Parses a registry file's JSON object (`SessionRegistryEntry.init?(json:)`).
/// `pid` and `sessionId` are required; times are epoch milliseconds; the
/// process facts (`process_started`, `live`, `process_config_dir`) are filled
/// by [`read_registry`], not here.
pub fn parse_entry(json: &Value) -> Option<RegistryEntry> {
    let object = json.as_object()?;
    let pid = valid_pid(object.get("pid"))?;
    let session_id = json_string(object.get("sessionId"))?;
    let text = |key: &str| json_string(object.get(key));
    let mut entry = RegistryEntry::new(pid, SessionId::from(session_id));
    entry.cwd = text("cwd");
    entry.kind = text("kind");
    entry.entrypoint = text("entrypoint");
    entry.name = text("name");
    entry.name_source = text("nameSource");
    entry.version = text("version");
    entry.status = text("status");
    entry.waiting_for = text("waitingFor");
    entry.started_at = time_from_ms(object.get("startedAt"));
    entry.updated_at = time_from_ms(object.get("updatedAt"));
    entry.status_updated_at = time_from_ms(object.get("statusUpdatedAt"));
    entry.proc_start = text("procStart");
    entry.host_session_id = text("hostSessionId").filter(|id| valid_host_session_id(id));
    Some(entry)
}

/// Parses `ps -o lstart` text such as `Wed Sep  3 04:43:16 2026` (UTC). The
/// day may be space-padded; the weekday is read past, not checked.
pub fn parse_proc_start(text: &str) -> Option<SystemTime> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() != 5 {
        return None;
    }
    let rest = tokens[1..].join(" ");
    let parsed = chrono::NaiveDateTime::parse_from_str(&rest, "%b %d %H:%M:%S %Y").ok()?;
    let seconds = parsed.and_utc().timestamp();
    u64::try_from(seconds)
        .ok()
        .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
}

// ---- Liveness ----

/// The entry's process is running and is the same process that wrote it
/// (guards against pid reuse, which Windows does quickly, by comparing
/// process creation times).
///
/// - Not running (`Gone`): not live. `Alive` and `Unknown` run: a probe that
///   can't tell never drops a session.
/// - No creation time to compare: live.
/// - With `procStart`: the creation time is within 2 s of it.
/// - Else with `startedAt`: the creation time is at most `startedAt` + 5 s.
/// - Else live.
pub fn check_live(
    liveness: Liveness,
    creation: Option<SystemTime>,
    proc_start: Option<&str>,
    started_at: Option<SystemTime>,
) -> bool {
    if liveness == Liveness::Gone {
        return false;
    }
    let Some(creation) = creation else {
        return true;
    };
    if let Some(recorded) = proc_start.and_then(parse_proc_start) {
        let gap = match creation.duration_since(recorded) {
            Ok(gap) => gap,
            Err(early) => early.duration(),
        };
        return gap <= PROC_START_TOLERANCE;
    }
    if let Some(started_at) = started_at {
        return creation <= started_at + STARTED_AT_TOLERANCE;
    }
    true
}

// ---- The job ----

/// Is `name` a registry file's name: ASCII digits, then `.json`.
fn is_entry_name(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()))
}

/// Reads one entry's file: a regular file under 1 MB holding a JSON object.
fn read_entry(path: &Path) -> Option<RegistryEntry> {
    let file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() >= MAX_ENTRY_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    // The size may have changed since the stat: read no more than the limit.
    file.take(MAX_ENTRY_BYTES).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 >= MAX_ENTRY_BYTES {
        return None;
    }
    parse_entry(&serde_json::from_slice::<Value>(&bytes).ok()?)
}

/// `Job::ReadRegistry`: every parseable `<digits>.json` of `sessions_dir`,
/// sorted by pid, with each process's facts filled in: its creation time
/// (`process_started`), whether it is `live` ([`check_live`]) and, for a
/// folder reached through a link (`via_link`), the `CLAUDE_CONFIG_DIR` of each
/// live entry's process (`process_config_dir`; read only then, and never for
/// an entry whose process is gone). Untracked entries (`bg`, `daemon`, SDK)
/// are kept: [`RegistryEntry::is_tracked`] is the consumer's filter. A
/// folder that doesn't exist is an empty snapshot; one that can't be listed
/// is an empty snapshot with `error` set.
pub fn read_registry(
    sessions_dir: &Path,
    via_link: bool,
    procs: &dyn Processes,
    now: SystemTime,
) -> RegistrySnapshot {
    let mut snapshot = RegistrySnapshot {
        sessions_dir: sessions_dir.to_path_buf(),
        via_link,
        entries: Vec::new(),
        read_at: now,
        error: None,
    };
    let listing = match fs::read_dir(sessions_dir) {
        Ok(listing) => listing,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                snapshot.error = Some(format!("{:?}", error.kind()));
            }
            return snapshot;
        }
    };
    let mut entries: Vec<RegistryEntry> = listing
        .filter_map(|item| item.ok())
        .filter(|item| item.file_name().to_str().is_some_and(is_entry_name))
        .filter_map(|item| read_entry(&item.path()))
        .collect();
    entries.sort_by_key(|entry| entry.pid);
    for entry in &mut entries {
        let liveness = procs.liveness(entry.pid);
        entry.process_started = (liveness != Liveness::Gone)
            .then(|| procs.start_time(entry.pid))
            .flatten();
        entry.live = check_live(
            liveness,
            entry.process_started,
            entry.proc_start.as_deref(),
            entry.started_at,
        );
        if via_link && entry.live {
            entry.process_config_dir = Some(procs.config_dir_env(entry.pid));
        }
    }
    snapshot.entries = entries;
    snapshot
}

// ---- Shared folders ----

/// `path` with its links resolved as far as it exists: the nearest existing
/// ancestor is canonicalised and the rest is appended as spelt.
pub fn resolve_path(files: &dyn SecureFiles, path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(resolved) = files.canonical(&current) {
            let mut out = resolved;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_owned());
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The physical sessions folder of a config folder, links resolved.
pub fn sessions_folder_of(config_dir: &str, paths: &Paths, files: &dyn SecureFiles) -> PathBuf {
    let folder = paths.join(&paths.normalize(config_dir), "sessions");
    resolve_path(files, Path::new(&folder))
}

/// Whether the config folder's `sessions` (or the config folder itself) is a
/// link: its entries may belong to other config folders then.
pub fn is_linked_sessions_folder(config_dir: &str, paths: &Paths, files: &dyn SecureFiles) -> bool {
    let folder = paths.join(&paths.normalize(config_dir), "sessions");
    let resolved = resolve_path(files, Path::new(&folder));
    !paths.same(&resolved.to_string_lossy(), &folder)
}

/// The config folders that lead to one physical sessions folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsFolderGroup {
    /// The physical folder, links resolved.
    pub folder: PathBuf,
    /// The config folders (normalized, sorted, each once).
    pub aliases: Vec<String>,
}

/// Config folders grouped by the physical sessions folder they lead to, so
/// each is read once however many config folders share it. Sorted by folder.
pub fn grouped_by_sessions_folder(
    dirs: &[String],
    paths: &Paths,
    files: &dyn SecureFiles,
) -> Vec<SessionsFolderGroup> {
    let mut groups: BTreeMap<String, SessionsFolderGroup> = BTreeMap::new();
    for dir in dirs {
        let normalized = paths.normalize(dir);
        let folder = sessions_folder_of(&normalized, paths, files);
        let group = groups
            .entry(paths.key(&folder.to_string_lossy()))
            .or_insert_with(|| SessionsFolderGroup {
                folder,
                aliases: Vec::new(),
            });
        if !group.aliases.iter().any(|a| paths.same(a, &normalized)) {
            group.aliases.push(normalized);
        }
    }
    groups
        .into_values()
        .map(|mut group| {
            group.aliases.sort();
            group
        })
        .collect()
}

/// Which config folder an entry belongs to, for a folder that is shared (or
/// reached through a link): its process's `CLAUDE_CONFIG_DIR`, unset meaning
/// `default_dir` (`~\.claude`). A process that can't be asked
/// (`process_config_dir` unread or `Unreadable`) goes to `default_dir` if it
/// leads here (is among `aliases`), else to the first alias.
///
/// A process whose `CLAUDE_CONFIG_DIR` names a folder by another path (the
/// resolved target of a linked config folder, say `D:\dotfiles\claude-work`
/// for `~\.claude-work`) goes to the alias that resolves to the same place.
/// A folder no alias reaches is returned as named: a window not known yet.
/// `aliases` is non-empty, sorted and normalized.
pub fn attribute_entry(
    entry: &RegistryEntry,
    aliases: &[String],
    default_dir: &str,
    paths: &Paths,
    resolve: &dyn Fn(&str) -> String,
) -> String {
    let default_dir = paths.normalize(default_dir);
    match entry
        .process_config_dir
        .as_ref()
        .unwrap_or(&EnvRead::Unreadable)
    {
        EnvRead::Set(value) => {
            let normalized = paths.normalize(value);
            if let Some(alias) = aliases.iter().find(|a| paths.same(a, &normalized)) {
                return alias.clone();
            }
            let wanted = paths.key(&resolve(&normalized));
            aliases
                .iter()
                .find(|alias| paths.key(&resolve(alias)) == wanted)
                .cloned()
                .unwrap_or(normalized)
        }
        EnvRead::Unset => default_dir,
        EnvRead::Unreadable => {
            if aliases.iter().any(|a| paths.same(a, &default_dir)) {
                default_dir
            } else {
                aliases.first().cloned().unwrap_or(default_dir)
            }
        }
    }
}

/// Which config folder each entry belongs to ([`attribute_entry`] for every
/// entry of one read). A folder only one config folder reaches directly
/// (`is_shared` false) keeps them all, under its first alias. Entries keep
/// their order within a folder; a folder with none is absent.
pub fn attribute(
    entries: &[RegistryEntry],
    aliases: &[String],
    is_shared: bool,
    default_dir: &str,
    paths: &Paths,
    resolve: &dyn Fn(&str) -> String,
) -> BTreeMap<String, Vec<RegistryEntry>> {
    let mut sorted: Vec<String> = aliases.iter().map(|a| paths.normalize(a)).collect();
    sorted.sort();
    let mut result: BTreeMap<String, Vec<RegistryEntry>> = BTreeMap::new();
    let Some(first) = sorted.first() else {
        return result;
    };
    if !is_shared {
        if !entries.is_empty() {
            result.insert(first.clone(), entries.to_vec());
        }
        return result;
    }
    for entry in entries {
        let dir = attribute_entry(entry, &sorted, default_dir, paths, resolve);
        result.entry(dir).or_default().push(entry.clone());
    }
    result
}
