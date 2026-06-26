//! `rusm-otp` — the Wasm-free Erlang/OTP core of RUSM.
//!
//! Lightweight processes (Tokio tasks), each with a message mailbox, over a
//! sharded process table. A process is killed by aborting its task — Tokio gives
//! us that handle for free — so a process carries just one channel. This crate
//! must never depend on or reference Wasmtime — the actor model stands alone;
//! Wasm is a separate, optional backend (`rusm-wasm`). See `docs/01-architecture.md`.

mod exit;
mod lifecycle;
mod message;
mod net;
mod pid;
mod runtime;
mod stream;
mod supervisor;

pub use exit::{ExitReason, MonitorRef};
pub use lifecycle::LogLevel;
pub use message::{Message, Meta, Received};
pub use pid::Pid;
pub use runtime::{Context, ProcessHandle, ProcessInfo, Runtime, TimerRef};
pub use stream::{stream, stream_with_capacity, StreamHandle, StreamWriter};
pub use supervisor::{Strategy, Supervisor};
