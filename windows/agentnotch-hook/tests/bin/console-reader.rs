//! Test-only console reader for `tests/console_type.rs` (DESIGN-WIN §7.3); never bundled.
//!
//! The finished reader puts its console in raw mode, reads `ReadConsoleInputW` records until a
//! carriage return and writes what it received to the file named by its first argument, so the
//! typing tests can compare it with what the helper typed. Until the console tests exist it only
//! creates that file empty, so a test that runs it early fails on the comparison, not on a
//! missing file.

fn main() {
    if let Some(out) = std::env::args_os().nth(1) {
        let _ = std::fs::write(out, b"");
    }
}
