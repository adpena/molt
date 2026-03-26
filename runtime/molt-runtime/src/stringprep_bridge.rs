//! FFI bridge shims for `molt-runtime-stringprep`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The stringprep crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::*;
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_stringprep_raise_exception(
    type_ptr: *const u8,
    type_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_ptr, type_len)) };
        let msg = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
        raise_exception::<u64>(_py, type_name, msg)
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_stringprep_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_stringprep_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            let len = bytes.len();
            let ptr = Box::into_raw(bytes) as *const u8;
            unsafe {
                *out_ptr = ptr;
                *out_len = len;
            }
            1
        }
        None => 0,
    }
}
