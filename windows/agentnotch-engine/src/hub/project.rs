//! The hub's snapshot projection: pure functions from what the stores hold
//! (accounts, ring readings, sessions, settings, setup state, "now") to the
//! [`HubSnapshot`] the pages draw, and to the usage upstream's code reads.
//! Ports of `ClaudeControlHub.recompute` and its pure helpers (ring ids,
//! counts, "just finished", the next boundary), `ClaudeHostProjections.
//! ringReading`, `ClaudeAttentionPolicy.notchValues`/`restingMarks`, and the
//! Mac bridge's `ClaudeUsageProvider.snapshot` (design §3.6, §4.6; AU§7, AU§8.4).
//!
//! Nothing here reads a file, a clock or a store: the runtime hands over what
//! it holds in a [`ProjectionInput`] of borrowed values (an-core calls this
//! after every change, so it stays cheap) and the tests build the same input
//! from the packages' own stores.
//!
//! Where the Mac and the WP0 fixtures differ the Mac decides: a failed turn is
//! never amber, so it is no part of a ring's "needs you" badge, of the folded
//! notch's bar, or of the tray's count (`Counts::needs_you` is what can be
//! answered). The sealed demo (wp7-5) reconciles the fixture with that.
//!
//! Owner: WP7.

use crate::accounts::registry::AccountRegistry;
use crate::attention::policy;
use crate::attention::rows::{session_row, RateLimitReset, ResetClock, RowContext};
use crate::attention::sections::{self, AttentionCounts};
use crate::core::time::{from_ms, to_ms};
use crate::model::*;
use crate::runtime_types::RingReading;
use crate::usage::ring_windows::{self, RingWindow};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

/// How long a ring's newest completion is "just finished": its green arc
/// pulses this long, then rests (`ClaudeControlHub.freshSuccessWindow`).
pub const FRESH_SUCCESS_WINDOW: Duration = Duration::from_secs(90);

/// The ring id of the default account before any account is known
/// (`ClaudeRingIdentity.defaultRingID`).
pub const FALLBACK_RING_ID: &str = crate::accounts::identities::DEFAULT_RING_ID;

/// Upstream's note for a ring that is signed out (never `needsAuth`: that
/// would offer upstream's token sign-in, which this app never has).
pub const SIGN_IN_NOTE: &str = "Not signed in to Claude. Run claude, then /login.";
/// Upstream's note while no reading has arrived.
pub const WAITING_NOTE: &str = "Waiting for the first reading…";
/// The ring's own note when its login is gone.
const NOT_SIGNED_IN: &str = "Not signed in to Claude";

// ---- input ----

/// What the projection asks the account registry about a session's folder.
/// [`AccountRegistry`] implements it; tests may use a small fake.
pub trait Directory {
    /// `~\.claude`: where a session that names no folder runs.
    fn default_folder(&self) -> AccountId;
    /// The identity a folder belongs to now.
    fn identity_of_folder(&self, folder: &AccountId) -> Option<IdentityId>;
    /// An account (an identity or folder id) the user forgot: its sessions are
    /// listed nowhere, rather than landing on the default ring (BHV-3).
    fn is_forgotten(&self, id: &str) -> bool;
    /// The folder, for a process that started at `started`, belongs to an
    /// untracked or forgotten account.
    fn is_untracked(&self, folder: &AccountId, started: Option<SystemTime>) -> bool;
    /// The ring a folder has, when the registry knows it.
    fn ring_of_folder(&self, folder: &AccountId) -> Option<RingId>;
}

impl Directory for AccountRegistry {
    fn default_folder(&self) -> AccountId {
        AccountId::new(self.paths().default_config_dir())
    }

    fn identity_of_folder(&self, folder: &AccountId) -> Option<IdentityId> {
        self.identity_id_for(folder.as_str())
    }

    fn is_forgotten(&self, id: &str) -> bool {
        AccountRegistry::is_forgotten(self, id)
    }

    fn is_untracked(&self, folder: &AccountId, started: Option<SystemTime>) -> bool {
        AccountRegistry::is_untracked(self, folder, started)
    }

    fn ring_of_folder(&self, folder: &AccountId) -> Option<RingId> {
        AccountRegistry::ring_of_folder(self, folder)
    }
}

/// What a row needs of its session besides the session itself: the hub's
/// caches (host app, windows to jump to, typing availability).
#[derive(Debug, Clone, Default)]
pub struct RowExtras {
    pub host_app: Option<String>,
    pub can_focus: bool,
    pub can_message: bool,
}

/// Everything one projection reads, borrowed.
pub struct ProjectionInput<'a> {
    pub now: SystemTime,
    /// Every identity of the registry, tracked or not, in the registry's order.
    pub accounts: &'a [Account],
    pub directory: &'a dyn Directory,
    /// Each identity's ring reading (`UsageStore::ring_reading`); one missing
    /// reads as "waiting for the first reading".
    pub readings: &'a BTreeMap<IdentityId, RingReading>,
    /// Every session the store holds, with the attribution the hub gave it.
    pub sessions: &'a [SessionView],
    pub extras: &'a dyn Fn(&SessionView) -> RowExtras,
    pub clock: ResetClock,
    pub ui: &'a UiSettings,
    pub setup: &'a SetupState,
    pub sealed: bool,
}

/// `generated_at_ms`, never decreasing: the panel drops a snapshot older than
/// the last it drew, so a clock that steps back must not date a newer
/// snapshot earlier. Strictly increasing, so two snapshots never tie.
#[derive(Debug, Clone, Copy, Default)]
pub struct SnapshotClock {
    last: u64,
}

impl SnapshotClock {
    pub fn next(&mut self, now_ms: u64) -> u64 {
        self.last = now_ms.max(self.last.saturating_add(1));
        self.last
    }

    /// The last value handed out (0 before the first).
    pub fn last(&self) -> u64 {
        self.last
    }
}

// ---- ring ids ----

/// A session's ring: its account's when that account has one, else the
/// default ring (a folder seen in a hook before the registry knows it).
pub fn ring_id(ring: &str, known: &BTreeSet<String>, default_ring: &str) -> String {
    if known.contains(ring) {
        ring.to_owned()
    } else {
        default_ring.to_owned()
    }
}

/// The ring sessions of unknown folders go to: the account `~\.claude` runs
/// as, else the first tracked one, else the fallback id.
pub fn default_ring_id(accounts: &[Account]) -> String {
    accounts
        .iter()
        .find(|a| a.includes_default && a.is_tracked)
        .or_else(|| accounts.iter().find(|a| a.is_tracked))
        .map_or_else(
            || FALLBACK_RING_ID.to_owned(),
            |a| a.ring_id.as_str().to_owned(),
        )
}

/// The identities whose ring isn't shown (or that aren't tracked): nothing
/// is probed for them, since there is no ring to draw the answer on. Pure.
pub fn paused_account_ids(accounts: &[Account]) -> BTreeSet<IdentityId> {
    accounts
        .iter()
        .filter(|a| !(a.is_tracked && a.ring_shown))
        .map(|a| a.identity_id.clone())
        .collect()
}

// ---- readings to rings ----

fn clamp_fraction(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// A ring window as upstream's `LimitWindow`: used 0..=1, reset in epoch ms,
/// labelled by its own name else by upstream's wording for the id.
fn upstream_window(window: &RingWindow) -> UpstreamWindow {
    UpstreamWindow {
        id: window.id.clone(),
        label: window
            .label
            .clone()
            .unwrap_or_else(|| ring_windows::label_for_id(&window.id)),
        used: clamp_fraction(window.used_fraction),
        resets_at: window.resets_at.map(to_ms),
        count: None,
        derived: false,
        group: None,
    }
}

/// A ring's usage in the pages' shape, and the windows it came from (the
/// rows' rate-limit phrase reads those). `stale` is the engine's rule, which
/// the store applied when it made the reading (AU§8.4: 1 h with probes off,
/// else max(15 min, 1.5 x interval), never for an exhausted window).
fn ring_usage(reading: Option<&RingReading>, now: SystemTime) -> (RingUsage, Vec<RingWindow>) {
    let plain = |status: &str, note: &str| RingUsage {
        status: status.to_owned(),
        windows: Vec::new(),
        fetched_at_ms: 0,
        note: note.to_owned(),
        stale: false,
    };
    match reading {
        Some(RingReading::Reading { usage, status, .. }) => {
            let windows = ring_windows::windows(usage, now);
            let shown = RingUsage {
                status: status.as_str().to_owned(),
                windows: windows.iter().map(upstream_window).collect(),
                fetched_at_ms: to_ms(usage.updated_at),
                note: String::new(),
                stale: *status == RingStatus::Stale,
            };
            (shown, windows)
        }
        Some(RingReading::SignInNeeded) => (plain("sign_in_needed", NOT_SIGNED_IN), Vec::new()),
        Some(RingReading::Unavailable(text)) => (plain("unavailable", text), Vec::new()),
        Some(RingReading::Failed(text)) => (plain("failed", text), Vec::new()),
        Some(RingReading::Waiting) | None => (plain("waiting", ""), Vec::new()),
    }
}

// ---- counts ----

fn counts_of(counts: AttentionCounts) -> Counts {
    Counts {
        needs_you: counts.answerable(),
        failed: counts.failed,
        review: counts.ready_for_review,
        working: counts.working,
        idle: counts.idle,
    }
}

/// "2 need you, 1 to review, 3 working, 1 failed"; empty when all zero.
pub fn counts_text(counts: &Counts) -> String {
    let mut parts = Vec::new();
    if counts.needs_you > 0 {
        let verb = if counts.needs_you == 1 {
            "needs"
        } else {
            "need"
        };
        parts.push(format!("{} {verb} you", counts.needs_you));
    }
    if counts.review > 0 {
        parts.push(format!("{} to review", counts.review));
    }
    if counts.working > 0 {
        parts.push(format!("{} working", counts.working));
    }
    if counts.failed > 0 {
        parts.push(format!("{} failed", counts.failed));
    }
    parts.join(", ")
}

fn percent(fraction: f64) -> u32 {
    (fraction * 100.0).round() as u32
}

/// The ring's accessible label: "Personal: 5-hour 34%, weekly 41%; 3 need
/// you, 2 to review".
fn ring_a11y(label: &str, usage: &RingUsage, counts: &Counts) -> String {
    let usage_text = match usage.status.as_str() {
        "ok" | "stale" => {
            let mut parts: Vec<String> = Vec::new();
            for window in &usage.windows {
                match window.id.as_str() {
                    "session" => parts.push(format!("5-hour {}%", percent(window.used))),
                    "weekly_all" => parts.push(format!("weekly {}%", percent(window.used))),
                    _ => {}
                }
            }
            let mut text = parts.join(", ");
            if usage.stale && !text.is_empty() {
                text.push_str(" (stale)");
            }
            text
        }
        "waiting" => "waiting for the first reading".to_owned(),
        "sign_in_needed" => "not signed in".to_owned(),
        "unavailable" => "usage unavailable".to_owned(),
        "failed" => "usage check failed".to_owned(),
        _ => String::new(),
    };
    let sessions = counts_text(counts);
    let body: Vec<String> = [usage_text, sessions]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if body.is_empty() {
        label.to_owned()
    } else {
        format!("{label}: {}", body.join("; "))
    }
}

// ---- placing sessions ----

/// A session on a ring, with the identity it runs as.
struct Placed {
    view: SessionView,
    identity: Option<IdentityId>,
}

/// Which ring each session sits on, and which sessions are listed at all.
///
/// A session's folder names its account; a folder not known yet goes to the
/// default ring. Sessions of an account that is forgotten, switched off or
/// no longer tracked are listed nowhere (they must not land on the default
/// ring). A session whose account can't be told for certain is placed by its
/// best guess, else by its folder.
fn place(input: &ProjectionInput<'_>, known: &BTreeSet<String>, default_ring: &str) -> Vec<Placed> {
    let mut placed = Vec::new();
    for view in input.sessions {
        let folder = view
            .account
            .clone()
            .unwrap_or_else(|| input.directory.default_folder());
        let identity = match &view.attribution {
            Attribution::Known(Some(id)) => {
                if input.directory.is_forgotten(id.as_str()) {
                    continue;
                }
                Some(id.clone())
            }
            other => {
                if input.directory.is_untracked(&folder, view.pid_started)
                    || input.directory.is_forgotten(folder.as_str())
                {
                    continue;
                }
                let guess = match other {
                    Attribution::Unsure(guess) => guess.clone(),
                    _ => None,
                };
                guess.or_else(|| input.directory.identity_of_folder(&folder))
            }
        };
        let account = identity
            .as_ref()
            .and_then(|id| input.accounts.iter().find(|a| &a.identity_id == id));
        if account.is_some_and(|a| !a.is_tracked) {
            continue;
        }
        let ring = account
            .map(|a| a.ring_id.clone())
            .or_else(|| input.directory.ring_of_folder(&folder));
        let ring = ring_id(
            ring.as_ref().map_or("", RingId::as_str),
            known,
            default_ring,
        );
        let mut view = view.clone();
        view.ring = Some(RingId::new(ring));
        placed.push(Placed { view, identity });
    }
    placed
}

fn ring_of(placed: &Placed) -> &str {
    placed.view.ring.as_ref().map_or("", RingId::as_str)
}

// ---- the snapshot ----

/// The whole snapshot at `generated_at_ms` (see [`SnapshotClock`]).
pub fn project(input: &ProjectionInput<'_>, generated_at_ms: u64) -> HubSnapshot {
    let default_ring = default_ring_id(input.accounts);
    let known: BTreeSet<String> = input
        .accounts
        .iter()
        .map(|a| a.ring_id.as_str().to_owned())
        .collect();
    let placed = place(input, &known, &default_ring);
    let views: Vec<SessionView> = placed.iter().map(|p| p.view.clone()).collect();

    // Rows, in the list's order: sections, then the order inside each.
    let tracked_count = input.accounts.iter().filter(|a| a.is_tracked).count();
    let accounts_multi = tracked_count > 1;
    let ordered = sections::build(&views, None);
    let infos = sections::infos(&ordered);
    let by_id: BTreeMap<&str, &Placed> = placed.iter().map(|p| (p.view.id.as_str(), p)).collect();
    let windows_of: BTreeMap<&IdentityId, Vec<RingWindow>> = input
        .accounts
        .iter()
        .filter(|a| a.is_tracked)
        .map(|a| {
            let (_, windows) = ring_usage(input.readings.get(&a.identity_id), input.now);
            (&a.identity_id, windows)
        })
        .collect();
    let mut rows = Vec::with_capacity(placed.len());
    for section in &ordered {
        for view in &section.sessions {
            let Some(placed) = by_id.get(view.id.as_str()) else {
                continue;
            };
            let account = input
                .accounts
                .iter()
                .find(|a| a.ring_id.as_str() == ring_of(placed));
            let extras = (input.extras)(view);
            let rate_limit = placed
                .identity
                .as_ref()
                .and_then(|id| windows_of.get(id))
                .and_then(|windows| RateLimitReset::current(windows, input.now));
            let mut ctx = RowContext::new(input.now);
            ctx.account_label = account.filter(|_| accounts_multi).map(|a| a.label.as_str());
            ctx.account_color = account.filter(|_| accounts_multi).map(|a| a.color_index);
            ctx.rate_limit = rate_limit.as_ref();
            ctx.host_app = extras.host_app.as_deref();
            ctx.can_focus = extras.can_focus;
            ctx.can_message = extras.can_message;
            ctx.clock = input.clock;
            rows.push(session_row(view, &ctx));
        }
    }

    let totals = counts_of(AttentionCounts::of(&views));
    let tracked_rings: Vec<String> = input
        .accounts
        .iter()
        .filter(|a| a.is_tracked)
        .map(|a| a.ring_id.as_str().to_owned())
        .collect();
    let counts = ring_counts(&views, &tracked_rings);
    let newest_review = newest_reviews(&views);
    let rings = build_rings(input, &counts, &newest_review, &default_ring);

    // The notch only counts what it shows (see `policy::notch_values`).
    let shown: BTreeSet<String> = rings
        .iter()
        .filter(|r| r.shown)
        .map(|r| r.ring_id.clone())
        .collect();
    let notch = policy::notch_values(
        &counts,
        &tracked_rings,
        Some(&shown),
        &default_ring,
        policy::combined,
    );
    let notch_total = policy::total(notch.values());
    let marks = policy::resting_marks(&notch_total);

    HubSnapshot {
        version: HubSnapshot::VERSION,
        generated_at_ms,
        sealed: input.sealed,
        rings,
        sessions: rows,
        sections: infos,
        totals,
        resting_marks: RestingMarks {
            needs_you: marks.contains(&policy::RestingMark::NeedsYou),
            review: marks.contains(&policy::RestingMark::Review),
            working: marks.contains(&policy::RestingMark::Working),
            // The bar breathes again whenever the count changes.
            needs_you_key: notch_total.needs_you,
        },
        tray_badge: policy::tray_badge(totals.needs_you, input.ui.tray_badge),
        setup: input.setup.clone(),
        ui: input.ui.clone(),
        accounts_multi,
    }
}

/// The rings with no sessions: what `Hub::launch_rings` returns before the
/// first session is seen, and what the Settings pane draws from.
pub fn launch_rings(
    accounts: &[Account],
    readings: &BTreeMap<IdentityId, RingReading>,
    now: SystemTime,
) -> Vec<RingSummary> {
    let nobody = Placeholder;
    let none: &[SessionView] = &[];
    let extras = |_: &SessionView| RowExtras::default();
    let ui = default_ui();
    let setup = empty_setup();
    let input = ProjectionInput {
        now,
        accounts,
        directory: &nobody,
        readings,
        sessions: none,
        extras: &extras,
        clock: ResetClock::default(),
        ui: &ui,
        setup: &setup,
        sealed: false,
    };
    let tracked: Vec<String> = accounts
        .iter()
        .filter(|a| a.is_tracked)
        .map(|a| a.ring_id.as_str().to_owned())
        .collect();
    build_rings(
        &input,
        &ring_counts(&[], &tracked),
        &BTreeMap::new(),
        &default_ring_id(accounts),
    )
}

/// A directory that knows nothing, for projections with no sessions.
struct Placeholder;

impl Directory for Placeholder {
    fn default_folder(&self) -> AccountId {
        AccountId::default()
    }
    fn identity_of_folder(&self, _: &AccountId) -> Option<IdentityId> {
        None
    }
    fn is_forgotten(&self, _: &str) -> bool {
        false
    }
    fn is_untracked(&self, _: &AccountId, _: Option<SystemTime>) -> bool {
        false
    }
    fn ring_of_folder(&self, _: &AccountId) -> Option<RingId> {
        None
    }
}

fn default_ui() -> UiSettings {
    crate::core::settings::ControlSettings::default().ui()
}

fn empty_setup() -> SetupState {
    SetupState {
        hook_consent: None,
        needs_hook_consent: false,
        consent_files: Vec::new(),
        codenotch_hooks_folders: Vec::new(),
        new_install_folders: Vec::new(),
        transport_error: None,
        control_off: false,
        missing_hooks_accounts: Vec::new(),
        install_disabled: false,
    }
}

/// Counts per ring, with every ring in `rings` present at zero so its badges
/// clear when its last session goes.
pub fn ring_counts(views: &[SessionView], rings: &[String]) -> BTreeMap<String, Counts> {
    let mut grouped: BTreeMap<String, Vec<&SessionView>> = BTreeMap::new();
    for view in views {
        grouped
            .entry(
                view.ring
                    .as_ref()
                    .map_or_else(String::new, |r| r.as_str().to_owned()),
            )
            .or_default()
            .push(view);
    }
    let mut counts: BTreeMap<String, Counts> = grouped
        .into_iter()
        .map(|(ring, sessions)| (ring, counts_of(AttentionCounts::of(sessions))))
        .collect();
    for ring in rings {
        counts.entry(ring.clone()).or_default();
    }
    counts
}

/// Per ring, when its newest completion that still waits for review
/// finished (the "just finished" window runs from it).
fn newest_reviews(views: &[SessionView]) -> BTreeMap<String, SystemTime> {
    let mut newest: BTreeMap<String, SystemTime> = BTreeMap::new();
    for view in views {
        if view.state != SessionState::ReadyForReview {
            continue;
        }
        let since = crate::attention::rows::since(view);
        let ring = view
            .ring
            .as_ref()
            .map_or_else(String::new, |r| r.as_str().to_owned());
        let slot = newest.entry(ring).or_insert(since);
        if since > *slot {
            *slot = since;
        }
    }
    newest
}

/// Per ring, until when its newest completion is "just finished"; rings
/// whose window has passed are absent. Pure.
pub fn fresh_success_until(views: &[SessionView], now: SystemTime) -> BTreeMap<String, SystemTime> {
    newest_reviews(views)
        .into_iter()
        .map(|(ring, since)| (ring, since + FRESH_SUCCESS_WINDOW))
        .filter(|(_, until)| *until > now)
        .collect()
}

fn build_rings(
    input: &ProjectionInput<'_>,
    counts: &BTreeMap<String, Counts>,
    newest_review: &BTreeMap<String, SystemTime>,
    default_ring: &str,
) -> Vec<RingSummary> {
    let tracked_rings: Vec<String> = input
        .accounts
        .iter()
        .filter(|a| a.is_tracked)
        .map(|a| a.ring_id.as_str().to_owned())
        .collect();
    let shown: BTreeSet<String> = input
        .accounts
        .iter()
        .filter(|a| a.is_tracked && a.ring_shown)
        .map(|a| a.ring_id.as_str().to_owned())
        .collect();
    let notch = policy::notch_values(
        counts,
        &tracked_rings,
        Some(&shown),
        default_ring,
        policy::combined,
    );
    input
        .accounts
        .iter()
        .filter(|a| a.is_tracked)
        .map(|account| {
            let ring = account.ring_id.as_str();
            let (usage, _) = ring_usage(input.readings.get(&account.identity_id), input.now);
            let own = counts.get(ring).copied().unwrap_or_default();
            // What the notch shows: nothing for a ring switched off.
            let seen = notch.get(ring).copied().unwrap_or_default();
            let activity = if seen.needs_you > 0 {
                RingActivity::Waiting
            } else if seen.working > 0 {
                RingActivity::Working
            } else if seen.review > 0 {
                RingActivity::Success
            } else {
                RingActivity::Idle
            };
            let settles = (activity == RingActivity::Success)
                .then(|| newest_review.get(ring))
                .flatten()
                .map(|since| to_ms(*since + FRESH_SUCCESS_WINDOW));
            let a11y = ring_a11y(&account.label, &usage, &own);
            RingSummary {
                ring_id: ring.to_owned(),
                label: account.label.clone(),
                monogram: account.monogram.clone(),
                color_index: account.color_index,
                shown: account.ring_shown,
                is_default: account.includes_default,
                usage,
                activity,
                success_settles_at_ms: settles,
                badges: Badges {
                    needs_you: seen.needs_you,
                    review: seen.review,
                },
                counts: own,
                a11y,
            }
        })
        .collect()
}

// ---- when the projection changes by itself ----

/// The next moment the snapshot changes with no new input: a ring's "just
/// finished" window ends (the arc settles), or a usage window resets (it
/// reads 0% again, a used-up limit lifts, rate-limited rows lose their
/// "resets 14:05"). `None` when neither is ahead. Pure.
pub fn next_boundary(snapshot: &HubSnapshot, now: SystemTime) -> Option<SystemTime> {
    let resets = snapshot
        .rings
        .iter()
        .flat_map(|ring| ring.usage.windows.iter())
        .filter_map(|window| window.resets_at);
    let settles = snapshot
        .rings
        .iter()
        .filter(|ring| ring.activity == RingActivity::Success)
        .filter_map(|ring| ring.success_settles_at_ms);
    resets
        .chain(settles)
        .map(from_ms)
        .filter(|at| *at > now)
        .min()
}

// ---- upstream's usage ----

/// `AppState.usage`'s shape (design §4.6): every shown ring's windows. The
/// default ring (the one holding `~\.claude`, else the first shown) keeps
/// bare window ids, the others get `<id>@<ring_id>`; `group` is the ring's
/// label when more than one ring is shown. The status is the default ring's:
/// ok and stale are upstream's own; waiting, signed out and unavailable are
/// `none` with a note; failed is `error`. Never `needsAuth`.
pub fn upstream_usage(rings: &[RingSummary]) -> UpstreamUsage {
    let shown: Vec<&RingSummary> = rings.iter().filter(|r| r.shown).collect();
    let Some(default) = shown
        .iter()
        .find(|r| r.is_default)
        .or(shown.first())
        .copied()
    else {
        return UpstreamUsage {
            status: "none".into(),
            note: "No Claude account yet.".into(),
            ..UpstreamUsage::default()
        };
    };
    let several = shown.len() > 1;
    let mut windows = Vec::new();
    for ring in &shown {
        for window in &ring.usage.windows {
            let mut window = window.clone();
            if ring.ring_id != default.ring_id {
                window.id = format!("{}@{}", window.id, ring.ring_id);
            }
            window.group = several.then(|| ring.label.clone());
            windows.push(window);
        }
    }
    let (status, note) = match default.usage.status.as_str() {
        "ok" => ("ok", default.usage.note.clone()),
        "stale" => ("stale", default.usage.note.clone()),
        "waiting" => ("none", WAITING_NOTE.to_owned()),
        "sign_in_needed" => ("none", SIGN_IN_NOTE.to_owned()),
        "failed" => ("error", default.usage.note.clone()),
        _ => ("none", default.usage.note.clone()),
    };
    UpstreamUsage {
        status: status.into(),
        windows,
        fetched_at: shown
            .iter()
            .map(|r| r.usage.fetched_at_ms)
            .max()
            .unwrap_or(0),
        note,
        backoff_until: 0,
    }
}
