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

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Response, StatusCode};
use rusm_otp::{ProcessHandle, Received, Runtime};
use serde::Deserialize;
use wasmtime_wasi_http::io::TokioIo;

use crate::bridges::auth::{AuthHook, AuthRequest, AuthVerdict};
use crate::caps::Capabilities;
use crate::context::ProcessContext;
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
    /// The listener's `[serve.headers]` — merged into every response (e.g. CORS).
    headers: Arc<Vec<(String, String)>>,
    /// Max concurrent connections (the listener's `max_connections`); at the cap a new
    /// connection is dropped before it's served. `None` = unlimited.
    max_connections: Option<usize>,
    /// gzip eligible responses the client accepts (the listener's `compression`).
    compress: bool,
    /// TLS acceptor (the listener's `tls`); when set, each connection is TLS-terminated
    /// before hyper (`https`). `None` = plain HTTP.
    tls: Option<Arc<super::tls::TlsAcceptor>>,
    /// The listener's auth hook (`[[serve]] authentication`); when set, each request is
    /// validated before a handler is spawned — claims seed the request's context, a denial
    /// is `401`. `None` = no authentication (the request's context starts empty).
    auth: Option<AuthHook>,
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
            headers: Arc::new(Vec::new()),
            max_connections: None,
            compress: false,
            tls: None,
            auth: None,
        }
    }
}

impl RoutedHttpServer {
    /// Add the listener's `[serve.headers]` (response-policy headers merged into each
    /// reply — e.g. CORS / security headers).
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = Arc::new(headers);
        self
    }

    /// gzip eligible responses the client accepts (the listener's `compression`).
    pub fn with_compression(mut self, on: bool) -> Self {
        self.compress = on;
        self
    }

    /// Terminate TLS on each connection with this acceptor (`https`); `None` = plain HTTP.
    pub fn with_tls(mut self, tls: Option<Arc<super::tls::TlsAcceptor>>) -> Self {
        self.tls = tls;
        self
    }

    /// Authenticate each request with this hook (`[[serve]] authentication`); `None` = no
    /// authentication. The hook runs before the handler spawns: claims seed the request's
    /// host-only context, a denial short-circuits to `401`.
    pub fn with_auth(mut self, auth: Option<AuthHook>) -> Self {
        self.auth = auth;
        self
    }

    /// Cap concurrent connections; at the cap a new connection is dropped before it's
    /// served (a flood can't pile up unbounded handler instances). `None` = unlimited.
    pub fn with_max_connections(mut self, max: Option<usize>) -> Self {
        self.max_connections = max;
        self
    }

    /// Serve HTTP/1.1 on `listener` until it closes — one task per connection. Abort
    /// the task driving this to stop.
    pub async fn serve(self, listener: tokio::net::TcpListener) {
        // A connection-cap semaphore (when set): a permit held for each connection's life.
        let limiter = self
            .max_connections
            .map(|n| Arc::new(tokio::sync::Semaphore::new(n)));
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                break;
            };
            // At the cap, drop the socket before serving it.
            let permit = match &limiter {
                Some(sem) => match Arc::clone(sem).try_acquire_owned() {
                    Ok(p) => Some(p),
                    Err(_) => continue,
                },
                None => None,
            };
            let server = self.clone();
            tokio::spawn(async move {
                // TCP_NODELAY + TLS termination (when configured) off the accept loop.
                let Ok(io) = super::tls::MaybeTlsStream::accept(stream, &server.tls).await else {
                    return; // a failed TLS handshake drops just this connection (+ its permit)
                };
                let _permit = permit; // held until this connection ends
                let service = hyper::service::service_fn(move |req| {
                    let server = server.clone();
                    async move { server.handle(req).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(io), service)
                    .await;
            });
        }
    }

    /// Serve one request and log it: `rusm http <method> <path> → <status>` (gated by
    /// `[log] level`). Routed responses are buffered HTTP (SSE is served per-connection by
    /// [`super::sse::SseServer`], not through here).
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
        // Capture whether the client accepts gzip before the request is consumed.
        let accept_gzip = super::compress::accepts_gzip(
            req.headers()
                .get(hyper::header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok()),
        );
        let mut response = match self.dispatch(req).await {
            Ok(response) => response,
            Err(never) => match never {}, // dispatch is infallible
        };
        // Merge the listener's declared response policy (e.g. CORS) into every reply.
        super::access::apply_extra_headers(&mut response, &self.headers);
        // gzip the buffered reply when enabled, accepted, and eligible (after the policy
        // headers so a declared `content-encoding`/`content-type` is respected).
        response = super::compress::maybe_gzip(response, accept_gzip, self.compress).await;
        super::access::log_request(
            &self.spawner.rt,
            "http",
            &method,
            &path,
            response.status().as_u16(),
        );
        Ok(response)
    }

    /// Run the listener's auth hook, if configured. `Ok(context)` carries the seeded
    /// claims (an empty context when no hook is set); `Err(response)` is a `401` that
    /// short-circuits dispatch — no handler is spawned. The hook is host code; the request
    /// it sees never includes guest-controlled state.
    async fn authenticate(
        &self,
        method: &str,
        uri: &hyper::Uri,
        headers: &[(String, String)],
    ) -> Result<ProcessContext, Response<ResBody>> {
        let Some(hook) = &self.auth else {
            return Ok(ProcessContext::new());
        };
        let request = AuthRequest {
            method: method.to_string(),
            path: uri.path().to_string(),
            query: uri.query().unwrap_or("").to_string(),
            headers: headers.to_vec(),
        };
        match hook(request).await {
            AuthVerdict::Allow(claims) => Ok(ProcessContext::from_iter(claims)),
            AuthVerdict::Deny => Err(error_response(StatusCode::UNAUTHORIZED, "unauthorized")),
        }
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
        let method = parts.method.as_str().to_string();
        let headers: Vec<(String, String)> = parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // Authenticate before routing: a denied request is `401` and never spawns a handler
        // or reveals whether a route exists. On success the claims seed this request's
        // host-only context (empty when no hook is configured).
        let context = match self.authenticate(&method, &parts.uri, &headers).await {
            Ok(context) => context,
            Err(response) => return Ok(response),
        };

        let (component, action, params) = match (self.resolve)(&method, target) {
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

        let url = target.to_string();
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

        // A routed HTTP handler is a fixed component (never a dynamic template), so `prepared`
        // is present; `None` would mean a route pointing at a template — a config error.
        let Some(prepared) = entry.prepared.as_ref() else {
            return Ok(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("`{component}` is a dynamic template, not an HTTP handler"),
            ));
        };
        // Process-per-request: a fresh instance handles this one request, then exits.
        let child = self
            .spawner
            .spawn_component(prepared, caps, Some(&component));
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
        // The request rides to the handler with the auth-seeded claims context as its
        // mailbox meta (never in the payload); the handler binds it on receive, so any
        // bridge it — or a sub-component it spawns and calls — reaches acts for the
        // authenticated tenant. Guest code neither sees nor sets it.
        self.spawner.rt.send_with_meta(
            child_pid,
            serde_json::to_vec(&envelope).expect("envelope serializes"),
            context.into_meta(),
        );

        Ok(match rx.await {
            Ok(GatewayReply::Buffered(resp)) => build_response(resp),
            Ok(GatewayReply::Err(message)) => {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
            }
            Err(_) => error_response(StatusCode::BAD_GATEWAY, "handler did not reply"),
        })
    }
}

// ── The reply machinery: handler reply → HTTP response ────────────────────────
//
// An ephemeral Wasm-free **responder** process owns a `oneshot` and turns the handler's
// (buffered) reply into the HTTP response.

/// The handler's reply, as the responder hands it to the HTTP task.
enum GatewayReply {
    /// A complete buffered response.
    Buffered(rusm_wire::Response),
    /// The handler errored.
    Err(String),
}

/// A Wasm-free process that waits for the handler's reply and hands it to the HTTP task.
fn spawn_responder(rt: &Runtime, tx: tokio::sync::oneshot::Sender<GatewayReply>) -> ProcessHandle {
    rt.spawn(move |mut ctx| async move {
        let head = loop {
            match ctx.recv().await {
                Received::Message(bytes) => break bytes,
                _ => continue,
            }
        };
        let _ = tx.send(match parse_reply(&head) {
            Ok(resp) => GatewayReply::Buffered(resp),
            Err(err) => GatewayReply::Err(err),
        });
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

    /// Send a raw HTTP/1.1 request and split the response into (head text, body bytes) — so a
    /// gzipped (non-UTF-8) body can be decoded exactly.
    async fn raw_request(addr: SocketAddr, req: &str) -> (String, Vec<u8>) {
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a response has a header/body boundary");
        (
            String::from_utf8_lossy(&buf[..split]).into_owned(),
            buf[split + 4..].to_vec(),
        )
    }

    fn gunzip(data: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data)
            .read_to_end(&mut out)
            .unwrap();
        out
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

    /// Host bindings for the `whoami` custom bridge the `auth_demo` handler imports — it
    /// reflects the request's tenant straight out of the host-only claims context, which
    /// only an auth hook can have seeded. The mirror of an app's `bridges/whoami/host.rs`.
    mod whoami_bridge {
        wasmtime::component::bindgen!({
            inline: "
                package demo:bridge@0.1.0;
                interface whoami { tenant: func() -> string; }
                world whoami-host { import whoami; }
            ",
            imports: { default: async },
        });
    }

    impl whoami_bridge::demo::bridge::whoami::Host for crate::BridgeHost {
        async fn tenant(&mut self) -> String {
            // The request's authenticated tenant — host-only, never guest-supplied.
            self.context().get("app_id").unwrap_or("-").to_string()
        }
    }

    /// A serving server wired with the `auth_demo` handler, the `whoami` bridge, and an
    /// auth hook that only accepts `Authorization: Bearer good` (→ `app_id=acme`). `GET /me`
    /// reflects the tenant.
    async fn auth_server(with_hook: bool) -> SocketAddr {
        const AUTH_DEMO: &[u8] = include_bytes!("../../tests/fixtures/auth_demo.wasm");
        let wr = WasmRuntime::with_bridges(Runtime::new(), |linker| {
            whoami_bridge::demo::bridge::whoami::add_to_linker::<
                _,
                wasmtime::component::HasSelf<crate::BridgeHost>,
            >(linker, |host| host)
        })
        .unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(AUTH_DEMO).unwrap(), "run")
            .unwrap();
        wr.register_component("api", prepared);
        let table = RouteTable::from_map(&HashMap::from([(
            "GET /me".to_string(),
            "api#me".to_string(),
        )]))
        .unwrap();
        let caps = HashMap::from([(
            "api".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        let mut server = wr.routed_http_server(resolver(table), caps);
        if with_hook {
            // Built via the public `auth_hook` constructor — the exact shape the codegen emits
            // (`rusm_wasm::auth_hook(authenticate)`), so this exercises that path end to end.
            let hook = crate::auth_hook(|req: AuthRequest| async move {
                match req.header("authorization") {
                    Some("Bearer good") => {
                        AuthVerdict::Allow(vec![("app_id".to_string(), "acme".to_string())])
                    }
                    _ => AuthVerdict::Deny,
                }
            });
            server = server.with_auth(Some(hook));
        }
        serve_on(server).await
    }

    /// One raw HTTP/1.1 request carrying an `Authorization` header → the full response text.
    async fn request_with_auth(addr: SocketAddr, path: &str, authorization: &str) -> String {
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\nAuthorization: {authorization}\r\nContent-Length: 0\r\n\r\n"
        );
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_auth_hook_seeds_the_tenant_a_bridge_reads_and_rejects_a_bad_token() {
        // The multi-tenant serving seam, end to end over real HTTP. A valid token makes the
        // hook seed `app_id=acme`; that context rides the request message to the handler,
        // which calls the `whoami` bridge — the response is the *host-decided* tenant. An
        // invalid/missing token is `401` and the handler never runs. The guest code is
        // identical across tenants and never sees the identity.
        let addr = auth_server(true).await;

        let ok = request_with_auth(addr, "/me", "Bearer good").await;
        assert!(
            ok.starts_with("HTTP/1.1 200"),
            "valid token is served: {ok}"
        );
        assert!(
            ok.trim_end().ends_with("acme"),
            "bridge read the seeded tenant: {ok}"
        );

        let denied = request_with_auth(addr, "/me", "Bearer nope").await;
        assert!(
            denied.starts_with("HTTP/1.1 401"),
            "an invalid token is rejected before any handler runs: {denied}"
        );

        let missing = request(addr, "GET", "/me", "").await;
        assert!(
            missing.starts_with("HTTP/1.1 401"),
            "a missing token is rejected: {missing}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_an_auth_hook_the_context_is_empty_and_the_request_passes_through() {
        // No `authentication` configured: every request is served (no 401), and the bridge
        // sees an empty claims context — the handler returns the `-` sentinel. Proves the
        // hook is strictly opt-in and adds nothing to the unauthenticated path.
        let addr = auth_server(false).await;
        let resp = request_with_auth(addr, "/me", "Bearer good").await;
        assert!(resp.starts_with("HTTP/1.1 200"), "no hook → served: {resp}");
        assert!(
            resp.trim_end().ends_with('-'),
            "no hook → empty context (the `-` sentinel): {resp}"
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

    /// A routed listener with `compression` on: a large compressible reply the client accepts
    /// comes back gzip-encoded and round-trips, while a sub-threshold reply is left plain.
    async fn compression_server() -> SocketAddr {
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(HANDLERS).unwrap(), "run")
            .unwrap();
        wr.register_component("demo", prepared);
        let table = RouteTable::from_map(&HashMap::from([(
            "GET /hello/:name".to_string(),
            "demo#hello".to_string(),
        )]))
        .unwrap();
        let caps = HashMap::from([(
            "demo".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        serve_on(
            wr.routed_http_server(resolver(table), caps)
                .with_compression(true),
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gzips_a_large_eligible_response_the_client_accepts() {
        let addr = compression_server().await;
        // `hello` replies `text/plain` "hi <name>\n"; a long name pushes it past the
        // threshold. The handler's body is identical compressed or not — only the wire differs.
        let name = "a".repeat(400);
        let body_plain = format!("hi {name}\n").into_bytes();

        // Accept gzip → the reply is gzip-encoded and decodes back to the handler's bytes.
        let (head, body) = raw_request(
            addr,
            &format!(
                "GET /hello/{name} HTTP/1.1\r\nHost: rusm\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        let lower = head.to_lowercase();
        assert!(head.starts_with("HTTP/1.1 200"), "got: {head}");
        assert!(
            lower.contains("content-encoding: gzip"),
            "gzip applied: {head}"
        );
        assert!(lower.contains("vary: accept-encoding"), "Vary set: {head}");
        assert!(body.len() < body_plain.len(), "the wire body shrank");
        assert_eq!(
            gunzip(&body),
            body_plain,
            "gzip round-trips to the handler body"
        );

        // No `Accept-Encoding` → the same bytes, uncompressed.
        let (head, body) = raw_request(
            addr,
            &format!("GET /hello/{name} HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            !head.to_lowercase().contains("content-encoding"),
            "no encoding without Accept-Encoding: {head}"
        );
        assert_eq!(body, body_plain);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn leaves_a_sub_threshold_response_uncompressed() {
        let addr = compression_server().await;
        // "hi alice\n" is well under the gzip threshold, so it's sent plain even though the
        // client accepts gzip — small bodies don't benefit.
        let (head, body) = raw_request(
            addr,
            "GET /hello/alice HTTP/1.1\r\nHost: rusm\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            !head.to_lowercase().contains("content-encoding"),
            "tiny body left uncompressed: {head}"
        );
        assert_eq!(body, b"hi alice\n");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn does_not_compress_without_the_opt_in() {
        // Compression defaults off: even a large body the client accepts is sent plain.
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(HANDLERS).unwrap(), "run")
            .unwrap();
        wr.register_component("demo", prepared);
        let table = RouteTable::from_map(&HashMap::from([(
            "GET /hello/:name".to_string(),
            "demo#hello".to_string(),
        )]))
        .unwrap();
        let caps = HashMap::from([(
            "demo".to_string(),
            CapabilityProfile::Sandboxed.capabilities(),
        )]);
        let addr = serve_on(wr.routed_http_server(resolver(table), caps)).await; // no with_compression

        let name = "a".repeat(400);
        let (head, body) = raw_request(
            addr,
            &format!(
                "GET /hello/{name} HTTP/1.1\r\nHost: rusm\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(
            !head.to_lowercase().contains("content-encoding"),
            "no compression without the opt-in: {head}"
        );
        assert_eq!(body, format!("hi {name}\n").into_bytes());
    }
}
