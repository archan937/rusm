//! The generated WIT bindings for the whole `rusm:runtime` world — bound once here with
//! `bindgen!` as shared infrastructure (not a capability). Every bridge's `host.rs`
//! implements its interface's `Host` trait over [`crate::bridges::WasiHost`] and references
//! `crate::bindings::rusm::runtime::<interface>`; the async component model lets a host call
//! suspend the guest fiber (the "write blocking code, get async" property).

wasmtime::component::bindgen!({
    world: "process",
    path: "wit",
    imports: { default: async },
    exports: { default: async },
});
