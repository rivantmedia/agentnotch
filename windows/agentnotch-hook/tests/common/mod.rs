//! What the Windows test files share (`mod common;` in a file that starts with `#![cfg(windows)]`):
//!
//! - one test at a time, each with a hard limit, so a hang fails the run instead of stalling it;
//! - the hook exe, spawned with piped stdio and none of the environment a developer's shell or
//!   the CI runner may carry, talking to a pipe name of the test's own (`AGENTNOTCH_DEV=1` +
//!   `AGENTNOTCH_SOCKET`, set on the child only: never on this process, whose tests share it);
//! - the app's side in-process: the real `PipeServer` feeding the engine's real `HookIngress`,
//!   stepped by the test from the transport's channel;
//! - the string commands the installer writes and the two shells that run them, and temporary
//!   folders for copies of the exes;
//! - raw ends of a pipe for what neither of those does: a client that says nothing, a server that
//!   never reads, the security descriptor as a client sees it.
//!
//! No test touches this user's real pipe, a real terminal or a real Claude Code folder.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr
)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::{addr_of_mut, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use agentnotch_engine::ingress::HookIngress;
use agentnotch_engine::model::{HeldPermission, HookEvent};
use agentnotch_engine::platform::{IncomingFrame, TransportEvent};
use agentnotch_engine::runtime_types::{IngressConfig, IngressOut};
use agentnotch_proto::limits::PIPE_BUFFER_BYTES;
use agentnotch_proto::{pipe_sddl, HookEnv, PipeAce, PipeSecurity};
use agentnotch_win::pipe_server::PipeServer;
use agentnotch_win::sid::SecurityDescriptor;
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_PIPE_BUSY, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetShortPathNameW, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, CREATE_NO_WINDOW, PROCESS_TERMINATE,
};

pub const EXE: &str = env!("CARGO_BIN_EXE_agentnotch-hook");

/// A hook run that takes longer than this has hung; the exe's own bound is 1.2 s, and a
/// PermissionRequest in these tests is answered within seconds.
pub const HARD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a test waits for something the server or the engine is about to report.
pub const WAIT: Duration = Duration::from_secs(10);
/// No test needs longer than this; one that does is stuck.
const TEST_LIMIT: Duration = Duration::from_secs(90);

/// What the hooks in these tests are told their session's config folder is. Only forwarded,
/// never opened.
pub const CONFIG_DIR: &str = r"C:\Users\me\.claude-work";
/// Their Windows Terminal tab.
pub const WT_SESSION: &str = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";

// ---- one test at a time, each with a limit ----

static SERIAL: Mutex<()> = Mutex::new(());

/// Held for the length of a test.
pub struct TestGuard {
    _serial: MutexGuard<'static, ()>,
    /// Dropping it tells the limit's thread that the test ended.
    _alive: mpsc::Sender<()>,
}

/// Call first in every test. Tests run one at a time: several of them measure how long the exe
/// takes, and a neighbour spawning processes would be measured with it. A test still running
/// after the limit ends the whole test run with a failure, because a hung pipe call cannot be
/// interrupted from outside and CI would otherwise wait out its 90 minutes.
pub fn begin(name: &'static str) -> TestGuard {
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

// ---- names, fixtures, the trace ----

fn unique(what: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "agentnotch-test-{}-{}-{what}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A pipe name no other test, process or app uses.
pub fn unique_pipe(what: &str) -> String {
    format!(r"\\.\pipe\{}", unique(what))
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../agentnotch-proto/tests/fixtures")
}

/// A file of `agentnotch-proto/tests/fixtures`, e.g. `stdin/pre_tool_use.json`.
pub fn fixture(relative: &str) -> Vec<u8> {
    let path = fixtures().join(relative);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// One `v1-responses` sample: what Claude Code wrote on the hook's stdin, the app's response
/// frame, and what the Mac hook printed for the two.
pub struct ResponseFixture {
    pub stdin: Vec<u8>,
    /// The frame's bytes as the fixture holds them (key order kept).
    pub response_frame: Vec<u8>,
    /// Empty = the hook prints nothing.
    pub stdout: Vec<u8>,
}

pub fn response_fixture(name: &str) -> ResponseFixture {
    let document = String::from_utf8(fixture(&format!("v1-responses/{name}.json")))
        .expect("a fixture is UTF-8");
    let parsed: serde_json::Value = serde_json::from_str(&document).expect("a fixture is JSON");
    let stdin = parsed["stdin"]
        .as_str()
        .expect("the fixture names its stdin");
    ResponseFixture {
        stdin: fixture(&format!("stdin/{stdin}.json")),
        response_frame: response_text(&document),
        stdout: fixture(&format!("v1-responses/{name}.stdout")),
    }
}

/// The `"response": {…}` object's text, without re-serialising it: parsing and writing it again
/// would sort its keys, and the hook prints them in the order they came.
fn response_text(document: &str) -> Vec<u8> {
    let key = "\"response\":";
    let start = document.find(key).expect("a response") + key.len();
    let open = start + document[start..].find('{').expect("a response object");
    let bytes = document.as_bytes();
    let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
    for (offset, &byte) in bytes[open..].iter().enumerate() {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return bytes[open..=open + offset].to_vec();
                }
            }
            _ => {}
        }
    }
    panic!("the response object never ends")
}

/// A file for `AGENTNOTCH_HOOK_TRACE`.
pub fn trace_file() -> PathBuf {
    std::env::temp_dir().join(format!("{}.trace", unique("hook")))
}

/// What the run with this pid wrote to the trace, which is then deleted: every line is
/// `<unix ms> <pid> <what>`; the `<what>`s are returned.
pub fn traced(path: &Path, pid: u32) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (_ms, by, what) = (parts.next(), parts.next(), parts.next());
            assert_eq!(by, Some(pid.to_string().as_str()), "{line}");
            what.expect("a line says what happened").to_owned()
        })
        .collect()
}

/// Every line of a trace, which is then deleted, as `(pid, what)`: for a run whose hook is not
/// the process the test started (a shell ran it), so its pid is not known beforehand.
pub fn trace_lines(path: &Path) -> Vec<(u32, String)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (_ms, by, what) = (parts.next(), parts.next(), parts.next());
            let pid = by.and_then(|by| by.parse().ok());
            (
                pid.unwrap_or_else(|| panic!("no pid in {line:?}")),
                what.expect("a line says what happened").to_owned(),
            )
        })
        .collect()
}

/// A folder of the test's own under the temporary folder, removed when dropped.
pub struct TempFolder(PathBuf);

pub fn temp_folder(what: &str) -> TempFolder {
    let path = std::env::temp_dir().join(unique(what));
    std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    TempFolder(path)
}

impl TempFolder {
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A copy of the exe `from` at `relative` (its folders are made).
    pub fn copy_exe(&self, from: &str, relative: &str) -> PathBuf {
        let to = self.0.join(relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("{}: {error}", parent.display()));
        }
        std::fs::copy(from, &to).unwrap_or_else(|error| panic!("{}: {error}", to.display()));
        to
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        // An exe that only just exited may still be locked; a leftover in the temporary folder
        // is not worth failing a test for.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- string commands and the shells that run them ----

/// DESIGN-WIN §4.3's test for a path written into a string command with no quotes: Claude Code
/// runs a string command through Git Bash or PowerShell, so only characters that mean the same
/// in both may appear. Letters and digits of any script, `_ . - / :`, and `~` anywhere but first
/// (8.3 names hold it; at the start both shells expand it).
pub fn unquoted(path: &str) -> bool {
    !path.is_empty()
        && path
            .chars()
            .enumerate()
            .all(|(index, c)| c.is_alphanumeric() || "_.-/:".contains(c) || (c == '~' && index > 0))
}

fn forward_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

/// The 8.3 form of an existing path (`GetShortPathNameW`). On a volume without 8.3 names this
/// is the long path again.
pub fn short_path(path: &Path) -> Option<String> {
    let long = wide(path.to_str()?);
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: `long` is NUL-terminated and `buffer` holds the number of units passed.
    let length =
        unsafe { GetShortPathNameW(long.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    let length = length as usize;
    (length != 0 && length < buffer.len()).then(|| String::from_utf16_lossy(&buffer[..length]))
}

/// How the installer writes an exe's path into a string command (DESIGN-WIN §4.3): with forward
/// slashes and no quotes; else its 8.3 form, when that passes the same test; else there is no
/// string form for it.
///
/// The rule is written out here because the engine's builder (`hooks::commands`, WP2) is not on
/// this branch: once it is, the tests take the strings from it.
pub fn string_form_path(exe: &Path) -> Option<String> {
    let long = forward_slashes(exe.to_str()?);
    if unquoted(&long) {
        return Some(long);
    }
    short_path(exe)
        .map(|short| forward_slashes(&short))
        .filter(|short| unquoted(short))
}

/// The two shells Claude Code may run a string command through on Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// `%ProgramFiles%\Git\bin\bash.exe -c <line>`.
    GitBash,
    /// `powershell -NoProfile -Command <line>`.
    PowerShell,
}

impl Shell {
    /// The shell's exe, by its full path: a test never runs whatever `PATH` happens to hold.
    pub fn program(self) -> PathBuf {
        let (variable, fallback, relative) = match self {
            Shell::GitBash => ("ProgramFiles", r"C:\Program Files", r"Git\bin\bash.exe"),
            Shell::PowerShell => (
                "SystemRoot",
                r"C:\Windows",
                r"System32\WindowsPowerShell\v1.0\powershell.exe",
            ),
        };
        let root =
            std::env::var_os(variable).map_or_else(|| PathBuf::from(fallback), PathBuf::from);
        let program = root.join(relative);
        assert!(
            program.is_file(),
            "{self:?} is not at {}: these tests need it",
            program.display()
        );
        program
    }

    /// The arguments that make the shell run `line`.
    pub fn args(self, line: &str) -> Vec<String> {
        match self {
            Shell::GitBash => vec!["-c".into(), line.into()],
            Shell::PowerShell => vec!["-NoProfile".into(), "-Command".into(), line.into()],
        }
    }

    /// The shell running `line`, started the way Claude Code starts a hook's shell: with no
    /// window of its own.
    pub fn command(self, line: &str) -> Command {
        let mut command = Command::new(self.program());
        command
            .args(self.args(line))
            .creation_flags(CREATE_NO_WINDOW);
        command
    }
}

// ---- the hook exe ----

/// `agentnotch-hook.exe hook` as an attended terminal session of this test process would run
/// it, talking to `pipe`. A test changes what it needs with `.env`/`.env_remove`/`.arg`.
pub fn hook_command(pipe: &str) -> Command {
    let mut command = Command::new(EXE);
    command.arg("hook");
    with_hook_env(command, pipe)
}

/// `command` with the environment [`hook_command`] gives the exe. Whatever `command` starts
/// inherits it, so the hook may be a grandchild: behind a shell, or behind a stand-in parent.
pub fn with_hook_env(mut command: Command, pipe: &str) -> Command {
    for name in [
        "CLAUDE_PID",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_ENTRYPOINT",
        "AGENTNOTCH_HOOK_TRACE",
        "AGENTNOTCH_HOOK_TEST_PANIC",
        "AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS",
        "AGENTNOTCH_STATUSLINE_DEPTH",
        "WT_SESSION",
        "TERM_PROGRAM",
    ] {
        command.env_remove(name);
    }
    command
        .env("AGENTNOTCH_DEV", "1")
        .env("AGENTNOTCH_SOCKET", pipe)
        .env("CLAUDE_PID", std::process::id().to_string())
        .env("CLAUDE_CONFIG_DIR", CONFIG_DIR)
        .env("CLAUDE_CODE_SESSION_ATTENDED", "1")
        .env("CLAUDE_CODE_ENTRYPOINT", "cli")
        .env("WT_SESSION", WT_SESSION);
    command
}

/// What a hook started by [`hook_command`] knows besides stdin, for building the message it is
/// expected to send.
pub fn hook_env(hook_pid: u32) -> HookEnv {
    HookEnv {
        claude_pid: Some(std::process::id().to_string()),
        claude_config_dir: Some(CONFIG_DIR.to_owned()),
        attended: Some("1".to_owned()),
        entrypoint: Some("cli".to_owned()),
        pid_guess: None,
        hook_pid,
        wt_session: Some(WT_SESSION.to_owned()),
        term_program: None,
    }
}

/// A hook that was started and given its stdin.
pub struct Running {
    pid: u32,
    done: mpsc::Receiver<std::io::Result<Finished>>,
}

/// How a hook ended.
#[derive(Debug)]
pub struct Finished {
    pub pid: u32,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// From just before the process was created until it had exited and its output was read.
    pub elapsed: Duration,
}

/// Starts `command` with piped stdio, writes `stdin` and closes it (the hook reads to the end).
pub fn spawn(mut command: Command, stdin: &[u8]) -> Running {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = command.spawn().expect("the hook exe starts");
    let pid = child.id();
    if let Some(mut pipe) = child.stdin.take() {
        // A run that ended before it read everything is judged by its exit code, not here.
        let _ = pipe.write_all(stdin);
    }
    let (tell, done) = mpsc::channel();
    std::thread::spawn(move || {
        let output = child.wait_with_output();
        let elapsed = started.elapsed();
        let _ = tell.send(output.map(|output| Finished {
            pid,
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
            elapsed,
        }));
    });
    Running { pid, done }
}

impl Running {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Waits for the hook to end; one that has not after [`HARD_TIMEOUT`] is killed and the
    /// test fails.
    #[track_caller]
    pub fn finish(self) -> Finished {
        match self.done.recv_timeout(HARD_TIMEOUT) {
            Ok(Ok(finished)) => finished,
            Ok(Err(error)) => panic!("the hook exe's output could not be read: {error}"),
            Err(_) => {
                kill_process(self.pid);
                panic!("the hook exe hung");
            }
        }
    }

    /// The hook is still running `after` this long (a PermissionRequest waiting for its answer).
    #[track_caller]
    pub fn assert_waiting(&self, after: Duration) {
        if let Ok(ended) = self.done.recv_timeout(after) {
            panic!("the hook did not wait for an answer: {ended:?}");
        }
    }

    /// Ends the hook the way Claude Code does when its own dialog was answered first.
    #[track_caller]
    pub fn kill(&self) {
        assert!(kill_process(self.pid), "the hook could not be killed");
    }
}

/// Exit 0, nothing printed: what every path of a hook looks like except an answered request.
#[track_caller]
pub fn assert_silent_success(done: &Finished, what: &str) {
    assert_eq!(done.code, Some(0), "{what}: exit code");
    assert!(
        done.stdout.is_empty(),
        "{what}: stdout {:?}",
        String::from_utf8_lossy(&done.stdout)
    );
    assert!(
        done.stderr.is_empty(),
        "{what}: stderr {:?}",
        String::from_utf8_lossy(&done.stderr)
    );
}

fn kill_process(pid: u32) -> bool {
    // SAFETY: plain calls; the handle is checked before it is used and closed after.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return false;
        }
        let ended = TerminateProcess(process, 1) != 0;
        CloseHandle(process);
        ended
    }
}

// ---- the app's side, in-process ----

/// The real pipe server feeding the engine's real ingress. Nothing runs between the two on its
/// own: the test takes each transport event with [`Harness::step`] (or a `wait_*`), which hands
/// it to the ingress as the hub's core thread would.
pub struct Harness {
    pub pipe: String,
    pub server: Arc<PipeServer>,
    pub ingress: HookIngress,
    events: crossbeam_channel::Receiver<TransportEvent>,
}

/// One transport event and what the ingress made of it.
#[derive(Debug)]
pub struct Step {
    pub event: TransportEvent,
    pub outs: Vec<IngressOut>,
}

impl Harness {
    /// A server on a name of its own, listening.
    pub fn start(what: &str) -> Harness {
        let mut harness = Harness::on(&unique_pipe(what));
        harness.wait_listening();
        harness
    }

    /// A server started on `pipe`; whether it got the name is its first event.
    pub fn on(pipe: &str) -> Harness {
        let server = Arc::new(PipeServer::new());
        let ingress = HookIngress::with_transport(IngressConfig::new(pipe), server.clone());
        let (sink, events) = crossbeam_channel::unbounded();
        ingress.start(sink).expect("the pipe server starts");
        Harness {
            pipe: pipe.to_owned(),
            server,
            ingress,
            events,
        }
    }

    /// The next transport event, handed to the ingress; `None` when none came `within`.
    pub fn step(&mut self, within: Duration) -> Option<Step> {
        let event = self.events.recv_timeout(within).ok()?;
        let outs = self.ingress.on_transport(event.clone(), SystemTime::now());
        Some(Step { event, outs })
    }

    /// Steps until `pick` finds what the test waits for. The steps before it are dropped.
    #[track_caller]
    pub fn wait_for<T>(&mut self, what: &str, mut pick: impl FnMut(&Step) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(step) = self.step(left) else {
                panic!("timed out waiting for {what}");
            };
            if let Some(found) = pick(&step) {
                return found;
            }
        }
    }

    #[track_caller]
    pub fn wait_listening(&mut self) {
        let pipe = self.pipe.clone();
        self.wait_for("the server to listen", |step| match &step.event {
            TransportEvent::Listening(name) => {
                assert_eq!(*name, pipe);
                Some(())
            }
            TransportEvent::Error(why) => panic!("the server does not listen: {why}"),
            _ => None,
        });
    }

    /// The next frame as the transport delivered it, and what the ingress made of it.
    #[track_caller]
    pub fn wait_frame(&mut self) -> (IncomingFrame, Vec<IngressOut>) {
        self.wait_for("a frame", |step| match &step.event {
            TransportEvent::Frame(frame) => Some((frame.clone(), step.outs.clone())),
            _ => None,
        })
    }

    /// The next hook event of that name the ingress passes on.
    #[track_caller]
    pub fn wait_hook(&mut self, name: &str) -> HookEvent {
        self.wait_for(name, |step| {
            step.outs.iter().find_map(|out| match out {
                IngressOut::Hook(event) if event.event == name => Some(event.clone()),
                _ => None,
            })
        })
    }

    /// The next PermissionRequest the ingress holds.
    #[track_caller]
    pub fn wait_held(&mut self) -> HeldPermission {
        self.wait_for("a held PermissionRequest", |step| {
            step.outs.iter().find_map(|out| match out {
                IngressOut::PermissionHeld(held) => Some(held.clone()),
                _ => None,
            })
        })
    }

    /// Everything that arrives in the next `span`, handed to the ingress.
    pub fn during(&mut self, span: Duration) -> Vec<Step> {
        let deadline = Instant::now() + span;
        let mut steps = Vec::new();
        while let Some(step) = self.step(deadline.saturating_duration_since(Instant::now())) {
            steps.push(step);
        }
        steps
    }

    /// The frames among what arrives in the next `span`: for "nothing was delivered".
    pub fn frames_during(&mut self, span: Duration) -> Vec<IncomingFrame> {
        self.during(span)
            .into_iter()
            .filter_map(|step| match step.event {
                TransportEvent::Frame(frame) => Some(frame),
                _ => None,
            })
            .collect()
    }
}

// ---- raw pipe ends ----

/// `S-1-5-21-…` of the user the tests run as.
pub fn own_sid() -> String {
    agentnotch_win::sid::current_user_sid().expect("this user's SID")
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A client end of `pipe`, opened as every client of the app opens it (identification only),
/// that has written nothing yet.
pub fn open_client(pipe: &str) -> File {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            // std adds SECURITY_SQOS_PRESENT itself.
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(pipe);
        match opened {
            Ok(client) => return client,
            // The server replaces a connected instance at once; a client that came in between
            // finds every instance taken for an instant.
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{pipe} can't be opened: {error}"),
        }
    }
}

/// The pipe object's owner and DACL as a client reads them through its own handle: what the
/// hook checks before it writes a byte.
pub fn pipe_security(client: &File) -> PipeSecurity {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the handle is open with READ_CONTROL (part of GENERIC_READ); the out pointers are
    // valid; the group and the SACL are not asked for.
    let status = unsafe {
        GetSecurityInfo(
            client.as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(status, ERROR_SUCCESS, "GetSecurityInfo");
    assert!(!descriptor.is_null(), "GetSecurityInfo gave no descriptor");
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: `descriptor` is the valid descriptor GetSecurityInfo returned.
    let read = unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) };
    assert_ne!(read, 0, "GetSecurityDescriptorControl");
    let security = PipeSecurity {
        owner: sid_to_string(owner),
        dacl_protected: control & SE_DACL_PROTECTED != 0,
        dacl: (!dacl.is_null()).then(|| read_aces(dacl)),
    };
    // SAFETY: GetSecurityInfo allocated the descriptor with LocalAlloc; `owner` and `dacl` point
    // into it and are not used after this.
    unsafe { LocalFree(descriptor) };
    security
}

fn read_aces(dacl: *const ACL) -> Vec<PipeAce> {
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `dacl` is the valid ACL of the descriptor; `info` is a valid out buffer of the
    // size passed.
    let read = unsafe {
        GetAclInformation(
            dacl,
            addr_of_mut!(info).cast::<c_void>(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    assert_ne!(read, 0, "GetAclInformation");
    (0..info.AceCount)
        .map(|index| {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: `index` is below the ACL's entry count; `ace` is a valid out pointer.
            let found = unsafe { GetAce(dacl, index, &mut ace) };
            assert!(found != 0 && !ace.is_null(), "GetAce({index})");
            // SAFETY: every ACE starts with a header, and GetAce returned one inside the ACL.
            let kind = unsafe { (*ace.cast::<ACE_HEADER>()).AceType };
            // ACCESS_ALLOWED_ACE_TYPE; every other kind has another layout.
            if kind != 0 {
                return PipeAce {
                    allow: false,
                    sid: String::new(),
                };
            }
            // SAFETY: an access-allowed ACE is a header, a mask and then the SID, which starts
            // where `SidStart` is.
            let sid = unsafe { addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart) };
            PipeAce {
                allow: true,
                sid: sid_to_string(sid.cast::<c_void>()).unwrap_or_default(),
            }
        })
        .collect()
}

fn sid_to_string(sid: PSID) -> Option<String> {
    if sid.is_null() {
        return None;
    }
    let mut text: *mut u16 = null_mut();
    // SAFETY: `sid` points at a SID inside a live descriptor; `text` receives a LocalAlloc'd,
    // NUL-terminated string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return None;
    }
    let mut length = 0;
    // SAFETY: the string is NUL-terminated, so every unit up to the NUL is readable.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were just read from `text`.
    let string = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: the string came from LocalAlloc and is not used after this.
    unsafe { LocalFree(text.cast::<c_void>()) };
    Some(string)
}

/// A pipe a client can connect and write to, whose server never reads: one instance with the
/// app's own owner and DACL (so the hook's check passes and it starts writing), held open and
/// left alone. Closed when dropped.
pub struct SilentServer(HANDLE);

pub fn silent_server(pipe: &str) -> SilentServer {
    let descriptor =
        SecurityDescriptor::from_sddl(&pipe_sddl(&own_sid())).expect("the pipe's descriptor");
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.as_ptr(),
        bInheritHandle: 0,
    };
    let name = wide(pipe);
    // SAFETY: `name` is NUL-terminated and `attributes` (with the descriptor it points at)
    // lives until the call returns; Windows copies both.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            &attributes,
        )
    };
    assert!(
        !handle.is_null() && handle != INVALID_HANDLE_VALUE,
        "{pipe} can't be created: {}",
        std::io::Error::last_os_error()
    );
    SilentServer(handle)
}

impl Drop for SilentServer {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and is not used after this.
        unsafe { CloseHandle(self.0) };
    }
}
