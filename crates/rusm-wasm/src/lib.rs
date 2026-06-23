//! `rusm-wasm` — the Wasmtime backend for RUSM.
//!
//! Each process runs as an **isolated Wasm instance**: [`WasmRuntime::spawn`]
//! starts a fresh instance as a [`rusm_otp`] process (one instance = one process
//! = one Tokio task). A guest trap becomes a process crash (so links/monitors
//! fire), and an **epoch** ticker preempts even an infinite-loop guest so it
//! yields cooperatively and stays killable — the BEAM's reduction counting, done
//! with Wasmtime epochs.
//!
//! Wasm lives *only* here; `rusm-otp` never references Wasmtime.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use rusm_otp::{ProcessHandle, Runtime};
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy, Linker, Module, PoolingAllocationConfig,
};

mod bindings;
mod bridges;
mod bundle_cache;
mod caps;

pub use bridges::http::{HttpServer, PreparedHttp};
pub use bridges::routed::{Resolver, Routed, RoutedHttpServer};
pub use bridges::sse::SseServer;
pub use bridges::tls::{tls_acceptor, TlsAcceptor};
pub use bridges::wasip1::PreparedModule;
pub use bridges::wasip2::PreparedComponent;
pub use bridges::ws::{serve_ws_echo, WsServer};
pub use caps::{Capabilities, CapabilityProfile};

/// The host state every bridge's `Host` impl is written against — including a
/// **custom application bridge**. An app's own `bridges/<name>/host.rs` does
/// `impl my_iface::Host for rusm_wasm::BridgeHost { … }` and reaches the calling
/// process through [`BridgeHost`]'s public accessors ([`pid`](BridgeHost::pid) /
/// [`runtime`](BridgeHost::runtime) / [`caps`](BridgeHost::caps)). The struct's
/// fields stay private: a bridge gets process identity, the runtime handle, and its
/// capabilities — the same surface the built-in bridges use, nothing more. Register
/// it with [`WasmRuntime::with_bridges`].
pub use bridges::WasiHost as BridgeHost;

/// Re-exported so a custom bridge's `bindgen!` lowers against the **exact** Wasmtime
/// the runtime links. Typed host calls must resolve against one `Linker` type, so an
/// app authoring a bridge writes `rusm_wasm::wasmtime::component::bindgen!{…}` rather
/// than depending on Wasmtime directly (a version skew would fail to type-check).
pub use wasmtime;

/// Re-exported for **generated TS/Go bridge delegation shims**: the host crate only
/// needs `rusm_wasm` as a dep, not a separate `serde_json` in its `Cargo.toml`.
pub use serde_json;

/// The component linker a custom bridge extends inside [`WasmRuntime::with_bridges`].
/// A bridge calls its generated `add_to_linker(linker, |h| h)` against it.
pub type BridgeLinker = wasmtime::component::Linker<BridgeHost>;

/// A custom-bridge linker extension: it adds typed host functions to the component linker
/// (a bridge's generated `add_to_linker`), run once at construction on every engine tier.
/// The owned form held by [`WasmRuntimeBuilder`]; the public entry points
/// ([`WasmRuntime::with_bridges`], [`WasmRuntimeBuilder::bridges`]) take it as `impl Fn`.
pub type BridgeExtension = dyn Fn(&mut BridgeLinker) -> wasmtime::Result<()>;

/// How often the epoch is bumped. A guest runs at most this long before it must
/// yield to the scheduler, so tight loops can't starve other processes.
const EPOCH_TICK: Duration = Duration::from_millis(10);
/// Default pool slots: the most Wasm instances that may be live at once. The
/// pooling allocator pre-reserves their memory slabs so a spawn is a slab reuse,
/// not an mmap — the lever that makes instance-per-process cheap. The reservation
/// (`max_instances` × `max_memory`) is **lazy, copy-on-write virtual address
/// space** (e.g. 1024 × 16 MiB = 16 GiB virtual), so real RSS scales only with
/// *live* instances. A busy node raises this via [`WasmRuntime::with_limits`].
/// (A true "millions of Wasm processes" tier needs an on-demand fallback above
/// the pool — see the roadmap.)
pub const DEFAULT_MAX_INSTANCES: u32 = 1024;
/// Default per-instance linear-memory ceiling (virtual, copy-on-write). Sized for
/// real components (a minimal Rust component needs ~1 MiB; the rquickjs js-runner
/// a few); a per-process capability `StoreLimiter` caps usage *below* this.
pub const DEFAULT_MAX_MEMORY: usize = 16 << 20;
/// The prebuilt rquickjs **js-runner** component (rusm-ts), embedded so a TS/JS
/// bundle can be spawned with no extra artifacts. Built from `js-runner/` with
/// wasi-sdk (QuickJS is C); the artifact is checked in under `runtimes/` (a plain
/// dir, not the nested `js-runner/` crate, so it ships in the published package).
/// See `js-runner/Cargo.toml` to regenerate.
const JS_RUNNER_WASM: &[u8] = include_bytes!("../runtimes/js_runner.wasm");

/// The prebuilt rquickjs **js-http-runner** (`wasi:http`) component (rusm-ts),
/// embedded so a TS/JS `fetch` handler can be served with no extra artifacts. Built
/// from `js-http-runner/` with wasi-sdk; checked in under `runtimes/`. See
/// `js-http-runner/Cargo.toml` to regenerate.
const JS_HTTP_RUNNER_WASM: &[u8] = include_bytes!("../runtimes/js_http_runner.wasm");

/// Counters shared by every instance of one [`WasmRuntime`], so host functions
/// can report aggregate activity (e.g. guest progress for the fairness scenario).
#[derive(Default)]
pub(crate) struct Counters {
    /// Total `notify` host-function calls across all guests.
    pub(crate) notifications: AtomicU64,
}

/// A **dynamic runner template** kind — a registered component whose code is supplied per
/// spawn from a runtime source (`spawn-from`), not fixed at registration. The loaded code
/// runs under the template's declared `caps` (operator policy — the guest picks the code,
/// never the capabilities). `spawn`-by-name is rejected for a template; only `spawn-from`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DynamicKind {
    /// A JS runner template: the prepared component is the shared js-runner; the source is a
    /// JS bundle delivered as message 1 (the runner's protocol).
    Js,
    /// A WASM runner template: the source is a `.wasm` component, compiled + prepared (cached
    /// by content hash) at spawn — cold once, hot thereafter on the pooled fast path.
    Wasm,
}

/// A component registered for spawn-by-name: the prepared component plus an optional first
/// message to deliver on spawn. For a **TS service** the prepared component is the shared
/// js-runner and `bundle` is the JS source (replayed as message 1, the runner's protocol);
/// for a Rust component `bundle` is `None`; for a dynamic template see [`DynamicKind`].
#[derive(Clone)]
pub(crate) struct Registered {
    /// The prepared component to instantiate on spawn. `None` only for a [`DynamicKind::Wasm`]
    /// template, whose component is compiled per-source at spawn (see the dynamic-WASM cache).
    pub(crate) prepared: Option<PreparedComponent>,
    /// `Arc` so a lookup clone is cheap; the bytes copy once, on the actual send.
    pub(crate) bundle: Option<Arc<Vec<u8>>>,
    /// The component's **declared** capability profile. When set, a guest `spawn`-by-name
    /// runs the child under *this* profile (the node operator's explicit per-component
    /// policy — what the manifest declares is what runs), instead of inheriting the
    /// spawner's caps. `None` → inherit the spawner's caps (ad-hoc registration / tests).
    pub(crate) caps: Option<Capabilities>,
    /// `Some(kind)` marks a **dynamic runner template** (JS or WASM): code supplied per spawn
    /// via `spawn-from`. `None` is an ordinary fixed component.
    pub(crate) dynamic: Option<DynamicKind>,
}

/// Resolves a `url:` `spawn-from` source to JS bundle bytes. **Injected by the
/// orchestrator** (the node owns network egress + policy), so `rusm-wasm` needs no HTTP
/// client. Async — a one-shot fetch returning the bundle or a human-readable error.
pub type BundleResolver = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send>> + Send + Sync,
>;

/// Compiles + prepares a fetched dynamic-`.wasm` bundle into a [`PreparedComponent`]. Injected
/// at construction (it captures the engine + the component linker + the overflow tier), so the
/// dynamic-WASM `spawn-from` path compiles a runtime bundle without reaching back into the
/// `WasmRuntime` — no `Arc` cycle. Sync: `Component::new` + `instantiate_pre` are CPU-bound.
pub(crate) type WasmPreparer =
    Arc<dyn Fn(&[u8]) -> Result<PreparedComponent, String> + Send + Sync>;

/// Default freshness/idle window for the dynamic-WASM compile cache: how long a fetched
/// bundle is reused for a source before the source is re-checked, and how long a compiled
/// artifact survives without use. Generous by default; tune per node via the builder.
pub(crate) const DEFAULT_DYNAMIC_TTL: Duration = Duration::from_secs(300);

/// The spawn core shared between the [`WasmRuntime`] and every **running guest** — held
/// behind an `Arc` so a component's host context can spawn siblings without a back-reference
/// to the whole runtime. It carries what a spawn needs (the engine, the process runtime, a
/// name → prepared-component registry) plus the dynamic-WASM compile cache + preparer, keeping
/// the per-spawn path lean.
pub(crate) struct Spawner {
    pub(crate) engine: Engine,
    pub(crate) rt: Runtime,
    /// Components registered by name so a guest may `spawn` them. Read-mostly
    /// (written when an app loads, read per guest-initiated spawn), so a plain
    /// `RwLock` — uncontended reads, no extra dependency.
    components: RwLock<HashMap<String, Registered>>,
    /// The **on-demand overflow engine** (opt-in — `None` by default). When the
    /// pooled tier is full, a spawn instantiates here instead, so the live count is
    /// bounded by memory rather than the fixed pool size. See
    /// [`WasmRuntime::with_overflow`].
    pub(crate) overflow: Option<Engine>,
    /// Live instances occupying the pooled tier (only tracked when `overflow` is
    /// set; the default path adds no atomic). A spawn reserves a slot here; if the
    /// pool is full it falls to the overflow engine.
    pub(crate) pooled_live: AtomicU32,
    /// The pooled-tier capacity (`pooled_live`'s ceiling).
    pub(crate) pooled_cap: u32,
    /// The node's durable key-value store (`None` unless built with
    /// [`WasmRuntime::with_store`]). Shared with every guest; the `kv-*` actor ABI
    /// reaches it, gated by the `storage` capability.
    pub(crate) store: Option<Arc<rusm_kv::Store>>,
    /// Resolver for `url:` `spawn-from` sources, set once by the orchestrator (see
    /// [`WasmRuntime::set_bundle_resolver`]). Empty by default → a `url:` source errors
    /// ("not configured"); `inline:`/`kv:` never need it.
    pub(crate) bundle_resolver: OnceLock<BundleResolver>,
    /// Compiles a fetched dynamic-`.wasm` bundle to a prepared component (see [`WasmPreparer`]).
    /// Injected once at construction; absent only on a bare spawner (never on a real runtime).
    pub(crate) wasm_preparer: OnceLock<WasmPreparer>,
    /// Content-addressed compile cache for dynamic-WASM `spawn-from`: the first spawn of a
    /// bundle compiles (cold), every later spawn reuses the prepared component on the pooled
    /// fast path (hot). See [`crate::bundle_cache`].
    pub(crate) wasm_cache: bundle_cache::BundleCache<PreparedComponent>,
}

impl Spawner {
    /// Registers a component under `name` so guests may `spawn` it by that name.
    pub(crate) fn register(&self, name: impl Into<String>, entry: Registered) {
        self.components
            .write()
            .expect("component registry is never poisoned")
            .insert(name.into(), entry);
    }

    /// Looks up a registered component by name (cloned out so no lock is held
    /// across the spawn — both fields are `Arc`-backed, so this is cheap).
    pub(crate) fn lookup(&self, name: &str) -> Option<Registered> {
        self.components
            .read()
            .expect("component registry is never poisoned")
            .get(name)
            .cloned()
    }

    /// Fetch a dynamic bundle's bytes from a runtime `source` — `inline:<bytes>`,
    /// `kv:<bucket>/<key>` (the node store), or `url:`/`http(s)://…` (the injected resolver).
    /// Pure I/O; the **caller gates the capability** for the scheme before calling.
    pub(crate) async fn fetch_bundle(&self, source: &str) -> Result<Vec<u8>, String> {
        let source = source.trim();
        if let Some(inline) = source.strip_prefix("inline:") {
            return Ok(inline.as_bytes().to_vec());
        }
        if let Some(rest) = source.strip_prefix("kv:") {
            let (bucket, key) = rest
                .split_once('/')
                .ok_or_else(|| format!("kv source must be `kv:<bucket>/<key>`, got {source:?}"))?;
            let store = self
                .store
                .as_ref()
                .ok_or_else(|| format!("kv source {source:?} needs a node store"))?;
            return store
                .bucket(bucket)
                .get(key)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("bundle not found at {source}"));
        }
        let url = source.strip_prefix("url:").unwrap_or(source);
        if url.starts_with("http://") || url.starts_with("https://") {
            let resolver = self
                .bundle_resolver
                .get()
                .cloned()
                .ok_or("url: bundle sources are not configured on this node")?;
            return resolver(url.to_string()).await;
        }
        Err(format!(
            "unrecognised bundle source {source:?} (expected `inline:<…>`, `kv:<bucket>/<key>`, or `http(s)://…`)"
        ))
    }

    /// Resolve a dynamic-WASM `source` to a prepared component through the content-addressed
    /// cache: the first spawn of a bundle compiles (cold), every later spawn reuses the prepared
    /// component on the pooled fast path (hot). The caller must have gated the capability for the
    /// source's scheme first.
    pub(crate) async fn dynamic_wasm(
        &self,
        source: &str,
    ) -> Result<Arc<PreparedComponent>, String> {
        let preparer = self
            .wasm_preparer
            .get()
            .cloned()
            .ok_or("dynamic WASM is not configured on this node")?;
        self.wasm_cache
            .get(
                source,
                || self.fetch_bundle(source),
                move |bytes| preparer(bytes),
            )
            .await
    }
}

/// Runs Wasm guests as RUSM processes.
pub struct WasmRuntime {
    /// The spawn core (engine + process runtime + component registry), shared with
    /// running guests so they can spawn by name. The runtime reads the engine and
    /// process runtime through it rather than duplicating those handles.
    spawner: Arc<Spawner>,
    /// The core-module linker (preview1 WASI + the raw `rusm::*` actor ABI). Built
    /// once; a module's imports resolve into a `PreparedModule` at `prepare`, never
    /// per spawn. The component counterpart is `component_linker`.
    linker: Linker<bridges::wasip1::CoreHost>,
    /// The component-model counterpart of `linker`, with WASI wired in. Used by
    /// the wasip2/p3 bridges to prepare and spawn components. Built once.
    /// `Arc`-shared so the dynamic-WASM preparer closure (injected into the spawner) can
    /// capture it to compile runtime bundles, without an `Arc` cycle back to the runtime.
    /// `Arc` auto-derefs, so `instantiate_pre` call sites are unchanged.
    component_linker: Arc<wasmtime::component::Linker<bridges::WasiHost>>,
    /// The component linker bound to the **overflow** engine (`None` unless built
    /// with [`with_overflow`](Self::with_overflow)); used to prepare a component's
    /// overflow `InstancePre` alongside its pooled one. `Arc`-shared (see above).
    pub(crate) overflow_component_linker:
        Option<Arc<wasmtime::component::Linker<bridges::WasiHost>>>,
    /// The prebuilt rquickjs **js-runner** component (for `spawn_js`), compiled +
    /// prepared lazily on first use so non-JS nodes pay nothing.
    js_runner: std::sync::OnceLock<PreparedComponent>,
    /// The prebuilt rquickjs **js-http-runner** `wasi:http` component (for
    /// `http_server_js`), compiled + prepared lazily on first use.
    js_http_runner: std::sync::OnceLock<PreparedHttp>,
    /// Per-app **js-runner override** (the [`WasmRuntimeBuilder::js_runner`] bytes): an app
    /// with TS guests that call custom bridges builds its own js-runner (the custom bridge
    /// imports + glue compiled in) and supplies it here; `None` uses the embedded runner.
    js_runner_wasm: Option<Vec<u8>>,
    /// Per-app **js-http-runner override** ([`WasmRuntimeBuilder::js_http_runner`]); `None`
    /// uses the embedded runner.
    js_http_runner_wasm: Option<Vec<u8>>,
    shared: Arc<Counters>,
    epoch_stop: Arc<AtomicBool>,
    epoch_ticker: Option<JoinHandle<()>>,
}

/// Composable construction for a [`WasmRuntime`] — created by [`WasmRuntime::builder`].
/// Set pool limits, attach a durable store, opt into the on-demand overflow tier, and/or
/// wire custom application bridges, in **any** combination, then [`build`](Self::build).
/// Every option defaults to off / the standard value, so `WasmRuntime::builder(rt).build()`
/// equals [`WasmRuntime::new`]. The fixed `WasmRuntime::with_*` constructors are shortcuts
/// for the single-option cases; reach for the builder when an app needs more than one
/// (e.g. a durable store **and** custom bridges, which no single constructor expresses).
pub struct WasmRuntimeBuilder {
    rt: Runtime,
    max_instances: u32,
    max_memory: usize,
    store: Option<std::path::PathBuf>,
    overflow: bool,
    bridges: Box<BridgeExtension>,
    js_runner_wasm: Option<Vec<u8>>,
    js_http_runner_wasm: Option<Vec<u8>>,
    dynamic_ttl: Duration,
}

impl WasmRuntimeBuilder {
    fn new(rt: Runtime) -> Self {
        Self {
            rt,
            max_instances: DEFAULT_MAX_INSTANCES,
            max_memory: DEFAULT_MAX_MEMORY,
            store: None,
            overflow: false,
            bridges: Box::new(|_| Ok(())),
            js_runner_wasm: None,
            js_http_runner_wasm: None,
            dynamic_ttl: DEFAULT_DYNAMIC_TTL,
        }
    }

    /// The freshness/idle window for the **dynamic-WASM** compile cache (`spawn-from` a `.wasm`
    /// source): how long a fetched bundle is reused for a source before the source is
    /// re-checked, and how long a compiled artifact survives without use. Default
    /// [`DEFAULT_DYNAMIC_TTL`]. Longer = fewer re-fetches/recompiles; shorter = faster
    /// live-redeploy pickup.
    pub fn dynamic_ttl(mut self, ttl: Duration) -> Self {
        self.dynamic_ttl = ttl;
        self
    }

    /// Pooled instance slots — the most Wasm processes live at once (default
    /// [`DEFAULT_MAX_INSTANCES`]). See [`WasmRuntime::with_limits`].
    pub fn max_instances(mut self, max_instances: u32) -> Self {
        self.max_instances = max_instances;
        self
    }

    /// Per-instance linear-memory ceiling (default [`DEFAULT_MAX_MEMORY`]).
    pub fn max_memory(mut self, max_memory: usize) -> Self {
        self.max_memory = max_memory;
        self
    }

    /// Attach a durable redb **key-value store** at `path` (created if absent), reached by
    /// guests granted `storage`. See [`WasmRuntime::with_store`].
    pub fn store(mut self, path: impl AsRef<std::path::Path>) -> Self {
        self.store = Some(path.as_ref().to_path_buf());
        self
    }

    /// Enable the **on-demand overflow tier** (off by default). See
    /// [`WasmRuntime::with_overflow`].
    pub fn overflow(mut self) -> Self {
        self.overflow = true;
        self
    }

    /// Wire **custom application bridges** into the component linker. `extend` runs after
    /// the built-in bridges, on every engine tier; inside it a bridge calls its generated
    /// `add_to_linker(linker, |h| h)` — a typed WIT host call, no dispatcher. The bridge's
    /// `impl <iface>::Host for` [`BridgeHost`] reaches the calling process through
    /// [`BridgeHost`]'s accessors. The same seam the built-in bridges use, exposed for
    /// application code — RUSM's compiled-in, typed answer to a wasmCloud capability
    /// provider. See `bridges/README.md`. Calling this again replaces the prior extension.
    pub fn bridges(
        mut self,
        extend: impl Fn(&mut BridgeLinker) -> wasmtime::Result<()> + 'static,
    ) -> Self {
        self.bridges = Box::new(extend);
        self
    }

    /// Use a **per-app js-runner** (TS/JS actor/service guests) instead of the embedded one —
    /// the runner an app rebuilds with its custom bridges' host imports + glue compiled in
    /// (`rusm build` produces it). A TS guest that imports no custom bridge needs no override.
    pub fn js_runner(mut self, wasm: impl Into<Vec<u8>>) -> Self {
        self.js_runner_wasm = Some(wasm.into());
        self
    }

    /// Use a **per-app js-http-runner** (TS `fetch` handlers) instead of the embedded one —
    /// the HTTP runner twin of [`js_runner`](Self::js_runner).
    pub fn js_http_runner(mut self, wasm: impl Into<Vec<u8>>) -> Self {
        self.js_http_runner_wasm = Some(wasm.into());
        self
    }

    /// Construct the [`WasmRuntime`]. Must run inside a Tokio runtime (it starts the epoch
    /// ticker).
    pub fn build(self) -> Result<WasmRuntime> {
        let engine = Engine::new(&WasmRuntime::pooled_config(
            self.max_instances,
            self.max_memory,
        ))?;
        // The overflow engine shares the compile config but uses the default (on-demand)
        // allocator — no fixed cap, so the live count is bounded by memory, not the pool.
        let overflow = if self.overflow {
            Some(Engine::new(&WasmRuntime::base_config())?)
        } else {
            None
        };
        let store = match self.store {
            Some(path) => Some(Arc::new(rusm_kv::Store::open(path)?)),
            None => None,
        };
        let mut runtime = WasmRuntime::assemble(
            self.rt,
            engine,
            overflow,
            self.max_instances,
            store,
            self.bridges.as_ref(),
            self.dynamic_ttl,
        )?;
        // Per-app JS-runner overrides (consumed lazily by `js_runner`/`js_http_runner`).
        runtime.js_runner_wasm = self.js_runner_wasm;
        runtime.js_http_runner_wasm = self.js_http_runner_wasm;
        Ok(runtime)
    }
}

impl WasmRuntime {
    /// Start a [`WasmRuntimeBuilder`] over an existing process [`Runtime`]. The
    /// composable construction path: set pool limits, attach a durable store, opt into
    /// the on-demand overflow tier, and/or wire custom application bridges — in **any**
    /// combination — then [`build`](WasmRuntimeBuilder::build). The `with_*` constructors
    /// below are thin shortcuts for the single-option common cases. Must run inside a
    /// Tokio runtime (build starts the epoch ticker).
    pub fn builder(rt: Runtime) -> WasmRuntimeBuilder {
        WasmRuntimeBuilder::new(rt)
    }

    /// Builds a backend over an existing process [`Runtime`], with the default pool
    /// limits ([`DEFAULT_MAX_INSTANCES`] live instances × [`DEFAULT_MAX_MEMORY`] each).
    /// Must run inside a Tokio runtime (it starts the epoch ticker).
    pub fn new(rt: Runtime) -> Result<Self> {
        Self::builder(rt).build()
    }

    /// Like [`new`](Self::new) but with explicit pool limits — raise
    /// `max_instances` for a node that hosts many concurrent Wasm processes (the
    /// reservation is lazy virtual memory; real RSS tracks live instances), and
    /// `max_memory` for components that need larger heaps.
    pub fn with_limits(rt: Runtime, max_instances: u32, max_memory: usize) -> Result<Self> {
        Self::builder(rt)
            .max_instances(max_instances)
            .max_memory(max_memory)
            .build()
    }

    /// Like [`new`](Self::new) but lets an application wire its own **custom bridges**
    /// into the component linker (see [`WasmRuntimeBuilder::bridges`]). For combining
    /// bridges with a store or other options, use [`builder`](Self::builder) directly.
    pub fn with_bridges(
        rt: Runtime,
        extend: impl Fn(&mut BridgeLinker) -> wasmtime::Result<()> + 'static,
    ) -> Result<Self> {
        Self::builder(rt).bridges(extend).build()
    }

    /// Like [`new`](Self::new) but with a durable **key-value store** at `path`
    /// (created if absent): guests granted the `storage` capability reach it through
    /// the actor ABI (`kv-*`). One embedded redb file the node owns — no external
    /// daemon. Uses the default pool limits; for other combinations use
    /// [`builder`](Self::builder).
    pub fn with_store(rt: Runtime, path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::builder(rt).store(path).build()
    }

    /// Like [`with_limits`](Self::with_limits) but adds an **on-demand overflow
    /// tier**: once the pooled `max_instances` are all live, further spawns
    /// instantiate on a second (on-demand) engine instead of failing — so the live
    /// Wasm-process count is bounded by available memory, not the fixed pool size.
    /// The pooled tier stays the fast path; overflow only engages past capacity.
    pub fn with_overflow(rt: Runtime, max_instances: u32, max_memory: usize) -> Result<Self> {
        Self::builder(rt)
            .max_instances(max_instances)
            .max_memory(max_memory)
            .overflow()
            .build()
    }

    /// The compile/runtime config shared by both engines: epoch interruption (the
    /// preemption lever), the async component model (WASI p3), and copy-on-write
    /// memory init.
    fn base_config() -> Config {
        let mut config = Config::new();
        config.epoch_interruption(true);
        // Fibers (always on in wasmtime 45) make a host async fn — `actor::receive`, a
        // `wasi:io` poll — **suspend** the guest's fiber and free the worker ("write
        // blocking code, get async"). The component-model-**async** ABI is deliberately
        // left OFF: with it on, a sync (p2) guest's `wasi:io` poll is routed through the
        // `concurrent` task machinery, which *busy-polls* instead of parking — it pegged
        // a core per in-flight streaming `fetch`. Off, the poll suspends the fiber. The
        // p3 interfaces in use (`wasi:random@0.3.0`) are synchronous and don't need it.
        config.wasm_component_model_async(false);
        config.memory_init_cow(true);
        config
    }

    /// [`base_config`](Self::base_config) plus the pooling allocator — pre-reserved
    /// instance slabs reused per spawn (the instance-per-process efficiency win).
    fn pooled_config(max_instances: u32, max_memory: usize) -> Config {
        let mut config = Self::base_config();
        let mut pool = PoolingAllocationConfig::default();
        pool.total_core_instances(max_instances);
        pool.total_memories(max_instances);
        pool.total_tables(max_instances);
        pool.max_memory_size(max_memory);
        // Component guests also draw a *component-instance* slot from the pool.
        pool.total_component_instances(max_instances);
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        config
    }

    fn assemble(
        rt: Runtime,
        engine: Engine,
        overflow: Option<Engine>,
        max_instances: u32,
        store: Option<Arc<rusm_kv::Store>>,
        extend: &BridgeExtension,
        dynamic_ttl: Duration,
    ) -> Result<Self> {
        let linker = bridges::wasip1::build_linker(&engine)?;
        // The component linker carries the built-in bridges; `extend` then wires any
        // custom application bridges on top — on *every* engine tier (pooled +
        // overflow), so an overflow-tier instance resolves the same custom imports.
        // `Arc`-shared so the dynamic-WASM preparer can capture it (no cycle to the runtime).
        let component_linker = Arc::new({
            let mut linker = bridges::wasip2::build_linker(&engine)?;
            extend(&mut linker)?;
            linker
        });
        let overflow_component_linker = match overflow.as_ref() {
            Some(engine) => {
                let mut linker = bridges::wasip2::build_linker(engine)?;
                extend(&mut linker)?;
                Some(Arc::new(linker))
            }
            None => None,
        };

        // The dynamic-WASM preparer: compiles a fetched `.wasm` bundle to a prepared component
        // against the same linker(s). Captures clones of the engine + linkers (not the runtime),
        // so it can run from the spawner's `spawn-from` path without an `Arc` cycle.
        let wasm_preparer: WasmPreparer = {
            let engine = engine.clone();
            let linker = Arc::clone(&component_linker);
            let overflow_engine = overflow.clone();
            let overflow_linker = overflow_component_linker.clone();
            Arc::new(move |bytes: &[u8]| {
                let component = wasmtime::component::Component::new(&engine, bytes)
                    .map_err(|e| format!("compiling dynamic WASM component: {e}"))?;
                let overflow = match (&overflow_engine, &overflow_linker) {
                    (Some(e), Some(l)) => Some((e, l.as_ref())),
                    _ => None,
                };
                bridges::wasip2::prepare_component_with(&linker, overflow, &component, "run")
                    .map_err(|e| format!("preparing dynamic WASM component: {e}"))
            })
        };
        let wasm_preparer_cell = OnceLock::new();
        let _ = wasm_preparer_cell.set(wasm_preparer);

        // Bump the epoch on a cadence — on a **dedicated OS thread**, not a Tokio
        // task. The whole point is to preempt guests that are pinning the Tokio
        // workers; a ticker that needed a worker itself would starve exactly when
        // it's needed (and deadlock once every worker runs a tight-loop guest).
        // Every engine (pooled + overflow) is ticked, so overflow guests are
        // preempted too.
        let mut ticker_engines = vec![engine.clone()];
        ticker_engines.extend(overflow.clone());
        let epoch_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&epoch_stop);
        let epoch_ticker = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(EPOCH_TICK);
                for engine in &ticker_engines {
                    engine.increment_epoch();
                }
            }
        });

        Ok(Self {
            spawner: Arc::new(Spawner {
                engine,
                rt,
                components: RwLock::new(HashMap::new()),
                overflow,
                pooled_live: AtomicU32::new(0),
                pooled_cap: max_instances,
                store,
                bundle_resolver: OnceLock::new(),
                wasm_preparer: wasm_preparer_cell,
                wasm_cache: bundle_cache::BundleCache::new(dynamic_ttl),
            }),
            linker,
            component_linker,
            overflow_component_linker,
            js_runner: std::sync::OnceLock::new(),
            js_http_runner: std::sync::OnceLock::new(),
            js_runner_wasm: None,
            js_http_runner_wasm: None,
            shared: Arc::new(Counters::default()),
            epoch_stop,
            epoch_ticker: Some(epoch_ticker),
        })
    }

    /// The node's durable key-value store, if one was configured
    /// ([`with_store`](Self::with_store)). The app loader uses it to read e.g. a
    /// kv-sourced component bundle; it's the **same** store running guests reach via
    /// the `kv` ABI (a cheap `Arc` clone), so reads see their writes.
    pub fn store(&self) -> Option<rusm_kv::Store> {
        self.spawner.store.as_deref().cloned()
    }

    /// Registers a prepared component under `name` so a **running guest** may
    /// `spawn` it by that name through the actor ABI (capability-gated). The child
    /// inherits the spawner's caps (non-escalating) — use
    /// [`register_component_with`](Self::register_component_with) to run it under an
    /// explicit declared profile regardless of who spawns it.
    pub fn register_component(&self, name: impl Into<String>, prepared: PreparedComponent) {
        self.spawner.register(
            name,
            Registered {
                prepared: Some(prepared),
                bundle: None,
                caps: None,
                dynamic: None,
            },
        );
    }

    /// Like [`register_component`](Self::register_component) but pins the component's
    /// **declared** capability profile: a guest `spawn`-by-name runs the child under
    /// `caps` (the node operator's explicit per-component policy — what the manifest
    /// declares is what runs), not the spawner's inherited caps. The app loader uses
    /// this so each `[components.<name>]`/serve handler runs under its own profile.
    pub fn register_component_with(
        &self,
        name: impl Into<String>,
        prepared: PreparedComponent,
        caps: Capabilities,
    ) {
        self.spawner.register(
            name,
            Registered {
                prepared: Some(prepared),
                bundle: None,
                caps: Some(caps),
                dynamic: None,
            },
        );
    }

    /// Registers a **TypeScript service** under `name`: a guest `spawn`ing it gets a
    /// fresh js-runner instance fed this Bun-built bundle (the runner's protocol).
    /// The child inherits the spawner's caps; use
    /// [`register_js_component_with`](Self::register_js_component_with) to pin a profile.
    pub fn register_js_component(&self, name: impl Into<String>, bundle: impl Into<Vec<u8>>) {
        let prepared = self.js_runner().clone();
        self.spawner.register(
            name,
            Registered {
                prepared: Some(prepared),
                bundle: Some(Arc::new(bundle.into())),
                caps: None,
                dynamic: None,
            },
        );
    }

    /// Like [`register_js_component`](Self::register_js_component) but pins the TS
    /// component's **declared** capability profile (see
    /// [`register_component_with`](Self::register_component_with)).
    pub fn register_js_component_with(
        &self,
        name: impl Into<String>,
        bundle: impl Into<Vec<u8>>,
        caps: Capabilities,
    ) {
        let prepared = self.js_runner().clone();
        self.spawner.register(
            name,
            Registered {
                prepared: Some(prepared),
                bundle: Some(Arc::new(bundle.into())),
                caps: Some(caps),
                dynamic: None,
            },
        );
    }

    /// Registers a **dynamic JS runner template** under `name`: a capability profile with
    /// no fixed bundle. A guest cannot `spawn` it (there's nothing to run); it runs only
    /// via `spawn-from(name, source)`, which fetches a JS bundle from a runtime source and
    /// runs it on the js-runner under `caps` — the operator's declared profile (the guest
    /// chooses the code, never the capabilities). The app loader registers these for a
    /// `[components.<name>]` marked `dynamic = "js"`.
    pub fn register_js_template(&self, name: impl Into<String>, caps: Capabilities) {
        let prepared = self.js_runner().clone();
        self.spawner.register(
            name,
            Registered {
                prepared: Some(prepared),
                bundle: None,
                caps: Some(caps),
                dynamic: Some(DynamicKind::Js),
            },
        );
    }

    /// Registers a **dynamic WASM runner template** under `name`: a capability profile with no
    /// fixed component. A guest cannot `spawn` it; it runs only via `spawn-from(name, source)`,
    /// which fetches a `.wasm` component from a runtime source (`kv:`/`url:`), compiles +
    /// prepares it (cached by content hash — cold once, hot thereafter on the pooled fast
    /// path), and instantiates it under `caps` (the operator's declared profile — the guest
    /// chooses the code, never the capabilities). The app loader registers these for a
    /// `[components.<name>]` marked `dynamic = "wasm"`.
    pub fn register_wasm_template(&self, name: impl Into<String>, caps: Capabilities) {
        self.spawner.register(
            name,
            Registered {
                prepared: None,
                bundle: None,
                caps: Some(caps),
                dynamic: Some(DynamicKind::Wasm),
            },
        );
    }

    /// Set the resolver for `url:` `spawn-from` sources — a network fetch the orchestrator
    /// owns, so `rusm-wasm` stays HTTP-free. Set-once (later calls are ignored); `inline:`
    /// and `kv:` sources never need it.
    pub fn set_bundle_resolver(&self, resolver: BundleResolver) {
        let _ = self.spawner.bundle_resolver.set(resolver);
    }

    /// Record a **named** component the app loader spawned directly (not via the
    /// by-name guest path) in the [platform lifecycle log](rusm_otp::LogLevel): label
    /// it so its `exit` line can name it, and emit its `spawn` line at `Debug`. No-op
    /// unless logging is on. Use after `spawn_component_with` / `spawn_js_with` for a
    /// `[[serve]]` / `[components.<name>]` entry.
    pub fn note_spawn(&self, handle: &ProcessHandle, name: &str, caps: &Capabilities) {
        if self.spawner.rt.wants_log(rusm_otp::LogLevel::Error) {
            self.spawner.record_spawn(handle.pid(), name, caps);
        }
    }

    /// Set the [platform lifecycle log](rusm_otp::LogLevel) level (spawn / exit, from
    /// `rusm.toml [log] level`). Off by default; set once at startup.
    pub fn set_log_level(&self, level: rusm_otp::LogLevel) {
        self.spawner.rt.set_log_level(level);
    }

    /// Spawn a component registered by `name` under its declared profile — the host
    /// twin of the guest `spawn` ABI. `None` if no component is registered under
    /// `name`. Used to (re)start a resident service.
    pub fn spawn_registered(&self, name: &str) -> Option<ProcessHandle> {
        self.spawner.spawn_registered(name)
    }

    /// Boot-spawn and **supervise** the named (already-registered) components as
    /// long-lived services: a one-for-one supervisor (re)starts each under its
    /// declared profile via [`spawn_registered`](Self::spawn_registered), bounded by
    /// the runtime's restart-intensity (3 restarts / 5 s) so a crash-loop is capped,
    /// not infinite. Returns the supervisor handle — hold it to keep the tree alive
    /// (and to tear it down). It stops abnormally only if a service blows the budget.
    /// `names` empty → no supervisor is started and `None` is returned.
    pub fn supervise(&self, names: &[String]) -> Option<ProcessHandle> {
        if names.is_empty() {
            return None;
        }
        let mut sup = self.spawner.rt.supervisor(rusm_otp::Strategy::OneForOne);
        for name in names {
            let spawner = Arc::clone(&self.spawner);
            let name = name.clone();
            sup = sup.child(move |_rt| {
                spawner
                    .spawn_registered(&name)
                    .expect("a supervised component stays registered for its lifetime")
            });
        }
        Some(sup.start())
    }

    /// Aborts **every** process on this runtime, returning how many were stopped —
    /// the kill-switch that makes teardown deterministic. A guest process (a Wasm
    /// instance) ending here drops its `Store`, freeing its pooled-allocator slot, so
    /// after this the instance pool is fully reclaimed. Dropping a `WasmRuntime` that
    /// owns its runtime should call this first, so long-lived/parked processes (e.g. a
    /// per-connection handler) never leak past the thing that spawned them.
    pub fn shutdown(&self) -> usize {
        self.spawner.rt.shutdown()
    }

    /// Spawns a **JavaScript/TypeScript** bundle as a sandboxed process via the
    /// embedded rquickjs js-runner (default-deny `Sandboxed`). The bundle is the
    /// Bun-built JS source; it runs with the `Process` actor API + Web API
    /// polyfills. Use [`spawn_js_with`](Self::spawn_js_with) to grant capabilities.
    pub fn spawn_js(&self, bundle: impl Into<Vec<u8>>) -> ProcessHandle {
        self.spawn_js_with(bundle, CapabilityProfile::Sandboxed.capabilities())
    }

    /// Like [`spawn_js`](Self::spawn_js) but under explicit [`Capabilities`]. The
    /// js-runner component is compiled + prepared once (lazily) and shared across
    /// all JS processes; each spawn is a fresh, isolated instance fed the bundle.
    pub fn spawn_js_with(&self, bundle: impl Into<Vec<u8>>, caps: Capabilities) -> ProcessHandle {
        let handle = self.spawn_component_with(self.js_runner(), caps);
        // The runner's protocol: its first message is the JS bundle to execute.
        self.spawner.rt.send(handle.pid(), bundle.into());
        handle
    }

    /// The shared, embedded rquickjs js-runner — compiled + prepared once (lazily)
    /// so non-JS nodes pay nothing. Backs `spawn_js` and TS-service registration.
    fn js_runner(&self) -> &PreparedComponent {
        self.js_runner.get_or_init(|| {
            // The embedded runner is a known-good artifact; a per-app override is produced by
            // `rusm build` (custom bridges compiled in). A failure here is a build bug.
            let wasm = self.js_runner_wasm.as_deref().unwrap_or(JS_RUNNER_WASM);
            self.prepare_component(
                &self.compile_component(wasm).expect("js-runner compiles"),
                "run",
            )
            .expect("js-runner prepares")
        })
    }

    /// Compiles a module from Wasm bytes or `.wat` text.
    pub fn compile(&self, wasm: impl AsRef<[u8]>) -> Result<Module> {
        Ok(Module::new(&self.spawner.engine, wasm)?)
    }

    /// Total `notify` calls made by all guests so far — guest-reported progress.
    pub fn notifications(&self) -> u64 {
        self.shared.notifications.load(Ordering::Relaxed)
    }
}

impl Drop for WasmRuntime {
    fn drop(&mut self) {
        self.epoch_stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.epoch_ticker.take() {
            let _ = ticker.join();
        }
    }
}
