//! FFI bridge shims for `molt-runtime-logging`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The logging crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_runtime_state_get_or_init(
    key_ptr: *const u8,
    key_len: usize,
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::runtime_extension_state_get_or_init(
            crate::state::runtime_state::runtime_state(_py),
            key,
            init,
            clear,
            drop,
        )
    })
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_raise_exception(
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

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_string_obj_to_owned(
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

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_to_i64(bits: u64, out: *mut i64) -> i32 {
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
