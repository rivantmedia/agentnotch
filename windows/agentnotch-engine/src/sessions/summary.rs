//! What a transcript says about its session (ConversationParser.swift's
//! `TranscriptSummary`): titles, the last message, the first prompt, the
//! context size of the latest response, and how the latest turn ended. A
//! forward fold over [`TranscriptEntry`]s: the sync job reads the lines and
//! keeps no state, the store feeds them here in order, and the result is the
//! same whether they arrive in one read or many.
//!
//! The Mac's running token totals are not kept: nothing on Windows shows
//! them (the cloud counts tokens with its own scanner).

use crate::runtime_types::TranscriptEntry;
use crate::sessions::tool_input;
use std::time::SystemTime;

/// How the transcript's latest turn ended, from main-chain entries only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptTurn {
    /// Text of Claude's last reply that had text, and when it was written.
    pub reply_text: Option<String>,
    pub reply_at: Option<SystemTime>,
    /// When a person last typed a prompt (not a task notification, compact
    /// summary, tool result, command echo or meta entry).
    pub human_prompt_at: Option<SystemTime>,
    /// When the user last interrupted Claude.
    pub interrupted_at: Option<SystemTime>,
    /// Nothing came after the last reply: no tool call, tool result, prompt,
    /// wake-up or interrupt. A turn that ended this way finished.
    pub ends_with_reply: bool,
}

impl TranscriptTurn {
    /// The last reply, when it is a finished turn later than `after` (`None`:
    /// no bound) and than anything the user did after it.
    pub fn finished_turn(&self, after: Option<SystemTime>) -> Option<(Option<&str>, SystemTime)> {
        if !self.ends_with_reply {
            return None;
        }
        let reply_at = self.reply_at?;
        let newer = [after, self.human_prompt_at, self.interrupted_at]
            .into_iter()
            .flatten()
            .all(|bound| reply_at > bound);
        newer.then_some((self.reply_text.as_deref(), reply_at))
    }
}

/// The summary as the session shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConversationInfo {
    /// The transcript's `summary` line.
    pub summary: Option<String>,
    /// The last message, one line of at most 80 characters.
    pub last_message: Option<String>,
    /// `user` | `assistant` | `tool`.
    pub last_message_role: Option<String>,
    /// The tool, when the role is `tool`.
    pub last_tool_name: Option<String>,
    /// The first prompt (50 characters): the title until there is one.
    pub first_user_message: Option<String>,
    /// When a person last typed a prompt.
    pub last_user_message_at: Option<SystemTime>,
    /// `custom-title` (set with /rename), else `ai-title`.
    pub title: Option<String>,
    /// Context size of the latest main-thread response: input + cache
    /// creation + cache read tokens.
    pub last_context_tokens: Option<u64>,
    /// Model id of the latest main-thread response.
    pub last_model: Option<String>,
    pub last_turn: TranscriptTurn,
}

/// The fold itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptSummary {
    summary: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    last_message: Option<String>,
    last_message_role: Option<&'static str>,
    last_tool_name: Option<String>,
    first_user_message: Option<String>,
    last_user_message_at: Option<SystemTime>,
    last_context_tokens: Option<u64>,
    last_model: Option<String>,
    turn: TranscriptTurn,
}

impl TranscriptSummary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn turn(&self) -> &TranscriptTurn {
        &self.turn
    }

    pub fn info(&self) -> ConversationInfo {
        ConversationInfo {
            summary: self.summary.clone(),
            last_message: self.last_message.as_deref().map(|m| truncate(m, 80)),
            last_message_role: self.last_message_role.map(str::to_owned),
            last_tool_name: self.last_tool_name.clone(),
            first_user_message: self.first_user_message.clone(),
            last_user_message_at: self.last_user_message_at,
            title: self.custom_title.clone().or_else(|| self.ai_title.clone()),
            last_context_tokens: self.last_context_tokens,
            last_model: self.last_model.clone(),
            last_turn: self.turn.clone(),
        }
    }

    /// One entry of the session's own transcript, in file order. Chat
    /// entries (`Message`, `ToolOutput`) and `Clear` say nothing here: a
    /// /clear keeps the titles and the turn (its own line is an `Injected`).
    pub fn apply(&mut self, entry: &TranscriptEntry) {
        match entry {
            TranscriptEntry::Title { kind, text } if !text.is_empty() => match kind.as_str() {
                "summary" => self.summary = Some(text.clone()),
                "ai" => self.ai_title = Some(text.clone()),
                "custom" => self.custom_title = Some(text.clone()),
                _ => {}
            },
            TranscriptEntry::Assistant {
                at,
                text,
                model,
                usage,
                sidechain,
                synthetic,
                ..
            } => {
                if *sidechain {
                    return;
                }
                // Claude Code repeats a response's usage on the line of each
                // of its content blocks: the latest is the context now.
                if let (Some(usage), false) = (usage, synthetic) {
                    self.last_context_tokens =
                        Some(usage.input + usage.cache_creation + usage.cache_read);
                    if let Some(model) = model.as_deref().filter(|m| !m.is_empty()) {
                        self.last_model = Some(model.to_owned());
                    }
                }
                if let Some(text) = text {
                    self.last_message = Some(text.clone());
                    self.last_message_role = Some("assistant");
                    self.last_tool_name = None;
                    self.record_reply(text, *at);
                }
            }
            TranscriptEntry::HumanPrompt { at, .. } => {
                if let Some(at) = at {
                    self.last_user_message_at = Some(*at);
                    self.turn.human_prompt_at = Some(*at);
                }
                self.turn.ends_with_reply = false;
            }
            TranscriptEntry::PromptText { text } => {
                self.last_message = Some(text.clone());
                self.last_message_role = Some("user");
                self.last_tool_name = None;
                if self.first_user_message.is_none() {
                    self.first_user_message = Some(truncate(text, 50));
                }
            }
            // A wake-up (task notification), compact summary or other
            // injected entry: Claude works on, and it isn't anyone's words.
            TranscriptEntry::Injected { .. } => self.turn.ends_with_reply = false,
            TranscriptEntry::Interrupt { at } => {
                if at.is_some() {
                    self.turn.interrupted_at = *at;
                }
                self.turn.ends_with_reply = false;
            }
            TranscriptEntry::ToolUse { name, input, .. } => {
                self.last_message = Some(
                    tool_input::preview(name, &tool_input::flatten_value(input), None)
                        .unwrap_or_default(),
                );
                self.last_message_role = Some("tool");
                self.last_tool_name = Some(name.clone());
                self.turn.ends_with_reply = false;
            }
            // Claude asked for a tool; the turn goes on.
            TranscriptEntry::ToolResult { .. } => self.turn.ends_with_reply = false,
            _ => {}
        }
    }

    fn record_reply(&mut self, text: &str, at: Option<SystemTime>) {
        if text.trim().is_empty() {
            return;
        }
        self.turn.reply_text = Some(text.to_owned());
        if at.is_some() {
            self.turn.reply_at = at;
        }
        self.turn.ends_with_reply = true;
    }
}

/// One line: trimmed, newlines as spaces, cut to `max_length` characters
/// with "..." (the three dots count).
pub fn truncate(message: &str, max_length: usize) -> String {
    let cleaned = message.trim().replace('\n', " ");
    if cleaned.chars().count() > max_length {
        let kept: String = cleaned.chars().take(max_length.saturating_sub(3)).collect();
        return kept + "...";
    }
    cleaned
}

/// Slash-command echoes and caveats aren't conversation text.
pub fn is_command_text(text: &str) -> bool {
    text.starts_with("<command-name>")
        || text.starts_with("<local-command")
        || text.starts_with("Caveat:")
}

/// What Claude Code writes in the user turn when the user presses Esc.
pub const INTERRUPT_MARKER: &str = "[Request interrupted by user";

pub fn is_interrupt_marker(text: &str) -> bool {
    text.starts_with(INTERRUPT_MARKER)
}

/// The context window of a session without status line data.
pub mod context {
    pub const STANDARD_WINDOW: u64 = 200_000;
    pub const EXTENDED_WINDOW: u64 = 1_000_000;

    /// The status line's size when known, else 1M for "[1m]" models or when
    /// the context already exceeds 200k, else 200k.
    pub fn window_size(
        context_tokens: u64,
        status_line_window_size: Option<u64>,
        model_ids: &[Option<&str>],
    ) -> u64 {
        if let Some(size) = status_line_window_size.filter(|size| *size > 0) {
            return size;
        }
        let extended = model_ids
            .iter()
            .flatten()
            .any(|model| model.to_lowercase().contains("[1m]"));
        if extended || context_tokens > STANDARD_WINDOW {
            EXTENDED_WINDOW
        } else {
            STANDARD_WINDOW
        }
    }

    /// Percent 0..=100 of the window used by `context_tokens`.
    pub fn percent(
        context_tokens: u64,
        status_line_window_size: Option<u64>,
        model_ids: &[Option<&str>],
    ) -> f64 {
        let window = window_size(context_tokens, status_line_window_size, model_ids);
        (context_tokens as f64 / window as f64 * 100.0).clamp(0.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_types::TokenUsage;
    use serde_json::json;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(seconds: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds))
    }

    fn reply(text: &str, seconds: u64) -> TranscriptEntry {
        TranscriptEntry::Assistant {
            uuid: "a".into(),
            at: at(seconds),
            text: Some(text.into()),
            model: None,
            usage: None,
            sidechain: false,
            synthetic: false,
        }
    }

    fn response(model: &str, input: u64, cache_read: u64, cache_creation: u64) -> TranscriptEntry {
        TranscriptEntry::Assistant {
            uuid: "a".into(),
            at: None,
            text: None,
            model: Some(model.into()),
            usage: Some(TokenUsage {
                input,
                output: 5,
                cache_creation,
                cache_read,
            }),
            sidechain: false,
            synthetic: model == "<synthetic>",
        }
    }

    fn fold(entries: &[TranscriptEntry]) -> TranscriptSummary {
        let mut summary = TranscriptSummary::new();
        entries.iter().for_each(|entry| summary.apply(entry));
        summary
    }

    // TranscriptTests.usageIsCountedOncePerResponse / syntheticAndSidechainMessagesAreSkipped
    // (the context of the latest real response; no running totals here).
    #[test]
    fn the_latest_real_response_gives_the_context_and_model() {
        let info = fold(&[
            response("claude-opus-4-5", 10, 1000, 200),
            response("claude-opus-4-5", 10, 1000, 200),
            response("claude-opus-4-5", 3, 1200, 0),
        ])
        .info();
        assert_eq!(info.last_context_tokens, Some(3 + 1200));
        assert_eq!(info.last_model.as_deref(), Some("claude-opus-4-5"));

        let mut side = response("claude-haiku", 999, 999, 0);
        if let TranscriptEntry::Assistant { sidechain, .. } = &mut side {
            *sidechain = true;
        }
        let info = fold(&[
            response("claude-opus-4-5", 10, 0, 0),
            response("<synthetic>", 0, 0, 0),
            side,
        ])
        .info();
        assert_eq!(info.last_context_tokens, Some(10));
        assert_eq!(info.last_model.as_deref(), Some("claude-opus-4-5"));
    }

    // TranscriptTests.titlesPreferCustomOverAI
    #[test]
    fn titles_prefer_custom_over_ai() {
        let title = |kind: &str, text: &str| TranscriptEntry::Title {
            kind: kind.into(),
            text: text.into(),
        };
        assert_eq!(
            fold(&[title("ai", "Fix login bug")])
                .info()
                .title
                .as_deref(),
            Some("Fix login bug")
        );
        let both = fold(&[title("custom", "My rename"), title("ai", "Later AI title")]).info();
        assert_eq!(both.title.as_deref(), Some("My rename"));
        assert_eq!(
            fold(&[title("summary", "Login work")])
                .info()
                .summary
                .as_deref(),
            Some("Login work")
        );
    }

    // TranscriptTests.tracksMessagesAndSkipsCommands
    #[test]
    fn tracks_messages() {
        let info = fold(&[
            TranscriptEntry::Injected { at: None },
            TranscriptEntry::HumanPrompt {
                uuid: "u".into(),
                at: None,
            },
            TranscriptEntry::PromptText {
                text: "Please fix the login flow".into(),
            },
            TranscriptEntry::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command": "npm test"}),
            },
        ])
        .info();
        assert_eq!(
            info.first_user_message.as_deref(),
            Some("Please fix the login flow")
        );
        assert_eq!(info.last_message.as_deref(), Some("npm test"));
        assert_eq!(info.last_message_role.as_deref(), Some("tool"));
        assert_eq!(info.last_tool_name.as_deref(), Some("Bash"));
    }

    // TranscriptTests.contextEstimateWindow
    #[test]
    fn context_estimate_window() {
        use context::*;
        assert_eq!(
            window_size(50_000, None, &[Some("claude-opus-4-5")]),
            200_000
        );
        assert_eq!(
            window_size(50_000, None, &[Some("claude-opus-4-5[1m]")]),
            1_000_000
        );
        assert_eq!(window_size(250_000, None, &[None]), 1_000_000);
        assert_eq!(window_size(50_000, Some(400_000), &[]), 400_000);
        assert_eq!(percent(50_000, None, &[]), 25.0);
    }

    // A1_TranscriptParserTests.aTurnEndsWithClaudesReply
    #[test]
    fn a_turn_ends_with_claudes_reply() {
        let summary = fold(&[
            TranscriptEntry::HumanPrompt {
                uuid: "u".into(),
                at: at(0),
            },
            TranscriptEntry::PromptText {
                text: "do it".into(),
            },
            TranscriptEntry::ToolUse {
                id: "x".into(),
                name: "Bash".into(),
                input: json!({}),
            },
            TranscriptEntry::ToolResult {
                tool_use_id: "x".into(),
                status: "success".into(),
                task_id: None,
            },
            reply("All done.", 3),
        ]);
        let turn = summary.turn();
        assert!(turn.ends_with_reply);
        assert_eq!(turn.reply_text.as_deref(), Some("All done."));
        assert_eq!(turn.human_prompt_at, at(0));
        assert_eq!(turn.finished_turn(at(1)).map(|f| f.1), at(3));
        assert_eq!(turn.finished_turn(at(5)), None);
    }

    // A1_TranscriptParserTests.interruptsToolCallsAndWakeUpsAreNotAFinishedTurn
    #[test]
    fn interrupts_tool_calls_and_wake_ups_are_not_a_finished_turn() {
        let working = reply("Working on it", 0);
        let ends = |after: TranscriptEntry| fold(&[working.clone(), after]).turn().ends_with_reply;
        assert!(fold(std::slice::from_ref(&working)).turn().ends_with_reply);
        assert!(!ends(TranscriptEntry::Interrupt { at: at(1) }));
        assert!(!ends(TranscriptEntry::ToolUse {
            id: "y".into(),
            name: "Read".into(),
            input: json!({}),
        }));
        assert!(!ends(TranscriptEntry::Injected { at: at(1) }));
        // A blank reply is no reply.
        assert!(!fold(&[reply("  \n", 0)]).turn().ends_with_reply);
    }

    #[test]
    fn a_reply_older_than_the_prompt_or_the_interrupt_is_not_finished() {
        let prompted = fold(&[
            reply("Done.", 1),
            TranscriptEntry::HumanPrompt {
                uuid: "u".into(),
                at: at(2),
            },
            reply("Done again.", 3),
        ]);
        assert_eq!(prompted.turn().finished_turn(None).map(|f| f.1), at(3));
        let mut stale = prompted.clone();
        stale.apply(&TranscriptEntry::Interrupt { at: at(9) });
        stale.turn.ends_with_reply = true;
        assert_eq!(stale.turn().finished_turn(None), None);
    }

    #[test]
    fn truncation_counts_characters_and_flattens_lines() {
        assert_eq!(truncate("  two\nlines  ", 80), "two lines");
        assert_eq!(truncate(&"é".repeat(90), 80), "é".repeat(77) + "...");
        assert_eq!(truncate(&"a".repeat(80), 80), "a".repeat(80));
    }

    #[test]
    fn command_text_and_interrupt_markers() {
        assert!(is_command_text("<command-name>/model</command-name>"));
        assert!(is_command_text("<local-command-stdout>x"));
        assert!(is_command_text("Caveat: the messages below"));
        assert!(!is_command_text("fix <command-name>"));
        assert!(is_interrupt_marker(
            "[Request interrupted by user for tool use]"
        ));
        assert!(!is_interrupt_marker(
            "It printed [Request interrupted by user]"
        ));
    }
}
