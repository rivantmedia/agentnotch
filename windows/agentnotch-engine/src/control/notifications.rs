//! Banners for sessions that need the user or finished work to review
//! (HS§7, §4.10): what each says, which toast it replaces, when it is
//! withdrawn, and what a click on it means. A port of the Mac's
//! `SessionNotificationContent`, `LimitNotificationContent` and
//! `NotificationRouting`.
//!
//! - One toast per session and kind: the tag is the kind (`needs`, `review`,
//!   `failed`), the group the session id, so a newer banner replaces the
//!   older and each is withdrawn once it no longer applies.
//! - Sessions stopped by a usage limit share one banner per account ring
//!   (tag `limit`, group the ring id): "Work: 3 sessions hit the limit".
//! - Any other failed turn gets "<title> stopped" with what to do, never
//!   "needs you": there is nothing to answer.
//! - Private: a banner names the session by its public title (never the
//!   first prompt) and says what it wants in the words of the hover rows (a
//!   tool and a short input preview, a question's header), never Claude's
//!   messages, questions or plans. Windows keeps banners in its notification
//!   centre and shows them on the lock screen.
//! - Silent: the app chimes itself.
//! - A click is a protocol activation (`agentnotch://open?…`), which only
//!   ever opens the panel or marks a completion reviewed. It never answers
//!   a request.

use super::text::{
    collapse_whitespace, collapsed, format_tool_name, off_panel_preview, preview, truncated,
    PREVIEW_LENGTH,
};
use crate::attention::policy;
use crate::core::time;
use crate::model::{
    AttentionTransition, Bucket, NeedsInputReason, Phase, RingId, SessionId, SessionState,
    SessionView,
};
use crate::platform::{NotifyPermission, Toast, ToastKind};
use crate::runtime_types::ToastContext;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

/// A banner's title is cut to this many characters (before " needs you").
pub const MAX_TITLE_LENGTH: usize = 60;
pub const MAX_BODY_LENGTH: usize = 220;
/// Windows limits a toast's tag and group to 64 UTF-16 units each.
pub const MAX_IDENTIFIER_LENGTH: usize = 64;

// ---- Identifiers ----

/// The toast tag of each kind.
pub fn tag(kind: ToastKind) -> &'static str {
    match kind {
        ToastKind::NeedsInput => "needs",
        ToastKind::Review => "review",
        ToastKind::Failed => "failed",
        ToastKind::Limit => "limit",
    }
}

/// The kind of one of our tags; `None` for anybody else's.
pub fn kind_of_tag(tag: &str) -> Option<ToastKind> {
    match tag {
        "needs" => Some(ToastKind::NeedsInput),
        "review" => Some(ToastKind::Review),
        "failed" => Some(ToastKind::Failed),
        "limit" => Some(ToastKind::Limit),
        _ => None,
    }
}

/// A session's or ring's toast group: the id itself when it fits, else its
/// start and a hash, so two long ids never share a group.
pub fn group(id: &str) -> String {
    if id.encode_utf16().count() <= MAX_IDENTIFIER_LENGTH {
        return id.to_owned();
    }
    let digest = Sha256::digest(id.as_bytes());
    let hash: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    // 47 units of the id, a dash and 16 hex digits.
    let budget = MAX_IDENTIFIER_LENGTH - 1 - hash.len();
    let mut start = String::new();
    let mut used = 0;
    for c in id.chars() {
        used += c.len_utf16();
        if used > budget {
            break;
        }
        start.push(c);
    }
    format!("{start}-{hash}")
}

// ---- What a click means ----

/// The app's URL scheme (`web/contract` fixes it for the sign-in callback).
pub const URL_SCHEME: &str = "agentnotch";

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// A banner's own click and its "Open" button: the panel, at this session.
pub fn open_session_url(session: &SessionId) -> String {
    format!("{URL_SCHEME}://open?session={}", encode(session.as_str()))
}

/// The limit banner's click: the panel, on this ring's sessions.
pub fn open_ring_url(ring: &RingId) -> String {
    format!("{URL_SCHEME}://open?ring={}", encode(ring.as_str()))
}

/// "Mark Reviewed": reviews the completion the banner announced, never a
/// later one (the banner can sit in the notification centre for hours).
pub fn review_url(session: &SessionId, completed_at: Option<SystemTime>) -> String {
    let mut url = format!("{URL_SCHEME}://review?session={}", encode(session.as_str()));
    if let Some(completed_at) = completed_at {
        url.push_str(&format!("&completed={}", time::to_ms(completed_at)));
    }
    url
}

/// What an `agentnotch://` link from a banner asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkAction {
    /// Open the sessions panel on this session.
    OpenSession(SessionId),
    /// Open the sessions panel on this ring's list.
    OpenRing(RingId),
    /// Mark the completion at `completed_at` reviewed (`None`: as of now).
    MarkReviewed {
        session: SessionId,
        completed_at: Option<SystemTime>,
    },
}

/// Longest link and longest id a banner link may carry: ours are far
/// shorter, and anything can put an `agentnotch:` link on the command line.
const MAX_LINK_LENGTH: usize = 2048;
const MAX_LINK_ID_LENGTH: usize = 256;

/// A banner's link, or `None` for anything else (the sign-in callback, a
/// link another program made up, a malformed one). Opening and reviewing
/// are all a link can ask for; the hub still ignores sessions and rings it
/// doesn't know.
pub fn parse_deep_link(link: &str) -> Option<DeepLinkAction> {
    if link.len() > MAX_LINK_LENGTH {
        return None;
    }
    let url = url::Url::parse(link.trim()).ok()?;
    if url.scheme() != URL_SCHEME || !matches!(url.path(), "" | "/") {
        return None;
    }
    let value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty() && value.chars().count() <= MAX_LINK_ID_LENGTH)
    };
    match url.host_str()?.to_ascii_lowercase().as_str() {
        "open" => match value("session") {
            Some(session) => Some(DeepLinkAction::OpenSession(SessionId(session))),
            None => value("ring").map(|ring| DeepLinkAction::OpenRing(RingId(ring))),
        },
        "review" => Some(DeepLinkAction::MarkReviewed {
            session: SessionId(value("session")?),
            completed_at: value("completed")
                .and_then(|ms| ms.parse::<u64>().ok())
                .map(time::from_ms),
        }),
        _ => None,
    }
}

/// At most [`DeepLinkGate::LIMIT`] banner links a minute: a link is a
/// command line anything can run, and each one moves the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeepLinkGate {
    recent: VecDeque<SystemTime>,
}

impl DeepLinkGate {
    pub const LIMIT: usize = 10;
    pub const WINDOW: Duration = Duration::from_secs(60);

    /// Whether a link arriving `now` is acted on (it then counts).
    pub fn allow(&mut self, now: SystemTime) -> bool {
        while self.recent.front().is_some_and(|at| {
            now.duration_since(*at)
                .map_or(true, |age| age >= Self::WINDOW)
        }) {
            self.recent.pop_front();
        }
        if self.recent.len() >= Self::LIMIT {
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

// ---- Reasons ----

/// Why a turn failed (StopFailure's `error`), grouped by what the user can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopErrorKind {
    /// The account hit a usage limit; the session can go on after the reset.
    RateLimit,
    /// Anthropic's side was busy or failed; retrying may work.
    Overloaded,
    ServerError,
    /// The account needs /login, or can't be used.
    Authentication,
    Billing,
    /// The request itself was refused.
    InvalidRequest,
    MaxOutputTokens,
    Other,
}

impl StopErrorKind {
    /// From the raw code; `None` without one.
    pub fn from_code(code: Option<&str>) -> Option<StopErrorKind> {
        let code = code?.to_lowercase();
        if code.is_empty() {
            return None;
        }
        Some(match code.as_str() {
            "rate_limit" => StopErrorKind::RateLimit,
            "overloaded" => StopErrorKind::Overloaded,
            "server_error" => StopErrorKind::ServerError,
            "authentication_failed"
            | "oauth_org_not_allowed"
            | "account_on_hold"
            | "cloud_credential_error" => StopErrorKind::Authentication,
            "billing_error" => StopErrorKind::Billing,
            "invalid_request" | "model_not_found" => StopErrorKind::InvalidRequest,
            "max_output_tokens" => StopErrorKind::MaxOutputTokens,
            _ => StopErrorKind::Other,
        })
    }

    pub fn display_text(self) -> &'static str {
        match self {
            StopErrorKind::RateLimit => "Rate limited",
            StopErrorKind::Overloaded => "Overloaded",
            StopErrorKind::ServerError => "Server error",
            StopErrorKind::Authentication => "Sign-in failed",
            StopErrorKind::Billing => "Billing problem",
            StopErrorKind::InvalidRequest => "Invalid request",
            StopErrorKind::MaxOutputTokens => "Output limit reached",
            StopErrorKind::Other => "Turn failed",
        }
    }

    /// The kind of a failed turn's reason: by its code, else by the text a
    /// known code is humanised to (a failure restored without its code).
    pub fn of(reason: &NeedsInputReason) -> Option<StopErrorKind> {
        let NeedsInputReason::Error { text, code } = reason else {
            return None;
        };
        StopErrorKind::from_code(code.as_deref()).or_else(|| {
            [
                StopErrorKind::RateLimit,
                StopErrorKind::Overloaded,
                StopErrorKind::ServerError,
                StopErrorKind::Authentication,
                StopErrorKind::Billing,
                StopErrorKind::InvalidRequest,
                StopErrorKind::MaxOutputTokens,
            ]
            .into_iter()
            .find(|kind| kind.display_text() == text)
        })
    }
}

/// A failed turn because the account hit a usage limit.
pub fn is_rate_limit(reason: &NeedsInputReason) -> bool {
    StopErrorKind::of(reason) == Some(StopErrorKind::RateLimit)
}

/// What the user can do about a failed turn.
pub fn failure_hint(kind: Option<StopErrorKind>) -> Option<&'static str> {
    match kind? {
        StopErrorKind::Overloaded | StopErrorKind::ServerError => Some("retry in its terminal"),
        StopErrorKind::Authentication => Some("run /login in its terminal"),
        StopErrorKind::Billing => Some("check the account's billing"),
        StopErrorKind::MaxOutputTokens => Some("ask Claude to go on in smaller steps"),
        StopErrorKind::InvalidRequest | StopErrorKind::Other | StopErrorKind::RateLimit => None,
    }
}

fn is_error(state: &SessionState) -> bool {
    matches!(state, SessionState::Failed(_))
        || state.reason().is_some_and(NeedsInputReason::is_error)
}

fn state_is_rate_limit(state: &SessionState) -> bool {
    state.reason().is_some_and(is_rate_limit)
}

// ---- Content ----

/// The tool and input of the request the session shows, when it holds one.
fn active_request(view: &SessionView) -> Option<(&str, &Value)> {
    if let Phase::WaitingForApproval(context) = &view.phase {
        return Some((&context.tool_name, &context.tool_input));
    }
    view.pending
        .first()
        .map(|request| (request.tool_name.as_str(), &request.input))
}

/// How many questions an AskUserQuestion input holds (entries with a
/// question text), and the first entry's header as a label.
fn questions(input: Option<&Value>) -> (usize, Option<String>) {
    let Some(entries) = input
        .and_then(|input| input.get("questions"))
        .and_then(Value::as_array)
    else {
        return (0, None);
    };
    let count = entries
        .iter()
        .filter(|entry| {
            entry
                .get("question")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
        })
        .count();
    // Claude Code caps headers at a dozen characters: a label, not the
    // question itself.
    let header = entries
        .first()
        .and_then(|entry| entry.get("header"))
        .and_then(Value::as_str)
        .map(|header| collapsed(header, 24))
        .filter(|header| !header.is_empty());
    (count, header)
}

/// What a needs-you banner says: the tool and its input for a permission,
/// the question's header, a plan to approve, or the reason (with when the
/// limit lifts, for a rate-limited turn).
pub fn needs_input_body(
    view: &SessionView,
    reason: &NeedsInputReason,
    limit_reset: Option<&str>,
) -> String {
    let request = active_request(view);
    let body = match reason {
        NeedsInputReason::Permission { tool } => {
            let tool_name = request
                .map(|(name, _)| name)
                .or(tool.as_deref())
                .unwrap_or("");
            let label = if tool_name.is_empty() {
                reason.display_text()
            } else {
                format!("Approve {}", format_tool_name(tool_name))
            };
            let input = request.and_then(|(name, input)| {
                preview(off_panel_preview(name, input, Some(PREVIEW_LENGTH)).as_deref())
            });
            match input {
                Some(input) => format!("{label}: {input}"),
                None => label,
            }
        }
        NeedsInputReason::Question => {
            let (count, header) = questions(request.map(|(_, input)| input));
            if count == 0 {
                reason.display_text()
            } else {
                let more = if count > 1 {
                    format!(" (+{} more)", count - 1)
                } else {
                    String::new()
                };
                match header {
                    Some(header) => format!("Question · {header}{more}"),
                    None => format!("Question for you{more}"),
                }
            }
        }
        NeedsInputReason::PlanApproval => "Plan ready for approval".to_owned(),
        NeedsInputReason::Elicitation { .. } | NeedsInputReason::Dialog { .. } => {
            reason.display_text()
        }
        NeedsInputReason::Error { .. } => match limit_reset {
            Some(reset) if is_rate_limit(reason) => {
                format!("{} · {reset}", reason.display_text())
            }
            _ => reason.display_text(),
        },
    };
    truncated(&body, MAX_BODY_LENGTH)
}

/// The session's name on a banner: the public title the hub gives, else the
/// project folder. Never the row's own title, which can be the first prompt
/// until Claude Code names the session.
fn public_title(view: &SessionView, ctx: &ToastContext) -> String {
    preview(Some(&ctx.title))
        .or_else(|| project(view, ctx))
        .unwrap_or_else(|| "Claude Code".to_owned())
}

fn project(view: &SessionView, ctx: &ToastContext) -> Option<String> {
    preview(Some(&ctx.project)).or_else(|| preview(Some(&view.project_name)))
}

/// The account's name, only when several accounts are in use.
fn subtitle(ctx: &ToastContext) -> Option<String> {
    if !ctx.multi_account {
        return None;
    }
    preview(ctx.account_label.as_deref())
}

fn session_toast(
    kind: ToastKind,
    view: &SessionView,
    title: String,
    subtitle: Option<String>,
    body: String,
) -> Toast {
    let launch_url = open_session_url(&view.id);
    let mut actions = vec![("Open".to_owned(), launch_url.clone())];
    if kind == ToastKind::Review {
        actions.push((
            "Mark Reviewed".to_owned(),
            review_url(&view.id, view.completed_at),
        ));
    }
    Toast {
        tag: tag(kind).to_owned(),
        group: group(view.id.as_str()),
        kind,
        title,
        subtitle,
        body,
        launch_url,
        actions,
    }
}

/// "<title> needs you", with why and what.
pub fn needs_input_toast(
    view: &SessionView,
    reason: &NeedsInputReason,
    ctx: &ToastContext,
    limit_reset: Option<&str>,
) -> Toast {
    session_toast(
        ToastKind::NeedsInput,
        view,
        format!(
            "{} needs you",
            truncated(&public_title(view, ctx), MAX_TITLE_LENGTH)
        ),
        subtitle(ctx),
        needs_input_body(view, reason, limit_reset),
    )
}

/// "Done: <title>", "Ready for review · <project>", and how many background
/// tasks still run. Claude's final message stays in the panel.
pub fn review_toast(view: &SessionView, ctx: &ToastContext) -> Toast {
    let mut parts = vec!["Ready for review".to_owned()];
    parts.extend(project(view, ctx));
    let background = view.background.task_count;
    if background > 0 {
        parts.push(format!(
            "{background} background task{} running",
            if background == 1 { "" } else { "s" }
        ));
    }
    session_toast(
        ToastKind::Review,
        view,
        format!(
            "Done: {}",
            truncated(&public_title(view, ctx), MAX_TITLE_LENGTH)
        ),
        subtitle(ctx),
        truncated(&parts.join(" · "), MAX_BODY_LENGTH),
    )
}

/// "<title> stopped", with why and what to do: "Overloaded · retry in its
/// terminal", "Sign-in failed · run /login in its terminal".
pub fn failed_toast(view: &SessionView, reason: &NeedsInputReason, ctx: &ToastContext) -> Toast {
    let mut parts = vec![reason.display_text()];
    parts.extend(failure_hint(StopErrorKind::of(reason)).map(str::to_owned));
    session_toast(
        ToastKind::Failed,
        view,
        format!(
            "{} stopped",
            truncated(&public_title(view, ctx), MAX_TITLE_LENGTH)
        ),
        subtitle(ctx),
        truncated(&parts.join(" · "), MAX_BODY_LENGTH),
    )
}

/// The banner for one transition, or `None` when it announces nothing:
/// banners are off or not allowed, the user is looking at that session's
/// own terminal, or the turn hit a usage limit (those share one banner per
/// account: [`LimitBanners`]). The hub leaves out sessions of accounts
/// whose tracking is off.
pub fn toast_for(tr: &AttentionTransition, ctx: &ToastContext) -> Option<Toast> {
    if ctx.suppressed || ctx.permission != NotifyPermission::Allowed {
        return None;
    }
    let looking = ctx.looking_at == Some(true);
    let view = &tr.session;
    if tr.became_needs_you() && ctx.notify_needs_input {
        let reason = tr.to.reason()?;
        if is_rate_limit(reason) || looking {
            return None;
        }
        // A failed turn has nothing to answer: its own banner.
        return Some(if policy::is_failure(tr) {
            failed_toast(view, reason, ctx)
        } else {
            needs_input_toast(view, reason, ctx, None)
        });
    }
    if tr.became_ready_for_review() && ctx.notify_ready_for_review && !looking {
        return Some(review_toast(view, ctx));
    }
    None
}

// ---- Withdrawing ----

/// Whether a banner of `kind` still describes a session whose state is now
/// `state` (`None`: the session isn't known).
pub fn still_applies(kind: ToastKind, state: Option<&SessionState>) -> bool {
    match kind {
        ToastKind::NeedsInput => state.is_some_and(|state| state.bucket() == Bucket::NeedsYou),
        ToastKind::Review => state == Some(&SessionState::ReadyForReview),
        ToastKind::Failed => state.is_some_and(is_error),
        // One per ring, not per session: `stale` asks the ring.
        ToastKind::Limit => false,
    }
}

/// The banners (tag, group) a transition takes back: the one for a wait it
/// left, a failure it cleared, or a completion that was reviewed.
pub fn withdrawals(tr: &AttentionTransition) -> Vec<(String, String)> {
    let Some(from) = tr.from.as_ref() else {
        return Vec::new();
    };
    let group = group(tr.session.id.as_str());
    let mut gone = Vec::new();
    if from.bucket() == Bucket::NeedsYou && tr.to.bucket() != Bucket::NeedsYou {
        gone.push((tag(ToastKind::NeedsInput).to_owned(), group.clone()));
    }
    if is_error(from) && !is_error(&tr.to) {
        gone.push((tag(ToastKind::Failed).to_owned(), group.clone()));
    }
    if *from == SessionState::ReadyForReview && tr.to != SessionState::ReadyForReview {
        gone.push((tag(ToastKind::Review).to_owned(), group));
    }
    gone
}

/// A session that ended takes its banners with it.
pub fn withdrawals_for_gone(session: &SessionId) -> Vec<(String, String)> {
    let group = group(session.as_str());
    [ToastKind::NeedsInput, ToastKind::Review, ToastKind::Failed]
        .into_iter()
        .map(|kind| (tag(kind).to_owned(), group.clone()))
        .collect()
}

/// How long after launch the banners an earlier run left behind are checked
/// against the sessions found again ([`stale`]).
pub const LAUNCH_RECONCILE_DELAY: Duration = Duration::from_secs(10);

/// Of the banners still delivered (`Notifier::delivered`), those that no
/// longer apply: their session is gone or has moved on, or no session of
/// their ring is rate limited any more. A session whose account isn't known
/// yet counts on `default_ring`, where its rows go. Banners that aren't ours
/// are never touched.
pub fn stale(
    delivered: &[(String, String)],
    sessions: &[SessionView],
    default_ring: Option<&RingId>,
) -> Vec<(String, String)> {
    let limited_rings: BTreeSet<String> = sessions
        .iter()
        .filter(|view| state_is_rate_limit(&view.state))
        .filter_map(|view| view.ring.as_ref().or(default_ring))
        .map(|ring| group(ring.as_str()))
        .collect();
    delivered
        .iter()
        .filter(|(tag, toast_group)| match kind_of_tag(tag) {
            None => false,
            Some(ToastKind::Limit) => !limited_rings.contains(toast_group),
            Some(kind) => {
                let state = sessions
                    .iter()
                    .find(|view| group(view.id.as_str()) == *toast_group)
                    .map(|view| &view.state);
                !still_applies(kind, state)
            }
        })
        .cloned()
        .collect()
}

// ---- Accounts ----

/// A banner's subtitle: nothing for a single account; the account's name;
/// and with the organization, the plan or the folder when another visible
/// account has the same name (the same email in two organizations, say), so
/// the two can be told apart without their colours. `all_labels` maps every
/// visible account's id to its name.
pub fn account_subtitle(
    id: &str,
    label: &str,
    organization: Option<&str>,
    plan: Option<&str>,
    folder: &str,
    all_labels: &BTreeMap<String, String>,
) -> Option<String> {
    if all_labels.len() <= 1 {
        return None;
    }
    let collides = all_labels
        .iter()
        .any(|(other_id, other)| other_id != id && other.to_lowercase() == label.to_lowercase());
    if !collides {
        return Some(label.to_owned());
    }
    let detail = organization
        .filter(|organization| !organization.is_empty())
        .or(plan)
        .unwrap_or(folder);
    Some(format!("{label} · {detail}"))
}

// ---- The limit banner ----

/// A session stopped by its account's usage limit, by its public title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedSession {
    pub id: SessionId,
    pub title: String,
}

/// What the limit banner of one ring needs besides its sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitContext {
    pub notify_needs_input: bool,
    pub permission: NotifyPermission,
    /// Sealed, `AGENTNOTCH_NO_NOTIFICATIONS`, full screen.
    pub suppressed: bool,
    /// The account's name, when several accounts are in use.
    pub account_label: Option<String>,
    /// "resets 14:05" ([`reset_phrase`]), when the window that ran out says.
    pub limit_reset: Option<String>,
}

/// "Work: 3 sessions hit the limit" / "Refactor the parser hit the limit",
/// with when it lifts: "Rate limited · resets 14:05".
pub fn limit_toast(
    ring: &RingId,
    account_label: Option<&str>,
    session_titles: &[String],
    limit_reset: Option<&str>,
) -> Toast {
    let subject = match session_titles {
        [only] => truncated(only, MAX_TITLE_LENGTH),
        titles => format!("{} sessions", titles.len()),
    };
    let account = account_label
        .map(|label| format!("{label}: "))
        .unwrap_or_default();
    let reason = StopErrorKind::RateLimit.display_text();
    let body = match limit_reset {
        Some(reset) => format!("{reason} · {reset}"),
        None => format!("{reason}. Claude Code waits for you once it lifts."),
    };
    let launch_url = open_ring_url(ring);
    Toast {
        tag: tag(ToastKind::Limit).to_owned(),
        group: group(ring.as_str()),
        kind: ToastKind::Limit,
        title: format!("{account}{subject} hit the limit"),
        subtitle: None,
        body,
        launch_url: launch_url.clone(),
        actions: vec![("Open".to_owned(), launch_url)],
    }
}

/// What to do with a ring's limit banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitChange {
    Post(Toast),
    /// (tag, group).
    Withdraw(String, String),
    Nothing,
}

/// One limit banner per account ring, counting the sessions stopped by its
/// limit. A shrinking count isn't posted again (that would show the banner
/// again); zero withdraws it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitBanners {
    /// The rate-limited sessions each ring's banner counted.
    counted: BTreeMap<RingId, BTreeSet<SessionId>>,
}

impl LimitBanners {
    pub fn new() -> LimitBanners {
        LimitBanners::default()
    }

    /// Whether a transition changes some ring's count: it became rate
    /// limited, or stopped being so. The hub then calls [`update`] for the
    /// session's ring.
    ///
    /// [`update`]: LimitBanners::update
    pub fn concerns(tr: &AttentionTransition) -> bool {
        let was = tr.from.as_ref().is_some_and(state_is_rate_limit);
        let is = state_is_rate_limit(&tr.to);
        (was && !is) || (is && tr.became_needs_you())
    }

    /// The rings whose banner counted any of these sessions (they ended).
    pub fn rings_counting(&self, sessions: &BTreeSet<SessionId>) -> Vec<RingId> {
        self.counted
            .iter()
            .filter(|(_, counted)| !counted.is_disjoint(sessions))
            .map(|(ring, _)| ring.clone())
            .collect()
    }

    /// `limited` are the sessions of `ring` stopped by its limit now (those
    /// of accounts whose tracking is off left out).
    pub fn update(
        &mut self,
        ring: &RingId,
        limited: &[LimitedSession],
        ctx: &LimitContext,
    ) -> LimitChange {
        let now: BTreeSet<SessionId> = limited.iter().map(|session| session.id.clone()).collect();
        let before = self.counted.remove(ring).unwrap_or_default();
        if now.is_empty() {
            // Taking back a banner that was never posted does nothing.
            return LimitChange::Withdraw(tag(ToastKind::Limit).to_owned(), group(ring.as_str()));
        }
        let joined = now.difference(&before).next().is_some();
        self.counted.insert(ring.clone(), now);
        if !joined
            || !ctx.notify_needs_input
            || ctx.suppressed
            || ctx.permission != NotifyPermission::Allowed
        {
            return LimitChange::Nothing;
        }
        let titles: Vec<String> = limited
            .iter()
            .map(|session| collapse_whitespace(&session.title))
            .collect();
        LimitChange::Post(limit_toast(
            ring,
            ctx.account_label.as_deref(),
            &titles,
            ctx.limit_reset.as_deref(),
        ))
    }
}

// ---- When a limit lifts ----

/// "resets 14:05" today, "resets Thu 09:00" within the week, "resets 3 Oct"
/// later, in the clock `utc_offset_seconds` east of UTC (the user's);
/// `None` without a reset time ahead.
pub fn reset_phrase(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    utc_offset_seconds: i32,
) -> Option<String> {
    use chrono::{DateTime, FixedOffset, Utc};
    let resets_at = resets_at.filter(|at| *at > now)?;
    let offset = FixedOffset::east_opt(utc_offset_seconds)?;
    let local = |at: SystemTime| DateTime::<Utc>::from(at).with_timezone(&offset);
    let (reset, today) = (local(resets_at), local(now));
    let ahead = resets_at.duration_since(now).unwrap_or_default();
    let format = if reset.date_naive() == today.date_naive() {
        "%H:%M"
    } else if ahead < Duration::from_secs(6 * 24 * 60 * 60) {
        "%a %H:%M"
    } else {
        "%-d %b"
    };
    Some(format!("resets {}", reset.format(format)))
}

/// The user's clock at `at`, in seconds east of UTC.
pub fn local_utc_offset_seconds(at: SystemTime) -> i32 {
    use chrono::{DateTime, Local, Offset, Utc};
    DateTime::<Utc>::from(at)
        .with_timezone(&Local)
        .offset()
        .fix()
        .local_minus_utc()
}
