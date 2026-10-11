use super::*;
use crate::builtins::exceptions::ExceptionValue;
use crate::cpython_abi_hooks::native_test_fixture::NativeType;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{
    errors, object as cobject, refcount::OwnedPyObject, sequences, typeobj,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_runtime_core::OwnedRuntimeValue;
use std::cell::Cell;
use std::ptr;

thread_local! {
    static FAIL: Cell<bool> = const { Cell::new(false) };
    static SEEN: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackReset;
impl Drop for CallbackReset {
    fn drop(&mut self) {
        FAIL.set(false);
        SEEN.set(ptr::null_mut());
        CALLS.set(0);
        unsafe { errors::PyErr_Clear() };
    }
}

unsafe extern "C" fn mapping_item(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
    CALLS.set(CALLS.get() + 1);
    unsafe {
        // A legal observer transfers and restores the exact incoming instance.
        // Returning NULL deliberately re-raises that same instance; identity
        // comparison cannot substitute for the operation's explicit status.
        let prior = errors::PyErr_GetRaisedException();
        SEEN.set(prior);
        errors::PyErr_SetRaisedException(prior);
        if FAIL.get() {
            ptr::null_mut()
        } else {
            cobject::Py_NewRef(&raw mut Py_None)
        }
    }
}

#[test]
fn native_mapping_slot_status_preserves_prior_and_legitimate_none() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let _reset = CallbackReset;
            let prior_ptr =
                crate::builtins::exceptions::alloc_exception(py, "LookupError", "index prior");
            assert!(!prior_ptr.is_null());
            let prior = ExceptionValue::adopt(py, MoltObject::from_ptr(prior_ptr).bits());
            let prior_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(prior.bits());
            assert!(!prior_view.is_null());
            let mut mapping: PyMappingMethods = std::mem::zeroed();
            mapping.mp_subscript = mapping_item as *const () as *mut _;
            let mut class = NativeType::subtype(&raw mut PyBaseObject_Type, c"NativeIndexStatus");
            class.tp_as_mapping = &raw mut mapping;
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
            assert_eq!(
                object_type_id(obj_from_bits(receiver_bits.bits()).as_ptr().unwrap()),
                TYPE_ID_FOREIGN
            );
            for runtime_prior in [false, true] {
                for legacy_mapping in [false, true] {
                    for fail in [false, true] {
                        FAIL.set(fail);
                        SEEN.set(ptr::null_mut());
                        CALLS.set(0);
                        if runtime_prior {
                            crate::record_exception(py, prior_ptr);
                        } else {
                            errors::PyErr_SetRaisedException(cobject::Py_NewRef(prior_view));
                        }
                        let result = if legacy_mapping {
                            let bits = crate::c_api::PyMapping_GetItemString(
                                receiver_bits.bits(),
                                c"key".as_ptr(),
                            );
                            if bits == 0 { Err(()) } else { Ok(bits) }
                        } else {
                            index_impl(
                                receiver_bits.bits(),
                                MoltObject::from_int(0).bits(),
                                false,
                                true,
                            )
                        };
                        let result = result
                            .map(|bits| OwnedRuntimeValue::from_owned_bits(py.core_token(), bits));
                        let observed =
                            OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(CALLS.get(), 1, "one canonical native mp_subscript call");
                        assert_eq!(SEEN.get(), prior_view, "native slot sees admitted prior");
                        assert_eq!(observed.as_ptr(), prior_view);
                        if fail {
                            assert!(result.is_err());
                        } else {
                            assert_eq!(result.unwrap().bits(), MoltObject::none().bits());
                        }
                    }
                }
            }
        }
    });
}

#[test]
fn managed_and_native_sequence_none_remains_success_with_prior_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let _reset = CallbackReset;
            let prior_ptr =
                crate::builtins::exceptions::alloc_exception(py, "LookupError", "sequence prior");
            assert!(!prior_ptr.is_null());
            let prior = ExceptionValue::adopt(py, MoltObject::from_ptr(prior_ptr).bits());
            let prior_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(prior.bits());
            assert!(!prior_view.is_null());
            let none = MoltObject::none().bits();
            let key_bits = MoltObject::from_int(0).bits();
            let key =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(key_bits));
            assert!(!key.as_ptr().is_null());
            let list = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(alloc_list(py, &[none])).bits(),
            );
            let tuple = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(alloc_tuple(py, &[none])).bits(),
            );
            let dict = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits(),
            );
            assert!(dict_set_in_place(
                py,
                obj_from_bits(dict.bits()).as_ptr().unwrap(),
                key_bits,
                none
            ));
            let native =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyTuple_Type, 1));
            assert!(!native.as_ptr().is_null());
            assert_eq!(
                sequences::PyTuple_SetItem(
                    native.as_ptr(),
                    0,
                    cobject::Py_NewRef(&raw mut Py_None)
                ),
                0
            );
            let native_bits = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap(),
            );
            let flat_int = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(
                    crate::object::builders::alloc_list_int_from_raw_slice(py, &[7]).unwrap(),
                )
                .bits(),
            );
            let flat_bool = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(
                    crate::object::builders::alloc_list_bool_from_raw_slice(py, &[1]).unwrap(),
                )
                .bits(),
            );
            for runtime_prior in [false, true] {
                for receiver in [list.bits(), tuple.bits(), dict.bits(), native_bits.bits()] {
                    let view = OwnedPyObject::from_owned(
                        GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver),
                    );
                    assert!(!view.as_ptr().is_null());
                    if runtime_prior {
                        crate::record_exception(py, prior_ptr);
                    } else {
                        errors::PyErr_SetRaisedException(cobject::Py_NewRef(prior_view));
                    }
                    let result = OwnedPyObject::from_owned(cobject::PyObject_GetItem(
                        view.as_ptr(),
                        key.as_ptr(),
                    ));
                    let observed = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(result.as_ptr(), &raw mut Py_None);
                    assert_eq!(observed.as_ptr(), prior_view);
                }
                for (receiver, expected) in [
                    (flat_int.bits(), MoltObject::from_int(7).bits()),
                    (flat_bool.bits(), MoltObject::from_bool(true).bits()),
                ] {
                    if runtime_prior {
                        crate::record_exception(py, prior_ptr);
                    } else {
                        errors::PyErr_SetRaisedException(cobject::Py_NewRef(prior_view));
                    }
                    let result = index_impl(receiver, key_bits, false, true)
                        .map(|bits| OwnedRuntimeValue::from_owned_bits(py.core_token(), bits));
                    let observed = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(result.unwrap().bits(), expected);
                    assert_eq!(observed.as_ptr(), prior_view);
                }
                // The inherited tuple descriptor uses its physical base slot,
                // separately from the public native dispatch exercised above.
                if runtime_prior {
                    crate::record_exception(py, prior_ptr);
                } else {
                    errors::PyErr_SetRaisedException(cobject::Py_NewRef(prior_view));
                }
                let result = index_impl(native_bits.bits(), key_bits, true, true)
                    .map(|bits| OwnedRuntimeValue::from_owned_bits(py.core_token(), bits));
                let observed = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(result.unwrap().bits(), none);
                assert_eq!(observed.as_ptr(), prior_view);
            }
        }
    });
}
