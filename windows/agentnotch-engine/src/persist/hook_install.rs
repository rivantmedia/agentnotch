//! `hook-install.json` v1 (§1.5): per physical settings.json this app wrote,
//! the folder, the command form and the hook copy, so `uninstall-hooks` (and
//! the uninstaller) can clean up without discovering folders again. A
//! Windows-only file, in the Mac files' style (camelCase, ISO 8601 seconds).
//!
//! Owner after WP0: WP2.

use crate::core::time::IsoSeconds;
use crate::model::AccountId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

pub const FILE_NAME: &str = "hook-install.json";
pub const VERSION: u32 = 1;

/// Every settings.json holding this app's hooks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookInstallRecord {
    pub files: Vec<HookInstallEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookInstallEntry {
    /// Resolved.
    pub settings_path: PathBuf,
    pub folder: AccountId,
    /// `exec` | `string`.
    pub form: String,
    /// The command (string form) or the exe (exec form) written.
    pub command: String,
    /// `<cfg>\hooks\agentnotch-hook.exe`.
    pub hook_copy: Option<PathBuf>,
    /// The status line is ours (wrapped or installed).
    pub status_line: bool,
    pub installed_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookInstallFile {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub files: Vec<PersistedHookInstall>,
}

fn version() -> u32 {
    VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedHookInstall {
    #[serde(rename = "settingsPath")]
    pub settings_path: String,
    pub folder: String,
    pub form: String,
    pub command: String,
    #[serde(rename = "hookCopy", default, skip_serializing_if = "Option::is_none")]
    pub hook_copy: Option<String>,
    #[serde(rename = "statusLine", default)]
    pub status_line: bool,
    #[serde(rename = "installedAt")]
    pub installed_at: IsoSeconds,
}

impl HookInstallFile {
    pub fn parse(bytes: &[u8]) -> Option<HookInstallFile> {
        serde_json::from_slice(bytes).ok()
    }

    pub fn encode(&self) -> Vec<u8> {
        super::encode_pretty_sorted(self)
    }

    pub fn to_model(&self) -> HookInstallRecord {
        HookInstallRecord {
            files: self
                .files
                .iter()
                .map(|f| HookInstallEntry {
                    settings_path: PathBuf::from(&f.settings_path),
                    folder: AccountId(f.folder.clone()),
                    form: f.form.clone(),
                    command: f.command.clone(),
                    hook_copy: f.hook_copy.as_ref().map(PathBuf::from),
                    status_line: f.status_line,
                    installed_at: f.installed_at.0,
                })
                .collect(),
        }
    }

    pub fn from_model(record: &HookInstallRecord) -> Self {
        HookInstallFile {
            version: VERSION,
            files: record
                .files
                .iter()
                .map(|f| PersistedHookInstall {
                    settings_path: f.settings_path.to_string_lossy().into_owned(),
                    folder: f.folder.0.clone(),
                    form: f.form.clone(),
                    command: f.command.clone(),
                    hook_copy: f
                        .hook_copy
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    status_line: f.status_line,
                    installed_at: IsoSeconds(f.installed_at),
                })
                .collect(),
        }
    }
}
