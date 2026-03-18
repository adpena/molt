use molt_backend::rust::RustBackend;
use molt_backend::wasm::WasmBackend;
use molt_backend::{SimpleBackend, SimpleIR};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
use std::io::Write;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
#[derive(Debug, Deserialize)]
struct DaemonJobRequest {
    id: String,
    is_wasm: bool,
    target_triple: Option<String>,
    output: String,
    cache_key: String,
    function_cache_key: Option<String>,
    #[serde(default)]
    skip_module_output_if_synced: bool,
    #[serde(default)]
    skip_function_output_if_synced: bool,
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
    output_written: bool,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct DaemonHealthResponse {
    protocol_version: u32,
    pid: u32,
    uptime_ms: u64,
    cache_entries: usize,
    cache_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_limit_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_jobs: Option<usize>,
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
    entries: HashMap<Arc<str>, CacheEntry>,
    order: BinaryHeap<Reverse<(u64, Arc<str>)>>,
    clock: u64,
    bytes: usize,
    max_bytes: Option<usize>,
}

struct CacheEntry {
    bytes: Vec<u8>,
    stamp: u64,
}

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
        Some(entry.bytes.as_slice())
    }

    fn insert(&mut self, key: String, value: Vec<u8>) {
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
        while self.max_bytes.is_some_and(|max_bytes| self.bytes > max_bytes) {
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

fn daemon_health(
    cache: &DaemonCache,
    stats: &DaemonStats,
    start: Instant,
) -> DaemonHealthResponse {
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

fn compile_single_job(job: DaemonJobRequest, cache: &mut DaemonCache) -> DaemonJobResponse {
    let cache_key = job.cache_key.trim().to_string();
    let function_cache_key = job
        .function_cache_key
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if !cache_key.is_empty()
        && let Some(bytes) = cache.get_bytes(&cache_key)
    {
        match write_cached_output(&job.output, bytes, job.skip_module_output_if_synced) {
            Ok(output_written) => {
                return DaemonJobResponse {
                    id: job.id,
                    ok: true,
                    cached: true,
                    cache_tier: Some("module".to_string()),
                    output_written,
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
                    message: Some(format!("failed to write cached output: {err}")),
                };
            }
        }
    }
    if !function_cache_key.is_empty()
        && function_cache_key != cache_key
        && let Some(bytes) = cache.get_bytes(&function_cache_key)
    {
        match write_cached_output(&job.output, bytes, job.skip_function_output_if_synced) {
            Ok(output_written) => {
                return DaemonJobResponse {
                    id: job.id,
                    ok: true,
                    cached: true,
                    cache_tier: Some("function".to_string()),
                    output_written,
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
                    message: Some(format!("failed to write cached output: {err}")),
                };
            }
        }
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
            output_written: false,
            message: Some(format!("failed to write compiled output: {err}")),
        };
    }

    if !cache_key.is_empty() && !function_cache_key.is_empty() && function_cache_key != cache_key
    {
        cache.insert(cache_key, output_bytes.clone());
        cache.insert(function_cache_key, output_bytes);
    } else if !cache_key.is_empty() {
        cache.insert(cache_key, output_bytes);
    } else if !function_cache_key.is_empty() {
        cache.insert(function_cache_key, output_bytes);
    }

    DaemonJobResponse {
        id: job.id,
        ok: true,
        cached: false,
        cache_tier: None,
        output_written: true,
        message: None,
    }
}

fn write_cached_output(path: &str, bytes: &[u8], skip_if_synced: bool) -> io::Result<bool> {
    if skip_if_synced {
        return Ok(false);
    }
    write_output(path, bytes)?;
    Ok(true)
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
    let mut raw_bytes = Vec::new();
    stream.read_to_end(&mut raw_bytes)?;
    stats.requests_total = stats.requests_total.saturating_add(1);
    if raw_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some("empty request".to_string()),
            health: Some(daemon_health(cache, stats, started_at)),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    let req: DaemonRequest = match serde_json::from_slice(&raw_bytes) {
        Ok(req) => req,
        Err(err) => {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!("invalid request JSON: {err}")),
                health: Some(daemon_health(cache, stats, started_at)),
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
            health: Some(daemon_health(cache, stats, started_at)),
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
            health: Some(daemon_health(cache, stats, started_at)),
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
            health: Some(daemon_health(cache, stats, started_at)),
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
            health: Some(daemon_health(cache, stats, started_at)),
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
        health: Some(daemon_health(cache, stats, started_at)),
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
            .map(String::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))?;
        return run_daemon(socket_path);
    }
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());
    let is_rust = args.contains(&"--target".to_string()) && args.contains(&"rust".to_string());
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

    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let mut deserializer = serde_json::Deserializer::from_str(&buffer);
    let ir: SimpleIR = match serde_path_to_error::deserialize(&mut deserializer) {
        Ok(ir) => ir,
        Err(err) => {
            let path = err.path().to_string();
            let inner = err.into_inner();
            eprintln!("Invalid IR JSON at {path}: {inner}");
            std::process::exit(1);
        }
    };

    let default_output = if is_rust {
        "output.rs"
    } else if is_wasm {
        "output.wasm"
    } else {
        "output.o"
    };
    let output_file = output_path.unwrap_or(default_output);
    let mut file = File::create(output_file)?;

    if is_rust {
        let mut backend = RustBackend::new();
        let source = backend.compile(&ir);
        file.write_all(source.as_bytes())?;
        println!("Successfully transpiled to {output_file}");
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

#[cfg(test)]
mod tests {
    use super::{write_cached_output, DaemonCache};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn daemon_cache_get_bytes_updates_lru_without_cloning() {
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), vec![1, 2, 3, 4]);

        let bytes = cache.get_bytes("module").expect("cache hit");
        assert_eq!(bytes, &[1, 2, 3, 4]);

        let entry = cache.entries.get("module").expect("entry retained");
        assert_eq!(entry.bytes, vec![1, 2, 3, 4]);
        assert_eq!(entry.stamp, cache.clock);
    }

    #[test]
    fn write_cached_output_can_skip_disk_write_when_synced() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-test-{nonce}.o"));

        let written =
            write_cached_output(output.to_str().expect("utf8 path"), b"artifact", true)
                .expect("cache hit succeeds");

        assert!(!written);
        assert!(!output.exists());
    }
}
