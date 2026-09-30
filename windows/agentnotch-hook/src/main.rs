//! `agentnotch-hook.exe`: the one native helper of Agent Notch for Windows (DESIGN-WIN §1.1).
//!
//! Claude Code runs it for every hook event (`hook`, `hook --exec`) and as the account's status
//! line (`statusline`); the app runs it to type a reply into a session's console (`type`) and to
//! describe that console (`console-info`). Claude Code treats a hook that exits 2 as a block and
//! waits on one that hangs, so this program has one overriding rule (§1.8): **whatever happens,
//! it exits 0**, with empty stdout unless it has a real answer to give.
//!
//! - argv is matched by hand in `agentnotch_proto::parse_invocation`: an unknown subcommand or an
//!   extra argument is `Ignore`, never a usage error (argument-parser crates exit 2 on those, and
//!   check-seams.sh keeps them out of this crate);
//! - `main` runs everything inside `catch_unwind` with a silent panic hook and ends with
//!   `process::exit(0)` (a panic would otherwise exit 101 and print to stderr);
//! - output goes through `io::write_stdout`, which ignores errors (`println!` panics when
//!   Claude Code has already closed the pipe);
//! - a watchdog ends the process with exit 0 when talking to the app stalls (§1.4).

mod ancestry;
mod console;
mod hook;
mod io;
mod pipe;
mod statusline;
mod trace;
mod watchdog;
#[cfg(windows)]
mod win;

use agentnotch_proto::Invocation;

fn main() {
    // A panic must neither print (stderr reaches Claude Code's logs and, in some modes, the user)
    // nor change the exit code: the hook is always a no-op on failure.
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(run);
    std::process::exit(0);
}

fn run() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let invocation = agentnotch_proto::parse_invocation(&args);
    // The role only, never the arguments: they can carry a pid or a path.
    trace::note(|| match &invocation {
        Invocation::Hook { exec_form } => format!("invoked hook exec={exec_form}"),
        Invocation::StatusLine => "invoked statusline".into(),
        Invocation::Type(_) => "invoked type".into(),
        Invocation::ConsoleInfo { .. } => "invoked console-info".into(),
        Invocation::Ignore => "invoked unknown".into(),
    });
    match invocation {
        Invocation::Hook { exec_form } => hook::run(exec_form),
        Invocation::StatusLine => statusline::run(),
        Invocation::Type(target) => console::type_reply(&target),
        Invocation::ConsoleInfo { pid } => console::info(pid),
        // Anything else, including no arguments at all, is not ours to answer.
        Invocation::Ignore => {}
    }
}
