use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::io::{Read, Write};

// ── Format constants (mirror CPython lzma module) ────────────────────────────

pub(crate) const FORMAT_AUTO: i64 = 0;
pub(crate) const FORMAT_XZ: i64 = 1;
pub(crate) const FORMAT_ALONE: i64 = 2; // raw LZMA-alone (.lzma)
pub(crate) const FORMAT_RAW: i64 = 3;

pub(crate) const CHECK_NONE: i64 = 0;
pub(crate) const CHECK_CRC32: i64 = 1;
pub(crate) const CHECK_CRC64: i64 = 4;
pub(crate) const CHECK_SHA256: i64 = 10;

pub(crate) const PRESET_DEFAULT: i64 = 6;
pub(crate) const PRESET_EXTREME: i64 = 1 << 31;

// ── Constant intrinsics ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_auto() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_AUTO) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_xz() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_XZ) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_alone() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_ALONE) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_raw() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_RAW) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_none() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_NONE) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc32() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_CRC32) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc64() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_CRC64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_sha256() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_SHA256) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_default() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, PRESET_DEFAULT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_extreme() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, PRESET_EXTREME) })
}

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

fn return_bytes(_py: &PyToken<'_>, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

/// Extract integer from bits, returning a default if None.
fn opt_i64(_py: &PyToken<'_>, bits: u64, default: i64, name: &str) -> Result<i64, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(default);
    }
    let val = index_i64_from_obj(_py, bits, name);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(val)
}

/// Map CPython preset to xz2 preset level (0-9), honouring PRESET_EXTREME flag.
fn resolve_preset(preset: i64) -> u32 {
    const LIBLZMA_PRESET_EXTREME: u32 = 1u32 << 31;
    let extreme = preset & PRESET_EXTREME != 0;
    let level = (preset & 0x1f).clamp(0, 9) as u32;
    if extreme {
        level | LIBLZMA_PRESET_EXTREME
    } else {
        level
    }
}

/// Map Molt check constant to xz2 Check enum.
fn resolve_check(check: i64) -> xz2::stream::Check {
    match check {
        0 => xz2::stream::Check::None,
        1 => xz2::stream::Check::Crc32,
        4 => xz2::stream::Check::Crc64,
        10 => xz2::stream::Check::Sha256,
        _ => xz2::stream::Check::Crc64, // default
    }
}

// ── One-shot compress ─────────────────────────────────────────────────────────

/// `lzma.compress(data, format=FORMAT_XZ, check=-1, preset=None) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compress(
    data_bits: u64,
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let format = match opt_i64(_py, format_bits, FORMAT_XZ, "format") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let preset = match opt_i64(_py, preset_bits, PRESET_DEFAULT, "preset") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let check_val = match opt_i64(_py, check_bits, -1, "check") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let xz_preset = resolve_preset(preset);
        let xz_check = if check_val < 0 {
            xz2::stream::Check::Crc64
        } else {
            resolve_check(check_val)
        };
        let result: Result<Vec<u8>, std::io::Error> = match format {
            FORMAT_XZ | FORMAT_AUTO => {
                let stream = match xz2::stream::Stream::new_easy_encoder(xz_preset, xz_check) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                    }
                };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(data).and_then(|()| enc.finish())
            }
            FORMAT_ALONE | FORMAT_RAW => {
                let opts = xz2::stream::LzmaOptions::new_preset(xz_preset)
                    .unwrap_or_else(|_| xz2::stream::LzmaOptions::new_preset(6).unwrap());
                let stream = match xz2::stream::Stream::new_lzma_encoder(&opts) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                    }
                };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(data).and_then(|()| enc.finish())
            }
            _ => {
                return raise_exception::<u64>(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(out) => return_bytes(_py, &out),
            Err(e) => {
                let msg = format!("lzma compress failed: {e}");
                raise_exception::<u64>(_py, "lzma.LZMAError", &msg)
            }
        }
    })
}

// ── One-shot decompress ───────────────────────────────────────────────────────

/// `lzma.decompress(data, format=FORMAT_AUTO, memlimit=None) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompress(
    data_bits: u64,
    format_bits: u64,
    _memlimit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let format = match opt_i64(_py, format_bits, FORMAT_AUTO, "format") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let result: Result<Vec<u8>, std::io::Error> = match format {
            FORMAT_XZ => {
                let mut dec = xz2::read::XzDecoder::new(data);
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            FORMAT_ALONE | FORMAT_RAW => {
                let stream = match xz2::stream::Stream::new_lzma_decoder(u64::MAX) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                    }
                };
                let mut dec = xz2::read::XzDecoder::new_stream(data, stream);
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            FORMAT_AUTO => {
                // XZ streams start with 0xfd '7zXZ\0'
                if data.starts_with(b"\xfd7zXZ\x00") {
                    let mut dec = xz2::read::XzDecoder::new(data);
                    let mut out = Vec::new();
                    dec.read_to_end(&mut out).map(|_| out)
                } else {
                    let stream = match xz2::stream::Stream::new_lzma_decoder(u64::MAX) {
                        Ok(s) => s,
                        Err(e) => {
                            let msg = format!("lzma init error: {e}");
                            return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                        }
                    };
                    let mut dec = xz2::read::XzDecoder::new_stream(data, stream);
                    let mut out = Vec::new();
                    dec.read_to_end(&mut out).map(|_| out)
                }
            }
            _ => {
                return raise_exception::<u64>(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(out) => return_bytes(_py, &out),
            Err(_) => raise_exception::<u64>(
                _py,
                "lzma.LZMAError",
                "Input data is not a valid XZ/LZMA stream",
            ),
        }
    })
}

// ── Stateful LZMACompressor handle ───────────────────────────────────────────

struct LzmaCompressorHandle {
    format: i64,
    preset: u32,
    check: xz2::stream::Check,
    buffer: Vec<u8>,
    flushed: bool,
}

fn lzma_compressor_from_bits(bits: u64) -> Option<&'static mut LzmaCompressorHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut LzmaCompressorHandle) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_new(
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let format = match opt_i64(_py, format_bits, FORMAT_XZ, "format") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let preset = match opt_i64(_py, preset_bits, PRESET_DEFAULT, "preset") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let check_val = match opt_i64(_py, check_bits, -1, "check") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let handle = Box::new(LzmaCompressorHandle {
            format,
            preset: resolve_preset(preset),
            check: if check_val < 0 {
                xz2::stream::Check::Crc64
            } else {
                resolve_check(check_val)
            },
            buffer: Vec::new(),
            flushed: false,
        });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// `compressor.compress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_compress(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = lzma_compressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "invalid compressor handle");
        };
        if handle.flushed {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "Compressor has been flushed");
        }
        let data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        handle.buffer.extend_from_slice(data);
        return_bytes(_py, &[])
    })
}

/// `compressor.flush() -> bytes`  (finalises and returns compressed stream)
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_flush(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = lzma_compressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "invalid compressor handle");
        };
        if handle.flushed {
            return raise_exception::<u64>(
                _py,
                "lzma.LZMAError",
                "Compressor has already been flushed",
            );
        }
        handle.flushed = true;
        let input = std::mem::take(&mut handle.buffer);
        let result: Result<Vec<u8>, std::io::Error> = match handle.format {
            FORMAT_XZ | FORMAT_AUTO => {
                let stream =
                    match xz2::stream::Stream::new_easy_encoder(handle.preset, handle.check) {
                        Ok(s) => s,
                        Err(e) => {
                            let msg = format!("lzma init error: {e}");
                            return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                        }
                    };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(&input).and_then(|()| enc.finish())
            }
            FORMAT_ALONE | FORMAT_RAW => {
                let opts = xz2::stream::LzmaOptions::new_preset(handle.preset)
                    .unwrap_or_else(|_| xz2::stream::LzmaOptions::new_preset(6).unwrap());
                let stream = match xz2::stream::Stream::new_lzma_encoder(&opts) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                    }
                };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(&input).and_then(|()| enc.finish())
            }
            _ => {
                return raise_exception::<u64>(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(out) => return_bytes(_py, &out),
            Err(e) => {
                let msg = format!("lzma compress failed: {e}");
                raise_exception::<u64>(_py, "lzma.LZMAError", &msg)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut LzmaCompressorHandle) };
        MoltObject::none().bits()
    })
}

// ── Stateful LZMADecompressor handle ─────────────────────────────────────────

struct LzmaDecompressorHandle {
    format: i64,
    input_buffer: Vec<u8>,
    eof: bool,
    unused_data: Vec<u8>,
    needs_input: bool,
}

fn lzma_decompressor_from_bits(bits: u64) -> Option<&'static mut LzmaDecompressorHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut LzmaDecompressorHandle) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_new(format_bits: u64, _memlimit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format = match opt_i64(_py, format_bits, FORMAT_AUTO, "format") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let handle = Box::new(LzmaDecompressorHandle {
            format,
            input_buffer: Vec::new(),
            eof: false,
            unused_data: Vec::new(),
            needs_input: true,
        });
        let ptr = Box::into_raw(handle) as *mut u8;
        bits_from_ptr(ptr)
    })
}

/// `decompressor.decompress(data, max_length=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_decompress(
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = lzma_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "invalid decompressor handle");
        };
        if handle.eof {
            return raise_exception::<u64>(_py, "EOFError", "End of stream already reached");
        }
        let new_data = match require_bytes_slice(_py, data_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let max_len = match opt_i64(_py, max_length_bits, -1, "max_length") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        handle.input_buffer.extend_from_slice(new_data);
        let input = std::mem::take(&mut handle.input_buffer);
        let format = handle.format;
        let effective_format = if format == FORMAT_AUTO {
            if input.starts_with(b"\xfd7zXZ\x00") {
                FORMAT_XZ
            } else {
                FORMAT_ALONE
            }
        } else {
            format
        };
        let result: Result<Vec<u8>, std::io::Error> = match effective_format {
            FORMAT_XZ => {
                let mut dec = xz2::read::XzDecoder::new(input.as_slice());
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            FORMAT_ALONE | FORMAT_RAW => {
                let stream = match xz2::stream::Stream::new_lzma_decoder(u64::MAX) {
                    Ok(s) => s,
                    Err(e) => {
                        handle.input_buffer = input;
                        let msg = format!("lzma init error: {e}");
                        return raise_exception::<u64>(_py, "lzma.LZMAError", &msg);
                    }
                };
                let mut dec = xz2::read::XzDecoder::new_stream(input.as_slice(), stream);
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            _ => {
                return raise_exception::<u64>(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(mut out) => {
                handle.eof = true;
                handle.needs_input = false;
                handle.unused_data = Vec::new();
                if max_len >= 0 && out.len() > max_len as usize {
                    out.truncate(max_len as usize);
                }
                return_bytes(_py, &out)
            }
            Err(_) => {
                handle.input_buffer = input;
                handle.needs_input = true;
                return_bytes(_py, &[])
            }
        }
    })
}

/// `decompressor.eof` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_eof(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = lzma_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "invalid decompressor handle");
        };
        MoltObject::from_bool(handle.eof).bits()
    })
}

/// `decompressor.needs_input` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_needs_input(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = lzma_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "invalid decompressor handle");
        };
        MoltObject::from_bool(handle.needs_input).bits()
    })
}

/// `decompressor.unused_data` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_unused_data(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = lzma_decompressor_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "lzma.LZMAError", "invalid decompressor handle");
        };
        let tail = handle.unused_data.clone();
        return_bytes(_py, &tail)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut LzmaDecompressorHandle) };
        MoltObject::none().bits()
    })
}
