// Canonical source: bridges/actor/guest.rs — the actor bridge's Rust guest binding (the
// Erlang Process core: Pid, send/receive, spawn, monitor, the registry). Synced into rusm-rs
// (crates/rusm-rs/src/actor.rs) by `make sync-bridges`; edit this file, not the copy. These
// are re-exported at the rusm-rs crate root, so the public paths stay `rusm_rs::{Pid, send,
// receive, spawn, …}`. `bridge_guest_in_sync` guards drift.

//! The Erlang Process core for a Rust guest — the foundation the other bridges layer on.

// The generated `actor` interface bindings.
use crate::rusm::runtime::actor as abi;
use serde::de::DeserializeOwned;
use serde::Serialize;

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
    Pid(abi::own_pid())
}

/// Every live pid (subject to capability).
pub fn list() -> Vec<Pid> {
    abi::list_processes().into_iter().map(Pid).collect()
}

/// Spawn a registered component by name → its pid (capability-gated `spawn`).
pub fn spawn(component: &str) -> Result<Pid, String> {
    abi::spawn(component).map(Pid)
}

/// Spawn a **dynamic JS** instance of a registered runner template `component`, loading
/// its bundle at runtime from `source` — `inline:<js>` (the bundle itself),
/// `kv:<bucket>/<key>` (the node store), or `url:`/`http(s)://…` (fetched). The JS runs
/// under the template's *declared* profile (the guest chooses the code, never the
/// capabilities). Gated by `spawn` plus the source's I/O capability (`storage`/`network`).
pub fn spawn_from(component: &str, source: &str) -> Result<Pid, String> {
    abi::spawn_from(component, source).map(Pid)
}

/// Monitor a process: when it dies, this process receives a `__down` message
/// (see [`supervisor`]). Capability-gated like spawn.
pub fn monitor(target: Pid) {
    abi::monitor(target.0);
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
    abi::register(name)
}

/// Look up a registered name, or `None` if unregistered.
pub fn whereis(name: &str) -> Option<Pid> {
    abi::whereis(name).map(Pid)
}

/// Release a registered name.
pub fn unregister(name: &str) -> bool {
    abi::unregister(name)
}

/// Set this process's human-readable label (shown in introspection).
pub fn set_label(label: &str) {
    abi::set_label(label);
}

/// Whether a pid is still alive (subject to capability).
pub fn is_alive(pid: Pid) -> bool {
    abi::is_alive(pid.0)
}

/// Kill a pid (subject to capability).
pub fn kill(pid: Pid) -> bool {
    abi::kill(pid.0)
}

/// Schedule `message` to be delivered to `to` after `delay_ms` milliseconds —
/// Erlang's `erlang:send_after/3`. Returns a timer handle for [`cancel_timer`].
/// If `to` is gone when the timer fires, the delivery is a silent no-op.
pub fn send_after(to: Pid, delay_ms: u64, message: &[u8]) -> u64 {
    abi::send_after(to.0, delay_ms, message)
}

/// Cancel a pending timer by the handle returned by [`send_after`]. Returns `true`
/// if the timer was found and aborted before it fired; `false` if unknown — already
/// fired, already cancelled, or never issued by this process.
pub fn cancel_timer(timer_ref: u64) -> bool {
    abi::cancel_timer(timer_ref)
}

/// Send raw bytes to a pid (dropped if it's gone).
pub fn send_bytes(to: Pid, msg: &[u8]) {
    abi::send(to.0, msg);
}

/// **Set aside** the message just received while an RPC client awaits its reply, keeping the
/// host-managed metadata with it. The host holds it apart from the live queue (so the next
/// [`receive_bytes`] never re-reads it), until [`unstash`] returns it. Stashing host-side —
/// not in a guest buffer — is what keeps each set-aside message bound to its own request on
/// replay; a guest cannot carry the host-only metadata itself.
pub(crate) fn stash(message: &[u8]) {
    abi::stash(message);
}

/// Return every [`stash`]ed message (in arrival order) to the front of the mailbox, so the
/// app's own `receive` sees them next — each rebound to its own request — before newer mail.
pub(crate) fn unstash() {
    abi::unstash();
}

/// Block until the next message arrives; returns its raw bytes. The host delivers any
/// [`stash`]ed-then-[`unstash`]ed mail first (arrival order preserved, metadata intact).
pub fn receive_bytes() -> Vec<u8> {
    abi::receive()
}

/// Like [`receive_bytes`], but gives up after `timeout_ms` and returns `None` —
/// Erlang's `receive … after`. Stashed-then-unstashed mail is delivered immediately
/// (it can't "time out"); otherwise this waits up to the deadline. The basis for an SSE
/// heartbeat: wait for the next event *or* the tick, whichever comes first.
pub fn receive_bytes_timeout(timeout_ms: u64) -> Option<Vec<u8>> {
    abi::receive_timeout(timeout_ms)
}

/// Send a serializable value as a JSON message — the wire shared with TS guests.
pub fn send<T: Serialize>(to: Pid, msg: &T) -> serde_json::Result<()> {
    abi::send(to.0, &serde_json::to_vec(msg)?);
    Ok(())
}

/// Block for the next message and deserialize it from JSON.
pub fn receive<T: DeserializeOwned>() -> serde_json::Result<T> {
    serde_json::from_slice(&abi::receive())
}

/// Like [`receive`], but gives up after `timeout_ms`: `None` on timeout, otherwise
/// the next message deserialized from JSON.
pub fn receive_timeout<T: DeserializeOwned>(timeout_ms: u64) -> Option<serde_json::Result<T>> {
    receive_bytes_timeout(timeout_ms).map(|raw| serde_json::from_slice(&raw))
}

// `stash`/`unstash` are thin host-op wrappers (no guest-side state to unit-test); the
// set-aside-and-replay behaviour — and that each replayed message keeps its own metadata — is
// tested host-side in `rusm-otp` (`stash_then_unstash_redelivers_with_each_item_s_own_meta`)
// and end to end through a guest in the `rusm-wasm` integration tests.
