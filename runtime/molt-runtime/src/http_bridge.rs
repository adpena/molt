//! FFI bridge shims for `molt-runtime-http`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal function.  The http crate declares matching `extern "C"` imports
//! and they are resolved at link time.

use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_emit};
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_raise_exception(
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
pub extern "C" fn __molt_http_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_clear_exception() {
    crate::with_gil_entry_nopanic!(_py, {
        clear_exception(_py);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_exception_last() -> u64 {
    molt_exception_last()
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_exception_kind_bits(ptr: *mut u8) -> u64 {
    unsafe { crate::builtins::exceptions::exception_kind_bits(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_exception_init(self_bits: u64, args_bits: u64) -> u64 {
    let _ = crate::builtins::exceptions::molt_exception_init(self_bits, args_bits);
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_raise(exc_bits: u64) -> u64 {
    crate::molt_raise(exc_bits)
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_alloc_list_with_capacity(
    elems_ptr: *const u64,
    elems_len: usize,
    cap: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list_with_capacity(_py, elems, cap)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_alloc_dict_with_pairs(
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
pub extern "C" fn __molt_http_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_string_obj_to_owned(
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
pub extern "C" fn __molt_http_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_maybe_ptr_from_bits(bits: u64) -> *mut u8 {
    crate::object::maybe_ptr_from_bits(bits).unwrap_or(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_to_i64(bits: u64, out: *mut i64) -> i32 {
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
pub extern "C" fn __molt_http_to_f64(bits: u64, out: *mut f64) -> i32 {
    let obj = obj_from_bits(bits);
    match crate::builtins::numbers::to_f64(obj) {
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
pub extern "C" fn __molt_http_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { seq_vec_ptr(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_dict_get_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            Some(bits) => {
                unsafe {
                    *out = bits;
                }
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_list_insert(
    list_bits: u64,
    index_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::object::ops_list::molt_list_insert(list_bits, index_bits, value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_dict_new(initial_capacity: usize) -> u64 {
    crate::molt_dict_new(initial_capacity as u64)
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_iter(bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_iter_next(iter_bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter_next(iter_bits)
}

// ---------------------------------------------------------------------------
// Attribute / callable helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_attr_name_bits_from_bytes(key_ptr: *const u8, key_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::builtins::attr::attr_name_bits_from_bytes(_py, key).unwrap_or_default()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_call_callable0(call_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::call::dispatch::call_callable0(_py, call_bits) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_call_callable1(call_bits: u64, arg0: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::call::dispatch::call_callable1(_py, call_bits, arg0) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_call_callable2(call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::call::dispatch::call_callable2(_py, call_bits, arg0, arg1) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_call_class_init_with_args(
    class_bits: u64,
    args_ptr: *const u64,
    args_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args = unsafe { std::slice::from_raw_parts(args_ptr, args_len) };
        let class_ptr = obj_from_bits(class_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        unsafe { call_class_init_with_args(_py, class_ptr, args) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_missing_bits() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { missing_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_getattr_builtin(
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    molt_getattr_builtin(obj_bits, name_bits, default_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_is_callable(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if is_truthy(_py, obj_from_bits(molt_is_callable(bits))) {
            1
        } else {
            0
        }
    })
}

// ---------------------------------------------------------------------------
// String formatting / representation helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_format_obj_str(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let s = crate::format_obj_str(_py, obj);
        let bytes = s.into_bytes().into_boxed_slice();
        let len = bytes.len();
        let ptr = Box::into_raw(bytes) as *const u8;
        unsafe {
            *out_ptr = ptr;
            *out_len = len;
        }
        1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_repr_from_obj(bits: u64) -> u64 {
    crate::molt_repr_from_obj(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_str_from_obj(bits: u64) -> u64 {
    crate::molt_str_from_obj(bits)
}

// ---------------------------------------------------------------------------
// Module / object attribute helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_module_import(name_bits: u64) -> u64 {
    crate::molt_module_import(name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_object_setattr(obj_bits: u64, name_bits: u64, value_bits: u64) {
    let _ = crate::molt_object_setattr(obj_bits, name_bits, value_bits);
}

// ---------------------------------------------------------------------------
// Buffer export
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_molt_buffer_export(
    buffer_bits: u64,
    export: *mut crate::BufferExport,
) -> i32 {
    unsafe { crate::molt_buffer_export(buffer_bits, export) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_bytes_like_slice(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return 0;
    };
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
// Capability / environment helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_has_capability(name_ptr: *const u8, name_len: usize) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        let allowed = crate::has_capability(_py, name);
        {
            let decision = if allowed {
                AuditDecision::Allowed
            } else {
                AuditDecision::Denied {
                    reason: format!("missing {name} capability"),
                }
            };
            audit_emit(AuditEvent::new(
                "http.has_capability",
                "http.has_capability",
                AuditArgs::Custom(name.to_string()),
                decision,
                module_path!().to_string(),
            ));
        }
        if allowed { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_env_state_get(
    key_ptr: *const u8,
    key_len: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let key =
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(key_ptr, key_len)) };
    match crate::builtins::platform::env_state_get(key) {
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
// Class / type resolution
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_builtin_classes(name_ptr: *const u8, name_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        let classes = crate::builtin_classes(_py);
        match name {
            "list" => classes.list,
            _ => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_resolve_global_bits(
    module_ptr: *const u8,
    module_len: usize,
    name_ptr: *const u8,
    name_len: usize,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let module = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(module_ptr, module_len))
        };
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        match crate::builtins::functions_pickle::pickle_resolve_global_bits(_py, module, name) {
            Ok(bits) => {
                unsafe {
                    *out = bits;
                }
                1
            }
            Err(_) => 0,
        }
    })
}

// ---------------------------------------------------------------------------
// GIL release guard
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_gil_release_new() -> u64 {
    let guard = Box::new(crate::concurrency::GilReleaseGuard::new());
    Box::into_raw(guard) as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_gil_release_drop(handle: u64) {
    if handle != 0 {
        unsafe {
            let _ = Box::from_raw(handle as *mut crate::concurrency::GilReleaseGuard);
        }
    }
}

// ---------------------------------------------------------------------------
// Type ID constants
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_type_id_module() -> u32 {
    crate::TYPE_ID_MODULE
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_type_id_list() -> u32 {
    crate::TYPE_ID_LIST
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_type_id_tuple() -> u32 {
    crate::TYPE_ID_TUPLE
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_type_id_dict() -> u32 {
    crate::TYPE_ID_DICT
}
