//! SessionRowContent and the hover card's row: what each session row says
//! (its state word, second line, elapsed anchor, VoiceOver sentence, the
//! request its action bar answers) and the card row of the notch's hover
//! card (UI§3.5, UI§5.4).
//!
//! Pure functions of one `SessionView` and a [`RowContext`] (the clock, the
//! account's name and colour, the exhausted limit, the host app): nothing
//! here reads the platform. Elapsed labels are drawn by the pages from
//! `since_ms`; the engine only spells them for the VoiceOver sentence.
//!
//! Owner: WP7. Ports `SessionRowContent.swift` and `ClaudeHostProjections`
//! (`activityRow`, `attention`, `tasks`, `runningTool`). The card is the
//! Windows `CardRow` of the UI contract: its name carries the place ("Windows
//! Terminal · acme-web"), its detail the progress, and its waiting line only
//! what a stranger may read over another window, never a request's input.

use crate::control::text::{collapse_whitespace, collapsed, format_tool_name};
use crate::core::time::{from_ms, to_ms};
use crate::model::ui::{CardRow, PendingRequestView, RowDetail, SessionRow, UpstreamWindow};
use crate::model::{
    Bucket, NeedsInputReason, PendingRequest, PermissionContext, Phase, RequestKind, SessionState,
    SessionView, TaskProgress,
};
use crate::sessions::attention::humanized_stop_error;
use crate::sessions::session::parse_questions;
use crate::usage::ring_windows::{RingWindow, EXTRA_USAGE_ID};
use chrono::{DateTime, Datelike, FixedOffset, Utc};
use std::time::{Duration, SystemTime};

/// What a row says for a prompt only the terminal can answer.
pub const TERMINAL_WAIT: &str = "waiting in the terminal";

/// A question answers with one tap when it has at most this many options.
pub const MAX_INLINE_OPTIONS: usize = 4;

/// The longest duration spelled out; longer ones are held here (10 000 days)
/// so no conversion can overflow on bad data.
const MAX_DURATION: Duration = Duration::from_secs(10_000 * 86_400);

/// A reset within this reads as a countdown, later ones as a weekday.
const COUNTDOWN_HORIZON: Duration = Duration::from_secs(24 * 60 * 60);

/// Task names on a card are cut to this many characters.
const CARD_TASK_LIMIT: usize = 48;
/// A question's header on a card ("Charts") is cut to this many characters.
const CARD_HEADER_LIMIT: usize = 24;
/// A reason on a card is cut to this many characters.
const CARD_REASON_LIMIT: usize = 60;

// ---- context ----

/// The user's clock for a reset time: seconds east of UTC and 12 or 24 hour
/// style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetClock {
    pub utc_offset_seconds: i32,
    pub hour12: bool,
}

impl Default for ResetClock {
    /// UTC, 12-hour: what the sealed fixtures show.
    fn default() -> ResetClock {
        ResetClock {
            utc_offset_seconds: 0,
            hour12: true,
        }
    }
}

/// What a row needs besides the session itself.
#[derive(Debug, Clone)]
pub struct RowContext<'a> {
    pub now: SystemTime,
    /// The account's name and colour (`None` with one account).
    pub account_label: Option<&'a str>,
    pub account_color: Option<u8>,
    /// The exhausted limit of the session's account, if any.
    pub rate_limit: Option<&'a RateLimitReset>,
    /// Where it runs, as rows name it ("Windows Terminal", "VS Code").
    pub host_app: Option<&'a str>,
    /// There is a window to jump to.
    pub can_focus: bool,
    /// A reply can be typed into it.
    pub can_message: bool,
    pub clock: ResetClock,
}

impl<'a> RowContext<'a> {
    pub fn new(now: SystemTime) -> RowContext<'a> {
        RowContext {
            now,
            account_label: None,
            account_color: None,
            rate_limit: None,
            host_app: None,
            can_focus: false,
            can_message: false,
            clock: ResetClock::default(),
        }
    }
}

// ---- rate limits ----

/// The limit that stopped a rate-limited session, and when it lifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitReset {
    /// "5-hour limit", "weekly limit", "Opus weekly limit".
    pub window: String,
    pub resets_at: SystemTime,
}

impl RateLimitReset {
    /// "weekly limit resets Thu 9:00 AM", "5-hour limit resets in 47m".
    pub fn phrase(&self, now: SystemTime, clock: ResetClock) -> String {
        match reset_phrase(Some(self.resets_at), now, clock) {
            Some(reset) => format!("{} {reset}", self.window),
            None => self.window.clone(),
        }
    }

    /// The exhausted window of an account's reading with the latest reset, or
    /// `None` when no window is at 100% (then the row says only "Rate
    /// limited"). A stale reading still counts for a window at 100% whose
    /// reset is ahead: usage cannot drop before its window resets.
    pub fn current(windows: &[RingWindow], now: SystemTime) -> Option<RateLimitReset> {
        let mut latest: Option<(&RingWindow, SystemTime)> = None;
        for window in windows {
            let Some(resets_at) = window.resets_at else {
                continue;
            };
            if window.money.is_some() || window.used_fraction < 1.0 || resets_at <= now {
                continue;
            }
            // The first of equal resets wins, as the Mac's `max(by:)`.
            if latest.is_none_or(|(_, best)| resets_at > best) {
                latest = Some((window, resets_at));
            }
        }
        let (window, resets_at) = latest?;
        Some(RateLimitReset {
            window: limit_name(&window.id, window.label.as_deref()),
            resets_at,
        })
    }

    /// The same, from the windows the snapshot carries (`UpstreamWindow`:
    /// `used` 0..=1, resets in epoch ms; money is `extra_usage`).
    pub fn current_of_snapshot(
        windows: &[UpstreamWindow],
        now: SystemTime,
    ) -> Option<RateLimitReset> {
        let mut latest: Option<(&UpstreamWindow, SystemTime)> = None;
        for window in windows {
            let Some(resets_at) = window.resets_at.map(from_ms) else {
                continue;
            };
            if window.id == EXTRA_USAGE_ID || window.used < 1.0 || resets_at <= now {
                continue;
            }
            if latest.is_none_or(|(_, best)| resets_at > best) {
                latest = Some((window, resets_at));
            }
        }
        let (window, resets_at) = latest?;
        Some(RateLimitReset {
            window: limit_name(
                &window.id,
                Some(window.label.as_str()).filter(|label| !label.is_empty()),
            ),
            resets_at,
        })
    }
}

fn limit_name(id: &str, label: Option<&str>) -> String {
    match id {
        "session" => "5-hour limit".to_owned(),
        "weekly_all" => "weekly limit".to_owned(),
        _ => {
            let model = match label {
                Some(label) => label.to_owned(),
                None => capitalized(&id.replace("weekly_", "").replace('_', " ")),
            };
            format!("{model} weekly limit")
        }
    }
}

/// Foundation's `capitalized`: every word starts upper case, the rest lower.
fn capitalized(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- time words (UsageFormatter) ----

/// Compact countdown: "<1m", "45m", "2h 13m", "2h", "3d 5h", "3d".
pub fn duration_text(interval: Duration) -> String {
    let total = interval.min(MAX_DURATION).as_secs();
    let (days, hours, minutes) = (
        total / 86_400,
        (total % 86_400) / 3_600,
        (total % 3_600) / 60,
    );
    if days > 0 {
        return if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        };
    }
    if hours > 0 {
        return if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        };
    }
    if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".to_owned()
    }
}

/// "just now", "5m ago", "3h ago", "2d ago". A later date reads "just now".
pub fn age_text(date: SystemTime, now: SystemTime) -> String {
    let seconds = now
        .duration_since(date)
        .unwrap_or_default()
        .min(MAX_DURATION)
        .as_secs();
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

/// Reset wording for inside a sentence: "resets in 47m" within a day, "resets
/// Tue 9:00 AM" beyond ("next Tue" when that is today's weekday, a week
/// out), "has reset" once passed, `None` when unknown.
pub fn reset_phrase(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    clock: ResetClock,
) -> Option<String> {
    let resets_at = resets_at?;
    let remaining = match resets_at.duration_since(now) {
        Ok(remaining) if !remaining.is_zero() => remaining,
        _ => return Some("has reset".to_owned()),
    };
    if remaining < COUNTDOWN_HORIZON {
        return Some(format!("resets in {}", duration_text(remaining)));
    }
    let offset = FixedOffset::east_opt(clock.utc_offset_seconds)?;
    let local = |at: SystemTime| DateTime::<Utc>::from(at).with_timezone(&offset);
    let (reset, today) = (local(resets_at), local(now));
    let weekday = reset.format("%a").to_string();
    // The same weekday on a later date would read as later today.
    let weekday = if reset.weekday() == today.weekday() && reset.date_naive() != today.date_naive()
    {
        format!("next {weekday}")
    } else {
        weekday
    };
    let time = if clock.hour12 {
        reset.format("%-I:%M %p").to_string()
    } else {
        reset.format("%H:%M").to_string()
    };
    Some(format!("resets {weekday} {time}"))
}

// ---- states ----

/// The row's state word (`SessionRow::state_word`).
pub fn state_word(state: &SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou(_) => "needs you",
        SessionState::Failed(_) => "failed",
        SessionState::ReadyForReview => "done",
        SessionState::Working => "working",
        SessionState::Idle => "idle",
    }
}

/// What VoiceOver says for the state.
pub fn spoken_state(state: &SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou(_) => "Needs you",
        SessionState::Failed(_) => "Failed",
        SessionState::ReadyForReview => "Ready for review",
        SessionState::Working => "Working",
        SessionState::Idle => "Idle",
    }
}

/// The card's state name (`CardRow::state`).
pub fn card_state(state: &SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou(_) => "needs_you",
        SessionState::Failed(_) => "failed",
        SessionState::ReadyForReview => "review",
        SessionState::Working => "working",
        SessionState::Idle => "idle",
    }
}

/// The approval the session waits on now, in its phase.
fn active_permission(view: &SessionView) -> Option<&PermissionContext> {
    match &view.phase {
        Phase::WaitingForApproval(context) => Some(context),
        _ => None,
    }
}

/// When the session started waiting on the user: the pending approval's
/// arrival, else the last hook event (the one that blocked it).
pub fn waiting_since(view: &SessionView) -> SystemTime {
    active_permission(view).map_or(view.last_activity, |permission| permission.received_at)
}

/// What the row's elapsed label counts from (UI§5.4): the wait, the turn,
/// the completion, or the last activity.
pub fn since(view: &SessionView) -> SystemTime {
    match view.state {
        SessionState::NeedsYou(_) | SessionState::Failed(_) => waiting_since(view),
        SessionState::Working => view.turn_started_at.unwrap_or(view.last_activity),
        SessionState::ReadyForReview => view.completed_at.unwrap_or(view.last_activity),
        SessionState::Idle => view.last_activity,
    }
}

/// The row's time label: how long it has waited (needs you), how long the
/// turn has run (working), when it finished ("5m ago", review) or when it
/// was last active (idle). `None` when there is nothing meaningful to show.
pub fn elapsed(view: &SessionView, now: SystemTime) -> Option<String> {
    let since_now = |from: SystemTime| now.duration_since(from).unwrap_or_default();
    match view.state.bucket() {
        Bucket::NeedsYou => Some(duration_text(since_now(waiting_since(view)))),
        Bucket::Working => Some(duration_text(since_now(view.turn_started_at?))),
        Bucket::ReadyForReview => Some(age_text(view.completed_at?, now)),
        Bucket::Idle => Some(age_text(view.last_activity, now)),
    }
}

// ---- the second line ----

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.trim().is_empty())
}

/// A tool's name and its input as one line.
fn tool_line(tool: &str, input: Option<&str>) -> String {
    let name = format_tool_name(tool);
    match non_empty(input) {
        Some(input) => format!("{name} {}", collapse_whitespace(input)),
        None => name,
    }
}

/// The request as a row shows it: the whole text (`PendingRequest`'s
/// preview), else the engine's one-line input.
fn request_text(view: &SessionView, permission: &PermissionContext) -> String {
    view.pending
        .iter()
        .find(|request| request.tool_use_id == permission.tool_use_id)
        .map(|request| request.input_preview.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .or_else(|| permission.formatted_input())
        .unwrap_or_default()
}

/// The second line for the session's state.
pub fn detail(
    view: &SessionView,
    rate_limit: Option<&RateLimitReset>,
    now: SystemTime,
    clock: ResetClock,
) -> RowDetail {
    match &view.state {
        SessionState::NeedsYou(reason) | SessionState::Failed(reason) => {
            needs_input_detail(view, reason, rate_limit, now, clock)
        }
        SessionState::Working => working_detail(view),
        SessionState::ReadyForReview => {
            let message = non_empty(view.last_assistant_message.as_deref())
                .or_else(|| non_empty(view.last_message.as_deref()))
                .unwrap_or("Finished");
            RowDetail::Review {
                text: collapse_whitespace(message),
            }
        }
        SessionState::Idle => idle_detail(view),
    }
}

fn needs_input_detail(
    view: &SessionView,
    reason: &NeedsInputReason,
    rate_limit: Option<&RateLimitReset>,
    now: SystemTime,
    clock: ResetClock,
) -> RowDetail {
    match reason {
        NeedsInputReason::Permission { tool } => match active_permission(view) {
            Some(permission) => RowDetail::Permission {
                tool: format_tool_name(&permission.tool_name),
                request: request_text(view, permission),
                waiting_in_terminal: false,
            },
            // A prompt seen only through a notification or the registry: it
            // can be answered in the terminal, not from here.
            None => RowDetail::Permission {
                tool: match tool.as_deref() {
                    Some(tool) if !tool.is_empty() => format_tool_name(tool),
                    _ => "Permission".to_owned(),
                },
                request: TERMINAL_WAIT.to_owned(),
                waiting_in_terminal: true,
            },
        },
        NeedsInputReason::Question => {
            let question = active_permission(view)
                .and_then(|permission| parse_questions(&permission.tool_input).into_iter().next())
                .map(|question| collapse_whitespace(&question.text))
                .filter(|text| !text.is_empty());
            RowDetail::Question {
                text: question.unwrap_or_else(|| "A question for you".to_owned()),
            }
        }
        NeedsInputReason::PlanApproval => RowDetail::Plan,
        NeedsInputReason::Elicitation { .. } | NeedsInputReason::Dialog { .. } => {
            RowDetail::Dialog {
                text: collapse_whitespace(&reason.display_text()),
            }
        }
        NeedsInputReason::Error { text, .. } => {
            let mut text = text.clone();
            if text == humanized_stop_error(Some("rate_limit")) {
                if let Some(limit) = rate_limit {
                    text.push_str(&format!(" · {}", limit.phrase(now, clock)));
                }
            }
            RowDetail::Failed { text }
        }
    }
}

fn working_detail(view: &SessionView) -> RowDetail {
    let secondary = |text: String| RowDetail::Working {
        text,
        secondary: true,
    };
    if view.phase == Phase::Compacting {
        return secondary("Compacting context…".to_owned());
    }
    if let Some(wait) = &view.background_wait_description {
        // The turn is over; the agents it started aren't.
        return secondary(format!("Waiting on {wait}…"));
    }
    if let Some(label) = view
        .tasks
        .as_ref()
        .and_then(|tasks| tasks.active_label.as_deref())
    {
        return RowDetail::Working {
            text: collapse_whitespace(label),
            secondary: false,
        };
    }
    if view.last_message_role.as_deref() == Some("tool") {
        if let Some(tool) = view
            .last_tool_name
            .as_deref()
            .filter(|tool| !tool.is_empty())
        {
            return secondary(tool_line(tool, view.last_message.as_deref()));
        }
    }
    secondary("Thinking…".to_owned())
}

fn idle_detail(view: &SessionView) -> RowDetail {
    match view.last_message_role.as_deref() {
        Some("tool") => {
            if let Some(tool) = view
                .last_tool_name
                .as_deref()
                .filter(|tool| !tool.is_empty())
            {
                return RowDetail::Idle {
                    text: tool_line(tool, view.last_message.as_deref()),
                };
            }
        }
        Some("user") => {
            if let Some(message) = non_empty(view.last_message.as_deref()) {
                return RowDetail::Idle {
                    text: format!("You: {}", collapse_whitespace(message)),
                };
            }
        }
        _ => {}
    }
    let message = non_empty(view.last_message.as_deref())
        .or_else(|| non_empty(view.last_assistant_message.as_deref()));
    RowDetail::Idle {
        text: message.map_or_else(|| "No messages yet".to_owned(), collapse_whitespace),
    }
}

/// The whole line as one string (VoiceOver, one-line rows).
pub fn plain_text(detail: &RowDetail) -> String {
    match detail {
        RowDetail::Permission { tool, request, .. } if request.is_empty() => tool.clone(),
        RowDetail::Permission { tool, request, .. } => format!("{tool} {request}"),
        RowDetail::Question { text } => format!("Asks {text}"),
        RowDetail::Plan => "Plan ready for approval".to_owned(),
        RowDetail::Dialog { text }
        | RowDetail::Failed { text }
        | RowDetail::Working { text, .. }
        | RowDetail::Review { text }
        | RowDetail::Idle { text } => text.clone(),
    }
}

/// The short text a one-line row shows after the title: what Claude is on,
/// what it asks, or the project.
pub fn compact_detail(
    view: &SessionView,
    rate_limit: Option<&RateLimitReset>,
    now: SystemTime,
    clock: ResetClock,
) -> String {
    match view.state {
        SessionState::Working | SessionState::NeedsYou(_) | SessionState::Failed(_) => {
            plain_text(&detail(view, rate_limit, now, clock))
        }
        SessionState::ReadyForReview | SessionState::Idle => view.display_project_name.clone(),
    }
}

// ---- VoiceOver ----

/// "3 of 7 tasks done, now: Writing tests".
pub fn task_summary(tasks: &TaskProgress) -> String {
    let mut text = format!("{} of {} tasks done", tasks.done, tasks.total);
    if let Some(active) = &tasks.active_label {
        text.push_str(&format!(", now: {active}"));
    }
    text
}

/// One sentence for VoiceOver: title, state, what it waits on, the time and
/// the account. Never the assistant's message.
pub fn accessibility_label(
    view: &SessionView,
    account_label: Option<&str>,
    rate_limit: Option<&RateLimitReset>,
    now: SystemTime,
    clock: ResetClock,
) -> String {
    let mut parts = vec![view.title.clone(), spoken_state(&view.state).to_owned()];
    if matches!(
        view.state,
        SessionState::NeedsYou(_) | SessionState::Failed(_) | SessionState::Working
    ) {
        parts.push(plain_text(&detail(view, rate_limit, now, clock)));
    }
    if let Some(elapsed) = elapsed(view, now) {
        parts.push(match view.state.bucket() {
            Bucket::NeedsYou => format!("waiting {elapsed}"),
            Bucket::Working => format!("running {elapsed}"),
            Bucket::ReadyForReview => format!("finished {elapsed}"),
            Bucket::Idle => format!("last active {elapsed}"),
        });
    }
    if let Some(tasks) = view.tasks.as_ref().filter(|tasks| tasks.total > 0) {
        parts.push(task_summary(tasks));
    }
    if let Some(label) = account_label {
        parts.push(format!("account {label}"));
    }
    parts.join(", ")
}

// ---- the request the action bar answers ----

/// A request as the row's action bar needs it.
pub fn pending_view(request: &PendingRequest) -> PendingRequestView {
    let (kind, shown) = match request.kind {
        RequestKind::Permission => ("permission", permission_text(request)),
        RequestKind::Question => ("question", request.input_preview.clone()),
        RequestKind::Plan => ("plan", "Plan ready for approval".to_owned()),
    };
    // One single-choice question with a few options: one tap answers.
    let single_tap = request.questions.as_ref().is_some_and(|questions| {
        matches!(questions.as_slice(), [only]
            if !only.multi_select && (1..=MAX_INLINE_OPTIONS).contains(&only.options.len()))
    });
    PendingRequestView {
        tool_use_id: request.tool_use_id.clone(),
        kind: kind.to_owned(),
        tool_name: request.tool_name.clone(),
        received_at_ms: to_ms(request.received_at),
        request: shown,
        needs_review: request.needs_review,
        always: request.always.as_ref().map(|rule| rule.description.clone()),
        inline_always: request.always.as_ref().is_some_and(|rule| rule.inline),
        questions: request.questions.clone(),
        single_tap,
        plan_markdown: request.plan_markdown.clone(),
        diff: None,
    }
}

fn permission_text(request: &PendingRequest) -> String {
    if !request.input_preview.is_empty() {
        return request.input_preview.clone();
    }
    let flat = crate::sessions::tool_input::flatten_value(&request.input);
    crate::sessions::tool_input::preview(
        &request.tool_name,
        &flat,
        Some(PermissionContext::PREVIEW_LENGTH),
    )
    .unwrap_or_default()
}

// ---- the session row ----

/// "Show in editor" for sessions in VS Code, else "Show terminal".
pub fn focus_label(view: &SessionView) -> &'static str {
    if view.entrypoint.as_deref() == Some("claude-vscode") {
        "Show in editor"
    } else {
        "Show terminal"
    }
}

/// The panel's row for one session.
pub fn session_row(view: &SessionView, ctx: &RowContext<'_>) -> SessionRow {
    let project = Some(view.display_project_name.clone()).filter(|name| !name.is_empty());
    SessionRow {
        session_id: view.id.as_str().to_owned(),
        ring_id: view.ring.as_ref().map(|ring| ring.as_str().to_owned()),
        account_label: ctx.account_label.map(str::to_owned),
        account_color: ctx.account_color,
        title: view.title.clone(),
        project,
        bucket: view.state.bucket().as_str().to_owned(),
        failed: view.has_failed_turn(),
        state_word: state_word(&view.state).to_owned(),
        since_ms: to_ms(since(view)),
        detail: detail(view, ctx.rate_limit, ctx.now, ctx.clock),
        tasks: view.tasks.clone(),
        context_pct: view.context_pct,
        background_count: view.background.task_count,
        pending: view.active_request().map(pending_view),
        focus_label: ctx.can_focus.then(|| focus_label(view).to_owned()),
        can_message: ctx.can_message,
        reviewable: view.state.bucket() == Bucket::ReadyForReview,
        a11y: accessibility_label(view, ctx.account_label, ctx.rate_limit, ctx.now, ctx.clock),
        card: card_row(view, ctx),
    }
}

// ---- the hover card ----

/// The tool the session runs now (the newest one started), by its display
/// name; `None` between tools.
pub fn running_tool(view: &SessionView) -> Option<String> {
    view.running_tools
        .iter()
        .max_by_key(|tool| tool.started_at)
        .map(|tool| format_tool_name(&tool.name))
}

/// What the card says a session waits on, in a few words: "Approve Bash",
/// "Question · Charts", "Plan ready for approval", "Rate limited". Never the
/// request's input, a prompt or a reply: the card shows over other windows.
fn waiting_line(view: &SessionView, reason: &NeedsInputReason) -> String {
    match reason {
        NeedsInputReason::Permission { tool } => {
            let name = active_permission(view)
                .map(|permission| permission.tool_name.as_str())
                .or(tool.as_deref())
                .unwrap_or_default();
            if name.is_empty() {
                "Needs permission".to_owned()
            } else {
                format!("Approve {}", format_tool_name(name))
            }
        }
        NeedsInputReason::Question => {
            let header = active_permission(view)
                .and_then(|permission| parse_questions(&permission.tool_input).into_iter().next())
                .and_then(|question| question.header)
                .map(|header| collapsed(&header, CARD_HEADER_LIMIT))
                .filter(|header| !header.is_empty());
            match header {
                Some(header) => format!("Question · {header}"),
                None => "Question".to_owned(),
            }
        }
        NeedsInputReason::PlanApproval => "Plan ready for approval".to_owned(),
        NeedsInputReason::Elicitation { .. }
        | NeedsInputReason::Dialog { .. }
        | NeedsInputReason::Error { .. } => collapsed(&reason.display_text(), CARD_REASON_LIMIT),
    }
}

/// "3/7 Writing tests" (with the active task cut to 48 characters), or just
/// "3/7" when none is active or `with_active` is off.
fn card_tasks(view: &SessionView, with_active: bool) -> Option<String> {
    let tasks = view.tasks.as_ref().filter(|tasks| tasks.total > 0)?;
    let mut text = format!("{}/{}", tasks.done, tasks.total);
    if with_active {
        if let Some(active) = tasks
            .active_label
            .as_deref()
            .map(|label| collapsed(label, CARD_TASK_LIMIT))
            .filter(|label| !label.is_empty())
        {
            text.push(' ');
            text.push_str(&active);
        }
    }
    Some(text)
}

/// The card's progress line: "3/7 Writing tests · ctx 42%", "Waiting on 1
/// workflow · 3/7 · ctx 42%", "Bash… · ctx 42%", "2 background · ctx 22%";
/// `None` when there is nothing to say.
fn card_detail(view: &SessionView) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match view.state {
        SessionState::Working => {
            if let Some(wait) = &view.background_wait_description {
                // The turn is over; its agents aren't.
                parts.push(format!("Waiting on {wait}"));
                parts.extend(card_tasks(view, false));
            } else if let Some(tasks) = card_tasks(view, true) {
                parts.push(tasks);
            } else if let Some(tool) = running_tool(view).filter(|tool| !tool.is_empty()) {
                parts.push(format!("{tool}…"));
            }
        }
        SessionState::ReadyForReview => {
            parts.extend(card_tasks(view, true));
            if view.background.task_count > 0 {
                parts.push(format!("{} background", view.background.task_count));
            }
        }
        _ => parts.extend(card_tasks(view, true)),
    }
    if let Some(context) = view.context_pct.filter(|context| context.is_finite()) {
        parts.push(format!("ctx {}%", context.round() as i64));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The hover card's row: "<host app> · <project>" (or the title when the
/// host is unknown), the progress, the state and what it waits on.
pub fn card_row(view: &SessionView, ctx: &RowContext<'_>) -> CardRow {
    let place = [ctx.host_app, Some(view.display_project_name.as_str())]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let name = if ctx.host_app.is_some_and(|host| !host.is_empty()) && !place.is_empty() {
        place
    } else {
        view.public_title.clone()
    };
    CardRow {
        name,
        detail: card_detail(view),
        state: card_state(&view.state).to_owned(),
        waiting_for: view.state.reason().map(|reason| waiting_line(view, reason)),
        since_ms: to_ms(since(view)),
    }
}
