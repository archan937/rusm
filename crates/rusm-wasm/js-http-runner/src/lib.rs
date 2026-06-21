//! The **rusm-ts js-http-runner**: a `wasi:http` component that embeds rquickjs and runs
//! a Bun-built TS/JS bundle's `fetch` handler — both request/response HTTP and
//! **streaming SSE**. The TypeScript twin of the wstd HTTP path.
//!
//! Why raw `wasi:http` (not wstd)? To stream, the response body must lazily pull each
//! event from the JS reader *as bytes are sent* — i.e. touch the rquickjs context.
//! rquickjs's context is `!Send`, and wstd's `Body` is `Send + 'static`, so a wstd
//! streaming body can't hold it. The raw handler is synchronous `handle(request, out)`:
//! we set headers, take the body's output-stream, and write chunks ourselves — the
//! context just lives in a thread-local for the duration, nothing crosses a thread
//! boundary, no `Send` bound. We own the bridge.
//!
//! **Pre-initialized (wizer).** The QuickJS engine + the JS bridge are booted **once** at
//! build time by [`wizer_initialize`] and snapshotted into the image; each per-request
//! instance CoW-starts from that snapshot, so [`warm`] finds the engine already up and
//! only the runtime bundle + `fetch` run per request. The exact same `warm()` path boots
//! lazily if the module was *not* wizer'd, so behaviour is identical either way — wizer
//! just moves the cost from per-request to build-time. Still **instance-per-request**
//! (a fresh, isolated instance per request; never resident), so a hung or crashed
//! request can never block the server.
//!
//! The bundle is delivered per-request via the `RUSM_JS_BUNDLE` env capability; the guest
//! writes the Workers/Deno `export default { fetch }` shape. A static response is written
//! in one shot; a `ReadableStream` body is pulled chunk-by-chunk and flushed as it's
//! produced (back-pressured by the socket via blocking writes).
//!
//! Beyond `fetch`, a handler gets the actor-ABI subset a *request* handler can back: the
//! `kv` global (durable storage, gated by `storage`), `console.*` → the platform log, and
//! `Process.{self,send,whereis,whereisTag}` — so it can persist and **publish** to
//! subscribers (e.g. notify open SSE streams), exactly like an actor component. It has no
//! mailbox, so the receive/spawn/monitor ops are absent (the bridge's presence-guards
//! install only the wired subset). The actor world is imported in WIT and satisfied by the
//! same host linker as every component; the JS bridge is shared verbatim with the js-runner.

use anyhow::{anyhow, Result};
use rquickjs::{Array, Context, Ctx, Exception, Function, Object, Promise, Runtime, TypedArray};
use std::cell::OnceCell;

wit_bindgen::generate!({ world: "bindings", path: "wit", generate_all });

use exports::wasi::http::incoming_handler::Guest;
use rusm::runtime::actor;
use rusm::runtime::kv;
use wasi::http::types::{
    Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam,
};
use wasi::io::streams::OutputStream;

// The actor bridge JS is shared verbatim with the js-runner (single source of truth) —
// only the *backing* native `__*` ops differ (this runner wires the no-mailbox subset),
// so `process.js`'s presence-guards expose exactly that subset here.
const LOG_JS: &str = include_str!("../../js-runner/bridge/log.js");
const WEBAPI_JS: &str = include_str!("../../js-runner/bridge/webapi.js");
const STREAM_JS: &str = include_str!("../../js-runner/bridge/streams.js");
const PROCESS_JS: &str = include_str!("../../js-runner/bridge/process.js");
const KV_JS: &str = include_str!("../../js-runner/bridge/kv.js");
const HTTP_JS: &str = include_str!("../bridge/http.js");
/// wasi:io `blocking-write-and-flush` accepts at most 4096 bytes per call.
const WRITE_CHUNK: usize = 4096;

thread_local! {
    /// The **warm** QuickJS context — engine + bridge, booted once. Baked into the image
    /// by [`wizer_initialize`] at build time (so each instance CoW-starts warm) and
    /// reused for the instance's lifetime. The `Context` owns its `Runtime`, so holding
    /// it here keeps the engine alive.
    static WARM: OnceCell<Context> = const { OnceCell::new() };
}

/// Run `f` against the warm context, booting the engine + bridge **once** if it isn't up
/// yet (the build-time wizer snapshot normally means it already is — this is the single
/// source for "a ready context", whether wizer ran or not).
fn with_warm<R>(f: impl FnOnce(&Ctx<'_>) -> R) -> R {
    WARM.with(|cell| {
        let ctx = cell.get_or_init(|| {
            let runtime = Runtime::new().expect("quickjs runtime");
            let context = Context::full(&runtime).expect("quickjs context");
            context.with(boot_bridge);
            context
        });
        ctx.with(|c| f(&c))
    })
}

/// Boot the JS bridge into a fresh context: the host `__print`, the Web-API polyfills
/// (`webapi.js`), the HTTP glue (`http.js`), and an empty CommonJS module the per-request
/// bundle populates. This is the work wizer bakes into the snapshot — done once, not per
/// request. (`__print` is a Rust closure; verified to survive the wizer snapshot.)
fn boot_bridge(ctx: Ctx<'_>) {
    let g = ctx.globals();
    macro_rules! def {
        ($name:expr, $func:expr) => {
            g.set($name, Function::new(ctx.clone(), $func).expect($name))
                .expect($name);
        };
    }
    def!("__print", |s: String| eprintln!("{s}"));

    // The actor-ABI subset a *request* handler can back: no mailbox, so storage, the
    // platform logger, and the outbound/lookup ops (publish to subscribers, find a peer)
    // — but no `receive`/`spawn`/`monitor`/tag-join. `process.js`'s presence-guards turn
    // this set into exactly `Process.{self,send,whereis,whereisTag}`; `console.*` routes
    // to `__log`; `kv` is gated by the component's `storage` capability.
    def!("__own_pid", || actor::own_pid().to_string());
    def!("__whereis", |n: String| actor::whereis(&n)
        .map(|p| p.to_string())
        .unwrap_or_default());
    def!("__whereis_tag", |t: String| actor::whereis_tag(&t)
        .into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>());
    def!("__send_text", |to: String, s: String| actor::send(
        to.parse().unwrap_or(0),
        s.as_bytes()
    ));
    def!("__send", js_send);
    def!("__log", |level: String, message: String| {
        use rusm::runtime::log as hostlog;
        let level = match level.as_str() {
            "error" => hostlog::LogLevel::Error,
            "warn" => hostlog::LogLevel::Warn,
            "debug" => hostlog::LogLevel::Debug,
            _ => hostlog::LogLevel::Info,
        };
        hostlog::log(level, &message);
    });
    def!("__kv_get", js_kv_get);
    def!("__kv_set", js_kv_set);
    def!("__kv_delete", js_kv_delete);
    def!("__kv_exists", js_kv_exists);
    def!("__kv_list", js_kv_list);

    // Bridge JS, in dependency order: log (console → `__log`), webapi (the base Web
    // standards), the actor `Process` (guarded to the wired subset), `kv`, then the HTTP glue.
    eval(&ctx, LOG_JS, "log.js").expect("log.js");
    eval(&ctx, WEBAPI_JS, "webapi.js").expect("webapi.js");
    eval(&ctx, STREAM_JS, "stream.js").expect("stream.js");
    eval(&ctx, PROCESS_JS, "process.js").expect("process.js");
    eval(&ctx, KV_JS, "kv.js").expect("kv.js");
    eval(&ctx, HTTP_JS, "http.js").expect("http.js");
    eval(
        &ctx,
        "globalThis.module={exports:{}};globalThis.exports=module.exports;",
        "cjs-shim",
    )
    .expect("cjs-shim");
}

// Thin wit-bindgen glue mirroring the js-runner's (the bindings are crate-local generated
// code, so the wrappers are necessarily per-crate; the JS logic above is the shared source).
fn js_send(to: String, data: TypedArray<u8>) {
    actor::send(to.parse().unwrap_or(0), data.as_bytes().unwrap_or(&[]));
}
fn js_kv_get(ctx: Ctx<'_>, bucket: String, key: String) -> rquickjs::Result<Option<TypedArray<'_, u8>>> {
    match kv::get(&bucket, &key) {
        Ok(Some(bytes)) => Ok(Some(TypedArray::new(ctx, bytes)?)),
        Ok(None) => Ok(None),
        Err(e) => Err(Exception::throw_message(&ctx, &e)),
    }
}
fn js_kv_set(ctx: Ctx<'_>, bucket: String, key: String, value: TypedArray<u8>) -> rquickjs::Result<()> {
    kv::set(&bucket, &key, value.as_bytes().unwrap_or(&[]))
        .map_err(|e| Exception::throw_message(&ctx, &e))
}
fn js_kv_delete(ctx: Ctx<'_>, bucket: String, key: String) -> rquickjs::Result<bool> {
    kv::delete(&bucket, &key).map_err(|e| Exception::throw_message(&ctx, &e))
}
fn js_kv_exists(ctx: Ctx<'_>, bucket: String, key: String) -> rquickjs::Result<bool> {
    kv::exists(&bucket, &key).map_err(|e| Exception::throw_message(&ctx, &e))
}
fn js_kv_list(ctx: Ctx<'_>, bucket: String) -> rquickjs::Result<Vec<String>> {
    kv::list(&bucket).map_err(|e| Exception::throw_message(&ctx, &e))
}

/// Wizer's build-time entry point (`./build.sh` runs `wizer --init-func wizer_initialize`):
/// boot the engine + bridge so the snapshot captures a warm context. Not part of the
/// component interface — a plain core export wizer calls, then `wasm-tools component new`
/// drops. May not call imports, so the bridge eval (pure JS, no host calls) is fine.
#[no_mangle]
pub extern "C" fn wizer_initialize() {
    with_warm(|_| {});
}

/// The handler's response metadata; the body is either fully-known bytes (static) or
/// pulled incrementally from the JS reader (`streaming`).
struct Outcome {
    status: u16,
    headers: Vec<(String, String)>,
    streaming: bool,
    body: Vec<u8>,
}

struct Handler;

impl Guest for Handler {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let bundle = std::env::var("RUSM_JS_BUNDLE").unwrap_or_default();
        let (method, url, req_headers, req_body) = read_request(request);

        // Phase 1: eval the bundle into the warm context and run `fetch`, settling status
        // + headers (+ a static body, or arming the stream reader).
        let outcome =
            with_warm(|ctx| run_fetch(ctx, &bundle, &method, &url, &req_headers, &req_body));
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => return fail(response_out, &format!("{e}")),
        };

        // Phase 2: hand off the response head, then write the body.
        let response = build_response(outcome.status, &outcome.headers);
        let Ok(out_body) = response.body() else {
            return fail(response_out, "response has no body");
        };
        ResponseOutparam::set(response_out, Ok(response));
        let Ok(stream) = out_body.write() else { return };

        if outcome.streaming {
            // Pull each chunk from the JS reader and flush it — true incremental SSE.
            // A write error means the client hung up; stop pulling.
            loop {
                match with_warm(pull_chunk) {
                    Ok(Some(chunk)) => {
                        if write_all(&stream, &chunk).is_err() {
                            break;
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
        } else {
            let _ = write_all(&stream, &outcome.body);
        }

        drop(stream); // release the borrow before finishing the body
        let _ = OutgoingBody::finish(out_body, None);
    }
}

/// In the **already-warm** context (bridge loaded), eval the per-request bundle, build the
/// JS `Request`, run `fetch`, and read back the response head. For a static body the bytes
/// come back now; for a stream the JS reader is armed and `pull_chunk` drives it.
fn run_fetch(
    ctx: &Ctx<'_>,
    bundle: &str,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Outcome> {
    // Wrap the bundle in a CommonJS scope so its top-level vars don't leak.
    let wrapped = format!(
        "(function(module,exports){{\n{bundle}\n}})(globalThis.module,globalThis.module.exports);"
    );
    eval(ctx, &wrapped, "bundle")?;

    let header_arr = Array::new(ctx.clone()).map_err(|e| anyhow!("header array: {e}"))?;
    for (i, (k, v)) in headers.iter().enumerate() {
        let pair = Array::new(ctx.clone()).map_err(|e| anyhow!("header pair: {e}"))?;
        pair.set(0, k.as_str()).ok();
        pair.set(1, v.as_str()).ok();
        header_arr.set(i, pair).ok();
    }
    let body_ta = TypedArray::new(ctx.clone(), body).map_err(|e| anyhow!("body array: {e}"))?;

    let func: Function = ctx
        .globals()
        .get("__rusm_http")
        .map_err(|e| anyhow!("__rusm_http: {e}"))?;
    let promise: Promise = func
        .call((method, url, header_arr, body_ta))
        .map_err(|e| js_err(ctx, e, "call fetch handler"))?;
    let result: Object = promise
        .finish()
        .map_err(|e| js_err(ctx, e, "resolve fetch"))?;

    let status: i32 = result.get("status").map_err(|e| anyhow!("status: {e}"))?;
    let headers = read_headers(result.get("headers").map_err(|e| anyhow!("headers: {e}"))?)?;
    let streaming: bool = result.get("streaming").unwrap_or(false);
    let body = if streaming {
        Vec::new()
    } else {
        let ta: TypedArray<u8> = result.get("body").map_err(|e| anyhow!("body: {e}"))?;
        ta.as_bytes().unwrap_or(&[]).to_vec()
    };

    Ok(Outcome {
        status: status as u16,
        headers,
        streaming,
        body,
    })
}

/// Pull the next streamed chunk from the JS reader: `Some(bytes)` or `None` at end.
fn pull_chunk(ctx: &Ctx<'_>) -> Result<Option<Vec<u8>>> {
    let func: Function = ctx
        .globals()
        .get("__rusm_http_pull")
        .map_err(|e| anyhow!("__rusm_http_pull: {e}"))?;
    let promise: Promise = func.call(()).map_err(|e| js_err(ctx, e, "pull"))?;
    let chunk: Option<TypedArray<u8>> = promise
        .finish()
        .map_err(|e| js_err(ctx, e, "pull resolve"))?;
    Ok(chunk.map(|ta| ta.as_bytes().unwrap_or(&[]).to_vec()))
}

/// Read the request's method, full URL, headers, and body bytes.
fn read_request(request: IncomingRequest) -> (String, String, Vec<(String, String)>, Vec<u8>) {
    let method = method_str(request.method());
    let headers: Vec<(String, String)> = request
        .headers()
        .entries()
        .into_iter()
        .map(|(k, v)| (k, String::from_utf8_lossy(&v).into_owned()))
        .collect();
    // Reconstruct a full URL (scheme + host + path) so the guest's `new URL(req.url)`
    // parses the query — wasi:http only carries the path-with-query.
    let path = request.path_with_query().unwrap_or_else(|| "/".to_owned());
    let host = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "localhost".to_owned());
    let url = format!("http://{host}{path}");

    let body = read_body(&request);
    (method, url, headers, body)
}

fn read_body(request: &IncomingRequest) -> Vec<u8> {
    let Ok(incoming) = request.consume() else {
        return Vec::new();
    };
    let Ok(stream) = incoming.stream() else {
        return Vec::new();
    };
    let mut body = Vec::new();
    // `blocking_read` blocks until data or end-of-stream (Err = closed/EOF).
    while let Ok(chunk) = stream.blocking_read(64 * 1024) {
        if chunk.is_empty() {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    body
}

fn build_response(status: u16, headers: &[(String, String)]) -> OutgoingResponse {
    let fields = Fields::new();
    for (k, v) in headers {
        // append (not set) so multi-value headers survive; ignore invalid names.
        let _ = fields.append(k, v.as_bytes());
    }
    let response = OutgoingResponse::new(fields);
    let _ = response.set_status_code(status);
    response
}

/// Write all of `data` in ≤4096-byte flushes (the wasi:io per-call limit).
fn write_all(stream: &OutputStream, data: &[u8]) -> Result<()> {
    for chunk in data.chunks(WRITE_CHUNK) {
        stream
            .blocking_write_and_flush(chunk)
            .map_err(|e| anyhow!("write: {e:?}"))?;
    }
    Ok(())
}

/// Send a 500 with the cause as the body — a guest error is a 500, never a dropped
/// socket. Only valid before the response head has been handed off.
fn fail(response_out: ResponseOutparam, message: &str) {
    let fields = Fields::new();
    let _ = fields.append("content-type", b"text/plain;charset=utf-8");
    let response = OutgoingResponse::new(fields);
    let _ = response.set_status_code(500);
    let body = response.body().ok();
    ResponseOutparam::set(response_out, Ok(response));
    if let Some(body) = body {
        if let Ok(stream) = body.write() {
            let _ = write_all(
                &stream,
                format!("js-http-runner error: {message}").as_bytes(),
            );
            drop(stream);
            let _ = OutgoingBody::finish(body, None);
        }
    }
}

fn method_str(method: Method) -> String {
    match method {
        Method::Get => "GET".to_owned(),
        Method::Head => "HEAD".to_owned(),
        Method::Post => "POST".to_owned(),
        Method::Put => "PUT".to_owned(),
        Method::Delete => "DELETE".to_owned(),
        Method::Connect => "CONNECT".to_owned(),
        Method::Options => "OPTIONS".to_owned(),
        Method::Trace => "TRACE".to_owned(),
        Method::Patch => "PATCH".to_owned(),
        Method::Other(s) => s,
    }
}

fn eval(ctx: &Ctx<'_>, src: &str, what: &str) -> Result<()> {
    ctx.eval::<(), _>(src)
        .map_err(|e| js_err(ctx, e, &format!("eval {what}")))
}

/// Turn a rquickjs error into one carrying the actual JS exception message + stack
/// (the bare `Error::Exception` says nothing — the detail is in `ctx.catch()`).
fn js_err(ctx: &Ctx<'_>, err: rquickjs::Error, what: &str) -> anyhow::Error {
    if !matches!(err, rquickjs::Error::Exception) {
        return anyhow!("{what}: {err}");
    }
    let caught = ctx.catch();
    if let Some(ex) = caught.as_exception() {
        let msg = ex.message().unwrap_or_default();
        let stack = ex.stack().unwrap_or_default();
        return anyhow!("{what}: {msg}\n{stack}");
    }
    anyhow!("{what}: {caught:?}")
}

fn read_headers(arr: Array<'_>) -> Result<Vec<(String, String)>> {
    let mut out = Vec::with_capacity(arr.len());
    for pair in arr.iter::<Array>() {
        let pair = pair.map_err(|e| anyhow!("header pair: {e}"))?;
        let k: String = pair.get(0).map_err(|e| anyhow!("header key: {e}"))?;
        let v: String = pair.get(1).map_err(|e| anyhow!("header value: {e}"))?;
        out.push((k, v));
    }
    Ok(out)
}

export!(Handler);
