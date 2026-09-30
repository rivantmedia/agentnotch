//! A transcript line's `toolUseResult` as the chat draws it for each tool
//! (ToolResultParser.swift, ToolResultData.swift): file reads, diffs, command
//! output, search hits, task results. Pure.
//!
//! The panel is another process here, so what it can't show is left out:
//! every text is cut to [`MAX_RESULT_TEXT`] characters and a diff to
//! [`MAX_DIFF_LINES`] lines (the page cuts further when it draws them).

use crate::model::{
    DiffKind, DiffLine, Question, QuestionOption, SearchResultView, TodoView, ToolResultView,
};
use crate::sessions::tool_input;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The longest text a result carries to the panel, in characters.
pub const MAX_RESULT_TEXT: usize = 16 * 1024;
/// The most lines of a diff sent to the panel.
pub const MAX_DIFF_LINES: usize = 200;
/// File names, search results and todos listed at most.
pub const MAX_LIST_ITEMS: usize = 200;

type Object = Map<String, Value>;

fn text(data: &Object, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).map(clamp)
}

fn text_or_empty(data: &Object, key: &str) -> String {
    text(data, key).unwrap_or_default()
}

fn flag(data: &Object, key: &str) -> bool {
    data.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A whole number (`5`, or `5.0` as Swift's `as? Int` takes it).
fn int(data: &Object, key: &str) -> Option<i64> {
    let number = data.get(key)?.as_number()?;
    number.as_i64().or_else(|| {
        number
            .as_f64()
            .filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15)
            .map(|f| f as i64)
    })
}

fn count(data: &Object, key: &str) -> u32 {
    int(data, key).map_or(0, |n| n.clamp(0, u32::MAX as i64) as u32)
}

fn strings(data: &Object, key: &str) -> Vec<String> {
    data.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .take(MAX_LIST_ITEMS)
                .map(clamp)
                .collect()
        })
        .unwrap_or_default()
}

/// The first [`MAX_RESULT_TEXT`] characters.
pub fn clamp(text: &str) -> String {
    match text.char_indices().nth(MAX_RESULT_TEXT) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

/// Every nested string cut to 2000 characters and every list to 50 items:
/// an MCP result is shown as a few key/value pairs, never whole.
fn trimmed(value: &Value) -> Value {
    match value {
        Value::String(s) => match s.char_indices().nth(2000) {
            Some((end, _)) => Value::String(s[..end].to_owned()),
            None => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().take(50).map(trimmed).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), trimmed(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// One hunk of Claude Code's `structuredPatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Each with its `+`, `-` or space prefix.
    pub lines: Vec<String>,
}

fn hunks(data: &Object) -> Vec<PatchHunk> {
    let Some(Value::Array(patches)) = data.get("structuredPatch") else {
        return Vec::new();
    };
    patches
        .iter()
        .filter_map(|patch| {
            let patch = patch.as_object()?;
            let number = |key: &str| int(patch, key).and_then(|n| u32::try_from(n).ok());
            let lines = patch
                .get("lines")?
                .as_array()?
                .iter()
                .map(|line| line.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()?;
            Some(PatchHunk {
                old_start: number("oldStart")?,
                old_lines: number("oldLines")?,
                new_start: number("newStart")?,
                new_lines: number("newLines")?,
                lines,
            })
        })
        .collect()
}

/// A unified diff's lines: each hunk's header, then its lines with their
/// numbers in the old and the new file.
pub fn patch_diff(hunks: &[PatchHunk]) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for hunk in hunks {
        if lines.len() >= MAX_DIFF_LINES {
            break;
        }
        lines.push(DiffLine {
            kind: DiffKind::Hunk,
            text: format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
            ),
            old_line: None,
            new_line: None,
        });
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        for line in &hunk.lines {
            if lines.len() >= MAX_DIFF_LINES {
                break;
            }
            let (prefix, rest) = match line.chars().next() {
                Some(first @ ('+' | '-' | ' ')) => (first, &line[1..]),
                // "\ No newline at end of file" and the like.
                Some('\\') => continue,
                _ => (' ', line.as_str()),
            };
            let text = clamp(rest);
            match prefix {
                '+' => {
                    lines.push(DiffLine {
                        kind: DiffKind::Add,
                        text,
                        old_line: None,
                        new_line: Some(new),
                    });
                    new += 1;
                }
                '-' => {
                    lines.push(DiffLine {
                        kind: DiffKind::Remove,
                        text,
                        old_line: Some(old),
                        new_line: None,
                    });
                    old += 1;
                }
                _ => {
                    lines.push(DiffLine {
                        kind: DiffKind::Context,
                        text,
                        old_line: Some(old),
                        new_line: Some(new),
                    });
                    old += 1;
                    new += 1;
                }
            }
        }
    }
    lines
}

/// The most cells the longest-common-subsequence table may have (old lines
/// × new lines, after the shared start and end are set aside): about 2 MB.
pub const MAX_TABLE_CELLS: usize = 250_000;

/// The changed lines between two texts (LineDiff.changes): removals carry
/// their line in the old text, additions theirs in the new one. The lines
/// both texts start and end with are set aside first; when what remains is
/// still too big for the table, its old lines are listed as removed and the
/// new ones as added. The flag says lines past `limit` were left out.
pub fn line_diff(old: &str, new: &str, limit: usize) -> (Vec<DiffLine>, bool) {
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start
        && end < b.len() - start
        && a[a.len() - 1 - end] == b[b.len() - 1 - end]
    {
        end += 1;
    }
    let old_middle = &a[start..a.len() - end];
    let new_middle = &b[start..b.len() - end];
    let common = if old_middle.len() * new_middle.len() <= MAX_TABLE_CELLS {
        lcs(old_middle, new_middle)
    } else {
        Vec::new()
    };

    let mut lines = Vec::new();
    let (mut i, mut j, mut k) = (0, 0, 0);
    let mut truncated = false;
    while i < old_middle.len() || j < new_middle.len() {
        let shared = common.get(k).copied();
        if i < old_middle.len() && shared != Some(old_middle[i]) {
            lines.push(DiffLine {
                kind: DiffKind::Remove,
                text: clamp(old_middle[i]),
                old_line: Some((start + i + 1) as u32),
                new_line: None,
            });
            i += 1;
        } else if j < new_middle.len() && shared != Some(new_middle[j]) {
            lines.push(DiffLine {
                kind: DiffKind::Add,
                text: clamp(new_middle[j]),
                old_line: None,
                new_line: Some((start + j + 1) as u32),
            });
            j += 1;
        } else {
            i += 1;
            j += 1;
            k += 1;
        }
        if lines.len() > limit {
            truncated = true;
            lines.pop();
            break;
        }
    }
    (lines, truncated)
}

fn lcs<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<&'a str> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let width = b.len() + 1;
    let mut table = vec![0u32; (a.len() + 1) * width];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            table[i * width + j] = if a[i - 1] == b[j - 1] {
                table[(i - 1) * width + j - 1] + 1
            } else {
                table[(i - 1) * width + j].max(table[i * width + j - 1])
            };
        }
    }
    let mut result = Vec::new();
    let (mut i, mut j) = (a.len(), b.len());
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push(a[i - 1]);
            i -= 1;
            j -= 1;
        } else if table[(i - 1) * width + j] > table[i * width + j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

/// An Edit's diff from what it replaced with what, as the chat shows it
/// before (or without) a structured result. `None` when both are empty.
pub fn edit_preview(input: &BTreeMap<String, String>) -> Option<ToolResultView> {
    let old = input.get("old_string").map(String::as_str).unwrap_or("");
    let new = input.get("new_string").map(String::as_str).unwrap_or("");
    if old.is_empty() && new.is_empty() {
        return None;
    }
    Some(ToolResultView::Edit {
        file_path: input.get("file_path").cloned().unwrap_or_default(),
        replace_all: false,
        user_modified: false,
        diff: line_diff(old, new, MAX_DIFF_LINES).0,
    })
}

/// A tool's structured result. `input` is the call's flat input (an Edit
/// result that doesn't repeat its strings falls back to it).
pub fn parse(tool_name: &str, data: &Object, input: &BTreeMap<String, String>) -> ToolResultView {
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        let mut parts = rest.split("__");
        let server = parts.next().filter(|s| !s.is_empty()).unwrap_or("unknown");
        let tool = parts.collect::<Vec<_>>().join("__");
        return ToolResultView::Mcp {
            server_name: server.to_owned(),
            tool_name: if tool.is_empty() {
                tool_name.to_owned()
            } else {
                tool
            },
            raw: trimmed(&Value::Object(data.clone())),
        };
    }
    match tool_name {
        "Read" => {
            let file = data.get("file").and_then(Value::as_object).unwrap_or(data);
            ToolResultView::Read {
                file_path: text_or_empty(file, "filePath"),
                content: text_or_empty(file, "content"),
                num_lines: count(file, "numLines"),
                start_line: int(file, "startLine")
                    .map_or(1, |n| n.clamp(0, u32::MAX as i64) as u32),
                total_lines: count(file, "totalLines"),
            }
        }
        "Edit" => {
            let patch = hunks(data);
            let diff = if patch.is_empty() {
                let own = |key: &str, fallback: &str| {
                    data.get(key)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| input.get(fallback).cloned())
                        .unwrap_or_default()
                };
                let (old, new) = (
                    own("oldString", "old_string"),
                    own("newString", "new_string"),
                );
                line_diff(&old, &new, MAX_DIFF_LINES).0
            } else {
                patch_diff(&patch)
            };
            ToolResultView::Edit {
                file_path: text_or_empty(data, "filePath"),
                replace_all: flag(data, "replaceAll"),
                user_modified: flag(data, "userModified"),
                diff,
            }
        }
        "Write" => ToolResultView::Write {
            file_path: text_or_empty(data, "filePath"),
            created: data.get("type").and_then(Value::as_str) != Some("overwrite"),
            content: text_or_empty(data, "content"),
            diff: patch_diff(&hunks(data)),
        },
        "Bash" => ToolResultView::Bash {
            stdout: text_or_empty(data, "stdout"),
            stderr: text_or_empty(data, "stderr"),
            interrupted: flag(data, "interrupted"),
            return_code_interpretation: text(data, "returnCodeInterpretation"),
            background_task_id: text(data, "backgroundTaskId"),
        },
        "Grep" => ToolResultView::Grep {
            mode: match data.get("mode").and_then(Value::as_str) {
                Some("content") => "content",
                Some("count") => "count",
                _ => "files_with_matches",
            }
            .to_owned(),
            filenames: strings(data, "filenames"),
            num_files: count(data, "numFiles"),
            content: text(data, "content"),
            num_lines: int(data, "numLines").map(|n| n.clamp(0, u32::MAX as i64) as u32),
        },
        "Glob" => ToolResultView::Glob {
            filenames: strings(data, "filenames"),
            num_files: count(data, "numFiles"),
            truncated: flag(data, "truncated"),
        },
        "TodoWrite" => ToolResultView::Todo {
            items: data
                .get("newTodos")
                .and_then(Value::as_array)
                .map(|todos| {
                    todos
                        .iter()
                        .filter_map(|todo| {
                            let todo = todo.as_object()?;
                            Some(TodoView {
                                content: clamp(todo.get("content")?.as_str()?),
                                status: todo.get("status")?.as_str()?.to_owned(),
                                active_form: text(todo, "activeForm"),
                            })
                        })
                        .take(MAX_LIST_ITEMS)
                        .collect()
                })
                .unwrap_or_default(),
        },
        "Task" | "Agent" => ToolResultView::Task {
            agent_id: text_or_empty(data, "agentId"),
            status: text(data, "status").unwrap_or_else(|| "unknown".to_owned()),
            content: text_or_empty(data, "content"),
            total_duration_ms: int(data, "totalDurationMs").and_then(|n| u64::try_from(n).ok()),
            total_tokens: int(data, "totalTokens").and_then(|n| u64::try_from(n).ok()),
            total_tool_use_count: int(data, "totalToolUseCount")
                .and_then(|n| u32::try_from(n).ok()),
        },
        "WebFetch" => ToolResultView::WebFetch {
            url: text_or_empty(data, "url"),
            code: count(data, "code"),
            code_text: text_or_empty(data, "codeText"),
            bytes: int(data, "bytes").map_or(0, |n| n.max(0) as u64),
            duration_ms: int(data, "durationMs").map_or(0, |n| n.max(0) as u64),
            result: text_or_empty(data, "result"),
        },
        "WebSearch" => ToolResultView::WebSearch {
            query: text_or_empty(data, "query"),
            results: search_results(data),
        },
        "AskUserQuestion" => ToolResultView::AskUserQuestion {
            questions: answered_questions(data),
            answers: data
                .get("answers")
                .and_then(Value::as_object)
                // All strings or none: `[String: String]` on the Mac.
                .and_then(|answers| {
                    answers
                        .iter()
                        .map(|(question, answer)| Some((question.clone(), clamp(answer.as_str()?))))
                        .collect::<Option<BTreeMap<_, _>>>()
                })
                .unwrap_or_default(),
        },
        "BashOutput" => ToolResultView::BashOutput {
            shell_id: text_or_empty(data, "shellId"),
            status: text_or_empty(data, "status"),
            stdout: text_or_empty(data, "stdout"),
            stderr: text_or_empty(data, "stderr"),
            exit_code: int(data, "exitCode").and_then(|n| i32::try_from(n).ok()),
        },
        "KillShell" => ToolResultView::KillShell {
            shell_id: text(data, "shell_id")
                .or_else(|| text(data, "shellId"))
                .unwrap_or_default(),
            message: text_or_empty(data, "message"),
        },
        "ExitPlanMode" => ToolResultView::ExitPlanMode {
            plan: text(data, "plan"),
            file_path: text(data, "filePath"),
        },
        _ => ToolResultView::Generic {
            text: text(data, "content")
                .or_else(|| text(data, "stdout"))
                .or_else(|| text(data, "result")),
            raw: Some(trimmed(&Value::Object(data.clone()))),
        },
    }
}

fn search_results(data: &Object) -> Vec<SearchResultView> {
    let Some(Value::Array(results)) = data.get("results") else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|item| {
            let item = item.as_object()?;
            Some(SearchResultView {
                title: clamp(item.get("title")?.as_str()?),
                url: clamp(item.get("url")?.as_str()?),
                snippet: text_or_empty(item, "snippet"),
            })
        })
        .take(MAX_LIST_ITEMS)
        .collect()
}

/// The questions an AskUserQuestion result repeats.
fn answered_questions(data: &Object) -> Vec<Question> {
    let Some(Value::Array(questions)) = data.get("questions") else {
        return Vec::new();
    };
    questions
        .iter()
        .filter_map(|question| {
            let question = question.as_object()?;
            Some(Question {
                text: clamp(question.get("question")?.as_str()?),
                header: text(question, "header"),
                multi_select: flag(question, "multiSelect"),
                options: question
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(|option| {
                                let option = option.as_object()?;
                                Some(QuestionOption {
                                    label: clamp(option.get("label")?.as_str()?),
                                    description: text(option, "description"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// What a finished tool's line says (ToolStatusDisplay.completed): "Read
/// main.rs (40 lines)", "Found 3 files", "Completed". `raw` is the result's
/// `toolUseResult`, which says how long a web search took.
pub fn completed_text(view: Option<&ToolResultView>, raw: Option<&Object>) -> String {
    let Some(view) = view else {
        return "Completed".to_owned();
    };
    let files = |count: u32| if count == 1 { "file" } else { "files" };
    match view {
        ToolResultView::Read {
            file_path,
            num_lines,
            total_lines,
            ..
        } => {
            let lines = if total_lines > num_lines {
                format!("{num_lines}+ lines")
            } else {
                format!("{num_lines} lines")
            };
            format!("Read {} ({lines})", tool_input::file_name(file_path))
        }
        ToolResultView::Edit { file_path, .. } => {
            format!("Edited {}", tool_input::file_name(file_path))
        }
        ToolResultView::Write {
            file_path, created, ..
        } => format!(
            "{} {}",
            if *created { "Created" } else { "Wrote" },
            tool_input::file_name(file_path)
        ),
        ToolResultView::Bash {
            return_code_interpretation,
            background_task_id,
            ..
        } => match (background_task_id, return_code_interpretation) {
            (Some(id), _) => format!("Running in background ({id})"),
            (None, Some(interpretation)) => interpretation.clone(),
            _ => "Completed".to_owned(),
        },
        ToolResultView::Grep { num_files, .. } => {
            format!("Found {num_files} {}", files(*num_files))
        }
        ToolResultView::Glob { num_files, .. } => {
            if *num_files == 0 {
                "No files found".to_owned()
            } else {
                format!("Found {num_files} {}", files(*num_files))
            }
        }
        ToolResultView::Todo { .. } => "Updated todos".to_owned(),
        ToolResultView::Task { status, .. } => capitalized(status),
        ToolResultView::WebFetch {
            code, code_text, ..
        } => format!("{code} {code_text}"),
        ToolResultView::WebSearch { results, .. } => {
            let seconds = raw
                .and_then(|raw| raw.get("durationSeconds"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let time = if seconds >= 1.0 {
                format!("{}s", seconds as i64)
            } else {
                format!("{}ms", (seconds * 1000.0) as i64)
            };
            let word = if results.len() == 1 {
                "search"
            } else {
                "searches"
            };
            format!("Did 1 {word} in {time}")
        }
        ToolResultView::AskUserQuestion { .. } => "Answered".to_owned(),
        ToolResultView::BashOutput { status, .. } => format!("Status: {status}"),
        ToolResultView::KillShell { .. } => "Terminated".to_owned(),
        ToolResultView::ExitPlanMode { .. } => "Plan ready".to_owned(),
        ToolResultView::Mcp { .. } | ToolResultView::Generic { .. } => "Completed".to_owned(),
    }
}

/// Foundation's `capitalized`: the first letter of every word upper-cased,
/// the rest lower-cased ("async_launched" → "Async_Launched").
pub fn capitalized(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if at_word_start {
                result.extend(ch.to_uppercase());
            } else {
                result.extend(ch.to_lowercase());
            }
            at_word_start = false;
        } else {
            result.push(ch);
            at_word_start = !ch.is_numeric();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Object {
        value.as_object().cloned().unwrap()
    }

    fn no_input() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    // B_ToolSummaryTests.diffShowsRemovalsThenAdditionsAndStopsAtTheLimit
    #[test]
    fn diff_shows_removals_then_additions_and_stops_at_the_limit() {
        let (lines, truncated) = line_diff("a\nb\nc", "a\nB\nc", 12);
        assert_eq!(
            lines,
            vec![
                DiffLine {
                    kind: DiffKind::Remove,
                    text: "b".into(),
                    old_line: Some(2),
                    new_line: None
                },
                DiffLine {
                    kind: DiffKind::Add,
                    text: "B".into(),
                    old_line: None,
                    new_line: Some(2)
                },
            ]
        );
        assert!(!truncated);
        let long: String = (1..=20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let (lines, truncated) = line_diff(&long, "", 5);
        assert_eq!(lines.len(), 5);
        assert!(truncated);
        assert!(line_diff("", "", 5).0.is_empty());
    }

    #[test]
    fn a_patch_keeps_its_line_numbers() {
        let data = object(json!({
            "filePath": "C:\\code\\app\\redirect.ts",
            "structuredPatch": [{
                "oldStart": 12, "oldLines": 3, "newStart": 12, "newLines": 4,
                "lines": [" const next = x;", "-return redirect(next);", "+if (!ok) return;", "+return redirect(next);", "\\ No newline at end of file"]
            }]
        }));
        let ToolResultView::Edit {
            diff, file_path, ..
        } = parse("Edit", &data, &no_input())
        else {
            panic!("not an edit");
        };
        assert_eq!(file_path, "C:\\code\\app\\redirect.ts");
        let shape: Vec<(DiffKind, Option<u32>, Option<u32>)> = diff
            .iter()
            .map(|line| (line.kind, line.old_line, line.new_line))
            .collect();
        assert_eq!(
            shape,
            vec![
                (DiffKind::Hunk, None, None),
                (DiffKind::Context, Some(12), Some(12)),
                (DiffKind::Remove, Some(13), None),
                (DiffKind::Add, None, Some(13)),
                (DiffKind::Add, None, Some(14)),
            ]
        );
        assert_eq!(diff[0].text, "@@ -12,3 +12,4 @@");
        assert_eq!(diff[2].text, "return redirect(next);");
    }

    #[test]
    fn an_edit_without_a_patch_falls_back_to_its_strings() {
        let mut input = BTreeMap::new();
        input.insert("old_string".to_owned(), "sleep(100)".to_owned());
        input.insert("new_string".to_owned(), "await ready()".to_owned());
        let ToolResultView::Edit { diff, .. } =
            parse("Edit", &object(json!({"filePath": "/a/t.ts"})), &input)
        else {
            panic!("not an edit");
        };
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].kind, DiffKind::Remove);
        assert_eq!(diff[1].text, "await ready()");
        assert!(edit_preview(&BTreeMap::new()).is_none());
        assert!(edit_preview(&input).is_some());
    }

    #[test]
    fn each_tool_gets_its_own_shape() {
        let read = parse(
            "Read",
            &object(
                json!({"file": {"filePath": "/x/redirect.ts", "content": "a", "numLines": 5, "startLine": 3, "totalLines": 9}}),
            ),
            &no_input(),
        );
        assert_eq!(
            read,
            ToolResultView::Read {
                file_path: "/x/redirect.ts".into(),
                content: "a".into(),
                num_lines: 5,
                start_line: 3,
                total_lines: 9
            }
        );
        assert_eq!(
            completed_text(Some(&read), None),
            "Read redirect.ts (5+ lines)"
        );

        let bash = parse(
            "Bash",
            &object(
                json!({"stdout": "ok", "stderr": "", "interrupted": false, "backgroundTaskId": "b1"}),
            ),
            &no_input(),
        );
        assert_eq!(
            completed_text(Some(&bash), None),
            "Running in background (b1)"
        );

        let task = parse(
            "Agent",
            &object(
                json!({"agentId": "a7", "status": "async_launched", "content": "x", "totalTokens": 12}),
            ),
            &no_input(),
        );
        assert!(
            matches!(&task, ToolResultView::Task { agent_id, total_tokens: Some(12), .. } if agent_id == "a7")
        );
        assert_eq!(completed_text(Some(&task), None), "Async_Launched");

        let raw = object(
            json!({"query": "rust", "durationSeconds": 0.25, "results": [{"title": "t", "url": "u"}]}),
        );
        let search = parse("WebSearch", &raw, &no_input());
        assert_eq!(
            completed_text(Some(&search), Some(&raw)),
            "Did 1 search in 250ms"
        );

        let mcp = parse(
            "mcp__github__create_issue",
            &object(json!({"ok": true})),
            &no_input(),
        );
        assert!(
            matches!(&mcp, ToolResultView::Mcp { server_name, tool_name, .. }
            if server_name == "github" && tool_name == "create_issue")
        );

        let other = parse("Mystery", &object(json!({"result": "fine"})), &no_input());
        assert!(
            matches!(&other, ToolResultView::Generic { text: Some(text), .. } if text == "fine")
        );
        assert_eq!(completed_text(None, None), "Completed");
        assert_eq!(
            completed_text(
                Some(&parse("Glob", &object(json!({"numFiles": 0})), &no_input())),
                None
            ),
            "No files found"
        );
        assert_eq!(
            completed_text(
                Some(&parse("Grep", &object(json!({"numFiles": 1})), &no_input())),
                None
            ),
            "Found 1 file"
        );
    }

    #[test]
    fn long_output_is_cut_for_the_panel() {
        let data = object(json!({"stdout": "x".repeat(MAX_RESULT_TEXT + 500)}));
        let ToolResultView::Bash { stdout, .. } = parse("Bash", &data, &no_input()) else {
            panic!("not bash");
        };
        assert_eq!(stdout.chars().count(), MAX_RESULT_TEXT);
    }

    #[test]
    fn capitalised_like_foundation() {
        assert_eq!(capitalized("completed"), "Completed");
        assert_eq!(capitalized("async_launched"), "Async_Launched");
        assert_eq!(capitalized("IN PROGRESS"), "In Progress");
    }
}
