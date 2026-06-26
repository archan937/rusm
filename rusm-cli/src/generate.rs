//! `rusm generate component <name> [--lang ts|rust|go] [--protocol http|sse|ws]`
//! `rusm generate bridge <name> [--lang ts|rust|go]`
//! `rusm generate authentication <name> [--lang ts|rust|go]`
//!
//! Adds a component, bridge, or auth hook to an **existing** RUSM project (a directory that
//! already has a `rusm.toml`). Unlike `rusm new`, it never creates a project skeleton and never
//! modifies files it did not create — only new `components/<name>/`, `bridges/<name>/`, or
//! `auth/<name>/` directories and a targeted append to `rusm.toml`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::scaffold::{
    cargo_toml, go_component, go_mod, package_json, parse_lang, parse_protocol, rust_component,
    ts_component, validate_name, Lang, Protocol,
};

// ── Types ─────────────────────────────────────────────────────────────────────

/// A parsed `rusm generate component` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateComponent {
    pub name: String,
    pub lang: Lang,
    pub protocol: Protocol,
}

/// A parsed `rusm generate bridge` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateBridge {
    pub name: String,
    pub lang: Lang,
}

/// A parsed `rusm generate authentication` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateAuth {
    pub name: String,
    pub lang: Lang,
}

/// The result of parsing `rusm generate <subcommand> …`.
pub enum GenerateCommand {
    Component(GenerateComponent),
    Bridge(GenerateBridge),
    Authentication(GenerateAuth),
}

// ── Parsing ───────────────────────────────────────────────────────────────────

/// Parse the arguments following `rusm generate`.
pub fn parse_generate_args(mut args: pico_args::Arguments) -> Result<GenerateCommand> {
    let sub = args.subcommand()?;
    match sub.as_deref() {
        Some("component") => Ok(GenerateCommand::Component(parse_component_args(args)?)),
        Some("bridge") => Ok(GenerateCommand::Bridge(parse_bridge_args(args)?)),
        Some("authentication") | Some("auth") => {
            Ok(GenerateCommand::Authentication(parse_auth_args(args)?))
        }
        Some(other) => bail!(
            "unknown generate subcommand `{other}` — use `component`, `bridge`, or `authentication`\n\
             usage: rusm generate component|bridge|authentication <name> [options]"
        ),
        None => bail!(
            "usage: rusm generate component|bridge|authentication <name> [options]\n\
             Try `rusm generate --help` for details."
        ),
    }
}

fn parse_component_args(mut args: pico_args::Arguments) -> Result<GenerateComponent> {
    let lang_str = args.opt_value_from_str::<_, String>("--lang")?;
    let protocol_str = match args.opt_value_from_str::<_, String>("--protocol")? {
        Some(v) => Some(v),
        None => args.opt_value_from_str::<_, String>("-p")?,
    };
    let name: String = args.free_from_str().map_err(|_| {
        anyhow!(
            "usage: rusm generate component <name> \
             [--lang ts|rust|go] [--protocol http|sse|ws]"
        )
    })?;
    validate_name(&name)?;
    if let Some(extra) = args.finish().first() {
        bail!(
            "unexpected argument `{}` — the component name is already `{name}`",
            extra.to_string_lossy()
        );
    }
    let lang = lang_str
        .as_deref()
        .map(parse_lang)
        .transpose()?
        .unwrap_or(Lang::TypeScript);
    if matches!(lang, Lang::Generic) {
        bail!("a generated component must have source — use `--lang ts`, `rust`, or `go`");
    }
    Ok(GenerateComponent {
        protocol: protocol_str
            .as_deref()
            .map(parse_protocol)
            .transpose()?
            .unwrap_or(Protocol::Http),
        name,
        lang,
    })
}

fn parse_bridge_args(mut args: pico_args::Arguments) -> Result<GenerateBridge> {
    let lang_str = args.opt_value_from_str::<_, String>("--lang")?;
    let name: String = args
        .free_from_str()
        .map_err(|_| anyhow!("usage: rusm generate bridge <name> [--lang ts|rust|go]"))?;
    validate_name(&name)?;
    if let Some(extra) = args.finish().first() {
        bail!(
            "unexpected argument `{}` — the bridge name is already `{name}`",
            extra.to_string_lossy()
        );
    }
    let lang = lang_str
        .as_deref()
        .map(parse_lang)
        .transpose()?
        .unwrap_or(Lang::TypeScript);
    if matches!(lang, Lang::Generic) {
        bail!("a bridge host must be a real language — use `--lang ts`, `rust`, or `go`");
    }
    Ok(GenerateBridge { name, lang })
}

fn parse_auth_args(mut args: pico_args::Arguments) -> Result<GenerateAuth> {
    let lang_str = args.opt_value_from_str::<_, String>("--lang")?;
    let name: String = args
        .free_from_str()
        .map_err(|_| anyhow!("usage: rusm generate authentication <name> [--lang ts|rust|go]"))?;
    validate_name(&name)?;
    if let Some(extra) = args.finish().first() {
        bail!(
            "unexpected argument `{}` — the auth hook name is already `{name}`",
            extra.to_string_lossy()
        );
    }
    let lang = lang_str
        .as_deref()
        .map(parse_lang)
        .transpose()?
        .unwrap_or(Lang::TypeScript);
    if matches!(lang, Lang::Generic) {
        bail!("an auth hook must be a real language — use `--lang ts`, `rust`, or `go`");
    }
    Ok(GenerateAuth { name, lang })
}

// ── Generation ────────────────────────────────────────────────────────────────

/// Add a component to an existing RUSM project at `root`.
///
/// Creates `components/<name>/` source files and appends the matching entry to
/// `rusm.toml`. Errors if `rusm.toml` is missing, `components/<name>/` already exists,
/// or the component is already declared in `rusm.toml`.
pub fn generate_component(root: &Path, gen: &GenerateComponent) -> Result<Vec<PathBuf>> {
    validate_project(root)?;
    ensure_no_component_dir(root, &gen.name)?;
    ensure_not_in_toml(root, &gen.name)?;

    let mut files = component_source_files(gen);

    // TS WS imports from rusm-ts; add package.json if the project doesn't have one yet.
    if gen.lang == Lang::TypeScript && gen.protocol == Protocol::Ws {
        let pkg = root.join("package.json");
        if !pkg.exists() {
            files.push((PathBuf::from("package.json"), package_json(&gen.name)));
        }
    }
    // Any TS component benefits from tsconfig.json for type-checking.
    if gen.lang == Lang::TypeScript && !root.join("tsconfig.json").exists() {
        files.push((PathBuf::from("tsconfig.json"), TSCONFIG.to_string()));
    }

    let mut created = Vec::new();
    for (rel, contents) in files {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &contents).with_context(|| format!("writing {}", path.display()))?;
        created.push(rel);
    }
    patch_toml_component(root, gen)?;
    Ok(created)
}

/// Add a bridge to an existing RUSM project at `root`.
///
/// Creates `bridges/<name>/bridge.wit` and `bridges/<name>/host.<ext>`, then appends
/// an instructional comment to `rusm.toml` showing how to grant the bridge to a
/// capability. Errors if `rusm.toml` is missing or `bridges/<name>/` already exists.
pub fn generate_bridge(root: &Path, gen: &GenerateBridge) -> Result<Vec<PathBuf>> {
    validate_project(root)?;
    ensure_no_bridge_dir(root, &gen.name)?;

    let mut created = Vec::new();
    for (rel, contents) in bridge_source_files(gen) {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &contents).with_context(|| format!("writing {}", path.display()))?;
        created.push(rel);
    }
    patch_toml_bridge(root, &gen.name)?;
    Ok(created)
}

/// Add a serving **auth hook** to an existing RUSM project at `root`.
///
/// Creates `auth/<name>/host.<ext>` (a starter `authenticate`), then appends a comment to
/// `rusm.toml` showing how to apply it to a listener (`authentication = "<name>"`). Errors if
/// `rusm.toml` is missing or `auth/<name>/` already exists.
pub fn generate_authentication(root: &Path, gen: &GenerateAuth) -> Result<Vec<PathBuf>> {
    validate_project(root)?;
    if root.join("auth").join(&gen.name).exists() {
        bail!("auth/{} already exists", gen.name);
    }

    let rel = PathBuf::from("auth")
        .join(&gen.name)
        .join(host_filename(gen.lang));
    let path = root.join(&rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, auth_host(gen.lang))
        .with_context(|| format!("writing {}", path.display()))?;
    patch_toml_auth(root, &gen.name)?;
    Ok(vec![rel])
}

// ── File content ──────────────────────────────────────────────────────────────

fn component_source_files(gen: &GenerateComponent) -> Vec<(PathBuf, String)> {
    let base = PathBuf::from("components").join(&gen.name);
    match gen.lang {
        Lang::TypeScript => vec![(
            base.join("index.ts"),
            ts_component(gen.protocol).to_string(),
        )],
        Lang::Rust => vec![
            (base.join("Cargo.toml"), cargo_toml(&gen.name)),
            (
                base.join("src/lib.rs"),
                rust_component(gen.protocol, &gen.name),
            ),
        ],
        Lang::Go => vec![
            (base.join("go.mod"), go_mod(&gen.name)),
            (base.join("main.go"), go_component(gen.protocol).to_string()),
        ],
        Lang::Generic => unreachable!("Generic rejected by parser"),
    }
}

fn bridge_source_files(gen: &GenerateBridge) -> Vec<(PathBuf, String)> {
    let base = PathBuf::from("bridges").join(&gen.name);
    vec![
        (base.join("bridge.wit"), bridge_wit(&gen.name)),
        (
            base.join(host_filename(gen.lang)),
            bridge_host(&gen.name, gen.lang),
        ),
    ]
}

/// Starter `auth/<name>/host.<ext>` — an `authenticate` that denies by default (fail-closed),
/// with the allow path shown. Host code: it validates the request and returns claims (the
/// tenant a bridge then acts for) or a denial; guest components never see it.
fn auth_host(lang: Lang) -> String {
    match lang {
        Lang::Rust => "// auth/<name>/host.rs — the only Rust an auth hook must add.\n\
             // Validate the request and return claims (the tenant a bridge acts for) or a denial.\n\
             use rusm_wasm::{AuthRequest, AuthVerdict};\n\
             \n\
             pub async fn authenticate(req: AuthRequest) -> AuthVerdict {\n\
             \x20\x20\x20\x20// TODO: verify the token and derive the tenant. For example:\n\
             \x20\x20\x20\x20//   match req.header(\"authorization\") {\n\
             \x20\x20\x20\x20//       Some(tok) if verify(tok) => return AuthVerdict::Allow(\n\
             \x20\x20\x20\x20//           vec![(\"app_id\".to_string(), tenant_of(tok))]),\n\
             \x20\x20\x20\x20//       _ => {}\n\
             \x20\x20\x20\x20//   }\n\
             \x20\x20\x20\x20let _ = req;\n\
             \x20\x20\x20\x20AuthVerdict::Deny\n\
             }\n"
            .to_string(),
        Lang::Go => "// auth/<name>/host.go — the only Go an auth hook must write.\n\
             // Validate the request and return claims (the tenant a bridge acts for) or a denial.\n\
             package main\n\
             \n\
             import rusm \"github.com/archan937/rusm/packages/rusm-go\"\n\
             \n\
             func Authenticate(req rusm.AuthRequest) rusm.AuthVerdict {\n\
             \t// TODO: verify the token and derive the tenant. For example:\n\
             \t//   if ok, appID := verify(req.Header(\"authorization\")); ok {\n\
             \t//       return rusm.Allow(map[string]string{\"app_id\": appID})\n\
             \t//   }\n\
             \treturn rusm.Deny()\n\
             }\n"
            .to_string(),
        Lang::TypeScript => "// auth/<name>/host.ts — the only file a TypeScript auth hook must write.\n\
             // Export `authenticate(req)`; return { allow: { app_id: \"…\" } } or { deny: true }.\n\
             // `req` is { method, path, query, headers: [name, value][] } — a WebSocket token\n\
             // usually arrives in `query` (browsers can't set Authorization on a WS).\n\
             \n\
             export async function authenticate(req) {\n\
             \x20\x20// TODO: verify the token and derive the tenant. For example:\n\
             \x20\x20//   const auth = req.headers.find(([k]) => k.toLowerCase() === \"authorization\")?.[1];\n\
             \x20\x20//   if (verify(auth)) return { allow: { app_id: tenantOf(auth) } };\n\
             \x20\x20return { deny: true };\n\
             }\n"
            .to_string(),
        Lang::Generic => unreachable!("Generic rejected by parser"),
    }
}

fn host_filename(lang: Lang) -> &'static str {
    match lang {
        Lang::TypeScript => "host.ts",
        Lang::Rust => "host.rs",
        Lang::Go => "host.go",
        Lang::Generic => unreachable!("Generic rejected by parser"),
    }
}

fn bridge_wit(name: &str) -> String {
    format!(
        "package app:{name}@0.1.0;\n\
         \n\
         interface {name} {{\n\
         \x20\x20\x20\x20// Define your bridge contract here.\n\
         \x20\x20\x20\x20//\n\
         \x20\x20\x20\x20// Example:\n\
         \x20\x20\x20\x20//   record message {{ to: string, subject: string, body: string }}\n\
         \x20\x20\x20\x20//   send: func(msg: message) -> bool;\n\
         }}\n"
    )
}

fn bridge_host(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Rust => format!(
            "// bridges/{name}/host.rs — the only Rust a Rust bridge must add.\n\
             // Add any crates this bridge needs to Cargo.toml.\n\
             use crate::bindings::app::{name}::{name};\n\
             use rusm_wasm::wasmtime::component::HasSelf;\n\
             use rusm_wasm::{{wasmtime, BridgeHost, BridgeLinker}};\n\
             \n\
             pub fn add_to_linker(linker: &mut BridgeLinker) -> wasmtime::Result<()> {{\n\
             \x20\x20\x20\x20{name}::add_to_linker::<_, HasSelf<BridgeHost>>(linker, |host| host)\n\
             }}\n\
             \n\
             impl {name}::Host for BridgeHost {{\n\
             \x20\x20\x20\x20// TODO: implement the functions from bridge.wit here.\n\
             }}\n"
        ),
        Lang::Go => format!(
            "// bridges/{name}/host.go — the only Go a Go bridge must write.\n\
             // The generated dispatcher calls each exported function with JSON-encoded args.\n\
             package main\n\
             \n\
             import \"encoding/json\"\n\
             \n\
             // TODO: implement the functions from bridge.wit as exported Go functions.\n\
             // Primitive params arrive as their Go type; record/variant/result params arrive as json.RawMessage.\n\
             //\n\
             // func Send(raw json.RawMessage) bool {{\n\
             //\x20\x20\x20var msg struct{{ To, Subject, Body string }}\n\
             //\x20\x20\x20if err := json.Unmarshal(raw, &msg); err != nil {{ return false }}\n\
             //\x20\x20\x20return true\n\
             // }}\n\
             \n\
             var _ = json.RawMessage{{}} // suppress unused import until functions are added\n"
        ),
        Lang::TypeScript => format!(
            "// bridges/{name}/host.ts — the only file a TypeScript bridge must write.\n\
             // Export one async function per function declared in bridge.wit.\n\
             // `rusm build` generates the runner, dispatcher, and Rust glue from this file.\n\
             \n\
             // TODO: export the functions from bridge.wit.\n\
             //\n\
             // Example:\n\
             // export async function send(\n\
             //   msg: {{ to: string; subject: string; body: string }}\n\
             // ): Promise<boolean> {{\n\
             //   // call external API ...\n\
             //   return true;\n\
             // }}\n"
        ),
        Lang::Generic => unreachable!("Generic rejected by parser"),
    }
}

const TSCONFIG: &str = "\
{
  \"compilerOptions\": {
    \"target\": \"ES2022\",
    \"module\": \"ESNext\",
    \"moduleResolution\": \"bundler\",
    \"lib\": [\"ES2022\", \"DOM\"],
    \"strict\": true,
    \"skipLibCheck\": true,
    \"noEmit\": true,
    \"types\": []
  },
  \"include\": [\"components/**/*.ts\"]
}
";

// ── rusm.toml patching ────────────────────────────────────────────────────────

fn patch_toml_component(root: &Path, gen: &GenerateComponent) -> Result<()> {
    let toml_path = root.join("rusm.toml");
    let existing = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let patched = format!("{}\n{}", existing.trim_end(), component_toml_patch(gen));
    std::fs::write(&toml_path, patched).with_context(|| format!("writing {}", toml_path.display()))
}

fn component_toml_patch(gen: &GenerateComponent) -> String {
    let name = &gen.name;
    if is_routed(gen.lang, gen.protocol) {
        // Rust/Go HTTP: handler dispatched from [serve.routes] on an existing listener.
        // Append the [components.<name>] section; the user wires the route themselves.
        format!(
            "\n\
             # Wire to a listener via [serve.routes]: \"GET /path\" = \"{name}#action\"\n\
             [components.{name}]\n\
             capability = \"sandboxed\"\n"
        )
    } else {
        // Non-routed: the [[serve]] entry IS the component declaration; no separate
        // [components.<name>] needed (mirrors the rusm new non-routed layout).
        let proto = gen.protocol.as_str();
        format!(
            "\n\
             [[serve]]\n\
             component = \"{name}\"\n\
             protocol = \"{proto}\"\n\
             listen = \"127.0.0.1:8081\"   # TODO: update the listen address if needed\n"
        )
    }
}

fn patch_toml_bridge(root: &Path, name: &str) -> Result<()> {
    let toml_path = root.join("rusm.toml");
    let existing = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let comment = format!(
        "\n\
         # Bridge '{name}' — grant it in a capability to call it from a guest:\n\
         #   [capabilities.my-cap]\n\
         #   inherits = \"sandboxed\"\n\
         #   bridges = [\"{name}\"]\n\
         # Then set capability = \"my-cap\" on the component(s) that import it.\n"
    );
    let patched = format!("{}\n{}", existing.trim_end(), comment);
    std::fs::write(&toml_path, patched).with_context(|| format!("writing {}", toml_path.display()))
}

fn patch_toml_auth(root: &Path, name: &str) -> Result<()> {
    let toml_path = root.join("rusm.toml");
    let existing = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let comment = format!(
        "\n\
         # Auth hook '{name}' — apply it to a listener by adding to its [[serve]] entry:\n\
         #   authentication = \"{name}\"\n\
         # It runs before each request: a valid token seeds the request's context (which a\n\
         # bridge reads to act for the right tenant); an invalid one is rejected with 401.\n"
    );
    let patched = format!("{}\n{}", existing.trim_end(), comment);
    std::fs::write(&toml_path, patched).with_context(|| format!("writing {}", toml_path.display()))
}

/// True for component shapes that use named handler actions dispatched via `[serve.routes]`.
fn is_routed(lang: Lang, protocol: Protocol) -> bool {
    matches!(lang, Lang::Rust | Lang::Go) && protocol == Protocol::Http
}

// ── Guards ────────────────────────────────────────────────────────────────────

fn validate_project(root: &Path) -> Result<()> {
    if !root.join("rusm.toml").exists() {
        bail!(
            "no rusm.toml found — run `rusm new <name>` to create a project first, \
             or run this command inside an existing project directory"
        );
    }
    Ok(())
}

fn ensure_no_component_dir(root: &Path, name: &str) -> Result<()> {
    if root.join("components").join(name).exists() {
        bail!("components/{name} already exists");
    }
    Ok(())
}

fn ensure_not_in_toml(root: &Path, name: &str) -> Result<()> {
    let content = std::fs::read_to_string(root.join("rusm.toml"))
        .with_context(|| format!("reading {}", root.join("rusm.toml").display()))?;
    // Non-routed: declared as `component = "<name>"` in [[serve]].
    // Routed: declared as `[components.<name>]`.
    let serve_ref = format!("component = \"{name}\"");
    let section_ref = format!("[components.{name}]");
    if content.contains(&serve_ref) || content.contains(&section_ref) {
        bail!("component `{name}` is already declared in rusm.toml");
    }
    Ok(())
}

fn ensure_no_bridge_dir(root: &Path, name: &str) -> Result<()> {
    if root.join("bridges").join(name).exists() {
        bail!("bridges/{name} already exists");
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn p_args(items: &[&str]) -> pico_args::Arguments {
        pico_args::Arguments::from_vec(items.iter().map(OsString::from).collect())
    }

    fn parse(items: &[&str]) -> Result<GenerateCommand> {
        parse_generate_args(p_args(items))
    }

    fn component(items: &[&str]) -> Result<GenerateComponent> {
        match parse(items)? {
            GenerateCommand::Component(c) => Ok(c),
            _ => panic!("expected component"),
        }
    }

    fn bridge(items: &[&str]) -> Result<GenerateBridge> {
        match parse(items)? {
            GenerateCommand::Bridge(b) => Ok(b),
            _ => panic!("expected bridge"),
        }
    }

    fn auth(items: &[&str]) -> Result<GenerateAuth> {
        match parse(items)? {
            GenerateCommand::Authentication(a) => Ok(a),
            _ => panic!("expected authentication"),
        }
    }

    // ── parse_generate_args ────────────────────────────────────────────────

    #[test]
    fn component_defaults_to_ts_and_http() {
        let c = component(&["component", "chat"]).unwrap();
        assert_eq!(c.name, "chat");
        assert_eq!(c.lang, Lang::TypeScript);
        assert_eq!(c.protocol, Protocol::Http);
    }

    #[test]
    fn component_parses_explicit_lang_and_protocol() {
        let c = component(&["component", "feed", "--lang", "rust", "--protocol", "ws"]).unwrap();
        assert_eq!(c.name, "feed");
        assert_eq!(c.lang, Lang::Rust);
        assert_eq!(c.protocol, Protocol::Ws);
    }

    #[test]
    fn component_accepts_short_p_for_protocol() {
        let c = component(&["component", "api", "-p", "sse"]).unwrap();
        assert_eq!(c.protocol, Protocol::Sse);
    }

    #[test]
    fn component_accepts_go_lang() {
        let c = component(&["component", "worker", "--lang", "go"]).unwrap();
        assert_eq!(c.lang, Lang::Go);
    }

    #[test]
    fn component_rejects_generic_lang() {
        assert!(component(&["component", "api", "--lang", "generic"]).is_err());
    }

    #[test]
    fn component_flags_are_order_independent() {
        let c = component(&["component", "--lang", "rust", "api", "-p", "sse"]).unwrap();
        assert_eq!((c.lang, c.protocol), (Lang::Rust, Protocol::Sse));
    }

    #[test]
    fn component_rejects_missing_name() {
        assert!(component(&["component"]).is_err());
    }

    #[test]
    fn component_rejects_stray_argument() {
        assert!(component(&["component", "api", "extra"]).is_err());
    }

    #[test]
    fn component_rejects_invalid_name() {
        assert!(component(&["component", "-bad"]).is_err());
        assert!(component(&["component", ""]).is_err());
    }

    #[test]
    fn bridge_defaults_to_ts() {
        let b = bridge(&["bridge", "mailer"]).unwrap();
        assert_eq!(b.name, "mailer");
        assert_eq!(b.lang, Lang::TypeScript);
    }

    #[test]
    fn bridge_parses_explicit_lang() {
        let b = bridge(&["bridge", "payments", "--lang", "rust"]).unwrap();
        assert_eq!(b.lang, Lang::Rust);

        let b = bridge(&["bridge", "data", "--lang", "go"]).unwrap();
        assert_eq!(b.lang, Lang::Go);
    }

    #[test]
    fn bridge_rejects_generic_lang() {
        assert!(bridge(&["bridge", "b", "--lang", "generic"]).is_err());
    }

    #[test]
    fn bridge_rejects_missing_name() {
        assert!(bridge(&["bridge"]).is_err());
    }

    #[test]
    fn bridge_rejects_stray_argument() {
        assert!(bridge(&["bridge", "mailer", "extra"]).is_err());
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(parse(&["middleware", "name"]).is_err());
    }

    #[test]
    fn rejects_missing_subcommand() {
        assert!(parse(&[]).is_err());
    }

    // ── generate_component ─────────────────────────────────────────────────

    fn project_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rusm.toml"), minimal_toml()).unwrap();
        dir
    }

    fn minimal_toml() -> String {
        "# RUSM app config.\n\
         [[serve]]\n\
         component = \"api\"\n\
         protocol = \"http\"\n\
         listen = \"127.0.0.1:8080\"\n"
            .to_string()
    }

    #[test]
    fn component_creates_ts_http_files() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "chat".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
        };
        let created = generate_component(dir.path(), &gen).unwrap();
        let ts = dir.path().join("components/chat/index.ts");
        assert!(ts.is_file(), "index.ts created");
        for rel in &created {
            assert!(dir.path().join(rel).is_file(), "{rel:?} in created list");
        }
        // TS HTTP is zero-dep — no package.json or tsconfig
        assert!(!dir.path().join("package.json").is_file());
    }

    #[test]
    fn component_creates_rust_http_files() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "notifier".into(),
            lang: Lang::Rust,
            protocol: Protocol::Http,
        };
        generate_component(dir.path(), &gen).unwrap();

        let cargo =
            std::fs::read_to_string(dir.path().join("components/notifier/Cargo.toml")).unwrap();
        assert!(
            cargo.contains("name = \"notifier\""),
            "Cargo.toml uses component name"
        );
        assert!(cargo.contains("[workspace]"), "standalone workspace");

        let lib =
            std::fs::read_to_string(dir.path().join("components/notifier/src/lib.rs")).unwrap();
        assert!(
            lib.contains("pub mod notifier"),
            "HTTP module uses component name"
        );
        assert!(lib.contains("#[rusm_rs::handlers]"), "uses handlers macro");
        assert!(
            !lib.contains("pub mod api"),
            "no hardcoded 'api' module name"
        );
    }

    #[test]
    fn component_creates_rust_ws_files() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "conn".into(),
            lang: Lang::Rust,
            protocol: Protocol::Ws,
        };
        generate_component(dir.path(), &gen).unwrap();
        let lib = std::fs::read_to_string(dir.path().join("components/conn/src/lib.rs")).unwrap();
        assert!(lib.contains("#[rusm_rs::main]"), "WS uses main macro");
        assert!(lib.contains("ws::serve"), "WS serve call");
    }

    #[test]
    fn component_creates_go_files() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "worker".into(),
            lang: Lang::Go,
            protocol: Protocol::Http,
        };
        generate_component(dir.path(), &gen).unwrap();

        let go_mod = std::fs::read_to_string(dir.path().join("components/worker/go.mod")).unwrap();
        assert!(
            go_mod.contains("module worker"),
            "go.mod uses component name"
        );
        assert!(go_mod.contains("rusm-go"), "rusm-go dep");

        let main = std::fs::read_to_string(dir.path().join("components/worker/main.go")).unwrap();
        assert!(main.contains("web.NewHandlers()"), "HTTP shape");
    }

    #[test]
    fn component_all_combos_patch_toml_and_parse() {
        use rusm_node::NodeConfig;
        const COMBOS: &[(Lang, Protocol)] = &[
            (Lang::TypeScript, Protocol::Http),
            (Lang::TypeScript, Protocol::Sse),
            (Lang::TypeScript, Protocol::Ws),
            (Lang::Rust, Protocol::Http),
            (Lang::Rust, Protocol::Sse),
            (Lang::Rust, Protocol::Ws),
            (Lang::Go, Protocol::Http),
            (Lang::Go, Protocol::Sse),
            (Lang::Go, Protocol::Ws),
        ];
        for &(lang, protocol) in COMBOS {
            let dir = tempfile::tempdir().unwrap();
            // Start with a minimal toml that has no components so all names are fresh.
            std::fs::write(
                dir.path().join("rusm.toml"),
                "# RUSM app config.\n[node]\nlisten = \"127.0.0.1:4000\"\n",
            )
            .unwrap();
            let gen = GenerateComponent {
                name: "added".into(),
                lang,
                protocol,
            };
            generate_component(dir.path(), &gen).unwrap();
            let toml = std::fs::read_to_string(dir.path().join("rusm.toml")).unwrap();
            NodeConfig::from_toml(&toml).unwrap_or_else(|e| {
                panic!("{lang:?}/{protocol:?}: patched rusm.toml must parse: {e}")
            });
        }
    }

    #[test]
    fn component_routed_appends_components_section() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "handler".into(),
            lang: Lang::Rust,
            protocol: Protocol::Http,
        };
        generate_component(dir.path(), &gen).unwrap();
        let toml = std::fs::read_to_string(dir.path().join("rusm.toml")).unwrap();
        assert!(
            toml.contains("[components.handler]"),
            "routed: [components.handler]"
        );
        assert!(
            toml.contains("capability = \"sandboxed\""),
            "sandboxed capability"
        );
        assert!(
            !toml.contains("component = \"handler\""),
            "no serve component ref"
        );
    }

    #[test]
    fn component_non_routed_appends_serve_section() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "stream".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Sse,
        };
        generate_component(dir.path(), &gen).unwrap();
        let toml = std::fs::read_to_string(dir.path().join("rusm.toml")).unwrap();
        assert!(
            toml.contains("component = \"stream\""),
            "serve component ref"
        );
        assert!(
            toml.contains("protocol = \"sse\""),
            "protocol in serve entry"
        );
        assert!(
            !toml.contains("[components.stream]"),
            "no separate [components] section"
        );
    }

    #[test]
    fn component_errors_without_rusm_toml() {
        let dir = tempfile::tempdir().unwrap();
        let gen = GenerateComponent {
            name: "api".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
        };
        let err = generate_component(dir.path(), &gen).unwrap_err();
        assert!(err.to_string().contains("rusm.toml"), "mentions rusm.toml");
    }

    #[test]
    fn component_errors_if_dir_already_exists() {
        let dir = project_dir();
        std::fs::create_dir_all(dir.path().join("components/chat")).unwrap();
        let gen = GenerateComponent {
            name: "chat".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
        };
        let err = generate_component(dir.path(), &gen).unwrap_err();
        assert!(err.to_string().contains("already exists"), "dir conflict");
    }

    #[test]
    fn component_errors_if_component_in_toml_serve() {
        let dir = project_dir();
        // minimal_toml already has `component = "api"`
        let gen = GenerateComponent {
            name: "api".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
        };
        let err = generate_component(dir.path(), &gen).unwrap_err();
        assert!(
            err.to_string().contains("already declared"),
            "toml conflict"
        );
    }

    #[test]
    fn component_errors_if_component_in_toml_section() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("rusm.toml"),
            "[[serve]]\nprotocol = \"http\"\nlisten = \"127.0.0.1:8080\"\n\
             [components.worker]\ncapability = \"sandboxed\"\n",
        )
        .unwrap();
        let gen = GenerateComponent {
            name: "worker".into(),
            lang: Lang::Rust,
            protocol: Protocol::Http,
        };
        let err = generate_component(dir.path(), &gen).unwrap_err();
        assert!(
            err.to_string().contains("already declared"),
            "section conflict"
        );
    }

    #[test]
    fn component_ts_ws_adds_package_json_if_missing() {
        let dir = project_dir();
        assert!(!dir.path().join("package.json").is_file());
        let gen = GenerateComponent {
            name: "chat".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Ws,
        };
        let created = generate_component(dir.path(), &gen).unwrap();
        assert!(
            dir.path().join("package.json").is_file(),
            "package.json added"
        );
        assert!(
            created.iter().any(|p| p == Path::new("package.json")),
            "package.json in created list"
        );
        let pkg = std::fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert!(pkg.contains("rusm-ts"), "rusm-ts dep");
    }

    #[test]
    fn component_ts_ws_skips_package_json_if_present() {
        let dir = project_dir();
        let existing_pkg = "{\"name\":\"myapp\",\"dependencies\":{\"rusm-ts\":\"^0.4.0\"}}";
        std::fs::write(dir.path().join("package.json"), existing_pkg).unwrap();
        let gen = GenerateComponent {
            name: "chat".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Ws,
        };
        let created = generate_component(dir.path(), &gen).unwrap();
        let pkg = std::fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert_eq!(pkg, existing_pkg, "existing package.json is untouched");
        assert!(
            !created.iter().any(|p| p == Path::new("package.json")),
            "package.json not in created list"
        );
    }

    #[test]
    fn component_ts_adds_tsconfig_if_missing() {
        let dir = project_dir();
        let gen = GenerateComponent {
            name: "chat".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
        };
        let created = generate_component(dir.path(), &gen).unwrap();
        assert!(
            dir.path().join("tsconfig.json").is_file(),
            "tsconfig.json added"
        );
        assert!(
            created.iter().any(|p| p == Path::new("tsconfig.json")),
            "tsconfig.json in created list"
        );
    }

    #[test]
    fn component_ts_skips_tsconfig_if_present() {
        let dir = project_dir();
        let existing = r#"{"compilerOptions":{"strict":true}}"#;
        std::fs::write(dir.path().join("tsconfig.json"), existing).unwrap();
        let gen = GenerateComponent {
            name: "chat".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
        };
        let created = generate_component(dir.path(), &gen).unwrap();
        let tsconfig = std::fs::read_to_string(dir.path().join("tsconfig.json")).unwrap();
        assert_eq!(tsconfig, existing, "existing tsconfig.json is untouched");
        assert!(
            !created.iter().any(|p| p == Path::new("tsconfig.json")),
            "tsconfig.json not in created list"
        );
    }

    // ── generate_bridge ────────────────────────────────────────────────────

    #[test]
    fn bridge_creates_wit_and_host_ts() {
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "mailer".into(),
            lang: Lang::TypeScript,
        };
        let created = generate_bridge(dir.path(), &gen).unwrap();

        assert!(dir.path().join("bridges/mailer/bridge.wit").is_file());
        assert!(dir.path().join("bridges/mailer/host.ts").is_file());
        for rel in &created {
            assert!(dir.path().join(rel).is_file(), "{rel:?} in created list");
        }
    }

    #[test]
    fn bridge_creates_wit_and_host_rs() {
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "crypto".into(),
            lang: Lang::Rust,
        };
        generate_bridge(dir.path(), &gen).unwrap();

        assert!(dir.path().join("bridges/crypto/bridge.wit").is_file());
        assert!(dir.path().join("bridges/crypto/host.rs").is_file());
    }

    #[test]
    fn bridge_creates_wit_and_host_go() {
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "storage".into(),
            lang: Lang::Go,
        };
        generate_bridge(dir.path(), &gen).unwrap();

        assert!(dir.path().join("bridges/storage/bridge.wit").is_file());
        assert!(dir.path().join("bridges/storage/host.go").is_file());
    }

    #[test]
    fn bridge_wit_contains_package_and_interface() {
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "notify".into(),
            lang: Lang::TypeScript,
        };
        generate_bridge(dir.path(), &gen).unwrap();

        let wit = std::fs::read_to_string(dir.path().join("bridges/notify/bridge.wit")).unwrap();
        assert!(wit.contains("package app:notify@0.1.0"), "WIT package name");
        assert!(wit.contains("interface notify"), "WIT interface name");
    }

    #[test]
    fn bridge_host_rs_references_bridge_name() {
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "mailer".into(),
            lang: Lang::Rust,
        };
        generate_bridge(dir.path(), &gen).unwrap();

        let host = std::fs::read_to_string(dir.path().join("bridges/mailer/host.rs")).unwrap();
        assert!(host.contains("mailer::Host for BridgeHost"), "trait impl");
        assert!(host.contains("add_to_linker"), "linker registration");
    }

    #[test]
    fn bridge_appends_grant_comment_to_toml() {
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "mailer".into(),
            lang: Lang::TypeScript,
        };
        generate_bridge(dir.path(), &gen).unwrap();

        let toml = std::fs::read_to_string(dir.path().join("rusm.toml")).unwrap();
        assert!(toml.contains("Bridge 'mailer'"), "bridge name in comment");
        assert!(toml.contains("bridges = [\"mailer\"]"), "grant example");
    }

    #[test]
    fn bridge_toml_still_parses_after_patching() {
        use rusm_node::NodeConfig;
        let dir = project_dir();
        let gen = GenerateBridge {
            name: "notify".into(),
            lang: Lang::TypeScript,
        };
        generate_bridge(dir.path(), &gen).unwrap();

        let toml = std::fs::read_to_string(dir.path().join("rusm.toml")).unwrap();
        NodeConfig::from_toml(&toml).expect("rusm.toml must still parse after bridge patch");
    }

    #[test]
    fn bridge_errors_without_rusm_toml() {
        let dir = tempfile::tempdir().unwrap();
        let gen = GenerateBridge {
            name: "mailer".into(),
            lang: Lang::TypeScript,
        };
        let err = generate_bridge(dir.path(), &gen).unwrap_err();
        assert!(err.to_string().contains("rusm.toml"), "mentions rusm.toml");
    }

    #[test]
    fn bridge_errors_if_dir_already_exists() {
        let dir = project_dir();
        std::fs::create_dir_all(dir.path().join("bridges/mailer")).unwrap();
        let gen = GenerateBridge {
            name: "mailer".into(),
            lang: Lang::TypeScript,
        };
        let err = generate_bridge(dir.path(), &gen).unwrap_err();
        assert!(err.to_string().contains("already exists"), "dir conflict");
    }

    // ── generate authentication ────────────────────────────────────────────

    #[test]
    fn auth_defaults_to_ts() {
        let a = auth(&["authentication", "jwt"]).unwrap();
        assert_eq!(a.name, "jwt");
        assert_eq!(a.lang, Lang::TypeScript);
    }

    #[test]
    fn auth_alias_and_explicit_langs_parse() {
        assert_eq!(auth(&["auth", "jwt"]).unwrap().name, "jwt"); // `auth` alias
        assert_eq!(
            auth(&["authentication", "j", "--lang", "rust"])
                .unwrap()
                .lang,
            Lang::Rust
        );
        assert_eq!(
            auth(&["authentication", "j", "--lang", "go"]).unwrap().lang,
            Lang::Go
        );
    }

    #[test]
    fn auth_rejects_generic_missing_and_stray() {
        assert!(auth(&["authentication", "j", "--lang", "generic"]).is_err());
        assert!(auth(&["authentication"]).is_err());
        assert!(auth(&["authentication", "j", "extra"]).is_err());
    }

    #[test]
    fn auth_creates_the_host_file_per_language() {
        for (lang, file) in [
            (Lang::Rust, "host.rs"),
            (Lang::TypeScript, "host.ts"),
            (Lang::Go, "host.go"),
        ] {
            let dir = project_dir();
            let gen = GenerateAuth {
                name: "jwt".into(),
                lang,
            };
            let created = generate_authentication(dir.path(), &gen).unwrap();
            let path = dir.path().join("auth/jwt").join(file);
            assert!(path.is_file(), "{lang:?}: {file} created");
            assert_eq!(created, vec![PathBuf::from("auth/jwt").join(file)]);
        }
    }

    #[test]
    fn auth_host_files_expose_authenticate() {
        let dir = project_dir();
        generate_authentication(
            dir.path(),
            &GenerateAuth {
                name: "jwt".into(),
                lang: Lang::Rust,
            },
        )
        .unwrap();
        let rs = std::fs::read_to_string(dir.path().join("auth/jwt/host.rs")).unwrap();
        assert!(rs.contains("pub async fn authenticate(req: AuthRequest) -> AuthVerdict"));
        assert!(rs.contains("AuthVerdict::Deny"), "fail-closed default");

        let dir = project_dir();
        generate_authentication(
            dir.path(),
            &GenerateAuth {
                name: "jwt".into(),
                lang: Lang::Go,
            },
        )
        .unwrap();
        let go = std::fs::read_to_string(dir.path().join("auth/jwt/host.go")).unwrap();
        assert!(go.contains("func Authenticate(req rusm.AuthRequest) rusm.AuthVerdict"));
        assert!(go.contains("rusm.Deny()"));

        let dir = project_dir();
        generate_authentication(
            dir.path(),
            &GenerateAuth {
                name: "jwt".into(),
                lang: Lang::TypeScript,
            },
        )
        .unwrap();
        let ts = std::fs::read_to_string(dir.path().join("auth/jwt/host.ts")).unwrap();
        assert!(ts.contains("export async function authenticate(req)"));
        assert!(ts.contains("deny: true"));
    }

    #[test]
    fn auth_appends_hint_and_toml_still_parses() {
        use rusm_node::NodeConfig;
        let dir = project_dir();
        generate_authentication(
            dir.path(),
            &GenerateAuth {
                name: "jwt".into(),
                lang: Lang::Rust,
            },
        )
        .unwrap();
        let toml = std::fs::read_to_string(dir.path().join("rusm.toml")).unwrap();
        assert!(
            toml.contains("authentication = \"jwt\""),
            "wiring hint present"
        );
        NodeConfig::from_toml(&toml).expect("rusm.toml must still parse after auth patch");
    }

    #[test]
    fn auth_errors_without_rusm_toml_and_on_existing_dir() {
        let bare = tempfile::tempdir().unwrap();
        let err = generate_authentication(
            bare.path(),
            &GenerateAuth {
                name: "jwt".into(),
                lang: Lang::Rust,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("rusm.toml"));

        let dir = project_dir();
        std::fs::create_dir_all(dir.path().join("auth/jwt")).unwrap();
        let err = generate_authentication(
            dir.path(),
            &GenerateAuth {
                name: "jwt".into(),
                lang: Lang::Rust,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }
}
