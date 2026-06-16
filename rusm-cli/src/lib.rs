//! Logic for the `rusm` CLI, kept separate from the I/O glue in `main.rs` so it
//! is unit-testable: argument parsing, the command/usage table, REPL command
//! parsing, and live-message formatting.

mod app;
mod cli;
mod endpoint;
mod render;
mod repl;
mod scaffold;

pub use app::{capabilities_for, serve_apps, spawn_components, Hosted, ServedEndpoint};
pub use cli::{command_help, node_overrides, usage, wants_help, NodeOverrides};
pub use endpoint::{normalize_target, DEFAULT_HOST};
pub use render::render_message;
pub use repl::{parse, ReplInput, HELP};
pub use scaffold::{parse_new_args, scaffold, Lang, NewApp, Protocol};
