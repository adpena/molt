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
//! # molt-cache-index-v4 hash|last_access_unix_secs|envelope_bytes|state
//! <64 lowercase hex>|1679900000|4096|artifact
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

const BACKEND_CACHE_NAMESPACE_VERSION: &str =
    "molt-backend-tir-cache-v7-semantic-transaction-authority";
const CACHE_INDEX_HEADER: &str =
    "# molt-cache-index-v4 hash|last_access_unix_secs|envelope_bytes|state";
const CACHE_ARTIFACT_MAGIC: &[u8] = b"MOLT:CACHE-ARTIFACT:v1\0";
const CACHE_ARTIFACT_KEY_BYTES: usize = 64;
const CACHE_ARTIFACT_DIGEST_BYTES: usize = 32;
const CACHE_ARTIFACT_FIXED_BYTES: usize =
    CACHE_ARTIFACT_MAGIC.len() + 1 + CACHE_ARTIFACT_KEY_BYTES + 8 + CACHE_ARTIFACT_DIGEST_BYTES;
const CACHE_INDEX_MAX_LINE_BYTES: usize = 160;
const BACKEND_COMPILER_FINGERPRINT_ENV: &str = "MOLT_BACKEND_COMPILER_FINGERPRINT";
const DEFAULT_MEMORY_CACHE_BYTES_FALLBACK: usize = 64 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_AVAILABLE_BYTES_MIN: usize = 8 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_BYTES_MIN: usize = 32 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_BYTES_MAX: usize = 512 * 1024 * 1024;
const DEFAULT_MEMORY_CACHE_AVAILABLE_DIVISOR: usize = 128;
const DEFAULT_MEMORY_CACHE_TOTAL_DIVISOR: usize = 512;
const DEFAULT_PERSISTENT_CACHE_MAX_ENTRIES: usize = 65_536;
const DEFAULT_PERSISTENT_CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const CACHE_NAMESPACE_LOCK_FILE: &str = ".namespace.lock";
const CACHE_NAMESPACE_EPOCH_FILE: &str = ".namespace.epoch";
const CACHE_NAMESPACE_EPOCH_SLOT_BYTES: usize = 8;
const CACHE_NAMESPACE_EPOCH_SLOTS: usize = 2;
const CACHE_NAMESPACE_EPOCH_LIVE_OFFSET: usize = 0;
const CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET: usize = CACHE_NAMESPACE_EPOCH_SLOT_BYTES;
const CACHE_NAMESPACE_EPOCH_BYTES: usize = CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET
    + CACHE_NAMESPACE_EPOCH_SLOT_BYTES * CACHE_NAMESPACE_EPOCH_SLOTS;
const CACHE_NAMESPACE_EPOCH_VALUE_BITS: u32 = 31;
const CACHE_NAMESPACE_EPOCH_VALUE_MASK: u64 = (1_u64 << CACHE_NAMESPACE_EPOCH_VALUE_BITS) - 1;
const CACHE_NAMESPACE_EPOCH_TAG: u64 = 0b10_u64 << 62;

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

    /// One open OS lock handle per cache instance. Every namespace read holds
    /// a shared lock; every mutation/reconciliation holds an exclusive lock.
    namespace_lock: Option<File>,
    namespace_lock_error: Option<String>,

    /// Double-buffered, crash-consistent semantic namespace generation. Resident hits read
    /// this permanently open handle positionally before consulting memory.
    /// Even generations are quiescent; odd generations make every reader take
    /// the locked reconciliation path until the poison transition completes.
    namespace_epoch: Option<File>,
    namespace_epoch_error: Option<String>,
    observed_epoch_word: u64,
    #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
    namespace_epoch_map: Option<memmap2::MmapMut>,
    #[cfg(test)]
    resident_epoch_barrier: Option<Arc<std::sync::Barrier>>,
    #[cfg(test)]
    poison_epoch_barrier: Option<Arc<std::sync::Barrier>>,
    #[cfg(test)]
    namespace_reset_enumeration_error: bool,

    /// In-memory index: `content_hash` → [`CacheEntry`].
    index: HashMap<Arc<str>, CacheEntry>,

    /// Bytes currently retained in `CacheEntry::data`.
    memory_bytes: usize,

    /// Maximum bytes retained in-memory. Disk cache entries remain indexed.
    max_memory_bytes: usize,

    /// Monotonic logical clock for in-memory LRU eviction.
    memory_clock: u64,
    /// Indexed min-heap of resident entries. Touches mutate heap nodes in
    /// place, so warm hits allocate nothing and stale nodes never accumulate.
    memory_lru: Vec<MemoryLruNode>,

    /// Deterministic metadata/disk LRU. The tuple tie-breaks equal recency
    /// stamps by canonical content hash, so pruning never depends on hash-map
    /// or directory iteration order.
    metadata_lru: BTreeSet<(u64, Arc<str>)>,
    persistent_bytes: u64,
    max_entries: usize,
    max_persistent_bytes: u64,
    telemetry: CompilationCacheTelemetry,
}

/// A single entry in the compilation cache.
#[derive(Debug, Clone)]
struct CacheEntry {
    metadata_key: Arc<str>,
    poison_path: PathBuf,
    /// Unix timestamp (seconds) of last access.
    last_access: u64,
    /// Cached artifact bytes — `None` until loaded on demand.
    data: Option<Arc<[u8]>>,
    /// Logical LRU stamp for in-memory artifact bytes.
    memory_stamp: u64,
    /// Position in `CompilationCache::memory_lru` while data is resident.
    memory_lru_index: Option<usize>,
    /// Complete authenticated envelope bytes retained on disk. Zero denotes a
    /// memory-only entry.
    persistent_bytes: u64,
    /// A quarantined same-key/different-payload invariant violation. Poison
    /// envelopes are misses and reject subsequent writes until normal bounded
    /// cache pruning retires the entry.
    poisoned: bool,
}

#[derive(Debug)]
struct MemoryLruNode {
    stamp: u64,
    content_hash: Arc<str>,
}

struct HeldNamespaceLock {
    file: Option<File>,
    locked: bool,
}

impl HeldNamespaceLock {
    fn finish(mut self) -> (File, std::io::Result<()>) {
        let result = self
            .file
            .as_ref()
            .expect("held namespace lock owns a file")
            .unlock();
        self.locked = false;
        (
            self.file.take().expect("held namespace lock owns a file"),
            result,
        )
    }
}

impl Drop for HeldNamespaceLock {
    fn drop(&mut self) {
        if self.locked
            && let Some(file) = self.file.as_ref()
        {
            let _ = file.unlock();
        }
    }
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
    pub evicted_entries: u64,
    pub evicted_persistent_bytes: u64,
    pub orphan_artifacts_discovered: u64,
    pub dropped_index_rows: u64,
    pub limit_enforcement_failures: u64,
    pub untracked_files_retired: u64,
    pub untracked_bytes_retired: u64,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct CompilationCacheStats {
    pub entries: usize,
    pub resident_entries: usize,
    pub resident_bytes: usize,
    pub persistent_bytes: u64,
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
        Self::open_with_limits(
            cache_dir,
            default_memory_cache_limit_bytes(),
            default_persistent_cache_max_entries(),
            default_persistent_cache_max_bytes(),
        )
    }

    /// Create or open a compilation cache with an explicit in-memory cap.
    pub fn open_with_memory_limit(cache_dir: PathBuf, max_memory_bytes: usize) -> Self {
        Self::open_with_limits(
            cache_dir,
            max_memory_bytes,
            default_persistent_cache_max_entries(),
            default_persistent_cache_max_bytes(),
        )
    }

    /// Create or open a compilation cache with explicit resident, metadata,
    /// and persistent-byte ceilings.
    pub fn open_with_limits(
        cache_dir: PathBuf,
        max_memory_bytes: usize,
        max_entries: usize,
        max_persistent_bytes: u64,
    ) -> Self {
        let namespace_lock_result = std::fs::create_dir_all(&cache_dir).and_then(|()| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(cache_dir.join(CACHE_NAMESPACE_LOCK_FILE))
        });
        let (namespace_lock, namespace_lock_error) = match namespace_lock_result {
            Ok(file) => (Some(file), None),
            Err(error) => (
                None,
                Some(format!(
                    "persistent compilation cache cannot lock namespace {}: {error}",
                    cache_dir.display()
                )),
            ),
        };
        let namespace_epoch_result = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(cache_dir.join(CACHE_NAMESPACE_EPOCH_FILE));
        let (namespace_epoch, namespace_epoch_error) = match namespace_epoch_result {
            Ok(file) => (Some(file), None),
            Err(error) => (
                None,
                Some(format!(
                    "persistent compilation cache cannot open namespace epoch {}: {error}",
                    cache_dir.display()
                )),
            ),
        };
        let mut cache = Self {
            cache_dir,
            namespace_lock,
            namespace_lock_error,
            namespace_epoch,
            namespace_epoch_error,
            observed_epoch_word: encode_namespace_epoch_word(0),
            #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
            namespace_epoch_map: None,
            #[cfg(test)]
            resident_epoch_barrier: None,
            #[cfg(test)]
            poison_epoch_barrier: None,
            #[cfg(test)]
            namespace_reset_enumeration_error: false,
            index: HashMap::new(),
            memory_bytes: 0,
            max_memory_bytes,
            memory_clock: 0,
            memory_lru: Vec::new(),
            metadata_lru: BTreeSet::new(),
            persistent_bytes: 0,
            max_entries,
            max_persistent_bytes,
            telemetry: CompilationCacheTelemetry::default(),
        };
        let startup = cache.with_namespace_lock(true, |cache| {
            cache.initialize_namespace_epoch_locked()?;
            if let Err(error) = cache.map_namespace_epoch() {
                eprintln!("MOLT_CACHE: {error}; using allocation-free positioned epoch reads");
            }
            cache.reconcile_namespace_epoch_locked()?;
            cache.load_index_locked();
            let transition = cache.begin_namespace_mutation_locked()?;
            cache.reconcile_persistent_artifacts();
            cache.enforce_cache_limits();
            cache.finish_namespace_mutation_locked(transition)
        });
        match startup {
            Ok(Ok(epoch)) => {
                cache.observed_epoch_word = encode_namespace_epoch_word(epoch);
            }
            Ok(Err(error)) | Err(error) => {
                cache.namespace_epoch_error = Some(error.clone());
                eprintln!("MOLT_CACHE: {error}");
            }
        }
        cache
    }

    fn with_namespace_lock<R>(
        &mut self,
        exclusive: bool,
        operation: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, String> {
        let Some(file) = self.namespace_lock.take() else {
            return Err(self.namespace_lock_error.clone().unwrap_or_else(|| {
                "persistent compilation cache namespace lock is unavailable".to_string()
            }));
        };
        let lock_result = if exclusive {
            file.lock()
        } else {
            file.lock_shared()
        };
        if let Err(error) = lock_result {
            self.namespace_lock = Some(file);
            return Err(format!(
                "persistent compilation cache cannot acquire {} namespace lock: {error}",
                if exclusive { "exclusive" } else { "shared" }
            ));
        }
        let held = HeldNamespaceLock {
            file: Some(file),
            locked: true,
        };
        let result = operation(self);
        let (file, unlock_result) = held.finish();
        self.namespace_lock = Some(file);
        unlock_result.map_err(|error| {
            format!("persistent compilation cache cannot release namespace lock: {error}")
        })?;
        Ok(result)
    }

    fn initialize_namespace_epoch_locked(&self) -> Result<(), String> {
        let file = self.namespace_epoch.as_ref().ok_or_else(|| {
            self.namespace_epoch_error.clone().unwrap_or_else(|| {
                "persistent compilation cache namespace epoch is unavailable".to_string()
            })
        })?;
        let length = file
            .metadata()
            .map_err(|error| format!("cannot stat persistent cache namespace epoch: {error}"))?
            .len();
        if length == 0 {
            file.set_len(CACHE_NAMESPACE_EPOCH_BYTES as u64)
                .map_err(|error| {
                    format!("cannot size persistent cache namespace epoch: {error}")
                })?;
            initialize_namespace_epoch(file)?;
            return Ok(());
        }
        if length != CACHE_NAMESPACE_EPOCH_BYTES as u64 {
            return Err(format!(
                "persistent cache namespace epoch has invalid length {length}, expected {CACHE_NAMESPACE_EPOCH_BYTES}"
            ));
        }
        Ok(())
    }

    fn read_namespace_epoch(&self) -> Result<u64, String> {
        decode_namespace_epoch_word(self.read_namespace_epoch_word()?)
            .ok_or_else(|| "persistent cache live namespace epoch is torn".to_string())
    }

    fn read_namespace_epoch_word(&self) -> Result<u64, String> {
        #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
        if let Some(map) = self.namespace_epoch_map.as_ref() {
            return Ok(
                mapped_namespace_epoch_word(map, CACHE_NAMESPACE_EPOCH_LIVE_OFFSET)
                    .load(Ordering::Acquire),
            );
        }
        let file = self.namespace_epoch.as_ref().ok_or_else(|| {
            self.namespace_epoch_error.clone().unwrap_or_else(|| {
                "persistent compilation cache namespace epoch is unavailable".to_string()
            })
        })?;
        read_live_namespace_epoch_word(file)
    }

    fn read_durable_namespace_epoch(&self) -> Result<u64, String> {
        validate_namespace_epoch_slots(self.read_namespace_epoch_slots()?)
    }

    fn read_namespace_epoch_slots(&self) -> Result<[Option<u64>; 2], String> {
        #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
        if let Some(map) = self.namespace_epoch_map.as_ref() {
            return Ok(read_mapped_namespace_epoch_slots(map));
        }
        let file = self.namespace_epoch.as_ref().ok_or_else(|| {
            self.namespace_epoch_error.clone().unwrap_or_else(|| {
                "persistent compilation cache namespace epoch is unavailable".to_string()
            })
        })?;
        read_namespace_epoch_slots(file)
    }

    fn write_namespace_epoch_locked(&self, epoch: u64) -> Result<(), String> {
        if epoch > CACHE_NAMESPACE_EPOCH_VALUE_MASK {
            return Err("persistent cache namespace epoch exhausted".to_string());
        }
        #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
        if let Some(map) = self.namespace_epoch_map.as_ref() {
            return write_mapped_namespace_epoch(
                map,
                self.namespace_epoch.as_ref().ok_or_else(|| {
                    "persistent compilation cache namespace epoch is unavailable".to_string()
                })?,
                epoch,
            );
        }
        let file = self.namespace_epoch.as_ref().ok_or_else(|| {
            "persistent compilation cache namespace epoch is unavailable".to_string()
        })?;
        write_namespace_epoch(file, epoch)
    }

    fn map_namespace_epoch(&mut self) -> Result<(), String> {
        #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
        {
            let file = self.namespace_epoch.as_ref().ok_or_else(|| {
                "persistent compilation cache namespace epoch is unavailable".to_string()
            })?;
            // SAFETY: the epoch file is held open for the cache lifetime, is
            // sized once under the namespace lock, and is never truncated.
            // Every mapped participant accesses the two aligned u64 slots via
            // cross-process atomic loads/stores only.
            let map = unsafe {
                memmap2::MmapOptions::new()
                    .len(CACHE_NAMESPACE_EPOCH_BYTES)
                    .map_mut(file)
            }
            .map_err(|error| format!("cannot map persistent cache namespace epoch: {error}"))?;
            self.namespace_epoch_map = Some(map);
        }
        Ok(())
    }

    fn reconcile_namespace_epoch_locked(&mut self) -> Result<u64, String> {
        let live = self.read_namespace_epoch();
        let durable = self.read_durable_namespace_epoch();
        let epoch = match (live, durable) {
            (Ok(live), Ok(durable)) if live == durable && live & 1 == 0 => {
                let word = encode_namespace_epoch_word(live);
                if word != self.observed_epoch_word {
                    // Another process committed a semantic namespace mutation.
                    // The entire local view predates that transaction. Drop it
                    // without mutating the durable namespace while its epoch is
                    // quiescent; callers that intend to write perform their own
                    // odd-epoch reconciliation transaction. The advisory index
                    // is safe to reload read-only, and canonical artifacts absent
                    // from it are still discovered by `get_locked` on demand.
                    self.discard_local_namespace_view();
                    self.load_index_locked();
                }
                live
            }
            (Ok(live), Ok(durable)) if live == durable && live & 1 != 0 => {
                self.reset_interrupted_namespace_locked()?;
                let complete = next_namespace_epoch(live)?;
                self.write_namespace_epoch_locked(complete)?;
                complete
            }
            (Ok(live), Ok(durable)) if live & 1 != 0 && live.checked_sub(1) == Some(durable) => {
                // A writer published an odd live generation but did not durably
                // complete its mutation intent. The affected key set is unknown,
                // so preserving any prior artifact could resurrect a detected
                // same-key divergence. Fail closed by retiring the namespace.
                self.reset_interrupted_namespace_locked()?;
                self.write_namespace_epoch_locked(live)?;
                let complete = next_namespace_epoch(live)?;
                self.write_namespace_epoch_locked(complete)?;
                complete
            }
            (Ok(live), Ok(durable)) if live & 1 != 0 && live.checked_add(1) == Some(durable) => {
                // The mutation is durably complete but the live publication did
                // not reach every reader. Invalidate all pre-transaction leases
                // before publishing the completed generation.
                self.discard_local_namespace_view();
                self.load_index_locked();
                self.write_namespace_epoch_locked(durable)?;
                durable
            }
            (live, durable) => {
                let slots = self.read_namespace_epoch_slots()?;
                let mut recovered = slots
                    .into_iter()
                    .flatten()
                    .chain(live.ok())
                    .chain(durable.ok())
                    .max()
                    .ok_or_else(|| {
                        "persistent cache namespace epoch has no recoverable slot".to_string()
                    })?;
                self.reset_interrupted_namespace_locked()?;
                if recovered & 1 == 0 {
                    recovered = next_namespace_epoch(recovered)?;
                }
                // Rewriting the odd/even pair repairs both durable slots and
                // leaves a fail-closed live publication throughout recovery.
                self.write_namespace_epoch_locked(recovered)?;
                recovered = next_namespace_epoch(recovered)?;
                self.write_namespace_epoch_locked(recovered)?;
                recovered
            }
        };
        self.observed_epoch_word = encode_namespace_epoch_word(epoch);
        Ok(epoch)
    }

    fn discard_resident_data(&mut self) {
        for entry in self.index.values_mut() {
            entry.data = None;
            entry.memory_stamp = 0;
            entry.memory_lru_index = None;
        }
        self.memory_bytes = 0;
        self.memory_clock = 0;
        self.memory_lru.clear();
    }

    fn discard_local_namespace_view(&mut self) {
        self.index.clear();
        self.metadata_lru.clear();
        self.persistent_bytes = 0;
        self.discard_resident_data();
    }

    /// Recover an interrupted semantic mutation without guessing its key set.
    ///
    /// An odd or incoherent epoch proves that a writer may have observed
    /// same-key divergence before its durable poison sidecar became visible.
    /// The only sound key-independent recovery is to retire every canonical
    /// artifact and poison tombstone. Failure leaves the epoch non-quiescent,
    /// so every reader remains fail-closed.
    fn reset_interrupted_namespace_locked(&mut self) -> Result<(), String> {
        let functions_dir = self.cache_dir.join("functions");
        #[cfg(test)]
        if self.namespace_reset_enumeration_error {
            return Err("injected cache namespace enumeration failure".to_string());
        }
        match std::fs::read_dir(&functions_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| {
                        format!(
                            "cannot enumerate interrupted cache namespace {}: {error}",
                            functions_dir.display()
                        )
                    })?;
                    let file_type = entry.file_type().map_err(|error| {
                        format!(
                            "cannot classify interrupted cache namespace entry {}: {error}",
                            entry.path().display()
                        )
                    })?;
                    if !file_type.is_file() && !file_type.is_symlink() {
                        continue;
                    }
                    std::fs::remove_file(entry.path()).map_err(|error| {
                        format!(
                            "cannot retire interrupted cache namespace entry {}: {error}",
                            entry.path().display()
                        )
                    })?;
                }
                sync_cache_directory(&functions_dir)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot enumerate interrupted cache namespace {}: {error}",
                    functions_dir.display()
                ));
            }
        }
        self.discard_local_namespace_view();
        Ok(())
    }

    fn retire_namespace_after_quarantine_failure(
        &mut self,
        reason: String,
    ) -> CompilationCacheWriteError {
        match self.reset_interrupted_namespace_locked() {
            Ok(()) => CompilationCacheWriteError::Integrity(format!(
                "{reason}; the semantic cache namespace was retired fail-closed"
            )),
            Err(reset_error) => {
                self.namespace_epoch_error = Some(reset_error.clone());
                CompilationCacheWriteError::Unavailable(format!(
                    "{reason}; cache namespace retirement failed and the epoch remains non-quiescent: {reset_error}"
                ))
            }
        }
    }

    fn begin_namespace_mutation_locked(&mut self) -> Result<u64, String> {
        let current = self.read_durable_namespace_epoch()?;
        if current & 1 != 0 {
            return Err(format!(
                "persistent cache namespace epoch {current} is already in transition"
            ));
        }
        let transition = next_namespace_epoch(current)?;
        self.write_namespace_epoch_locked(transition)?;
        Ok(transition)
    }

    fn finish_namespace_mutation_locked(&mut self, transition: u64) -> Result<u64, String> {
        debug_assert_eq!(transition & 1, 1);
        self.sync_namespace_directories_locked()?;
        let complete = next_namespace_epoch(transition)?;
        self.write_namespace_epoch_locked(complete)?;
        self.observed_epoch_word = encode_namespace_epoch_word(complete);
        Ok(complete)
    }

    fn sync_namespace_directories_locked(&self) -> Result<(), String> {
        let functions_dir = self.cache_dir.join("functions");
        match std::fs::metadata(&functions_dir) {
            Ok(metadata) if metadata.is_dir() => sync_cache_directory(&functions_dir)?,
            Ok(_) => {
                return Err(format!(
                    "persistent cache functions namespace is not a directory: {}",
                    functions_dir.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot stat persistent cache functions namespace {}: {error}",
                    functions_dir.display()
                ));
            }
        }
        sync_cache_directory(&self.cache_dir)
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_poison_transition_locked(
        &mut self,
        functions_dir: &std::path::Path,
        artifact_path: &std::path::Path,
        poison_path: &std::path::Path,
        content_hash: &str,
        left: &[u8],
        right: &[u8],
        max_persistent_bytes: u64,
    ) -> Result<u64, String> {
        #[cfg(test)]
        if let Some(barrier) = self.poison_epoch_barrier.as_ref() {
            barrier.wait();
            barrier.wait();
        }
        let bytes = persist_poison_sidecar(
            functions_dir,
            artifact_path,
            poison_path,
            content_hash,
            left,
            right,
            max_persistent_bytes,
        )?;
        Ok(bytes)
    }

    fn resident_candidate(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        let bytes = self
            .index
            .get(content_hash)
            .and_then(|entry| entry.data.clone())?;
        self.touch_memory_entry(content_hash);
        Some(bytes)
    }

    fn resident_hit(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        let bytes = self.resident_candidate(content_hash)?;
        self.telemetry.memory_hits = self.telemetry.memory_hits.saturating_add(1);
        Some(bytes)
    }

    /// Look up a cached artifact by content hash.
    ///
    /// Updates `last_access` on a hit. If advisory index metadata is missing,
    /// the canonical content-addressed artifact path is still consulted so a
    /// concurrent index writer cannot hide a valid artifact.
    ///
    /// A returned [`Arc`] is a point-in-time lease: poison installed after the
    /// second equal even epoch sample cannot revoke an already-returned value,
    /// matching the former shared-lock lifetime. A get overlapping any observed
    /// poison transition discards its speculative resident clone and reconciles
    /// under the exclusive namespace lock.
    pub fn get(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        if !is_canonical_cache_hash(content_hash) {
            self.telemetry.misses = self.telemetry.misses.saturating_add(1);
            return None;
        }
        let resident = self
            .index
            .get(content_hash)
            .is_some_and(|entry| entry.data.is_some());
        if resident
            && let Ok(epoch_before) = self.read_namespace_epoch_word()
            && epoch_before == self.observed_epoch_word
            && let Some(bytes) = self.resident_candidate(content_hash)
        {
            #[cfg(test)]
            if let Some(barrier) = self.resident_epoch_barrier.as_ref() {
                barrier.wait();
                barrier.wait();
            }
            let epoch_after = self.read_namespace_epoch_word();
            #[cfg(test)]
            if let Some(barrier) = self.resident_epoch_barrier.as_ref() {
                barrier.wait();
                barrier.wait();
            }
            if epoch_after.is_ok_and(|epoch_after| epoch_after == epoch_before) {
                self.telemetry.memory_hits = self.telemetry.memory_hits.saturating_add(1);
                return Some(bytes);
            }
        }
        // Disk misses are already I/O-bound, so the slow path takes the
        // exclusive lock and repairs or rejects epoch state before reading.
        // This keeps one fail-closed authority for resident and nonresident
        // reads and permits no log-and-serve fallback after reconciliation
        // failure.
        let result = self.with_namespace_lock(true, |cache| {
            cache.reconcile_namespace_epoch_locked()?;
            Ok::<_, String>(cache.get_locked(content_hash))
        });
        match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) | Err(error) => {
                self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                eprintln!("MOLT_CACHE: {error}");
                None
            }
        }
    }

    fn get_locked(&mut self, content_hash: &str) -> Option<Arc<[u8]>> {
        let now = unix_now();
        let poison_path = if let Some(entry) = self.index.get(content_hash) {
            entry.poison_path.clone()
        } else {
            self.poison_path(content_hash)?
        };
        let poison_state =
            read_poison_sidecar(content_hash, &poison_path, self.max_persistent_bytes);
        match poison_state {
            Ok(Some(_)) => {
                let path = self.artifact_path(content_hash)?;
                let persistent_bytes = persistent_entry_bytes(&path, &poison_path);
                self.upsert_metadata_entry(content_hash, now, persistent_bytes, true);
                self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                return None;
            }
            Ok(None) => {
                if self
                    .index
                    .get(content_hash)
                    .is_some_and(|entry| entry.poisoned)
                {
                    self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                    return None;
                }
            }
            Err(error) => {
                self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                let path = self.artifact_path(content_hash)?;
                let persistent_bytes = persistent_entry_bytes(&path, &poison_path);
                self.upsert_metadata_entry(content_hash, now, persistent_bytes, true);
                eprintln!(
                    "MOLT_CACHE: refusing cache key {content_hash} with invalid poison sidecar: {error}"
                );
                self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                return None;
            }
        }

        if let Some(bytes) = self.resident_hit(content_hash) {
            return Some(bytes);
        }

        let path = self.artifact_path(content_hash)?;
        let poison_path = self.poison_path(content_hash)?;
        match read_file_bounded(&path, self.max_persistent_bytes) {
            Ok(Some(envelope)) => {
                match read_poison_sidecar(content_hash, &poison_path, self.max_persistent_bytes) {
                    Ok(Some(_)) => {
                        let persistent_bytes = persistent_entry_bytes(&path, &poison_path);
                        self.upsert_metadata_entry(content_hash, now, persistent_bytes, true);
                        self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                        return None;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                        let persistent_bytes = persistent_entry_bytes(&path, &poison_path);
                        self.upsert_metadata_entry(content_hash, now, persistent_bytes, true);
                        eprintln!(
                            "MOLT_CACHE: refusing cache key {content_hash} after bounded read with invalid poison sidecar: {error}"
                        );
                        self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                        return None;
                    }
                }
                match decode_artifact_envelope(content_hash, &envelope) {
                    Ok(payload) => {
                        let bytes = Arc::<[u8]>::from(payload);
                        self.upsert_metadata_entry(content_hash, now, envelope.len() as u64, false);
                        self.store_memory_data(content_hash, Arc::clone(&bytes));
                        self.telemetry.disk_hits = self.telemetry.disk_hits.saturating_add(1);
                        Some(bytes)
                    }
                    Err(error) => {
                        self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                        eprintln!(
                            "MOLT_CACHE: quarantining corrupt artifact {content_hash}: {error}"
                        );
                        self.remove_entry(content_hash, false);
                        self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                        None
                    }
                }
            }
            Ok(None) => {
                self.remove_entry(content_hash, false);
                self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                None
            }
            Err(error) => {
                self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                eprintln!(
                    "MOLT_CACHE: cannot read cache artifact {}: {error}",
                    path.display()
                );
                self.remove_entry(content_hash, false);
                self.telemetry.misses = self.telemetry.misses.saturating_add(1);
                None
            }
        }
    }

    /// Store a compilation artifact in memory and on disk.
    ///
    /// Creates `cache_dir/functions/` if it does not exist.  If an entry
    /// with the same `content_hash` already exists, that immutable
    /// content-addressed artifact is reused.
    pub fn put(
        &mut self,
        content_hash: &str,
        artifact: &[u8],
    ) -> Result<(), CompilationCacheWriteError> {
        match self.with_namespace_lock(true, |cache| {
            cache
                .reconcile_namespace_epoch_locked()
                .map_err(CompilationCacheWriteError::Unavailable)?;
            let transition = cache
                .begin_namespace_mutation_locked()
                .map_err(CompilationCacheWriteError::Unavailable)?;
            cache.load_index_locked();
            let result = cache.put_locked(content_hash, artifact);
            if cache.namespace_epoch_error.is_some() {
                return result;
            }
            cache
                .finish_namespace_mutation_locked(transition)
                .map_err(CompilationCacheWriteError::Unavailable)?;
            result
        }) {
            Ok(result) => result,
            Err(error) => Err(CompilationCacheWriteError::Unavailable(error)),
        }
    }

    fn put_locked(
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
        self.reconcile_persistent_artifacts();
        self.enforce_cache_limits();
        let funcs_dir = self.cache_dir.join("functions");
        std::fs::create_dir_all(&funcs_dir).map_err(|error| {
            CompilationCacheWriteError::Unavailable(format!(
                "persistent compilation cache cannot create {}: {error}",
                funcs_dir.display()
            ))
        })?;
        let artifact_path = self
            .artifact_path(content_hash)
            .expect("validated cache hash has a canonical artifact path");
        let poison_path = self
            .poison_path(content_hash)
            .expect("validated cache hash has a canonical poison path");
        match read_poison_sidecar(content_hash, &poison_path, self.max_persistent_bytes) {
            Ok(Some(poison)) => {
                self.upsert_metadata_entry(
                    content_hash,
                    unix_now(),
                    persistent_entry_bytes(&artifact_path, &poison_path),
                    true,
                );
                self.enforce_cache_limits();
                return Err(CompilationCacheWriteError::Integrity(format!(
                    "cache key {content_hash} is quarantined: {}",
                    String::from_utf8_lossy(&poison)
                )));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(CompilationCacheWriteError::Integrity(format!(
                    "cache key {content_hash} has invalid immutable poison state: {error}"
                )));
            }
        }
        if let Some((poisoned, resident, indexed_persistent_bytes)) = self
            .index
            .get(content_hash)
            .map(|entry| (entry.poisoned, entry.data.clone(), entry.persistent_bytes))
        {
            if poisoned {
                return Err(CompilationCacheWriteError::Integrity(format!(
                    "cache key {content_hash} is quarantined"
                )));
            }
            if let Some(existing) = resident.as_deref() {
                if existing == artifact {
                    self.telemetry.reuses = self.telemetry.reuses.saturating_add(1);
                    self.touch_metadata_entry(content_hash, unix_now());
                    return Ok(());
                }
                self.telemetry.nondeterminism_quarantines =
                    self.telemetry.nondeterminism_quarantines.saturating_add(1);
                let quarantine_time = unix_now();
                let poison_required = poison_envelope_for_limit(
                    content_hash,
                    existing,
                    artifact,
                    self.max_persistent_bytes,
                )
                .map(|envelope| envelope.len() as u64)
                .unwrap_or(0);
                if poison_required > 0
                    && !self.reserve_persistent_capacity(content_hash, poison_required)
                {
                    return Err(self.retire_namespace_after_quarantine_failure(format!(
                        "same cache key {content_hash} produced different resident payloads and durable quarantine cannot fit within namespace limits"
                    )));
                }
                self.upsert_metadata_entry(
                    content_hash,
                    quarantine_time,
                    indexed_persistent_bytes,
                    true,
                );
                let poison_bytes = match self.persist_poison_transition_locked(
                    &funcs_dir,
                    &artifact_path,
                    &poison_path,
                    content_hash,
                    existing,
                    artifact,
                    self.max_persistent_bytes,
                ) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return Err(self.retire_namespace_after_quarantine_failure(format!(
                            "same cache key {content_hash} produced different resident payloads and durable quarantine was unavailable: {error}"
                        )));
                    }
                };
                self.upsert_metadata_entry(content_hash, quarantine_time, poison_bytes, true);
                self.enforce_cache_limits();
                return Err(CompilationCacheWriteError::Integrity(format!(
                    "same cache key {content_hash} produced different resident payloads"
                )));
            }
        }
        match read_file_bounded(&artifact_path, self.max_persistent_bytes) {
            Ok(Some(existing_envelope)) => {
                match decode_artifact_envelope(content_hash, &existing_envelope) {
                    Ok(existing) if existing == artifact => {
                        self.telemetry.reuses = self.telemetry.reuses.saturating_add(1);
                        self.upsert_metadata_entry(
                            content_hash,
                            unix_now(),
                            existing_envelope.len() as u64,
                            false,
                        );
                        self.store_memory_data(content_hash, Arc::from(artifact));
                        return Ok(());
                    }
                    Ok(existing) => {
                        self.telemetry.nondeterminism_quarantines =
                            self.telemetry.nondeterminism_quarantines.saturating_add(1);
                        let poison_required = poison_envelope_for_limit(
                            content_hash,
                            existing,
                            artifact,
                            self.max_persistent_bytes,
                        )
                        .map(|envelope| envelope.len() as u64)
                        .unwrap_or(0);
                        if poison_required > 0
                            && !self.reserve_persistent_capacity(content_hash, poison_required)
                        {
                            return Err(self.retire_namespace_after_quarantine_failure(format!(
                                "same cache key {content_hash} produced different disk payloads and durable quarantine cannot fit within namespace limits"
                            )));
                        }
                        let poison_bytes = match self.persist_poison_transition_locked(
                            &funcs_dir,
                            &artifact_path,
                            &poison_path,
                            content_hash,
                            existing,
                            artifact,
                            self.max_persistent_bytes,
                        ) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                return Err(self.retire_namespace_after_quarantine_failure(
                                        format!(
                                            "same cache key {content_hash} produced different disk payloads and durable quarantine was unavailable: {error}"
                                        ),
                                    ));
                            }
                        };
                        self.upsert_metadata_entry(content_hash, unix_now(), poison_bytes, true);
                        self.enforce_cache_limits();
                        return Err(CompilationCacheWriteError::Integrity(format!(
                            "same cache key {content_hash} produced different authenticated payloads"
                        )));
                    }
                    Err(_) => {
                        self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                        if let Err(error) = std::fs::remove_file(&artifact_path)
                            && error.kind() != std::io::ErrorKind::NotFound
                        {
                            return Err(CompilationCacheWriteError::Unavailable(format!(
                                "cannot retire corrupt cache artifact {}: {error}",
                                artifact_path.display()
                            )));
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.telemetry.corruptions = self.telemetry.corruptions.saturating_add(1);
                if let Err(remove_error) = std::fs::remove_file(&artifact_path)
                    && remove_error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(CompilationCacheWriteError::Unavailable(format!(
                        "cannot retire unreadable bounded cache artifact {} ({error}): {remove_error}",
                        artifact_path.display()
                    )));
                }
            }
        }
        let envelope = encode_cache_envelope(content_hash, CacheEnvelopeKind::Artifact, artifact);
        let persistent_bytes = if self.max_entries > 0
            && (envelope.len() as u64) <= self.max_persistent_bytes
        {
            let required_bytes = envelope.len() as u64;
            if !self.reserve_persistent_capacity(content_hash, required_bytes) {
                return Err(CompilationCacheWriteError::Unavailable(format!(
                    "persistent compilation cache cannot reserve {required_bytes} bytes for {content_hash} within configured namespace limits"
                )));
            }
            install_authenticated_artifact(&funcs_dir, &artifact_path, content_hash, &envelope)
                .map_err(CompilationCacheWriteError::Unavailable)?;
            self.telemetry.writes = self.telemetry.writes.saturating_add(1);
            std::fs::metadata(&artifact_path)
                .map(|metadata| metadata.len())
                .unwrap_or(envelope.len() as u64)
        } else {
            if let Err(error) = std::fs::remove_file(&artifact_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(CompilationCacheWriteError::Unavailable(format!(
                    "persistent compilation cache cannot retire oversized artifact {}: {error}",
                    artifact_path.display()
                )));
            }
            0
        };

        self.upsert_metadata_entry(content_hash, unix_now(), persistent_bytes, false);
        self.store_memory_data(content_hash, Arc::from(artifact));
        self.enforce_cache_limits();
        Ok(())
    }

    /// Persist the cache index to `cache_dir/index.txt`.
    ///
    /// Entries are sorted by hash so identical metadata serializes identically
    /// across processes. I/O failures are returned to the caller: the cache is
    /// advisory, but falsely reporting a persisted index is not.
    pub fn save_index(&mut self) -> Result<(), String> {
        self.with_namespace_lock(true, |cache| {
            cache.reconcile_namespace_epoch_locked()?;
            let transition = cache.begin_namespace_mutation_locked()?;
            let result = cache.save_index_locked();
            cache.finish_namespace_mutation_locked(transition)?;
            result
        })?
    }

    fn save_index_locked(&mut self) -> Result<(), String> {
        // Merge the latest persisted recency while the namespace lock is held.
        // A process may never overwrite another process's newer touches with
        // its stale local snapshot.
        self.project_resident_metadata_recency();
        self.load_index_locked();
        self.reconcile_persistent_artifacts();
        self.enforce_cache_limits();
        std::fs::create_dir_all(&self.cache_dir).map_err(|error| {
            format!(
                "persistent compilation cache cannot create {}: {error}",
                self.cache_dir.display()
            )
        })?;
        let index_path = self.cache_dir.join("index.txt");

        let mut lines = format!("{CACHE_INDEX_HEADER}\n");
        let mut entries = self.index.iter().collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(content_hash, _)| *content_hash);
        for (content_hash, entry) in entries {
            if entry.persistent_bytes == 0 {
                continue;
            }
            let state = if entry.poisoned { "poison" } else { "artifact" };
            lines.push_str(&format!(
                "{content_hash}|{}|{}|{state}\n",
                entry.last_access, entry.persistent_bytes
            ));
        }

        // Atomic write: write to PID-unique temp file then rename so concurrent
        // readers never see a partially-written index.
        let tmp_path = write_unique_temp_file(&self.cache_dir, "index.txt", lines.as_bytes())
            .map_err(|error| format!("cannot write cache index temporary file: {error}"))?;
        if let Err(first_error) = std::fs::rename(&tmp_path, &index_path) {
            // Windows rename does not replace an existing target. The index is
            // advisory and artifacts are independently discoverable, so a
            // same-directory remove+rename fallback is safe and bounded.
            if let Err(error) = std::fs::remove_file(&index_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(format!(
                    "cannot replace cache index {} after rename failed ({first_error}): {error}",
                    index_path.display()
                ));
            }
            if let Err(error) = std::fs::rename(&tmp_path, &index_path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(format!(
                    "cannot install cache index {} after rename failed ({first_error}): {error}",
                    index_path.display()
                ));
            }
        }
        Ok(())
    }

    /// Load the cache index from `cache_dir/index.txt`.
    ///
    /// Artifacts are not read eagerly; they are loaded on demand by [`get`]. A
    /// missing/wrong version header rejects the whole old-format index. Invalid
    /// hashes, timestamps, and extra columns are skipped fail-closed.
    fn load_index_locked(&mut self) {
        let index_path = self.cache_dir.join("index.txt");
        let Ok(file) = std::fs::File::open(&index_path) else {
            return;
        };
        let max_scan_bytes = (CACHE_INDEX_HEADER.len() + 1).saturating_add(
            self.max_entries
                .saturating_add(64)
                .saturating_mul(CACHE_INDEX_MAX_LINE_BYTES + 1),
        ) as u64;
        if file
            .metadata()
            .is_ok_and(|metadata| metadata.len() > max_scan_bytes)
        {
            self.telemetry.dropped_index_rows = self.telemetry.dropped_index_rows.saturating_add(1);
        }
        let mut reader = std::io::BufReader::new(file.take(max_scan_bytes));
        let Ok(Some(Ok(header))) = read_bounded_line(&mut reader, CACHE_INDEX_MAX_LINE_BYTES)
        else {
            return;
        };
        if header != CACHE_INDEX_HEADER.as_bytes() {
            return;
        }
        let mut selected = BTreeMap::<(u64, String), (u64, bool)>::new();
        loop {
            let line = match read_bounded_line(&mut reader, CACHE_INDEX_MAX_LINE_BYTES) {
                Ok(Some(Ok(line))) => line,
                Ok(Some(Err(()))) => {
                    self.telemetry.dropped_index_rows =
                        self.telemetry.dropped_index_rows.saturating_add(1);
                    continue;
                }
                Ok(None) => break,
                Err(_) => return,
            };
            let Ok(line) = std::str::from_utf8(&line) else {
                self.telemetry.dropped_index_rows =
                    self.telemetry.dropped_index_rows.saturating_add(1);
                continue;
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split('|');
            let (Some(content_hash), Some(last_access), Some(persistent_bytes), Some(state)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if fields.next().is_some() || !is_canonical_cache_hash(content_hash) {
                continue;
            }
            let Ok(last_access) = last_access.parse::<u64>() else {
                continue;
            };
            let Ok(persistent_bytes) = persistent_bytes.parse::<u64>() else {
                continue;
            };
            let poisoned = match state {
                "artifact" => false,
                "poison" => true,
                _ => continue,
            };
            selected.insert(
                (last_access, content_hash.to_owned()),
                (persistent_bytes, poisoned),
            );
            while selected.len() > self.max_entries {
                selected.pop_first();
                self.telemetry.dropped_index_rows =
                    self.telemetry.dropped_index_rows.saturating_add(1);
            }
        }
        for ((last_access, content_hash), (persistent_bytes, poisoned)) in selected {
            let (last_access, persistent_bytes, poisoned) = self
                .index
                .get(content_hash.as_str())
                .map_or((last_access, persistent_bytes, poisoned), |entry| {
                    (
                        last_access.max(entry.last_access),
                        persistent_bytes.max(entry.persistent_bytes),
                        poisoned || entry.poisoned,
                    )
                });
            self.upsert_metadata_entry(&content_hash, last_access, persistent_bytes, poisoned);
        }
    }

    fn artifact_path(&self, content_hash: &str) -> Option<PathBuf> {
        is_canonical_cache_hash(content_hash).then(|| {
            self.cache_dir
                .join("functions")
                .join(format!("{content_hash}.bin"))
        })
    }

    fn poison_path(&self, content_hash: &str) -> Option<PathBuf> {
        is_canonical_cache_hash(content_hash).then(|| {
            self.cache_dir
                .join("functions")
                .join(format!("{content_hash}.poison"))
        })
    }

    pub fn stats(&self) -> CompilationCacheStats {
        CompilationCacheStats {
            entries: self.index.len(),
            resident_entries: self.memory_lru.len(),
            resident_bytes: self.memory_bytes,
            persistent_bytes: self.persistent_bytes,
            max_entries: self.max_entries,
            max_resident_bytes: self.max_memory_bytes,
            max_persistent_bytes: self.max_persistent_bytes,
            telemetry: self.telemetry,
        }
    }

    fn touch_metadata_entry(&mut self, content_hash: &str, now: u64) {
        let Some(entry) = self.index.get_mut(content_hash) else {
            return;
        };
        if entry.last_access == now {
            return;
        }
        let metadata_key = Arc::clone(&entry.metadata_key);
        self.metadata_lru
            .remove(&(entry.last_access, Arc::clone(&metadata_key)));
        entry.last_access = now;
        self.metadata_lru.insert((now, metadata_key));
    }

    fn upsert_metadata_entry(
        &mut self,
        content_hash: &str,
        last_access: u64,
        persistent_bytes: u64,
        poisoned: bool,
    ) {
        let poison_path = self
            .poison_path(content_hash)
            .expect("metadata keys are canonical cache hashes");
        if let Some(entry) = self.index.get_mut(content_hash) {
            let metadata_key = Arc::clone(&entry.metadata_key);
            self.metadata_lru
                .remove(&(entry.last_access, Arc::clone(&metadata_key)));
            self.persistent_bytes = self.persistent_bytes.saturating_sub(entry.persistent_bytes);
            entry.last_access = last_access;
            entry.persistent_bytes = persistent_bytes;
            entry.poisoned = poisoned;
        } else {
            let metadata_key: Arc<str> = Arc::from(content_hash);
            self.index.insert(
                Arc::clone(&metadata_key),
                CacheEntry {
                    metadata_key: Arc::clone(&metadata_key),
                    poison_path,
                    last_access,
                    data: None,
                    memory_stamp: 0,
                    memory_lru_index: None,
                    persistent_bytes,
                    poisoned,
                },
            );
        }
        self.persistent_bytes = self.persistent_bytes.saturating_add(persistent_bytes);
        let metadata_key = Arc::clone(&self.index[content_hash].metadata_key);
        self.metadata_lru.insert((last_access, metadata_key));
        if poisoned {
            if let Some(lru_index) = self
                .index
                .get(content_hash)
                .and_then(|entry| entry.memory_lru_index)
            {
                self.memory_lru_remove(lru_index);
            }
            if let Some(entry) = self.index.get_mut(content_hash)
                && let Some(bytes) = entry.data.take()
            {
                self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
                entry.memory_stamp = 0;
            }
        }
    }

    fn remove_entry(&mut self, content_hash: &str, remove_persistent: bool) -> bool {
        let Some(snapshot) = self.index.get(content_hash).cloned() else {
            return true;
        };
        if remove_persistent && snapshot.persistent_bytes > 0 {
            let Some(path) = self.artifact_path(content_hash) else {
                return false;
            };
            let Some(poison_path) = self.poison_path(content_hash) else {
                return false;
            };
            for candidate in [&path, &poison_path] {
                if let Err(error) = std::fs::remove_file(candidate)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    eprintln!(
                        "MOLT_CACHE: failed to enforce cache bound for {}: {error}",
                        candidate.display()
                    );
                    return false;
                }
            }
        }
        if let Some(lru_index) = snapshot.memory_lru_index {
            self.memory_lru_remove(lru_index);
        }
        let entry = self
            .index
            .remove(content_hash)
            .expect("cache entry existed before removal");
        self.metadata_lru
            .remove(&(entry.last_access, Arc::clone(&entry.metadata_key)));
        self.persistent_bytes = self.persistent_bytes.saturating_sub(entry.persistent_bytes);
        if let Some(bytes) = entry.data {
            self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
        }
        true
    }

    fn enforce_cache_limits(&mut self) {
        if self.index.len() > self.max_entries || self.persistent_bytes > self.max_persistent_bytes
        {
            self.project_resident_metadata_recency();
        }
        while self.index.len() > self.max_entries
            || self.persistent_bytes > self.max_persistent_bytes
        {
            let Some((_, content_hash)) = self.metadata_lru.first().cloned() else {
                break;
            };
            let persistent_bytes = self
                .index
                .get(content_hash.as_ref())
                .map(|entry| entry.persistent_bytes)
                .unwrap_or(0);
            if !self.remove_entry(&content_hash, true) {
                self.telemetry.limit_enforcement_failures =
                    self.telemetry.limit_enforcement_failures.saturating_add(1);
                break;
            }
            self.telemetry.evicted_entries = self.telemetry.evicted_entries.saturating_add(1);
            self.telemetry.evicted_persistent_bytes = self
                .telemetry
                .evicted_persistent_bytes
                .saturating_add(persistent_bytes);
        }
    }

    fn reserve_persistent_capacity(&mut self, content_hash: &str, required_bytes: u64) -> bool {
        if required_bytes > self.max_persistent_bytes || self.max_entries == 0 {
            return false;
        }
        self.project_resident_metadata_recency();
        loop {
            let current = self
                .index
                .get(content_hash)
                .map_or(0, |entry| entry.persistent_bytes);
            let projected_bytes = self
                .persistent_bytes
                .saturating_sub(current)
                .saturating_add(required_bytes);
            let projected_entries =
                self.index.len() + usize::from(!self.index.contains_key(content_hash));
            if projected_bytes <= self.max_persistent_bytes && projected_entries <= self.max_entries
            {
                return true;
            }
            let Some((_, victim)) = self
                .metadata_lru
                .iter()
                .find(|(_, hash)| hash.as_ref() != content_hash)
                .cloned()
            else {
                return false;
            };
            let victim_bytes = self
                .index
                .get(victim.as_ref())
                .map_or(0, |entry| entry.persistent_bytes);
            if !self.remove_entry(victim.as_ref(), true) {
                self.telemetry.limit_enforcement_failures =
                    self.telemetry.limit_enforcement_failures.saturating_add(1);
                return false;
            }
            self.telemetry.evicted_entries = self.telemetry.evicted_entries.saturating_add(1);
            self.telemetry.evicted_persistent_bytes = self
                .telemetry
                .evicted_persistent_bytes
                .saturating_add(victim_bytes);
        }
    }

    fn project_resident_metadata_recency(&mut self) {
        if self.memory_lru.is_empty() {
            return;
        }
        let mut resident_order = self
            .memory_lru
            .iter()
            .map(|node| (node.stamp, Arc::clone(&node.content_hash)))
            .collect::<Vec<_>>();
        resident_order.sort_unstable_by(|left, right| {
            (left.0, left.1.as_ref()).cmp(&(right.0, right.1.as_ref()))
        });
        let count = resident_order.len() as u64;
        let newest_existing = self
            .index
            .values()
            .map(|entry| entry.last_access)
            .max()
            .unwrap_or(0);
        let newest_projection = unix_now().max(newest_existing.saturating_add(count));
        let base = newest_projection.saturating_sub(count);
        for (rank, (_, content_hash)) in resident_order.into_iter().enumerate() {
            self.touch_metadata_entry(
                content_hash.as_ref(),
                base.saturating_add(rank as u64).saturating_add(1),
            );
        }
    }

    fn reconcile_persistent_artifacts(&mut self) {
        let functions_dir = self.cache_dir.join("functions");
        let mut resident_entries = self
            .index
            .iter()
            .filter_map(|(content_hash, entry)| {
                entry.data.as_ref().map(|data| {
                    (
                        Arc::clone(content_hash),
                        Arc::clone(data),
                        entry.memory_stamp,
                    )
                })
            })
            .collect::<Vec<_>>();
        resident_entries.sort_unstable_by(|left, right| {
            (left.2, left.0.as_ref()).cmp(&(right.2, right.0.as_ref()))
        });
        // Select bounded persistent survivors by global recency, never by hash
        // or directory iteration order. Persisted index timestamps were merged
        // under the namespace lock before this reconciliation; orphan files use
        // their durable modification time as the only available recency fact.
        let mut selected_lru = BTreeSet::<(u64, Arc<str>)>::new();
        let mut selected_recency = HashMap::<Arc<str>, u64>::new();
        if let Ok(entries) = std::fs::read_dir(&functions_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if name.starts_with('.') && name.contains(".tmp.") {
                    if let Err(error) = std::fs::remove_file(entry.path()) {
                        eprintln!(
                            "MOLT_CACHE: failed to retire stale temporary file {}: {error}",
                            entry.path().display()
                        );
                    }
                    continue;
                }
                let content_hash = name
                    .strip_suffix(".bin")
                    .or_else(|| name.strip_suffix(".poison"));
                let Some(content_hash) = content_hash else {
                    let bytes = entry.metadata().map_or(0, |metadata| metadata.len());
                    match std::fs::remove_file(entry.path()) {
                        Ok(()) => {
                            self.telemetry.untracked_files_retired =
                                self.telemetry.untracked_files_retired.saturating_add(1);
                            self.telemetry.untracked_bytes_retired =
                                self.telemetry.untracked_bytes_retired.saturating_add(bytes);
                        }
                        Err(_) => {
                            self.telemetry.limit_enforcement_failures =
                                self.telemetry.limit_enforcement_failures.saturating_add(1);
                        }
                    }
                    continue;
                };
                if !is_canonical_cache_hash(content_hash) {
                    let _ = std::fs::remove_file(entry.path());
                    continue;
                }
                let content_hash: Arc<str> = Arc::from(content_hash);
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |duration| {
                        duration.as_nanos().min(u128::from(u64::MAX)) as u64
                    });
                let recency = self
                    .index
                    .get(content_hash.as_ref())
                    .map_or(modified, |indexed| indexed.last_access.max(modified));
                if let Some(previous) = selected_recency.get_mut(content_hash.as_ref()) {
                    if recency > *previous {
                        selected_lru.remove(&(*previous, Arc::clone(&content_hash)));
                        *previous = recency;
                        selected_lru.insert((recency, Arc::clone(&content_hash)));
                    }
                    continue;
                }
                if !self.index.contains_key(content_hash.as_ref()) {
                    self.telemetry.orphan_artifacts_discovered =
                        self.telemetry.orphan_artifacts_discovered.saturating_add(1);
                }
                selected_recency.insert(Arc::clone(&content_hash), recency);
                selected_lru.insert((recency, Arc::clone(&content_hash)));
                if selected_lru.len() > self.max_entries
                    && let Some((_, victim)) = selected_lru.pop_first()
                {
                    selected_recency.remove(victim.as_ref());
                    for suffix in ["bin", "poison"] {
                        let _ =
                            std::fs::remove_file(functions_dir.join(format!("{victim}.{suffix}")));
                    }
                }
            }
        }
        let discovered_hashes = selected_lru.into_iter().collect::<Vec<_>>();
        self.persistent_bytes = 0;
        self.metadata_lru.clear();
        self.memory_bytes = 0;
        self.memory_lru.clear();
        let previous = std::mem::take(&mut self.index);
        for (discovered_recency, content_hash) in discovered_hashes {
            let Some(path) = self.artifact_path(&content_hash) else {
                continue;
            };
            let Some(poison_path) = self.poison_path(&content_hash) else {
                continue;
            };
            if poison_path.is_file()
                && let Err(error) = persist_poison_sidecar(
                    &functions_dir,
                    &path,
                    &poison_path,
                    &content_hash,
                    &[],
                    &[],
                    self.max_persistent_bytes,
                )
            {
                eprintln!(
                    "MOLT_CACHE: poison intent remains fail-closed for {content_hash} but could not be finalized: {error}"
                );
            }
            if matches!(
                read_poison_sidecar(&content_hash, &poison_path, self.max_persistent_bytes,),
                Ok(Some(_))
            ) {
                let _ = std::fs::remove_file(&path);
            }
            let persistent_bytes = persistent_entry_bytes(&path, &poison_path);
            if persistent_bytes == 0 {
                continue;
            }
            let previous_entry = previous.get(&content_hash);
            self.upsert_metadata_entry(
                &content_hash,
                previous_entry.map_or(discovered_recency, |entry| {
                    entry.last_access.max(discovered_recency)
                }),
                persistent_bytes,
                poison_path.is_file(),
            );
            self.enforce_cache_limits();
        }
        for (content_hash, data, memory_stamp) in resident_entries {
            if self
                .index
                .get(content_hash.as_ref())
                .is_some_and(|entry| entry.poisoned)
            {
                continue;
            }
            if !self.index.contains_key(content_hash.as_ref()) {
                self.upsert_metadata_entry(content_hash.as_ref(), unix_now(), 0, false);
            }
            self.restore_memory_data(content_hash.as_ref(), data, memory_stamp);
        }
        self.evict_memory();
        self.enforce_cache_limits();
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
            entry.data.as_ref()?;
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

    /// Rebuild resident-index storage without redefining logical recency.
    ///
    /// Persistent reconciliation reconstructs `index` and its heap nodes, but
    /// it is not an access. Restoring through `store_memory_data` would assign
    /// fresh stamps in `HashMap` iteration order and silently change the LRU
    /// victim. The caller supplies entries sorted by their preserved stamp.
    fn restore_memory_data(&mut self, content_hash: &str, bytes: Arc<[u8]>, memory_stamp: u64) {
        if self.max_memory_bytes == 0 || bytes.len() > self.max_memory_bytes {
            return;
        }
        let Some(entry) = self.index.get_mut(content_hash) else {
            return;
        };
        debug_assert!(entry.data.is_none());
        debug_assert!(entry.memory_lru_index.is_none());
        self.memory_clock = self.memory_clock.max(memory_stamp);
        entry.memory_stamp = memory_stamp;
        self.memory_bytes = self.memory_bytes.saturating_add(bytes.len());
        entry.data = Some(bytes);
        self.memory_lru_insert(content_hash);
    }

    fn evict_memory(&mut self) {
        while self.memory_bytes > self.max_memory_bytes {
            if self.memory_lru.is_empty() {
                break;
            }
            let content_hash = self.memory_lru_remove(0).content_hash;
            if let Some(entry) = self.index.get_mut(content_hash.as_ref())
                && let Some(bytes) = entry.data.take()
            {
                self.memory_bytes = self.memory_bytes.saturating_sub(bytes.len());
                entry.memory_stamp = 0;
            }
        }
    }

    fn memory_lru_insert(&mut self, content_hash: &str) {
        let entry = &self.index[content_hash];
        let stamp = entry.memory_stamp;
        let metadata_key = Arc::clone(&entry.metadata_key);
        let index = self.memory_lru.len();
        self.memory_lru.push(MemoryLruNode {
            stamp,
            content_hash: metadata_key,
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
            .get_mut(removed.content_hash.as_ref())
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
        (left.stamp, left.content_hash.as_ref()) < (right.stamp, right.content_hash.as_ref())
    }

    fn memory_lru_swap(&mut self, left: usize, right: usize) {
        self.memory_lru.swap(left, right);
        let left_hash = self.memory_lru[left].content_hash.as_ref();
        self.index.get_mut(left_hash).unwrap().memory_lru_index = Some(left);
        let right_hash = self.memory_lru[right].content_hash.as_ref();
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
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }
}

#[cfg(unix)]
fn positioned_read(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, bytes, offset)
}

#[cfg(windows)]
fn positioned_read(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, bytes, offset)
}

#[cfg(not(any(unix, windows)))]
fn positioned_read(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(bytes)
}

#[cfg(unix)]
fn positioned_write(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(file, bytes, offset)
}

#[cfg(windows)]
fn positioned_write(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(file, bytes, offset)
}

#[cfg(not(any(unix, windows)))]
fn positioned_write(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.write(bytes)
}

fn read_exact_positioned(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<()> {
    let mut read = 0;
    while read < bytes.len() {
        let count = positioned_read(file, &mut bytes[read..], offset + read as u64)?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "persistent cache namespace epoch is truncated",
            ));
        }
        read += count;
    }
    Ok(())
}

fn write_all_positioned(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        let count = positioned_write(file, &bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "persistent cache namespace epoch write made no progress",
            ));
        }
        written += count;
    }
    Ok(())
}

fn encode_namespace_epoch_word(epoch: u64) -> u64 {
    debug_assert!(epoch <= CACHE_NAMESPACE_EPOCH_VALUE_MASK);
    let inverse = (!epoch) & CACHE_NAMESPACE_EPOCH_VALUE_MASK;
    CACHE_NAMESPACE_EPOCH_TAG | epoch | (inverse << CACHE_NAMESPACE_EPOCH_VALUE_BITS)
}

fn decode_namespace_epoch_word(word: u64) -> Option<u64> {
    if word >> 62 != CACHE_NAMESPACE_EPOCH_TAG >> 62 {
        return None;
    }
    let epoch = word & CACHE_NAMESPACE_EPOCH_VALUE_MASK;
    let inverse = (word >> CACHE_NAMESPACE_EPOCH_VALUE_BITS) & CACHE_NAMESPACE_EPOCH_VALUE_MASK;
    (inverse == ((!epoch) & CACHE_NAMESPACE_EPOCH_VALUE_MASK)).then_some(epoch)
}

fn next_namespace_epoch(epoch: u64) -> Result<u64, String> {
    epoch
        .checked_add(1)
        .filter(|next| *next <= CACHE_NAMESPACE_EPOCH_VALUE_MASK)
        .ok_or_else(|| "persistent cache namespace epoch exhausted".to_string())
}

fn validate_namespace_epoch_slots(slots: [Option<u64>; 2]) -> Result<u64, String> {
    let [Some(left), Some(right)] = slots else {
        return Err("persistent cache namespace epoch has a torn durable slot".to_string());
    };
    if left.abs_diff(right) > 1 {
        return Err(format!(
            "persistent cache namespace epoch slots diverge: {left} vs {right}"
        ));
    }
    Ok(left.max(right))
}

fn read_namespace_epoch_slots(file: &File) -> Result<[Option<u64>; 2], String> {
    let mut bytes = [0; CACHE_NAMESPACE_EPOCH_BYTES];
    read_exact_positioned(file, &mut bytes, 0)
        .map_err(|error| format!("cannot read persistent cache namespace epoch: {error}"))?;
    Ok(std::array::from_fn(|slot_index| {
        let start =
            CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + slot_index * CACHE_NAMESPACE_EPOCH_SLOT_BYTES;
        decode_namespace_epoch_word(u64::from_le_bytes(
            bytes[start..start + CACHE_NAMESPACE_EPOCH_SLOT_BYTES]
                .try_into()
                .expect("fixed-width epoch slot"),
        ))
    }))
}

fn read_live_namespace_epoch_word(file: &File) -> Result<u64, String> {
    let mut bytes = [0; CACHE_NAMESPACE_EPOCH_SLOT_BYTES];
    read_exact_positioned(file, &mut bytes, CACHE_NAMESPACE_EPOCH_LIVE_OFFSET as u64)
        .map_err(|error| format!("cannot read persistent cache live namespace epoch: {error}"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn initialize_namespace_epoch(file: &File) -> Result<(), String> {
    let word = encode_namespace_epoch_word(0).to_le_bytes();
    let mut bytes = [0; CACHE_NAMESPACE_EPOCH_BYTES];
    bytes[CACHE_NAMESPACE_EPOCH_LIVE_OFFSET
        ..CACHE_NAMESPACE_EPOCH_LIVE_OFFSET + CACHE_NAMESPACE_EPOCH_SLOT_BYTES]
        .copy_from_slice(&word);
    for slot_index in 0..CACHE_NAMESPACE_EPOCH_SLOTS {
        let start =
            CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + slot_index * CACHE_NAMESPACE_EPOCH_SLOT_BYTES;
        bytes[start..start + CACHE_NAMESPACE_EPOCH_SLOT_BYTES].copy_from_slice(&word);
    }
    write_all_positioned(file, &bytes, 0)
        .and_then(|()| file.sync_data())
        .map_err(|error| format!("cannot initialize persistent cache namespace epoch: {error}"))
}

fn write_namespace_epoch(file: &File, epoch: u64) -> Result<(), String> {
    let slot_index = epoch as usize % CACHE_NAMESPACE_EPOCH_SLOTS;
    let durable_offset = (CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET
        + slot_index * CACHE_NAMESPACE_EPOCH_SLOT_BYTES) as u64;
    let word = encode_namespace_epoch_word(epoch).to_le_bytes();
    let result = if epoch & 1 != 0 {
        write_all_positioned(file, &word, CACHE_NAMESPACE_EPOCH_LIVE_OFFSET as u64)
            .and_then(|()| write_all_positioned(file, &word, durable_offset))
            .and_then(|()| file.sync_data())
    } else {
        write_all_positioned(file, &word, durable_offset)
            .and_then(|()| file.sync_data())
            .and_then(|()| {
                write_all_positioned(file, &word, CACHE_NAMESPACE_EPOCH_LIVE_OFFSET as u64)
            })
    };
    result.map_err(|error| format!("cannot persist cache namespace epoch {epoch}: {error}"))
}

#[cfg(all(any(unix, windows), target_has_atomic = "64"))]
fn mapped_namespace_epoch_word(map: &memmap2::MmapMut, offset: usize) -> &AtomicU64 {
    debug_assert_eq!(offset % CACHE_NAMESPACE_EPOCH_SLOT_BYTES, 0);
    debug_assert!(offset + CACHE_NAMESPACE_EPOCH_SLOT_BYTES <= CACHE_NAMESPACE_EPOCH_BYTES);
    // SAFETY: OS mappings are page-aligned, each slot is u64-aligned, the map
    // spans both slots, and all mapped access uses AtomicU64 for its lifetime.
    unsafe { &*(map.as_ptr().add(offset) as *const AtomicU64) }
}

#[cfg(all(any(unix, windows), target_has_atomic = "64"))]
fn read_mapped_namespace_epoch_slots(map: &memmap2::MmapMut) -> [Option<u64>; 2] {
    std::array::from_fn(|slot_index| {
        let offset =
            CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + slot_index * CACHE_NAMESPACE_EPOCH_SLOT_BYTES;
        decode_namespace_epoch_word(
            mapped_namespace_epoch_word(map, offset).load(Ordering::Acquire),
        )
    })
}

#[cfg(all(any(unix, windows), target_has_atomic = "64"))]
fn write_mapped_namespace_epoch(
    map: &memmap2::MmapMut,
    file: &File,
    epoch: u64,
) -> Result<(), String> {
    let slot_index = epoch as usize % CACHE_NAMESPACE_EPOCH_SLOTS;
    let durable_offset =
        CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + slot_index * CACHE_NAMESPACE_EPOCH_SLOT_BYTES;
    let word = encode_namespace_epoch_word(epoch);
    if epoch & 1 != 0 {
        mapped_namespace_epoch_word(map, CACHE_NAMESPACE_EPOCH_LIVE_OFFSET)
            .store(word, Ordering::Release);
    }
    mapped_namespace_epoch_word(map, durable_offset).store(word, Ordering::Release);
    let (flush_offset, flush_len) = if epoch & 1 != 0 {
        (
            CACHE_NAMESPACE_EPOCH_LIVE_OFFSET,
            durable_offset + CACHE_NAMESPACE_EPOCH_SLOT_BYTES,
        )
    } else {
        (durable_offset, CACHE_NAMESPACE_EPOCH_SLOT_BYTES)
    };
    map.flush_range(flush_offset, flush_len)
        .and_then(|()| file.sync_data())
        .map_err(|error| format!("cannot persist mapped cache namespace epoch {epoch}: {error}"))?;
    if epoch & 1 == 0 {
        mapped_namespace_epoch_word(map, CACHE_NAMESPACE_EPOCH_LIVE_OFFSET)
            .store(word, Ordering::Release);
    }
    Ok(())
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

fn persist_poison_sidecar(
    functions_dir: &std::path::Path,
    artifact_path: &std::path::Path,
    poison_path: &std::path::Path,
    content_hash: &str,
    left: &[u8],
    right: &[u8],
    max_persistent_bytes: u64,
) -> Result<u64, String> {
    let envelope = poison_envelope_for_limit(content_hash, left, right, max_persistent_bytes)?;
    match read_poison_sidecar(content_hash, poison_path, max_persistent_bytes) {
        Ok(Some(_)) => {
            if let Err(error) = std::fs::remove_file(artifact_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!(
                    "durable poison exists but ordinary artifact cannot be retired {}: {error}",
                    artifact_path.display()
                ));
            }
            return Ok(std::fs::metadata(poison_path).map_or(0, |metadata| metadata.len()));
        }
        Ok(None) => {}
        Err(_) if poison_path.is_file() => {
            // A crash after the durable filename transition can leave the old
            // artifact envelope or a partial poison envelope at `.poison`.
            // The filename is already fail-closed authority; finish it below.
        }
        Err(error) => return Err(error),
    }

    if !poison_path.is_file() && artifact_path.is_file() {
        match std::fs::hard_link(artifact_path, poison_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(link_error) => match std::fs::rename(artifact_path, poison_path) {
                Ok(()) => {}
                Err(rename_error) => {
                    return Err(format!(
                        "cannot install durable poison intent {} (hard-link: {link_error}; rename: {rename_error})",
                        poison_path.display()
                    ));
                }
            },
        }
        sync_cache_directory(functions_dir)?;
    } else if !poison_path.is_file() {
        let temp =
            write_unique_temp_file(functions_dir, &format!("{content_hash}.poison"), &envelope)
                .map_err(|error| format!("cannot write cache poison sidecar: {error}"))?;
        match std::fs::hard_link(&temp, poison_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                return Err(format!(
                    "cannot install cache poison sidecar {}: {error}",
                    poison_path.display()
                ));
            }
        }
        let _ = std::fs::remove_file(temp);
        sync_cache_directory(functions_dir)?;
    }

    if let Err(error) = std::fs::remove_file(artifact_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!(
            "durable poison intent installed but ordinary artifact cannot be retired {}: {error}",
            artifact_path.display()
        ));
    }
    sync_cache_directory(functions_dir)?;

    let mut poison_file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(poison_path)
        .map_err(|error| format!("cannot finalize cache poison sidecar: {error}"))?;
    poison_file
        .write_all(&envelope)
        .and_then(|()| poison_file.sync_all())
        .map_err(|error| format!("cannot sync cache poison sidecar: {error}"))?;
    sync_cache_directory(functions_dir)?;
    Ok(std::fs::metadata(poison_path).map_or(envelope.len() as u64, |metadata| metadata.len()))
}

fn sync_cache_directory(directory: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                format!(
                    "cannot sync cache namespace directory {}: {error}",
                    directory.display()
                )
            })?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory)
            .and_then(|handle| handle.sync_all())
            .map_err(|error| {
                format!(
                    "cannot flush cache namespace directory {}: {error}",
                    directory.display()
                )
            })?;
    }
    #[cfg(not(any(unix, windows)))]
    let _ = directory;
    Ok(())
}

fn install_authenticated_artifact(
    functions_dir: &std::path::Path,
    artifact_path: &std::path::Path,
    content_hash: &str,
    envelope: &[u8],
) -> Result<(), String> {
    let artifact_temp =
        write_unique_temp_file(functions_dir, &format!("{content_hash}.bin"), envelope)
            .map_err(|error| format!("cannot write authenticated cache artifact: {error}"))?;
    if let Err(error) = std::fs::hard_link(&artifact_temp, artifact_path) {
        let _ = std::fs::remove_file(&artifact_temp);
        return Err(format!(
            "cannot install authenticated cache artifact {}: {error}",
            artifact_path.display()
        ));
    }
    let _ = std::fs::remove_file(&artifact_temp);
    sync_cache_directory(functions_dir)?;
    Ok(())
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

fn persistent_entry_bytes(artifact_path: &std::path::Path, poison_path: &std::path::Path) -> u64 {
    [artifact_path, poison_path]
        .into_iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .filter(|metadata| metadata.is_file())
        .fold(0_u64, |total, metadata| {
            total.saturating_add(metadata.len())
        })
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !oversized {
                return Ok(None);
            }
            return Ok(Some(if oversized { Err(()) } else { Ok(line) }));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let body = &available[..newline.unwrap_or(available.len())];
        if !oversized {
            if line.len().saturating_add(body.len()) > max_bytes {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(body);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(if oversized { Err(()) } else { Ok(line) }));
        }
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

/// Return a cross-process-comparable, high-resolution Unix recency stamp.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
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

fn default_persistent_cache_max_entries() -> usize {
    env_cache_limit_bytes("MOLT_BACKEND_TIR_CACHE_MAX_ENTRIES")
        .unwrap_or(DEFAULT_PERSISTENT_CACHE_MAX_ENTRIES)
}

fn default_persistent_cache_max_bytes() -> u64 {
    if let Some(bytes) = env_cache_limit_u64("MOLT_BACKEND_TIR_CACHE_DISK_BYTES") {
        return bytes;
    }
    env_cache_limit_u64("MOLT_BACKEND_TIR_CACHE_DISK_MB")
        .and_then(|mib| mib.checked_mul(1024 * 1024))
        .unwrap_or(DEFAULT_PERSISTENT_CACHE_MAX_BYTES)
}

fn env_cache_limit_bytes(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
}

fn env_cache_limit_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
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
mod tests;
