//! The background tasks a turn waits for, among those its Stop reports
//! (`background_tasks[].type`), how to name them, and when the wait is over
//! (BackgroundWork.swift). Claude Code 2.1.x sends a friendly label per
//! running task ("shell", "subagent", "monitor", "workflow", "MCP task",
//! "teammate", "dream", "auto-mode scan", "cloud session"), and the raw
//! discriminant for a kind it has no label for.

use std::time::{Duration, SystemTime};

/// Agents, workflows, teammates and cloud sessions: each wakes Claude for
/// another turn when it finishes, fails or is stopped, so a turn that leaves
/// one running isn't done. They are also what keeps Claude Code's session
/// registry "busy" after the turn. Background shells, monitors and MCP tasks
/// wake Claude too, but a shell or monitor may run for the whole session (a
/// dev server, a file watcher), and the registry doesn't count MCP tasks, so
/// none of them holds a session "working". "dream" and "auto-mode scan" are
/// Claude Code's own housekeeping and never wake it.
pub const AWAITED_TYPES: [&str; 10] = [
    "subagent",
    "agent",
    "local_agent",
    "workflow",
    "local_workflow",
    "teammate",
    "in_process_teammate",
    "cloud session",
    "remote_agent",
    "remote_session",
];

const WORKFLOW_TYPES: [&str; 2] = ["workflow", "local_workflow"];
const TEAMMATE_TYPES: [&str; 2] = ["teammate", "in_process_teammate"];

pub fn is_awaited(task_type: &str) -> bool {
    AWAITED_TYPES.contains(&task_type)
}

/// "1 workflow", "2 background agents", "1 workflow and 1 teammate"; `None`
/// for none. `types` are awaited ones.
pub fn phrase(types: &[String]) -> Option<String> {
    let workflows = types
        .iter()
        .filter(|t| WORKFLOW_TYPES.contains(&t.as_str()))
        .count();
    let teammates = types
        .iter()
        .filter(|t| TEAMMATE_TYPES.contains(&t.as_str()))
        .count();
    let agents = types.len() - workflows - teammates;
    let parts: Vec<String> = [
        counted(workflows, "workflow"),
        counted(agents, "background agent"),
        counted(teammates, "teammate"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let (last, rest) = parts.split_last()?;
    if rest.is_empty() {
        return Some(last.clone());
    }
    Some(format!("{} and {last}", rest.join(", ")))
}

fn counted(count: usize, noun: &str) -> Option<String> {
    (count > 0).then(|| format!("{count} {noun}{}", if count == 1 { "" } else { "s" }))
}

/// Whether a background wait is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitDecision {
    /// Still waiting; decide again after this long if nothing else happens
    /// (`None`: only a registry change or a hook can end it).
    Keep { recheck_in: Option<Duration> },
    /// No awaited work is left (or none has shown a sign of life for a long
    /// time); it ended at `at`.
    End { at: SystemTime },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitTiming {
    /// How long the registry must report no agent work before the wait ends
    /// without Claude being woken. A finished agent's wake-up turn starts
    /// within moments and ends the wait itself, so this only matters when no
    /// wake-up comes.
    pub registry_grace: Duration,
    /// How long without a single hook event (background agents send theirs)
    /// before giving up while the registry still says busy (a paused
    /// workflow keeps it busy and sends nothing) or doesn't follow the
    /// session at all.
    pub quiet_timeout: Duration,
}

impl WaitTiming {
    pub const STANDARD: WaitTiming = WaitTiming {
        registry_grace: Duration::from_secs(10),
        quiet_timeout: Duration::from_secs(30 * 60),
    };
}

impl Default for WaitTiming {
    fn default() -> Self {
        WaitTiming::STANDARD
    }
}

/// Whether a background wait that began at `wait_since` is over. Claude
/// Code keeps the session registry "busy" while the turn runs or any agent,
/// workflow (paused ones too), cloud session or active teammate of the
/// session is unfinished; "shell" when only shells or monitors are left,
/// and "idle" when nothing is. Idle teammates and long-running cloud
/// sessions leave it idle, so a wait on them ends after the grace; their
/// later reports wake Claude for a turn of their own. Pure.
pub fn decide(
    wait_since: SystemTime,
    registry_status: Option<&str>,
    registry_changed_at: Option<SystemTime>,
    last_hook_event_at: Option<SystemTime>,
    now: SystemTime,
    timing: WaitTiming,
) -> WaitDecision {
    let (quiet_since, deadline) = quiet_deadline(
        wait_since,
        registry_status,
        registry_changed_at,
        last_hook_event_at,
        timing,
    );
    if now >= deadline {
        WaitDecision::End { at: quiet_since }
    } else {
        WaitDecision::Keep {
            recheck_in: Some(deadline.duration_since(now).unwrap_or_default()),
        }
    }
}

/// When [`decide`] ends the wait if nothing changes before: the store's
/// next deadline for a wait (the clock alone can end it).
pub fn check_at(
    wait_since: SystemTime,
    registry_status: Option<&str>,
    registry_changed_at: Option<SystemTime>,
    last_hook_event_at: Option<SystemTime>,
    timing: WaitTiming,
) -> SystemTime {
    quiet_deadline(
        wait_since,
        registry_status,
        registry_changed_at,
        last_hook_event_at,
        timing,
    )
    .1
}

/// Since when the wait has had no sign of work, and when that has lasted
/// long enough to end it.
fn quiet_deadline(
    wait_since: SystemTime,
    registry_status: Option<&str>,
    registry_changed_at: Option<SystemTime>,
    last_hook_event_at: Option<SystemTime>,
    timing: WaitTiming,
) -> (SystemTime, SystemTime) {
    let (quiet_since, patience) = match registry_status {
        Some("idle" | "shell") => (
            registry_changed_at.unwrap_or(wait_since).max(wait_since),
            timing.registry_grace,
        ),
        _ => (
            last_hook_event_at.unwrap_or(wait_since).max(wait_since),
            timing.quiet_timeout,
        ),
    };
    (quiet_since, quiet_since + patience)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn phrases() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(phrase(&[]), None);
        assert_eq!(phrase(&s(&["workflow"])), Some("1 workflow".into()));
        assert_eq!(
            phrase(&s(&["subagent", "agent"])),
            Some("2 background agents".into())
        );
        assert_eq!(
            phrase(&s(&["workflow", "teammate"])),
            Some("1 workflow and 1 teammate".into())
        );
        assert_eq!(
            phrase(&s(&[
                "local_workflow",
                "subagent",
                "in_process_teammate",
                "teammate"
            ])),
            Some("1 workflow, 1 background agent and 2 teammates".into())
        );
    }

    #[test]
    fn registry_idle_ends_after_the_grace() {
        let since = UNIX_EPOCH + Duration::from_secs(1000);
        let idle_at = since + Duration::from_secs(5);
        let at = |s: u64| since + Duration::from_secs(s);
        assert_eq!(
            decide(
                since,
                Some("idle"),
                Some(idle_at),
                None,
                at(14),
                WaitTiming::STANDARD
            ),
            WaitDecision::Keep {
                recheck_in: Some(Duration::from_secs(1))
            }
        );
        assert_eq!(
            decide(
                since,
                Some("shell"),
                Some(idle_at),
                None,
                at(15),
                WaitTiming::STANDARD
            ),
            WaitDecision::End { at: idle_at }
        );
        // An idle from before the wait counts from the wait.
        assert_eq!(
            decide(
                since,
                Some("idle"),
                Some(at(0) - Duration::from_secs(60)),
                None,
                at(10),
                WaitTiming::STANDARD
            ),
            WaitDecision::End { at: since }
        );
        // Busy: only the quiet timeout ends it.
        assert_eq!(
            decide(
                since,
                Some("busy"),
                Some(idle_at),
                Some(at(60)),
                at(60 + 1799),
                WaitTiming::STANDARD
            ),
            WaitDecision::Keep {
                recheck_in: Some(Duration::from_secs(1))
            }
        );
        assert_eq!(
            decide(
                since,
                None,
                None,
                Some(at(60)),
                at(60 + 1800),
                WaitTiming::STANDARD
            ),
            WaitDecision::End { at: at(60) }
        );
    }
}
