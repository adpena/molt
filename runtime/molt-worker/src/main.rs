use arrow::array::{
    make_builder, ArrayBuilder, ArrayRef, BinaryBuilder, BooleanBuilder, Float64Builder,
    Int32Builder, Int64Builder, ListBuilder, NullArray, NullBuilder, StringBuilder, StructBuilder,
};
use arrow::datatypes::{DataType, Field, Fields, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use fallible_iterator::FallibleIterator;
mod diagnostics;

use diagnostics::{
    ComputeRequest, ComputeResponse, HealthResponse, OffloadTableRequest, OffloadTableResponse,
};
use postgres_protocol::types::{array_from_sql, range_from_sql, ArrayDimension, Range, RangeBound};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

use molt_db::{
    AcquireError, AsyncAcquireError, CancelToken, PgPool, PgPoolConfig, Pool, Pooled, SqliteConn,
    SqliteOpenMode,
};
use rusqlite::{params_from_iter, types::Value, types::ValueRef, InterruptHandle};
use sqlparser::ast::{
    Expr as SqlExpr, GroupByExpr as SqlGroupByExpr, Ident as SqlIdent, Query as SqlQuery,
    Select as SqlSelect, SelectItem as SqlSelectItem, SetExpr as SqlSetExpr,
    Statement as SqlStatement, TableAlias as SqlTableAlias, TableFactor as SqlTableFactor,
    TableWithJoins as SqlTableWithJoins, Value as SqlValue,
    WildcardAdditionalOptions as SqlWildcardAdditionalOptions,
};
use sqlparser::dialect::{PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Builder as TokioRuntimeBuilder;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tokio::time::sleep as tokio_sleep;
use tokio_postgres::types::{FromSql, Kind, ToSql, Type};
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
        let format = value.unwrap_or("json").trim().to_lowercase();
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
    metrics: Option<HashMap<String, MetricValue>>,
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
    metrics: Option<HashMap<String, MetricValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiled: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(untagged)]
enum MetricValue {
    Int(u64),
    Str(String),
}

impl From<u64> for MetricValue {
    fn from(value: u64) -> Self {
        MetricValue::Int(value)
    }
}

impl From<usize> for MetricValue {
    fn from(value: usize) -> Self {
        MetricValue::Int(value as u64)
    }
}

impl From<String> for MetricValue {
    fn from(value: String) -> Self {
        MetricValue::Str(value)
    }
}

impl From<&str> for MetricValue {
    fn from(value: &str) -> Self {
        MetricValue::Str(value.to_string())
    }
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
#[derive(Deserialize, Serialize)]
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
#[derive(Deserialize, Serialize)]
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
#[derive(Deserialize, Serialize)]
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

impl serde::Serialize for DbNamedParam {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.name)?;
        match &self.param {
            DbParam::Raw(value) => {
                map.serialize_entry("value", value)?;
            }
            DbParam::Typed { value, r#type } => {
                map.serialize_entry("value", value)?;
                if let Some(type_name) = r#type {
                    map.serialize_entry("type", type_name)?;
                }
            }
        }
        map.end()
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
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
#[derive(Deserialize, Serialize)]
struct DbQueryResponse {
    columns: Vec<String>,
    rows: Vec<Vec<DbRowValue>>,
    row_count: usize,
}

#[derive(Deserialize, Serialize)]
struct DbExecResponse {
    rows_affected: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_insert_id: Option<i64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum DbRowValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(ByteBuf),
    Array(Vec<DbRowValue>),
    ArrayWithBounds(DbArray),
    Range(Box<DbRange>),
    Interval(DbInterval),
}

#[derive(Debug, Deserialize, Serialize)]
struct DbArray {
    lower_bounds: Vec<i32>,
    values: Vec<DbRowValue>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DbInterval {
    months: i32,
    days: i32,
    micros: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct DbRangeBound {
    value: DbRowValue,
    inclusive: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct DbRange {
    empty: bool,
    lower: Option<DbRangeBound>,
    upper: Option<DbRangeBound>,
}

struct DbQueryExecResult {
    codec: String,
    payload: Vec<u8>,
    row_count: usize,
    db_alias: String,
    tag: Option<String>,
    result_format: DbResultFormat,
}

struct DbExecResult {
    codec: String,
    payload: Vec<u8>,
    rows_affected: u64,
    last_insert_id: Option<i64>,
    db_alias: String,
    tag: Option<String>,
    result_format: DbResultFormat,
}

enum ExecOutcome {
    Standard { codec: String, payload: Vec<u8> },
    DbQuery(DbQueryExecResult),
    DbExec(DbExecResult),
}

#[derive(Clone, Debug, PartialEq)]
enum ArrowColumnType {
    Null,
    Bool,
    Int64,
    Float64,
    Utf8,
    Binary,
    Array {
        element: Box<ArrowColumnType>,
        dims: usize,
    },
    Range {
        element: Box<ArrowColumnType>,
    },
    Interval,
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

struct SqliteCancelRegistry {
    pending: Mutex<HashSet<u64>>,
    handles: Mutex<HashMap<u64, InterruptHandle>>,
}

impl SqliteCancelRegistry {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashSet::new()),
            handles: Mutex::new(HashMap::new()),
        }
    }

    fn register(&self, request_id: u64, handle: InterruptHandle) -> SqliteCancelGuard<'_> {
        {
            let mut handles = self.handles.lock().unwrap();
            handles.insert(request_id, handle);
        }
        let cancelled = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(&request_id)
        };
        if cancelled {
            let handles = self.handles.lock().unwrap();
            if let Some(handle) = handles.get(&request_id) {
                handle.interrupt();
            }
        }
        SqliteCancelGuard {
            registry: self,
            request_id,
        }
    }

    fn cancel(&self, request_id: u64) {
        let handles = self.handles.lock().unwrap();
        if let Some(handle) = handles.get(&request_id) {
            handle.interrupt();
            return;
        }
        drop(handles);
        let mut pending = self.pending.lock().unwrap();
        pending.insert(request_id);
    }

    fn clear(&self, request_id: u64) {
        {
            let mut handles = self.handles.lock().unwrap();
            handles.remove(&request_id);
        }
        let mut pending = self.pending.lock().unwrap();
        pending.remove(&request_id);
    }
}

struct SqliteCancelGuard<'a> {
    registry: &'a SqliteCancelRegistry,
    request_id: u64,
}

impl Drop for SqliteCancelGuard<'_> {
    fn drop(&mut self) {
        self.registry.clear(self.request_id);
    }
}

#[derive(Clone)]
struct WorkerContext {
    exports: Arc<HashSet<String>>,
    cancelled: CancelSet,
    sqlite_cancel_registry: Arc<SqliteCancelRegistry>,
    pool: DbPool,
    compiled_entries: Arc<HashMap<String, CompiledEntry>>,
    fake_delay: Duration,
    fake_decode_us_per_row: u64,
    fake_cpu_iters: u32,
    default_max_rows: u32,
}

struct ExecContext<'a> {
    cancelled: &'a CancelSet,
    sqlite_cancel_registry: &'a SqliteCancelRegistry,
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
    sqlite_cancel_registry: Arc<SqliteCancelRegistry>,
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
    sqlite_cancel_registry: &'a SqliteCancelRegistry,
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
    Uuid(Uuid),
    Json(serde_json::Value),
    Date(NaiveDate),
    Time(NaiveTime),
    Timestamp(NaiveDateTime),
    TimestampTz(DateTime<Utc>),
    NullBool(Option<bool>),
    NullInt2(Option<i16>),
    NullInt4(Option<i32>),
    NullInt8(Option<i64>),
    NullFloat4(Option<f32>),
    NullFloat8(Option<f64>),
    NullString(Option<String>),
    NullBytes(Option<Vec<u8>>),
    NullUuid(Option<Uuid>),
    NullJson(Option<serde_json::Value>),
    NullDate(Option<NaiveDate>),
    NullTime(Option<NaiveTime>),
    NullTimestamp(Option<NaiveDateTime>),
    NullTimestampTz(Option<DateTime<Utc>>),
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
            PgParam::Uuid(value) => value,
            PgParam::Json(value) => value,
            PgParam::Date(value) => value,
            PgParam::Time(value) => value,
            PgParam::Timestamp(value) => value,
            PgParam::TimestampTz(value) => value,
            PgParam::NullBool(value) => value,
            PgParam::NullInt2(value) => value,
            PgParam::NullInt4(value) => value,
            PgParam::NullInt8(value) => value,
            PgParam::NullFloat4(value) => value,
            PgParam::NullFloat8(value) => value,
            PgParam::NullString(value) => value,
            PgParam::NullBytes(value) => value,
            PgParam::NullUuid(value) => value,
            PgParam::NullJson(value) => value,
            PgParam::NullDate(value) => value,
            PgParam::NullTime(value) => value,
            PgParam::NullTimestamp(value) => value,
            PgParam::NullTimestampTz(value) => value,
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

async fn read_frame_async<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    if let Err(err) = reader.read_exact(&mut header).await {
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
    reader.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

async fn write_frame_async<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> io::Result<()> {
    let size = payload.len() as u32;
    writer.write_all(&size.to_le_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
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
    sqlite_cancel: Option<&SqliteCancelRegistry>,
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
    if let Some(registry) = sqlite_cancel {
        registry.cancel(cancel.request_id);
    }
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
fn validate_query(
    sql: &str,
    max_rows: u32,
    allow_write: bool,
    dialect: &dyn sqlparser::dialect::Dialect,
) -> Result<String, ExecError> {
    let mut statements = Parser::parse_sql(dialect, sql).map_err(|err| ExecError {
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
            message: "Write statements require db_exec".to_string(),
        }),
        _ => Err(ExecError {
            status: "InvalidInput",
            message: "Only read-only SELECT/CTE queries are supported".to_string(),
        }),
    }
}

#[allow(dead_code)]
struct ValidatedExec {
    sql: String,
    is_insert: bool,
}

#[allow(dead_code)]
fn validate_exec(
    sql: &str,
    allow_write: bool,
    dialect: &dyn sqlparser::dialect::Dialect,
) -> Result<ValidatedExec, ExecError> {
    if !allow_write {
        return Err(ExecError {
            status: "InvalidInput",
            message: "db_exec requires allow_write=true".to_string(),
        });
    }
    let mut statements = Parser::parse_sql(dialect, sql).map_err(|err| ExecError {
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
        SqlStatement::Query(_) => Err(ExecError {
            status: "InvalidInput",
            message: "db_exec requires a write statement".to_string(),
        }),
        other => {
            let is_insert = matches!(&other, SqlStatement::Insert { .. });
            Ok(ValidatedExec {
                sql: other.to_string(),
                is_insert,
            })
        }
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
                Type::UUID => PgParam::NullUuid(None),
                Type::JSON | Type::JSONB => PgParam::NullJson(None),
                Type::DATE => PgParam::NullDate(None),
                Type::TIME => PgParam::NullTime(None),
                Type::TIMESTAMP => PgParam::NullTimestamp(None),
                Type::TIMESTAMPTZ => PgParam::NullTimestampTz(None),
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
        Type::UUID => {
            let parsed = Uuid::parse_str(&value).map_err(|_| ExecError {
                status: "InvalidInput",
                message: "Invalid uuid value".to_string(),
            })?;
            Ok((PgParam::Uuid(parsed), pg_type))
        }
        Type::JSON | Type::JSONB => {
            let parsed = serde_json::from_str(&value).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Invalid json value: {err}"),
            })?;
            Ok((PgParam::Json(parsed), pg_type))
        }
        Type::DATE => {
            let parsed = parse_pg_date(&value)?;
            Ok((PgParam::Date(parsed), pg_type))
        }
        Type::TIME => {
            let parsed = parse_pg_time(&value)?;
            Ok((PgParam::Time(parsed), pg_type))
        }
        Type::TIMESTAMP => {
            let parsed = parse_pg_timestamp(&value)?;
            Ok((PgParam::Timestamp(parsed), pg_type))
        }
        Type::TIMESTAMPTZ => {
            let parsed = parse_pg_timestamptz(&value)?;
            Ok((PgParam::TimestampTz(parsed), pg_type))
        }
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
        "uuid" => Some(Type::UUID),
        "json" => Some(Type::JSON),
        "jsonb" => Some(Type::JSONB),
        "date" => Some(Type::DATE),
        "time" | "time without time zone" => Some(Type::TIME),
        "timestamp" | "timestamp without time zone" => Some(Type::TIMESTAMP),
        "timestamptz" | "timestamp with time zone" => Some(Type::TIMESTAMPTZ),
        _ => None,
    }
}

fn parse_pg_date(value: &str) -> Result<NaiveDate, ExecError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ExecError {
        status: "InvalidInput",
        message: "Invalid date format (expected YYYY-MM-DD)".to_string(),
    })
}

fn parse_pg_time(value: &str) -> Result<NaiveTime, ExecError> {
    NaiveTime::parse_from_str(value, "%H:%M:%S%.f")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map_err(|_| ExecError {
            status: "InvalidInput",
            message: "Invalid time format (expected HH:MM:SS[.ffffff])".to_string(),
        })
}

fn parse_pg_timestamp(value: &str) -> Result<NaiveDateTime, ExecError> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f"))
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|_| ExecError {
            status: "InvalidInput",
            message: "Invalid timestamp format (expected YYYY-MM-DD[ T]HH:MM:SS[.ffffff])"
                .to_string(),
        })
}

fn parse_pg_timestamptz(value: &str) -> Result<DateTime<Utc>, ExecError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ExecError {
            status: "InvalidInput",
            message: "Invalid timestamptz format (expected RFC3339)".to_string(),
        })
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

fn db_query_arrow_ipc_bytes(response: &DbQueryResponse) -> Result<Vec<u8>, ExecError> {
    let mut fields = Vec::with_capacity(response.columns.len());
    let mut arrays = Vec::with_capacity(response.columns.len());
    for (idx, name) in response.columns.iter().enumerate() {
        let column_type = infer_arrow_column_type(&response.rows, idx, name)?;
        let data_type = arrow_data_type(&column_type);
        fields.push(Field::new(name, data_type.clone(), true));
        arrays.push(build_arrow_array(&response.rows, idx, &column_type)?);
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(|err| ExecError {
        status: "InternalError",
        message: format!("Arrow record batch failed: {err}"),
    })?;
    let mut buffer = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new(&mut buffer, schema.as_ref()).map_err(|err| ExecError {
                status: "InternalError",
                message: format!("Arrow IPC writer failed: {err}"),
            })?;
        writer.write(&batch).map_err(|err| ExecError {
            status: "InternalError",
            message: format!("Arrow IPC write failed: {err}"),
        })?;
        writer.finish().map_err(|err| ExecError {
            status: "InternalError",
            message: format!("Arrow IPC finish failed: {err}"),
        })?;
    }
    Ok(buffer)
}

fn infer_arrow_column_type(
    rows: &[Vec<DbRowValue>],
    idx: usize,
    name: &str,
) -> Result<ArrowColumnType, ExecError> {
    let mut current: Option<ArrowColumnType> = None;
    for row in rows {
        let value = row.get(idx).ok_or_else(|| ExecError {
            status: "InternalError",
            message: format!("Row length mismatch for column '{name}'"),
        })?;
        let next = infer_arrow_value_type(value, name)?;
        if let Some(next) = next {
            current = Some(merge_arrow_types(current, next, name)?);
        }
    }
    Ok(current.unwrap_or(ArrowColumnType::Null))
}

fn build_arrow_array(
    rows: &[Vec<DbRowValue>],
    idx: usize,
    column_type: &ArrowColumnType,
) -> Result<ArrayRef, ExecError> {
    if matches!(column_type, ArrowColumnType::Null) {
        return Ok(Arc::new(NullArray::new(rows.len())));
    }
    let data_type = arrow_data_type(column_type);
    let mut builder = make_builder(&data_type, rows.len());
    for row in rows {
        let value = row.get(idx).ok_or_else(|| ExecError {
            status: "InternalError",
            message: "Row length mismatch while encoding Arrow IPC".to_string(),
        })?;
        append_arrow_value(column_type, value, builder.as_mut())?;
    }
    Ok(builder.finish())
}

fn arrow_data_type(column_type: &ArrowColumnType) -> DataType {
    match column_type {
        ArrowColumnType::Null => DataType::Null,
        ArrowColumnType::Bool => DataType::Boolean,
        ArrowColumnType::Int64 => DataType::Int64,
        ArrowColumnType::Float64 => DataType::Float64,
        ArrowColumnType::Utf8 => DataType::Utf8,
        ArrowColumnType::Binary => DataType::Binary,
        ArrowColumnType::Interval => DataType::Struct(Fields::from(vec![
            Field::new("months", DataType::Int32, true),
            Field::new("days", DataType::Int32, true),
            Field::new("micros", DataType::Int64, true),
        ])),
        ArrowColumnType::Range { element } => {
            let bound = DataType::Struct(Fields::from(vec![
                Field::new("value", arrow_data_type(element), true),
                Field::new("inclusive", DataType::Boolean, true),
            ]));
            DataType::Struct(Fields::from(vec![
                Field::new("empty", DataType::Boolean, true),
                Field::new("lower", bound.clone(), true),
                Field::new("upper", bound, true),
            ]))
        }
        ArrowColumnType::Array { element, dims } => {
            let lower_bounds = DataType::List(Arc::new(Field::new("item", DataType::Int32, true)));
            let values = list_data_type(element, *dims);
            DataType::Struct(Fields::from(vec![
                Field::new("lower_bounds", lower_bounds, true),
                Field::new("values", values, true),
            ]))
        }
    }
}

fn list_data_type(element: &ArrowColumnType, depth: usize) -> DataType {
    let mut out = arrow_data_type(element);
    for _ in 0..depth {
        out = DataType::List(Arc::new(Field::new("item", out, true)));
    }
    out
}

fn arrow_type_label(column_type: &ArrowColumnType) -> &'static str {
    match column_type {
        ArrowColumnType::Null => "null",
        ArrowColumnType::Bool => "bool",
        ArrowColumnType::Int64 => "int",
        ArrowColumnType::Float64 => "float",
        ArrowColumnType::Utf8 => "string",
        ArrowColumnType::Binary => "bytes",
        ArrowColumnType::Array { .. } => "array",
        ArrowColumnType::Range { .. } => "range",
        ArrowColumnType::Interval => "interval",
    }
}

fn merge_arrow_types(
    current: Option<ArrowColumnType>,
    next: ArrowColumnType,
    name: &str,
) -> Result<ArrowColumnType, ExecError> {
    match current {
        None => Ok(next),
        Some(current) => merge_arrow_non_null(current, next, name),
    }
}

fn merge_arrow_non_null(
    current: ArrowColumnType,
    next: ArrowColumnType,
    name: &str,
) -> Result<ArrowColumnType, ExecError> {
    if matches!(current, ArrowColumnType::Null) {
        return Ok(next);
    }
    if matches!(next, ArrowColumnType::Null) {
        return Ok(current);
    }
    match (current, next) {
        (ArrowColumnType::Int64, ArrowColumnType::Float64)
        | (ArrowColumnType::Float64, ArrowColumnType::Int64) => Ok(ArrowColumnType::Float64),
        (left, right) if left == right => Ok(left),
        (
            ArrowColumnType::Array {
                element: left,
                dims: left_dims,
            },
            ArrowColumnType::Array {
                element: right,
                dims: right_dims,
            },
        ) => {
            if left_dims != right_dims {
                return Err(ExecError {
                    status: "InvalidInput",
                    message: format!(
                        "Arrow IPC requires consistent array dimensions; column '{name}' mixed {left_dims} and {right_dims}"
                    ),
                });
            }
            let merged = merge_arrow_non_null(*left, *right, name)?;
            Ok(ArrowColumnType::Array {
                element: Box::new(merged),
                dims: left_dims,
            })
        }
        (ArrowColumnType::Range { element: left }, ArrowColumnType::Range { element: right }) => {
            let merged = merge_arrow_non_null(*left, *right, name)?;
            Ok(ArrowColumnType::Range {
                element: Box::new(merged),
            })
        }
        (left, right) => Err(ExecError {
            status: "InvalidInput",
            message: format!(
                "Arrow IPC requires consistent types; column '{name}' mixed {} and {}",
                arrow_type_label(&left),
                arrow_type_label(&right)
            ),
        }),
    }
}

fn infer_arrow_value_type(
    value: &DbRowValue,
    name: &str,
) -> Result<Option<ArrowColumnType>, ExecError> {
    match value {
        DbRowValue::Null => Ok(None),
        DbRowValue::Bool(_) => Ok(Some(ArrowColumnType::Bool)),
        DbRowValue::Int(_) => Ok(Some(ArrowColumnType::Int64)),
        DbRowValue::Float(_) => Ok(Some(ArrowColumnType::Float64)),
        DbRowValue::String(_) => Ok(Some(ArrowColumnType::Utf8)),
        DbRowValue::Bytes(_) => Ok(Some(ArrowColumnType::Binary)),
        DbRowValue::Array(values) => {
            let (element, dims) = infer_array_shape(values, name)?;
            Ok(Some(ArrowColumnType::Array {
                element: Box::new(element),
                dims,
            }))
        }
        DbRowValue::ArrayWithBounds(array) => {
            let (element, dims) = infer_array_shape(&array.values, name)?;
            if array.lower_bounds.len() != dims {
                return Err(ExecError {
                    status: "InvalidInput",
                    message: format!(
                        "Arrow IPC requires consistent array dimensions; column '{name}' has {} bounds for {dims} dims",
                        array.lower_bounds.len()
                    ),
                });
            }
            Ok(Some(ArrowColumnType::Array {
                element: Box::new(element),
                dims,
            }))
        }
        DbRowValue::Range(range) => Ok(Some(ArrowColumnType::Range {
            element: Box::new(infer_range_element_type(range, name)?),
        })),
        DbRowValue::Interval(_) => Ok(Some(ArrowColumnType::Interval)),
    }
}

fn infer_array_shape(
    values: &[DbRowValue],
    name: &str,
) -> Result<(ArrowColumnType, usize), ExecError> {
    let mut element_type: Option<ArrowColumnType> = None;
    let mut nested_dims: Option<usize> = None;
    for value in values {
        match value {
            DbRowValue::Null => continue,
            DbRowValue::Array(inner) => {
                let (inner_type, inner_dims) = infer_array_shape(inner, name)?;
                match nested_dims {
                    None => nested_dims = Some(inner_dims),
                    Some(existing) if existing == inner_dims => {}
                    Some(existing) => {
                        return Err(ExecError {
                            status: "InvalidInput",
                            message: format!(
                                "Arrow IPC requires consistent array dimensions; column '{name}' mixed nested dims {existing} and {inner_dims}"
                            ),
                        });
                    }
                }
                element_type = Some(merge_arrow_types(element_type, inner_type, name)?);
            }
            DbRowValue::ArrayWithBounds(array) => {
                let (inner_type, inner_dims) = infer_array_shape(&array.values, name)?;
                if array.lower_bounds.len() != inner_dims {
                    return Err(ExecError {
                        status: "InvalidInput",
                        message: format!(
                            "Arrow IPC requires consistent array dimensions; column '{name}' has {} bounds for {inner_dims} dims",
                            array.lower_bounds.len()
                        ),
                    });
                }
                match nested_dims {
                    None => nested_dims = Some(inner_dims),
                    Some(existing) if existing == inner_dims => {}
                    Some(existing) => {
                        return Err(ExecError {
                            status: "InvalidInput",
                            message: format!(
                                "Arrow IPC requires consistent array dimensions; column '{name}' mixed nested dims {existing} and {inner_dims}"
                            ),
                        });
                    }
                }
                element_type = Some(merge_arrow_types(element_type, inner_type, name)?);
            }
            other => {
                if nested_dims.is_some() {
                    return Err(ExecError {
                        status: "InvalidInput",
                        message: format!(
                            "Arrow IPC requires consistent array dimensions; column '{name}' mixed scalars and arrays"
                        ),
                    });
                }
                let next = infer_arrow_value_type(other, name)?.unwrap_or(ArrowColumnType::Null);
                element_type = Some(merge_arrow_types(element_type, next, name)?);
            }
        }
    }
    let element_type = element_type.unwrap_or(ArrowColumnType::Null);
    let dims = nested_dims.map(|dims| dims + 1).unwrap_or(1);
    Ok((element_type, dims))
}

fn infer_range_element_type(range: &DbRange, name: &str) -> Result<ArrowColumnType, ExecError> {
    let mut current: Option<ArrowColumnType> = None;
    if let Some(bound) = range.lower.as_ref() {
        if let Some(next) = infer_arrow_value_type(&bound.value, name)? {
            current = Some(merge_arrow_types(current, next, name)?);
        }
    }
    if let Some(bound) = range.upper.as_ref() {
        if let Some(next) = infer_arrow_value_type(&bound.value, name)? {
            current = Some(merge_arrow_types(current, next, name)?);
        }
    }
    Ok(current.unwrap_or(ArrowColumnType::Null))
}

fn append_arrow_value(
    column_type: &ArrowColumnType,
    value: &DbRowValue,
    builder: &mut dyn ArrayBuilder,
) -> Result<(), ExecError> {
    match column_type {
        ArrowColumnType::Null => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<NullBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("null"))?;
            match value {
                DbRowValue::Null => builder.append_null(),
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC type mismatch (null)".to_string(),
                    })
                }
            }
        }
        ArrowColumnType::Bool => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<BooleanBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("bool"))?;
            match value {
                DbRowValue::Null => builder.append_null(),
                DbRowValue::Bool(val) => builder.append_value(*val),
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC type mismatch (bool)".to_string(),
                    })
                }
            }
        }
        ArrowColumnType::Int64 => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<Int64Builder>()
                .ok_or_else(|| arrow_builder_mismatch("int"))?;
            match value {
                DbRowValue::Null => builder.append_null(),
                DbRowValue::Int(val) => builder.append_value(*val),
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC type mismatch (int)".to_string(),
                    })
                }
            }
        }
        ArrowColumnType::Float64 => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<Float64Builder>()
                .ok_or_else(|| arrow_builder_mismatch("float"))?;
            match value {
                DbRowValue::Null => builder.append_null(),
                DbRowValue::Float(val) => builder.append_value(*val),
                DbRowValue::Int(val) => builder.append_value(*val as f64),
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC type mismatch (float)".to_string(),
                    })
                }
            }
        }
        ArrowColumnType::Utf8 => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<StringBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("string"))?;
            match value {
                DbRowValue::Null => builder.append_null(),
                DbRowValue::String(val) => builder.append_value(val),
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC type mismatch (string)".to_string(),
                    })
                }
            }
        }
        ArrowColumnType::Binary => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<BinaryBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("bytes"))?;
            match value {
                DbRowValue::Null => builder.append_null(),
                DbRowValue::Bytes(val) => builder.append_value(val.as_ref()),
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC type mismatch (bytes)".to_string(),
                    })
                }
            }
        }
        ArrowColumnType::Array { element, dims } => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<StructBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("array"))?;
            append_array_struct(builder, element, *dims, value)?;
        }
        ArrowColumnType::Range { element } => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<StructBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("range"))?;
            append_range_struct(builder, element, value)?;
        }
        ArrowColumnType::Interval => {
            let builder = builder
                .as_any_mut()
                .downcast_mut::<StructBuilder>()
                .ok_or_else(|| arrow_builder_mismatch("interval"))?;
            append_interval_struct(builder, value)?;
        }
    }
    Ok(())
}

type DynListBuilder = ListBuilder<Box<dyn ArrayBuilder>>;

fn append_array_struct(
    builder: &mut StructBuilder,
    element: &ArrowColumnType,
    dims: usize,
    value: &DbRowValue,
) -> Result<(), ExecError> {
    match value {
        DbRowValue::Null => {
            {
                let lower_builder = builder
                    .field_builder::<DynListBuilder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("array lower_bounds"))?;
                append_list_null(lower_builder);
            }
            {
                let values_builder = builder
                    .field_builder::<DynListBuilder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("array values"))?;
                append_list_null(values_builder);
            }
            builder.append(false);
        }
        DbRowValue::Array(values) => {
            {
                let lower_builder = builder
                    .field_builder::<DynListBuilder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("array lower_bounds"))?;
                append_i32_list(lower_builder, &default_array_bounds(dims))?;
            }
            {
                let values_builder = builder
                    .field_builder::<DynListBuilder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("array values"))?;
                append_list_value(values_builder, element, dims, values)?;
            }
            builder.append(true);
        }
        DbRowValue::ArrayWithBounds(array) => {
            if array.lower_bounds.len() != dims {
                return Err(ExecError {
                    status: "InternalError",
                    message: "Arrow IPC array bounds mismatch".to_string(),
                });
            }
            {
                let lower_builder = builder
                    .field_builder::<DynListBuilder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("array lower_bounds"))?;
                append_i32_list(lower_builder, &array.lower_bounds)?;
            }
            {
                let values_builder = builder
                    .field_builder::<DynListBuilder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("array values"))?;
                append_list_value(values_builder, element, dims, &array.values)?;
            }
            builder.append(true);
        }
        _ => {
            return Err(ExecError {
                status: "InternalError",
                message: "Arrow IPC type mismatch (array)".to_string(),
            })
        }
    }
    Ok(())
}

fn append_range_struct(
    builder: &mut StructBuilder,
    element: &ArrowColumnType,
    value: &DbRowValue,
) -> Result<(), ExecError> {
    match value {
        DbRowValue::Null => {
            {
                let empty_builder = builder
                    .field_builder::<BooleanBuilder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("range empty"))?;
                empty_builder.append_null();
            }
            {
                let lower_builder = builder
                    .field_builder::<StructBuilder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("range lower"))?;
                append_range_bound(lower_builder, None, element)?;
            }
            {
                let upper_builder = builder
                    .field_builder::<StructBuilder>(2)
                    .ok_or_else(|| arrow_builder_mismatch("range upper"))?;
                append_range_bound(upper_builder, None, element)?;
            }
            builder.append(false);
        }
        DbRowValue::Range(range) => {
            {
                let empty_builder = builder
                    .field_builder::<BooleanBuilder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("range empty"))?;
                empty_builder.append_value(range.empty);
            }
            {
                let lower_builder = builder
                    .field_builder::<StructBuilder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("range lower"))?;
                append_range_bound(lower_builder, range.lower.as_ref(), element)?;
            }
            {
                let upper_builder = builder
                    .field_builder::<StructBuilder>(2)
                    .ok_or_else(|| arrow_builder_mismatch("range upper"))?;
                append_range_bound(upper_builder, range.upper.as_ref(), element)?;
            }
            builder.append(true);
        }
        _ => {
            return Err(ExecError {
                status: "InternalError",
                message: "Arrow IPC type mismatch (range)".to_string(),
            })
        }
    }
    Ok(())
}

fn append_range_bound(
    builder: &mut StructBuilder,
    bound: Option<&DbRangeBound>,
    element: &ArrowColumnType,
) -> Result<(), ExecError> {
    match bound {
        None => {
            append_struct_field_value(builder, 0, element, &DbRowValue::Null)?;
            let inclusive_builder = builder
                .field_builder::<BooleanBuilder>(1)
                .ok_or_else(|| arrow_builder_mismatch("range bound inclusive"))?;
            inclusive_builder.append_null();
            builder.append(false);
        }
        Some(bound) => {
            append_struct_field_value(builder, 0, element, &bound.value)?;
            let inclusive_builder = builder
                .field_builder::<BooleanBuilder>(1)
                .ok_or_else(|| arrow_builder_mismatch("range bound inclusive"))?;
            inclusive_builder.append_value(bound.inclusive);
            builder.append(true);
        }
    }
    Ok(())
}

fn append_interval_struct(
    builder: &mut StructBuilder,
    value: &DbRowValue,
) -> Result<(), ExecError> {
    match value {
        DbRowValue::Null => {
            {
                let months_builder = builder
                    .field_builder::<Int32Builder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("interval months"))?;
                months_builder.append_null();
            }
            {
                let days_builder = builder
                    .field_builder::<Int32Builder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("interval days"))?;
                days_builder.append_null();
            }
            {
                let micros_builder = builder
                    .field_builder::<Int64Builder>(2)
                    .ok_or_else(|| arrow_builder_mismatch("interval micros"))?;
                micros_builder.append_null();
            }
            builder.append(false);
        }
        DbRowValue::Interval(interval) => {
            {
                let months_builder = builder
                    .field_builder::<Int32Builder>(0)
                    .ok_or_else(|| arrow_builder_mismatch("interval months"))?;
                months_builder.append_value(interval.months);
            }
            {
                let days_builder = builder
                    .field_builder::<Int32Builder>(1)
                    .ok_or_else(|| arrow_builder_mismatch("interval days"))?;
                days_builder.append_value(interval.days);
            }
            {
                let micros_builder = builder
                    .field_builder::<Int64Builder>(2)
                    .ok_or_else(|| arrow_builder_mismatch("interval micros"))?;
                micros_builder.append_value(interval.micros);
            }
            builder.append(true);
        }
        _ => {
            return Err(ExecError {
                status: "InternalError",
                message: "Arrow IPC type mismatch (interval)".to_string(),
            })
        }
    }
    Ok(())
}

fn append_list_value(
    builder: &mut DynListBuilder,
    element: &ArrowColumnType,
    dims: usize,
    values: &[DbRowValue],
) -> Result<(), ExecError> {
    if dims == 0 {
        return Err(ExecError {
            status: "InternalError",
            message: "Arrow IPC array dimension mismatch".to_string(),
        });
    }
    if dims == 1 {
        for value in values {
            append_arrow_value(element, value, builder.values().as_mut())?;
        }
    } else {
        let nested_builder = builder
            .values()
            .as_any_mut()
            .downcast_mut::<DynListBuilder>()
            .ok_or_else(|| arrow_builder_mismatch("array nested list"))?;
        for value in values {
            match value {
                DbRowValue::Array(inner) => {
                    append_list_value(nested_builder, element, dims - 1, inner)?
                }
                DbRowValue::ArrayWithBounds(array) => {
                    append_list_value(nested_builder, element, dims - 1, &array.values)?
                }
                DbRowValue::Null => {
                    append_list_null(nested_builder);
                }
                _ => {
                    return Err(ExecError {
                        status: "InternalError",
                        message: "Arrow IPC array nested type mismatch".to_string(),
                    })
                }
            }
        }
    }
    builder.append(true);
    Ok(())
}

fn append_i32_list(builder: &mut DynListBuilder, values: &[i32]) -> Result<(), ExecError> {
    let value_builder = builder
        .values()
        .as_any_mut()
        .downcast_mut::<Int32Builder>()
        .ok_or_else(|| arrow_builder_mismatch("array lower_bounds values"))?;
    for value in values {
        value_builder.append_value(*value);
    }
    builder.append(true);
    Ok(())
}

fn append_struct_field_value(
    builder: &mut StructBuilder,
    idx: usize,
    field_type: &ArrowColumnType,
    value: &DbRowValue,
) -> Result<(), ExecError> {
    match field_type {
        ArrowColumnType::Null => {
            let field_builder = builder
                .field_builder::<NullBuilder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct null field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
        ArrowColumnType::Bool => {
            let field_builder = builder
                .field_builder::<BooleanBuilder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct bool field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
        ArrowColumnType::Int64 => {
            let field_builder = builder
                .field_builder::<Int64Builder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct int field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
        ArrowColumnType::Float64 => {
            let field_builder = builder
                .field_builder::<Float64Builder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct float field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
        ArrowColumnType::Utf8 => {
            let field_builder = builder
                .field_builder::<StringBuilder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct string field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
        ArrowColumnType::Binary => {
            let field_builder = builder
                .field_builder::<BinaryBuilder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct binary field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
        ArrowColumnType::Array { .. }
        | ArrowColumnType::Range { .. }
        | ArrowColumnType::Interval => {
            let field_builder = builder
                .field_builder::<StructBuilder>(idx)
                .ok_or_else(|| arrow_builder_mismatch("struct nested field"))?;
            append_arrow_value(field_type, value, field_builder)
        }
    }
}

fn append_list_null(builder: &mut DynListBuilder) {
    builder.append(false);
}

fn default_array_bounds(dims: usize) -> Vec<i32> {
    vec![1; dims.max(1)]
}

fn arrow_builder_mismatch(context: &str) -> ExecError {
    ExecError {
        status: "InternalError",
        message: format!("Arrow IPC builder mismatch ({context})"),
    }
}

struct PgRawValue(Vec<u8>);

impl PgRawValue {
    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> FromSql<'a> for PgRawValue {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Self(raw.to_vec()))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

fn decode_pg_raw_value(ty: &Type, raw: Option<&[u8]>) -> Result<DbRowValue, ExecError> {
    let Some(raw) = raw else {
        return Ok(DbRowValue::Null);
    };
    let mut base = ty;
    while let Kind::Domain(inner) = base.kind() {
        base = inner;
    }
    match base.kind() {
        Kind::Array(_) => return decode_pg_array_value(base, raw),
        Kind::Range(_) => return decode_pg_range_value(base, raw),
        Kind::Multirange(_) => return decode_pg_multirange_value(base, raw),
        _ => {}
    }
    let value = match *base {
        Type::BOOL => DbRowValue::Bool(bool::from_sql(base, raw).map_err(|err| ExecError {
            status: "InvalidInput",
            message: format!("Postgres decode failed for {}: {err}", base.name()),
        })?),
        Type::INT2 => {
            let val = i16::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::Int(val as i64)
        }
        Type::INT4 => {
            let val = i32::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::Int(val as i64)
        }
        Type::INT8 => {
            let val = i64::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::Int(val)
        }
        Type::FLOAT4 => {
            let val = f32::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::Float(val as f64)
        }
        Type::FLOAT8 => {
            let val = f64::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::Float(val)
        }
        Type::TEXT | Type::VARCHAR | Type::BPCHAR => {
            let val = String::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val)
        }
        Type::BYTEA => {
            let val = Vec::<u8>::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::Bytes(ByteBuf::from(val))
        }
        Type::UUID => {
            let val = Uuid::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val.to_string())
        }
        Type::JSON | Type::JSONB => {
            let val = serde_json::Value::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val.to_string())
        }
        Type::DATE => {
            let val = NaiveDate::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val.to_string())
        }
        Type::TIME => {
            let val = NaiveTime::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val.format("%H:%M:%S%.f").to_string())
        }
        Type::TIMESTAMP => {
            let val = NaiveDateTime::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
        }
        Type::TIMESTAMPTZ => {
            let val = DateTime::<Utc>::from_sql(base, raw).map_err(|err| ExecError {
                status: "InvalidInput",
                message: format!("Postgres decode failed for {}: {err}", base.name()),
            })?;
            DbRowValue::String(val.to_rfc3339())
        }
        Type::INTERVAL => DbRowValue::Interval(decode_pg_interval(raw)?),
        _ => {
            if <String as FromSql>::accepts(base) {
                let val = String::from_sql(base, raw).map_err(|err| ExecError {
                    status: "InvalidInput",
                    message: format!("Postgres decode failed for {}: {err}", base.name()),
                })?;
                DbRowValue::String(val)
            } else {
                return Err(ExecError {
                    status: "InvalidInput",
                    message: format!("Postgres type '{}' is not supported yet", base.name()),
                });
            }
        }
    };
    Ok(value)
}

fn decode_pg_array_value(ty: &Type, raw: &[u8]) -> Result<DbRowValue, ExecError> {
    let element_type = match ty.kind() {
        Kind::Array(element_type) => element_type,
        _ => {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Postgres type '{}' is not an array", ty.name()),
            })
        }
    };
    let array = array_from_sql(raw).map_err(|err| ExecError {
        status: "InvalidInput",
        message: format!("Postgres array decode failed: {err}"),
    })?;
    let mut dimensions = Vec::new();
    let mut lower_bounds = Vec::new();
    let mut dims_iter = array.dimensions();
    while let Some(dim) = dims_iter.next().map_err(|err| ExecError {
        status: "InvalidInput",
        message: format!("Postgres array dimension decode failed: {err}"),
    })? {
        lower_bounds.push(dim.lower_bound);
        dimensions.push(dim);
    }
    let mut values = Vec::new();
    let mut values_iter = array.values();
    while let Some(raw_val) = values_iter.next().map_err(|err| ExecError {
        status: "InvalidInput",
        message: format!("Postgres array value decode failed: {err}"),
    })? {
        values.push(decode_pg_raw_value(element_type, raw_val)?);
    }
    let mut queue = VecDeque::from(values);
    let nested = build_pg_array(&dimensions, &mut queue)?;
    if !queue.is_empty() {
        return Err(ExecError {
            status: "InternalError",
            message: "Postgres array decode mismatch (extra values)".to_string(),
        });
    }
    if lower_bounds.iter().all(|bound| *bound == 1) {
        Ok(DbRowValue::Array(nested))
    } else {
        Ok(DbRowValue::ArrayWithBounds(DbArray {
            lower_bounds,
            values: nested,
        }))
    }
}

fn build_pg_array(
    dimensions: &[ArrayDimension],
    values: &mut VecDeque<DbRowValue>,
) -> Result<Vec<DbRowValue>, ExecError> {
    if dimensions.is_empty() {
        return Ok(Vec::new());
    }
    let len = usize::try_from(dimensions[0].len).map_err(|_| ExecError {
        status: "InvalidInput",
        message: format!(
            "Postgres array dimension length {} is invalid",
            dimensions[0].len
        ),
    })?;
    let mut out = Vec::with_capacity(len);
    if dimensions.len() == 1 {
        for _ in 0..len {
            let value = values.pop_front().ok_or_else(|| ExecError {
                status: "InternalError",
                message: "Postgres array decode mismatch (missing values)".to_string(),
            })?;
            out.push(value);
        }
    } else {
        for _ in 0..len {
            let nested = build_pg_array(&dimensions[1..], values)?;
            out.push(DbRowValue::Array(nested));
        }
    }
    Ok(out)
}

fn read_be_i32(buf: &mut &[u8], context: &str) -> Result<i32, ExecError> {
    if buf.len() < 4 {
        return Err(ExecError {
            status: "InvalidInput",
            message: format!("Postgres {context} decode failed: unexpected end of buffer"),
        });
    }
    let (head, tail) = buf.split_at(4);
    *buf = tail;
    let raw: [u8; 4] = head.try_into().map_err(|_| ExecError {
        status: "InvalidInput",
        message: format!("Postgres {context} decode failed: malformed int32"),
    })?;
    Ok(i32::from_be_bytes(raw))
}

fn decode_pg_range_value(ty: &Type, raw: &[u8]) -> Result<DbRowValue, ExecError> {
    let element_type = match ty.kind() {
        Kind::Range(element_type) => element_type,
        _ => {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Postgres type '{}' is not a range", ty.name()),
            })
        }
    };
    let decoded = decode_pg_range_raw(element_type, raw)?;
    Ok(DbRowValue::Range(Box::new(decoded)))
}

fn decode_pg_range_raw(element_type: &Type, raw: &[u8]) -> Result<DbRange, ExecError> {
    let range = range_from_sql(raw).map_err(|err| ExecError {
        status: "InvalidInput",
        message: format!("Postgres range decode failed: {err}"),
    })?;
    let decoded = match range {
        Range::Empty => DbRange {
            empty: true,
            lower: None,
            upper: None,
        },
        Range::Nonempty(lower, upper) => DbRange {
            empty: false,
            lower: decode_pg_range_bound(lower, element_type)?,
            upper: decode_pg_range_bound(upper, element_type)?,
        },
    };
    Ok(decoded)
}

fn decode_pg_multirange_value(ty: &Type, raw: &[u8]) -> Result<DbRowValue, ExecError> {
    let element_type = match ty.kind() {
        Kind::Multirange(element_type) => element_type,
        _ => {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Postgres type '{}' is not a multirange", ty.name()),
            })
        }
    };
    let mut buf = raw;
    let count = read_be_i32(&mut buf, "multirange count")?;
    if count < 0 {
        return Err(ExecError {
            status: "InvalidInput",
            message: "Postgres multirange count must be non-negative".to_string(),
        });
    }
    let mut ranges = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = read_be_i32(&mut buf, "multirange entry length")?;
        if len < 0 {
            return Err(ExecError {
                status: "InvalidInput",
                message: "Postgres multirange entry length must be non-negative".to_string(),
            });
        }
        let len = len as usize;
        if buf.len() < len {
            return Err(ExecError {
                status: "InvalidInput",
                message: "Postgres multirange entry length exceeds buffer".to_string(),
            });
        }
        let (range_bytes, rest) = buf.split_at(len);
        buf = rest;
        let range = decode_pg_range_raw(element_type, range_bytes)?;
        ranges.push(DbRowValue::Range(Box::new(range)));
    }
    if !buf.is_empty() {
        return Err(ExecError {
            status: "InvalidInput",
            message: "Postgres multirange decode left trailing bytes".to_string(),
        });
    }
    Ok(DbRowValue::Array(ranges))
}

fn decode_pg_range_bound(
    bound: RangeBound<Option<&[u8]>>,
    element_type: &Type,
) -> Result<Option<DbRangeBound>, ExecError> {
    match bound {
        RangeBound::Unbounded => Ok(None),
        RangeBound::Inclusive(raw) => Ok(Some(DbRangeBound {
            value: decode_pg_raw_value(element_type, raw)?,
            inclusive: true,
        })),
        RangeBound::Exclusive(raw) => Ok(Some(DbRangeBound {
            value: decode_pg_raw_value(element_type, raw)?,
            inclusive: false,
        })),
    }
}

fn decode_pg_interval(raw: &[u8]) -> Result<DbInterval, ExecError> {
    if raw.len() != 16 {
        return Err(ExecError {
            status: "InvalidInput",
            message: format!(
                "Postgres interval decode expected 16 bytes, got {}",
                raw.len()
            ),
        });
    }
    let micros = i64::from_be_bytes(raw[0..8].try_into().map_err(|_| ExecError {
        status: "InvalidInput",
        message: "Postgres interval microseconds were malformed".to_string(),
    })?);
    let days = i32::from_be_bytes(raw[8..12].try_into().map_err(|_| ExecError {
        status: "InvalidInput",
        message: "Postgres interval days were malformed".to_string(),
    })?);
    let months = i32::from_be_bytes(raw[12..16].try_into().map_err(|_| ExecError {
        status: "InvalidInput",
        message: "Postgres interval months were malformed".to_string(),
    })?);
    Ok(DbInterval {
        months,
        days,
        micros,
    })
}

fn pg_row_values(
    row: &PgRow,
    columns: &[tokio_postgres::Column],
) -> Result<Vec<DbRowValue>, ExecError> {
    let mut values = Vec::with_capacity(columns.len());
    for (idx, col) in columns.iter().enumerate() {
        let ty = col.type_();
        let value = match ty.kind() {
            Kind::Array(_) | Kind::Range(_) | Kind::Multirange(_) | Kind::Domain(_) => {
                let raw = row
                    .try_get::<_, Option<PgRawValue>>(idx)
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    })?;
                let raw = raw.as_ref().map(PgRawValue::as_slice);
                decode_pg_raw_value(ty, raw)
            }
            _ => match *ty {
                Type::INTERVAL => {
                    let raw =
                        row.try_get::<_, Option<PgRawValue>>(idx)
                            .map_err(|err| ExecError {
                                status: "InvalidInput",
                                message: format!("Unsupported column type {ty}: {err}"),
                            })?;
                    let raw = raw.as_ref().map(PgRawValue::as_slice);
                    decode_pg_raw_value(ty, raw)
                }
                Type::BOOL => row
                    .try_get::<_, Option<bool>>(idx)
                    .map(|val| val.map(DbRowValue::Bool).unwrap_or(DbRowValue::Null))
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::INT2 => row
                    .try_get::<_, Option<i16>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::Int(v as i64))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::INT4 => row
                    .try_get::<_, Option<i32>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::Int(v as i64))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::INT8 => row
                    .try_get::<_, Option<i64>>(idx)
                    .map(|val| val.map(DbRowValue::Int).unwrap_or(DbRowValue::Null))
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::FLOAT4 => row
                    .try_get::<_, Option<f32>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::Float(v as f64))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::FLOAT8 => row
                    .try_get::<_, Option<f64>>(idx)
                    .map(|val| val.map(DbRowValue::Float).unwrap_or(DbRowValue::Null))
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::TEXT | Type::VARCHAR | Type::BPCHAR => row
                    .try_get::<_, Option<String>>(idx)
                    .map(|val| val.map(DbRowValue::String).unwrap_or(DbRowValue::Null))
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::BYTEA => row
                    .try_get::<_, Option<Vec<u8>>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::Bytes(ByteBuf::from(v)))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::UUID => row
                    .try_get::<_, Option<Uuid>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::String(v.to_string()))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::JSON | Type::JSONB => row
                    .try_get::<_, Option<serde_json::Value>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::String(v.to_string()))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::DATE => row
                    .try_get::<_, Option<NaiveDate>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::String(v.to_string()))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::TIME => row
                    .try_get::<_, Option<NaiveTime>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::String(v.format("%H:%M:%S%.f").to_string()))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::TIMESTAMP => row
                    .try_get::<_, Option<NaiveDateTime>>(idx)
                    .map(|val| {
                        val.map(|v| {
                            DbRowValue::String(v.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
                        })
                        .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                Type::TIMESTAMPTZ => row
                    .try_get::<_, Option<DateTime<Utc>>>(idx)
                    .map(|val| {
                        val.map(|v| DbRowValue::String(v.to_rfc3339()))
                            .unwrap_or(DbRowValue::Null)
                    })
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
                _ => row
                    .try_get::<_, Option<String>>(idx)
                    .map(|val| val.map(DbRowValue::String).unwrap_or(DbRowValue::Null))
                    .map_err(|err| ExecError {
                        status: "InvalidInput",
                        message: format!("Unsupported column type {ty}: {err}"),
                    }),
            },
        }?;
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
    let _cancel_guard = ctx
        .sqlite_cancel_registry
        .register(ctx.request_id, sqlite.interrupt_handle());
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

fn execute_db_exec_sync(
    envelope: &RequestEnvelope,
    ctx: &ExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    let result = execute_db_exec_sync_result(envelope, ctx)?;
    Ok((result.codec, result.payload))
}

fn execute_db_exec_sync_result(
    envelope: &RequestEnvelope,
    ctx: &ExecContext<'_>,
) -> Result<DbExecResult, ExecError> {
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
            message: "db_exec does not support arrow_ipc".to_string(),
        });
    }
    let allow_write = request.allow_write.unwrap_or(false);
    let (normalized_sql, specs) =
        normalize_params_and_sql(sql, request.params, ParamStyle::QuestionNumbered)?;
    let dialect = SQLiteDialect {};
    let validated = validate_exec(&normalized_sql, allow_write, &dialect)?;
    let response = db_exec_sqlite_response(&validated.sql, validated.is_insert, specs, ctx)?;
    finalize_db_exec_response(response, result_format, alias.to_string(), request.tag)
}

fn execute_db_query_sync(
    envelope: &RequestEnvelope,
    ctx: &ExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    let result = execute_db_query_sync_result(envelope, ctx)?;
    Ok((result.codec, result.payload))
}

fn execute_db_query_sync_result(
    envelope: &RequestEnvelope,
    ctx: &ExecContext<'_>,
) -> Result<DbQueryExecResult, ExecError> {
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
    let max_rows = request.max_rows.unwrap_or(ctx.default_max_rows);
    let allow_write = request.allow_write.unwrap_or(false);
    let (normalized_sql, specs) =
        normalize_params_and_sql(sql, request.params, ParamStyle::QuestionNumbered)?;
    let dialect = SQLiteDialect {};
    let validated_sql = validate_query(&normalized_sql, max_rows, allow_write, &dialect)?;
    let response = db_query_sqlite_response(&validated_sql, specs, ctx)?;
    finalize_db_query_response(response, result_format, alias.to_string(), request.tag)
}

fn db_exec_sqlite_response(
    sql: &str,
    is_insert: bool,
    params: Vec<DbParamSpec>,
    ctx: &ExecContext<'_>,
) -> Result<DbExecResponse, ExecError> {
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
                message: "db_exec requires a real SQLite or Postgres connection".to_string(),
            })
        }
    };
    let _cancel_guard = ctx
        .sqlite_cancel_registry
        .register(ctx.request_id, sqlite.interrupt_handle());
    check_timeout(ctx.exec_start, ctx.timeout)?;
    check_cancelled(ctx.cancelled, ctx.request_id)?;
    let values = resolve_sqlite_params(params)?;
    let conn = sqlite.connection();
    let affected = conn
        .execute(sql, params_from_iter(values))
        .map_err(|err| ExecError {
            status: "InternalError",
            message: format!("SQLite exec failed: {err}"),
        })?;
    let last_insert_id = if is_insert {
        let rowid = conn.last_insert_rowid();
        if rowid == 0 {
            None
        } else {
            Some(rowid)
        }
    } else {
        None
    };
    Ok(DbExecResponse {
        rows_affected: affected as u64,
        last_insert_id,
    })
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
    let _cancel_guard = ctx
        .sqlite_cancel_registry
        .register(ctx.request_id, sqlite.interrupt_handle());
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

fn finalize_db_exec_response(
    response: DbExecResponse,
    result_format: DbResultFormat,
    db_alias: String,
    tag: Option<String>,
) -> Result<DbExecResult, ExecError> {
    let payload = encode_payload(&response, result_format.codec()).map_err(|err| ExecError {
        status: "InternalError",
        message: err,
    })?;
    Ok(DbExecResult {
        codec: result_format.codec().to_string(),
        payload,
        rows_affected: response.rows_affected,
        last_insert_id: response.last_insert_id,
        db_alias,
        tag,
        result_format,
    })
}

fn finalize_db_query_response(
    response: DbQueryResponse,
    result_format: DbResultFormat,
    db_alias: String,
    tag: Option<String>,
) -> Result<DbQueryExecResult, ExecError> {
    let payload = match result_format {
        DbResultFormat::Json | DbResultFormat::Msgpack => {
            encode_payload(&response, result_format.codec()).map_err(|err| ExecError {
                status: "InternalError",
                message: err,
            })?
        }
        DbResultFormat::ArrowIpc => db_query_arrow_ipc_bytes(&response)?,
    };
    Ok(DbQueryExecResult {
        codec: result_format.codec().to_string(),
        payload,
        row_count: response.row_count,
        db_alias,
        tag,
        result_format,
    })
}

async fn execute_db_exec_async(
    envelope: RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    let result = execute_db_exec_async_result(envelope, ctx).await?;
    Ok((result.codec, result.payload))
}

async fn execute_db_exec_async_result(
    envelope: RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<DbExecResult, ExecError> {
    if ctx.pg_pool.is_some() {
        return execute_db_exec_postgres_result(&envelope, ctx).await;
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
    let sqlite_cancel_registry = ctx.sqlite_cancel_registry.clone();
    spawn_blocking(move || {
        let exec_ctx = ExecContext {
            cancelled: &cancelled,
            sqlite_cancel_registry: sqlite_cancel_registry.as_ref(),
            request_id,
            pool: &pool,
            timeout,
            exec_start,
            fake_delay,
            fake_decode_us_per_row,
            fake_cpu_iters,
            default_max_rows,
        };
        execute_db_exec_sync_result(&envelope, &exec_ctx)
    })
    .await
    .map_err(|err| ExecError {
        status: "InternalError",
        message: format!("db_exec worker join failed: {err}"),
    })?
}

async fn execute_db_query_async(
    envelope: RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<(String, Vec<u8>), ExecError> {
    let result = execute_db_query_async_result(envelope, ctx).await?;
    Ok((result.codec, result.payload))
}

async fn execute_db_query_async_result(
    envelope: RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<DbQueryExecResult, ExecError> {
    if ctx.pg_pool.is_some() {
        return execute_db_query_postgres_result(&envelope, ctx).await;
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
    let sqlite_cancel_registry = ctx.sqlite_cancel_registry.clone();
    spawn_blocking(move || {
        let exec_ctx = ExecContext {
            cancelled: &cancelled,
            sqlite_cancel_registry: sqlite_cancel_registry.as_ref(),
            request_id,
            pool: &pool,
            timeout,
            exec_start,
            fake_delay,
            fake_decode_us_per_row,
            fake_cpu_iters,
            default_max_rows,
        };
        execute_db_query_sync_result(&envelope, &exec_ctx)
    })
    .await
    .map_err(|err| ExecError {
        status: "InternalError",
        message: format!("db_query worker join failed: {err}"),
    })?
}

async fn execute_db_query_postgres_result(
    envelope: &RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<DbQueryExecResult, ExecError> {
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
    let max_rows = request.max_rows.unwrap_or(ctx.default_max_rows);
    let allow_write = request.allow_write.unwrap_or(false);
    let (normalized_sql, specs) =
        normalize_params_and_sql(sql, request.params, ParamStyle::DollarNumbered)?;
    let dialect = PostgreSqlDialect {};
    let validated_sql = validate_query(&normalized_sql, max_rows, allow_write, &dialect)?;
    let (pg_params, pg_types) = resolve_pg_params(specs)?;
    let params_refs: Vec<&(dyn ToSql + Sync)> = pg_params.iter().map(|p| p.as_tosql()).collect();
    let pool = ctx.pg_pool.ok_or_else(|| ExecError {
        status: "InvalidInput",
        message: "Postgres pool not configured".to_string(),
    })?;
    let query_timeout = pool.config().query_timeout;
    let effective_timeout = if query_timeout.is_zero() {
        ctx.timeout
    } else {
        match ctx.timeout {
            Some(limit) => Some(limit.min(query_timeout)),
            None => Some(query_timeout),
        }
    };
    let conn = acquire_pg_connection(pool, &ctx.cancel_token, ctx.exec_start, ctx.timeout).await?;
    let (conn, statement) = prepare_pg_statement(
        conn,
        &validated_sql,
        &pg_types,
        &ctx.cancel_token,
        ctx.exec_start,
        effective_timeout,
    )
    .await?;
    let columns = statement.columns();
    let rows = execute_pg_query(
        conn,
        &statement,
        &params_refs,
        &ctx.cancel_token,
        ctx.exec_start,
        effective_timeout,
    )
    .await?;
    let mut decoded_rows = Vec::with_capacity(rows.len());
    for row in rows {
        if ctx.cancel_token.is_cancelled() {
            return Err(cancelled_error());
        }
        check_timeout(ctx.exec_start, ctx.timeout)?;
        decoded_rows.push(pg_row_values(&row, columns)?);
    }
    let column_names = columns.iter().map(|col| col.name().to_string()).collect();
    let response = DbQueryResponse {
        columns: column_names,
        row_count: decoded_rows.len(),
        rows: decoded_rows,
    };
    finalize_db_query_response(response, result_format, alias.to_string(), request.tag)
}

async fn execute_db_exec_postgres_result(
    envelope: &RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
) -> Result<DbExecResult, ExecError> {
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
            message: "db_exec does not support arrow_ipc".to_string(),
        });
    }
    let allow_write = request.allow_write.unwrap_or(false);
    let (normalized_sql, specs) =
        normalize_params_and_sql(sql, request.params, ParamStyle::DollarNumbered)?;
    let dialect = PostgreSqlDialect {};
    let validated = validate_exec(&normalized_sql, allow_write, &dialect)?;
    let (pg_params, pg_types) = resolve_pg_params(specs)?;
    let params_refs: Vec<&(dyn ToSql + Sync)> = pg_params.iter().map(|p| p.as_tosql()).collect();
    let pool = ctx.pg_pool.ok_or_else(|| ExecError {
        status: "InvalidInput",
        message: "Postgres pool not configured".to_string(),
    })?;
    let query_timeout = pool.config().query_timeout;
    let effective_timeout = if query_timeout.is_zero() {
        ctx.timeout
    } else {
        match ctx.timeout {
            Some(limit) => Some(limit.min(query_timeout)),
            None => Some(query_timeout),
        }
    };
    let conn = acquire_pg_connection(pool, &ctx.cancel_token, ctx.exec_start, ctx.timeout).await?;
    let (conn, statement) = prepare_pg_statement(
        conn,
        &validated.sql,
        &pg_types,
        &ctx.cancel_token,
        ctx.exec_start,
        effective_timeout,
    )
    .await?;
    let affected = execute_pg_exec(
        conn,
        &statement,
        &params_refs,
        &ctx.cancel_token,
        ctx.exec_start,
        effective_timeout,
    )
    .await?;
    let response = DbExecResponse {
        rows_affected: affected,
        last_insert_id: None,
    };
    finalize_db_exec_response(response, result_format, alias.to_string(), request.tag)
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
    conn: molt_db::AsyncPooled<molt_db::PgConn>,
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

async fn execute_pg_exec(
    conn: molt_db::AsyncPooled<molt_db::PgConn>,
    statement: &tokio_postgres::Statement,
    params: &[&(dyn ToSql + Sync)],
    cancel: &CancelToken,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<u64, ExecError> {
    let exec = conn.as_ref().client().execute(statement, params);
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
            result = exec => result,
        }
    } else {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = conn.as_ref().cancel_query().await;
                conn.discard();
                return Err(cancelled_error());
            }
            result = exec => result,
        }
    };
    match rows {
        Ok(count) => {
            conn.as_ref().touch();
            Ok(count)
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
                message: format!("Postgres exec failed: {err}"),
            })
        }
    }
}

async fn prepare_pg_statement(
    conn: molt_db::AsyncPooled<molt_db::PgConn>,
    sql: &str,
    types: &[tokio_postgres::types::Type],
    cancel: &CancelToken,
    exec_start: Instant,
    timeout: Option<Duration>,
) -> Result<
    (
        molt_db::AsyncPooled<molt_db::PgConn>,
        tokio_postgres::Statement,
    ),
    ExecError,
> {
    let prepare = conn.as_ref().prepare_cached(sql, types);
    let statement = if let Some(limit) = timeout {
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
            result = prepare => result,
        }
    } else {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = conn.as_ref().cancel_query().await;
                conn.discard();
                return Err(cancelled_error());
            }
            result = prepare => result,
        }
    };
    match statement {
        Ok(statement) => Ok((conn, statement)),
        Err(err) => {
            if err.is_closed() {
                conn.discard();
            }
            Err(ExecError {
                status: "InternalError",
                message: format!("Postgres prepare failed: {err}"),
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
        "db_exec" => execute_db_exec_sync(envelope, ctx),
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
    metrics.insert("decode_us".to_string(), request.decode_us.into());
    metrics.insert("queue_ms".to_string(), queue_ms.into());
    metrics.insert("queue_us".to_string(), queue_us.into());
    metrics.insert("queue_depth".to_string(), (queue_depth as u64).into());
    metrics.insert(
        "pool_in_flight".to_string(),
        (ctx.pool.in_flight() as u64).into(),
    );
    metrics.insert(
        "pool_idle".to_string(),
        (ctx.pool.idle_count() as u64).into(),
    );
    let payload_bytes = envelope.payload.as_ref().map(|p| p.len()).unwrap_or(0) as u64;
    metrics.insert("payload_bytes".to_string(), payload_bytes.into());
    if is_cancelled(&ctx.cancelled, request_id) {
        ctx.sqlite_cancel_registry.clear(request_id);
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
        ctx.sqlite_cancel_registry.clear(request_id);
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
        sqlite_cancel_registry: ctx.sqlite_cancel_registry.as_ref(),
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
    let result = if envelope.entry == "db_query" {
        execute_db_query_sync_result(&envelope, &exec_ctx).map(ExecOutcome::DbQuery)
    } else if envelope.entry == "db_exec" {
        execute_db_exec_sync_result(&envelope, &exec_ctx).map(ExecOutcome::DbExec)
    } else {
        execute_entry(&envelope, &exec_ctx, &ctx.exports, &ctx.compiled_entries)
            .map(|(codec, payload)| ExecOutcome::Standard { codec, payload })
    };
    let handler_us = handler_start
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let exec_us = exec_start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    let exec_ms = exec_us / 1_000;
    metrics.insert("handler_us".to_string(), handler_us.into());
    metrics.insert("exec_us".to_string(), exec_us.into());
    metrics.insert("exec_ms".to_string(), exec_ms.into());
    if let Ok(ExecOutcome::DbQuery(db_result)) = &result {
        metrics.insert("db_row_count".to_string(), db_result.row_count.into());
        metrics.insert("db_bytes_in".to_string(), payload_bytes.into());
        metrics.insert(
            "db_bytes_out".to_string(),
            (db_result.payload.len() as u64).into(),
        );
        metrics.insert(
            "db_alias".to_string(),
            MetricValue::from(db_result.db_alias.as_str()),
        );
        metrics.insert(
            "db_result_format".to_string(),
            MetricValue::from(db_result.result_format.codec()),
        );
        if let Some(tag) = db_result.tag.as_deref() {
            metrics.insert("db_tag".to_string(), MetricValue::from(tag));
        }
    }
    if let Ok(ExecOutcome::DbExec(db_result)) = &result {
        metrics.insert(
            "db_rows_affected".to_string(),
            db_result.rows_affected.into(),
        );
        if let Some(last_id) = db_result.last_insert_id {
            if last_id >= 0 {
                metrics.insert("db_last_insert_id".to_string(), (last_id as u64).into());
            }
        }
        metrics.insert("db_bytes_in".to_string(), payload_bytes.into());
        metrics.insert(
            "db_bytes_out".to_string(),
            (db_result.payload.len() as u64).into(),
        );
        metrics.insert(
            "db_alias".to_string(),
            MetricValue::from(db_result.db_alias.as_str()),
        );
        metrics.insert(
            "db_result_format".to_string(),
            MetricValue::from(db_result.result_format.codec()),
        );
        if let Some(tag) = db_result.tag.as_deref() {
            metrics.insert("db_tag".to_string(), MetricValue::from(tag));
        }
    }
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

    let response = match result {
        Ok(ExecOutcome::Standard { codec, payload }) => (
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
        Ok(ExecOutcome::DbQuery(db_result)) => (
            wire,
            ResponseEnvelope {
                request_id,
                status: "Ok".to_string(),
                codec: db_result.codec,
                payload: Some(ByteBuf::from(db_result.payload)),
                metrics: Some(metrics),
                error: None,
                entry: Some(entry_name),
                compiled: Some(compiled_flag),
            },
        ),
        Ok(ExecOutcome::DbExec(db_result)) => (
            wire,
            ResponseEnvelope {
                request_id,
                status: "Ok".to_string(),
                codec: db_result.codec,
                payload: Some(ByteBuf::from(db_result.payload)),
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
    };
    ctx.sqlite_cancel_registry.clear(request_id);
    response
}

async fn handle_request_async(
    request: DecodedRequest,
    queue_depth: usize,
    ctx: &AsyncWorkerContext,
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
    metrics.insert("decode_us".to_string(), request.decode_us.into());
    metrics.insert("queue_ms".to_string(), queue_ms.into());
    metrics.insert("queue_us".to_string(), queue_us.into());
    metrics.insert("queue_depth".to_string(), (queue_depth as u64).into());
    let (pool_in_flight, pool_idle) = match ctx.pg_pool.as_ref() {
        Some(pool) => (pool.in_flight(), pool.idle_count()),
        None => (ctx.pool.in_flight(), ctx.pool.idle_count()),
    };
    metrics.insert("pool_in_flight".to_string(), (pool_in_flight as u64).into());
    metrics.insert("pool_idle".to_string(), (pool_idle as u64).into());
    let payload_bytes = envelope.payload.as_ref().map(|p| p.len()).unwrap_or(0) as u64;
    metrics.insert("payload_bytes".to_string(), payload_bytes.into());
    if is_cancelled(&ctx.cancelled, request_id) {
        ctx.cancel_registry.clear(request_id);
        ctx.sqlite_cancel_registry.clear(request_id);
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
        ctx.cancel_registry.clear(request_id);
        ctx.sqlite_cancel_registry.clear(request_id);
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
    let cancel_token = ctx.cancel_registry.register(request_id);
    let exec_ctx = AsyncExecContext {
        cancelled: &ctx.cancelled,
        cancel_token,
        sqlite_cancel_registry: ctx.sqlite_cancel_registry.as_ref(),
        request_id,
        pool: &ctx.pool,
        pg_pool: ctx.pg_pool.as_deref(),
        timeout,
        exec_start,
        fake_delay: ctx.fake_delay,
        fake_decode_us_per_row: ctx.fake_decode_us_per_row,
        fake_cpu_iters: ctx.fake_cpu_iters,
        default_max_rows: ctx.default_max_rows,
    };
    let handler_start = Instant::now();
    let result = if entry_name == "db_query" {
        execute_db_query_async_result(envelope, &exec_ctx)
            .await
            .map(ExecOutcome::DbQuery)
    } else if entry_name == "db_exec" {
        execute_db_exec_async_result(envelope, &exec_ctx)
            .await
            .map(ExecOutcome::DbExec)
    } else {
        execute_entry_async(
            envelope,
            &exec_ctx,
            ctx.exports.clone(),
            ctx.compiled_entries.clone(),
        )
        .await
        .map(|(codec, payload)| ExecOutcome::Standard { codec, payload })
    };
    ctx.cancel_registry.clear(request_id);
    ctx.sqlite_cancel_registry.clear(request_id);
    let handler_us = handler_start
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let exec_us = exec_start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    let exec_ms = exec_us / 1_000;
    metrics.insert("handler_us".to_string(), handler_us.into());
    metrics.insert("exec_us".to_string(), exec_us.into());
    metrics.insert("exec_ms".to_string(), exec_ms.into());
    if let Ok(ExecOutcome::DbQuery(db_result)) = &result {
        metrics.insert("db_row_count".to_string(), db_result.row_count.into());
        metrics.insert("db_bytes_in".to_string(), payload_bytes.into());
        metrics.insert(
            "db_bytes_out".to_string(),
            (db_result.payload.len() as u64).into(),
        );
        metrics.insert(
            "db_alias".to_string(),
            MetricValue::from(db_result.db_alias.as_str()),
        );
        metrics.insert(
            "db_result_format".to_string(),
            MetricValue::from(db_result.result_format.codec()),
        );
        if let Some(tag) = db_result.tag.as_deref() {
            metrics.insert("db_tag".to_string(), MetricValue::from(tag));
        }
    }
    if let Ok(ExecOutcome::DbExec(db_result)) = &result {
        metrics.insert(
            "db_rows_affected".to_string(),
            db_result.rows_affected.into(),
        );
        if let Some(last_id) = db_result.last_insert_id {
            if last_id >= 0 {
                metrics.insert("db_last_insert_id".to_string(), (last_id as u64).into());
            }
        }
        metrics.insert("db_bytes_in".to_string(), payload_bytes.into());
        metrics.insert(
            "db_bytes_out".to_string(),
            (db_result.payload.len() as u64).into(),
        );
        metrics.insert(
            "db_alias".to_string(),
            MetricValue::from(db_result.db_alias.as_str()),
        );
        metrics.insert(
            "db_result_format".to_string(),
            MetricValue::from(db_result.result_format.codec()),
        );
        if let Some(tag) = db_result.tag.as_deref() {
            metrics.insert("db_tag".to_string(), MetricValue::from(tag));
        }
    }
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
                    entry: Some(entry_name),
                    compiled: Some(compiled_flag),
                },
            );
        }
    }

    match result {
        Ok(ExecOutcome::Standard { codec, payload }) => (
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
        Ok(ExecOutcome::DbQuery(db_result)) => (
            wire,
            ResponseEnvelope {
                request_id,
                status: "Ok".to_string(),
                codec: db_result.codec,
                payload: Some(ByteBuf::from(db_result.payload)),
                metrics: Some(metrics),
                error: None,
                entry: Some(entry_name),
                compiled: Some(compiled_flag),
            },
        ),
        Ok(ExecOutcome::DbExec(db_result)) => (
            wire,
            ResponseEnvelope {
                request_id,
                status: "Ok".to_string(),
                codec: db_result.codec,
                payload: Some(ByteBuf::from(db_result.payload)),
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

async fn execute_entry_async(
    envelope: RequestEnvelope,
    ctx: &AsyncExecContext<'_>,
    exports: Arc<HashSet<String>>,
    compiled_entries: Arc<HashMap<String, CompiledEntry>>,
) -> Result<(String, Vec<u8>), ExecError> {
    if envelope.entry == "db_query" {
        return execute_db_query_async(envelope, ctx).await;
    }
    if envelope.entry == "db_exec" {
        return execute_db_exec_async(envelope, ctx).await;
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
    let sqlite_cancel_registry = ctx.sqlite_cancel_registry.clone();
    spawn_blocking(move || {
        let exec_ctx = ExecContext {
            cancelled: &cancelled,
            sqlite_cancel_registry: sqlite_cancel_registry.as_ref(),
            request_id,
            pool: &pool,
            timeout,
            exec_start,
            fake_delay,
            fake_decode_us_per_row,
            fake_cpu_iters,
            default_max_rows,
        };
        execute_entry(
            &envelope,
            &exec_ctx,
            exports.as_ref(),
            compiled_entries.as_ref(),
        )
    })
    .await
    .map_err(|err| ExecError {
        status: "InternalError",
        message: format!("worker task join failed: {err}"),
    })?
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

fn run_sync(ctx: WorkerContext, max_queue: usize, thread_count: usize) -> io::Result<()> {
    let (request_tx, request_rx) = bounded::<DecodedRequest>(max_queue);
    let (response_tx, response_rx) = bounded::<(WireCodec, ResponseEnvelope)>(max_queue);

    for _ in 0..thread_count {
        let request_rx = request_rx.clone();
        let response_tx = response_tx.clone();
        let ctx = ctx.clone();
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
            let response = match handle_cancel_request(
                &decoded.envelope,
                &ctx.cancelled,
                Some(ctx.sqlite_cancel_registry.as_ref()),
                None,
            ) {
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
    let mut runtime = env::var("MOLT_WORKER_RUNTIME").unwrap_or_else(|_| "sync".to_string());
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
    let default_max_rows = env::var("MOLT_DB_MAX_ROWS")
        .ok()
        .and_then(|val| val.parse::<u32>().ok())
        .filter(|val| *val > 0)
        .unwrap_or(1000);
    let pg_config = env::var("MOLT_DB_POSTGRES_DSN")
        .ok()
        .and_then(|val| {
            if val.trim().is_empty() {
                None
            } else {
                Some(val)
            }
        })
        .map(|dsn| {
            let mut config = PgPoolConfig::new(dsn);
            if let Some(val) = env::var("MOLT_DB_POSTGRES_MIN_CONNS")
                .ok()
                .and_then(|val| val.parse::<usize>().ok())
            {
                config.min_conns = val;
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_MAX_CONNS")
                .ok()
                .and_then(|val| val.parse::<usize>().ok())
            {
                config.max_conns = val.max(1);
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_MAX_IDLE_MS")
                .ok()
                .and_then(|val| val.parse::<u64>().ok())
            {
                config.max_idle = Some(Duration::from_millis(val));
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_CONNECT_TIMEOUT_MS")
                .ok()
                .and_then(|val| val.parse::<u64>().ok())
            {
                config.connect_timeout = Duration::from_millis(val);
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_QUERY_TIMEOUT_MS")
                .ok()
                .and_then(|val| val.parse::<u64>().ok())
            {
                config.query_timeout = Duration::from_millis(val);
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_MAX_WAIT_MS")
                .ok()
                .and_then(|val| val.parse::<u64>().ok())
            {
                config.max_wait = Duration::from_millis(val);
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_HEALTH_CHECK_MS")
                .ok()
                .and_then(|val| val.parse::<u64>().ok())
            {
                config.health_check_interval = Some(Duration::from_millis(val));
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_STATEMENT_CACHE_SIZE")
                .ok()
                .and_then(|val| val.parse::<usize>().ok())
            {
                config.statement_cache_size = val;
            }
            if let Some(val) = env::var("MOLT_DB_POSTGRES_SSL_ROOT_CERT")
                .ok()
                .filter(|val| !val.trim().is_empty())
            {
                config.ssl_root_cert = Some(PathBuf::from(val));
            }
            config
        });
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
            "--runtime" => {
                if let Some(val) = args.next() {
                    runtime = val;
                }
            }
            "--stdio" => {}
            _ => {}
        }
    }

    let runtime = match runtime.trim().to_lowercase().as_str() {
        "sync" => WorkerRuntime::Sync,
        "async" => WorkerRuntime::Async,
        other => {
            eprintln!("Unknown worker runtime '{other}', defaulting to sync");
            WorkerRuntime::Sync
        }
    };

    if matches!(runtime, WorkerRuntime::Sync) && pg_config.is_some() {
        return Err(io::Error::other(
            "Postgres requires async runtime; set MOLT_WORKER_RUNTIME=async or --runtime async",
        ));
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

    let sqlite_cancel_registry = Arc::new(SqliteCancelRegistry::new());

    match runtime {
        WorkerRuntime::Sync => {
            let worker_ctx = WorkerContext {
                exports: exports.clone(),
                cancelled: cancelled.clone(),
                sqlite_cancel_registry: sqlite_cancel_registry.clone(),
                pool: pool.clone(),
                compiled_entries: compiled_entries.clone(),
                fake_delay,
                fake_decode_us_per_row,
                fake_cpu_iters,
                default_max_rows,
            };
            run_sync(worker_ctx, max_queue, thread_count)
        }
        WorkerRuntime::Async => {
            let runtime = TokioRuntimeBuilder::new_multi_thread()
                .worker_threads(thread_count)
                .enable_all()
                .build()
                .map_err(|err| io::Error::other(format!("Failed to build tokio runtime: {err}")))?;
            runtime.block_on(async move {
                let pg_pool = if let Some(config) = pg_config {
                    Some(PgPool::new(config).await.map_err(io::Error::other)?)
                } else {
                    None
                };
                let worker_ctx = AsyncWorkerContext {
                    exports,
                    cancelled,
                    cancel_registry: Arc::new(CancelRegistry::new()),
                    sqlite_cancel_registry,
                    pool,
                    pg_pool: pg_pool.map(Arc::new),
                    compiled_entries,
                    fake_delay,
                    fake_decode_us_per_row,
                    fake_cpu_iters,
                    default_max_rows,
                };
                run_async(worker_ctx, thread_count, max_queue).await
            })
        }
    }
}

fn worker_loop(
    request_rx: Receiver<DecodedRequest>,
    response_tx: Sender<(WireCodec, ResponseEnvelope)>,
    ctx: WorkerContext,
) {
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

async fn read_loop_async(
    request_tx: mpsc::Sender<DecodedRequest>,
    response_tx: mpsc::Sender<(WireCodec, ResponseEnvelope)>,
    cancelled: CancelSet,
    cancel_registry: AsyncCancelRegistry,
    sqlite_cancel_registry: Arc<SqliteCancelRegistry>,
    queue_depth: Arc<AtomicUsize>,
) -> io::Result<()> {
    let mut reader = tokio::io::stdin();
    loop {
        let frame = match read_frame_async(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(err) => {
                let response = ResponseEnvelope {
                    request_id: 0,
                    status: "InvalidInput".to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: None,
                    error: Some(err.to_string()),
                    entry: None,
                    compiled: None,
                };
                let _ = response_tx.send((WireCodec::Json, response)).await;
                break;
            }
        };
        let decoded = match decode_request(&frame) {
            Ok(decoded) => decoded,
            Err(err) => {
                let response = ResponseEnvelope {
                    request_id: 0,
                    status: "InvalidInput".to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: None,
                    error: Some(err),
                    entry: None,
                    compiled: None,
                };
                let _ = response_tx.send((WireCodec::Json, response)).await;
                continue;
            }
        };
        if decoded.envelope.entry == "__cancel__" {
            let response = match handle_cancel_request(
                &decoded.envelope,
                &cancelled,
                Some(sqlite_cancel_registry.as_ref()),
                Some(cancel_registry.as_ref()),
            ) {
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
            let _ = response_tx.send((decoded.wire, response)).await;
            continue;
        }
        match request_tx.try_send(decoded) {
            Ok(()) => {
                queue_depth.fetch_add(1, Ordering::SeqCst);
            }
            Err(mpsc::error::TrySendError::Full(request)) => {
                let response = ResponseEnvelope {
                    request_id: request.envelope.request_id,
                    status: "Busy".to_string(),
                    codec: "raw".to_string(),
                    payload: None,
                    metrics: None,
                    error: Some("Worker queue full".to_string()),
                    entry: Some(request.envelope.entry.clone()),
                    compiled: Some(0),
                };
                let _ = response_tx.send((request.wire, response)).await;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => break,
        }
    }
    Ok(())
}

async fn write_loop_async(
    mut response_rx: mpsc::Receiver<(WireCodec, ResponseEnvelope)>,
) -> io::Result<()> {
    let mut writer = tokio::io::stdout();
    while let Some((wire, response)) = response_rx.recv().await {
        let encoded = match encode_response(&response, wire) {
            Ok(encoded) => encoded,
            Err(err) => {
                eprintln!("Failed to encode response: {err}");
                continue;
            }
        };
        if let Err(err) = write_frame_async(&mut writer, &encoded).await {
            eprintln!("Failed to write response: {err}");
            break;
        }
    }
    Ok(())
}

async fn worker_loop_async(
    request_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<DecodedRequest>>>,
    response_tx: mpsc::Sender<(WireCodec, ResponseEnvelope)>,
    ctx: AsyncWorkerContext,
    queue_depth: Arc<AtomicUsize>,
) {
    loop {
        let request = {
            let mut guard = request_rx.lock().await;
            guard.recv().await
        };
        let request = match request {
            Some(request) => request,
            None => break,
        };
        let prev = queue_depth.fetch_sub(1, Ordering::SeqCst);
        let depth = prev.saturating_sub(1);
        let response = handle_request_async(request, depth, &ctx).await;
        let _ = response_tx.send(response).await;
    }
}

async fn run_async(
    ctx: AsyncWorkerContext,
    thread_count: usize,
    max_queue: usize,
) -> io::Result<()> {
    let (request_tx, request_rx) = mpsc::channel::<DecodedRequest>(max_queue);
    let (response_tx, response_rx) = mpsc::channel::<(WireCodec, ResponseEnvelope)>(max_queue);
    let queue_depth = Arc::new(AtomicUsize::new(0));
    let request_rx = Arc::new(tokio::sync::Mutex::new(request_rx));

    let mut workers = Vec::with_capacity(thread_count);
    for _ in 0..thread_count {
        let request_rx = request_rx.clone();
        let response_tx = response_tx.clone();
        let ctx = ctx.clone();
        let queue_depth = queue_depth.clone();
        workers.push(tokio::spawn(worker_loop_async(
            request_rx,
            response_tx,
            ctx,
            queue_depth,
        )));
    }

    let writer = tokio::spawn(write_loop_async(response_rx));
    read_loop_async(
        request_tx,
        response_tx.clone(),
        ctx.cancelled.clone(),
        ctx.cancel_registry.clone(),
        ctx.sqlite_cancel_registry.clone(),
        queue_depth,
    )
    .await?;
    drop(response_tx);
    for worker in workers {
        let _ = worker.await;
    }
    let _ = writer.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        db_query_arrow_ipc_bytes, decode_pg_raw_value, dispatch_compiled, execute_db_exec_sync,
        execute_db_query_sync, load_compiled_entries, load_exports, mark_cancelled, CancelSet,
        CompiledEntry, DbArray, DbConn, DbExecResponse, DbInterval, DbNamedParam, DbParam,
        DbParamValue, DbParams, DbPool, DbQueryRequest, DbQueryResponse, DbRange, DbRangeBound,
        DbRowValue, ExecContext, ListItemsRequest, ListItemsResponse, Pool, RequestEnvelope,
        SqliteCancelRegistry, SqliteConn, SqliteOpenMode,
    };
    use arrow::array::{BooleanArray, Int32Array, Int64Array, ListArray, StructArray};
    use arrow::ipc::reader::StreamReader;
    use bytes::BytesMut;
    use postgres_protocol::types::{
        array_to_sql, int4_to_sql, range_to_sql, ArrayDimension, RangeBound,
    };
    use postgres_protocol::IsNull;
    use rusqlite::Connection;
    use serde_bytes::ByteBuf;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tokio_postgres::types::Type;

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
        sqlite_cancel_registry: &'a SqliteCancelRegistry,
        request_id: u64,
    ) -> ExecContext<'a> {
        ExecContext {
            cancelled,
            sqlite_cancel_registry,
            request_id,
            pool,
            timeout: None,
            exec_start: Instant::now(),
            fake_delay: Duration::from_millis(0),
            fake_decode_us_per_row: 0,
            fake_cpu_iters: 0,
            default_max_rows: 1000,
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
        let sqlite_registry = SqliteCancelRegistry::new();
        let req = ListItemsRequest {
            user_id: 7,
            q: None,
            status: None,
            limit: Some(5),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 7);
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
        let sqlite_registry = SqliteCancelRegistry::new();
        let req = ListItemsRequest {
            user_id: 1,
            q: None,
            status: None,
            limit: Some(1),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 42);
        let result = dispatch_compiled(&entry, &payload, &ctx);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().status, "Cancelled");
    }

    #[test]
    fn db_query_sqlite_roundtrip() {
        let path = temp_db_path();
        seed_sqlite_db(&path);
        let pool_path = path.clone();
        let pool = Pool::new(1, move || {
            DbConn::Sqlite(SqliteConn::open(&pool_path, SqliteOpenMode::ReadWrite).expect("sqlite"))
        });
        let cancel = CancelSet::default();
        let sqlite_registry = SqliteCancelRegistry::new();
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 99);
        let request = DbQueryRequest {
            db_alias: None,
            sql: "select id, status from items where status = :status order by id".to_string(),
            params: DbParams::Named {
                values: vec![DbNamedParam {
                    name: "status".to_string(),
                    param: DbParam::Typed {
                        value: DbParamValue::String("open".to_string()),
                        r#type: None,
                    },
                }],
            },
            max_rows: Some(10),
            result_format: Some("json".to_string()),
            allow_write: None,
            tag: None,
        };
        let payload = rmp_serde::to_vec_named(&request).expect("encode");
        let envelope = RequestEnvelope {
            request_id: 99,
            entry: "db_query".to_string(),
            timeout_ms: 0,
            codec: "msgpack".to_string(),
            payload: Some(ByteBuf::from(payload)),
            payload_b64: None,
        };
        let (codec, payload) = execute_db_query_sync(&envelope, &ctx).expect("db_query");
        assert_eq!(codec, "json");
        let response: DbQueryResponse = super::decode_payload(&payload, "json").expect("decode");
        assert_eq!(
            response.columns,
            vec!["id".to_string(), "status".to_string()]
        );
        assert!(response.row_count > 0);
    }

    #[test]
    fn db_exec_sqlite_roundtrip() {
        let path = temp_db_path();
        seed_sqlite_db(&path);
        let pool_path = path.clone();
        let pool = Pool::new(1, move || {
            DbConn::Sqlite(SqliteConn::open(&pool_path, SqliteOpenMode::ReadWrite).expect("sqlite"))
        });
        let cancel = CancelSet::default();
        let sqlite_registry = SqliteCancelRegistry::new();
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 77);
        let request = DbQueryRequest {
            db_alias: None,
            sql: "update items set status = :status where id = :id".to_string(),
            params: DbParams::Named {
                values: vec![
                    DbNamedParam {
                        name: "status".to_string(),
                        param: DbParam::Raw(DbParamValue::String("closed".to_string())),
                    },
                    DbNamedParam {
                        name: "id".to_string(),
                        param: DbParam::Raw(DbParamValue::Int(1000)),
                    },
                ],
            },
            max_rows: None,
            result_format: Some("json".to_string()),
            allow_write: Some(true),
            tag: None,
        };
        let payload = rmp_serde::to_vec_named(&request).expect("encode");
        let envelope = RequestEnvelope {
            request_id: 77,
            entry: "db_exec".to_string(),
            timeout_ms: 0,
            codec: "msgpack".to_string(),
            payload: Some(ByteBuf::from(payload)),
            payload_b64: None,
        };
        let (codec, payload) = execute_db_exec_sync(&envelope, &ctx).expect("db_exec");
        assert_eq!(codec, "json");
        let response: DbExecResponse = super::decode_payload(&payload, "json").expect("decode");
        assert_eq!(response.rows_affected, 1);
        assert!(response.last_insert_id.is_none());
        let conn = Connection::open(path).expect("sqlite open");
        let status: String = conn
            .query_row("select status from items where id = 1000", [], |row| {
                row.get(0)
            })
            .expect("query");
        assert_eq!(status, "closed");
    }

    #[test]
    fn db_query_sqlite_arrow_ipc_roundtrip() {
        let path = temp_db_path();
        seed_sqlite_db(&path);
        let pool_path = path.clone();
        let pool = Pool::new(1, move || {
            DbConn::Sqlite(SqliteConn::open(&pool_path, SqliteOpenMode::ReadWrite).expect("sqlite"))
        });
        let cancel = CancelSet::default();
        let sqlite_registry = SqliteCancelRegistry::new();
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 88);
        let request = DbQueryRequest {
            db_alias: None,
            sql: "select id, status from items where status = :status order by id".to_string(),
            params: DbParams::Named {
                values: vec![DbNamedParam {
                    name: "status".to_string(),
                    param: DbParam::Typed {
                        value: DbParamValue::String("open".to_string()),
                        r#type: None,
                    },
                }],
            },
            max_rows: Some(10),
            result_format: Some("arrow_ipc".to_string()),
            allow_write: None,
            tag: None,
        };
        let payload = rmp_serde::to_vec_named(&request).expect("encode");
        let envelope = RequestEnvelope {
            request_id: 88,
            entry: "db_query".to_string(),
            timeout_ms: 0,
            codec: "msgpack".to_string(),
            payload: Some(ByteBuf::from(payload)),
            payload_b64: None,
        };
        let (codec, payload) = execute_db_query_sync(&envelope, &ctx).expect("db_query");
        assert_eq!(codec, "arrow_ipc");
        let cursor = Cursor::new(payload);
        let mut reader = StreamReader::try_new(cursor, None).expect("arrow reader");
        let batch = reader.next().expect("batch").expect("read batch");
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.schema().field(0).name(), "id");
        assert_eq!(batch.schema().field(1).name(), "status");
        assert_eq!(batch.num_rows(), 2);
    }

    #[test]
    fn arrow_ipc_complex_types() {
        let response = DbQueryResponse {
            columns: vec![
                "arr".to_string(),
                "range".to_string(),
                "interval".to_string(),
            ],
            rows: vec![
                vec![
                    DbRowValue::Array(vec![DbRowValue::Int(1), DbRowValue::Int(2)]),
                    DbRowValue::Range(Box::new(DbRange {
                        empty: false,
                        lower: Some(DbRangeBound {
                            value: DbRowValue::Int(1),
                            inclusive: true,
                        }),
                        upper: Some(DbRangeBound {
                            value: DbRowValue::Int(10),
                            inclusive: false,
                        }),
                    })),
                    DbRowValue::Interval(DbInterval {
                        months: 1,
                        days: 2,
                        micros: 300,
                    }),
                ],
                vec![
                    DbRowValue::ArrayWithBounds(DbArray {
                        lower_bounds: vec![0],
                        values: vec![DbRowValue::Int(3)],
                    }),
                    DbRowValue::Range(Box::new(DbRange {
                        empty: true,
                        lower: None,
                        upper: None,
                    })),
                    DbRowValue::Interval(DbInterval {
                        months: 0,
                        days: 0,
                        micros: 0,
                    }),
                ],
            ],
            row_count: 2,
        };
        let payload = db_query_arrow_ipc_bytes(&response).expect("arrow ipc");
        let mut reader = StreamReader::try_new(Cursor::new(payload), None).expect("arrow reader");
        let batch = reader.next().expect("batch").expect("read batch");
        assert_eq!(batch.num_columns(), 3);
        assert_eq!(batch.num_rows(), 2);

        let arr_struct = batch
            .column(0)
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("array struct");
        let lower_bounds = arr_struct
            .column(0)
            .as_any()
            .downcast_ref::<ListArray>()
            .expect("lower bounds");
        let lb0 = lower_bounds.value(0);
        let lb0 = lb0
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("lower bounds 0");
        assert_eq!(lb0.value(0), 1);
        let lb1 = lower_bounds.value(1);
        let lb1 = lb1
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("lower bounds 1");
        assert_eq!(lb1.value(0), 0);
        let values = arr_struct
            .column(1)
            .as_any()
            .downcast_ref::<ListArray>()
            .expect("array values");
        let values0 = values.value(0);
        let values0 = values0
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("array values 0");
        assert_eq!(values0.value(0), 1);
        assert_eq!(values0.value(1), 2);
        let values1 = values.value(1);
        let values1 = values1
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("array values 1");
        assert_eq!(values1.value(0), 3);

        let range_struct = batch
            .column(1)
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("range struct");
        let empty = range_struct
            .column(0)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .expect("range empty");
        assert!(!empty.value(0));
        assert!(empty.value(1));
        let lower = range_struct
            .column(1)
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("range lower");
        let lower_values = lower
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("range lower values");
        let lower_inclusive = lower
            .column(1)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .expect("range lower inclusive");
        assert_eq!(lower_values.value(0), 1);
        assert!(lower_inclusive.value(0));
        let upper = range_struct
            .column(2)
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("range upper");
        let upper_values = upper
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("range upper values");
        let upper_inclusive = upper
            .column(1)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .expect("range upper inclusive");
        assert_eq!(upper_values.value(0), 10);
        assert!(!upper_inclusive.value(0));

        let interval = batch
            .column(2)
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("interval struct");
        let months = interval
            .column(0)
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("interval months");
        let days = interval
            .column(1)
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("interval days");
        let micros = interval
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("interval micros");
        assert_eq!(months.value(0), 1);
        assert_eq!(days.value(0), 2);
        assert_eq!(micros.value(0), 300);
    }

    #[test]
    fn pg_interval_decode() {
        let micros: i64 = 1_234_567;
        let days: i32 = -7;
        let months: i32 = 3;
        let mut raw = Vec::new();
        raw.extend_from_slice(&micros.to_be_bytes());
        raw.extend_from_slice(&days.to_be_bytes());
        raw.extend_from_slice(&months.to_be_bytes());
        let value = decode_pg_raw_value(&Type::INTERVAL, Some(&raw)).expect("interval decode");
        match value {
            DbRowValue::Interval(interval) => {
                assert_eq!(interval.micros, micros);
                assert_eq!(interval.days, days);
                assert_eq!(interval.months, months);
            }
            other => panic!("unexpected interval decode: {other:?}"),
        }
    }

    #[test]
    fn pg_range_decode() {
        let mut buf = BytesMut::new();
        range_to_sql(
            |buf| {
                int4_to_sql(1, buf);
                Ok(RangeBound::Inclusive(IsNull::No))
            },
            |buf| {
                int4_to_sql(10, buf);
                Ok(RangeBound::Exclusive(IsNull::No))
            },
            &mut buf,
        )
        .expect("range encode");
        let value = decode_pg_raw_value(&Type::INT4_RANGE, Some(&buf)).expect("range decode");
        match value {
            DbRowValue::Range(range) => {
                let range = *range;
                assert!(!range.empty);
                let lower = range.lower.expect("lower");
                let upper = range.upper.expect("upper");
                assert!(lower.inclusive);
                assert!(!upper.inclusive);
                match lower.value {
                    DbRowValue::Int(val) => assert_eq!(val, 1),
                    other => panic!("unexpected lower bound: {other:?}"),
                }
                match upper.value {
                    DbRowValue::Int(val) => assert_eq!(val, 10),
                    other => panic!("unexpected upper bound: {other:?}"),
                }
            }
            other => panic!("unexpected range decode: {other:?}"),
        }
    }

    #[test]
    fn pg_multirange_decode() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2i32.to_be_bytes());
        for (start, end) in [(1, 5), (10, 12)] {
            let mut range_buf = BytesMut::new();
            range_to_sql(
                |buf| {
                    int4_to_sql(start, buf);
                    Ok(RangeBound::Inclusive(IsNull::No))
                },
                |buf| {
                    int4_to_sql(end, buf);
                    Ok(RangeBound::Exclusive(IsNull::No))
                },
                &mut range_buf,
            )
            .expect("range encode");
            let len = i32::try_from(range_buf.len()).expect("range len");
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(&range_buf);
        }
        let value =
            decode_pg_raw_value(&Type::INT4MULTI_RANGE, Some(&buf)).expect("multirange decode");
        match value {
            DbRowValue::Array(values) => {
                assert_eq!(values.len(), 2);
                for (idx, value) in values.into_iter().enumerate() {
                    match value {
                        DbRowValue::Range(range) => {
                            let range = *range;
                            let lower = range.lower.expect("lower");
                            let upper = range.upper.expect("upper");
                            match lower.value {
                                DbRowValue::Int(val) => {
                                    let expected = if idx == 0 { 1 } else { 10 };
                                    assert_eq!(val, expected);
                                }
                                other => panic!("unexpected multirange lower: {other:?}"),
                            }
                            match upper.value {
                                DbRowValue::Int(val) => {
                                    let expected = if idx == 0 { 5 } else { 12 };
                                    assert_eq!(val, expected);
                                }
                                other => panic!("unexpected multirange upper: {other:?}"),
                            }
                        }
                        other => panic!("unexpected multirange entry: {other:?}"),
                    }
                }
            }
            other => panic!("unexpected multirange decode: {other:?}"),
        }
    }

    #[test]
    fn pg_array_decode() {
        let dims = vec![ArrayDimension {
            len: 3,
            lower_bound: 1,
        }];
        let elements = vec![Some(1), None, Some(3)];
        let mut buf = BytesMut::new();
        array_to_sql(
            dims,
            Type::INT4.oid(),
            elements,
            |val, buf| match val {
                Some(val) => {
                    int4_to_sql(val, buf);
                    Ok(IsNull::No)
                }
                None => Ok(IsNull::Yes),
            },
            &mut buf,
        )
        .expect("array encode");
        let value = decode_pg_raw_value(&Type::INT4_ARRAY, Some(&buf)).expect("array decode");
        match value {
            DbRowValue::Array(values) => {
                assert_eq!(values.len(), 3);
                match values[0] {
                    DbRowValue::Int(val) => assert_eq!(val, 1),
                    ref other => panic!("unexpected array[0]: {other:?}"),
                }
                assert!(matches!(values[1], DbRowValue::Null));
                match values[2] {
                    DbRowValue::Int(val) => assert_eq!(val, 3),
                    ref other => panic!("unexpected array[2]: {other:?}"),
                }
            }
            other => panic!("unexpected array decode: {other:?}"),
        }
    }

    #[test]
    fn pg_array_lower_bounds() {
        let dims = vec![ArrayDimension {
            len: 1,
            lower_bound: 0,
        }];
        let elements = vec![Some(1)];
        let mut buf = BytesMut::new();
        array_to_sql(
            dims,
            Type::INT4.oid(),
            elements,
            |val, buf| match val {
                Some(val) => {
                    int4_to_sql(val, buf);
                    Ok(IsNull::No)
                }
                None => Ok(IsNull::Yes),
            },
            &mut buf,
        )
        .expect("array encode");
        let value = decode_pg_raw_value(&Type::INT4_ARRAY, Some(&buf)).expect("array lower bounds");
        match value {
            DbRowValue::ArrayWithBounds(array) => {
                assert_eq!(array.lower_bounds, vec![0]);
                assert_eq!(array.values.len(), 1);
                match array.values[0] {
                    DbRowValue::Int(val) => assert_eq!(val, 1),
                    ref other => panic!("unexpected array value: {other:?}"),
                }
            }
            other => panic!("unexpected array lower bounds decode: {other:?}"),
        }
    }

    #[test]
    fn db_query_null_requires_type() {
        let path = temp_db_path();
        let pool_path = path.clone();
        let pool = Pool::new(1, move || {
            DbConn::Sqlite(SqliteConn::open(&pool_path, SqliteOpenMode::ReadWrite).expect("sqlite"))
        });
        let cancel = CancelSet::default();
        let sqlite_registry = SqliteCancelRegistry::new();
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 1);
        let request = DbQueryRequest {
            db_alias: None,
            sql: "select ?".to_string(),
            params: DbParams::Positional {
                values: vec![DbParam::Raw(DbParamValue::Null)],
            },
            max_rows: Some(1),
            result_format: Some("json".to_string()),
            allow_write: None,
            tag: None,
        };
        let payload = rmp_serde::to_vec_named(&request).expect("encode");
        let envelope = RequestEnvelope {
            request_id: 1,
            entry: "db_query".to_string(),
            timeout_ms: 0,
            codec: "msgpack".to_string(),
            payload: Some(ByteBuf::from(payload)),
            payload_b64: None,
        };
        let err = execute_db_query_sync(&envelope, &ctx).expect_err("null should fail");
        assert_eq!(err.status, "InvalidInput");
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
        let sqlite_registry = SqliteCancelRegistry::new();
        let req = ListItemsRequest {
            user_id: 3,
            q: Some("x".into()),
            status: Some("open".into()),
            limit: Some(2),
            cursor: None,
        };
        let payload = super::encode_payload(&req, "msgpack").expect("encode");
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 3);
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
        let sqlite_registry = SqliteCancelRegistry::new();
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
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 1);
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
        let sqlite_registry = SqliteCancelRegistry::new();
        let req = ListItemsRequest {
            user_id: 1,
            q: None,
            status: None,
            limit: Some(2),
            cursor: None,
        };
        let ctx = exec_ctx(&cancel, &pool, &sqlite_registry, 1);
        let response = super::list_items_response(&req, &ctx).expect("sqlite list items");
        assert_eq!(response.items.len(), 2);
        assert_eq!(response.counts.open + response.counts.closed, 2);
        let _ = fs::remove_file(&path);
    }
}
