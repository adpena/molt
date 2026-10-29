use crate::bridge::*;
use molt_runtime_core::prelude::*;
use std::fs::File;
use std::io::{BufReader, Read};
use std::sync::{Arc, Mutex};

// ── Helper ───────────────────────────────────────────────────────────────────

fn return_bytes(_py: &PyToken, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception(_py, BuiltinExc::MemoryError, "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── Stateful GzipFile handle ─────────────────────────────────────────────────

enum GzipHandleInner {
    Reading(crate::zlib::GzipReader<BufReader<File>>),
    Closed,
}

pub struct GzipFileHandle {
    inner: GzipHandleInner,
}

fn gzip_handle_from_bits(bits: u64) -> Option<Arc<Mutex<GzipFileHandle>>> {
    unsafe { molt_runtime_core::clone_native_arc(bits, __molt_compression_gzip_file_owner_clone) }
}

/// `gzip.open(filename, mode, compresslevel) -> handle`
///
/// This is the retained filename READ owner. Writing uses BufferedWriter and zlib.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_open(
    filename_bits: u64,
    mode_bits: u64,
    _compresslevel_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(filename_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            return raise_exception(_py, BuiltinExc::TypeError, "filename must be str");
        };
        let mode_obj = obj_from_bits(mode_bits);
        let mode = string_obj_to_owned(mode_obj).unwrap_or_else(|| "rb".to_string());
        let inner = match mode.trim_matches('b') {
            "r" => {
                let file = match File::open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("gzip.open: {e}");
                        return raise_exception(_py, BuiltinExc::OSError, &msg);
                    }
                };
                let reader = match crate::zlib::GzipReader::new(BufReader::new(file)) {
                    Ok(reader) => reader,
                    Err(libz_rs_sys::Z_MEM_ERROR) => {
                        return raise_exception(
                            _py,
                            BuiltinExc::MemoryError,
                            "gzip reader out of memory",
                        );
                    }
                    Err(_) => {
                        return raise_exception(
                            _py,
                            BuiltinExc::OSError,
                            "gzip reader initialization failed",
                        );
                    }
                };
                GzipHandleInner::Reading(reader)
            }
            _ => {
                return raise_exception(
                    _py,
                    BuiltinExc::ValueError,
                    "native gzip reader requires 'r' or 'rb'",
                );
            }
        };
        let handle = Arc::new(Mutex::new(GzipFileHandle { inner }));
        unsafe {
            molt_runtime_core::transfer_native_arc(handle, __molt_compression_gzip_file_owner_new)
        }
    })
}

/// `handle.read(size=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = gzip_handle_from_bits(handle_bits) else {
            return raise_exception(_py, BuiltinExc::OSError, "invalid gzip handle");
        };

        let size = {
            let obj = obj_from_bits(size_bits);
            if obj.is_none() {
                -1i64
            } else {
                let val = index_i64_from_obj(_py, size_bits, "size must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let GzipHandleInner::Reading(ref mut reader) = handle.inner else {
            drop(handle);
            return raise_exception(_py, BuiltinExc::OSError, "file not open for reading");
        };

        let mut out = Vec::new();
        let ok = if size < 0 {
            reader.read_to_end(&mut out).is_ok()
        } else {
            out.resize(size as usize, 0);
            match reader.read(&mut out) {
                Ok(n) => {
                    out.truncate(n);
                    true
                }
                Err(_) => false,
            }
        };
        if !ok {
            drop(handle);
            return raise_exception(_py, BuiltinExc::OSError, "gzip read failed");
        }
        drop(handle);
        return_bytes(_py, &out)
    })
}

/// `reader.close() -> None`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = gzip_handle_from_bits(handle_bits) else {
            return raise_exception(_py, BuiltinExc::OSError, "invalid gzip handle");
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        handle.inner = GzipHandleInner::Closed;
        MoltObject::none().bits()
    })
}
