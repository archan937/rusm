//! The RUSM benchmark harness: scenarios, a deterministic synthetic data source,
//! a runner that aggregates ticks into transportable frames, and the WebSocket
//! server feeding the dashboard and the `rusm attach` REPL.

// `config` (the `rusm.toml` manifest) and `profile` (the resource tier) are the
// shared node types — they live in `rusm-node`; we re-export them and build on
// top (the benchmark interpretation of a profile is `profile_tuning`).
use rusm_node::{config, profile};

mod componentstorm;
mod connectionscale;
mod connectionstorm;
mod cryptoops;
mod custombridge;
mod distributedfanout;
mod dynamicwasm;
mod fairness;
mod faultrecovery;
mod kvstorm;
mod modulestorm;
mod pingpong;
mod profile_tuning;
mod protocol;
mod pubsubfanout;
mod report;
mod runner;
mod sample;
mod scenario;
mod server;
mod serving;
mod spawnstorm;
mod streampipe;
mod synthetic;

pub use config::{
    CapabilitySpec, ComponentSpec, NodeConfig, PreopenSpec, ServeProtocol, ServeSpec,
};
pub use profile::{ResourceProfile, ResourceProfileMeta};
pub use protocol::{ClientCommand, Frame, ServerMessage};
pub use report::summarize_frame;
pub use runner::{runner_config, Runner, RunnerConfig};
pub use sample::Sample;
pub use scenario::{MetricUnit, Scenario, ScenarioMeta};
pub use server::{serve, serve_on, Node};
pub use synthetic::SyntheticSource;
