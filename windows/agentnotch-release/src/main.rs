//! `agentnotch-release`: the Windows update key, signature and feed (DESIGN-WIN §6.4).
//!
//! | Subcommand | Input | Output |
//! |---|---|---|
//! | `derive-public` | seed on stdin | `{"pubkey","key_id","minisign_public_key"}` |
//! | `sign --file F --version V --out S [--timestamp UNIX]` | seed on stdin | writes S; `{"key_id","trusted_comment"}` |
//! | `verify --pubkey P --file F --sig S --version V` | — | exit 0, or 1 with the reason |
//! | `feed --version V --tag T --repo R --installer NAME --sig S --pubkey P --file F --notes-url U --out J [--pub-date RFC3339]` | — | writes and re-verifies `latest.json` |
//! | `key-id-of-feed J` / `key-id-of-pubkey P` | — | the 16-hex key id |
//!
//! The seed (the Sparkle key's, base64) is only ever read from stdin, never from argv, never
//! printed, and zeroised after use: the release jobs pipe it in from a secret held in one step's
//! environment.
//!
//! This build knows the subcommands but implements none of them yet, and fails every one with
//! exit 1 before reading anything, so a release run can never publish an unsigned or unchecked
//! feed through it. An unknown subcommand is a usage error (exit 2).

use std::process::ExitCode;

const SUBCOMMANDS: [&str; 6] = [
    "derive-public",
    "sign",
    "verify",
    "feed",
    "key-id-of-feed",
    "key-id-of-pubkey",
];

const USAGE: &str = "usage: agentnotch-release <derive-public|sign|verify|feed|key-id-of-feed|key-id-of-pubkey> [options]\n\
                     (the seed, when one is needed, is read from stdin)";

fn main() -> ExitCode {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        Some(name) if SUBCOMMANDS.contains(&name) => {
            eprintln!("agentnotch-release: `{name}` is not implemented in this build");
            ExitCode::from(1)
        }
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
