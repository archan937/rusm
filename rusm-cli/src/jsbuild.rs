//! Building a per-app **js-runner** with the app's custom bridges compiled in — the TS side
//! of custom bridges. The embedded runner's host imports are fixed at compile time, so an app
//! whose TS guests call a custom bridge rebuilds the runner with the bridge's *typed* WIT
//! import + the generated `bridges_gen` glue. Mirrors `js-runner/build.sh` (cargo → wizer →
//! wasm-tools), staged from the runner crate source.
//!
//! It needs the js-runner **source** plus the build toolchain (wasi-sdk + wizer + wasm-tools)
//! — available in the RUSM dev repo and a path-dep app (the real scenario). An installed
//! `rusm` would embed the source instead; that is a noted follow-on. The per-app build's
//! `target/` is reused across builds, so the QuickJS C object compiles once, not per build.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::bridges::{self, BridgeSpec};

/// The js-runner crate source, located relative to where rusm-cli was built.
const JS_RUNNER_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../crates/rusm-wasm/js-runner");

/// The js-http-runner crate source (the HTTP/SSE `fetch` twin of the js-runner).
const JS_HTTP_RUNNER_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crates/rusm-wasm/js-http-runner"
);

/// Build a per-app **js-runner** with `bridges` compiled in (for an app's TS actor/service/WS
/// guests); returns the component wasm. Stages a copy of the runner crate, injects the
/// generated `bridges_gen.rs` + the custom WIT imports, then runs the build pipeline.
pub fn build_app_js_runner(root: &Path, bridges: &[BridgeSpec]) -> Result<Vec<u8>> {
    let src = Path::new(JS_RUNNER_SRC);
    require_runner_src(src, "js-runner")?;
    let build = root.join("target/rusm-js-runner");
    bridges::stage_runner(src, &build, bridges)?;
    run_runner_build(&build, "js_runner")
}

/// Build a per-app **js-http-runner** with `bridges` compiled in (for an app's TS HTTP/SSE
/// `fetch` handlers), so a TS HTTP handler reaches a custom bridge exactly like a WS one;
/// returns the component wasm. Same pipeline as [`build_app_js_runner`]; staged with its sibling
/// js-runner `bridge/` (the shared JS it `include_str!`s — see [`bridges::stage_http_runner`]).
pub fn build_app_js_http_runner(root: &Path, bridges: &[BridgeSpec]) -> Result<Vec<u8>> {
    let http_src = Path::new(JS_HTTP_RUNNER_SRC);
    let js_src = Path::new(JS_RUNNER_SRC);
    require_runner_src(http_src, "js-http-runner")?;
    let parent = root.join("target/rusm-runners");
    bridges::stage_http_runner(http_src, js_src, &parent, bridges)?;
    run_runner_build(&parent.join("js-http-runner"), "js_http_runner")
}

/// Guard that a runner crate's source is present (a dev checkout / path-dep app) before a
/// per-app build — the one error message both runner builds share.
fn require_runner_src(src: &Path, name: &str) -> Result<()> {
    if !src.join("Cargo.toml").is_file() {
        bail!(
            "the {name} source isn't at {} — a TS guest that calls a custom bridge needs the RUSM \
             source (a dev checkout / path-dep app) plus wasi-sdk + wizer + wasm-tools; call the \
             bridge from a Rust or Go guest otherwise",
            src.display()
        );
    }
    Ok(())
}

/// The `cargo → wizer → wasm-tools` pipeline (mirrors `js-runner/build.sh`) in `dir`, for the
/// core crate `crate_name`. The wasi-sdk paths default to `~/.wasi-sdk` (as `build.sh`) unless
/// the matching `*_wasm32_wasip1` env var is already set (so a non-default install is honored).
fn run_runner_build(dir: &Path, crate_name: &str) -> Result<Vec<u8>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let env_or = |var: &str, default: String| std::env::var(var).unwrap_or(default);
    let cc = env_or("CC_wasm32_wasip1", format!("{home}/.wasi-sdk/bin/clang"));
    let ar = env_or("AR_wasm32_wasip1", format!("{home}/.wasi-sdk/bin/llvm-ar"));
    let cflags = env_or(
        "CFLAGS_wasm32_wasip1",
        format!("--sysroot={home}/.wasi-sdk/share/wasi-sysroot"),
    );

    let cargo = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-wasip1"])
        .env("CC_wasm32_wasip1", &cc)
        .env("AR_wasm32_wasip1", &ar)
        .env("CFLAGS_wasm32_wasip1", &cflags)
        .current_dir(dir)
        .status()
        .with_context(|| "running cargo for the per-app js-runner (is wasi-sdk installed?)")?;
    if !cargo.success() {
        bail!("building the per-app js-runner failed (cargo)");
    }

    let core = dir.join(format!("target/wasm32-wasip1/release/{crate_name}.wasm"));
    let wizer_out = dir.join("target/runner.wizer.wasm");
    let wizer = Command::new("wizer")
        .arg(&core)
        .arg("-o")
        .arg(&wizer_out)
        .args(["--init-func", "wizer_initialize", "--allow-wasi"])
        .status()
        .with_context(|| "running wizer (cargo install wizer --all-features?)")?;
    if !wizer.success() {
        bail!("pre-initializing the per-app js-runner failed (wizer)");
    }

    let component = dir.join("target/runner.component.wasm");
    let adapt = format!(
        "wasi_snapshot_preview1={}",
        dir.join("wasi_snapshot_preview1.reactor.wasm").display()
    );
    let wasm_tools = Command::new("wasm-tools")
        .args(["component", "new"])
        .arg(&wizer_out)
        .args(["--adapt", &adapt, "-o"])
        .arg(&component)
        .status()
        .with_context(|| "running wasm-tools (cargo install wasm-tools?)")?;
    if !wasm_tools.success() {
        bail!("componentizing the per-app js-runner failed (wasm-tools)");
    }
    std::fs::read(&component).with_context(|| format!("reading {}", component.display()))
}
