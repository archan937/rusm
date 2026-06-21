//! Discovery of an app's **custom bridges** — the `bridges/<name>/` directories an
//! application carries in its own repo, authored in the exact shape RUSM's own platform
//! bridges use (`bridge.wit` + `host.rs`). Presence *is* the declaration: a well-formed
//! `bridges/<name>/` is a bridge, no manifest entry required. Whether a *component* may
//! reach one is the separate, default-deny capability whitelist (`[capabilities.<name>]
//! bridges = [...]`, see `caps.rs`).
//!
//! Discovery is deliberately structural — it locates and validates directories; it does
//! not parse WIT. A bridge's `host.rs` encapsulates its own `bindgen!` + `add_to_linker`
//! (dogfooding the platform-bridge convention), so the toolchain never needs to understand
//! the contract, only to find it and hand it to the guest build + the generated host crate.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// A discovered custom bridge: its `name` (the directory name, which is also the bridge
/// name used in the capability whitelist) and the `dir` holding `bridge.wit` + `host.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeSpec {
    pub name: String,
    pub dir: PathBuf,
}

impl BridgeSpec {
    /// The WIT contract (`bridges/<name>/bridge.wit`) — vendored into each granted guest
    /// component's `wit/deps/` and into the generated host crate.
    pub fn wit(&self) -> PathBuf {
        self.dir.join("bridge.wit")
    }

    /// The native host impl (`bridges/<name>/host.rs`) — `impl <iface>::Host for BridgeHost`
    /// plus a `pub fn add_to_linker`, compiled into the generated host crate.
    pub fn host(&self) -> PathBuf {
        self.dir.join("host.rs")
    }
}

/// Discover the custom bridges under `<root>/bridges/`. Returns them sorted by name (a
/// stable order, so generated code is deterministic). No `bridges/` directory → no custom
/// bridges (an empty list, not an error — most apps have none). A `bridges/<name>/` that is
/// missing `bridge.wit` or `host.rs` is a **malformed** bridge and fails loudly, rather
/// than being silently skipped — a half-authored bridge is a mistake, not a non-bridge.
pub fn discover(root: &Path) -> Result<Vec<BridgeSpec>> {
    let dir = root.join("bridges");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut bridges = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue; // ignore stray files (e.g. a README) directly under bridges/
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let spec = BridgeSpec {
            name: name.clone(),
            dir: path,
        };
        if !spec.wit().is_file() {
            bail!("custom bridge `{name}` is missing bridges/{name}/bridge.wit");
        }
        if !spec.host().is_file() {
            bail!("custom bridge `{name}` is missing bridges/{name}/host.rs");
        }
        bridges.push(spec);
    }
    bridges.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(bridges)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway app dir under the system temp, removed first for a clean slate.
    fn app_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rusm-bridges-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_bridge(root: &Path, name: &str, wit: bool, host: bool) {
        let dir = root.join("bridges").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if wit {
            std::fs::write(dir.join("bridge.wit"), "package demo:bridge@0.1.0;\n").unwrap();
        }
        if host {
            std::fs::write(dir.join("host.rs"), "// host impl\n").unwrap();
        }
    }

    #[test]
    fn no_bridges_dir_is_empty_not_an_error() {
        let root = app_dir("none");
        assert!(discover(&root).unwrap().is_empty());
    }

    #[test]
    fn discovers_well_formed_bridges_sorted_by_name() {
        let root = app_dir("ok");
        write_bridge(&root, "weather", true, true);
        write_bridge(&root, "codec", true, true);
        // A stray file directly under bridges/ is ignored, not treated as a bridge.
        std::fs::write(root.join("bridges").join("README.md"), "x").unwrap();
        let found = discover(&root).unwrap();
        let names: Vec<&str> = found.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["codec", "weather"], "sorted, stray file ignored");
        assert_eq!(found[1].wit(), root.join("bridges/weather/bridge.wit"));
        assert_eq!(found[1].host(), root.join("bridges/weather/host.rs"));
    }

    #[test]
    fn a_malformed_bridge_fails_loudly() {
        let root = app_dir("malformed-wit");
        write_bridge(&root, "weather", false, true); // no bridge.wit
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("bridge.wit"), "names the missing file: {err}");

        let root = app_dir("malformed-host");
        write_bridge(&root, "weather", true, false); // no host.rs
        let err = discover(&root).unwrap_err().to_string();
        assert!(err.contains("host.rs"), "names the missing file: {err}");
    }
}
