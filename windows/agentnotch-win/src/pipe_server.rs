//! The hook pipe's server (DESIGN-WIN §1.4, §3.2 `HookTransport`; WP1).
//!
//! `\\.\pipe\agentnotch-hook-<SID>` is served on its own thread `an-pipe` by a Tokio
//! current-thread runtime:
//!
//! - Every instance is created with the owner and the protected DACL of
//!   `agentnotch_proto::pipe_sddl` (the user and SYSTEM, nobody else), in byte mode, with remote
//!   clients rejected; the first one also with the first-instance flag, so a name somebody else
//!   made first is never joined. That case is reported and tried again every 5 s.
//! - One idle instance always listens. When a client connects, the next instance is created
//!   before the connected one is served.
//! - A connection sends one frame within 5 s. Only then is its user checked, by impersonating it
//!   at identification level; a frame from anyone but this user is never delivered.
//! - A delivered connection stays open, with a read pending that ends when the hook goes away,
//!   until the engine answers it (`respond`: one frame, then close) or closes it (`close`: no
//!   frame, which a hook reads as "no decision").
//!
//! What can be decided without Windows (which connections exist and what the engine may still do
//! with each, what to report and when, whose frame is accepted) is plain code at the top of this
//! file, tested on every system. The Win32 and Tokio part is the `server` module, on Windows only.
//!
//! Beside it: [`client`], the checked client the app's own programs use (`control status|quit`,
//! the doctor), [`script`], what the test-only `pipe-test-server.exe` does with this server, and
//! [`squatter`], the command line and record of the test-only `pipe-squatter.exe`.

pub mod client;
pub mod script;
pub mod squatter;

use std::collections::BTreeMap;

use agentnotch_engine::platform::{ConnId, TransportEvent};
use agentnotch_proto::SYSTEM_SID;

/// What the hub shows when the pipe's name was made by someone else first: a second copy of the
/// app, or a program squatting on the name.
pub const PIPE_IN_USE: &str = "The hook pipe is in use by another program";

/// `ERROR_ACCESS_DENIED`: the name exists and is not ours to add an instance to.
const OS_ACCESS_DENIED: i32 = 5;
/// `ERROR_PIPE_BUSY`: the name exists and its owner allows no further instance.
const OS_PIPE_BUSY: i32 = 231;

/// Why no pipe instance could be created, for the hub's banner. `os_error` is the Windows error
/// code, when there was one.
pub fn create_failure_message(os_error: Option<i32>) -> String {
    match os_error {
        Some(OS_ACCESS_DENIED | OS_PIPE_BUSY) => PIPE_IN_USE.to_owned(),
        Some(code) => format!("The hook pipe can't be opened (Windows error {code})"),
        None => "The hook pipe can't be opened".to_owned(),
    }
}

/// Turns what the server finds on each attempt into the events the engine gets: one when the
/// state changes, none while it stays the same (a taken name is tried again every 5 s, and the
/// banner needs telling once).
#[derive(Debug, Default)]
pub struct StatusLog {
    /// `Ok` = listening, `Err` = the last failure reported.
    last: Option<Result<(), String>>,
}

impl StatusLog {
    /// An instance was created: `Listening` the first time, and again after a failure.
    pub fn listening(&mut self, pipe_name: &str) -> Option<TransportEvent> {
        if self.last == Some(Ok(())) {
            return None;
        }
        self.last = Some(Ok(()));
        Some(TransportEvent::Listening(pipe_name.to_owned()))
    }

    /// No instance could be created: `Error`, unless the same failure was reported last.
    pub fn failed(&mut self, why: String) -> Option<TransportEvent> {
        if matches!(&self.last, Some(Err(last)) if *last == why) {
            return None;
        }
        self.last = Some(Err(why.clone()));
        Some(TransportEvent::Error(why))
    }
}

/// Whether a frame from a client running as `peer_sid` is delivered: only when that user could
/// be read and is the app's own. SYSTEM may open the pipe (the DACL names it so the OS can
/// service it) but is never a hook.
pub fn peer_allowed(own_sid: &str, peer_sid: Option<&str>) -> bool {
    match peer_sid {
        Some(peer) => {
            !peer.is_empty()
                && !peer.eq_ignore_ascii_case(SYSTEM_SID)
                && peer.eq_ignore_ascii_case(own_sid)
        }
        None => false,
    }
}

/// The body length a frame's header declares; `None` when it is above `max`, so nothing of an
/// oversized frame is read.
pub fn declared_length(header: [u8; 4], max: usize) -> Option<usize> {
    let length = usize::try_from(u32::from_le_bytes(header)).ok()?;
    (length <= max).then_some(length)
}

/// How a connection left the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It was not in the table (the server stopped meanwhile).
    Unknown,
    /// Its frame was never delivered: the engine has not heard of it.
    Undelivered,
    /// Its frame was delivered and the engine had not answered or closed it yet: the engine
    /// must be told when it was the peer that went away.
    Held,
    /// The engine had answered or closed it already.
    Finished,
}

#[derive(Debug)]
enum Slot<H> {
    /// Connected; its first frame is not read yet.
    Reading,
    /// Delivered; `H` is the engine's way to answer it.
    Held(H),
    /// Answered or closed by the engine; the handle is being written to or closed.
    Finishing,
}

/// The open connections: how many there are, and what the engine may still do with each.
///
/// A connection is counted from the moment a client connects until its handle is closed, so the
/// cap bounds handles, not just held requests. `H` is whatever answers a held connection (the
/// server's channel into the connection's task); it is handed out once, to the first `take`.
#[derive(Debug)]
pub struct ConnTable<H> {
    cap: usize,
    /// The last id given out. Ids only grow, so an answer meant for a finished connection can
    /// never reach a later one.
    last: ConnId,
    open: BTreeMap<ConnId, Slot<H>>,
}

impl<H> ConnTable<H> {
    pub fn new(cap: usize) -> Self {
        ConnTable {
            cap,
            last: 0,
            open: BTreeMap::new(),
        }
    }

    /// A client connected: its id, or `None` when `cap` connections are open already (the
    /// caller closes it at once; the hook fails open).
    pub fn connect(&mut self) -> Option<ConnId> {
        if self.open.len() >= self.cap {
            return None;
        }
        self.last += 1;
        self.open.insert(self.last, Slot::Reading);
        Some(self.last)
    }

    /// The connection's frame is about to be delivered: from now on the engine may answer it
    /// through `handle`. False when the connection is not waiting for its frame (unknown, or
    /// delivered before): nothing may be delivered then.
    pub fn hold(&mut self, conn: ConnId, handle: H) -> bool {
        match self.open.get_mut(&conn) {
            Some(slot @ Slot::Reading) => {
                *slot = Slot::Held(handle);
                true
            }
            _ => false,
        }
    }

    /// The engine answers or closes `conn`: its handle, the first time. `None` for an id that
    /// was never delivered, is finished, or was taken before.
    pub fn take(&mut self, conn: ConnId) -> Option<H> {
        let slot = self.open.get_mut(&conn)?;
        if !matches!(slot, Slot::Held(_)) {
            return None;
        }
        match std::mem::replace(slot, Slot::Finishing) {
            Slot::Held(handle) => Some(handle),
            Slot::Reading | Slot::Finishing => None,
        }
    }

    /// The connection's handle is closed: it no longer counts.
    pub fn end(&mut self, conn: ConnId) -> Ended {
        match self.open.remove(&conn) {
            None => Ended::Unknown,
            Some(Slot::Reading) => Ended::Undelivered,
            Some(Slot::Held(_)) => Ended::Held,
            Some(Slot::Finishing) => Ended::Finished,
        }
    }

    /// Forgets every connection (the server stops). Their handles are dropped, which is how a
    /// held connection's task learns it must close.
    pub fn clear(&mut self) {
        self.open.clear();
    }

    /// How many connections are open.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

#[cfg(windows)]
pub use server::PipeServer;

#[cfg(windows)]
mod server {
    use std::ffi::c_void;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
    use std::thread::JoinHandle;
    use std::time::{Duration, SystemTime};

    use agentnotch_engine::platform::{ConnId, HookTransport, IncomingFrame, TransportEvent};
    use agentnotch_proto::limits::{
        MAX_CONNECTIONS, MAX_SERVER_MESSAGE, PIPE_BUFFER_BYTES, RESPONSE_WRITE_TIMEOUT_MS,
        SERVER_READ_DEADLINE_MS, SERVER_RETRY_MS,
    };
    use agentnotch_proto::{encode_frame, pipe_sddl};
    use crossbeam_channel::Sender;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, Interest};
    use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
    use tokio::runtime::{Builder, Runtime};
    use tokio::sync::oneshot;
    use tokio::task::JoinSet;
    use tokio::time::{sleep, timeout};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{RevertToSelf, SECURITY_ATTRIBUTES};
    use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, ImpersonateNamedPipeClient};

    use super::{
        create_failure_message, declared_length, peer_allowed, ConnTable, Ended, StatusLog,
    };
    use crate::sid::{self, SecurityDescriptor};

    const READ_DEADLINE: Duration = Duration::from_millis(SERVER_READ_DEADLINE_MS);
    const WRITE_DEADLINE: Duration = Duration::from_millis(RESPONSE_WRITE_TIMEOUT_MS);
    const RETRY_EVERY: Duration = Duration::from_millis(SERVER_RETRY_MS);
    /// After a failed accept, before the next one is awaited: a failure that repeats must not
    /// spin the thread.
    const ACCEPT_PAUSE: Duration = Duration::from_millis(50);
    /// How long the runtime keeps turning after everything was closed at `stop`. A handle is
    /// only released once the cancellation of its pending read has come back through the
    /// runtime; a hook waiting on a held connection sees the close no earlier.
    const STOP_DRAIN: Duration = Duration::from_millis(50);
    /// `respond` waits for the pipe thread this much longer than the write may take, so a busy
    /// thread is not mistaken for a hook that went away. Short: the caller is the engine's core
    /// thread.
    const RESPOND_SLACK: Duration = Duration::from_millis(250);

    /// The engine's answer to a held connection. Dropping the sender instead is "close, no frame".
    struct Answer {
        json: Vec<u8>,
        /// Whether the frame was written.
        done: mpsc::Sender<bool>,
    }

    type Table = Mutex<ConnTable<oneshot::Sender<Answer>>>;

    /// What every task of one run shares.
    struct Context {
        pipe_name: String,
        own_sid: String,
        sink: Sender<TransportEvent>,
        table: Arc<Table>,
    }

    struct Running {
        table: Arc<Table>,
        stop: oneshot::Sender<()>,
        thread: JoinHandle<()>,
    }

    /// The hook pipe's server. Nothing runs until `start`.
    #[derive(Default)]
    pub struct PipeServer {
        running: Mutex<Option<Running>>,
    }

    impl std::fmt::Debug for PipeServer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("PipeServer")
                .field("running", &lock(&self.running).is_some())
                .finish()
        }
    }

    impl PipeServer {
        pub fn new() -> Self {
            PipeServer::default()
        }

        /// The running server's connection table, without keeping `running` locked while the
        /// caller waits on the pipe thread.
        fn table(&self) -> Option<Arc<Table>> {
            lock(&self.running)
                .as_ref()
                .map(|running| running.table.clone())
        }
    }

    impl HookTransport for PipeServer {
        fn start(&self, pipe_name: &str, sink: Sender<TransportEvent>) -> Result<(), String> {
            self.stop();
            let own_sid = sid::current_user_sid()
                .ok_or("The hook pipe can't be opened: this user's SID is unreadable")?;
            let descriptor =
                SecurityDescriptor::from_sddl(&pipe_sddl(&own_sid)).map_err(|why| {
                    format!("The hook pipe's security descriptor can't be built: {why}")
                })?;
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("The hook pipe's thread can't start: {error}"))?;
            let table = Arc::new(Mutex::new(ConnTable::new(MAX_CONNECTIONS)));
            let context = Arc::new(Context {
                pipe_name: pipe_name.to_owned(),
                own_sid,
                sink,
                table: table.clone(),
            });
            let (stop, stopped) = oneshot::channel();
            let thread = std::thread::Builder::new()
                .name("an-pipe".into())
                .spawn(move || run(runtime, context, descriptor, stopped))
                .map_err(|error| format!("The hook pipe's thread can't start: {error}"))?;
            *lock(&self.running) = Some(Running {
                table,
                stop,
                thread,
            });
            Ok(())
        }

        fn respond(&self, conn: ConnId, frame_json: Vec<u8>) -> bool {
            let Some(table) = self.table() else {
                return false;
            };
            let Some(answer) = lock(&table).take(conn) else {
                return false;
            };
            let (done, written) = mpsc::channel();
            let sent = answer.send(Answer {
                json: frame_json,
                done,
            });
            if sent.is_err() {
                // The connection's task ended first: the hook went away.
                return false;
            }
            // An answer that is dropped unwritten (the hook vanished, the server stopped) drops
            // `done`, which ends this wait with an error: false as well.
            written
                .recv_timeout(WRITE_DEADLINE + RESPOND_SLACK)
                .unwrap_or(false)
        }

        fn close(&self, conn: ConnId) {
            if let Some(table) = self.table() {
                // Dropping the answer's sender is what the connection's task reads as "close".
                drop(lock(&table).take(conn));
            }
        }

        fn stop(&self) {
            let Some(running) = lock(&self.running).take() else {
                return;
            };
            let _ = running.stop.send(());
            // Returning only once the thread is gone means every handle is closed by then: the
            // waiting hooks have their "no decision", and the name is free for a later `start`.
            let _ = running.thread.join();
        }
    }

    impl Drop for PipeServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        // A poisoned table is still a table; losing the pipe over it would lose every hook.
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(
        runtime: Runtime,
        context: Arc<Context>,
        descriptor: SecurityDescriptor,
        stopped: oneshot::Receiver<()>,
    ) {
        runtime.block_on(serve(context, descriptor, stopped));
    }

    /// The accept loop. It owns the idle instance and every connection's task, and ends when
    /// `stopped` fires (or its sender is dropped).
    async fn serve(
        context: Arc<Context>,
        descriptor: SecurityDescriptor,
        mut stopped: oneshot::Receiver<()>,
    ) {
        let mut tasks = JoinSet::new();
        let mut status = StatusLog::default();
        // Only the very first instance claims the name: later ones join our own pipe.
        let mut first = true;
        let mut idle: Option<NamedPipeServer> = None;
        loop {
            let pipe = match idle.take() {
                Some(pipe) => pipe,
                None => match create_instance(&context.pipe_name, first, &descriptor) {
                    Ok(pipe) => {
                        first = false;
                        report(&context, status.listening(&context.pipe_name));
                        pipe
                    }
                    Err(error) => {
                        let why = create_failure_message(error.raw_os_error());
                        report(&context, status.failed(why));
                        tokio::select! {
                            _ = &mut stopped => break,
                            () = sleep(RETRY_EVERY) => continue,
                        }
                    }
                },
            };
            let connected = tokio::select! {
                _ = &mut stopped => break,
                connected = pipe.connect() => connected,
            };
            if connected.is_err() {
                drop(pipe);
                // The pause is spent with a fresh instance in place, so a hook that arrives
                // meanwhile still finds the pipe.
                idle = create_instance(&context.pipe_name, false, &descriptor).ok();
                tokio::select! {
                    _ = &mut stopped => break,
                    () = sleep(ACCEPT_PAUSE) => continue,
                }
            }
            // The next hook must find an instance to connect to, however long this one is held:
            // replace the instance before serving it. When that fails the connection is served
            // all the same, and the top of the loop reports and retries.
            match create_instance(&context.pipe_name, false, &descriptor) {
                Ok(next) => {
                    report(&context, status.listening(&context.pipe_name));
                    idle = Some(next);
                }
                Err(error) => {
                    let why = create_failure_message(error.raw_os_error());
                    report(&context, status.failed(why));
                }
            }
            // Finished tasks stay in the set until they are collected.
            while tasks.try_join_next().is_some() {}
            let Some(conn) = lock(&context.table).connect() else {
                // Too many open connections: closed at once, the hook fails open.
                drop(pipe);
                continue;
            };
            tasks.spawn(connection(context.clone(), conn, pipe));
        }
        drop(idle);
        lock(&context.table).clear();
        tasks.shutdown().await;
        sleep(STOP_DRAIN).await;
    }

    fn report(context: &Context, event: Option<TransportEvent>) {
        if let Some(event) = event {
            let _ = context.sink.send(event);
        }
    }

    fn create_instance(
        pipe_name: &str,
        first: bool,
        descriptor: &SecurityDescriptor,
    ) -> io::Result<NamedPipeServer> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: false.into(),
        };
        let mut options = ServerOptions::new();
        // `max_instances` stays unset (unlimited): the table counts connections instead.
        options
            .pipe_mode(PipeMode::Byte)
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .in_buffer_size(PIPE_BUFFER_BYTES)
            .out_buffer_size(PIPE_BUFFER_BYTES);
        let attributes: *mut SECURITY_ATTRIBUTES = &mut attributes;
        // SAFETY: `attributes` points at a SECURITY_ATTRIBUTES that lives until the call
        // returns, and its descriptor is alive as long as `descriptor` is. Windows copies both.
        unsafe {
            options.create_with_security_attributes_raw(pipe_name, attributes.cast::<c_void>())
        }
    }

    /// Why a connection's task ended.
    enum Outcome {
        /// Closed by this side: no frame in time, a refused peer, an answer, a close.
        Closed,
        /// The hook closed its end, or its process ended.
        PeerGone,
    }

    /// One connection, from the connect to the close of its handle.
    async fn connection(context: Arc<Context>, conn: ConnId, mut pipe: NamedPipeServer) {
        let outcome = converse(&context, conn, &mut pipe).await;
        let ended = lock(&context.table).end(conn);
        // Only a connection the engine still holds is news to it; one it answered or closed
        // already, or never heard of, is not.
        if matches!(outcome, Outcome::PeerGone) && ended == Ended::Held {
            let _ = context.sink.send(TransportEvent::PeerClosed(conn));
        }
        // Dropping the pipe closes the handle, with whatever was written still readable by the
        // client. It is never disconnected first: that would discard what the client has not
        // read yet.
        drop(pipe);
    }

    async fn converse(context: &Context, conn: ConnId, pipe: &mut NamedPipeServer) -> Outcome {
        let Ok(Some(bytes)) = timeout(READ_DEADLINE, read_first_frame(pipe)).await else {
            return Outcome::Closed;
        };
        let received_at = SystemTime::now();
        // The user can only be asked for now: impersonation needs data read from the pipe first.
        if !peer_allowed(&context.own_sid, peer_sid(pipe).as_deref()) {
            return Outcome::Closed;
        }
        let peer_pid = client_pid(pipe);
        let (answer, mut answered) = oneshot::channel();
        // Held before it is delivered: the engine may answer as soon as it has the frame.
        if !lock(&context.table).hold(conn, answer) {
            return Outcome::Closed;
        }
        let frame = TransportEvent::Frame(IncomingFrame {
            conn,
            bytes,
            received_at,
            peer_pid,
        });
        if context.sink.send(frame).is_err() {
            return Outcome::Closed;
        }
        let answer = tokio::select! {
            answer = &mut answered => answer.ok(),
            () = peer_gone(pipe) => return Outcome::PeerGone,
        };
        if let Some(Answer { json, done }) = answer {
            let written = write_response(pipe, &json).await;
            let _ = done.send(written);
        }
        Outcome::Closed
    }

    /// The connection's one frame; `None` when the peer closed first, the frame is cut, or it
    /// declares more than the server accepts.
    async fn read_first_frame(pipe: &mut NamedPipeServer) -> Option<Vec<u8>> {
        let mut header = [0u8; 4];
        pipe.read_exact(&mut header).await.ok()?;
        let length = declared_length(header, MAX_SERVER_MESSAGE)?;
        // Grown as the bytes arrive: a header alone never makes the server allocate 8 MiB.
        let mut body = Vec::new();
        let mut limited = (&mut *pipe).take(length as u64);
        limited.read_to_end(&mut body).await.ok()?;
        (body.len() == length).then_some(body)
    }

    /// Ends when the peer closed its end or went away. A delivered connection has nothing more
    /// to say, so anything it still sends is read and dropped.
    async fn peer_gone(pipe: &mut NamedPipeServer) {
        let mut scratch = [0u8; 512];
        loop {
            match pipe.read(&mut scratch).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }

    /// Writes one response frame; true when Windows took all of it within the deadline.
    async fn write_response(pipe: &mut NamedPipeServer, json: &[u8]) -> bool {
        let Ok(frame) = encode_frame(json) else {
            return false;
        };
        let written = timeout(WRITE_DEADLINE, async {
            pipe.write_all(&frame).await?;
            // The runtime's pipe takes the whole buffer and writes it in the background, so
            // `write_all` returning says nothing yet. Forget that the pipe was writable: it
            // becomes writable again exactly when that background write has finished.
            let _ = pipe.try_io(Interest::WRITABLE, || {
                Err::<(), io::Error>(io::ErrorKind::WouldBlock.into())
            });
            pipe.writable().await
        })
        .await;
        match written {
            Ok(Ok(())) => true,
            Ok(Err(_)) => false,
            Err(_) => {
                // The hook has not read for the whole deadline: it is not going to. Only now is
                // the pipe disconnected, which fails the write still pending on it; without
                // that the handle would stay open for as long as the hook's process lives.
                let _ = pipe.disconnect();
                false
            }
        }
    }

    /// The string SID of the user at the other end, read by impersonating it for the length of
    /// this call; `None` when it can't be impersonated or its token can't be read.
    ///
    /// Synchronous on purpose: the thread must never reach an await while it impersonates, or
    /// another connection's code would run as this client.
    fn peer_sid(pipe: &NamedPipeServer) -> Option<String> {
        // SAFETY: the handle is the pipe's own and stays open for the whole call.
        unsafe { ImpersonateNamedPipeClient(HANDLE(pipe.as_raw_handle())) }.ok()?;
        let sid = sid::thread_token_sid();
        // SAFETY: no arguments; it ends the impersonation begun above.
        if unsafe { RevertToSelf() }.is_err() {
            // The thread would go on serving every hook as this client.
            std::process::abort();
        }
        sid
    }

    /// The client's process id, for the engine's diagnostics only: nothing is decided by it.
    fn client_pid(pipe: &NamedPipeServer) -> Option<u32> {
        let mut pid = 0u32;
        // SAFETY: the handle is the pipe's own and open; `pid` is a valid out pointer.
        unsafe { GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()), &mut pid) }.ok()?;
        (pid != 0).then_some(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentnotch_proto::limits::{MAX_CONNECTIONS, MAX_SERVER_MESSAGE};

    const ME: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";

    /// A table with `n` connections, all delivered, and their ids.
    fn held(n: usize) -> (ConnTable<&'static str>, Vec<ConnId>) {
        let mut table = ConnTable::new(MAX_CONNECTIONS);
        let ids: Vec<ConnId> = (0..n).filter_map(|_| table.connect()).collect();
        for id in &ids {
            assert!(table.hold(*id, "answer"));
        }
        (table, ids)
    }

    #[test]
    fn the_table_stops_at_512_connections() {
        let (mut table, ids) = held(MAX_CONNECTIONS);
        assert_eq!(ids.len(), 512);
        assert_eq!(table.len(), 512);
        assert_eq!(table.connect(), None);
        assert_eq!(table.connect(), None);
        assert_eq!(table.len(), 512);

        // One closes: there is room for exactly one more.
        assert_eq!(table.end(ids[0]), Ended::Held);
        assert!(table.connect().is_some());
        assert_eq!(table.connect(), None);
    }

    #[test]
    fn a_connection_counts_until_its_handle_is_closed() {
        let mut table: ConnTable<&str> = ConnTable::new(2);
        let reading = table.connect().unwrap();
        let answered = table.connect().unwrap();
        assert!(table.hold(answered, "answer"));
        assert_eq!(table.take(answered), Some("answer"));
        // One unread, one being answered: both still hold a handle.
        assert_eq!(table.len(), 2);
        assert_eq!(table.connect(), None);
        assert_eq!(table.end(answered), Ended::Finished);
        assert_eq!(table.end(reading), Ended::Undelivered);
        assert!(table.is_empty());
    }

    #[test]
    fn ids_are_never_reused() {
        let mut table: ConnTable<&str> = ConnTable::new(2);
        let a = table.connect().unwrap();
        let b = table.connect().unwrap();
        assert_eq!((a, b), (1, 2));
        table.end(a);
        let c = table.connect().unwrap();
        assert_eq!(c, 3);
        table.clear();
        assert!(table.is_empty());
        assert_eq!(table.connect(), Some(4));
        assert_eq!(table.connect(), Some(5));
        // A refused connection takes no id either.
        assert_eq!(table.connect(), None);
        table.end(4);
        assert_eq!(table.connect(), Some(6));
    }

    #[test]
    fn an_unknown_id_is_neither_answered_nor_closed() {
        let (mut table, ids) = held(3);
        assert_eq!(table.take(0), None);
        assert_eq!(table.take(99), None);
        assert_eq!(table.end(99), Ended::Unknown);
        assert!(!table.hold(99, "answer"));
        // Nothing of the real ones was touched.
        assert_eq!(table.len(), 3);
        for id in ids {
            assert_eq!(table.take(id), Some("answer"));
        }
    }

    #[test]
    fn a_connection_is_answered_or_closed_once() {
        let (mut table, ids) = held(2);
        assert_eq!(table.take(ids[0]), Some("answer"));
        // A second answer, or a close after the answer, finds nothing.
        assert_eq!(table.take(ids[0]), None);
        assert_eq!(table.take(ids[0]), None);
        // The other one is untouched.
        assert_eq!(table.take(ids[1]), Some("answer"));
    }

    #[test]
    fn a_finished_connection_is_not_answered() {
        let (mut table, ids) = held(1);
        // The hook went away while the request was held.
        assert_eq!(table.end(ids[0]), Ended::Held);
        assert_eq!(table.take(ids[0]), None);
        assert_eq!(table.end(ids[0]), Ended::Unknown);
    }

    #[test]
    fn a_connection_is_not_answered_before_its_frame_is_delivered() {
        let mut table: ConnTable<&str> = ConnTable::new(4);
        let conn = table.connect().unwrap();
        assert_eq!(table.take(conn), None);
        // The refused answer changed nothing: the frame can still be delivered and answered.
        assert!(table.hold(conn, "answer"));
        assert!(!table.hold(conn, "again"));
        assert_eq!(table.take(conn), Some("answer"));
        assert!(!table.hold(conn, "late"));
    }

    #[test]
    fn only_a_held_connection_that_ends_is_news_to_the_engine() {
        let mut table: ConnTable<&str> = ConnTable::new(4);
        let unread = table.connect().unwrap();
        let delivered = table.connect().unwrap();
        let answered = table.connect().unwrap();
        assert!(table.hold(delivered, "answer"));
        assert!(table.hold(answered, "answer"));
        assert!(table.take(answered).is_some());
        assert_eq!(table.end(unread), Ended::Undelivered);
        assert_eq!(table.end(delivered), Ended::Held);
        assert_eq!(table.end(answered), Ended::Finished);
    }

    #[test]
    fn stopping_forgets_every_connection() {
        let (mut table, ids) = held(3);
        table.clear();
        assert!(table.is_empty());
        for id in ids {
            assert_eq!(table.take(id), None);
            assert!(!table.hold(id, "answer"));
            assert_eq!(table.end(id), Ended::Unknown);
        }
    }

    #[test]
    fn a_taken_name_reads_in_use() {
        assert_eq!(PIPE_IN_USE, "The hook pipe is in use by another program");
        // ERROR_ACCESS_DENIED: the first-instance flag met somebody else's pipe.
        assert_eq!(create_failure_message(Some(5)), PIPE_IN_USE);
        // ERROR_PIPE_BUSY: somebody else's pipe allows no further instance.
        assert_eq!(create_failure_message(Some(231)), PIPE_IN_USE);
    }

    #[test]
    fn another_failure_names_its_code_and_nothing_else() {
        assert_eq!(
            create_failure_message(Some(1450)),
            "The hook pipe can't be opened (Windows error 1450)"
        );
        assert_eq!(
            create_failure_message(None),
            "The hook pipe can't be opened"
        );
    }

    #[test]
    fn a_status_is_reported_when_it_changes() {
        let name = r"\\.\pipe\agentnotch-test";
        let mut status = StatusLog::default();
        assert_eq!(
            status.failed(PIPE_IN_USE.to_owned()),
            Some(TransportEvent::Error(PIPE_IN_USE.to_owned()))
        );
        // The retries every 5 s say nothing new.
        assert_eq!(status.failed(PIPE_IN_USE.to_owned()), None);
        assert_eq!(status.failed(PIPE_IN_USE.to_owned()), None);
        // A different failure is news.
        assert_eq!(
            status.failed(create_failure_message(Some(8))),
            Some(TransportEvent::Error(
                "The hook pipe can't be opened (Windows error 8)".to_owned()
            ))
        );
        // The name came free.
        assert_eq!(
            status.listening(name),
            Some(TransportEvent::Listening(name.to_owned()))
        );
        // Every further instance is created silently.
        assert_eq!(status.listening(name), None);
        assert_eq!(
            status.failed(PIPE_IN_USE.to_owned()),
            Some(TransportEvent::Error(PIPE_IN_USE.to_owned()))
        );
        assert_eq!(
            status.listening(name),
            Some(TransportEvent::Listening(name.to_owned()))
        );
    }

    #[test]
    fn the_first_instance_reports_listening_once() {
        let name = r"\\.\pipe\agentnotch-test";
        let mut status = StatusLog::default();
        assert_eq!(
            status.listening(name),
            Some(TransportEvent::Listening(name.to_owned()))
        );
        assert_eq!(status.listening(name), None);
    }

    #[test]
    fn only_this_user_is_a_hook() {
        assert!(peer_allowed(ME, Some(ME)));
        assert!(peer_allowed(ME, Some(&ME.to_ascii_lowercase())));
        // Another user, SYSTEM, and a client whose user could not be read.
        assert!(!peer_allowed(
            ME,
            Some("S-1-5-21-1111111111-2222222222-3333333333-1002")
        ));
        assert!(!peer_allowed(ME, Some(SYSTEM_SID)));
        assert!(!peer_allowed(ME, Some("")));
        assert!(!peer_allowed(ME, None));
        // Not even when the app itself runs as SYSTEM, or its own SID is unknown.
        assert!(!peer_allowed(SYSTEM_SID, Some(SYSTEM_SID)));
        assert!(!peer_allowed("", Some("")));
    }

    #[test]
    fn a_frame_over_8_mib_is_refused_by_its_header() {
        assert_eq!(declared_length([0, 0, 0, 0], MAX_SERVER_MESSAGE), Some(0));
        assert_eq!(
            declared_length(16u32.to_le_bytes(), MAX_SERVER_MESSAGE),
            Some(16)
        );
        assert_eq!(
            declared_length((8u32 << 20).to_le_bytes(), MAX_SERVER_MESSAGE),
            Some(8 << 20)
        );
        assert_eq!(
            declared_length(((8u32 << 20) + 1).to_le_bytes(), MAX_SERVER_MESSAGE),
            None
        );
        assert_eq!(declared_length([0xff; 4], MAX_SERVER_MESSAGE), None);
    }
}
