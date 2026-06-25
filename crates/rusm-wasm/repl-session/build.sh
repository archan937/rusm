#!/usr/bin/env bash
# Build the REPL session bundle that the node spawns for `rusm attach`'s live JS
# shell. Bun bundles the worker + its transform (acorn) into one CommonJS file
# the js-runner can eval; the artifact is committed and embedded by rusm-wasm via
# include_bytes!, so a plain `cargo build` needs no Bun.
#
# Output: ../runtimes/repl_session.js
set -euo pipefail
cd "$(dirname "$0")"

bun install --frozen-lockfile 2>/dev/null || bun install

bun build ./index.ts \
  --target=browser \
  --format=cjs \
  --minify \
  --outfile ../runtimes/repl_session.js

echo "built ../runtimes/repl_session.js ($(wc -c < ../runtimes/repl_session.js) bytes)"
