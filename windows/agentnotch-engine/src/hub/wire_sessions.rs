//! Sessions, the review queue and chat, wired into the hub (design §4.5,
//! HS§5): WP5's `SessionStore` takes every input in arrival order (hook
//! events, status lines, held requests and their answers, registry reads,
//! transcript syncs, Desktop lookups, interrupts, review actions, ticks at
//! its own deadlines), and what an input causes goes on: held requests to
//! close, jobs to run (the review file through the hub's one writer per
//! file), account sightings to the registry, chat changes to the panel.
//!
//! - Every config folder's registry is read at the start (the launch
//!   baseline closes once all are in) and every 3 s after, each physical
//!   `sessions` folder once.
//! - Every `SyncTranscript` job comes back as `TranscriptSynced`, a failed
//!   one too (with the cursor where it was), or the store would wait on it
//!   for ever.
//! - The transcripts of running turns are watched for an interrupt every
//!   250 ms.
//! - `review-state.json` is read at load, handed over at the first start and
//!   written whenever the store asks; a stop writes it at once.
//!
//! Owner: WP7.

use super::api::{CallError, DataUrlReply, HubEvent, TextReply};
use super::core_state::{dump_line, to_value, Core};
use crate::core::paths::Paths;
use crate::core::time::from_ms;
use crate::model::{
    Attribution, DesktopCandidate, IdentityId, SessionId, SessionState, SessionView,
};
use crate::model::{ChatPage, RegistrySnapshot};
use crate::platform::Platform;
use crate::runtime_types::{
    AccountsChanged, Job, JobId, PersistFile, ReviewAction, SessionEffects, SessionInput,
    TranscriptDelta, VersionSighting, VersionSource,
};
use crate::sessions::interrupt::{InterruptWatcher, POLL_INTERVAL};
use crate::sessions::registry::{
    grouped_by_sessions_folder, is_linked_sessions_folder, SessionsFolderGroup, SCAN_INTERVAL,
};
use crate::sessions::{desktop, SessionStore};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// The session store's schedule and the jobs it waits on.
pub(crate) struct SessionsWiring {
    /// `review-state.json` as read at load, until the first start.
    review_bytes: Option<Vec<u8>>,
    /// The review file went in (the first start): from then on the store's
    /// queue is the one to keep.
    pub(crate) review_loaded: bool,
    /// The physical `sessions` folders and whether one is reached through a
    /// link (its entries may belong to other config folders then).
    groups: Vec<(SessionsFolderGroup, bool)>,
    /// When every registry is next read.
    next_scan: Option<SystemTime>,
    /// The registry reads in flight (a scan never piles onto one).
    scans: BTreeSet<JobId>,
    /// The launch's reads not back yet; `None` once the baseline closed.
    launch_scan: Option<BTreeSet<JobId>>,
    /// Desktop lookups in flight, by the host session id they are for.
    hosted: BTreeMap<JobId, String>,
    interrupts: InterruptWatcher,
    next_poll: Option<SystemTime>,
}

impl SessionsWiring {
    pub(crate) fn new(review_bytes: Option<Vec<u8>>) -> SessionsWiring {
        SessionsWiring {
            review_bytes,
            review_loaded: false,
            groups: Vec::new(),
            next_scan: None,
            scans: BTreeSet::new(),
            launch_scan: None,
            hosted: BTreeMap::new(),
            interrupts: InterruptWatcher::new(),
            next_poll: None,
        }
    }
}

/// The store a run uses: the real paths, processes (the periodic check
/// drops sessions whose process is gone or was replaced) and files, and
/// Claude Desktop's folders unless sealed.
pub(crate) fn session_store(
    paths: Paths,
    platform: &Platform,
    desktop: &[std::path::PathBuf],
    sealed: bool,
) -> SessionStore {
    let mut store = SessionStore::new()
        .with_paths(paths)
        .with_processes(platform.processes.clone())
        .with_files(platform.files.clone());
    if !sealed {
        store.set_desktop_roots(desktop.to_vec());
    }
    store
}

impl Core {
    /// The hub starts: the review file goes in (the first time), every
    /// registry is read, and the 3-second scans begin.
    pub(crate) fn sessions_on_start(&mut self, now: SystemTime) {
        self.sessions_accounts_changed(None, now);
        if !self.sessions_w.review_loaded {
            let bytes = self.sessions_w.review_bytes.take();
            self.sessions.load_review(bytes.as_deref(), now);
            self.sessions_w.review_loaded = true;
            let reads = self.scan_registries();
            if reads.is_empty() {
                self.sessions.initial_scan_completed(now);
            } else {
                self.sessions_w.launch_scan = Some(reads);
            }
        }
        self.sessions_w.next_scan = Some(now + SCAN_INTERVAL);
    }

    /// While the hub runs: the store's deadlines, the scans and the
    /// interrupt watch.
    pub(crate) fn drive_sessions(&mut self, now: SystemTime) {
        if self.sessions.next_deadline().is_some_and(|at| at <= now) {
            self.sessions_input(SessionInput::Tick, now);
        }
        if self.sessions_w.next_scan.is_some_and(|at| at <= now) {
            self.sessions_w.next_scan = Some(now + SCAN_INTERVAL);
            if self.sessions_w.scans.is_empty() {
                self.scan_registries();
            }
        }
        self.watch_interrupts(now);
    }

    /// The panel's open chats: what they gained since the last input.
    pub(crate) fn publish_chats(&mut self) {
        if !self.sessions.has_open_chats() {
            return;
        }
        for update in self.sessions.take_chat_updates() {
            self.push_event(HubEvent::Chat(update));
        }
    }

    pub(crate) fn sessions_deadline(&self) -> Option<SystemTime> {
        [
            self.sessions.next_deadline(),
            self.sessions_w.next_scan,
            self.sessions_w.next_poll,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// One input to the store, and what it caused.
    pub(crate) fn sessions_input(&mut self, input: SessionInput, now: SystemTime) {
        let effects = self.sessions.apply(input, now);
        self.session_effects(effects, now);
    }

    fn session_effects(&mut self, effects: SessionEffects, now: SystemTime) {
        for release in effects.release {
            self.release_held(release);
        }
        for job in effects.jobs {
            self.session_job(job);
        }
        for sighting in effects.sightings {
            let changed = self.registry.record(sighting, now);
            self.accounts_changed(&changed, now);
        }
        // `effects.transitions` (the attention news) are the reactions'
        // (chime, peek, toasts), which aren't wired in this build.
    }

    fn session_job(&mut self, job: Job) {
        match job {
            // Through the one writer of the file: an older write never
            // lands after a newer one.
            Job::Persist {
                file: PersistFile::Review,
                bytes,
            } => self.persist(PersistFile::Review, bytes),
            Job::DesktopHosted {
                roots,
                host_session_id,
                candidates,
            } => {
                let id = self.schedule(
                    Job::DesktopHosted {
                        roots,
                        host_session_id: host_session_id.clone(),
                        candidates,
                    },
                    None,
                );
                self.sessions_w.hosted.insert(id, host_session_id);
            }
            job => {
                self.schedule(job, None);
            }
        }
    }

    // ---- registries ----

    /// The folders whose registries are read, and the identities a Desktop
    /// session may belong to, after the accounts changed; the store hears
    /// of folders that went.
    pub(crate) fn sessions_accounts_changed(
        &mut self,
        changed: Option<&AccountsChanged>,
        now: SystemTime,
    ) {
        let dirs: Vec<String> = self
            .registry
            .folders()
            .iter()
            .map(|folder| folder.config_dir.to_string_lossy().into_owned())
            .collect();
        let paths = self.registry.paths().clone();
        let files = self.platform.files.clone();
        let groups = grouped_by_sessions_folder(&dirs, &paths, files.as_ref());
        self.sessions.set_registry_folders(groups.clone());
        self.sessions_w.groups = groups
            .into_iter()
            .map(|group| {
                let linked = group
                    .aliases
                    .iter()
                    .any(|alias| is_linked_sessions_folder(alias, &paths, files.as_ref()));
                (group, linked)
            })
            .collect();
        let candidates: Vec<DesktopCandidate> = self
            .registry
            .identities()
            .iter()
            .filter_map(|identity| {
                Some(DesktopCandidate {
                    identity_id: identity.id.clone(),
                    account_uuid: identity.account_uuid.clone()?,
                    organization_uuid: identity.organization_uuid.clone(),
                })
            })
            .collect();
        self.sessions.set_desktop_candidates(candidates);
        if let Some(changed) = changed.filter(|changed| changed.any()) {
            self.sessions_input(SessionInput::AccountsChanged(changed.clone()), now);
        }
    }

    /// One read of every physical `sessions` folder; the jobs.
    fn scan_registries(&mut self) -> BTreeSet<JobId> {
        let groups = self.sessions_w.groups.clone();
        let mut reads = BTreeSet::new();
        for (group, via_link) in groups {
            let id = self.schedule(
                Job::ReadRegistry {
                    sessions_dir: group.folder,
                    via_link,
                },
                None,
            );
            reads.insert(id);
        }
        self.sessions_w.scans.extend(reads.iter().copied());
        reads
    }

    pub(crate) fn registry_read(&mut self, id: JobId, snapshot: RegistrySnapshot, now: SystemTime) {
        self.sessions_w.scans.remove(&id);
        let versions: Vec<String> = snapshot
            .entries
            .iter()
            .filter_map(|entry| entry.version.clone())
            .collect();
        for version in versions {
            self.note_version_sighting(
                VersionSighting {
                    source: VersionSource::Registry,
                    path: None,
                    version: Some(version),
                },
                now,
            );
        }
        self.sessions_input(SessionInput::Registry(snapshot), now);
        if let Some(waiting) = self.sessions_w.launch_scan.as_mut() {
            waiting.remove(&id);
            if waiting.is_empty() {
                self.sessions_w.launch_scan = None;
                self.sessions.initial_scan_completed(now);
            }
        }
    }

    pub(crate) fn transcript_synced(&mut self, delta: TranscriptDelta, now: SystemTime) {
        self.sessions_input(SessionInput::TranscriptSynced(delta), now);
    }

    pub(crate) fn chat_page_read(&mut self, page: ChatPage, now: SystemTime) {
        let effects = self.sessions.chat_loaded(page, now);
        self.session_effects(effects, now);
    }

    /// A Desktop lookup came back: every session of that host session id
    /// hears whose it is (or that it is no one's yet).
    pub(crate) fn hosted_read(&mut self, id: JobId, identity: Option<IdentityId>, now: SystemTime) {
        let Some(host) = self.sessions_w.hosted.remove(&id) else {
            return;
        };
        let sessions: Vec<SessionId> = self
            .sessions
            .views()
            .into_iter()
            .filter(|view| view.host_session_id.as_deref() == Some(host.as_str()))
            .map(|view| view.id)
            .collect();
        for session in sessions {
            self.sessions_input(
                SessionInput::Hosted {
                    session,
                    identity: identity.clone(),
                },
                now,
            );
        }
    }

    /// The running turns' transcripts, looked at every 250 ms.
    fn watch_interrupts(&mut self, now: SystemTime) {
        let watches = self.sessions.interrupt_watches();
        let paths = self.registry.paths().clone();
        self.sessions_w
            .interrupts
            .set_watches(&watches, now, |a, b| {
                paths.same(&a.to_string_lossy(), &b.to_string_lossy())
            });
        if self.sessions_w.interrupts.is_empty() {
            self.sessions_w.next_poll = None;
            return;
        }
        // A new watch's first look is now.
        if self.sessions_w.next_poll.is_none_or(|at| at <= now) {
            self.sessions_w.next_poll = Some(now + POLL_INTERVAL);
            for input in self.sessions_w.interrupts.poll(now) {
                self.sessions_input(input, now);
            }
        }
    }

    /// The sessions as the hub places them (ClaudeControlHub's recompute):
    /// one the registry made is attributed by its folder now (the store
    /// can't know whose a folder is), and one Claude Desktop hosts by its
    /// record only (`theHubAttributesDesktopSessionsByTheirRecordOnly`):
    /// the identity whose record names it, else unsure.
    pub(crate) fn session_views(&self) -> Vec<SessionView> {
        self.sessions
            .views()
            .into_iter()
            .map(|mut view| {
                if view.attribution == Attribution::Known(None) {
                    if let Some(folder) = &view.account {
                        view.attribution = self.registry.attribution(folder, view.pid_started);
                    }
                }
                view.attribution = desktop::attribution(
                    &view.attribution,
                    view.is_desktop_hosted,
                    view.desktop_identity.clone(),
                );
                view
            })
            .collect()
    }

    /// The hooks were turned off: no request can be answered any more,
    /// and no session is hook-backed.
    pub(crate) fn sessions_hooks_turned_off(&mut self, now: SystemTime) {
        self.release_held(crate::runtime_types::Release::All);
        self.sessions_input(SessionInput::Review(ReviewAction::HooksTurnedOff), now);
    }

    /// The review file's bytes now (a stop), once the store has it.
    pub(crate) fn review_bytes_now(&mut self, now: SystemTime) -> Option<Vec<u8>> {
        if !self.sessions_w.review_loaded {
            return None;
        }
        self.sessions.review_file_now(now)
    }

    // ---- calls ----

    /// A moment the page sent: never later than now (a click can't be in
    /// the future, and a bad clock mustn't mark later turns reviewed).
    fn page_time(&self, at_ms: u64, now: SystemTime) -> SystemTime {
        from_ms(at_ms).min(now)
    }

    fn review(&mut self, action: ReviewAction, now: SystemTime) -> Result<Value, CallError> {
        self.sessions_input(SessionInput::Review(action), now);
        Ok(json!({}))
    }

    /// `mark_reviewed {session_id, at_ms}`: reviewed as of the click.
    pub(crate) fn mark_reviewed_call(
        &mut self,
        session: SessionId,
        at_ms: u64,
    ) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        let at = self.page_time(at_ms, now);
        self.review(ReviewAction::MarkReviewed { session, at }, now)
    }

    /// `mark_viewed {session_id, completed_at_ms}`: only that completion.
    pub(crate) fn mark_viewed_call(
        &mut self,
        session: SessionId,
        completed_at_ms: u64,
    ) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        let completed_at = from_ms(completed_at_ms);
        self.review(
            ReviewAction::MarkViewed {
                session,
                completed_at,
            },
            now,
        )
    }

    /// `mark_all_reviewed {session_ids, at_ms}` (after the page's undo).
    pub(crate) fn mark_all_call(
        &mut self,
        sessions: Vec<SessionId>,
        at_ms: u64,
    ) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        let at = self.page_time(at_ms, now);
        self.review(ReviewAction::MarkAll { sessions, at }, now)
    }

    pub(crate) fn dismiss_failure_call(&mut self, session: SessionId) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        self.review(ReviewAction::DismissFailure(session), now)
    }

    pub(crate) fn reset_review_call(&mut self) -> Result<Value, CallError> {
        let now = self.platform.clock.now();
        self.review(ReviewAction::Reset, now)
    }

    /// `session_state_text` → `{text}`: Advanced › Copy, a line per session
    /// for bug reports (the Mac's `SessionStateDump.summary`).
    pub(crate) fn session_state_text_call(&self) -> Result<Value, CallError> {
        let text = self
            .session_views()
            .iter()
            .map(state_text_line)
            .collect::<Vec<_>>()
            .join("\n");
        to_value(&TextReply { text })
    }

    /// `chat_open {session_id}`: the session counts as reviewed, its chat
    /// is read and pushed (`an:chat`, a reset first).
    pub(crate) fn chat_open_call(&mut self, session: &SessionId) -> Result<Value, CallError> {
        if self.sessions.session(session).is_none() {
            return Err(CallError::not_found("That session has ended."));
        }
        let now = self.platform.clock.now();
        let effects = self.sessions.open_chat(session, now);
        self.session_effects(effects, now);
        Ok(json!({}))
    }

    pub(crate) fn chat_close_call(&mut self, session: &SessionId) -> Result<Value, CallError> {
        self.sessions.close_chat(session);
        Ok(json!({}))
    }

    /// `chat_more {session_id, before_id}`: the page before, read and pushed.
    pub(crate) fn chat_more_call(
        &mut self,
        session: &SessionId,
        before_id: &str,
    ) -> Result<Value, CallError> {
        for job in self.sessions.chat_more(session, before_id) {
            self.session_job(job);
        }
        Ok(json!({}))
    }

    pub(crate) fn chat_image_call(
        &self,
        session: &SessionId,
        image_id: &str,
    ) -> Result<Value, CallError> {
        match self.sessions.chat_image(session, image_id) {
            Some(data_url) => to_value(&DataUrlReply { data_url }),
            None => Err(CallError::not_found("That image isn't available.")),
        }
    }
}

/// One line of the session state text: the `--dump-state` line plus what a
/// bug report needs (a pending completion, a quiet one, the model, the
/// title and the start of a finished reply).
fn state_text_line(view: &SessionView) -> String {
    let mut line = dump_line(view);
    if view.completion_pending_since.is_some() {
        line.push_str(" stop=pending");
    }
    let ready = view.state == SessionState::ReadyForReview;
    if ready && view.completion_quiet {
        line.push_str(" quiet");
    }
    if let Some(model) = &view.model {
        line.push_str(&format!(" model={model}"));
    }
    let title: String = view.title.chars().take(40).collect();
    line.push_str(&format!(" title=\"{title}\""));
    if ready {
        if let Some(message) = &view.last_assistant_message {
            let start: String = message.chars().take(40).collect();
            line.push_str(&format!(" review=\"{}\"", start.replace('\n', " ")));
        }
    }
    line
}
