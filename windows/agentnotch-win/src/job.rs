//! Child processes on Windows (DESIGN-WIN §3.2 `CommandRunner`; WP4): every child in a Job object
//! with KILL_ON_JOB_CLOSE, CREATE_NO_WINDOW, a cleared and then complete environment, piped stdio;
//! `kill_tree` = `TerminateJobObject`. Used by the usage probe, `claude --version` and summaries.
//!
//! Not implemented in this build: nothing is ever started, so no probe or summary runs and the
//! rings show the engine's honest "unavailable" status.

use std::io;

use agentnotch_engine::platform::{CommandRunner, CommandSpec, RunningCommand};

use crate::NOT_IMPLEMENTED;

#[derive(Debug, Default)]
pub struct JobRunner;

impl JobRunner {
    pub fn new() -> Self {
        JobRunner
    }
}

impl CommandRunner for JobRunner {
    fn spawn(&self, _spec: CommandSpec) -> io::Result<Box<dyn RunningCommand>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, NOT_IMPLEMENTED))
    }
}
