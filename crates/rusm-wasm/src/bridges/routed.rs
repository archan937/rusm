//! Per-request HTTP serving with the actor world — the unified serving model. For
//! each request the host resolves the route, spawns the matched handler component
//! **fresh** (process-per-request, so head-of-line blocking is impossible by
//! construction), dispatches the matched *action* over the `"fetch"` actor wire, and
//! turns the reply into the HTTP response. The handler component is just a module of
//! `fn action(Request, Params) -> Response` (see `#[rusm_rs::handlers]`); the routing
//! and the wire are entirely platform code.
//!
//! Contrast with the neighbours: [`super::http`] is the handler-less `wasi:http` path
//! (no actor world, no routing), and [`super::resident`] is one long-lived stateful
//! instance. This is the shape RUSM standardizes serving on — stateless, isolated,
//! routable — reusing resident's reply machinery ([`GatewayReply`], [`spawn_responder`],
//! the response builders).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Bytes, Frame};
use hyper::{Response, StatusCode};
use rusm_otp::{ProcessHandle, Received, Runtime, StreamHandle};
use serde::Deserialize;
use wasmtime_wasi_http::io::TokioIo;

use crate::caps::Capabilities;
use crate::{Spawner, WasmRuntime};

/// The response body type the gateway produces — a boxed body, so a buffered (`Full`)
/// and a streamed/SSE (`StreamBody`) response share one type.
type ResBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

/// The host's per-request routing decision — the engine-local mirror of the manifest
/// route table, so `rusm-wasm` needn't depend on the config crate. `rusm-cli` bridges
/// `rusm_node::RouteTable::resolve` into a [`Resolver`] that yields this.
pub enum Routed {
    /// A route matched: dispatch `action` on `component` with these captured path params.
    Found {
        component: String,
        action: String,
        params: Vec<(String, String)>,
    },
    /// The path matched a route, but not for this method (HTTP 405).
    MethodNotAllowed,
    /// No route matched the path (HTTP 404).
    NotFound,
}

/// Resolves `(method, path)` to a [`Routed`] decision — supplied by the orchestrator
/// (it owns the manifest `[routes]` table; the engine stays routing-agnostic).
pub type Resolver = Arc<dyn Fn(&str, &str) -> Routed + Send + Sync>;

/// A per-request routed HTTP server: resolve the route, spawn the matched handler
/// fresh, dispatch the action, reply. Cheap to clone — one task per connection.
#[derive(Clone)]
pub struct RoutedHttpServer {
    spawner: Arc<Spawner>,
    resolve: Resolver,
    /// The capability profile to spawn each handler component under, by name.
    caps: Arc<HashMap<String, Capabilities>>,
    /// Listener declared `protocol = "sse"`: every 2xx reply must be a
    /// `text/event-stream`, or the host fails loud (a manifest/handler contract check).
    require_sse: bool,
}

impl WasmRuntime {
    /// Build a per-request routed HTTP server. `resolve` maps `(method, path)` to a
    /// [`Routed`] decision (the orchestrator bridges in the manifest `[routes]` table);
    /// `caps` gives the capability profile to spawn each handler component under, keyed
    /// by component name. The handler components must already be registered
    /// ([`register_component`](Self::register_component) /
    /// [`register_js_component`](Self::register_js_component)).
    pub fn routed_http_server(
        &self,
        resolve: Resolver,
        caps: HashMap<String, Capabilities>,
    ) -> RoutedHttpServer {
        RoutedHttpServer {
            spawner: self.spawner.clone(),
            resolve,
            caps: Arc::new(caps),
            require_sse: false,
        }
    }
}

impl RoutedHttpServer {
    /// Mark this as an SSE listener (`protocol = "sse"`): every successful reply must be a
    /// `text/event-stream`, or the host serves 500 (a contract check — see
    /// [`super::access::violates_sse_contract`]).
    pub fn require_event_stream(mut self) -> Self {
        self.require_sse = true;
        self
    }

    /// Serve HTTP/1.1 on `listener` until it closes — one task per connection. Abort
    /// the task driving this to stop.
    pub async fn serve(self, listener: tokio::net::TcpListener) {
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

    /// Serve one request and log it: `rusm http|sse <method> <path> → <status>` (gated by
    /// `[log] level`). SSE is told apart from plain HTTP by the response content-type,
    /// since both ride this path.
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
        let mut response = match self.dispatch(req).await {
            Ok(response) => response,
            Err(never) => match never {}, // dispatch is infallible
        };
        // SSE transport correctness, uniform across all guest languages: fail loud on a
        // `protocol = "sse"` listener that produced a non-event-stream 2xx, else ensure an
        // event-stream is never cached.
        if super::access::violates_sse_contract(
            self.require_sse,
            response.status().as_u16(),
            response.headers(),
        ) {
            response = error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "protocol=\"sse\" requires a text/event-stream response",
            );
        } else {
            super::access::ensure_no_cache(&mut response);
        }
        let proto = if super::access::is_event_stream(response.headers()) {
            "sse"
        } else {
            "http"
        };
        super::access::log_request(
            &self.spawner.rt,
            proto,
            &method,
            &path,
            response.status().as_u16(),
        );
        Ok(response)
    }

    /// Resolve one request, spawn the matched handler fresh, dispatch the action over
    /// the `"fetch"` wire, and turn the reply into the response. Always `Ok` — every
    /// failure becomes a status code.
    async fn dispatch(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<Response<ResBody>, Infallible> {
        let (parts, body) = req.into_parts();
        let target = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        let (component, action, params) = match (self.resolve)(parts.method.as_str(), target) {
            Routed::Found {
                component,
                action,
                params,
            } => (component, action, params),
            Routed::MethodNotAllowed => {
                return Ok(error_response(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "method not allowed",
                ))
            }
            Routed::NotFound => return Ok(error_response(StatusCode::NOT_FOUND, "not found")),
        };

        // The matched component must be registered and have a capability profile; a
        // mismatch is a manifest error, so 500 (the orchestrator validates up front).
        let Some(entry) = self.spawner.lookup(&component) else {
            return Ok(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("route names unregistered component `{component}`"),
            ));
        };
        let Some(caps) = self.caps.get(&component).cloned() else {
            return Ok(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("no capability profile for `{component}`"),
            ));
        };

        let method = parts.method.as_str().to_string();
        let url = target.to_string();
        let headers: Vec<(String, String)> = parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = match body.collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(_) => {
                return Ok(error_response(
                    StatusCode::BAD_REQUEST,
                    "could not read body",
                ))
            }
        };
        let request = rusm_wire::Request {
            method,
            url,
            headers,
            body,
        };

        // Process-per-request: a fresh instance handles this one request, then exits.
        let child = self
            .spawner
            .spawn_component(&entry.prepared, caps, Some(&component));
        let child_pid = child.pid();
        // A TS handler carries its bundle as message 1 (the js-runner's protocol).
        if let Some(bundle) = &entry.bundle {
            self.spawner.rt.send(child_pid, (**bundle).clone());
        }
        // An ephemeral responder owns the oneshot and turns the reply into the response;
        // the handler sends exactly one reply to it, so no ref-matching is needed here.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let responder = spawn_responder(&self.spawner.rt, tx);
        let envelope = serde_json::json!({
            "op": "fetch",
            "ref": 0u64,
            "from": responder.pid().raw().to_string(),
            "action": action,
            "params": params,
            "request": request,
        });
        self.spawner.rt.send(
            child_pid,
            serde_json::to_vec(&envelope).expect("envelope serializes"),
        );

        Ok(match rx.await {
            Ok(GatewayReply::Buffered(resp)) => build_response(resp),
            Ok(GatewayReply::Streaming {
                status,
                headers,
                handle,
            }) => build_streaming_response(status, headers, handle),
            Ok(GatewayReply::Err(message)) => {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
            }
            Err(_) => error_response(StatusCode::BAD_GATEWAY, "handler did not reply"),
        })
    }
}

// ── The reply machinery: handler reply → HTTP response ────────────────────────
//
// An ephemeral Wasm-free **responder** process owns a `oneshot` and turns the
// handler's reply into the HTTP response — a buffered body, or a streamed/SSE body
// that drains the guest's byte stream directly into the chunked HTTP body.

/// The handler's reply, as the responder hands it to the HTTP task.
enum GatewayReply {
    /// A complete buffered response.
    Buffered(rusm_wire::Response),
    /// A streaming response (SSE): the head, plus the guest's byte stream which the
    /// HTTP task drains directly into a chunked body — no intermediate channel.
    Streaming {
        status: u16,
        headers: Vec<(String, String)>,
        handle: StreamHandle,
    },
    /// The handler errored.
    Err(String),
}

/// A Wasm-free process that waits for the handler's reply and hands it to the HTTP
/// task. For a streaming reply the guest sends the head, then opens a byte stream to
/// us (`Received::Stream`); we forward the stream **handle** itself (already a
/// back-pressured Tokio channel), so the HTTP body reads the guest directly.
fn spawn_responder(rt: &Runtime, tx: tokio::sync::oneshot::Sender<GatewayReply>) -> ProcessHandle {
    rt.spawn(move |mut ctx| async move {
        // The head reply (a plain message).
        let head = loop {
            match ctx.recv().await {
                Received::Message(bytes) => break bytes,
                _ => continue,
            }
        };
        let resp = match parse_reply(&head) {
            Ok(resp) => resp,
            Err(err) => {
                let _ = tx.send(GatewayReply::Err(err));
                return;
            }
        };
        if !resp.stream {
            let _ = tx.send(GatewayReply::Buffered(resp));
            return;
        }
        // Streaming: the guest opens a byte stream to us next; hand its read end to
        // the HTTP task and exit (no forwarding loop).
        loop {
            if let Received::Stream(handle) = ctx.recv().await {
                let _ = tx.send(GatewayReply::Streaming {
                    status: resp.status,
                    headers: resp.headers,
                    handle,
                });
                return;
            }
        }
    })
}

/// A reply envelope `{ref, ok|err}` as produced by the guest's `reply_ok`/`reply_err`;
/// `ok` is the shared [`rusm_wire::Response`].
#[derive(Deserialize)]
struct WireReply {
    #[serde(default)]
    ok: Option<rusm_wire::Response>,
    #[serde(default)]
    err: Option<String>,
}

fn parse_reply(bytes: &[u8]) -> Result<rusm_wire::Response, String> {
    let reply: WireReply = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if let Some(err) = reply.err {
        return Err(err);
    }
    reply.ok.ok_or_else(|| "reply missing `ok`".to_string())
}

fn response_builder(status: u16, headers: Vec<(String, String)>) -> hyper::http::response::Builder {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
}

fn build_response(resp: rusm_wire::Response) -> Response<ResBody> {
    response_builder(resp.status, resp.headers)
        .body(Full::new(Bytes::from(resp.body)).boxed())
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid response"))
}

/// Build a chunked, streamed response by draining the guest's byte stream — each
/// chunk becomes a body frame as it's produced (true SSE), with the stream's own
/// Tokio back-pressure carrying through.
fn build_streaming_response(
    status: u16,
    headers: Vec<(String, String)>,
    handle: StreamHandle,
) -> Response<ResBody> {
    let body = futures_util::stream::unfold(handle, |mut handle| async move {
        handle
            .read()
            .await
            .map(|chunk| (Ok::<_, Infallible>(Frame::data(Bytes::from(chunk))), handle))
    });
    response_builder(status, headers)
        .body(StreamBody::new(body).boxed())
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid response"))
}

fn error_response(status: StatusCode, message: &str) -> Response<ResBody> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(message.to_owned())).boxed())
        .expect("error response builds")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapabilityProfile;
    use rusm_node::{Resolution, RouteTable};
    use rusm_otp::Runtime;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // A `#[rusm_rs::handlers] pub mod demo` with `fn hello(_, params)` (→ "hi <name>")
    // and `fn echo(req, _)` (→ the request body). See `tests/fixtures/handlers-demo`.
    const HANDLERS: &[u8] = include_bytes!("../../tests/fixtures/rs_handlers_demo.wasm");

    /// The exact bridge `rusm-cli` builds: the manifest [`RouteTable`] → a [`Resolver`].
    fn resolver(table: RouteTable) -> Resolver {
        Arc::new(
            move |method: &str, path: &str| match table.resolve(method, path) {
                Resolution::Found {
                    component,
                    action,
                    params,
                } => Routed::Found {
                    component,
                    action,
                    params,
                },
                Resolution::MethodNotAllowed => Routed::MethodNotAllowed,
                Resolution::NotFound => Routed::NotFound,
            },
        )
    }

    async fn serve_on(server: RoutedHttpServer) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(server.serve(listener));
        addr
    }

    /// One raw HTTP/1.1 request (Connection: close) → the full response text.
    async fn request(addr: SocketAddr, method: &str, path: &str, body: &str) -> String {
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatches_each_request_by_route_to_a_freshly_spawned_handler() {
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(HANDLERS).unwrap(), "run")
            .unwrap();
        wr.register_component("demo", prepared);

        let table = RouteTable::from_map(&HashMap::from([
            ("GET /hello/:name".to_string(), "demo#hello".to_string()),
            ("POST /echo".to_string(), "demo#echo".to_string()),
            ("GET /ticks".to_string(), "demo#ticks".to_string()),
        ]))
        .unwrap();
        let caps = HashMap::from([(
            "demo".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        let addr = serve_on(wr.routed_http_server(resolver(table), caps)).await;

        // A matched route dispatches the named action with the captured path param.
        let hello = request(addr, "GET", "/hello/alice", "").await;
        assert!(hello.starts_with("HTTP/1.1 200"), "got: {hello}");
        assert!(hello.contains("hi alice"), "param dispatched: {hello}");

        // A different action on the same component, carrying the request body.
        let echo = request(addr, "POST", "/echo", "ping").await;
        assert!(echo.starts_with("HTTP/1.1 200"), "got: {echo}");
        assert!(echo.trim_end().ends_with("ping"), "echo body: {echo}");

        // Each request is a fresh instance: the second `/hello` is independent of the
        // first (no shared state to leak), and still resolves correctly.
        let again = request(addr, "GET", "/hello/bob", "").await;
        assert!(
            again.contains("hi bob"),
            "fresh instance per request: {again}"
        );

        // A 3-arg (`Sse`) action streams a chunked text/event-stream body.
        let ticks = request(addr, "GET", "/ticks", "").await;
        let lower = ticks.to_lowercase();
        assert!(ticks.starts_with("HTTP/1.1 200"), "got: {ticks}");
        assert!(
            lower.contains("text/event-stream") && lower.contains("transfer-encoding: chunked"),
            "streamed SSE body: {ticks}"
        );
        for n in 0..3 {
            assert!(
                ticks.contains(&format!("data: tick {n}")),
                "event {n}: {ticks}"
            );
        }

        // Unmatched path → 404; matched path, wrong method → 405.
        assert!(
            request(addr, "GET", "/nope", "")
                .await
                .starts_with("HTTP/1.1 404"),
            "unmatched path is 404"
        );
        assert!(
            request(addr, "DELETE", "/echo", "")
                .await
                .starts_with("HTTP/1.1 405"),
            "matched path + wrong method is 405"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_sse_listener_adds_no_cache_and_rejects_a_non_event_stream() {
        // A `protocol = "sse"` listener (`.require_event_stream()`): the host enforces SSE
        // transport correctness on every reply — add `Cache-Control: no-cache` to the
        // event-stream, and fail loud (500) on a route that returns a non-event-stream.
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(HANDLERS).unwrap(), "run")
            .unwrap();
        wr.register_component("demo", prepared);
        let table = RouteTable::from_map(&HashMap::from([
            ("GET /ticks".to_string(), "demo#ticks".to_string()),
            ("GET /hello/:name".to_string(), "demo#hello".to_string()),
        ]))
        .unwrap();
        let caps = HashMap::from([(
            "demo".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        let server = wr
            .routed_http_server(resolver(table), caps)
            .require_event_stream();
        let addr = serve_on(server).await;

        // A real SSE action: 200, text/event-stream, and the host-added `no-cache`.
        let ticks = request(addr, "GET", "/ticks", "").await.to_lowercase();
        assert!(ticks.starts_with("http/1.1 200"), "got: {ticks}");
        assert!(ticks.contains("text/event-stream"), "is SSE: {ticks}");
        assert!(
            ticks.contains("cache-control: no-cache"),
            "the host adds no-cache to an SSE stream: {ticks}"
        );

        // A buffered (non-event-stream) action on an SSE listener breaks the contract → 500.
        let bad = request(addr, "GET", "/hello/alice", "").await;
        assert!(
            bad.starts_with("HTTP/1.1 500"),
            "a non-event-stream reply on an sse listener fails loud: {bad}"
        );
        assert!(
            bad.contains("text/event-stream"),
            "the 500 explains the SSE contract: {bad}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatches_requests_to_a_go_handler_component() {
        // The same per-request serving path, but the handler is a Go component built on
        // the rusm-go `web` package (web.Handlers / Handle). Proves a Go handler speaks
        // the host's fetch/reply wire end-to-end over real HTTP — buffered responses, a
        // captured path param, and a request body. (SSE is a per-connection handler now;
        // see the Go SSE test in `bridges::sse`.)
        const GO_HANDLERS: &[u8] = include_bytes!("../../tests/fixtures/go_handlers.wasm");
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(GO_HANDLERS).unwrap(), "run")
            .unwrap();
        wr.register_component("demo", prepared);

        let table = RouteTable::from_map(&HashMap::from([
            ("GET /hello/:name".to_string(), "demo#hello".to_string()),
            ("POST /echo".to_string(), "demo#echo".to_string()),
        ]))
        .unwrap();
        let caps = HashMap::from([(
            "demo".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        let addr = serve_on(wr.routed_http_server(resolver(table), caps)).await;

        let hello = request(addr, "GET", "/hello/alice", "").await;
        assert!(
            hello.starts_with("HTTP/1.1 200") && hello.contains("hi alice"),
            "param dispatched to the Go handler: {hello}"
        );
        let echo = request(addr, "POST", "/echo", "ping").await;
        assert!(
            echo.starts_with("HTTP/1.1 200") && echo.trim_end().ends_with("ping"),
            "Go handler echoed the request body: {echo}"
        );
        assert!(
            request(addr, "GET", "/nope", "")
                .await
                .starts_with("HTTP/1.1 404"),
            "unmatched path is 404"
        );
        assert!(
            request(addr, "DELETE", "/echo", "")
                .await
                .starts_with("HTTP/1.1 405"),
            "matched path + wrong method is 405"
        );
    }

    /// Robustness guard: an **endless** SSE handler must be torn down the moment the
    /// client disconnects — never leaked, never left spinning. The byte stream is a
    /// bounded channel, so the producer parks under back-pressure (no busy loop) and
    /// the write returns `false` once the reader is gone, ending the handler. We prove
    /// it observably: the per-request process count returns to baseline after a drop.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_endless_sse_handler_is_torn_down_when_the_client_disconnects() {
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(HANDLERS).unwrap(), "run")
            .unwrap();
        wr.register_component("demo", prepared);
        let table = RouteTable::from_map(&HashMap::from([(
            "GET /firehose".to_string(),
            "demo#firehose".to_string(),
        )]))
        .unwrap();
        let caps = HashMap::from([(
            "demo".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        let addr = serve_on(wr.routed_http_server(resolver(table), caps)).await;

        let baseline = rt.process_count();

        // Open the endless stream and read until the first event lands (handler live).
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /firehose HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf))
                .await
                .expect("first SSE event arrives in time")
                .expect("socket read ok");
            assert!(n > 0, "the endless feed produced data");
            seen.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&seen).contains("ev 0") {
                break;
            }
        }
        assert!(
            rt.process_count() > baseline,
            "the endless handler process is live while streaming"
        );

        // Disconnect: dropping the socket must end the handler — back-pressured write
        // returns false, the loop breaks, the process exits. No leak, no spin.
        drop(conn);
        let mut reclaimed = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if rt.process_count() <= baseline {
                reclaimed = true;
                break;
            }
        }
        assert!(
            reclaimed,
            "the endless SSE handler must exit on client disconnect (it did not — leaked or spinning)"
        );
    }
}
