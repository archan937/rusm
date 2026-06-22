//! Building a per-connection [`ConnectionInfo`] from an inbound request — the single
//! source both serving bridges (WebSocket, SSE) use, so the two never disagree on how a
//! connection's context is captured. The handler reads it back through the `connection`
//! actor op (set on the store by [`Spawner::spawn_connection`]).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use super::routed::{Resolver, Routed};

/// A control frame a WebSocket handler emits to its connection's writer — the typed channel
/// that backs `ws-send-text` and `ws-close` (binary frames take the plain `send`→writer
/// path). One bounded channel for both, so a slow client back-pressures either uniformly.
pub(crate) enum WsOut {
    /// A text frame (the bytes are UTF-8).
    Text(Vec<u8>),
    /// A close frame with a status code + reason, then the writer ends the connection.
    Close(u16, String),
    /// A pong echoing a client ping's payload — the reader forwards inbound pings here so the
    /// writer (the sole socket owner) answers them. Not guest-visible.
    Pong(Vec<u8>),
}

/// A **rich** SSE event an SSE handler emits to its writer (the `sse-send` op) — `data`
/// plus optional `event` name, `id` (echoed by the client as `Last-Event-ID`), and `retry`
/// backoff. The plain `data:`-only path stays a `send` to the writer pid.
pub(crate) struct SseEvent {
    pub(crate) data: Vec<u8>,
    pub(crate) event: Option<String>,
    pub(crate) id: Option<String>,
    pub(crate) retry: Option<u32>,
}
use crate::bridges::serve::ConnectionInfo;
use crate::caps::Capabilities;
use crate::{PreparedComponent, Spawner};

/// Where a per-connection serving handler (WebSocket or SSE) comes from — shared by both
/// bridges so they resolve a connection identically. Either one fixed component for every
/// connection (an unrouted listener), or a `[serve.routes]` table resolving the path to a
/// registered handler component with captured params.
#[derive(Clone)]
pub(crate) enum Source {
    /// One handler for every connection (no `[serve.routes]`); no path params.
    Single {
        prepared: PreparedComponent,
        /// `Some` for a TS/JS bundle on the js-runner (sent as the runner's first message).
        bundle: Option<Arc<Vec<u8>>>,
        caps: Capabilities,
    },
    /// `[serve.routes]` routing: resolve the path to a registered handler component, with
    /// captured params flowing to its connection context. `caps` keys the per-component
    /// capability profile by name.
    Routed {
        resolve: Resolver,
        caps: Arc<HashMap<String, Capabilities>>,
    },
}

/// A resolved connection handler: the component to spawn, its optional JS bundle, the
/// capability profile to run it under, and the captured route params.
pub(crate) struct Resolved {
    pub(crate) prepared: PreparedComponent,
    pub(crate) bundle: Option<Arc<Vec<u8>>>,
    pub(crate) caps: Capabilities,
    pub(crate) params: Vec<(String, String)>,
}

impl Source {
    /// Resolve a connection to its handler. `None` when a routed listener has no matching
    /// route (the caller answers `404`); an unrouted listener always matches its single
    /// handler with no params.
    pub(crate) fn resolve(&self, spawner: &Spawner, method: &str, path: &str) -> Option<Resolved> {
        match self {
            Source::Single {
                prepared,
                bundle,
                caps,
            } => Some(Resolved {
                prepared: prepared.clone(),
                bundle: bundle.clone(),
                caps: caps.clone(),
                params: Vec::new(),
            }),
            Source::Routed { resolve, caps } => match resolve(method, path) {
                Routed::Found {
                    component, params, ..
                } => {
                    let entry = spawner.lookup(&component)?;
                    Some(Resolved {
                        // A routed WS/SSE handler is a fixed component (never a dynamic
                        // template), so `prepared` is present; `None` would mean a misrouted
                        // template — treated as no match (the caller answers 404).
                        prepared: entry.prepared?,
                        bundle: entry.bundle,
                        caps: caps.get(&component).cloned()?,
                        params,
                    })
                }
                Routed::MethodNotAllowed | Routed::NotFound => None,
            },
        }
    }
}

/// Capture the connection context from `req`: method, the path and query split apart,
/// every header (names are already lowercased by `http`, kept in arrival order so a
/// repeated header is preserved), the peer address, the route `params` the listener
/// captured, and any negotiated `subprotocol` (always `None` for SSE).
pub(crate) fn connection_info<B>(
    req: &hyper::Request<B>,
    peer: Option<SocketAddr>,
    params: Vec<(String, String)>,
    subprotocol: Option<String>,
) -> ConnectionInfo {
    let uri = req.uri();
    ConnectionInfo {
        method: req.method().as_str().to_string(),
        path: uri.path().to_string(),
        query: uri.query().unwrap_or_default().to_string(),
        params,
        headers: req
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
        remote_addr: peer.map(|addr| addr.to_string()).unwrap_or_default(),
        subprotocol,
    }
}
