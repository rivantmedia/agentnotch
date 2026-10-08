//! What each banner says, how it is identified, what a click on it means and
//! when it is taken back (HS§7, design §4.10). Ported from
//! `A3_NotificationTests`, `ChatAndNotificationTests`,
//! `Fix_OffPanelPreviewTests` and the banner half of
//! `A3_ReviewTests.rowsAndBannersCarryNoPromptOrAssistantText`.
//!
//! Mapped, not ported one to one:
//! - A Mac identifier `agentnotch.<kind>.<sid>` is a (tag, group) pair here:
//!   the tag the kind (`needs`, `review`, `failed`, `limit`), the group the
//!   session or ring id, both at most 64 UTF-16 units.
//! - `NotificationRouting.response` is `parse_deep_link` over the toast's
//!   `launch_url` and action URLs.
//!
//! Skipped, with no Windows counterpart: UNNotification presentation options
//! (`presentationOptions(forIdentifier:)`: a toast is shown by the shell, and
//! its silence is the toast XML's `<audio silent="true"/>`, wp6-8), category
//! registration (`needsInputCategory` and the rest: a toast carries its
//! buttons itself), and the dismiss action (nothing is sent on dismissal).
//! Not here: the hover row of `theHoverRowAndBannerUseIt` is WP7's.

mod control_support;

use agentnotch_engine::control::notifications::*;
use agentnotch_engine::control::text::off_panel_preview;
use agentnotch_engine::core::time::{from_ms, to_ms};
use agentnotch_engine::model::*;
use agentnotch_engine::platform::{NotifyPermission, Toast, ToastKind};
use agentnotch_engine::runtime_types::ToastContext;
use chrono::{TimeZone, Utc};
use control_support::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

const ALL_KINDS: [ToastKind; 4] = [
    ToastKind::NeedsInput,
    ToastKind::Review,
    ToastKind::Failed,
    ToastKind::Limit,
];

fn sid(id: &str) -> SessionId {
    SessionId::from(id)
}

fn ring(id: &str) -> RingId {
    RingId::from(id)
}

fn permission_reason(tool: &str) -> NeedsInputReason {
    NeedsInputReason::Permission {
        tool: Some(tool.into()),
    }
}

fn error_reason(text: &str, code: Option<&str>) -> NeedsInputReason {
    NeedsInputReason::Error {
        text: text.into(),
        code: code.map(str::to_owned),
    }
}

/// `ChatQuestionTests.input`: two questions and an entry without a question
/// text, which is skipped.
fn question_input() -> Value {
    json!({"questions": [
        {"question": "Which database?", "header": "DB", "multiSelect": false,
         "options": [{"label": "Postgres", "description": "Relational"}, {"label": "SQLite"},
                     {"description": "no label, dropped"}]},
        {"question": "Which features?", "multiSelect": true,
         "options": [{"label": "Auth"}, {"label": "Billing"}, {"label": "Search"}]},
        {"header": "no question text, skipped"}
    ]})
}

/// The banner `needs_input_toast` makes for `tool` waiting on `input`.
fn needs_toast(tool: &str, input: Value) -> Toast {
    let view = waiting_on(view("sess-1"), tool, input);
    let reason = NeedsInputReason::for_approval(tool);
    needs_input_toast(&view, &reason, &toast_ctx(), None)
}

fn limited(ids: &[&str]) -> Vec<LimitedSession> {
    ids.iter()
        .map(|id| LimitedSession {
            id: sid(id),
            title: format!("Session {id}"),
            restored: false,
        })
        .collect()
}

fn limit_ctx() -> LimitContext {
    LimitContext {
        notify_needs_input: true,
        permission: NotifyPermission::Allowed,
        suppressed: false,
        account_label: Some("Work".into()),
        limit_reset: Some("resets 14:05".into()),
    }
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> SystemTime {
    SystemTime::from(Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap())
}

// ---- A3_NotificationPassThroughTests ----

/// `foreignNotificationsAreNotPresentedOrActedOn`: a tag that isn't one of
/// ours is nobody's of ours, and no link but ours means anything.
#[test]
fn foreign_notifications_are_not_acted_on() {
    for foreign in [
        "codenotch.threshold.claude.80",
        "limit-reached-openai",
        "",
        "agentnotch",
        "needs-input.sess-1",
        "review.sess-1",
        "needsInput",
        "Needs",
        "limit ",
    ] {
        assert_eq!(kind_of_tag(foreign), None, "{foreign:?}");
    }
    for link in [
        "",
        "codenotch://open?session=s-1",
        "https://open?session=s-1",
        "agentnotch-evil://open?session=s-1",
        "agentnotch:open?session=s-1",
        "agentnotch://auth-callback?code=abc&state=x",
        "agentnotch://settings",
        "agentnotch://answer?session=s-1",
        "agentnotch://approve?session=s-1&tool=toolu_1",
        "agentnotch://OPEN.example?session=s-1",
        "agentnotch://open",
        "agentnotch://open?",
        "agentnotch://open?session=",
        "agentnotch://review",
        "agentnotch://review?completed=1790000000000",
        "needs-input.sess-1",
        "review.sess-1",
    ] {
        assert_eq!(parse_deep_link(link), None, "{link:?}");
    }
    // Banners that aren't ours are never withdrawn.
    let delivered = vec![
        ("codenotch.threshold".to_owned(), "claude".to_owned()),
        ("".to_owned(), "s".to_owned()),
        ("needsInput".to_owned(), "gone".to_owned()),
    ];
    assert!(stale(&delivered, &[], None).is_empty());
}

/// `oursAreSilentBanners`: a toast has no field that makes a sound; its
/// silence is the XML's (wp6-8). Destructuring all fields means a new one,
/// a sound say, can't be added without this test being looked at.
#[test]
fn ours_carry_no_sound() {
    let view = waiting_on(view("s-1"), "Bash", json!({"command": "ls"}));
    let toasts = [
        needs_input_toast(&view, &permission_reason("Bash"), &toast_ctx(), None),
        review_toast(&view, &toast_ctx()),
        failed_toast(&view, &error_reason("Overloaded", None), &toast_ctx()),
        limit_toast(&ring(RING), None, &["A".to_owned()], None),
    ];
    for toast in toasts {
        let Toast {
            tag,
            group,
            kind: _,
            title,
            subtitle: _,
            body,
            launch_url,
            actions,
        } = toast;
        for text in [&tag, &group, &title, &body, &launch_url] {
            assert!(!text.to_lowercase().contains("sound"), "{text}");
        }
        assert!(actions.iter().all(|(label, _)| label != "Sound"));
    }
}

/// `clicksOnOursRoute`: the banner's own click and each button, as links.
#[test]
fn clicks_on_ours_route() {
    let mut review_view = in_state(view("s-1"), SessionState::ReadyForReview);
    review_view.completed_at = Some(from_ms(1_790_000_123_456));
    let review = review_toast(&review_view, &toast_ctx());
    assert_eq!(review.launch_url, "agentnotch://open?session=s-1");
    assert_eq!(
        review.actions,
        vec![
            (
                "Open".to_owned(),
                "agentnotch://open?session=s-1".to_owned()
            ),
            (
                "Mark Reviewed".to_owned(),
                "agentnotch://review?session=s-1&completed=1790000123456".to_owned()
            ),
        ]
    );
    assert_eq!(
        parse_deep_link(&review.launch_url),
        Some(DeepLinkAction::OpenSession(sid("s-1")))
    );
    assert_eq!(
        parse_deep_link(&review.actions[0].1),
        Some(DeepLinkAction::OpenSession(sid("s-1")))
    );
    assert_eq!(
        parse_deep_link(&review.actions[1].1),
        Some(DeepLinkAction::MarkReviewed {
            session: sid("s-1"),
            completed_at: Some(from_ms(1_790_000_123_456)),
        })
    );

    // Needs you and failed: Open alone, never an answer.
    let waiting = waiting_on(view("s-1"), "Bash", json!({"command": "ls"}));
    for toast in [
        needs_input_toast(&waiting, &permission_reason("Bash"), &toast_ctx(), None),
        failed_toast(&waiting, &error_reason("Overloaded", None), &toast_ctx()),
    ] {
        assert_eq!(toast.actions.len(), 1, "{:?}", toast.kind);
        assert_eq!(toast.actions[0].0, "Open");
        assert_eq!(toast.actions[0].1, toast.launch_url);
        assert_eq!(
            parse_deep_link(&toast.launch_url),
            Some(DeepLinkAction::OpenSession(sid("s-1")))
        );
    }

    // The limit banner opens the ring.
    let limit = limit_toast(&ring("claude-work"), None, &["A".to_owned()], None);
    assert_eq!(limit.launch_url, "agentnotch://open?ring=claude-work");
    assert_eq!(
        limit.actions,
        vec![("Open".to_owned(), limit.launch_url.clone())]
    );
    assert_eq!(
        parse_deep_link(&limit.launch_url),
        Some(DeepLinkAction::OpenRing(ring("claude-work")))
    );

    // A review without a completion time reviews "as of now".
    assert_eq!(
        parse_deep_link(&review_url(&sid("s-1"), None)),
        Some(DeepLinkAction::MarkReviewed {
            session: sid("s-1"),
            completed_at: None
        })
    );
    // Every kind has a tag of its own.
    for kind in ALL_KINDS {
        assert!(kind_of_tag(tag(kind)).is_some());
    }
}

/// `identifiersFollowTheDesign` for tags and groups.
#[test]
fn identifiers_follow_the_design() {
    assert_eq!(tag(ToastKind::NeedsInput), "needs");
    assert_eq!(tag(ToastKind::Review), "review");
    assert_eq!(tag(ToastKind::Failed), "failed");
    assert_eq!(tag(ToastKind::Limit), "limit");
    let tags: BTreeSet<&str> = ALL_KINDS.into_iter().map(tag).collect();
    assert_eq!(tags.len(), 4);
    for kind in ALL_KINDS {
        assert_eq!(kind_of_tag(tag(kind)), Some(kind));
    }
    // The group is the id itself, a dot or dash included.
    assert_eq!(group("a1"), "a1");
    assert_eq!(group("a1.b2"), "a1.b2");
    assert_eq!(
        group("claude-acct-1a2b3c4d5e6f"),
        "claude-acct-1a2b3c4d5e6f"
    );

    let view = in_state(view("a1"), SessionState::ReadyForReview);
    let toast = review_toast(&view, &toast_ctx());
    assert_eq!((toast.tag.as_str(), toast.group.as_str()), ("review", "a1"));
    let toast = needs_input_toast(&view, &permission_reason("Bash"), &toast_ctx(), None);
    assert_eq!((toast.tag.as_str(), toast.group.as_str()), ("needs", "a1"));
    let toast = failed_toast(&view, &error_reason("Overloaded", None), &toast_ctx());
    assert_eq!((toast.tag.as_str(), toast.group.as_str()), ("failed", "a1"));
    let toast = limit_toast(&ring("claude"), None, &["A".to_owned()], None);
    assert_eq!(
        (toast.tag.as_str(), toast.group.as_str()),
        ("limit", "claude")
    );
}

/// Windows limits a tag and a group to 64 units: a longer id maps
/// deterministically, and two long ids stay apart.
#[test]
fn long_identifiers_fit_and_stay_distinct() {
    let exact = "s".repeat(64);
    assert_eq!(group(&exact), exact);
    // 32 emoji are 64 UTF-16 units: they fit as they are.
    let emoji = "😀".repeat(32);
    assert_eq!(group(&emoji), emoji);

    let long = format!("{}-tail-a", "x".repeat(70));
    let other = format!("{}-tail-b", "x".repeat(70));
    let longer = format!("{long}!");
    let (g, h, i) = (group(&long), group(&other), group(&longer));
    for g in [&g, &h, &i] {
        assert!(units(g) <= MAX_IDENTIFIER_LENGTH, "{g}");
        assert!(g.starts_with("xxxx"));
    }
    assert_eq!(g, group(&long), "deterministic");
    assert_ne!(g, h, "differ only past the cut");
    assert_ne!(g, i);
    assert_ne!(h, i);
    // A 65-unit id is already too long.
    assert!(units(&group(&"s".repeat(65))) <= MAX_IDENTIFIER_LENGTH);
    assert_ne!(group(&"s".repeat(65)), "s".repeat(65));

    // The cut never lands inside a surrogate pair.
    let astral = format!("{}{}", "a".repeat(46), "😀".repeat(10));
    let cut = group(&astral);
    assert!(units(&cut) <= MAX_IDENTIFIER_LENGTH);
    assert!(cut.starts_with(&"a".repeat(46)));
    assert_ne!(
        cut,
        group(&format!("{}{}", "a".repeat(46), "😁".repeat(10)))
    );

    // A toast of a long id carries the short group, and a withdrawal and
    // `stale` find it again by the same mapping.
    let mut long_view = view(&long);
    long_view.state = SessionState::ReadyForReview;
    let toast = review_toast(&long_view, &toast_ctx());
    assert_eq!(toast.group, g);
    assert!(units(&toast.tag) <= MAX_IDENTIFIER_LENGTH);
    assert_eq!(
        withdrawals_for_gone(&sid(&long))[0],
        ("needs".to_owned(), g.clone())
    );
    let delivered = vec![(toast.tag, toast.group)];
    assert!(stale(&delivered, &[long_view.clone()], None).is_empty());
    assert_eq!(stale(&delivered, &[view(&other)], None), delivered);
    // The same for a ring.
    let long_ring = format!("claude-acct-{}", "9".repeat(80));
    let toast = limit_toast(&ring(&long_ring), None, &["A".to_owned()], None);
    assert!(units(&toast.group) <= MAX_IDENTIFIER_LENGTH);
    assert_eq!(toast.group, group(&long_ring));
    assert_eq!(
        toast.launch_url,
        format!("agentnotch://open?ring={long_ring}")
    );
}

// ---- Deep links ----

#[test]
fn deep_link_vectors() {
    use DeepLinkAction::*;
    for (link, expected) in [
        (
            "agentnotch://open?session=s-1",
            Some(OpenSession(sid("s-1"))),
        ),
        (
            "AgentNotch://Open?session=s-1",
            Some(OpenSession(sid("s-1"))),
        ),
        (
            "agentnotch://open/?session=s-1",
            Some(OpenSession(sid("s-1"))),
        ),
        (
            "  agentnotch://open?session=s-1\n",
            Some(OpenSession(sid("s-1"))),
        ),
        (
            "agentnotch://open?ring=claude-acct-1a2b",
            Some(OpenRing(ring("claude-acct-1a2b"))),
        ),
        (
            "agentnotch://review?session=s-1&completed=5",
            Some(MarkReviewed {
                session: sid("s-1"),
                completed_at: Some(from_ms(5)),
            }),
        ),
        // Missing parameters.
        ("agentnotch://open", None),
        ("agentnotch://open?completed=5", None),
        ("agentnotch://review?session=", None),
        // Extra or repeated parameters, a wrong place for one, a path,
        // user info, a port or a fragment: not a link we made.
        ("agentnotch://open?session=a&session=b", None),
        ("agentnotch://open?session=a&ring=b", None),
        ("agentnotch://open?session=a&completed=5", None),
        ("agentnotch://open?session=a&answer=allow", None),
        ("agentnotch://review?session=a&ring=b", None),
        (
            "agentnotch://review?session=a&completed=5&completed=6",
            None,
        ),
        ("agentnotch://open/extra?session=a", None),
        ("agentnotch://open?session=a#frag", None),
        ("agentnotch://user@open?session=a", None),
        ("agentnotch://open:80?session=a", None),
        // A completion time that isn't a time.
        ("agentnotch://review?session=a&completed=soon", None),
        ("agentnotch://review?session=a&completed=-1", None),
        (
            "agentnotch://review?session=a&completed=18446744073709551615",
            None,
        ),
        ("agentnotch://review?session=a&completed=", None),
        // Joined and split by the single-instance hand-over (`|`): neither
        // piece nor the whole is a link.
        (
            "agentnotch://open?session=a|agentnotch://open?session=b",
            None,
        ),
        ("agentnotch://open?session=a|b", None),
        ("agentnotch://open?session=a|", None),
        ("b", None),
        ("|agentnotch://open?session=a", None),
    ] {
        assert_eq!(parse_deep_link(link), expected, "{link:?}");
    }
    // An id with a `|` travels percent-encoded.
    let odd = sid("a|b c&d=e%f é");
    let link = open_session_url(&odd);
    assert!(!link.contains('|') && !link.contains(' '));
    assert_eq!(parse_deep_link(&link), Some(OpenSession(odd.clone())));
    let link = review_url(&odd, Some(from_ms(1_790_000_000_000)));
    assert_eq!(
        parse_deep_link(&link),
        Some(MarkReviewed {
            session: odd,
            completed_at: Some(from_ms(1_790_000_000_000))
        })
    );
    // Lengths: a link or an id past the limits is refused.
    assert!(parse_deep_link(&format!("agentnotch://open?session={}", "a".repeat(256))).is_some());
    assert_eq!(
        parse_deep_link(&format!("agentnotch://open?session={}", "a".repeat(257))),
        None
    );
    assert_eq!(
        parse_deep_link(&format!("agentnotch://open?session={}", "a".repeat(4000))),
        None
    );
}

/// Any id a banner holds survives its own link.
#[test]
fn links_round_trip_what_banners_make() {
    for id in [
        "sess-1",
        "a1.b2",
        "9f1c2e7a-3b4d-4e5f-8a9b-0c1d2e3f4a5b",
        "naïve ☃ 😀",
        "x".repeat(200).as_str(),
    ] {
        let mut v = in_state(view(id), SessionState::ReadyForReview);
        v.completed_at = Some(from_ms(1_790_000_000_001));
        let toast = review_toast(&v, &toast_ctx());
        assert_eq!(
            parse_deep_link(&toast.launch_url),
            Some(DeepLinkAction::OpenSession(sid(id)))
        );
        assert_eq!(
            parse_deep_link(&toast.actions[1].1),
            Some(DeepLinkAction::MarkReviewed {
                session: sid(id),
                completed_at: Some(from_ms(1_790_000_000_001))
            })
        );
        let limit = limit_toast(&ring(id), None, &["A".to_owned()], None);
        assert_eq!(
            parse_deep_link(&limit.launch_url),
            Some(DeepLinkAction::OpenRing(ring(id)))
        );
    }
}

#[test]
fn deep_link_gate_allows_ten_a_minute() {
    let mut gate = DeepLinkGate::default();
    let t = t0();
    for i in 0..10 {
        assert!(gate.allow(t + Duration::from_secs(i)), "link {i}");
    }
    // The eleventh and every attempt until the first one is a minute old.
    assert!(!gate.allow(t + Duration::from_secs(10)));
    assert!(!gate.allow(t + Duration::from_millis(59_999)));
    // A refused attempt doesn't count: at 60 s only the first has aged out.
    assert!(gate.allow(t + Duration::from_secs(60)));
    assert!(!gate.allow(t + Duration::from_secs(60)));
    // At 61 s the second has too.
    assert!(gate.allow(t + Duration::from_secs(61)));
    // Long after, everything is allowed again.
    let later = t + Duration::from_secs(3600);
    for _ in 0..10 {
        assert!(gate.allow(later));
    }
    assert!(!gate.allow(later));
    assert!(DeepLinkGate::default().allow(t));
}

// ---- A3_NotificationContentTests ----

#[test]
fn one_limit_banner_per_account() {
    let titles: Vec<String> = ["A", "B", "C"].map(str::to_owned).to_vec();
    let many = limit_toast(
        &ring("claude-work"),
        Some("Work"),
        &titles,
        Some("resets 14:05"),
    );
    assert_eq!(
        (many.tag.as_str(), many.group.as_str()),
        ("limit", "claude-work")
    );
    assert_eq!(many.kind, ToastKind::Limit);
    assert_eq!(many.title, "Work: 3 sessions hit the limit");
    assert_eq!(many.body, "Rate limited · resets 14:05");
    assert_eq!(many.subtitle, None);

    let one = limit_toast(
        &ring("claude"),
        None,
        &["Refactor the parser".to_owned()],
        None,
    );
    assert_eq!(one.title, "Refactor the parser hit the limit");
    assert!(one.body.starts_with("Rate limited"));
    assert_eq!(
        one.body,
        "Rate limited. Claude Code waits for you once it lifts."
    );
    // A long title is cut like any other.
    let long = limit_toast(&ring("claude"), None, &["w".repeat(100)], None);
    assert_eq!(long.title, format!("{}… hit the limit", "w".repeat(59)));
}

/// `LimitBanners`: one banner per ring, posted again only when a session
/// joins, withdrawn at zero.
#[test]
fn limit_banners_post_withdraw_and_count() {
    let (r, ctx) = (ring(RING), limit_ctx());
    let mut banners = LimitBanners::new();

    let LimitChange::Post(first) = banners.update(&r, &limited(&["a", "b"]), &ctx) else {
        panic!("a first banner is posted");
    };
    assert_eq!(first.title, "Work: 2 sessions hit the limit");
    assert_eq!((first.tag.as_str(), first.group.as_str()), ("limit", RING));
    // Nothing joined: no second banner.
    assert_eq!(
        banners.update(&r, &limited(&["b", "a"]), &ctx),
        LimitChange::Nothing
    );
    // One joins: the same banner again, with the new count.
    let LimitChange::Post(more) = banners.update(&r, &limited(&["a", "b", "c"]), &ctx) else {
        panic!("a joining session re-posts");
    };
    assert_eq!(more.title, "Work: 3 sessions hit the limit");
    assert_eq!((more.tag, more.group), (first.tag, first.group));
    // A shrinking count is not shown again; the next join is judged against it.
    assert_eq!(
        banners.update(&r, &limited(&["a"]), &ctx),
        LimitChange::Nothing
    );
    let LimitChange::Post(single) = banners.update(&r, &limited(&["a", "d"]), &ctx) else {
        panic!("d joined");
    };
    assert_eq!(single.title, "Work: 2 sessions hit the limit");
    // Zero withdraws, and a later limit starts afresh.
    assert_eq!(
        banners.update(&r, &[], &ctx),
        LimitChange::Withdraw("limit".into(), RING.into())
    );
    assert!(matches!(
        banners.update(&r, &limited(&["a"]), &ctx),
        LimitChange::Post(_)
    ));
    // Another ring is counted on its own.
    let other = ring("claude-acct-ffffffffffff");
    assert!(matches!(
        banners.update(&other, &limited(&["a"]), &ctx),
        LimitChange::Post(toast) if toast.group == "claude-acct-ffffffffffff"
    ));
    // Withdrawing a ring that never had one is harmless.
    assert_eq!(
        LimitBanners::new().update(&r, &[], &ctx),
        LimitChange::Withdraw("limit".into(), RING.into())
    );
}

#[test]
fn limit_banners_follow_the_switches_but_keep_counting() {
    for (name, ctx) in [
        (
            "needs-input off",
            LimitContext {
                notify_needs_input: false,
                ..limit_ctx()
            },
        ),
        (
            "suppressed",
            LimitContext {
                suppressed: true,
                ..limit_ctx()
            },
        ),
        (
            "not allowed",
            LimitContext {
                permission: NotifyPermission::DisabledForUser,
                ..limit_ctx()
            },
        ),
    ] {
        let mut banners = LimitBanners::new();
        let r = ring(RING);
        assert_eq!(
            banners.update(&r, &limited(&["a"]), &ctx),
            LimitChange::Nothing,
            "{name}"
        );
        // Switched back on with the same sessions: they were counted, so
        // nothing is announced late.
        assert_eq!(
            banners.update(&r, &limited(&["a"]), &limit_ctx()),
            LimitChange::Nothing,
            "{name}"
        );
        // Zero still takes the banner back whatever the switches say.
        assert!(
            matches!(banners.update(&r, &[], &ctx), LimitChange::Withdraw(..)),
            "{name}"
        );
    }
    // Several sessions, long titles, line breaks: collapsed like a title.
    let mut banners = LimitBanners::new();
    let one = vec![LimitedSession {
        id: sid("a"),
        title: "Two\n  lines ".into(),
        restored: false,
    }];
    let LimitChange::Post(toast) = banners.update(&ring(RING), &one, &limit_ctx()) else {
        panic!()
    };
    assert_eq!(toast.title, "Work: Two lines hit the limit");
    let no_label = LimitContext {
        account_label: None,
        limit_reset: None,
        ..limit_ctx()
    };
    let LimitChange::Post(toast) = LimitBanners::new().update(&ring(RING), &one, &no_label) else {
        panic!()
    };
    assert_eq!(toast.title, "Two lines hit the limit");
    assert_eq!(
        toast.body,
        "Rate limited. Claude Code waits for you once it lifts."
    );
}

#[test]
fn limit_banners_concern_rate_limit_transitions() {
    let limit = failed("Rate limited", "rate_limit");
    let overloaded = failed("Overloaded", "overloaded");
    let from_work =
        |state: SessionState| transition(in_state(view("s"), state), Some(SessionState::Working));
    // Became rate limited.
    assert!(LimitBanners::concerns(&from_work(limit.clone())));
    assert!(LimitBanners::concerns(&transition(
        in_state(view("s"), limit.clone()),
        None
    )));
    // Some other wait or failure.
    assert!(!LimitBanners::concerns(&from_work(overloaded.clone())));
    assert!(!LimitBanners::concerns(&from_work(permission("Bash"))));
    assert!(!LimitBanners::concerns(&from_work(
        SessionState::ReadyForReview
    )));
    // Stopped being rate limited: resumed, answered, or a different failure.
    assert!(LimitBanners::concerns(&transition(
        in_state(view("s"), SessionState::Working),
        Some(limit.clone())
    )));
    assert!(LimitBanners::concerns(&transition(
        in_state(view("s"), overloaded),
        Some(limit.clone())
    )));
    // Still rate limited, nothing new.
    assert!(!LimitBanners::concerns(&transition(
        in_state(view("s"), limit.clone()),
        Some(limit.clone())
    )));

    // A session that ended takes its ring's banner count with it.
    let mut banners = LimitBanners::new();
    banners.update(&ring(RING), &limited(&["a", "b"]), &limit_ctx());
    banners.update(&ring("other"), &limited(&["c"]), &limit_ctx());
    let gone: BTreeSet<SessionId> = [sid("b"), sid("zzz")].into();
    assert_eq!(banners.rings_counting(&gone), vec![ring(RING)]);
    assert!(banners.rings_counting(&BTreeSet::new()).is_empty());
}

/// `rateLimitedBodyNamesTheReset`.
#[test]
fn rate_limited_body_names_the_reset() {
    let v = view("s");
    let limit = error_reason("Rate limited", Some("rate_limit"));
    let toast = needs_input_toast(&v, &limit, &toast_ctx(), Some("resets Thu 09:00"));
    assert_eq!(toast.body, "Rate limited · resets Thu 09:00");
    // Found by its text when the code was lost (a restored failure).
    let restored = error_reason("Rate limited", None);
    assert_eq!(
        needs_input_body(&v, &restored, Some("resets Thu 09:00")),
        "Rate limited · resets Thu 09:00"
    );
    assert_eq!(needs_input_body(&v, &limit, None), "Rate limited");
    // Other errors never carry a reset.
    for other in [
        error_reason("Overloaded", Some("overloaded")),
        error_reason("Overloaded", None),
        error_reason("Turn failed", Some("whatever")),
    ] {
        assert_eq!(
            needs_input_body(&v, &other, Some("resets Thu 09:00")),
            other.display_text()
        );
    }
}

/// `reset_phrase`, in a clock the test fixes (the Mac's test fixes the
/// calendar the same way).
#[test]
fn reset_phrases_in_a_fixed_clock() {
    // 2026-10-01 is a Thursday.
    let now = utc(2026, 10, 1, 9, 0);
    let phrase = |at: Option<SystemTime>, offset: i32| reset_phrase(at, now, offset);
    assert_eq!(
        phrase(Some(utc(2026, 10, 1, 14, 5)), 0).as_deref(),
        Some("resets 14:05")
    );
    // The clock east of UTC moves both ends: still the same local day.
    assert_eq!(
        phrase(Some(utc(2026, 10, 1, 12, 5)), 2 * 3600).as_deref(),
        Some("resets 14:05")
    );
    // 23:00 UTC is already tomorrow at +1 h.
    assert_eq!(
        phrase(Some(utc(2026, 10, 1, 23, 0)), 3600).as_deref(),
        Some("resets Fri 00:00")
    );
    // West of UTC, 02:00 UTC tomorrow is still this evening.
    assert_eq!(
        phrase(Some(utc(2026, 10, 2, 2, 0)), -5 * 3600).as_deref(),
        Some("resets 21:00")
    );
    // Within the week: the weekday.
    assert_eq!(
        phrase(Some(utc(2026, 10, 2, 9, 0)), 0).as_deref(),
        Some("resets Fri 09:00")
    );
    assert_eq!(
        phrase(Some(utc(2026, 10, 5, 7, 30)), 0).as_deref(),
        Some("resets Mon 07:30")
    );
    assert_eq!(
        phrase(Some(utc(2026, 10, 7, 8, 59)), 0).as_deref(),
        Some("resets Wed 08:59")
    );
    // Six days or more: the date.
    assert_eq!(
        phrase(Some(utc(2026, 10, 7, 9, 0)), 0).as_deref(),
        Some("resets 7 Oct")
    );
    assert_eq!(
        phrase(Some(utc(2026, 10, 20, 12, 0)), 0).as_deref(),
        Some("resets 20 Oct")
    );
    assert_eq!(
        phrase(Some(utc(2026, 11, 3, 12, 0)), 0).as_deref(),
        Some("resets 3 Nov")
    );
    // Nothing ahead: no phrase.
    assert_eq!(phrase(None, 0), None);
    assert_eq!(phrase(Some(now), 0), None);
    assert_eq!(phrase(Some(utc(2026, 10, 1, 8, 59)), 0), None);
    // An offset no clock has.
    assert_eq!(phrase(Some(utc(2026, 10, 1, 14, 5)), 48 * 3600), None);
    // The user's own offset is a real one.
    let offset = local_utc_offset_seconds(now);
    assert!(offset.abs() <= 14 * 3600 && offset % 60 == 0, "{offset}");
    assert!(reset_phrase(Some(utc(2026, 10, 1, 14, 5)), now, offset).is_some());
}

/// `collidingAccountLabelsAreToldApart`.
#[test]
fn colliding_account_labels_are_told_apart() {
    let labels = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(id, label)| ((*id).to_owned(), (*label).to_owned()))
            .collect()
    };
    let same = labels(&[("personal", "me@x.dev"), ("team", "me@x.dev")]);
    assert_eq!(
        account_subtitle(
            "team",
            "me@x.dev",
            Some("Acme"),
            Some("Team"),
            ".claude-team",
            &same
        )
        .as_deref(),
        Some("me@x.dev · Acme")
    );
    assert_eq!(
        account_subtitle("personal", "me@x.dev", None, Some("Max"), ".claude", &same).as_deref(),
        Some("me@x.dev · Max")
    );
    // Neither organization nor plan: the folder.
    assert_eq!(
        account_subtitle("personal", "me@x.dev", None, None, ".claude", &same).as_deref(),
        Some("me@x.dev · .claude")
    );
    assert_eq!(
        account_subtitle("personal", "me@x.dev", Some(""), None, ".claude", &same).as_deref(),
        Some("me@x.dev · .claude")
    );
    // The same name in another case collides too.
    let cased = labels(&[("a", "Me@X.dev"), ("b", "me@x.dev")]);
    assert_eq!(
        account_subtitle("a", "Me@X.dev", Some("Acme"), None, "f", &cased).as_deref(),
        Some("Me@X.dev · Acme")
    );
    // Distinct names: the name alone; one account (or none): nothing.
    let distinct = labels(&[("personal", "Personal"), ("team", "Work")]);
    assert_eq!(
        account_subtitle("team", "Work", Some("Acme"), None, "f", &distinct).as_deref(),
        Some("Work")
    );
    assert_eq!(
        account_subtitle(
            "team",
            "Work",
            None,
            None,
            "f",
            &labels(&[("team", "Work")])
        ),
        None
    );
    assert_eq!(
        account_subtitle("team", "Work", None, None, "f", &BTreeMap::new()),
        None
    );
}

/// A banner's subtitle is the account's name, with several accounts only.
#[test]
fn the_subtitle_needs_several_accounts() {
    let v = in_state(view("s"), SessionState::ReadyForReview);
    let mut ctx = toast_ctx();
    ctx.account_label = Some("work".into());
    assert_eq!(review_toast(&v, &ctx).subtitle, None);
    ctx.multi_account = true;
    assert_eq!(review_toast(&v, &ctx).subtitle.as_deref(), Some("work"));
    ctx.account_label = Some("  ".into());
    assert_eq!(review_toast(&v, &ctx).subtitle, None);
    ctx.account_label = None;
    assert_eq!(review_toast(&v, &ctx).subtitle, None);
}

// ---- SessionNotificationContentTests ----

#[test]
fn permission_shows_tool_and_input() {
    let v = waiting_on(
        view("sess-1"),
        "Bash",
        json!({"command": "npm   run\n build"}),
    );
    let mut ctx = toast_ctx();
    ctx.multi_account = true;
    ctx.account_label = Some("work".into());
    let toast = needs_input_toast(&v, &permission_reason("Bash"), &ctx, None);
    assert_eq!(toast.title, "Refactor the parser needs you");
    assert_eq!(toast.subtitle.as_deref(), Some("work"));
    assert_eq!(toast.body, "Approve Bash: npm run build");
    assert_eq!(
        (toast.tag.as_str(), toast.group.as_str()),
        ("needs", "sess-1")
    );
    assert_eq!(toast.kind, ToastKind::NeedsInput);
}

#[test]
fn permission_from_notification_has_no_input() {
    let toast = needs_input_toast(
        &view("sess-1"),
        &permission_reason("Edit"),
        &toast_ctx(),
        None,
    );
    assert_eq!(toast.body, "Approve Edit");
    assert_eq!(toast.subtitle, None);
    // No tool named at all.
    let nameless = NeedsInputReason::Permission { tool: None };
    assert_eq!(
        needs_input_body(&view("s"), &nameless, None),
        "Needs permission"
    );
    // An MCP tool by its friendly name.
    assert_eq!(
        needs_input_body(
            &view("s"),
            &permission_reason("mcp__github__list_issues"),
            None
        ),
        "Approve Github - List Issues"
    );
}

/// Banners never quote Claude: a question by its header, a plan unread.
#[test]
fn question_shows_its_header_not_its_text() {
    let v = waiting_on(view("sess-1"), "AskUserQuestion", question_input());
    let toast = needs_input_toast(&v, &NeedsInputReason::Question, &toast_ctx(), None);
    assert_eq!(toast.body, "Question · DB (+1 more)");
    assert!(!toast.body.contains("database") && !toast.body.contains("Postgres"));

    let one = waiting_on(
        view("s"),
        "AskUserQuestion",
        json!({"questions": [{"question": "Which?", "header": "  A header that is much too long to be a label  "}]}),
    );
    assert_eq!(
        needs_input_body(&one, &NeedsInputReason::Question, None),
        "Question · A header that is much t…"
    );
    let headerless = waiting_on(
        view("s"),
        "AskUserQuestion",
        json!({"questions": [{"question": "A"}, {"question": "B"}, {"question": "C"}]}),
    );
    assert_eq!(
        needs_input_body(&headerless, &NeedsInputReason::Question, None),
        "Question for you (+2 more)"
    );
    // No parsable question: the reason's own text.
    for input in [
        json!({}),
        json!({"questions": []}),
        json!({"questions": [{"header": "H"}]}),
    ] {
        let v = waiting_on(view("s"), "AskUserQuestion", input);
        assert_eq!(
            needs_input_body(&v, &NeedsInputReason::Question, None),
            "Question for you"
        );
    }
    assert_eq!(
        needs_input_body(&view("s"), &NeedsInputReason::Question, None),
        "Question for you"
    );
}

#[test]
fn plan_is_not_quoted() {
    let v = waiting_on(
        view("sess-1"),
        "ExitPlanMode",
        json!({"plan": "\n## Plan: ship rings\n1. Do it"}),
    );
    let toast = needs_input_toast(&v, &NeedsInputReason::PlanApproval, &toast_ctx(), None);
    assert_eq!(toast.body, "Plan ready for approval");
}

#[test]
fn errors_and_dialogs_use_the_reason() {
    let v = view("sess-1");
    let error = needs_input_toast(
        &v,
        &error_reason("Rate limited", Some("rate_limit")),
        &toast_ctx(),
        None,
    );
    assert_eq!(error.body, "Rate limited");
    let dialog = NeedsInputReason::Dialog {
        detail: "permission prompt".into(),
    };
    assert_eq!(
        needs_input_toast(&v, &dialog, &toast_ctx(), None).body,
        "Permission prompt"
    );
    let elicitation = NeedsInputReason::Elicitation {
        message: "Enter the API key name".into(),
    };
    assert_eq!(
        needs_input_body(&v, &elicitation, None),
        "Enter the API key name"
    );
    assert_eq!(
        needs_input_body(
            &v,
            &NeedsInputReason::Elicitation {
                message: String::new()
            },
            None
        ),
        "Input requested"
    );
    assert_eq!(
        needs_input_body(
            &v,
            &NeedsInputReason::Dialog {
                detail: String::new()
            },
            None
        ),
        "Waiting for you"
    );
}

#[test]
fn review_names_the_project_not_the_last_message() {
    let mut v = in_state(view("sess-1"), SessionState::ReadyForReview);
    v.last_assistant_message =
        Some("All tests pass.\n\nI split the parser into   three files.".into());
    let toast = review_toast(&v, &toast_ctx());
    assert_eq!(toast.title, "Done: Refactor the parser");
    assert_eq!(toast.body, "Ready for review · app");
    assert_eq!(
        (toast.tag.as_str(), toast.group.as_str()),
        ("review", "sess-1")
    );
    assert_eq!(toast.kind, ToastKind::Review);
}

#[test]
fn review_mentions_background_tasks() {
    let mut v = in_state(view("sess-1"), SessionState::ReadyForReview);
    v.background.task_count = 2;
    assert_eq!(
        review_toast(&v, &toast_ctx()).body,
        "Ready for review · app · 2 background tasks running"
    );
    v.background.task_count = 1;
    v.last_assistant_message = Some("x".repeat(400));
    assert_eq!(
        review_toast(&v, &toast_ctx()).body,
        "Ready for review · app · 1 background task running"
    );
}

#[test]
fn long_titles_are_truncated() {
    let mut ctx = toast_ctx();
    ctx.title = "word ".repeat(30);
    let v = in_state(view("sess-1"), SessionState::ReadyForReview);
    let toast = review_toast(&v, &ctx);
    assert!(toast.title.chars().count() <= "Done: ".chars().count() + MAX_TITLE_LENGTH);
    assert!(toast.title.ends_with('…'));
    let needs = needs_input_toast(&v, &NeedsInputReason::Question, &ctx, None);
    assert!(needs.title.ends_with("… needs you"), "{}", needs.title);
    assert_eq!(
        needs.title.chars().count(),
        MAX_TITLE_LENGTH + " needs you".chars().count()
    );
    // A title of exactly the limit is kept; one more is cut.
    ctx.title = "t".repeat(60);
    assert_eq!(
        review_toast(&v, &ctx).title,
        format!("Done: {}", "t".repeat(60))
    );
    ctx.title = "t".repeat(61);
    assert_eq!(
        review_toast(&v, &ctx).title,
        format!("Done: {}…", "t".repeat(59))
    );
    // Counted in characters, not bytes or units: 60 family emoji fit.
    let family = "👨‍👩‍👧‍👦";
    ctx.title = family.repeat(60);
    assert_eq!(
        review_toast(&v, &ctx).title,
        format!("Done: {}", family.repeat(60))
    );
    ctx.title = family.repeat(61);
    assert_eq!(
        review_toast(&v, &ctx).title,
        format!("Done: {}…", family.repeat(59))
    );
}

#[test]
fn long_bodies_are_truncated() {
    let long = NeedsInputReason::Dialog {
        detail: "d".repeat(500),
    };
    let body = needs_input_body(&view("s"), &long, None);
    assert_eq!(body.chars().count(), MAX_BODY_LENGTH);
    assert!(body.ends_with('…'));
    // A 300-character project in a review body.
    let mut ctx = toast_ctx();
    ctx.project = "p".repeat(300);
    let v = in_state(view("s"), SessionState::ReadyForReview);
    let toast = review_toast(&v, &ctx);
    assert_eq!(toast.body.chars().count(), MAX_BODY_LENGTH);
    assert!(toast.body.starts_with("Ready for review · ppp") && toast.body.ends_with('…'));
    // A permission preview is cut at 100 characters, then "...".
    let command = "x".repeat(150);
    let v = waiting_on(view("s"), "Bash", json!({ "command": command }));
    assert_eq!(
        needs_input_body(&v, &permission_reason("Bash"), None),
        format!("Approve Bash: {}...", "x".repeat(100))
    );
}

/// `identifiersRoundTrip`: tag and kind, and the session behind a group.
#[test]
fn identifiers_round_trip() {
    for kind in ALL_KINDS {
        assert_eq!(kind_of_tag(tag(kind)), Some(kind));
    }
    for id in ["a1b2.c3", "sess-1", &"y".repeat(90)] {
        let v = in_state(view(id), SessionState::ReadyForReview);
        for toast in [
            needs_input_toast(&v, &NeedsInputReason::Question, &toast_ctx(), None),
            review_toast(&v, &toast_ctx()),
            failed_toast(&v, &error_reason("Overloaded", None), &toast_ctx()),
        ] {
            assert_eq!(kind_of_tag(&toast.tag), Some(toast.kind));
            assert_eq!(toast.group, group(id));
        }
    }
    assert_eq!(kind_of_tag("review."), None);
    assert_eq!(kind_of_tag("other.sess-1"), None);
    assert_eq!(kind_of_tag("no-dot"), None);
}

/// `staleNotificationsAreDetected`: `still_applies` for the three session
/// kinds, then `stale` over what is still delivered.
#[test]
fn stale_banners_are_detected() {
    let needs = SessionState::NeedsYou(NeedsInputReason::Question);
    let rate = failed("Rate limited", "rate_limit");
    assert!(still_applies(ToastKind::NeedsInput, Some(&needs)));
    assert!(still_applies(ToastKind::NeedsInput, Some(&rate)));
    assert!(!still_applies(
        ToastKind::NeedsInput,
        Some(&SessionState::Working)
    ));
    assert!(!still_applies(ToastKind::NeedsInput, None));
    assert!(still_applies(
        ToastKind::Review,
        Some(&SessionState::ReadyForReview)
    ));
    assert!(!still_applies(ToastKind::Review, Some(&SessionState::Idle)));
    assert!(!still_applies(
        ToastKind::Review,
        Some(&SessionState::NeedsYou(NeedsInputReason::PlanApproval))
    ));
    assert!(!still_applies(ToastKind::Review, None));
    assert!(still_applies(ToastKind::Failed, Some(&rate)));
    assert!(!still_applies(ToastKind::Failed, Some(&needs)));
    assert!(!still_applies(ToastKind::Failed, None));
    // A limit banner is judged by its ring, never by one session.
    assert!(!still_applies(ToastKind::Limit, Some(&rate)));

    let delivered = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(t, g)| ((*t).to_owned(), (*g).to_owned()))
            .collect()
    };
    let sessions = vec![
        in_state(view("asks"), needs),
        in_state(view("done"), SessionState::ReadyForReview),
        in_state(view("busy"), SessionState::Working),
        in_state(view("limited"), rate.clone()),
        {
            // No ring yet: it counts on the default one.
            let mut v = in_state(view("nameless"), failed("Rate limited", "rate_limit"));
            v.ring = None;
            v
        },
    ];
    let all = delivered(&[
        ("needs", "asks"),
        ("needs", "busy"),
        ("needs", "gone"),
        ("review", "done"),
        ("review", "asks"),
        ("failed", "limited"),
        ("failed", "asks"),
        ("limit", RING),
        ("limit", "claude-acct-aaaaaaaaaaaa"),
        ("limit", "default-ring"),
        ("codenotch", "done"),
    ]);
    let default = ring("default-ring");
    assert_eq!(
        stale(&all, &sessions, Some(&default)),
        delivered(&[
            ("needs", "busy"),
            ("needs", "gone"),
            ("review", "asks"),
            ("failed", "asks"),
            ("limit", "claude-acct-aaaaaaaaaaaa"),
        ])
    );
    // Without a default ring the nameless session counts for none.
    assert!(stale(&delivered(&[("limit", "default-ring")]), &sessions, None).len() == 1);
    // Nothing delivered, nothing stale; nothing running, everything of ours is.
    assert!(stale(&[], &sessions, None).is_empty());
    assert_eq!(stale(&all, &[], None).len(), all.len() - 1);
    assert_eq!(LAUNCH_RECONCILE_DELAY, Duration::from_secs(10));
}

#[test]
fn helpers() {
    use agentnotch_engine::control::text::{preview, truncated};
    assert_eq!(truncated("abcdef", 4), "abc…");
    assert_eq!(truncated("abc", 4), "abc");
    assert_eq!(preview(Some("  \n ")), None);
    assert_eq!(MAX_TITLE_LENGTH, 60);
    assert_eq!(MAX_BODY_LENGTH, 220);
    assert_eq!(MAX_IDENTIFIER_LENGTH, 64);
}

// ---- Fix_OffPanelPreviewTests ----

#[test]
fn named_fields_only() {
    let preview = |tool: &str, input: Value| off_panel_preview(tool, &input, None);
    assert_eq!(
        preview("Bash", json!({"command": "npm test"})).as_deref(),
        Some("npm test")
    );
    assert_eq!(
        preview("Edit", json!({"file_path": "/Users/x/p/auth.ts"})).as_deref(),
        Some("auth.ts")
    );
    assert_eq!(
        preview("Edit", json!({"file_path": r"C:\Users\x\p\auth.ts"})).as_deref(),
        Some("auth.ts")
    );
    assert_eq!(
        preview("Grep", json!({"pattern": "TODO"})).as_deref(),
        Some("TODO")
    );
    assert_eq!(
        preview("WebFetch", json!({"url": "https://example.com/a?token=x"})).as_deref(),
        Some("example.com")
    );
    assert_eq!(
        preview("WebSearch", json!({"query": "swift testing"})).as_deref(),
        Some("swift testing")
    );
    // Empty or missing: nothing.
    assert_eq!(preview("Bash", json!({"command": ""})), None);
    assert_eq!(preview("Bash", json!({})), None);
    assert_eq!(preview("WebFetch", json!({"url": "not a url"})), None);
}

#[test]
fn mcp_and_unknown_tools_show_their_name_alone() {
    let email = json!({"to": "boss@x.com", "subject": "Quarterly", "body": "Here is what Claude wrote for you"});
    assert_eq!(
        off_panel_preview("mcp__gmail__send_email", &email, None),
        None
    );
    assert_eq!(
        off_panel_preview(
            "Task",
            &json!({"description": "Explore", "prompt": "secret"}),
            None
        ),
        None
    );
    // The banner then names the tool alone.
    let toast = needs_toast("mcp__gmail__send_email", email);
    assert_eq!(toast.body, "Approve Gmail - Send Email");
    let toast = needs_toast(
        "Task",
        json!({"description": "Explore", "prompt": "secret"}),
    );
    assert_eq!(toast.body, "Approve Task");
}

/// `theHoverRowAndBannerUseIt`, the banner half.
#[test]
fn the_banner_uses_it() {
    let tool = "mcp__slack__post_message";
    let v = waiting_on(
        view("s"),
        tool,
        json!({"channel": "#general", "text": "Claude's draft"}),
    );
    let toast = needs_input_toast(&v, &permission_reason(tool), &toast_ctx(), None);
    assert!(
        !toast.body.contains("draft") && !toast.body.contains("general"),
        "{}",
        toast.body
    );
    assert_eq!(toast.body, "Approve Slack - Post Message");
    // A named field still shows: the file's name, not its folder.
    let edit = needs_toast(
        "Edit",
        json!({"file_path": r"C:\Users\x\secret-project\auth.ts"}),
    );
    assert_eq!(edit.body, "Approve Edit: auth.ts");
    let fetch = needs_toast("WebFetch", json!({"url": "https://example.com/a?token=x"}));
    assert_eq!(fetch.body, "Approve Fetch: example.com");
    // A request in the queue, not yet the session's phase, is read too.
    let mut queued = view("s");
    queued.pending = vec![request("s", "Bash", json!({"command": "ls -la"}), &[])];
    assert_eq!(
        needs_input_body(&queued, &permission_reason("Bash"), None),
        "Approve Bash: ls -la"
    );
}

// ---- A3_PublicTextTests (the banner half) ----

#[test]
fn banners_carry_no_prompt_or_assistant_text() {
    let mut v = view("s1");
    v.title = "SECRET-PROMPT please fix my login".into();
    v.last_assistant_message = Some("SECRET-ASSISTANT all done".into());
    v.cwd = "C:\\Users\\me\\code\\acme-web".into();
    v.project_name = "acme-web".into();
    let mut ctx = toast_ctx();
    ctx.title = "acme-web".into();
    ctx.project = "acme-web".into();

    let review = review_toast(&in_state(v.clone(), SessionState::ReadyForReview), &ctx);
    let asks = needs_input_toast(&v, &NeedsInputReason::Question, &ctx, None);
    let failed_banner = failed_toast(&v, &error_reason("Overloaded", None), &ctx);
    let limit = limit_toast(&ring("claude"), None, &["acme-web".to_owned()], None);
    for toast in [&review, &asks, &failed_banner, &limit] {
        for text in [&toast.title, &toast.body] {
            assert!(!text.contains("SECRET"), "{text}");
        }
    }
    assert_eq!(review.title, "Done: acme-web");
    assert_eq!(review.body, "Ready for review · acme-web");

    // Without a public title the project names the session; never the row's
    // own title (the first prompt, until Claude Code names the session).
    ctx.title = String::new();
    assert_eq!(review_toast(&v, &ctx).title, "Done: acme-web");
    ctx.title = "  Two\n lines ".into();
    assert_eq!(review_toast(&v, &ctx).title, "Done: Two lines");
    ctx.title = " ".into();
    ctx.project = String::new();
    assert_eq!(review_toast(&v, &ctx).body, "Ready for review · acme-web");
    assert_eq!(
        needs_input_toast(&v, &NeedsInputReason::Question, &ctx, None).title,
        "acme-web needs you"
    );
    v.project_name = String::new();
    assert_eq!(review_toast(&v, &ctx).title, "Done: Claude Code");
    assert_eq!(review_toast(&v, &ctx).body, "Ready for review");
}

// ---- Suppression, withdrawal ----

/// A session newly needing the user, ready for review, or failed.
fn arriving(state: SessionState) -> AttentionTransition {
    let v = match &state {
        SessionState::NeedsYou(NeedsInputReason::Permission { tool: Some(tool) }) => {
            waiting_on(view("s1"), tool, json!({"command": "ls"}))
        }
        _ => in_state(view("s1"), state),
    };
    transition(v, Some(SessionState::Working))
}

#[test]
fn toasts_for_transitions() {
    let ctx = toast_ctx();
    let needs = toast_for(&arriving(permission("Bash")), &ctx).expect("a needs-you banner");
    assert_eq!(
        (needs.tag.as_str(), needs.kind),
        ("needs", ToastKind::NeedsInput)
    );
    assert_eq!(needs.body, "Approve Bash: ls");
    let review = toast_for(&arriving(SessionState::ReadyForReview), &ctx).expect("a review banner");
    assert_eq!(
        (review.tag.as_str(), review.kind),
        ("review", ToastKind::Review)
    );
    // A failed turn gets its own banner, with what to do.
    let failure =
        toast_for(&arriving(failed("Overloaded", "overloaded")), &ctx).expect("a failure banner");
    assert_eq!(
        (failure.tag.as_str(), failure.kind),
        ("failed", ToastKind::Failed)
    );
    assert_eq!(failure.title, "Refactor the parser stopped");
    assert_eq!(failure.body, "Overloaded · retry in its terminal");
    let sign_in = toast_for(
        &arriving(failed("Sign-in failed", "authentication_failed")),
        &ctx,
    )
    .unwrap();
    assert_eq!(sign_in.body, "Sign-in failed · run /login in its terminal");
    // The same failure as a needs-you reason is still a failure.
    let as_needs = arriving(SessionState::NeedsYou(error_reason(
        "Billing problem",
        Some("billing_error"),
    )));
    assert_eq!(toast_for(&as_needs, &ctx).unwrap().tag, "failed");
    // Usage limits share one banner per account: none per session.
    assert_eq!(
        toast_for(&arriving(failed("Rate limited", "rate_limit")), &ctx),
        None
    );
    // Nothing new, nothing to say.
    assert_eq!(toast_for(&arriving(SessionState::Working), &ctx), None);
    assert_eq!(toast_for(&arriving(SessionState::Idle), &ctx), None);
    let again = transition(
        waiting_on(view("s1"), "Bash", json!({"command": "ls"})),
        Some(permission("Bash")),
    );
    assert_eq!(toast_for(&again, &ctx), None, "the same wait again");
    let reviewed_again = transition(
        in_state(view("s1"), SessionState::ReadyForReview),
        Some(SessionState::ReadyForReview),
    );
    assert_eq!(toast_for(&reviewed_again, &ctx), None);
    // The banner a session first appears with.
    let first = transition(in_state(view("s1"), SessionState::ReadyForReview), None);
    assert!(toast_for(&first, &ctx).is_some());
}

#[test]
fn suppression_goes_through_the_context() {
    let needs = arriving(permission("Bash"));
    let review = arriving(SessionState::ReadyForReview);
    let failure = arriving(failed("Overloaded", "overloaded"));
    let all = [&needs, &review, &failure];
    let check = |ctx: &ToastContext| -> Vec<bool> {
        all.iter().map(|tr| toast_for(tr, ctx).is_some()).collect()
    };
    assert_eq!(check(&toast_ctx()), [true, true, true]);

    let mut ctx = toast_ctx();
    ctx.suppressed = true;
    assert_eq!(
        check(&ctx),
        [false, false, false],
        "sealed, switched off or full screen"
    );

    for permission in [
        NotifyPermission::DisabledForApp,
        NotifyPermission::DisabledForUser,
        NotifyPermission::DisabledByPolicy,
        NotifyPermission::Unavailable,
    ] {
        let ctx = ToastContext {
            permission,
            ..toast_ctx()
        };
        assert_eq!(check(&ctx), [false, false, false], "{permission:?}");
    }

    // Looking at that session's own terminal: none; looking elsewhere or
    // not knowing: shown.
    for (looking_at, expected) in [(Some(true), false), (Some(false), true), (None, true)] {
        let ctx = ToastContext {
            looking_at,
            ..toast_ctx()
        };
        assert_eq!(check(&ctx), [expected; 3], "{looking_at:?}");
    }

    // The two switches are separate; a failure follows the needs-you one.
    let ctx = ToastContext {
        notify_needs_input: false,
        ..toast_ctx()
    };
    assert_eq!(check(&ctx), [false, true, false]);
    let ctx = ToastContext {
        notify_ready_for_review: false,
        ..toast_ctx()
    };
    assert_eq!(check(&ctx), [true, false, true]);
    let ctx = ToastContext {
        notify_needs_input: false,
        notify_ready_for_review: false,
        ..toast_ctx()
    };
    assert_eq!(check(&ctx), [false, false, false]);
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(t, g)| ((*t).to_owned(), (*g).to_owned()))
        .collect()
}

#[test]
fn withdrawals_on_resolve() {
    let step = |from: Option<SessionState>, to: SessionState| {
        withdrawals(&transition(in_state(view("s1"), to), from))
    };
    // Answered: back to work or idle.
    assert_eq!(
        step(Some(permission("Bash")), SessionState::Working),
        pairs(&[("needs", "s1")])
    );
    assert_eq!(
        step(Some(permission("Bash")), SessionState::Idle),
        pairs(&[("needs", "s1")])
    );
    assert_eq!(
        step(Some(permission("Bash")), SessionState::ReadyForReview),
        pairs(&[("needs", "s1")])
    );
    // A failure cleared: the needs and failed banners both go.
    assert_eq!(
        step(
            Some(failed("Overloaded", "overloaded")),
            SessionState::Working
        ),
        pairs(&[("needs", "s1"), ("failed", "s1")])
    );
    // Reviewed or moved on.
    assert_eq!(
        step(Some(SessionState::ReadyForReview), SessionState::Idle),
        pairs(&[("review", "s1")])
    );
    assert_eq!(
        step(Some(SessionState::ReadyForReview), SessionState::Working),
        pairs(&[("review", "s1")])
    );
    assert_eq!(
        step(Some(SessionState::ReadyForReview), permission("Bash")),
        pairs(&[("review", "s1")])
    );
    // Still waiting, for something else: the newer banner replaces it, the
    // older isn't taken back.
    assert!(step(Some(permission("Bash")), permission("Edit")).is_empty());
    assert!(step(
        Some(permission("Bash")),
        SessionState::NeedsYou(NeedsInputReason::Question)
    )
    .is_empty());
    // A wait that turns into a failure keeps its needs banner's place.
    assert!(step(Some(permission("Bash")), failed("Overloaded", "overloaded")).is_empty());
    // The failure that turns into a wait loses only its failed banner.
    assert_eq!(
        step(Some(failed("Overloaded", "overloaded")), permission("Bash")),
        pairs(&[("failed", "s1")])
    );
    // Nothing before, nothing to take back; nothing changed, likewise.
    assert!(step(None, SessionState::Working).is_empty());
    assert!(step(Some(SessionState::Working), SessionState::Idle).is_empty());
    assert!(step(
        Some(SessionState::ReadyForReview),
        SessionState::ReadyForReview
    )
    .is_empty());
}

#[test]
fn withdrawals_when_a_session_goes_away() {
    assert_eq!(
        withdrawals_for_gone(&sid("s1")),
        pairs(&[("needs", "s1"), ("review", "s1"), ("failed", "s1")])
    );
    // The limit banner belongs to the ring; the hub recounts it.
    assert!(withdrawals_for_gone(&sid("s1"))
        .iter()
        .all(|(t, _)| t != "limit"));
}

#[test]
fn the_review_link_carries_the_completion_it_announced() {
    // In milliseconds, so a later completion is never reviewed by it.
    let mut v = in_state(view("s1"), SessionState::ReadyForReview);
    v.completed_at = Some(t0());
    let toast = review_toast(&v, &toast_ctx());
    assert_eq!(
        toast.actions[1].1,
        format!("agentnotch://review?session=s1&completed={}", to_ms(t0()))
    );
}
