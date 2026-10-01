//! Which hook events are registered, and the entries written for them
//! (`HookInstaller.hookEventConfigs`, HS§3.5).
//!
//! Before 2.1.101 an event name Claude Code didn't know made it ignore the
//! whole settings file, so only events the oldest Claude Code on this PC
//! knows are written, and without a version the baseline.

use super::commands::PERMISSION_TIMEOUT_SECONDS;
use super::version::ClaudeCodeVersion;
use crate::core::settings_doc::Json;
use crate::runtime_types::CommandForm;

/// How an event's matcher groups look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventShape {
    /// `[{"hooks": [H]}]`.
    Plain,
    /// `[{"matcher": "*", "hooks": [H]}]`.
    Matcher,
    /// As `Matcher`, the hook waiting a day for the app's answer.
    MatcherWithTimeout,
    /// `[{"matcher": "auto", …}, {"matcher": "manual", …}]`.
    Compact,
}

/// Present in every Claude Code that has hooks.
const BASELINE: [(&str, EventShape); 10] = [
    ("UserPromptSubmit", EventShape::Plain),
    ("PreToolUse", EventShape::Matcher),
    ("PostToolUse", EventShape::Matcher),
    ("PermissionRequest", EventShape::MatcherWithTimeout),
    ("Notification", EventShape::Matcher),
    ("Stop", EventShape::Plain),
    ("SubagentStop", EventShape::Plain),
    ("SessionStart", EventShape::Plain),
    ("SessionEnd", EventShape::Plain),
    ("PreCompact", EventShape::Compact),
];

/// Added later, with the version that brought each (Claude Code's changelog).
const ADDED: [(ClaudeCodeVersion, &str, EventShape); 7] = [
    // PostToolUseFailure shipped with the PostToolUse redesign.
    (
        ClaudeCodeVersion::new(2, 0, 0),
        "PostToolUseFailure",
        EventShape::Matcher,
    ),
    // Pairs with SubagentStop.
    (
        ClaudeCodeVersion::new(2, 0, 43),
        "SubagentStart",
        EventShape::Plain,
    ),
    (
        ClaudeCodeVersion::new(2, 1, 33),
        "TaskCompleted",
        EventShape::Plain,
    ),
    // Pairs with PreCompact.
    (
        ClaudeCodeVersion::new(2, 1, 76),
        "PostCompact",
        EventShape::Compact,
    ),
    // API errors: rate limit, auth, billing.
    (
        ClaudeCodeVersion::new(2, 1, 78),
        "StopFailure",
        EventShape::Plain,
    ),
    (
        ClaudeCodeVersion::new(2, 1, 84),
        "TaskCreated",
        EventShape::Plain,
    ),
    // Auto-mode classifier denials (the changelog has no 2.1.88).
    (
        ClaudeCodeVersion::new(2, 1, 89),
        "PermissionDenied",
        EventShape::Matcher,
    ),
];

/// The events to register for a Claude Code `version`, in the order they are
/// written. `None` (no version known): the baseline.
pub fn hook_events(version: Option<ClaudeCodeVersion>) -> Vec<&'static str> {
    let mut events: Vec<&'static str> = BASELINE.iter().map(|(name, _)| *name).collect();
    if let Some(version) = version {
        events.extend(
            ADDED
                .iter()
                .filter(|(since, _, _)| version >= *since)
                .map(|(_, name, _)| *name),
        );
    }
    events
}

/// The shape of an event this app registers; `None` for any other name.
pub fn event_shape(event: &str) -> Option<EventShape> {
    BASELINE
        .iter()
        .map(|(name, shape)| (*name, *shape))
        .chain(ADDED.iter().map(|(_, name, shape)| (*name, *shape)))
        .find(|(name, _)| *name == event)
        .map(|(_, shape)| shape)
}

/// One hook entry in `form`: `{"type": "command", "command": …}` with
/// `args` in exec form, and the day-long `timeout` when it waits for an
/// answer. `None` when the form can't be written.
pub fn hook_entry(form: &CommandForm, waits_for_answer: bool) -> Option<Json> {
    let mut members = vec![("type", Json::string("command"))];
    match form {
        CommandForm::Exec { command, args } => {
            members.push(("command", Json::string(command.to_string_lossy())));
            members.push(("args", Json::Array(args.iter().map(Json::string).collect())));
        }
        CommandForm::Text(command) => members.push(("command", Json::string(command))),
        CommandForm::NotPossible(_) => return None,
    }
    if waits_for_answer {
        members.push(("timeout", Json::int(PERMISSION_TIMEOUT_SECONDS)));
    }
    Some(Json::object(members))
}

/// The matcher groups to append for `event`. `None` for an event this app
/// doesn't register, or a form that can't be written.
pub fn event_groups(event: &str, form: &CommandForm) -> Option<Vec<Json>> {
    let shape = event_shape(event)?;
    let entry = hook_entry(form, shape == EventShape::MatcherWithTimeout)?;
    let hooks = || Json::Array(vec![entry.clone()]);
    let with_matcher =
        |matcher: &str| Json::object([("matcher", Json::string(matcher)), ("hooks", hooks())]);
    Some(match shape {
        EventShape::Plain => vec![Json::object([("hooks", hooks())])],
        EventShape::Matcher | EventShape::MatcherWithTimeout => vec![with_matcher("*")],
        EventShape::Compact => vec![with_matcher("auto"), with_matcher("manual")],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const BASELINE_NAMES: [&str; 10] = [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Notification",
        "Stop",
        "SubagentStop",
        "SessionStart",
        "SessionEnd",
        "PreCompact",
    ];

    fn events(version: Option<(u32, u32, u32)>) -> BTreeSet<&'static str> {
        hook_events(
            version.map(|(major, minor, patch)| ClaudeCodeVersion::new(major, minor, patch)),
        )
        .into_iter()
        .collect()
    }

    /// `HookInstallerPlanTests.versionGating`
    #[test]
    fn version_gating() {
        let baseline: BTreeSet<&str> = BASELINE_NAMES.into_iter().collect();
        assert_eq!(events(None), baseline);
        assert_eq!(hook_events(None), BASELINE_NAMES, "in the order written");
        assert!(!events(Some((2, 1, 32))).contains("TaskCompleted"));
        assert!(events(Some((2, 1, 33))).contains("TaskCompleted"));
        assert!(!events(Some((2, 1, 83))).contains("TaskCreated"));
        assert!(events(Some((2, 1, 84))).contains("TaskCreated"));
        // PermissionDenied shipped in 2.1.89 (the changelog has no 2.1.88).
        assert!(!events(Some((2, 1, 88))).contains("PermissionDenied"));
        assert!(events(Some((2, 1, 89))).contains("PermissionDenied"));
        let mut latest = baseline.clone();
        latest.extend([
            "PostToolUseFailure",
            "SubagentStart",
            "TaskCompleted",
            "PostCompact",
            "StopFailure",
            "TaskCreated",
            "PermissionDenied",
        ]);
        assert_eq!(events(Some((2, 1, 280))), latest);
        // Before the 2.0 redesign: the baseline only.
        assert_eq!(events(Some((1, 0, 120))), baseline);
    }

    #[test]
    fn entries_in_both_forms() {
        let text = CommandForm::Text("C:/Users/me/.claude/hooks/agentnotch-hook.exe hook".into());
        assert_eq!(
            hook_entry(&text, false)
                .unwrap()
                .serialized_with("", "", "\n"),
            r#"{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe hook"}"#
        );
        let exec = CommandForm::Exec {
            command: r"C:\Users\me\.claude\hooks\agentnotch-hook.exe".into(),
            args: vec!["hook".into(), "--exec".into()],
        };
        assert_eq!(
            hook_entry(&exec, true)
                .unwrap()
                .serialized_with("", "", "\n"),
            r#"{"type":"command","command":"C:\\Users\\me\\.claude\\hooks\\agentnotch-hook.exe","args":["hook","--exec"],"timeout":86400}"#
        );
        assert_eq!(
            hook_entry(&CommandForm::NotPossible("no".into()), false),
            None
        );
    }

    #[test]
    fn groups_per_shape() {
        let form = CommandForm::Text("x hook".into());
        let one_line = |event: &str| -> Vec<String> {
            event_groups(event, &form)
                .unwrap()
                .iter()
                .map(|group| group.serialized_with("", "", "\n"))
                .collect()
        };
        assert_eq!(
            one_line("Stop"),
            [r#"{"hooks":[{"type":"command","command":"x hook"}]}"#]
        );
        assert_eq!(
            one_line("PreToolUse"),
            [r#"{"matcher":"*","hooks":[{"type":"command","command":"x hook"}]}"#]
        );
        // `permissionRequestWaitsForTheApp`
        assert_eq!(
            one_line("PermissionRequest"),
            [r#"{"matcher":"*","hooks":[{"type":"command","command":"x hook","timeout":86400}]}"#]
        );
        assert_eq!(
            one_line("PreCompact"),
            [
                r#"{"matcher":"auto","hooks":[{"type":"command","command":"x hook"}]}"#,
                r#"{"matcher":"manual","hooks":[{"type":"command","command":"x hook"}]}"#
            ]
        );
        assert_eq!(event_groups("TeammateIdle", &form), None);
        assert_eq!(
            event_groups("Stop", &CommandForm::NotPossible("no".into())),
            None
        );
    }
}
