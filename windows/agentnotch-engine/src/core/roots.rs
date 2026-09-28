//! Names of the fork's folders and helpers on [`Roots`] (the struct itself is
//! in `platform`, §3.2). The glue resolves the real folders once (§1.5);
//! tests build them under a temporary folder.

use crate::core::paths::{PathStyle, Paths};
use crate::platform::Roots;
use std::path::{Path, PathBuf};

/// The Tauri identifier: single-instance mutex, WebView2 folder, AUMID.
pub const IDENTIFIER: &str = "com.rivantmedia.agentnotch";
/// `%APPDATA%\<this>`: upstream's config and logs.
pub const DATA_FOLDER: &str = "Agent Notch";
/// The same when sealed, so a sealed run never edits the real `config.json`.
pub const DATA_FOLDER_SEALED: &str = "Agent Notch Sealed";
/// `%LOCALAPPDATA%\<IDENTIFIER>\<this>` is `<support>`.
pub const SUPPORT_CHILD: &str = "Claude";
/// The probe's working folder inside `<support>`.
pub const USAGE_PROBE_DIR: &str = "usage-probe";
/// The summarizer's working folder inside `<support>`.
pub const SESSION_SUMMARY_DIR: &str = "session-summary";

impl Roots {
    /// Every root under `base` (a temporary folder): `home`, `data`,
    /// `support`, one Claude Desktop root, `Users`, `install`. Nothing is
    /// created.
    pub fn under(base: &Path) -> Roots {
        Roots {
            home: base.join("home"),
            data: base.join("data"),
            support: base.join("support"),
            claude_desktop: vec![base.join("desktop").join("Claude")],
            system_users: Some(base.join("Users")),
            install_dir: Some(base.join("install")),
        }
    }

    /// Path rules for this home, in the native style.
    pub fn paths(&self) -> Paths {
        Paths::native(&self.home)
    }

    /// Path rules for this home, in `style` (tests of the other OS's rules).
    pub fn paths_in(&self, style: PathStyle) -> Paths {
        Paths::new(style, &self.home.to_string_lossy())
    }

    /// A file of the engine's own in `<support>`.
    pub fn support_file(&self, name: &str) -> PathBuf {
        self.support.join(name)
    }

    /// `~\.claude`.
    pub fn default_config_dir(&self) -> PathBuf {
        self.home.join(".claude")
    }

    /// `~\.claude.json`.
    pub fn default_identity_file(&self) -> PathBuf {
        self.home.join(".claude.json")
    }

    pub fn usage_probe_dir(&self) -> PathBuf {
        self.support.join(USAGE_PROBE_DIR)
    }

    pub fn session_summary_dir(&self) -> PathBuf {
        self.support.join(SESSION_SUMMARY_DIR)
    }
}

/// `Agent Notch`, or `Agent Notch Sealed` when sealed.
pub fn data_folder_name(sealed: bool) -> &'static str {
    if sealed {
        DATA_FOLDER_SEALED
    } else {
        DATA_FOLDER
    }
}
