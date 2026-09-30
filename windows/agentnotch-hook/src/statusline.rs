//! `statusline`: the account's status line command (DESIGN-WIN §1.4, §4.3; the Mac's
//! agentnotch-statusline.py).
//!
//! Claude Code runs this after each assistant message with the session's status JSON on stdin.
//! The wrapper
//!
//! - starts the status line command that was configured before the app took the setting over
//!   (saved beside this exe in `agentnotch-statusline.previous.json`), with the same stdin;
//! - while that runs, sends a small part of the status (rate limits, context window, model,
//!   cost, name, Claude Code's pid) to the app, on a thread of its own with 0.3 s to spend;
//! - then passes the previous command's output and exit code straight through. No previous
//!   command: no output.
//!
//! It must never break the user's status line. Every failure of our own ends as exit 0 with
//! nothing printed; the app costs at most the send's budget, once; the previous command gets as
//! long as it needs, up to a cap that only stops one that hangs. This is the one role whose exit
//! code is not always 0: the previous command's own code is Claude Code's to see, as it was
//! before the wrapper stood in front of it.
//!
//! The previous command runs as `bash.exe -c "<command>"` through Git Bash, found again on every
//! run, inside a Job object (`job.rs`), so nothing it started outlives the wrapper. Two rules
//! keep a wrapper from ever starting a wrapper: the saved command is refused when it names one
//! (`agentnotch_proto::statusline::previous_command`), and the command's environment carries
//! `AGENTNOTCH_STATUSLINE_DEPTH`, which makes a wrapper reached under any other spelling chain
//! nothing.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use agentnotch_proto::limits::{
    MAX_CLIENT_MESSAGE, PREVIOUS_STATUS_LINE_TIMEOUT_MS, STATUS_LINE_SEND_BUDGET_MS,
};
use agentnotch_proto::statusline::{previous_command, DEPTH_ENV, PREVIOUS_FILE_NAME};
use agentnotch_proto::{build_statusline_message, HookEnv};
use serde_json::Value;

use crate::job::{self, Job};
use crate::{hook, io, pipe, trace};

/// Claude Code's status JSON is a few KiB; anything far larger is not one.
const STDIN_LIMIT: usize = 8 << 20;
/// The saved `statusLine` object is a line or two of JSON.
const PREVIOUS_FILE_LIMIT: usize = 1 << 20;
/// A status line is a few lines of text. More than this is kept from growing the wrapper, and
/// the rest is read and dropped so the command can still end.
const OUTPUT_LIMIT: usize = 8 << 20;
/// The send's whole budget, connect included.
const SEND_BUDGET: Duration = Duration::from_millis(STATUS_LINE_SEND_BUDGET_MS);
/// After the previous command was cut off, how long its shell gets to be gone.
const CUT_OFF_WAIT: Duration = Duration::from_secs(1);

/// Runs the wrapper; the code the process exits with.
pub fn run() -> i32 {
    // Unreadable stdin still chains, with nothing to pass on: the status line the user had
    // does not depend on us being able to use what Claude Code sent.
    let status = io::read_stdin(STDIN_LIMIT).unwrap_or_else(|| {
        trace::note(|| "stdin: unreadable or too large".into());
        Vec::new()
    });

    // The previous command first, so it runs while the app is told. No watchdog here: the
    // wrapper's bound is the previous command's cap, and the send has its own.
    let previous = Previous::start(&status);
    hook::forced_panic_for_tests();
    if let Some(sending) = Sending::start(&status) {
        sending.wait();
    }
    previous.map_or(0, Previous::finish)
}

// ---- the previous command ----

/// The previous status line command while it runs.
struct Previous {
    child: Child,
    /// `None` when Windows would not make a job or take the process into it: the command still
    /// runs, and only its shell can be ended.
    job: Option<Job>,
    /// Everything the command printed, once it closed its stdout.
    output: mpsc::Receiver<Vec<u8>>,
    /// When a command that has not finished is cut off.
    deadline: Instant,
}

impl Previous {
    /// Starts the command this wrapper replaced, feeding it `status`; `None` when there is
    /// nothing to chain or it could not be started.
    fn start(status: &[u8]) -> Option<Previous> {
        let command = command_to_chain()?;
        let Some(bash) = find_git_bash() else {
            trace::note(|| "chain: no git bash".into());
            return None;
        };
        let job = Job::new();
        let deadline = Instant::now() + previous_timeout();
        let spawned = shell(&bash, &command)
            .env(DEPTH_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn();
        let Ok(mut child) = spawned else {
            trace::note(|| "chain: not started".into());
            return None;
        };
        // Right after the start: the shell has not run an instruction of its own yet, so
        // everything it starts is started inside the job.
        let job = job.filter(|job| job.adopt(&child));
        trace::note(|| {
            if job.is_some() {
                "chain: started".into()
            } else {
                "chain: started without a job".into()
            }
        });

        // Both pipes are served from threads: a command that prints before it has read its
        // input, or reads none of it, would otherwise block against a full pipe. When a thread
        // cannot be started its pipe closes, which the command sees as the end of its input,
        // and `finish` as no output.
        if let Some(mut pipe) = child.stdin.take() {
            let status = status.to_vec();
            let _ = thread::Builder::new()
                .name("previous-stdin".into())
                .spawn(move || {
                    // A command that does not read its input is fine.
                    let _ = pipe.write_all(&status);
                });
        }
        let (tell, output) = mpsc::channel();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = thread::Builder::new()
                .name("previous-stdout".into())
                .spawn(move || {
                    let mut printed = Vec::new();
                    let read = pipe
                        .by_ref()
                        .take(OUTPUT_LIMIT as u64)
                        .read_to_end(&mut printed);
                    if read.is_ok() {
                        let _ = std::io::copy(&mut pipe, &mut std::io::sink());
                        let _ = tell.send(printed);
                    }
                });
        }
        Some(Previous {
            child,
            job,
            output,
            deadline,
        })
    }

    /// Waits for the command, prints what it printed, and returns the code to exit with.
    fn finish(mut self) -> i32 {
        // Finished means both: its output has ended and its shell has exited. Nothing is
        // printed before that, so a command that is cut off shows nothing rather than a part.
        let Ok(printed) = self.output.recv_timeout(self.left()) else {
            return self.cut_off();
        };
        let left = self.left();
        let Some(status) = job::wait_exit(&mut self.child, left) else {
            return self.cut_off();
        };
        io::write_stdout(&printed);
        let code = exit_code(status.code());
        trace::note(|| format!("chain: exit {code}"));
        // The job closes as `self` is dropped: what the command left running ends here.
        code
    }

    fn left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// The command hangs: end everything it started, print nothing, and succeed.
    fn cut_off(mut self) -> i32 {
        if let Some(job) = &self.job {
            job.end();
        }
        let _ = self.child.kill();
        let _ = job::wait_exit(&mut self.child, CUT_OFF_WAIT);
        trace::note(|| "chain: timed out".into());
        0
    }
}

/// The command to chain, or `None` (said in the trace) when this run chains nothing.
fn command_to_chain() -> Option<String> {
    // Set by the wrapper that started this command: we were reached through a wrapper, under a
    // spelling its own check could not see. Chaining again could go round for ever.
    if std::env::var_os(DEPTH_ENV).is_some() {
        trace::note(|| "chain: nested".into());
        return None;
    }
    let command = read_previous_file().and_then(|bytes| previous_command(&bytes));
    if command.is_none() {
        trace::note(|| "chain: nothing".into());
    }
    command
}

/// The saved `statusLine` object beside this exe; `None` when there is none to read.
fn read_previous_file() -> Option<Vec<u8>> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(PREVIOUS_FILE_NAME);
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(PREVIOUS_FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= PREVIOUS_FILE_LIMIT).then_some(bytes)
}

/// Where Git Bash may be, in the order Claude Code looks: `CLAUDE_CODE_GIT_BASH_PATH`, then the
/// two Program Files folders. The installer wrapped the previous command only when one of these
/// existed; Git may have been removed since, so they are looked up again on every run.
fn git_bash_candidates(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let set = |name: &str| get(name).filter(|value| !value.trim().is_empty());
    let mut candidates: Vec<String> = set("CLAUDE_CODE_GIT_BASH_PATH").into_iter().collect();
    for root in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = set(root) {
            let root = root.trim_end_matches(['\\', '/']);
            candidates.push(format!(r"{root}\Git\bin\bash.exe"));
        }
    }
    candidates
}

fn find_git_bash() -> Option<PathBuf> {
    git_bash_candidates(io::env_text)
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

/// `-c "<command>"` as it is written on bash.exe's command line.
///
/// Git Bash is an MSYS program: started by a Windows program, its runtime splits the command
/// line itself, and not by the C runtime's rules that `std::process::Command` quotes for. Inside
/// double quotes it reads `\\` as one backslash and `\"` as a quote wherever they stand, so
/// both are escaped everywhere (the C runtime's rule, which doubles backslashes only in front of
/// a quote, would lose half of every other pair). The space after the opening quote is for one
/// more rule of that runtime: an argument that begins with a drive letter (`C:/tools/sl.exe
/// "a b"`) is taken for a Windows path and its backslashes are not escapes at all, which would
/// undo the escaped quotes. A blank in front means nothing to bash.
// Only Windows has a command line to write; the rule is tested on every system.
#[cfg_attr(not(windows), allow(dead_code))]
fn bash_arguments(command: &str) -> String {
    let mut arguments = String::with_capacity(command.len() + 8);
    arguments.push_str("-c \" ");
    for character in command.chars() {
        if matches!(character, '\\' | '"') {
            arguments.push('\\');
        }
        arguments.push(character);
    }
    arguments.push('"');
    arguments
}

/// Git Bash running `command`, with no window of its own (the wrapper has none either: Claude
/// Code starts it hidden, and a console flashing up on every render would be ours).
#[cfg(windows)]
fn shell(bash: &Path, command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let mut shell = Command::new(bash);
    shell
        .raw_arg(bash_arguments(command))
        .creation_flags(CREATE_NO_WINDOW);
    shell
}

/// Other systems pass arguments as they are; only Windows has a command line to write.
#[cfg(not(windows))]
fn shell(bash: &Path, command: &str) -> Command {
    let mut shell = Command::new(bash);
    shell.arg("-c").arg(command);
    shell
}

/// What the wrapper exits with for the previous command's status: its own code, as the Mac
/// wrapper passes it. A status with no code, or a negative one (Windows reports a crash as an
/// NTSTATUS, which is negative as an `i32`), is a plain failure.
fn exit_code(status: Option<i32>) -> i32 {
    match status {
        Some(code) if code >= 0 => code,
        _ => 1,
    }
}

/// How long the previous command may run. Debug builds (never the shipped exe) take another
/// value from the environment, so the tests need not wait out the real one.
fn previous_timeout() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(millis) = io::env_text("AGENTNOTCH_HOOK_TEST_PREVIOUS_TIMEOUT_MS")
        .and_then(|text| text.parse::<u64>().ok())
    {
        return Duration::from_millis(millis);
    }
    Duration::from_millis(PREVIOUS_STATUS_LINE_TIMEOUT_MS)
}

// ---- telling the app ----

/// The status message on its way to the app.
struct Sending {
    done: mpsc::Receiver<()>,
    deadline: Instant,
}

impl Sending {
    /// Builds the message and starts sending it; `None` when stdin is not a status (nothing is
    /// forwarded then) or the thread could not be started.
    fn start(status: &[u8]) -> Option<Sending> {
        let Ok(status) = serde_json::from_slice::<Value>(status) else {
            trace::note(|| "stdin: not JSON".into());
            return None;
        };
        // No guess at Claude Code's pid when `CLAUDE_PID` is missing: the wrapper runs under a
        // shell, and the rate limits belong to one process.
        let env = HookEnv::from_env(io::env_text, std::process::id());
        let Some(message) = build_statusline_message(&status, &env) else {
            trace::note(|| "stdin: not a JSON object".into());
            return None;
        };
        let frame = serde_json::to_vec(&message).ok()?;
        if frame.len() > MAX_CLIENT_MESSAGE {
            trace::note(|| "send skipped: too large".into());
            return None;
        }
        // Whether Claude Code's variables reached the wrapper; never what they hold.
        trace::note(|| {
            format!(
                "env claude_pid={} config_dir={}",
                hook::set_or_unset(env.claude_pid.is_some()),
                hook::set_or_unset(env.claude_config_dir.is_some())
            )
        });
        let pid = env.claude_pid_value();
        let deadline = Instant::now() + SEND_BUDGET;
        let (tell, done) = mpsc::channel();
        thread::Builder::new()
            .name("send".into())
            .spawn(move || {
                send(&frame, pid);
                let _ = tell.send(());
            })
            .ok()?;
        Some(Sending { done, deadline })
    }

    /// Waits for the send, for what is left of its budget and never again: a pipe that accepts
    /// and does not read keeps the thread, not the status line. The process's exit ends it.
    fn wait(self) {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if self.done.recv_timeout(left) == Err(RecvTimeoutError::Timeout) {
            trace::note(|| "send: still running".into());
        }
    }
}

/// Fire and forget: connect (checked, as for every hook event), write one frame, close.
fn send(frame: &[u8], pid: Option<u32>) {
    let mut pipe = match pipe::connect(SEND_BUDGET) {
        Ok(pipe) => pipe,
        Err(why) => {
            trace::note(|| why.as_str().into());
            return;
        }
    };
    if pipe::send(&mut pipe, frame) {
        trace::note(|| {
            let pid = pid.map_or_else(|| "none".to_owned(), |pid| pid.to_string());
            format!("sent StatusLine {} bytes pid={pid}", frame.len())
        });
    } else {
        trace::note(|| "send failed: StatusLine".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn git_bash_is_looked_for_where_claude_code_looks() {
        // Claude Code's own setting first, then the 64-bit folder, then the 32-bit one.
        assert_eq!(
            git_bash_candidates(env(&[
                ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
                ("ProgramFiles", r"C:\Program Files"),
                ("CLAUDE_CODE_GIT_BASH_PATH", r"D:\tools\git\bin\bash.exe"),
            ])),
            [
                r"D:\tools\git\bin\bash.exe",
                r"C:\Program Files\Git\bin\bash.exe",
                r"C:\Program Files (x86)\Git\bin\bash.exe",
            ]
        );
        assert_eq!(
            git_bash_candidates(env(&[("ProgramFiles", r"C:\Program Files")])),
            [r"C:\Program Files\Git\bin\bash.exe"]
        );
        // A root written with a separator at its end names the same folder.
        assert_eq!(
            git_bash_candidates(env(&[
                ("ProgramFiles", r"E:\Apps\"),
                ("ProgramFiles(x86)", "E:/")
            ])),
            [r"E:\Apps\Git\bin\bash.exe", r"E:\Git\bin\bash.exe"]
        );
        // Unset and blank variables name nothing; nothing else is ever searched (no `PATH`).
        assert!(git_bash_candidates(env(&[])).is_empty());
        assert!(git_bash_candidates(env(&[
            ("CLAUDE_CODE_GIT_BASH_PATH", ""),
            ("ProgramFiles", "  "),
            ("PATH", r"C:\Program Files\Git\bin"),
            ("ProgramW6432", r"C:\Program Files"),
        ]))
        .is_empty());
    }

    #[test]
    fn the_previous_commands_exit_code_passes_through() {
        assert_eq!(exit_code(Some(0)), 0);
        assert_eq!(exit_code(Some(3)), 3);
        // Not ours to change, even where a hook's 2 would mean "block".
        assert_eq!(exit_code(Some(2)), 2);
        assert_eq!(exit_code(Some(127)), 127);
        assert_eq!(exit_code(Some(i32::MAX)), i32::MAX);
        // No code, or a crash (0xC0000005 as Windows reports it): a plain failure.
        assert_eq!(exit_code(None), 1);
        assert_eq!(exit_code(Some(-1)), 1);
        assert_eq!(exit_code(Some(0xC000_0005_u32 as i32)), 1);
        assert_eq!(exit_code(Some(i32::MIN)), 1);
    }

    #[test]
    fn the_command_is_quoted_for_git_bash() {
        assert_eq!(bash_arguments("echo hi"), r#"-c " echo hi""#);
        assert_eq!(
            bash_arguments("cat; echo TAIL; exit 3"),
            r#"-c " cat; echo TAIL; exit 3""#
        );
        // Single quotes, `$`, pipes and the rest mean nothing on a Windows command line.
        assert_eq!(
            bash_arguments("printf '%s|' $HOME | tr -d ' ' && echo `date` > /dev/null"),
            r#"-c " printf '%s|' $HOME | tr -d ' ' && echo `date` > /dev/null""#
        );
        // Double quotes and backslashes are escaped wherever they stand.
        assert_eq!(
            bash_arguments(r#"node "C:/Users/John Smith/sl.js" --sep " | ""#),
            r#"-c " node \"C:/Users/John Smith/sl.js\" --sep \" | \"""#
        );
        assert_eq!(
            bash_arguments(r"printf 'a\\nb\n'"),
            r#"-c " printf 'a\\\\nb\\n'""#
        );
        assert_eq!(
            bash_arguments(r#"echo "a\"b" \"#),
            r#"-c " echo \"a\\\"b\" \\""#
        );
        // A command that begins with a drive letter does not begin the argument with one.
        assert!(bash_arguments(r#"C:/tools/sl.exe "a b""#).starts_with(r#"-c " C:/"#));
        // Nothing else is touched: text outside ASCII, tabs, line ends.
        assert_eq!(
            bash_arguments("echo '\u{e9}\u{4e2d}'\t&&\necho 2"),
            "-c \" echo '\u{e9}\u{4e2d}'\t&&\necho 2\""
        );
        assert_eq!(bash_arguments(""), r#"-c " ""#);
    }

    /// Undoing the quoting by the rule it was written for gives the command back.
    #[test]
    fn the_quoting_reads_back_as_the_command() {
        fn read_back(arguments: &str) -> Option<String> {
            let quoted = arguments.strip_prefix("-c \" ")?.strip_suffix('"')?;
            let mut command = String::new();
            let mut characters = quoted.chars();
            while let Some(character) = characters.next() {
                match character {
                    '\\' => command.push(characters.next()?),
                    // A bare quote would end the argument early.
                    '"' => return None,
                    other => command.push(other),
                }
            }
            Some(command)
        }
        for command in [
            "echo hi",
            r#"echo "a  b""#,
            r"echo a\\b \",
            r#"\"\\"\"#,
            r#"C:\tools\sl.exe --sep "\\""#,
            "",
        ] {
            assert_eq!(
                read_back(&bash_arguments(command)).as_deref(),
                Some(command),
                "{command}"
            );
        }
    }
}
