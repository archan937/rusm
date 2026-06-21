// Canonical source: bridges/pg/guest.rs — the pg bridge's Rust guest binding.
// Synced into rusm-rs (crates/rusm-rs/src/pg.rs) by `make sync-bridges`; edit this
// file, not the copy. The `bridge_guest_in_sync` test fails the build on drift.

//! Process-group tags (Erlang's `pg`) for a Rust guest — RUSM's cross-language pub/sub
//! primitive (subscribe = [`register_tag`], publish = [`whereis_tag`] + `send`). A process
//! joins a tag, many processes may share a tag, and memberships release on exit. These are
//! re-exported at the crate root, so the public paths stay `rusm_rs::{register_tag, …}`.

// The generated `pg` interface bindings.
use crate::rusm::runtime::pg as abi;
use crate::Pid;

/// Join **this** process to a process-group `tag`: a process may hold many tags, a tag many
/// processes. Released automatically on exit. Unprivileged — a process tags itself;
/// terminating a group is the gated [`kill_tag`].
pub fn register_tag(tag: &str) {
    abi::register_tag(tag);
}

/// Leave a process-group `tag` this process holds.
pub fn unregister_tag(tag: &str) {
    abi::unregister_tag(tag);
}

/// Live members of process-group `tag` (empty if unknown).
pub fn whereis_tag(tag: &str) -> Vec<Pid> {
    abi::whereis_tag(tag).into_iter().map(Pid).collect()
}

/// Terminate every live member of process-group `tag`; returns how many were killed.
/// Capability-gated by `process-control` (it terminates other processes); returns `0` if
/// denied or the tag is empty.
pub fn kill_tag(tag: &str) -> u32 {
    abi::kill_tag(tag)
}
