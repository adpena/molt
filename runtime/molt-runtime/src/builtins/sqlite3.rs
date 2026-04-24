use crate::*;
use molt_lang_db::SqliteConn;
use molt_lang_db::SqliteOpenMode;
use std::path::Path;

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_connect(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path_obj = obj_from_bits(path_bits);
        let Some(path_str) = string_obj_to_owned(path_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "database path must be a string");
        };

        let path = Path::new(&path_str);
        match SqliteConn::open(path, SqliteOpenMode::ReadWrite) {
            Ok(conn) => {
                let boxed = Box::new(conn);
                let ptr = Box::into_raw(boxed) as *mut u8;
                let handle = register_ptr(ptr);
                MoltObject::from_int(handle as i64).bits()
            }
            Err(e) => raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("sqlite3 connect failed: {}", e),
            ),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match to_i64(obj_from_bits(handle_bits)) {
            Some(h) => h as u64,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        if let Some(ptr) = release_ptr(handle as *mut u8) {
            let _ = unsafe { Box::from_raw(ptr as *mut SqliteConn) };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sqlite3_execute(handle_bits: u64, sql_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match to_i64(obj_from_bits(handle_bits)) {
            Some(h) => h as u64,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let sql_obj = obj_from_bits(sql_bits);
        let Some(sql_str) = string_obj_to_owned(sql_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "sql must be a string");
        };

        let ptr = match resolve_ptr(handle) {
            Some(p) => p as *mut SqliteConn,
            None => {
                return raise_exception::<u64>(_py, "RuntimeError", "invalid connection handle");
            }
        };

        let conn = unsafe { &*ptr };
        let mut stmt = match conn.connection().prepare(&sql_str) {
            Ok(s) => s,
            Err(e) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("sqlite3 prepare failed: {}", e),
                );
            }
        };

        let column_count = stmt.column_count();
        let mut rows_list = Vec::new();

        let mut rows = match stmt.query([]) {
            Ok(r) => r,
            Err(e) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("sqlite3 query failed: {}", e),
                );
            }
        };

        while let Ok(Some(row)) = rows.next() {
            let mut row_list = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let val_bits = match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Null => MoltObject::none().bits(),
                    rusqlite::types::ValueRef::Integer(i) => MoltObject::from_int(i).bits(),
                    rusqlite::types::ValueRef::Real(f) => MoltObject::from_float(f).bits(),
                    rusqlite::types::ValueRef::Text(s) => {
                        let s_str = std::str::from_utf8(s).unwrap();
                        let ptr = alloc_string(_py, s_str.as_bytes());
                        MoltObject::from_ptr(ptr).bits()
                    }
                    rusqlite::types::ValueRef::Blob(b) => {
                        let ptr = alloc_bytes(_py, b);
                        MoltObject::from_ptr(ptr).bits()
                    }
                };
                row_list.push(val_bits);
            }
            let row_tuple_ptr = alloc_tuple(_py, &row_list);
            rows_list.push(MoltObject::from_ptr(row_tuple_ptr).bits());
            for b in row_list {
                dec_ref_bits(_py, b);
            }
        }

        let final_list_ptr = alloc_list(_py, &rows_list);
        for b in rows_list {
            dec_ref_bits(_py, b);
        }

        MoltObject::from_ptr(final_list_ptr).bits()
    })
}
