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

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_clear_exception() {
    crate::with_gil_entry_nopanic!(_py, {
        clear_exception(_py);
    })
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
            crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
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

// ---------------------------------------------------------------------------
// Callable / protocol helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_call_callable1(call_bits: u64, arg0: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { unsafe { call_callable1(_py, call_bits, arg0) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_attr_lookup_ptr_allow_missing(
    ptr: *mut u8,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits: u64 =
            unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) }.unwrap_or_default();
        bits
    })
}

fn unsupported_logging_intern_name(_py: &PyToken<'_>, key: &[u8]) -> u64 {
    let key = String::from_utf8_lossy(key);
    raise_exception::<u64>(
        _py,
        "RuntimeError",
        &format!("molt-runtime-logging requested unsupported interned static name {key:?}"),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_logging_intern_static_name(key_ptr: *const u8, key_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::cache::intern_bridge_write_name(_py, key)
            .unwrap_or_else(|| unsupported_logging_intern_name(_py, key))
    })
}

#[cfg(test)]
mod tests {
    use super::__molt_logging_intern_static_name;
    use crate::{clear_exception, exception_pending, runtime_state};
    use std::sync::atomic::Ordering;

    #[test]
    fn logging_bridge_interns_write_name_in_runtime_state() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"write";
            let bits = __molt_logging_intern_static_name(key.as_ptr(), key.len());
            assert!(!exception_pending(_py));
            assert_eq!(
                runtime_state(_py)
                    .interned
                    .write_name
                    .load(Ordering::Acquire),
                bits
            );
        });
    }

    #[test]
    fn logging_bridge_rejects_unknown_intern_name() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"flush";
            let _ = __molt_logging_intern_static_name(key.as_ptr(), key.len());
            assert!(exception_pending(_py));
            clear_exception(_py);
        });
    }
}
