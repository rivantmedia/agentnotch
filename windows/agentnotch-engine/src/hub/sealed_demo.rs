//! The sealed demo: the fixture accounts, sessions and usage a sealed run
//! shows, and the changes the Mac's sealed run plays after launch. Ports of
//! `SampleLayout`, `SampleSessions`, `SampleData` and
//! `ClaudeControlHub+SealedDemo` (`SealedDemoScript`).
//!
//! The demo holds the real stores, filled in memory: an [`AccountRegistry`]
//! given the fixture folders (`replace_all_with_fixtures`), a [`UsageStore`]
//! given the fixture readings, a [`HookManager`] given each folder's hook
//! status, and [`Session`] records. The snapshot, the settings pane and the
//! setup state then come out of the hub's own projections
//! (`project::project`, `project_settings::settings_snapshot`, as sealed:
//! no consent card, nothing to install), so what a sealed run shows is what
//! the live hub would show for the same stores.
//! `tests/ui-contract/{snapshot,settings}.json` are this demo at
//! `generated_at_ms` (the hub shifts nothing: it builds the demo at "now").
//!
//! The Windows sample is the Mac's with Windows' own values (the
//! ui-contract fixtures were written from it first): two accounts in
//! `~\.claude` and `~\.claude-work` (Claude Parallel Profiles never runs on
//! native Windows, so the Mac's spread of one identity over windows' working
//! copies and account stores has no Windows counterpart), the hosts Windows
//! tells apart, and one elicitation row standing for both of the Mac's
//! terminal dialogs (a Windows row draws an elicitation and a dialog alike).
//! The chat is the transcript sample `chat.json` holds.
//!
//! Nothing here reads, writes, spawns or sends anything; no clock is read
//! either: every time is relative to the `now` the caller passes.
//!
//! Owner: WP7.

use super::project::{self, Directory, ProjectionInput, RowExtras};
use super::project_settings::{settings_snapshot, SettingsInput, SetupInput};
use crate::accounts::{AccountRegistry, Folder};
use crate::attention::rows::ResetClock;
use crate::core::flags::DevFlags;
use crate::core::paths::{PathStyle, Paths};
use crate::core::settings::ControlSettings;
use crate::core::time::to_ms;
use crate::hooks::HookManager;
use crate::model::*;
use crate::platform::NotifyPermission;
use crate::runtime_types::{FolderHookStatus, RingReading, VersionSighting, VersionSource};
use crate::sessions::attention::humanized_stop_error;
use crate::sessions::session::{Session, SessionTitleSource};
use crate::usage::store::{UsageStore, UsageStoreConfig};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// The chat the demo shows for any session it is opened on (a reset).
pub const CHAT_SAMPLE: &str = include_str!("../../tests/ui-contract/chat.json");

// ---- the layout (SampleLayout) ----

/// The home folder the fixture folders are written against. Nothing under
/// it is read: the folders are fixtures.
pub const HOME: &str = r"C:\Users\me";
/// The personal identity's folder: the default `~\.claude`.
pub const PERSONAL_DIR: &str = r"C:\Users\me\.claude";
/// The work identity's folder.
pub const WORK_DIR: &str = r"C:\Users\me\.claude-work";
/// The folder the third account signs in to a few seconds into the Mac's
/// sealed run (`SampleData.side`).
pub const SIDE_DIR: &str = r"C:\Users\me\.claude-side";

pub const PERSONAL_UUID: &str = "5f0c3a1e-0000-4000-8000-000000000001";
pub const WORK_UUID: &str = "8a7b6c5d-0000-4000-8000-000000000002";
pub const WORK_ORGANIZATION: &str = "org-work-0002";
pub const SIDE_UUID: &str = "e8d4b1c6-2a7f-4e93-b05d-9c3e6f1a8b27";

/// The fixture's paths: Windows rules over [`HOME`], whatever the build host.
pub fn paths() -> Paths {
    Paths::new(PathStyle::Windows, HOME)
}

fn folder(
    paths: &Paths,
    dir: &str,
    label: &str,
    identity: Identity,
    plan: &str,
    color: i64,
) -> Folder {
    let mut folder = Folder::new(paths, dir);
    folder.config_dir_env = (!paths.is_default_config_dir(dir)).then(|| dir.to_owned());
    folder.custom_label = Some(label.to_owned());
    folder.identity = Some(identity);
    folder.subscription_type = Some(plan.to_owned());
    folder.color_index = color;
    folder
}

/// `~\.claude`, signed in to the personal account (Max 5x).
pub fn personal_folder(paths: &Paths) -> Folder {
    let identity = Identity {
        account_uuid: Some(PERSONAL_UUID.into()),
        email: Some("me@personal.example".into()),
        display_name: Some("Me".into()),
        organization_type: Some("claude_max".into()),
        rate_limit_tier: Some("default_claude_max_5x".into()),
        ..Identity::default()
    };
    folder(paths, PERSONAL_DIR, "Personal", identity, "max", 0)
}

/// `~\.claude-work`, signed in to the work organisation (Team).
pub fn work_folder(paths: &Paths) -> Folder {
    let identity = Identity {
        account_uuid: Some(WORK_UUID.into()),
        email: Some("me@work.example".into()),
        organization_name: Some("Acme".into()),
        organization_uuid: Some(WORK_ORGANIZATION.into()),
        organization_type: Some("claude_team".into()),
        ..Identity::default()
    };
    folder(paths, WORK_DIR, "Work", identity, "team", 3)
}

/// `~\.claude-side` ("Side project", Pro): the third account.
pub fn side_folder(paths: &Paths) -> Folder {
    let identity = Identity {
        account_uuid: Some(SIDE_UUID.into()),
        email: Some("hello@side.dev".into()),
        organization_type: Some("claude_pro".into()),
        ..Identity::default()
    };
    folder(paths, SIDE_DIR, "Side project", identity, "pro", 2)
}

/// Every fixture folder, the default first (`SampleLayout.folders`).
pub fn folders(paths: &Paths) -> Vec<Folder> {
    vec![personal_folder(paths), work_folder(paths)]
}

// ---- the sessions (SampleSessions) ----

/// Where each sample session runs, as the rows name it, and what can be
/// done with its window: the hub's caches for a live session. The question
/// runs in VS Code's chat panel, which takes no typed reply; the oldest idle
/// session's terminal is gone.
pub fn extras(session_id: &str) -> RowExtras {
    let host = match session_id {
        "needs-question" | "work-ci" | "side-landing-page" => "VS Code",
        "needs-plan" | "review-darkmode" => "Console",
        _ => "Windows Terminal",
    };
    let gone = session_id == "idle-logo";
    RowExtras {
        host_app: Some(host.to_owned()),
        can_focus: !gone,
        can_message: !gone && session_id != "needs-question",
    }
}

/// Which fixture account a session runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleAccount {
    Personal,
    Work,
    Side,
}

impl SampleAccount {
    pub fn dir(self) -> &'static str {
        match self {
            SampleAccount::Personal => PERSONAL_DIR,
            SampleAccount::Work => WORK_DIR,
            SampleAccount::Side => SIDE_DIR,
        }
    }
}

/// What `SampleSessions.make` takes beyond the id, title and project.
#[derive(Default)]
struct Make {
    /// `(id prefix, done, the active task, pending)`.
    tasks: Option<(&'static str, u32, Option<&'static str>, u32)>,
    context: Option<f64>,
    turn_started: Option<SystemTime>,
    completed: Option<SystemTime>,
    last_assistant: Option<&'static str>,
    background: u32,
    last_activity: Option<SystemTime>,
    /// `(role, tool, text)`.
    last_message: Option<(&'static str, Option<&'static str>, &'static str)>,
}

fn before(now: SystemTime, secs: u64) -> SystemTime {
    now.checked_sub(Duration::from_secs(secs)).unwrap_or(now)
}

/// One sample session (`SampleSessions.make`): a hook-backed session of
/// `account` in `C:\Users\me\code\<project>`, first seen six hours ago.
fn make(
    now: SystemTime,
    id: &str,
    title: &str,
    project: &str,
    account: SampleAccount,
    phase: Phase,
    more: Make,
) -> Session {
    let paths = paths();
    let mut session = Session::new(
        id,
        format!(r"C:\Users\me\code\{project}"),
        before(now, 6 * 3600),
    );
    // Stable across runs: the sum of the id's characters, as the Mac's.
    let seed: u32 = id.chars().map(|c| c as u32).sum();
    session.pid = Some(40_000 + seed % 9_000);
    session.apply_title(Some(title), SessionTitleSource::Hook);
    let dir = account.dir();
    session.account = Some(AccountId::new(paths.normalize(dir)));
    session.config_dir_env = (!paths.is_default_config_dir(dir)).then(|| dir.to_owned());
    session.entrypoint = Some(if extras(id).host_app.as_deref() == Some("VS Code") {
        "claude-vscode".to_owned()
    } else {
        "cli".to_owned()
    });
    if let Some((prefix, done, active, pending)) = more.tasks {
        // Created first, then moved on: a list whose tasks are all done
        // starts afresh at the next TaskCreate, as Claude Code's does.
        let mut items: Vec<(String, &str, Option<&str>)> = Vec::new();
        for step in 0..done {
            items.push((format!("Step {}", step + 1), "completed", None));
        }
        if let Some(active) = active {
            items.push((active.to_owned(), "in_progress", Some(active)));
        }
        let first = items.len();
        for step in 0..pending as usize {
            items.push((format!("Step {}", first + step + 1), "pending", None));
        }
        for (index, (subject, _, _)) in items.iter().enumerate() {
            session
                .tasks
                .task_created(&format!("{prefix}-{}", index + 1), Some(subject));
        }
        for (index, (_, status, active_form)) in items.iter().enumerate() {
            let id = format!("{prefix}-{}", index + 1);
            session
                .tasks
                .task_updated(&id, Some(status), None, *active_form);
        }
    }
    session.context_used_percent = more.context;
    session.phase = phase;
    session.turn_started_at = more.turn_started;
    session.completed_at = more.completed;
    session.last_assistant_message = more.last_assistant.map(str::to_owned);
    session.background_task_count = more.background;
    let info = &mut session.conversation_info;
    match more.last_message {
        Some((role, tool, text)) => {
            info.last_message = Some(text.to_owned());
            info.last_message_role = Some(role.to_owned());
            info.last_tool_name = tool.map(str::to_owned);
        }
        None => {
            info.last_message = more
                .last_assistant
                .map(|text| text.chars().take(80).collect());
            info.last_message_role = more.last_assistant.map(|_| "assistant".to_owned());
        }
    }
    session.last_activity = more
        .last_activity
        .or(more.completed)
        .or(more.turn_started)
        .unwrap_or(now);
    session.last_event_at = session.last_activity;
    session.last_hook_event_at = Some(session.last_activity);
    session
}

fn approval(id: &str, tool: &str, input: Value, received: SystemTime, rules: Vec<Value>) -> Phase {
    Phase::WaitingForApproval(PermissionContext {
        tool_use_id: id.to_owned(),
        tool_name: tool.to_owned(),
        tool_input: input,
        received_at: received,
        permission_suggestions: rules,
        has_synthetic_tool_use_id: false,
        agent_id: None,
        activated_at: Some(received),
    })
}

/// Every attention state, as the Sessions tab lists them
/// (`SampleSessions.all`): needs you, ready for review, working, idle.
pub fn all(now: SystemTime) -> Vec<Session> {
    let mut sessions = needs_you(now);
    sessions.extend(review(now));
    sessions.extend(working(now));
    sessions.extend(idle(now));
    sessions
}

/// A permission, a question, a plan, an MCP server's dialog and a
/// rate-limited turn.
pub fn needs_you(now: SystemTime) -> Vec<Session> {
    let ago = |secs| before(now, secs);
    let permission = make(
        now,
        "needs-permission",
        "Fix the login redirect loop",
        "acme-web",
        SampleAccount::Work,
        approval(
            "toolu_sample_bash",
            "Bash",
            json!({"command": "npm run test -- --watch=false auth/redirect.spec.ts"}),
            ago(120),
            vec![json!({
                "type": "addRules",
                "rules": [{"toolName": "Bash", "ruleContent": "npm run test:*"}],
                "behavior": "allow",
                "destination": "localSettings",
            })],
        ),
        Make {
            tasks: Some(("p", 3, Some("Running the auth test suite"), 3)),
            context: Some(42.0),
            turn_started: Some(ago(9 * 60)),
            last_activity: Some(ago(120)),
            ..Make::default()
        },
    );
    let question = make(
        now,
        "needs-question",
        "Pick a charting library",
        "dashboard",
        SampleAccount::Personal,
        approval(
            "toolu_sample_question",
            "AskUserQuestion",
            json!({"questions": [{
                "question": "Which charting library should the dashboard use?",
                "header": "Charts",
                "multiSelect": false,
                "options": [
                    {"label": "Recharts", "description": "Composable React components"},
                    {"label": "Chart.js", "description": "Canvas, small bundle"},
                    {"label": "ECharts", "description": "Feature-rich, larger bundle"},
                ],
            }]}),
            ago(6 * 60),
            Vec::new(),
        ),
        Make {
            context: Some(18.0),
            turn_started: Some(ago(8 * 60)),
            last_activity: Some(ago(6 * 60)),
            ..Make::default()
        },
    );
    let plan = make(
        now,
        "needs-plan",
        "Migrate settings storage to SQLite",
        "notes-app",
        SampleAccount::Personal,
        approval(
            "toolu_sample_plan",
            "ExitPlanMode",
            json!({"plan": "## Plan\n\n1. Add the models\n2. Migrate the stored settings\n3. Remove the old store"}),
            ago(11 * 60),
            Vec::new(),
        ),
        Make {
            context: Some(61.0),
            turn_started: Some(ago(16 * 60)),
            last_activity: Some(ago(11 * 60)),
            ..Make::default()
        },
    );
    // An MCP server asked for input: a dialog in the terminal. On the
    // personal account: the work account's only prompts are the permission
    // and the rate-limited turn, which the demo answers so that ring works.
    let mut elicitation = make(
        now,
        "needs-elicitation",
        "Sync the design tokens",
        "design-system",
        SampleAccount::Personal,
        Phase::Processing,
        Make {
            context: Some(33.0),
            turn_started: Some(ago(5 * 60)),
            last_activity: Some(ago(5 * 60)),
            ..Make::default()
        },
    );
    elicitation.set_needs_input(
        Some(NeedsInputReason::Elicitation {
            message: "Figma needs you to pick a file".into(),
        }),
        ago(5 * 60),
    );
    let mut rate_limited = make(
        now,
        "needs-ratelimit",
        "Refactor the billing webhooks",
        "billing-service",
        SampleAccount::Work,
        Phase::WaitingForInput,
        Make {
            context: Some(57.0),
            last_activity: Some(ago(9 * 60)),
            ..Make::default()
        },
    );
    rate_limited.set_needs_input(
        Some(NeedsInputReason::Error {
            text: humanized_stop_error(Some("rate_limit")),
            code: Some("rate_limit".into()),
        }),
        ago(9 * 60),
    );
    vec![permission, question, plan, elicitation, rate_limited]
}

/// Finished and not looked at: just now, five and twelve minutes ago.
pub fn review(now: SystemTime) -> Vec<Session> {
    let ago = |secs| before(now, secs);
    vec![
        make(
            now,
            "review-just-finished",
            "Fix the flaky date test",
            "billing-service",
            SampleAccount::Personal,
            Phase::WaitingForInput,
            Make {
                tasks: Some(("r", 3, None, 0)),
                context: Some(27.0),
                turn_started: Some(ago(6 * 60)),
                completed: Some(ago(20)),
                last_assistant: Some("The test pinned the time zone to UTC; it now uses a fixed calendar and passes 50 runs in a row."),
                ..Make::default()
            },
        ),
        make(
            now,
            "review-darkmode",
            "Add a dark mode toggle to settings",
            "acme-web",
            SampleAccount::Personal,
            Phase::WaitingForInput,
            Make {
                tasks: Some(("d", 5, None, 0)),
                context: Some(38.0),
                turn_started: Some(ago(19 * 60)),
                completed: Some(ago(5 * 60)),
                last_assistant: Some("Added a Dark mode toggle under Settings › Appearance. It follows the system by default, persists the choice, and all 42 tests pass."),
                ..Make::default()
            },
        ),
        make(
            now,
            "review-devserver",
            "Set up the local dev server",
            "acme-web",
            SampleAccount::Work,
            Phase::WaitingForInput,
            Make {
                context: Some(22.0),
                turn_started: Some(ago(31 * 60)),
                completed: Some(ago(12 * 60)),
                last_assistant: Some("The dev server is up on http://localhost:5173 with hot reload. I left the type checker watching in the background."),
                background: 2,
                ..Make::default()
            },
        ),
    ]
}

/// Working through tasks, bisecting with a tool, and just thinking.
pub fn working(now: SystemTime) -> Vec<Session> {
    let ago = |secs| before(now, secs);
    vec![
        make(
            now,
            "work-migration",
            "Write migration tests for the v2 schema",
            "billing-service",
            SampleAccount::Work,
            Phase::Processing,
            Make {
                tasks: Some(("m", 2, Some("Writing tests for the v2 schema"), 3)),
                context: Some(84.0),
                turn_started: Some(ago(14 * 60)),
                ..Make::default()
            },
        ),
        make(
            now,
            "work-ci",
            "Investigate the flaky CI job",
            "infra",
            SampleAccount::Personal,
            Phase::Processing,
            Make {
                tasks: Some(("c", 9, Some("Bisecting the failing commit"), 5)),
                context: Some(93.0),
                turn_started: Some(ago(8 * 60)),
                last_message: Some(("tool", Some("Grep"), "ETIMEDOUT|socket hang up")),
                ..Make::default()
            },
        ),
        make(
            now,
            "work-summary",
            "Summarize the PR review feedback",
            "acme-web",
            SampleAccount::Work,
            Phase::Processing,
            Make {
                context: Some(12.0),
                turn_started: Some(ago(40)),
                ..Make::default()
            },
        ),
    ]
}

/// Idle for fifty minutes, three and five hours, and a day.
pub fn idle(now: SystemTime) -> Vec<Session> {
    let ago = |secs| before(now, secs);
    vec![
        make(
            now,
            "idle-notch",
            "Explore the notch APIs",
            "agent-notch",
            SampleAccount::Personal,
            Phase::Idle,
            Make {
                context: Some(7.0),
                last_activity: Some(ago(50 * 60)),
                last_message: Some((
                    "assistant",
                    None,
                    "The work area excludes the taskbar, so the notch sits below it.",
                )),
                ..Make::default()
            },
        ),
        make(
            now,
            "idle-readme",
            "Tidy up the README",
            "acme-web",
            SampleAccount::Work,
            Phase::Idle,
            Make {
                last_activity: Some(ago(3 * 3600)),
                last_message: Some(("user", None, "thanks, that's all for now")),
                ..Make::default()
            },
        ),
        make(
            now,
            "idle-deps",
            "Bump dependencies",
            "billing-service",
            SampleAccount::Work,
            Phase::Idle,
            Make {
                last_activity: Some(ago(5 * 3600)),
                last_message: Some(("tool", Some("Bash"), "npm outdated")),
                ..Make::default()
            },
        ),
        make(
            now,
            "idle-logo",
            "Draft a new logo brief",
            "brand",
            SampleAccount::Personal,
            Phase::Idle,
            Make {
                last_activity: Some(ago(26 * 3600)),
                ..Make::default()
            },
        ),
    ]
}

/// What the third account brings when it signs in (`SealedDemoScript.
/// thirdAccountSessions`): a session that finished [`THIRD_ACCOUNT_REVIEW_AGE`]
/// ago, so its ring's green arc settles a few seconds later, and an idle one.
pub fn third_account_sessions(now: SystemTime) -> Vec<Session> {
    let ago = |secs| before(now, secs);
    let finished = ago(THIRD_ACCOUNT_REVIEW_AGE.as_secs());
    let review = make(
        now,
        "side-launch-post",
        "Write the launch announcement",
        "side-site",
        SampleAccount::Side,
        Phase::WaitingForInput,
        Make {
            tasks: Some(("s", 4, None, 0)),
            context: Some(31.0),
            turn_started: Some(ago(6 * 60)),
            completed: Some(finished),
            last_assistant: Some("The announcement draft is in posts\\launch.md."),
            ..Make::default()
        },
    );
    let idle = make(
        now,
        "side-pricing",
        "Sketch the pricing page",
        "side-site",
        SampleAccount::Side,
        Phase::Idle,
        Make {
            context: Some(9.0),
            last_activity: Some(ago(40 * 60)),
            ..Make::default()
        },
    );
    vec![review, idle]
}

// ---- usage (SampleData) ----

fn window(used: f64, resets_in: u64, duration_s: u64, now: SystemTime) -> UsageWindow {
    UsageWindow::new(used, Some(now + Duration::from_secs(resets_in)), duration_s)
}

/// Each fixture identity's usage (`SampleData.usage`). The work account has
/// used up its 5-hour window: its rate-limited session waits for that reset.
pub fn usage(identity: &IdentityId, account: SampleAccount, now: SystemTime) -> AccountUsage {
    let session = UsageWindow::SESSION_DURATION_S;
    let weekly = UsageWindow::WEEKLY_DURATION_S;
    let mut usage = AccountUsage::new(identity.clone(), UsageSource::Probe, before(now, 240));
    match account {
        SampleAccount::Personal => {
            usage.five_hour = Some(window(34.0, 7_800, session, now));
            usage.seven_day = Some(window(41.0, 273_600, weekly, now));
            usage.scoped = vec![("Opus".into(), window(12.0, 273_600, weekly, now))];
            usage.subscription_type = Some("max".into());
        }
        SampleAccount::Work => {
            usage.five_hour = Some(window(100.0, 2_400, session, now));
            usage.seven_day = Some(window(55.0, 439_200, weekly, now));
            usage.subscription_type = Some("team".into());
            usage.source = UsageSource::StatusLine;
        }
        // Burning faster than the window allows (`SampleData.aheadOfPace`).
        SampleAccount::Side => {
            usage.five_hour = Some(window(72.0, 3 * 3600, session, now));
            usage.seven_day = Some(window(30.0, 5 * 86_400 + 2 * 3600, weekly, now));
            usage.subscription_type = Some("pro".into());
            usage.source = UsageSource::Cache;
        }
    }
    usage
}

// ---- the script (ClaudeControlHub+SealedDemo) ----

/// One change the Mac's sealed run makes after launch, in order. The
/// Windows sealed hub doesn't play them by itself (its self-test and
/// snapshots need the fixture to hold still); they are here for a run that
/// wants the notch to react, and for their tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SealedDemoStep {
    /// The work account's permission prompt is allowed and its
    /// rate-limited turn retried: both go back to work, so that ring spins
    /// while the personal one still asks. Resolutions: nothing chimes.
    AnswerWorkPrompts,
    /// A third account (`~\.claude-side`, "Side project") appears, with a
    /// session that finished 85 s ago and an idle one.
    AddThirdAccount,
    /// Two working sessions on two rings finish at the same moment: one
    /// burst of ready-for-review transitions.
    FinishTwoSessions,
    /// A working session stops for a permission prompt.
    AskPermission,
}

impl SealedDemoStep {
    pub const ALL: [SealedDemoStep; 4] = [
        SealedDemoStep::AnswerWorkPrompts,
        SealedDemoStep::AddThirdAccount,
        SealedDemoStep::FinishTwoSessions,
        SealedDemoStep::AskPermission,
    ];

    /// When the demo plays this step after launch: the whole timeline fits
    /// a run of ten seconds, and the third ring settles before it ends.
    pub fn after_launch(self) -> Duration {
        Duration::from_millis(match self {
            SealedDemoStep::AnswerWorkPrompts => 1_500,
            SealedDemoStep::AddThirdAccount => 3_000,
            SealedDemoStep::FinishTwoSessions => 4_500,
            SealedDemoStep::AskPermission => 6_000,
        })
    }
}

/// How long ago the third account's session finished when it appears.
pub const THIRD_ACCOUNT_REVIEW_AGE: Duration = Duration::from_secs(85);

/// The work account's prompts that [`SealedDemoStep::AnswerWorkPrompts`]
/// answers.
pub const ANSWERED_PROMPTS: [&str; 2] = ["needs-permission", "needs-ratelimit"];

/// The demo's change as a pure function of the current fixtures
/// (`SealedDemoScript.apply`). A step that already happened changes nothing.
pub fn apply_step(
    step: SealedDemoStep,
    folders: &[Folder],
    sessions: &[Session],
    now: SystemTime,
) -> (Vec<Folder>, Vec<Session>) {
    let mut folders = folders.to_vec();
    let mut sessions = sessions.to_vec();
    match step {
        SealedDemoStep::AnswerWorkPrompts => {
            for session in &mut sessions {
                let asking = matches!(
                    session.attention(),
                    SessionState::NeedsYou(_) | SessionState::Failed(_)
                );
                if !ANSWERED_PROMPTS.contains(&session.id.as_str()) || !asking {
                    continue;
                }
                session.phase = Phase::Processing;
                session.set_needs_input(None, now);
                session.completed_at = None;
                session.turn_started_at = Some(now);
                session.last_activity = now;
                session.last_event_at = now;
            }
        }
        SealedDemoStep::AddThirdAccount => {
            let side = side_folder(&paths());
            if !folders.iter().any(|folder| folder.id == side.id) {
                folders.push(side);
                sessions.extend(third_account_sessions(now));
            }
        }
        SealedDemoStep::FinishTwoSessions => {
            for session in &mut sessions {
                let finishing = ["work-summary", "work-ci"].contains(&session.id.as_str());
                if !finishing || session.attention() != SessionState::Working {
                    continue;
                }
                session.phase = Phase::WaitingForInput;
                session.completed_at = Some(now);
                session.last_activity = now;
                session.last_event_at = now;
                session.last_assistant_message = Some("Done.".into());
            }
        }
        SealedDemoStep::AskPermission => {
            for session in &mut sessions {
                if session.id.as_str() != "work-migration"
                    || session.attention() != SessionState::Working
                {
                    continue;
                }
                session.phase = approval(
                    "toolu_sealed_demo_edit",
                    "Edit",
                    json!({"file_path": r"migrations\v2_schema.sql"}),
                    now,
                    Vec::new(),
                );
                session.last_activity = now;
                session.last_event_at = now;
            }
        }
    }
    (folders, sessions)
}

// ---- the demo's stores ----

/// What the hook manager found in a folder's settings.json: our hooks, and
/// the status line unless the folder's own was left alone.
fn hook_status(dir: &str, status_line: bool, left_alone: Option<&str>) -> FolderHookStatus {
    FolderHookStatus {
        config_dir_exists: true,
        settings_readable: true,
        hooks_registered: true,
        hooks_installed: true,
        status_line_installed: status_line,
        status_line_left_alone: left_alone.map(str::to_owned),
        form: Some("exec".into()),
        newest_backup: (dir == WORK_DIR).then(|| {
            PathBuf::from(format!(
                r"{dir}\settings.json.agentnotch-20260921-141000-000.bak"
            ))
        }),
        ..FolderHookStatus::default()
    }
}

/// The sealed run's stores, filled with the fixtures.
pub struct SealedDemo {
    paths: Paths,
    registry: AccountRegistry,
    usage: UsageStore,
    hooks: HookManager,
    settings: ControlSettings,
    sessions: Vec<Session>,
    versions: Vec<VersionSighting>,
    changed_folders: Vec<AccountId>,
    cloud: CloudState,
    pipe_name: String,
}

impl SealedDemo {
    /// The demo as it stands at `now`, with the hook pipe the run would
    /// name (nothing listens on it).
    pub fn new(now: SystemTime, pipe_name: &str) -> SealedDemo {
        let paths = paths();
        let settings = ControlSettings {
            hook_consent: Some(true),
            hook_consent_scope: ControlSettings::CURRENT_CONSENT_SCOPE,
            hooks_enabled: true,
            status_line_integration: true,
            ..ControlSettings::default()
        };
        let mut registry = AccountRegistry::new(paths.clone());
        registry.replace_all_with_fixtures(folders(&paths), None);
        let flags = DevFlags {
            sealed: true,
            ..DevFlags::default()
        };
        let mut hooks = HookManager::configured(
            PathBuf::from(r"C:\Users\me\AppData\Local\Agent Notch\agentnotch-hook.exe"),
            &flags,
        );
        hooks.absorb_status(
            &AccountId::new(paths.normalize(PERSONAL_DIR)),
            hook_status(PERSONAL_DIR, true, None),
            &settings,
        );
        hooks.absorb_status(
            &AccountId::new(paths.normalize(WORK_DIR)),
            hook_status(
                WORK_DIR,
                false,
                Some("Status line left alone: its command uses Windows paths"),
            ),
            &settings,
        );
        let usage = UsageStore::with_config(UsageStoreConfig {
            home: PathBuf::from(HOME),
            probes_allowed: false,
            probes_disabled: true,
            probe_interval_minutes: settings.usage_probe_interval_minutes,
            reads_desktop: false,
            mirrors_default: false,
        });
        let mut demo = SealedDemo {
            changed_folders: vec![AccountId::new(paths.normalize(WORK_DIR))],
            paths,
            registry,
            usage,
            hooks,
            settings,
            sessions: all(now),
            versions: vec![VersionSighting {
                source: VersionSource::Binary,
                path: Some(PathBuf::from(r"C:\Users\me\.local\bin\claude.exe")),
                version: Some("2.1.282".into()),
            }],
            cloud: cloud(now),
            pipe_name: pipe_name.to_owned(),
        };
        demo.attribute(now);
        demo
    }

    /// The registry's folders' readings into the usage store, and each
    /// session's account into its attribution (the hub's own work in a
    /// live run).
    fn attribute(&mut self, now: SystemTime) {
        let accounts = self.registry.accounts();
        self.usage
            .set_accounts(&accounts, &self.registry.folders(), now);
        for account in &accounts {
            let which = if account
                .run_dirs
                .iter()
                .any(|d| self.paths.same(d.as_str(), PERSONAL_DIR))
            {
                SampleAccount::Personal
            } else if account
                .run_dirs
                .iter()
                .any(|d| self.paths.same(d.as_str(), WORK_DIR))
            {
                SampleAccount::Work
            } else {
                SampleAccount::Side
            };
            if self.usage.usage_of(&account.identity_id).is_none() {
                self.usage
                    .accept_snapshot(usage(&account.identity_id, which, now), now);
            }
        }
        for session in &mut self.sessions {
            let folder = session.account.clone();
            let identity = folder
                .as_ref()
                .and_then(|folder| self.registry.identity_id_for(folder.as_str()));
            session.attribution = Attribution::Known(identity);
        }
    }

    pub fn accounts(&self) -> Vec<Account> {
        self.registry.accounts()
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn views(&self) -> Vec<SessionView> {
        self.sessions.iter().map(Session::to_view).collect()
    }

    pub fn control(&self) -> &ControlSettings {
        &self.settings
    }

    pub fn registry(&self) -> &AccountRegistry {
        &self.registry
    }

    /// Each identity's ring reading at `now`.
    pub fn readings(&self, now: SystemTime) -> BTreeMap<IdentityId, RingReading> {
        self.registry
            .accounts()
            .iter()
            .map(|a| {
                (
                    a.identity_id.clone(),
                    self.usage.ring_reading(&a.identity_id, now),
                )
            })
            .collect()
    }

    fn setup_input(&self) -> SetupInput<'_> {
        SetupInput {
            registry: &self.registry,
            hooks: &self.hooks,
            settings: &self.settings,
            window_names: &EMPTY_NAMES,
            transport_error: None,
            sealed: true,
        }
    }

    /// The hub snapshot at `now`.
    pub fn snapshot(&self, now: SystemTime, generated_at_ms: u64) -> HubSnapshot {
        let accounts = self.registry.accounts();
        let readings = self.readings(now);
        let views = self.views();
        let ui = self.settings.ui();
        let setup = super::project_settings::setup_state(&self.setup_input());
        let directory: &dyn Directory = &self.registry;
        project::project(
            &ProjectionInput {
                now,
                accounts: &accounts,
                directory,
                readings: &readings,
                sessions: &views,
                extras: &|view: &SessionView| extras(view.id.as_str()),
                clock: ResetClock::default(),
                ui: &ui,
                setup: &setup,
                sealed: true,
            },
            generated_at_ms,
        )
    }

    /// The Claude Code settings pane at `now`.
    pub fn settings_snapshot(&self, now: SystemTime) -> SettingsSnapshot {
        let readings = self.readings(now);
        let views = self.views();
        settings_snapshot(&SettingsInput {
            now,
            setup: self.setup_input(),
            readings: &readings,
            versions: &self.versions,
            changed_folders: &self.changed_folders,
            pipe_name: &self.pipe_name,
            busy: false,
            refreshing: false,
            desktop_format: Some(DesktopCacheFormat::Absent),
            notify_permission: NotifyPermission::Allowed,
            hotkey_ok: true,
            hotkey_message: None,
            cloud: &self.cloud,
            session_count: views.len() as u32,
            review_count: views
                .iter()
                .filter(|v| v.state == SessionState::ReadyForReview)
                .count() as u32,
        })
    }

    /// The settings, for a call that changes one (in memory only).
    pub fn control_mut(&mut self) -> &mut ControlSettings {
        &mut self.settings
    }

    pub fn has_session(&self, session_id: &str) -> bool {
        self.sessions.iter().any(|s| s.id.as_str() == session_id)
    }

    fn session_mut(&mut self, session_id: &str) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .find(|s| s.id.as_str() == session_id)
    }

    /// An answer to the session's active request: it goes back to work (no
    /// hook is waiting, so nothing is sent). `false` when that request isn't
    /// the one waiting.
    pub fn answer(&mut self, session_id: &str, tool_use_id: &str, now: SystemTime) -> bool {
        let Some(session) = self.session_mut(session_id) else {
            return false;
        };
        let asked = matches!(&session.phase,
            Phase::WaitingForApproval(request) if request.tool_use_id == tool_use_id);
        if asked {
            session.phase = Phase::Processing;
            session.set_needs_input(None, now);
            session.last_activity = now;
            session.last_event_at = now;
        }
        asked
    }

    /// The session's finished work was looked at. `false` when it had
    /// nothing to review.
    pub fn mark_reviewed(&mut self, session_id: &str, now: SystemTime) -> bool {
        match self.session_mut(session_id) {
            Some(session) if session.is_ready_for_review() => {
                session.reviewed_at = Some(now);
                true
            }
            _ => false,
        }
    }

    /// Every session with finished work, as `reset_review_queue` clears them.
    pub fn reviewable(&self) -> Vec<String> {
        self.sessions
            .iter()
            .filter(|s| s.is_ready_for_review())
            .map(|s| s.id.as_str().to_owned())
            .collect()
    }

    /// A failed turn is put aside. `false` when the session hadn't failed.
    pub fn dismiss_failure(&mut self, session_id: &str, now: SystemTime) -> bool {
        match self.session_mut(session_id) {
            Some(session) if matches!(session.attention(), SessionState::Failed(_)) => {
                session.set_needs_input(None, now);
                true
            }
            _ => false,
        }
    }

    /// The identity `id` (or a folder of it) is a fixture account.
    pub fn has_account(&self, id: &str) -> bool {
        self.registry.identity_id_for(id).is_some()
    }

    /// The user's own name for an account, `None` to go back to its own.
    pub fn rename(&mut self, id: &str, label: Option<&str>, now: SystemTime) {
        let folders: Vec<String> = self
            .registry
            .folders_for(id)
            .iter()
            .map(|f| f.dir().to_owned())
            .collect();
        for folder in folders {
            self.registry.rename(&folder, label);
        }
        self.attribute(now);
    }

    /// "Track sessions and hooks".
    pub fn track(&mut self, id: &str, on: bool, now: SystemTime) {
        self.registry.set_hidden(id, !on);
        self.attribute(now);
    }

    /// "Ring in notch".
    pub fn ring_shown(&mut self, ring_id: &str, on: bool, now: SystemTime) {
        self.registry.set_ring_shown(ring_id, on);
        self.attribute(now);
    }

    /// Play one step of the script on the demo's stores.
    pub fn apply(&mut self, step: SealedDemoStep, now: SystemTime) {
        let known = self.registry.known_folders().to_vec();
        let (folders, sessions) = apply_step(step, &known, &self.sessions, now);
        if folders != known {
            self.registry.replace_all_with_fixtures(folders, None);
        }
        self.sessions = sessions;
        self.attribute(now);
    }
}

static EMPTY_NAMES: BTreeMap<String, String> = BTreeMap::new();

/// What the sealed run's Cloud section shows: signed in to the app's
/// website, sync on, the last sync three minutes ago. Nothing behind it is
/// real (the cloud service isn't started sealed).
pub fn cloud(now: SystemTime) -> CloudState {
    let website = "https://agentnotch.rivant.in";
    CloudState {
        website_url: Some(website.into()),
        website_is_overridden: false,
        auth: CloudAuthState::SignedIn {
            email: Some("me@personal.example".into()),
        },
        sync_enabled: true,
        summaries_enabled: false,
        summaries_available: true,
        is_syncing: false,
        last_sync_at_ms: Some(to_ms(before(now, 180))),
        last_error: None,
        pending_sessions: 1,
        pending_usage: 4,
        summarized_sessions: 0,
        dashboard_url: Some(format!("{website}/dashboard")),
        pools_url: Some(format!("{website}/pools")),
        settings_url: Some(format!("{website}/settings")),
    }
}

/// The chat sample (it carries no times), as a reset of `session_id` at
/// `revision`.
pub fn chat(session_id: &str, revision: u64) -> ChatUpdate {
    let mut chat: ChatUpdate = serde_json::from_str(CHAT_SAMPLE).expect("ui-contract chat.json");
    chat.session_id = session_id.to_owned();
    chat.revision = revision;
    chat.reset = true;
    chat
}
