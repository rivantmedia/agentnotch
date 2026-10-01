//! Who a config folder ran as, over time, so a session is attributed to the
//! account it started as rather than to whoever the folder names now
//! (FolderIdentityTimeline.swift, AU§5.6).
//!
//! Claude Parallel Profiles mirrors the focused VS Code window's account into
//! `~/.claude` (rewriting `~/.claude.json`) every time another window is
//! focused, while a Claude Code process that is already running keeps the
//! account it started with. The registry observes `~/.claude`'s identity
//! whenever it reads it, with the modification time of the `.claude.json` it
//! read and the file's own `accountUuid` (which a mirror keeps and a `/login`
//! replaces). A change is known to have happened after the last look that
//! still showed the old identity and no later than the write the new one was
//! read from; a process that started in between can't be attributed
//! (`Switching`), one that started before the timeline begins neither
//! (`Unknown`), and after a real login (the UUID itself changed) no earlier
//! process can be either: a `/login` typed in a running session moves that
//! session to the new account. Other folders change hands only when their
//! window switches account or someone signs in there, so they are attributed
//! to whoever they name now.
//!
//! The extension does nothing on native Windows (AU§0.2), so there the
//! timeline is kept but never consulted (`mirrored` is false). Pure.

use crate::core::time::IsoSeconds;
use crate::persist::accounts::{PersistedSpan, PersistedTimeline};
use std::time::SystemTime;

/// One stretch of time the folder named one identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The identity id (`uuid:…`, `email:…`); `None` when nobody was signed in.
    pub identity: Option<String>,
    /// The file's own `accountUuid`, lowercased (not corrected for a mirror).
    pub raw_uuid: Option<String>,
    /// It began with a real login (the file's UUID changed), not a mirror:
    /// processes that started earlier may have moved with it.
    pub is_login: bool,
    /// Held for certain from here: the `.claude.json` write it was first read
    /// from (or the look itself when the write time is unknown).
    pub from: SystemTime,
    /// The span before it was last seen at this time: the switch happened
    /// between this and `from`. `None` for the first span.
    pub after: Option<SystemTime>,
    /// The last look that still showed it.
    pub last_seen: SystemTime,
}

/// Who the folder ran as at some moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineAnswer {
    /// This identity (`None`: nobody signed in).
    Identity(Option<String>),
    /// It changed hands around then; which side is unknown.
    Switching,
    /// Before anything was recorded.
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderIdentityTimeline {
    spans: Vec<Span>,
}

impl FolderIdentityTimeline {
    /// Spans kept (oldest dropped first).
    pub const MAX_SPANS: usize = 24;

    pub fn new(spans: Vec<Span>) -> Self {
        let mut timeline = FolderIdentityTimeline { spans };
        timeline.trim();
        timeline
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The identity the folder names now, if it was ever observed.
    pub fn current(&self) -> Option<Option<&str>> {
        self.spans.last().map(|span| span.identity.as_deref())
    }

    /// Records a look at the folder: it names `identity`, read from a
    /// `.claude.json` last written at `modified_at`.
    ///
    /// `resumed` marks the first look after a relaunch: the folder wasn't
    /// watched meanwhile, so if its file was written since the last look the
    /// same identity may still have changed hands and back, and the time in
    /// between is left open. Returns whether a new span started (worth
    /// saving).
    pub fn observe(
        &mut self,
        identity: Option<&str>,
        raw_uuid: Option<&str>,
        modified_at: Option<SystemTime>,
        now: SystemTime,
        resumed: bool,
    ) -> bool {
        let raw_uuid = raw_uuid.map(str::to_lowercase);
        let Some(last) = self.spans.last().cloned() else {
            self.spans.push(Span {
                identity: identity.map(str::to_owned),
                raw_uuid,
                is_login: false,
                from: modified_at.unwrap_or(now).min(now),
                after: None,
                last_seen: now,
            });
            return true;
        };
        let written = modified_at.unwrap_or(now);
        let unwatched_write = resumed && written > last.last_seen;
        if last.identity.as_deref() == identity && last.raw_uuid == raw_uuid && !unwatched_write {
            if let Some(span) = self.spans.last_mut() {
                span.last_seen = span.last_seen.max(now);
            }
            return false;
        }
        self.spans.push(Span {
            identity: identity.map(str::to_owned),
            is_login: last.raw_uuid != raw_uuid,
            raw_uuid,
            from: written.max(last.last_seen).min(now),
            after: Some(last.last_seen),
            last_seen: now,
        });
        self.trim();
        true
    }

    fn trim(&mut self) {
        if self.spans.len() > Self::MAX_SPANS {
            let excess = self.spans.len() - Self::MAX_SPANS;
            self.spans.drain(..excess);
        }
    }

    /// Who the folder ran as at `date` (a process's start time).
    pub fn identity_at(&self, date: SystemTime) -> TimelineAnswer {
        let Some(first) = self.spans.first() else {
            return TimelineAnswer::Unknown;
        };
        if date < first.from {
            return TimelineAnswer::Unknown;
        }
        for index in (0..self.spans.len()).rev() {
            let span = &self.spans[index];
            if date >= span.from {
                // A later login may have taken this process with it.
                return if self.spans[index + 1..].iter().any(|s| s.is_login) {
                    TimelineAnswer::Switching
                } else {
                    TimelineAnswer::Identity(span.identity.clone())
                };
            }
            if span.after.is_some_and(|after| date > after) {
                return TimelineAnswer::Switching;
            }
        }
        TimelineAnswer::Unknown
    }

    /// As `accounts.json` keeps it (whole-second ISO dates, as the Mac).
    pub fn to_persisted(&self) -> PersistedTimeline {
        PersistedTimeline {
            spans: self
                .spans
                .iter()
                .map(|span| PersistedSpan {
                    identity: span.identity.clone(),
                    raw_uuid: span.raw_uuid.clone(),
                    is_login: span.is_login,
                    from: IsoSeconds(span.from),
                    after: span.after.map(IsoSeconds),
                    last_seen: IsoSeconds(span.last_seen),
                })
                .collect(),
        }
    }

    pub fn from_persisted(saved: &PersistedTimeline) -> Self {
        FolderIdentityTimeline::new(
            saved
                .spans
                .iter()
                .map(|span| Span {
                    identity: span.identity.clone(),
                    raw_uuid: span.raw_uuid.clone(),
                    is_login: span.is_login,
                    from: span.from.0,
                    after: span.after.map(|a| a.0),
                    last_seen: span.last_seen.0,
                })
                .collect(),
        )
    }
}

/// Which account a session in a folder runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderAttribution {
    /// Known: the identity id (`None`: nobody signed in, or a folder the
    /// registry hasn't grouped yet).
    Known(Option<String>),
    /// Can't be told: `~/.claude` while Claude Parallel Profiles mirrors
    /// accounts into it, for a process that started around a switch (or
    /// before anything was recorded). Holds who the folder names now.
    Unsure(Option<String>),
}

impl FolderAttribution {
    /// The identity to show the session under: the known one, else the
    /// folder's current one.
    pub fn best_guess(&self) -> Option<&str> {
        match self {
            FolderAttribution::Known(identity) | FolderAttribution::Unsure(identity) => {
                identity.as_deref()
            }
        }
    }

    /// Attributes a session in a folder whose process started at `started`.
    ///
    /// `current` is who the folder names now. `mirrored` says the folder is
    /// `~/.claude` and Claude Parallel Profiles mirrors accounts into it: a
    /// guess would be wrong on every focus switch there, so anything
    /// uncertain is `Unsure`. Other folders are attributed to whoever they
    /// name now.
    pub fn attribute(
        current: Option<&str>,
        timeline: Option<&FolderIdentityTimeline>,
        started: Option<SystemTime>,
        mirrored: bool,
    ) -> FolderAttribution {
        let current = current.map(str::to_owned);
        if !mirrored {
            return FolderAttribution::Known(current);
        }
        let (Some(started), Some(timeline)) = (started, timeline) else {
            return FolderAttribution::Unsure(current);
        };
        match timeline.identity_at(started) {
            TimelineAnswer::Identity(Some(identity)) => FolderAttribution::Known(Some(identity)),
            TimelineAnswer::Identity(None)
            | TimelineAnswer::Switching
            | TimelineAnswer::Unknown => FolderAttribution::Unsure(current),
        }
    }
}
