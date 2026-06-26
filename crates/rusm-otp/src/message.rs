use std::any::Any;
use std::sync::Arc;

use crate::exit::{ExitReason, MonitorRef};
use crate::pid::Pid;
use crate::stream::StreamHandle;

/// A user message delivered to a process mailbox.
///
/// Opaque bytes, handed to the recipient **by value** — ownership moves into its
/// mailbox, so processes never share memory, exactly like Erlang. Mailboxes are
/// unbounded (also like Erlang). Phase 2 carries raw bytes; the Wasm backend
/// (Phase 6) will copy these across isolated guest memories unchanged.
pub type Message = Vec<u8>;

/// **Opaque** per-message metadata that rides *beside* a [`Received`] item through the
/// mailbox — never inside the payload, so a guest can neither read nor forge it.
///
/// It is `Arc<dyn Any>` on purpose: the Wasm-free core moves it without ever
/// interpreting it, keeping `rusm-otp` free of any Wasm/host concept. A host layer
/// (`rusm-wasm`) attaches a boxed value when it sends and downcasts it after it
/// receives; the runtime is none the wiser. `None` for plain sends and every system
/// signal — the hot path pays nothing (no allocation, a single `None` word).
pub type Meta = Option<Arc<dyn Any + Send + Sync>>;

/// A mailbox entry: a [`Received`] item plus its opaque [`Meta`] sidecar. The channel
/// carries this; [`recv`](crate::Context::recv) hands back the `Received` and stashes
/// the meta for [`current_meta`](crate::Context::current_meta). Keeping meta *out* of
/// `Received` is deliberate — the public receive API and every native process body are
/// unchanged, and meta can never be mistaken for message bytes.
pub struct Envelope {
    pub received: Received,
    pub meta: Meta,
}

impl Envelope {
    /// An entry with no metadata — plain sends and every system signal use this.
    pub fn bare(received: Received) -> Self {
        Self {
            received,
            meta: None,
        }
    }
}

/// What a process pulls from its mailbox with [`recv`](crate::Context::recv): an
/// ordinary message, a byte **stream**, or a system notification the runtime
/// injected. Exit and down signals share the one mailbox (and FIFO order) with
/// user messages — exactly as Erlang delivers `{'EXIT', …}` / `{'DOWN', …}`.
///
/// Not `Clone`/`Eq`: a [`Received::Stream`] owns a live, single-consumer stream
/// handle. Equality is provided manually for the comparable variants (so tests
/// and matches keep working); two streams are never considered equal.
#[derive(Debug)]
pub enum Received {
    /// A user message sent with [`send`](crate::Runtime::send).
    Message(Message),
    /// A byte stream sent with [`send_stream`](crate::Runtime::send_stream); read
    /// chunks from the handle at your own pace (back-pressured).
    Stream(StreamHandle),
    /// A monitored process terminated (set up with [`monitor`](crate::Runtime::monitor)).
    Down {
        reference: MonitorRef,
        pid: Pid,
        reason: ExitReason,
    },
    /// A linked process exited while this one traps exits (see
    /// [`link`](crate::Runtime::link) and [`set_trap_exit`](crate::Runtime::set_trap_exit)).
    Exit { from: Pid, reason: ExitReason },
}

impl Received {
    /// The user-message bytes, or `None` for a stream or system notification —
    /// convenient when a process only cares about ordinary messages.
    pub fn message(self) -> Option<Message> {
        match self {
            Received::Message(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The byte stream, or `None` for any other kind — convenient when a process
    /// expects a stream.
    pub fn stream(self) -> Option<StreamHandle> {
        match self {
            Received::Stream(handle) => Some(handle),
            _ => None,
        }
    }
}

impl PartialEq for Received {
    /// Structural equality for the comparable variants; two live streams are
    /// never equal (a stream isn't a value). Lets `assert_eq!` keep working on
    /// messages and signals without making `Received` falsely `Eq`.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Received::Message(a), Received::Message(b)) => a == b,
            (
                Received::Down {
                    reference: r1,
                    pid: p1,
                    reason: rs1,
                },
                Received::Down {
                    reference: r2,
                    pid: p2,
                    reason: rs2,
                },
            ) => r1 == r2 && p1 == p2 && rs1 == rs2,
            (
                Received::Exit {
                    from: f1,
                    reason: rs1,
                },
                Received::Exit {
                    from: f2,
                    reason: rs2,
                },
            ) => f1 == f2 && rs1 == rs2,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit::{ExitReason, MonitorRef};

    #[test]
    fn message_extracts_user_bytes_only() {
        assert_eq!(
            Received::Message(b"hi".to_vec()).message(),
            Some(b"hi".to_vec())
        );
        let down = Received::Down {
            reference: MonitorRef(1),
            pid: Pid::from_raw(2),
            reason: ExitReason::Crashed,
        };
        assert_eq!(down.message(), None);
        let exit = Received::Exit {
            from: Pid::from_raw(3),
            reason: ExitReason::Normal,
        };
        assert_eq!(exit.message(), None);
    }

    #[test]
    fn stream_extracts_only_a_stream() {
        let (_writer, handle) = crate::stream::stream();
        assert!(Received::Stream(handle).stream().is_some());
        // A stream is neither a user message...
        let (_w2, h2) = crate::stream::stream();
        assert!(Received::Stream(h2).message().is_none());
        // ...nor is a user message a stream.
        assert!(Received::Message(b"x".to_vec()).stream().is_none());
    }

    #[test]
    fn distinct_kinds_are_never_equal() {
        // Covers the catch-all arm of the manual `PartialEq`.
        assert_ne!(
            Received::Message(b"x".to_vec()),
            Received::Exit {
                from: Pid::from_raw(1),
                reason: ExitReason::Normal,
            }
        );
        // Two live streams are never considered equal (a stream isn't a value).
        let (_w1, a) = crate::stream::stream();
        let (_w2, b) = crate::stream::stream();
        assert_ne!(Received::Stream(a), Received::Stream(b));
    }
}
