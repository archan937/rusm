//! **rusm-rs** — the ergonomic Rust *guest* crate for RUSM: write a component (or
//! a service) in Rust over the `rusm:runtime` actor world, the Rust twin of
//! rusm-ts. It wraps the raw `wit-bindgen` actor bindings into a small, idiomatic
//! API — `Pid`, `send`/`receive` (serde-typed), `spawn`, the registry, `Stream`,
//! and the `#[service]` macro. A guest depends on this and generates the `process`
//! world mapping the actor import to `rusm_rs::rusm::runtime::actor`, then
//! `export!`s its own `run` — the wit-bindgen library/binary split, so the actor
//! interface is imported exactly once (see the README / the `rs-guest` fixture).
//!
//! Blocking "just works": `receive`/`Stream::read` suspend the instance's fiber
//! (freeing the scheduler thread) until data arrives — like a Rust host process,
//! and like an Erlang `receive`.

// This crate owns the actor **import** bindings; a guest maps to them with
// `with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor }` and
// `export!`s its own `run` — the wit-bindgen library/binary split, so the actor
// interface is imported exactly once in the final component. (See the `rs-guest`
// fixture / the docs for the guest-side boilerplate.)
wit_bindgen::generate!({
    world: "imports",
    path: "wit",
});

use rusm::runtime::actor;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub use rusm_rs_macros::{handlers, main, service};
pub use serde;
pub use serde_json;

pub mod http;
pub mod kv;
pub mod logging;
pub mod pubsub;
pub mod sse;
pub mod supervisor;
pub mod wire;
pub mod ws;

pub use supervisor::{Strategy, Supervisor};

/// A process identifier (Erlang's pid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pid(pub u64);

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A handle to a **callback** the caller passed into a service call: invoking it
/// sends the argument back to the caller as a message (the function stays in the
/// caller; only the invocation travels). Service handlers take a `Callback<A>`
/// parameter; on the caller side the typed client takes a closure `FnMut(A)`.
pub struct Callback<A> {
    to: Pid,
    cbref: u64,
    _marker: std::marker::PhantomData<fn(A)>,
}

impl<A: Serialize> Callback<A> {
    #[doc(hidden)]
    pub fn __new(to: Pid, cbref: u64) -> Self {
        Self {
            to,
            cbref,
            _marker: std::marker::PhantomData,
        }
    }

    /// Invoke the caller's callback with `arg`.
    pub fn call(&self, arg: A) {
        let msg = serde_json::json!({ "op": "__cb", "cbref": self.cbref, "args": [arg] });
        send_bytes(
            self.to,
            &serde_json::to_vec(&msg).expect("callback serializes"),
        );
    }
}

/// This process's own pid (`self()`).
pub fn me() -> Pid {
    Pid(actor::own_pid())
}

/// Every live pid (subject to capability).
pub fn list() -> Vec<Pid> {
    actor::list_processes().into_iter().map(Pid).collect()
}

/// Spawn a registered component by name → its pid (capability-gated `spawn`).
pub fn spawn(component: &str) -> Result<Pid, String> {
    actor::spawn(component).map(Pid)
}

/// Spawn a **dynamic JS** instance of a registered runner template `component`, loading
/// its bundle at runtime from `source` — `inline:<js>` (the bundle itself),
/// `kv:<bucket>/<key>` (the node store), or `url:`/`http(s)://…` (fetched). The JS runs
/// under the template's *declared* profile (the guest chooses the code, never the
/// capabilities). Gated by `spawn` plus the source's I/O capability (`storage`/`network`).
pub fn spawn_from(component: &str, source: &str) -> Result<Pid, String> {
    actor::spawn_from(component, source).map(Pid)
}

/// Monitor a process: when it dies, this process receives a `__down` message
/// (see [`supervisor`]). Capability-gated like spawn.
pub fn monitor(target: Pid) {
    actor::monitor(target.0);
}

/// Parse a monitor `__down` message (`{"__down":"<pid>","reason":...}`) into the dead
/// process's [`Pid`], or `None` for an ordinary message. The single source for `__down`
/// decoding across the guest crate ([`pubsub`] and [`ws`]); a fast prefix check keeps
/// ordinary messages from ever being parsed.
pub fn down_pid(msg: &[u8]) -> Option<Pid> {
    if !msg.starts_with(br#"{"__down":"#) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(msg).ok()?;
    value.get("__down")?.as_str()?.parse().ok().map(Pid)
}

/// Register this process under a name in the node registry.
pub fn register(name: &str) -> bool {
    actor::register(name)
}

/// Look up a registered name, or `None` if unregistered.
pub fn whereis(name: &str) -> Option<Pid> {
    actor::whereis(name).map(Pid)
}

/// Release a registered name.
pub fn unregister(name: &str) -> bool {
    actor::unregister(name)
}

/// Set this process's human-readable label (shown in introspection).
pub fn set_label(label: &str) {
    actor::set_label(label);
}

/// Join **this** process to a process-group `tag` (Erlang's `pg`): a process may hold many
/// tags, a tag many processes. Released automatically on exit. Unprivileged — a process
/// tags itself; terminating a group is the gated [`kill_tag`].
pub fn register_tag(tag: &str) {
    actor::register_tag(tag);
}

/// Leave a process-group `tag` this process holds.
pub fn unregister_tag(tag: &str) {
    actor::unregister_tag(tag);
}

/// Live members of process-group `tag` (empty if unknown).
pub fn whereis_tag(tag: &str) -> Vec<Pid> {
    actor::whereis_tag(tag).into_iter().map(Pid).collect()
}

/// Terminate every live member of process-group `tag`; returns how many were killed.
/// Capability-gated by `process-control` (it terminates other processes); returns `0` if
/// denied or the tag is empty.
pub fn kill_tag(tag: &str) -> u32 {
    actor::kill_tag(tag)
}

/// The HTTP context of a **per-connection** WebSocket or SSE handler — the request that
/// opened this connection. Fixed for the connection's life; read it in your handler's
/// `open` (via [`ws::Connection::info`] / [`sse::Stream::info`], or [`connection`] directly).
/// A normal process (not a connection handler) has no context — [`connection`] returns
/// `None`, and these accessors on a defaulted value are empty.
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
/// [`ws::Connection::info`] / [`sse::Stream::info`] rather than calling this directly.
pub fn connection() -> Option<ConnectionInfo> {
    actor::connection().map(|c| ConnectionInfo {
        method: c.method,
        path: c.path,
        query: c.query,
        params: c.params,
        headers: c.headers,
        remote_addr: c.remote_addr,
        subprotocol: c.subprotocol,
    })
}

/// Whether a pid is still alive (subject to capability).
pub fn is_alive(pid: Pid) -> bool {
    actor::is_alive(pid.0)
}

/// Kill a pid (subject to capability).
pub fn kill(pid: Pid) -> bool {
    actor::kill(pid.0)
}

/// Send raw bytes to a pid (dropped if it's gone).
pub fn send_bytes(to: Pid, msg: &[u8]) {
    actor::send(to.0, msg);
}

/// Send a **text** WebSocket frame on this connection (used by [`ws::Connection::send_text`];
/// a binary frame is a plain [`send_bytes`] to the writer pid). `false` if this process is
/// not a WebSocket handler, or the socket has closed.
pub(crate) fn ws_send_text(payload: &[u8]) -> bool {
    actor::ws_send_text(payload)
}

thread_local! {
    /// Messages the RPC client set aside while awaiting a reply, so the app's own
    /// `receive` still sees them (the guest is single-threaded — one mailbox).
    static INBOX: std::cell::RefCell<std::collections::VecDeque<Vec<u8>>> =
        std::cell::RefCell::new(std::collections::VecDeque::new());
}

/// Set a message aside for the app's own `receive` (used by the RPC client).
pub(crate) fn stash(raw: Vec<u8>) {
    INBOX.with(|q| q.borrow_mut().push_back(raw));
}

/// Block until the next message arrives; returns its raw bytes. Drains any mail
/// the RPC client set aside first (FIFO preserved).
pub fn receive_bytes() -> Vec<u8> {
    if let Some(raw) = INBOX.with(|q| q.borrow_mut().pop_front()) {
        return raw;
    }
    actor::receive()
}

/// Like [`receive_bytes`], but gives up after `timeout_ms` and returns `None` —
/// Erlang's `receive … after`. Mail the RPC client set aside is delivered
/// immediately (a pending message can't "time out"); otherwise this waits up to
/// the deadline. The basis for an SSE heartbeat: wait for the next event *or* the
/// tick, whichever comes first.
pub fn receive_bytes_timeout(timeout_ms: u64) -> Option<Vec<u8>> {
    if let Some(raw) = INBOX.with(|q| q.borrow_mut().pop_front()) {
        return Some(raw);
    }
    actor::receive_timeout(timeout_ms)
}

/// Send a serializable value as a JSON message — the wire shared with TS guests.
pub fn send<T: Serialize>(to: Pid, msg: &T) -> serde_json::Result<()> {
    actor::send(to.0, &serde_json::to_vec(msg)?);
    Ok(())
}

/// Block for the next message and deserialize it from JSON.
pub fn receive<T: DeserializeOwned>() -> serde_json::Result<T> {
    serde_json::from_slice(&actor::receive())
}

/// Like [`receive`], but gives up after `timeout_ms`: `None` on timeout, otherwise
/// the next message deserialized from JSON.
pub fn receive_timeout<T: DeserializeOwned>(timeout_ms: u64) -> Option<serde_json::Result<T>> {
    receive_bytes_timeout(timeout_ms).map(|raw| serde_json::from_slice(&raw))
}

/// A back-pressured byte stream to or from another process — the same primitive
/// as the host's, surfaced ergonomically. `read` suspends the fiber until a chunk
/// arrives; `None` is end-of-stream.
pub struct Stream {
    handle: u64,
}

impl Stream {
    /// Open a stream to a pid; `None` if the target is gone.
    pub fn open(to: Pid) -> Option<Stream> {
        actor::stream_open(to.0).map(|handle| Stream { handle })
    }

    /// Block until an incoming stream arrives, and take it for reading.
    pub fn accept() -> Stream {
        Stream {
            handle: actor::stream_accept(),
        }
    }

    /// Write one chunk; `false` once the reader is gone.
    pub fn write(&self, chunk: &[u8]) -> bool {
        actor::stream_write(self.handle, chunk)
    }

    /// Read the next chunk, or `None` at end-of-stream.
    pub fn read(&self) -> Option<Vec<u8>> {
        actor::stream_read(self.handle)
    }

    /// Close the write end (signals end-of-stream to the reader).
    pub fn close(self) {
        actor::stream_close(self.handle);
    }
}
