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

const BACKEND_CACHE_NAMESPACE_VERSION: &str = "molt-backend-tir-cache-v1";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Content-addressed cache for compiled TIR functions.
///
/// Key: hex-encoded hash of (function name + body bytes).
/// Value: cached compilation artifact stored both in-memory and on disk.
pub struct CompilationCache {
    /// Cache root directory (e.g. `.molt_cache/`).
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

    /// Compute a content hash for a function body.
    ///
    /// This preserves the historical body-only contract for callers that do
    /// not need signature-sensitive cache invalidation.
    pub fn compute_hash(func_name: &str, body: &[u8]) -> String {
        Self::compute_hash_with_signature(func_name, &[], None, body)
    }

    /// Compute a content hash for a function.
    ///
    /// The hash is derived from the function signature surface plus the
    /// serialized body bytes so cache hits remain valid when only parameter
    /// metadata changes.
    pub fn compute_hash_with_signature(
        func_name: &str,
        params: &[String],
        param_types: Option<&[String]>,
        body: &[u8],
    ) -> String {
        let mut h = DefaultHasher::new();
        func_name.hash(&mut h);
        params.hash(&mut h);
        param_types.hash(&mut h);
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
            // Lazily load from disk.  Guard against partial/corrupted reads:
            // an empty file is treated as a cache miss (artifact writes are
            // atomic via rename, so an empty file means something went wrong).
            let path = entry.artifact_path.clone();
            match std::fs::read(&path) {
                Ok(bytes) if !bytes.is_empty() => {
                    entry.data = Some(bytes.clone());
                    return Some(bytes);
                }
                _ => {
                    // Missing, unreadable, or zero-length — treat as cache miss.
                    return None;
                }
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
        // Atomic write: write to PID-unique temp file then rename to prevent
        // partial reads and collisions between concurrent processes.
        let tmp_path = funcs_dir.join(format!("{}.bin.tmp.{}", content_hash, std::process::id()));
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
            !entry
                .dependencies
                .iter()
                .any(|d| changed.contains(d.as_str()))
        });
    }

    /// Remove entries that have not been accessed within the last
    /// `max_age_secs` seconds.
    pub fn evict_stale(&mut self, max_age_secs: u64) {
        let now = unix_now();
        self.index
            .retain(|_hash, entry| now.saturating_sub(entry.last_access) <= max_age_secs);
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

        // Atomic write: write to PID-unique temp file then rename so concurrent
        // readers never see a partially-written index.
        let tmp_path = self
            .cache_dir
            .join(format!("index.txt.tmp.{}", std::process::id()));
        if std::fs::write(&tmp_path, &lines).is_err() {
            return;
        }
        if std::fs::rename(&tmp_path, &index_path).is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
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

/// Resolve the canonical cache namespace for the current backend binary.
///
/// The namespace is rooted under `MOLT_CACHE` when configured, or `.molt_cache`
/// otherwise, and salted with the current executable path + mtime so cached
/// optimized IR is invalidated automatically when the backend binary changes.
pub fn backend_cache_dir() -> PathBuf {
    let root = std::env::var_os("MOLT_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".molt_cache"));
    let exe = std::env::current_exe().unwrap_or_default();
    let mtime = std::fs::metadata(&exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    backend_cache_dir_for(&root, &exe, mtime)
}

/// Resolve the backend cache namespace for an explicit root/executable/mtime.
///
/// This is testable and deterministic: identical inputs must produce the same
/// path, and changing either the executable path or mtime must invalidate the
/// namespace.
pub(crate) fn backend_cache_dir_for(root: &std::path::Path, exe: &std::path::Path, mtime: u64) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    BACKEND_CACHE_NAMESPACE_VERSION.hash(&mut hasher);
    exe.hash(&mut hasher);
    mtime.hash(&mut hasher);
    root.join(format!("{:016x}", hasher.finish()))
}

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
    use std::sync::{Mutex, OnceLock};

    // Use a unique temp directory per test run to avoid collisions.
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_cache_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("molt-cache-test-{}-{}", std::process::id(), n))
    }

    fn make_cache() -> CompilationCache {
        CompilationCache::open(tmp_cache_dir())
    }

    fn hash(func_name: &str, body: &[u8]) -> String {
        CompilationCache::compute_hash_with_signature(func_name, &[], None, body)
    }

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    /// 1. put + get round-trip (in-memory)
    #[test]
    fn test_put_get_roundtrip() {
        let mut cache = make_cache();
        let hash = hash("my_func", b"op1 op2 op3");
        let artifact = b"compiled artifact bytes";

        cache.put(&hash, artifact, vec![]);
        let result = cache.get(&hash);

        assert_eq!(result, Some(artifact.to_vec()));
    }

    #[test]
    fn backend_cache_dir_uses_env_root_and_version_namespace() {
        let _guard = env_lock().lock().expect("env lock");
        let root = tmp_cache_dir();
        unsafe { std::env::set_var("MOLT_CACHE", &root) };
        let dir = backend_cache_dir();
        unsafe { std::env::remove_var("MOLT_CACHE") };
        assert!(
            dir.starts_with(&root),
            "backend cache dir should live under configured MOLT_CACHE root: dir={dir:?} root={root:?}"
        );
        assert_ne!(
            dir, root,
            "backend cache dir should use a versioned namespace below the root"
        );
    }

    #[test]
    fn backend_cache_dir_for_is_deterministic_and_input_sensitive() {
        let root = tmp_cache_dir();
        let exe_a = PathBuf::from("/tmp/molt-backend-a");
        let exe_b = PathBuf::from("/tmp/molt-backend-b");
        let dir_a_1 = backend_cache_dir_for(&root, &exe_a, 111);
        let dir_a_2 = backend_cache_dir_for(&root, &exe_a, 111);
        let dir_b = backend_cache_dir_for(&root, &exe_b, 111);
        let dir_time = backend_cache_dir_for(&root, &exe_a, 222);

        assert_eq!(
            dir_a_1, dir_a_2,
            "same inputs must produce the same cache dir"
        );
        assert_ne!(dir_a_1, dir_b, "exe path must affect the cache namespace");
        assert_ne!(dir_a_1, dir_time, "mtime must affect the cache namespace");
        assert!(
            dir_a_1.starts_with(&root),
            "cache namespace must stay under the provided root"
        );
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

        let dep_hash = hash("dep_func", b"dep body");
        let caller_hash = hash("caller_func", b"caller body");
        let other_hash = hash("other_func", b"other body");

        cache.put(&dep_hash, b"dep artifact", vec![]);
        cache.put(&caller_hash, b"caller artifact", vec![dep_hash.clone()]);
        cache.put(&other_hash, b"other artifact", vec![]);

        assert_eq!(cache.len(), 3);

        cache.invalidate(std::slice::from_ref(&dep_hash));

        assert!(cache.get(&dep_hash).is_some(), "dep entry should remain");
        assert!(
            cache.get(&caller_hash).is_none(),
            "caller should be invalidated"
        );
        assert!(
            cache.get(&other_hash).is_some(),
            "unrelated entry should remain"
        );
    }

    /// 4. evict_stale removes old entries
    #[test]
    fn test_evict_stale() {
        let mut cache = make_cache();
        let hash = hash("old_func", b"old body");
        cache.put(&hash, b"old artifact", vec![]);

        cache.index.get_mut(&hash).unwrap().last_access = 1;

        cache.evict_stale(60);

        assert_eq!(cache.get(&hash), None);
    }

    /// 5. compute_hash produces consistent results for identical inputs
    #[test]
    fn test_compute_hash_consistent() {
        let h1 = hash("func_a", b"body bytes");
        let h2 = hash("func_a", b"body bytes");
        assert_eq!(h1, h2, "same inputs must produce the same hash");
    }

    /// compute_hash produces different results for different inputs
    #[test]
    fn test_compute_hash_different_inputs() {
        let h1 = hash("func_a", b"body A");
        let h2 = hash("func_a", b"body B");
        assert_ne!(h1, h2, "different bodies must produce different hashes");

        let h3 = hash("func_x", b"body A");
        let h4 = hash("func_y", b"body A");
        assert_ne!(h3, h4, "different names must produce different hashes");
    }

    #[test]
    fn test_compute_hash_with_signature_tracks_param_surface() {
        let params_a = vec!["lhs".to_string()];
        let params_b = vec!["rhs".to_string()];
        let types_a = vec!["module".to_string()];
        let types_b = vec!["bool".to_string()];

        let base = CompilationCache::compute_hash_with_signature("func", &params_a, None, b"body");
        let renamed =
            CompilationCache::compute_hash_with_signature("func", &params_b, None, b"body");
        let typed = CompilationCache::compute_hash_with_signature(
            "func",
            &params_a,
            Some(&types_a),
            b"body",
        );
        let typed_changed = CompilationCache::compute_hash_with_signature(
            "func",
            &params_a,
            Some(&types_b),
            b"body",
        );

        assert_ne!(base, renamed, "parameter names must affect the cache key");
        assert_ne!(base, typed, "parameter types must affect the cache key");
        assert_ne!(
            typed, typed_changed,
            "type metadata changes must invalidate cache"
        );
    }

    /// Stale eviction does NOT remove recently-accessed entries
    #[test]
    fn test_evict_stale_keeps_recent_entries() {
        let mut cache = make_cache();
        let hash = hash("recent_func", b"recent body");
        cache.put(&hash, b"recent artifact", vec![]);

        cache.evict_stale(60);

        assert!(
            cache.get(&hash).is_some(),
            "recent entry must not be evicted"
        );
    }

    /// 6. save_index + load_index round-trip
    #[test]
    fn test_disk_roundtrip() {
        let dir = tmp_cache_dir();

        // Write entries to disk.
        {
            let mut cache = CompilationCache::open(dir.clone());
            let h1 = hash("fn_a", b"body a");
            let h2 = hash("fn_b", b"body b");
            cache.put(&h1, b"artifact a", vec![]);
            cache.put(&h2, b"artifact b", vec![h1.clone()]);
            cache.save_index();
        }

        // Open a fresh cache from the same directory — index loads from disk.
        let mut cache2 = CompilationCache::open(dir);
        let h1 = hash("fn_a", b"body a");
        let h2 = hash("fn_b", b"body b");

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
