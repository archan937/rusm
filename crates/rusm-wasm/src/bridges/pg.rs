// Canonical source: bridges/pg/host.rs — the pg (process-group tags) bridge's native host
// impl. Synced into rusm-wasm (crates/rusm-wasm/src/bridges/pg.rs) by `make sync-bridges`;
// edit this file, not the copy. The `bridge_host_in_sync` test fails the build on drift.

//! pg bridge — host side. Implements the `pg` WIT interface on [`WasiHost`] as thin calls
//! into `rusm-otp`'s process-group registry. `register-tag`/`unregister-tag`/`whereis-tag`
//! are unprivileged (a process tags itself; membership is a read); `kill-tag` is gated by
//! `process-control` (it terminates other processes), exactly like `kill`.

use crate::actor::rusm::runtime::pg;
use crate::bridges::WasiHost;
use rusm_otp::Pid;

/// Wire the pg interface into a component linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WasiHost>,
) -> wasmtime::Result<()> {
    pg::add_to_linker::<_, wasmtime::component::HasSelf<WasiHost>>(linker, |host| host)
}

impl pg::Host for WasiHost {
    /// Join this process to `tag`.
    async fn register_tag(&mut self, tag: String) {
        self.rt.register_tag(tag, Pid::from_raw(self.pid));
    }

    /// Leave a tag this process holds.
    async fn unregister_tag(&mut self, tag: String) {
        self.rt.unregister_tag(&tag, Pid::from_raw(self.pid));
    }

    /// Live members of `tag` (a read, like `whereis`/`list` — ungated).
    async fn whereis_tag(&mut self, tag: String) -> Vec<u64> {
        self.rt
            .whereis_tag(&tag)
            .into_iter()
            .map(|p| p.raw())
            .collect()
    }

    /// Terminate every live member of `tag`. Capability-gated by process-control like
    /// `kill` — a group kill targets other processes, so there is no self-exception.
    async fn kill_tag(&mut self, tag: String) -> u32 {
        if !self.caps.process_control() {
            return 0; // default-deny: cannot terminate other processes
        }
        self.rt.kill_tag(&tag) as u32
    }
}
