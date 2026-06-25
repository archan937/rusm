use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use futures_util::{SinkExt, StreamExt};
use pico_args::Arguments;
use rusm_cli::{
    capabilities_for, command_help, exec_kv, generate_bridge, generate_component, host,
    node_overrides, normalize_target, parse, parse_generate_args, parse_kv, parse_new_args,
    prebuilt_wasm, render_message, scaffold, spawn_components, usage, version, wants_help,
    wants_version, GenerateCommand, KvCommand, KvOutput, Protocol, ReplInput, WasmReplHost,
    DEFAULT_HOST, HELP,
};
use rusm_node::{serve, ClientCommand, Node, NodeConfig, ServerMessage};
use rusm_otp::Runtime;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let mut args = Arguments::from_env();

    // `--version` / `-V` is global: handle it before anything else, so it works with or
    // without a command (`rusm -V`, `rusm serve --version`).
    if wants_version(&mut args) {
        println!("{}", version());
        return;
    }

    let command = args
        .subcommand()
        .unwrap_or_else(|error| die(format!("error: {error}"), 2));

    // `rusm` / `rusm help` / `rusm --help` → the top-level help; a recognised command
    // followed by `--help`/`-h` → that command's help. Both are handled once, here, so the
    // command bodies below stay free of help plumbing. Requested help goes to stdout (exit
    // 0); only misuse (unknown command) prints to stderr.
    match command.as_deref() {
        None | Some("help") => print_help(),
        Some(name) if wants_help(&mut args) => match command_help(name) {
            Some(help) => println!("{help}"),
            None => unknown_command(name),
        },
        Some("new") => cmd_new(args),
        Some("generate") => cmd_generate(args),
        Some("build") => cmd_build(),
        Some("node") => cmd_node(args).await,
        Some("run") => cmd_run(args).await,
        Some("serve") => cmd_serve(args).await,
        Some("dev") => cmd_dev(args).await,
        Some("kv") => cmd_kv(args),
        Some("attach") => cmd_attach(args).await,
        Some(other) => unknown_command(other),
    }
}

/// Print `message` to stderr and exit: code 2 for usage/argument errors (CLI
/// misuse), 1 for operational failures.
fn die(message: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("{message}");
    std::process::exit(code);
}

/// Print the top-level help to stdout and exit 0 — the response to a *requested* help
/// (`rusm`, `rusm help`, `rusm --help`), so it can be piped without mixing into stderr.
fn print_help() -> ! {
    print!("{}", usage());
    std::process::exit(0);
}

/// Print the top-level usage to stderr and exit with `code` — for misuse (an unknown
/// command), where help is a diagnostic, not the requested output.
fn die_usage(code: i32) -> ! {
    eprint!("{}", usage());
    std::process::exit(code);
}

/// Report an unrecognised command, then the usage, and exit (code 2).
fn unknown_command(name: &str) -> ! {
    eprintln!("unknown command `{name}`\n");
    die_usage(2);
}

/// `rusm new <name>`: scaffold an app, then print the next-steps hint.
fn cmd_new(args: Arguments) {
    let app = parse_new_args(args).unwrap_or_else(|error| die(error, 2));
    if let Err(error) = scaffold(Path::new("."), &app) {
        die(format!("new failed: {error}"), 1);
    }
    let probe = if app.bridges {
        // The bridge starter routes `GET /forecast/:city` to a handler that calls the bridge.
        "curl http://127.0.0.1:8080/forecast/Amsterdam"
    } else {
        match app.protocol {
            Protocol::Http => "curl http://127.0.0.1:8080/",
            Protocol::Sse => "curl -N http://127.0.0.1:8080/",
            Protocol::Ws => "websocat ws://127.0.0.1:8080/",
        }
    };
    println!("created {}/", app.name);
    println!("\nnext:");
    println!("  cd {}", app.name);
    println!("  rusm build      # compile components/ -> wasm/");
    println!("  rusm serve      # http://127.0.0.1:8080");
    println!("  {probe}");
}

/// `rusm generate component|bridge`: add to an existing project.
fn cmd_generate(args: Arguments) {
    let root = Path::new(".");
    match parse_generate_args(args).unwrap_or_else(|e| die(e, 2)) {
        GenerateCommand::Component(gen) => {
            let created = generate_component(root, &gen)
                .unwrap_or_else(|e| die(format!("generate failed: {e}"), 1));
            println!("added component {}/", gen.name);
            for f in &created {
                println!("  {}", f.display());
            }
            let probe = match gen.protocol {
                Protocol::Http => "curl http://127.0.0.1:8080/".to_string(),
                Protocol::Sse => "curl -N http://127.0.0.1:8080/".to_string(),
                Protocol::Ws => "websocat ws://127.0.0.1:8080/".to_string(),
            };
            println!("\nnext:");
            println!("  rusm build");
            println!("  rusm serve      # then: {probe}");
        }
        GenerateCommand::Bridge(gen) => {
            let created = generate_bridge(root, &gen)
                .unwrap_or_else(|e| die(format!("generate failed: {e}"), 1));
            println!("added bridge {}/", gen.name);
            for f in &created {
                println!("  {}", f.display());
            }
            println!("\nTo use this bridge, add to rusm.toml:");
            println!("  [capabilities.my-cap]");
            println!("  inherits = \"sandboxed\"");
            println!("  bridges = [\"{}\"]", gen.name);
            println!();
            println!("Then set capability = \"my-cap\" on the component(s) that call it.");
            println!("Run `rusm build` to regenerate the glue.");
        }
    }
}

/// `rusm build`: compile every `./components/*` crate to `./wasm`, and — for a
/// **custom-bridge app** — regenerate the host glue and compile the host binary.
fn cmd_build() {
    if let Err(error) = build_all(Path::new(".")) {
        die(format!("build failed: {error}"), 1);
    }
}

/// The full `rusm build`: (1) regenerate any custom-bridge host glue from `bridges/<name>/`
/// (so a guest that imports a bridge and the host crate both build against fresh, in-sync
/// generated code), (2) compile the components, (3) for a custom-bridge app, compile the
/// host binary (which has the bridge impls compiled in). A pure-guest app does only (2).
fn build_all(root: &Path) -> anyhow::Result<()> {
    let bridges = rusm_cli::bridges::generate_host_files(root)?;
    if !bridges.is_empty() {
        let names: Vec<&str> = bridges.iter().map(|b| b.name.as_str()).collect();
        println!("custom bridge(s): {}", names.join(", "));
    }
    let built = build_components(root, &bridges)?;
    if built.is_empty() {
        println!("no component crates found under ./components");
    } else {
        println!(
            "built {} component(s) -> ./wasm: {}",
            built.len(),
            built.join(", ")
        );
    }
    if !bridges.is_empty() {
        build_js_runner_if_ts_uses_bridges(root, &bridges)?;
        build_ts_bridge_runners(root, &bridges)?;
        build_go_bridge_runners(root, &bridges)?;
        build_host_crate(root)?;
        println!("built host binary (custom bridges compiled in)");
    }
    Ok(())
}

/// Bundle each **TS-hosted** bridge runner (`bridges/<name>/_runner.ts`) into
/// `wasm/bridge-<name>.js` with Bun. Skipped for Rust-only and Go-only bridge apps.
fn build_ts_bridge_runners(
    root: &Path,
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> anyhow::Result<()> {
    use rusm_cli::bridges::HostImpl;
    let wasm_dir = root.join("wasm");
    std::fs::create_dir_all(&wasm_dir)?;
    for bridge in bridges
        .iter()
        .filter(|b| matches!(b.host_impl, HostImpl::TypeScript(_)))
    {
        let runner_ts = bridge.dir.join("_runner.ts");
        let bundle_name = format!("bridge-{}.js", bridge.name);
        let dest = wasm_dir.join(&bundle_name);
        let status = Command::new("bun")
            .args([
                "build",
                "--target=browser",
                "--format=cjs",
                "--minify",
                "--outfile",
            ])
            .arg(&dest)
            .arg(&runner_ts)
            .status()
            .with_context(|| format!("bundling TS bridge runner `{}`", bridge.name))?;
        if !status.success() {
            return Err(anyhow!(
                "`bun build` failed for bridge runner `{}`",
                bridge.name
            ));
        }
        println!("built bridge runner -> {}", dest.display());
    }
    Ok(())
}

/// Compile each **Go-hosted** bridge runner (`bridges/<name>/_runner.go` + `host.go`) to
/// `wasm/bridge-<name>.wasm` with TinyGo. The runner + user's host.go share `package main` in
/// the same directory, so TinyGo sees both. Skipped for Rust/TS bridge apps.
fn build_go_bridge_runners(
    root: &Path,
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> anyhow::Result<()> {
    use rusm_cli::bridges::HostImpl;
    let wasm_dir = root.join("wasm");
    std::fs::create_dir_all(&wasm_dir)?;
    for bridge in bridges
        .iter()
        .filter(|b| matches!(b.host_impl, HostImpl::Go(_)))
    {
        let bridge_dir = &bridge.dir;
        let name = &bridge.name;
        // go mod tidy to fetch the rusm-go SDK into the module cache.
        let status = Command::new("go")
            .args(["mod", "tidy"])
            .current_dir(bridge_dir)
            .status()
            .with_context(|| format!("running `go mod tidy` for bridge `{name}`"))?;
        if !status.success() {
            return Err(anyhow!("`go mod tidy` failed for bridge `{name}`"));
        }
        // Locate the rusm-go SDK's `wit/` (TinyGo needs -wit-package for the actor ABI).
        let sdk_wit = go_sdk_wit(bridge_dir)
            .with_context(|| format!("locating rusm-go SDK for bridge `{name}`"))?;
        let dest = std::fs::canonicalize(&wasm_dir)
            .with_context(|| format!("resolving {}", wasm_dir.display()))?
            .join(format!("bridge-{name}.wasm"));
        let status = Command::new("tinygo")
            .args([
                "build",
                "-target=wasip2",
                "-no-debug",
                "-panic=trap",
                "-opt=z",
            ])
            .arg("-wit-package")
            .arg(&sdk_wit)
            .args(["-wit-world", "component", "-o"])
            .arg(&dest)
            .arg(".")
            .current_dir(bridge_dir)
            .status()
            .with_context(|| format!("running tinygo for bridge `{name}`"))?;
        if !status.success() {
            return Err(anyhow!("`tinygo build` failed for bridge `{name}`"));
        }
        println!("built bridge runner -> {}", dest.display());
    }
    Ok(())
}

/// If any **TS** component is granted a custom bridge, rebuild the js-runner with every
/// bridge's typed import + glue compiled in (a TS guest's actor/service/WS runner), and write
/// it to `wasm/js_runner.wasm` for `host::build_runtime` to load. Skipped when no TS guest
/// uses a bridge — Rust/Go guests need no runner (the slow build only runs when it must).
fn build_js_runner_if_ts_uses_bridges(
    root: &Path,
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> anyhow::Result<()> {
    let granted = granted_bridges(bridges);
    let ts_uses_a_bridge = granted.iter().any(|(name, specs)| {
        !specs.is_empty() && ts_entrypoint(&root.join("components").join(name)).is_some()
    });
    if !ts_uses_a_bridge {
        return Ok(());
    }
    // Ambient TS types so the guest calls the bridge typed (`/// <reference>` it).
    let dts = root.join("bridges.d.ts");
    std::fs::write(&dts, rusm_cli::bridges::gen_bridge_dts(bridges)?)
        .with_context(|| format!("writing {}", dts.display()))?;
    println!("building js-runner with custom bridges (TS guest) — first build compiles QuickJS…");
    let wasm = rusm_cli::jsbuild::build_app_js_runner(root, bridges)?;
    let dest = root.join("wasm/js_runner.wasm");
    std::fs::write(&dest, wasm).with_context(|| format!("writing {}", dest.display()))?;
    println!("built {} (custom bridges compiled in)", dest.display());
    Ok(())
}

/// Resolve, per component name, the custom bridges its capability profile **grants** (the
/// default-deny whitelist). A wit-based guest (Rust/Go) gets these vendored into its build so
/// it can `import` them; the map is empty when the app declares no bridges. Each builder
/// looks up its component's slice and does the language-appropriate WIT setup.
fn granted_bridges(
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> std::collections::HashMap<String, Vec<rusm_cli::bridges::BridgeSpec>> {
    if bridges.is_empty() {
        return std::collections::HashMap::new();
    }
    let cfg = load_node_config(None, None);
    let by_name: std::collections::HashMap<&str, &rusm_cli::bridges::BridgeSpec> =
        bridges.iter().map(|b| (b.name.as_str(), b)).collect();
    cfg.components
        .iter()
        .map(|(name, comp)| {
            let caps = capabilities_for(&comp.capability, &cfg.capabilities);
            let specs = caps
                .granted_bridges()
                .filter_map(|g| by_name.get(g).map(|b| (*b).clone()))
                .collect();
            (name.clone(), specs)
        })
        .collect()
}

/// Compile a custom-bridge app's **host binary** — the app's own crate at `root`, with its
/// bridge impls compiled in (`cargo build --release` in the app dir).
fn build_host_crate(root: &Path) -> anyhow::Result<()> {
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(root)
        .status()
        .with_context(|| "running cargo build for the host binary")?;
    if !status.success() {
        return Err(anyhow!("`cargo build --release` failed for the host crate"));
    }
    Ok(())
}

/// `rusm node start`: `start` is the only subcommand. Host the app and expose the
/// live attach endpoint.
async fn cmd_node(mut args: Arguments) {
    if args.subcommand().ok().flatten().as_deref() != Some("start") {
        die(command_help("node").expect("node is a command"), 2);
    }
    let ov = node_overrides(&mut args).unwrap_or_else(|error| die(error, 2));
    if let Err(error) = start_node(ov.config.as_deref(), ov.listen.as_deref()).await {
        die(format!("node start failed: {error}"), 1);
    }
}

/// `rusm run`: run the app's resident + on-demand components.
async fn cmd_run(mut args: Arguments) {
    let ov = node_overrides(&mut args).unwrap_or_else(|error| die(error, 2));
    if let Err(error) = run_app(ov.config.as_deref(), ov.listen.as_deref()).await {
        die(format!("run failed: {error}"), 1);
    }
}

/// `rusm serve`: host the app's `[[serve]]` listeners on their ports.
async fn cmd_serve(mut args: Arguments) {
    let ov = node_overrides(&mut args).unwrap_or_else(|error| die(error, 2));
    if let Err(error) = serve_app(ov.config.as_deref(), ov.listen.as_deref()).await {
        die(format!("serve failed: {error}"), 1);
    }
}

/// `rusm dev`: build + run, watching `./components` for edits.
async fn cmd_dev(mut args: Arguments) {
    let ov = node_overrides(&mut args).unwrap_or_else(|error| die(error, 2));
    if let Err(error) = dev(ov.config.as_deref(), ov.listen.as_deref()).await {
        die(format!("dev failed: {error}"), 1);
    }
}

/// `rusm kv <action> …`: read/write the node's durable store from the shell — chiefly to
/// publish a dynamic `kv:` bundle. Parses the action + operands, then runs it against the
/// configured store.
fn cmd_kv(mut args: Arguments) {
    let action = match args.subcommand() {
        Ok(Some(action)) => action,
        _ => die(command_help("kv").expect("kv is a command"), 2),
    };
    let operands: Vec<String> = args
        .finish()
        .into_iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let command =
        parse_kv(&action, &operands).unwrap_or_else(|error| die(format!("error: {error}"), 2));
    if let Err(error) = run_kv(command) {
        die(format!("kv failed: {error}"), 1);
    }
}

/// Open the configured `[node] store` and run a parsed kv command, emitting its output.
/// Opens the store file directly, so the node must be stopped (redb is single-writer).
fn run_kv(command: KvCommand) -> anyhow::Result<()> {
    let cfg = load_node_config(None, None);
    let rel = cfg.node.store.ok_or_else(|| {
        anyhow!("no durable store configured — set `store` in the [node] table of rusm.toml")
    })?;
    let path = Path::new(".").join(&rel);
    let store = rusm_kv::Store::open(&path).with_context(|| {
        format!(
            "opening the store at {} (is a node still running? it holds the lock)",
            path.display()
        )
    })?;
    match exec_kv(&store, command)? {
        KvOutput::Message(message) => println!("{message}"),
        KvOutput::Bytes(bytes) => {
            use std::io::Write as _;
            std::io::stdout()
                .write_all(&bytes)
                .context("writing bytes to stdout")?;
        }
    }
    Ok(())
}

/// `rusm attach [target]`: connect the REPL/observer to a node (default: local).
async fn cmd_attach(mut args: Arguments) {
    let target: Option<String> = args.opt_free_from_str().ok().flatten();
    let target = normalize_target(target.as_deref().unwrap_or(DEFAULT_HOST));
    if let Err(error) = attach(&target).await {
        die(format!("attach failed: {error}"), 1);
    }
}

/// `rusm node start`: host the app's `[components.<name>]` (like `rusm run`) and expose
/// a live **attach** endpoint on `[node] listen`, so `rusm attach` can observe the
/// node's processes. The served runtime + held handles keep everything alive for
/// the lifetime of the server (which runs until Ctrl-C or a bind error).
async fn start_node(config: Option<&str>, listen: Option<&str>) -> anyhow::Result<()> {
    let cfg = load_node_config(config, listen);
    let rt = Runtime::new();
    // `wasm` + `hosted` stay bound for the whole function: they own the hosted
    // components' runtime + resident supervisor, so they must outlive the server below.
    // `wasm` is shared (Arc) with the REPL host, which spawns eval sessions on it.
    let wasm = Arc::new(host::build_runtime(rt.clone(), &cfg, |_| Ok(()))?);
    let hosted =
        spawn_components(Path::new("."), &wasm, &cfg.components, &cfg.capabilities).await?;
    // The live JS shell behind `rusm attach`: eval is gated to loopback clients by the
    // node, so wiring it in is safe by default for a locally-started node.
    let repl = Arc::new(WasmReplHost::new(Arc::clone(&wasm), rt.clone()));
    let node = Node::with_repl(rt.clone(), node_name(), cfg.node.ticks_per_second, repl);
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

/// Runs the app's components: register each `[components.<name>]` entry from `./wasm`
/// under its capability profile (booting + supervising the resident ones), and wait for
/// Ctrl-C (`.env` is loaded in `host::build_runtime`). `wasm` + `hosted` keep the
/// processes alive.
async fn run_app(config: Option<&str>, listen: Option<&str>) -> anyhow::Result<()> {
    let cfg = load_node_config(config, listen);
    let rt = Runtime::new();
    let wasm = host::build_runtime(rt.clone(), &cfg, |_| Ok(()))?;
    let hosted =
        spawn_components(Path::new("."), &wasm, &cfg.components, &cfg.capabilities).await?;
    if hosted.is_empty() {
        println!("no [components] in rusm.toml — nothing to run");
        return Ok(());
    }
    host::print_hosted(&hosted);
    println!("press Ctrl-C to stop");
    tokio::signal::ctrl_c().await?;
    println!("\nstopping {} process(es)…", rt.shutdown());
    Ok(())
}

/// `rusm serve`: host each `[[serve]]` component as a real network server on its
/// own port (HTTP/SSE or WebSocket), then wait for Ctrl-C. The bound runtime + the
/// accept-loop tasks keep the servers up. This is the *server* side of a fair
/// benchmark: the node only serves; load is driven out-of-process (`rusm-loadtest`).
async fn serve_app(config: Option<&str>, listen: Option<&str>) -> anyhow::Result<()> {
    let root = Path::new(".");
    // A custom-bridge app serves via its OWN host binary, which has the bridge impls
    // compiled in (the prebuilt `rusm` can't host them). Run it — `rusm build` produced it.
    if rusm_cli::bridges::has_bridges(root) {
        return run_host_binary(root);
    }
    let cfg = load_node_config(config, listen);
    host::serve(root, &cfg, |_| Ok(())).await
}

/// Run a custom-bridge app's host binary — it registers the app's bridges and serves the
/// manifest via the same `host::serve` loop. `cargo run --release` so a stale binary
/// rebuilds first; it blocks until the host exits (Ctrl-C propagates to the whole group).
fn run_host_binary(root: &Path) -> anyhow::Result<()> {
    let status = Command::new("cargo")
        .args(["run", "--release"])
        .current_dir(root)
        .status()
        .with_context(|| "running the host binary (did `rusm build` succeed?)")?;
    if !status.success() {
        return Err(anyhow!("the host binary exited with an error"));
    }
    Ok(())
}

/// `rusm dev`: build, spawn, and **watch** `./components` — on any source change,
/// rebuild and reload the components (kill + respawn). Ctrl-C stops. Watching is a
/// dependency-free mtime poll (a ~400 ms scan, skipping build output).
async fn dev(config: Option<&str>, listen: Option<&str>) -> anyhow::Result<()> {
    let cfg = load_node_config(config, listen);
    let rt = Runtime::new();
    let wasm = host::build_runtime(rt.clone(), &cfg, |_| Ok(()))?;
    let root = Path::new(".");
    // Guest WIT for any custom bridges so components build; dev hosts via the prebuilt
    // runtime (no compiled-in bridge impls), so a bridge app's full flow is `build` + `serve`.
    let bridges = rusm_cli::bridges::discover(root)?;

    build_components(root, &bridges)?;
    let mut hosted = spawn_components(root, &wasm, &cfg.components, &cfg.capabilities).await?;
    if hosted.is_empty() {
        println!("no [components] in rusm.toml — nothing to run");
        return Ok(());
    }
    host::print_hosted(&hosted);
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
                if let Err(error) = build_components(root, &bridges) {
                    eprintln!("build failed: {error}");
                    continue;
                }
                match spawn_components(root, &wasm, &cfg.components, &cfg.capabilities).await {
                    Ok(reloaded) => {
                        hosted = reloaded;
                        host::print_hosted(&hosted);
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
fn build_components(
    dir: &Path,
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> anyhow::Result<Vec<String>> {
    let components_dir = dir.join("components");
    let wasm_dir = dir.join("wasm");
    std::fs::create_dir_all(&wasm_dir)?;
    // Per-component custom-bridge grants (empty unless the app declares bridges); each
    // wit-based guest gets the bridges its profile grants wired into its build.
    let granted = granted_bridges(bridges);
    let none: Vec<rusm_cli::bridges::BridgeSpec> = Vec::new();

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
        let component_bridges = granted.get(&name).unwrap_or(&none);
        if crate_dir.join("Cargo.toml").is_file() {
            build_rust_component(&crate_dir, &name, &wasm_dir, component_bridges)?;
            built.push(name);
        } else if crate_dir.join("go.mod").is_file() {
            build_go_component(&crate_dir, &name, &wasm_dir, component_bridges)?;
            built.push(name);
        } else if let Some(ts_entry) = ts_entrypoint(&crate_dir) {
            build_ts_component(&ts_entry, &name, &wasm_dir)?;
            built.push(name);
        } else if let Some(wasm_file) = prebuilt_wasm(&crate_dir, &name)? {
            // A user-supplied, pre-built wasip2 component (the `generic` scaffold) —
            // copied into wasm/ as-is; its interface is the operator's contract.
            copy_prebuilt_wasm(&wasm_file, &name, &wasm_dir)?;
            built.push(name);
        }
        // A dir with no recognized component type is skipped.
    }
    Ok(built)
}

/// Builds one Rust component crate to `wasm/<name>.wasm` via `cargo build
/// --target wasm32-wasip2 --release` (which componentizes). If the component is granted
/// custom bridges, its `wit/` is generated first so a `#[handlers(bridge=…)]`/`generate!`
/// guest resolves the import.
fn build_rust_component(
    crate_dir: &Path,
    name: &str,
    wasm_dir: &Path,
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> anyhow::Result<()> {
    rusm_cli::bridges::generate_guest_wit(crate_dir, bridges)?;
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

/// The Go module path of the rusm-go guest SDK — its `wit/` (the `component` world plus
/// vendored WASI) is what TinyGo embeds. Resolved at build time wherever the module
/// lives (a `replace` path in dev, the module cache in a published app).
const RUSM_GO_SDK: &str = "github.com/archan937/rusm/packages/rusm-go";

/// Builds one Go component (a dir with `go.mod`) to `wasm/<name>.wasm` with TinyGo.
/// TinyGo compiles straight to a `wasm32-wasip2` component, embedding the rusm-go SDK's
/// `component` world. `-no-debug` strips DWARF, `-panic=trap` makes a Go panic a wasm
/// trap (→ process Crashed, RUSM's crash model), `-opt=z` optimizes for size.
fn build_go_component(
    crate_dir: &Path,
    name: &str,
    wasm_dir: &Path,
    bridges: &[rusm_cli::bridges::BridgeSpec],
) -> anyhow::Result<()> {
    // Resolve the component's module deps (the rusm-go SDK + its transitive cm) so
    // `go list` below and TinyGo build on a fresh checkout — the Go analog of the
    // `bun install` the TS path runs. `tidy` (not just `download`) is needed to populate
    // go.sum for transitive deps reached through a local `replace`.
    let status = Command::new("go")
        .args(["mod", "tidy"])
        .current_dir(crate_dir)
        .status()
        .with_context(|| "running go (is Go installed? https://go.dev)")?;
    if !status.success() {
        return Err(anyhow!("`go mod tidy` failed for component `{name}`"));
    }
    // With custom bridges, embed a per-component WIT (the SDK's WIT + the granted bridges,
    // a `component` world for TinyGo) and generate the bridges' Go bindings into
    // `internal/wit` from the `bridges` world (rusm:runtime stays the SDK's); otherwise embed
    // the SDK's WIT directly.
    let wit = if bridges.is_empty() {
        go_sdk_wit(crate_dir)?
    } else {
        let sdk_wit = go_sdk_wit(crate_dir)?;
        rusm_cli::bridges::generate_go_guest_wit(crate_dir, bridges, &sdk_wit)?;
        let status = Command::new("wit-bindgen-go")
            .args([
                "generate",
                "--world",
                "bridges",
                "--out",
                "internal/wit",
                "wit",
            ])
            .current_dir(crate_dir)
            .status()
            .with_context(|| "running wit-bindgen-go (mise-managed; in go.mod tool deps?)")?;
        if !status.success() {
            return Err(anyhow!("`wit-bindgen-go` failed for component `{name}`"));
        }
        // Absolute, because TinyGo runs with `current_dir(crate_dir)` (as `go_sdk_wit` is).
        std::fs::canonicalize(crate_dir.join("wit"))
            .with_context(|| "resolving the generated per-component wit/")?
    };
    // TinyGo runs in crate_dir, so its `-o` path must be absolute to land in the app's
    // wasm/ (canonicalize is safe — build_components already created wasm_dir).
    let dest = std::fs::canonicalize(wasm_dir)
        .with_context(|| format!("resolving {}", wasm_dir.display()))?
        .join(format!("{name}.wasm"));
    let status = Command::new("tinygo")
        .args([
            "build",
            "-target=wasip2",
            "-no-debug",
            "-panic=trap",
            "-opt=z",
        ])
        .arg("-wit-package")
        .arg(&wit)
        .args(["-wit-world", "component", "-o"])
        .arg(&dest)
        .arg(".")
        .current_dir(crate_dir)
        .status()
        .with_context(|| "running tinygo (is TinyGo installed? https://tinygo.org)")?;
    if !status.success() {
        return Err(anyhow!("`tinygo build` failed for component `{name}`"));
    }
    Ok(())
}

/// Locates the rusm-go SDK's `wit/` directory via `go list -m`, so TinyGo's
/// `-wit-package` points at it regardless of where the module resolves.
fn go_sdk_wit(crate_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let out = Command::new("go")
        .args(["list", "-m", "-f", "{{.Dir}}", RUSM_GO_SDK])
        .current_dir(crate_dir)
        .output()
        .with_context(|| "running go (is Go installed? https://go.dev)")?;
    if !out.status.success() {
        return Err(anyhow!(
            "could not locate the rusm-go SDK ({RUSM_GO_SDK}) — is it required in go.mod? \
             try `go mod download` in the component dir"
        ));
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(dir).join("wit"))
}

/// The TS entrypoint of a component dir, if any: `index.ts` or `src/index.ts`.
fn ts_entrypoint(crate_dir: &Path) -> Option<std::path::PathBuf> {
    [crate_dir.join("index.ts"), crate_dir.join("src/index.ts")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Copies a pre-built `.wasm` into the `wasm/` output directory as `<name>.wasm`.
fn copy_prebuilt_wasm(wasm_file: &Path, name: &str, wasm_dir: &Path) -> anyhow::Result<()> {
    let dest = wasm_dir.join(format!("{name}.wasm"));
    std::fs::copy(wasm_file, &dest)
        .with_context(|| format!("copying {} -> {}", wasm_file.display(), dest.display()))?;
    Ok(())
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

/// Load node config: defaults → `rusm.toml` (or `--config <file>`) → a `--listen`
/// override. The flags are already parsed by pico-args (the sole arg parser).
fn load_node_config(config: Option<&str>, listen: Option<&str>) -> NodeConfig {
    let path = config.unwrap_or("rusm.toml");
    let mut cfg = NodeConfig::load(Path::new(path), config.is_some()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    if let Some(listen) = listen {
        cfg.node.listen = listen.to_string();
    }
    cfg
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
                        // A bare statement (e.g. `const p = …`) renders empty — don't
                        // print a blank line for it.
                        let line = render_message(&message);
                        if !line.is_empty() {
                            println!("{line}");
                        }
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
