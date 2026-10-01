//! Cloud sync (CL): the contract, sign-in with PKCE, the sync service, the
//! ledger, the token scanner, the backfill, folder logins, the usage outbox
//! and session summaries, on its own thread (`an-cloud`).
//!
//! Owner: WP8. [`CloudService::start`] runs [`service::CloudSync`] on the
//! `an-cloud` thread; [`CloudHandle`] is how the rest of the app reaches it
//! (the §3.4 signatures, plus additive `update_config` and `flush`).

pub mod api;
pub mod auth;
pub mod backfill;
pub mod browser;
pub mod contract;
pub mod environment;
pub mod feed;
pub mod files;
pub mod folder_logins;
pub mod keys;
pub mod ledger;
pub mod pass;
pub mod pricing;
pub mod recorder;
pub mod scanner;
pub mod service;
pub mod summary;
pub mod website;

use crate::hub::DeepLinkOutcome;
use crate::model::CloudState;
use crate::platform::{Clock, Platform};
use crate::runtime_types::{CloudCall, CloudConfig, CloudDeps, LiveBatch, UsageObservation};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use service::{AcceptedSignIn, CloudSync, Generations, STOPPED, TICK_INTERVAL};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

/// What the `an-cloud` thread is asked to do, in order.
enum Message {
    Call(CloudCall),
    ObserveLive(Box<LiveBatch>),
    RecordUsage(UsageObservation),
    /// A callback whose code was traded on the caller's thread: adopt it.
    FinishSignIn(Box<AcceptedSignIn>),
    Config(Box<CloudConfig>),
    /// Answered once everything queued before it is done.
    Flush(Sender<()>),
    Stop,
}

pub struct CloudService;

impl CloudService {
    /// Starts the service (a sealed one only shows its fixture) and its
    /// `an-cloud` thread, which ticks every [`TICK_INTERVAL`] (the first
    /// tick at once) and runs the calls queued to it one at a time.
    pub fn start(cfg: CloudConfig, deps: Arc<dyn CloudDeps>, platform: &Platform) -> CloudHandle {
        let clock = platform.clock.clone();
        let service = Arc::new(CloudSync::new(cfg, deps, platform));
        service.start(clock.now());
        let (sender, receiver) = crossbeam_channel::unbounded();
        let worker = {
            let service = service.clone();
            let clock = clock.clone();
            std::thread::Builder::new()
                .name("an-cloud".into())
                .spawn(move || run(&service, &receiver, &*clock))
        };
        // A thread that can't start leaves the service as it started: its
        // state is still shown, and calls say it stopped.
        let worker = worker.ok();
        CloudHandle {
            generations: service.generations(),
            service,
            clock,
            sender,
            worker: Mutex::new(worker),
        }
    }
}

fn run(service: &Arc<CloudSync>, receiver: &Receiver<Message>, clock: &dyn Clock) {
    let mut next_tick = Instant::now();
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        let message = match receiver.recv_timeout(wait) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                service.tick(clock.now());
                next_tick = Instant::now() + TICK_INTERVAL;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => Message::Stop,
        };
        match message {
            Message::Call(call) => {
                // The state says how it went (an error is its last error).
                let _ = service.call(call, clock.now());
            }
            Message::ObserveLive(batch) => service.observe_live(*batch, clock.now()),
            Message::RecordUsage(observation) => service.record_usage(observation),
            Message::FinishSignIn(accepted) => {
                service.finish_sign_in(*accepted, clock.now());
            }
            Message::Config(cfg) => service.update_config(*cfg, clock.now()),
            Message::Flush(done) => {
                let _ = done.send(());
            }
            Message::Stop => {
                service.stop(clock.now());
                return;
            }
        }
    }
}

/// The rest of the app's way to the cloud. Every method returns without
/// waiting on the `an-cloud` thread, except [`flush`](Self::flush) and
/// [`stop`](Self::stop).
pub struct CloudHandle {
    service: Arc<CloudSync>,
    generations: Arc<Generations>,
    clock: Arc<dyn Clock>,
    sender: Sender<Message>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CloudHandle {
    /// The hub's running sessions (`cloud::feed::live_batch`).
    pub fn observe_live(&self, o: LiveBatch) {
        let _ = self.sender.send(Message::ObserveLive(Box::new(o)));
    }

    /// A usage reading the engine took in.
    pub fn record_usage(&self, o: UsageObservation) {
        let _ = self.sender.send(Message::RecordUsage(o));
    }

    /// Queues a settings-section action; the state shows how it went. What
    /// the call stops (a pass, a summary, a sign-in still out) is stopped
    /// before it is queued, so nothing more is sent while it waits its turn.
    pub fn call(&self, c: CloudCall) -> Result<(), String> {
        self.generations.interrupt(c);
        self.sender
            .send(Message::Call(c))
            .map_err(|_| STOPPED.to_owned())
    }

    /// An `agentnotch://` link. The sign-in's callback is checked and its
    /// code traded here, on the caller's thread (bounded by the token
    /// request's timeout, never by a pass the cloud thread runs); the
    /// session is adopted on the cloud thread.
    pub fn deep_link(&self, url: &str) -> DeepLinkOutcome {
        match self.service.accept_callback(url, self.clock.now()) {
            Ok(accepted) => match self.sender.send(Message::FinishSignIn(Box::new(accepted))) {
                Ok(()) => DeepLinkOutcome::SignInCompleted,
                // Stopped: finishing it ends the session on Supabase.
                Err(unsent) => match unsent.into_inner() {
                    Message::FinishSignIn(accepted) => {
                        self.service.finish_sign_in(*accepted, self.clock.now())
                    }
                    _ => DeepLinkOutcome::SignInIgnored(STOPPED.into()),
                },
            },
            Err(outcome) => outcome,
        }
    }

    /// The published state, read without waiting on the cloud thread.
    pub fn state(&self) -> CloudState {
        self.service.state()
    }

    /// `an-core` republished the cloud's config (after a switch was
    /// written, or the device changed). Additive (WP8), for WP7.
    pub fn update_config(&self, cfg: CloudConfig) {
        let _ = self.sender.send(Message::Config(Box::new(cfg)));
    }

    /// Waits until everything queued before it has been handled (or the
    /// thread is gone). Additive (WP8): tests and an orderly shutdown.
    pub fn flush(&self) {
        let (done, wait) = crossbeam_channel::bounded(1);
        if self.sender.send(Message::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }

    /// Stops what runs at once (a pass sends nothing after the request it
    /// has out, a summary's `claude` is killed: [`CloudSync::halt`]), then
    /// the thread after what is queued, saves the stores and waits for it to
    /// end. Calling it again does nothing.
    pub fn stop(&self) {
        self.service.halt();
        let worker = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take();
        let _ = self.sender.send(Message::Stop);
        match worker {
            Some(worker) => {
                let _ = worker.join();
            }
            None => self.service.stop(self.clock.now()),
        }
    }
}
