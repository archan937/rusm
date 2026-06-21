use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context};
use futures_util::{SinkExt, StreamExt};
use pico_args::Arguments;
use rusm_cli::{
    command_help, host, node_overrides, normalize_target, parse, parse_new_args, prebuilt_wasm,
    render_message, scaffold, spawn_components, usage, version, wants_help, wants_version,
    Protocol, ReplInput, DEFAULT_HOST, HELP,
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
        Some("build") => cmd_build(),
        Some("node") => cmd_node(args).await,
        Some("run") => cmd_run(args).await,
        Some("serve") => cmd_serve(args).await,
        Some("dev") => cmd_dev(args).await,
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

/// `rusm build`: compile every `./components/*` crate to `./wasm`.
fn cmd_build() {
    match build_components(Path::new(".")) {
        Ok(built) if built.is_empty() => println!("no component crates found under ./components"),
        Ok(built) => println!(
            "built {} component(s) -> ./wasm: {}",
            built.len(),
            built.join(", ")
        ),
        Err(error) => die(format!("build failed: {error}"), 1),
    }
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
    dotenvy::dotenv().ok();
    let cfg = load_node_config(config, listen);
    let rt = Runtime::new();
    // `wasm` + `hosted` stay bound for the whole function: they own the hosted
    // components' runtime + resident supervisor, so they must outlive the server below.
    let wasm = host::build_runtime(rt.clone(), &cfg, |_| Ok(()))?;
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
async fn run_app(config: Option<&str>, listen: Option<&str>) -> anyhow::Result<()> {
    // Environment variables the Rust way: process env first, then ./.env.
    dotenvy::dotenv().ok();

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
    // Env the Rust way: process env first, then ./.env.
    dotenvy::dotenv().ok();
    let cfg = load_node_config(config, listen);
    // The bare CLI wires no custom bridges; an app that needs them runs its own generated
    // host crate, which calls `host::serve` with its `add_to_linker` extension instead.
    host::serve(Path::new("."), &cfg, |_| Ok(())).await
}

/// `rusm dev`: build, spawn, and **watch** `./components` — on any source change,
/// rebuild and reload the components (kill + respawn). Ctrl-C stops. Watching is a
/// dependency-free mtime poll (a ~400 ms scan, skipping build output).
async fn dev(config: Option<&str>, listen: Option<&str>) -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = load_node_config(config, listen);
    let rt = Runtime::new();
    let wasm = host::build_runtime(rt.clone(), &cfg, |_| Ok(()))?;
    let root = Path::new(".");

    build_components(root)?;
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
                if let Err(error) = build_components(root) {
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
        } else if crate_dir.join("go.mod").is_file() {
            build_go_component(&crate_dir, &name, &wasm_dir)?;
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

/// The Go module path of the rusm-go guest SDK — its `wit/` (the `component` world plus
/// vendored WASI) is what TinyGo embeds. Resolved at build time wherever the module
/// lives (a `replace` path in dev, the module cache in a published app).
const RUSM_GO_SDK: &str = "github.com/archan937/rusm/packages/rusm-go";

/// Builds one Go component (a dir with `go.mod`) to `wasm/<name>.wasm` with TinyGo.
/// TinyGo compiles straight to a `wasm32-wasip2` component, embedding the rusm-go SDK's
/// `component` world. `-no-debug` strips DWARF, `-panic=trap` makes a Go panic a wasm
/// trap (→ process Crashed, RUSM's crash model), `-opt=z` optimizes for size.
fn build_go_component(crate_dir: &Path, name: &str, wasm_dir: &Path) -> anyhow::Result<()> {
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
    let wit = go_sdk_wit(crate_dir)?;
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
