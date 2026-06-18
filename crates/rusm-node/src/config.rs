use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::profile::ResourceProfile;

/// The `[node]` table — the node's own settings, distinct from the listeners it serves
/// (`[[serve]]`) and the components it hosts (`[components.<name>]`). Missing keys (or an
/// absent `[node]` table) fall back to these defaults.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeSettings {
    /// The attach/observer WebSocket endpoint — what `rusm attach` and the live dashboard
    /// connect to (`rusm node start` opens it). Not a serving port: each `[[serve]]`
    /// listener binds its own.
    pub listen: String,
    /// Starting resource profile (`light` / `balanced` / `max`).
    pub profile: ResourceProfile,
    /// Observer snapshot/sampling rate in Hz.
    pub ticks_per_second: u32,
    /// Path to the node's durable key-value store (one embedded file the node owns,
    /// resolved relative to the app directory). Omitted → no store: a component
    /// granted `storage` then gets an error if it uses `kv`. Set it to give resident
    /// state somewhere to survive a restart.
    pub store: Option<String>,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:4000".to_string(),
            profile: ResourceProfile::default(),
            ticks_per_second: 20,
            store: None,
        }
    }
}

/// Node startup configuration, loaded from `rusm.toml`.
///
/// Layering: these are *defaults* — the CLI applies any flags on top. Missing
/// fields fall back to the values below; unknown fields are an error (catch typos
/// early rather than silently ignore them).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeConfig {
    /// The node's own settings — the `[node]` table (endpoint, profile, store, …).
    pub node: NodeSettings,
    /// Components the app hosts, declared as `[components.<name>]` tables keyed by
    /// name (consistent with `[capabilities.<name>]`). Every entry is registered so
    /// a route or a sibling can `spawn` it by name; one marked `resident = true` is
    /// also boot-spawned and supervised. Ordered by name for deterministic boot/logs.
    pub components: BTreeMap<String, ComponentSpec>,
    /// Servers to host, declared as `[[serve]]` tables. Each loads a component
    /// from `./wasm/<name>.{wasm,js}` and serves it on a real TCP port over HTTP
    /// (also SSE) or WebSocket — what `rusm serve` runs and a load driver hits.
    pub serve: Vec<ServeSpec>,
    /// Custom capability profiles, declared as `[capabilities.<name>]` tables. A
    /// component's `capability = "<name>"` resolves to one of these first, then to
    /// the built-in profiles (`sandboxed` / `network-client` / `trusted`).
    pub capabilities: HashMap<String, CapabilitySpec>,
    /// Platform logging, the `[log]` table — explicit, off by default.
    pub log: LogConfig,
}

/// The `[log]` table: opt-in **platform logging**, off by default. One node-wide `level`
/// gates three streams at once — the runtime's lifecycle log (spawn/exit/crash), guest
/// logs (`console.*` / the `log` crate), and the serving access log (HTTP/SSE/WS) — all
/// coloured and tagged `rusm` or `component#pid`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// `off` (default) / `error` (crashes) / `warn` (+ kills) / `info` (+ clean exits,
    /// guest info logs, every served request) / `debug` (+ every spawn, guest debug).
    /// Anything unrecognised is `off`.
    #[serde(default)]
    pub level: String,
}

impl NodeConfig {
    /// The configured platform-log level (`[log] level`), parsed — `Off` when unset.
    pub fn log_level(&self) -> rusm_otp::LogLevel {
        rusm_otp::LogLevel::parse(&self.log.level)
    }
}

/// A custom capability profile (`[capabilities.<name>]`) — mirrors Cargo's
/// `[profile.<name>]`: it `inherits` a built-in base (default `sandboxed`,
/// default-deny) and overrides specific grants. Only set fields override the base,
/// so a profile close to a preset stays terse. Referenced by a component's
/// `capability = "<name>"`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct CapabilitySpec {
    /// Built-in profile to start from (`sandboxed` / `network-client` / `trusted`);
    /// omitted (or unrecognised) → `sandboxed`, the most restrictive base.
    pub inherits: Option<String>,
    /// `allow-network`: outbound network access (e.g. `fetch`).
    pub allow_network: Option<bool>,
    /// `allow-spawn`: may spawn **other** components by name (the `spawn` ABI).
    pub allow_spawn: Option<bool>,
    /// `allow-process-control`: may observe + control **other** processes —
    /// `monitor`, `kill`, `list`, `info` over foreign pids.
    pub allow_process_control: Option<bool>,
    /// `allow-stdio`: inherit the host's stdio (so `print`/`console.log` reach the
    /// terminal).
    pub allow_stdio: Option<bool>,
    /// `allow-storage`: durable key-value storage (the `kv-*` actor ABI), if the node
    /// has a `store` configured. Default-deny.
    pub allow_storage: Option<bool>,
    /// Per-process memory ceiling in MiB.
    pub max_memory_mb: Option<usize>,
    /// Environment-variable keys to grant; values are resolved from the process
    /// environment (process env, then `.env`) at load — keys with no value are skipped.
    #[serde(default)]
    pub env: Vec<String>,
    /// Host directories to preopen inside the sandbox.
    #[serde(default)]
    pub preopen: Vec<PreopenSpec>,
}

/// One `preopen` entry of a [`CapabilitySpec`]: a host directory mounted at `guest`
/// inside the sandbox, read-only unless `read-only = false`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PreopenSpec {
    pub host: String,
    pub guest: String,
    #[serde(default)]
    pub read_only: bool,
}

// `CapabilitySpec::to_capabilities()` (the conversion to a concrete
// `rusm_wasm::Capabilities`) lives in the CLI, the only consumer that links
// `rusm-wasm` — keeping this manifest crate free of the Wasm backend.

/// One `[components.<name>]` entry: a component the app hosts under a capability
/// profile. The table key is the component name (resolving the `./wasm/<name>`
/// artifact); every entry is registered as spawnable-by-name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSpec {
    /// Capability profile id (`sandboxed` / `network-client` / `trusted`).
    #[serde(default = "default_capability")]
    pub capability: String,
    /// A long-lived service: boot-spawn it at startup and supervise it (auto-restart
    /// on crash, bounded by the runtime's restart-intensity). Off by default — a
    /// component without `resident` is registered only, spawned on demand by a route
    /// or a sibling (e.g. a per-request handler), never boot-parked.
    #[serde(default)]
    pub resident: bool,
    /// Where to load the (JS) bundle from instead of the local `./wasm/<name>`
    /// artifact: a `url:`/`http(s)://` URL or a `kv:<bucket>/<key>` store entry
    /// (see [`BundleSource`]). Omitted → the local artifact. Lets JS deploy live —
    /// change the bundle at the source and re-`spawn`/reload, no node rebuild.
    #[serde(default)]
    pub source: Option<String>,
    /// Marks this as a **dynamic JS runner template** (`dynamic = "js"`): a capability
    /// profile with no fixed bundle. A guest can't `spawn` it; it runs only via
    /// `spawn-from(name, source)`, which loads a runtime-chosen JS bundle and runs it
    /// under this profile (operator policy — the guest picks the code, never the caps).
    /// Mutually exclusive with `source` (a template has no fixed bundle).
    #[serde(default)]
    pub dynamic: Option<String>,
}

/// Where a component's JS bundle is fetched from when a [`ComponentSpec`]/[`ServeSpec`]
/// sets `source`, beyond the default local `./wasm/<name>` artifact:
/// - `http(s)://…` (or `url:<u>`) — an HTTP(S) URL (e.g. a presigned blob / artifact API),
/// - `kv:<bucket>/<key>` — an entry in the node's durable store ([`crate`]'s `store`).
///
/// Parsing is pure (this type carries no I/O); the app loader resolves it to bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleSource {
    /// Fetch over HTTP(S).
    Url(String),
    /// Read `key` from `bucket` in the node's durable key-value store.
    Kv { bucket: String, key: String },
}

impl BundleSource {
    /// Parse a manifest `source` string. `kv:<bucket>/<key>` → [`Kv`](Self::Kv);
    /// `url:<u>` or a bare `http(s)://…` → [`Url`](Self::Url). Any other shape is a
    /// (human-readable) error, so a typo is caught at load rather than silently
    /// falling back to a local file.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if let Some(rest) = spec.strip_prefix("kv:") {
            let (bucket, key) = rest
                .split_once('/')
                .ok_or_else(|| format!("kv source must be `kv:<bucket>/<key>`, got {spec:?}"))?;
            if bucket.is_empty() || key.is_empty() {
                return Err(format!(
                    "kv source needs a non-empty bucket and key: {spec:?}"
                ));
            }
            return Ok(BundleSource::Kv {
                bucket: bucket.to_string(),
                key: key.to_string(),
            });
        }
        let url = spec.strip_prefix("url:").unwrap_or(spec);
        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(BundleSource::Url(url.to_string()));
        }
        Err(format!(
            "unrecognised bundle source {spec:?} (expected `http(s)://…` or `kv:<bucket>/<key>`)"
        ))
    }
}

fn default_capability() -> String {
    "sandboxed".to_string()
}

/// The wire protocol a `[[serve]]` entry is hosted over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServeProtocol {
    /// Request/response HTTP/1.1 — a `wasi:http` component, one instance per request.
    Http,
    /// Server-Sent Events: an HTTP component that streams a `text/event-stream`
    /// body. Hosted by the same server as [`Http`](Self::Http), but the tag is
    /// load-bearing: it makes the host enforce SSE transport correctness (add
    /// `Cache-Control: no-cache`, and fail loud if a reply isn't `text/event-stream`),
    /// drives the CLI probe hint, and lets a load driver pick the streaming scenario.
    Sse,
    /// WebSocket — one sandboxed component process per connection.
    Ws,
}

impl ServeProtocol {
    /// Whether this protocol is hosted by the HTTP server (`http_server`). Both
    /// plain HTTP and SSE are; only WebSocket uses a different server.
    pub fn is_http(self) -> bool {
        matches!(self, Self::Http | Self::Sse)
    }

    /// Whether this listener is declared for Server-Sent Events — so the host enforces
    /// the `text/event-stream` contract and SSE caching rules on its replies.
    pub fn is_sse(self) -> bool {
        matches!(self, Self::Sse)
    }
}

/// One `[[serve]]` entry: a network listener hosted on its own port. HTTP/SSE
/// listeners dispatch each request through the `[routes]` table to a handler
/// component (process-per-request); a WebSocket listener runs one sandboxed component
/// process per connection (loaded from `./wasm/<name>.{wasm,js}`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeSpec {
    /// Wire protocol to host over (`http` / `sse` / `ws`).
    pub protocol: ServeProtocol,
    /// TCP address to bind, e.g. `"127.0.0.1:8080"` (the real serving port).
    pub listen: String,
    /// Declarative HTTP routing for **this** listener, the `[serve.routes]` table:
    /// `"METHOD /path/:param" = "component#action"`. The gateway resolves each request
    /// to a `[components.<name>]` handler + action (with path params) and dispatches it
    /// per-request. This is the usual HTTP/SSE shape; ignored for `protocol = "ws"`.
    #[serde(default)]
    pub routes: HashMap<String, String>,
    /// The single handler **component** for a listener that has no `[serve.routes]`: a
    /// WebSocket listener (one process per connection) or a handler-less `wasi:http`
    /// HTTP component. Resolves to `./wasm/<name>.{wasm,js}`; its capability profile
    /// comes from a matching `[components.<name>]` entry (else default-deny `sandboxed`).
    /// Omitted for a routed HTTP/SSE listener — its routes name the components.
    #[serde(default)]
    pub name: Option<String>,
    /// Load the WS/HTTP component's (JS) bundle from a `url:`/`http(s)://` URL or
    /// `kv:<bucket>/<key>` instead of `./wasm/<name>` (see [`BundleSource`]).
    #[serde(default)]
    pub source: Option<String>,
}

impl ServeSpec {
    /// The compiled [`RouteTable`] for this listener's `[serve.routes]` map (errors on a
    /// malformed entry). Empty when no routes are declared.
    pub fn route_table(&self) -> Result<crate::routes::RouteTable, String> {
        crate::routes::RouteTable::from_map(&self.routes)
    }
}

impl NodeConfig {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Loads a manifest from `path`. A `required` (explicitly requested) file that
    /// is missing is an error; a missing optional file (the default `rusm.toml`)
    /// yields [`NodeConfig::default`]. Invalid TOML is always an error. The
    /// returned message is human-readable, ready to print.
    pub fn load(path: &Path, required: bool) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                Self::from_toml(&text).map_err(|e| format!("invalid {}: {e}", path.display()))
            }
            Err(_) if !required => Ok(Self::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_all_defaults() {
        assert_eq!(NodeConfig::from_toml("").unwrap(), NodeConfig::default());
    }

    #[test]
    fn load_reads_file_defaults_when_optional_and_errors_when_required() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rusm.toml");
        std::fs::write(&path, "[node]\nlisten = \"0.0.0.0:9000\"\n").unwrap();
        // An explicit, valid file is parsed.
        assert_eq!(
            NodeConfig::load(&path, true).unwrap().node.listen,
            "0.0.0.0:9000"
        );
        // A missing optional file → defaults; a missing required file → error.
        let missing = dir.path().join("absent.toml");
        assert_eq!(
            NodeConfig::load(&missing, false).unwrap(),
            NodeConfig::default()
        );
        assert!(NodeConfig::load(&missing, true).is_err());
        // Invalid TOML always errors.
        std::fs::write(&path, "nope = 1\n").unwrap();
        assert!(NodeConfig::load(&path, true).is_err());
    }

    #[test]
    fn parses_a_full_file() {
        let cfg = NodeConfig::from_toml(
            r#"
            [node]
            listen = "0.0.0.0:9000"
            profile = "max"
            ticks_per_second = 60
            "#,
        )
        .unwrap();
        assert_eq!(cfg.node.listen, "0.0.0.0:9000");
        assert_eq!(cfg.node.profile, ResourceProfile::Max);
        assert_eq!(cfg.node.ticks_per_second, 60);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg = NodeConfig::from_toml("[node]\nprofile = \"light\"").unwrap();
        assert_eq!(cfg.node.profile, ResourceProfile::Light);
        assert_eq!(cfg.node.listen, NodeConfig::default().node.listen); // default kept
    }

    #[test]
    fn unknown_field_is_an_error() {
        assert!(NodeConfig::from_toml("nope = 1").is_err());
    }

    #[test]
    fn bare_node_keys_are_rejected_now_they_live_under_node() {
        // The pre-`[node]` layout (bare top-level keys) is no longer valid — a clear
        // error, not a silent ignore, so an un-migrated rusm.toml fails fast.
        assert!(NodeConfig::from_toml("listen = \"a:1\"").is_err());
        assert!(NodeConfig::from_toml("store = \"x.redb\"").is_err());
    }

    #[test]
    fn invalid_profile_is_an_error() {
        assert!(NodeConfig::from_toml("[node]\nprofile = \"turbo\"").is_err());
    }

    #[test]
    fn parses_component_manifest_with_defaults() {
        let cfg = NodeConfig::from_toml(
            r#"
            [components.source]
            capability = "network-client"

            [components.sink]
            resident = true
            "#,
        )
        .unwrap();
        assert_eq!(cfg.components.len(), 2);
        assert_eq!(cfg.components["source"].capability, "network-client");
        assert!(!cfg.components["source"].resident);
        // capability defaults to sandboxed; resident parsed.
        assert_eq!(cfg.components["sink"].capability, "sandboxed");
        assert!(cfg.components["sink"].resident);
    }

    #[test]
    fn no_components_by_default() {
        assert!(NodeConfig::from_toml("").unwrap().components.is_empty());
    }

    #[test]
    fn unknown_component_field_is_an_error() {
        let toml = "[components.x]\nnope = 1\n";
        assert!(NodeConfig::from_toml(toml).is_err());
    }

    #[test]
    fn parses_custom_capability_profiles() {
        let cfg = NodeConfig::from_toml(
            r#"
            [capabilities.agent]
            inherits = "network-client"
            allow-spawn = true
            max-memory-mb = 256
            preopen = [{ host = "./data", guest = "/data", read-only = false }]

            [components.pages-agent]
            capability = "agent"
            "#,
        )
        .unwrap();
        let spec = &cfg.capabilities["agent"];
        assert_eq!(spec.inherits.as_deref(), Some("network-client"));
        assert_eq!(spec.allow_spawn, Some(true));
        assert_eq!(spec.max_memory_mb, Some(256));
        assert_eq!(spec.preopen.len(), 1);
        assert!(!spec.preopen[0].read_only);
        assert_eq!(spec.allow_storage, None, "storage defaults to unset (deny)");
    }

    #[test]
    fn parses_bundle_source_field_on_components_and_serve() {
        let cfg = NodeConfig::from_toml(
            r#"
            [components.api]
            source = "kv:bundles/api"

            [[serve]]
            name = "web"
            protocol = "http"
            listen = "127.0.0.1:8080"
            source = "https://cdn.example/web.js"
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.components["api"].source.as_deref(),
            Some("kv:bundles/api")
        );
        assert_eq!(
            cfg.serve[0].source.as_deref(),
            Some("https://cdn.example/web.js")
        );
        // Default: no source (use the local ./wasm artifact).
        assert!(NodeConfig::from_toml("[components.x]\n")
            .unwrap()
            .components["x"]
            .source
            .is_none());
    }

    #[test]
    fn parses_a_dynamic_js_runner_template() {
        // `dynamic = "js"` marks a runner template (a profile, no fixed bundle) a guest
        // reaches via spawn-from; absent by default.
        let cfg = NodeConfig::from_toml(
            "[components.sandbox-runner]\ncapability = \"network-client\"\ndynamic = \"js\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.components["sandbox-runner"].dynamic.as_deref(),
            Some("js")
        );
        assert!(NodeConfig::from_toml("[components.x]\n")
            .unwrap()
            .components["x"]
            .dynamic
            .is_none());
    }

    #[test]
    fn bundle_source_parses_url_and_kv_and_rejects_garbage() {
        use BundleSource::*;
        assert_eq!(
            BundleSource::parse("https://cdn/x.js"),
            Ok(Url("https://cdn/x.js".into()))
        );
        assert_eq!(
            BundleSource::parse("url:http://h/x.js"),
            Ok(Url("http://h/x.js".into())) // the `url:` prefix is stripped
        );
        assert_eq!(
            BundleSource::parse("kv:bundles/api"),
            Ok(Kv {
                bucket: "bundles".into(),
                key: "api".into()
            })
        );
        // A key may itself contain slashes (split on the first only).
        assert_eq!(
            BundleSource::parse("kv:b/a/b/c"),
            Ok(Kv {
                bucket: "b".into(),
                key: "a/b/c".into()
            })
        );
        for bad in [
            "kv:nokey",
            "kv:/key",
            "kv:bucket/",
            "ftp://x",
            "./wasm/x.js",
            "",
        ] {
            assert!(
                BundleSource::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn parses_store_and_storage_capability() {
        let cfg = NodeConfig::from_toml(
            r#"
            [node]
            store = "data/app.redb"

            [capabilities.stateful]
            inherits = "trusted"
            allow-storage = true
            "#,
        )
        .unwrap();
        assert_eq!(cfg.node.store.as_deref(), Some("data/app.redb"));
        assert_eq!(cfg.capabilities["stateful"].allow_storage, Some(true));
        // Default: no store configured.
        assert!(NodeConfig::from_toml("").unwrap().node.store.is_none());
    }

    #[test]
    fn unknown_capability_field_is_an_error() {
        assert!(NodeConfig::from_toml("[capabilities.x]\nnope = true\n").is_err());
    }

    #[test]
    fn parses_serve_manifest_with_defaults() {
        let cfg = NodeConfig::from_toml(
            r#"
            [[serve]]
            protocol = "http"
            listen = "127.0.0.1:8080"

            [[serve]]
            name = "echo"
            protocol = "ws"
            listen = "0.0.0.0:8081"

            [[serve]]
            protocol = "sse"
            listen = "127.0.0.1:8082"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.serve.len(), 3);
        // A pure listener: protocol + listen, no name (routed HTTP).
        assert_eq!(cfg.serve[0].protocol, ServeProtocol::Http);
        assert_eq!(cfg.serve[0].listen, "127.0.0.1:8080");
        assert!(cfg.serve[0].name.is_none());
        // A WS listener names its per-connection component.
        assert_eq!(cfg.serve[1].protocol, ServeProtocol::Ws);
        assert_eq!(cfg.serve[1].name.as_deref(), Some("echo"));
        // SSE is an HTTP-hosted server; WS is not.
        assert_eq!(cfg.serve[2].protocol, ServeProtocol::Sse);
        assert!(cfg.serve[0].protocol.is_http() && cfg.serve[2].protocol.is_http());
        assert!(!cfg.serve[1].protocol.is_http());
        // ...but only the SSE listener enforces the event-stream contract.
        assert!(cfg.serve[2].protocol.is_sse());
        assert!(!cfg.serve[0].protocol.is_sse() && !cfg.serve[1].protocol.is_sse());
    }

    #[test]
    fn parses_per_listener_routes() {
        // Routes are scoped to their listener: `[serve.routes]` attaches to the
        // preceding `[[serve]]` entry, so each port carries its own table.
        let cfg = NodeConfig::from_toml(
            r#"
            [[serve]]
            name = "api"
            protocol = "http"
            listen = "127.0.0.1:8080"

            [serve.routes]
            "GET /" = "api#home"
            "POST /users/:id" = "api#update"

            [[serve]]
            name = "admin"
            protocol = "http"
            listen = "127.0.0.1:9090"

            [serve.routes]
            "GET /health" = "admin#health"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.serve.len(), 2);
        // Each listener compiles its own table; the matcher itself is unit-tested in `routes.rs`.
        assert!(!cfg.serve[0]
            .route_table()
            .expect("api routes compile")
            .is_empty());
        assert!(!cfg.serve[1]
            .route_table()
            .expect("admin routes compile")
            .is_empty());
        // A listener with no `[serve.routes]` yields an empty table (the wasi:http path).
        assert!(NodeConfig::default()
            .serve
            .first()
            .map_or(true, |s| s.route_table().unwrap().is_empty()));
    }

    #[test]
    fn no_servers_by_default() {
        assert!(NodeConfig::from_toml("").unwrap().serve.is_empty());
    }

    #[test]
    fn log_level_defaults_off_and_parses_the_log_table() {
        // Default: no `[log]` → Off (explicit opt-in; nothing logs by surprise).
        assert_eq!(NodeConfig::default().log_level(), rusm_otp::LogLevel::Off);
        // Declared level parses; an unknown value quiets to Off rather than erroring.
        let cfg = NodeConfig::from_toml("[log]\nlevel = \"debug\"\n").unwrap();
        assert_eq!(cfg.log_level(), rusm_otp::LogLevel::Debug);
        let bad = NodeConfig::from_toml("[log]\nlevel = \"loud\"\n").unwrap();
        assert_eq!(bad.log_level(), rusm_otp::LogLevel::Off);
    }

    #[test]
    fn unknown_serve_protocol_is_an_error() {
        let toml = "[[serve]]\nname = \"x\"\nprotocol = \"grpc\"\nlisten = \"127.0.0.1:1\"\n";
        assert!(NodeConfig::from_toml(toml).is_err());
    }

    #[test]
    fn serve_requires_a_listen_address() {
        let toml = "[[serve]]\nname = \"x\"\nprotocol = \"http\"\n";
        assert!(NodeConfig::from_toml(toml).is_err());
    }

    #[test]
    fn unknown_serve_field_is_an_error() {
        let toml = "[[serve]]\nname = \"x\"\nprotocol = \"http\"\nlisten = \"a:1\"\nnope = 1\n";
        assert!(NodeConfig::from_toml(toml).is_err());
    }
}
