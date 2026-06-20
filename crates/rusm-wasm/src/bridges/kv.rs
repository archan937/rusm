// Canonical source: bridges/kv/host.rs — the kv bridge's native host impl.
// Synced into rusm-wasm (crates/rusm-wasm/src/bridges/kv.rs) by `make sync-bridges`; edit
// this file, not the copy. The `bridge_host_in_sync` test fails the build on drift.

//! kv bridge — host side. Implements the `kv` WIT interface on [`WasiHost`] over the
//! node's durable store (`rusm-kv`). The `storage`-capability gate and store lookup live
//! in [`WasiHost::kv_bucket`] (shared with `spawn-from`'s `kv:` source loader, so the gate
//! is in exactly one place); each op here is a thin lift→bucket→lower.

use crate::actor::rusm::runtime::kv;
use crate::bridges::WasiHost;

/// Wire the kv interface into a component linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WasiHost>,
) -> wasmtime::Result<()> {
    kv::add_to_linker::<_, wasmtime::component::HasSelf<WasiHost>>(linker, |host| host)
}

impl kv::Host for WasiHost {
    async fn get(&mut self, bucket: String, key: String) -> Result<Option<Vec<u8>>, String> {
        self.kv_bucket(&bucket)?
            .get(&key)
            .map_err(|e| e.to_string())
    }

    async fn set(&mut self, bucket: String, key: String, value: Vec<u8>) -> Result<(), String> {
        self.kv_bucket(&bucket)?
            .set(&key, &value)
            .map_err(|e| e.to_string())
    }

    async fn delete(&mut self, bucket: String, key: String) -> Result<bool, String> {
        self.kv_bucket(&bucket)?
            .delete(&key)
            .map_err(|e| e.to_string())
    }

    async fn exists(&mut self, bucket: String, key: String) -> Result<bool, String> {
        self.kv_bucket(&bucket)?
            .exists(&key)
            .map_err(|e| e.to_string())
    }

    async fn list(&mut self, bucket: String) -> Result<Vec<String>, String> {
        self.kv_bucket(&bucket)?.list().map_err(|e| e.to_string())
    }
}
