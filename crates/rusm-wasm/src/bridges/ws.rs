//! Serving **WebSockets** (Phase 11). A WebSocket is only HTTP for its handshake;
//! after the `Upgrade` it's a raw bidirectional stream — and the handshake + the
//! protocol live entirely on the host, which RUSM controls. So WS never goes
//! through `wasi:http`: **hyper** surfaces the upgrade, **`tokio-tungstenite`** runs
//! the WS protocol (framing, ping/pong, close), and each connection is its own
//! supervised task — a failure drops only that socket, never the listener.
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
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use std::collections::HashMap;

use super::conn::{Resolved, Source};
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
        let Some(mut ws) = upgraded_ws(req).await else {
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
    Ok(switching_protocols(accept))
}

/// The `Sec-WebSocket-Accept` for a request, or `None` if it carries no WS key.
pub(crate) fn ws_accept(req: &hyper::Request<hyper::body::Incoming>) -> Option<String> {
    req.headers()
        .get("sec-websocket-key")
        .and_then(|k| k.to_str().ok())
        .map(|key| derive_accept_key(key.as_bytes()))
}

/// Complete the `Upgrade` and wrap the raw stream as a server-side `WebSocketStream`.
pub(crate) async fn upgraded_ws(
    req: hyper::Request<hyper::body::Incoming>,
) -> Option<WebSocketStream<TokioIo<Upgraded>>> {
    let upgraded = hyper::upgrade::on(req).await.ok()?;
    Some(WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await)
}

pub(crate) fn switching_protocols(accept: String) -> hyper::Response<Empty<Bytes>> {
    hyper::Response::builder()
        .status(101)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-accept", accept)
        .body(Empty::new())
        .unwrap()
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
        }
    }
}

/// How long to wait for a connection's handler to run its `close` and exit on its own
/// (after its writer is killed) before force-reaping it. A cooperating handler — the SDK
/// `ws::serve`, which monitors the writer — exits in microseconds; this only caps a
/// handler that ignores the disconnect.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// Bound on a connection's pending **text** frames (the `ws-send-text` channel) — a slow
/// client back-pressures the handler (it parks on send) instead of buffering without limit.
const WS_TEXT_CAPACITY: usize = 64;

impl WsServer {
    /// Serve WebSockets on `listener` until it closes — one connection per task.
    pub async fn serve(self, listener: TcpListener) {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            stream.set_nodelay(true).ok();
            let server = self.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    let server = server.clone();
                    async move { server.upgrade(req, Some(peer)).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    }

    async fn upgrade(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
        peer: Option<std::net::SocketAddr>,
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
        // Capture the connection context before `upgraded_ws` consumes the request; route
        // params come from the resolver. (Subprotocol negotiation lands with the WS frame work.)
        let connection = super::conn::connection_info(&req, peer, params, None);
        let server = self.clone();
        tokio::spawn(async move {
            if let Some(ws) = upgraded_ws(req).await {
                server
                    .run_connection(ws, prepared, bundle, caps, connection)
                    .await;
            }
        });
        Ok(switching_protocols(accept))
    }

    /// Wire one upgraded connection to a fresh component process (the resolved handler).
    async fn run_connection(
        &self,
        ws: WebSocketStream<TokioIo<Upgraded>>,
        prepared: PreparedComponent,
        bundle: Option<Arc<Vec<u8>>>,
        caps: Capabilities,
        connection: crate::actor::ConnectionInfo,
    ) {
        let (mut sink, mut stream) = ws.split();
        let rt = self.spawner.rt.clone();

        // Writer: a Wasm-free process owning the socket sink. It races two inputs and frames
        // each with the right opcode — **binary** frames arrive via its mailbox (a plain
        // `send` to the writer pid, the unchanged path), **text** frames on a bounded channel
        // the handler feeds via `ws-send-text` (the bound back-pressures a slow client). All
        // IO stays out of the sandboxed component.
        let (text_tx, mut text_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(WS_TEXT_CAPACITY);
        let writer = rt.spawn(move |mut ctx| async move {
            loop {
                tokio::select! {
                    received = ctx.recv() => match received.message() {
                        Some(bytes) => {
                            if sink.send(Message::binary(bytes)).await.is_err() {
                                break;
                            }
                        }
                        None => break, // mailbox closed / a non-message signal
                    },
                    text = text_rx.recv() => match text {
                        Some(payload) => {
                            let text = String::from_utf8_lossy(&payload).into_owned();
                            if sink.send(Message::text(text)).await.is_err() {
                                break;
                            }
                        }
                        None => break, // handler gone (its ws-send-text sender dropped)
                    },
                }
            }
        });

        // The sandboxed handler. For a JS bundle, the runner's first message is the
        // bundle itself; the writer pid then lands as the guest's first receive.
        // (Per-connection handlers aren't named in the platform lifecycle log — the
        // server doesn't carry the serve name; add it to `WsServer` if that's wanted.)
        let component = self
            .spawner
            .spawn_connection(&prepared, caps, connection, Some(text_tx));
        if let Some(bundle) = &bundle {
            rt.send(component.pid(), bundle.as_ref().clone());
        }
        rt.send(component.pid(), writer.pid().raw().to_string().into_bytes());

        // Pump inbound frames into the component's mailbox (one message per frame).
        while let Some(Ok(message)) = stream.next().await {
            if message.is_close() {
                break;
            }
            if message.is_text() || message.is_binary() {
                rt.send(component.pid(), message.into_data().to_vec());
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
            b"ctx /chat/room1 q=x=1",
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
}
