//! Incremental compilation cache for TIR functions.
//!
//! Provides a content-addressed cache so unchanged functions are not
//! recompiled between pipeline invocations.  Artifacts are stored on disk
//! under `cache_dir/functions/<hash>.bin`; a plain-text index file at
//! `cache_dir/index.txt` records advisory access metadata. Artifact identity and
//! location remain fully derivable from the content hash.
//!
//! Index file format (one entry per line after the required version header):
//! ```text
//! # molt-cache-index-v2 hash|last_access_unix_secs
//! <64 lowercase hex>|1679900000
//! ```

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

const BACKEND_CACHE_NAMESPACE_VERSION: &str = "molt-backend-tir-cache-v3-sha256-contract";
const CACHE_INDEX_HEADER: &str = "# molt-cache-index-v2 hash|last_access_unix_secs";
const BACKEND_COMPILER_FINGERPRINT_ENV: &str = "MOLT_BACKEND_COMPILER_FINGERPRINT";
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
    /// Indexed min-heap of resident entries. Touches mutate heap nodes in
    /// place, so warm hits allocate nothing and stale nodes never accumulate.
    memory_lru: Vec<MemoryLruNode>,
}

/// A single entry in the compilation cache.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Unix timestamp (seconds) of last access.
    last_access: u64,
    /// Cached artifact bytes — `None` until loaded on demand.
    data: Option<Arc<[u8]>>,
    /// Logical LRU stamp for in-memory artifact bytes.
    memory_stamp: u64,
    /// Position in `CompilationCache::memory_lru` while data is resident.
    memory_lru_index: Option<usize>,
}

#[derive(Debug)]
struct MemoryLruNode {
    stamp: u64,
    content_hash: String,
}

static CACHE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Durable, ambiguity-resistant SHA-256 identity for persistent compiler cache
/// artifacts. Every field is tagged and length-delimited; large semantic
/// contracts can be streamed through a nested digest without an O(IR) buffer.
pub struct CompilationCacheKey {
    digest: Sha256,
}

struct Sha256Writer<'a>(&'a mut Sha256);

impl Write for Sha256Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CompilationCacheKey {
    pub fn new(domain: &[u8]) -> Self {
        let mut key = Self {
            digest: Sha256::new(),
        };
        key.field(b"cache-key-domain", domain);
        key
    }

    fn length_delimited(digest: &mut Sha256, bytes: &[u8]) {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }

    pub fn field(&mut self, label: &[u8], value: &[u8]) {
        self.digest.update([0]);
        Self::length_delimited(&mut self.digest, label);
        Self::length_delimited(&mut self.digest, value);
    }

    pub fn digest_field(
        &mut self,
        label: &[u8],
        write_value: impl FnOnce(&mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut field_digest = Sha256::new();
        write_value(&mut Sha256Writer(&mut field_digest))?;
        self.digest.update([1]);
        Self::length_delimited(&mut self.digest, label);
        Self::length_delimited(&mut self.digest, &field_digest.finalize());
        Ok(())
    }

    pub fn finish_hex(self) -> String {
        let bytes = self.digest.finalize();
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").expect("String formatting cannot fail");
        }
        hex
    }
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
            memory_lru: Vec::new(),
        };
        cache.load_index();
        cache
    }

    /// Look up a cached artifact by content hash.
    ///
    /// Updates `last_access` on a hit. If advisory index metadata is missing,
    /// the canonical content-addressed artifact path is still consulted so a
    /// concurrent index writer cannot hide a valid artifact.
    pub fn get(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        let path = self.artifact_path(content_hash)?;
        let now = unix_now();
        if let Some(entry) = self.index.get_mut(content_hash) {
            entry.last_access = now;
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
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => {
                let bytes = Arc::<[u8]>::from(bytes);
                self.index
                    .entry(content_hash.to_owned())
                    .or_insert_with(|| CacheEntry {
                        last_access: now,
                        data: None,
                        memory_stamp: 0,
                        memory_lru_index: None,
                    });
                self.store_memory_data(content_hash, Arc::clone(&bytes));
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
    /// with the same `content_hash` already exists, that immutable
    /// content-addressed artifact is reused.
    pub fn put(&mut self, content_hash: &str, artifact: &[u8]) {
        assert!(
            is_canonical_cache_hash(content_hash),
            "persistent compilation cache requires a canonical lowercase SHA-256 key"
        );
        assert!(
            !artifact.is_empty(),
            "persistent compilation cache refuses zero-length artifacts"
        );
        let funcs_dir = self.cache_dir.join("functions");
        if std::fs::create_dir_all(&funcs_dir).is_err() {
            return; // can't create cache dir — skip caching silently
        }
        let artifact_path = self
            .artifact_path(content_hash)
            .expect("validated cache hash has a canonical artifact path");
        let mut stored_artifact = Cow::Borrowed(artifact);
        match std::fs::read(&artifact_path) {
            Ok(existing) if existing == artifact => {
                stored_artifact = Cow::Owned(existing);
            }
            existing => {
                if let Ok(existing) = &existing {
                    eprintln!(
                        "MOLT_CACHE: replacing mismatched artifact for {content_hash} (cached_bytes={}, rebuilt_bytes={})",
                        existing.len(),
                        artifact.len()
                    );
                }
                let Ok(tmp_path) =
                    write_unique_temp_file(&funcs_dir, &format!("{content_hash}.bin"), artifact)
                else {
                    return;
                };
                if !install_artifact_temp(&tmp_path, &artifact_path, artifact) {
                    return;
                }
            }
        }

        if let Some(lru_index) = self
            .index
            .get(content_hash)
            .and_then(|entry| entry.memory_lru_index)
        {
            self.memory_lru_remove(lru_index);
        }
        if let Some(previous) = self.index.remove(content_hash)
            && let Some(bytes) = previous.data
        {
            self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
        }

        let mut entry = CacheEntry {
            last_access: unix_now(),
            data: None,
            memory_stamp: 0,
            memory_lru_index: None,
        };
        if stored_artifact.len() <= self.max_memory_bytes && self.max_memory_bytes > 0 {
            self.memory_clock = self.memory_clock.wrapping_add(1);
            entry.memory_stamp = self.memory_clock;
            entry.data = Some(Arc::from(stored_artifact.as_ref()));
            self.memory_bytes = self.memory_bytes.saturating_add(stored_artifact.len());
        }
        let resident = entry.data.is_some();
        self.index.insert(content_hash.to_owned(), entry);
        if resident {
            self.memory_lru_insert(content_hash);
        }
        self.evict_memory();
    }

    /// Persist the cache index to `cache_dir/index.txt`.
    ///
    /// Entries are sorted by hash so identical metadata serializes identically
    /// across processes. Silently ignores I/O errors (cache is advisory).
    pub fn save_index(&self) {
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let index_path = self.cache_dir.join("index.txt");

        let mut lines = format!("{CACHE_INDEX_HEADER}\n");
        let mut entries = self.index.iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        for (content_hash, entry) in entries {
            lines.push_str(&format!("{content_hash}|{}\n", entry.last_access));
        }

        // Atomic write: write to PID-unique temp file then rename so concurrent
        // readers never see a partially-written index.
        let Ok(tmp_path) = write_unique_temp_file(&self.cache_dir, "index.txt", lines.as_bytes())
        else {
            return;
        };
        if std::fs::rename(&tmp_path, &index_path).is_err() {
            // Windows rename does not replace an existing target. The index is
            // advisory and artifacts are independently discoverable, so a
            // same-directory remove+rename fallback is safe and bounded.
            let _ = std::fs::remove_file(&index_path);
            if std::fs::rename(&tmp_path, &index_path).is_err() {
                let _ = std::fs::remove_file(&tmp_path);
            }
        }
    }

    /// Load the cache index from `cache_dir/index.txt`.
    ///
    /// Artifacts are not read eagerly; they are loaded on demand by [`get`]. A
    /// missing/wrong version header rejects the whole old-format index. Invalid
    /// hashes, timestamps, and extra columns are skipped fail-closed.
    pub fn load_index(&mut self) {
        let index_path = self.cache_dir.join("index.txt");
        let contents = match std::fs::read_to_string(&index_path) {
            Ok(c) => c,
            Err(_) => return,
        };

        let mut lines = contents.lines();
        if lines.next() != Some(CACHE_INDEX_HEADER) {
            return;
        }
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((content_hash, last_access)) = line.split_once('|') else {
                continue;
            };
            if last_access.contains('|') || !is_canonical_cache_hash(content_hash) {
                continue;
            }
            let Ok(last_access) = last_access.parse::<u64>() else {
                continue;
            };

            self.index.insert(
                content_hash.to_owned(),
                CacheEntry {
                    last_access,
                    data: None, // loaded on demand
                    memory_stamp: 0,
                    memory_lru_index: None,
                },
            );
        }
    }

    fn artifact_path(&self, content_hash: &str) -> Option<PathBuf> {
        is_canonical_cache_hash(content_hash).then(|| {
            self.cache_dir
                .join("functions")
                .join(format!("{content_hash}.bin"))
        })
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
        self.memory_clock = self.memory_clock.wrapping_add(1);
        let Some(lru_index) = self.index.get_mut(content_hash).and_then(|entry| {
            if entry.data.is_none() {
                return None;
            }
            entry.memory_stamp = self.memory_clock;
            entry.memory_lru_index
        }) else {
            return;
        };
        self.memory_lru[lru_index].stamp = self.memory_clock;
        self.memory_lru_sift_down(lru_index);
    }

    fn store_memory_data(&mut self, content_hash: &str, bytes: Arc<[u8]>) {
        if self.max_memory_bytes == 0 || bytes.len() > self.max_memory_bytes {
            if let Some(lru_index) = self
                .index
                .get(content_hash)
                .and_then(|entry| entry.memory_lru_index)
            {
                self.memory_lru_remove(lru_index);
            }
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
        let lru_index = entry.memory_lru_index;
        if let Some(lru_index) = lru_index {
            self.memory_lru[lru_index].stamp = self.memory_clock;
            self.memory_lru_sift_down(lru_index);
        } else {
            self.memory_lru_insert(content_hash);
        }
        self.evict_memory();
    }

    fn evict_memory(&mut self) {
        while self.memory_bytes > self.max_memory_bytes {
            if self.memory_lru.is_empty() {
                break;
            }
            let content_hash = self.memory_lru_remove(0).content_hash;
            if let Some(entry) = self.index.get_mut(content_hash.as_str())
                && let Some(bytes) = entry.data.take()
            {
                self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
                entry.memory_stamp = 0;
            }
        }
    }

    fn memory_lru_insert(&mut self, content_hash: &str) {
        let stamp = self.index[content_hash].memory_stamp;
        let index = self.memory_lru.len();
        self.memory_lru.push(MemoryLruNode {
            stamp,
            content_hash: content_hash.to_owned(),
        });
        self.index.get_mut(content_hash).unwrap().memory_lru_index = Some(index);
        self.memory_lru_sift_up(index);
    }

    fn memory_lru_remove(&mut self, index: usize) -> MemoryLruNode {
        let last = self.memory_lru.len() - 1;
        if index != last {
            self.memory_lru_swap(index, last);
        }
        let removed = self.memory_lru.pop().expect("LRU removal requires a node");
        self.index
            .get_mut(removed.content_hash.as_str())
            .unwrap()
            .memory_lru_index = None;
        if index < self.memory_lru.len() {
            let index = self.memory_lru_sift_up(index);
            self.memory_lru_sift_down(index);
        }
        removed
    }

    fn memory_lru_sift_up(&mut self, mut index: usize) -> usize {
        while index > 0 {
            let parent = (index - 1) / 2;
            if !self.memory_lru_less(index, parent) {
                break;
            }
            self.memory_lru_swap(index, parent);
            index = parent;
        }
        index
    }

    fn memory_lru_sift_down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.memory_lru.len() {
                break;
            }
            let right = left + 1;
            let smallest = if right < self.memory_lru.len() && self.memory_lru_less(right, left) {
                right
            } else {
                left
            };
            if !self.memory_lru_less(smallest, index) {
                break;
            }
            self.memory_lru_swap(index, smallest);
            index = smallest;
        }
    }

    fn memory_lru_less(&self, left: usize, right: usize) -> bool {
        let left = &self.memory_lru[left];
        let right = &self.memory_lru[right];
        (left.stamp, left.content_hash.as_str()) < (right.stamp, right.content_hash.as_str())
    }

    fn memory_lru_swap(&mut self, left: usize, right: usize) {
        self.memory_lru.swap(left, right);
        let left_hash = self.memory_lru[left].content_hash.as_str();
        self.index.get_mut(left_hash).unwrap().memory_lru_index = Some(left);
        let right_hash = self.memory_lru[right].content_hash.as_str();
        self.index.get_mut(right_hash).unwrap().memory_lru_index = Some(right);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_canonical_cache_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn write_unique_temp_file(
    directory: &std::path::Path,
    stem: &str,
    bytes: &[u8],
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    loop {
        let serial = CACHE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(".{stem}.tmp.{}.{}", std::process::id(), serial));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }
}

fn install_artifact_temp(
    temp_path: &std::path::Path,
    artifact_path: &std::path::Path,
    expected: &[u8],
) -> bool {
    if std::fs::rename(temp_path, artifact_path).is_ok() {
        return true;
    }
    if let Ok(racing_artifact) = std::fs::read(artifact_path)
        && racing_artifact == expected
    {
        let _ = std::fs::remove_file(temp_path);
        return true;
    }
    eprintln!(
        "MOLT_CACHE: same-key writer produced different bytes; replacing `{}` fail-closed",
        artifact_path.display()
    );
    let _ = std::fs::remove_file(artifact_path);
    if std::fs::rename(temp_path, artifact_path).is_ok() {
        true
    } else {
        let _ = std::fs::remove_file(temp_path);
        false
    }
}

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
    let build_identity = backend_build_identity(&exe);
    backend_cache_dir_for(&root, &exe, mtime, &build_identity)
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
    build_identity: &[u8],
) -> PathBuf {
    let mut key = CompilationCacheKey::new(b"molt-backend-cache-directory-v1");
    key.field(
        b"backend-cache-namespace",
        BACKEND_CACHE_NAMESPACE_VERSION.as_bytes(),
    );
    let (path_encoding, path_bytes) = platform_os_str_identity(exe.as_os_str());
    key.field(b"executable-path-encoding", path_encoding);
    key.field(b"executable-path", &path_bytes);
    key.field(b"executable-mtime", &mtime.to_be_bytes());
    key.field(b"backend-build-identity", build_identity);
    root.join(key.finish_hex())
}

fn backend_build_identity(executable: &std::path::Path) -> Vec<u8> {
    if let Some(provided) = std::env::var_os(BACKEND_COMPILER_FINGERPRINT_ENV)
        && !provided.is_empty()
    {
        let (encoding, bytes) = platform_os_str_identity(&provided);
        let mut key = CompilationCacheKey::new(b"molt-provided-backend-build-identity-v1");
        key.field(b"encoding", encoding);
        key.field(b"fingerprint", &bytes);
        return key.finish_hex().into_bytes();
    }

    let mut digest = Sha256::new();
    let Ok(mut file) = std::fs::File::open(executable) else {
        return b"missing-backend-executable".to_vec();
    };
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match std::io::Read::read(&mut file, &mut buffer) {
            Ok(0) => break,
            Ok(read) => digest.update(&buffer[..read]),
            Err(_) => return b"unreadable-backend-executable".to_vec(),
        }
    }
    digest.finalize().to_vec()
}

#[cfg(unix)]
fn platform_os_str_identity(value: &OsStr) -> (&'static [u8], Vec<u8>) {
    use std::os::unix::ffi::OsStrExt as _;

    (b"unix-bytes", value.as_bytes().to_vec())
}

#[cfg(windows)]
fn platform_os_str_identity(value: &OsStr) -> (&'static [u8], Vec<u8>) {
    use std::os::windows::ffi::OsStrExt as _;

    let bytes = value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    (b"windows-u16le", bytes)
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

    fn fixture_hash(func_name: &str, body: &[u8]) -> String {
        let mut key = CompilationCacheKey::new(b"molt-cache-test-fixture-v1");
        key.field(b"function-name", func_name.as_bytes());
        key.field(b"body", body);
        key.finish_hex()
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
        let hash = fixture_hash("my_func", b"op1 op2 op3");
        let artifact = b"compiled artifact bytes";

        cache.put(&hash, artifact);
        let result = cache.get(&hash);

        assert_eq!(result.as_deref(), Some(artifact.as_slice()));
    }

    #[test]
    fn warm_memory_hits_share_storage_and_do_not_copy_artifact_bytes() {
        let mut cache = make_cache_with_memory_limit(2 * 1024 * 1024);
        let hash = fixture_hash("warm", b"large-artifact");
        let artifact = vec![0x5a; 1024 * 1024];
        cache.put(&hash, &artifact);

        let first = cache.get(&hash).unwrap();
        let second = cache.get(&hash).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let lru_capacity = cache.memory_lru.capacity();
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            let hit = std::hint::black_box(cache.get(&hash).unwrap());
            assert!(Arc::ptr_eq(&first, &hit));
        }
        let elapsed = start.elapsed();
        eprintln!(
            "warm cache: 10000 shared 1MiB hits in {elapsed:?} ({:?}/hit)",
            elapsed / 10_000
        );
        assert!(elapsed < std::time::Duration::from_secs(2));
        assert_eq!(cache.memory_lru.len(), 1);
        assert_eq!(
            cache.memory_lru.capacity(),
            lru_capacity,
            "warm hits must mutate the indexed LRU node in place"
        );
    }

    #[test]
    fn indexed_lru_scales_with_many_metadata_entries_and_bounded_residency() {
        const ENTRY_BYTES: usize = 1024;
        const RESIDENT_ENTRIES: usize = 128;
        const TOTAL_ENTRIES: usize = 20_000;

        let mut cache = make_cache_with_memory_limit(ENTRY_BYTES * RESIDENT_ENTRIES);
        let artifact = Arc::<[u8]>::from(vec![0x6b; ENTRY_BYTES]);
        let start = std::time::Instant::now();
        for index in 0..TOTAL_ENTRIES {
            let hash = format!("{index:064x}");
            cache.index.insert(
                hash.clone(),
                CacheEntry {
                    last_access: 0,
                    data: None,
                    memory_stamp: 0,
                    memory_lru_index: None,
                },
            );
            cache.store_memory_data(&hash, Arc::clone(&artifact));
        }
        let elapsed = start.elapsed();

        let resident_entries = cache
            .index
            .values()
            .filter(|entry| entry.data.is_some())
            .count();
        assert_eq!(resident_entries, RESIDENT_ENTRIES);
        assert_eq!(cache.memory_lru.len(), RESIDENT_ENTRIES);
        assert_eq!(cache.memory_bytes(), ENTRY_BYTES * RESIDENT_ENTRIES);
        for (heap_index, node) in cache.memory_lru.iter().enumerate() {
            assert_eq!(
                cache.index[&node.content_hash].memory_lru_index,
                Some(heap_index)
            );
            if heap_index > 0 {
                let parent = (heap_index - 1) / 2;
                assert!(!cache.memory_lru_less(heap_index, parent));
            }
        }
        eprintln!(
            "indexed LRU: inserted {TOTAL_ENTRIES} metadata rows with {RESIDENT_ENTRIES} resident artifacts in {elapsed:?}"
        );
        assert!(elapsed < std::time::Duration::from_secs(5));
    }

    #[test]
    fn memory_cache_evicts_lru_bytes_without_dropping_disk_index() {
        let mut cache = make_cache_with_memory_limit(8);
        let h1 = fixture_hash("fn_1", b"body 1");
        let h2 = fixture_hash("fn_2", b"body 2");
        let h3 = fixture_hash("fn_3", b"body 3");

        cache.put(&h1, b"1111");
        cache.put(&h2, b"2222");
        assert_eq!(cache.memory_bytes(), 8);

        cache.put(&h3, b"3333");
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

        assert_eq!(cache.get(&h1).as_deref(), Some(b"1111".as_slice()));
        assert_eq!(cache.len(), 3);
        assert!(cache.memory_bytes() <= 8);
    }

    #[test]
    fn memory_cache_does_not_retain_oversized_artifacts() {
        let mut cache = make_cache_with_memory_limit(4);
        let hash = fixture_hash("large_func", b"body");

        cache.put(&hash, b"artifact-too-large");

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.memory_bytes(), 0);
        assert!(
            cache
                .index
                .get(&hash)
                .is_some_and(|entry| entry.data.is_none())
        );
        assert_eq!(
            cache.get(&hash).as_deref(),
            Some(b"artifact-too-large".as_slice())
        );
        assert_eq!(
            cache.memory_bytes(),
            0,
            "oversized disk hits must not be retained in memory"
        );
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
        let dir_a_1 = backend_cache_dir_for(&root, &exe_a, 111, b"build-a");
        let dir_a_2 = backend_cache_dir_for(&root, &exe_a, 111, b"build-a");
        let dir_b = backend_cache_dir_for(&root, &exe_b, 111, b"build-a");
        let dir_time = backend_cache_dir_for(&root, &exe_a, 222, b"build-a");
        let dir_build = backend_cache_dir_for(&root, &exe_a, 111, b"build-b");

        assert_eq!(
            dir_a_1, dir_a_2,
            "same inputs must produce the same cache dir"
        );
        assert_ne!(dir_a_1, dir_b, "exe path must affect the cache namespace");
        assert_ne!(dir_a_1, dir_time, "mtime must affect the cache namespace");
        assert_ne!(
            dir_a_1, dir_build,
            "build identity must invalidate same-path/same-mtime replacements"
        );
        assert!(
            dir_a_1.starts_with(&root),
            "cache namespace must stay under the provided root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn backend_cache_dir_distinguishes_non_utf8_paths_with_same_lossy_display() {
        use std::os::unix::ffi::OsStringExt as _;

        let root = tmp_cache_dir();
        let exe_a = PathBuf::from(std::ffi::OsString::from_vec(vec![b'm', 0x80]));
        let exe_b = PathBuf::from(std::ffi::OsString::from_vec(vec![b'm', 0x81]));
        assert_eq!(exe_a.to_string_lossy(), exe_b.to_string_lossy());
        assert_ne!(
            backend_cache_dir_for(&root, &exe_a, 111, b"build"),
            backend_cache_dir_for(&root, &exe_b, 111, b"build"),
            "raw Unix executable path bytes must own cache namespace identity"
        );
    }

    #[cfg(windows)]
    #[test]
    fn backend_cache_dir_distinguishes_unpaired_utf16_paths_with_same_lossy_display() {
        use std::os::windows::ffi::OsStringExt as _;

        let root = tmp_cache_dir();
        let exe_a = PathBuf::from(std::ffi::OsString::from_wide(&[b'm' as u16, 0xd800]));
        let exe_b = PathBuf::from(std::ffi::OsString::from_wide(&[b'm' as u16, 0xd801]));
        assert_eq!(exe_a.to_string_lossy(), exe_b.to_string_lossy());
        assert_ne!(
            backend_cache_dir_for(&root, &exe_a, 111, b"build"),
            backend_cache_dir_for(&root, &exe_b, 111, b"build"),
            "raw Windows executable UTF-16 units must own cache namespace identity"
        );
    }

    /// 2. get on missing key → None
    #[test]
    fn test_get_missing_key() {
        let mut cache = make_cache();
        assert_eq!(cache.get("nonexistent_hash"), None);
    }

    /// 5. Fixture keys are consistent for identical inputs.
    #[test]
    fn fixture_hash_is_consistent() {
        let h1 = fixture_hash("func_a", b"body bytes");
        let h2 = fixture_hash("func_a", b"body bytes");
        assert_eq!(h1, h2, "same inputs must produce the same hash");
    }

    /// Fixture keys differ for different inputs.
    #[test]
    fn fixture_hash_distinguishes_inputs() {
        let h1 = fixture_hash("func_a", b"body A");
        let h2 = fixture_hash("func_a", b"body B");
        assert_ne!(h1, h2, "different bodies must produce different hashes");

        let h3 = fixture_hash("func_x", b"body A");
        let h4 = fixture_hash("func_y", b"body A");
        assert_ne!(h3, h4, "different names must produce different hashes");
    }

    #[test]
    fn zero_length_artifacts_are_rejected_before_disk_or_index_mutation() {
        let mut cache = make_cache();
        let key = fixture_hash("empty", b"body");
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.put(&key, &[]);
        }));
        assert!(rejected.is_err());
        assert!(cache.get(&key).is_none());
        assert!(
            !cache
                .cache_dir
                .join("functions")
                .join(format!("{key}.bin"))
                .exists()
        );
    }

    #[test]
    fn canonical_cache_key_is_length_delimited_typed_and_streamable() {
        let build_partitioned = |first: &[u8], second: &[u8]| {
            let mut key = CompilationCacheKey::new(b"ambiguity-test-v1");
            key.field(b"part", first);
            key.field(b"part", second);
            key.finish_hex()
        };
        assert_ne!(
            build_partitioned(b"a", b"bc"),
            build_partitioned(b"ab", b"c"),
            "length-delimited fields must resist concatenation ambiguity"
        );

        let mut raw = CompilationCacheKey::new(b"typed-field-test-v1");
        raw.field(b"value", &[7; 32]);
        let raw = raw.finish_hex();
        let mut streamed = CompilationCacheKey::new(b"typed-field-test-v1");
        streamed
            .digest_field(b"value", |writer| {
                writer
                    .write_all(&[7; 32])
                    .map_err(|error| error.to_string())
            })
            .unwrap();
        assert_ne!(
            raw,
            streamed.finish_hex(),
            "raw and digest fields are domain-separated"
        );
    }

    #[test]
    fn fixture_key_is_namespaced_sha256_and_process_deterministic() {
        let first = fixture_hash("func", b"body");
        let second = fixture_hash("func", b"body");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    /// 6. save_index + load_index round-trip
    #[test]
    fn test_disk_roundtrip() {
        let dir = tmp_cache_dir();

        // Write entries to disk.
        {
            let mut cache = CompilationCache::open(dir.clone());
            let h1 = fixture_hash("fn_a", b"body a");
            let h2 = fixture_hash("fn_b", b"body b");
            cache.put(&h1, b"artifact a");
            cache.put(&h2, b"artifact b");
            cache.save_index();
        }

        // Open a fresh cache from the same directory — index loads from disk.
        let mut cache2 = CompilationCache::open(dir);
        let h1 = fixture_hash("fn_a", b"body a");
        let h2 = fixture_hash("fn_b", b"body b");

        // Entries should be present (loaded lazily from disk).
        assert_eq!(cache2.get(&h1).as_deref(), Some(b"artifact a".as_slice()));
        assert_eq!(cache2.get(&h2).as_deref(), Some(b"artifact b".as_slice()));

        assert_eq!(cache2.len(), 2);
    }

    #[test]
    fn test_get_missing_artifact_file() {
        let dir = tmp_cache_dir();
        let mut cache = CompilationCache::open(dir);
        let hash = fixture_hash("missing", b"artifact");
        assert_eq!(cache.get(&hash), None);
    }

    #[test]
    fn canonical_artifact_is_discovered_without_an_index_row() {
        let dir = tmp_cache_dir();
        let hash = fixture_hash("concurrent", b"artifact");
        let functions = dir.join("functions");
        std::fs::create_dir_all(&functions).unwrap();
        std::fs::write(functions.join(format!("{hash}.bin")), b"artifact").unwrap();

        let mut cache = CompilationCache::open(dir);
        assert!(cache.is_empty());
        assert_eq!(cache.get(&hash).as_deref(), Some(b"artifact".as_slice()));
        assert_eq!(
            cache.len(),
            1,
            "a discovered artifact should restore advisory metadata"
        );
    }

    #[test]
    fn concurrent_index_save_cannot_hide_an_existing_artifact() {
        let dir = tmp_cache_dir();
        let hash = fixture_hash("writer-a", b"artifact");
        let mut writer_a = CompilationCache::open(dir.clone());
        writer_a.put(&hash, b"artifact");

        let writer_b = CompilationCache::open(dir.clone());
        writer_b.save_index();

        let mut reader = CompilationCache::open(dir);
        assert!(reader.is_empty());
        assert_eq!(reader.get(&hash).as_deref(), Some(b"artifact".as_slice()));
    }

    #[test]
    fn same_process_concurrent_writers_use_collision_free_temp_files() {
        let dir = tmp_cache_dir();
        let h1 = fixture_hash("thread-a", b"artifact-a");
        let h2 = fixture_hash("thread-b", b"artifact-b");
        std::thread::scope(|scope| {
            for (hash, artifact) in [
                (h1.clone(), b"artifact-a".as_slice()),
                (h2.clone(), b"artifact-b".as_slice()),
            ] {
                let dir = dir.clone();
                scope.spawn(move || {
                    let mut cache = CompilationCache::open(dir);
                    cache.put(&hash, artifact);
                    cache.save_index();
                });
            }
        });

        let mut reader = CompilationCache::open(dir.clone());
        assert_eq!(reader.get(&h1).as_deref(), Some(b"artifact-a".as_slice()));
        assert_eq!(reader.get(&h2).as_deref(), Some(b"artifact-b".as_slice()));
        for directory in [dir.clone(), dir.join("functions")] {
            for entry in std::fs::read_dir(directory).unwrap() {
                let name = entry.unwrap().file_name();
                assert!(
                    !name.to_string_lossy().contains(".tmp."),
                    "completed writers must clean every unique temp file"
                );
            }
        }
    }

    #[test]
    fn corrupt_nonempty_and_concurrent_same_key_artifacts_are_replaced_whole() {
        let dir = tmp_cache_dir();
        let hash = fixture_hash("same-key", b"semantic-contract");
        let functions = dir.join("functions");
        std::fs::create_dir_all(&functions).unwrap();
        let artifact_path = functions.join(format!("{hash}.bin"));
        std::fs::write(&artifact_path, b"truncated-poison").unwrap();

        let mut repair = CompilationCache::open(dir.clone());
        repair.put(&hash, b"rebuilt-artifact");
        assert_eq!(std::fs::read(&artifact_path).unwrap(), b"rebuilt-artifact");

        let barrier = Arc::new(std::sync::Barrier::new(3));
        std::thread::scope(|scope| {
            for artifact in [b"writer-a".as_slice(), b"writer-b-complete".as_slice()] {
                let dir = dir.clone();
                let hash = hash.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    let mut cache = CompilationCache::open(dir);
                    barrier.wait();
                    cache.put(&hash, artifact);
                });
            }
            barrier.wait();
        });
        let final_bytes = std::fs::read(&artifact_path).unwrap();
        assert!(
            final_bytes == b"writer-a" || final_bytes == b"writer-b-complete",
            "same-key races must leave one complete artifact, got {final_bytes:?}"
        );
    }

    #[test]
    fn index_is_versioned_strict_and_rejects_old_or_malformed_rows() {
        let dir = tmp_cache_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let valid = fixture_hash("valid", b"row");
        let malformed = format!(
            "{CACHE_INDEX_HEADER}\n../escape|1\nABCDEF{}|2\n{valid}|not-a-time\n{valid}|3|extra\n{valid}|4\n",
            "0".repeat(58)
        );
        std::fs::write(dir.join("index.txt"), malformed).unwrap();
        let cache = CompilationCache::open(dir.clone());
        assert_eq!(cache.len(), 1);
        assert!(cache.index.contains_key(&valid));

        std::fs::write(
            dir.join("index.txt"),
            format!("# hash|artifact_path|deps|last_access\n{valid}|../../escape||5\n"),
        )
        .unwrap();
        assert!(
            CompilationCache::open(dir).is_empty(),
            "old index formats must not be parsed"
        );
    }

    #[test]
    fn invalid_hashes_cannot_escape_the_canonical_artifact_directory() {
        let dir = tmp_cache_dir();
        let mut cache = CompilationCache::open(dir.clone());
        assert!(cache.get("../escape").is_none());
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.put("../escape", b"artifact");
        }));
        assert!(rejected.is_err());
        assert!(!dir.join("escape.bin").exists());
    }

    #[test]
    fn sorted_index_serialization_is_process_stable() {
        fn cache_with_metadata(dir: PathBuf, rows: &[(String, u64)]) -> CompilationCache {
            let mut cache = CompilationCache::open(dir);
            for (hash, last_access) in rows {
                cache.index.insert(
                    hash.clone(),
                    CacheEntry {
                        last_access: *last_access,
                        data: None,
                        memory_stamp: 0,
                        memory_lru_index: None,
                    },
                );
            }
            cache
        }

        let h1 = fixture_hash("a", b"body");
        let h2 = fixture_hash("b", b"body");
        let dir_a = tmp_cache_dir();
        let dir_b = tmp_cache_dir();
        let cache_a = cache_with_metadata(dir_a.clone(), &[(h2.clone(), 2), (h1.clone(), 1)]);
        let cache_b = cache_with_metadata(dir_b.clone(), &[(h1, 1), (h2, 2)]);
        cache_a.save_index();
        cache_b.save_index();
        assert_eq!(
            std::fs::read(dir_a.join("index.txt")).unwrap(),
            std::fs::read(dir_b.join("index.txt")).unwrap()
        );
    }
}
