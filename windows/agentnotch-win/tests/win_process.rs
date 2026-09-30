//! The real process services on Windows (DESIGN-WIN §7.3): liveness, start times, the Toolhelp
//! parent link, the image path, the token's user and elevation, against a child this test
//! spawns. The child is `cmd.exe` waiting on a pipe: a system executable that stays alive until
//! told, never a real tool.
//!
//! The runner is elevated and has one user, so these assert relative facts (the child is like
//! this process), not constants.

#![cfg(windows)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

use agentnotch_engine::platform::{Liveness, Processes};
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
