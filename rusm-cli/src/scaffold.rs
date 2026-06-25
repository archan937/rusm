//! `rusm new <name> [--rust|--lang ts|rust|go|generic] [--protocol http|sse|ws]` —
//! scaffold a new RUSM app.
//!
//! Produces a project whose component source is **pure developer logic** — no
//! `wit-bindgen`/`export!` boilerplate (Rust hides it behind `#[rusm_rs::main]`, Go
//! behind the `rusm-go` SDK) and no `Process`/frame plumbing (TS uses web standards and
//! the `rusm-ts` package). Pick a language (`--rust`/`--lang`, default TypeScript; `go`
//! for TinyGo; `generic` for a pre-built wasm you supply yourself) and a
//! protocol (`--protocol`, default `http`); from nothing to a live server in three
//! commands:
//!
//! ```text
//! rusm new hello && cd hello
//! rusm build      # components/<name>/ -> wasm/<name>.{js,wasm}
//! rusm serve      # hosts it on http://127.0.0.1:8080
//! ```

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::template::{self, parse_template, Template};

/// The guest language for the scaffolded component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    TypeScript,
    Rust,
    Go,
    /// Generic: no source files scaffolded; user provides a pre-built .wasm file.
    Generic,
}

/// The protocol the component is served over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Http,
    Sse,
    Ws,
}

impl Protocol {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Sse => "sse",
            Protocol::Ws => "ws",
        }
    }
}

/// A parsed `rusm new` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewApp {
    pub name: String,
    pub lang: Lang,
    pub protocol: Protocol,
    /// `Some` for `--template <name>`: scaffold a full example app instead of the minimal
    /// single-component starter. The `protocol` field is unused in that case (a template
    /// brings its own listeners).
    pub template: Option<Template>,
    /// `--bridges`: scaffold a **custom-bridge** app — a host crate (which registers the
    /// app's native bridges, then serves) plus an example `weather` bridge and a guest that
    /// calls it. The host impl is always Rust (the host *is* Rust); `lang` is the **guest**
    /// language — TypeScript, Rust, or Go. (The `weather` template is the same app.)
    pub bridges: bool,
}

/// Parse the arguments following `rusm new` into a [`NewApp`]: a single positional
/// name plus optional `--rust`/`--lang <ts|rust|generic>` and `--protocol <http|sse|ws>`
/// (`-p`, `--protocol=…` also accepted). Unknown flags, bad values, and a missing or
/// duplicate name are hard errors — a typo never silently scaffolds the wrong thing.
pub fn parse_new_args(mut args: pico_args::Arguments) -> Result<NewApp> {
    // Options first — pico-args consumes named options before free arguments. `--lang`
    // takes precedence over the `--rust` shorthand; `-p` is an alias for `--protocol`.
    let rust = args.contains("--rust");
    let bridges = args.contains("--bridges");
    let lang = args.opt_value_from_str::<_, String>("--lang")?;
    let protocol_arg = match args.opt_value_from_str::<_, String>("--protocol")? {
        Some(value) => Some(value),
        None => args.opt_value_from_str("-p")?,
    };
    let template_arg = args.opt_value_from_str::<_, String>("--template")?;

    // Then the one positional: the app name. A missing name is the usage error.
    let name: String = args.free_from_str().map_err(|_| {
        anyhow!(
            "usage: rusm new <name> [--rust] [--lang ts|rust|go|generic] \
             [--protocol http|sse|ws] [--template todo-board|weather|mailer] [--bridges]"
        )
    })?;
    validate_name(&name)?;

    // Anything left after one name and the known options is a stray argument (an
    // unknown flag or a second name) — a typo never silently scaffolds the wrong thing.
    if let Some(extra) = args.finish().first() {
        bail!(
            "unexpected argument `{}` (the app name is already `{name}`)",
            extra.to_string_lossy()
        );
    }

    let lang = match lang {
        Some(value) => parse_lang(&value)?,
        None if rust => Lang::Rust,
        // A custom-bridge guest defaults to Rust (its host is Rust anyway, the simplest
        // end-to-end); any guest language works. A plain app defaults to TypeScript.
        None if bridges => Lang::Rust,
        None => Lang::TypeScript,
    };
    let protocol = match &protocol_arg {
        Some(value) => parse_protocol(value)?,
        None => Protocol::Http,
    };

    // A template scaffolds a full example app, so it owns its own listeners + language
    // surface: `--protocol` is meaningless and `generic` (no source) has nothing to fill.
    let template = match template_arg {
        Some(value) => {
            let template = parse_template(&value)?;
            if matches!(lang, Lang::Generic) {
                bail!("`--template` needs a guest language — use `--lang ts|rust|go`");
            }
            if protocol_arg.is_some() {
                bail!(
                    "`--protocol` can't be combined with `--template` \
                       (the todo board serves http, sse, and ws)"
                );
            }
            Some(template)
        }
        None => None,
    };

    // A bridge app comes three ways: the `--bridges` flag (weather example, Rust host), the
    // `weather` template (same), or the `mailer` template (TS host, generated host binary).
    // All three require a real guest language (TS/Rust/Go — not `generic`, which has no guest
    // source). TS guests can call bridges (the per-app js-runner is rebuilt with them compiled
    // in), so all three languages are accepted for every bridge template.
    if bridges && template.is_some() {
        bail!(
            "`--bridges` can't be combined with `--template` — use `--template weather` or \
             `--template mailer` for a bridge example, or `--bridges` to start a new bridge app"
        );
    }
    let bridge_app =
        bridges || matches!(template, Some(Template::Weather) | Some(Template::Mailer));
    if bridge_app && matches!(lang, Lang::Generic) {
        bail!(
            "a custom-bridge app needs a guest language to call the bridge — \
             use `--lang ts`, `rust`, or `go`"
        );
    }

    Ok(NewApp {
        name,
        lang,
        protocol,
        template,
        bridges,
    })
}

pub(crate) fn parse_lang(value: &str) -> Result<Lang> {
    match value {
        "ts" | "typescript" => Ok(Lang::TypeScript),
        "rust" | "rs" => Ok(Lang::Rust),
        "go" | "golang" => Ok(Lang::Go),
        "generic" | "wasm" => Ok(Lang::Generic),
        other => bail!("unknown language `{other}` — use `ts`, `rust`, `go`, or `generic`"),
    }
}

pub(crate) fn parse_protocol(value: &str) -> Result<Protocol> {
    match value {
        "http" => Ok(Protocol::Http),
        "sse" => Ok(Protocol::Sse),
        "ws" => Ok(Protocol::Ws),
        other => bail!("unknown protocol `{other}` — use `http`, `sse`, or `ws`"),
    }
}

/// Scaffolds the app at `<root>/<name>`, returning the files created (relative to the
/// project). Fails if the target directory exists and is non-empty, so an existing
/// project is never clobbered.
pub fn scaffold(root: &Path, app: &NewApp) -> Result<Vec<PathBuf>> {
    let project = root.join(&app.name);
    if project.exists()
        && project
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        bail!("`{}` already exists and is not empty", app.name);
    }

    let mut created = Vec::new();
    for (rel, contents) in files(app) {
        let path = project.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
        created.push(rel);
    }
    Ok(created)
}

/// The full set of (relative path, contents) for an app — the single place that maps
/// a (language, protocol) to its files.
fn files(app: &NewApp) -> Vec<(PathBuf, String)> {
    // The `weather` template IS the custom-bridge example (host crate + bridge + a guest).
    if app.template == Some(Template::Weather) {
        return bridge_files(app);
    }
    // The `mailer` template is a TS-hosted bridge that calls the Resend API.
    if app.template == Some(Template::Mailer) {
        return mailer_bridge_files(app);
    }
    // The todo-board template scaffolds the full multi-protocol app (the real `examples/<lang>`).
    if app.template.is_some() {
        return template::files(app.lang, &app.name);
    }
    // `--bridges` adds a custom bridge to a new app — the same scaffold as `--template weather`.
    if app.bridges {
        return bridge_files(app);
    }
    let mut out = vec![
        (PathBuf::from("rusm.toml"), rusm_toml(app)),
        (PathBuf::from(".gitignore"), GITIGNORE.to_string()),
        (PathBuf::from("README.md"), readme(app)),
    ];
    match app.lang {
        Lang::TypeScript => {
            out.push((
                PathBuf::from("components/api/index.ts"),
                ts_component(app.protocol).to_string(),
            ));
            out.push((PathBuf::from("tsconfig.json"), TSCONFIG.to_string()));
            // Only a `rusm`-importing component needs a manifest + install; HTTP/SSE
            // are zero-dependency web-standard handlers.
            if app.protocol == Protocol::Ws {
                out.push((PathBuf::from("package.json"), package_json(&app.name)));
            }
        }
        Lang::Rust => {
            out.push((
                PathBuf::from("components/api/Cargo.toml"),
                cargo_toml("api"),
            ));
            out.push((
                PathBuf::from("components/api/src/lib.rs"),
                rust_component(app.protocol, "api"),
            ));
        }
        Lang::Go => {
            out.push((PathBuf::from("components/api/go.mod"), go_mod("api")));
            out.push((
                PathBuf::from("components/api/main.go"),
                go_component(app.protocol).to_string(),
            ));
        }
        Lang::Generic => {
            // No source is generated — the user drops in a pre-built `.wasm`. A README
            // (not an empty `.gitkeep`) documents the interface RUSM expects.
            out.push((
                PathBuf::from("components/api/README.md"),
                GENERIC_COMPONENT_README.to_string(),
            ));
        }
    }
    out
}

/// Only a Rust or Go **HTTP** app uses the named-action model — Rust's
/// `#[rusm_rs::handlers]` / Go's `web.Handlers` — reached through a `[serve.routes]` table
/// to a `[components.<name>]` handler. Everything else is a single named handler component
/// with no routes: SSE and WebSocket are per-connection (`sse::serve` / `web.Sse`,
/// `ws::serve` / `web.WebSocket`), and TS HTTP is a `wasi:http` `export default`.
fn has_routes(app: &NewApp) -> bool {
    matches!(app.lang, Lang::Rust | Lang::Go) && app.protocol == Protocol::Http
}

/// Wasmtime version the scaffolded host crate pins — it **must** match what `rusm-wasm`
/// links, because the `rusm build`-generated `src/bindings.rs` `bindgen!` emits `wasmtime::`
/// paths whose types must be identical to the runtime's. `bridge_wasmtime_pin_tracks_rusm_wasm`
/// guards it against drift.
const WASMTIME_VERSION: &str = "45.0.1";

// The host-side custom-bridge files are the live `examples/weather-api/rust` (the single source,
// proven end to end), **vendored** into `templates/weather/` so they ship in the published
// tarball (the workspace `../../examples/` isn't packaged) — `make sync-templates` keeps them
// byte-identical to the example, guarded by `vendored_weather_template_matches_sources`. A
// scaffolded app is exactly the example: the host `main.rs`, the bridge's contract + impl, and
// the guest (all three languages share the template; Go uses `api/internal` directly).
const BRIDGE_HOST_MAIN: &str = include_str!("../templates/weather/host-main.rs");
const BRIDGE_WIT: &str = include_str!("../templates/weather/bridge.wit");
const BRIDGE_HOST_IMPL: &str = include_str!("../templates/weather/host.rs");
const BRIDGE_RUST_GUEST: &str = include_str!("../templates/weather/rust-guest.rs");
// The TypeScript guest — a per-connection WebSocket handler calling the `weather` bridge.
// Verbatim from the live example; its `/// <reference path="../../bridges.d.ts" />` resolves
// the same from `components/api/` (the scaffold) as from `components/tsweather/` (the example).
const BRIDGE_TS_GUEST: &str = include_str!("../templates/weather/ts-guest.ts");

// Mailer bridge template files — vendored from `examples/mailer/` into `templates/mailer/`
// so they ship in the published `rusm-cli` tarball. `make sync-templates` keeps them
// byte-identical to the example; drift is caught by `vendored_mailer_template_matches_sources`.
const MAILER_BRIDGE_WIT: &str = include_str!("../templates/mailer/bridge.wit");
const MAILER_HOST_TS: &str = include_str!("../templates/mailer/host.ts");
const MAILER_RUST_GUEST: &str = include_str!("../templates/mailer/rust-guest.rs");
const MAILER_TS_GUEST: &str = include_str!("../templates/mailer/ts-guest.ts");
const MAILER_GO_GUEST: &str = include_str!("../templates/mailer/go-guest.go");

/// `.gitignore` for a custom-bridge app: build output plus the `rusm build`-generated bridge
/// glue (the host crate's `wit/` + `src/{bindings,bridges}.rs`, and each guest's `wit/` +
/// `internal/`) — all regenerated from `bridges/` every build, so source, not artifacts.
const BRIDGE_GITIGNORE: &str = "\
/wasm/
/target/
# `rusm build`-generated bridge glue (regenerated from bridges/ each build).
/wit/
/src/bindings.rs
/src/bridges.rs
/bridges.d.ts
/node_modules/
/components/*/wit/
/components/*/internal/
";

/// The files for a **custom-bridge** app (`rusm new <name> --bridges`): a host crate that
/// registers the app's bridges then serves, an example `weather` bridge, and a guest (Rust or
/// Go) that calls it. Host-side files come verbatim from `examples/weather-api`; only the
/// dependency manifests, `rusm.toml`, `.gitignore`, and README are generated (version deps +
/// the chosen guest language). The generated glue is `rusm build` output (git-ignored).
fn bridge_files(app: &NewApp) -> Vec<(PathBuf, String)> {
    let mut out = vec![
        (PathBuf::from("Cargo.toml"), host_cargo_toml(&app.name)),
        (PathBuf::from("src/main.rs"), BRIDGE_HOST_MAIN.to_string()),
        (
            PathBuf::from("bridges/weather/bridge.wit"),
            BRIDGE_WIT.to_string(),
        ),
        (
            PathBuf::from("bridges/weather/host.rs"),
            BRIDGE_HOST_IMPL.to_string(),
        ),
        (PathBuf::from("rusm.toml"), bridge_rusm_toml(app.lang)),
        (PathBuf::from(".gitignore"), BRIDGE_GITIGNORE.to_string()),
        (PathBuf::from("README.md"), bridge_readme(app)),
    ];
    match app.lang {
        Lang::TypeScript => {
            // The TS guest is a per-connection WebSocket handler; `rusm build` rebuilds the
            // js-runner with the bridge compiled in and generates the typed `bridges.d.ts`.
            out.push((
                PathBuf::from("components/api/index.ts"),
                BRIDGE_TS_GUEST.to_string(),
            ));
            out.push((PathBuf::from("package.json"), package_json(&app.name)));
            out.push((PathBuf::from("tsconfig.json"), TSCONFIG.to_string()));
        }
        Lang::Go => {
            out.push((PathBuf::from("components/api/go.mod"), go_mod("api")));
            out.push((PathBuf::from("components/api/main.go"), go_bridge_guest()));
        }
        // Rust is the default; `Generic` is rejected in `parse_new_args`.
        _ => {
            out.push((
                PathBuf::from("components/api/Cargo.toml"),
                cargo_toml("api"),
            ));
            out.push((
                PathBuf::from("components/api/src/lib.rs"),
                BRIDGE_RUST_GUEST.to_string(),
            ));
        }
    }
    out
}

/// Files for a **mailer** bridge app (`rusm new <name> --template mailer`): a TS-hosted bridge
/// that calls the Resend API, registered as a resident actor by a generated host binary. The
/// developer authors `bridges/mailer/host.ts` and `components/api/`; `rusm build` generates
/// the Rust delegation shim, the TS runner, and the host binary entry point.
fn mailer_bridge_files(app: &NewApp) -> Vec<(PathBuf, String)> {
    let mut out = vec![
        (
            PathBuf::from("bridges/mailer/bridge.wit"),
            MAILER_BRIDGE_WIT.to_string(),
        ),
        (
            PathBuf::from("bridges/mailer/host.ts"),
            MAILER_HOST_TS.to_string(),
        ),
        (PathBuf::from("rusm.toml"), mailer_rusm_toml()),
        (PathBuf::from(".gitignore"), BRIDGE_GITIGNORE.to_string()),
        (PathBuf::from("package.json"), package_json(&app.name)),
        (PathBuf::from("tsconfig.json"), TSCONFIG.to_string()),
        (PathBuf::from("README.md"), mailer_readme(app)),
    ];
    match app.lang {
        Lang::TypeScript => {
            out.push((
                PathBuf::from("components/api/index.ts"),
                MAILER_TS_GUEST.to_string(),
            ));
        }
        Lang::Go => {
            out.push((PathBuf::from("components/api/go.mod"), go_mod("api")));
            out.push((
                PathBuf::from("components/api/main.go"),
                mailer_go_guest(app),
            ));
        }
        _ => {
            out.push((
                PathBuf::from("components/api/Cargo.toml"),
                cargo_toml("api"),
            ));
            out.push((
                PathBuf::from("components/api/src/lib.rs"),
                MAILER_RUST_GUEST.to_string(),
            ));
        }
    }
    out
}

/// The custom-bridge **host crate** `Cargo.toml`: the small Rust binary that registers the
/// app's bridges and serves via `rusm_cli::host`. Depends on the runtime crates by version
/// (like every scaffolded component) plus the exact-pinned `wasmtime` the generated bindings
/// need. Detached from any parent workspace (`[workspace]`).
fn host_cargo_toml(name: &str) -> String {
    format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [[bin]]\n\
         name = \"{name}\"\n\
         path = \"src/main.rs\"\n\
         \n\
         [dependencies]\n\
         rusm-cli = \"{SDK_VERSION}\"\n\
         rusm-wasm = \"{SDK_VERSION}\"\n\
         rusm-node = \"{SDK_VERSION}\"\n\
         wasmtime = \"{WASMTIME_VERSION}\"\n\
         anyhow = \"1\"\n\
         tokio = {{ version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"signal\"] }}\n\
         \n\
         [workspace]\n"
    )
}

/// The `rusm.toml` for a custom-bridge app: one HTTP listener routing to an `api` handler
/// under the `forecaster` profile, which grants the `weather` bridge (default-deny, by name).
/// Identical for a Rust or Go guest — the component is `api` either way.
fn bridge_rusm_toml(lang: Lang) -> String {
    // The `weather` bridge grant is the same for every guest (default-deny, by name). Only the
    // listener shape differs: a Rust/Go guest is a routed HTTP handler; the TS guest is a
    // per-connection WebSocket handler (`websocket({ message })`).
    let grant =
        "# The handler may import the custom `weather` bridge — default-deny, granted by name.\n\
                 [capabilities.forecaster]\n\
                 inherits = \"sandboxed\"\n\
                 bridges = [\"weather\"]\n";
    let listener = if matches!(lang, Lang::TypeScript) {
        "[[serve]]\n\
         component = \"api\"\n\
         protocol = \"ws\"\n\
         listen = \"127.0.0.1:8080\"\n"
    } else {
        "[[serve]]\n\
         protocol = \"http\"\n\
         listen = \"127.0.0.1:8080\"\n\
         \n\
         [serve.routes]\n\
         \"GET /forecast/:city\" = \"api#forecast\"\n"
    };
    format!(
        "{TOML_HEADER}\
         [node]\n\
         listen = \"127.0.0.1:8080\"\n\
         \n\
         {grant}\
         \n\
         {listener}\
         \n\
         [components.api]\n\
         capability = \"forecaster\"\n"
    )
}

/// The Go guest — verbatim from the template (uses `api/internal`, the canonical module name
/// `go_mod()` emits; the logic is identical to the weather-api/go example).
fn go_bridge_guest() -> String {
    include_str!("../templates/weather/go-guest.go").to_string()
}

/// The `rusm.toml` for a mailer bridge app: an HTTP POST /send listener routing to `api`
/// under the `notifier` profile, which grants the `mailer` bridge (default-deny, by name).
fn mailer_rusm_toml() -> String {
    format!(
        "{TOML_HEADER}\
         [node]\n\
         listen = \"127.0.0.1:8080\"\n\
         \n\
         # The handler may import the `mailer` bridge — default-deny, granted by name.\n\
         # Set RESEND_API_KEY in your environment or .env before serving.\n\
         [capabilities.notifier]\n\
         inherits = \"sandboxed\"\n\
         bridges = [\"mailer\"]\n\
         \n\
         [[serve]]\n\
         protocol = \"http\"\n\
         listen = \"127.0.0.1:8080\"\n\
         \n\
         [serve.routes]\n\
         \"POST /send\" = \"api#post\"\n\
         \n\
         [components.api]\n\
         capability = \"notifier\"\n"
    )
}

/// The Go guest for a mailer bridge app — calls `smtp.Send()` from the bridge-generated binding.
/// The template already uses the `api` module name (matching `go_mod()`'s `module api`), so
/// no retargeting is needed here — unlike the weather Go guest which starts from `go-api`.
fn mailer_go_guest(_app: &NewApp) -> String {
    MAILER_GO_GUEST.to_string()
}

const TOML_HEADER: &str =
    "# RUSM app config. `rusm serve` hosts each [[serve]] listener; `rusm build` compiles\n\
     # components/<name>/ into wasm/ first. See https://github.com/archan937/rusm.\n\n";

fn rusm_toml(app: &NewApp) -> String {
    let proto = app.protocol.as_str();
    if has_routes(app) {
        // Routed: a pure listener whose `[serve.routes]` dispatch to a `[components.api]`
        // handler (which carries its own capability profile).
        format!(
            "{TOML_HEADER}\
             [[serve]]\n\
             protocol = \"{proto}\"           # http (routed)\n\
             listen = \"127.0.0.1:8080\"\n\n\
             [serve.routes]\n\
             \"GET /\" = \"api#home\"        # METHOD /path = component#action\n\n\
             [components.api]              # the handler, built from components/api\n\
             capability = \"sandboxed\"    # default-deny; see [capabilities.<name>] for more\n"
        )
    } else {
        // A single named handler component (TS `export default`, a WebSocket worker, or a generic wasm).
        let artifact = match app.lang {
            Lang::TypeScript => "wasm/api.js",
            Lang::Rust | Lang::Go | Lang::Generic => "wasm/api.wasm",
        };
        format!(
            "{TOML_HEADER}\
             [[serve]]\n\
             component = \"api\"           # loads {artifact}, built from components/api\n\
             protocol = \"{proto}\"        # http | sse | ws\n\
             listen = \"127.0.0.1:8080\"\n"
        )
    }
}

/// Build output and installed dependencies are not source.
const GITIGNORE: &str = "/wasm/\n/node_modules/\n/target/\n";

/// One tsconfig for any TS component: web-standard `Request`/`Response`/streams come
/// from the DOM lib, and bundler resolution finds the `rusm-ts` package (WS).
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

pub(crate) fn package_json(name: &str) -> String {
    format!(
        "{{\n  \"name\": \"{name}\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"dependencies\": {{\n    \"rusm-ts\": \"^{SDK_VERSION}\"\n  }}\n}}\n"
    )
}

/// The placeholder README scaffolded for a generic (bring-your-own-wasm) component.
const GENERIC_COMPONENT_README: &str = "\
# Generic WASM component

Drop a pre-built **wasip2** component here as `api.wasm` — `rusm build` copies it
into `wasm/`, then `rusm serve` / `rusm run` hosts it. No source is generated: this
is the bring-your-own-component path for any toolchain (`cargo component`, `wstd`,
TinyGo, …). If you ship more than one `.wasm`, name the one to use `api.wasm`.

The interface RUSM expects depends on how `rusm.toml` hosts it:

- **HTTP / SSE** (`[[serve]] protocol = \"http\"|\"sse\"`): a standard `wasi:http`
  component exporting `wasi:http/incoming-handler` — the same contract a TypeScript
  `export default { fetch }` compiles to. One fresh instance per request.
- **WebSocket** (`protocol = \"ws\"`): a `rusm:runtime` actor component, one process
  per connection.
- **CLI command** (`rusm run`): a standard command component exporting `wasi:cli/run`.

To dispatch by route instead, add a `[serve.routes]` table mapping
`\"METHOD /path\" = \"api#action\"` — your component then implements those actions.

```sh
rusm build      # copies api.wasm -> wasm/api.wasm
rusm serve      # http://127.0.0.1:8080
```

See https://github.com/archan937/rusm for more.
";

/// The published SDK version a scaffolded app depends on — the next release. **One
/// source of truth** for every scaffold/template dependency reference (`rusm-rs`,
/// `rusm-ts`, `rusm-go`), so a version bump is a single edit and can't drift again. The
/// actual crate/package versions are bumped to match when that release is cut.
pub(crate) const SDK_VERSION: &str = "0.5.0";

/// The Rust component crate — one `cdylib`, the `rusm-rs` guest crate, and
/// `wit-bindgen` (which `#[rusm_rs::main]` drives so the source carries no `wit/`).
pub(crate) fn cargo_toml(name: &str) -> String {
    format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [lib]\n\
         crate-type = [\"cdylib\"]\n\
         \n\
         [dependencies]\n\
         rusm-rs = \"{SDK_VERSION}\"\n\
         wit-bindgen = \"0.46\"\n\
         \n\
         [profile.release]\n\
         opt-level = \"z\"\n\
         strip = true\n\
         \n\
         [workspace]\n"
    )
}

pub(crate) fn ts_component(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Http => {
            "\
// A RUSM HTTP component: export a default handler. `rusm serve` runs it with one
// sandboxed WASM instance per request — you write the handler, RUSM owns the rest.
export default function handle(request: Request): Response {
  const url = new URL(request.url);
  return new Response(`Hello from RUSM \u{1F44B}  (you asked for ${url.pathname})\\n`, {
    headers: { \"content-type\": \"text/plain; charset=utf-8\" },
  });
}
"
        }
        Protocol::Sse => {
            "\
// A RUSM SSE component: return a streaming `text/event-stream` Response. Each chunk
// is one Server-Sent Event; close the controller to end the stream.
export default function handle(_request: Request): Response {
  const encoder = new TextEncoder();
  let n = 0;
  const body = new ReadableStream({
    pull(controller) {
      if (n >= 5) return controller.close();
      controller.enqueue(encoder.encode(`data: tick ${n++}\\n\\n`));
    },
  });
  return new Response(body, { headers: { \"content-type\": \"text/event-stream; charset=utf-8\" } });
}
"
        }
        Protocol::Ws => {
            "\
// A RUSM WebSocket component: one instance serves every connection. Reply with
// `socket.send(...)`; keep shared state (rooms, presence) in the handler's closure.
import { websocket } from \"rusm-ts\";

export default websocket({
  open(socket) {
    socket.send(\"welcome to RUSM\\n\");
  },
  message(socket, data) {
    socket.send(data); // echo the frame back to the sender
  },
});
"
        }
    }
}

/// The Go component module: the rusm-go guest SDK (its `web` subpackage provides the
/// HTTP/SSE/WebSocket handler surface). TinyGo + wit-bindgen-go are driven by `rusm
/// build`, so the source carries no bindings boilerplate and no `wit/` dir.
pub(crate) fn go_mod(name: &str) -> String {
    format!(
        "module {name}\n\
         \n\
         go 1.24\n\
         \n\
         require github.com/archan937/rusm/packages/rusm-go v{SDK_VERSION}\n"
    )
}

pub(crate) fn go_component(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Http => {
            "\
// A RUSM HTTP component in Go: register handler actions and Serve. rusm.toml's
// [serve.routes] maps `METHOD /path` to `api#<action>`; the host spawns a fresh
// sandboxed instance per request and dispatches the matched action here — no main,
// no router, no request/reply plumbing, just normal Go.
package main

import (
	rusm \"github.com/archan937/rusm/packages/rusm-go\"
	\"github.com/archan937/rusm/packages/rusm-go/web\"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	h.Handle(\"home\", func(req web.Request, _ web.Params) web.Response {
		return web.Text(\"Hello from RUSM \u{1F44B}  (you asked for \" + req.URL + \")\\n\")
	})
	h.Serve()
}
"
        }
        Protocol::Sse => {
            "\
// A RUSM SSE component in Go: a per-connection handler (like WebSocket). The host runs
// one instance per connection; Open emits initial events and Message emits each event
// pushed to this process's mailbox (typically via a process-group tag subscribed to in
// Open). Keep shared state in a resident [components.<name>] service or kv.
package main

import (
	\"fmt\"

	rusm \"github.com/archan937/rusm/packages/rusm-go\"
	\"github.com/archan937/rusm/packages/rusm-go/web\"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.Sse{
		Open: func(s web.Stream) {
			for n := 0; n < 5; n++ {
				s.Data([]byte(fmt.Sprintf(\"tick %d\", n)))
			}
		},
		Message: func(s web.Stream, event []byte) {
			s.Data(event) // a pushed event → emit it
		},
	}.Serve()
}
"
        }
        Protocol::Ws => {
            "\
// A RUSM WebSocket component in Go: the host runs one instance **per connection**, so
// the handler is naturally isolated. Reply with conn.Send(...); keep shared state in a
// resident [components.<name>] service or kv (not in this per-connection process).
package main

import (
	rusm \"github.com/archan937/rusm/packages/rusm-go\"
	\"github.com/archan937/rusm/packages/rusm-go/web\"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.WebSocket{
		Open: func(c web.Conn) {
			c.Send([]byte(\"welcome to RUSM\\n\"))
		},
		Message: func(c web.Conn, data []byte) {
			c.Send(data) // echo the frame back to the sender
		},
	}.Serve()
}
"
        }
    }
}

pub(crate) fn rust_component(protocol: Protocol, name: &str) -> String {
    match protocol {
        Protocol::Http => format!(
            "//! A RUSM HTTP component: a module of named handler **actions**. The `[routes]` table\n\
             //! in rusm.toml maps `METHOD /path` to `{name}#<action>`; the host spawns a fresh\n\
             //! sandboxed instance per request and dispatches the matched action here — no `main`,\n\
             //! no router, no request/reply plumbing.\n\
             use rusm_rs::http::{{Params, Request, Response}};\n\
             \n\
             #[rusm_rs::handlers]\n\
             pub mod {name} {{\n\
             \u{20}\u{20}\u{20}\u{20}use super::*;\n\
             \n\
             \u{20}\u{20}\u{20}\u{20}pub fn home(request: Request, _params: Params) -> Response {{\n\
             \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}let url = request.url;\n\
             \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}Response::text(format!(\"Hello from RUSM \u{1F44B}  (you asked for {{url}})\\n\"))\n\
             \u{20}\u{20}\u{20}\u{20}}}\n\
             }}\n"
        ),
        Protocol::Sse => "\
//! A RUSM SSE component: a per-connection handler (like WebSocket). The host runs one
//! instance per connection; `open` emits initial events and `message` emits each event
//! pushed to this process's mailbox (typically via a process-group tag subscribed to in
//! `open`). Keep shared state in a `[components.<name>]` service or `kv`.
use rusm_rs::sse::{self, Handler, Stream};

#[derive(Default)]
struct Api;

impl Handler for Api {
    fn open(&mut self, stream: &Stream) {
        for n in 0..5 {
            stream.data(format!(\"tick {n}\").as_bytes());
        }
    }
    fn message(&mut self, stream: &Stream, event: Vec<u8>) {
        stream.data(&event); // a pushed event → emit it
    }
}

#[rusm_rs::main]
fn main() {
    sse::serve(Api::default());
}
"
        .to_string(),
        Protocol::Ws => "\
//! A RUSM WebSocket component: the host runs one instance **per connection**, so the
//! handler is naturally isolated. Reply with `conn.send(...)`; keep shared state in a
//! `[components.<name>]` service or `kv` (not in this per-connection process).
use rusm_rs::ws::{self, Connection, Handler};

#[derive(Default)]
struct Api;

impl Handler for Api {
    fn open(&mut self, conn: &Connection) {
        conn.send(b\"welcome to RUSM\\n\");
    }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        conn.send(&data); // echo the frame back to the sender
    }
}

#[rusm_rs::main]
fn main() {
    ws::serve(Api::default());
}
"
        .to_string(),
    }
}

fn readme(app: &NewApp) -> String {
    let name = &app.name;
    let lang = match app.lang {
        Lang::TypeScript => "TypeScript",
        Lang::Rust => "Rust",
        Lang::Go => "Go",
        Lang::Generic => "Generic (pre-built wasm)",
    };
    let source = match app.lang {
        Lang::TypeScript => "components/api/index.ts",
        Lang::Rust => "components/api/src/lib.rs",
        Lang::Go => "components/api/main.go",
        Lang::Generic => "components/api/api.wasm",
    };
    let probe = match app.protocol {
        Protocol::Http => "curl http://127.0.0.1:8080/",
        Protocol::Sse => "curl -N http://127.0.0.1:8080/        # streams events; Ctrl-C to stop",
        Protocol::Ws => "websocat ws://127.0.0.1:8080/          # type a line; it echoes back",
    };
    format!(
        "# {name}\n\n\
         A RUSM app — a {lang} **{proto}** component running as an isolated, supervised\n\
         WASM process on an Erlang-style actor runtime.\n\n\
         ## Run it\n\n\
         ```sh\n\
         rusm build      # compile components/ -> wasm/\n\
         rusm serve      # serve on http://127.0.0.1:8080\n\
         ```\n\n\
         Then, in another terminal:\n\n\
         ```sh\n\
         {probe}\n\
         ```\n\n\
         ## Layout\n\n\
         - `{source}` — the handler (edit this).\n\
         - `rusm.toml` — what to serve, on which port, under which capability profile.\n\
         - `wasm/` — built artifacts (git-ignored); produced by `rusm build`.\n\n\
         Add more components under `components/<name>/` and reference them from `rusm.toml`.\n",
        proto = app.protocol.as_str(),
    )
}

/// The README for a custom-bridge app — explains the bridge (the app's own native host
/// capability), the host crate, and the build/serve flow, with the guest-specific call site.
fn bridge_readme(app: &NewApp) -> String {
    let name = &app.name;
    // HTTP-handler guests (Rust/Go) are curl'd; the TS guest is a per-connection WebSocket
    // handler, so its "Run it" sends a frame over a WebSocket instead.
    let http_run =
        "rusm build      # generates the bridge glue, compiles the guest + host binary\n\
         rusm serve      # runs the host binary; serves http://127.0.0.1:8080\n\
         curl http://127.0.0.1:8080/forecast/Amsterdam\n\
         # -> sunny in Amsterdam (served by pid ...)";
    let (guest, call, run) = match app.lang {
        Lang::TypeScript => (
            "components/api/index.ts (TypeScript)",
            "`weather.lookup(city)` / `weather.detailed(...)` (typed by the generated `bridges.d.ts`)",
            "rusm build      # rebuilds the js-runner with the bridge + generates bridges.d.ts\n\
             rusm serve      # runs the host binary; serves ws://127.0.0.1:8080\n\
             # send a city over a WebSocket, get its forecast back:\n\
             bun -e 'const w=new WebSocket(\"ws://127.0.0.1:8080\");w.onopen=()=>w.send(\"Amsterdam\");w.onmessage=e=>{console.log(\"\"+e.data);process.exit(0)}'\n\
             # -> sunny in Amsterdam (served by pid ...) — sunny @ 21°C",
        ),
        Lang::Go => (
            "components/api/main.go (Go)",
            "`forecast.Lookup(city)` (the wit-bindgen-go binding)",
            http_run,
        ),
        _ => (
            "components/api/src/lib.rs (Rust)",
            "`crate::forecast::lookup(city)` (re-exported by `#[handlers(bridge=…)]`)",
            http_run,
        ),
    };
    format!(
        "# {name}\n\n\
         A RUSM app with a **custom bridge** — its own native host capability (`weather`),\n\
         RUSM's compiled-in answer to a wasmCloud capability provider. The host impl is Rust\n\
         (the host *is* Rust); the guest calls it as an ordinary typed import, in any language.\n\n\
         ## Run it\n\n\
         ```sh\n\
         {run}\n\
         ```\n\n\
         ## Layout\n\n\
         - `bridges/weather/bridge.wit` — the bridge contract (the app's own WIT package).\n\
         - `bridges/weather/host.rs` — the native impl (`impl forecast::Host for BridgeHost`).\n\
         - `{guest}` — the guest handler; it calls the bridge via {call}.\n\
         - `src/main.rs` — the host binary: registers the bridges, then serves `rusm.toml`.\n\
         - `Cargo.toml` / `rusm.toml` — the host crate's deps and the app manifest.\n\n\
         `rusm build` generates the typed glue (`src/bindings.rs`, `src/bridges.rs`, the `wit/`\n\
         dirs) from `bridges/` — git-ignored build output. Add a bridge by dropping a new\n\
         `bridges/<name>/` (a `bridge.wit` + `host.rs`) and granting it in a `[capabilities.*]`\n\
         `bridges = [...]` list.\n\n\
         > The host crate depends on the `rusm-*` runtime crates by version; until they are\n\
         > published, point them at a local RUSM checkout (a `[patch.crates-io]` or `path`).\n",
    )
}

/// README for a mailer bridge app.
fn mailer_readme(app: &NewApp) -> String {
    let name = &app.name;
    let (guest, call) = match app.lang {
        Lang::TypeScript => (
            "components/api/index.ts (TypeScript)",
            "`mailer.send({ to, subject, body })` (typed global from the generated `bridges.d.ts`)",
        ),
        Lang::Go => (
            "components/api/main.go (Go)",
            "`smtp.Send(smtp.Message{...})` (the wit-bindgen-go binding)",
        ),
        _ => (
            "components/api/src/lib.rs (Rust)",
            "`crate::smtp::send(&msg)` (re-exported by `#[handlers(bridge=…)]`)",
        ),
    };
    format!(
        "# {name}\n\n\
         A RUSM app with a **mailer bridge** — a TypeScript host bridge that sends\n\
         transactional email via [Resend](https://resend.com). The guest calls\n\
         it as a plain typed import; RUSM routes the call to the resident host actor.\n\n\
         ## Run it\n\n\
         ```sh\n\
         # Set your Resend API key first:\n\
         echo 'RESEND_API_KEY=re_...' >> .env\n\n\
         rusm build      # generates the bridge glue + compiles the guest\n\
         rusm serve      # runs the host binary; serves http://127.0.0.1:8080\n\n\
         curl -X POST http://127.0.0.1:8080/send \\\n\
           -H 'Content-Type: application/json' \\\n\
           -d '{{\"to\":\"you@example.com\",\"subject\":\"Hello\",\"body\":\"<b>It works!</b>\"}}'\n\
         ```\n\n\
         ## Layout\n\n\
         - `bridges/mailer/bridge.wit` — the bridge contract (the app's own WIT package).\n\
         - `bridges/mailer/host.ts` — the TypeScript host impl (calls the Resend API).\n\
         - `{guest}` — the guest handler; it calls the bridge via {call}.\n\
         - `rusm.toml` — the app manifest.\n\n\
         `rusm build` generates all Rust glue (`src/bindings.rs`, `src/bridges.rs`,\n\
         `src/bridge_mailer_delegate.rs`, `src/main.rs`, `wit/`, `bridges.d.ts`) and\n\
         bundles the TS runner to `wasm/bridge-mailer.js` — all git-ignored build output.\n\n\
         Swap `noreply@example.com` in `bridges/mailer/host.ts` with a domain you own\n\
         and have verified with Resend.\n",
    )
}

/// A project name must be a single safe path segment (no separators, no `..`), so
/// scaffolding can never escape the target directory.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('-')
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        bail!("invalid app name `{name}` — use a simple directory name like `my-app`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusm_node::{NodeConfig, ServeProtocol};

    fn app(lang: Lang, protocol: Protocol) -> NewApp {
        NewApp {
            name: "demo".into(),
            lang,
            protocol,
            template: None,
            bridges: false,
        }
    }

    const COMBOS: &[(Lang, Protocol, ServeProtocol)] = &[
        (Lang::TypeScript, Protocol::Http, ServeProtocol::Http),
        (Lang::TypeScript, Protocol::Sse, ServeProtocol::Sse),
        (Lang::TypeScript, Protocol::Ws, ServeProtocol::Ws),
        (Lang::Rust, Protocol::Http, ServeProtocol::Http),
        (Lang::Rust, Protocol::Sse, ServeProtocol::Sse),
        (Lang::Rust, Protocol::Ws, ServeProtocol::Ws),
        (Lang::Go, Protocol::Http, ServeProtocol::Http),
        (Lang::Go, Protocol::Sse, ServeProtocol::Sse),
        (Lang::Go, Protocol::Ws, ServeProtocol::Ws),
        (Lang::Generic, Protocol::Http, ServeProtocol::Http),
        (Lang::Generic, Protocol::Ws, ServeProtocol::Ws),
    ];

    #[test]
    fn every_combo_scaffolds_a_coherent_app() {
        for &(lang, protocol, want_proto) in COMBOS {
            let dir = tempfile::tempdir().unwrap();
            let app = app(lang, protocol);
            let created = scaffold(dir.path(), &app).unwrap();
            let root = dir.path().join("demo");

            // Every advertised file is on disk.
            for rel in &created {
                assert!(
                    root.join(rel).is_file(),
                    "{lang:?}/{protocol:?}: missing {rel:?}"
                );
            }

            // For TS/Rust, the right component source exists with no leaked
            // boilerplate. Generic scaffolds no source — a README documents the
            // interface, never an empty `.gitkeep`.
            let source_check: Option<(PathBuf, &[&str])> = match lang {
                Lang::TypeScript => Some((
                    "components/api/index.ts".into(),
                    &["declare const Process", "Process.receive", "wit_bindgen"],
                )),
                Lang::Rust => Some((
                    "components/api/src/lib.rs".into(),
                    &["wit_bindgen::generate", "export!(", "impl Guest"],
                )),
                // Go's bindings live in the SDK, so the component carries none of the
                // wit-bindgen-go / component-export boilerplate.
                Lang::Go => Some((
                    "components/api/main.go".into(),
                    &["wit-bindgen", "//go:wasmexport", "process.Exports"],
                )),
                Lang::Generic => None,
            };
            match source_check {
                Some((src, forbidden)) => {
                    let source = std::fs::read_to_string(root.join(&src)).unwrap();
                    for needle in forbidden {
                        assert!(
                            !source.contains(needle),
                            "{lang:?}/{protocol:?}: leaked boilerplate `{needle}`"
                        );
                    }
                }
                None => {
                    let readme =
                        std::fs::read_to_string(root.join("components/api/README.md")).unwrap();
                    assert!(
                        readme.contains("wasi:http"),
                        "generic README names the interface"
                    );
                    assert!(
                        !root.join("components/api/.gitkeep").exists(),
                        "generic scaffolds a README, not an empty .gitkeep"
                    );
                }
            }

            // The generated rusm.toml parses through the real config and declares the
            // right protocol. A routed (Rust HTTP/SSE) app is a pure listener + a
            // `[components.<name>]` handler; a non-routed app names its handler on the listener.
            let toml = std::fs::read_to_string(root.join("rusm.toml")).unwrap();
            let cfg = NodeConfig::from_toml(&toml).expect("scaffolded rusm.toml must parse");
            assert_eq!(cfg.serve.len(), 1);
            assert_eq!(cfg.serve[0].protocol, want_proto, "{lang:?}/{protocol:?}");
            let routed = matches!(lang, Lang::Rust | Lang::Go) && protocol == Protocol::Http;
            let table = cfg.serve[0].route_table().expect("routes compile");
            assert_eq!(
                table.is_empty(),
                !routed,
                "{lang:?}/{protocol:?}: routes present iff Rust/Go HTTP"
            );
            if routed {
                // Routes name the `[components.api]` handler; the listener has no `component`.
                assert!(cfg.serve[0].component.is_none(), "{lang:?}/{protocol:?}");
                assert!(cfg.components.contains_key("api"), "{lang:?}/{protocol:?}");
            } else {
                // A single handler `component` on the listener.
                assert_eq!(
                    cfg.serve[0].component.as_deref(),
                    Some("api"),
                    "{lang:?}/{protocol:?}"
                );
            }
        }
    }

    #[test]
    fn rust_components_carry_no_wit_dir_and_use_a_rusm_macro() {
        // HTTP is a `#[rusm_rs::handlers]` component; SSE and WS are per-connection
        // `#[rusm_rs::main]` components. None needs a `wit/` dir (the macro inlines the
        // WIT) nor any wit-bindgen boilerplate.
        for (protocol, macro_attr) in [
            (Protocol::Http, "#[rusm_rs::handlers]"),
            (Protocol::Sse, "#[rusm_rs::main]"),
            (Protocol::Ws, "#[rusm_rs::main]"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            scaffold(dir.path(), &app(Lang::Rust, protocol)).unwrap();
            let root = dir.path().join("demo");
            assert!(
                !root.join("components/api/wit").exists(),
                "{protocol:?}: no wit/ dir needed"
            );
            let src = std::fs::read_to_string(root.join("components/api/src/lib.rs")).unwrap();
            assert!(
                src.contains(macro_attr),
                "{protocol:?}: expected {macro_attr}"
            );
        }
    }

    #[test]
    fn only_the_rusm_importing_ts_component_gets_a_package_json() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &app(Lang::TypeScript, Protocol::Ws)).unwrap();
        assert!(dir.path().join("demo/package.json").is_file());
        assert!(
            std::fs::read_to_string(dir.path().join("demo/components/api/index.ts"))
                .unwrap()
                .contains("import { websocket } from \"rusm-ts\"")
        );

        let dir2 = tempfile::tempdir().unwrap();
        scaffold(dir2.path(), &app(Lang::TypeScript, Protocol::Http)).unwrap();
        assert!(
            !dir2.path().join("demo/package.json").exists(),
            "a zero-dep web-standard handler needs no package.json"
        );
    }

    #[test]
    fn parses_flags_with_sensible_defaults() {
        use pico_args::Arguments;

        let p = |args: &[&str]| {
            let os_args: Vec<std::ffi::OsString> =
                args.iter().map(std::ffi::OsString::from).collect();
            parse_new_args(Arguments::from_vec(os_args))
        };
        let d = p(&["hello"]).unwrap();
        assert_eq!(d.lang, Lang::TypeScript);
        assert_eq!(d.protocol, Protocol::Http);

        assert_eq!(p(&["hello", "--rust"]).unwrap().lang, Lang::Rust);
        assert_eq!(p(&["hello", "--lang", "rust"]).unwrap().lang, Lang::Rust);
        assert_eq!(
            p(&["hello", "--protocol", "ws"]).unwrap().protocol,
            Protocol::Ws
        );
        assert_eq!(
            p(&["hello", "--protocol=sse"]).unwrap().protocol,
            Protocol::Sse
        );
        assert_eq!(p(&["hello", "-p", "ws"]).unwrap().protocol, Protocol::Ws);
        assert_eq!(
            p(&["hello", "--lang", "generic"]).unwrap().lang,
            Lang::Generic
        );
        assert_eq!(p(&["hello", "--lang", "wasm"]).unwrap().lang, Lang::Generic);
        assert_eq!(p(&["hello", "--lang", "go"]).unwrap().lang, Lang::Go);
        assert_eq!(p(&["hello", "--lang", "golang"]).unwrap().lang, Lang::Go);
        // Order-independent.
        let mixed = p(&["--rust", "-p", "sse", "hello"]).unwrap();
        assert_eq!(
            (mixed.lang, mixed.protocol, mixed.name.as_str()),
            (Lang::Rust, Protocol::Sse, "hello")
        );

        // `--template` is off by default and parses when given.
        assert_eq!(d.template, None);
        assert_eq!(
            p(&["hello", "--template", "todo-board", "--lang", "go"])
                .unwrap()
                .template,
            Some(Template::TodoBoard)
        );
    }

    #[test]
    fn rejects_bad_input() {
        use pico_args::Arguments;

        let p = |args: &[&str]| {
            let os_args: Vec<std::ffi::OsString> =
                args.iter().map(std::ffi::OsString::from).collect();
            parse_new_args(Arguments::from_vec(os_args))
        };
        assert!(p(&[]).is_err(), "missing name");
        assert!(p(&["a", "b"]).is_err(), "two names");
        assert!(p(&["hello", "--protocol", "grpc"]).is_err(), "bad protocol");
        assert!(p(&["hello", "--lang", "cobol"]).is_err(), "bad language");
        assert!(p(&["hello", "--frobnicate"]).is_err(), "unknown flag");
        assert!(
            p(&["hello", "--protocol"]).is_err(),
            "missing protocol value"
        );
        assert!(p(&["--rust"]).is_err(), "options but no name");
        assert!(p(&["-p", "ws"]).is_err(), "options but no name");
        assert!(p(&["--frobnicate"]).is_err(), "a lone flag is not a name");
        assert!(
            p(&["hello", "--template", "bogus"]).is_err(),
            "bad template"
        );
        assert!(
            p(&["hello", "--template", "todo-board", "--lang", "generic"]).is_err(),
            "a template needs a guest language"
        );
        assert!(
            p(&["hello", "--template", "todo-board", "--protocol", "ws"]).is_err(),
            "--protocol conflicts with --template"
        );
        for bad in ["..", ".", "a/b", "", "a\\b", "-x", "--name"] {
            assert!(p(&[bad]).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn refuses_a_non_empty_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("taken")).unwrap();
        std::fs::write(dir.path().join("taken/keep.txt"), "x").unwrap();
        let occupied = NewApp {
            name: "taken".into(),
            lang: Lang::TypeScript,
            protocol: Protocol::Http,
            template: None,
            bridges: false,
        };
        let err = scaffold(dir.path(), &occupied).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn template_scaffolds_the_full_todo_board() {
        for lang in [Lang::TypeScript, Lang::Rust, Lang::Go] {
            let dir = tempfile::tempdir().unwrap();
            let app = NewApp {
                name: "demo".into(),
                lang,
                protocol: Protocol::Http,
                template: Some(Template::TodoBoard),
                bridges: false,
            };
            let created = scaffold(dir.path(), &app).unwrap();
            let root = dir.path().join("demo");
            for rel in &created {
                assert!(root.join(rel).is_file(), "{lang:?}: missing {rel:?}");
            }
            // The full app, not the single-component starter: the three serving listeners
            // plus the composition components, and a README to read.
            let cfg =
                NodeConfig::from_toml(&std::fs::read_to_string(root.join("rusm.toml")).unwrap())
                    .expect("template rusm.toml parses");
            assert_eq!(cfg.serve.len(), 3, "{lang:?}: http + sse + ws listeners");
            assert!(
                cfg.components.contains_key("store"),
                "{lang:?}: store service"
            );
            assert!(
                cfg.components.contains_key("reporter"),
                "{lang:?}: reporter worker"
            );
            assert!(root.join("README.md").is_file(), "{lang:?}: README");
        }
    }

    fn parse(args: &[&str]) -> Result<NewApp> {
        let os: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
        parse_new_args(pico_args::Arguments::from_vec(os))
    }

    #[test]
    fn bridges_flag_parses_and_accepts_every_guest_language() {
        // `--bridges` defaults the *guest* to Rust (its host is Rust); ts and go are accepted
        // too — a TS guest can call custom bridges (the per-app js-runner is rebuilt with it).
        let rust = parse(&["app", "--bridges"]).unwrap();
        assert!(rust.bridges && rust.lang == Lang::Rust);
        assert_eq!(
            parse(&["app", "--bridges", "--lang", "go"]).unwrap().lang,
            Lang::Go
        );
        assert_eq!(
            parse(&["app", "--bridges", "--lang", "ts"]).unwrap().lang,
            Lang::TypeScript
        );
        // `generic` (no guest source) and combining with `--template` are rejected.
        assert!(
            parse(&["app", "--bridges", "--lang", "generic"]).is_err(),
            "generic guest"
        );
        assert!(
            parse(&["app", "--bridges", "--template", "todo-board"]).is_err(),
            "template combo"
        );
        // A plain app is unaffected.
        assert!(!parse(&["app"]).unwrap().bridges);
    }

    /// `rusm new <name> --bridges` scaffolds a coherent custom-bridge app whose host-side
    /// files are exactly the live example, with a generated (version-dep) host crate +
    /// manifest. Asserted for both guest languages.
    #[test]
    fn bridges_scaffolds_a_host_app_for_each_guest() {
        for (lang, guest_src) in [
            (Lang::Rust, "components/api/src/lib.rs"),
            (Lang::Go, "components/api/main.go"),
            (Lang::TypeScript, "components/api/index.ts"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = NewApp {
                name: "weatherapp".into(),
                lang,
                protocol: Protocol::Http,
                template: None,
                bridges: true,
            };
            let created = scaffold(dir.path(), &app).unwrap();
            let root = dir.path().join("weatherapp");
            for rel in &created {
                assert!(root.join(rel).is_file(), "{lang:?}: missing {rel:?}");
            }
            // Host crate + the example bridge + the guest.
            for rel in [
                "Cargo.toml",
                "src/main.rs",
                "bridges/weather/bridge.wit",
                "bridges/weather/host.rs",
                "rusm.toml",
                ".gitignore",
                "README.md",
                guest_src,
            ] {
                assert!(root.join(rel).is_file(), "{lang:?}: missing {rel}");
            }

            // Host-side files are the live example verbatim (single source).
            assert_eq!(
                std::fs::read_to_string(root.join("src/main.rs")).unwrap(),
                include_str!("../../examples/weather-api/rust/src/main.rs"),
            );
            assert_eq!(
                std::fs::read_to_string(root.join("bridges/weather/host.rs")).unwrap(),
                include_str!("../../examples/weather-api/rust/bridges/weather/host.rs"),
            );

            // The host crate pins the runtime crates + the exact wasmtime, named after the app.
            let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
            assert!(cargo.contains("name = \"weatherapp\""));
            assert!(cargo.contains(&format!("rusm-wasm = \"{SDK_VERSION}\"")));
            assert!(cargo.contains(&format!("wasmtime = \"{WASMTIME_VERSION}\"")));

            // The manifest routes to `api` under a profile granting the `weather` bridge.
            let cfg =
                NodeConfig::from_toml(&std::fs::read_to_string(root.join("rusm.toml")).unwrap())
                    .expect("bridge rusm.toml parses");
            assert_eq!(cfg.capabilities["forecaster"].bridges, ["weather"]);
            assert!(cfg.components.contains_key("api"));

            // Generated glue is git-ignored (it's `rusm build` output, not scaffolded).
            let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
            assert!(ignore.contains("/src/bindings.rs") && ignore.contains("/components/*/wit/"));
        }
    }

    /// `--template weather` is the discoverable name for the custom-bridge example: it parses
    /// to the `Weather` template and scaffolds the same host + bridge + guest app (the bridge
    /// path, not `template::files`), in any guest language.
    #[test]
    fn weather_template_scaffolds_the_custom_bridge_app() {
        assert_eq!(
            parse(&["app", "--template", "weather"]).unwrap().template,
            Some(Template::Weather)
        );
        for (lang, guest_src) in [
            (Lang::TypeScript, "components/api/index.ts"),
            (Lang::Rust, "components/api/src/lib.rs"),
            (Lang::Go, "components/api/main.go"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = NewApp {
                name: "forecast".into(),
                lang,
                protocol: Protocol::Http,
                template: Some(Template::Weather),
                bridges: false,
            };
            scaffold(dir.path(), &app).unwrap();
            let root = dir.path().join("forecast");
            // The bridge, the host binary, and the chosen guest are all present.
            for rel in [
                "bridges/weather/host.rs",
                "src/main.rs",
                "rusm.toml",
                guest_src,
            ] {
                assert!(root.join(rel).is_file(), "{lang:?}: missing {rel}");
            }
            // The manifest grants the `weather` bridge to the `api` component, every guest.
            let cfg =
                NodeConfig::from_toml(&std::fs::read_to_string(root.join("rusm.toml")).unwrap())
                    .expect("weather rusm.toml parses");
            assert_eq!(cfg.capabilities["forecaster"].bridges, ["weather"]);
            assert!(cfg.components.contains_key("api"));
        }
    }

    /// The Go guest template uses the `api` module name (from `go_mod()`), not a custom name.
    #[test]
    fn go_bridge_guest_uses_api_module() {
        let go = go_bridge_guest();
        assert!(go.contains("\"api/internal/wit/weather/bridge/forecast\""));
        assert!(
            !go.contains("go-api/internal"),
            "template must not contain the old go-api module path"
        );
        assert!(go.contains("forecast.Lookup"));
    }

    /// The mailer Go guest uses the `api` module name (from `go_mod()`), not the app name.
    #[test]
    fn mailer_go_guest_uses_api_module() {
        let app = NewApp {
            name: "notifier".into(),
            lang: Lang::Go,
            protocol: Protocol::Http,
            template: Some(Template::Mailer),
            bridges: false,
        };
        let go = mailer_go_guest(&app);
        assert!(
            go.contains("\"api/internal/wit/app/mailer/smtp\""),
            "import path must use the `api` module name"
        );
        assert!(
            !go.contains("notifier/internal"),
            "must not embed the app name — go.mod is always `module api`"
        );
        assert!(go.contains("smtp.Send"));
    }

    /// The host crate's pinned wasmtime must track what `rusm-wasm` links (bindgen type
    /// identity) — guard the constant against the runtime crate going out of sync.
    #[test]
    fn bridge_wasmtime_pin_tracks_rusm_wasm() {
        let rusm_wasm_cargo = include_str!("../../crates/rusm-wasm/Cargo.toml");
        assert!(
            rusm_wasm_cargo.contains(&format!("wasmtime = \"{WASMTIME_VERSION}\"")),
            "WASMTIME_VERSION ({WASMTIME_VERSION}) is stale vs crates/rusm-wasm/Cargo.toml",
        );
    }

    /// `--template mailer` scaffolds a TS-hosted mailer bridge app in any guest language.
    #[test]
    fn mailer_template_scaffolds_the_bridge_app() {
        assert_eq!(
            parse(&["app", "--template", "mailer"]).unwrap().template,
            Some(Template::Mailer)
        );
        for (lang, guest_src) in [
            (Lang::TypeScript, "components/api/index.ts"),
            (Lang::Rust, "components/api/src/lib.rs"),
            (Lang::Go, "components/api/main.go"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = NewApp {
                name: "notifier".into(),
                lang,
                protocol: Protocol::Http,
                template: Some(Template::Mailer),
                bridges: false,
            };
            scaffold(dir.path(), &app).unwrap();
            let root = dir.path().join("notifier");
            for rel in [
                "bridges/mailer/bridge.wit",
                "bridges/mailer/host.ts",
                "rusm.toml",
                guest_src,
            ] {
                assert!(root.join(rel).is_file(), "{lang:?}: missing {rel}");
            }
            let cfg =
                NodeConfig::from_toml(&std::fs::read_to_string(root.join("rusm.toml")).unwrap())
                    .expect("mailer rusm.toml parses");
            assert_eq!(cfg.capabilities["notifier"].bridges, ["mailer"]);
            assert!(cfg.components.contains_key("api"));
            // The generated glue is git-ignored (produced by `rusm build`).
            let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
            assert!(ignore.contains("/src/bindings.rs") && ignore.contains("/components/*/wit/"));
        }
    }

    /// The `mailer` template files are vendored from `examples/mailer/` — byte-identical to
    /// the live example. Regenerate with `make sync-templates`.
    #[test]
    fn vendored_mailer_template_matches_sources() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.join("..");
        let pairs = [
            (
                "templates/mailer/bridge.wit",
                "examples/mailer/typescript/bridges/mailer/bridge.wit",
            ),
            (
                "templates/mailer/host.ts",
                "examples/mailer/typescript/bridges/mailer/host.ts",
            ),
            (
                "templates/mailer/ts-guest.ts",
                "examples/mailer/typescript/components/api/index.ts",
            ),
        ];
        for (vendored, source) in pairs {
            let v = std::fs::read(manifest.join(vendored)).unwrap_or_else(|_| {
                panic!("missing vendored {vendored} — run `make sync-templates`")
            });
            let s = std::fs::read(root.join(source))
                .unwrap_or_else(|_| panic!("missing source {source}"));
            assert_eq!(
                v, s,
                "{vendored} drifted from {source} — run `make sync-templates`"
            );
        }
    }
}
