//! The `rusm` command-line surface: one command table that backs both the
//! top-level usage and each command's `--help`, plus the shared pico-args helpers.
//! Pure logic, kept out of `main.rs` (thin I/O glue) so it is unit-tested.

use anyhow::Result;
use pico_args::Arguments;

/// One CLI command: its name, full invocation, and one-line summary.
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
}

/// Every `rusm` command — the single source of truth for the help text, so the
/// overview and the per-command help can never drift apart.
const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "new",
        usage: "rusm new <name> [--rust] [--lang ts|rust] [--protocol http|sse|ws]",
        summary: "scaffold a new RUSM app in ./<name>",
    },
    CommandSpec {
        name: "node",
        usage: "rusm node start [--config <file>] [--listen <addr>]",
        summary: "host the app and expose a live attach endpoint",
    },
    CommandSpec {
        name: "build",
        usage: "rusm build",
        summary: "compile ./components/* -> ./wasm/*.wasm",
    },
    CommandSpec {
        name: "run",
        usage: "rusm run",
        summary: "run ./wasm components per rusm.toml [components.<name>]",
    },
    CommandSpec {
        name: "dev",
        usage: "rusm dev",
        summary: "build + run, then watch ./components and reload on edits",
    },
    CommandSpec {
        name: "serve",
        usage: "rusm serve",
        summary: "host ./wasm components as HTTP/WS/SSE servers per rusm.toml [[serve]]",
    },
    CommandSpec {
        name: "attach",
        usage: "rusm attach [<host | host:port | ws-url>]",
        summary: "observe a running node (defaults to 127.0.0.1:4000)",
    },
];

/// The top-level `usage:` block: every command's invocation and summary.
pub fn usage() -> String {
    let mut out = String::from("usage:\n");
    for c in COMMANDS {
        out.push_str("  ");
        out.push_str(c.usage);
        out.push_str("\n      ");
        out.push_str(c.summary);
        out.push('\n');
    }
    out
}

/// The `--help` text for one command, or `None` when `name` is not a command.
pub fn command_help(name: &str) -> Option<String> {
    COMMANDS
        .iter()
        .find(|c| c.name == name)
        .map(|c| format!("{}\n      {}", c.usage, c.summary))
}

/// Whether the remaining arguments request help (`-h` / `--help`). Consumes the
/// flag so it is never mistaken for a positional argument afterwards.
pub fn wants_help(args: &mut Arguments) -> bool {
    args.contains(["-h", "--help"])
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
    fn usage_lists_every_command_with_its_summary() {
        let u = usage();
        assert!(u.starts_with("usage:\n"));
        for name in ["new", "node", "build", "run", "dev", "serve", "attach"] {
            assert!(
                u.contains(&format!("rusm {name}")),
                "usage missing `{name}`"
            );
        }
        assert!(u.contains("scaffold a new RUSM app"));
    }

    #[test]
    fn command_help_is_per_command_and_none_for_unknown() {
        let help = command_help("build").expect("build is a command");
        assert!(help.contains("rusm build"));
        assert!(help.contains("compile ./components"));
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
