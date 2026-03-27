//! FFI bridge shims for `molt-runtime-logging`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The logging crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;
use std::sync::atomic::{AtomicU64, Ordering};

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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_clear_exception() {
    crate::with_gil_entry!(_py, {
        clear_exception(_py);
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
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

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_dec_ref_bits(bits: u64) {
    crate::with_gil_entry!(_py, {
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

// ---------------------------------------------------------------------------
// Callable / protocol helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_call_callable1(call_bits: u64, arg0: u64) -> u64 {
    crate::with_gil_entry!(_py, { unsafe { call_callable1(_py, call_bits, arg0) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_attr_lookup_ptr_allow_missing(
    ptr: *mut u8,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        match unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) } {
            Some(bits) => bits,
            None => 0,
        }
    })
}

// Interning slot for attribute names used by the logging crate.
// Currently only "write" is interned.
static INTERN_WRITE: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_intern_static_name(key_ptr: *const u8, key_len: usize) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        let slot = &INTERN_WRITE;
        // Fast path: already interned.
        let cached = slot.load(Ordering::Acquire);
        if cached != 0 {
            return cached;
        }
        // Slow path: allocate and cache.
        let ptr = alloc_string(_py, key);
        let bits = if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        };
        match slot.compare_exchange(0, bits, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => bits,
            Err(existing) => {
                if let Some(p) = MoltObject::from_bits(bits).as_ptr() {
                    dec_ref_bits(_py, MoltObject::from_ptr(p).bits());
                }
                existing
            }
        }
    })
}
