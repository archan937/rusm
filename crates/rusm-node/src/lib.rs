//! `rusm-node` — the node layer shared by the `rusm` CLI and any tooling that
//! hosts or observes a running node.
//!
//! It owns three things:
//! - [`config`] — the `rusm.toml` app manifest (components, servers, capabilities).
//! - [`profile`] — [`ResourceProfile`], the machine-usage tier.
//! - the live **attach** layer (`protocol` + `node`) — a node streams plain
//!   [`rusm_otp`] process introspection to attached clients over WebSocket, and
//!   `rusm attach` renders it.

pub mod config;
pub mod node;
pub mod profile;
pub mod protocol;
pub mod routes;

pub use config::{
    BundleSource, CapabilitySpec, ComponentSpec, LogConfig, NodeConfig, NodeSettings, PreopenSpec,
    ServeProtocol, ServeSpec, TlsConfig,
};
pub use node::{serve, serve_on, Node};
pub use profile::{ResourceProfile, ResourceProfileMeta};
pub use protocol::{ClientCommand, NodeSnapshot, ProcessInfo, ServerMessage};
pub use routes::{Resolution, RouteTable};
