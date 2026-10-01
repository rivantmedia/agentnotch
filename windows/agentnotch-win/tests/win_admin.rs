//! The pipe's checks where an administrator is needed to set them up (DESIGN-WIN §7.3, §1.4,
//! R17, F3, F11): the hook and the app's server at different integrity levels, both ways, and a
//! pipe name taken first by another local user.
//!
//! - **Integrity levels.** The real server (`pipe-test-server.exe`) at medium integrity answers a
//!   PermissionRequest from the hook at high integrity, and the server at high answers the hook at
//!   medium; the hook prints the answer exactly as the Mac hook did (`v1-responses/allow`). The
//!   high side is this test process itself (the runner is elevated), the medium side a child
//!   started with a Safer normal-user token lowered to Medium (`integrity::spawn_medium`).
//! - **Squatter.** A second local user (created here, random name with a fixed prefix and a random
//!   password that is never printed) runs `pipe-squatter.exe`, which creates the pipe's name
//!   first with an Everyone DACL. The hook connects, finds that the owner is not its user, and
//!   leaves without writing (the squatter records zero bytes, the hook's trace says `pipe is not
//!   ours`); the app's server, started on that name, reports it in use.
//!
//! These tests create a Windows account and change who a process runs as, so they run only with
//! `AGENTNOTCH_CI_ADMIN=1` (the Windows workflow sets it on its throwaway runner). Without it a
//! developer's machine is left alone with a printed notice; on CI (`CI=true`) that, or a runner
//! that is not elevated, is a failure, so CI can never skip them silently. The account, its
//! profile and the folder given to it are removed on every path (guards), and leftovers of an
//! earlier run that was killed are swept first. Every pipe name is the test's own
//! (`AGENTNOTCH_DEV=1` + `AGENTNOTCH_SOCKET` on the hook), never this user's real one.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::collections::hash_map::RandomState;
use std::ffi::c_void;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agentnotch_engine::platform::{HookTransport, TransportEvent};
use agentnotch_proto::ControlOp;
use agentnotch_win::integrity::{self, Level, MediumChild, Piped};
use agentnotch_win::pipe_server::client;
use agentnotch_win::pipe_server::squatter::{self, Entry};
use agentnotch_win::pipe_server::{PipeServer, PIPE_IN_USE};
use agentnotch_win::sid;
use serde_json::Value;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, WAIT_OBJECT_0};
use windows::Win32::NetworkManagement::NetManagement::{
    NERR_Success, NetApiBufferFree, NetUserAdd, NetUserDel, NetUserEnum, FILTER_NORMAL_ACCOUNT,
    MAX_PREFERRED_LENGTH, UF_DONT_EXPIRE_PASSWD, UF_SCRIPT, USER_INFO_0, USER_INFO_1,
    USER_PRIV_USER,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    LookupAccountNameW, SetFileSecurityW, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    CreateProcessWithLogonW, GetExitCodeProcess, OpenProcessToken, TerminateProcess,
    WaitForSingleObject, CREATE_NEW_CONSOLE, CREATE_PROCESS_LOGON_FLAGS, PROCESS_INFORMATION,
    STARTF_USESHOWWINDOW, STARTUPINFOW,
};
use windows::Win32::UI::Shell::DeleteProfileW;

const SERVER: &str = env!("CARGO_BIN_EXE_pipe-test-server");
const SQUATTER: &str = env!("CARGO_BIN_EXE_pipe-squatter");

/// What the hooks here are told their session's config folder is: forwarded, never opened.
const CONFIG_DIR: &str = r"C:\Users\me\.claude-work";
/// Every account these tests create starts with this; only such accounts are ever deleted.
const USER_PREFIX: &str = "anwinadm";
/// The folder given to the second user is `%PUBLIC%\<this><pid>`.
const FOLDER_PREFIX: &str = "agentnotch-win-admin-";

/// A hook, a server start or a squatter taking longer than this has hung.
const WAIT: Duration = Duration::from_secs(20);
/// No test here needs longer; one that does is stuck.
const TEST_LIMIT: Duration = Duration::from_secs(150);

// ---- one test at a time, each with a limit, and the gate ----

static SERIAL: Mutex<()> = Mutex::new(());

struct TestGuard {
    _serial: MutexGuard<'static, ()>,
    _alive: mpsc::Sender<()>,
}

/// First line of every test: one at a time (they spawn processes and create accounts), and a
/// test still running after the limit ends the run with a failure instead of stalling CI.
fn begin(name: &'static str) -> TestGuard {
    let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (alive, ended) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        if ended.recv_timeout(TEST_LIMIT) == Err(RecvTimeoutError::Timeout) {
            eprintln!("{name}: still running after {TEST_LIMIT:?}; ending the test run");
            std::process::exit(101);
        }
    });
    TestGuard {
        _serial: serial,
        _alive: alive,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Gate {
    Run,
    Skip,
    Fail(&'static str),
}

/// Whether the admin tests run, from `AGENTNOTCH_CI_ADMIN` and `CI` (DESIGN-WIN §6.1 item 9).
fn gate(admin: Option<&str>, ci: Option<&str>) -> Gate {
    let admin = admin == Some("1");
    let ci = ci.is_some_and(|ci| ci.eq_ignore_ascii_case("true"));
    match (admin, ci) {
        (true, _) => Gate::Run,
        (false, false) => Gate::Skip,
        (false, true) => Gate::Fail(
            "CI=true but AGENTNOTCH_CI_ADMIN is not 1: the admin tests may not be skipped on CI",
        ),
    }
}

/// Call right after [`begin`]: false = skip (a notice is printed); a run that must not skip and
/// can't run fails here, saying why.
fn admin_run(name: &str) -> bool {
    let admin = std::env::var("AGENTNOTCH_CI_ADMIN").ok();
    let ci = std::env::var("CI").ok();
    match gate(admin.as_deref(), ci.as_deref()) {
        Gate::Skip => {
            eprintln!(
                "{name}: NOTICE: skipped; set AGENTNOTCH_CI_ADMIN=1 in an elevated shell on a \
                 throwaway machine to run it (it creates a local account)"
            );
            false
        }
        Gate::Fail(why) => panic!("{name}: {why}"),
        Gate::Run => {
            assert!(
                integrity::is_elevated(),
                "{name}: this process is not elevated (integrity {:?}); the admin tests need an \
                 elevated runner",
                integrity::integrity_level()
            );
            let level = integrity::integrity_level();
            assert_eq!(
                level,
                Some(Level::High),
                "{name}: an elevated process should run at High integrity, this one is {level:?}"
            );
            true
        }
    }
}

// ---- names, fixtures, exes ----

fn unique_pipe(what: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\agentnotch-test-{}-{}-{what}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agentnotch-proto/tests/fixtures")
        .join(relative);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `agentnotch-hook.exe`, which `cargo test` of the fork crates builds into the same profile
/// folder as this test (`target\<profile>\deps\win_admin-….exe`).
fn hook_exe() -> PathBuf {
    let exe = std::env::current_exe().expect("the test's own path");
    let hook = exe
        .parent()
        .and_then(Path::parent)
        .map(|profile| profile.join("agentnotch-hook.exe"))
        .expect("the test runs from target\\<profile>\\deps");
    assert!(
        hook.is_file(),
        "{} is missing: win_admin.rs runs the real hook exe, which `cargo test` builds only \
         when agentnotch-hook is among the crates tested (CI tests all five fork crates in one \
         run; locally, `cargo build -p agentnotch-hook` first)",
        hook.display()
    );
    hook
}

/// The hook's environment: this process's, minus anything Claude Code or a developer's shell
/// could have left that changes what the hook does, plus a Claude session of the test's own on
/// the test's pipe.
fn hook_environment(pipe: &str, trace: Option<&Path>) -> Vec<(String, String)> {
    const CLEARED: [&str; 13] = [
        "CLAUDE_PID",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_ENTRYPOINT",
        "AGENTNOTCH_DEV",
        "AGENTNOTCH_HOOK_TRACE",
        "AGENTNOTCH_HOOK_TEST_PANIC",
        "AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS",
        "AGENTNOTCH_HOOK_TEST_PROGRAM_FILES",
        "AGENTNOTCH_STATUSLINE_DEPTH",
        "AGENTNOTCH_SOCKET",
        "WT_SESSION",
        "TERM_PROGRAM",
    ];
    let mut environment: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        // `=C:` and the like are cmd's per-drive folders, not variables a child needs.
        .filter(|(name, _)| {
            !name.starts_with('=')
                && !CLEARED
                    .iter()
                    .any(|cleared| cleared.eq_ignore_ascii_case(name))
        })
        .collect();
    environment.extend([
        ("AGENTNOTCH_DEV".to_owned(), "1".to_owned()),
        ("AGENTNOTCH_SOCKET".to_owned(), pipe.to_owned()),
        ("CLAUDE_PID".to_owned(), std::process::id().to_string()),
        ("CLAUDE_CONFIG_DIR".to_owned(), CONFIG_DIR.to_owned()),
        ("CLAUDE_CODE_SESSION_ATTENDED".to_owned(), "1".to_owned()),
        ("CLAUDE_CODE_ENTRYPOINT".to_owned(), "cli".to_owned()),
    ]);
    if let Some(trace) = trace {
        environment.push((
            "AGENTNOTCH_HOOK_TRACE".to_owned(),
            trace.to_str().expect("a Unicode temp path").to_owned(),
        ));
    }
    environment
}

/// How a hook run ended.
#[derive(Debug)]
struct Finished {
    code: Option<u32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs the hook at this process's (high) level: checks its level, then writes `stdin`, closes
/// it and waits for the end.
fn run_high_hook(pipe: &str, stdin: &[u8], trace: Option<&Path>) -> Finished {
    let mut command = Command::new(hook_exe());
    command
        .arg("hook")
        .env_clear()
        .envs(hook_environment(pipe, trace))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the hook exe starts");
    let level = integrity::process_integrity_level(HANDLE(child.as_raw_handle()));
    if level != Some(Level::High) {
        let _ = child.kill();
        panic!("the hook should run at High integrity, it runs at {level:?}");
    }
    let mut input = child.stdin.take().expect("piped stdin");
    // A hook that ended before reading everything is judged by what it printed, not here.
    let _ = input.write_all(stdin);
    drop(input);
    let (tell, done) = mpsc::channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = tell.send(child.wait_with_output());
    });
    match done.recv_timeout(WAIT) {
        Ok(Ok(output)) => Finished {
            code: output.status.code().map(|code| code as u32),
            stdout: output.stdout,
            stderr: output.stderr,
        },
        Ok(Err(error)) => panic!("the hook's output could not be read: {error}"),
        Err(_) => {
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            panic!("the hook hung");
        }
    }
}

/// Runs the hook at medium integrity: checks its level before it has its stdin (so it is
/// certainly still running), then writes it, closes it and waits for the end.
fn run_medium_hook(pipe: &str, stdin: &[u8]) -> Finished {
    let environment = hook_environment(pipe, None);
    let mut hook = integrity::spawn_medium(
        &hook_exe(),
        &["hook".to_owned()],
        Some(&environment),
        Piped {
            stdin: true,
            stdout: true,
            stderr: true,
        },
    )
    .expect("the hook starts at medium integrity");
    assert_eq!(
        hook.integrity_level(),
        Some(Level::Medium),
        "the medium hook's level"
    );
    let stdout = reader(hook.stdout.take().expect("piped stdout"));
    let stderr = reader(hook.stderr.take().expect("piped stderr"));
    let mut input = hook.stdin.take().expect("piped stdin");
    let _ = input.write_all(stdin);
    drop(input);
    let code = hook.wait(WAIT);
    assert!(code.is_some(), "the medium hook hung");
    Finished {
        code,
        stdout: stdout.recv_timeout(WAIT).expect("the hook's stdout ends"),
        stderr: stderr.recv_timeout(WAIT).expect("the hook's stderr ends"),
    }
}

/// Reads a stream to its end on a thread of its own.
fn reader(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tell, done) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = tell.send(bytes);
    });
    done
}

#[track_caller]
fn assert_allow_printed(done: &Finished, what: &str) {
    let expected = fixture("v1-responses/allow.stdout");
    assert_eq!(done.code, Some(0), "{what}: exit code ({done:?})");
    assert!(
        done.stderr.is_empty(),
        "{what}: stderr {:?}",
        String::from_utf8_lossy(&done.stderr)
    );
    assert!(
        done.stdout == expected,
        "{what}: printed {:?}, expected {:?}",
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&expected)
    );
}

// ---- the test server ----

fn server_arguments(pipe: &str, folder: &Path) -> Vec<String> {
    [
        "--pipe",
        pipe,
        "--record",
        folder.join("record.jsonl").to_str().expect("Unicode"),
        "--ready",
        folder.join("ready.txt").to_str().expect("Unicode"),
        "--answer",
        "Bash=allow",
        "--max-seconds",
        "120",
    ]
    .iter()
    .map(|argument| (*argument).to_owned())
    .collect()
}

/// Waits for `path` to exist; `alive` says whether its writer still runs (its exit code when it
/// has ended), so a writer that died is reported at once, with what `context` adds.
fn wait_for_file(
    path: &Path,
    what: &str,
    mut alive: impl FnMut() -> Option<u32>,
    context: impl Fn() -> String,
) {
    let until = Instant::now() + WAIT;
    while !path.exists() {
        if let Some(code) = alive() {
            if !path.exists() {
                panic!(
                    "{what} ended with exit code {code} before writing its ready file{}",
                    context()
                );
            }
        }
        assert!(
            Instant::now() < until,
            "{what}: no ready file after {WAIT:?}{}",
            context()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The record lines of `kind`.
fn recorded(folder: &Path, kind: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(folder.join("record.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["kind"] == kind)
        .collect()
}

#[track_caller]
fn assert_delivered_allow(folder: &Path, what: &str) {
    let held = recorded(folder, "permission_held");
    assert_eq!(held.len(), 1, "{what}: held requests {held:?}");
    assert_eq!(held[0]["tool"], "Bash", "{what}");
    assert_eq!(held[0]["answer"], "allow", "{what}");
    assert_eq!(held[0]["result"], "delivered", "{what}");
}

/// Asks the server to quit over the pipe (a control request from this, high, process).
fn quit(pipe: &str) {
    let answer = client::control(pipe, ControlOp::Quit, Duration::from_secs(5))
        .unwrap_or_else(|error| panic!("control quit: {error}"));
    assert!(answer.ok, "control quit: {answer:?}");
}

/// A server spawned with std's `Command`, killed if the test ends before it.
struct HighServer(Child);

impl Drop for HighServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// ---- the tests ----

#[test]
fn the_gate_skips_only_off_ci() {
    let _guard = begin("the_gate_skips_only_off_ci");
    assert_eq!(gate(Some("1"), Some("true")), Gate::Run);
    assert_eq!(gate(Some("1"), None), Gate::Run);
    assert_eq!(gate(None, None), Gate::Skip);
    assert_eq!(gate(Some("0"), Some("false")), Gate::Skip);
    assert!(matches!(gate(None, Some("true")), Gate::Fail(_)));
    assert!(matches!(gate(Some(""), Some("TRUE")), Gate::Fail(_)));
}

#[test]
fn a_medium_server_answers_a_high_hook() {
    let _guard = begin("a_medium_server_answers_a_high_hook");
    if !admin_run("a_medium_server_answers_a_high_hook") {
        return;
    }
    let folder = tempfile::tempdir().expect("a temp folder");
    let pipe = unique_pipe("medium-server");
    let mut server = integrity::spawn_medium(
        Path::new(SERVER),
        &server_arguments(&pipe, folder.path()),
        None,
        Piped {
            stderr: true,
            ..Piped::default()
        },
    )
    .expect("pipe-test-server starts at medium integrity");
    assert_eq!(
        server.integrity_level(),
        Some(Level::Medium),
        "the server's level"
    );
    let stderr = reader(server.stderr.take().expect("piped stderr"));
    wait_for_file(
        &folder.path().join("ready.txt"),
        "the medium server",
        || server.wait(Duration::ZERO),
        String::new,
    );

    let done = run_high_hook(&pipe, &fixture("stdin/permission_request_bash.json"), None);

    assert_allow_printed(&done, "high hook -> medium server");
    quit(&pipe);
    ended_cleanly(&server, stderr, "the medium server");
    // Read once the server has ended: the line is written after the answer went out.
    assert_delivered_allow(folder.path(), "high hook -> medium server");
}

#[test]
fn a_high_server_answers_a_medium_hook() {
    let _guard = begin("a_high_server_answers_a_medium_hook");
    if !admin_run("a_high_server_answers_a_medium_hook") {
        return;
    }
    let folder = tempfile::tempdir().expect("a temp folder");
    let pipe = unique_pipe("high-server");
    let mut server = HighServer(
        Command::new(SERVER)
            .args(server_arguments(&pipe, folder.path()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("pipe-test-server starts"),
    );
    let level = integrity::process_integrity_level(HANDLE(server.0.as_raw_handle()));
    assert_eq!(level, Some(Level::High), "the server's level");
    let stderr = reader(server.0.stderr.take().expect("piped stderr"));
    wait_for_file(
        &folder.path().join("ready.txt"),
        "the high server",
        || {
            server
                .0
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.code().unwrap_or(-1) as u32)
        },
        String::new,
    );

    let done = run_medium_hook(&pipe, &fixture("stdin/permission_request_bash.json"));

    assert_allow_printed(&done, "medium hook -> high server");
    quit(&pipe);
    let until = Instant::now() + WAIT;
    let code = loop {
        if let Some(status) = server.0.try_wait().expect("the server's state") {
            break status.code();
        }
        assert!(Instant::now() < until, "the high server did not quit");
        std::thread::sleep(Duration::from_millis(50));
    };
    let complaints = stderr.recv_timeout(WAIT).unwrap_or_default();
    assert_eq!(
        code,
        Some(0),
        "the high server's exit code; stderr {:?}",
        String::from_utf8_lossy(&complaints)
    );
    assert_delivered_allow(folder.path(), "medium hook -> high server");
}

fn ended_cleanly(server: &MediumChild, stderr: mpsc::Receiver<Vec<u8>>, what: &str) {
    let code = server.wait(WAIT);
    let complaints = stderr.recv_timeout(WAIT).unwrap_or_default();
    assert_eq!(
        code,
        Some(0),
        "{what}'s exit code; stderr {:?}",
        String::from_utf8_lossy(&complaints)
    );
}

#[test]
fn a_pipe_another_user_made_first_gets_nothing_and_is_reported_in_use() {
    let name = "a_pipe_another_user_made_first_gets_nothing_and_is_reported_in_use";
    let _guard = begin(name);
    if !admin_run(name) {
        return;
    }
    sweep_leftovers();
    // Declared in this order so they are dropped the other way round: the squatter ends before
    // its folder goes, and both before the account.
    let user = TestUser::create();
    let folder = SharedFolder::create(&user);
    let squatter_exe = folder.path().join("pipe-squatter.exe");
    std::fs::copy(SQUATTER, &squatter_exe).expect("pipe-squatter.exe is copied for the user");
    let record = folder.path().join("record.txt");
    let ready = folder.path().join("ready.txt");
    let pipe = unique_pipe("squatted");
    let squatter = user.run(
        &squatter_exe,
        &[
            "--pipe".to_owned(),
            pipe.clone(),
            "--record".to_owned(),
            record.to_str().expect("Unicode").to_owned(),
            "--ready".to_owned(),
            ready.to_str().expect("Unicode").to_owned(),
            "--max-seconds".to_owned(),
            "90".to_owned(),
        ],
        folder.path(),
    );
    // The other user's token may not let an administrator in (its default DACL is that user's
    // and SYSTEM's): then this check is skipped, and the hook's refusal below still shows that
    // the pipe's owner is somebody else.
    match squatter.user_sid() {
        Some(squatter_sid) => {
            assert_eq!(
                Some(squatter_sid.as_str()),
                user.sid.as_deref(),
                "the squatter runs as the second user"
            );
            let own_sid = sid::current_user_sid().expect("this user's SID");
            assert_ne!(squatter_sid, own_sid);
        }
        None => {
            eprintln!("{name}: NOTICE: the squatter's token can't be read; its user is not checked")
        }
    }
    // It runs as another user, whose stderr is out of reach: its failures are in its record.
    wait_for_file(
        &ready,
        "pipe-squatter",
        || squatter.exit_code(),
        || {
            format!(
                "; its record: {:?}",
                std::fs::read_to_string(&record).unwrap_or_default()
            )
        },
    );

    // The hook: connects, reads the pipe's owner, leaves.
    let trace = folder.path().join("hook.trace");
    let done = run_high_hook(
        &pipe,
        &fixture("stdin/permission_request_bash.json"),
        Some(&trace),
    );
    assert_eq!(done.code, Some(0), "the hook's exit code ({done:?})");
    assert!(done.stdout.is_empty(), "the hook printed {done:?}");
    assert!(done.stderr.is_empty(), "the hook wrote to stderr: {done:?}");
    let traced = std::fs::read_to_string(&trace).unwrap_or_default();
    assert!(
        traced
            .lines()
            .any(|line| line.ends_with(" pipe is not ours")),
        "the hook's trace: {traced:?}"
    );

    // What the squatter received: one connection, no byte.
    let until = Instant::now() + WAIT;
    let entries = loop {
        let text = std::fs::read_to_string(&record).unwrap_or_default();
        let entries = squatter::read_record(&text);
        if !entries.is_empty() {
            break entries;
        }
        assert!(
            Instant::now() < until,
            "the squatter recorded no connection"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(entries, vec![Entry::Connection(0)], "the squatter's record");

    // The app's server on the same name: the name is somebody else's.
    let server = PipeServer::new();
    let (sink, events) = crossbeam_channel::unbounded();
    server
        .start(&pipe, sink)
        .expect("the server's thread starts");
    let event = events.recv_timeout(WAIT);
    server.stop();
    match event {
        Ok(TransportEvent::Error(why)) => assert_eq!(why, PIPE_IN_USE),
        other => panic!("the server should report the name in use, it reported {other:?}"),
    }

    // Nothing else reached the squatter, and it is still there (it wasn't gone all along).
    let text = std::fs::read_to_string(&record).unwrap_or_default();
    assert_eq!(squatter::read_record(&text), vec![Entry::Connection(0)]);
    assert_eq!(squatter.exit_code(), None, "the squatter ended early");
}

// ---- the second user ----

/// Random hex from the OS's generator (std's hash keys come from it); `words` x 16 digits.
fn random_hex(words: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    (0..words)
        .map(|word| {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_usize(word);
            hasher.write_u128(nanos);
            format!("{:016x}", hasher.finish())
        })
        .collect()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A local account made for one test; deleted, with its profile, when dropped.
struct TestUser {
    name: String,
    /// Never printed: `Debug` is not derived.
    password: String,
    sid: Option<String>,
}

impl TestUser {
    fn create() -> TestUser {
        // 16 characters: account names may have 20.
        let name = format!("{USER_PREFIX}{}", &random_hex(1)[..8]);
        // Upper, lower, digit and symbol, and nothing of the name: any complexity policy.
        let password = format!("Aa1!{}", random_hex(2));
        let mut wide_name = wide(&name);
        let mut wide_password = wide(&password);
        let info = USER_INFO_1 {
            usri1_name: PWSTR(wide_name.as_mut_ptr()),
            usri1_password: PWSTR(wide_password.as_mut_ptr()),
            usri1_password_age: 0,
            usri1_priv: USER_PRIV_USER,
            usri1_home_dir: PWSTR::null(),
            usri1_comment: PWSTR::null(),
            usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD,
            usri1_script_path: PWSTR::null(),
        };
        let mut bad_field = 0u32;
        // SAFETY: `info` and the strings it points to are alive for the call.
        let status = unsafe {
            NetUserAdd(
                PCWSTR::null(),
                1,
                (&info as *const USER_INFO_1).cast(),
                Some(&mut bad_field),
            )
        };
        assert_eq!(
            status, NERR_Success,
            "NetUserAdd failed with {status} (field {bad_field})"
        );
        let user = TestUser {
            sid: account_sid(&name),
            name,
            password,
        };
        assert!(user.sid.is_some(), "the new account's SID can't be read");
        user
    }

    /// Runs `program` as this user (`CreateProcessWithLogonW`, no profile loaded), in a hidden
    /// console of its own, in `folder`.
    fn run(&self, program: &Path, arguments: &[String], folder: &Path) -> LogonChild {
        let line = integrity::command_line(program.to_str().expect("Unicode"), arguments);
        let mut line = wide(&line);
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: 0, // SW_HIDE
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        let (name, domain, password) = (
            HSTRING::from(self.name.as_str()),
            HSTRING::from("."),
            HSTRING::from(self.password.as_str()),
        );
        let folder = HSTRING::from(folder.as_os_str());
        // SAFETY: every string is NUL-terminated and alive for the call; the command line is
        // writable; `startup` and `info` are valid for their sizes.
        unsafe {
            CreateProcessWithLogonW(
                &name,
                &domain,
                &password,
                CREATE_PROCESS_LOGON_FLAGS(0),
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                CREATE_NEW_CONSOLE,
                None,
                &folder,
                &startup,
                &mut info,
            )
        }
        .unwrap_or_else(|error| panic!("CreateProcessWithLogonW: {}", error.message()));
        // SAFETY: returned open; the thread handle is not needed.
        let _ = unsafe { CloseHandle(info.hThread) };
        LogonChild(info.hProcess)
    }
}

impl Drop for TestUser {
    fn drop(&mut self) {
        delete_account(&self.name, self.sid.as_deref());
    }
}

/// Deletes an account this file made, and its profile if one was made (none should be: nothing
/// here loads it).
fn delete_account(name: &str, sid: Option<&str>) {
    // Never another account, whatever the caller passed (a panic here could be mid-unwind).
    if !name.starts_with(USER_PREFIX) {
        return;
    }
    if let Some(sid) = sid {
        let sid = HSTRING::from(sid);
        // SAFETY: a NUL-terminated SID string alive for the call. A missing profile is an error
        // that means there is nothing to delete.
        let _ = unsafe { DeleteProfileW(&sid, PCWSTR::null(), PCWSTR::null()) };
    }
    let name_w = HSTRING::from(name);
    // SAFETY: a NUL-terminated name alive for the call.
    let status = unsafe { NetUserDel(PCWSTR::null(), &name_w) };
    if status != NERR_Success {
        eprintln!("win_admin: the test account {name} could not be deleted ({status})");
    }
}

/// `S-1-5-21-…` of a local account.
fn account_sid(name: &str) -> Option<String> {
    let mut sid_buffer = [0u64; 9]; // SECURITY_MAX_SID_SIZE (68 bytes), aligned
    let mut sid_size = std::mem::size_of_val(&sid_buffer) as u32;
    let mut domain = [0u16; 256];
    let mut domain_size = domain.len() as u32;
    let mut kind = SID_NAME_USE::default();
    let sid = PSID(sid_buffer.as_mut_ptr().cast());
    let name = HSTRING::from(name);
    // SAFETY: every buffer is alive and its size is given.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &name,
            Some(sid),
            &mut sid_size,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut kind,
        )
    }
    .ok()?;
    sid_text(sid)
}

fn sid_text(sid: PSID) -> Option<String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` is a valid SID; `text` receives a LocalAlloc'd string.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.ok()?;
    // SAFETY: the NUL-terminated string just returned.
    let result = unsafe { text.to_string() }.ok();
    // SAFETY: allocated by the call above, not used after this.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    result
}

/// Every account and folder an earlier run of these tests left behind (killed before its
/// guards ran): accounts named `anwinadm…` and `%PUBLIC%\agentnotch-win-admin-*`.
fn sweep_leftovers() {
    for name in local_accounts() {
        if name.starts_with(USER_PREFIX) {
            eprintln!("win_admin: deleting the leftover test account {name}");
            let sid = account_sid(&name);
            delete_account(&name, sid.as_deref());
        }
    }
    if let Ok(entries) = std::fs::read_dir(public_folder()) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(FOLDER_PREFIX))
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn local_accounts() -> Vec<String> {
    const ERROR_MORE_DATA: u32 = 234;
    let mut names = Vec::new();
    let mut resume = 0u32;
    loop {
        let mut buffer: *mut u8 = std::ptr::null_mut();
        let (mut read, mut total) = (0u32, 0u32);
        // SAFETY: valid out pointers; the buffer is freed below.
        let status = unsafe {
            NetUserEnum(
                PCWSTR::null(),
                0,
                FILTER_NORMAL_ACCOUNT,
                &mut buffer,
                MAX_PREFERRED_LENGTH,
                &mut read,
                &mut total,
                Some(&mut resume),
            )
        };
        if !buffer.is_null() {
            // SAFETY: the call returned `read` USER_INFO_0 entries in `buffer`.
            let entries =
                unsafe { std::slice::from_raw_parts(buffer.cast::<USER_INFO_0>(), read as usize) };
            for entry in entries {
                // SAFETY: each name is a NUL-terminated string inside `buffer`.
                if let Ok(name) = unsafe { entry.usri0_name.to_string() } {
                    names.push(name);
                }
            }
            // SAFETY: allocated by NetUserEnum, not used after this.
            unsafe { NetApiBufferFree(Some(buffer.cast::<c_void>().cast_const())) };
        }
        if status != ERROR_MORE_DATA {
            break;
        }
    }
    names
}

fn public_folder() -> PathBuf {
    std::env::var_os("PUBLIC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"))
}

/// A folder the second user may read and write (and nobody else but SYSTEM, the administrators
/// and this user); removed when dropped.
struct SharedFolder(PathBuf);

impl SharedFolder {
    fn create(user: &TestUser) -> SharedFolder {
        let path = public_folder().join(format!("{FOLDER_PREFIX}{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the shared folder is created");
        let folder = SharedFolder(path);
        let own = sid::current_user_sid().expect("this user's SID");
        let other = user.sid.as_deref().expect("the account's SID");
        // Inherited by what is put inside: the exe copy, the record, the ready file.
        let sddl =
            format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{own})(A;OICI;FA;;;{other})");
        let descriptor = sid::SecurityDescriptor::from_sddl(&sddl).expect("the folder's SDDL");
        let path_w = HSTRING::from(folder.0.as_os_str());
        // SAFETY: the path and the descriptor are alive for the call.
        let set = unsafe {
            SetFileSecurityW(
                &path_w,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                PSECURITY_DESCRIPTOR(descriptor.as_ptr()),
            )
        };
        assert!(
            set.as_bool(),
            "SetFileSecurityW on the shared folder failed"
        );
        folder
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SharedFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A process started as the second user; ended when dropped.
struct LogonChild(HANDLE);

impl LogonChild {
    /// Its exit code once it has ended; `None` while it runs.
    fn exit_code(&self) -> Option<u32> {
        // SAFETY: the process handle is open while `self` lives.
        if unsafe { WaitForSingleObject(self.0, 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: as above; a valid out pointer.
        unsafe { GetExitCodeProcess(self.0, &mut code) }.ok()?;
        Some(code)
    }

    /// The SID of the user it runs as.
    fn user_sid(&self) -> Option<String> {
        let mut token = HANDLE::default();
        // SAFETY: the process handle has full access; `token` is closed below.
        unsafe { OpenProcessToken(self.0, TOKEN_QUERY, &mut token) }.ok()?;
        let sid = sid::sid_of_token(token);
        // SAFETY: opened above, not used after this.
        let _ = unsafe { CloseHandle(token) };
        sid
    }
}

impl Drop for LogonChild {
    fn drop(&mut self) {
        // SAFETY: the handle is open (CreateProcessWithLogonW returns it with full access) and
        // is not used after this.
        unsafe {
            let _ = TerminateProcess(self.0, 1);
            let _ = WaitForSingleObject(self.0, 5_000);
            let _ = CloseHandle(self.0);
        }
    }
}
