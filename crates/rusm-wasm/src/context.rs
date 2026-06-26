//! A **host-only, per-process claims context** — identity/attributes the host attaches to a
//! process (e.g. an `app_id` derived from a validated request token by an auth hook).
//!
//! It is written only by host code (a serving auth hook seeds it; a custom bridge may read or
//! amend it) and read only through [`BridgeHost::context`](crate::BridgeHost::context). **No
//! WIT op exposes it**, so guest code can never read, write, or forge it — the security of
//! per-tenant bridges rests on that. It lives on the per-process `WasiHost`; a wasm instance
//! executes serially (one call at a time, never reentrant), so this needs no task-local or
//! lock — access is naturally single-threaded per process, and each process has its own.

use std::collections::BTreeMap;

/// A process's claims context: a small, ordered string→string map. Cheap to clone (it rides
/// down the spawn tree as a snapshot). `BTreeMap` so iteration/labels are deterministic.
/// `Serialize` (transparent → a plain JSON object) so a Rust bridge's delegation shim can
/// forward the claims in-band to a TS/Go bridge runner, which exposes them via `context()`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct ProcessContext(BTreeMap<String, String>);

impl ProcessContext {
    /// An empty context — the default for a process with no claims attached.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value for `key`, or `None`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Set (or replace) a claim. Host-side only — there is no guest path to this.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }

    /// Whether any claim is set.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The claims in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Box this context as opaque mailbox [`Meta`](rusm_otp::Meta) for an outbound send —
    /// `None` when empty, so a context-less message stays on the zero-cost meta path. The
    /// single place the context↔meta representation is bridged (the actor `send`/`send-after`
    /// ops and the serving auth seed both go through here).
    pub(crate) fn into_meta(self) -> rusm_otp::Meta {
        if self.is_empty() {
            None
        } else {
            Some(std::sync::Arc::new(self))
        }
    }
}

impl FromIterator<(String, String)> for ProcessContext {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip_and_emptiness() {
        let mut ctx = ProcessContext::new();
        assert!(ctx.is_empty());
        assert_eq!(ctx.get("app_id"), None);
        ctx.set("app_id", "acme");
        assert!(!ctx.is_empty());
        assert_eq!(ctx.get("app_id"), Some("acme"));
        ctx.set("app_id", "globex"); // replace
        assert_eq!(ctx.get("app_id"), Some("globex"));
    }

    #[test]
    fn builds_from_claims_and_iterates_in_key_order() {
        let ctx = ProcessContext::from_iter([
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ]);
        assert_eq!(ctx.iter().collect::<Vec<_>>(), [("a", "1"), ("b", "2")]);
    }

    #[test]
    fn a_clone_is_an_independent_snapshot() {
        // A child inherits a *clone* of its parent's context (a snapshot), so a later write
        // on either side never bleeds into the other — no cross-tenant leakage by aliasing.
        let mut parent = ProcessContext::new();
        parent.set("app_id", "acme");
        let mut child = parent.clone();
        child.set("app_id", "globex");
        child.set("scope", "read");
        assert_eq!(parent.get("app_id"), Some("acme"));
        assert_eq!(parent.get("scope"), None);
        assert_eq!(child.get("app_id"), Some("globex"));
    }

    #[test]
    fn the_actor_world_exposes_no_guest_context_op() {
        // Structural guarantee — the per-tenant-bridge security model rests on it: the
        // claims context is host-only. The guest's `rusm:runtime` WIT world declares no
        // op that reads, writes, or forges it. If anyone ever adds a `context` op to the
        // world, this trips and forces the question.
        let wit = include_str!("../wit/world.wit");
        let in_code = wit
            .lines()
            .map(str::trim_start)
            .filter(|line| !line.starts_with("//"));
        for line in in_code {
            assert!(
                !line.contains("context"),
                "the actor world must expose no guest-facing `context` op (found: {line:?})"
            );
        }
    }
}
