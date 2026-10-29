use crate::bridge::*;
use crate::compression_common::{CodecIndexRole, codec_index};
use libz_rs_sys as zlib;
use molt_runtime_capi::ByteInput;
use molt_runtime_core::prelude::*;
use std::borrow::Cow;
use std::io::{self, BufRead, Read};
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn return_bytes(_py: &PyToken, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception(_py, BuiltinExc::MemoryError, "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── Existing raw deflate/inflate ─────────────────────────────────────────────
#[unsafe(no_mangle)]
pub extern "C" fn molt_deflate_raw(data_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &export,
            data_bits,
            "compress",
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
        // The internal ZIP entry contract uses None for its default level.
        let level = if obj_from_bits(level_bits).is_none() {
            -1
        } else {
            match codec_index(_py, level_bits, CodecIndexRole::CInt) {
                Ok(value) => value as i32,
                Err(bits) => return bits,
            }
        };
        match compress_bytes(unsafe { export.bytes(_py) }, level, -15) {
            Ok(out) => return_bytes(_py, &out),
            Err(ZlibFailure::Memory) => {
                raise_exception(_py, BuiltinExc::MemoryError, "deflate out of memory")
            }
            Err(ZlibFailure::Initialization(zlib::Z_STREAM_ERROR)) => raise_exception(
                _py,
                BuiltinExc::ValueError,
                "deflate level must be in range -1..9",
            ),
            Err(_) => raise_exception(_py, BuiltinExc::ValueError, "deflate failed"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inflate_raw(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // Internal ZIP storage accepts only physical bytes/bytearray here.
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception(_py, BuiltinExc::TypeError, "inflate expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                return raise_exception(
                    _py,
                    BuiltinExc::TypeError,
                    &format!("inflate expects bytes, got {}", type_name(_py, obj)),
                );
            }
            let input = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
            match decompress_bytes(input, -15, 16384) {
                Ok(out) => return_bytes(_py, &out),
                Err(ZlibFailure::Memory) => {
                    raise_exception(_py, BuiltinExc::MemoryError, "inflate out of memory")
                }
                Err(_) => raise_exception(_py, BuiltinExc::ValueError, "invalid deflate data"),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compress(
    error_class: u64,
    data_bits: u64,
    level_bits: u64,
    wbits_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &export,
            data_bits,
            "compress",
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
        let level = match codec_index(_py, level_bits, CodecIndexRole::CInt) {
            Ok(value) => value as i32,
            Err(bits) => return bits,
        };
        let wbits = match codec_index(_py, wbits_bits, CodecIndexRole::CInt) {
            Ok(value) => value as i32,
            Err(bits) => return bits,
        };
        match compress_bytes(unsafe { export.bytes(_py) }, level, wbits) {
            Ok(out) => return_bytes(_py, &out),
            Err(ZlibFailure::Memory) => raise_exception(
                _py,
                BuiltinExc::MemoryError,
                "Out of memory while compressing data",
            ),
            Err(ZlibFailure::Initialization(zlib::Z_STREAM_ERROR)) => {
                molt_runtime_core::rt_raise_class(_py, error_owner.bits(), "Bad compression level")
            }
            Err(error) => molt_runtime_core::rt_raise_class(
                _py,
                error_owner.bits(),
                &error.message("while compressing data"),
            ),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompress(
    error_class: u64,
    data_bits: u64,
    wbits_bits: u64,
    bufsize_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &export,
            data_bits,
            "decompress",
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
        let wbits = match codec_index(_py, wbits_bits, CodecIndexRole::CInt) {
            Ok(value) => value as i32,
            Err(bits) => return bits,
        };
        let initial = match codec_index(_py, bufsize_bits, CodecIndexRole::OutputSize) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if initial < 0 {
            return raise_exception(_py, BuiltinExc::ValueError, "bufsize must be non-negative");
        }
        match decompress_bytes(unsafe { export.bytes(_py) }, wbits, initial.max(1) as usize) {
            Ok(out) => return_bytes(_py, &out),
            Err(ZlibFailure::Memory) => raise_exception(
                _py,
                BuiltinExc::MemoryError,
                "Out of memory while decompressing data",
            ),
            Err(ZlibFailure::Initialization(code)) => molt_runtime_core::rt_raise_class(
                _py,
                error_owner.bits(),
                &zlib_error_message(code, None, "while preparing to decompress data"),
            ),
            Err(error) => molt_runtime_core::rt_raise_class(
                _py,
                error_owner.bits(),
                &error.message("while decompressing data"),
            ),
        }
    })
}

// Shared pure byte consumers for gzip/tar/ZIP adapters. They hold no Python
// references and reuse these exact stream owners, including fallible startup.
pub(crate) enum ZlibFailure {
    Memory,
    Initialization(i32),
    // Boxed so the cold message keeps every Result<_, ZlibFailure> small.
    Stream(i32, Option<Box<ZlibMessage>>),
}

impl ZlibFailure {
    pub(crate) fn message(&self, phase: &str) -> String {
        match self {
            Self::Memory => "out of memory".to_owned(),
            Self::Initialization(code) => zlib_error_message(*code, None, phase),
            Self::Stream(code, message) => zlib_error_message(*code, message.as_deref(), phase),
        }
    }
}

pub(crate) fn compress_bytes(input: &[u8], level: i32, wbits: i32) -> Result<Vec<u8>, ZlibFailure> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(16384)
        .map_err(|_| ZlibFailure::Memory)?;
    let encoder = DeflateState::new(level, 8, wbits, 8, 0).map_err(|code| {
        if code == zlib::Z_MEM_ERROR {
            ZlibFailure::Memory
        } else {
            ZlibFailure::Initialization(code)
        }
    })?;
    let mut handle = CompressorHandle {
        encoder: Some(encoder),
    };
    let result = handle
        .deflate(input, zlib::Z_FINISH, 16384, output)
        .map_err(|_| ZlibFailure::Memory)?;
    match result.status {
        Ok(ZlibStatus::StreamEnd) => Ok(result.output),
        Err(code) => Err(ZlibFailure::Stream(code, result.message.map(Box::new))),
        Ok(_) => Err(ZlibFailure::Stream(
            zlib::Z_BUF_ERROR,
            result.message.map(Box::new),
        )),
    }
}

pub(crate) fn decompress_bytes(
    input: &[u8],
    wbits: i32,
    initial: usize,
) -> Result<Vec<u8>, ZlibFailure> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(initial)
        .map_err(|_| ZlibFailure::Memory)?;
    let decoder = InflateState::new(wbits).map_err(|code| {
        if code == zlib::Z_MEM_ERROR {
            ZlibFailure::Memory
        } else {
            ZlibFailure::Initialization(code)
        }
    })?;
    let mut handle = DecompressorHandle {
        decoder: Some(decoder),
        unconsumed_tail: Vec::new(),
        unused_data: Vec::new(),
        eof: false,
    };
    let result = handle
        .inflate(input, isize::MAX as usize, initial, zlib::Z_FINISH, output)
        .map_err(|_| ZlibFailure::Memory)?;
    match result.status {
        Ok(ZlibStatus::StreamEnd) => Ok(result.output),
        Err(zlib::Z_MEM_ERROR) => Err(ZlibFailure::Memory),
        Err(code) => Err(ZlibFailure::Stream(code, result.message.map(Box::new))),
        Ok(_) => Err(ZlibFailure::Stream(
            zlib::Z_BUF_ERROR,
            result.message.map(Box::new),
        )),
    }
}

// ── zlib.crc32 ───────────────────────────────────────────────────────────────

/// `zlib.crc32(data, value=0) -> int`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_crc32(data_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &data_export,
            data_bits,
            "crc32",
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
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
        let result = molt_runtime_core::crc32::crc32(unsafe { data_export.bytes(_py) }, initial);
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
        let data_export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &data_export,
            data_bits,
            "adler32",
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
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
        let result = adler32_compute(unsafe { data_export.bytes(_py) }, initial);
        int_bits_from_i64(_py, i64::from(result))
    })
}

// ── Constants ────────────────────────────────────────────────────────────────
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_max_wbits() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 15))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_def_mem_level() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 8))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_def_buf_size() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 16384))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_default_compression() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, -1))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_best_speed() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 1))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_best_compression() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 9))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_no_compression() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 0))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_filtered() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 1))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_huffman_only() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 2))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_default_strategy() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 0))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_finish() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 4))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_no_flush() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 0))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_sync_flush() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 2))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_full_flush() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, int_bits_from_i64(_py, 3))
}

// ── Streaming compressobj ────────────────────────────────────────────────────

// One public, fallibly initialized maintained-backend stream. Like upstream's
// stable owner, it moves its inline z_stream: deflate state has no back-pointer.
struct DeflateState {
    stream: zlib::z_stream,
}

// SAFETY: production uses the backend Rust allocator with null opaque; callers
// exclusively own the state under CompressorHandle's Mutex and clear all
// borrowed input/output pointers before returning from the bounded call.
unsafe impl Send for DeflateState {}

impl DeflateState {
    fn new(level: i32, method: i32, wbits: i32, memlevel: i32, strategy: i32) -> Result<Self, i32> {
        unsafe {
            Self::initialize(
                level,
                method,
                wbits,
                memlevel,
                strategy,
                zlib::z_stream::default(),
            )
        }
    }

    // Custom allocator callbacks/opaque must remain valid and transferable for
    // the stream lifetime. The failure tests use this same public contract.
    unsafe fn initialize(
        level: i32,
        method: i32,
        wbits: i32,
        memlevel: i32,
        strategy: i32,
        mut stream: zlib::z_stream,
    ) -> Result<Self, i32> {
        let status = unsafe {
            zlib::deflateInit2_(
                &mut stream,
                level,
                method,
                wbits,
                memlevel,
                strategy,
                zlib::zlibVersion(),
                size_of::<zlib::z_stream>() as i32,
            )
        };
        if status == zlib::Z_OK {
            Ok(Self { stream })
        } else {
            Err(status)
        }
    }

    fn end(&mut self) -> i32 {
        unsafe { zlib::deflateEnd(&mut self.stream) }
    }
}

impl Drop for DeflateState {
    fn drop(&mut self) {
        // deflateEnd clears this public state pointer, also on unfinished-data
        // errors. Explicit finish and ordinary owner retirement each free once.
        if !self.stream.state.is_null() {
            self.end();
        }
    }
}

pub struct CompressorHandle {
    encoder: Option<DeflateState>,
}

impl CompressorHandle {
    fn deflate(
        &mut self,
        input: &[u8],
        flush: i32,
        initial: usize,
        mut output: Vec<u8>,
    ) -> Result<ZlibResult, std::collections::TryReserveError> {
        let Some(encoder) = self.encoder.as_mut() else {
            return Ok(ZlibResult {
                output: Vec::new(),
                consumed: 0,
                status: Err(zlib::Z_STREAM_ERROR),
                message: None,
            });
        };
        let mut consumed = 0;
        let mut size = initial;
        loop {
            output.try_reserve_exact(size - output.len())?;
            let old_len = output.len();
            let available = (size - old_len).min(u32::MAX as usize);
            let supplied = (input.len() - consumed).min(u32::MAX as usize);
            // Finish/flush only the final input window; earlier windows must
            // consume normally, without closing or adding intermediate markers.
            let window_flush = if input.len() - consumed > u32::MAX as usize {
                zlib::Z_NO_FLUSH
            } else {
                flush
            };
            encoder.stream.next_in = input[consumed..].as_ptr();
            encoder.stream.avail_in = supplied as u32;
            encoder.stream.next_out = output.spare_capacity_mut().as_mut_ptr().cast();
            encoder.stream.avail_out = available as u32;
            let code = unsafe { zlib::deflate(&mut encoder.stream, window_flush) };
            let used = supplied - encoder.stream.avail_in as usize;
            let written = available - encoder.stream.avail_out as usize;
            encoder.stream.next_in = std::ptr::null();
            encoder.stream.avail_in = 0;
            encoder.stream.next_out = std::ptr::null_mut();
            encoder.stream.avail_out = 0;
            consumed += used;
            // Only the public avail_out delta is initialized, even on failure.
            unsafe { output.set_len(old_len + written) };
            let mut status = match code {
                zlib::Z_OK => Ok(ZlibStatus::Progress),
                zlib::Z_BUF_ERROR => Ok(ZlibStatus::NeedInputOrOutput),
                zlib::Z_STREAM_END => Ok(ZlibStatus::StreamEnd),
                error => Err(error),
            };
            if used == 0 && written == 0 && consumed < input.len() {
                status = Err(zlib::Z_BUF_ERROR);
            }
            if status.is_err()
                || status == Ok(ZlibStatus::StreamEnd)
                || (consumed == input.len() && written < available)
            {
                let mut message = snapshot_zlib_message(&encoder.stream);
                if status == Ok(ZlibStatus::StreamEnd) && flush == zlib::Z_FINISH {
                    let code = encoder.end();
                    if code != zlib::Z_OK {
                        status = Err(code);
                        message = snapshot_zlib_message(&encoder.stream);
                    }
                    self.encoder = None;
                }
                return Ok(ZlibResult {
                    output,
                    consumed,
                    status,
                    message,
                });
            }
            if output.len() == size {
                size = size.saturating_mul(2);
            }
        }
    }
}

fn compressor_from_bits(bits: u64) -> Option<Arc<Mutex<CompressorHandle>>> {
    unsafe {
        molt_runtime_core::clone_native_arc(bits, __molt_compression_zlib_compressor_owner_clone)
    }
}

/// Options are converted in Clinic order before backend validation. Supplied
/// dictionary bytes are borrowed only through deflateSetDictionary, never saved
/// as an untraced Python edge in the native owner.
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_new(
    error_class: u64,
    level_bits: u64,
    method_bits: u64,
    wbits_bits: u64,
    memlevel_bits: u64,
    strategy_bits: u64,
    zdict_bits: u64,
    zdict_present_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let Some(zdict_present) = obj_from_bits(zdict_present_bits).as_bool() else {
            return raise_exception(_py, BuiltinExc::SystemError, "invalid zdict presence flag");
        };
        let mut options = [0i32; 5];
        for (value, bits) in options.iter_mut().zip([
            level_bits,
            method_bits,
            wbits_bits,
            memlevel_bits,
            strategy_bits,
        ]) {
            *value = match codec_index(_py, bits, CodecIndexRole::CInt) {
                Ok(value) => value as i32,
                Err(bits) => return bits,
            };
        }
        let dictionary = if zdict_present {
            let export = match unsafe { ByteInput::acquire(_py, zdict_bits) } {
                Ok(export) => export,
                Err(_) => return MoltObject::none().bits(),
            };
            if !crate::compression_common::validate_clinic_buffer(
                _py,
                &export,
                zdict_bits,
                "compressobj",
                "argument 'zdict'",
            ) {
                return MoltObject::none().bits();
            }
            Some(export)
        } else {
            None
        };
        if dictionary
            .as_ref()
            .is_some_and(|input| unsafe { input.bytes(_py) }.len() > u32::MAX as usize)
        {
            return raise_exception(
                _py,
                BuiltinExc::OverflowError,
                "zdict length does not fit in an unsigned int",
            );
        }
        let mut encoder =
            match DeflateState::new(options[0], options[1], options[2], options[3], options[4]) {
                Ok(encoder) => encoder,
                Err(zlib::Z_MEM_ERROR) => {
                    return raise_exception(
                        _py,
                        BuiltinExc::MemoryError,
                        "Can't allocate memory for compression object",
                    );
                }
                Err(zlib::Z_STREAM_ERROR) => {
                    return raise_exception(
                        _py,
                        BuiltinExc::ValueError,
                        "Invalid initialization option",
                    );
                }
                Err(code) => {
                    return molt_runtime_core::rt_raise_class(
                        _py,
                        error_owner.bits(),
                        &zlib_error_message(code, None, "while creating compression object"),
                    );
                }
            };
        if let Some(dictionary) = dictionary.as_ref() {
            let bytes = unsafe { dictionary.bytes(_py) };
            let code = unsafe {
                zlib::deflateSetDictionary(&mut encoder.stream, bytes.as_ptr(), bytes.len() as u32)
            };
            if code != zlib::Z_OK {
                return raise_exception(
                    _py,
                    BuiltinExc::ValueError,
                    if code == zlib::Z_STREAM_ERROR {
                        "Invalid dictionary"
                    } else {
                        "deflateSetDictionary()"
                    },
                );
            }
        }
        let owner = Arc::new(Mutex::new(CompressorHandle {
            encoder: Some(encoder),
        }));
        unsafe {
            molt_runtime_core::transfer_native_arc(
                owner,
                __molt_compression_zlib_compressor_owner_new,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_compress(
    error_class: u64,
    handle_bits: u64,
    data_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let Some(handle_owner) = compressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_owner.bits(),
                "invalid compressor handle",
            );
        };
        let export = match unsafe { ByteInput::acquire(_py, data_bits) } {
            Ok(export) => export,
            Err(_) => return MoltObject::none().bits(),
        };
        if !crate::compression_common::validate_clinic_buffer(
            _py,
            &export,
            data_bits,
            "compress",
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
        let result = {
            let mut handle = handle_owner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            handle.deflate(
                unsafe { export.bytes(_py) },
                zlib::Z_NO_FLUSH,
                16384,
                Vec::new(),
            )
        };
        finish_deflate_output(_py, error_owner.bits(), result, "while compressing data")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_flush(
    error_class: u64,
    handle_bits: u64,
    mode_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let Some(handle_owner) = compressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_owner.bits(),
                "invalid compressor handle",
            );
        };
        let mode = match codec_index(_py, mode_bits, CodecIndexRole::CInt) {
            Ok(value) => value as i32,
            Err(bits) => return bits,
        };
        // Clinic converts even on a retired stream; Z_NO_FLUSH then does
        // nothing, including after a successful Z_FINISH.
        if mode == zlib::Z_NO_FLUSH {
            return return_bytes(_py, &[]);
        }
        let result = {
            let mut handle = handle_owner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            handle.deflate(&[], mode, 16384, Vec::new())
        };
        finish_deflate_output(_py, error_owner.bits(), result, "while flushing")
    })
}

fn finish_deflate_output(
    py: &PyToken,
    error_class: u64,
    result: Result<ZlibResult, std::collections::TryReserveError>,
    phase: &str,
) -> u64 {
    match result {
        Err(_) => raise_exception(
            py,
            BuiltinExc::MemoryError,
            "Out of memory while compressing data",
        ),
        Ok(result) => match result.status {
            Err(code) => molt_runtime_core::rt_raise_class(
                py,
                error_class,
                &zlib_error_message(code, result.message.as_ref(), phase),
            ),
            Ok(_) => return_bytes(py, &result.output),
        },
    }
}

// ── Streaming decompressobj ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ZlibStatus {
    Progress,
    NeedInputOrOutput,
    StreamEnd,
}

// The public C API uses the same maintained backend as flate2, but exposes its
// fallible initialization result. Upstream's stable Rust owner also moves its
// inline z_stream after initialization: inflate state has no back-pointer to it.
struct InflateState {
    stream: zlib::z_stream,
}

// SAFETY: only the Rust allocator with null opaque is installed in production.
// All state use is exclusive through DecompressorHandle's existing Mutex, and
// transient caller slices are cleared before that lock can be released.
unsafe impl Send for InflateState {}

impl InflateState {
    fn new(wbits: i32) -> Result<Self, i32> {
        unsafe { Self::initialize(wbits, zlib::z_stream::default()) }
    }

    // One bounded engine call shared by the Python stream and file reader.
    // It borrows no input/output beyond this call and never allocates a buffer.
    fn window(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], flush: i32) -> ZlibWindow {
        let supplied = input.len().min(u32::MAX as usize);
        let available = output.len().min(u32::MAX as usize);
        self.stream.next_in = input.as_ptr();
        self.stream.avail_in = supplied as u32;
        self.stream.next_out = output.as_mut_ptr().cast();
        self.stream.avail_out = available as u32;
        let code = unsafe { zlib::inflate(&mut self.stream, flush) };
        let consumed = supplied - self.stream.avail_in as usize;
        let written = available - self.stream.avail_out as usize;
        self.stream.next_in = std::ptr::null();
        self.stream.avail_in = 0;
        self.stream.next_out = std::ptr::null_mut();
        self.stream.avail_out = 0;
        let status = match code {
            zlib::Z_OK => Ok(ZlibStatus::Progress),
            zlib::Z_BUF_ERROR => Ok(ZlibStatus::NeedInputOrOutput),
            zlib::Z_STREAM_END => Ok(ZlibStatus::StreamEnd),
            error => Err(error),
        };
        ZlibWindow {
            consumed,
            written,
            status,
            message: if status.is_err() {
                snapshot_zlib_message(&self.stream)
            } else {
                None
            },
        }
    }

    // SAFETY: custom allocation callbacks and opaque must remain valid for the
    // stream lifetime and support its transfer between threads. This is the
    // actual z_stream allocation contract, also used by the failure control.
    unsafe fn initialize(wbits: i32, mut stream: zlib::z_stream) -> Result<Self, i32> {
        let status = unsafe {
            zlib::inflateInit2_(
                &mut stream,
                wbits,
                zlib::zlibVersion(),
                size_of::<zlib::z_stream>() as i32,
            )
        };
        if status == zlib::Z_OK {
            Ok(Self { stream })
        } else {
            // inflateInit2_ cleans up its own partial initialization on failure.
            Err(status)
        }
    }
}

impl Drop for InflateState {
    fn drop(&mut self) {
        unsafe { zlib::inflateEnd(&mut self.stream) };
    }
}

pub struct DecompressorHandle {
    // One maintained backend stream, retired only by a completed flush or Drop.
    // No compressed input or decompressed output is replayed on the next call.
    decoder: Option<InflateState>,
    unconsumed_tail: Vec<u8>,
    unused_data: Vec<u8>,
    eof: bool,
}

// CPython zlib_error limits the backend diagnostic to 200 bytes. Copy that
// bounded payload while the stream is owned, without allocating under its lock.
pub(crate) struct ZlibMessage {
    bytes: [u8; 200],
    len: usize,
}

struct ZlibResult {
    output: Vec<u8>,
    consumed: usize,
    status: Result<ZlibStatus, i32>,
    message: Option<ZlibMessage>,
}

struct ZlibWindow {
    consumed: usize,
    written: usize,
    status: Result<ZlibStatus, i32>,
    message: Option<ZlibMessage>,
}

// Existing filename gzip reads are sequential and single-member. Preserve that
// contract with bounded buffered input and direct caller output; no replay,
// per-read decoder, decoded history, or second decompression implementation.
pub(crate) struct GzipReader<R> {
    input: R,
    decoder: InflateState,
    eof: bool,
}

impl<R: BufRead> GzipReader<R> {
    pub(crate) fn new(input: R) -> Result<Self, i32> {
        Ok(Self {
            input,
            decoder: InflateState::new(31)?,
            eof: false,
        })
    }
}

impl<R: BufRead> Read for GzipReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.eof {
            return Ok(0);
        }
        loop {
            let input = self.input.fill_buf()?;
            let source_empty = input.is_empty();
            // Every byte of the caller's initialized output remains initialized;
            // the shared engine reports exactly which prefix it overwrote.
            let spare = unsafe {
                std::slice::from_raw_parts_mut(
                    output.as_mut_ptr().cast::<MaybeUninit<u8>>(),
                    output.len(),
                )
            };
            // Stop at the deflate block boundary so final body bytes are
            // observable before the next read validates the gzip trailer, as
            // with the previous sequential GzDecoder reader.
            let result = self.decoder.window(input, spare, zlib::Z_BLOCK);
            self.input.consume(result.consumed);
            if result.status.is_err() {
                // The native reader boundary owns its existing fixed OSError
                // message. Keep this transport error allocation-free under the
                // handle mutex; public zlib calls retain the backend diagnostic.
                return Err(io::ErrorKind::InvalidData.into());
            }
            if result.status == Ok(ZlibStatus::StreamEnd) {
                self.eof = true;
            }
            if result.written != 0 || self.eof {
                return Ok(result.written);
            }
            // Z_BLOCK can finish an empty block using bits retained from a
            // prior input window. Public data_type bit128 identifies that
            // boundary; the next call advances past it without new input.
            if result.consumed == 0 && self.decoder.stream.data_type & 128 != 0 {
                continue;
            }
            if source_empty {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            if result.consumed == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }
    }
}

impl DecompressorHandle {
    fn inflate(
        &mut self,
        input: &[u8],
        limit: usize,
        initial: usize,
        flush: i32,
        mut output: Vec<u8>,
    ) -> Result<ZlibResult, std::collections::TryReserveError> {
        let Some(decoder) = self.decoder.as_mut() else {
            return Ok(ZlibResult {
                output: Vec::new(),
                consumed: 0,
                status: Err(zlib::Z_STREAM_ERROR),
                message: None,
            });
        };
        if self.eof {
            return Ok(ZlibResult {
                output: Vec::new(),
                consumed: 0,
                status: Ok(ZlibStatus::StreamEnd),
                message: None,
            });
        }
        let mut consumed = 0;
        let mut size = initial.min(limit);
        loop {
            // Spare capacity is the actual output window. Restrict it explicitly:
            // an allocator may provide more capacity than requested.
            output.try_reserve_exact(size - output.len())?;
            let old_len = output.len();
            let available = (size - old_len).min(u32::MAX as usize);
            let window_flush =
                if flush == zlib::Z_FINISH && input.len() - consumed > u32::MAX as usize {
                    zlib::Z_NO_FLUSH
                } else {
                    flush
                };
            let result = decoder.window(
                &input[consumed..],
                &mut output.spare_capacity_mut()[..available],
                window_flush,
            );
            let used = result.consumed;
            let written = result.written;
            let status = result.status;
            consumed += used;
            // The backend initialized exactly the avail_out delta, including on
            // errors. No runtime callback occurs while these slices are live.
            unsafe { output.set_len(old_len + written) };
            let complete = status == Ok(ZlibStatus::StreamEnd) || status.is_err();
            if complete
                || output.len() == limit
                || (used == 0 && written == 0)
                || (consumed == input.len() && written < available)
            {
                return Ok(ZlibResult {
                    output,
                    consumed,
                    status,
                    message: result.message,
                });
            }
            if output.len() == size {
                size = size.saturating_mul(2).min(limit);
            }
        }
    }

    // CPython save_unconsumed_input: each decompress call supplies only its
    // explicit input. A caller chooses whether to resubmit unconsumed_tail;
    // flush is the sole operation that implicitly consumes that saved tail.
    fn save_input<'a>(
        &mut self,
        input: Cow<'a, [u8]>,
        result: &ZlibResult,
        had_tail: bool,
    ) -> Result<(), (std::collections::TryReserveError, Cow<'a, [u8]>)> {
        let remaining = &input[result.consumed..];
        let ended = result.status == Ok(ZlibStatus::StreamEnd);
        if ended && !remaining.is_empty() {
            if let Err(error) = self.unused_data.try_reserve(remaining.len()) {
                return Err((error, input));
            }
            self.unused_data.extend_from_slice(remaining);
        }
        if (!ended && !remaining.is_empty()) || had_tail {
            self.unconsumed_tail = match input {
                Cow::Borrowed(input) => {
                    let remaining = &input[result.consumed..];
                    let mut tail = Vec::new();
                    if let Err(error) = tail.try_reserve_exact(remaining.len()) {
                        return Err((error, Cow::Borrowed(input)));
                    }
                    tail.extend_from_slice(remaining);
                    tail
                }
                Cow::Owned(mut input) => {
                    input.drain(..result.consumed);
                    input
                }
            };
        }
        if ended {
            self.eof = true;
        }
        Ok(())
    }
}

fn snapshot_zlib_message(stream: &zlib::z_stream) -> Option<ZlibMessage> {
    if stream.msg.is_null() {
        return None;
    }
    let source = unsafe { std::ffi::CStr::from_ptr(stream.msg) }.to_bytes();
    let len = source.len().min(200);
    let mut message = ZlibMessage {
        bytes: [0; 200],
        len,
    };
    message.bytes[..len].copy_from_slice(&source[..len]);
    Some(message)
}

fn zlib_error_message(code: i32, message: Option<&ZlibMessage>, phase: &str) -> String {
    let message = message.map(|message| String::from_utf8_lossy(&message.bytes[..message.len]));
    let fallback = match code {
        zlib::Z_VERSION_ERROR => Some("library version mismatch"),
        zlib::Z_BUF_ERROR => Some("incomplete or truncated stream"),
        zlib::Z_STREAM_ERROR => Some("inconsistent stream state"),
        zlib::Z_DATA_ERROR => Some("invalid input data"),
        _ => None,
    };
    match message.as_deref().or(fallback) {
        Some(message) => format!("Error {code} {phase}: {message}"),
        None => format!("Error {code} {phase}"),
    }
}

fn decompressor_from_bits(bits: u64) -> Option<Arc<Mutex<DecompressorHandle>>> {
    unsafe {
        molt_runtime_core::clone_native_arc(bits, __molt_compression_zlib_decompressor_owner_clone)
    }
}

/// `zlib.decompressobj(wbits=MAX_WBITS) -> handle`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_new(wbits_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let wbits = match codec_index(_py, wbits_bits, CodecIndexRole::CInt) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let decoder = match InflateState::new(wbits as i32) {
            Ok(decoder) => decoder,
            Err(zlib::Z_MEM_ERROR) => {
                return raise_exception(
                    _py,
                    BuiltinExc::MemoryError,
                    "Can't allocate memory for decompression object",
                );
            }
            Err(zlib::Z_STREAM_ERROR) => {
                return raise_exception(
                    _py,
                    BuiltinExc::ValueError,
                    "Invalid initialization option",
                );
            }
            Err(_) => {
                return raise_exception(
                    _py,
                    BuiltinExc::SystemError,
                    "zlib decompressor ABI initialization failed",
                );
            }
        };
        let handle = Arc::new(Mutex::new(DecompressorHandle {
            decoder: Some(decoder),
            unconsumed_tail: Vec::new(),
            unused_data: Vec::new(),
            eof: false,
        }));
        unsafe {
            molt_runtime_core::transfer_native_arc(
                handle,
                __molt_compression_zlib_decompressor_owner_new,
            )
        }
    })
}

/// `decompressobj.decompress(data, max_length=0) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_decompress(
    error_class: u64,
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "zlib.error: invalid decompressor handle",
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
            "argument 1",
        ) {
            return MoltObject::none().bits();
        }
        let max_length = match codec_index(_py, max_length_bits, CodecIndexRole::OutputSize) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if max_length < 0 {
            return raise_exception(
                _py,
                BuiltinExc::ValueError,
                "max_length must be non-negative",
            );
        }
        let limit = if max_length == 0 {
            isize::MAX as usize
        } else {
            max_length as usize
        };
        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let input = unsafe { new_data_export.bytes(_py) };
        let result = match handle.inflate(input, limit, 16 * 1024, zlib::Z_SYNC_FLUSH, Vec::new()) {
            Ok(result) => result,
            Err(_) => {
                drop(handle);
                return raise_exception(_py, BuiltinExc::MemoryError, "out of memory");
            }
        };
        let had_tail = !handle.unconsumed_tail.is_empty();
        if handle
            .save_input(Cow::Borrowed(input), &result, had_tail)
            .is_err()
        {
            drop(handle);
            return raise_exception(_py, BuiltinExc::MemoryError, "out of memory");
        }
        drop(handle);
        if let Err(error) = result.status {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                &zlib_error_message(error, result.message.as_ref(), "while decompressing data"),
            );
        }
        return_bytes(_py, &result.output)
    })
}

/// `decompressobj.flush(length=DEF_BUF_SIZE) -> bytes`
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_flush(
    error_class: u64,
    handle_bits: u64,
    _length_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "zlib.error: invalid decompressor handle",
            );
        };

        let length = match codec_index(_py, _length_bits, CodecIndexRole::OutputSize) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if length <= 0 {
            return raise_exception(
                _py,
                BuiltinExc::ValueError,
                "length must be greater than zero",
            );
        }
        let mut handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Move, rather than copy, the only input implicitly consumed by flush.
        let input = std::mem::take(&mut handle.unconsumed_tail);
        let result = match handle.inflate(
            &input,
            isize::MAX as usize,
            length as usize,
            zlib::Z_FINISH,
            Vec::new(),
        ) {
            Ok(result) => result,
            Err(_) => {
                handle.unconsumed_tail = input;
                drop(handle);
                return raise_exception(_py, BuiltinExc::MemoryError, "out of memory");
            }
        };
        let had_tail = !input.is_empty();
        if let Err((_, input)) = handle.save_input(Cow::Owned(input), &result, had_tail) {
            // The tail was moved only to borrow it during inflate. A failed
            // unused_data allocation must not retire that pre-existing owner.
            handle.unconsumed_tail = input.into_owned();
            drop(handle);
            return raise_exception(_py, BuiltinExc::MemoryError, "out of memory");
        }
        if result.status == Ok(ZlibStatus::StreamEnd) {
            handle.decoder = None;
        }
        // CPython flush preserves incomplete state and returns partial output;
        // only STREAM_END retires inflate state. Repeated flush returns b"".
        drop(handle);
        return_bytes(_py, &result.output)
    })
}

/// `decompressobj.eof` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_eof(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "zlib.error: invalid decompressor handle",
            );
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        MoltObject::from_bool(handle.eof).bits()
    })
}

/// `decompressobj.unconsumed_tail` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_unconsumed_tail(
    error_class: u64,
    handle_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "zlib.error: invalid decompressor handle",
            );
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        {
            let bytes = handle.unconsumed_tail.clone();
            drop(handle);
            return_bytes(_py, &bytes)
        }
    })
}

/// `decompressobj.unused_data` property.
///
/// Per CPython zlib semantics, this is b"" until the last byte of compressed
/// data has been processed; once the stream ends it holds any bytes found
/// past the end of the compressed data (e.g. trailing/concatenated streams).
#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_unused_data(error_class: u64, handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let error_owner = OwnedRuntimeValue::retain(_py, error_class);
        let error_class = error_owner.bits();
        let Some(handle_owner) = decompressor_from_bits(handle_bits) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                error_class,
                "zlib.error: invalid decompressor handle",
            );
        };

        let handle = handle_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        {
            let bytes = handle.unused_data.clone();
            drop(handle);
            return_bytes(_py, &bytes)
        }
    })
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DeniedAllocator {
        allocations: AtomicUsize,
        frees: AtomicUsize,
    }

    unsafe extern "C" fn deny_allocation(
        opaque: zlib::voidpf,
        _items: zlib::uInt,
        _size: zlib::uInt,
    ) -> zlib::voidpf {
        let counts = unsafe { &*opaque.cast::<DeniedAllocator>() };
        counts.allocations.fetch_add(1, Ordering::Relaxed);
        std::ptr::null_mut()
    }

    unsafe extern "C" fn count_free(opaque: zlib::voidpf, _pointer: zlib::voidpf) {
        let counts = unsafe { &*opaque.cast::<DeniedAllocator>() };
        counts.frees.fetch_add(1, Ordering::Relaxed);
    }

    #[test]
    fn inflate_initialization_reports_allocator_denial() {
        // Exercise the maintained public allocator contract, not a mocked
        // status. A backend allocation refusal must be returned, never asserted.
        let mut counts = DeniedAllocator {
            allocations: AtomicUsize::new(0),
            frees: AtomicUsize::new(0),
        };
        let stream = zlib::z_stream {
            zalloc: Some(deny_allocation),
            zfree: Some(count_free),
            opaque: (&mut counts as *mut DeniedAllocator).cast(),
            ..zlib::z_stream::default()
        };
        let result = unsafe { InflateState::initialize(-15, stream) };
        assert!(matches!(result, Err(zlib::Z_MEM_ERROR)));
        assert!(counts.allocations.load(Ordering::Relaxed) > 0);
        assert_eq!(counts.frees.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn raw_stream_keeps_progress_and_trailer_after_owner_move() {
        // Independent RFC 1951 final stored block, followed by non-deflate data.
        // Moving the initialized owner must preserve the backend state, and a
        // bounded output window must retain compressed input rather than replay.
        let input = b"\x01\x0b\x00\xf4\xffhello worldTAIL";
        let mut decoder = InflateState::new(-15).unwrap();
        // Exercise the actual cumulative-counter boundary without allocating a
        // 4 GiB input. On every native width, an unchecked backend += would fail.
        decoder.stream.total_in = zlib::uLong::MAX;
        let mut handle = DecompressorHandle {
            decoder: Some(decoder),
            unconsumed_tail: Vec::new(),
            unused_data: Vec::new(),
            eof: false,
        };
        let first = handle
            .inflate(input, 3, 3, zlib::Z_SYNC_FLUSH, Vec::new())
            .unwrap();
        assert_eq!(first.output, b"hel");
        assert_eq!(
            handle.decoder.as_ref().unwrap().stream.total_in,
            zlib::uLong::MAX.wrapping_add(first.consumed as zlib::uLong),
        );
        assert_ne!(first.status, Ok(ZlibStatus::StreamEnd));
        assert!(
            handle
                .save_input(Cow::Borrowed(input), &first, false)
                .is_ok()
        );
        assert!(!handle.eof);
        let tail = std::mem::take(&mut handle.unconsumed_tail);
        let tail_pointer = tail.as_ptr();
        let last = handle
            .inflate(&tail, isize::MAX as usize, 1, zlib::Z_FINISH, Vec::new())
            .unwrap();
        assert_eq!(last.output, b"lo world");
        assert_eq!(last.status, Ok(ZlibStatus::StreamEnd));
        assert!(handle.save_input(Cow::Owned(tail), &last, true).is_ok());
        assert!(handle.eof);
        assert_eq!(handle.unused_data, b"TAIL");
        assert_eq!(handle.unconsumed_tail, b"TAIL");
        assert_eq!(handle.unconsumed_tail.as_ptr(), tail_pointer);
        let stream = &handle.decoder.as_ref().unwrap().stream;
        assert!(stream.next_in.is_null());
        assert!(stream.next_out.is_null());
        assert_eq!((stream.avail_in, stream.avail_out), (0, 0));
    }

    #[test]
    fn deflate_initialization_reports_allocator_denial_and_invalid_options() {
        let mut counts = DeniedAllocator {
            allocations: AtomicUsize::new(0),
            frees: AtomicUsize::new(0),
        };
        let stream = zlib::z_stream {
            zalloc: Some(deny_allocation),
            zfree: Some(count_free),
            opaque: (&mut counts as *mut DeniedAllocator).cast(),
            ..zlib::z_stream::default()
        };
        let result = unsafe { DeflateState::initialize(6, 8, 15, 8, 0, stream) };
        assert!(matches!(result, Err(zlib::Z_MEM_ERROR)));
        assert!(counts.allocations.load(Ordering::Relaxed) > 0);
        assert_eq!(counts.frees.load(Ordering::Relaxed), 0);
        for (level, method, window, memory, strategy) in [
            (10, 8, 15, 8, 0),
            (6, 7, 15, 8, 0),
            (6, 8, -8, 8, 0),
            (6, 8, 24, 8, 0),
            (6, 8, 15, 0, 0),
            (6, 8, 15, 8, 5),
        ] {
            assert!(matches!(
                DeflateState::new(level, method, window, memory, strategy),
                Err(zlib::Z_STREAM_ERROR)
            ));
        }
    }

    struct CountingAllocator {
        allocations: AtomicUsize,
        frees: AtomicUsize,
    }

    unsafe extern "C" fn counted_allocate(
        opaque: zlib::voidpf,
        items: zlib::uInt,
        size: zlib::uInt,
    ) -> zlib::voidpf {
        let context = unsafe { &*opaque.cast::<CountingAllocator>() };
        let Some(bytes) = (items as usize).checked_mul(size as usize) else {
            return std::ptr::null_mut();
        };
        if bytes == 0 {
            return std::ptr::null_mut();
        }
        let Some(total) = bytes.checked_add(std::mem::size_of::<usize>()) else {
            return std::ptr::null_mut();
        };
        let Ok(layout) = std::alloc::Layout::from_size_align(total, std::mem::align_of::<usize>())
        else {
            return std::ptr::null_mut();
        };
        let base = unsafe { std::alloc::alloc(layout) };
        if base.is_null() {
            return std::ptr::null_mut();
        }
        // A custom zalloc/zfree pair owns its allocation layout. The backend
        // provides any additional alignment it needs around this returned span.
        unsafe { base.cast::<usize>().write(total) };
        context.allocations.fetch_add(1, Ordering::Relaxed);
        unsafe { base.add(std::mem::size_of::<usize>()).cast() }
    }

    unsafe extern "C" fn counted_free(opaque: zlib::voidpf, pointer: zlib::voidpf) {
        if pointer.is_null() {
            return;
        }
        let context = unsafe { &*opaque.cast::<CountingAllocator>() };
        let base = unsafe { pointer.cast::<u8>().sub(std::mem::size_of::<usize>()) };
        let total = unsafe { base.cast::<usize>().read() };
        // The prefix was written by counted_allocate with this exact layout.
        let layout = unsafe {
            std::alloc::Layout::from_size_align_unchecked(total, std::mem::align_of::<usize>())
        };
        unsafe { std::alloc::dealloc(base, layout) };
        context.frees.fetch_add(1, Ordering::Relaxed);
    }

    #[test]
    fn deflate_owner_retirement_and_bounded_output_preserve_stored_block() {
        // Exercise the public custom-allocator contract with a paired allocator,
        // recording real successful allocations and frees. Backend default Rust
        // callbacks have a private layout-bearing free convention and cannot be
        // detached from that backend's allocator-identity dispatch.
        let mut counts = CountingAllocator {
            allocations: AtomicUsize::new(0),
            frees: AtomicUsize::new(0),
        };
        for finish in [false, true] {
            let stream = zlib::z_stream {
                zalloc: Some(counted_allocate),
                zfree: Some(counted_free),
                opaque: (&mut counts as *mut CountingAllocator).cast(),
                ..zlib::z_stream::default()
            };
            let encoder = unsafe { DeflateState::initialize(0, 8, -15, 8, 0, stream) }.unwrap();
            let owner = Arc::new(Mutex::new(CompressorHandle {
                encoder: Some(encoder),
            }));
            let retained = Arc::clone(&owner);
            drop(owner);
            assert!(
                counts.allocations.load(Ordering::Relaxed) > counts.frees.load(Ordering::Relaxed)
            );
            if finish {
                let mut handle = retained.lock().unwrap();
                // One-byte initial output forces growth; this literal is the
                // RFC 1951 final stored block for the eleven-byte plaintext.
                let result = handle
                    .deflate(b"hello world", zlib::Z_FINISH, 1, Vec::new())
                    .unwrap();
                assert_eq!(result.status, Ok(ZlibStatus::StreamEnd));
                assert_eq!(result.output, b"\x01\x0b\x00\xf4\xffhello world");
                assert_eq!(result.consumed, 11);
                assert!(handle.encoder.is_none());
                let again = handle
                    .deflate(b"x", zlib::Z_NO_FLUSH, 1, Vec::new())
                    .unwrap();
                assert_eq!(again.status, Err(zlib::Z_STREAM_ERROR));
            }
            drop(retained);
            assert_eq!(
                counts.allocations.load(Ordering::Relaxed),
                counts.frees.load(Ordering::Relaxed)
            );
        }
    }

    #[test]
    fn gzip_reader_tiny_input_truncation_and_trailer_preserve_partial_output() {
        // Independent gzip header + RFC stored block + literal CRC32/ISIZE.
        let member = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\xff\x01\x0b\x00\xf4\xffhello world\x85\x11\x4a\x0d\x0b\x00\x00\x00";
        let input = std::io::BufReader::with_capacity(1, std::io::Cursor::new(member));
        let mut reader = GzipReader::new(input).unwrap();
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        let mut decoded = Vec::new();
        let mut byte = [0u8; 1];
        while reader.read(&mut byte).unwrap() != 0 {
            decoded.push(byte[0]);
        }
        assert_eq!(decoded, b"hello world");
        assert_eq!(reader.read(&mut byte).unwrap(), 0);
        assert!(reader.decoder.stream.next_in.is_null());
        assert!(reader.decoder.stream.next_out.is_null());

        for bad_checksum in [false, true] {
            let mut input = member.to_vec();
            if bad_checksum {
                input[26] ^= 1;
            } else {
                input.truncate(input.len() - 1);
            }
            let mut reader = GzipReader::new(std::io::Cursor::new(input)).unwrap();
            let mut output = [0u8; 64];
            let size = reader.read(&mut output).unwrap();
            assert_eq!(&output[..size], b"hello world");
            let error = reader.read(&mut output).unwrap_err();
            assert_eq!(
                error.kind(),
                if bad_checksum {
                    io::ErrorKind::InvalidData
                } else {
                    io::ErrorKind::UnexpectedEof
                }
            );
        }
        let empty_member =
            b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\xff\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let mut empty_stream = GzipReader::new(std::io::Cursor::new(empty_member)).unwrap();
        assert_eq!(empty_stream.read(&mut byte).unwrap(), 0);
        assert!(empty_stream.eof);
        let mut members = member.to_vec();
        members.extend_from_slice(member);
        let mut one_member = GzipReader::new(std::io::Cursor::new(members)).unwrap();
        let mut first = Vec::new();
        one_member.read_to_end(&mut first).unwrap();
        assert_eq!(first, b"hello world");
        assert_eq!(one_member.input.position(), member.len() as u64);

        // Three empty fixed-Huffman blocks, the third final: each consists
        // of its three-bit header plus the seven-bit end-of-block symbol.
        // Whole input plus tiny output also exercises buffered-bit boundaries.
        let empty_blocks = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\xff\x02\x08\x30\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let mut empty_blocks = GzipReader::new(std::io::Cursor::new(empty_blocks)).unwrap();
        assert_eq!(empty_blocks.read(&mut byte).unwrap(), 0);
        assert!(empty_blocks.eof);

        let mut empty = GzipReader::new(std::io::Cursor::new(&[])).unwrap();
        assert_eq!(
            empty.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
