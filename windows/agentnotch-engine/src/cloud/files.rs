//! How the cloud keeps its state in `<support>` (CL§4; the Mac's
//! `CloudFiles.swift`): each file written whole and atomically through the
//! platform's [`SecureFiles`] (a new file with the protected DACL beside the
//! old one, flushed, then renamed over it), so a crash never leaves half a
//! file and no other user can read one even for a moment. `<support>` is
//! machine-local (`%LOCALAPPDATA%`, DESIGN-WIN §1.5). A sealed run and the
//! stores tests keep in memory never touch the disk.
//!
//! The files are the Mac's formats (CL§4.2): each store's `Contents` type is
//! its file's data-transfer type, with the Mac's field names and its dates
//! (ISO 8601 with milliseconds, [`super::contract::date`]). Fixtures of each
//! in `tests/fixtures/mac-files/` round-trip through them.

use crate::platform::{Expect, SecureFiles, WriteMode, WriteResult};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Locks a mutex whatever a panicking holder left behind: the cloud's state
/// stays usable (the worst case is one lost update, never a stuck service).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `N` bytes from the system's random source; `None` when it gives none
/// (then nothing that must be unpredictable is made: no install secret, no
/// PKCE verifier, no device id).
pub fn random_bytes<const N: usize>() -> Option<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).ok()?;
    Some(bytes)
}

/// Write `bytes` to `path` privately and atomically, creating its folder
/// (private) first when it is missing.
pub fn write_private(files: &dyn SecureFiles, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        if !folder.is_dir() {
            files.ensure_private_dir(folder)?;
        }
    }
    match files.write_atomic(path, bytes, WriteMode::Private, Expect::Nothing)? {
        WriteResult::Written => Ok(()),
        other => Err(std::io::Error::other(format!("not written: {other:?}"))),
    }
}

/// What a background write will write: asked for when the write happens, so
/// a store that changes ten times within the delay is encoded once.
type Snapshot = Box<dyn FnOnce() -> Option<Vec<u8>> + Send>;

/// One JSON file of cloud state: read once, written in the background or
/// right away. Background writes are throttled: at most one per
/// `write_delay`, always of the newest value. Not persisted (sealed, tests):
/// never touches the disk.
pub struct StateFile<T> {
    path: Option<PathBuf>,
    files: Option<Arc<dyn SecureFiles>>,
    write_delay: Duration,
    shared: Arc<Shared>,
    value: PhantomData<fn() -> T>,
}

struct Shared {
    state: Mutex<Pending>,
    /// Serialises writes; holds the sequence number of the last one written.
    written: Mutex<u64>,
    idle: Condvar,
}

struct Pending {
    /// The newest value not written yet.
    snapshot: Option<(u64, Snapshot)>,
    scheduled: bool,
    /// Background writes started and not finished.
    in_flight: usize,
    next_sequence: u64,
}

impl<T> StateFile<T>
where
    T: Serialize + DeserializeOwned,
{
    /// `path` `None` or `files` `None`: in memory only.
    pub fn new(
        path: Option<PathBuf>,
        files: Option<Arc<dyn SecureFiles>>,
        write_delay: Duration,
    ) -> Self {
        let persists = path.is_some() && files.is_some();
        StateFile {
            path: if persists { path } else { None },
            files: if persists { files } else { None },
            write_delay,
            shared: Arc::new(Shared {
                state: Mutex::new(Pending {
                    snapshot: None,
                    scheduled: false,
                    in_flight: 0,
                    next_sequence: 1,
                }),
                written: Mutex::new(0),
                idle: Condvar::new(),
            }),
            value: PhantomData,
        }
    }

    /// Memory only.
    pub fn memory() -> Self {
        Self::new(None, None, Duration::ZERO)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The saved value; `None` when there is none, it can't be read (a
    /// damaged file starts fresh), or the file isn't persisted.
    pub fn load(&self) -> Option<T> {
        let path = self.path.as_ref()?;
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write what `snapshot` returns in the background, within
    /// `write_delay`. It is called when the write happens, so it sees the
    /// newest value; the latest `save` before then replaces it.
    pub fn save(&self, snapshot: impl FnOnce() -> T + Send + 'static)
    where
        T: 'static,
    {
        let (Some(path), Some(files)) = (self.path.clone(), self.files.clone()) else {
            return;
        };
        let encode: Snapshot = Box::new(move || Some(super::contract::to_json(&snapshot())));
        let schedules = {
            let mut state = lock(&self.shared.state);
            let sequence = state.next_sequence;
            state.next_sequence += 1;
            state.snapshot = Some((sequence, encode));
            let schedules = !std::mem::replace(&mut state.scheduled, true);
            if schedules {
                state.in_flight += 1;
            }
            schedules
        };
        if !schedules {
            return;
        }
        let shared = self.shared.clone();
        let delay = self.write_delay;
        let background = {
            let (shared, files, path) = (shared.clone(), files.clone(), path.clone());
            move || {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                write_pending(&shared, files.as_ref(), &path);
            }
        };
        let spawned = std::thread::Builder::new()
            .name("an-cloud-save".into())
            .spawn(background);
        if spawned.is_err() {
            // No thread to write with: write now rather than lose it.
            write_pending(&shared, files.as_ref(), &path);
        }
    }

    /// Write `value` now, on the calling thread (quitting, tests): nothing
    /// saved before it can land after it.
    pub fn save_now(&self, value: &T) {
        let (Some(path), Some(files)) = (self.path.as_ref(), self.files.as_ref()) else {
            return;
        };
        let sequence = {
            let mut state = lock(&self.shared.state);
            state.snapshot = None;
            let sequence = state.next_sequence;
            state.next_sequence += 1;
            sequence
        };
        let bytes = super::contract::to_json(value);
        write_if_newer(&self.shared, files.as_ref(), path, sequence, &bytes);
    }

    /// Remove the file.
    pub fn remove(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let sequence = {
            let mut state = lock(&self.shared.state);
            state.snapshot = None;
            let sequence = state.next_sequence;
            state.next_sequence += 1;
            sequence
        };
        let mut written = lock(&self.shared.written);
        *written = (*written).max(sequence);
        let _ = std::fs::remove_file(path);
    }

    /// Waits until every background write has landed (tests).
    pub fn flush(&self) {
        let mut state = lock(&self.shared.state);
        while state.in_flight > 0 {
            state = self
                .shared
                .idle
                .wait_timeout(state, Duration::from_millis(50))
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
    }
}

/// The background half of [`StateFile::save`]: take the newest value and
/// write it.
fn write_pending(shared: &Shared, files: &dyn SecureFiles, path: &Path) {
    let taken = {
        let mut state = lock(&shared.state);
        state.scheduled = false;
        state.snapshot.take()
    };
    if let Some((sequence, snapshot)) = taken {
        if let Some(bytes) = snapshot() {
            write_if_newer(shared, files, path, sequence, &bytes);
        }
    }
    lock(&shared.state).in_flight -= 1;
    shared.idle.notify_all();
}

fn write_if_newer(
    shared: &Shared,
    files: &dyn SecureFiles,
    path: &Path,
    sequence: u64,
    bytes: &[u8],
) {
    let mut written = lock(&shared.written);
    if *written >= sequence {
        return;
    }
    // A failed write keeps the previous file; the next save tries again.
    if write_private(files, path, bytes).is_ok() {
        *written = sequence;
    }
}

/// This install's secret for project keys (CL§4.1): 32 random bytes made
/// once and kept in `<support>\cloud-install-secret`, never sent. Made
/// lazily, the first time a sync pass needs a project key.
pub mod install_secret {
    use super::*;

    pub const FILE_NAME: &str = "cloud-install-secret";
    pub const LENGTH: usize = 32;

    /// 32 bytes from the system's random source.
    pub fn random() -> Option<[u8; LENGTH]> {
        random_bytes::<LENGTH>()
    }

    /// The secret in `folder`, made first when there is none. A file of the
    /// wrong size (damaged) is replaced: project keys made from now on
    /// differ from the old ones, which is better than none. `None` when the
    /// folder can't be written.
    pub fn load(folder: &Path, files: &dyn SecureFiles) -> Option<Vec<u8>> {
        let path = folder.join(FILE_NAME);
        if let Ok(saved) = std::fs::read(&path) {
            if saved.len() == LENGTH {
                return Some(saved);
            }
        }
        let made = random()?;
        if path.exists() {
            return write_private(files, &path, &made)
                .ok()
                .map(|_| made.to_vec());
        }
        if !folder.is_dir() {
            files.ensure_private_dir(folder).ok()?;
        }
        match files.create_exclusive(&path, &made) {
            Ok(true) => Some(made.to_vec()),
            // Another run made it a moment ago: use that one.
            Ok(false) => std::fs::read(&path)
                .ok()
                .filter(|saved| saved.len() == LENGTH),
            Err(_) => None,
        }
    }
}
