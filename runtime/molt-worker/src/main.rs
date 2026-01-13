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
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use molt_db::{
    AcquireError, AsyncAcquireError, CancelToken, PgPool, PgPoolConfig, Pool, Pooled, SqliteConn,
    SqliteOpenMode,
};
use rusqlite::{params_from_iter, types::Value, types::ValueRef};
use sqlparser::ast::{
    Expr as SqlExpr, GroupByExpr as SqlGroupByExpr, Ident as SqlIdent, Query as SqlQuery,
    Select as SqlSelect, SelectItem as SqlSelectItem, SetExpr as SqlSetExpr,
    Statement as SqlStatement, TableAlias as SqlTableAlias, TableFactor as SqlTableFactor,
    TableWithJoins as SqlTableWithJoins, Value as SqlValue,
    WildcardAdditionalOptions as SqlWildcardAdditionalOptions,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Builder as TokioRuntimeBuilder;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tokio::time::{sleep as tokio_sleep};
use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::Row as PgRow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireCodec {
    Json,
    Msgpack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerRuntime {
    Sync,
    Async,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParamStyle {
    DollarNumbered,
    QuestionNumbered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DbResultFormat {
    Json,
    Msgpack,
    ArrowIpc,
}

impl DbResultFormat {
    fn parse(value: Option<&str>) -> Result<Self, ExecError> {
        let format = value
            .unwrap_or("json")
            .trim()
            .to_lowercase();
        match format.as_str() {
            "json" => Ok(Self::Json),
            "msgpack" => Ok(Self::Msgpack),
            "arrow_ipc" => Ok(Self::ArrowIpc),
            _ => Err(ExecError {
                status: "InvalidInput",
                message: format!("Unsupported result_format '{format}'"),
            }),
        }
    }

    fn codec(self) -> &'static str {
        match self {
            DbResultFormat::Json => "json",
            DbResultFormat::Msgpack => "msgpack",
            DbResultFormat::ArrowIpc => "arrow_ipc",
        }
    }
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

#[derive(Clone, Deserialize)]
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

#[allow(dead_code)]
#[derive(Deserialize)]
struct DbQueryRequest {
    #[serde(default)]
    db_alias: Option<String>,
    sql: String,
    #[serde(default)]
    params: DbParams,
    #[serde(default)]
    max_rows: Option<u32>,
    #[serde(default)]
    result_format: Option<String>,
    #[serde(default)]
    allow_write: Option<bool>,
    #[serde(default)]
    tag: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum DbParams {
    Positional { values: Vec<DbParam> },
    Named { values: Vec<DbNamedParam> },
}

impl Default for DbParams {
    fn default() -> Self {
        DbParams::Positional { values: Vec::new() }
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(untagged)]
enum DbParam {
    Raw(DbParamValue),
    Typed {
        value: DbParamValue,
        #[serde(default)]
        r#type: Option<String>,
    },
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct DbNamedParam {
    name: String,
    #[serde(flatten)]
    param: DbParam,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
enum DbParamValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
}

impl<'de> Deserialize<'de> for DbParamValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DbParamVisitor;

        impl<'de> serde::de::Visitor<'de> for DbParamVisitor {
            type Value = DbParamValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("null, bool, int, float, string, or bytes")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(DbParamValue::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(DbParamValue::Int(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value > i64::MAX as u64 {
                    return Err(E::custom("integer out of range"));
                }
                Ok(DbParamValue::Int(value as i64))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(DbParamValue::Float(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(DbParamValue::String(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(DbParamValue::String(value))
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(DbParamValue::Bytes(value.to_vec()))
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                Ok(DbParamValue::Bytes(value))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(DbParamValue::Null)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(DbParamValue::Null)
            }
        }

        deserializer.deserialize_any(DbParamVisitor)
    }
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

#[allow(dead_code)]
#[derive(Serialize)]
struct DbQueryResponse {
    columns: Vec<String>,
    rows: Vec<Vec<DbRowValue>>,
    row_count: usize,
}

#[allow(dead_code)]
#[derive(Serialize)]
#[serde(untagged)]
enum DbRowValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(ByteBuf),
}

struct DecodedRequest {
    envelope: RequestEnvelope,
    wire: WireCodec,
    queued_at: Instant,
    decode_us: u64,
}

type CancelSet = Arc<Mutex<HashSet<u64>>>;
#[allow(dead_code)]
type AsyncCancelRegistry = Arc<CancelRegistry>;

type DbPool = Arc<Pool<DbConn>>;

struct CancelRegistry {
    pending: Mutex<HashSet<u64>>,
    tokens: Mutex<HashMap<u64, CancelToken>>,
}

#[allow(dead_code)]
impl CancelRegistry {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashSet::new()),
            tokens: Mutex::new(HashMap::new()),
        }
    }

    fn register(&self, request_id: u64) -> CancelToken {
        let token = CancelToken::new();
        {
            let mut tokens = self.tokens.lock().unwrap();
            tokens.insert(request_id, token.clone());
        }
        let cancelled = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(&request_id)
        };
        if cancelled {
            token.cancel();
        }
        token
    }

    fn mark_cancelled(&self, request_id: u64) {
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(request_id);
        }
        let token = {
            let tokens = self.tokens.lock().unwrap();
            tokens.get(&request_id).cloned()
        };
        if let Some(token) = token {
            token.cancel();
        }
    }

    fn take_cancelled(&self, request_id: u64) -> bool {
        let mut pending = self.pending.lock().unwrap();
        pending.remove(&request_id)
    }

    fn clear(&self, request_id: u64) {
        {
            let mut tokens = self.tokens.lock().unwrap();
            tokens.remove(&request_id);
        }
        let mut pending = self.pending.lock().unwrap();
        pending.remove(&request_id);
    }
}

#[derive(Clone)]
struct WorkerContext {
    exports: Arc<HashSet<String>>,
    cancelled: CancelSet,
    pool: DbPool,
    compiled_entries: Arc<HashMap<String, CompiledEntry>>,
    fake_delay: Duration,
    fake_decode_us_per_row: u64,
    fake_cpu_iters: u32,
    default_max_rows: u32,
}

struct ExecContext<'a> {
    cancelled: &'a CancelSet,
    request_id: u64,
    pool: &'a DbPool,
    timeout: Option<Duration>,
    exec_start: Instant,
    fake_delay: Duration,
    fake_decode_us_per_row: u64,
    fake_cpu_iters: u32,
    default_max_rows: u32,
}

#[derive(Clone)]
struct AsyncWorkerContext {
    exports: Arc<HashSet<String>>,
    cancelled: CancelSet,
    cancel_registry: AsyncCancelRegistry,
    pool: DbPool,
    pg_pool: Option<Arc<PgPool>>,
    compiled_entries: Arc<HashMap<String, CompiledEntry>>,
    fake_delay: Duration,
    fake_decode_us_per_row: u64,
    fake_cpu_iters: u32,
    default_max_rows: u32,
}

struct AsyncExecContext<'a> {
    cancelled: &'a CancelSet,
    cancel_token: CancelToken,
    request_id: u64,
    pool: &'a DbPool,
    pg_pool: Option<&'a PgPool>,
    timeout: Option<Duration>,
    exec_start: Instant,
    fake_delay: Duration,
    fake_decode_us_per_row: u64,
    fake_cpu_iters: u32,
    default_max_rows: u32,
}

#[derive(Debug)]
struct ExecError {
    status: &'static str,
    message: String,
}

#[allow(dead_code)]
enum PgParam {
    Bool(bool),
    Int2(i16),
    Int4(i32),
    Int8(i64),
    Float4(f32),
    Float8(f64),
    String(String),
    Bytes(Vec<u8>),
    NullBool(Option<bool>),
    NullInt2(Option<i16>),
    NullInt4(Option<i32>),
    NullInt8(Option<i64>),
    NullFloat4(Option<f32>),
    NullFloat8(Option<f64>),
    NullString(Option<String>),
    NullBytes(Option<Vec<u8>>),
}

#[allow(dead_code)]
impl PgParam {
    fn as_tosql(&self) -> &(dyn ToSql + Sync) {
        match self {
            PgParam::Bool(value) => value,
            PgParam::Int2(value) => value,
            PgParam::Int4(value) => value,
            PgParam::Int8(value) => value,
            PgParam::Float4(value) => value,
            PgParam::Float8(value) => value,
            PgParam::String(value) => value,
            PgParam::Bytes(value) => value,
            PgParam::NullBool(value) => value,
            PgParam::NullInt2(value) => value,
            PgParam::NullInt4(value) => value,
            PgParam::NullInt8(value) => value,
            PgParam::NullFloat4(value) => value,
            PgParam::NullFloat8(value) => value,
            PgParam::NullString(value) => value,
            PgParam::NullBytes(value) => value,
        }
    }
}

enum DbConn {
    Fake(FakeDbConn),
    Sqlite(SqliteConn),
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
    let start = Instant::now();
    if let Ok(env) = rmp_serde::from_slice::<RequestEnvelope>(bytes) {
        let decode_us = start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        return Ok(DecodedRequest {
            envelope: env,
            wire: WireCodec::Msgpack,
            queued_at: Instant::now(),
            decode_us,
        });
    }
    let env = serde_json::from_slice::<RequestEnvelope>(bytes)
        .map_err(|err| format!("Invalid request: {err}"))?;
    let decode_us = start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    Ok(DecodedRequest {
        envelope: env,
        wire: WireCodec::Json,
        queued_at: Instant::now(),
        decode_us,
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
    cancelled: &CancelSet,
    request_id: u64,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<Pooled<DbConn>, ExecError> {
    check_timeout(exec_start, timeout)?;
    let remaining = timeout.map(|limit| limit.saturating_sub(exec_start.elapsed()));
    match pool.acquire_with_cancel(remaining, || is_cancelled(cancelled, request_id)) {
        Ok(conn) => Ok(conn),
        Err(AcquireError::Cancelled) => Err(cancelled_error()),
        Err(AcquireError::Timeout) => Err(timeout_error()),
    }
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
    async_cancel: Option<&CancelRegistry>,
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
    if let Some(registry) = async_cancel {
        registry.mark_cancelled(cancel.request_id);
    }
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

fn burn_cpu(iterations: u32, seed: u64) {
    if iterations == 0 {
        return;
    }
    let mut acc = seed;
    for i in 0..iterations {
        acc = acc
            .wrapping_mul(1664525)
            .wrapping_add(1013904223u64.wrapping_add(i as u64));
    }
    black_box(acc);
}

fn sleep_decode_cost(
    rows: usize,
    decode_us_per_row: u64,
    cancelled: &CancelSet,
    request_id: u64,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<(), ExecError> {
    if decode_us_per_row == 0 || rows == 0 {
        return Ok(());
    }
    let micros = decode_us_per_row.saturating_mul(rows as u64);
    if micros == 0 {
        return Ok(());
    }
    sleep_with_cancel(
        Duration::from_micros(micros),
        cancelled,
        request_id,
        exec_start,
        timeout,
    )
}

#[allow(dead_code)]
#[derive(Clone)]
struct DbParamSpec {
    value: DbParamValue,
    type_hint: Option<String>,
}

#[allow(dead_code)]
fn parse_param_spec(param: DbParam) -> DbParamSpec {
    match param {
        DbParam::Raw(value) => DbParamSpec {
            value,
            type_hint: None,
        },
        DbParam::Typed { value, r#type } => DbParamSpec {
            value,
            type_hint: r#type,
        },
    }
}

#[allow(dead_code)]
fn normalize_params_and_sql(
    sql: &str,
    params: DbParams,
    style: ParamStyle,
) -> Result<(String, Vec<DbParamSpec>), ExecError> {
    match params {
        DbParams::Positional { values } => {
            let specs = values.into_iter().map(parse_param_spec).collect();
            Ok((sql.to_string(), specs))
        }
        DbParams::Named { values } => normalize_named_params(sql, values, style),
    }
}

#[allow(dead_code)]
fn normalize_named_params(
    sql: &str,
    params: Vec<DbNamedParam>,
    style: ParamStyle,
) -> Result<(String, Vec<DbParamSpec>), ExecError> {
    let mut map = HashMap::new();
    for param in params {
        let name = param.name.trim().to_string();
        if name.is_empty() {
            return Err(ExecError {
                status: "InvalidInput",
                message: "Named params must use non-empty names".to_string(),
            });
        }
        if map.contains_key(&name) {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Duplicate named param '{name}'"),
            });
        }
        map.insert(name, parse_param_spec(param.param));
    }

    let mut ordered = Vec::new();
    let mut index_map: HashMap<String, usize> = HashMap::new();
    let mut used = HashSet::new();
    let mut out = String::with_capacity(sql.len());

    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut state = SqlScanState::Normal;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match state {
            SqlScanState::Normal => match ch {
                '\'' => {
                    out.push(ch);
                    state = SqlScanState::SingleQuote;
                    i += 1;
                }
                '"' => {
                    out.push(ch);
                    state = SqlScanState::DoubleQuote;
                    i += 1;
                }
                '-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                    out.push_str("--");
                    state = SqlScanState::LineComment;
                    i += 2;
                }
                '/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                    out.push_str("/*");
                    state = SqlScanState::BlockComment;
                    i += 2;
                }
                '$' => {
                    if let Some(tag) = parse_dollar_tag(sql, i) {
                        out.push_str(&tag);
                        state = SqlScanState::DollarQuote(tag);
                        i += state.len();
                    } else {
                        out.push(ch);
                        i += 1;
                    }
                }
                ':' => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                        out.push_str("::");
                        i += 2;
                        continue;
                    }
                    if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                        out.push_str(":=");
                        i += 2;
                        continue;
                    }
                    if i + 1 < bytes.len() && is_ident_start(bytes[i + 1] as char) {
                        let start = i + 1;
                        let mut end = start + 1;
                        while end < bytes.len() && is_ident_continue(bytes[end] as char) {
                            end += 1;
                        }
                        let name = &sql[start..end];
                        let spec = map.get(name).ok_or_else(|| ExecError {
                            status: "InvalidInput",
                            message: format!("Missing param value for '{name}'"),
                        })?;
                        let idx = if let Some(existing) = index_map.get(name) {
                            *existing
                        } else {
                            let next_idx = ordered.len() + 1;
                            ordered.push(spec.clone());
                            index_map.insert(name.to_string(), next_idx);
                            used.insert(name.to_string());
                            next_idx
                        };
                        match style {
                            ParamStyle::DollarNumbered => {
                                out.push('$');
                                out.push_str(&idx.to_string());
                            }
                            ParamStyle::QuestionNumbered => {
                                out.push('?');
                                out.push_str(&idx.to_string());
                            }
                        }
                        i = end;
                    } else {
                        out.push(ch);
                        i += 1;
                    }
                }
                _ => {
                    out.push(ch);
                    i += 1;
                }
            },
            SqlScanState::SingleQuote => {
                out.push(ch);
                if ch == '\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        out.push('\'');
                        i += 2;
                    } else {
                        state = SqlScanState::Normal;
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            SqlScanState::DoubleQuote => {
                out.push(ch);
                if ch == '"' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                        out.push('"');
                        i += 2;
                    } else {
                        state = SqlScanState::Normal;
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            SqlScanState::LineComment => {
                out.push(ch);
                i += 1;
                if ch == '\n' {
                    state = SqlScanState::Normal;
                }
            }
            SqlScanState::BlockComment => {
                out.push(ch);
                if ch == '*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push('/');
                    i += 2;
                    state = SqlScanState::Normal;
                } else {
                    i += 1;
                }
            }
            SqlScanState::DollarQuote(ref tag) => {
                out.push(ch);
                if ch == '$' && sql[i..].starts_with(tag) {
                    out.push_str(&tag[1..]);
                    i += tag.len();
                    state = SqlScanState::Normal;
                } else {
                    i += 1;
                }
            }
        }
    }

    if used.len() != map.len() {
        let unused: Vec<String> = map
            .keys()
            .filter(|name| !used.contains(*name))
            .cloned()
            .collect();
        return Err(ExecError {
            status: "InvalidInput",
            message: format!("Unused params: {}", unused.join(", ")),
        });
    }

    Ok((out, ordered))
}

#[allow(dead_code)]
#[derive(Clone)]
enum SqlScanState {
    Normal,
    SingleQuote,
    DoubleQuote,
    LineComment,
    BlockComment,
    DollarQuote(String),
}

#[allow(dead_code)]
impl SqlScanState {
    fn len(&self) -> usize {
        match self {
            SqlScanState::DollarQuote(tag) => tag.len(),
            _ => 1,
        }
    }
}

#[allow(dead_code)]
fn parse_dollar_tag(sql: &str, start: usize) -> Option<String> {
    let bytes = sql.as_bytes();
    if bytes[start] != b'$' {
        return None;
    }
    let mut end = start + 1;
    while end < bytes.len() && bytes[end] != b'$' {
        let ch = bytes[end] as char;
        if !is_ident_continue(ch) {
            return None;
        }
        end += 1;
    }
    if end >= bytes.len() || bytes[end] != b'$' {
        return None;
    }
    Some(sql[start..=end].to_string())
}

#[allow(dead_code)]
fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

#[allow(dead_code)]
fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

#[allow(dead_code)]
fn validate_query(sql: &str, max_rows: u32, allow_write: bool) -> Result<String, ExecError> {
    let dialect = PostgreSqlDialect {};
    let mut statements = Parser::parse_sql(&dialect, sql).map_err(|err| ExecError {
        status: "InvalidInput",
        message: format!("SQL parse error: {err}"),
    })?;
    if statements.len() != 1 {
        return Err(ExecError {
            status: "InvalidInput",
            message: "SQL must contain exactly one statement".to_string(),
        });
    }
    let stmt = statements.remove(0);
    match stmt {
        SqlStatement::Query(query) => {
            let wrapped = wrap_query_limit(*query, max_rows);
            Ok(wrapped.to_string())
        }
        _ if allow_write => Err(ExecError {
            status: "InvalidInput",
            message: "Write statements require db_exec (not yet implemented)".to_string(),
        }),
        _ => Err(ExecError {
            status: "InvalidInput",
            message: "Only read-only SELECT/CTE queries are supported".to_string(),
        }),
    }
}

#[allow(dead_code)]
fn wrap_query_limit(query: SqlQuery, max_rows: u32) -> SqlStatement {
    let subquery = SqlQuery {
        body: query.body,
        order_by: query.order_by,
        limit: query.limit,
        limit_by: query.limit_by,
        offset: query.offset,
        fetch: query.fetch,
        locks: query.locks,
        for_clause: query.for_clause,
        with: query.with,
    };
    let select = SqlSelect {
        distinct: None,
        top: None,
        projection: vec![SqlSelectItem::Wildcard(
            SqlWildcardAdditionalOptions::default(),
        )],
        into: None,
        from: vec![SqlTableWithJoins {
            relation: SqlTableFactor::Derived {
                lateral: false,
                subquery: Box::new(subquery),
                alias: Some(SqlTableAlias {
                    name: SqlIdent::new("_molt_sub"),
                    columns: Vec::new(),
                }),
            },
            joins: Vec::new(),
        }],
        lateral_views: Vec::new(),
        selection: None,
        group_by: SqlGroupByExpr::Expressions(Vec::new()),
        cluster_by: Vec::new(),
        distribute_by: Vec::new(),
        sort_by: Vec::new(),
        having: None,
        named_window: Vec::new(),
        qualify: None,
        window_before_qualify: false,
        value_table_mode: None,
        connect_by: None,
    };
    let wrapper = SqlQuery {
        with: None,
        body: Box::new(SqlSetExpr::Select(Box::new(select))),
        order_by: Vec::new(),
        limit: Some(SqlExpr::Value(SqlValue::Number(
            max_rows.to_string(),
            false,
        ))),
        limit_by: Vec::new(),
        offset: None,
        fetch: None,
        locks: Vec::new(),
        for_clause: None,
    };
    SqlStatement::Query(Box::new(wrapper))
}

#[allow(dead_code)]
fn resolve_pg_params(specs: Vec<DbParamSpec>) -> Result<(Vec<PgParam>, Vec<Type>), ExecError> {
    let mut params = Vec::with_capacity(specs.len());
    let mut types = Vec::with_capacity(specs.len());
    for spec in specs {
        let (param, ty) = resolve_pg_param(spec)?;
        params.push(param);
        types.push(ty);
    }
    Ok((params, types))
}

#[allow(dead_code)]
fn resolve_pg_param(spec: DbParamSpec) -> Result<(PgParam, Type), ExecError> {
    let type_hint = spec.type_hint.as_ref().map(|t| t.trim().to_lowercase());
    let hint = type_hint.as_deref();
    match spec.value {
        DbParamValue::Null => {
            let pg_type = hint.and_then(parse_pg_type).ok_or_else(|| ExecError {
                status: "InvalidInput",
                message: "Null params must include an explicit type".to_string(),
            })?;
            let param = match pg_type {
                Type::BOOL => PgParam::NullBool(None),
                Type::INT2 => PgParam::NullInt2(None),
                Type::INT4 => PgParam::NullInt4(None),
                Type::INT8 => PgParam::NullInt8(None),
                Type::FLOAT4 => PgParam::NullFloat4(None),
                Type::FLOAT8 => PgParam::NullFloat8(None),
                Type::TEXT | Type::VARCHAR | Type::BPCHAR => PgParam::NullString(None),
                Type::BYTEA => PgParam::NullBytes(None),
                _ => {
                    return Err(ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported null param type {pg_type}"),
                    })
                }
            };
            Ok((param, pg_type))
        }
        DbParamValue::Bool(value) => {
            let pg_type = hint.and_then(parse_pg_type).unwrap_or(Type::BOOL);
            if pg_type != Type::BOOL {
                return Err(ExecError {
                    status: "InvalidInput",
                    message: format!("Expected bool for type {pg_type}"),
                });
            }
            Ok((PgParam::Bool(value), pg_type))
        }
        DbParamValue::Int(value) => resolve_int_param(value, hint),
        DbParamValue::Float(value) => resolve_float_param(value, hint),
        DbParamValue::String(value) => resolve_string_param(value, hint),
        DbParamValue::Bytes(value) => {
            let pg_type = hint.and_then(parse_pg_type).unwrap_or(Type::BYTEA);
            if pg_type != Type::BYTEA {
                return Err(ExecError {
                    status: "InvalidInput",
                    message: format!("Expected bytes for type {pg_type}"),
                });
            }
            Ok((PgParam::Bytes(value), pg_type))
        }
    }
}

#[allow(dead_code)]
fn resolve_int_param(value: i64, hint: Option<&str>) -> Result<(PgParam, Type), ExecError> {
    let pg_type = hint.and_then(parse_pg_type).unwrap_or(Type::INT8);
    match pg_type {
        Type::INT2 => {
            let cast = i16::try_from(value).map_err(|_| ExecError {
                status: "InvalidInput",
                message: "Value out of range for int2".to_string(),
            })?;
            Ok((PgParam::Int2(cast), pg_type))
        }
        Type::INT4 => {
            let cast = i32::try_from(value).map_err(|_| ExecError {
                status: "InvalidInput",
                message: "Value out of range for int4".to_string(),
            })?;
            Ok((PgParam::Int4(cast), pg_type))
        }
        Type::INT8 => Ok((PgParam::Int8(value), pg_type)),
        Type::FLOAT4 => Ok((PgParam::Float4(value as f32), pg_type)),
        Type::FLOAT8 => Ok((PgParam::Float8(value as f64), pg_type)),
        _ => Err(ExecError {
            status: "InvalidInput",
            message: format!("Expected integer for type {pg_type}"),
        }),
    }
}

#[allow(dead_code)]
fn resolve_float_param(value: f64, hint: Option<&str>) -> Result<(PgParam, Type), ExecError> {
    let pg_type = hint.and_then(parse_pg_type).unwrap_or(Type::FLOAT8);
    match pg_type {
        Type::FLOAT4 => Ok((PgParam::Float4(value as f32), pg_type)),
        Type::FLOAT8 => Ok((PgParam::Float8(value), pg_type)),
        _ => Err(ExecError {
            status: "InvalidInput",
            message: format!("Expected float for type {pg_type}"),
        }),
    }
}

#[allow(dead_code)]
fn resolve_string_param(value: String, hint: Option<&str>) -> Result<(PgParam, Type), ExecError> {
    let pg_type = hint.and_then(parse_pg_type).unwrap_or(Type::TEXT);
    match pg_type {
        Type::TEXT | Type::VARCHAR | Type::BPCHAR => Ok((PgParam::String(value), pg_type)),
        _ => Err(ExecError {
            status: "InvalidInput",
            message: format!("Expected string for type {pg_type}"),
        }),
    }
}

#[allow(dead_code)]
fn parse_pg_type(name: &str) -> Option<Type> {
    match name {
        "bool" | "boolean" => Some(Type::BOOL),
        "int2" | "smallint" => Some(Type::INT2),
        "int4" | "int" | "integer" => Some(Type::INT4),
        "int8" | "bigint" => Some(Type::INT8),
        "float4" | "real" => Some(Type::FLOAT4),
        "float8" | "double" | "double precision" => Some(Type::FLOAT8),
        "text" => Some(Type::TEXT),
        "varchar" | "character varying" => Some(Type::VARCHAR),
        "bpchar" | "char" | "character" => Some(Type::BPCHAR),
        "bytea" => Some(Type::BYTEA),
        _ => None,
    }
}

fn resolve_sqlite_params(specs: Vec<DbParamSpec>) -> Result<Vec<Value>, ExecError> {
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        let value = match spec.value {
            DbParamValue::Null => {
                if spec.type_hint.is_none() {
                    return Err(ExecError {
                        status: "InvalidInput",
                        message: "Null params must include an explicit type".to_string(),
                    });
                }
                Value::Null
            }
            DbParamValue::Bool(value) => Value::Integer(if value { 1 } else { 0 }),
            DbParamValue::Int(value) => Value::Integer(value),
            DbParamValue::Float(value) => Value::Real(value),
            DbParamValue::String(value) => Value::Text(value),
            DbParamValue::Bytes(value) => Value::Blob(value),
        };
        params.push(value);
    }
    Ok(params)
}

fn sqlite_value_to_row(value: ValueRef<'_>) -> Result<DbRowValue, ExecError> {
    match value {
        ValueRef::Null => Ok(DbRowValue::Null),
        ValueRef::Integer(value) => Ok(DbRowValue::Int(value)),
        ValueRef::Real(value) => Ok(DbRowValue::Float(value)),
        ValueRef::Text(value) => {
            let text = std::str::from_utf8(value).map_err(|err| ExecError {
                status: "InternalError",
                message: format!("SQLite text decode failed: {err}"),
            })?;
            Ok(DbRowValue::String(text.to_string()))
        }
        ValueRef::Blob(value) => Ok(DbRowValue::Bytes(ByteBuf::from(value.to_vec()))),
    }
}

fn pg_row_values(
    row: &PgRow,
    columns: &[tokio_postgres::Column],
) -> Result<Vec<DbRowValue>, ExecError> {
    let mut values = Vec::with_capacity(columns.len());
    for (idx, col) in columns.iter().enumerate() {
        let ty = col.type_();
        let value = match *ty {
            Type::BOOL => row
                .try_get::<_, Option<bool>>(idx)
                .map(|val| val.map(DbRowValue::Bool).unwrap_or(DbRowValue::Null)),
            Type::INT2 => row
                .try_get::<_, Option<i16>>(idx)
                .map(|val| val.map(|v| DbRowValue::Int(v as i64)).unwrap_or(DbRowValue::Null)),
            Type::INT4 => row
                .try_get::<_, Option<i32>>(idx)
                .map(|val| val.map(|v| DbRowValue::Int(v as i64)).unwrap_or(DbRowValue::Null)),
            Type::INT8 => row
                .try_get::<_, Option<i64>>(idx)
                .map(|val| val.map(DbRowValue::Int).unwrap_or(DbRowValue::Null)),
            Type::FLOAT4 => row
                .try_get::<_, Option<f32>>(idx)
                .map(|val| val.map(|v| DbRowValue::Float(v as f64)).unwrap_or(DbRowValue::Null)),
            Type::FLOAT8 => row
                .try_get::<_, Option<f64>>(idx)
                .map(|val| val.map(DbRowValue::Float).unwrap_or(DbRowValue::Null)),
            Type::TEXT | Type::VARCHAR | Type::BPCHAR => row
                .try_get::<_, Option<String>>(idx)
                .map(|val| val.map(DbRowValue::String).unwrap_or(DbRowValue::Null)),
            Type::BYTEA => row
                .try_get::<_, Option<Vec<u8>>>(idx)
                .map(|val| {
                    val.map(|v| DbRowValue::Bytes(ByteBuf::from(v)))
                        .unwrap_or(DbRowValue::Null)
                }),
            _ => row
                .try_get::<_, Option<String>>(idx)
                .map(|val| val.map(DbRowValue::String).unwrap_or(DbRowValue::Null)),
        }
        .map_err(|err| ExecError {
            status: "InvalidInput",
            message: format!("Unsupported column type {ty}: {err}"),
        })?;
        values.push(value);
    }
    Ok(values)
}

fn list_items_response(
    request: &ListItemsRequest,
    ctx: &ExecContext<'_>,
) -> Result<ListItemsResponse, ExecError> {
    let conn = acquire_connection(
        ctx.pool,
        ctx.cancelled,
        ctx.request_id,
        ctx.exec_start,
        ctx.timeout,
    )?;
    match conn.as_ref() {
        DbConn::Sqlite(sqlite) => list_items_sqlite_response(request, ctx, sqlite),
        DbConn::Fake(_) => list_items_fake_response(request, ctx),
    }
}

fn list_items_fake_response(
    request: &ListItemsRequest,
    ctx: &ExecContext<'_>,
) -> Result<ListItemsResponse, ExecError> {
    check_timeout(ctx.exec_start, ctx.timeout)?;
    if ctx.fake_delay.as_millis() > 0 {
        sleep_with_cancel(
            ctx.fake_delay,
            ctx.cancelled,
            ctx.request_id,
            ctx.exec_start,
            ctx.timeout,
        )?;
    }
    let limit = request.limit.unwrap_or(50).min(500) as usize;
    sleep_decode_cost(
        limit,
        ctx.fake_decode_us_per_row,
        ctx.cancelled,
        ctx.request_id,
        ctx.exec_start,
        ctx.timeout,
    )?;
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
        check_cancelled(ctx.cancelled, ctx.request_id)?;
        check_timeout(ctx.exec_start, ctx.timeout)?;
        if ctx.fake_cpu_iters > 0 {
            let seed = request.user_id.unsigned_abs() + idx as u64;
            burn_cpu(ctx.fake_cpu_iters, seed);
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

fn list_items_sqlite_response(
    request: &ListItemsRequest,
    ctx: &ExecContext<'_>,
    sqlite: &SqliteConn,
) -> Result<ListItemsResponse, ExecError> {
    check_timeout(ctx.exec_start, ctx.timeout)?;
    check_cancelled(ctx.cancelled, ctx.request_id)?;
    let limit = request.limit.unwrap_or(50).min(500) as i64;
    let mut sql = String::from(
        "SELECT id, created_at, status, title, score, unread FROM items WHERE user_id = ?",
    );
    let mut params: Vec<Value> = vec![Value::from(request.user_id)];
    if let Some(status) = request.status.as_ref() {
        sql.push_str(" AND status = ?");
        params.push(Value::from(status.clone()));
    }
    if let Some(query) = request.q.as_ref() {
        sql.push_str(" AND title LIKE ?");
        params.push(Value::from(format!("%{query}%")));
    }
    sql.push_str(" ORDER BY id ASC LIMIT ?");
    params.push(Value::from(limit));

    let conn = sqlite.connection();
    let mut stmt = conn.prepare(&sql).map_err(|err| ExecError {
        status: "InternalError",
        message: format!("SQLite prepare failed: {err}"),
    })?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        let id: i64 = row.get(0)?;
        let created_at: String = row.get(1)?;
        let status: String = row.get(2)?;
        let title: String = row.get(3)?;
        let score: f64 = row.get(4)?;
        let unread_raw: i64 = row.get(5)?;
        Ok(ItemRow {
            id,
            created_at,
            status,
            title,
            score,
            unread: unread_raw != 0,
        })
    });

    let mut items = Vec::with_capacity(limit as usize);
    let mut open = 0u32;
    let mut closed = 0u32;
    let rows_iter = rows.map_err(|err| ExecError {
        status: "InternalError",
        message: format!("SQLite query failed: {err}"),
    })?;
    for item in rows_iter {
        check_cancelled(ctx.cancelled, ctx.request_id)?;
        check_timeout(ctx.exec_start, ctx.timeout)?;
        let item = item.map_err(|err| ExecError {
            status: "InternalError",
            message: format!("SQLite row decode failed: {err}"),
        })?;
        if item.status == "open" {
            open += 1;
        } else if item.status == "closed" {
            closed += 1;
        }
        items.push(item);
    }

    let next_cursor = if items.len() == limit as usize {
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

fn decode_db_query_request(envelope: &RequestEnvelope) -> Result<DbQueryRequest, ExecError> {
    let payload = extract_payload(envelope).map_err(|err| ExecError {
        status: "InvalidInput",
        message: err,
    })?;
    decode_payload::<DbQueryRequest>(&payload, &envelope.codec).map_err(|err| ExecError {
        status: "InvalidInput",
        message: err,
    })
}

fn execute_db_query_sync(
    envelope: &RequestEnvelope,
    ctx: &ExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    let request = decode_db_query_request(envelope)?;
    let sql = request.sql.trim();
    if sql.is_empty() {
        return Err(ExecError {
            status: "InvalidInput",
            message: "SQL must be non-empty".to_string(),
        });
    }
    let alias = request.db_alias.as_deref().unwrap_or("default");
    if alias != "default" {
        return Err(ExecError {
            status: "InvalidInput",
            message: format!("Unknown db_alias '{alias}'"),
        });
    }
    let result_format = DbResultFormat::parse(request.result_format.as_deref())?;
    if matches!(result_format, DbResultFormat::ArrowIpc) {
        return Err(ExecError {
            status: "InvalidInput",
            message: "arrow_ipc result_format is not supported yet".to_string(),
        });
    }
    let max_rows = request.max_rows.unwrap_or(ctx.default_max_rows);
    let allow_write = request.allow_write.unwrap_or(false);
    let (normalized_sql, specs) =
        normalize_params_and_sql(sql, request.params, ParamStyle::QuestionNumbered)?;
    let validated_sql = validate_query(&normalized_sql, max_rows, allow_write)?;
    let response = db_query_sqlite_response(&validated_sql, specs, ctx)?;
    let encoded = encode_payload(&response, result_format.codec()).map_err(|err| ExecError {
        status: "InternalError",
        message: err,
    })?;
    Ok((result_format.codec().to_string(), encoded))
}

fn db_query_sqlite_response(
    sql: &str,
    params: Vec<DbParamSpec>,
    ctx: &ExecContext<'_>,
) -> Result<DbQueryResponse, ExecError> {
    let conn = acquire_connection(
        ctx.pool,
        ctx.cancelled,
        ctx.request_id,
        ctx.exec_start,
        ctx.timeout,
    )?;
    let sqlite = match conn.as_ref() {
        DbConn::Sqlite(sqlite) => sqlite,
        DbConn::Fake(_) => {
            return Err(ExecError {
                status: "InvalidInput",
                message: "db_query requires a real SQLite or Postgres connection".to_string(),
            })
        }
    };
    check_timeout(ctx.exec_start, ctx.timeout)?;
    check_cancelled(ctx.cancelled, ctx.request_id)?;
    let values = resolve_sqlite_params(params)?;
    let conn = sqlite.connection();
    let mut stmt = conn.prepare(sql).map_err(|err| ExecError {
        status: "InternalError",
        message: format!("SQLite prepare failed: {err}"),
    })?;
    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let mut rows_iter = stmt
        .query(params_from_iter(values))
        .map_err(|err| ExecError {
            status: "InternalError",
            message: format!("SQLite query failed: {err}"),
        })?;
    let mut rows = Vec::new();
    while let Some(row) = rows_iter.next().map_err(|err| ExecError {
        status: "InternalError",
        message: format!("SQLite row fetch failed: {err}"),
    })? {
        check_timeout(ctx.exec_start, ctx.timeout)?;
        check_cancelled(ctx.cancelled, ctx.request_id)?;
        let mut row_values = Vec::with_capacity(columns.len());
        for idx in 0..columns.len() {
            let value = row.get_ref(idx).map_err(|err| ExecError {
                status: "InternalError",
                message: format!("SQLite row decode failed: {err}"),
            })?;
            row_values.push(sqlite_value_to_row(value)?);
        }
        rows.push(row_values);
    }
    Ok(DbQueryResponse {
        columns,
        row_count: rows.len(),
        rows,
    })
}

async fn execute_db_query_async(
    envelope: RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    if ctx.pg_pool.is_some() {
        return execute_db_query_postgres(&envelope, ctx).await;
    }
    let cancelled = ctx.cancelled.clone();
    let pool = ctx.pool.clone();
    let exec_start = ctx.exec_start;
    let timeout = ctx.timeout;
    let request_id = ctx.request_id;
    let fake_delay = ctx.fake_delay;
    let fake_decode_us_per_row = ctx.fake_decode_us_per_row;
    let fake_cpu_iters = ctx.fake_cpu_iters;
    let default_max_rows = ctx.default_max_rows;
    spawn_blocking(move || {
        let exec_ctx = ExecContext {
            cancelled: &cancelled,
            request_id,
            pool: &pool,
            timeout,
            exec_start,
            fake_delay,
            fake_decode_us_per_row,
            fake_cpu_iters,
            default_max_rows,
        };
        execute_db_query_sync(&envelope, &exec_ctx)
    })
    .await
    .map_err(|err| ExecError {
        status: "InternalError",
        message: format!("db_query worker join failed: {err}"),
    })?
}

async fn execute_db_query_postgres(
    envelope: &RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    let request = decode_db_query_request(envelope)?;
    let sql = request.sql.trim();
    if sql.is_empty() {
        return Err(ExecError {
            status: "InvalidInput",
            message: "SQL must be non-empty".to_string(),
        });
    }
    let alias = request.db_alias.as_deref().unwrap_or("default");
    if alias != "default" {
        return Err(ExecError {
            status: "InvalidInput",
            message: format!("Unknown db_alias '{alias}'"),
        });
    }
    let result_format = DbResultFormat::parse(request.result_format.as_deref())?;
    if matches!(result_format, DbResultFormat::ArrowIpc) {
        return Err(ExecError {
            status: "InvalidInput",
            message: "arrow_ipc result_format is not supported yet".to_string(),
        });
    }
    let max_rows = request.max_rows.unwrap_or(ctx.default_max_rows);
    let allow_write = request.allow_write.unwrap_or(false);
    let (normalized_sql, specs) =
        normalize_params_and_sql(sql, request.params, ParamStyle::DollarNumbered)?;
    let validated_sql = validate_query(&normalized_sql, max_rows, allow_write)?;
    let (pg_params, pg_types) = resolve_pg_params(specs)?;
    let params_refs: Vec<&(dyn ToSql + Sync)> = pg_params.iter().map(|p| p.as_tosql()).collect();
    let pool = ctx.pg_pool.ok_or_else(|| ExecError {
        status: "InvalidInput",
        message: "Postgres pool not configured".to_string(),
    })?;
    let mut conn = acquire_pg_connection(
        pool,
        &ctx.cancel_token,
        ctx.exec_start,
        ctx.timeout,
    )
    .await?;
    let statement = conn
        .as_ref()
        .prepare_cached(&validated_sql, &pg_types)
        .await
        .map_err(|err| ExecError {
            status: "InternalError",
            message: format!("Postgres prepare failed: {err}"),
        })?;
    let columns = statement.columns().to_vec();
    let rows = execute_pg_query(
        &mut conn,
        &statement,
        &params_refs,
        &ctx.cancel_token,
        ctx.exec_start,
        ctx.timeout,
    )
    .await?;
    let mut decoded_rows = Vec::with_capacity(rows.len());
    for row in rows {
        decoded_rows.push(pg_row_values(&row, &columns)?);
    }
    let column_names = columns
        .iter()
        .map(|col| col.name().to_string())
        .collect();
    let response = DbQueryResponse {
        columns: column_names,
        row_count: decoded_rows.len(),
        rows: decoded_rows,
    };
    let encoded = encode_payload(&response, result_format.codec()).map_err(|err| ExecError {
        status: "InternalError",
        message: err,
    })?;
    Ok((result_format.codec().to_string(), encoded))
}

async fn acquire_pg_connection(
    pool: &PgPool,
    cancel: &CancelToken,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<molt_db::AsyncPooled<molt_db::PgConn>, ExecError> {
    if let Some(limit) = timeout {
        if exec_start.elapsed() >= limit {
            return Err(timeout_error());
        }
        let remaining = limit.saturating_sub(exec_start.elapsed());
        let acquire = pool.acquire(Some(cancel));
        let conn = tokio::select! {
            _ = tokio_sleep(remaining) => return Err(timeout_error()),
            result = acquire => result,
        };
        return map_pg_acquire(conn);
    }
    let conn = pool.acquire(Some(cancel)).await;
    map_pg_acquire(conn)
}

fn map_pg_acquire(
    result: Result<molt_db::AsyncPooled<molt_db::PgConn>, AsyncAcquireError>,
) -> Result<molt_db::AsyncPooled<molt_db::PgConn>, ExecError> {
    match result {
        Ok(conn) => Ok(conn),
        Err(AsyncAcquireError::Timeout) => Err(ExecError {
            status: "Busy",
            message: "Postgres pool exhausted".to_string(),
        }),
        Err(AsyncAcquireError::Cancelled) => Err(cancelled_error()),
        Err(AsyncAcquireError::Create(err)) => Err(ExecError {
            status: "InternalError",
            message: format!("Postgres connect failed: {err}"),
        }),
    }
}

async fn execute_pg_query(
    conn: &mut molt_db::AsyncPooled<molt_db::PgConn>,
    statement: &tokio_postgres::Statement,
    params: &[&(dyn ToSql + Sync)],
    cancel: &CancelToken,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<Vec<PgRow>, ExecError> {
    let query = conn.as_ref().client().query(statement, params);
    let rows = if let Some(limit) = timeout {
        if exec_start.elapsed() >= limit {
            return Err(timeout_error());
        }
        let remaining = limit.saturating_sub(exec_start.elapsed());
        tokio::select! {
            _ = tokio_sleep(remaining) => {
                let _ = conn.as_ref().cancel_query().await;
                conn.discard();
                return Err(timeout_error());
            }
            _ = cancel.cancelled() => {
                let _ = conn.as_ref().cancel_query().await;
                conn.discard();
                return Err(cancelled_error());
            }
            result = query => result,
        }
    } else {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = conn.as_ref().cancel_query().await;
                conn.discard();
                return Err(cancelled_error());
            }
            result = query => result,
        }
    };
    match rows {
        Ok(rows) => {
            conn.as_ref().touch();
            Ok(rows)
        }
        Err(err) => {
            if err.is_closed() {
                conn.discard();
            }
            let status = if err.as_db_error().is_some() {
                "InvalidInput"
            } else {
                "InternalError"
            };
            Err(ExecError {
                status,
                message: format!("Postgres query failed: {err}"),
            })
        }
    }
}

fn dispatch_compiled(
    entry: &CompiledEntry,
    payload_bytes: &[u8],
    ctx: &ExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    match entry.name.as_str() {
        "list_items" => {
            let req = decode_payload::<ListItemsRequest>(payload_bytes, &entry.codec_in).map_err(
                |err| ExecError {
                    status: "InvalidInput",
                    message: err,
                },
            )?;
            let response = list_items_response(&req, ctx)?;
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
                    check_cancelled(ctx.cancelled, ctx.request_id)?;
                    check_timeout(ctx.exec_start, ctx.timeout)?;
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
            check_cancelled(ctx.cancelled, ctx.request_id)?;
            check_timeout(ctx.exec_start, ctx.timeout)?;
            let rows = req.rows.min(50_000);
            sleep_decode_cost(
                rows,
                ctx.fake_decode_us_per_row,
                ctx.cancelled,
                ctx.request_id,
                ctx.exec_start,
                ctx.timeout,
            )?;
            if ctx.fake_cpu_iters > 0 {
                let burn_rows = rows.min(5_000);
                for idx in 0..burn_rows {
                    if idx % 1024 == 0 {
                        check_cancelled(ctx.cancelled, ctx.request_id)?;
                        check_timeout(ctx.exec_start, ctx.timeout)?;
                    }
                    burn_cpu(ctx.fake_cpu_iters, idx as u64);
                }
            }
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
            check_cancelled(ctx.cancelled, ctx.request_id)?;
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

fn execute_entry(
    envelope: &RequestEnvelope,
    ctx: &ExecContext<'_>,
    exports: &HashSet<String>,
    compiled_entries: &HashMap<String, CompiledEntry>,
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
            let response = list_items_response(&req, ctx)?;
            let codec = envelope.codec.as_str();
            let encoded = encode_payload(&response, codec).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?;
            Ok((codec.to_string(), encoded))
        }
        "db_query" => execute_db_query_sync(envelope, ctx),
        _ => {
            if let Some(entry) = compiled_entries.get(&envelope.entry) {
                return dispatch_compiled(entry, &payload_bytes, ctx);
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
    ctx: &WorkerContext,
) -> (WireCodec, ResponseEnvelope) {
    let wire = request.wire;
    let envelope = request.envelope;
    let request_id = envelope.request_id;
    let exec_start = Instant::now();
    let queue_us = exec_start
        .duration_since(request.queued_at)
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let queue_ms = queue_us / 1_000;
    let mut metrics = HashMap::new();
    metrics.insert("decode_us".to_string(), request.decode_us);
    metrics.insert("queue_ms".to_string(), queue_ms);
    metrics.insert("queue_us".to_string(), queue_us);
    metrics.insert("queue_depth".to_string(), queue_depth as u64);
    metrics.insert("pool_in_flight".to_string(), ctx.pool.in_flight() as u64);
    metrics.insert("pool_idle".to_string(), ctx.pool.idle_count() as u64);
    metrics.insert(
        "payload_bytes".to_string(),
        envelope.payload.as_ref().map(|p| p.len()).unwrap_or(0) as u64,
    );
    if is_cancelled(&ctx.cancelled, request_id) {
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
    if !envelope.entry.starts_with("__") && !ctx.exports.contains(&envelope.entry) {
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
    let compiled_flag = ctx.compiled_entries.contains_key(&entry_name) as u64;
    let exec_ctx = ExecContext {
        cancelled: &ctx.cancelled,
        request_id,
        pool: &ctx.pool,
        timeout,
        exec_start,
        fake_delay: ctx.fake_delay,
        fake_decode_us_per_row: ctx.fake_decode_us_per_row,
        fake_cpu_iters: ctx.fake_cpu_iters,
        default_max_rows: ctx.default_max_rows,
    };
    let handler_start = Instant::now();
    let result = execute_entry(&envelope, &exec_ctx, &ctx.exports, &ctx.compiled_entries);
    let handler_us = handler_start
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let exec_us = exec_start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    let exec_ms = exec_us / 1_000;
    metrics.insert("handler_us".to_string(), handler_us);
    metrics.insert("exec_us".to_string(), exec_us);
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
                    compiled: Some(compiled_flag),
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
    let mut max_queue = env::var("MOLT_WORKER_MAX_QUEUE")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|val| *val > 0)
        .unwrap_or(64);
    let mut threads = env::var("MOLT_WORKER_THREADS")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|val| *val > 0);
    let fake_delay_ms = env::var("MOLT_FAKE_DB_DELAY_MS")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or(0);
    let fake_cpu_iters = env::var("MOLT_FAKE_DB_CPU_ITERS")
        .ok()
        .and_then(|val| val.parse::<u32>().ok())
        .unwrap_or(0);
    let fake_decode_us_per_row = env::var("MOLT_FAKE_DB_DECODE_US_PER_ROW")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or(0);
    let sqlite_path = env::var("MOLT_DB_SQLITE_PATH").ok().and_then(|val| {
        if val.trim().is_empty() {
            None
        } else {
            Some(val)
        }
    });
    let sqlite_readwrite = env::var("MOLT_DB_SQLITE_READWRITE")
        .ok()
        .map(|val| matches!(val.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let sqlite_mode = if sqlite_readwrite {
        SqliteOpenMode::ReadWrite
    } else {
        SqliteOpenMode::ReadOnly
    };
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
    let pool = if let Some(path) = sqlite_path {
        let path = PathBuf::from(path);
        if let Err(err) = SqliteConn::open(&path, sqlite_mode) {
            return Err(io::Error::other(format!(
                "Failed to open SQLite DB {}: {err}",
                path.display()
            )));
        }
        Pool::new(pool_size, {
            let counter = conn_counter.clone();
            let path = path.clone();
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                DbConn::Sqlite(SqliteConn::open(&path, sqlite_mode).expect("sqlite open failed"))
            }
        })
    } else {
        Pool::new(pool_size, {
            let counter = conn_counter.clone();
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                DbConn::Fake(FakeDbConn)
            }
        })
    };
    let fake_delay = Duration::from_millis(fake_delay_ms);
    let worker_ctx = WorkerContext {
        exports: exports.clone(),
        cancelled: cancelled.clone(),
        pool: pool.clone(),
        compiled_entries: compiled_entries.clone(),
        fake_delay,
        fake_decode_us_per_row,
        fake_cpu_iters,
    };

    for _ in 0..thread_count {
        let request_rx = request_rx.clone();
        let response_tx = response_tx.clone();
        let ctx = worker_ctx.clone();
        thread::spawn(move || worker_loop(request_rx, response_tx, ctx));
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
            let response = match handle_cancel_request(&decoded.envelope, &cancelled, None) {
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
    ctx: WorkerContext,
) {
    // TODO(offload, owner:runtime, milestone:SL1): propagate cancellation into real DB tasks.
    while let Ok(request) = request_rx.recv() {
        let queue_depth = request_rx.len();
        let response = handle_request(request, queue_depth, &ctx);
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
        CompiledEntry, DbConn, DbPool, ExecContext, ListItemsRequest, ListItemsResponse, Pool,
    };
    use rusqlite::Connection;
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

    fn temp_db_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        let name = format!("molt_demo_db_{}_{}.sqlite", std::process::id(), rand_id());
        path.push(name);
        path
    }

    fn seed_sqlite_db(path: &PathBuf) {
        let conn = Connection::open(path).expect("sqlite open");
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS items;
            CREATE TABLE items (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                score REAL NOT NULL,
                unread INTEGER NOT NULL
            );
            "#,
        )
        .expect("create table");
        let mut rows = Vec::new();
        for idx in 0..3 {
            let item_id = 1000 + idx;
            let status = if idx % 2 == 0 { "open" } else { "closed" };
            let created_at = format!("2026-01-{:02}T00:00:{:02}Z", (idx % 28) + 1, idx % 60);
            rows.push((
                item_id,
                1i64,
                created_at,
                status.to_string(),
                format!("Item {item_id}"),
                (idx % 100) as f64 / 100.0,
                if idx % 3 == 0 { 1 } else { 0 },
            ));
        }
        conn.execute_batch("BEGIN").expect("begin");
        for row in rows {
            conn.execute(
                "INSERT INTO items (id, user_id, created_at, status, title, score, unread) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![row.0, row.1, row.2, row.3, row.4, row.5, row.6],
            )
            .expect("insert");
        }
        conn.execute_batch("COMMIT").expect("commit");
    }

    fn exec_ctx<'a>(
        cancelled: &'a CancelSet,
        pool: &'a DbPool,
        request_id: u64,
    ) -> ExecContext<'a> {
        ExecContext {
            cancelled,
            request_id,
            pool,
            timeout: None,
            exec_start: Instant::now(),
            fake_delay: Duration::from_millis(0),
            fake_decode_us_per_row: 0,
            fake_cpu_iters: 0,
        }
    }

    fn fake_pool() -> DbPool {
        Pool::new(1, || DbConn::Fake(super::FakeDbConn))
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
        let pool = fake_pool();
        let req = ListItemsRequest {
            user_id: 7,
            q: None,
            status: None,
            limit: Some(5),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let ctx = exec_ctx(&cancel, &pool, 7);
        let result = dispatch_compiled(&entry, &payload, &ctx).expect("compiled dispatch");
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
        let pool = fake_pool();
        let req = ListItemsRequest {
            user_id: 1,
            q: None,
            status: None,
            limit: Some(1),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let ctx = exec_ctx(&cancel, &pool, 42);
        let result = dispatch_compiled(&entry, &payload, &ctx);
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
        let pool = fake_pool();
        let req = ListItemsRequest {
            user_id: 3,
            q: Some("x".into()),
            status: Some("open".into()),
            limit: Some(2),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let ctx = exec_ctx(&cancel, &pool, 3);
        let result = dispatch_compiled(entry, &payload, &ctx).expect("dispatch");
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
        let pool = fake_pool();
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
        let ctx = exec_ctx(&cancel, &pool, 1);
        let result = dispatch_compiled(entry, &payload, &ctx);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().status, "InternalError");
    }

    #[test]
    fn sqlite_list_items_roundtrip() {
        let path = temp_db_path();
        seed_sqlite_db(&path);
        let pool_path = path.clone();
        let cancel = CancelSet::default();
        let pool: DbPool = Pool::new(1, move || {
            DbConn::Sqlite(
                super::SqliteConn::open(&pool_path, super::SqliteOpenMode::ReadOnly)
                    .expect("sqlite open"),
            )
        });
        let req = ListItemsRequest {
            user_id: 1,
            q: None,
            status: None,
            limit: Some(2),
            cursor: None,
        };
        let ctx = exec_ctx(&cancel, &pool, 1);
        let response = super::list_items_response(&req, &ctx).expect("sqlite list items");
        assert_eq!(response.items.len(), 2);
        assert_eq!(response.counts.open + response.counts.closed, 2);
        let _ = fs::remove_file(&path);
    }
}
