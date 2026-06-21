// Canonical source: bridges/serve/guest.rs — the serve bridge's Rust guest binding (the
// per-connection WS/SSE handler controls). Synced into rusm-rs (crates/rusm-rs/src/serve.rs)
// by `make sync-bridges`; edit this file, not the copy. `bridge_guest_in_sync` guards drift.

//! Per-connection serving controls for a Rust guest — the request [`ConnectionInfo`] plus
//! the WS/SSE push ops. `ConnectionInfo` and [`connection`] are re-exported at the crate
//! root (`rusm_rs::ConnectionInfo`, `rusm_rs::connection`); the push ops are `pub(crate)`,
//! consumed by the ergonomic [`ws::Connection`](crate::ws) / [`sse::Stream`](crate::sse)
//! handler wrappers.

// The generated `serve` interface bindings.
use crate::rusm::runtime::serve as abi;

/// The HTTP context of a **per-connection** WebSocket or SSE handler — the request that
/// opened this connection. Fixed for the connection's life; read it in your handler's
/// `open` (via [`ws::Connection::info`](crate::ws) / [`sse::Stream::info`](crate::sse), or
/// [`connection`] directly). A normal process (not a connection handler) has no context —
/// [`connection`] returns `None`, and these accessors on a defaulted value are empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionInfo {
    method: String,
    path: String,
    query: String,
    params: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    remote_addr: String,
    subprotocol: Option<String>,
}

impl ConnectionInfo {
    /// Request method, uppercased (`GET`, …).
    pub fn method(&self) -> &str {
        &self.method
    }
    /// Path without the query string (`/events/plan/pages/42`).
    pub fn path(&self) -> &str {
        &self.path
    }
    /// Raw query string without the leading `?` (empty when absent).
    pub fn query(&self) -> &str {
        &self.query
    }
    /// All route parameters captured from the listener's `[serve.routes]` pattern.
    pub fn params(&self) -> &[(String, String)] {
        &self.params
    }
    /// One captured route parameter by name (`:plan` → `param("plan")`).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    /// All request headers (lowercased names, arrival order; a name may repeat).
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }
    /// The first value of header `name` (case-insensitive), or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    /// Peer socket address (`ip:port`), empty if the transport can't report one.
    pub fn remote_addr(&self) -> &str {
        &self.remote_addr
    }
    /// The negotiated WebSocket subprotocol, if any (always `None` for SSE).
    pub fn subprotocol(&self) -> Option<&str> {
        self.subprotocol.as_deref()
    }
}

/// This process's [`ConnectionInfo`] when it is a per-connection WebSocket/SSE handler, or
/// `None` for every other process. A WS/SSE handler usually reads it through
/// [`ws::Connection::info`](crate::ws) / [`sse::Stream::info`](crate::sse) rather than
/// calling this directly.
pub fn connection() -> Option<ConnectionInfo> {
    abi::connection().map(|c| ConnectionInfo {
        method: c.method,
        path: c.path,
        query: c.query,
        params: c.params,
        headers: c.headers,
        remote_addr: c.remote_addr,
        subprotocol: c.subprotocol,
    })
}

/// Send a **text** WebSocket frame on this connection (used by [`ws::Connection::send_text`](crate::ws);
/// a binary frame is a plain `send_bytes` to the writer pid). `false` if this process is
/// not a WebSocket handler, or the socket has closed.
pub(crate) fn ws_send_text(payload: &[u8]) -> bool {
    abi::ws_send_text(payload)
}

/// Close this WebSocket connection with a status `code` + `reason` (used by
/// [`ws::Connection::close`](crate::ws)). No-op for a non-WebSocket process.
pub(crate) fn ws_close(code: u16, reason: &str) {
    abi::ws_close(code, reason);
}

/// Emit a rich SSE event (data + optional event/id/retry); used by [`sse::Stream::emit`](crate::sse).
/// `false` if this process is not an SSE handler or the client has disconnected.
pub(crate) fn sse_send(
    data: &[u8],
    event: Option<&str>,
    id: Option<&str>,
    retry: Option<u32>,
) -> bool {
    abi::sse_send(data, event, id, retry)
}
