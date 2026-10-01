//! The small text rules banners, hover rows and typed replies share: one
//! line, a length limit, a tool's display name, and the only parts of a
//! tool's input that may be shown outside the panel.
//!
//! Lengths count characters as the Mac does (Swift's `Character`: a
//! grapheme cluster), so a family emoji or a letter with combining marks is
//! one, and a cut never lands inside one.

use serde_json::Value;
use std::collections::BTreeMap;
use unicode_segmentation::UnicodeSegmentation;

/// Foundation's `CharacterSet.whitespaces`: the space separators and tab,
/// never a line break.
fn is_inline_space(c: char) -> bool {
    matches!(
        c,
        '\t' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Whitespace runs (line breaks included) become single spaces; nothing
/// leads or trails.
pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Shortens to `limit` characters, the last one an ellipsis.
pub fn truncated(text: &str, limit: usize) -> String {
    if limit <= 1 {
        return text.to_owned();
    }
    let mut graphemes = text.grapheme_indices(true);
    let Some((cut, _)) = graphemes.nth(limit - 1) else {
        // `limit - 1` characters or fewer.
        return text.to_owned();
    };
    if graphemes.next().is_none() {
        // Exactly `limit`.
        return text.to_owned();
    }
    format!("{}…", text[..cut].trim_matches(is_inline_space))
}

/// One line of at most `limit` characters.
pub fn collapsed(text: &str, limit: usize) -> String {
    truncated(&collapse_whitespace(text), limit)
}

/// One line; `None` when nothing is left.
pub fn preview(text: Option<&str>) -> Option<String> {
    let line = collapse_whitespace(text?);
    (!line.is_empty()).then_some(line)
}

/// A tool's name as rows and banners show it: a friendlier alias for a few
/// built-in tools, and "Server - Tool Name" for an MCP tool
/// (`mcp__server__tool_name`).
pub fn format_tool_name(tool: &str) -> String {
    let alias = match tool {
        "AgentOutputTool" => Some("Await Agent"),
        "AskUserQuestion" => Some("Question"),
        "TodoWrite" | "TodoRead" => Some("Todo"),
        "WebFetch" => Some("Fetch"),
        "WebSearch" => Some("Search"),
        "NotebookEdit" => Some("Notebook"),
        "BashOutput" => Some("Bash"),
        "KillShell" => Some("Shell"),
        "EnterPlanMode" | "ExitPlanMode" => Some("Plan"),
        "SlashCommand" => Some("Command"),
        _ => None,
    };
    if let Some(alias) = alias {
        return alias.to_owned();
    }
    let Some(rest) = tool.strip_prefix("mcp__") else {
        return tool.to_owned();
    };
    // The server is everything up to the first underscore; what follows its
    // second underscore is the tool.
    let rest = rest.trim_start_matches('_');
    if rest.is_empty() {
        return tool.to_owned();
    }
    let (server, tool_part) = match rest.split_once('_') {
        Some((server, tail)) => (server, Some(tail.strip_prefix('_').unwrap_or(tail))),
        None => (rest, None),
    };
    let server = title_case(server);
    match tool_part {
        Some(name) => format!("{server} - {}", title_case(name)),
        None => server,
    }
}

/// `ask_question` → `Ask Question`.
fn title_case(snake: &str) -> String {
    snake
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let first: String = chars
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            first + &chars.as_str().to_lowercase()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A tool input's top-level scalars as text; arrays, objects and nulls are
/// left out.
pub fn flat_input(input: &Value) -> BTreeMap<String, String> {
    let mut flat = BTreeMap::new();
    if let Some(object) = input.as_object() {
        for (key, value) in object {
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Bool(flag) => flag.to_string(),
                Value::Number(number) => number.to_string(),
                _ => continue,
            };
            flat.insert(key.clone(), text);
        }
    }
    flat
}

/// The longest one-line preview of a tool input.
pub const PREVIEW_LENGTH: usize = 100;

/// The last component of a path written with either separator.
fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(index) => &trimmed[index + 1..],
        None if trimmed.is_empty() => path,
        None => trimmed,
    }
}

/// What may be shown of a tool's input outside the panel (hover rows,
/// banners, which Windows also keeps in its notification centre): only a
/// named field, the command, a file's name, a search pattern or query, a
/// URL's host. Never a free-text field an MCP or unknown tool took (a
/// message body, a channel, a description Claude wrote). `None` means "the
/// tool's name alone".
pub fn off_panel_preview(tool: &str, input: &Value, max_length: Option<usize>) -> Option<String> {
    let flat = flat_input(input);
    let field = |key: &str| flat.get(key).cloned();
    let text = match tool {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => field("file_path")
            .or_else(|| field("notebook_path"))
            .map(|path| file_name(&path).to_owned()),
        "Bash" => field("command"),
        "Grep" | "Glob" => field("pattern"),
        "WebFetch" => field("url").and_then(|text| {
            url::Url::parse(&text)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
        }),
        "WebSearch" => field("query"),
        _ => None,
    }?;
    if text.is_empty() {
        return None;
    }
    match max_length {
        Some(max) if text.graphemes(true).count() > max => {
            let cut: String = text.graphemes(true).take(max).collect();
            Some(format!("{cut}..."))
        }
        _ => Some(text),
    }
}

/// A message as one line: Claude Code submits on every line break it
/// receives, so line breaks and tabs become spaces and control characters
/// go.
pub fn single_line(message: &str) -> String {
    let mut line = String::with_capacity(message.len());
    for c in message.chars() {
        match c {
            '\n' | '\r' | '\t' => line.push(' '),
            // Foundation's `controlCharacters`: the Cc and Cf categories.
            // Cf holds the bidi overrides and zero-width marks, which must
            // not reach a prompt unseen either.
            c if c.is_control() || is_format_character(c) => {}
            c => line.push(c),
        }
    }
    line.trim_matches(is_inline_space).to_owned()
}

/// Unicode's Cf (format) characters: invisible, and able to reorder or hide
/// the text around them.
fn is_format_character(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// SessionNotificationContentTests.helpers.
    #[test]
    fn truncation_and_previews() {
        assert_eq!(truncated("abcdef", 4), "abc…");
        assert_eq!(truncated("abc", 4), "abc");
        assert_eq!(truncated("abcd", 4), "abcd");
        assert_eq!(preview(Some("  \n ")), None);
        assert_eq!(preview(None), None);
        assert_eq!(
            preview(Some("npm   run\n build")).as_deref(),
            Some("npm run build")
        );
        // The cut never leaves a space before the ellipsis.
        assert_eq!(truncated("ab cdef", 4), "ab…");
        assert_eq!(collapsed("  Two\n lines ", usize::MAX), "Two lines");
        // A character of several scalars counts once and is never split.
        let family = "👨‍👩‍👧‍👦";
        assert_eq!(truncated(&format!("a{family}bc"), 3), format!("a{family}…"));
    }

    /// MCPToolFormatter.
    #[test]
    fn tool_names() {
        assert_eq!(format_tool_name("Bash"), "Bash");
        assert_eq!(format_tool_name("AskUserQuestion"), "Question");
        assert_eq!(format_tool_name("ExitPlanMode"), "Plan");
        assert_eq!(format_tool_name("WebFetch"), "Fetch");
        assert_eq!(
            format_tool_name("mcp__deepwiki__ask_question"),
            "Deepwiki - Ask Question"
        );
        assert_eq!(
            format_tool_name("mcp__github__list_issues"),
            "Github - List Issues"
        );
        assert_eq!(format_tool_name("mcp__slack"), "Slack");
        assert_eq!(format_tool_name("mcp__"), "mcp__");
    }

    /// TerminalScriptTests.singleLineCollapsesLineBreaksAndDropsControls.
    #[test]
    fn single_line_collapses_line_breaks_and_drops_controls() {
        assert_eq!(single_line("fix the\nbug\r\nnow"), "fix the bug  now");
        assert_eq!(single_line("tab\there"), "tab here");
        assert_eq!(single_line("bell\u{07}ring\u{1B}[0m"), "bellring[0m");
        assert_eq!(single_line("  padded  "), "padded");
        // A right-to-left override would show one command and send another.
        assert_eq!(single_line("rm \u{202E}fdp.x"), "rm fdp.x");
        assert_eq!(single_line("héllo wörld ✓"), "héllo wörld ✓");
        assert_eq!(single_line(" \n\t "), "");
    }

    #[test]
    fn flat_input_keeps_scalars() {
        let flat = flat_input(&json!({"command": "ls", "timeout": 5, "background": true,
            "nested": {"a": 1}, "list": [1], "nothing": null}));
        assert_eq!(flat.get("command").map(String::as_str), Some("ls"));
        assert_eq!(flat.get("timeout").map(String::as_str), Some("5"));
        assert_eq!(flat.get("background").map(String::as_str), Some("true"));
        assert_eq!(flat.len(), 3);
        assert!(flat_input(&json!("text")).is_empty());
    }
}
