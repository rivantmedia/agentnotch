//! Chat history (sessions::chat): the LoadChat job, per-session chat state,
//! the open-chat policy and the patches sent to the panel. Ports of
//! A1_AttentionAndReviewTests.onlyOpenChatsKeepHistoriesAndTheOldestIsReleased,
//! A1_SessionStoreRegressionTests.aClosedChatKeepsOnlyTheNewestItemsAndRunningTools
//! and anOpenChatKeepsEverythingUntilReleased, plus the chat contract (D§3.6).

mod sessions_support;

use agentnotch_engine::model::*;
use agentnotch_engine::persist::json_equivalent;
use agentnotch_engine::runtime_types::TranscriptEntry;
use agentnotch_engine::sessions::chat::{
    chat_image, chat_update, filter_out_subagent_tools, load_chat, ChatState, OpenChats,
    SubagentTool, MAX_OPEN_HISTORIES, PAGE_SIZE, RETAINED_ITEMS,
};
use agentnotch_engine::sessions::transcript::decode_line;
use serde_json::{json, Value};
use sessions_support::*;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

fn at(seconds: u64) -> SystemTime {
    t0() + Duration::from_secs(seconds)
}

fn message(id: &str, role: ChatRole, seconds: u64, blocks: Vec<MessageBlock>) -> ChatMessage {
    ChatMessage {
        id: id.to_owned(),
        role,
        at: Some(at(seconds)),
        blocks,
    }
}

fn text(id: &str, seconds: u64, body: &str) -> ChatMessage {
    message(
        id,
        ChatRole::Assistant,
        seconds,
        vec![MessageBlock::Text(body.to_owned())],
    )
}

fn flat(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn ids(update: &ChatUpdate) -> Vec<&str> {
    update.items.iter().map(|i| i.id.as_str()).collect()
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

// ---- patches ----

#[test]
fn the_first_update_is_a_reset_and_later_ones_carry_only_what_changed() {
    let mut state = ChatState::new();
    state.merge_messages(
        &[
            message(
                "u1",
                ChatRole::User,
                1,
                vec![MessageBlock::Text("fix it".into())],
            ),
            text("a1", 2, "on it"),
        ],
        at(100),
        true,
    );
    state.place_tool("toolu_1", "Bash", &flat(&[("command", "ls")]), at(3));
    let first = state.history();
    let reset = chat_update("s1", None, &first, None, false, false);
    assert!(reset.reset);
    assert_eq!(ids(&reset), ["u1-text-0", "a1-text-0", "toolu_1"]);
    assert_eq!(reset.order, strs(&["u1-text-0", "a1-text-0", "toolu_1"]));
    assert!(reset.removed.is_empty());
    assert!(reset.revision >= 1);

    // A changed tool status yields exactly that item.
    state.set_tool_status("toolu_1", "waiting_for_approval", true);
    let second = state.history();
    let patch = chat_update(
        "s1",
        Some(&first),
        &second,
        Some("Thinking…".into()),
        false,
        false,
    );
    assert!(!patch.reset);
    assert_eq!(ids(&patch), ["toolu_1"]);
    assert!(matches!(&patch.items[0].body,
        ChatBody::Tool { status, .. } if status == "waiting_for_approval"));
    assert_eq!(patch.order, reset.order);
    assert_eq!(patch.working.as_deref(), Some("Thinking…"));
    assert!(patch.revision > reset.revision);

    // An appended message yields only it, and the order is complete.
    state.merge_messages(&[text("a2", 4, "done")], at(100), false);
    let third = state.history();
    let patch3 = chat_update("s1", Some(&second), &third, None, false, false);
    assert_eq!(ids(&patch3), ["a2-text-0"]);
    assert_eq!(
        patch3.order,
        strs(&["u1-text-0", "a1-text-0", "toolu_1", "a2-text-0"])
    );
    assert!(patch3.revision > patch.revision);

    // A /clear yields the removed ids.
    state.apply_entries(&[TranscriptEntry::Clear], at(1000), false);
    state.merge_messages(&[text("a3", 2000, "fresh")], at(2000), false);
    let fourth = state.history();
    let patch4 = chat_update("s1", Some(&third), &fourth, None, true, false);
    assert_eq!(ids(&patch4), ["a3-text-0"]);
    assert_eq!(
        patch4.removed,
        strs(&["u1-text-0", "a1-text-0", "toolu_1", "a2-text-0"])
    );
    assert_eq!(patch4.order, strs(&["a3-text-0"]));
    assert!(patch4.ended);
    assert!(patch4.revision > patch3.revision);

    // Nothing changed: an empty patch (the order still travels).
    let again = chat_update("s1", Some(&fourth), &fourth, None, true, false);
    assert!(again.items.is_empty() && again.removed.is_empty());
    assert_eq!(again.order, patch4.order);
    assert!(again.revision > fourth.revision);
}

#[test]
fn an_image_travels_by_reference_and_chat_image_returns_its_data_url() {
    let png = "iVBORw0KGgo=";
    let mut state = ChatState::new();
    state.merge_messages(
        &[message(
            "u9",
            ChatRole::User,
            1,
            vec![
                MessageBlock::Text("look".into()),
                MessageBlock::Image {
                    media_type: "image/png".into(),
                    data_base64: png.into(),
                },
            ],
        )],
        at(5),
        false,
    );
    let history = state.history();
    let update = chat_update("s1", None, &history, None, false, false);
    let image = update
        .items
        .iter()
        .find(|item| item.id == "u9-image-1")
        .expect("image item");
    match &image.body {
        ChatBody::Image {
            media_type,
            image_id,
            bytes,
        } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(image_id, "u9-image-1");
            assert_eq!(*bytes, 8);
        }
        other => panic!("{other:?}"),
    }
    // Never inline: the encoded update does not contain the data.
    assert!(!serde_json::to_string(&update).unwrap().contains(png));
    assert_eq!(
        chat_image(&history, "u9-image-1").as_deref(),
        Some("data:image/png;base64,iVBORw0KGgo=")
    );
    assert_eq!(chat_image(&history, "nope"), None);

    // Larger than 2 MiB: refused. Not an image: refused.
    let mut big = ChatHistory::default();
    big.images.insert(
        "huge".into(),
        ChatImage {
            media_type: "image/png".into(),
            data_base64: "A".repeat(3 * 1024 * 1024),
        },
    );
    big.images.insert(
        "html".into(),
        ChatImage {
            media_type: "text/html".into(),
            data_base64: "PGI+".into(),
        },
    );
    big.images.insert(
        "sneaky".into(),
        ChatImage {
            media_type: "image/png\"onload=\"x".into(),
            data_base64: "PGI+".into(),
        },
    );
    // The data is base64 or nothing: the URL never carries a quote, a
    // bracket or a space a transcript put in it.
    for (id, data) in [
        ("quote", "iVBO\"><img src=x onerror=alert(1)>"),
        ("space", "iVBO Rw0K"),
        ("paren", "iVBO)Rw0K"),
    ] {
        big.images.insert(
            id.into(),
            ChatImage {
                media_type: "image/png".into(),
                data_base64: data.into(),
            },
        );
    }
    assert_eq!(chat_image(&big, "huge"), None);
    assert_eq!(chat_image(&big, "html"), None);
    assert_eq!(chat_image(&big, "sneaky"), None);
    assert_eq!(chat_image(&big, "quote"), None);
    assert_eq!(chat_image(&big, "space"), None);
    assert_eq!(chat_image(&big, "paren"), None);
}

// ---- LoadChat ----

fn transcript_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("projects").join("proj").join("sess.jsonl")
}

fn many_lines(count: u64) -> Vec<Value> {
    (0..count)
        .map(|i| assistant_text(&format!("line {i}"), at(i)))
        .collect()
}

fn texts(page: &ChatPage) -> Vec<&str> {
    page.items
        .iter()
        .map(|item| match &item.body {
            ChatBody::User { text } | ChatBody::Assistant { text } => text.as_str(),
            _ => "",
        })
        .collect()
}

#[test]
fn a_page_is_150_items_and_earlier_pages_come_with_before() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(&path, &many_lines(400));
    let session = SessionId::from("sess");

    let newest = load_chat(&session, &path, None, PAGE_SIZE);
    assert_eq!(PAGE_SIZE, 150);
    assert_eq!(newest.items.len(), 150);
    assert_eq!(newest.has_earlier, 250);
    assert_eq!(newest.before, None);
    assert_eq!(newest.error, None);
    assert_eq!(texts(&newest)[0], "line 250");
    assert_eq!(texts(&newest)[149], "line 399");
    assert_eq!(newest.times.len(), 150);
    assert!(newest.times.iter().all(Option::is_some));

    let first_id = newest.items[0].id.clone();
    let earlier = load_chat(&session, &path, Some(&first_id), PAGE_SIZE);
    assert_eq!(earlier.items.len(), 150);
    assert_eq!(earlier.has_earlier, 100);
    assert_eq!(earlier.before.as_deref(), Some(first_id.as_str()));
    assert_eq!(texts(&earlier)[0], "line 100");
    assert_eq!(texts(&earlier)[149], "line 249");

    let oldest = load_chat(&session, &path, Some(&earlier.items[0].id), PAGE_SIZE);
    assert_eq!(oldest.items.len(), 100);
    assert_eq!(oldest.has_earlier, 0);
    assert_eq!(texts(&oldest)[0], "line 0");

    // A before-id the transcript no longer has gives the newest page.
    let lost = load_chat(&session, &path, Some("gone-text-0"), PAGE_SIZE);
    assert_eq!(lost.items, newest.items);
    assert_eq!(lost.before, None);
}

#[test]
fn a_clear_drops_what_came_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(
        &path,
        &[
            user("old question", at(1), json!({})),
            assistant_text("old answer", at(2)),
            user("<command-name>/clear</command-name>", at(3), json!({})),
            user("new question", at(4), json!({})),
            assistant_text("new answer", at(5)),
        ],
    );
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert_eq!(texts(&page), ["new question", "new answer"]);
    assert_eq!(page.has_earlier, 0);
}

#[test]
fn an_interrupt_marker_is_an_item_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    let marker = user("[Request interrupted by user]", at(2), json!({}));
    let marker_id = marker["uuid"].as_str().unwrap().to_owned();
    write_lines(&path, &[user("go", at(1), json!({})), marker]);
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[1].id, format!("{marker_id}-interrupted-0"));
    assert_eq!(page.items[1].body, ChatBody::Interrupted);
}

#[test]
fn a_tool_result_is_joined_to_its_call() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    let mut failed = tool_result("toolu_bad", "boom", at(5), None);
    failed["message"]["content"][0]["is_error"] = json!(true);
    let mut refused = tool_result("toolu_no", "The user doesn't want to proceed", at(7), None);
    refused["message"]["content"][0]["is_error"] = json!(true);
    write_lines(
        &path,
        &[
            tool_use(
                "toolu_read",
                "Read",
                json!({"file_path": "C:\\code\\src\\a.ts"}),
                at(1),
            ),
            tool_result(
                "toolu_read",
                "1\tfoo",
                at(2),
                Some(json!({"type": "text", "file": {
                    "filePath": "C:\\code\\src\\a.ts", "content": "foo",
                    "numLines": 1, "startLine": 1, "totalLines": 9}})),
            ),
            tool_use("toolu_bash", "Bash", json!({"command": "echo hi"}), at(3)),
            tool_result(
                "toolu_bash",
                "hi",
                at(4),
                Some(json!({"stdout": "hi\n", "stderr": "", "interrupted": false})),
            ),
            tool_use("toolu_bad", "Bash", json!({"command": "false"}), at(5)),
            failed,
            tool_use("toolu_no", "Bash", json!({"command": "rm x"}), at(6)),
            refused,
            tool_use("toolu_open", "Bash", json!({"command": "sleep 99"}), at(8)),
            tool_use("toolu_plain", "Mystery", json!({"q": "x"}), at(9)),
            tool_result("toolu_plain", "just words", at(10), None),
        ],
    );
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    let tool = |id: &str| -> &ChatBody {
        &page
            .items
            .iter()
            .find(|item| item.id == id)
            .unwrap_or_else(|| panic!("no item {id}"))
            .body
    };
    match tool("toolu_read") {
        ChatBody::Tool {
            name,
            summary,
            status,
            input,
            result,
            subagent,
        } => {
            assert_eq!(name, "Read");
            assert_eq!(summary, "a.ts");
            assert_eq!(status, "success");
            assert_eq!(input["file_path"], "C:\\code\\src\\a.ts");
            assert!(subagent.is_none());
            match result {
                Some(ToolResultView::Read {
                    file_path,
                    content,
                    total_lines,
                    ..
                }) => {
                    assert_eq!(file_path, "C:\\code\\src\\a.ts");
                    assert_eq!(content, "foo");
                    assert_eq!(*total_lines, 9);
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(tool("toolu_bash"),
        ChatBody::Tool { status, result: Some(ToolResultView::Bash { stdout, .. }), .. }
            if status == "success" && stdout == "hi\n"));
    assert!(matches!(tool("toolu_bad"),
        ChatBody::Tool { status, result: Some(ToolResultView::Generic { text: Some(t), .. }), .. }
            if status == "error" && t == "boom"));
    assert!(matches!(tool("toolu_no"),
        ChatBody::Tool { status, result: None, .. } if status == "interrupted"));
    assert!(matches!(tool("toolu_open"),
        ChatBody::Tool { status, result: None, .. } if status == "running"));
    assert!(matches!(tool("toolu_plain"),
        ChatBody::Tool { status, result: Some(ToolResultView::Generic { text: Some(t), .. }), .. }
            if status == "success" && t == "just words"));
}

#[test]
fn a_streamed_call_written_twice_is_one_item() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(
        &path,
        &[
            tool_use("toolu_dup", "Bash", json!({"command": "ls"}), at(1)),
            tool_use("toolu_dup", "Bash", json!({"command": "ls"}), at(2)),
        ],
    );
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert_eq!(page.items.len(), 1);
}

#[test]
fn an_agent_call_carries_its_subagents_tools() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(
        &path,
        &[
            tool_use(
                "toolu_agent",
                "Agent",
                json!({"description": "Find callers", "subagent_type": "Explore"}),
                at(1),
            ),
            tool_result(
                "toolu_agent",
                "Found 3.",
                at(9),
                Some(json!({"agentId": "a1b2c3", "status": "completed", "content": "Found 3."})),
            ),
        ],
    );
    // Current Claude Code nests a subagent's transcript under its session.
    let agent_file = dir
        .path()
        .join("projects/proj/sess/subagents/agent-a1b2c3.jsonl");
    write_lines(
        &agent_file,
        &[
            tool_use("sub_1", "Grep", json!({"pattern": "next\\("}), at(2)),
            tool_result("sub_1", "ok", at(3), None),
            tool_use("sub_2", "Read", json!({"file_path": "C:\\x\\y.ts"}), at(4)),
        ],
    );
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    let ChatBody::Tool {
        status,
        result,
        subagent,
        ..
    } = &page.items[0].body
    else {
        panic!("{:?}", page.items[0]);
    };
    assert_eq!(status, "success");
    assert!(matches!(result, Some(ToolResultView::Task { agent_id, .. }) if agent_id == "a1b2c3"));
    let view = subagent.as_ref().expect("subagent view");
    assert_eq!(view.agent_id.as_deref(), Some("a1b2c3"));
    assert_eq!(view.description.as_deref(), Some("Find callers"));
    assert_eq!(
        view.tools,
        vec![
            SubagentToolView {
                id: "sub_1".into(),
                name: "Grep".into(),
                summary: "next\\(".into(),
                status: "success".into()
            },
            SubagentToolView {
                id: "sub_2".into(),
                name: "Read".into(),
                summary: "y.ts".into(),
                status: "running".into()
            },
        ]
    );
}

#[test]
fn an_agent_id_cannot_name_another_folder() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(
        &path,
        &[
            tool_use("toolu_agent", "Agent", json!({"description": "x"}), at(1)),
            tool_result(
                "toolu_agent",
                "done",
                at(2),
                Some(json!({"agentId": "..\\..\\evil", "status": "completed"})),
            ),
        ],
    );
    write_lines(
        &dir.path().join("projects/evil.jsonl"),
        &[tool_use("sub_x", "Bash", json!({"command": "x"}), at(3))],
    );
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert!(matches!(
        &page.items[0].body,
        ChatBody::Tool { subagent: None, .. }
    ));
}

#[test]
fn an_image_is_an_item_with_its_data_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    let line = json!({
        "type": "user", "uuid": "img-1", "timestamp": "2027-01-15T10:00:00Z",
        "message": {"role": "user", "content": [
            {"type": "text", "text": "see"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                          "data": "iVBORw0KGgo="}}]},
    });
    write_lines(&path, &[line]);
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert_eq!(page.items.len(), 2);
    assert!(matches!(&page.items[1].body,
        ChatBody::Image { media_type, image_id, bytes }
            if media_type == "image/png" && image_id == "img-1-image-1" && *bytes == 8));
    assert_eq!(page.images["img-1-image-1"].data_base64, "iVBORw0KGgo=");
}

#[test]
fn an_unreadable_transcript_says_so_and_a_half_written_line_waits() {
    let dir = tempfile::tempdir().unwrap();
    let missing = load_chat(
        &SessionId::from("sess"),
        &dir.path().join("none.jsonl"),
        None,
        PAGE_SIZE,
    );
    assert!(missing.items.is_empty());
    assert!(missing.error.is_some());

    let path = transcript_in(&dir);
    write_lines(&path, &[assistant_text("whole", at(1))]);
    append_bytes(&path, br#"{"type":"assistant","uuid":"half","mess"#);
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert_eq!(texts(&page), ["whole"]);
    assert_eq!(page.error, None);
}

// ---- the store's items ----

#[test]
fn a_closed_chat_keeps_only_the_newest_items_and_running_tools() {
    let mut state = ChatState::new();
    state.place_tool(
        "toolu_long",
        "Bash",
        &flat(&[("command", "sleep 99")]),
        at(1),
    );
    for index in 0..200u64 {
        let id = format!("toolu_{index}");
        state.place_tool(&id, "Read", &flat(&[("file_path", "a.ts")]), at(2 + index));
        state.set_tool_status(&id, "success", true);
    }
    assert!(state.len() <= RETAINED_ITEMS + 1);
    assert!(state.item("toolu_long").is_some());
    assert_eq!(state.ids().last().copied(), Some("toolu_199"));
}

#[test]
fn an_open_chat_keeps_everything_until_released() {
    let mut state = ChatState::new();
    state.set_open(true);
    let messages: Vec<ChatMessage> = (0..120)
        .map(|i| text(&format!("m{i}"), i, &format!("line {i}")))
        .collect();
    state.merge_messages(&messages, at(1000), true);
    assert_eq!(state.len(), 120);
    assert!(state.is_open());
    state.set_open(false);
    assert_eq!(state.len(), RETAINED_ITEMS);
    assert_eq!(RETAINED_ITEMS, 40);
    // The newest ones survive.
    assert_eq!(state.ids().last().copied(), Some("m119-text-0"));
    assert_eq!(state.ids()[0], "m80-text-0");
}

#[test]
fn trimming_a_closed_chat_drops_the_images_of_dropped_items() {
    let mut state = ChatState::new();
    state.set_open(true);
    state.merge_messages(
        &[message(
            "img",
            ChatRole::User,
            1,
            vec![MessageBlock::Image {
                media_type: "image/png".into(),
                data_base64: "iVBORw0KGgo=".into(),
            }],
        )],
        at(5),
        false,
    );
    let messages: Vec<ChatMessage> = (0..60)
        .map(|i| text(&format!("m{i}"), 10 + i, "x"))
        .collect();
    state.merge_messages(&messages, at(5), false);
    assert!(chat_image(&state.history(), "img-image-0").is_some());
    state.set_open(false);
    assert!(state.item("img-image-0").is_none());
    assert!(chat_image(&state.history(), "img-image-0").is_none());
}

#[test]
fn a_hook_placeholder_meets_the_transcripts_call_and_result() {
    let mut state = ChatState::new();
    state.place_tool("toolu_1", "Bash", &flat(&[("command", "ls")]), at(10));
    state.set_tool_status("toolu_1", "waiting_for_approval", false);
    assert_eq!(state.tool_status("toolu_1"), Some("waiting_for_approval"));

    // The transcript: a prompt that came before the call, the call, its result.
    let line_user = user("list files", at(5), json!({}));
    let line_call = tool_use("toolu_1", "Bash", json!({"command": "ls -la"}), at(10));
    let line_result = tool_result(
        "toolu_1",
        "a\nb",
        at(11),
        Some(json!({"stdout": "a\nb", "stderr": ""})),
    );
    let entries = entries_of(&[line_user, line_call, line_result]);
    let outcome = state.apply_entries(&entries, at(100), true);
    assert!(outcome.changed);
    assert_eq!(outcome.completed_tools, ["toolu_1"]);
    assert!(!outcome.cleared);
    // One item for the call; the prompt sorts before it.
    assert_eq!(state.len(), 2);
    assert_eq!(state.ids()[1], "toolu_1");
    match &state.item("toolu_1").unwrap().body {
        ChatBody::Tool {
            status,
            input,
            result,
            summary,
            ..
        } => {
            assert_eq!(status, "success");
            assert_eq!(input["command"], "ls -la");
            assert_eq!(summary, "ls -la");
            assert!(
                matches!(result, Some(ToolResultView::Bash { stdout, .. }) if stdout == "a\nb")
            );
        }
        other => panic!("{other:?}"),
    }
    // A late outcome does not revive a finished tool.
    assert!(!state.set_tool_status("toolu_1", "interrupted", true));
    assert_eq!(state.tool_status("toolu_1"), Some("success"));
}

#[test]
fn a_hook_finished_tool_still_gets_the_transcripts_result() {
    let mut state = ChatState::new();
    state.place_tool("toolu_q", "Bash", &flat(&[("command", "echo q")]), at(10));
    state.set_tool_status("toolu_q", "success", true);
    let entries = entries_of(&[
        tool_use("toolu_q", "Bash", json!({"command": "echo q"}), at(10)),
        tool_result(
            "toolu_q",
            "q",
            at(11),
            Some(json!({"stdout": "q\n", "stderr": ""})),
        ),
    ]);
    let outcome = state.apply_entries(&entries, at(100), false);
    // It was not pending: nothing for the store's tracker to complete.
    assert!(outcome.completed_tools.is_empty());
    assert!(matches!(&state.item("toolu_q").unwrap().body,
        ChatBody::Tool { status, result: Some(_), .. } if status == "success"));
}

#[test]
fn interrupting_marks_running_tools_only() {
    let mut state = ChatState::new();
    state.place_tool("a", "Bash", &flat(&[("command", "x")]), at(1));
    state.place_tool("b", "Bash", &flat(&[("command", "y")]), at(2));
    state.set_tool_status("b", "success", true);
    state.place_tool("c", "Bash", &flat(&[("command", "z")]), at(3));
    state.set_tool_status("c", "waiting_for_approval", true);
    assert!(state.interrupt_running());
    assert_eq!(state.tool_status("a"), Some("interrupted"));
    assert_eq!(state.tool_status("b"), Some("success"));
    assert_eq!(state.tool_status("c"), Some("waiting_for_approval"));
    assert!(!state.interrupt_running());
}

#[test]
fn a_clear_keeps_the_placeholders_of_the_last_moments() {
    let mut state = ChatState::new();
    state.merge_messages(&[text("old", 1, "old")], at(1000), false);
    state.place_tool("toolu_old", "Bash", &flat(&[("command", "x")]), at(2));
    state.place_tool("toolu_new", "Bash", &flat(&[("command", "y")]), at(999));
    let outcome = state.apply_entries(&[TranscriptEntry::Clear], at(1000), false);
    assert!(outcome.cleared && outcome.changed);
    assert_eq!(state.ids(), ["toolu_new"]);
}

#[test]
fn subagent_tools_show_under_their_agent_call() {
    let mut state = ChatState::new();
    state.place_tool(
        "toolu_agent",
        "Agent",
        &flat(&[("description", "Find callers")]),
        at(1),
    );
    state.place_tool("sub_1", "Grep", &flat(&[("pattern", "x")]), at(2));
    state.place_tool(
        "sub_2",
        "Read",
        &flat(&[("file_path", "C:\\a\\b.ts")]),
        at(3),
    );
    let tools = vec![
        SubagentTool {
            id: "sub_1".into(),
            name: "Grep".into(),
            input: flat(&[("pattern", "x")]),
            completed: true,
        },
        SubagentTool {
            id: "sub_2".into(),
            name: "Read".into(),
            input: flat(&[("file_path", "C:\\a\\b.ts")]),
            completed: false,
        },
    ];
    assert!(state.apply_subagent_tools("toolu_agent", &tools));
    // The same list again leaves the item (and the revision) alone.
    let revision = state.revision();
    assert!(!state.apply_subagent_tools("toolu_agent", &tools));
    assert_eq!(state.revision(), revision);
    // A call that isn't an Agent takes none.
    assert!(!state.apply_subagent_tools("sub_1", &tools));
    assert!(!state.apply_subagent_tools("unknown", &tools));

    let history = state.history();
    let shown: Vec<&str> = history.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(shown, ["toolu_agent"]);
    // The state still has them (late hooks find their placeholders).
    assert_eq!(state.len(), 3);
    match &history.items[0].body {
        ChatBody::Tool {
            subagent: Some(view),
            ..
        } => {
            assert_eq!(view.description.as_deref(), Some("Find callers"));
            assert_eq!(view.tools.len(), 2);
            assert_eq!(view.tools[0].status, "success");
            assert_eq!(view.tools[1].status, "running");
            assert_eq!(view.tools[1].summary, "b.ts");
        }
        other => panic!("{other:?}"),
    }
    let all: Vec<ChatItem> = state.items().cloned().collect();
    assert_eq!(filter_out_subagent_tools(&all).len(), 1);
    // Without a subagent view nothing is filtered.
    let plain: Vec<ChatItem> = all[1..].to_vec();
    assert_eq!(filter_out_subagent_tools(&plain).len(), 2);
}

#[test]
fn a_loaded_page_merges_into_the_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(&path, &many_lines(300));
    let session = SessionId::from("sess");
    let mut state = ChatState::new();
    state.set_open(true);
    // A hook placeholder from before the chat opened.
    state.place_tool("toolu_h", "Bash", &flat(&[("command", "x")]), at(299));

    let newest = load_chat(&session, &path, None, PAGE_SIZE);
    assert!(state.merge_page(&newest, at(5000)));
    assert_eq!(state.len(), 151);
    assert_eq!(state.has_earlier(), 150);
    // The placeholder sorts by its own time among the lines.
    assert_eq!(
        state.ids().last().copied(),
        Some(newest.items[149].id.as_str())
    );
    assert_eq!(state.ids()[149], "toolu_h");

    let earlier = load_chat(&session, &path, Some(&newest.items[0].id), PAGE_SIZE);
    assert!(state.merge_page(&earlier, at(5000)));
    assert_eq!(state.len(), 301);
    assert_eq!(state.has_earlier(), 0);
    // Merged twice: nothing new, nothing changes.
    let revision = state.revision();
    assert!(!state.merge_page(&earlier, at(5000)));
    assert_eq!(state.revision(), revision);
    // The newest page again does not bring back an earlier count.
    assert!(!state.merge_page(&newest, at(5000)));
    assert_eq!(state.has_earlier(), 0);
    // An error page changes nothing.
    let bad = load_chat(&session, &dir.path().join("none.jsonl"), None, PAGE_SIZE);
    assert!(!state.merge_page(&bad, at(5000)));
}

#[test]
fn a_loaded_page_gives_a_placed_call_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_in(&dir);
    write_lines(
        &path,
        &[
            tool_use("toolu_1", "Bash", json!({"command": "ls"}), at(1)),
            tool_result(
                "toolu_1",
                "a",
                at(2),
                Some(json!({"stdout": "a\n", "stderr": ""})),
            ),
        ],
    );
    let mut state = ChatState::new();
    state.place_tool("toolu_1", "Bash", &flat(&[("command", "ls")]), at(1));
    let page = load_chat(&SessionId::from("sess"), &path, None, PAGE_SIZE);
    assert!(state.merge_page(&page, at(100)));
    assert_eq!(state.len(), 1);
    assert!(matches!(&state.item("toolu_1").unwrap().body,
        ChatBody::Tool { status, result: Some(ToolResultView::Bash { .. }), .. } if status == "success"));
}

#[test]
fn entries_decoded_from_lines_fold_like_a_loaded_page() {
    let lines = [
        user("hello", at(1), json!({})),
        assistant_text("hi there", at(2)),
        tool_use("toolu_x", "Bash", json!({"command": "ls"}), at(3)),
    ];
    let mut state = ChatState::new();
    let entries: Vec<TranscriptEntry> = lines
        .iter()
        .flat_map(|line| decode_line(line.to_string().as_bytes()))
        .collect();
    state.apply_entries(&entries, at(50), false);
    assert_eq!(state.len(), 3);
    assert_eq!(state.tool_status("toolu_x"), Some("running"));
}

// ---- open chats ----

#[test]
fn only_open_chats_keep_histories_and_the_oldest_is_released() {
    let mut open = OpenChats::new();
    assert_eq!(MAX_OPEN_HISTORIES, 2);
    let [c0, c1, c2] = ["c0", "c1", "c2"].map(SessionId::from);

    assert!(open.touch(&c0).is_empty());
    assert!(open.touch(&c1).is_empty());
    open.mark_loaded(&c0);
    assert_eq!(open.ids(), [c0.clone(), c1.clone()]);
    assert!(open.is_loaded(&c0));

    // Opening a third releases the one touched longest ago.
    assert_eq!(open.touch(&c2), vec![c0.clone()]);
    assert_eq!(open.ids(), [c1.clone(), c2.clone()]);
    assert!(!open.is_open(&c0));
    assert!(!open.is_loaded(&c0));

    // Touching an open chat again makes it the newest; nothing is released.
    assert!(open.touch(&c1).is_empty());
    assert_eq!(open.ids(), [c2.clone(), c1.clone()]);

    assert!(open.close(&c1));
    assert_eq!(open.ids(), std::slice::from_ref(&c2));
    assert!(!open.close(&c1));
    // Only an open chat can be marked loaded.
    open.mark_loaded(&c1);
    assert!(!open.is_loaded(&c1));
}

// ---- the UI contract ----

#[test]
fn the_chat_fixture_deserialises_and_a_reset_serialises_with_the_same_keys() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui-contract/chat.json");
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let parsed: ChatUpdate = serde_json::from_value(fixture.clone()).unwrap();

    let history = ChatHistory {
        items: parsed.items.clone(),
        has_earlier: parsed.has_earlier,
        images: BTreeMap::new(),
        revision: parsed.revision,
    };
    let reset = chat_update(
        &parsed.session_id,
        None,
        &history,
        parsed.working.clone(),
        parsed.ended,
        parsed.loading,
    );
    assert!(reset.reset);
    let built = serde_json::to_value(&reset).unwrap();
    let keys =
        |value: &Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&built), keys(&fixture));
    for (built, expected) in built["items"]
        .as_array()
        .unwrap()
        .iter()
        .zip(fixture["items"].as_array().unwrap())
    {
        assert_eq!(keys(built), keys(expected));
        assert!(json_equivalent(built, expected));
    }
    assert!(json_equivalent(&built, &fixture));
}
