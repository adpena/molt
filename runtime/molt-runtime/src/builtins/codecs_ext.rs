// === FILE: runtime/molt-runtime/src/builtins/codecs_ext.rs ===
//
// Intrinsics for incremental codecs, stream readers, BOM constants,
// encoding normalization, and error handler registration.
//
// Architecture
// ────────────
// IncrementalEncoder, IncrementalDecoder and StreamReader state is stored
// in thread-local handle tables (HashMap<i64, State>),
// matching the pattern used in builtins/csv.rs.
//
// The actual encode/decode work delegates to the same `encode_string_with_errors`
// / `decode_bytes_text` functions used by builtins/codecs.rs so that all 30+
// codec implementations share a single code path.
//
// Error handlers are stored as runtime-owned NaN-boxed callable bits, matching
// CPython's interpreter-wide registry. We do *not* call them from intrinsics (Molt's
// static analysis prevents dynamic callsite lowering); instead we store them
// and surface them to the Python wrapper via `molt_codecs_lookup_error`.

use crate::object::ops::DecodeTextError as OpsDecodeTextError;
use crate::object::ops_encoding::{
    DecodeFailure as OpsDecodeFailure, push_xmlcharref_ascii, unicode_escape_codepoint,
    unicode_name_escape,
};
use crate::*;
use molt_stdlib_text::wtf8::{push_backslash_bytes_vec, push_wtf8_codepoint};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

// ─── Handle counters ──────────────────────────────────────────────────────────

static NEXT_INC_ENC_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_INC_DEC_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_STREAM_RDR_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_inc_enc_handle() -> i64 {
    NEXT_INC_ENC_HANDLE.fetch_add(1, Ordering::Relaxed)
}
fn next_inc_dec_handle() -> i64 {
    NEXT_INC_DEC_HANDLE.fetch_add(1, Ordering::Relaxed)
}
fn next_stream_rdr_handle() -> i64 {
    NEXT_STREAM_RDR_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Error handler registry (runtime-owned, keyed by name)
// ─────────────────────────────────────────────────────────────────────────────

/// Register a custom error handler. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_register_error(name_bits: u64, handler_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let tn = type_name(_py, name_obj);
            let msg = format!("register_error() argument 'name' must be str, not {tn}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if !crate::builtins::callable::is_callable_impl(_py, handler_bits) {
            return raise_exception::<_>(_py, "TypeError", "handler must be callable");
        }
        inc_ref_bits(_py, handler_bits);
        let previous = runtime_state(_py)
            .codec_error_handlers
            .lock()
            .unwrap()
            .insert(name, handler_bits);
        if let Some(previous) = previous {
            dec_ref_bits(_py, previous);
        }
        MoltObject::none().bits()
    })
}

/// Lookup an error handler by name. Raises LookupError if unknown.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_lookup_error(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let tn = type_name(_py, name_obj);
            let msg = format!("lookup_error() argument must be str, not {tn}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let handler = runtime_state(_py)
            .codec_error_handlers
            .lock()
            .unwrap()
            .get(&name)
            .copied();
        match handler {
            Some(bits) => {
                inc_ref_bits(_py, bits);
                bits
            }
            None => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
        }
    })
}

pub(crate) fn codecs_clear_error_handlers(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    let handlers = std::mem::take(
        &mut *state
            .codec_error_handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    let changed = !handlers.is_empty();
    for bits in handlers.into_values() {
        dec_ref_bits(_py, bits);
    }
    changed
}

#[cfg(test)]
mod error_handler_tests {
    use super::*;

    #[test]
    fn charmap_result_tuple_is_the_only_payload_owner() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Spaces prevent string interning; the bytes payload is nonempty.
            let payload = b"charmap result ownership";
            for tuple in [
                charmap_decode_tuple(py, payload, payload.len()),
                charmap_encode_tuple(py, payload, payload.len()),
            ] {
                let values = decode_value_list(obj_from_bits(tuple)).unwrap();
                assert_eq!(values.len(), 2);
                assert_eq!(to_i64(obj_from_bits(values[1])), Some(payload.len() as i64));
                let child = values[0];
                let ptr = obj_from_bits(child).as_ptr().unwrap();
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() },
                    1
                );
                inc_ref_bits(py, child);
                dec_ref_bits(py, tuple);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() },
                    1,
                    "retiring the tuple must leave only the independently held payload"
                );
                dec_ref_bits(py, child);
            }
            assert!(!exception_pending(py));
        });
    }

    extern "C" fn test_error_handler(_error_bits: u64) -> i64 {
        MoltObject::none().bits() as i64
    }

    #[test]
    fn error_handler_registry_is_runtime_wide_and_owns_exactly_one_reference() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let (handler_bits, handler_address, baseline) = crate::with_gil_entry_nopanic!(_py, {
            let handler_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(test_error_handler as *const ()),
                1,
            );
            let handler_bits = MoltObject::from_ptr(handler_ptr).bits();
            let name_ptr = alloc_string(_py, b"molt-test-runtime-wide-handler");
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let baseline =
                unsafe { (*crate::header_from_obj_ptr(handler_ptr)).ref_count_snapshot() };
            assert_eq!(
                molt_codecs_register_error(name_bits, handler_bits),
                MoltObject::none().bits()
            );
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(handler_ptr)).ref_count_snapshot() },
                baseline + 1
            );
            let looked_up = molt_codecs_lookup_error(name_bits);
            assert_eq!(looked_up, handler_bits);
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(handler_ptr)).ref_count_snapshot() },
                baseline + 2,
                "lookup must return a new owned reference"
            );
            dec_ref_bits(_py, looked_up);
            dec_ref_bits(_py, name_bits);
            (handler_bits, handler_ptr as usize, baseline)
        });

        #[cfg(not(target_arch = "wasm32"))]
        {
            let origin = crate::with_gil_entry_nopanic!(py, {
                crate::state::runtime_state::RuntimeWorkerOrigin::new(runtime_state(py))
            });
            let worker = std::thread::spawn(move || {
                crate::state::run_runtime_worker(origin, || {
                    crate::with_gil_entry_nopanic!(_py, {
                        let name_ptr = alloc_string(_py, b"molt-test-runtime-wide-handler");
                        let name_bits = MoltObject::from_ptr(name_ptr).bits();
                        let looked_up = molt_codecs_lookup_error(name_bits);
                        assert_eq!(looked_up, handler_bits);
                        dec_ref_bits(_py, looked_up);
                        dec_ref_bits(_py, name_bits);
                    });
                });
            });
            worker.join().unwrap();
        }

        crate::with_gil_entry_nopanic!(_py, {
            let removed = runtime_state(_py)
                .codec_error_handlers
                .lock()
                .unwrap()
                .remove("molt-test-runtime-wide-handler")
                .expect("registered handler");
            dec_ref_bits(_py, removed);
            assert_eq!(
                unsafe {
                    (*crate::header_from_obj_ptr(handler_address as *mut u8)).ref_count_snapshot()
                },
                baseline,
                "registry removal must release exactly its one owned reference"
            );
            dec_ref_bits(_py, handler_bits);
        });
    }
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(encoding) = string_obj_to_owned(obj_from_bits(encoding_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "encoding must be str");
        };
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());

        // Validate the codec without eagerly rejecting an unused error handler.
        if let Err(error) = encode_string_with_errors(&[], &encoding, Some(&errors)) {
            return error.raise(_py, MoltObject::none().bits());
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = inc_enc_id_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let input_obj = obj_from_bits(input_bits);
        // The argument stays owned through this call. Bound the lossless
        // borrow here so input type validation precedes other observations.
        let Some(result) = (unsafe {
            crate::object::ops_format::with_string_bytes(input_obj, |input_bytes| {
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

                let result = encode_string_with_errors(input_bytes, &encoding, Some(&errors));
                match result {
                    Ok(bytes) => {
                        let _ = is_final; // stateless encodings: no pending
                        let ptr = alloc_bytes(_py, &bytes);
                        if ptr.is_null() {
                            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                        }
                        MoltObject::from_ptr(ptr).bits()
                    }
                    Err(error) => error.raise(_py, input_bits),
                }
            })
        }) else {
            return raise_exception::<_>(_py, "TypeError", "input must be str");
        };
        result
    })
}

/// Reset incremental encoder state. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_encoder_reset(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            Err(OpsDecodeTextError::Failure(_, _, _)) => {}
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
    crate::with_gil_entry_nopanic!(_py, {
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
        // Match BufferedIncrementalDecoder: combine the owned prefix and input
        // before final.__bool__ can mutate the input or reenter this decoder.
        // No borrowed Python buffer or registry borrow crosses that callback.
        let captured = INC_DEC_HANDLES.with(|h| {
            let states = h.borrow();
            states.get(&id).map(|state| {
                let mut combined = Vec::with_capacity(state.buffer.len() + input_bytes.len());
                combined.extend_from_slice(&state.buffer);
                combined.extend_from_slice(input_bytes);
                (state.encoding.clone(), state.errors.clone(), combined)
            })
        });
        let Some((encoding, errors, decode_input)) = captured else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid decoder handle");
        };
        let is_final = is_truthy(_py, obj_from_bits(final_bits));
        if exception_pending(_py) {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }

        if crate::object::ops_encoding::normalize_encoding(&encoding).is_some_and(|kind| {
            kind.runtime_class() == crate::object::ops_encoding::CodecRuntimeClass::Utf8
        }) {
            return match crate::object::ops_encoding::decode_utf8_incremental(
                &decode_input,
                &errors,
                is_final,
            ) {
                Ok((text_bytes, consumed)) => {
                    let ptr = alloc_string(_py, &text_bytes);
                    if ptr.is_null() {
                        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(
                        );
                    }
                    INC_DEC_HANDLES.with(|h| {
                        if let Some(state) = h.borrow_mut().get_mut(&id) {
                            state.buffer = decode_input[consumed..].to_vec();
                        }
                    });
                    MoltObject::from_ptr(ptr).bits()
                }
                Err(error) => error.raise(_py, &decode_input),
            };
        }

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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(OpsDecodeTextError::Failure(OpsDecodeFailure::Byte { pos, .. }, _, _))
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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(error) => error.raise(_py, &decode_input),
        }
    })
}

/// Reset incremental decoder buffer. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_incremental_decoder_reset(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            return MoltObject::none().bits();
        }
        let Some(raw_ptr) = raw_obj.as_ptr() else {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(error) => error.raise(_py, &combined),
        }
    })
}

/// Read one line from the stream. Returns str.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_reader_readline(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            return MoltObject::none().bits();
        }
        let Some(raw_ptr) = raw_obj.as_ptr() else {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(error) => error.raise(_py, &raw_vec),
        }
    })
}

/// Drop stream reader handle. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_stream_reader_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = stream_rdr_id(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        STREAM_RDR_HANDLES.with(|h| h.borrow_mut().remove(&id));
        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// BOM constants
// ─────────────────────────────────────────────────────────────────────────────

/// Return b'\xef\xbb\xbf' (UTF-8 BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf8() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_bytes(_py, &[0xEF, 0xBB, 0xBF]);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\xff\xfe' (UTF-16-LE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf16_le() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_bytes(_py, &[0xFF, 0xFE]);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\xfe\xff' (UTF-16-BE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf16_be() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_bytes(_py, &[0xFE, 0xFF]);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\xff\xfe\x00\x00' (UTF-32-LE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf32_le() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_bytes(_py, &[0xFF, 0xFE, 0x00, 0x00]);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return b'\x00\x00\xfe\xff' (UTF-32-BE BOM).
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_bom_utf32_be() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_bytes(_py, &[0x00, 0x00, 0xFE, 0xFF]);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
    crate::with_gil_entry_nopanic!(_py, {
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
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

/// Decode bytes using a character mapping.
/// `input_bits`: bytes to decode
/// `errors_bits`: error mode string ("strict", "ignore", "replace")
/// `mapping_bits`: dict or None (None = latin-1 identity)
/// Returns a (str, int) tuple of (decoded_text, bytes_consumed).
fn charmap_decode_tuple(_py: &PyToken<'_>, decoded: &[u8], consumed: usize) -> u64 {
    let str_ptr = alloc_string(_py, decoded);
    if str_ptr.is_null() {
        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
    }
    let text = crate::builtins::exceptions::ExceptionValue::adopt(
        _py,
        MoltObject::from_ptr(str_ptr).bits(),
    );
    let len_bits = MoltObject::from_int(consumed as i64).bits();
    let elems = [text.bits(), len_bits];
    let tuple_ptr = crate::alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn charmap_decode_undefined(
    _py: &PyToken<'_>,
    input: &[u8],
    out: &mut Vec<u8>,
    errors: &str,
    byte: u8,
    pos: usize,
) -> Result<(), u64> {
    match errors {
        "ignore" => Ok(()),
        "replace" => {
            push_wtf8_codepoint(out, 0xFFFD);
            Ok(())
        }
        "backslashreplace" => {
            push_backslash_bytes_vec(out, &[byte]);
            Ok(())
        }
        "surrogateescape" => {
            push_wtf8_codepoint(out, 0xDC00 + byte as u32);
            Ok(())
        }
        "xmlcharrefreplace" | "namereplace" => Err(raise_exception::<_>(
            _py,
            "TypeError",
            "don't know how to handle UnicodeDecodeError in error callback",
        )),
        "strict" | "surrogatepass" => Err(OpsDecodeTextError::Failure(
            OpsDecodeFailure::Byte {
                pos,
                byte,
                message: "character maps to <undefined>",
            },
            "charmap".to_owned(),
            0,
        )
        .raise(_py, input)),
        other => {
            let msg = format!("unknown error handler name '{other}'");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_charmap_decode(
    input_bits: u64,
    errors_bits: u64,
    mapping_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

        if mapping_obj.is_none() {
            let mut decoded = Vec::with_capacity(input_vec.len());
            for &byte in &input_vec {
                push_wtf8_codepoint(&mut decoded, byte as u32);
            }
            return charmap_decode_tuple(_py, &decoded, input_vec.len());
        }

        if let Some(table) = string_obj_to_owned(mapping_obj) {
            let table_chars: Vec<char> = table.chars().collect();
            let mut out = Vec::with_capacity(input_vec.len());
            for (pos, &byte) in input_vec.iter().enumerate() {
                match table_chars.get(byte as usize).copied() {
                    Some('\u{FFFE}') | None => {
                        if let Err(bits) =
                            charmap_decode_undefined(_py, &input_vec, &mut out, &errors, byte, pos)
                        {
                            return bits;
                        }
                    }
                    Some(ch) => push_wtf8_codepoint(&mut out, ch as u32),
                }
            }
            return charmap_decode_tuple(_py, &out, input_vec.len());
        }

        let map_ptr = match mapping_obj.as_ptr() {
            Some(p) => p,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "mapping must be a dict, str, bytes, or None",
                );
            }
        };

        if let Some(mapping_bytes) = unsafe { bytes_like_slice(map_ptr) } {
            let mapping_vec = mapping_bytes.to_vec();
            let mut out = Vec::with_capacity(input_vec.len());
            for (pos, &byte) in input_vec.iter().enumerate() {
                match mapping_vec.get(byte as usize).copied() {
                    Some(mapped) => push_wtf8_codepoint(&mut out, mapped as u32),
                    None => {
                        if let Err(bits) =
                            charmap_decode_undefined(_py, &input_vec, &mut out, &errors, byte, pos)
                        {
                            return bits;
                        }
                    }
                }
            }
            return charmap_decode_tuple(_py, &out, input_vec.len());
        }

        if unsafe { object_type_id(map_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "mapping must be a dict, str, bytes, or None",
            );
        }

        let mut out = Vec::with_capacity(input_vec.len());
        for (pos, &b) in input_vec.iter().enumerate() {
            let key_bits = MoltObject::from_int(b as i64).bits();
            let mapped = match unsafe { crate::dict_get_in_place(_py, map_ptr, key_bits) } {
                Ok(mapped) => mapped,
                Err(()) => return MoltObject::none().bits(),
            };
            match mapped {
                Some(val) => {
                    let val_obj = obj_from_bits(val);
                    if val_obj.is_none() {
                        if let Err(bits) =
                            charmap_decode_undefined(_py, &input_vec, &mut out, &errors, b, pos)
                        {
                            return bits;
                        }
                    } else if let Some(ptr) = val_obj.as_ptr() {
                        match unsafe { object_type_id(ptr) } {
                            TYPE_ID_STRING => {
                                if let Some(s) = string_obj_to_owned(val_obj) {
                                    out.extend_from_slice(s.as_bytes());
                                }
                            }
                            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "character mapping must return integer, None or str",
                                );
                            }
                            _ => {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "character mapping must return integer, None or str",
                                );
                            }
                        }
                    } else if let Some(i) = crate::to_i64(val_obj) {
                        if !(0..=0x10FFFF).contains(&i) || (0xD800..=0xDFFF).contains(&(i as u32)) {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "character mapping must be in range(0x110000)",
                            );
                        }
                        push_wtf8_codepoint(&mut out, i as u32);
                    } else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "character mapping must return integer, None or str",
                        );
                    }
                }
                None => {
                    if let Err(bits) =
                        charmap_decode_undefined(_py, &input_vec, &mut out, &errors, b, pos)
                    {
                        return bits;
                    }
                }
            }
        }
        charmap_decode_tuple(_py, &out, input_vec.len())
    })
}

/// Encode a string using a character mapping.
/// `input_bits`: str to encode
/// `errors_bits`: error mode string
/// `mapping_bits`: dict or None (None = latin-1 identity)
/// Returns a (bytes, int) tuple of (encoded_bytes, chars_consumed).
fn charmap_encode_tuple(_py: &PyToken<'_>, encoded: &[u8], consumed: usize) -> u64 {
    let bytes_ptr = alloc_bytes(_py, encoded);
    if bytes_ptr.is_null() {
        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
    }
    let bytes = crate::builtins::exceptions::ExceptionValue::adopt(
        _py,
        MoltObject::from_ptr(bytes_ptr).bits(),
    );
    let len_bits = MoltObject::from_int(consumed as i64).bits();
    let elems = [bytes.bits(), len_bits];
    let tuple_ptr = crate::alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn charmap_append_encode_value(
    _py: &PyToken<'_>,
    out: &mut Vec<u8>,
    value_bits: u64,
) -> Result<bool, u64> {
    let value_obj = obj_from_bits(value_bits);
    if value_obj.is_none() {
        return Ok(false);
    }
    if let Some(ptr) = value_obj.as_ptr() {
        match unsafe { object_type_id(ptr) } {
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                if let Some(slice) = unsafe { bytes_like_slice(ptr) } {
                    out.extend_from_slice(slice);
                }
                return Ok(true);
            }
            TYPE_ID_STRING => {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "character mapping must return integer, bytes or None, not str",
                ));
            }
            _ => {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "character mapping must return integer, bytes or None",
                ));
            }
        }
    }
    if let Some(i) = crate::to_i64(value_obj) {
        if !(0..=255).contains(&i) {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "character mapping must be in range(256)",
            ));
        }
        out.push(i as u8);
        return Ok(true);
    }
    Err(raise_exception::<_>(
        _py,
        "TypeError",
        "character mapping must return integer, bytes or None",
    ))
}

fn charmap_lookup_encode_char(
    _py: &PyToken<'_>,
    map_ptr: *mut u8,
    out: &mut Vec<u8>,
    code: u32,
) -> Result<bool, u64> {
    let key = MoltObject::from_int(code as i64).bits();
    let Some(value_bits) = (unsafe { crate::dict_get_in_place(_py, map_ptr, key) })
        .map_err(|()| MoltObject::none().bits())?
    else {
        return Ok(false);
    };
    charmap_append_encode_value(_py, out, value_bits)
}

fn charmap_encode_replacement(
    _py: &PyToken<'_>,
    input_bits: u64,
    map_ptr: *mut u8,
    out: &mut Vec<u8>,
    replacement: &str,
    pos: usize,
) -> Result<(), u64> {
    for repl in replacement.chars() {
        if !charmap_lookup_encode_char(_py, map_ptr, out, repl as u32)? {
            return Err(charmap_encode_undefined(_py, input_bits, pos));
        }
    }
    Ok(())
}

fn charmap_encode_undefined(_py: &PyToken<'_>, input_bits: u64, pos: usize) -> u64 {
    crate::object::ops_encoding::EncodeError::InvalidSequence {
        encoding: "charmap",
        start: pos,
        end: pos + 1,
        reason: "character maps to <undefined>",
    }
    .raise(_py, input_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_charmap_encode(
    input_bits: u64,
    errors_bits: u64,
    mapping_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let input_obj = obj_from_bits(input_bits);
        let errors =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_owned());
        let mapping_obj = obj_from_bits(mapping_bits);

        // A missing mapping uses the ordinal codec and the original WTF-8
        // object. The explicit mapping branch has its own lookup protocol.
        if mapping_obj.is_none() {
            let Some(result) = (unsafe {
                crate::object::ops_format::with_string_bytes(input_obj, |bytes| {
                    encode_string_with_errors(bytes, "latin-1", Some(&errors)).map(|encoded| {
                        let consumed = crate::object::ops_string::wtf8_from_bytes(bytes)
                            .code_points()
                            .count();
                        (encoded, consumed)
                    })
                })
            }) else {
                return raise_exception::<_>(_py, "TypeError", "input must be str");
            };
            return match result {
                Ok((encoded, consumed)) => charmap_encode_tuple(_py, &encoded, consumed),
                Err(error) => error.raise(_py, input_bits),
            };
        }
        let Some(input_str) = string_obj_to_owned(input_obj) else {
            return raise_exception::<_>(_py, "TypeError", "input must be str");
        };

        let mut out: Vec<u8> = Vec::new();
        let Some(map_ptr) = mapping_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "mapping must be a dict or None");
        };
        if unsafe { object_type_id(map_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "mapping must be a dict or None");
        }
        for (idx, ch) in input_str.chars().enumerate() {
            let found = match charmap_lookup_encode_char(_py, map_ptr, &mut out, ch as u32) {
                Ok(found) => found,
                Err(bits) => return bits,
            };
            if found {
                continue;
            }
            match errors.as_str() {
                "ignore" => continue,
                "replace" => {
                    if let Err(bits) =
                        charmap_encode_replacement(_py, input_bits, map_ptr, &mut out, "?", idx)
                    {
                        return bits;
                    }
                }
                "xmlcharrefreplace" => {
                    let mut replacement = Vec::new();
                    push_xmlcharref_ascii(&mut replacement, ch as u32);
                    let replacement = String::from_utf8(replacement).unwrap_or_default();
                    if let Err(bits) = charmap_encode_replacement(
                        _py,
                        input_bits,
                        map_ptr,
                        &mut out,
                        &replacement,
                        idx,
                    ) {
                        return bits;
                    }
                }
                "backslashreplace" => {
                    let replacement = unicode_escape_codepoint(ch as u32);
                    if let Err(bits) = charmap_encode_replacement(
                        _py,
                        input_bits,
                        map_ptr,
                        &mut out,
                        &replacement,
                        idx,
                    ) {
                        return bits;
                    }
                }
                "namereplace" => {
                    let replacement = unicode_name_escape(ch as u32);
                    if let Err(bits) = charmap_encode_replacement(
                        _py,
                        input_bits,
                        map_ptr,
                        &mut out,
                        &replacement,
                        idx,
                    ) {
                        return bits;
                    }
                }
                "strict" | "surrogatepass" | "surrogateescape" => {
                    return charmap_encode_undefined(_py, input_bits, idx);
                }
                other => {
                    let msg = format!("unknown error handler name '{other}'");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
            }
        }
        charmap_encode_tuple(_py, &out, input_str.chars().count())
    })
}

/// Build an identity dict mapping each integer in `range_bits` to itself.
/// Returns a dict.
#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_make_identity_dict(range_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
    crate::with_gil_entry_nopanic!(_py, {
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
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
