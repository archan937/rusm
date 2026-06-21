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

    // actor bridge (the Erlang Process core) — host impl. The `bindgen!` itself lives in the
    // neutral crate::bindings module (shared infra); this is the actor interface's impl.
    assert_synced(
        "bridges/actor/host.rs",
        "crates/rusm-wasm/src/bridges/actor.rs",
        include_str!("../../../bridges/actor/host.rs"),
        include_str!("../src/bridges/actor.rs"),
    );
    assert_synced(
        "bridges/actor/guest.rs",
        "crates/rusm-rs/src/actor.rs",
        include_str!("../../../bridges/actor/guest.rs"),
        include_str!("../../rusm-rs/src/actor.rs"),
    );
    assert_synced(
        "bridges/actor/guest.go",
        "packages/rusm-go/actor.go",
        include_str!("../../../bridges/actor/guest.go"),
        include_str!("../../../packages/rusm-go/actor.go"),
    );
    assert_synced(
        "bridges/actor/guest.js",
        "crates/rusm-wasm/js-runner/bridge/actor.js",
        include_str!("../../../bridges/actor/guest.js"),
        include_str!("../js-runner/bridge/actor.js"),
    );

    // pg bridge (process-group tags) — host + all three guests.
    assert_synced(
        "bridges/pg/host.rs",
        "crates/rusm-wasm/src/bridges/pg.rs",
        include_str!("../../../bridges/pg/host.rs"),
        include_str!("../src/bridges/pg.rs"),
    );
    assert_synced(
        "bridges/pg/guest.rs",
        "crates/rusm-rs/src/pg.rs",
        include_str!("../../../bridges/pg/guest.rs"),
        include_str!("../../rusm-rs/src/pg.rs"),
    );
    assert_synced(
        "bridges/pg/guest.go",
        "packages/rusm-go/pg.go",
        include_str!("../../../bridges/pg/guest.go"),
        include_str!("../../../packages/rusm-go/pg.go"),
    );
    assert_synced(
        "bridges/pg/guest.js",
        "crates/rusm-wasm/js-runner/bridge/pg.js",
        include_str!("../../../bridges/pg/guest.js"),
        include_str!("../js-runner/bridge/pg.js"),
    );

    // serve bridge (per-connection WS/SSE handler controls) — host + all three guests.
    assert_synced(
        "bridges/serve/host.rs",
        "crates/rusm-wasm/src/bridges/serve.rs",
        include_str!("../../../bridges/serve/host.rs"),
        include_str!("../src/bridges/serve.rs"),
    );
    assert_synced(
        "bridges/serve/guest.rs",
        "crates/rusm-rs/src/serve.rs",
        include_str!("../../../bridges/serve/guest.rs"),
        include_str!("../../rusm-rs/src/serve.rs"),
    );
    assert_synced(
        "bridges/serve/guest.go",
        "packages/rusm-go/serve.go",
        include_str!("../../../bridges/serve/guest.go"),
        include_str!("../../../packages/rusm-go/serve.go"),
    );
    assert_synced(
        "bridges/serve/guest.js",
        "crates/rusm-wasm/js-runner/bridge/serve.js",
        include_str!("../../../bridges/serve/guest.js"),
        include_str!("../js-runner/bridge/serve.js"),
    );

    // stream bridge — host + all three guests.
    assert_synced(
        "bridges/streams/host.rs",
        "crates/rusm-wasm/src/bridges/streams.rs",
        include_str!("../../../bridges/streams/host.rs"),
        include_str!("../src/bridges/streams.rs"),
    );
    assert_synced(
        "bridges/streams/guest.rs",
        "crates/rusm-rs/src/streams.rs",
        include_str!("../../../bridges/streams/guest.rs"),
        include_str!("../../rusm-rs/src/streams.rs"),
    );
    assert_synced(
        "bridges/streams/guest.go",
        "packages/rusm-go/streams.go",
        include_str!("../../../bridges/streams/guest.go"),
        include_str!("../../../packages/rusm-go/streams.go"),
    );
    assert_synced(
        "bridges/streams/guest.js",
        "crates/rusm-wasm/js-runner/bridge/streams.js",
        include_str!("../../../bridges/streams/guest.js"),
        include_str!("../js-runner/bridge/streams.js"),
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
