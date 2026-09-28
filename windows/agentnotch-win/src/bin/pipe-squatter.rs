//! Test-only: creates the hook pipe's name first, as another local user would, with an Everyone
//! DACL, and records every byte it receives, so `win_admin.rs` can prove the hook refuses to write
//! to it (DESIGN-WIN §7.3; WP1); never shipped. Not implemented in this build.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "pipe-squatter: not implemented in this build"
    );
    ExitCode::from(1)
}
