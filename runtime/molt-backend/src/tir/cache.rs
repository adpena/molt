//! Incremental compilation cache for TIR functions.
//!
//! Provides a content-addressed in-memory cache so unchanged functions
//! are not recompiled between pipeline invocations within the same process.
//!
//! Phase 4 implementation: in-memory HashMap only (no disk persistence).
//! Content hashes are computed via `std::collections::hash_map::DefaultHasher`
//! (SipHash-1-3), which is not cryptographic but is sufficient for cache keys.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Content-addressed cache for compiled TIR functions.
///
/// Key: hex-encoded hash of (function name + type env bytes + body bytes).
/// Value: cached compilation artifact (raw bytes — TIR serialisation deferred).
pub struct CompilationCache {
    /// Cache directory (default: `.molt-cache/`).
    /// Reserved for future disk-persistence; not used in Phase 4.
    #[allow(dead_code)]
    cache_dir: PathBuf,

    /// In-memory index: `content_hash` → [`CacheEntry`].
    index: HashMap<String, CacheEntry>,
}

/// A single entry in the compilation cache.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Hex-encoded hash of the function's content + type environment.
    pub content_hash: String,
    /// Path to cached artifact on disk (reserved; not populated in Phase 4).
    pub artifact_path: PathBuf,
    /// Hashes of callee functions this entry depends on.
    pub dependencies: Vec<String>,
    /// Unix timestamp (seconds) of last access.
    pub last_access: u64,
    /// The cached artifact bytes.
    pub(crate) data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// CompilationCache implementation
// ---------------------------------------------------------------------------

impl CompilationCache {
    /// Create or open a compilation cache rooted at `cache_dir`.
    ///
    /// In Phase 4 no disk I/O is performed; the directory path is stored for
    /// future use.
    pub fn open(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            index: HashMap::new(),
        }
    }

    /// Compute a content hash for a function.
    ///
    /// The hash is derived from:
    /// - `func_name`: the fully-qualified function name.
    /// - `body`: serialised body bytes (ops, types, etc.).
    ///
    /// Uses [`DefaultHasher`] (SipHash-1-3) — not cryptographic, but stable
    /// within a single Rust binary version and sufficient for cache keys.
    pub fn compute_hash(func_name: &str, body: &[u8]) -> String {
        let mut h = DefaultHasher::new();
        func_name.hash(&mut h);
        body.hash(&mut h);
        format!("{:016x}", h.finish())
    }

    /// Look up a cached artifact by content hash.
    ///
    /// Updates `last_access` on a hit and returns a clone of the stored bytes.
    /// Returns `None` when the hash is absent from the cache.
    pub fn get(&mut self, content_hash: &str) -> Option<Vec<u8>> {
        let now = unix_now();
        if let Some(entry) = self.index.get_mut(content_hash) {
            entry.last_access = now;
            Some(entry.data.clone())
        } else {
            None
        }
    }

    /// Store a compilation artifact.
    ///
    /// If an entry with the same `content_hash` already exists it is silently
    /// replaced (idempotent for identical hashes).
    pub fn put(&mut self, content_hash: &str, artifact: &[u8], deps: Vec<String>) {
        let artifact_path = self
            .cache_dir
            .join("functions")
            .join(format!("{}.bin", content_hash));

        let entry = CacheEntry {
            content_hash: content_hash.to_owned(),
            artifact_path,
            dependencies: deps,
            last_access: unix_now(),
            data: artifact.to_vec(),
        };
        self.index.insert(content_hash.to_owned(), entry);
    }

    /// Invalidate cache entries whose dependency hashes appear in
    /// `changed_hashes`.
    ///
    /// Any entry that lists a changed hash as a dependency is removed from the
    /// cache, triggering recompilation on next lookup.
    pub fn invalidate(&mut self, changed_hashes: &[String]) {
        let changed: std::collections::HashSet<&str> =
            changed_hashes.iter().map(String::as_str).collect();

        self.index.retain(|_hash, entry| {
            // Keep entries whose dependency list does not overlap with the
            // changed set.
            !entry.dependencies.iter().any(|d| changed.contains(d.as_str()))
        });
    }

    /// Remove entries that have not been accessed within the last
    /// `max_age_secs` seconds.
    pub fn evict_stale(&mut self, max_age_secs: u64) {
        let now = unix_now();
        self.index.retain(|_hash, entry| {
            now.saturating_sub(entry.last_access) <= max_age_secs
        });
    }

    /// Persist the index to disk (no-op in Phase 4).
    ///
    /// Reserved for future disk-persistence implementation.
    pub fn save_index(&self) {
        // Phase 4: in-memory only — intentionally a no-op.
    }

    /// Load the index from disk (no-op in Phase 4).
    ///
    /// Reserved for future disk-persistence implementation.
    pub fn load_index(&mut self) {
        // Phase 4: in-memory only — intentionally a no-op.
    }

    /// Return the number of entries currently in the cache.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.index.len()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the current time as seconds since the Unix epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cache() -> CompilationCache {
        CompilationCache::open(PathBuf::from(".molt-cache"))
    }

    /// 1. put + get round-trip
    #[test]
    fn test_put_get_roundtrip() {
        let mut cache = make_cache();
        let hash = CompilationCache::compute_hash("my_func", b"op1 op2 op3");
        let artifact = b"compiled artifact bytes";

        cache.put(&hash, artifact, vec![]);
        let result = cache.get(&hash);

        assert_eq!(result, Some(artifact.to_vec()));
    }

    /// 2. get on missing key → None
    #[test]
    fn test_get_missing_key() {
        let mut cache = make_cache();
        assert_eq!(cache.get("nonexistent_hash"), None);
    }

    /// 3. invalidate removes dependent entries but not independent ones
    #[test]
    fn test_invalidate_removes_dependents() {
        let mut cache = make_cache();

        // dep_hash: a callee we will later mark as changed.
        let dep_hash = CompilationCache::compute_hash("dep_func", b"dep body");

        // caller depends on dep_hash.
        let caller_hash = CompilationCache::compute_hash("caller_func", b"caller body");

        // unrelated entry with no deps.
        let other_hash = CompilationCache::compute_hash("other_func", b"other body");

        cache.put(&dep_hash, b"dep artifact", vec![]);
        cache.put(&caller_hash, b"caller artifact", vec![dep_hash.clone()]);
        cache.put(&other_hash, b"other artifact", vec![]);

        assert_eq!(cache.len(), 3);

        // Invalidate because dep_hash changed.
        cache.invalidate(&[dep_hash.clone()]);

        // dep_hash itself has no dependency on dep_hash, so it stays.
        // caller_hash depends on dep_hash → must be evicted.
        // other_hash has no dependency on dep_hash → must stay.
        assert!(cache.get(&dep_hash).is_some(), "dep entry should remain");
        assert!(cache.get(&caller_hash).is_none(), "caller should be invalidated");
        assert!(cache.get(&other_hash).is_some(), "unrelated entry should remain");
    }

    /// 4. evict_stale removes old entries
    #[test]
    fn test_evict_stale() {
        let mut cache = make_cache();
        let hash = CompilationCache::compute_hash("old_func", b"old body");
        cache.put(&hash, b"old artifact", vec![]);

        // Force last_access to a very old timestamp (epoch + 1 second).
        cache.index.get_mut(&hash).unwrap().last_access = 1;

        // Evict anything not accessed in the last 60 seconds — epoch+1 is very old.
        cache.evict_stale(60);

        assert_eq!(cache.get(&hash), None);
    }

    /// 5. compute_hash produces consistent results for identical inputs
    #[test]
    fn test_compute_hash_consistent() {
        let h1 = CompilationCache::compute_hash("func_a", b"body bytes");
        let h2 = CompilationCache::compute_hash("func_a", b"body bytes");
        assert_eq!(h1, h2, "same inputs must produce the same hash");
    }

    /// compute_hash produces different results for different inputs
    #[test]
    fn test_compute_hash_different_inputs() {
        let h1 = CompilationCache::compute_hash("func_a", b"body A");
        let h2 = CompilationCache::compute_hash("func_a", b"body B");
        assert_ne!(h1, h2, "different bodies must produce different hashes");

        let h3 = CompilationCache::compute_hash("func_x", b"body A");
        let h4 = CompilationCache::compute_hash("func_y", b"body A");
        assert_ne!(h3, h4, "different names must produce different hashes");
    }

    /// Stale eviction does NOT remove recently-accessed entries
    #[test]
    fn test_evict_stale_keeps_recent_entries() {
        let mut cache = make_cache();
        let hash = CompilationCache::compute_hash("recent_func", b"recent body");
        cache.put(&hash, b"recent artifact", vec![]);

        // last_access is set to unix_now() during put — should survive eviction.
        cache.evict_stale(60);

        assert!(cache.get(&hash).is_some(), "recent entry must not be evicted");
    }
}
