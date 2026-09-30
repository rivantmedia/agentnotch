//! The Claude accounts the app knows about (AccountRegistry.swift, AU§3).
//!
//! A folder is a Claude Code config folder (`CLAUDE_CONFIG_DIR`, `~\.claude`
//! when unset). Folders come from:
//! - discovery at launch and every five minutes, which adds only folders
//!   clearly in use: `~\.claude`, and a `~\.claude-*` or `~\.claude_*` folder
//!   that is signed in or has a live session, unless its name says it is a
//!   backup; `AGENTNOTCH_EXTRA_CONFIG_DIRS`;
//! - hook and status line events ([`AccountSighting`]), which also carry the
//!   raw `CLAUDE_CONFIG_DIR` a session runs with;
//! - the user (add an existing folder, create a new one, accept a suggestion).
//!
//! Other look-alike folders are only suggested; a forgotten account that
//! shows up again in a session is suggested too, never re-added by itself.
//! Folders are never deleted, only forgotten.
//!
//! The folders are classified and grouped by who is signed in
//! (`identities::group`): [`AccountRegistry::identities`] is the list of
//! accounts, one per identity, and name, colour and tracking are chosen per
//! identity. What the user chose is kept in `<support>\accounts.json`.
//!
//! The registry never touches the disk by itself. `an-core` drives it: a
//! [`FolderSnapshot`] read on a file lane goes into [`AccountRegistry::discover`],
//! a `.claude.json` read into [`AccountRegistry::apply_claude_json`], and what
//! is worth saving comes out of [`AccountRegistry::file_bytes`]. Only a folder
//! the user picks is looked at on the spot, through a [`FolderProbe`], because
//! the answer goes straight back to Settings.

use super::classify::{
    self, can_be_account, is_window_dir, shared_store, windows_root, FolderSuggestion, Layout,
    SuggestionReason,
};
use super::folder::Folder;
use super::identities::{
    self, next_color_index, IdentityAccount, IdentityPrefs, DIR_PREFIX, EMAIL_PREFIX, UUID_PREFIX,
};
use super::naming;
use super::snapshot::{FolderMarkers, FolderProbe, Picked, PickedFolder};
use super::timeline::{FolderAttribution, FolderIdentityTimeline};
use crate::core::paths::Paths;
use crate::core::time::IsoSeconds;
use crate::model::{
    Account, AccountAction, AccountId, AccountSighting, Attribution, ConfigRead, FolderKind,
    FolderSnapshot, FolderSource, Identity, IdentityId, RingId, RunFolder,
};
use crate::persist::accounts::{self as file, AccountsFile, PersistedFolder};
use crate::runtime_types::{AccountsChanged, ClaudeJsonRead};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the home folder is read again for new config folders.
pub const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Sightings closer together than this don't move a folder's "last seen".
pub const SIGHTING_RESOLUTION: Duration = Duration::from_secs(60);
/// How long a change waits before `accounts.json` is written (several
/// usually come together).
pub const SAVE_DELAY: Duration = Duration::from_secs(1);

/// Why an account action was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    Missing,
    NotAFolder,
    /// The home folder itself: Claude Code would treat all of home as its config.
    HomeFolder,
    /// A folder that holds the home folder or `~\.claude` (`C:\`, `C:\Users`, …).
    ContainsAccounts,
    /// Inside another account's config folder (its `projects\`, `hooks\`, …).
    InsideAccount(String),
    /// Sealed runs never touch real folders.
    UnavailableWhenSealed,
    /// Claude Parallel Profiles' shared history or windows folder.
    Infrastructure,
    InvalidName,
    CreateFailed(String),
    /// `~\.claude-<name>` exists and isn't a Claude Code folder (another tool's?).
    FolderExistsNotClaude(String),
    /// Forgetting it would forget every terminal session.
    NotForgettable,
    /// This registry was given no way to look at a folder.
    Unavailable,
}

impl AccountError {
    /// The sentence Settings shows.
    pub fn message(&self, paths: &Paths) -> String {
        let example = paths.abbreviate(&paths.join(paths.home(), ".claude-work"));
        match self {
            AccountError::Missing => "That folder doesn't exist.".to_owned(),
            AccountError::NotAFolder => "That's a file, not a folder.".to_owned(),
            AccountError::HomeFolder => format!(
                "That's your home folder, not a Claude Code config folder. Pick a folder like {example}."
            ),
            AccountError::ContainsAccounts => format!(
                "That folder contains your home folder. Pick a Claude Code config folder like {example}."
            ),
            AccountError::InsideAccount(label) => format!(
                "That folder is inside {label}'s config folder. Pick the config folder itself."
            ),
            AccountError::UnavailableWhenSealed => "Not available in the sealed demo.".to_owned(),
            AccountError::Infrastructure => "That folder is Claude Parallel Profiles' shared history (or its windows folder), not an account. Its accounts are found by themselves.".to_owned(),
            AccountError::InvalidName => {
                "Use letters, numbers, dashes or underscores for the account name.".to_owned()
            }
            AccountError::CreateFailed(reason) => {
                format!("Couldn't create the account folder: {reason}")
            }
            AccountError::FolderExistsNotClaude(folder) => format!(
                "{folder} already exists and isn't a Claude Code folder. Pick another name."
            ),
            AccountError::NotForgettable => {
                "That's the account Claude Code runs as by default, so it can't be forgotten. Turn off Track sessions and hooks instead.".to_owned()
            }
            AccountError::Unavailable => "Folders can't be checked right now.".to_owned(),
        }
    }
}

/// A folder "New account…" made (or adopted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedAccount {
    pub folder: AccountId,
    /// The line that starts Claude Code there; the user runs it, then `/login`.
    pub launch_command: String,
    pub changed: AccountsChanged,
}

/// Which ring each folder's sessions go to, and which sessions belong to an
/// untracked or forgotten account (the Mac's `FolderRings.Snapshot`): what
/// the registry published last, in a shape that can be handed to another
/// thread. Folders are looked up by `Paths::key`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderRings {
    /// Folder → its identity's ring.
    pub rings: BTreeMap<String, RingId>,
    /// Folder → the identity it names now (forgotten identities included).
    pub identity_of_folder: BTreeMap<String, String>,
    pub ring_of_identity: BTreeMap<String, RingId>,
    /// Identities that are untracked or forgotten.
    pub untracked_identities: BTreeSet<String>,
    /// Folders whose account is untracked or forgotten now.
    pub untracked_folders: BTreeSet<String>,
    /// `~\.claude`, and whether Claude Parallel Profiles mirrors into it.
    pub default_folder: Option<String>,
    pub mirrors_default: bool,
    pub default_timeline: FolderIdentityTimeline,
}

impl FolderRings {
    /// Who a session in `folder`, whose process started at `started`, runs as.
    pub fn attribution(
        &self,
        paths: &Paths,
        folder: &str,
        started: Option<SystemTime>,
    ) -> FolderAttribution {
        let key = paths.key(folder);
        let is_default = self.default_folder.as_deref() == Some(key.as_str());
        FolderAttribution::attribute(
            self.identity_of_folder.get(&key).map(String::as_str),
            is_default.then_some(&self.default_timeline),
            started,
            self.mirrors_default && is_default,
        )
    }

    /// The session's account is untracked or forgotten, when it is known;
    /// else the folder's current account decides. Nothing shows such a
    /// session, so nothing may hold its requests.
    pub fn is_untracked(&self, paths: &Paths, folder: &str, started: Option<SystemTime>) -> bool {
        match self.attribution(paths, folder, started) {
            FolderAttribution::Known(Some(identity)) => {
                self.untracked_identities.contains(&identity)
            }
            FolderAttribution::Known(None) | FolderAttribution::Unsure(_) => {
                self.untracked_folders.contains(&paths.key(folder))
            }
        }
    }

    pub fn ring_of_folder(&self, paths: &Paths, folder: &str) -> Option<&RingId> {
        self.rings.get(&paths.key(folder))
    }
}

/// What a snapshot says about one `.claude.json`.
enum Lookup {
    /// It was read: the login it holds, or `None` when the file isn't there.
    Known(Option<Box<ConfigRead>>),
    /// The read didn't cover it.
    Unknown,
}

/// Who an identity is signed in as, and whether it is tracked: what the
/// usage store and the sessions follow.
#[derive(PartialEq)]
struct LoginSummary {
    id: IdentityId,
    account_uuid: Option<String>,
    email: Option<String>,
    organization_uuid: Option<String>,
    plan: Option<String>,
    is_hidden: bool,
}

/// What callers compare to say what changed.
#[derive(PartialEq)]
struct Summary {
    accounts: Vec<Account>,
    folders: Vec<RunFolder>,
    unsigned: Vec<AccountId>,
    suggestions: Vec<FolderSuggestion>,
    folder_identities: BTreeMap<String, String>,
    logins: Vec<LoginSummary>,
}

pub struct AccountRegistry {
    paths: Paths,
    /// `AGENTNOTCH_EXTRA_CONFIG_DIRS`, added like discovered folders.
    extra_config_dirs: Vec<String>,
    probe: Option<Arc<dyn FolderProbe>>,

    /// Every known folder: the default folder first, then by label. Untracked
    /// ones are included (flagged `is_hidden`).
    folders: Vec<Folder>,
    /// One per signed-in identity (plus folders added by hand that nobody
    /// has signed in to yet), by label.
    identities: Vec<IdentityAccount>,
    /// Run folders nobody is signed in to, with no ring of their own.
    unsigned_folders: Vec<Folder>,
    /// Folders that look like accounts but need the user's yes.
    suggestions: Vec<FolderSuggestion>,
    /// The latest classification of every Claude folder found.
    layout: Layout,
    /// Discovery has classified the folders at least once this run (nothing
    /// is installed before).
    has_classified: bool,

    /// What the user chose per identity.
    identity_prefs: BTreeMap<String, IdentityPrefs>,
    /// Identities the user forgot: their folders stay known (a window can
    /// switch to another account), but have no ring and no hooks.
    forgotten_identity_keys: BTreeSet<String>,
    /// Folder id → identity id.
    identity_of_folder: BTreeMap<String, String>,
    /// Folders of forgotten identities.
    forgotten_identity_folders: BTreeSet<String>,
    /// Folders `accounts.json` had (their per-folder choices seed their
    /// identity's, when it has none yet).
    saved_folder_ids: BTreeSet<String>,
    /// Identities have been read from the folders at least once (before,
    /// every folder looks signed out).
    identities_read: bool,
    /// Folder id → identity key, forgotten identities included.
    folder_keys: BTreeMap<String, String>,
    /// The identity `~\.claude`'s own `accountUuid` names.
    default_owner: Option<String>,
    /// Folders whose `.claude.json` names another account's UUID (mirrored).
    corrected_folders: BTreeSet<String>,
    /// Who `~\.claude` ran as over time.
    default_timeline: FolderIdentityTimeline,
    /// The next look at `~\.claude` is the first since launch.
    default_timeline_resumed: bool,
    /// When `~\.claude.json` was written, as of its last read.
    default_identity_modified_at: Option<SystemTime>,
    /// Sealed: fixtures only, nothing observed or saved.
    holds_fixtures: bool,

    /// Folders the user forgot (or suggestions they dismissed), by
    /// `Paths::key`, with the path as it was written. Neither discovery nor
    /// a sighting brings them back; a sighting only suggests.
    removed_ids: BTreeMap<String, String>,
    /// Removed folders a session ran in since.
    seen_again_ids: BTreeMap<String, String>,
    /// What the last discovery found but did not add.
    discovered_suggestions: Vec<FolderSuggestion>,

    /// Folders no read of the disk has covered yet (by `Paths::key`): seen in
    /// a session or added by hand since the last read was planned.
    pending_facts: BTreeSet<String>,
    /// `accounts.json` is out of date.
    dirty: bool,
    /// What other threads read.
    rings: FolderRings,
}

impl AccountRegistry {
    /// An empty registry for the home folder `paths` was made with.
    pub fn new(paths: Paths) -> Self {
        AccountRegistry {
            paths,
            extra_config_dirs: Vec::new(),
            probe: None,
            folders: Vec::new(),
            identities: Vec::new(),
            unsigned_folders: Vec::new(),
            suggestions: Vec::new(),
            layout: Layout::default(),
            has_classified: false,
            identity_prefs: BTreeMap::new(),
            forgotten_identity_keys: BTreeSet::new(),
            identity_of_folder: BTreeMap::new(),
            forgotten_identity_folders: BTreeSet::new(),
            saved_folder_ids: BTreeSet::new(),
            identities_read: false,
            folder_keys: BTreeMap::new(),
            default_owner: None,
            corrected_folders: BTreeSet::new(),
            default_timeline: FolderIdentityTimeline::default(),
            default_timeline_resumed: true,
            default_identity_modified_at: None,
            holds_fixtures: false,
            removed_ids: BTreeMap::new(),
            seen_again_ids: BTreeMap::new(),
            discovered_suggestions: Vec::new(),
            pending_facts: BTreeSet::new(),
            dirty: false,
            rings: FolderRings::default(),
        }
    }

    /// `AGENTNOTCH_EXTRA_CONFIG_DIRS`: folders added like discovered ones.
    pub fn with_extra_config_dirs(mut self, dirs: &[String]) -> Self {
        self.extra_config_dirs = dirs.iter().map(|dir| self.paths.normalize(dir)).collect();
        self
    }

    /// How a folder the user picks is looked at. Without one, adding and
    /// creating folders is refused.
    pub fn with_probe(mut self, probe: Arc<dyn FolderProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    // ---- accounts.json ----

    /// Takes what `accounts.json` holds (before the first discovery). False
    /// when the bytes aren't one: the registry then starts from discovery.
    pub fn load(&mut self, bytes: &[u8]) -> bool {
        let Some(state) = AccountsFile::parse(bytes) else {
            return false;
        };
        let paths = self.paths.clone();
        self.removed_ids = state
            .removed_ids
            .iter()
            .map(|id| (paths.key(id), paths.normalize(id)))
            .collect();
        self.identity_prefs = state
            .identities
            .unwrap_or_default()
            .into_iter()
            .map(|(key, prefs)| {
                (
                    key,
                    IdentityPrefs {
                        custom_label: prefs.custom_label,
                        color_index: prefs.color_index,
                        is_hidden: prefs.is_hidden,
                        ring_hidden: prefs.ring_hidden,
                    },
                )
            })
            .collect();
        self.forgotten_identity_keys = state
            .forgotten_identities
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.default_timeline = state
            .default_identity_timeline
            .as_ref()
            .map(FolderIdentityTimeline::from_persisted)
            .unwrap_or_default();
        let mut seen = BTreeSet::new();
        let mut loaded = Vec::new();
        for stored in state.accounts {
            // The resolved form is checked by the first discovery: nothing is
            // read from the disk here.
            if !can_be_account(&paths, &stored.config_dir, None, None) {
                continue;
            }
            let mut folder = Folder::new(&paths, &stored.config_dir);
            if !seen.insert(paths.key(folder.dir())) {
                continue;
            }
            folder.config_dir_env = stored.config_dir_env;
            folder.custom_label = stored.custom_label;
            for variant in stored.seen_config_dir_envs.unwrap_or_default() {
                if !self.has_seen(&folder, &variant) {
                    folder.seen_config_dir_envs.push(variant);
                }
            }
            folder.color_index = stored.color_index;
            folder.source = match stored.source.as_str() {
                "hook" => FolderSource::Hook,
                "manual" => FolderSource::Manual,
                _ => FolderSource::Discovered,
            };
            folder.last_seen_at = stored.last_seen_at.map(|date| date.0);
            folder.is_hidden = stored.is_hidden;
            loaded.push(folder);
        }
        self.saved_folder_ids = loaded.iter().map(|f| f.dir().to_owned()).collect();
        self.publish(loaded, None);
        self.dirty = false;
        true
    }

    /// `accounts.json` as it should be now.
    pub fn file(&self) -> AccountsFile {
        AccountsFile {
            version: file::VERSION,
            accounts: self
                .folders
                .iter()
                .map(|folder| PersistedFolder {
                    id: folder.dir().to_owned(),
                    config_dir: folder.dir().to_owned(),
                    config_dir_env: folder.config_dir_env.clone(),
                    custom_label: folder.custom_label.clone(),
                    color_index: folder.color_index,
                    is_hidden: folder.is_hidden,
                    source: match folder.source {
                        FolderSource::Discovered => "discovered",
                        FolderSource::Hook => "hook",
                        FolderSource::Manual => "manual",
                    }
                    .to_owned(),
                    last_seen_at: folder.last_seen_at.map(IsoSeconds),
                    seen_config_dir_envs: (!folder.seen_config_dir_envs.is_empty())
                        .then(|| folder.seen_config_dir_envs.clone()),
                })
                .collect(),
            removed_ids: {
                let mut removed: Vec<String> = self.removed_ids.values().cloned().collect();
                removed.sort();
                removed
            },
            identities: (!self.identity_prefs.is_empty()).then(|| {
                self.identity_prefs
                    .iter()
                    .map(|(key, prefs)| {
                        (
                            key.clone(),
                            file::IdentityPrefs {
                                custom_label: prefs.custom_label.clone(),
                                color_index: prefs.color_index,
                                is_hidden: prefs.is_hidden,
                                ring_hidden: prefs.ring_hidden,
                            },
                        )
                    })
                    .collect()
            }),
            forgotten_identities: (!self.forgotten_identity_keys.is_empty())
                .then(|| self.forgotten_identity_keys.iter().cloned().collect()),
            default_identity_timeline: (!self.default_timeline.is_empty())
                .then(|| self.default_timeline.to_persisted()),
        }
    }

    /// The bytes to write as `accounts.json`; `None` while the registry
    /// holds fixtures (a sealed run saves nothing).
    pub fn file_bytes(&self) -> Option<Vec<u8>> {
        (!self.holds_fixtures).then(|| self.file().encode())
    }

    /// Something worth saving changed since [`AccountRegistry::mark_saved`].
    pub fn needs_save(&self) -> bool {
        self.dirty && !self.holds_fixtures
    }

    /// The caller wrote (or queued) [`AccountRegistry::file_bytes`].
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    // ---- fixtures ----

    /// Sealed mode: show `fixtures` and nothing else. In memory only;
    /// nothing is discovered, read or saved afterwards.
    pub fn replace_all_with_fixtures(&mut self, fixtures: Vec<Folder>, layout: Option<Layout>) {
        self.holds_fixtures = true;
        self.dirty = false;
        self.suggestions.clear();
        self.discovered_suggestions.clear();
        self.pending_facts.clear();
        if let Some(layout) = layout {
            self.layout = layout;
        }
        self.identities_read = true;
        self.publish(fixtures, None);
    }

    // ---- queries ----

    /// One per identity, tracked or not (forgotten ones left out), by label.
    pub fn accounts(&self) -> Vec<Account> {
        self.identities
            .iter()
            .map(|identity| identity.to_account(&self.paths))
            .collect()
    }

    /// Every known folder (stores included), the default first.
    pub fn folders(&self) -> Vec<RunFolder> {
        self.folders.iter().map(Folder::to_run_folder).collect()
    }

    /// The identities behind [`AccountRegistry::accounts`], with their folders.
    pub fn identities(&self) -> &[IdentityAccount] {
        &self.identities
    }

    /// Identities to show and poll.
    pub fn visible_identities(&self) -> Vec<&IdentityAccount> {
        self.identities.iter().filter(|i| !i.is_hidden).collect()
    }

    pub fn known_folders(&self) -> &[Folder] {
        &self.folders
    }

    /// Folders to show and poll.
    pub fn visible_folders(&self) -> Vec<&Folder> {
        self.folders.iter().filter(|f| !f.is_hidden).collect()
    }

    /// Run folders nobody is signed in to, with no ring of their own
    /// (Settings lists them as "Not signed in").
    pub fn unsigned_folders(&self) -> &[Folder] {
        &self.unsigned_folders
    }

    pub fn suggestions(&self) -> &[FolderSuggestion] {
        &self.suggestions
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn has_classified(&self) -> bool {
        self.has_classified
    }

    pub fn identities_read(&self) -> bool {
        self.identities_read
    }

    /// Whether Claude Parallel Profiles mirrors accounts into `~\.claude`.
    pub fn mirrors_default(&self) -> bool {
        self.layout.extension_detected
    }

    /// Folders classified as infrastructure (the shared history).
    pub fn infrastructure_dirs(&self) -> Vec<String> {
        self.layout.infrastructure()
    }

    pub fn corrected_folders(&self) -> &BTreeSet<String> {
        &self.corrected_folders
    }

    pub fn default_owner(&self) -> Option<&str> {
        self.default_owner.as_deref()
    }

    pub fn default_timeline(&self) -> &FolderIdentityTimeline {
        &self.default_timeline
    }

    /// What the last publish says about rings and tracking, for a thread
    /// that can't ask the registry.
    pub fn folder_rings(&self) -> &FolderRings {
        &self.rings
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        let key = self.paths.key(id);
        self.folders
            .iter()
            .position(|folder| self.paths.key(folder.dir()) == key)
    }

    /// The id this registry keeps for a folder (the spelling it was first
    /// known by), else the path normalized: on Windows two spellings of one
    /// folder are one folder.
    fn own_id(&self, id: &str) -> String {
        match self.index_of(id) {
            Some(index) => self.folders[index].dir().to_owned(),
            None => self.paths.normalize(id),
        }
    }

    pub fn folder(&self, id: &str) -> Option<&Folder> {
        self.index_of(id).map(|index| &self.folders[index])
    }

    pub fn identity(&self, id: &str) -> Option<&IdentityAccount> {
        self.identities.iter().find(|i| i.id.as_str() == id)
    }

    /// The identity a folder (an id sessions and hooks carry) belongs to now.
    pub fn identity_of_folder(&self, folder_id: &str) -> Option<&IdentityAccount> {
        self.identity_of_folder
            .get(&self.own_id(folder_id))
            .and_then(|id| self.identity(id))
    }

    /// The identity id for a folder id, or the identity id itself.
    pub fn identity_id_for(&self, id: &str) -> Option<IdentityId> {
        if self.identity(id).is_some() {
            return Some(IdentityId::new(id));
        }
        self.identity_of_folder
            .get(&self.own_id(id))
            .map(|id| IdentityId::new(id.clone()))
    }

    pub fn ring_of_folder(&self, folder: &AccountId) -> Option<RingId> {
        self.rings
            .ring_of_folder(&self.paths, folder.as_str())
            .cloned()
    }

    /// Who a session in `folder`, whose process started at `started`, runs
    /// as: `~\.claude` is attributed by who it ran as then while Claude
    /// Parallel Profiles mirrors accounts into it; other folders by who they
    /// name now.
    pub fn attribution(&self, folder: &AccountId, started: Option<SystemTime>) -> Attribution {
        match self
            .rings
            .attribution(&self.paths, folder.as_str(), started)
        {
            FolderAttribution::Known(identity) => Attribution::Known(identity.map(IdentityId::new)),
            FolderAttribution::Unsure(current) => Attribution::Unsure(current.map(IdentityId::new)),
        }
    }

    /// A hook event from this folder and process belongs to an untracked or
    /// forgotten account: nothing shows it, so nothing may hold it (a
    /// permission request is released at once and the session asks in its
    /// own terminal).
    pub fn is_untracked(&self, folder: &AccountId, started: Option<SystemTime>) -> bool {
        self.rings
            .is_untracked(&self.paths, folder.as_str(), started)
    }

    /// The identity what was saved for a folder (before accounts were
    /// identities) belongs to: its identity, except for a mirrored
    /// `~\.claude`, whose saved choices and readings are its owner's (the
    /// account its own `accountUuid` names), or nobody's when that is unknown.
    pub fn owner_of_saved_folder(&self, folder_id: &str) -> Option<IdentityId> {
        let folder = self.own_id(folder_id);
        if self.paths.is_default_config_dir(&folder) && self.corrected_folders.contains(&folder) {
            return self.default_owner.clone().map(IdentityId::new);
        }
        self.identity_id_for(&folder)
    }

    /// The folders an id stands for: an identity's (run folders and stores),
    /// or the one folder.
    pub fn folders_for(&self, id: &str) -> Vec<&Folder> {
        if let Some(identity) = self.identity(id) {
            return identity
                .folder_ids()
                .iter()
                .filter_map(|folder| self.folder(folder.as_str()))
                .collect();
        }
        self.folder(id).into_iter().collect()
    }

    /// An account the user forgot (and hasn't added back): its sessions are
    /// shown nowhere and announce nothing, rather than landing on the default
    /// ring. Takes a folder id or an identity id.
    pub fn is_forgotten(&self, id: &str) -> bool {
        if [UUID_PREFIX, EMAIL_PREFIX, DIR_PREFIX]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        {
            return self.forgotten_identity_keys.contains(id);
        }
        if self.forgotten_identity_folders.contains(&self.own_id(id)) {
            return true;
        }
        self.removed_ids.contains_key(&self.paths.key(id)) && self.folder(id).is_none()
    }

    /// Every forgotten folder id.
    pub fn forgotten_ids(&self) -> BTreeSet<String> {
        self.removed_ids
            .iter()
            .filter(|(_, id)| self.folder(id).is_none())
            .map(|(_, id)| id.clone())
            .chain(self.forgotten_identity_folders.iter().cloned())
            .collect()
    }

    /// Where each folder's identity file is, as Claude Code resolves it: what
    /// the hub reads between discoveries (a `/login` shows within seconds).
    pub fn identity_files(&self) -> Vec<(AccountId, PathBuf)> {
        self.folders
            .iter()
            .map(|folder| {
                (
                    folder.id.clone(),
                    PathBuf::from(folder.global_config_file(&self.paths)),
                )
            })
            .collect()
    }

    /// The folders a read of the disk is asked about by name: the known
    /// ones, `AGENTNOTCH_EXTRA_CONFIG_DIRS`, and the stores of the last
    /// classification (so an unreadable manifest can keep them stores).
    pub fn explicit_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<String> = Vec::new();
        let candidates = self
            .extra_config_dirs
            .iter()
            .cloned()
            .chain(self.folders.iter().map(|f| f.dir().to_owned()))
            .filter(|dir| can_be_account(&self.paths, dir, None, None))
            .chain(self.layout.stores());
        for dir in candidates {
            if !dirs.iter().any(|known| self.paths.same(known, &dir)) {
                dirs.push(dir);
            }
        }
        dirs.into_iter().map(PathBuf::from).collect()
    }

    /// A folder is known that no read of the disk has covered: read the
    /// folders again soon rather than at the next interval.
    pub fn needs_discovery(&self) -> bool {
        !self.pending_facts.is_empty() && !self.holds_fixtures
    }

    /// The line that starts Claude Code as an account (or in a folder), when
    /// a terminal can: `claude`, or one that sets `CLAUDE_CONFIG_DIR` first.
    pub fn launch_command(&self, id: &str) -> Option<String> {
        if let Some(identity) = self.identity(id) {
            return identity.terminal_launch_command(&self.paths);
        }
        self.folder(id)
            .filter(|folder| folder.kind == FolderKind::Run)
            .map(|folder| folder.launch_command(&self.paths))
    }

    /// The folder an account is shown from, or the folder itself.
    pub fn primary_dir(&self, id: &str) -> Option<AccountId> {
        if let Some(identity) = self.identity(id) {
            return identity.primary_dir().map(|folder| folder.id.clone());
        }
        self.folder(id).map(|folder| folder.id.clone())
    }

    // ---- discovery ----

    /// Takes a read of the home folder: classifies what it found, adds the
    /// folders clearly in use, drops what turned out not to be an account,
    /// refreshes the suggestions, and applies who is signed in where.
    pub fn discover(&mut self, snap: FolderSnapshot, now: SystemTime) -> AccountsChanged {
        if self.holds_fixtures {
            return AccountsChanged::default();
        }
        let before = self.summary();
        let paths = self.paths.clone();
        let found = classify::discover(
            &snap,
            &self.extra_config_dirs,
            &self.layout.stores(),
            &paths,
        );
        self.layout = found.layout.clone();
        self.has_classified = true;

        let mut folders = std::mem::take(&mut self.folders);
        let count = folders.len();
        folders.retain(|folder| self.keeps(folder, &snap));
        if folders.len() != count {
            self.dirty = true;
        }
        for dir in &found.accounts {
            let key = paths.key(dir);
            if folders.iter().any(|f| paths.key(f.dir()) == key)
                || self.removed_ids.contains_key(&key)
            {
                continue;
            }
            let mut folder = Folder::new(&paths, dir);
            // A discovered custom folder is used as CLAUDE_CONFIG_DIR=<path>;
            // a sighting replaces this with the exact string sessions use.
            folder.config_dir_env =
                (!paths.is_default_config_dir(dir)).then(|| folder.dir().to_owned());
            let used: Vec<i64> = folders.iter().map(|f| f.color_index).collect();
            folder.color_index = next_color_index(&used);
            folder.source = FolderSource::Discovered;
            folder.kind = found.layout.kind(&paths, dir).unwrap_or(FolderKind::Run);
            folders.push(folder);
            self.dirty = true;
        }

        self.identities_read = true;
        self.apply_snapshot_identities(&mut folders, &snap);
        self.discovered_suggestions = found.suggestions;
        self.pending_facts.retain(|key| {
            !snap.requested.iter().any(|dir| &paths.key(dir) == key)
                && !snap.folders.iter().any(|f| &paths.key(&f.path) == key)
        });
        self.publish(folders, Some(now));
        self.refresh_suggestions();
        self.changes_since(&before)
    }

    /// Whether a known folder stays after a read of the disk: never one that
    /// turned out to be infrastructure, or the home folder through a link;
    /// the extension's own folders go when they are gone from disk, anything
    /// else stays ("Folder missing").
    fn keeps(&self, folder: &Folder, snap: &FolderSnapshot) -> bool {
        let paths = &self.paths;
        let facts = classify::find(snap, paths, folder.dir());
        match self.layout.kind(paths, folder.dir()) {
            Some(FolderKind::Infrastructure) => false,
            Some(_) => can_be_account(
                paths,
                folder.dir(),
                facts.and_then(|f| f.canonical.as_deref()),
                snap.home_canonical.as_deref(),
            ),
            None => {
                // Not there when the read looked. A folder the registry
                // learnt of after the read was planned wasn't asked about:
                // nothing is known of it yet.
                let asked = snap
                    .requested
                    .iter()
                    .any(|dir| paths.same(dir, folder.dir()));
                let extension_folder =
                    is_window_dir(paths, folder.dir()) || folder.kind == FolderKind::Store;
                !(asked && extension_folder)
            }
        }
    }

    /// What a snapshot read of the `.claude.json` at `file`.
    fn config_at(&self, file: &str, snap: &FolderSnapshot) -> Lookup {
        let paths = &self.paths;
        if paths.same(file, &paths.default_identity_file()) {
            return Lookup::Known(snap.home_config.clone().map(Box::new));
        }
        let Some(dir) = paths.parent(file) else {
            return Lookup::Unknown;
        };
        match classify::find(snap, paths, &dir) {
            Some(facts) => Lookup::Known(facts.own_config.clone().map(Box::new)),
            // Asked about and not there: the folder is gone, its login with it.
            None if snap.requested.iter().any(|asked| paths.same(asked, &dir)) => {
                Lookup::Known(None)
            }
            None => Lookup::Unknown,
        }
    }

    /// The identity file of a folder run with `env`, as a snapshot read it.
    /// An `env` that names the folder by another path (through a link, say)
    /// isn't in the snapshot: the folder's own file is the one Claude Code
    /// reads there.
    fn folder_config(&self, dir: &str, env: Option<&str>, snap: &FolderSnapshot) -> Lookup {
        let paths = &self.paths;
        let file = paths.global_config_file(dir, env);
        match self.config_at(&file, snap) {
            Lookup::Unknown => self.config_at(&paths.join(dir, ".claude.json"), snap),
            known => known,
        }
    }

    /// Applies who is signed in where, from a snapshot. A folder seen with
    /// `CLAUDE_CONFIG_DIR` both set and unset (only `~\.claude` can be: the
    /// two read different identity files) moves to the one that is signed
    /// in, if the current one isn't. Two rounds at most: the move, then its
    /// identity.
    fn apply_snapshot_identities(&mut self, folders: &mut [Folder], snap: &FolderSnapshot) {
        let paths = self.paths.clone();
        for _ in 0..2 {
            let mut moved = false;
            for folder in folders.iter_mut() {
                let (dir, env) = (folder.dir().to_owned(), folder.config_dir_env.clone());
                let Lookup::Known(config) = self.folder_config(&dir, env.as_deref(), snap) else {
                    continue;
                };
                if folder.is_default(&paths) {
                    self.default_identity_modified_at = config.as_ref().and_then(|c| c.modified_at);
                }
                let identity = config.and_then(|c| c.identity);
                if identity.is_none() && folder.seen_config_dir_envs.len() > 1 {
                    let current = paths.global_config_file(&dir, env.as_deref());
                    let better = folder
                        .seen_config_dir_envs
                        .iter()
                        .find(|variant| {
                            let variant_env = (!variant.is_empty()).then_some(variant.as_str());
                            let file = paths.global_config_file(&dir, variant_env);
                            !paths.same(&file, &current)
                                && matches!(
                                    self.config_at(&file, snap),
                                    Lookup::Known(Some(config)) if config.identity.is_some()
                                )
                        })
                        .cloned();
                    if let Some(better) = better {
                        folder.config_dir_env = (!better.is_empty()).then_some(better);
                        moved = true;
                        self.dirty = true;
                        continue;
                    }
                }
                folder.apply_identity(identity);
            }
            if !moved {
                break;
            }
        }
    }

    // ---- identity ----

    /// Applies the identity read from a folder's `.claude.json` (`None` =
    /// signed out), last written at `modified_at`.
    pub fn apply_identity(
        &mut self,
        folder: &AccountId,
        identity: Option<Identity>,
        modified_at: Option<SystemTime>,
        now: SystemTime,
    ) -> AccountsChanged {
        if self.holds_fixtures {
            return AccountsChanged::default();
        }
        let Some(index) = self.index_of(folder.as_str()) else {
            return AccountsChanged::default();
        };
        let before = self.summary();
        self.identities_read = true;
        let is_default = self.folders[index].is_default(&self.paths);
        if is_default && modified_at.is_some() {
            self.default_identity_modified_at = modified_at;
        }
        let mut updated = self.folders[index].clone();
        updated.apply_identity(identity);
        if updated == self.folders[index] {
            if is_default {
                self.observe_default(Some(now));
            }
            return self.changes_since(&before);
        }
        let mut folders = self.folders.clone();
        folders[index] = updated;
        self.publish(folders, Some(now));
        self.changes_since(&before)
    }

    /// [`AccountRegistry::apply_identity`] from a `.claude.json` read job. A
    /// read that failed says nothing (the file may be mid-replace): who was
    /// signed in stays.
    pub fn apply_claude_json(&mut self, read: &ClaudeJsonRead, now: SystemTime) -> AccountsChanged {
        if read.error.is_some() {
            return AccountsChanged::default();
        }
        let modified_at = read.stamp.and_then(|(nanoseconds, _)| {
            u64::try_from(nanoseconds)
                .ok()
                .map(|ns| UNIX_EPOCH + Duration::from_nanos(ns))
        });
        self.apply_identity(&read.folder, read.identity.clone(), modified_at, now)
    }

    /// Records the plan `get_usage` reported (`max`, …) when `.claude.json`
    /// carries no organization type.
    pub fn note_subscription_type(
        &mut self,
        folder: &AccountId,
        subscription_type: &str,
    ) -> AccountsChanged {
        let Some(index) = self.index_of(folder.as_str()) else {
            return AccountsChanged::default();
        };
        if subscription_type.is_empty()
            || self.folders[index].subscription_type.as_deref() == Some(subscription_type)
        {
            return AccountsChanged::default();
        }
        let before = self.summary();
        let mut folders = self.folders.clone();
        folders[index].subscription_type = Some(subscription_type.to_owned());
        self.publish(folders, None);
        self.changes_since(&before)
    }

    // ---- sightings ----

    /// `variant` is a spelling this folder was already seen with. Spellings
    /// collapse by `Paths::key`: on Windows a folder's login is a file inside
    /// it, reached through a case-insensitive path, so two spellings of one
    /// folder are one login (there is no "login conflict" to show). Unset
    /// (`""`) stays apart from set: for `~\.claude` they read different
    /// identity files.
    fn has_seen(&self, folder: &Folder, variant: &str) -> bool {
        folder
            .seen_config_dir_envs
            .iter()
            .any(|seen| match (seen.is_empty(), variant.is_empty()) {
                (true, true) => true,
                (false, false) => self.paths.key(seen) == self.paths.key(variant),
                _ => false,
            })
    }

    /// A hook or status line event came from this config folder.
    pub fn record(&mut self, sighting: AccountSighting, now: SystemTime) -> AccountsChanged {
        if self.holds_fixtures {
            return AccountsChanged::default();
        }
        let paths = self.paths.clone();
        let id = paths.normalize(sighting.config_dir.as_str());
        // A transcript path resolved through the shared history's links, or
        // the windows folder itself: never an account.
        if self.is_infrastructure(&id) {
            return AccountsChanged::default();
        }
        let is_default_dir = paths.is_default_config_dir(&id);
        // What Claude Code saw: a raw CLAUDE_CONFIG_DIR, or "" for unset
        // (which only ever means ~\.claude).
        let raw_env = sighting.config_dir_env.filter(|env| !env.is_empty());
        let variant = raw_env.clone().or_else(|| is_default_dir.then(String::new));
        let before = self.summary();

        let Some(index) = self.index_of(&id) else {
            if !can_be_account(&paths, &id, None, None) {
                return AccountsChanged::default();
            }
            let key = paths.key(&id);
            if self.removed_ids.contains_key(&key) {
                // Forgotten on purpose: ask, don't re-add.
                if self.seen_again_ids.insert(key, id).is_none() {
                    self.refresh_suggestions();
                }
                return self.changes_since(&before);
            }
            let mut folder = Folder::new(&paths, &id);
            folder.config_dir_env = raw_env.or_else(|| (!is_default_dir).then(|| id.clone()));
            folder.seen_config_dir_envs = variant.into_iter().collect();
            let used: Vec<i64> = self.folders.iter().map(|f| f.color_index).collect();
            folder.color_index = next_color_index(&used);
            folder.source = FolderSource::Hook;
            folder.last_seen_at = Some(sighting.at);
            let mut folders = self.folders.clone();
            folders.push(folder);
            // What it is (a window's working copy, a standalone folder) and
            // who is signed in there comes with the next read of the disk.
            self.pending_facts.insert(key);
            self.dirty = true;
            self.publish(folders, Some(now));
            return self.changes_since(&before);
        };

        let mut folder = self.folders[index].clone();
        // The raw CLAUDE_CONFIG_DIR is kept verbatim: the usage check runs
        // with the same one. The first spelling seen sticks.
        if let Some(variant) = variant {
            if !self.has_seen(&folder, &variant) {
                if folder.seen_config_dir_envs.is_empty() {
                    folder.config_dir_env = (!variant.is_empty()).then(|| variant.clone());
                }
                folder.seen_config_dir_envs.push(variant);
            }
        }
        let is_later = folder.last_seen_at.is_none_or(|seen| {
            sighting
                .at
                .duration_since(seen)
                .is_ok_and(|gap| gap >= SIGHTING_RESOLUTION)
        });
        if is_later {
            folder.last_seen_at = Some(sighting.at);
        }
        if folder == self.folders[index] {
            return AccountsChanged::default();
        }
        let env_changed = folder.config_dir_env != self.folders[index].config_dir_env
            || folder.seen_config_dir_envs.len() != self.folders[index].seen_config_dir_envs.len();
        if env_changed {
            // The identity file moves with the variable: read it again.
            self.pending_facts.insert(paths.key(folder.dir()));
        }
        let mut folders = self.folders.clone();
        folders[index] = folder;
        self.dirty = true;
        self.publish(folders, Some(now));
        self.changes_since(&before)
    }

    fn is_infrastructure(&self, normalized: &str) -> bool {
        let paths = &self.paths;
        self.layout.kind(paths, normalized) == Some(FolderKind::Infrastructure)
            || paths.same(normalized, &shared_store(paths))
            || paths.same(normalized, &windows_root(paths))
    }

    // ---- suggestions ----

    fn refresh_suggestions(&mut self) {
        let paths = &self.paths;
        let known: BTreeSet<String> = self.folders.iter().map(|f| paths.key(f.dir())).collect();
        let mut list: Vec<FolderSuggestion> = self
            .discovered_suggestions
            .iter()
            .filter(|s| {
                let key = paths.key(&s.config_dir);
                !known.contains(&key) && !self.removed_ids.contains_key(&key)
            })
            .cloned()
            .collect();
        let mut seen_again: Vec<(&String, &String)> = self.seen_again_ids.iter().collect();
        seen_again.sort_by(|a, b| a.1.cmp(b.1));
        for (key, id) in seen_again {
            if known.contains(key) {
                continue;
            }
            list.retain(|s| &paths.key(&s.config_dir) != key);
            list.push(FolderSuggestion {
                config_dir: id.clone(),
                reason: SuggestionReason::SeenAgain,
            });
        }
        self.suggestions = list;
    }

    /// Stops suggesting a folder (until the user adds it by hand).
    pub fn dismiss_suggestion(&mut self, config_dir: &str) -> AccountsChanged {
        let before = self.summary();
        let id = self.paths.normalize(config_dir);
        let key = self.paths.key(&id);
        self.removed_ids.insert(key.clone(), id);
        self.seen_again_ids.remove(&key);
        self.refresh_suggestions();
        self.dirty = true;
        self.changes_since(&before)
    }

    // ---- user actions ----

    /// What the user asked of an account in Settings. The error is the
    /// sentence to show.
    pub fn apply_user(&mut self, action: AccountAction) -> Result<AccountsChanged, String> {
        let paths = self.paths.clone();
        let refused = |error: AccountError| error.message(&paths);
        match action {
            AccountAction::Rename { id, label } => Ok(self.rename(&id, label.as_deref())),
            AccountAction::Track { id, on } => Ok(self.set_hidden(&id, !on)),
            AccountAction::RingShown { ring_id, on } => Ok(self.set_ring_shown(&ring_id, on)),
            AccountAction::Forget { id } => {
                if self.identity(&id).is_some_and(|i| !i.can_be_forgotten()) {
                    return Err(refused(AccountError::NotForgettable));
                }
                Ok(self.remove(&id))
            }
            AccountAction::AddFolder { path } | AccountAction::SuggestionAdd { path } => self
                .add_folder(&path)
                .map(|(_, changed)| changed)
                .map_err(refused),
            AccountAction::Create { name } => self
                .create_account(&name)
                .map(|created| created.changed)
                .map_err(refused),
            AccountAction::SuggestionDismiss { path } => Ok(self.dismiss_suggestion(&path)),
        }
    }

    /// Whether `path` can be added as an account, and what it holds. Refuses
    /// the home folder, a folder containing it or `~\.claude`, a folder
    /// inside another account's config folder, and anything not a folder.
    pub fn check_folder(&self, path: &str) -> Result<FolderMarkers, AccountError> {
        self.check_picked(path)
            .map(|picked| FolderMarkers::of(&picked.facts))
    }

    fn check_picked(&self, path: &str) -> Result<PickedFolder, AccountError> {
        let paths = &self.paths;
        let normalized = paths.normalize(path);
        if self.is_infrastructure(&normalized) {
            return Err(AccountError::Infrastructure);
        }
        let probe = self.probe()?;
        let picked = match probe.look(&normalized) {
            Picked::Missing => return Err(AccountError::Missing),
            Picked::NotAFolder => return Err(AccountError::NotAFolder),
            Picked::Folder(picked) => *picked,
        };
        // Checked as picked and with links resolved: a link to home is home.
        let home = paths.home().to_owned();
        let mut pairs = vec![(normalized.clone(), home.clone())];
        if let Some(resolved) = picked.facts.canonical.clone() {
            pairs.push((resolved, picked.home_canonical.clone().unwrap_or(home)));
        }
        for (folder, home) in pairs {
            if paths.same(&folder, &home) {
                return Err(AccountError::HomeFolder);
            }
            if paths.is_ancestor(&folder, &home)
                || paths.is_ancestor(&folder, &paths.join(&home, ".claude"))
            {
                return Err(AccountError::ContainsAccounts);
            }
        }
        let default_dir = paths.default_config_dir();
        let container = self
            .folders
            .iter()
            .map(Folder::dir)
            .chain([default_dir.as_str()])
            .find(|container| paths.is_ancestor(container, &normalized));
        if let Some(container) = container {
            let label = match self.folder(container) {
                Some(folder) => folder.label(paths),
                None => paths.abbreviate(container),
            };
            return Err(AccountError::InsideAccount(label));
        }
        Ok(picked)
    }

    fn probe(&self) -> Result<&Arc<dyn FolderProbe>, AccountError> {
        if self.holds_fixtures {
            return Err(AccountError::UnavailableWhenSealed);
        }
        self.probe.as_ref().ok_or(AccountError::Unavailable)
    }

    /// Adds an existing config folder (checked with
    /// [`AccountRegistry::check_folder`]). Returns the (possibly already
    /// known, now tracked again) folder's id.
    pub fn add_folder(
        &mut self,
        config_dir: &str,
    ) -> Result<(AccountId, AccountsChanged), AccountError> {
        self.add_folder_as(config_dir, None)
    }

    fn add_folder_as(
        &mut self,
        config_dir: &str,
        config_dir_env: Option<String>,
    ) -> Result<(AccountId, AccountsChanged), AccountError> {
        let picked = self.check_picked(config_dir)?;
        let before = self.summary();
        let paths = self.paths.clone();
        let id = self.own_id(config_dir);
        let key = paths.key(&id);
        if self.removed_ids.remove(&key).is_some() {
            self.dirty = true;
        }
        self.seen_again_ids.remove(&key);
        // Adding a folder of a forgotten account brings the account back; a
        // folder added by hand that nobody signed in to is its own account.
        let forgotten_key = self
            .folder_keys
            .get(&id)
            .filter(|_| self.forgotten_identity_folders.contains(&id))
            .cloned();
        let mut brought_back = false;
        for key in [forgotten_key, Some(format!("{DIR_PREFIX}{id}"))]
            .into_iter()
            .flatten()
        {
            brought_back |= self.forgotten_identity_keys.remove(&key);
        }
        if brought_back {
            self.dirty = true;
            self.publish(self.folders.clone(), None);
        }

        if let Some(existing) = self.folder(&id).cloned() {
            if existing.is_hidden {
                self.set_hidden(&id, false);
            }
            self.refresh_suggestions();
            return Ok((existing.id, self.changes_since(&before)));
        }

        let mut folder = Folder::new(&paths, &id);
        folder.config_dir_env =
            config_dir_env.or_else(|| (!paths.is_default_config_dir(&id)).then(|| id.clone()));
        let used: Vec<i64> = self.folders.iter().map(|f| f.color_index).collect();
        folder.color_index = next_color_index(&used);
        folder.source = FolderSource::Manual;
        // Who is signed in there, when the folder reads its own file (the
        // default folder's is beside it: the next read of the disk has it).
        if paths.same(
            &folder.global_config_file(&paths),
            &paths.join(&id, ".claude.json"),
        ) {
            folder.apply_identity(picked.facts.own_config.and_then(|config| config.identity));
        }
        let added = folder.id.clone();
        let mut folders = self.folders.clone();
        folders.push(folder);
        self.pending_facts.insert(key);
        self.dirty = true;
        self.publish(folders, None);
        self.refresh_suggestions();
        Ok((added, self.changes_since(&before)))
    }

    /// Creates `~\.claude-<name>` for a new account and adds it. The user
    /// then signs in by running its launch command and `/login`. An existing
    /// folder is adopted only if it already is a Claude Code folder.
    pub fn create_account(&mut self, name: &str) -> Result<CreatedAccount, AccountError> {
        let paths = self.paths.clone();
        let slug = sanitized_account_name(name).ok_or(AccountError::InvalidName)?;
        let folder_name = format!(".claude-{slug}");
        let path = paths.join(paths.home(), &folder_name);
        let probe = self.probe()?.clone();
        match probe.look(&path) {
            Picked::Missing => probe
                .create_dir(&path)
                .map_err(AccountError::CreateFailed)?,
            Picked::NotAFolder => {
                return Err(AccountError::CreateFailed(format!(
                    "a file named {folder_name} already exists"
                )))
            }
            Picked::Folder(picked) => {
                let markers = FolderMarkers::of(&picked.facts);
                if !(markers.is_clearly_config_dir()
                    || markers.has_global_config
                    || picked.has_settings)
                {
                    return Err(AccountError::FolderExistsNotClaude(paths.abbreviate(&path)));
                }
            }
        }
        let before = self.summary();
        let (id, _) = self.add_folder_as(&path, Some(path.clone()))?;
        let label = name.trim();
        let unnamed = self
            .folder(id.as_str())
            .is_some_and(|folder| folder.custom_label.is_none());
        if unnamed && !label.is_empty() {
            self.rename(id.as_str(), Some(label));
        }
        let launch_command = self
            .folder(id.as_str())
            .map(|folder| folder.launch_command(&paths))
            .unwrap_or_default();
        Ok(CreatedAccount {
            folder: id,
            launch_command,
            changed: self.changes_since(&before),
        })
    }

    /// Sets (or with `None`/empty, clears) a custom name: an identity's, or
    /// a folder's (and then its identity's too).
    pub fn rename(&mut self, id: &str, label: Option<&str>) -> AccountsChanged {
        let before = self.summary();
        let new_label = label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_owned);
        if let Some(identity) = self.identity_id_for(id) {
            if let Some(prefs) = self.identity_prefs.get_mut(identity.as_str()) {
                prefs.custom_label = new_label.clone();
            }
        }
        let mut folders = self.folders.clone();
        if let Some(index) = self.index_of(id) {
            folders[index].custom_label = new_label;
        }
        self.dirty = true;
        self.publish(folders, None);
        self.changes_since(&before)
    }

    /// "Track sessions and hooks": an untracked account gets no hooks,
    /// sessions or usage checks. For an identity (or a folder of one) it
    /// applies to every folder of it, those that appear later included.
    pub fn set_hidden(&mut self, id: &str, hidden: bool) -> AccountsChanged {
        let before = self.summary();
        if let Some(identity) = self.identity_id_for(id) {
            if let Some(prefs) = self.identity_prefs.get_mut(identity.as_str()) {
                if prefs.is_hidden == hidden {
                    return AccountsChanged::default();
                }
                prefs.is_hidden = hidden;
                self.dirty = true;
                self.publish(self.folders.clone(), None);
                return self.changes_since(&before);
            }
        }
        let Some(index) = self.index_of(id) else {
            return AccountsChanged::default();
        };
        if self.folders[index].is_hidden == hidden {
            return AccountsChanged::default();
        }
        let mut folders = self.folders.clone();
        folders[index].is_hidden = hidden;
        self.dirty = true;
        self.publish(folders, None);
        self.changes_since(&before)
    }

    /// "Ring in notch": the account's usage ring is drawn or not (its
    /// sessions are still tracked).
    pub fn set_ring_shown(&mut self, ring_id: &str, shown: bool) -> AccountsChanged {
        let Some(identity) = self
            .identities
            .iter()
            .find(|identity| identity.ring_id.as_str() == ring_id)
            .cloned()
        else {
            return AccountsChanged::default();
        };
        if identity.ring_hidden != shown {
            return AccountsChanged::default();
        }
        let before = self.summary();
        let prefs = self
            .identity_prefs
            .entry(identity.id.as_str().to_owned())
            .or_insert_with(|| IdentityPrefs {
                custom_label: identity.custom_label.clone(),
                color_index: identity.color_index,
                is_hidden: identity.is_hidden,
                ring_hidden: identity.ring_hidden,
            });
        prefs.ring_hidden = !shown;
        self.dirty = true;
        self.publish(self.folders.clone(), None);
        self.changes_since(&before)
    }

    /// Forgets an account. Its folders are left alone; neither discovery nor
    /// a session re-adds it (a session only makes it a suggestion). This
    /// app's hooks are removed first by the caller, or they keep firing
    /// there. An identity is forgotten as a whole: every folder of it, those
    /// that appear later included, until one of them is added back.
    pub fn remove(&mut self, id: &str) -> AccountsChanged {
        let before = self.summary();
        if let Some(identity) = self.identity(id).cloned() {
            self.forgotten_identity_keys
                .insert(identity.id.as_str().to_owned());
            let folders = if identity.is_standalone_unsigned() {
                // A folder added by hand: forgotten as that folder.
                let gone: Vec<AccountId> = identity.folder_ids();
                for folder in &gone {
                    self.removed_ids
                        .insert(self.paths.key(folder.as_str()), folder.as_str().to_owned());
                }
                self.folders
                    .iter()
                    .filter(|f| !gone.contains(&f.id))
                    .cloned()
                    .collect()
            } else {
                self.folders.clone()
            };
            self.dirty = true;
            self.publish(folders, None);
            self.refresh_suggestions();
            return self.changes_since(&before);
        }
        let Some(index) = self.index_of(id) else {
            return AccountsChanged::default();
        };
        let mut folders = self.folders.clone();
        let removed = folders.remove(index);
        let key = self.paths.key(removed.dir());
        self.removed_ids
            .insert(key.clone(), removed.dir().to_owned());
        self.seen_again_ids.remove(&key);
        self.dirty = true;
        self.publish(folders, None);
        self.refresh_suggestions();
        self.changes_since(&before)
    }

    // ---- ordering, names and colours ----

    /// Classifies, groups by identity, names (distinct default labels and
    /// badges) and sorts, then keeps folders and identities. `now` is given
    /// when the caller knows the time: `~\.claude`'s timeline is observed
    /// only then.
    fn publish(&mut self, list: Vec<Folder>, now: Option<SystemTime>) {
        let paths = self.paths.clone();
        let mut folders: Vec<Folder> = list
            .into_iter()
            .filter_map(|mut folder| match self.layout.kind(&paths, folder.dir()) {
                Some(FolderKind::Infrastructure) => None,
                Some(kind) => {
                    folder.kind = kind;
                    Some(folder)
                }
                None => (folder.kind != FolderKind::Infrastructure).then_some(folder),
            })
            .collect();

        let grouping = identities::group(
            &folders,
            &self.identity_prefs,
            &self.forgotten_identity_keys,
            &self.saved_folder_ids,
            self.layout.extension_detected,
            &paths,
        );
        // Prefs are derived for good only once who is signed in where has
        // been read (before, every folder looks signed out).
        if self.identities_read && grouping.prefs != self.identity_prefs {
            let derived = grouping
                .prefs
                .keys()
                .any(|key| !self.identity_prefs.contains_key(key));
            self.identity_prefs = grouping.prefs.clone();
            if derived {
                self.dirty = true;
            }
        }
        self.identity_of_folder = grouping.identity_of_folder.clone();
        self.folder_keys = grouping.folder_keys.clone();
        self.default_owner = grouping.default_owner.clone();
        self.corrected_folders = grouping.corrected_folders.clone();
        self.forgotten_identity_folders = grouping
            .forgotten_folders
            .iter()
            .map(|folder| folder.dir().to_owned())
            .collect();
        // Tracking is an identity's choice: every folder of it follows (a
        // forgotten identity's folders are untracked).
        let hidden_by_identity: BTreeMap<&str, bool> = grouping
            .identities
            .iter()
            .map(|identity| (identity.id.as_str(), identity.is_hidden))
            .collect();
        for folder in &mut folders {
            if self.forgotten_identity_folders.contains(folder.dir()) {
                folder.is_hidden = true;
            } else if let Some(hidden) = grouping
                .identity_of_folder
                .get(folder.dir())
                .and_then(|identity| hidden_by_identity.get(identity.as_str()))
            {
                folder.is_hidden = *hidden;
            }
        }
        let named = naming::named(&folders, &paths);
        let sorted = sorted(named, &paths);
        // The grouping saw the folders before tracking and names were copied in.
        let current = |folder: &Folder| {
            sorted
                .iter()
                .find(|f| f.id == folder.id)
                .cloned()
                .unwrap_or_else(|| folder.clone())
        };
        self.identities = grouping
            .identities
            .into_iter()
            .map(|mut identity| {
                identity.run_dirs = identity.run_dirs.iter().map(current).collect();
                identity.store_dirs = identity.store_dirs.iter().map(current).collect();
                identity
            })
            .collect();
        self.unsigned_folders = grouping.unsigned_folders.iter().map(current).collect();
        self.folders = sorted;
        self.observe_default(now);
    }

    /// Notes who `~\.claude` names now in its timeline (once identities were
    /// read), and republishes what other threads read.
    fn observe_default(&mut self, now: Option<SystemTime>) {
        let paths = &self.paths;
        let default_dir = paths.default_config_dir();
        let default_folder = self
            .folders
            .iter()
            .find(|folder| folder.is_default(paths))
            .cloned();
        if let (Some(now), Some(folder), true, false) = (
            now,
            default_folder.as_ref(),
            self.identities_read,
            self.holds_fixtures,
        ) {
            let started = self.default_timeline.observe(
                self.folder_keys.get(folder.dir()).map(String::as_str),
                folder.account_uuid(),
                self.default_identity_modified_at,
                now,
                self.default_timeline_resumed,
            );
            self.default_timeline_resumed = false;
            if started {
                self.dirty = true;
            }
        }

        let mut rings = FolderRings {
            default_folder: Some(paths.key(&default_dir)),
            mirrors_default: self.layout.extension_detected,
            default_timeline: self.default_timeline.clone(),
            ..FolderRings::default()
        };
        for identity in &self.identities {
            rings
                .ring_of_identity
                .insert(identity.id.as_str().to_owned(), identity.ring_id.clone());
            for folder in identity.folders() {
                rings
                    .rings
                    .insert(paths.key(folder.dir()), identity.ring_id.clone());
            }
        }
        for (folder, identity) in self.identity_of_folder.iter().chain(&self.folder_keys) {
            rings
                .identity_of_folder
                .insert(paths.key(folder), identity.clone());
        }
        rings.untracked_identities = self
            .identities
            .iter()
            .filter(|identity| identity.is_hidden)
            .map(|identity| identity.id.as_str().to_owned())
            .chain(self.forgotten_identity_keys.iter().cloned())
            .collect();
        rings.untracked_folders = self
            .folders
            .iter()
            .filter(|folder| folder.is_hidden)
            .map(|folder| paths.key(folder.dir()))
            .collect();
        self.rings = rings;
    }

    // ---- what changed ----

    fn summary(&self) -> Summary {
        Summary {
            accounts: self.accounts(),
            folders: self.folders(),
            unsigned: self.unsigned_folders.iter().map(|f| f.id.clone()).collect(),
            suggestions: self.suggestions.clone(),
            folder_identities: self.rings.identity_of_folder.clone(),
            logins: self
                .identities
                .iter()
                .map(|identity| LoginSummary {
                    id: identity.id.clone(),
                    account_uuid: identity.account_uuid.clone(),
                    email: identity.email.clone(),
                    organization_uuid: identity.organization_uuid.clone(),
                    plan: identity.plan_name(),
                    is_hidden: identity.is_hidden,
                })
                .collect(),
        }
    }

    fn changes_since(&self, before: &Summary) -> AccountsChanged {
        let after = self.summary();
        let paths = &self.paths;
        let run_keys = |folders: &[RunFolder]| -> BTreeSet<String> {
            folders
                .iter()
                .filter(|folder| folder.kind == FolderKind::Run)
                .map(|folder| paths.key(folder.id.as_str()))
                .collect()
        };
        let all_keys = |folders: &[RunFolder]| -> BTreeSet<String> {
            folders
                .iter()
                .map(|folder| paths.key(folder.id.as_str()))
                .collect()
        };
        let (ran, runs) = (run_keys(&before.folders), run_keys(&after.folders));
        let known = all_keys(&after.folders);
        AccountsChanged {
            rings: before.accounts != after.accounts,
            folders: before.folders != after.folders
                || before.unsigned != after.unsigned
                || before.suggestions != after.suggestions,
            identities: before.folder_identities != after.folder_identities
                || before.logins != after.logins,
            new_run_folders: after
                .folders
                .iter()
                .filter(|folder| {
                    let key = paths.key(folder.id.as_str());
                    runs.contains(&key) && !ran.contains(&key)
                })
                .map(|folder| folder.id.clone())
                .collect(),
            removed_folders: before
                .folders
                .iter()
                .filter(|folder| !known.contains(&paths.key(folder.id.as_str())))
                .map(|folder| folder.id.clone())
                .collect(),
        }
    }
}

/// The default folder first, then by label (case-insensitive), then by path.
pub fn sorted(mut folders: Vec<Folder>, paths: &Paths) -> Vec<Folder> {
    folders.sort_by(|a, b| {
        b.is_default(paths)
            .cmp(&a.is_default(paths))
            .then_with(|| naming::compare_labels(&a.label(paths), &b.label(paths)))
            .then_with(|| a.id.cmp(&b.id))
    });
    folders
}

/// A folder-safe version of a name the user typed: ASCII letters, digits,
/// `-`, `_` and `.`, with spaces turned into dashes, lower-cased. `None` if
/// nothing is left.
pub fn sanitized_account_name(name: &str) -> Option<String> {
    let mut slug = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            slug.push(ch);
        } else if identities::is_inline_space(ch) {
            slug.push('-');
        }
    }
    let slug = slug.trim_start_matches(['.', '-']);
    // Windows drops the dots a name ends with: the folder made would not be
    // the folder named.
    let slug = slug.trim_end_matches('.');
    (!slug.is_empty()).then(|| slug.to_lowercase())
}
