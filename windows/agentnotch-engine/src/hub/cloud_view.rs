//! What the cloud thread reads of the running engine (design §3.4
//! `CloudDeps`; the Mac's `LiveCloudEnvironment`): an immutable
//! [`CloudView`] that `an-core` republishes after every projection, behind
//! a mutex the cloud thread only ever holds to clone an `Arc`, so a call
//! from `an-cloud` never waits on `an-core`.
//!
//! - The accounts, folder logins and backfill folders are the registry's
//!   own answers (`accounts::for_cloud`), taken when the view is made.
//! - A summary's folder is chosen as the usage probe's is (one of the
//!   account's run folders, never a Claude Parallel Profiles store) and its
//!   `.claude.json` is read here, on the cloud thread, right before and
//!   again right after the run: still signed in as that account.
//! - `claude_binary()` is asked twice per publish and on every tick: the
//!   search (a few file checks) is kept until what it searches with changes,
//!   or for a minute (an install or removal is noticed then).
//! - [`running_batch`]: what the ledger is fed after every projection.
//! - `set_setting` becomes `Input::SetSetting`: `an-core` is the settings'
//!   one writer, and republishes the cloud's config from what it wrote.
//!
//! Owner: WP7.

use crate::cloud::{feed, keys};
use crate::core::paths::Paths;
use crate::model::{
    Account, BackfillFolder, ExpectedLogin, FolderKind, IdentityId, Phase, RunFolder, SessionView,
    PLACEMENT_GRACE,
};
use crate::platform::Roots;
use crate::runtime_types::{ClaudeBinary, CloudAccount, CloudDeps, Input, LiveBatch};
use crate::usage::planner::{
    expected_login, folder_runs, is_default_folder, probe_folder_among, STORE_MARKER,
};
use crossbeam_channel::Sender;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// How long a found (or missing) `claude` is believed without a new search.
const BINARY_KEPT_FOR: Duration = Duration::from_secs(60);

/// Where `claude` is looked for: the usage probe's own search inputs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BinarySearch {
    pub(crate) roots: Option<Roots>,
    /// "Claude Code location" in Settings.
    pub(crate) choice: Option<PathBuf>,
    /// The file the last working probe ran.
    pub(crate) remembered: Option<PathBuf>,
    pub(crate) env_path: OsString,
}

/// One account a summary may run for: its run folders and the login they
/// must show.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SummaryAccount {
    pub(crate) run_dirs: Vec<RunFolder>,
    pub(crate) expected: ExpectedLogin,
}

/// The engine as the cloud reads it, as of one projection.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct CloudView {
    pub(crate) accounts: Vec<CloudAccount>,
    pub(crate) folder_logins: Option<BTreeMap<String, String>>,
    pub(crate) backfill_folders: Vec<BackfillFolder>,
    /// Each account's 5-hour use now (0 once its window reset).
    pub(crate) five_hour: BTreeMap<IdentityId, f64>,
    pub(crate) summary: BTreeMap<IdentityId, SummaryAccount>,
    pub(crate) mirrors_default: bool,
    pub(crate) paths: Option<Paths>,
    /// A usage probe is running Claude Code now.
    pub(crate) launching: bool,
    pub(crate) binary: BinarySearch,
}

impl CloudView {
    /// The summary inputs of `accounts` over the registry's `folders`.
    pub(crate) fn summary_accounts(
        accounts: &[Account],
        folders: &[RunFolder],
    ) -> BTreeMap<IdentityId, SummaryAccount> {
        accounts
            .iter()
            .filter(|account| account.is_tracked && account.is_signed_in)
            .map(|account| {
                let run_dirs = account
                    .run_dirs
                    .iter()
                    .filter_map(|id| folders.iter().find(|f| f.id == *id).cloned())
                    .collect();
                (
                    account.identity_id.clone(),
                    SummaryAccount {
                        run_dirs,
                        expected: expected_login(account),
                    },
                )
            })
            .collect()
    }
}

/// The running sessions as the ledger hears of them (the Mac's
/// `feedCloud` from `recompute`): every session that hasn't ended, each
/// placed by `cloud::feed::live_batch` (certain with the key of an account
/// the website may hear of, `accounts`: hidden and forgotten ones aren't
/// there, so their sessions are counted for nobody; unsure; or waiting for
/// at most [`PLACEMENT_GRACE`]). Also when the first session waiting now
/// runs out of its grace, when the batch must be made again. Pure.
pub fn running_batch(
    views: &[SessionView],
    accounts: &[CloudAccount],
    now: SystemTime,
) -> (LiveBatch, Option<SystemTime>) {
    let keys: BTreeMap<&IdentityId, String> = accounts
        .iter()
        .filter_map(|a| Some((&a.identity_id, keys::account_key_of(a)?)))
        .collect();
    let running: Vec<SessionView> = views
        .iter()
        .filter(|view| view.phase != Phase::Ended)
        .cloned()
        .collect();
    let batch = feed::live_batch(&running, now, &|identity| keys.get(identity).cloned());
    let due = running
        .iter()
        .filter(|view| feed::placement(view, now) == feed::Placement::Waiting)
        .map(|view| view.attribution_since + PLACEMENT_GRACE)
        .min();
    (batch, due)
}

/// [`CloudDeps`] over the view `an-core` last published.
pub(crate) struct LiveCloudDeps {
    view: Mutex<Arc<CloudView>>,
    binary: Mutex<Option<(BinarySearch, Instant, Option<ClaudeBinary>)>>,
    inputs: Sender<Input>,
}

impl LiveCloudDeps {
    pub(crate) fn new(view: CloudView, inputs: Sender<Input>) -> LiveCloudDeps {
        LiveCloudDeps {
            view: Mutex::new(Arc::new(view)),
            binary: Mutex::new(None),
            inputs,
        }
    }

    /// `an-core` made a new projection.
    pub(crate) fn publish(&self, view: CloudView) {
        let mut current = lock(&self.view);
        if **current != view {
            *current = Arc::new(view);
        }
    }

    fn view(&self) -> Arc<CloudView> {
        lock(&self.view).clone()
    }

    /// Whether `folder`'s `.claude.json` names the account's login now.
    fn runs_as(view: &CloudView, folder: &RunFolder, account: &SummaryAccount) -> bool {
        let Some(paths) = &view.paths else {
            return false;
        };
        let file = paths.global_config_file(
            &folder.config_dir.to_string_lossy(),
            folder.config_dir_env.as_deref(),
        );
        let login = crate::usage::claude_json::reader()
            .read(Path::new(&file))
            .and_then(|config| config.identity);
        folder_runs(login.as_ref(), &account.expected)
    }
}

impl CloudDeps for LiveCloudDeps {
    fn accounts(&self) -> Vec<CloudAccount> {
        self.view().accounts.clone()
    }

    fn folder_logins(&self) -> Option<BTreeMap<String, String>> {
        self.view().folder_logins.clone()
    }

    fn backfill_folders(&self) -> Vec<BackfillFolder> {
        self.view().backfill_folders.clone()
    }

    fn five_hour(&self, identity: &IdentityId) -> Option<f64> {
        self.view().five_hour.get(identity).copied()
    }

    /// The probe's folder rules: the account's most recently active run
    /// folder (a session seen there, or its `.claude.json` written; never
    /// a `~\.claude.json` a mirror rewrites), never a store, and signed in
    /// as the account right now.
    fn summary_folder(&self, identity: &IdentityId) -> Option<RunFolder> {
        let view = self.view();
        let account = view.summary.get(identity)?;
        let paths = view.paths.as_ref()?;
        let activity: Vec<Option<SystemTime>> = account
            .run_dirs
            .iter()
            .map(|folder| {
                let dir = folder.config_dir.to_string_lossy();
                let mirrored = view.mirrors_default && paths.is_default_config_dir(&dir);
                let modified = (!mirrored)
                    .then(|| {
                        let file = paths.global_config_file(&dir, folder.config_dir_env.as_deref());
                        std::fs::metadata(file).and_then(|m| m.modified()).ok()
                    })
                    .flatten();
                [folder.last_seen_at, modified].into_iter().flatten().max()
            })
            .collect();
        let folder = probe_folder_among(
            &account.run_dirs,
            &activity,
            &is_default_folder,
            view.mirrors_default,
        )?;
        if folder.kind != FolderKind::Run
            || std::fs::symlink_metadata(folder.config_dir.join(STORE_MARKER)).is_ok()
            || !Self::runs_as(&view, &folder, account)
        {
            return None;
        }
        Some(folder)
    }

    fn summary_folder_still_runs(&self, folder: &RunFolder, identity: &IdentityId) -> bool {
        let view = self.view();
        view.summary
            .get(identity)
            .is_some_and(|account| Self::runs_as(&view, folder, account))
    }

    fn is_launching_claude(&self) -> bool {
        self.view().launching
    }

    fn claude_binary(&self) -> Option<ClaudeBinary> {
        let search = self.view().binary.clone();
        let mut cached = lock(&self.binary);
        if let Some((key, at, found)) = cached.as_ref() {
            if *key == search && at.elapsed() < BINARY_KEPT_FOR {
                return found.clone();
            }
        }
        let found = search.roots.as_ref().and_then(|roots| {
            crate::usage::locator::locate_claude_from(
                roots,
                search.choice.as_deref(),
                search.remembered.as_deref(),
                &search.env_path,
                &|path| path.exists(),
            )
        });
        *cached = Some((search, Instant::now(), found.clone()));
        found
    }

    fn set_setting(&self, key: &str, value: serde_json::Value) {
        // A hub that is gone has nobody to write it.
        let _ = self.inputs.send(Input::SetSetting {
            key: key.to_owned(),
            value,
        });
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
