//! Starting the hub inside upstream's `setup` (seam WB2, DESIGN-WIN §2.4 `setup.rs`).
//!
//! First makes sure this process is the only copy of the app (`instance.rs`). Then builds the
//! hub's configuration from what this process knows (the folders, the flags, the
//! installed hook exe, the pipe name), picks the fixture hub when sealed and the real platform
//! otherwise, subscribes the event sink, starts the hub and hands it a cold-start deep link.
//!
//! Whatever goes wrong here, upstream's app keeps running: the notch, the tray and the other
//! providers don't depend on the fork's hub, so a hub that can't start (or panics while starting)
//! is logged and left out rather than taking the app down.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentnotch_engine::hub::{Hub, HubConfig};
use agentnotch_engine::platform::{Platform, Roots};
use agentnotch_win::proto;
use tauri::AppHandle;

use super::{deeplink, emit, hotkey, instance, panel_window, selftest, IDENTIFIER};

pub fn setup(app: &AppHandle) {
    // Upstream makes its config folder only when it first saves a setting, so on a first launch
    // run.log has nowhere to go yet. This launch's lines need it, and so does the hub, whose
    // `data` root it is (the sealed folder when sealed: WR-DIR).
    if let Some(folder) = crate::config::config_path().parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    // Before anything of the fork starts: a second copy the single-instance plugin let through
    // hands its launch to the first and exits here (sealed or not, §4.13).
    instance::ensure_single(app);
    // The sessions panel follows the notch from the start, whether or not the hub comes up.
    panel_window::start(app);
    match panic::catch_unwind(AssertUnwindSafe(|| start(app))) {
        Ok(Ok(())) => {
            let mode = if super::sealed() { "sealed" } else { "live" };
            super::log(&format!("hub started ({mode})"));
        }
        Ok(Err(e)) => super::log(&format!("hub not started: {e}")),
        Err(_) => super::log("hub not started: it panicked while starting"),
    }
}

fn start(app: &AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("the app's own path: {e}"))?;
    let config = hub_config(app.package_info().version.to_string(), &exe)?;
    let sealed = super::sealed();
    let hub = if sealed {
        // Fixtures only: no pipe, no Claude folder, no `<support>`, no network, no child.
        Hub::sealed(config, Arc::new(agentnotch_win::SystemClock))
    } else {
        let platform = agentnotch_win::platform(&config.roots, &config.hook_exe);
        Hub::new(config, platform)
    };
    // The sink first, so nothing the hub says while it starts is lost.
    let sink = app.clone();
    hub.on_event(Box::new(move |event| emit::forward(&sink, event)));
    hub.start()?;
    if super::HUB.set(hub.clone()).is_err() {
        // setup runs once per process; a second hub would answer nothing the first didn't.
        hub.stop();
        return Err("a hub was already running".into());
    }
    stop_at_exit(app, &hub);
    // Upstream's code reads Claude's usage from AppState: fill it before the notch's first
    // reading arrives, from the hub's launch-time discovery.
    emit::publish_usage(app, &hub.upstream_usage());
    // A link that started the app (a toast clicked while it wasn't running). Never when sealed:
    // a sealed run follows no link (§4.13).
    if !sealed {
        let args: Vec<String> = std::env::args().collect();
        if let Some(url) = deeplink::from_args(&args) {
            deeplink::handle(url.to_string());
        }
    }
    // The panel shortcut, from the settings as they are now (never when sealed).
    hotkey::start(app, &hub);
    selftest::start(app);
    Ok(())
}

/// How long the app's exit waits for the hub to stop; past this the process ends with it.
const EXIT_STOP_WAIT: Duration = Duration::from_secs(5);

/// Upstream's own Quit (the tray, the notch's menu, Settings) ends the app with `app.exit(0)`,
/// which raises `RunEvent::Exit` before `run` returns: the hub is stopped there, so it saves what
/// is still to be written (review marks, usage, settings), ends its children and lets every held
/// request go, as at every other exit. (The updater exits without that event; `update.rs` stops
/// the hub itself, and a second stop does nothing.) A plugin added at run time, because the run
/// loop is upstream's. The stop runs off the main thread, which only waits for it: the hub's
/// last events are dropped (`emit::quitting`), so none of them waits on the main thread.
fn stop_at_exit(app: &AppHandle, hub: &Hub) {
    let hub = hub.clone();
    let plugin = tauri::plugin::Builder::<tauri::Wry>::new("agentnotch-exit")
        .on_event(move |_, event| {
            if !matches!(event, tauri::RunEvent::Exit) {
                return;
            }
            emit::quitting();
            let hub = hub.clone();
            let (stopped, wait) = std::sync::mpsc::channel();
            let stopper = std::thread::Builder::new()
                .name("an-exit".into())
                .spawn(move || {
                    hub.stop();
                    let _ = stopped.send(());
                });
            if stopper.is_ok() {
                let _ = wait.recv_timeout(EXIT_STOP_WAIT);
            }
        })
        .build();
    if let Err(e) = app.plugin(plugin) {
        super::log(&format!("the hub isn't stopped at exit: {e}"));
    }
}

/// The hub's configuration for this process. `app_version` is Tauri's (= `VERSION`) when the app
/// runs, the compiled-in `VERSION` for the command line.
pub(super) fn hub_config(app_version: String, exe: &Path) -> Result<HubConfig, String> {
    let roots = roots(exe)?;
    let mut flags = super::engine::dev_flags(&roots.home);
    // One answer for the whole process: the one upstream's config folder already followed.
    // Both come from the same variables through the same rule, so this only ever seals.
    flags.sealed |= super::sealed();
    Ok(HubConfig {
        roots,
        app_version,
        website: super::website::website(),
        flags,
        hook_exe: exe.with_file_name("agentnotch-hook.exe"),
        pipe_name: pipe_name(),
    })
}

/// The folders and the real platform services, for the commands that work without a hub
/// (`inspect-accounts`, `uninstall-hooks`).
pub(super) fn roots_and_platform() -> Result<(Roots, Platform), String> {
    let exe = std::env::current_exe().map_err(|e| format!("the app's own path: {e}"))?;
    let roots = roots(&exe)?;
    let platform = agentnotch_win::platform(&roots, &exe.with_file_name("agentnotch-hook.exe"));
    Ok((roots, platform))
}

fn roots(exe: &Path) -> Result<Roots, String> {
    // `data` is wherever upstream keeps its config.json (the sealed folder when sealed, seam
    // WR-DIR), so the engine and upstream always agree on it.
    let config = crate::config::config_path();
    let data = config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    agentnotch_win::paths::roots(IDENTIFIER, data, exe.parent().map(Path::to_path_buf))
}

/// `\\.\pipe\agentnotch-hook-<SID>`, or the dev override the app always honours
/// (`AGENTNOTCH_SOCKET`; the hook honours it only with `AGENTNOTCH_DEV=1`). Empty when the SID
/// can't be read: the hub then reports that it can't listen.
fn pipe_name() -> String {
    proto::dev_pipe_override(|name| std::env::var(name).ok(), true)
        .or_else(|| agentnotch_win::user_sid().map(|sid| proto::pipe_name(&sid)))
        .unwrap_or_default()
}
