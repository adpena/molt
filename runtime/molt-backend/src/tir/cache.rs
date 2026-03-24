//! Incremental compilation cache for TIR functions.
//!
//! Provides a content-addressed cache so unchanged functions are not
//! recompiled between pipeline invocations.  Artifacts are stored on disk
//! under `cache_dir/functions/<hash>.bin`; a plain-text index file at
//! `cache_dir/index.txt` records metadata so the cache survives process
//! restarts.
//!
//! Index file format (one entry per non-blank, non-comment line):
//! ```text
//! # hash|artifact_path|dep1,dep2,...|last_access_unix_secs
//! abc123|functions/abc123.bin||1679900000
//! def456|functions/def456.bin|abc123|1679900001
//! ```

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
/// Key: hex-encoded hash of (function name + body bytes).
/// Value: cached compilation artifact stored both in-memory and on disk.
pub struct CompilationCache {
    /// Cache root directory (e.g. `.molt-cache/`).
    cache_dir: PathBuf,

    /// In-memory index: `content_hash` → [`CacheEntry`].
    index: HashMap<String, CacheEntry>,
}

/// A single entry in the compilation cache.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Hex-encoded hash of the function's content + type environment.
    pub content_hash: String,
    /// Path to the artifact file on disk.
    pub artifact_path: PathBuf,
    /// Hashes of callee functions this entry depends on.
    pub dependencies: Vec<String>,
    /// Unix timestamp (seconds) of last access.
    pub last_access: u64,
    /// Cached artifact bytes — `None` until loaded on demand.
    pub(crate) data: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// CompilationCache implementation
// ---------------------------------------------------------------------------

impl CompilationCache {
    /// Create or open a compilation cache rooted at `cache_dir`.
    ///
    /// Attempts to load the persisted index from disk; silently proceeds with
    /// an empty in-memory cache if the index does not exist or is unreadable.
    pub fn open(cache_dir: PathBuf) -> Self {
        let mut cache = Self {
            cache_dir,
            index: HashMap::new(),
        };
        cache.load_index();
        cache
    }

    /// Compute a content hash for a function.
    ///
    /// The hash is derived from `func_name` and `body` (serialised body
    /// bytes).  Uses [`DefaultHasher`] (SipHash-1-3) — not cryptographic,
    /// but stable within a single Rust binary version and sufficient for
    /// cache keys.
    pub fn compute_hash(func_name: &str, body: &[u8]) -> String {
        let mut h = DefaultHasher::new();
        func_name.hash(&mut h);
        body.hash(&mut h);
        format!("{:016x}", h.finish())
    }

    /// Look up a cached artifact by content hash.
    ///
    /// Updates `last_access` on a hit.  If the artifact is not in memory it
    /// is read from the path recorded in the index entry.  Returns `None`
    /// when the hash is absent or the on-disk file is missing/unreadable.
    pub fn get(&mut self, content_hash: &str) -> Option<Vec<u8>> {
        let now = unix_now();
        if let Some(entry) = self.index.get_mut(content_hash) {
            entry.last_access = now;
            // Return in-memory copy if available.
            if let Some(ref bytes) = entry.data {
                return Some(bytes.clone());
            }
            // Lazily load from disk.
            let path = entry.artifact_path.clone();
            if let Ok(bytes) = std::fs::read(&path) {
                entry.data = Some(bytes.clone());
                return Some(bytes);
            }
        }
        None
    }

    /// Store a compilation artifact in memory and on disk.
    ///
    /// Creates `cache_dir/functions/` if it does not exist.  If an entry
    /// with the same `content_hash` already exists it is silently replaced
    /// (idempotent for identical hashes).
    pub fn put(&mut self, content_hash: &str, artifact: &[u8], deps: Vec<String>) {
        let funcs_dir = self.cache_dir.join("functions");
        if std::fs::create_dir_all(&funcs_dir).is_err() {
            return; // can't create cache dir — skip caching silently
        }
        let artifact_path = funcs_dir.join(format!("{}.bin", content_hash));
        // Atomic write: write to temp file then rename to prevent partial writes
        let tmp_path = funcs_dir.join(format!("{}.bin.tmp", content_hash));
        if std::fs::write(&tmp_path, artifact).is_err() {
            return; // disk full or permission error — skip
        }
        if std::fs::rename(&tmp_path, &artifact_path).is_err() {
            let _ = std::fs::remove_file(&tmp_path); // cleanup temp
            return;
        }

        let entry = CacheEntry {
            content_hash: content_hash.to_owned(),
            artifact_path,
            dependencies: deps,
            last_access: unix_now(),
            data: Some(artifact.to_vec()),
        };
        self.index.insert(content_hash.to_owned(), entry);
    }

    /// Invalidate cache entries whose dependency hashes appear in
    /// `changed_hashes`.
    ///
    /// Any entry that lists a changed hash as a dependency is removed from
    /// the cache, triggering recompilation on next lookup.
    pub fn invalidate(&mut self, changed_hashes: &[String]) {
        let changed: std::collections::HashSet<&str> =
            changed_hashes.iter().map(String::as_str).collect();

        self.index.retain(|_hash, entry| {
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

    /// Persist the cache index to `cache_dir/index.txt`.
    ///
    /// Uses a simple pipe-delimited text format so no extra dependencies are
    /// required.  Silently ignores I/O errors (cache is advisory).
    pub fn save_index(&self) {
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let index_path = self.cache_dir.join("index.txt");

        let mut lines = String::from(
            "# molt cache index — hash|artifact_path|deps(comma-sep)|last_access_secs\n",
        );

        for entry in self.index.values() {
            let artifact_path_str = entry.artifact_path.to_string_lossy();
            let deps = entry.dependencies.join(",");
            lines.push_str(&format!(
                "{}|{}|{}|{}\n",
                entry.content_hash, artifact_path_str, deps, entry.last_access,
            ));
        }

        let _ = std::fs::write(&index_path, lines);
    }

    /// Load the cache index from `cache_dir/index.txt`.
    ///
    /// Artifacts are not read eagerly; they are loaded on demand by [`get`].
    /// Malformed or missing index files are silently ignored.
    pub fn load_index(&mut self) {
        let index_path = self.cache_dir.join("index.txt");
        let contents = match std::fs::read_to_string(&index_path) {
            Ok(c) => c,
            Err(_) => return,
        };

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() != 4 {
                continue;
            }
            let content_hash = parts[0].to_owned();
            let artifact_path = PathBuf::from(parts[1]);
            let dependencies: Vec<String> = if parts[2].is_empty() {
                vec![]
            } else {
                parts[2].split(',').map(str::to_owned).collect()
            };
            let last_access: u64 = parts[3].parse().unwrap_or(0);

            self.index.insert(
                content_hash.clone(),
                CacheEntry {
                    content_hash,
                    artifact_path,
                    dependencies,
                    last_access,
                    data: None, // loaded on demand
                },
            );
        }
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
    use std::sync::atomic::{AtomicU64, Ordering};

    // Use a unique temp directory per test run to avoid collisions.
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_cache_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("molt-cache-test-{}-{}", std::process::id(), n))
    }

    fn make_cache() -> CompilationCache {
        CompilationCache::open(tmp_cache_dir())
    }

    /// 1. put + get round-trip (in-memory)
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

        let dep_hash = CompilationCache::compute_hash("dep_func", b"dep body");
        let caller_hash = CompilationCache::compute_hash("caller_func", b"caller body");
        let other_hash = CompilationCache::compute_hash("other_func", b"other body");

        cache.put(&dep_hash, b"dep artifact", vec![]);
        cache.put(&caller_hash, b"caller artifact", vec![dep_hash.clone()]);
        cache.put(&other_hash, b"other artifact", vec![]);

        assert_eq!(cache.len(), 3);

        cache.invalidate(&[dep_hash.clone()]);

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

        cache.index.get_mut(&hash).unwrap().last_access = 1;

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

        cache.evict_stale(60);

        assert!(cache.get(&hash).is_some(), "recent entry must not be evicted");
    }

    /// 6. save_index + load_index round-trip
    #[test]
    fn test_disk_roundtrip() {
        let dir = tmp_cache_dir();

        // Write entries to disk.
        {
            let mut cache = CompilationCache::open(dir.clone());
            let h1 = CompilationCache::compute_hash("fn_a", b"body a");
            let h2 = CompilationCache::compute_hash("fn_b", b"body b");
            cache.put(&h1, b"artifact a", vec![]);
            cache.put(&h2, b"artifact b", vec![h1.clone()]);
            cache.save_index();
        }

        // Open a fresh cache from the same directory — index loads from disk.
        let mut cache2 = CompilationCache::open(dir);
        let h1 = CompilationCache::compute_hash("fn_a", b"body a");
        let h2 = CompilationCache::compute_hash("fn_b", b"body b");

        // Entries should be present (loaded lazily from disk).
        assert_eq!(cache2.get(&h1), Some(b"artifact a".to_vec()));
        assert_eq!(cache2.get(&h2), Some(b"artifact b".to_vec()));

        // Dependency metadata should survive.
        assert!(
            cache2.index[&h2].dependencies.contains(&h1),
            "dependency must be preserved across save/load"
        );
    }

    /// 7. get returns None when artifact file is missing even if index entry exists
    #[test]
    fn test_get_missing_artifact_file() {
        let dir = tmp_cache_dir();
        let mut cache = CompilationCache::open(dir);
        let hash = "deadbeef00000000".to_owned();
        // Insert an entry pointing at a non-existent file.
        cache.index.insert(
            hash.clone(),
            CacheEntry {
                content_hash: hash.clone(),
                artifact_path: PathBuf::from("/nonexistent/path/deadbeef.bin"),
                dependencies: vec![],
                last_access: unix_now(),
                data: None,
            },
        );
        assert_eq!(cache.get(&hash), None);
    }
}
