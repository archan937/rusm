//! Serving **Server-Sent Events** (Phase 11) — the SSE twin of [`super::ws`], on the
//! same per-connection-process model. SSE is a plain HTTP `GET` that streams a
//! `text/event-stream` body for the life of one connection, so (unlike WS) there is no
//! `Upgrade` — but the lifecycle is identical: one sandboxed **component process** per
//! connection, plus a Wasm-free **writer** process that owns the response body. The
//! writer frames each event the handler emits as a `data:` field, injects keep-alive
//! `: ping` heartbeats on idle, and detects the client disconnect. The handler is pure
//! sandboxed logic (it emits raw payloads and subscribes to its event source — e.g. a
//! process-group tag); a crash drops only that one stream, never the listener.
//!
//! Teardown is mutual, so neither side leaks: the writer ends the body when the handler
//! exits (a `Stream::close` self-stop, or a crash — a monitored `Down`), and the handler
//! ends when the writer dies (the client disconnected — a body send fails). All SSE wire
//! framing + keep-alive lives here (the platform), never in guest code.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Bytes, Frame};
use hyper::Response;
use hyper_util::rt::TokioIo;
use rusm_otp::{Pid, Received};
use tokio::net::TcpListener;

use std::collections::HashMap;

use super::conn::Source;
use super::routed::Resolver;
use crate::caps::Capabilities;
use crate::{PreparedComponent, Spawner, WasmRuntime};

/// The response body type — a boxed `StreamBody` fed by the writer process.
type ResBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

/// Keep-alive interval: if no event flows for this long the writer emits a `: ping`
/// comment, so intermediaries don't reap an idle stream. Prompt disconnect *teardown* does
/// **not** rely on this ping — SSE is one-way (no inbound read channel for the writer to
/// notice a drop on), so it comes from the connection task reaping the per-connection
/// processes when hyper completes `serve_connection` (see [`SseServer::serve`]).
const HEARTBEAT: Duration = Duration::from_secs(15);

/// Grace for a handler to run its own `close` (via its monitor of the writer) after a
/// client disconnect, before the connection task force-reaps it — mirrors the WS bridge,
/// so a cooperating handler tears down gracefully and one that ignores it still can't leak.
const CLOSE_GRACE: Duration = Duration::from_millis(200);

/// Body channel depth — bounded, so a slow client back-pressures the writer (it parks on
/// `send`) instead of the body buffering without limit.
const BODY_CAPACITY: usize = 64;

/// Bound on a connection's pending **rich** SSE events (the `sse-send` channel) — a slow
/// client back-pressures the handler (it parks on send) instead of buffering without limit.
const SSE_OUT_CAPACITY: usize = 64;

/// Serves each SSE connection with a **WASM component process** — the actor way, mirroring
/// [`super::ws::WsServer`]. The handler emits events to a per-connection **writer** process
/// that owns the chunked response body. Cheap to clone — one task per connection.
#[derive(Clone)]
pub struct SseServer {
    source: Source,
    spawner: Arc<Spawner>,
    /// The listener's `[serve.headers]` — merged into every response head (e.g. CORS so a
    /// browser may read this cross-origin feed). Empty by default.
    headers: Arc<Vec<(String, String)>>,
}

impl WasmRuntime {
    /// Build an SSE server that runs `prepared` (a `rusm:runtime` actor component) as the
    /// handler process for **every** connection, under `caps` (an unrouted listener).
    pub fn sse_server(&self, prepared: &PreparedComponent, caps: Capabilities) -> SseServer {
        SseServer {
            source: Source::Single {
                prepared: prepared.clone(),
                bundle: None,
                caps,
            },
            spawner: Arc::clone(&self.spawner),
            headers: Arc::new(Vec::new()),
        }
    }

    /// Build an SSE server whose per-connection handler is a **TypeScript/JS bundle**
    /// (Bun-built) on the embedded js-runner — the TS twin of [`sse_server`](Self::sse_server).
    /// The guest's first `Process.receive()` is the writer pid, then each pushed event.
    pub fn sse_server_js(&self, bundle: impl Into<Vec<u8>>, caps: Capabilities) -> SseServer {
        SseServer {
            source: Source::Single {
                prepared: self.js_runner().clone(),
                bundle: Some(Arc::new(bundle.into())),
                caps,
            },
            spawner: Arc::clone(&self.spawner),
            headers: Arc::new(Vec::new()),
        }
    }

    /// Build a **routed** SSE server: each connection's path resolves (via `resolve`, the
    /// listener's `[serve.routes]`) to a registered handler component, spawned per
    /// connection with the captured path params in its connection context. `caps` gives
    /// each handler component's capability profile, keyed by name. A path that matches no
    /// route is answered `404` (no stream opened).
    pub fn routed_sse_server(
        &self,
        resolve: Resolver,
        caps: HashMap<String, Capabilities>,
    ) -> SseServer {
        SseServer {
            source: Source::Routed {
                resolve,
                caps: Arc::new(caps),
            },
            spawner: Arc::clone(&self.spawner),
            headers: Arc::new(Vec::new()),
        }
    }
}

impl SseServer {
    /// Add the listener's `[serve.headers]` (the response-policy headers merged into each
    /// SSE head — e.g. CORS).
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = Arc::new(headers);
        self
    }

    /// Serve SSE on `listener` until it closes — one connection per task. Abort the task
    /// driving this to stop.
    pub async fn serve(self, listener: TcpListener) {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            stream.set_nodelay(true).ok();
            let server = self.clone();
            tokio::spawn(async move {
                let rt = server.spawner.rt.clone();
                // The per-connection processes (handler + its writer) spawned while serving
                // this connection. SSE is one-way with no inbound read channel, so the
                // handler can't see a client disconnect itself — but hyper surfaces it by
                // *completing* `serve_connection` (even for an idle stream). That is the
                // prompt teardown signal: when the connection ends we reap these processes,
                // releasing the handler's process-group tag at once — so a dropped/refreshed
                // connection never leaks a live process. (The WS bridge force-reaps the same
                // way; the writer's heartbeat is only a keep-alive + last-resort backstop.)
                let procs: Arc<std::sync::Mutex<Vec<(Pid, Pid)>>> =
                    Arc::new(std::sync::Mutex::new(Vec::new()));
                let slot = Arc::clone(&procs);
                let service = hyper::service::service_fn(move |req| {
                    let server = server.clone();
                    let slot = Arc::clone(&slot);
                    async move { server.handle(req, slot, Some(peer)).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
                // `serve_connection` completed = the client disconnected (hyper surfaces it
                // by finishing the future, even for an idle stream). Reap each per-connection
                // (writer, handler): kill the writer — its `tx` drop ends the body and a
                // handler monitoring it runs `close` and exits — give that a brief grace,
                // then force-reap a handler that ignored the disconnect. Mirrors the WS
                // bridge, so a dropped/refreshed connection releases its tag and leaks nothing.
                let pairs = std::mem::take(&mut *procs.lock().unwrap());
                for (writer_pid, component_pid) in pairs {
                    rt.kill(writer_pid);
                    let deadline = tokio::time::Instant::now() + CLOSE_GRACE;
                    while rt.is_alive(component_pid) && tokio::time::Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    if rt.is_alive(component_pid) {
                        rt.kill(component_pid);
                    }
                }
            });
        }
    }

    /// Open one SSE stream: spawn the writer (owns the body) and the handler (gets the
    /// writer pid as message 1), wire their mutual teardown, and return the streaming
    /// `text/event-stream` response. Always `Ok` — the head goes out immediately so the
    /// client's `EventSource` connects, then events stream as the handler emits them.
    async fn handle(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
        procs: Arc<std::sync::Mutex<Vec<(Pid, Pid)>>>,
        peer: Option<std::net::SocketAddr>,
    ) -> Result<Response<ResBody>, Infallible> {
        let method = req.method().as_str().to_string();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();

        // Resolve which handler serves this connection (and capture its route params).
        // An unrouted listener always matches its one handler; a routed listener answers
        // `404` for a path that matches no route — no stream is opened.
        let Some(super::conn::Resolved {
            prepared,
            bundle,
            caps,
            params,
        }) = self.source.resolve(&self.spawner, &method, &path)
        else {
            super::access::log_request(&self.spawner.rt, "sse", &method, &path, 404);
            return Ok(not_found());
        };
        super::access::log_request(&self.spawner.rt, "sse", &method, &path, 200);

        // Capture the connection context for the handler's `connection` op (SSE has no
        // subprotocol); the route params come from the resolver above.
        let connection = super::conn::connection_info(&req, peer, params, None);

        let rt = self.spawner.rt.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(BODY_CAPACITY);

        // Writer: the one Wasm-free process owning the response body — the sole sender, so
        // its exit (by any arm) drops `tx` and ends the body. Each loop it races three
        // things: the client disconnecting (`closed()` resolves the instant hyper drops
        // the body — immediate detection, not only at the next ping), an event from the
        // handler (frame + emit), and the idle heartbeat. A `Down` means the handler
        // exited (self-close or crash), so end the body. `recv` is cancel-safe, so losing
        // the race never drops a queued event.
        let (sse_tx, mut sse_rx) =
            tokio::sync::mpsc::channel::<super::conn::SseEvent>(SSE_OUT_CAPACITY);
        let writer = rt.spawn(move |mut ctx| async move {
            loop {
                tokio::select! {
                    _ = tx.closed() => break, // client disconnected (hyper dropped the body)
                    received = ctx.recv() => match received {
                        // A plain `data:` event (stream.data → send to the writer pid).
                        Received::Message(payload) => {
                            if tx.send(sse_data_frame(&payload)).await.is_err() {
                                break;
                            }
                        }
                        Received::Down { .. } => break, // handler exited — end the body
                        _ => {}                         // ignore streams / other signals
                    },
                    // A rich event (`sse-send`): id:/event:/retry:/data: framing.
                    event = sse_rx.recv() => match event {
                        Some(e) => {
                            if tx.send(sse_event_frame(&e)).await.is_err() {
                                break;
                            }
                        }
                        None => break, // handler gone (its rich-event sender dropped)
                    },
                    _ = tokio::time::sleep(HEARTBEAT) => {
                        if tx.send(Bytes::from_static(b": ping\n\n")).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // The sandboxed handler. For a JS bundle the runner's first message is the bundle;
        // the writer pid then lands as the guest's first receive (the WS handshake).
        // SSE has no inbound/text frames — only `ws-send-text` is WebSocket-specific.
        let component =
            self.spawner
                .spawn_connection(&prepared, caps, connection, None, Some(sse_tx));
        if let Some(bundle) = &bundle {
            rt.send(component.pid(), bundle.as_ref().clone());
        }
        rt.send(component.pid(), writer.pid().raw().to_string().into_bytes());
        // Mutual teardown: the writer ends the body when the handler exits (self-close or
        // crash); the handler (monitoring the writer in its SDK loop) ends when the writer
        // dies on client disconnect (its `closed()` arm). Neither side leaks.
        rt.monitor(writer.pid(), component.pid());
        // Hand this connection's (writer, handler) to the connection task, which reaps them
        // when `serve_connection` completes (the client disconnect) — SSE has no inbound
        // read channel, so that completion is the only prompt disconnect signal.
        procs.lock().unwrap().push((writer.pid(), component.pid()));

        let body = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv()
                .await
                .map(|chunk| (Ok::<_, Infallible>(Frame::data(chunk)), rx))
        });
        let mut response = Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(StreamBody::new(body).boxed())
            .expect("sse response builds");
        // Merge the listener's declared response policy (e.g. CORS) over the SSE defaults.
        super::access::apply_extra_headers(&mut response, &self.headers);
        Ok(response)
    }
}

/// A `404` for a routed SSE listener whose path matched no route — a plain buffered body,
/// not an event stream (no handler was spawned).
fn not_found() -> Response<ResBody> {
    Response::builder()
        .status(404)
        .header("content-type", "text/plain; charset=utf-8")
        .body(http_body_util::Full::new(Bytes::from_static(b"not found")).boxed())
        .expect("404 response builds")
}

/// Frame a raw event payload as one SSE event — each line its own `data:` field, a blank
/// line terminating the event (so multi-line payloads are valid, not just single-line).
fn sse_data_frame(payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() + 8);
    push_data_lines(&mut out, payload);
    out.push(b'\n');
    Bytes::from(out)
}

/// Frame a **rich** SSE event: optional `id:`/`event:`/`retry:` lines, then the `data:`
/// payload (multi-line aware), then the terminating blank line. `id`/`event` are
/// single-line by spec, so any embedded newline/CR is dropped (it would corrupt framing).
fn sse_event_frame(event: &super::conn::SseEvent) -> Bytes {
    let mut out = Vec::with_capacity(event.data.len() + 32);
    if let Some(id) = &event.id {
        push_single_line_field(&mut out, b"id: ", id);
    }
    if let Some(name) = &event.event {
        push_single_line_field(&mut out, b"event: ", name);
    }
    if let Some(retry) = event.retry {
        out.extend_from_slice(format!("retry: {retry}\n").as_bytes());
    }
    push_data_lines(&mut out, &event.data);
    out.push(b'\n');
    Bytes::from(out)
}

/// Append `data: <line>\n` for each `\n`-separated line of `payload`.
fn push_data_lines(out: &mut Vec<u8>, payload: &[u8]) {
    for line in payload.split(|&b| b == b'\n') {
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(line);
        out.push(b'\n');
    }
}

/// Append a single-line SSE field (`id: `/`event: `), dropping any CR/LF in the value so a
/// stray newline can't inject extra fields or terminate the event early.
fn push_single_line_field(out: &mut Vec<u8>, name: &[u8], value: &str) {
    out.extend_from_slice(name);
    out.extend(value.bytes().filter(|&b| b != b'\n' && b != b'\r'));
    out.push(b'\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridges::routed::Routed;
    use crate::{CapabilityProfile, WasmRuntime};
    use rusm_otp::Runtime;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A process registered as `"collector"` that forwards every message to the channel,
    /// so a test can observe a guest's lifecycle reports.
    fn collector(rt: &Runtime) -> tokio::sync::mpsc::UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let proc = rt.spawn(move |mut ctx| async move {
            loop {
                if let Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(b);
                }
            }
        });
        rt.register("collector", proc.pid());
        rx
    }

    async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a lifecycle event within 5s")
            .expect("collector channel stays open")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_sse_handler_runs_open_pushes_events_and_closes_on_disconnect() {
        // A full-lifecycle SSE handler (rs-sse-feed): open subscribes to the "feed" tag
        // and reports "open"; a published event (whereis_tag + send) arrives as `message`
        // and is emitted as a `data:` frame; close reports "close" on disconnect. Proves
        // the per-connection SSE model end-to-end — push-via-tags, host framing + the
        // text/event-stream + no-cache head, and monitor-the-writer teardown.
        const SSE_FEED: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_feed.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_FEED).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // Connect and read the streaming response incrementally.
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        // `open` fires once connected (and has subscribed to the tag by the time it reports).
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        // Publish to the "feed" tag — the platform pub/sub primitive; it lands in the
        // handler's mailbox as `message`, which emits it.
        for pid in rt.whereis_tag("feed") {
            rt.send(pid, b"hello".to_vec());
        }

        // Read until the framed event lands; assert the SSE head too.
        let mut seen = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf))
                .await
                .expect("the pushed event arrives in time")
                .expect("socket read ok");
            assert!(n > 0, "stream produced data");
            seen.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&seen).contains("data: hello") {
                break;
            }
        }
        let lower = String::from_utf8_lossy(&seen).to_lowercase();
        assert!(lower.contains("text/event-stream"), "SSE head: {lower}");
        assert!(
            lower.contains("cache-control: no-cache"),
            "host sets no-cache: {lower}"
        );

        // Disconnect: `close` must fire (the writer dies → the handler, monitoring it,
        // runs close and exits — tag auto-released).
        drop(conn);
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on disconnect"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_sse_handler_reads_its_connection_context() {
        // The `connection` op end-to-end: a per-connection SSE handler reads its request
        // method, path, query, and a header (via `Stream::info`) and reports them. Proves
        // the additive connection context reaches a real guest through host store state —
        // the linchpin for path-parameterised SSE.
        const SSE_CONN: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_CONN).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(
            b"GET /events/plan7?x=1 HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();

        // No routing on this listener yet, so `plan` is unset (`-`); path, query, and the
        // host header are delivered verbatim.
        assert_eq!(
            next_event(&mut rx).await,
            b"GET /events/plan7 q=x=1 plan=- host=rusm",
            "the handler reads method/path/query/header from its connection context"
        );

        drop(conn);
        handle.abort();
    }

    /// Bridge a manifest per-connection [`rusm_node::RouteTable`] into the engine's
    /// routing-agnostic [`Resolver`] — exactly what `rusm-cli` does for a routed listener.
    fn resolver(table: rusm_node::RouteTable) -> Resolver {
        Arc::new(
            move |method: &str, path: &str| match table.resolve(method, path) {
                rusm_node::Resolution::Found {
                    component,
                    action,
                    params,
                } => Routed::Found {
                    component,
                    action,
                    params,
                },
                rusm_node::Resolution::MethodNotAllowed => Routed::MethodNotAllowed,
                rusm_node::Resolution::NotFound => Routed::NotFound,
            },
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_routed_sse_listener_captures_path_params_for_the_handler() {
        // A `[serve.routes]` SSE listener resolves the connection's path to a registered
        // handler component and captures path params into its connection context — proving
        // path-parameterised SSE end-to-end (the linchpin for multi-entity feeds).
        const SSE_CONN: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_CONN).unwrap(), "run")
            .unwrap();
        wr.register_component("events", prepared);

        let table = rusm_node::RouteTable::from_handler_map(&std::collections::HashMap::from([(
            "GET /events/:plan/:collection/:id".to_string(),
            "events".to_string(),
        )]))
        .unwrap();
        let caps = std::collections::HashMap::from([(
            "events".to_string(),
            CapabilityProfile::Trusted.capabilities(),
        )]);
        let server = wr.routed_sse_server(resolver(table), caps);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(
            b"GET /events/p7/pages/42 HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();

        // The handler reports the params captured from the route pattern.
        assert_eq!(
            next_event(&mut rx).await,
            b"GET /events/p7/pages/42 q= plan=p7 host=rusm",
            "the routed listener captured :plan and delivered it to the handler"
        );

        drop(conn);
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_routed_sse_listener_404s_an_unmatched_path() {
        // A path matching no route opens no stream — a plain 404, no handler spawned.
        const SSE_CONN: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_CONN).unwrap(), "run")
            .unwrap();
        wr.register_component("events", prepared);
        let table = rusm_node::RouteTable::from_handler_map(&std::collections::HashMap::from([(
            "GET /events/:plan".to_string(),
            "events".to_string(),
        )]))
        .unwrap();
        let caps = std::collections::HashMap::from([(
            "events".to_string(),
            CapabilityProfile::Trusted.capabilities(),
        )]);
        let server = wr.routed_sse_server(resolver(table), caps);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /nope HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), conn.read_to_end(&mut buf))
            .await
            .expect("response in time")
            .unwrap();
        assert!(
            String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 404"),
            "unmatched path → 404, got: {}",
            String::from_utf8_lossy(&buf).lines().next().unwrap_or("")
        );
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_sse_handler_reads_its_routed_connection_context() {
        // The Go SDK twin of the routed-context test: a routed SSE listener captures :plan
        // and a Go `web.Sse` handler reads method/path/query/param/header via `Stream.Info()`
        // — proving RS/Go parity for the connection context.
        const GO_SSE_CONN: &[u8] = include_bytes!("../../tests/fixtures/go_sse_conn.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);
        let prepared = wr
            .prepare_component(&wr.compile_component(GO_SSE_CONN).unwrap(), "run")
            .unwrap();
        wr.register_component("events", prepared);
        let table = rusm_node::RouteTable::from_handler_map(&std::collections::HashMap::from([(
            "GET /events/:plan/:collection/:id".to_string(),
            "events".to_string(),
        )]))
        .unwrap();
        let caps = std::collections::HashMap::from([(
            "events".to_string(),
            CapabilityProfile::Trusted.capabilities(),
        )]);
        let server = wr.routed_sse_server(resolver(table), caps);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(
            b"GET /events/p7/pages/42 HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            b"GET /events/p7/pages/42 q= plan=p7 host=rusm",
            "the Go handler read its routed connection context (param + header)"
        );
        drop(conn);
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ts_sse_handler_reads_its_connection_context() {
        // The TS SDK twin: a TS `sse()` handler reads method/path/query/header via
        // `stream.info` — through the js-runner's `__connection` primitive + bridge — so
        // RS/Go/TS all reach connection-context parity. Unrouted, so `plan` is unset.
        const TS_SSE_CONN: &[u8] = include_bytes!("../../tests/fixtures/ts_sse_conn.js");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);
        let server = wr.sse_server_js(
            TS_SSE_CONN.to_vec(),
            CapabilityProfile::Trusted.capabilities(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /events/p7?x=1 HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            b"GET /events/p7 q=x=1 plan=- host=rusm",
            "the TS handler read its connection context via stream.info"
        );
        drop(conn);
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_idle_sse_client_disconnect_is_reaped_with_no_leak() {
        // An *idle* SSE client disconnect (no events ever published): the writer can't see
        // it (SSE has no inbound read channel), so the connection task reaps the handler
        // when hyper completes `serve_connection` — `close` fires and the "feed" tag is
        // released, so a dropped/refreshed connection leaks no live process. Regression for
        // feeds lingering and piling up one-per-refresh.
        const SSE_FEED: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_feed.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_FEED).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // Connect, let `open` fire (it subscribes to "feed"), then go idle and disconnect —
        // no events are ever published, so only the connection task (hyper completing
        // `serve_connection`) can surface the drop, never the handler itself.
        {
            let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
            conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");
            assert_eq!(rt.whereis_tag("feed").len(), 1, "one live feed subscriber");
        } // `conn` dropped here → client disconnect, stream idle

        // The handler must run `close` and exit promptly (the connection task reaps it)…
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "idle disconnect tears the handler down"
        );
        // …and its "feed" tag is released, so a dropped connection leaves no live process.
        let start = std::time::Instant::now();
        while !rt.whereis_tag("feed").is_empty() {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "feed tag released promptly after idle disconnect (no leak)"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_sse_handler_can_stop_itself() {
        // Server-initiated close: the handler calls `Stream::close` (on a "close" event),
        // which ends the stream and its process — `close` fires and the client sees EOF.
        const SSE_FEED: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_feed.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_FEED).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        // Tell the handler to stop itself.
        for pid in rt.whereis_tag("feed") {
            rt.send(pid, b"close".to_vec());
        }
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on self-stop"
        );

        // The body ends → the client read drains to EOF (no hang).
        let mut tail = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), conn.read_to_end(&mut tail))
            .await
            .expect("the stream ends after self-close")
            .expect("socket read ok");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_sse_handler_pushes_events_and_closes_on_disconnect() {
        // An inline JS bundle in the `sse({…})` shape rusm-ts emits — open subscribes to
        // the "feed" tag and reports "open", message emits each pushed event, close
        // reports "close". Proves the actor-aware TS SSE path: the per-connection js-runner
        // handler subscribes + receives published events (push-via-tags) and fires close on
        // disconnect, resembling the Rust handler.
        const BUNDLE: &str = r#"
            const report = (e) => { const c = Process.whereis("collector"); if (c !== null) Process.send(c, e); };
            module.exports.default = {
              sse: {
                open:    (conn)     => { Process.registerTag("feed"); report("open"); },
                message: (conn, ev) => Process.send(conn, ev), // emit the pushed event
                close:   (conn)     => report("close"),
              },
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let server = wr.sse_server_js(BUNDLE, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        for pid in rt.whereis_tag("feed") {
            rt.send(pid, b"hello".to_vec());
        }
        let mut seen = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf))
                .await
                .expect("the pushed event arrives in time")
                .expect("socket read ok");
            assert!(n > 0, "stream produced data");
            seen.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&seen).contains("data: hello") {
                break;
            }
        }
        assert!(
            String::from_utf8_lossy(&seen)
                .to_lowercase()
                .contains("text/event-stream"),
            "SSE head present"
        );

        drop(conn);
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on disconnect"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_sse_handler_pushes_events_and_closes_on_disconnect() {
        // A Go (TinyGo) full-lifecycle SSE handler (go-sse-feed): open subscribes to the
        // "feed" tag and reports "open", message emits each pushed event, close reports
        // "close". Proves `web.Sse{Open,Message,Close}` does push-via-tags + close on
        // disconnect, resembling the Rust and TS handlers.
        const GO_SSE_FEED: &[u8] = include_bytes!("../../tests/fixtures/go_sse_feed.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(GO_SSE_FEED).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        for pid in rt.whereis_tag("feed") {
            rt.send(pid, b"hello".to_vec());
        }
        let mut seen = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf))
                .await
                .expect("the pushed event arrives in time")
                .expect("socket read ok");
            assert!(n > 0, "stream produced data");
            seen.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&seen).contains("data: hello") {
                break;
            }
        }

        drop(conn);
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on disconnect"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_sse_listener_applies_serve_headers() {
        // `[serve.headers]` (e.g. CORS) merges into the SSE head, over the transport
        // defaults — so a browser can read a cross-origin feed.
        const SSE_FEED: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_feed.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_FEED).unwrap(), "run")
            .unwrap();
        let server = wr
            .sse_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_headers(vec![("access-control-allow-origin".into(), "*".into())]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf))
                .await
                .expect("the head arrives")
                .expect("socket read ok");
            seen.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&seen).contains("\r\n\r\n") {
                break; // end of response headers
            }
        }
        let head = String::from_utf8_lossy(&seen).to_lowercase();
        assert!(
            head.contains("access-control-allow-origin: *"),
            "serve.headers applied: {head}"
        );
        assert!(
            head.contains("text/event-stream"),
            "transport default kept: {head}"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_sse_handler_can_stop_itself() {
        // Inline bundle in the `sse({…})` shape with the helper's `done` flag: on a "close"
        // event the handler flips `done` (the `stream.close()` equivalent), so the driver
        // stops, `close` fires, and the client sees EOF — the TS twin of the Rust self-stop.
        const BUNDLE: &str = r#"
            const report = (e) => { const c = Process.whereis("collector"); if (c !== null) Process.send(c, e); };
            let done = false;
            module.exports.default = {
              sse: {
                open:    (conn)     => { Process.registerTag("feed"); report("open"); },
                message: (conn, ev) => {
                  if (new TextDecoder().decode(ev) === "close") { done = true; }
                  else { Process.send(conn, ev); }
                },
                close:   (conn)     => report("close"),
                done:    ()         => done,
              },
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let server = wr.sse_server_js(BUNDLE, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        for pid in rt.whereis_tag("feed") {
            rt.send(pid, b"close".to_vec());
        }
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on self-stop"
        );
        let mut tail = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), conn.read_to_end(&mut tail))
            .await
            .expect("the stream ends after self-close")
            .expect("socket read ok");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_sse_handler_can_stop_itself() {
        // The Go twin of the self-stop test: web.Sse calls Stream.Close() on a "close"
        // event, ending the stream and its process — close fires and the client sees EOF.
        const GO_SSE_FEED: &[u8] = include_bytes!("../../tests/fixtures/go_sse_feed.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut rx = collector(&rt);

        let prepared = wr
            .prepare_component(&wr.compile_component(GO_SSE_FEED).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /feed HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(next_event(&mut rx).await, b"open", "open fires on connect");

        for pid in rt.whereis_tag("feed") {
            rt.send(pid, b"close".to_vec());
        }
        assert_eq!(
            next_event(&mut rx).await,
            b"close",
            "close fires on self-stop"
        );
        let mut tail = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), conn.read_to_end(&mut tail))
            .await
            .expect("the stream ends after self-close")
            .expect("socket read ok");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_sse_handler_emits_a_rich_event() {
        // A handler using `Stream::emit` must frame `id:`/`event:`/`data:` (the basis for
        // Last-Event-ID resumption) — proving the additive `sse-send` op.
        const SSE_EVENT: &[u8] = include_bytes!("../../tests/fixtures/rs_sse_event.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(SSE_EVENT).unwrap(), "run")
            .unwrap();
        let server = wr.sse_server(&prepared, CapabilityProfile::Trusted.capabilities());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /events HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        // The handler emits then self-closes, so the body ends — read it all.
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), conn.read_to_end(&mut buf))
            .await
            .expect("the stream ends after the handler closes")
            .expect("socket read ok");
        let body = String::from_utf8_lossy(&buf);
        assert!(body.contains("id: 42"), "id framed: {body}");
        assert!(body.contains("event: greeting"), "event framed: {body}");
        assert!(body.contains("data: hello"), "data framed: {body}");

        handle.abort();
    }
}
