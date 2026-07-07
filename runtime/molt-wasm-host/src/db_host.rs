use super::*;
use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use rmpv::Value as MsgpackValue;
use rmpv::encode::write_value;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const MAX_DB_FRAME_SIZE: usize = 64 * 1024 * 1024;
const CANCEL_POLL_MS: u64 = 10;
const CANCEL_POLL_BATCH: usize = 256;
const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const INT_MASK: u64 = (1 << 47) - 1;

pub(super) struct DbHostState {
    worker: Option<DbWorker>,
    pending: HashMap<u64, PendingDbRequest>,
    cancel_index: Vec<u64>,
    cancel_positions: HashMap<u64, usize>,
    cancel_cursor: usize,
    last_cancel_check: Option<Instant>,
}

impl DbHostState {
    pub(super) fn new() -> Self {
        Self {
            worker: None,
            pending: HashMap::new(),
            cancel_index: Vec::new(),
            cancel_positions: HashMap::new(),
            cancel_cursor: 0,
            last_cancel_check: None,
        }
    }
}

fn db_cancel_track(db: &mut DbHostState, req_id: u64) {
    indexed_track(&mut db.cancel_index, &mut db.cancel_positions, req_id);
}

fn db_cancel_untrack(db: &mut DbHostState, req_id: u64) {
    indexed_untrack(
        &mut db.cancel_index,
        &mut db.cancel_positions,
        &mut db.cancel_cursor,
        req_id,
    );
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
        && let Ok(val) = raw.parse::<u64>()
    {
        return val;
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

fn box_int(value: u64) -> u64 {
    QNAN | TAG_INT | (value & INT_MASK)
}

fn is_bool_bits(bits: u64) -> bool {
    (bits & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
}

fn unbox_bool(bits: u64) -> bool {
    (bits & 1) == 1
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

fn drain_db_pending(state: &mut HostState) -> Vec<PendingDbRequest> {
    state.db.cancel_index.clear();
    state.db.cancel_positions.clear();
    state.db.cancel_cursor = 0;
    std::mem::take(&mut state.db.pending)
        .into_values()
        .collect::<Vec<_>>()
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
        let worker_status = match state.db.worker.as_mut() {
            Some(worker) => worker.child.try_wait(),
            None => return 0,
        };
        match worker_status {
            Ok(Some(_)) | Err(_) => {
                let pending = drain_db_pending(state);
                failures = Some((pending, "db host worker exited".to_string()));
                drop_worker = true;
            }
            Ok(None) => {}
        }
        if failures.is_none() {
            loop {
                let message = match state.db.worker.as_mut() {
                    Some(worker) => worker.responses.try_recv(),
                    None => Err(mpsc::TryRecvError::Disconnected),
                };
                match message {
                    Ok(WorkerMessage::Response(resp)) => {
                        if let Some(pending) = state.db.pending.remove(&resp.request_id) {
                            db_cancel_untrack(&mut state.db, resp.request_id);
                            deliveries.push((pending, resp));
                        }
                    }
                    Ok(WorkerMessage::Error(err)) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, format!("db host error: {err}")));
                        drop_worker = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, "db host disconnected".to_string()));
                        drop_worker = true;
                        break;
                    }
                }
            }
        }
    }
    if drop_worker {
        caller.data_mut().db.worker = None;
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
            .db
            .last_cancel_check
            .map(|last| now.duration_since(last) >= Duration::from_millis(CANCEL_POLL_MS))
            .unwrap_or(true)
    };
    if should_check {
        let cancel_func = exports.cancel_is_cancelled;
        if let Some(cancel_func) = cancel_func {
            let candidate_ids = {
                let state = caller.data_mut();
                let budget = state.db.cancel_index.len().min(CANCEL_POLL_BATCH);
                indexed_next_batch(&state.db.cancel_index, &mut state.db.cancel_cursor, budget)
            };
            let candidates = {
                let state = caller.data_mut();
                let mut stale_ids = Vec::new();
                let mut batch = Vec::with_capacity(candidate_ids.len());
                for req_id in candidate_ids {
                    if let Some(pending) = state.db.pending.get(&req_id)
                        && pending.token_id != 0
                        && !pending.cancel_sent
                    {
                        batch.push((req_id, pending.token_id));
                    } else {
                        stale_ids.push(req_id);
                    }
                }
                for req_id in stale_ids {
                    db_cancel_untrack(&mut state.db, req_id);
                }
                batch
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
                let worker_stdin = state.db.worker.as_ref().map(|worker| worker.stdin.clone());
                if let Some(worker_stdin) = worker_stdin {
                    for req_id in cancel_ids {
                        let mut stop_polling_token = false;
                        if let Some(pending) = state.db.pending.get_mut(&req_id)
                            && pending.token_id != 0
                            && !pending.cancel_sent
                            && send_worker_cancel(&worker_stdin, req_id).is_ok()
                        {
                            pending.cancel_sent = true;
                            stop_polling_token = true;
                        }
                        if stop_polling_token || !state.db.pending.contains_key(&req_id) {
                            db_cancel_untrack(&mut state.db, req_id);
                        }
                    }
                }
            }
        }
        caller.data_mut().db.last_cancel_check = Some(now);
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
        let mut need_spawn = state.db.worker.is_none();
        if let Some(worker) = state.db.worker.as_mut() {
            match worker.child.try_wait() {
                Ok(Some(_)) => need_spawn = true,
                Ok(None) => {}
                Err(_) => need_spawn = true,
            }
        }
        if need_spawn {
            match DbWorker::new() {
                Ok(worker) => state.db.worker = Some(worker),
                Err(err) => break 'worker Err(WorkerError::Unavailable(err)),
            }
        }
        let worker = state
            .db
            .worker
            .as_mut()
            .expect("db_worker should be initialized");
        match worker.send_request(entry, &payload, timeout_ms) {
            Ok(id) => {
                state.db.pending.insert(
                    id,
                    PendingDbRequest {
                        stream_bits,
                        token_id,
                        cancel_sent: false,
                    },
                );
                if token_id != 0 {
                    db_cancel_track(&mut state.db, id);
                }
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

pub(super) fn define_db_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
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
