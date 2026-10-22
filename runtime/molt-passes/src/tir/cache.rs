//! Incremental compilation cache for TIR functions.
//!
//! A content-addressed store: unchanged functions are not recompiled between
//! pipeline invocations, and concurrent compiler processes share the work.
//! Each artifact lives at `cache_dir/functions/<hash>.bin` as an authenticated
//! envelope (magic, kind, key, payload length, SHA-256 of the payload), so a
//! reader verifies everything it serves and needs no lock, index or flush
//! ordering:
//!
//! - `put` publishes with a no-clobber hard link from a unique temporary file.
//!   The first writer wins; a later writer compares payloads. An equal payload
//!   is a reuse. A different payload quarantines the key: a poison sidecar
//!   (`<hash>.poison`) makes every later read miss and every later write fail.
//! - `get` verifies the envelope. Torn or corrupt files are crash residue; they
//!   are retired and read as misses.
//! - `trim_if_due` bounds the namespace at most once per trim interval, under a
//!   non-blocking trim lock. It evicts the least recently used files (file
//!   modification time, refreshed on use at most once per recency interval)
//!   and retires sibling backend namespaces unused for the namespace limit.
//!
//! This is the design of Go's build cache (`cmd/go/internal/cache`: unlocked
//! reads and writes, checksum-verified reads, `trim.txt`-gated trimming,
//! hourly mtime refresh) and of ccache. Nothing is flushed to stable storage:
//! the cache is advisory, and verification turns any crash residue into a
//! miss. Opening a cache performs no I/O.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use molt_ir::content_digest::CompilationCacheKey;
use sha2::{Digest, Sha256};

use crate::resident_cache::ResidentCache;

const BACKEND_CACHE_NAMESPACE_VERSION: &str = "molt-backend-tir-cache-v8-lock-free-content-store";
const CACHE_ARTIFACT_MAGIC: &[u8] = b"MOLT:CACHE-ARTIFACT:v1\0";
const CACHE_ARTIFACT_KEY_BYTES: usize = 64;
const CACHE_ARTIFACT_DIGEST_BYTES: usize = 32;
const CACHE_ARTIFACT_FIXED_BYTES: usize =
    CACHE_ARTIFACT_MAGIC.len() + 1 + CACHE_ARTIFACT_KEY_BYTES + 8 + CACHE_ARTIFACT_DIGEST_BYTES;
const CACHE_FUNCTIONS_DIR: &str = "functions";
const CACHE_TRIM_STAMP_FILE: &str = "trim.txt";
const CACHE_TRIM_LOCK_FILE: &str = ".trim.lock";
/// Minimum time between trims of one namespace.
const CACHE_TRIM_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Granularity of on-disk recency: a hit rewrites an artifact's modification
/// time only when it is older than this, so hits stay read-only in practice.
const CACHE_RECENCY_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// A sibling backend namespace unused for this long belongs to a retired
/// backend binary and is removed (Go's build cache uses the same 5 days).
const CACHE_NAMESPACE_UNUSED_LIMIT: Duration = Duration::from_secs(5 * 24 * 60 * 60);
const DEFAULT_MEMORY_CACHE_BYTES_FALLBACK: usize = 64 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_AVAILABLE_BYTES_MIN: usize = 8 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_BYTES_MIN: usize = 32 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_BYTES_MAX: usize = 512 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_AVAILABLE_DIVISOR: usize = 128;
const DEFAULT_MEMORY_CACHE_TOTAL_DIVISOR: usize = 512;
const DEFAULT_PERSISTENT_CACHE_MAX_ENTRIES: usize = 65_536;
const DEFAULT_PERSISTENT_CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Content-addressed cache for compiled TIR functions.
///
/// Key: hex-encoded hash of (cache namespace + function signature + body bytes).
/// Value: cached compilation artifact, resident in memory and stored on disk.
pub struct CompilationCache {
    /// Namespace directory (e.g. `.molt_cache/<namespace>/`).
    cache_dir: PathBuf,
    functions_dir: PathBuf,
    /// Root of the backend namespaces whose stale siblings trimming retires.
    /// `None` for an explicitly placed cache, which owns only itself.
    namespace_root: Option<PathBuf>,

    /// Resident ownership: payloads, keys, quarantine paths and entry storage.
    resident: ResidentCache<Arc<str>, ResidentEntry>,

    /// Maximum files (artifacts and poison sidecars) a trim leaves on disk.
    max_entries: usize,
    /// Maximum bytes a trim leaves on disk; larger envelopes stay memory-only.
    max_persistent_bytes: u64,
    telemetry: CompilationCacheTelemetry,
}

/// A resident artifact payload.
#[derive(Debug)]
struct ResidentEntry {
    data: Arc<[u8]>,
    /// Precomputed so the quarantine probe on a warm hit allocates nothing.
    poison_path: PathBuf,
}

static CACHE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct CompilationCacheTelemetry {
    pub memory_hits: u64,
    pub disk_hits: u64,
    pub misses: u64,
    pub writes: u64,
    pub reuses: u64,
    pub corruptions: u64,
    pub nondeterminism_quarantines: u64,
    pub trims: u64,
    pub evicted_entries: u64,
    pub evicted_persistent_bytes: u64,
    pub temp_files_retired: u64,
    pub untracked_files_retired: u64,
    pub stale_namespaces_retired: u64,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct CompilationCacheStats {
    pub resident_entries: usize,
    /// Charged key/path/payload/entry ownership, not process RSS.
    pub resident_bytes: usize,
    pub max_entries: usize,
    pub max_resident_bytes: usize,
    pub max_persistent_bytes: u64,
    pub telemetry: CompilationCacheTelemetry,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CompilationCacheWriteError {
    Unavailable(String),
    Integrity(String),
}

impl std::fmt::Display for CompilationCacheWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(detail) | Self::Integrity(detail) => formatter.write_str(detail),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CacheEnvelopeKind {
    Artifact = 0,
    Poison = 1,
}

impl CacheEnvelopeKind {
    fn from_byte(byte: u8) -> Result<Self, String> {
        match byte {
            0 => Ok(Self::Artifact),
            1 => Ok(Self::Poison),
            _ => Err(format!("unknown cache envelope kind {byte}")),
        }
    }
}

// ---------------------------------------------------------------------------
// CompilationCache implementation
// ---------------------------------------------------------------------------

impl CompilationCache {
    /// Open the compilation cache rooted at `cache_dir` with default limits.
    pub fn open(cache_dir: PathBuf) -> Self {
        Self::open_with_limits(
            cache_dir,
            default_memory_cache_limit_bytes(),
            default_persistent_cache_max_entries(),
            default_persistent_cache_max_bytes(),
        )
    }

    /// Open the namespace of the running backend binary under the process's
    /// cache root (see [`backend_cache_dir`]). Its trims also retire stale
    /// sibling namespaces.
    pub fn open_backend_namespace() -> Self {
        Self::open_backend_namespace_under(backend_cache_root())
    }

    /// Open the namespace of the running backend binary under `root`, for a
    /// process whose own environment and current directory do not name its
    /// cache (the JIT runner's are the program's). Its trims also retire
    /// stale sibling namespaces under `root`.
    pub fn open_backend_namespace_under(root: PathBuf) -> Self {
        let mut cache = Self::open(backend_cache_dir_under(&root));
        cache.namespace_root = Some(root);
        cache
    }

    /// Open a compilation cache with an explicit in-memory cap.
    pub fn open_with_memory_limit(cache_dir: PathBuf, max_memory_bytes: usize) -> Self {
        Self::open_with_limits(
            cache_dir,
            max_memory_bytes,
            default_persistent_cache_max_entries(),
            default_persistent_cache_max_bytes(),
        )
    }

    /// Open a compilation cache with explicit resident, entry and
    /// persistent-byte ceilings.
    pub fn open_with_limits(
        cache_dir: PathBuf,
        max_memory_bytes: usize,
        max_entries: usize,
        max_persistent_bytes: u64,
    ) -> Self {
        let functions_dir = cache_dir.join(CACHE_FUNCTIONS_DIR);
        Self {
            cache_dir,
            functions_dir,
            namespace_root: None,
            resident: ResidentCache::new(Some(max_memory_bytes)),
            max_entries,
            max_persistent_bytes,
            telemetry: CompilationCacheTelemetry::default(),
        }
    }

    /// Return the verified payload cached under `content_hash`, if any.
    pub fn get(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        if !is_canonical_cache_hash(content_hash) {
            self.telemetry.misses = self.telemetry.misses.saturating_add(1);
            return None;
        }
        // Warm hits allocate nothing: paths are built only for disk reads.
        if let Some(bytes) = self.resident_hit(content_hash) {
            self.telemetry.memory_hits = self.telemetry.memory_hits.saturating_add(1);
            return Some(bytes);
        }
        let artifact_path = self
            .artifact_path(content_hash)
            .expect("validated cache hash has a canonical artifact path");
        let read = read_file_bounded(&artifact_path, self.max_persistent_bytes);
        let envelope = match read {
            Ok(Some(envelope)) => envelope,
            Ok(None) => {
                self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                return None;
            }
            Err(error) => {
                self.retire_corrupt_artifact(&artifact_path, content_hash, &error);
                return None;
            }
        };
        // A quarantine names the poison sidecar before it retires the
        // artifact, so a reader inside that window must still miss.
        let poison_path = self
            .poison_path(content_hash)
            .expect("validated cache hash has a canonical poison path");
        if !matches!(quarantine_probe(&poison_path), Ok(false)) {
            self.telemetry.misses = self.telemetry.misses.saturating_add(1);
            return None;
        }
        match decode_artifact_envelope(content_hash, &envelope) {
            Ok(payload) => {
                let bytes = Arc::<[u8]>::from(payload);
                refresh_recency(&artifact_path, SystemTime::now());
                self.store_memory_data(content_hash, Arc::clone(&bytes));
                self.telemetry.disk_hits = self.telemetry.disk_hits.saturating_add(1);
                Some(bytes)
            }
            Err(error) => {
                self.retire_corrupt_artifact(&artifact_path, content_hash, &error);
                None
            }
        }
    }

    /// Store a compilation artifact in memory and on disk.
    ///
    /// The first writer of a key publishes it; a later writer with an equal
    /// payload reuses it. A different payload for the same key proves the
    /// compiler nondeterministic: the key is quarantined and the write fails
    /// with [`CompilationCacheWriteError::Integrity`].
    pub fn put(
        &mut self,
        content_hash: &str,
        artifact: &[u8],
    ) -> Result<(), CompilationCacheWriteError> {
        assert!(
            is_canonical_cache_hash(content_hash),
            "persistent compilation cache requires a canonical lowercase SHA-256 key"
        );
        assert!(
            !artifact.is_empty(),
            "persistent compilation cache refuses zero-length artifacts"
        );
        if self.max_entries == 0 {
            return Err(CompilationCacheWriteError::Unavailable(
                "persistent compilation cache is disabled by a zero entry limit".to_string(),
            ));
        }
        let artifact_path = self
            .artifact_path(content_hash)
            .expect("validated cache hash has a canonical artifact path");
        let poison_path = self
            .poison_path(content_hash)
            .expect("validated cache hash has a canonical poison path");
        // A quarantined key rejects every write, an equal one included: the
        // compiler has already produced two payloads for it.
        match quarantine_probe(&poison_path) {
            Ok(false) => {}
            Ok(true) => {
                self.forget_resident(content_hash);
                return Err(self.quarantined(content_hash, &poison_path));
            }
            Err(error) => {
                return Err(CompilationCacheWriteError::Unavailable(format!(
                    "cannot probe cache quarantine {}: {error}",
                    poison_path.display()
                )));
            }
        }
        if let Some(existing) = self
            .resident
            .get(content_hash)
            .map(|entry| Arc::clone(&entry.data))
        {
            if *existing == *artifact {
                self.telemetry.reuses = self.telemetry.reuses.saturating_add(1);
                return Ok(());
            }
            return Err(self.quarantine(content_hash, &existing, artifact));
        }
        let envelope = encode_cache_envelope(content_hash, CacheEnvelopeKind::Artifact, artifact);
        if envelope.len() as u64 > self.max_persistent_bytes {
            // The namespace cannot hold it; keep it for this process only.
            self.store_memory_data(content_hash, Arc::from(artifact));
            return Ok(());
        }
        let temp = write_unique_temp_file(
            &self.functions_dir,
            &format!("{content_hash}.bin"),
            &envelope,
        )
        .map_err(|error| {
            CompilationCacheWriteError::Unavailable(format!(
                "persistent compilation cache cannot stage {content_hash} in {}: {error}",
                self.functions_dir.display()
            ))
        })?;
        let published = self.publish(content_hash, &temp, &artifact_path, &poison_path, artifact);
        let _ = std::fs::remove_file(&temp);
        published?;
        self.store_memory_data(content_hash, Arc::from(artifact));
        Ok(())
    }

    /// Link the staged envelope into place without replacing an incumbent.
    fn publish(
        &mut self,
        content_hash: &str,
        temp: &Path,
        artifact_path: &Path,
        poison_path: &Path,
        artifact: &[u8],
    ) -> Result<(), CompilationCacheWriteError> {
        // A corrupt incumbent is crash residue: retire it and link once more.
        for _ in 0..2 {
            match std::fs::hard_link(temp, artifact_path) {
                Ok(()) => {
                    // A quarantine names its poison sidecar before it retires
                    // the incumbent. A link that landed after that retirement
                    // therefore sees the sidecar here and must not stand.
                    if !matches!(quarantine_probe(poison_path), Ok(false)) {
                        if let Err(error) = retire_cache_file(artifact_path) {
                            eprintln!("MOLT_CACHE: {error}");
                        }
                        return Err(self.quarantined(content_hash, poison_path));
                    }
                    self.telemetry.writes = self.telemetry.writes.saturating_add(1);
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(CompilationCacheWriteError::Unavailable(format!(
                        "cannot publish cache artifact {}: {error}",
                        artifact_path.display()
                    )));
                }
            }
            let incumbent = match read_file_bounded(artifact_path, self.max_persistent_bytes) {
                // Retired by a concurrent reader or trim: link again.
                Ok(None) => continue,
                Ok(Some(envelope)) => Ok(envelope),
                Err(error) => Err(error),
            };
            match incumbent
                .as_deref()
                .map_err(String::clone)
                .and_then(|envelope| decode_artifact_envelope(content_hash, envelope))
            {
                Ok(existing) if existing == artifact => {
                    self.telemetry.reuses = self.telemetry.reuses.saturating_add(1);
                    refresh_recency(artifact_path, SystemTime::now());
                    return Ok(());
                }
                Ok(existing) => {
                    let existing = existing.to_vec();
                    return Err(self.quarantine(content_hash, &existing, artifact));
                }
                Err(_) => {
                    self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                    retire_cache_file(artifact_path)
                        .map_err(CompilationCacheWriteError::Unavailable)?;
                }
            }
        }
        Err(CompilationCacheWriteError::Unavailable(format!(
            "cache artifact {} changed during publication",
            artifact_path.display()
        )))
    }

    /// Quarantine a key whose writers produced different payloads.
    ///
    /// The poison sidecar is named before the artifact is retired, so no
    /// reader can serve either payload once the quarantine starts. A sidecar
    /// that cannot be stored still retires the artifact; the error stays an
    /// integrity failure either way.
    fn quarantine(
        &mut self,
        content_hash: &str,
        existing: &[u8],
        artifact: &[u8],
    ) -> CompilationCacheWriteError {
        self.telemetry.nondeterminism_quarantines =
            self.telemetry.nondeterminism_quarantines.saturating_add(1);
        self.forget_resident(content_hash);
        let detail = format!("same cache key {content_hash} produced different payloads");
        let artifact_path = self
            .artifact_path(content_hash)
            .expect("validated cache hash has a canonical artifact path");
        let poison_path = self
            .poison_path(content_hash)
            .expect("validated cache hash has a canonical poison path");
        let poison =
            poison_envelope_for_limit(content_hash, existing, artifact, self.max_persistent_bytes)
                .and_then(|envelope| {
                    install_poison_sidecar(
                        &self.functions_dir,
                        &poison_path,
                        content_hash,
                        &envelope,
                    )
                });
        let retired = retire_cache_file(&artifact_path);
        match (poison, retired) {
            (Ok(()), Ok(())) => CompilationCacheWriteError::Integrity(detail),
            (poison, retired) => CompilationCacheWriteError::Integrity(format!(
                "{detail}; quarantine incomplete (poison: {}; artifact retirement: {})",
                poison.err().unwrap_or_else(|| "stored".to_string()),
                retired.err().unwrap_or_else(|| "done".to_string()),
            )),
        }
    }

    fn quarantined(&self, content_hash: &str, poison_path: &Path) -> CompilationCacheWriteError {
        let detail = match read_poison_sidecar(content_hash, poison_path, self.max_persistent_bytes)
        {
            Ok(Some(poison)) => String::from_utf8_lossy(&poison).into_owned(),
            Ok(None) => "its poison sidecar was trimmed during the write".to_string(),
            Err(error) => format!("invalid poison sidecar: {error}"),
        };
        CompilationCacheWriteError::Integrity(format!(
            "cache key {content_hash} is quarantined: {detail}"
        ))
    }

    fn retire_corrupt_artifact(&mut self, artifact_path: &Path, content_hash: &str, error: &str) {
        self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
        self.telemetry.misses = self.telemetry.misses.saturating_add(1);
        eprintln!("MOLT_CACHE: retiring corrupt artifact {content_hash}: {error}");
        if let Err(error) = retire_cache_file(artifact_path) {
            eprintln!("MOLT_CACHE: {error}");
        }
    }

    /// Trim the namespace if no trim ran within the trim interval.
    pub fn trim_if_due(&mut self) -> Result<(), String> {
        let now = SystemTime::now();
        if !trim_due(&self.cache_dir.join(CACHE_TRIM_STAMP_FILE), now) {
            return Ok(());
        }
        self.trim_at(now, true)
    }

    /// Trim the namespace now: retire stale temporary and untracked files,
    /// evict the least recently used files beyond the entry and byte limits,
    /// and retire stale sibling backend namespaces.
    pub fn trim(&mut self) -> Result<(), String> {
        self.trim_at(SystemTime::now(), false)
    }

    fn trim_at(&mut self, now: SystemTime, recheck_due: bool) -> Result<(), String> {
        std::fs::create_dir_all(&self.cache_dir).map_err(|error| {
            format!(
                "cannot create cache namespace {}: {error}",
                self.cache_dir.display()
            )
        })?;
        let lock_path = self.cache_dir.join(CACHE_TRIM_LOCK_FILE);
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| format!("cannot open {}: {error}", lock_path.display()))?;
        // One trimmer at a time; a busy lock means another process trims now.
        // Concurrent trims would only repeat work, so a platform without file
        // locks trims unlocked.
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
            Err(std::fs::TryLockError::Error(error))
                if error.kind() == std::io::ErrorKind::Unsupported => {}
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("cannot lock {}: {error}", lock_path.display()));
            }
        }
        let stamp_path = self.cache_dir.join(CACHE_TRIM_STAMP_FILE);
        if recheck_due && !trim_due(&stamp_path, now) {
            return Ok(());
        }
        self.telemetry.trims = self.telemetry.trims.saturating_add(1);
        self.trim_functions(now)?;
        if let Some(root) = self.namespace_root.clone() {
            self.retire_stale_namespaces(&root, now)?;
        }
        File::create(&stamp_path)
            .and_then(|stamp| stamp.set_modified(now))
            .map_err(|error| format!("cannot stamp {}: {error}", stamp_path.display()))
    }

    fn trim_functions(&mut self, now: SystemTime) -> Result<(), String> {
        let entries = match std::fs::read_dir(&self.functions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "cannot enumerate {}: {error}",
                    self.functions_dir.display()
                ));
            }
        };
        let mut tracked = Vec::new();
        let mut tracked_bytes = 0_u64;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("cannot enumerate {}: {error}", self.functions_dir.display())
            })?;
            let Ok(metadata) = entry.metadata() else {
                // Retired concurrently.
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let name = entry.file_name();
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            match classify_namespace_file(&name) {
                NamespaceFile::Tracked => {
                    tracked_bytes = tracked_bytes.saturating_add(metadata.len());
                    tracked.push((modified, name, metadata.len()));
                }
                NamespaceFile::Temporary => {
                    // A live writer links its temporary within milliseconds.
                    if age(now, modified) >= CACHE_RECENCY_INTERVAL {
                        retire_cache_file(&entry.path())?;
                        self.telemetry.temp_files_retired =
                            self.telemetry.temp_files_retired.saturating_add(1);
                    }
                }
                NamespaceFile::Untracked => {
                    retire_cache_file(&entry.path())?;
                    self.telemetry.untracked_files_retired =
                        self.telemetry.untracked_files_retired.saturating_add(1);
                }
            }
        }
        // Oldest first; the name breaks ties, so eviction never depends on
        // directory iteration order.
        tracked.sort_unstable();
        let mut remaining = tracked.len();
        for (_, name, bytes) in tracked {
            if remaining <= self.max_entries && tracked_bytes <= self.max_persistent_bytes {
                break;
            }
            retire_cache_file(&self.functions_dir.join(&name))?;
            remaining -= 1;
            tracked_bytes = tracked_bytes.saturating_sub(bytes);
            self.telemetry.evicted_entries = self.telemetry.evicted_entries.saturating_add(1);
            self.telemetry.evicted_persistent_bytes = self
                .telemetry
                .evicted_persistent_bytes
                .saturating_add(bytes);
        }
        Ok(())
    }

    /// Retire sibling backend namespaces that no process used within the
    /// namespace limit. Only directories with a canonical namespace name and
    /// a `functions` directory are namespaces; everything else under the root
    /// belongs to other caches.
    fn retire_stale_namespaces(&mut self, root: &Path, now: SystemTime) -> Result<(), String> {
        let own = self.cache_dir.file_name().map(OsStr::to_os_string);
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("cannot enumerate {}: {error}", root.display())),
        };
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot enumerate {}: {error}", root.display()))?;
            let name = entry.file_name();
            if own.as_deref() == Some(name.as_os_str())
                || !name.to_str().is_some_and(is_canonical_cache_hash)
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let namespace = entry.path();
            let Some(last_use) = namespace_last_use(&namespace) else {
                continue;
            };
            if age(now, last_use) < CACHE_NAMESPACE_UNUSED_LIMIT {
                continue;
            }
            match std::fs::remove_dir_all(&namespace) {
                Ok(()) => {
                    self.telemetry.stale_namespaces_retired =
                        self.telemetry.stale_namespaces_retired.saturating_add(1);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "cannot retire stale cache namespace {}: {error}",
                        namespace.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn artifact_path(&self, content_hash: &str) -> Option<PathBuf> {
        is_canonical_cache_hash(content_hash)
            .then(|| self.functions_dir.join(format!("{content_hash}.bin")))
    }

    fn poison_path(&self, content_hash: &str) -> Option<PathBuf> {
        is_canonical_cache_hash(content_hash)
            .then(|| self.functions_dir.join(format!("{content_hash}.poison")))
    }

    pub fn stats(&self) -> CompilationCacheStats {
        CompilationCacheStats {
            resident_entries: self.resident.len(),
            resident_bytes: self.resident.bytes(),
            max_entries: self.max_entries,
            max_resident_bytes: self.resident.max_bytes().expect("bounded TIR cache"),
            max_persistent_bytes: self.max_persistent_bytes,
            telemetry: self.telemetry,
        }
    }

    /// Serve a resident payload unless another process quarantined its key.
    fn resident_hit(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        let entry = self.resident.get(content_hash)?;
        let probe = quarantine_probe(&entry.poison_path);
        let bytes = Arc::clone(&entry.data);
        if !matches!(probe, Ok(false)) {
            self.forget_resident(content_hash);
            return None;
        }
        Some(bytes)
    }

    fn forget_resident(&mut self, content_hash: &str) {
        self.resident.remove(content_hash);
    }

    fn store_memory_data(&mut self, content_hash: &str, bytes: Arc<[u8]>) {
        if self
            .resident
            .max_bytes()
            .is_some_and(|max| max == 0 || bytes.len() > max)
        {
            self.forget_resident(content_hash);
            return;
        }
        let Some(poison_path) = self.poison_path(content_hash) else {
            return;
        };
        let allocation_bytes = content_hash
            .len()
            .saturating_add(4 * std::mem::size_of::<usize>()) // key/output Arc headers
            .saturating_add(bytes.len())
            .saturating_add(poison_path.capacity());
        self.resident.insert(
            Arc::<str>::from(content_hash),
            ResidentEntry {
                data: bytes,
                poison_path,
            },
            allocation_bytes,
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

enum NamespaceFile {
    /// `<hash>.bin` or `<hash>.poison`.
    Tracked,
    /// `.<stem>.tmp.<pid>.<serial>`, a writer's staging file.
    Temporary,
    Untracked,
}

fn classify_namespace_file(name: &OsStr) -> NamespaceFile {
    let Some(name) = name.to_str() else {
        return NamespaceFile::Untracked;
    };
    if name.starts_with('.') && name.contains(".tmp.") {
        return NamespaceFile::Temporary;
    }
    let tracked = name
        .strip_suffix(".bin")
        .or_else(|| name.strip_suffix(".poison"))
        .is_some_and(is_canonical_cache_hash);
    if tracked {
        NamespaceFile::Tracked
    } else {
        NamespaceFile::Untracked
    }
}

/// Elapsed time since `then`; a future timestamp (clock skew) counts as zero.
fn age(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or_default()
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

/// A trim is due when the stamp is missing, older than the trim interval, or
/// implausibly far in the future (a skewed clock must not disable trimming).
fn trim_due(stamp_path: &Path, now: SystemTime) -> bool {
    let Some(stamp) = modified(stamp_path) else {
        return true;
    };
    match now.duration_since(stamp) {
        Ok(elapsed) => elapsed >= CACHE_TRIM_INTERVAL,
        Err(skew) => skew.duration() > CACHE_TRIM_INTERVAL,
    }
}

/// Latest use of a namespace: its last trim (hourly while in use) or its
/// last artifact addition or removal. `None` if it is not a TIR namespace.
fn namespace_last_use(namespace: &Path) -> Option<SystemTime> {
    let functions = modified(&namespace.join(CACHE_FUNCTIONS_DIR))?;
    Some(
        modified(&namespace.join(CACHE_TRIM_STAMP_FILE))
            .map_or(functions, |stamp| stamp.max(functions)),
    )
}

/// Trimming evicts by modification time. Refresh it at most once per recency
/// interval, so hits rarely write (Go's build cache `markUsed`).
fn refresh_recency(path: &Path, now: SystemTime) {
    if modified(path).is_some_and(|then| age(now, then) < CACHE_RECENCY_INTERVAL) {
        return;
    }
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(now);
    }
}

fn quarantine_probe(poison_path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(poison_path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn install_poison_sidecar(
    functions_dir: &Path,
    poison_path: &Path,
    content_hash: &str,
    envelope: &[u8],
) -> Result<(), String> {
    let temp = write_unique_temp_file(functions_dir, &format!("{content_hash}.poison"), envelope)
        .map_err(|error| format!("cannot stage cache poison sidecar: {error}"))?;
    let linked = std::fs::hard_link(&temp, poison_path);
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(()),
        // Another writer quarantined the key first; one sidecar suffices.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!(
            "cannot install cache poison sidecar {}: {error}",
            poison_path.display()
        )),
    }
}

fn write_unique_temp_file(directory: &Path, stem: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
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

fn is_canonical_cache_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn encode_cache_envelope(content_hash: &str, kind: CacheEnvelopeKind, payload: &[u8]) -> Vec<u8> {
    debug_assert!(is_canonical_cache_hash(content_hash));
    let digest = Sha256::digest(payload);
    let mut envelope = Vec::with_capacity(CACHE_ARTIFACT_FIXED_BYTES + payload.len());
    envelope.extend_from_slice(CACHE_ARTIFACT_MAGIC);
    envelope.push(kind as u8);
    envelope.extend_from_slice(content_hash.as_bytes());
    envelope.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    envelope.extend_from_slice(&digest);
    envelope.extend_from_slice(payload);
    envelope
}

fn decode_cache_envelope<'a>(
    expected_hash: &str,
    envelope: &'a [u8],
) -> Result<(CacheEnvelopeKind, &'a [u8]), String> {
    if envelope.len() < CACHE_ARTIFACT_FIXED_BYTES {
        return Err("cache envelope is truncated".to_string());
    }
    let mut cursor = 0;
    let magic_end = CACHE_ARTIFACT_MAGIC.len();
    if &envelope[..magic_end] != CACHE_ARTIFACT_MAGIC {
        return Err("cache envelope magic/version mismatch".to_string());
    }
    cursor += magic_end;
    let kind = CacheEnvelopeKind::from_byte(envelope[cursor])?;
    cursor += 1;
    let key_end = cursor + CACHE_ARTIFACT_KEY_BYTES;
    if envelope.get(cursor..key_end) != Some(expected_hash.as_bytes()) {
        return Err("cache envelope key does not match requested key".to_string());
    }
    cursor = key_end;
    let length_end = cursor + 8;
    let payload_len = u64::from_be_bytes(
        envelope[cursor..length_end]
            .try_into()
            .expect("fixed-width cache envelope length"),
    );
    cursor = length_end;
    let digest_end = cursor + CACHE_ARTIFACT_DIGEST_BYTES;
    let expected_digest = &envelope[cursor..digest_end];
    cursor = digest_end;
    let payload_len = usize::try_from(payload_len)
        .map_err(|_| "cache envelope payload length exceeds address space".to_string())?;
    let expected_total = cursor
        .checked_add(payload_len)
        .ok_or_else(|| "cache envelope payload length overflow".to_string())?;
    if envelope.len() != expected_total {
        return Err(format!(
            "cache envelope length mismatch: declared {payload_len}, total {}",
            envelope.len()
        ));
    }
    let payload = &envelope[cursor..];
    if Sha256::digest(payload).as_slice() != expected_digest {
        return Err("cache envelope payload checksum mismatch".to_string());
    }
    Ok((kind, payload))
}

fn decode_artifact_envelope<'a>(
    expected_hash: &str,
    envelope: &'a [u8],
) -> Result<&'a [u8], String> {
    let (kind, payload) = decode_cache_envelope(expected_hash, envelope)?;
    if kind != CacheEnvelopeKind::Artifact {
        return Err("ordinary artifact path contains a non-artifact envelope".to_string());
    }
    Ok(payload)
}

fn payload_digest_hex(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn nondeterminism_poison_payload(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut identities = [
        (payload_digest_hex(left), left.len()),
        (payload_digest_hex(right), right.len()),
    ];
    identities.sort_unstable();
    format!(
        "same-key-different-payload|{}:{}|{}:{}",
        identities[0].0, identities[0].1, identities[1].0, identities[1].1
    )
    .into_bytes()
}

fn poison_envelope_for_limit(
    content_hash: &str,
    left: &[u8],
    right: &[u8],
    max_persistent_bytes: u64,
) -> Result<Vec<u8>, String> {
    let payload = nondeterminism_poison_payload(left, right);
    let detailed = encode_cache_envelope(content_hash, CacheEnvelopeKind::Poison, &payload);
    if detailed.len() as u64 <= max_persistent_bytes {
        return Ok(detailed);
    }
    let minimal = encode_cache_envelope(content_hash, CacheEnvelopeKind::Poison, &[]);
    if minimal.len() as u64 > max_persistent_bytes {
        return Err(format!(
            "cache key {content_hash} diverged but the configured persistent-byte limit cannot retain the minimal durable quarantine"
        ));
    }
    Ok(minimal)
}

fn read_file_bounded(path: &std::path::Path, max_bytes: u64) -> Result<Option<Vec<u8>>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot open bounded cache file {}: {error}",
                path.display()
            ));
        }
    };
    let initial_len = file
        .metadata()
        .map_err(|error| format!("cannot stat bounded cache file {}: {error}", path.display()))?
        .len();
    if initial_len > max_bytes {
        return Err(format!(
            "cache file {} is {initial_len} bytes, exceeding configured read bound {max_bytes}",
            path.display()
        ));
    }
    let capacity = usize::try_from(initial_len)
        .map_err(|_| format!("cache file {} exceeds address space", path.display()))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read bounded cache file {}: {error}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "cache file {} grew beyond configured read bound {max_bytes}",
            path.display()
        ));
    }
    Ok(Some(bytes))
}

fn read_poison_sidecar(
    content_hash: &str,
    poison_path: &std::path::Path,
    max_persistent_bytes: u64,
) -> Result<Option<Vec<u8>>, String> {
    let envelope = match read_file_bounded(poison_path, max_persistent_bytes) {
        Ok(Some(envelope)) => envelope,
        Ok(None) => return Ok(None),
        Err(error) => {
            return Err(error);
        }
    };
    let (kind, payload) = decode_cache_envelope(content_hash, &envelope)?;
    if kind != CacheEnvelopeKind::Poison {
        return Err("poison sidecar contains an ordinary artifact envelope".to_string());
    }
    Ok(Some(payload.to_vec()))
}

fn retire_cache_file(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot retire cache file {}: {error}",
            path.display()
        )),
    }
}

/// Root of every backend binary's cache namespace: `MOLT_CACHE` when
/// configured, `.molt_cache` otherwise.
#[expect(
    clippy::disallowed_methods,
    reason = "TIR cache placement and limits are process configuration, outside the backend catalog"
)]
fn backend_cache_root() -> PathBuf {
    std::env::var_os("MOLT_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".molt_cache"))
}

/// Resolve the canonical cache namespace for the current backend binary.
///
/// The namespace lives under [`backend_cache_root`] and is salted with the
/// current executable path, mtime and build identity, so cached optimized IR
/// is invalidated automatically when the backend binary changes.
pub fn backend_cache_dir() -> PathBuf {
    backend_cache_dir_under(&backend_cache_root())
}

fn backend_cache_dir_under(root: &Path) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mtime = std::fs::metadata(&exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let build_identity = backend_build_identity(&exe);
    backend_cache_dir_for(root, &exe, mtime, &build_identity)
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
    if let Some(provided) = molt_ir::backend_environment::var_os(
        molt_ir::backend_environment::BackendEnv::MOLT_BACKEND_COMPILER_FINGERPRINT,
    ) && !provided.is_empty()
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

#[cfg(target_os = "wasi")]
fn platform_os_str_identity(value: &OsStr) -> (&'static [u8], Vec<u8>) {
    // WASI has no target-specific OsStrExt. Keep std's lossless encoded bytes
    // opaque and bind them to an explicit target domain in the cache key.
    (b"wasi-encoded-bytes", value.as_encoded_bytes().to_vec())
}

#[cfg(not(any(unix, windows, target_os = "wasi")))]
fn platform_os_str_identity(value: &OsStr) -> (&'static [u8], Vec<u8>) {
    // Preserve compilation and lossless identity on future std targets without
    // pretending their platform encoding is Unix bytes or Windows UTF-16.
    (
        b"rust-platform-encoded-bytes",
        value.as_encoded_bytes().to_vec(),
    )
}

fn default_memory_cache_limit_bytes() -> usize {
    if let Some(mib) = env_cache_limit_bytes("MOLT_BACKEND_TIR_CACHE_MEMORY_MB") {
        return mib.saturating_mul(1024 * 1024);
    }
    if let Some(bytes) = crate::memory_budget::backend_memory_limit_bytes()
        .map(|bytes| usize::try_from(bytes).unwrap_or(usize::MAX))
    {
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

fn default_persistent_cache_max_entries() -> usize {
    env_cache_limit_bytes("MOLT_BACKEND_TIR_CACHE_MAX_ENTRIES")
        .unwrap_or(DEFAULT_PERSISTENT_CACHE_MAX_ENTRIES)
}

fn default_persistent_cache_max_bytes() -> u64 {
    env_cache_limit_u64("MOLT_BACKEND_TIR_CACHE_DISK_MB")
        .and_then(|mib| mib.checked_mul(1024 * 1024))
        .unwrap_or(DEFAULT_PERSISTENT_CACHE_MAX_BYTES)
}

#[expect(
    clippy::disallowed_methods,
    reason = "TIR cache placement and limits are process configuration, outside the backend catalog"
)]
fn env_cache_limit_bytes(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
}

#[expect(
    clippy::disallowed_methods,
    reason = "TIR cache placement and limits are process configuration, outside the backend catalog"
)]
fn env_cache_limit_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
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
mod tests;
