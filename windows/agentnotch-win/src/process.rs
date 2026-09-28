//! Processes on Windows (DESIGN-WIN §3.2 `Processes`, §4.2; WP3): Toolhelp32 snapshots with
//! creation times, liveness (`OpenProcess`; ACCESS_DENIED counts as alive), `GetProcessTimes`,
//! the PEB read of `CLAUDE_CONFIG_DIR` (same user only; a partial read is `Unreadable`, never
//! `Unset`), token user and elevation, `QueryFullProcessImageNameW`.
//!
//! Not implemented in this build: every answer is "unknown", which the engine already handles
//! (sessions keyed by id, no liveness checks, attribution from the hooks alone).

use std::path::PathBuf;
use std::time::SystemTime;

use agentnotch_engine::platform::{EnvRead, Liveness, ProcessTable, Processes};

#[derive(Debug, Default)]
pub struct WinProcesses;

impl WinProcesses {
    pub fn new() -> Self {
        WinProcesses
    }
}

impl Processes for WinProcesses {
    fn liveness(&self, _pid: u32) -> Liveness {
        Liveness::Unknown
    }
    fn start_time(&self, _pid: u32) -> Option<SystemTime> {
        None
    }
    fn table(&self) -> ProcessTable {
        ProcessTable {
            entries: Vec::new(),
        }
    }
    fn config_dir_env(&self, _pid: u32) -> EnvRead {
        EnvRead::Unreadable
    }
    fn same_user(&self, _pid: u32) -> Option<bool> {
        None
    }
    fn elevated(&self, _pid: u32) -> Option<bool> {
        None
    }
    fn exe_path(&self, _pid: u32) -> Option<PathBuf> {
        None
    }
}
