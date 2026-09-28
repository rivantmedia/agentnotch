//! Standard streams, the fail-open way (DESIGN-WIN §1.8).

use std::io::{Read, Write};

/// Writes `text` to stdout and flushes, ignoring every error.
///
/// Claude Code may close its end of the pipe before a hook answers (the user answered in the
/// terminal, or the session ended); `println!` would panic there and the panic would become exit
/// code 101. A lost answer is harmless: Claude Code's own prompt decides.
pub fn write_stdout(text: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(text.as_bytes());
    let _ = lock.flush();
}

/// Reads stdin to its end, or up to `limit` bytes; `None` when it can't be read or is larger.
///
/// Claude Code closes stdin after writing the event, so this returns promptly; the bound keeps a
/// runaway writer from growing the process without limit (the message is truncated to fit the
/// protocol's limits afterwards anyway, HS§1.5).
pub fn read_stdin(limit: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let read = std::io::stdin()
        .lock()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (read <= limit).then_some(bytes)
}
