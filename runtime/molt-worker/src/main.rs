use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
mod diagnostics;

use diagnostics::{
    ComputeRequest, ComputeResponse, HealthResponse, OffloadTableRequest, OffloadTableResponse,
};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use molt_db::{Pool, Pooled};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireCodec {
    Json,
    Msgpack,
}

const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
struct ExportEntry {
    name: String,
}

#[derive(Deserialize)]
struct ExportsManifest {
    exports: Vec<ExportEntry>,
}

#[derive(Clone)]
struct CompiledEntry {
    name: String,
    codec_in: String,
    codec_out: String,
}

#[derive(Deserialize)]
struct RequestEnvelope {
    request_id: u64,
    entry: String,
    timeout_ms: u32,
    codec: String,
    payload: Option<ByteBuf>,
    payload_b64: Option<String>,
}

#[derive(Serialize)]
struct ResponseEnvelope {
    request_id: u64,
    status: String,
    codec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<ByteBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<HashMap<String, u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiled: Option<u64>,
}

#[derive(Serialize)]
struct ResponseEnvelopeJson {
    request_id: u64,
    status: String,
    codec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload_b64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<HashMap<String, u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiled: Option<u64>,
}

#[derive(Deserialize, Serialize)]
struct ListItemsRequest {
    user_id: i64,
    q: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct CancelRequest {
    request_id: u64,
}

#[derive(Serialize, Deserialize)]
struct ItemRow {
    id: i64,
    created_at: String,
    status: String,
    title: String,
    score: f64,
    unread: bool,
}

#[derive(Serialize, Deserialize)]
struct CountSummary {
    open: u32,
    closed: u32,
}

#[derive(Serialize, Deserialize)]
struct ListItemsResponse {
    items: Vec<ItemRow>,
    next_cursor: Option<String>,
    counts: CountSummary,
}

struct DecodedRequest {
    envelope: RequestEnvelope,
    wire: WireCodec,
    queued_at: Instant,
}

type CancelSet = Arc<Mutex<HashSet<u64>>>;

type DbPool = Arc<Pool<FakeDbConn>>;

#[derive(Debug)]
struct ExecError {
    status: &'static str,
    message: String,
}

struct FakeDbConn;

fn load_compiled_entries(path: Option<PathBuf>) -> Result<HashMap<String, CompiledEntry>, String> {
    let mut compiled = HashMap::new();
    if let Some(path) = path {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(exports) = manifest.get("exports").and_then(|v| v.as_array()) {
                    for entry in exports {
                        let name = entry
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let codec_in = entry
                            .get("codec_in")
                            .and_then(|v| v.as_str())
                            .unwrap_or("msgpack")
                            .to_string();
                        let codec_out = entry
                            .get("codec_out")
                            .and_then(|v| v.as_str())
                            .unwrap_or("msgpack")
                            .to_string();
                        if name.is_empty() || name.starts_with("__") {
                            continue;
                        }
                        compiled.insert(
                            name.clone(),
                            CompiledEntry {
                                name,
                                codec_in,
                                codec_out,
                            },
                        );
                    }
                }
            }
        }
    }
    Ok(compiled)
}

fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    if let Err(err) = reader.read_exact(&mut header) {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(err);
    }
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Frame exceeds max size",
        ));
    }
    let mut buf = vec![0u8; size];
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    let size = payload.len() as u32;
    writer.write_all(&size.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

fn decode_request(bytes: &[u8]) -> Result<DecodedRequest, String> {
    if let Ok(env) = rmp_serde::from_slice::<RequestEnvelope>(bytes) {
        return Ok(DecodedRequest {
            envelope: env,
            wire: WireCodec::Msgpack,
            queued_at: Instant::now(),
        });
    }
    let env = serde_json::from_slice::<RequestEnvelope>(bytes)
        .map_err(|err| format!("Invalid request: {err}"))?;
    Ok(DecodedRequest {
        envelope: env,
        wire: WireCodec::Json,
        queued_at: Instant::now(),
    })
}

fn extract_payload(envelope: &RequestEnvelope) -> Result<Vec<u8>, String> {
    if let Some(payload) = &envelope.payload {
        return Ok(payload.clone().into_vec());
    }
    if let Some(encoded) = &envelope.payload_b64 {
        return BASE64
            .decode(encoded)
            .map_err(|err| format!("Invalid payload base64: {err}"));
    }
    Ok(Vec::new())
}

fn encode_response(response: &ResponseEnvelope, wire: WireCodec) -> Result<Vec<u8>, String> {
    match wire {
        WireCodec::Msgpack => rmp_serde::to_vec_named(response).map_err(|err| err.to_string()),
        WireCodec::Json => {
            let payload_b64 = response
                .payload
                .as_ref()
                .map(|payload| BASE64.encode(payload.as_ref()));
            let json = ResponseEnvelopeJson {
                request_id: response.request_id,
                status: response.status.clone(),
                codec: response.codec.clone(),
                payload_b64,
                metrics: response.metrics.clone(),
                error: response.error.clone(),
                entry: response.entry.clone(),
                compiled: response.compiled,
            };
            serde_json::to_vec(&json).map_err(|err| err.to_string())
        }
    }
}

fn decode_payload<T: for<'de> Deserialize<'de>>(payload: &[u8], codec: &str) -> Result<T, String> {
    match codec {
        "msgpack" => rmp_serde::from_slice(payload).map_err(|err| err.to_string()),
        "json" => serde_json::from_slice(payload).map_err(|err| err.to_string()),
        _ => Err(format!("Unsupported payload codec '{codec}'")),
    }
}

fn encode_payload<T: Serialize>(payload: &T, codec: &str) -> Result<Vec<u8>, String> {
    match codec {
        "msgpack" => rmp_serde::to_vec_named(payload).map_err(|err| err.to_string()),
        "json" => serde_json::to_vec(payload).map_err(|err| err.to_string()),
        "raw" => Ok(Vec::new()),
        _ => Err(format!("Unsupported payload codec '{codec}'")),
    }
}

fn acquire_connection(
    pool: &DbPool,
    timeout: Option<Duration>,
) -> Result<Pooled<FakeDbConn>, ExecError> {
    pool.acquire(timeout).ok_or_else(|| ExecError {
        status: "Busy",
        message: "DB pool exhausted".to_string(),
    })
}

fn is_cancelled(cancelled: &CancelSet, request_id: u64) -> bool {
    let mut guard = cancelled.lock().unwrap();
    guard.remove(&request_id)
}

fn mark_cancelled(cancelled: &CancelSet, request_id: u64) {
    let mut guard = cancelled.lock().unwrap();
    guard.insert(request_id);
}

fn handle_cancel_request(
    envelope: &RequestEnvelope,
    cancelled: &CancelSet,
) -> Result<(), ExecError> {
    let payload = extract_payload(envelope).map_err(|err| ExecError {
        status: "InvalidInput",
        message: err,
    })?;
    let cancel =
        decode_payload::<CancelRequest>(&payload, &envelope.codec).map_err(|err| ExecError {
            status: "InvalidInput",
            message: err,
        })?;
    mark_cancelled(cancelled, cancel.request_id);
    Ok(())
}

fn timeout_error() -> ExecError {
    ExecError {
        status: "Timeout",
        message: "Request timed out".to_string(),
    }
}

fn cancelled_error() -> ExecError {
    ExecError {
        status: "Cancelled",
        message: "Request cancelled".to_string(),
    }
}

fn check_timeout(exec_start: Instant, timeout: Option<Duration>) -> Result<(), ExecError> {
    if let Some(limit) = timeout {
        if exec_start.elapsed() > limit {
            return Err(timeout_error());
        }
    }
    Ok(())
}

fn check_cancelled(cancelled: &CancelSet, request_id: u64) -> Result<(), ExecError> {
    if is_cancelled(cancelled, request_id) {
        return Err(cancelled_error());
    }
    Ok(())
}

fn sleep_with_cancel(
    delay: Duration,
    cancelled: &CancelSet,
    request_id: u64,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<(), ExecError> {
    let mut remaining = delay;
    let slice = Duration::from_millis(5);
    while remaining > Duration::ZERO {
        check_cancelled(cancelled, request_id)?;
        check_timeout(exec_start, timeout)?;
        let step = if remaining > slice { slice } else { remaining };
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
    Ok(())
}

fn list_items_response(
    request: &ListItemsRequest,
    cancelled: &CancelSet,
    request_id: u64,
    pool: &DbPool,
    timeout: Option<Duration>,
    exec_start: Instant,
    fake_delay: Duration,
) -> Result<ListItemsResponse, ExecError> {
    let _conn = acquire_connection(pool, timeout)?;
    check_timeout(exec_start, timeout)?;
    if fake_delay.as_millis() > 0 {
        sleep_with_cancel(fake_delay, cancelled, request_id, exec_start, timeout)?;
    }
    let limit = request.limit.unwrap_or(50).min(500) as usize;
    let q_len = request.q.as_ref().map(|q| q.len()).unwrap_or(0) as i64;
    let status_len = request
        .status
        .as_ref()
        .map(|status| status.len())
        .unwrap_or(0) as i64;
    let cursor_len = request
        .cursor
        .as_ref()
        .map(|cursor| cursor.len())
        .unwrap_or(0) as i64;
    let base = request.user_id.abs() * 1000 + q_len + status_len + cursor_len;
    let mut items = Vec::with_capacity(limit);
    let mut open = 0u32;
    let mut closed = 0u32;
    for idx in 0..limit {
        check_cancelled(cancelled, request_id)?;
        check_timeout(exec_start, timeout)?;
        let id = base + idx as i64;
        let is_open = idx % 2 == 0;
        let status = if is_open { "open" } else { "closed" };
        if is_open {
            open += 1;
        } else {
            closed += 1;
        }
        let created_at = format!("2026-01-{:02}T00:00:{:02}Z", (idx % 28) + 1, idx % 60);
        let title = format!("Item {id}");
        let score = (idx % 100) as f64 / 100.0;
        let unread = idx % 3 == 0;
        items.push(ItemRow {
            id,
            created_at,
            status: status.to_string(),
            title,
            score,
            unread,
        });
    }

    let next_cursor = if items.len() == limit {
        Some(format!("{}:{}", request.user_id, limit))
    } else {
        None
    };

    Ok(ListItemsResponse {
        items,
        next_cursor,
        counts: CountSummary { open, closed },
    })
}

#[allow(clippy::too_many_arguments)]
fn dispatch_compiled(
    entry: &CompiledEntry,
    payload_bytes: &[u8],
    request_id: u64,
    cancelled: &CancelSet,
    pool: &DbPool,
    timeout: Option<Duration>,
    exec_start: Instant,
    fake_delay: Duration,
) -> Result<(String, Vec<u8>), ExecError> {
    match entry.name.as_str() {
        "list_items" => {
            let req = decode_payload::<ListItemsRequest>(payload_bytes, &entry.codec_in).map_err(
                |err| ExecError {
                    status: "InvalidInput",
                    message: err,
                },
            )?;
            let response = list_items_response(
                &req, cancelled, request_id, pool, timeout, exec_start, fake_delay,
            )?;
            let encoded = encode_payload(&response, &entry.codec_out).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((entry.codec_out.clone(), encoded))
        }
        "compute" => {
            let req = decode_payload::<ComputeRequest>(payload_bytes, &entry.codec_in).map_err(
                |err| ExecError {
                    status: "InvalidInput",
                    message: err,
                },
            )?;
            let scale = req.scale.unwrap_or(1.0);
            let offset = req.offset.unwrap_or(0.0);
            let mut scaled = Vec::with_capacity(req.values.len());
            let mut sum = 0.0f64;
            for (idx, v) in req.values.iter().enumerate() {
                if idx % 1024 == 0 {
                    check_cancelled(cancelled, request_id)?;
                    check_timeout(exec_start, timeout)?;
                }
                let val = v * scale + offset;
                sum += val;
                // Avoid NaN propagation impacting the whole batch; keep parity with simple math.
                if !val.is_nan() {
                    scaled.push(val);
                } else {
                    scaled.push(f64::NAN);
                }
            }
            let response = ComputeResponse {
                count: scaled.len(),
                sum,
                scaled,
            };
            let encoded = encode_payload(&response, &entry.codec_out).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((entry.codec_out.clone(), encoded))
        }
        "offload_table" => {
            let req = decode_payload::<OffloadTableRequest>(payload_bytes, &entry.codec_in)
                .map_err(|err| ExecError {
                    status: "InvalidInput",
                    message: err,
                })?;
            check_cancelled(cancelled, request_id)?;
            check_timeout(exec_start, timeout)?;
            let rows = req.rows.min(50_000);
            let mut sample = Vec::with_capacity(rows.min(8));
            for i in 0..rows.min(8) {
                let mut row = HashMap::new();
                row.insert("id".to_string(), i as i64);
                row.insert("value".to_string(), (i % 7) as i64);
                sample.push(row);
            }
            let response = OffloadTableResponse { rows, sample };
            let encoded = encode_payload(&response, &entry.codec_out).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((entry.codec_out.clone(), encoded))
        }
        "health" => {
            check_cancelled(cancelled, request_id)?;
            let response = HealthResponse { ok: true };
            let encoded = encode_payload(&response, &entry.codec_out).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((entry.codec_out.clone(), encoded))
        }
        _ => Err(ExecError {
            status: "InternalError",
            message: format!("Compiled entry '{}' has no handler", entry.name),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_entry(
    envelope: &RequestEnvelope,
    cancelled: &CancelSet,
    pool: &DbPool,
    timeout: Option<Duration>,
    exec_start: Instant,
    exports: &HashSet<String>,
    compiled_entries: &HashMap<String, CompiledEntry>,
    fake_delay: Duration,
) -> Result<(String, Vec<u8>), ExecError> {
    let payload_bytes = extract_payload(envelope).map_err(|err| ExecError {
        status: "InvalidInput",
        message: err,
    })?;
    match envelope.entry.as_str() {
        "__ping__" => Ok(("raw".to_string(), Vec::new())),
        "list_items" => {
            let req = decode_payload::<ListItemsRequest>(&payload_bytes, &envelope.codec).map_err(
                |err| ExecError {
                    status: "InvalidInput",
                    message: err,
                },
            )?;
            let response = list_items_response(
                &req,
                cancelled,
                envelope.request_id,
                pool,
                timeout,
                exec_start,
                fake_delay,
            )?;
            let codec = envelope.codec.as_str();
            let encoded = encode_payload(&response, codec).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((codec.to_string(), encoded))
        }
        _ => {
            if let Some(entry) = compiled_entries.get(&envelope.entry) {
                return dispatch_compiled(
                    entry,
                    &payload_bytes,
                    envelope.request_id,
                    cancelled,
                    pool,
                    timeout,
                    exec_start,
                    fake_delay,
                );
            }
            if exports.contains(&envelope.entry) {
                return Err(ExecError {
                    status: "InternalError",
                    message: format!(
                        "Compiled entrypoints not yet wired (entry '{}')",
                        envelope.entry
                    ),
                });
            }
            Err(ExecError {
                status: "InvalidInput",
                message: format!("Unknown entry '{}'.", envelope.entry),
            })
        }
    }
}

fn handle_request(
    request: DecodedRequest,
    queue_depth: usize,
    exports: &HashSet<String>,
    cancelled: &CancelSet,
    pool: &DbPool,
    compiled_entries: &HashMap<String, CompiledEntry>,
    fake_delay: Duration,
) -> (WireCodec, ResponseEnvelope) {
    let wire = request.wire;
    let envelope = request.envelope;
    let request_id = envelope.request_id;
    let exec_start = Instant::now();
    let queue_ms = exec_start
        .duration_since(request.queued_at)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let mut metrics = HashMap::new();
    metrics.insert("queue_ms".to_string(), queue_ms);
    metrics.insert("queue_depth".to_string(), queue_depth as u64);
    metrics.insert("pool_in_flight".to_string(), pool.in_flight() as u64);
    metrics.insert("pool_idle".to_string(), pool.idle_count() as u64);
    metrics.insert(
        "payload_bytes".to_string(),
        envelope.payload.as_ref().map(|p| p.len()).unwrap_or(0) as u64,
    );
    if is_cancelled(cancelled, request_id) {
        return (
            wire,
            ResponseEnvelope {
                request_id,
                status: "Cancelled".to_string(),
                codec: "raw".to_string(),
                payload: None,
                metrics: Some(metrics),
                error: Some("Request cancelled".to_string()),
                entry: Some(envelope.entry.clone()),
                compiled: Some(0),
            },
        );
    }
    if !envelope.entry.starts_with("__") && !exports.contains(&envelope.entry) {
        return (
            wire,
            ResponseEnvelope {
                request_id,
                status: "InvalidInput".to_string(),
                codec: "raw".to_string(),
                payload: None,
                metrics: Some(metrics),
                error: Some(format!("Unknown entry '{}'", envelope.entry)),
                entry: Some(envelope.entry.clone()),
                compiled: Some(0),
            },
        );
    }

    let timeout = if envelope.timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(envelope.timeout_ms as u64))
    };
    let entry_name = envelope.entry.clone();
    let compiled_flag = compiled_entries.contains_key(&entry_name) as u64;
    let result = execute_entry(
        &envelope,
        cancelled,
        pool,
        timeout,
        exec_start,
        exports,
        compiled_entries,
        fake_delay,
    );
    let exec_ms = exec_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    metrics.insert("exec_ms".to_string(), exec_ms);
    if let Some(limit) = timeout {
        if exec_start.elapsed() > limit {
            return (
                wire,
                ResponseEnvelope {
                    request_id,
                    status: "Timeout".to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: Some(metrics),
                    error: Some("Request timed out".to_string()),
                    entry: Some(envelope.entry.clone()),
                    compiled: Some(compiled_entries.contains_key(&envelope.entry) as u64),
                },
            );
        }
    }

    match result {
        Ok((codec, payload)) => (
            wire,
            ResponseEnvelope {
                request_id,
                status: "Ok".to_string(),
                codec,
                payload: Some(ByteBuf::from(payload)),
                metrics: Some(metrics),
                error: None,
                entry: Some(entry_name),
                compiled: Some(compiled_flag),
            },
        ),
        Err(err) => (
            wire,
            ResponseEnvelope {
                request_id,
                status: err.status.to_string(),
                codec: "raw".to_string(),
                payload: None,
                metrics: Some(metrics),
                error: Some(err.message),
                entry: Some(entry_name),
                compiled: Some(compiled_flag),
            },
        ),
    }
}

fn response_with_status(wire: WireCodec, request_id: u64, status: &'static str, error: &str) {
    let mut stdout = io::stdout();
    let response = ResponseEnvelope {
        request_id,
        status: status.to_string(),
        codec: "raw".to_string(),
        payload: None,
        metrics: None,
        error: Some(error.to_string()),
        entry: None,
        compiled: None,
    };
    if let Ok(encoded) = encode_response(&response, wire) {
        let _ = write_frame(&mut stdout, &encoded);
    }
}

fn load_exports(path: &str) -> Result<HashSet<String>, String> {
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let manifest: ExportsManifest =
        serde_json::from_str(&content).map_err(|err| err.to_string())?;
    let mut exports = HashSet::new();
    for entry in manifest.exports {
        let name = entry.name.trim();
        if name.is_empty() {
            eprintln!("Invalid export name: empty");
            continue;
        }
        if name.starts_with("__") {
            eprintln!("Invalid export name (reserved): {name}");
            continue;
        }
        if !exports.insert(name.to_string()) {
            eprintln!("Duplicate export name: {name}");
        }
    }
    Ok(exports)
}

fn main() -> io::Result<()> {
    let mut exports_path = None;
    let mut compiled_exports = None;
    let mut max_queue = 64usize;
    let mut threads = None;
    let fake_delay_ms = env::var("MOLT_FAKE_DB_DELAY_MS")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or(0);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exports" => exports_path = args.next(),
            "--compiled-exports" => compiled_exports = args.next().map(PathBuf::from),
            "--max-queue" => {
                if let Some(val) = args.next() {
                    max_queue = val.parse().unwrap_or(64)
                }
            }
            "--threads" => {
                if let Some(val) = args.next() {
                    threads = val.parse().ok();
                }
            }
            "--stdio" => {}
            _ => {}
        }
    }

    let exports = if let Some(path) = exports_path.as_deref() {
        match load_exports(path) {
            Ok(exports) => exports,
            Err(err) => {
                eprintln!("Failed to load exports: {err}");
                HashSet::new()
            }
        }
    } else {
        HashSet::new()
    };
    let exports = Arc::new(exports);
    let compiled_entries =
        load_compiled_entries(compiled_exports).unwrap_or_else(|_| HashMap::new());
    let compiled_entries = Arc::new(compiled_entries);
    let cancelled = Arc::new(Mutex::new(HashSet::new()));

    let (request_tx, request_rx) = bounded::<DecodedRequest>(max_queue);
    let (response_tx, response_rx) = bounded::<(WireCodec, ResponseEnvelope)>(max_queue);

    let thread_count = threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4)
    });

    let pool_size = env::var("MOLT_DB_POOL")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(thread_count.max(1));
    let conn_counter = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(pool_size, {
        let counter = conn_counter.clone();
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            FakeDbConn
        }
    });
    let fake_delay = Duration::from_millis(fake_delay_ms);

    for _ in 0..thread_count {
        let request_rx = request_rx.clone();
        let response_tx = response_tx.clone();
        let exports = exports.clone();
        let cancelled = cancelled.clone();
        let pool = pool.clone();
        let compiled_entries = compiled_entries.clone();
        let delay = fake_delay;
        thread::spawn(move || {
            worker_loop(
                request_rx,
                response_tx,
                exports,
                cancelled,
                pool,
                compiled_entries,
                delay,
            )
        });
    }

    let writer = thread::spawn(move || write_loop(response_rx));

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(err) => {
                response_with_status(WireCodec::Json, 0, "InvalidInput", &err.to_string());
                break;
            }
        };
        let decoded = match decode_request(&frame) {
            Ok(decoded) => decoded,
            Err(err) => {
                response_with_status(WireCodec::Json, 0, "InvalidInput", &err);
                continue;
            }
        };
        if decoded.envelope.entry == "__cancel__" {
            let response = match handle_cancel_request(&decoded.envelope, &cancelled) {
                Ok(()) => ResponseEnvelope {
                    request_id: decoded.envelope.request_id,
                    status: "Ok".to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: None,
                    error: None,
                    entry: Some("__cancel__".to_string()),
                    compiled: Some(0),
                },
                Err(err) => ResponseEnvelope {
                    request_id: decoded.envelope.request_id,
                    status: err.status.to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: None,
                    error: Some(err.message),
                    entry: Some("__cancel__".to_string()),
                    compiled: Some(0),
                },
            };
            let _ = response_tx.send((decoded.wire, response));
            continue;
        }
        match request_tx.try_send(decoded) {
            Ok(()) => {}
            Err(TrySendError::Full(request)) => {
                response_with_status(
                    request.wire,
                    request.envelope.request_id,
                    "Busy",
                    "Worker queue full",
                );
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
    }

    drop(request_tx);
    drop(response_tx);
    let _ = writer.join();
    Ok(())
}

fn worker_loop(
    request_rx: Receiver<DecodedRequest>,
    response_tx: Sender<(WireCodec, ResponseEnvelope)>,
    exports: Arc<HashSet<String>>,
    cancelled: CancelSet,
    pool: DbPool,
    compiled_entries: Arc<HashMap<String, CompiledEntry>>,
    fake_delay: Duration,
) {
    // TODO(offload, owner:runtime, milestone:SL1): propagate cancellation into pool waits and real DB tasks.
    while let Ok(request) = request_rx.recv() {
        let queue_depth = request_rx.len();
        let response = handle_request(
            request,
            queue_depth,
            &exports,
            &cancelled,
            &pool,
            &compiled_entries,
            fake_delay,
        );
        let _ = response_tx.send(response);
    }
}

fn write_loop(response_rx: Receiver<(WireCodec, ResponseEnvelope)>) {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    while let Ok((wire, response)) = response_rx.recv() {
        let encoded = match encode_response(&response, wire) {
            Ok(encoded) => encoded,
            Err(err) => {
                eprintln!("Failed to encode response: {err}");
                continue;
            }
        };
        if let Err(err) = write_frame(&mut writer, &encoded) {
            eprintln!("Failed to write response: {err}");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dispatch_compiled, load_compiled_entries, load_exports, mark_cancelled, CancelSet,
        CompiledEntry, DbPool, ListItemsRequest, ListItemsResponse, Pool,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn temp_manifest(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let name = format!("molt_exports_{}_{}.json", std::process::id(), rand_id());
        path.push(name);
        fs::write(&path, contents).expect("write manifest");
        path
    }

    fn rand_id() -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time");
        now.as_nanos() as u64
    }

    #[test]
    fn load_exports_filters_reserved_and_duplicates() {
        let path = temp_manifest(
            r#"{"exports":[{"name":"list_items"},{"name":"__ping__"},{"name":"list_items"},{"name":"  "},{"name":"compute"}]}"#,
        );
        let exports = load_exports(path.to_str().expect("path")).expect("exports");
        let _ = fs::remove_file(&path);
        assert!(exports.contains("list_items"));
        assert!(exports.contains("compute"));
        assert!(!exports.contains("__ping__"));
        assert_eq!(exports.len(), 2);
    }

    #[test]
    fn compiled_dispatch_roundtrip_list_items() {
        let entry = CompiledEntry {
            name: "list_items".to_string(),
            codec_in: "msgpack".to_string(),
            codec_out: "msgpack".to_string(),
        };
        let cancel = CancelSet::default();
        let pool: DbPool = Pool::new(1, || super::FakeDbConn);
        let req = ListItemsRequest {
            user_id: 7,
            q: None,
            status: None,
            limit: Some(5),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let result = dispatch_compiled(
            &entry,
            &payload,
            7,
            &cancel,
            &pool,
            None,
            Instant::now(),
            Duration::from_millis(0),
        )
        .expect("compiled dispatch");
        assert_eq!(result.0, "msgpack");
        let decoded: ListItemsResponse =
            super::decode_payload(&result.1, "msgpack").expect("decode");
        assert_eq!(decoded.items.len(), 5);
        assert_eq!(decoded.counts.open + decoded.counts.closed, 5);
    }

    #[test]
    fn compiled_dispatch_cancelled() {
        let entry = CompiledEntry {
            name: "list_items".to_string(),
            codec_in: "msgpack".to_string(),
            codec_out: "msgpack".to_string(),
        };
        let cancel = CancelSet::default();
        mark_cancelled(&cancel, 42);
        let pool: DbPool = Pool::new(1, || super::FakeDbConn);
        let req = ListItemsRequest {
            user_id: 1,
            q: None,
            status: None,
            limit: Some(1),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let result = dispatch_compiled(
            &entry,
            &payload,
            42,
            &cancel,
            &pool,
            None,
            Instant::now(),
            Duration::from_millis(0),
        );
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().status, "Cancelled");
    }

    #[test]
    fn compiled_manifest_roundtrip() {
        let manifest =
            r#"{"exports":[{"name":"list_items","codec_in":"msgpack","codec_out":"msgpack"}]}"#;
        let path = temp_manifest(manifest);
        let map = load_compiled_entries(Some(path.clone())).expect("manifest");
        let _ = fs::remove_file(&path);
        let entry = map.get("list_items").expect("list_items entry");
        assert_eq!(entry.codec_in, "msgpack");
        let cancel = CancelSet::default();
        let pool: DbPool = Pool::new(1, || super::FakeDbConn);
        let req = ListItemsRequest {
            user_id: 3,
            q: Some("x".into()),
            status: Some("open".into()),
            limit: Some(2),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let result = dispatch_compiled(
            entry,
            &payload,
            3,
            &cancel,
            &pool,
            None,
            Instant::now(),
            Duration::from_millis(0),
        )
        .expect("dispatch");
        assert_eq!(result.0, "msgpack");
        let decoded: ListItemsResponse =
            super::decode_payload(&result.1, "msgpack").expect("decode");
        assert_eq!(decoded.items.len(), 2);
    }

    #[test]
    fn compiled_manifest_unknown_entry_errors() {
        let manifest =
            r#"{"exports":[{"name":"unknown","codec_in":"msgpack","codec_out":"msgpack"}]}"#;
        let path = temp_manifest(manifest);
        let map = load_compiled_entries(Some(path.clone())).expect("manifest");
        let _ = fs::remove_file(&path);
        let entry = map.get("unknown").expect("unknown entry");
        let cancel = CancelSet::default();
        let pool: DbPool = Pool::new(1, || super::FakeDbConn);
        let payload = super::encode_payload(
            &ListItemsRequest {
                user_id: 1,
                q: None,
                status: None,
                limit: None,
                cursor: None,
            },
            "msgpack",
        )
        .expect("encode");
        let result = dispatch_compiled(
            entry,
            &payload,
            1,
            &cancel,
            &pool,
            None,
            Instant::now(),
            Duration::from_millis(0),
        );
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().status, "InternalError");
    }
}
