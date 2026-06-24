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
//!
//! Supported host implementations (exactly one must be present):
//! - `host.rs` — Rust: compiled directly into the host binary, zero delegation overhead.
//! - `host.ts` — TypeScript: generates a Rust delegation shim + a resident TS actor runner.
//! - `host.go` — Go: same delegation pattern as TS; the runner is TinyGo-compiled to
//!   `wasm/bridge-<name>.wasm` and registered as a resident Wasm component.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use wit_parser::Resolve;

/// How the host-side of a custom bridge is implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostImpl {
    /// A Rust `host.rs` — compiled directly into the host binary (zero delegation
    /// overhead beyond the WIT ABI boundary crossing).
    Rust(PathBuf),
    /// A TypeScript `host.ts` — `rusm build` generates a Rust delegation shim and a TS
    /// dispatch runner; the runner runs as a resident actor (`bridge:<name>`). Each call
    /// is ~1–10µs from the actor round-trip + JSON marshaling.
    TypeScript(PathBuf),
    /// A Go `host.go` — same delegation pattern as TS: `rusm build` generates a Rust
    /// delegation shim and a Go dispatch runner (`_runner.go`). The runner is compiled by
    /// TinyGo to `wasm/bridge-<name>.wasm` and registered as a resident Wasm component.
    Go(PathBuf),
}

/// A discovered custom bridge: its `name` (the directory name, which is also the bridge
/// name used in the capability whitelist), the `dir` holding `bridge.wit` and the host
/// implementation, and the [`HostImpl`] variant describing how the host side is authored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeSpec {
    pub name: String,
    pub dir: PathBuf,
    pub host_impl: HostImpl,
}

impl BridgeSpec {
    /// The WIT contract (`bridges/<name>/bridge.wit`) — vendored into each granted guest
    /// component's `wit/deps/` and into the generated host crate.
    pub fn wit(&self) -> PathBuf {
        self.dir.join("bridge.wit")
    }

    /// The host implementation file (`host.rs`, `host.ts`, or `host.go`).
    pub fn host(&self) -> PathBuf {
        match &self.host_impl {
            HostImpl::Rust(p) | HostImpl::TypeScript(p) | HostImpl::Go(p) => p.clone(),
        }
    }

    /// Whether this bridge uses a Rust host impl (compiled directly into the host binary).
    pub fn is_rust_host(&self) -> bool {
        matches!(self.host_impl, HostImpl::Rust(_))
    }

    /// The component name the generated bridge runner registers as (`"bridge:<name>"`).
    pub fn runner_name(&self) -> String {
        format!("bridge:{}", self.name)
    }
}

/// Discover the custom bridges under `<root>/bridges/`. Returns them sorted by name (a
/// stable order, so generated code is deterministic). No `bridges/` directory → no custom
/// bridges (an empty list, not an error — most apps have none). A `bridges/<name>/` that is
/// missing `bridge.wit` or a host implementation is **malformed** and fails loudly.
///
/// Supported host implementations (exactly one must be present):
/// - `host.rs` — Rust: compiled directly into the host binary, zero delegation overhead.
/// - `host.ts` — TypeScript: `rusm build` generates a delegation shim + resident TS runner.
/// - `host.go` — Go: same delegation pattern; TinyGo compiles the runner to a Wasm component.
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
        if !path.join("bridge.wit").is_file() {
            bail!("custom bridge `{name}` is missing bridges/{name}/bridge.wit");
        }
        let rs = path.join("host.rs");
        let ts = path.join("host.ts");
        let go = path.join("host.go");
        let host_impl = match (rs.is_file(), ts.is_file(), go.is_file()) {
            (true, false, false) => HostImpl::Rust(rs),
            (false, true, false) => HostImpl::TypeScript(ts),
            (false, false, true) => HostImpl::Go(go),
            (false, false, false) => bail!(
                "custom bridge `{name}` needs a host implementation — \
                 add bridges/{name}/host.rs (Rust), bridges/{name}/host.ts (TypeScript), \
                 or bridges/{name}/host.go (Go)"
            ),
            _ => bail!(
                "custom bridge `{name}` has multiple host implementation files — \
                 keep exactly one of host.rs, host.ts, or host.go"
            ),
        };
        bridges.push(BridgeSpec { name, dir: path, host_impl });
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

/// Stage a per-app **js-runner** build: copy the runner crate `src` into `dest` (minus
/// `target/`), overwrite `src/bridges_gen.rs` with the generated glue, and write a scoped
/// `wit-bridges/` package — a synthesized `bridge-imports` world importing every custom bridge,
/// with each contract vendored under `wit-bridges/deps/<name>/`. `bridges_gen`'s own
/// `generate!` binds that world (serde-deriving, self-contained — no WASI types), keeping the
/// runner's `process`-world bindings untouched. The staged crate then builds (cargo → wizer →
/// wasm-tools) into a runner with the app's bridges compiled in. Idempotent (the dest is
/// rebuilt).
pub fn stage_js_runner(src: &Path, dest: &Path, bridges: &[BridgeSpec]) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).with_context(|| format!("clearing {}", dest.display()))?;
    }
    copy_dir(src, dest)?;
    std::fs::write(
        dest.join("src/bridges_gen.rs"),
        gen_runner_bridges_gen(bridges)?,
    )?;
    stage_bridge_imports_wit(dest, bridges)
}

/// Write the scoped `wit-bridges/` package the runner's `bridges_gen` `generate!` binds: a
/// `bridge-imports` world importing each custom bridge interface, with every contract vendored
/// as a dep. Self-contained (only the bridge packages), so serde derives never reach WASI.
fn stage_bridge_imports_wit(dest: &Path, bridges: &[BridgeSpec]) -> Result<()> {
    let mut imports = String::new();
    for bridge in bridges {
        for iface in parse_contract(&bridge.wit())?.interface_refs() {
            imports.push_str(&format!("    import {iface};\n"));
        }
        let dep = dest.join("wit-bridges/deps").join(&bridge.name);
        std::fs::create_dir_all(&dep).with_context(|| format!("creating {}", dep.display()))?;
        std::fs::copy(bridge.wit(), dep.join("bridge.wit"))
            .with_context(|| format!("vendoring bridge `{}`", bridge.name))?;
    }
    let world = format!(
        "// GENERATED by `rusm build` — do not edit. The scoped world `bridges_gen` binds: the\n\
         // app's custom bridges alone (self-contained), so its serde-deriving `generate!` never\n\
         // touches WASI types. Regenerated from `bridges/` each build.\n\
         package rusm:bridges@0.1.0;\n\nworld bridge-imports {{\n{imports}}}\n"
    );
    std::fs::write(dest.join("wit-bridges/world.wit"), world)
        .with_context(|| format!("writing {}/wit-bridges/world.wit", dest.display()))?;
    Ok(())
}

/// A WIT identifier (namespace / package / interface / bridge name) as a Rust/JS identifier:
/// kebab → snake (`json-codec` → `json_codec`). The js-runner's generated WIT bindings + the
/// `__<bridge>__<func>` primitives use this form.
fn ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Generate the per-app js-runner's `bridges_gen.rs` (a scoped, serde-deriving `generate!` over
/// the custom bridges' self-contained WIT + the [`register`] host primitives + the `BRIDGE_JS`
/// TS API). Each function becomes a typed `__<bridge>__<func>` primitive that JSON-deserializes
/// its args, calls the *typed* wit-bindgen binding (no dispatcher), and JSON-serializes the
/// result; `globalThis.<bridge>.<func>` wraps it. Mirrors the committed empty `bridges_gen.rs`'s
/// `register`/`BRIDGE_JS` shape, so the runner compiles either way.
///
/// The `generate!` is **scoped to the bridge package** (`world: "bridge-imports"` over
/// `wit-bridges/`, no `generate_all`) so serde derives land only on the bridge's own value
/// types — never WASI types, which can wrap resources serde cannot derive. It sits at this
/// module's top level, so the binding paths witmap emits (`<ns>::<pkg>::<iface>::…`) resolve
/// relative to `bridges_gen` without a prefix.
///
/// [`register`]: it's the generated function the runner's `boot_bridge` calls.
pub fn gen_runner_bridges_gen(bridges: &[BridgeSpec]) -> Result<String> {
    let mut registers = String::new();
    let mut js = String::new();
    for bridge in bridges {
        let object = ident(&bridge.name);
        let api = crate::witmap::bridge_api(&bridge.wit())?;
        js.push_str(&format!(
            "globalThis.{object} = globalThis.{object} || {{}};\n"
        ));
        for f in &api.functions {
            registers.push_str(&runner_fn_glue(&object, f));
            // JS wrapper: JSON-marshal the args to the typed primitive, parse the result.
            let names = f
                .params
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            js.push_str(&format!(
                "globalThis.{object}.{0} = ({names}) => JSON.parse(__{object}__{0}(JSON.stringify([{names}])));\n",
                f.name
            ));
        }
    }
    Ok(format!(
        "//! GENERATED by `rusm build` — do not edit. Per-app custom-bridge glue for the\n\
         //! js-runner: a serde-deriving `generate!` scoped to the bridge package, then each\n\
         //! `__<bridge>__<func>` primitive deserializes its JSON args, calls the typed\n\
         //! wit-bindgen binding (no dispatcher), and returns the JSON result; `BRIDGE_JS`\n\
         //! exposes `globalThis.<bridge>`. Regenerated from `bridges/` each build.\n\
         wit_bindgen::generate!({{\n\
         \x20   world: \"bridge-imports\",\n\
         \x20   path: \"wit-bridges\",\n\
         \x20   generate_all,\n\
         \x20   additional_derives: [serde::Serialize, serde::Deserialize],\n\
         }});\n\
         \n\
         use rquickjs::{{Ctx, Function, Object}};\n\
         \n\
         pub fn register<'js>(ctx: &Ctx<'js>, globals: &Object<'js>) {{\n\
         {registers}}}\n\
         \n\
         pub const BRIDGE_JS: &str = {js:?};\n"
    ))
}

/// One function's host-primitive registration: a typed `__<bridge>__<func>` that deserializes
/// its JSON-array arguments into the owned Rust param tuple, calls the *typed* wit-bindgen
/// binding (with the borrow that binding expects), and returns the JSON-serialized result.
/// Bad arguments throw a JS `TypeError` (not a process trap).
fn runner_fn_glue(object: &str, f: &crate::witmap::Func) -> String {
    use crate::witmap::Borrow;
    let prim = format!("__{object}__{}", f.name);
    // A 0-arg function ignores `ctx`/`__args` (underscore them to avoid unused warnings).
    let (ctx_name, args_name) = if f.params.is_empty() {
        ("_ctx", "_args")
    } else {
        ("ctx", "__args")
    };
    let deser = if f.params.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
        let types: Vec<&str> = f.params.iter().map(|p| p.owned_rust.as_str()).collect();
        // A 1-tuple needs the trailing comma.
        let (pat, ty) = if f.params.len() == 1 {
            (format!("({},)", names[0]), format!("({},)", types[0]))
        } else {
            (
                format!("({})", names.join(", ")),
                format!("({})", types.join(", ")),
            )
        };
        format!(
            "            let {pat}: {ty} = match ::serde_json::from_str(&__args) {{\n\
             \x20               ::core::result::Result::Ok(v) => v,\n\
             \x20               ::core::result::Result::Err(e) => return ::core::result::Result::Err(::rquickjs::Exception::throw_type(&ctx, &(::std::string::String::from(\"bridge {object}.{}: bad arguments: \") + &e.to_string()))),\n\
             \x20           }};\n",
            f.name
        )
    };
    let call_args = f
        .params
        .iter()
        .map(|p| match p.borrow {
            Borrow::Value => p.name.clone(),
            Borrow::Ref => format!("&{}", p.name),
            Borrow::AsDeref => format!("{}.as_deref()", p.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "    globals\n\
         \x20       .set(\n\
         \x20           \"{prim}\",\n\
         \x20           Function::new(ctx.clone(), |{ctx_name}: Ctx<'_>, {args_name}: ::std::string::String| -> ::rquickjs::Result<::std::string::String> {{\n\
         {deser}\
         \x20               let __r = {}({call_args});\n\
         \x20               ::core::result::Result::Ok(::serde_json::to_string(&__r).expect(\"serialize bridge result\"))\n\
         \x20           }})\n\
         \x20           .expect(\"{prim}\"),\n\
         \x20       )\n\
         \x20       .expect(\"{prim}\");\n",
        f.call_path
    )
}

/// Generate the **TypeScript ambient types** for the app's custom bridges, so a TS guest calls
/// them fully typed (no hand-written `declare`) over arbitrary WIT value types. Written by
/// `rusm build` to `<app>/bridges.d.ts`; a TS component `/// <reference>`s it. Emits each named
/// type's declaration (interface / union) once, then the `declare const <bridge>` globals.
pub fn gen_bridge_dts(bridges: &[BridgeSpec]) -> Result<String> {
    let mut decls = std::collections::BTreeMap::new();
    let mut globals = String::new();
    for bridge in bridges {
        let object = ident(&bridge.name);
        let api = crate::witmap::bridge_api(&bridge.wit())?;
        decls.extend(api.ts_decls);
        globals.push_str(&format!("declare const {object}: {{\n"));
        for f in &api.functions {
            let params = f
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ts))
                .collect::<Vec<_>>()
                .join(", ");
            let ret = f.result_ts.as_deref().unwrap_or("void");
            globals.push_str(&format!("  {}({params}): {ret};\n", f.name));
        }
        globals.push_str("};\n\n");
    }
    let mut out = String::from(
        "// GENERATED by `rusm build` — do not edit. Ambient types for the app's custom bridges,\n\
         // so a TS guest calls them typed. Regenerated from bridges/ each build.\n\n",
    );
    for decl in decls.values() {
        out.push_str(decl);
        out.push_str("\n\n");
    }
    out.push_str(&globals);
    Ok(out)
}

/// Generate the Rust **delegation shim** for a TS- or Go-hosted bridge: each WIT function
/// JSON-encodes its arguments, sends a tagged request to the resident `bridge:<name>`
/// actor, and awaits the tagged reply via [`rusm_wasm::BridgeHost::recv_bridge_reply`]
/// (selective receive — unrelated mailbox messages are parked in the save queue and
/// replayed by the next `receive`). The wire protocol is identical for TS and Go runners;
/// only the runner language differs. Written to `src/bridge_<ident>_delegate.rs` and
/// mounted from `src/bridges.rs` via a `#[path]` attribute.
pub fn gen_delegate_host(bridge: &BridgeSpec, contract: &Contract) -> Result<String> {
    let name = &bridge.name;
    let api = crate::witmap::bridge_api(&bridge.wit())?;
    let runner_name = bridge.runner_name();

    let mut linker_calls = String::new();
    let mut host_impls = String::new();

    for iface_name in &contract.interfaces {
        let iface_mod = module_ident(iface_name);
        let ns_mod = module_ident(&contract.namespace);
        let pkg_mod = module_ident(&contract.name);
        let iface_path = format!("crate::bindings::{ns_mod}::{pkg_mod}::{iface_mod}");

        linker_calls.push_str(&format!(
            "    {iface_path}::add_to_linker::\
             <_, ::rusm_wasm::wasmtime::component::HasSelf<::rusm_wasm::BridgeHost>>\
             (linker, |host| host)?;\n"
        ));

        let prefix = format!("{ns_mod}::{pkg_mod}::{iface_mod}::");
        let iface_funcs: Vec<&crate::witmap::Func> =
            api.functions.iter().filter(|f| f.call_path.starts_with(&prefix)).collect();

        host_impls.push_str(&format!(
            "impl {iface_path}::Host for ::rusm_wasm::BridgeHost {{\n"
        ));
        for f in iface_funcs {
            host_impls.push_str(&delegate_fn_impl(f, &bridge.name, &runner_name));
        }
        host_impls.push_str("}\n\n");
    }

    let bridge_name = &bridge.name;
    let impl_file = match &bridge.host_impl {
        HostImpl::TypeScript(_) => format!("bridges/{bridge_name}/host.ts"),
        HostImpl::Go(_) => format!("bridges/{bridge_name}/host.go"),
        HostImpl::Rust(_) => unreachable!(),
    };
    Ok(format!(
        "//! GENERATED by `rusm build` — do not edit. Delegation shim for \
         `{impl_file}`: each function JSON-encodes its arguments, sends a tagged\n\
         //! request to the resident `{runner_name}` actor, and awaits the tagged reply via\n\
         //! selective receive. Overhead: ~1–10µs/call (actor round-trip + JSON). Regenerated\n\
         //! each build.\n\
         \n\
         pub fn add_to_linker(\n\
         \x20   linker: &mut ::rusm_wasm::BridgeLinker,\n\
         ) -> ::rusm_wasm::wasmtime::Result<()> {{\n\
         {linker_calls}\
         \x20   Ok(())\n\
         }}\n\n\
         {host_impls}",
    ))
}

/// Generate one async function body for the delegation shim.
fn delegate_fn_impl(f: &crate::witmap::Func, bridge_name: &str, runner_name: &str) -> String {
    let params = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.owned_rust))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = f.result_rust.as_deref().unwrap_or("()");
    let ret_default = if f.result_rust.is_some() {
        "::std::default::Default::default()"
    } else {
        "()"
    };

    // args JSON array: `[<json0>, <json1>, …]`.
    let args_json = if f.params.is_empty() {
        "\"[]\".to_string()".to_string()
    } else {
        let parts: Vec<String> = f
            .params
            .iter()
            .map(|p| {
                format!(
                    "::rusm_wasm::serde_json::to_string(&{}).expect(\"serialize bridge arg\")",
                    p.name
                )
            })
            .collect();
        format!(
            "::std::format!(\"[{{}}]\", [{}].join(\",\"))",
            parts.join(", ")
        )
    };

    let parse_reply = if f.result_rust.is_some() {
        format!(
            "::rusm_wasm::serde_json::from_slice::<{ret}>(payload).unwrap_or_default()"
        )
    } else {
        "()".to_string()
    };
    let comma_params = if params.is_empty() {
        String::new()
    } else {
        format!(", {params}")
    };

    format!(
        "    async fn {fn_name}(&mut self{comma_params}) -> {ret} {{\n\
         \x20       static CALL_CTR: ::std::sync::atomic::AtomicU64 =\n\
         \x20           ::std::sync::atomic::AtomicU64::new(0);\n\
         \x20       let call_id = ::std::format!(\n\
         \x20           \"rusm-bridge:{bridge_name}-{{}}-{{}}\",\n\
         \x20           self.pid(),\n\
         \x20           CALL_CTR.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed),\n\
         \x20       );\n\
         \x20       let Some(pid) = self.runtime().whereis(\"{runner_name}\") else {{\n\
         \x20           return {ret_default};\n\
         \x20       }};\n\
         \x20       let args_json = {args_json};\n\
         \x20       let req = ::std::format!(\n\
         \x20           \"{{\\\"fn\\\":\\\"{fn_name}\\\",\\\"args\\\":{{}},\\\"replyTo\\\":{{\\\"pid\\\":\\\"{{}}\\\",\\\"callId\\\":\\\"{{}}\\\"}}}}\",\n\
         \x20           args_json, self.pid(), call_id,\n\
         \x20       ).into_bytes();\n\
         \x20       self.runtime().send(pid, req);\n\
         \x20       let tag = ::std::format!(\"{{}}:\", call_id);\n\
         \x20       let raw = match self.recv_bridge_reply(&tag).await {{\n\
         \x20           Some(b) => b,\n\
         \x20           None => return {ret_default},\n\
         \x20       }};\n\
         \x20       let payload = raw.strip_prefix(tag.as_bytes()).unwrap_or(&raw);\n\
         \x20       {parse_reply}\n\
         \x20   }}\n\n",
        fn_name = f.name,
        comma_params = comma_params,
        ret = ret,
        ret_default = ret_default,
        args_json = args_json,
        parse_reply = parse_reply,
        bridge_name = bridge_name,
        runner_name = runner_name,
    )
}

/// Generate the TypeScript **dispatch runner** for a TS-hosted bridge: a long-lived
/// actor that registers as `bridge:<name>`, receives tagged JSON requests, calls the
/// corresponding export from the user's `host.ts`, and sends back a tagged reply.
/// Written to `bridges/<name>/_runner.ts`; bundled by `rusm build` into
/// `wasm/bridge-<name>.js`.
pub fn gen_ts_runner(bridge: &BridgeSpec) -> String {
    let runner_name = bridge.runner_name();
    format!(
        "// GENERATED by `rusm build` — do not edit.\n\
         // Dispatch runner for the `{name}` bridge. Receives tagged JSON requests\n\
         // from the Rust delegation shim, calls the matching export from host.ts, and\n\
         // sends back a tagged reply. Runs as a resident actor: \"{runner_name}\".\n\
         import type * as UserBridge from \"./host\";\n\
         import {{ Process }} from \"rusm-ts\";\n\
         \n\
         // CommonJS require so host.ts top-level imports resolve through the Bun bundler.\n\
         const bridge = require(\"./host\") as typeof UserBridge;\n\
         \n\
         Process.register(\"{runner_name}\");\n\
         Process.setLabel(\"{runner_name}\");\n\
         \n\
         while (true) {{\n\
         \x20 const raw = await Process.receiveText();\n\
         \x20 let msg: {{ fn: string; args: unknown[]; replyTo: {{ pid: string; callId: string }} }};\n\
         \x20 try {{\n\
         \x20   msg = JSON.parse(raw);\n\
         \x20 }} catch {{\n\
         \x20   continue; // malformed request — skip\n\
         \x20 }}\n\
         \x20 const {{ fn: fnName, args, replyTo: {{ pid: replyPid, callId }} }} = msg;\n\
         \x20 let result: unknown = null;\n\
         \x20 try {{\n\
         \x20   result =\n\
         \x20     (await (bridge as Record<string, (...a: unknown[]) => unknown>)\n\
         \x20       [fnName]?.(...(args ?? []))) ?? null;\n\
         \x20 }} catch {{\n\
         \x20   result = null;\n\
         \x20 }}\n\
         \x20 // Reply prefix = callId so the shim's selective receive matches it.\n\
         \x20 Process.send(replyPid, `${{callId}}:${{JSON.stringify(result)}}`);\n\
         }}\n",
        name = bridge.name,
        runner_name = runner_name,
    )
}

/// Generate the Go **dispatch runner** for a Go-hosted bridge: a TinyGo/rusm-go actor that
/// registers as `bridge:<name>`, receives JSON requests from the Rust delegation shim, calls
/// the matching exported function from the user's `host.go` (same `package main`, so the call
/// is direct — no import), and sends back a tagged reply. Written to `bridges/<name>/_runner.go`;
/// TinyGo compiles the whole `bridges/<name>/` package to `wasm/bridge-<name>.wasm`.
pub fn gen_go_bridge_runner(bridge: &BridgeSpec) -> Result<String> {
    let api = crate::witmap::bridge_api(&bridge.wit())?;
    let runner_name = bridge.runner_name();
    let mut dispatch_cases = String::new();
    for f in &api.functions {
        dispatch_cases.push_str(&go_dispatch_case(f));
    }
    Ok(format!(
        "// GENERATED by `rusm build` — do not edit.\n\
         // Dispatch runner for the `{name}` bridge: receives JSON requests from the Rust\n\
         // delegation shim, calls the matching export from host.go (same package, direct call),\n\
         // and sends back a tagged reply. Runs as a resident actor: \"{runner_name}\".\n\
         package main\n\n\
         import (\n\
         \t\"encoding/json\"\n\n\
         \trusm \"github.com/archan937/rusm/packages/rusm-go\"\n\
         )\n\n\
         type bridgeRequest struct {{\n\
         \tFn      string            `json:\"fn\"`\n\
         \tArgs    []json.RawMessage `json:\"args\"`\n\
         \tReplyTo struct {{\n\
         \t\tPid    string `json:\"pid\"`\n\
         \t\tCallID string `json:\"callId\"`\n\
         \t}} `json:\"replyTo\"`\n\
         }}\n\n\
         func main() {{\n\
         \trusm.Register(\"{runner_name}\")\n\
         \trusm.SetLabel(\"{runner_name}\")\n\
         \tfor {{\n\
         \t\traw := rusm.Receive()\n\
         \t\tvar req bridgeRequest\n\
         \t\tif err := json.Unmarshal(raw, &req); err != nil {{\n\
         \t\t\tcontinue\n\
         \t\t}}\n\
         \t\tresult := dispatch(req.Fn, req.Args)\n\
         \t\tresultBytes, _ := json.Marshal(result)\n\
         \t\treply := append([]byte(req.ReplyTo.CallID+\":\"), resultBytes...)\n\
         \t\trusm.Send(req.ReplyTo.Pid, reply)\n\
         \t}}\n\
         }}\n\n\
         func dispatch(fn string, args []json.RawMessage) interface{{}} {{\n\
         \tswitch fn {{\n\
         {dispatch_cases}\
         \t}}\n\
         \treturn nil\n\
         }}\n",
        name = bridge.name,
        runner_name = runner_name,
    ))
}

/// One function's case in the Go dispatch switch: deserialises each argument, calls the
/// user's exported Go function (PascalCase of the WIT name — same package, no import), and
/// returns the result as `interface{}` for JSON marshaling.
///
/// Primitive params (`string`, `uint32`, `bool`, …) are unmarshalled into the target type.
/// Complex params (`json.RawMessage` — records, enums, variants) are passed as-is: `args[i]`
/// is already `json.RawMessage`, so no intermediate unmarshal is needed; the user's function
/// calls `json.Unmarshal` directly into its own struct.
fn go_dispatch_case(f: &crate::witmap::Func) -> String {
    let guard = if !f.params.is_empty() {
        format!("\t\tif len(args) < {} {{ return nil }}\n", f.params.len())
    } else {
        String::new()
    };
    let mut deser = String::new();
    let mut call_args: Vec<String> = Vec::new();
    for (i, p) in f.params.iter().enumerate() {
        let var_name = format!("arg{i}");
        if p.go == "json.RawMessage" {
            // Pass the raw JSON bytes directly — the user's function unmarshals into its own type.
            deser.push_str(&format!("\t\t{var_name} := args[{i}]\n"));
        } else {
            deser.push_str(&format!(
                "\t\tvar {var_name} {go_type}\n\
                 \t\tif err := json.Unmarshal(args[{i}], &{var_name}); err != nil {{ return nil }}\n",
                go_type = p.go,
            ));
        }
        call_args.push(var_name);
    }
    format!(
        "\tcase \"{fn_name}\":\n\
         {guard}\
         {deser}\
         \t\treturn {go_fn}({args})\n",
        fn_name = f.name,
        go_fn = pascal_go(&f.name),
        args = call_args.join(", "),
    )
}

/// WIT function name (kebab-case) to Go exported function name (PascalCase):
/// `get-weather` → `GetWeather`, `lookup` → `Lookup`.
fn pascal_go(name: &str) -> String {
    name.split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Generate the `go.mod` for a Go bridge runner directory — created only if none exists, so
/// the user can manage it manually once they need additional dependencies. Uses the same
/// [`crate::scaffold::SDK_VERSION`] as scaffolded Go components (single source of truth).
pub fn gen_go_bridge_gomod(name: &str) -> String {
    format!(
        "module bridge-{name}\n\
         \n\
         go 1.24\n\
         \n\
         require github.com/archan937/rusm/packages/rusm-go v{}\n",
        crate::scaffold::SDK_VERSION
    )
}

/// The static `src/bindings.rs` of a generated host crate: one `bindgen!` over the
/// synthesized `wit/` world, producing the typed `Host` traits a bridge's `host.rs`
/// implements over [`rusm_wasm::BridgeHost`]. Identical for every app (the variation is in
/// the `wit/` world), so it's a constant — emitted as a file only so the layout reads like
/// the platform's own `crate::bindings`. Used for pure-Rust-bridge apps.
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

/// Like [`BINDINGS_RS`] but adds serde derives to all generated value types — required
/// for TS/Go-bridge delegation shims that JSON-marshal WIT record/enum/variant params.
pub const BINDINGS_RS_SERDE: &str = "\
//! GENERATED by `rusm build` — do not edit. Typed bindings for the app's custom bridges,
//! from the synthesized `wit/` world, with serde derives so TS/Go delegation shims can
//! JSON-marshal WIT value types. `wasmtime` is the runtime's exact version (pinned in
//! Cargo.toml), so the generated types are identical to the ones the runtime links.
wasmtime::component::bindgen!({
    path: \"wit\",
    world: \"host\",
    imports: { default: async },
    additional_derives: [serde::Serialize, serde::Deserialize],
});
";

/// The generated `src/main.rs` for a TS/Go-bridge app (no `host.rs` — `rusm build` writes
/// all Rust). Builds the runtime, calls `bridges::init` to register the runner components,
/// then hands off to `rusm_cli::host::serve_with_init`. Only written if `src/main.rs`
/// does not already exist (Rust-bridge apps author their own `main.rs`).
pub const MAIN_RS_BRIDGE_RUNNERS: &str = "\
//! GENERATED by `rusm build` — do not edit. Host binary entry point for this TS/Go-bridge
//! app: wires the delegation shims, registers the runner components as resident actors,
//! then hands off to `rusm_cli::host::serve_with_init`.
mod bindings;
mod bridges;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = ::std::path::Path::new(\".\");
    let cfg = rusm_node::NodeConfig::load(root.join(\"rusm.toml\"), false)
        .map_err(anyhow::Error::msg)?;
    rusm_cli::host::serve_with_init(root, &cfg, bridges::extend, bridges::init).await
}
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

/// Synthesize the host crate's `src/bridges.rs`: mounts each bridge — Rust bridges via
/// `#[path]` to the author's `host.rs`, TS/Go bridges via their generated delegate shim
/// (`src/bridge_<name>_delegate.rs`) — and exposes [`extend`] (registers all bridges into
/// the linker) and, when there are TS/Go bridges, [`init`] (registers and boots the runner
/// components as resident actors).
pub fn gen_bridges_module(bridges: &[&BridgeSpec]) -> String {
    let mut out = String::from(
        "//! GENERATED by `rusm build` — do not edit. Mounts each custom bridge's host impl\n\
         //! and registers them all. For pure-Rust-bridge apps pass `extend` to\n\
         //! `rusm_cli::host::serve`; for TS/Go-bridge apps call `init` first via\n\
         //! `rusm_cli::host::serve_with_init`.\n\n",
    );
    for bridge in bridges {
        let mod_ident = module_ident(&bridge.name);
        if bridge.is_rust_host() {
            out.push_str(&format!(
                "#[path = \"../bridges/{}/host.rs\"]\npub mod {mod_ident};\n\n",
                bridge.name
            ));
        } else {
            out.push_str(&format!(
                "#[path = \"bridge_{mod_ident}_delegate.rs\"]\npub mod {mod_ident};\n\n"
            ));
        }
    }
    out.push_str(
        "/// Register every custom application bridge into the component linker.\n\
         pub fn extend(linker: &mut rusm_wasm::BridgeLinker) -> rusm_wasm::wasmtime::Result<()> {\n",
    );
    for bridge in bridges {
        out.push_str(&format!(
            "    {}::add_to_linker(linker)?;\n",
            module_ident(&bridge.name)
        ));
    }
    out.push_str("    Ok(())\n}\n");

    // `init` is only emitted when there are TS/Go bridges whose runners must be registered.
    let non_rust: Vec<&&BridgeSpec> = bridges.iter().filter(|b| !b.is_rust_host()).collect();
    if !non_rust.is_empty() {
        out.push_str(
            "\n/// Register and boot TS/Go bridge runners as resident actors. Call once after\n\
             /// `rusm_cli::host::build_runtime` and before the first component spawn —\n\
             /// the generated `src/main.rs` does this via `serve_with_init`.\n\
             pub fn init(wasm: &rusm_wasm::WasmRuntime) -> anyhow::Result<()> {\n",
        );
        for bridge in &non_rust {
            let runner_name = bridge.runner_name();
            if matches!(bridge.host_impl, HostImpl::TypeScript(_)) {
                let js_file = format!("wasm/bridge-{}.js", bridge.name);
                out.push_str(&format!(
                    "    wasm.register_js_component_with(\n\
                     \x20       \"{runner_name}\".to_string(),\n\
                     \x20       std::fs::read(\"{js_file}\")\n\
                     \x20           .map_err(|e| anyhow::anyhow!(\"{js_file}: {{}}\", e))?,\n\
                     \x20       rusm_wasm::CapabilityProfile::Trusted.capabilities(),\n\
                     \x20   );\n\
                     \x20   // Drop the handle — Tokio detaches a dropped JoinHandle, so the\n\
                     \x20   // supervisor process keeps running for the node's lifetime.\n\
                     \x20   wasm.supervise(&[\"{runner_name}\".to_string()]);\n"
                ));
            } else {
                // Go bridge: wasm bytes compiled by TinyGo.
                let wasm_file = format!("wasm/bridge-{}.wasm", bridge.name);
                out.push_str(&format!(
                    "    wasm.register_component_with(\n\
                     \x20       \"{runner_name}\".to_string(),\n\
                     \x20       wasm.prepare_component_bytes(\n\
                     \x20           &std::fs::read(\"{wasm_file}\")\n\
                     \x20               .map_err(|e| anyhow::anyhow!(\"{wasm_file}: {{}}\", e))?,\n\
                     \x20       ).map_err(|e| anyhow::anyhow!(\"compiling {runner_name}: {{}}\", e))?,\n\
                     \x20       rusm_wasm::CapabilityProfile::Trusted.capabilities(),\n\
                     \x20   );\n\
                     \x20   // Drop the handle — Tokio detaches a dropped JoinHandle, so the\n\
                     \x20   // supervisor process keeps running for the node's lifetime.\n\
                     \x20   wasm.supervise(&[\"{runner_name}\".to_string()]);\n"
                ));
            }
        }
        out.push_str("    Ok(())\n}\n");
    }

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

    // TS/Go bridges need serde derives on all generated WIT value types for JSON marshaling.
    let needs_delegation = bridges.iter().any(|b| !b.is_rust_host());
    std::fs::write(
        src.join("bindings.rs"),
        if needs_delegation { BINDINGS_RS_SERDE } else { BINDINGS_RS },
    )?;

    // Write the generated delegation shim and language-specific runner for each non-Rust bridge.
    for (bridge, contract) in bridges.iter().zip(contracts.iter()) {
        if bridge.is_rust_host() {
            continue;
        }
        let shim = gen_delegate_host(bridge, contract)?;
        let id = module_ident(&bridge.name);
        std::fs::write(src.join(format!("bridge_{id}_delegate.rs")), shim)?;
        match &bridge.host_impl {
            HostImpl::TypeScript(_) => {
                std::fs::write(bridge.dir.join("_runner.ts"), gen_ts_runner(bridge))?;
            }
            HostImpl::Go(_) => {
                std::fs::write(bridge.dir.join("_runner.go"), gen_go_bridge_runner(bridge)?)?;
                // Only write go.mod if absent — preserves user customisations (extra deps).
                let gomod = bridge.dir.join("go.mod");
                if !gomod.is_file() {
                    std::fs::write(&gomod, gen_go_bridge_gomod(&bridge.name))?;
                }
            }
            HostImpl::Rust(_) => unreachable!(),
        }
    }

    let bridge_refs: Vec<&BridgeSpec> = bridges.iter().collect();
    std::fs::write(src.join("bridges.rs"), gen_bridges_module(&bridge_refs))?;

    // For TS/Go-bridge apps there is no author-written main.rs — write the generated one.
    if needs_delegation && !src.join("main.rs").exists() {
        std::fs::write(src.join("main.rs"), MAIN_RS_BRIDGE_RUNNERS)?;
    }

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

/// The canonical `rusm:runtime` WIT, vendored from `rusm-rs` into `templates/` (the published
/// crate's tarball has no `../../crates/`, so it can't `include_str!` across the workspace —
/// `make sync-templates` keeps this copy byte-identical, guarded by a drift test). `rusm build`
/// writes it into a generated guest component's `wit/deps/rusm-runtime/`, so the component's
/// world resolves the same `rusm:runtime` interfaces the `rusm-rs` SDK is generated from.
pub const RUNTIME_WIT: &str = include_str!("../templates/runtime-world.wit");

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

/// Recursively copy a directory tree, **skipping `target/`** (a crate's build cache, which is
/// huge and rebuilt anyway). `std::fs` has no recursive copy. Used to stage the SDK's `wit/`
/// (small) and the js-runner crate source (for a per-app TS-bridge build).
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        if from.is_dir() && entry.file_name() == "target" {
            continue;
        }
        let to = dst.join(entry.file_name());
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
        assert!(
            err.contains("host implementation"),
            "explains the missing impl: {err}"
        );
        // The error message names all three options.
        assert!(err.contains("host.go"), "mentions Go option: {err}");
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
        let dir = PathBuf::from("../examples/weather-api/bridges/weather");
        BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::Rust(dir.join("host.rs")),
            dir,
        }
    }

    fn ts_bridge(root: &Path, name: &str) -> BridgeSpec {
        let dir = root.join("bridges").join(name);
        BridgeSpec {
            name: name.into(),
            host_impl: HostImpl::TypeScript(dir.join("host.ts")),
            dir,
        }
    }

    #[test]
    fn gen_runner_bridges_gen_marshals_args_through_the_typed_binding() {
        let gen = gen_runner_bridges_gen(std::slice::from_ref(&weather_bridge())).unwrap();
        // The typed primitive deserializes the JSON arg tuple into owned Rust, calls the typed
        // wit-bindgen binding (no dispatcher) with the borrow it expects, and serializes back.
        assert!(gen.contains("\"__weather__lookup\""));
        assert!(gen.contains("let (city,): (String,) = match ::serde_json::from_str(&__args)"));
        // The binding path is relative to `bridges_gen` (where the scoped `generate!` lives).
        assert!(gen.contains("weather::bridge::forecast::lookup(&city)"));
        assert!(gen.contains("::serde_json::to_string(&__r)"));
        // A scoped, serde-deriving `generate!` over the bridge package heads the module.
        assert!(gen.contains("wit_bindgen::generate!({"));
        assert!(gen.contains("world: \"bridge-imports\""));
        assert!(gen.contains("generate_all"));
        assert!(gen.contains("additional_derives: [serde::Serialize, serde::Deserialize]"));
        // The JS wrapper marshals args→JSON to the primitive and parses the result back.
        assert!(gen.contains("pub const BRIDGE_JS: &str ="));
        assert!(gen.contains(
            "globalThis.weather.lookup = (city) => JSON.parse(__weather__lookup(JSON.stringify([city])));"
        ));
        // Shape matches the committed empty module (register + BRIDGE_JS).
        assert!(gen.contains("pub fn register<'js>(ctx: &Ctx<'js>, globals: &Object<'js>)"));
    }

    #[test]
    fn gen_delegate_host_generates_delegation_shim() {
        // Use the weather bridge (weather:bridge/forecast, `lookup(city: string) -> string`).
        let bridge = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::TypeScript(PathBuf::from("bridges/weather/host.ts")),
            dir: PathBuf::from("../examples/weather-api/bridges/weather"),
        };
        let contract = parse_contract(&bridge.wit()).unwrap();
        let shim = gen_delegate_host(&bridge, &contract).unwrap();
        // Header comment names the host file (language-specific) and explains the mechanism.
        assert!(shim.contains("GENERATED"), "header present: {shim}");
        assert!(shim.contains("bridges/weather/host.ts"), "TS host file in header: {shim}");
        assert!(shim.contains("~1–10µs"), "overhead disclosed: {shim}");
        // add_to_linker wires the WIT interface via the typed bindgen path.
        assert!(shim.contains("pub fn add_to_linker("), "linker fn: {shim}");
        assert!(
            shim.contains("add_to_linker::<_, ::rusm_wasm::wasmtime::component::HasSelf"),
            "linker call: {shim}"
        );
        // impl block for the forecast interface.
        assert!(
            shim.contains("impl crate::bindings::weather::bridge::forecast::Host"),
            "impl: {shim}"
        );
        // Function body: call_id, whereis runner, send request, recv_bridge_reply.
        assert!(shim.contains("CALL_CTR"), "counter: {shim}");
        assert!(shim.contains("\"bridge:weather\""), "runner name: {shim}");
        // The selective receive tag includes the `:` separator so that call_id
        // `rusm-bridge:weather-42-1` does not falsely match `rusm-bridge:weather-42-10:…`.
        assert!(shim.contains("recv_bridge_reply(&tag)"), "tagged recv: {shim}");
        assert!(shim.contains("format!(\"{}:\", call_id)"), "tag includes separator: {shim}");
        assert!(shim.contains("strip_prefix(tag.as_bytes())"), "strip tag prefix: {shim}");
        assert!(shim.contains("rusm_wasm::serde_json::to_string"), "arg marshal: {shim}");
        assert!(shim.contains("from_slice::<String>"), "reply deser: {shim}");
    }

    #[test]
    fn gen_delegate_host_go_names_host_go_in_header() {
        // The delegation shim is identical for TS and Go; only the header comment differs —
        // it names `host.go` for a Go bridge so the reader can find the implementation.
        let bridge = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::Go(PathBuf::from("bridges/weather/host.go")),
            dir: PathBuf::from("../examples/weather-api/bridges/weather"),
        };
        let contract = parse_contract(&bridge.wit()).unwrap();
        let shim = gen_delegate_host(&bridge, &contract).unwrap();
        assert!(shim.contains("bridges/weather/host.go"), "Go host file in header: {shim}");
        assert!(!shim.contains("host.ts"), "no TS mention in Go shim: {shim}");
        // Functional content is the same as the TS shim.
        assert!(shim.contains("pub fn add_to_linker("), "linker fn: {shim}");
        assert!(shim.contains("\"bridge:weather\""), "runner name: {shim}");
    }

    #[test]
    fn gen_ts_runner_generates_dispatch_loop() {
        let bridge = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::TypeScript(PathBuf::from("bridges/weather/host.ts")),
            dir: PathBuf::from("bridges/weather"),
        };
        let runner = gen_ts_runner(&bridge);
        assert!(runner.contains("GENERATED"), "header: {runner}");
        assert!(runner.contains("Process.register(\"bridge:weather\")"), "self-register: {runner}");
        assert!(runner.contains("Process.receiveText()"), "receive loop: {runner}");
        assert!(runner.contains("JSON.parse(raw)"), "parse request: {runner}");
        assert!(runner.contains("require(\"./host\")"), "loads user bridge: {runner}");
        assert!(runner.contains("JSON.stringify(result)"), "serialize reply: {runner}");
        assert!(runner.contains("callId}:"), "reply tagged with callId: {runner}");
    }

    #[test]
    fn gen_bridge_dts_declares_typed_globals() {
        let dts = gen_bridge_dts(std::slice::from_ref(&weather_bridge())).unwrap();
        assert!(dts.contains("declare const weather: {"));
        assert!(dts.contains("lookup(city: string): string;"));
    }

    #[test]
    fn stage_js_runner_writes_the_glue_and_the_scoped_bridge_world() {
        // Stage a per-app js-runner from the real runner source; the slow cargo→wizer build
        // itself is exercised by the example e2e — here we check the (fast) staging.
        let dir = app_dir("stage-runner");
        let dest = dir.join("runner");
        stage_js_runner(
            Path::new("../crates/rusm-wasm/js-runner"),
            &dest,
            std::slice::from_ref(&weather_bridge()),
        )
        .unwrap();
        // bridges_gen overwritten with the generated glue (relative binding path).
        let gen = std::fs::read_to_string(dest.join("src/bridges_gen.rs")).unwrap();
        assert!(gen.contains("__weather__lookup"));
        assert!(gen.contains("weather::bridge::forecast::lookup(&city)"));
        // the scoped `bridge-imports` world imports the bridge, with the contract vendored as a
        // dep — separate from the runner's untouched `process` world.
        let world = std::fs::read_to_string(dest.join("wit-bridges/world.wit")).unwrap();
        assert!(world.contains("world bridge-imports {"));
        assert!(world.contains("import weather:bridge/forecast@0.1.0;"));
        assert!(dest.join("wit-bridges/deps/weather/bridge.wit").is_file());
        // the runner's own `process` world is left as-is (no bridge import bleeds in).
        let process = std::fs::read_to_string(dest.join("wit/world.wit")).unwrap();
        assert!(!process.contains("weather:bridge"));
        // the runner source came along; `target/` did not.
        assert!(dest.join("src/lib.rs").is_file() && dest.join("Cargo.toml").is_file());
        assert!(
            !dest.join("target").exists(),
            "target/ is skipped (build cache)"
        );
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
        let root = app_dir("bridges-mod");
        let w = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::Rust(root.join("bridges/weather/host.rs")),
            dir: root.join("bridges/weather"),
        };
        let j = BridgeSpec {
            name: "json-codec".into(),
            host_impl: HostImpl::Rust(root.join("bridges/json-codec/host.rs")),
            dir: root.join("bridges/json-codec"),
        };
        let module = gen_bridges_module(&[&w, &j]);
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
        // Pure-Rust bridges: no `init` function.
        assert!(!module.contains("pub fn init("), "no init for Rust-only: {module}");
    }

    #[test]
    fn generates_bridges_module_with_init_for_ts_bridges() {
        let root = app_dir("bridges-mod-ts");
        let rs = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::Rust(root.join("bridges/weather/host.rs")),
            dir: root.join("bridges/weather"),
        };
        let ts = BridgeSpec {
            name: "notifier".into(),
            host_impl: HostImpl::TypeScript(root.join("bridges/notifier/host.ts")),
            dir: root.join("bridges/notifier"),
        };
        let module = gen_bridges_module(&[&rs, &ts]);
        // Rust bridge still mounted via #[path] to the author's file.
        assert!(module.contains("#[path = \"../bridges/weather/host.rs\"]"));
        // TS bridge mounted via the generated delegate shim.
        assert!(module.contains("#[path = \"bridge_notifier_delegate.rs\"]"));
        assert!(module.contains("pub mod notifier;"));
        // `init` function emitted (registers + boots the TS runner).
        assert!(module.contains("pub fn init(wasm: &rusm_wasm::WasmRuntime)"));
        assert!(module.contains("\"bridge:notifier\""));
        assert!(module.contains("wasm/bridge-notifier.js"));
        assert!(module.contains("wasm.supervise("));
        // TS path uses register_js_component_with (not compile_component_bytes).
        assert!(module.contains("register_js_component_with"), "TS uses JS registration: {module}");
    }

    #[test]
    fn generates_bridges_module_with_init_for_go_bridges() {
        let root = app_dir("bridges-mod-go");
        let rs = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::Rust(root.join("bridges/weather/host.rs")),
            dir: root.join("bridges/weather"),
        };
        let go = BridgeSpec {
            name: "signer".into(),
            host_impl: HostImpl::Go(root.join("bridges/signer/host.go")),
            dir: root.join("bridges/signer"),
        };
        let module = gen_bridges_module(&[&rs, &go]);
        // Rust bridge via #[path].
        assert!(module.contains("#[path = \"../bridges/weather/host.rs\"]"));
        // Go bridge via the generated delegate shim.
        assert!(module.contains("#[path = \"bridge_signer_delegate.rs\"]"));
        assert!(module.contains("pub mod signer;"));
        // `init` uses prepare_component_bytes + register_component_with (not JS).
        assert!(module.contains("pub fn init(wasm: &rusm_wasm::WasmRuntime)"));
        assert!(module.contains("\"bridge:signer\""));
        assert!(module.contains("wasm/bridge-signer.wasm"), "Go uses .wasm: {module}");
        assert!(module.contains("prepare_component_bytes"), "Go compiles wasm: {module}");
        assert!(module.contains("register_component_with"), "Go registers as component: {module}");
        assert!(module.contains("wasm.supervise("));
        // Go path must NOT use register_js_component_with.
        assert!(!module.contains("register_js_component_with"), "Go doesn't use JS registration: {module}");
    }

    #[test]
    fn discovers_ts_bridge() {
        let root = app_dir("ts-bridge");
        let dir = root.join("bridges/notifier");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bridge.wit"), "package notifier:bridge@0.1.0;\n").unwrap();
        std::fs::write(dir.join("host.ts"), "export async function ping() { return 'pong'; }\n")
            .unwrap();
        let found = discover(&root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "notifier");
        assert!(
            matches!(found[0].host_impl, HostImpl::TypeScript(_)),
            "host.ts → TypeScript variant"
        );
        assert!(!found[0].is_rust_host());
        assert_eq!(found[0].runner_name(), "bridge:notifier");
    }

    #[test]
    fn multiple_host_files_fails_loudly() {
        let root = app_dir("multi-host");
        let dir = root.join("bridges/weather");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bridge.wit"), "package weather:bridge@0.1.0;\n").unwrap();
        std::fs::write(dir.join("host.rs"), "// rs\n").unwrap();
        std::fs::write(dir.join("host.ts"), "// ts\n").unwrap();
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("multiple"), "names the conflict: {err}");
        // Covers the Go variant too.
        let root2 = app_dir("multi-host-go");
        let dir2 = root2.join("bridges/weather");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("bridge.wit"), "package weather:bridge@0.1.0;\n").unwrap();
        std::fs::write(dir2.join("host.go"), "// go\n").unwrap();
        std::fs::write(dir2.join("host.ts"), "// ts\n").unwrap();
        let err2 = discover(&root2).unwrap_err().to_string();
        assert!(err2.contains("multiple"), "go+ts conflict: {err2}");
    }

    #[test]
    fn discovers_go_bridge() {
        let root = app_dir("go-host");
        let dir = root.join("bridges/weather");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bridge.wit"), "package weather:bridge@0.1.0;\n").unwrap();
        std::fs::write(dir.join("host.go"), "package main\nfunc Lookup(city string) string { return city }\n").unwrap();
        let found = discover(&root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "weather");
        assert!(
            matches!(found[0].host_impl, HostImpl::Go(_)),
            "host.go → Go variant"
        );
        assert!(!found[0].is_rust_host());
        assert_eq!(found[0].runner_name(), "bridge:weather");
        assert_eq!(found[0].host(), dir.join("host.go"));
    }

    #[test]
    fn gen_go_bridge_runner_generates_dispatch_loop() {
        let bridge = BridgeSpec {
            name: "weather".into(),
            host_impl: HostImpl::Go(PathBuf::from("bridges/weather/host.go")),
            dir: PathBuf::from("../examples/weather-api/bridges/weather"),
        };
        let runner = gen_go_bridge_runner(&bridge).unwrap();
        assert!(runner.contains("GENERATED"), "header: {runner}");
        assert!(runner.contains("package main"), "package: {runner}");
        assert!(runner.contains("\"encoding/json\""), "json import: {runner}");
        assert!(runner.contains("rusm \"github.com/archan937/rusm/packages/rusm-go\""), "sdk import: {runner}");
        assert!(runner.contains("rusm.Register(\"bridge:weather\")"), "self-register: {runner}");
        assert!(runner.contains("rusm.Receive()"), "receive loop: {runner}");
        assert!(runner.contains("json.Unmarshal(raw, &req)"), "parse request: {runner}");
        assert!(runner.contains("case \"lookup\":"), "dispatch case: {runner}");
        assert!(runner.contains("var arg0 string"), "deser arg: {runner}");
        assert!(runner.contains("return Lookup(arg0)"), "calls user fn: {runner}");
        assert!(runner.contains("json.Marshal(result)"), "serialize reply: {runner}");
        assert!(runner.contains("req.ReplyTo.CallID+\":\""), "reply tagged with callId: {runner}");
        assert!(runner.contains("rusm.Send(req.ReplyTo.Pid, reply)"), "sends reply: {runner}");
    }

    #[test]
    fn gen_go_bridge_gomod_uses_sdk_version() {
        let gomod = gen_go_bridge_gomod("weather");
        assert!(gomod.contains("module bridge-weather"), "module name: {gomod}");
        assert!(gomod.contains("go 1.24"), "go version: {gomod}");
        assert!(
            gomod.contains(&format!(
                "require github.com/archan937/rusm/packages/rusm-go v{}",
                crate::scaffold::SDK_VERSION
            )),
            "sdk dep: {gomod}"
        );
    }

    #[test]
    fn go_dispatch_case_for_record_param_uses_raw_message() {
        // A WIT record param must emit `arg0 := args[0]` (pass-through) in the dispatch
        // switch — no intermediate Unmarshal — so the user's host.go function receives
        // `json.RawMessage` and can unmarshal into its own struct.
        let dir = app_dir("go-record-dispatch");
        let wit = dir.join("bridge.wit");
        std::fs::write(
            &wit,
            "package app:mailer@0.1.0;\n\
             interface smtp {\n\
             \x20   record message { to: string, subject: string }\n\
             \x20   send: func(msg: message) -> bool;\n\
             }\n",
        )
        .unwrap();
        let bridge = BridgeSpec {
            name: "mailer".into(),
            host_impl: HostImpl::Go(dir.join("host.go")),
            dir: dir.clone(),
        };
        let runner = gen_go_bridge_runner(&bridge).unwrap();
        // Record param → pass-through assignment, NOT json.Unmarshal.
        assert!(runner.contains("arg0 := args[0]"), "record → pass-through: {runner}");
        assert!(!runner.contains("json.Unmarshal(args[0]"), "no Unmarshal for record: {runner}");
        // The dispatch case still calls the user's PascalCase function.
        assert!(runner.contains("case \"send\":"), "dispatch case: {runner}");
        assert!(runner.contains("return Send(arg0)"), "calls Send: {runner}");
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
            "../examples/weather-api/bridges/weather/bridge.wit",
            weather.join("bridge.wit"),
        )
        .unwrap();
        std::fs::write(weather.join("host.rs"), "// host impl\n").unwrap();

        let bridges = generate_host_files(&root).unwrap();
        assert_eq!(bridges.len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("wit/world.wit")).unwrap(),
            include_str!("../../examples/weather-api/wit/world.wit"),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("wit/deps/weather/bridge.wit")).unwrap(),
            include_str!("../../examples/weather-api/wit/deps/weather/bridge.wit"),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/bindings.rs")).unwrap(),
            include_str!("../../examples/weather-api/src/bindings.rs"),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/bridges.rs")).unwrap(),
            include_str!("../../examples/weather-api/src/bridges.rs"),
        );
    }

    #[test]
    fn vendors_a_bridge_into_a_guest_component() {
        let root = app_dir("vendor");
        let weather = root.join("bridges/weather");
        std::fs::create_dir_all(&weather).unwrap();
        std::fs::copy(
            "../examples/weather-api/bridges/weather/bridge.wit",
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
            "../examples/weather-api/bridges/weather/bridge.wit",
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
            "../examples/weather-api/bridges/weather/bridge.wit",
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

    /// The worked example (`examples/weather-api/`) commits the *generated* files so it
    /// compiles in the workspace — proving the codegen output is valid Rust. This guards that
    /// those committed files are byte-identical to what the generator emits: the example
    /// can't drift from the generator, and `rusm build` reproduces the example exactly.
    #[test]
    fn the_worked_example_is_exactly_what_the_generator_emits() {
        let contract = parse_contract(Path::new(
            "../examples/weather-api/bridges/weather/bridge.wit",
        ))
        .unwrap();
        assert_eq!(
            synth_world(std::slice::from_ref(&contract)),
            include_str!("../../examples/weather-api/wit/world.wit"),
            "examples/weather-api/wit/world.wit is not what synth_world emits",
        );
        assert_eq!(
            gen_bridges_module(&[&weather_bridge()]),
            include_str!("../../examples/weather-api/src/bridges.rs"),
            "examples/weather-api/src/bridges.rs is not what gen_bridges_module emits",
        );
        assert_eq!(
            BINDINGS_RS,
            include_str!("../../examples/weather-api/src/bindings.rs"),
            "examples/weather-api/src/bindings.rs is not what BINDINGS_RS holds",
        );
        // The TS guest's ambient types are the generator's output too.
        assert_eq!(
            gen_bridge_dts(std::slice::from_ref(&weather_bridge())).unwrap(),
            include_str!("../../examples/weather-api/bridges.d.ts"),
            "examples/weather-api/bridges.d.ts is not what gen_bridge_dts emits",
        );
    }
}
