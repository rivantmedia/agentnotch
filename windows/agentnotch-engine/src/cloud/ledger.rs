//! Which Claude account each session belonged to, remembered after it ends
//! (CL§6; the Mac's `SessionLedger.swift`, ported whole).
//!
//! With Claude Parallel Profiles every folder's `projects\` is a link to one
//! shared history, so a transcript names no account; only the running
//! session can be attributed (the hub's attribution: the folder it runs in,
//! and for a mirrored `~\.claude`, who it ran as when the process started).
//! The ledger captures that while the session is live and keeps it.
//!
//! One entry per session and account. A session resumed under another
//! account (the extension's "hit a limit, switch account, continue the same
//! conversation") keeps its id, so when a running session turns up under a
//! different account than it last ran as, the old account's part ends at
//! that moment and the new account's begins: the session's owners
//! ([`SessionOwner`]) say who ran it from when, and the transcript scanner
//! gives each account the responses made while it ran it. Each part is sent
//! as its own session row; the website keys sessions by account and id.
//!
//! Captured per part: the account (identity id and its contract key), the
//! project (name, and the resolved working directory its key is made from,
//! local only), the transcript path and config folder (local only, never
//! synced), where it ran (CLI, VS Code, Claude Desktop, SDK), start, last
//! activity and end, the model, Claude Code's cost estimate from the status
//! line (sent only for a session one account ran: it is the process's
//! total, which can't be split), and its title (never a prompt). Only
//! sessions of visible, remembered, signed-in accounts that the hub could
//! attribute for certain are captured: an unsure one is never guessed onto
//! an account.
//!
//! While the hub can't attribute a running session for certain (a mirrored
//! `~\.claude` around an account switch, a Claude Desktop session whose
//! record isn't found), or it runs as an account the website may not hear
//! of, its new responses count for no account: the session's owners get a
//! stretch of nobody (`""`), from the last activity seen while it was
//! certain, and the account's part ends there. Once it is certain again,
//! its account takes over from then (from its process's start for a new
//! process; for the same process, whose responses in between were its own
//! all along, from where nobody began). A session first seen unsure is
//! remembered as nobody's from its start (`unattributed`), so a later,
//! certain process of it counts only its own responses. A session the hub
//! simply hasn't placed yet (`waiting`: a folder not grouped yet, a Claude
//! Desktop session whose registry entry hasn't been read) is neither: it
//! runs on as it was, nothing paused, until the hub places it.
//!
//! A part that stops being live is ended a minute later (the time it went
//! away when that was seen while capturing, else its last activity), and
//! comes back to life if it shows up again.
//!
//! The backfill ([`super::backfill`]) adds sessions found on disk in
//! folders whose `projects\` is their own (not a link, and reached by no
//! other folder), and only those begun after the app first saw the folder
//! signed in as the account it names now ([`super::folder_logins`]):
//! earlier ones may be another account's. Live capture always wins over it.
//!
//! Kept in `cloud-ledger.json` (atomic, private); in memory only when
//! sealed. The ledger is driven with explicit times: it has no clock.

use super::contract::{date, SessionSource, Stamp};
use super::files::{lock, StateFile};
use super::keys;
use crate::core::paths::PathStyle;
use crate::model::{IdentityId, LiveSessionObservation};
use crate::platform::SecureFiles;
use crate::runtime_types::CloudAccount;
use crate::runtime_types::LiveBatch;
use serde::{Deserialize, Serialize};
use std::cmp::{max, min};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Where a ledger entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Seen running, attributed by the hub.
    Live,
    /// Found on disk in a folder only one account uses.
    Backfill,
}

/// One session (or, for a session more than one account ran, one account's
/// part of it) the ledger knows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudLedgerEntry {
    pub session_id: String,
    /// The engine's identity id (`uuid:…`), as of the last sighting.
    pub identity_id: IdentityId,
    /// The contract's account key.
    pub account_key: String,
    pub project_name: String,
    /// Local only: the working directory (`~` expanded, links resolved) the
    /// project key is made from. Never sent.
    pub project_path: String,
    /// Local only: never sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Local only: the config folder the session ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    pub source: SessionSource,
    #[serde(with = "date")]
    pub started_at: SystemTime,
    #[serde(with = "date")]
    pub last_activity_at: SystemTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub ended_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub origin: Origin,
}

impl CloudLedgerEntry {
    /// Where the ledger (and what was sent, and summaries) keep it: one per
    /// session and account.
    pub fn key(&self) -> String {
        Self::key_of(&self.session_id, &self.account_key)
    }

    pub fn key_of(session_id: &str, account_key: &str) -> String {
        format!("{session_id}|{account_key}")
    }
}

/// From `from` on (from the start when `None`), the session ran as
/// `account_key`, until the next owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOwner {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub from: Option<SystemTime>,
    pub account_key: String,
}

impl SessionOwner {
    pub fn new(from: Option<SystemTime>, account_key: &str) -> Self {
        SessionOwner {
            from,
            account_key: account_key.to_owned(),
        }
    }
}

/// One stretch of a session an account ran: from (the start when `None`) to
/// (no end when `None`).
pub type Stretch = (Option<SystemTime>, Option<SystemTime>);

/// A session's owners over time. Pure.
pub struct SessionOwners;

impl SessionOwners {
    /// Who ran the session at `at` (`None`: its start). `""` when nobody is
    /// known.
    pub fn owner_at(at: Option<SystemTime>, owners: &[SessionOwner]) -> String {
        let mut result = owners
            .first()
            .map(|o| o.account_key.clone())
            .unwrap_or_default();
        let Some(at) = at else {
            return result;
        };
        for owner in owners {
            if owner.from.is_some_and(|from| from > at) {
                break;
            }
            result = owner.account_key.clone();
        }
        result
    }

    /// Whether two lists name the same owner at every moment up to `end`
    /// (the latest line counted with `lhs`): the counts made with one are
    /// then right for the other. Owners change only at their `from`, so
    /// those moments (and the start) are the ones to compare.
    pub fn agree(lhs: &[SessionOwner], rhs: &[SessionOwner], end: Option<SystemTime>) -> bool {
        if lhs == rhs {
            return true;
        }
        let mut moments: Vec<Option<SystemTime>> = vec![None];
        if let Some(end) = end {
            moments.extend(
                lhs.iter()
                    .chain(rhs)
                    .filter_map(|o| o.from)
                    .filter(|from| *from <= end)
                    .map(Some),
            );
        }
        moments
            .into_iter()
            .all(|moment| Self::owner_at(moment, lhs) == Self::owner_at(moment, rhs))
    }

    /// When `account_key` ran the session: its stretches. `None` when the
    /// account was its only owner (the whole session is its).
    pub fn stretches(account_key: &str, owners: &[SessionOwner]) -> Option<Vec<Stretch>> {
        if owners.len() <= 1 {
            return None;
        }
        let mut result = Vec::new();
        for (index, owner) in owners.iter().enumerate() {
            if owner.account_key == account_key {
                result.push((owner.from, owners.get(index + 1).and_then(|next| next.from)));
            }
        }
        Some(result)
    }

    /// The same owners without a stretch that covers no time (one whose next
    /// owner starts at the same moment) or that repeats the owner before it:
    /// the owner at every moment is unchanged. Pure.
    pub fn normalized(owners: &[SessionOwner]) -> Vec<SessionOwner> {
        let mut result: Vec<SessionOwner> = Vec::new();
        for owner in owners {
            if result.last().is_some_and(|last| last.from == owner.from) {
                result.pop();
            }
            if result
                .last()
                .is_some_and(|last| last.account_key == owner.account_key)
            {
                continue;
            }
            result.push(owner.clone());
        }
        result
    }

    /// Whether `at` lies in one of `stretches` (a missing date lies in none).
    pub fn contains(stretches: &[Stretch], at: Option<SystemTime>) -> bool {
        let Some(at) = at else {
            return false;
        };
        stretches
            .iter()
            .any(|(from, to)| from.is_none_or(|from| at >= from) && to.is_none_or(|to| at < to))
    }
}

/// What the website is told about an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudLedgerAccount {
    pub identity_id: IdentityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The user's name for it in the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl CloudLedgerAccount {
    pub fn from_account(account: &CloudAccount) -> Self {
        CloudLedgerAccount {
            identity_id: account.identity_id.clone(),
            email: account.email.clone(),
            organization_name: account.organization_name.clone(),
            plan: account.plan.clone(),
            label: account.label.clone(),
        }
    }
}

/// `cloud-ledger.json`: the Mac's `SessionLedger.Contents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contents {
    /// 2: one entry per session and account, project paths kept locally.
    pub version: u32,
    /// By [`CloudLedgerEntry::key`].
    pub sessions: BTreeMap<String, CloudLedgerEntry>,
    /// By account key.
    pub accounts: BTreeMap<String, CloudLedgerAccount>,
    /// By session id: who ran it from when, for sessions more than one
    /// account ran (the others belong wholly to their one entry's account).
    pub owners: BTreeMap<String, Vec<SessionOwner>>,
    /// Sessions with no entry that were seen running while their account
    /// couldn't be told, and since when: their responses so far are no
    /// one's. (Optional: a ledger written before it still loads.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattributed: Option<BTreeMap<String, Stamp>>,
}

impl Contents {
    pub const CURRENT_VERSION: u32 = 2;

    fn new() -> Self {
        Contents {
            version: Self::CURRENT_VERSION,
            sessions: BTreeMap::new(),
            accounts: BTreeMap::new(),
            owners: BTreeMap::new(),
            unattributed: None,
        }
    }
}

/// A working directory as the project key uses it (`~` expanded, links
/// resolved).
pub type PathResolver = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The resolver of a given path style over the platform's canonical paths.
pub fn path_resolver(style: PathStyle, home: &Path, files: Arc<dyn SecureFiles>) -> PathResolver {
    let home = home.to_string_lossy().into_owned();
    Arc::new(move |cwd: &str| {
        keys::project_path_in(style, cwd, &home, &|path| {
            files
                .canonical(Path::new(path))
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
    })
}

/// How long a captured session may be missing before it counts as ended.
pub const END_GRACE: Duration = Duration::from_secs(60);
/// Most entries kept (the oldest by last activity go first): months of
/// heavy use. The website keeps what was sent.
pub const CAPACITY: usize = 10_000;
/// Last activity moves with every hook event: the file is written at most
/// this often (and when the app quits).
pub const WRITE_DELAY: Duration = Duration::from_secs(10);
/// Hand-overs between accounts kept per session at most: a session that
/// keeps changing hands (two windows running it at once) stops being split
/// further. Stretches of nobody are bounded on their own (by the same
/// number): a hand-over away from nobody is never refused, so a session is
/// never left counting for no one while the hub is certain.
pub const MAX_OWNERS: usize = 32;
/// Sessions first seen unsure kept at most (the oldest go first).
pub const UNATTRIBUTED_CAPACITY: usize = 2_000;
/// Nobody's stretch starts this long after the last certain activity
/// (transcript times have millisecond precision).
pub const UNCOUNTED_AFTER: Duration = Duration::from_millis(1);

pub const FILE_NAME: &str = "cloud-ledger.json";

struct Inner {
    contents: Contents,
    resolver: PathResolver,
    /// Session id → keys of its entries.
    keys_of_session: HashMap<String, BTreeSet<String>>,
    /// Entries seen live while capturing, and when each was last seen.
    seen_this_run: HashMap<String, SystemTime>,
    /// Captured entries missing from the live list, since when.
    missing_since: HashMap<String, SystemTime>,
    /// Start directory → project path (links resolved once).
    project_paths: HashMap<String, String>,
    /// Captured entries not ended yet (what `end_missing` looks at).
    open_ids: BTreeSet<String>,
}

pub struct SessionLedger {
    inner: Arc<Mutex<Inner>>,
    file: StateFile<Contents>,
    resolver: PathResolver,
}

impl SessionLedger {
    /// `file` in memory (`StateFile::memory()`) or in `<support>`, written at
    /// most every [`WRITE_DELAY`]; `resolver` makes a project path from a
    /// working directory (`path_resolver`).
    pub fn new(file: StateFile<Contents>, resolver: PathResolver) -> Self {
        let contents = match file.load() {
            Some(saved) if saved.version == Contents::CURRENT_VERSION => saved,
            _ => Contents::new(),
        };
        let mut keys_of_session: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut open_ids = BTreeSet::new();
        for (key, entry) in &contents.sessions {
            keys_of_session
                .entry(entry.session_id.clone())
                .or_default()
                .insert(key.clone());
            if entry.origin == Origin::Live && entry.ended_at.is_none() {
                open_ids.insert(key.clone());
            }
        }
        SessionLedger {
            inner: Arc::new(Mutex::new(Inner {
                contents,
                resolver: resolver.clone(),
                keys_of_session,
                seen_this_run: HashMap::new(),
                missing_since: HashMap::new(),
                project_paths: HashMap::new(),
                open_ids,
            })),
            file,
            resolver,
        }
    }

    /// The ledger of `<support>\cloud-ledger.json` (`persist` false: memory
    /// only, for a sealed run).
    pub fn in_support(
        support: &Path,
        files: Arc<dyn SecureFiles>,
        persist: bool,
        home: &Path,
    ) -> Self {
        let file = if persist {
            StateFile::new(
                Some(support.join(FILE_NAME)),
                Some(files.clone()),
                WRITE_DELAY,
            )
        } else {
            StateFile::memory()
        };
        Self::new(file, path_resolver(PathStyle::native(), home, files))
    }

    // ---- Reading ----

    pub fn entries(&self) -> Vec<CloudLedgerEntry> {
        lock(&self.inner)
            .contents
            .sessions
            .values()
            .cloned()
            .collect()
    }

    /// The session's entry of the account that ran it last.
    pub fn entry(&self, session_id: &str) -> Option<CloudLedgerEntry> {
        let inner = lock(&self.inner);
        let owner = inner.current_owner(session_id)?;
        inner
            .contents
            .sessions
            .get(&CloudLedgerEntry::key_of(session_id, &owner))
            .cloned()
    }

    pub fn entry_of(&self, session_id: &str, account_key: &str) -> Option<CloudLedgerEntry> {
        self.entry_by_key(&CloudLedgerEntry::key_of(session_id, account_key))
    }

    pub fn entry_by_key(&self, key: &str) -> Option<CloudLedgerEntry> {
        lock(&self.inner).contents.sessions.get(key).cloned()
    }

    /// Every account's entry of the session, earliest first.
    pub fn segments(&self, session_id: &str) -> Vec<CloudLedgerEntry> {
        let inner = lock(&self.inner);
        let mut result: Vec<CloudLedgerEntry> = inner
            .keys_of_session
            .get(session_id)
            .into_iter()
            .flatten()
            .filter_map(|key| inner.contents.sessions.get(key).cloned())
            .collect();
        result.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.account_key.cmp(&b.account_key))
        });
        result
    }

    /// Whether any account's entry of the session is known.
    pub fn knows(&self, session_id: &str) -> bool {
        lock(&self.inner).is_known(session_id)
    }

    /// Whether the session was seen running with its account unknown, and
    /// nobody's responses since are counted for any account.
    pub fn is_uncounted(&self, session_id: &str) -> bool {
        lock(&self.inner).current_owner(session_id).as_deref() == Some("")
    }

    /// Who ran the session from when: the kept owners of a session more than
    /// one account ran, else its one account from the start. Empty when the
    /// session isn't known.
    pub fn owners(&self, session_id: &str) -> Vec<SessionOwner> {
        let inner = lock(&self.inner);
        if let Some(owners) = inner.contents.owners.get(session_id) {
            if !owners.is_empty() {
                return owners.clone();
            }
        }
        inner
            .current_owner(session_id)
            .map(|owner| vec![SessionOwner::new(None, &owner)])
            .unwrap_or_default()
    }

    pub fn account(&self, account_key: &str) -> Option<CloudLedgerAccount> {
        lock(&self.inner)
            .contents
            .accounts
            .get(account_key)
            .cloned()
    }

    /// Entries (one per session and account).
    pub fn count(&self) -> usize {
        lock(&self.inner).contents.sessions.len()
    }

    // ---- Live capture ----

    /// Take the running sessions the hub attributed for certain, and the
    /// accounts they belong to. `live_ids` is every session running now,
    /// attributed or not (one the hub can't attribute this time is still
    /// running); `unsure` are those whose account the hub can't tell now;
    /// `waiting` those it hasn't placed yet (a folder not grouped yet, a
    /// Claude Desktop session whose registry entry hasn't been read): they
    /// are running, and neither counted nor paused until it has.
    ///
    /// A known session running but not among `live` (nor waiting) counts
    /// for no account from now on (see the module notes); an unknown one in
    /// `unsure` is remembered as no one's. Returns the keys of the entries
    /// that ended with this call.
    pub fn observe(
        &self,
        live: &[LiveSessionObservation],
        live_ids: &BTreeSet<String>,
        unsure: &BTreeSet<String>,
        waiting: &BTreeSet<String>,
        accounts: &BTreeMap<String, CloudLedgerAccount>,
        now: SystemTime,
    ) -> Vec<String> {
        let live: Vec<&LiveSessionObservation> = live
            .iter()
            .filter(|o| {
                keys::is_uuid(&o.session_id)
                    && keys::is_key(&o.account_key)
                    && !o.cwd.is_empty()
                    && !unsure.contains(&o.session_id)
            })
            .collect();
        // Resolve new start directories before taking the lock.
        let new_dirs: BTreeSet<&str> = {
            let inner = lock(&self.inner);
            live.iter()
                .map(|o| o.cwd.as_str())
                .filter(|dir| !inner.project_paths.contains_key(*dir))
                .collect()
        };
        let resolved: Vec<(String, String)> = new_dirs
            .into_iter()
            .map(|dir| (dir.to_owned(), (self.resolver)(dir)))
            .collect();

        self.mutate(|inner| {
            for (dir, path) in resolved {
                inner.project_paths.entry(dir).or_insert(path);
            }
            let mut changed = false;
            for (key, account) in accounts {
                if inner.contents.accounts.get(key) != Some(account) {
                    inner.contents.accounts.insert(key.clone(), account.clone());
                    changed = true;
                }
            }
            // A session running under two accounts at once (two windows):
            // which one a response came from can't be told, so it stays with
            // the account it ran as (or, new, isn't captured).
            let mut accounts_of: HashMap<&str, BTreeSet<&str>> = HashMap::new();
            for o in &live {
                accounts_of
                    .entry(o.session_id.as_str())
                    .or_default()
                    .insert(o.account_key.as_str());
            }
            let attributable: Vec<&LiveSessionObservation> = live
                .iter()
                .copied()
                .filter(|o| match accounts_of.get(o.session_id.as_str()) {
                    Some(set) if set.len() > 1 => {
                        inner.current_owner(&o.session_id).as_deref()
                            == Some(o.account_key.as_str())
                    }
                    _ => true,
                })
                .collect();
            for o in attributable {
                let key = CloudLedgerEntry::key_of(&o.session_id, &o.account_key);
                let mut part_start = None;
                if let Some(current) = inner.current_owner(&o.session_id) {
                    if current != o.account_key {
                        // Away from nobody always: the hub is certain now.
                        if !current.is_empty()
                            && inner.stretch_count(&o.session_id, false) >= MAX_OWNERS
                        {
                            continue;
                        }
                        part_start = Some(inner.hand_over(&o.session_id, &current, o, now));
                        changed = true;
                    }
                }
                inner.seen_this_run.insert(key.clone(), now);
                inner.missing_since.remove(&key);
                let is_split = inner
                    .contents
                    .owners
                    .get(&o.session_id)
                    .is_some_and(|owners| owners.len() > 1);
                let updated = inner.merged(o, part_start, is_split);
                inner.open_ids.insert(key.clone());
                inner
                    .keys_of_session
                    .entry(o.session_id.clone())
                    .or_default()
                    .insert(key.clone());
                if inner.contents.sessions.get(&key) != Some(&updated) {
                    inner.contents.sessions.insert(key, updated);
                    changed = true;
                }
            }
            // Running, but not as an account it may be counted for (a
            // session not placed yet waits: nothing is known of it yet).
            let mut ended: Vec<String> = Vec::new();
            let counted: BTreeSet<&str> = live.iter().map(|o| o.session_id.as_str()).collect();
            let not_placed: BTreeSet<&str> = waiting
                .iter()
                .filter(|id| !unsure.contains(*id))
                .map(String::as_str)
                .collect();
            let running: BTreeSet<&str> = live_ids
                .iter()
                .chain(unsure)
                .map(String::as_str)
                .filter(|id| !counted.contains(id) && !not_placed.contains(id))
                .collect();
            for id in running {
                if !keys::is_uuid(id) {
                    continue;
                }
                if inner.is_known(id) {
                    if inner.current_owner(id).as_deref() == Some("") {
                        continue;
                    }
                    let Some(stopped) = inner.stop_counting(id, now) else {
                        continue;
                    };
                    ended.extend(stopped);
                    changed = true;
                } else if unsure.contains(id) {
                    inner.note_unattributed(id, now);
                    changed = true;
                }
            }
            let present: BTreeSet<&str> = live_ids
                .iter()
                .map(String::as_str)
                .chain(not_placed.iter().copied())
                .chain(counted.iter().copied())
                .collect();
            ended.extend(inner.end_missing(&present, now));
            if !ended.is_empty() {
                changed = true;
            }
            (ended, changed)
        })
    }

    /// [`Self::observe`] for what the hub hands over.
    pub fn observe_batch(
        &self,
        batch: &LiveBatch,
        accounts: &BTreeMap<String, CloudLedgerAccount>,
    ) -> Vec<String> {
        self.observe(
            &batch.attributed,
            &batch.live_ids,
            &batch.unsure,
            &batch.waiting,
            accounts,
            batch.at,
        )
    }

    /// End the captured entries that have been missing from `live_ids` for
    /// [`END_GRACE`] (the regular tick, when no session changed). Returns
    /// their keys.
    pub fn settle(&self, live_ids: &BTreeSet<String>, now: SystemTime) -> Vec<String> {
        let present: BTreeSet<&str> = live_ids.iter().map(String::as_str).collect();
        self.mutate(|inner| {
            let ended = inner.end_missing(&present, now);
            let changed = !ended.is_empty();
            (ended, changed)
        })
    }

    /// Capture stopped (sync off, signed out): what was seen while it ran
    /// says nothing about when a session ended during the gap. A session
    /// found gone after capture resumes ends at its last activity.
    pub fn forget_run_state(&self) {
        let mut inner = lock(&self.inner);
        inner.seen_this_run.clear();
        inner.missing_since.clear();
    }

    // ---- Backfill ----

    /// Add sessions found on disk; a session the ledger already knows (under
    /// any account) is left as it is.
    pub fn record_backfill(
        &self,
        found: &[CloudLedgerEntry],
        accounts: &BTreeMap<String, CloudLedgerAccount>,
    ) -> usize {
        self.mutate(|inner| {
            let mut added = 0;
            // A session seen unsure is never backfilled onto an account either.
            for entry in found {
                if inner.is_known(&entry.session_id) {
                    continue;
                }
                let mut entry = entry.clone();
                entry.origin = Origin::Backfill;
                let key = entry.key();
                inner
                    .keys_of_session
                    .entry(entry.session_id.clone())
                    .or_default()
                    .insert(key.clone());
                inner.contents.sessions.insert(key, entry);
                added += 1;
            }
            for (key, account) in accounts {
                inner
                    .contents
                    .accounts
                    .entry(key.clone())
                    .or_insert_with(|| account.clone());
            }
            (added, added > 0)
        })
    }

    /// Fill in what a later look at the transcript learned (a title, the
    /// last activity, the end of a backfilled one that was still going when
    /// found).
    pub fn refine(
        &self,
        key: &str,
        last_activity_at: Option<SystemTime>,
        title: Option<&str>,
        transcript_path: Option<&str>,
        ended_at: Option<SystemTime>,
    ) {
        self.mutate(|inner| {
            let Some(before) = inner.contents.sessions.get(key) else {
                return ((), false);
            };
            let mut entry = before.clone();
            if let Some(last) = last_activity_at {
                entry.last_activity_at = max(entry.last_activity_at, last);
            }
            if entry.title.is_none() {
                entry.title = title.map(str::to_owned);
            }
            if entry.transcript_path.is_none() {
                entry.transcript_path = transcript_path.map(str::to_owned);
            }
            if let Some(ended) = ended_at {
                if entry.origin == Origin::Backfill && entry.ended_at.is_none() {
                    entry.ended_at = Some(max(ended, entry.last_activity_at));
                }
            }
            if &entry == before {
                return ((), false);
            }
            inner.contents.sessions.insert(key.to_owned(), entry);
            ((), true)
        });
    }

    // ---- Persistence ----

    /// Write the file now (quitting, tests).
    pub fn save_now(&self) {
        let snapshot = lock(&self.inner).contents.clone();
        self.file.save_now(&snapshot);
    }

    /// Waits until every background write has landed (tests).
    pub fn flush(&self) {
        self.file.flush();
    }

    /// Runs `change` on the state; when it says something changed, keeps the
    /// ledger within its capacity and schedules a write (after the lock is
    /// released: a write that can't be handed to a thread runs here).
    fn mutate<R>(&self, change: impl FnOnce(&mut Inner) -> (R, bool)) -> R {
        let (result, changed) = {
            let mut inner = lock(&self.inner);
            let (result, changed) = change(&mut inner);
            if changed {
                inner.trim();
            }
            (result, changed)
        };
        if changed {
            let shared = self.inner.clone();
            self.file.save(move || lock(&shared).contents.clone());
        }
        result
    }
}

impl Inner {
    /// The account that ran the session last (`""` for nobody: its account
    /// couldn't be told); `None` when the session is unknown.
    fn current_owner(&self, session_id: &str) -> Option<String> {
        if let Some(last) = self
            .contents
            .owners
            .get(session_id)
            .and_then(|owners| owners.last())
        {
            return Some(last.account_key.clone());
        }
        let latest = self
            .keys_of_session
            .get(session_id)
            .into_iter()
            .flatten()
            .filter_map(|key| self.contents.sessions.get(key))
            .max_by(|a, b| {
                a.last_activity_at
                    .cmp(&b.last_activity_at)
                    .then_with(|| a.account_key.cmp(&b.account_key))
            });
        if let Some(entry) = latest {
            return Some(entry.account_key.clone());
        }
        self.contents
            .unattributed
            .as_ref()
            .is_some_and(|u| u.contains_key(session_id))
            .then(String::new)
    }

    /// Any entry, owner or unsure sighting of the session.
    fn is_known(&self, session_id: &str) -> bool {
        self.keys_of_session
            .get(session_id)
            .is_some_and(|keys| !keys.is_empty())
            || self.contents.owners.contains_key(session_id)
            || self
                .contents
                .unattributed
                .as_ref()
                .is_some_and(|u| u.contains_key(session_id))
    }

    /// How many of the session's kept owners are nobody's stretches
    /// (`nobody`), or accounts'.
    fn stretch_count(&self, session_id: &str, nobody: bool) -> usize {
        match self.contents.owners.get(session_id) {
            None => usize::from(!nobody),
            Some(owners) => owners
                .iter()
                .filter(|o| o.account_key.is_empty() == nobody)
                .count(),
        }
    }

    /// The session's responses after its last activity seen while its
    /// account was certain (`UNCOUNTED_AFTER` later, so a response written
    /// at that very moment stays its account's; never before an earlier
    /// hand-over) are no one's, and its account's part ends at that
    /// activity. Returns that part's key if it was open; `None` when nothing
    /// changed: a session that already lost its account `MAX_OWNERS` times
    /// stops being split further and stays with the account it ran as.
    fn stop_counting(&mut self, session_id: &str, now: SystemTime) -> Option<Vec<String>> {
        let current = self.current_owner(session_id)?;
        if current.is_empty() || self.stretch_count(session_id, true) >= MAX_OWNERS {
            return None;
        }
        let key = CloudLedgerEntry::key_of(session_id, &current);
        let mut owners = self
            .contents
            .owners
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| vec![SessionOwner::new(None, &current)]);
        let mut boundary = self
            .contents
            .sessions
            .get(&key)
            .map_or(now, |entry| entry.last_activity_at + UNCOUNTED_AFTER);
        if let Some(previous) = owners.last().and_then(|o| o.from) {
            boundary = max(boundary, previous);
        }
        owners.push(SessionOwner::new(Some(boundary), ""));
        self.set_owners(session_id, owners);
        let mut ended = Vec::new();
        if let Some(entry) = self.contents.sessions.get_mut(&key) {
            if entry.ended_at.is_none() {
                entry.ended_at = Some(entry.last_activity_at);
                if self.open_ids.contains(&key) {
                    ended.push(key.clone());
                }
            }
        }
        self.open_ids.remove(&key);
        self.missing_since.remove(&key);
        // The Mac logs here that the session's responses count for no
        // account until it can be told; the engine has no log.
        Some(ended)
    }

    /// A session the ledger has no entry of runs unsure.
    fn note_unattributed(&mut self, session_id: &str, now: SystemTime) {
        let unattributed = self.contents.unattributed.get_or_insert_with(BTreeMap::new);
        if unattributed.contains_key(session_id) {
            return;
        }
        unattributed.insert(session_id.to_owned(), Stamp(now));
        if unattributed.len() > UNATTRIBUTED_CAPACITY {
            let mut by_age: Vec<(SystemTime, String)> = unattributed
                .iter()
                .map(|(id, stamp)| (stamp.0, id.clone()))
                .collect();
            by_age.sort();
            for (_, id) in by_age
                .into_iter()
                .take(unattributed.len() - UNATTRIBUTED_CAPACITY)
            {
                unattributed.remove(&id);
            }
        }
    }

    /// Keep the session's owners, without empty or repeated stretches; a
    /// session back to one owner from its start that is its only entry's
    /// needs none kept.
    fn set_owners(&mut self, session_id: &str, owners: Vec<SessionOwner>) {
        let owners = SessionOwners::normalized(&owners);
        let only_entry_is_theirs = match owners.as_slice() {
            [only] if only.from.is_none() && !only.account_key.is_empty() => {
                let theirs = CloudLedgerEntry::key_of(session_id, &only.account_key);
                self.keys_of_session
                    .get(session_id)
                    .is_none_or(|keys| keys.iter().all(|key| *key == theirs))
            }
            _ => false,
        };
        if only_entry_is_theirs {
            self.contents.owners.remove(session_id);
        } else {
            self.contents.owners.insert(session_id.to_owned(), owners);
        }
    }

    /// The session, last run as `current`, now runs as the observation's
    /// account. The old account's part ends when the new one began (its
    /// process's start, never before the old part's last activity or an
    /// earlier hand-over, never after now). Returns that moment. From nobody
    /// (`current` is `""`), the same rule: a new process takes over from its
    /// start, the process that ran through the unsure stretch from where it
    /// began (the stretch goes).
    fn hand_over(
        &mut self,
        session_id: &str,
        current: &str,
        observation: &LiveSessionObservation,
        now: SystemTime,
    ) -> SystemTime {
        let old_key = CloudLedgerEntry::key_of(session_id, current);
        let mut owners = self
            .contents
            .owners
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| vec![SessionOwner::new(None, current)]);
        let mut boundary = min(
            observation
                .process_started_at
                .unwrap_or(observation.started_at),
            now,
        );
        if let Some(old) = self.contents.sessions.get(&old_key) {
            boundary = max(boundary, old.last_activity_at);
        }
        if let Some(previous) = owners.last().and_then(|o| o.from) {
            boundary = max(boundary, previous);
        }
        owners.push(SessionOwner::new(Some(boundary), &observation.account_key));
        self.set_owners(session_id, owners);
        if let Some(unattributed) = self.contents.unattributed.as_mut() {
            unattributed.remove(session_id);
        }
        if let Some(old) = self.contents.sessions.get_mut(&old_key) {
            if old.ended_at.is_none() {
                old.ended_at = Some(max(boundary, old.last_activity_at));
            }
        }
        self.open_ids.remove(&old_key);
        self.missing_since.remove(&old_key);
        boundary
    }

    /// `part_start`: where a new part of a split session begins; `is_split`:
    /// the session has more than one owner, so a part's start is where its
    /// account took over, not when the app first saw the process.
    fn merged(
        &self,
        observation: &LiveSessionObservation,
        part_start: Option<SystemTime>,
        is_split: bool,
    ) -> CloudLedgerEntry {
        let key = CloudLedgerEntry::key_of(&observation.session_id, &observation.account_key);
        let path = self
            .project_paths
            .get(&observation.cwd)
            .cloned()
            .unwrap_or_else(|| (self.resolver)(&observation.cwd));
        let Some(mut entry) = self.contents.sessions.get(&key).cloned() else {
            let start = part_start
                .unwrap_or_else(|| min(observation.started_at, observation.last_activity_at));
            return CloudLedgerEntry {
                session_id: observation.session_id.clone(),
                identity_id: observation.identity_id.clone(),
                account_key: observation.account_key.clone(),
                project_name: keys::project_name(&observation.cwd),
                project_path: path,
                transcript_path: observation.transcript_path.clone(),
                config_dir: observation.config_dir.clone(),
                source: SessionSource::from_entrypoint(observation.entrypoint.as_deref()),
                started_at: start,
                last_activity_at: max(observation.last_activity_at, start),
                ended_at: None,
                model: observation.model.clone(),
                cost_usd: observation.cost_usd,
                title: observation.title.clone(),
                origin: Origin::Live,
            };
        };
        // A backfilled session seen running is the hub's from now on.
        if entry.origin == Origin::Backfill {
            entry.origin = Origin::Live;
            entry.project_name = keys::project_name(&observation.cwd);
            entry.project_path = path;
        }
        // The account's identity id can change (an organization split); its
        // key is what stays.
        entry.identity_id = observation.identity_id.clone();
        if observation.transcript_path.is_some() {
            entry.transcript_path = observation.transcript_path.clone();
        }
        if observation.config_dir.is_some() {
            entry.config_dir = observation.config_dir.clone();
        }
        if entry.source == SessionSource::Other {
            entry.source = SessionSource::from_entrypoint(observation.entrypoint.as_deref());
        }
        if !is_split {
            entry.started_at = min(entry.started_at, observation.started_at);
        }
        entry.last_activity_at = max(entry.last_activity_at, observation.last_activity_at);
        entry.ended_at = None;
        if observation.model.is_some() {
            entry.model = observation.model.clone();
        }
        if observation.cost_usd.is_some() {
            entry.cost_usd = observation.cost_usd;
        }
        if observation.title.is_some() {
            entry.title = observation.title.clone();
        }
        entry
    }

    /// Ends the open live entries whose session isn't among `present` and
    /// has been missing for [`END_GRACE`].
    fn end_missing(&mut self, present: &BTreeSet<&str>, now: SystemTime) -> Vec<String> {
        let mut ended = Vec::new();
        for key in self.open_ids.clone() {
            let Some(entry) = self
                .contents
                .sessions
                .get(&key)
                .filter(|e| e.origin == Origin::Live && e.ended_at.is_none())
            else {
                self.open_ids.remove(&key);
                continue;
            };
            if present.contains(entry.session_id.as_str()) {
                continue;
            }
            let Some(since) = self.missing_since.get(&key).copied() else {
                self.missing_since.insert(key, now);
                continue;
            };
            if !now
                .duration_since(since)
                .is_ok_and(|gone| gone >= END_GRACE)
            {
                continue;
            }
            // Seen going away while capturing: then. Otherwise (it ended
            // while the app wasn't running, or wasn't capturing): its last
            // activity.
            let at = if self.seen_this_run.contains_key(&key) {
                max(since, entry.last_activity_at)
            } else {
                entry.last_activity_at
            };
            if let Some(entry) = self.contents.sessions.get_mut(&key) {
                entry.ended_at = Some(at);
            }
            self.missing_since.remove(&key);
            self.open_ids.remove(&key);
            ended.push(key);
        }
        ended
    }

    /// Keep within [`CAPACITY`]: the oldest by last activity go first.
    fn trim(&mut self) {
        if self.contents.sessions.len() <= CAPACITY {
            return;
        }
        let mut by_age: Vec<(SystemTime, String)> = self
            .contents
            .sessions
            .iter()
            .map(|(key, entry)| (entry.last_activity_at, key.clone()))
            .collect();
        by_age.sort();
        let excess = self.contents.sessions.len() - CAPACITY;
        for (_, key) in by_age.into_iter().take(excess) {
            let Some(entry) = self.contents.sessions.remove(&key) else {
                continue;
            };
            self.open_ids.remove(&key);
            if let Some(keys) = self.keys_of_session.get_mut(&entry.session_id) {
                keys.remove(&key);
                if keys.is_empty() {
                    self.keys_of_session.remove(&entry.session_id);
                    self.contents.owners.remove(&entry.session_id);
                }
            }
        }
    }
}

/// Where a ledger sits for a given support folder.
pub fn file_path(support: &Path) -> PathBuf {
    support.join(FILE_NAME)
}
