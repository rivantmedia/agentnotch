//! The exe fails open on every system (DESIGN-WIN §1.8): whatever it is given, it exits 0 with
//! nothing on stdout, because Claude Code treats a hook that exits 2 as a block and shows what a
//! hook prints. No app listens in these tests, so every hook run takes the "no app" path; what a
//! run did is read from the dev-only trace.
//!
//! The status line wrapper is the one role with more to it: it runs the status line command it
//! replaced and passes that command's output and exit code on. What holds for it on every system
//! is here too (the Mac's StatusLineScriptTests, minus what needs the app or a Job object, which
//! `tests/statusline.rs` proves on Windows): these tests run the previous command through
//! `/bin/bash` where there is no Git Bash.
//!
//! Also the exe-level half of `Fix_ScriptSocketOverrideTests.aLeftoverExportIsIgnored`: the
//! development switches (`AGENTNOTCH_SOCKET`, `AGENTNOTCH_HOOK_TRACE`) do nothing without
//! `AGENTNOTCH_DEV=1`. The pipe name rule itself is pinned in `agentnotch-proto`
//! (`dev_pipe_override`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_agentnotch-hook");

/// A run that takes longer than this has hung; the exe's own bound is 1.2 s.
const HARD_TIMEOUT: Duration = Duration::from_secs(30);

const EVENT: &[u8] = br#"{"hook_event_name":"PreToolUse","session_id":"11111111-2222-3333-4444-555555555555","cwd":"/work/secret-project","tool_name":"Bash","tool_input":{"command":"echo secret-input"},"tool_use_id":"toolu_1"}"#;

enum Stdin<'a> {
    Bytes(&'a [u8]),
    /// Nothing to read at all (the null device; a closed descriptor reads the same).
    Closed,
}

struct Run {
    output: Output,
    pid: u32,
}

fn unique(what: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "agentnotch-test-{}-{}-{what}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The exe with none of Claude Code's or this app's variables, and a pipe name no app serves
/// (honoured only where a test also sets `AGENTNOTCH_DEV=1`).
fn hook_command(args: &[&str]) -> Command {
    exe_command(Path::new(EXE), args)
}

/// The same for a copy of the exe.
fn exe_command(exe: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(exe);
    command.args(args);
    for name in [
        "CLAUDE_PID",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_ENTRYPOINT",
        "AGENTNOTCH_DEV",
        "AGENTNOTCH_HOOK_TRACE",
        "AGENTNOTCH_HOOK_TEST_PANIC",
        "AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS",
        "AGENTNOTCH_STATUSLINE_DEPTH",
    ] {
        command.env_remove(name);
    }
    command.env(
        "AGENTNOTCH_SOCKET",
        format!(r"\\.\pipe\{}", unique("no-app")),
    );
    command
}

fn run(mut command: Command, stdin: Stdin) -> Run {
    command
        .stdin(match stdin {
            Stdin::Bytes(_) => Stdio::piped(),
            Stdin::Closed => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the hook exe starts");
    let pid = child.id();
    if let (Stdin::Bytes(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // A role that reads no stdin may be gone already: a broken pipe is fine.
        let _ = pipe.write_all(bytes);
    }
    // Read while it runs: a status line may print more than a pipe holds.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the hook exe can be waited for") {
            break status;
        }
        if started.elapsed() > HARD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the hook exe hung");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Run {
        output: Output {
            status,
            stdout: stdout.join().expect("the hook exe's stdout"),
            stderr: stderr.join().expect("the hook exe's stderr"),
        },
        pid,
    }
}

/// Everything a stream holds, read on a thread of its own.
fn drain(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_end(&mut bytes);
        }
        bytes
    })
}

#[track_caller]
fn assert_silent_success(run: &Run, what: &str) {
    assert_eq!(run.output.status.code(), Some(0), "{what}: exit code");
    assert!(
        run.output.stdout.is_empty(),
        "{what}: stdout {:?}",
        String::from_utf8_lossy(&run.output.stdout)
    );
    assert!(
        run.output.stderr.is_empty(),
        "{what}: stderr {:?}",
        String::from_utf8_lossy(&run.output.stderr)
    );
}

fn trace_file() -> PathBuf {
    std::env::temp_dir().join(format!("{}.trace", unique("hook")))
}

/// What one run wrote: every line is `<unix ms> <pid> <what>`; the `<what>`s are returned.
fn traced(path: &PathBuf, pid: u32) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (ms, by, what) = (parts.next(), parts.next(), parts.next());
            assert!(
                ms.is_some_and(|ms| ms.len() >= 13 && ms.bytes().all(|b| b.is_ascii_digit())),
                "{line}"
            );
            assert_eq!(by, Some(pid.to_string().as_str()), "{line}");
            what.expect("a line says what happened").to_owned()
        })
        .collect()
}

#[test]
fn every_argv_form_exits_zero_with_nothing_printed() {
    let forms: [&[&str]; 14] = [
        &["hook"],
        &["hook", "--exec"],
        &["statusline"],
        // Not ours to answer: no arguments, unknown subcommands, extra arguments.
        &[],
        &["frobnicate"],
        &["--help"],
        &["-h"],
        &["hook", "--exec", "extra"],
        &["hook", "--unknown"],
        &["hook", "hook"],
        &["statusline", "extra"],
        &["console-info"],
        &["console-info", "--pid", "not-a-pid"],
        &["type", "--pid"],
    ];
    for args in forms {
        let done = run(hook_command(args), Stdin::Bytes(EVENT));
        assert_silent_success(&done, &format!("{args:?}"));
    }
}

#[test]
fn garbage_on_stdin_is_ignored() {
    let mut large = vec![b'{'; 3 << 20];
    large.extend_from_slice(b"\xff\xfe");
    let inputs: [&[u8]; 8] = [
        b"",
        b"not json",
        b"{",
        b"[1, 2, 3]",
        b"\"PreToolUse\"",
        b"null",
        b"\xff\xfe\x00\x00{\x00}",
        &large,
    ];
    for args in [&["hook"][..], &["hook", "--exec"], &["statusline"]] {
        for input in inputs {
            let shown = String::from_utf8_lossy(&input[..input.len().min(24)]).into_owned();
            // Once as a real session runs it, once with the development switches on.
            let done = run(hook_command(args), Stdin::Bytes(input));
            assert_silent_success(&done, &format!("{args:?} < {shown:?}"));
            let mut dev = hook_command(args);
            dev.env("AGENTNOTCH_DEV", "1");
            let done = run(dev, Stdin::Bytes(input));
            assert_silent_success(&done, &format!("dev {args:?} < {shown:?}"));
        }
    }
}

#[test]
fn closed_stdin_is_ignored() {
    for args in [&["hook"][..], &["hook", "--exec"], &["statusline"], &[]] {
        let done = run(hook_command(args), Stdin::Closed);
        assert_silent_success(&done, &format!("{args:?}"));
    }
}

/// The switch exists in debug builds only, and tests build the exe in their own profile.
#[cfg(debug_assertions)]
#[test]
fn a_panic_is_still_exit_zero_with_nothing_printed() {
    for args in [&["hook"][..], &["hook", "--exec"]] {
        let path = trace_file();
        let mut command = hook_command(args);
        command
            .env("AGENTNOTCH_HOOK_TEST_PANIC", "1")
            .env("AGENTNOTCH_DEV", "1")
            .env("AGENTNOTCH_HOOK_TRACE", &path);
        let done = run(command, Stdin::Bytes(EVENT));
        assert_silent_success(&done, &format!("{args:?}"));
        // It did panic: the run ended before it looked for the app.
        let lines = traced(&path, done.pid);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("invoked hook exec="), "{lines:?}");
    }
}

#[test]
fn the_trace_tells_what_a_run_did() {
    let cases: [(&[&str], &[&str]); 5] = [
        (
            &["hook"],
            &[
                "invoked hook exec=false",
                "env claude_pid=unset config_dir=unset",
                "no app",
            ],
        ),
        (
            &["hook", "--exec"],
            &[
                "invoked hook exec=true",
                "env claude_pid=unset config_dir=unset",
                "no app",
            ],
        ),
        // Nothing saved beside the exe to chain; the status is still sent (to no app).
        (
            &["statusline"],
            &[
                "invoked statusline",
                "chain: nothing",
                "env claude_pid=unset config_dir=unset",
                "no app",
            ],
        ),
        (&[], &["invoked unknown"]),
        (&["hook", "--exec", "extra"], &["invoked unknown"]),
    ];
    for (args, expected) in cases {
        let path = trace_file();
        let mut command = hook_command(args);
        command
            .env("AGENTNOTCH_DEV", "1")
            .env("AGENTNOTCH_HOOK_TRACE", &path);
        let done = run(command, Stdin::Bytes(EVENT));
        assert_silent_success(&done, &format!("{args:?}"));
        assert_eq!(traced(&path, done.pid), expected, "{args:?}");
    }
}

#[test]
fn the_trace_names_states_never_values() {
    let path = trace_file();
    let mut command = hook_command(&["hook"]);
    command
        .env("AGENTNOTCH_DEV", "1")
        .env("AGENTNOTCH_HOOK_TRACE", &path)
        .env("CLAUDE_PID", "4242")
        .env("CLAUDE_CONFIG_DIR", "/home/someone/secret-config");
    let done = run(command, Stdin::Bytes(EVENT));
    assert_silent_success(&done, "hook");
    let lines = traced(&path, done.pid);
    assert_eq!(
        lines,
        [
            "invoked hook exec=false",
            "env claude_pid=set config_dir=set",
            "no app"
        ]
    );
    let all = lines.join("\n");
    for value in ["secret", "4242", "echo", "toolu_1", "11111111"] {
        assert!(!all.contains(value), "{value} in {all}");
    }
}

/// A leftover export in a real session: without `AGENTNOTCH_DEV=1` (or with another value) no
/// trace file is made.
#[test]
fn nothing_is_traced_without_the_dev_switch() {
    for dev in [None, Some("0"), Some("true"), Some("")] {
        for args in [&["hook"][..], &["hook", "--exec"], &["statusline"], &[]] {
            let path = trace_file();
            let mut command = hook_command(args);
            command.env("AGENTNOTCH_HOOK_TRACE", &path);
            if let Some(dev) = dev {
                command.env("AGENTNOTCH_DEV", dev);
            }
            // Not an event: a run that reached an app under this user's real pipe name (the
            // override is off here) still sends it nothing.
            let done = run(command, Stdin::Bytes(b"not json"));
            assert_silent_success(&done, &format!("{args:?} dev={dev:?}"));
            assert!(!path.exists(), "{args:?} dev={dev:?} wrote a trace");
        }
    }
    // And an empty trace path with the switch on names no file.
    let mut command = hook_command(&["hook"]);
    command
        .env("AGENTNOTCH_DEV", "1")
        .env("AGENTNOTCH_HOOK_TRACE", "");
    assert_silent_success(&run(command, Stdin::Bytes(EVENT)), "empty trace path");
}

// ---- the status line wrapper ----

/// What Claude Code writes on a status line command's stdin, in short.
const STATUS: &[u8] = br#"{"session_id":"sess-1","transcript_path":"/work/.claude-work/projects/-work-proj/sess-1.jsonl","cwd":"/work/proj","session_name":"my-session","model":{"id":"claude-opus-4-5","display_name":"Opus"},"version":"2.1.282","cost":{"total_cost_usd":0.0123},"context_window":{"context_window_size":200000,"used_percentage":8},"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600}}}"#;

/// A copy of the exe in a folder of its own, as a config folder's `hooks` holds it, with what
/// the app saved beside it. The wrapper reads the saved status line from the folder of the exe
/// it runs as, so no test writes beside the build's own exe.
struct Wrapper {
    folder: PathBuf,
    exe: PathBuf,
}

impl Wrapper {
    fn new() -> Wrapper {
        Wrapper::named("agentnotch-hook")
    }

    /// The copy under another name: a spelling the wrapper's own name check cannot see.
    fn named(stem: &str) -> Wrapper {
        let folder = std::env::temp_dir().join(unique("sl"));
        std::fs::create_dir_all(&folder).expect("a temporary folder");
        let exe = folder.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(EXE, &exe).expect("a copy of the exe");
        Wrapper { folder, exe }
    }

    /// Saves `command` as the status line this wrapper replaced.
    fn previous(&self, command: &str) {
        let object = serde_json::json!({"type": "command", "command": command, "padding": 0});
        self.previous_bytes(object.to_string().as_bytes());
    }

    fn previous_bytes(&self, bytes: &[u8]) {
        std::fs::write(
            self.folder.join("agentnotch-statusline.previous.json"),
            bytes,
        )
        .expect("the saved status line");
    }

    /// `<copy> statusline`, traced to `trace`, with no app to talk to.
    fn command(&self, trace: &Path) -> Command {
        let mut command = exe_command(&self.exe, &["statusline"]);
        command
            .env("AGENTNOTCH_DEV", "1")
            .env("AGENTNOTCH_HOOK_TRACE", trace);
        // Windows finds Git Bash itself; elsewhere the system's bash stands in for it.
        if cfg!(not(windows)) {
            command.env("CLAUDE_CODE_GIT_BASH_PATH", "/bin/bash");
        }
        command
    }

    /// The copy's path as a shell command names it.
    fn shell_path(&self) -> String {
        format!(
            "'{}'",
            self.exe
                .to_str()
                .expect("a Unicode path")
                .replace('\\', "/")
        )
    }
}

impl Drop for Wrapper {
    fn drop(&mut self) {
        // A copy that only just exited may still be locked; a leftover in the temporary folder
        // is not worth failing a test for.
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Every line of a trace, which is then deleted, as `(pid, what)`.
fn trace_lines(path: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (_ms, by, what) = (parts.next(), parts.next(), parts.next());
            (
                by.expect("a line names its process").to_owned(),
                what.expect("a line says what happened").to_owned(),
            )
        })
        .collect()
}

/// StatusLineScriptTests.forwardsToTheAppAndChainsThePreviousCommand and appNotRunningStillChains,
/// without the app: the previous command sees the same stdin, and its output and exit code pass
/// through.
#[test]
fn the_status_line_chains_the_previous_command() {
    let wrapper = Wrapper::new();
    let trace = trace_file();

    wrapper.previous("cat; echo TAIL; exit 3");
    let done = run(wrapper.command(&trace), Stdin::Bytes(STATUS));
    let mut expected = STATUS.to_vec();
    expected.extend_from_slice(b"TAIL\n");
    assert_eq!(
        String::from_utf8_lossy(&done.output.stdout),
        String::from_utf8_lossy(&expected)
    );
    assert_eq!(done.output.status.code(), Some(3));
    assert!(done.output.stderr.is_empty());
    assert_eq!(
        traced(&trace, done.pid),
        [
            "invoked statusline",
            "chain: started",
            "env claude_pid=unset config_dir=unset",
            "no app",
            "chain: exit 3"
        ]
    );

    wrapper.previous("echo from-previous");
    let done = run(wrapper.command(&trace), Stdin::Bytes(STATUS));
    assert_eq!(done.output.stdout, b"from-previous\n");
    assert_eq!(done.output.status.code(), Some(0));
    let _ = std::fs::remove_file(&trace);

    // A command that reads none of its input, given more than a pipe holds.
    let mut large = br#"{"session_id":"sess-1","session_name":""#.to_vec();
    large.resize(large.len() + (1 << 20), b'x');
    large.extend_from_slice(br#""}"#);
    let done = run(wrapper.command(&trace), Stdin::Bytes(&large));
    assert_eq!(done.output.stdout, b"from-previous\n");
    assert_eq!(done.output.status.code(), Some(0));
    let _ = std::fs::remove_file(&trace);

    // And one that prints more than a pipe holds before it reads any.
    wrapper.previous("head -c 300000 /dev/zero | tr '\\0' 'y'; cat >/dev/null");
    let done = run(wrapper.command(&trace), Stdin::Bytes(&large));
    assert_eq!(done.output.stdout.len(), 300_000);
    assert!(done.output.stdout.iter().all(|&byte| byte == b'y'));
    assert_eq!(done.output.status.code(), Some(0));
    let _ = std::fs::remove_file(&trace);
}

/// StatusLineScriptTests.withoutAPreviousCommandPrintsNothing, and every saved file that names
/// no command.
#[test]
fn without_a_previous_command_the_status_line_prints_nothing() {
    let wrapper = Wrapper::new();
    let saved: [Option<&[u8]>; 8] = [
        None,
        Some(b"{}"),
        Some(b""),
        Some(b"not json"),
        Some(b"[\"echo hi\"]"),
        Some(br#"{"type":"command","command":"   "}"#),
        Some(br#"{"type":"command","command":7}"#),
        Some(b"\xff\xfe{\x00}\x00"),
    ];
    for bytes in saved {
        if let Some(bytes) = bytes {
            wrapper.previous_bytes(bytes);
        }
        let trace = trace_file();
        let done = run(wrapper.command(&trace), Stdin::Bytes(STATUS));
        let what = format!("{:?}", bytes.map(String::from_utf8_lossy));
        assert_silent_success(&done, &what);
        assert_eq!(
            traced(&trace, done.pid),
            [
                "invoked statusline",
                "chain: nothing",
                "env claude_pid=unset config_dir=unset",
                "no app"
            ],
            "{what}"
        );
    }
}

/// StatusLineScriptTests.garbageInputIsNotForwardedButStillChained.
#[test]
fn a_status_that_is_not_json_is_not_forwarded_but_still_chained() {
    let wrapper = Wrapper::new();
    wrapper.previous("wc -c | tr -d ' '");
    for (stdin, why) in [
        (&b"not json"[..], "stdin: not JSON"),
        (b"[1,2,3,4]", "stdin: not a JSON object"),
    ] {
        let trace = trace_file();
        let done = run(wrapper.command(&trace), Stdin::Bytes(stdin));
        assert_eq!(done.output.stdout, format!("{}\n", stdin.len()).as_bytes());
        assert_eq!(done.output.status.code(), Some(0));
        // Nothing was sent: the run never looked for the app.
        assert_eq!(
            traced(&trace, done.pid),
            ["invoked statusline", "chain: started", why, "chain: exit 0"]
        );
    }
    // No stdin at all still chains, with nothing to pass on.
    let trace = trace_file();
    let done = run(wrapper.command(&trace), Stdin::Closed);
    assert_eq!(done.output.stdout, b"0\n");
    assert_eq!(done.output.status.code(), Some(0));
    let _ = std::fs::remove_file(&trace);
}

/// StatusLineScriptTests.neverChainsToAWrapper (its three names), and the Windows wrapper under
/// the spellings a settings file may hold. The rule's own vectors are in agentnotch-proto.
#[test]
fn the_status_line_never_chains_to_a_wrapper() {
    let wrapper = Wrapper::new();
    let own = wrapper.shell_path();
    let commands = [
        "python3 '/x/hooks/agentnotch-statusline.py'".to_owned(),
        "python3 '/x/hooks/superpowered-codenotch-statusline.py'".to_owned(),
        "python3 '/x/hooks/superpowered-notch-statusline.py'".to_owned(),
        format!("{own} statusline"),
        format!("{} STATUSLINE", own.to_uppercase()),
        format!("cd / && {own} statusline"),
        r"C:\Users\me\.claude\hooks\agentnotch-hook.exe statusline".to_owned(),
        "C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE statusline".to_owned(),
    ];
    for command in commands {
        wrapper.previous(&command);
        let trace = trace_file();
        let done = run(wrapper.command(&trace), Stdin::Bytes(STATUS));
        assert_silent_success(&done, &command);
        // One process, which started nothing.
        let lines = traced(&trace, done.pid);
        assert_eq!(lines[1], "chain: nothing", "{command}: {lines:?}");
    }
}

/// The loop guard for a wrapper the name check cannot see: the chained command's environment
/// says it was started by a wrapper, and a wrapper that finds that chains nothing.
#[test]
fn a_wrapper_reached_through_a_wrapper_chains_nothing() {
    // Started with the mark already set: nothing is chained, whatever was saved.
    let wrapper = Wrapper::new();
    wrapper.previous("echo chained");
    let trace = trace_file();
    let mut command = wrapper.command(&trace);
    command.env("AGENTNOTCH_STATUSLINE_DEPTH", "1");
    let done = run(command, Stdin::Bytes(STATUS));
    assert_silent_success(&done, "started with the mark");
    assert_eq!(traced(&trace, done.pid)[1], "chain: nested");

    // A copy under another name whose saved command is itself: without the mark every render
    // would start a process that starts another, for ever. Twenty renders start twenty.
    let copy = Wrapper::named("sl");
    copy.previous(&format!("{} statusline", copy.shell_path()));
    let trace = trace_file();
    let mut outer = Vec::new();
    for attempt in 0..20 {
        let done = run(copy.command(&trace), Stdin::Bytes(STATUS));
        assert_silent_success(&done, &format!("attempt {attempt}"));
        outer.push(done.pid.to_string());
    }
    let lines = trace_lines(&trace);
    let count = |what: &str| lines.iter().filter(|(_, line)| line == what).count();
    assert_eq!(count("invoked statusline"), 40, "{lines:?}");
    assert_eq!(count("chain: started"), 20, "{lines:?}");
    assert_eq!(count("chain: nested"), 20, "{lines:?}");
    assert_eq!(count("chain: exit 0"), 20, "{lines:?}");
    // The ones the test started chained; the ones they started did not.
    for (pid, line) in &lines {
        match line.as_str() {
            "chain: started" | "chain: exit 0" => assert!(outer.contains(pid), "{lines:?}"),
            "chain: nested" => assert!(!outer.contains(pid), "{lines:?}"),
            _ => {}
        }
    }
}

/// Git Bash was there when the app wrapped the command and is gone now: no output, exit 0.
#[test]
fn without_git_bash_the_status_line_prints_nothing() {
    let wrapper = Wrapper::new();
    wrapper.previous("echo chained");
    let trace = trace_file();
    // Git uninstalled, the Program Files folders still there: every place Claude Code looks
    // exists and holds no Git. (Only removing the two variables from the child's environment
    // did not keep the runner's child from finding the real Git Bash.)
    let no_git = wrapper.folder.join("Program Files without Git");
    std::fs::create_dir_all(&no_git).expect("an empty Program Files");
    let mut command = wrapper.command(&trace);
    command
        .env(
            "CLAUDE_CODE_GIT_BASH_PATH",
            wrapper.folder.join("no-such-bash.exe"),
        )
        .env("ProgramFiles", &no_git)
        .env("ProgramFiles(x86)", &no_git);
    let done = run(command, Stdin::Bytes(STATUS));
    // The trace first: when the output is wrong, it says which way the wrapper went.
    assert_eq!(
        traced(&trace, done.pid),
        [
            "invoked statusline",
            "chain: no git bash",
            "env claude_pid=unset config_dir=unset",
            "no app"
        ]
    );
    assert_silent_success(&done, "no Git Bash");
}

/// The app never wraps a command written for another shell (the engine's rule). One forced into
/// the saved file anyway still fails open: it ends, with the shell's own verdict, and never as
/// a blocking 2.
#[test]
fn a_command_for_another_shell_still_fails_open() {
    let wrapper = Wrapper::new();
    for command in [
        r"C:\nowhere\status.exe --theme dark",
        "./no-such-status.ps1",
        "echo $env:USERNAME",
    ] {
        wrapper.previous(command);
        let trace = trace_file();
        let done = run(wrapper.command(&trace), Stdin::Bytes(STATUS));
        let code = done.output.status.code();
        assert!(code.is_some_and(|code| code != 2), "{command}: {code:?}");
        let lines = traced(&trace, done.pid);
        assert_eq!(lines[1], "chain: started", "{command}: {lines:?}");
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("chain: exit {}", code.unwrap_or(-1)).as_str()),
            "{command}: {lines:?}"
        );
    }
}

/// StatusLineScriptTests.aSlowPreviousCommandStillPrints: the cap is for commands that hang, not
/// for slow ones. (The switch that shortens it exists in debug builds only.)
#[cfg(debug_assertions)]
#[test]
fn a_slow_previous_command_still_prints() {
    let wrapper = Wrapper::new();
    wrapper.previous("sleep 1; echo slow");
    let trace = trace_file();
    let mut command = wrapper.command(&trace);
    command.env("AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS", "10000");
    let done = run(command, Stdin::Bytes(STATUS));
    assert_eq!(done.output.stdout, b"slow\n");
    assert_eq!(done.output.status.code(), Some(0));
    let _ = std::fs::remove_file(&trace);
}

/// StatusLineScriptTests.aHangingPreviousCommandIsCutOff: past the cap the command is stopped
/// and nothing is printed, not even what it had printed by then. (A 1 s cap here; the shipped
/// one is 30 s. That everything the command started is stopped with it is Windows' Job object,
/// proven in `tests/statusline.rs`.)
#[cfg(debug_assertions)]
#[test]
fn a_hanging_previous_command_is_cut_off() {
    let wrapper = Wrapper::new();
    wrapper.previous("echo partial; exec sleep 20");
    let trace = trace_file();
    let mut command = wrapper.command(&trace);
    command.env("AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS", "1000");
    let started = Instant::now();
    let done = run(command, Stdin::Bytes(STATUS));
    let elapsed = started.elapsed();
    assert_silent_success(&done, "cut off");
    assert!(
        elapsed >= Duration::from_secs(1) && elapsed < Duration::from_secs(10),
        "{elapsed:?}"
    );
    assert_eq!(
        traced(&trace, done.pid).last().map(String::as_str),
        Some("chain: timed out")
    );
}

/// A panic in the wrapper, with the previous command already running: exit 0, nothing printed.
#[cfg(debug_assertions)]
#[test]
fn a_panic_in_the_status_line_is_exit_zero_with_nothing_printed() {
    let wrapper = Wrapper::new();
    wrapper.previous("echo chained; exit 3");
    let trace = trace_file();
    let mut command = wrapper.command(&trace);
    command.env("AGENTNOTCH_HOOK_TEST_PANIC", "1");
    let done = run(command, Stdin::Bytes(STATUS));
    assert_silent_success(&done, "panic");
    assert_eq!(
        traced(&trace, done.pid),
        ["invoked statusline", "chain: started"]
    );
}

// The console helper (`type`, `console-info`): what holds on every system. What it does to a
// console is proven on Windows by `tests/console_type.rs`; its decisions are unit-tested in
// `src/console/checks.rs`.

/// No process has this pid: Windows hands out multiples of four far below it, and other systems
/// stay below their own, lower limits.
const NO_SUCH_PID: &str = "2147483644";

fn type_args(pid: &'static str) -> [&'static str; 9] {
    [
        "type",
        "--pid",
        pid,
        "--started",
        "1790000000000",
        "--expect-window",
        "none",
        "--shells",
        "",
    ]
}

/// The helper's stdout: whole lines, each one the protocol's.
#[track_caller]
fn type_phases(run: &Run, what: &str) -> Vec<agentnotch_proto::TypePhase> {
    assert_eq!(run.output.status.code(), Some(0), "{what}: exit code");
    assert!(run.output.stderr.is_empty(), "{what}: stderr");
    let text = String::from_utf8(run.output.stdout.clone()).expect("UTF-8 on stdout");
    assert!(text.ends_with('\n'), "{what}: {text:?}");
    text.lines()
        .map(|line| {
            agentnotch_proto::TypePhase::from_line(line)
                .unwrap_or_else(|| panic!("{what}: not a line of the protocol: {line:?}"))
        })
        .collect()
}

#[track_caller]
fn only_outcome(run: &Run, what: &str) -> (String, Option<String>) {
    let phases = type_phases(run, what);
    match phases.as_slice() {
        [agentnotch_proto::TypePhase::Outcome { outcome, reason }] => {
            (outcome.clone(), reason.clone())
        }
        other => panic!("{what}: expected one outcome line, got {other:?}"),
    }
}

#[test]
fn typing_for_a_process_that_does_not_exist_is_one_outcome_line() {
    let path = trace_file();
    let mut command = hook_command(&type_args(NO_SUCH_PID));
    command
        .env("AGENTNOTCH_DEV", "1")
        .env("AGENTNOTCH_HOOK_TRACE", &path);
    let done = run(command, Stdin::Bytes(b"{\"text\":\"hello\"}\n"));
    let (outcome, reason) = only_outcome(&done, "type");
    // Nothing was typed, so there is no `{"phase":"typed"}` line and no "typed" outcome.
    if cfg!(windows) {
        assert_eq!(outcome, "refused");
        assert_eq!(
            reason.as_deref(),
            Some("This session has no console to type into")
        );
    } else {
        assert_eq!(outcome, "failed");
        assert!(reason.is_some());
    }
    assert_eq!(
        traced(&path, done.pid),
        vec!["invoked type".to_owned(), format!("type: {outcome}")]
    );
}

#[test]
fn typing_without_a_request_fails_without_touching_anything() {
    let none = "No reply was given to type";
    let one_line = "A reply is typed as one line";
    let cases: [(Stdin, &str); 9] = [
        (Stdin::Closed, none),
        (Stdin::Bytes(b""), none),
        (Stdin::Bytes(b"\n"), none),
        (Stdin::Bytes(b"hello\n"), none),
        (Stdin::Bytes(b"{\"text\":7}\n"), none),
        (Stdin::Bytes(b"{\"text\":\"\"}\n"), none),
        (Stdin::Bytes(b"\xff\xfe\n"), none),
        // A line break would press Return before the engine looked again.
        (Stdin::Bytes(b"{\"text\":\"a\\nb\"}\n"), one_line),
        (
            Stdin::Bytes(b"{\"text\":\"rm -rf\\r\"}\nsubmit\n"),
            one_line,
        ),
    ];
    for (index, (stdin, reason)) in cases.into_iter().enumerate() {
        // This test's own pid: were the request accepted, the helper would go on to attach.
        let own = std::process::id().to_string();
        let mut args = type_args(NO_SUCH_PID).map(str::to_owned);
        args[2] = own;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let done = run(hook_command(&args), stdin);
        let (outcome, given) = only_outcome(&done, &format!("case {index}"));
        assert_eq!(outcome, "failed", "case {index}");
        assert_eq!(given.as_deref(), Some(reason), "case {index}");
    }
}

#[test]
fn console_info_for_a_process_that_does_not_exist_is_one_json_line() {
    let path = trace_file();
    let mut command = hook_command(&["console-info", "--pid", NO_SUCH_PID]);
    command
        .env("AGENTNOTCH_DEV", "1")
        .env("AGENTNOTCH_HOOK_TRACE", &path);
    let done = run(command, Stdin::Closed);
    assert_eq!(done.output.status.code(), Some(0));
    assert!(done.output.stderr.is_empty());
    let text = String::from_utf8(done.output.stdout.clone()).expect("UTF-8 on stdout");
    assert!(
        text.ends_with('\n') && text.lines().count() == 1,
        "{text:?}"
    );
    let info: agentnotch_proto::ConsoleInfo =
        serde_json::from_str(text.trim_end()).expect("a ConsoleInfo line");
    assert!(!info.attached);
    assert!(!info.elevated_target);
    assert_eq!(info.window, None);
    assert_eq!(info.title, None);
    assert!(info.processes.is_empty());
    assert_eq!(info.line_input, None);
    assert!(info.error.is_some());
    if cfg!(windows) {
        assert_eq!(
            info.error.as_deref(),
            Some("This session has no console to type into")
        );
    }
    // Every field is on the line, null or not: the app's side reads them by name.
    let value: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
    for field in [
        "attached",
        "window",
        "title",
        "processes",
        "line_input",
        "elevated_target",
        "error",
    ] {
        assert!(value.get(field).is_some(), "{field} in {text}");
    }
    assert_eq!(
        traced(&path, done.pid),
        vec![
            "invoked console-info".to_owned(),
            "console-info: attached=false".to_owned()
        ]
    );
}
