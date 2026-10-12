//! FFI bridge shims for `molt-runtime-collections`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The collections crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_runtime_state_get_or_init(
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
pub extern "C" fn __molt_collections_raise_exception(
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
pub extern "C" fn __molt_collections_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_raise_key_error_with_key(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { raise_key_error_with_key::<u64>(_py, key_bits) })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_tuple(
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_list(
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_dict_with_pairs(
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
pub extern "C" fn __molt_collections_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_string_obj_to_owned(
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
pub extern "C" fn __molt_collections_attr_name_bits_from_bytes(
    name_ptr: *const u8,
    name_len: usize,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
        match attr_name_bits_from_bytes(_py, name) {
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
pub extern "C" fn __molt_collections_attr_lookup_ptr_allow_missing(
    obj_ptr: *mut u8,
    attr_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe { attr_lookup_ptr_allow_missing(_py, obj_ptr, attr_bits) } {
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
pub extern "C" fn __molt_collections_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let name = type_name(_py, obj);
        let owned: String = name.into_owned();
        let bytes = owned.into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    })
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_to_i64(bits: u64, out: *mut i64) -> i32 {
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
pub extern "C" fn __molt_collections_index_i64_with_overflow(
    bits: u64,
    type_err_ptr: *const u8,
    type_err_len: usize,
    out: *mut i64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_err = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_err_ptr, type_err_len))
        };
        match index_i64_with_overflow(_py, bits, type_err, None) {
            Some(v) => {
                unsafe {
                    *out = v;
                }
                1
            }
            None => 0,
        }
    })
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_set_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        i32::from(unsafe { dict_set_in_place(_py, dict_ptr, key_bits, val_bits) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_like_bits_from_ptr(
    obj_ptr: *mut u8,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe { dict_like_bits_from_ptr(_py, obj_ptr) } {
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
pub extern "C" fn __molt_collections_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export_dict(_py, ptr, out_ptr, out_len) }
    })
}

// ---------------------------------------------------------------------------
// Object comparison
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_compare_eq(lhs_bits: u64, rhs_bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(lhs_bits);
        let rhs = obj_from_bits(rhs_bits);
        match crate::object::ops_compare::compare_object_eq_bool(_py, lhs, rhs) {
            crate::object::ops_compare::CompareBoolOutcome::True => 1,
            crate::object::ops_compare::CompareBoolOutcome::False
            | crate::object::ops_compare::CompareBoolOutcome::NotComparable => 0,
            crate::object::ops_compare::CompareBoolOutcome::Error => -1,
        }
    })
}

// ---------------------------------------------------------------------------
// Callable helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_call_callable0(call_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::call::dispatch::call_callable0(_py, call_bits) }
    })
}

#[cfg(all(test, feature = "stdlib_collections"))]
mod tests {
    use crate::{TYPE_ID_FUNCTION, dec_ref_bits, obj_from_bits, object_type_id};
    use molt_obj_model::MoltObject;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn init_runtime() {
        INIT.call_once(|| {
            assert_ne!(crate::lifecycle::init(), 0);
        });
        let _ = crate::molt_exception_clear();
    }

    extern "C" fn default_factory_marker() -> i64 {
        MoltObject::from_int(42).bits() as i64
    }

    #[test]
    fn defaultdict_handle_owns_factory_after_caller_releases_local() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                default_factory_marker as *const () as usize as u64,
                0,
            );
            assert!(!func_ptr.is_null());
            assert_eq!(unsafe { object_type_id(func_ptr) }, TYPE_ID_FUNCTION);
            let func_bits = MoltObject::from_ptr(func_ptr).bits();

            let handle_bits =
                crate::molt_runtime_collections::collections_ext::molt_defaultdict_new(func_bits);
            assert!(!obj_from_bits(handle_bits).is_none());

            dec_ref_bits(_py, func_bits);
            assert!(
                obj_from_bits(func_bits).as_ptr().is_some(),
                "defaultdict handle must keep the factory function live"
            );
            let rc_after_caller_drop =
                unsafe { (*crate::object::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
            assert_eq!(
                rc_after_caller_drop, 1,
                "the handle must be the remaining factory owner"
            );

            let key_bits = MoltObject::from_int(7).bits();
            let produced =
                crate::molt_runtime_collections::collections_ext::molt_defaultdict_missing(
                    handle_bits,
                    key_bits,
                );
            assert_eq!(obj_from_bits(produced).as_int(), Some(42));

            let returned_factory =
                crate::molt_runtime_collections::collections_ext::molt_defaultdict_factory(
                    handle_bits,
                );
            assert_eq!(returned_factory, func_bits);
            dec_ref_bits(_py, returned_factory);

            let _ = crate::molt_runtime_collections::collections_ext::molt_defaultdict_drop(
                handle_bits,
            );
        });
    }

    #[test]
    fn optional_attribute_transports_preserve_status_output_and_prior_identity() {
        use crate::builtins::exceptions::ExceptionValue;
        use crate::cpython_abi_hooks::native_test_fixture::NativeType;
        use molt_cpython_abi::abi_types::*;
        use molt_cpython_abi::api::{errors, object, refcount::OwnedPyObject, typeobj};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        use molt_runtime_core::OwnedRuntimeValue;
        use std::cell::Cell;
        use std::ptr;

        thread_local! {
            static MODE: Cell<u8> = const { Cell::new(0) };
            static SEEN: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                MODE.set(0);
                SEEN.set(ptr::null_mut());
                unsafe { errors::PyErr_Clear() };
            }
        }
        unsafe extern "C" fn get(receiver: *mut PyObject, name: *mut PyObject) -> *mut PyObject {
            unsafe {
                if MODE.get() == 2 {
                    return object::PyObject_GenericGetAttr(receiver, name);
                }
                let prior = errors::PyErr_GetRaisedException();
                SEEN.set(prior);
                errors::PyErr_SetRaisedException(prior);
                if MODE.get() == 1 {
                    ptr::null_mut()
                } else {
                    object::Py_NewRef(&raw mut Py_None)
                }
            }
        }

        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let _reset = Reset;
                let prior_ptr = crate::builtins::exceptions::alloc_exception(
                    py,
                    "LookupError",
                    "transport prior",
                );
                assert!(!prior_ptr.is_null());
                let prior = ExceptionValue::adopt(py, MoltObject::from_ptr(prior_ptr).bits());
                let prior_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(prior.bits());
                assert!(!prior_view.is_null());
                let mut class =
                    NativeType::subtype(&raw mut PyBaseObject_Type, c"AttributeTransport");
                class.tp_getattro = Some(get);
                assert_eq!(class.ready(), 0);
                let receiver =
                    OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *class, 0));
                assert!(!receiver.as_ptr().is_null());
                let receiver_bits = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    GLOBAL_BRIDGE
                        .molt_value_for_pyobj(receiver.as_ptr())
                        .unwrap(),
                );
                let name = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::attr_name_bits_from_bytes(py, b"transport_missing").unwrap(),
                );
                let pointer = obj_from_bits(receiver_bits.bits()).as_ptr().unwrap();
                let transports: &[extern "C" fn(*mut u8, u64, *mut u64) -> i32] =
                    &[super::__molt_collections_attr_lookup_ptr_allow_missing];
                for &transport in transports {
                    for runtime_prior in [false, true] {
                        for mode in [0, 1] {
                            MODE.set(mode);
                            if runtime_prior {
                                crate::record_exception(py, prior_ptr);
                            } else {
                                errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                            }
                            let mut out = 0x1234_u64;
                            let status = transport(pointer, name.bits(), &mut out);
                            let result = (status == 1)
                                .then(|| OwnedRuntimeValue::from_owned_bits(py.core_token(), out));
                            let observed =
                                OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                            assert_eq!(
                                SEEN.get(),
                                prior_view,
                                "native slot must observe the exact incoming instance"
                            );
                            assert_eq!(observed.as_ptr(), prior_view);
                            if mode == 0 {
                                assert_eq!(status, 1);
                                assert_eq!(result.unwrap().bits(), MoltObject::none().bits());
                            } else {
                                assert_eq!(
                                    status, -1,
                                    "NULL with the same instance is still failure"
                                );
                                assert_eq!(out, 0x1234, "failure must not write output");
                            }
                        }
                    }
                    MODE.set(2);
                    let mut out = 0x5678_u64;
                    assert_eq!(transport(pointer, name.bits(), &mut out), 0);
                    assert_eq!(out, 0x5678, "absence must not write output");
                    let unexpected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(unexpected.as_ptr().is_null());
                }
            }
        });
    }
}
