//! `rusm new <name> --template todo-board --lang ts|rust|go` — scaffold the full
//! collaborative todo board (HTTP CRUD + a live SSE feed + WebSocket chat + a service
//! driven by a worker), the same app the `examples/<lang>/` directories hold.
//!
//! **Single source of truth.** The file *contents* ARE the real example files, embedded
//! with `include_str!` — change the example, the template changes with it (a test asserts
//! the embedded set matches the on-disk example, so a new file can't be silently missed).
//! The one thing a standalone scaffolded app must differ in is its dependency manifests:
//! the examples use repo-local path/`replace` deps (they live in this tree), so those are
//! rewritten to published references on the way out.

use anyhow::{bail, Result};
use std::path::PathBuf;

use crate::scaffold::{package_json, Lang, SDK_VERSION};

/// The template to scaffold. One today (the todo board); the enum leaves room for more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    TodoBoard,
}

/// Parse the `--template` value.
pub fn parse_template(value: &str) -> Result<Template> {
    match value {
        "todo-board" | "todo" | "board" => Ok(Template::TodoBoard),
        other => bail!("unknown template `{other}` — the only template is `todo-board`"),
    }
}

/// `(relative path within the app, file contents)` for one example file, embedded from
/// `examples/<lang>/<path>` at build time.
macro_rules! embed {
    ($lang:literal, $($path:literal),+ $(,)?) => {
        &[ $( ($path, include_str!(concat!("../../examples/", $lang, "/", $path))) ),+ ]
    };
}

type Files = &'static [(&'static str, &'static str)];

// The embedded example trees, minus build artifacts (wasm/, node_modules, *.lock, go.sum)
// and minus files this module *generates* (the TS package.json — it carries the dep refs).
const TS_FILES: Files = embed!(
    "typescript",
    "rusm.toml",
    ".gitignore",
    "README.md",
    "tsconfig.json",
    "lib/todos.ts",
    "lib/page.ts",
    "components/api/index.ts",
    "components/api/index.test.ts",
    "components/chat/index.ts",
    "components/chat/index.test.ts",
    "components/feed/index.ts",
    "components/feed/index.test.ts",
    "components/reporter/index.ts",
    "components/reporter/index.test.ts",
    "components/store/index.ts",
    "components/store/index.test.ts",
);

const RS_FILES: Files = embed!(
    "rust",
    "rusm.toml",
    ".gitignore",
    "README.md",
    "todos/Cargo.toml",
    "todos/src/lib.rs",
    "store-svc/Cargo.toml",
    "store-svc/src/lib.rs",
    "components/api/Cargo.toml",
    "components/api/page.html",
    "components/api/src/lib.rs",
    "components/chat/Cargo.toml",
    "components/chat/src/lib.rs",
    "components/feed/Cargo.toml",
    "components/feed/src/lib.rs",
    "components/reporter/Cargo.toml",
    "components/reporter/src/lib.rs",
    "components/store/Cargo.toml",
    "components/store/src/lib.rs",
);

const GO_FILES: Files = embed!(
    "go",
    "rusm.toml",
    ".gitignore",
    "README.md",
    "shared/go.mod",
    "shared/todos/todos.go",
    "shared/store/store.go",
    "components/api/go.mod",
    "components/api/main.go",
    "components/api/page.html",
    "components/chat/go.mod",
    "components/chat/main.go",
    "components/feed/go.mod",
    "components/feed/main.go",
    "components/reporter/go.mod",
    "components/reporter/main.go",
    "components/store/go.mod",
    "components/store/main.go",
);

/// The full file set for a template app: the embedded example, with manifests rewritten to
/// published dependency references and (for TS) a generated `package.json`.
pub fn files(lang: Lang, name: &str) -> Vec<(PathBuf, String)> {
    let raw: Files = match lang {
        Lang::TypeScript => TS_FILES,
        Lang::Rust => RS_FILES,
        Lang::Go => GO_FILES,
        // Templates require a real guest language; `parse_new_args` rejects `generic`.
        Lang::Generic => &[],
    };

    let mut out: Vec<(PathBuf, String)> = raw
        .iter()
        .map(|(rel, content)| {
            let contents = if rel.ends_with("Cargo.toml") {
                publishify_cargo(content)
            } else if rel.ends_with("go.mod") {
                publishify_gomod(content)
            } else {
                (*content).to_string()
            };
            (PathBuf::from(rel), contents)
        })
        .collect();

    if lang == Lang::TypeScript {
        out.push((PathBuf::from("package.json"), package_json(name)));
    }
    out
}

/// Rewrite the repo-local `rusm-rs` path dependency to the published crate; intra-app path
/// deps (`todos`, `store-svc`) are left as-is — they're correct in a standalone app.
fn publishify_cargo(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        if line.trim_start().starts_with("rusm-rs = { path") {
            out.push_str(&format!("rusm-rs = \"{SDK_VERSION}\""));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Rewrite a `go.mod`: drop the repo-local `rusm-go` `replace` and pin the published
/// version; the intra-app `replace todoboard => ...` stays. Collapses the blank line the
/// dropped `replace` would leave behind.
fn publishify_gomod(src: &str) -> String {
    let kept: Vec<String> = src
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .starts_with("replace github.com/archan937/rusm/packages/rusm-go =>")
        })
        .map(|line| {
            line.replace(
                "github.com/archan937/rusm/packages/rusm-go v0.0.0",
                &format!("github.com/archan937/rusm/packages/rusm-go v{SDK_VERSION}"),
            )
        })
        .collect();

    // Collapse any blank-line run (the dropped `replace` can leave a double blank) and
    // trim trailing blanks, so the output is clean without needing `go mod tidy`.
    let mut out = String::with_capacity(src.len());
    let mut prev_blank = false;
    for line in kept.iter().map(|l| l.as_str()) {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        prev_blank = blank;
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Every source file of `examples/<lang>` (minus build artifacts and the files this
    /// module generates), as app-relative paths.
    fn on_disk_sources(lang: &str, generated: &[&str]) -> BTreeSet<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("examples")
            .join(lang);
        let mut set = BTreeSet::new();
        collect(&root, &root, &mut set);
        for g in generated {
            set.remove(*g);
        }
        set
    }

    fn collect(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | "node_modules" | "wasm") {
                    collect(base, &path, out);
                }
            } else if !matches!(
                name.as_str(),
                "Cargo.lock" | "bun.lock" | "go.sum" | "data.redb"
            ) {
                out.insert(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
    }

    /// The embedded set must equal the on-disk example, so an example that grows or loses a
    /// file fails here until the template list is updated — the single-source guard.
    #[test]
    fn embedded_set_matches_each_example() {
        for (lang, files, generated) in [
            ("typescript", TS_FILES, &["package.json"][..]),
            ("rust", RS_FILES, &[][..]),
            ("go", GO_FILES, &[][..]),
        ] {
            let embedded: BTreeSet<String> = files.iter().map(|(p, _)| (*p).to_string()).collect();
            assert_eq!(
                embedded,
                on_disk_sources(lang, generated),
                "{lang}: template file set drifted from examples/{lang}"
            );
        }
    }

    /// No scaffolded manifest may carry a repo-local dependency reference — the standalone
    /// app must depend on the published SDKs, not paths into this tree.
    #[test]
    fn manifests_have_no_repo_local_dep_refs() {
        for lang in [Lang::TypeScript, Lang::Rust, Lang::Go] {
            for (rel, content) in files(lang, "demo") {
                let p = rel.to_string_lossy();
                if p.ends_with("Cargo.toml") || p.ends_with("go.mod") || p.ends_with("package.json")
                {
                    assert!(
                        !content.contains("crates/rusm-rs"),
                        "{p}: leaks rusm-rs path"
                    );
                    assert!(
                        !content.contains("packages/rusm-go =>"),
                        "{p}: leaks rusm-go replace"
                    );
                    assert!(!content.contains("file:"), "{p}: leaks a file: dep");
                }
            }
        }
    }

    /// The published SDK refs the rewrite produces actually land.
    #[test]
    fn manifests_pin_published_sdks() {
        let rs: String = files(Lang::Rust, "demo")
            .into_iter()
            .find(|(p, _)| p.ends_with("components/api/Cargo.toml"))
            .map(|(_, c)| c)
            .unwrap();
        assert!(
            rs.contains(&format!("rusm-rs = \"{SDK_VERSION}\"")),
            "rust pins the published rusm-rs"
        );
        assert!(
            rs.contains("todos = { path"),
            "rust keeps the intra-app todos dep"
        );

        let go: String = files(Lang::Go, "demo")
            .into_iter()
            .find(|(p, _)| p.ends_with("components/api/go.mod"))
            .map(|(_, c)| c)
            .unwrap();
        assert!(
            go.contains(&format!("rusm-go v{SDK_VERSION}")),
            "go pins the published rusm-go"
        );
        assert!(
            go.contains("replace todoboard =>"),
            "go keeps the intra-app replace"
        );
    }
}
