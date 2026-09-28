//! Terminals on Windows (DESIGN-WIN §3.2 `Terminals`, §4.9; WP6): host classification from the
//! process tree, console facts through `agentnotch-hook.exe console-info`, focus steps (raise a
//! window, select a Windows Terminal tab through `uia`, open an editor), the foreground window
//! and its watcher (`an-foreground`), terminal visibility (`visibility`).
//!
//! Not implemented in this build: no host is found, so rows offer no "Show terminal" and the
//! engine never believes the user is looking at a session.

use std::path::{Path, PathBuf};

use agentnotch_engine::platform::{
    ConsoleInfo, FocusOutcome, FocusStep, Foreground, HostApp, HostKind, ProcessTable, Terminals,
};

use crate::NOT_IMPLEMENTED;

#[derive(Debug)]
pub struct WinTerminals {
    /// The installed `agentnotch-hook.exe`, which reads a console for us (`console-info`).
    #[allow(dead_code)]
    helper: PathBuf,
}

impl WinTerminals {
    pub fn new(helper: &Path) -> Self {
        WinTerminals {
            helper: helper.to_path_buf(),
        }
    }
}

impl Terminals for WinTerminals {
    fn classify_host(&self, _claude_pid: u32, _table: &ProcessTable) -> HostApp {
        HostApp {
            kind: HostKind::Unknown,
            window: None,
            host_pid: None,
            exe_path: None,
        }
    }
    fn console_info(&self, _claude_pid: u32) -> ConsoleInfo {
        ConsoleInfo {
            attached: false,
            window: None,
            title: None,
            processes: Vec::new(),
            line_input: None,
            elevated_target: false,
            error: Some(NOT_IMPLEMENTED.into()),
        }
    }
    fn run_focus(&self, _step: &FocusStep) -> FocusOutcome {
        FocusOutcome::Failed(format!("Showing a terminal is {NOT_IMPLEMENTED}."))
    }
    fn foreground(&self) -> Option<Foreground> {
        None
    }
    fn window_title(&self, _window: u64) -> Option<String> {
        None
    }
    fn wt_tab_titles(&self, _window: u64) -> Option<Vec<(String, bool)>> {
        None
    }
    fn any_terminal_visible(&self) -> bool {
        false
    }
    fn watch_foreground(&self, _sink: crossbeam_channel::Sender<Foreground>) {}
}
