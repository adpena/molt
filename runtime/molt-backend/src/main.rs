use molt_lang_backend::luau::LuauBackend;
use molt_lang_backend::rust::RustBackend;
use molt_lang_backend::wasm::WasmBackend;
use molt_lang_backend::{
    SIMPLE_IR_CONTRACT_NAME, SIMPLE_IR_CONTRACT_VERSION, SimpleBackend, SimpleIR,
    validate_simple_ir,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
use std::io::Write;
use std::io::{self, Read};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
const BACKEND_DAEMON_DEFAULT_CACHE_MB: usize = 512;
const BACKEND_DAEMON_DEFAULT_REQUEST_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const BACKEND_DAEMON_DEFAULT_MAX_JOBS: usize = 8;

#[derive(Debug, Deserialize)]
struct DaemonJobRequest {
    id: String,
    is_wasm: bool,
    #[serde(default)]
    is_luau: bool,
    #[serde(default)]
    is_rust: bool,
    #[serde(default)]
    use_crate: bool,
    target_triple: Option<String>,
    output: String,
    cache_key: String,
    function_cache_key: Option<String>,
    env_overrides: Option<HashMap<String, String>>,
    ir: SimpleIR,
}

#[derive(Debug, Deserialize)]
struct DaemonRequest {
    version: Option<u32>,
    ping: Option<bool>,
    config_digest: Option<String>,
    jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug, Serialize)]
struct DaemonJobResponse {
    id: String,
    ok: bool,
    cached: bool,
    cache_tier: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct DaemonHealthResponse {
    protocol_version: u32,
    pid: u32,
    uptime_ms: u64,
    cache_entries: usize,
    cache_bytes: usize,
    cache_max_bytes: usize,
    request_limit_bytes: usize,
    max_jobs: usize,
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

#[derive(Debug, Serialize)]
struct DaemonResponse {
    ok: bool,
    pong: bool,
    jobs: Vec<DaemonJobResponse>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<DaemonHealthResponse>,
}

#[derive(Default)]
struct DaemonStats {
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

struct DaemonCache {
    entries: HashMap<String, CacheEntry>,
    order: BinaryHeap<Reverse<(u64, String)>>,
    clock: u64,
    bytes: usize,
    max_bytes: usize,
}

struct CacheEntry {
    bytes: Vec<u8>,
    stamp: u64,
}

impl DaemonCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: BinaryHeap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
        }
    }

    fn touch(&mut self, key: &str) -> Option<u64> {
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        entry.stamp = stamp;
        self.order.push(Reverse((stamp, key.to_string())));
        Some(stamp)
    }

    fn get_cloned(&mut self, key: &str) -> Option<Vec<u8>> {
        let value = self.entries.get(key).map(|entry| entry.bytes.clone())?;
        let _ = self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: String, value: Vec<u8>) {
        if key.is_empty() {
            return;
        }
        if let Some(prev) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(prev.bytes.len());
        }
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        self.bytes = self.bytes.saturating_add(value.len());
        self.order.push(Reverse((stamp, key.clone())));
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
        while self.bytes > self.max_bytes {
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
                compacted.push(Reverse((entry.stamp, key.clone())));
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

fn daemon_cache_limit_bytes() -> usize {
    daemon_parse_positive_usize(
        "MOLT_BACKEND_DAEMON_CACHE_MB",
        BACKEND_DAEMON_DEFAULT_CACHE_MB,
    )
    .saturating_mul(1024 * 1024)
}

fn daemon_request_limit_bytes() -> usize {
    daemon_parse_positive_usize(
        "MOLT_BACKEND_DAEMON_MAX_REQUEST_BYTES",
        BACKEND_DAEMON_DEFAULT_REQUEST_LIMIT_BYTES,
    )
}

fn daemon_max_jobs() -> usize {
    daemon_parse_positive_usize(
        "MOLT_BACKEND_DAEMON_MAX_JOBS",
        BACKEND_DAEMON_DEFAULT_MAX_JOBS,
    )
}

fn daemon_parse_positive_usize(var: &str, default: usize) -> usize {
    let raw = env::var(var).unwrap_or_else(|_| default.to_string());
    match raw.trim().parse::<usize>() {
        Ok(value) if value > 0 => value,
        _ => default,
    }
}

fn daemon_health(
    cache: &DaemonCache,
    stats: &DaemonStats,
    start: Instant,
    request_limit_bytes: usize,
    max_jobs: usize,
) -> DaemonHealthResponse {
    let uptime_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    DaemonHealthResponse {
        protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
        pid: std::process::id(),
        uptime_ms,
        cache_entries: cache.entries.len(),
        cache_bytes: cache.bytes,
        cache_max_bytes: cache.max_bytes,
        request_limit_bytes,
        max_jobs,
        requests_total: stats.requests_total,
        jobs_total: stats.jobs_total,
        cache_hits: stats.cache_hits,
        cache_misses: stats.cache_misses,
    }
}

fn ir_contract_missing_allowed() -> bool {
    matches!(
        env::var("MOLT_IR_CONTRACT_ALLOW_MISSING")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn validate_ir_contract_object(ir: &serde_json::Value) -> Result<(), String> {
    let name_field = ir.get("ir_contract_name");
    let version_field = ir.get("ir_contract_version");
    if name_field.is_none() && version_field.is_none() {
        if ir_contract_missing_allowed() {
            return Ok(());
        }
        return Err(format!(
            "missing `ir_contract_name`/`ir_contract_version` (expected `{}` v{})",
            SIMPLE_IR_CONTRACT_NAME, SIMPLE_IR_CONTRACT_VERSION
        ));
    }
    let Some(name) = name_field.and_then(serde_json::Value::as_str) else {
        return Err("`ir_contract_name` must be a string".to_string());
    };
    if name != SIMPLE_IR_CONTRACT_NAME {
        return Err(format!(
            "unsupported `ir_contract_name` `{}` (expected `{}`)",
            name, SIMPLE_IR_CONTRACT_NAME
        ));
    }
    let Some(version) = version_field.and_then(serde_json::Value::as_u64) else {
        return Err("`ir_contract_version` must be an integer".to_string());
    };
    if version != SIMPLE_IR_CONTRACT_VERSION as u64 {
        return Err(format!(
            "unsupported `ir_contract_version` {} (expected {})",
            version, SIMPLE_IR_CONTRACT_VERSION
        ));
    }
    Ok(())
}

fn validate_daemon_request_ir_contract(request: &serde_json::Value) -> Result<(), String> {
    let Some(jobs) = request.get("jobs").and_then(serde_json::Value::as_array) else {
        return Ok(());
    };
    for (idx, job) in jobs.iter().enumerate() {
        let Some(ir) = job.get("ir") else {
            continue;
        };
        validate_ir_contract_object(ir).map_err(|err| format!("jobs[{idx}].ir: {err}"))?;
    }
    Ok(())
}

fn compile_single_job(job: DaemonJobRequest, cache: &mut DaemonCache) -> DaemonJobResponse {
    let cache_key = job.cache_key.trim().to_string();
    let function_cache_key = job
        .function_cache_key
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if !cache_key.is_empty()
        && let Some(bytes) = cache.get_cloned(&cache_key)
    {
        match write_output(&job.output, &bytes) {
            Ok(()) => {
                return DaemonJobResponse {
                    id: job.id,
                    ok: true,
                    cached: true,
                    cache_tier: Some("module".to_string()),
                    message: None,
                };
            }
            Err(err) => {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    message: Some(format!("failed to write cached output: {err}")),
                };
            }
        }
    }
    if !function_cache_key.is_empty()
        && function_cache_key != cache_key
        && let Some(bytes) = cache.get_cloned(&function_cache_key)
    {
        match write_output(&job.output, &bytes) {
            Ok(()) => {
                return DaemonJobResponse {
                    id: job.id,
                    ok: true,
                    cached: true,
                    cache_tier: Some("function".to_string()),
                    message: None,
                };
            }
            Err(err) => {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    message: Some(format!("failed to write cached output: {err}")),
                };
            }
        }
    }

    if let Err(err) = validate_simple_ir(&job.ir) {
        return DaemonJobResponse {
            id: job.id,
            ok: false,
            cached: false,
            cache_tier: None,
            message: Some(err),
        };
    }

    let _env_guard = DaemonEnvOverridesGuard::apply(job.env_overrides.as_ref());
    if job.is_luau {
        let job_id = job.id.clone();
        let output_path = job.output.clone();
        let mut ir = job.ir;
        ir.tree_shake_luau();
        let mut backend = LuauBackend::new();
        let luau_source = match backend.compile_checked(&ir) {
            Ok(source) => source,
            Err(err) => {
                return DaemonJobResponse {
                    id: job_id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    message: Some(err),
                };
            }
        };
        if let Err(err) = write_output(&output_path, luau_source.as_bytes()) {
            return DaemonJobResponse {
                id: job_id,
                ok: false,
                cached: false,
                cache_tier: None,
                message: Some(format!("failed to write output: {err}")),
            };
        }
        return DaemonJobResponse {
            id: job_id,
            ok: true,
            cached: false,
            cache_tier: None,
            message: None,
        };
    }

    if job.is_rust {
        let job_id = job.id.clone();
        let output_path = job.output.clone();
        let ir = job.ir;
        let mut backend = if job.use_crate { RustBackend::new_with_crate() } else { RustBackend::new() };
        let rust_source = backend.compile(&ir);
        if let Err(err) = write_output(&output_path, rust_source.as_bytes()) {
            return DaemonJobResponse {
                id: job_id,
                ok: false,
                cached: false,
                cache_tier: None,
                message: Some(format!("failed to write output: {err}")),
            };
        }
        return DaemonJobResponse {
            id: job_id,
            ok: true,
            cached: false,
            cache_tier: None,
            message: None,
        };
    }

    let output_bytes = if job.is_wasm {
        let backend = WasmBackend::new();
        backend.compile(job.ir)
    } else {
        let backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
        backend.compile(job.ir)
    };

    if let Err(err) = write_output(&job.output, &output_bytes) {
        return DaemonJobResponse {
            id: job.id,
            ok: false,
            cached: false,
            cache_tier: None,
            message: Some(format!("failed to write compiled output: {err}")),
        };
    }

    if !cache_key.is_empty() {
        cache.insert(cache_key.clone(), output_bytes.clone());
    }
    if !function_cache_key.is_empty() && function_cache_key != cache_key {
        cache.insert(function_cache_key, output_bytes.clone());
    }

    DaemonJobResponse {
        id: job.id,
        ok: true,
        cached: false,
        cache_tier: None,
        message: None,
    }
}

struct DaemonEnvOverridesGuard {
    saved: Vec<(String, Option<String>)>,
}

impl DaemonEnvOverridesGuard {
    fn apply(overrides: Option<&HashMap<String, String>>) -> Self {
        let mut saved: Vec<(String, Option<String>)> = Vec::new();
        if let Some(overrides) = overrides {
            for (key, value) in overrides {
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                saved.push((key.to_string(), env::var(key).ok()));
                unsafe {
                    env::set_var(key, value);
                }
            }
        }
        Self { saved }
    }
}

impl Drop for DaemonEnvOverridesGuard {
    fn drop(&mut self) {
        for (key, previous) in self.saved.drain(..).rev() {
            match previous {
                Some(value) => unsafe {
                    env::set_var(&key, value);
                },
                None => unsafe {
                    env::remove_var(&key);
                },
            }
        }
    }
}

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
    file.sync_all()?;
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
    let mut cache = DaemonCache::new(daemon_cache_limit_bytes());
    let mut stats = DaemonStats::default();
    let mut active_config_digest: Option<String> = None;
    let request_limit_bytes = daemon_request_limit_bytes();
    let max_jobs = daemon_max_jobs();
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
                    request_limit_bytes,
                    max_jobs,
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
    request_limit_bytes: usize,
    max_jobs: usize,
) -> io::Result<()> {
    let mut raw_bytes = Vec::new();
    let read_limit = (request_limit_bytes as u64).saturating_add(1);
    stream.take(read_limit).read_to_end(&mut raw_bytes)?;
    stats.requests_total = stats.requests_total.saturating_add(1);
    if raw_bytes.len() > request_limit_bytes {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some(format!(
                "request too large: {} bytes (max {request_limit_bytes})",
                raw_bytes.len()
            )),
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    let raw = String::from_utf8_lossy(&raw_bytes);
    if raw.trim().is_empty() {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some("empty request".to_string()),
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    let req_json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(req) => req,
        Err(err) => {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!("invalid request JSON: {err}")),
                health: Some(daemon_health(
                    cache,
                    stats,
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )),
            };
            write_daemon_response(stream, &response)?;
            return Ok(());
        }
    };
    if let Err(err) = validate_daemon_request_ir_contract(&req_json) {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some(format!("invalid IR contract: {err}")),
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    let req: DaemonRequest = match serde_json::from_value(req_json) {
        Ok(req) => req,
        Err(err) => {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!("invalid request payload: {err}")),
                health: Some(daemon_health(
                    cache,
                    stats,
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )),
            };
            write_daemon_response(stream, &response)?;
            return Ok(());
        }
    };
    let version = req.version.unwrap_or(0);
    if version != BACKEND_DAEMON_PROTOCOL_VERSION {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some(format!(
                "unsupported protocol version {version}; expected {BACKEND_DAEMON_PROTOCOL_VERSION}"
            )),
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
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
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
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
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    };
    if jobs.is_empty() {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some("empty jobs in request".to_string()),
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    if jobs.len() > max_jobs {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some(format!(
                "too many jobs in request: {} (max {max_jobs})",
                jobs.len()
            )),
            health: Some(daemon_health(
                cache,
                stats,
                started_at,
                request_limit_bytes,
                max_jobs,
            )),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
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
        health: Some(daemon_health(
            cache,
            stats,
            started_at,
            request_limit_bytes,
            max_jobs,
        )),
    };
    write_daemon_response(stream, &response)?;
    Ok(())
}

#[cfg(unix)]
fn write_daemon_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &DaemonResponse,
) -> io::Result<()> {
    let payload = serde_json::to_vec(response).map_err(io::Error::other)?;
    stream.write_all(&payload)?;
    Ok(())
}

#[cfg(not(unix))]
fn run_daemon(_socket_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon mode requires unix domain sockets",
    ))
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--daemon") {
        let socket_path = args
            .iter()
            .position(|arg| arg == "--socket")
            .and_then(|idx| args.get(idx + 1))
            .map(|value| value.as_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))?;
        return run_daemon(socket_path);
    }
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());
    let is_luau = args.contains(&"--target".to_string()) && args.contains(&"luau".to_string());
    let is_rust = args.contains(&"--target".to_string()) && args.contains(&"rust".to_string());
    let use_crate = args.contains(&"--use-crate".to_string());
    let target_triple = args
        .iter()
        .position(|arg| arg == "--target-triple")
        .and_then(|idx| args.get(idx + 1))
        .map(|value| value.as_str());
    let output_path = args
        .iter()
        .position(|arg| arg == "--output")
        .and_then(|idx| args.get(idx + 1))
        .map(|value| value.as_str());

    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    let ir_value: serde_json::Value = match serde_json::from_str(&buffer) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("Invalid IR JSON: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = validate_ir_contract_object(&ir_value) {
        eprintln!("Invalid IR contract: {err}");
        std::process::exit(1);
    }

    let mut deserializer = serde_json::Deserializer::from_str(&buffer);
    let mut ir: SimpleIR = match serde_path_to_error::deserialize(&mut deserializer) {
        Ok(ir) => ir,
        Err(err) => {
            let path = err.path().to_string();
            let inner = err.into_inner();
            eprintln!("Invalid IR JSON at {path}: {inner}");
            std::process::exit(1);
        }
    };
    if let Err(err) = validate_simple_ir(&ir) {
        eprintln!("{err}");
        std::process::exit(1);
    }

    let output_file = output_path.unwrap_or(if is_luau {
        "output.luau"
    } else if is_rust {
        "output.rs"
    } else if is_wasm {
        "output.wasm"
    } else {
        "output.o"
    });
    let mut file = File::create(output_file)?;

    if is_rust {
        let mut backend = if use_crate { RustBackend::new_with_crate() } else { RustBackend::new() };
        let rust_source = backend.compile(&ir);
        file.write_all(rust_source.as_bytes())?;
        println!("Successfully compiled to {output_file}");
    } else if is_luau {
        // Dump IR before tree shaking if MOLT_DUMP_IR is set
        if let Ok(filter) = std::env::var("MOLT_DUMP_IR") {
            for func in &ir.functions {
                if filter == "1" || filter == "all" || func.name.contains(&filter) {
                    eprintln!("=== IR [pre-shake] {} ({} ops) ===", func.name, func.ops.len());
                    for (i, op) in func.ops.iter().enumerate() {
                        eprintln!("  [{i:4}] kind={:<24} out={:<12} s_value={:<30} args={:?}",
                            op.kind,
                            op.out.as_deref().unwrap_or("-"),
                            op.s_value.as_deref().unwrap_or("-"),
                            op.args.as_ref().map(|a| a.join(", ")).unwrap_or_default());
                    }
                }
            }
        }
        ir.tree_shake_luau();
        // Dump IR after tree shaking if MOLT_DUMP_IR is set
        if let Ok(filter) = std::env::var("MOLT_DUMP_IR") {
            for func in &ir.functions {
                if filter == "1" || filter == "all" || func.name.contains(&filter) {
                    eprintln!("=== IR [post-shake] {} ({} ops) ===", func.name, func.ops.len());
                    for (i, op) in func.ops.iter().enumerate() {
                        if op.kind == "nop" { continue; }
                        eprintln!("  [{i:4}] kind={:<24} out={:<12} s_value={:<30} args={:?}",
                            op.kind,
                            op.out.as_deref().unwrap_or("-"),
                            op.s_value.as_deref().unwrap_or("-"),
                            op.args.as_ref().map(|a| a.join(", ")).unwrap_or_default());
                    }
                }
            }
        }
        let mut backend = LuauBackend::new();
        let luau_source = match backend.compile_checked(&ir) {
            Ok(source) => source,
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(1);
            }
        };
        file.write_all(luau_source.as_bytes())?;
        println!("Successfully compiled to {output_file}");
    } else if is_wasm {
        let backend = WasmBackend::new();
        let wasm_bytes = backend.compile(ir);
        file.write_all(&wasm_bytes)?;
        println!("Successfully compiled to output.wasm");
    } else {
        let backend = SimpleBackend::new_with_target(target_triple);
        let obj_bytes = backend.compile(ir);
        file.write_all(&obj_bytes)?;
        println!("Successfully compiled to output.o");
    }

    Ok(())
}
