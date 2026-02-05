use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as Base64Engine;
use num_format::{Grouping, SystemLocale};
use rmpv::encode::write_value;
use rmpv::Value as MsgpackValue;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message};
use url::Url;
use wasmtime::{
    Cache, Caller, Config, Engine, Extern, ExternType, Func, FuncType, Linker, Memory, MemoryType,
    Module, OptLevel, Ref, Store, Table, TableType, Val,
};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{p1, DirPerms, FilePerms, WasiCtxBuilder};

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};

#[derive(Clone, Copy, Debug)]
struct Limits {
    min: u32,
    max: Option<u32>,
}

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const INT_MASK: u64 = (1 << 47) - 1;
const MAX_DB_FRAME_SIZE: usize = 64 * 1024 * 1024;
const CANCEL_POLL_MS: u64 = 10;
const IO_EVENT_READ: u32 = 1;
const IO_EVENT_WRITE: u32 = 1 << 1;
const IO_EVENT_ERROR: u32 = 1 << 2;
const PROCESS_STDIO_PIPE: i32 = 1;
const PROCESS_STDIO_DEVNULL: i32 = 2;
const PROCESS_STDIO_STDOUT_REDIRECT: i32 = -2;
const PROCESS_STDIO_FD_BASE: i32 = 1 << 30;
const PROCESS_STDIO_STDOUT: i32 = 1;
const PROCESS_STDIO_STDERR: i32 = 2;
type AddrInfoEntry = (i32, i32, i32, Vec<u8>, Vec<u8>);

fn debug_log<F: FnOnce() -> String>(message: F) {
    if env::var("MOLT_WASM_HOST_DEBUG").is_ok() {
        eprintln!("[molt-wasm-host] {}", message());
    }
}

fn precompiled_enabled() -> bool {
    matches!(env::var("MOLT_WASM_PRECOMPILED").as_deref(), Ok("1"))
}

fn precompiled_write_enabled() -> bool {
    matches!(env::var("MOLT_WASM_PRECOMPILED_WRITE").as_deref(), Ok("1"))
}

fn resolve_precompiled_path(wasm_path: &Path, override_env: &str) -> Option<PathBuf> {
    if !precompiled_enabled() {
        return None;
    }
    if let Ok(path) = env::var(override_env) {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    Some(wasm_path.with_extension("cwasm"))
}

fn load_or_compile_module(
    engine: &Engine,
    wasm_path: &Path,
    label: &str,
    override_env: &str,
) -> Result<Module> {
    if let Some(precompiled) = resolve_precompiled_path(wasm_path, override_env) {
        if precompiled.exists() {
            debug_log(|| format!("loading {label} precompiled: {precompiled:?}"));
            match unsafe { Module::deserialize_file(engine, &precompiled) } {
                Ok(module) => return Ok(module),
                Err(err) => {
                    debug_log(|| format!("precompiled load failed ({label}): {err}"));
                }
            }
        }
    }
    let read_start = Instant::now();
    let wasm_bytes = fs::read(wasm_path).with_context(|| format!("read {label} {wasm_path:?}"))?;
    debug_log(|| format!("read {label} wasm in {:?}", read_start.elapsed()));
    let compile_start = Instant::now();
    let module = Module::new(engine, wasm_bytes)
        .with_context(|| format!("compile {label} {wasm_path:?}"))?;
    debug_log(|| format!("compiled {label} module in {:?}", compile_start.elapsed()));
    if precompiled_write_enabled() {
        if let Some(precompiled) = resolve_precompiled_path(wasm_path, override_env) {
            match module.serialize() {
                Ok(bytes) => {
                    let _ = fs::write(&precompiled, bytes);
                    debug_log(|| format!("wrote {label} precompiled: {precompiled:?}"));
                }
                Err(err) => {
                    debug_log(|| format!("serialize {label} failed: {err}"));
                }
            }
        }
    }
    Ok(module)
}

fn build_engine() -> Result<Engine> {
    let mut config = Config::new();
    let cache_toggle = env::var("MOLT_WASM_CACHE").ok();
    let max_stack = env::var("MOLT_WASM_MAX_STACK")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|val| *val > 0)
        .unwrap_or(8 * 1024 * 1024);
    config.max_wasm_stack(max_stack);
    debug_log(|| format!("wasmtime max_wasm_stack set to {max_stack}"));
    if cache_toggle.as_deref() != Some("0") {
        let cache_path = env::var("MOLT_WASM_CACHE_CONFIG").ok();
        if cache_toggle.as_deref() == Some("1") || cache_path.is_some() {
            let cache = match cache_path.as_deref() {
                Some(path) => {
                    debug_log(|| format!("wasmtime cache config: {path}"));
                    Cache::from_file(Some(Path::new(path)))?
                }
                None => {
                    debug_log(|| "wasmtime cache config: default".to_string());
                    Cache::from_file(None)?
                }
            };
            config.cache(Some(cache));
            debug_log(|| "wasmtime cache enabled".to_string());
        }
    }
    if matches!(env::var("MOLT_WASM_COMPILE_SERIAL").as_deref(), Ok("1")) {
        config.parallel_compilation(false);
        debug_log(|| "wasmtime parallel compilation disabled".to_string());
    }
    if matches!(env::var("MOLT_WASM_COMPILE_FAST").as_deref(), Ok("1")) {
        config.cranelift_opt_level(OptLevel::None);
        debug_log(|| "wasmtime opt level set to none".to_string());
    }
    Ok(Engine::new(&config)?)
}

struct HostState {
    wasi: WasiP1Ctx,
    memory: Option<Memory>,
    call_indirect: Arc<Mutex<HashMap<String, Option<Func>>>>,
    db_worker: Option<DbWorker>,
    db_pending: HashMap<u64, PendingDbRequest>,
    last_cancel_check: Option<Instant>,
    socket_manager: SocketManager,
    ws_manager: WebSocketManager,
    process_manager: ProcessManager,
}

struct SocketManager {
    next_id: u64,
    sockets: HashMap<u64, Socket>,
}

impl SocketManager {
    fn new() -> Self {
        Self {
            next_id: 1,
            sockets: HashMap::new(),
        }
    }

    fn insert(&mut self, socket: Socket) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sockets.insert(id, socket);
        id
    }

    fn remove(&mut self, id: u64) -> Option<Socket> {
        self.sockets.remove(&id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Socket> {
        self.sockets.get_mut(&id)
    }
}

impl WebSocketManager {
    fn new() -> Self {
        Self {
            next_id: 1,
            sockets: HashMap::new(),
        }
    }

    fn insert(&mut self, socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sockets.insert(
            id,
            WebSocketEntry {
                socket,
                queue: VecDeque::new(),
                closed: false,
            },
        );
        id
    }

    fn remove(&mut self, id: u64) -> Option<WebSocketEntry> {
        self.sockets.remove(&id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut WebSocketEntry> {
        self.sockets.get_mut(&id)
    }
}

struct WebSocketManager {
    next_id: u64,
    sockets: HashMap<u64, WebSocketEntry>,
}

struct WebSocketEntry {
    socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    queue: VecDeque<Vec<u8>>,
    closed: bool,
}

struct ProcessManager {
    next_id: u64,
    processes: HashMap<u64, ProcessEntry>,
    events_tx: mpsc::Sender<ProcessEvent>,
    events_rx: mpsc::Receiver<ProcessEvent>,
}

struct ProcessEntry {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_stream: Option<u64>,
    stderr_stream: Option<u64>,
    exit_code: Option<i32>,
}

enum ProcessEvent {
    Stdout(u64, Vec<u8>),
    Stderr(u64, Vec<u8>),
    StdoutClosed(u64),
    StderrClosed(u64),
}

enum ProcessStreamKind {
    Stdout,
    Stderr,
}

impl ProcessManager {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            next_id: 1,
            processes: HashMap::new(),
            events_tx: tx,
            events_rx: rx,
        }
    }

    fn alloc_handle(&mut self, pid: u32) -> u64 {
        let handle = if pid != 0 { pid as u64 } else { self.next_id };
        if pid == 0 {
            self.next_id = self.next_id.saturating_add(1);
        }
        handle
    }
}

fn resolve_wasm_path(arg: Option<String>) -> Result<PathBuf> {
    let env_path = env::var("MOLT_WASM_PATH").ok();
    let local = PathBuf::from("output.wasm");
    let temp = env::temp_dir().join("output.wasm");
    let candidates = [arg, env_path]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    if local.exists() {
        return Ok(local);
    }
    if temp.exists() {
        return Ok(temp);
    }
    bail!("WASM path not found (arg, MOLT_WASM_PATH, ./output.wasm, or temp output.wasm)");
}

fn resolve_linked_path(wasm_path: &Path) -> Option<PathBuf> {
    if let Ok(path) = env::var("MOLT_WASM_LINKED_PATH") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    if let Some(stem) = wasm_path.file_stem().and_then(|s| s.to_str()) {
        let ext = wasm_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("wasm");
        let sibling = wasm_path.with_file_name(format!("{stem}_linked.{ext}"));
        if sibling.exists() {
            return Some(sibling);
        }
    }
    let default_linked = PathBuf::from("output_linked.wasm");
    if default_linked.exists() {
        return Some(default_linked);
    }
    None
}

fn prefer_linked() -> bool {
    match env::var("MOLT_WASM_PREFER_LINKED") {
        Ok(val) => !matches!(val.to_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

fn force_linked() -> bool {
    matches!(env::var("MOLT_WASM_LINKED").as_deref(), Ok("1"))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_env = env::var("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_exports_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("MOLT_WASM_DB_EXPORTS").or_else(|_| env::var("MOLT_WORKER_EXPORTS"))
    {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let packaged = PathBuf::from("src/molt_accel/default_exports.json");
    if packaged.exists() {
        return Some(packaged);
    }
    let demo = PathBuf::from("demo/molt_worker_app/molt_exports.json");
    if demo.exists() {
        return Some(demo);
    }
    None
}

fn resolve_worker_cmd() -> Result<Vec<String>> {
    if let Ok(cmd) = env::var("MOLT_WASM_DB_WORKER_CMD").or_else(|_| env::var("MOLT_WORKER_CMD")) {
        let parts = cmd
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if parts.is_empty() {
            bail!("MOLT_WASM_DB_WORKER_CMD is empty");
        }
        return Ok(parts);
    }
    let worker = find_in_path("molt-worker").or_else(|| find_in_path("molt_worker"));
    let Some(worker) = worker else {
        bail!("molt-worker not found; set MOLT_WASM_DB_WORKER_CMD or MOLT_WORKER_CMD");
    };
    let exports_path = resolve_exports_path()
        .context("molt-worker exports manifest not found (set MOLT_WASM_DB_EXPORTS)")?;
    let mut cmd = vec![
        worker.to_string_lossy().to_string(),
        "--stdio".into(),
        "--exports".into(),
    ];
    cmd.push(exports_path.to_string_lossy().to_string());
    if let Ok(compiled) = env::var("MOLT_WASM_DB_COMPILED_EXPORTS") {
        cmd.push("--compiled-exports".into());
        cmd.push(compiled);
    }
    Ok(cmd)
}

fn resolve_timeout_ms() -> u64 {
    if let Ok(raw) =
        env::var("MOLT_WASM_DB_TIMEOUT_MS").or_else(|_| env::var("MOLT_DB_QUERY_TIMEOUT_MS"))
    {
        if let Ok(val) = raw.parse::<u64>() {
            return val;
        }
    }
    250
}

fn write_frame(mut writer: impl Write, payload: &[u8]) -> Result<()> {
    let len = payload.len();
    if len > u32::MAX as usize {
        bail!("frame too large: {len}");
    }
    let header = (len as u32).to_le_bytes();
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    Ok(())
}

fn read_frame(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_DB_FRAME_SIZE {
        bail!("worker frame too large: {size}");
    }
    let mut payload = vec![0u8; size];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

#[derive(Deserialize)]
struct WorkerEnvelope {
    request_id: Option<u64>,
    status: Option<String>,
    codec: Option<String>,
    payload_b64: Option<String>,
    error: Option<String>,
    metrics: Option<JsonValue>,
}

struct WorkerResponse {
    request_id: u64,
    status: String,
    codec: String,
    payload: Vec<u8>,
    error: Option<String>,
    metrics: Option<JsonValue>,
}

struct PendingDbRequest {
    stream_bits: u64,
    token_id: u64,
    cancel_sent: bool,
}

enum WorkerMessage {
    Response(WorkerResponse),
    Error(anyhow::Error),
}

enum WorkerError {
    Unavailable(anyhow::Error),
    SendFailed(anyhow::Error),
}

fn decode_worker_frame(frame: &[u8]) -> Result<WorkerResponse> {
    let envelope: WorkerEnvelope = serde_json::from_slice(frame)?;
    let request_id = envelope.request_id.unwrap_or(0);
    let status = envelope
        .status
        .unwrap_or_else(|| "InternalError".to_string());
    let codec = envelope.codec.unwrap_or_else(|| "raw".to_string());
    let payload = match envelope.payload_b64 {
        Some(encoded) => STANDARD.decode(encoded)?,
        None => Vec::new(),
    };
    Ok(WorkerResponse {
        request_id,
        status,
        codec,
        payload,
        error: envelope.error,
        metrics: envelope.metrics,
    })
}

fn map_worker_status(status: &str) -> &'static str {
    match status {
        "Ok" => "ok",
        "InvalidInput" => "invalid_input",
        "Busy" => "busy",
        "Timeout" => "timeout",
        "Cancelled" => "cancelled",
        "InternalError" => "internal_error",
        _ => "internal_error",
    }
}

fn json_to_msgpack(value: &JsonValue) -> MsgpackValue {
    match value {
        JsonValue::Null => MsgpackValue::Nil,
        JsonValue::Bool(val) => MsgpackValue::from(*val),
        JsonValue::Number(num) => {
            if let Some(int) = num.as_i64() {
                MsgpackValue::from(int)
            } else if let Some(uint) = num.as_u64() {
                MsgpackValue::from(uint)
            } else if let Some(float) = num.as_f64() {
                MsgpackValue::from(float)
            } else {
                MsgpackValue::Nil
            }
        }
        JsonValue::String(val) => MsgpackValue::from(val.as_str()),
        JsonValue::Array(items) => MsgpackValue::Array(items.iter().map(json_to_msgpack).collect()),
        JsonValue::Object(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, val) in map {
                entries.push((MsgpackValue::from(key.as_str()), json_to_msgpack(val)));
            }
            MsgpackValue::Map(entries)
        }
    }
}

fn encode_msgpack_header(
    status: &str,
    codec: &str,
    payload: Option<&[u8]>,
    error: Option<&str>,
    metrics: Option<&JsonValue>,
) -> Result<Vec<u8>> {
    let mut map = Vec::new();
    map.push((MsgpackValue::from("status"), MsgpackValue::from(status)));
    map.push((MsgpackValue::from("codec"), MsgpackValue::from(codec)));
    if let Some(payload) = payload {
        map.push((
            MsgpackValue::from("payload"),
            MsgpackValue::Binary(payload.to_vec()),
        ));
    }
    if let Some(error) = error {
        map.push((MsgpackValue::from("error"), MsgpackValue::from(error)));
    }
    if let Some(metrics) = metrics {
        map.push((MsgpackValue::from("metrics"), json_to_msgpack(metrics)));
    }
    let mut out = Vec::new();
    write_value(&mut out, &MsgpackValue::Map(map))?;
    Ok(out)
}

struct DbWorker {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    responses: mpsc::Receiver<WorkerMessage>,
    next_id: u64,
}

impl DbWorker {
    fn new() -> Result<Self> {
        let cmd = resolve_worker_cmd()?;
        let mut command = Command::new(&cmd[0]);
        command.args(&cmd[1..]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command.envs(env::vars());
        let mut child = command.spawn().context("spawn molt-worker")?;
        let stdin = child.stdin.take().context("missing worker stdin")?;
        let stdout = child.stdout.take().context("missing worker stdout")?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = match read_frame(&mut reader) {
                    Ok(frame) => frame,
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(err));
                        break;
                    }
                };
                let response = match decode_worker_frame(&frame) {
                    Ok(resp) => WorkerMessage::Response(resp),
                    Err(err) => WorkerMessage::Error(err),
                };
                if tx.send(response).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            responses: rx,
            next_id: 1,
        })
    }

    fn send_request(&mut self, entry: &str, payload: &[u8], timeout_ms: u64) -> Result<u64> {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let payload_b64 = STANDARD.encode(payload);
        let msg = serde_json::json!({
            "request_id": request_id,
            "entry": entry,
            "timeout_ms": timeout_ms,
            "codec": "msgpack",
            "payload_b64": payload_b64,
        });
        let bytes = serde_json::to_vec(&msg)?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?;
        write_frame(&mut *stdin, &bytes)?;
        Ok(request_id)
    }
}

fn send_worker_cancel(stdin: &Arc<Mutex<ChildStdin>>, target_id: u64) -> Result<()> {
    let cancel_payload = serde_json::json!({ "request_id": target_id });
    let cancel_bytes = serde_json::to_vec(&cancel_payload)?;
    let payload_b64 = STANDARD.encode(cancel_bytes);
    let msg = serde_json::json!({
        "request_id": 0,
        "entry": "__cancel__",
        "timeout_ms": 0,
        "codec": "json",
        "payload_b64": payload_b64,
    });
    let bytes = serde_json::to_vec(&msg)?;
    let mut guard = stdin
        .lock()
        .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?;
    write_frame(&mut *guard, &bytes)?;
    Ok(())
}

fn ensure_locale_env(envs: &mut Vec<(String, String)>) {
    let has_locale = envs.iter().any(|(k, _)| {
        k == "MOLT_WASM_LOCALE_DECIMAL"
            || k == "MOLT_WASM_LOCALE_THOUSANDS"
            || k == "MOLT_WASM_LOCALE_GROUPING"
    });
    if has_locale {
        return;
    }
    let locale = match SystemLocale::default() {
        Ok(locale) => locale,
        Err(_) => return,
    };
    envs.push((
        "MOLT_WASM_LOCALE_DECIMAL".to_string(),
        locale.decimal().to_string(),
    ));
    let sep = locale.separator().to_string();
    if !sep.is_empty() {
        envs.push(("MOLT_WASM_LOCALE_THOUSANDS".to_string(), sep));
        let grouping = match locale.grouping() {
            Grouping::Posix => None,
            Grouping::Standard | Grouping::Indian => Some("3"),
        };
        if let Some(grouping) = grouping {
            envs.push((
                "MOLT_WASM_LOCALE_GROUPING".to_string(),
                grouping.to_string(),
            ));
        }
    }
}

fn build_wasi_ctx() -> Result<WasiP1Ctx> {
    let mut envs = env::vars().collect::<Vec<_>>();
    ensure_locale_env(&mut envs);
    let mut builder = WasiCtxBuilder::new();
    builder.inherit_stdio();
    builder.envs(&envs);
    builder.inherit_args();
    builder.preopened_dir(".", ".", DirPerms::all(), FilePerms::all())?;
    Ok(builder.build_p1())
}

fn merge_limits(
    left: Option<Limits>,
    right: Option<Limits>,
    label: &str,
) -> Result<Option<Limits>> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(lim), None) | (None, Some(lim)) => Ok(Some(lim)),
        (Some(a), Some(b)) => {
            let min = a.min.max(b.min);
            let max = match (a.max, b.max) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            if let Some(max) = max {
                if min > max {
                    bail!("incompatible {label} limits: min {min} > max {max}");
                }
            }
            Ok(Some(Limits { min, max }))
        }
    }
}

fn memory_limits(module: &Module) -> Option<MemoryType> {
    module.imports().find_map(|import| {
        if import.module() != "env" || import.name() != "memory" {
            return None;
        }
        match import.ty() {
            ExternType::Memory(mem) => Some(mem),
            _ => None,
        }
    })
}

fn table_limits(module: &Module) -> Option<TableType> {
    module.imports().find_map(|import| {
        if import.module() != "env" || import.name() != "__indirect_function_table" {
            return None;
        }
        match import.ty() {
            ExternType::Table(table) => Some(table),
            _ => None,
        }
    })
}

fn collect_call_indirect_imports(module: &Module) -> Vec<(String, FuncType)> {
    module
        .imports()
        .filter_map(|import| {
            let name = import.name();
            if import.module() != "env" || !name.starts_with("molt_call_indirect") {
                return None;
            }
            let ty = match import.ty() {
                ExternType::Func(func) => func,
                _ => return None,
            };
            Some((name.to_string(), ty))
        })
        .collect()
}

fn has_runtime_imports(module: &Module) -> bool {
    module
        .imports()
        .any(|import| import.module() == "molt_runtime")
}

fn make_call_indirect_func(
    store: &mut Store<HostState>,
    name: String,
    ty: FuncType,
    registry: Arc<Mutex<HashMap<String, Option<Func>>>>,
) -> Func {
    Func::new(store, ty, move |mut caller, params, results| {
        let func = registry
            .lock()
            .ok()
            .and_then(|map| map.get(&name).cloned())
            .flatten();
        let Some(func) = func else {
            return Err(anyhow::anyhow!("{name} used before output instantiation"));
        };
        func.call(&mut caller, params, results)
    })
}

fn box_int(value: u64) -> u64 {
    QNAN | TAG_INT | (value & INT_MASK)
}

fn is_bool_bits(bits: u64) -> bool {
    (bits & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
}

fn unbox_bool(bits: u64) -> bool {
    (bits & 1) == 1
}

struct RuntimeExports {
    stream_new: Func,
    stream_send: Func,
    stream_close: Func,
    alloc: Func,
    handle_resolve: Func,
    dec_ref_obj: Func,
    header_size: Option<Func>,
    cancel_is_cancelled: Option<Func>,
}

fn runtime_exports(caller: &mut Caller<HostState>) -> Result<RuntimeExports> {
    let stream_new = caller
        .get_export("molt_stream_new")
        .and_then(Extern::into_func)
        .context("missing molt_stream_new export")?;
    let stream_send = caller
        .get_export("molt_stream_send")
        .and_then(Extern::into_func)
        .context("missing molt_stream_send export")?;
    let stream_close = caller
        .get_export("molt_stream_close")
        .and_then(Extern::into_func)
        .context("missing molt_stream_close export")?;
    let alloc = caller
        .get_export("molt_alloc")
        .and_then(Extern::into_func)
        .context("missing molt_alloc export")?;
    let handle_resolve = caller
        .get_export("molt_handle_resolve")
        .and_then(Extern::into_func)
        .context("missing molt_handle_resolve export")?;
    let dec_ref_obj = caller
        .get_export("molt_dec_ref_obj")
        .and_then(Extern::into_func)
        .context("missing molt_dec_ref_obj export")?;
    let header_size = caller
        .get_export("molt_header_size")
        .and_then(Extern::into_func);
    let cancel_is_cancelled = caller
        .get_export("molt_cancel_token_is_cancelled")
        .and_then(Extern::into_func);
    Ok(RuntimeExports {
        stream_new,
        stream_send,
        stream_close,
        alloc,
        handle_resolve,
        dec_ref_obj,
        header_size,
        cancel_is_cancelled,
    })
}

fn call_i64(func: &Func, caller: &mut Caller<HostState>, args: &[Val]) -> Result<i64> {
    let mut results = [Val::I64(0)];
    func.call(caller, args, &mut results)?;
    match results[0] {
        Val::I64(val) => Ok(val),
        _ => bail!("unexpected wasm result type"),
    }
}

fn ensure_memory(caller: &mut Caller<HostState>) -> Result<Memory> {
    if let Some(mem) = caller.data().memory {
        return Ok(mem);
    }
    if let Some(mem) = caller
        .get_export("molt_memory")
        .and_then(Extern::into_memory)
    {
        caller.data_mut().memory = Some(mem);
        return Ok(mem);
    }
    if let Some(mem) = caller.get_export("memory").and_then(Extern::into_memory) {
        caller.data_mut().memory = Some(mem);
        return Ok(mem);
    }
    bail!("wasm memory not available");
}

fn alloc_temp_bytes(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    bytes: &[u8],
) -> Result<(u64, u64)> {
    let alloc_bits = call_i64(&exports.alloc, caller, &[Val::I64(bytes.len() as i64)])? as u64;
    if alloc_bits == 0 {
        bail!("molt_alloc failed");
    }
    let ptr_bits = call_i64(
        &exports.handle_resolve,
        caller,
        &[Val::I64(alloc_bits as i64)],
    )? as u64;
    if ptr_bits == 0 {
        bail!("molt_handle_resolve failed");
    }
    let header_size = if let Some(ref func) = exports.header_size {
        call_i64(func, caller, &[])? as u64
    } else {
        40
    };
    let payload_ptr = ptr_bits + header_size;
    memory.write(caller, payload_ptr as usize, bytes)?;
    Ok((alloc_bits, payload_ptr))
}

fn send_stream_frame(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    payload: &[u8],
) -> Result<()> {
    let (alloc_bits, payload_ptr) = alloc_temp_bytes(caller, exports, memory, payload)?;
    let _ = call_i64(
        &exports.stream_send,
        caller,
        &[
            Val::I64(stream_bits as i64),
            Val::I32(payload_ptr as i32),
            Val::I64(payload.len() as i64),
        ],
    )?;
    exports
        .dec_ref_obj
        .call(caller, &[Val::I64(alloc_bits as i64)], &mut [])?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn send_stream_header(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    status: &str,
    codec: &str,
    payload: Option<&[u8]>,
    error: Option<&str>,
    metrics: Option<&JsonValue>,
) -> Result<()> {
    let header = encode_msgpack_header(status, codec, payload, error, metrics)?;
    send_stream_frame(caller, exports, memory, stream_bits, &header)
}

fn send_stream_error(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    message: &str,
) -> Result<()> {
    send_stream_header(
        caller,
        exports,
        memory,
        stream_bits,
        "internal_error",
        "raw",
        None,
        Some(message),
        None,
    )?;
    exports
        .stream_close
        .call(caller, &[Val::I64(stream_bits as i64)], &mut [])?;
    Ok(())
}

fn db_host_unavailable(caller: &mut Caller<HostState>, memory: &Memory, out_ptr: usize) -> i32 {
    if out_ptr == 0 {
        return 2;
    }
    let bytes = 0u64.to_le_bytes();
    if memory.write(caller, out_ptr, &bytes).is_err() {
        return 2;
    }
    7
}

fn read_bytes(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>> {
    if ptr == 0 || len <= 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len as usize];
    memory.read(caller, ptr as usize, &mut buf)?;
    Ok(buf)
}

fn write_bytes(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    bytes: &[u8],
) -> Result<()> {
    if ptr == 0 {
        bail!("null pointer");
    }
    memory.write(caller, ptr as usize, bytes)?;
    Ok(())
}

fn write_u32(caller: &mut Caller<HostState>, memory: &Memory, ptr: i32, val: u32) -> Result<()> {
    write_bytes(caller, memory, ptr, &val.to_le_bytes())
}

fn write_u64(caller: &mut Caller<HostState>, memory: &Memory, ptr: i32, val: u64) -> Result<()> {
    write_bytes(caller, memory, ptr, &val.to_le_bytes())
}

fn decode_string_list(buf: &[u8]) -> Result<Vec<String>> {
    if buf.len() < 4 {
        bail!("string list buffer too small");
    }
    let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut offset = 4;
    for _ in 0..count {
        if offset + 4 > buf.len() {
            bail!("string list truncated");
        }
        let len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let end = offset + len;
        if end > buf.len() {
            bail!("string list truncated");
        }
        let value = std::str::from_utf8(&buf[offset..end])?.to_string();
        out.push(value);
        offset = end;
    }
    Ok(out)
}

fn decode_env(buf: &[u8]) -> Result<(u8, Vec<(String, String)>)> {
    if buf.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mode = buf[0];
    if buf.len() < 5 {
        bail!("env buffer too small");
    }
    let count = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    let mut offset = 5;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if offset + 4 > buf.len() {
            bail!("env buffer truncated");
        }
        let key_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let key_end = offset + key_len;
        if key_end > buf.len() {
            bail!("env buffer truncated");
        }
        let key = std::str::from_utf8(&buf[offset..key_end])?.to_string();
        offset = key_end;
        if offset + 4 > buf.len() {
            bail!("env buffer truncated");
        }
        let val_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let val_end = offset + val_len;
        if val_end > buf.len() {
            bail!("env buffer truncated");
        }
        let value = std::str::from_utf8(&buf[offset..val_end])?.to_string();
        offset = val_end;
        out.push((key, value));
    }
    Ok((mode, out))
}

fn stdio_from_fd(fd: i32) -> Option<Stdio> {
    if fd < 0 {
        return None;
    }
    #[cfg(unix)]
    {
        let duped = unsafe { libc::dup(fd as libc::c_int) };
        if duped < 0 {
            return None;
        }
        let file = unsafe { std::fs::File::from_raw_fd(duped) };
        return Some(Stdio::from(file));
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;
        let duped = unsafe { libc::_dup(fd as libc::c_int) };
        if duped < 0 {
            return None;
        }
        let handle = unsafe { libc::_get_osfhandle(duped as libc::c_int) };
        if handle == -1 {
            unsafe {
                libc::_close(duped as libc::c_int);
            }
            return None;
        }
        let file = unsafe { std::fs::File::from_raw_handle(handle as *mut _) };
        return Some(Stdio::from(file));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        None
    }
}

fn spawn_process_reader<R: Read + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<ProcessEvent>,
    handle: u64,
    kind: ProcessStreamKind,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(match kind {
                        ProcessStreamKind::Stdout => ProcessEvent::StdoutClosed(handle),
                        ProcessStreamKind::Stderr => ProcessEvent::StderrClosed(handle),
                    });
                    break;
                }
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    let _ = tx.send(match kind {
                        ProcessStreamKind::Stdout => ProcessEvent::Stdout(handle, data),
                        ProcessStreamKind::Stderr => ProcessEvent::Stderr(handle, data),
                    });
                }
                Err(_) => {
                    let _ = tx.send(match kind {
                        ProcessStreamKind::Stdout => ProcessEvent::StdoutClosed(handle),
                        ProcessStreamKind::Stderr => ProcessEvent::StderrClosed(handle),
                    });
                    break;
                }
            }
        }
    });
}

fn decode_sockaddr(buf: &[u8]) -> Result<SockAddr> {
    if buf.len() < 4 {
        bail!("sockaddr buffer too small");
    }
    let family = u16::from_le_bytes([buf[0], buf[1]]) as i32;
    let port = u16::from_le_bytes([buf[2], buf[3]]);
    if family == libc::AF_INET {
        if buf.len() < 8 {
            bail!("invalid IPv4 sockaddr");
        }
        let mut octets = [0u8; 4];
        octets.copy_from_slice(&buf[4..8]);
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(octets), port));
        return Ok(SockAddr::from(addr));
    }
    if family == libc::AF_INET6 {
        if buf.len() < 28 {
            bail!("invalid IPv6 sockaddr");
        }
        let flowinfo = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let scope_id = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&buf[12..28]);
        let addr = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::from(octets),
            port,
            flowinfo,
            scope_id,
        ));
        return Ok(SockAddr::from(addr));
    }
    bail!("unsupported address family");
}

fn encode_sockaddr(addr: &SockAddr) -> Result<Vec<u8>> {
    let Some(socket_addr) = addr.as_socket() else {
        bail!("unsupported sockaddr");
    };
    let mut out = Vec::new();
    match socket_addr {
        SocketAddr::V4(addr) => {
            out.extend_from_slice(&(libc::AF_INET as u16).to_le_bytes());
            out.extend_from_slice(&addr.port().to_le_bytes());
            out.extend_from_slice(&addr.ip().octets());
        }
        SocketAddr::V6(addr) => {
            out.extend_from_slice(&(libc::AF_INET6 as u16).to_le_bytes());
            out.extend_from_slice(&addr.port().to_le_bytes());
            out.extend_from_slice(&addr.flowinfo().to_le_bytes());
            out.extend_from_slice(&addr.scope_id().to_le_bytes());
            out.extend_from_slice(&addr.ip().octets());
        }
    }
    Ok(out)
}

fn map_io_error(err: &std::io::Error) -> i32 {
    if let Some(code) = err.raw_os_error() {
        return code;
    }
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return libc::EWOULDBLOCK;
    }
    libc::EIO
}

fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -sig;
        }
    }
    -1
}

fn socket_get_mut(state: &mut HostState, handle: i64) -> Result<&mut Socket, i32> {
    if handle <= 0 {
        return Err(libc::EBADF);
    }
    state
        .socket_manager
        .get_mut(handle as u64)
        .ok_or(libc::EBADF)
}

fn ws_get_mut(state: &mut HostState, handle: i64) -> Result<&mut WebSocketEntry, i32> {
    if handle <= 0 {
        return Err(libc::EBADF);
    }
    state.ws_manager.get_mut(handle as u64).ok_or(libc::EBADF)
}

fn ws_set_nonblocking(
    ws: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
) -> std::io::Result<()> {
    match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true)?;
        }
        MaybeTlsStream::Rustls(stream) => {
            stream.get_ref().set_nonblocking(true)?;
        }
        _ => {}
    }
    Ok(())
}

fn map_ws_error(err: &tungstenite::Error) -> i32 {
    match err {
        tungstenite::Error::Io(io_err) => map_io_error(io_err),
        tungstenite::Error::Url(_) => libc::EINVAL,
        tungstenite::Error::Http(_) => libc::ECONNREFUSED,
        tungstenite::Error::Tls(_) => libc::EIO,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => libc::EPIPE,
        _ => libc::EIO,
    }
}

fn ws_drain_incoming(entry: &mut WebSocketEntry) -> Result<(), i32> {
    if entry.closed {
        return Ok(());
    }
    loop {
        match entry.socket.read() {
            Ok(Message::Binary(bytes)) => {
                entry.queue.push_back(bytes);
            }
            Ok(Message::Text(text)) => {
                entry.queue.push_back(text.into_bytes());
            }
            Ok(Message::Ping(payload)) => {
                let _ = entry.socket.send(Message::Pong(payload));
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => {
                entry.closed = true;
                break;
            }
            Err(tungstenite::Error::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {
                break;
            }
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                entry.closed = true;
                break;
            }
            Err(err) => {
                entry.closed = true;
                return Err(map_ws_error(&err));
            }
        }
        if entry.queue.len() >= 64 {
            break;
        }
    }
    Ok(())
}

fn poll_ws_stream(stream: &TcpStream, events: u32) -> Result<u32, i32> {
    let mut poll_events: i16 = 0;
    if (events & IO_EVENT_READ) != 0 {
        poll_events |= libc::POLLIN;
    }
    if (events & IO_EVENT_WRITE) != 0 {
        poll_events |= libc::POLLOUT;
    }
    if poll_events == 0 {
        poll_events |= libc::POLLIN;
    }
    #[cfg(unix)]
    {
        let fd = stream.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & libc::POLLERR) != 0
            || (revents & libc::POLLHUP) != 0
            || (revents & libc::POLLNVAL) != 0
        {
            ready |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return Ok(ready);
        }
        if (revents & libc::POLLIN) != 0 {
            ready |= IO_EVENT_READ;
        }
        if (revents & libc::POLLOUT) != 0 {
            ready |= IO_EVENT_WRITE;
        }
        Ok(ready)
    }
    #[cfg(windows)]
    {
        let fd = stream.as_raw_socket() as usize;
        let mut pfd = libc::WSAPOLLFD {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::WSAPoll(&mut pfd, 1, 0) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & (libc::POLLERR as i16)) != 0
            || (revents & (libc::POLLHUP as i16)) != 0
            || (revents & (libc::POLLNVAL as i16)) != 0
        {
            ready |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return Ok(ready);
        }
        if (revents & (libc::POLLIN as i16)) != 0 {
            ready |= IO_EVENT_READ;
        }
        if (revents & (libc::POLLOUT as i16)) != 0 {
            ready |= IO_EVENT_WRITE;
        }
        Ok(ready)
    }
}

fn poll_socket(socket: &Socket, events: u32, timeout_ms: i32) -> Result<u32, i32> {
    let mut poll_events: i16 = 0;
    if (events & 1) != 0 {
        poll_events |= libc::POLLIN;
    }
    if (events & 2) != 0 {
        poll_events |= libc::POLLOUT;
    }
    if poll_events == 0 {
        poll_events |= libc::POLLIN;
    }
    #[cfg(unix)]
    {
        let fd = socket.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & libc::POLLERR) != 0
            || (revents & libc::POLLHUP) != 0
            || (revents & libc::POLLNVAL) != 0
        {
            ready |= 4 | 1 | 2;
            return Ok(ready);
        }
        if (revents & libc::POLLIN) != 0 {
            ready |= 1;
        }
        if (revents & libc::POLLOUT) != 0 {
            ready |= 2;
        }
        Ok(ready)
    }
    #[cfg(windows)]
    {
        let fd = socket.as_raw_socket() as usize;
        let mut pfd = libc::WSAPOLLFD {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::WSAPoll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & (libc::POLLERR as i16)) != 0
            || (revents & (libc::POLLHUP as i16)) != 0
            || (revents & (libc::POLLNVAL as i16)) != 0
        {
            ready |= 4 | 1 | 2;
            return Ok(ready);
        }
        if (revents & (libc::POLLIN as i16)) != 0 {
            ready |= 1;
        }
        if (revents & (libc::POLLOUT as i16)) != 0 {
            ready |= 2;
        }
        Ok(ready)
    }
}

fn deliver_worker_response(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    response: WorkerResponse,
) {
    let status = map_worker_status(&response.status);
    if status != "ok" {
        let message = response
            .error
            .clone()
            .unwrap_or_else(|| response.status.clone());
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            None,
            Some(&message),
            response.metrics.as_ref(),
        );
        let _ = exports
            .stream_close
            .call(caller, &[Val::I64(stream_bits as i64)], &mut []);
        return;
    }

    if response.codec == "arrow_ipc" {
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            None,
            None,
            response.metrics.as_ref(),
        );
        if !response.payload.is_empty() {
            let _ = send_stream_frame(caller, exports, memory, stream_bits, &response.payload);
        }
    } else {
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            Some(&response.payload),
            None,
            response.metrics.as_ref(),
        );
    }
    let _ = exports
        .stream_close
        .call(caller, &[Val::I64(stream_bits as i64)], &mut []);
}

fn fail_pending_requests(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    pending: Vec<PendingDbRequest>,
    message: &str,
) {
    for entry in pending {
        let _ = send_stream_error(caller, exports, memory, entry.stream_bits, message);
    }
}

fn handle_db_host_poll(mut caller: Caller<'_, HostState>) -> i32 {
    let memory = match ensure_memory(&mut caller) {
        Ok(mem) => mem,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    let exports = match runtime_exports(&mut caller) {
        Ok(exports) => exports,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };

    let mut deliveries = Vec::new();
    let mut failures: Option<(Vec<PendingDbRequest>, String)> = None;
    let mut drop_worker = false;
    {
        let state = caller.data_mut();
        let Some(worker) = state.db_worker.as_mut() else {
            return 0;
        };
        match worker.child.try_wait() {
            Ok(Some(_)) | Err(_) => {
                let pending = std::mem::take(&mut state.db_pending)
                    .into_values()
                    .collect::<Vec<_>>();
                failures = Some((pending, "db host worker exited".to_string()));
                drop_worker = true;
            }
            Ok(None) => {}
        }
        if failures.is_none() {
            loop {
                match worker.responses.try_recv() {
                    Ok(WorkerMessage::Response(resp)) => {
                        if let Some(pending) = state.db_pending.remove(&resp.request_id) {
                            deliveries.push((pending, resp));
                        }
                    }
                    Ok(WorkerMessage::Error(err)) => {
                        let pending = std::mem::take(&mut state.db_pending)
                            .into_values()
                            .collect::<Vec<_>>();
                        failures = Some((pending, format!("db host error: {err}")));
                        drop_worker = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let pending = std::mem::take(&mut state.db_pending)
                            .into_values()
                            .collect::<Vec<_>>();
                        failures = Some((pending, "db host disconnected".to_string()));
                        drop_worker = true;
                        break;
                    }
                }
            }
        }
    }
    if drop_worker {
        caller.data_mut().db_worker = None;
    }

    if let Some((pending, message)) = failures {
        fail_pending_requests(&mut caller, &exports, &memory, pending, &message);
        return 0;
    }

    for (pending, response) in deliveries {
        deliver_worker_response(
            &mut caller,
            &exports,
            &memory,
            pending.stream_bits,
            response,
        );
    }

    let now = Instant::now();
    let should_check = {
        let state = caller.data();
        state
            .last_cancel_check
            .map(|last| now.duration_since(last) >= Duration::from_millis(CANCEL_POLL_MS))
            .unwrap_or(true)
    };
    if should_check {
        let cancel_func = exports.cancel_is_cancelled;
        if let Some(cancel_func) = cancel_func {
            let candidates = {
                let state = caller.data();
                state
                    .db_pending
                    .iter()
                    .filter_map(|(id, pending)| {
                        if pending.token_id != 0 && !pending.cancel_sent {
                            Some((*id, pending.token_id))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let mut cancel_ids = Vec::new();
            for (req_id, token_id) in candidates {
                let boxed = box_int(token_id);
                if let Ok(bits) = call_i64(&cancel_func, &mut caller, &[Val::I64(boxed as i64)]) {
                    let bits = bits as u64;
                    if is_bool_bits(bits) && unbox_bool(bits) {
                        cancel_ids.push(req_id);
                    }
                }
            }
            if !cancel_ids.is_empty() {
                let state = caller.data_mut();
                if let Some(worker) = state.db_worker.as_ref() {
                    for req_id in cancel_ids {
                        if let Some(pending) = state.db_pending.get_mut(&req_id) {
                            if send_worker_cancel(&worker.stdin, req_id).is_ok() {
                                pending.cancel_sent = true;
                            }
                        }
                    }
                }
            }
        }
        caller.data_mut().last_cancel_check = Some(now);
    }

    0
}

fn ptr_from_i64(ptr: i64) -> Result<usize, i32> {
    let ptr_u64 = u64::try_from(ptr).map_err(|_| 1)?;
    usize::try_from(ptr_u64).map_err(|_| 1)
}

fn handle_db_host(
    mut caller: Caller<'_, HostState>,
    entry: &str,
    req_ptr: usize,
    len_bits: i64,
    out_ptr: usize,
    token_bits: i64,
) -> i32 {
    let len_bits_u64 = match u64::try_from(len_bits) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let len = match usize::try_from(len_bits_u64) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    if out_ptr == 0 {
        return 2;
    }
    if req_ptr == 0 && len != 0 {
        return 1;
    }
    let memory = match ensure_memory(&mut caller) {
        Ok(mem) => mem,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    let mut payload = vec![0u8; len];
    if len > 0 && memory.read(&mut caller, req_ptr, &mut payload).is_err() {
        return 1;
    }

    let exports = match runtime_exports(&mut caller) {
        Ok(exports) => exports,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };

    let stream_bits = match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
        Ok(bits) => bits as u64,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    if memory
        .write(&mut caller, out_ptr, &stream_bits.to_le_bytes())
        .is_err()
    {
        return 2;
    }

    let timeout_ms = resolve_timeout_ms();
    let token_id = u64::try_from(token_bits).unwrap_or(0);
    let request_id = 'worker: {
        let state = caller.data_mut();
        let mut need_spawn = state.db_worker.is_none();
        if let Some(worker) = state.db_worker.as_mut() {
            match worker.child.try_wait() {
                Ok(Some(_)) => need_spawn = true,
                Ok(None) => {}
                Err(_) => need_spawn = true,
            }
        }
        if need_spawn {
            match DbWorker::new() {
                Ok(worker) => state.db_worker = Some(worker),
                Err(err) => break 'worker Err(WorkerError::Unavailable(err)),
            }
        }
        let worker = state
            .db_worker
            .as_mut()
            .expect("db_worker should be initialized");
        match worker.send_request(entry, &payload, timeout_ms) {
            Ok(id) => {
                state.db_pending.insert(
                    id,
                    PendingDbRequest {
                        stream_bits,
                        token_id,
                        cancel_sent: false,
                    },
                );
                Ok(id)
            }
            Err(err) => Err(WorkerError::SendFailed(err)),
        }
    };
    match request_id {
        Ok(_) => 0,
        Err(WorkerError::Unavailable(err)) => {
            eprintln!("{err}");
            db_host_unavailable(&mut caller, &memory, out_ptr)
        }
        Err(WorkerError::SendFailed(err)) => {
            let _ = send_stream_error(
                &mut caller,
                &exports,
                &memory,
                stream_bits,
                &format!("db host send failed: {err}"),
            );
            0
        }
    }
}

fn define_db_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let query = Func::wrap(
        &mut *store,
        |caller: Caller<'_, HostState>, req_ptr: i64, len: i64, out_ptr: i64, token: i64| {
            let req_ptr = match ptr_from_i64(req_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            let out_ptr = match ptr_from_i64(out_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            handle_db_host(caller, "db_query", req_ptr, len, out_ptr, token)
        },
    );
    let exec = Func::wrap(
        &mut *store,
        |caller: Caller<'_, HostState>, req_ptr: i64, len: i64, out_ptr: i64, token: i64| {
            let req_ptr = match ptr_from_i64(req_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            let out_ptr = match ptr_from_i64(out_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            handle_db_host(caller, "db_exec", req_ptr, len, out_ptr, token)
        },
    );
    let poll = Func::wrap(&mut *store, |caller: Caller<'_, HostState>| {
        handle_db_host_poll(caller)
    });
    linker.define(&mut *store, "env", "molt_db_query_host", query)?;
    linker.define(&mut *store, "env", "molt_db_exec_host", exec)?;
    linker.define(&mut *store, "env", "molt_db_host_poll", poll)?;
    Ok(())
}

fn define_socket_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let socket_new = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         family: i32,
         sock_type: i32,
         proto: i32,
         fileno: i64|
         -> i64 {
            let domain = match family {
                x if x == libc::AF_INET => Domain::IPV4,
                x if x == libc::AF_INET6 => Domain::IPV6,
                x if x == libc::AF_UNIX => {
                    #[cfg(unix)]
                    {
                        Domain::UNIX
                    }
                    #[cfg(not(unix))]
                    {
                        return -(libc::EAFNOSUPPORT as i64);
                    }
                }
                _ => return -(libc::EAFNOSUPPORT as i64),
            };
            let ty = Type::from(sock_type);
            let protocol = if proto == 0 {
                None
            } else {
                Some(Protocol::from(proto))
            };
            let socket = if fileno >= 0 {
                #[cfg(unix)]
                unsafe {
                    Socket::from_raw_fd(fileno as RawFd)
                }
                #[cfg(windows)]
                unsafe {
                    Socket::from_raw_socket(fileno as RawSocket)
                }
            } else {
                match Socket::new(domain, ty, protocol) {
                    Ok(sock) => sock,
                    Err(err) => return -(map_io_error(&err) as i64),
                }
            };
            if let Err(err) = socket.set_nonblocking(true) {
                return -(map_io_error(&err) as i64);
            }
            let id = caller.data_mut().socket_manager.insert(socket);
            id as i64
        },
    );
    let socket_close = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            if caller
                .data_mut()
                .socket_manager
                .remove(handle as u64)
                .is_none()
            {
                return -libc::EBADF;
            }
            0
        },
    );
    let socket_clone = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i64 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -(errno as i64),
            };
            let cloned = match socket.try_clone() {
                Ok(sock) => sock,
                Err(err) => return -(map_io_error(&err) as i64),
            };
            if let Err(err) = cloned.set_nonblocking(true) {
                return -(map_io_error(&err) as i64);
            }
            let id = caller.data_mut().socket_manager.insert(cloned);
            id as i64
        },
    );
    let socket_bind = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, addr_ptr: i32, addr_len: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = match decode_sockaddr(&addr_bytes) {
                Ok(addr) => addr,
                Err(_) => return -libc::EAFNOSUPPORT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.bind(&addr) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_listen = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, backlog: i32| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.listen(backlog) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_accept = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i64 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -(libc::EFAULT as i64),
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -(errno as i64),
            };
            let (accepted, addr) = match socket.accept() {
                Ok(pair) => pair,
                Err(err) => return -(map_io_error(&err) as i64),
            };
            if let Err(err) = accepted.set_nonblocking(true) {
                return -(map_io_error(&err) as i64);
            }
            let encoded = encode_sockaddr(&addr).unwrap_or_default();
            if encoded.len() > addr_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return -(libc::ENOMEM as i64);
            }
            if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                return -(libc::EFAULT as i64);
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
            let id = caller.data_mut().socket_manager.insert(accepted);
            id as i64
        },
    );
    let socket_connect = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, addr_ptr: i32, addr_len: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = match decode_sockaddr(&addr_bytes) {
                Ok(addr) => addr,
                Err(_) => return -libc::EAFNOSUPPORT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.connect(&addr) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_connect_ex = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.take_error() {
                Ok(None) => 0,
                Ok(Some(err)) => map_io_error(&err),
                Err(err) => map_io_error(&err),
            }
        },
    );
    let socket_recv = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let mut buf = vec![0u8; buf_len.max(0) as usize];
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), flags)
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::recv(
                            fd as _,
                            buf.as_mut_ptr() as *mut libc::c_char,
                            buf.len() as i32,
                            flags,
                        ) as isize
                    }
                }
            };
            if rc >= 0 {
                let n = rc as usize;
                if write_bytes(&mut caller, &memory, buf_ptr, &buf[..n]).is_err() {
                    return -libc::EFAULT;
                }
                return n as i32;
            }
            -map_io_error(&std::io::Error::last_os_error())
        },
    );
    let socket_send = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, buf_ptr, buf_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::send(fd, data.as_ptr() as *const libc::c_void, data.len(), flags)
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::send(
                            fd as _,
                            data.as_ptr() as *const libc::c_char,
                            data.len() as i32,
                            flags,
                        ) as isize
                    }
                }
            };
            if rc >= 0 {
                return rc as i32;
            }
            -map_io_error(&std::io::Error::last_os_error())
        },
    );
    let socket_sendto = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32,
         addr_ptr: i32,
         addr_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, buf_ptr, buf_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = match decode_sockaddr(&addr_bytes) {
                Ok(addr) => addr,
                Err(_) => return -libc::EAFNOSUPPORT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::sendto(
                            fd,
                            data.as_ptr() as *const libc::c_void,
                            data.len(),
                            flags,
                            addr.as_ptr(),
                            addr.len(),
                        )
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::sendto(
                            fd as _,
                            data.as_ptr() as *const libc::c_char,
                            data.len() as i32,
                            flags,
                            addr.as_ptr() as *const _,
                            addr.len() as i32,
                        ) as isize
                    }
                }
            };
            if rc >= 0 {
                return rc as i32;
            }
            -map_io_error(&std::io::Error::last_os_error())
        },
    );
    let socket_recvfrom = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let mut buf = vec![0u8; buf_len.max(0) as usize];
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::recvfrom(
                            fd,
                            buf.as_mut_ptr() as *mut libc::c_void,
                            buf.len(),
                            flags,
                            &mut storage as *mut _ as *mut libc::sockaddr,
                            &mut len,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::recvfrom(
                            fd as _,
                            buf.as_mut_ptr() as *mut libc::c_char,
                            buf.len() as i32,
                            flags,
                            &mut storage as *mut _ as *mut libc::sockaddr,
                            &mut len,
                        ) as isize
                    }
                }
            };
            if rc >= 0 {
                let n = rc as usize;
                if write_bytes(&mut caller, &memory, buf_ptr, &buf[..n]).is_err() {
                    return -libc::EFAULT;
                }
                let addr = unsafe { SockAddr::new(storage, len) };
                let encoded = encode_sockaddr(&addr).unwrap_or_default();
                if encoded.len() > addr_cap as usize {
                    let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                    return -libc::ENOMEM;
                }
                if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                    return -libc::EFAULT;
                }
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return n as i32;
            }
            -map_io_error(&std::io::Error::last_os_error())
        },
    );
    let socket_shutdown = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, how: i32| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let how = match how {
                x if x == libc::SHUT_RD => std::net::Shutdown::Read,
                x if x == libc::SHUT_WR => std::net::Shutdown::Write,
                _ => std::net::Shutdown::Both,
            };
            match socket.shutdown(how) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_getsockname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let addr = match socket.local_addr() {
                Ok(addr) => addr,
                Err(err) => return -map_io_error(&err),
            };
            let encoded = encode_sockaddr(&addr).unwrap_or_default();
            if encoded.len() > addr_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
            0
        },
    );
    let socket_getpeername = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let addr = match socket.peer_addr() {
                Ok(addr) => addr,
                Err(err) => return -map_io_error(&err),
            };
            let encoded = encode_sockaddr(&addr).unwrap_or_default();
            if encoded.len() > addr_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
            0
        },
    );
    let socket_setsockopt = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         level: i32,
         optname: i32,
         val_ptr: i32,
         val_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, val_ptr, val_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::setsockopt(
                            fd,
                            level,
                            optname,
                            data.as_ptr() as *const libc::c_void,
                            data.len() as libc::socklen_t,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::setsockopt(
                            fd as _,
                            level,
                            optname,
                            data.as_ptr() as *const _,
                            data.len() as i32,
                        )
                    }
                }
            };
            if rc == 0 {
                0
            } else {
                -map_io_error(&std::io::Error::last_os_error())
            }
        },
    );
    let socket_getsockopt = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         level: i32,
         optname: i32,
         val_ptr: i32,
         val_len: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let mut buf = vec![0u8; val_len.max(0) as usize];
            let mut len = buf.len() as libc::socklen_t;
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::getsockopt(
                            fd,
                            level,
                            optname,
                            buf.as_mut_ptr() as *mut libc::c_void,
                            &mut len,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::getsockopt(
                            fd as _,
                            level,
                            optname,
                            buf.as_mut_ptr() as *mut _,
                            &mut len,
                        )
                    }
                }
            };
            if rc != 0 {
                return -map_io_error(&std::io::Error::last_os_error());
            }
            if write_bytes(&mut caller, &memory, val_ptr, &buf[..len as usize]).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, len as u32);
            0
        },
    );
    let socket_detach = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i64 {
            let socket = match caller.data_mut().socket_manager.remove(handle as u64) {
                Some(sock) => sock,
                None => return -(libc::EBADF as i64),
            };
            #[cfg(unix)]
            {
                let raw = socket.into_raw_fd();
                raw as i64
            }
            #[cfg(windows)]
            {
                let raw = socket.into_raw_socket();
                raw as i64
            }
        },
    );
    let os_close = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, HostState>, fd: i64| -> i32 {
            if fd < 0 {
                return -(libc::EBADF as i32);
            }
            #[cfg(unix)]
            {
                let rc = unsafe { libc::close(fd as libc::c_int) };
                if rc == 0 {
                    return 0;
                }
                return -map_io_error(&std::io::Error::last_os_error());
            }
            #[cfg(windows)]
            {
                let sock_rc = unsafe { libc::closesocket(fd as libc::SOCKET) };
                if sock_rc == 0 {
                    return 0;
                }
                let sock_err = unsafe { libc::WSAGetLastError() };
                if sock_err == libc::WSAENOTSOCK {
                    let rc = unsafe { libc::_close(fd as libc::c_int) };
                    if rc == 0 {
                        return 0;
                    }
                    return -map_io_error(&std::io::Error::last_os_error());
                }
                return -(sock_err as i32);
            }
        },
    );
    let socketpair = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         family: i32,
         sock_type: i32,
         proto: i32,
         out_left_ptr: i32,
         out_right_ptr: i32|
         -> i32 {
            #[cfg(not(unix))]
            {
                let _ = (family, sock_type, proto, out_left_ptr, out_right_ptr);
                return -libc::ENOSYS;
            }
            #[cfg(unix)]
            {
                let memory = match ensure_memory(&mut caller) {
                    Ok(mem) => mem,
                    Err(_) => return -libc::EFAULT,
                };
                let domain = match family {
                    x if x == libc::AF_UNIX => Domain::UNIX,
                    x if x == libc::AF_INET => Domain::IPV4,
                    x if x == libc::AF_INET6 => Domain::IPV6,
                    _ => return -libc::EAFNOSUPPORT,
                };
                let ty = Type::from(sock_type);
                let protocol = if proto == 0 {
                    None
                } else {
                    Some(Protocol::from(proto))
                };
                let (left, right) = match Socket::pair(domain, ty, protocol) {
                    Ok(pair) => pair,
                    Err(err) => return -map_io_error(&err),
                };
                let _ = left.set_nonblocking(true);
                let _ = right.set_nonblocking(true);
                let left_id = caller.data_mut().socket_manager.insert(left);
                let right_id = caller.data_mut().socket_manager.insert(right);
                let _ = write_u64(&mut caller, &memory, out_left_ptr, left_id);
                let _ = write_u64(&mut caller, &memory, out_right_ptr, right_id);
                0
            }
        },
    );
    let socket_getaddrinfo = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         host_ptr: i32,
         host_len: i32,
         serv_ptr: i32,
         serv_len: i32,
         family: i32,
         sock_type: i32,
         proto: i32,
         flags: i32,
         out_ptr: i32,
         out_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let host = if host_len > 0 {
                match read_bytes(&mut caller, &memory, host_ptr, host_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let service = if serv_len > 0 {
                match read_bytes(&mut caller, &memory, serv_ptr, serv_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let host_cstr = host
                .as_ref()
                .map(|val| std::ffi::CString::new(val.as_slice()))
                .transpose()
                .map_err(|_| -libc::EINVAL);
            let serv_cstr = service
                .as_ref()
                .map(|val| std::ffi::CString::new(val.as_slice()))
                .transpose()
                .map_err(|_| -libc::EINVAL);
            let host_cstr = match host_cstr {
                Ok(val) => val,
                Err(err) => return err,
            };
            let serv_cstr = match serv_cstr {
                Ok(val) => val,
                Err(err) => return err,
            };
            let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
            hints.ai_family = family;
            hints.ai_socktype = sock_type;
            hints.ai_protocol = proto;
            hints.ai_flags = flags;
            let mut res: *mut libc::addrinfo = std::ptr::null_mut();
            let err = unsafe {
                libc::getaddrinfo(
                    host_cstr
                        .as_ref()
                        .map(|s| s.as_ptr())
                        .unwrap_or(std::ptr::null()),
                    serv_cstr
                        .as_ref()
                        .map(|s| s.as_ptr())
                        .unwrap_or(std::ptr::null()),
                    &hints as *const libc::addrinfo,
                    &mut res as *mut *mut libc::addrinfo,
                )
            };
            if err != 0 {
                return -err;
            }
            let mut entries: Vec<AddrInfoEntry> = Vec::new();
            let mut cur = res;
            while !cur.is_null() {
                let ai = unsafe { &*cur };
                if ai.ai_addr.is_null() {
                    cur = ai.ai_next;
                    continue;
                }
                if ai.ai_family != libc::AF_INET && ai.ai_family != libc::AF_INET6 {
                    cur = ai.ai_next;
                    continue;
                }
                let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        ai.ai_addr as *const u8,
                        &mut storage as *mut _ as *mut u8,
                        ai.ai_addrlen as usize,
                    );
                }
                let sockaddr = unsafe { SockAddr::new(storage, ai.ai_addrlen) };
                let addr_bytes = match encode_sockaddr(&sockaddr) {
                    Ok(val) => val,
                    Err(_) => {
                        cur = ai.ai_next;
                        continue;
                    }
                };
                let canon = if !ai.ai_canonname.is_null() {
                    unsafe { std::ffi::CStr::from_ptr(ai.ai_canonname) }
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec()
                } else {
                    Vec::new()
                };
                entries.push((
                    ai.ai_family,
                    ai.ai_socktype,
                    ai.ai_protocol,
                    canon,
                    addr_bytes,
                ));
                cur = ai.ai_next;
            }
            unsafe { libc::freeaddrinfo(res) };
            let mut payload = Vec::new();
            payload.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (family, sock_type, proto, canon, addr) in entries {
                payload.extend_from_slice(&family.to_le_bytes());
                payload.extend_from_slice(&sock_type.to_le_bytes());
                payload.extend_from_slice(&proto.to_le_bytes());
                payload.extend_from_slice(&(canon.len() as u32).to_le_bytes());
                payload.extend_from_slice(&canon);
                payload.extend_from_slice(&(addr.len() as u32).to_le_bytes());
                payload.extend_from_slice(&addr);
            }
            if payload.len() > out_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, payload.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, out_ptr, &payload).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, payload.len() as u32);
            0
        },
    );
    let socket_gethostname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_cap: i32, out_len_ptr: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let mut buf = vec![0u8; 256];
            let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
            if rc != 0 {
                return -map_io_error(&std::io::Error::last_os_error());
            }
            let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            if len > buf_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, len as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, buf_ptr, &buf[..len]).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, len as u32);
            0
        },
    );
    let socket_getservbyname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         name_ptr: i32,
         name_len: i32,
         proto_ptr: i32,
         proto_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let name = match read_bytes(&mut caller, &memory, name_ptr, name_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let proto = if proto_len > 0 {
                match read_bytes(&mut caller, &memory, proto_ptr, proto_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let name_cstr = match std::ffi::CString::new(name) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            let proto_cstr = match proto {
                Some(buf) => match std::ffi::CString::new(buf) {
                    Ok(val) => Some(val),
                    Err(_) => return -libc::EINVAL,
                },
                None => None,
            };
            let serv = unsafe {
                libc::getservbyname(
                    name_cstr.as_ptr(),
                    proto_cstr
                        .as_ref()
                        .map(|s| s.as_ptr())
                        .unwrap_or(std::ptr::null()),
                )
            };
            if serv.is_null() {
                return -libc::ENOENT;
            }
            unsafe { libc::ntohs((*serv).s_port as u16) as i32 }
        },
    );
    let socket_getservbyport = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         port: i32,
         proto_ptr: i32,
         proto_len: i32,
         buf_ptr: i32,
         buf_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let proto = if proto_len > 0 {
                match read_bytes(&mut caller, &memory, proto_ptr, proto_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let proto_cstr = match proto {
                Some(buf) => match std::ffi::CString::new(buf) {
                    Ok(val) => Some(val),
                    Err(_) => return -libc::EINVAL,
                },
                None => None,
            };
            let serv = unsafe {
                libc::getservbyport(
                    libc::htons(port as u16) as i32,
                    proto_cstr
                        .as_ref()
                        .map(|s| s.as_ptr())
                        .unwrap_or(std::ptr::null()),
                )
            };
            if serv.is_null() {
                return -libc::ENOENT;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*serv).s_name) }
                .to_string_lossy()
                .to_string();
            let bytes = name.as_bytes();
            if bytes.len() > buf_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, buf_ptr, bytes).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32);
            0
        },
    );
    let socket_poll = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match poll_socket(socket, events as u32, 0) {
                Ok(mask) => mask as i32,
                Err(errno) => -errno,
            }
        },
    );
    let socket_wait = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32, timeout_ms: i64| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let timeout = if timeout_ms < 0 {
                -1
            } else if timeout_ms > i32::MAX as i64 {
                i32::MAX
            } else {
                timeout_ms as i32
            };
            match poll_socket(socket, events as u32, timeout) {
                Ok(mask) => {
                    if mask == 0 {
                        return -libc::ETIMEDOUT;
                    }
                    0
                }
                Err(errno) => -errno,
            }
        },
    );
    let socket_has_ipv6 = Func::wrap(&mut *store, || -> i32 {
        let listener = std::net::TcpListener::bind("[::1]:0");
        if listener.is_ok() {
            1
        } else {
            0
        }
    });

    linker.define(&mut *store, "env", "molt_socket_new_host", socket_new)?;
    linker.define(&mut *store, "env", "molt_socket_close_host", socket_close)?;
    linker.define(&mut *store, "env", "molt_socket_clone_host", socket_clone)?;
    linker.define(&mut *store, "env", "molt_socket_bind_host", socket_bind)?;
    linker.define(&mut *store, "env", "molt_socket_listen_host", socket_listen)?;
    linker.define(&mut *store, "env", "molt_socket_accept_host", socket_accept)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_connect_host",
        socket_connect,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_connect_ex_host",
        socket_connect_ex,
    )?;
    linker.define(&mut *store, "env", "molt_socket_recv_host", socket_recv)?;
    linker.define(&mut *store, "env", "molt_socket_send_host", socket_send)?;
    linker.define(&mut *store, "env", "molt_socket_sendto_host", socket_sendto)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_recvfrom_host",
        socket_recvfrom,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_shutdown_host",
        socket_shutdown,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getsockname_host",
        socket_getsockname,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getpeername_host",
        socket_getpeername,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_setsockopt_host",
        socket_setsockopt,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getsockopt_host",
        socket_getsockopt,
    )?;
    linker.define(&mut *store, "env", "molt_socket_detach_host", socket_detach)?;
    linker.define(&mut *store, "env", "molt_os_close_host", os_close)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_socketpair_host",
        socketpair,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getaddrinfo_host",
        socket_getaddrinfo,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_gethostname_host",
        socket_gethostname,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getservbyname_host",
        socket_getservbyname,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getservbyport_host",
        socket_getservbyport,
    )?;
    linker.define(&mut *store, "env", "molt_socket_poll_host", socket_poll)?;
    linker.define(&mut *store, "env", "molt_socket_wait_host", socket_wait)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_has_ipv6_host",
        socket_has_ipv6,
    )?;
    Ok(())
}

fn define_ws_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let ws_connect = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i64, out_handle: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if out_handle == 0 {
                return -libc::EFAULT;
            }
            if url_len < 0 || url_len > i64::from(i32::MAX) {
                return -libc::EINVAL;
            }
            let url_len = url_len as i32;
            let url_bytes = match read_bytes(&mut caller, &memory, url_ptr, url_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let url_str = match String::from_utf8(url_bytes) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            let url = match Url::parse(&url_str) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            if url.scheme() != "ws" && url.scheme() != "wss" {
                return -libc::EINVAL;
            }
            let (mut socket, _) = match connect(url.as_str()) {
                Ok(val) => val,
                Err(err) => return -map_ws_error(&err),
            };
            if let Err(err) = ws_set_nonblocking(&mut socket) {
                return -map_io_error(&err);
            }
            let handle = {
                let state = caller.data_mut();
                state.ws_manager.insert(socket)
            };
            if write_u64(&mut caller, &memory, out_handle, handle).is_err() {
                caller.data_mut().ws_manager.remove(handle);
                return -libc::EFAULT;
            }
            0
        },
    );
    let ws_send = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, data_ptr: i32, len: i64| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if len < 0 || len > i64::from(i32::MAX) {
                return -libc::EINVAL;
            }
            let len = len as i32;
            let payload = match read_bytes(&mut caller, &memory, data_ptr, len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let entry = match ws_get_mut(caller.data_mut(), handle) {
                Ok(entry) => entry,
                Err(errno) => return -errno,
            };
            if entry.closed {
                return -libc::EPIPE;
            }
            match entry.socket.send(Message::Binary(payload)) {
                Ok(_) => 0,
                Err(tungstenite::Error::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    -libc::EWOULDBLOCK
                }
                Err(err) => {
                    entry.closed = true;
                    -map_ws_error(&err)
                }
            }
        },
    );
    let ws_recv = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_cap: i32,
         out_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if out_len == 0 {
                return -libc::EFAULT;
            }
            let cap = if buf_cap < 0 {
                return -libc::EINVAL;
            } else {
                buf_cap as usize
            };

            let (pending_bytes, needed_len, closed) = {
                let mut pending_bytes: Option<Vec<u8>> = None;
                let mut needed_len: Option<usize> = None;
                let entry = match ws_get_mut(caller.data_mut(), handle) {
                    Ok(entry) => entry,
                    Err(errno) => return -errno,
                };
                if entry.queue.is_empty() && !entry.closed {
                    if let Err(errno) = ws_drain_incoming(entry) {
                        return -errno;
                    }
                }
                if let Some(front) = entry.queue.front() {
                    if front.len() > cap {
                        needed_len = Some(front.len());
                    } else {
                        pending_bytes = entry.queue.pop_front();
                    }
                }
                (pending_bytes, needed_len, entry.closed)
            };

            if let Some(len) = needed_len {
                let _ = write_u32(&mut caller, &memory, out_len, len as u32);
                return -libc::ENOMEM;
            }
            if let Some(bytes) = pending_bytes {
                if write_bytes(&mut caller, &memory, buf_ptr, &bytes).is_err() {
                    return -libc::EFAULT;
                }
                let _ = write_u32(&mut caller, &memory, out_len, bytes.len() as u32);
                return 0;
            }
            let _ = write_u32(&mut caller, &memory, out_len, 0);
            if closed {
                0
            } else {
                -libc::EWOULDBLOCK
            }
        },
    );
    let ws_poll = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32| -> i32 {
            let entry = match ws_get_mut(caller.data_mut(), handle) {
                Ok(entry) => entry,
                Err(errno) => return -errno,
            };
            if entry.closed {
                return (IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE) as i32;
            }
            let events = events as u32;
            let mut ready = 0u32;
            if (events & IO_EVENT_READ) != 0 {
                if entry.queue.is_empty() {
                    if let Err(errno) = ws_drain_incoming(entry) {
                        return -errno;
                    }
                }
                if !entry.queue.is_empty() {
                    ready |= IO_EVENT_READ;
                }
            }
            if (events & IO_EVENT_WRITE) != 0 {
                let stream_ref = match entry.socket.get_ref() {
                    MaybeTlsStream::Plain(stream) => stream,
                    MaybeTlsStream::Rustls(stream) => stream.get_ref(),
                    _ => return -libc::EIO,
                };
                let poll_ready = match poll_ws_stream(stream_ref, IO_EVENT_WRITE) {
                    Ok(mask) => mask,
                    Err(errno) => return -errno,
                };
                if (poll_ready & IO_EVENT_ERROR) != 0 {
                    return (IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE) as i32;
                }
                if (poll_ready & IO_EVENT_WRITE) != 0 {
                    ready |= IO_EVENT_WRITE;
                }
            }
            ready as i32
        },
    );
    let ws_close = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller.data_mut().ws_manager.remove(handle as u64) {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            if entry.closed {
                return 0;
            }
            let mut socket = entry.socket;
            let _ = socket.close(None);
            0
        },
    );
    linker.define(&mut *store, "env", "molt_ws_connect_host", ws_connect)?;
    linker.define(&mut *store, "env", "molt_ws_poll_host", ws_poll)?;
    linker.define(&mut *store, "env", "molt_ws_send_host", ws_send)?;
    linker.define(&mut *store, "env", "molt_ws_recv_host", ws_recv)?;
    linker.define(&mut *store, "env", "molt_ws_close_host", ws_close)?;
    Ok(())
}

fn define_process_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let process_spawn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         args_ptr: i32,
         args_len: i32,
         env_ptr: i32,
         env_len: i32,
         cwd_ptr: i32,
         cwd_len: i32,
         stdin_mode: i32,
         stdout_mode: i32,
         stderr_mode: i32,
         out_handle_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let args_buf = match read_bytes(&mut caller, &memory, args_ptr, args_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let args = match decode_string_list(&args_buf) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            if args.is_empty() {
                return -libc::EINVAL;
            }
            let env_mode;
            let env_entries;
            if env_ptr != 0 && env_len > 0 {
                let env_buf = match read_bytes(&mut caller, &memory, env_ptr, env_len) {
                    Ok(buf) => buf,
                    Err(_) => return -libc::EFAULT,
                };
                match decode_env(&env_buf) {
                    Ok((mode, entries)) => {
                        env_mode = mode;
                        env_entries = entries;
                    }
                    Err(_) => return -libc::EINVAL,
                }
            } else {
                env_mode = 0;
                env_entries = Vec::new();
            }
            let cwd = if cwd_ptr != 0 && cwd_len > 0 {
                let cwd_buf = match read_bytes(&mut caller, &memory, cwd_ptr, cwd_len) {
                    Ok(buf) => buf,
                    Err(_) => return -libc::EFAULT,
                };
                match String::from_utf8(cwd_buf) {
                    Ok(val) => Some(val),
                    Err(_) => return -libc::EINVAL,
                }
            } else {
                None
            };

            let mut cmd = Command::new(&args[0]);
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }
            match env_mode {
                1 => {
                    cmd.env_clear();
                    for (key, value) in env_entries {
                        cmd.env(key, value);
                    }
                }
                2 => {
                    for (key, value) in env_entries {
                        cmd.env(key, value);
                    }
                }
                _ => {}
            }
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }
            match stdin_mode {
                PROCESS_STDIO_PIPE => {
                    cmd.stdin(Stdio::piped());
                }
                PROCESS_STDIO_DEVNULL => {
                    cmd.stdin(Stdio::null());
                }
                val if val >= PROCESS_STDIO_FD_BASE => {
                    let fd = val - PROCESS_STDIO_FD_BASE;
                    let Some(stdio) = stdio_from_fd(fd) else {
                        return -libc::EBADF;
                    };
                    cmd.stdin(stdio);
                }
                _ => {
                    cmd.stdin(Stdio::inherit());
                }
            }

            let mut merged_stdout_reader: Option<os_pipe::PipeReader> = None;
            if stderr_mode == PROCESS_STDIO_STDOUT_REDIRECT {
                if stdout_mode == PROCESS_STDIO_PIPE {
                    let (reader, writer) = match os_pipe::pipe() {
                        Ok(val) => val,
                        Err(err) => return -map_io_error(&err),
                    };
                    let writer_err = match writer.try_clone() {
                        Ok(val) => val,
                        Err(err) => return -map_io_error(&err),
                    };
                    cmd.stdout(writer);
                    cmd.stderr(writer_err);
                    merged_stdout_reader = Some(reader);
                } else if stdout_mode == PROCESS_STDIO_DEVNULL {
                    cmd.stdout(Stdio::null());
                    cmd.stderr(Stdio::null());
                } else if stdout_mode >= PROCESS_STDIO_FD_BASE {
                    let fd = stdout_mode - PROCESS_STDIO_FD_BASE;
                    let Some(stdout_stdio) = stdio_from_fd(fd) else {
                        return -libc::EBADF;
                    };
                    let Some(stderr_stdio) = stdio_from_fd(fd) else {
                        return -libc::EBADF;
                    };
                    cmd.stdout(stdout_stdio);
                    cmd.stderr(stderr_stdio);
                } else {
                    cmd.stdout(Stdio::inherit());
                    cmd.stderr(Stdio::inherit());
                }
            } else {
                match stdout_mode {
                    PROCESS_STDIO_PIPE => {
                        cmd.stdout(Stdio::piped());
                    }
                    PROCESS_STDIO_DEVNULL => {
                        cmd.stdout(Stdio::null());
                    }
                    val if val >= PROCESS_STDIO_FD_BASE => {
                        let fd = val - PROCESS_STDIO_FD_BASE;
                        let Some(stdio) = stdio_from_fd(fd) else {
                            return -libc::EBADF;
                        };
                        cmd.stdout(stdio);
                    }
                    _ => {
                        cmd.stdout(Stdio::inherit());
                    }
                }
                match stderr_mode {
                    PROCESS_STDIO_PIPE => {
                        cmd.stderr(Stdio::piped());
                    }
                    PROCESS_STDIO_DEVNULL => {
                        cmd.stderr(Stdio::null());
                    }
                    val if val >= PROCESS_STDIO_FD_BASE => {
                        let fd = val - PROCESS_STDIO_FD_BASE;
                        let Some(stdio) = stdio_from_fd(fd) else {
                            return -libc::EBADF;
                        };
                        cmd.stderr(stdio);
                    }
                    _ => {
                        cmd.stderr(Stdio::inherit());
                    }
                }
            }

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => return -map_io_error(&err),
            };
            let pid = child.id();
            let handle = {
                let state = caller.data_mut();
                state.process_manager.alloc_handle(pid)
            };

            let exports = match runtime_exports(&mut caller) {
                Ok(exports) => exports,
                Err(_) => return -libc::EFAULT,
            };

            let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
                match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
                    Ok(bits) => Some(bits as u64),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
                match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
                    Ok(bits) => Some(bits as u64),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let stdin = child.stdin.take();
            {
                let state = caller.data_mut();
                state.process_manager.processes.insert(
                    handle,
                    ProcessEntry {
                        child,
                        stdin,
                        stdout_stream,
                        stderr_stream,
                        exit_code: None,
                    },
                );
            }
            if let Some(reader) = merged_stdout_reader.take() {
                let tx = caller.data().process_manager.events_tx.clone();
                spawn_process_reader(reader, tx, handle, ProcessStreamKind::Stdout);
            } else if let Some(stdout) = stdout {
                let tx = caller.data().process_manager.events_tx.clone();
                spawn_process_reader(stdout, tx, handle, ProcessStreamKind::Stdout);
            }
            if let Some(stderr) = stderr {
                let tx = caller.data().process_manager.events_tx.clone();
                spawn_process_reader(stderr, tx, handle, ProcessStreamKind::Stderr);
            }

            if out_handle_ptr != 0 {
                let _ = write_u64(&mut caller, &memory, out_handle_ptr, handle);
            }
            0
        },
    );

    let process_wait = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, _timeout_ms: i64, out_code: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let code = {
                let entry = match caller
                    .data_mut()
                    .process_manager
                    .processes
                    .get_mut(&(handle as u64))
                {
                    Some(entry) => entry,
                    None => return -libc::EBADF,
                };
                if entry.exit_code.is_none() {
                    match entry.child.try_wait() {
                        Ok(Some(status)) => {
                            entry.exit_code = Some(exit_code_from_status(status));
                        }
                        Ok(None) => {}
                        Err(err) => return -map_io_error(&err),
                    }
                }
                entry.exit_code
            };
            let Some(code) = code else {
                return -libc::EWOULDBLOCK;
            };
            if out_code != 0 {
                let _ = write_bytes(&mut caller, &memory, out_code, &code.to_le_bytes());
            }
            if let Some(func) = caller
                .get_export("molt_process_host_notify")
                .and_then(Extern::into_func)
            {
                let _ = func.call(&mut caller, &[Val::I64(handle), Val::I32(code)], &mut []);
            }
            0
        },
    );

    let process_kill = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            match entry.child.kill() {
                Ok(_) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );

    let process_terminate = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            #[cfg(unix)]
            {
                let pid = entry.child.id() as i32;
                let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
                if rc != 0 {
                    return -map_io_error(&std::io::Error::last_os_error());
                }
                0
            }
            #[cfg(not(unix))]
            {
                match entry.child.kill() {
                    Ok(_) => 0,
                    Err(err) => -map_io_error(&err),
                }
            }
        },
    );

    let process_write = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, data_ptr: i32, len: i64| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let len_i32 = i32::try_from(len).unwrap_or(0);
            if len_i32 <= 0 {
                return 0;
            }
            let buf = match read_bytes(&mut caller, &memory, data_ptr, len_i32) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            let Some(stdin) = entry.stdin.as_mut() else {
                return -libc::EPIPE;
            };
            if let Err(err) = stdin.write_all(&buf) {
                return -map_io_error(&err);
            }
            if let Err(err) = stdin.flush() {
                return -map_io_error(&err);
            }
            0
        },
    );

    let process_close_stdin = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            entry.stdin = None;
            0
        },
    );

    let process_stdio = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, which: i32, out_stream: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let stream_bits = match caller
                .data()
                .process_manager
                .processes
                .get(&(handle as u64))
            {
                Some(entry) => match which {
                    PROCESS_STDIO_STDOUT => entry.stdout_stream,
                    PROCESS_STDIO_STDERR => entry.stderr_stream,
                    _ => None,
                },
                None => return -libc::EBADF,
            };
            let Some(bits) = stream_bits else {
                return -libc::EINVAL;
            };
            if out_stream != 0 {
                let _ = write_u64(&mut caller, &memory, out_stream, bits);
            }
            0
        },
    );

    let process_poll = Func::wrap(&mut *store, |mut caller: Caller<'_, HostState>| -> i32 {
        let memory = match ensure_memory(&mut caller) {
            Ok(mem) => mem,
            Err(_) => return -libc::EFAULT,
        };
        let exports = match runtime_exports(&mut caller) {
            Ok(exports) => exports,
            Err(_) => return -libc::EFAULT,
        };
        let mut events = Vec::new();
        while let Ok(event) = caller.data_mut().process_manager.events_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                ProcessEvent::Stdout(handle, data) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stdout_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ =
                            send_stream_frame(&mut caller, &exports, &memory, stream_bits, &data);
                    }
                }
                ProcessEvent::Stderr(handle, data) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stderr_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ =
                            send_stream_frame(&mut caller, &exports, &memory, stream_bits, &data);
                    }
                }
                ProcessEvent::StdoutClosed(handle) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stdout_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ = exports.stream_close.call(
                            &mut caller,
                            &[Val::I64(stream_bits as i64)],
                            &mut [],
                        );
                    }
                }
                ProcessEvent::StderrClosed(handle) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stderr_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ = exports.stream_close.call(
                            &mut caller,
                            &[Val::I64(stream_bits as i64)],
                            &mut [],
                        );
                    }
                }
            }
        }
        let mut exited = Vec::new();
        {
            let state = caller.data_mut();
            for (handle, entry) in state.process_manager.processes.iter_mut() {
                if entry.exit_code.is_none() {
                    if let Ok(Some(status)) = entry.child.try_wait() {
                        let code = exit_code_from_status(status);
                        entry.exit_code = Some(code);
                        exited.push((*handle, code));
                    }
                }
            }
        }
        if !exited.is_empty() {
            if let Some(func) = caller
                .get_export("molt_process_host_notify")
                .and_then(Extern::into_func)
            {
                for (handle, code) in exited {
                    let _ = func.call(
                        &mut caller,
                        &[Val::I64(handle as i64), Val::I32(code)],
                        &mut [],
                    );
                }
            }
        }
        0
    });

    linker.define(&mut *store, "env", "molt_process_spawn_host", process_spawn)?;
    linker.define(&mut *store, "env", "molt_process_wait_host", process_wait)?;
    linker.define(&mut *store, "env", "molt_process_kill_host", process_kill)?;
    linker.define(
        &mut *store,
        "env",
        "molt_process_terminate_host",
        process_terminate,
    )?;
    linker.define(&mut *store, "env", "molt_process_write_host", process_write)?;
    linker.define(
        &mut *store,
        "env",
        "molt_process_close_stdin_host",
        process_close_stdin,
    )?;
    linker.define(&mut *store, "env", "molt_process_stdio_host", process_stdio)?;
    linker.define(&mut *store, "env", "molt_process_host_poll", process_poll)?;
    Ok(())
}

fn set_memory_from_exports(store: &mut Store<HostState>, instance: &wasmtime::Instance) {
    if store.data().memory.is_some() {
        return;
    }
    if let Some(mem) = instance.get_memory(&mut *store, "molt_memory") {
        store.data_mut().memory = Some(mem);
        return;
    }
    if let Some(mem) = instance.get_memory(&mut *store, "memory") {
        store.data_mut().memory = Some(mem);
    }
}

fn register_call_indirect_exports(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    registry: &Arc<Mutex<HashMap<String, Option<Func>>>>,
    names: &[String],
) -> Result<()> {
    let mut map = registry
        .lock()
        .map_err(|_| anyhow::anyhow!("call_indirect registry poisoned"))?;
    for name in names {
        let func = instance
            .get_func(&mut *store, name)
            .with_context(|| format!("missing export {name}"))?;
        map.insert(name.clone(), Some(func));
    }
    Ok(())
}

fn alloc_results(ty: &FuncType) -> Result<Vec<Val>> {
    let mut results = Vec::new();
    for val_ty in ty.results() {
        let Some(val) = Val::default_for_ty(&val_ty) else {
            bail!("unsupported molt_main return type: {val_ty:?}");
        };
        results.push(val);
    }
    Ok(results)
}

fn main() -> Result<()> {
    debug_log(|| "starting".to_string());
    let mut args = env::args().skip(1);
    let arg = match args.next() {
        Some(flag) if flag == "-h" || flag == "--help" => {
            eprintln!("usage: molt-wasm-host [output.wasm]");
            return Ok(());
        }
        other => other,
    };

    let wasm_path = resolve_wasm_path(arg)?;
    let linked_path = resolve_linked_path(&wasm_path);
    let mut use_linked = force_linked() || (prefer_linked() && linked_path.is_some());
    let mut main_path = if use_linked {
        linked_path.clone().unwrap_or_else(|| wasm_path.clone())
    } else {
        wasm_path.clone()
    };

    let engine = build_engine()?;
    let mut output_module =
        load_or_compile_module(&engine, &main_path, "main", "MOLT_WASM_PRECOMPILED_PATH")?;
    let mut needs_runtime = has_runtime_imports(&output_module);
    if needs_runtime {
        if use_linked {
            bail!("linked wasm still imports molt_runtime; link step incomplete");
        }
        let Some(linked_path) = linked_path.clone() else {
            bail!(
                "linked wasm required for Molt runtime outputs; build with --linked or set MOLT_WASM_LINK=1."
            );
        };
        output_module =
            load_or_compile_module(&engine, &linked_path, "main", "MOLT_WASM_PRECOMPILED_PATH")?;
        needs_runtime = has_runtime_imports(&output_module);
        if needs_runtime {
            bail!("linked wasm still imports molt_runtime; link step incomplete");
        }
        main_path = linked_path;
        use_linked = true;
    }
    debug_log(|| format!("main wasm: {main_path:?} (linked={use_linked})"));

    let runtime_module = if needs_runtime {
        let runtime_path = env::var("MOLT_RUNTIME_WASM")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("wasm/molt_runtime.wasm"));
        Some(load_or_compile_module(
            &engine,
            &runtime_path,
            "runtime",
            "MOLT_WASM_PRECOMPILED_RUNTIME_PATH",
        )?)
    } else {
        None
    };

    let output_mem = memory_limits(&output_module);
    let output_table = table_limits(&output_module);
    let runtime_mem = runtime_module.as_ref().and_then(memory_limits);
    let runtime_table = runtime_module.as_ref().and_then(table_limits);

    let memory_limits = merge_limits(
        output_mem.as_ref().map(|mem| Limits {
            min: mem.minimum() as u32,
            max: mem.maximum().map(|v| v as u32),
        }),
        runtime_mem.as_ref().map(|mem| Limits {
            min: mem.minimum() as u32,
            max: mem.maximum().map(|v| v as u32),
        }),
        "memory",
    )?;
    let table_limits = merge_limits(
        output_table.as_ref().map(|table| Limits {
            min: table.minimum() as u32,
            max: table.maximum().map(|v| v as u32),
        }),
        runtime_table.as_ref().map(|table| Limits {
            min: table.minimum() as u32,
            max: table.maximum().map(|v| v as u32),
        }),
        "table",
    )?;

    let mut store = Store::new(
        &engine,
        HostState {
            wasi: build_wasi_ctx()?,
            memory: None,
            call_indirect: Arc::new(Mutex::new(HashMap::new())),
            db_worker: None,
            db_pending: HashMap::new(),
            last_cancel_check: None,
            socket_manager: SocketManager::new(),
            ws_manager: WebSocketManager::new(),
            process_manager: ProcessManager::new(),
        },
    );

    let mut linker = Linker::new(&engine);
    p1::add_to_linker_sync(&mut linker, |state: &mut HostState| &mut state.wasi)?;

    if let Some(limits) = memory_limits {
        let output_is_64 = output_mem.as_ref().map(|mem| mem.is_64()).unwrap_or(false);
        let runtime_is_64 = runtime_mem.as_ref().map(|mem| mem.is_64()).unwrap_or(false);
        if output_is_64 || runtime_is_64 {
            bail!("memory64 not supported in wasm host");
        }
        let memory = Memory::new(&mut store, MemoryType::new(limits.min, limits.max))?;
        linker.define(&mut store, "env", "memory", memory)?;
        store.data_mut().memory = Some(memory);
    }
    if let Some(limits) = table_limits {
        let element = match (
            output_table.as_ref().map(|table| table.element().clone()),
            runtime_table.as_ref().map(|table| table.element().clone()),
        ) {
            (Some(left), Some(_right)) => left,
            (Some(left), None) => left,
            (None, Some(right)) => right,
            (None, None) => wasmtime::RefType::FUNCREF,
        };
        let table = Table::new(
            &mut store,
            TableType::new(element, limits.min, limits.max),
            Ref::Func(None),
        )?;
        linker.define(&mut store, "env", "__indirect_function_table", table)?;
    }

    define_db_host(&mut linker, &mut store)?;
    define_socket_host(&mut linker, &mut store)?;
    define_ws_host(&mut linker, &mut store)?;
    define_process_host(&mut linker, &mut store)?;
    let getpid = Func::wrap(&mut store, || -> i64 { std::process::id() as i64 });
    linker.define(&mut store, "env", "molt_getpid_host", getpid)?;

    let registry = store.data().call_indirect.clone();
    let call_imports = if let Some(runtime_module) = runtime_module.as_ref() {
        collect_call_indirect_imports(runtime_module)
    } else {
        collect_call_indirect_imports(&output_module)
    };
    let call_names = call_imports
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    for (name, ty) in call_imports {
        let func = make_call_indirect_func(&mut store, name.clone(), ty, registry.clone());
        linker.define(&mut store, "env", &name, func)?;
    }

    if let Some(runtime_module) = runtime_module {
        debug_log(|| "instantiating runtime".to_string());
        let runtime_instance = linker
            .instantiate(&mut store, &runtime_module)
            .context("instantiate runtime")?;
        debug_log(|| "runtime instantiated".to_string());
        for import in output_module.imports() {
            if import.module() != "molt_runtime" {
                continue;
            }
            let name = import.name();
            let export_name = format!("molt_{name}");
            let export = runtime_instance
                .get_export(&mut store, &export_name)
                .with_context(|| format!("missing runtime export {export_name}"))?;
            linker.define(&mut store, "molt_runtime", name, export)?;
        }
        debug_log(|| "instantiating output module".to_string());
        let output_instance = linker
            .instantiate(&mut store, &output_module)
            .context("instantiate output")?;
        debug_log(|| "output module instantiated".to_string());
        register_call_indirect_exports(&mut store, &output_instance, &registry, &call_names)?;
        set_memory_from_exports(&mut store, &output_instance);
        let main = output_instance
            .get_func(&mut store, "molt_main")
            .context("missing molt_main export")?;
        debug_log(|| "calling molt_main".to_string());
        let mut results = alloc_results(&main.ty(&store))?;
        main.call(&mut store, &[], &mut results)?;
        debug_log(|| "molt_main returned".to_string());
    } else {
        debug_log(|| "instantiating linked output".to_string());
        let output_instance = linker
            .instantiate(&mut store, &output_module)
            .context("instantiate linked output")?;
        debug_log(|| "linked output instantiated".to_string());
        register_call_indirect_exports(&mut store, &output_instance, &registry, &call_names)?;
        set_memory_from_exports(&mut store, &output_instance);
        let main = output_instance
            .get_func(&mut store, "molt_main")
            .context("missing molt_main export")?;
        debug_log(|| "calling molt_main".to_string());
        let mut results = alloc_results(&main.ty(&store))?;
        main.call(&mut store, &[], &mut results)?;
        debug_log(|| "molt_main returned".to_string());
    }

    Ok(())
}
