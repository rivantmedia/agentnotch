//! Opening the website sign-in in the default browser (DESIGN-WIN §3.2 `Browser`, §4.11; WP8):
//! `ShellExecuteW("open")`; with `AGENTNOTCH_DEV=1` and `AGENTNOTCH_DEV_BROWSER_LOG` (never sealed)
//! the URL is appended to that file instead, for the smoke test's sign-in.
//!
//! What may be opened and when the log applies are the engine's rules
//! (`agentnotch_engine::cloud::browser`); this file only does the OS part.

#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use agentnotch_engine::cloud::browser::{browser_log_target, may_open};
use agentnotch_engine::core::sealed;
use agentnotch_engine::platform::Browser;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// What the user reads when the shell couldn't hand the page to a browser.
pub const OPEN_FAILED: &str = "The sign-in page couldn't open in your browser.";

#[derive(Debug, Default)]
pub struct ShellBrowser {
    /// Read once at start, like every other switch: a sealed run never opens anything.
    sealed: bool,
    /// The dev run's URL log, when its switches allow one.
    log: Option<PathBuf>,
}

impl ShellBrowser {
    pub fn new() -> Self {
        let get_env = |key: &str| std::env::var_os(key).map(|v| v.to_string_lossy().into_owned());
        ShellBrowser {
            sealed: sealed::is_sealed(get_env),
            log: browser_log_target(get_env),
        }
    }
}

impl Browser for ShellBrowser {
    fn open(&self, url: &str) -> Result<(), String> {
        // Only https (or http to this PC): the shell would otherwise launch whatever handler a
        // path or scheme names. A NUL would cut the string the shell sees short of the one checked.
        if self.sealed || url.contains('\0') || !may_open(url) {
            return Err(OPEN_FAILED.to_owned());
        }
        if let Some(log) = &self.log {
            return append_line(log, url).map_err(|e| format!("{OPEN_FAILED} ({e})"));
        }
        shell_open(url)
    }
}

fn append_line(log: &PathBuf, url: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    file.write_all(format!("{url}\n").as_bytes())
}

fn shell_open(url: &str) -> Result<(), String> {
    // The shell may hand the URL to a COM-based handler, which wants an initialised apartment on
    // this thread (Microsoft's guidance for ShellExecute). A thread already in another mode keeps
    // it; only a call that initialised COM here is paired with an uninitialise.
    // SAFETY: no reserved pointer; the flags are valid COINIT values.
    let com = unsafe {
        CoInitializeEx(
            None,
            COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0),
        )
    };
    let target = HSTRING::from(url);
    // SAFETY: both strings are NUL-terminated and outlive the call; no window, parameters or
    // directory.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &target,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if com.is_ok() {
        // SAFETY: pairs the successful CoInitializeEx above on this same thread.
        unsafe { CoUninitialize() };
    }
    // ShellExecuteW reports success as a value above 32; anything else is an error code.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(OPEN_FAILED.to_owned())
    }
}
