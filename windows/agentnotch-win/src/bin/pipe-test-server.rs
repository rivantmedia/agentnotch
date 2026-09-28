//! Test-only: the real hook pipe server in a process of its own, so the integrity-level tests can
//! run it at medium integrity against a high-integrity hook and the other way round (DESIGN-WIN
//! §7.3 `win_admin.rs`; WP1); never shipped. Not implemented in this build.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "pipe-test-server: not implemented in this build"
    );
    ExitCode::from(1)
}
