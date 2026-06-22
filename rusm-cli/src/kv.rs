//! The `rusm kv` operator command: read and write the node's durable key-value store
//! from the shell. Its reason to exist is **publishing dynamic bundles** — a
//! `source = "kv:<bucket>/<key>"` (or a guest's `spawn-from "kv:…"`) needs the bytes to
//! be *in* the store first, and this is how an operator puts them there (a compiled
//! `.wasm` component, a JS bundle) without writing a component to do it. It also serves
//! for inspecting state.
//!
//! redb is a single-writer store, so this opens the file directly and therefore needs the
//! node **stopped** (a running `rusm serve`/`node` holds the lock). The arg parsing
//! ([`parse_kv`]) and the store actions ([`exec_kv`]) are split from the I/O glue in
//! `main.rs` so both are unit-tested.

use anyhow::{anyhow, Context, Result};
use rusm_kv::Store;

/// One parsed `rusm kv` action. The `<bucket>/<key>` operand is split once at boundary
/// (a key may itself contain `/`, e.g. `kv:plugins/v2/greeter`).
#[derive(Debug, PartialEq, Eq)]
pub enum KvCommand {
    /// `set <bucket>/<key> <file>` — store the file's bytes at the key.
    Set {
        bucket: String,
        key: String,
        file: String,
    },
    /// `get <bucket>/<key> [<out-file>]` — read the value; to a file, or to stdout.
    Get {
        bucket: String,
        key: String,
        out: Option<String>,
    },
    /// `list <bucket>` — list the keys in a bucket.
    List { bucket: String },
    /// `rm <bucket>/<key>` — delete a key.
    Remove { bucket: String, key: String },
}

/// What [`exec_kv`] produced for `main` to emit: a human line (printed with a newline)
/// or raw bytes (a `get` to stdout, written verbatim so binary survives a pipe).
#[derive(Debug, PartialEq, Eq)]
pub enum KvOutput {
    Message(String),
    Bytes(Vec<u8>),
}

/// Parse a `rusm kv` invocation from its `action` and remaining `operands`. Errors (exit
/// 2) on an unknown action or the wrong operand count — the misuse is caught before any
/// store is opened.
pub fn parse_kv(action: &str, operands: &[String]) -> Result<KvCommand> {
    let need = |n: usize| -> Result<()> {
        if operands.len() == n {
            Ok(())
        } else {
            Err(anyhow!(
                "`rusm kv {action}` takes {n} argument(s), got {}",
                operands.len()
            ))
        }
    };
    match action {
        "set" => {
            need(2)?;
            let (bucket, key) = split_ref(&operands[0])?;
            Ok(KvCommand::Set {
                bucket,
                key,
                file: operands[1].clone(),
            })
        }
        "get" => {
            if operands.is_empty() || operands.len() > 2 {
                return Err(anyhow!("`rusm kv get` takes <bucket>/<key> [<out-file>]"));
            }
            let (bucket, key) = split_ref(&operands[0])?;
            Ok(KvCommand::Get {
                bucket,
                key,
                out: operands.get(1).cloned(),
            })
        }
        "list" => {
            need(1)?;
            let bucket = operands[0].trim();
            if bucket.is_empty() {
                return Err(anyhow!("`rusm kv list` needs a bucket name"));
            }
            Ok(KvCommand::List {
                bucket: bucket.to_string(),
            })
        }
        "rm" | "delete" => {
            need(1)?;
            let (bucket, key) = split_ref(&operands[0])?;
            Ok(KvCommand::Remove { bucket, key })
        }
        other => Err(anyhow!(
            "unknown `rusm kv` action `{other}` (use set | get | list | rm)"
        )),
    }
}

/// Split a `<bucket>/<key>` operand. The bucket is everything before the first `/`; the
/// key is the (possibly `/`-containing) remainder. Both must be non-empty.
fn split_ref(spec: &str) -> Result<(String, String)> {
    let (bucket, key) = spec
        .split_once('/')
        .ok_or_else(|| anyhow!("expected `<bucket>/<key>`, got {spec:?}"))?;
    if bucket.is_empty() || key.is_empty() {
        return Err(anyhow!("both bucket and key must be non-empty in {spec:?}"));
    }
    Ok((bucket.to_string(), key.to_string()))
}

/// Run a parsed [`KvCommand`] against an open `store`, returning what to emit. Pure store
/// and filesystem I/O — no argument parsing, no process exit — so it is driven directly in
/// tests against a tempfile store.
pub fn exec_kv(store: &Store, command: KvCommand) -> Result<KvOutput> {
    match command {
        KvCommand::Set { bucket, key, file } => {
            let bytes = std::fs::read(&file).with_context(|| format!("reading {file}"))?;
            let n = bytes.len();
            store
                .bucket(&bucket)
                .set(&key, &bytes)
                .map_err(|e| anyhow!("writing {bucket}/{key}: {e}"))?;
            Ok(KvOutput::Message(format!("set {bucket}/{key} ({n} bytes)")))
        }
        KvCommand::Get { bucket, key, out } => {
            let bytes = store
                .bucket(&bucket)
                .get(&key)
                .map_err(|e| anyhow!("reading {bucket}/{key}: {e}"))?
                .ok_or_else(|| anyhow!("no value at {bucket}/{key}"))?;
            match out {
                Some(path) => {
                    let n = bytes.len();
                    std::fs::write(&path, &bytes).with_context(|| format!("writing {path}"))?;
                    Ok(KvOutput::Message(format!("wrote {n} bytes to {path}")))
                }
                None => Ok(KvOutput::Bytes(bytes)),
            }
        }
        KvCommand::List { bucket } => {
            let keys = store
                .bucket(&bucket)
                .list()
                .map_err(|e| anyhow!("listing {bucket}: {e}"))?;
            Ok(KvOutput::Message(if keys.is_empty() {
                format!("(bucket `{bucket}` is empty)")
            } else {
                keys.join("\n")
            }))
        }
        KvCommand::Remove { bucket, key } => {
            let existed = store
                .bucket(&bucket)
                .delete(&key)
                .map_err(|e| anyhow!("deleting {bucket}/{key}: {e}"))?;
            Ok(KvOutput::Message(if existed {
                format!("removed {bucket}/{key}")
            } else {
                format!("no value at {bucket}/{key}")
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("kv.redb")).unwrap();
        (dir, store)
    }

    #[test]
    fn parses_each_action() {
        assert_eq!(
            parse_kv("set", &["b/k".into(), "f.wasm".into()]).unwrap(),
            KvCommand::Set {
                bucket: "b".into(),
                key: "k".into(),
                file: "f.wasm".into()
            }
        );
        assert_eq!(
            parse_kv("get", &["b/k".into()]).unwrap(),
            KvCommand::Get {
                bucket: "b".into(),
                key: "k".into(),
                out: None
            }
        );
        assert_eq!(
            parse_kv("get", &["b/k".into(), "out".into()]).unwrap(),
            KvCommand::Get {
                bucket: "b".into(),
                key: "k".into(),
                out: Some("out".into())
            }
        );
        assert_eq!(
            parse_kv("list", &["b".into()]).unwrap(),
            KvCommand::List { bucket: "b".into() }
        );
        assert_eq!(
            parse_kv("rm", &["b/k".into()]).unwrap(),
            KvCommand::Remove {
                bucket: "b".into(),
                key: "k".into()
            }
        );
        // `delete` is an alias for `rm`.
        assert_eq!(
            parse_kv("delete", &["b/k".into()]).unwrap(),
            parse_kv("rm", &["b/k".into()]).unwrap()
        );
    }

    #[test]
    fn a_key_may_contain_slashes() {
        // Only the first `/` separates bucket from key, so versioned keys work.
        assert_eq!(
            parse_kv("set", &["plugins/v2/greeter".into(), "f".into()]).unwrap(),
            KvCommand::Set {
                bucket: "plugins".into(),
                key: "v2/greeter".into(),
                file: "f".into()
            }
        );
    }

    #[test]
    fn rejects_misuse() {
        assert!(
            parse_kv("set", &["b/k".into()]).is_err(),
            "set needs a file"
        );
        assert!(parse_kv("get", &[]).is_err(), "get needs a ref");
        assert!(parse_kv("get", &["a".into(), "b".into(), "c".into()]).is_err());
        assert!(parse_kv("list", &[]).is_err());
        assert!(
            parse_kv("set", &["nokey".into(), "f".into()]).is_err(),
            "ref needs a /"
        );
        assert!(
            parse_kv("set", &["/k".into(), "f".into()]).is_err(),
            "empty bucket"
        );
        assert!(
            parse_kv("set", &["b/".into(), "f".into()]).is_err(),
            "empty key"
        );
        assert!(parse_kv("frob", &[]).is_err(), "unknown action");
    }

    #[test]
    fn set_then_get_round_trips_bytes() {
        let (dir, store) = store();
        let file = dir.path().join("bundle.wasm");
        std::fs::write(&file, b"\0asm\x0d\x00\x01\x00payload").unwrap();
        let msg = exec_kv(
            &store,
            KvCommand::Set {
                bucket: "plugins".into(),
                key: "greeter".into(),
                file: file.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
        assert_eq!(
            msg,
            KvOutput::Message("set plugins/greeter (15 bytes)".into())
        );
        // Get to stdout returns the exact bytes…
        assert_eq!(
            exec_kv(
                &store,
                KvCommand::Get {
                    bucket: "plugins".into(),
                    key: "greeter".into(),
                    out: None
                }
            )
            .unwrap(),
            KvOutput::Bytes(b"\0asm\x0d\x00\x01\x00payload".to_vec())
        );
        // …and get to a file writes them.
        let out = dir.path().join("out.wasm");
        exec_kv(
            &store,
            KvCommand::Get {
                bucket: "plugins".into(),
                key: "greeter".into(),
                out: Some(out.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"\0asm\x0d\x00\x01\x00payload"
        );
    }

    #[test]
    fn list_and_remove() {
        let (dir, store) = store();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let f = || KvCommand::Set {
            bucket: "b".into(),
            key: "k".into(),
            file: file.to_string_lossy().into_owned(),
        };
        assert_eq!(
            exec_kv(&store, KvCommand::List { bucket: "b".into() }).unwrap(),
            KvOutput::Message("(bucket `b` is empty)".into())
        );
        exec_kv(&store, f()).unwrap();
        assert_eq!(
            exec_kv(&store, KvCommand::List { bucket: "b".into() }).unwrap(),
            KvOutput::Message("k".into())
        );
        assert_eq!(
            exec_kv(
                &store,
                KvCommand::Remove {
                    bucket: "b".into(),
                    key: "k".into()
                }
            )
            .unwrap(),
            KvOutput::Message("removed b/k".into())
        );
        // A second remove reports the key is already gone (idempotent, not an error).
        assert_eq!(
            exec_kv(
                &store,
                KvCommand::Remove {
                    bucket: "b".into(),
                    key: "k".into()
                }
            )
            .unwrap(),
            KvOutput::Message("no value at b/k".into())
        );
    }

    #[test]
    fn get_a_missing_key_is_an_error() {
        let (_dir, store) = store();
        assert!(exec_kv(
            &store,
            KvCommand::Get {
                bucket: "b".into(),
                key: "absent".into(),
                out: None
            }
        )
        .is_err());
    }

    #[test]
    fn set_a_missing_file_is_an_error() {
        let (_dir, store) = store();
        assert!(exec_kv(
            &store,
            KvCommand::Set {
                bucket: "b".into(),
                key: "k".into(),
                file: "/no/such/file".into()
            }
        )
        .is_err());
    }
}
