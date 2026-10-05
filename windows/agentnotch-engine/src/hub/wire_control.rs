//! Control, wired into the hub (design §4.8-§4.10; HS§7-§9; UI§3.8): the
//! jump to a session's terminal, typed replies, banners, chimes, peeks, the
//! panel that opens by itself, and "is the user looking at it".
//!
//! - Each session's host (`Job::Classify`) and console (`Job::ConsoleInfo`)
//!   are looked up on `an-ui` when the session is first seen, keyed by its
//!   pid and start time (Windows reuses pids). A console's facts are reused
//!   for 30 s; the first console window seen is kept, and a reply is only
//!   typed while the console still names it (the Mac's "the process's TTY is
//!   the session's").
//! - `focus` runs the session's focus plan on `an-ui`; a jump that brought
//!   something forward marks the session reviewed and closes an unpinned
//!   panel.
//! - `send_message` is refused while typing replies is off. A reply waits
//!   (checked again every 250 ms, for up to 10 s) while Claude is busy, is
//!   typed on `an-ui`, and between the text and Return `an-core` checks the
//!   session again on its state then (`Input::TypeCheckpoint`): a request
//!   that appeared in the gap means Return is never pressed, so our Return
//!   can't confirm a dialog the user never saw.
//! - Attention transitions (after the store's silent launch baseline) post
//!   and withdraw banners at once; chime, peek and auto-open are decided per
//!   burst once a visibility scan (`Job::Visibility`) says whether a terminal
//!   is on screen. A panel opened by itself is followed until it closes, and
//!   closed at its deadline unless the user took it over.
//! - A foreground window that stays 1.5 s marks the completions it shows
//!   viewed.
//!
//! Owner: WP7.

use super::api::{CallError, HubEvent, OutcomeReply, RouteReply};
use super::core_state::{to_value, Core, Reply};
use super::project::{self, RowExtras};
use crate::attention::policy::{self, Burst, TransitionKind, BURST_WINDOW};
use crate::control::focus::{self, FocusExtras};
use crate::control::notifications::{
    self, LimitBanners, LimitChange, LimitContext, LimitedSession,
};
use crate::control::panel::{self, AutoOpenWatch};
use crate::control::{hosts, looking, messaging, reactions};
use crate::model::{
    AttentionTransition, HubSnapshot, RingId, SessionId, SessionState, SessionView,
};
use crate::platform::{
    ConsoleInfo, FocusOutcome, Foreground, HostApp, HostKind, Liveness, Processes, TypeOutcome,
};
use crate::runtime_types::{
    Input, Job, JobId, PanelState, ReactionContext, ReviewAction, RingReading, SessionInput,
    ToastContext,
};
use crate::usage::ring_windows;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime};

/// A process as the lookups know it: its pid and start time.
type ProcKey = (u32, SystemTime);

/// A process at the session's pid that started this much later than the
/// session's own is another process (the session store's tolerance).
const PID_REUSE_TOLERANCE: Duration = Duration::from_secs(1);

/// A host that wasn't found is looked for again after this long (the Mac's
/// `HostAppCache`): the terminal may have been reattached.
const HOST_RETRY: Duration = Duration::from_secs(30);

/// `PanelClose`'s reason after a jump.
const REASON_JUMP: &str = "jump";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LookupKind {
    Host,
    Console,
}

/// A call waiting on lookups (or, for a reply, on Claude).
enum Waiter {
    Focus {
        session: SessionId,
        reply: Reply,
    },
    Send {
        session: SessionId,
        text: String,
        reply: Reply,
        since: SystemTime,
        /// When a busy session is asked again; `None` while waiting on a
        /// lookup (its result wakes the waiter).
        next_check: Option<SystemTime>,
    },
    Route {
        session: SessionId,
        reply: Reply,
    },
}

impl Waiter {
    fn reply(self) -> Reply {
        match self {
            Waiter::Focus { reply, .. }
            | Waiter::Send { reply, .. }
            | Waiter::Route { reply, .. } => reply,
        }
    }
}

/// A transition of the open burst, waiting for the visibility scan.
struct Deciding {
    tr: AttentionTransition,
    looking_at: Option<bool>,
    ring_shown: bool,
}

/// What a session's lookups found.
enum Facts {
    /// No process to look up.
    None,
    Waiting,
    Ready(HostApp, ConsoleInfo),
}

#[derive(Default)]
pub(crate) struct ControlWiring {
    hosts: HashMap<ProcKey, (HostApp, SystemTime)>,
    consoles: HashMap<ProcKey, (ConsoleInfo, SystemTime)>,
    /// The first console window each process was seen with.
    first_window: HashMap<ProcKey, u64>,
    lookups: HashMap<JobId, (ProcKey, LookupKind)>,
    waiters: Vec<Waiter>,
    typing: HashMap<JobId, SessionId>,
    focusing: HashMap<JobId, (SessionId, HostApp)>,
    burst: Burst,
    deciding: Vec<Deciding>,
    visibility: Option<JobId>,
    limits: LimitBanners,
    /// The panel the engine opened by itself, and whether the glue has
    /// reported it open yet.
    auto_open: Option<(AutoOpenWatch, bool)>,
    /// The foreground window and when it has stayed long enough.
    foreground: Option<(Foreground, SystemTime)>,
    watching_foreground: bool,
}

impl ControlWiring {
    fn in_flight(&self, key: ProcKey, kind: LookupKind) -> bool {
        self.lookups.values().any(|lookup| *lookup == (key, kind))
    }

    fn fresh_console(&self, key: ProcKey, now: SystemTime) -> Option<&ConsoleInfo> {
        self.consoles
            .get(&key)
            .filter(|(_, at)| is_fresh(*at, now, messaging::CONSOLE_INFO_LIFETIME))
            .map(|(info, _)| info)
    }
}

fn is_fresh(at: SystemTime, now: SystemTime, lifetime: Duration) -> bool {
    // A clock that went back keeps what it has.
    now.duration_since(at).map_or(true, |age| age < lifetime)
}

fn proc_key(view: &SessionView) -> Option<ProcKey> {
    Some((view.pid?, view.pid_started?))
}

/// The process at `pid` runs now and is the one started at `started`: a
/// start time read now, within the tolerance. Unknown is not confirmed.
fn process_confirmed(processes: &dyn Processes, (pid, started): ProcKey) -> bool {
    if processes.liveness(pid) == Liveness::Gone {
        return false;
    }
    let Some(current) = processes.start_time(pid) else {
        return false;
    };
    let apart = match current.duration_since(started) {
        Ok(apart) => apart,
        Err(earlier) => earlier.duration(),
    };
    apart < PID_REUSE_TOLERANCE
}

fn unknown_host() -> HostApp {
    HostApp {
        kind: HostKind::Unknown,
        window: None,
        host_pid: None,
        exe_path: None,
    }
}

fn send_outcome(reply: Reply, outcome: &str, reason: Option<String>) {
    let _ = reply.send(to_value(&OutcomeReply {
        outcome: outcome.to_owned(),
        reason,
    }));
}

fn refused(reply: Reply, reason: impl Into<String>) {
    send_outcome(reply, "refused", Some(reason.into()));
}

fn send_route(reply: Reply, route: Result<(), String>) {
    let _ = reply.send(to_value(&RouteReply {
        available: route.is_ok(),
        reason: route.err(),
    }));
}

/// The row's host, jump and composer, from what the lookups found so far.
pub(crate) fn row_extras(w: &ControlWiring, view: &SessionView, type_replies: bool) -> RowExtras {
    let key = proc_key(view);
    let host = key.and_then(|k| w.hosts.get(&k)).map(|(h, _)| h);
    let info = key.and_then(|k| w.consoles.get(&k)).map(|(i, _)| i);
    RowExtras {
        host_app: hosts::host_app_name(host, view.entrypoint.as_deref()),
        can_focus: focus::can_focus(view, host, info, &FocusExtras::default()),
        can_message: type_replies
            && match (host, info) {
                (Some(host), Some(info)) => {
                    messaging::availability(true, false, Some(view), info, host).is_ok()
                }
                _ => false,
            },
    }
}

impl Core {
    // ---- start and stop ----

    /// The hub starts: the foreground window is followed (once per run).
    pub(crate) fn control_on_start(&mut self) {
        if self.control_w.watching_foreground || self.cfg.flags.sealed {
            return;
        }
        let Some(inputs) = self.ingress_w.inputs.clone() else {
            return;
        };
        let (sink, events) = crossbeam_channel::unbounded::<Foreground>();
        // The platform's watcher has a thread of its own; this one passes
        // its reports on in order, until the watcher lets its sink go.
        let forwarder = std::thread::Builder::new()
            .name("an-foreground-events".into())
            .spawn(move || {
                while let Ok(fg) = events.recv() {
                    if inputs.send(Input::Foreground(fg)).is_err() {
                        return;
                    }
                }
            });
        match forwarder {
            Ok(_) => {
                self.platform.terminals.watch_foreground(sink);
                self.control_w.watching_foreground = true;
            }
            Err(e) => self.log(format!("the foreground window isn't followed: {e}")),
        }
    }

    /// The hub stops: calls still waiting are answered, and nothing decided
    /// before is carried out later.
    pub(crate) fn control_on_stop(&mut self, why: &str) {
        for waiter in std::mem::take(&mut self.control_w.waiters) {
            let _ = waiter.reply().send(Err(CallError::failed(why)));
        }
        let w = &mut self.control_w;
        w.burst = Burst::default();
        w.deciding.clear();
        w.visibility = None;
        w.auto_open = None;
        w.foreground = None;
        w.typing.clear();
        w.focusing.clear();
        w.lookups.clear();
    }

    // ---- inputs ----

    pub(crate) fn foreground_changed(&mut self, fg: Foreground, now: SystemTime) {
        self.control_w.foreground = Some((fg, now + looking::VIEWED_DWELL));
    }

    /// `panel_state` from the glue: the reactions read it, and a panel the
    /// engine opened by itself is followed through it.
    pub(crate) fn panel_reported(&mut self, state: PanelState) {
        if let Some((mut watch, mut seen)) = self.control_w.auto_open.take() {
            let auto = state.open && state.reason.as_deref() == Some(panel::REASON_AUTO);
            // A report from before the glue opened it doesn't end the watch;
            // a close after it was open, or another way of opening, does.
            let keep = if auto {
                seen = true;
                watch.panel_reported(&state)
            } else {
                !state.open && !seen
            };
            if keep {
                self.control_w.auto_open = Some((watch, seen));
            }
        }
        self.panel = state;
    }

    // ---- calls ----

    /// `focus {session_id}` → `{outcome, reason?}`, answered when the jump
    /// is done.
    pub(crate) fn focus_call(&mut self, session: SessionId, reply: Reply) {
        self.control_w
            .waiters
            .push(Waiter::Focus { session, reply });
        let now = self.platform.clock.now();
        self.resolve_waiters(now);
    }

    /// `send_message {session_id, text}` → `{outcome, reason?}`.
    pub(crate) fn send_message_call(&mut self, session: SessionId, text: &str, reply: Reply) {
        if !self.settings.type_replies {
            return refused(reply, messaging::TYPING_OFF);
        }
        let text = match messaging::prepare(text) {
            Ok(text) => text,
            Err(reason) => return refused(reply, reason),
        };
        let now = self.platform.clock.now();
        self.control_w.waiters.push(Waiter::Send {
            session,
            text,
            reply,
            since: now,
            next_check: None,
        });
        self.resolve_waiters(now);
    }

    /// `message_route {session_id}` → `{available, reason?}`.
    pub(crate) fn message_route_call(&mut self, session: SessionId, reply: Reply) {
        if self.cfg.flags.sealed {
            return send_route(reply, Err(messaging::SEALED.into()));
        }
        if !self.settings.type_replies {
            return send_route(reply, Err(messaging::TYPING_OFF.into()));
        }
        self.control_w
            .waiters
            .push(Waiter::Route { session, reply });
        let now = self.platform.clock.now();
        self.resolve_waiters(now);
    }

    /// The calls waiting on lookups or on Claude, tried again.
    pub(crate) fn resolve_waiters(&mut self, now: SystemTime) {
        if self.control_w.waiters.is_empty() {
            return;
        }
        let views = self.session_views();
        for waiter in std::mem::take(&mut self.control_w.waiters) {
            if let Some(back) = self.try_waiter(waiter, &views, now) {
                self.control_w.waiters.push(back);
            }
        }
    }

    fn try_waiter(
        &mut self,
        waiter: Waiter,
        views: &[SessionView],
        now: SystemTime,
    ) -> Option<Waiter> {
        match waiter {
            Waiter::Focus { session, reply } => {
                let Some(view) = views.iter().find(|v| v.id == session) else {
                    let (word, reason) =
                        focus::outcome_reply(&FocusOutcome::NotFound, &unknown_host());
                    send_outcome(reply, word, reason);
                    return None;
                };
                let (host, info) = match self.facts(view, now) {
                    Facts::Waiting => return Some(Waiter::Focus { session, reply }),
                    Facts::None => (unknown_host(), ConsoleInfo::default()),
                    Facts::Ready(host, info) => (host, info),
                };
                let root = self.workspace_root_for(view, &host);
                let extras = FocusExtras {
                    workspace_root: root.as_deref().map(std::path::Path::new),
                    fallback_editor: None,
                };
                let steps = focus::focus_plan_with(view, &host, &info, &extras);
                if steps.is_empty() {
                    let (word, reason) = focus::outcome_reply(&FocusOutcome::NotFound, &host);
                    send_outcome(reply, word, reason);
                    return None;
                }
                let id = self.schedule(Job::Focus { steps }, Some(reply));
                self.control_w.focusing.insert(id, (session, host));
                None
            }
            Waiter::Route { session, reply } => {
                let sealed = self.cfg.flags.sealed;
                let type_replies = self.settings.type_replies;
                let view = views.iter().find(|v| v.id == session);
                let route = match view {
                    None => messaging::availability(
                        type_replies,
                        sealed,
                        None,
                        &ConsoleInfo::default(),
                        &unknown_host(),
                    ),
                    Some(view) => match self.facts(view, now) {
                        Facts::Waiting => return Some(Waiter::Route { session, reply }),
                        Facts::None => messaging::availability(
                            type_replies,
                            sealed,
                            Some(view),
                            &ConsoleInfo::default(),
                            &unknown_host(),
                        ),
                        Facts::Ready(host, info) => {
                            messaging::availability(type_replies, sealed, Some(view), &info, &host)
                        }
                    },
                };
                send_route(reply, route);
                None
            }
            Waiter::Send {
                session,
                text,
                reply,
                since,
                next_check,
            } => {
                if !self.settings.type_replies {
                    refused(reply, messaging::TYPING_OFF);
                    return None;
                }
                let Some(view) = views.iter().find(|v| v.id == session) else {
                    refused(reply, messaging::SESSION_ENDED);
                    return None;
                };
                let (host, info) = match self.facts(view, now) {
                    Facts::Waiting => {
                        return Some(Waiter::Send {
                            session,
                            text,
                            reply,
                            since,
                            next_check: None,
                        })
                    }
                    Facts::None => (unknown_host(), ConsoleInfo::default()),
                    Facts::Ready(host, info) => (host, info),
                };
                if next_check.is_some_and(|at| at > now) {
                    return Some(Waiter::Send {
                        session,
                        text,
                        reply,
                        since,
                        next_check,
                    });
                }
                let waited = now.duration_since(since).unwrap_or_default();
                let check = messaging::typing_check(view, &info, &host, None);
                match messaging::hold(check, waited) {
                    messaging::Hold::Go => {
                        let Some(key) = proc_key(view) else {
                            refused(reply, messaging::PROCESS_UNKNOWN);
                            return None;
                        };
                        // The process tree is read here, once per reply: the
                        // console must hold only Claude, its children and
                        // the shells that started it.
                        let table = self.platform.processes.table();
                        let recorded = self.control_w.first_window.get(&key).copied();
                        match messaging::console_target_with(view, &info, &table, recorded) {
                            Err(reason) => refused(reply, reason),
                            Ok(target) => {
                                let id = self.schedule(
                                    Job::Type {
                                        session: session.clone(),
                                        target,
                                        text,
                                    },
                                    Some(reply),
                                );
                                self.control_w.typing.insert(id, session);
                            }
                        }
                        None
                    }
                    messaging::Hold::Wait => Some(Waiter::Send {
                        session,
                        text,
                        reply,
                        since,
                        next_check: Some(now + messaging::HOLD_POLL),
                    }),
                    messaging::Hold::Refuse(reason) => {
                        refused(reply, reason);
                        None
                    }
                }
            }
        }
    }

    /// The session's host and fresh console facts, looked up when missing.
    fn facts(&mut self, view: &SessionView, now: SystemTime) -> Facts {
        let Some(key) = proc_key(view) else {
            return Facts::None;
        };
        self.want_host(key, now);
        self.want_console(key, now, true);
        let host = self.control_w.hosts.get(&key).map(|(h, _)| h.clone());
        let info = self.control_w.fresh_console(key, now).cloned();
        match (host, info) {
            (Some(host), Some(info)) => Facts::Ready(host, info),
            _ => Facts::Waiting,
        }
    }

    fn want_host(&mut self, key: ProcKey, now: SystemTime) {
        let known = self.control_w.hosts.get(&key).is_some_and(|(host, at)| {
            host.kind != HostKind::Unknown || is_fresh(*at, now, HOST_RETRY)
        });
        if known || self.control_w.in_flight(key, LookupKind::Host) {
            return;
        }
        let id = self.schedule(Job::Classify { pid: key.0 }, None);
        self.control_w.lookups.insert(id, (key, LookupKind::Host));
    }

    fn want_console(&mut self, key: ProcKey, now: SystemTime, fresh: bool) {
        let known = if fresh {
            self.control_w.fresh_console(key, now).is_some()
        } else {
            self.control_w.consoles.contains_key(&key)
        };
        if known || self.control_w.in_flight(key, LookupKind::Console) {
            return;
        }
        let id = self.schedule(
            Job::ConsoleInfo {
                pid: key.0,
                started: key.1,
            },
            None,
        );
        self.control_w
            .lookups
            .insert(id, (key, LookupKind::Console));
    }

    /// The folder an editor most likely has open for a CLI session in its
    /// terminal (its git top level below home).
    fn workspace_root_for(&self, view: &SessionView, host: &HostApp) -> Option<String> {
        if !matches!(host.kind, HostKind::VsCode { .. })
            || hosts::is_vscode_extension(view.entrypoint.as_deref())
        {
            return None;
        }
        let cwd = view.cwd.to_string_lossy();
        focus::workspace_root(&cwd, self.registry.paths(), &|path| {
            std::path::Path::new(path).exists()
        })
    }

    // ---- job results ----

    pub(crate) fn host_read(&mut self, id: JobId, host: HostApp, now: SystemTime) {
        if let Some((key, _)) = self.control_w.lookups.remove(&id) {
            self.control_w.hosts.insert(key, (host, now));
        }
    }

    pub(crate) fn console_read(&mut self, id: JobId, info: ConsoleInfo, now: SystemTime) {
        let Some((key, _)) = self.control_w.lookups.remove(&id) else {
            return;
        };
        if info.attached {
            if let Some(window) = info.window {
                self.control_w.first_window.entry(key).or_insert(window);
            }
        }
        self.control_w.consoles.insert(key, (info, now));
    }

    pub(crate) fn focus_done(
        &mut self,
        id: JobId,
        outcome: FocusOutcome,
        reply: &mut Option<Reply>,
        now: SystemTime,
    ) {
        let Some((session, host)) = self.control_w.focusing.remove(&id) else {
            return;
        };
        let (word, reason) = focus::outcome_reply(&outcome, &host);
        if focus::jumped(&outcome) {
            self.sessions_input(
                SessionInput::Review(ReviewAction::MarkReviewed { session, at: now }),
                now,
            );
            if !(self.settings.panel_pinned || self.panel.pinned) {
                self.push_event(HubEvent::PanelClose {
                    reason: REASON_JUMP.into(),
                });
            }
        }
        if let Some(reply) = reply.take() {
            send_outcome(reply, word, reason);
        }
    }

    pub(crate) fn typed(&mut self, id: JobId, outcome: TypeOutcome, reply: &mut Option<Reply>) {
        if self.control_w.typing.remove(&id).is_none() {
            return;
        }
        let (word, reason) = messaging::outcome_reply(&outcome);
        self.log(format!("reply typed: {word}"));
        if let Some(reply) = reply.take() {
            send_outcome(reply, word, reason);
        }
    }

    /// Between a typed reply and its Return: the session as it is now. A
    /// request of any agent, a dialog, a busy turn or a lost console means
    /// no Return.
    pub(crate) fn control_checkpoint(&mut self, job: JobId) -> bool {
        let Some(session) = self.control_w.typing.get(&job).cloned() else {
            return false;
        };
        if !self.settings.type_replies {
            return false;
        }
        let views = self.session_views();
        let Some(view) = views.iter().find(|v| v.id == session) else {
            return false;
        };
        let Some(key) = proc_key(view) else {
            return false;
        };
        // Return goes only to the process the session recorded. The store
        // keeps a session whose pid it can't date as running; for typing, a
        // start time that can't be read now confirms nothing.
        if !process_confirmed(self.platform.processes.as_ref(), key) {
            return false;
        }
        let Some((info, _)) = self.control_w.consoles.get(&key) else {
            return false;
        };
        let host = self
            .control_w
            .hosts
            .get(&key)
            .map_or_else(unknown_host, |(h, _)| h.clone());
        messaging::message_safety(view, info, &host).is_ok()
    }

    pub(crate) fn visible_read(
        &mut self,
        id: JobId,
        any_terminal: bool,
        full_screen: bool,
        now: SystemTime,
    ) {
        if self.control_w.visibility != Some(id) {
            return;
        }
        self.control_w.visibility = None;
        for d in std::mem::take(&mut self.control_w.deciding) {
            let ctx =
                self.reaction_context(now, d.looking_at, d.ring_shown, any_terminal, full_screen);
            let made = reactions::decide(&d.tr, &ctx);
            self.control_w.burst.finish(made);
        }
        self.close_burst(now);
    }

    // ---- attention ----

    fn reaction_context(
        &self,
        now: SystemTime,
        looking_at: Option<bool>,
        ring_shown: bool,
        any_terminal_visible: bool,
        full_screen: bool,
    ) -> ReactionContext {
        ReactionContext {
            now,
            auto_open: self.settings.auto_open.clone(),
            sound: self.settings.sound,
            peek: self.settings.peek,
            peek_seconds: self.settings.peek_seconds,
            panel: self.panel.clone(),
            full_screen,
            any_terminal_visible,
            looking_at,
            ring_shown,
            notch_hidden: false,
        }
    }

    /// The store's attention news: banners now, the rest per burst.
    pub(crate) fn attention_transitions(
        &mut self,
        transitions: Vec<AttentionTransition>,
        now: SystemTime,
    ) {
        if transitions.is_empty() {
            return;
        }
        for tr in &transitions {
            for (tag, group) in notifications::withdrawals(tr) {
                self.platform.notifier.withdraw(&tag, &group);
            }
        }
        if !self.live {
            return;
        }
        // Placed as the pages show them: a session listed nowhere (its
        // account hidden or forgotten) makes no noise.
        let snapshot = self.snapshot_at(now, 0);
        let all_labels: BTreeMap<String, String> = snapshot
            .rings
            .iter()
            .map(|r| (r.ring_id.clone(), r.label.clone()))
            .collect();
        let fg = self.platform.terminals.foreground();
        let mut viewed = Vec::new();
        let mut limit_rings = BTreeSet::new();
        for mut tr in transitions {
            let Some(row) = snapshot
                .sessions
                .iter()
                .find(|row| row.session_id == tr.session.id.as_str())
            else {
                continue;
            };
            tr.session.ring = row.ring_id.clone().map(RingId::from);
            if LimitBanners::concerns(&tr) {
                if let Some(ring) = &tr.session.ring {
                    limit_rings.insert(ring.clone());
                }
            }
            let looking = fg.as_ref().and_then(|fg| self.looking_at(&tr.session, fg));
            // A turn that finished while its own terminal was in front was
            // watched.
            if tr.became_ready_for_review() && looking == Some(true) {
                if let Some(completed_at) = tr.session.completed_at {
                    viewed.push((tr.session.id.clone(), completed_at));
                }
            }
            let ring = row
                .ring_id
                .as_deref()
                .and_then(|id| snapshot.rings.iter().find(|r| r.ring_id == id));
            let ctx = ToastContext {
                now,
                notify_needs_input: self.settings.notify_needs_input,
                notify_ready_for_review: self.settings.notify_ready_for_review,
                permission: self.notify_permission,
                suppressed: self.cfg.flags.no_notifications
                    || fg.as_ref().is_some_and(|fg| fg.fullscreen),
                looking_at: looking,
                account_label: ring
                    .and_then(|r| self.account_subtitle(&r.ring_id, &r.label, &all_labels)),
                multi_account: all_labels.len() > 1,
                title: tr.session.public_title.clone(),
                project: tr.session.project_name.clone(),
            };
            if let Some(toast) = notifications::toast_for(&tr, &ctx) {
                self.platform.notifier.post(&toast);
            }
            // Resolutions make no sound.
            if matches!(
                policy::kind_of(&tr),
                Some(TransitionKind::NeedsInput | TransitionKind::ReadyForReview)
            ) {
                self.control_w.burst.begin(now);
                self.control_w.deciding.push(Deciding {
                    tr,
                    looking_at: looking,
                    ring_shown: ring.is_some_and(|r| r.shown),
                });
            }
        }
        for ring in limit_rings {
            self.update_limit_banner(&ring, &snapshot, now);
        }
        if !self.control_w.deciding.is_empty() && self.control_w.visibility.is_none() {
            self.control_w.visibility = Some(self.schedule(Job::Visibility, None));
        }
        for (session, completed_at) in viewed {
            self.sessions_input(
                SessionInput::Review(ReviewAction::MarkViewed {
                    session,
                    completed_at,
                }),
                now,
            );
        }
    }

    /// A banner's subtitle for the account of `ring_id`.
    fn account_subtitle(
        &self,
        ring_id: &str,
        label: &str,
        all_labels: &BTreeMap<String, String>,
    ) -> Option<String> {
        let accounts = self.registry.accounts();
        let account = accounts.iter().find(|a| a.ring_id.as_str() == ring_id);
        let folder = account
            .and_then(|a| a.run_dirs.first())
            .and_then(|dir| {
                std::path::Path::new(dir.as_str())
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        notifications::account_subtitle(
            ring_id,
            label,
            None,
            account.and_then(|a| a.plan_name.as_deref()),
            &folder,
            all_labels,
        )
    }

    fn update_limit_banner(&mut self, ring: &RingId, snapshot: &HubSnapshot, now: SystemTime) {
        let views = self.session_views();
        let limited: Vec<LimitedSession> = snapshot
            .sessions
            .iter()
            .filter(|row| row.ring_id.as_deref() == Some(ring.as_str()))
            .filter_map(|row| views.iter().find(|v| v.id.as_str() == row.session_id))
            .filter(|v| v.state.reason().is_some_and(notifications::is_rate_limit))
            .map(|v| LimitedSession {
                id: v.id.clone(),
                title: v.public_title.clone(),
            })
            .collect();
        let label = snapshot
            .rings
            .iter()
            .find(|r| r.ring_id == ring.as_str())
            .map(|r| r.label.clone());
        let ctx = LimitContext {
            notify_needs_input: self.settings.notify_needs_input,
            permission: self.notify_permission,
            suppressed: self.cfg.flags.no_notifications,
            account_label: label.filter(|_| snapshot.rings.len() > 1),
            limit_reset: self.limit_reset(ring, now),
        };
        match self.control_w.limits.update(ring, &limited, &ctx) {
            LimitChange::Post(toast) => self.platform.notifier.post(&toast),
            LimitChange::Withdraw(tag, group) => self.platform.notifier.withdraw(&tag, &group),
            LimitChange::Nothing => {}
        }
    }

    /// When the ring's spent window lifts, in the user's words.
    fn limit_reset(&self, ring: &RingId, now: SystemTime) -> Option<String> {
        let accounts = self.registry.accounts();
        let account = accounts.iter().find(|a| &a.ring_id == ring)?;
        let RingReading::Reading { usage, .. } = self.usage.ring_reading(&account.identity_id, now)
        else {
            return None;
        };
        let resets_at = ring_windows::windows(&usage, now)
            .into_iter()
            .filter(|w| w.used_fraction >= 1.0)
            .filter_map(|w| w.resets_at)
            .max()?;
        notifications::reset_phrase(
            Some(resets_at),
            now,
            notifications::local_utc_offset_seconds(now),
        )
    }

    /// Whether `fg` shows this session's own terminal (`None`: can't tell).
    fn looking_at(&self, view: &SessionView, fg: &Foreground) -> Option<bool> {
        let key = proc_key(view)?;
        let host = self.control_w.hosts.get(&key).map(|(h, _)| h)?;
        let all: Vec<&HostApp> = self.control_w.hosts.values().map(|(h, _)| h).collect();
        let sharing = looking::sessions_sharing(host, &all);
        match self.control_w.consoles.get(&key) {
            Some((info, _)) => {
                let others: Vec<String> = self
                    .control_w
                    .hosts
                    .iter()
                    .filter(|(k, (h, _))| {
                        **k != key && h.window.is_some() && h.window == host.window
                    })
                    .filter_map(|(k, _)| {
                        self.control_w
                            .consoles
                            .get(k)
                            .and_then(|(info, _)| info.title.clone())
                    })
                    .collect();
                looking::looking_at_console(view, host, info, fg, sharing, &others)
            }
            None => looking::looking_at(view, host, fg, sharing),
        }
    }

    /// A closed burst's chime, peek or panel.
    fn close_burst(&mut self, now: SystemTime) {
        let Some(merged) = self.control_w.burst.close(now) else {
            return;
        };
        if merged.is_empty() {
            return;
        }
        let default_ring = RingId::from(project::default_ring_id(&self.registry.accounts()));
        let done = reactions::carry_out(&merged, self.settings.peek_seconds, Some(&default_ring));
        let mut said = Vec::new();
        if let Some(chime) = done.chime {
            said.push(format!("chime {chime:?}"));
            self.platform.sounds.play(chime);
        }
        if let Some((ring, seconds)) = done.peek {
            said.push(format!("peek {}", ring.as_str()));
            self.push_event(HubEvent::Peek {
                ring_id: ring.as_str().to_owned(),
                seconds,
            });
        }
        if let Some(request) = done.open_panel {
            // Never over a panel the user has open (the policy saw it closed
            // when it decided; it may have opened since).
            if !self.panel.open {
                if let Some(session) = request.highlight.clone().map(SessionId::from) {
                    let views = self.session_views();
                    let state = views.iter().find(|v| v.id == session).map(|v| &v.state);
                    let watch = AutoOpenWatch::opened(session.clone(), state, now);
                    self.control_w.auto_open = Some((watch, false));
                }
                said.push("auto-open".to_owned());
                self.push_event(HubEvent::Panel(request));
            }
        }
        if !said.is_empty() {
            self.log(format!("attention: {}", said.join(", ")));
        }
    }

    // ---- the schedule ----

    /// While the hub runs: lookups for new sessions, the burst's close, the
    /// auto-opened panel's deadline and the foreground dwell.
    pub(crate) fn drive_control(&mut self, now: SystemTime) {
        let views = self.session_views();
        let keys: HashSet<ProcKey> = views.iter().filter_map(proc_key).collect();
        for key in &keys {
            self.want_host(*key, now);
            // Once at first sight, to record its console window.
            self.want_console(*key, now, false);
        }
        let w = &mut self.control_w;
        w.hosts.retain(|k, _| keys.contains(k));
        w.consoles.retain(|k, _| keys.contains(k));
        w.first_window.retain(|k, _| keys.contains(k));
        if self.control_w.burst.pending() == 0 {
            self.close_burst(now);
        }
        self.watch_auto_open(&views, now);
        self.viewed_after_dwell(&views, now);
    }

    fn watch_auto_open(&mut self, views: &[SessionView], now: SystemTime) {
        let Some((mut watch, seen)) = self.control_w.auto_open.take() else {
            return;
        };
        let state: Option<&SessionState> = views
            .iter()
            .find(|v| v.id == watch.session)
            .map(|v| &v.state);
        watch.session_is(state, now);
        if watch
            .deadline(self.settings.peek_seconds)
            .is_some_and(|at| at <= now)
        {
            self.push_event(HubEvent::PanelClose {
                reason: panel::REASON_AUTO.into(),
            });
            return;
        }
        self.control_w.auto_open = Some((watch, seen));
    }

    fn viewed_after_dwell(&mut self, views: &[SessionView], now: SystemTime) {
        let Some((fg, due)) = self.control_w.foreground.clone() else {
            return;
        };
        if now < due {
            return;
        }
        self.control_w.foreground = None;
        let marks: Vec<(SessionId, SystemTime)> = views
            .iter()
            .filter(|v| v.state == SessionState::ReadyForReview)
            .filter_map(|v| Some((v, v.completed_at?)))
            .filter(|(v, _)| self.looking_at(v, &fg) == Some(true))
            .map(|(v, completed_at)| (v.id.clone(), completed_at))
            .collect();
        for (session, completed_at) in marks {
            self.sessions_input(
                SessionInput::Review(ReviewAction::MarkViewed {
                    session,
                    completed_at,
                }),
                now,
            );
        }
    }

    pub(crate) fn control_deadline(&self) -> Option<SystemTime> {
        let w = &self.control_w;
        // A burst still waiting on its scan closes when the scan is back.
        let burst = w
            .burst
            .opened_at()
            .filter(|_| w.burst.pending() == 0)
            .map(|at| at + BURST_WINDOW);
        let sends = w
            .waiters
            .iter()
            .filter_map(|waiter| match waiter {
                Waiter::Send { next_check, .. } => *next_check,
                _ => None,
            })
            .min();
        let auto = w
            .auto_open
            .as_ref()
            .and_then(|(watch, _)| watch.deadline(self.settings.peek_seconds));
        let dwell = w.foreground.as_ref().map(|(_, due)| *due);
        [burst, sends, auto, dwell].into_iter().flatten().min()
    }
}
