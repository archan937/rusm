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
pub mod pg;
pub mod pubsub;
pub mod serve;
pub mod sse;
pub mod streams;
pub mod supervisor;
pub mod wire;
pub mod ws;

/// The cross-process byte [`Stream`](streams::Stream), re-exported at the crate root so the
/// public path stays `rusm_rs::Stream`.
pub use streams::Stream;

/// Process-group tag ops, re-exported at the crate root so the public paths stay
/// `rusm_rs::{register_tag, unregister_tag, whereis_tag, kill_tag}`.
pub use pg::{kill_tag, register_tag, unregister_tag, whereis_tag};

/// The per-connection serving controls, re-exported at the crate root: `ConnectionInfo` +
/// `connection` are public (`rusm_rs::ConnectionInfo`, `rusm_rs::connection`); the WS/SSE
/// push ops stay `pub(crate)`, consumed by the `ws`/`sse` handler wrappers.
pub use serve::{connection, ConnectionInfo};
pub(crate) use serve::{sse_send, ws_close, ws_send_text};

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

