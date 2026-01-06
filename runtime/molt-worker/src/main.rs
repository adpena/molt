use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
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
    payload: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<HashMap<String, u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
}

#[derive(Deserialize)]
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

#[derive(Serialize)]
struct ItemRow {
    id: i64,
    created_at: String,
    status: String,
    title: String,
    score: f64,
    unread: bool,
}

#[derive(Serialize)]
struct CountSummary {
    open: u32,
    closed: u32,
}

#[derive(Serialize)]
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

struct ExecError {
    status: &'static str,
    message: String,
}

struct FakeDbConn;

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
                .map(|payload| BASE64.encode(payload));
            let json = ResponseEnvelopeJson {
                request_id: response.request_id,
                status: response.status.clone(),
                codec: response.codec.clone(),
                payload_b64,
                metrics: response.metrics.clone(),
                error: response.error.clone(),
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

fn list_items_response(
    request: &ListItemsRequest,
    cancelled: &CancelSet,
    request_id: u64,
    pool: &DbPool,
    timeout: Option<Duration>,
    fake_delay: Duration,
) -> Result<ListItemsResponse, ExecError> {
    let _conn = acquire_connection(pool, timeout)?;
    if fake_delay.as_millis() > 0 {
        thread::sleep(fake_delay);
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
        if is_cancelled(cancelled, request_id) {
            return Err(ExecError {
                status: "Cancelled",
                message: "Request cancelled".to_string(),
            });
        }
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

fn execute_entry(
    envelope: &RequestEnvelope,
    cancelled: &CancelSet,
    pool: &DbPool,
    timeout: Option<Duration>,
    fake_delay: Duration,
) -> Result<(String, Vec<u8>), ExecError> {
    // TODO(offload, owner:runtime, milestone:SL1): dispatch to compiled entrypoints.
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
                fake_delay,
            )?;
            let codec = envelope.codec.as_str();
            let encoded = encode_payload(&response, codec).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((codec.to_string(), encoded))
        }
        _ => Err(ExecError {
            status: "InvalidInput",
            message: format!("Unknown entry '{}'.", envelope.entry),
        }),
    }
}

fn handle_request(
    request: DecodedRequest,
    exports: &HashSet<String>,
    cancelled: &CancelSet,
    pool: &DbPool,
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
            },
        );
    }

    let timeout = if envelope.timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(envelope.timeout_ms as u64))
    };
    let result = execute_entry(&envelope, cancelled, pool, timeout, fake_delay);
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
                payload: Some(payload),
                metrics: Some(metrics),
                error: None,
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
    };
    if let Ok(encoded) = encode_response(&response, wire) {
        let _ = write_frame(&mut stdout, &encoded);
    }
}

fn load_exports(path: &str) -> Result<HashSet<String>, String> {
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let manifest: ExportsManifest =
        serde_json::from_str(&content).map_err(|err| err.to_string())?;
    Ok(manifest
        .exports
        .into_iter()
        .map(|entry| entry.name)
        .collect())
}

fn main() -> io::Result<()> {
    let mut exports_path = None;
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
        let delay = fake_delay;
        thread::spawn(move || {
            worker_loop(request_rx, response_tx, exports, cancelled, pool, delay)
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
                },
                Err(err) => ResponseEnvelope {
                    request_id: decoded.envelope.request_id,
                    status: err.status.to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: None,
                    error: Some(err.message),
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
    fake_delay: Duration,
) {
    // TODO(offload, owner:runtime, milestone:SL1): propagate cancellation into compiled entrypoints and DB tasks.
    while let Ok(request) = request_rx.recv() {
        let response = handle_request(request, &exports, &cancelled, &pool, fake_delay);
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
