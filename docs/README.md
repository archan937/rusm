# RUSM documentation site

The RUSM documentation — a [VitePress](https://vitepress.dev/) site, published at
**https://archan937.github.io/rusm/**. This directory is the source; the rendered pages are
what users read.

## Preview & build

```sh
cd docs
bun install
bun run dev            # local preview with hot reload (Bun — never Node.js)
bun run build          # build the static site into docs/.vitepress/dist
```

From the repo root, `make docs` previews and `make docs-build` builds.

## Structure

- **Content** lives in per-section folders: `introduction/`, `build-an-app/`, `deep-dive/`,
  `about/`, `phases/`. Each `.md` file is one page; the filename matches the page title.
- **Navigation is single-sourced** in `.vitepress/config.ts` — the one `sections` array drives
  both the top nav and the sidebar, so they never diverge. Add a page by creating its `.md`
  **and** adding it to that array.
- `index.md` is the landing page.

## Conventions

- Every **guest/application** code snippet shows all three languages (TypeScript / Rust / Go)
  in a `::: code-group`. Host-side Rust (embedding, cluster, OTP core, bridge `host.rs`),
  config TOML, shell, and WIT are single-language.
- `bun run build` fails on dead internal links — keep links valid.
- Internal links are root-relative (e.g. `/build-an-app/serve-http`); `cleanUrls` is on, so
  omit the `.md`.

## Deploy

`make docs-deploy` builds and force-pushes the static site to the `gh-pages` branch. It is a
**manual** step — there is no CI auto-deploy — so docs ship when a maintainer runs it.
