//! FFI bridge shims for `molt-runtime-regex`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The regex crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::BuiltinExc;
use crate::*;
use molt_runtime_capi::{BufferAcquireError, ByteInput};

#[cfg(feature = "stdlib_regex")]
crate::object::native_handle::native_handle_bridge!(
    __molt_regex_pattern_handle_new,
    __molt_regex_pattern_handle_clone,
    molt_runtime_regex::PatternHandle
);
#[cfg(feature = "stdlib_regex")]
crate::object::native_handle::native_handle_bridge!(
    __molt_regex_scanner_handle_new,
    __molt_regex_scanner_handle_clone,
    std::sync::Mutex<molt_runtime_regex::ScannerHandle>
);

/// SRE's getstring role replaces every failed buffer export, including a
/// missing slot, with its own type diagnostic. Runtime projection/allocation
/// preparation happens before that protocol and retains its original error.
fn regex_subject_buffer<T>(
    py: &PyToken<'_>,
    subject_bits: u64,
    result: Result<T, BufferAcquireError>,
) -> Option<T> {
    match result {
        Ok(input) => Some(input),
        Err(BufferAcquireError::Preparation) => None,
        Err(BufferAcquireError::Exporter) => {
            crate::builtins::exceptions::with_saved_raised_exception(py, || {
                let name = crate::object::ops::protocol_type_name(py, subject_bits, 200);
                raise_exception::<()>(
                    py,
                    BuiltinExc::TypeError,
                    &format!("expected string or bytes-like object, got '{name}'"),
                );
                false
            });
            None
        }
    }
}

fn immutable_regex_subject(py: &PyToken<'_>, bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    let type_id = unsafe { object_type_id(ptr) };
    type_id == TYPE_ID_STRING
        || (type_id == TYPE_ID_BYTES
            && crate::object::buffer_exports::is_exact_builtin_storage(py, bits))
}

/// Admit a regex subject once at the Python buffer boundary. Text and exact
/// immutable bytes retain their existing object; mutable and arbitrary
/// exporters become an immutable byte snapshot for the matching operation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_subject_snapshot(subject_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if immutable_regex_subject(_py, subject_bits) {
            inc_ref_bits(_py, subject_bits);
            return subject_bits;
        }
        // ByteInput uses the canonical PyBUF_SIMPLE custody and keeps the
        // exporter alive until the snapshot has been allocated.
        let Some(input) = regex_subject_buffer(_py, subject_bits, unsafe {
            ByteInput::acquire(_py.core_token(), subject_bits)
        }) else {
            return MoltObject::none().bits();
        };
        // Read the leased length, then allocate before borrowing the mutable
        // exporter again: allocation may run finalizers, so no buffer slice
        // may remain live across it.
        let len = unsafe { input.bytes(_py.core_token()).len() };
        let snapshot = if len == 0 {
            // Keep the runtime's canonical empty-bytes singleton.
            alloc_bytes(_py, &[])
        } else {
            crate::object::builders::alloc_inline_bytes_with_len(
                _py,
                len,
                crate::object::builders::InlineBytesKind::Bytes,
            )
        };
        if snapshot.is_null() {
            return MoltObject::none().bits();
        }
        if len != 0 {
            let bytes = unsafe { input.bytes(_py.core_token()) };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    crate::object::layout::InlineBytesStorage::data(snapshot),
                    len,
                );
            }
        }
        MoltObject::from_ptr(snapshot).bits()
    })
}

/// Retain immutable subjects directly; otherwise acquire exactly one SIMPLE
/// export and transfer it to the ordinary traced memoryview storage owner.
/// The Python scanner owns this view until exhaustion, never a GREEN handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_subject_view(subject_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if immutable_regex_subject(_py, subject_bits) {
            inc_ref_bits(_py, subject_bits);
            return subject_bits;
        }
        let Some(lease) = regex_subject_buffer(_py, subject_bits, unsafe {
            molt_runtime_capi::acquire_buffer(subject_bits, molt_runtime_capi::PyBUF_SIMPLE)
        }) else {
            return MoltObject::none().bits();
        };
        let descriptor = match unsafe {
            molt_cpython_abi::api::buffer::descriptor_from_pybuffer(lease.descriptor())
        } {
            Ok(descriptor) => descriptor,
            Err(molt_cpython_abi::api::buffer::InvalidBufferDescriptor) => {
                return raise_exception(
                    _py,
                    BuiltinExc::BufferError,
                    "invalid or indirect memoryview buffer descriptor",
                );
            }
        };
        let format = unsafe { (*lease.descriptor()).format };
        unsafe {
            crate::object::memoryview::from_native_descriptor(_py, &descriptor, format, Some(lease))
        }
    })
}

/// Copy one requested byte capture from the original subject. Allocate the
/// destination before borrowing exporter memory so no live slice crosses a
/// possible GC/finalizer boundary; only the capture bytes are copied.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_subject_slice(subject_bits: u64, start: usize, end: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if start > end {
            return raise_exception::<u64>(_py, BuiltinExc::IndexError, "invalid group span");
        }
        if let Some(subject_ptr) = obj_from_bits(subject_bits).as_ptr()
            && unsafe { object_type_id(subject_ptr) } == TYPE_ID_STRING
        {
            let subject = obj_from_bits(subject_bits);
            let (byte_start, byte_end) = (unsafe {
                crate::object::ops_format::with_string_bytes(subject, |bytes| {
                    let key = Some(subject_ptr as usize);
                    let start = crate::object::ops_string::utf8_char_to_byte_index_cached(
                        _py,
                        bytes,
                        i64::try_from(start).unwrap_or(i64::MAX),
                        key,
                    );
                    let end = crate::object::ops_string::utf8_char_to_byte_index_cached(
                        _py,
                        bytes,
                        i64::try_from(end).unwrap_or(i64::MAX),
                        key,
                    );
                    (start, end)
                })
            })
            .expect(
                "TYPE_ID_STRING must expose its physical UTF-8 surrogatepass storage under the GIL",
            );
            let size = byte_end.saturating_sub(byte_start);
            let ptr = if size == 0 {
                alloc_string(_py, &[])
            } else {
                crate::object::builders::alloc_inline_bytes_with_len(
                    _py,
                    size,
                    crate::object::builders::InlineBytesKind::String,
                )
            };
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            if size != 0 {
                // Allocation may run finalizers. Reacquire the immutable string
                // storage only after it has completed, and never retain this
                // borrow across another runtime call.
                (unsafe {
                    crate::object::ops_format::with_string_bytes(
                        obj_from_bits(subject_bits),
                        |bytes| {
                            std::ptr::copy_nonoverlapping(
                                bytes.as_ptr().add(byte_start),
                                crate::object::layout::InlineBytesStorage::data(ptr),
                                size,
                            );
                        },
                    )
                })
                .expect("retained TYPE_ID_STRING must keep its physical storage");
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let Some(input) = regex_subject_buffer(_py, subject_bits, unsafe {
            ByteInput::acquire(_py.core_token(), subject_bits)
        }) else {
            return MoltObject::none().bits();
        };
        let len = unsafe { input.bytes(_py.core_token()).len() };
        // Match marks belong to the earlier search operation. The source may
        // have shrunk since then; CPython clamps both marks to its current
        // buffer length when each capture is read.
        let start = start.min(len);
        let end = end.min(len);
        let size = end.saturating_sub(start);
        let ptr = if size == 0 {
            alloc_bytes(_py, &[])
        } else {
            crate::object::builders::alloc_inline_bytes_with_len(
                _py,
                size,
                crate::object::builders::InlineBytesKind::Bytes,
            )
        };
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        if size != 0 {
            let bytes = unsafe { input.bytes(_py.core_token()) };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add(start),
                    crate::object::layout::InlineBytesStorage::data(ptr),
                    size,
                );
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_raise_exception(
    kind_code: u16,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let msg = unsafe { std::slice::from_raw_parts(msg_ptr, msg_len) };
        crate::builtins::exceptions::raise_exception_code_bytes(_py, kind_code, msg)
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_dict_with_pairs(
    pairs_ptr: *const u64,
    pairs_len: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

/// Borrow bytes storage for one regex transition. For mutable exporters the
/// argument is the iterator-owned memoryview, which pins the PyBUF_SIMPLE
/// lease; the slice never outlives this call.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_bytes_view(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return 0;
    };
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_BYTES => unsafe {
            *out_ptr = bytes_data(ptr);
            *out_len = bytes_len(ptr);
        },
        TYPE_ID_MEMORYVIEW => {
            let Some(bytes) =
                (unsafe { crate::object::memoryview::bytes_like_slice_checked(ptr).ok() })
            else {
                return 0;
            };
            unsafe {
                *out_ptr = bytes.as_ptr();
                *out_len = bytes.len();
            }
        }
        _ => return 0,
    }
    1
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(v) => {
            unsafe {
                *out = v;
            }
            1
        }
        None => 0,
    }
}

#[cfg(test)]
#[path = "regex_bridge_tests.rs"]
mod tests;

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_dict_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export_dict(_py, ptr, out_ptr, out_len) }
    })
}
