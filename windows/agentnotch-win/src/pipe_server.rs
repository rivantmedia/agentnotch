//! The hook pipe's server (DESIGN-WIN §1.4, §3.2 `HookTransport`; WP1):
//! `\\.\pipe\agentnotch-hook-<SID>` on its own Tokio current-thread runtime (`an-pipe`), owner and
//! protected DACL from SDDL `O:<SID>D:P(A;;GA;;;<SID>)(A;;GA;;;SY)`, first-instance flag, remote
//! clients rejected, the peer's user checked by identification-level impersonation after the
//! first frame, held PermissionRequest connections answered or closed.
//!
//! Not implemented in this build: `start` reports it, which the hub shows as "Not receiving hook
//! events" while sessions still come from Claude Code's session files.

#![cfg(windows)]

use agentnotch_engine::platform::{ConnId, HookTransport, TransportEvent};

use crate::NOT_IMPLEMENTED;

#[derive(Debug, Default)]
pub struct PipeServer;

impl PipeServer {
    pub fn new() -> Self {
        PipeServer
    }
}

impl HookTransport for PipeServer {
    fn start(
        &self,
        _pipe_name: &str,
        _sink: crossbeam_channel::Sender<TransportEvent>,
    ) -> Result<(), String> {
        Err(format!("The hook pipe is {NOT_IMPLEMENTED}"))
    }
    fn respond(&self, _conn: ConnId, _frame_json: Vec<u8>) -> bool {
        false
    }
    fn close(&self, _conn: ConnId) {}
    fn stop(&self) {}
}
