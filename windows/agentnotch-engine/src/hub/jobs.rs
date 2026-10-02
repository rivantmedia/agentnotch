//! The worker lanes (design §1.2) and the job executor: every [`Job`] runs
//! here, off `an-core`, through the body its package wrote, and comes back
//! as exactly one `Input::JobDone`, also when the body panicked (a lost
//! probe result would stop all probes until a restart; a lost transcript
//! sync would leave the session never read again).
//!
//! - `an-io-0..2` share the file queue: transcript and registry reads,
//!   settings.json writes, `.claude.json` and Desktop cache reads, the
//!   engine's own files.
//! - `an-probe` runs one child at a time (the usage probe, `claude
//!   --version`), so a slow `claude` never starves file jobs.
//! - `an-ui` runs what can hang on another process (console helper runs,
//!   focus, UI Automation, visibility), so a hung terminal only delays other
//!   UI jobs.
//!
//! The queues outlive a stop: a job queued while the hub is stopped runs at
//! the next start. The children every job starts go through a
//! [`TrackingRunner`], so `stop()` can end them at once.
//!
//! Owner: WP7.

use crate::control::focus;
use crate::model::DesktopReading;
use crate::platform::{
    CommandRunner, CommandSpec, ConsoleInfo, Exit, FocusOutcome, HostApp, HostKind, Platform,
    RunningCommand, TypeOutcome, WriteMode,
};
use crate::platform::{Expect, Roots};
use crate::runtime_types::*;
use crossbeam_channel::{Receiver, Sender};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// File-lane workers (`an-io-0..2`).
pub const IO_WORKERS: usize = 3;

/// Why a job queued at a stop was answered without running.
pub const STOPPED_BEFORE_RUNNING: &str = "The app stopped before this ran.";

/// What a job body may use.
#[derive(Clone)]
pub struct JobContext {
    pub roots: Roots,
    /// The hub's platform, its runner the [`TrackingRunner`].
    pub platform: Platform,
    /// The app's own environment, read once (the probe's and the version
    /// checks' is this, scrubbed).
    pub base_env: Vec<(OsString, OsString)>,
    pub sealed: bool,
}

/// Runs `job` with its package's body. `checkpoint` is asked between typing
/// a reply and pressing Return (`Job::Type`): `an-core`'s fresh check.
pub fn run(job: &Job, ctx: &JobContext, checkpoint: &mut dyn FnMut() -> bool) -> JobResult {
    let p = &ctx.platform;
    let now = p.clock.now();
    match job {
        Job::ReadFolders { explicit } => JobResult::Folders(crate::accounts::read_folder_snapshot(
            &ctx.roots,
            explicit,
            p.files.as_ref(),
            p.processes.as_ref(),
        )),
        Job::ReadClaudeJson { folder, path } => {
            JobResult::ClaudeJson(crate::usage::claude_json::read_claude_json(folder, path))
        }
        Job::ReadRegistry {
            sessions_dir,
            via_link,
        } => JobResult::Registry(crate::sessions::registry::read_registry(
            sessions_dir,
            *via_link,
            p.processes.as_ref(),
            now,
        )),
        Job::SyncTranscript {
            session,
            path,
            cursor,
        } => JobResult::Transcript(crate::sessions::transcript::sync_transcript(
            session,
            path,
            *cursor,
            crate::sessions::transcript::is_agent_transcript(path),
            p.files.as_ref(),
        )),
        Job::LoadChat {
            session,
            path,
            before,
        } => JobResult::Chat(crate::sessions::chat::load_chat(
            session,
            path,
            before.as_deref(),
            crate::sessions::chat::PAGE_SIZE,
        )),
        Job::DesktopHosted {
            roots,
            host_session_id,
            candidates,
        } => JobResult::Hosted(crate::sessions::desktop::find_hosted(
            roots,
            host_session_id,
            candidates,
            p.files.as_ref(),
            ctx.sealed,
        )),
        Job::ReadDesktopCache { organization_uuid } => JobResult::Desktop(
            crate::usage::desktop::read_desktop_cache(&ctx.roots, organization_uuid, now),
        ),
        Job::Install { plans } => JobResult::Installed(crate::hooks::apply::apply_installs(
            plans,
            p.files.as_ref(),
            p.clock.as_ref(),
        )),
        Job::Uninstall { record, folders } => JobResult::Installed(
            crate::hooks::manager::uninstall_all(
                record,
                folders,
                p.files.as_ref(),
                p.clock.as_ref(),
            )
            .0,
        ),
        Job::Persist { file, bytes } => JobResult::Persisted(persist(ctx, *file, bytes)),
        Job::Probe(plan) => JobResult::Probe(crate::usage::probe::run_probe(
            plan,
            p.runner.as_ref(),
            p.clock.as_ref(),
        )),
        Job::Versions { binaries, bundled } => {
            JobResult::Versions(crate::usage::versions::read_versions(
                binaries,
                bundled,
                p.runner.as_ref(),
                &ctx.base_env,
            ))
        }
        Job::ConsoleInfo { pid, started } => {
            // Pids come back fast on Windows: a process that isn't the one
            // the session ran is never asked about.
            if p.processes.start_time(*pid) != Some(*started) {
                JobResult::Console(ConsoleInfo {
                    error: Some("Claude Code's process has ended.".into()),
                    ..ConsoleInfo::default()
                })
            } else {
                JobResult::Console(p.terminals.console_info(*pid))
            }
        }
        Job::Classify { pid } => {
            let table = p.processes.table();
            JobResult::Host(p.terminals.classify_host(*pid, &table))
        }
        Job::Focus { steps } => JobResult::Focus(focus::run_plan(steps, &mut |step| {
            p.terminals.run_focus(step)
        })),
        Job::Type { target, text, .. } => {
            JobResult::Typed(p.console.type_text(target, text, checkpoint))
        }
        Job::Visibility => JobResult::Visible {
            any_terminal: p.terminals.any_terminal_visible(),
            full_screen: p.terminals.foreground().is_some_and(|f| f.fullscreen),
        },
    }
}

/// [`run`], with a panic turned into the job's own failure result: every
/// job is answered exactly once.
pub fn run_guarded(job: &Job, ctx: &JobContext, checkpoint: &mut dyn FnMut() -> bool) -> JobResult {
    match catch_unwind(AssertUnwindSafe(|| run(job, ctx, checkpoint))) {
        Ok(result) => result,
        Err(panic) => {
            let why = panic_text(panic.as_ref());
            failure_result(
                job,
                &format!("It failed unexpectedly ({why})."),
                &ctx.platform,
            )
        }
    }
}

/// The result a job gives when it could not run: what its consumer reads as
/// "nothing learnt", never as "everything is gone".
pub fn failure_result(job: &Job, why: &str, platform: &Platform) -> JobResult {
    let now = platform.clock.now();
    let why = why.to_owned();
    match job {
        // A real read always names the home folder: an empty one is a read
        // that failed, never "every folder disappeared" (see
        // `is_failed_folder_read`).
        Job::ReadFolders { .. } => JobResult::Folders(crate::model::FolderSnapshot::default()),
        Job::ReadClaudeJson { folder, .. } => JobResult::ClaudeJson(ClaudeJsonRead {
            folder: folder.clone(),
            identity: None,
            cached_usage: None,
            stamp: None,
            error: Some(why),
        }),
        Job::ReadRegistry {
            sessions_dir,
            via_link,
        } => JobResult::Registry(crate::model::RegistrySnapshot {
            sessions_dir: sessions_dir.clone(),
            via_link: *via_link,
            entries: Vec::new(),
            read_at: now,
            error: Some(why),
        }),
        // The cursor stays where it was: the next sync reads the same lines.
        Job::SyncTranscript {
            session,
            path,
            cursor,
        } => JobResult::Transcript(TranscriptDelta {
            session: session.clone(),
            path: path.clone(),
            cursor: *cursor,
            reset: false,
            entries: Vec::new(),
        }),
        Job::LoadChat {
            session,
            path,
            before,
        } => JobResult::Chat(crate::model::ChatPage {
            session: session.clone(),
            path: path.clone(),
            items: Vec::new(),
            has_earlier: 0,
            before: before.clone(),
            images: Default::default(),
            error: Some(why),
            times: Vec::new(),
        }),
        Job::DesktopHosted { .. } => JobResult::Hosted(None),
        Job::ReadDesktopCache { .. } => JobResult::Desktop(DesktopReading::Unavailable(why)),
        Job::Install { plans } => JobResult::Installed(
            plans
                .iter()
                .map(|plan| failed_outcome(plan, &why))
                .collect(),
        ),
        Job::Uninstall { record, folders } => JobResult::Installed(
            crate::hooks::manager::removal_plans(record, folders)
                .iter()
                .map(|plan| failed_outcome(plan, &why))
                .collect(),
        ),
        Job::Persist { .. } => JobResult::Persisted(Err(why)),
        Job::Probe(plan) => JobResult::Probe(ProbeResult {
            plan: plan.clone(),
            outcome: ProbeOutcome::Failed(why),
            started: now,
            finished: now,
            folder_identity_after: None,
        }),
        Job::Versions { binaries, .. } => JobResult::Versions(
            binaries
                .iter()
                .map(|path| VersionSighting {
                    source: VersionSource::Binary,
                    path: Some(path.clone()),
                    version: None,
                })
                .collect(),
        ),
        Job::ConsoleInfo { .. } => JobResult::Console(ConsoleInfo {
            error: Some(why),
            ..ConsoleInfo::default()
        }),
        Job::Classify { .. } => JobResult::Host(HostApp {
            kind: HostKind::Unknown,
            window: None,
            host_pid: None,
            exe_path: None,
        }),
        Job::Focus { .. } => JobResult::Focus(FocusOutcome::Failed(why)),
        Job::Type { .. } => JobResult::Typed(TypeOutcome::Failed(why)),
        // Unknown counts as "a terminal is in view": the panel then peeks
        // rather than opening over what the user may be reading.
        Job::Visibility => JobResult::Visible {
            any_terminal: true,
            full_screen: false,
        },
    }
}

/// A `Folders` result that stands for a failed read (an empty home folder:
/// a real read always names it). The registry must not take it as a
/// discovery.
pub fn is_failed_folder_read(snapshot: &crate::model::FolderSnapshot) -> bool {
    snapshot.home.is_empty()
}

fn failed_outcome(plan: &InstallPlan, why: &str) -> InstallOutcome {
    InstallOutcome {
        folder: plan.folder.clone(),
        settings_path: plan.settings_path.clone(),
        result: Err(why.to_owned()),
        backup: None,
        entry: None,
        status_line: None,
    }
}

/// One of the engine's files in `<support>`, private, all or nothing.
fn persist(ctx: &JobContext, file: PersistFile, bytes: &[u8]) -> Result<(), String> {
    if ctx.sealed {
        // A sealed run never writes `<support>`.
        return Err("Sealed: nothing is saved.".into());
    }
    let files = ctx.platform.files.as_ref();
    let support = &ctx.roots.support;
    files
        .ensure_private_dir(support)
        .map_err(|e| format!("{}: {e}", support.display()))?;
    let path = support.join(file.file_name());
    files
        .write_atomic(&path, bytes, WriteMode::Private, Expect::Nothing)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", file.file_name()))
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic".to_owned()
    }
}

// ---- children ----

/// The platform's runner, remembering every child it started so a stop can
/// end them all at once (`kill_tree`: the whole Job object), and refusing
/// new ones while the hub stops.
pub struct TrackingRunner {
    inner: Arc<dyn CommandRunner>,
    children: Mutex<Vec<Weak<Child>>>,
    stopping: AtomicBool,
}

/// How long a wait holds a child's lock: a stop's kill lands within this.
const WAIT_SLICE: Duration = Duration::from_millis(50);

/// A started child and whether a stop asked for its end.
struct Child {
    command: Mutex<Box<dyn RunningCommand>>,
    kill: AtomicBool,
}

impl Child {
    /// Ends the tree when a stop asked for it and nobody did yet; called
    /// with the lock held.
    fn kill_if_asked(&self, command: &mut Box<dyn RunningCommand>) {
        if self.kill.swap(false, Ordering::SeqCst) {
            command.kill_tree();
        }
    }
}

impl TrackingRunner {
    pub fn new(inner: Arc<dyn CommandRunner>) -> TrackingRunner {
        TrackingRunner {
            inner,
            children: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
        }
    }

    /// Ends every child still running and refuses new ones until
    /// [`TrackingRunner::resume`]. Never waits on a child: one that is being
    /// waited on ends at its waiter's next slice.
    pub fn kill_all(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let children: Vec<_> = lock(&self.children).drain(..).collect();
        for child in children.iter().filter_map(Weak::upgrade) {
            child.kill.store(true, Ordering::SeqCst);
            let command = match child.command.try_lock() {
                Ok(command) => Some(command),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => None,
            };
            if let Some(mut command) = command {
                child.kill_if_asked(&mut command);
            }
        }
    }

    /// Children may be started again (the hub started).
    pub fn resume(&self) {
        self.stopping.store(false, Ordering::SeqCst);
    }

    /// Children started and not yet finished with.
    pub fn running(&self) -> usize {
        let mut children = lock(&self.children);
        children.retain(|child| child.strong_count() > 0);
        children.len()
    }
}

impl CommandRunner for TrackingRunner {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(io::Error::other("The app is stopping."));
        }
        let child = Arc::new(Child {
            command: Mutex::new(self.inner.spawn(spec)?),
            kill: AtomicBool::new(false),
        });
        let mut children = lock(&self.children);
        children.retain(|child| child.strong_count() > 0);
        children.push(Arc::downgrade(&child));
        drop(children);
        // A stop that came in while the child started still ends it.
        if self.stopping.load(Ordering::SeqCst) {
            child.kill.store(true, Ordering::SeqCst);
            child.kill_if_asked(&mut lock(&child.command));
        }
        Ok(Box::new(Tracked { child }))
    }
}

/// A child the [`TrackingRunner`] can end from another thread: waits hold
/// its lock only in short slices and end it between them when asked.
struct Tracked {
    child: Arc<Child>,
}

impl RunningCommand for Tracked {
    fn pid(&self) -> u32 {
        lock(&self.child.command).pid()
    }

    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        lock(&self.child.command).take_stdin()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        lock(&self.child.command).take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        lock(&self.child.command).take_stderr()
    }

    fn wait_timeout(&mut self, d: Duration) -> io::Result<Option<Exit>> {
        let deadline = Instant::now() + d;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let exit = {
                let mut command = lock(&self.child.command);
                self.child.kill_if_asked(&mut command);
                command.wait_timeout(left.min(WAIT_SLICE))?
            };
            if exit.is_some() || left <= WAIT_SLICE {
                return Ok(exit);
            }
        }
    }

    fn kill_tree(&mut self) {
        lock(&self.child.command).kill_tree();
    }
}

// ---- lanes ----

/// A job on its way to a worker.
pub(crate) type Queued = (JobId, Job);

/// The three job queues. They live as long as the hub: a stop ends the
/// workers, not the queues.
pub(crate) struct Lanes {
    io: (Sender<Queued>, Receiver<Queued>),
    probe: (Sender<Queued>, Receiver<Queued>),
    ui: (Sender<Queued>, Receiver<Queued>),
}

impl Lanes {
    pub(crate) fn new() -> Lanes {
        Lanes {
            io: crossbeam_channel::unbounded(),
            probe: crossbeam_channel::unbounded(),
            ui: crossbeam_channel::unbounded(),
        }
    }

    fn queue(&self, lane: Lane) -> &(Sender<Queued>, Receiver<Queued>) {
        match lane {
            Lane::Io => &self.io,
            Lane::Probe => &self.probe,
            Lane::Ui => &self.ui,
        }
    }

    /// Queues a job on its lane.
    pub(crate) fn submit(&self, id: JobId, job: Job) {
        // The receiver lives in `self`: the send can't fail.
        let _ = self.queue(job.lane()).0.send((id, job));
    }

    /// The jobs no worker has taken yet, in queue order (at a stop).
    pub(crate) fn drain_queued(&self) -> Vec<Queued> {
        let mut jobs = Vec::new();
        for lane in [Lane::Io, Lane::Probe, Lane::Ui] {
            jobs.extend(self.queue(lane).1.try_iter());
        }
        jobs
    }

    /// Starts the workers of one run. They end when `quit`'s sender is
    /// dropped (after the job each is running).
    pub(crate) fn spawn_workers(
        &self,
        ctx: &JobContext,
        results: &Sender<Input>,
        quit: &Receiver<()>,
    ) -> io::Result<Vec<JoinHandle<()>>> {
        let mut names: Vec<(String, Lane)> = (0..IO_WORKERS)
            .map(|n| (format!("an-io-{n}"), Lane::Io))
            .collect();
        names.push(("an-probe".into(), Lane::Probe));
        names.push(("an-ui".into(), Lane::Ui));
        let mut handles = Vec::new();
        for (name, lane) in names {
            let jobs = self.queue(lane).1.clone();
            let (ctx, results, quit) = (ctx.clone(), results.clone(), quit.clone());
            let handle = std::thread::Builder::new()
                .name(name)
                .spawn(move || worker(jobs, ctx, results, quit))?;
            handles.push(handle);
        }
        Ok(handles)
    }
}

fn worker(jobs: Receiver<Queued>, ctx: JobContext, results: Sender<Input>, quit: Receiver<()>) {
    loop {
        crossbeam_channel::select! {
            recv(quit) -> _ => return,
            recv(jobs) -> queued => {
                let Ok((id, job)) = queued else { return };
                let result = {
                    let mut checkpoint = || type_checkpoint(&results, id);
                    run_guarded(&job, &ctx, &mut checkpoint)
                };
                // Disconnected only when the hub is gone.
                let _ = results.send(Input::JobDone { id, result });
            }
        }
    }
}

/// How long the typing helper waits for `an-core`'s fresh check before it
/// leaves the reply typed but not sent (`ConsoleInput::type_text`: 2 s).
pub const CHECKPOINT_WAIT: Duration = Duration::from_secs(2);

/// Asks `an-core` whether Return may still be pressed (§4.8). No answer in
/// time is a no.
fn type_checkpoint(results: &Sender<Input>, job: JobId) -> bool {
    let (reply, answer) = crossbeam_channel::bounded(1);
    if results.send(Input::TypeCheckpoint { job, reply }).is_err() {
        return false;
    }
    answer.recv_timeout(CHECKPOINT_WAIT).unwrap_or(false)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The platform with its runner replaced by `runner`.
pub fn with_runner(platform: &Platform, runner: Arc<TrackingRunner>) -> Platform {
    Platform {
        runner,
        ..platform.clone()
    }
}
