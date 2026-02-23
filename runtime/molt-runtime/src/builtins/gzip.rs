use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};

// ── Helper ───────────────────────────────────────────────────────────────────

fn require_bytes_slice(_py: &PyToken<'_>, bits: u64) -> Result<&'static [u8], u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "a bytes-like object is required",
        ));
    };
    unsafe {
        if let Some(slice) = bytes_like_slice(ptr) {
            return Ok(slice);
        }
    }
    let tname = type_name(_py, obj);
    let msg = format!("a bytes-like object is required, not '{tname}'");
    Err(raise_exception::<u64>(_py, "TypeError", &msg))
}

fn compression_from_level(_py: &PyToken<'_>, level_bits: u64) -> Result<Compression, u64> {
    let obj = obj_from_bits(level_bits);
    if obj.is_none() {
        return Ok(Compression::default());
    }
    let val = index_i64_from_obj(_py, level_bits, "compresslevel must be an integer");
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !(0..=9).contains(&val) {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "compresslevel must be between 0 and 9",
        ));
    }
    Ok(Compression::new(val as u32))
}

fn return_bytes(_py: &PyToken<'_>, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── One-shot compress / decompress ───────────────────────────────────────────

/// `gzip.compress(data, compresslevel=9, *, mtime=None) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_compress(
    data_bits: u64,
    compresslevel_bits: u64,
    mtime_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let compression = match compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        // Honour explicit mtime if provided (gzip header field).
        let mtime: Option<u32> = {
            let obj = obj_from_bits(mtime_bits);
            if obj.is_none() {
                None
            } else {
                let val = index_i64_from_obj(_py, mtime_bits, "mtime must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                Some(val as u32)
            }
        };
        let mut builder = flate2::GzBuilder::new();
        if let Some(t) = mtime {
            builder = builder.mtime(t);
        }
        let mut encoder = builder.write(Vec::new(), compression);
        if encoder.write_all(data).is_err() {
            return raise_exception::<u64>(_py, "OSError", "gzip compress failed");
        }
        let out = match encoder.finish() {
            Ok(v) => v,
            Err(_) => return raise_exception::<u64>(_py, "OSError", "gzip compress failed"),
        };
        return_bytes(_py, &out)
    })
}

/// `gzip.decompress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_decompress(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let mut decoder = GzDecoder::new(data);
        let mut out = Vec::new();
        if decoder.read_to_end(&mut out).is_err() {
            return raise_exception::<u64>(_py, "OSError", "Not a gzipped file (b'\\x1f\\x8b')");
        }
        return_bytes(_py, &out)
    })
}

// ── Stateful GzipFile handle ─────────────────────────────────────────────────

enum GzipHandleInner {
    Writing(GzEncoder<File>),
    Reading(BufReader<GzDecoder<File>>),
    Closed,
}

struct GzipFileHandle {
    inner: GzipHandleInner,
}

fn gzip_handle_from_bits(bits: u64) -> Option<&'static mut GzipFileHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut GzipFileHandle) })
}

/// `gzip.open(filename, mode, compresslevel) -> handle`
///
/// Supported modes: "rb" / "r" (read), "wb" / "w" (write), "ab" / "a" (append-write).
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_open(
    filename_bits: u64,
    mode_bits: u64,
    compresslevel_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(filename_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "filename must be str");
        };
        let mode_obj = obj_from_bits(mode_bits);
        let mode = string_obj_to_owned(mode_obj).unwrap_or_else(|| "rb".to_string());
        let compression = match compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let inner = match mode.trim_matches('b') {
            "r" => {
                let file = match File::open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("gzip.open: {e}");
                        return raise_exception::<u64>(_py, "OSError", &msg);
                    }
                };
                GzipHandleInner::Reading(BufReader::new(GzDecoder::new(file)))
            }
            "w" => {
                let file = match OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&name)
                {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("gzip.open: {e}");
                        return raise_exception::<u64>(_py, "OSError", &msg);
                    }
                };
                GzipHandleInner::Writing(GzEncoder::new(file, compression))
            }
            "a" => {
                let file = match OpenOptions::new().create(true).append(true).open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("gzip.open: {e}");
                        return raise_exception::<u64>(_py, "OSError", &msg);
                    }
                };
                GzipHandleInner::Writing(GzEncoder::new(file, compression))
            }
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "Invalid mode ('r', 'rb', 'w', 'wb', 'a', 'ab' supported)",
                );
            }
        };
        let handle = Box::new(GzipFileHandle { inner });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// `handle.read(size=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_read(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = gzip_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid gzip handle");
        };
        let GzipHandleInner::Reading(ref mut reader) = handle.inner else {
            return raise_exception::<u64>(_py, "OSError", "file not open for reading");
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
            return raise_exception::<u64>(_py, "OSError", "gzip read failed");
        }
        return_bytes(_py, &out)
    })
}

/// `handle.write(data) -> int`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = gzip_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid gzip handle");
        };
        let GzipHandleInner::Writing(ref mut encoder) = handle.inner else {
            return raise_exception::<u64>(_py, "OSError", "file not open for writing");
        };
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match encoder.write_all(data) {
            Ok(()) => int_bits_from_i64(_py, data.len() as i64),
            Err(e) => {
                let msg = format!("gzip write failed: {e}");
                raise_exception::<u64>(_py, "OSError", &msg)
            }
        }
    })
}

/// `handle.close() -> None`  (finishes the gzip stream)
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = gzip_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid gzip handle");
        };
        let inner = std::mem::replace(&mut handle.inner, GzipHandleInner::Closed);
        match inner {
            GzipHandleInner::Writing(enc) => {
                if enc.finish().is_err() {
                    return raise_exception::<u64>(_py, "OSError", "gzip close/finish failed");
                }
            }
            GzipHandleInner::Reading(_) | GzipHandleInner::Closed => {}
        }
        MoltObject::none().bits()
    })
}

/// Free the handle (without finishing the stream if not already closed).
#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut GzipFileHandle) };
        MoltObject::none().bits()
    })
}
