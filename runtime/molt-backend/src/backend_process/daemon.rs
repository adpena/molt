use super::*;

#[derive(Debug)]
#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct DaemonJobRequest {
    pub(crate) id: String,
    pub(crate) is_wasm: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<String>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_link: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_table_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_split_runtime_runtime_table_min: Option<u32>,
    pub(crate) output: String,
    pub(crate) cache_key: String,
    pub(crate) function_cache_key: Option<String>,
    pub(crate) skip_module_output_if_synced: bool,
    pub(crate) skip_function_output_if_synced: bool,
    pub(crate) probe_cache_only: bool,
    pub(crate) ir: Option<SimpleIR>,
    pub(crate) ir_path: Option<String>,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonRequest {
    pub(crate) version: Option<u32>,
    pub(crate) ping: Option<bool>,
    pub(crate) include_health: Option<bool>,
    pub(crate) config_digest: Option<String>,
    pub(crate) jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonJobResponse {
    pub(crate) id: String,
    pub(crate) ok: bool,
    pub(crate) cached: bool,
    pub(crate) cache_tier: Option<String>,
    pub(crate) output_written: bool,
    pub(crate) needs_ir: bool,
    pub(crate) message: Option<String>,
    /// Function names that were replaced with trap stubs due to Cranelift
    /// compilation failures.  Propagated to the CLI for build warnings.
    pub(crate) warnings: Vec<String>,
}

#[cfg(any(unix, test))]
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

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

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonHealthResponse {
    pub(crate) protocol_version: u32,
    pub(crate) pid: u32,
    pub(crate) spawn_config_digest: Option<String>,
    pub(crate) active_config_digest: Option<String>,
    pub(crate) uptime_ms: u64,
    pub(crate) cache_entries: usize,
    pub(crate) cache_bytes: usize,
    pub(crate) cache_max_bytes: Option<usize>,
    pub(crate) request_limit_bytes: Option<usize>,
    pub(crate) max_jobs: Option<usize>,
    pub(crate) requests_total: u64,
    pub(crate) jobs_total: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonResponse {
    pub(crate) ok: bool,
    pub(crate) pong: bool,
    pub(crate) jobs: Vec<DaemonJobResponse>,
    pub(crate) error: Option<String>,
    pub(crate) health: Option<DaemonHealthResponse>,
}

#[cfg(any(unix, test))]
impl DaemonJobRequest {
    pub(crate) fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
        let ir_path = optional_string(obj, "ir_path", ctx)?;
        if obj.get("ir").is_some_and(|value| !value.is_null()) && ir_path.is_some() {
            return Err(format!(
                "{ctx} must use exactly one IR custody field: ir or ir_path"
            ));
        }
        let ir = match obj.get("ir") {
            None | Some(JsonValue::Null) => None,
            Some(ir_value) => Some(SimpleIR::from_json_value(ir_value)?),
        };
        Ok(Self {
            id: required_string(obj, "id", ctx)?,
            is_wasm,
            target_triple: optional_string(obj, "target_triple", ctx)?,
            wasm_link: optional_bool(obj, "wasm_link", ctx)?.unwrap_or(false),
            wasm_data_base: optional_u32(obj, "wasm_data_base", ctx)?,
            wasm_table_base: optional_u32(obj, "wasm_table_base", ctx)?,
            wasm_split_runtime_runtime_table_min: optional_u32(
                obj,
                "wasm_split_runtime_runtime_table_min",
                ctx,
            )?,
            output: required_string(obj, "output", ctx)?,
            cache_key: required_string(obj, "cache_key", ctx)?,
            function_cache_key: optional_string(obj, "function_cache_key", ctx)?,
            skip_module_output_if_synced: optional_bool(obj, "skip_module_output_if_synced", ctx)?
                .unwrap_or(false),
            skip_function_output_if_synced: optional_bool(
                obj,
                "skip_function_output_if_synced",
                ctx,
            )?
            .unwrap_or(false),
            probe_cache_only: optional_bool(obj, "probe_cache_only", ctx)?.unwrap_or(false),
            ir,
            ir_path,
        })
    }
}

#[cfg(any(unix, test))]
pub(crate) fn simple_ir_from_json_path(path: &str) -> Result<SimpleIR, String> {
    let file = File::open(path).map_err(|err| format!("failed to open ir_path {path:?}: {err}"))?;
    serde_json::from_reader(io::BufReader::new(file))
        .map_err(|err| format!("failed to parse ir_path {path:?}: {err}"))
}

#[cfg(any(unix, test))]
impl DaemonRequest {
    pub(crate) fn from_json_bytes(bytes: &[u8]) -> Result<Self, String> {
        let value: JsonValue =
            serde_json::from_slice(bytes).map_err(|err| format!("invalid request JSON: {err}"))?;
        let obj = expect_object(&value, "request")?;
        let version = match obj.get("version") {
            None | Some(JsonValue::Null) => None,
            Some(value) => {
                let Some(raw) = value.as_u64() else {
                    return Err("request.version must be a non-negative integer".to_string());
                };
                Some(
                    u32::try_from(raw)
                        .map_err(|_| "request.version is out of range for u32".to_string())?,
                )
            }
        };
        let jobs = match obj.get("jobs") {
            None | Some(JsonValue::Null) => None,
            Some(value) => {
                let array = value
                    .as_array()
                    .ok_or_else(|| "request.jobs must be an array".to_string())?;
                let mut out = Vec::with_capacity(array.len());
                for (idx, item) in array.iter().enumerate() {
                    out.push(DaemonJobRequest::from_json_value(
                        item,
                        &format!("request.jobs[{idx}]"),
                    )?);
                }
                Some(out)
            }
        };
        // Apply per-request env var overrides so callers can control
        // backend diagnostics and non-TIR tuning without restarting the
        // daemon. TIR itself is not request-optional.
        for key in DAEMON_REQUEST_ENV_KEYS {
            unsafe {
                std::env::remove_var(key);
            }
        }
        if let Some(JsonValue::Object(env_map)) = obj.get("env") {
            for (key, val) in env_map {
                if let Some(s) = val.as_str() {
                    if key == molt_backend::STDLIB_MODULE_SYMBOLS_ENV {
                        molt_backend::parse_stdlib_module_symbols(s)?;
                    }
                    unsafe {
                        std::env::set_var(key, s);
                    }
                } else if key == molt_backend::STDLIB_MODULE_SYMBOLS_ENV {
                    return Err(format!(
                        "{} must be a string containing a JSON array of emitted module symbols",
                        molt_backend::STDLIB_MODULE_SYMBOLS_ENV
                    ));
                }
            }
        }
        Ok(Self {
            version,
            ping: optional_bool(obj, "ping", "request")?,
            include_health: optional_bool(obj, "include_health", "request")?,
            config_digest: optional_string(obj, "config_digest", "request")?,
            jobs,
        })
    }
}

#[cfg(any(unix, test))]
impl DaemonJobResponse {
    pub(crate) fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), JsonValue::String(self.id.clone()));
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert("cached".to_string(), JsonValue::Bool(self.cached));
        if let Some(cache_tier) = &self.cache_tier {
            obj.insert(
                "cache_tier".to_string(),
                JsonValue::String(cache_tier.clone()),
            );
        }
        obj.insert(
            "output_written".to_string(),
            JsonValue::Bool(self.output_written),
        );
        if !is_false(&self.needs_ir) {
            obj.insert("needs_ir".to_string(), JsonValue::Bool(self.needs_ir));
        }
        if let Some(message) = &self.message {
            obj.insert("message".to_string(), JsonValue::String(message.clone()));
        }
        if !self.warnings.is_empty() {
            obj.insert(
                "warnings".to_string(),
                JsonValue::Array(
                    self.warnings
                        .iter()
                        .map(|w| JsonValue::String(w.clone()))
                        .collect(),
                ),
            );
        }
        JsonValue::Object(obj)
    }
}

#[cfg(any(unix, test))]
impl DaemonHealthResponse {
    pub(crate) fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "protocol_version".to_string(),
            JsonValue::from(self.protocol_version),
        );
        obj.insert("pid".to_string(), JsonValue::from(self.pid));
        if let Some(spawn_config_digest) = &self.spawn_config_digest {
            obj.insert(
                "spawn_config_digest".to_string(),
                JsonValue::String(spawn_config_digest.clone()),
            );
        }
        if let Some(active_config_digest) = &self.active_config_digest {
            obj.insert(
                "active_config_digest".to_string(),
                JsonValue::String(active_config_digest.clone()),
            );
        }
        obj.insert("uptime_ms".to_string(), JsonValue::from(self.uptime_ms));
        obj.insert(
            "cache_entries".to_string(),
            JsonValue::from(self.cache_entries),
        );
        obj.insert("cache_bytes".to_string(), JsonValue::from(self.cache_bytes));
        if let Some(cache_max_bytes) = self.cache_max_bytes {
            obj.insert(
                "cache_max_bytes".to_string(),
                JsonValue::from(cache_max_bytes),
            );
        }
        if let Some(request_limit_bytes) = self.request_limit_bytes {
            obj.insert(
                "request_limit_bytes".to_string(),
                JsonValue::from(request_limit_bytes),
            );
        }
        if let Some(max_jobs) = self.max_jobs {
            obj.insert("max_jobs".to_string(), JsonValue::from(max_jobs));
        }
        obj.insert(
            "requests_total".to_string(),
            JsonValue::from(self.requests_total),
        );
        obj.insert("jobs_total".to_string(), JsonValue::from(self.jobs_total));
        obj.insert("cache_hits".to_string(), JsonValue::from(self.cache_hits));
        obj.insert(
            "cache_misses".to_string(),
            JsonValue::from(self.cache_misses),
        );
        JsonValue::Object(obj)
    }
}

#[cfg(any(unix, test))]
impl DaemonResponse {
    pub(crate) fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert("pong".to_string(), JsonValue::Bool(self.pong));
        obj.insert(
            "jobs".to_string(),
            JsonValue::Array(
                self.jobs
                    .iter()
                    .map(DaemonJobResponse::to_json_value)
                    .collect(),
            ),
        );
        if let Some(error) = &self.error {
            obj.insert("error".to_string(), JsonValue::String(error.clone()));
        }
        if let Some(health) = &self.health {
            obj.insert("health".to_string(), health.to_json_value());
        }
        JsonValue::Object(obj)
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
pub(crate) enum DaemonCompiledOutput {
    #[cfg(feature = "wasm-backend")]
    Bytes(Arc<[u8]>),
    WrittenToPath,
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

#[cfg(any(unix, test))]
pub(crate) fn compile_single_job(
    job: DaemonJobRequest,
    _cache: &mut DaemonCache,
) -> DaemonJobResponse {
    #[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
    {
        let unsupported = if job.is_wasm {
            "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend"
        } else {
            "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend"
        };
        return DaemonJobResponse {
            id: job.id,
            ok: false,
            cached: false,
            cache_tier: None,
            output_written: false,
            needs_ir: false,
            message: Some(unsupported.to_string()),
            warnings: Vec::new(),
        };
    }

    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    {
        let cache_key = job.cache_key.trim();
        let function_cache_key = job
            .function_cache_key
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        let daemon_memory_cache_allowed = daemon_memory_cache_allowed_for_job(&job);
        if daemon_memory_cache_allowed
            && !cache_key.is_empty()
            && let Some(bytes) = _cache.get_bytes(cache_key)
        {
            match write_cached_output(&job.output, bytes, job.skip_module_output_if_synced) {
                Ok(output_written) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        cache_tier: Some("module".to_string()),
                        output_written,
                        needs_ir: false,
                        message: None,
                        warnings: Vec::new(),
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write cached output: {err}")),
                        warnings: Vec::new(),
                    };
                }
            }
        }
        if daemon_memory_cache_allowed
            && !function_cache_key.is_empty()
            && function_cache_key != cache_key
            && let Some(bytes) = _cache.get_bytes(function_cache_key)
        {
            match write_cached_output(&job.output, bytes, job.skip_function_output_if_synced) {
                Ok(output_written) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        cache_tier: Some("function".to_string()),
                        output_written,
                        needs_ir: false,
                        message: None,
                        warnings: Vec::new(),
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write cached output: {err}")),
                        warnings: Vec::new(),
                    };
                }
            }
        }

        if job.probe_cache_only {
            return DaemonJobResponse {
                id: job.id,
                ok: true,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: true,
                message: None,
                warnings: Vec::new(),
            };
        }

        let mut ir = if let Some(ir) = job.ir {
            ir
        } else if let Some(ir_path) = job.ir_path.as_deref() {
            match simple_ir_from_json_path(ir_path) {
                Ok(ir) => ir,
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(err),
                        warnings: Vec::new(),
                    };
                }
            }
        } else {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some("missing ir for cache miss".to_string()),
                warnings: Vec::new(),
            };
        };

        let mut warnings = Vec::new();
        let compiled_output = if job.is_wasm {
            #[cfg(feature = "wasm-backend")]
            {
                let mut options = WasmCompileOptions {
                    reloc_enabled: job.wasm_link,
                    ..WasmCompileOptions::default()
                };
                if let Some(data_base) = job.wasm_data_base {
                    options.data_base = data_base;
                }
                if let Some(table_base) = job.wasm_table_base {
                    options.table_base = table_base;
                }
                if let Some(split_runtime_runtime_table_min) =
                    job.wasm_split_runtime_runtime_table_min
                {
                    options.split_runtime_runtime_table_min = Some(split_runtime_runtime_table_min);
                }
                let backend = WasmBackend::with_options(options);
                DaemonCompiledOutput::Bytes(Arc::from(backend.compile(ir)))
            }
            #[cfg(not(feature = "wasm-backend"))]
            {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    output_written: false,
                    needs_ir: false,
                    message: Some(
                        "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend".to_string(),
                    ),
                    warnings: Vec::new(),
                };
            }
        } else {
            #[cfg(feature = "native-backend")]
            {
                let target_triple = job.target_triple.as_deref();
                let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
                let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
                let expected_stdlib_cache_manifest =
                    std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
                let entry_module =
                    std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
                let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
                let explicit_stdlib_module_symbols =
                    match molt_backend::stdlib_module_symbols_from_env() {
                        Ok(symbols) => symbols,
                        Err(err) => {
                            return DaemonJobResponse {
                                id: job.id,
                                ok: false,
                                cached: false,
                                cache_tier: None,
                                output_written: false,
                                needs_ir: false,
                                message: Some(err),
                                warnings: Vec::new(),
                            };
                        }
                    };
                let compile_options = match prepare_native_application_object(
                    &mut ir,
                    NativeStdlibCachePrepare {
                        target_triple,
                        stdlib_obj_path: stdlib_obj_path.as_deref(),
                        expected_cache_key: expected_stdlib_cache_key.as_deref(),
                        expected_cache_manifest: expected_stdlib_cache_manifest.as_deref(),
                        have_entry_module,
                        entry_module: &entry_module,
                        explicit_stdlib_module_symbols: explicit_stdlib_module_symbols.as_ref(),
                        log_prefix: "MOLT_BACKEND(daemon)",
                    },
                ) {
                    Ok(options) => options,
                    Err(err) => {
                        return DaemonJobResponse {
                            id: job.id,
                            ok: false,
                            cached: false,
                            cache_tier: None,
                            output_written: false,
                            needs_ir: false,
                            message: Some(err.to_string()),
                            warnings: Vec::new(),
                        };
                    }
                };

                if let Err(err) = compile_native_application_object_to_path(
                    ir,
                    Path::new(&job.output),
                    compile_options,
                ) {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!(
                            "failed to compile native application object: {err}"
                        )),
                        warnings: Vec::new(),
                    };
                }
                DaemonCompiledOutput::WrittenToPath
            }
            #[cfg(not(feature = "native-backend"))]
            {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    output_written: false,
                    needs_ir: false,
                    message: Some(
                        "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend".to_string(),
                    ),
                    warnings: Vec::new(),
                };
            }
        };

        match compiled_output {
            #[cfg(feature = "wasm-backend")]
            DaemonCompiledOutput::Bytes(output_bytes) => {
                if let Err(err) = write_output(&job.output, output_bytes.as_ref()) {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write compiled output: {err}")),
                        warnings: Vec::new(),
                    };
                }
                insert_daemon_cache_entries(_cache, cache_key, function_cache_key, output_bytes);
            }
            DaemonCompiledOutput::WrittenToPath => {
                if daemon_memory_cache_allowed {
                    maybe_cache_output_file(
                        _cache,
                        Path::new(&job.output),
                        cache_key,
                        function_cache_key,
                        &mut warnings,
                    );
                }
            }
        }

        DaemonJobResponse {
            id: job.id,
            ok: true,
            cached: false,
            cache_tier: None,
            output_written: true,
            needs_ir: false,
            message: None,
            warnings,
        }
    }
}

#[cfg(unix)]
pub(crate) fn run_daemon(socket_path: &str) -> io::Result<()> {
    use std::os::unix::net::UnixListener;

    let socket = Path::new(socket_path);
    if socket.exists() {
        let _ = std::fs::remove_file(socket);
    }
    if let Some(parent) = socket.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket)?;
    let request_limit_bytes = daemon_request_limit_bytes();
    let max_jobs = daemon_max_jobs();
    let mut cache = DaemonCache::new(Some(daemon_cache_limit_bytes()));
    let mut stats = DaemonStats::default();
    let spawn_config_digest = env::var("MOLT_BACKEND_DAEMON_CONFIG_DIGEST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut active_config_digest: Option<String> = None;
    let started_at = Instant::now();
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(err) = handle_daemon_connection(
                    &mut conn,
                    DaemonConnectionContext {
                        cache: &mut cache,
                        stats: &mut stats,
                        spawn_config_digest: spawn_config_digest.as_deref(),
                        active_config_digest: &mut active_config_digest,
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    },
                ) {
                    eprintln!("backend daemon connection error: {err}");
                }
            }
            Err(err) => {
                eprintln!("backend daemon accept error: {err}");
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) struct DaemonConnectionContext<'a> {
    pub(crate) cache: &'a mut DaemonCache,
    pub(crate) stats: &'a mut DaemonStats,
    pub(crate) spawn_config_digest: Option<&'a str>,
    pub(crate) active_config_digest: &'a mut Option<String>,
    pub(crate) started_at: Instant,
    pub(crate) request_limit_bytes: usize,
    pub(crate) max_jobs: usize,
}

#[cfg(unix)]
pub(crate) fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    ctx: DaemonConnectionContext<'_>,
) -> io::Result<()> {
    let DaemonConnectionContext {
        cache,
        stats,
        spawn_config_digest,
        active_config_digest,
        started_at,
        request_limit_bytes,
        max_jobs,
    } = ctx;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    loop {
        let raw_bytes = read_daemon_request_bytes(&mut reader, request_limit_bytes)?;
        if raw_bytes.is_empty() {
            return Ok(());
        }
        stats.requests_total = stats.requests_total.saturating_add(1);
        if raw_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("empty request".to_string()),
                health: None,
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        let req = match DaemonRequest::from_json_bytes(&raw_bytes) {
            Ok(req) => req,
            Err(err) => {
                let response = DaemonResponse {
                    ok: false,
                    pong: false,
                    jobs: Vec::new(),
                    error: Some(format!("invalid request JSON: {err}")),
                    health: None,
                };
                write_daemon_response(stream, &response)?;
                continue;
            }
        };
        let include_health = req.include_health.unwrap_or(req.ping.unwrap_or(false));
        let version = req.version.unwrap_or(0);
        if version != BACKEND_DAEMON_PROTOCOL_VERSION {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!(
                    "unsupported protocol version {version}; expected {BACKEND_DAEMON_PROTOCOL_VERSION}"
                )),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        if req.ping.unwrap_or(false) {
            let response = DaemonResponse {
                ok: true,
                pong: true,
                jobs: Vec::new(),
                error: None,
                health: Some(daemon_health(
                    cache,
                    stats,
                    spawn_config_digest,
                    active_config_digest.as_deref(),
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        let request_config_digest = req
            .config_digest
            .as_deref()
            .map(str::trim)
            .filter(|digest| !digest.is_empty())
            .map(|digest| digest.to_string());
        if let Some(ref digest) = request_config_digest
            && active_config_digest.as_deref() != Some(digest.as_str())
        {
            cache.clear();
            *active_config_digest = Some(digest.clone());
        }
        let Some(jobs) = req.jobs else {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("missing jobs in request".to_string()),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        };
        if jobs.is_empty() {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("empty jobs in request".to_string()),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        if jobs.len() > max_jobs {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!(
                    "too many jobs in request: {} exceeds daemon max_jobs {}",
                    jobs.len(),
                    max_jobs
                )),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        stats.jobs_total = stats.jobs_total.saturating_add(jobs.len() as u64);
        let mut results = Vec::with_capacity(jobs.len());
        for job in jobs {
            let result = compile_single_job(job, cache);
            if result.ok && result.cached {
                stats.cache_hits = stats.cache_hits.saturating_add(1);
            } else {
                stats.cache_misses = stats.cache_misses.saturating_add(1);
            }
            results.push(result);
        }
        let all_ok = results.iter().all(|job| job.ok);
        let response = DaemonResponse {
            ok: all_ok,
            pong: false,
            jobs: results,
            error: None,
            health: include_health.then(|| {
                daemon_health(
                    cache,
                    stats,
                    spawn_config_digest,
                    active_config_digest.as_deref(),
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )
            }),
        };
        write_daemon_response(stream, &response)?;
    }
}

#[cfg(unix)]
pub(crate) fn read_daemon_request_bytes<R: BufRead>(
    reader: &mut R,
    request_limit_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut raw_bytes = Vec::new();
    let limit = u64::try_from(request_limit_bytes)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    reader.take(limit).read_until(b'\n', &mut raw_bytes)?;
    if raw_bytes.len() > request_limit_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("daemon request exceeded {request_limit_bytes} byte limit"),
        ));
    }
    Ok(raw_bytes)
}

#[cfg(unix)]
pub(crate) fn write_daemon_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &DaemonResponse,
) -> io::Result<()> {
    let mut payload = daemon_response_payload(response)?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn daemon_response_payload(response: &DaemonResponse) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&response.to_json_value()).map_err(io::Error::other)
}

#[cfg(not(unix))]
pub(crate) fn run_daemon(_socket_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon mode requires unix domain sockets",
    ))
}
