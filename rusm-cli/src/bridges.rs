//! Discovery of an app's **custom bridges** — the `bridges/<name>/` directories an
//! application carries in its own repo, authored in the exact shape RUSM's own platform
//! bridges use (`bridge.wit` + `host.rs`). Presence *is* the declaration: a well-formed
//! `bridges/<name>/` is a bridge, no manifest entry required. Whether a *component* may
//! reach one is the separate, default-deny capability whitelist (`[capabilities.<name>]
//! bridges = [...]`, see `caps.rs`).
//!
//! Discovery is deliberately structural — it locates and validates directories; it does
//! not parse WIT. A bridge's `host.rs` encapsulates its own `bindgen!` + `add_to_linker`
//! (dogfooding the platform-bridge convention), so the toolchain never needs to understand
//! the contract, only to find it and hand it to the guest build + the generated host crate.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use wit_parser::Resolve;

/// A discovered custom bridge: its `name` (the directory name, which is also the bridge
/// name used in the capability whitelist) and the `dir` holding `bridge.wit` + `host.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeSpec {
    pub name: String,
    pub dir: PathBuf,
}

impl BridgeSpec {
    /// The WIT contract (`bridges/<name>/bridge.wit`) — vendored into each granted guest
    /// component's `wit/deps/` and into the generated host crate.
    pub fn wit(&self) -> PathBuf {
        self.dir.join("bridge.wit")
    }

    /// The native host impl (`bridges/<name>/host.rs`) — `impl <iface>::Host for BridgeHost`
    /// plus a `pub fn add_to_linker`, compiled into the generated host crate.
    pub fn host(&self) -> PathBuf {
        self.dir.join("host.rs")
    }
}

/// Discover the custom bridges under `<root>/bridges/`. Returns them sorted by name (a
/// stable order, so generated code is deterministic). No `bridges/` directory → no custom
/// bridges (an empty list, not an error — most apps have none). A `bridges/<name>/` that is
/// missing `bridge.wit` or `host.rs` is a **malformed** bridge and fails loudly, rather
/// than being silently skipped — a half-authored bridge is a mistake, not a non-bridge.
pub fn discover(root: &Path) -> Result<Vec<BridgeSpec>> {
    let dir = root.join("bridges");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut bridges = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue; // ignore stray files (e.g. a README) directly under bridges/
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let spec = BridgeSpec {
            name: name.clone(),
            dir: path,
        };
        if !spec.wit().is_file() {
            bail!("custom bridge `{name}` is missing bridges/{name}/bridge.wit");
        }
        if !spec.host().is_file() {
            bail!("custom bridge `{name}` is missing bridges/{name}/host.rs");
        }
        bridges.push(spec);
    }
    bridges.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(bridges)
}

/// A custom bridge's parsed WIT contract — its package coordinates and the func-bearing
/// interfaces a guest imports. Extracted from `bridge.wit` to synthesize the host crate's
/// `bindgen!` world (`import <namespace>:<name>/<iface>@<version>;`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub namespace: String,
    pub name: String,
    pub version: Option<String>,
    /// Interfaces that declare at least one `func` (the importable capabilities); a
    /// types-only interface is `use`d, not imported, so it's excluded.
    pub interfaces: Vec<String>,
}

impl Contract {
    /// The fully-qualified WIT references a guest/host imports, one per func-bearing
    /// interface: `weather:bridge/forecast@0.1.0` (version omitted if the package has none).
    pub fn interface_refs(&self) -> Vec<String> {
        let suffix = self
            .version
            .as_deref()
            .map(|v| format!("@{v}"))
            .unwrap_or_default();
        self.interfaces
            .iter()
            .map(|iface| format!("{}:{}/{}{}", self.namespace, self.name, iface, suffix))
            .collect()
    }
}

/// Parse a custom bridge's `bridge.wit` into its [`Contract`]. The bridge must be a
/// **self-contained** WIT package (its own types) — a contract that `use`s another package
/// (e.g. `rusm:runtime/types`) fails here with the parser's error, rather than silently
/// half-resolving (cross-package shared types in a custom bridge is a documented future
/// extension, not a quiet gap).
pub fn parse_contract(wit: &Path) -> Result<Contract> {
    let mut resolve = Resolve::new();
    let (pkg_id, _) = resolve
        .push_path(wit)
        .with_context(|| format!("parsing bridge WIT {}", wit.display()))?;
    let pkg = &resolve.packages[pkg_id];
    let interfaces = pkg
        .interfaces
        .iter()
        .filter(|(_, &iface_id)| !resolve.interfaces[iface_id].functions.is_empty())
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if interfaces.is_empty() {
        bail!(
            "bridge WIT {} declares no interface with a function — nothing to import",
            wit.display()
        );
    }
    Ok(Contract {
        namespace: pkg.name.namespace.clone(),
        name: pkg.name.name.clone(),
        version: pkg.name.version.as_ref().map(|v| v.to_string()),
        interfaces,
    })
}

/// One custom bridge function the generated js-runner glue exposes to a TS guest: which
/// interface + function it is, and its parameter names. (v1 supports `string` params and a
/// `string`/no result — the example's shape and most provider bridges; richer WIT types are a
/// documented follow-on that fails loudly, never silently.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeFn {
    pub interface: String,
    pub func: String,
    pub params: Vec<String>,
    /// Whether the function returns a `string` (vs no result) — the JS wrapper returns the
    /// value or `undefined` accordingly.
    pub returns_string: bool,
}

/// Parse a bridge's `bridge.wit` into the functions a TS guest can call. Validates that every
/// param is `string` and each result is `string` or none — the v1 TS type surface; a richer
/// type fails **loudly** here, naming the function + the Rust/Go-guest workaround, so a TS
/// guest never silently gets a half-typed bridge.
pub fn bridge_functions(wit: &Path) -> Result<Vec<BridgeFn>> {
    let mut resolve = Resolve::new();
    let (pkg_id, _) = resolve
        .push_path(wit)
        .with_context(|| format!("parsing bridge WIT {}", wit.display()))?;
    let pkg = &resolve.packages[pkg_id];
    let mut out = Vec::new();
    for (iface_name, &iface_id) in &pkg.interfaces {
        for (fn_name, function) in &resolve.interfaces[iface_id].functions {
            let unsupported = |what: &str| {
                anyhow::anyhow!(
                    "custom bridge function `{iface_name}/{fn_name}` {what} — TS guests support \
                     `string` params and a `string`/no result today; call it from a Rust or Go \
                     guest for richer types, or keep the bridge's TS-facing functions to strings"
                )
            };
            let mut params = Vec::new();
            for param in &function.params {
                if param.ty != wit_parser::Type::String {
                    return Err(unsupported(&format!(
                        "has a non-string param `{}`",
                        param.name
                    )));
                }
                params.push(param.name.clone());
            }
            let returns_string = match &function.result {
                None => false,
                Some(wit_parser::Type::String) => true,
                Some(_) => return Err(unsupported("has a non-string result")),
            };
            out.push(BridgeFn {
                interface: iface_name.clone(),
                func: fn_name.clone(),
                params,
                returns_string,
            });
        }
    }
    Ok(out)
}

/// A WIT identifier (namespace / package / interface / bridge name) as a Rust/JS identifier:
/// kebab → snake (`json-codec` → `json_codec`). The js-runner's generated WIT bindings + the
/// `__<bridge>__<func>` primitives use this form.
fn ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Generate the per-app js-runner's `bridges_gen.rs` (the [`register`] host primitives + the
/// `BRIDGE_JS` TS API) for every custom bridge. Each function becomes a typed
/// `__<bridge>__<func>` primitive that calls the *typed* wit-bindgen binding (no dispatcher),
/// and `globalThis.<bridge>.<func>` wraps it. Mirrors the committed empty `bridges_gen.rs`, so
/// the per-app runner compiles with the same module shape.
///
/// [`register`]: it's the generated function the runner's `boot_bridge` calls.
pub fn gen_runner_bridges_gen(bridges: &[BridgeSpec]) -> Result<String> {
    let mut registers = String::new();
    let mut js = String::new();
    for bridge in bridges {
        let contract = parse_contract(&bridge.wit())?;
        let (ns, pkg) = (ident(&contract.namespace), ident(&contract.name));
        let object = ident(&bridge.name);
        js.push_str(&format!(
            "globalThis.{object} = globalThis.{object} || {{}};\n"
        ));
        for f in bridge_functions(&bridge.wit())? {
            let iface = ident(&f.interface);
            let prim = format!("__{object}__{}", ident(&f.func));
            let func = ident(&f.func);
            // Rust glue: a closure taking each string param, calling the typed binding.
            let closure_params = f
                .params
                .iter()
                .map(|p| format!("{}: String", ident(p)))
                .collect::<Vec<_>>()
                .join(", ");
            let call_args = f
                .params
                .iter()
                .map(|p| format!("&{}", ident(p)))
                .collect::<Vec<_>>()
                .join(", ");
            registers.push_str(&format!(
                "    globals\n\
                 \x20       .set(\n\
                 \x20           \"{prim}\",\n\
                 \x20           Function::new(ctx.clone(), |{closure_params}| {{\n\
                 \x20               crate::{ns}::{pkg}::{iface}::{func}({call_args})\n\
                 \x20           }})\n\
                 \x20           .expect(\"{prim}\"),\n\
                 \x20       )\n\
                 \x20       .expect(\"{prim}\");\n"
            ));
            // JS wrapper: `globalThis.<bridge>.<func> = (params) => __<bridge>__<func>(params)`.
            let js_params = f
                .params
                .iter()
                .map(|p| ident(p))
                .collect::<Vec<_>>()
                .join(", ");
            js.push_str(&format!(
                "globalThis.{object}.{func} = ({js_params}) => {prim}({js_params});\n"
            ));
        }
    }
    Ok(format!(
        "//! GENERATED by `rusm build` — do not edit. Per-app custom-bridge glue for the\n\
         //! js-runner: each `__<bridge>__<func>` primitive calls the typed wit-bindgen binding,\n\
         //! and `BRIDGE_JS` exposes `globalThis.<bridge>`. Regenerated from `bridges/` each build.\n\
         use rquickjs::{{Ctx, Function, Object}};\n\
         \n\
         pub fn register(ctx: &Ctx<'_>, globals: &Object<'_>) {{\n\
         {registers}}}\n\
         \n\
         pub const BRIDGE_JS: &str = {js:?};\n"
    ))
}

/// The static `src/bindings.rs` of a generated host crate: one `bindgen!` over the
/// synthesized `wit/` world, producing the typed `Host` traits a bridge's `host.rs`
/// implements over [`rusm_wasm::BridgeHost`]. Identical for every app (the variation is in
/// the `wit/` world), so it's a constant — emitted as a file only so the layout reads like
/// the platform's own `crate::bindings`.
pub const BINDINGS_RS: &str = "\
//! GENERATED by `rusm build` — do not edit. Typed bindings for the app's custom bridges,
//! from the synthesized `wit/` world. Each bridge's `host.rs` implements these `Host`
//! traits over `rusm_wasm::BridgeHost`. `wasmtime` is the runtime's exact version (pinned
//! in Cargo.toml), so the generated types are identical to the ones the runtime links.
wasmtime::component::bindgen!({
    path: \"wit\",
    world: \"host\",
    imports: { default: async },
});
";

/// Synthesize the host crate's `wit/world.wit`: a `rusm:host` world that `import`s every
/// func-bearing interface of every custom bridge. The bridges' own packages are vendored
/// beside it under `wit/deps/<name>/`, so this world resolves against them.
pub fn synth_world(contracts: &[Contract]) -> String {
    let mut out = String::from(
        "// GENERATED by `rusm build` — do not edit. The bindgen world over the app's\n\
         // custom bridges; each `import` resolves against a package vendored in deps/.\n\
         package rusm:host@0.1.0;\n\nworld host {\n",
    );
    for contract in contracts {
        for iface in contract.interface_refs() {
            out.push_str(&format!("    import {iface};\n"));
        }
    }
    out.push_str("}\n");
    out
}

/// Synthesize the host crate's `src/bridges.rs`: it mounts each bridge's `host.rs` as a
/// module (`#[path]` to the app's `bridges/<name>/host.rs` — the source of truth stays in
/// the app, never copied) and exposes [`extend`], which registers every bridge's
/// `add_to_linker`. `extend` is what `main.rs` hands to `rusm_cli::host::serve`.
pub fn gen_bridges_module(names: &[&str]) -> String {
    let mut out = String::from(
        "//! GENERATED by `rusm build` — do not edit. Mounts each custom bridge's host impl\n\
         //! and registers them all. Pass `extend` to `rusm_cli::host::serve`.\n\n",
    );
    for name in names {
        let ident = module_ident(name);
        out.push_str(&format!(
            "#[path = \"../bridges/{name}/host.rs\"]\npub mod {ident};\n\n"
        ));
    }
    out.push_str(
        "/// Register every custom application bridge into the component linker.\n\
         pub fn extend(linker: &mut rusm_wasm::BridgeLinker) -> rusm_wasm::wasmtime::Result<()> {\n",
    );
    for name in names {
        out.push_str(&format!(
            "    {}::add_to_linker(linker)?;\n",
            module_ident(name)
        ));
    }
    out.push_str("    Ok(())\n}\n");
    out
}

/// A bridge directory name as a Rust module identifier: hyphens → underscores (a bridge dir
/// may be kebab-case, a Rust module may not). The `#[path]` keeps the real directory name.
fn module_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Whether `root` is a **custom-bridge app** — it carries a `bridges/` directory. Such an
/// app serves via its own host binary (which has the bridge impls compiled in); a pure-guest
/// app (no `bridges/`) is hosted by the prebuilt `rusm` directly.
pub fn has_bridges(root: &Path) -> bool {
    root.join("bridges").is_dir()
}

/// (Re)generate a custom-bridge app's host glue from its `bridges/<name>/` — the files
/// `rusm build` owns and overwrites every run: `wit/world.wit` (the synthesized bindgen
/// world), `wit/deps/<name>/bridge.wit` (each contract vendored beside it), `src/bindings.rs`,
/// and `src/bridges.rs`. The app author owns everything else (`src/main.rs`, `Cargo.toml`,
/// `bridges/<name>/host.rs`). Returns the discovered bridges (empty → nothing written).
pub fn generate_host_files(root: &Path) -> Result<Vec<BridgeSpec>> {
    let bridges = discover(root)?;
    if bridges.is_empty() {
        return Ok(bridges);
    }
    let contracts = bridges
        .iter()
        .map(|b| parse_contract(&b.wit()))
        .collect::<Result<Vec<_>>>()?;
    let names: Vec<&str> = bridges.iter().map(|b| b.name.as_str()).collect();

    let wit = root.join("wit");
    std::fs::create_dir_all(wit.join("deps"))
        .with_context(|| format!("creating {}", wit.join("deps").display()))?;
    std::fs::write(wit.join("world.wit"), synth_world(&contracts))?;
    for bridge in &bridges {
        let dep = wit.join("deps").join(&bridge.name);
        std::fs::create_dir_all(&dep)?;
        std::fs::copy(bridge.wit(), dep.join("bridge.wit"))
            .with_context(|| format!("vendoring {} bridge.wit into wit/deps/", bridge.name))?;
    }

    let src = root.join("src");
    std::fs::create_dir_all(&src).with_context(|| format!("creating {}", src.display()))?;
    std::fs::write(src.join("bindings.rs"), BINDINGS_RS)?;
    std::fs::write(src.join("bridges.rs"), gen_bridges_module(&names))?;
    Ok(bridges)
}

/// Vendor a bridge's contract into a **guest component**'s WIT tree, at
/// `<component_dir>/wit/deps/<name>/bridge.wit`, so the guest can `import` it (the author
/// still declares the import in the component's own world + `generate!` — vendoring only
/// makes the dependency resolvable). Idempotent; only meaningful for a wit-based guest
/// (Rust/Go). A TS guest runs on the js-runner and needs no per-component WIT.
pub fn vendor_into_component(component_dir: &Path, bridge: &BridgeSpec) -> Result<()> {
    let dep = component_dir.join("wit").join("deps").join(&bridge.name);
    std::fs::create_dir_all(&dep).with_context(|| format!("creating {}", dep.display()))?;
    std::fs::copy(bridge.wit(), dep.join("bridge.wit")).with_context(|| {
        format!(
            "vendoring `{}` into {}",
            bridge.name,
            component_dir.display()
        )
    })?;
    Ok(())
}

/// The canonical `rusm:runtime` WIT, embedded directly from `rusm-rs` (not a vendored copy —
/// `include_str!` of the one source, so it can never drift). `rusm build` writes it into a
/// generated guest component's `wit/deps/rusm-runtime/`, so the component's world resolves the
/// same `rusm:runtime` interfaces the `rusm-rs` SDK is generated from (identical types).
pub const RUNTIME_WIT: &str = include_str!("../../crates/rusm-rs/wit/world.wit");

/// The func-bearing `rusm:runtime` interfaces a component imports (the `process` world's
/// imports; `types` is `use`d, not imported).
const RUNTIME_INTERFACES: [&str; 6] = ["actor", "kv", "log", "pg", "serve", "streams"];

/// Synthesize the `wit/world.wit` for a **generated guest component** that imports custom
/// bridges: the `rusm:runtime` interfaces (the `rusm-rs` SDK binds these) plus each granted
/// bridge interface, exporting `run`. The `rusm-rs` `#[handlers]`/`#[main]` macro binds this
/// world (mapping `rusm:runtime` to the SDK, `generate_all` for the custom bridges).
pub fn component_world(custom_refs: &[String]) -> String {
    let mut out = String::from(
        "// GENERATED by `rusm build` — do not edit. The world for a guest component that\n\
         // imports custom bridges: the rusm:runtime interfaces (bound to the rusm-rs SDK)\n\
         // plus each granted bridge. The rusm-rs #[handlers]/#[main] macro binds it.\n\
         package rusm:component@0.1.0;\n\nworld handler {\n",
    );
    for iface in RUNTIME_INTERFACES {
        out.push_str(&format!("    import rusm:runtime/{iface}@0.1.0;\n"));
    }
    for r in custom_refs {
        out.push_str(&format!("    import {r};\n"));
    }
    out.push_str("    export run: func();\n}\n");
    out
}

/// The `component.wit` for a **Go** guest that imports custom bridges — two worlds in the
/// SDK's `rusm:runtime` package. `component` is the TinyGo **embedding** world (WASI +
/// rusm:runtime + the bridges + `export run`), used by `tinygo -wit-world component`.
/// `bridges` is the bridges **alone**, the world `wit-bindgen-go` generates Go bindings from
/// — so the guest's custom bindings are generated while `rusm:runtime` stays the SDK's
/// (no duplicate, no type clash). Overwrites the per-component copy of the SDK's
/// `component.wit`; the rusm:runtime imports are same-package (unqualified), the bridges
/// cross-package (fully qualified), matching the SDK's own `component.wit` style.
pub fn go_component_wit(custom_refs: &[String]) -> String {
    let mut out = String::from(
        "// GENERATED by `rusm build` — do not edit. The TinyGo embedding world (`component`)\n\
         // plus the bindgen world (`bridges`, custom bridges only) for a Go guest.\n\
         package rusm:runtime@0.1.0;\n\nworld component {\n    include wasi:cli/imports@0.2.0;\n",
    );
    for iface in RUNTIME_INTERFACES {
        out.push_str(&format!("    import {iface};\n"));
    }
    for r in custom_refs {
        out.push_str(&format!("    import {r};\n"));
    }
    out.push_str("    export run: func();\n}\n\nworld bridges {\n");
    for r in custom_refs {
        out.push_str(&format!("    import {r};\n"));
    }
    out.push_str("}\n");
    out
}

/// Recursively copy a directory tree (the SDK's `wit/` into a per-component copy). `std::fs`
/// has no recursive copy, and the tree is small (the WASI + rusm:runtime WIT).
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// Assemble a **Go** guest component's WIT so TinyGo can embed it and `wit-bindgen-go` can
/// bind the bridges. Starts from a fresh copy of the rusm-go SDK's `wit/` (`sdk_wit` — the
/// WASI + rusm:runtime packages TinyGo needs), vendors each granted `bridge.wit` into
/// `wit/deps/<name>/`, and overwrites `wit/component.wit` with [`go_component_wit`] (the
/// `component` + `bridges` worlds). Idempotent (the `wit/` dir is rebuilt each run).
pub fn generate_go_guest_wit(
    component_dir: &Path,
    granted: &[BridgeSpec],
    sdk_wit: &Path,
) -> Result<()> {
    let wit = component_dir.join("wit");
    if wit.exists() {
        std::fs::remove_dir_all(&wit).with_context(|| format!("clearing {}", wit.display()))?;
    }
    copy_dir(sdk_wit, &wit)?;
    for bridge in granted {
        vendor_into_component(component_dir, bridge)?;
    }
    let refs: Vec<String> = granted
        .iter()
        .map(|b| parse_contract(&b.wit()))
        .collect::<Result<Vec<_>>>()?
        .iter()
        .flat_map(Contract::interface_refs)
        .collect();
    std::fs::write(wit.join("component.wit"), go_component_wit(&refs))?;
    Ok(())
}

/// Set up a **guest component**'s WIT so it can import the custom bridges its capability
/// profile grants. Always vendors each granted `bridge.wit` into `wit/deps/<name>/`. When the
/// component has **no author-written `wit/world.wit`** (a macro-driven `#[handlers]`/`#[main]`
/// component), `rusm build` *owns* its WIT: it also vendors `rusm:runtime` and writes the
/// synthesized world. A component that ships its own `world.wit` keeps it — only the bridge
/// deps are added (so a hand-written `generate!` guest stays in control).
pub fn generate_guest_wit(component_dir: &Path, granted: &[BridgeSpec]) -> Result<()> {
    if granted.is_empty() {
        return Ok(());
    }
    for bridge in granted {
        vendor_into_component(component_dir, bridge)?;
    }
    let world = component_dir.join("wit").join("world.wit");
    if !world.exists() {
        let deps = component_dir.join("wit").join("deps").join("rusm-runtime");
        std::fs::create_dir_all(&deps).with_context(|| format!("creating {}", deps.display()))?;
        std::fs::write(deps.join("world.wit"), RUNTIME_WIT)?;
        let refs: Vec<String> = granted
            .iter()
            .map(|b| parse_contract(&b.wit()))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .flat_map(Contract::interface_refs)
            .collect();
        std::fs::write(&world, component_world(&refs))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway app dir under the system temp, removed first for a clean slate.
    fn app_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rusm-bridges-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_bridge(root: &Path, name: &str, wit: bool, host: bool) {
        let dir = root.join("bridges").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if wit {
            std::fs::write(dir.join("bridge.wit"), "package demo:bridge@0.1.0;\n").unwrap();
        }
        if host {
            std::fs::write(dir.join("host.rs"), "// host impl\n").unwrap();
        }
    }

    #[test]
    fn no_bridges_dir_is_empty_not_an_error() {
        let root = app_dir("none");
        assert!(discover(&root).unwrap().is_empty());
    }

    #[test]
    fn discovers_well_formed_bridges_sorted_by_name() {
        let root = app_dir("ok");
        write_bridge(&root, "weather", true, true);
        write_bridge(&root, "codec", true, true);
        // A stray file directly under bridges/ is ignored, not treated as a bridge.
        std::fs::write(root.join("bridges").join("README.md"), "x").unwrap();
        let found = discover(&root).unwrap();
        let names: Vec<&str> = found.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["codec", "weather"], "sorted, stray file ignored");
        assert_eq!(found[1].wit(), root.join("bridges/weather/bridge.wit"));
        assert_eq!(found[1].host(), root.join("bridges/weather/host.rs"));
    }

    #[test]
    fn a_malformed_bridge_fails_loudly() {
        let root = app_dir("malformed-wit");
        write_bridge(&root, "weather", false, true); // no bridge.wit
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("bridge.wit"), "names the missing file: {err}");

        let root = app_dir("malformed-host");
        write_bridge(&root, "weather", true, false); // no host.rs
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("host.rs"), "names the missing file: {err}");
    }

    /// The canonical custom-bridge WIT already used by the rusm-wasm end-to-end test —
    /// reused here so the contract parser is checked against a real, load-bearing artifact.
    const DEMO_WIT: &str =
        "../crates/rusm-wasm/tests/fixtures/custom-bridge/wit/deps/demo-bridge/bridge.wit";

    #[test]
    fn parses_a_real_bridge_contract() {
        let contract = parse_contract(Path::new(DEMO_WIT)).unwrap();
        assert_eq!(contract.namespace, "demo");
        assert_eq!(contract.name, "bridge");
        assert_eq!(contract.version.as_deref(), Some("0.1.0"));
        assert_eq!(contract.interfaces, ["greet"]);
        assert_eq!(contract.interface_refs(), ["demo:bridge/greet@0.1.0"]);
    }

    /// The example's weather bridge spec, for the TS-codegen tests.
    fn weather_bridge() -> BridgeSpec {
        BridgeSpec {
            name: "weather".into(),
            dir: PathBuf::from("../examples/custom-bridge/bridges/weather"),
        }
    }

    #[test]
    fn bridge_functions_extracts_string_signatures_and_rejects_richer_types() {
        let fns = bridge_functions(&weather_bridge().wit()).unwrap();
        assert_eq!(
            fns,
            [BridgeFn {
                interface: "forecast".into(),
                func: "lookup".into(),
                params: vec!["city".into()],
                returns_string: true,
            }]
        );
        // A non-string signature fails loudly (not a silent half-typed bridge).
        let dir = app_dir("richer");
        std::fs::write(
            dir.join("b.wit"),
            "package x:y@0.1.0;\ninterface i { f: func(n: u32) -> string; }\n",
        )
        .unwrap();
        let err = bridge_functions(&dir.join("b.wit"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("non-string param") && err.contains("Rust or Go"),
            "{err}"
        );
    }

    #[test]
    fn gen_runner_bridges_gen_wires_the_typed_primitive_and_js() {
        let gen = gen_runner_bridges_gen(std::slice::from_ref(&weather_bridge())).unwrap();
        // The typed primitive calls the typed wit-bindgen binding (no dispatcher).
        assert!(gen.contains("\"__weather__lookup\""));
        assert!(gen.contains("|city: String|"));
        assert!(gen.contains("crate::weather::bridge::forecast::lookup(&city)"));
        // The JS wrapper exposes globalThis.weather.lookup over the primitive.
        assert!(gen.contains("pub const BRIDGE_JS: &str ="));
        assert!(gen.contains("globalThis.weather.lookup = (city) => __weather__lookup(city);"));
        // Shape matches the committed empty module (register + BRIDGE_JS).
        assert!(gen.contains("pub fn register(ctx: &Ctx<'_>, globals: &Object<'_>)"));
    }

    #[test]
    fn synthesizes_the_bindgen_world_over_every_bridge() {
        let contracts = vec![
            Contract {
                namespace: "weather".into(),
                name: "bridge".into(),
                version: Some("0.1.0".into()),
                interfaces: vec!["forecast".into()],
            },
            Contract {
                namespace: "acme".into(),
                name: "codec".into(),
                version: None,
                interfaces: vec!["encode".into(), "decode".into()],
            },
        ];
        let world = synth_world(&contracts);
        assert!(world.contains("package rusm:host@0.1.0;"));
        assert!(world.contains("import weather:bridge/forecast@0.1.0;"));
        assert!(
            world.contains("import acme:codec/encode;"),
            "versionless ref: {world}"
        );
        assert!(world.contains("import acme:codec/decode;"));
    }

    #[test]
    fn generates_the_bridges_module_mounting_each_host_impl() {
        // A kebab-case dir name becomes a snake_case module, but `#[path]` keeps the real dir.
        let module = gen_bridges_module(&["weather", "json-codec"]);
        assert!(module.contains("#[path = \"../bridges/weather/host.rs\"]"));
        assert!(module.contains("pub mod weather;"));
        assert!(module.contains("#[path = \"../bridges/json-codec/host.rs\"]"));
        assert!(
            module.contains("pub mod json_codec;"),
            "kebab → snake: {module}"
        );
        assert!(module.contains("weather::add_to_linker(linker)?;"));
        assert!(module.contains("json_codec::add_to_linker(linker)?;"));
        assert!(module.contains("pub fn extend(linker: &mut rusm_wasm::BridgeLinker)"));
    }

    #[test]
    fn generate_host_files_writes_the_glue_and_reproduces_the_example() {
        // Generating into a fresh app dir produces exactly the codegen output — and an empty
        // app (no bridges/) writes nothing and reports no bridges.
        let root = app_dir("generate");
        assert!(
            generate_host_files(&root).unwrap().is_empty(),
            "no bridges/ → nothing"
        );
        assert!(
            !root.join("wit").exists(),
            "nothing written without bridges/"
        );

        // Mirror the worked example's bridge so the generated files must match it byte-for-byte.
        let weather = root.join("bridges/weather");
        std::fs::create_dir_all(&weather).unwrap();
        std::fs::copy(
            "../examples/custom-bridge/bridges/weather/bridge.wit",
            weather.join("bridge.wit"),
        )
        .unwrap();
        std::fs::write(weather.join("host.rs"), "// host impl\n").unwrap();

        let bridges = generate_host_files(&root).unwrap();
        assert_eq!(bridges.len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("wit/world.wit")).unwrap(),
            include_str!("../../examples/custom-bridge/wit/world.wit"),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("wit/deps/weather/bridge.wit")).unwrap(),
            include_str!("../../examples/custom-bridge/wit/deps/weather/bridge.wit"),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/bindings.rs")).unwrap(),
            include_str!("../../examples/custom-bridge/src/bindings.rs"),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/bridges.rs")).unwrap(),
            include_str!("../../examples/custom-bridge/src/bridges.rs"),
        );
    }

    #[test]
    fn vendors_a_bridge_into_a_guest_component() {
        let root = app_dir("vendor");
        let weather = root.join("bridges/weather");
        std::fs::create_dir_all(&weather).unwrap();
        std::fs::copy(
            "../examples/custom-bridge/bridges/weather/bridge.wit",
            weather.join("bridge.wit"),
        )
        .unwrap();
        std::fs::write(weather.join("host.rs"), "// host\n").unwrap();
        let bridges = discover(&root).unwrap();

        let component = root.join("components/api");
        std::fs::create_dir_all(&component).unwrap();
        vendor_into_component(&component, &bridges[0]).unwrap();

        let vendored = component.join("wit/deps/weather/bridge.wit");
        assert!(
            vendored.is_file(),
            "bridge.wit lands in the guest's wit/deps/"
        );
        assert_eq!(
            std::fs::read_to_string(&vendored).unwrap(),
            std::fs::read_to_string(weather.join("bridge.wit")).unwrap(),
            "vendored copy is the contract verbatim",
        );
    }

    #[test]
    fn component_world_imports_runtime_plus_each_bridge() {
        let world = component_world(&["weather:bridge/forecast@0.1.0".into()]);
        assert!(world.contains("package rusm:component@0.1.0;"));
        for iface in ["actor", "kv", "log", "pg", "serve", "streams"] {
            assert!(
                world.contains(&format!("import rusm:runtime/{iface}@0.1.0;")),
                "missing rusm:runtime/{iface}"
            );
        }
        assert!(world.contains("import weather:bridge/forecast@0.1.0;"));
        assert!(world.contains("export run: func();"));
    }

    #[test]
    fn generate_guest_wit_owns_a_macro_components_wit_but_respects_a_handwritten_one() {
        let root = app_dir("guest-wit");
        let weather = root.join("bridges/weather");
        std::fs::create_dir_all(&weather).unwrap();
        std::fs::copy(
            "../examples/custom-bridge/bridges/weather/bridge.wit",
            weather.join("bridge.wit"),
        )
        .unwrap();
        std::fs::write(weather.join("host.rs"), "// host\n").unwrap();
        let granted = discover(&root).unwrap();

        // A macro-driven component (no author wit/): `rusm build` owns its WIT — vendors the
        // bridge + rusm:runtime and writes the synthesized world.
        let macro_comp = root.join("components/api");
        std::fs::create_dir_all(&macro_comp).unwrap();
        generate_guest_wit(&macro_comp, &granted).unwrap();
        assert!(macro_comp.join("wit/deps/weather/bridge.wit").is_file());
        assert!(macro_comp.join("wit/deps/rusm-runtime/world.wit").is_file());
        assert_eq!(
            std::fs::read_to_string(macro_comp.join("wit/deps/rusm-runtime/world.wit")).unwrap(),
            RUNTIME_WIT,
        );
        assert_eq!(
            std::fs::read_to_string(macro_comp.join("wit/world.wit")).unwrap(),
            component_world(&["weather:bridge/forecast@0.1.0".into()]),
        );

        // A hand-written guest (ships its own world.wit): only the bridge dep is vendored;
        // its world is left untouched.
        let hand = root.join("components/custom");
        std::fs::create_dir_all(hand.join("wit")).unwrap();
        std::fs::write(hand.join("wit/world.wit"), "// my own world\n").unwrap();
        generate_guest_wit(&hand, &granted).unwrap();
        assert!(
            hand.join("wit/deps/weather/bridge.wit").is_file(),
            "bridge dep added"
        );
        assert_eq!(
            std::fs::read_to_string(hand.join("wit/world.wit")).unwrap(),
            "// my own world\n",
            "a hand-written world is never overwritten",
        );
        assert!(
            !hand.join("wit/deps/rusm-runtime").exists(),
            "a hand-written guest manages its own rusm:runtime dep",
        );
    }

    #[test]
    fn go_component_wit_has_an_embed_world_and_a_bindgen_world() {
        let wit = go_component_wit(&["weather:bridge/forecast@0.1.0".into()]);
        assert!(wit.contains("package rusm:runtime@0.1.0;"));
        // The embedding world: WASI + same-package rusm imports + the bridge + export run.
        assert!(wit.contains("world component {"));
        assert!(wit.contains("include wasi:cli/imports@0.2.0;"));
        assert!(wit.contains("    import actor;") && wit.contains("    import serve;"));
        assert!(wit.contains("import weather:bridge/forecast@0.1.0;"));
        assert!(wit.contains("export run: func();"));
        // The bindgen world: the bridges ALONE (so wit-bindgen-go binds only them).
        assert!(wit.contains("world bridges {"));
        // `actor` appears in `component` but the `bridges` world holds only the custom import.
        let bridges_world = wit.split("world bridges {").nth(1).unwrap();
        assert!(bridges_world.contains("import weather:bridge/forecast@0.1.0;"));
        assert!(!bridges_world.contains("import actor;"));
    }

    #[test]
    fn generate_go_guest_wit_copies_the_sdk_and_adds_the_bridge() {
        let root = app_dir("go-wit");
        let weather = root.join("bridges/weather");
        std::fs::create_dir_all(&weather).unwrap();
        std::fs::copy(
            "../examples/custom-bridge/bridges/weather/bridge.wit",
            weather.join("bridge.wit"),
        )
        .unwrap();
        std::fs::write(weather.join("host.rs"), "// host\n").unwrap();
        let granted = discover(&root).unwrap();

        let comp = root.join("components/go-api");
        std::fs::create_dir_all(&comp).unwrap();
        // The real rusm-go SDK wit is the source TinyGo needs (WASI + rusm:runtime).
        generate_go_guest_wit(&comp, &granted, Path::new("../packages/rusm-go/wit")).unwrap();

        assert_eq!(
            std::fs::read_to_string(comp.join("wit/component.wit")).unwrap(),
            go_component_wit(&["weather:bridge/forecast@0.1.0".into()]),
            "component.wit is the generated two-world WIT",
        );
        assert!(
            comp.join("wit/deps/weather/bridge.wit").is_file(),
            "bridge vendored"
        );
        assert!(
            comp.join("wit/world.wit").is_file(),
            "SDK rusm:runtime world copied"
        );
        assert!(
            comp.join("wit/deps/io").is_dir(),
            "SDK's WASI deps copied for TinyGo"
        );
    }

    /// The worked example (`examples/custom-bridge/`) commits the *generated* files so it
    /// compiles in the workspace — proving the codegen output is valid Rust. This guards that
    /// those committed files are byte-identical to what the generator emits: the example
    /// can't drift from the generator, and `rusm build` reproduces the example exactly.
    #[test]
    fn the_worked_example_is_exactly_what_the_generator_emits() {
        let contract = parse_contract(Path::new(
            "../examples/custom-bridge/bridges/weather/bridge.wit",
        ))
        .unwrap();
        assert_eq!(
            synth_world(std::slice::from_ref(&contract)),
            include_str!("../../examples/custom-bridge/wit/world.wit"),
            "examples/custom-bridge/wit/world.wit is not what synth_world emits",
        );
        assert_eq!(
            gen_bridges_module(&["weather"]),
            include_str!("../../examples/custom-bridge/src/bridges.rs"),
            "examples/custom-bridge/src/bridges.rs is not what gen_bridges_module emits",
        );
        assert_eq!(
            BINDINGS_RS,
            include_str!("../../examples/custom-bridge/src/bindings.rs"),
            "examples/custom-bridge/src/bindings.rs is not what BINDINGS_RS holds",
        );
    }
}
