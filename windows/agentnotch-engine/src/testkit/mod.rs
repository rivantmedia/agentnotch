//! Fakes of every platform service, for the engine's tests and (with the
//! `testkit` feature) other crates' tests. [`platform`] builds a whole
//! `Platform` over a temporary root the test owns; the returned
//! [`TestHandles`] script the fakes and read what the engine did. Nothing
//! here touches the real profile, the network or a real process.
//!
//! Owners: `mod`, `clock`, `files` WP0; `transport` WP1; `process` WP3;
//! `runner` WP4; `terminal` WP6; `http` WP8.

pub mod clock;
pub mod files;
pub mod http;
pub mod process;
pub mod runner;
pub mod terminal;
pub mod transport;

pub use clock::FakeClock;
pub use files::*;
pub use http::{FakeDevice, FixtureHttp, RecordingBrowser};
pub use process::FakeProcesses;
pub use runner::{Script, ScriptedRunner};
pub use terminal::{FakeConsole, FakeTerminals, RecordingNotifier, RecordingSounds};
pub use transport::MemoryTransport;

use crate::platform::{Platform, Roots};
use std::path::Path;
use std::sync::Arc;

/// When every fake clock starts: 2026-09-21T14:13:20Z.
pub const TEST_START_MS: u64 = 1_790_000_000_000;

/// The fakes behind a test `Platform`.
#[derive(Clone)]
pub struct TestHandles {
    pub roots: Roots,
    pub clock: Arc<FakeClock>,
    pub processes: Arc<FakeProcesses>,
    pub runner: Arc<ScriptedRunner>,
    pub transport: Arc<MemoryTransport>,
    pub terminals: Arc<FakeTerminals>,
    pub console: Arc<FakeConsole>,
    pub notifier: Arc<RecordingNotifier>,
    pub sounds: Arc<RecordingSounds>,
    pub http: Arc<FixtureHttp>,
    pub browser: Arc<RecordingBrowser>,
    pub files: Arc<StdSecureFiles>,
    pub device: Arc<FakeDevice>,
}

/// A platform of fakes whose roots all lie under `root` (a temporary folder
/// the test made). `home` and `data` exist; `support` is left for the
/// engine to create privately.
pub fn platform(root: &Path) -> (Platform, TestHandles) {
    let roots = Roots::under(root);
    for dir in [&roots.home, &roots.data] {
        std::fs::create_dir_all(dir).expect("create a test root");
    }
    let handles = TestHandles {
        roots,
        clock: Arc::new(FakeClock::at_ms(TEST_START_MS)),
        processes: Arc::new(FakeProcesses::default()),
        runner: Arc::new(ScriptedRunner::default()),
        transport: Arc::new(MemoryTransport::default()),
        terminals: Arc::new(FakeTerminals::default()),
        console: Arc::new(FakeConsole::default()),
        notifier: Arc::new(RecordingNotifier::default()),
        sounds: Arc::new(RecordingSounds::default()),
        http: Arc::new(FixtureHttp::default()),
        browser: Arc::new(RecordingBrowser::default()),
        files: Arc::new(StdSecureFiles),
        device: Arc::new(FakeDevice),
    };
    let platform = Platform {
        clock: handles.clock.clone(),
        processes: handles.processes.clone(),
        runner: handles.runner.clone(),
        transport: handles.transport.clone(),
        terminals: handles.terminals.clone(),
        console: handles.console.clone(),
        notifier: handles.notifier.clone(),
        sounds: handles.sounds.clone(),
        http: handles.http.clone(),
        browser: handles.browser.clone(),
        files: handles.files.clone(),
        device: handles.device.clone(),
    };
    (platform, handles)
}

/// Locks a test fake's mutex, whatever a panicking test left behind.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
