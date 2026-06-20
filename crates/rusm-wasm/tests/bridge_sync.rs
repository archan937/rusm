//! Drift guard for the `bridges/` layout: each bridge's canonical files
//! (`bridges/<name>/{host.rs,guest.rs,guest.go,guest.js,bridge.wit}`) must be
//! byte-identical to the copies materialized into the crates by `make sync-bridges`.
//! Edit the canonical file, then `make sync-bridges`; this fails the build if a copy
//! drifts — the same pattern as `wit_in_sync` and the `rusm-cli` `template::` tests.
//!
//! `host.rs`/`guest.*` are compared directly (byte-for-byte). The assembled `world.wit`
//! copies are checked by re-running the assembler in `--check` mode (it regenerates to a
//! temp and diffs every target).

use std::process::Command;

/// Canonical bridge file ↔ its synced copy. Paths are relative to this test file
/// (`crates/rusm-wasm/tests/`).
fn assert_synced(canonical: &str, copy: &str, canonical_text: &str, copy_text: &str) {
    assert_eq!(
        canonical_text, copy_text,
        "\n{copy}\n  drifted from canonical\n{canonical}\n  → run `make sync-bridges`\n",
    );
}

#[test]
fn bridge_files_in_sync() {
    // kv bridge — host impl + the three guest bindings.
    assert_synced(
        "bridges/kv/host.rs",
        "crates/rusm-wasm/src/bridges/kv.rs",
        include_str!("../../../bridges/kv/host.rs"),
        include_str!("../src/bridges/kv.rs"),
    );
    assert_synced(
        "bridges/kv/guest.rs",
        "crates/rusm-rs/src/kv.rs",
        include_str!("../../../bridges/kv/guest.rs"),
        include_str!("../../rusm-rs/src/kv.rs"),
    );
    assert_synced(
        "bridges/kv/guest.go",
        "packages/rusm-go/kv.go",
        include_str!("../../../bridges/kv/guest.go"),
        include_str!("../../../packages/rusm-go/kv.go"),
    );
    assert_synced(
        "bridges/kv/guest.js",
        "crates/rusm-wasm/js-runner/bridge/kv.js",
        include_str!("../../../bridges/kv/guest.js"),
        include_str!("../js-runner/bridge/kv.js"),
    );

    // log bridge — host + all three guests (RS module is `logging`, not `log`, to avoid
    // clashing with the `log` crate).
    assert_synced(
        "bridges/log/host.rs",
        "crates/rusm-wasm/src/bridges/log.rs",
        include_str!("../../../bridges/log/host.rs"),
        include_str!("../src/bridges/log.rs"),
    );
    assert_synced(
        "bridges/log/guest.rs",
        "crates/rusm-rs/src/logging.rs",
        include_str!("../../../bridges/log/guest.rs"),
        include_str!("../../rusm-rs/src/logging.rs"),
    );
    assert_synced(
        "bridges/log/guest.go",
        "packages/rusm-go/log.go",
        include_str!("../../../bridges/log/guest.go"),
        include_str!("../../../packages/rusm-go/log.go"),
    );
    assert_synced(
        "bridges/log/guest.js",
        "crates/rusm-wasm/js-runner/bridge/log.js",
        include_str!("../../../bridges/log/guest.js"),
        include_str!("../js-runner/bridge/log.js"),
    );
}

#[test]
fn world_wit_in_sync() {
    // The assembler regenerates every world.wit to a temp and diffs; non-zero on drift.
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bridges/assemble-wit.sh");
    let out = Command::new("bash")
        .arg(script)
        .arg("--check")
        .output()
        .expect("run bridges/assemble-wit.sh --check");
    assert!(
        out.status.success(),
        "world.wit copies drifted from bridges/*/bridge.wit — run `make sync-bridges`\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
