//! A command runner that plays scripts instead of running anything: tests
//! never run `claude`. Each spawn takes the next script and records the
//! spec it was given (argv, the complete environment, cwd).
//!
//! Owner after WP0: WP4 (stdout lines per stdin line for the probe).

use super::lock;
use crate::platform::{CommandRunner, CommandSpec, Exit, RunningCommand};
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};
use std::sync::{Arc, Mutex};
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

#[derive(Default)]
pub struct ScriptedRunner {
    scripts: Mutex<VecDeque<Script>>,
    spawned: Mutex<Vec<CommandSpec>>,
    stdin: Mutex<Vec<Arc<Mutex<Vec<u8>>>>>,
}

impl ScriptedRunner {
    pub fn push(&self, script: Script) {
        lock(&self.scripts).push_back(script);
    }

    /// Every spec spawned, in order.
    pub fn spawned(&self) -> Vec<CommandSpec> {
        lock(&self.spawned).clone()
    }

    /// What each spawned command was sent on stdin.
    pub fn stdin_of(&self, index: usize) -> Option<Vec<u8>> {
        lock(&self.stdin)
            .get(index)
            .map(|buffer| lock(buffer).clone())
    }
}

impl CommandRunner for ScriptedRunner {
    fn spawn(&self, spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        lock(&self.spawned).push(spec);
        let script = lock(&self.scripts)
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no script for this command"))?;
        let stdin = Arc::new(Mutex::new(Vec::new()));
        lock(&self.stdin).push(stdin.clone());
        let pid = 40_000 + lock(&self.spawned).len() as u32;
        Ok(Box::new(ScriptedCommand {
            pid,
            stdin: Some(stdin),
            stdout: Some(script.stdout),
            stderr: Some(script.stderr),
            exit: script.exit,
            killed: false,
        }))
    }
}

struct ScriptedCommand {
    pid: u32,
    stdin: Option<Arc<Mutex<Vec<u8>>>>,
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    exit: Exit,
    killed: bool,
}

struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RunningCommand for ScriptedCommand {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.stdin
            .take()
            .map(|buffer| Box::new(SharedWriter(buffer)) as Box<dyn Write + Send>)
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
    }
}
