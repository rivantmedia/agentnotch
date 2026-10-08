//! Which usage limits of each account have been announced, so a limit is told
//! once per account until it lifts, however often it is seen again. A port of
//! the Mac's `LimitAnnouncements` and `LimitAnnouncementStore` (Release 1.0.2).
//!
//! Two things see an account hit its limit: the account's limit banner
//! (sessions stopped by the limit, `LimitBanners`) and the notch's chime and
//! peek for a failed turn (the hub's reactions). Each used to announce the same
//! limit, and again on every retry, wake-up, /loop tick and reopened session.
//! Both key an account by the ring the hub shows it on. (The Mac has a third:
//! upstream's "limit reached" card, which Windows doesn't have; the reaction is
//! then the limit's only announcement when no banner shows.)
//!
//! - An incident is one account (its ring) and one window: the 5-hour, the
//!   weekly, a model's weekly, or not known yet. It lasts until that window
//!   resets, or [`UNKNOWN_LIFETIME`] when no reset time is known, or until the
//!   readings show the window back under its limit (an early reset keeps the
//!   reset time): then using it up again is news.
//! - Each channel fires once per incident: a notification (a banner) and a
//!   reaction (the chime and peek).
//! - An incident of a window not known yet covers every window for
//!   [`UNKNOWN_ABSORBS_FOR`], and takes the first window and reset time it is
//!   told, by a claim or by the readings ([`LimitAnnouncements::learn`]): a turn
//!   fails on the limit before the readings say which window ran out. A window
//!   that runs out later than that is a limit of its own (the first failure was
//!   a burst of requests turned away, not a window running out).
//! - Kept in `<support>\limit-announcements.json`, so a relaunch doesn't tell a
//!   limit again. A sealed run has no hub of this kind.

use crate::core::time::IsoSeconds;
use crate::model::LimitWindowKind;
use crate::persist::limits::{LimitAnnouncementsFile, PersistedIncident, VERSION};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// A banner.
    Notification,
    /// The notch's chime and peek for a turn stopped by the limit.
    Reaction,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Channel::Notification => "notification",
            Channel::Reaction => "reaction",
        }
    }

    fn from_name(name: &str) -> Option<Channel> {
        match name {
            "notification" => Some(Channel::Notification),
            "reaction" => Some(Channel::Reaction),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    /// [`window_key`]: "session", "weekly", "scoped:<model>", or "unknown".
    pub window: String,
    /// When the window resets; `None` when nobody said.
    pub resets_at: Option<SystemTime>,
    pub started_at: SystemTime,
    pub channels: BTreeSet<Channel>,
}

impl Incident {
    pub fn expires_at(&self) -> SystemTime {
        // Checked: a start dated at the very end of what the platform's
        // `SystemTime` holds (a corrupt file) must not panic the hub.
        self.resets_at.unwrap_or_else(|| {
            self.started_at
                .checked_add(UNKNOWN_LIFETIME)
                .unwrap_or(self.started_at)
        })
    }
}

pub const UNKNOWN_WINDOW: &str = "unknown";
/// How long an incident without a reset time lasts. A limit the usage readings
/// don't show is most likely brief (a burst of requests turned away), so an
/// hour, not a whole window.
pub const UNKNOWN_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// How long an incident of a window not known yet waits for the readings to
/// name it: the longest automatic probe interval.
pub const UNKNOWN_ABSORBS_FOR: Duration = Duration::from_secs(30 * 60);
/// Incidents kept per ring at most (one per window is the most there can be at
/// once).
pub const MAX_INCIDENTS_PER_RING: usize = 8;

/// The key of the window a limit is announced under; `None` is "not known yet".
pub fn window_key(window: Option<&LimitWindowKind>) -> String {
    match window {
        Some(LimitWindowKind::Session) => "session".to_owned(),
        Some(LimitWindowKind::Weekly) => "weekly".to_owned(),
        Some(LimitWindowKind::Scoped(name)) => format!("scoped:{name}"),
        None => UNKNOWN_WINDOW.to_owned(),
    }
}

/// The announced incidents, per ring. Pure, so the rule is unit-tested.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitAnnouncements {
    pub incidents: BTreeMap<String, Vec<Incident>>,
}

impl LimitAnnouncements {
    /// `incident` speaks for a claim on the window `claimed` at `now`: the
    /// same window; a claim that names none; or an incident that names none
    /// yet, while the readings may still be catching up with it.
    pub fn covers(incident: &Incident, claimed: &str, now: SystemTime) -> bool {
        if incident.window == claimed || claimed == UNKNOWN_WINDOW {
            return true;
        }
        incident.window == UNKNOWN_WINDOW && waiting_for_a_name(incident, now)
    }

    /// Whether `channel` may announce `ring`'s limit on `window` (a
    /// [`window_key`]), resetting at `resets_at`: true the first time in an
    /// incident, which it then records, with `extra` channels marked too. A
    /// reset time that has already passed counts as unknown.
    pub fn claim(
        &mut self,
        channel: Channel,
        ring: &str,
        window: &str,
        resets_at: Option<SystemTime>,
        now: SystemTime,
        also_marking: &[Channel],
    ) -> bool {
        self.prune(now);
        let resets_at = resets_at.filter(|at| *at > now);
        let mut marked: BTreeSet<Channel> = also_marking.iter().copied().collect();
        marked.insert(channel);
        let list = self.incidents.entry(ring.to_owned()).or_default();
        let covering: Vec<usize> = (0..list.len())
            .filter(|&i| Self::covers(&list[i], window, now))
            .collect();
        let Some(&first) = covering.first() else {
            list.push(Incident {
                window: window.to_owned(),
                resets_at,
                started_at: now,
                channels: marked,
            });
            if list.len() > MAX_INCIDENTS_PER_RING {
                list.drain(..list.len() - MAX_INCIDENTS_PER_RING);
            }
            return true;
        };
        let is_new = !covering
            .iter()
            .any(|&i| list[i].channels.contains(&channel));
        let incident = &mut list[first];
        if incident.window == UNKNOWN_WINDOW && window != UNKNOWN_WINDOW {
            incident.window = window.to_owned();
            incident.resets_at = resets_at.or(incident.resets_at);
        } else if incident.window == window && incident.resets_at.is_none() {
            incident.resets_at = resets_at;
        }
        if is_new {
            incident.channels.extend(marked);
        }
        is_new
    }

    /// The readings now say which window ran out: an incident of a window not
    /// known yet (and still waiting for one) takes it and its reset time,
    /// announcing nothing, so it lasts until that reset rather than an hour.
    pub fn learn(
        &mut self,
        ring: &str,
        window: &str,
        resets_at: Option<SystemTime>,
        now: SystemTime,
    ) {
        self.prune(now);
        if window == UNKNOWN_WINDOW {
            return;
        }
        let Some(list) = self.incidents.get_mut(ring) else {
            return;
        };
        let Some(incident) = list.iter_mut().rev().find(|i| i.window == UNKNOWN_WINDOW) else {
            return;
        };
        if !waiting_for_a_name(incident, now) {
            return;
        }
        incident.window = window.to_owned();
        incident.resets_at = resets_at.filter(|at| *at > now).or(incident.resets_at);
    }

    /// The readings show `window` back under its limit before its reset time
    /// (an early reset, a reset credit): its incident is over, and using the
    /// window up again is a new limit.
    pub fn end(&mut self, ring: &str, window: &str) {
        let Some(list) = self.incidents.get_mut(ring) else {
            return;
        };
        list.retain(|incident| incident.window != window);
        if list.is_empty() {
            self.incidents.remove(ring);
        }
    }

    /// Drops the incidents whose window has reset (or that outlived
    /// [`UNKNOWN_LIFETIME`]).
    pub fn prune(&mut self, now: SystemTime) {
        self.incidents.retain(|_, list| {
            list.retain(|incident| incident.expires_at() > now);
            !list.is_empty()
        });
    }

    /// Some incident still waits for the readings to name its window.
    pub fn is_waiting_for_a_name(&self, now: SystemTime) -> bool {
        self.incidents
            .values()
            .flatten()
            .any(|i| i.window == UNKNOWN_WINDOW && waiting_for_a_name(i, now))
    }

    pub fn is_empty(&self) -> bool {
        self.incidents.is_empty()
    }

    fn to_file(&self) -> LimitAnnouncementsFile {
        LimitAnnouncementsFile {
            version: VERSION,
            incidents: self
                .incidents
                .iter()
                .map(|(ring, list)| {
                    let list = list
                        .iter()
                        .map(|i| PersistedIncident {
                            window: i.window.clone(),
                            resets_at: i.resets_at.map(IsoSeconds),
                            started_at: IsoSeconds(i.started_at),
                            channels: i.channels.iter().map(|c| c.name().to_owned()).collect(),
                        })
                        .collect();
                    (ring.clone(), list)
                })
                .collect(),
        }
    }

    /// `None` when a channel isn't one this knows: the file is then unreadable
    /// as a whole, as the Mac's decoder fails it.
    fn from_file(file: &LimitAnnouncementsFile) -> Option<LimitAnnouncements> {
        let mut incidents = BTreeMap::new();
        for (ring, list) in &file.incidents {
            let mut kept = Vec::with_capacity(list.len());
            for incident in list {
                kept.push(Incident {
                    window: incident.window.clone(),
                    resets_at: incident.resets_at.map(|at| at.0),
                    started_at: incident.started_at.0,
                    channels: incident
                        .channels
                        .iter()
                        .map(|name| Channel::from_name(name))
                        .collect::<Option<_>>()?,
                });
            }
            incidents.insert(ring.clone(), kept);
        }
        Some(LimitAnnouncements { incidents })
    }
}

fn waiting_for_a_name(incident: &Incident, now: SystemTime) -> bool {
    now.duration_since(incident.started_at)
        .map_or(true, |age| age <= UNKNOWN_ABSORBS_FOR)
}

/// The app's one [`LimitAnnouncements`] and the bytes of its file. The hub
/// writes what `take_bytes` hands over; a store that doesn't persist (a test
/// run's, or one with no folder) hands over nothing.
#[derive(Debug, Clone)]
pub struct LimitAnnouncementStore {
    state: LimitAnnouncements,
    persists: bool,
    unreadable: bool,
    /// Changed since the last `take_bytes`.
    changed: bool,
    /// Changed at all in this run: a store that never was writes nothing at a
    /// stop (no file of empty announcements).
    ever_changed: bool,
}

impl LimitAnnouncementStore {
    /// The store of the file's bytes (`None`: no file), with the incidents
    /// that already lifted left out. Unreadable bytes are an empty store.
    pub fn load(bytes: Option<&[u8]>, persists: bool, now: SystemTime) -> LimitAnnouncementStore {
        let mut unreadable = false;
        let mut state = match bytes.filter(|_| persists) {
            None => LimitAnnouncements::default(),
            Some(bytes) => LimitAnnouncementsFile::parse(bytes)
                .and_then(|file| LimitAnnouncements::from_file(&file))
                .unwrap_or_else(|| {
                    unreadable = true;
                    LimitAnnouncements::default()
                }),
        };
        state.prune(now);
        LimitAnnouncementStore {
            state,
            persists,
            unreadable,
            changed: false,
            ever_changed: false,
        }
    }

    /// The file was there and wasn't ours: the store started afresh.
    pub fn was_unreadable(&self) -> bool {
        self.unreadable
    }

    pub fn state(&self) -> &LimitAnnouncements {
        &self.state
    }

    fn update<T>(&mut self, change: impl FnOnce(&mut LimitAnnouncements) -> T) -> T {
        let mut next = self.state.clone();
        let result = change(&mut next);
        if next != self.state {
            self.state = next;
            self.changed = true;
            self.ever_changed = true;
        }
        result
    }

    /// [`LimitAnnouncements::claim`], to be saved when it changed anything.
    pub fn claim(
        &mut self,
        channel: Channel,
        ring: &str,
        window: Option<&LimitWindowKind>,
        resets_at: Option<SystemTime>,
        now: SystemTime,
        also_marking: &[Channel],
    ) -> bool {
        let window = window_key(window);
        self.update(|state| state.claim(channel, ring, &window, resets_at, now, also_marking))
    }

    /// [`LimitAnnouncements::learn`], to be saved when it changed anything.
    pub fn learn(
        &mut self,
        ring: &str,
        window: &LimitWindowKind,
        resets_at: Option<SystemTime>,
        now: SystemTime,
    ) {
        let window = window_key(Some(window));
        self.update(|state| state.learn(ring, &window, resets_at, now));
    }

    /// [`LimitAnnouncements::end`], to be saved when it changed anything.
    pub fn end(&mut self, ring: &str, window: &LimitWindowKind) {
        let window = window_key(Some(window));
        self.update(|state| state.end(ring, &window));
    }

    /// The file's bytes when something changed since the last time (and this
    /// store persists).
    pub fn take_bytes(&mut self) -> Option<Vec<u8>> {
        if !std::mem::take(&mut self.changed) || !self.persists {
            return None;
        }
        Some(self.state.to_file().encode())
    }

    /// The file's bytes for a stop: whatever this run changed (`None`: it
    /// changed nothing, or the store doesn't persist).
    pub fn bytes_now(&self) -> Option<Vec<u8>> {
        (self.persists && self.ever_changed).then(|| self.state.to_file().encode())
    }
}
