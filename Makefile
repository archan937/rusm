.DEFAULT_GOAL := help

DASHBOARD := bench/dashboard
DOCS := docs
DIST := $(DOCS)/.vitepress/dist
GH_PAGES := gh-pages
SCENARIO ?= connection-storm
SECONDS ?= 5
EX ?= host_components
VERSION ?= 0.2.0

# crates.io publish order — dependencies before dependents. Each crate is published from
# its own directory, so the same loop works for workspace members AND the wasm-only guest
# crates excluded from the workspace (rusm-rs, rusm-rs-macros).
PUBLISH_ORDER := \
	crates/rusm-logfmt crates/rusm-wire crates/rusm-metrics crates/rusm-kv crates/rusm-jsc \
	crates/rusm-rs-macros crates/rusm-otp crates/rusm-observer crates/rusm-node \
	crates/rusm-cluster crates/rusm-rs crates/rusm-wasm rusm-cli

.PHONY: help
help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

.PHONY: dashboard
dashboard: ## Start the benchmark node + the dashboard, then open the printed URL — "the money"
	@cargo build --release -p rusm-bench
	@# Kill any stale node still holding the port — otherwise the new node fails to
	@# bind, the dashboard silently talks to the OLD node, and you debug a ghost.
	@pkill -f "rusm-bench start" 2>/dev/null && sleep 1 || true
	@lsof -nP -iTCP:4000 -sTCP:LISTEN -t 2>/dev/null | xargs -r kill 2>/dev/null || true
	@echo "→ starting node (log: /tmp/rusm-node.log) + dashboard…"
	@./target/release/rusm-bench start >/tmp/rusm-node.log 2>&1 & \
		NODE=$$!; \
		trap 'kill $$NODE 2>/dev/null' EXIT INT TERM; \
		sleep 1; \
		if ! kill -0 $$NODE 2>/dev/null; then \
			echo "✗ node failed to start — most likely the port is in use:"; \
			sed 's/^/    /' /tmp/rusm-node.log; exit 1; \
		fi; \
		cd $(DASHBOARD) && { test -d node_modules || bun install; } && bun run dev

.PHONY: node
node: ## Start the benchmark node on ws://127.0.0.1:4000 (release — Wasm perf is ~3-4x debug)
	cargo run --release -p rusm-bench -- start

.PHONY: ui
ui: ## Start only the dashboard dev server (expects a node already running)
	cd $(DASHBOARD) && { test -d node_modules || bun install; } && bun run dev

.PHONY: run
run: ## Run a scenario in the terminal (SCENARIO=… SECONDS=…)
	cargo run -p rusm-bench -- run $(SCENARIO) $(SECONDS)

.PHONY: example
example: ## Run an example (EX=host_components|host_ts_component|embedded_node|cluster|http_bench|ws_bench|sse_bench|connection_scale|cluster_fanout)
	cargo run -p rusm-bench --example $(EX)

.PHONY: build
build: ## Build the whole workspace
	cargo build --workspace

.PHONY: test
test: ## Run all Rust + dashboard tests
	cargo test --workspace
	cd $(DASHBOARD) && bun test

.PHONY: cov
cov: ## Coverage report (Rust workspace + dashboard)
	cargo llvm-cov --workspace --ignore-filename-regex 'main\.rs' --summary-only
	cd $(DASHBOARD) && bun test --coverage

.PHONY: fmt
fmt: ## Format Rust + dashboard
	cargo fmt
	cd $(DASHBOARD) && bunx prettier --write src

.PHONY: fmt-check
fmt-check: ## Check formatting (Rust + dashboard)
	cargo fmt --check
	cd $(DASHBOARD) && bunx prettier --check src

.PHONY: docs
docs: ## Live-preview the documentation site
	cd $(DOCS) && { test -d node_modules || bun install; } && bun run dev

.PHONY: docs-build
docs-build: ## Build the static documentation site
	cd $(DOCS) && { test -d node_modules || bun install; } && bun run build

.PHONY: docs-deploy
docs-deploy: docs-build ## Build the docs and force-push them to the gh-pages branch
	@test -d $(DIST) || { echo "no docs build output at $(DIST)"; exit 1; }
	@# `.nojekyll` stops GitHub Pages from dropping VitePress's _-prefixed asset dirs.
	@touch $(DIST)/.nojekyll
	@# Publish from a throwaway repo inside the build output, so the working tree is
	@# untouched and gh-pages stays a single-commit, source-free artifact branch.
	@origin=$$(git remote get-url origin); \
		echo "==> publishing $(DIST) -> $$origin ($(GH_PAGES))"; \
		rm -rf $(DIST)/.git; \
		git -C $(DIST) init -q && \
		git -C $(DIST) checkout -q -b $(GH_PAGES) && \
		git -C $(DIST) add -A && \
		git -C $(DIST) commit -q -m "deploy docs" && \
		git -C $(DIST) push -f "$$origin" HEAD:$(GH_PAGES); \
		status=$$?; rm -rf $(DIST)/.git; \
		[ $$status -eq 0 ] && echo "==> done: https://archan937.github.io/rusm/" || exit $$status

.PHONY: publish-dry
publish-dry: ## Release pre-flight: dry-run every crate publish (no side effects)
	@for d in $(PUBLISH_ORDER); do \
		echo "==> dry-run $$d"; \
		( cd $$d && cargo publish --dry-run ) || { echo "✗ dry-run failed: $$d"; exit 1; }; \
	done
	@echo "==> all crates package cleanly"

.PHONY: publish-crates
publish-crates: ## Publish all crates to crates.io in dependency order
	@for d in $(PUBLISH_ORDER); do \
		echo "==> publish $$d"; \
		( cd $$d && cargo publish ) || { echo "✗ publish failed: $$d — fix, then re-run from here"; exit 1; }; \
	done

.PHONY: publish-npm
publish-npm: ## Publish the rusm-ts package to npm
	cd packages/rusm-ts && npm publish

.PHONY: publish-tags
publish-tags: ## Tag the release (v$(VERSION)) + the rusm-go submodule, push the tags
	git tag v$(VERSION)
	git tag packages/rusm-go/v$(VERSION)
	git push origin v$(VERSION) packages/rusm-go/v$(VERSION)

.PHONY: publish
publish: ## Full release v$(VERSION): crates.io + npm + tags (run `make publish-dry` first)
	@test -z "$$(git status --porcelain)" || { echo "✗ working tree not clean — commit first"; exit 1; }
	@git diff --quiet origin/main..HEAD 2>/dev/null || { echo "→ note: push commits to origin first so the tags point at pushed history"; }
	@echo "Publishing RUSM v$(VERSION) to crates.io + npm + git tags — Ctrl-C within 5s to abort."
	@sleep 5
	$(MAKE) publish-crates
	$(MAKE) publish-npm
	$(MAKE) publish-tags
	@echo "==> v$(VERSION) published. Now create the GitHub release from the CHANGELOG."

.PHONY: clean
clean: ## Remove Rust build artifacts
	cargo clean
