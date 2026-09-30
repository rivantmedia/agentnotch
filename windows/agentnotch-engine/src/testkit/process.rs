//! A scripted process table.
//!
//! Owner after WP0: WP3.

use super::lock;
use crate::platform::{EnvRead, Liveness, ProcEntry, ProcessTable, Processes};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Default)]
pub struct FakeProcesses {
    table: Mutex<ProcessTable>,
    env: Mutex<HashMap<u32, EnvRead>>,
    elevated: Mutex<HashMap<u32, bool>>,
    exe_paths: Mutex<HashMap<u32, PathBuf>>,
    liveness: Mutex<HashMap<u32, Liveness>>,
    other_users: Mutex<HashSet<u32>>,
}

impl FakeProcesses {
    /// Adds (or replaces) a running process.
    pub fn add(&self, pid: u32, ppid: u32, exe_name: &str, started: SystemTime) {
        let mut table = lock(&self.table);
        table.entries.retain(|e| e.pid != pid);
        table.entries.push(ProcEntry {
            pid,
            ppid,
            exe_name: exe_name.to_owned(),
            started: Some(started),
        });
    }

    /// The process ended.
    pub fn remove(&self, pid: u32) {
        lock(&self.table).entries.retain(|e| e.pid != pid);
    }

    pub fn set_config_dir_env(&self, pid: u32, read: EnvRead) {
        lock(&self.env).insert(pid, read);
    }

    pub fn set_elevated(&self, pid: u32, elevated: bool) {
        lock(&self.elevated).insert(pid, elevated);
    }

    pub fn set_exe_path(&self, pid: u32, path: PathBuf) {
        lock(&self.exe_paths).insert(pid, path);
    }

    /// Scripts the answer for `pid` whatever the table says: `Unknown` is a
    /// process that could not be asked (no access), the case a caller must
    /// not read as "gone". `clear_liveness` goes back to the table.
    pub fn set_liveness(&self, pid: u32, liveness: Liveness) {
        lock(&self.liveness).insert(pid, liveness);
    }

    pub fn clear_liveness(&self, pid: u32) {
        lock(&self.liveness).remove(&pid);
    }

    /// `pid` runs as another user: it is alive, but `same_user` says no and
    /// its environment is not readable.
    pub fn set_other_user(&self, pid: u32, other: bool) {
        let mut users = lock(&self.other_users);
        if other {
            users.insert(pid);
        } else {
            users.remove(&pid);
        }
    }
}

impl Processes for FakeProcesses {
    fn liveness(&self, pid: u32) -> Liveness {
        if let Some(scripted) = lock(&self.liveness).get(&pid) {
            return *scripted;
        }
        if lock(&self.table).get(pid).is_some() {
            Liveness::Alive
        } else {
            Liveness::Gone
        }
    }

    fn start_time(&self, pid: u32) -> Option<SystemTime> {
        lock(&self.table).get(pid).and_then(|e| e.started)
    }

    fn table(&self) -> ProcessTable {
        lock(&self.table).clone()
    }

    fn config_dir_env(&self, pid: u32) -> EnvRead {
        // Another user's environment is never readable.
        if lock(&self.other_users).contains(&pid) {
            return EnvRead::Unreadable;
        }
        lock(&self.env)
            .get(&pid)
            .cloned()
            .unwrap_or(EnvRead::Unreadable)
    }

    fn same_user(&self, pid: u32) -> Option<bool> {
        if lock(&self.other_users).contains(&pid) {
            return Some(false);
        }
        lock(&self.table).get(pid).map(|_| true)
    }

    fn elevated(&self, pid: u32) -> Option<bool> {
        lock(&self.elevated)
            .get(&pid)
            .copied()
            .or_else(|| lock(&self.table).get(pid).map(|_| false))
    }

    fn exe_path(&self, pid: u32) -> Option<PathBuf> {
        lock(&self.exe_paths).get(&pid).cloned()
    }
}
