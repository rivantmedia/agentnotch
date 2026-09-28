//! Test-only stand-in for `claude.exe` (DESIGN-WIN §2.2, §7.3, §7.5; WP4); never shipped.
//!
//! The finished fake answers `--version` with `$FAKE_CLAUDE_VERSION`, speaks the usage probe's
//! stream-json from the fixture named by `$FAKE_CLAUDE_USAGE`, echoes argv as JSON when asked,
//! can spawn a grandchild, and logs each run (argv, cwd, the names of `CLAUDE*`/`ANTHROPIC*`
//! variables it received) to `$FAKE_CLAUDE_LOG`. This build answers `--version` only and fails
//! anything else, so no test can mistake it for a working probe.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        let version =
            std::env::var("FAKE_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.282 (Claude Code)".into());
        let _ = writeln!(std::io::stdout(), "{version}");
        return ExitCode::SUCCESS;
    }
    let _ = writeln!(
        std::io::stderr(),
        "fake-claude: only --version is implemented in this build"
    );
    ExitCode::from(1)
}
