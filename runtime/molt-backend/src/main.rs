use molt_backend::wasm::WasmBackend;
use molt_backend::{SimpleBackend, SimpleIR};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs::File;
use std::io::Write;
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct DaemonJobRequest {
    id: String,
    is_wasm: bool,
    target_triple: Option<String>,
    output: String,
    cache_key: String,
    ir: SimpleIR,
}

#[derive(Debug, Deserialize)]
struct DaemonRequest {
    version: Option<u32>,
    ping: Option<bool>,
    jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug, Serialize)]
struct DaemonJobResponse {
    id: String,
    ok: bool,
    cached: bool,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct DaemonResponse {
    ok: bool,
    pong: bool,
    jobs: Vec<DaemonJobResponse>,
    error: Option<String>,
}

struct DaemonCache {
    entries: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
    bytes: usize,
    max_bytes: usize,
}

impl DaemonCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
        }
    }

    fn get_cloned(&mut self, key: &str) -> Option<Vec<u8>> {
        let value = self.entries.get(key).cloned();
        if value.is_some() {
            self.touch(key);
        }
        value
    }

    fn insert(&mut self, key: String, value: Vec<u8>) {
        if key.is_empty() {
            return;
        }
        if let Some(prev) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(prev.len());
        }
        self.order.retain(|queued| queued != &key);
        self.bytes = self.bytes.saturating_add(value.len());
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
        self.evict();
    }

    fn touch(&mut self, key: &str) {
        self.order.retain(|queued| queued != key);
        self.order.push_back(key.to_string());
    }

    fn evict(&mut self) {
        while self.bytes > self.max_bytes {
            let Some(old_key) = self.order.pop_front() else {
                break;
            };
            if let Some(old_val) = self.entries.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(old_val.len());
            }
        }
    }
}

fn daemon_cache_limit_bytes() -> usize {
    let raw = env::var("MOLT_BACKEND_DAEMON_CACHE_MB").unwrap_or_else(|_| "512".to_string());
    match raw.trim().parse::<usize>() {
        Ok(mb) if mb > 0 => mb.saturating_mul(1024 * 1024),
        _ => 512 * 1024 * 1024,
    }
}

fn compile_single_job(job: DaemonJobRequest, cache: &mut DaemonCache) -> DaemonJobResponse {
    let cache_key = job.cache_key.trim().to_string();
    if !cache_key.is_empty() {
        if let Some(bytes) = cache.get_cloned(&cache_key) {
            match write_output(&job.output, &bytes) {
                Ok(()) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        message: None,
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        message: Some(format!("failed to write cached output: {err}")),
                    };
                }
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
            message: Some(format!("failed to write compiled output: {err}")),
        };
    }

    if !cache_key.is_empty() {
        cache.insert(cache_key, output_bytes);
    }

    DaemonJobResponse {
        id: job.id,
        ok: true,
        cached: false,
        message: None,
    }
}

fn write_output(path: &str, bytes: &[u8]) -> io::Result<()> {
    let output_path = Path::new(path);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
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
    if let Some(parent) = socket.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let listener = UnixListener::bind(socket)?;
    let mut cache = DaemonCache::new(daemon_cache_limit_bytes());
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(err) = handle_daemon_connection(&mut conn, &mut cache) {
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
) -> io::Result<()> {
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    if raw.trim().is_empty() {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some("empty request".to_string()),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    let req: DaemonRequest = match serde_json::from_str(&raw) {
        Ok(req) => req,
        Err(err) => {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!("invalid request JSON: {err}")),
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
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    }
    let Some(jobs) = req.jobs else {
        let response = DaemonResponse {
            ok: false,
            pong: false,
            jobs: Vec::new(),
            error: Some("missing jobs in request".to_string()),
        };
        write_daemon_response(stream, &response)?;
        return Ok(());
    };
    let mut results = Vec::with_capacity(jobs.len());
    for job in jobs {
        results.push(compile_single_job(job, cache));
    }
    let all_ok = results.iter().all(|job| job.ok);
    let response = DaemonResponse {
        ok: all_ok,
        pong: false,
        jobs: results,
        error: None,
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

    let output_file = output_path.unwrap_or(if is_wasm { "output.wasm" } else { "output.o" });
    let mut file = File::create(output_file)?;

    if is_wasm {
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
