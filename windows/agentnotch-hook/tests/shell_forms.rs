//! The commands the installer writes into `settings.json`, run the way Claude Code runs them
//! (DESIGN-WIN §4.3, §7.3).
//!
//! A string command goes through whichever shell Claude Code uses, Git Bash or PowerShell, so it
//! is written with no quotes and only characters both read the same: the exe's path with forward
//! slashes, or its 8.3 form when the long one holds a space. Each form is run through both
//! shells here, for three profile folders (plain, with a space, not ASCII), and must reach the
//! exe with argv `["hook"]`; the exec form, which Claude Code starts itself with no window, with
//! `["hook", "--exec"]`; the status line's command with `["statusline"]`. What the exe was given
//! is read from its trace, and for a hook from the message the app received.
//!
//! The strings are built by `common::string_form_path`, which writes the design's rule out: the
//! engine's builder (`hooks::commands`, WP2) is not on this branch. At the merge these tests
//! switch to it.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use common::{
    assert_silent_success, begin, fixture, short_path, spawn, string_form_path, temp_folder,
    trace_file, trace_lines, traced, unique_pipe, unquoted, with_hook_env, Harness, Shell,
    TempFolder, EXE,
};
use serde_json::{json, Value};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

/// Where the installer puts the exe inside a config folder.
const INSTALLED: &str = r".claude\hooks\agentnotch-hook.exe";

/// A user profile with a copy of the exe where the installer would put it.
struct Profile {
    kind: &'static str,
    /// The copy's path as Windows gives it: backslashes, long names.
    exe: PathBuf,
}

fn profiles(folder: &TempFolder) -> [Profile; 3] {
    [
        ("plain", "jsmith"),
        ("with a space", "John Smith"),
        ("not ASCII", "J\u{f6}rg-\u{141}ukasz-\u{674e}\u{96f7}"),
    ]
    .map(|(kind, user)| Profile {
        kind,
        exe: folder.copy_exe(EXE, &format!(r"{user}\{INSTALLED}")),
    })
}

impl Profile {
    /// The exe's path as a string command carries it, checked against what the design says this
    /// kind of folder gets. `None`: this folder has no string form on this volume.
    fn string_path(&self) -> Option<String> {
        let long = self
            .exe
            .to_str()
            .expect("a Unicode path")
            .replace('\\', "/");
        let form = string_form_path(&self.exe);
        if self.kind != "with a space" {
            assert!(
                unquoted(&long),
                "{}: the temporary folder itself can't be written without quotes: {long}",
                self.kind
            );
            assert_eq!(form.as_deref(), Some(long.as_str()), "{}", self.kind);
            if self.kind == "not ASCII" {
                assert!(!long.is_ascii(), "{long}");
            }
            return form;
        }
        assert!(!unquoted(&long), "{long}");
        let short = short_path(&self.exe).expect("GetShortPathNameW");
        if short.contains(' ') {
            // 8.3 names are off on this volume, so the short path is the long one again. The
            // design gives such a folder no string form (exec form or "can't be hooked here").
            eprintln!(
                "shell_forms: NOTICE: no 8.3 names on the volume of {}; the 8.3 form is not run",
                self.exe.display()
            );
            assert_eq!(form, None, "{short}");
            return None;
        }
        let form = form.expect("an 8.3 path is a string form");
        assert_eq!(form, short.replace('\\', "/"));
        assert!(form.contains('~') && !form.contains(' '), "{form}");
        assert!(
            form.to_ascii_lowercase().ends_with(".exe"),
            "the short name keeps the extension: {form}"
        );
        Some(form)
    }
}

// ---- the rule ----

#[test]
fn only_what_both_shells_read_alike_goes_unquoted() {
    let _test = begin("only_what_both_shells_read_alike_goes_unquoted");
    for path in [
        "C:/Users/me/.claude/hooks/agentnotch-hook.exe",
        "C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE",
        "C:/Users/J\u{f6}rg/.claude-work_2/hooks/agentnotch-hook.exe",
        "D:/\u{674e}\u{96f7}/x.exe",
    ] {
        assert!(unquoted(path), "{path}");
    }
    for path in [
        "",
        "C:/Users/John Smith/.claude/hooks/agentnotch-hook.exe",
        r"C:\Users\me\.claude\hooks\agentnotch-hook.exe",
        "~/.claude/hooks/agentnotch-hook.exe",
        "C:/Users/me(1)/x.exe",
        "C:/Users/a&b/x.exe",
        "C:/Users/$me/x.exe",
        "C:/Users/it's/x.exe",
        "C:/Users/a;b/x.exe",
        "C:/Users/a`b/x.exe",
        "C:/Users/a\"b/x.exe",
        "C:/Users/a#b/x.exe",
        "C:/Users/a,b/x.exe",
    ] {
        assert!(!unquoted(path), "{path}");
    }
}

// ---- string form ----

#[test]
fn the_string_command_reaches_the_hook_through_git_bash() {
    let _test = begin("the_string_command_reaches_the_hook_through_git_bash");
    the_string_command_reaches_the_hook(Shell::GitBash);
}

#[test]
fn the_string_command_reaches_the_hook_through_powershell() {
    let _test = begin("the_string_command_reaches_the_hook_through_powershell");
    the_string_command_reaches_the_hook(Shell::PowerShell);
}

fn the_string_command_reaches_the_hook(shell: Shell) {
    let mut app = Harness::start("string-form");
    let folder = temp_folder("string-form");
    let stdin = fixture("stdin/pre_tool_use.json");
    for profile in profiles(&folder) {
        let Some(path) = profile.string_path() else {
            continue;
        };
        let what = format!("{shell:?}, {}: {path} hook", profile.kind);
        let trace = trace_file();
        let mut command = with_hook_env(shell.command(&format!("{path} hook")), &app.pipe);
        command.env("AGENTNOTCH_HOOK_TRACE", &trace);
        let done = spawn(command, &stdin).finish();
        assert_silent_success(&done, &what);

        let (frame, _) = app.wait_frame();
        let message: Value = serde_json::from_slice(&frame.bytes).expect("a JSON message");
        assert_eq!(message["event"], json!("PreToolUse"), "{what}");
        assert_eq!(message["tool"], json!("Bash"), "{what}");
        // The environment went through the shell with it.
        assert_eq!(message["pid"], json!(std::process::id()), "{what}");
        let hook_pid = message["hook_pid"].as_u64().expect("the hook's own pid") as u32;
        assert_ne!(hook_pid, done.pid, "{what}: the shell ran the exe");
        let lines = traced(&trace, hook_pid);
        assert_eq!(lines[0], "invoked hook exec=false", "{what}: {lines:?}");
        assert!(
            lines
                .last()
                .is_some_and(|last| last.starts_with("sent PreToolUse ")),
            "{what}: {lines:?}"
        );
    }
}

// ---- exec form ----

/// Claude Code starts an exec-form hook itself: `CreateProcessW` on the path as it is (spaces
/// and all), no window, stdin a pipe.
#[test]
fn the_exec_form_reaches_the_hook_with_exec() {
    let _test = begin("the_exec_form_reaches_the_hook_with_exec");
    let mut app = Harness::start("exec-form");
    let folder = temp_folder("exec-form");
    let stdin = fixture("stdin/pre_tool_use.json");
    for profile in profiles(&folder) {
        let what = format!("{}: {}", profile.kind, profile.exe.display());
        let trace = trace_file();
        let mut command = Command::new(&profile.exe);
        command
            .args(["hook", "--exec"])
            .creation_flags(CREATE_NO_WINDOW);
        let mut command = with_hook_env(command, &app.pipe);
        command
            .env_remove("CLAUDE_PID")
            .env("AGENTNOTCH_HOOK_TRACE", &trace);
        let done = spawn(command, &stdin).finish();
        assert_silent_success(&done, &what);

        let (frame, _) = app.wait_frame();
        let message: Value = serde_json::from_slice(&frame.bytes).expect("a JSON message");
        assert_eq!(message["event"], json!("PreToolUse"), "{what}");
        assert_eq!(message["hook_pid"], json!(done.pid), "{what}");
        // Only `--exec` makes the hook take its parent for Claude Code.
        assert_eq!(message["pid"], json!(std::process::id()), "{what}");
        let lines = traced(&trace, done.pid);
        assert_eq!(lines[0], "invoked hook exec=true", "{what}: {lines:?}");
    }
}

// ---- the status line ----

#[test]
fn the_status_line_command_reaches_the_wrapper_through_git_bash() {
    let _test = begin("the_status_line_command_reaches_the_wrapper_through_git_bash");
    the_status_line_command_reaches_the_wrapper(Shell::GitBash);
}

#[test]
fn the_status_line_command_reaches_the_wrapper_through_powershell() {
    let _test = begin("the_status_line_command_reaches_the_wrapper_through_powershell");
    the_status_line_command_reaches_the_wrapper(Shell::PowerShell);
}

/// No app listens and the folder holds no previous status line, so the wrapper has nothing to
/// send or to chain: it prints nothing and exits 0. That it was the wrapper is in its trace.
fn the_status_line_command_reaches_the_wrapper(shell: Shell) {
    let folder = temp_folder("status-line");
    let stdin = fixture("stdin/status_line.json");
    for profile in profiles(&folder) {
        let Some(path) = profile.string_path() else {
            continue;
        };
        let what = format!("{shell:?}, {}: {path} statusline", profile.kind);
        let trace = trace_file();
        let line = format!("{path} statusline");
        let mut command = with_hook_env(shell.command(&line), &unique_pipe("status-line"));
        command.env("AGENTNOTCH_HOOK_TRACE", &trace);
        let done = spawn(command, &stdin).finish();
        assert_silent_success(&done, &what);

        let lines = trace_lines(&trace);
        let Some((wrapper, first)) = lines.first() else {
            panic!("{what}: the exe never ran");
        };
        assert_eq!(first, "invoked statusline", "{what}: {lines:?}");
        assert_ne!(*wrapper, done.pid, "{what}: the shell ran the exe");
        assert!(
            lines.iter().all(|(pid, _)| pid == wrapper),
            "{what}: more than one run: {lines:?}"
        );
    }
}
