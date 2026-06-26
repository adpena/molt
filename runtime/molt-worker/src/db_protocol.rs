use arrow::array::{
    ArrayBuilder, ArrayRef, BinaryBuilder, BooleanBuilder, Float64Builder, Int32Builder,
    Int64Builder, ListBuilder, NullArray, NullBuilder, StringBuilder, StructBuilder, make_builder,
};
use arrow::datatypes::{DataType, Field, Fields, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use fallible_iterator_02::FallibleIterator;
use postgres_protocol::types::{ArrayDimension, Range, RangeBound, array_from_sql, range_from_sql};
use rusqlite::types::{Value, ValueRef};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use sqlparser::ast::{Query as SqlQuery, Statement as SqlStatement};
use sqlparser::parser::Parser;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_postgres::Row as PgRow;
use tokio_postgres::types::{FromSql, Kind, ToSql, Type};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParamStyle {
    DollarNumbered,
    QuestionNumbered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DbResultFormat {
    Json,
    Msgpack,
    ArrowIpc,
}

impl DbResultFormat {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, ExecError> {
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

    pub(super) fn codec(self) -> &'static str {
        match self {
            DbResultFormat::Json => "json",
            DbResultFormat::Msgpack => "msgpack",
            DbResultFormat::ArrowIpc => "arrow_ipc",
        }
    }
}

#[allow(dead_code)]
#[derive(Deserialize, Serialize)]
pub(super) struct DbQueryRequest {
    #[serde(default)]
    pub(super) db_alias: Option<String>,
    pub(super) sql: String,
    #[serde(default)]
    pub(super) params: DbParams,
    #[serde(default)]
    pub(super) max_rows: Option<u32>,
    #[serde(default)]
    pub(super) result_format: Option<String>,
    #[serde(default)]
    pub(super) allow_write: Option<bool>,
    #[serde(default)]
    pub(super) tag: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum DbParams {
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
pub(super) enum DbParam {
    Raw(DbParamValue),
    Typed {
        value: DbParamValue,
        #[serde(default)]
        r#type: Option<String>,
    },
}

#[allow(dead_code)]
#[derive(Deserialize)]
pub(super) struct DbNamedParam {
    pub(super) name: String,
    #[serde(flatten)]
    pub(super) param: DbParam,
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
pub(super) enum DbParamValue {
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

#[allow(dead_code)]
#[derive(Deserialize, Serialize)]
pub(super) struct DbQueryResponse {
    pub(super) columns: Vec<String>,
    pub(super) rows: Vec<Vec<DbRowValue>>,
    pub(super) row_count: usize,
}

#[derive(Deserialize, Serialize)]
pub(super) struct DbExecResponse {
    pub(super) rows_affected: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) last_insert_id: Option<i64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum DbRowValue {
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
pub(super) struct DbArray {
    pub(super) lower_bounds: Vec<i32>,
    pub(super) values: Vec<DbRowValue>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct DbInterval {
    pub(super) months: i32,
    pub(super) days: i32,
    pub(super) micros: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct DbRangeBound {
    pub(super) value: DbRowValue,
    pub(super) inclusive: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct DbRange {
    pub(super) empty: bool,
    pub(super) lower: Option<DbRangeBound>,
    pub(super) upper: Option<DbRangeBound>,
}

pub(super) struct DbQueryExecResult {
    pub(super) codec: String,
    pub(super) payload: Vec<u8>,
    pub(super) row_count: usize,
    pub(super) db_alias: String,
    pub(super) tag: Option<String>,
    pub(super) result_format: DbResultFormat,
}

pub(super) struct DbExecResult {
    pub(super) codec: String,
    pub(super) payload: Vec<u8>,
    pub(super) rows_affected: u64,
    pub(super) last_insert_id: Option<i64>,
    pub(super) db_alias: String,
    pub(super) tag: Option<String>,
    pub(super) result_format: DbResultFormat,
}

pub(super) enum ExecOutcome {
    Standard { codec: String, payload: Vec<u8> },
    DbQuery(DbQueryExecResult),
    DbExec(DbExecResult),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ArrowColumnType {
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

#[derive(Debug)]
pub(super) struct ExecError {
    pub(super) status: &'static str,
    pub(super) message: String,
}

#[allow(dead_code)]
pub(super) enum PgParam {
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
    pub(super) fn as_tosql(&self) -> &(dyn ToSql + Sync) {
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

#[allow(dead_code)]
#[derive(Clone)]
pub(super) struct DbParamSpec {
    pub(super) value: DbParamValue,
    pub(super) type_hint: Option<String>,
}

#[allow(dead_code)]
pub(super) fn parse_param_spec(param: DbParam) -> DbParamSpec {
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
pub(super) fn normalize_params_and_sql(
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
pub(super) fn normalize_named_params(
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
pub(super) enum SqlScanState {
    Normal,
    SingleQuote,
    DoubleQuote,
    LineComment,
    BlockComment,
    DollarQuote(String),
}

#[allow(dead_code)]
impl SqlScanState {
    pub(super) fn len(&self) -> usize {
        match self {
            SqlScanState::DollarQuote(tag) => tag.len(),
            _ => 1,
        }
    }
}

#[allow(dead_code)]
pub(super) fn parse_dollar_tag(sql: &str, start: usize) -> Option<String> {
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
pub(super) fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

#[allow(dead_code)]
pub(super) fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

#[allow(dead_code)]
pub(super) fn validate_query(
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
            Ok(wrapped)
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
pub(super) struct ValidatedExec {
    pub(super) sql: String,
    pub(super) is_insert: bool,
}

#[allow(dead_code)]
pub(super) fn validate_exec(
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
pub(super) fn wrap_query_limit(query: SqlQuery, max_rows: u32) -> String {
    format!("SELECT * FROM ({query}) AS _molt_sub LIMIT {max_rows}")
}

#[allow(dead_code)]
pub(super) fn resolve_pg_params(
    specs: Vec<DbParamSpec>,
) -> Result<(Vec<PgParam>, Vec<Type>), ExecError> {
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
pub(super) fn resolve_pg_param(spec: DbParamSpec) -> Result<(PgParam, Type), ExecError> {
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
                    });
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
pub(super) fn resolve_int_param(
    value: i64,
    hint: Option<&str>,
) -> Result<(PgParam, Type), ExecError> {
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
pub(super) fn resolve_float_param(
    value: f64,
    hint: Option<&str>,
) -> Result<(PgParam, Type), ExecError> {
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
pub(super) fn resolve_string_param(
    value: String,
    hint: Option<&str>,
) -> Result<(PgParam, Type), ExecError> {
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
pub(super) fn parse_pg_type(name: &str) -> Option<Type> {
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

pub(super) fn parse_pg_date(value: &str) -> Result<NaiveDate, ExecError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ExecError {
        status: "InvalidInput",
        message: "Invalid date format (expected YYYY-MM-DD)".to_string(),
    })
}

pub(super) fn parse_pg_time(value: &str) -> Result<NaiveTime, ExecError> {
    NaiveTime::parse_from_str(value, "%H:%M:%S%.f")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map_err(|_| ExecError {
            status: "InvalidInput",
            message: "Invalid time format (expected HH:MM:SS[.ffffff])".to_string(),
        })
}

pub(super) fn parse_pg_timestamp(value: &str) -> Result<NaiveDateTime, ExecError> {
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

pub(super) fn parse_pg_timestamptz(value: &str) -> Result<DateTime<Utc>, ExecError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ExecError {
            status: "InvalidInput",
            message: "Invalid timestamptz format (expected RFC3339)".to_string(),
        })
}

pub(super) fn resolve_sqlite_params(specs: Vec<DbParamSpec>) -> Result<Vec<Value>, ExecError> {
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

pub(super) fn sqlite_value_to_row(value: ValueRef<'_>) -> Result<DbRowValue, ExecError> {
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

pub(super) fn db_query_arrow_ipc_bytes(response: &DbQueryResponse) -> Result<Vec<u8>, ExecError> {
    let column_types = infer_arrow_column_types(response)?;
    let mut fields = Vec::with_capacity(response.columns.len());
    let mut arrays = Vec::with_capacity(response.columns.len());
    for (idx, name) in response.columns.iter().enumerate() {
        let column_type = &column_types[idx];
        let data_type = arrow_data_type(column_type);
        fields.push(Field::new(name, data_type.clone(), true));
        arrays.push(build_arrow_array(&response.rows, idx, column_type)?);
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

pub(super) fn infer_arrow_column_types(
    response: &DbQueryResponse,
) -> Result<Vec<ArrowColumnType>, ExecError> {
    let column_count = response.columns.len();
    let mut inferred: Vec<Option<ArrowColumnType>> = vec![None; column_count];
    for row in &response.rows {
        if row.len() != column_count {
            return Err(ExecError {
                status: "InternalError",
                message: "Row length mismatch while inferring Arrow schema".to_string(),
            });
        }
        for (idx, value) in row.iter().enumerate() {
            let name = &response.columns[idx];
            if let Some(next) = infer_arrow_value_type(value, name)? {
                let merged = merge_arrow_types(inferred[idx].take(), next, name)?;
                inferred[idx] = Some(merged);
            }
        }
    }
    Ok(inferred
        .into_iter()
        .map(|ty| ty.unwrap_or(ArrowColumnType::Null))
        .collect())
}

pub(super) fn build_arrow_array(
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

pub(super) fn arrow_data_type(column_type: &ArrowColumnType) -> DataType {
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

pub(super) fn list_data_type(element: &ArrowColumnType, depth: usize) -> DataType {
    let mut out = arrow_data_type(element);
    for _ in 0..depth {
        out = DataType::List(Arc::new(Field::new("item", out, true)));
    }
    out
}

pub(super) fn arrow_type_label(column_type: &ArrowColumnType) -> &'static str {
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

pub(super) fn merge_arrow_types(
    current: Option<ArrowColumnType>,
    next: ArrowColumnType,
    name: &str,
) -> Result<ArrowColumnType, ExecError> {
    match current {
        None => Ok(next),
        Some(current) => merge_arrow_non_null(current, next, name),
    }
}

pub(super) fn merge_arrow_non_null(
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

pub(super) fn infer_arrow_value_type(
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

pub(super) fn infer_array_shape(
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

pub(super) fn infer_range_element_type(
    range: &DbRange,
    name: &str,
) -> Result<ArrowColumnType, ExecError> {
    let mut current: Option<ArrowColumnType> = None;
    if let Some(bound) = range.lower.as_ref()
        && let Some(next) = infer_arrow_value_type(&bound.value, name)?
    {
        current = Some(merge_arrow_types(current, next, name)?);
    }
    if let Some(bound) = range.upper.as_ref()
        && let Some(next) = infer_arrow_value_type(&bound.value, name)?
    {
        current = Some(merge_arrow_types(current, next, name)?);
    }
    Ok(current.unwrap_or(ArrowColumnType::Null))
}

pub(super) fn append_arrow_value(
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
                    });
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
                    });
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
                    });
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
                    });
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
                    });
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
                    });
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

pub(super) type DynListBuilder = ListBuilder<Box<dyn ArrayBuilder>>;

pub(super) fn append_array_struct(
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
            });
        }
    }
    Ok(())
}

pub(super) fn append_range_struct(
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
            });
        }
    }
    Ok(())
}

pub(super) fn append_range_bound(
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

pub(super) fn append_interval_struct(
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
            });
        }
    }
    Ok(())
}

pub(super) fn append_list_value(
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
                    });
                }
            }
        }
    }
    builder.append(true);
    Ok(())
}

pub(super) fn append_i32_list(
    builder: &mut DynListBuilder,
    values: &[i32],
) -> Result<(), ExecError> {
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

pub(super) fn append_struct_field_value(
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

pub(super) fn append_list_null(builder: &mut DynListBuilder) {
    builder.append(false);
}

pub(super) fn default_array_bounds(dims: usize) -> Vec<i32> {
    vec![1; dims.max(1)]
}

pub(super) fn arrow_builder_mismatch(context: &str) -> ExecError {
    ExecError {
        status: "InternalError",
        message: format!("Arrow IPC builder mismatch ({context})"),
    }
}

pub(super) struct PgRawValue(Vec<u8>);

impl PgRawValue {
    pub(super) fn as_slice(&self) -> &[u8] {
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

pub(super) fn decode_pg_raw_value(ty: &Type, raw: Option<&[u8]>) -> Result<DbRowValue, ExecError> {
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

pub(super) fn decode_pg_array_value(ty: &Type, raw: &[u8]) -> Result<DbRowValue, ExecError> {
    let element_type = match ty.kind() {
        Kind::Array(element_type) => element_type,
        _ => {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Postgres type '{}' is not an array", ty.name()),
            });
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
    let mut values_iter = values.into_iter();
    let nested = build_pg_array(&dimensions, &mut values_iter)?;
    if values_iter.next().is_some() {
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

pub(super) fn build_pg_array(
    dimensions: &[ArrayDimension],
    values: &mut std::vec::IntoIter<DbRowValue>,
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
            let Some(value) = values.next() else {
                return Err(ExecError {
                    status: "InternalError",
                    message: "Postgres array decode mismatch (missing values)".to_string(),
                });
            };
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

pub(super) fn read_be_i32(buf: &mut &[u8], context: &str) -> Result<i32, ExecError> {
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

pub(super) fn decode_pg_range_value(ty: &Type, raw: &[u8]) -> Result<DbRowValue, ExecError> {
    let element_type = match ty.kind() {
        Kind::Range(element_type) => element_type,
        _ => {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Postgres type '{}' is not a range", ty.name()),
            });
        }
    };
    let decoded = decode_pg_range_raw(element_type, raw)?;
    Ok(DbRowValue::Range(Box::new(decoded)))
}

pub(super) fn decode_pg_range_raw(element_type: &Type, raw: &[u8]) -> Result<DbRange, ExecError> {
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

pub(super) fn decode_pg_multirange_value(ty: &Type, raw: &[u8]) -> Result<DbRowValue, ExecError> {
    let element_type = match ty.kind() {
        Kind::Multirange(element_type) => element_type,
        _ => {
            return Err(ExecError {
                status: "InvalidInput",
                message: format!("Postgres type '{}' is not a multirange", ty.name()),
            });
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

pub(super) fn decode_pg_range_bound(
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

pub(super) fn decode_pg_interval(raw: &[u8]) -> Result<DbInterval, ExecError> {
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

pub(super) fn pg_row_values(
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
