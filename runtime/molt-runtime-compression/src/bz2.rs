use crate::bridge::*;
use bzip2::Compression as BzCompression;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use molt_runtime_core::native_custody::ByteInput;
use molt_runtime_core::prelude::*;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::sync::{Arc, Mutex};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Maps CPython compresslevel (1–9) to a bzip2::Compression value.
fn bz_compression_from_level(_py: &PyToken, level_bits: u64) -> Result<BzCompression, u64> {
    let obj = obj_from_bits(level_bits);
    if obj.is_none() {
        return Ok(BzCompression::default());
    }
    let val = index_i64_from_obj(_py, level_bits, "compresslevel must be an integer");
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !(1..=9).contains(&val) {
        return Err(raise_exception(
            _py,
            "ValueError",
            "compresslevel must be between 1 and 9",
        ));
    }
    Ok(BzCompression::new(val as u32))
}

fn return_bytes(_py: &PyToken, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── One-shot compress / decompress ───────────────────────────────────────────

/// `bz2.compress(data, compresslevel=9) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compress(data_bits: u64, compresslevel_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let compression = match bz_compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &data_export,
            data_bits,
            "compress",
            "argument",
        ) {
            return MoltObject::none().bits();
        }
        let mut encoder = BzEncoder::new(Vec::new(), compression);
        if encoder
            .write_all(unsafe { data_export.bytes(_py) })
            .is_err()
        {
            return raise_exception(_py, "OSError", "bz2 compress failed");
        }
        let out = match encoder.finish() {
            Ok(v) => v,
            Err(_) => return raise_exception(_py, "OSError", "bz2 compress failed"),
        };
        return_bytes(_py, &out)
    })
}

/// `bz2.decompress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompress(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &data_export,
            data_bits,
            "decompress",
            "argument 'data'",
        ) {
            return MoltObject::none().bits();
        }
        let mut decoder = BzDecoder::new(unsafe { data_export.bytes(_py) });
        let mut out = Vec::new();
        if decoder.read_to_end(&mut out).is_err() {
            return raise_exception(_py, "OSError", "Invalid data stream");
        }
        return_bytes(_py, &out)
    })
}

// ── Stateful BZ2Compressor handle ────────────────────────────────────────────

pub struct Bz2CompressorHandle {
    // We accumulate data and compress in flush() to keep the interface simple.
    // The bzip2 crate's streaming encoder writes directly to a sink; we use
    // Vec<u8> as the sink and drain it on each call.
    encoder: Option<BzEncoder<Vec<u8>>>,
}

fn bz2_compressor_from_bits(bits: u64) -> Option<Arc<Mutex<Bz2CompressorHandle>>> {
    unsafe {
        molt_runtime_core::native_custody::clone_native_arc(
            bits,
            __molt_compression_bz2_compressor_owner_clone,
        )
    }
}

/// `bz2.BZ2Compressor(compresslevel=9)` → handle
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_new(compresslevel_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let compression = match bz_compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let handle = Arc::new(Mutex::new(Bz2CompressorHandle {
            encoder: Some(BzEncoder::new(Vec::new(), compression)),
        }));
        unsafe {
            molt_runtime_core::native_custody::transfer_native_arc(
                handle,
                __molt_compression_bz2_compressor_owner_new,
            )
        }
    })
}

/// `compressor.compress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_compress(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_compressor_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 compressor handle");
        };
        let data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &data_export,
            data_bits,
            "compress",
            "argument",
        ) {
            return MoltObject::none().bits();
        }

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(enc) = handle.encoder.as_mut() else {
            drop(handle);
            return raise_exception(
                _py,
                "OSError",
                "compressor has been flushed and is no longer usable",
            );
        };
        if enc.write_all(unsafe { data_export.bytes(_py) }).is_err() {
            drop(handle);
            return raise_exception(_py, "OSError", "bz2 compress failed");
        }
        // Drain buffered output so far.
        let mut out = Vec::new();
        std::mem::swap(enc.get_mut(), &mut out);
        drop(handle);
        return_bytes(_py, &out)
    })
}

/// `compressor.flush() -> bytes`  (finalises the stream)
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_flush(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_compressor_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 compressor handle");
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(enc) = handle.encoder.take() else {
            drop(handle);
            return raise_exception(_py, "OSError", "compressor has already been flushed");
        };
        let out = match enc.finish() {
            Ok(v) => v,
            Err(_) => {
                drop(handle);
                return raise_exception(_py, "OSError", "bz2 flush failed");
            }
        };
        drop(handle);
        return_bytes(_py, &out)
    })
}

// ── Stateful BZ2Decompressor handle ──────────────────────────────────────────

pub struct Bz2DecompressorHandle {
    leftover: Vec<u8>,
    eof: bool,
    needs_input: bool,
}

fn bz2_decompressor_from_bits(bits: u64) -> Option<Arc<Mutex<Bz2DecompressorHandle>>> {
    unsafe {
        molt_runtime_core::native_custody::clone_native_arc(
            bits,
            __molt_compression_bz2_decompressor_owner_clone,
        )
    }
}

/// `bz2.BZ2Decompressor()` → handle
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_new() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let handle = Arc::new(Mutex::new(Bz2DecompressorHandle {
            leftover: Vec::new(),
            eof: false,
            needs_input: true,
        }));
        unsafe {
            molt_runtime_core::native_custody::transfer_native_arc(
                handle,
                __molt_compression_bz2_decompressor_owner_new,
            )
        }
    })
}

/// `decompressor.decompress(data, max_length=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_decompress(
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 decompressor handle");
        };
        let new_data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &new_data_export,
            data_bits,
            "decompress",
            "argument 'data'",
        ) {
            return MoltObject::none().bits();
        }
        let max_len = {
            let obj = obj_from_bits(max_length_bits);
            if obj.is_none() {
                -1i64
            } else {
                let val = index_i64_from_obj(_py, max_length_bits, "max_length must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if handle.eof {
            drop(handle);
            return raise_exception(_py, "EOFError", "End of stream already reached");
        }
        // Concatenate leftover + new data then run through a single-shot decoder.
        // This is correct for bzip2 because each bz2 stream is independently decodeable.
        let mut input = std::mem::take(&mut handle.leftover);
        input.extend_from_slice(unsafe { new_data_export.bytes(_py) });
        let mut decoder = BzDecoder::new(input.as_slice());
        let mut out = Vec::new();
        match decoder.read_to_end(&mut out) {
            Ok(_) => {
                handle.eof = true;
                handle.needs_input = false;
            }
            Err(_) => {
                // Partial data — store for next call
                handle.leftover = input;
                handle.needs_input = true;
                // Return whatever we managed to decode (may be empty)
            }
        }
        // Honour max_length
        if max_len >= 0 && out.len() > max_len as usize {
            out.truncate(max_len as usize);
        }
        drop(handle);
        return_bytes(_py, &out)
    })
}

/// `decompressor.eof` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_eof(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 decompressor handle");
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        MoltObject::from_bool(handle.eof).bits()
    })
}

/// `decompressor.needs_input` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_needs_input(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 decompressor handle");
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        MoltObject::from_bool(handle.needs_input).bits()
    })
}

// ── Convenience: total bytes in (de)compressed output ────────────────────────

/// Returns the number of bytes written to the underlying stream so far.
/// Matches `BZ2Compressor.unused_data` / `BZ2Decompressor.unused_data` concept.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_unused_data(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 decompressor handle");
        };

        // unused_data is the data after the end-of-stream (empty until eof)
        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if handle.eof {
            {
                let bytes = handle.leftover.clone();
                drop(handle);
                return_bytes(_py, &bytes)
            }
        } else {
            drop(handle);
            return_bytes(_py, &[])
        }
    })
}

// ── Stateful BZ2File handle ──────────────────────────────────────────────────

enum Bz2FileInner {
    Writing(BzEncoder<File>),
    Reading(BufReader<BzDecoder<File>>),
    Closed,
}

pub struct Bz2FileHandle {
    inner: Bz2FileInner,
}

fn bz2_file_handle_from_bits(bits: u64) -> Option<Arc<Mutex<Bz2FileHandle>>> {
    unsafe {
        molt_runtime_core::native_custody::clone_native_arc(
            bits,
            __molt_compression_bz2_file_owner_clone,
        )
    }
}

/// `bz2.open(filename, mode, compresslevel) -> handle`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_open(
    filename_bits: u64,
    mode_bits: u64,
    compresslevel_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(filename_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            return raise_exception(_py, "TypeError", "filename must be str");
        };
        let mode_obj = obj_from_bits(mode_bits);
        let mode = string_obj_to_owned(mode_obj).unwrap_or_else(|| "rb".to_string());
        let compression = match bz_compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let inner = match mode.trim_matches('b') {
            "r" => {
                let file = match File::open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("bz2.open: {e}");
                        return raise_exception(_py, "OSError", &msg);
                    }
                };
                Bz2FileInner::Reading(BufReader::new(BzDecoder::new(file)))
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
                        let msg = format!("bz2.open: {e}");
                        return raise_exception(_py, "OSError", &msg);
                    }
                };
                Bz2FileInner::Writing(BzEncoder::new(file, compression))
            }
            "a" => {
                let file = match OpenOptions::new().create(true).append(true).open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("bz2.open: {e}");
                        return raise_exception(_py, "OSError", &msg);
                    }
                };
                Bz2FileInner::Writing(BzEncoder::new(file, compression))
            }
            _ => {
                return raise_exception(
                    _py,
                    "ValueError",
                    "Invalid mode ('r', 'rb', 'w', 'wb', 'a', 'ab' supported)",
                );
            }
        };
        let handle = Arc::new(Mutex::new(Bz2FileHandle { inner }));
        unsafe {
            molt_runtime_core::native_custody::transfer_native_arc(
                handle,
                __molt_compression_bz2_file_owner_new,
            )
        }
    })
}

/// `handle.read(size=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_file_handle_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 file handle");
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
        let Bz2FileInner::Reading(ref mut reader) = handle.inner else {
            drop(handle);
            return raise_exception(_py, "OSError", "file not open for reading");
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
            return raise_exception(_py, "OSError", "bz2 read failed");
        }
        drop(handle);
        return_bytes(_py, &out)
    })
}

/// `handle.write(data) -> int`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_file_handle_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 file handle");
        };
        let data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &data_export,
            data_bits,
            "compress",
            "argument",
        ) {
            return MoltObject::none().bits();
        }

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Bz2FileInner::Writing(ref mut encoder) = handle.inner else {
            drop(handle);
            return raise_exception(_py, "OSError", "file not open for writing");
        };
        let written = encoder.write_all(unsafe { data_export.bytes(_py) });
        drop(handle);
        match written {
            Ok(()) => int_bits_from_i64(_py, (unsafe { data_export.bytes(_py) }).len() as i64),
            Err(e) => {
                let msg = format!("bz2 write failed: {e}");
                raise_exception(_py, "OSError", &msg)
            }
        }
    })
}

/// `handle.close() -> None`  (finishes the bz2 stream)
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle_owner) = bz2_file_handle_from_bits(handle_bits) else {
            return raise_exception(_py, "OSError", "invalid bz2 file handle");
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let inner = std::mem::replace(&mut handle.inner, Bz2FileInner::Closed);
        match inner {
            Bz2FileInner::Writing(enc) => {
                if enc.finish().is_err() {
                    drop(handle);
                    return raise_exception(_py, "OSError", "bz2 close/finish failed");
                }
            }
            Bz2FileInner::Reading(_) | Bz2FileInner::Closed => {}
        }
        MoltObject::none().bits()
    })
}
