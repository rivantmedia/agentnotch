//! The session store's transcript syncs, chat and restored-state inference:
//! A1_TranscriptParserTests (store level), SessionCoreRegressionTests'
//! preamble, restored-review and subagent-cache tests, A1_SessionStoreRegression-
//! Tests' completions from a transcript, plus the chat calls the panel uses
//! (a reset on open, then patches of the changed items; images by id).
//!
//! The Mac's parser actor became a job and the store's own state: the
//! tests run the jobs themselves (`Harness::run_jobs`: `sync_transcript` and
//! `load_chat` on the emitted job, the result fed back), with real files in
//! a temporary folder.
//!
//! Not ported, with their reason: `timestampsParseWithAndWithoutFractional-
//! Seconds` (the decoder's, in sessions_transcript.rs); `partialLinesWait-
//! ForTheirNewline` (the job's, in sessions_transcript.rs; here only that a
//! cursor carries on); the parser statistics (`linesDecoded`, `syncs`) have
//! no counterpart: jobs are counted instead.

mod sessions_support;

use agentnotch_engine::model::{ChatBody, SessionId, SessionState};
use agentnotch_engine::runtime_types::{Job, Release};
use agentnotch_engine::sessions::background::WaitTiming;
use agentnotch_engine::sessions::chat::{RETAINED_ITEMS, SUCCESS};
use agentnotch_engine::sessions::completion::CompletionTiming;
use agentnotch_engine::sessions::store::CONTEXT_RESUME_PREFIX;
use agentnotch_engine::sessions::transcript::is_agent_transcript;
use serde_json::{json, Value};
use sessions_support::{
    append_bytes, append_lines, assistant_text, line_bytes, registry_entry, t0, tool_result,
    tool_use, user, write_lines, Harness,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn s1() -> SessionId {
    SessionId::from("s1")
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn before(n: u64) -> SystemTime {
    t0() - secs(n)
}

/// A harness reading real transcripts under `dir`, and the transcript path.
fn reading(dir: &Path) -> (Harness, PathBuf) {
    let h = Harness::reading(dir, CompletionTiming::IMMEDIATE, WaitTiming::STANDARD);
    let path = PathBuf::from(&h.transcript);
    (h, path)
}

fn prompt(h: &mut Harness) {
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
}

/// A hook event that asks for another read, then the read.
fn resync(h: &mut Harness) {
    h.hook("PostToolUse", "processing");
    h.sync();
}

fn texts(h: &Harness) -> Vec<String> {
    h.session()
        .unwrap()
        .chat
        .items()
        .filter_map(|item| match &item.body {
            ChatBody::User { text } | ChatBody::Assistant { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn tool_lines(count: usize) -> Vec<Value> {
    let mut lines = Vec::new();
    for index in 0..count {
        let at = before(1000 - index as u64 * 2);
        lines.push(tool_use(
            &format!("tool-{index}"),
            "Read",
            json!({"file_path": format!("/tmp/f{index}")}),
            at,
        ));
        lines.push(tool_result(
            &format!("tool-{index}"),
            &"x".repeat(2_000),
            at + Duration::from_millis(500),
            Some(json!({"file": {"filePath": format!("/tmp/f{index}"), "content": "c"}})),
        ));
    }
    lines
}

// ---- A1_TranscriptParserTests, at the store ----

#[test]
fn one_sync_feeds_summary_tasks_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(
        &path,
        &[
            json!({"type": "ai-title", "aiTitle": "Build the thing"}),
            user("please build it", before(10), json!({})),
            tool_use(
                "t1",
                "TaskCreate",
                json!({"subject": "Design", "activeForm": "Designing"}),
                before(9),
            ),
            tool_result(
                "t1",
                "Task #1 created successfully: Design",
                before(8),
                Some(json!({"task": {"id": "1", "subject": "Design"}})),
            ),
            assistant_text("Done designing.", before(7)),
        ],
    );
    prompt(&mut h);
    h.sync();

    let session = h.session().unwrap();
    assert_eq!(
        session.conversation_info.title.as_deref(),
        Some("Build the thing")
    );
    let subjects: Vec<&str> = session
        .tasks
        .items()
        .iter()
        .map(|task| task.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Design"]);
    assert_eq!(session.chat.len(), 3);
    assert_eq!(session.chat.tool_status("t1"), Some(SUCCESS));
    assert_eq!(
        session.conversation_info.last_message.as_deref(),
        Some("Done designing.")
    );
    assert_eq!(
        session.cursor.offset,
        std::fs::metadata(&path).unwrap().len()
    );
    assert_eq!(h.reads_of("s1.jsonl"), 1);

    // Nothing appended: the read finds nothing and changes nothing.
    let revision = session.chat.revision();
    let views = h.store.views();
    resync(&mut h);
    assert_eq!(h.reads_of("s1.jsonl"), 2);
    assert_eq!(h.session().unwrap().chat.revision(), revision);
    assert_eq!(h.store.views().len(), views.len());
    assert!(h.session().unwrap().agents.is_empty());
}

#[test]
fn a_session_whose_chat_is_closed_gets_only_the_newest_messages() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &tool_lines(200));
    prompt(&mut h);
    // A call the hooks showed as running, long gone from the newest lines.
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Read".into());
        b.tool_use_id = Some("tool-3".into());
    });
    assert_eq!(
        h.session().unwrap().chat.tool_status("tool-3"),
        Some("running")
    );
    h.sync();

    let session = h.session().unwrap();
    assert_eq!(session.chat.len(), RETAINED_ITEMS);
    assert!(!session.chat.is_open());
    let newest = session.chat.item("tool-199").expect("the newest call");
    assert!(matches!(
        &newest.body,
        ChatBody::Tool { status, result: Some(_), .. } if status == SUCCESS
    ));
    assert!(session.chat.item("tool-50").is_none());
    // Its result was applied although its line is old: it is no longer
    // running anywhere.
    assert!(session.chat.item("tool-3").is_none());
    assert!(!session.tool_tracker.contains("tool-3"));

    // Opening the chat reads the newest page, then earlier ones on request.
    let opened = h.store.open_chat(&s1(), h.now);
    let first = h.store.take_chat_updates();
    assert_eq!(first.len(), 1);
    assert!(first[0].reset && first[0].loading);
    assert_eq!(first[0].items.len(), RETAINED_ITEMS);
    h.run_jobs(opened.jobs);
    let session = h.session().unwrap();
    assert!(session.chat.is_open());
    assert_eq!(session.chat.len(), 150);
    assert_eq!(session.chat.has_earlier(), 50);

    let loaded = h.store.take_chat_updates();
    assert_eq!(loaded.len(), 1);
    assert!(!loaded[0].reset && !loaded[0].loading);
    assert_eq!(loaded[0].items.len(), 150 - RETAINED_ITEMS);
    assert_eq!(loaded[0].order.len(), 150);

    let oldest = h.session().unwrap().chat.ids()[0].to_owned();
    assert_eq!(oldest, "tool-50");
    let jobs = h.store.chat_more(&s1(), &oldest);
    assert!(matches!(&jobs[..], [Job::LoadChat { before: Some(id), .. }] if id == "tool-50"));
    h.run_jobs(jobs);
    assert_eq!(h.session().unwrap().chat.len(), 200);
    assert_eq!(h.session().unwrap().chat.has_earlier(), 0);
    // A page of earlier items is sent as a reset.
    let paged = h.store.take_chat_updates();
    assert_eq!(paged.len(), 1);
    assert!(paged[0].reset);
    assert_eq!(paged[0].items.len(), 200);

    // Closing releases the history down to the retention rule.
    h.store.close_chat(&s1());
    assert_eq!(h.session().unwrap().chat.len(), RETAINED_ITEMS);
    assert!(h.store.take_chat_updates().is_empty());
}

#[test]
fn a_rewritten_transcript_is_read_again_and_replaces_history() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let old: Vec<Value> = (0..5)
        .map(|index| assistant_text(&format!("old {index}"), before(100 - index)))
        .collect();
    write_lines(&path, &old);
    prompt(&mut h);
    h.sync();
    assert_eq!(h.session().unwrap().chat.len(), 5);

    // Shorter than what was read: rewritten.
    write_lines(&path, &[assistant_text("new", before(10))]);
    resync(&mut h);
    let session = h.session().unwrap();
    assert_eq!(texts(&h), ["new"]);
    assert_eq!(session.fold.turn().reply_text.as_deref(), Some("new"));
    assert_eq!(
        session.conversation_info.last_message.as_deref(),
        Some("new")
    );
    assert_eq!(
        session.cursor.offset,
        std::fs::metadata(&path).unwrap().len()
    );
}

#[test]
fn clear_after_the_first_read_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &[assistant_text("before", before(100))]);
    prompt(&mut h);
    h.sync();
    assert_eq!(texts(&h), ["before"]);

    append_lines(
        &path,
        &[
            user("<command-name>/clear</command-name>", before(50), json!({})),
            assistant_text("after", before(40)),
        ],
    );
    resync(&mut h);
    assert_eq!(texts(&h), ["after"]);
}

#[test]
fn a_cursor_carries_on_over_a_half_written_line() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let second = line_bytes(&assistant_text("two", before(8)));
    let mut bytes = line_bytes(&assistant_text("one", before(9)));
    bytes.extend_from_slice(&second[..20]);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    prompt(&mut h);
    h.sync();
    assert_eq!(texts(&h), ["one"]);

    append_bytes(&path, &second[20..]);
    resync(&mut h);
    assert_eq!(texts(&h), ["one", "two"]);
    assert_eq!(
        h.session().unwrap().fold.turn().reply_text.as_deref(),
        Some("two")
    );
}

#[test]
fn a_transcript_longer_than_one_read_is_read_in_a_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    // About 9 MiB: more than one 8 MiB read.
    let filler = "x".repeat(1_000);
    let lines: Vec<Value> = (0..9_000)
        .map(|index| assistant_text(&format!("{index} {filler}"), before(20_000 - index)))
        .collect();
    write_lines(&path, &lines);
    prompt(&mut h);
    h.sync();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > 8 * 1024 * 1024);
    assert_eq!(h.session().unwrap().cursor.offset, size);
    assert_eq!(h.reads_of("s1.jsonl"), 2);
    let last = format!("8999 {filler}");
    assert_eq!(
        h.session().unwrap().fold.turn().reply_text.as_deref(),
        Some(last.as_str())
    );
}

// ---- scheduling ----

#[test]
fn a_burst_of_events_is_one_read_and_one_read_is_in_flight_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &[assistant_text("one", before(5))]);
    let start = h.now;
    prompt(&mut h);
    assert_eq!(
        h.store.next_deadline(),
        Some(start + Duration::from_millis(100))
    );
    h.hook("PostToolUse", "processing");
    h.hook("PostToolUse", "processing");

    h.now = start + Duration::from_millis(150);
    let jobs = h.tick().jobs;
    assert_eq!(jobs.len(), 1);
    assert!(matches!(&jobs[0], Job::SyncTranscript { cursor, .. } if cursor.offset == 0));

    // Another event while the read is out: no second job...
    h.hook("PostToolUse", "processing");
    h.now += Duration::from_millis(150);
    assert!(h.tick().jobs.is_empty());
    // ...but one more read when the first is back.
    h.run_jobs(jobs);
    assert!(h.store.next_deadline().is_some());
    append_lines(&path, &[assistant_text("two", before(4))]);
    h.sync();
    assert_eq!(texts(&h), ["one", "two"]);
    assert_eq!(h.reads_of("s1.jsonl"), 2);
}

#[test]
fn a_session_without_a_transcript_asks_for_no_read() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = reading(dir.path());
    prompt(&mut h);
    // The file is not there yet (its first line comes after the hook).
    assert!(h.sync().is_empty());
    assert_eq!(h.reads_of("s1.jsonl"), 0);
}

#[test]
fn a_registry_session_finds_its_transcript_from_its_folder_and_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &[assistant_text("hello", before(5))]);
    let folder = Harness::folder_of(dir.path());
    let now = h.now;
    h.registry_entries(&folder, false, vec![registry_entry("s1", 77, "busy", now)]);
    h.sync();
    assert_eq!(texts(&h), ["hello"]);
    assert_eq!(
        h.session()
            .unwrap()
            .transcript_path
            .as_deref()
            .map(Path::new),
        Some(path.as_path())
    );
}

#[test]
fn an_ended_session_leaves_no_read_state() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &[assistant_text("hi", before(5))]);
    prompt(&mut h);
    h.now += Duration::from_millis(150);
    let jobs = h.tick().jobs;
    assert_eq!(jobs.len(), 1);
    h.hook("SessionEnd", "ended");
    assert!(h.store.views().is_empty());

    // The read comes back after the session ended: nothing is revived.
    let late = h.run_jobs(jobs);
    assert!(late.is_empty());
    assert!(h.store.views().is_empty());
    assert!(h.store.next_deadline().is_none());
    assert!(!h.store.has_open_chats());
    assert!(h.store.interrupt_watches().is_empty());
}

// ---- subagent transcripts ----

fn agent_path(main: &Path, agent: &str) -> PathBuf {
    main.parent()
        .unwrap()
        .join("s1")
        .join("subagents")
        .join(format!("agent-{agent}.jsonl"))
}

fn write_agent(main: &Path, agent: &str, tools: usize, completed: bool) {
    let mut lines = Vec::new();
    for index in 0..tools {
        let id = format!("{agent}-{index}");
        lines.push(tool_use(
            &id,
            "Grep",
            json!({"pattern": format!("p{index}")}),
            before(5),
        ));
        if completed || index < tools - 1 {
            lines.push(tool_result(&id, "ok", before(4), None));
        }
    }
    write_lines(&agent_path(main, agent), &lines);
}

fn agent_call(id: &str, agent: &str, status: &str) -> Vec<Value> {
    vec![
        tool_use(
            id,
            "Agent",
            json!({"description": "look around"}),
            before(6),
        ),
        tool_result(
            id,
            "found it",
            before(5),
            Some(json!({"agentId": agent, "status": status, "content": "found it"})),
        ),
    ]
}

/// The tool statuses under an Agent call of the chat.
fn agent_tools(h: &Harness, call: &str) -> Option<Vec<String>> {
    match &h.session().unwrap().chat.item(call)?.body {
        ChatBody::Tool {
            subagent: Some(view),
            ..
        } => Some(view.tools.iter().map(|tool| tool.status.clone()).collect()),
        _ => None,
    }
}

fn agent_reads(h: &Harness) -> usize {
    h.ran
        .iter()
        .filter(|job| matches!(job, Job::SyncTranscript { path, .. } if is_agent_transcript(path)))
        .count()
}

#[test]
fn finished_agents_are_read_once_not_on_every_sync() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let mut lines = Vec::new();
    for index in 0..40 {
        write_agent(&path, &format!("a{index}"), 20, true);
        lines.extend(agent_call(
            &format!("call-{index}"),
            &format!("a{index}"),
            "completed",
        ));
    }
    write_lines(&path, &lines);
    prompt(&mut h);
    h.sync();

    let tools = agent_tools(&h, "call-7").expect("the Agent call shows its tools");
    assert_eq!(tools.len(), 20);
    assert!(tools.iter().all(|status| status == SUCCESS));
    assert_eq!(agent_reads(&h), 40);
    assert!(h.session().unwrap().agents.is_empty());

    // The periodic resync of a working session: nothing new anywhere.
    for _ in 0..10 {
        resync(&mut h);
    }
    assert_eq!(agent_reads(&h), 40);

    // One more agent: only it is read.
    write_agent(&path, "late", 3, true);
    append_lines(&path, &agent_call("call-late", "late", "completed"));
    resync(&mut h);
    assert_eq!(agent_reads(&h), 41);
    assert_eq!(agent_tools(&h, "call-late").unwrap().len(), 3);
}

#[test]
fn a_running_background_agent_is_followed_until_it_settles() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_agent(&path, "bg", 2, false);
    write_lines(&path, &agent_call("call-bg", "bg", "async_launched"));
    prompt(&mut h);
    h.sync();
    assert_eq!(agent_tools(&h, "call-bg").unwrap(), ["success", "running"]);
    let followed: Vec<&String> = h.session().unwrap().agents.keys().collect();
    assert_eq!(followed, ["call-bg"]);

    // Unchanged file: read, nothing to apply.
    let revision = h.session().unwrap().chat.revision();
    resync(&mut h);
    assert_eq!(h.session().unwrap().chat.revision(), revision);
    assert_eq!(h.session().unwrap().agents.len(), 1);

    // It finishes its last tool: only the change is read and reported.
    append_lines(
        &agent_path(&path, "bg"),
        &[tool_result("bg-1", "ok", before(3), None)],
    );
    resync(&mut h);
    assert_eq!(agent_tools(&h, "call-bg").unwrap(), ["success", "success"]);
    assert_eq!(h.session().unwrap().agents.len(), 1);

    // Ten silent minutes later it is given up on.
    h.advance(11 * 60);
    resync(&mut h);
    assert!(h.session().unwrap().agents.is_empty());
    assert!(h.session().unwrap().settled_agents.contains("call-bg"));
}

#[test]
fn at_most_sixty_four_agents_are_followed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let mut lines = Vec::new();
    for index in 0..70 {
        lines.extend(agent_call(
            &format!("call-{index:02}"),
            &format!("a{index:02}"),
            "async_launched",
        ));
    }
    write_lines(&path, &lines);
    prompt(&mut h);
    h.sync();
    let session = h.session().unwrap();
    assert_eq!(session.agents.len(), 64);
    assert_eq!(session.settled_agents.len(), 6);
    assert!(session.settled_agents.contains("call-00"));
    assert!(session.agents.contains_key("call-69"));
}

#[test]
fn a_finished_agent_whose_transcript_never_shows_up_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &agent_call("call-x", "ghost", "completed"));
    prompt(&mut h);
    h.sync();
    assert_eq!(h.session().unwrap().agents.len(), 1);
    resync(&mut h);
    assert_eq!(h.session().unwrap().agents.len(), 1);
    resync(&mut h);
    assert!(h.session().unwrap().agents.is_empty());
    assert!(h.session().unwrap().settled_agents.contains("call-x"));
}

#[test]
fn a_running_agent_found_in_a_loaded_chat_is_followed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_agent(&path, "bg", 2, false);
    write_lines(&path, &agent_call("call-bg", "bg", "async_launched"));
    // The chat opens before any sync read the transcript.
    prompt(&mut h);
    let opened = h.store.open_chat(&s1(), h.now);
    h.run_jobs(opened.jobs);
    assert_eq!(agent_tools(&h, "call-bg").unwrap(), ["success", "running"]);
    assert!(h.session().unwrap().agents.contains_key("call-bg"));
}

#[test]
fn subagent_tool_cache_follows_file_growth() {
    // SubagentTranscript's growth, at the job level: two reads of one file.
    use agentnotch_engine::core::atomic::StdSecureFiles;
    use agentnotch_engine::runtime_types::TranscriptCursor;
    use agentnotch_engine::sessions::chat::SubagentTranscript;
    use agentnotch_engine::sessions::transcript::sync_transcript;

    let dir = tempfile::tempdir().unwrap();
    let (h, main) = reading(dir.path());
    let agent = agent_path(&main, "a1");
    write_lines(
        &agent,
        &[tool_use("t1", "Read", json!({"file_path": "/tmp/x"}), t0())],
    );
    let mut transcript = SubagentTranscript::default();
    let session = s1();

    let first = sync_transcript(
        &session,
        &agent,
        TranscriptCursor::default(),
        true,
        &StdSecureFiles,
    );
    assert!(transcript.fold(&first.entries));
    let ids: Vec<&str> = transcript.tools().iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1"]);
    let idle = sync_transcript(&session, &agent, first.cursor, true, &StdSecureFiles);
    assert!(idle.entries.is_empty());

    append_lines(
        &agent,
        &[tool_use("t2", "Read", json!({"file_path": "/tmp/y"}), t0())],
    );
    let second = sync_transcript(&session, &agent, first.cursor, true, &StdSecureFiles);
    assert!(transcript.fold(&second.entries));
    let ids: Vec<&str> = transcript.tools().iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2"]);
    drop(h);
}

// ---- SessionCoreRegressionTests: the context preamble ----

fn preamble_line(text: &str, at: SystemTime) -> Value {
    user(text, at, json!({}))
}

#[test]
fn stale_transcript_preamble_does_not_hide_a_completion() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let preamble = format!(
        "{CONTEXT_RESUME_PREFIX} that ran out of context. The conversation is summarized below."
    );
    prompt(&mut h);
    write_lines(&path, &[preamble_line(&preamble, before(5))]);
    h.sync();
    assert!(h
        .session()
        .unwrap()
        .conversation_info
        .last_message
        .as_deref()
        .is_some_and(|message| message.starts_with(CONTEXT_RESUME_PREFIX)));

    // The hook's own final message is what counts.
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Done: all green.".into())
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);
}

#[test]
fn preamble_still_suppresses_without_a_hook_message() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    prompt(&mut h);
    write_lines(
        &path,
        &[preamble_line(
            &format!("{CONTEXT_RESUME_PREFIX}."),
            before(5),
        )],
    );
    h.sync();
    h.hook("Stop", "waiting_for_input");
    assert!(h.session().unwrap().completed_at.is_none());
}

// ---- restored review state (the first sync of a rediscovered session) ----

/// A turn finished in a first run; the review file it wrote.
fn first_run_finished(message: &str) -> (Vec<u8>, SystemTime) {
    use agentnotch_engine::runtime_types::PersistFile;
    let mut h = Harness::new();
    h.store.load_review(None, h.now);
    prompt(&mut h);
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some(message.into())
    });
    let completed_at = h.session().unwrap().completed_at.expect("completed");
    h.now += secs(1);
    let bytes = h
        .tick()
        .jobs
        .into_iter()
        .find_map(|job| match job {
            Job::Persist {
                file: PersistFile::Review,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .expect("the review file is asked for");
    (bytes, completed_at)
}

/// The next run: rediscovered through the registry as idle.
fn second_run(dir: &Path, bytes: &[u8], at: SystemTime) -> Harness {
    let mut h = Harness::reading(dir, CompletionTiming::IMMEDIATE, WaitTiming::STANDARD);
    h.now = at;
    h.store.load_review(Some(bytes), at);
    let folder = Harness::folder_of(dir);
    h.registry_entries(&folder, false, vec![registry_entry("s1", 77, "idle", at)]);
    h
}

#[test]
fn prompt_after_a_restored_completion_counts_as_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let (bytes, _) = first_run_finished("Shipped");
    let mut second = second_run(dir.path(), &bytes, t0() + secs(100));
    assert_eq!(second.state(), SessionState::ReadyForReview);

    // ...but its transcript shows the user prompted again after that result.
    let prompted_at = t0() + secs(60);
    write_lines(
        Path::new(&second.transcript),
        &[user("and now the docs", prompted_at, json!({}))],
    );
    second.sync();
    let restored = second.session().unwrap();
    assert_eq!(second.state(), SessionState::Idle);
    assert_eq!(restored.reviewed_at, Some(prompted_at));
}

#[test]
fn an_older_prompt_leaves_a_restored_completion_for_review() {
    let dir = tempfile::tempdir().unwrap();
    let (bytes, completed_at) = first_run_finished("Shipped");
    let mut second = second_run(dir.path(), &bytes, t0() + secs(100));
    write_lines(
        Path::new(&second.transcript),
        &[user("fix it", completed_at - secs(30), json!({}))],
    );
    second.sync();
    assert_eq!(second.state(), SessionState::ReadyForReview);
}

#[test]
fn stop_of_a_session_first_seen_mid_turn_is_a_completion() {
    let dir = tempfile::tempdir().unwrap();
    let (bytes, old_completion) = first_run_finished("First result");

    // The app restarts while the user runs another turn; its Stop is the
    // first event the new run sees.
    let mut second = Harness::reading(
        dir.path(),
        CompletionTiming::IMMEDIATE,
        WaitTiming::STANDARD,
    );
    second.now = t0() + secs(100);
    second.store.load_review(Some(&bytes), second.now);
    second.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Second result".into())
    });
    let new_completion = second.session().unwrap().completed_at.expect("completed");
    assert!(new_completion > old_completion);

    // The prompt of that turn predates its Stop: still waiting for review.
    write_lines(
        Path::new(&second.transcript),
        &[
            user("second", t0() + secs(50), json!({})),
            assistant_text("Second result", t0() + secs(99)),
        ],
    );
    second.sync();
    assert_eq!(second.state(), SessionState::ReadyForReview);
}

#[test]
fn a_restored_failure_clears_once_the_user_moved_on() {
    use agentnotch_engine::runtime_types::PersistFile;
    let dir = tempfile::tempdir().unwrap();
    let mut first = Harness::new();
    first.store.load_review(None, first.now);
    prompt(&mut first);
    first.hook_with("StopFailure", "waiting_for_input", |b| {
        b.stop_error = Some("rate_limit".into())
    });
    first.now += secs(1);
    let bytes = first
        .tick()
        .jobs
        .into_iter()
        .find_map(|job| match job {
            Job::Persist {
                file: PersistFile::Review,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .unwrap();

    let mut second = second_run(dir.path(), &bytes, t0() + secs(100));
    assert!(matches!(second.state(), SessionState::Failed(_)));
    write_lines(
        Path::new(&second.transcript),
        &[user("try again", t0() + secs(60), json!({}))],
    );
    second.sync();
    assert!(!second.session().unwrap().has_failed_turn());
    assert!(!matches!(second.state(), SessionState::Failed(_)));
}

// ---- completions from a transcript ----

/// A review file whose previous run was last alive at `alive_at`.
fn previous_run(alive_at: SystemTime) -> Vec<u8> {
    let epoch = alive_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    serde_json::to_vec(&json!({"version": 2, "lastAliveAt": epoch, "sessions": {}})).unwrap()
}

#[test]
fn a_session_without_hooks_completes_from_its_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let folder = Harness::folder_of(dir.path());
    h.registry_entries(&folder, false, vec![registry_entry("s1", 77, "busy", t0())]);
    assert_eq!(h.state(), SessionState::Working);
    write_lines(
        &path,
        &[
            user("go", t0() + Duration::from_millis(500), json!({})),
            assistant_text("Finished the refactor.", t0() + secs(2)),
        ],
    );
    h.now = t0() + secs(3);
    h.registry_entries(
        &folder,
        false,
        vec![registry_entry("s1", 77, "idle", t0() + secs(3))],
    );
    h.sync();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    let current = h.session().unwrap();
    assert_eq!(
        current.last_assistant_message.as_deref(),
        Some("Finished the refactor.")
    );
    assert_eq!(current.completed_at, Some(t0() + secs(2)));
}

#[test]
fn an_interrupted_session_without_hooks_is_not_done() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let folder = Harness::folder_of(dir.path());
    h.registry_entries(&folder, false, vec![registry_entry("s1", 77, "busy", t0())]);
    write_lines(
        &path,
        &[
            assistant_text("Starting…", t0() + secs(1)),
            user("[Request interrupted by user]", t0() + secs(2), json!({})),
        ],
    );
    h.now = t0() + secs(3);
    h.registry_entries(
        &folder,
        false,
        vec![registry_entry("s1", 77, "idle", t0() + secs(3))],
    );
    h.sync();
    assert!(h.session().unwrap().completed_at.is_none());
    assert_eq!(h.state(), SessionState::Idle);
}

#[test]
fn a_turn_that_finished_while_the_app_was_down_is_ready_for_review() {
    let dir = tempfile::tempdir().unwrap();
    let alive_at = t0();
    let (mut h, path) = reading(dir.path());
    h.now = alive_at + secs(600);
    h.store.load_review(Some(&previous_run(alive_at)), h.now);
    write_lines(
        &path,
        &[
            user("long job", alive_at - secs(60), json!({})),
            assistant_text("All done while you were away.", alive_at + secs(5)),
        ],
    );
    let folder = Harness::folder_of(dir.path());
    h.registry_entries(
        &folder,
        false,
        vec![registry_entry("s1", 77, "idle", alive_at + secs(6))],
    );
    h.sync();
    assert_eq!(h.state(), SessionState::ReadyForReview);
    assert_eq!(
        h.session().unwrap().last_assistant_message.as_deref(),
        Some("All done while you were away.")
    );
}

#[test]
fn a_reply_written_mid_turn_is_not_taken_for_a_finished_turn() {
    let dir = tempfile::tempdir().unwrap();
    let alive_at = t0();
    let (mut h, path) = reading(dir.path());
    h.now = alive_at + secs(600);
    h.store.load_review(Some(&previous_run(alive_at)), h.now);
    // Claude is mid-turn: it wrote text and is about to call a tool.
    write_lines(
        &path,
        &[
            user("big refactor", alive_at - secs(60), json!({})),
            assistant_text("Now updating the call sites.", alive_at + secs(5)),
        ],
    );
    // Found through its status line: no registry status yet.
    let message = agentnotch_engine::model::StatusLineMessage {
        session_id: s1(),
        cwd: None,
        transcript_path: Some(h.transcript.clone()),
        config_dir_env: None,
        account_id: None,
        received_at: h.now,
        rate_limits: None,
        five_hour: None,
        seven_day: None,
        context_used_percent: None,
        context_window_size: None,
        model_id: None,
        model_display_name: None,
        cost_usd: None,
        session_name: None,
        claude_code_version: None,
        pid: None,
    };
    h.status_line(message);
    h.sync();
    assert_eq!(h.reads_of("s1.jsonl"), 1);
    assert!(h.session().unwrap().completed_at.is_none());
    assert_eq!(h.state(), SessionState::Idle);
}

#[test]
fn no_completion_is_inferred_on_a_first_run_or_from_old_work() {
    let dir = tempfile::tempdir().unwrap();
    let folder = Harness::folder_of(dir.path());
    let lines = [
        user("job", t0() - secs(120), json!({})),
        assistant_text("Done long ago.", t0() - secs(100)),
    ];

    // First run ever: no heartbeat, nothing inferred.
    let (mut first, path) = reading(dir.path());
    write_lines(&path, &lines);
    first.store.load_review(None, first.now);
    first.registry_entries(&folder, false, vec![registry_entry("s1", 77, "idle", t0())]);
    first.sync();
    assert_eq!(first.reads_of("s1.jsonl"), 1);
    assert_eq!(first.state(), SessionState::Idle);

    // A heartbeat newer than the reply: the app saw that turn end.
    let (mut later, _) = reading(dir.path());
    later
        .store
        .load_review(Some(&previous_run(t0() - secs(10))), t0());
    later.registry_entries(&folder, false, vec![registry_entry("s1", 77, "idle", t0())]);
    later.sync();
    assert_eq!(later.reads_of("s1.jsonl"), 1);
    assert_eq!(later.state(), SessionState::Idle);
}

// ---- the chat, as the panel sees it ----

#[test]
fn chat_open_marks_reviewed_and_yields_a_reset_then_patches_of_only_the_changed_items() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(
        &path,
        &[
            user("please look", before(30), json!({})),
            assistant_text("Looking.", before(29)),
            tool_use("c1", "Bash", json!({"command": "ls"}), before(28)),
        ],
    );
    prompt(&mut h);
    h.sync();
    h.hook_with("Stop", "waiting_for_input", |b| {
        b.last_assistant_message = Some("Done".into())
    });
    assert_eq!(h.state(), SessionState::ReadyForReview);

    // Opening marks it reviewed and yields a reset of what is known.
    let opened = h.store.open_chat(&s1(), h.now);
    assert_eq!(h.state(), SessionState::Idle);
    assert!(h.session().unwrap().reviewed_at.is_some());
    assert!(h.store.has_open_chats());
    let reset = h.store.take_chat_updates();
    assert_eq!(reset.len(), 1);
    assert!(reset[0].reset);
    assert_eq!(reset[0].session_id, "s1");
    assert_eq!(reset[0].order.len(), 3);
    assert_eq!(reset[0].items.len(), 3);
    let revision = reset[0].revision;

    // The page arrives: the same items, so only the loading flag moves.
    h.run_jobs(opened.jobs);
    let loaded = h.store.take_chat_updates();
    assert_eq!(loaded.len(), 1);
    assert!(!loaded[0].reset && !loaded[0].loading);
    assert!(loaded[0].items.is_empty() && loaded[0].removed.is_empty());
    assert_eq!(loaded[0].order.len(), 3);
    assert!(h.store.take_chat_updates().is_empty());

    // A transcript change: the new message, and the call that finished.
    append_lines(
        &path,
        &[
            tool_result(
                "c1",
                "file.txt",
                before(27),
                Some(json!({"stdout": "file.txt", "stderr": ""})),
            ),
            assistant_text("Found one file.", before(26)),
        ],
    );
    resync(&mut h);
    let patch = h.store.take_chat_updates();
    assert_eq!(patch.len(), 1);
    let patch = &patch[0];
    assert!(!patch.reset);
    assert!(patch.revision > revision);
    assert_eq!(patch.order.len(), 4);
    assert!(patch.removed.is_empty());
    // Only the two changed items travel: the finished call and the message.
    let ids: Vec<&str> = patch.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"c1"));
    assert!(matches!(
        &patch.items.iter().find(|item| item.id == "c1").unwrap().body,
        ChatBody::Tool { status, result: Some(_), .. } if status == SUCCESS
    ));
    assert!(patch.items.iter().any(|item| matches!(
        &item.body, ChatBody::Assistant { text } if text == "Found one file.")));

    // Nothing changed since: nothing is sent.
    assert!(h.store.take_chat_updates().is_empty());
}

#[test]
fn a_session_that_ends_closes_its_open_chat() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    write_lines(&path, &[assistant_text("hi", before(5))]);
    prompt(&mut h);
    h.sync();
    let opened = h.store.open_chat(&s1(), h.now);
    h.run_jobs(opened.jobs);
    h.store.take_chat_updates();

    h.hook("SessionEnd", "ended");
    let updates = h.store.take_chat_updates();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].ended && updates[0].items.is_empty());
    assert!(!h.store.has_open_chats());
}

#[test]
fn chat_image_answers_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let line = json!({
        "type": "user", "uuid": "img-1", "timestamp": agentnotch_engine::core::time::iso8601(before(20)),
        "message": {"role": "user", "content": [
            {"type": "text", "text": "see"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                          "data": "iVBORw0KGgo="}}]},
    });
    write_lines(&path, &[line]);
    prompt(&mut h);
    h.sync();
    assert_eq!(
        h.store.chat_image(&s1(), "img-1-image-1").as_deref(),
        Some("data:image/png;base64,iVBORw0KGgo=")
    );
    assert_eq!(h.store.chat_image(&s1(), "nope"), None);
    assert_eq!(
        h.store
            .chat_image(&SessionId::from("other"), "img-1-image-1"),
        None
    );

    // The patch names the image by id; its bytes never travel with it.
    let opened = h.store.open_chat(&s1(), h.now);
    h.run_jobs(opened.jobs);
    let update = &h.store.take_chat_updates()[0];
    let image = update
        .items
        .iter()
        .find(|item| item.id == "img-1-image-1")
        .unwrap();
    assert!(
        matches!(&image.body, ChatBody::Image { image_id, bytes: 8, .. } if image_id == "img-1-image-1")
    );
}

// ---- tools the transcript finished ----

#[test]
fn a_tool_the_transcript_finished_is_no_longer_waiting_for_approval() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    prompt(&mut h);
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    assert!(matches!(h.state(), SessionState::NeedsYou(_)));
    h.take_releases();

    // It was approved in the terminal and ran: only the transcript says so.
    write_lines(
        &path,
        &[
            tool_use("toolu_1", "Bash", json!({"command": "ls"}), before(5)),
            tool_result(
                "toolu_1",
                "ok",
                before(4),
                Some(json!({"stdout": "ok", "stderr": ""})),
            ),
        ],
    );
    h.sync();
    assert_eq!(
        h.session().unwrap().chat.tool_status("toolu_1"),
        Some(SUCCESS)
    );
    assert!(!matches!(h.state(), SessionState::NeedsYou(_)));
    assert_eq!(
        h.take_releases(),
        [Release::Request {
            session: s1(),
            tool_use_id: "toolu_1".into()
        }]
    );
    assert!(!h.session().unwrap().tool_tracker.contains("toolu_1"));
}

#[test]
fn the_context_estimate_comes_from_the_transcript_unless_the_status_line_gave_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = reading(dir.path());
    let response = json!({
        "type": "assistant", "uuid": "r1", "timestamp": agentnotch_engine::core::time::iso8601(before(5)),
        "message": {"id": "m1", "model": "claude-opus-4", "content": [{"type": "text", "text": "ok"}],
                    "usage": {"input_tokens": 50_000, "output_tokens": 5,
                              "cache_read_input_tokens": 50_000, "cache_creation_input_tokens": 0}},
    });
    write_lines(&path, &[response]);
    prompt(&mut h);
    h.sync();
    let session = h.session().unwrap();
    assert_eq!(session.context_used_percent, Some(50.0));
    assert_eq!(session.model.as_deref(), Some("claude-opus-4"));

    // An exact figure from the status line is never overwritten.
    let message = agentnotch_engine::model::StatusLineMessage {
        session_id: s1(),
        cwd: None,
        transcript_path: None,
        config_dir_env: None,
        account_id: None,
        received_at: h.now,
        rate_limits: None,
        five_hour: None,
        seven_day: None,
        context_used_percent: Some(12.0),
        context_window_size: Some(200_000),
        model_id: None,
        model_display_name: None,
        cost_usd: None,
        session_name: None,
        claude_code_version: None,
        pid: None,
    };
    h.status_line(message);
    append_lines(&path, &[assistant_text("more", before(3))]);
    resync(&mut h);
    assert_eq!(h.session().unwrap().context_used_percent, Some(12.0));
}
