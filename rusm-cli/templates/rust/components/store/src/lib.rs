//! The `store` service as an isolated, supervised process. `#[rusm_rs::main]` provides the
//! component shell; `store::serve()` runs the receive→dispatch→reply loop around the
//! service's functions (defined once in the shared `store-svc` crate). Spawned on demand by
//! the `reporter` and reached through the generated typed client; it runs under its own
//! manifest-declared profile whoever spawns it.
use store_svc::store;

#[rusm_rs::main]
fn run() {
    store::serve();
}
