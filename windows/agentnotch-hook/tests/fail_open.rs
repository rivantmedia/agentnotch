//! The exe fails open on every system (DESIGN-WIN §1.8): whatever it is given, it exits 0 with
//! nothing on stdout, because Claude Code treats a hook that exits 2 as a block and shows what a
//! hook prints. No app listens in these tests, so every hook run takes the "no app" path; what a
//! run did is read from the dev-only trace.
//!
//! Also the exe-level half of `Fix_ScriptSocketOverrideTests.aLeftoverExportIsIgnored`: the
//! development switches (`AGENTNOTCH_SOCKET`, `AGENTNOTCH_HOOK_TRACE`) do nothing without
//! `AGENTNOTCH_DEV=1`. The pipe name rule itself is pinned in `agentnotch-proto`
//! (`dev_pipe_override`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::PathBuf;
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
    let mut command = Command::new(EXE);
    command.args(args);
    for name in [
        "CLAUDE_PID",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_ENTRYPOINT",
        "AGENTNOTCH_DEV",
        "AGENTNOTCH_HOOK_TRACE",
        "AGENTNOTCH_HOOK_TEST_PANIC",
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
    let started = Instant::now();
    while child
        .try_wait()
        .expect("the hook exe can be waited for")
        .is_none()
    {
        if started.elapsed() > HARD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the hook exe hung");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Run {
        output: child.wait_with_output().expect("the hook exe's output"),
        pid,
    }
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
        (&["statusline"], &["invoked statusline"]),
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
