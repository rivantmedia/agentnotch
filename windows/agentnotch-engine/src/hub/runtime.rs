//! The live hub (design §1.2): `an-core`, the one consumer of the input
//! queue, strictly in arrival order like the Mac's `HookEventPipeline`
//! (HS§5.1), waiting with `recv_timeout(next deadline)`; the worker lanes
//! (`hub::jobs`); the coalesced events; start and stop.
//!
//! - The input queue lives as long as the hub, so inputs sent while it is
//!   stopped wait for the next start (`inputsYieldedWhileStoppedWaitForTheNextStart`),
//!   and a stopped hub started again applies inputs as before
//!   (`thePipelineAppliesEventsAfterARestart`). The core keeps its state
//!   across a stop.
//! - A call waits at most 25 s for `an-core` (`CallError::busy`); long work
//!   keeps the call's reply with its job, so it never holds `an-core`.
//! - Snapshot and settings events go out at least 50 ms apart, and only
//!   when what they show changed (`aBurstIsPublishedInAFewCoalescedUpdates`).
//! - A hub that was never started (the command line's doctor and
//!   `install-hooks`) answers calls on the caller's thread, running their
//!   jobs there too: nothing listens and no thread is started.
//! - `agentnotch://` links are taken on the caller's thread (the glue's own
//!   thread: a sign-in callback trades its code with the website, up to
//!   30 s): the callback goes to the cloud; a banner's open and review
//!   links, at most ten a minute, open the panel or review the completion
//!   for sessions and rings the pages are shown, never answering anything.
//!
//! Owner: WP7.

use super::api::{
    Call, CallError, DeepLinkOutcome, DoctorExtras, EventSink, Hub, HubBackend, HubConfig, HubEvent,
};
use super::core_state::{Core, Projection};
use super::jobs::{self, JobContext, Lanes, TrackingRunner};
use super::project;
use crate::cloud::service::{is_sign_in_callback, NO_SIGN_IN_PENDING};
use crate::cloud::CloudHandle;
use crate::control::notifications::{parse_deep_link, DeepLinkAction, DeepLinkGate};
use crate::control::panel::{lands_on_list, REASON_NOTIFICATION, ROUTE_SESSIONS};
use crate::model::{HubSnapshot, PanelRequest, RingSummary, SettingsSnapshot, UpstreamUsage};
use crate::platform::Platform;
use crate::runtime_types::{Input, JobId};
use agentnotch_proto::ControlStatus;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

/// How long a call waits for `an-core` before it is answered `busy`.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(25);
/// The shortest gap between two snapshot (or settings) events.
pub const COALESCE: Duration = Duration::from_millis(50);
/// How long a stop waits for `an-core` to save and stop.
const STOP_WAIT: Duration = Duration::from_secs(10);
/// How long a stop waits for the workers' current jobs (children are
/// already ended; a hung terminal's UI job is left behind).
const WORKER_JOIN_WAIT: Duration = Duration::from_secs(3);
/// How long a stop waits for a settings write in flight, so the last write
/// is the newest.
const SETTINGS_WRITE_WAIT: Duration = Duration::from_secs(2);
/// How long a stop waits for the cloud thread to finish what it has out
/// and save; past that it ends with the process.
const CLOUD_STOP_WAIT: Duration = Duration::from_secs(3);

/// The runtime's timings; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub call_timeout: Duration,
    pub coalesce: Duration,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        RuntimeOptions {
            call_timeout: CALL_TIMEOUT,
            coalesce: COALESCE,
        }
    }
}

/// A way into `an-core`'s queue from another thread: the cloud's
/// `Input::SetSetting` and `Input::CloudState`, the transport's events.
#[derive(Clone)]
pub struct HubInputs {
    tx: Sender<Input>,
}

impl HubInputs {
    /// Queued in arrival order; applied now while the hub runs, else at its
    /// next start. False only when the hub is gone.
    pub fn send(&self, input: Input) -> bool {
        self.tx.send(input).is_ok()
    }
}

/// The live hub with its timings given, and a way into its queue.
pub fn live_hub(cfg: HubConfig, platform: Platform, options: RuntimeOptions) -> (Hub, HubInputs) {
    let runtime = Runtime::new(cfg, platform, options);
    let inputs = HubInputs {
        tx: runtime.inner.tx.clone(),
    };
    (Hub::with_backend(Arc::new(runtime)), inputs)
}

pub(crate) struct Runtime {
    inner: Arc<Inner>,
}

struct Inner {
    cfg: HubConfig,
    /// The hub's platform, its runner the tracking one.
    platform: Platform,
    runner: Arc<TrackingRunner>,
    ctx: JobContext,
    options: RuntimeOptions,
    tx: Sender<Input>,
    rx: Receiver<Input>,
    lanes: Arc<Lanes>,
    sinks: Mutex<Vec<Arc<EventSink>>>,
    publisher: Mutex<Publisher>,
    /// The core while no `an-core` holds it. Locked through a whole start
    /// or stop, so a call never sees a core half handed over.
    idle: Mutex<Option<Core>>,
    /// `an-core` holds the core (set and cleared under `idle`'s lock).
    running: AtomicBool,
    /// The threads of one run; also serialises start and stop.
    run: Mutex<Option<Running>>,
    /// The cloud service of this run (`an-core` holds it too).
    cloud: Mutex<Option<Arc<CloudHandle>>>,
    /// Banner links acted on in the last minute.
    links: Mutex<DeepLinkGate>,
}

struct Running {
    core: JoinHandle<Core>,
    workers: Vec<JoinHandle<()>>,
    /// Dropped to end the workers.
    quit: Sender<()>,
}

impl Runtime {
    pub(crate) fn new(cfg: HubConfig, platform: Platform, options: RuntimeOptions) -> Runtime {
        let runner = Arc::new(TrackingRunner::new(platform.runner.clone()));
        let platform = jobs::with_runner(&platform, runner.clone());
        let ctx = JobContext {
            roots: cfg.roots.clone(),
            platform: platform.clone(),
            base_env: std::env::vars_os().collect(),
            sealed: cfg.flags.sealed,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        Runtime {
            inner: Arc::new(Inner {
                cfg,
                platform,
                runner,
                ctx,
                options,
                tx,
                rx,
                lanes: Arc::new(Lanes::new()),
                sinks: Mutex::new(Vec::new()),
                publisher: Mutex::new(Publisher::new(options.coalesce)),
                idle: Mutex::new(None),
                running: AtomicBool::new(false),
                run: Mutex::new(None),
                cloud: Mutex::new(None),
                links: Mutex::new(DeepLinkGate::default()),
            }),
        }
    }
}

impl Inner {
    fn emit(&self, events: &[HubEvent]) {
        if events.is_empty() {
            return;
        }
        let sinks: Vec<Arc<EventSink>> = lock(&self.sinks).clone();
        for event in events {
            for sink in &sinks {
                sink(event);
            }
        }
    }

    fn load_core(&self) -> Core {
        Core::load(self.cfg.clone(), self.platform.clone())
    }

    /// `f` on the core while no `an-core` holds it (loading it first when
    /// this hub never ran); `None` while one does. Events are emitted after
    /// the lock is let go.
    fn with_idle_core<R>(&self, f: impl FnOnce(&mut Core, &mut Vec<HubEvent>) -> R) -> Option<R> {
        let mut events = Vec::new();
        let result = {
            let mut idle = lock(&self.idle);
            if idle.is_none() && self.running.load(Ordering::SeqCst) {
                return None;
            }
            let core = idle.get_or_insert_with(|| self.load_core());
            f(core, &mut events)
        };
        self.emit(&events);
        Some(result)
    }

    /// A call on a stopped hub: answered here, its jobs run here.
    fn call_inline(&self, call: Call) -> Option<Result<Value, CallError>> {
        self.with_idle_core(|core, events| {
            let (reply, answer) = crossbeam_channel::bounded(1);
            core.handle_call(call, reply);
            self.settle_inline(core);
            events.extend(core.take_events());
            let now = self.platform.clock.now();
            let mono = self.platform.clock.monotonic();
            events.extend(lock(&self.publisher).publish(core, now, mono, true));
            answer.try_recv().unwrap_or_else(|_| {
                Err(CallError::failed(
                    "That needs the app running: start Agent Notch and try again.",
                ))
            })
        })
    }

    /// Runs what a stopped hub's call scheduled, until nothing is left.
    fn settle_inline(&self, core: &mut Core) {
        // Each round's results may schedule more (a write after a change);
        // a bound keeps a result that always asks again from looping.
        for _ in 0..16 {
            core.after_input();
            let jobs = core.take_outbox();
            if jobs.is_empty() {
                return;
            }
            for (id, job) in jobs {
                // No `an-core` to recheck a typed reply: never submitted.
                let result = jobs::run_guarded(&job, &self.ctx, &mut || false);
                core.job_done(id, result);
            }
        }
    }

    fn projection(&self) -> Projection {
        if let Some(projection) = lock(&self.publisher).last.clone() {
            return projection;
        }
        let now = self.platform.clock.now();
        self.with_idle_core(|core, _| core.project(now))
            .unwrap_or_else(|| {
                // `an-core` publishes before it takes its first input; this
                // is only reached in the instant of a start.
                lock(&self.publisher)
                    .last
                    .clone()
                    .expect("a started hub has published")
            })
    }
}

impl HubBackend for Runtime {
    fn start(&self) -> Result<(), String> {
        let inner = &self.inner;
        let mut run = lock(&inner.run);
        if run.is_some() {
            return Ok(());
        }
        let mut idle = lock(&inner.idle);
        let mut core = idle.take().unwrap_or_else(|| inner.load_core());
        // The hook pipe's and the cloud's events come back through the queue.
        core.ingress_w.inputs = Some(inner.tx.clone());
        core.cloud_w.inputs = Some(inner.tx.clone());
        inner.runner.resume();
        // The launch discovery and each store's schedule, before the first
        // projection.
        core.on_start();
        *lock(&inner.cloud) = core.cloud_w.handle.clone();
        let (quit, quit_rx) = crossbeam_channel::bounded::<()>(0);
        let workers = match inner.lanes.spawn_workers(&inner.ctx, &inner.tx, &quit_rx) {
            Ok(workers) => workers,
            Err(e) => {
                core.cloud_on_stop();
                inner.stop_cloud();
                *idle = Some(core);
                return Err(format!("the engine's workers didn't start: {e}"));
            }
        };
        // What the pages see from the first instant: `snapshot()` never
        // waits for `an-core`.
        let now = inner.platform.clock.now();
        let mono = inner.platform.clock.monotonic();
        let first = lock(&inner.publisher).publish(&mut core, now, mono, true);
        let loop_inner = inner.clone();
        let spawned = std::thread::Builder::new()
            .name("an-core".into())
            .spawn(move || core_loop(core, &loop_inner, first));
        let core = match spawned {
            Ok(handle) => handle,
            Err(e) => {
                drop(quit);
                inner.stop_cloud();
                return Err(format!("the engine didn't start: {e}"));
            }
        };
        inner.running.store(true, Ordering::SeqCst);
        *run = Some(Running {
            core,
            workers,
            quit,
        });
        Ok(())
    }

    fn stop(&self) {
        let inner = &self.inner;
        let mut run = lock(&inner.run);
        let Some(running) = run.take() else {
            return;
        };
        // Calls from here on wait for the core to come back, then are
        // answered on their own thread.
        let mut idle = lock(&inner.idle);
        inner.running.store(false, Ordering::SeqCst);
        // Children first: a probe in progress ends now (its job comes back
        // as failed) instead of holding its lane through the stop.
        inner.runner.kill_all();
        let (done, stopped) = crossbeam_channel::bounded(1);
        let _ = inner.tx.send(Input::Stop { done });
        let core_stopped = stopped.recv_timeout(STOP_WAIT).is_ok();
        drop(running.quit);
        join_within(running.workers, WORKER_JOIN_WAIT);
        if core_stopped {
            if let Ok(core) = running.core.join() {
                *idle = Some(core);
            }
        } else {
            // The core stays with its thread; the next start reads the
            // saved files again.
            drop(idle);
            inner.emit(&[HubEvent::Log(
                "the engine didn't stop in time; it is left behind".into(),
            )]);
        }
        // After the core: held requests are released before anything the
        // cloud has out is waited for.
        inner.stop_cloud();
    }

    fn on_event(&self, sink: EventSink) {
        lock(&self.inner.sinks).push(Arc::new(sink));
    }

    fn call(&self, call: Call) -> Result<Value, CallError> {
        let inner = &self.inner;
        if !inner.running.load(Ordering::SeqCst) {
            if let Some(answer) = inner.call_inline(call.clone()) {
                return answer;
            }
        }
        let (reply, answer) = crossbeam_channel::bounded(1);
        if inner.tx.send(Input::Call { call, reply }).is_err() {
            return Err(CallError::failed("The engine has stopped."));
        }
        match answer.recv_timeout(inner.options.call_timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(CallError::busy(
                "The engine is busy. Try again in a moment.",
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(CallError::failed("The engine dropped the call."))
            }
        }
    }

    fn snapshot(&self) -> HubSnapshot {
        self.inner.projection().snapshot
    }

    fn settings_snapshot(&self) -> SettingsSnapshot {
        self.inner.projection().settings
    }

    fn launch_rings(&self) -> Vec<RingSummary> {
        // Before a start: a synchronous discovery that reads only (the
        // rings exist from the first frame); a running hub has its rings.
        let now = self.inner.platform.clock.now();
        self.inner
            .with_idle_core(|core, _| {
                core.ensure_discovered(now);
                let readings = core.readings(now);
                project::launch_rings(&core.registry.accounts(), &readings, now)
            })
            .unwrap_or_else(|| self.snapshot().rings)
    }

    fn upstream_usage(&self) -> UpstreamUsage {
        project::upstream_usage(&self.snapshot().rings)
    }

    fn handle_deep_link(&self, url: &str) -> DeepLinkOutcome {
        let inner = &self.inner;
        if inner.cfg.flags.sealed {
            return DeepLinkOutcome::Ignored("Sealed: deep links are ignored.".into());
        }
        if let Some(action) = parse_deep_link(url) {
            return self.banner_link(action);
        }
        let cloud = lock(&inner.cloud).clone();
        match cloud {
            Some(cloud) => cloud.deep_link(url),
            // No cloud running: nothing can be waiting for a callback.
            None if is_sign_in_callback(url) => {
                DeepLinkOutcome::SignInIgnored(NO_SIGN_IN_PENDING.into())
            }
            None => DeepLinkOutcome::Ignored("not a link this app follows".into()),
        }
    }

    fn control_status(&self) -> ControlStatus {
        self.inner.projection().status
    }

    fn doctor_report(&self, _extra: &DoctorExtras) -> String {
        // wp7-11: the §4.14 report.
        format!(
            "Agent Notch doctor v{} ({})\nThe full report isn't in this build yet.\n",
            self.inner.cfg.app_version,
            crate::core::roots::IDENTIFIER
        )
    }
}

impl Runtime {
    /// A banner's link: the panel at a session or ring the pages are shown,
    /// or that session's completion reviewed. Never an answer to anything.
    fn banner_link(&self, action: DeepLinkAction) -> DeepLinkOutcome {
        let inner = &self.inner;
        let now = inner.platform.clock.now();
        if !lock(&inner.links).allow(now) {
            return DeepLinkOutcome::Ignored("too many links in a minute".into());
        }
        let shown = self.snapshot();
        let knows_session = |id: &str| shown.sessions.iter().any(|row| row.session_id == id);
        match action {
            DeepLinkAction::OpenSession(session) => {
                if !knows_session(session.as_str()) {
                    return DeepLinkOutcome::Ignored("no such session".into());
                }
                let request = lands_on_list(&session, REASON_NOTIFICATION);
                inner.emit(&[HubEvent::Panel(request)]);
                DeepLinkOutcome::Opened
            }
            DeepLinkAction::OpenRing(ring) => {
                if !shown.rings.iter().any(|r| r.ring_id == ring.as_str()) {
                    return DeepLinkOutcome::Ignored("no such ring".into());
                }
                inner.emit(&[HubEvent::Panel(PanelRequest {
                    route: ROUTE_SESSIONS.to_owned(),
                    ring_id: Some(ring.as_str().to_owned()),
                    highlight: None,
                    reason: REASON_NOTIFICATION.to_owned(),
                })]);
                DeepLinkOutcome::Opened
            }
            DeepLinkAction::MarkReviewed {
                session,
                completed_at,
            } => {
                if !knows_session(session.as_str()) {
                    return DeepLinkOutcome::Ignored("no such session".into());
                }
                // The completion the banner announced, never a later one.
                let call = match completed_at {
                    Some(at) => Call::MarkViewed {
                        session_id: session,
                        completed_at_ms: crate::core::time::to_ms(at),
                    },
                    None => Call::MarkReviewed {
                        session_id: session,
                        at_ms: crate::core::time::to_ms(now),
                    },
                };
                match self.call(call) {
                    Ok(_) => DeepLinkOutcome::Reviewed,
                    Err(e) => DeepLinkOutcome::Ignored(e.message),
                }
            }
        }
    }
}

impl Inner {
    /// Stops this run's cloud service, waiting a little for what it has out
    /// (its stores are saved as its thread ends).
    fn stop_cloud(&self) {
        let Some(cloud) = lock(&self.cloud).take() else {
            return;
        };
        let stopper = std::thread::Builder::new()
            .name("an-cloud-stop".into())
            .spawn(move || cloud.stop());
        if let Ok(stopper) = stopper {
            join_within(vec![stopper], CLOUD_STOP_WAIT);
        }
    }
}

// ---- an-core ----

/// `an-core`: one input at a time, in order, until a stop; returns the core
/// so a later start goes on from it.
fn core_loop(mut core: Core, inner: &Inner, first: Vec<HubEvent>) -> Core {
    let clock = inner.platform.clock.clone();
    let mut events = core.take_events();
    events.extend(first);
    inner.emit(&events);
    // Jobs a stop left unsent.
    dispatch(&mut core, &inner.lanes);
    loop {
        let input = match core.backlog.pop_front() {
            Some(input) => Some(input),
            None => match wait_time(&core, inner, clock.now(), clock.monotonic()) {
                None => match inner.rx.recv() {
                    Ok(input) => Some(input),
                    Err(_) => return core,
                },
                Some(wait) => match inner.rx.recv_timeout(wait) {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return core,
                },
            },
        };
        let now = clock.now();
        match input {
            Some(Input::Stop { done }) => {
                stop_core(&mut core, inner);
                let mut events = core.take_events();
                events.extend(lock(&inner.publisher).publish(
                    &mut core,
                    clock.now(),
                    clock.monotonic(),
                    true,
                ));
                inner.emit(&events);
                let _ = done.send(());
                return core;
            }
            Some(input) => {
                core.handle(input);
                lock(&inner.publisher).dirty = true;
            }
            None => {
                // A deadline passed: the stores' own, or a label boundary.
                if core.next_deadline().is_some_and(|at| at <= now) {
                    core.handle(Input::Tick);
                }
                let mut publisher = lock(&inner.publisher);
                if publisher.boundary.is_some_and(|at| at <= now) {
                    publisher.dirty = true;
                }
            }
        }
        core.after_input();
        dispatch(&mut core, &inner.lanes);
        let mut events = core.take_events();
        events.extend(lock(&inner.publisher).publish(
            &mut core,
            clock.now(),
            clock.monotonic(),
            false,
        ));
        inner.emit(&events);
    }
}

/// Hands the core's new jobs to their lanes.
fn dispatch(core: &mut Core, lanes: &Lanes) {
    for (id, job) in core.take_outbox() {
        lanes.submit(id, job);
    }
}

/// How long `an-core` may wait for an input: until the next deadline
/// (`None`: none is set).
fn wait_time(core: &Core, inner: &Inner, now: SystemTime, mono: Instant) -> Option<Duration> {
    let publisher = lock(&inner.publisher);
    let wall = [core.next_deadline(), publisher.boundary]
        .into_iter()
        .flatten()
        .min()
        .map(|at| at.duration_since(now).unwrap_or(Duration::ZERO));
    let coalesced = publisher
        .due_at(mono)
        .map(|at| at.saturating_duration_since(mono));
    [wall, coalesced].into_iter().flatten().min()
}

/// A stop, on `an-core`: queued jobs are answered as not run, the settings
/// write in flight lands first, then the core saves everything now.
fn stop_core(core: &mut Core, inner: &Inner) {
    for (id, job) in inner.lanes.drain_queued() {
        let result = jobs::failure_result(&job, jobs::STOPPED_BEFORE_RUNNING, &inner.platform);
        core.job_done(id, result);
    }
    if let Some(write) = core.settings_write_in_flight() {
        await_job(core, inner, write, SETTINGS_WRITE_WAIT);
    }
    core.stop();
}

/// Waits for one job's result; whatever else arrives meanwhile keeps its
/// order for the next start.
fn await_job(core: &mut Core, inner: &Inner, id: JobId, within: Duration) {
    let deadline = Instant::now() + within;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match inner.rx.recv_timeout(left) {
            Ok(Input::JobDone { id: done, result }) if done == id => {
                core.job_done(done, result);
                return;
            }
            Ok(other) => core.backlog.push_back(other),
            Err(_) => return,
        }
    }
}

fn join_within(handles: Vec<JoinHandle<()>>, within: Duration) {
    let deadline = Instant::now() + within;
    let mut left = handles;
    while !left.is_empty() && Instant::now() < deadline {
        let (done, running): (Vec<_>, Vec<_>) = left.into_iter().partition(|h| h.is_finished());
        for handle in done {
            let _ = handle.join();
        }
        left = running;
        if !left.is_empty() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // What is still running (a hung terminal's UI job) is left behind; it
    // ends with the process.
}

// ---- events ----

/// What was last shown, and when the next projection may go out.
struct Publisher {
    coalesce: Duration,
    /// Something the projections read may have changed.
    dirty: bool,
    /// When the last snapshot or settings event went out.
    sent_at: Option<Instant>,
    /// When a label of the last snapshot changes by time alone.
    boundary: Option<SystemTime>,
    last: Option<Projection>,
    last_upstream: Option<UpstreamUsage>,
    last_cloud: Option<crate::model::CloudState>,
    last_dump: Option<Vec<String>>,
    dumps: u64,
}

impl Publisher {
    fn new(coalesce: Duration) -> Publisher {
        Publisher {
            coalesce,
            dirty: true,
            sent_at: None,
            boundary: None,
            last: None,
            last_upstream: None,
            last_cloud: None,
            last_dump: None,
            dumps: 0,
        }
    }

    /// When a pending change may go out (`None`: nothing is pending).
    fn due_at(&self, mono: Instant) -> Option<Instant> {
        if !self.dirty {
            return None;
        }
        Some(self.sent_at.map_or(mono, |at| at + self.coalesce))
    }

    /// Projects and returns the events for what changed; nothing before
    /// the coalescing gap unless `now_anyway`.
    fn publish(
        &mut self,
        core: &mut Core,
        now: SystemTime,
        mono: Instant,
        now_anyway: bool,
    ) -> Vec<HubEvent> {
        if !now_anyway {
            if !self.dirty {
                return Vec::new();
            }
            if self.sent_at.is_some_and(|at| mono < at + self.coalesce) {
                return Vec::new();
            }
        }
        self.dirty = false;
        let mut next = core.project(now);
        let mut events = Vec::new();
        let mut sent = false;
        match &self.last {
            Some(last) if same_snapshot(&last.snapshot, &next.snapshot) => {
                next.snapshot.generated_at_ms = last.snapshot.generated_at_ms;
            }
            last => {
                if last
                    .as_ref()
                    .is_none_or(|l| l.snapshot.tray_badge != next.snapshot.tray_badge)
                {
                    events.push(HubEvent::TrayBadge(next.snapshot.tray_badge));
                }
                events.push(HubEvent::Snapshot(next.snapshot.clone()));
                sent = true;
            }
        }
        if self
            .last
            .as_ref()
            .is_none_or(|l| l.settings != next.settings)
        {
            events.push(HubEvent::Settings(next.settings.clone()));
            sent = true;
        }
        if self.last_cloud.as_ref() != Some(&core.cloud) {
            self.last_cloud = Some(core.cloud.clone());
            events.push(HubEvent::Cloud(core.cloud.clone()));
        }
        let upstream = project::upstream_usage(&next.snapshot.rings);
        if self.last_upstream.as_ref() != Some(&upstream) {
            self.last_upstream = Some(upstream.clone());
            events.push(HubEvent::UpstreamUsage(upstream));
        }
        if core.cfg.flags.dump_state {
            let lines = core.dump_lines();
            if self.last_dump.as_ref() != Some(&lines) {
                self.dumps += 1;
                events.push(HubEvent::Log(format!(
                    "[agentnotch-state] publish #{}: {} session(s)",
                    self.dumps,
                    lines.len()
                )));
                events.extend(lines.iter().cloned().map(HubEvent::Log));
                self.last_dump = Some(lines);
            }
        }
        if sent {
            self.sent_at = Some(mono);
        }
        self.boundary = project::next_boundary(&next.snapshot, now);
        self.last = Some(next);
        events
    }
}

/// Two snapshots show the same (their dates aside).
fn same_snapshot(a: &HubSnapshot, b: &HubSnapshot) -> bool {
    let mut b = b.clone();
    b.generated_at_ms = a.generated_at_ms;
    *a == b
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
