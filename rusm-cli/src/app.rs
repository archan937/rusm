//! The **app model**: build a project's components and run them from `./wasm/`.
//!
//! A RUSM app is a directory with `rusm.toml` (`[components.<name>]`), a `components/`
//! tree of source crates, and a `wasm/` dir of built artifacts. `rusm build`
//! compiles each `components/<name>/` to either `wasm/<name>.wasm` (a Rust
//! component) or `wasm/<name>.js` (a TypeScript bundle, Bun-built); the loader
//! resolves whichever exists and **registers** each declared component for
//! spawn-by-name (a route or a sibling spawns it on demand). A component marked
//! `resident = true` is additionally boot-spawned and supervised as a long-lived
//! service. A `.js` artifact runs on the shared rquickjs js-runner; a `.wasm`
//! artifact is a component instance (or a stock `wasi:cli` command, run once).
//! Env vars are the Rust way: process env first, then `.env`.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rusm_node::{
    BundleSource, CapabilitySpec, ComponentSpec, Resolution, RouteTable, ServeProtocol, ServeSpec,
};
use rusm_otp::{ProcessHandle, Runtime};
use rusm_wasm::{
    Capabilities, CapabilityProfile, HttpServer, Resolver, Routed, SseServer, WasmRuntime, WsServer,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Resolves a [`CapabilitySpec`] to concrete [`Capabilities`]: start from the
/// inherited built-in base (default `sandboxed`), then apply each set override.
/// Env keys are resolved from the process environment (process env, then `.env`).
///
/// This is the `rusm-wasm` conversion of the manifest's `[capabilities.<name>]`
/// table; it lives here, the only place that links the Wasm backend, so the
/// `rusm-node` manifest crate stays Wasm-free.
fn to_capabilities(spec: &CapabilitySpec) -> Capabilities {
    let mut caps = spec
        .inherits
        .as_deref()
        .and_then(CapabilityProfile::from_id)
        .unwrap_or(CapabilityProfile::Sandboxed)
        .capabilities();
    if let Some(v) = spec.allow_network {
        caps = caps.allow_network(v);
    }
    if let Some(v) = spec.allow_spawn {
        caps = caps.allow_spawn(v);
    }
    if let Some(v) = spec.allow_process_control {
        caps = caps.allow_process_control(v);
    }
    if let Some(v) = spec.allow_stdio {
        caps = caps.inherit_stdio(v);
    }
    if let Some(v) = spec.allow_storage {
        caps = caps.allow_storage(v);
    }
    if let Some(mb) = spec.max_memory_mb {
        caps = caps.max_memory(mb << 20);
    }
    for key in &spec.env {
        if let Ok(value) = std::env::var(key) {
            caps = caps.env(key, value);
        }
    }
    for p in &spec.preopen {
        caps = caps.preopen(&p.host, &p.guest, p.read_only);
    }
    caps
}

/// Resolves a capability id to its [`Capabilities`]: a custom `[capabilities.<id>]`
/// profile first, then a built-in (`sandboxed` / `network-client` / `trusted`),
/// falling back to the secure `Sandboxed` default (default-deny) for an unknown id.
pub fn capabilities_for(id: &str, profiles: &HashMap<String, CapabilitySpec>) -> Capabilities {
    if let Some(spec) = profiles.get(id) {
        return to_capabilities(spec);
    }
    CapabilityProfile::from_id(id)
        .unwrap_or(CapabilityProfile::Sandboxed)
        .capabilities()
}

/// Resolve a component's optional `source` to JS bundle bytes — the dynamic-deploy
/// path: fetch from a URL (e.g. a presigned blob / artifact API) or read from the
/// node's durable `kv` store, instead of the local `./wasm/<name>` artifact. `None`
/// when no `source` is set (the caller falls back to the local file). A remote source
/// is always a **JS** bundle. Re-run on each `spawn`/reload, so updating the source
/// deploys new JS with no node rebuild.
async fn remote_bundle(source: Option<&str>, wasm: &WasmRuntime) -> Result<Option<Vec<u8>>> {
    let Some(spec) = source else {
        return Ok(None);
    };
    let bytes = match BundleSource::parse(spec).map_err(|e| anyhow!(e))? {
        BundleSource::Url(url) => fetch_url(&url).await?,
        BundleSource::Kv { bucket, key } => wasm
            .store()
            .ok_or_else(|| anyhow!("kv source `{spec}` needs a store (set `store` in rusm.toml)"))?
            .bucket(&bucket)
            .get(&key)
            .map_err(|e| anyhow!("reading kv {bucket}/{key}: {e}"))?
            .ok_or_else(|| anyhow!("kv source `{spec}` not found ({bucket}/{key})"))?,
    };
    Ok(Some(bytes))
}

/// GET `url` and return the body bytes (a one-shot bundle fetch). A non-2xx status
/// is an error, so a stale/forbidden link fails loudly rather than loading garbage.
async fn fetch_url(url: &str) -> Result<Vec<u8>> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("fetching bundle from {url}"))?
        .error_for_status()
        .with_context(|| format!("fetching bundle from {url}"))?;
    Ok(response
        .bytes()
        .await
        .with_context(|| format!("reading bundle body from {url}"))?
        .to_vec())
}

/// Build the resolver for guest `spawn-from` `url:` sources — a one-shot HTTP(S) GET,
/// injected into the [`WasmRuntime`] so the engine crate needs no HTTP client of its own.
/// `inline:`/`kv:` sources are resolved in the engine and never reach this.
fn url_bundle_resolver() -> rusm_wasm::BundleResolver {
    std::sync::Arc::new(|url: String| {
        Box::pin(async move { fetch_url(&url).await.map_err(|e| e.to_string()) })
    })
}

/// The app's hosted components after [`spawn_components`]: every entry is
/// registered for spawn-by-name, and the `resident` subset is boot-spawned under a
/// single supervisor. Hold this for the node's lifetime (the `supervisor` keeps the
/// resident tree alive); [`teardown`](Self::teardown) stops it for a `rusm dev` reload.
pub struct Hosted {
    /// Every hosted component name (all registered, spawnable by a route or sibling).
    pub names: Vec<String>,
    /// The `resident = true` subset: boot-spawned and supervised as long-lived services.
    pub resident: Vec<String>,
    /// The one-for-one supervisor over the residents (`None` if there are none).
    supervisor: Option<ProcessHandle>,
    /// One-shot stock `wasi:cli` commands, spawned at boot and held.
    commands: Vec<ProcessHandle>,
}

impl Hosted {
    /// No components were declared at all (nothing to register, boot, or serve).
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Tears the hosted processes down for a `rusm dev` reload: stop the resident
    /// supervisor (so it ceases restarting), kill each resident by its registered
    /// name (a resident service registers itself — the RUSM convention), and kill any
    /// one-shot command. Registered-but-not-resident components hold no instance, so
    /// re-registering on reload simply overwrites their factory.
    pub fn teardown(&self, rt: &Runtime) {
        if let Some(sup) = &self.supervisor {
            sup.kill();
        }
        for name in &self.resident {
            if let Some(pid) = rt.whereis(name) {
                rt.kill(pid);
            }
        }
        for cmd in &self.commands {
            cmd.kill();
        }
    }
}

/// The outcome of registering one component: a registrable service (actor/JS,
/// spawnable by name) or a stock `wasi:cli` command (run once at boot).
enum Registration {
    Service,
    Command(ProcessHandle),
}

/// Resolves one component's artifact from `<wasm_dir>` and registers it for
/// spawn-by-name under `caps`. A `source` (url/kv) or a `<name>.{qjsbc,js}` bundle is
/// a TS service; a `<name>.wasm` is an actor component (registrable) or, failing the
/// actor world, a stock `wasi:cli` command spawned once. Errors if no artifact exists
/// or it won't compile — a clear signal to run `rusm build` first.
async fn register_component(
    wasm_dir: &Path,
    wasm: &WasmRuntime,
    name: &str,
    caps: &Capabilities,
    source: Option<&str>,
    dynamic: Option<&str>,
) -> Result<Registration> {
    // A dynamic JS runner template (`dynamic = "js"`): a capability profile with no fixed
    // bundle. A guest reaches it via spawn-from(name, runtime-source); nothing loads here.
    if let Some(kind) = dynamic {
        if kind != "js" {
            return Err(anyhow!(
                "component `{name}`: dynamic = {kind:?} is not supported (use \"js\")"
            ));
        }
        if source.is_some() {
            return Err(anyhow!(
                "component `{name}`: `dynamic` and `source` are mutually exclusive (a template has no fixed bundle)"
            ));
        }
        wasm.register_js_template(name.to_string(), caps.clone());
        return Ok(Registration::Service);
    }
    // A configured `source` (url/kv) supplies a JS bundle directly — the
    // dynamic-deploy path, no local artifact needed.
    if let Some(bundle) = remote_bundle(source, wasm).await? {
        wasm.register_js_component_with(name.to_string(), bundle, caps.clone());
        return Ok(Registration::Service);
    }
    // TypeScript component: prefer the precompiled QuickJS bytecode (`<name>.qjsbc`,
    // no runtime parse) and fall back to the `.js` source. Both run on the shared
    // js-runner, which detects the form by its magic prefix.
    let bc_path = wasm_dir.join(format!("{name}.qjsbc"));
    let js_path = wasm_dir.join(format!("{name}.js"));
    if let Some(path) = [bc_path, js_path].into_iter().find(|p| p.is_file()) {
        let bundle = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        wasm.register_js_component_with(name.to_string(), bundle, caps.clone());
        return Ok(Registration::Service);
    }
    let path = wasm_dir.join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {} (run `rusm build`?)", path.display()))?;
    let component = wasm
        .compile_component(&bytes)
        .with_context(|| format!("compiling component `{name}`"))?;
    // An actor component exports `run` (rusm:runtime); a stock component exports
    // `wasi:cli/run`. Prefer the actor path (registrable + spawnable by siblings, and
    // resident-supervisable); otherwise run it unchanged as a one-shot command.
    match wasm.prepare_component(&component, "run") {
        Ok(prepared) => {
            wasm.register_component_with(name.to_string(), prepared, caps.clone());
            Ok(Registration::Service)
        }
        Err(_) => {
            let handle = wasm
                .spawn_command_with(&component, caps.clone())
                .with_context(|| {
                    format!("`{name}` is neither a rusm actor component nor a wasi:cli command")
                })?;
            Ok(Registration::Command(handle))
        }
    }
}

/// Loads each manifest `[components.<name>]` from `<dir>/wasm/` and **registers** it
/// for spawn-by-name under its capability profile. A component marked
/// `resident = true` is also boot-spawned and supervised as a long-lived service
/// (one-for-one, restart-intensity-bounded); a non-resident one holds no instance
/// until a route or sibling spawns it. A stock `wasi:cli` command runs once at boot.
/// Returns the [`Hosted`] set (hold it to keep residents alive). Errors if an
/// artifact is missing or won't compile.
pub async fn spawn_components(
    dir: &Path,
    wasm: &WasmRuntime,
    specs: &BTreeMap<String, ComponentSpec>,
    profiles: &HashMap<String, CapabilitySpec>,
) -> Result<Hosted> {
    let wasm_dir = dir.join("wasm");
    // Wire the resolver for guest `spawn-from` `url:` sources (rusm-wasm stays HTTP-free;
    // the node owns egress). `inline:`/`kv:` sources need no resolver.
    wasm.set_bundle_resolver(url_bundle_resolver());
    let mut names = Vec::with_capacity(specs.len());
    let mut resident = Vec::new();
    let mut commands = Vec::new();
    for (name, spec) in specs {
        let caps = capabilities_for(&spec.capability, profiles);
        names.push(name.clone());
        if spec.dynamic.is_some() && spec.resident {
            return Err(anyhow!(
                "component `{name}`: a dynamic template can't be `resident` (it has no bundle to boot-spawn)"
            ));
        }
        match register_component(
            &wasm_dir,
            wasm,
            name,
            &caps,
            spec.source.as_deref(),
            spec.dynamic.as_deref(),
        )
        .await?
        {
            Registration::Service if spec.resident => resident.push(name.clone()),
            Registration::Service => {}
            Registration::Command(handle) => {
                // Platform log: name this boot component (so its exit can name it) +
                // log its spawn at Debug. No-op unless `[log]` is set.
                wasm.note_spawn(&handle, name, &caps);
                commands.push(handle);
            }
        }
    }
    // One supervisor boots + keeps every resident service alive (logs each (re)start).
    let supervisor = wasm.supervise(&resident);
    Ok(Hosted {
        names,
        resident,
        supervisor,
        commands,
    })
}

/// A `[[serve]]` server now running: its name, protocol, the address it actually
/// bound (resolves `:0` to the real port), and the task driving it. Hold the task
/// (and the `WasmRuntime`) to keep the server up.
pub struct ServedEndpoint {
    pub name: String,
    pub protocol: ServeProtocol,
    pub addr: SocketAddr,
    pub task: JoinHandle<()>,
}

/// Hosts each `[[serve]]` entry on its own TCP port: load the component from
/// `<dir>/wasm/<name>.{wasm,js}`, build the matching server (HTTP/SSE via
/// `http_server`, WebSocket via `ws_server`), bind its `listen` address, and spawn
/// the accept loop. Returns the live endpoints (the bound address is captured
/// before serving, so a caller can log or connect to it immediately). Errors with a
/// clear message if an artifact is missing or an address won't bind.
pub async fn serve_apps(
    dir: &Path,
    wasm: &WasmRuntime,
    specs: &[ServeSpec],
    components: &BTreeMap<String, ComponentSpec>,
    profiles: &HashMap<String, CapabilitySpec>,
) -> Result<Vec<ServedEndpoint>> {
    // A listener is pure: protocol + listen (+ routes). Its handler components are
    // `[components.<name>]`, each under its own declared profile — already registered
    // for spawn-by-name by `spawn_components`. The routed gateway spawns the matched
    // handler per request under that profile (the map below, shared across listeners).
    let caps_map: HashMap<String, Capabilities> = components
        .iter()
        .map(|(name, c)| (name.clone(), capabilities_for(&c.capability, profiles)))
        .collect();

    let mut endpoints = Vec::with_capacity(specs.len());
    for spec in specs {
        let label = spec.name.clone().unwrap_or_else(|| spec.listen.clone());
        let listener = TcpListener::bind(&spec.listen)
            .await
            .with_context(|| format!("binding {} for `{label}`", spec.listen))?;
        let addr = listener
            .local_addr()
            .with_context(|| format!("local address of `{label}`"))?;
        // Routed serving: a `[serve.routes]` table maps each request/connection's path to a
        // handler component — per request for HTTP (`component#action`), per connection for
        // ws/sse (the component *is* the handler, with captured path params in its context).
        let routed = !spec.routes.is_empty();
        // Build the server up front so a load/compile error surfaces here (before we
        // claim the endpoint is up), then drive the accept loop on its own task.
        let task = if routed {
            // Resolve this listener's routes (the table is parsed per protocol:
            // `component#action` for HTTP, bare `component` for ws/sse).
            let table = spec
                .route_table()
                .map_err(|e| anyhow!("invalid [serve.routes] for {}: {e}", spec.listen))?;
            let resolver = routed_resolver(table);
            let headers = spec.header_pairs();
            match spec.protocol {
                // Per-request HTTP: spawn the matched handler fresh, dispatch the action.
                ServeProtocol::Http => tokio::spawn(
                    wasm.routed_http_server(resolver, caps_map.clone())
                        .with_headers(headers)
                        .serve(listener),
                ),
                // Per-connection SSE: resolve the path to a handler component, capturing params.
                ServeProtocol::Sse => tokio::spawn(
                    wasm.routed_sse_server(resolver, caps_map.clone())
                        .with_headers(headers)
                        .serve(listener),
                ),
                // Per-connection WebSocket: same; an unmatched path refuses with 404.
                ServeProtocol::Ws => tokio::spawn(
                    wasm.routed_ws_server(resolver, caps_map.clone())
                        .serve(listener),
                ),
            }
        } else {
            // A single named handler component: a handler-less `wasi:http` HTTP component,
            // an SSE handler, or a WebSocket worker — each one process per request/connection.
            let name = spec.name.as_deref().ok_or_else(|| {
                let hint = if matches!(spec.protocol, ServeProtocol::Http) {
                    ", or a `[serve.routes]` table for HTTP"
                } else {
                    ""
                };
                anyhow!(
                    "the `{:?}` listener on {} needs a `name` (its handler component){hint}",
                    spec.protocol,
                    spec.listen
                )
            })?;
            // Its capability profile comes from a matching `[components.<name>]` entry,
            // else default-deny `sandboxed`.
            let caps = components
                .get(name)
                .map(|c| capabilities_for(&c.capability, profiles))
                .unwrap_or_else(|| CapabilityProfile::Sandboxed.capabilities());
            let remote = remote_bundle(spec.source.as_deref(), wasm).await?;
            let headers = spec.header_pairs();
            match spec.protocol {
                ServeProtocol::Sse => tokio::spawn(
                    build_sse_server(dir, wasm, name, caps, remote)?
                        .with_headers(headers)
                        .serve(listener),
                ),
                ServeProtocol::Ws => {
                    tokio::spawn(build_ws_server(dir, wasm, name, caps, remote)?.serve(listener))
                }
                ServeProtocol::Http => tokio::spawn(
                    build_http_server(dir, wasm, name, caps, remote)?
                        .with_headers(headers)
                        .serve(listener),
                ),
            }
        };
        endpoints.push(ServedEndpoint {
            name: label,
            protocol: spec.protocol,
            addr,
            task,
        });
    }
    Ok(endpoints)
}

/// Bridge a listener's [`RouteTable`] into the engine's routing-agnostic [`Resolver`]
/// — the only place the config's `[serve.routes]` shape meets the Wasm gateway.
fn routed_resolver(table: RouteTable) -> Resolver {
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

/// Builds an HTTP/SSE server for `name`, resolving a `.js` bundle (on the
/// js-http-runner) before a `.wasm` component (instance-per-request `wasi:http`).
fn build_http_server(
    dir: &Path,
    wasm: &WasmRuntime,
    name: &str,
    caps: Capabilities,
    remote: Option<Vec<u8>>,
) -> Result<HttpServer> {
    if let Some(bundle) = remote {
        let bundle = String::from_utf8(bundle).context("URL/kv bundle is not valid UTF-8 JS")?;
        return Ok(wasm.http_server_js(bundle, caps));
    }
    let wasm_dir = dir.join("wasm");
    let js_path = wasm_dir.join(format!("{name}.js"));
    if js_path.is_file() {
        let bundle = std::fs::read_to_string(&js_path)
            .with_context(|| format!("reading {}", js_path.display()))?;
        return Ok(wasm.http_server_js(bundle, caps));
    }
    let path = wasm_dir.join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {} (run `rusm build`?)", path.display()))?;
    let component = wasm
        .compile_component(&bytes)
        .with_context(|| format!("compiling component `{name}`"))?;
    let prepared = wasm
        .prepare_http(&component)
        .with_context(|| format!("`{name}` is not a `wasi:http` component"))?;
    Ok(wasm.http_server(&prepared, caps))
}

/// Builds a WebSocket server for `name`: a `.js` worker bundle (on the js-runner)
/// before a `.wasm` actor component (one process per connection).
fn build_ws_server(
    dir: &Path,
    wasm: &WasmRuntime,
    name: &str,
    caps: Capabilities,
    remote: Option<Vec<u8>>,
) -> Result<WsServer> {
    if let Some(bundle) = remote {
        return Ok(wasm.ws_server_js(bundle, caps));
    }
    let wasm_dir = dir.join("wasm");
    let js_path = wasm_dir.join(format!("{name}.js"));
    if js_path.is_file() {
        let bundle =
            std::fs::read(&js_path).with_context(|| format!("reading {}", js_path.display()))?;
        return Ok(wasm.ws_server_js(bundle, caps));
    }
    let path = wasm_dir.join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {} (run `rusm build`?)", path.display()))?;
    let component = wasm
        .compile_component(&bytes)
        .with_context(|| format!("compiling component `{name}`"))?;
    let prepared = wasm.prepare_component(&component, "run")?;
    Ok(wasm.ws_server(&prepared, caps))
}

/// Builds an SSE server for `name` — the SSE twin of [`build_ws_server`]: a `.js` worker
/// bundle (on the js-runner) before a `.wasm` actor component (one process per connection).
fn build_sse_server(
    dir: &Path,
    wasm: &WasmRuntime,
    name: &str,
    caps: Capabilities,
    remote: Option<Vec<u8>>,
) -> Result<SseServer> {
    if let Some(bundle) = remote {
        return Ok(wasm.sse_server_js(bundle, caps));
    }
    let wasm_dir = dir.join("wasm");
    let js_path = wasm_dir.join(format!("{name}.js"));
    if js_path.is_file() {
        let bundle =
            std::fs::read(&js_path).with_context(|| format!("reading {}", js_path.display()))?;
        return Ok(wasm.sse_server_js(bundle, caps));
    }
    let path = wasm_dir.join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {} (run `rusm build`?)", path.display()))?;
    let component = wasm
        .compile_component(&bytes)
        .with_context(|| format!("compiling component `{name}`"))?;
    let prepared = wasm.prepare_component(&component, "run")?;
    Ok(wasm.sse_server(&prepared, caps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusm_otp::Pid;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// A `[components.<name>]` map from `(name, spec)` pairs (the loader's input).
    fn components(entries: &[(&str, ComponentSpec)]) -> BTreeMap<String, ComponentSpec> {
        entries
            .iter()
            .map(|(n, s)| (n.to_string(), s.clone()))
            .collect()
    }

    /// A `ComponentSpec` with a capability id and the resident flag (no remote source).
    fn spec(capability: &str, resident: bool) -> ComponentSpec {
        ComponentSpec {
            capability: capability.to_string(),
            resident,
            source: None,
            dynamic: None,
        }
    }

    /// Poll until `name` resolves (the process registered itself), or give up (~2 s).
    async fn await_named(rt: &Runtime, name: &str) -> Option<Pid> {
        for _ in 0..400 {
            if let Some(pid) = rt.whereis(name) {
                return Some(pid);
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        None
    }

    /// Poll until `name` resolves to a pid different from `old` (a restart re-registered).
    async fn await_renamed(rt: &Runtime, name: &str, old: Pid) -> Option<Pid> {
        for _ in 0..400 {
            match rt.whereis(name) {
                Some(pid) if pid != old => return Some(pid),
                _ => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        None
    }

    #[test]
    fn a_custom_profile_inherits_then_overrides() {
        // Starts from network-client (network on, spawn off), then turns spawn on
        // and tightens memory — only the set fields override the inherited base.
        let cfg = rusm_node::NodeConfig::from_toml(
            "[capabilities.worker]\ninherits = \"network-client\"\nallow-spawn = true\nmax-memory-mb = 32\n",
        )
        .unwrap();
        let caps = to_capabilities(&cfg.capabilities["worker"]);
        assert!(caps.can_spawn(), "override turned spawn on");
        assert_eq!(caps.memory_limit(), 32 << 20, "override tightened memory");
        // An omitted base → the most restrictive default (sandboxed): no spawn.
        let bare = CapabilitySpec {
            inherits: None,
            allow_network: None,
            allow_spawn: None,
            allow_process_control: None,
            allow_stdio: None,
            allow_storage: None,
            max_memory_mb: None,
            env: Vec::new(),
            preopen: Vec::new(),
        };
        assert!(
            !to_capabilities(&bare).can_spawn(),
            "default base is sandboxed"
        );
    }

    #[test]
    fn storage_grant_maps_through() {
        // `allow-storage = true` on a profile turns the durable-KV grant on…
        let cfg = rusm_node::NodeConfig::from_toml(
            "[capabilities.stateful]\ninherits = \"trusted\"\nallow-storage = true\n",
        )
        .unwrap();
        assert!(to_capabilities(&cfg.capabilities["stateful"]).storage_allowed());
        // …and omitting it inherits the base (sandboxed → no storage).
        let cfg = rusm_node::NodeConfig::from_toml("[capabilities.x]\ninherits = \"sandboxed\"\n")
            .unwrap();
        assert!(!to_capabilities(&cfg.capabilities["x"]).storage_allowed());
    }

    // A minimal component (WAT text — accepted by compile_component) standing in
    // for a built `wasm/<name>.wasm`; it just runs and returns.
    const COMPONENT: &str = r#"(component
        (core module $m (memory (export "mem") 1) (func (export "run")))
        (core instance $i (instantiate $m))
        (func (export "run") (canon lift (core func $i "run"))))"#;

    #[test]
    fn capability_resolution_prefers_custom_then_builtin_then_sandboxed() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "agent".to_string(),
            CapabilitySpec {
                inherits: Some("network-client".to_string()),
                allow_network: None,
                allow_spawn: Some(true),
                allow_process_control: None,
                allow_stdio: None,
                allow_storage: None,
                max_memory_mb: Some(16),
                env: Vec::new(),
                preopen: Vec::new(),
            },
        );
        // A custom profile resolves to its grants.
        let agent = capabilities_for("agent", &profiles);
        assert!(agent.can_spawn(), "custom profile grants spawn");
        assert_eq!(agent.memory_limit(), 16 << 20);
        // A built-in still resolves; an unknown id falls back to sandboxed.
        assert!(
            capabilities_for("trusted", &profiles).can_spawn(),
            "built-in trusted resolves and grants spawn"
        );
        assert!(
            !capabilities_for("does-not-exist", &profiles).can_spawn(),
            "unknown id falls back to default-deny sandboxed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_resident_components_register_without_booting() {
        // A non-resident component is registered for spawn-by-name (a route or sibling
        // spawns it on demand) but is NOT boot-spawned — no idle instance is parked.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        std::fs::write(dir.path().join("wasm/echo.wasm"), COMPONENT).unwrap();

        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt.clone()).unwrap();
        let hosted = spawn_components(
            dir.path(),
            &wasm,
            &components(&[("echo", spec("sandboxed", false))]),
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(hosted.names, ["echo"]);
        assert!(hosted.resident.is_empty(), "echo is not resident");
        assert!(
            rt.list().is_empty(),
            "registered only — no boot instance was spawned"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_resident_service_is_boot_spawned_and_supervised() {
        // `resident = true` boot-spawns the component AND supervises it: it comes up
        // (registers its well-known name), and on a crash the supervisor restarts it
        // under a fresh pid — the robustness that replaces the old dead `restart` flag.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        // A long-lived JS service: register a name, then loop on receive forever.
        std::fs::write(
            dir.path().join("wasm/svc.js"),
            "module.exports.default = async function(){ Process.register('svc'); for(;;) await Process.receive(); };",
        )
        .unwrap();

        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt.clone()).unwrap();
        let hosted = spawn_components(
            dir.path(),
            &wasm,
            &components(&[("svc", spec("sandboxed", true))]),
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(hosted.resident, ["svc"]);

        // It booted and registered its name.
        let first = await_named(&rt, "svc").await.expect("resident booted");
        // Crash it; the supervisor restarts it under a fresh pid, re-registering `svc`.
        rt.kill(first);
        let second = await_renamed(&rt, "svc", first)
            .await
            .expect("supervisor restarted the crashed resident");
        assert_ne!(first, second, "a fresh instance replaced the crashed one");
    }

    // A lean `wasi:http` component fixture (the same one rusm-wasm/http_bench uses),
    // standing in for a built `wasm/<name>.wasm` HTTP server.
    const HTTP_LEAN: &[u8] = include_bytes!("../../crates/rusm-wasm/tests/fixtures/http_lean.wasm");

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn serves_an_http_component_on_a_real_port() {
        // The exact `rusm serve` path: drop a built `.wasm` into ./wasm, host it on a
        // real TCP port via `[[serve]]`, then hit it with a genuine HTTP GET. This is
        // the fair, out-of-process shape a load driver uses — no in-process generator.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        std::fs::write(dir.path().join("wasm/api.wasm"), HTTP_LEAN).unwrap();

        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt).unwrap();
        let specs = vec![ServeSpec {
            protocol: ServeProtocol::Http,
            listen: "127.0.0.1:0".to_string(), // ephemeral; we read back the real port
            routes: HashMap::new(),            // no routes → the handler-less wasi:http path
            name: Some("api".to_string()),
            source: None,
            headers: Default::default(),
        }];
        let endpoints = serve_apps(dir.path(), &wasm, &specs, &BTreeMap::new(), &HashMap::new())
            .await
            .unwrap();
        assert_eq!(endpoints.len(), 1);
        let addr = endpoints[0].addr;
        assert_ne!(addr.port(), 0, "an ephemeral port was bound and reported");

        // A real client connection — exactly what an external load tool does.
        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        conn.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response);
        assert!(
            text.starts_with("HTTP/1.1 200"),
            "the hosted component answered 200 (got: {:?})",
            text.lines().next()
        );
    }

    // A Rust WS-handler component fixture (echoes each frame from the sandbox).
    const RS_WS_ECHO: &[u8] =
        include_bytes!("../../crates/rusm-wasm/tests/fixtures/rs_ws_echo.wasm");

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn serves_a_websocket_component_on_a_real_port() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        std::fs::write(dir.path().join("wasm/echo.wasm"), RS_WS_ECHO).unwrap();

        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt).unwrap();
        let specs = vec![ServeSpec {
            protocol: ServeProtocol::Ws,
            listen: "127.0.0.1:0".to_string(),
            routes: HashMap::new(),
            name: Some("echo".to_string()),
            source: None,
            headers: Default::default(),
        }];
        let endpoints = serve_apps(dir.path(), &wasm, &specs, &BTreeMap::new(), &HashMap::new())
            .await
            .unwrap();
        let addr = endpoints[0].addr;

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        ws.send(Message::binary(b"ping".as_slice())).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(
            reply.into_data().as_ref(),
            b"ping",
            "the WS component echoed"
        );
    }

    // A `#[rusm_rs::handlers]` component: `fn hello(_, params)` + `fn echo(req, _)`.
    const RS_HANDLERS: &[u8] =
        include_bytes!("../../crates/rusm-wasm/tests/fixtures/rs_handlers_demo.wasm");

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_routes_table_dispatches_per_request_to_a_handler_component() {
        // The unified model end-to-end via `rusm serve`: a `[serve.routes]` table on an
        // HTTP `[[serve]]` listener; each request spawns the matched handler fresh and
        // dispatches `component#action`. The handler is just `fn`s — no router code.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        std::fs::write(dir.path().join("wasm/api.wasm"), RS_HANDLERS).unwrap();

        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt).unwrap();
        // The handler is a `[components.api]` entry (registered for spawn-by-name); the
        // listener is pure routes — no name/capability.
        let handlers = components(&[("api", spec("sandboxed", false))]);
        spawn_components(dir.path(), &wasm, &handlers, &HashMap::new())
            .await
            .unwrap();
        let specs = vec![ServeSpec {
            protocol: ServeProtocol::Http,
            listen: "127.0.0.1:0".to_string(),
            routes: HashMap::from([
                ("GET /hello/:name".to_string(), "api#hello".to_string()),
                ("POST /echo".to_string(), "api#echo".to_string()),
            ]),
            name: None,
            source: None,
            headers: Default::default(),
        }];
        let endpoints = serve_apps(dir.path(), &wasm, &specs, &handlers, &HashMap::new())
            .await
            .unwrap();
        let addr = endpoints[0].addr;

        let send = |method: &'static str, path: &'static str, body: &'static str| async move {
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: rusm\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let mut conn = TcpStream::connect(addr).await.unwrap();
            conn.write_all(req.as_bytes()).await.unwrap();
            let mut buf = Vec::new();
            conn.read_to_end(&mut buf).await.unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        };

        let hello = send("GET", "/hello/ada", "").await;
        assert!(hello.starts_with("HTTP/1.1 200"), "got: {hello}");
        assert!(hello.contains("hi ada"), "param dispatched: {hello}");
        assert!(send("POST", "/echo", "pong").await.contains("pong"), "echo");
        assert!(
            send("GET", "/nope", "").await.starts_with("HTTP/1.1 404"),
            "unmatched path is 404"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serving_a_missing_artifact_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt).unwrap();
        let specs = vec![ServeSpec {
            protocol: ServeProtocol::Http,
            listen: "127.0.0.1:0".to_string(),
            routes: HashMap::new(),
            name: Some("ghost".to_string()),
            source: None,
            headers: Default::default(),
        }];
        let err = serve_apps(dir.path(), &wasm, &specs, &BTreeMap::new(), &HashMap::new())
            .await
            .err()
            .expect("missing artifact must error");
        assert!(err.to_string().contains("ghost.wasm"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_missing_artifact_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wasm")).unwrap();
        let rt = Runtime::new();
        let wasm = WasmRuntime::new(rt).unwrap();
        let err = spawn_components(
            dir.path(),
            &wasm,
            &components(&[("absent", spec("sandboxed", false))]),
            &HashMap::new(),
        )
        .await
        .err()
        .expect("missing artifact must error");
        assert!(err.to_string().contains("absent.wasm"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawns_a_component_whose_bundle_is_sourced_from_kv() {
        // The dynamic-deploy path: a component with no local artifact, its JS bundle
        // read from the node's durable store. Put a worker that registers a name, then
        // spawn it from `kv:` and confirm it actually ran.
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new();
        let wasm = WasmRuntime::with_store(rt.clone(), dir.path().join("kv.redb")).unwrap();
        let bundle =
            b"module.exports.default = async function(){ Process.register('from-kv'); for(;;) await Process.receive(); };";
        wasm.store()
            .unwrap()
            .bucket("bundles")
            .set("greeter", bundle)
            .unwrap();

        let mut greeter = spec("sandboxed", true);
        greeter.source = Some("kv:bundles/greeter".to_string());
        let hosted = spawn_components(
            dir.path(),
            &wasm,
            &components(&[("greeter", greeter)]),
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(hosted.resident, ["greeter"]);
        // The kv-sourced bundle actually ran on the js-runner: it registered its name.
        assert!(
            await_named(&rt, "from-kv").await.is_some(),
            "the kv-sourced JS component ran and registered its name"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolves_a_bundle_from_a_url() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        // A minimal HTTP server returning a JS bundle body.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = "console.log('hi');";
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await; // consume the request head
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });

        let wasm = WasmRuntime::new(Runtime::new()).unwrap();
        let bundle = remote_bundle(Some(&format!("url:http://{addr}/x.js")), &wasm)
            .await
            .unwrap();
        assert_eq!(bundle.as_deref(), Some(body.as_bytes()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bundle_source_errors_are_clear() {
        let rt = Runtime::new();
        // No source → None (fall back to the local artifact).
        let plain = WasmRuntime::new(rt.clone()).unwrap();
        assert!(remote_bundle(None, &plain).await.unwrap().is_none());
        // A kv source with no store configured is a clear error.
        assert!(remote_bundle(Some("kv:b/k"), &plain).await.is_err());
        // A store, but a missing key.
        let dir = tempfile::tempdir().unwrap();
        let stored = WasmRuntime::with_store(rt, dir.path().join("kv.redb")).unwrap();
        assert!(remote_bundle(Some("kv:b/absent"), &stored).await.is_err());
        // An unrecognised source shape.
        assert!(remote_bundle(Some("ftp://x"), &stored).await.is_err());
    }
}
