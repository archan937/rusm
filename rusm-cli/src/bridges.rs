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
