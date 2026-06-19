//! Ergonomics for a **per-connection** WebSocket handler: the host runs one sandboxed
//! component process *per connection*, so a handler is naturally isolated — its state
//! is that one connection's. The host delivers the connection's **writer pid** as the
//! first message (the process that owns the socket sink); reply by sending frames to it
//! (see [`Connection::send`]). Each later message is one inbound frame; when the socket
//! closes — clean close or dropped connection alike — [`Handler::close`] fires, then the
//! process exits.

use crate::Pid;

pub use crate::send_bytes as send;

/// One WebSocket connection — the per-connection process's view of its socket. Reply
/// to the client by writing frames; the host's writer process owns the actual sink.
pub struct Connection {
    writer: Pid,
    info: crate::ConnectionInfo,
}

impl Connection {
    /// The connection's writer pid (the reply target).
    pub fn writer(&self) -> Pid {
        self.writer
    }

    /// This connection's request context — method, path, query, route params, headers,
    /// peer address, and negotiated subprotocol (e.g. `conn.info().param("room")`).
    pub fn info(&self) -> &crate::ConnectionInfo {
        &self.info
    }

    /// Send one **binary** frame back to the client. Dropped if the socket has closed.
    pub fn send(&self, frame: &[u8]) {
        crate::send_bytes(self.writer, frame);
    }

    /// Send one **text** frame back to the client (UTF-8). Returns `false` if the socket
    /// has closed. Use this for browsers that expect text messages (the default `send`
    /// emits a binary frame).
    pub fn send_text(&self, text: &str) -> bool {
        crate::ws_send_text(text.as_bytes())
    }
}

/// A per-connection WebSocket handler. There is one handler instance per connection,
/// so `&mut self` is *this connection's* state (no cross-connection sharing — keep
/// shared state in a `[components.<name>]` service or `kv`).
pub trait Handler {
    /// The connection opened. Default: do nothing.
    fn open(&mut self, conn: &Connection) {
        let _ = conn;
    }
    /// One inbound frame from the client.
    fn message(&mut self, conn: &Connection, data: Vec<u8>);
    /// The connection closed — the client disconnected, cleanly or by a dropped socket.
    /// Fires exactly once, before the process exits. Default: do nothing.
    fn close(&mut self, conn: &Connection) {
        let _ = conn;
    }
}

/// Run `handler` for this connection: learn the writer pid (the host's message 1), fire
/// [`Handler::open`], dispatch each inbound frame to [`Handler::message`], and fire
/// [`Handler::close`] when the socket closes — then return. Call it from a component's `run`.
pub fn serve<H: Handler>(mut handler: H) {
    // Message 1: the writer pid (decimal — the RUSM "tell me where to answer"
    // convention shared with the other per-connection paths).
    let writer = Pid(String::from_utf8(crate::receive_bytes())
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0));
    let conn = Connection {
        writer,
        info: crate::connection().unwrap_or_default(),
    };
    // The writer process owns the socket, so its death *is* the disconnect (clean close
    // or dropped connection alike); monitoring it turns that into the `close` callback.
    crate::monitor(writer);
    handler.open(&conn);
    loop {
        let data = crate::receive_bytes();
        if crate::down_pid(&data) == Some(writer) {
            handler.close(&conn);
            return;
        }
        handler.message(&conn, data);
    }
}
