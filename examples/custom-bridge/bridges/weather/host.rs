//! The `weather` bridge's native host impl — authored in the **exact platform-bridge
//! convention** (`impl <iface>::Host for BridgeHost` + a `pub fn add_to_linker`), the same
//! shape RUSM's own `bridges/<name>/host.rs` use. This is the one file the app author
//! writes for the bridge's behaviour; `rusm build` generates the surrounding glue
//! (`src/bindings.rs`, `src/bridges.rs`, `wit/`).
//!
//! It reaches the calling process through [`rusm_wasm::BridgeHost`]'s public accessors
//! (here `pid()`); a real bridge would gate on `caps().allows_bridge("weather")` and call
//! out via `runtime()`. The `Host` trait + `add_to_linker` come from `crate::bindings`,
//! which `bindgen!`s the synthesized `wit/` world.

use crate::bindings::weather::bridge::forecast;
use rusm_wasm::wasmtime::component::HasSelf;
use rusm_wasm::{wasmtime, BridgeHost, BridgeLinker};

/// Register this bridge into the component linker (called by the generated `bridges::extend`).
pub fn add_to_linker(linker: &mut BridgeLinker) -> wasmtime::Result<()> {
    forecast::add_to_linker::<_, HasSelf<BridgeHost>>(linker, |host| host)
}

impl forecast::Host for BridgeHost {
    async fn lookup(&mut self, city: String) -> String {
        // A real bridge would call a weather service; this proves the typed call reaches
        // genuine host state — the calling process's pid.
        format!("sunny in {city} (served by pid {})", self.pid())
    }
}
