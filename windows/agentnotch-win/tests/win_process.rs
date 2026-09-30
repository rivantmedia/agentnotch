//! The real process services on Windows (DESIGN-WIN §7.3): liveness, start times, the Toolhelp
//! parent link, the image path, the token's user and elevation, and the read of
//! `CLAUDE_CONFIG_DIR` from a process's environment block, against a child this test spawns. The child is `cmd.exe` waiting on a pipe: a system executable that stays alive until
//! told, never a real tool.
//!
//! The runner is elevated and has one user, so these assert relative facts (the child is like
//! this process), not constants.

#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

use agentnotch_engine::platform::{EnvRead, Liveness, Processes};
use agentnotch_win::process::WinProcesses;

/// The system clock that stamps a process's creation advances with the timer interrupt (about
/// 16 ms at the default rate), while `SystemTime::now` reads the precise clock: a child can be
/// stamped slightly before the "before" reading taken just ahead of it.
const CLOCK_SLACK: Duration = Duration::from_millis(100);

/// `set /p` reads a line from stdin, so the child lives until its stdin pipe is dropped.
fn waiting_child() -> Child {
    Command::new("cmd.exe")
        .args(["/D", "/C", "set /p x="])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cmd.exe starts")
}

/// Closes the child's stdin and waits for it. The `Child` keeps its handle, so the pid is not
/// reused while the caller still holds it.
fn finish(child: &mut Child) {
    drop(child.stdin.take());
    child.wait().expect("the child exits");
}

#[test]
fn a_running_child_is_alive_with_a_start_time_from_its_spawn() {
    let processes = WinProcesses::new();
    let before = SystemTime::now();
    let mut child = waiting_child();
    let after = SystemTime::now();
    let pid = child.id();

    assert_eq!(processes.liveness(pid), Liveness::Alive);
    let started = processes
        .start_time(pid)
        .expect("a running child has a start time");
    assert!(
        started >= before - CLOCK_SLACK && started <= after,
        "started {started:?} outside {before:?}..{after:?}"
    );
    // The same process gives the same answer every time: it is what a pid is paired with.
    assert_eq!(processes.start_time(pid), Some(started));

    finish(&mut child);
}

#[test]
fn an_exited_child_is_gone_and_never_gets_another_start_time() {
    let processes = WinProcesses::new();
    let mut child = waiting_child();
    let pid = child.id();
    let started = processes
        .start_time(pid)
        .expect("a running child has a start time");

    finish(&mut child);

    assert_eq!(processes.liveness(pid), Liveness::Gone);
    let afterwards = processes.start_time(pid);
    assert!(
        afterwards.is_none() || afterwards == Some(started),
        "an exited child's start time became {afterwards:?} (was {started:?})"
    );
}

#[test]
fn this_process_is_alive_and_pid_zero_is_unknown() {
    let processes = WinProcesses::new();
    let own = std::process::id();
    assert_eq!(processes.liveness(own), Liveness::Alive);
    assert!(processes.start_time(own).expect("own start time") <= SystemTime::now());
    assert_eq!(processes.liveness(0), Liveness::Unknown);
    assert_eq!(processes.start_time(0), None);
}

#[test]
fn the_table_links_a_child_to_this_process() {
    let processes = WinProcesses::new();
    let mut child = waiting_child();
    let pid = child.id();
    let own = std::process::id();

    let table = processes.table();
    let entry = table.get(pid).expect("the child is in the table");
    assert_eq!(entry.ppid, own);
    assert!(
        entry.exe_name.eq_ignore_ascii_case("cmd.exe"),
        "{}",
        entry.exe_name
    );
    assert_eq!(entry.started, processes.start_time(pid));

    let parent = table.get(own).expect("this process is in the table");
    assert_eq!(parent.started, processes.start_time(own));
    let (parent_started, child_started) = (
        parent.started.expect("own start time"),
        entry.started.expect("the child's start time"),
    );
    assert!(parent_started <= child_started);
    // Which is what makes the link a valid one.
    assert_eq!(table.parent(pid).map(|p| p.pid), Some(own));
    assert!(table
        .ancestors(pid, 8)
        .first()
        .is_some_and(|p| p.pid == own));

    finish(&mut child);
}

#[test]
fn the_image_path_the_user_and_the_elevation_of_a_child() {
    let processes = WinProcesses::new();
    let mut child = waiting_child();
    let pid = child.id();
    let own = std::process::id();

    let path = processes.exe_path(pid).expect("the child's image path");
    assert!(path.is_absolute(), "{}", path.display());
    assert!(
        path.to_string_lossy().to_lowercase().ends_with("cmd.exe"),
        "{}",
        path.display()
    );
    let own_path = processes.exe_path(own).expect("own image path");
    let current = std::env::current_exe().expect("current_exe");
    assert_eq!(
        own_path.file_name().map(|n| n.to_ascii_lowercase()),
        current.file_name().map(|n| n.to_ascii_lowercase())
    );

    assert_eq!(processes.same_user(pid), Some(true));
    assert_eq!(processes.same_user(own), Some(true));

    let own_elevation = processes.elevated(own);
    assert!(
        own_elevation.is_some(),
        "this process's own token can be read"
    );
    assert_eq!(processes.elevated(pid), own_elevation);

    finish(&mut child);
}

#[test]
fn a_pid_nobody_has_answers_nothing() {
    let processes = WinProcesses::new();
    // Pids are multiples of four; this one is no process's.
    let nobody = u32::MAX - 2;
    assert_eq!(processes.liveness(nobody), Liveness::Gone);
    assert_eq!(processes.start_time(nobody), None);
    assert_eq!(processes.exe_path(nobody), None);
    assert_eq!(processes.same_user(nobody), None);
    assert_eq!(processes.elevated(nobody), None);
    assert!(processes.table().get(nobody).is_none());
}

// The environment read (DESIGN-WIN §4.2, AU§14.1). These prove the x64 offsets of the
// environment block on the Windows the runner has.

const VARIABLE: &str = "CLAUDE_CONFIG_DIR";

/// A waiting child whose environment `configure` has set up, read only once it has printed a
/// line: by then the process has initialised and `cmd.exe` has made the block its own (it adds
/// its per-drive entries), which is the state a real session's process is in.
fn child_with_env(configure: impl FnOnce(&mut Command)) -> Child {
    let mut command = Command::new("cmd.exe");
    command
        .args(["/D", "/C", "echo ready& set /p x="])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure(&mut command);
    let mut child = command.spawn().expect("cmd.exe starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("the child's stdout"))
        .read_line(&mut line)
        .expect("the child prints a line");
    assert_eq!(line.trim(), "ready");
    child
}

fn set(value: &str) -> EnvRead {
    EnvRead::Set(value.to_owned())
}

#[test]
fn a_childs_config_dir_is_read_from_its_environment() {
    let processes = WinProcesses::new();
    let value = r"C:\Users\me\.claude-work";
    let mut child = child_with_env(|command| {
        command.env(VARIABLE, value);
    });
    let pid = child.id();

    assert_eq!(processes.config_dir_env(pid), set(value));
    // Reading changes nothing: the same answer again.
    assert_eq!(processes.config_dir_env(pid), set(value));

    finish(&mut child);
}

#[test]
fn a_child_without_the_variable_is_unset() {
    let processes = WinProcesses::new();
    let mut child = child_with_env(|command| {
        command.env_remove(VARIABLE);
    });

    assert_eq!(processes.config_dir_env(child.id()), EnvRead::Unset);

    finish(&mut child);
}

#[test]
fn the_variables_name_is_matched_without_regard_to_case() {
    let processes = WinProcesses::new();
    let value = r"D:\profiles\Mixed Case";
    // The name goes into the child's block as written here.
    let mut child = child_with_env(|command| {
        command.env_remove(VARIABLE).env("Claude_Config_Dir", value);
    });

    assert_eq!(processes.config_dir_env(child.id()), set(value));

    finish(&mut child);
}

#[test]
fn the_variable_is_found_after_100_kib_of_other_variables() {
    let processes = WinProcesses::new();
    let value = r"C:\Users\me\.claude-far";
    // 110 variables of 1,000 characters: more than 100 KiB of characters, twice that in bytes.
    // The block a child gets is sorted by name, so these all lie before the variable.
    let padding = "p".repeat(1000);
    let mut child = child_with_env(|command| {
        for index in 0..110 {
            command.env(format!("AAA_PAD_{index:03}"), &padding);
        }
        command.env(VARIABLE, value);
    });
    let pid = child.id();

    assert_eq!(processes.config_dir_env(pid), set(value));
    // The first 100 KiB alone do not reach the end of the block: that is not "unset".
    assert_eq!(
        WinProcesses::reading_at_most(100 * 1024).config_dir_env(pid),
        EnvRead::Unreadable
    );

    finish(&mut child);
}

#[test]
fn a_read_cut_short_is_unreadable_never_unset() {
    let mut without = child_with_env(|command| {
        command.env_remove(VARIABLE);
    });
    let mut with = child_with_env(|command| {
        command.env(VARIABLE, r"C:\Users\me\.claude-cut");
    });

    // The whole block says "unset" and "set"; any shorter read of either says neither.
    assert_eq!(
        WinProcesses::new().config_dir_env(without.id()),
        EnvRead::Unset
    );
    assert_eq!(
        WinProcesses::new().config_dir_env(with.id()),
        set(r"C:\Users\me\.claude-cut")
    );
    for bytes in [0, 1, 2, 64, 65, 200, 1024] {
        let cut = WinProcesses::reading_at_most(bytes);
        assert_eq!(
            cut.config_dir_env(without.id()),
            EnvRead::Unreadable,
            "{bytes} bytes of a block without the variable"
        );
        assert_eq!(
            cut.config_dir_env(with.id()),
            EnvRead::Unreadable,
            "{bytes} bytes of a block with the variable"
        );
    }
    // A limit beyond the block cuts nothing.
    assert_eq!(
        WinProcesses::reading_at_most(usize::MAX).config_dir_env(without.id()),
        EnvRead::Unset
    );

    finish(&mut without);
    finish(&mut with);
}

#[test]
fn an_exited_childs_environment_is_unreadable() {
    let processes = WinProcesses::new();
    let mut child = child_with_env(|command| {
        command.env(VARIABLE, r"C:\Users\me\.claude-gone");
    });
    let pid = child.id();
    assert_eq!(
        processes.config_dir_env(pid),
        set(r"C:\Users\me\.claude-gone")
    );

    finish(&mut child);

    assert_eq!(processes.config_dir_env(pid), EnvRead::Unreadable);
    // And so are pid 0 and a pid nobody has.
    assert_eq!(processes.config_dir_env(0), EnvRead::Unreadable);
    assert_eq!(processes.config_dir_env(u32::MAX - 2), EnvRead::Unreadable);
}

#[test]
fn a_value_outside_ascii_round_trips() {
    let processes = WinProcesses::new();
    // Two-unit characters too (U+1F4C1 is a surrogate pair).
    let value = "C:\\Users\\Zo\u{eb}\\.claude-\u{65e5}\u{672c}-\u{1f4c1}";
    let mut child = child_with_env(|command| {
        command.env(VARIABLE, value);
    });

    assert_eq!(processes.config_dir_env(child.id()), set(value));

    finish(&mut child);
}

#[test]
fn this_processs_own_environment_reads_like_std_does() {
    let expected = match std::env::var(VARIABLE) {
        Ok(value) if !value.is_empty() => EnvRead::Set(value),
        _ => EnvRead::Unset,
    };
    assert_eq!(
        WinProcesses::new().config_dir_env(std::process::id()),
        expected
    );
}
