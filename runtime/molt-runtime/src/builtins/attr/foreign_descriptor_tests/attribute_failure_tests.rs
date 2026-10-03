//! A native failure must cross the Python boundary once, without becoming a miss.
use super::*;
use std::os::raw::c_int;

#[test]
fn c_attribute_consumers_reject_non_string_names_before_callbacks() {
    use molt_cpython_abi::api::{numbers, object};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.ob_base.ob_base.ob_refcnt = 1;
        kind.tp_name = c"NamedAttributes".as_ptr();
        kind.tp_getattro = Some(attribute_get);
        kind.tp_setattro = Some(attribute_set);
        let mut receiver = Receiver {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            calls: 0,
            malformed: false,
        };
        let name = numbers::PyLong_FromLong(3);
        for operation in 0..4 {
            let target = &raw mut receiver.object;
            match operation {
                0 => assert!(object::PyObject_GetAttr(target, name).is_null()),
                1 => assert!(object::PyObject_GenericGetAttr(target, name).is_null()),
                2 => assert_eq!(object::PyObject_SetAttr(target, name, ptr::null_mut()), -1),
                _ => assert_eq!(
                    object::PyObject_GenericSetAttr(target, name, ptr::null_mut()),
                    -1
                ),
            }
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            crate::clear_exception(&py);
        }
        assert_eq!(receiver.calls, 0);
        refcount::Py_DECREF(name);
    });
}

#[repr(C)]
struct Receiver {
    object: PyObject,
    calls: usize,
    malformed: bool,
}

unsafe extern "C" fn attribute_get(obj: *mut PyObject, _name: *mut PyObject) -> *mut PyObject {
    let receiver = unsafe { &mut *obj.cast::<Receiver>() };
    receiver.calls += 1;
    if !receiver.malformed {
        unsafe {
            errors::PyErr_SetNone((&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast())
        };
    }
    ptr::null_mut()
}

unsafe extern "C" fn attribute_set(
    obj: *mut PyObject,
    _name: *mut PyObject,
    _value: *mut PyObject,
) -> c_int {
    unsafe { attribute_get(obj, ptr::null_mut()) };
    -1
}

unsafe extern "C" fn legacy_get(
    obj: *mut PyObject,
    _name: *const std::os::raw::c_char,
) -> *mut PyObject {
    unsafe { attribute_get(obj, ptr::null_mut()) }
}

#[test]
fn native_attribute_failures_reach_all_public_runtime_consumers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.ob_base.ob_base.ob_refcnt = 1;
        kind.tp_name = c"FailingNativeAttributes".as_ptr();
        kind.tp_getattro = Some(attribute_get);
        kind.tp_setattro = Some(attribute_set);
        let mut receiver = Receiver {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            calls: 0,
            malformed: false,
        };
        let bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut receiver.object)
            .unwrap();
        let name = attr_name_bits_from_bytes(&py, b"field").unwrap();
        let none = MoltObject::none().bits();
        for (legacy, malformed) in [(false, false), (false, true), (true, false), (true, true)] {
            kind.tp_getattro = if legacy { None } else { Some(attribute_get) };
            kind.tp_getattr = if legacy { Some(legacy_get) } else { None };
            receiver.malformed = malformed;
            for operation in 0..4 {
                let before = receiver.calls;
                match operation {
                    0 => {
                        crate::molt_get_attr_name(bits, name);
                    }
                    1 => {
                        crate::molt_get_attr_name_default(bits, name, none);
                    }
                    2 => {
                        crate::molt_set_attr_name(bits, name, none);
                    }
                    _ => {
                        crate::molt_del_attr_name(bits, name);
                    }
                }
                assert_eq!(receiver.calls, before + 1, "callback must not be retried");
                let error =
                    crate::exception_last_bits_noinc(&py).expect("native failure is pending");
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    &py,
                    error,
                    if malformed {
                        "SystemError"
                    } else {
                        "ValueError"
                    }
                ));
                assert!(
                    errors::take_current_error().is_none(),
                    "C error transfers to runtime"
                );
                assert_eq!(
                    errors::PyErr_Occurred(),
                    if malformed {
                        (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
                    } else {
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                    }
                );
                assert!(
                    errors::take_current_error().is_none(),
                    "observing the pending type must not create a C-owned error"
                );
                assert_eq!(crate::exception_last_bits_noinc(&py), Some(error));
                crate::clear_exception(&py);
            }
        }
        kind.tp_setattro = None;
        for delete in [false, true] {
            if delete {
                crate::molt_del_attr_name(bits, name);
            } else {
                crate::molt_set_attr_name(bits, name, none);
            }
            let error = crate::exception_last_bits_noinc(&py).expect("missing setter is an error");
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                &py,
                error,
                "TypeError"
            ));
            assert!(errors::take_current_error().is_none());
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
            );
            assert!(
                errors::take_current_error().is_none(),
                "observing the pending type must not create a C-owned error"
            );
            assert_eq!(crate::exception_last_bits_noinc(&py), Some(error));
            crate::clear_exception(&py);
        }
        dec_ref_bits(&py, name);
        dec_ref_bits(&py, bits);
        assert_eq!(receiver.object.ob_refcnt, 1);
        assert_eq!(kind.ob_base.ob_base.ob_refcnt, 1);
    });
}

unsafe extern "C" fn legacy_set(
    obj: *mut PyObject,
    _name: *const std::os::raw::c_char,
    _value: *mut PyObject,
) -> c_int {
    unsafe { attribute_get(obj, ptr::null_mut()) };
    // Deliberately violate the callback result/error protocol.
    0
}

#[test]
fn legacy_attribute_setters_validate_the_same_result_error_contract() {
    use molt_cpython_abi::api::{object, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.ob_base.ob_base.ob_refcnt = 1;
        kind.tp_name = c"LegacyAttributes".as_ptr();
        kind.tp_setattr = Some(legacy_set);
        let mut receiver = Receiver {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            calls: 0,
            malformed: false,
        };
        let name = strings::PyUnicode_FromString(c"field".as_ptr());
        for as_string in [false, true] {
            let status = if as_string {
                object::PyObject_SetAttrString(
                    &raw mut receiver.object,
                    c"field".as_ptr(),
                    ptr::null_mut(),
                )
            } else {
                object::PyObject_SetAttr(&raw mut receiver.object, name, ptr::null_mut())
            };
            assert_eq!(status, -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            crate::clear_exception(&py);
        }
        assert_eq!(receiver.calls, 2);
        refcount::Py_DECREF(name);
        assert_eq!(receiver.object.ob_refcnt, 1);
        assert_eq!(kind.ob_base.ob_base.ob_refcnt, 1);
    });
}
