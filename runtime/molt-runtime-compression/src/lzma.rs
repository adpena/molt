use crate::bridge::*;
use molt_runtime_core::native_custody::ByteInput;
use molt_runtime_core::prelude::*;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::sync::{Arc, Mutex};

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
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, FORMAT_AUTO))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_xz() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, FORMAT_XZ))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_alone() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, FORMAT_ALONE))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_raw() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, FORMAT_RAW))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_none() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, CHECK_NONE))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc32() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, CHECK_CRC32))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc64() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, CHECK_CRC64))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_sha256() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, CHECK_SHA256))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_default() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, PRESET_DEFAULT))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_extreme() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, PRESET_EXTREME))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn return_bytes(_py: &PyToken, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

/// Extract integer from bits, returning a default if None.
fn opt_i64(_py: &PyToken, bits: u64, default: i64, name: &str) -> Result<i64, u64> {
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
    error_class: u64,
    data_bits: u64,
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();

        let format = match opt_i64(_py, format_bits, FORMAT_XZ, "format") {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let check_val = match opt_i64(_py, check_bits, -1, "check") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let preset = match opt_i64(_py, preset_bits, PRESET_DEFAULT, "preset") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let xz_preset = resolve_preset(preset);
        let xz_check = if check_val < 0 {
            xz2::stream::Check::Crc64
        } else {
            resolve_check(check_val)
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
        let result: Result<Vec<u8>, std::io::Error> = match format {
            FORMAT_XZ | FORMAT_AUTO => {
                let stream = match xz2::stream::Stream::new_easy_encoder(xz_preset, xz_check) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                    }
                };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(unsafe { data_export.bytes(_py) })
                    .and_then(|()| enc.finish())
            }
            FORMAT_ALONE | FORMAT_RAW => {
                let opts = xz2::stream::LzmaOptions::new_preset(xz_preset)
                    .unwrap_or_else(|_| xz2::stream::LzmaOptions::new_preset(6).unwrap());
                let stream = match xz2::stream::Stream::new_lzma_encoder(&opts) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                    }
                };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(unsafe { data_export.bytes(_py) })
                    .and_then(|()| enc.finish())
            }
            _ => {
                return raise_exception(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(out) => return_bytes(_py, &out),
            Err(e) => {
                let msg = format!("lzma compress failed: {e}");
                molt_runtime_core::rt_raise_class(_py, error_class, &msg)
            }
        }
    })
}

// ── One-shot decompress ───────────────────────────────────────────────────────

/// `lzma.decompress(data, format=FORMAT_AUTO, memlimit=None) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompress(
    error_class: u64,
    data_bits: u64,
    format_bits: u64,
    _memlimit_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();

        let format = match opt_i64(_py, format_bits, FORMAT_AUTO, "format") {
            Ok(v) => v,
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
            "decompress",
            "argument 'data'",
        ) {
            return MoltObject::none().bits();
        }
        let result: Result<Vec<u8>, std::io::Error> = match format {
            FORMAT_XZ => {
                let mut dec = xz2::read::XzDecoder::new(unsafe { data_export.bytes(_py) });
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            FORMAT_ALONE | FORMAT_RAW => {
                let stream = match xz2::stream::Stream::new_lzma_decoder(u64::MAX) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("lzma init error: {e}");
                        return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                    }
                };
                let mut dec =
                    xz2::read::XzDecoder::new_stream(unsafe { data_export.bytes(_py) }, stream);
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            FORMAT_AUTO => {
                // XZ streams start with 0xfd '7zXZ\0'
                if (unsafe { data_export.bytes(_py) }).starts_with(b"\xfd7zXZ\x00") {
                    let mut dec = xz2::read::XzDecoder::new(unsafe { data_export.bytes(_py) });
                    let mut out = Vec::new();
                    dec.read_to_end(&mut out).map(|_| out)
                } else {
                    let stream = match xz2::stream::Stream::new_lzma_decoder(u64::MAX) {
                        Ok(s) => s,
                        Err(e) => {
                            let msg = format!("lzma init error: {e}");
                            return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                        }
                    };
                    let mut dec =
                        xz2::read::XzDecoder::new_stream(unsafe { data_export.bytes(_py) }, stream);
                    let mut out = Vec::new();
                    dec.read_to_end(&mut out).map(|_| out)
                }
            }
            _ => {
                return raise_exception(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(out) => return_bytes(_py, &out),
            Err(_) => molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "Input data is not a valid XZ/LZMA stream",
            ),
        }
    })
}

// ── Stateful LZMACompressor handle ───────────────────────────────────────────

pub struct LzmaCompressorHandle {
    format: i64,
    preset: u32,
    check: xz2::stream::Check,
    buffer: Vec<u8>,
    flushed: bool,
}

fn lzma_compressor_from_bits(bits: u64) -> Option<Arc<Mutex<LzmaCompressorHandle>>> {
    unsafe {
        molt_runtime_core::native_custody::clone_native_arc(
            bits,
            __molt_compression_lzma_compressor_owner_clone,
        )
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_new(
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
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
        let handle = Arc::new(Mutex::new(LzmaCompressorHandle {
            format,
            preset: resolve_preset(preset),
            check: if check_val < 0 {
                xz2::stream::Check::Crc64
            } else {
                resolve_check(check_val)
            },
            buffer: Vec::new(),
            flushed: false,
        }));
        unsafe {
            molt_runtime_core::native_custody::transfer_native_arc(
                handle,
                __molt_compression_lzma_compressor_owner_new,
            )
        }
    })
}

/// `compressor.compress(data) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_compress(
    error_class: u64,
    handle_bits: u64,
    data_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_compressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "invalid compressor handle",
            );
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
        if handle.flushed {
            drop(handle);
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "Compressor has been flushed",
            );
        }
        handle
            .buffer
            .extend_from_slice(unsafe { data_export.bytes(_py) });
        drop(handle);
        return_bytes(_py, &[])
    })
}

/// `compressor.flush() -> bytes`  (finalises and returns compressed stream)
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_flush(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_compressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "invalid compressor handle",
            );
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if handle.flushed {
            drop(handle);
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
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
                            drop(handle);
                            return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
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
                        drop(handle);
                        return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                    }
                };
                let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                enc.write_all(&input).and_then(|()| enc.finish())
            }
            _ => {
                drop(handle);
                return raise_exception(_py, "ValueError", "unknown format");
            }
        };
        match result {
            Ok(out) => {
                drop(handle);
                return_bytes(_py, &out)
            }
            Err(e) => {
                let msg = format!("lzma compress failed: {e}");
                drop(handle);
                molt_runtime_core::rt_raise_class(_py, error_class, &msg)
            }
        }
    })
}

// ── Stateful LZMADecompressor handle ─────────────────────────────────────────

pub struct LzmaDecompressorHandle {
    format: i64,
    input_buffer: Vec<u8>,
    eof: bool,
    unused_data: Vec<u8>,
    needs_input: bool,
}

fn lzma_decompressor_from_bits(bits: u64) -> Option<Arc<Mutex<LzmaDecompressorHandle>>> {
    unsafe {
        molt_runtime_core::native_custody::clone_native_arc(
            bits,
            __molt_compression_lzma_decompressor_owner_clone,
        )
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_new(format_bits: u64, _memlimit_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let format = match opt_i64(_py, format_bits, FORMAT_AUTO, "format") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let handle = Arc::new(Mutex::new(LzmaDecompressorHandle {
            format,
            input_buffer: Vec::new(),
            eof: false,
            unused_data: Vec::new(),
            needs_input: true,
        }));
        unsafe {
            molt_runtime_core::native_custody::transfer_native_arc(
                handle,
                __molt_compression_lzma_decompressor_owner_new,
            )
        }
    })
}

/// `decompressor.decompress(data, max_length=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_decompress(
    error_class: u64,
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "invalid decompressor handle",
            );
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
        let max_len = match opt_i64(_py, max_length_bits, -1, "max_length") {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if handle.eof {
            drop(handle);
            return raise_exception(_py, "EOFError", "End of stream already reached");
        }
        handle
            .input_buffer
            .extend_from_slice(unsafe { new_data_export.bytes(_py) });
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
                        drop(handle);
                        return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                    }
                };
                let mut dec = xz2::read::XzDecoder::new_stream(input.as_slice(), stream);
                let mut out = Vec::new();
                dec.read_to_end(&mut out).map(|_| out)
            }
            _ => {
                drop(handle);
                return raise_exception(_py, "ValueError", "unknown format");
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
                drop(handle);
                return_bytes(_py, &out)
            }
            Err(_) => {
                handle.input_buffer = input;
                handle.needs_input = true;
                drop(handle);
                return_bytes(_py, &[])
            }
        }
    })
}

/// `decompressor.eof` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_eof(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "invalid decompressor handle",
            );
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        MoltObject::from_bool(handle.eof).bits()
    })
}

/// `decompressor.needs_input` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_needs_input(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "invalid decompressor handle",
            );
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        MoltObject::from_bool(handle.needs_input).bits()
    })
}

/// `decompressor.unused_data` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_unused_data(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "invalid decompressor handle",
            );
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tail = handle.unused_data.clone();
        return_bytes(_py, &tail)
    })
}

// ── Stateful LZMAFile handle ─────────────────────────────────────────────────

enum LzmaFileInner {
    Writing {
        file: File,
        buffer: Vec<u8>,
        format: i64,
        preset: u32,
        check: xz2::stream::Check,
    },
    Reading(BufReader<xz2::read::XzDecoder<File>>),
    ReadingLzma(BufReader<xz2::read::XzDecoder<File>>),
    Closed,
}

pub struct LzmaFileHandle {
    inner: LzmaFileInner,
}

fn lzma_file_handle_from_bits(bits: u64) -> Option<Arc<Mutex<LzmaFileHandle>>> {
    unsafe {
        molt_runtime_core::native_custody::clone_native_arc(
            bits,
            __molt_compression_lzma_file_owner_clone,
        )
    }
}

/// `lzma.open(filename, mode, format, check, preset) -> handle`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_open(
    error_class: u64,
    filename_bits: u64,
    mode_bits: u64,
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let name_obj = obj_from_bits(filename_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            return raise_exception(_py, "TypeError", "filename must be str");
        };
        let mode_obj = obj_from_bits(mode_bits);
        let mode = string_obj_to_owned(mode_obj).unwrap_or_else(|| "rb".to_string());
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
        let inner = match mode.trim_matches('b') {
            "r" => {
                let file = match File::open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("lzma.open: {e}");
                        return raise_exception(_py, "OSError", &msg);
                    }
                };
                match format {
                    FORMAT_ALONE | FORMAT_RAW => {
                        let stream = match xz2::stream::Stream::new_lzma_decoder(u64::MAX) {
                            Ok(s) => s,
                            Err(e) => {
                                let msg = format!("lzma init error: {e}");
                                return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                            }
                        };
                        LzmaFileInner::ReadingLzma(BufReader::new(
                            xz2::read::XzDecoder::new_stream(file, stream),
                        ))
                    }
                    _ => {
                        // FORMAT_XZ or FORMAT_AUTO: XzDecoder auto-detects
                        LzmaFileInner::Reading(BufReader::new(xz2::read::XzDecoder::new(file)))
                    }
                }
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
                        let msg = format!("lzma.open: {e}");
                        return raise_exception(_py, "OSError", &msg);
                    }
                };
                LzmaFileInner::Writing {
                    file,
                    buffer: Vec::new(),
                    format,
                    preset: xz_preset,
                    check: xz_check,
                }
            }
            "a" => {
                let file = match OpenOptions::new().create(true).append(true).open(&name) {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = format!("lzma.open: {e}");
                        return raise_exception(_py, "OSError", &msg);
                    }
                };
                LzmaFileInner::Writing {
                    file,
                    buffer: Vec::new(),
                    format,
                    preset: xz_preset,
                    check: xz_check,
                }
            }
            _ => {
                return raise_exception(
                    _py,
                    "ValueError",
                    "Invalid mode ('r', 'rb', 'w', 'wb', 'a', 'ab' supported)",
                );
            }
        };
        let handle = Arc::new(Mutex::new(LzmaFileHandle { inner }));
        unsafe {
            molt_runtime_core::native_custody::transfer_native_arc(
                handle,
                __molt_compression_lzma_file_owner_new,
            )
        }
    })
}

/// `handle.read(size=-1) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_read(error_class: u64, handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_file_handle_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(_py, error_class, "invalid lzma file handle");
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
        let reader: &mut dyn Read = match &mut handle.inner {
            LzmaFileInner::Reading(r) => r,
            LzmaFileInner::ReadingLzma(r) => r,
            _ => {
                drop(handle);
                return raise_exception(_py, "OSError", "file not open for reading");
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
            drop(handle);
            return molt_runtime_core::rt_raise_class(_py, error_class, "lzma read failed");
        }
        drop(handle);
        return_bytes(_py, &out)
    })
}

/// `handle.write(data) -> int`
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_write(error_class: u64, handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_file_handle_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(_py, error_class, "invalid lzma file handle");
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
        let LzmaFileInner::Writing { ref mut buffer, .. } = handle.inner else {
            drop(handle);
            return raise_exception(_py, "OSError", "file not open for writing");
        };
        buffer.extend_from_slice(unsafe { data_export.bytes(_py) });
        drop(handle);
        int_bits_from_i64(_py, (unsafe { data_export.bytes(_py) }).len() as i64)
    })
}

/// `handle.close() -> None` (finishes the lzma stream and writes to file)
#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_close(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = lzma_file_handle_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(_py, error_class, "invalid lzma file handle");
        };

        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let inner = std::mem::replace(&mut handle.inner, LzmaFileInner::Closed);
        match inner {
            LzmaFileInner::Writing {
                mut file,
                buffer,
                format,
                preset,
                check,
            } => {
                let result: Result<Vec<u8>, std::io::Error> = match format {
                    FORMAT_XZ | FORMAT_AUTO => {
                        let stream = match xz2::stream::Stream::new_easy_encoder(preset, check) {
                            Ok(s) => s,
                            Err(e) => {
                                let msg = format!("lzma init error: {e}");
                                drop(handle);
                                return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                            }
                        };
                        let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                        enc.write_all(&buffer).and_then(|()| enc.finish())
                    }
                    FORMAT_ALONE | FORMAT_RAW => {
                        let opts = xz2::stream::LzmaOptions::new_preset(preset)
                            .unwrap_or_else(|_| xz2::stream::LzmaOptions::new_preset(6).unwrap());
                        let stream = match xz2::stream::Stream::new_lzma_encoder(&opts) {
                            Ok(s) => s,
                            Err(e) => {
                                let msg = format!("lzma init error: {e}");
                                drop(handle);
                                return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                            }
                        };
                        let mut enc = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                        enc.write_all(&buffer).and_then(|()| enc.finish())
                    }
                    _ => {
                        drop(handle);
                        return raise_exception(_py, "ValueError", "unknown format");
                    }
                };
                match result {
                    Ok(compressed) => {
                        if file.write_all(&compressed).is_err() {
                            drop(handle);
                            return raise_exception(_py, "OSError", "lzma write to file failed");
                        }
                    }
                    Err(e) => {
                        let msg = format!("lzma compress failed: {e}");
                        drop(handle);
                        return molt_runtime_core::rt_raise_class(_py, error_class, &msg);
                    }
                }
            }
            LzmaFileInner::Reading(_) | LzmaFileInner::ReadingLzma(_) | LzmaFileInner::Closed => {}
        }
        MoltObject::none().bits()
    })
}
