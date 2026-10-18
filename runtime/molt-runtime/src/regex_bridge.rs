//! FFI bridge shims for `molt-runtime-regex`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The regex crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;
use molt_cpython_abi::native_custody::ByteInput;

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

/// Admit a regex subject once at the Python buffer boundary. Text and exact
/// immutable bytes retain their existing object; mutable and arbitrary
/// exporters become an immutable byte snapshot for the matching operation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_subject_snapshot(subject_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let subject = obj_from_bits(subject_bits);
        let Some(ptr) = subject.as_ptr() else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "expected string or bytes-like object",
            );
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_STRING {
            inc_ref_bits(_py, subject_bits);
            return subject_bits;
        }
        // Exact immutable bytes need no exporter call. Subclasses must go
        // through their actual buffer slot, which may override their storage.
        if type_id == TYPE_ID_BYTES
            && super::object::buffer_exports::is_exact_builtin_storage(_py, subject_bits)
        {
            inc_ref_bits(_py, subject_bits);
            return subject_bits;
        }
        // ByteInput uses the canonical PyBUF_SIMPLE custody and keeps the
        // exporter alive until the snapshot has been allocated.
        let input = match unsafe { ByteInput::acquire(_py, subject_bits) } {
            Ok(input) => input,
            Err(_) => return MoltObject::none().bits(),
        };
        // Read the leased length, then allocate before borrowing the mutable
        // exporter again: allocation may run finalizers, so no buffer slice
        // may remain live across it.
        let len = unsafe { input.bytes(_py).len() };
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
            let bytes = unsafe { input.bytes(_py) };
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

/// Copy one requested byte capture from the original subject. Allocate the
/// destination before borrowing exporter memory so no live slice crosses a
/// possible GC/finalizer boundary; only the capture bytes are copied.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_subject_slice(subject_bits: u64, start: usize, end: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if start > end {
            return MoltObject::none().bits();
        }
        let input = match unsafe { ByteInput::acquire(_py, subject_bits) } {
            Ok(input) => input,
            Err(_) => return MoltObject::none().bits(),
        };
        let len = unsafe { input.bytes(_py).len() };
        if end > len {
            return MoltObject::none().bits();
        }
        let size = end - start;
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
            let bytes = unsafe { input.bytes(_py) };
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
    type_ptr: *const u8,
    type_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_ptr, type_len))
        };
        let msg =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
        raise_exception::<u64>(_py, type_name, msg)
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

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
        }
        None => 0,
    }
}

/// Convert an exact bytes regex pattern to a temporary one-unit-per-byte text
/// representation for the shared parser. This is compile-time-only; subject
/// matching retains borrowed byte storage and never UTF-8-decodes bytes.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_bytes_pattern_to_string(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
            return 0;
        }
        let raw = unsafe { std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr)) };
        let pattern: String = raw.iter().map(|&byte| char::from(byte)).collect();
        let bytes = pattern.into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    })
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
