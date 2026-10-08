//! Shared by the hub suites (`tests/hub_*.rs`): fixed times, accounts,
//! sessions in each of the five states, a fake account directory and the
//! projection input built from them. Nothing here touches the disk.

// Each suite uses its own part of this.
#![allow(dead_code)]

use agentnotch_engine::accounts::AccountRegistry;
use agentnotch_engine::attention::rows::ResetClock;
use agentnotch_engine::core::settings::ControlSettings;
use agentnotch_engine::hooks::HookManager;
use agentnotch_engine::hub::project::{Directory, ProjectionInput, RowExtras};
use agentnotch_engine::hub::project_settings::{
    settings_snapshot, setup_state, SettingsInput, SetupInput,
};
use agentnotch_engine::model::{
    Account, AccountId, Attribution, CloudState, DesktopCacheFormat, HubSnapshot, IdentityId,
    NeedsInputReason, PermissionContext, Phase, RingId, SessionView, SettingsSnapshot, SetupState,
    UiSettings,
};
use agentnotch_engine::platform::NotifyPermission;
use agentnotch_engine::runtime_types::{RingReading, VersionSighting};
use agentnotch_engine::sessions::session::Session;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 2027-01-15T08:00:00Z: the Swift tests' fixed "now".
pub fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

pub fn now_ms() -> u64 {
    1_800_000_000_000
}

/// `offset` seconds from now (negative: before).
pub fn at(offset: i64) -> SystemTime {
    if offset < 0 {
        now() - Duration::from_secs(offset.unsigned_abs())
    } else {
        now() + Duration::from_secs(offset as u64)
    }
}

/// One account, tracked and with its ring shown.
pub fn account(identity: &str, ring: &str, label: &str, default: bool) -> Account {
    Account {
        identity_id: IdentityId::from(identity),
        ring_id: RingId::from(ring),
        label: label.to_owned(),
        own_label: Some(label.to_owned()),
        monogram: label.chars().take(2).collect::<String>().to_uppercase(),
        color_index: 0,
        email: None,
        plan_name: None,
        organization_uuid: None,
        run_dirs: Vec::new(),
        store_dirs: Vec::new(),
        includes_default: default,
        is_tracked: true,
        ring_shown: true,
        is_signed_in: true,
        launch_command: None,
        can_forget: false,
    }
}

/// What a session is doing.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Permission,
    Question,
    /// A failed turn (rate limit).
    Failed,
    /// Finished `secs` seconds before now, not reviewed.
    Review(u64),
    Working,
    Idle,
}

/// A session of `kind`, running in `folder`, as the store would hand it out
/// (no ring yet: the projection places it).
pub fn session(id: &str, folder: &str, kind: Kind) -> SessionView {
    let mut s = Session::new(id, format!("/work/{id}"), at(-600));
    s.account = Some(AccountId::from(folder));
    match kind {
        Kind::Permission => {
            s.phase = Phase::WaitingForApproval(PermissionContext {
                tool_use_id: format!("toolu_{id}"),
                tool_name: "Bash".into(),
                tool_input: Value::Null,
                received_at: at(-120),
                permission_suggestions: vec![],
                has_synthetic_tool_use_id: false,
                agent_id: None,
                activated_at: Some(at(-120)),
            });
            s.set_needs_input(
                Some(NeedsInputReason::Permission {
                    tool: Some("Bash".into()),
                }),
                at(-120),
            );
        }
        Kind::Question => {
            s.phase = Phase::WaitingForInput;
            s.set_needs_input(Some(NeedsInputReason::Question), at(-120));
        }
        Kind::Failed => {
            s.set_needs_input(
                Some(NeedsInputReason::Error {
                    text: "Rate limited".into(),
                    code: Some("rate_limit".into()),
                }),
                at(-120),
            );
        }
        Kind::Review(secs) => {
            s.completed_at = Some(now() - Duration::from_secs(secs));
        }
        Kind::Working => {
            s.phase = Phase::Processing;
            s.turn_started_at = Some(at(-60));
        }
        Kind::Idle => {}
    }
    s.to_view()
}

/// The same, with the attribution the hub gave it.
pub fn attributed(mut view: SessionView, attribution: Attribution) -> SessionView {
    view.attribution = attribution;
    view
}

/// A session already on `ring` (for the pure helpers that take placed views).
pub fn on_ring(mut view: SessionView, ring: &str) -> SessionView {
    view.ring = Some(RingId::from(ring));
    view
}

/// An account directory written out by hand.
#[derive(Default)]
pub struct FakeDirectory {
    pub default_folder: String,
    pub identities: BTreeMap<String, String>,
    pub rings: BTreeMap<String, String>,
    pub forgotten: BTreeSet<String>,
    pub untracked: BTreeSet<String>,
}

impl Directory for FakeDirectory {
    fn default_folder(&self) -> AccountId {
        AccountId::from(self.default_folder.as_str())
    }
    fn identity_of_folder(&self, folder: &AccountId) -> Option<IdentityId> {
        self.identities
            .get(folder.as_str())
            .map(|id| IdentityId::from(id.as_str()))
    }
    fn is_forgotten(&self, id: &str) -> bool {
        self.forgotten.contains(id)
    }
    fn is_untracked(&self, folder: &AccountId, _: Option<SystemTime>) -> bool {
        self.untracked.contains(folder.as_str())
    }
    fn ring_of_folder(&self, folder: &AccountId) -> Option<RingId> {
        self.rings
            .get(folder.as_str())
            .map(|ring| RingId::from(ring.as_str()))
    }
}

pub fn ui() -> UiSettings {
    ControlSettings::default().ui()
}

pub fn setup() -> SetupState {
    SetupState {
        hook_consent: Some(true),
        needs_hook_consent: false,
        consent_files: vec![],
        codenotch_hooks_folders: vec![],
        new_install_folders: vec![],
        transport_error: None,
        control_off: false,
        missing_hooks_accounts: vec![],
        install_disabled: false,
    }
}

/// Everything one projection reads, owned, so a test can change one part.
pub struct World {
    pub accounts: Vec<Account>,
    pub directory: FakeDirectory,
    pub readings: BTreeMap<IdentityId, RingReading>,
    pub sessions: Vec<SessionView>,
    pub ui: UiSettings,
    pub setup: SetupState,
    pub sealed: bool,
    pub extras: BTreeMap<String, RowExtras>,
}

impl World {
    pub fn new(accounts: Vec<Account>) -> World {
        World {
            accounts,
            directory: FakeDirectory::default(),
            readings: BTreeMap::new(),
            sessions: Vec::new(),
            ui: ui(),
            setup: setup(),
            sealed: false,
            extras: BTreeMap::new(),
        }
    }

    pub fn project_at(&self, now: SystemTime, generated_at_ms: u64) -> HubSnapshot {
        let extras = |view: &SessionView| {
            self.extras
                .get(view.id.as_str())
                .cloned()
                .unwrap_or_default()
        };
        agentnotch_engine::hub::project::project(
            &ProjectionInput {
                now,
                accounts: &self.accounts,
                directory: &self.directory,
                readings: &self.readings,
                sessions: &self.sessions,
                extras: &extras,
                clock: ResetClock::default(),
                ui: &self.ui,
                setup: &self.setup,
                sealed: self.sealed,
            },
            generated_at_ms,
        )
    }

    pub fn project(&self) -> HubSnapshot {
        self.project_at(now(), now_ms())
    }
}

/// Everything one settings projection reads, owned, so a test can change one
/// part (the registry and the hook manager are the real stores).
pub struct SettingsWorld {
    pub registry: AccountRegistry,
    pub hooks: HookManager,
    pub settings: ControlSettings,
    pub readings: BTreeMap<IdentityId, RingReading>,
    pub versions: Vec<VersionSighting>,
    pub window_names: BTreeMap<String, String>,
    pub changed: Vec<AccountId>,
    pub pipe_name: String,
    pub busy: bool,
    pub refreshing: bool,
    pub desktop_format: Option<DesktopCacheFormat>,
    pub notify: NotifyPermission,
    pub hotkey_ok: bool,
    pub hotkey_message: Option<String>,
    pub cloud: CloudState,
    pub session_count: u32,
    pub review_count: u32,
    pub transport_error: Option<String>,
    pub sealed: bool,
}

impl SettingsWorld {
    pub fn new(registry: AccountRegistry, hooks: HookManager) -> SettingsWorld {
        SettingsWorld {
            registry,
            hooks,
            settings: ControlSettings::default(),
            readings: BTreeMap::new(),
            versions: Vec::new(),
            window_names: BTreeMap::new(),
            changed: Vec::new(),
            pipe_name: r"\\.\pipe\agentnotch-hook-test".to_owned(),
            busy: false,
            refreshing: false,
            desktop_format: None,
            notify: NotifyPermission::Allowed,
            hotkey_ok: true,
            hotkey_message: None,
            cloud: CloudState::default(),
            session_count: 0,
            review_count: 0,
            transport_error: None,
            sealed: false,
        }
    }

    pub fn setup_input(&self) -> SetupInput<'_> {
        SetupInput {
            registry: &self.registry,
            hooks: &self.hooks,
            settings: &self.settings,
            window_names: &self.window_names,
            transport_error: self.transport_error.as_deref(),
            sealed: self.sealed,
        }
    }

    pub fn setup(&self) -> SetupState {
        setup_state(&self.setup_input())
    }

    pub fn snapshot(&self) -> SettingsSnapshot {
        self.snapshot_at(now())
    }

    pub fn snapshot_at(&self, now: SystemTime) -> SettingsSnapshot {
        settings_snapshot(&SettingsInput {
            now,
            setup: self.setup_input(),
            readings: &self.readings,
            versions: &self.versions,
            changed_folders: &self.changed,
            pipe_name: &self.pipe_name,
            busy: self.busy,
            refreshing: self.refreshing,
            desktop_format: self.desktop_format,
            notify_permission: self.notify,
            hotkey_ok: self.hotkey_ok,
            hotkey_message: self.hotkey_message.as_deref(),
            cloud: &self.cloud,
            session_count: self.session_count,
            review_count: self.review_count,
        })
    }
}

/// A live hub on the test platform (`testkit::platform`): its files under a
/// temporary root, its events recorded, its settings writes counted.
pub mod live {
    use agentnotch_engine::core::atomic::StdSecureFiles;
    use agentnotch_engine::core::flags::DevFlags;
    use agentnotch_engine::hub::runtime::{live_hub, HubInputs, RuntimeOptions};
    use agentnotch_engine::hub::{Call, Hub, HubConfig, HubEvent};
    use agentnotch_engine::platform::{
        Expect, FileIdentity, Platform, Roots, SecureFiles, WriteMode, WriteResult,
    };
    use agentnotch_engine::testkit::{self, TestHandles};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// One `write_atomic`: the file and the thread that wrote it.
    #[derive(Debug, Clone)]
    pub struct Write {
        pub path: PathBuf,
        pub thread: String,
    }

    /// The real file service, recording every write; the first
    /// `panics` writes panic instead (a job body that panics), and a
    /// worker's (`an-io-*`) write of the file named in `refused` fails.
    #[derive(Default)]
    pub struct RecordingFiles {
        pub writes: Mutex<Vec<Write>>,
        pub panics: AtomicUsize,
        pub refused: Mutex<Option<String>>,
    }

    impl RecordingFiles {
        pub fn writes_of(&self, name: &str) -> Vec<Write> {
            lock(&self.writes)
                .iter()
                .filter(|w| w.path.file_name().is_some_and(|n| n == name))
                .cloned()
                .collect()
        }
    }

    impl SecureFiles for RecordingFiles {
        fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
            StdSecureFiles.ensure_private_dir(dir)
        }

        fn write_atomic(
            &self,
            path: &Path,
            bytes: &[u8],
            mode: WriteMode,
            expect: Expect,
        ) -> io::Result<WriteResult> {
            let panics = self.panics.load(Ordering::SeqCst);
            if panics > 0 {
                self.panics.store(panics - 1, Ordering::SeqCst);
                panic!("a disk that panics");
            }
            let on_worker = std::thread::current()
                .name()
                .is_some_and(|name| name.starts_with("an-io-"));
            if on_worker
                && lock(&self.refused)
                    .as_deref()
                    .is_some_and(|name| path.file_name().is_some_and(|n| n == name))
            {
                return Err(io::Error::other("a disk that refuses"));
            }
            lock(&self.writes).push(Write {
                path: path.to_path_buf(),
                thread: std::thread::current().name().unwrap_or("").to_owned(),
            });
            StdSecureFiles.write_atomic(path, bytes, mode, expect)
        }

        fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
            StdSecureFiles.create_exclusive(path, bytes)
        }

        fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
            StdSecureFiles.identity(path)
        }

        fn is_reparse(&self, path: &Path) -> io::Result<bool> {
            StdSecureFiles.is_reparse(path)
        }

        fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
            StdSecureFiles.canonical(path)
        }

        fn is_private(&self, path: &Path) -> io::Result<bool> {
            StdSecureFiles.is_private(path)
        }
    }

    pub struct TestHub {
        pub hub: Hub,
        pub inputs: HubInputs,
        pub handles: TestHandles,
        pub files: Arc<RecordingFiles>,
        pub events: Arc<Mutex<Vec<(Instant, HubEvent)>>>,
        pub roots: Roots,
        _dir: Option<tempfile::TempDir>,
    }

    pub fn config(roots: &Roots) -> HubConfig {
        HubConfig {
            roots: roots.clone(),
            app_version: "1.1.0".into(),
            website: None,
            flags: DevFlags::default(),
            hook_exe: roots
                .install_dir
                .clone()
                .unwrap_or_default()
                .join("agentnotch-hook.exe"),
            pipe_name: r"\\.\pipe\agentnotch-hook-test".into(),
        }
    }

    impl TestHub {
        /// Not started; the runtime's own timings.
        pub fn new() -> TestHub {
            TestHub::with(RuntimeOptions::default(), |_| {})
        }

        /// Not started; `customise` may replace platform services.
        pub fn with(options: RuntimeOptions, customise: impl FnOnce(&mut Platform)) -> TestHub {
            TestHub::with_config(options, |_| {}, customise)
        }

        pub fn with_config(
            options: RuntimeOptions,
            configure: impl FnOnce(&mut HubConfig),
            customise: impl FnOnce(&mut Platform),
        ) -> TestHub {
            let dir = tempfile::tempdir().expect("a temporary root");
            let mut hub = TestHub::over(dir.path(), options, configure, customise);
            hub._dir = Some(dir);
            hub
        }

        /// A hub over a root the test made (and keeps): its home is
        /// `<base>\home`, as `Roots::under` lays it out.
        pub fn over(
            base: &Path,
            options: RuntimeOptions,
            configure: impl FnOnce(&mut HubConfig),
            customise: impl FnOnce(&mut Platform),
        ) -> TestHub {
            let (mut platform, handles) = testkit::platform(base);
            let files = Arc::new(RecordingFiles::default());
            platform.files = files.clone();
            customise(&mut platform);
            let roots = handles.roots.clone();
            let mut cfg = config(&roots);
            configure(&mut cfg);
            let (hub, inputs) = live_hub(cfg, platform, options);
            let events: Arc<Mutex<Vec<(Instant, HubEvent)>>> = Arc::default();
            let (sink, clock) = (events.clone(), handles.clock.clone());
            hub.on_event(Box::new(move |event| {
                use agentnotch_engine::platform::Clock;
                lock(&sink).push((clock.monotonic(), event.clone()));
            }));
            TestHub {
                hub,
                inputs,
                handles,
                files,
                events,
                roots,
                _dir: None,
            }
        }

        pub fn started() -> TestHub {
            let hub = TestHub::new();
            hub.hub.start().expect("the hub starts");
            hub
        }

        pub fn settings_path(&self) -> PathBuf {
            self.roots.support.join("control-settings.json")
        }

        /// The settings file as JSON (`Null` while it doesn't exist).
        pub fn settings_file(&self) -> serde_json::Value {
            std::fs::read(self.settings_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null)
        }

        /// Returns once `an-core` has applied everything sent before (a
        /// call is answered in order).
        pub fn sync(&self) {
            self.hub.call(Call::Settings).expect("the hub answers");
        }

        pub fn events(&self) -> Vec<(Instant, HubEvent)> {
            lock(&self.events).clone()
        }

        pub fn snapshots(&self) -> Vec<(Instant, agentnotch_engine::model::HubSnapshot)> {
            self.events()
                .into_iter()
                .filter_map(|(at, e)| match e {
                    HubEvent::Snapshot(s) => Some((at, s)),
                    _ => None,
                })
                .collect()
        }

        pub fn settings_events(&self) -> usize {
            self.events()
                .iter()
                .filter(|(_, e)| matches!(e, HubEvent::Settings(_)))
                .count()
        }

        pub fn logs(&self) -> Vec<String> {
            self.events()
                .into_iter()
                .filter_map(|(_, e)| match e {
                    HubEvent::Log(line) => Some(line),
                    _ => None,
                })
                .collect()
        }
    }

    impl Drop for TestHub {
        fn drop(&mut self) {
            self.hub.stop();
        }
    }

    /// Polls `condition` every 5 ms for at most 5 s (threads at work; no
    /// test waits on wall time otherwise).
    pub fn eventually(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|p| p.into_inner())
    }
}
