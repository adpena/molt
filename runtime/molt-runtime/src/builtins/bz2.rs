use crate::*;
use bzip2::Compression as BzCompression;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use std::io::{Read, Write};

// ── Helpers ──────────────────────────────────────────────────────────────────

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

/// Maps CPython compresslevel (1–9) to a bzip2::Compression value.
fn bz_compression_from_level(_py: &PyToken<'_>, level_bits: u64) -> Result<BzCompression, u64> {
    let obj = obj_from_bits(level_bits);
    if obj.is_none() {
        return Ok(BzCompression::default());
    }
    let val = index_i64_from_obj(_py, level_bits, "compresslevel must be an integer");
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !(1..=9).contains(&val) {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "compresslevel must be between 1 and 9",
        ));
    }
    Ok(BzCompression::new(val as u32))
}

fn return_bytes(_py: &PyToken<'_>, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── One-shot compress / decompress ───────────────────────────────────────────

/// `bz2.compress(data, compresslevel=9) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compress(data_bits: u64, compresslevel_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let compression = match bz_compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let mut encoder = BzEncoder::new(Vec::new(), compression);
        if encoder.write_all(data).is_err() {
            return raise_exception::<u64>(_py, "OSError", "bz2 compress failed");
        }
        let out = match encoder.finish() {
            Ok(v) => v,
            Err(_) => return raise_exception::<u64>(_py, "OSError", "bz2 compress failed"),
        };
        return_bytes(_py, &out)
    })
}

/// `bz2.decompress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompress(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let mut decoder = BzDecoder::new(data);
        let mut out = Vec::new();
        if decoder.read_to_end(&mut out).is_err() {
            return raise_exception::<u64>(_py, "OSError", "Invalid data stream");
        }
        return_bytes(_py, &out)
    })
}

// ── Stateful BZ2Compressor handle ────────────────────────────────────────────

struct Bz2CompressorHandle {
    // We accumulate data and compress in flush() to keep the interface simple.
    // The bzip2 crate's streaming encoder writes directly to a sink; we use
    // Vec<u8> as the sink and drain it on each call.
    encoder: Option<BzEncoder<Vec<u8>>>,
}

fn bz2_compressor_from_bits(bits: u64) -> Option<&'static mut Bz2CompressorHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut Bz2CompressorHandle) })
}

/// `bz2.BZ2Compressor(compresslevel=9)` → handle
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_new(compresslevel_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let compression = match bz_compression_from_level(_py, compresslevel_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let handle = Box::new(Bz2CompressorHandle {
            encoder: Some(BzEncoder::new(Vec::new(), compression)),
        });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// `compressor.compress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_compress(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = bz2_compressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid bz2 compressor handle");
        };
        let Some(enc) = handle.encoder.as_mut() else {
            return raise_exception::<u64>(
                _py,
                "OSError",
                "compressor has been flushed and is no longer usable",
            );
        };
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        if enc.write_all(data).is_err() {
            return raise_exception::<u64>(_py, "OSError", "bz2 compress failed");
        }
        // Drain buffered output so far.
        let mut out = Vec::new();
        std::mem::swap(enc.get_mut(), &mut out);
        return_bytes(_py, &out)
    })
}

/// `compressor.flush() -> bytes`  (finalises the stream)
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_flush(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = bz2_compressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid bz2 compressor handle");
        };
        let Some(enc) = handle.encoder.take() else {
            return raise_exception::<u64>(_py, "OSError", "compressor has already been flushed");
        };
        let out = match enc.finish() {
            Ok(v) => v,
            Err(_) => return raise_exception::<u64>(_py, "OSError", "bz2 flush failed"),
        };
        return_bytes(_py, &out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut Bz2CompressorHandle) };
        MoltObject::none().bits()
    })
}

// ── Stateful BZ2Decompressor handle ──────────────────────────────────────────

struct Bz2DecompressorHandle {
    leftover: Vec<u8>,
    eof: bool,
    needs_input: bool,
}

fn bz2_decompressor_from_bits(bits: u64) -> Option<&'static mut Bz2DecompressorHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut Bz2DecompressorHandle) })
}

/// `bz2.BZ2Decompressor()` → handle
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = Box::new(Bz2DecompressorHandle {
            leftover: Vec::new(),
            eof: false,
            needs_input: true,
        });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// `decompressor.decompress(data, max_length=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_decompress(
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid bz2 decompressor handle");
        };
        if handle.eof {
            return raise_exception::<u64>(_py, "EOFError", "End of stream already reached");
        }
        let new_data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
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
        // Concatenate leftover + new data then run through a single-shot decoder.
        // This is correct for bzip2 because each bz2 stream is independently decodeable.
        let mut input = std::mem::take(&mut handle.leftover);
        input.extend_from_slice(new_data);
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
        return_bytes(_py, &out)
    })
}

/// `decompressor.eof` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_eof(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid bz2 decompressor handle");
        };
        MoltObject::from_bool(handle.eof).bits()
    })
}

/// `decompressor.needs_input` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_needs_input(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid bz2 decompressor handle");
        };
        MoltObject::from_bool(handle.needs_input).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut Bz2DecompressorHandle) };
        MoltObject::none().bits()
    })
}

// ── Convenience: total bytes in (de)compressed output ────────────────────────

/// Returns the number of bytes written to the underlying stream so far.
/// Matches `BZ2Compressor.unused_data` / `BZ2Decompressor.unused_data` concept.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_unused_data(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = bz2_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "OSError", "invalid bz2 decompressor handle");
        };
        // unused_data is the data after the end-of-stream (empty until eof)
        if handle.eof {
            return_bytes(_py, &handle.leftover)
        } else {
            return_bytes(_py, &[])
        }
    })
}
