//! Hosting an app's node — the orchestration shared by the `rusm` CLI and by an
//! application's **own generated host crate** (the custom-bridge model: an app with a
//! `bridges/<name>/` compiles a small host binary that registers its bridges, then serves
//! its manifest). Both paths build the runtime and run the serve loop *here*, so there is
//! one construction + one serve loop, never two copies that could drift.
//!
//! [`build_runtime`] is the single place a [`WasmRuntime`] is wired from a manifest (store,
//! log level, custom bridges); [`serve`] is the single serve-until-Ctrl-C loop. A pure-guest
//! app passes a no-op extension (`|_| Ok(())`); an app with custom bridges passes its
//! generated `add_to_linker` calls.

use std::path::Path;

use anyhow::{Context, Result};
use rusm_node::NodeConfig;
use rusm_otp::Runtime;
use rusm_wasm::{wasmtime, BridgeLinker, WasmRuntime};

use crate::app::{serve_apps, spawn_components, Hosted};

/// Build the app's [`WasmRuntime`] from its manifest, wiring any **custom application
/// bridges** via `extend` (a bridge's generated `add_to_linker`; pass `|_| Ok(())` for a
/// pure-guest app). The one construction path for the CLI and a generated host crate, so
/// both apply the same store, log level, and bridges. Opens the configured durable store
/// (`store = "…"`, relative to the app dir, parent created) when set.
pub fn build_runtime(
    rt: Runtime,
    cfg: &NodeConfig,
    extend: impl Fn(&mut BridgeLinker) -> wasmtime::Result<()> + 'static,
) -> Result<WasmRuntime> {
    // Environment the Rust way: process env first, then `./.env`. Loaded here — the one
    // construction path — so an app's own host binary (the custom-bridge model) sees its
    // `.env` exactly as `rusm serve` does, and a bridge's host impl can read a secret the
    // manifest never grants to any guest.
    dotenvy::dotenv().ok();
    let mut builder = WasmRuntime::builder(rt).bridges(extend);
    if let Some(secs) = cfg.node.dynamic_wasm_ttl_secs {
        builder = builder.dynamic_ttl(std::time::Duration::from_secs(secs));
    }
    if let Some(rel) = &cfg.node.store {
        let path = Path::new(".").join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        builder = builder.store(path);
    }
    // A per-app js-runner (TS guests that call a custom bridge) — `rusm build` rebuilds it with
    // the bridges' typed imports compiled in and writes it to `wasm/`. Absent for pure-guest
    // and Rust/Go-only apps, which use the embedded runner.
    let runner = Path::new("./wasm/js_runner.wasm");
    if runner.is_file() {
        builder = builder.js_runner(
            std::fs::read(runner).with_context(|| format!("reading {}", runner.display()))?,
        );
    }
    let wasm = builder.build()?;
    // Platform lifecycle logging: explicit, off by default — declared via `[log] level`.
    wasm.set_log_level(cfg.log_level());
    Ok(wasm)
}

/// Host the app from `root`: register its `[components]` on the node, serve its `[[serve]]`
/// endpoints (wiring custom bridges via `extend`), then run until Ctrl-C. The single serve
/// loop behind `rusm serve` and an app's generated host crate. Registering components first
/// means a `[[serve]]` route can spawn a matched handler and a sibling can `whereis` a
/// resident service — one node, brought up once.
pub async fn serve(
    root: &Path,
    cfg: &NodeConfig,
    extend: impl Fn(&mut BridgeLinker) -> wasmtime::Result<()> + 'static,
) -> Result<()> {
    serve_with_init(root, cfg, extend, |_| Ok(())).await
}

/// Like [`serve`] but runs `init(&wasm)` between runtime construction and component spawn —
/// so a TS/Go-bridge app can register its runner components as resident actors before any
/// guest tries to call them. The generated `src/main.rs` for a TS/Go-bridge app calls this;
/// a pure-Rust-bridge app calls `serve` with `bridges::extend` directly.
pub async fn serve_with_init(
    root: &Path,
    cfg: &NodeConfig,
    extend: impl Fn(&mut BridgeLinker) -> wasmtime::Result<()> + 'static,
    init: impl FnOnce(&WasmRuntime) -> anyhow::Result<()>,
) -> Result<()> {
    let rt = Runtime::new();
    let wasm = build_runtime(rt.clone(), cfg, extend)?;
    // `init` registers an app's resident bridge runners *and* its serving auth hooks
    // (via [`WasmRuntime::register_auth_hook`]) — both after the runtime exists, before
    // serving, so a `[[serve]] authentication` can resolve its hook in `serve_apps`.
    init(&wasm)?;
    let hosted = spawn_components(root, &wasm, &cfg.components, &cfg.capabilities).await?;
    let endpoints = serve_apps(root, &wasm, &cfg.serve, &cfg.components, &cfg.capabilities).await?;
    if endpoints.is_empty() && hosted.is_empty() {
        println!("no [[serve]] entries or [components] in rusm.toml — nothing to do");
        return Ok(());
    }
    if !hosted.is_empty() {
        print_hosted(&hosted);
    }
    println!("serving {} endpoint(s):", endpoints.len());
    for ep in &endpoints {
        let scheme = if ep.protocol.is_http() { "http" } else { "ws" };
        println!("  {:<16} {scheme}://{}", ep.name, ep.addr);
    }
    println!("press Ctrl-C to stop");
    tokio::signal::ctrl_c().await?;
    println!("\nstopping {} process(es)…", rt.shutdown());
    Ok(())
}

/// One line describing what the node is hosting: the resident services (boot-spawned +
/// supervised) and the on-demand components (registered, spawned per request/call).
pub fn print_hosted(hosted: &Hosted) {
    let on_demand: Vec<&str> = hosted
        .names
        .iter()
        .filter(|n| !hosted.resident.contains(*n))
        .map(String::as_str)
        .collect();
    if !hosted.resident.is_empty() {
        println!("resident: {}", hosted.resident.join(", "));
    }
    if !on_demand.is_empty() {
        println!("on demand: {}", on_demand.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// `build_runtime` forwards the custom-bridge extension to the runtime: it is invoked
    /// during construction (the builder runs it against every engine tier). This is
    /// `build_runtime`'s own responsibility — that a *real* extension makes a guest call
    /// resolve is proven end to end at the rusm-wasm layer.
    #[tokio::test]
    async fn build_runtime_invokes_the_bridge_extension() {
        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);
        let _wasm = build_runtime(Runtime::new(), &NodeConfig::default(), move |_linker| {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(
            called.load(Ordering::SeqCst),
            "build_runtime must run the custom-bridge extension at construction"
        );
    }

    /// `serve_with_init` runs `init` after `build_runtime` and before the component spawn.
    /// An empty `NodeConfig` (no `[[serve]]` entries, no `[components]`) causes the function
    /// to return immediately (the "nothing to do" path) — making it directly testable without
    /// blocking on Ctrl-C.
    #[tokio::test]
    async fn serve_with_init_calls_init_before_spawn() {
        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);
        serve_with_init(
            Path::new("."),
            &NodeConfig::default(),
            |_| Ok(()),
            move |_wasm| {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(called.load(Ordering::SeqCst), "init must be called");
    }

    /// `build_runtime` attaches the durable store iff the manifest declares one — the
    /// store-threading half of construction (the bridge half is above).
    #[tokio::test]
    async fn build_runtime_attaches_the_store_only_when_configured() {
        let storeless = build_runtime(Runtime::new(), &NodeConfig::default(), |_| Ok(())).unwrap();
        assert!(
            storeless.store().is_none(),
            "no `store =` in the manifest → no store"
        );

        let dir = std::env::temp_dir().join(format!("rusm-host-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("kv.redb");
        let _ = std::fs::remove_file(&path);
        let mut cfg = NodeConfig::default();
        cfg.node.store = Some(path.to_string_lossy().into_owned());
        let stored = build_runtime(Runtime::new(), &cfg, |_| Ok(())).unwrap();
        assert!(
            stored.store().is_some(),
            "`store =` in the manifest → the durable store is attached"
        );
        let _ = std::fs::remove_file(&path);
    }
}
