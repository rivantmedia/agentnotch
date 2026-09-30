//! The engine's hook installer over `WinFiles`, the real `SecureFiles`, in a temporary profile
//! (DESIGN-WIN §4.3, §6.2): a settings.json the way Windows tools leave one (a BOM, CRLF, other
//! tools' hooks, a status line) is installed into and given back byte for byte; a hook copy that
//! is running is replaced; and the two things that go wrong around the replace are played for
//! real: someone holds settings.json open without delete sharing, and someone deletes it after
//! the plan was made. Throughout, settings.json holds its old bytes or its new bytes.
//!
//! The same installer against plain `std::fs`, on every OS: `agentnotch-engine/tests/hooks_install.rs`.

#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agentnotch_engine::core::settings_doc::Json;
use agentnotch_engine::hooks::apply::{
    apply_installs_with, previous_status_line_path, read_saved_status_line, read_status, Setup,
    VANISHED,
};
use agentnotch_engine::hooks::backups::{our_backups, ORIGINAL_BACKUP_NAME};
use agentnotch_engine::hooks::commands::{
    hook_copy_path, string_command, string_command_for, Subcommand, HOOKS_DIR_NAME, HOOK_EXE_NAME,
};
use agentnotch_engine::hooks::copy::{is_leftover, marked_name, ASIDE_MARK, STAGE_MARK};
use agentnotch_engine::hooks::events::hook_events;
use agentnotch_engine::hooks::uninstall_everything;
use agentnotch_engine::model::AccountId;
use agentnotch_engine::persist::hook_install::{HookInstallEntry, HookInstallRecord};
use agentnotch_engine::platform::{
    Clock, Expect, FileIdentity, SecureFiles, WriteMode, WriteResult,
};
use agentnotch_engine::runtime_types::{
    CommandForm, InstallChange, InstallOutcome, InstallPlan, StatusLineIntent,
};
use agentnotch_engine::testkit::FakeClock;
use agentnotch_win::files::{retry_delays, WinFiles};
use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

/// 2026-09-21T14:13:20Z.
const START_MS: u64 = 1_790_000_000_000;

/// Git Bash is there: someone else's status line may be wrapped.
const SETUP: Setup = Setup { git_bash: true };

const HOOK_BYTES: &[u8] = b"MZ not really an exe";

/// What a plan made from the path alone says when no string command can carry it.
const NOT_POSSIBLE: &str = "no command can carry this path";

/// Set for the copy of this test binary that plays a running hook: the file it writes once it
/// is up.
const HOLD_ENV: &str = "AGENTNOTCH_WIN_INSTALL_HOLD";
const HELD_TEST: &str = "a_hook_copy_holds_still";

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

// The settings.json, in five pieces, so a test can say exactly which bytes are ours to change:
// the values of `hooks` and `statusLine`. Those two are laid out the way Claude Code writes
// them (and the way the installer writes them back); everything around them is laid out by
// hand, which a rewrite of the whole file would not survive.

const BEFORE_HOOKS: &str = "{\n  \"$schema\": \"https://json.schemastore.org/claude-code-settings.json\",\n  \"model\":   \"opus\",\n  \"hooks\": ";

const HOOKS: &str = r#"{
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "~/bin/guard-bash.sh",
            "timeout": 30
          }
        ]
      },
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe",
            "timeout": 5
          }
        ]
      }
    ],
    "PermissionRequest": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "node ~/tools/notify.js --skip agentnotch-hook.exe",
            "timeout": 86400
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe"
          }
        ]
      }
    ]
  }"#;

const BETWEEN: &str = ",\n  \"permissions\": {\"allow\": [\"Bash(npm test:*)\"],   \"deny\": []},\n  \"statusLine\": ";

const STATUS_LINE: &str = r#"{
    "type": "command",
    "command": "~/.claude/statusline.sh",
    "padding": 2
  }"#;

const AFTER: &str = ",\n  \"env\": {\"BASH_DEFAULT_TIMEOUT_MS\": \"300000\"}\n}\n";

const GUARD_COMMAND: &str = "~/bin/guard-bash.sh";
const UPSTREAM_COMMAND: &str = "C:/Users/me/AppData/Local/codenotch/codenotch-hook.exe";
const MENTION_COMMAND: &str = "node ~/tools/notify.js --skip agentnotch-hook.exe";
const THEIR_STATUS_LINE: &str = "~/.claude/statusline.sh";

fn crlf(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

fn before_hooks() -> Vec<u8> {
    let mut bytes = BOM.to_vec();
    bytes.extend(crlf(BEFORE_HOOKS));
    bytes
}

/// The settings.json every test starts from: a BOM, CRLF throughout.
fn original() -> Vec<u8> {
    let mut bytes = before_hooks();
    for part in [HOOKS, BETWEEN, STATUS_LINE, AFTER] {
        bytes.extend(crlf(part));
    }
    bytes
}

/// Proves every byte of `written` outside the two values we may change is the original's, and
/// that what we wrote keeps the file's line endings. Returns the two values as written.
fn spliced(written: &[u8]) -> (Json, Json) {
    let (before, between, after) = (before_hooks(), crlf(BETWEEN), crlf(AFTER));
    assert!(
        written.starts_with(&before),
        "the bytes before the hooks changed"
    );
    assert!(
        written.ends_with(&after),
        "the bytes after the status line changed"
    );
    let body = &written[before.len()..written.len() - after.len()];
    let found: Vec<usize> = (0..body.len())
        .filter(|&at| body[at..].starts_with(&between))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "the bytes between the hooks and the status line changed"
    );
    let bare_line_feed = written
        .iter()
        .enumerate()
        .any(|(at, &byte)| byte == b'\n' && (at == 0 || written[at - 1] != b'\r'));
    assert!(!bare_line_feed, "a line of ours doesn't end in CRLF");
    let hooks = Json::parse(&body[..found[0]]).expect("the hooks value parses");
    let status_line =
        Json::parse(&body[found[0] + between.len()..]).expect("the status line value parses");
    assert!(hooks.is_object() && status_line.is_object());
    (hooks, status_line)
}

/// The `hooks` object of a whole settings.json.
fn hooks_of(settings: &[u8]) -> Json {
    let text = settings.strip_prefix(&BOM[..]).unwrap_or(settings);
    Json::parse(text)
        .expect("settings.json parses")
        .get("hooks")
        .cloned()
        .expect("settings.json has hooks")
}

/// The commands `hooks` registers for `event`, in file order.
fn commands(hooks: &Json, event: &str) -> Vec<String> {
    hooks
        .get(event)
        .and_then(Json::items)
        .unwrap_or_default()
        .iter()
        .flat_map(|group| group.get("hooks").and_then(Json::items).unwrap_or_default())
        .filter_map(|entry| entry.get("command").and_then(Json::as_str))
        .map(str::to_owned)
        .collect()
}

/// Ours on every event, everyone else's where they were.
fn assert_hooked(hooks: &Json, command: &str) {
    for event in hook_events(None) {
        assert!(
            commands(hooks, event).iter().any(|found| found == command),
            "missing {event}"
        );
    }
    assert_eq!(
        commands(hooks, "PreToolUse")[..2],
        [GUARD_COMMAND.to_owned(), UPSTREAM_COMMAND.to_owned()]
    );
    assert_eq!(commands(hooks, "PermissionRequest")[0], MENTION_COMMAND);
    assert_eq!(commands(hooks, "Stop")[0], UPSTREAM_COMMAND);
}

struct Fixture {
    _temp: tempfile::TempDir,
    /// Resolved, so paths built from it are the ones the installer reports.
    root: PathBuf,
    /// The hook exe beside the app.
    source: PathBuf,
    clock: FakeClock,
    files: WinFiles,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let files = WinFiles::new();
    let root = files.canonical(temp.path()).unwrap();
    let source = root.join("app").join(HOOK_EXE_NAME);
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, HOOK_BYTES).unwrap();
    Fixture {
        _temp: temp,
        root,
        source,
        clock: FakeClock::at_ms(START_MS),
        files,
    }
}

impl Fixture {
    /// A config folder with a `projects` folder, as Claude Code leaves one, and the
    /// settings.json given.
    fn config_dir(&self, name: impl AsRef<Path>, settings: Option<&[u8]>) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(dir.join("projects")).unwrap();
        if let Some(settings) = settings {
            fs::write(dir.join("settings.json"), settings).unwrap();
        }
        dir
    }

    /// Hooks and the status line, as the manager plans them from the path alone.
    fn plan(&self, dir: &Path) -> InstallPlan {
        InstallPlan {
            folder: folder(dir),
            settings_path: settings(dir),
            expected: Expect::Nothing,
            existed: false,
            hook_copy: Some((self.source.clone(), hook_copy_path(dir))),
            form: string_command(&hook_copy_path(dir).to_string_lossy(), Subcommand::Hook)
                .map_or_else(
                    || CommandForm::NotPossible(NOT_POSSIBLE.to_owned()),
                    CommandForm::Text,
                ),
            events: hook_events(None).into_iter().map(str::to_owned).collect(),
            status_line: StatusLineIntent::Wrap,
            remove_only: false,
        }
    }

    /// One pass over `dir`, a second later than the last.
    fn install_with(&self, dir: &Path, files: &dyn SecureFiles) -> InstallOutcome {
        self.clock.advance(Duration::from_secs(1));
        let mut outcomes = apply_installs_with(&[self.plan(dir)], files, &self.clock, &SETUP);
        assert_eq!(outcomes.len(), 1);
        outcomes.remove(0)
    }

    fn install(&self, dir: &Path) -> InstallOutcome {
        self.install_with(dir, &self.files)
    }

    /// The uninstall the CLI runs: from the record alone.
    fn uninstall(&self, entry: HookInstallEntry) -> InstallOutcome {
        self.clock.advance(Duration::from_secs(1));
        let record = HookInstallRecord { files: vec![entry] };
        let mut outcomes = uninstall_everything(&record, &self.files, &self.clock);
        assert_eq!(outcomes.len(), 1);
        outcomes.remove(0)
    }
}

fn folder(dir: &Path) -> AccountId {
    AccountId(dir.to_string_lossy().into_owned())
}

fn settings(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn hook_command(dir: &Path) -> String {
    string_command(&hook_copy_path(dir).to_string_lossy(), Subcommand::Hook)
        .expect("a temporary folder's path needs no quotes")
}

fn status_line_command(dir: &Path) -> String {
    string_command(
        &hook_copy_path(dir).to_string_lossy(),
        Subcommand::StatusLine,
    )
    .expect("a temporary folder's path needs no quotes")
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

/// Stage files of an atomic write in `dir`.
fn stages(dir: &Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

/// The stage of a settings.json replace, and not of a backup beside it.
fn is_settings_stage(name: &str) -> bool {
    name.strip_prefix(".settings.json.agentnotch-")
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|hex| hex.len() == 8 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// `.new-` and `.old-` copies beside the hook copy.
fn copy_leftovers(dir: &Path) -> Vec<String> {
    names(&dir.join(HOOKS_DIR_NAME))
        .into_iter()
        .filter(|name| is_leftover(name))
        .collect()
}

fn is_missing(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
}

// ---- Things that happen at the worst moment ----

type BeforeWrite = Box<dyn FnMut(&Path) + Send>;

/// `WinFiles`, with a say just before each settings.json replace: after the installer read the
/// file and made its plan, before the real write starts.
struct Hooked {
    inner: WinFiles,
    before_settings_write: Mutex<BeforeWrite>,
    settings_writes: AtomicUsize,
    /// The path each settings.json replace was asked for.
    written_paths: Mutex<Vec<PathBuf>>,
}

impl Hooked {
    fn new(before: impl FnMut(&Path) + Send + 'static) -> Hooked {
        Hooked {
            inner: WinFiles::new(),
            before_settings_write: Mutex::new(Box::new(before)),
            settings_writes: AtomicUsize::new(0),
            written_paths: Mutex::new(Vec::new()),
        }
    }

    fn settings_writes(&self) -> usize {
        self.settings_writes.load(Ordering::SeqCst)
    }
}

impl SecureFiles for Hooked {
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
        // Only settings.json is written with its own security kept.
        if mode == WriteMode::KeepTargetSecurity {
            self.settings_writes.fetch_add(1, Ordering::SeqCst);
            self.written_paths.lock().unwrap().push(path.to_path_buf());
            (self.before_settings_write.lock().unwrap())(path);
        }
        self.inner.write_atomic(path, bytes, mode, expect)
    }

    fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
        self.inner.create_exclusive(path, bytes)
    }

    fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
        self.inner.identity(path)
    }

    fn is_reparse(&self, path: &Path) -> io::Result<bool> {
        self.inner.is_reparse(path)
    }

    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonical(path)
    }

    fn is_private(&self, path: &Path) -> io::Result<bool> {
        self.inner.is_private(path)
    }

    fn short_path(&self, path: &Path) -> Option<PathBuf> {
        self.inner.short_path(path)
    }

    fn long_path(&self, path: &Path) -> Option<PathBuf> {
        self.inner.long_path(path)
    }
}

/// A thread holding a file open, and the flag that turns true just before it lets go.
type Hold = (JoinHandle<()>, Arc<AtomicBool>);

/// Opens `path` on another thread the way a scanner or an editor does (others may read and
/// write it, nobody may delete or replace it), returns once it is open, and lets go after
/// `hold`.
fn hold_open(path: &Path, hold: Duration) -> Hold {
    let (opened, is_open) = mpsc::channel();
    let releasing = Arc::new(AtomicBool::new(false));
    let thread = {
        let path = path.to_owned();
        let releasing = Arc::clone(&releasing);
        thread::spawn(move || {
            let held = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                .open(&path)
                .unwrap();
            opened.send(()).unwrap();
            thread::sleep(hold);
            releasing.store(true, Ordering::SeqCst);
            drop(held);
        })
    };
    is_open.recv().unwrap();
    (thread, releasing)
}

/// Files that take hold of settings.json for `hold` just before its first replace.
fn holding(hold: Duration) -> (Hooked, Arc<Mutex<Option<Hold>>>) {
    let held = Arc::new(Mutex::new(None));
    let files = {
        let held = Arc::clone(&held);
        Hooked::new(move |path| {
            let mut held = held.lock().unwrap();
            if held.is_none() {
                *held = Some(hold_open(path, hold));
            }
        })
    };
    (files, held)
}

/// Something looked at every millisecond from another thread.
struct Watch<T> {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<T>,
}

/// Polls once before returning, so the watch covers everything the caller does next.
fn every_millisecond<T: Send + 'static>(
    mut state: T,
    mut poll: impl FnMut(&mut T) + Send + 'static,
) -> Watch<T> {
    let stop = Arc::new(AtomicBool::new(false));
    let (started, is_started) = mpsc::channel();
    let thread = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            poll(&mut state);
            let _ = started.send(());
            while !stop.load(Ordering::SeqCst) {
                poll(&mut state);
                thread::sleep(Duration::from_millis(1));
            }
            poll(&mut state);
            state
        })
    };
    is_started.recv().unwrap();
    Watch { stop, thread }
}

impl<T> Watch<T> {
    fn finish(self) -> T {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

/// What a reader of settings.json saw, the way Claude Code reads it when it reloads.
#[derive(Default)]
struct Reads {
    good: usize,
    /// "No such file": what Claude Code would take for "no settings".
    missing: usize,
    /// Any other failure to read.
    failed: Vec<String>,
    /// Each content in turn, a repeat counted once.
    contents: Vec<Vec<u8>>,
}

fn read_every_millisecond(path: &Path) -> Watch<Reads> {
    let path = path.to_owned();
    every_millisecond(Reads::default(), move |reads| match fs::read(&path) {
        Ok(bytes) => {
            reads.good += 1;
            if reads.contents.last() != Some(&bytes) {
                reads.contents.push(bytes);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => reads.missing += 1,
        Err(error) => reads.failed.push(error.to_string()),
    })
}

// ---- Install, then uninstall ----

#[test]
fn install_then_uninstall_gives_the_file_back_byte_for_byte() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(".claude", Some(&original));
    let private_before = fx.files.is_private(&settings(&dir)).unwrap();

    let outcome = fx.install(&dir);
    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert_eq!(outcome.settings_path, settings(&dir));
    assert_eq!(outcome.status_line, Some(StatusLineIntent::Wrap));

    // The copy is in place.
    let copy = hook_copy_path(&dir);
    assert_eq!(fs::read(&copy).unwrap(), HOOK_BYTES);

    // Only the two values changed; ours are in, everyone else's are where they were.
    let written = fs::read(settings(&dir)).unwrap();
    assert_ne!(written, original);
    let (hooks, status_line) = spliced(&written);
    assert_hooked(&hooks, &hook_command(&dir));
    assert_eq!(
        status_line.get("command").and_then(Json::as_str),
        Some(status_line_command(&dir).as_str())
    );
    // The replace kept the file's own security: it was nobody's business to tighten it.
    assert_eq!(
        fx.files.is_private(&settings(&dir)).unwrap(),
        private_before
    );
    assert!(stages(&dir).is_empty(), "{:?}", stages(&dir));

    // The backups hold the original bytes, and only the user can read them.
    let backups = our_backups(&dir);
    assert_eq!(backups.len(), 1);
    let backup = dir.join(&backups[0]);
    assert_eq!(outcome.backup, Some(backup.clone()));
    let kept_original = dir.join(ORIGINAL_BACKUP_NAME);
    for held in [&backup, &kept_original] {
        assert_eq!(fs::read(held).unwrap(), original, "{}", held.display());
        assert!(fx.files.is_private(held).unwrap(), "{}", held.display());
    }
    // So does the status line that was taken over.
    let saved_path = previous_status_line_path(&dir);
    let saved = read_saved_status_line(&saved_path).expect("the saved status line");
    assert_eq!(
        saved.get("command").and_then(Json::as_str),
        Some(THEIR_STATUS_LINE)
    );
    assert!(fx.files.is_private(&saved_path).unwrap());

    let status = read_status(&dir, &fx.files, &SETUP);
    assert!(status.config_dir_exists && status.settings_readable);
    assert!(status.hooks_registered && status.hooks_installed);
    assert!(status.status_line_installed);
    assert!(status.codenotch_hooks);
    assert_eq!(status.form.as_deref(), Some("string"));

    let entry = outcome.entry.expect("an entry for hook-install.json");
    assert_eq!(entry.settings_path, settings(&dir));
    assert_eq!(entry.command, hook_command(&dir));
    assert_eq!(entry.hook_copy, Some(copy.clone()));
    assert!(entry.status_line);

    // A second pass finds nothing to do and writes nothing.
    let again = fx.install(&dir);
    assert_eq!(again.result, Ok(InstallChange::Unchanged));
    assert_eq!(fs::read(settings(&dir)).unwrap(), written);
    assert_eq!(our_backups(&dir).len(), 1);

    // Uninstall: the file is the original again, to the byte, and nothing of ours is left
    // but the backups.
    let removed = fx.uninstall(entry);
    assert_eq!(removed.result, Ok(InstallChange::Removed));
    assert_eq!(removed.entry, None);
    assert_eq!(fs::read(settings(&dir)).unwrap(), original);
    assert!(is_missing(&copy));
    assert!(is_missing(&saved_path));
    assert!(is_missing(&dir.join(HOOKS_DIR_NAME)));
    assert!(stages(&dir).is_empty(), "{:?}", stages(&dir));
    for name in our_backups(&dir) {
        assert!(fx.files.is_private(&dir.join(&name)).unwrap(), "{name}");
    }
    assert_eq!(fs::read(&kept_original).unwrap(), original);
    assert_eq!(
        fx.files.is_private(&settings(&dir)).unwrap(),
        private_before
    );
}

// ---- A hook copy that is running ----

/// A process that is ended when the test ends, however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Not a test of its own: the body of the process that plays a running hook. A copy of this
/// test binary is installed as the hook exe and started on this test alone, where it says it
/// is up and then holds still until it is killed.
#[test]
#[ignore = "runs only as the held hook copy of a_running_hook_copy_is_replaced"]
fn a_hook_copy_holds_still() {
    let Some(ready) = std::env::var_os(HOLD_ENV) else {
        return;
    };
    fs::write(ready, b"running").unwrap();
    // The test kills this process; the limit is for a test that died first.
    thread::sleep(Duration::from_secs(120));
}

/// How long the hook copy's path was seen naming nothing.
#[derive(Default)]
struct Gaps {
    polls: usize,
    gaps: usize,
    longest: Duration,
}

fn watch_for_gaps(path: &Path) -> Watch<Gaps> {
    let path = path.to_owned();
    every_millisecond(Gaps::default(), move |seen| {
        seen.polls += 1;
        if !is_missing(&path) {
            return;
        }
        // Measured without sleeping: how long until an exe is there again.
        let since = Instant::now();
        while is_missing(&path) && since.elapsed() < Duration::from_secs(5) {
            std::hint::spin_loop();
        }
        seen.gaps += 1;
        seen.longest = seen.longest.max(since.elapsed());
    })
}

/// A running image can't be replaced, only renamed: the copy is moved aside and the new one
/// put in its place at once. Between those two renames the name is free for some microseconds
/// (the design accepts a hook that fails to start there), so a poll can land in it; what must
/// never happen is a gap the length of a copy or a pass, which is what the limit tells apart.
const LONGEST_GAP: Duration = Duration::from_millis(100);

#[test]
fn a_running_hook_copy_is_replaced() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(".claude", Some(&original));
    let hooks_dir = dir.join(HOOKS_DIR_NAME);
    let copy = hook_copy_path(&dir);

    // The hook exe is this test binary: something that runs and holds still.
    fs::copy(std::env::current_exe().unwrap(), &fx.source).unwrap();
    let first_version = fs::read(&fx.source).unwrap();
    let installed = fx.install(&dir);
    assert_eq!(installed.result, Ok(InstallChange::Written));
    assert_eq!(fs::read(&copy).unwrap(), first_version);
    let entry = installed.entry.expect("an entry for hook-install.json");

    // Claude Code runs the installed copy, and it is still running.
    let ready = fx.root.join("hook-is-running");
    let mut running = Running(
        Command::new(&copy)
            .args(["--exact", HELD_TEST, "--ignored", "--nocapture"])
            .env(HOLD_ENV, &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the installed copy starts"),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while is_missing(&ready) {
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "the hook copy exited before it was up"
        );
        assert!(Instant::now() < deadline, "the hook copy never came up");
        thread::sleep(Duration::from_millis(10));
    }

    // The app was updated: a different exe beside it (the same image with a tail).
    let mut second_version = first_version.clone();
    second_version.extend_from_slice(b"agentnotch: a newer build");
    fs::write(&fx.source, &second_version).unwrap();

    let watch = watch_for_gaps(&copy);
    let updated = fx.install(&dir);
    let seen = watch.finish();

    // settings.json already names the copy: only the exe changed.
    assert_eq!(updated.result, Ok(InstallChange::Unchanged));
    assert_eq!(fs::read(&copy).unwrap(), second_version);
    assert!(
        running.0.try_wait().unwrap().is_none(),
        "the running hook was disturbed"
    );
    // The one that is running was moved aside, whole.
    let asides = copy_leftovers(&dir);
    assert_eq!(asides.len(), 1, "{asides:?}");
    assert!(
        asides[0].starts_with("agentnotch-hook.old-"),
        "{}",
        asides[0]
    );
    assert_eq!(fs::read(hooks_dir.join(&asides[0])).unwrap(), first_version);
    assert!(seen.polls > 1);
    assert!(
        seen.longest < LONGEST_GAP,
        "no hook exe for {:?} ({} gaps in {} polls)",
        seen.longest,
        seen.gaps,
        seen.polls
    );
    println!(
        "the hook copy's name was seen free {} times in {} polls, at most {:?}",
        seen.gaps, seen.polls, seen.longest
    );

    // A stage an interrupted pass left goes with the next pass, whatever is running.
    let stale_stage = hooks_dir.join(marked_name(
        STAGE_MARK,
        fx.clock.now() - Duration::from_secs(3600),
    ));
    fs::write(&stale_stage, b"half a copy").unwrap();
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Unchanged));
    assert!(is_missing(&stale_stage));
    assert_eq!(fs::read(&copy).unwrap(), second_version);

    // Once the hook has ended, the copy moved aside goes with the next pass too. (The OS can
    // keep an image a moment after its process is gone, so "next" is given a few tries.)
    drop(running);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert_eq!(fx.install(&dir).result, Ok(InstallChange::Unchanged));
        let left = copy_leftovers(&dir);
        if left.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "never swept: {left:?}");
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(fs::read(&copy).unwrap(), second_version);

    // Uninstall sweeps them as well, and leaves nothing of the folder it made.
    let at = fx.clock.now() - Duration::from_secs(60);
    for mark in [STAGE_MARK, ASIDE_MARK] {
        fs::write(hooks_dir.join(marked_name(mark, at)), b"left behind").unwrap();
    }
    assert_eq!(copy_leftovers(&dir).len(), 2);
    assert_eq!(fx.uninstall(entry).result, Ok(InstallChange::Removed));
    assert!(is_missing(&hooks_dir), "{:?}", names(&hooks_dir));
    assert_eq!(fs::read(settings(&dir)).unwrap(), original);
}

// ---- Faults around the replace ----

#[test]
fn a_short_hold_on_settings_json_is_waited_out() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(".claude", Some(&original));
    let (files, held) = holding(Duration::from_millis(300));

    let reader = read_every_millisecond(&settings(&dir));
    let outcome = fx.install_with(&dir, &files);
    let (holder, releasing) = held.lock().unwrap().take().expect("the file was held");
    // The replace can't go through a file held without delete sharing, so it went through
    // only after the holder let go.
    let released_first = releasing.load(Ordering::SeqCst);
    holder.join().unwrap();
    let reads = reader.finish();

    assert_eq!(outcome.result, Ok(InstallChange::Written));
    assert!(released_first, "the replace went through a held file");
    // One write that waited, not a second plan.
    assert_eq!(files.settings_writes(), 1);
    let written = fs::read(settings(&dir)).unwrap();
    let (hooks, _) = spliced(&written);
    assert_hooked(&hooks, &hook_command(&dir));
    assert!(stages(&dir).is_empty(), "{:?}", stages(&dir));

    // A reader never found the file missing, and saw the old bytes, then the new ones, and
    // nothing else.
    assert_eq!(reads.missing, 0, "settings.json was missing");
    assert!(reads.good >= 10, "only {} reads", reads.good);
    assert!(
        reads.contents == [original.clone(), written.clone()],
        "{} contents were seen",
        reads.contents.len()
    );
    if !reads.failed.is_empty() {
        println!("reads that failed otherwise: {:?}", reads.failed);
    }
}

#[test]
fn a_long_hold_on_settings_json_leaves_it_as_it_was() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(".claude", Some(&original));
    let before = fx.files.identity(&settings(&dir)).unwrap();
    let (files, held) = holding(Duration::from_secs(2));

    let reader = read_every_millisecond(&settings(&dir));
    let started = Instant::now();
    let outcome = fx.install_with(&dir, &files);
    let waited = started.elapsed();
    let (holder, releasing) = held.lock().unwrap().take().expect("the file was held");
    let still_held = !releasing.load(Ordering::SeqCst);

    // Given up, with the reason, after every retry, while the file was still held.
    let error = outcome.result.expect_err("the file was held throughout");
    assert!(
        error.starts_with("Couldn't write settings.json: "),
        "{error}"
    );
    assert!(still_held, "gave up only after the holder let go");
    assert!(
        waited >= retry_delays().iter().sum::<Duration>(),
        "{waited:?}"
    );
    assert_eq!(files.settings_writes(), 1);
    assert_eq!(outcome.entry, None);
    // The file is exactly the one it was, and no stage is left beside it.
    assert_eq!(fs::read(settings(&dir)).unwrap(), original);
    assert_eq!(fx.files.identity(&settings(&dir)).unwrap(), before);
    assert!(stages(&dir).is_empty(), "{:?}", stages(&dir));

    holder.join().unwrap();
    let reads = reader.finish();
    assert_eq!(reads.missing, 0, "settings.json was missing");
    assert!(reads.good >= 10, "only {} reads", reads.good);
    assert!(
        reads.contents == [original.clone()],
        "{} contents were seen",
        reads.contents.len()
    );
    if !reads.failed.is_empty() {
        println!("reads that failed otherwise: {:?}", reads.failed);
    }
    assert_eq!(fs::read(settings(&dir)).unwrap(), original);

    // The next pass, with the file free, goes through.
    let next = fx.install(&dir);
    assert_eq!(next.result, Ok(InstallChange::Written));
    let (hooks, _) = spliced(&fs::read(settings(&dir)).unwrap());
    assert_hooked(&hooks, &hook_command(&dir));
}

#[test]
fn a_settings_json_deleted_after_the_plan_is_not_written() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(".claude", Some(&original));
    // Deleted after it was read and planned from, before the replace.
    let files = Hooked::new(|path| fs::remove_file(path).unwrap());

    let outcome = fx.install_with(&dir, &files);

    assert_eq!(
        outcome.result,
        Ok(InstallChange::Aborted(VANISHED.to_owned()))
    );
    assert_eq!(outcome.entry, None);
    // Stopped there: no second plan, and above all no file made of `{}` and our hooks where
    // the user's settings were.
    assert_eq!(files.settings_writes(), 1);
    assert!(is_missing(&settings(&dir)), "a settings.json was written");
    assert!(stages(&dir).is_empty(), "{:?}", stages(&dir));
    // What was read is still held, for whoever wants it back.
    let kept_original = dir.join(ORIGINAL_BACKUP_NAME);
    assert_eq!(fs::read(&kept_original).unwrap(), original);
    assert!(fx.files.is_private(&kept_original).unwrap());
}

#[test]
fn a_settings_json_saved_after_the_plan_is_planned_again() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(".claude", Some(&original));
    // Claude Code saves a change after the file was read, before the replace.
    let theirs = String::from_utf8(original.clone())
        .unwrap()
        .replace("\"opus\"", "\"sonnet-with-a-longer-name\"")
        .into_bytes();
    assert_ne!(theirs, original);
    let files = {
        let theirs = theirs.clone();
        let mut saved = false;
        Hooked::new(move |path| {
            if !saved {
                saved = true;
                fs::write(path, &theirs).unwrap();
            }
        })
    };

    let outcome = fx.install_with(&dir, &files);

    assert_eq!(outcome.result, Ok(InstallChange::Written));
    // The first write was refused as stale; the second was planned from their version.
    assert_eq!(files.settings_writes(), 2);
    let written = fs::read(settings(&dir)).unwrap();
    let text = String::from_utf8(written.clone()).unwrap();
    assert!(text.contains("\"sonnet-with-a-longer-name\""), "{text}");
    assert!(written.starts_with(&BOM));
    assert_hooked(&hooks_of(&written), &hook_command(&dir));
    assert!(stages(&dir).is_empty(), "{:?}", stages(&dir));
}

// ---- A linked settings.json ----

#[test]
fn a_linked_settings_json_is_written_through_to_its_target() {
    let fx = fixture();
    let original = original();
    // settings.json lives in a dotfiles folder; the config folder holds a link to it.
    let real_dir = fx.root.join("dotfiles").join("claude");
    fs::create_dir_all(&real_dir).unwrap();
    let target = real_dir.join("settings.json");
    fs::write(&target, &original).unwrap();
    let dir = fx.config_dir(".claude", None);
    let link = settings(&dir);
    if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
        println!("skipped: a file symlink can't be created here ({error})");
        return;
    }
    assert!(fx.files.is_reparse(&link).unwrap());

    // The target is held for a moment so that the stage stays long enough to be seen.
    let (files, held) = holding(Duration::from_millis(300));
    let watch = {
        let (dir, real_dir) = (dir.clone(), real_dir.clone());
        every_millisecond(
            (Vec::<String>::new(), Vec::<String>::new()),
            move |(beside_link, beside_target)| {
                beside_link.extend(stages(&dir));
                for name in stages(&real_dir) {
                    if !beside_target.contains(&name) {
                        beside_target.push(name);
                    }
                }
            },
        )
    };
    let outcome = fx.install_with(&dir, &files);
    let (holder, _) = held.lock().unwrap().take().expect("the target was held");
    holder.join().unwrap();
    let (beside_link, beside_target) = watch.finish();

    assert_eq!(outcome.result, Ok(InstallChange::Written));
    // The replace was asked for the target, and staged beside it; nothing was ever staged
    // beside the link.
    assert_eq!(outcome.settings_path, target);
    assert_eq!(
        files.written_paths.lock().unwrap().as_slice(),
        std::slice::from_ref(&target)
    );
    assert!(beside_link.is_empty(), "{beside_link:?}");
    assert!(
        beside_target.iter().any(|name| is_settings_stage(name)),
        "{beside_target:?}"
    );
    // The link is still a link, and the target has our hooks.
    assert!(fx.files.is_reparse(&link).unwrap());
    assert!(!fx.files.is_reparse(&target).unwrap());
    let written = fs::read(&target).unwrap();
    assert_eq!(fs::read(&link).unwrap(), written);
    let (hooks, _) = spliced(&written);
    assert_hooked(&hooks, &hook_command(&dir));
    // The backups sit beside the real file; the copy in the folder Claude Code runs in.
    assert!(our_backups(&dir).is_empty());
    assert_eq!(our_backups(&real_dir).len(), 1);
    assert_eq!(
        fs::read(real_dir.join(ORIGINAL_BACKUP_NAME)).unwrap(),
        original
    );
    assert!(fx
        .files
        .is_private(&real_dir.join(ORIGINAL_BACKUP_NAME))
        .unwrap());
    assert_eq!(fs::read(hook_copy_path(&dir)).unwrap(), HOOK_BYTES);
    assert!(stages(&real_dir).is_empty() && stages(&dir).is_empty());

    // Uninstall goes through the link too, and gives the target back to the byte.
    let entry = outcome.entry.expect("an entry for hook-install.json");
    assert_eq!(entry.settings_path, target);
    assert_eq!(fx.uninstall(entry).result, Ok(InstallChange::Removed));
    assert_eq!(fs::read(&target).unwrap(), original);
    assert!(fx.files.is_reparse(&link).unwrap());
    assert_eq!(fs::read(&link).unwrap(), original);
    assert!(is_missing(&dir.join(HOOKS_DIR_NAME)));
}

// ---- A profile path no string command can carry ----

#[test]
fn a_profile_path_with_a_space_is_hooked_by_its_short_name() {
    let fx = fixture();
    let original = original();
    let dir = fx.config_dir(Path::new("Jo Smith").join(".claude"), Some(&original));
    let copy = hook_copy_path(&dir);
    // From the path alone there is no command: a space can't be written unquoted.
    assert_eq!(
        fx.plan(&dir).form,
        CommandForm::NotPossible(NOT_POSSIBLE.to_owned())
    );
    let short_path = |path: &Path| fx.files.short_path(path);
    let expected = string_command_for(&copy, Subcommand::Hook, &short_path);

    let outcome = fx.install(&dir);

    let Some(expected) = expected else {
        // 8.3 names are off on this volume: the folder can't be hooked, and isn't touched.
        println!("skipped: this volume has no 8.3 names");
        assert_eq!(outcome.result, Err(NOT_POSSIBLE.to_owned()));
        assert_eq!(fs::read(settings(&dir)).unwrap(), original);
        assert!(is_missing(&dir.join(HOOKS_DIR_NAME)));
        return;
    };

    assert_eq!(outcome.result, Ok(InstallChange::Written));
    // The command spells the folder in 8.3 names, and still names the copy.
    let exe = expected.strip_suffix(" hook").expect("the hook command");
    assert!(!exe.contains(' ') && exe.contains('~'), "{exe}");
    assert_eq!(
        fx.files.identity(Path::new(exe)).unwrap(),
        fx.files.identity(&copy).unwrap()
    );
    let entry = outcome.entry.expect("an entry for hook-install.json");
    assert_eq!(entry.form, "string");
    assert_eq!(entry.command, expected);
    assert!(entry.status_line);

    let written = fs::read(settings(&dir)).unwrap();
    let (hooks, status_line) = spliced(&written);
    assert_hooked(&hooks, &expected);
    assert_eq!(
        status_line.get("command").and_then(Json::as_str),
        string_command_for(&copy, Subcommand::StatusLine, &short_path).as_deref()
    );

    // Entries spelled that way are known as ours: read back, left alone, and removed.
    let status = read_status(&dir, &fx.files, &SETUP);
    assert!(status.hooks_registered && status.hooks_installed);
    assert!(status.status_line_installed);
    assert_eq!(fx.install(&dir).result, Ok(InstallChange::Unchanged));
    assert_eq!(fs::read(settings(&dir)).unwrap(), written);
    assert_eq!(fx.uninstall(entry).result, Ok(InstallChange::Removed));
    assert_eq!(fs::read(settings(&dir)).unwrap(), original);
    assert!(is_missing(&dir.join(HOOKS_DIR_NAME)));
}
