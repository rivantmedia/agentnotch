//! Test-only console reader for `tests/console_type.rs` (DESIGN-WIN §7.3); never bundled.
//!
//! It stands in for Claude Code at its prompt: the typing tests must never type into a real
//! terminal, so they start this program in a hidden console of its own (or in a pseudo console)
//! and let the helper type into that.
//!
//! `console-reader <out> [raw|cooked] [timeout ms]`
//!
//! 1. opens `CONIN$` (its standard handles may be anything) and puts the input buffer in raw
//!    mode, as Claude Code does, or in line ("cooked") mode, as a shell prompt does;
//! 2. writes `<out>.ready`: `{"pid":…,"started_ms":…,"window":…}`, what the helper is told about
//!    its target (`window` is null for a console without one);
//! 3. reads key-down records with `ReadConsoleInputW` until one carries `'\r'`, or until the
//!    timeout (10 s when not given);
//! 4. writes `<out>`: `{"text":"…","return":true|false}`, the UTF-16 units it received as UTF-8
//!    and whether Return arrived; with `"error"` when there was no console to read.
//!
//! Both files appear by rename, so a test never reads half of one. Written like the exe it shares
//! a crate with: nothing printed, nothing unwrapped, exit 0.

use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(out) = args.next().map(PathBuf::from) else {
        return;
    };
    let cooked = args.next().is_some_and(|mode| mode == "cooked");
    let timeout = args
        .next()
        .and_then(|text| text.to_str().and_then(|text| text.parse::<u64>().ok()))
        .map_or(DEFAULT_TIMEOUT, Duration::from_millis);
    let result = reader::run(&out, cooked, timeout);
    write_by_rename(&out, &result);
}

fn write_by_rename(path: &Path, value: &serde_json::Value) {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    if std::fs::write(&partial, format!("{value}\n")).is_ok() {
        let _ = std::fs::rename(&partial, path);
    }
}

#[cfg(not(windows))]
mod reader {
    use std::path::Path;
    use std::time::Duration;

    /// There is no console to read anywhere else; the tests that use this run on Windows only.
    pub fn run(_out: &Path, _cooked: bool, _timeout: Duration) -> serde_json::Value {
        serde_json::json!({"text": "", "return": false, "error": "not available on this system"})
    }
}

#[cfg(windows)]
mod reader {
    use std::mem::zeroed;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        FlushConsoleInputBuffer, GetConsoleMode, GetConsoleWindow, ReadConsoleInputW,
        SetConsoleMode, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_WINDOW_INPUT, INPUT_RECORD, KEY_EVENT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, WaitForSingleObject,
    };

    /// `<out>.ready`
    fn ready_path(out: &Path) -> PathBuf {
        let mut name = out.as_os_str().to_owned();
        name.push(".ready");
        PathBuf::from(name)
    }

    /// 1601-01-01 to 1970-01-01, in 100 ns units.
    const EPOCH_DIFFERENCE: u64 = 116_444_736_000_000_000;

    pub fn run(out: &Path, cooked: bool, timeout: Duration) -> serde_json::Value {
        let name: Vec<u16> = "CONIN$".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `name` is NUL-terminated and outlives the call; the rest are plain values.
        // not a pipe: the console input buffer
        let input = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if input.is_null() || input == INVALID_HANDLE_VALUE {
            return serde_json::json!({"text": "", "return": false, "error": "no console"});
        }

        let mut original = 0u32;
        // SAFETY: a live console input handle and a valid out pointer.
        let had_mode = unsafe { GetConsoleMode(input, &mut original) } != 0;
        let mode = if cooked {
            ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT
        } else {
            // What a raw-mode reader asks for: no line editing, no echo, no Ctrl+C processing.
            ENABLE_WINDOW_INPUT
        };
        // SAFETY: a live console input handle; plain values. Whatever was in the buffer before
        // the mode was set is not the helper's.
        let mode_set = unsafe {
            let set = SetConsoleMode(input, mode) != 0;
            FlushConsoleInputBuffer(input);
            set
        };
        if !mode_set {
            // SAFETY: the handle is ours and is not used after this.
            unsafe { CloseHandle(input) };
            return serde_json::json!({"text": "", "return": false, "error": "console mode"});
        }

        // SAFETY: return values about the calling process and its console; no arguments.
        let (pid, window) = unsafe { (GetCurrentProcessId(), GetConsoleWindow() as usize) };
        super::write_by_rename(
            &ready_path(out),
            &serde_json::json!({
                "pid": pid,
                "started_ms": started_ms(),
                "window": (window != 0).then_some(window as u64),
            }),
        );

        let (units, got_return) = read_until_return(input, Instant::now() + timeout);

        // SAFETY: the handle is ours; it is closed last and not used afterwards.
        unsafe {
            if had_mode {
                SetConsoleMode(input, original);
            }
            CloseHandle(input);
        }
        serde_json::json!({"text": String::from_utf16_lossy(&units), "return": got_return})
    }

    /// The characters of the key-down records that arrive before `deadline`, up to a `'\r'`.
    fn read_until_return(input: HANDLE, deadline: Instant) -> (Vec<u16>, bool) {
        let mut units: Vec<u16> = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return (units, false);
            }
            let wait = u32::try_from(left.as_millis())
                .unwrap_or(u32::MAX - 1)
                .max(1);
            // SAFETY: a console input handle is signalled while its buffer holds records.
            if unsafe { WaitForSingleObject(input, wait) } != WAIT_OBJECT_0 {
                continue;
            }
            // SAFETY: INPUT_RECORD is plain data; all-zero is a valid value.
            let mut records: [INPUT_RECORD; 32] = unsafe { zeroed() };
            let mut read = 0u32;
            // SAFETY: `records` holds the number of records passed; `read` is a valid out
            // pointer. The buffer is not empty (the wait said so), so this does not block.
            let ok = unsafe {
                ReadConsoleInputW(input, records.as_mut_ptr(), records.len() as u32, &mut read)
            } != 0;
            if !ok {
                return (units, false);
            }
            for record in records.iter().take(read as usize) {
                if u32::from(record.EventType) != KEY_EVENT {
                    continue;
                }
                // SAFETY: the event type says which member of the union is set.
                let key = unsafe { record.Event.KeyEvent };
                if key.bKeyDown == 0 {
                    continue;
                }
                // SAFETY: records read with the W function carry the UTF-16 member.
                let unit = unsafe { key.uChar.UnicodeChar };
                if unit == u16::from(b'\r') {
                    return (units, true);
                }
                if unit != 0 {
                    units.push(unit);
                }
            }
        }
    }

    /// When this process was created, in Unix milliseconds: the helper's `--started`.
    fn started_ms() -> Option<u64> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the pseudo-handle of the current process and four valid out pointers.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } != 0;
        let filetime =
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        ok.then(|| filetime.saturating_sub(EPOCH_DIFFERENCE) / 10_000)
    }
}
