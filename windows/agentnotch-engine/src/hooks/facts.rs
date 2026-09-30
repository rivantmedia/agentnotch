//! What the committed facts file establishes about Claude Code's builds
//! (DESIGN-WIN §6.2), compiled in.
//!
//! The file, `tests/fixtures/claude-code-facts.json`, is written by the facts
//! job (`windows/scripts/check-claude-code-facts.mjs`, WP11), which downloads
//! Claude Code packages and reads their code without ever running them, and
//! may be backed by the hermetic real-Claude-Code CI job (maintainer decision
//! Q3). A pull request with a regenerated file is reviewed by hand. It is the
//! only source of [`ClaudeCodeFacts::exec_form_min`]: while it doesn't
//! establish one, exec-form hooks are never written and every folder keeps
//! the string form.
//!
//! Schema 1:
//!
//! ```json
//! {
//!   "schema": 1,
//!   "versions": [
//!     { "version": "2.1.282",
//!       "hook_exec_form": true,              // hook entries run {"command", "args"} without a shell
//!       "hook_shell_key": true,              // hook entries honour "shell"
//!       "hook_unknown_keys_rejected": false, // the hook schema refuses unknown keys
//!       "status_line_shell_key": false,      // statusLine honours "shell"
//!       "windows_default_shell": "bash",     // what string-form hooks and status lines run in (R1())
//!       "claude_pid_exported": true,         // hooks get CLAUDE_PID
//!       "bash_transform": "…" }              // the bash-form rewrite (yun), as read
//!   ],
//!   "exec_form_min": null,                   // optional: a minimum proven by running Claude Code
//!   "exec_form_evidence": null               // optional: which job or run proved it
//! }
//! ```
//!
//! Only `version` and `hook_exec_form` decide anything here; the other
//! fields are evidence for the reviewers and for rules that stay
//! conservative until they are established. Unknown fields are ignored, so
//! the job can record more without a code change.

use super::version::ClaudeCodeVersion;
use serde::{Deserialize, Serialize};

/// The committed facts file, compiled in.
pub const FACTS_JSON: &str = include_str!("../../tests/fixtures/claude-code-facts.json");

/// The schema this build reads.
pub const SCHEMA: u32 = 1;

/// One Claude Code version as the facts job read it. Every fact is optional:
/// a missing one is unknown, which never counts as support.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VersionFacts {
    pub version: String,
    pub hook_exec_form: Option<bool>,
    pub hook_shell_key: Option<bool>,
    pub hook_unknown_keys_rejected: Option<bool>,
    pub status_line_shell_key: Option<bool>,
    pub windows_default_shell: Option<String>,
    pub claude_pid_exported: Option<bool>,
    pub bash_transform: Option<String>,
}

/// The facts file as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FactsFile {
    pub schema: u32,
    pub versions: Vec<VersionFacts>,
    pub exec_form_min: Option<String>,
    pub exec_form_evidence: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeFacts {
    /// The first Claude Code version from which every version runs
    /// exec-form hooks (`{"command", "args"}`). `None`: not established,
    /// so exec form is never written.
    pub exec_form_min: Option<String>,
    /// The versions the file lists, oldest first.
    pub versions: Vec<VersionFacts>,
}

impl ClaudeCodeFacts {
    /// The facts this build was compiled with. A file this build can't read
    /// (a newer schema, or broken) establishes nothing.
    pub fn compiled_in() -> ClaudeCodeFacts {
        ClaudeCodeFacts::from_json(FACTS_JSON).unwrap_or_default()
    }

    /// Reads a facts file; `Err` when it isn't one this build understands.
    pub fn from_json(text: &str) -> Result<ClaudeCodeFacts, String> {
        let file: FactsFile =
            serde_json::from_str(text).map_err(|e| format!("the facts file isn't valid: {e}"))?;
        if file.schema != SCHEMA {
            return Err(format!(
                "the facts file has schema {}, this build reads {SCHEMA}",
                file.schema
            ));
        }
        Ok(ClaudeCodeFacts::from_file(file))
    }

    pub fn from_file(file: FactsFile) -> ClaudeCodeFacts {
        let mut versions: Vec<(ClaudeCodeVersion, VersionFacts)> = file
            .versions
            .into_iter()
            .filter_map(|facts| Some((ClaudeCodeVersion::parse(&facts.version)?, facts)))
            .collect();
        versions.sort_by_key(|(version, _)| *version);
        let derived = derive_exec_form_min(&versions);
        let explicit = file
            .exec_form_min
            .as_deref()
            .and_then(ClaudeCodeVersion::parse)
            .filter(|min| !contradicted(*min, &versions));
        // Both sources, when both speak: the later (more conservative) one.
        let exec_form_min = match (derived, explicit) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        ClaudeCodeFacts {
            exec_form_min: exec_form_min.map(|v| v.to_string()),
            versions: versions.into_iter().map(|(_, facts)| facts).collect(),
        }
    }

    /// `EXEC_FORM_MIN`.
    pub fn exec_form_min(&self) -> Option<ClaudeCodeVersion> {
        self.exec_form_min
            .as_deref()
            .and_then(ClaudeCodeVersion::parse)
    }
}

/// The first listed version after which every listed version runs exec
/// form: the newest must run it, and the minimum sits just after the last
/// version that doesn't (or is unknown). Versions between two checked ones
/// weren't read, so the boundary is the first *checked* version known to
/// run it, never a guess below it.
fn derive_exec_form_min(
    versions: &[(ClaudeCodeVersion, VersionFacts)],
) -> Option<ClaudeCodeVersion> {
    let supports = |facts: &VersionFacts| facts.hook_exec_form == Some(true);
    let (_, newest) = versions.last()?;
    if !supports(newest) {
        return None;
    }
    let first_supported = versions
        .iter()
        .rposition(|(_, facts)| !supports(facts))
        .map_or(0, |last_unsupported| last_unsupported + 1);
    versions.get(first_supported).map(|(version, _)| *version)
}

/// A minimum some listed version at or above it says isn't enough.
fn contradicted(min: ClaudeCodeVersion, versions: &[(ClaudeCodeVersion, VersionFacts)]) -> bool {
    versions
        .iter()
        .any(|(version, facts)| *version >= min && facts.hook_exec_form == Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(versions: &[(&str, Option<bool>)], explicit: Option<&str>) -> String {
        let versions: Vec<serde_json::Value> = versions
            .iter()
            .map(|(v, exec)| serde_json::json!({"version": v, "hook_exec_form": exec}))
            .collect();
        serde_json::json!({"schema": 1, "versions": versions, "exec_form_min": explicit})
            .to_string()
    }

    fn min(versions: &[(&str, Option<bool>)], explicit: Option<&str>) -> Option<String> {
        ClaudeCodeFacts::from_json(&file(versions, explicit))
            .unwrap()
            .exec_form_min
    }

    #[test]
    fn the_minimum_is_the_first_checked_version_after_the_last_without() {
        let table = [
            ("2.0.0", Some(false)),
            ("2.1.0", Some(false)),
            ("2.1.60", Some(true)),
            ("2.1.99", Some(true)),
            ("2.1.282", Some(true)),
        ];
        assert_eq!(min(&table, None).as_deref(), Some("2.1.60"));
        // Listed out of order: sorted first.
        let shuffled = [table[4], table[0], table[2], table[1], table[3]];
        assert_eq!(min(&shuffled, None).as_deref(), Some("2.1.60"));
    }

    #[test]
    fn unknown_and_regressions_move_it_up_or_away() {
        // An unknown version counts as "doesn't".
        assert_eq!(
            min(
                &[
                    ("2.1.0", Some(true)),
                    ("2.1.50", None),
                    ("2.1.60", Some(true))
                ],
                None
            )
            .as_deref(),
            Some("2.1.60")
        );
        // The newest doesn't: nothing is established.
        assert_eq!(
            min(&[("2.1.0", Some(true)), ("2.1.60", Some(false))], None),
            None
        );
        assert_eq!(min(&[], None), None);
        // All of them do: the oldest listed.
        assert_eq!(
            min(&[("2.0.0", Some(true)), ("2.1.0", Some(true))], None).as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn an_explicit_minimum_counts_unless_the_table_contradicts_it() {
        // Proven by running Claude Code, no table yet.
        assert_eq!(min(&[], Some("2.1.200")).as_deref(), Some("2.1.200"));
        // With a table: the later of the two.
        let table = [("2.1.0", Some(false)), ("2.1.60", Some(true))];
        assert_eq!(min(&table, Some("2.1.100")).as_deref(), Some("2.1.100"));
        assert_eq!(min(&table, Some("2.1.10")).as_deref(), Some("2.1.60"));
        // A listed version above it that doesn't run exec form: ignored.
        let bad = [("2.1.150", Some(false)), ("2.1.160", Some(true))];
        assert_eq!(min(&bad, Some("2.1.100")).as_deref(), Some("2.1.160"));
        assert_eq!(min(&[("2.1.150", Some(false))], Some("2.1.100")), None);
        // Not a version: ignored.
        assert_eq!(min(&[], Some("soon")), None);
    }

    #[test]
    fn a_file_this_build_cannot_read_establishes_nothing() {
        assert!(ClaudeCodeFacts::from_json("{").is_err());
        assert!(ClaudeCodeFacts::from_json(r#"{"schema": 2, "exec_form_min": "2.0.0"}"#).is_err());
        // Unknown fields are fine.
        let facts = ClaudeCodeFacts::from_json(
            r#"{"schema": 1, "versions": [{"version": "2.1.0", "hook_exec_form": true, "new_fact": 1}], "later": true}"#,
        )
        .unwrap();
        assert_eq!(facts.exec_form_min.as_deref(), Some("2.1.0"));
        // A version that isn't one is dropped.
        let facts = ClaudeCodeFacts::from_json(
            r#"{"schema": 1, "versions": [{"version": "next", "hook_exec_form": false}, {"version": "2.1.0", "hook_exec_form": true}]}"#,
        )
        .unwrap();
        assert_eq!(facts.exec_form_min.as_deref(), Some("2.1.0"));
        assert_eq!(facts.versions.len(), 1);
    }
}
