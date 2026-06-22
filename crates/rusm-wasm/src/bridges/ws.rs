//! Serving **WebSockets** (Phase 11). A WebSocket is only HTTP for its handshake;
//! after the `Upgrade` it's a raw bidirectional stream — and the handshake + the
//! protocol live entirely on the host, which RUSM controls. So WS never goes
//! through `wasi:http`: **hyper** surfaces the upgrade and each connection is its own
//! supervised task — a failure drops only that socket, never the listener. The WS
//! protocol itself runs through [`super::ws_codec`] (a frame transport on tungstenite's
//! frame primitives) so **permessage-deflate** is available; the [`serve_ws_echo`]
//! baseline still uses tungstenite's `WebSocketStream` directly.
//!
//! Two entry points: [`serve_ws_echo`] is a host-side echo (the transport baseline);
//! [`WsServer`] runs an actual **WASM component process** per connection — each
//! inbound frame becomes one mailbox message, replies flow back through a Wasm-free
//! writer process that owns the socket sink. Wasmtime stays inside this crate; the
//! `rusm-otp` core never sees hyper, tungstenite, or `wasi:http`.

use std::convert::Infallible;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

use std::collections::HashMap;

use super::conn::{Resolved, Source, WsOut};
use super::routed::Resolver;
use crate::caps::Capabilities;
use crate::{PreparedComponent, Spawner, WasmRuntime};

/// Serve a WebSocket **echo** on `listener` until it closes — one supervised task
/// per connection. Abort the task driving this to stop.
pub async fn serve_ws_echo(listener: TcpListener) {
    loop {
        let Ok((stream, _peer)) = listener.accept().await else {
            break;
        };
        stream.set_nodelay(true).ok();
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(stream),
                    hyper::service::service_fn(echo_upgrade),
                )
                // `with_upgrades` is what lets `hyper::upgrade::on` hand us the
                // raw stream after the 101.
                .with_upgrades()
                .await;
        });
    }
}

/// Answer the HTTP `Upgrade` with a 101 and spawn a host-side echo task. A request
/// without a WebSocket key gets a plain 426.
async fn echo_upgrade(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Empty<Bytes>>, Infallible> {
    let Some(accept) = ws_accept(&req) else {
        return Ok(upgrade_required());
    };
    tokio::spawn(async move {
        let Some(mut ws) = upgraded_ws(req, None).await else {
            return;
        };
        while let Some(Ok(message)) = ws.next().await {
            if message.is_close() {
                break;
            }
            if (message.is_text() || message.is_binary()) && ws.send(message).await.is_err() {
                break;
            }
        }
    });
    Ok(switching_protocols(accept, None, None))
}

/// The `Sec-WebSocket-Accept` for a request, or `None` if it carries no WS key.
pub(crate) fn ws_accept(req: &hyper::Request<hyper::body::Incoming>) -> Option<String> {
    req.headers()
        .get("sec-websocket-key")
        .and_then(|k| k.to_str().ok())
        .map(|key| derive_accept_key(key.as_bytes()))
}

/// Complete the `Upgrade` and wrap the raw stream as a server-side `WebSocketStream`.
/// `max_message_size` caps an inbound message/frame in bytes (a larger one closes the
/// connection); `None` uses the transport default.
pub(crate) async fn upgraded_ws(
    req: hyper::Request<hyper::body::Incoming>,
    max_message_size: Option<usize>,
) -> Option<WebSocketStream<TokioIo<Upgraded>>> {
    let upgraded = hyper::upgrade::on(req).await.ok()?;
    let config = max_message_size.map(|max| {
        use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
        WebSocketConfig::default()
            .max_message_size(Some(max))
            .max_frame_size(Some(max))
    });
    Some(WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, config).await)
}

pub(crate) fn switching_protocols(
    accept: String,
    subprotocol: Option<String>,
    extensions: Option<String>,
) -> hyper::Response<Empty<Bytes>> {
    let mut builder = hyper::Response::builder()
        .status(101)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-accept", accept);
    // Echo the negotiated subprotocol so the client adopts it (RFC 6455).
    if let Some(proto) = subprotocol {
        builder = builder.header("sec-websocket-protocol", proto);
    }
    // Echo the negotiated extension (permessage-deflate) so the client enables it (RFC 7692).
    if let Some(ext) = extensions {
        builder = builder.header("sec-websocket-extensions", ext);
    }
    builder.body(Empty::new()).unwrap()
}

pub(crate) fn upgrade_required() -> hyper::Response<Empty<Bytes>> {
    hyper::Response::builder()
        .status(426)
        .body(Empty::new())
        .unwrap()
}

/// A `404` for a routed WebSocket listener whose path matched no route — the handshake is
/// refused before any upgrade.
fn not_found() -> hyper::Response<Empty<Bytes>> {
    hyper::Response::builder()
        .status(404)
        .body(Empty::new())
        .unwrap()
}

/// A `403` for a handshake whose `Origin` isn't in the listener's allow-list (CSWSH
/// protection) — the upgrade is refused before any process is spawned.
fn forbidden() -> hyper::Response<Empty<Bytes>> {
    hyper::Response::builder()
        .status(403)
        .body(Empty::new())
        .unwrap()
}

/// Serves each WebSocket connection with a **WASM component process** — the actor
/// way. A connection's inbound messages land in the component's mailbox (one
/// message = one frame); its replies go to a per-connection **writer** process that
/// owns the socket sink. The component is pure sandboxed logic (no IO); the writer
/// and reader are Wasm-free `rusm-otp` glue. A handler crash drops only that
/// connection's processes — never the listener or other sockets.
#[derive(Clone)]
pub struct WsServer {
    source: Source,
    spawner: Arc<Spawner>,
    /// Idle keep-alive: if no frame flows for this long, the writer sends a WebSocket
    /// **ping** — keeping the connection alive through idle-reaping proxies. (An inbound
    /// client ping is answered with a pong by the reader, via the writer.)
    keepalive: std::time::Duration,
    /// Supported subprotocols (the listener's `subprotocols` list). On the handshake the
    /// first client-offered one present here is negotiated + echoed; empty = none.
    subprotocols: Arc<Vec<String>>,
    /// Max concurrent connections (the listener's `max_connections`); at the cap a new
    /// connection is dropped before the handshake. `None` = unlimited.
    max_connections: Option<usize>,
    /// Max inbound frame size in bytes (the listener's `max_message_size`); a larger frame
    /// closes the connection instead of allocating. `None` = the transport default.
    max_message_size: Option<usize>,
    /// Allowed `Origin`s for the handshake (CSWSH protection); empty = any origin.
    allowed_origins: Arc<Vec<String>>,
    /// Negotiate **permessage-deflate** when the client offers it (the listener's
    /// `compression`). `false` = never compressed.
    compress: bool,
    /// TLS acceptor (the listener's `tls`); when set, each connection is TLS-terminated
    /// before the handshake (`wss`). `None` = plain `ws`.
    tls: Option<Arc<super::tls::TlsAcceptor>>,
}

impl WasmRuntime {
    /// Build a WebSocket server that runs `prepared` (a `rusm:runtime` actor
    /// component) as the handler process for **every** connection, under `caps`.
    pub fn ws_server(&self, prepared: &PreparedComponent, caps: Capabilities) -> WsServer {
        WsServer {
            source: Source::Single {
                prepared: prepared.clone(),
                bundle: None,
                caps,
            },
            spawner: Arc::clone(&self.spawner),
            keepalive: WS_KEEPALIVE,
            subprotocols: Arc::new(Vec::new()),
            max_connections: None,
            max_message_size: None,
            allowed_origins: Arc::new(Vec::new()),
            compress: false,
            tls: None,
        }
    }

    /// Build a WebSocket server whose per-connection handler is a **TypeScript/JS
    /// bundle** (Bun-built) running on the embedded js-runner — the TS twin of
    /// [`ws_server`](Self::ws_server). The guest is a worker (`export default`): its
    /// first `Process.receive()` is the writer pid, then each inbound frame.
    pub fn ws_server_js(&self, bundle: impl Into<Vec<u8>>, caps: Capabilities) -> WsServer {
        WsServer {
            source: Source::Single {
                prepared: self.js_runner().clone(),
                bundle: Some(Arc::new(bundle.into())),
                caps,
            },
            spawner: Arc::clone(&self.spawner),
            keepalive: WS_KEEPALIVE,
            subprotocols: Arc::new(Vec::new()),
            max_connections: None,
            max_message_size: None,
            allowed_origins: Arc::new(Vec::new()),
            compress: false,
            tls: None,
        }
    }

    /// Build a **routed** WebSocket server: each connection's path resolves (via `resolve`,
    /// the listener's `[serve.routes]`) to a registered handler component, run per
    /// connection with the captured path params in its connection context. `caps` gives
    /// each handler component's capability profile by name. A path that matches no route is
    /// answered `404` (the handshake is refused).
    pub fn routed_ws_server(
        &self,
        resolve: Resolver,
        caps: HashMap<String, Capabilities>,
    ) -> WsServer {
        WsServer {
            source: Source::Routed {
                resolve,
                caps: Arc::new(caps),
            },
            spawner: Arc::clone(&self.spawner),
            keepalive: WS_KEEPALIVE,
            subprotocols: Arc::new(Vec::new()),
            max_connections: None,
            max_message_size: None,
            allowed_origins: Arc::new(Vec::new()),
            compress: false,
            tls: None,
        }
    }
}

/// How long to wait for a connection's handler to run its `close` and exit on its own
/// (after its writer is killed) before force-reaping it. A cooperating handler — the SDK
/// `ws::serve`, which monitors the writer — exits in microseconds; this only caps a
/// handler that ignores the disconnect.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// Bound on a connection's pending control frames (the `ws-send-text` / `ws-close` channel)
/// — a slow client back-pressures the handler (it parks on send), never buffering unbounded.
const WS_OUT_CAPACITY: usize = 64;

/// Default idle keep-alive interval: send a ping after this long with no frame, so an idle
/// connection survives proxy idle-reaping. Override per-listener with [`WsServer::with_keepalive`].
const WS_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(30);

impl WsServer {
    /// Set the idle keep-alive ping interval (default 30s). A connection with no frame for
    /// this long gets a server ping, so idle-reaping proxies don't drop it.
    pub fn with_keepalive(mut self, interval: std::time::Duration) -> Self {
        self.keepalive = interval;
        self
    }

    /// Set the supported WebSocket subprotocols. On the handshake, the first client-offered
    /// subprotocol present in this list is negotiated — echoed in the `101` and surfaced to
    /// the handler via its connection context. Empty (default) negotiates none.
    pub fn with_subprotocols(mut self, subprotocols: Vec<String>) -> Self {
        self.subprotocols = Arc::new(subprotocols);
        self
    }

    /// Cap concurrent connections; at the cap a new connection is dropped before the
    /// handshake (a flood can't spawn unbounded handler instances). `None` = unlimited.
    pub fn with_max_connections(mut self, max: Option<usize>) -> Self {
        self.max_connections = max;
        self
    }

    /// Cap the inbound frame size in bytes; a larger frame closes the connection. `None` =
    /// the transport default.
    pub fn with_max_message_size(mut self, max: Option<usize>) -> Self {
        self.max_message_size = max;
        self
    }

    /// Restrict the handshake to these `Origin`s (CSWSH protection); empty = any origin.
    pub fn with_allowed_origins(mut self, origins: Vec<String>) -> Self {
        self.allowed_origins = Arc::new(origins);
        self
    }

    /// Negotiate permessage-deflate when the client offers it (the listener's `compression`).
    pub fn with_compression(mut self, on: bool) -> Self {
        self.compress = on;
        self
    }

    /// Terminate TLS on each connection with this acceptor (`wss`); `None` = plain `ws`.
    pub fn with_tls(mut self, tls: Option<Arc<super::tls::TlsAcceptor>>) -> Self {
        self.tls = tls;
        self
    }

    /// Whether the request's `Origin` is allowed (always true when no allow-list is set).
    fn origin_allowed(&self, req: &hyper::Request<hyper::body::Incoming>) -> bool {
        if self.allowed_origins.is_empty() {
            return true;
        }
        match req.headers().get("origin").and_then(|o| o.to_str().ok()) {
            Some(origin) => self.allowed_origins.iter().any(|a| a == origin),
            None => false, // an allow-list is set but the request carries no Origin
        }
    }

    /// Negotiate a subprotocol: the first one the client offers (its `Sec-WebSocket-Protocol`
    /// header, comma-separated) that this listener supports. `None` if the client offered
    /// none, none matched, or the listener supports none.
    fn negotiate_subprotocol(&self, req: &hyper::Request<hyper::body::Incoming>) -> Option<String> {
        if self.subprotocols.is_empty() {
            return None;
        }
        let offered = req.headers().get("sec-websocket-protocol")?.to_str().ok()?;
        offered
            .split(',')
            .map(str::trim)
            .find(|o| self.subprotocols.iter().any(|s| s == o))
            .map(String::from)
    }

    /// Serve WebSockets on `listener` until it closes — one connection per task.
    pub async fn serve(self, listener: TcpListener) {
        // A connection-cap semaphore (when `max_connections` is set): a permit is acquired
        // before the handshake and held for the connection's whole life, so the cap bounds
        // *live* connections, not just in-flight handshakes.
        let limiter = self
            .max_connections
            .map(|n| Arc::new(tokio::sync::Semaphore::new(n)));
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            // At the cap, drop the socket before the handshake — a flood can't spawn handlers.
            let permit = match &limiter {
                Some(sem) => match Arc::clone(sem).try_acquire_owned() {
                    Ok(p) => Some(p),
                    Err(_) => continue,
                },
                None => None,
            };
            let server = self.clone();
            tokio::spawn(async move {
                // TCP_NODELAY + TLS termination (when configured) off the accept loop. The
                // upgrade rides over whatever IO this is — `hyper::upgrade::on` abstracts it.
                let Ok(io) = super::tls::MaybeTlsStream::accept(stream, &server.tls).await else {
                    return; // a failed TLS handshake drops just this connection
                };
                // The permit moves through the (call-once-per-WS) service into `upgrade`,
                // which hands it to the connection task; a `Mutex<Option<_>>` carries a
                // move-once value through hyper's `Fn` service.
                let permit = Arc::new(std::sync::Mutex::new(permit));
                let service = hyper::service::service_fn(move |req| {
                    let server = server.clone();
                    let permit = Arc::clone(&permit);
                    async move {
                        let permit = permit.lock().unwrap().take();
                        server.upgrade(req, Some(peer), permit).await
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(io), service)
                    .with_upgrades()
                    .await;
            });
        }
    }

    async fn upgrade(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
        peer: Option<std::net::SocketAddr>,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<hyper::Response<Empty<Bytes>>, Infallible> {
        // Log the incoming WS request (gated by `[log] level`): a valid handshake as the
        // `101` upgrade, a non-WS request as the `426` we reject it with.
        let method = req.method().as_str().to_string();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();
        let log =
            |status| super::access::log_request(&self.spawner.rt, "ws", &method, &path, status);
        // Reject a disallowed `Origin` before anything else (CSWSH): a `403`, no handshake.
        if !self.origin_allowed(&req) {
            log(403);
            return Ok(forbidden());
        }
        // Resolve the route first: an unmatched path is a `404` (no handshake). An unrouted
        // listener always matches its single handler.
        let Some(Resolved {
            prepared,
            bundle,
            caps,
            params,
        }) = self.source.resolve(&self.spawner, &method, &path)
        else {
            log(404);
            return Ok(not_found());
        };
        let Some(accept) = ws_accept(&req) else {
            log(426);
            return Ok(upgrade_required());
        };
        log(101);
        // Negotiate a subprotocol (echoed in the 101 + surfaced via the connection context),
        // then capture the connection context before the request is consumed; route params
        // come from the resolver.
        let subprotocol = self.negotiate_subprotocol(&req);
        // Negotiate permessage-deflate when enabled and the client offers it (RFC 7692); the
        // echoed extension value goes in the 101, and `deflate` drives the per-message codec.
        let extensions = self
            .compress
            .then(|| {
                super::ws_codec::negotiate_permessage_deflate(
                    req.headers()
                        .get("sec-websocket-extensions")
                        .and_then(|v| v.to_str().ok()),
                )
            })
            .flatten();
        let deflate = extensions.is_some();
        let connection = super::conn::connection_info(&req, peer, params, subprotocol.clone());
        let server = self.clone();
        let max_size = self.max_message_size;
        tokio::spawn(async move {
            // Hold the connection-cap permit for the connection's whole life.
            let _permit = permit;
            if let Ok(upgraded) = hyper::upgrade::on(req).await {
                let conn = super::ws_codec::WsConn::new(TokioIo::new(upgraded), deflate, max_size);
                server
                    .run_connection(conn, prepared, bundle, caps, connection)
                    .await;
            }
        });
        Ok(switching_protocols(accept, subprotocol, extensions))
    }

    /// Wire one upgraded connection to a fresh component process (the resolved handler).
    async fn run_connection(
        &self,
        conn: super::ws_codec::WsConn<TokioIo<Upgraded>>,
        prepared: PreparedComponent,
        bundle: Option<Arc<Vec<u8>>>,
        caps: Capabilities,
        connection: crate::bridges::serve::ConnectionInfo,
    ) {
        let (mut sink, mut stream) = conn.split();
        let rt = self.spawner.rt.clone();

        // Writer: a Wasm-free process owning the socket sink. It races the handler's outputs
        // and the keep-alive — **binary** frames arrive via its mailbox (a plain `send` to the
        // writer pid), **text/close** on a bounded channel the handler feeds via
        // `ws-send-text`/`ws-close`, and **pong** forwarded by the reader for an inbound ping
        // (the writer is the sole socket owner). Compression, if negotiated, is applied here
        // per message. All IO stays out of the sandboxed component.
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<WsOut>(WS_OUT_CAPACITY);
        let reader_out = out_tx.clone(); // the reader uses this to answer inbound pings
        let keepalive = self.keepalive;
        let writer = rt.spawn(move |mut ctx| async move {
            loop {
                tokio::select! {
                    received = ctx.recv() => match received.message() {
                        Some(bytes) => {
                            if sink.send_binary(bytes).await.is_err() {
                                break;
                            }
                        }
                        None => break, // mailbox closed / a non-message signal
                    },
                    out = out_rx.recv() => match out {
                        // A text frame (ws-send-text).
                        Some(WsOut::Text(payload)) => {
                            if sink.send_text(payload).await.is_err() {
                                break;
                            }
                        }
                        // A pong answering a client ping (forwarded by the reader).
                        Some(WsOut::Pong(payload)) => {
                            if sink.send_pong(payload).await.is_err() {
                                break;
                            }
                        }
                        // A server-initiated close with a code + reason (ws-close), then end.
                        Some(WsOut::Close(code, reason)) => {
                            let _ = sink.send_close(code, reason).await;
                            break;
                        }
                        None => break, // handler gone (its control sender dropped)
                    },
                    // Idle keep-alive: no frame for `keepalive` → ping. Resets each loop, so it
                    // fires only on a genuinely idle link.
                    _ = tokio::time::sleep(keepalive) => {
                        if sink.send_ping(Vec::new()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // The sandboxed handler. For a JS bundle, the runner's first message is the
        // bundle itself; the writer pid then lands as the guest's first receive.
        // (Per-connection handlers aren't named in the platform lifecycle log — the
        // server doesn't carry the serve name; add it to `WsServer` if that's wanted.)
        let component =
            self.spawner
                .spawn_connection(&prepared, caps, connection, Some(out_tx), None);
        if let Some(bundle) = &bundle {
            rt.send(component.pid(), bundle.as_ref().clone());
        }
        rt.send(component.pid(), writer.pid().raw().to_string().into_bytes());

        // Pump inbound messages into the component's mailbox (one message per WS message).
        // Control frames are handled here: a ping is answered with a pong (via the writer), a
        // pong is ignored, a close ends the loop. A protocol error also ends the connection.
        use super::ws_codec::WsMessage;
        while let Some(result) = stream.recv().await {
            match result {
                Ok(WsMessage::Text(data)) | Ok(WsMessage::Binary(data)) => {
                    rt.send(component.pid(), data);
                }
                Ok(WsMessage::Ping(payload)) => {
                    // Answer via the writer (the sole socket owner); if it's gone, end.
                    if reader_out.send(WsOut::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Ok(WsMessage::Pong(_)) => {} // a reply to our keep-alive ping — nothing to do
                Ok(WsMessage::Close(_)) | Err(_) => break,
            }
        }

        // Connection done. Kill the writer first: a handler monitoring it (the SDK
        // `ws::serve` does) then receives the `__down`, runs its `close`, and exits — so
        // we await that exit (briefly) before reaping. A handler that ignores the
        // disconnect is force-killed once the grace elapses.
        let component_pid = component.pid();
        writer.kill();
        if tokio::time::timeout(CLOSE_GRACE, component.join())
            .await
            .is_err()
        {
            rt.kill(component_pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::Message;

    /// Spawn a process registered as `"collector"` that forwards every message it
    /// receives to the returned channel — lets a test observe what a guest reports.
    fn collector(rt: &rusm_otp::Runtime) -> tokio::sync::mpsc::UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let proc = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(b);
                }
            }
        });
        rt.register("collector", proc.pid());
        rx
    }

    /// The next event from a [`collector`] channel, or panic after 5s.
    async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<u8> {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a lifecycle event within 5s")
            .expect("collector channel stays open")
    }

    /// Drive a spawned per-connection echo handler `handler` directly (bypassing the socket)
    /// and assert the connection loop **skips a stray `__down`** — a monitor down for a pid
    /// other than the writer must never be echoed to the client as an inbound frame. Shared by
    /// the RS/Go/TS cases below (the three connection-loop implementations).
    async fn assert_skips_stray_down(rt: &rusm_otp::Runtime, handler: rusm_otp::Pid) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // The collector stands in for the writer: the handler echoes frames to it.
        let writer = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(b);
                }
            }
        });
        rt.send(handler, writer.pid().raw().to_string().into_bytes()); // msg 1: the writer pid
        rt.send(handler, br#"{"__down":"999999"}"#.to_vec()); // a stray __down (not the writer)
        rt.send(handler, b"frame".to_vec()); // a real inbound frame
        let got = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("the connection loop must not hang or mis-handle the stray __down")
            .expect("collector channel stays open");
        assert_eq!(
            got, b"frame",
            "the stray __down was skipped; only the real frame was echoed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rs_ws_connection_loop_skips_a_stray_down() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        const RS_WS_ECHO: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_echo.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(RS_WS_ECHO).unwrap(), "run")
            .unwrap();
        let handler = wr.spawn_component_with(&prepared, CapabilityProfile::Trusted.capabilities());
        assert_skips_stray_down(&rt, handler.pid()).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn go_ws_connection_loop_skips_a_stray_down() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        const GO_WS_ECHO: &[u8] = include_bytes!("../../tests/fixtures/go_ws_echo.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(GO_WS_ECHO).unwrap(), "run")
            .unwrap();
        let handler = wr.spawn_component_with(&prepared, CapabilityProfile::Trusted.capabilities());
        assert_skips_stray_down(&rt, handler.pid()).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn js_connection_loop_skips_a_stray_down() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        // An inline per-connection handler that echoes each frame to the writer — driving the
        // js-runner's `__rusm_connection` loop (the TS twin of the RS/Go cases).
        const JS_ECHO: &str =
            "module.exports.default = { websocket: { message: (w, ev) => Process.send(w, ev) } };";
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let handler = wr.spawn_js_with(
            JS_ECHO.as_bytes(),
            CapabilityProfile::Trusted.capabilities(),
        );
        assert_skips_stray_down(&rt, handler.pid()).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_handler_runs_open_message_and_close_on_disconnect() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // A full-lifecycle handler (rs-ws-lifecycle) reports "open"/"close" to a
        // registered collector and echoes frames — proving `close` fires when the client
        // disconnects (the host kills the writer; the handler, monitoring it, sees the
        // `__down` and runs `close` before exiting).
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        ws.send(Message::text("hi")).await.unwrap();
        assert_eq!(&ws.next().await.unwrap().unwrap().into_data()[..], b"hi");

        ws.close(None).await.unwrap(); // disconnect
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on disconnect"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_ws_handler_fires_close_on_disconnect() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // An inline JS bundle in the `websocket({…})` shape rusm-ts emits — open/close
        // report to a registered collector, message echoes. Proves the per-connection
        // js-runner path drives open/message/close and fires `close` on disconnect (the
        // shape that used to hit the dead resident `__rusm_role` branch and no-op).
        const BUNDLE: &str = r#"
            const report = (e) => { const c = Process.whereis("collector"); if (c !== null) Process.send(c, e); };
            module.exports.default = {
              websocket: {
                open:    (conn)       => report("open"),
                message: (conn, data) => Process.send(conn, data),
                close:   (conn)       => report("close"),
              },
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let server = wr.ws_server_js(BUNDLE, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        ws.send(Message::text("hi ts")).await.unwrap();
        assert_eq!(&ws.next().await.unwrap().unwrap().into_data()[..], b"hi ts");

        ws.close(None).await.unwrap(); // disconnect
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on disconnect"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_ws_handler_fires_close_on_disconnect() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // A Go (TinyGo) full-lifecycle handler (go-ws-lifecycle) — open/close report to a
        // registered collector, message echoes — proving `web.WebSocket.Close` fires on
        // disconnect (monitor-the-writer), resembling the Rust and TS handlers.
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/go_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        ws.send(Message::text("hi go")).await.unwrap();
        assert_eq!(&ws.next().await.unwrap().unwrap().into_data()[..], b"hi go");

        ws.close(None).await.unwrap(); // disconnect
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on disconnect"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn echoes_a_websocket_message() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_ws_echo(listener));

        let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::text("hello ws")).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(&reply.into_data()[..], b"hello ws");

        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wasm_component_handles_a_websocket() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The reply comes from a sandboxed WASM component (rs-ws-echo), not the host.
        const WS_ECHO: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_echo.wasm");
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_ECHO).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::text("hi component")).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(&reply.into_data()[..], b"hi component");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_component_handles_a_websocket() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The reply comes from a sandboxed Go (TinyGo) component built on the rusm-go
        // `web.WebSocket` API — one process per connection, echoing each frame.
        const WS_ECHO: &[u8] = include_bytes!("../../tests/fixtures/go_ws_echo.wasm");
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_ECHO).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::text("hi go")).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(&reply.into_data()[..], b"hi go");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_reclaims_every_held_process() {
        // The control USP: components parked on `receive` (here, handlers awaiting a
        // writer pid that never comes) must not leak — `shutdown` aborts them all and
        // frees their pooled instances, so a dropped engine never starves the next.
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        use std::time::Duration;

        const WS_ECHO: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_echo.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_ECHO).unwrap(), "run")
            .unwrap();

        let n = 8u64;
        for _ in 0..n {
            // Drop the handle on purpose — the process stays parked (a leak, without
            // shutdown). Trusted just to keep the spawn unconditional.
            let _ = wr.spawn_component_with(&prepared, CapabilityProfile::Trusted.capabilities());
        }
        for _ in 0..200 {
            if rt.process_count() as u64 >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            rt.process_count() as u64 >= n,
            "the parked handlers are alive"
        );

        assert!(
            wr.shutdown() as u64 >= n,
            "shutdown reports the processes it aborted"
        );
        for _ in 0..200 {
            if rt.process_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rt.process_count(), 0, "shutdown reclaimed every process");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_component_handles_a_websocket() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The reply comes from a TypeScript worker (Bun-built) on the js-runner.
        const TS_WS_ECHO: &str = include_str!("../../tests/fixtures/ts_ws_echo.js");
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let server = wr.ws_server_js(
            TS_WS_ECHO.as_bytes().to_vec(),
            CapabilityProfile::Trusted.capabilities(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::text("hi from TS")).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(&reply.into_data()[..], b"hi from TS");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_handler_reads_its_connection_context() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // A WS handler reads its request path + query via `Connection::info` across the
        // upgrade — the WS twin of the SSE context test, proving the shared connection
        // context reaches the WebSocket path too.
        const WS_CONN: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_CONN).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/chat/room1?x=1"))
            .await
            .unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            b"ctx /chat/room1 q=x=1 proto=-",
            "the WS handler reads its path + query from the connection context"
        );

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_handler_can_send_a_text_frame() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // A handler replying via `Connection::send_text` must reach the client as a *text*
        // frame (the default `send` is binary) — proving the additive `ws-send-text` op.
        const WS_TEXT: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_text.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_TEXT).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"hi".to_vec())).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert!(reply.is_text(), "the reply is a text frame, not binary");
        assert_eq!(reply.into_text().unwrap().as_str(), "hi");

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_ws_handler_can_send_a_text_frame() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The Go SDK twin: web.Conn.SendText must reach the client as a text frame — RS/Go
        // parity for the additive ws-send-text op.
        const GO_WS_TEXT: &[u8] = include_bytes!("../../tests/fixtures/go_ws_text.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(GO_WS_TEXT).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"hi go".to_vec())).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert!(reply.is_text(), "the Go reply is a text frame");
        assert_eq!(reply.into_text().unwrap().as_str(), "hi go");

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ts_ws_handler_can_send_a_text_frame() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The TS SDK twin: socket.sendText must reach the client as a text frame through the
        // js-runner's __ws_send_text primitive — RS/Go/TS parity.
        const TS_WS_TEXT: &[u8] = include_bytes!("../../tests/fixtures/ts_ws_text.js");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let server = wr.ws_server_js(
            TS_WS_TEXT.to_vec(),
            CapabilityProfile::Trusted.capabilities(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"hi ts".to_vec())).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert!(reply.is_text(), "the TS reply is a text frame");
        assert_eq!(reply.into_text().unwrap().as_str(), "hi ts");

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_handler_can_close_with_a_code_and_reason() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // A handler calling Connection::close must send a Close frame carrying the status
        // code + reason — proving the additive ws-close op.
        const WS_CLOSE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_close.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_CLOSE).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"go".to_vec())).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            Message::Close(Some(frame)) => {
                assert_eq!(
                    u16::from(frame.code),
                    1000,
                    "the close code reaches the client"
                );
                assert_eq!(
                    frame.reason.as_str(),
                    "bye",
                    "the close reason reaches the client"
                );
            }
            other => panic!("expected a Close frame, got {other:?}"),
        }

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_ws_handler_can_close_with_a_code_and_reason() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The Go SDK twin: web.Conn.Close must send a Close frame with the code + reason —
        // RS/Go parity for the additive ws-close op.
        const GO_WS_CLOSE: &[u8] = include_bytes!("../../tests/fixtures/go_ws_close.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(GO_WS_CLOSE).unwrap(), "run")
            .unwrap();
        let server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"go".to_vec())).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1000);
                assert_eq!(frame.reason.as_str(), "bye");
            }
            other => panic!("expected a Close frame from Go, got {other:?}"),
        }

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ts_ws_handler_can_close_with_a_code_and_reason() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // The TS SDK twin: socket.close must send a Close frame with the code + reason
        // through the js-runner's __ws_close primitive — RS/Go/TS parity.
        const TS_WS_CLOSE: &[u8] = include_bytes!("../../tests/fixtures/ts_ws_close.js");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let server = wr.ws_server_js(
            TS_WS_CLOSE.to_vec(),
            CapabilityProfile::Trusted.capabilities(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"go".to_vec())).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1000);
                assert_eq!(frame.reason.as_str(), "bye");
            }
            other => panic!("expected a Close frame from TS, got {other:?}"),
        }

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_idle_ws_connection_gets_a_keepalive_ping() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // An idle connection (no frames either way) must receive a server keep-alive ping,
        // so idle-reaping proxies don't drop it. Short interval for the test.
        const WS_CONN: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_CONN).unwrap(), "run")
            .unwrap();
        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_keepalive(std::time::Duration::from_millis(150));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        // Stay idle; a keep-alive ping must arrive within a few intervals.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("a keep-alive ping within 3s")
            .unwrap()
            .unwrap();
        assert!(
            frame.is_ping(),
            "idle connection gets a keep-alive ping, got {frame:?}"
        );

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_listener_negotiates_a_subprotocol() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        // A listener offering `graphql-ws` negotiates it from a client offering `mqtt,
        // graphql-ws` — echoed in the 101 and read by the handler via its connection context.
        const WS_CONN: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_CONN).unwrap(), "run")
            .unwrap();
        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_subprotocols(vec!["graphql-ws".to_string()]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut req = format!("ws://{addr}/").into_client_request().unwrap();
        req.headers_mut().insert(
            "sec-websocket-protocol",
            "mqtt, graphql-ws".parse().unwrap(),
        );
        let (mut ws, resp) = tokio_tungstenite::connect_async(req).await.unwrap();

        // The 101 echoes the negotiated subprotocol (first offered that's supported)…
        assert_eq!(
            resp.headers()
                .get("sec-websocket-protocol")
                .and_then(|h| h.to_str().ok()),
            Some("graphql-ws"),
            "the handshake echoes the negotiated subprotocol"
        );
        // …and the handler reads it from its connection context.
        assert_eq!(
            next_event(&mut rx).await,
            b"ctx / q= proto=graphql-ws",
            "the handler sees the negotiated subprotocol"
        );

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_listener_rejects_a_disallowed_origin() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        // CSWSH protection: with an allow-list set, a handshake from an unlisted `Origin` is
        // refused (403, no upgrade), while a listed one connects and the handler runs.
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();
        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_allowed_origins(vec!["https://app.example.com".to_string()]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // A foreign Origin is refused — the upgrade never happens.
        let mut bad = format!("ws://{addr}/").into_client_request().unwrap();
        bad.headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        assert!(
            tokio_tungstenite::connect_async(bad).await.is_err(),
            "a disallowed Origin is refused before the handshake"
        );

        // The allowed Origin connects and the handler's `open` fires.
        let mut good = format!("ws://{addr}/").into_client_request().unwrap();
        good.headers_mut()
            .insert("origin", "https://app.example.com".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async(good).await.unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            b"open",
            "the allowed Origin completes the handshake"
        );

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_listener_closes_an_oversized_frame() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // `max_message_size` caps an inbound frame: a small frame echoes (the connection
        // works), then a frame past the cap closes the connection rather than allocating it.
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();
        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_max_message_size(Some(8));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        // A small frame round-trips — the connection is healthy.
        ws.send(Message::binary(b"hi".to_vec())).await.unwrap();
        assert_eq!(&ws.next().await.unwrap().unwrap().into_data()[..], b"hi");

        // A frame past the 8-byte cap tears the connection down (no echo of it).
        ws.send(Message::binary(vec![b'x'; 64])).await.ok();
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) => break true,
                    Some(Ok(m)) if m.is_close() => break true,
                    Some(Ok(_)) => continue, // any queued small echo; keep reading
                }
            }
        })
        .await
        .expect("the connection ends after the oversized frame");
        assert!(ended, "an oversized frame closes the connection");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ws_listener_caps_concurrent_connections() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;

        // `max_connections = 1`: the first connection is served, a second while it's live is
        // dropped before the handshake, and once the first closes a new one is admitted again.
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();
        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_max_connections(Some(1));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // First connection: admitted (its `open` fires).
        let (mut first, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "the first is admitted");

        // Second, while the first is live: dropped at the cap (the socket closes mid-handshake).
        assert!(
            tokio_tungstenite::connect_async(format!("ws://{addr}/"))
                .await
                .is_err(),
            "a second connection is refused at the cap"
        );

        // Close the first → its permit releases → a new connection is admitted again.
        first.close(None).await.ok();
        assert_eq!(next_event(&mut rx).await, b"close", "the first tears down");
        let admitted = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok((ws, _)) = tokio_tungstenite::connect_async(format!("ws://{addr}/")).await
                {
                    break ws;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("a new connection is admitted after the first releases its slot");
        assert_eq!(
            next_event(&mut rx).await,
            b"open",
            "the freed slot admits a new connection"
        );
        drop(admitted);

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serves_wss_over_native_tls() {
        use crate::{CapabilityProfile, WasmRuntime};
        use futures_util::{SinkExt, StreamExt};
        use rusm_otp::Runtime;
        use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
        use tokio_rustls::rustls::{ClientConfig, RootCertStore};
        use tokio_rustls::TlsConnector;
        use tokio_tungstenite::tungstenite::Message;

        // A WebSocket listener with TLS serves `wss`: the upgrade rides over a TLS-terminated
        // connection, and an echoed frame round-trips. Proves the `with_upgrades`-over-TLS path.
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let acceptor = crate::tls_acceptor(
            cert.serialize_pem().unwrap().as_bytes(),
            cert.serialize_private_key_pem().as_bytes(),
        )
        .unwrap();
        let cert_der = CertificateDer::from(cert.serialize_der().unwrap());

        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_tls(Some(Arc::new(acceptor)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // TLS-connect (trusting the self-signed cert), then do the WS handshake over it.
        let mut roots = RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .expect("the TLS handshake succeeds");
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", tls)
            .await
            .expect("the WebSocket handshake completes over TLS");

        ws.send(Message::text("over tls")).await.unwrap();
        assert_eq!(
            &ws.next().await.unwrap().unwrap().into_data()[..],
            b"over tls",
            "the frame echoes back over wss"
        );

        ws.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn negotiates_and_round_trips_permessage_deflate() {
        use crate::{CapabilityProfile, WasmRuntime};
        use rusm_otp::Runtime;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // A real permessage-deflate round-trip through the running server: a raw client offers
        // the extension, the 101 echoes it, the client sends a masked RSV1 *deflated* frame,
        // and the echoing handler's reply comes back as an RSV1 deflated frame that inflates to
        // the original. (tokio-tungstenite can't drive this — it has no deflate — so we speak
        // the wire directly, reusing the codec's own deflate transform.)
        const WS_LIFECYCLE: &[u8] = include_bytes!("../../tests/fixtures/rs_ws_lifecycle.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(WS_LIFECYCLE).unwrap(), "run")
            .unwrap();
        let server = wr
            .ws_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_compression(true);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(
            b"GET / HTTP/1.1\r\nHost: rusm\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Extensions: permessage-deflate\r\n\r\n",
        )
        .await
        .unwrap();

        // Read the handshake response head; it must accept the extension.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            conn.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).to_lowercase();
        assert!(head.starts_with("http/1.1 101"), "got: {head}");
        assert!(
            head.contains("sec-websocket-extensions: permessage-deflate"),
            "the 101 negotiates permessage-deflate: {head}"
        );

        // Send a masked, RSV1, deflated text frame (a small payload → a 1-byte length).
        let payload = b"hello compress hello compress".to_vec();
        let compressed = crate::bridges::ws_codec::deflate_message(&payload).unwrap();
        assert!(compressed.len() < 126, "payload fits a 7-bit length");
        let mask = [0x01u8, 0x02, 0x03, 0x04];
        let mut frame = vec![0xC1, 0x80 | compressed.len() as u8]; // fin+rsv1+text; masked+len
        frame.extend_from_slice(&mask);
        frame.extend(compressed.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        conn.write_all(&frame).await.unwrap();

        // The handler echoes it back as a binary frame; the server compresses it (RSV1).
        let mut reply_head = [0u8; 2];
        conn.read_exact(&mut reply_head).await.unwrap();
        assert_eq!(reply_head[0] & 0x40, 0x40, "server set RSV1 (compressed)");
        assert_eq!(reply_head[0] & 0x0F, 0x02, "echoed as a binary frame");
        let len = (reply_head[1] & 0x7F) as usize;
        assert!(len < 126, "the compressed echo fits a 7-bit length");
        let mut body = vec![0u8; len];
        conn.read_exact(&mut body).await.unwrap();
        assert_eq!(
            crate::bridges::ws_codec::inflate_message(&body, None).unwrap(),
            payload,
            "the deflated echo inflates to the original message"
        );

        handle.abort();
    }
}
