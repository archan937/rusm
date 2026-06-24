//! Serving a component as an **HTTP handler** (Phase 11): host the standard
//! `wasi:http/incoming-handler` via hyper + `wasmtime-wasi-http`. One fresh,
//! sandboxed component instance **per request** — cheap on the pooled spawn path,
//! and a trap is just that one request failing. The response is produced **by the
//! guest** (RS via `wstd`, or TS via the js-runner's `fetch` shape); the host only
//! moves bytes.

use std::sync::Arc;

use anyhow::{bail, Result};
use wasmtime::component::Component;
use wasmtime::Store;
use wasmtime_wasi::ResourceTable;
use wasmtime_wasi_http::io::TokioIo;
use wasmtime_wasi_http::p2::bindings::http::types::Scheme;
use wasmtime_wasi_http::p2::bindings::ProxyPre;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::WasiHttpView;
use wasmtime_wasi_http::WasiHttpCtx;

use super::{HttpCaps, WasiHost};
use crate::caps::Capabilities;
use crate::{Spawner, WasmRuntime};

/// A `wasi:http` component with its imports resolved and pre-instantiated — the
/// fast path for per-request instantiation.
#[derive(Clone)]
pub struct PreparedHttp {
    pre: ProxyPre<WasiHost>,
}

/// A ready-to-run HTTP server: a prepared component, the spawn core (engine +
/// runtime), and the capability profile each request instance runs under. Cheap to
/// clone (all `Arc`-backed), so it spawns one task per connection.
#[derive(Clone)]
pub struct HttpServer {
    pre: ProxyPre<WasiHost>,
    spawner: Arc<Spawner>,
    caps: Capabilities,
    /// The listener's `[serve.headers]` — merged into every response (e.g. CORS / security
    /// headers). Empty by default.
    headers: Arc<Vec<(String, String)>>,
    /// Max concurrent connections (the listener's `max_connections`); at the cap a new
    /// connection is dropped before it's served. `None` = unlimited.
    max_connections: Option<usize>,
    /// TLS acceptor (the listener's `tls`); when set, each connection is TLS-terminated
    /// before hyper (`https`). `None` = plain HTTP.
    tls: Option<Arc<super::tls::TlsAcceptor>>,
}

impl WasmRuntime {
    /// Prepare a `wasi:http` (proxy) component for serving.
    pub fn prepare_http(&self, component: &Component) -> Result<PreparedHttp> {
        let pre = ProxyPre::new(self.component_linker.instantiate_pre(component)?)?;
        Ok(PreparedHttp { pre })
    }

    /// Build a server that runs each request on a fresh instance under `caps`.
    pub fn http_server(&self, prepared: &PreparedHttp, caps: Capabilities) -> HttpServer {
        HttpServer {
            pre: prepared.pre.clone(),
            spawner: Arc::clone(&self.spawner),
            caps,
            headers: Arc::new(Vec::new()),
            max_connections: None,
            tls: None,
        }
    }

    /// Build an HTTP server whose handler is a **TypeScript/JS bundle** (Bun-built)
    /// on the embedded js-http-runner — the TS twin of [`http_server`]. The bundle is
    /// delivered to each per-request instance via the `RUSM_JS_BUNDLE` env capability;
    /// the guest exports a server-side request→response handler (`export default async
    /// (request) => Response`; the Workers `{ fetch }` shape is also accepted).
    pub fn http_server_js(&self, bundle: impl Into<String>, caps: Capabilities) -> HttpServer {
        let caps = caps.env("RUSM_JS_BUNDLE", bundle.into());
        let prepared = self.js_http_runner().clone();
        self.http_server(&prepared, caps)
    }

    /// The shared, embedded js-http-runner, compiled + prepared once (lazily) so
    /// non-serving nodes pay nothing.
    fn js_http_runner(&self) -> &PreparedHttp {
        self.js_http_runner.get_or_init(|| {
            // The embedded runner, or a per-app override (`rusm build` rebuilds it with the
            // app's custom bridges compiled in). A failure here is a build bug.
            let wasm = self
                .js_http_runner_wasm
                .as_deref()
                .unwrap_or(crate::JS_HTTP_RUNNER_WASM);
            self.prepare_http(
                &self
                    .compile_component(wasm)
                    .expect("js-http-runner compiles"),
            )
            .expect("js-http-runner prepares")
        })
    }
}

impl HttpServer {
    /// Add the listener's `[serve.headers]` (response-policy headers merged into each
    /// reply — e.g. CORS / security headers).
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = Arc::new(headers);
        self
    }

    /// Cap concurrent connections; at the cap a new connection is dropped before it's
    /// served (a flood can't pile up unbounded handler instances). `None` = unlimited.
    pub fn with_max_connections(mut self, max: Option<usize>) -> Self {
        self.max_connections = max;
        self
    }

    /// Terminate TLS on each connection with this acceptor (`https`); `None` = plain HTTP.
    pub fn with_tls(mut self, tls: Option<Arc<super::tls::TlsAcceptor>>) -> Self {
        self.tls = tls;
        self
    }

    /// Serve HTTP/1.1 on `listener` until it closes (one connection per task, one
    /// component instance per request). Abort the task driving this to stop.
    pub async fn serve(self, listener: tokio::net::TcpListener) {
        // A connection-cap semaphore (when set): a permit held for each connection's life
        // (until `serve_connection` completes), so the cap bounds live connections.
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
                // Set TCP_NODELAY + terminate TLS (when configured) in the task, off the accept
                // loop — a slow TLS handshake can't stall accepting other connections.
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

    /// A fresh per-request store: a new sandboxed `WasiHost` under this server's
    /// capability profile, with the memory limiter and epoch deadline set.
    fn fresh_store(&self) -> Result<Store<WasiHost>> {
        let host = WasiHost {
            wasi: self.caps.build_wasi()?,
            table: ResourceTable::new(),
            http: WasiHttpCtx::new(),
            http_hooks: HttpCaps {
                allow_network: self.caps.network_allowed(),
            },
            pid: 0,
            // The wasi:http per-request path exposes the request through wasi:http itself,
            // not the actor `connection` op; that op is for per-connection ws/sse handlers.
            connection: None,
            ws_out: None,
            sse_out: None,
            caps: self.caps.clone(),
            rt: self.spawner.rt.clone(),
            ctx: None,
            spawner: Some(Arc::clone(&self.spawner)),
            out_streams: Default::default(),
            in_streams: Default::default(),
            next_stream: 0,
            timers: Default::default(),
            next_timer: 0,
        };
        let mut store = Store::new(self.pre.engine(), host);
        store.limiter(|host| host as &mut dyn wasmtime::ResourceLimiter);
        // Epoch preemption applies to request handlers too — a runaway guest yields.
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        Ok(store)
    }

    /// Build a store and instantiate the component **without serving a request** —
    /// a measurement hook to separate per-request instantiation cost from the
    /// handler's own work (see the `http_bench` example).
    pub async fn instantiate_once(&self) -> Result<()> {
        let mut store = self.fresh_store()?;
        self.pre.instantiate_async(&mut store).await?;
        Ok(())
    }

    /// Serve one request and log it: `rusm http|sse <method> <path> → <status>` (gated by
    /// `[log] level`). SSE is told from plain HTTP by the response content-type; a handler
    /// that errors before producing a response logs as `502`.
    async fn handle(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<HyperOutgoingBody>> {
        let method = req.method().as_str().to_string();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();
        // A guest that streams a `text/event-stream` body must not be cached — ensure
        // `no-cache` (the recommended SSE path is the per-connection `SseServer`, which
        // sets it itself; this covers a raw `wasi:http` component that streams one).
        let result = self.dispatch(req).await.map(|mut response| {
            super::access::ensure_no_cache(&mut response);
            super::access::apply_extra_headers(&mut response, &self.headers);
            response
        });
        let (status, proto) = match &result {
            Ok(r) => (
                r.status().as_u16(),
                if super::access::is_event_stream(r.headers()) {
                    "sse"
                } else {
                    "http"
                },
            ),
            Err(_) => (502, "http"),
        };
        super::access::log_request(&self.spawner.rt, proto, &method, &path, status);
        result
    }

    /// Run one request through a fresh component instance and return its response.
    async fn dispatch(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<HyperOutgoingBody>> {
        let mut store = self.fresh_store()?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let request = store
            .data_mut()
            .http()
            .new_incoming_request(Scheme::Http, req)?;
        let out = store.data_mut().http().new_response_outparam(tx)?;
        let pre = self.pre.clone();

        // The handler runs in its own task: it may keep streaming the body after the
        // status/headers are sent (SSE), so we don't join it before replying.
        let task = tokio::spawn(async move {
            let proxy = pre.instantiate_async(&mut store).await?;
            proxy
                .wasi_http_incoming_handler()
                .call_handle(store, request, out)
                .await
        });

        match rx.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(code)) => Err(code.into()),
            // The guest dropped the outparam without setting a response — surface why.
            Err(_) => match task.await {
                Ok(Ok(())) => bail!("guest handler returned without setting a response"),
                Ok(Err(err)) => Err(err.into()),
                Err(join) => Err(join.into()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapabilityProfile;
    use rusm_otp::Runtime;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const HELLO: &[u8] = include_bytes!("../../tests/fixtures/http_hello.wasm");
    const SSE: &[u8] = include_bytes!("../../tests/fixtures/sse_ticker.wasm");
    const TS_HELLO: &str = include_str!("../../tests/fixtures/ts_http_hello.js");
    const TS_SSE: &str = include_str!("../../tests/fixtures/ts_sse_ticker.js");

    /// One raw HTTP/1.1 GET (Connection: close) → the full response text.
    async fn get(addr: std::net::SocketAddr) -> String {
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// One raw HTTP/1.1 request (Connection: close) → the full response text.
    async fn request(addr: std::net::SocketAddr, method: &str, path: &str, body: &str) -> String {
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

    /// Like [`get`], but returns the raw response bytes — so a non-ASCII body can be
    /// asserted exactly, not through a lossy (replacement-char) `String`.
    async fn get_bytes(addr: std::net::SocketAddr) -> Vec<u8> {
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        buf
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serves_https_over_native_tls() {
        // A listener with TLS serves the same component over `https`: a self-signed cert
        // terminates on the host, and a rustls client that trusts it round-trips a request.
        use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
        use tokio_rustls::rustls::{ClientConfig, RootCertStore};
        use tokio_rustls::TlsConnector;

        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_http(&wr.compile_component(HELLO).unwrap())
            .unwrap();

        // A self-signed cert for "localhost" → the listener's acceptor + the client's trust root.
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let acceptor = crate::tls_acceptor(
            cert.serialize_pem().unwrap().as_bytes(),
            cert.serialize_private_key_pem().as_bytes(),
        )
        .unwrap();
        let cert_der = CertificateDer::from(cert.serialize_der().unwrap());

        let server = wr
            .http_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_tls(Some(std::sync::Arc::new(acceptor)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // A rustls client that trusts only the self-signed cert.
        let mut roots = RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let provider = std::sync::Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(std::sync::Arc::new(config));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .expect("the TLS handshake succeeds against the self-signed cert");

        tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        tls.read_to_end(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains("hello from RUSM"), "TLS-served body: {resp}");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wasm_component_serves_an_http_request() {
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_http(&wr.compile_component(HELLO).unwrap())
            .unwrap();
        let server = wr.http_server(&prepared, CapabilityProfile::Trusted.capabilities());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let response = get(addr).await;
        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
        assert!(
            response.contains("hello from RUSM"),
            "the component produced the body: {response}"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wasm_component_streams_server_sent_events() {
        use std::time::{Duration, Instant};

        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_http(&wr.compile_component(SSE).unwrap())
            .unwrap();
        let server = wr.http_server(&prepared, CapabilityProfile::Trusted.capabilities());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        // Read incrementally, stamping when the first event lands vs the last byte —
        // the gap proves the body was streamed over time, not buffered then flushed.
        let start = Instant::now();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let mut first_event_at = None;
        loop {
            let n = conn.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if first_event_at.is_none() && String::from_utf8_lossy(&buf).contains("data: tick 0") {
                first_event_at = Some(start.elapsed());
            }
        }
        let total = start.elapsed();
        let text = String::from_utf8_lossy(&buf);

        assert!(text.starts_with("HTTP/1.1 200"), "got: {text}");
        assert!(
            text.to_lowercase().contains("text/event-stream"),
            "SSE content-type from the guest: {text}"
        );
        for n in 0..5 {
            assert!(
                text.contains(&format!("data: tick {n}")),
                "missing event {n}"
            );
        }
        // Five events 50ms apart: the first must arrive well before the stream ends.
        let first = first_event_at.expect("the first SSE event was seen");
        assert!(
            total - first >= Duration::from_millis(120),
            "events should stream over time (first at {first:?}, done at {total:?})"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_component_serves_an_http_request() {
        // The response is produced by a TS `fetch` handler on the js-http-runner.
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let server = wr.http_server_js(TS_HELLO, CapabilityProfile::Trusted.capabilities());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let response = get(addr).await;
        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
        assert!(
            response.contains("hello from TS"),
            "the TS HTTP handler produced the body: {response}"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_string_response_is_utf8_with_charset() {
        // Two regressions in one bare-string Response (no Content-Type set):
        //  1. the TextEncoder must encode an astral code point (emoji — a UTF-16
        //     surrogate pair) as one 4-byte UTF-8 sequence, not two bogus 3-byte ones
        //     (`👋` → `??????`);
        //  2. a string body must default to `text/plain;charset=UTF-8`, or a browser
        //     decodes the UTF-8 bytes as Latin-1 (`👋` → `ðŸ‘‹`).
        // The `rusm new` HTTP template greets with 👋, so both shipped broken once.
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let bundle = r#"module.exports = { default: () => new Response("wave \u{1F44B} done") };"#;
        let server = wr.http_server_js(bundle, CapabilityProfile::Trusted.capabilities());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let bytes = get_bytes(addr).await;
        // U+1F44B 👋 must appear as its exact 4-byte UTF-8 encoding in the body.
        let wave = [0xF0u8, 0x9F, 0x91, 0x8B];
        assert!(
            bytes.windows(wave.len()).any(|w| w == wave),
            "emoji must round-trip as 4-byte UTF-8; got: {}",
            String::from_utf8_lossy(&bytes)
        );
        // ...and the (ASCII) headers must declare the charset.
        let head = String::from_utf8_lossy(&bytes).to_lowercase();
        assert!(
            head.contains("charset=utf-8"),
            "a string Response must default to text/plain;charset=UTF-8; got: {head}"
        );

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_http_handler_uses_kv_publish_and_console() {
        use rusm_otp::Received;

        // The js-http-runner now gives a TS `fetch` handler the actor subset a request
        // handler can back: `kv` (persist), `Process.whereisTag`/`send` (publish to
        // subscribers), and `console` → the platform log. This proves all three end-to-end
        // — exactly the primitives a stateful TS HTTP API needs.
        // An inline bundle is eval'd inside a CommonJS wrapper, so it uses `module.exports`
        // (a built bundle would `export default`; Bun lowers it to this).
        const BUNDLE: &str = r#"
            module.exports.default = async function handle(request) {
              const store = kv.bucket("items");
              const url = new URL(request.url);
              if (request.method === "POST") {
                const item = await request.json();
                store.set(item.id, item.text);              // kv persist
                console.log("stored " + item.id);           // console → platform log
                const payload = new TextEncoder().encode(item.text);
                for (const pid of Process.whereisTag("items")) Process.send(pid, payload); // publish
                return new Response("ok", { status: 201 });
              }
              const v = store.get(url.pathname.slice(1));    // GET /<id>
              return v ? new Response(new TextDecoder().decode(v)) : new Response("missing", { status: 404 });
            };
        "#;
        let path =
            std::env::temp_dir().join(format!("rusm-kv-httpgap-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let rt = Runtime::new();
        let wr = WasmRuntime::with_store(rt.clone(), &path).unwrap();

        // A subscriber tagged "items" forwards each published payload to a channel.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sub = rt.spawn(move |mut ctx| async move {
            loop {
                if let Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(b);
                }
            }
        });
        rt.register_tag("items", sub.pid());

        let server = wr.http_server_js(BUNDLE, CapabilityProfile::Trusted.capabilities());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // POST: the handler persists to kv, publishes to the "items" tag, and logs.
        let post = request(addr, "POST", "/items", r#"{"id":"1","text":"hello"}"#).await;
        assert!(post.starts_with("HTTP/1.1 201"), "POST ok: {post}");

        // The published payload reached the tagged subscriber — `Process.whereisTag` +
        // `send` work from a TS HTTP handler.
        let published = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a published message within 5s")
            .expect("subscriber channel stays open");
        assert_eq!(
            published, b"hello",
            "publish via whereisTag+send from a TS HTTP handler"
        );

        // A GET on a fresh per-request instance reads the persisted value back — `kv` is
        // durable across the process-per-request instances.
        let got = request(addr, "GET", "/1", "").await;
        assert!(
            got.starts_with("HTTP/1.1 200") && got.contains("hello"),
            "kv persisted across requests: {got}"
        );

        handle.abort();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_component_streams_server_sent_events() {
        // A TS handler returns a Response whose body is a ReadableStream; the raw
        // wasi:http runner pulls + flushes each event, so the response is chunked
        // (written incrementally) rather than a single Content-Length body.
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let server = wr.http_server_js(TS_SSE, CapabilityProfile::Trusted.capabilities());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        let response = get(addr).await;
        let lower = response.to_lowercase();
        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
        assert!(
            lower.contains("text/event-stream"),
            "SSE content-type from the TS guest: {response}"
        );
        assert!(
            lower.contains("transfer-encoding: chunked"),
            "streamed incrementally (chunked), not buffered: {response}"
        );
        // The host adds `no-cache` to any event-stream response (an event-stream must
        // never be cached) — see `access::ensure_no_cache`.
        assert!(
            lower.contains("cache-control: no-cache"),
            "host adds no-cache to an event-stream: {response}"
        );
        for n in 0..5 {
            assert!(
                response.contains(&format!("data: tick {n}")),
                "missing event {n}"
            );
        }

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_http_listener_caps_concurrent_connections() {
        use std::time::Duration;

        // `max_connections = 1`: one keep-alive connection held live holds the only slot, a
        // second is dropped before any response, and once the first closes the slot frees up.
        let wr = WasmRuntime::new(Runtime::new()).unwrap();
        let prepared = wr
            .prepare_http(&wr.compile_component(HELLO).unwrap())
            .unwrap();
        let server = wr
            .http_server(&prepared, CapabilityProfile::Trusted.capabilities())
            .with_max_connections(Some(1));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(server.serve(listener));

        // First connection: keep-alive (no `Connection: close`), held open after its request
        // so its permit stays held while we test the cap.
        let mut first = tokio::net::TcpStream::connect(addr).await.unwrap();
        first
            .write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\n\r\n")
            .await
            .unwrap();
        let mut head = [0u8; 1024];
        let n = first.read(&mut head).await.unwrap();
        assert!(
            String::from_utf8_lossy(&head[..n]).starts_with("HTTP/1.1 200"),
            "the first connection is served"
        );

        // Second, while the first is live: dropped at the cap — no response (EOF or reset).
        let mut second = tokio::net::TcpStream::connect(addr).await.unwrap();
        second
            .write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut sbuf = Vec::new();
        let dropped = tokio::time::timeout(Duration::from_secs(5), second.read_to_end(&mut sbuf))
            .await
            .expect("the capped connection is dropped promptly");
        assert!(
            matches!(dropped, Ok(0)) || dropped.is_err(),
            "a second connection at the cap gets no response, got {dropped:?}"
        );
        assert!(
            sbuf.is_empty(),
            "no HTTP head is written to a capped connection"
        );

        // Close the first → its permit releases → a new connection is served again.
        drop(first);
        let served = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
                conn.write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                let mut b = Vec::new();
                if tokio::time::timeout(Duration::from_millis(300), conn.read_to_end(&mut b))
                    .await
                    .is_ok()
                    && String::from_utf8_lossy(&b).starts_with("HTTP/1.1 200")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            served.is_ok(),
            "the freed slot serves a new connection after the first releases it"
        );

        handle.abort();
    }
}
