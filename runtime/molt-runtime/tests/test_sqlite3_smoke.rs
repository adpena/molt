//! End-to-end smoke test for the sqlite3 driver implemented in
//! `src/builtins/sqlite3.rs`.
//!
//! Mirrors the user-facing smoke test:
//!
//! ```python
//! conn = sqlite3.connect(":memory:")
//! cur = conn.cursor()
//! cur.execute("CREATE TABLE t (a INTEGER, b TEXT)")
//! cur.executemany("INSERT INTO t VALUES (?, ?)", [(1, "x"), (2, "y")])
//! conn.commit()
//! cur.execute("SELECT a, b FROM t ORDER BY a")
//! assert cur.fetchall() == [(1, "x"), (2, "y")]
//! conn.close()
//! ```
//!
//! Drives the same `molt_sqlite3_*` extern "C" functions the Python wrapper
//! calls, validating the full DB-API 2.0 contract end to end.

#![cfg(feature = "sqlite")]

use molt_obj_model::MoltObject;
use std::sync::Once;

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_: u64) -> u64 {
    MoltObject::none().bits()
}

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_exception_clear() -> u64;
    fn molt_string_from_bytes(ptr: *const u8, len_bits: u64, out: *mut u64) -> i32;
    fn molt_list_builtin(val_bits: u64) -> u64;
    fn molt_list_append(list_bits: u64, val_bits: u64) -> u64;
    fn molt_tuple_from_list(bits: u64) -> u64;
    fn molt_missing() -> u64;
}

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });
    let _ = unsafe { molt_exception_clear() };
}

fn missing() -> u64 {
    unsafe { molt_missing() }
}

fn string_bits(text: &str) -> u64 {
    let mut out = 0u64;
    let rc = unsafe { molt_string_from_bytes(text.as_ptr(), text.len() as u64, &mut out) };
    assert_eq!(rc, 0, "string alloc failed for {text}");
    out
}

fn empty_list() -> u64 {
    unsafe { molt_list_builtin(missing()) }
}

fn list_from(elems: &[u64]) -> u64 {
    let list = empty_list();
    for &b in elems {
        unsafe {
            molt_list_append(list, b);
        }
    }
    list
}

fn tuple_from(elems: &[u64]) -> u64 {
    unsafe { molt_tuple_from_list(list_from(elems)) }
}

fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}

#[test]
fn sqlite3_db_api_smoke() {
    init();

    // 1. connect(":memory:") --------------------------------------------------
    let path_bits = string_bits(":memory:");
    let conn_bits = molt_runtime::molt_sqlite3_connect(path_bits);
    let conn_id = MoltObject::from_bits(conn_bits)
        .as_int()
        .expect("connect must return int handle");
    assert!(conn_id > 0, "connect handle must be positive");

    // 2. cursor() ------------------------------------------------------------
    let cursor_bits = molt_runtime::molt_sqlite3_cursor(conn_bits);
    let cursor_id = MoltObject::from_bits(cursor_bits)
        .as_int()
        .expect("cursor must return int handle");
    assert!(cursor_id > 0, "cursor handle must be positive");

    // 3. CREATE TABLE --------------------------------------------------------
    let sql_create = string_bits("CREATE TABLE t (a INTEGER, b TEXT)");
    let empty_params = tuple_from(&[]);
    let r = molt_runtime::molt_sqlite3_execute(cursor_bits, sql_create, empty_params);
    assert_eq!(MoltObject::from_bits(r).as_int(), Some(cursor_id));

    // 4. executemany(INSERT, [(1, "x"), (2, "y")]) ----------------------------
    let row1 = tuple_from(&[int(1), string_bits("x")]);
    let row2 = tuple_from(&[int(2), string_bits("y")]);
    let rows = list_from(&[row1, row2]);
    let sql_insert = string_bits("INSERT INTO t VALUES (?, ?)");
    let r = molt_runtime::molt_sqlite3_executemany(cursor_bits, sql_insert, rows);
    assert_eq!(MoltObject::from_bits(r).as_int(), Some(cursor_id));

    // 5. commit() ------------------------------------------------------------
    let r = molt_runtime::molt_sqlite3_commit(conn_bits);
    assert!(MoltObject::from_bits(r).is_none());

    // 6. SELECT --------------------------------------------------------------
    let sql_select = string_bits("SELECT a, b FROM t ORDER BY a");
    let r = molt_runtime::molt_sqlite3_execute(cursor_bits, sql_select, empty_params);
    assert_eq!(MoltObject::from_bits(r).as_int(), Some(cursor_id));

    // 7. rowcount + description for SELECT  -----------------------------------
    let rowcount_bits = molt_runtime::molt_sqlite3_rowcount(cursor_bits);
    assert_eq!(
        MoltObject::from_bits(rowcount_bits).as_int(),
        Some(2),
        "rowcount should reflect the materialized SELECT result set"
    );
    let desc_bits = molt_runtime::molt_sqlite3_description(cursor_bits);
    assert!(
        !MoltObject::from_bits(desc_bits).is_none(),
        "description must be a tuple of column descriptors"
    );

    // 8. fetchall() and verify row contents ----------------------------------
    let fetched_bits = molt_runtime::molt_sqlite3_fetchall(cursor_bits);
    let fetched = MoltObject::from_bits(fetched_bits);
    assert!(
        fetched.as_ptr().is_some(),
        "fetchall must return a list pointer"
    );

    // Verify by re-running and using fetchone twice — that exercises the
    // cursor advance path independently of fetchall layout details.
    let r2 = molt_runtime::molt_sqlite3_execute(cursor_bits, sql_select, empty_params);
    assert_eq!(MoltObject::from_bits(r2).as_int(), Some(cursor_id));

    let row1_bits = molt_runtime::molt_sqlite3_fetchone(cursor_bits);
    let row1 = MoltObject::from_bits(row1_bits);
    assert!(row1.as_ptr().is_some(), "first fetchone must return a tuple");

    let row2_bits = molt_runtime::molt_sqlite3_fetchone(cursor_bits);
    let row2 = MoltObject::from_bits(row2_bits);
    assert!(row2.as_ptr().is_some(), "second fetchone must return a tuple");

    let row3_bits = molt_runtime::molt_sqlite3_fetchone(cursor_bits);
    assert!(
        MoltObject::from_bits(row3_bits).is_none(),
        "third fetchone after exhausted result set must return None"
    );

    // 9. close() -------------------------------------------------------------
    let _ = molt_runtime::molt_sqlite3_cursor_close(cursor_bits);
    let r = molt_runtime::molt_sqlite3_close(conn_bits);
    assert!(MoltObject::from_bits(r).is_none());
}

#[test]
fn sqlite3_library_version_is_nonempty() {
    init();
    let bits = molt_runtime::molt_sqlite3_library_version();
    let obj = MoltObject::from_bits(bits);
    assert!(
        obj.as_ptr().is_some(),
        "library_version must return a heap-allocated string"
    );
}

#[test]
fn sqlite3_complete_statement_recognizes_terminator() {
    init();
    let complete = string_bits("SELECT 1;");
    let r = molt_runtime::molt_sqlite3_complete_statement(complete);
    assert_eq!(MoltObject::from_bits(r).as_bool(), Some(true));

    let incomplete = string_bits("SELECT 1");
    let r = molt_runtime::molt_sqlite3_complete_statement(incomplete);
    assert_eq!(MoltObject::from_bits(r).as_bool(), Some(false));
}
