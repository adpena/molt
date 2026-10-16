use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

// Use a unique temp directory per test run to avoid collisions.
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ThreadTrackingAllocator;

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static THREAD_ALLOCATION_COUNT: Cell<u64> = const { Cell::new(0) };
}

fn record_thread_allocation() {
    let _ = TRACK_ALLOCATIONS.try_with(|tracking| {
        if tracking.get() {
            let _ = THREAD_ALLOCATION_COUNT.try_with(|count| {
                count.set(count.get().saturating_add(1));
            });
        }
    });
}

unsafe impl GlobalAlloc for ThreadTrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_thread_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_thread_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_thread_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static TEST_ALLOCATOR: ThreadTrackingAllocator = ThreadTrackingAllocator;

fn count_thread_allocations<T>(run: impl FnOnce() -> T) -> (T, u64) {
    THREAD_ALLOCATION_COUNT.with(|count| count.set(0));
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(true));
    let result = run();
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));
    let allocations = THREAD_ALLOCATION_COUNT.with(Cell::get);
    (result, allocations)
}

fn tmp_cache_dir() -> PathBuf {
    let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    static TEST_RUN_NONCE: OnceLock<u128> = OnceLock::new();
    let run_nonce = TEST_RUN_NONCE.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test run must follow the Unix epoch")
            .as_nanos()
    });
    std::env::temp_dir().join(format!(
        "molt-cache-test-{}-{run_nonce}-{n}",
        std::process::id()
    ))
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
    "MOLT_BACKEND_TIR_CACHE_MEMORY_MB",
    "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEMORY_AVAILABLE_GB",
    "MOLT_MEMORY_AVAILABLE_GB",
    "MOLT_BACKEND_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEMORY_RESERVE_GB",
    "MOLT_MEMORY_RESERVE_GB",
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

    cache.put(&hash, artifact).unwrap();
    let result = cache.get(&hash);

    assert_eq!(result.as_deref(), Some(artifact.as_slice()));
}

#[test]
fn warm_memory_hits_share_storage_and_do_not_copy_artifact_bytes() {
    let mut cache = make_cache_with_memory_limit(2 * 1024 * 1024);
    let hash = fixture_hash("warm", b"large-artifact");
    let artifact = vec![0x5a; 1024 * 1024];
    cache.put(&hash, &artifact).unwrap();

    let first = cache.get(&hash).unwrap();
    let second = cache.get(&hash).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let lru_capacity = cache.memory_lru.capacity();
    let start = std::time::Instant::now();
    let (_, allocations) = count_thread_allocations(|| {
        for _ in 0..10_000 {
            let hit = std::hint::black_box(cache.get(&hash).unwrap());
            assert!(Arc::ptr_eq(&first, &hit));
        }
    });
    let elapsed = start.elapsed();
    eprintln!(
        "warm cache: 10000 shared 1MiB hits in {elapsed:?} ({:?}/hit), allocations={allocations}",
        elapsed / 10_000,
    );
    assert!(elapsed < std::time::Duration::from_secs(2));
    assert_eq!(
        allocations, 0,
        "resident cache hits must remain allocation-free"
    );
    assert_eq!(cache.memory_lru.len(), 1);
    assert_eq!(
        cache.memory_lru.capacity(),
        lru_capacity,
        "warm hits must mutate the indexed LRU node in place"
    );
}

#[test]
fn explicit_memory_cache_env_overrides_adaptive_default() {
    let _guard = env_lock().lock().expect("env lock");
    let _env = EnvRestore::apply(&[
        ("MOLT_BACKEND_TIR_CACHE_MEMORY_MB", "3"),
        ("MOLT_MEMORY_AVAILABLE_GB", "1"),
        ("MOLT_MEMORY_RESERVE_GB", "1"),
    ]);
    let limit = default_memory_cache_limit_bytes();
    assert_eq!(limit, 3 * 1024 * 1024);
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
        let _ = cache.put(&key, &[]);
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

#[test]
fn test_get_missing_artifact_file() {
    let dir = tmp_cache_dir();
    let mut cache = CompilationCache::open(dir);
    let hash = fixture_hash("missing", b"artifact");
    assert_eq!(cache.get(&hash), None);
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
                cache.put(&hash, artifact).unwrap();
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
fn external_poison_invalidates_an_already_resident_hit_and_equal_put() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("resident-poison", b"semantic-contract");
    let mut resident = CompilationCache::open(dir.clone());
    let mut divergent_writer = CompilationCache::open(dir.clone());
    resident.put(&hash, b"first").unwrap();
    assert_eq!(resident.get(&hash).as_deref(), Some(b"first".as_slice()));

    assert!(matches!(
        divergent_writer.put(&hash, b"second"),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
    assert!(resident.get(&hash).is_none());
    assert!(!resident.resident.contains_key(hash.as_str()));
    assert!(matches!(
        resident.put(&hash, b"first"),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
}

#[test]
fn corrupt_artifact_repairs_then_same_key_divergence_is_immutably_poisoned() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("same-key", b"semantic-contract");
    let functions = dir.join("functions");
    std::fs::create_dir_all(&functions).unwrap();
    let artifact_path = functions.join(format!("{hash}.bin"));
    std::fs::write(&artifact_path, b"truncated-poison").unwrap();

    let mut repair = CompilationCache::open(dir.clone());
    repair.put(&hash, b"rebuilt-artifact").unwrap();
    let repaired = std::fs::read(&artifact_path).unwrap();
    assert_eq!(
        decode_cache_envelope(&hash, &repaired).unwrap().1,
        b"rebuilt-artifact"
    );

    let barrier = Arc::new(std::sync::Barrier::new(3));
    std::thread::scope(|scope| {
        for artifact in [b"writer-a".as_slice(), b"writer-b-complete".as_slice()] {
            let dir = dir.clone();
            let hash = hash.clone();
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                let mut cache = CompilationCache::open(dir);
                barrier.wait();
                assert!(matches!(
                    cache.put(&hash, artifact),
                    Err(CompilationCacheWriteError::Integrity(_))
                ));
            });
        }
        barrier.wait();
    });
    let poison_path = functions.join(format!("{hash}.poison"));
    assert!(poison_path.is_file());
    assert!(matches!(
        decode_cache_envelope(&hash, &std::fs::read(&poison_path).unwrap()),
        Ok((CacheEnvelopeKind::Poison, _))
    ));
    let mut reopened = CompilationCache::open(dir);
    assert!(reopened.get(&hash).is_none());
    assert!(matches!(
        reopened.put(&hash, b"rebuilt-artifact"),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
}

#[test]
fn memory_only_oversized_same_key_divergence_persists_bounded_poison() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("memory-only", b"same-contract");
    let mut cache = CompilationCache::open_with_limits(dir.clone(), 4096, 8, 512);
    let first = vec![b'a'; 1024];
    let second = vec![b'b'; 1024];
    cache.put(&hash, &first).unwrap();
    assert!(!cache.artifact_path(&hash).unwrap().exists());
    assert!(matches!(
        cache.put(&hash, &second),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
    let poison_path = cache.poison_path(&hash).unwrap();
    assert!(std::fs::metadata(&poison_path).unwrap().len() <= 512);

    let mut reopened = CompilationCache::open_with_limits(dir, 4096, 8, 512);
    assert!(reopened.get(&hash).is_none());
    assert!(matches!(
        reopened.put(&hash, &first),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
}

#[test]
fn durable_poison_falls_back_to_minimal_envelope_and_retires_ordinary_artifact() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("minimal-poison", b"same-contract");
    let artifact = b"a";
    let artifact_envelope = encode_cache_envelope(&hash, CacheEnvelopeKind::Artifact, artifact);
    let detailed_poison = encode_cache_envelope(
        &hash,
        CacheEnvelopeKind::Poison,
        &nondeterminism_poison_payload(artifact, b"b"),
    );
    let max_persistent_bytes = artifact_envelope.len() as u64;
    assert!(detailed_poison.len() as u64 > max_persistent_bytes);

    let mut cache = CompilationCache::open_with_limits(dir.clone(), 4096, 8, max_persistent_bytes);
    cache.put(&hash, artifact).unwrap();
    let artifact_path = cache.artifact_path(&hash).unwrap();
    assert!(artifact_path.is_file());
    assert!(matches!(
        cache.put(&hash, b"b"),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
    assert!(!artifact_path.exists());
    let poison_path = cache.poison_path(&hash).unwrap();
    let poison = std::fs::read(&poison_path).unwrap();
    let (kind, payload) = decode_cache_envelope(&hash, &poison).unwrap();
    assert_eq!(kind, CacheEnvelopeKind::Poison);
    assert!(
        payload.is_empty(),
        "the bounded fallback marker is sufficient"
    );
    assert!(poison.len() as u64 <= max_persistent_bytes);

    let mut reopened = CompilationCache::open_with_limits(dir, 4096, 8, max_persistent_bytes);
    assert!(reopened.get(&hash).is_none());
    assert!(matches!(
        reopened.put(&hash, artifact),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
}

#[test]
fn impossible_durable_poison_fails_closed_and_drops_resident_payload() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("memory-only-zero-cap", b"same-contract");
    let mut cache = CompilationCache::open_with_limits(dir.clone(), 4096, 8, 0);
    cache.put(&hash, b"first").unwrap();
    assert!(cache.get(&hash).is_some());

    let error = cache
        .put(&hash, b"second")
        .expect_err("known divergence must fail closed even without durable capacity");
    assert!(matches!(error, CompilationCacheWriteError::Integrity(_)));
    assert!(cache.get(&hash).is_none());
    assert!(!cache.resident.contains_key(hash.as_str()));
    assert!(!cache.artifact_path(&hash).unwrap().exists());
    assert!(!cache.poison_path(&hash).unwrap().exists());
}

#[test]
fn authenticated_envelope_cannot_be_replayed_under_a_different_key() {
    let dir = tmp_cache_dir();
    let key_a = fixture_hash("key-a", b"body");
    let key_b = fixture_hash("key-b", b"body");
    let functions = dir.join("functions");
    std::fs::create_dir_all(&functions).unwrap();
    std::fs::write(
        functions.join(format!("{key_b}.bin")),
        encode_cache_envelope(&key_a, CacheEnvelopeKind::Artifact, b"artifact"),
    )
    .unwrap();
    let mut cache = CompilationCache::open(dir);
    assert!(cache.get(&key_b).is_none());
    assert!(cache.stats().telemetry.corruptions > 0);
}

#[test]
fn cache_directory_io_failure_is_typed_availability_not_integrity() {
    let cache_path = tmp_cache_dir();
    std::fs::write(&cache_path, b"not-a-directory").unwrap();
    let mut cache = CompilationCache::open(cache_path);
    let hash = fixture_hash("unavailable", b"body");
    assert!(matches!(
        cache.put(&hash, b"artifact"),
        Err(CompilationCacheWriteError::Unavailable(_))
    ));
}

#[test]
fn invalid_hashes_cannot_escape_the_canonical_artifact_directory() {
    let dir = tmp_cache_dir();
    let mut cache = CompilationCache::open(dir.clone());
    assert!(cache.get("../escape").is_none());
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = cache.put("../escape", b"artifact");
    }));
    assert!(rejected.is_err());
    assert!(!dir.join("escape.bin").exists());
}

fn set_mtime(path: &std::path::Path, time: SystemTime) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(time))
        .unwrap();
}

fn tracked_files(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut files = std::fs::read_dir(dir.join("functions"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    (name.ends_with(".bin") || name.ends_with(".poison"))
                        .then(|| (name, entry.metadata().unwrap().len()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    files.sort();
    files
}

#[test]
fn memory_eviction_keeps_artifacts_on_disk() {
    let dir = tmp_cache_dir();
    let mut cache = CompilationCache::open_with_memory_limit(dir, 8);
    let h1 = fixture_hash("fn_1", b"body 1");
    let h2 = fixture_hash("fn_2", b"body 2");
    let h3 = fixture_hash("fn_3", b"body 3");

    cache.put(&h1, b"1111").unwrap();
    cache.put(&h2, b"2222").unwrap();
    assert_eq!(cache.stats().resident_bytes, 8);
    cache.put(&h3, b"3333").unwrap();
    assert!(cache.stats().resident_bytes <= 8);
    assert!(
        !cache.resident.contains_key(h1.as_str()),
        "the least recently used payload leaves memory first"
    );

    let before = cache.stats().telemetry.disk_hits;
    assert_eq!(cache.get(&h1).as_deref(), Some(b"1111".as_slice()));
    assert_eq!(cache.stats().telemetry.disk_hits, before + 1);
    assert!(cache.stats().resident_bytes <= 8);
}

#[test]
fn memory_cache_does_not_retain_oversized_artifacts() {
    let mut cache = make_cache_with_memory_limit(4);
    let hash = fixture_hash("large_func", b"body");

    cache.put(&hash, b"artifact-too-large").unwrap();
    assert_eq!(cache.stats().resident_bytes, 0);
    assert_eq!(
        cache.get(&hash).as_deref(),
        Some(b"artifact-too-large".as_slice())
    );
    assert_eq!(
        cache.stats().resident_bytes,
        0,
        "oversized disk hits must not be retained in memory"
    );
}

#[test]
fn indexed_lru_scales_with_many_entries_and_bounded_residency() {
    const ENTRY_BYTES: usize = 1024;
    const RESIDENT_ENTRIES: usize = 128;
    const TOTAL_ENTRIES: usize = 20_000;

    let mut cache = make_cache_with_memory_limit(ENTRY_BYTES * RESIDENT_ENTRIES);
    let artifact = Arc::<[u8]>::from(vec![0x6b; ENTRY_BYTES]);
    let start = std::time::Instant::now();
    for index in 0..TOTAL_ENTRIES {
        cache.store_memory_data(&format!("{index:064x}"), Arc::clone(&artifact));
    }
    let elapsed = start.elapsed();

    assert_eq!(cache.resident.len(), RESIDENT_ENTRIES);
    assert_eq!(cache.memory_lru.len(), RESIDENT_ENTRIES);
    assert_eq!(cache.stats().resident_bytes, ENTRY_BYTES * RESIDENT_ENTRIES);
    for (heap_index, node) in cache.memory_lru.iter().enumerate() {
        assert_eq!(
            cache.resident[node.content_hash.as_ref()].memory_lru_index,
            heap_index
        );
        if heap_index > 0 {
            let parent = (heap_index - 1) / 2;
            assert!(!cache.memory_lru_less(heap_index, parent));
        }
    }
    // The newest entries survive: residency is least-recently-used.
    assert!(
        cache
            .resident
            .contains_key(format!("{:064x}", TOTAL_ENTRIES - 1).as_str())
    );
    assert!(!cache.resident.contains_key(format!("{:064x}", 0).as_str()));
    eprintln!("indexed LRU: {TOTAL_ENTRIES} insertions, {RESIDENT_ENTRIES} resident, {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(5));
}

#[test]
fn artifacts_survive_reopening() {
    let dir = tmp_cache_dir();
    let h1 = fixture_hash("fn_a", b"body a");
    let h2 = fixture_hash("fn_b", b"body b");
    {
        let mut cache = CompilationCache::open(dir.clone());
        cache.put(&h1, b"artifact a").unwrap();
        cache.put(&h2, b"artifact b").unwrap();
    }
    let mut reopened = CompilationCache::open(dir);
    assert_eq!(reopened.get(&h1).as_deref(), Some(b"artifact a".as_slice()));
    assert_eq!(reopened.get(&h2).as_deref(), Some(b"artifact b".as_slice()));
    assert_eq!(reopened.stats().telemetry.disk_hits, 2);
}

#[test]
fn opening_a_cache_performs_no_io() {
    let dir = tmp_cache_dir();
    let mut cache = CompilationCache::open(dir.clone());
    assert!(
        !dir.exists(),
        "open must not create, lock or scan the namespace"
    );
    assert!(cache.get(&fixture_hash("absent", b"body")).is_none());
    assert!(!dir.exists(), "a miss must not create the namespace");
}

#[test]
fn concurrent_identical_writers_publish_once_and_reuse() {
    const WRITERS: usize = 8;
    let dir = tmp_cache_dir();
    let hash = fixture_hash("shared", b"body");
    let barrier = Arc::new(std::sync::Barrier::new(WRITERS));
    let telemetry = std::thread::scope(|scope| {
        let handles = (0..WRITERS)
            .map(|_| {
                let dir = dir.clone();
                let hash = hash.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    let mut cache = CompilationCache::open(dir);
                    barrier.wait();
                    cache.put(&hash, b"same payload").unwrap();
                    cache.stats().telemetry
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(telemetry.iter().map(|t| t.writes).sum::<u64>(), 1);
    assert_eq!(
        telemetry.iter().map(|t| t.reuses).sum::<u64>(),
        WRITERS as u64 - 1
    );
    assert_eq!(telemetry.iter().map(|t| t.corruptions).sum::<u64>(), 0);
    assert_eq!(tracked_files(&dir).len(), 1);
    let mut reader = CompilationCache::open(dir);
    assert_eq!(
        reader.get(&hash).as_deref(),
        Some(b"same payload".as_slice())
    );
}

#[test]
fn a_held_trim_lock_never_blocks_reads_writes_or_other_trims() {
    let dir = tmp_cache_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let held = File::create(dir.join(CACHE_TRIM_LOCK_FILE)).unwrap();
    held.lock().unwrap();

    let hash = fixture_hash("unlocked", b"body");
    let mut cache = CompilationCache::open(dir.clone());
    cache.put(&hash, b"artifact").unwrap();
    let mut reader = CompilationCache::open(dir.clone());
    assert_eq!(reader.get(&hash).as_deref(), Some(b"artifact".as_slice()));
    cache.trim().unwrap();
    assert_eq!(
        cache.stats().telemetry.trims,
        0,
        "a busy trim lock means another process is trimming"
    );
    assert!(!dir.join(CACHE_TRIM_STAMP_FILE).exists());

    held.unlock().unwrap();
    cache.trim().unwrap();
    assert_eq!(cache.stats().telemetry.trims, 1);
}

#[test]
fn trimming_is_amortized_by_its_stamp() {
    let dir = tmp_cache_dir();
    let mut cache = CompilationCache::open(dir.clone());
    cache
        .put(&fixture_hash("stamp", b"body"), b"artifact")
        .unwrap();
    cache.trim_if_due().unwrap();
    assert_eq!(cache.stats().telemetry.trims, 1);
    cache.trim_if_due().unwrap();
    assert_eq!(
        cache.stats().telemetry.trims,
        1,
        "a fresh stamp defers trims"
    );

    let stamp = dir.join(CACHE_TRIM_STAMP_FILE);
    set_mtime(&stamp, SystemTime::now() - CACHE_TRIM_INTERVAL);
    cache.trim_if_due().unwrap();
    assert_eq!(
        cache.stats().telemetry.trims,
        2,
        "an expired stamp trims again"
    );

    set_mtime(&stamp, SystemTime::now() + 2 * CACHE_TRIM_INTERVAL);
    cache.trim_if_due().unwrap();
    assert_eq!(
        cache.stats().telemetry.trims,
        3,
        "a far-future stamp from a skewed clock cannot disable trimming"
    );
}

#[test]
fn trim_bounds_concurrent_writers_to_entry_and_byte_limits() {
    let dir = tmp_cache_dir();
    let sample_hash = fixture_hash("sample", b"body");
    let envelope_bytes =
        encode_cache_envelope(&sample_hash, CacheEnvelopeKind::Artifact, b"row-00").len() as u64;
    let max_entries = 3;
    let max_bytes = envelope_bytes * 2;
    std::thread::scope(|scope| {
        for index in 0..12 {
            let dir = dir.clone();
            scope.spawn(move || {
                let hash = fixture_hash(&format!("writer-{index}"), b"body");
                let mut cache =
                    CompilationCache::open_with_limits(dir, 4096, max_entries, max_bytes);
                cache
                    .put(&hash, format!("row-{index:02}").as_bytes())
                    .unwrap();
            });
        }
    });
    assert_eq!(
        tracked_files(&dir).len(),
        12,
        "writes are not bounded inline"
    );

    let mut trimmer = CompilationCache::open_with_limits(dir.clone(), 4096, max_entries, max_bytes);
    trimmer.trim().unwrap();
    let files = tracked_files(&dir);
    assert!(files.len() <= max_entries);
    assert!(files.iter().map(|(_, bytes)| bytes).sum::<u64>() <= max_bytes);
    assert_eq!(
        files.len(),
        2,
        "the byte limit binds before the entry limit"
    );
    assert_eq!(trimmer.stats().telemetry.evicted_entries, 10);
}

#[test]
fn trim_evicts_least_recently_used_files_and_hits_refresh_recency() {
    let dir = tmp_cache_dir();
    let cold = fixture_hash("cold", b"body");
    let hot = fixture_hash("hot", b"body");
    let newcomer = fixture_hash("new", b"body");
    let mut writer = CompilationCache::open_with_limits(dir.clone(), 4096, 2, 1 << 20);
    writer.put(&cold, b"c").unwrap();
    writer.put(&hot, b"h").unwrap();
    writer.put(&newcomer, b"n").unwrap();
    let now = SystemTime::now();
    let hour = std::time::Duration::from_secs(3600);
    set_mtime(&writer.artifact_path(&cold).unwrap(), now - 2 * hour);
    set_mtime(&writer.artifact_path(&hot).unwrap(), now - 3 * hour);

    // A fresh process hits `hot`; its stale recency is refreshed on use.
    let mut reader = CompilationCache::open(dir.clone());
    assert!(reader.get(&hot).is_some());
    writer.trim().unwrap();

    assert!(!writer.artifact_path(&cold).unwrap().exists());
    assert!(writer.artifact_path(&hot).unwrap().exists());
    assert!(writer.artifact_path(&newcomer).unwrap().exists());
    assert_eq!(writer.stats().telemetry.evicted_entries, 1);
}

#[test]
fn trim_retires_stale_temporaries_and_untracked_files_only() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("tracked", b"body");
    let mut cache = CompilationCache::open(dir.clone());
    cache.put(&hash, b"artifact").unwrap();
    let functions = dir.join("functions");
    let stale_temp = functions.join(format!(".{hash}.bin.tmp.1.1"));
    let live_temp = functions.join(format!(".{hash}.bin.tmp.1.2"));
    let untracked = functions.join("index.txt");
    for path in [&stale_temp, &live_temp, &untracked] {
        std::fs::write(path, b"x").unwrap();
    }
    set_mtime(
        &stale_temp,
        SystemTime::now() - CACHE_RECENCY_INTERVAL - std::time::Duration::from_secs(1),
    );

    cache.trim().unwrap();
    assert!(!stale_temp.exists());
    assert!(live_temp.exists(), "a live writer's staging file survives");
    assert!(!untracked.exists());
    assert!(cache.artifact_path(&hash).unwrap().exists());
    let telemetry = cache.stats().telemetry;
    assert_eq!(telemetry.temp_files_retired, 1);
    assert_eq!(telemetry.untracked_files_retired, 1);
    assert_eq!(telemetry.evicted_entries, 0);
}

#[test]
fn trim_retires_only_stale_sibling_backend_namespaces() {
    let root = tmp_cache_dir();
    let name = |seed: &str| fixture_hash(seed, b"namespace");
    let own = root.join(name("own"));
    let stale = root.join(name("stale"));
    let active = root.join(name("active"));
    let not_a_namespace = root.join(name("other-cache"));
    let foreign = root.join("python_source_closure");
    for namespace in [&stale, &active, &foreign] {
        std::fs::create_dir_all(namespace.join("functions")).unwrap();
    }
    std::fs::create_dir_all(&not_a_namespace).unwrap();
    let artifact_file = root.join(format!("{}.artifact-x.a", name("archive")));
    std::fs::write(&artifact_file, b"archive").unwrap();
    // A namespace that a live backend trimmed recently.
    std::fs::write(active.join(CACHE_TRIM_STAMP_FILE), b"").unwrap();
    let later = SystemTime::now() + CACHE_NAMESPACE_UNUSED_LIMIT;
    set_mtime(&active.join(CACHE_TRIM_STAMP_FILE), later);

    let mut cache = CompilationCache::open(own.clone());
    cache.namespace_root = Some(root.clone());
    cache.put(&name("artifact"), b"artifact").unwrap();
    cache.trim_at(later, false).unwrap();

    assert!(!stale.exists(), "an unused backend namespace is retired");
    assert!(active.exists());
    assert!(own.exists(), "a trim never retires its own namespace");
    assert!(
        not_a_namespace.exists(),
        "only namespaces with functions/ qualify"
    );
    assert!(foreign.exists(), "non-hash names belong to other caches");
    assert!(artifact_file.exists());
    assert_eq!(cache.stats().telemetry.stale_namespaces_retired, 1);
}

#[test]
fn explicitly_placed_caches_never_sweep_their_parent() {
    let root = tmp_cache_dir();
    let sibling = root.join(fixture_hash("sibling", b"namespace"));
    std::fs::create_dir_all(sibling.join("functions")).unwrap();
    let mut cache = CompilationCache::open(root.join(fixture_hash("own", b"namespace")));
    cache
        .trim_at(SystemTime::now() + 2 * CACHE_NAMESPACE_UNUSED_LIMIT, false)
        .unwrap();
    assert!(sibling.exists());
}

#[test]
fn torn_artifacts_read_as_misses_and_are_retired() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("torn", b"body");
    let mut writer = CompilationCache::open(dir.clone());
    writer.put(&hash, b"complete artifact").unwrap();
    let path = writer.artifact_path(&hash).unwrap();
    let envelope = std::fs::read(&path).unwrap();
    std::fs::write(&path, &envelope[..envelope.len() - 1]).unwrap();

    let mut reader = CompilationCache::open(dir);
    assert!(reader.get(&hash).is_none());
    assert_eq!(reader.stats().telemetry.corruptions, 1);
    assert!(!path.exists());
    reader.put(&hash, b"complete artifact").unwrap();
    assert_eq!(reader.stats().telemetry.writes, 1);
}

#[test]
fn bounded_read_rejects_an_oversized_file_before_decode() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("oversized", b"artifact");
    let max_bytes = 512;
    let mut cache = CompilationCache::open_with_limits(dir.clone(), 4096, 8, max_bytes);
    let functions = dir.join("functions");
    std::fs::create_dir_all(&functions).unwrap();
    let artifact = functions.join(format!("{hash}.bin"));
    File::create(&artifact)
        .unwrap()
        .set_len(max_bytes + 1)
        .unwrap();

    assert!(cache.get(&hash).is_none());
    assert_eq!(cache.stats().telemetry.corruptions, 1);
    assert!(!artifact.exists());
    cache.put(&hash, b"repaired").unwrap();
    assert_eq!(cache.get(&hash).as_deref(), Some(b"repaired".as_slice()));
}
