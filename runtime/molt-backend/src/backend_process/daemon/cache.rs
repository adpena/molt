use super::*;

#[cfg(all(
    any(unix, test),
    any(feature = "native-backend", feature = "wasm-backend")
))]
pub(crate) fn daemon_memory_cache_allowed_for_job(job: &DaemonJobRequest) -> bool {
    if job.is_wasm {
        return true;
    }
    #[cfg(feature = "native-backend")]
    {
        let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
            return true;
        };
        shared_stdlib_cache_matches(
            Path::new(&stdlib_obj_path),
            std::env::var("MOLT_STDLIB_CACHE_KEY").ok().as_deref(),
            std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok().as_deref(),
            None,
        )
    }
    #[cfg(not(feature = "native-backend"))]
    {
        false
    }
}

#[derive(Default)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonStats {
    pub(crate) requests_total: u64,
    pub(crate) jobs_total: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
}

#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct DaemonCache {
    pub(crate) entries: HashMap<Arc<str>, CacheEntry>,
    pub(crate) order: BinaryHeap<Reverse<(u64, Arc<str>)>>,
    pub(crate) clock: u64,
    pub(crate) bytes: usize,
    pub(crate) max_bytes: Option<usize>,
}

#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct CacheEntry {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) stamp: u64,
}

#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
impl DaemonCache {
    pub(crate) fn new(max_bytes: Option<usize>) -> Self {
        Self {
            entries: HashMap::new(),
            order: BinaryHeap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
        }
    }

    pub(crate) fn get_bytes(&mut self, key: &str) -> Option<&[u8]> {
        let key_ref = Arc::clone(self.entries.get_key_value(key)?.0);
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        entry.stamp = stamp;
        self.order.push(Reverse((stamp, key_ref)));
        Some(entry.bytes.as_ref())
    }

    pub(crate) fn insert(&mut self, key: String, value: Arc<[u8]>) {
        if key.is_empty() {
            return;
        }
        if let Some(prev) = self.entries.remove(key.as_str()) {
            self.bytes = self.bytes.saturating_sub(prev.bytes.len());
        }
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        self.bytes = self.bytes.saturating_add(value.len());
        let key = Arc::<str>::from(key);
        self.order.push(Reverse((stamp, Arc::clone(&key))));
        self.entries.insert(
            key,
            CacheEntry {
                bytes: value,
                stamp,
            },
        );
        self.evict();
    }

    fn evict(&mut self) {
        while self
            .max_bytes
            .is_some_and(|max_bytes| self.bytes > max_bytes)
        {
            let Some(Reverse((stamp, old_key))) = self.order.pop() else {
                break;
            };
            let is_live = self
                .entries
                .get(&old_key)
                .is_some_and(|entry| entry.stamp == stamp);
            if !is_live {
                continue;
            }
            if let Some(old_val) = self.entries.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(old_val.bytes.len());
            }
        }
        // Compact stale generations after enough churn.
        if self.order.len() > self.entries.len().saturating_mul(8).saturating_add(32) {
            let mut compacted = BinaryHeap::with_capacity(self.entries.len());
            for (key, entry) in &self.entries {
                compacted.push(Reverse((entry.stamp, Arc::clone(key))));
            }
            self.order = compacted;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.clock = 0;
        self.bytes = 0;
    }
}

#[cfg(any(unix, test))]
pub(crate) fn default_daemon_cache_bytes_from_physical_mem_bytes(bytes: Option<u64>) -> usize {
    let default = bytes
        .and_then(|raw| usize::try_from(raw / 64).ok())
        .unwrap_or(512 * MIB);
    default.clamp(128 * MIB, 2 * 1024 * MIB)
}

#[cfg(any(unix, test))]
pub(crate) fn daemon_cache_limit_bytes() -> usize {
    env::var("MOLT_BACKEND_DAEMON_CACHE_MB")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|mb| mb.saturating_mul(MIB))
        .unwrap_or_else(|| {
            default_daemon_cache_bytes_from_physical_mem_bytes(detect_physical_memory_bytes())
        })
}

#[cfg(any(unix, test))]
pub(crate) fn daemon_health(
    cache: &DaemonCache,
    stats: &DaemonStats,
    spawn_config_digest: Option<&str>,
    active_config_digest: Option<&str>,
    start: Instant,
    request_limit_bytes: usize,
    max_jobs: usize,
) -> DaemonHealthResponse {
    let uptime_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    DaemonHealthResponse {
        protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
        pid: std::process::id(),
        spawn_config_digest: spawn_config_digest.map(str::to_string),
        active_config_digest: active_config_digest.map(str::to_string),
        uptime_ms,
        cache_entries: cache.entries.len(),
        cache_bytes: cache.bytes,
        cache_max_bytes: cache.max_bytes,
        request_limit_bytes: Some(request_limit_bytes),
        max_jobs: Some(max_jobs),
        requests_total: stats.requests_total,
        jobs_total: stats.jobs_total,
        cache_hits: stats.cache_hits,
        cache_misses: stats.cache_misses,
    }
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
pub(crate) fn insert_daemon_cache_entries(
    cache: &mut DaemonCache,
    cache_key: &str,
    function_cache_key: &str,
    output_bytes: Arc<[u8]>,
) {
    if !cache_key.is_empty() && !function_cache_key.is_empty() && function_cache_key != cache_key {
        cache.insert(cache_key.to_string(), Arc::clone(&output_bytes));
        cache.insert(function_cache_key.to_string(), output_bytes);
    } else if !cache_key.is_empty() {
        cache.insert(cache_key.to_string(), output_bytes);
    } else if !function_cache_key.is_empty() {
        cache.insert(function_cache_key.to_string(), output_bytes);
    }
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
pub(crate) fn maybe_cache_output_file(
    cache: &mut DaemonCache,
    output_path: &Path,
    cache_key: &str,
    function_cache_key: &str,
    warnings: &mut Vec<String>,
) {
    if cache_key.is_empty() && function_cache_key.is_empty() {
        return;
    }
    let metadata = match std::fs::metadata(output_path) {
        Ok(metadata) => metadata,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': metadata failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    let output_len = metadata.len();
    if cache
        .max_bytes
        .is_some_and(|max_bytes| output_len > max_bytes as u64)
    {
        let warning = format!(
            "skipped daemon memory cache for '{}' ({} bytes exceeds cache budget)",
            output_path.display(),
            output_len
        );
        eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
        warnings.push(warning);
        return;
    }
    let bytes = match std::fs::read(output_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': read failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    insert_daemon_cache_entries(
        cache,
        cache_key,
        function_cache_key,
        Arc::from(bytes.into_boxed_slice()),
    );
}
