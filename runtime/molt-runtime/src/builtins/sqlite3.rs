//! Intrinsics for the `sqlite3` stdlib module.
//!
//! Implements a CPython-compatible DB-API 2.0 driver on top of `rusqlite`,
//! gated behind the `sqlite` Cargo feature so that builds that do not need
//! SQLite (or that target environments where `rusqlite` cannot link) can
//! omit the dependency entirely.
//!
//! ## Architecture
//!
//! Two thread-local handle tables encode the connection/cursor state machine:
//!
//! * `CONNECTIONS` maps an `i64` connection handle to a `ConnectionState`
//!   that wraps a `rusqlite::Connection` plus DB-API metadata (autocommit,
//!   pending transaction flag, total_changes baseline).
//! * `CURSORS` maps an `i64` cursor handle to a `CursorState` that holds a
//!   reference to its parent connection by handle id, the buffered result
//!   set produced by the last `execute*` call, and the latest values for
//!   `description`, `rowcount`, and `lastrowid`.
//!
//! Results are buffered eagerly: `execute` runs the statement to completion,
//! materializes every row as a `Vec<MoltValue>`, and stashes it in the
//! cursor.  This keeps the lifetime story simple (no borrowed `Statement`
//! or `Rows` outliving its connection) and matches the typical usage of
//! the CPython driver — pysqlite also implements `description`/`rowcount`
//! via the same eager-buffering strategy.
//!
//! All Python-visible errors are raised as `RuntimeError` with a descriptive
//! prefix.  The Python-side `sqlite3/__init__.py` translates the prefixes
//! into the proper DB-API exception subclasses (`OperationalError`,
//! `IntegrityError`, `ProgrammingError`, …).

#![cfg(feature = "sqlite")]

use crate::{
    MoltObject, PyToken, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_bytes, alloc_list,
    alloc_string, alloc_tuple, bytes_data, bytes_len, dec_ref_bits, obj_from_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_i64,
};
use molt_db::{SqliteConn, SqliteOpenMode, rusqlite};
use rusqlite::ErrorCode;
use rusqlite::types::{Value as SqlValue, ValueRef};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// State containers
// ---------------------------------------------------------------------------

static NEXT_CONN_ID: AtomicI64 = AtomicI64::new(1);
static NEXT_CURSOR_ID: AtomicI64 = AtomicI64::new(1);

fn next_conn_id() -> i64 {
    NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_cursor_id() -> i64 {
    NEXT_CURSOR_ID.fetch_add(1, Ordering::Relaxed)
}

/// One row materialized from the SQLite result set, parameterized over
/// the storage type used for column values (typically `MoltValue`).
type MaterializedRow = Vec<MoltValue>;

/// Owned representation of a single column value returned by SQLite.
///
/// Decoupling from the runtime's NaN-boxed bits gives us a stable, lifetime-
/// free storage for buffered rows without juggling refcounts on the hot path.
/// The value is converted to runtime bits at fetch time inside the GIL.
#[derive(Clone, Debug)]
enum MoltValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

struct ConnectionState {
    conn: SqliteConn,
    /// Mirrors CPython's default isolation_level="" semantics: an implicit
    /// transaction is opened on the first DML statement after a commit/
    /// rollback and closed on `commit()` / `rollback()` / `close()`.
    in_transaction: bool,
    /// Snapshot of `connection.total_changes()` at connection open time.
    /// Used by Connection.total_changes to subtract the open-time baseline
    /// so the property reports per-connection-lifetime changes, matching
    /// CPython's sqlite3.Connection.total_changes semantics.
    last_total_changes: i64,
    closed: bool,
}

struct CursorState {
    /// Parent connection handle id.  We keep an id (not a reference) so the
    /// borrow checker stays out of our way; lookups go through the
    /// thread-local `CONNECTIONS` map on each call.
    conn_id: i64,
    rows: Vec<MaterializedRow>,
    /// Index into `rows` of the next row to be returned by `fetchone` /
    /// `fetchmany`.
    cursor_pos: usize,
    /// Column metadata captured from the most recent statement.  Stored as
    /// `(name, type_code, display_size, internal_size, precision, scale, null_ok)`
    /// using DB-API 2.0 7-tuples.  We only fill the column name; everything
    /// else is `None`, matching CPython's pysqlite.
    description: Option<Vec<String>>,
    rowcount: i64,
    lastrowid: Option<i64>,
    closed: bool,
    /// Index of the next row in a `fetchall`/`fetchmany` iteration relative
    /// to `arraysize`.  Currently unused beyond `fetchmany(size)` parameter.
    arraysize: i64,
}

thread_local! {
    static CONNECTIONS: RefCell<HashMap<i64, ConnectionState>> =
        RefCell::new(HashMap::new());
    static CURSORS: RefCell<HashMap<i64, CursorState>> =
        RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Error translation
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` to the (exception_kind, message) pair raised on
/// the Python side.  The exception kind is encoded as a string prefix so the
/// Python wrapper can pick the correct DB-API subclass without needing
/// runtime support for richer exception types.
fn classify_sqlite_error(err: &rusqlite::Error) -> (&'static str, String) {
    let msg = err.to_string();
    match err {
        rusqlite::Error::SqliteFailure(ffi_err, _) => match ffi_err.code {
            ErrorCode::ConstraintViolation => ("__IntegrityError__", msg),
            ErrorCode::TypeMismatch => ("__InterfaceError__", msg),
            ErrorCode::ApiMisuse | ErrorCode::ParameterOutOfRange => ("__ProgrammingError__", msg),
            ErrorCode::DatabaseCorrupt
            | ErrorCode::NotADatabase
            | ErrorCode::SystemIoFailure => ("__DatabaseError__", msg),
            _ => ("__OperationalError__", msg),
        },
        rusqlite::Error::InvalidParameterName(_)
        | rusqlite::Error::InvalidParameterCount(_, _)
        | rusqlite::Error::InvalidColumnName(_)
        | rusqlite::Error::InvalidColumnIndex(_)
        | rusqlite::Error::InvalidQuery
        | rusqlite::Error::MultipleStatement
        | rusqlite::Error::ExecuteReturnedResults => ("__ProgrammingError__", msg),
        rusqlite::Error::ToSqlConversionFailure(_)
        | rusqlite::Error::FromSqlConversionFailure(_, _, _)
        | rusqlite::Error::IntegralValueOutOfRange(_, _)
        | rusqlite::Error::Utf8Error(_, _)
        | rusqlite::Error::InvalidColumnType(_, _, _) => ("__InterfaceError__", msg),
        _ => ("__DatabaseError__", msg),
    }
}

fn raise_sqlite_error(_py: &PyToken<'_>, err: &rusqlite::Error) -> u64 {
    let (kind, msg) = classify_sqlite_error(err);
    let formatted = format!("{kind} {msg}");
    raise_exception::<u64>(_py, "RuntimeError", &formatted)
}

// ---------------------------------------------------------------------------
// Bit conversions
// ---------------------------------------------------------------------------

fn extract_text(bits: u64) -> Option<String> {
    string_obj_to_owned(obj_from_bits(bits))
}

fn extract_bytes(bits: u64) -> Option<Vec<u8>> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
        return None;
    }
    let len = unsafe { bytes_len(ptr) };
    let data = unsafe { std::slice::from_raw_parts(bytes_data(ptr), len) };
    Some(data.to_vec())
}

/// Decode a single Python value into a SQLite `Value`.  Mirrors the type
/// coercion table from CPython's pysqlite default adapter:
///
/// | Python      | SQLite     |
/// | ----------- | ---------- |
/// | `None`      | `NULL`     |
/// | `int`       | `INTEGER`  |
/// | `bool`      | `INTEGER`  |
/// | `float`     | `REAL`     |
/// | `str`       | `TEXT`     |
/// | `bytes`     | `BLOB`     |
fn molt_to_sql_value(bits: u64) -> Result<SqlValue, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(SqlValue::Null);
    }
    if let Some(b) = obj.as_bool() {
        return Ok(SqlValue::Integer(if b { 1 } else { 0 }));
    }
    if let Some(i) = to_i64(obj) {
        return Ok(SqlValue::Integer(i));
    }
    if let Some(f) = obj.as_float() {
        return Ok(SqlValue::Real(f));
    }
    if let Some(s) = string_obj_to_owned(obj) {
        return Ok(SqlValue::Text(s));
    }
    if let Some(b) = extract_bytes(bits) {
        return Ok(SqlValue::Blob(b));
    }
    Err(format!("unsupported parameter type for SQLite (bits=0x{bits:x})"))
}

/// Decode a parameter sequence (list/tuple) into a Vec of SQLite values.
fn decode_param_seq(seq_bits: u64) -> Result<Vec<SqlValue>, String> {
    let obj = obj_from_bits(seq_bits);
    if obj.is_none() {
        return Ok(Vec::new());
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err("parameters must be a list or tuple".into());
    };
    let tid = unsafe { object_type_id(ptr) };
    if tid != TYPE_ID_LIST && tid != TYPE_ID_TUPLE {
        return Err("parameters must be a list or tuple".into());
    }
    let elems = unsafe { seq_vec_ref(ptr) };
    let mut out = Vec::with_capacity(elems.len());
    for &elem_bits in elems.iter() {
        out.push(molt_to_sql_value(elem_bits)?);
    }
    Ok(out)
}

/// Convert a buffered `MoltValue` into runtime bits, allocating heap-backed
/// strings/bytes/lists as needed.  The caller is responsible for managing
/// the refcount of the returned bits (typically by stuffing them into a
/// tuple via `alloc_tuple`).
fn molt_value_to_bits(_py: &PyToken<'_>, value: &MoltValue) -> u64 {
    match value {
        MoltValue::Null => MoltObject::none().bits(),
        MoltValue::Integer(i) => MoltObject::from_int(*i).bits(),
        MoltValue::Real(f) => MoltObject::from_float(*f).bits(),
        MoltValue::Text(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
        MoltValue::Blob(b) => {
            let ptr = alloc_bytes(_py, b);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    }
}

/// Convert a `ValueRef` from a row into our owned `MoltValue` representation.
fn value_ref_to_molt(value_ref: ValueRef<'_>) -> MoltValue {
    match value_ref {
        ValueRef::Null => MoltValue::Null,
        ValueRef::Integer(i) => MoltValue::Integer(i),
        ValueRef::Real(f) => MoltValue::Real(f),
        ValueRef::Text(bytes) => {
            // SQLite text is UTF-8 by spec.  Use lossy conversion to mirror
            // CPython's default text_factory=str behavior for non-strict text.
            MoltValue::Text(String::from_utf8_lossy(bytes).into_owned())
        }
        ValueRef::Blob(bytes) => MoltValue::Blob(bytes.to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn with_connection<R>(
    _py: &PyToken<'_>,
    handle: i64,
    f: impl FnOnce(&mut ConnectionState) -> Result<R, rusqlite::Error>,
) -> Result<R, u64> {
    let result = CONNECTIONS.with(|map| {
        let mut borrow = map.borrow_mut();
        let state = match borrow.get_mut(&handle) {
            Some(s) => s,
            None => {
                return Err(Either::Left(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "Connection is not valid",
                )));
            }
        };
        if state.closed {
            return Err(Either::Left(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "__ProgrammingError__ Cannot operate on a closed database.",
            )));
        }
        f(state).map_err(Either::Right)
    });
    result.map_err(|e| match e {
        Either::Left(bits) => bits,
        Either::Right(err) => raise_sqlite_error(_py, &err),
    })
}

fn with_cursor<R>(
    _py: &PyToken<'_>,
    handle: i64,
    f: impl FnOnce(&mut CursorState) -> Result<R, &'static str>,
) -> Result<R, u64> {
    let result = CURSORS.with(|map| {
        let mut borrow = map.borrow_mut();
        let state = match borrow.get_mut(&handle) {
            Some(s) => s,
            None => return Err("invalid cursor handle"),
        };
        if state.closed {
            return Err("__ProgrammingError__ Cannot operate on a closed cursor.");
        }
        f(state)
    });
    result.map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", msg))
}

enum Either<L, R> {
    Left(L),
    Right(R),
}

/// Determine whether a SQL statement is a DML/SELECT-style query that
/// implicitly opens a transaction in CPython's default isolation level.
/// We intentionally keep this conservative — anything other than SELECT,
/// EXPLAIN, or PRAGMA opens a transaction.  This matches the CPython
/// behavior of opening a transaction before INSERT/UPDATE/DELETE/REPLACE
/// (and any DDL inside an explicit transaction).
fn statement_starts_transaction(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    let upper: String = trimmed.chars().take(16).flat_map(|c| c.to_uppercase()).collect();
    let upper_sliced = upper.trim_start();
    !(upper_sliced.starts_with("SELECT")
        || upper_sliced.starts_with("EXPLAIN")
        || upper_sliced.starts_with("PRAGMA")
        || upper_sliced.starts_with("BEGIN")
        || upper_sliced.starts_with("COMMIT")
        || upper_sliced.starts_with("END")
        || upper_sliced.starts_with("ROLLBACK")
        || upper_sliced.starts_with("SAVEPOINT")
        || upper_sliced.starts_with("RELEASE"))
}

fn statement_is_explicit_commit(sql: &str) -> bool {
    let trimmed = sql.trim_start().to_ascii_uppercase();
    trimmed.starts_with("COMMIT") || trimmed.starts_with("END")
}

fn statement_is_explicit_rollback(sql: &str) -> bool {
    let trimmed = sql.trim_start().to_ascii_uppercase();
    trimmed.starts_with("ROLLBACK")
}

fn statement_is_explicit_begin(sql: &str) -> bool {
    let trimmed = sql.trim_start().to_ascii_uppercase();
    trimmed.starts_with("BEGIN")
}

/// Run a single SQL statement against a connection state, materializing
/// the full result set into the provided cursor state.  Wraps the rusqlite
/// `Statement` lifetime so callers don't have to.
fn execute_statement(
    conn_state: &mut ConnectionState,
    sql: &str,
    params: &[SqlValue],
    cursor: &mut CursorState,
) -> Result<(), rusqlite::Error> {
    // Implicit transaction handling — match CPython's default isolation_level=""
    // (deferred): the first DML statement after a commit or rollback opens a
    // transaction implicitly.
    if statement_starts_transaction(sql) && !conn_state.in_transaction {
        conn_state
            .conn
            .connection()
            .execute_batch("BEGIN")?;
        conn_state.in_transaction = true;
    }

    if statement_is_explicit_begin(sql) {
        conn_state.in_transaction = true;
    }

    let total_before = conn_state.conn.connection().total_changes();

    let conn_ref = conn_state.conn.connection();
    let mut stmt = conn_ref.prepare(sql)?;

    // Capture column metadata up front (before stepping rows).  Pysqlite's
    // description tuple is `(name, None, None, None, None, None, None)` for
    // every column — we only fill the name, leaving the rest as None.
    let column_names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    cursor.description = if column_names.is_empty() {
        None
    } else {
        Some(column_names.clone())
    };

    let column_count = stmt.column_count();
    let mut materialized: Vec<MaterializedRow> = Vec::new();

    if column_count == 0 {
        // Pure DML/DDL — execute and skip iteration.
        stmt.execute(rusqlite::params_from_iter(params.iter()))?;
    } else {
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
        while let Some(row) = rows.next()? {
            let mut record = Vec::with_capacity(column_count);
            for col in 0..column_count {
                let vref = row.get_ref(col)?;
                record.push(value_ref_to_molt(vref));
            }
            materialized.push(record);
        }
    }

    drop(stmt);

    let total_after = conn_state.conn.connection().total_changes();
    let delta = total_after.saturating_sub(total_before);
    cursor.rows = materialized;
    cursor.cursor_pos = 0;
    cursor.rowcount = if column_count == 0 {
        delta as i64
    } else {
        // CPython sets rowcount = -1 for SELECT until all rows are consumed,
        // then sets it to the total row count.  We have a fully materialized
        // set up front, so we can safely report it immediately.
        cursor.rows.len() as i64
    };
    cursor.lastrowid = if column_count == 0 && delta > 0 {
        Some(conn_state.conn.connection().last_insert_rowid())
    } else {
        cursor.lastrowid // preserve previous lastrowid across SELECTs
    };

    if statement_is_explicit_commit(sql) {
        conn_state.in_transaction = false;
    }
    if statement_is_explicit_rollback(sql) {
        conn_state.in_transaction = false;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Connection intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_connect(database_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path_str = match extract_text(database_bits) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "database path must be a string"),
        };

        let path = Path::new(&path_str);
        let conn = match SqliteConn::open(path, SqliteOpenMode::ReadWrite) {
            Ok(c) => c,
            Err(e) => {
                let (kind, msg) = classify_sqlite_error(&e);
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("{kind} unable to open database file: {msg}"),
                );
            }
        };

        let total_changes = conn.connection().total_changes() as i64;
        let id = next_conn_id();
        CONNECTIONS.with(|map| {
            map.borrow_mut().insert(
                id,
                ConnectionState {
                    conn,
                    in_transaction: false,
                    last_total_changes: total_changes,
                    closed: false,
                },
            );
        });
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid connection handle"),
        };

        // Drop the connection state, which in turn drops the rusqlite::Connection
        // and finalizes any prepared statements still attached to it.  Cursors
        // referencing this connection become "stale" — they'll surface a
        // ProgrammingError on next use, mirroring CPython.
        let state_was_present = CONNECTIONS.with(|map| {
            let mut borrow = map.borrow_mut();
            let removed = borrow.remove(&id);
            removed.is_some()
        });
        if !state_was_present {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "__ProgrammingError__ Cannot operate on a closed database.",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_commit(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid connection handle"),
        };
        match with_connection(_py, id, |state| {
            if state.in_transaction {
                state.conn.connection().execute_batch("COMMIT")?;
                state.in_transaction = false;
            }
            Ok(())
        }) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_rollback(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid connection handle"),
        };
        match with_connection(_py, id, |state| {
            if state.in_transaction {
                state.conn.connection().execute_batch("ROLLBACK")?;
                state.in_transaction = false;
            }
            Ok(())
        }) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_in_transaction(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid connection handle"),
        };
        let in_txn = CONNECTIONS.with(|map| {
            let borrow = map.borrow();
            borrow.get(&id).map(|s| s.in_transaction).unwrap_or(false)
        });
        MoltObject::from_bool(in_txn).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_total_changes(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid connection handle"),
        };
        let total = match with_connection(_py, id, |state| {
            // Subtract the baseline captured at open time so the property
            // reflects per-connection lifetime changes (CPython parity).
            let now = state.conn.connection().total_changes() as i64;
            Ok(now.saturating_sub(state.last_total_changes))
        }) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        MoltObject::from_int(total).bits()
    })
}

// ---------------------------------------------------------------------------
// Cursor intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_cursor(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let conn_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid connection handle"),
        };
        // Validate the connection exists and is open.
        let exists = CONNECTIONS.with(|map| {
            let borrow = map.borrow();
            borrow
                .get(&conn_id)
                .map(|s| !s.closed)
                .unwrap_or(false)
        });
        if !exists {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "__ProgrammingError__ Cannot operate on a closed database.",
            );
        }
        let id = next_cursor_id();
        CURSORS.with(|map| {
            map.borrow_mut().insert(
                id,
                CursorState {
                    conn_id,
                    rows: Vec::new(),
                    cursor_pos: 0,
                    description: None,
                    rowcount: -1,
                    lastrowid: None,
                    closed: false,
                    arraysize: 1,
                },
            );
        });
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_cursor_close(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        CURSORS.with(|map| {
            let mut borrow = map.borrow_mut();
            if let Some(state) = borrow.get_mut(&id) {
                state.closed = true;
                state.rows.clear();
                state.description = None;
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_cursor_drop(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return MoltObject::none().bits(),
        };
        CURSORS.with(|map| {
            map.borrow_mut().remove(&id);
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_execute(
    cursor_bits: u64,
    sql_bits: u64,
    params_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let sql = match extract_text(sql_bits) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "sql must be a string"),
        };
        let params = match decode_param_seq(params_bits) {
            Ok(p) => p,
            Err(msg) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("__InterfaceError__ {msg}"),
                );
            }
        };

        // Look up the cursor's parent connection id without holding the
        // cursor borrow while we run the statement.
        let conn_id = CURSORS.with(|map| -> Result<i64, u64> {
            let borrow = map.borrow();
            let state = borrow.get(&cursor_id).ok_or_else(|| {
                raise_exception::<u64>(_py, "RuntimeError", "invalid cursor handle")
            })?;
            if state.closed {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "__ProgrammingError__ Cannot operate on a closed cursor.",
                ));
            }
            Ok(state.conn_id)
        });
        let conn_id = match conn_id {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let exec_result: Result<(), rusqlite::Error> = CONNECTIONS.with(|cmap| {
            let mut cborrow = cmap.borrow_mut();
            let conn_state = cborrow.get_mut(&conn_id).ok_or_else(|| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                    Some("Connection is closed".into()),
                )
            })?;
            CURSORS.with(|map| -> Result<(), rusqlite::Error> {
                let mut borrow = map.borrow_mut();
                let cursor = borrow
                    .get_mut(&cursor_id)
                    .expect("cursor id was validated above");
                execute_statement(conn_state, &sql, &params, cursor)
            })
        });

        match exec_result {
            Ok(()) => MoltObject::from_int(cursor_id).bits(),
            Err(e) => raise_sqlite_error(_py, &e),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_executemany(
    cursor_bits: u64,
    sql_bits: u64,
    seq_of_params_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let sql = match extract_text(sql_bits) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "sql must be a string"),
        };
        let outer_obj = obj_from_bits(seq_of_params_bits);
        let outer_ptr = match outer_obj.as_ptr() {
            Some(p) => p,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "executemany() requires an iterable of parameter sequences",
                );
            }
        };
        let outer_tid = unsafe { object_type_id(outer_ptr) };
        if outer_tid != TYPE_ID_LIST && outer_tid != TYPE_ID_TUPLE {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "executemany() requires an iterable of parameter sequences",
            );
        }
        let outer_elems = unsafe { seq_vec_ref(outer_ptr) };
        let mut all_params: Vec<Vec<SqlValue>> = Vec::with_capacity(outer_elems.len());
        for &elem in outer_elems.iter() {
            match decode_param_seq(elem) {
                Ok(p) => all_params.push(p),
                Err(msg) => {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        &format!("__InterfaceError__ {msg}"),
                    );
                }
            }
        }

        let conn_id = CURSORS.with(|map| -> Result<i64, u64> {
            let borrow = map.borrow();
            let state = borrow.get(&cursor_id).ok_or_else(|| {
                raise_exception::<u64>(_py, "RuntimeError", "invalid cursor handle")
            })?;
            if state.closed {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "__ProgrammingError__ Cannot operate on a closed cursor.",
                ));
            }
            Ok(state.conn_id)
        });
        let conn_id = match conn_id {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let exec_result: Result<i64, rusqlite::Error> = CONNECTIONS.with(|cmap| {
            let mut cborrow = cmap.borrow_mut();
            let conn_state = cborrow.get_mut(&conn_id).ok_or_else(|| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                    Some("Connection is closed".into()),
                )
            })?;
            CURSORS.with(|map| -> Result<i64, rusqlite::Error> {
                let mut borrow = map.borrow_mut();
                let cursor = borrow
                    .get_mut(&cursor_id)
                    .expect("cursor id was validated above");
                let mut total: i64 = 0;
                for params in all_params.iter() {
                    execute_statement(conn_state, &sql, params, cursor)?;
                    if cursor.rowcount > 0 {
                        total += cursor.rowcount;
                    }
                }
                cursor.rowcount = total;
                cursor.rows.clear();
                cursor.cursor_pos = 0;
                Ok(total)
            })
        });

        match exec_result {
            Ok(_) => MoltObject::from_int(cursor_id).bits(),
            Err(e) => raise_sqlite_error(_py, &e),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_executescript(cursor_bits: u64, script_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let script = match extract_text(script_bits) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "script must be a string"),
        };

        let conn_id = CURSORS.with(|map| -> Result<i64, u64> {
            let borrow = map.borrow();
            let state = borrow.get(&cursor_id).ok_or_else(|| {
                raise_exception::<u64>(_py, "RuntimeError", "invalid cursor handle")
            })?;
            if state.closed {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "__ProgrammingError__ Cannot operate on a closed cursor.",
                ));
            }
            Ok(state.conn_id)
        });
        let conn_id = match conn_id {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        // executescript implicitly commits any open transaction before running
        // the script (CPython parity).
        let res: Result<(), rusqlite::Error> = CONNECTIONS.with(|cmap| {
            let mut cborrow = cmap.borrow_mut();
            let conn_state = cborrow.get_mut(&conn_id).ok_or_else(|| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                    Some("Connection is closed".into()),
                )
            })?;
            if conn_state.in_transaction {
                conn_state.conn.connection().execute_batch("COMMIT")?;
                conn_state.in_transaction = false;
            }
            conn_state.conn.connection().execute_batch(&script)?;
            // execute_batch may leave us inside a fresh transaction if the
            // script contains explicit BEGIN.  Inspect the connection.
            conn_state.in_transaction = !conn_state
                .conn
                .connection()
                .is_autocommit();
            Ok(())
        });

        // Reset cursor result state so subsequent fetches return nothing.
        CURSORS.with(|map| {
            if let Some(cursor) = map.borrow_mut().get_mut(&cursor_id) {
                cursor.rows.clear();
                cursor.cursor_pos = 0;
                cursor.rowcount = -1;
                cursor.description = None;
            }
        });

        match res {
            Ok(()) => MoltObject::from_int(cursor_id).bits(),
            Err(e) => raise_sqlite_error(_py, &e),
        }
    })
}

// ---------------------------------------------------------------------------
// Result fetchers
// ---------------------------------------------------------------------------

fn row_to_tuple_bits(_py: &PyToken<'_>, row: &MaterializedRow) -> u64 {
    let mut elems: Vec<u64> = Vec::with_capacity(row.len());
    for v in row.iter() {
        elems.push(molt_value_to_bits(_py, v));
    }
    let ptr = alloc_tuple(_py, &elems);
    // alloc_tuple incremented refs on the elems we copied in; release our
    // local refs to avoid leaking heap-backed strings/bytes.
    for b in &elems {
        dec_ref_bits(_py, *b);
    }
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_fetchone(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let row_opt = match with_cursor(_py, cursor_id, |cur| {
            if cur.cursor_pos < cur.rows.len() {
                let row = cur.rows[cur.cursor_pos].clone();
                cur.cursor_pos += 1;
                Ok(Some(row))
            } else {
                Ok(None)
            }
        }) {
            Ok(opt) => opt,
            Err(bits) => return bits,
        };
        match row_opt {
            Some(row) => row_to_tuple_bits(_py, &row),
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_fetchall(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let drained = match with_cursor(_py, cursor_id, |cur| {
            let drained: Vec<MaterializedRow> = cur.rows.drain(cur.cursor_pos..).collect();
            cur.cursor_pos = cur.rows.len();
            Ok(drained)
        }) {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let mut bits_vec: Vec<u64> = Vec::with_capacity(drained.len());
        for row in &drained {
            bits_vec.push(row_to_tuple_bits(_py, row));
        }
        let list_ptr = alloc_list(_py, &bits_vec);
        for b in &bits_vec {
            dec_ref_bits(_py, *b);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_fetchmany(cursor_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let size = match to_i64(obj_from_bits(size_bits)) {
            Some(v) if v > 0 => v as usize,
            Some(_) => 0,
            None => return raise_exception::<u64>(_py, "TypeError", "size must be an integer"),
        };

        let drained = match with_cursor(_py, cursor_id, |cur| {
            let take = size.min(cur.rows.len().saturating_sub(cur.cursor_pos));
            let end = cur.cursor_pos + take;
            let drained: Vec<MaterializedRow> = cur.rows[cur.cursor_pos..end].to_vec();
            cur.cursor_pos = end;
            Ok(drained)
        }) {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let mut bits_vec: Vec<u64> = Vec::with_capacity(drained.len());
        for row in &drained {
            bits_vec.push(row_to_tuple_bits(_py, row));
        }
        let list_ptr = alloc_list(_py, &bits_vec);
        for b in &bits_vec {
            dec_ref_bits(_py, *b);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// Cursor metadata
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_lastrowid(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let val = CURSORS.with(|map| {
            let borrow = map.borrow();
            borrow.get(&cursor_id).and_then(|s| s.lastrowid)
        });
        match val {
            Some(v) => MoltObject::from_int(v).bits(),
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_rowcount(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let val = CURSORS.with(|map| {
            let borrow = map.borrow();
            borrow.get(&cursor_id).map(|s| s.rowcount).unwrap_or(-1)
        });
        MoltObject::from_int(val).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_arraysize_get(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let val = CURSORS.with(|map| {
            map.borrow().get(&cursor_id).map(|s| s.arraysize).unwrap_or(1)
        });
        MoltObject::from_int(val).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_arraysize_set(cursor_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let size = match to_i64(obj_from_bits(size_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "size must be an integer"),
        };
        CURSORS.with(|map| {
            if let Some(s) = map.borrow_mut().get_mut(&cursor_id) {
                s.arraysize = size.max(1);
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_description(cursor_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cursor_id = match to_i64(obj_from_bits(cursor_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid cursor handle"),
        };
        let names = CURSORS.with(|map| {
            map.borrow()
                .get(&cursor_id)
                .and_then(|s| s.description.clone())
        });
        let Some(names) = names else {
            return MoltObject::none().bits();
        };

        // Build a tuple of 7-tuples: (name, None, None, None, None, None, None).
        let none_bits = MoltObject::none().bits();
        let mut entry_bits: Vec<u64> = Vec::with_capacity(names.len());
        for name in &names {
            let name_ptr = alloc_string(_py, name.as_bytes());
            let name_bits = if name_ptr.is_null() {
                none_bits
            } else {
                MoltObject::from_ptr(name_ptr).bits()
            };
            let inner: [u64; 7] = [name_bits, none_bits, none_bits, none_bits, none_bits, none_bits, none_bits];
            let inner_ptr = alloc_tuple(_py, &inner);
            // alloc_tuple incremented the name ref; drop our local copy.
            dec_ref_bits(_py, name_bits);
            if inner_ptr.is_null() {
                entry_bits.push(none_bits);
            } else {
                entry_bits.push(MoltObject::from_ptr(inner_ptr).bits());
            }
        }
        let outer_ptr = alloc_tuple(_py, &entry_bits);
        for b in &entry_bits {
            dec_ref_bits(_py, *b);
        }
        if outer_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(outer_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// Module-level helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_library_version() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let v = rusqlite::version();
        let ptr = alloc_string(_py, v.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_complete_statement(sql_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sql = match extract_text(sql_bits) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "sql must be a string"),
        };
        let cstr = match std::ffi::CString::new(sql) {
            Ok(c) => c,
            Err(_) => return MoltObject::from_bool(false).bits(),
        };
        // SAFETY: rusqlite re-exports the libsqlite3-sys bindings; calling
        // sqlite3_complete on a NUL-terminated string is well-defined.
        let rc = unsafe { rusqlite::ffi::sqlite3_complete(cstr.as_ptr()) };
        MoltObject::from_bool(rc != 0).bits()
    })
}
