#!/usr/bin/env bash
# Build every Go (TinyGo) test fixture (the go-*/ dirs) into <name>.wasm beside them
# (e.g. go-guest → go_guest.wasm), checked in and embedded by the rusm-wasm tests via
# include_bytes!. Each is a component over the rusm:runtime actor world, built against
# the rusm-go SDK's wit. Flags: -no-debug (strip DWARF), -panic=trap (a Go panic → a
# wasm trap → process Crashed, RUSM's crash model), -opt=z (size).
# Run with the pinned toolchain: `mise exec -- ./build-go.sh`.
set -euo pipefail
cd "$(dirname "$0")"

for dir in go-*/; do
  name="${dir%/}"
  wasm="${name//-/_}.wasm" # go-guest → go_guest.wasm, beside the fixture dirs
  (
    cd "$dir"
    go mod tidy
    tinygo build -target=wasip2 -no-debug -panic=trap -opt=z \
      -wit-package ../../../../../packages/rusm-go/wit -wit-world component \
      -o "../$wasm" .
  )
  echo "built $wasm ($(wc -c <"$wasm") bytes)"
done
