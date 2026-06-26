//! Discovery + host-glue codegen for an app's serving **auth hooks**.
//!
//! An auth hook is host code at `auth/<name>/host.{rs,ts,go}` that validates an incoming
//! request and either seeds its host-only **claims context** (the tenant a multi-tenant
//! bridge then acts for) or rejects it with `401`. A `[[serve]] authentication = "<name>"`
//! selects one per listener. Like a [custom bridge](crate::bridges) the *presence* of a
//! well-formed `auth/<name>/` is the declaration — no manifest entry beyond `authentication`.
//!
//! Unlike a bridge an auth hook is **not** a WIT interface a guest imports — it is a host
//! closure ([`rusm_wasm::AuthHook`]) the runtime runs before spawning a handler. So discovery
//! needs no `bridge.wit`; it locates `auth/<name>/host.*` and the codegen wires each into the
//! generated host crate, registering it on the runtime via `WasmRuntime::register_auth_hook`.
//!
//! Host implementations (exactly one per hook):
//! - `host.rs` — Rust: compiled into the host binary; the hook is `async fn authenticate`.
//! - `host.ts` / `host.go` — a resident runner the hook delegates to (the bridge pattern).

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::bridges::HostImpl;

/// A discovered auth hook: its `name` (the directory name, also the `authentication = "<name>"`
/// value), the `dir`, and how its host side is authored ([`HostImpl`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSpec {
    pub name: String,
    pub dir: PathBuf,
    pub host_impl: HostImpl,
}

impl AuthSpec {
    /// Whether this hook is authored in Rust (compiled directly into the host binary).
    pub fn is_rust_host(&self) -> bool {
        matches!(self.host_impl, HostImpl::Rust(_))
    }

    /// The component name a delegated (TS/Go) auth runner registers as (`"auth:<name>"`).
    pub fn runner_name(&self) -> String {
        format!("auth:{}", self.name)
    }
}

/// Discover the auth hooks under `<root>/auth/`. Sorted by name (a stable order, so generated
/// code is deterministic). No `auth/` dir → none (an empty list, not an error). An
/// `auth/<name>/` with no — or more than one — `host.{rs,ts,go}` is malformed and fails loudly.
pub fn discover(root: &Path) -> Result<Vec<AuthSpec>> {
    let dir = root.join("auth");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut hooks = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue; // ignore stray files (e.g. a README) directly under auth/
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let rs = path.join("host.rs");
        let ts = path.join("host.ts");
        let go = path.join("host.go");
        let host_impl = match (rs.is_file(), ts.is_file(), go.is_file()) {
            (true, false, false) => HostImpl::Rust(rs),
            (false, true, false) => HostImpl::TypeScript(ts),
            (false, false, true) => HostImpl::Go(go),
            (false, false, false) => bail!(
                "auth hook `{name}` needs a host implementation — \
                 add auth/{name}/host.rs (Rust), auth/{name}/host.ts (TypeScript), \
                 or auth/{name}/host.go (Go)"
            ),
            _ => bail!(
                "auth hook `{name}` has multiple host implementation files — \
                 keep exactly one of host.rs, host.ts, or host.go"
            ),
        };
        hooks.push(AuthSpec {
            name,
            dir: path,
            host_impl,
        });
    }
    hooks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(hooks)
}

/// Whether `root` declares any auth hook (an `auth/<name>/` directory). Such an app — like a
/// custom-bridge app — serves via its own generated host binary (the hook is host code).
pub fn has_auth_hooks(root: &Path) -> bool {
    discover(root).map(|h| !h.is_empty()).unwrap_or(false)
}

/// An auth directory name as a Rust module identifier: hyphens → underscores. The `#[path]`
/// keeps the real directory name.
fn module_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Emit, for the generated host crate's `src/bridges.rs`, the **module mounts** for each Rust
/// auth hook (`#[path = "../auth/<name>/host.rs"]`). TS/Go hooks delegate to a resident runner
/// and are mounted by their generated shim instead (see [`crate::bridges`]).
pub fn gen_auth_mounts(hooks: &[AuthSpec]) -> String {
    let mut out = String::new();
    for hook in hooks.iter().filter(|h| h.is_rust_host()) {
        let ident = module_ident(&hook.name);
        out.push_str(&format!(
            "#[path = \"../auth/{}/host.rs\"]\npub mod auth_{ident};\n\n",
            hook.name
        ));
    }
    out
}

/// Emit the body that registers every auth hook on the runtime — appended to the generated
/// host crate's `init`. A **Rust** hook is wrapped into an [`rusm_wasm::AuthHook`] around its
/// `authenticate` fn (zero delegation). A **TS/Go** hook boots its dispatch runner as a
/// supervised resident (`auth:<name>`) and registers an [`rusm_wasm::delegated_auth_hook`] that
/// round-trips to it. Empty when there are no hooks, so a bridges-only app's `init` is unchanged.
pub fn gen_auth_registration(hooks: &[AuthSpec]) -> String {
    let mut out = String::new();
    for hook in hooks {
        let name = &hook.name;
        let runner = hook.runner_name(); // "auth:<name>"
        match &hook.host_impl {
            HostImpl::Rust(_) => {
                let ident = module_ident(name);
                out.push_str(&format!(
                    "    wasm.register_auth_hook(\n\
                     \x20       \"{name}\",\n\
                     \x20       rusm_wasm::auth_hook(auth_{ident}::authenticate),\n\
                     \x20   );\n"
                ));
            }
            HostImpl::TypeScript(_) => {
                let js_file = format!("wasm/auth-{name}.js");
                out.push_str(&format!(
                    "    wasm.register_js_component_with(\n\
                     \x20       \"{runner}\".to_string(),\n\
                     \x20       std::fs::read(\"{js_file}\")\n\
                     \x20           .map_err(|e| anyhow::anyhow!(\"{js_file}: {{}}\", e))?,\n\
                     \x20       rusm_wasm::CapabilityProfile::Trusted.capabilities(),\n\
                     \x20   );\n\
                     \x20   wasm.supervise(&[\"{runner}\".to_string()]);\n\
                     \x20   wasm.register_auth_hook(\n\
                     \x20       \"{name}\",\n\
                     \x20       rusm_wasm::delegated_auth_hook(wasm.runtime_handle(), \"{runner}\".to_string()),\n\
                     \x20   );\n"
                ));
            }
            HostImpl::Go(_) => {
                let wasm_file = format!("wasm/auth-{name}.wasm");
                out.push_str(&format!(
                    "    wasm.register_component_with(\n\
                     \x20       \"{runner}\".to_string(),\n\
                     \x20       wasm.prepare_component_bytes(\n\
                     \x20           &std::fs::read(\"{wasm_file}\")\n\
                     \x20               .map_err(|e| anyhow::anyhow!(\"{wasm_file}: {{}}\", e))?,\n\
                     \x20       ).map_err(|e| anyhow::anyhow!(\"compiling {runner}: {{}}\", e))?,\n\
                     \x20       rusm_wasm::CapabilityProfile::Trusted.capabilities(),\n\
                     \x20   );\n\
                     \x20   wasm.supervise(&[\"{runner}\".to_string()]);\n\
                     \x20   wasm.register_auth_hook(\n\
                     \x20       \"{name}\",\n\
                     \x20       rusm_wasm::delegated_auth_hook(wasm.runtime_handle(), \"{runner}\".to_string()),\n\
                     \x20   );\n"
                ));
            }
        }
    }
    out
}

/// The TS **auth dispatch runner** for an `auth/<name>/host.ts`: a resident actor (`auth:<name>`)
/// that calls the host's exported `authenticate`. Reuses the shared TS dispatch runner (TS
/// dispatch is dynamic — the same machinery a custom-bridge runner uses).
pub fn gen_ts_auth_runner(name: &str) -> String {
    crate::bridges::gen_ts_dispatch_runner(name, &format!("auth:{name}"))
}

/// The Go **auth dispatch runner** for an `auth/<name>/host.go`: a TinyGo/rusm-go resident actor
/// (`auth:<name>`) that receives the `{fn, args, replyTo}` envelope, decodes the [`AuthRequest`]
/// from `args[0]`, calls the user's exported `Authenticate`, and replies the tagged verdict.
/// Unlike a Go *bridge* runner this needs no WIT — the one function is fixed (`authenticate`).
pub fn gen_go_auth_runner(name: &str) -> String {
    let runner = format!("auth:{name}");
    format!(
        "// GENERATED by `rusm build` — do not edit.\n\
         // Auth dispatch runner for `{name}`: validates each request via host.go's\n\
         // Authenticate and replies the verdict. Runs as a resident actor: \"{runner}\".\n\
         package main\n\n\
         import (\n\
         \t\"encoding/json\"\n\n\
         \trusm \"github.com/archan937/rusm/packages/rusm-go\"\n\
         )\n\n\
         type authEnvelope struct {{\n\
         \tFn      string            `json:\"fn\"`\n\
         \tArgs    []json.RawMessage `json:\"args\"`\n\
         \tReplyTo struct {{\n\
         \t\tPid    string `json:\"pid\"`\n\
         \t\tCallID string `json:\"callId\"`\n\
         \t}} `json:\"replyTo\"`\n\
         }}\n\n\
         // The rusm-go component shell: `init` wires the entry, `main` is empty, the logic\n\
         // lives in `run` (the actor world's `run` export — not a wasi:cli `main`).\n\
         func init() {{ rusm.Run(run) }}\n\
         func main() {{}}\n\n\
         func run() {{\n\
         \trusm.Register(\"{runner}\")\n\
         \trusm.SetLabel(\"{runner}\")\n\
         \tfor {{\n\
         \t\traw := rusm.ReceiveBytes()\n\
         \t\tvar env authEnvelope\n\
         \t\tif err := json.Unmarshal(raw, &env); err != nil {{\n\
         \t\t\tcontinue\n\
         \t\t}}\n\
         \t\tvar verdict rusm.AuthVerdict\n\
         \t\tif env.Fn == \"authenticate\" && len(env.Args) >= 1 {{\n\
         \t\t\tvar req rusm.AuthRequest\n\
         \t\t\tif json.Unmarshal(env.Args[0], &req) == nil {{\n\
         \t\t\t\tverdict = Authenticate(req)\n\
         \t\t\t}}\n\
         \t\t}}\n\
         \t\tresultBytes, _ := json.Marshal(verdict)\n\
         \t\treply := append([]byte(env.ReplyTo.CallID+\":\"), resultBytes...)\n\
         \t\tif pid, ok := rusm.ParsePid(env.ReplyTo.Pid); ok {{\n\
         \t\t\trusm.SendBytes(pid, reply)\n\
         \t\t}}\n\
         \t}}\n\
         }}\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rusm-auth-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_auth_dir_is_empty_not_an_error() {
        let root = tmp("none");
        assert!(discover(&root).unwrap().is_empty());
        assert!(!has_auth_hooks(&root));
    }

    #[test]
    fn discovers_each_hook_by_language_sorted() {
        let root = tmp("langs");
        write(&root, "auth/jwt/host.rs", "pub async fn authenticate() {}");
        write(&root, "auth/api-key/host.ts", "export default {}");
        let found = discover(&root).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "api-key"); // sorted
        assert!(matches!(found[0].host_impl, HostImpl::TypeScript(_)));
        assert_eq!(found[1].name, "jwt");
        assert!(found[1].is_rust_host());
        assert_eq!(found[1].runner_name(), "auth:jwt");
        assert!(has_auth_hooks(&root));
    }

    #[test]
    fn a_hook_with_no_host_impl_is_an_error() {
        let root = tmp("empty-hook");
        std::fs::create_dir_all(root.join("auth/broken")).unwrap();
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("needs a host implementation"), "got: {err}");
    }

    #[test]
    fn a_hook_with_two_host_impls_is_an_error() {
        let root = tmp("dual-hook");
        write(&root, "auth/dual/host.rs", "x");
        write(&root, "auth/dual/host.go", "x");
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("multiple host implementation"), "got: {err}");
    }

    #[test]
    fn rust_hook_codegen_mounts_and_registers() {
        let hooks = vec![AuthSpec {
            name: "jwt".to_string(),
            dir: PathBuf::from("auth/jwt"),
            host_impl: HostImpl::Rust(PathBuf::from("auth/jwt/host.rs")),
        }];
        let mounts = gen_auth_mounts(&hooks);
        assert!(mounts.contains("#[path = \"../auth/jwt/host.rs\"]"));
        assert!(mounts.contains("pub mod auth_jwt;"));
        let reg = gen_auth_registration(&hooks);
        assert!(reg.contains("wasm.register_auth_hook("));
        assert!(reg.contains("\"jwt\""));
        // Uses the `auth_hook` constructor (boxes the future correctly) — not a bare closure,
        // which would not reliably coerce to the `AuthHook` trait object in the generated crate.
        assert!(reg.contains("rusm_wasm::auth_hook(auth_jwt::authenticate)"));
        // A Rust hook is mounted + compiled in — no runner, no delegation.
        assert!(!reg.contains("delegated_auth_hook"));
    }

    #[test]
    fn ts_hook_codegen_boots_a_runner_and_registers_a_delegated_hook() {
        let hooks = vec![AuthSpec {
            name: "jwt".to_string(),
            dir: PathBuf::from("auth/jwt"),
            host_impl: HostImpl::TypeScript(PathBuf::from("auth/jwt/host.ts")),
        }];
        // No Rust module is mounted for a TS hook (it runs in a runner, not compiled in).
        assert!(gen_auth_mounts(&hooks).is_empty());
        let reg = gen_auth_registration(&hooks);
        assert!(
            reg.contains("register_js_component_with(")
                && reg.contains("\"auth:jwt\"")
                && reg.contains("wasm/auth-jwt.js")
                && reg.contains("supervise(&[\"auth:jwt\".to_string()])"),
            "the TS runner is registered + supervised: {reg}"
        );
        assert!(
            reg.contains(
                "rusm_wasm::delegated_auth_hook(wasm.runtime_handle(), \"auth:jwt\".to_string())"
            ),
            "a delegating hook round-trips to the runner: {reg}"
        );
        // The TS runner reuses the shared dispatch runner, registered under the auth name.
        let runner = gen_ts_auth_runner("jwt");
        assert!(runner.contains("\"auth:jwt\"") && runner.contains("require(\"./host\")"));
    }

    #[test]
    fn go_hook_codegen_boots_a_runner_and_registers_a_delegated_hook() {
        let hooks = vec![AuthSpec {
            name: "jwt".to_string(),
            dir: PathBuf::from("auth/jwt"),
            host_impl: HostImpl::Go(PathBuf::from("auth/jwt/host.go")),
        }];
        let reg = gen_auth_registration(&hooks);
        assert!(
            reg.contains("register_component_with(")
                && reg.contains("wasm/auth-jwt.wasm")
                && reg.contains("supervise(&[\"auth:jwt\".to_string()])")
                && reg.contains(
                    "rusm_wasm::delegated_auth_hook(wasm.runtime_handle(), \"auth:jwt\".to_string())"
                ),
            "the Go runner is registered + supervised + delegated: {reg}"
        );
        // The Go runner has a fixed `authenticate` dispatch over the rusm-go SDK types.
        let runner = gen_go_auth_runner("jwt");
        assert!(
            runner.contains("rusm.Register(\"auth:jwt\")")
                && runner.contains("var req rusm.AuthRequest")
                && runner.contains("verdict = Authenticate(req)"),
            "the Go auth runner dispatches authenticate: {runner}"
        );
    }
}
