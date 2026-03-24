//! `zipfile` module intrinsics for Molt.
//!
//! Provides handle-based ZIP archive read/write operations, delegating to
//! the `zip` crate for format compliance.
//!
//! ABI: NaN-boxed u64 in/out.

use molt_runtime_core::prelude::*;
use crate::bridge::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write, Cursor};
use std::sync::atomic::{AtomicI64, Ordering};

// ── Handle-id counter ───────────────────────────────────────────────────
static NEXT_ZIP_ID: AtomicI64 = AtomicI64::new(1);
fn next_zip_id() -> i64 {
    NEXT_ZIP_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Archive state ───────────────────────────────────────────────────────

enum ZipState {
    Reader {
        data: Vec<u8>,
    },
    Writer {
        path: String,
        entries: Vec<(String, Vec<u8>, u16)>, // (name, data, compression_method)
    },
}

thread_local! {
    static ZIP_MAP: RefCell<HashMap<i64, ZipState>> = RefCell::new(HashMap::new());
}

// ── open(path, mode) -> handle ──────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_open(path_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
            return raise_exception(_py, "TypeError", "path must be a string");
        };
        let Some(mode) = string_obj_to_owned(obj_from_bits(mode_bits)) else {
            return raise_exception(_py, "TypeError", "mode must be a string");
        };

        let id = next_zip_id();
        match mode.as_str() {
            "r" => {
                let data = match std::fs::read(&path) {
                    Ok(d) => d,
                    Err(e) => return raise_exception(_py, "FileNotFoundError", &format!("{}", e)),
                };
                ZIP_MAP.with(|m| {
                    m.borrow_mut().insert(id, ZipState::Reader { data });
                });
            }
            "w" => {
                ZIP_MAP.with(|m| {
                    m.borrow_mut().insert(id, ZipState::Writer {
                        path,
                        entries: Vec::new(),
                    });
                });
            }
            _ => return raise_exception(_py, "ValueError", "unsupported zipfile mode"),
        }
        MoltObject::from_int(id).bits()
    })
}

// ── close(handle) -> None ───────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };
        let state = ZIP_MAP.with(|m| m.borrow_mut().remove(&id));
        match state {
            Some(ZipState::Writer { path, entries }) => {
                // Write the zip file using the `zip` crate
                let file = match std::fs::File::create(&path) {
                    Ok(f) => f,
                    Err(e) => return raise_exception(_py, "IOError", &format!("{}", e)),
                };
                let mut writer = zip::ZipWriter::new(file);
                for (name, data, method) in &entries {
                    let options = zip::write::SimpleFileOptions::default()
                        .compression_method(if *method == 8 {
                            zip::CompressionMethod::Deflated
                        } else {
                            zip::CompressionMethod::Stored
                        });
                    if let Err(e) = writer.start_file(name.as_str(), options) {
                        return raise_exception(_py, "IOError", &format!("{}", e));
                    }
                    if let Err(e) = writer.write_all(data) {
                        return raise_exception(_py, "IOError", &format!("{}", e));
                    }
                }
                if let Err(e) = writer.finish() {
                    return raise_exception(_py, "IOError", &format!("{}", e));
                }
            }
            Some(ZipState::Reader { .. }) => { /* Nothing to do for readers */ }
            None => { /* Already closed */ }
        }
        MoltObject::none().bits()
    })
}

// ── writestr(handle, name, data, compress_type) -> None ─────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_writestr(
    handle_bits: u64,
    name_bits: u64,
    data_bits: u64,
    method_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception(_py, "TypeError", "name must be a string");
        };
        let method = to_i64(obj_from_bits(method_bits)).unwrap_or(0) as u16;

        // Get data as bytes
        let data_obj = obj_from_bits(data_bits);
        let data: Vec<u8> = if let Some(s) = string_obj_to_owned(data_obj) {
            s.into_bytes()
        } else if let Some(ptr) = data_obj.as_ptr() {
            if unsafe { object_type_id(ptr) } == TYPE_ID_BYTES {
                let len = unsafe { bytes_len(ptr) };
                let raw = unsafe { bytes_data(ptr) };
                unsafe { std::slice::from_raw_parts(raw, len) }.to_vec()
            } else {
                return raise_exception(_py, "TypeError", "data must be bytes or str");
            }
        } else {
            return raise_exception(_py, "TypeError", "data must be bytes or str");
        };

        ZIP_MAP.with(|m| {
            let mut map = m.borrow_mut();
            let Some(state) = map.get_mut(&id) else {
                return raise_exception(_py, "ValueError", "invalid zip handle");
            };
            match state {
                ZipState::Writer { entries, .. } => {
                    entries.push((name, data, method));
                    MoltObject::none().bits()
                }
                _ => raise_exception(_py, "ValueError", "writestr requires mode='w'"),
            }
        })
    })
}

// ── namelist(handle) -> list[str] ───────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_namelist(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };

        ZIP_MAP.with(|m| {
            let map = m.borrow();
            let Some(state) = map.get(&id) else {
                return raise_exception(_py, "ValueError", "invalid zip handle");
            };
            match state {
                ZipState::Reader { data } => {
                    let cursor = Cursor::new(data);
                    let mut archive = match zip::ZipArchive::new(cursor) {
                        Ok(a) => a,
                        Err(e) => return raise_exception(_py, "ValueError", &format!("bad zip: {}", e)),
                    };
                    let mut bits = Vec::with_capacity(archive.len());
                    for i in 0..archive.len() {
                        if let Ok(entry) = archive.by_index(i) {
                            let name = entry.name().to_string();
                            let ptr = alloc_string(_py, name.as_bytes());
                            bits.push(MoltObject::from_ptr(ptr).bits());
                        }
                    }
                    let list_ptr = alloc_list(_py, &bits);
                    MoltObject::from_ptr(list_ptr).bits()
                }
                ZipState::Writer { entries, .. } => {
                    let mut bits = Vec::with_capacity(entries.len());
                    for (name, _, _) in entries {
                        let ptr = alloc_string(_py, name.as_bytes());
                        bits.push(MoltObject::from_ptr(ptr).bits());
                    }
                    let list_ptr = alloc_list(_py, &bits);
                    MoltObject::from_ptr(list_ptr).bits()
                }
            }
        })
    })
}

// ── read(handle, name) -> bytes ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_read(handle_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception(_py, "TypeError", "name must be a string");
        };

        ZIP_MAP.with(|m| {
            let map = m.borrow();
            let Some(state) = map.get(&id) else {
                return raise_exception(_py, "ValueError", "invalid zip handle");
            };
            match state {
                ZipState::Reader { data } => {
                    let cursor = Cursor::new(data);
                    let mut archive = match zip::ZipArchive::new(cursor) {
                        Ok(a) => a,
                        Err(e) => return raise_exception(_py, "ValueError", &format!("bad zip: {}", e)),
                    };
                    let mut entry = match archive.by_name(&name) {
                        Ok(e) => e,
                        Err(_) => return raise_exception(_py, "KeyError", &name),
                    };
                    let mut buf = Vec::new();
                    if let Err(e) = entry.read_to_end(&mut buf) {
                        return raise_exception(_py, "ValueError", &format!("read error: {}", e));
                    }
                    let ptr = alloc_bytes(_py, &buf);
                    MoltObject::from_ptr(ptr).bits()
                }
                _ => raise_exception(_py, "ValueError", "read requires mode='r'"),
            }
        })
    })
}
