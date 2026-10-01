//! The interrupt watcher and the store's Interrupt input:
//! A1_InterruptWatcherTests (`interruptLinesAreRecognizedAsBytes`,
//! `watchesOnlyWhileTheMainTurnRuns`, `reportsAnInterruptAppendedToThe-
//! Transcript`), SessionCoreRegressionTests' `interruptWatcherWaitsForThe-
//! TranscriptToAppear` and A1_SessionStoreRegressionTests'
//! `anInterruptSeenAfterTheNextPromptIsIgnored`.
//!
//! The Mac watcher sleeps on a dispatch source and a real clock; this one is
//! polled by the runtime with the time passed in, so the tests move the clock
//! themselves.

mod sessions_support;

use agentnotch_engine::model::{Phase, SessionId, SessionState};
use agentnotch_engine::runtime_types::{Release, SessionInput};
use agentnotch_engine::sessions::background::WaitTiming;
use agentnotch_engine::sessions::completion::CompletionTiming;
use agentnotch_engine::sessions::interrupt::{
    is_interrupt_line, InterruptWatch, InterruptWatcher, MAX_OPEN_ATTEMPTS, OPEN_RETRY_INTERVAL,
    POLL_INTERVAL,
};
use serde_json::{json, Value};
use sessions_support::{
    append_bytes, append_lines, assistant_text, t0, user, write_lines, Harness, TRANSCRIPT,
};
use std::path::Path;
use std::time::Duration;

fn bytes(line: &Value) -> Vec<u8> {
    serde_json::to_vec(line).unwrap()
}

fn s(id: &str) -> SessionId {
    SessionId::from(id)
}

fn same(a: &Path, b: &Path) -> bool {
    a == b
}

fn watch(session: &str, path: &Path) -> InterruptWatch {
    InterruptWatch {
        session: s(session),
        path: path.to_path_buf(),
    }
}

fn interrupted(session: &str, inputs: &[SessionInput]) -> bool {
    inputs.iter().any(|input| {
        matches!(input, SessionInput::Interrupt { session: id, .. } if id.as_str() == session)
    })
}

#[test]
fn interrupt_lines_are_recognized_as_bytes() {
    let user_interrupt = bytes(&json!({
        "type": "user",
        "message": {"role": "user", "content": [
            {"type": "text", "text": "[Request interrupted by user for tool use]"}]},
    }));
    let rejected_tool = bytes(&json!({
        "type": "user",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "is_error": true,
             "content": "The user doesn't want to proceed with this tool use."}]},
    }));
    let interrupted_bash = bytes(&json!({
        "type": "user",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t2", "content": "partial output"}]},
        "toolUseResult": {"stdout": "partial output", "interrupted": true},
    }));
    assert!(is_interrupt_line(&user_interrupt));
    assert!(is_interrupt_line(&rejected_tool));
    assert!(is_interrupt_line(&interrupted_bash));

    // Claude quoting the phrase, a failed (not interrupted) tool, and a plain
    // result are not interrupts.
    let quoted = bytes(&json!({
        "type": "assistant",
        "message": {"role": "assistant", "content": [
            {"type": "text", "text": "It printed [Request interrupted by user] earlier."}]},
    }));
    let failed = bytes(&json!({
        "type": "user",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t3", "is_error": true, "content": "exit code 1"}]},
    }));
    let plain = bytes(&json!({
        "type": "user",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t4", "content": "ok"}]},
    }));
    // The interrupted flag is trusted only on a tool result line.
    let flag_elsewhere = bytes(&json!({
        "type": "assistant",
        "message": {"role": "assistant", "content": [{"type": "text", "text": "\"interrupted\":true"}]},
        "note": "\"interrupted\":true",
    }));
    assert!(!is_interrupt_line(&quoted));
    assert!(!is_interrupt_line(&failed));
    assert!(!is_interrupt_line(&plain));
    assert!(!is_interrupt_line(&flag_elsewhere));
    assert!(!is_interrupt_line(b""));
}

#[test]
fn watches_only_while_the_main_turn_runs() {
    let mut h = Harness::new();
    let watching = |h: &Harness| h.store.interrupt_watches();

    // A background agent's event doesn't start it.
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.agent_id = Some("bg-1".into());
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("t1".into());
    });
    assert!(watching(&h).is_empty());

    h.hook("UserPromptSubmit", "processing");
    assert_eq!(
        watching(&h),
        [InterruptWatch {
            session: s("s1"),
            path: TRANSCRIPT.into()
        }]
    );
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("t2".into());
    });
    assert_eq!(watching(&h).len(), 1);

    // The turn's end stops it (no handle held per idle session).
    h.hook("Stop", "waiting_for_input");
    assert!(watching(&h).is_empty());

    h.hook("UserPromptSubmit", "processing");
    h.hook("StopFailure", "waiting_for_input");
    assert!(watching(&h).is_empty());

    h.hook("UserPromptSubmit", "processing");
    assert_eq!(watching(&h).len(), 1);
    h.hook("SessionEnd", "ended");
    assert!(watching(&h).is_empty());
}

#[test]
fn a_session_with_no_transcript_path_is_not_watched() {
    let mut h = Harness::new();
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.transcript_path = None
    });
    // hook_with fills the harness's path first; a builder that clears it
    // again is what the hook script sends when it has none.
    assert!(h.store.interrupt_watches().is_empty());
}

#[test]
fn reports_an_interrupt_appended_to_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("iw-2.jsonl");
    write_lines(&path, &[user("start the build", t0(), json!({}))]);
    let mut now = t0();
    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&[watch("iw-2", &path)], now, same);
    assert!(watcher.is_watching(&s("iw-2")));
    assert!(watcher.poll(now).is_empty());

    // What was there before the watcher started isn't news; an ordinary line
    // isn't an interrupt.
    now += POLL_INTERVAL;
    append_lines(&path, &[assistant_text("Building…", t0())]);
    assert!(watcher.poll(now).is_empty());

    now += POLL_INTERVAL;
    append_lines(
        &path,
        &[user("[Request interrupted by user]", t0(), json!({}))],
    );
    let found = watcher.poll(now);
    assert_eq!(found.len(), 1);
    assert!(matches!(&found[0],
        SessionInput::Interrupt { session, at } if session.as_str() == "iw-2" && *at == now));
    // Reported once.
    now += POLL_INTERVAL;
    assert!(watcher.poll(now).is_empty());
}

#[test]
fn a_half_written_line_waits_for_its_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("half.jsonl");
    write_lines(&path, &[user("go", t0(), json!({}))]);
    let mut now = t0();
    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&[watch("half", &path)], now, same);
    watcher.poll(now);

    let line = bytes(&user("[Request interrupted by user]", t0(), json!({})));
    append_bytes(&path, &line[..30]);
    now += POLL_INTERVAL;
    assert!(watcher.poll(now).is_empty());
    let mut rest = line[30..].to_vec();
    rest.push(b'\n');
    append_bytes(&path, &rest);
    now += POLL_INTERVAL;
    assert!(interrupted("half", &watcher.poll(now)));
}

#[test]
fn interrupt_watcher_waits_for_the_transcript_to_appear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("late.jsonl");
    let mut now = t0();
    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&[watch("late", &path)], now, same);
    assert!(watcher.poll(now).is_empty());

    // The transcript shows up after the watcher started (first prompt).
    now += Duration::from_millis(300);
    std::fs::write(&path, b"{\"type\":\"user\"}\n").unwrap();
    assert!(watcher.poll(now).is_empty());
    // It looks for the file once a second: found now, read from its end.
    now += OPEN_RETRY_INTERVAL;
    assert!(watcher.poll(now).is_empty());

    append_bytes(
        &path,
        b"{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Request interrupted by user]\"}]}}\n",
    );
    now += POLL_INTERVAL;
    let found = watcher.poll(now);
    assert_eq!(found.len(), 1);
    assert!(interrupted("late", &found));
}

#[test]
fn a_transcript_that_never_appears_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never.jsonl");
    let mut now = t0();
    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&[watch("never", &path)], now, same);
    for _ in 0..MAX_OPEN_ATTEMPTS {
        watcher.poll(now);
        now += OPEN_RETRY_INTERVAL;
    }
    std::fs::write(&path, b"{\"type\":\"user\"}\n").unwrap();
    now += OPEN_RETRY_INTERVAL;
    assert!(watcher.poll(now).is_empty());
    append_bytes(&path, b"[Request interrupted by user]\n");
    now += OPEN_RETRY_INTERVAL;
    assert!(watcher.poll(now).is_empty());
    // Still listed (the store says when its turn is over), just silent.
    assert!(watcher.is_watching(&s("never")));
}

#[test]
fn watches_follow_the_stores_list() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
    write_lines(&a, &[user("a", t0(), json!({}))]);
    write_lines(&b, &[user("b", t0(), json!({}))]);
    let now = t0();
    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&[watch("a", &a), watch("b", &b)], now, same);
    assert_eq!(watcher.len(), 2);
    watcher.poll(now);

    // The same file named again keeps its place (nothing is read twice)...
    watcher.set_watches(&[watch("a", &a)], now, same);
    assert_eq!(watcher.len(), 1);
    append_lines(
        &a,
        &[user("[Request interrupted by user]", t0(), json!({}))],
    );
    // ...and a file named another way that is the same file keeps it too.
    watcher.set_watches(
        &[watch("a", &dir.path().join(".").join("a.jsonl"))],
        now,
        |x, y| x.file_name() == y.file_name(),
    );
    assert!(interrupted("a", &watcher.poll(now + POLL_INTERVAL)));

    // A different file starts over; a stopped watch reports nothing.
    watcher.set_watches(&[watch("a", &b)], now, same);
    assert!(watcher.poll(now + POLL_INTERVAL).is_empty());
    watcher.set_watches(&[], now, same);
    assert!(watcher.is_empty());
    assert!(watcher.poll(now + POLL_INTERVAL).is_empty());
}

#[test]
fn a_replaced_shorter_file_is_not_an_interrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    write_lines(
        &path,
        &[
            assistant_text("a long first line of text", t0()),
            assistant_text("another", t0()),
        ],
    );
    let mut now = t0();
    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&[watch("r", &path)], now, same);
    watcher.poll(now);
    write_lines(&path, &[assistant_text("x", t0())]);
    now += POLL_INTERVAL;
    assert!(watcher.poll(now).is_empty());
    // From its new end on, an interrupt is seen.
    append_lines(
        &path,
        &[user("[Request interrupted by user]", t0(), json!({}))],
    );
    now += POLL_INTERVAL;
    assert!(interrupted("r", &watcher.poll(now)));
}

// ---- the store's Interrupt input ----

#[test]
fn an_interrupt_seen_after_the_next_prompt_is_ignored() {
    let mut h = Harness::new();
    let t = t0();
    h.now = t;
    h.hook("UserPromptSubmit", "processing");
    h.now = t + Duration::from_secs(2);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    // Seen late, about the first turn: the second started since.
    h.apply(SessionInput::Interrupt {
        session: s("s1"),
        at: t + Duration::from_secs(1),
    });
    assert_eq!(h.state(), SessionState::Working);
    assert_eq!(h.store.interrupt_watches().len(), 1);
    h.apply(SessionInput::Interrupt {
        session: s("s1"),
        at: t + Duration::from_secs(3),
    });
    assert_eq!(h.session().unwrap().phase, Phase::Idle);
    // The turn is over: nothing is watched any more.
    assert!(h.store.interrupt_watches().is_empty());
}

#[test]
fn a_session_found_by_the_watcher_is_stopped_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::reading(
        dir.path(),
        CompletionTiming::IMMEDIATE,
        WaitTiming::STANDARD,
    );
    let path = std::path::PathBuf::from(&h.transcript);
    write_lines(&path, &[user("run the tests", t0(), json!({}))]);
    h.hook_with("UserPromptSubmit", "processing", |b| {
        b.source = Some("user".into())
    });
    h.hook_with("PreToolUse", "running_tool", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.hook_with("PermissionRequest", "waiting_for_approval", |b| {
        b.tool = Some("Bash".into());
        b.tool_use_id = Some("toolu_1".into());
    });
    h.take_releases();

    let mut watcher = InterruptWatcher::new();
    watcher.set_watches(&h.store.interrupt_watches(), h.now, same);
    watcher.poll(h.now);
    append_lines(
        &path,
        &[user(
            "[Request interrupted by user for tool use]",
            t0(),
            json!({}),
        )],
    );
    h.now += POLL_INTERVAL;
    let found = watcher.poll(h.now);
    assert_eq!(found.len(), 1);
    for input in found {
        h.apply(input);
    }

    // The call that was waiting on the user was interrupted, its request is
    // gone and the main agent's held pipes are closed.
    let session = h.session().unwrap();
    assert_eq!(session.phase, Phase::Idle);
    assert_eq!(session.chat.tool_status("toolu_1"), Some("interrupted"));
    assert!(session.pending_requests().is_empty());
    assert!(h.take_releases().contains(&Release::MainAgent(s("s1"))));
    watcher.set_watches(&h.store.interrupt_watches(), h.now, same);
    assert!(watcher.is_empty());
}
