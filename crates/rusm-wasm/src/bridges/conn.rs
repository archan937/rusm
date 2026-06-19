//! Building a per-connection [`ConnectionInfo`] from an inbound request — the single
//! source both serving bridges (WebSocket, SSE) use, so the two never disagree on how a
//! connection's context is captured. The handler reads it back through the `connection`
//! actor op (set on the store by [`Spawner::spawn_connection`]).

use std::net::SocketAddr;

use crate::actor::ConnectionInfo;

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
