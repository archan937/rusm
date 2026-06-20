// Canonical source: bridges/log/host.rs — the log bridge's native host impl.
// Synced into rusm-wasm (crates/rusm-wasm/src/bridges/log.rs) by `make sync-bridges`; edit
// this file, not the copy. The `bridge_host_in_sync` test fails the build on drift.

//! log bridge — host side. Implements the `log` WIT interface on [`WasiHost`]: maps the
//! guest's severity to the node logger's gate + the `rusm-logfmt` colour, drops the line if
//! the node `[log] level` filters it, else stamps `component#pid` and writes one atomic
//! line to stderr — the same stream + format as the platform's own lifecycle logs. Not
//! capability-gated (operator-controlled via the level).

use crate::actor::rusm::runtime::log as log_bridge;
use crate::bridges::WasiHost;
use rusm_otp::Pid;

/// Wire the log interface into a component linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WasiHost>,
) -> wasmtime::Result<()> {
    log_bridge::add_to_linker::<_, wasmtime::component::HasSelf<WasiHost>>(linker, |host| host)
}

impl log_bridge::Host for WasiHost {
    async fn log(&mut self, level: log_bridge::LogLevel, message: String) {
        let (gate, severity) = match level {
            log_bridge::LogLevel::Error => (rusm_otp::LogLevel::Error, rusm_logfmt::Level::Error),
            log_bridge::LogLevel::Warn => (rusm_otp::LogLevel::Warn, rusm_logfmt::Level::Warn),
            log_bridge::LogLevel::Info => (rusm_otp::LogLevel::Info, rusm_logfmt::Level::Info),
            log_bridge::LogLevel::Debug => (rusm_otp::LogLevel::Debug, rusm_logfmt::Level::Debug),
        };
        if !self.rt.wants_log(gate) {
            return;
        }
        let component = self
            .rt
            .info(Pid::from_raw(self.pid))
            .and_then(|i| i.label)
            .unwrap_or_default();
        // One atomic stderr write per line (`eprintln!` locks stderr for the call), so
        // concurrently-logging processes never interleave mid-line.
        eprintln!(
            "{}",
            rusm_logfmt::line(severity, &component, self.pid, &message)
        );
    }
}
