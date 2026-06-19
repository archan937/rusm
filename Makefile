.DEFAULT_GOAL := help

DASHBOARD := bench/dashboard
DOCS := docs
DIST := $(DOCS)/.vitepress/dist
GH_PAGES := gh-pages
SCENARIO ?= connection-storm
SECONDS ?= 5
EX ?= host_components
VERSION ?= 0.3.0

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

.PHONY: sync-templates
sync-templates: ## Regenerate rusm-cli/templates/ — the vendored copy of the example apps the scaffolder ships
	@for lang in typescript rust go; do \
		rsync -a --delete \
			--exclude target --exclude node_modules --exclude wasm \
			--exclude Cargo.lock --exclude bun.lock --exclude go.sum \
			--exclude data.redb --exclude package.json \
			examples/$$lang/ rusm-cli/templates/$$lang/; \
	done
	@# cargo drops any directory holding a literal Cargo.toml from the package tarball (it
	@# reads it as a nested package), so the Rust manifests are vendored as Cargo.toml.tmpl;
	@# template::files() strips the suffix back to Cargo.toml when scaffolding an app.
	@find rusm-cli/templates -name Cargo.toml -exec sh -c 'mv "$$1" "$$1.tmpl"' _ {} \;
	@echo "==> synced — \`cargo test -p rusm-cli template::\` guards against drift"

# Already on crates.io? (200 → that exact version exists; 404 → not yet.) The per-crate skip
# that makes publishing resumable: a failed run re-runs cleanly and an already-uploaded
# version is never re-sent. crates.io rejects requests without a User-Agent (403), so set one.
crate_published = curl -fsS -A "rusm-make-publish" "https://crates.io/api/v1/crates/$$(basename $1)/$(VERSION)" >/dev/null 2>&1

NPM_PKG := rusm-ts
NPM_DIR := packages/rusm-ts
# Is this exact version already on npm? (`npm view` is a public read — works logged out.)
npm_published = [ -n "$$(npm view $(NPM_PKG)@$(VERSION) version 2>/dev/null)" ]
# Logged in to npm? (npm publish on an existing package returns a misleading 404 when not.)
npm_authed = npm whoami >/dev/null 2>&1
npm_login_hint = run \`npm login\` (registry $$(npm config get registry)), then re-run

.PHONY: publish-dry
publish-dry: ## Release pre-flight: dry-run each not-yet-published crate (no side effects)
	@for d in $(PUBLISH_ORDER); do \
		if $(call crate_published,$$d); then echo "↷ $$(basename $$d)@$(VERSION) already published — skip"; continue; fi; \
		echo "==> dry-run $$d"; \
		out=$$( cd $$d && cargo publish --dry-run 2>&1 ); \
		if [ $$? -ne 0 ]; then \
			if echo "$$out" | grep -q 'failed to select a version for the requirement `rusm-'; then \
				echo "↷ $$(basename $$d): depends on a sibling crate not yet on crates.io at $(VERSION) — can't pre-verify; the topological \`publish-crates\` uploads it after its deps"; \
			else \
				echo "$$out"; echo "✗ dry-run failed: $$d"; exit 1; \
			fi; \
		fi; \
	done
	@echo "==> packages cleanly (sibling-dependent crates are verified during the topological publish)"

.PHONY: publish-crates
publish-crates: ## Publish crates to crates.io in dependency order (skips versions already uploaded)
	@for d in $(PUBLISH_ORDER); do \
		if $(call crate_published,$$d); then echo "↷ $$(basename $$d)@$(VERSION) already on crates.io — skip"; continue; fi; \
		echo "==> publish $$d"; \
		( cd $$d && cargo publish ) || { echo "✗ publish failed: $$d — fix, then re-run (published crates are skipped)"; exit 1; }; \
	done

.PHONY: publish-npm
publish-npm: ## Publish rusm-ts to npm (skips if already published; verifies login first)
	@if $(npm_published); then \
		echo "↷ $(NPM_PKG)@$(VERSION) already on npm — skip"; \
	else \
		$(npm_authed) || { echo "✗ not logged in to npm — $(npm_login_hint)"; exit 1; }; \
		( cd $(NPM_DIR) && npm publish ); \
	fi

.PHONY: publish-tags
publish-tags: ## Tag the release (v$(VERSION)) + the rusm-go submodule, push the tags
	@git rev-parse "v$(VERSION)" >/dev/null 2>&1 || git tag v$(VERSION)
	@git rev-parse "packages/rusm-go/v$(VERSION)" >/dev/null 2>&1 || git tag packages/rusm-go/v$(VERSION)
	git push origin v$(VERSION) packages/rusm-go/v$(VERSION)

.PHONY: publish
publish: ## Full release v$(VERSION): dry-run, then crates.io + npm + tags (resumable — published crates are skipped)
	@test -z "$$(git status --porcelain)" || { echo "✗ working tree not clean — commit first"; exit 1; }
	@git diff --quiet origin/main..HEAD 2>/dev/null || echo "→ note: push commits to origin first so the tags point at pushed history"
	@# Check npm login up front (unless rusm-ts is already published) — a missing login must
	@# abort BEFORE any crate is uploaded, not halfway through the release.
	@if $(npm_published); then :; else $(npm_authed) || { echo "✗ not logged in to npm — $(npm_login_hint) \`make publish\`"; exit 1; }; fi
	@echo "==> verifying packaging (dry-run of not-yet-published crates)…"
	@$(MAKE) --no-print-directory publish-dry
	@echo "==> dry-run clean. Uploading to crates.io + npm + git tags — Ctrl-C within 5s to abort."
	@sleep 5
	@$(MAKE) --no-print-directory publish-crates
	@$(MAKE) --no-print-directory publish-npm
	@$(MAKE) --no-print-directory publish-tags
	@echo "==> v$(VERSION) published. Now create the GitHub release from the CHANGELOG."

.PHONY: clean
clean: ## Remove Rust build artifacts
	cargo clean
