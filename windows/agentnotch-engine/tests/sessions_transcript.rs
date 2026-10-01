//! The transcript sync job and the locator, ported from the Mac's
//! TranscriptTests, A1_TranscriptParserTests (job level) and
//! PP_SessionTests.theSharedHistoryIsSearchedOnceAndSpeltThroughTheSessionsFolder.
//!
//! Adapted: the Mac keeps running token totals; Windows does not (the cloud
//! has its own scanner), so `usageIsCountedOncePerResponse` and
//! `repeatedResponseUsesItsLatestUsage` assert what the summary does keep:
//! the latest response's usage drives the context estimate and a repeated
//! response counts once. The context-window vectors of TranscriptTests
//! (`contextEstimateWindow`) live in `sessions::summary::tests`.

mod sessions_support;

use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::{project_slug, PathStyle, Paths};
use agentnotch_engine::core::time::parse_iso8601;
use agentnotch_engine::model::{ChatRole, MessageBlock, SessionId};
use agentnotch_engine::runtime_types::{
    TokenUsage, TranscriptCursor, TranscriptDelta, TranscriptEntry,
};
use agentnotch_engine::sessions::locator::TranscriptLocator;
use agentnotch_engine::sessions::summary::TranscriptSummary;
use agentnotch_engine::sessions::tasks::TaskList;
use agentnotch_engine::sessions::tool_input::flatten_value;
use agentnotch_engine::sessions::transcript::{
    decode_line, is_human_prompt, sync_transcript, sync_transcript_chunked, CHUNK_SIZE,
};
use serde_json::{json, Value};
use sessions_support::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---- TranscriptTests: summary through decoded entries ----

fn opus(id: Option<&str>, input: u64, output: u64, read: u64, creation: u64) -> Value {
    assistant_usage(
        id,
        "claude-opus-4-5",
        false,
        input,
        output,
        read,
        creation,
        "hi",
    )
}

#[test]
fn the_latest_response_drives_the_context_and_a_repeat_counts_once() {
    // One API response streamed as three content-block lines, then another.
    let lines = vec![
        opus(Some("msg_1"), 10, 5, 1000, 200),
        opus(Some("msg_1"), 10, 5, 1000, 200),
        opus(Some("msg_1"), 10, 5, 1000, 200),
        opus(Some("msg_2"), 3, 7, 1200, 0),
    ];
    let (info, _) = summarize(&lines);
    assert_eq!(info.last_context_tokens, Some(3 + 1200));
    assert_eq!(info.last_model.as_deref(), Some("claude-opus-4-5"));
    // The decoded usage carries every field of the response.
    let usages: Vec<TokenUsage> = entries_of(&lines[..1])
        .into_iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Assistant { usage, .. } => usage,
            _ => None,
        })
        .collect();
    assert_eq!(
        usages,
        [TokenUsage {
            input: 10,
            output: 5,
            cache_creation: 200,
            cache_read: 1000
        }]
    );
}

#[test]
fn a_repeated_response_uses_its_latest_usage() {
    let (info, _) = summarize(&[
        opus(Some("msg_1"), 10, 1, 0, 0),
        opus(Some("msg_1"), 12, 40, 0, 0),
    ]);
    assert_eq!(info.last_context_tokens, Some(12));
}

#[test]
fn synthetic_and_sidechain_messages_are_skipped() {
    let lines = vec![
        opus(Some("msg_1"), 10, 5, 0, 0),
        assistant_usage(
            Some("msg_err"),
            "<synthetic>",
            false,
            0,
            0,
            0,
            0,
            "API Error",
        ),
        assistant_usage(
            Some("msg_side"),
            "claude-haiku",
            true,
            999,
            999,
            0,
            0,
            "side",
        ),
    ];
    let (info, _) = summarize(&lines);
    assert_eq!(info.last_context_tokens, Some(10));
    assert_eq!(info.last_model.as_deref(), Some("claude-opus-4-5"));
    // A sidechain line makes no entry on the main transcript at all.
    assert!(entries_of(&lines[2..]).is_empty());
}

#[test]
fn titles_prefer_custom_over_ai() {
    let (ai, _) = summarize(&[json!({"type": "ai-title", "aiTitle": "Fix login bug"})]);
    assert_eq!(ai.title.as_deref(), Some("Fix login bug"));
    let (both, _) = summarize(&[
        json!({"type": "custom-title", "customTitle": "My rename"}),
        json!({"type": "ai-title", "aiTitle": "Later AI title"}),
        json!({"type": "summary", "summary": "What happened"}),
    ]);
    assert_eq!(both.title.as_deref(), Some("My rename"));
    assert_eq!(both.summary.as_deref(), Some("What happened"));
}

#[test]
fn tracks_messages_and_skips_commands() {
    let (info, _) = summarize(&[
        json!({"type": "user", "message": {"content": "<command-name>/model</command-name>"}}),
        json!({"type": "user", "message": {"content": "Please fix the login flow"}}),
        json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "npm test"}}]}}),
    ]);
    assert_eq!(
        info.first_user_message.as_deref(),
        Some("Please fix the login flow")
    );
    assert_eq!(info.last_message.as_deref(), Some("npm test"));
    assert_eq!(info.last_message_role.as_deref(), Some("tool"));
    assert_eq!(info.last_tool_name.as_deref(), Some("Bash"));
}

#[test]
fn the_last_block_of_a_reply_decides_what_the_summary_shows() {
    // Text after a call: the text is the last message and ends the turn.
    let (after, _) = summarize(&[
        json!({"type": "assistant", "timestamp": "2027-01-15T10:00:00Z",
        "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
            {"type": "text", "text": "done"}]}}),
    ]);
    assert_eq!(after.last_message.as_deref(), Some("done"));
    assert_eq!(after.last_message_role.as_deref(), Some("assistant"));
    assert!(after.last_turn.ends_with_reply);
    // A call after text: the call is, and the turn goes on.
    let (before, _) = summarize(&[
        json!({"type": "assistant", "timestamp": "2027-01-15T10:00:00Z",
        "message": {"content": [
            {"type": "text", "text": "let me check"},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]}}),
    ]);
    assert_eq!(before.last_message.as_deref(), Some("ls"));
    assert_eq!(before.last_message_role.as_deref(), Some("tool"));
    assert!(!before.last_turn.ends_with_reply);
}

// ---- slug ----

#[test]
fn the_slug_replaces_every_non_alphanumeric() {
    assert_eq!(
        project_slug("/Users/paras/Documents/GitHub/@paraswtf/superpowered-vibe-notch"),
        "-Users-paras-Documents-GitHub--paraswtf-superpowered-vibe-notch"
    );
    assert_eq!(
        project_slug("/Users/me/my_app.v2 (copy)"),
        "-Users-me-my-app-v2--copy-"
    );
    // JavaScript replaces per UTF-16 unit: é is one unit, an emoji two.
    assert_eq!(project_slug("/tmp/café"), "-tmp-caf-");
    assert_eq!(project_slug("/tmp/a😀"), "-tmp-a--");
    assert_eq!(project_slug(r"C:\Users\me\proj"), "C--Users-me-proj");
}

#[test]
fn the_expected_path_is_spelt_in_each_style() {
    let files = StdSecureFiles;
    let posix = Paths::new(PathStyle::Posix, "/Users/me");
    let locator = TranscriptLocator::new(&posix, &files);
    let expected =
        locator.expected_transcript_path("s1", "/Users/me/@org/app", "/Users/me/.claude-work");
    assert_eq!(
        expected,
        "/Users/me/.claude-work/projects/-Users-me--org-app/s1.jsonl"
    );

    let windows = Paths::new(PathStyle::Windows, r"C:\Users\me");
    let locator = TranscriptLocator::new(&windows, &files);
    let expected = locator.expected_transcript_path(
        "s1",
        r"C:\Users\me\@org\app",
        r"C:\Users\me\.claude-work",
    );
    assert_eq!(
        expected,
        r"C:\Users\me\.claude-work\projects\C--Users-me--org-app\s1.jsonl"
    );
    // The slash spelling and the case of the drive name one folder (the
    // slug keeps the spelling Claude Code was given, the file system folds it).
    let again =
        locator.expected_transcript_path("s1", "c:/Users/me/@org/app", "c:/Users/me/.claude-work");
    assert!(windows.same(&again, &expected));
}

// ---- locator ----

fn home() -> (tempfile::TempDir, Paths, String) {
    let dir = tempfile::tempdir().unwrap();
    let canonical = plain_canonical(dir.path());
    let paths = Paths::native(&canonical);
    let home = paths.home().to_owned();
    (dir, paths, home)
}

#[test]
fn a_transcript_is_located_by_slug_then_by_search() {
    let (_guard, paths, home) = home();
    let files = StdSecureFiles;
    let locator = TranscriptLocator::new(&paths, &files);
    let config = paths.join(&home, ".claude-work");
    let cwd = paths.join(&home, "@org/app");

    // The expected slug location.
    let expected = locator.expected_transcript_path("s1", &cwd, &config);
    assert!(
        expected
            .ends_with(&format!("{}-{}", paths.style().separator(), "s1.jsonl").replace('-', ""))
            || expected.ends_with("s1.jsonl")
    );
    write_lines(Path::new(&expected), &[json!({})]);
    assert_eq!(
        locator.transcript_path("s1", Some(&cwd), &config, None),
        Some(expected.clone())
    );

    // A session whose folder name can't be reproduced (a long slug's hash) is found by search.
    let odd = paths.join(
        &paths.join(
            &paths.join(&config, "projects"),
            "-some-truncated-slug-abc123",
        ),
        "s2.jsonl",
    );
    write_lines(Path::new(&odd), &[json!({})]);
    assert_eq!(
        locator.transcript_path("s2", Some("/elsewhere"), &config, None),
        Some(odd.clone())
    );

    // The hook's path wins when it exists; a missing session is nothing.
    assert_eq!(
        locator.transcript_path("s2", Some(&cwd), &config, Some(&expected)),
        Some(expected.clone())
    );
    assert_eq!(
        locator.transcript_path("s2", Some(&cwd), &config, Some("nowhere.jsonl")),
        Some(odd)
    );
    assert_eq!(
        locator.transcript_path("nope", Some(&cwd), &config, None),
        None
    );
    assert_eq!(locator.search_transcript("../s1", &config), None);
    assert_eq!(locator.search_transcript(r"..\s1", &config), None);
    assert_eq!(locator.search_transcript("", &config), None);
}

#[test]
fn a_subagent_transcript_is_found_in_its_three_layouts() {
    let (_guard, paths, home) = home();
    let files = StdSecureFiles;
    let locator = TranscriptLocator::new(&paths, &files);
    let project = paths.join(&paths.join(&home, "projects"), "-p");
    let main = paths.join(&project, "s1.jsonl");
    let workflow = paths.join(&project, "s1/subagents/workflows/wf_1/agent-a7.jsonl");
    write_lines(Path::new(&workflow), &[]);

    // Workflow agents sit one level deeper.
    assert_eq!(locator.subagent_transcript_path(&main, "a7"), workflow);
    // Nothing yet: the nested path, where the file will appear.
    let nested_zz = paths.join(&project, "s1/subagents/agent-zz.jsonl");
    assert_eq!(locator.subagent_transcript_path(&main, "zz"), nested_zz);
    // Nested wins over flat; flat (older Claude Code) is found; a hostile id is never searched.
    let flat = paths.join(&project, "agent-old.jsonl");
    write_lines(Path::new(&flat), &[]);
    assert_eq!(locator.subagent_transcript_path(&main, "old"), flat);
    let nested = paths.join(&project, "s1/subagents/agent-old.jsonl");
    write_lines(Path::new(&nested), &[]);
    assert_eq!(locator.subagent_transcript_path(&main, "old"), nested);
    assert_eq!(
        locator.subagent_transcript_path(&main, "../x"),
        paths.join(&project, "s1/subagents/agent-../x.jsonl")
    );
}

/// `.claude-shared` holds the history; the window folder and `.claude` link
/// their `projects` to it. Returns (home, window, default folder, cwd, slug).
fn shared_history(paths: &Paths, home: &str) -> (String, String, String, String) {
    let cwd = "/Users/me/@paraswtf/app".to_owned();
    let slug = project_slug(&cwd);
    let shared = paths.join(
        &paths.join(&paths.join(home, ".claude-shared"), "projects"),
        &slug,
    );
    write_lines(Path::new(&paths.join(&shared, "s-1.jsonl")), &[json!({})]);
    let window = paths.join(&paths.join(home, ".claude-windows"), "801f9dd51396");
    let default = paths.join(home, ".claude");
    (window, default, cwd, slug)
}

fn check_shared_history(
    locator: &TranscriptLocator,
    paths: &Paths,
    home: &str,
    window: &str,
    default: &str,
    cwd: &str,
    slug: &str,
) {
    let spelt = |dir: &str, sid: &str| {
        paths.join(
            &paths.join(&paths.join(dir, "projects"), slug),
            &format!("{sid}.jsonl"),
        )
    };
    let shared = paths.join(home, ".claude-shared");
    let found = locator.transcript_path_in(
        "s-1",
        Some(cwd),
        &[window.to_owned(), default.to_owned()],
        None,
    );
    // Spelt through the window, its folder reads back as the window, never the shared history.
    assert_eq!(found.as_deref(), Some(spelt(window, "s-1").as_str()));
    assert_eq!(
        found
            .as_deref()
            .and_then(|f| paths.config_dir_from_transcript(f))
            .as_deref(),
        Some(window)
    );
    let found = found.unwrap();
    assert!(locator.is_same_file(&found, &spelt(default, "s-1")));
    assert!(locator.is_same_file(&found, &spelt(&shared, "s-1")));
    assert!(!locator.is_same_file(&found, &spelt(&shared, "s-2")));
    assert_eq!(
        locator.transcript_path_in(
            "s-9",
            Some(cwd),
            &[window.to_owned(), default.to_owned()],
            None
        ),
        None
    );
    // The default folder first finds it spelt through itself.
    let first = locator.transcript_path_in(
        "s-1",
        Some(cwd),
        &[default.to_owned(), window.to_owned()],
        None,
    );
    assert_eq!(first.as_deref(), Some(spelt(default, "s-1").as_str()));
}

#[test]
fn the_shared_history_is_searched_once_and_spelt_through_the_sessions_folder() {
    // Portable variant: a fake that says the two `projects` folders are links.
    let (_guard, paths, home) = home();
    let (window, default, cwd, slug) = shared_history(&paths, &home);
    let shared_projects =
        PathBuf::from(paths.join(&paths.join(&home, ".claude-shared"), "projects"));
    let files = LinkedFiles {
        links: vec![
            (
                PathBuf::from(paths.join(&window, "projects")),
                shared_projects.clone(),
            ),
            (
                PathBuf::from(paths.join(&default, "projects")),
                shared_projects,
            ),
        ],
    };
    let locator = TranscriptLocator::new(&paths, &files);
    check_shared_history(&locator, &paths, &home, &window, &default, &cwd, &slug);
}

#[cfg(unix)]
#[test]
fn the_shared_history_is_searched_once_through_real_links() {
    let (_guard, paths, home) = home();
    let (window, default, cwd, slug) = shared_history(&paths, &home);
    let shared_projects = paths.join(&paths.join(&home, ".claude-shared"), "projects");
    for dir in [&window, &default] {
        std::fs::create_dir_all(dir).unwrap();
        std::os::unix::fs::symlink(&shared_projects, paths.join(dir, "projects")).unwrap();
    }
    let files = StdSecureFiles;
    let locator = TranscriptLocator::new(&paths, &files);
    check_shared_history(&locator, &paths, &home, &window, &default, &cwd, &slug);
    // The search by id goes through the link too.
    assert!(locator.search_transcript("s-1", &window).is_some());
}

// ---- the job ----

fn session() -> SessionId {
    SessionId::from("p1")
}

fn sync(path: &Path, cursor: TranscriptCursor, agent: bool) -> TranscriptDelta {
    sync_transcript(&session(), path, cursor, agent, &StdSecureFiles)
}

fn messages(delta: &TranscriptDelta) -> Vec<String> {
    delta
        .entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => {
                message.blocks.iter().find_map(|block| match block {
                    MessageBlock::Text(text) => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

fn transcript() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects/-p/p1.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    (dir, path)
}

#[test]
fn incremental_parse_skips_partial_lines() {
    let (_guard, path) = transcript();
    let first = line_bytes(&opus(Some("m1"), 1, 1, 0, 0));
    let second = line_bytes(&opus(Some("m2"), 2, 2, 0, 0));
    let mut content = first.clone();
    content.extend_from_slice(&second[..10]);
    std::fs::write(&path, &content).unwrap();

    let one = sync(&path, TranscriptCursor::default(), false);
    let mut summary = TranscriptSummary::new();
    one.entries.iter().for_each(|e| summary.apply(e));
    assert_eq!(summary.info().last_context_tokens, Some(1));
    // The offset stops after the first newline; the tail waits.
    assert_eq!(one.cursor.offset, first.len() as u64);
    assert!(one.cursor.size > one.cursor.offset);

    let mut content = first;
    content.extend_from_slice(&second);
    std::fs::write(&path, &content).unwrap();
    let two = sync(&path, one.cursor, false);
    two.entries.iter().for_each(|e| summary.apply(e));
    assert_eq!(summary.info().last_context_tokens, Some(2));
    assert_eq!(two.cursor.offset, content.len() as u64);
    assert!(!two.reset);
}

#[test]
fn partial_lines_wait_for_their_newline() {
    let (_guard, path) = transcript();
    let first = line_bytes(&assistant_text("one", t0()));
    let second = line_bytes(&assistant_text("two", t0()));
    let mut content = first.clone();
    content.extend_from_slice(&second[..20]);
    std::fs::write(&path, &content).unwrap();

    let one = sync(&path, TranscriptCursor::default(), false);
    assert_eq!(messages(&one), ["one"]);
    content.truncate(first.len());
    content.extend_from_slice(&second);
    std::fs::write(&path, &content).unwrap();
    let rest = sync(&path, one.cursor, false);
    assert_eq!(messages(&rest), ["two"]);
    let (info, _) = summarize_entries(&rest);
    assert_eq!(info.last_turn.reply_text.as_deref(), Some("two"));
}

fn summarize_entries(
    delta: &TranscriptDelta,
) -> (
    agentnotch_engine::sessions::summary::ConversationInfo,
    TranscriptSummary,
) {
    let mut summary = TranscriptSummary::new();
    delta.entries.iter().for_each(|e| summary.apply(e));
    (summary.info(), summary)
}

#[test]
fn a_rewritten_transcript_is_read_again_and_replaces_history() {
    let (_guard, path) = transcript();
    let old: Vec<Value> = (0..5)
        .map(|i| assistant_text(&format!("old {i}"), t0()))
        .collect();
    write_lines(&path, &old);
    let first = sync(&path, TranscriptCursor::default(), false);
    assert_eq!(messages(&first).len(), 5);
    assert!(!first.reset);

    // The new file is shorter (a Windows runner reports no file index, so the
    // shrink is what tells).
    write_lines(&path, &[assistant_text("new", t0())]);
    let again = sync(&path, first.cursor, false);
    assert!(again.reset);
    assert_eq!(messages(&again), ["new"]);
    assert_eq!(
        summarize_entries(&again).0.last_turn.reply_text.as_deref(),
        Some("new")
    );
    // Then it is caught up.
    let quiet = sync(&path, again.cursor, false);
    assert!(!quiet.reset && quiet.entries.is_empty());
}

#[test]
fn a_replaced_file_resets_by_identity_only_when_both_are_known() {
    let (_guard, path) = transcript();
    let lines: Vec<Value> = (0..3)
        .map(|i| assistant_text(&format!("old {i}"), t0()))
        .collect();
    write_lines(&path, &lines);
    let files = FixedIdentity::new(7, 42);
    let first = sync_transcript(
        &session(),
        &path,
        TranscriptCursor::default(),
        false,
        &files,
    );
    assert_eq!(
        first.cursor.file.map(|f| (f.volume, f.index)),
        Some((7, 42))
    );

    // Longer than before, but another file index: replaced.
    let longer: Vec<Value> = (0..6)
        .map(|i| assistant_text(&format!("new {i}"), t0()))
        .collect();
    write_lines(&path, &longer);
    files.set(7, 43);
    let replaced = sync_transcript(&session(), &path, first.cursor, false, &files);
    assert!(replaced.reset);
    assert_eq!(messages(&replaced).len(), 6);

    // The same index: an append, no reset.
    append_lines(&path, &[assistant_text("more", t0())]);
    let appended = sync_transcript(&session(), &path, replaced.cursor, false, &files);
    assert!(!appended.reset);
    assert_eq!(messages(&appended), ["more"]);

    // An unknown (zero) index says nothing: a longer replacement is an append.
    let zero = FixedIdentity::new(0, 0);
    let cursor = TranscriptCursor {
        file: Some(agentnotch_engine::platform::FileIdentity {
            volume: 7,
            index: 1,
            modified_ns: 0,
            size: 0,
        }),
        ..appended.cursor
    };
    let unknown = sync_transcript(&session(), &path, cursor, false, &zero);
    assert!(!unknown.reset);
}

#[test]
fn clear_after_the_first_read_is_reported() {
    let (_guard, path) = transcript();
    write_lines(&path, &[assistant_text("before", t0())]);
    let first = sync(&path, TranscriptCursor::default(), false);
    append_lines(
        &path,
        &[
            user("<command-name>/clear</command-name>", t0(), json!({})),
            assistant_text("after", t0()),
        ],
    );
    let result = sync(&path, first.cursor, false);
    assert!(result.entries.contains(&TranscriptEntry::Clear));
    // The command's own line is not a chat message, and what follows is.
    assert_eq!(messages(&result), ["after"]);
    let clear = result
        .entries
        .iter()
        .position(|e| *e == TranscriptEntry::Clear)
        .unwrap();
    assert!(result.entries[..clear]
        .iter()
        .all(|e| !matches!(e, TranscriptEntry::Message(_))));
    // A /clear in an array text block counts too.
    let block = json!({"type": "user", "uuid": "c", "message": {"content": [
        {"type": "text", "text": "<command-name>/clear</command-name>"}]}});
    assert!(entries_of(&[block]).contains(&TranscriptEntry::Clear));
}

#[test]
fn timestamps_parse_with_and_without_fractional_seconds() {
    assert!(parse_iso8601("2026-09-24T10:00:00.123Z").is_some());
    assert!(parse_iso8601("2026-09-24T10:00:00Z").is_some());
    assert!(parse_iso8601("yesterday").is_none());
    // And they reach the entries.
    let with = json!({"type": "assistant", "uuid": "a", "timestamp": "2026-09-24T10:00:00.250Z",
        "message": {"content": [{"type": "text", "text": "x"}]}});
    let at = entries_of(&[with])
        .into_iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Assistant { at, .. } => Some(at),
            _ => None,
        })
        .expect("an assistant line produces an Assistant entry");
    assert_eq!(at, parse_iso8601("2026-09-24T10:00:00.250Z"));
}

fn write_agent(path: &Path, id: &str, tools: usize, completed: bool) {
    let mut lines = Vec::new();
    for index in 0..tools {
        lines.push(tool_use(
            &format!("{id}-{index}"),
            "Grep",
            json!({"pattern": format!("p{index}")}),
            t0(),
        ));
        if completed || index < tools - 1 {
            lines.push(tool_result(&format!("{id}-{index}"), "ok", t0(), None));
        }
    }
    write_lines(path, &lines);
}

#[test]
fn subagent_transcripts_are_incremental() {
    let (_guard, path) = transcript();
    let files = StdSecureFiles;
    let paths = Paths::native(path.parent().unwrap());
    let locator = TranscriptLocator::new(&paths, &files);
    let agent_path =
        PathBuf::from(locator.subagent_transcript_path(&path.to_string_lossy(), "solo"));
    write_agent(&agent_path, "solo", 2, true);

    let first = sync(&agent_path, TranscriptCursor::default(), true);
    let uses: Vec<&str> = first
        .entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let results: Vec<&str> = first
        .entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::ToolResult {
                tool_use_id,
                status,
                ..
            } if status == "success" => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(uses, ["solo-0", "solo-1"]);
    assert_eq!(results, ["solo-0", "solo-1"]);
    // Only calls and results: no messages, outputs or summary entries.
    assert!(first.entries.iter().all(|e| matches!(
        e,
        TranscriptEntry::ToolUse { .. } | TranscriptEntry::ToolResult { .. }
    )));
    // Nothing appended: nothing to read.
    let second = sync(&agent_path, first.cursor, true);
    assert!(second.entries.is_empty() && !second.reset);
    assert_eq!(second.cursor.offset, second.cursor.size);
    // A call appended later arrives alone.
    append_lines(&agent_path, &[tool_use("solo-2", "Read", json!({}), t0())]);
    let third = sync(&agent_path, second.cursor, true);
    assert_eq!(third.entries.len(), 1);
}

#[test]
fn agent_mode_keeps_sidechain_lines_and_main_mode_drops_them() {
    let (_guard, path) = transcript();
    let mut call = tool_use("s-1", "Bash", json!({"command": "ls"}), t0());
    call["isSidechain"] = json!(true);
    let mut result = tool_result("s-1", "ok", t0(), None);
    result["isSidechain"] = json!(true);
    write_lines(&path, &[call, result]);
    let main = sync(&path, TranscriptCursor::default(), false);
    assert!(main.entries.is_empty());
    let agent = sync(&path, TranscriptCursor::default(), true);
    assert_eq!(agent.entries.len(), 2);
}

#[test]
fn only_words_a_person_typed_count_as_prompts() {
    let notification = user(
        "<task-notification>x</task-notification>",
        t0(),
        json!({"origin": {"kind": "task-notification"}}),
    );
    let old_notification = user("<task-notification>y</task-notification>", t0(), json!({}));
    let compact = user(
        "This session is being continued from a previous conversation ...",
        t0(),
        json!({"isCompactSummary": true}),
    );
    let continuation = user(
        "continue",
        t0(),
        json!({"origin": {"kind": "auto-continuation"}}),
    );
    let later = t0() + Duration::from_secs(5);
    let human = user("fix the tests", later, json!({"origin": {"kind": "human"}}));

    for entry in [&notification, &old_notification, &compact, &continuation] {
        assert!(!is_human_prompt(entry));
    }
    assert!(is_human_prompt(&human));
    assert!(is_human_prompt(&user("plain old prompt", t0(), json!({}))));
    // A tool result is not a prompt, nor is a meta line.
    assert!(!is_human_prompt(&tool_result(
        "t",
        "ok",
        t0(),
        Some(json!({"stdout": "x"}))
    )));
    assert!(!is_human_prompt(&user("hi", t0(), json!({"isMeta": true}))));

    let (result, _) = summarize(&[notification.clone(), compact, continuation]);
    assert_eq!(result.last_user_message_at, None);
    assert_eq!(result.first_user_message, None);
    assert_eq!(result.last_message, None);
    let (with_human, _) = summarize(&[notification, human]);
    assert_eq!(with_human.last_user_message_at, Some(later));
    assert_eq!(with_human.last_message.as_deref(), Some("fix the tests"));

    // A prompt's text reaches the entries cut to 200 characters.
    let long = "x".repeat(500);
    let texts: Vec<String> = entries_of(&[user(&long, t0(), json!({}))])
        .into_iter()
        .filter_map(|e| match e {
            TranscriptEntry::PromptText { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["x".repeat(200)]);
}

#[test]
fn a_wake_up_or_a_compact_summary_is_injected_and_an_interrupt_is_one() {
    let entries = entries_of(&[user(
        "<task-notification>x</task-notification>",
        t0(),
        json!({}),
    )]);
    assert!(entries
        .iter()
        .any(|e| matches!(e, TranscriptEntry::Injected { .. })));
    assert!(!entries
        .iter()
        .any(|e| matches!(e, TranscriptEntry::HumanPrompt { .. })));
    let entries = entries_of(&[user("[Request interrupted by user]", t0(), json!({}))]);
    assert!(entries
        .iter()
        .any(|e| matches!(e, TranscriptEntry::Interrupt { at: Some(_) })));
    // The interrupt also reaches the chat as its own block.
    let message = entries
        .iter()
        .find_map(|e| match e {
            TranscriptEntry::Message(m) => Some(m),
            _ => None,
        })
        .unwrap();
    assert_eq!(message.blocks, [MessageBlock::Interrupted]);
    assert_eq!(message.role, ChatRole::User);
}

#[test]
fn tool_input_flattens_the_same_on_both_paths() {
    let raw = json!({"run_in_background": true, "timeout": 30, "ratio": 0.5, "command": "ls", "list": [1]});
    let expected: BTreeMap<String, String> = [
        ("run_in_background", "true"),
        ("timeout", "30"),
        ("ratio", "0.5"),
        ("command", "ls"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    // The entry's input, the chat block's and the subagent's all flatten alike.
    let line = json!({"type": "assistant", "uuid": "a", "message": {"content": [
        {"type": "tool_use", "id": "t1", "name": "Bash", "input": raw}]}});
    let entries = entries_of(std::slice::from_ref(&line));
    let input = entries
        .iter()
        .find_map(|e| match e {
            TranscriptEntry::ToolUse { input, .. } => Some(input.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(flatten_value(&input), expected);
    let block = entries
        .iter()
        .find_map(|e| match e {
            TranscriptEntry::Message(m) => m.blocks.iter().find_map(|b| match b {
                MessageBlock::ToolUse { input, .. } => Some(input.clone()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap();
    assert_eq!(flatten_value(&block), expected);
    let (_guard, path) = transcript();
    write_lines(&path, &[line]);
    let agent = sync(&path, TranscriptCursor::default(), true);
    let TranscriptEntry::ToolUse { input, .. } = &agent.entries[0] else {
        panic!()
    };
    assert_eq!(flatten_value(input), expected);
}

// ---- tool results ----

#[test]
fn tool_results_carry_their_status_task_and_output() {
    let lines = vec![
        tool_result(
            "t1",
            "fine",
            t0(),
            Some(json!({"stdout": "out", "stderr": "err", "task": {"id": "7"}})),
        ),
        json!({"type": "user", "uuid": "u2", "toolName": "Bash", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": "boom"}]}}),
        json!({"type": "user", "uuid": "u3", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t3", "is_error": true,
             "content": [{"type": "text", "text": "The user doesn't want to proceed with this tool use."}]}]}}),
        json!({"type": "user", "uuid": "u4", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t4", "is_error": true, "content": "Interrupted by user"}]}}),
        tool_result(
            "t5",
            "Task #9 created successfully: Write tests",
            t0(),
            None,
        ),
    ];
    let entries = entries_of(&lines);
    let results: Vec<(String, String, Option<String>)> = entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::ToolResult {
                tool_use_id,
                status,
                task_id,
            } => Some((tool_use_id.clone(), status.clone(), task_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            ("t1".into(), "success".into(), Some("7".into())),
            ("t2".into(), "error".into(), None),
            ("t3".into(), "interrupted".into(), None),
            ("t4".into(), "interrupted".into(), None),
            ("t5".into(), "success".into(), Some("9".into())),
        ]
    );
    let outputs: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::ToolOutput(o) => Some(o),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), 5);
    assert_eq!(outputs[0].stdout.as_deref(), Some("out"));
    assert_eq!(outputs[0].stderr.as_deref(), Some("err"));
    assert_eq!(outputs[0].content.as_deref(), Some("fine"));
    assert_eq!(
        outputs[0].raw,
        Some(json!({"stdout": "out", "stderr": "err", "task": {"id": "7"}}))
    );
    assert_eq!(outputs[1].tool_name.as_deref(), Some("Bash"));
    assert!(outputs[1].is_error && !outputs[1].is_interrupted);
    assert!(outputs[2].is_error && outputs[2].is_interrupted);
    assert_eq!(
        outputs[2].content.as_deref(),
        Some("The user doesn't want to proceed with this tool use.")
    );
    // A tool result is never a prompt or a chat message.
    assert!(!entries.iter().any(|e| matches!(
        e,
        TranscriptEntry::HumanPrompt { .. } | TranscriptEntry::Message(_)
    )));
}

#[test]
fn chat_messages_carry_their_blocks() {
    let line = json!({"type": "assistant", "uuid": "a1", "timestamp": "2027-01-15T10:00:00Z", "message": {
        "model": "claude-opus-4-5", "content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "hello"},
            {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "C:\\x\\y.ts"}},
            {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
            {"type": "image", "source": {"media_type": "image/png", "data": "AAAA"}},
            {"type": "text", "text": "[Request interrupted by user for tool use]"}]}});
    let entries = entries_of(&[line]);
    let message = entries
        .iter()
        .find_map(|e| match e {
            TranscriptEntry::Message(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(message.id, "a1");
    assert_eq!(message.role, ChatRole::Assistant);
    assert_eq!(message.at, parse_iso8601("2027-01-15T10:00:00Z"));
    // The streamed duplicate of t1 is dropped from the chat, not from the tool entries.
    assert_eq!(
        message.item_ids(),
        [
            "a1-thinking-0",
            "a1-text-1",
            "t1",
            "a1-image-3",
            "a1-interrupted-4"
        ]
    );
    let tools = entries
        .iter()
        .filter(|e| matches!(e, TranscriptEntry::ToolUse { .. }))
        .count();
    assert_eq!(tools, 2);
    // A meta line, a command echo and a line without uuid make no message.
    for line in [
        json!({"type": "assistant", "uuid": "m", "isMeta": true, "message": {"content": [{"type": "text", "text": "x"}]}}),
        json!({"type": "user", "uuid": "c", "message": {"content": "<local-command-stdout>x</local-command-stdout>"}}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "x"}]}}),
    ] {
        assert!(!entries_of(&[line])
            .iter()
            .any(|e| matches!(e, TranscriptEntry::Message(_))));
    }
}

#[test]
fn tasks_are_rebuilt_from_what_the_job_reads() {
    // The store feeds entries to the list in file order (SessionTaskListTests'
    // reconstruction: batches, a failed create, /clear).
    let (_guard, path) = transcript();
    let mut lines = vec![
        tool_use("c1", "TaskCreate", json!({"subject": "Old"}), t0()),
        tool_result("c1", "Task #1 created successfully: Old", t0(), None),
        tool_use(
            "u1",
            "TaskUpdate",
            json!({"taskId": "1", "status": "completed"}),
            t0(),
        ),
        tool_use("c2", "TaskCreate", json!({"subject": "Fails"}), t0()),
        json!({"type": "user", "uuid": "f", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "c2", "is_error": true, "content": "no"}]}}),
        tool_use("c3", "TaskCreate", json!({"subject": "New"}), t0()),
        tool_result(
            "c3",
            "x",
            t0(),
            Some(json!({"task": {"id": "3", "subject": "New"}})),
        ),
    ];
    write_lines(&path, &lines);
    let fold = |list: &mut TaskList, delta: &TranscriptDelta| {
        for entry in &delta.entries {
            match entry {
                TranscriptEntry::ToolUse { id, name, input } => {
                    list.apply_transcript_tool_use(id, name, input)
                }
                TranscriptEntry::ToolResult {
                    tool_use_id,
                    status,
                    task_id,
                } => list.apply_transcript_tool_result(
                    tool_use_id,
                    status != "success",
                    task_id.as_deref(),
                ),
                TranscriptEntry::Clear => list.reset(),
                _ => {}
            }
        }
    };
    let mut list = TaskList::new();
    let delta = sync(&path, TranscriptCursor::default(), false);
    fold(&mut list, &delta);
    let subjects: Vec<&str> = list.items().iter().map(|t| t.subject.as_str()).collect();
    assert_eq!(subjects, ["New"]);
    // A /clear wipes the list; later creates start it again.
    lines = vec![
        user("<command-name>/clear</command-name>", t0(), json!({})),
        tool_use("c4", "TaskCreate", json!({"subject": "After"}), t0()),
        tool_result("c4", "Task #1 created successfully: After", t0(), None),
    ];
    append_lines(&path, &lines);
    let delta = sync(&path, delta.cursor, false);
    fold(&mut list, &delta);
    let subjects: Vec<&str> = list.items().iter().map(|t| t.subject.as_str()).collect();
    assert_eq!(subjects, ["After"]);
}

// ---- reading ----

#[test]
fn a_missing_file_gives_an_empty_delta_without_reset() {
    let dir = tempfile::tempdir().unwrap();
    let cursor = TranscriptCursor {
        offset: 10,
        size: 10,
        file: None,
    };
    let delta = sync(&dir.path().join("none.jsonl"), cursor, false);
    assert!(delta.entries.is_empty() && !delta.reset);
    assert_eq!(delta.cursor, cursor);
    assert_eq!(delta.session, session());
}

#[test]
fn crlf_terminated_lines_decode() {
    let (_guard, path) = transcript();
    let mut content = Vec::new();
    for text in ["one", "two"] {
        content.extend_from_slice(assistant_text(text, t0()).to_string().as_bytes());
        content.extend_from_slice(b"\r\n");
    }
    content.extend_from_slice(b"\r\n   \r\n");
    std::fs::write(&path, &content).unwrap();
    let delta = sync(&path, TranscriptCursor::default(), false);
    assert_eq!(messages(&delta), ["one", "two"]);
    assert_eq!(delta.cursor.offset, content.len() as u64);
    assert_eq!(decode_line(b"  \r\n").len(), 0);
    assert_eq!(decode_line(b"not json").len(), 0);
}

#[test]
fn a_chunk_boundary_inside_a_line_loses_and_splits_nothing() {
    let (_guard, path) = transcript();
    let lines: Vec<Value> = (0..40)
        .map(|i| assistant_text(&format!("message {i} {}", "é😀".repeat(i % 7)), t0()))
        .collect();
    write_lines(&path, &lines);
    let size = std::fs::metadata(&path).unwrap().len();
    let whole = sync(&path, TranscriptCursor::default(), false);
    assert_eq!(messages(&whole).len(), 40);
    for chunk in [1usize, 2, 7, 64, 100, 333, 1024] {
        let mut cursor = TranscriptCursor::default();
        let mut seen = Vec::new();
        let mut calls = 0;
        loop {
            let delta =
                sync_transcript_chunked(&session(), &path, cursor, false, &StdSecureFiles, chunk);
            calls += 1;
            assert!(!delta.reset);
            let advanced = delta.cursor.offset > cursor.offset;
            seen.extend(messages(&delta));
            cursor = delta.cursor;
            if !advanced || cursor.offset >= size {
                break;
            }
        }
        assert_eq!(seen, messages(&whole), "chunk {chunk}");
        assert_eq!(cursor.offset, size, "chunk {chunk}");
        assert!(calls >= 1);
    }
}

#[test]
fn a_file_over_8_mib_is_read_over_several_calls() {
    let (_guard, path) = transcript();
    // About 9.5 MiB: 600 lines of 16 KiB, so a chunk boundary falls inside a line.
    let filler = "a".repeat(16 * 1024);
    let lines: Vec<Value> = (0..600)
        .map(|i| assistant_text(&format!("{i}:{filler}"), t0()))
        .collect();
    write_lines(&path, &lines);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > CHUNK_SIZE as u64);

    let mut cursor = TranscriptCursor::default();
    let mut numbers = Vec::new();
    let mut calls = 0;
    while cursor.offset < size {
        let delta = sync(&path, cursor, false);
        assert!(
            delta.cursor.offset > cursor.offset,
            "each call makes progress"
        );
        assert!(
            delta.cursor.offset - cursor.offset <= CHUNK_SIZE as u64,
            "one call reads at most a chunk"
        );
        assert_eq!(delta.cursor.size, size);
        numbers.extend(
            messages(&delta)
                .iter()
                .map(|m| m.split(':').next().unwrap().parse::<usize>().unwrap()),
        );
        cursor = delta.cursor;
        calls += 1;
    }
    assert!(calls >= 2, "{calls}");
    assert_eq!(numbers, (0..600).collect::<Vec<_>>());
}

#[test]
fn a_line_longer_than_a_chunk_is_read_whole() {
    let (_guard, path) = transcript();
    let huge = "b".repeat(5_000);
    write_lines(
        &path,
        &[assistant_text(&huge, t0()), assistant_text("small", t0())],
    );
    let mut cursor = TranscriptCursor::default();
    let mut seen = Vec::new();
    for _ in 0..3 {
        let delta =
            sync_transcript_chunked(&session(), &path, cursor, false, &StdSecureFiles, 1000);
        seen.extend(messages(&delta));
        cursor = delta.cursor;
    }
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], huge);
    assert_eq!(seen[1], "small");
}

/// A date past the year 9999 (chrono reads one with a sign) is no date: on
/// Windows the clock ends in the year 30828 and turning such a date into a
/// time would overflow (a panic in the read job). Checked by the decoder
/// before converting, so this holds on every platform.
#[test]
fn a_date_past_the_year_9999_is_no_date() {
    let at = |entries: &[TranscriptEntry]| {
        entries.iter().find_map(|entry| match entry {
            TranscriptEntry::HumanPrompt { at, .. } | TranscriptEntry::Assistant { at, .. } => {
                Some(*at)
            }
            _ => None,
        })
    };
    for stamp in [
        "+50000-01-01T00:00:00",
        "-50000-01-01T00:00:00",
        "+10000-01-01T00:00:00.000",
    ] {
        let prompt = json!({
            "type": "user", "uuid": "u1", "timestamp": stamp,
            "message": {"role": "user", "content": "hello"},
        });
        assert_eq!(
            at(&decode_line(prompt.to_string().as_bytes())),
            Some(None),
            "{stamp}"
        );
        let reply = json!({
            "type": "assistant", "uuid": "a1", "timestamp": stamp,
            "message": {"role": "assistant", "model": "claude-opus-4-5",
                        "content": [{"type": "text", "text": "hi"}]},
        });
        assert_eq!(
            at(&decode_line(reply.to_string().as_bytes())),
            Some(None),
            "{stamp}"
        );
    }
    // The last moment of the year 9999 is still a date.
    let prompt = json!({
        "type": "user", "uuid": "u1", "timestamp": "9999-12-31T23:59:59Z",
        "message": {"role": "user", "content": "hello"},
    });
    assert_eq!(
        at(&decode_line(prompt.to_string().as_bytes())),
        Some(parse_iso8601("9999-12-31T23:59:59Z"))
    );
}
