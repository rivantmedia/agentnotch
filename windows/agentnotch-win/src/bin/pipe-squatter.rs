//! Test-only: creates the hook pipe's name first, as another local user would, with an Everyone
//! DACL (or, with `--descriptor app`, the app's own owner and DACL, as a sandboxed process of the
//! same user at Low integrity would), and records how many bytes each connection sent, so
//! `win_admin.rs` can prove the hook refuses to write to it (DESIGN-WIN §7.3; WP1). Never shipped.
//!
//! `pipe-squatter --pipe <\\.\pipe\name> --record <file> --ready <file> [--max-seconds <n>]
//! [--descriptor everyone|app]`
//!
//! Exit codes: 0 at the time limit, 1 it could not run, 2 a wrong command line, 3 the name was
//! taken already. The command line and the record's lines are
//! `agentnotch_win::pipe_server::squatter`, tested on every system.

use std::io::Write;
use std::process::ExitCode;

use agentnotch_win::pipe_server::squatter::{self, Args};

fn complain(why: &str) {
    let _ = writeln!(std::io::stderr(), "pipe-squatter: {why}");
}

fn main() -> ExitCode {
    // Matched by hand, like the hook's and the test server's: no parser crate in the fork crates.
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(_) => {
                complain("an argument is not Unicode");
                return ExitCode::from(squatter::EXIT_USAGE);
            }
        }
    }
    match squatter::parse_args(&args) {
        Ok(args) => ExitCode::from(run(&args)),
        Err(why) => {
            complain(&why);
            let _ = writeln!(std::io::stderr(), "{}", squatter::USAGE);
            ExitCode::from(squatter::EXIT_USAGE)
        }
    }
}

/// The exe builds everywhere so the workspace does; there is no pipe to squat on elsewhere.
#[cfg(not(windows))]
fn run(_args: &Args) -> u8 {
    complain("named pipes exist on Windows only");
    squatter::EXIT_FAILED
}

#[cfg(windows)]
fn run(args: &Args) -> u8 {
    win::run(args)
}

#[cfg(windows)]
mod win {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write};
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use agentnotch_proto::limits::PIPE_BUFFER_BYTES;
    use agentnotch_proto::pipe_sddl;
    use agentnotch_win::pipe_server::squatter::{
        connection_line, error_line, Args, Descriptor, EVERYONE_SDDL, EXIT_FAILED, EXIT_OK,
        EXIT_PIPE_IN_USE,
    };
    use agentnotch_win::sid::{self, SecurityDescriptor};
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        HANDLE,
    };
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::Storage::FileSystem::{
        ReadFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    /// One instance of the pipe; closed on drop.
    struct Instance(HANDLE);

    impl Drop for Instance {
        fn drop(&mut self) {
            // SAFETY: the handle was returned open by CreateNamedPipeW and is not used after this.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    struct Record(File);

    impl Record {
        fn write(&mut self, line: &str) -> io::Result<()> {
            self.0.write_all(line.as_bytes())?;
            self.0.flush()
        }
    }

    pub fn run(args: &Args) -> u8 {
        let limit = Duration::from_secs(args.max_seconds);
        // The main thread may sit in ConnectNamedPipe or ReadFile for good; the time limit ends
        // the process from beside it. Every record line was flushed when it was written.
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            std::process::exit(i32::from(EXIT_OK));
        });
        let mut record = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&args.record)
        {
            Ok(file) => Record(file),
            Err(error) => {
                super::complain(&format!("the record file can't be opened: {error}"));
                return EXIT_FAILED;
            }
        };
        match serve(args, &mut record) {
            Ok(()) => EXIT_OK,
            Err((code, why)) => {
                let _ = record.write(&error_line(&why));
                super::complain(&why);
                code
            }
        }
    }

    fn serve(args: &Args, record: &mut Record) -> Result<(), (u8, String)> {
        let sddl = match args.descriptor {
            Descriptor::Everyone => EVERYONE_SDDL.to_owned(),
            Descriptor::App => pipe_sddl(
                &sid::current_user_sid()
                    .ok_or_else(|| (EXIT_FAILED, "this user's SID is unreadable".to_owned()))?,
            ),
        };
        let descriptor = SecurityDescriptor::from_sddl(&sddl)
            .map_err(|why| (EXIT_FAILED, format!("the descriptor can't be built: {why}")))?;
        let mut current = create(&args.pipe, true, &descriptor).map_err(|error| {
            let code = match error.raw_os_error() {
                Some(code)
                    if code == ERROR_ACCESS_DENIED.0 as i32 || code == ERROR_PIPE_BUSY.0 as i32 =>
                {
                    EXIT_PIPE_IN_USE
                }
                _ => EXIT_FAILED,
            };
            (code, format!("CreateNamedPipeW: {error}"))
        })?;
        write_ready(&args.ready, &args.pipe).map_err(|error| {
            (
                EXIT_FAILED,
                format!("the ready file can't be written: {error}"),
            )
        })?;
        loop {
            // SAFETY: the instance is an open pipe handle; a synchronous connect.
            if let Err(error) = unsafe { ConnectNamedPipe(current.0, None) } {
                // A client that connected between create and connect is connected all the same;
                // one that also closed again already (ERROR_NO_DATA) is a connection of no bytes.
                if error.code() != ERROR_PIPE_CONNECTED.to_hresult()
                    && error.code() != ERROR_NO_DATA.to_hresult()
                {
                    return Err((
                        EXIT_FAILED,
                        format!("ConnectNamedPipe: {}", error.message()),
                    ));
                }
            }
            // The next instance exists before this one closes, so the name is never free for
            // someone else (the app) to take in between.
            let next = create(&args.pipe, false, &descriptor)
                .map_err(|error| (EXIT_FAILED, format!("CreateNamedPipeW: {error}")))?;
            let bytes = drain(&current);
            record.write(&connection_line(bytes)).map_err(|error| {
                (
                    EXIT_FAILED,
                    format!("the record file can't be written: {error}"),
                )
            })?;
            // SAFETY: the instance is an open, connected pipe handle.
            let _ = unsafe { DisconnectNamedPipe(current.0) };
            current = next;
        }
    }

    fn create(pipe: &str, first: bool, descriptor: &SecurityDescriptor) -> io::Result<Instance> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let name = HSTRING::from(pipe);
        // SAFETY: the name and the attributes (with the descriptor they point to) are alive for
        // the call; Windows copies both.
        let handle = unsafe {
            CreateNamedPipeW(
                &name,
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                Some(&attributes),
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        Ok(Instance(handle))
    }

    /// Reads until the client closes its end; how many bytes it sent.
    fn drain(instance: &Instance) -> u64 {
        let mut total = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            // SAFETY: the instance is an open, connected pipe handle; `buffer` and `read` are
            // alive for the synchronous call.
            let result = unsafe { ReadFile(instance.0, Some(&mut buffer), Some(&mut read), None) };
            total += u64::from(read);
            // ERROR_BROKEN_PIPE (the client closed) and every other failure end the connection;
            // so does a read of nothing, which a byte-mode pipe only gives at its end.
            if result.is_err() || read == 0 {
                return total;
            }
        }
    }

    /// The ready file, written under another name first so whoever waits never reads half.
    fn write_ready(path: &Path, pipe: &str) -> io::Result<()> {
        let mut staging = path.as_os_str().to_owned();
        staging.push(".tmp");
        let staging = PathBuf::from(staging);
        fs::write(&staging, format!("{pipe}\n"))?;
        fs::rename(&staging, path)
    }
}
