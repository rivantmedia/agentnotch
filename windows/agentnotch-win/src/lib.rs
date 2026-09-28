//! Windows implementations of `agentnotch_engine::platform` (DESIGN-WIN §2.3, §3.2).
//!
//! Each service does one OS thing and returns plain data; anything decidable without the OS
//! (which window to prefer, whether a tab title matches, which folder to probe) is engine code
//! taking that data. Pure helpers that live here (SDDL strings, frame parsing, console records)
//! are plain functions tested on every OS.
//!
//! On other systems the crate compiles to [`stub`]: every service answers "not available on this
//! platform", so the fork crates build and test on macOS and Linux while the app itself (which
//! only builds on Windows) always gets the real set.
//!
//! Module owners (DESIGN-WIN §2.1): `pipe_server`, `console`, `sid`, `integrity` WP1; `files`
//! WP2; `process`, `paths` WP3; `job` WP4; `focus`, `uia`, `toast`, `sound`, `visibility` WP6;
//! `http`, `browser`, `device` WP8; `window`, `hotkey`, `clipboard`, `capture`, `shell` WP9.

use std::path::Path;
use std::sync::Arc;

use agentnotch_engine::platform::{Platform, Roots};

mod clock;
pub use clock::SystemClock;

/// The wire protocol, re-exported so the app crate reaches it through the one fork crate it
/// already depends on for platform work (its `Cargo.toml` seam names only the engine and this).
pub use agentnotch_proto as proto;

#[cfg(windows)]
mod browser;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
mod console;
#[cfg(windows)]
mod device;
#[cfg(windows)]
mod files;
#[cfg(windows)]
mod focus;
#[cfg(windows)]
pub mod hotkey;
#[cfg(windows)]
mod http;
#[cfg(windows)]
pub mod integrity;
#[cfg(windows)]
mod job;
#[cfg(windows)]
pub mod paths;
#[cfg(windows)]
mod pipe_server;
#[cfg(windows)]
mod process;
#[cfg(windows)]
pub mod shell;
#[cfg(windows)]
pub mod sid;
#[cfg(windows)]
mod sound;
#[cfg(windows)]
mod toast;
#[cfg(windows)]
mod uia;
#[cfg(windows)]
mod visibility;
#[cfg(windows)]
pub mod window;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::{capture, clipboard, hotkey, paths, shell, window};

/// The real platform services for the hub (`Hub::new`).
///
/// `roots` are the folders the glue resolved once (§1.5); `hook_exe` is the installed
/// `agentnotch-hook.exe`, which the console helper runs (`type`, `console-info`).
#[cfg(windows)]
pub fn platform(roots: &Roots, hook_exe: &Path) -> Platform {
    Platform {
        clock: Arc::new(SystemClock),
        processes: Arc::new(process::WinProcesses::new()),
        runner: Arc::new(job::JobRunner::new()),
        transport: Arc::new(pipe_server::PipeServer::new()),
        terminals: Arc::new(focus::WinTerminals::new(hook_exe)),
        console: Arc::new(console::ConsoleHelper::new(hook_exe)),
        notifier: Arc::new(toast::Toasts::new()),
        sounds: Arc::new(sound::Chimes::new()),
        http: Arc::new(http::UreqHttp::new()),
        browser: Arc::new(browser::ShellBrowser::new()),
        files: Arc::new(files::WinFiles::new()),
        device: Arc::new(device::WinDevice::new(roots)),
    }
}

/// On other systems every service is the stub: nothing on this platform is reached.
#[cfg(not(windows))]
pub fn platform(_roots: &Roots, _hook_exe: &Path) -> Platform {
    let unavailable = Arc::new(stub::Unavailable);
    Platform {
        clock: Arc::new(SystemClock),
        processes: unavailable.clone(),
        runner: unavailable.clone(),
        transport: unavailable.clone(),
        terminals: unavailable.clone(),
        console: unavailable.clone(),
        notifier: unavailable.clone(),
        sounds: unavailable.clone(),
        http: unavailable.clone(),
        browser: unavailable.clone(),
        files: unavailable.clone(),
        device: unavailable,
    }
}

/// The string SID (`S-1-5-21-…`) of the user this process runs as, which names the hook pipe
/// (`proto::pipe_name`). `None` when the token can't be read, and on other systems.
pub fn user_sid() -> Option<String> {
    #[cfg(windows)]
    {
        sid::current_user_sid()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The message every service gives until its package implements it on Windows.
#[cfg(windows)]
pub(crate) const NOT_IMPLEMENTED: &str = "not implemented in this build";
