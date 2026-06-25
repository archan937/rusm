//! The REPL host: how a [`Node`](crate::Node) evaluates a line of JavaScript
//! against its live processes — the engine behind the `rusm attach` shell.
//!
//! Only the **contract** lives here; this crate stays Wasm-free. The actual JS
//! engine is injected by the composition layer (`rusm-cli`), which owns the
//! WebAssembly runtime and implements [`ReplHost`] over it. The node depends on
//! the trait, never on Wasmtime.

use std::future::Future;
use std::pin::Pin;

use crate::protocol::ServerMessage;

/// The outcome of evaluating one REPL line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvalOutcome {
    /// The rendered return value (empty for a statement that yields nothing).
    pub value: String,
    /// Captured `console.*` lines, in emission order.
    pub output: Vec<String>,
    /// The thrown error message, if evaluation failed. A failed eval still leaves
    /// the session alive for the next line.
    pub error: Option<String>,
}

impl EvalOutcome {
    /// An error outcome with no value or output — used for host-side failures
    /// (e.g. a timed-out or unreachable session) that never reached the guest.
    pub fn from_error(message: impl Into<String>) -> Self {
        Self {
            value: String::new(),
            output: Vec::new(),
            error: Some(message.into()),
        }
    }

    /// The wire message sent back to the attached client.
    pub fn into_message(self) -> ServerMessage {
        ServerMessage::EvalResult {
            value: self.value,
            output: self.output,
            error: self.error,
        }
    }
}

/// A boxed `Send` future of an [`EvalOutcome`] — the dependency-free stand-in for
/// an `async fn` in an object-safe trait (the workspace avoids `async-trait`).
pub type EvalFuture<'a> = Pin<Box<dyn Future<Output = EvalOutcome> + Send + 'a>>;

/// Opens REPL sessions. Injected into a [`Node`](crate::Node) by the layer that
/// owns the WebAssembly runtime; the node itself stays Wasm-free.
pub trait ReplHost: Send + Sync {
    /// Open a fresh, isolated evaluation session. Each attach connection gets its
    /// own, so bindings never leak between clients.
    fn open_session(&self) -> Box<dyn ReplSession>;
}

/// A live REPL session: a persistent JavaScript scope where bindings set on one
/// line are visible on the next. Dropping the session tears down its process.
pub trait ReplSession: Send {
    /// Evaluate one line, returning its outcome. Implementations bound this with
    /// an internal timeout so a wedged line (e.g. awaiting a message that never
    /// arrives) cannot hang the connection — on timeout the session resets and the
    /// outcome carries an error.
    fn eval(&mut self, code: String) -> EvalFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_error_carries_only_the_message() {
        let outcome = EvalOutcome::from_error("boom");
        assert_eq!(outcome.error.as_deref(), Some("boom"));
        assert!(outcome.value.is_empty() && outcome.output.is_empty());
    }

    #[test]
    fn into_message_maps_every_field() {
        let outcome = EvalOutcome {
            value: "42".into(),
            output: vec!["hi".into()],
            error: None,
        };
        assert_eq!(
            outcome.into_message(),
            ServerMessage::EvalResult {
                value: "42".into(),
                output: vec!["hi".into()],
                error: None,
            }
        );
    }
}
