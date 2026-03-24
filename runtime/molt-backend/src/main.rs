#[cfg(feature = "luau-backend")]
use molt_backend::luau::LuauBackend;
#[cfg(feature = "native-backend")]
use molt_backend::SimpleBackend;
use molt_backend::SimpleIR;
#[cfg(feature = "rust-backend")]
use molt_backend::rust::RustBackend;
#[cfg(feature = "wasm-backend")]
use molt_backend::wasm::{WasmBackend, WasmCompileOptions};
use serde_json::Value as JsonValue;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
use std::io::BufRead;
use std::io::Write;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

mod json_boundary;

use crate::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};

const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
#[derive(Debug)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct DaemonJobRequest {
    id: String,
    is_wasm: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    target_triple: Option<String>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_link: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_table_base: Option<u32>,
    output: String,
    cache_key: String,
    function_cache_key: Option<String>,
    skip_module_output_if_synced: bool,
    skip_function_output_if_synced: bool,
    probe_cache_only: bool,
    ir: Option<SimpleIR>,
}

#[derive(Debug)]
struct DaemonRequest {
    version: Option<u32>,
    ping: Option<bool>,
    include_health: Option<bool>,
    config_digest: Option<String>,
    jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug)]
struct DaemonJobResponse {
    id: String,
    ok: bool,
    cached: bool,
    cache_tier: Option<String>,
    output_written: bool,
    needs_ir: bool,
    message: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug)]
struct DaemonHealthResponse {
    protocol_version: u32,
    pid: u32,
    uptime_ms: u64,
    cache_entries: usize,
    cache_bytes: usize,
    cache_max_bytes: Option<usize>,
    request_limit_bytes: Option<usize>,
    max_jobs: Option<usize>,
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

#[derive(Debug)]
struct DaemonResponse {
    ok: bool,
    pong: bool,
    jobs: Vec<DaemonJobResponse>,
    error: Option<String>,
    health: Option<DaemonHealthResponse>,
}

impl DaemonJobRequest {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
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
        })
    }
}

impl DaemonRequest {
    fn from_json_bytes(bytes: &[u8]) -> Result<Self, String> {
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
        Ok(Self {
            version,
            ping: optional_bool(obj, "ping", "request")?,
            include_health: optional_bool(obj, "include_health", "request")?,
            config_digest: optional_string(obj, "config_digest", "request")?,
            jobs,
        })
    }
}

impl DaemonJobResponse {
    fn to_json_value(&self) -> JsonValue {
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
        JsonValue::Object(obj)
    }
}

impl DaemonHealthResponse {
    fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "protocol_version".to_string(),
            JsonValue::from(self.protocol_version),
        );
        obj.insert("pid".to_string(), JsonValue::from(self.pid));
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

impl DaemonResponse {
    fn to_json_value(&self) -> JsonValue {
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
struct DaemonStats {
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct DaemonCache {
    entries: HashMap<Arc<str>, CacheEntry>,
    order: BinaryHeap<Reverse<(u64, Arc<str>)>>,
    clock: u64,
    bytes: usize,
    max_bytes: Option<usize>,
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct CacheEntry {
    bytes: Arc<[u8]>,
    stamp: u64,
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
impl DaemonCache {
    fn new(max_bytes: Option<usize>) -> Self {
        Self {
            entries: HashMap::new(),
            order: BinaryHeap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
        }
    }

    fn get_bytes(&mut self, key: &str) -> Option<&[u8]> {
        let key_ref = Arc::clone(self.entries.get_key_value(key)?.0);
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        entry.stamp = stamp;
        self.order.push(Reverse((stamp, key_ref)));
        Some(entry.bytes.as_ref())
    }

    fn insert(&mut self, key: String, value: Arc<[u8]>) {
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

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.clock = 0;
        self.bytes = 0;
    }
}

fn daemon_health(cache: &DaemonCache, stats: &DaemonStats, start: Instant) -> DaemonHealthResponse {
    let uptime_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    DaemonHealthResponse {
        protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
        pid: std::process::id(),
        uptime_ms,
        cache_entries: cache.entries.len(),
        cache_bytes: cache.bytes,
        cache_max_bytes: cache.max_bytes,
        request_limit_bytes: None,
        max_jobs: None,
        requests_total: stats.requests_total,
        jobs_total: stats.jobs_total,
        cache_hits: stats.cache_hits,
        cache_misses: stats.cache_misses,
    }
}

fn compile_single_job(job: DaemonJobRequest, _cache: &mut DaemonCache) -> DaemonJobResponse {
    #[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
    {
        let unsupported = if job.is_wasm {
            "backend binary was built without wasm-backend support"
        } else {
            "backend binary was built without native-backend support"
        };
        return DaemonJobResponse {
            id: job.id,
            ok: false,
            cached: false,
            cache_tier: None,
            output_written: false,
            needs_ir: false,
            message: Some(unsupported.to_string()),
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
        if !cache_key.is_empty()
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
                    };
                }
            }
        }
        if !function_cache_key.is_empty()
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
            };
        }

        let Some(ir) = job.ir else {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some("missing ir for cache miss".to_string()),
            };
        };

        let output_bytes: Arc<[u8]> = if job.is_wasm {
            #[cfg(feature = "wasm-backend")]
            {
                let mut options = WasmCompileOptions::default();
                options.reloc_enabled = job.wasm_link;
                if let Some(data_base) = job.wasm_data_base {
                    options.data_base = data_base;
                }
                if let Some(table_base) = job.wasm_table_base {
                    options.table_base = table_base;
                }
                let backend = WasmBackend::with_options(options);
                Arc::from(backend.compile(ir))
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
                        "backend binary was built without wasm-backend support".to_string(),
                    ),
                };
            }
        } else {
            #[cfg(feature = "native-backend")]
            {
                let backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
                Arc::from(backend.compile(ir))
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
                        "backend binary was built without native-backend support".to_string(),
                    ),
                };
            }
        };

        if let Err(err) = write_output(&job.output, output_bytes.as_ref()) {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some(format!("failed to write compiled output: {err}")),
            };
        }

        if !cache_key.is_empty()
            && !function_cache_key.is_empty()
            && function_cache_key != cache_key
        {
            _cache.insert(cache_key.to_string(), Arc::clone(&output_bytes));
            _cache.insert(function_cache_key.to_string(), output_bytes);
        } else if !cache_key.is_empty() {
            _cache.insert(cache_key.to_string(), output_bytes);
        } else if !function_cache_key.is_empty() {
            _cache.insert(function_cache_key.to_string(), output_bytes);
        }

        DaemonJobResponse {
            id: job.id,
            ok: true,
            cached: false,
            cache_tier: None,
            output_written: true,
            needs_ir: false,
            message: None,
        }
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
fn write_cached_output(path: &str, bytes: &[u8], skip_if_synced: bool) -> io::Result<bool> {
    if skip_if_synced {
        return Ok(false);
    }
    write_output(path, bytes)?;
    Ok(true)
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
fn write_output(path: &str, bytes: &[u8]) -> io::Result<()> {
    let output_path = Path::new(path);
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let base_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_name = format!(".{base_name}.{}.{}.tmp", std::process::id(), nonce);
    let tmp_path = output_path.with_file_name(tmp_name);
    let mut file = File::create(&tmp_path)?;
    file.write_all(bytes)?;
    drop(file);

    match std::fs::rename(&tmp_path, output_path) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            let _ = std::fs::remove_file(output_path);
            match std::fs::rename(&tmp_path, output_path) {
                Ok(()) => Ok(()),
                Err(second_err) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    Err(io::Error::new(
                        second_err.kind(),
                        format!(
                            "failed to atomically replace output (first: {first_err}; second: {second_err})"
                        ),
                    ))
                }
            }
        }
    }
}

#[cfg(unix)]
fn run_daemon(socket_path: &str) -> io::Result<()> {
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
    let mut cache = DaemonCache::new(None);
    let mut stats = DaemonStats::default();
    let mut active_config_digest: Option<String> = None;
    let started_at = Instant::now();
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(err) = handle_daemon_connection(
                    &mut conn,
                    &mut cache,
                    &mut stats,
                    &mut active_config_digest,
                    started_at,
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
fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    cache: &mut DaemonCache,
    stats: &mut DaemonStats,
    active_config_digest: &mut Option<String>,
    started_at: Instant,
) -> io::Result<()> {
    loop {
        let raw_bytes = read_daemon_request_bytes(stream)?;
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
                health: include_health.then(|| daemon_health(cache, stats, started_at)),
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
                health: Some(daemon_health(cache, stats, started_at)),
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
                health: include_health.then(|| daemon_health(cache, stats, started_at)),
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
                health: include_health.then(|| daemon_health(cache, stats, started_at)),
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
            health: include_health.then(|| daemon_health(cache, stats, started_at)),
        };
        write_daemon_response(stream, &response)?;
    }
}

#[cfg(unix)]
fn read_daemon_request_bytes<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut raw_bytes = Vec::new();
    let mut buffered = io::BufReader::new(reader);
    buffered.read_until(b'\n', &mut raw_bytes)?;
    Ok(raw_bytes)
}

#[cfg(unix)]
fn write_daemon_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &DaemonResponse,
) -> io::Result<()> {
    let mut payload = daemon_response_payload(response)?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    Ok(())
}

#[cfg(unix)]
fn daemon_response_payload(response: &DaemonResponse) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&response.to_json_value()).map_err(io::Error::other)
}

#[cfg(not(unix))]
fn run_daemon(_socket_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon mode requires unix domain sockets",
    ))
}

fn main() -> io::Result<()> {
    // Hard memory guard: set rlimit on virtual memory to prevent OOM
    // from crashing the entire machine.  Default 4GB, override with
    // MOLT_BACKEND_MAX_RSS_GB env var.
    #[cfg(unix)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            let rlim = libc::rlimit {
                rlim_cur: max_bytes,
                rlim_max: max_bytes,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                eprintln!(
                    "WARNING: failed to set memory limit (RLIMIT_AS={max_gb}GB).                      OOM guard not active."
                );
            }
        }
    }

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--daemon") {
        let socket_path = args
            .iter()
            .position(|arg| arg == "--socket")
            .and_then(|idx| args.get(idx + 1))
            .map(String::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))?;
        return run_daemon(socket_path);
    }
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());
    let is_rust = args.contains(&"--target".to_string()) && args.contains(&"rust".to_string());
    let is_luau = args.contains(&"--target".to_string()) && args.contains(&"luau".to_string());
    #[allow(unused_variables)]
    let use_ir_pipeline = args.contains(&"--ir-pipeline".to_string());
    #[cfg_attr(not(feature = "native-backend"), allow(unused_variables))]
    let target_triple = args
        .iter()
        .position(|arg| arg == "--target-triple")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let output_path = args
        .iter()
        .position(|arg| arg == "--output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);

    let ir_file_path = args
        .iter()
        .position(|arg| arg == "--ir-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);

    let ir_format = args
        .iter()
        .position(|arg| arg == "--ir-format")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str)
        .unwrap_or("json");

    // Read and parse IR.  Drop the raw buffer immediately after
    // deserialization to avoid holding two copies in memory simultaneously.
    let mut ir: SimpleIR = {
        if ir_format == "msgpack" {
            // msgpack binary format — deserialize directly via serde
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to open IR file '{}': {}", ir_path, e)))?;
                let reader = io::BufReader::new(file);
                match rmp_serde::from_read(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                let mut buf = Vec::new();
                io::stdin().read_to_end(&mut buf)?;
                match rmp_serde::from_slice::<SimpleIR>(&buf) {
                    Ok(ir) => { drop(buf); ir },
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if ir_format == "cbor" {
            // CBOR binary format — deserialize via ciborium
            #[cfg(not(feature = "cbor"))]
            {
                eprintln!("CBOR support requires the 'cbor' feature");
                std::process::exit(1);
            }
            #[cfg(feature = "cbor")]
            {
                if let Some(ir_path) = ir_file_path {
                    let file = std::fs::File::open(ir_path)
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to open IR file '{}': {}", ir_path, e)))?;
                    let reader = io::BufReader::new(file);
                    match ciborium::de::from_reader(reader) {
                        Ok(ir) => ir,
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    let mut buf = Vec::new();
                    io::stdin().read_to_end(&mut buf)?;
                    match ciborium::de::from_reader::<SimpleIR, _>(&buf[..]) {
                        Ok(ir) => { drop(buf); ir },
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        } else if let Some(ir_path) = ir_file_path {
            // Stream JSON directly from file — never holds raw JSON string in memory.
            let file = std::fs::File::open(ir_path)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to open IR file '{}': {}", ir_path, e)))?;
            let reader = io::BufReader::with_capacity(1 << 20, file);
            match serde_json::from_reader::<_, SimpleIR>(reader) {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        } else {
            // Stdin: read into string then deserialize directly (skips DOM intermediate).
            let mut buffer = String::new();
            io::stdin().read_to_string(&mut buffer)?;
            let result = serde_json::from_str::<SimpleIR>(&buffer);
            drop(buffer);
            match result {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        }
    };

    // Tree-shake for Luau: remove unreachable stdlib functions.
    if is_luau {
        ir.tree_shake_luau();
    }

    let default_output = if is_luau {
        "output.luau"
    } else if is_rust {
        "output.rs"
    } else if is_wasm {
        "output.wasm"
    } else {
        "output.o"
    };
    let output_file = output_path.unwrap_or(default_output);
    let mut file = File::create(output_file)?;

    if is_luau {
        #[cfg(feature = "luau-backend")]
        {
            let mut backend = LuauBackend::new();
            let source = if use_ir_pipeline {
                backend.compile_via_ir(&ir)
            } else {
                // Use unchecked compile for now — compile_checked rejects
                // unsupported ops from stdlib modules. Once more ops are
                // implemented, switch to compile_checked.
                backend.compile(&ir)
            };
            file.write_all(source.as_bytes())?;
            let lines = source.lines().count();
            eprintln!(
                "Successfully transpiled to {output_file} ({lines} lines, {:.1} KB)",
                source.len() as f64 / 1024.0
            );
        }
        #[cfg(not(feature = "luau-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without luau-backend support",
            ));
        }
    } else if is_rust {
        #[cfg(feature = "rust-backend")]
        {
            let mut backend = RustBackend::new();
            let source = backend.compile(&ir);
            file.write_all(source.as_bytes())?;
            println!("Successfully transpiled to {output_file}");
        }
        #[cfg(not(feature = "rust-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without rust-backend support",
            ));
        }
    } else if is_wasm {
        #[cfg(feature = "wasm-backend")]
        {
            let backend = WasmBackend::with_options(WasmCompileOptions::default());
            let wasm_bytes = backend.compile(ir);
            file.write_all(&wasm_bytes)?;
            println!("Successfully compiled to output.wasm");
        }
        #[cfg(not(feature = "wasm-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without wasm-backend support",
            ));
        }
    } else {
        #[cfg(feature = "native-backend")]
        {
            let func_count = ir.functions.len();
            let batch_size: usize = std::env::var("MOLT_BACKEND_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64);

            if func_count <= batch_size || batch_size == 0 {
                // Small IR: compile everything in one shot
                let backend = SimpleBackend::new_with_target(target_triple);
                let obj_bytes = backend.compile(ir);
                file.write_all(&obj_bytes)?;
                eprintln!(
                    "Successfully compiled to output.o ({func_count} functions)"
                );
            } else {
                // Large IR: split into batches, compile each independently,
                // then merge with ld -r (partial link).  This prevents OOM
                // when compiling 1000+ stdlib functions into one ObjectModule.
                let mut all_functions = ir.functions;
                let profile = ir.profile;
                let total_batches = (all_functions.len() + batch_size - 1) / batch_size;
                let mut batch_paths: Vec<std::path::PathBuf> = Vec::new();
                let tmp_dir = std::env::temp_dir().join(format!("molt_batch_{}", std::process::id()));
                let _ = std::fs::create_dir_all(&tmp_dir);

                let mut batch_idx = 0usize;
                // Pre-collect all function names for cross-batch import resolution.
                let all_func_names: std::collections::BTreeSet<String> =
                    all_functions.iter().map(|f| f.name.clone()).collect();

                while !all_functions.is_empty() {
                    let remaining = all_functions.len();
                    let take = remaining.min(batch_size);
                    // drain from the front to keep order
                    let batch_funcs: Vec<_> = all_functions.drain(..take).collect();
                    eprintln!(
                        "MOLT_BACKEND: batch {}/{total_batches} ({} functions)",
                        batch_idx + 1,
                        batch_funcs.len()
                    );
                    let batch_ir = SimpleIR {
                        functions: batch_funcs,
                        profile: profile.clone(),
                    };
                    let mut backend = SimpleBackend::new_with_target(target_triple);
                    // CRITICAL: skip IR-level passes (inline, dead func elim)
                    // for batched compilation — those were already run on the
                    // full IR above. Each batch only does Cranelift codegen.
                    backend.skip_ir_passes = true;
                    backend.external_function_names = all_func_names.clone();
                    let obj_bytes = backend.compile(batch_ir);

                    let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
                    std::fs::write(&batch_path, &obj_bytes)?;
                    batch_paths.push(batch_path);
                    batch_idx += 1;
                }

                // Merge batch objects with ld -r (relocatable partial link)
                drop(file); // close the output file handle
                if batch_paths.len() == 1 {
                    std::fs::copy(&batch_paths[0], output_file)?;
                } else {
                    // Use the system linker for partial linking.
                    // Respect CC/LD env vars for cross-compilation.
                    let ld_bin = std::env::var("LD")
                        .or_else(|_| std::env::var("CC"))
                        .unwrap_or_else(|_| "ld".to_string());
                    let mut cmd = std::process::Command::new(&ld_bin);
                    if ld_bin.contains("clang") || ld_bin.contains("gcc") {
                        // When using a compiler driver, pass -r via -Wl
                        cmd.arg("-Wl,-r").arg("-o").arg(output_file);
                    } else {
                        cmd.arg("-r").arg("-o").arg(output_file);
                    }
                    for p in &batch_paths {
                        cmd.arg(p);
                    }
                    let ld_result = cmd.output().map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::Other,
                            format!("failed to run ld -r for batch merge: {e}"),
                        )
                    })?;
                    if !ld_result.status.success() {
                        let stderr = String::from_utf8_lossy(&ld_result.stderr);
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("ld -r failed: {stderr}"),
                        ));
                    }
                }

                // Cleanup
                for p in &batch_paths {
                    let _ = std::fs::remove_file(p);
                }
                let _ = std::fs::remove_dir(&tmp_dir);
                eprintln!(
                    "Successfully compiled to output.o ({func_count} functions, {total_batches} batches)"
                );
            }
        }
        #[cfg(not(feature = "native-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without native-backend support",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonCache, DaemonJobRequest, DaemonRequest, DaemonResponse, compile_single_job,
        daemon_response_payload, read_daemon_request_bytes, write_cached_output,
    };
    use std::io::Cursor;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn daemon_cache_get_bytes_updates_lru_without_cloning() {
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1, 2, 3, 4]));

        let bytes = cache.get_bytes("module").expect("cache hit");
        assert_eq!(bytes, &[1, 2, 3, 4]);

        let entry = cache.entries.get("module").expect("entry retained");
        assert_eq!(entry.bytes.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(entry.stamp, cache.clock);
    }

    #[test]
    fn daemon_cache_can_share_bytes_across_keys() {
        let mut cache = DaemonCache::new(None);
        let shared = Arc::<[u8]>::from(vec![9, 8, 7, 6]);
        cache.insert("module".to_string(), Arc::clone(&shared));
        cache.insert("function".to_string(), shared);

        let module = cache.entries.get("module").expect("module entry");
        let function = cache.entries.get("function").expect("function entry");
        assert!(Arc::ptr_eq(&module.bytes, &function.bytes));
    }

    #[test]
    fn read_daemon_request_bytes_stops_at_protocol_newline() {
        let mut cursor = Cursor::new(b"{\"version\":1}\ntrailing".to_vec());
        let bytes = read_daemon_request_bytes(&mut cursor).expect("request bytes");
        assert_eq!(bytes, b"{\"version\":1}\n");
    }

    #[test]
    fn daemon_request_parse_applies_boolean_defaults() {
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(!job.wasm_link);
        assert!(!job.skip_module_output_if_synced);
        assert!(!job.skip_function_output_if_synced);
        assert!(!job.probe_cache_only);
        assert!(job.ir.is_none());
    }

    #[test]
    fn daemon_response_payload_omits_false_optional_fields() {
        let payload = daemon_response_payload(&DaemonResponse {
            ok: true,
            pong: false,
            jobs: vec![super::DaemonJobResponse {
                id: "job0".to_string(),
                ok: true,
                cached: false,
                cache_tier: None,
                output_written: true,
                needs_ir: false,
                message: None,
            }],
            error: None,
            health: None,
        })
        .expect("response payload");

        let text = String::from_utf8(payload).expect("utf8 json");
        assert!(!text.contains("\"needs_ir\""));
        assert!(!text.contains("\"health\""));
        assert!(!text.contains("\"error\""));
    }

    #[test]
    fn write_cached_output_can_skip_disk_write_when_synced() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-test-{nonce}.o"));

        let written = write_cached_output(output.to_str().expect("utf8 path"), b"artifact", true)
            .expect("cache hit succeeds");

        assert!(!written);
        assert!(!output.exists());
    }

    #[test]
    fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                output: "/tmp/unused.o".to_string(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(!result.cached);
        assert!(result.needs_ir);
        assert!(!result.output_written);
    }

    #[test]
    fn daemon_probe_cache_only_hits_without_ir() {
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-probe-hit-{nonce}.o"));

        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                output: output.to_string_lossy().into_owned(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(result.cached);
        assert!(!result.needs_ir);
        assert!(output.exists());
        let _ = std::fs::remove_file(output);
    }
}
