//! Intrinsics for the `dbm.dumb` stdlib module.
//!
//! Implements a simple key-value database compatible with CPython's `dbm.dumb`.
//! Uses the same handle-based state machine pattern as `configparser.rs`.
//!
//! File format (CPython-compatible):
//! - `.dir`: text index, one `'key', (offset, size)\n` per entry
//! - `.dat`: binary data, values stored at offsets

use crate::{
    MoltObject, PyToken, TYPE_ID_BYTES, alloc_bytes, alloc_list, bytes_data, bytes_len,
    obj_from_bits, object_type_id, raise_exception, string_obj_to_owned, to_i64,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// Handle counter
// ---------------------------------------------------------------------------

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// DBM state
// ---------------------------------------------------------------------------

struct DumbState {
    dir_file: String,
    dat_file: String,
    bak_file: String,
    /// key → (offset_in_dat, size)
    index: HashMap<String, (u64, u64)>,
    flag: u8,
    modified: bool,
}

impl DumbState {
    fn is_readonly(&self) -> bool {
        self.flag == b'r'
    }
}

thread_local! {
    static DBM_HANDLES: RefCell<HashMap<i64, DumbState>> =
        RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Index parsing (CPython's .dir format)
// ---------------------------------------------------------------------------

/// Parse a `.dir` file.  Each line is: `'key', (offset, size)\n`
fn parse_dir_file(content: &str) -> HashMap<String, (u64, u64)> {
    let mut index = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Format: 'key', (offset, size)
        if let Some((key_part, rest)) = line.split_once(", (")
            && let Some((offset_str, size_str)) = rest.trim_end_matches(')').split_once(", ")
            && let (Ok(offset), Ok(size)) = (
                offset_str.trim().parse::<u64>(),
                size_str.trim().parse::<u64>(),
            )
        {
            let key = key_part
                .trim()
                .trim_start_matches('\'')
                .trim_end_matches('\'');
            index.insert(key.to_string(), (offset, size));
        }
    }
    index
}

/// Serialize index to `.dir` format.
fn serialize_dir(index: &HashMap<String, (u64, u64)>) -> String {
    let mut keys: Vec<&String> = index.keys().collect();
    keys.sort();
    let mut out = String::new();
    for key in keys {
        let (offset, size) = index[key];
        out.push_str(&format!("'{}', ({}, {})\n", key, offset, size));
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract raw bytes from a NaN-boxed bytes object.
fn extract_raw_bytes(bits: u64) -> Option<Vec<u8>> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
        return None;
    }
    let len = unsafe { bytes_len(ptr) };
    let data = unsafe { std::slice::from_raw_parts(bytes_data(ptr), len) };
    Some(data.to_vec())
}

/// Convert a `*mut u8` to NaN-boxed bits, returning None bits on null.
fn ptr_to_bits(ptr: *mut u8) -> u64 {
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Extract a key as a String from either str or bytes bits.
fn extract_key(_py: &PyToken, key_bits: u64) -> Option<String> {
    if let Some(s) = string_obj_to_owned(obj_from_bits(key_bits)) {
        return Some(s);
    }
    if let Some(data) = extract_raw_bytes(key_bits) {
        return Some(String::from_utf8_lossy(&data).into_owned());
    }
    let _ = raise_exception::<u64>(_py, "TypeError", "dbm key must be str or bytes");
    None
}

/// Extract a value as Vec<u8> from either str or bytes bits.
fn extract_bytes_value(_py: &PyToken, bits: u64) -> Option<Vec<u8>> {
    if let Some(data) = extract_raw_bytes(bits) {
        return Some(data);
    }
    if let Some(s) = string_obj_to_owned(obj_from_bits(bits)) {
        return Some(s.into_bytes());
    }
    let _ = raise_exception::<u64>(_py, "TypeError", "dbm value must be str or bytes");
    None
}

/// Read a slice from the .dat file.
fn read_dat_value(dat_file: &str, offset: u64, size: u64) -> Result<Vec<u8>, &'static str> {
    let mut file = fs::File::open(dat_file).map_err(|_| "cannot open .dat file")?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "seek failed on .dat file")?;
    let mut buf = vec![0u8; size as usize];
    file.read_exact(&mut buf)
        .map_err(|_| "read failed on .dat file")?;
    Ok(buf)
}

/// Append a value to .dat, return the offset at which it was written.
fn append_dat_value(dat_file: &str, value: &[u8]) -> Result<u64, &'static str> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dat_file)
        .map_err(|_| "cannot open .dat file for writing")?;
    let offset = file
        .seek(SeekFrom::End(0))
        .map_err(|_| "seek failed on .dat file")?;
    file.write_all(value)
        .map_err(|_| "write failed on .dat file")?;
    Ok(offset)
}

/// Sync state to disk: backup .dir → .bak, rewrite .dir.
fn sync_state(state: &mut DumbState) -> Result<(), String> {
    if std::path::Path::new(&state.dir_file).exists() {
        let _ = fs::copy(&state.dir_file, &state.bak_file);
    }
    let dir_content = serialize_dir(&state.index);
    fs::write(&state.dir_file, dir_content)
        .map_err(|e| format!("cannot write {}: {}", state.dir_file, e))?;
    state.modified = false;
    Ok(())
}

// ---------------------------------------------------------------------------
// Intrinsics
// ---------------------------------------------------------------------------

/// Open a dbm.dumb database.
/// flag: 'r' (read), 'w' (write), 'c' (create), 'n' (new/truncate)
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_open(path_bits: u64, flag_bits: u64, _mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "expected str for path"),
        };
        let flag_str = match string_obj_to_owned(obj_from_bits(flag_bits)) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "expected str for flag"),
        };
        let flag = match flag_str.as_str() {
            "r" => b'r',
            "w" => b'w',
            "c" => b'c',
            "n" => b'n',
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("invalid flag: {flag_str:?}"),
                );
            }
        };

        let dir_file = format!("{}.dir", path);
        let dat_file = format!("{}.dat", path);
        let bak_file = format!("{}.bak", path);

        let index = if flag == b'n' {
            if let Err(e) = fs::write(&dir_file, "") {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &format!("cannot create {}: {}", dir_file, e),
                );
            }
            if let Err(e) = fs::write(&dat_file, b"") {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &format!("cannot create {}: {}", dat_file, e),
                );
            }
            HashMap::new()
        } else {
            match fs::read_to_string(&dir_file) {
                Ok(content) => parse_dir_file(&content),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if flag == b'r' || flag == b'w' {
                        return raise_exception::<u64>(
                            _py,
                            "FileNotFoundError",
                            &format!("need '{}' for flag='{}'", dir_file, flag_str),
                        );
                    }
                    if let Err(e) = fs::write(&dir_file, "") {
                        return raise_exception::<u64>(
                            _py,
                            "OSError",
                            &format!("cannot create {}: {}", dir_file, e),
                        );
                    }
                    if let Err(e) = fs::write(&dat_file, b"") {
                        return raise_exception::<u64>(
                            _py,
                            "OSError",
                            &format!("cannot create {}: {}", dat_file, e),
                        );
                    }
                    HashMap::new()
                }
                Err(e) => {
                    return raise_exception::<u64>(
                        _py,
                        "OSError",
                        &format!("cannot open {}: {}", dir_file, e),
                    );
                }
            }
        };

        let state = DumbState {
            dir_file,
            dat_file,
            bak_file,
            index,
            flag,
            modified: false,
        };

        let id = next_handle_id();
        DBM_HANDLES.with(|map| {
            map.borrow_mut().insert(id, state);
        });
        MoltObject::from_int(id).bits()
    })
}

/// Get a value by key. Returns bytes.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_getitem(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let key = match extract_key(_py, key_bits) {
            Some(k) => k,
            None => return MoltObject::none().bits(),
        };

        let result = DBM_HANDLES.with(|map| {
            let borrow = map.borrow();
            let state = match borrow.get(&id) {
                Some(s) => s,
                None => return Err("invalid dbm handle"),
            };
            match state.index.get(&key) {
                Some(&(offset, size)) => read_dat_value(&state.dat_file, offset, size),
                None => Err("__KeyError__"),
            }
        });

        match result {
            Ok(data) => ptr_to_bits(alloc_bytes(_py, &data)),
            Err("__KeyError__") => raise_exception::<u64>(_py, "KeyError", &key),
            Err(msg) => raise_exception::<u64>(_py, "OSError", msg),
        }
    })
}

/// Set a key-value pair.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_setitem(handle_bits: u64, key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let key = match extract_key(_py, key_bits) {
            Some(k) => k,
            None => return MoltObject::none().bits(),
        };

        let value = match extract_bytes_value(_py, value_bits) {
            Some(v) => v,
            None => return MoltObject::none().bits(),
        };

        let result = DBM_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            let state = match borrow.get_mut(&id) {
                Some(s) => s,
                None => return Err("invalid dbm handle"),
            };
            if state.is_readonly() {
                return Err("cannot add item to database opened with flag 'r'");
            }
            let offset = match append_dat_value(&state.dat_file, &value) {
                Ok(off) => off,
                Err(msg) => return Err(msg),
            };
            state.index.insert(key, (offset, value.len() as u64));
            state.modified = true;
            Ok(())
        });

        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "OSError", msg),
        }
    })
}

/// Delete a key.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_delitem(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let key = match extract_key(_py, key_bits) {
            Some(k) => k,
            None => return MoltObject::none().bits(),
        };

        let result = DBM_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            let state = match borrow.get_mut(&id) {
                Some(s) => s,
                None => return Err("invalid dbm handle"),
            };
            if state.is_readonly() {
                return Err("cannot delete item from database opened with flag 'r'");
            }
            if state.index.remove(&key).is_none() {
                return Err("__KeyError__");
            }
            state.modified = true;
            Ok(())
        });

        match result {
            Ok(()) => MoltObject::none().bits(),
            Err("__KeyError__") => raise_exception::<u64>(_py, "KeyError", &key),
            Err(msg) => raise_exception::<u64>(_py, "OSError", msg),
        }
    })
}

/// Check if a key exists.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_contains(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let key = match extract_key(_py, key_bits) {
            Some(k) => k,
            None => return MoltObject::none().bits(),
        };

        let result = DBM_HANDLES.with(|map| {
            let borrow = map.borrow();
            match borrow.get(&id) {
                Some(state) => Ok(state.index.contains_key(&key)),
                None => Err("invalid dbm handle"),
            }
        });

        match result {
            Ok(v) => MoltObject::from_bool(v).bits(),
            Err(msg) => raise_exception::<u64>(_py, "OSError", msg),
        }
    })
}

/// Return a list of all keys (as bytes).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_keys(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let result = DBM_HANDLES.with(|map| {
            let borrow = map.borrow();
            match borrow.get(&id) {
                Some(state) => {
                    let keys: Vec<u64> = state
                        .index
                        .keys()
                        .map(|k| {
                            let ptr = alloc_bytes(_py, k.as_bytes());
                            ptr_to_bits(ptr)
                        })
                        .collect();
                    Ok(keys)
                }
                None => Err("invalid dbm handle"),
            }
        });

        match result {
            Ok(items) => ptr_to_bits(alloc_list(_py, &items)),
            Err(msg) => raise_exception::<u64>(_py, "OSError", msg),
        }
    })
}

/// Sync the database to disk.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_sync(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let result = DBM_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            let state = match borrow.get_mut(&id) {
                Some(s) => s,
                None => return Err("invalid dbm handle".to_string()),
            };
            if !state.modified || state.is_readonly() {
                return Ok(());
            }
            sync_state(state)
        });

        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "OSError", &msg),
        }
    })
}

/// Close the database (sync + release handle).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dbm_dumb_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "invalid handle"),
        };

        let result: Result<(), String> = DBM_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            if let Some(state) = borrow.get_mut(&id)
                && state.modified
                && !state.is_readonly()
            {
                sync_state(state)?;
            }
            borrow.remove(&id);
            Ok(())
        });

        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "OSError", &msg),
        }
    })
}
