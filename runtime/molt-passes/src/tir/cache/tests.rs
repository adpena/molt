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
fn indexed_lru_scales_with_many_metadata_entries_and_bounded_residency() {
    const ENTRY_BYTES: usize = 1024;
    const RESIDENT_ENTRIES: usize = 128;
    const TOTAL_ENTRIES: usize = 20_000;

    let mut cache = make_cache_with_memory_limit(ENTRY_BYTES * RESIDENT_ENTRIES);
    let artifact = Arc::<[u8]>::from(vec![0x6b; ENTRY_BYTES]);
    let start = std::time::Instant::now();
    for index in 0..TOTAL_ENTRIES {
        let hash = format!("{index:064x}");
        let poison_path = cache.poison_path(&hash).unwrap();
        cache.index.insert(
            Arc::from(hash.as_str()),
            CacheEntry {
                metadata_key: Arc::from(hash.as_str()),
                poison_path,
                last_access: 0,
                data: None,
                memory_stamp: 0,
                memory_lru_index: None,
                persistent_bytes: 0,
                poisoned: false,
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
            cache.index[node.content_hash.as_ref()].memory_lru_index,
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

    cache.put(&h1, b"1111").unwrap();
    cache.put(&h2, b"2222").unwrap();
    assert_eq!(cache.memory_bytes(), 8);

    cache.put(&h3, b"3333").unwrap();
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
            .get(h1.as_str())
            .is_some_and(|entry| entry.data.is_none()),
        "least-recently-used artifact bytes should be evicted first: h1={:?}, h2={:?}, h3={:?}, lru={:?}",
        cache.index.get(h1.as_str()),
        cache.index.get(h2.as_str()),
        cache.index.get(h3.as_str()),
        cache.memory_lru,
    );

    assert_eq!(cache.get(&h1).as_deref(), Some(b"1111".as_slice()));
    assert_eq!(cache.len(), 3);
    assert!(cache.memory_bytes() <= 8);
}

#[test]
fn memory_cache_does_not_retain_oversized_artifacts() {
    let mut cache = make_cache_with_memory_limit(4);
    let hash = fixture_hash("large_func", b"body");

    cache.put(&hash, b"artifact-too-large").unwrap();

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.memory_bytes(), 0);
    assert!(
        cache
            .index
            .get(hash.as_str())
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

/// 6. save_index + load_index round-trip
#[test]
fn test_disk_roundtrip() {
    let dir = tmp_cache_dir();

    // Write entries to disk.
    {
        let mut cache = CompilationCache::open(dir.clone());
        let h1 = fixture_hash("fn_a", b"body a");
        let h2 = fixture_hash("fn_b", b"body b");
        cache.put(&h1, b"artifact a").unwrap();
        cache.put(&h2, b"artifact b").unwrap();
        cache.save_index().unwrap();
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
    std::fs::write(
        functions.join(format!("{hash}.bin")),
        encode_cache_envelope(&hash, CacheEnvelopeKind::Artifact, b"artifact"),
    )
    .unwrap();

    let mut cache = CompilationCache::open(dir);
    assert_eq!(cache.len(), 1);
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
    writer_a.put(&hash, b"artifact").unwrap();

    let mut writer_b = CompilationCache::open(dir.clone());
    writer_b.save_index().unwrap();

    let mut reader = CompilationCache::open(dir);
    assert_eq!(reader.len(), 1);
    assert_eq!(reader.get(&hash).as_deref(), Some(b"artifact".as_slice()));
}

#[test]
fn stale_writer_merges_global_recency_before_selecting_capacity_survivors() {
    let dir = tmp_cache_dir();
    let h1 = fixture_hash("global-hot", b"body");
    let h2 = fixture_hash("global-cold", b"body");
    let h3 = fixture_hash("global-new", b"body");
    let mut seed = CompilationCache::open_with_limits(dir.clone(), 1024, 2, 1024 * 1024);
    seed.put(&h1, b"hot").unwrap();
    seed.put(&h2, b"cold").unwrap();
    seed.save_index().unwrap();

    let mut hot_writer = CompilationCache::open_with_limits(dir.clone(), 1024, 2, 1024 * 1024);
    let mut stale_writer = CompilationCache::open_with_limits(dir.clone(), 1024, 2, 1024 * 1024);
    assert_eq!(hot_writer.get(&h1).as_deref(), Some(b"hot".as_slice()));
    hot_writer.save_index().unwrap();
    stale_writer.touch_metadata_entry(&h2, 1);
    stale_writer.put(&h3, b"new").unwrap();
    stale_writer.save_index().unwrap();

    let mut recovered = CompilationCache::open_with_limits(dir, 1024, 2, 1024 * 1024);
    assert_eq!(recovered.get(&h1).as_deref(), Some(b"hot".as_slice()));
    assert_eq!(recovered.get(&h3).as_deref(), Some(b"new".as_slice()));
    assert!(recovered.get(&h2).is_none());
}

#[test]
fn resident_hit_order_controls_persistent_capacity_victims() {
    let dir = tmp_cache_dir();
    let h1 = fixture_hash("resident-hot", b"body");
    let h2 = fixture_hash("resident-cold", b"body");
    let h3 = fixture_hash("resident-new", b"body");
    let mut cache = CompilationCache::open_with_limits(dir, 1024, 2, 1024 * 1024);
    cache.put(&h1, b"hot").unwrap();
    cache.put(&h2, b"cold").unwrap();
    assert_eq!(cache.get(&h1).as_deref(), Some(b"hot".as_slice()));

    cache.put(&h3, b"new").unwrap();

    assert_eq!(cache.get(&h1).as_deref(), Some(b"hot".as_slice()));
    assert_eq!(cache.get(&h3).as_deref(), Some(b"new".as_slice()));
    assert!(cache.get(&h2).is_none());
}

#[test]
fn observing_a_committed_epoch_never_mutates_the_quiescent_namespace() {
    let dir = tmp_cache_dir();
    let h1 = fixture_hash("read-only-reconcile-a", b"body");
    let h2 = fixture_hash("read-only-reconcile-b", b"body");
    let mut reader = CompilationCache::open_with_limits(dir.clone(), 1024, 1, 1024 * 1024);
    let mut writer = CompilationCache::open_with_limits(dir, 1024, 2, 1024 * 1024);
    writer.put(&h1, b"one").unwrap();
    writer.put(&h2, b"two").unwrap();
    writer.save_index().unwrap();

    // Reconciling another process's completed generation is a read path,
    // not a capacity-enforcement transaction. In particular, this reader's
    // smaller local bound must not silently delete the writer's second
    // canonical artifact while the namespace epoch is even.
    assert_eq!(reader.get(&h1).as_deref(), Some(b"one".as_slice()));
    assert!(reader.artifact_path(&h1).unwrap().is_file());
    assert!(reader.artifact_path(&h2).unwrap().is_file());
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
                cache.save_index().unwrap();
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
    assert!(resident.index[hash.as_str()].data.is_none());
    assert!(matches!(
        resident.put(&hash, b"first"),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
}

#[test]
fn poison_pruning_advances_epoch_and_cannot_resurrect_remote_resident_bytes() {
    let dir = tmp_cache_dir();
    let h1 = fixture_hash("poison-prune-h1", b"semantic-contract");
    let h2 = fixture_hash("poison-prune-h2", b"semantic-contract");
    let mut resident = CompilationCache::open_with_limits(dir.clone(), 1024, 1, 1024 * 1024);
    let mut writer = CompilationCache::open_with_limits(dir, 1024, 1, 1024 * 1024);

    resident.put(&h1, b"first").unwrap();
    assert_eq!(resident.get(&h1).as_deref(), Some(b"first".as_slice()));
    assert!(matches!(
        writer.put(&h1, b"second"),
        Err(CompilationCacheWriteError::Integrity(_))
    ));

    // Capacity retirement removes both the poisoned artifact and its
    // tombstone in a later semantic transaction. A process retaining the
    // pre-poison bytes must invalidate them on that later generation; the
    // absence of a sidecar is never evidence that the old bytes are sound.
    writer.put(&h2, b"replacement").unwrap();
    assert!(!writer.poison_path(&h1).unwrap().exists());
    assert!(resident.get(&h1).is_none());
    assert!(!resident.index.contains_key(h1.as_str()));
}

#[test]
fn resident_seqlock_rejects_poison_transition_between_epoch_samples() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("resident-seqlock", b"semantic-contract");
    let mut resident = CompilationCache::open(dir.clone());
    let mut divergent_writer = CompilationCache::open(dir);
    resident.put(&hash, b"first").unwrap();
    assert_eq!(resident.get(&hash).as_deref(), Some(b"first".as_slice()));

    let reader_phases = Arc::new(std::sync::Barrier::new(2));
    let writer_phases = Arc::new(std::sync::Barrier::new(2));
    resident.resident_epoch_barrier = Some(Arc::clone(&reader_phases));
    divergent_writer.poison_epoch_barrier = Some(Arc::clone(&writer_phases));

    std::thread::scope(|scope| {
        let reader_hash = hash.clone();
        let reader = scope.spawn(move || {
            let result = resident.get(&reader_hash);
            (resident, result)
        });
        reader_phases.wait();

        let writer_hash = hash.clone();
        let writer = scope.spawn(move || {
            let result = divergent_writer.put(&writer_hash, b"second");
            (divergent_writer, result)
        });
        writer_phases.wait();

        // Publish the writer's odd epoch between the reader's two samples,
        // then wait until the reader has observed it before completion.
        reader_phases.wait();
        reader_phases.wait();
        writer_phases.wait();
        reader_phases.wait();

        let (_, write_result) = writer.join().unwrap();
        assert!(matches!(
            write_result,
            Err(CompilationCacheWriteError::Integrity(_))
        ));
        let (resident, read_result) = reader.join().unwrap();
        assert!(read_result.is_none());
        assert!(resident.index[hash.as_str()].data.is_none());
    });
}

#[test]
fn interrupted_epoch_without_poison_retires_the_entire_namespace() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("epoch-before-poison", b"semantic-contract");
    let mut resident = CompilationCache::open(dir.clone());
    resident.put(&hash, b"first").unwrap();
    assert_eq!(resident.get(&hash).as_deref(), Some(b"first".as_slice()));

    let mut crashed_writer = CompilationCache::open(dir);
    let transition = crashed_writer
        .with_namespace_lock(true, |cache| cache.begin_namespace_mutation_locked())
        .unwrap()
        .unwrap();
    assert_eq!(transition & 1, 1);
    drop(crashed_writer);

    assert!(resident.get(&hash).is_none());
    assert!(!resident.index.contains_key(hash.as_str()));
    assert!(!resident.artifact_path(&hash).unwrap().exists());
    assert!(!resident.poison_path(&hash).unwrap().exists());
    let live = resident.read_namespace_epoch().unwrap();
    assert_eq!(live & 1, 0);
    assert_eq!(resident.read_durable_namespace_epoch().unwrap(), live);
}

#[test]
fn interrupted_epoch_enumeration_failure_stays_odd_and_serves_nothing() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("epoch-enumeration-failure", b"semantic-contract");
    let mut cache = CompilationCache::open(dir.clone());
    cache.put(&hash, b"artifact").unwrap();
    assert_eq!(cache.get(&hash).as_deref(), Some(b"artifact".as_slice()));

    let mut crashed_writer = CompilationCache::open(dir);
    let transition = crashed_writer
        .with_namespace_lock(true, |writer| writer.begin_namespace_mutation_locked())
        .unwrap()
        .unwrap();
    drop(crashed_writer);

    cache.namespace_reset_enumeration_error = true;
    assert!(
        cache.get(&hash).is_none(),
        "resident bytes must fail closed"
    );
    cache.discard_resident_data();
    assert!(cache.get(&hash).is_none(), "disk bytes must fail closed");
    assert_eq!(cache.read_namespace_epoch().unwrap(), transition);
    assert_eq!(cache.read_durable_namespace_epoch().unwrap(), transition);
    assert!(cache.artifact_path(&hash).unwrap().is_file());
}

#[test]
fn nonresident_read_with_unrecoverable_epoch_fails_closed() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("nonresident-torn-epoch", b"semantic-contract");
    let mut writer = CompilationCache::open(dir.clone());
    writer.put(&hash, b"artifact").unwrap();
    let mut reader = CompilationCache::open_with_memory_limit(dir, 0);
    assert!(reader.index[hash.as_str()].data.is_none());

    let invalid = [0_u8; CACHE_NAMESPACE_EPOCH_BYTES];
    write_all_positioned(reader.namespace_epoch.as_ref().unwrap(), &invalid, 0).unwrap();
    reader
        .namespace_epoch
        .as_ref()
        .unwrap()
        .sync_data()
        .unwrap();
    #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
    for offset in [
        CACHE_NAMESPACE_EPOCH_LIVE_OFFSET,
        CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET,
        CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + CACHE_NAMESPACE_EPOCH_SLOT_BYTES,
    ] {
        mapped_namespace_epoch_word(reader.namespace_epoch_map.as_ref().unwrap(), offset)
            .store(0, Ordering::Release);
    }

    assert!(reader.get(&hash).is_none());
    assert!(reader.get(&hash).is_none());
}

#[test]
fn torn_durable_odd_slot_forces_locked_namespace_retirement() {
    assert!(validate_namespace_epoch_slots([Some(0), None]).is_err());

    let dir = tmp_cache_dir();
    let hash = fixture_hash("torn-odd-slot", b"semantic-contract");
    let mut resident = CompilationCache::open(dir.clone());
    resident.put(&hash, b"first").unwrap();
    assert_eq!(resident.get(&hash).as_deref(), Some(b"first".as_slice()));

    let transition = encode_namespace_epoch_word(1);
    #[cfg(all(any(unix, windows), target_has_atomic = "64"))]
    mapped_namespace_epoch_word(resident.namespace_epoch_map.as_ref().unwrap(), 0)
        .store(transition, Ordering::Release);
    #[cfg(not(all(any(unix, windows), target_has_atomic = "64")))]
    write_all_positioned(
        resident.namespace_epoch.as_ref().unwrap(),
        &transition.to_le_bytes(),
        CACHE_NAMESPACE_EPOCH_LIVE_OFFSET as u64,
    )
    .unwrap();
    let transition_bytes = transition.to_le_bytes();
    write_all_positioned(
        resident.namespace_epoch.as_ref().unwrap(),
        &transition_bytes[..3],
        (CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + CACHE_NAMESPACE_EPOCH_SLOT_BYTES) as u64,
    )
    .unwrap();
    resident
        .namespace_epoch
        .as_ref()
        .unwrap()
        .sync_data()
        .unwrap();
    assert!(resident.read_durable_namespace_epoch().is_err());

    let artifact = resident.artifact_path(&hash).unwrap();
    let poison = resident.poison_path(&hash).unwrap();
    std::fs::hard_link(&artifact, &poison).unwrap();

    assert!(resident.get(&hash).is_none());
    assert!(!artifact.exists());
    assert!(!poison.exists());
    let live = resident.read_namespace_epoch().unwrap();
    assert_eq!(live & 1, 0);
    assert_eq!(resident.read_durable_namespace_epoch().unwrap(), live);
}

#[test]
fn fresh_open_retires_namespace_after_torn_epoch_before_serving_cache() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("fresh-open-torn-epoch", b"semantic-contract");
    let mut crashed_writer = CompilationCache::open(dir.clone());
    crashed_writer.put(&hash, b"first").unwrap();
    let artifact = crashed_writer.artifact_path(&hash).unwrap();
    let poison = crashed_writer.poison_path(&hash).unwrap();
    std::fs::hard_link(&artifact, &poison).unwrap();

    let epoch_file = crashed_writer.namespace_epoch.as_ref().unwrap();
    let transition = encode_namespace_epoch_word(1).to_le_bytes();
    write_all_positioned(
        epoch_file,
        &transition,
        CACHE_NAMESPACE_EPOCH_LIVE_OFFSET as u64,
    )
    .unwrap();
    write_all_positioned(
        epoch_file,
        &transition[..3],
        (CACHE_NAMESPACE_EPOCH_DURABLE_OFFSET + CACHE_NAMESPACE_EPOCH_SLOT_BYTES) as u64,
    )
    .unwrap();
    epoch_file.sync_data().unwrap();
    drop(crashed_writer);

    let mut recovered = CompilationCache::open(dir);
    assert!(
        recovered.namespace_epoch_error.is_none(),
        "fresh open must route torn durable slots through locked recovery"
    );
    assert!(recovered.get(&hash).is_none());
    assert!(!artifact.exists());
    assert!(!poison.exists());
    let live = recovered.read_namespace_epoch().unwrap();
    assert_eq!(live & 1, 0);
    assert_eq!(recovered.read_durable_namespace_epoch().unwrap(), live);
}

#[test]
fn crash_point_poison_intents_are_fail_closed_and_finalized_on_open() {
    for crash_point in ["linked-intent", "renamed-intent", "partial-finalize"] {
        let dir = tmp_cache_dir();
        let hash = fixture_hash(crash_point, b"semantic-contract");
        let mut writer = CompilationCache::open(dir.clone());
        writer.put(&hash, b"first").unwrap();
        drop(writer);
        let functions = dir.join("functions");
        let artifact = functions.join(format!("{hash}.bin"));
        let poison = functions.join(format!("{hash}.poison"));
        match crash_point {
            "linked-intent" => std::fs::hard_link(&artifact, &poison).unwrap(),
            "renamed-intent" => std::fs::rename(&artifact, &poison).unwrap(),
            "partial-finalize" => {
                std::fs::rename(&artifact, &poison).unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&poison)
                    .unwrap()
                    .write_all(b"partial")
                    .unwrap();
            }
            _ => unreachable!(),
        }

        let mut recovered = CompilationCache::open(dir);
        assert!(recovered.get(&hash).is_none(), "{crash_point}");
        assert!(!artifact.exists(), "{crash_point}");
        let poison_bytes = std::fs::read(&poison).unwrap();
        assert!(matches!(
            decode_cache_envelope(&hash, &poison_bytes),
            Ok((CacheEnvelopeKind::Poison, _))
        ));
    }
}

#[test]
fn bounded_read_rejects_an_oversized_file_before_allocation_or_decode() {
    let dir = tmp_cache_dir();
    let hash = fixture_hash("oversized", b"artifact");
    let max_bytes = 512;
    let mut cache = CompilationCache::open_with_limits(dir.clone(), 4096, 8, max_bytes);
    let functions = dir.join("functions");
    std::fs::create_dir_all(&functions).unwrap();
    let artifact = functions.join(format!("{hash}.bin"));
    let file = File::create(&artifact).unwrap();
    file.set_len(max_bytes + 1).unwrap();
    drop(file);

    assert!(cache.get(&hash).is_none());
    assert!(cache.stats().telemetry.corruptions > 0);
    assert!(artifact.is_file());
    cache.put(&hash, b"repaired").unwrap();
    assert_eq!(cache.get(&hash).as_deref(), Some(b"repaired".as_slice()));
}

#[test]
fn concurrent_writers_preserve_global_entry_and_byte_limits() {
    let dir = tmp_cache_dir();
    let sample_hash = fixture_hash("sample", b"body");
    let envelope_bytes =
        encode_cache_envelope(&sample_hash, CacheEnvelopeKind::Artifact, b"row-0").len() as u64;
    let max_entries = 3;
    let max_bytes = envelope_bytes * max_entries as u64;
    std::thread::scope(|scope| {
        for index in 0..12 {
            let dir = dir.clone();
            scope.spawn(move || {
                let hash = fixture_hash(&format!("writer-{index}"), b"body");
                let mut cache =
                    CompilationCache::open_with_limits(dir, 4096, max_entries, max_bytes);
                cache.put(&hash, format!("row-{index}").as_bytes()).unwrap();
            });
        }
    });

    let recovered = CompilationCache::open_with_limits(dir.clone(), 4096, max_entries, max_bytes);
    assert!(recovered.stats().entries <= max_entries);
    assert!(recovered.stats().persistent_bytes <= max_bytes);
    let persisted = std::fs::read_dir(dir.join("functions"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".bin") || name.ends_with(".poison"))
        })
        .collect::<Vec<_>>();
    assert!(persisted.len() <= max_entries);
    assert!(
        persisted
            .iter()
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum::<u64>()
            <= max_bytes
    );
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
    let stats = cache.stats();
    assert!(stats.persistent_bytes <= stats.max_persistent_bytes);
    assert!(cache.poison_path(&hash).unwrap().is_file());

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
    assert!(cache.stats().persistent_bytes <= max_persistent_bytes);

    let mut reopened = CompilationCache::open_with_limits(dir, 4096, 8, max_persistent_bytes);
    assert!(reopened.get(&hash).is_none());
    assert!(matches!(
        reopened.put(&hash, artifact),
        Err(CompilationCacheWriteError::Integrity(_))
    ));
}

#[test]
fn impossible_durable_poison_retires_namespace_and_drops_resident_payload() {
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
    assert!(!cache.index.contains_key(hash.as_str()));
    assert!(!cache.artifact_path(&hash).unwrap().exists());
    assert!(!cache.poison_path(&hash).unwrap().exists());
}

#[test]
fn limit_projection_keeps_hot_resident_ahead_of_cold_persistent_metadata() {
    let mut cache = CompilationCache::open_with_limits(tmp_cache_dir(), 1, 2, 4096);
    let cold = fixture_hash("cold", b"body");
    let hot = fixture_hash("hot", b"body");
    let newcomer = fixture_hash("new", b"body");
    cache.put(&cold, b"c").unwrap();
    cache.put(&hot, b"h").unwrap();
    assert!(cache.index[cold.as_str()].data.is_none());
    assert!(cache.index[hot.as_str()].data.is_some());

    cache.touch_metadata_entry(&hot, 1);
    cache.touch_metadata_entry(&cold, 2);
    assert!(cache.get(&hot).is_some());
    cache.upsert_metadata_entry(&newcomer, 3, 0, false);
    cache.enforce_cache_limits();

    assert!(!cache.index.contains_key(cold.as_str()));
    assert!(cache.index.contains_key(hot.as_str()));
    assert!(cache.index.contains_key(newcomer.as_str()));
    assert!(cache.index[hot.as_str()].last_access > 3);
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
fn persistent_entry_and_byte_limits_bound_artifacts_and_poison() {
    let dir = tmp_cache_dir();
    let envelope_bytes = encode_cache_envelope(
        &fixture_hash("measure", b"body"),
        CacheEnvelopeKind::Artifact,
        b"1234",
    )
    .len() as u64;
    let mut cache =
        CompilationCache::open_with_limits(dir, 1024, 2, envelope_bytes.saturating_mul(2));
    for index in 0..5 {
        cache
            .put(&format!("{index:064x}"), format!("row-{index}").as_bytes())
            .unwrap();
    }
    let stats = cache.stats();
    assert!(stats.entries <= 2);
    assert!(stats.persistent_bytes <= stats.max_persistent_bytes);
    assert!(stats.telemetry.evicted_entries >= 3);
}

#[test]
fn index_scan_and_retained_rows_are_bounded_by_entry_limit() {
    let dir = tmp_cache_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut index = format!("{CACHE_INDEX_HEADER}\n");
    for row in 0..10_000 {
        index.push_str(&format!("{row:064x}|{row}|1|artifact\n"));
    }
    std::fs::write(dir.join("index.txt"), index).unwrap();
    let cache = CompilationCache::open_with_limits(dir, 0, 2, 1024);
    assert!(cache.len() <= 2);
    assert!(cache.stats().telemetry.dropped_index_rows > 0);
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
fn index_is_versioned_strict_and_rejects_old_or_malformed_rows() {
    let dir = tmp_cache_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let valid = fixture_hash("valid", b"row");
    let functions = dir.join("functions");
    std::fs::create_dir_all(&functions).unwrap();
    let valid_envelope = encode_cache_envelope(&valid, CacheEnvelopeKind::Artifact, b"row");
    std::fs::write(functions.join(format!("{valid}.bin")), &valid_envelope).unwrap();
    let malformed = format!(
        "{CACHE_INDEX_HEADER}\n../escape|1|1|artifact\nABCDEF{}|2|1|artifact\n{valid}|not-a-time|1|artifact\n{valid}|3|1|artifact|extra\n{valid}|4|{}|artifact\n",
        valid_envelope.len(),
        "0".repeat(58)
    );
    std::fs::write(dir.join("index.txt"), malformed).unwrap();
    let cache = CompilationCache::open(dir.clone());
    assert_eq!(cache.len(), 1);
    assert!(cache.index.contains_key(valid.as_str()));

    std::fs::write(
        dir.join("index.txt"),
        format!("# hash|artifact_path|deps|last_access\n{valid}|../../escape||5\n"),
    )
    .unwrap();
    std::fs::remove_file(functions.join(format!("{valid}.bin"))).unwrap();
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
        let _ = cache.put("../escape", b"artifact");
    }));
    assert!(rejected.is_err());
    assert!(!dir.join("escape.bin").exists());
}

#[test]
fn sorted_index_serialization_is_process_stable() {
    fn cache_with_metadata(dir: PathBuf, rows: &[(String, u64)]) -> CompilationCache {
        let mut cache = CompilationCache::open(dir);
        for (hash, last_access) in rows {
            let poison_path = cache.poison_path(hash).unwrap();
            cache.index.insert(
                Arc::from(hash.as_str()),
                CacheEntry {
                    metadata_key: Arc::from(hash.as_str()),
                    poison_path,
                    last_access: *last_access,
                    data: None,
                    memory_stamp: 0,
                    memory_lru_index: None,
                    persistent_bytes: 0,
                    poisoned: false,
                },
            );
        }
        cache
    }

    let h1 = fixture_hash("a", b"body");
    let h2 = fixture_hash("b", b"body");
    let dir_a = tmp_cache_dir();
    let dir_b = tmp_cache_dir();
    let mut cache_a = cache_with_metadata(dir_a.clone(), &[(h2.clone(), 2), (h1.clone(), 1)]);
    let mut cache_b = cache_with_metadata(dir_b.clone(), &[(h1, 1), (h2, 2)]);
    cache_a.save_index().unwrap();
    cache_b.save_index().unwrap();
    assert_eq!(
        std::fs::read(dir_a.join("index.txt")).unwrap(),
        std::fs::read(dir_b.join("index.txt")).unwrap()
    );
}
