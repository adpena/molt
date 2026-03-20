// === FILE: runtime/molt-runtime/src/builtins/codecs_ext.rs ===
//
// Intrinsics for incremental codecs, stream reader/writer, BOM constants,
// encoding normalization, and error handler registration.
//
// Architecture
// ────────────
// All stateful objects (IncrementalEncoder, IncrementalDecoder, StreamReader,
// StreamWriter) are stored in thread-local handle tables (HashMap<i64, State>),
// matching the pattern used in builtins/csv.rs.
//
// The actual encode/decode work delegates to the same `encode_string_with_errors`
// / `decode_bytes_text` functions used by builtins/codecs.rs so that all 30+
// codec implementations share a single code path.
//
// Error handlers are stored as NaN-boxed callable bits in a thread-local map
// keyed by name string.  We do *not* call them from within intrinsics (Molt's
// static analysis prevents dynamic callsite lowering); instead we store them
// and surface them to the Python wrapper via `molt_codecs_lookup_error`.

use crate::DecodeFailure as OpsDecodeFailure;
use crate::object::ops::{DecodeTextError as OpsDecodeTextError, EncodeError as OpsEncodeError};
use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

// ─── Handle counters ──────────────────────────────────────────────────────────

static NEXT_INC_ENC_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_INC_DEC_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_STREAM_RDR_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_STREAM_WTR_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_inc_enc_handle() -> i64 {
    NEXT_INC_ENC_HANDLE.fetch_add(1, Ordering::Relaxed)
}
fn next_inc_dec_handle() -> i64 {
    NEXT_INC_DEC_HANDLE.fetch_add(1, Ordering::Relaxed)
}
fn next_stream_rdr_handle() -> i64 {
    NEXT_STREAM_RDR_HANDLE.fetch_add(1, Ordering::Relaxed)
}
fn next_stream_wtr_handle() -> i64 {
    NEXT_STREAM_WTR_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Error handler registry (thread-local, keyed by name)
// ─────────────────────────────────────────────────────────────────────────────

thread_local! {
    /// name → callable bits
    static ERROR_HANDLERS: RefCell<HashMap<String, u64>> = RefCell::new({
        // Pre-populate CPython built-in handler names with a sentinel non-None
        // value (MoltObject::from_bool(true)) so that `lookup_error` can
        // return a truthy object for them.  The Python wrapper recognises these
        // by name and applies the built-in logic.
        let mut m = HashMap::new();
        m.insert("strict".to_owned(),   MoltObject::from_bool(true).bits());
        m.insert("ignore".to_owned(),   MoltObject::from_bool(true).bits());
        m.insert("replace".to_owned(),  MoltObject::from_bool(true).bits());
        m.insert("xmlcharrefreplace".to_owned(), MoltObject::from_bool(true).bits());
        m.insert("backslashreplace".to_owned(),  MoltObject::from_bool(true).bits());
        m.insert("namereplace".to_owned(),       MoltObject::from_bool(true).bits());
        m.insert("surrogateescape".to_owned(),   MoltObject::from_bool(true).bits());
        m.insert("surrogatepass".to_owned(),     MoltObject::from_bool(true).bits());
        m
    });
}

/// Register a custom error handler. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_register_error(name_bits: u64, handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let tn = type_name(_py, name_obj);
            let msg = format!("register_error() argument 'name' must be str, not {tn}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        ERROR_HANDLERS.with(|h| h.borrow_mut().insert(name, handler_bits));
        MoltObject::none().bits()
    })
}

/// Lookup an error handler by name. Raises LookupError if unknown.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_lookup_error(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let tn = type_name(_py, name_obj);
            let msg = format!("lookup_error() argument must be str, not {tn}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let handler = ERROR_HANDLERS.with(|h| h.borrow().get(&name).copied());
        match handler {
            Some(bits) => bits,
            None => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Incremental encoder
// ─────────────────────────────────────────────────────────────────────────────

struct IncrementalEncoderState {
    encoding: String,
    errors: String,
    /// Accumulated pending input (for codecs that may produce incomplete output
    /// until `final_=True`). For stateless encodings this is always empty.
    pending: Vec<u8>,
}

thread_local! {
    static INC_ENC_HANDLES: RefCell<HashMap<i64, IncrementalEncoderState>> =
        RefCell::new(HashMap::new());
}

fn inc_enc_id_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(handle_bits)).or_else(|| {
        let _ = raise_exception::<u64>(_py, "TypeError", "incremental encoder handle must be int");
        None
    })
}

/// Create a new IncrementalEncoder. Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_encoder_new(encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(encoding) = string_obj_to_owned(obj_from_bits(encoding_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "encoding must be str");
        };
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());

        // Validate encoding + error handler by attempting a dummy encode.
        match encode_string_with_errors(&[], &encoding, Some(&errors)) {
            Ok(_) => {}
            Err(OpsEncodeError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsEncodeError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsEncodeError::InvalidChar { .. }) => {}
        }

        let id = next_inc_enc_handle();
        INC_ENC_HANDLES.with(|h| {
            h.borrow_mut().insert(
                id,
                IncrementalEncoderState {
                    encoding,
                    errors,
                    pending: Vec::new(),
                },
            );
        });
        MoltObject::from_int(id).bits()
    })
}

/// Encode `input_bits` (str) and return bytes.  When `final_bits` is truthy,
/// flush any pending state.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_encoder_encode(
    handle_bits: u64,
    input_bits: u64,
    final_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = inc_enc_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let input_obj = obj_from_bits(input_bits);
        let Some(input_str) = string_obj_to_owned(input_obj) else {
            return raise_exception::<_>(_py, "TypeError", "input must be str");
        };
        let is_final = is_truthy(_py, obj_from_bits(final_bits));

        let (encoding, errors) = INC_ENC_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| (s.encoding.clone(), s.errors.clone()))
                .unwrap_or_default()
        });
        if encoding.is_empty() {
            return raise_exception::<_>(_py, "RuntimeError", "invalid encoder handle");
        }

        let result = encode_string_with_errors(input_str.as_bytes(), &encoding, Some(&errors));
        match result {
            Ok(bytes) => {
                let _ = is_final; // stateless encodings: no pending
                let ptr = alloc_bytes(_py, &bytes);
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(OpsEncodeError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(OpsEncodeError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(OpsEncodeError::InvalidChar {
                encoding,
                code,
                pos,
                limit,
            }) => {
                let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                raise_unicode_encode_error::<_>(_py, encoding, input_bits, pos, pos + 1, &reason)
            }
        }
    })
}

/// Reset incremental encoder state. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_encoder_reset(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = inc_enc_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        INC_ENC_HANDLES.with(|h| {
            if let Some(state) = h.borrow_mut().get_mut(&id) {
                state.pending.clear();
            }
        });
        MoltObject::none().bits()
    })
}

/// Drop incremental encoder handle. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_encoder_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = inc_enc_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        INC_ENC_HANDLES.with(|h| h.borrow_mut().remove(&id));
        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Incremental decoder
// ─────────────────────────────────────────────────────────────────────────────

struct IncrementalDecoderState {
    encoding: String,
    errors: String,
    /// Buffered bytes not yet decoded (e.g. a partial multi-byte sequence).
    buffer: Vec<u8>,
}

thread_local! {
    static INC_DEC_HANDLES: RefCell<HashMap<i64, IncrementalDecoderState>> =
        RefCell::new(HashMap::new());
}

fn inc_dec_id_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(handle_bits)).or_else(|| {
        let _ = raise_exception::<u64>(_py, "TypeError", "incremental decoder handle must be int");
        None
    })
}

/// Create a new IncrementalDecoder. Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_decoder_new(encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(encoding) = string_obj_to_owned(obj_from_bits(encoding_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "encoding must be str");
        };
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());

        // Validate.
        match decode_bytes_text(&encoding, &errors, &[]) {
            Ok(_) => {}
            Err(OpsDecodeTextError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsDecodeTextError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsDecodeTextError::Failure(_, _)) => {}
        }

        let id = next_inc_dec_handle();
        INC_DEC_HANDLES.with(|h| {
            h.borrow_mut().insert(
                id,
                IncrementalDecoderState {
                    encoding,
                    errors,
                    buffer: Vec::new(),
                },
            );
        });
        MoltObject::from_int(id).bits()
    })
}

/// Decode `input_bits` (bytes) and return str.  Buffers incomplete byte
/// sequences when `final_bits` is falsy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_decoder_decode(
    handle_bits: u64,
    input_bits: u64,
    final_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = inc_dec_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let input_obj = obj_from_bits(input_bits);
        let Some(ptr) = input_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "input must be bytes");
        };
        let Some(input_bytes) = (unsafe { bytes_like_slice(ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "input must be bytes");
        };
        let is_final = is_truthy(_py, obj_from_bits(final_bits));

        // Copy input bytes into a local vec to avoid borrow lifetime conflicts.
        let input_vec = input_bytes.to_vec();

        let (encoding, errors, mut buffer) = INC_DEC_HANDLES.with(|h| {
            let s = h.borrow();
            if let Some(state) = s.get(&id) {
                (
                    state.encoding.clone(),
                    state.errors.clone(),
                    state.buffer.clone(),
                )
            } else {
                (String::new(), String::new(), Vec::new())
            }
        });
        if encoding.is_empty() {
            return raise_exception::<_>(_py, "RuntimeError", "invalid decoder handle");
        }

        buffer.extend_from_slice(&input_vec);

        let decode_input = if is_final {
            buffer.clone()
        } else {
            // For incremental: try to decode all available bytes; on failure,
            // leave up to 3 trailing bytes buffered (max multi-byte seq length).
            buffer.clone()
        };

        let result = decode_bytes_text(&encoding, &errors, &decode_input);
        match result {
            Ok((text_bytes, _label)) => {
                // Reset buffer on success.
                INC_DEC_HANDLES.with(|h| {
                    if let Some(state) = h.borrow_mut().get_mut(&id) {
                        state.buffer.clear();
                    }
                });
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(OpsDecodeTextError::Failure(OpsDecodeFailure::Byte { pos, .. }, _))
                if !is_final =>
            {
                // Buffer bytes from pos onward; return decoded prefix.
                let decoded_part = &decode_input[..pos.min(decode_input.len())];
                let remainder = decode_input[pos.min(decode_input.len())..].to_vec();
                INC_DEC_HANDLES.with(|h| {
                    if let Some(state) = h.borrow_mut().get_mut(&id) {
                        state.buffer = remainder;
                    }
                });
                // Decode the prefix that succeeded.
                let prefix_result = decode_bytes_text(&encoding, "ignore", decoded_part);
                let text_bytes = prefix_result.map(|(b, _)| b).unwrap_or_default();
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(OpsDecodeTextError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(OpsDecodeTextError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(OpsDecodeTextError::Failure(
                OpsDecodeFailure::Byte { pos, byte, message },
                label,
            )) => {
                let msg = format!(
                    "'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}"
                );
                raise_exception::<_>(_py, "UnicodeDecodeError", &msg)
            }
            Err(OpsDecodeTextError::Failure(
                OpsDecodeFailure::Range {
                    start,
                    end,
                    message,
                },
                label,
            )) => {
                let msg = format!(
                    "'{label}' codec can't decode bytes in position {start}-{end}: {message}"
                );
                raise_exception::<_>(_py, "UnicodeDecodeError", &msg)
            }
            Err(OpsDecodeTextError::Failure(
                OpsDecodeFailure::UnknownErrorHandler(name),
                _label,
            )) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
        }
    })
}

/// Reset incremental decoder buffer. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_decoder_reset(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = inc_dec_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        INC_DEC_HANDLES.with(|h| {
            if let Some(state) = h.borrow_mut().get_mut(&id) {
                state.buffer.clear();
            }
        });
        MoltObject::none().bits()
    })
}

/// Drop incremental decoder handle. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_decoder_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = inc_dec_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        INC_DEC_HANDLES.with(|h| h.borrow_mut().remove(&id));
        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Stream reader
//
// stream_bits is an opaque i64 file handle (from molt_io / MoltFileHandle).
// We store the handle ID plus codec state.  Actual reads are forwarded to the
// Molt I/O layer via the file handle ABI.  For the intrinsic boundary we accept
// the handle as an integer, read bytes from the underlying file, and decode.
// ─────────────────────────────────────────────────────────────────────────────

struct StreamReaderState {
    encoding: String,
    errors: String,
    /// Buffered undecoded bytes (from partial multi-byte sequences).
    buffer: Vec<u8>,
    /// The underlying stream object bits (passed back to I/O intrinsics).
    stream_bits: u64,
}

thread_local! {
    static STREAM_RDR_HANDLES: RefCell<HashMap<i64, StreamReaderState>> =
        RefCell::new(HashMap::new());
}

fn stream_rdr_id(_py: &PyToken<'_>, bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(bits)).or_else(|| {
        let _ = raise_exception::<u64>(_py, "TypeError", "stream reader handle must be int");
        None
    })
}

/// Create a StreamReader wrapping `stream_bits` (a file-like object handle).
/// Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_reader_new(
    stream_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(encoding) = string_obj_to_owned(obj_from_bits(encoding_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "encoding must be str");
        };
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());

        let id = next_stream_rdr_handle();
        STREAM_RDR_HANDLES.with(|h| {
            h.borrow_mut().insert(
                id,
                StreamReaderState {
                    encoding,
                    errors,
                    buffer: Vec::new(),
                    stream_bits,
                },
            );
        });
        MoltObject::from_int(id).bits()
    })
}

/// Read up to `size_bits` characters from the stream (int; -1 for all).
/// Returns str.
///
/// This intrinsic reads raw bytes from the stream object, decodes them, and
/// returns the resulting string.  `size_bits` is a character count hint; the
/// intrinsic reads bytes at a ~4x character ratio to handle multi-byte encodings.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_reader_read(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = stream_rdr_id(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(-1);

        let (encoding, errors, stream_bits) = STREAM_RDR_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| (s.encoding.clone(), s.errors.clone(), s.stream_bits))
                .unwrap_or_default()
        });
        if encoding.is_empty() {
            return raise_exception::<_>(_py, "RuntimeError", "invalid stream reader handle");
        }

        // Read raw bytes from the underlying stream via molt_io read builtin.
        // We call the file-read intrinsic with a byte count.
        let byte_count = if size < 0 {
            // Read all: use -1 sentinel.
            MoltObject::from_int(-1).bits()
        } else {
            // Over-read by 4x to handle multi-byte encodings.
            MoltObject::from_int(size * 4).bits()
        };

        let raw_bits = crate::molt_file_read(stream_bits, byte_count);
        let raw_obj = obj_from_bits(raw_bits);
        if raw_obj.is_none() {
            // EOF or error propagated via exception; return empty string.
            if !exception_pending(_py) {
                let ptr = alloc_string(_py, b"");
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            return MoltObject::none().bits();
        }
        let Some(raw_ptr) = raw_obj.as_ptr() else {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
            }
            return MoltObject::from_ptr(ptr).bits();
        };
        let Some(raw_bytes) = (unsafe { bytes_like_slice(raw_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "stream read() must return bytes");
        };
        let raw_vec = raw_bytes.to_vec();

        // Prepend any buffered bytes.
        let buffered = STREAM_RDR_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.buffer.clone())
                .unwrap_or_default()
        });
        let mut combined = buffered;
        combined.extend_from_slice(&raw_vec);

        let result = decode_bytes_text(&encoding, &errors, &combined);
        match result {
            Ok((text_bytes, _)) => {
                STREAM_RDR_HANDLES.with(|h| {
                    if let Some(s) = h.borrow_mut().get_mut(&id) {
                        s.buffer.clear();
                    }
                });
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(e) => stream_decode_error(_py, e),
        }
    })
}

/// Read one line from the stream. Returns str.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_reader_readline(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = stream_rdr_id(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let (encoding, errors, stream_bits) = STREAM_RDR_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| (s.encoding.clone(), s.errors.clone(), s.stream_bits))
                .unwrap_or_default()
        });
        if encoding.is_empty() {
            return raise_exception::<_>(_py, "RuntimeError", "invalid stream reader handle");
        }

        // Call the underlying readline.
        let raw_bits = crate::molt_file_readline(stream_bits, MoltObject::from_int(-1).bits());
        let raw_obj = obj_from_bits(raw_bits);
        if raw_obj.is_none() {
            if !exception_pending(_py) {
                let ptr = alloc_string(_py, b"");
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            return MoltObject::none().bits();
        }
        let Some(raw_ptr) = raw_obj.as_ptr() else {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
            }
            return MoltObject::from_ptr(ptr).bits();
        };
        let Some(raw_bytes) = (unsafe { bytes_like_slice(raw_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "stream readline() must return bytes");
        };
        let raw_vec = raw_bytes.to_vec();

        let result = decode_bytes_text(&encoding, &errors, &raw_vec);
        match result {
            Ok((text_bytes, _)) => {
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(e) => stream_decode_error(_py, e),
        }
    })
}

/// Drop stream reader handle. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_reader_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = stream_rdr_id(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        STREAM_RDR_HANDLES.with(|h| h.borrow_mut().remove(&id));
        MoltObject::none().bits()
    })
}

fn stream_decode_error(_py: &PyToken<'_>, e: OpsDecodeTextError) -> u64 {
    match e {
        OpsDecodeTextError::UnknownEncoding(name) => {
            let msg = format!("unknown encoding: {name}");
            raise_exception::<u64>(_py, "LookupError", &msg)
        }
        OpsDecodeTextError::UnknownErrorHandler(name) => {
            let msg = format!("unknown error handler name '{name}'");
            raise_exception::<u64>(_py, "LookupError", &msg)
        }
        OpsDecodeTextError::Failure(OpsDecodeFailure::Byte { pos, byte, message }, label) => {
            let msg = format!(
                "'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}"
            );
            raise_exception::<u64>(_py, "UnicodeDecodeError", &msg)
        }
        OpsDecodeTextError::Failure(
            OpsDecodeFailure::Range {
                start,
                end,
                message,
            },
            label,
        ) => {
            let msg =
                format!("'{label}' codec can't decode bytes in position {start}-{end}: {message}");
            raise_exception::<u64>(_py, "UnicodeDecodeError", &msg)
        }
        OpsDecodeTextError::Failure(OpsDecodeFailure::UnknownErrorHandler(name), _label) => {
            let msg = format!("unknown error handler name '{name}'");
            raise_exception::<u64>(_py, "LookupError", &msg)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stream writer
// ─────────────────────────────────────────────────────────────────────────────

struct StreamWriterState {
    encoding: String,
    errors: String,
    stream_bits: u64,
}

thread_local! {
    static STREAM_WTR_HANDLES: RefCell<HashMap<i64, StreamWriterState>> =
        RefCell::new(HashMap::new());
}

fn stream_wtr_id(_py: &PyToken<'_>, bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(bits)).or_else(|| {
        let _ = raise_exception::<u64>(_py, "TypeError", "stream writer handle must be int");
        None
    })
}

/// Create a StreamWriter wrapping `stream_bits`. Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_writer_new(
    stream_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(encoding) = string_obj_to_owned(obj_from_bits(encoding_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "encoding must be str");
        };
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());

        let id = next_stream_wtr_handle();
        STREAM_WTR_HANDLES.with(|h| {
            h.borrow_mut().insert(
                id,
                StreamWriterState {
                    encoding,
                    errors,
                    stream_bits,
                },
            );
        });
        MoltObject::from_int(id).bits()
    })
}

/// Encode `text_bits` (str) and write to the underlying stream.
/// Returns int: number of bytes written.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_writer_write(handle_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = stream_wtr_id(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let text_obj = obj_from_bits(text_bits);
        let Some(text) = string_obj_to_owned(text_obj) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };

        let (encoding, errors, stream_bits) = STREAM_WTR_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| (s.encoding.clone(), s.errors.clone(), s.stream_bits))
                .unwrap_or_default()
        });
        if encoding.is_empty() {
            return raise_exception::<_>(_py, "RuntimeError", "invalid stream writer handle");
        }

        let encoded = match encode_string_with_errors(text.as_bytes(), &encoding, Some(&errors)) {
            Ok(b) => b,
            Err(OpsEncodeError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsEncodeError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsEncodeError::InvalidChar {
                encoding,
                code,
                pos,
                limit,
            }) => {
                let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                return raise_unicode_encode_error::<_>(
                    _py,
                    encoding,
                    text_bits,
                    pos,
                    pos + 1,
                    &reason,
                );
            }
        };
        let n_bytes = encoded.len();

        // Write bytes to the underlying stream.
        let bytes_ptr = alloc_bytes(_py, &encoded);
        if bytes_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let _ = crate::molt_file_write(stream_bits, bytes_bits);

        MoltObject::from_int(n_bytes as i64).bits()
    })
}

/// Drop stream writer handle. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_writer_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = stream_wtr_id(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        STREAM_WTR_HANDLES.with(|h| h.borrow_mut().remove(&id));
        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// BOM constants
// ─────────────────────────────────────────────────────────────────────────────

/// Return b'\xef\xbb\xbf' (UTF-8 BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf8() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_bytes(_py, &[0xEF, 0xBB, 0xBF]);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\xff\xfe' (UTF-16-LE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf16_le() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_bytes(_py, &[0xFF, 0xFE]);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\xfe\xff' (UTF-16-BE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf16_be() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_bytes(_py, &[0xFE, 0xFF]);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\xff\xfe\x00\x00' (UTF-32-LE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf32_le() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_bytes(_py, &[0xFF, 0xFE, 0x00, 0x00]);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\x00\x00\xfe\xff' (UTF-32-BE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf32_be() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_bytes(_py, &[0x00, 0x00, 0xFE, 0xFF]);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate bytes");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// charmap_build / charmap_decode / charmap_encode / make_identity_dict
// ─────────────────────────────────────────────────────────────────────────────

/// Build an encoding map from a decoding table string.
/// Maps each character in the decoding table to its index (skipping U+FFFE).
/// Returns a dict (int ordinal → int index).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_charmap_build(table_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let table_obj = obj_from_bits(table_bits);
        let Some(table) = string_obj_to_owned(table_obj) else {
            return raise_exception::<_>(_py, "TypeError", "charmap_build argument must be str");
        };
        // alloc_dict_with_pairs expects a flat &[u64] with alternating key, value
        let mut flat_pairs: Vec<u64> = Vec::new();
        for (i, ch) in table.chars().enumerate() {
            if ch == '\u{FFFE}' {
                continue;
            }
            flat_pairs.push(MoltObject::from_int(ch as i64).bits());
            flat_pairs.push(MoltObject::from_int(i as i64).bits());
        }
        let dict_ptr = crate::alloc_dict_with_pairs(_py, &flat_pairs);
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

/// Decode bytes using a character mapping.
/// `input_bits`: bytes to decode
/// `errors_bits`: error mode string ("strict", "ignore", "replace")
/// `mapping_bits`: dict or None (None = latin-1 identity)
/// Returns a (str, int) tuple of (decoded_text, bytes_consumed).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_charmap_decode(
    input_bits: u64,
    errors_bits: u64,
    mapping_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let input_obj = obj_from_bits(input_bits);
        let Some(input_ptr) = input_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "input must be bytes");
        };
        let Some(input_bytes) = (unsafe { bytes_like_slice(input_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "input must be bytes");
        };
        let input_vec = input_bytes.to_vec();
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());
        let mapping_obj = obj_from_bits(mapping_bits);

        // None mapping = latin-1 identity decode
        if mapping_obj.is_none() {
            let decoded: String = input_vec.iter().map(|&b| b as char).collect();
            let str_ptr = alloc_string(_py, decoded.as_bytes());
            if str_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let str_bits = MoltObject::from_ptr(str_ptr).bits();
            let len_bits = MoltObject::from_int(input_vec.len() as i64).bits();
            let elems = [str_bits, len_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        let map_ptr = match mapping_obj.as_ptr() {
            Some(p) => p,
            None => {
                return raise_exception::<_>(_py, "TypeError", "mapping must be a dict or None");
            }
        };

        let mut out = String::new();
        for (pos, &b) in input_vec.iter().enumerate() {
            let key_bits = MoltObject::from_int(b as i64).bits();
            let mapped = unsafe { crate::dict_get_in_place(_py, map_ptr, key_bits) };
            match mapped {
                Some(val) => {
                    let val_obj = obj_from_bits(val);
                    if let Some(s) = string_obj_to_owned(val_obj) {
                        out.push_str(&s);
                    } else if let Some(i) = crate::to_i64(val_obj) {
                        if let Some(ch) = char::from_u32(i as u32) {
                            out.push(ch);
                        }
                    }
                }
                None => match errors.as_str() {
                    "ignore" => continue,
                    "replace" => out.push('\u{FFFD}'),
                    _ => {
                        let msg = format!(
                            "'charmap' codec can't decode byte 0x{b:02x} in position {pos}: character maps to <undefined>"
                        );
                        return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                    }
                },
            }
        }
        let str_ptr = alloc_string(_py, out.as_bytes());
        if str_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let str_bits = MoltObject::from_ptr(str_ptr).bits();
        let len_bits = MoltObject::from_int(input_vec.len() as i64).bits();
        let elems = [str_bits, len_bits];
        let tuple_ptr = crate::alloc_tuple(_py, &elems);
        if tuple_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

/// Encode a string using a character mapping.
/// `input_bits`: str to encode
/// `errors_bits`: error mode string
/// `mapping_bits`: dict or None (None = latin-1 identity)
/// Returns a (bytes, int) tuple of (encoded_bytes, chars_consumed).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_charmap_encode(
    input_bits: u64,
    errors_bits: u64,
    mapping_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let input_obj = obj_from_bits(input_bits);
        let Some(input_str) = string_obj_to_owned(input_obj) else {
            return raise_exception::<_>(_py, "TypeError", "input must be str");
        };
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());
        let mapping_obj = obj_from_bits(mapping_bits);

        // None mapping = latin-1 identity encode
        if mapping_obj.is_none() {
            match encode_string_with_errors(input_str.as_bytes(), "latin-1", Some(&errors)) {
                Ok(encoded) => {
                    let bytes_ptr = alloc_bytes(_py, &encoded);
                    if bytes_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
                    let len_bits = MoltObject::from_int(input_str.len() as i64).bits();
                    let elems = [bytes_bits, len_bits];
                    let tuple_ptr = crate::alloc_tuple(_py, &elems);
                    if tuple_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "UnicodeEncodeError",
                        "charmap encode failed with latin-1 fallback",
                    );
                }
            }
        }

        let mut out: Vec<u8> = Vec::new();
        let map_ptr = mapping_obj.as_ptr();
        for (idx, ch) in input_str.chars().enumerate() {
            let mut found = false;
            if let Some(mp) = map_ptr {
                // Try character key first
                let ch_key_ptr = alloc_string(_py, ch.encode_utf8(&mut [0u8; 4]).as_bytes());
                if !ch_key_ptr.is_null() {
                    let ch_key_bits = MoltObject::from_ptr(ch_key_ptr).bits();
                    let val = unsafe { crate::dict_get_in_place(_py, mp, ch_key_bits) };
                    crate::dec_ref_bits(_py, ch_key_bits);
                    if let Some(v) = val {
                        let v_obj = obj_from_bits(v);
                        if let Some(b_ptr) = v_obj.as_ptr() {
                            if let Some(b_slice) = unsafe { bytes_like_slice(b_ptr) } {
                                out.extend_from_slice(b_slice);
                                found = true;
                            }
                        }
                        if !found {
                            if let Some(i) = crate::to_i64(v_obj) {
                                out.push((i & 0xFF) as u8);
                                found = true;
                            } else if let Some(s) = string_obj_to_owned(v_obj) {
                                out.extend_from_slice(s.as_bytes().get(..1).unwrap_or(b"?"));
                                found = true;
                            }
                        }
                    }
                }
                // Try ordinal key
                if !found {
                    let ord_key = MoltObject::from_int(ch as i64).bits();
                    let val = unsafe { crate::dict_get_in_place(_py, mp, ord_key) };
                    if let Some(v) = val {
                        let v_obj = obj_from_bits(v);
                        if let Some(b_ptr) = v_obj.as_ptr() {
                            if let Some(b_slice) = unsafe { bytes_like_slice(b_ptr) } {
                                out.extend_from_slice(b_slice);
                                found = true;
                            }
                        }
                        if !found {
                            if let Some(i) = crate::to_i64(v_obj) {
                                out.push((i & 0xFF) as u8);
                                found = true;
                            }
                        }
                    }
                }
            }
            if !found {
                match errors.as_str() {
                    "ignore" => continue,
                    "replace" => out.push(b'?'),
                    _ => {
                        let msg = format!(
                            "'charmap' codec can't encode character '\\u{:04x}' in position {}: character maps to <undefined>",
                            ch as u32, idx
                        );
                        return raise_exception::<_>(_py, "UnicodeEncodeError", &msg);
                    }
                }
            }
        }
        let bytes_ptr = alloc_bytes(_py, &out);
        if bytes_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let len_bits = MoltObject::from_int(input_str.len() as i64).bits();
        let elems = [bytes_bits, len_bits];
        let tuple_ptr = crate::alloc_tuple(_py, &elems);
        if tuple_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

/// Build an identity dict mapping each integer in `range_bits` to itself.
/// Returns a dict.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_make_identity_dict(range_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let range_obj = obj_from_bits(range_bits);
        let Some(items) = crate::decode_value_list(range_obj) else {
            return raise_exception::<_>(_py, "TypeError", "argument must be iterable of ints");
        };
        // flat pairs: [key, val, key, val, ...]
        let mut flat_pairs: Vec<u64> = Vec::with_capacity(items.len() * 2);
        for item_bits in &items {
            flat_pairs.push(*item_bits);
            flat_pairs.push(*item_bits);
        }
        let dict_ptr = crate::alloc_dict_with_pairs(_py, &flat_pairs);
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Encoding name normalization
// ─────────────────────────────────────────────────────────────────────────────

/// Normalize an encoding name to its canonical form.
/// Raises LookupError for unknown encodings.  Returns str.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_normalize_encoding(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let tn = type_name(_py, name_obj);
            let msg = format!("normalize_encoding() argument must be str, not {tn}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(kind) = crate::object::ops::normalize_encoding(&name) else {
            let msg = format!("unknown encoding: {name}");
            return raise_exception::<_>(_py, "LookupError", &msg);
        };
        let canonical = crate::object::ops::encoding_kind_name(kind);
        let ptr = alloc_string(_py, canonical.as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
