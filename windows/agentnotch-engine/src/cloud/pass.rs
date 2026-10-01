//! One sync pass's work (CL§5.5-5.7, §7.3; the Mac's `CloudSync.swift`
//! 54-660): what the website already has (`cloud-sync-state.json`), the
//! files the service keeps, the backfill, the transcripts' totals, what
//! changed since it was sent, and the requests to send it in. No HTTP here:
//! the service sends. Every time is an argument, so a pass is deterministic
//! given the stores it is handed.
//!
//! The website never learns a path: a project is its key (an HMAC with this
//! install's secret) and its name (the last folder's name).

use super::backfill;
use super::contract::{
    self, date, limit, SessionSource, SyncAccount, SyncDevice, SyncRequest, SyncSession,
};
use super::files::{install_secret, lock, StateFile};
use super::folder_logins::CloudFolderLogins;
use super::keys;
use super::ledger::{CloudLedgerAccount, CloudLedgerEntry, Origin, SessionLedger, SessionOwner};
use super::recorder::{RecordedUsageReading, UsageHistoryRecorder};
use super::scanner::{self, seconds_of, SessionTokenScanner, SessionTokenSummary};
use super::summary::store::{Candidate, Entry as SummaryEntry, SessionSummaryStore};
use crate::core::paths::Paths;
use crate::model::IdentityId;
use crate::platform::SecureFiles;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---- What the service reads ----

/// One Claude account as the website may hear of it: visible, remembered,
/// signed in, with an account UUID to derive its key from (the Mac's
/// `CloudAccountInfo`; `environment::account_infos` makes them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAccountInfo {
    pub identity_id: IdentityId,
    pub account_key: String,
    pub account_uuid: String,
    pub email: Option<String>,
    pub organization_name: Option<String>,
    pub plan: Option<String>,
    /// What the app calls it (the ring's nickname or the account's name).
    pub label: Option<String>,
}

impl CloudAccountInfo {
    pub fn contract(&self) -> SyncAccount {
        SyncAccount {
            key: self.account_key.clone(),
            email: self.email.clone(),
            organization_name: self.organization_name.clone(),
            plan: self.plan.clone(),
            label: self.label.clone(),
        }
    }

    pub fn ledger_account(&self) -> CloudLedgerAccount {
        CloudLedgerAccount {
            identity_id: self.identity_id.clone(),
            email: self.email.clone(),
            organization_name: self.organization_name.clone(),
            plan: self.plan.clone(),
            label: self.label.clone(),
        }
    }
}

// ---- What was sent ----

/// A transcript as it was when a session was built from it: its size and
/// when it was last written (a stat tells whether it moved since).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TranscriptStamp {
    pub bytes: u64,
    /// Epoch seconds, as the scanner keeps them (`scanner::seconds_of`).
    pub modified: f64,
}

impl TranscriptStamp {
    /// The file's now; `None` when it can't be read.
    pub fn of(files: &dyn SecureFiles, path: &str) -> Option<TranscriptStamp> {
        let identity = files.identity(Path::new(path)).ok()?;
        Some(TranscriptStamp {
            bytes: identity.size,
            modified: seconds_of(identity.modified_ns),
        })
    }

    /// The same file in the same state: equal sizes and times to the
    /// microsecond (a double that went through a file may come back one
    /// step off, which must not make a quiet transcript look written to).
    pub fn matches(&self, other: &TranscriptStamp) -> bool {
        self.bytes == other.bytes && (self.modified - other.modified).abs() < 1e-6
    }
}

/// A session as last sent: the hash of its payload without the summary, and
/// of the summary sent with it; whether it had ended; the transcript it was
/// built from; how it was built ([`CloudSyncPass::PAYLOAD_VERSION`], `None`
/// before there was one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(rename = "base")]
    pub base: String,
    #[serde(rename = "summary", default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(rename = "ended", default)]
    pub ended: bool,
    #[serde(
        rename = "transcript",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub transcript: Option<TranscriptStamp>,
    #[serde(rename = "version", default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
}

impl Record {
    /// A record with no summary, not ended, from no transcript: what a
    /// caller that only has a hash makes.
    pub fn new(base: impl Into<String>) -> Record {
        Record {
            base: base.into(),
            summary: None,
            ended: false,
            transcript: None,
            version: None,
        }
    }
}

/// `cloud-sync-state.json`: what the website has, kept per website user
/// (another user or website starts over), by ledger entry key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contents {
    /// 2: by ledger entry key, with `ended` and `transcript`.
    #[serde(rename = "version")]
    pub version: u32,
    #[serde(rename = "userId", default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(rename = "website", default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(rename = "sessions", default)]
    pub sessions: BTreeMap<String, Record>,
    #[serde(
        rename = "lastSyncAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::option"
    )]
    pub last_sync_at: Option<SystemTime>,
    #[serde(
        rename = "dashboardUrl",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dashboard_url: Option<String>,
}

impl Contents {
    pub const CURRENT_VERSION: u32 = 2;
}

impl Default for Contents {
    fn default() -> Self {
        Contents {
            version: Self::CURRENT_VERSION,
            user_id: None,
            website: None,
            sessions: BTreeMap::new(),
            last_sync_at: None,
            dashboard_url: None,
        }
    }
}

pub const SYNC_STATE_FILE: &str = "cloud-sync-state.json";

/// What the website already has, so an unchanged session isn't sent again.
pub struct CloudSyncMemory {
    contents: Arc<Mutex<Contents>>,
    file: StateFile<Contents>,
}

impl CloudSyncMemory {
    /// `file` in memory or `<support>\cloud-sync-state.json`. A file of
    /// another version or a damaged one starts fresh.
    pub fn new(file: StateFile<Contents>) -> Self {
        let contents = match file.load() {
            Some(saved) if saved.version == Contents::CURRENT_VERSION => saved,
            _ => Contents::default(),
        };
        CloudSyncMemory {
            contents: Arc::new(Mutex::new(contents)),
            file,
        }
    }

    pub fn in_support(support: &Path, files: Arc<dyn SecureFiles>, persist: bool) -> Self {
        let file = if persist {
            StateFile::new(
                Some(support.join(SYNC_STATE_FILE)),
                Some(files),
                Duration::ZERO,
            )
        } else {
            StateFile::memory()
        };
        Self::new(file)
    }

    fn save(&self) {
        let shared = self.contents.clone();
        self.file.save(move || lock(&shared).clone());
    }

    pub fn last_sync_at(&self) -> Option<SystemTime> {
        lock(&self.contents).last_sync_at
    }

    pub fn dashboard_url(&self) -> Option<String> {
        lock(&self.contents).dashboard_url.clone()
    }

    pub fn user_id(&self) -> Option<String> {
        lock(&self.contents).user_id.clone()
    }

    pub fn sent_count(&self) -> usize {
        lock(&self.contents).sessions.len()
    }

    /// A copy of everything kept (tests, the fixture round trip).
    pub fn contents(&self) -> Contents {
        lock(&self.contents).clone()
    }

    /// Signed in as `user_id` on `website`: what was sent to anyone else is
    /// forgotten.
    pub fn adopt(&self, user_id: Option<&str>, website: &str, dashboard_url: Option<&str>) {
        {
            let mut contents = lock(&self.contents);
            if contents.user_id.as_deref() != user_id
                || contents.website.as_deref() != Some(website)
            {
                *contents = Contents::default();
                contents.user_id = user_id.map(str::to_owned);
                contents.website = Some(website.to_owned());
            }
            if let Some(url) = dashboard_url {
                contents.dashboard_url = Some(url.to_owned());
            }
        }
        self.save();
    }

    /// What was last sent for the ledger entry.
    pub fn sent(&self, key: &str) -> Option<Record> {
        lock(&self.contents).sessions.get(key).cloned()
    }

    /// Whether the session has changed since it was sent (or a new summary
    /// came). Pure given the memory.
    pub fn needs_sending(&self, key: &str, record: &Record) -> bool {
        let contents = lock(&self.contents);
        let Some(sent) = contents.sessions.get(key) else {
            return true;
        };
        if sent.base != record.base {
            return true;
        }
        matches!(&record.summary, Some(summary) if sent.summary.as_ref() != Some(summary))
    }

    /// The session was built again, from a transcript that moved or in a
    /// newer way, and came out as it was sent: remember the transcript and
    /// the way, so the next pass needs only a stat.
    pub fn note_unchanged(&self, key: &str, record: &Record) {
        {
            let mut contents = lock(&self.contents);
            let Some(sent) = contents.sessions.get_mut(key) else {
                return;
            };
            if sent.transcript == record.transcript && sent.version == record.version {
                return;
            }
            sent.transcript = record.transcript;
            sent.version = record.version;
        }
        self.save();
    }

    pub fn mark_sent(&self, records: &BTreeMap<String, Record>, at: SystemTime) {
        {
            let mut contents = lock(&self.contents);
            for (key, record) in records {
                let previous = contents.sessions.get(key).and_then(|r| r.summary.clone());
                contents.sessions.insert(
                    key.clone(),
                    Record {
                        base: record.base.clone(),
                        summary: record.summary.clone().or(previous),
                        ended: record.ended,
                        transcript: record.transcript,
                        version: record.version,
                    },
                );
            }
            contents.last_sync_at = Some(at);
        }
        self.save();
    }

    pub fn note_synced(&self, at: SystemTime) {
        lock(&self.contents).last_sync_at = Some(at);
        self.save();
    }

    pub fn save_now(&self) {
        let snapshot = lock(&self.contents).clone();
        self.file.save_now(&snapshot);
    }

    /// Waits until every background write has landed (tests).
    pub fn flush(&self) {
        self.file.flush();
    }
}

// ---- The files ----

/// The service's state on disk (in memory only when not persisted), and this
/// install's secret for project keys.
pub struct CloudStores {
    pub ledger: SessionLedger,
    pub scanner: SessionTokenScanner,
    pub recorder: UsageHistoryRecorder,
    pub summaries: SessionSummaryStore,
    pub memory: CloudSyncMemory,
    pub folder_logins: CloudFolderLogins,
    /// What a stat and a canonical path go through.
    pub files: Arc<dyn SecureFiles>,
    /// The path rules of this PC and its home folder.
    pub paths: Paths,
    support: std::path::PathBuf,
    persist: bool,
    secret: Mutex<Option<Vec<u8>>>,
}

impl CloudStores {
    /// The stores of `support` (`persist` false: in memory only, for a
    /// sealed run). Nothing is read from or written to the disk until a
    /// store is asked, and the install secret not until a pass needs it.
    pub fn new(support: &Path, files: Arc<dyn SecureFiles>, persist: bool, home: &Path) -> Self {
        let paths = Paths::native(home);
        CloudStores {
            ledger: SessionLedger::in_support(support, files.clone(), persist, home),
            scanner: SessionTokenScanner::in_support(support, files.clone(), persist),
            recorder: UsageHistoryRecorder::in_support(support, files.clone(), persist),
            summaries: SessionSummaryStore::in_support(support, files.clone(), persist),
            memory: CloudSyncMemory::in_support(support, files.clone(), persist),
            folder_logins: CloudFolderLogins::in_support(
                support,
                files.clone(),
                persist,
                paths.clone(),
            ),
            files,
            paths,
            support: support.to_path_buf(),
            persist,
            secret: Mutex::new(None),
        }
    }

    /// The install secret, read (or made) the first time a project key is
    /// needed, which only a sync pass does (signed in, with sync on): never
    /// at launch. Kept in the folder when persisted, else made for this run
    /// only (as it is when the folder can't be written).
    pub fn secret(&self) -> Vec<u8> {
        let mut made = lock(&self.secret);
        if let Some(secret) = made.as_ref() {
            return secret.clone();
        }
        let loaded = if self.persist {
            install_secret::load(&self.support, &*self.files)
        } else {
            None
        };
        let secret = loaded
            .or_else(|| install_secret::random().map(|bytes| bytes.to_vec()))
            .unwrap_or_else(fallback_secret);
        *made = Some(secret.clone());
        secret
    }

    pub fn save_now(&self) {
        self.ledger.save_now();
        self.scanner.save_now();
        self.recorder.save_now();
        self.summaries.save_now();
        self.memory.save_now();
        self.folder_logins.save_now();
    }

    /// Waits until every background write has landed (tests).
    pub fn flush(&self) {
        self.ledger.flush();
        self.scanner.flush();
        self.recorder.flush();
        self.summaries.flush();
        self.memory.flush();
        self.folder_logins.flush();
    }
}

/// A secret for this run when the system's random source gave none: the
/// time, the process and an address hashed together. Never written down:
/// project keys made with it differ from the next run's, which is better
/// than sending none.
fn fallback_secret() -> Vec<u8> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let marker = 0u8;
    let seed = format!("{nanos}|{}|{:p}", std::process::id(), &marker);
    Sha256::digest(seed.as_bytes()).to_vec()
}

// ---- A pass ----

/// What a pass is given.
#[derive(Debug, Clone)]
pub struct Input {
    pub accounts: Vec<CloudAccountInfo>,
    /// `None`: no backfill this pass.
    pub backfill_folders: Option<Vec<backfill::Folder>>,
    pub include_summaries: bool,
    pub device: SyncDevice,
    pub now: SystemTime,
    /// This PC's user names, to scrub from summaries as they are sent
    /// (`summary::text::LocalNames::current`).
    pub known_names: Vec<String>,
    pub max_sessions_per_request: usize,
    pub max_requests: usize,
}

impl Input {
    pub fn new(accounts: Vec<CloudAccountInfo>, device: SyncDevice, now: SystemTime) -> Input {
        Input {
            accounts,
            backfill_folders: None,
            include_summaries: false,
            device,
            now,
            known_names: Vec::new(),
            max_sessions_per_request: limit::SESSIONS,
            max_requests: CloudSyncPass::MAX_REQUESTS_PER_PASS,
        }
    }
}

/// One request and what to remember once it is accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub request: SyncRequest,
    /// By ledger entry key.
    pub records: BTreeMap<String, Record>,
    pub readings: Vec<RecordedUsageReading>,
}

impl Batch {
    /// The batch with no summary in it (summaries were turned off during the
    /// pass): the website keeps what it has, and what was sent is
    /// remembered without them.
    pub fn without_summaries(&self) -> Batch {
        let mut copy = self.clone();
        for session in &mut copy.request.sessions {
            session.summary = None;
        }
        for record in copy.records.values_mut() {
            record.summary = None;
        }
        copy
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub batches: Vec<Batch>,
    /// Changed sessions waiting to be sent (all of them, not only this
    /// pass's).
    pub session_count: usize,
    pub usage_count: usize,
    /// More than `max_requests` were needed: the rest go next pass.
    pub has_more: bool,
    pub backfilled: usize,
}

/// What a payload is built from, for one ledger entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    pub session: SyncSession,
    pub transcript: Option<TranscriptStamp>,
}

pub struct CloudSyncPass;

impl CloudSyncPass {
    /// A backfilled session whose transcript moved in the last half hour may
    /// still be running.
    pub const BACKFILL_OPEN_WINDOW: Duration = Duration::from_secs(30 * 60);
    /// New transcripts the backfill reads per pass (the first pass on a long
    /// history spreads over a few).
    pub const BACKFILL_FILES_PER_PASS: usize = 200;
    /// Requests a pass sends at most, back to back; the rest go in the next
    /// pass, 30 seconds on. Well under the website's burst (12 a user,
    /// shared by all of the user's computers), so a catch-up isn't refused.
    pub const MAX_REQUESTS_PER_PASS: usize = 5;
    /// How a session is built for the website. One sent as ended by an
    /// earlier way is built again once, and sent again if it comes out
    /// different. 2: sessions whose cost Claude Code didn't report, or
    /// reported where it can't be used (split between accounts, or outgrown
    /// by the session), are priced from their transcripts.
    pub const PAYLOAD_VERSION: u32 = 2;

    pub fn prepare(input: &Input, stores: &CloudStores) -> Prepared {
        let allowed = allowed_accounts(&input.accounts);
        let mut backfilled = 0;
        if let Some(folders) = &input.backfill_folders {
            // Transcripts Claude Code deleted since (their totals were sent).
            stores.scanner.prune_missing_files();
            backfilled = Self::backfill(folders, &allowed, stores, input.now);
        }

        let mut changed: Vec<(SyncSession, Record)> = Vec::new();
        // Oldest first: an original session claims its responses before a
        // fork that copied them.
        let mut entries: Vec<CloudLedgerEntry> = stores
            .ledger
            .entries()
            .into_iter()
            .filter(|entry| allowed.contains_key(&entry.account_key))
            .collect();
        entries.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.key().cmp(&b.key()))
        });
        for entry in &entries {
            let key = entry.key();
            let sent = stores.memory.sent(&key);
            // Sent as ended, the way sessions are built now, from a
            // transcript that hasn't moved since, and no new summary:
            // nothing can have changed (a stat, no read).
            if let (Some(sent), Some(path)) = (&sent, &entry.transcript_path) {
                if sent.ended
                    && sent.version == Some(Self::PAYLOAD_VERSION)
                    && entry.ended_at.is_some()
                    && sent.transcript.as_ref().is_some_and(|stamp| {
                        TranscriptStamp::of(&*stores.files, path)
                            .is_some_and(|now| now.matches(stamp))
                    })
                    && !Self::has_new_summary(
                        &key,
                        sent,
                        stores,
                        input.include_summaries,
                        &input.known_names,
                    )
                {
                    continue;
                }
            }
            // Ended with no response of its account in it (so never sent),
            // read since its transcript last moved: nothing to report.
            if sent.is_none() && entry.ended_at.is_some() {
                if let Some(path) = &entry.transcript_path {
                    if let Some(cached) = stores.scanner.cached_summary(&entry.session_id) {
                        let stamp = TranscriptStamp {
                            bytes: cached.transcript_bytes,
                            modified: cached.transcript_modified,
                        };
                        if cached.part(&entry.account_key).message_count == 0
                            && TranscriptStamp::of(&*stores.files, path)
                                .is_some_and(|now| now.matches(&stamp))
                        {
                            continue;
                        }
                    }
                }
            }
            let Some(built) = Self::payload(
                entry,
                stores,
                input.include_summaries,
                &input.known_names,
                input.now,
            ) else {
                continue;
            };
            let record = Self::record(&built.session, built.transcript);
            if stores.memory.needs_sending(&key, &record) {
                changed.push((built.session, record));
            } else {
                stores.memory.note_unchanged(&key, &record);
            }
        }
        stores.scanner.save();

        // Readings of accounts that may not be mentioned (any more) go.
        stores
            .recorder
            .discard(|reading| !allowed.contains_key(&reading.account_key));
        let readings = stores.recorder.pending(usize::MAX);
        let session_count = changed.len();
        let usage_count = readings.len();
        let all = CloudBatcher::batches(
            changed,
            readings,
            &allowed,
            &input.device,
            input.max_sessions_per_request,
        );
        let has_more = all.len() > input.max_requests;
        Prepared {
            batches: all.into_iter().take(input.max_requests).collect(),
            session_count,
            usage_count,
            has_more,
            backfilled,
        }
    }

    /// A summary made since the session was last sent (and summaries are
    /// on).
    pub fn has_new_summary(
        key: &str,
        sent: &Record,
        stores: &CloudStores,
        include_summaries: bool,
        known_names: &[String],
    ) -> bool {
        if !include_summaries {
            return false;
        }
        let Some(summary) = stores.summaries.summary(key) else {
            return false;
        };
        Self::summary_hash(&summary, known_names) != sent.summary
    }

    /// The hash a summary is remembered by once sent ([`Self::record`]'s).
    /// Pure.
    pub fn summary_hash(summary: &SummaryEntry, known_names: &[String]) -> Option<String> {
        Some(keys::sha256_hex(contract::to_json(
            &summary.contract(known_names),
        )))
    }

    /// Whether the website this PC is signed in to has the summary (it was
    /// sent with its session, as it is now).
    pub fn was_sent(
        summary: &SummaryEntry,
        key: &str,
        stores: &CloudStores,
        known_names: &[String],
    ) -> bool {
        match stores.memory.sent(key).and_then(|r| r.summary) {
            Some(sent) => Self::summary_hash(summary, known_names).as_deref() == Some(&sent),
            None => false,
        }
    }

    /// The session (this entry's account's part of it) as the website gets
    /// it, from the ledger and its transcripts, read again now (an unchanged
    /// file costs a stat), and the transcript it came from. `None` when that
    /// account has no response in it (nothing to report).
    pub fn payload(
        entry: &CloudLedgerEntry,
        stores: &CloudStores,
        include_summaries: bool,
        known_names: &[String],
        now: SystemTime,
    ) -> Option<Payload> {
        let key = entry.key();
        let mut path = entry.transcript_path.clone();
        if path.is_none() {
            if let Some(config_dir) = &entry.config_dir {
                // Seen only in the session registry so far: look for it.
                path = search_transcript(&stores.paths, &entry.session_id, config_dir);
            }
        }
        let owners = stores.ledger.owners(&entry.session_id);
        let mut totals: Option<SessionTokenSummary> = None;
        if let Some(path) = &path {
            totals = stores.scanner.scan(&entry.session_id, path, &owners);
            if totals.is_some() && entry.transcript_path.is_none() {
                stores.ledger.refine(&key, None, None, Some(path), None);
            }
        }
        if totals.is_none() {
            totals = stores.scanner.cached_summary(&entry.session_id);
        }
        let totals = totals?;
        let part = totals.part(&entry.account_key);
        if part.message_count <= 0 {
            return None;
        }
        if let Some(last) = part.last_timestamp {
            // Found on disk still going, quiet since: it ended.
            let ends = entry.origin == Origin::Backfill
                && entry.ended_at.is_none()
                && contract::seconds_between(now, last) > Self::BACKFILL_OPEN_WINDOW.as_secs_f64();
            stores
                .ledger
                .refine(&key, Some(last), None, None, ends.then_some(last));
        }

        // The transcript's first line is when the session began; the
        // ledger's time is when the app first saw it running.
        let started_at = part.first_timestamp.unwrap_or(entry.started_at);
        // Likewise the last line is its last activity (the app may have
        // first seen it, idle, long after). An end is never before it: a
        // session found gone after capture paused ends at its last line.
        let last_activity_at = part
            .last_timestamp
            .unwrap_or(entry.last_activity_at)
            .max(started_at);
        let ended_at = stores
            .ledger
            .entry_by_key(&key)
            .and_then(|e| e.ended_at)
            .or(entry.ended_at)
            .map(|ended| ended.max(last_activity_at));
        let mut models = part.models.clone();
        if models.is_empty() {
            if let Some(model) = entry.model.as_deref().filter(|m| !m.is_empty()) {
                models = vec![model.to_owned()];
            }
        }
        let mut source = entry.source;
        if source == SessionSource::Other {
            source = SessionSource::from_entrypoint(totals.entrypoint.as_deref());
        }
        // A session more than one account ran: the status line's cost is
        // the whole process's (a resumed session starts from the total it
        // had), which can't be divided between them, so no part carries it.
        // Its tokens are split exactly, by who ran it when. Split means
        // another owner (nobody included) has responses in it: a stretch of
        // nobody with none (a session placed a moment after it started)
        // takes nothing from it.
        let split = Self::is_split(&totals, &entry.account_key);
        // Claude Code's own figure when it gave one, else the part's
        // responses at list prices: sessions no status line ran for (the VS
        // Code extension's chat panel, Claude Desktop, the SDK), found only
        // on disk, or split. Claude Code's counts calls the transcript
        // doesn't record, so it is the larger unless it no longer covers the
        // session (continued where no status line runs, or by a process
        // that didn't restore the earlier total): then the estimate is.
        let reported = if split { None } else { entry.cost_usd };
        let cost_usd = [reported, part.estimated_cost_usd()]
            .into_iter()
            .flatten()
            .filter(|cost| !cost.is_nan())
            .fold(None, |best: Option<f64>, cost| {
                Some(best.map_or(cost, |b| b.max(cost)))
            });
        let session = SyncSession {
            account_key: entry.account_key.clone(),
            session_id: entry.session_id.clone(),
            project: contract::SyncProject {
                key: keys::project_key(&entry.account_key, &entry.project_path, &stores.secret()),
                name: entry.project_name.clone(),
            },
            title: entry.title.clone().or_else(|| totals.title.clone()),
            source,
            models,
            started_at,
            last_activity_at,
            ended_at,
            message_count: part.message_count,
            tokens: part.tokens.contract(),
            cost_usd,
            summary: if include_summaries {
                stores
                    .summaries
                    .summary(&key)
                    .map(|summary| summary.contract(known_names))
            } else {
                None
            },
        };
        Some(Payload {
            session,
            transcript: Some(TranscriptStamp {
                bytes: totals.transcript_bytes,
                modified: totals.transcript_modified,
            }),
        })
    }

    /// Whether anyone but `account_key` (nobody included) made responses in
    /// the session, as the transcript was counted by its owners. Pure.
    pub fn is_split(totals: &SessionTokenSummary, account_key: &str) -> bool {
        totals
            .parts
            .iter()
            .any(|(key, part)| key != account_key && part.message_count > 0)
    }

    /// What is remembered of a sent session: hashes of its payload without
    /// its summary and of the summary, whether it had ended, the transcript
    /// it was built from, and how. Pure.
    pub fn record(session: &SyncSession, transcript: Option<TranscriptStamp>) -> Record {
        let mut base = session.clone();
        base.summary = None;
        let base_hash = keys::sha256_hex(contract::to_json(&base));
        let summary_hash = session
            .summary
            .as_ref()
            .map(|summary| keys::sha256_hex(contract::to_json(summary)));
        Record {
            base: base_hash,
            summary: summary_hash,
            ended: session.ended_at.is_some(),
            transcript,
            version: Some(Self::PAYLOAD_VERSION),
        }
    }

    /// Add the sessions found on disk in folders only one account uses,
    /// begun after the folder was first seen signed in as that account.
    /// Oldest first (by their first own line, then by file creation), so an
    /// original claims its responses before a copy resumed or forked from
    /// it. Returns how many were added.
    pub fn backfill(
        folders: &[backfill::Folder],
        accounts: &BTreeMap<String, CloudAccountInfo>,
        stores: &CloudStores,
        now: SystemTime,
    ) -> usize {
        struct Found {
            session_id: String,
            path: String,
            root: usize,
            first: SystemTime,
            born: Option<SystemTime>,
        }
        let roots = backfill::roots(folders, &stores.paths, &*stores.files);
        let mut found: Vec<Found> = Vec::new();
        for (index, root) in roots.iter().enumerate() {
            if !accounts.contains_key(&root.account_key) {
                continue;
            }
            for (session_id, path) in
                scanner::session_files(Path::new(&root.projects), stores.paths.style())
            {
                if stores.ledger.knows(&session_id) {
                    continue;
                }
                let Some(first) = stores.scanner.first_timestamp(&path, &session_id) else {
                    continue;
                };
                if first <= root.signed_in_since {
                    continue;
                }
                found.push(Found {
                    born: scanner::birth_time(Path::new(&path)),
                    session_id,
                    path,
                    root: index,
                    first,
                });
            }
        }
        found.sort_by(|a, b| {
            a.first
                .cmp(&b.first)
                .then_with(|| {
                    // No creation time sorts last.
                    match (a.born, b.born) {
                        (Some(x), Some(y)) => x.cmp(&y),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                })
                .then_with(|| a.path.cmp(&b.path))
        });

        let mut added: Vec<CloudLedgerEntry> = Vec::new();
        let mut budget = Self::BACKFILL_FILES_PER_PASS;
        for candidate in &found {
            if budget == 0 {
                break;
            }
            let root = &roots[candidate.root];
            if !accounts.contains_key(&root.account_key) {
                continue;
            }
            // Only transcripts never read count: one looked at before (and
            // left out) is a stat to check again.
            if stores
                .scanner
                .cached_summary(&candidate.session_id)
                .is_none()
            {
                budget -= 1;
            }
            let owners = [SessionOwner::new(None, &root.account_key)];
            let Some(totals) = stores
                .scanner
                .scan(&candidate.session_id, &candidate.path, &owners)
            else {
                continue;
            };
            let (Some(cwd), Some(first), Some(last)) = (
                totals.cwd.as_deref().filter(|cwd| !cwd.is_empty()),
                totals.first_timestamp,
                totals.last_timestamp,
            ) else {
                continue;
            };
            if totals.message_count <= 0 || first <= root.signed_in_since {
                continue;
            }
            let source = SessionSource::from_entrypoint(totals.entrypoint.as_deref());
            // Claude Desktop runs its sessions as whoever it is signed in
            // as, not as the folder: only its record of a running one tells
            // whose.
            if source == SessionSource::Desktop
                || contract::is_desktop_hosted(totals.entrypoint.as_deref())
            {
                continue;
            }
            added.push(CloudLedgerEntry {
                session_id: candidate.session_id.clone(),
                identity_id: root.identity_id.clone(),
                account_key: root.account_key.clone(),
                project_name: keys::project_name(cwd),
                project_path: keys::project_path(
                    cwd,
                    Path::new(stores.paths.home()),
                    &*stores.files,
                ),
                transcript_path: Some(candidate.path.clone()),
                config_dir: Some(root.config_dir.clone()),
                source,
                started_at: first,
                last_activity_at: last,
                ended_at: (contract::seconds_between(now, last)
                    > Self::BACKFILL_OPEN_WINDOW.as_secs_f64())
                .then_some(last),
                model: totals.models.first().cloned(),
                cost_usd: None,
                title: totals.title.clone(),
                origin: Origin::Backfill,
            });
        }
        if added.is_empty() {
            return 0;
        }
        stores.ledger.record_backfill(&added, &BTreeMap::new())
    }

    /// Ended sessions (each account's part) of the allowed accounts that
    /// could be summarised, with their responses so far: only those that
    /// ended after `since` (when summaries were last turned on).
    pub fn summary_candidates(
        accounts: &[CloudAccountInfo],
        stores: &CloudStores,
        since: Option<SystemTime>,
    ) -> Vec<Candidate> {
        let Some(since) = since else {
            return Vec::new();
        };
        let allowed = allowed_accounts(accounts);
        stores
            .ledger
            .entries()
            .into_iter()
            .filter_map(|entry| {
                let account = allowed.get(&entry.account_key)?;
                let ended_at = entry.ended_at.filter(|ended| *ended >= since)?;
                let path = entry.transcript_path.clone()?;
                let totals = stores.scanner.cached_summary(&entry.session_id)?;
                Some(Candidate {
                    key: entry.key(),
                    session_id: entry.session_id.clone(),
                    identity_id: account.identity_id.clone(),
                    account_key: entry.account_key.clone(),
                    transcript_path: path,
                    message_count: totals.part(&entry.account_key).message_count,
                    ended_at,
                })
            })
            .collect()
    }
}

/// The accounts by key; the first of two with one key.
pub fn allowed_accounts(accounts: &[CloudAccountInfo]) -> BTreeMap<String, CloudAccountInfo> {
    let mut map = BTreeMap::new();
    for account in accounts {
        map.entry(account.account_key.clone())
            .or_insert_with(|| account.clone());
    }
    map
}

/// `<config dir>\projects\<any folder>\<session id>.jsonl`, the first that
/// exists (folders in name order, so the answer doesn't depend on the file
/// system's listing order). `None` for an id that isn't a plain file name.
pub fn search_transcript(paths: &Paths, session_id: &str, config_dir: &str) -> Option<String> {
    let safe = !session_id.is_empty()
        && session_id != "."
        && session_id != ".."
        && !session_id.contains(['/', '\\', ':']);
    if !safe {
        return None;
    }
    let projects = paths.join(&paths.normalize(config_dir), "projects");
    let mut folders: Vec<String> = std::fs::read_dir(&projects)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    folders.sort();
    let file_name = format!("{session_id}.jsonl");
    folders.into_iter().find_map(|folder| {
        let candidate = paths.join(&paths.join(&projects, &folder), &file_name);
        Path::new(&candidate).is_file().then_some(candidate)
    })
}

// ---- The batcher ----

/// What a request may hold: the contract's limits (tests give smaller).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    pub sessions: usize,
    pub usage: usize,
    pub accounts: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        BatchLimits {
            sessions: limit::SESSIONS,
            usage: limit::USAGE,
            accounts: limit::ACCOUNTS,
        }
    }
}

/// Splits what is to be sent into requests the website takes: at most 200
/// sessions, 500 readings and 50 accounts each, every account a session or
/// reading names listed in its request. Pure.
pub struct CloudBatcher;

impl CloudBatcher {
    pub fn batches(
        sessions: Vec<(SyncSession, Record)>,
        readings: Vec<RecordedUsageReading>,
        accounts: &BTreeMap<String, CloudAccountInfo>,
        device: &SyncDevice,
        max_sessions: usize,
    ) -> Vec<Batch> {
        Self::batches_within(
            sessions,
            readings,
            accounts,
            device,
            BatchLimits {
                sessions: max_sessions,
                ..BatchLimits::default()
            },
        )
    }

    pub fn batches_within(
        sessions: Vec<(SyncSession, Record)>,
        readings: Vec<RecordedUsageReading>,
        accounts: &BTreeMap<String, CloudAccountInfo>,
        device: &SyncDevice,
        limits: BatchLimits,
    ) -> Vec<Batch> {
        struct Open {
            batch: Batch,
            keys: std::collections::BTreeSet<String>,
        }
        impl Open {
            fn new(device: &SyncDevice) -> Open {
                Open {
                    batch: Batch {
                        request: SyncRequest::new(device.clone()),
                        records: BTreeMap::new(),
                        readings: Vec::new(),
                    },
                    keys: Default::default(),
                }
            }
        }
        let mut batches: Vec<Batch> = Vec::new();
        let mut current = Open::new(device);

        fn close(
            current: &mut Open,
            batches: &mut Vec<Batch>,
            accounts: &BTreeMap<String, CloudAccountInfo>,
            device: &SyncDevice,
        ) {
            let done = std::mem::replace(current, Open::new(device));
            if !done.batch.request.sessions.is_empty() || !done.batch.request.usage.is_empty() {
                let mut batch = done.batch;
                batch.request.accounts = done
                    .keys
                    .iter()
                    .filter_map(|key| accounts.get(key).map(CloudAccountInfo::contract))
                    .collect();
                batches.push(batch);
            }
        }
        let make_room =
            |current: &mut Open, batches: &mut Vec<Batch>, key: &str, is_session: bool| {
                let full = if is_session {
                    current.batch.request.sessions.len() >= limits.sessions
                } else {
                    current.batch.request.usage.len() >= limits.usage
                };
                if full || (!current.keys.contains(key) && current.keys.len() >= limits.accounts) {
                    close(current, batches, accounts, device);
                }
                current.keys.insert(key.to_owned());
            };

        for (session, record) in sessions {
            if !accounts.contains_key(&session.account_key) {
                continue;
            }
            make_room(&mut current, &mut batches, &session.account_key, true);
            current.batch.records.insert(
                CloudLedgerEntry::key_of(&session.session_id, &session.account_key),
                record,
            );
            current.batch.request.sessions.push(session);
        }
        for reading in readings {
            if !accounts.contains_key(&reading.account_key) {
                continue;
            }
            make_room(&mut current, &mut batches, &reading.account_key, false);
            current.batch.request.usage.push(reading.contract());
            current.batch.readings.push(reading);
        }
        close(&mut current, &mut batches, accounts, device);
        batches
    }
}
