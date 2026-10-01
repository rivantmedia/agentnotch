//! Test-only stand-in for the process that runs a hook (`tests/hook_hygiene.rs`); never bundled.
//!
//! The hook works out which process is Claude Code from its ancestors' image names, so a test
//! needs a parent called `claude.exe` (or `code.exe`) that is not Claude Code: tests copy this
//! exe to a temporary folder under that name and run the copy by its full path. It runs the
//! command line it is given with its own stdin, stdout and stderr, stays alive until that ends
//! (the hook looks its ancestors up while it runs) and exits with the same code.

use std::process::Command;

/// No command was given (sysexits' EX_USAGE).
const USAGE: i32 = 64;
/// The command could not be started (a shell's "command not found").
const NOT_STARTED: i32 = 127;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(program) = args.next() else {
        std::process::exit(USAGE);
    };
    let code = match Command::new(program).args(args).status() {
        // No code: ended by a signal, which only other systems have.
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => NOT_STARTED,
    };
    std::process::exit(code);
}
