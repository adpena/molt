#![allow(dead_code, unused_imports)]
use molt_runtime_core::prelude::*;
use crate::bridge::*;
use flate2::Compression;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, ZlibEncoder};
use std::io::{Read, Write};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn require_bytes_slice(_py: &PyToken, bits: u64) -> Result<&'static [u8], u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception(
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
    Err(raise_exception(_py, "TypeError", &msg))
}

fn zlib_compression_from_level(_py: &PyToken, level_bits: u64) -> Result<Compression, u64> {
    let obj = obj_from_bits(level_bits);
    if obj.is_none() {
        return Ok(Compression::default());
    }
    let val = index_i64_from_obj(_py, level_bits, "level must be an integer");
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !(-1..=9).contains(&val) {
        return Err(raise_exception(
            _py,
            "ValueError",
            "Bad compression level",
        ));
    }
    if val == -1 {
        Ok(Compression::default())
    } else {
        Ok(Compression::new(val as u32))
    }
}

fn return_bytes(_py: &PyToken, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── Existing raw deflate/inflate ─────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_deflate_raw(data_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception(_py, "TypeError", "deflate expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("deflate expects bytes, got {}", type_name(_py, obj));
                return raise_exception(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let level_obj = obj_from_bits(level_bits);
            let compression = if level_obj.is_none() {
                Compression::default()
            } else {
                let val = index_i64_from_obj(_py, level_bits, "deflate level must be int");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !(-1..=9).contains(&val) {
                    return raise_exception(
                        _py,
                        "ValueError",
                        "deflate level must be in range -1..9",
                    );
                }
                if val == -1 {
                    Compression::default()
                } else {
                    Compression::new(val as u32)
                }
            };
            let mut encoder = DeflateEncoder::new(Vec::new(), compression);
            if encoder.write_all(slice).is_err() {
                return raise_exception(_py, "ValueError", "deflate failed");
            }
            let out = match encoder.finish() {
                Ok(val) => val,
                Err(_) => return raise_exception(_py, "ValueError", "deflate failed"),
            };
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                return raise_exception(_py, "MemoryError", "deflate out of memory");
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inflate_raw(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception(_py, "TypeError", "inflate expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("inflate expects bytes, got {}", type_name(_py, obj));
                return raise_exception(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let mut decoder = DeflateDecoder::new(slice);
            let mut out = Vec::new();
            if decoder.read_to_end(&mut out).is_err() {
                return raise_exception(_py, "ValueError", "invalid deflate data");
            }
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                return raise_exception(_py, "MemoryError", "inflate out of memory");
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

// ── zlib.compress ────────────────────────────────────────────────────────────

/// `zlib.compress(data, level=Z_DEFAULT_COMPRESSION) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compress(data_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let compression = match zlib_compression_from_level(_py, level_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let mut encoder = ZlibEncoder::new(Vec::new(), compression);
        if encoder.write_all(data).is_err() {
            return raise_exception(_py, "error", "zlib.error: compress failed");
        }
        let out = match encoder.finish() {
            Ok(v) => v,
            Err(_) => return raise_exception(_py, "error", "zlib.error: compress failed"),
        };
        return_bytes(_py, &out)
    })
}

// ── zlib.decompress ──────────────────────────────────────────────────────────

/// `zlib.decompress(data, wbits=MAX_WBITS, bufsize=DEF_BUF_SIZE) -> bytes`
///
/// wbits semantics:
///   positive (8..15)  → zlib format
///   negative (-8..-15) → raw deflate
///   >= 16 (16+8..16+15) → gzip format
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompress(data_bits: u64, wbits_bits: u64, bufsize_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        // wbits default = MAX_WBITS (15)
        let wbits = {
            let obj = obj_from_bits(wbits_bits);
            if obj.is_none() {
                15i64
            } else {
                let val = index_i64_from_obj(_py, wbits_bits, "wbits must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        // bufsize is accepted for API compatibility but we grow dynamically
        let _bufsize = {
            let obj = obj_from_bits(bufsize_bits);
            if obj.is_none() {
                16384i64
            } else {
                let val = index_i64_from_obj(_py, bufsize_bits, "bufsize must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        let mut out = Vec::new();
        let ok = if wbits >= 16 {
            // gzip format
            let mut decoder = GzDecoder::new(data);
            decoder.read_to_end(&mut out).is_ok()
        } else if wbits < 0 {
            // raw deflate (negative wbits)
            let mut decoder = DeflateDecoder::new(data);
            decoder.read_to_end(&mut out).is_ok()
        } else {
            // zlib format (positive wbits)
            let mut decoder = ZlibDecoder::new(data);
            decoder.read_to_end(&mut out).is_ok()
        };
        if !ok {
            return raise_exception(
                _py,
                "error",
                "zlib.error: Error -3 while decompressing data: incorrect header check",
            );
        }
        return_bytes(_py, &out)
    })
}

// ── zlib.crc32 ───────────────────────────────────────────────────────────────

/// Standard CRC-32 lookup table (polynomial 0xEDB88320, reflected).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

fn crc32_compute(data: &[u8], initial: u32) -> u32 {
    let mut crc = !initial;
    let mut i = 0usize;

    // Hardware CRC32 acceleration on aarch64
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("crc") {
            unsafe {
                use std::arch::aarch64::*;
                // Process 8 bytes at a time using hardware CRC32 instructions
                while i + 8 <= data.len() {
                    let val = u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
                    crc = __crc32d(crc, val);
                    i += 8;
                }
                // Process remaining bytes one at a time
                while i < data.len() {
                    crc = __crc32b(crc, data[i]);
                    i += 1;
                }
                return !crc;
            }
        }
    }

    // Hardware CRC32 acceleration on x86_64 (SSE4.2)
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sse4.2") {
            unsafe {
                use std::arch::x86_64::*;
                // Process 8 bytes at a time
                while i + 8 <= data.len() {
                    let val = u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
                    crc = _mm_crc32_u64(crc as u64, val) as u32;
                    i += 8;
                }
                // Process remaining bytes
                while i < data.len() {
                    crc = _mm_crc32_u8(crc, data[i]);
                    i += 1;
                }
                return !crc;
            }
        }
    }

    // Scalar fallback (table-based)
    for &byte in &data[i..] {
        let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = CRC32_TABLE[idx] ^ (crc >> 8);
    }
    !crc
}

/// `zlib.crc32(data, value=0) -> int`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_crc32(data_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let initial = {
            let obj = obj_from_bits(value_bits);
            if obj.is_none() {
                0u32
            } else {
                let val = index_i64_from_obj(_py, value_bits, "value must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val as u32
            }
        };
        let result = crc32_compute(data, initial);
        // CPython returns unsigned 32-bit int
        int_bits_from_i64(_py, i64::from(result))
    })
}

// ── zlib.adler32 ─────────────────────────────────────────────────────────────

const MOD_ADLER: u32 = 65521;

fn adler32_compute(data: &[u8], initial: u32) -> u32 {
    let mut a = initial & 0xFFFF;
    let mut b = (initial >> 16) & 0xFFFF;

    // Process in chunks of up to NMAX bytes (5552) to defer the expensive modulo.
    // Within each chunk, a and b cannot overflow a u32 since:
    //   a_max = 65520 + 5552*255 = 1,481,280 < 2^32
    //   b_max ≤ NMAX * a_max < 2^32
    const NMAX: usize = 5552;

    let mut idx = 0usize;
    while idx < data.len() {
        let chunk_end = (idx + NMAX).min(data.len());

        // Unrolled inner loop: process 16 bytes per iteration for better ILP
        while idx + 16 <= chunk_end {
            // Manually unrolled — helps auto-vectorization and hides latency
            a += data[idx] as u32;
            b += a;
            a += data[idx + 1] as u32;
            b += a;
            a += data[idx + 2] as u32;
            b += a;
            a += data[idx + 3] as u32;
            b += a;
            a += data[idx + 4] as u32;
            b += a;
            a += data[idx + 5] as u32;
            b += a;
            a += data[idx + 6] as u32;
            b += a;
            a += data[idx + 7] as u32;
            b += a;
            a += data[idx + 8] as u32;
            b += a;
            a += data[idx + 9] as u32;
            b += a;
            a += data[idx + 10] as u32;
            b += a;
            a += data[idx + 11] as u32;
            b += a;
            a += data[idx + 12] as u32;
            b += a;
            a += data[idx + 13] as u32;
            b += a;
            a += data[idx + 14] as u32;
            b += a;
            a += data[idx + 15] as u32;
            b += a;
            idx += 16;
        }

        // Scalar tail for remaining bytes in this chunk
        while idx < chunk_end {
            a += data[idx] as u32;
            b += a;
            idx += 1;
        }

        // Apply modulo only at chunk boundary (amortized cost)
        a %= MOD_ADLER;
        b %= MOD_ADLER;
    }

    (b << 16) | a
}

/// `zlib.adler32(data, value=1) -> int`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_adler32(data_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let initial = {
            let obj = obj_from_bits(value_bits);
            if obj.is_none() {
                1u32 // Adler-32 starts at 1 by default
            } else {
                let val = index_i64_from_obj(_py, value_bits, "value must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val as u32
            }
        };
        let result = adler32_compute(data, initial);
        int_bits_from_i64(_py, i64::from(result))
    })
}

// ── Constants ────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_max_wbits() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 15) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_def_mem_level() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 8) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_def_buf_size() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 16384) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_default_compression() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, -1) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_best_speed() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 1) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_best_compression() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 9) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_no_compression() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 0) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_filtered() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 1) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_huffman_only() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 2) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_default_strategy() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 0) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_finish() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 4) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_no_flush() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 0) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_sync_flush() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 2) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_full_flush() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { int_bits_from_i64(_py, 3) })
}

// ── Streaming compressobj ────────────────────────────────────────────────────

/// Internal format selector used by both compressobj and decompressobj.
#[derive(Clone, Copy)]
enum ZlibFormat {
    /// Raw deflate (negative wbits)
    Raw,
    /// Zlib-wrapped (positive wbits 1..=15)
    Zlib,
    /// Gzip-wrapped (wbits 16..=31)
    Gzip,
}

fn format_from_wbits(wbits: i64) -> ZlibFormat {
    if wbits < 0 {
        ZlibFormat::Raw
    } else if wbits >= 16 {
        ZlibFormat::Gzip
    } else {
        ZlibFormat::Zlib
    }
}

/// Compressor handle state.
///
/// We accumulate compressed bytes in an inner `Vec<u8>` and drain them on
/// each `compress` / `flush` call. The enum variant selects the wrapper
/// format (raw deflate, zlib, or gzip).
enum CompressorInner {
    Raw(DeflateEncoder<Vec<u8>>),
    Zlib(ZlibEncoder<Vec<u8>>),
    Gzip(flate2::write::GzEncoder<Vec<u8>>),
    Finished,
}

struct CompressorHandle {
    inner: CompressorInner,
}

fn compressor_from_bits(bits: u64) -> Option<&'static mut CompressorHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut CompressorHandle) })
}

/// `zlib.compressobj(level, method, wbits, memlevel, strategy) -> handle`
///
/// `method` and `memlevel` are accepted for API compatibility but not
/// forwarded to flate2 (it always uses deflate method 8 and a fixed
/// memory level).
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_new(
    level_bits: u64,
    _method_bits: u64,
    wbits_bits: u64,
    _memlevel_bits: u64,
    _strategy_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let compression = match zlib_compression_from_level(_py, level_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let wbits = {
            let obj = obj_from_bits(wbits_bits);
            if obj.is_none() {
                15i64 // MAX_WBITS
            } else {
                let val = index_i64_from_obj(_py, wbits_bits, "wbits must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        let inner = match format_from_wbits(wbits) {
            ZlibFormat::Raw => CompressorInner::Raw(DeflateEncoder::new(Vec::new(), compression)),
            ZlibFormat::Zlib => CompressorInner::Zlib(ZlibEncoder::new(Vec::new(), compression)),
            ZlibFormat::Gzip => {
                CompressorInner::Gzip(flate2::write::GzEncoder::new(Vec::new(), compression))
            }
        };
        let handle = Box::new(CompressorHandle { inner });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// `compressobj.compress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_compress(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = compressor_from_bits(handle_bits) else {
            return raise_exception(_py, "error", "zlib.error: invalid compressor handle");
        };
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let write_ok = match handle.inner {
            CompressorInner::Raw(ref mut enc) => enc.write_all(data).is_ok(),
            CompressorInner::Zlib(ref mut enc) => enc.write_all(data).is_ok(),
            CompressorInner::Gzip(ref mut enc) => enc.write_all(data).is_ok(),
            CompressorInner::Finished => {
                return raise_exception(
                    _py,
                    "error",
                    "zlib.error: compressor has been flushed",
                );
            }
        };
        if !write_ok {
            return raise_exception(_py, "error", "zlib.error: compress failed");
        }
        // Drain buffered output so far.
        let mut out = Vec::new();
        match handle.inner {
            CompressorInner::Raw(ref mut enc) => std::mem::swap(enc.get_mut(), &mut out),
            CompressorInner::Zlib(ref mut enc) => {
                std::mem::swap(enc.get_mut(), &mut out);
            }
            CompressorInner::Gzip(ref mut enc) => {
                std::mem::swap(enc.get_mut(), &mut out);
            }
            CompressorInner::Finished => {}
        }
        return_bytes(_py, &out)
    })
}

/// `compressobj.flush(mode=Z_FINISH) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_flush(handle_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = compressor_from_bits(handle_bits) else {
            return raise_exception(_py, "error", "zlib.error: invalid compressor handle");
        };
        let mode = {
            let obj = obj_from_bits(mode_bits);
            if obj.is_none() {
                4i64 // Z_FINISH
            } else {
                let val = index_i64_from_obj(_py, mode_bits, "mode must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        // Z_FINISH = 4 → finalize; Z_SYNC_FLUSH = 2 → sync flush; others → sync flush
        if mode == 4 {
            // Finalize: take ownership and finish the encoder.
            let inner = std::mem::replace(&mut handle.inner, CompressorInner::Finished);
            let out = match inner {
                CompressorInner::Raw(enc) => match enc.finish() {
                    Ok(v) => v,
                    Err(_) => {
                        return raise_exception(_py, "error", "zlib.error: flush failed");
                    }
                },
                CompressorInner::Zlib(enc) => match enc.finish() {
                    Ok(v) => v,
                    Err(_) => {
                        return raise_exception(_py, "error", "zlib.error: flush failed");
                    }
                },
                CompressorInner::Gzip(enc) => match enc.finish() {
                    Ok(v) => v,
                    Err(_) => {
                        return raise_exception(_py, "error", "zlib.error: flush failed");
                    }
                },
                CompressorInner::Finished => {
                    return raise_exception(
                        _py,
                        "error",
                        "zlib.error: compressor has already been flushed",
                    );
                }
            };
            return_bytes(_py, &out)
        } else {
            // Sync/full flush: flush the encoder but keep it alive.
            let flush_ok = match handle.inner {
                CompressorInner::Raw(ref mut enc) => enc.flush().is_ok(),
                CompressorInner::Zlib(ref mut enc) => enc.flush().is_ok(),
                CompressorInner::Gzip(ref mut enc) => enc.flush().is_ok(),
                CompressorInner::Finished => {
                    return raise_exception(
                        _py,
                        "error",
                        "zlib.error: compressor has been flushed",
                    );
                }
            };
            if !flush_ok {
                return raise_exception(_py, "error", "zlib.error: flush failed");
            }
            let mut out = Vec::new();
            match handle.inner {
                CompressorInner::Raw(ref mut enc) => {
                    std::mem::swap(enc.get_mut(), &mut out);
                }
                CompressorInner::Zlib(ref mut enc) => {
                    std::mem::swap(enc.get_mut(), &mut out);
                }
                CompressorInner::Gzip(ref mut enc) => {
                    std::mem::swap(enc.get_mut(), &mut out);
                }
                CompressorInner::Finished => {}
            }
            return_bytes(_py, &out)
        }
    })
}

/// Drop the compressor handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut CompressorHandle) };
        MoltObject::none().bits()
    })
}

// ── Streaming decompressobj ──────────────────────────────────────────────────

struct DecompressorHandle {
    format: ZlibFormat,
    /// Buffered input not yet consumed by the decompressor.
    leftover: Vec<u8>,
    /// Unconsumed tail after a max_length-limited decompress.
    unconsumed_tail: Vec<u8>,
    eof: bool,
}

fn decompressor_from_bits(bits: u64) -> Option<&'static mut DecompressorHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut DecompressorHandle) })
}

/// `zlib.decompressobj(wbits=MAX_WBITS) -> handle`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_new(wbits_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let wbits = {
            let obj = obj_from_bits(wbits_bits);
            if obj.is_none() {
                15i64
            } else {
                let val = index_i64_from_obj(_py, wbits_bits, "wbits must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        let handle = Box::new(DecompressorHandle {
            format: format_from_wbits(wbits),
            leftover: Vec::new(),
            unconsumed_tail: Vec::new(),
            eof: false,
        });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// Decompress `input` using the given format, returning (decompressed, bytes_consumed).
fn decompress_chunk(format: ZlibFormat, input: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let mut out = Vec::new();
    let consumed;
    match format {
        ZlibFormat::Raw => {
            let mut decoder = DeflateDecoder::new(input);
            match decoder.read_to_end(&mut out) {
                Ok(_) => {
                    consumed = input.len() - decoder.into_inner().len();
                }
                Err(e) => return Err(format!("zlib.error: {e}")),
            }
        }
        ZlibFormat::Zlib => {
            let mut decoder = ZlibDecoder::new(input);
            match decoder.read_to_end(&mut out) {
                Ok(_) => {
                    consumed = input.len() - decoder.into_inner().len();
                }
                Err(e) => return Err(format!("zlib.error: {e}")),
            }
        }
        ZlibFormat::Gzip => {
            let mut decoder = GzDecoder::new(input);
            match decoder.read_to_end(&mut out) {
                Ok(_) => {
                    consumed = input.len() - decoder.into_inner().len();
                }
                Err(e) => return Err(format!("zlib.error: {e}")),
            }
        }
    }
    Ok((out, consumed))
}

/// `decompressobj.decompress(data, max_length=0) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_decompress(
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "error", "zlib.error: invalid decompressor handle");
        };
        if handle.eof {
            return raise_exception(_py, "error", "zlib.error: inconsistent stream state");
        }
        let new_data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let max_length = {
            let obj = obj_from_bits(max_length_bits);
            if obj.is_none() {
                0i64
            } else {
                let val = index_i64_from_obj(_py, max_length_bits, "max_length must be an integer");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                val
            }
        };
        // Assemble input: leftover + unconsumed_tail + new data
        let mut input = std::mem::take(&mut handle.leftover);
        input.extend_from_slice(&std::mem::take(&mut handle.unconsumed_tail));
        input.extend_from_slice(new_data);

        let (mut out, consumed) = match decompress_chunk(handle.format, &input) {
            Ok(pair) => pair,
            Err(msg) => {
                // Partial data: store everything for next call
                handle.leftover = input;
                return raise_exception(_py, "error", &msg);
            }
        };

        let remaining = &input[consumed..];
        if consumed > 0 {
            handle.eof = true;
        }

        // Honour max_length
        if max_length > 0 && out.len() > max_length as usize {
            // We cannot truly "re-feed" partial output to flate2, so we
            // stash everything beyond max_length as unconsumed_tail and mark
            // not-EOF so the caller can call flush() to get the rest.
            handle.unconsumed_tail = remaining.to_vec();
            // Keep the excess output in leftover so flush() can return it.
            let excess = out.split_off(max_length as usize);
            handle.leftover = excess;
            handle.eof = false;
        } else {
            handle.unconsumed_tail = remaining.to_vec();
        }

        return_bytes(_py, &out)
    })
}

/// `decompressobj.flush(length=DEF_BUF_SIZE) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_flush(handle_bits: u64, _length_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "error", "zlib.error: invalid decompressor handle");
        };
        // Return any buffered leftover output from a max_length-limited decompress.
        let out = std::mem::take(&mut handle.leftover);
        handle.eof = true;
        return_bytes(_py, &out)
    })
}

/// `decompressobj.eof` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_eof(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "error", "zlib.error: invalid decompressor handle");
        };
        MoltObject::from_bool(handle.eof).bits()
    })
}

/// `decompressobj.unconsumed_tail` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_unconsumed_tail(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decompressor_from_bits(handle_bits) else {
            return raise_exception(_py, "error", "zlib.error: invalid decompressor handle");
        };
        return_bytes(_py, &handle.unconsumed_tail)
    })
}

/// Drop the decompressor handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut DecompressorHandle) };
        MoltObject::none().bits()
    })
}
