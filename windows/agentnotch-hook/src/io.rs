//! Standard streams and the environment, the fail-open way (DESIGN-WIN §1.8).

use std::io::{Read, Write};

/// Writes `bytes` to stdout and flushes, ignoring every error.
///
/// Claude Code may close its end of the pipe before a hook answers (the user answered in the
/// terminal, or the session ended); `println!` would panic there and the panic would become exit
/// code 101. A lost answer is harmless: Claude Code's own prompt decides.
///
/// The bytes go out as they are: UTF-8, no byte order mark, no newline translation.
pub fn write_stdout(bytes: &[u8]) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(bytes);
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

/// An environment variable as text, `None` when unset. A value that is not valid Unicode is
/// kept, with the offending units replaced, rather than treated as unset: `CLAUDE_CONFIG_DIR`
/// names the account, and an unset one means the default account.
pub fn env_text(name: &str) -> Option<String> {
    std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}
