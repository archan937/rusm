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

pub use rusm_rs_macros::{handlers, main, service};
pub use serde;
pub use serde_json;

pub mod http;
pub mod actor;
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

/// The Erlang Process core, re-exported at the crate root so the public paths stay
/// `rusm_rs::{Pid, send, receive, spawn, monitor, register, …}`.
pub use actor::*;
pub(crate) use actor::stash;

/// Process-group tag ops, re-exported at the crate root so the public paths stay
/// `rusm_rs::{register_tag, unregister_tag, whereis_tag, kill_tag}`.
pub use pg::{kill_tag, register_tag, unregister_tag, whereis_tag};

/// The per-connection serving controls, re-exported at the crate root: `ConnectionInfo` +
/// `connection` are public (`rusm_rs::ConnectionInfo`, `rusm_rs::connection`); the WS/SSE
/// push ops stay `pub(crate)`, consumed by the `ws`/`sse` handler wrappers.
pub use serve::{connection, ConnectionInfo};
pub(crate) use serve::{sse_send, ws_close, ws_send_text};

pub use supervisor::{Strategy, Supervisor};

// The Erlang Process core (Pid, send/receive, spawn, monitor, registry) is the actor bridge
// — see the `actor` module (bridges/actor/guest.rs), re-exported at the crate root below.

