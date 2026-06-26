//! The `rusm` command-line surface: one command table that backs the top-level help,
//! each command's `--help`, and `--version`, plus the shared pico-args helpers. Pure
//! logic, kept out of `main.rs` (thin I/O glue) so it is unit-tested.

use anyhow::Result;
use pico_args::Arguments;

/// One-line description of what `rusm` is, shown in the help header.
const TAGLINE: &str =
    "An Erlang-inspired WebAssembly runtime powered by Rust — isolated, supervised processes";

/// Where to send people for the long form.
const DOCS_URL: &str = "https://github.com/archan937/rusm";

/// One CLI command: its name, full invocation, a one-line summary (the overview), a
/// longer description and a few example invocations (`rusm <name> --help`).
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    details: &'static str,
    examples: &'static [&'static str],
}

/// Every `rusm` command — the single source of truth for all help text, ordered as the
/// app lifecycle reads (create → build → run/serve → operate), so the overview and the
/// per-command help can never drift apart.
const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "new",
        usage: "rusm new <name> [--rust] [--lang ts|rust|go|generic] \
                [--protocol http|sse|ws] [--template todo-board|weather] [--bridges]",
        summary: "scaffold a new RUSM app in ./<name>",
        details: "Creates ./<name> with a component, a rusm.toml, and a README. `--lang` \
                  picks the guest language (default TypeScript), `--protocol` the serving \
                  shape (default http). `--template todo-board` scaffolds the full \
                  five-component example app; `--template weather` scaffolds the custom-bridge \
                  example (a native `weather` host function called from a TS, Rust, or Go \
                  guest). `--bridges` adds a custom bridge to a new single-component app.",
        examples: &[
            "rusm new hello",
            "rusm new api --lang go --protocol ws",
            "rusm new board --template todo-board --lang rust",
            "rusm new forecast --template weather --lang ts",
        ],
    },
    CommandSpec {
        name: "build",
        usage: "rusm build",
        summary: "compile ./components/* -> ./wasm/*",
        details: "Compiles each ./components/<name>/ to ./wasm/ — cargo (wasm32-wasip2) for \
                  Rust, TinyGo for Go, Bun for TypeScript. Run it before `run` or `serve`.",
        examples: &["rusm build"],
    },
    CommandSpec {
        name: "run",
        usage: "rusm run",
        summary: "run ./wasm components per rusm.toml [components.<name>]",
        details: "Spawns each component declared in rusm.toml `[components.<name>]` as a \
                  supervised process — for non-serving apps (workers, services, CLIs).",
        examples: &["rusm run"],
    },
    CommandSpec {
        name: "dev",
        usage: "rusm dev",
        summary: "build + run, then watch ./components and reload on edits",
        details: "The fast inner loop: builds, runs, then watches ./components and rebuilds \
                  + reloads the changed component on every edit.",
        examples: &["rusm dev"],
    },
    CommandSpec {
        name: "serve",
        usage: "rusm serve",
        summary: "host ./wasm components as HTTP/WS/SSE servers per rusm.toml [[serve]]",
        details: "Hosts each rusm.toml `[[serve]]` listener on a real TCP port — a fresh \
                  sandboxed instance per HTTP/SSE request, one process per WebSocket \
                  connection. Routes come from each listener's `[serve.routes]`.",
        examples: &["rusm serve"],
    },
    CommandSpec {
        name: "node",
        usage: "rusm node start [--config <file>] [--listen <addr>]",
        summary: "host the app and expose a live attach endpoint",
        details: "Hosts the app as a long-lived node and exposes a live attach/observer \
                  endpoint (default ws://127.0.0.1:4000) for the dashboard or `rusm attach`.",
        examples: &["rusm node start", "rusm node start --listen 0.0.0.0:4000"],
    },
    CommandSpec {
        name: "kv",
        usage: "rusm kv <set|get|list|rm> …",
        summary: "read/write the node's durable store (publish kv: bundles)",
        details: "Reads and writes the node's durable key-value store (the `[node] store` \
                  file) from the shell — chiefly to **publish a dynamic bundle** a \
                  `source = \"kv:<bucket>/<key>\"` (or a guest's `spawn-from`) then loads: \
                  `set` a compiled `.wasm` component or a JS bundle, `get`/`list` to inspect, \
                  `rm` to remove. The node must be stopped (the store is single-writer).",
        examples: &[
            "rusm kv set plugins/greeter wasm/greeter.wasm",
            "rusm kv list plugins",
            "rusm kv get plugins/greeter ./greeter.wasm",
            "rusm kv rm plugins/greeter",
        ],
    },
    CommandSpec {
        name: "generate",
        usage:
            "rusm generate component <name> [--lang ts|rust|go] [--protocol http|sse|ws]\n       \
                rusm generate bridge <name> [--lang ts|rust|go]\n       \
                rusm generate authentication <name> [--lang ts|rust|go]",
        summary: "add a component, bridge, or auth hook to an existing project",
        details: "Adds to an existing project (must already have a rusm.toml). \
                  `generate component` creates components/<name>/ with the right source and \
                  appends the matching rusm.toml entry — ready to build immediately with \
                  `rusm build`. `generate bridge` creates bridges/<name>/bridge.wit and a host \
                  stub (host.ts by default, or host.rs/host.go with --lang); grant it in a \
                  [capabilities.*] `bridges = [\"<name>\"]` list to expose it to components. \
                  `generate authentication` creates auth/<name>/host.* (a serving auth hook); \
                  apply it to a listener with `authentication = \"<name>\"`.",
        examples: &[
            "rusm generate component chat --lang ts --protocol ws",
            "rusm generate component feed --lang rust",
            "rusm generate bridge mailer",
            "rusm generate bridge payments --lang ts",
            "rusm generate authentication jwt --lang rust",
        ],
    },
    CommandSpec {
        name: "attach",
        usage: "rusm attach [<host | host:port | ws-url>]",
        summary: "observe a running node (defaults to 127.0.0.1:4000)",
        details: "Connects a live REPL to a running node (like `iex --remsh`): run \
                  scenarios, toggle observer detail, inspect live processes.",
        examples: &["rusm attach", "rusm attach 192.168.1.10:4000"],
    },
];

/// `rusm <version>` — the version line for `--version`, sourced from the crate version so
/// it tracks releases automatically (one source: Cargo.toml).
pub fn version() -> String {
    format!("rusm {}", env!("CARGO_PKG_VERSION"))
}

/// The top-level help: a header with the version + tagline, the command table (names
/// aligned), the global options, and a docs pointer.
pub fn usage() -> String {
    let width = COMMANDS.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out = format!(
        "RUSM {} - {TAGLINE}\n\nUsage:\n  rusm <command> [options]\n\nCommands:\n",
        env!("CARGO_PKG_VERSION")
    );
    for c in COMMANDS {
        out.push_str(&format!(
            "  {:<width$}  {}\n",
            c.name,
            c.summary,
            width = width
        ));
    }
    out.push_str(
        "\nOptions:\n  \
         -h, --help       show this help (or `rusm <command> --help` for a command)\n  \
         -V, --version    print the version\n\n",
    );
    out.push_str(&format!("Docs: {DOCS_URL}\n"));
    out
}

/// The `--help` text for one command (usage + description + examples), or `None` when
/// `name` is not a command.
pub fn command_help(name: &str) -> Option<String> {
    COMMANDS.iter().find(|c| c.name == name).map(|c| {
        let mut out = format!("rusm {} — {}\n\nUsage:\n  {}\n", c.name, c.summary, c.usage);
        if !c.details.is_empty() {
            out.push_str(&format!("\n{}\n", c.details));
        }
        if !c.examples.is_empty() {
            out.push_str("\nExamples:\n");
            for example in c.examples {
                out.push_str(&format!("  {example}\n"));
            }
        }
        out
    })
}

/// Whether the remaining arguments request help (`-h` / `--help`). Consumes the flag so
/// it is never mistaken for a positional argument afterwards.
pub fn wants_help(args: &mut Arguments) -> bool {
    args.contains(["-h", "--help"])
}

/// Whether the remaining arguments request the version (`-V` / `--version`). Consumes the
/// flag, like [`wants_help`].
pub fn wants_version(args: &mut Arguments) -> bool {
    args.contains(["-V", "--version"])
}

/// The node-config overrides every node-booting command accepts: `--config <file>`
/// and `--listen <addr>`. pico-args is the sole parser — nothing re-scans `env::args`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NodeOverrides {
    pub config: Option<String>,
    pub listen: Option<String>,
}

/// Parse [`NodeOverrides`] from the remaining arguments. Errors if a flag is given
/// without its value (a bare `--config` / `--listen`).
pub fn node_overrides(args: &mut Arguments) -> Result<NodeOverrides> {
    Ok(NodeOverrides {
        config: args.opt_value_from_str("--config")?,
        listen: args.opt_value_from_str("--listen")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn args(items: &[&str]) -> Arguments {
        Arguments::from_vec(items.iter().map(OsString::from).collect())
    }

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(version(), format!("rusm {}", env!("CARGO_PKG_VERSION")));
        assert!(version().starts_with("rusm "));
    }

    #[test]
    fn usage_has_a_header_and_lists_every_command_with_its_summary() {
        let u = usage();
        assert!(
            u.starts_with("RUSM "),
            "starts with the RUSM version header"
        );
        assert!(
            u.contains(env!("CARGO_PKG_VERSION")),
            "header shows the version"
        );
        assert!(u.contains(TAGLINE));
        assert!(u.contains("Commands:"));
        assert!(u.contains("-V, --version"), "documents the version flag");
        assert!(u.contains(DOCS_URL));
        for name in [
            "new", "node", "build", "run", "dev", "serve", "kv", "attach", "generate",
        ] {
            assert!(u.contains(name), "usage missing `{name}`");
        }
        assert!(u.contains("scaffold a new RUSM app"));
    }

    #[test]
    fn command_help_is_per_command_with_examples_and_none_for_unknown() {
        let help = command_help("build").expect("build is a command");
        assert!(help.contains("rusm build"));
        assert!(help.contains("compile ./components"));
        assert!(help.contains("Examples:"));

        let new = command_help("new").expect("new is a command");
        assert!(
            new.contains("--template todo-board"),
            "new help shows the template flag"
        );
        assert!(
            new.contains("rusm new board --template"),
            "new help has a template example"
        );

        let gen = command_help("generate").expect("generate is a command");
        assert!(
            gen.contains("component"),
            "generate help mentions component"
        );
        assert!(gen.contains("bridge"), "generate help mentions bridge");
        assert!(
            gen.contains("rusm generate component chat"),
            "generate help has a component example"
        );

        assert!(command_help("frobnicate").is_none());
    }

    #[test]
    fn wants_help_detects_both_flags_and_consumes_them() {
        assert!(wants_help(&mut args(&["-h"])));
        assert!(wants_help(&mut args(&["--help"])));
        assert!(!wants_help(&mut args(&["serve"])));

        let mut a = args(&["--help"]);
        assert!(wants_help(&mut a));
        assert!(a.finish().is_empty(), "the flag is consumed, not left over");
    }

    #[test]
    fn wants_version_detects_both_flags_and_consumes_them() {
        assert!(wants_version(&mut args(&["-V"])));
        assert!(wants_version(&mut args(&["--version"])));
        assert!(!wants_version(&mut args(&["serve"])));

        let mut a = args(&["--version"]);
        assert!(wants_version(&mut a));
        assert!(a.finish().is_empty(), "the flag is consumed, not left over");
    }

    #[test]
    fn node_overrides_parse_config_and_listen_in_any_order() {
        let mut a = args(&["--listen", "0.0.0.0:9000", "--config", "alt.toml"]);
        let ov = node_overrides(&mut a).unwrap();
        assert_eq!(ov.config.as_deref(), Some("alt.toml"));
        assert_eq!(ov.listen.as_deref(), Some("0.0.0.0:9000"));

        assert_eq!(
            node_overrides(&mut args(&[])).unwrap(),
            NodeOverrides::default()
        );
    }

    #[test]
    fn node_overrides_reject_a_flag_without_a_value() {
        assert!(node_overrides(&mut args(&["--config"])).is_err());
    }
}
