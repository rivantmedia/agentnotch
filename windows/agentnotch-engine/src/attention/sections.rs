//! SessionSections: how the sessions panel groups and orders sessions, when
//! a section folds and when rows go compact, and the counts its attention
//! strip shows (UI§5.3). Pure functions of the session list, so the rules
//! are tested once and the panel, the notch and the sealed demo agree.
//!
//! Owner: WP7. Ports `SessionSections.swift` and the layout half of
//! `SessionList.swift` (`SessionListLayout`).

use crate::model::ui::SectionInfo;
use crate::model::{Bucket, SessionState, SessionView};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::SystemTime;

/// Idle sessions beyond this many fold into a one-line summary.
pub const IDLE_COLLAPSE_THRESHOLD: usize = 3;
/// Above this many rows drawn (folded sections don't count), rows that
/// don't need an answer go to one line, so 25 sessions fit without a scroll
/// hunt.
pub const COMPACT_THRESHOLD: usize = 8;

// ---- counts ----

/// How many sessions sit in each bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttentionCounts {
    /// Every session in "Needs you", failed turns included.
    pub needs_input: u32,
    pub ready_for_review: u32,
    pub working: u32,
    pub idle: u32,
    /// The failed turns among `needs_input`: they wait on the user (retry,
    /// switch account, sign in) but there is nothing to answer, so the strip
    /// counts them apart.
    pub failed: u32,
}

impl AttentionCounts {
    pub fn of<'a>(sessions: impl IntoIterator<Item = &'a SessionView>) -> AttentionCounts {
        let mut counts = AttentionCounts::default();
        for session in sessions {
            match session.state {
                SessionState::NeedsYou(_) => counts.needs_input += 1,
                SessionState::Failed(_) => {
                    counts.needs_input += 1;
                    counts.failed += 1;
                }
                SessionState::ReadyForReview => counts.ready_for_review += 1,
                SessionState::Working => counts.working += 1,
                SessionState::Idle => counts.idle += 1,
            }
        }
        counts
    }

    /// Sessions waiting on an answer (needs you, not failed).
    pub fn answerable(&self) -> u32 {
        self.needs_input - self.failed
    }

    pub fn total(&self) -> u32 {
        self.needs_input + self.ready_for_review + self.working + self.idle
    }
}

// ---- sections ----

/// One bucket's sessions, in display order.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSection<'a> {
    pub bucket: Bucket,
    pub sessions: Vec<&'a SessionView>,
}

/// Whether a section starts folded: only a long idle list does; the user
/// folds the others with their headers.
pub fn is_collapsed_by_default(bucket: Bucket, count: usize) -> bool {
    bucket == Bucket::Idle && count > IDLE_COLLAPSE_THRESHOLD
}

/// Whether `bucket` is folded, given the user's choices (bucket → folded).
pub fn is_collapsed(bucket: Bucket, count: usize, overrides: &HashMap<Bucket, bool>) -> bool {
    // Needs-you never folds: it is what the panel is for.
    if bucket == Bucket::NeedsYou {
        return false;
    }
    overrides
        .get(&bucket)
        .copied()
        .unwrap_or_else(|| is_collapsed_by_default(bucket, count))
}

/// Whether rows go to one line when `session_count` rows are drawn.
pub fn is_compact(session_count: usize) -> bool {
    session_count > COMPACT_THRESHOLD
}

/// The one line a folded section shows: its first titles, then how many
/// more. "Write migration tests, Investigate the flaky CI job and 1 more".
pub fn collapsed_summary(section: &SessionSection<'_>, max_titles: usize) -> String {
    let titles: Vec<&str> = section
        .sessions
        .iter()
        .take(max_titles)
        .map(|session| session.title.as_str())
        .collect();
    let rest = section.sessions.len() - titles.len();
    let list = titles.join(", ");
    if rest > 0 {
        format!("{list} and {rest} more")
    } else {
        list
    }
}

/// How many titles a folded section's line names.
pub const SUMMARY_TITLES: usize = 2;

/// When the session started waiting on the user: the pending approval's
/// arrival, else the last hook event (the one that blocked it).
pub fn waiting_since(view: &SessionView) -> SystemTime {
    super::rows::waiting_since(view)
}

/// Non-empty sections in bucket order (needs you, review, working, idle).
///
/// Within a section: needs-you answerable requests before failed turns,
/// each oldest waiting first; review newest completion first; working
/// longest-running first; idle most recent first. Ties by session id so equal
/// keys never swap between renders.
///
/// `previous_order`: the session ids as currently displayed. When given (the
/// pointer is over the list), sessions keep that relative order inside their
/// section and newcomers go to the end of theirs, so a row never moves under
/// the pointer. Sessions still change section as their state changes.
pub fn build<'a>(
    sessions: &'a [SessionView],
    previous_order: Option<&[String]>,
) -> Vec<SessionSection<'a>> {
    let previous_index: Option<HashMap<&str, usize>> = previous_order.map(|order| {
        let mut index = HashMap::new();
        for (position, id) in order.iter().enumerate() {
            index.entry(id.as_str()).or_insert(position);
        }
        index
    });

    Bucket::ALL
        .into_iter()
        .filter_map(|bucket| {
            let mut members: Vec<&SessionView> = sessions
                .iter()
                .filter(|session| session.state.bucket() == bucket)
                .collect();
            if members.is_empty() {
                return None;
            }
            members.sort_by(|a, b| compare(a, b, bucket));
            if let Some(previous) = &previous_index {
                members = preserve(members, previous);
            }
            Some(SessionSection {
                bucket,
                sessions: members,
            })
        })
        .collect()
}

/// The flat display order of a section list.
pub fn order_of(sections: &[SessionSection<'_>]) -> Vec<String> {
    sections
        .iter()
        .flat_map(|section| section.sessions.iter().map(|s| s.id.as_str().to_owned()))
        .collect()
}

/// The header of each section (UI§5.3).
pub fn infos(sections: &[SessionSection<'_>]) -> Vec<SectionInfo> {
    sections
        .iter()
        .map(|section| SectionInfo {
            bucket: section.bucket.as_str().to_owned(),
            title: section.bucket.title().to_owned(),
            count: section.sessions.len() as u32,
            fold_by_default: is_collapsed_by_default(section.bucket, section.sessions.len()),
        })
        .collect()
}

// ---- ordering ----

/// A prompt you can answer outranks a failed turn: nothing gets pushed below
/// the fold by sessions that only need a retry.
fn sort_rank(state: &SessionState) -> u8 {
    u8::from(matches!(state, SessionState::Failed(_)))
}

/// The order of two sessions of one bucket (a total order: equal keys fall
/// to the session id).
pub fn compare(a: &SessionView, b: &SessionView, bucket: Bucket) -> Ordering {
    let by_key = match bucket {
        Bucket::NeedsYou => sort_rank(&a.state)
            .cmp(&sort_rank(&b.state))
            .then_with(|| waiting_since(a).cmp(&waiting_since(b))),
        Bucket::ReadyForReview => newest_first(a.completed_at, b.completed_at),
        Bucket::Working => oldest_first(a.turn_started_at, b.turn_started_at),
        Bucket::Idle => b.last_activity.cmp(&a.last_activity),
    };
    by_key.then_with(|| a.id.cmp(&b.id))
}

/// Newest first; a missing date sorts last.
fn newest_first(a: Option<SystemTime>, b: Option<SystemTime>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Oldest first; a missing date sorts last.
fn oldest_first(a: Option<SystemTime>, b: Option<SystemTime>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Known sessions in their previous relative order, then the newcomers in
/// their natural order.
fn preserve<'a>(
    ordered: Vec<&'a SessionView>,
    previous: &HashMap<&str, usize>,
) -> Vec<&'a SessionView> {
    let (mut known, newcomers): (Vec<&SessionView>, Vec<&SessionView>) = ordered
        .into_iter()
        .partition(|session| previous.contains_key(session.id.as_str()));
    known.sort_by_key(|session| previous[session.id.as_str()]);
    known.extend(newcomers);
    known
}

// ---- the list as values (SessionListLayout) ----

/// One section as laid out: which rows, and whether it shows folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaidOutSection {
    pub bucket: Bucket,
    pub ids: Vec<String>,
    pub is_collapsed: bool,
    /// "Write migration tests, Investigate the flaky CI job and 1 more".
    pub summary: String,
}

/// The list as values: sections of session ids, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLayout {
    pub sections: Vec<LaidOutSection>,
    /// Rows other than answers go to one line (more than
    /// [`COMPACT_THRESHOLD`] rows drawn).
    pub is_compact: bool,
}

impl ListLayout {
    pub fn make(sections: &[SessionSection<'_>], folds: &HashMap<Bucket, bool>) -> ListLayout {
        let laid_out: Vec<LaidOutSection> = sections
            .iter()
            .map(|section| LaidOutSection {
                bucket: section.bucket,
                ids: section
                    .sessions
                    .iter()
                    .map(|session| session.id.as_str().to_owned())
                    .collect(),
                is_collapsed: is_collapsed(section.bucket, section.sessions.len(), folds),
                summary: collapsed_summary(section, SUMMARY_TITLES),
            })
            .collect();
        // Rows drawn, not sessions: a long idle list folded to one line
        // doesn't squeeze the few rows above it.
        let drawn: usize = laid_out
            .iter()
            .filter(|section| !section.is_collapsed)
            .map(|section| section.ids.len())
            .sum();
        ListLayout {
            sections: laid_out,
            is_compact: is_compact(drawn),
        }
    }

    /// Row ids as displayed (folded sections contribute none): the order the
    /// keyboard moves through.
    pub fn visible_order(&self) -> Vec<&str> {
        self.sections
            .iter()
            .filter(|section| !section.is_collapsed)
            .flat_map(|section| section.ids.iter().map(String::as_str))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The folded section `id`'s row is in, if it is folded away.
    pub fn folded_bucket_containing(&self, id: Option<&str>) -> Option<Bucket> {
        let id = id?;
        self.sections
            .iter()
            .find(|section| section.is_collapsed && section.ids.iter().any(|row| row == id))
            .map(|section| section.bucket)
    }

    /// The bucket a session's row is in.
    pub fn bucket_of(&self, id: &str) -> Option<Bucket> {
        self.sections
            .iter()
            .find(|section| section.ids.iter().any(|row| row == id))
            .map(|section| section.bucket)
    }
}
