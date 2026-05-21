//! `zipfile` module intrinsics for Molt.
//!
//! Provides handle-based ZIP archive read/write operations, delegating to
//! the `zip` crate for format compliance.
//!
//! ABI: NaN-boxed u64 in/out.

use crate::bridge::*;
use molt_runtime_core::prelude::*;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

// ── Handle-id counter ───────────────────────────────────────────────────
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

struct ZipRuntimeState {
    next_id: AtomicI64,
    archives: Mutex<HashMap<i64, ZipState>>,
}

impl ZipRuntimeState {
    fn new() -> Self {
        Self {
            next_id: AtomicI64::new(1),
            archives: Mutex::new(HashMap::new()),
        }
    }

    fn clear(&self) {
        self.archives.lock().unwrap().clear();
    }
}

unsafe extern "C" fn zip_runtime_state_init() -> *mut u8 {
    Box::into_raw(Box::new(ZipRuntimeState::new())) as *mut u8
}

unsafe extern "C" fn zip_runtime_state_clear(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        (&*(ptr as *const ZipRuntimeState)).clear();
    }
}

unsafe extern "C" fn zip_runtime_state_drop(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr as *mut ZipRuntimeState));
    }
}

fn zip_state(_py: &PyToken) -> &'static ZipRuntimeState {
    let ptr = runtime_state_get_or_init(
        b"molt-runtime-compression/zipfile/v1",
        zip_runtime_state_init,
        zip_runtime_state_clear,
        zip_runtime_state_drop,
    );
    assert!(
        !ptr.is_null(),
        "molt zipfile runtime state initialization failed"
    );
    unsafe { &*(ptr as *const ZipRuntimeState) }
}

fn next_zip_id(_py: &PyToken) -> i64 {
    zip_state(_py).next_id.fetch_add(1, Ordering::Relaxed)
}

// ── open(path, mode) -> handle ──────────────────────────────────────────
pub extern "C" fn molt_zipfile_open(path_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
            return raise_exception(_py, "TypeError", "path must be a string");
        };
        let Some(mode) = string_obj_to_owned(obj_from_bits(mode_bits)) else {
            return raise_exception(_py, "TypeError", "mode must be a string");
        };

        let id = next_zip_id(_py);
        match mode.as_str() {
            "r" => {
                let data = match std::fs::read(&path) {
                    Ok(d) => d,
                    Err(e) => return raise_exception(_py, "FileNotFoundError", &format!("{}", e)),
                };
                zip_state(_py)
                    .archives
                    .lock()
                    .unwrap()
                    .insert(id, ZipState::Reader { data });
            }
            "w" => {
                zip_state(_py).archives.lock().unwrap().insert(
                    id,
                    ZipState::Writer {
                        path,
                        entries: Vec::new(),
                    },
                );
            }
            _ => return raise_exception(_py, "ValueError", "unsupported zipfile mode"),
        }
        MoltObject::from_int(id).bits()
    })
}

// ── close(handle) -> None ───────────────────────────────────────────────
pub extern "C" fn molt_zipfile_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };
        let state = zip_state(_py).archives.lock().unwrap().remove(&id);
        match state {
            Some(ZipState::Writer { path, entries }) => {
                // Write the zip file using the `zip` crate
                let file = match std::fs::File::create(&path) {
                    Ok(f) => f,
                    Err(e) => return raise_exception(_py, "IOError", &format!("{}", e)),
                };
                let mut writer = zip::ZipWriter::new(file);
                for (name, data, method) in &entries {
                    let options = zip::write::SimpleFileOptions::default().compression_method(
                        if *method == 8 {
                            zip::CompressionMethod::Deflated
                        } else {
                            zip::CompressionMethod::Stored
                        },
                    );
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

        let mut archives = zip_state(_py).archives.lock().unwrap();
        let Some(state) = archives.get_mut(&id) else {
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
}

// ── namelist(handle) -> list[str] ───────────────────────────────────────
pub extern "C" fn molt_zipfile_namelist(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };

        let names: Result<Vec<String>, u64> = {
            let archives = zip_state(_py).archives.lock().unwrap();
            let Some(state) = archives.get(&id) else {
                return raise_exception(_py, "ValueError", "invalid zip handle");
            };
            match state {
                ZipState::Reader { data } => {
                    let cursor = Cursor::new(data);
                    let mut archive = match zip::ZipArchive::new(cursor) {
                        Ok(a) => a,
                        Err(e) => {
                            return raise_exception(_py, "ValueError", &format!("bad zip: {}", e));
                        }
                    };
                    let mut names = Vec::with_capacity(archive.len());
                    for i in 0..archive.len() {
                        if let Ok(entry) = archive.by_index(i) {
                            names.push(entry.name().to_string());
                        }
                    }
                    Ok(names)
                }
                ZipState::Writer { entries, .. } => {
                    Ok(entries.iter().map(|(name, _, _)| name.clone()).collect())
                }
            }
        };
        match names {
            Ok(names) => {
                let mut bits = Vec::with_capacity(names.len());
                for name in names {
                    let ptr = alloc_string(_py, name.as_bytes());
                    bits.push(MoltObject::from_ptr(ptr).bits());
                }
                let list_ptr = alloc_list(_py, &bits);
                MoltObject::from_ptr(list_ptr).bits()
            }
            Err(bits) => bits,
        }
    })
}

// ── read(handle, name) -> bytes ─────────────────────────────────────────
pub extern "C" fn molt_zipfile_read(handle_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception(_py, "TypeError", "invalid handle");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception(_py, "TypeError", "name must be a string");
        };

        let read_result: Result<Vec<u8>, u64> = {
            let archives = zip_state(_py).archives.lock().unwrap();
            let Some(state) = archives.get(&id) else {
                return raise_exception(_py, "ValueError", "invalid zip handle");
            };
            match state {
                ZipState::Reader { data } => {
                    let cursor = Cursor::new(data);
                    let mut archive = match zip::ZipArchive::new(cursor) {
                        Ok(a) => a,
                        Err(e) => {
                            return raise_exception(_py, "ValueError", &format!("bad zip: {}", e));
                        }
                    };
                    let mut entry = match archive.by_name(&name) {
                        Ok(e) => e,
                        Err(_) => return raise_exception(_py, "KeyError", &name),
                    };
                    let mut buf = Vec::new();
                    if let Err(e) = entry.read_to_end(&mut buf) {
                        return raise_exception(_py, "ValueError", &format!("read error: {}", e));
                    }
                    Ok(buf)
                }
                _ => Err(raise_exception(_py, "ValueError", "read requires mode='r'")),
            }
        };
        match read_result {
            Ok(buf) => {
                let ptr = alloc_bytes(_py, &buf);
                MoltObject::from_ptr(ptr).bits()
            }
            Err(bits) => bits,
        }
    })
}
