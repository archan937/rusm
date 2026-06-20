#!/usr/bin/env bash
# Assemble bridges/*/bridge.wit into every world.wit the build needs. The bridge
# interface slices are the SINGLE SOURCE OF TRUTH: edit bridges/<name>/bridge.wit, run
# this (via `make sync-bridges`), and every world.wit is regenerated identically — which
# also kills the historical doc drift between the copies.
#
# The interface bodies are shared verbatim; only the `world` block varies by consumer:
#   host       — world process { imports; export run }
#                (the host runtime + the raw-ABI fixtures, which generate the world inline)
#   guest      — + world imports { imports }  (the library half of rusm-rs's lib/bin split:
#                rusm-rs, the proc-macro, rusm-go, and the rs-* fixtures that map to them)
#   js-runner  — + import wasi:http (the fetch polyfill, declared here so the artifact
#                stays a wizer-able core module)
#
# Every rusm:runtime world.wit must be classified in TARGETS below; a discovered-but-
# unmapped file (or a mapped-but-missing path) is a hard error — so a new consumer can't
# silently drift from the canonical interface (CLAUDE.md: total awareness on sweeps).
#
# Usage: bridges/assemble-wit.sh [--check]
#   (no args) regenerate every world.wit in place
#   --check   verify every world.wit matches what would be generated; non-zero if stale
set -euo pipefail
cd "$(dirname "$0")/.."

PKG="package rusm:runtime@0.1.0;"
WASI_HTTP="wasi:http/outgoing-handler@0.2.6"

# path:variant — the single registry of every rusm:runtime world.wit and its world shape.
TARGETS="
crates/rusm-wasm/wit/world.wit:host
crates/rusm-rs/wit/world.wit:guest
crates/rusm-rs-macros/wit/world.wit:guest
packages/rusm-go/wit/world.wit:guest
crates/rusm-wasm/js-runner/wit/world.wit:js-runner
crates/rusm-wasm/js-http-runner/wit/deps/rusm-runtime/world.wit:guest
crates/rusm-wasm/tests/fixtures/actor-echo/wit/world.wit:host
crates/rusm-wasm/tests/fixtures/actor-echo/world.wit:host
crates/rusm-wasm/tests/fixtures/actor-kv/wit/world.wit:host
crates/rusm-wasm/tests/fixtures/actor-timeout/wit/world.wit:host
crates/rusm-wasm/tests/fixtures/callback/wit/world.wit:host
crates/rusm-wasm/tests/fixtures/stream-pipe/wit/world.wit:host
crates/rusm-wasm/tests/fixtures/pubsub-broker/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-flaky/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-guest/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-kv/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-service/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-sup/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-tag/wit/world.wit:guest
crates/rusm-wasm/tests/fixtures/rs-timeout/wit/world.wit:guest
"
TARGETS=$(echo "$TARGETS" | grep .)   # drop blank lines

# --- total-awareness cross-check: discovered set must equal the mapped set ----------------
mapped=$(echo "$TARGETS" | cut -d: -f1 | sort)
discovered=$(grep -rl "$PKG" --include=world.wit . | sed 's#^\./##' | grep -vE '(^|/)target/' | sort)
if [ "$mapped" != "$discovered" ]; then
	echo "✗ TARGETS is out of sync with the rusm:runtime world.wit files on disk:"
	diff <(echo "$mapped") <(echo "$discovered") | sed 's/^/    /'
	echo "→ add the new file to TARGETS in bridges/assemble-wit.sh (with its world variant)"
	exit 1
fi

# Every bridge with a bridge.wit is a guest-facing actor-class interface imported into the
# world (dir name == interface name). Host-only bridges (wasip*, http/ws/sse serving loops)
# have no bridge.wit and are wired into the linker separately. Sorted for determinism.
IFACE_FILES=$(ls bridges/*/bridge.wit | sort)
NAMES=$(for f in $IFACE_FILES; do basename "$(dirname "$f")"; done | sort)

emit_interfaces() { for f in $IFACE_FILES; do cat "$f"; echo; done; }
emit_imports()    { for n in $NAMES; do echo "    import $n;"; done; }

# $1 = variant → full world.wit on stdout.
emit_world_file() {
	local variant=$1
	echo "$PKG"
	echo
	emit_interfaces
	echo "/// A RUSM actor component: imports the actor API + platform bridges, and exports an"
	echo "/// entry point the runtime calls to start the process."
	if [ "$variant" = js-runner ]; then
		echo "/// It also imports outbound \`wasi:http\` for the \`fetch\` polyfill (the host gates it"
		echo "/// on the network capability) — declared here, via wit-bindgen, rather than pulled from"
		echo "/// the \`wasip2\` crate, so the guest builds as a **core module** that \`wizer\` can"
		echo "/// pre-initialize (a component can't be wizer'd)."
	fi
	echo "world process {"
	emit_imports
	[ "$variant" = js-runner ] && echo "    import $WASI_HTTP;"
	echo "    export run: func();"
	echo "}"
	if [ "$variant" = guest ]; then
		echo
		echo "/// Imports only — what the \`rusm-rs\` library generates. A guest crate generates the"
		echo "/// full \`process\` world and maps these interfaces to this crate's bindings (so each is"
		echo "/// imported once), then \`export!\`s its \`run\`."
		echo "world imports {"
		emit_imports
		echo "}"
	fi
}

check=0; [ "${1:-}" = --check ] && check=1
stale=0
for entry in $TARGETS; do
	path=${entry%:*}; variant=${entry##*:}
	if [ "$check" = 1 ]; then
		diff -q <(emit_world_file "$variant") "$path" >/dev/null 2>&1 || { echo "✗ stale: $path"; stale=1; }
	else
		emit_world_file "$variant" > "$path"
	fi
done
if [ "$check" = 1 ]; then
	[ "$stale" = 1 ] && { echo "→ run \`make sync-bridges\`"; exit 1; }
	echo "✓ all $(echo "$TARGETS" | wc -l | tr -d ' ') world.wit copies in sync"
else
	echo "==> assembled $(echo "$TARGETS" | wc -l | tr -d ' ') world.wit copies from bridges/*/bridge.wit"
fi
exit 0
