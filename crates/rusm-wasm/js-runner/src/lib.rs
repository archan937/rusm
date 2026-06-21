//! The **rusm-ts js-runner**: a component that embeds rquickjs (QuickJS) and runs a
//! JavaScript bundle, exposing a `Process` global (and `Stream`) bridged to the
//! `rusm:runtime` actor world. A TypeScript app is just a Bun-bundled `.js` — the
//! runner is one shared, sandboxed, capability-gated wasm process per JS instance.
//!
//! Protocol: the runner's **first** message is the JS bundle (UTF-8 source);
//! everything after is the app's own mailbox, read via `Process.receive()`.
//!
//! Bridge: messages and stream chunks cross as **`Uint8Array`** (the actor model's
//! currency), with text convenience helpers (UTF-8 done in Rust, since QuickJS has
//! no `TextEncoder`). Pids cross as decimal strings (a `u64` doesn't fit a JS
//! number) and JS wraps them in `BigInt`; stream handles are small ints.
//!
//! Blocking JS "just works": `Process.receive()` / `stream.read()` call the host,
//! which suspends the whole instance's fiber until data arrives — no async needed.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    generate_all,
});

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{IsTerminal, Write};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use rquickjs::{Context, Ctx, Exception, Function, Promise, Runtime, TypedArray};
use rusm::runtime::actor;
use rusm::runtime::kv;
use rusm::runtime::pg;
use rusm::runtime::serve;
use rusm::runtime::streams as stream_iface;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
// Outbound `wasi:http` bindings come from wit-bindgen (the `process` world imports
// `wasi:http/outgoing-handler`), not the `wasip2` crate — so the guest is a core module
// `wizer` can pre-initialize. Same interface, same blocking-read streaming as before.
use wasi::http::types::{
    Fields, IncomingBody, IncomingResponse, Method, OutgoingBody, OutgoingRequest, Scheme,
};
use wasi::io::streams::InputStream;

struct Component;

// The `TypedArray`-taking/returning bridges are named generic fns (not closures):
// rquickjs needs `for<'js>` HRTB on the `Ctx`/`TypedArray` lifetime, which inline
// closures don't infer ("lifetime may not live long enough").
fn js_send(to: String, data: TypedArray<u8>) {
    actor::send(to.parse().unwrap_or(0), data.as_bytes().unwrap_or(&[]));
}
fn js_receive(ctx: Ctx<'_>) -> rquickjs::Result<TypedArray<'_, u8>> {
    TypedArray::new(ctx, actor::receive())
}
fn js_receive_timeout(ctx: Ctx<'_>, ms: f64) -> rquickjs::Result<Option<TypedArray<'_, u8>>> {
    match actor::receive_timeout(ms.max(0.0) as u64) {
        Some(bytes) => Ok(Some(TypedArray::new(ctx, bytes)?)),
        None => Ok(None),
    }
}
fn js_stream_write(h: f64, data: TypedArray<u8>) -> bool {
    stream_iface::stream_write(h as u64, data.as_bytes().unwrap_or(&[]))
}
fn js_stream_read(ctx: Ctx<'_>, h: f64) -> Option<TypedArray<'_, u8>> {
    stream_iface::stream_read(h as u64).map(|b| TypedArray::new(ctx, b).unwrap())
}
// Cryptographically-secure random bytes from the host (wasi:random) — the basis for
// the `crypto.getRandomValues` / `randomUUID` polyfill the web ecosystem assumes.
fn js_random_bytes(ctx: Ctx<'_>, len: f64) -> rquickjs::Result<TypedArray<'_, u8>> {
    let mut buf = vec![0u8; len.max(0.0) as usize];
    getrandom::fill(&mut buf).expect("host entropy (wasi:random)");
    TypedArray::new(ctx, buf)
}

/// A `fetch` response body still streaming to the guest: the wasi:io input-stream we
/// read chunks from, plus the `incoming-body`/`incoming-response` kept alive for as
/// long as the stream is (dropping them mid-read is a wasi:http error). Field order is
/// drop order — stream first, then body, then response.
struct FetchBody {
    stream: InputStream,
    _body: IncomingBody,
    _response: IncomingResponse,
}

thread_local! {
    /// Response bodies of in-flight `fetch`es, keyed by the handle handed to JS.
    static FETCH_BODIES: RefCell<HashMap<u64, FetchBody>> = RefCell::new(HashMap::new());
    static FETCH_NEXT: Cell<u64> = const { Cell::new(1) };
}

/// Split a URL into the wasi:http (scheme, authority, path-with-query) triple. Returns
/// `None` for a scheme we can't serve. Authority is host[:port]; path defaults to `/`.
fn split_url(url: &str) -> Option<(Scheme, String, String)> {
    let (scheme, rest) = match url.split_once("://") {
        Some(("http", rest)) => (Scheme::Http, rest),
        Some(("https", rest)) => (Scheme::Https, rest),
        Some((other, rest)) => (Scheme::Other(other.to_string()), rest),
        None => return None,
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    };
    Some((scheme, authority, path))
}

/// Perform an outbound HTTP request over raw **`wasi:http`** (capability-gated at the
/// host — a sandboxed guest is refused). Blocking, not a reactor: `pollable.block()`
/// suspends the fiber until the response head arrives, so the guest parks instead of
/// busy-polling. Returns the head as a flat list `[status, handle, k, v, …]`; the body
/// then streams via `__fetch_read(handle)`. A transport error throws into JS.
fn js_fetch(
    ctx: Ctx<'_>,
    method: String,
    url: String,
    headers: String,
    body: TypedArray<u8>,
) -> rquickjs::Result<Vec<String>> {
    let throw = |e: String| Exception::throw_message(&ctx, &e);
    let (scheme, authority, path) =
        split_url(&url).ok_or_else(|| throw(format!("fetch: unsupported URL {url:?}")))?;

    let fields = Fields::new();
    for line in headers.split('\n').filter(|l| !l.is_empty()) {
        if let Some((k, v)) = line.split_once(':') {
            let _ = fields.append(&k.trim().to_string(), v.trim().as_bytes());
        }
    }
    let req = OutgoingRequest::new(fields);
    let method = match method.as_str() {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        "OPTIONS" => Method::Options,
        other => Method::Other(other.to_string()),
    };
    req.set_method(&method)
        .map_err(|_| throw("fetch: bad method".into()))?;
    req.set_scheme(Some(&scheme))
        .map_err(|_| throw("fetch: bad scheme".into()))?;
    req.set_authority(Some(&authority))
        .map_err(|_| throw("fetch: bad authority".into()))?;
    req.set_path_with_query(Some(&path))
        .map_err(|_| throw("fetch: bad path".into()))?;

    // Canonical wasi:http order: take the body handle, **dispatch** the request, then
    // stream the body into it, then finish. Dispatching first is required — a POST body
    // written before `handle` never reaches the server (the request goes out empty).
    let out_body = req
        .body()
        .map_err(|_| throw("fetch: no request body".into()))?;
    let future = wasi::http::outgoing_handler::handle(req, None)
        .map_err(|e| throw(format!("fetch: {e:?}")))?;
    let payload = body.as_bytes().unwrap_or(&[]);
    if !payload.is_empty() {
        let stream = out_body
            .write()
            .map_err(|_| throw("fetch: no body stream".into()))?;
        for chunk in payload.chunks(4096) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|e| throw(format!("fetch: write body: {e:?}")))?;
        }
        drop(stream); // release the borrow before finishing the body
    }
    OutgoingBody::finish(out_body, None)
        .map_err(|e| throw(format!("fetch: finish body: {e:?}")))?;

    // Park the fiber until the response head is ready — no busy poll.
    future.subscribe().block();
    let resp = match future.get() {
        Some(Ok(Ok(resp))) => resp,
        Some(Ok(Err(code))) => return Err(throw(format!("fetch: {code:?}"))),
        _ => return Err(throw("fetch: response unavailable".into())),
    };

    let handle = FETCH_NEXT.with(|c| {
        let h = c.get();
        c.set(h + 1);
        h
    });
    let mut head = vec![resp.status().to_string(), handle.to_string()];
    for (name, value) in resp.headers().entries() {
        head.push(name);
        head.push(String::from_utf8_lossy(&value).into_owned());
    }
    let incoming = resp
        .consume()
        .map_err(|_| throw("fetch: consume body".into()))?;
    let stream = incoming
        .stream()
        .map_err(|_| throw("fetch: body stream".into()))?;
    FETCH_BODIES.with(|m| {
        m.borrow_mut().insert(
            handle,
            FetchBody {
                stream,
                _body: incoming,
                _response: resp,
            },
        )
    });
    Ok(head)
}

/// Read the next body chunk of a `fetch` response, or `None` at end-of-stream. Exactly
/// **one** `blocking_read` per call — it suspends the fiber until the host has a chunk
/// (or EOF), so the guest parks; there is no loop, so it cannot busy-spin by
/// construction. JS calls this repeatedly until it gets `null`. A non-empty chunk is
/// returned (and the body retained for the next read); EOF (`Closed`), any error, or a
/// spec-impossible empty read all end the stream — dropping `body` releases the
/// stream + connection.
fn js_fetch_read(ctx: Ctx<'_>, handle: f64) -> rquickjs::Result<Option<TypedArray<'_, u8>>> {
    let key = handle as u64;
    let Some(body) = FETCH_BODIES.with(|m| m.borrow_mut().remove(&key)) else {
        return Ok(None);
    };
    match body.stream.blocking_read(64 * 1024) {
        Ok(chunk) if !chunk.is_empty() => {
            FETCH_BODIES.with(|m| m.borrow_mut().insert(key, body));
            Ok(Some(TypedArray::new(ctx, chunk)?))
        }
        _ => Ok(None),
    }
}

/// Abandon a `fetch` response body (abort / GC), releasing its connection.
fn js_fetch_close(handle: f64) {
    FETCH_BODIES.with(|m| {
        m.borrow_mut().remove(&(handle as u64));
    });
}

// Durable key-value storage over the `kv-*` actor ABI (gated by `storage`). A
// denied/failed op throws into JS (the `kv.js` bridge surfaces these as a thrown
// Error); `get` returns `undefined` (→ null in JS) when the key is absent.
fn js_kv_get(
    ctx: Ctx<'_>,
    bucket: String,
    key: String,
) -> rquickjs::Result<Option<TypedArray<'_, u8>>> {
    match kv::get(&bucket, &key) {
        Ok(Some(bytes)) => Ok(Some(TypedArray::new(ctx, bytes)?)),
        Ok(None) => Ok(None),
        Err(e) => Err(Exception::throw_message(&ctx, &e)),
    }
}
fn js_kv_set(
    ctx: Ctx<'_>,
    bucket: String,
    key: String,
    value: TypedArray<u8>,
) -> rquickjs::Result<()> {
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
// Spawn a registered component by name; a denied/unknown spawn throws into JS
// (surfacing the host's error message) rather than returning a sentinel.
fn js_spawn(ctx: Ctx<'_>, name: String) -> rquickjs::Result<String> {
    match actor::spawn(&name) {
        Ok(pid) => Ok(pid.to_string()),
        Err(e) => Err(Exception::throw_message(&ctx, &e)),
    }
}
// Spawn a dynamic JS instance of a runner template `name`, loading its bundle at runtime
// from `source` (`inline:<js>` / `kv:<bucket>/<key>` / `url:`/`http(s)://…`). The JS runs
// under the template's declared profile; a denial/failure throws the host's message.
fn js_spawn_from(ctx: Ctx<'_>, name: String, source: String) -> rquickjs::Result<String> {
    match actor::spawn_from(&name, &source) {
        Ok(pid) => Ok(pid.to_string()),
        Err(e) => Err(Exception::throw_message(&ctx, &e)),
    }
}

// ---------------------------------------------------------------------------
// crypto.subtle — native (RustCrypto) digest / HMAC / AES-GCM, backing the
// `crypto.subtle` polyfill in webapi.js. Compiled + constant-time (far faster and
// safer than a pure-JS impl in QuickJS). Inputs are borrowed (no copy); only the
// owned result is allocated. Unsupported algorithms surface as `None` → the JS
// bridge throws, matching Web Crypto's NotSupportedError/OperationError.
// ---------------------------------------------------------------------------

fn hmac_sign(hash: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    macro_rules! sign {
        ($h:ty) => {{
            let mut mac = <Hmac<$h> as Mac>::new_from_slice(key).ok()?;
            mac.update(data);
            Some(mac.finalize().into_bytes().to_vec())
        }};
    }
    match hash {
        "SHA-1" => sign!(Sha1),
        "SHA-256" => sign!(Sha256),
        "SHA-384" => sign!(Sha384),
        "SHA-512" => sign!(Sha512),
        _ => None,
    }
}

fn hmac_verify(hash: &str, key: &[u8], sig: &[u8], data: &[u8]) -> Option<bool> {
    macro_rules! verify {
        ($h:ty) => {{
            let mut mac = <Hmac<$h> as Mac>::new_from_slice(key).ok()?;
            mac.update(data);
            // `verify_slice` is constant-time — never a byte-by-byte tag compare.
            Some(mac.verify_slice(sig).is_ok())
        }};
    }
    match hash {
        "SHA-1" => verify!(Sha1),
        "SHA-256" => verify!(Sha256),
        "SHA-384" => verify!(Sha384),
        "SHA-512" => verify!(Sha512),
        _ => None,
    }
}

/// AES-GCM with a 96-bit (12-byte) nonce — the Web Crypto standard. `aad` may be
/// empty. Output is `ciphertext || 16-byte tag` (the Web Crypto layout). `None` for
/// an unsupported key length (only 128/256-bit) or a non-12-byte IV.
fn aes_gcm_encrypt(key: &[u8], iv: &[u8], aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    if iv.len() != 12 {
        return None;
    }
    let payload = Payload {
        msg: plaintext,
        aad,
    };
    match key.len() {
        16 => Aes128Gcm::new_from_slice(key)
            .ok()?
            .encrypt(Nonce::from_slice(iv), payload)
            .ok(),
        32 => Aes256Gcm::new_from_slice(key)
            .ok()?
            .encrypt(Nonce::from_slice(iv), payload)
            .ok(),
        _ => None,
    }
}

/// Inverse of [`aes_gcm_encrypt`]. `None` on a bad key/IV length **or** an
/// authentication failure (tampered ciphertext/tag/aad) — the JS bridge throws.
fn aes_gcm_decrypt(key: &[u8], iv: &[u8], aad: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
    if iv.len() != 12 {
        return None;
    }
    let payload = Payload {
        msg: ciphertext,
        aad,
    };
    match key.len() {
        16 => Aes128Gcm::new_from_slice(key)
            .ok()?
            .decrypt(Nonce::from_slice(iv), payload)
            .ok(),
        32 => Aes256Gcm::new_from_slice(key)
            .ok()?
            .decrypt(Nonce::from_slice(iv), payload)
            .ok(),
        _ => None,
    }
}

fn js_crypto_digest<'a>(
    ctx: Ctx<'a>,
    alg: String,
    data: TypedArray<'a, u8>,
) -> rquickjs::Result<TypedArray<'a, u8>> {
    let data = data.as_bytes().unwrap_or(&[]);
    let out = match alg.as_str() {
        "SHA-1" => Sha1::digest(data).to_vec(),
        "SHA-256" => Sha256::digest(data).to_vec(),
        "SHA-384" => Sha384::digest(data).to_vec(),
        "SHA-512" => Sha512::digest(data).to_vec(),
        other => {
            return Err(Exception::throw_message(
                &ctx,
                &format!("unsupported digest algorithm: {other}"),
            ))
        }
    };
    TypedArray::new(ctx, out)
}

fn js_crypto_hmac_sign<'a>(
    ctx: Ctx<'a>,
    hash: String,
    key: TypedArray<'a, u8>,
    data: TypedArray<'a, u8>,
) -> rquickjs::Result<TypedArray<'a, u8>> {
    match hmac_sign(
        &hash,
        key.as_bytes().unwrap_or(&[]),
        data.as_bytes().unwrap_or(&[]),
    ) {
        Some(mac) => TypedArray::new(ctx, mac),
        None => Err(Exception::throw_message(
            &ctx,
            &format!("unsupported HMAC hash: {hash}"),
        )),
    }
}

fn js_crypto_hmac_verify(
    ctx: Ctx<'_>,
    hash: String,
    key: TypedArray<u8>,
    sig: TypedArray<u8>,
    data: TypedArray<u8>,
) -> rquickjs::Result<bool> {
    hmac_verify(
        &hash,
        key.as_bytes().unwrap_or(&[]),
        sig.as_bytes().unwrap_or(&[]),
        data.as_bytes().unwrap_or(&[]),
    )
    .ok_or_else(|| Exception::throw_message(&ctx, &format!("unsupported HMAC hash: {hash}")))
}

fn js_crypto_aes_gcm_encrypt<'a>(
    ctx: Ctx<'a>,
    key: TypedArray<'a, u8>,
    iv: TypedArray<'a, u8>,
    aad: TypedArray<'a, u8>,
    plaintext: TypedArray<'a, u8>,
) -> rquickjs::Result<TypedArray<'a, u8>> {
    match aes_gcm_encrypt(
        key.as_bytes().unwrap_or(&[]),
        iv.as_bytes().unwrap_or(&[]),
        aad.as_bytes().unwrap_or(&[]),
        plaintext.as_bytes().unwrap_or(&[]),
    ) {
        Some(ct) => TypedArray::new(ctx, ct),
        None => Err(Exception::throw_message(
            &ctx,
            "AES-GCM encrypt failed (key must be 16 or 32 bytes, iv 12 bytes)",
        )),
    }
}

fn js_crypto_aes_gcm_decrypt<'a>(
    ctx: Ctx<'a>,
    key: TypedArray<'a, u8>,
    iv: TypedArray<'a, u8>,
    aad: TypedArray<'a, u8>,
    ciphertext: TypedArray<'a, u8>,
) -> rquickjs::Result<TypedArray<'a, u8>> {
    match aes_gcm_decrypt(
        key.as_bytes().unwrap_or(&[]),
        iv.as_bytes().unwrap_or(&[]),
        aad.as_bytes().unwrap_or(&[]),
        ciphertext.as_bytes().unwrap_or(&[]),
    ) {
        Some(pt) => TypedArray::new(ctx, pt),
        None => Err(Exception::throw_message(
            &ctx,
            "AES-GCM decrypt failed (bad key/iv or authentication)",
        )),
    }
}

/// The guest JS environment, split by concern (see `bridge/`): Web API polyfills
/// (standards-only) then the `Process`/`Stream` actor API (over the host `__*`
/// primitives). Both are evaluated before the user's bundle.
const LOG_JS: &str = include_str!("../bridge/log.js");
const WEBAPI_JS: &str = include_str!("../bridge/webapi.js");
const STREAM_JS: &str = include_str!("../bridge/streams.js");
const ACTOR_JS: &str = include_str!("../bridge/actor.js");
const PG_JS: &str = include_str!("../bridge/pg.js");
const SERVE_JS: &str = include_str!("../bridge/serve.js");
const KV_JS: &str = include_str!("../bridge/kv.js");
const RPC_JS: &str = include_str!("../bridge/rpc.js");

thread_local! {
    /// The **warm** QuickJS context — engine + the full bridge (host `__*` primitives +
    /// webapi/process/kv/rpc), booted once. Baked into the image by [`wizer_initialize`]
    /// at build time so each spawned instance CoW-starts warm; reused for its lifetime.
    /// The `Context` owns its `Runtime`, so holding it here keeps the engine alive.
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

/// Wizer's build-time entry (`./build.sh` runs `wizer --init-func wizer_initialize`): boot
/// the engine + bridge so the snapshot captures a warm context. Not part of the component
/// interface — a plain core export wizer calls, then `wasm-tools component new` drops. The
/// bridge eval is pure JS that never calls a host import, so it is wizer-safe.
#[no_mangle]
pub extern "C" fn wizer_initialize() {
    with_warm(|_| {});
}

/// Boot the full JS bridge into a fresh context: bind the host `__*` primitives, then eval
/// the Web-API polyfills, the actor `Process`/`Stream` API, KV, and the RPC/service layer,
/// plus an empty CommonJS module the per-instance bundle populates. This is the work wizer
/// bakes into the snapshot — done once, not per spawn. (The bound closures are Rust
/// closures; like the js-http-runner's `__print`, they survive the wizer snapshot.)
fn boot_bridge(ctx: Ctx<'_>) {
    let g = ctx.globals();
    // Each closure is its own type, so a macro (not a helper fn/closure) is
    // what lets us register them uniformly.
    macro_rules! def {
        ($name:expr, $func:expr) => {
            g.set($name, Function::new(ctx.clone(), $func).unwrap())
                .unwrap();
        };
    }

    // --- process / messaging ---
    def!("__own_pid", || actor::own_pid().to_string());
    // The per-connection serving context (ws/sse handlers); JSON for the bridge to parse,
    // or `null` for any other process. Serialised here so header/param values escape safely.
    def!("__connection", || serve::connection().map(|c| {
        serde_json::json!({
            "method": c.method,
            "path": c.path,
            "query": c.query,
            "params": c.params,
            "headers": c.headers,
            "remoteAddr": c.remote_addr,
            "subprotocol": c.subprotocol,
        })
        .to_string()
    }));
    // Send a text WebSocket frame (binary frames go through __send to the writer pid).
    def!("__ws_send_text", |s: String| serve::ws_send_text(s.as_bytes()));
    // Close the WebSocket connection with a status code + reason (JS numbers are f64).
    def!("__ws_close", |code: f64, reason: String| serve::ws_close(code as u16, &reason));
    // Emit a rich SSE event: data + optional event/id (empty string = none) + retry (0 = none).
    def!(
        "__sse_send",
        |data: String, event: String, id: String, retry: f64| {
            let event = (!event.is_empty()).then_some(event);
            let id = (!id.is_empty()).then_some(id);
            let retry = (retry > 0.0).then_some(retry as u32);
            serve::sse_send(data.as_bytes(), event.as_deref(), id.as_deref(), retry)
        }
    );
    def!("__list", || actor::list_processes()
        .into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>());
    def!("__send_text", |to: String, s: String| actor::send(
        to.parse().unwrap_or(0),
        s.as_bytes()
    ));
    def!("__send", js_send);
    def!("__receive", js_receive);
    def!("__receive_text", || String::from_utf8(actor::receive())
        .unwrap_or_default());
    // `receive … after`: the next message, or `undefined` on timeout.
    def!("__receive_timeout", js_receive_timeout);
    def!("__register", |n: String| actor::register(&n));
    def!("__whereis", |n: String| actor::whereis(&n)
        .map(|p| p.to_string())
        .unwrap_or_default());
    def!("__is_alive", |p: String| actor::is_alive(
        p.parse().unwrap_or(0)
    ));
    def!("__kill", |p: String| actor::kill(p.parse().unwrap_or(0)));
    def!("__set_label", |l: String| actor::set_label(&l));
    // Process-group tags (Erlang `pg`): self-tag (unprivileged); `__kill_tag` is gated at
    // the host by process-control. Pids cross as decimal strings, like `__list`.
    def!("__register_tag", |t: String| pg::register_tag(&t));
    def!("__unregister_tag", |t: String| pg::unregister_tag(&t));
    def!("__whereis_tag", |t: String| pg::whereis_tag(&t)
        .into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>());
    def!("__kill_tag", |t: String| pg::kill_tag(&t) as f64);
    def!("__spawn", js_spawn);
    def!("__spawnFrom", js_spawn_from);
    def!("__monitor", |p: String| actor::monitor(
        p.parse().unwrap_or(0)
    ));
    // Delegate guest `supervise` to the host's single native supervisor.
    def!("__supervise", |strategy: String,
                         children: Vec<String>,
                         max_restarts: f64,
                         within_ms: f64| {
        let strategy = match strategy.as_str() {
            "one_for_all" => actor::SuperviseStrategy::OneForAll,
            "rest_for_one" => actor::SuperviseStrategy::RestForOne,
            _ => actor::SuperviseStrategy::OneForOne,
        };
        let _ = actor::supervise(strategy, &children, max_restarts as u32, within_ms as u32);
    });

    // --- streams (handles are small ints carried as JS numbers) ---
    def!("__stream_open", |to: String| stream_iface::stream_open(
        to.parse().unwrap_or(0)
    )
    .map_or(-1.0, |h| h as f64));
    def!("__stream_write", js_stream_write);
    def!("__stream_write_text", |h: f64, s: String| {
        stream_iface::stream_write(h as u64, s.as_bytes())
    });
    def!("__stream_close", |h: f64| stream_iface::stream_close(h as u64));
    def!("__stream_accept", || stream_iface::stream_accept() as f64);
    def!("__stream_read", js_stream_read);
    // console output → WASI stderr (shown only if the `inherit_stdio`
    // capability is granted; discarded for a sandboxed guest). One `write_all` of
    // the whole line (not `eprintln!`'s piece-by-piece writes) so concurrent guest
    // processes sharing the host stderr can't interleave mid-line.
    def!("__print", |s: String| {
        let _ = std::io::stderr().write_all(format!("{s}\n").as_bytes());
    });
    // console.* → the platform logger: the host stamps the time, this process's
    // component name + pid, and the severity colour (gated by the node `[log]
    // level`). The guest passes only a severity + the joined message; no name,
    // pid, or format wiring (webapi.js maps the console methods to a level).
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
    // Whether stderr is a terminal — lets a TS logger colour only when piping
    // wouldn't litter escape codes, matching the host's platform-log gating.
    def!("__isatty", || std::io::stderr().is_terminal());
    // Secure randomness for the `crypto` polyfill (webapi.js).
    def!("__random_bytes", js_random_bytes);
    // Outbound HTTP for the `fetch` polyfill (capability-gated at the host).
    def!("__fetch", js_fetch);
    def!("__fetch_read", js_fetch_read);
    def!("__fetch_close", js_fetch_close);
    // Durable key-value storage for the `kv` polyfill (capability-gated).
    def!("__kv_get", js_kv_get);
    def!("__kv_set", js_kv_set);
    def!("__kv_delete", js_kv_delete);
    def!("__kv_exists", js_kv_exists);
    def!("__kv_list", js_kv_list);
    // Native crypto primitives for the `crypto.subtle` polyfill (webapi.js).
    def!("__crypto_digest", js_crypto_digest);
    def!("__crypto_hmac_sign", js_crypto_hmac_sign);
    def!("__crypto_hmac_verify", js_crypto_hmac_verify);
    def!("__crypto_aes_gcm_encrypt", js_crypto_aes_gcm_encrypt);
    def!("__crypto_aes_gcm_decrypt", js_crypto_aes_gcm_decrypt);

    // Capability-granted environment variables: `std::env::var` reads
    // `wasi:cli/environment`, which the host populates from this process's
    // capability `env = [...]` grants — so a guest sees only its granted keys
    // (an ungranted/absent key is `null`). Surfaced to JS as `process.env`.
    def!("__getenv", |key: String| std::env::var(key).ok());

    // Web API polyfills, the raw actor API, durable storage, then the
    // RPC/service layer.
    let _: () = ctx.eval(LOG_JS).unwrap();
    let _: () = ctx.eval(WEBAPI_JS).unwrap();
    let _: () = ctx.eval(STREAM_JS).unwrap();
    let _: () = ctx.eval(ACTOR_JS).unwrap();
    let _: () = ctx.eval(PG_JS).unwrap();
    let _: () = ctx.eval(SERVE_JS).unwrap();
    let _: () = ctx.eval(KV_JS).unwrap();
    let _: () = ctx.eval(RPC_JS).unwrap();
    // A CommonJS surface so a Bun-bundled (`--format=cjs`) service/worker can
    // populate `module.exports`; a bare script just ignores it. Empty in the
    // snapshot — each per-instance bundle populates it in `run`.
    let _: () = ctx
        .eval("globalThis.module={exports:{}};globalThis.exports=module.exports;")
        .unwrap();
}

impl Guest for Component {
    fn run() {
        // First message is the JS bundle: either raw JS source, or `rusm-jsc`
        // precompiled QuickJS bytecode (prefixed with the `QJSB` magic). Kept as raw
        // bytes so bytecode (not UTF-8) survives.
        let bundle_bytes = actor::receive();
        with_warm(|ctx| {
            // The user's bundle, in a CommonJS module scope. Wrapping in a function
            // keeps its top-level `var`s (e.g. a bundled `var spawn`) out of the
            // global object, where a classic eval would leak them and clobber the
            // runner's globals (Process/spawn/…). A bare script runs now; a
            // service/worker registers its exports for __rusm_entry to drive.
            //
            // Two delivery forms: precompiled **bytecode** (`QJSB` magic — skip the
            // parser, load the module + eval; the IIFE wrapper was applied at build
            // time by `rusm-jsc`), or raw **source** (wrap + eval as before). Both end
            // up populating `globalThis.module.exports` identically.
            const BYTECODE_MAGIC: &[u8] = b"QJSB";
            if let Some(bytecode) = bundle_bytes.strip_prefix(BYTECODE_MAGIC) {
                // SAFETY: bytecode is produced by `rusm-jsc` with the same rquickjs
                // version (0.9.0) the runner embeds; it is a valid module object.
                let module = unsafe { rquickjs::Module::load(ctx.clone(), bytecode) }.unwrap();
                let (_m, promise) = module.eval().unwrap();
                let _ = promise.finish::<()>();
            } else {
                let bundle = String::from_utf8(bundle_bytes).unwrap_or_default();
                let wrapped = format!(
                    "(function(module,exports){{\n{bundle}\n}})(globalThis.module,globalThis.module.exports);"
                );
                let _: () = ctx.eval(wrapped).unwrap();
            }
            // The host sets a serving role (e.g. "http") via the `RUSM_SERVE_ROLE`
            // env capability for a resident server; `__rusm_entry` reads it to pick
            // the serving loop. Absent for ordinary services/workers.
            let role = std::env::var("RUSM_SERVE_ROLE").unwrap_or_default();
            // `{:?}` quotes/escapes the (host-controlled) role as a JS string literal.
            let _: () = ctx
                .eval(format!("globalThis.__rusm_role={role:?};"))
                .unwrap();
            // Drive the entry point (service dispatch / worker `default`) to
            // completion. finish() pumps the QuickJS job queue; a long-running
            // service blocks here (each receive suspends the fiber) until killed.
            let entry: Function = ctx.globals().get("__rusm_entry").unwrap();
            let outcome: Promise = entry.call(()).unwrap();
            let _ = outcome.finish::<()>();
        });
    }
}

export!(Component);
