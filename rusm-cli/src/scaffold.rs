//! `rusm new <name> [--rust] [--protocol http|sse|ws]` — scaffold a new RUSM app.
//!
//! Produces a project whose component source is **pure developer logic** — no
//! `wit-bindgen`/`export!` boilerplate (Rust hides it behind `#[rusm_rs::main]`) and
//! no `Process`/frame plumbing (TS uses web standards and the `rusm-ts` package). Pick a
//! language (`--rust`, default TypeScript) and a protocol (`--protocol`, default
//! `http`); from nothing to a live server in three commands:
//!
//! ```text
//! rusm new hello && cd hello
//! rusm build      # components/<name>/ -> wasm/<name>.{js,wasm}
//! rusm serve      # hosts it on http://127.0.0.1:8080
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The guest language for the scaffolded component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    TypeScript,
    Rust,
}

/// The protocol the component is served over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Http,
    Sse,
    Ws,
}

impl Protocol {
    fn as_str(self) -> &'static str {
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
}

/// Parse the arguments following `rusm new` into a [`NewApp`]: a single positional
/// name plus optional `--rust`/`--lang <ts|rust>` and `--protocol <http|sse|ws>`
/// (`-p`, `--protocol=…` also accepted). Unknown flags, bad values, and a missing or
/// duplicate name are hard errors — a typo never silently scaffolds the wrong thing.
pub fn parse_new_args(mut parser: pico_args::Arguments) -> Result<NewApp> {
    // Parse flags and options first
    let rust_flag = parser.contains("--rust");
    let lang: Result<Option<String>, _> = parser.opt_value_from_str("--lang");
    // Try to parse --protocol or -p
    let protocol1: Result<Option<String>, _> = parser.opt_value_from_str("--protocol");
    let protocol: Result<Option<String>, _> = match protocol1 {
        Ok(Some(val)) => Ok(Some(val)),
        Ok(None) => parser.opt_value_from_str("-p"),
        Err(e) => Err(e),
    };

    // Then parse the free argument (name)
    let name: Result<String, _> = parser.free_from_str();

    // Check for remaining arguments which would indicate duplicate names
    // Note: picoargs consumes arguments as it parses them, so remaining should only
    // contain unexpected free arguments
    let remaining = parser.finish();
    if !remaining.is_empty() {
        // The first remaining argument is the duplicate name
        anyhow::bail!(
            "unexpected argument `{}`, the app name is already `{}`",
            remaining[0].to_string_lossy(),
            name.as_ref().map_or("", |n| n.as_str())
        );
    }

    let name_str = name.map_err(|e| anyhow::anyhow!(e))?;
    let lang_opt = lang.map_err(|e| anyhow::anyhow!(e))?;
    let protocol_opt = protocol.map_err(|e| anyhow::anyhow!(e))?;

    // Apply the parsed values to create the NewApp
    let mut lang = if rust_flag {
        Lang::Rust
    } else {
        Lang::TypeScript
    };

    let mut protocol = Protocol::Http;

    if let Some(lang_str) = lang_opt {
        lang = parse_lang(&lang_str)?;
    }

    if let Some(proto_str) = protocol_opt {
        protocol = parse_protocol(&proto_str)?;
    }

    validate_name(&name_str)?;
    Ok(NewApp {
        name: name_str,
        lang,
        protocol,
    })
}

fn parse_lang(value: &str) -> Result<Lang> {
    match value {
        "ts" | "typescript" => Ok(Lang::TypeScript),
        "rust" | "rs" => Ok(Lang::Rust),
        other => bail!("unknown language `{other}` — use `ts` or `rust`"),
    }
}

fn parse_protocol(value: &str) -> Result<Protocol> {
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
                CARGO_TOML.to_string(),
            ));
            out.push((
                PathBuf::from("components/api/src/lib.rs"),
                rust_component(app.protocol).to_string(),
            ));
        }
    }
    out
}

/// A Rust HTTP/SSE app uses the `#[rusm_rs::handlers]` model — named actions reached
/// through a `[serve.routes]` table to a `[components.<name>]` handler. TS HTTP/SSE (a
/// `wasi:http` `export default`) and WebSocket (per-connection) are a single named
/// handler component with no routes.
fn has_routes(app: &NewApp) -> bool {
    app.lang == Lang::Rust && matches!(app.protocol, Protocol::Http | Protocol::Sse)
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
             protocol = \"{proto}\"           # http | sse\n\
             listen = \"127.0.0.1:8080\"\n\n\
             [serve.routes]\n\
             \"GET /\" = \"api#home\"        # METHOD /path = component#action\n\n\
             [components.api]              # the handler, built from components/api\n\
             capability = \"sandboxed\"    # default-deny; see [capabilities.<name>] for more\n"
        )
    } else {
        // A single named handler component (TS `export default`, or a WebSocket worker).
        let artifact = match app.lang {
            Lang::TypeScript => "wasm/api.js",
            Lang::Rust => "wasm/api.wasm",
        };
        format!(
            "{TOML_HEADER}\
             [[serve]]\n\
             protocol = \"{proto}\"           # http | sse | ws\n\
             listen = \"127.0.0.1:8080\"\n\
             name = \"api\"               # loads {artifact}, built from components/api\n"
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

fn package_json(name: &str) -> String {
    format!(
        "{{\n  \"name\": \"{name}\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"dependencies\": {{\n    \"rusm-ts\": \"^0.1.0\"\n  }}\n}}\n"
    )
}

/// The Rust component crate — one `cdylib`, the `rusm-rs` guest crate, and
/// `wit-bindgen` (which `#[rusm_rs::main]` drives so the source carries no `wit/`).
const CARGO_TOML: &str = "\
[package]
name = \"api\"
version = \"0.1.0\"
edition = \"2021\"

[lib]
crate-type = [\"cdylib\"]

[dependencies]
rusm-rs = \"0.1\"
wit-bindgen = \"0.46\"

[profile.release]
opt-level = \"z\"
strip = true

[workspace]
";

fn ts_component(protocol: Protocol) -> &'static str {
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

fn rust_component(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Http => {
            "\
//! A RUSM HTTP component: a module of named handler **actions**. The `[routes]` table
//! in rusm.toml maps `METHOD /path` to `api#<action>`; the host spawns a fresh
//! sandboxed instance per request and dispatches the matched action here — no `main`,
//! no router, no request/reply plumbing.
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    pub fn home(request: Request, _params: Params) -> Response {
        let url = request.url;
        Response::text(format!(\"Hello from RUSM \u{1F44B}  (you asked for {url})\\n\"))
    }
}
"
        }
        Protocol::Sse => {
            "\
//! A RUSM SSE component: a handler **action** taking `Sse` streams a `text/event-stream`
//! body. Each request is its own process, so the action may block here for the whole
//! connection — write events as they occur. `[routes]` maps `GET /` to `api#home`.
use rusm_rs::http::{Params, Request, Sse};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    pub fn home(_request: Request, _params: Params, sse: Sse) {
        for n in 0..5 {
            if !sse.data(format!(\"tick {n}\").as_bytes()) {
                break; // the client disconnected
            }
        }
    }
}
"
        }
        Protocol::Ws => {
            "\
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
        }
    }
}

fn readme(app: &NewApp) -> String {
    let name = &app.name;
    let lang = match app.lang {
        Lang::TypeScript => "TypeScript",
        Lang::Rust => "Rust",
    };
    let source = match app.lang {
        Lang::TypeScript => "components/api/index.ts",
        Lang::Rust => "components/api/src/lib.rs",
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

/// A project name must be a single safe path segment (no separators, no `..`), so
/// scaffolding can never escape the target directory.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
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
        }
    }

    const COMBOS: &[(Lang, Protocol, ServeProtocol)] = &[
        (Lang::TypeScript, Protocol::Http, ServeProtocol::Http),
        (Lang::TypeScript, Protocol::Sse, ServeProtocol::Sse),
        (Lang::TypeScript, Protocol::Ws, ServeProtocol::Ws),
        (Lang::Rust, Protocol::Http, ServeProtocol::Http),
        (Lang::Rust, Protocol::Sse, ServeProtocol::Sse),
        (Lang::Rust, Protocol::Ws, ServeProtocol::Ws),
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

            // The right component source exists for the language, and no boilerplate
            // leaks into it.
            let (src, forbidden): (PathBuf, &[&str]) = match lang {
                Lang::TypeScript => (
                    "components/api/index.ts".into(),
                    &["declare const Process", "Process.receive", "wit_bindgen"],
                ),
                Lang::Rust => (
                    "components/api/src/lib.rs".into(),
                    &["wit_bindgen::generate", "export!(", "impl Guest"],
                ),
            };
            let source = std::fs::read_to_string(root.join(&src)).unwrap();
            for needle in forbidden {
                assert!(
                    !source.contains(needle),
                    "{lang:?}/{protocol:?}: leaked boilerplate `{needle}`"
                );
            }

            // The generated rusm.toml parses through the real config and declares the
            // right protocol. A routed (Rust HTTP/SSE) app is a pure listener + a
            // `[components.<name>]` handler; a non-routed app names its handler on the listener.
            let toml = std::fs::read_to_string(root.join("rusm.toml")).unwrap();
            let cfg = NodeConfig::from_toml(&toml).expect("scaffolded rusm.toml must parse");
            assert_eq!(cfg.serve.len(), 1);
            assert_eq!(cfg.serve[0].protocol, want_proto, "{lang:?}/{protocol:?}");
            let routed = lang == Lang::Rust && matches!(protocol, Protocol::Http | Protocol::Sse);
            let table = cfg.serve[0].route_table().expect("routes compile");
            assert_eq!(
                table.is_empty(),
                !routed,
                "{lang:?}/{protocol:?}: routes present iff Rust HTTP/SSE"
            );
            if routed {
                // Routes name the `[components.api]` handler; the listener has no `name`.
                assert!(cfg.serve[0].name.is_none(), "{lang:?}/{protocol:?}");
                assert!(cfg.components.contains_key("api"), "{lang:?}/{protocol:?}");
            } else {
                // A single named handler component on the listener.
                assert_eq!(
                    cfg.serve[0].name.as_deref(),
                    Some("api"),
                    "{lang:?}/{protocol:?}"
                );
            }
        }
    }

    #[test]
    fn rust_components_carry_no_wit_dir_and_use_a_rusm_macro() {
        // HTTP is a `#[rusm_rs::handlers]` component; WS is `#[rusm_rs::main]`. Neither
        // needs a `wit/` dir (the macro inlines the WIT) nor any wit-bindgen boilerplate.
        for (protocol, macro_attr) in [
            (Protocol::Http, "#[rusm_rs::handlers]"),
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
        // Order-independent.
        let mixed = p(&["--rust", "-p", "sse", "hello"]).unwrap();
        assert_eq!(
            (mixed.lang, mixed.protocol, mixed.name.as_str()),
            (Lang::Rust, Protocol::Sse, "hello")
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
        assert!(p(&["hello", "--lang", "go"]).is_err(), "bad language");
        assert!(p(&["hello", "--frobnicate"]).is_err(), "unknown flag");
        assert!(
            p(&["hello", "--protocol"]).is_err(),
            "missing protocol value"
        );
        for bad in ["..", ".", "a/b", "", "a\\b"] {
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
        };
        let err = scaffold(dir.path(), &occupied).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }
}
