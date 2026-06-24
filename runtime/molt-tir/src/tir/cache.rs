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

use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const BACKEND_CACHE_NAMESPACE_VERSION: &str = "molt-backend-tir-cache-v2-exception-regions";
const DEFAULT_MEMORY_CACHE_BYTES_FALLBACK: usize = 64 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_AVAILABLE_BYTES_MIN: usize = 8 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_BYTES_MIN: usize = 32 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_BYTES_MAX: usize = 512 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_AVAILABLE_DIVISOR: usize = 128;
const DEFAULT_MEMORY_CACHE_TOTAL_DIVISOR: usize = 512;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Content-addressed cache for compiled TIR functions.
///
/// Key: hex-encoded hash of (cache namespace + function signature + body bytes).
/// Value: cached compilation artifact stored both in-memory and on disk.
pub struct CompilationCache {
    /// Cache root directory (e.g. `.molt_cache/`).
    cache_dir: PathBuf,

    /// In-memory index: `content_hash` → [`CacheEntry`].
    index: HashMap<String, CacheEntry>,

    /// Bytes currently retained in `CacheEntry::data`.
    memory_bytes: usize,

    /// Maximum bytes retained in-memory. Disk cache entries remain indexed.
    max_memory_bytes: usize,

    /// Monotonic logical clock for in-memory LRU eviction.
    memory_clock: u64,

    /// LRU queue ordered by `(memory_stamp, content_hash)`.
    memory_order: BinaryHeap<Reverse<(u64, String)>>,
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
    /// Logical LRU stamp for in-memory artifact bytes.
    pub(crate) memory_stamp: u64,
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
        Self::open_with_memory_limit(cache_dir, default_memory_cache_limit_bytes())
    }

    /// Create or open a compilation cache with an explicit in-memory cap.
    pub fn open_with_memory_limit(cache_dir: PathBuf, max_memory_bytes: usize) -> Self {
        let mut cache = Self {
            cache_dir,
            index: HashMap::new(),
            memory_bytes: 0,
            max_memory_bytes,
            memory_clock: 0,
            memory_order: BinaryHeap::new(),
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
        BACKEND_CACHE_NAMESPACE_VERSION.hash(&mut h);
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
        } else {
            return None;
        }

        if let Some(bytes) = self
            .index
            .get(content_hash)
            .and_then(|entry| entry.data.clone())
        {
            self.touch_memory_entry(content_hash);
            return Some(bytes);
        }

        // Lazily load from disk. Guard against partial/corrupted reads:
        // an empty file is treated as a cache miss (artifact writes are
        // atomic via rename, so an empty file means something went wrong).
        let path = self.index.get(content_hash)?.artifact_path.clone();
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => {
                self.store_memory_data(content_hash, bytes.clone());
                Some(bytes)
            }
            _ => {
                // Missing, unreadable, or zero-length — treat as cache miss.
                None
            }
        }
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

        if let Some(previous) = self.index.remove(content_hash)
            && let Some(bytes) = previous.data
        {
            self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
        }

        let mut entry = CacheEntry {
            content_hash: content_hash.to_owned(),
            artifact_path,
            dependencies: deps,
            last_access: unix_now(),
            data: None,
            memory_stamp: 0,
        };
        if artifact.len() <= self.max_memory_bytes && self.max_memory_bytes > 0 {
            self.memory_clock = self.memory_clock.wrapping_add(1);
            entry.memory_stamp = self.memory_clock;
            entry.data = Some(artifact.to_vec());
            self.memory_bytes = self.memory_bytes.saturating_add(artifact.len());
            self.memory_order
                .push(Reverse((entry.memory_stamp, content_hash.to_owned())));
        }
        self.index.insert(content_hash.to_owned(), entry);
        self.evict_memory();
    }

    /// Invalidate cache entries whose dependency hashes appear in
    /// `changed_hashes`.
    ///
    /// Any entry that lists a changed hash as a dependency is removed from
    /// the cache, triggering recompilation on next lookup.
    pub fn invalidate(&mut self, changed_hashes: &[String]) {
        let changed: std::collections::HashSet<&str> =
            changed_hashes.iter().map(String::as_str).collect();

        let mut removed_bytes = 0usize;
        self.index.retain(|_hash, entry| {
            let keep = !entry
                .dependencies
                .iter()
                .any(|d| changed.contains(d.as_str()));
            if !keep && let Some(bytes) = &entry.data {
                removed_bytes = removed_bytes.saturating_add(bytes.len());
            }
            keep
        });
        self.memory_bytes = self.memory_bytes.saturating_sub(removed_bytes);
        self.compact_memory_order_if_needed();
    }

    /// Remove entries that have not been accessed within the last
    /// `max_age_secs` seconds.
    pub fn evict_stale(&mut self, max_age_secs: u64) {
        let now = unix_now();
        let mut removed_bytes = 0usize;
        self.index.retain(|_hash, entry| {
            let keep = now.saturating_sub(entry.last_access) <= max_age_secs;
            if !keep && let Some(bytes) = &entry.data {
                removed_bytes = removed_bytes.saturating_add(bytes.len());
            }
            keep
        });
        self.memory_bytes = self.memory_bytes.saturating_sub(removed_bytes);
        self.compact_memory_order_if_needed();
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
                    memory_stamp: 0,
                },
            );
        }
    }

    /// Return the number of entries currently in the cache.
    #[cfg(any(test, feature = "test-util"))]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Return whether the cache currently contains no entries.
    #[cfg(any(test, feature = "test-util"))]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes
    }

    fn touch_memory_entry(&mut self, content_hash: &str) {
        let Some(entry) = self.index.get_mut(content_hash) else {
            return;
        };
        if entry.data.is_none() {
            return;
        }
        self.memory_clock = self.memory_clock.wrapping_add(1);
        entry.memory_stamp = self.memory_clock;
        self.memory_order
            .push(Reverse((entry.memory_stamp, content_hash.to_owned())));
    }

    fn store_memory_data(&mut self, content_hash: &str, bytes: Vec<u8>) {
        if self.max_memory_bytes == 0 || bytes.len() > self.max_memory_bytes {
            if let Some(entry) = self.index.get_mut(content_hash)
                && let Some(previous) = entry.data.take()
            {
                self.memory_bytes = self.memory_bytes.saturating_sub(previous.len());
                entry.memory_stamp = 0;
            }
            return;
        }

        let Some(entry) = self.index.get_mut(content_hash) else {
            return;
        };
        if let Some(previous) = entry.data.take() {
            self.memory_bytes = self.memory_bytes.saturating_sub(previous.len());
        }
        self.memory_clock = self.memory_clock.wrapping_add(1);
        entry.memory_stamp = self.memory_clock;
        self.memory_bytes = self.memory_bytes.saturating_add(bytes.len());
        entry.data = Some(bytes);
        self.memory_order
            .push(Reverse((entry.memory_stamp, content_hash.to_owned())));
        self.evict_memory();
    }

    fn evict_memory(&mut self) {
        while self.memory_bytes > self.max_memory_bytes {
            let Some(Reverse((stamp, content_hash))) = self.memory_order.pop() else {
                break;
            };
            let is_live = self
                .index
                .get(&content_hash)
                .is_some_and(|entry| entry.memory_stamp == stamp && entry.data.is_some());
            if !is_live {
                continue;
            }
            if let Some(entry) = self.index.get_mut(&content_hash)
                && let Some(bytes) = entry.data.take()
            {
                self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
                entry.memory_stamp = 0;
            }
        }
        self.compact_memory_order_if_needed();
    }

    fn compact_memory_order_if_needed(&mut self) {
        if self.memory_order.len() <= self.index.len().saturating_mul(8).saturating_add(32) {
            return;
        }
        let mut compacted = BinaryHeap::new();
        for (hash, entry) in &self.index {
            if entry.data.is_some() {
                compacted.push(Reverse((entry.memory_stamp, hash.clone())));
            }
        }
        self.memory_order = compacted;
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
pub(crate) fn backend_cache_dir_for(
    root: &std::path::Path,
    exe: &std::path::Path,
    mtime: u64,
) -> PathBuf {
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

fn default_memory_cache_limit_bytes() -> usize {
    if let Some(bytes) = env_cache_limit_bytes("MOLT_BACKEND_TIR_CACHE_MEMORY_BYTES") {
        return bytes;
    }
    if let Some(mib) = env_cache_limit_bytes("MOLT_BACKEND_TIR_CACHE_MEMORY_MB") {
        return mib.saturating_mul(1024 * 1024);
    }
    if let Some(bytes) = usable_memory_budget_bytes_from_env() {
        return (bytes / DEFAULT_MEMORY_CACHE_AVAILABLE_DIVISOR).clamp(
            DEFAULT_MEMORY_CACHE_AVAILABLE_BYTES_MIN,
            DEFAULT_MEMORY_CACHE_BYTES_MAX,
        );
    }
    total_memory_bytes()
        .map(|bytes| {
            (bytes / DEFAULT_MEMORY_CACHE_TOTAL_DIVISOR).clamp(
                DEFAULT_MEMORY_CACHE_BYTES_MIN,
                DEFAULT_MEMORY_CACHE_BYTES_MAX,
            )
        })
        .unwrap_or(DEFAULT_MEMORY_CACHE_BYTES_FALLBACK)
}

fn env_cache_limit_bytes(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
}

fn usable_memory_budget_bytes_from_env() -> Option<usize> {
    let available_gb = env_cache_limit_gb(&[
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEM_AVAILABLE_GB",
        "MOLT_MEMORY_AVAILABLE_GB",
        "MOLT_MEM_AVAILABLE_GB",
    ])?;
    let reserve_gb = env_cache_limit_gb(&[
        "MOLT_BACKEND_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEM_RESERVE_GB",
        "MOLT_MEMORY_RESERVE_GB",
        "MOLT_MEM_RESERVE_GB",
    ])
    .unwrap_or(0.0);
    let usable_gb = (available_gb - reserve_gb).max(0.0);
    if usable_gb <= 0.0 {
        return Some(0);
    }
    Some((usable_gb * 1024.0 * 1024.0 * 1024.0) as usize)
}

fn env_cache_limit_gb(names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
    })
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn total_memory_bytes() -> Option<usize> {
    unsafe {
        let pages = libc::sysconf(libc::_SC_PHYS_PAGES);
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        if pages <= 0 || page_size <= 0 {
            return None;
        }
        (pages as usize).checked_mul(page_size as usize)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn total_memory_bytes() -> Option<usize> {
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
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

    fn make_cache_with_memory_limit(max_memory_bytes: usize) -> CompilationCache {
        CompilationCache::open_with_memory_limit(tmp_cache_dir(), max_memory_bytes)
    }

    fn hash(func_name: &str, body: &[u8]) -> String {
        CompilationCache::compute_hash_with_signature(func_name, &[], None, body)
    }

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    const CACHE_ENV_NAMES: &[&str] = &[
        "MOLT_BACKEND_TIR_CACHE_MEMORY_BYTES",
        "MOLT_BACKEND_TIR_CACHE_MEMORY_MB",
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEM_AVAILABLE_GB",
        "MOLT_MEMORY_AVAILABLE_GB",
        "MOLT_MEM_AVAILABLE_GB",
        "MOLT_BACKEND_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEM_RESERVE_GB",
        "MOLT_MEMORY_RESERVE_GB",
        "MOLT_MEM_RESERVE_GB",
    ];

    struct EnvRestore {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvRestore {
        fn apply(updates: &[(&'static str, &'static str)]) -> Self {
            let saved = CACHE_ENV_NAMES
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect::<Vec<_>>();
            unsafe {
                for name in CACHE_ENV_NAMES {
                    std::env::remove_var(name);
                }
                for (name, value) in updates {
                    std::env::set_var(name, value);
                }
            }
            Self { saved }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                for (name, value) in &self.saved {
                    if let Some(value) = value {
                        std::env::set_var(name, value);
                    } else {
                        std::env::remove_var(name);
                    }
                }
            }
        }
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
    fn memory_cache_evicts_lru_bytes_without_dropping_disk_index() {
        let mut cache = make_cache_with_memory_limit(8);
        let h1 = hash("fn_1", b"body 1");
        let h2 = hash("fn_2", b"body 2");
        let h3 = hash("fn_3", b"body 3");

        cache.put(&h1, b"1111", vec![]);
        cache.put(&h2, b"2222", vec![]);
        assert_eq!(cache.memory_bytes(), 8);

        cache.put(&h3, b"3333", vec![]);
        assert_eq!(
            cache.len(),
            3,
            "memory eviction must preserve disk index entries"
        );
        assert!(
            cache.memory_bytes() <= 8,
            "in-memory artifact bytes must stay under the configured cap"
        );
        assert!(
            cache
                .index
                .get(&h1)
                .is_some_and(|entry| entry.data.is_none()),
            "least-recently-used artifact bytes should be evicted first"
        );

        assert_eq!(cache.get(&h1), Some(b"1111".to_vec()));
        assert_eq!(cache.len(), 3);
        assert!(cache.memory_bytes() <= 8);
    }

    #[test]
    fn memory_cache_does_not_retain_oversized_artifacts() {
        let mut cache = make_cache_with_memory_limit(4);
        let hash = hash("large_func", b"body");

        cache.put(&hash, b"artifact-too-large", vec![]);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.memory_bytes(), 0);
        assert!(
            cache
                .index
                .get(&hash)
                .is_some_and(|entry| entry.data.is_none())
        );
        assert_eq!(cache.get(&hash), Some(b"artifact-too-large".to_vec()));
        assert_eq!(
            cache.memory_bytes(),
            0,
            "oversized disk hits must not be retained in memory"
        );
    }

    #[test]
    fn replacing_cache_entry_updates_memory_byte_accounting() {
        let mut cache = make_cache_with_memory_limit(64);
        let hash = hash("replace_func", b"body");

        cache.put(&hash, b"old", vec![]);
        assert_eq!(cache.memory_bytes(), 3);

        cache.put(&hash, b"new-data", vec![]);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.memory_bytes(), 8);
        assert_eq!(cache.get(&hash), Some(b"new-data".to_vec()));
        assert_eq!(cache.memory_bytes(), 8);
    }

    #[test]
    fn invalidate_and_stale_eviction_update_memory_byte_accounting() {
        let mut cache = make_cache_with_memory_limit(64);
        let dep_hash = hash("dep_func", b"dep body");
        let caller_hash = hash("caller_func", b"caller body");
        let stale_hash = hash("stale_func", b"stale body");

        cache.put(&dep_hash, b"dep", vec![]);
        cache.put(&caller_hash, b"caller", vec![dep_hash.clone()]);
        cache.put(&stale_hash, b"stale", vec![]);
        assert_eq!(
            cache.memory_bytes(),
            b"dep".len() + b"caller".len() + b"stale".len()
        );

        cache.invalidate(std::slice::from_ref(&dep_hash));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.memory_bytes(), b"dep".len() + b"stale".len());

        cache.index.get_mut(&stale_hash).unwrap().last_access = 1;
        cache.evict_stale(60);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.memory_bytes(), b"dep".len());
    }

    #[test]
    fn explicit_memory_cache_env_overrides_adaptive_default() {
        let _guard = env_lock().lock().expect("env lock");
        let _env = EnvRestore::apply(&[
            ("MOLT_BACKEND_TIR_CACHE_MEMORY_BYTES", "12345"),
            ("MOLT_MEMORY_AVAILABLE_GB", "1"),
            ("MOLT_MEMORY_RESERVE_GB", "1"),
        ]);
        let limit = default_memory_cache_limit_bytes();
        assert_eq!(limit, 12345);
    }

    #[test]
    fn memory_cache_default_uses_available_memory_after_reserve() {
        let _guard = env_lock().lock().expect("env lock");
        let _env = EnvRestore::apply(&[
            ("MOLT_MEMORY_AVAILABLE_GB", "4"),
            ("MOLT_MEMORY_RESERVE_GB", "2"),
        ]);
        let limit = default_memory_cache_limit_bytes();
        assert_eq!(
            limit,
            16 * 1024 * 1024,
            "2 GiB usable memory divided by the adaptive cache divisor"
        );
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

    #[test]
    fn test_compute_hash_includes_backend_namespace() {
        let params: Vec<String> = Vec::new();
        let mut legacy = DefaultHasher::new();
        "func".hash(&mut legacy);
        params.hash(&mut legacy);
        Option::<&[String]>::None.hash(&mut legacy);
        b"body".hash(&mut legacy);

        let namespaced =
            CompilationCache::compute_hash_with_signature("func", &params, None, b"body");
        assert_ne!(
            namespaced,
            format!("{:016x}", legacy.finish()),
            "backend cache namespace must invalidate stale body-only TIR artifacts"
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
                memory_stamp: 0,
            },
        );
        assert_eq!(cache.get(&hash), None);
    }
}
