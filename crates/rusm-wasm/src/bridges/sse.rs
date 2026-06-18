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
use rusm_otp::Received;
use tokio::net::TcpListener;

use crate::caps::Capabilities;
use crate::{PreparedComponent, Spawner, WasmRuntime};

/// The response body type — a boxed `StreamBody` fed by the writer process.
type ResBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

/// Keep-alive interval: if no event flows for this long the writer emits a `: ping`
/// comment, so intermediaries don't reap an idle stream and a dead client is noticed
/// (the ping write fails) within the window.
const HEARTBEAT: Duration = Duration::from_secs(15);

/// Body channel depth — bounded, so a slow client back-pressures the writer (it parks on
/// `send`) instead of the body buffering without limit.
const BODY_CAPACITY: usize = 64;

/// Serves each SSE connection with a **WASM component process** — the actor way, mirroring
/// [`super::ws::WsServer`]. The handler emits events to a per-connection **writer** process
/// that owns the chunked response body. Cheap to clone — one task per connection.
#[derive(Clone)]
pub struct SseServer {
    prepared: PreparedComponent,
    /// `Some` when the handler is a **TS/JS bundle** on the shared js-runner: the bundle
    /// is sent as the runner's first message (its protocol), so the writer pid lands as
    /// the guest's *first* `Process.receive()`. `None` = a plain `rusm:runtime` component
    /// that gets the writer pid as message 1 directly.
    bundle: Option<Arc<Vec<u8>>>,
    spawner: Arc<Spawner>,
    caps: Capabilities,
}

impl WasmRuntime {
    /// Build an SSE server that runs `prepared` (a `rusm:runtime` actor component) as the
    /// handler process for each connection, under `caps`.
    pub fn sse_server(&self, prepared: &PreparedComponent, caps: Capabilities) -> SseServer {
        SseServer {
            prepared: prepared.clone(),
            bundle: None,
            spawner: Arc::clone(&self.spawner),
            caps,
        }
    }

    /// Build an SSE server whose per-connection handler is a **TypeScript/JS bundle**
    /// (Bun-built) on the embedded js-runner — the TS twin of [`sse_server`](Self::sse_server).
    /// The guest's first `Process.receive()` is the writer pid, then each pushed event.
    pub fn sse_server_js(&self, bundle: impl Into<Vec<u8>>, caps: Capabilities) -> SseServer {
        SseServer {
            prepared: self.js_runner().clone(),
            bundle: Some(Arc::new(bundle.into())),
            spawner: Arc::clone(&self.spawner),
            caps,
        }
    }
}

impl SseServer {
    /// Serve SSE on `listener` until it closes — one connection per task. Abort the task
    /// driving this to stop.
    pub async fn serve(self, listener: TcpListener) {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                break;
            };
            stream.set_nodelay(true).ok();
            let server = self.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    let server = server.clone();
                    async move { server.handle(req).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
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
    ) -> Result<Response<ResBody>, Infallible> {
        let method = req.method().as_str().to_string();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();
        super::access::log_request(&self.spawner.rt, "sse", &method, &path, 200);

        let rt = self.spawner.rt.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(BODY_CAPACITY);

        // Writer: the one Wasm-free process owning the response body — the sole sender, so
        // its exit (by any arm) drops `tx` and ends the body. Each loop it races three
        // things: the client disconnecting (`closed()` resolves the instant hyper drops
        // the body — immediate detection, not only at the next ping), an event from the
        // handler (frame + emit), and the idle heartbeat. A `Down` means the handler
        // exited (self-close or crash), so end the body. `recv` is cancel-safe, so losing
        // the race never drops a queued event.
        let writer = rt.spawn(move |mut ctx| async move {
            loop {
                tokio::select! {
                    _ = tx.closed() => break, // client disconnected
                    received = ctx.recv() => match received {
                        Received::Message(payload) => {
                            if tx.send(sse_data_frame(&payload)).await.is_err() {
                                break;
                            }
                        }
                        Received::Down { .. } => break, // handler exited — end the body
                        _ => {}                         // ignore streams / other signals
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
        let component = self
            .spawner
            .spawn_component(&self.prepared, self.caps.clone(), None);
        if let Some(bundle) = &self.bundle {
            rt.send(component.pid(), bundle.as_ref().clone());
        }
        rt.send(component.pid(), writer.pid().raw().to_string().into_bytes());
        // Mutual teardown: the writer ends the body when the handler exits (self-close or
        // crash); the handler (monitoring the writer in its SDK loop) ends when the writer
        // dies on client disconnect (its `closed()` arm). Neither side leaks.
        rt.monitor(writer.pid(), component.pid());

        let body = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv()
                .await
                .map(|chunk| (Ok::<_, Infallible>(Frame::data(chunk)), rx))
        });
        Ok(Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(StreamBody::new(body).boxed())
            .expect("sse response builds"))
    }
}

/// Frame a raw event payload as one SSE event — each line its own `data:` field, a blank
/// line terminating the event (so multi-line payloads are valid, not just single-line).
fn sse_data_frame(payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() + 8);
    for line in payload.split(|&b| b == b'\n') {
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.push(b'\n');
    Bytes::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
