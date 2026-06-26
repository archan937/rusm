//! WASI bridges: per-version glue from a Wasm artifact to a `rusm-otp` process,
//! over the shared core in [`crate`] (engine, epoch ticker, pooling allocator).
//!
//! A bridge differs only in *artifact kind* (core module vs component) and which
//! WASI version it wires; the engine, preemption and pooling are shared. Keeping
//! each version in its own file keeps `lib.rs` lean (the project's file-splitting
//! convention) and makes "add a WASI version" a local change.

pub(crate) mod access;
pub(crate) mod actor;
pub(crate) mod auth;
pub(crate) mod compress;
pub(crate) mod conn;
pub(crate) mod http;
pub(crate) mod kv;
pub(crate) mod log;
pub(crate) mod pg;
pub(crate) mod routed;
pub(crate) mod serve;
pub(crate) mod sse;
pub(crate) mod streams;
pub(crate) mod tls;
pub(crate) mod wasip1;
pub(crate) mod wasip2;
pub(crate) mod wasip3;
pub(crate) mod ws;
pub(crate) mod ws_codec;

use std::collections::HashMap;
use std::sync::Arc;

use rusm_otp::{Context, Runtime, StreamHandle, StreamWriter};
use wasmtime::ResourceLimiter;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{
    default_send_request, HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
};
use wasmtime_wasi_http::WasiHttpCtx;

use crate::caps::Capabilities;
use crate::Spawner;

/// Store data for **component** guests (wasip2 today, wasip3 later): the WASI
/// context + resource table the component model needs, a per-process memory
/// ceiling enforced as a `ResourceLimiter`, and the actor handles (pid, runtime,
/// mailbox) that back the `rusm:runtime` host ABI. One host type serves both WASI
/// versions, since both `add_to_linker` entry points only require [`WasiView`].
///
/// Public as [`crate::BridgeHost`] so a **custom application bridge** can
/// `impl my_iface::Host for BridgeHost` and reach the calling process; the fields
/// stay crate-private, so a bridge sees only the curated accessors below.
pub struct WasiHost {
    pub(crate) wasi: WasiCtx,
    pub(crate) table: ResourceTable,
    /// `wasi:http` host context, for serving a component as an HTTP handler
    /// (Phase 11). Idle for non-HTTP guests.
    pub(crate) http: WasiHttpCtx,
    /// Capability gate for *outbound* `wasi:http` (a guest's `fetch`): denies the
    /// request unless this process was granted network access.
    pub(crate) http_hooks: HttpCaps,
    /// The owning process's pid (for `own-pid`, `register`, `set-label`).
    pub(crate) pid: u64,
    /// The HTTP connection context, when this process is a per-connection WebSocket/SSE
    /// handler the serving bridge spawned for one accepted connection; `None` for every
    /// other process. Backs the `connection` actor op (read once in the handler's `open`).
    pub(crate) connection: Option<crate::bridges::serve::ConnectionInfo>,
    /// A WebSocket handler's **control** channel to its connection's writer — backs the
    /// `ws-send-text` and `ws-close` ops (binary frames use the plain `send` path). Bounded,
    /// so a slow client back-pressures the handler. `None` for SSE / non-connection processes.
    pub(crate) ws_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::WsOut>>,
    /// An SSE handler's **rich-event** channel to its writer — backs the `sse-send` op
    /// (the plain `data:` path is a `send` to the writer pid). Bounded. `None` for WS /
    /// non-connection processes.
    pub(crate) sse_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::SseEvent>>,
    /// This process's capabilities: the source of truth for its memory ceiling,
    /// whether it may control other processes, whether it may spawn, and the
    /// ceiling any child it spawns inherits (a child is never broader).
    pub(crate) caps: Capabilities,
    /// Handle to the actor runtime, backing the actor host functions.
    pub(crate) rt: Runtime,
    /// The process's mailbox, for `receive`. `None` only for a bare host built
    /// outside a spawned process (e.g. direct inspection in a test); a running
    /// guest always has one.
    pub(crate) ctx: Option<Context>,
    /// The shared spawn core, so this process may `spawn` registered components.
    /// `None` only for a bare host built outside the runtime (a test).
    pub(crate) spawner: Option<Arc<Spawner>>,
    /// Byte streams this process is **writing** to others, keyed by the handle
    /// returned to the guest by `stream-open`.
    pub(crate) out_streams: HashMap<u64, StreamWriter>,
    /// Byte streams this process has **accepted** and is reading, keyed by the
    /// handle returned by `stream-accept`.
    pub(crate) in_streams: HashMap<u64, StreamHandle>,
    /// Monotonic handle source for this process's streams.
    pub(crate) next_stream: u64,
    /// Pending timers scheduled by `send-after`, keyed by the handle returned to
    /// the guest. Cancelled via `cancel-timer`; auto-dropped when the process exits,
    /// aborting any unfired timers (the `TimerRef` drop impl cancels the task).
    pub(crate) timers: HashMap<u64, rusm_otp::TimerRef>,
    /// Monotonic handle source for this process's timers.
    pub(crate) next_timer: u64,
    /// This process's **host-only claims context** (e.g. an `app_id` an auth hook
    /// derived from a validated token). Seeded at spawn (inherited from the parent) and,
    /// for a resident, refreshed per received message; read by host bridge code via
    /// [`WasiHost::context`]. **No guest op reaches it** — the per-tenant-bridge security
    /// rests on that. Each process owns its own, accessed serially (an instance never runs
    /// reentrantly), so it needs no lock.
    pub(crate) context: crate::context::ProcessContext,
}

/// The curated, **public** surface a custom application bridge reaches the calling
/// process through (the struct's fields stay crate-private). Deliberately minimal:
/// process identity, the runtime handle, and the process's capability grants — the
/// same primitives the built-in bridges build on.
impl WasiHost {
    /// The pid of the process making the call. A bridge stamps it onto outbound
    /// work, sends a reply to it, or scopes per-process state by it.
    pub fn pid(&self) -> u64 {
        self.pid
    }

    /// The actor runtime, so a bridge can `send`/`spawn`/consult the registry —
    /// i.e. integrate a custom capability with the process model, the way `actor`
    /// and `pg` do.
    pub fn runtime(&self) -> &Runtime {
        &self.rt
    }

    /// This process's capability grants. A custom bridge gates itself **default-deny**
    /// off these, exactly like every built-in capability (`storage`, `network`, …).
    pub fn caps(&self) -> &Capabilities {
        &self.caps
    }

    /// This process's **host-only claims context** — identity an auth hook attached to the
    /// request (e.g. `self.context().get("app_id")`), inherited by every process it spawns
    /// and carried on the messages it sends. A multi-tenant bridge reads it to act for the
    /// right tenant. It is host-authoritative: no guest op can read, write, or forge it, so
    /// a guest cannot claim an identity the operator didn't establish.
    pub fn context(&self) -> &crate::context::ProcessContext {
        &self.context
    }

    /// Mutable access to this process's claims context, for host code that derives or caches
    /// a claim (e.g. a bridge memoising a minted per-tenant token). Host-only, like
    /// [`context`](Self::context) — there is no guest path to it.
    pub fn context_mut(&mut self) -> &mut crate::context::ProcessContext {
        &mut self.context
    }

    /// Send `message` to `to` **carrying this process's claims context** as opaque mailbox
    /// metadata, so the recipient (and any bridge *it* calls) acts for the same tenant. A
    /// generated TS/Go bridge delegation shim uses this to forward the caller's context to its
    /// resident runner — which is what lets a bridge that calls another bridge keep the tenant.
    /// Host-only: the context is taken from this process, never from guest input. The empty
    /// context sends no metadata (the zero-cost path).
    pub fn send_with_context(&self, to: rusm_otp::Pid, message: Vec<u8>) {
        self.rt
            .send_with_meta(to, message, self.context.clone().into_meta());
    }

    /// Selective receive for a **bridge round-trip**: waits for a message whose bytes
    /// start with `tag` (a per-call unique prefix), parking unrelated messages in the
    /// save queue for later receives. Used by generated delegation shims for TS-hosted
    /// bridges — unrelated messages arriving in flight are saved and replayed by the
    /// next `receive`, so the process mailbox is never silently drained.
    ///
    /// Coverage: `recv_match` is tested by `rusm-otp`; the delegation path is exercised
    /// by the custom-bridge e2e tests.
    pub async fn recv_bridge_reply(&mut self, tag: &str) -> Option<Vec<u8>> {
        use rusm_otp::Received;
        let ctx = self.ctx.as_mut()?;
        let got = ctx
            .recv_match(|m| matches!(m, Received::Message(b) if b.starts_with(tag.as_bytes())))
            .await;
        match got {
            Received::Message(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Perform a full TS/Go custom-bridge round-trip to the resident `runner` actor and return
    /// the reply payload JSON (the `callId:` tag stripped), or `None` if the runner isn't
    /// registered or never answers. The **single place** the bridge wire protocol lives — the
    /// generated delegation shim only (de)serializes around this call.
    ///
    /// Builds the request envelope (`fn`, JSON-array `args_json`, this caller's claims
    /// `context`, a `replyTo` address), sends it carrying the context as metadata (so a bridge
    /// the runner calls keeps the tenant), and awaits the tagged reply. The reply address
    /// adapts to the caller: a process **with** a mailbox (`ctx`) replies to itself and
    /// selective-receives (no extra process — the actor/WebSocket hot path); a **mailbox-less**
    /// caller — a `wasi:http` per-request instance — routes the reply through a one-shot
    /// **responder** process, so an HTTP/SSE handler calls a custom bridge exactly like any
    /// other guest.
    pub async fn call_bridge(
        &mut self,
        runner: &str,
        fn_name: &str,
        args_json: &str,
    ) -> Option<Vec<u8>> {
        self.call_bridge_with(runner, fn_name, args_json, BRIDGE_CALL_TIMEOUT)
            .await
    }

    /// The core of [`call_bridge`], parameterised by `timeout` so a test can drive the
    /// fail-soft-on-wedged-runner path without waiting the production backstop.
    async fn call_bridge_with(
        &mut self,
        runner: &str,
        fn_name: &str,
        args_json: &str,
        timeout: std::time::Duration,
    ) -> Option<Vec<u8>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        let pid = self.runtime().whereis(runner)?;
        static CALL_CTR: AtomicU64 = AtomicU64::new(0);
        let call_id = format!(
            "rusm-bridge:{}-{}",
            self.pid(),
            CALL_CTR.fetch_add(1, Ordering::Relaxed)
        );
        let ctx_json = serde_json::to_string(self.context()).unwrap_or_else(|_| "{}".to_string());
        let tag = format!("{call_id}:");
        let strip = |raw: Vec<u8>| raw.strip_prefix(tag.as_bytes()).unwrap_or(&raw).to_vec();

        if self.ctx.is_some() {
            // The caller owns a mailbox: reply to it, selective-receive (parks unrelated mail).
            let req = bridge_envelope(fn_name, args_json, &ctx_json, self.pid(), &call_id);
            self.send_with_context(pid, req);
            // Bounded so a wedged runner can't hang the caller forever (fail-soft → the shim's
            // default). `recv_bridge_reply` is cancel-safe, so a timeout leaves the mailbox intact.
            tokio::time::timeout(timeout, self.recv_bridge_reply(&tag))
                .await
                .ok()
                .flatten()
                .map(strip)
        } else {
            // No mailbox (a `wasi:http` per-request instance): route the reply through a
            // one-shot responder process, so the round-trip works without a host process.
            let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
            let responder = self.rt.spawn(move |mut ctx| async move {
                if let rusm_otp::Received::Message(bytes) = ctx.recv().await {
                    let _ = tx.send(bytes);
                }
            });
            let req = bridge_envelope(
                fn_name,
                args_json,
                &ctx_json,
                responder.pid().raw(),
                &call_id,
            );
            self.send_with_context(pid, req);
            // Bounded; on timeout the responder is still reaped, so a wedged runner can't leak a
            // process per stuck call.
            let reply = tokio::time::timeout(timeout, rx)
                .await
                .ok()
                .and_then(|r| r.ok());
            responder.kill();
            reply.map(strip)
        }
    }
}

/// A backstop on a single bridge round-trip: long enough never to sever a legitimately slow
/// bridge (a host op doing real I/O), short enough that a *wedged* runner can't hang the caller
/// — or, on the mailbox-less `wasi:http` path, leak a responder process — forever. A fail-soft
/// bound (the call returns `None` → the shim's default), not a latency budget; a per-request
/// wall-clock deadline (roadmap) will subsume it for serving.
const BRIDGE_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Build the JSON request envelope a TS/Go bridge runner expects:
/// `{"fn":…,"args":…,"context":…,"replyTo":{"pid":…,"callId":…}}`. Assembled by concatenation
/// (string literals, not a `format!` over literal JSON braces) so the already-JSON `args_json`
/// and `ctx_json` embed verbatim. The one source for this shape (see [`WasiHost::call_bridge`]).
fn bridge_envelope(
    fn_name: &str,
    args_json: &str,
    ctx_json: &str,
    reply_pid: u64,
    call_id: &str,
) -> Vec<u8> {
    let mut s = String::from("{\"fn\":\"");
    s.push_str(fn_name);
    s.push_str("\",\"args\":");
    s.push_str(args_json);
    s.push_str(",\"context\":");
    s.push_str(ctx_json);
    s.push_str(",\"replyTo\":{\"pid\":\"");
    s.push_str(&reply_pid.to_string());
    s.push_str("\",\"callId\":\"");
    s.push_str(call_id);
    s.push_str("\"}}");
    s.into_bytes()
}

impl WasiView for WasiHost {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for WasiHost {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

/// Capability gate for outbound `wasi:http`: refuses the request at the host unless
/// the process was granted network access (default-deny). A refused request fails
/// with `HttpRequestDenied` — the guest's `fetch` rejects cleanly, no host trap.
pub(crate) struct HttpCaps {
    pub(crate) allow_network: bool,
}

impl WasiHttpHooks for HttpCaps {
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        if !self.allow_network {
            return Ok(HostFutureIncomingResponse::ready(Ok(Err(
                ErrorCode::HttpRequestDenied,
            ))));
        }
        Ok(default_send_request(request, config))
    }
}

impl ResourceLimiter for WasiHost {
    /// Denies growth past the capability's memory ceiling — `memory.grow` then
    /// returns -1 to the guest (no host trap), the standard sandbox signal.
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.caps.memory_limit())
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::component::Resource;
    use wasmtime_wasi::WasiCtxBuilder;

    /// A bare `WasiHost` (no running process) for exercising host logic directly.
    fn bare_host(caps: Capabilities) -> WasiHost {
        WasiHost {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            http: WasiHttpCtx::new(),
            http_hooks: HttpCaps {
                allow_network: false,
            },
            pid: 0,
            connection: None,
            ws_out: None,
            sse_out: None,
            caps,
            rt: Runtime::new(),
            ctx: None,
            spawner: None,
            out_streams: HashMap::new(),
            in_streams: HashMap::new(),
            next_stream: 0,
            timers: HashMap::new(),
            next_timer: 0,
            context: crate::context::ProcessContext::new(),
        }
    }

    #[test]
    fn the_claims_context_is_host_readable_and_writable_and_starts_empty() {
        // A bridge reaches the calling process's host-only claims context through the
        // curated surface: empty by default, settable host-side, read back by the bridge.
        let mut host = bare_host(Capabilities::nothing());
        assert!(host.context().is_empty());
        assert_eq!(host.context().get("app_id"), None);
        host.context_mut().set("app_id", "acme");
        assert_eq!(host.context().get("app_id"), Some("acme"));
    }

    #[test]
    fn wasi_view_exposes_a_live_table() {
        let mut host = bare_host(Capabilities::nothing());
        // The table reached through the view is the real one: a pushed resource
        // round-trips through it.
        let view = host.ctx();
        let handle: Resource<u32> = view.table.push(7u32).unwrap();
        assert_eq!(*view.table.get(&handle).unwrap(), 7);
    }

    /// A mock TS/Go bridge runner registered as `bridge:double`: it echoes the exact wire
    /// protocol the generated runners speak — `"<callId>:<json>"` — doubling `args[0]` and
    /// reflecting the forwarded claims `context.app_id`, so a test can prove both the round-trip
    /// and that the caller's context reached the runner.
    fn spawn_doubling_runner(rt: &Runtime) {
        let replier = rt.clone();
        let runner = rt.spawn(move |mut ctx| async move {
            loop {
                let Some(bytes) = ctx.recv().await.message() else {
                    continue;
                };
                let env: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let pid: u64 = env["replyTo"]["pid"].as_str().unwrap().parse().unwrap();
                let call_id = env["replyTo"]["callId"].as_str().unwrap();
                let n = env["args"][0].as_i64().unwrap();
                let tenant = env["context"]["app_id"].as_str().unwrap_or("none");
                replier.send(
                    rusm_otp::Pid::from_raw(pid),
                    format!("{call_id}:{{\"n\":{},\"tenant\":\"{tenant}\"}}", n * 2).into_bytes(),
                );
            }
        });
        rt.register("bridge:double", runner.pid());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_bridge_round_trips_without_a_mailbox() {
        // The `wasi:http` per-request path has no process mailbox (`ctx: None`); a TS/Go custom
        // bridge call must still round-trip — via the one-shot responder. Prove a mailbox-less
        // host gets the resident runner's reply (payload, `callId:` tag stripped), and that an
        // unregistered runner yields `None` (the fail-soft default the shim turns into a default).
        let mut host = bare_host(Capabilities::nothing());
        assert_eq!(host.call_bridge("bridge:missing", "f", "[]").await, None);
        // The caller's host-only claims context must reach the runner in-band (the multi-tenant
        // seam) — set one and have the runner echo it back so we prove it's forwarded.
        host.context_mut().set("app_id", "acme");
        spawn_doubling_runner(host.runtime());

        let reply = host.call_bridge("bridge:double", "double", "[21]").await;
        assert_eq!(
            reply.as_deref(),
            Some(br#"{"n":42,"tenant":"acme"}"#.as_slice()),
            "the reply round-trips and the runner saw the forwarded claims context"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_bridge_uses_the_caller_mailbox_when_present() {
        // The actor/WebSocket path: a caller WITH a mailbox replies to itself and
        // selective-receives — no responder spawn. Run `call_bridge` from inside a real process
        // so `self.ctx` is that process's mailbox (and `self.rt` the same runtime as the runner).
        let rt = Runtime::new();
        spawn_doubling_runner(&rt);
        let host_rt = rt.clone();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        rt.spawn(move |ctx| async move {
            let mut host = bare_host(Capabilities::nothing());
            host.rt = host_rt;
            host.pid = ctx.pid().raw();
            host.context.set("app_id", "globex");
            host.ctx = Some(ctx);
            let reply = host.call_bridge("bridge:double", "double", "[5]").await;
            let _ = done_tx.send(reply);
        });
        assert_eq!(
            done_rx.await.unwrap().as_deref(),
            Some(br#"{"n":10,"tenant":"globex"}"#.as_slice()),
            "the mailbox branch round-trips and forwards the context"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_bridge_fails_soft_when_the_runner_is_wedged() {
        // A registered-but-silent runner must not hang the caller (nor, on the responder path,
        // leak a process): the bounded round-trip returns `None` after the timeout, promptly.
        let mut host = bare_host(Capabilities::nothing());
        let rt = host.runtime().clone();
        let wedged = rt.spawn(|mut ctx| async move {
            loop {
                let _ = ctx.recv().await; // receive the request, never reply
            }
        });
        rt.register("bridge:double", wedged.pid());

        let started = std::time::Instant::now();
        let reply = host
            .call_bridge_with(
                "bridge:double",
                "double",
                "[1]",
                std::time::Duration::from_millis(50),
            )
            .await;
        assert_eq!(reply, None, "a wedged runner fails soft to None");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "it returned at the timeout, not the production backstop"
        );
    }

    #[test]
    fn the_memory_limiter_enforces_exactly_the_capability_ceiling() {
        // RUSM's *sole* responsibility for guest memory growth is this decision: permit a
        // grow whose new size is within the cap, deny one past it. Whether the OS can then
        // physically commit the permitted pages is the OS's concern, not RUSM's — so the
        // policy is tested here, at its boundary, deterministically, never through a real
        // grow (a real grow also depends on the OS committing, which is environmental and
        // would make this flaky under load — see `a_memory_cap_crashes_a_component_that_
        // grows_past_it` for the end-to-end *rejection* path, which needs no commit).
        let mut host = bare_host(Capabilities::nothing().max_memory(256 << 10));
        // Within the cap → permitted (one 64 KiB page growing to 192 KiB total).
        assert!(host.memory_growing(64 << 10, 192 << 10, None).unwrap());
        // Exactly at the cap → permitted (the boundary is inclusive).
        assert!(host.memory_growing(64 << 10, 256 << 10, None).unwrap());
        // One byte past the cap → denied; the guest then sees `memory.grow` return -1.
        assert!(!host
            .memory_growing(64 << 10, (256 << 10) + 1, None)
            .unwrap());
    }
}
