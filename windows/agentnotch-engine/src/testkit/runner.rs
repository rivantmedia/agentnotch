//! A command runner that plays scripts instead of running anything: tests
//! never run `claude`. Each spawn takes the next planned child and records
//! the spec it was given (argv, the complete environment, cwd).
//!
//! Three kinds of child:
//! - [`Script`]: its whole stdout and stderr at once, then its exit
//!   ([`ScriptedRunner::push`]);
//! - [`Conversation`]: answers stdin line by line, so an answer only exists
//!   after the request it answers was written; it ends when its stdin is
//!   closed, as `claude -p` does, or when its tree is killed
//!   ([`ScriptedRunner::push_conversation`]). [`Conversation::hanging`] never
//!   ends by itself: its stdout blocks and `wait_timeout` really waits until
//!   `kill_tree`;
//! - a spawn that fails ([`ScriptedRunner::push_spawn_error`]).
//!
//! Every child's stdin, whether it was closed, and how often its tree was
//! killed are recorded per spawn.
//!
//! Owner after WP0: WP4 (stdout lines per stdin line for the probe).

use super::lock;
use crate::platform::{CommandRunner, CommandSpec, Exit, RunningCommand};
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// What one spawned command does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: Exit,
}

impl Script {
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Script {
        Script {
            stdout: stdout.into(),
            stderr: Vec::new(),
            exit: Exit::Code(0),
        }
    }
}

/// Which stdin line a [`Conversation`] reply answers.
enum Matcher {
    /// A JSON line whose top-level `request_id` is this.
    RequestId(String),
    Line(Predicate),
}

impl Matcher {
    fn matches(&self, line: &[u8]) -> bool {
        match self {
            Matcher::RequestId(id) => serde_json::from_slice::<serde_json::Value>(line)
                .ok()
                .and_then(|value| value.get("request_id")?.as_str().map(str::to_owned))
                .is_some_and(|found| &found == id),
            Matcher::Line(predicate) => predicate(line),
        }
    }
}

type Predicate = Box<dyn Fn(&[u8]) -> bool + Send>;
type Answer = Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>> + Send>;

/// One reply: the lines printed when a stdin line matches. Each reply
/// answers once, the first unused one that matches.
struct Reply {
    matcher: Matcher,
    answer: Answer,
    used: bool,
}

/// A child that talks: stdout lines in answer to stdin lines.
pub struct Conversation {
    greeting: Vec<Vec<u8>>,
    replies: Vec<Reply>,
    stderr: Vec<u8>,
    exit: Exit,
    hangs: bool,
}

impl Default for Conversation {
    fn default() -> Self {
        Conversation::new()
    }
}

impl Conversation {
    /// Prints nothing until asked; exits 0 when its stdin is closed.
    pub fn new() -> Conversation {
        Conversation {
            greeting: Vec::new(),
            replies: Vec::new(),
            stderr: Vec::new(),
            exit: Exit::Code(0),
            hangs: false,
        }
    }

    /// Never answers and never exits by itself: its stdout blocks and
    /// `wait_timeout` waits the whole time asked, until `kill_tree`.
    pub fn hanging() -> Conversation {
        Conversation {
            hangs: true,
            ..Conversation::new()
        }
    }

    /// A line printed at once, before any input (`\n` is appended).
    pub fn print(mut self, line: impl Into<Vec<u8>>) -> Conversation {
        self.greeting.push(line.into());
        self
    }

    /// Answers the JSON request whose `request_id` is `id` with `lines`
    /// (each gets a `\n`).
    pub fn on_request(self, id: &str, lines: Vec<Vec<u8>>) -> Conversation {
        self.on_request_with(id, move |_| lines.clone())
    }

    /// [`Conversation::on_request`] with the answer made when the request
    /// arrives (a test can change files there, as a real child would).
    pub fn on_request_with(
        mut self,
        id: &str,
        answer: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static,
    ) -> Conversation {
        self.replies.push(Reply {
            matcher: Matcher::RequestId(id.to_owned()),
            answer: Box::new(answer),
            used: false,
        });
        self
    }

    /// Answers the first line `predicate` accepts with `lines`.
    pub fn on_line(
        mut self,
        predicate: impl Fn(&[u8]) -> bool + Send + 'static,
        lines: Vec<Vec<u8>>,
    ) -> Conversation {
        self.replies.push(Reply {
            matcher: Matcher::Line(Box::new(predicate)),
            answer: Box::new(move |_| lines.clone()),
            used: false,
        });
        self
    }

    /// What it writes to stderr (readable at once).
    pub fn stderr(mut self, bytes: impl Into<Vec<u8>>) -> Conversation {
        self.stderr = bytes.into();
        self
    }

    /// Its exit once its stdin is closed (default `Exit::Code(0)`).
    pub fn exit(mut self, exit: Exit) -> Conversation {
        self.exit = exit;
        self
    }
}

/// What the next spawn does.
enum Planned {
    Script(Script),
    Conversation(Conversation),
    SpawnError(io::ErrorKind, String),
}

/// What the runner saw of one spawned child.
#[derive(Default)]
struct ChildLog {
    stdin: Mutex<Vec<u8>>,
    stdin_closed: AtomicBool,
    kills: AtomicUsize,
}

#[derive(Default)]
pub struct ScriptedRunner {
    planned: Mutex<VecDeque<Planned>>,
    spawned: Mutex<Vec<CommandSpec>>,
    children: Mutex<Vec<Arc<ChildLog>>>,
}

impl ScriptedRunner {
    pub fn push(&self, script: Script) {
        lock(&self.planned).push_back(Planned::Script(script));
    }

    /// The next spawn talks line by line (or hangs).
    pub fn push_conversation(&self, conversation: Conversation) {
        lock(&self.planned).push_back(Planned::Conversation(conversation));
    }

    /// The next spawn fails with this error (a missing program, a batch
    /// shim whose arguments std refuses).
    pub fn push_spawn_error(&self, kind: io::ErrorKind, message: &str) {
        lock(&self.planned).push_back(Planned::SpawnError(kind, message.to_owned()));
    }

    /// Every spec spawned, in order.
    pub fn spawned(&self) -> Vec<CommandSpec> {
        lock(&self.spawned).clone()
    }

    /// What each spawned command was sent on stdin (children that started,
    /// in order).
    pub fn stdin_of(&self, index: usize) -> Option<Vec<u8>> {
        self.child(index).map(|child| lock(&child.stdin).clone())
    }

    /// Whether a started child's stdin was closed.
    pub fn stdin_closed(&self, index: usize) -> Option<bool> {
        self.child(index)
            .map(|child| child.stdin_closed.load(Ordering::SeqCst))
    }

    /// How often a started child's tree was killed.
    pub fn kills_of(&self, index: usize) -> Option<usize> {
        self.child(index)
            .map(|child| child.kills.load(Ordering::SeqCst))
    }

    fn child(&self, index: usize) -> Option<Arc<ChildLog>> {
        lock(&self.children).get(index).cloned()
    }
}

impl CommandRunner for ScriptedRunner {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        lock(&self.spawned).push(spec);
        let planned = lock(&self.planned)
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no script for this command"))?;
        let pid = 40_000 + lock(&self.spawned).len() as u32;
        let log = match planned {
            Planned::SpawnError(kind, message) => return Err(io::Error::new(kind, message)),
            _ => Arc::new(ChildLog::default()),
        };
        lock(&self.children).push(log.clone());
        Ok(match planned {
            Planned::Script(script) => Box::new(ScriptedCommand {
                pid,
                log,
                stdin: true,
                stdout: Some(script.stdout),
                stderr: Some(script.stderr),
                exit: script.exit,
                killed: false,
            }),
            Planned::Conversation(conversation) => {
                Box::new(TalkingCommand::start(pid, log, conversation))
            }
            Planned::SpawnError(..) => unreachable!("returned above"),
        })
    }
}

struct ScriptedCommand {
    pid: u32,
    log: Arc<ChildLog>,
    stdin: bool,
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    exit: Exit,
    killed: bool,
}

/// A script's stdin: kept, and marked closed when dropped.
struct SharedWriter(Arc<ChildLog>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock(&self.0.stdin).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for SharedWriter {
    fn drop(&mut self) {
        self.0.stdin_closed.store(true, Ordering::SeqCst);
    }
}

impl RunningCommand for ScriptedCommand {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        std::mem::take(&mut self.stdin)
            .then(|| Box::new(SharedWriter(self.log.clone())) as Box<dyn Write + Send>)
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stdout
            .take()
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>)
    }

    fn wait_timeout(&mut self, _d: Duration) -> io::Result<Option<Exit>> {
        Ok(Some(if self.killed { Exit::Killed } else { self.exit }))
    }

    fn kill_tree(&mut self) {
        self.killed = true;
        self.log.kills.fetch_add(1, Ordering::SeqCst);
    }
}

/// A conversation's side of its pipes, shared by the stdin writer, the
/// stdout reader and the command.
struct Pipes {
    state: Mutex<PipeState>,
    changed: Condvar,
}

struct PipeState {
    /// Printed and not read yet.
    stdout: VecDeque<u8>,
    /// Written to stdin since the last newline.
    partial_line: Vec<u8>,
    replies: Vec<Reply>,
    /// Exited by itself (its stdin was closed).
    ended: bool,
    killed: bool,
}

impl Pipes {
    fn lock(&self) -> MutexGuard<'_, PipeState> {
        lock(&self.state)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, PipeState>) -> MutexGuard<'a, PipeState> {
        self.changed
            .wait(guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct TalkingCommand {
    pid: u32,
    log: Arc<ChildLog>,
    pipes: Arc<Pipes>,
    stdin: bool,
    stdout: bool,
    stderr: Option<Vec<u8>>,
    exit: Exit,
    hangs: bool,
}

impl TalkingCommand {
    fn start(pid: u32, log: Arc<ChildLog>, conversation: Conversation) -> TalkingCommand {
        let mut stdout = VecDeque::new();
        for line in &conversation.greeting {
            stdout.extend(line.iter().copied().chain(*b"\n"));
        }
        TalkingCommand {
            pid,
            log,
            pipes: Arc::new(Pipes {
                state: Mutex::new(PipeState {
                    stdout,
                    partial_line: Vec::new(),
                    replies: conversation.replies,
                    ended: false,
                    killed: false,
                }),
                changed: Condvar::new(),
            }),
            stdin: true,
            stdout: true,
            stderr: Some(conversation.stderr),
            exit: conversation.exit,
            hangs: conversation.hangs,
        }
    }
}

/// A conversation's stdin: each complete line may be answered.
struct TalkingStdin {
    log: Arc<ChildLog>,
    pipes: Arc<Pipes>,
    hangs: bool,
}

impl Write for TalkingStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.pipes.lock();
        if state.ended || state.killed {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        lock(&self.log.stdin).extend_from_slice(buf);
        state.partial_line.extend_from_slice(buf);
        while let Some(newline) = state.partial_line.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = state.partial_line.drain(..=newline).collect();
            line.pop();
            let reply = state
                .replies
                .iter_mut()
                .find(|reply| !reply.used && reply.matcher.matches(&line));
            if let Some(reply) = reply {
                reply.used = true;
                let printed = (reply.answer)(&line);
                for out in printed {
                    state.stdout.extend(out.into_iter().chain(*b"\n"));
                }
            }
        }
        self.pipes.changed.notify_all();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for TalkingStdin {
    fn drop(&mut self) {
        self.log.stdin_closed.store(true, Ordering::SeqCst);
        // `claude -p` exits at the end of its input; a hanging child doesn't.
        if !self.hangs {
            self.pipes.lock().ended = true;
            self.pipes.changed.notify_all();
        }
    }
}

/// A conversation's stdout: blocks until something is printed, the child
/// ends, or its tree is killed.
struct TalkingStdout(Arc<Pipes>);

impl Read for TalkingStdout {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut state = self.0.lock();
        while state.stdout.is_empty() && !state.ended && !state.killed {
            state = self.0.wait(state);
        }
        let count = buf.len().min(state.stdout.len());
        for (slot, byte) in buf.iter_mut().zip(state.stdout.drain(..count)) {
            *slot = byte;
        }
        Ok(count)
    }
}

impl RunningCommand for TalkingCommand {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        std::mem::take(&mut self.stdin).then(|| {
            Box::new(TalkingStdin {
                log: self.log.clone(),
                pipes: self.pipes.clone(),
                hangs: self.hangs,
            }) as Box<dyn Write + Send>
        })
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        std::mem::take(&mut self.stdout)
            .then(|| Box::new(TalkingStdout(self.pipes.clone())) as Box<dyn Read + Send>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>)
    }

    fn wait_timeout(&mut self, d: Duration) -> io::Result<Option<Exit>> {
        let state = self.pipes.lock();
        let (state, _) = self
            .pipes
            .changed
            .wait_timeout_while(state, d, |state| !state.ended && !state.killed)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(if state.killed {
            Some(Exit::Killed)
        } else if state.ended {
            Some(self.exit)
        } else {
            None
        })
    }

    fn kill_tree(&mut self) {
        self.log.kills.fetch_add(1, Ordering::SeqCst);
        self.pipes.lock().killed = true;
        self.pipes.changed.notify_all();
    }
}
