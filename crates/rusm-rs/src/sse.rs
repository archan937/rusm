//! Ergonomics for a **per-connection** Server-Sent-Events handler — the SSE twin of
//! [`crate::ws`]. The host runs one sandboxed component process *per connection*, so a
//! handler is naturally isolated. SSE is one-way (server → client): the handler emits
//! events; there are no inbound client frames. The platform pushes events to the handler
//! through its **mailbox** — typically a [process-group tag](crate::register_tag) the
//! handler subscribes to in [`Handler::open`], so a publisher's `whereis_tag` + `send`
//! fans out to every open stream. [`Handler::message`] fires once per pushed event;
//! [`Stream::data`] emits it to the client. On disconnect — client close or dropped
//! socket — [`Handler::close`] fires, then the process exits (releasing its tags).
//!
//! The handler writes raw event payloads; the **platform** owns the SSE wire format
//! (`data:`-framing and keep-alive `: ping` heartbeats) and disconnect detection, so a
//! handler never writes framing or a heartbeat and can't leak a connection.

use std::cell::Cell;

use crate::Pid;

/// One SSE connection — the per-connection process's view of its stream. Emit events
/// with [`data`](Self::data); the host's writer process owns the actual response body
/// (frames each payload and pings on idle).
pub struct Stream {
    writer: Pid,
    done: Cell<bool>,
    info: crate::ConnectionInfo,
}

impl Stream {
    /// The connection's writer pid (the emit target / the process whose death is the
    /// disconnect).
    pub fn writer(&self) -> Pid {
        self.writer
    }

    /// This stream's request context — method, path, query, route params, headers, and
    /// peer address (e.g. `stream.info().param("plan")` or the `last-event-id` header).
    pub fn info(&self) -> &crate::ConnectionInfo {
        &self.info
    }

    /// Emit one event to the client. The platform frames it as a `data:` SSE event;
    /// dropped if the client has disconnected.
    pub fn data(&self, payload: &[u8]) {
        crate::send_bytes(self.writer, payload);
    }

    /// End the stream and this process (a server-initiated close). [`Handler::close`]
    /// then fires once, and the process exits — the same teardown as a client disconnect.
    pub fn close(&self) {
        self.done.set(true);
    }
}

/// A per-connection SSE handler — the twin of [`crate::ws::Handler`]. One instance per
/// connection, so `&mut self` is *this connection's* state (keep shared state in a
/// `[components.<name>]` service or `kv`).
pub trait Handler {
    /// The connection opened. Subscribe to your event source here (e.g.
    /// [`register_tag`](crate::register_tag)). Default: do nothing.
    fn open(&mut self, stream: &Stream) {
        let _ = stream;
    }
    /// One event pushed to this stream (e.g. a published message from a subscribed tag).
    /// Emit it with [`Stream::data`].
    fn message(&mut self, stream: &Stream, event: Vec<u8>);
    /// The connection closed — the client disconnected (cleanly or by a dropped socket)
    /// or the handler called [`Stream::close`]. Fires exactly once, before the process
    /// exits. Default: do nothing.
    fn close(&mut self, stream: &Stream) {
        let _ = stream;
    }
}

/// Run `handler` for this connection: learn the writer pid (the host's message 1), fire
/// [`Handler::open`], dispatch each pushed event to [`Handler::message`], and fire
/// [`Handler::close`] when the stream ends (client disconnect or [`Stream::close`]) —
/// then return. Call it from a component's `run`.
pub fn serve<H: Handler>(mut handler: H) {
    // Message 1: the writer pid (decimal — the RUSM "tell me where to answer" convention
    // shared with the WebSocket path).
    let writer = Pid(String::from_utf8(crate::receive_bytes())
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0));
    let stream = Stream {
        writer,
        done: Cell::new(false),
        info: crate::connection().unwrap_or_default(),
    };
    // The writer owns the response body, so its death *is* the client disconnect;
    // monitoring it turns that into the `close` callback.
    crate::monitor(writer);
    handler.open(&stream);
    while !stream.done.get() {
        let data = crate::receive_bytes();
        if crate::down_pid(&data) == Some(writer) {
            break; // client disconnected
        }
        handler.message(&stream, data);
    }
    handler.close(&stream);
}
