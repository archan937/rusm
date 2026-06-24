//! Host binary for the mailer Rust-bridge app: registers the `mailer` bridge, then serves
//! the manifest via `rusm_cli::host`. The generated `bridges::extend` wires the bridge;
//! `rusm_cli::host::serve` is the same loop as a pure-guest `rusm serve`.

mod bindings;
mod bridges;

use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg =
        rusm_node::NodeConfig::load(Path::new("rusm.toml"), false).map_err(anyhow::Error::msg)?;
    rusm_cli::host::serve(Path::new("."), &cfg, bridges::extend).await
}
