.DEFAULT_GOAL := help

DASHBOARD := bench/dashboard
DOCS := docs
DIST := $(DOCS)/.vitepress/dist
GH_PAGES := gh-pages
SCENARIO ?= connection-storm
SECONDS ?= 5
EX ?= headless_run

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
example: ## Run an example (EX=headless_run|synthetic_source|observer_overhead|embedded_node)
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

.PHONY: clean
clean: ## Remove Rust build artifacts
	cargo clean
