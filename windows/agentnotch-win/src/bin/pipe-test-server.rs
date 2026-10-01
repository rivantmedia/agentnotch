//! Test-only: the real hook pipe server feeding the real engine ingress, in a process of its own
//! and scripted from the command line, so the integrity-level tests can run it at another
//! integrity level than the hook (DESIGN-WIN §7.3 `win_admin.rs`) and the hermetic Claude Code
//! job can answer a real session's hooks (maintainer decision Q3). Never shipped.
//!
//! `pipe-test-server --pipe <\\.\pipe\name> [--record <file.jsonl>] [--ready <file>]
//! [--answer <Tool>=<verb>]... [--default <verb>] [--stop-file <path>] [--max-seconds <n>]
//! [--include-sdk]`
//!
//! Exit codes: 0 asked to stop (`control quit`, the stop file, the time limit), 1 it could not
//! run, 2 a wrong command line, 3 the pipe's name is taken. What it does is
//! `agentnotch_win::pipe_server::script`, which is tested on every system.

use std::io::Write;
use std::process::ExitCode;

use agentnotch_win::pipe_server::script::{self, Args};

fn complain(why: &str) {
    let _ = writeln!(std::io::stderr(), "pipe-test-server: {why}");
}

fn main() -> ExitCode {
    // The arguments are matched by hand, like the hook's: no parser crate in the fork crates.
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(_) => {
                complain("an argument is not Unicode");
                return ExitCode::from(script::EXIT_USAGE);
            }
        }
    }
    match script::parse_args(&args) {
        Ok(args) => ExitCode::from(run(&args)),
        Err(why) => {
            complain(&why);
            let _ = writeln!(std::io::stderr(), "{}", script::USAGE);
            ExitCode::from(script::EXIT_USAGE)
        }
    }
}

#[cfg(windows)]
fn run(args: &Args) -> u8 {
    use agentnotch_win::pipe_server::PipeServer;
    use std::sync::Arc;

    script::serve(args, Arc::new(PipeServer::new()), &mut complain)
}

/// The exe builds everywhere so the workspace does; there is no pipe to serve elsewhere.
#[cfg(not(windows))]
fn run(_args: &Args) -> u8 {
    complain("the hook pipe exists on Windows only");
    script::EXIT_FAILED
}
