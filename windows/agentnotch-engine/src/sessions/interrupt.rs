//! The transcript interrupt watcher (JSONLInterruptWatcher.swift's port).
//! Claude Code writes the user's Esc into the transcript before any hook
//! says so, so a session whose main turn runs has its transcript watched
//! for it: the store's [`crate::sessions::SessionStore::interrupt_watches`]
//! names the files, the runtime calls [`InterruptWatcher::set_watches`] when
//! they change and [`InterruptWatcher::poll`] every [`POLL_INTERVAL`], and
//! what comes back is fed to the store as `SessionInput::Interrupt`.
//!
//! Windows: no directory change notification (NTFS directory metadata lags
//! for a file another process holds open). Each watch keeps one open handle
//! (std's default share modes: Claude Code can append to, or replace, the
//! file under it) and compares its size with what was read; nothing the
//! file already holds when the watch opens is news. The clock is passed in,
//! so the tests drive it.

use crate::model::SessionId;
use crate::runtime_types::SessionInput;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often the runtime polls.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A new session's transcript appears with its first message, written after
/// the hook that starts the watch: the file is looked for this many times.
pub const MAX_OPEN_ATTEMPTS: u32 = 30;

/// Between two looks for a transcript that is not there yet.
pub const OPEN_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// At most this much is read per poll (the rest next time). A line longer
/// than this is skipped, not read whole (the Mac reads everything new): an
/// interrupt line is short, and a turn the watcher misses still ends with its
/// hooks and the registry.
const MAX_READ: u64 = 8 * 1024 * 1024;

/// A transcript to watch while its session's main turn runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptWatch {
    pub session: SessionId,
    pub path: PathBuf,
}

/// Text Claude Code writes when a turn or a tool call is interrupted or
/// refused.
const CONTENT_PATTERNS: [&str; 4] = [
    "Interrupted by user",
    "interrupted by user",
    "user doesn't want to proceed",
    "[Request interrupted by user",
];

fn contains(haystack: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Byte-level check of one transcript line (no decoding): a user line
/// carrying the interrupt marker, a failed tool result saying the user
/// interrupted or refused it, or a result flagged `"interrupted":true`
/// (Bash). The flag is trusted only on tool result lines, not in arbitrary
/// content.
pub fn is_interrupt_line(line: &[u8]) -> bool {
    if contains(line, "\"type\":\"user\"") && contains(line, "[Request interrupted by user") {
        return true;
    }
    if !contains(line, "\"tool_result\"") {
        return false;
    }
    if contains(line, "\"is_error\":true")
        && CONTENT_PATTERNS
            .iter()
            .any(|pattern| contains(line, pattern))
    {
        return true;
    }
    contains(line, "\"interrupted\":true")
}

enum Stage {
    /// The file isn't there yet: looked for again at `next_try`.
    Opening { attempts: u32, next_try: SystemTime },
    /// Open, read up to `offset`; `skipping` while inside a line too long
    /// to read whole (its rest, up to its newline, is not a line).
    Watching {
        file: File,
        offset: u64,
        skipping: bool,
    },
    /// Never appeared (or couldn't be read): given up on.
    GaveUp,
}

struct Watcher {
    path: PathBuf,
    stage: Stage,
}

/// The watches of every session whose main turn runs.
#[derive(Default)]
pub struct InterruptWatcher {
    watchers: BTreeMap<SessionId, Watcher>,
}

impl InterruptWatcher {
    pub fn new() -> Self {
        InterruptWatcher::default()
    }

    /// Makes the watches exactly `watches`: new ones start (the first look
    /// for the file is the next poll), a watch whose file changed starts
    /// over, the others stop. `same_file` says whether two paths name one
    /// file (a shared history reached through another config folder): that
    /// watch keeps going.
    pub fn set_watches(
        &mut self,
        watches: &[InterruptWatch],
        now: SystemTime,
        same_file: impl Fn(&Path, &Path) -> bool,
    ) {
        self.watchers
            .retain(|session, _| watches.iter().any(|watch| &watch.session == session));
        for watch in watches {
            if let Some(existing) = self.watchers.get(&watch.session) {
                if same_file(&existing.path, &watch.path) {
                    continue;
                }
            }
            self.watchers.insert(
                watch.session.clone(),
                Watcher {
                    path: watch.path.clone(),
                    stage: Stage::Opening {
                        attempts: 0,
                        next_try: now,
                    },
                },
            );
        }
    }

    pub fn is_watching(&self, session: &SessionId) -> bool {
        self.watchers.contains_key(session)
    }

    pub fn len(&self) -> usize {
        self.watchers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.watchers.is_empty()
    }

    /// One look at every watch: `SessionInput::Interrupt` (at `now`, the
    /// time the line was read) for each whose file gained an interrupt line.
    /// A file found now is read from its end: what it holds already isn't
    /// news.
    pub fn poll(&mut self, now: SystemTime) -> Vec<SessionInput> {
        let mut found = Vec::new();
        for (session, watcher) in &mut self.watchers {
            match &mut watcher.stage {
                Stage::Opening { attempts, next_try } => {
                    if now < *next_try {
                        continue;
                    }
                    match File::open(&watcher.path).and_then(|mut file| {
                        let end = file.seek(SeekFrom::End(0))?;
                        Ok((file, end))
                    }) {
                        Ok((file, offset)) => {
                            watcher.stage = Stage::Watching {
                                file,
                                offset,
                                skipping: false,
                            };
                        }
                        Err(_) => {
                            *attempts += 1;
                            if *attempts >= MAX_OPEN_ATTEMPTS {
                                watcher.stage = Stage::GaveUp;
                            } else {
                                *next_try = now + OPEN_RETRY_INTERVAL;
                            }
                        }
                    }
                }
                Stage::Watching {
                    file,
                    offset,
                    skipping,
                } => {
                    if read_interrupt(file, offset, skipping) {
                        found.push(SessionInput::Interrupt {
                            session: session.clone(),
                            at: now,
                        });
                    }
                }
                Stage::GaveUp => {}
            }
        }
        found
    }
}

/// Reads the complete lines the file gained since `offset` and says whether
/// one is an interrupt. A half-written line is read again next time, unless
/// it already fills a whole read: then it is skipped up to its newline
/// (`skipping`), so it can't stall the watch. A file that shrank was
/// replaced: reading goes on from its new end.
fn read_interrupt(file: &mut File, offset: &mut u64, skipping: &mut bool) -> bool {
    let Ok(size) = file.metadata().map(|metadata| metadata.len()) else {
        return false;
    };
    if size < *offset {
        *offset = size;
        *skipping = false;
        return false;
    }
    if size == *offset || file.seek(SeekFrom::Start(*offset)).is_err() {
        return false;
    }
    let wanted = (size - *offset).min(MAX_READ);
    let mut buffer = Vec::new();
    if file.take(wanted).read_to_end(&mut buffer).is_err() {
        return false;
    }
    let mut data = buffer.as_slice();
    if *skipping {
        let Some(end) = data.iter().position(|byte| *byte == b'\n') else {
            *offset += data.len() as u64;
            return false;
        };
        *offset += end as u64 + 1;
        data = &data[end + 1..];
        *skipping = false;
    }
    let Some(last_newline) = data.iter().rposition(|byte| *byte == b'\n') else {
        if data.len() as u64 >= MAX_READ {
            *offset += data.len() as u64;
            *skipping = true;
        }
        return false;
    };
    let complete = &data[..=last_newline];
    *offset += complete.len() as u64;
    complete
        .split(|byte| *byte == b'\n')
        .any(|line| !line.is_empty() && is_interrupt_line(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lines_are_not_interrupts() {
        assert!(!is_interrupt_line(b"{\"type\":\"assistant\"}"));
        assert!(is_interrupt_line(
            b"{\"type\":\"user\",\"message\":\"[Request interrupted by user]\"}"
        ));
    }
}
