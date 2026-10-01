//! An in-memory hook transport: tests inject frames and read the answers.
//!
//! It behaves like the pipe server as far as the engine can tell: every
//! injected frame is a new connection, a connection is answered or closed
//! once (an answer to a closed or vanished connection fails, like a write to
//! a pipe whose hook is gone), and `peer_closed` is the hook going away.
//!
//! Owner: WP1.

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
        let conn = self.next_conn();
        self.send(TransportEvent::Frame(IncomingFrame {
            conn,
            bytes: bytes.into(),
            received_at,
            peer_pid: None,
        }));
        conn
    }

    /// A frame event for a new connection, without sending it anywhere (for
    /// tests that drive `HookIngress::on_transport` themselves), and its id.
    pub fn frame(
        &self,
        json: &serde_json::Value,
        received_at: SystemTime,
    ) -> (ConnId, TransportEvent) {
        let conn = self.next_conn();
        let event = TransportEvent::Frame(IncomingFrame {
            conn,
            bytes: serde_json::to_vec(json).unwrap_or_default(),
            received_at,
            peer_pid: None,
        });
        (conn, event)
    }

    /// The hook of `conn` went away.
    pub fn peer_closed(&self, conn: ConnId) {
        self.mark_gone(conn);
        self.send(TransportEvent::PeerClosed(conn));
    }

    /// The hook of `conn` went away, without an event yet (it vanished
    /// between the engine's last look and its answer).
    pub fn mark_gone(&self, conn: ConnId) {
        lock(&self.gone).insert(conn);
    }

    pub fn send(&self, event: TransportEvent) {
        if let Some(sink) = lock(&self.sink).as_ref() {
            let _ = sink.send(event);
        }
    }

    /// What was written to connections, in order.
    pub fn responses(&self) -> Vec<(ConnId, Vec<u8>)> {
        lock(&self.responses).clone()
    }

    /// What was written to `conn`, if anything.
    pub fn response(&self, conn: ConnId) -> Option<Vec<u8>> {
        lock(&self.responses)
            .iter()
            .find(|(c, _)| *c == conn)
            .map(|(_, bytes)| bytes.clone())
    }

    /// Connections closed without an answer, in order.
    pub fn closed(&self) -> Vec<ConnId> {
        lock(&self.closed).clone()
    }

    pub fn is_closed(&self, conn: ConnId) -> bool {
        lock(&self.closed).contains(&conn)
    }

    pub fn pipe_name(&self) -> Option<String> {
        lock(&self.pipe_name).clone()
    }

    pub fn is_stopped(&self) -> bool {
        *lock(&self.stopped)
    }

    fn next_conn(&self) -> ConnId {
        let mut next = lock(&self.next_conn);
        *next += 1;
        *next
    }

    /// Answered or closed already: the real server has dropped it.
    fn is_done(&self, conn: ConnId) -> bool {
        lock(&self.closed).contains(&conn) || lock(&self.responses).iter().any(|(c, _)| *c == conn)
    }
}

impl HookTransport for MemoryTransport {
    fn start(
        &self,
        pipe_name: &str,
        sink: crossbeam_channel::Sender<TransportEvent>,
    ) -> Result<(), String> {
        *lock(&self.pipe_name) = Some(pipe_name.to_owned());
        *lock(&self.stopped) = false;
        let _ = sink.send(TransportEvent::Listening(pipe_name.to_owned()));
        *lock(&self.sink) = Some(sink);
        Ok(())
    }

    fn respond(&self, conn: ConnId, frame_json: Vec<u8>) -> bool {
        if self.is_done(conn) || lock(&self.gone).contains(&conn) {
            return false;
        }
        lock(&self.responses).push((conn, frame_json));
        true
    }

    fn close(&self, conn: ConnId) {
        if !self.is_done(conn) {
            lock(&self.closed).push(conn);
        }
    }

    fn stop(&self) {
        *lock(&self.stopped) = true;
        *lock(&self.sink) = None;
    }
}
