//! What `agentnotch-hook.exe console-info --pid N` prints: the console a
//! Claude process is attached to, read by the helper (the GUI app never
//! attaches to a console itself). The engine re-exports it as
//! `platform::ConsoleInfo`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConsoleInfo {
    /// `AttachConsole(pid)` succeeded.
    #[serde(default)]
    pub attached: bool,
    /// `GetConsoleWindow()`: the conhost window or the ConPTY pseudo-window.
    #[serde(default)]
    pub window: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
    /// `GetConsoleProcessList()`, the helper itself left out.
    #[serde(default)]
    pub processes: Vec<u32>,
    /// `CONIN$` has `ENABLE_LINE_INPUT` (a shell prompt reads in line mode;
    /// Claude Code in raw mode).
    #[serde(default)]
    pub line_input: Option<bool>,
    /// Claude runs elevated and this user's helper can't reach it.
    #[serde(default)]
    pub elevated_target: bool,
    #[serde(default)]
    pub error: Option<String>,
}
