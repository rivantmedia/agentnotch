//! The hook exe's copy and the settings.json backups, on temporary folders:
//! the backup half of the Mac's `HookInstallerFileTests`
//! (HookInstallerTests.swift) and the copy rules of DESIGN-WIN §4.3.
//!
//! A running image that refuses to be replaced exists only on Windows
//! (`agentnotch-win/tests/win_install.rs` proves it for real); here the
//! refusal is injected through `FileOps`.

use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::core::settings_doc::Json;
use agentnotch_engine::hooks::backups::{
    back_up_settings, backup_name, newest_backup, newest_status_line, our_backups, Backup,
    BACKUP_PREFIX, BACKUP_SUFFIX, MAX_BACKUPS, ORIGINAL_BACKUP_NAME,
};
use agentnotch_engine::hooks::commands::{hook_copy_path, Recogniser, HOOK_EXE_NAME};
use agentnotch_engine::hooks::copy::{
    install_hook_copy, install_hook_copy_with, is_leftover, marked_name, remove_hook_copy,
    remove_hook_copy_with, sweep, CopyError, CopyOutcome, FileOps, RemoveOutcome, ASIDE_MARK,
    STAGE_MARK,
};
use agentnotch_engine::hooks::plan::is_a_wrapper;
use agentnotch_engine::platform::Clock;
use agentnotch_engine::testkit::{mode_of, snapshot_dir, write_file, FakeClock, StdSecureFiles};
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 2026-09-21T14:13:20Z.
const START_MS: u64 = 1_790_000_000_000;

fn clock() -> FakeClock {
    FakeClock::at_ms(START_MS)
}

/// A config folder with a `projects` folder, as Claude Code leaves one, and
/// the settings.json given.
fn config_dir(root: &Path, settings: Option<&str>) -> PathBuf {
    let dir = root.join(".claude-test");
    fs::create_dir_all(dir.join("projects")).unwrap();
    if let Some(settings) = settings {
        write_file(&dir.join("settings.json"), settings);
    }
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn is_owner_only(path: &Path) -> bool {
    // Plain std can't read a Windows ACL; win_install.rs checks it there.
    !cfg!(unix) || mode_of(path) == Some(0o600)
}

// ---- Backups ----

const REALISTIC: &str = r#"{
  "model": "opus",
  "env": {"BASH_DEFAULT_TIMEOUT_MS": "300000"},
  "statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2}
}"#;

/// The backup half of `installIsIdempotentAndBacksUp`.
#[test]
fn backs_up_once_owner_only_and_keeps_the_original_apart() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), Some(REALISTIC));
    let settings = dir.join("settings.json");
    let (files, clock) = (StdSecureFiles, clock());

    let first = back_up_settings(REALISTIC.as_bytes(), &settings, &files, &clock);
    let backups = our_backups(&dir);
    assert_eq!(backups, vec![backup_name(clock.now())]);
    let path = dir.join(&backups[0]);
    assert_eq!(first, Backup::Written(path.clone()));
    assert_eq!(first.path(), Some(path.as_path()));
    assert_eq!(fs::read(&path).unwrap(), REALISTIC.as_bytes());
    assert!(is_owner_only(&path));
    let original = dir.join(ORIGINAL_BACKUP_NAME);
    assert_eq!(fs::read(&original).unwrap(), REALISTIC.as_bytes());
    assert!(is_owner_only(&original));
    assert_eq!(newest_backup(&settings), Some(path.clone()));
    // Nothing else appeared, and settings.json wasn't touched.
    assert_eq!(fs::read(&settings).unwrap(), REALISTIC.as_bytes());
    assert_eq!(names(&dir).len(), 4);

    // The same bytes again, later: the newest backup already holds them.
    clock.advance(Duration::from_secs(90));
    let second = back_up_settings(REALISTIC.as_bytes(), &settings, &files, &clock);
    assert_eq!(second, Backup::AlreadyHeld(path));
    assert_eq!(our_backups(&dir), backups);
}

#[test]
fn backup_names_are_timestamps_that_sort() {
    let clock = clock();
    let name = backup_name(clock.now());
    let stamp = name
        .strip_prefix(BACKUP_PREFIX)
        .and_then(|rest| rest.strip_suffix(BACKUP_SUFFIX))
        .expect("our prefix and suffix");
    // yyyyMMdd-HHmmss-SSS
    let parts: Vec<&str> = stamp.split('-').collect();
    assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 6, 3]);
    assert!(parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())));
    assert_eq!(parts[2], "000");
    clock.advance(Duration::from_millis(7));
    let later = backup_name(clock.now());
    assert!(later.ends_with("-007.bak"));
    assert!(later > name);
    // The original is not one of the rotated ones.
    assert!(!ORIGINAL_BACKUP_NAME.starts_with(BACKUP_PREFIX));
}

#[test]
fn keeps_only_the_newest_backups_and_the_original() {
    let root = tempfile::tempdir().unwrap();
    let first = r#"{"model":"opus"}"#;
    let dir = config_dir(root.path(), Some(first));
    let settings = dir.join("settings.json");
    let (files, clock) = (StdSecureFiles, clock());

    assert!(matches!(
        back_up_settings(first.as_bytes(), &settings, &files, &clock),
        Backup::Written(_)
    ));
    for index in 0..7 {
        let name = format!("{BACKUP_PREFIX}20260101-00000{index}-000{BACKUP_SUFFIX}");
        write_file(&dir.join(name), format!("old {index}"));
    }
    // Things that only look like ours, and must outlive every rotation.
    write_file(&dir.join("settings.json.bak"), "the user's own");
    write_file(
        &dir.join("settings.json.agentnotch-notes.txt"),
        "not a backup",
    );

    // Every write backs up, the rotation keeps five, and the pre-install
    // original stays.
    let mut written = vec![first.to_string()];
    for round in 0..4 {
        clock.advance(Duration::from_secs(1));
        let bytes = format!(r#"{{"model":"opus","round":{round}}}"#);
        assert!(matches!(
            back_up_settings(bytes.as_bytes(), &settings, &files, &clock),
            Backup::Written(_)
        ));
        written.push(bytes);
    }
    let kept = our_backups(&dir);
    assert_eq!(kept.len(), MAX_BACKUPS);
    // The five kept are the five newest: none of the planted old ones.
    let held: Vec<String> = kept
        .iter()
        .map(|name| fs::read_to_string(dir.join(name)).unwrap())
        .collect();
    assert_eq!(held, written);
    assert_eq!(
        fs::read(dir.join(ORIGINAL_BACKUP_NAME)).unwrap(),
        first.as_bytes()
    );
    assert_eq!(
        fs::read_to_string(dir.join("settings.json.bak")).unwrap(),
        "the user's own"
    );
    assert!(dir.join("settings.json.agentnotch-notes.txt").exists());
}

/// Two writes in one millisecond: the later backup takes the next name
/// rather than the earlier one's place.
#[test]
fn a_name_clash_keeps_both_backups() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    let settings = dir.join("settings.json");
    let (files, clock) = (StdSecureFiles, clock());

    let one = back_up_settings(b"{\"a\":1}", &settings, &files, &clock);
    let two = back_up_settings(b"{\"a\":2}", &settings, &files, &clock);
    let three = back_up_settings(b"{\"a\":1}", &settings, &files, &clock);
    let backups = our_backups(&dir);
    assert_eq!(backups.len(), 3);
    assert_eq!(one.path(), Some(dir.join(&backups[0]).as_path()));
    assert_eq!(two.path(), Some(dir.join(&backups[1]).as_path()));
    assert_eq!(three.path(), Some(dir.join(&backups[2]).as_path()));
    assert_eq!(fs::read(dir.join(&backups[0])).unwrap(), b"{\"a\":1}");
    assert_eq!(fs::read(dir.join(&backups[1])).unwrap(), b"{\"a\":2}");
    assert!(backups.iter().all(|name| is_owner_only(&dir.join(name))));
    // The original is the first bytes ever seen, not the latest.
    assert_eq!(
        fs::read(dir.join(ORIGINAL_BACKUP_NAME)).unwrap(),
        b"{\"a\":1}"
    );
}

#[test]
fn a_blank_file_is_not_backed_up() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), Some(""));
    let settings = dir.join("settings.json");
    let before = snapshot_dir(&dir).unwrap();
    for blank in [&b""[..], b"  \r\n\t", b"\xEF\xBB\xBF", b"\xEF\xBB\xBF\n"] {
        assert_eq!(
            back_up_settings(blank, &settings, &StdSecureFiles, &clock()),
            Backup::Blank
        );
    }
    assert_eq!(snapshot_dir(&dir).unwrap(), before);
    assert_eq!(newest_backup(&settings), None);
}

#[test]
fn a_backup_that_cannot_be_written_says_so() {
    let root = tempfile::tempdir().unwrap();
    // The folder doesn't exist: nothing can be created in it.
    let settings = root.path().join("gone").join("settings.json");
    let result = back_up_settings(b"{}", &settings, &StdSecureFiles, &clock());
    assert!(matches!(result, Backup::Failed(_)), "{result:?}");
    assert_eq!(result.path(), None);
    assert!(!root.path().join("gone").exists());
}

// ---- The status line in the backups ----

const OUR_STATUS: &str = "C:/Users/me/.claude-work/hooks/agentnotch-hook.exe statusline";

fn backup_at(dir: &Path, stamp: &str, settings: &str) {
    write_file(
        &dir.join(format!("{BACKUP_PREFIX}{stamp}{BACKUP_SUFFIX}")),
        settings,
    );
}

fn status_line_in_backups(dir: &Path) -> Option<Json> {
    let recogniser = Recogniser::new(PathStyle::Windows);
    newest_status_line(&dir.join("settings.json"), &|status_line| {
        is_a_wrapper(status_line, &recogniser)
    })
}

fn command_of(status_line: Option<Json>) -> Option<String> {
    status_line?.get("command")?.as_str().map(str::to_owned)
}

#[test]
fn the_newest_status_line_skips_backups_taken_while_a_wrapper_was_active() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    assert_eq!(status_line_in_backups(&dir), None);

    backup_at(
        &dir,
        "20260901-100000-000",
        r#"{"statusLine":{"type":"command","command":"~/older.sh"}}"#,
    );
    backup_at(
        &dir,
        "20260902-100000-000",
        r#"{"statusLine":{"type":"command","command":"~/mine.sh","padding":2}}"#,
    );
    // Taken while ours was the status line, in both spellings, and while a
    // former name's wrapper was (the loop guard's names).
    backup_at(
        &dir,
        "20260903-100000-000",
        &format!(r#"{{"statusLine":{{"type":"command","command":"{OUR_STATUS}"}}}}"#),
    );
    backup_at(
        &dir,
        "20260904-100000-000",
        r#"{"statusLine":{"type":"command","command":"C:\\Users\\me\\.claude\\hooks\\AgentNotch-Hook.exe statusline","padding":0}}"#,
    );
    backup_at(
        &dir,
        "20260905-100000-000",
        r#"{"statusLine":{"type":"command","command":"python3 ~/.claude/hooks/agentnotch-statusline.py"}}"#,
    );
    // Not settings at all: passed over.
    backup_at(&dir, "20260906-100000-000", "[1, 2");
    // Other files are never read for it.
    write_file(
        &dir.join("settings.json.bak"),
        r#"{"statusLine":{"type":"command","command":"~/not-ours.sh"}}"#,
    );

    let found = status_line_in_backups(&dir).expect("the user's status line");
    assert_eq!(
        found.get("command").and_then(Json::as_str),
        Some("~/mine.sh")
    );
    assert!(found.get("padding").is_some());
}

#[test]
fn a_newest_backup_without_a_status_line_means_there_was_none() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    backup_at(
        &dir,
        "20260901-100000-000",
        r#"{"statusLine":{"type":"command","command":"~/removed-since.sh"}}"#,
    );
    backup_at(&dir, "20260902-100000-000", r#"{"model":"opus"}"#);
    assert_eq!(status_line_in_backups(&dir), None);

    // An empty object says the same.
    backup_at(&dir, "20260903-100000-000", r#"{"statusLine":{}}"#);
    assert_eq!(status_line_in_backups(&dir), None);
}

#[test]
fn the_original_backup_is_consulted_after_the_timestamped_ones() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    write_file(
        &dir.join(ORIGINAL_BACKUP_NAME),
        r#"{"statusLine":{"type":"command","command":"~/original.sh"}}"#,
    );
    assert_eq!(
        command_of(status_line_in_backups(&dir)).as_deref(),
        Some("~/original.sh")
    );

    // Every timestamped one was taken under the wrapper: the original decides.
    backup_at(
        &dir,
        "20260903-100000-000",
        &format!(r#"{{"statusLine":{{"type":"command","command":"{OUR_STATUS}"}}}}"#),
    );
    assert_eq!(
        command_of(status_line_in_backups(&dir)).as_deref(),
        Some("~/original.sh")
    );

    // A timestamped one that wasn't wins over the original, whatever its name
    // sorts as.
    backup_at(
        &dir,
        "20260902-100000-000",
        r#"{"statusLine":{"type":"command","command":"~/later.sh"}}"#,
    );
    assert_eq!(
        command_of(status_line_in_backups(&dir)).as_deref(),
        Some("~/later.sh")
    );
}

// ---- The hook copy ----

/// The exe beside the app, with bytes that aren't text.
fn source_exe(root: &Path, bytes: &[u8]) -> PathBuf {
    let source = root.join("install").join(HOOK_EXE_NAME);
    write_file(&source, bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    }
    source
}

const V1: &[u8] = b"MZ\x90\x00 agentnotch-hook 1.1.0 \xff\xfe\x00";
/// The same length as `V1`: only the SHA-256 tells them apart.
const V2: &[u8] = b"MZ\x90\x00 agentnotch-hook 1.1.1 \xff\xfe\x00";

#[test]
fn the_copy_is_written_when_absent_skipped_when_the_same_and_replaced_when_different() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), Some(REALISTIC));
    let source = source_exe(root.path(), V1);
    let copy = hook_copy_path(&dir);
    let clock = clock();

    // Absent: the hooks folder and the copy appear, and nothing else.
    assert_eq!(
        install_hook_copy(&source, &dir, &clock).unwrap(),
        CopyOutcome::Written
    );
    assert_eq!(fs::read(&copy).unwrap(), V1);
    assert_eq!(names(&dir.join("hooks")), vec![HOOK_EXE_NAME.to_string()]);
    if cfg!(unix) {
        assert_eq!(mode_of(&copy), Some(0o755));
    }
    assert_eq!(
        fs::read(dir.join("settings.json")).unwrap(),
        REALISTIC.as_bytes()
    );

    // The same bytes: not rewritten (the file is the very same one).
    let before = fs::metadata(&copy).unwrap().modified().unwrap();
    let refuse_all = FileOps {
        rename: &|_, _| panic!("nothing to rename when the copy is current"),
        remove: &|_| panic!("nothing to remove when the copy is current"),
    };
    clock.advance(Duration::from_secs(5));
    assert_eq!(
        install_hook_copy_with(&source, &dir, &clock, &refuse_all).unwrap(),
        CopyOutcome::AlreadyCurrent
    );
    assert_eq!(fs::metadata(&copy).unwrap().modified().unwrap(), before);

    // Same size, other bytes: replaced, with no stage left behind.
    assert_eq!(V1.len(), V2.len());
    write_file(&source, V2);
    assert_eq!(
        install_hook_copy(&source, &dir, &clock).unwrap(),
        CopyOutcome::Written
    );
    assert_eq!(fs::read(&copy).unwrap(), V2);
    assert_eq!(names(&dir.join("hooks")), vec![HOOK_EXE_NAME.to_string()]);

    // Another size.
    write_file(&source, b"MZ short");
    assert_eq!(
        install_hook_copy(&source, &dir, &clock).unwrap(),
        CopyOutcome::Written
    );
    assert_eq!(fs::read(&copy).unwrap(), b"MZ short");
    assert_eq!(names(&dir.join("hooks")), vec![HOOK_EXE_NAME.to_string()]);
}

/// A rename asked for: from, to, and what the copy's path held just before.
type RenameCall = (PathBuf, PathBuf, Option<Vec<u8>>);

/// A hook is running the copy: replacing it is refused, renaming it is not.
#[test]
fn a_refused_replacement_moves_the_running_copy_aside_and_the_new_one_in_at_once() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    let copy = hook_copy_path(&dir);
    let hooks = dir.join("hooks");
    let clock = clock();
    install_hook_copy(&source_exe(root.path(), V1), &dir, &clock).unwrap();
    let source = source_exe(root.path(), V2);

    clock.advance(Duration::from_secs(60));
    let stage = hooks.join(marked_name(STAGE_MARK, clock.now()));
    let aside = hooks.join(marked_name(ASIDE_MARK, clock.now()));

    // Each rename asked for, with what the copy's path held just before it.
    let calls: RefCell<Vec<RenameCall>> = RefCell::new(Vec::new());
    let stage_when_refused: RefCell<Option<Vec<u8>>> = RefCell::new(None);
    let ops = FileOps {
        rename: &|from, to| {
            let first = calls.borrow().is_empty();
            calls
                .borrow_mut()
                .push((from.to_path_buf(), to.to_path_buf(), fs::read(&copy).ok()));
            if first {
                // The stage is complete before anything is asked of the copy.
                *stage_when_refused.borrow_mut() = fs::read(from).ok();
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the image is running",
                ));
            }
            fs::rename(from, to)
        },
        remove: &|path| fs::remove_file(path),
    };

    let outcome = install_hook_copy_with(&source, &dir, &clock, &ops).unwrap();
    assert_eq!(
        outcome,
        CopyOutcome::ReplacedRunning {
            aside: aside.clone()
        }
    );
    let calls = calls.into_inner();
    let moves: Vec<(&Path, &Path)> = calls
        .iter()
        .map(|(from, to, _)| (from.as_path(), to.as_path()))
        .collect();
    assert_eq!(
        moves,
        vec![
            (stage.as_path(), copy.as_path()),
            (copy.as_path(), aside.as_path()),
            (stage.as_path(), copy.as_path()),
        ]
    );
    assert_eq!(stage_when_refused.into_inner().as_deref(), Some(V2));
    // The old exe was there until the rename that set it aside, and the one
    // step where the path is empty is followed at once by the rename that
    // fills it: there is no other call in between.
    assert_eq!(calls[0].2.as_deref(), Some(V1));
    assert_eq!(calls[1].2.as_deref(), Some(V1));
    assert_eq!(calls[2].2, None);
    assert_eq!(fs::read(&copy).unwrap(), V2);
    assert_eq!(fs::read(&aside).unwrap(), V1);
    assert_eq!(
        names(&hooks),
        vec![
            HOOK_EXE_NAME.to_string(),
            marked_name(ASIDE_MARK, clock.now())
        ]
    );

    // The next pass finds the copy current and sweeps the one set aside.
    clock.advance(Duration::from_secs(600));
    assert_eq!(
        install_hook_copy(&source, &dir, &clock).unwrap(),
        CopyOutcome::AlreadyCurrent
    );
    assert_eq!(names(&hooks), vec![HOOK_EXE_NAME.to_string()]);
}

/// Neither rename is allowed: the copy in place stays, and no stage is left.
#[test]
fn a_replacement_refused_twice_leaves_the_copy_as_it_was() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    let copy = hook_copy_path(&dir);
    let clock = clock();
    install_hook_copy(&source_exe(root.path(), V1), &dir, &clock).unwrap();
    let source = source_exe(root.path(), V2);
    let denied = || io::Error::new(io::ErrorKind::PermissionDenied, "held");

    let ops = FileOps {
        rename: &|_, _| Err(denied()),
        remove: &|path| fs::remove_file(path),
    };
    let error = install_hook_copy_with(&source, &dir, &clock, &ops).unwrap_err();
    assert!(matches!(error, CopyError::Replace(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        format!("Couldn't write {HOOK_EXE_NAME}: held")
    );
    assert_eq!(fs::read(&copy).unwrap(), V1);
    assert_eq!(names(&dir.join("hooks")), vec![HOOK_EXE_NAME.to_string()]);

    // Set aside, but the stage then can't be moved in: the old copy goes back.
    let count = RefCell::new(0);
    let ops = FileOps {
        rename: &|from, to| {
            *count.borrow_mut() += 1;
            match *count.borrow() {
                1 | 3 => Err(denied()),
                _ => fs::rename(from, to),
            }
        },
        remove: &|path| fs::remove_file(path),
    };
    let error = install_hook_copy_with(&source, &dir, &clock, &ops).unwrap_err();
    assert!(matches!(error, CopyError::Replace(_)), "{error:?}");
    assert_eq!(*count.borrow(), 4);
    assert_eq!(fs::read(&copy).unwrap(), V1);
    assert_eq!(names(&dir.join("hooks")), vec![HOOK_EXE_NAME.to_string()]);
}

#[test]
fn the_sweep_removes_leftovers_and_nothing_else() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), Some(REALISTIC));
    let hooks = dir.join("hooks");
    let clock = clock();
    let kept = [
        HOOK_EXE_NAME,
        "agentnotch-statusline.previous.json",
        "codenotch-hook.exe",
        "agentnotch-hook.exe.bak",
        "agentnotch-hook.old-notes.txt",
        "my.old-20260101000000.exe",
    ];
    for name in kept {
        write_file(&hooks.join(name), name);
    }
    let leftovers = [
        "agentnotch-hook.old-20260101000000.exe".to_string(),
        "agentnotch-hook.old-20260102000000.exe".to_string(),
        "agentnotch-hook.new-20260103000000.exe".to_string(),
        marked_name(STAGE_MARK, clock.now()),
        marked_name(ASIDE_MARK, clock.now()),
    ];
    for name in &leftovers {
        assert!(is_leftover(name));
        write_file(&hooks.join(name), "left over");
    }
    // A folder with a leftover's name is someone else's: never removed.
    let folder = hooks.join("agentnotch-hook.old-20260104000000.exe");
    fs::create_dir_all(&folder).unwrap();
    write_file(&folder.join("inside.txt"), "kept");
    // The same names outside `hooks` aren't ours to sweep.
    write_file(&dir.join(&leftovers[0]), "elsewhere");
    let outside = snapshot_dir(&dir.join("projects")).unwrap();

    assert_eq!(sweep(&dir), leftovers.len());
    let mut expected: Vec<String> = kept.iter().map(|name| name.to_string()).collect();
    expected.push("agentnotch-hook.old-20260104000000.exe".into());
    expected.sort();
    assert_eq!(names(&hooks), expected);
    for name in kept {
        assert_eq!(fs::read_to_string(hooks.join(name)).unwrap(), name);
    }
    assert!(folder.join("inside.txt").exists());
    assert!(dir.join(&leftovers[0]).exists());
    assert_eq!(
        fs::read(dir.join("settings.json")).unwrap(),
        REALISTIC.as_bytes()
    );
    assert_eq!(snapshot_dir(&dir.join("projects")).unwrap(), outside);

    // Nothing left to sweep, and no folder is no error.
    assert_eq!(sweep(&dir), 0);
    assert_eq!(sweep(&root.path().join("no-such-folder")), 0);
}

/// A leftover that is still running can't be deleted: it stays for later.
#[test]
fn the_sweep_ignores_what_it_cannot_delete() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    let hooks = dir.join("hooks");
    let running = "agentnotch-hook.old-20260101000000.exe";
    write_file(&hooks.join(running), "running");
    write_file(
        &hooks.join("agentnotch-hook.new-20260101000000.exe"),
        "stale",
    );
    let source = source_exe(root.path(), V1);

    let ops = FileOps {
        rename: &|from, to| fs::rename(from, to),
        remove: &|path| {
            if path.ends_with(running) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "running"));
            }
            fs::remove_file(path)
        },
    };
    assert_eq!(
        install_hook_copy_with(&source, &dir, &clock(), &ops).unwrap(),
        CopyOutcome::Written
    );
    assert_eq!(
        names(&hooks),
        vec![HOOK_EXE_NAME.to_string(), running.to_string()]
    );
}

#[test]
fn a_missing_source_leaves_the_folder_untouched() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), Some(REALISTIC));
    let missing = root.path().join("install").join(HOOK_EXE_NAME);

    // No hooks folder yet: none is made.
    let before = snapshot_dir(&dir).unwrap();
    let error = install_hook_copy(&missing, &dir, &clock()).unwrap_err();
    assert!(matches!(error, CopyError::SourceMissing(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        format!("{HOOK_EXE_NAME} is not available")
    );
    assert_eq!(snapshot_dir(&dir).unwrap(), before);
    assert!(!dir.join("hooks").exists());

    // An earlier install's copy and its leftovers: all as they were.
    let hooks = dir.join("hooks");
    write_file(&hooks.join(HOOK_EXE_NAME), V1);
    write_file(&hooks.join("agentnotch-hook.old-20260101000000.exe"), "old");
    write_file(&hooks.join("agentnotch-hook.new-20260101000000.exe"), "new");
    let before = snapshot_dir(&dir).unwrap();
    let error = install_hook_copy(&missing, &dir, &clock()).unwrap_err();
    assert!(matches!(error, CopyError::SourceMissing(_)), "{error:?}");
    assert_eq!(snapshot_dir(&dir).unwrap(), before);
}

/// An earlier stage of this very second is still there and can't be deleted:
/// the new stage takes the next name instead of failing.
#[test]
fn a_stage_name_that_is_taken_is_stepped_over() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    let hooks = dir.join("hooks");
    let clock = clock();
    let taken = marked_name(STAGE_MARK, clock.now());
    write_file(&hooks.join(&taken), "held by someone");
    let source = source_exe(root.path(), V1);

    let staged_as: RefCell<Option<PathBuf>> = RefCell::new(None);
    let ops = FileOps {
        rename: &|from, to| {
            *staged_as.borrow_mut() = Some(from.to_path_buf());
            fs::rename(from, to)
        },
        remove: &|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "held")),
    };
    assert_eq!(
        install_hook_copy_with(&source, &dir, &clock, &ops).unwrap(),
        CopyOutcome::Written
    );
    let next = marked_name(STAGE_MARK, clock.now() + Duration::from_secs(1));
    assert_eq!(staged_as.into_inner(), Some(hooks.join(next)));
    assert_eq!(fs::read(hook_copy_path(&dir)).unwrap(), V1);
    assert_eq!(
        fs::read_to_string(hooks.join(&taken)).unwrap(),
        "held by someone"
    );
}

#[test]
fn removal_deletes_the_copy_or_renames_it_aside() {
    let root = tempfile::tempdir().unwrap();
    let dir = config_dir(root.path(), None);
    let hooks = dir.join("hooks");
    let copy = hook_copy_path(&dir);
    let clock = clock();
    let source = source_exe(root.path(), V1);

    // Nothing installed.
    assert_eq!(
        remove_hook_copy(&dir, &clock).unwrap(),
        RemoveOutcome::Absent
    );
    assert!(!hooks.exists());

    // Deleted with its leftovers; other files in `hooks` stay.
    install_hook_copy(&source, &dir, &clock).unwrap();
    write_file(&hooks.join("agentnotch-hook.old-20260101000000.exe"), "old");
    write_file(&hooks.join("agentnotch-hook.new-20260101000000.exe"), "new");
    write_file(&hooks.join("guard.sh"), "the user's");
    assert_eq!(
        remove_hook_copy(&dir, &clock).unwrap(),
        RemoveOutcome::Deleted
    );
    assert_eq!(names(&hooks), vec!["guard.sh".to_string()]);
    assert_eq!(
        remove_hook_copy(&dir, &clock).unwrap(),
        RemoveOutcome::Absent
    );

    // Running: it can't be deleted, so it is renamed aside.
    install_hook_copy(&source, &dir, &clock).unwrap();
    let running = FileOps {
        rename: &|from, to| fs::rename(from, to),
        remove: &|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "running")),
    };
    let aside = hooks.join(marked_name(ASIDE_MARK, clock.now()));
    assert_eq!(
        remove_hook_copy_with(&dir, &clock, &running).unwrap(),
        RemoveOutcome::SetAside {
            aside: aside.clone()
        }
    );
    assert!(!copy.exists());
    assert_eq!(fs::read(&aside).unwrap(), V1);

    // Not even that: the error is reported and the copy is still there.
    install_hook_copy(&source, &dir, &clock).unwrap();
    let held = FileOps {
        rename: &|_, _| Err(io::Error::new(io::ErrorKind::PermissionDenied, "held")),
        remove: &|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "running")),
    };
    // (the install above swept the copy set aside earlier)
    assert!(!aside.exists());
    assert!(remove_hook_copy_with(&dir, &clock, &held).is_err());
    assert_eq!(fs::read(&copy).unwrap(), V1);

    // A later removal finishes the job.
    assert_eq!(
        remove_hook_copy(&dir, &clock).unwrap(),
        RemoveOutcome::Deleted
    );
    assert_eq!(names(&hooks), vec!["guard.sh".to_string()]);
}
