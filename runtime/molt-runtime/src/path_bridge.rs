//! FFI bridge shims for `molt-runtime-path`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The path crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_emit};
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_exception(
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
pub extern "C" fn __molt_path_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_os_error(
    err_kind: u32,
    err_msg_ptr: *const u8,
    err_msg_len: usize,
    ctx_ptr: *const u8,
    ctx_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let err_msg = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_msg_ptr, err_msg_len))
        };
        let ctx =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ctx_ptr, ctx_len)) };
        let kind = match err_kind {
            0 => std::io::ErrorKind::NotFound,
            1 => std::io::ErrorKind::PermissionDenied,
            2 => std::io::ErrorKind::ConnectionRefused,
            3 => std::io::ErrorKind::ConnectionReset,
            4 => std::io::ErrorKind::ConnectionAborted,
            5 => std::io::ErrorKind::NotConnected,
            6 => std::io::ErrorKind::AddrInUse,
            7 => std::io::ErrorKind::AddrNotAvailable,
            8 => std::io::ErrorKind::BrokenPipe,
            9 => std::io::ErrorKind::AlreadyExists,
            10 => std::io::ErrorKind::WouldBlock,
            11 => std::io::ErrorKind::InvalidInput,
            12 => std::io::ErrorKind::InvalidData,
            13 => std::io::ErrorKind::TimedOut,
            14 => std::io::ErrorKind::WriteZero,
            15 => std::io::ErrorKind::Interrupted,
            16 => std::io::ErrorKind::Unsupported,
            17 => std::io::ErrorKind::UnexpectedEof,
            18 => std::io::ErrorKind::OutOfMemory,
            _ => std::io::ErrorKind::Other,
        };
        let err = std::io::Error::new(kind, err_msg.to_string());
        raise_os_error::<u64>(_py, err, ctx)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_os_error_errno(
    errno: i64,
    ctx_ptr: *const u8,
    ctx_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ctx_ptr, ctx_len)) };
        raise_os_error_errno::<u64>(_py, errno, ctx)
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_dict_with_pairs(
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
pub extern "C" fn __molt_path_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_string_obj_to_owned(
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

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_bytes_like_slice(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    match unsafe { crate::object::memoryview::bytes_like_slice(ptr) } {
        Some(slice) => {
            unsafe {
                *out_ptr = slice.as_ptr();
                *out_len = slice.len();
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
pub extern "C" fn __molt_path_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_to_i64(bits: u64, out: *mut i64) -> i32 {
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

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_to_f64(bits: u64, out: *mut f64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_f64(obj) {
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

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_iter(bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32 {
    let result = crate::object::ops_iter::molt_iter_next(iter_bits);
    let none_bits = MoltObject::none().bits();
    if result == none_bits {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                0 // StopIteration or error
            } else {
                unsafe {
                    *out = result;
                }
                1
            }
        })
    } else {
        unsafe {
            *out = result;
        }
        1
    }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_has_capability(name_ptr: *const u8, name_len: usize) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        let allowed = has_capability(_py, name);
        {
            let decision = if allowed {
                AuditDecision::Allowed
            } else {
                AuditDecision::Denied {
                    reason: format!("missing {name} capability"),
                }
            };
            audit_emit(AuditEvent::new(
                "path.has_capability",
                "path.has_capability",
                AuditArgs::Custom(name.to_string()),
                decision,
                module_path!().to_string(),
            ));
        }
        if allowed { 1 } else { 0 }
    })
}

// ---------------------------------------------------------------------------
// Hash helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_object_hash(bits: u64) -> u64 {
    molt_object_hash(bits)
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_path_from_bits(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match path_from_bits(_py, bits) {
            Ok(path) => {
                let s = path.to_string_lossy().into_owned();
                let bytes = s.into_bytes().into_boxed_slice();
                let len = bytes.len();
                let ptr = Box::into_raw(bytes) as *const u8;
                unsafe {
                    *out_ptr = ptr;
                    *out_len = len;
                }
                1
            }
            Err(msg) => {
                let bytes = msg.into_bytes().into_boxed_slice();
                let len = bytes.len();
                let ptr = Box::into_raw(bytes) as *const u8;
                unsafe {
                    *out_ptr = ptr;
                    *out_len = len;
                }
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let name = crate::object::ops::type_name(_py, obj);
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        let len = bytes.len();
        let ptr = Box::into_raw(bytes) as *const u8;
        unsafe {
            *out_ptr = ptr;
            *out_len = len;
        }
        1
    })
}
