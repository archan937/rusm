//! Logic for the `rusm` CLI, kept separate from the I/O glue in `main.rs` so it
//! is unit-testable: argument parsing, the command/usage table, REPL command
//! parsing, and live-message formatting.

mod app;
mod cli;
mod component;
mod endpoint;
/// Hosting an app's node — the construction + serve loop shared by the `rusm` CLI and an
/// app's own generated host crate (the custom-bridge model). Public so a host crate can
/// `rusm_cli::host::serve(root, &cfg, |l| my_bridge::add_to_linker(l))`.
pub mod host;
mod render;
mod repl;
mod scaffold;
mod template;

pub use app::{capabilities_for, serve_apps, spawn_components, Hosted, ServedEndpoint};
pub use cli::{
    command_help, node_overrides, usage, version, wants_help, wants_version, NodeOverrides,
};
pub use component::prebuilt_wasm;
pub use endpoint::{normalize_target, DEFAULT_HOST};
pub use render::render_message;
pub use repl::{parse, ReplInput, HELP};
pub use scaffold::{parse_new_args, scaffold, Lang, NewApp, Protocol};
pub use template::Template;
