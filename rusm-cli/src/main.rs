use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context};
use futures_util::{SinkExt, StreamExt};
use pico_args::Arguments;
use rusm_cli::{
    normalize_target, parse, parse_new_args, render_message, scaffold, serve_apps,
    spawn_components, Hosted, Protocol, ReplInput, DEFAULT_HOST, HELP,
};
use rusm_node::{serve, ClientCommand, Node, NodeConfig, ServerMessage};
use rusm_otp::Runtime;
use rusm_wasm::WasmRuntime;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let mut args = Arguments::from_env();

    // Get the subcommand
    let command = match args.subcommand() {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("Error: {e}");
            eprintln!("\nusage:");
            print_usage();
            std::process::exit(1);
        }
    };

    // If no command provided, check for global help or show usage
    let Some(command) = command else {
        if args.contains("--help") || args.contains("-h") || args.contains("help") {
            eprintln!("usage:");
            print_usage();
            std::process::exit(0);
        }
        eprintln!("usage:");
        print_usage();
        std::process::exit(2);
    };

    // Now match on the command
    match command.as_str() {
        "node" => {
            // For node, we expect a subcommand like "start"
            let node_subcommand = match args.subcommand() {
                Ok(cmd) => cmd,
                Err(_) => {
                    eprintln!("usage: rusm node start [--config <file>] [--listen <addr>]");
                    std::process::exit(1);
                }
            };

            let Some(node_subcommand) = node_subcommand else {
                eprintln!("usage: rusm node start [--config <file>] [--listen <addr>]");
                std::process::exit(1);
            };

            if node_subcommand != "start" {
                eprintln!("usage: rusm node start [--config <file>] [--listen <addr>]");
                std::process::exit(1);
            }

            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!("usage: rusm node start [--config <file>] [--listen <addr>]");
                std::process::exit(0);
            }

            let config: Option<String> = args.opt_value_from_str("--config").ok().flatten();
            let listen: Option<String> = args.opt_value_from_str("--listen").ok().flatten();

            // Collect remaining args for load_node_config compatibility
            let all_args: Vec<String> = std::env::args().collect();
            if let Err(error) = start_node(&all_args, config.as_deref(), listen.as_deref()).await {
                eprintln!("node start failed: {error}");
                std::process::exit(1);
            }
        }
        "attach" => {
            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!(
                    "usage: rusm attach [<host | host:port | ws-url>]   (defaults to 127.0.0.1:4000)"
                );
                std::process::exit(0);
            }

            // Get the target if provided, otherwise use default
            let target_str: Option<String> = args.opt_free_from_str().ok().flatten();
            let target = normalize_target(target_str.as_deref().unwrap_or(DEFAULT_HOST));
            if let Err(error) = attach(&target).await {
                eprintln!("attach failed: {error}");
                std::process::exit(1);
            }
        }
        "new" => {
            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!(
                    "usage: rusm new <name> [--rust] [--lang ts|rust] [--protocol http|sse|ws]"
                );
                std::process::exit(0);
            }

            // Pass the arguments directly to parse_new_args
            match parse_new_args(args) {
                Ok(app) => match scaffold(Path::new("."), &app) {
                    Ok(_) => {
                        let probe = match app.protocol {
                            Protocol::Http => "curl http://127.0.0.1:8080/",
                            Protocol::Sse => "curl -N http://127.0.0.1:8080/",
                            Protocol::Ws => "websocat ws://127.0.0.1:8080/",
                        };
                        println!("created {}/", app.name);
                        println!("\nnext:");
                        println!("  cd {}", app.name);
                        println!("  rusm build      # compile components/ -> wasm/");
                        println!("  rusm serve      # http://127.0.0.1:8080");
                        println!("  {probe}");
                    }
                    Err(error) => {
                        eprintln!("new failed: {error}");
                        std::process::exit(1);
                    }
                },
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
            }
        }
        "build" => {
            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!(
                    "usage: rusm build                 compile ./components/* -> ./wasm/*.wasm"
                );
                std::process::exit(0);
            }

            match build_components(Path::new(".")) {
                Ok(built) if built.is_empty() => {
                    println!("no component crates found under ./components");
                }
                Ok(built) => println!(
                    "built {} component(s) -> ./wasm: {}",
                    built.len(),
                    built.join(", ")
                ),
                Err(error) => {
                    eprintln!("build failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "run" => {
            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!("usage: rusm run                   run ./wasm components per rusm.toml [components.<name>]");
                std::process::exit(0);
            }

            let all_args: Vec<String> = std::env::args().collect();
            if let Err(error) = run_app(&all_args).await {
                eprintln!("run failed: {error}");
                std::process::exit(1);
            }
        }
        "serve" => {
            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!("usage: rusm serve                 host ./wasm components as HTTP/WS/SSE servers per rusm.toml [[serve]]");
                std::process::exit(0);
            }

            let all_args: Vec<String> = std::env::args().collect();
            if let Err(error) = serve_app(&all_args).await {
                eprintln!("serve failed: {error}");
                std::process::exit(1);
            }
        }
        "dev" => {
            // Check for help flag
            if args.contains("--help") || args.contains("-h") {
                eprintln!(
                    "usage: rusm dev                   build + run, then watch & reload on edits"
                );
                std::process::exit(0);
            }

            let all_args: Vec<String> = std::env::args().collect();
            if let Err(error) = dev(&all_args).await {
                eprintln!("dev failed: {error}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("Unknown command: {command}");
            eprintln!("\nusage:");
            print_usage();
            std::process::exit(2);
        }
    }
}

fn print_usage() {
    eprintln!("  rusm new <name>            scaffold a new RUSM app in ./<name>");
    eprintln!(
        "  rusm node start [--config <file>] [--listen <addr>]   host the app + a live attach endpoint"
    );
    eprintln!("  rusm build                 compile ./components/* -> ./wasm/*.wasm");
    eprintln!(
        "  rusm run                   run ./wasm components per rusm.toml [components.<name>]"
    );
    eprintln!("  rusm dev                   build + run, then watch & reload on edits");
    eprintln!(
        "  rusm serve                 host ./wasm components as HTTP/WS/SSE servers per rusm.toml [[serve]]"
    );
    eprintln!("  rusm attach [<host | host:port | ws-url>]   (defaults to 127.0.0.1:4000)");
}

/// `rusm node start`: host the app's `[components.<name>]` (like `rusm run`) and expose
/// a live **attach** endpoint on `[node] listen`, so `rusm attach` can observe the
/// node's processes. The served runtime + held handles keep everything alive for
/// the lifetime of the server (which runs until Ctrl-C or a bind error).
async fn start_node(
    args: &[String],
    config: Option<&str>,
    listen: Option<&str>,
) -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = load_node_config(args, config, listen);
    let rt = Runtime::new();
    // `wasm` + `hosted` stay bound for the whole function: they own the hosted
    // components' runtime + resident supervisor, so they must outlive the server below.
    let wasm = wasm_runtime(rt.clone(), &cfg)?;
    let hosted =
        spawn_components(Path::new("."), &wasm, &cfg.components, &cfg.capabilities).await?;
    let node = Node::new(rt.clone(), node_name(), cfg.node.ticks_per_second);
    println!(
        "rusm node listening on ws://{} ({} component(s), {} Hz)",
        cfg.node.listen,
        hosted.names.len(),
        cfg.node.ticks_per_second
    );
    println!("attach with:  rusm attach {}", cfg.node.listen);
    serve(&cfg.node.listen, node).await?;
    Ok(())
}

/// The node's display name for `attach`: the app directory's name (e.g. `hello`),
/// falling back to `rusm`.
fn node_name() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "rusm".to_string())
}

/// Runs the app's components: load `.env` (process env wins), then register each
/// `[components.<name>]` entry from `./wasm` under its capability profile (booting +
/// supervising the resident ones), and wait for Ctrl-C. `wasm` + `hosted` keep the
/// processes alive.
async fn run_app(args: &[String]) -> anyhow::Result<()> {
    // Environment variables the Rust way: process env first, then ./.env.
    dotenvy::dotenv().ok();

    let cfg = load_node_config(args, None, None);
    let rt = Runtime::new();
    let wasm = wasm_runtime(rt.clone(), &cfg)?;
    let hosted =
        spawn_components(Path::new("."), &wasm, &cfg.components, &cfg.capabilities).await?;
    if hosted.is_empty() {
        println!("no [components] in rusm.toml — nothing to run");
        return Ok(());
    }
    print_hosted(&hosted);
    println!("press Ctrl-C to stop");
    tokio::signal::ctrl_c().await?;
    println!("\nstopping {} process(es)…", rt.shutdown());
    Ok(())
}

/// One line describing what the node is hosting: the resident services (boot-spawned
/// + supervised) and the on-demand components (registered, spawned per request/call).
fn print_hosted(hosted: &Hosted) {
    let on_demand: Vec<&str> = hosted
        .names
        .iter()
        .filter(|n| !hosted.resident.contains(*n))
        .map(String::as_str)
        .collect();
    if !hosted.resident.is_empty() {
        println!("resident: {}", hosted.resident.join(", "));
    }
    if !on_demand.is_empty() {
        println!("on demand: {}", on_demand.join(", "));
    }
}

/// `rusm serve`: host each `[[serve]]` component as a real network server on its
/// own port (HTTP/SSE or WebSocket), then wait for Ctrl-C. The bound runtime + the
/// accept-loop tasks keep the servers up. This is the *server* side of a fair
/// benchmark: the node only serves; load is driven out-of-process (`rusm-loadtest`).
async fn serve_app(args: &[String]) -> anyhow::Result<()> {
    // Env the Rust way: process env first, then ./.env.
    dotenvy::dotenv().ok();

    let cfg = load_node_config(args, None, None);
    let rt = Runtime::new();
    let wasm = wasm_runtime(rt.clone(), &cfg)?;
    // Register the app's `[components.<name>]` on the **same** node first, so a
    // `[[serve]]` route can spawn a matched handler and a sibling can `whereis` a
    // resident service — an app that serves HTTP *and* runs resident services comes
    // up with one `rusm serve`. `hosted` holds the resident supervisor alive.
    let hosted =
        spawn_components(Path::new("."), &wasm, &cfg.components, &cfg.capabilities).await?;
    let endpoints = serve_apps(
        Path::new("."),
        &wasm,
        &cfg.serve,
        &cfg.components,
        &cfg.capabilities,
    )
    .await?;
    if endpoints.is_empty() && hosted.is_empty() {
        println!("no [[serve]] entries or [components] in rusm.toml — nothing to do");
        return Ok(());
    }
    if !hosted.is_empty() {
        print_hosted(&hosted);
    }
    println!("serving {} endpoint(s):", endpoints.len());
    for ep in &endpoints {
        let scheme = if ep.protocol.is_http() { "http" } else { "ws" };
        println!("  {:<16} {scheme}://{}", ep.name, ep.addr);
    }
    println!("press Ctrl-C to stop");
    tokio::signal::ctrl_c().await?;
    println!("\nstopping {} process(es)…", rt.shutdown());
    Ok(())
}

/// `rusm dev`: build, spawn, and **watch** `./components` — on any source change,
/// rebuild and reload the components (kill + respawn). Ctrl-C stops. Watching is a
/// dependency-free mtime poll (a ~400 ms scan, skipping build output).
async fn dev(args: &[String]) -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = load_node_config(args, None, None);
    let rt = Runtime::new();
    let wasm = wasm_runtime(rt.clone(), &cfg)?;
    let root = Path::new(".");

    build_components(root)?;
    let mut hosted = spawn_components(root, &wasm, &cfg.components, &cfg.capabilities).await?;
    if hosted.is_empty() {
        println!("no [components] in rusm.toml — nothing to run");
        return Ok(());
    }
    print_hosted(&hosted);
    println!("watching ./components — edit to reload, Ctrl-C to stop");

    let components = root.join("components");
    let mut fingerprint = source_fingerprint(&components);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {
                let next = source_fingerprint(&components);
                if next == fingerprint {
                    continue;
                }
                fingerprint = next;
                println!("change detected — rebuilding…");
                // Tear down the resident supervisor + its services, then re-register
                // (which overwrites every component's factory) and re-boot residents.
                hosted.teardown(&rt);
                if let Err(error) = build_components(root) {
                    eprintln!("build failed: {error}");
                    continue;
                }
                match spawn_components(root, &wasm, &cfg.components, &cfg.capabilities).await {
                    Ok(reloaded) => {
                        hosted = reloaded;
                        print_hosted(&hosted);
                    }
                    Err(error) => eprintln!("reload failed: {error}"),
                }
            }
        }
    }
    println!("\nstopping {} process(es)…", rt.shutdown());
    Ok(())
}

/// A fingerprint of the source files under `dir` (sorted path + mtime pairs),
/// skipping build output (`target/`, `node_modules/`). Any source edit changes it.
fn source_fingerprint(dir: &Path) -> Vec<(std::path::PathBuf, std::time::SystemTime)> {
    fn walk(dir: &Path, out: &mut Vec<(std::path::PathBuf, std::time::SystemTime)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name();
                if name != "target" && name != "node_modules" {
                    walk(&path, out);
                }
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ts" | "rs" | "toml" | "js" | "json" | "wit")
            ) {
                if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                    out.push((path, modified));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// Builds every component under `<dir>/components/<name>/` into `<dir>/wasm/`.
/// Two kinds, auto-detected, one toolchain each (no jco, no cargo-component):
/// a **Rust** component (has `Cargo.toml`) builds with `cargo build --target
/// wasm32-wasip2 --release` → `wasm/<name>.wasm`; a **TypeScript** component
/// (has `index.ts`/`src/index.ts`) bundles with `bun build` → `wasm/<name>.js`,
/// run on the shared rquickjs js-runner. Returns the built component names.
/// (Shell-orchestration glue, hence it lives in `main`.)
fn build_components(dir: &Path) -> anyhow::Result<Vec<String>> {
    let components_dir = dir.join("components");
    let wasm_dir = dir.join("wasm");
    std::fs::create_dir_all(&wasm_dir)?;

    // If the app declares JS dependencies (e.g. the `rusm-ts` package), make sure
    // they're installed so a TS component's `import` resolves during bundling.
    if dir.join("package.json").is_file() && !dir.join("node_modules").is_dir() {
        let status = Command::new("bun")
            .arg("install")
            .current_dir(dir)
            .status()
            .with_context(|| "running bun install (is Bun installed? https://bun.sh)")?;
        if !status.success() {
            return Err(anyhow!("`bun install` failed"));
        }
    }

    let mut entries: Vec<_> = std::fs::read_dir(&components_dir)
        .with_context(|| format!("reading {}", components_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut built = Vec::new();
    for entry in entries {
        let crate_dir = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if crate_dir.join("Cargo.toml").is_file() {
            build_rust_component(&crate_dir, &name, &wasm_dir)?;
            built.push(name);
        } else if let Some(ts_entry) = ts_entrypoint(&crate_dir) {
            build_ts_component(&ts_entry, &name, &wasm_dir)?;
            built.push(name);
        }
        // A dir that is neither a Rust crate nor a TS component is skipped.
    }
    Ok(built)
}

/// Builds one Rust component crate to `wasm/<name>.wasm` via `cargo build
/// --target wasm32-wasip2 --release` (which componentizes).
fn build_rust_component(crate_dir: &Path, name: &str, wasm_dir: &Path) -> anyhow::Result<()> {
    let status = Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip2", "--release"])
        .current_dir(crate_dir)
        .status()
        .with_context(|| "running cargo (is the wasm32-wasip2 target installed?)")?;
    if !status.success() {
        return Err(anyhow!("`cargo build` failed for component `{name}`"));
    }
    // Cargo names the artifact after the crate (dashes become underscores).
    let artifact = crate_dir
        .join("target/wasm32-wasip2/release")
        .join(format!("{}.wasm", name.replace('-', "_")));
    let dest = wasm_dir.join(format!("{name}.wasm"));
    std::fs::copy(&artifact, &dest)
        .with_context(|| format!("copying {} -> {}", artifact.display(), dest.display()))?;
    Ok(())
}

/// The TS entrypoint of a component dir, if any: `index.ts` or `src/index.ts`.
fn ts_entrypoint(crate_dir: &Path) -> Option<std::path::PathBuf> {
    [crate_dir.join("index.ts"), crate_dir.join("src/index.ts")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Bundles one TS component to `wasm/<name>.js` with `bun build`, in **CommonJS**
/// form (`--format=cjs`) so the runner sees its `export`s on `module.exports` — a
/// service component's functions, or a worker's `export default`. Targets `browser`
/// (no node/bun globals assumed); a bare script with no exports just runs.
///
/// Then **precompiles** the bundle to QuickJS bytecode → `wasm/<name>.qjsbc`
/// (version-locked to the js-runner via `rusm-jsc`), so the runner skips parsing at
/// load. The loader prefers the `.qjsbc`; the `.js` stays for debugging.
fn build_ts_component(entry: &Path, name: &str, wasm_dir: &Path) -> anyhow::Result<()> {
    let dest = wasm_dir.join(format!("{name}.js"));
    let status = Command::new("bun")
        .args([
            "build",
            "--target=browser",
            "--format=cjs",
            "--minify",
            "--outfile",
        ])
        .arg(&dest)
        .arg(entry)
        .status()
        .with_context(|| "running bun (is Bun installed? https://bun.sh)")?;
    if !status.success() {
        return Err(anyhow!("`bun build` failed for component `{name}`"));
    }
    // Precompile to QuickJS bytecode (skip the parser at runtime). A compile error
    // here is non-fatal: drop the stale .qjsbc so the loader falls back to source.
    let source = std::fs::read_to_string(&dest)
        .with_context(|| format!("reading bundled {}", dest.display()))?;
    let bc_path = wasm_dir.join(format!("{name}.qjsbc"));
    match rusm_jsc::compile(&source) {
        Ok(bytecode) => std::fs::write(&bc_path, bytecode)
            .with_context(|| format!("writing {}", bc_path.display()))?,
        Err(error) => {
            eprintln!("warning: bytecode precompile failed for `{name}` ({error}); using source");
            let _ = std::fs::remove_file(&bc_path);
        }
    }
    Ok(())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == name)?;
    args.get(idx + 1).cloned()
}

/// Loads node config: defaults → `rusm.toml` (or `--config <file>`) → CLI flags.
fn load_node_config(args: &[String], config: Option<&str>, listen: Option<&str>) -> NodeConfig {
    let explicit = config.map(String::from).or_else(|| flag(args, "--config"));
    let path = explicit.clone().unwrap_or_else(|| "rusm.toml".to_string());
    let mut cfg = NodeConfig::load(Path::new(&path), explicit.is_some()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    if let Some(listen_addr) = listen {
        cfg.node.listen = listen_addr.to_string();
    } else if let Some(listen_value) = flag(args, "--listen") {
        cfg.node.listen = listen_value;
    }
    cfg
}

/// Build the Wasm runtime for an app, opening the configured durable key-value
/// store (`store = "..."` in rusm.toml, relative to the app dir) when set — so
/// components granted `storage` can persist; otherwise a store-less runtime. The
/// store's parent dir is created so a fresh app's first run doesn't trip on it.
fn wasm_runtime(rt: Runtime, cfg: &NodeConfig) -> anyhow::Result<WasmRuntime> {
    let wasm = match &cfg.node.store {
        Some(rel) => {
            let path = Path::new(".").join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            WasmRuntime::with_store(rt, &path)?
        }
        None => WasmRuntime::new(rt)?,
    };
    // Platform lifecycle logging: explicit, off by default — declared via `[log] level`.
    wasm.set_log_level(cfg.log_level());
    Ok(wasm)
}

async fn attach(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (ws, _) = tokio_tungstenite::connect_async(url).await?;
    let (mut write, mut read) = ws.split();
    println!("attached to {url} — type `help` for commands");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            incoming = read.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(message) = ServerMessage::from_json(text.as_str()) {
                        println!("{}", render_message(&message));
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    println!("node disconnected");
                    break;
                }
                _ => {}
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => match parse(&line) {
                    ReplInput::Command(cmd) => send(&mut write, &cmd).await?,
                    ReplInput::Help => println!("{HELP}"),
                    ReplInput::Quit => break,
                    ReplInput::Empty => {}
                    ReplInput::Unknown(msg) => println!("{msg}"),
                },
                _ => break,
            },
        }
    }
    Ok(())
}

async fn send<S>(write: &mut S, command: &ClientCommand) -> Result<(), Box<dyn std::error::Error>>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::error::Error + 'static,
{
    write.send(Message::Text(command.to_json().into())).await?;
    Ok(())
}
