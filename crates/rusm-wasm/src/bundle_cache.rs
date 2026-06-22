//! Content-addressed compile cache for **dynamic** components (a `.wasm` bundle whose source
//! is only known at deploy/run time — see [`crate::WasmRuntime`]'s dynamic-WASM path).
//!
//! The expensive step is compiling + preparing a fetched bundle. This cache makes the **first**
//! spawn of a given bundle pay that cost (cold) and **every later** spawn hot — instantiate-only,
//! on the pooled fast path. Two TTL-bounded layers:
//!
//! - **`compiled`** — `content-hash → prepared component`, keyed by the SHA-256 of the bundle
//!   *bytes* (not the source string). Hashing the bytes is what makes it correct: identical
//!   bytes from any source share one compile, and a source that starts serving new bytes is a
//!   new key. `try_get_with` gives **single-flight** compilation — concurrent first-spawns of
//!   the same bundle compile once, the rest await. Evicted when idle past the TTL.
//! - **`fresh`** — `source → its last content-hash`, valid for the TTL. Within the TTL a hot
//!   spawn reads the hash here and hits `compiled` directly, **skipping the fetch entirely**.
//!
//! Generic over the prepared value `V` so the cache is unit-tested in isolation (no engine):
//! the runtime instantiates it as `BundleCache<PreparedComponent>`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use sha2::{Digest, Sha256};

/// The cache identity of a bundle: the SHA-256 of its bytes. The *only* correct key — see the
/// module docs. `Copy`, so it threads through the two cache layers without allocation.
pub(crate) type ContentHash = [u8; 32];

/// SHA-256 of `bytes` — the content hash that keys the compile cache.
pub(crate) fn content_hash(bytes: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// A content-addressed, TTL-bounded, single-flight compile cache (see module docs).
pub(crate) struct BundleCache<V> {
    /// content-hash → prepared value; single-flight compile, idle-evicted after the TTL.
    compiled: Cache<ContentHash, Arc<V>>,
    /// source → its last content-hash; lets a hot spawn skip the fetch. Re-fetch after the TTL.
    fresh: Cache<String, ContentHash>,
}

impl<V: Send + Sync + 'static> BundleCache<V> {
    /// A cache whose `ttl` is the **freshness window**: how long a fetched bundle is reused for
    /// a given source before the source is re-checked for changes, and how long a compiled
    /// artifact survives without use. A source spawned within the TTL is fully hot (no fetch,
    /// no compile); each access keeps its compiled artifact warm.
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            compiled: Cache::builder().time_to_idle(ttl).build(),
            fresh: Cache::builder().time_to_live(ttl).build(),
        }
    }

    /// Resolve `source` to a prepared value, doing the least work possible:
    ///
    /// - **Hot** (source fetched within the TTL and its compile still cached): returns the
    ///   cached value with **no `fetch`, no `prepare`**.
    /// - **Cold / stale**: `fetch` resolves the source to bytes, then `prepare` compiles them —
    ///   but only on a content-hash miss, so identical bytes (a new source, or unchanged bytes
    ///   after the TTL while the artifact is still warm) reuse the existing compile. Concurrent
    ///   compiles of the same bytes are single-flighted.
    ///
    /// `fetch` is invoked at most once per call (cold/stale only); `prepare` at most once per
    /// distinct content-hash across the whole cache.
    pub(crate) async fn get<F, Fut, P>(
        &self,
        source: &str,
        fetch: F,
        prepare: P,
    ) -> Result<Arc<V>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<u8>, String>>,
        P: FnOnce(&[u8]) -> Result<V, String>,
    {
        // Fully hot: the source is fresh and its compile is still warm.
        if let Some(hash) = self.fresh.get(source).await {
            if let Some(prepared) = self.compiled.get(&hash).await {
                return Ok(prepared);
            }
        }
        // Cold or stale: (re)fetch, hash, and compile only if these exact bytes aren't cached.
        let bytes = fetch().await?;
        let hash = content_hash(&bytes);
        let prepared = self
            .compiled
            .try_get_with(hash, async { prepare(&bytes).map(Arc::new) })
            .await
            .map_err(|e: Arc<String>| e.as_ref().clone())?;
        // Record the source → hash mapping only on success, so `fresh` never points at a hash
        // that failed to compile (a bad deploy surfaces the error without corrupting the cache;
        // a previously-good hash stays referenced and keeps serving until the next good bundle).
        self.fresh.insert(source.to_string(), hash).await;
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test harness: counts `fetch`/`prepare` calls so we can assert exactly what work the
    /// cache did. `fetch` returns the configured bytes; `prepare` echoes them as the value.
    struct Probe {
        fetches: AtomicUsize,
        prepares: AtomicUsize,
    }
    impl Probe {
        fn new() -> Self {
            Self {
                fetches: AtomicUsize::new(0),
                prepares: AtomicUsize::new(0),
            }
        }
        async fn get(&self, cache: &BundleCache<Vec<u8>>, source: &str, bytes: &[u8]) -> Vec<u8> {
            let bytes = bytes.to_vec();
            cache
                .get(
                    source,
                    || async {
                        self.fetches.fetch_add(1, Ordering::SeqCst);
                        Ok(bytes.clone())
                    },
                    |b| {
                        self.prepares.fetch_add(1, Ordering::SeqCst);
                        Ok(b.to_vec())
                    },
                )
                .await
                .map(|v| v.as_ref().clone())
                .unwrap()
        }
        fn counts(&self) -> (usize, usize) {
            (
                self.fetches.load(Ordering::SeqCst),
                self.prepares.load(Ordering::SeqCst),
            )
        }
    }

    #[test]
    fn content_hash_identifies_bytes_not_sources() {
        // The cache identity is the bytes: equal bytes → equal hash, different bytes → different.
        assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
        // SHA-256 is 32 bytes.
        assert_eq!(content_hash(b"").len(), 32);
    }

    #[tokio::test]
    async fn second_spawn_is_hot_no_fetch_no_compile() {
        let cache = BundleCache::new(Duration::from_secs(60));
        let probe = Probe::new();
        assert_eq!(probe.get(&cache, "api", b"v1").await, b"v1");
        assert_eq!(probe.counts(), (1, 1), "cold: one fetch, one compile");
        // Hot — within the TTL, the same source neither fetches nor compiles again.
        assert_eq!(probe.get(&cache, "api", b"v1").await, b"v1");
        assert_eq!(probe.get(&cache, "api", b"v1").await, b"v1");
        assert_eq!(probe.counts(), (1, 1), "hot: no extra fetch or compile");
    }

    #[tokio::test]
    async fn identical_bytes_from_two_sources_compile_once() {
        let cache = BundleCache::new(Duration::from_secs(60));
        let probe = Probe::new();
        // Two distinct sources serving the SAME bytes: each is fetched (different source), but
        // the content-hash dedups the expensive compile to one.
        probe.get(&cache, "url:a", b"same").await;
        probe.get(&cache, "url:b", b"same").await;
        assert_eq!(
            probe.counts(),
            (2, 1),
            "two fetches, one compile (content-hash dedup)"
        );
    }

    #[tokio::test]
    async fn changed_bytes_recompile_under_a_new_hash() {
        let cache = BundleCache::new(Duration::from_millis(60));
        let probe = Probe::new();
        probe.get(&cache, "api", b"v1").await;
        assert_eq!(probe.counts(), (1, 1));
        // Let the source go stale, then it serves new bytes → new hash → re-fetch + recompile.
        tokio::time::sleep(Duration::from_millis(90)).await;
        cache.fresh.run_pending_tasks().await;
        assert_eq!(probe.get(&cache, "api", b"v2").await, b"v2");
        assert_eq!(
            probe.counts(),
            (2, 2),
            "stale + changed → re-fetch and recompile"
        );
    }

    #[tokio::test]
    async fn stale_source_is_refetched_after_ttl() {
        let cache = BundleCache::new(Duration::from_millis(60));
        let probe = Probe::new();
        probe.get(&cache, "api", b"v1").await;
        assert_eq!(probe.counts().0, 1, "one fetch cold");
        tokio::time::sleep(Duration::from_millis(90)).await;
        cache.fresh.run_pending_tasks().await;
        // Source is re-checked after the freshness TTL.
        probe.get(&cache, "api", b"v1").await;
        assert_eq!(
            probe.counts().0,
            2,
            "re-fetched after the freshness TTL expired"
        );
    }

    #[tokio::test]
    async fn concurrent_first_spawns_compile_once_single_flight() {
        let cache: Arc<BundleCache<Vec<u8>>> = Arc::new(BundleCache::new(Duration::from_secs(60)));
        let prepares = Arc::new(AtomicUsize::new(0));
        // 16 concurrent first-spawns of the SAME source/bytes, each with a slow compile.
        let mut handles = Vec::new();
        for _ in 0..16 {
            let cache = Arc::clone(&cache);
            let prepares = Arc::clone(&prepares);
            handles.push(tokio::spawn(async move {
                cache
                    .get(
                        "api",
                        || async { Ok(b"payload".to_vec()) },
                        |b| {
                            prepares.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(20)); // slow compile
                            Ok(b.to_vec())
                        },
                    )
                    .await
                    .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(
            prepares.load(Ordering::SeqCst),
            1,
            "single-flight: compiled exactly once"
        );
    }

    #[tokio::test]
    async fn a_failed_compile_is_not_cached() {
        let cache = BundleCache::new(Duration::from_secs(60));
        let calls = AtomicUsize::new(0);
        let attempt = || {
            cache.get(
                "api",
                || async { Ok(b"x".to_vec()) },
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err::<Vec<u8>, String>("boom".to_string())
                },
            )
        };
        assert!(attempt().await.is_err(), "the error surfaces to the caller");
        assert!(attempt().await.is_err(), "and again — it was never cached");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a failed compile is retried, never cached"
        );
    }
}
