//! Tool inputs as display strings (HookEvent.swift's `ToolInput`), shared by
//! the hook path and the transcript path so both describe a call alike.

use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Top-level scalars as strings: strings as they are, booleans as
/// `true`/`false`, numbers as written (an integer without a fraction, a
/// double as Swift prints it: `2.0`, `0.5`). Arrays, objects and null are
/// left out.
pub fn flatten(input: &Map<String, Value>) -> BTreeMap<String, String> {
    input
        .iter()
        .filter_map(|(key, value)| scalar_string(value).map(|text| (key.clone(), text)))
        .collect()
}

/// `flatten` of any JSON value (a non-object has no fields).
pub fn flatten_value(input: &Value) -> BTreeMap<String, String> {
    input.as_object().map(flatten).unwrap_or_default()
}

/// A scalar JSON value as text; `None` for arrays, objects and null.
pub fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(if *flag { "true" } else { "false" }.to_owned()),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Some(int.to_string())
            } else if let Some(uint) = number.as_u64() {
                Some(uint.to_string())
            } else {
                number.as_f64().map(swift_double)
            }
        }
        _ => None,
    }
}

/// A double as Swift's `String(Double)` writes it: a whole value keeps its
/// `.0`, so a float stays recognisable as one on every path.
pub fn swift_double(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// The last part of a path in either separator style (a transcript written
/// on Windows names `C:\x\y.ts`, one copied from elsewhere `/x/y.ts`).
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(index) => &trimmed[index + 1..],
        None => trimmed,
    }
}

fn capped(text: Option<&str>, max_length: Option<usize>) -> Option<String> {
    let text = text.filter(|t| !t.is_empty())?;
    match max_length {
        Some(max) if text.chars().count() > max => {
            Some(text.chars().take(max).collect::<String>() + "...")
        }
        _ => Some(text.to_owned()),
    }
}

/// The one-line preview of a tool call: the file name for file tools, the
/// command, pattern, URL, query or description where a tool has one, else
/// the first telling string. Capped at `max_length` characters plus "...".
pub fn preview(
    tool_name: &str,
    input: &BTreeMap<String, String>,
    max_length: Option<usize>,
) -> Option<String> {
    let get = |key: &str| input.get(key).map(String::as_str);
    let text: Option<&str> = match tool_name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => get("file_path")
            .or_else(|| get("notebook_path"))
            .map(file_name),
        "Bash" => get("command"),
        "Grep" | "Glob" => get("pattern"),
        "Task" | "Agent" => get("description"),
        "WebFetch" => get("url"),
        "WebSearch" => get("query"),
        _ => ["command", "file_path", "path", "query", "pattern", "url"]
            .iter()
            .find_map(|key| get(key))
            .or_else(|| {
                // BTreeMap iterates keys sorted, as the Mac sorts them.
                input
                    .iter()
                    .filter(|(key, _)| key.as_str() != "description")
                    .map(|(_, value)| value.as_str())
                    .find(|value| !value.is_empty())
            }),
    };
    capped(text, max_length)
}

/// The preview that may leave the panel (hover rows, banners): only named
/// fields — the command, a file's name, a search pattern or query, a URL's
/// host — never a free text field an MCP or unknown tool took (a message
/// body, a channel, a description Claude wrote). `None` means "the tool's
/// name alone".
pub fn off_panel_preview(
    tool_name: &str,
    input: &BTreeMap<String, String>,
    max_length: Option<usize>,
) -> Option<String> {
    let get = |key: &str| input.get(key).map(String::as_str);
    let host;
    let text: Option<&str> = match tool_name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => get("file_path")
            .or_else(|| get("notebook_path"))
            .map(file_name),
        "Bash" => get("command"),
        "Grep" | "Glob" => get("pattern"),
        "WebFetch" => {
            host = get("url")
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(str::to_owned));
            host.as_deref()
        }
        "WebSearch" => get("query"),
        _ => None,
    };
    capped(text, max_length)
}

/// Text Claude Code puts in the user turn itself: background-task
/// notifications, slash-command echoes and caveats.
pub const INJECTED_PREFIXES: [&str; 7] = [
    "<task-notification>",
    "<command-name>",
    "<command-message>",
    "<local-command",
    "<bash-input>",
    "<bash-stdout>",
    "Caveat:",
];

pub fn is_injected_prompt(prompt: Option<&str>) -> bool {
    let Some(prompt) = prompt else {
        return false;
    };
    let prompt = prompt.trim_start();
    INJECTED_PREFIXES
        .iter()
        .any(|prefix| prompt.starts_with(prefix))
}

/// "Task" is the legacy name of the subagent container; Claude Code now
/// uses "Agent".
pub fn is_subagent_container(name: &str) -> bool {
    name == "Task" || name == "Agent"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flat(value: Value) -> BTreeMap<String, String> {
        flatten_value(&value)
    }

    #[test]
    fn flattening_keeps_scalars_as_the_mac_prints_them() {
        let input = flat(json!({
            "command": "npm test", "timeout": 120, "ratio": 0.5, "whole": 2.0,
            "run_in_background": true, "todos": [1], "nested": {"a": 1}, "none": null
        }));
        assert_eq!(input.get("command").unwrap(), "npm test");
        assert_eq!(input.get("timeout").unwrap(), "120");
        assert_eq!(input.get("ratio").unwrap(), "0.5");
        assert_eq!(input.get("whole").unwrap(), "2.0");
        assert_eq!(input.get("run_in_background").unwrap(), "true");
        assert!(!input.contains_key("todos"));
        assert!(!input.contains_key("nested"));
        assert!(!input.contains_key("none"));
    }

    #[test]
    fn previews() {
        let read = flat(json!({"file_path": "C:\\Users\\me\\code\\app\\src\\main.rs"}));
        assert_eq!(preview("Read", &read, None).unwrap(), "main.rs");
        let posix = flat(json!({"file_path": "/tmp/x/y.ts"}));
        assert_eq!(preview("Edit", &posix, None).unwrap(), "y.ts");
        let bash = flat(json!({"command": "a".repeat(120), "description": "d"}));
        assert_eq!(
            preview("Bash", &bash, Some(100)).unwrap(),
            "a".repeat(100) + "..."
        );
        let mcp = flat(json!({"channel": "#general", "text": "hi", "description": "x"}));
        assert_eq!(preview("mcp__slack__post", &mcp, None).unwrap(), "#general");
        assert_eq!(off_panel_preview("mcp__slack__post", &mcp, None), None);
        let fetch = flat(json!({"url": "https://example.com/a?b=c"}));
        assert_eq!(
            off_panel_preview("WebFetch", &fetch, None).unwrap(),
            "example.com"
        );
        assert_eq!(preview("Bash", &BTreeMap::new(), None), None);
    }

    #[test]
    fn injected_prompts() {
        assert!(is_injected_prompt(Some(
            "  <task-notification><task-id>a</task-id></task-notification>"
        )));
        assert!(is_injected_prompt(Some("Caveat: the messages below")));
        assert!(!is_injected_prompt(Some("fix the build")));
        assert!(!is_injected_prompt(None));
    }

    #[test]
    fn file_names_in_both_styles() {
        assert_eq!(file_name("C:\\a\\b.txt"), "b.txt");
        assert_eq!(file_name("/a/b.txt"), "b.txt");
        assert_eq!(file_name("b.txt"), "b.txt");
        assert_eq!(file_name("C:/a/dir/"), "dir");
    }
}
