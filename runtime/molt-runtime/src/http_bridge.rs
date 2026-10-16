//! FFI bridge shims for `molt-runtime-http`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal function.  The http crate declares matching `extern "C"` imports
//! and they are resolved at link time.

use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_emit};
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;
use num_bigint::{BigInt, Sign};

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
pub extern "C" fn __molt_http_clear_attribute_error_if_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if crate::builtins::attr::clear_attribute_error_if_pending(_py) {
            1
        } else {
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_pending_exception_matches_builtin(
    name_ptr: *const u8,
    name_len: usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if !exception_pending(py) {
            return 0;
        }
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        // Public exception_last may substitute an outer handled exception and
        // consume the pending error. HTTP dispatch always inspects raised state.
        let pending = crate::builtins::exceptions::ExceptionValue::adopt(
            py,
            crate::builtins::exceptions::molt_exception_last_pending(),
        );
        i32::from(crate::builtins::exceptions::exception_matches_builtin_name(
            py,
            pending.bits(),
            name,
        ))
    })
}

/// Synchronous projection of the canonical raised transaction and handler
/// stack. The leaf owns only its stack-local closure, never exception storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __molt_http_with_exception_scope(
    restore_on_success: i32,
    callback: unsafe extern "C" fn(*mut std::ffi::c_void) -> i32,
    context: *mut std::ffi::c_void,
) {
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::{
            ExceptionStackScope, ExceptionValue, exception_context_set,
            molt_exception_last_pending, with_saved_raised_exception,
        };

        let pending = ExceptionValue::adopt(py, molt_exception_last_pending());
        with_saved_raised_exception(py, || {
            let handler = if obj_from_bits(pending.bits()).is_none() {
                None
            } else {
                let handler = ExceptionStackScope::push(py);
                exception_context_set(py, pending.bits());
                Some(handler)
            };
            // The leaf catches its unwind before returning across C. Negative
            // status restores the incoming transaction before it resumes there.
            let status = unsafe { callback(context) };
            if status < 0 {
                unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
            }
            molt_cpython_abi::api::errors::with_preserved_error(|| drop(handler));
            status < 0 || (status > 0 && restore_on_success != 0)
        });
    })
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
            unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
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

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_index_bigint_from_obj(
    obj_bits: u64,
    err_ptr: *const u8,
    err_len: usize,
    out_sign: *mut i32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        match crate::builtins::numbers::index_bigint_from_obj(_py, obj_bits, err) {
            Some(value) => {
                let (sign, bytes) = value.to_bytes_be();
                let sign_i32 = match sign {
                    Sign::Minus => -1i32,
                    Sign::NoSign => 0i32,
                    Sign::Plus => 1i32,
                };
                let boxed = bytes.into_boxed_slice();
                let ok = unsafe {
                    crate::resource::bridge_buffer::export_u8_box(boxed, out_ptr, out_len)
                };
                if ok == 0 {
                    return 0;
                }
                unsafe {
                    *out_sign = sign_i32;
                }
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_int_bits_from_bigint(
    sign: i32,
    data_ptr: *const u8,
    data_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sign = match sign {
            -1 => Sign::Minus,
            0 => Sign::NoSign,
            _ => Sign::Plus,
        };
        let bytes = if data_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data_ptr, data_len) }
        };
        let value = BigInt::from_bytes_be(sign, bytes);
        crate::builtins::numbers::int_bits_from_bigint(_py, value)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __molt_http_float_from_object_value(val_bits: u64, out: *mut f64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let result = crate::molt_float_from_obj(val_bits);
        let _owner = obj_from_bits(result).as_ptr().map(crate::PtrDropGuard::new);
        if exception_pending(py) {
            return 0;
        }
        let value = crate::c_api::molt_float_as_f64(result);
        if exception_pending(py) {
            return 0;
        }
        unsafe { *out = value };
        1
    })
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_dict_get_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            Ok(Some(bits)) => {
                unsafe {
                    *out = bits;
                }
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
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
        let bytes = match crate::object::ops_format::format_obj_str_output(_py, obj)
            .into_bytes_result(_py)
        {
            Ok(bytes) => bytes.into_boxed_slice(),
            Err(error) => {
                error.raise::<()>(_py);
                return 0;
            }
        };
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
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
pub extern "C" fn __molt_http_molt_set_attr_name(obj_bits: u64, name_bits: u64, value_bits: u64) {
    let _ = crate::molt_set_attr_name(obj_bits, name_bits, value_bits);
}

// ---------------------------------------------------------------------------
// Buffer export
// ---------------------------------------------------------------------------

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
pub extern "C" fn __molt_http_require_operation(
    name_ptr: *const u8,
    name_len: usize,
    environment: i32,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        // The extracted caller passes the generated OperationId's static name.
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        let Some(operation) = crate::OperationId::from_name(name) else {
            crate::raise_exception::<u64>(py, "ValueError", "unknown HTTP host operation");
            return 0;
        };
        let extra: &[&'static str] = if environment != 0 { &["env.read"] } else { &[] };
        i32::from(
            crate::require_operation_with_capabilities::<u64>(
                py,
                operation,
                AuditArgs::None,
                extra,
            )
            .is_ok(),
        )
    })
}

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
            Err(bits) => {
                unsafe {
                    *out = bits;
                }
                -1
            }
        }
    })
}

// ---------------------------------------------------------------------------
// GIL release guard
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_http_gil_release_new() -> u64 {
    crate::concurrency::GilReleaseGuard::suspend().into_encoded_state()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __molt_http_gil_release_drop(handle: u64) {
    if handle != 0 {
        // SAFETY: the HTTP guard is !Send/!Sync and consumes its unmatched
        // same-thread encoded custody exactly once.
        drop(unsafe { crate::concurrency::GilReleaseGuard::from_encoded_state(handle) });
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

#[cfg(test)]
mod scalar_conversion_tests {
    use super::*;

    #[test]
    fn http_float_conversion_reads_foreign_storage_and_preserves_text_admission() {
        use molt_cpython_abi::abi_types::{PyExc_ValueError, PyFloat_Type, PyFloatObject};
        use molt_cpython_abi::api::{errors, object, refcount::OwnedPyObject, typeobj};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let native = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                    &raw mut PyFloat_Type,
                    0,
                ));
                assert!(!native.as_ptr().is_null());
                assert!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(native.as_ptr())
                        .is_none()
                );
                (*native.as_ptr().cast::<PyFloatObject>()).ob_fval = 4.25;
                let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap();
                let _owner = crate::PtrDropGuard::preserving(ptr_from_bits(bits));
                assert_eq!(
                    crate::object::object_type_id(ptr_from_bits(bits)),
                    crate::TYPE_ID_FOREIGN
                );
                let refs = (*native.as_ptr()).ob_refcnt;
                let mut value = -99.0;
                assert_eq!(__molt_http_float_from_object_value(bits, &mut value), 1);
                assert_eq!(value, 4.25);
                assert!(!exception_pending(py));
                assert_eq!((*native.as_ptr()).ob_refcnt, refs);
                for (text, expected) in [(b"2.5".as_slice(), Some(2.5)), (b"bad".as_slice(), None)]
                {
                    let ptr = alloc_string(py, text);
                    let _text_owner = crate::PtrDropGuard::preserving(ptr);
                    assert!(!ptr.is_null());
                    value = -99.0;
                    let ok = __molt_http_float_from_object_value(
                        MoltObject::from_ptr(ptr).bits(),
                        &mut value,
                    );
                    if let Some(expected) = expected {
                        assert_eq!(ok, 1);
                        assert_eq!(value, expected);
                        assert!(!exception_pending(py));
                    } else {
                        let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(ok, 0);
                        assert_eq!(value, -99.0, "failure must not publish an output scalar");
                        assert!(!error.as_ptr().is_null());
                        assert_eq!(object::Py_TYPE(error.as_ptr()), &raw mut PyExc_ValueError);
                    }
                }
            }
        });
    }
}

#[cfg(all(test, feature = "stdlib_http", not(target_arch = "wasm32")))]
mod request_serialization_tests {
    use crate::builtins::exceptions::{ExceptionValue, exception_type_bits_from_name};
    use crate::*;
    use molt_runtime_http::functions_http::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn header_predicates_and_final_wire_use_the_real_connection_owner() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("HTTP witness accept failed: {error}"),
                }
            };
            // The listener is non-blocking; an accepted stream inherits that on
            // BSD and macOS but not on Linux. Set the mode this thread reads in.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut wire = Vec::new();
            while !wire.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                wire.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            wire
        });
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
            "net.socket,net.resolve,net.connect",
            |py| {
                let string = |value: &str| {
                    let ptr = alloc_string(py, value.as_bytes());
                    assert!(!ptr.is_null());
                    ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits())
                };
                let bytes = |value: &[u8]| {
                    let ptr = alloc_bytes(py, value);
                    assert!(!ptr.is_null());
                    ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits())
                };
                for invalid in [b"a\nb".as_slice(), b"a\rb", b"a\r\nInjected: yes", b"a\n"] {
                    let value = bytes(invalid);
                    assert_eq!(
                        molt_http_client_validate_header_value(value.bits()),
                        MoltObject::none().bits()
                    );
                    let raised =
                        crate::builtins::exceptions::thread_last_exception_bits_noinc(py).unwrap();
                    assert_eq!(
                        crate::type_of_bits(py, raised),
                        exception_type_bits_from_name(py, "ValueError")
                    );
                    clear_exception(py);
                }
                let name = bytes(b"\x0bX");
                molt_http_client_validate_header_name(name.bits());
                assert!(exception_pending(py));
                clear_exception(py);
                let value = bytes(b"\xff\r\n\tvalue");
                molt_http_client_validate_header_value(value.bits());
                assert!(!exception_pending(py));
                let host = string("127.0.0.1");
                let method = string("GET");
                let path = string("/");
                let handle = molt_http_client_connection_new(
                    host.bits(),
                    MoltObject::from_int(i64::from(port)).bits(),
                    MoltObject::from_float(5.0).bits(),
                );
                assert!(!exception_pending(py));
                let line = ExceptionValue::adopt(
                    py,
                    molt_http_client_connection_putrequest(handle, method.bits(), path.bits()),
                );
                assert!(!exception_pending(py));
                assert_eq!(
                    unsafe {
                        crate::object::memoryview::bytes_like_slice(
                            MoltObject::from_bits(line.bits()).as_ptr().unwrap(),
                        )
                        .unwrap()
                    },
                    b"GET / HTTP/1.1"
                );
                let wire = bytes(b"GET / HTTP/1.1\r\nX-Test: \xff\r\n\tvalue\r\n\r\n");
                molt_http_client_connection_send(handle, wire.bits());
                assert!(!exception_pending(py));
                let response = molt_http_client_connection_getresponse(handle);
                assert!(!exception_pending(py));
                molt_http_client_response_drop(response);
                molt_http_client_connection_drop(handle);
            },
        );
        assert_eq!(
            server.join().unwrap(),
            b"GET / HTTP/1.1\r\nX-Test: \xff\r\n\tvalue\r\n\r\n"
        );
    }

    static LOCAL_LOOKUPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static METHOD_LOOKUPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn request_get_method(self_bits: u64) -> u64 {
        METHOD_LOOKUPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            let name_ptr = alloc_string(py, b"method");
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name = ExceptionValue::adopt(py, MoltObject::from_ptr(name_ptr).bits());
            // These Requests select an explicit method. Read the initialized
            // receiver through a real bound method, as a Request override does.
            molt_getattr_builtin(self_bits, name.bits(), missing_bits(py))
        })
    }

    fn request_fixture<'a, 'py>(py: &'a PyToken<'py>, name_bits: u64) -> ExceptionValue<'a, 'py> {
        let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "http_request_fixture_get_method",
                request_get_method as *const (),
            ),
            1,
        );
        assert!(!function_ptr.is_null());
        let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function_ptr).bits());
        let key_ptr = alloc_string(py, b"get_method");
        assert!(!key_ptr.is_null());
        let key = ExceptionValue::adopt(py, MoltObject::from_ptr(key_ptr).bits());
        let namespace_ptr = alloc_dict_with_pairs(py, &[key.bits(), function.bits()]);
        assert!(!namespace_ptr.is_null());
        let namespace = ExceptionValue::adopt(py, MoltObject::from_ptr(namespace_ptr).bits());
        let bases_ptr = alloc_tuple(py, &[]);
        assert!(!bases_ptr.is_null());
        let bases = ExceptionValue::adopt(py, MoltObject::from_ptr(bases_ptr).bits());
        let class = ExceptionValue::adopt(
            py,
            crate::builtins::types::molt_type_new(
                builtin_classes(py).type_obj,
                name_bits,
                bases.bits(),
                namespace.bits(),
                MoltObject::none().bits(),
            ),
        );
        assert!(!exception_pending(py));
        let request = ExceptionValue::adopt(py, unsafe { call_callable0(py, class.bits()) });
        assert!(!exception_pending(py));
        request
    }

    extern "C" fn forbidden_local_lookup(_host: u64, _port: u64) -> u64 {
        LOCAL_LOOKUPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<u64>(py, "ValueError", "local lookup reached")
        })
    }

    #[test]
    fn request_admission_precedes_local_callbacks_and_tcp_for_both_public_consumers() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        for (tls, required) in [
            (false, &["net.socket", "net.resolve", "net.connect"][..]),
            (
                true,
                &[
                    "net.socket",
                    "net.resolve",
                    "net.connect",
                    "ssl.connect",
                    "ssl.read",
                    "ssl.write",
                ][..],
            ),
        ] {
            // No grants, each individually missing grant, then the actual allowed callback.
            for missing_index in 0..=required.len() + 1 {
                let grants = required
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &grant)| {
                        (missing_index != 0 && missing_index != index + 1).then_some(grant)
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let missing = required
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &grant)| {
                        (missing_index == 0 || missing_index == index + 1).then_some(grant)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                    &grants,
                    |py| {
                        let string = |text: &str| {
                            let ptr = alloc_string(py, text.as_bytes());
                            assert!(!ptr.is_null());
                            ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits())
                        };
                        let module_name = string("socketserver");
                        let module = ExceptionValue::adopt(py, molt_module_new(module_name.bits()));
                        assert!(!exception_pending(py));
                        let callback_ptr = crate::alloc_function_obj(
                            py,
                            crate::provenance::abi::expose_function_address(
                                forbidden_local_lookup as *const (),
                            ),
                            2,
                        );
                        assert!(!callback_ptr.is_null());
                        unsafe {
                            crate::object::layout::function_set_call_target_ptr(
                                callback_ptr,
                                forbidden_local_lookup as *const (),
                            )
                        };
                        let callback =
                            ExceptionValue::adopt(py, MoltObject::from_ptr(callback_ptr).bits());
                        let lookup = string("_lookup_server");
                        let dictionary = unsafe {
                            crate::object::layout::module_dict_bits(
                                obj_from_bits(module.bits()).as_ptr().unwrap(),
                            )
                        };
                        assert!(unsafe {
                            dict_set_in_place(
                                py,
                                obj_from_bits(dictionary).as_ptr().unwrap(),
                                lookup.bits(),
                                callback.bits(),
                            )
                        });
                        molt_module_cache_set(module_name.bits(), module.bits());
                        assert!(!exception_pending(py));
                        LOCAL_LOOKUPS.store(0, std::sync::atomic::Ordering::SeqCst);
                        METHOD_LOOKUPS.store(0, std::sync::atomic::Ordering::SeqCst);
                        let host = string("127.0.0.1");
                        let method = string("GET");
                        let path = string("/");
                        let handle = if tls {
                            molt_http_client_connection_new_https(
                                host.bits(),
                                MoltObject::from_int(i64::from(port)).bits(),
                                MoltObject::from_float(0.01).bits(),
                            )
                        } else {
                            molt_http_client_connection_new(
                                host.bits(),
                                MoltObject::from_int(i64::from(port)).bits(),
                                MoltObject::from_float(0.01).bits(),
                            )
                        };
                        let line = ExceptionValue::adopt(
                            py,
                            molt_http_client_connection_putrequest(
                                handle,
                                method.bits(),
                                path.bits(),
                            ),
                        );
                        assert!(!exception_pending(py));
                        let ptr = alloc_bytes(py, b"GET / HTTP/1.1\r\n\r\n");
                        assert!(!ptr.is_null());
                        let wire = ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits());
                        molt_http_client_connection_send(handle, wire.bits());
                        assert!(!exception_pending(py));
                        let operation = if tls { "https.request" } else { "http.request" };
                        let expected = if missing.is_empty() {
                            ("ValueError", "local lookup reached".to_string())
                        } else {
                            (
                                "PermissionError",
                                format!("missing [{missing}] capabilities for '{operation}'"),
                            )
                        };
                        let check_error = || {
                            assert!(exception_pending(py));
                            let error = ExceptionValue::adopt(py, molt_exception_last_pending());
                            // Rendering is an ordinary Python operation; retain the error before
                            // clearing its raised state, then inspect the owned exception.
                            clear_exception(py);
                            let message = ExceptionValue::adopt(
                                py,
                                crate::builtins::exceptions::molt_exception_message(error.bits()),
                            );
                            assert!(
                                !exception_pending(py),
                                "exception rendering unexpectedly raised"
                            );
                            assert_eq!(
                                crate::type_name(py, obj_from_bits(error.bits())).as_ref(),
                                expected.0
                            );
                            assert_eq!(
                                string_obj_to_owned(obj_from_bits(message.bits())).unwrap(),
                                expected.1
                            );
                        };
                        molt_http_client_connection_getresponse(handle);
                        check_error();
                        molt_http_client_connection_drop(handle);
                        drop(line);
                        // The real urllib intrinsic consumes the same shared boundary.
                        let request_name = string("http_admission_request");
                        let opener_name = string("http_admission_opener");
                        let request = request_fixture(py, request_name.bits());
                        let opener = ExceptionValue::adopt(py, molt_module_new(opener_name.bits()));
                        let url = string(&format!(
                            "{}://127.0.0.1:{port}/",
                            if tls { "https" } else { "http" }
                        ));
                        let headers_ptr = alloc_dict_with_pairs(py, &[]);
                        assert!(!headers_ptr.is_null());
                        let headers =
                            ExceptionValue::adopt(py, MoltObject::from_ptr(headers_ptr).bits());
                        molt_urllib_request_request_init(
                            request.bits(),
                            url.bits(),
                            MoltObject::none().bits(),
                            headers.bits(),
                            method.bits(),
                        );
                        molt_urllib_request_opener_init(opener.bits());
                        assert!(!exception_pending(py));
                        let scheme = string(if tls { "https" } else { "http" });
                        molt_urllib_request_open(
                            opener.bits(),
                            request.bits(),
                            scheme.bits(),
                            MoltObject::none().bits(),
                        );
                        check_error();
                        assert_eq!(
                            METHOD_LOOKUPS.load(std::sync::atomic::Ordering::SeqCst),
                            usize::from(missing.is_empty()),
                            "admission precedes Request.get_method"
                        );
                        assert_eq!(
                            LOCAL_LOOKUPS.load(std::sync::atomic::Ordering::SeqCst),
                            if missing.is_empty() { 2 } else { 0 }
                        );
                        assert_eq!(
                            listener.accept().unwrap_err().kind(),
                            std::io::ErrorKind::WouldBlock
                        );
                    },
                );
            }
        }
    }

    extern "C" fn redirect_lookup(_host: u64, _port: u64) -> u64 {
        LOCAL_LOOKUPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            let name = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_string(py, b"http_redirect_server")).bits(),
            );
            molt_module_cache_get(name.bits())
        })
    }

    extern "C" fn redirect_dispatch(_request: u64, _timeout: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            MoltObject::from_ptr(alloc_bytes(py,
                b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:1/next\r\nContent-Length: 0\r\n\r\n")).bits()
        })
    }

    #[test]
    fn redirect_reselects_https_admission_before_the_second_local_lookup() {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
            "net.socket,net.resolve,net.connect",
            |py| {
                let string = |text: &str| {
                    let ptr = alloc_string(py, text.as_bytes());
                    assert!(!ptr.is_null());
                    ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits())
                };
                for (name, attribute, address) in [
                    (
                        "socketserver",
                        "_lookup_server",
                        redirect_lookup as *const (),
                    ),
                    (
                        "http_redirect_server",
                        "_dispatch",
                        redirect_dispatch as *const (),
                    ),
                ] {
                    let name = string(name);
                    let module = ExceptionValue::adopt(py, molt_module_new(name.bits()));
                    let function = crate::alloc_function_obj(
                        py,
                        crate::provenance::abi::expose_function_address(address),
                        2,
                    );
                    assert!(!function.is_null());
                    unsafe {
                        crate::object::layout::function_set_call_target_ptr(function, address)
                    };
                    let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits());
                    let attribute = string(attribute);
                    let dict = unsafe {
                        crate::object::layout::module_dict_bits(
                            obj_from_bits(module.bits()).as_ptr().unwrap(),
                        )
                    };
                    assert!(unsafe {
                        dict_set_in_place(
                            py,
                            obj_from_bits(dict).as_ptr().unwrap(),
                            attribute.bits(),
                            function.bits(),
                        )
                    });
                    molt_module_cache_set(name.bits(), module.bits());
                    assert!(!exception_pending(py));
                }
                LOCAL_LOOKUPS.store(0, std::sync::atomic::Ordering::SeqCst);
                METHOD_LOOKUPS.store(0, std::sync::atomic::Ordering::SeqCst);
                let request_name = string("http_redirect_request");
                let opener_name = string("http_redirect_opener");
                let request = request_fixture(py, request_name.bits());
                let opener = ExceptionValue::adopt(py, molt_module_new(opener_name.bits()));
                let url = string("http://127.0.0.1:1/start");
                let method = string("GET");
                let headers = ExceptionValue::adopt(
                    py,
                    MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits(),
                );
                molt_urllib_request_request_init(
                    request.bits(),
                    url.bits(),
                    MoltObject::none().bits(),
                    headers.bits(),
                    method.bits(),
                );
                molt_urllib_request_opener_init(opener.bits());
                assert!(!exception_pending(py));
                let scheme = string("http");
                molt_urllib_request_open(
                    opener.bits(),
                    request.bits(),
                    scheme.bits(),
                    MoltObject::none().bits(),
                );
                assert!(exception_pending(py));
                let error = ExceptionValue::adopt(py, molt_exception_last_pending());
                // Rendering is an ordinary Python operation; retain the error before
                // clearing its raised state, then inspect the owned exception.
                clear_exception(py);
                let message = ExceptionValue::adopt(
                    py,
                    crate::builtins::exceptions::molt_exception_message(error.bits()),
                );
                assert!(
                    !exception_pending(py),
                    "exception rendering unexpectedly raised"
                );
                assert_eq!(
                    crate::type_name(py, obj_from_bits(error.bits())).as_ref(),
                    "PermissionError"
                );
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(message.bits())).unwrap(),
                    "missing [ssl.connect, ssl.read, ssl.write] capabilities for 'https.request'"
                );
                assert_eq!(LOCAL_LOOKUPS.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert_eq!(METHOD_LOOKUPS.load(std::sync::atomic::Ordering::SeqCst), 1);
            },
        );
    }
}
