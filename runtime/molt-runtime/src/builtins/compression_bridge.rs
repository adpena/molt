//! Bridge wrappers for molt-runtime-compression.
//!
//! These `#[no_mangle] extern "C"` functions expose `pub(crate)` helpers
//! with C-ABI-safe signatures so that `molt-runtime-compression` can call
//! them via matching `extern "C"` declarations at link time.
//!
//! The caller (compression crate) already holds the GIL via its own
//! `with_gil_entry!` macro. Each wrapper re-acquires a token via the
//! runtime's `with_gil_entry!` (which is reentrant / a no-op if already held).

use crate::*;

// -- Numbers / ints -----------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_int_bits_from_i64(val: i64) -> u64 {
    crate::with_gil_entry!(_py, {
        crate::builtins::numbers::int_bits_from_i64(_py, val)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_to_i64(obj_bits: u64, ok: *mut bool) -> i64 {
    let obj = obj_from_bits(obj_bits);
    match crate::builtins::numbers::to_i64(obj) {
        Some(v) => {
            unsafe {
                *ok = true;
            }
            v
        }
        None => {
            unsafe {
                *ok = false;
            }
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_index_i64_from_obj(
    obj_bits: u64,
    err_ptr: *const u8,
    err_len: usize,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        crate::builtins::numbers::index_i64_from_obj(_py, obj_bits, err)
    })
}

// -- Exceptions ---------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_raise_exception(
    kind_ptr: *const u8,
    kind_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let kind = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(kind_ptr, kind_len))
        };
        let msg =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
        raise_exception::<u64>(_py, kind, msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_exception_pending() -> bool {
    crate::with_gil_entry!(_py, { exception_pending(_py) })
}

// -- Bytes / memory -----------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_bytes_like_slice(ptr: *mut u8, out_len: *mut usize) -> *const u8 {
    match unsafe { bytes_like_slice(ptr) } {
        Some(slice) => {
            unsafe {
                *out_len = slice.len();
            }
            slice.as_ptr()
        }
        None => {
            unsafe {
                *out_len = 0;
            }
            std::ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_bytes_data(ptr: *mut u8) -> *const u8 {
    unsafe { bytes_data(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_bytes_len(ptr: *mut u8) -> usize {
    unsafe { bytes_len(ptr) }
}

// -- Strings ------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_string_obj_to_owned(obj_bits: u64, out_len: *mut usize) -> *mut u8 {
    let obj = obj_from_bits(obj_bits);
    match string_obj_to_owned(obj) {
        Some(s) => {
            let len = s.len();
            unsafe {
                *out_len = len;
            }
            let mut v = s.into_bytes();
            let ptr = v.as_mut_ptr();
            std::mem::forget(v);
            ptr
        }
        None => {
            unsafe {
                *out_len = 0;
            }
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_free_string(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        unsafe {
            drop(Vec::from_raw_parts(ptr, len, len));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_type_name(obj_bits: u64, out_len: *mut usize) -> *const u8 {
    use std::cell::RefCell;
    thread_local! {
        static BUF: RefCell<String> = RefCell::new(String::new());
    }
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let name = type_name(_py, obj);
        BUF.with(|buf| {
            let mut buf = buf.borrow_mut();
            buf.clear();
            buf.push_str(&name);
            unsafe {
                *out_len = buf.len();
            }
            buf.as_ptr()
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

// -- Object allocation --------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_alloc_list(items_ptr: *const u64, items_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let items = unsafe { std::slice::from_raw_parts(items_ptr, items_len) };
        alloc_list(_py, items)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_alloc_tuple(items_ptr: *const u64, items_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let items = unsafe { std::slice::from_raw_parts(items_ptr, items_len) };
        alloc_tuple(_py, items)
    })
}

// -- Object inspection --------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

// -- Pointer registry ---------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_release_ptr(ptr: *mut u8) {
    let _ = release_ptr(ptr);
}
