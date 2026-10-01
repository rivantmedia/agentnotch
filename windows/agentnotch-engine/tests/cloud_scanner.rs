//! The transcript token scanner, state v4 (CL§8; the Mac's
//! `SessionTokenScannerTests`, all 12, plus Windows-style paths, a link out
//! of `projects` through a fake `SecureFiles`, and the version rule).

mod cloud_support;

use agentnotch_engine::cloud::files::StateFile;
use agentnotch_engine::cloud::ledger::SessionOwner;
use agentnotch_engine::cloud::scanner::{
    self, is_safe_transcript_path, CloudTokenTotals, SessionTokenScanner, State,
};
use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
use cloud_support::{CloudFixture as F, Lines as L};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const A: &str = F::SESSION_A;
const B: &str = F::SESSION_B;

/// A temporary `.claude/projects/-Users-me-code-app` folder.
struct Projects {
    root: tempfile::TempDir,
    projects: PathBuf,
    slug: PathBuf,
}

impl Projects {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a temporary folder");
        let projects = root.path().join(".claude").join("projects");
        let slug = projects.join("-Users-me-code-app");
        std::fs::create_dir_all(&slug).expect("made");
        Projects {
            root,
            projects,
            slug,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        let mut path = self.slug.clone();
        for part in name.split('/') {
            path.push(part);
        }
        path
    }

    fn transcript(&self, id: &str) -> PathBuf {
        self.path(&format!("{id}.jsonl"))
    }
}

fn text(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

fn memory_scanner() -> SessionTokenScanner {
    SessionTokenScanner::new(
        StateFile::memory(),
        Arc::new(StdSecureFiles),
        PathStyle::native(),
    )
}

fn saved_scanner(file: &Path) -> SessionTokenScanner {
    let files: Arc<dyn SecureFiles> = Arc::new(StdSecureFiles);
    SessionTokenScanner::new(
        StateFile::new(
            Some(file.to_path_buf()),
            Some(files.clone()),
            Duration::ZERO,
        ),
        files,
        PathStyle::native(),
    )
}

fn scan(
    scanner: &SessionTokenScanner,
    id: &str,
    path: &Path,
) -> Option<scanner::SessionTokenSummary> {
    scanner.scan(id, text(path), &[])
}

fn tokens(input: i64, output: i64, cache_creation: i64, cache_read: i64) -> CloudTokenTotals {
    CloudTokenTotals {
        input,
        output,
        cache_creation,
        cache_read,
    }
}

/// A directory link: a symlink, or on Windows a junction (no privilege).
fn link_dir(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("linked");
    #[cfg(not(unix))]
    {
        // `cmd` reads a `/` in a path as a switch: rebuild both from their
        // components so every separator is a backslash.
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.components().collect::<PathBuf>())
            .arg(target.components().collect::<PathBuf>())
            .output()
            .expect("cmd runs");
        assert!(
            status.status.success(),
            "mklink /J failed: {}{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );
    }
}

#[test]
fn counts_each_response_once_and_skips_synthetic() {
    let dir = Projects::new();
    L::write(
        &[
            L::user_in(
                "please fix the bug",
                A,
                0.0,
                "/Users/me/code/app",
                "claude-vscode",
            ),
            L::ai_title("Fix the parser bug", A),
            // One response written as two content-block lines: counted once,
            // the latest usage.
            L::assistant("msg_1", "req_1", A)
                .usage(10, 1)
                .cache(100, 1000)
                .at(5.0)
                .line(),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 40)
                .cache(100, 1000)
                .at(6.0)
                .tool_use()
                .line(),
            L::assistant("msg_2", "req_2", A)
                .model("claude-haiku-4-5")
                .usage(5, 7)
                .at(20.0)
                .line(),
            L::assistant("msg_3", "req_3", A)
                .usage(1, 2)
                .at(30.0)
                .line(),
            L::assistant("msg_4", "req_4", A)
                .model("<synthetic>")
                .usage(999, 999)
                .at(40.0)
                .line(),
        ],
        &dir.transcript(A),
        false,
    );
    let scanner = memory_scanner();
    let summary = scan(&scanner, A, &dir.transcript(A)).expect("scanned");
    assert_eq!(summary.tokens, tokens(16, 49, 100, 1000));
    assert_eq!(summary.message_count, 3);
    assert_eq!(summary.models, ["claude-opus-4-5", "claude-haiku-4-5"]);
    assert_eq!(summary.first_timestamp, Some(F::base()));
    assert_eq!(summary.last_timestamp, Some(F::at(40.0)));
    assert_eq!(summary.cwd.as_deref(), Some("/Users/me/code/app"));
    assert_eq!(summary.entrypoint.as_deref(), Some("claude-vscode"));
    assert_eq!(summary.title.as_deref(), Some("Fix the parser bug"));
    assert_eq!(scanner.cached_summary(A), Some(summary));
}

#[test]
fn prices_each_response_once_at_its_models_prices() {
    let dir = Projects::new();
    L::write(
        &[
            L::user("go", A, 0.0),
            // Written twice as it streamed: priced once, at its last usage.
            L::assistant("msg_1", "req_1", A)
                .usage(10, 1)
                .cache(100, 1000)
                .at(1.0)
                .line(),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 40)
                .cache(100, 1000)
                .at(2.0)
                .tool_use()
                .line(),
            L::assistant("msg_2", "req_2", A)
                .model("claude-haiku-4-5")
                .usage(5, 7)
                .at(3.0)
                .line(),
            L::assistant("msg_3", "req_3", A)
                .model("<synthetic>")
                .usage(999, 999)
                .at(4.0)
                .line(),
        ],
        &dir.transcript(A),
        false,
    );
    let file = dir.root.path().join("scan-state.json");
    let scanner = saved_scanner(&file);
    let summary = scan(&scanner, A, &dir.transcript(A)).expect("scanned");
    // Per million: Opus 4.5 10×5 + 40×25 + 100×6.25 + 1000×0.5 = 2175,
    // Haiku 4.5 5×1 + 7×5 = 40.
    assert_eq!(summary.cost, Some(2_215_000));
    let part = summary.part("");
    assert_eq!(part.cost, Some(2_215_000));
    assert_eq!(part.estimated_cost_usd(), Some(0.002215));
    // Kept with the watermarks.
    scanner.save_now();
    assert_eq!(
        saved_scanner(&file).cached_summary(A).map(|s| s.cost),
        Some(Some(2_215_000))
    );

    // A model with no known price: the cost is unknown, not a guess.
    L::write(
        &[L::assistant("msg_4", "req_4", A)
            .model("claude-opus-9")
            .usage(1, 1)
            .at(5.0)
            .line()],
        &dir.transcript(A),
        true,
    );
    let unknown = scan(&scanner, A, &dir.transcript(A)).expect("scanned");
    assert_eq!(unknown.message_count, 3);
    assert_eq!(unknown.cost, None);
    assert_eq!(unknown.part("").estimated_cost_usd(), None);
}

#[test]
fn subagents_count_once_wherever_their_lines_are() {
    let dir = Projects::new();
    L::write(
        &[
            L::user("go", A, 0.0),
            L::assistant("msg_main", "req_main", A)
                .usage(100, 10)
                .at(1.0)
                .line(),
            // An older transcript repeats a subagent's response in the parent.
            L::assistant("msg_sub", "req_sub", A)
                .usage(50, 5)
                .at(2.0)
                .sidechain()
                .line(),
        ],
        &dir.transcript(A),
        false,
    );
    // Current layout: <session>/subagents/…, workflows one level deeper.
    L::write(
        &[
            L::assistant("msg_sub", "req_sub", A)
                .usage(50, 5)
                .at(2.0)
                .sidechain()
                .line(),
            L::assistant("msg_sub2", "req_sub2", A)
                .model("claude-haiku-4-5")
                .usage(20, 2)
                .at(3.0)
                .sidechain()
                .line(),
        ],
        &dir.path(&format!("{A}/subagents/agent-one.jsonl")),
        false,
    );
    L::write(
        &[L::assistant("msg_wf", "req_wf", A)
            .usage(7, 1)
            .at(4.0)
            .sidechain()
            .line()],
        &dir.path(&format!("{A}/subagents/workflows/wf1/agent-two.jsonl")),
        false,
    );
    // Legacy flat agent files: this session's, and another session's.
    L::write(
        &[L::assistant("msg_flat", "req_flat", A)
            .usage(3, 3)
            .at(5.0)
            .sidechain()
            .line()],
        &dir.path("agent-flat.jsonl"),
        false,
    );
    L::write(
        &[L::assistant("msg_other", "req_other", B)
            .usage(1000, 1000)
            .at(6.0)
            .sidechain()
            .line()],
        &dir.path("agent-other.jsonl"),
        false,
    );

    let scanner = memory_scanner();
    let summary = scan(&scanner, A, &dir.transcript(A)).expect("scanned");
    assert_eq!(summary.tokens.input, 100 + 50 + 20 + 7 + 3);
    assert_eq!(summary.tokens.output, 10 + 5 + 2 + 1 + 3);
    assert_eq!(summary.message_count, 5);
    assert_eq!(
        summary.models.first().map(String::as_str),
        Some("claude-opus-4-5")
    );
    assert!(summary.models.iter().any(|m| m == "claude-haiku-4-5"));
    // Scanning again changes nothing.
    assert_eq!(scan(&scanner, A, &dir.transcript(A)), Some(summary));
}

#[test]
fn a_shared_history_reached_two_ways_counts_once() {
    let root = tempfile::tempdir().expect("a temporary folder");
    let shared = root.path().join(".claude-shared").join("projects");
    let slug = shared.join("-Users-me-code-app");
    std::fs::create_dir_all(&slug).expect("made");
    for name in [".claude", ".claude-windows/0123456789ab"] {
        let folder = root.path().join(name);
        std::fs::create_dir_all(&folder).expect("made");
        link_dir(&folder.join("projects"), &shared);
    }
    L::write(
        &[
            L::user("go", A, 0.0),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 10)
                .at(1.0)
                .line(),
        ],
        &slug.join(format!("{A}.jsonl")),
        false,
    );

    let scanner = memory_scanner();
    let via_default = root
        .path()
        .join(".claude/projects/-Users-me-code-app")
        .join(format!("{A}.jsonl"));
    let via_window = root
        .path()
        .join(".claude-windows/0123456789ab/projects/-Users-me-code-app")
        .join(format!("{A}.jsonl"));
    let first = scan(&scanner, A, &via_default).expect("scanned");
    let second = scan(&scanner, A, &via_window).expect("scanned");
    assert_eq!(first.message_count, 1);
    assert_eq!(second, first);
    assert_eq!(first.tokens.input, 10);
}

#[test]
fn a_resumed_sessions_copied_lines_stay_with_the_original() {
    let dir = Projects::new();
    L::write(
        &[
            L::user("start", A, 0.0),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 10)
                .at(1.0)
                .line(),
            L::assistant("msg_2", "req_2", A)
                .usage(20, 20)
                .at(2.0)
                .line(),
        ],
        &dir.transcript(A),
        false,
    );
    // B resumed A: A's lines copied verbatim (naming A), one copied as B,
    // then B's own work.
    L::write(
        &[
            L::user("start", A, 0.0),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 10)
                .at(1.0)
                .line(),
            L::assistant("msg_2", "req_2", B)
                .usage(20, 20)
                .at(2.0)
                .line(),
            L::user("continue", B, 3600.0),
            L::assistant("msg_3", "req_3", B)
                .usage(5, 5)
                .at(3601.0)
                .line(),
        ],
        &dir.transcript(B),
        false,
    );

    let scanner = memory_scanner();
    // The newer one first: the copies still don't count for it.
    let resumed = scan(&scanner, B, &dir.transcript(B)).expect("scanned");
    let original = scan(&scanner, A, &dir.transcript(A)).expect("scanned");
    assert_eq!(original.tokens.input + resumed.tokens.input, 35);
    assert_eq!(original.message_count + resumed.message_count, 3);
    assert!(original.message_count >= 1 && resumed.message_count >= 1);
    // Dated by its own lines, not the copies.
    assert!(
        resumed.first_timestamp == Some(F::at(2.0))
            || resumed.first_timestamp == Some(F::at(3600.0))
    );
    assert!(original.tokens.input >= 10);
}

/// Regression (review finding 24): lines `/branch` copied into a fork name
/// the fork but carry `forkedFrom`: they stay with the original whichever is
/// scanned first, and the fork is dated by its own lines.
#[test]
fn a_forks_copied_lines_stay_with_the_original_whatever_the_order() {
    let dir = Projects::new();
    let original = [
        L::user("start", B, 0.0),
        L::assistant("msg_1", "req_1", B)
            .usage(10, 10)
            .at(1.0)
            .line(),
        L::assistant("msg_2", "req_2", B)
            .usage(20, 20)
            .at(2.0)
            .line(),
    ];
    L::write(&original, &dir.transcript(B), false);
    // The fork is scanned first, as when its id sorts first in a listing.
    let mut fork: Vec<String> = original.iter().map(|l| L::forked(l, A, B)).collect();
    fork.push(L::user("try another way", A, 600.0));
    fork.push(
        L::assistant("msg_3", "req_3", A)
            .usage(5, 5)
            .at(601.0)
            .line(),
    );
    L::write(&fork, &dir.transcript(A), false);

    let scanner = memory_scanner();
    let fork = scan(&scanner, A, &dir.transcript(A)).expect("scanned");
    let parent = scan(&scanner, B, &dir.transcript(B)).expect("scanned");
    assert_eq!((parent.message_count, parent.tokens.input), (2, 30));
    assert_eq!((fork.message_count, fork.tokens.input), (1, 5));
    assert_eq!(fork.first_timestamp, Some(F::at(600.0)));
    assert_eq!(
        scanner.first_timestamp(text(&dir.transcript(A)), A),
        Some(F::at(600.0))
    );
    assert_eq!(
        scanner::read_first_timestamp(&dir.transcript(B), B, scanner::FIRST_TIMESTAMP_LIMIT),
        Some(F::base())
    );
    // Read without scanning, the same answer.
    let fresh = memory_scanner();
    assert_eq!(
        fresh.first_timestamp(text(&dir.transcript(A)), A),
        Some(F::at(600.0))
    );
}

/// Regression (review finding 7): a session two accounts ran is counted per
/// account, by who ran it when each line was written.
#[test]
fn a_session_two_accounts_ran_is_counted_per_account() {
    let dir = Projects::new();
    let (first, second) = ("key-first", "key-second");
    L::write(
        &[
            L::user("start", A, 0.0),
            L::assistant("m1", "r1", A).usage(10, 1).at(10.0).line(),
            L::assistant("m2", "r2", A).usage(20, 2).at(20.0).line(),
        ],
        &dir.transcript(A),
        false,
    );
    let scanner = memory_scanner();
    let alone = vec![SessionOwner::new(None, first)];
    let before = scanner
        .scan(A, text(&dir.transcript(A)), &alone)
        .expect("scanned");
    assert_eq!(before.part(first).message_count, 2);
    assert_eq!(before.part(second).message_count, 0);

    // Resumed under the second account at 100 s: what came before stays the
    // first's, without reading the file again.
    let mut handed_over = alone.clone();
    handed_over.push(SessionOwner::new(Some(F::at(100.0)), second));
    L::write(
        &[
            L::user("continue", A, 120.0),
            L::assistant("m3", "r3", A)
                .model("claude-haiku-4-5")
                .usage(300, 3)
                .at(130.0)
                .line(),
        ],
        &dir.transcript(A),
        true,
    );
    // A subagent of the second account's turn.
    L::write(
        &[L::assistant("m4", "r4", A)
            .usage(4000, 4)
            .at(140.0)
            .sidechain()
            .line()],
        &dir.path(&format!("{A}/subagents/agent-x.jsonl")),
        false,
    );
    let after = scanner
        .scan(A, text(&dir.transcript(A)), &handed_over)
        .expect("scanned");
    let (mine, theirs) = (after.part(first), after.part(second));
    assert_eq!((mine.message_count, mine.tokens.input), (2, 30));
    assert_eq!(mine.first_timestamp, Some(F::base()));
    assert_eq!(mine.last_timestamp, Some(F::at(20.0)));
    assert_eq!((theirs.message_count, theirs.tokens.input), (2, 4300));
    assert_eq!(theirs.first_timestamp, Some(F::at(120.0)));
    assert!(theirs.models.iter().any(|m| m == "claude-haiku-4-5"));
    assert!(!mine.models.iter().any(|m| m == "claude-haiku-4-5"));
    assert_eq!((after.message_count, after.tokens.input), (4, 4330));
    // Each part priced by its own responses (per million: Opus 4.5 75 + 150;
    // Haiku 4.5 315, Opus 4.5 20100).
    assert_eq!(mine.cost, Some(225_000));
    assert_eq!(theirs.cost, Some(20_415_000));
    assert_eq!(after.cost, Some(20_640_000));
    assert_eq!(scanner.cached_summary(A), Some(after.clone()));

    // Owners that disagree with what was counted: counted again.
    let other_way = vec![SessionOwner::new(None, second)];
    let recounted = scanner
        .scan(A, text(&dir.transcript(A)), &other_way)
        .expect("scanned");
    assert_eq!(recounted.part(second).message_count, 4);
    assert_eq!(recounted.part(first).message_count, 0);
    assert_eq!(recounted.tokens, after.tokens);
    assert_eq!(recounted.part(second).cost, Some(20_640_000));
    assert_eq!(recounted.part(first).cost, None);
}

#[test]
fn reads_only_what_was_added_and_recounts_a_rewrite() {
    let dir = Projects::new();
    let path = dir.transcript(A);
    L::write(
        &[
            L::user("go", A, 0.0),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 1)
                .at(1.0)
                .line(),
        ],
        &path,
        false,
    );
    let file = dir.root.path().join("scan-state.json");
    let scanner = saved_scanner(&file);
    assert_eq!(scan(&scanner, A, &path).map(|s| s.tokens.input), Some(10));

    // Appended, with a half-written last line: only complete lines count.
    L::write(
        &[L::assistant("msg_2", "req_2", A)
            .usage(20, 2)
            .at(2.0)
            .line()],
        &path,
        true,
    );
    let mut handle = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("opened");
    handle
        .write_all(br#"{"type":"assistant","message":{"id":"msg_3""#)
        .expect("written");
    drop(handle);
    assert_eq!(scan(&scanner, A, &path).map(|s| s.tokens.input), Some(30));

    // Saved, reloaded, finished: the new scanner continues from the watermark.
    scanner.save_now();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file)
            .expect("saved")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let reloaded = saved_scanner(&file);
    assert_eq!(reloaded.cached_summary(A).map(|s| s.tokens.input), Some(30));
    let rest = format!(
        r#","usage":{{"input_tokens":5,"output_tokens":5}},"model":"claude-opus-4-5"}},"requestId":"req_3","sessionId":"{A}","timestamp":"{}"}}"#,
        F::stamp(3.0)
    ) + "\n";
    let mut more = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("opened");
    more.write_all(rest.as_bytes()).expect("written");
    drop(more);
    let grown = scan(&reloaded, A, &path).expect("scanned");
    assert_eq!((grown.tokens.input, grown.message_count), (35, 3));
    // Priced across the reload too (per million: 75 + 150 + 150).
    assert_eq!(grown.cost, Some(375_000));

    // Rewritten shorter: counted again from the start, nothing doubled.
    L::write(
        &[
            L::user("go", A, 0.0),
            L::assistant("msg_1", "req_1", A)
                .usage(10, 1)
                .at(1.0)
                .line(),
        ],
        &path,
        false,
    );
    let rewritten = scan(&reloaded, A, &path).expect("scanned");
    assert_eq!((rewritten.tokens.input, rewritten.message_count), (10, 1));
}

#[test]
fn deleted_transcripts_are_forgotten() {
    let dir = Projects::new();
    L::write(
        &[L::assistant("m1", "r1", A).usage(1, 1).at(0.0).line()],
        &dir.transcript(A),
        false,
    );
    L::write(
        &[L::assistant("m2", "r2", B).usage(1, 1).at(0.0).line()],
        &dir.transcript(B),
        false,
    );
    let scanner = memory_scanner();
    assert!(scan(&scanner, A, &dir.transcript(A)).is_some());
    assert!(scan(&scanner, B, &dir.transcript(B)).is_some());
    std::fs::remove_file(dir.transcript(A)).expect("removed");
    assert_eq!(scanner.prune_missing_files(), 1);
    assert_eq!(scanner.cached_summary(A), None);
    assert_eq!(scanner.cached_summary(B).map(|s| s.message_count), Some(1));
}

#[test]
fn never_opens_anything_outside_projects() {
    let posix = |p: &str| is_safe_transcript_path(p, PathStyle::Posix);
    assert!(posix(&format!("/Users/me/.claude/projects/-x/{A}.jsonl")));
    assert!(posix(&format!(
        "/Users/me/.claude/projects/-x/{A}/subagents/agent-1.jsonl"
    )));
    assert!(!posix("/Users/me/.claude/sessions/123.json"));
    assert!(!posix("/Users/me/.claude/projects/sessions/x.jsonl"));
    assert!(!posix("/Users/me/.claude/projects/-x/../../settings.jsonl"));
    assert!(!posix("/Users/me/.claude/history.jsonl"));
    let scanner = memory_scanner();
    assert!(scanner
        .scan(A, &format!("/Users/me/.claude/sessions/{A}.jsonl"), &[])
        .is_none());
    assert!(scanner
        .scan(
            "not-a-session",
            "/Users/me/.claude/projects/-x/y.jsonl",
            &[]
        )
        .is_none());
}

#[test]
fn windows_style_paths_are_judged_by_their_own_rules() {
    let windows = |p: &str| is_safe_transcript_path(p, PathStyle::Windows);
    let posix = |p: &str| is_safe_transcript_path(p, PathStyle::Posix);
    assert!(windows(&format!(
        r"C:\Users\Me\.claude\projects\-C--code-app\{A}.jsonl"
    )));
    assert!(windows(&format!(
        r"C:\Users\Me\.claude-windows\0123456789ab\projects\-C--code-app\{A}\subagents\agent-1.jsonl"
    )));
    // Forward slashes work on Windows too, and folder names have no case.
    assert!(windows(&format!(
        "C:/Users/Me/.claude/PROJECTS/-x/{A}.jsonl"
    )));
    // Nothing from `sessions`, no `..`, no other files.
    assert!(!windows(&format!(
        r"C:\Users\Me\.claude\sessions\{A}.jsonl"
    )));
    assert!(!windows(&format!(
        r"C:\Users\Me\.claude\projects\Sessions\{A}.jsonl"
    )));
    assert!(!windows(&format!(
        r"C:\Users\Me\.claude\projects\-x\..\..\{A}.jsonl"
    )));
    assert!(!windows(r"C:\Users\Me\.claude\projects\-x\settings.json"));
    assert!(!windows(r"C:\Users\Me\.claude\history.jsonl"));
    // A projects folder with the transcript directly in it is not a project's.
    assert!(!windows(&format!(r"C:\projects\{A}.jsonl")));
    // A POSIX path has no backslash separators.
    assert!(!posix(&format!(
        r"C:\Users\Me\.claude\projects\-x\{A}.jsonl"
    )));
    assert!(!posix(&format!("/Users/me/.claude/PROJECTS/-x/{A}.jsonl")));
}

/// Real files, with a few paths canonicalised somewhere else: a link on any
/// host.
struct Redirect {
    inner: StdSecureFiles,
    to: BTreeMap<String, PathBuf>,
}

impl SecureFiles for Redirect {
    fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
        self.inner.ensure_private_dir(dir)
    }
    fn write_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: WriteMode,
        expect: Expect,
    ) -> io::Result<WriteResult> {
        self.inner.write_atomic(path, bytes, mode, expect)
    }
    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        self.inner.create_exclusive(path, bytes)
    }
    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        self.inner.identity(path)
    }
    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        Ok(self.to.contains_key(&path.to_string_lossy().into_owned()))
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        match self.to.get(&path.to_string_lossy().into_owned()) {
            Some(real) => Ok(real.clone()),
            None => self.inner.canonical(path),
        }
    }
    fn is_private(&self, path: &Path) -> io::Result<bool> {
        self.inner.is_private(path)
    }
}

/// Regression (fix check): a `<id>.jsonl` link in a projects folder that
/// points out of it (at a key file in `sessions/`, say) is never opened by
/// the backfill's first-line read, as `scan` never opens it; a link into
/// another projects folder (a shared history) still is.
#[test]
fn first_timestamp_never_opens_a_link_out_of_projects() {
    let dir = Projects::new();
    let sessions = dir.root.path().join(".claude").join("sessions");
    std::fs::create_dir_all(&sessions).expect("made");
    let key = sessions.join("123.key");
    // Looks like a transcript line, so a read would find a timestamp.
    L::write(&[L::user("x", A, 0.0)], &key, false);
    let shared = dir
        .root
        .path()
        .join(".claude-shared/projects/-Users-me-code-app")
        .join(format!("{B}.jsonl"));
    L::write(&[L::user("x", B, 60.0)], &shared, false);

    let files: Arc<dyn SecureFiles> = Arc::new(Redirect {
        inner: StdSecureFiles,
        to: BTreeMap::from([
            (
                dir.transcript(A).to_string_lossy().into_owned(),
                key.clone(),
            ),
            (
                dir.transcript(B).to_string_lossy().into_owned(),
                shared.clone(),
            ),
        ]),
    });
    // Both names exist as files too, so only the redirect decides.
    L::write(&[L::user("x", A, 0.0)], &dir.transcript(A), false);
    L::write(&[L::user("x", B, 60.0)], &dir.transcript(B), false);
    let scanner = SessionTokenScanner::new(StateFile::memory(), files, PathStyle::native());
    assert!(is_safe_transcript_path(
        text(&dir.transcript(A)),
        PathStyle::native()
    ));
    assert_eq!(scanner.first_timestamp(text(&dir.transcript(A)), A), None);
    assert!(scan(&scanner, A, &dir.transcript(A)).is_none());
    assert_eq!(
        scanner.first_timestamp(text(&dir.transcript(B)), B),
        Some(F::at(60.0))
    );
}

#[cfg(unix)]
#[test]
fn first_timestamp_never_opens_a_symlink_out_of_projects() {
    let dir = Projects::new();
    let sessions = dir.root.path().join(".claude").join("sessions");
    std::fs::create_dir_all(&sessions).expect("made");
    let key = sessions.join("123.key");
    L::write(&[L::user("x", A, 0.0)], &key, false);
    std::os::unix::fs::symlink(&key, dir.transcript(A)).expect("linked");
    let scanner = memory_scanner();
    assert_eq!(scanner.first_timestamp(text(&dir.transcript(A)), A), None);
    assert!(scan(&scanner, A, &dir.transcript(A)).is_none());

    let real = dir
        .root
        .path()
        .join(".claude-shared/projects/-Users-me-code-app")
        .join(format!("{B}.jsonl"));
    L::write(&[L::user("x", B, 60.0)], &real, false);
    std::os::unix::fs::symlink(&real, dir.transcript(B)).expect("linked");
    assert_eq!(
        scanner.first_timestamp(text(&dir.transcript(B)), B),
        Some(F::at(60.0))
    );
}

#[test]
fn lists_session_transcripts_of_a_projects_folder() {
    let dir = Projects::new();
    L::write(&[L::user("x", A, 0.0)], &dir.transcript(A), false);
    L::write(&[L::user("x", B, 0.0)], &dir.transcript(B), false);
    L::write(&[L::user("x", A, 0.0)], &dir.path("agent-x.jsonl"), false);
    L::write(&[L::user("x", A, 0.0)], &dir.path("notes.jsonl"), false);
    let found = scanner::session_files(&dir.projects, PathStyle::native());
    let mut ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    ids.sort();
    let mut expected = vec![A, B];
    expected.sort();
    assert_eq!(ids, expected);
    assert!(scanner::session_files(dir.root.path(), PathStyle::native()).is_empty());
}

#[test]
fn a_state_of_version_three_starts_fresh() {
    let dir = Projects::new();
    L::write(
        &[L::assistant("m1", "r1", A).usage(1, 1).at(0.0).line()],
        &dir.transcript(A),
        false,
    );
    let file = dir.root.path().join("scan-state.json");
    let scanner = saved_scanner(&file);
    assert!(scan(&scanner, A, &dir.transcript(A)).is_some());
    scanner.save_now();
    let saved = std::fs::read_to_string(&file).expect("saved");
    assert!(saved.contains(r#""version":4"#), "{saved}");

    let old = saved.replace(r#""version":4"#, r#""version":3"#);
    std::fs::write(&file, old).expect("written");
    let fresh = saved_scanner(&file);
    assert_eq!(fresh.cached_summary(A), None);
    let state: State = fresh.state();
    assert_eq!(state.version, State::CURRENT_VERSION);
    assert!(state.files.is_empty());
    // And it counts again from the start.
    assert_eq!(
        scan(&fresh, A, &dir.transcript(A)).map(|s| s.message_count),
        Some(1)
    );
}

#[test]
fn a_claim_is_sixteen_hex_digits_and_stable() {
    let key = scanner::claim_key("msg_1|req_1");
    assert_eq!(key.len(), 16);
    assert_eq!(key, scanner::claim_key("msg_1|req_1"));
    assert_ne!(key, scanner::claim_key("msg_1|req_2"));
}
