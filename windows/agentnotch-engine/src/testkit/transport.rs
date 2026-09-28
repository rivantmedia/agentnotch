//! An in-memory hook transport: tests inject frames and read the answers.
//!
//! Owner after WP0: WP1.

use super::lock;
use crate::platform::{ConnId, HookTransport, IncomingFrame, TransportEvent};
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Default)]
pub struct MemoryTransport {
    sink: Mutex<Option<crossbeam_channel::Sender<TransportEvent>>>,
    pipe_name: Mutex<Option<String>>,
    next_conn: Mutex<ConnId>,
    responses: Mutex<Vec<(ConnId, Vec<u8>)>>,
    closed: Mutex<Vec<ConnId>>,
    gone: Mutex<BTreeSet<ConnId>>,
    stopped: Mutex<bool>,
}

impl MemoryTransport {
    /// Sends a frame as a new connection; its id.
    pub fn inject(&self, bytes: impl Into<Vec<u8>>, received_at: SystemTime) -> ConnId {
        let conn = {
            let mut next = lock(&self.next_conn);
            *next += 1;
            *next
        };
        self.send(TransportEvent::Frame(IncomingFrame {
            conn,
            bytes: bytes.into(),
            received_at,
            peer_pid: None,
        }));
        conn
    }

    /// The hook of `conn` went away.
    pub fn peer_closed(&self, conn: ConnId) {
        lock(&self.gone).insert(conn);
        self.send(TransportEvent::PeerClosed(conn));
    }

    pub fn send(&self, event: TransportEvent) {
        if let Some(sink) = lock(&self.sink).as_ref() {
            let _ = sink.send(event);
        }
    }

    /// What was written to held connections, in order.
    pub fn responses(&self) -> Vec<(ConnId, Vec<u8>)> {
        lock(&self.responses).clone()
    }

    /// Connections closed without an answer.
    pub fn closed(&self) -> Vec<ConnId> {
        lock(&self.closed).clone()
    }

    pub fn pipe_name(&self) -> Option<String> {
        lock(&self.pipe_name).clone()
    }

    pub fn is_stopped(&self) -> bool {
        *lock(&self.stopped)
    }
}

impl HookTransport for MemoryTransport {
    fn start(
        &self,
        pipe_name: &str,
        sink: crossbeam_channel::Sender<TransportEvent>,
    ) -> Result<(), String> {
        *lock(&self.pipe_name) = Some(pipe_name.to_owned());
        let _ = sink.send(TransportEvent::Listening(pipe_name.to_owned()));
        *lock(&self.sink) = Some(sink);
        Ok(())
    }

    fn respond(&self, conn: ConnId, frame_json: Vec<u8>) -> bool {
        if lock(&self.gone).contains(&conn) {
            return false;
        }
        lock(&self.responses).push((conn, frame_json));
        true
    }

    fn close(&self, conn: ConnId) {
        lock(&self.closed).push(conn);
    }

    fn stop(&self) {
        *lock(&self.stopped) = true;
        *lock(&self.sink) = None;
    }
}
