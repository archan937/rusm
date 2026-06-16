//! Locating a component's pre-built artifact — the generic "bring your own wasm"
//! path. Pure selection logic, kept here (not in `main.rs` glue) so it is unit-tested.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// The pre-built wasip2 `.wasm` for a component directory, if any.
///
/// Prefers the explicitly named `<name>.wasm`; otherwise accepts a single `.wasm`
/// in the directory, and **errors when several are present** (ambiguous — the caller
/// can't guess which to ship, so name the right one `<name>.wasm`). Returns
/// `Ok(None)` when the directory holds no `.wasm`, so the build can fall through to
/// the Rust/TS component kinds. The single-file path is sorted, so the choice is
/// deterministic regardless of directory order.
pub fn prebuilt_wasm(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    let named = dir.join(format!("{name}.wasm"));
    if named.is_file() {
        return Ok(Some(named));
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let mut wasms: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "wasm"))
        .collect();
    wasms.sort();

    match wasms.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        _ => {
            bail!("component `{name}` has multiple .wasm files — name the one to use `{name}.wasm`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), b"\0asm").unwrap();
    }

    #[test]
    fn prefers_the_name_matched_wasm() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "api.wasm");
        touch(dir.path(), "other.wasm");
        assert_eq!(
            prebuilt_wasm(dir.path(), "api").unwrap(),
            Some(dir.path().join("api.wasm"))
        );
    }

    #[test]
    fn falls_back_to_a_lone_wasm() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "built.wasm");
        assert_eq!(
            prebuilt_wasm(dir.path(), "api").unwrap(),
            Some(dir.path().join("built.wasm"))
        );
    }

    #[test]
    fn errors_when_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.wasm");
        touch(dir.path(), "b.wasm");
        assert!(prebuilt_wasm(dir.path(), "api").is_err());
    }

    #[test]
    fn none_when_no_wasm_present() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "notes.txt");
        assert_eq!(prebuilt_wasm(dir.path(), "api").unwrap(), None);
        // A missing directory is "no component here", not an error.
        assert_eq!(
            prebuilt_wasm(&dir.path().join("nope"), "api").unwrap(),
            None
        );
    }
}
