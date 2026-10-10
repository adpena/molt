//! Real foreign exception slots with callback rebinding and physical presence.

use super::native_test_fixture::NativeType;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::refcount::OwnedPyObject;
use molt_cpython_abi::api::{errors, numbers, object, refcount, sequences, strings, typeobj};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

static OWNER: AtomicPtr<PyBaseExceptionObject> = AtomicPtr::new(ptr::null_mut());
static FAILURE: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static DROPS: AtomicUsize = AtomicUsize::new(0);
static DROP_RAISES: AtomicBool = AtomicBool::new(false);
static CONSTRUCTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static CONSTRUCTOR_RETURNS_NONE: AtomicBool = AtomicBool::new(false);
static CONSTRUCTOR_RESULT_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static RECURSIVE_INGRESS: AtomicUsize = AtomicUsize::new(0);
static BOOTSTRAP_TEXT_CALLS: AtomicUsize = AtomicUsize::new(0);
static SUBCLASS_CHECKS: AtomicUsize = AtomicUsize::new(0);
static SUBCLASS_CHECK_RAISES: AtomicBool = AtomicBool::new(false);
static INGRESS_CLASS: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static INGRESS_CLASS_REFS: AtomicUsize = AtomicUsize::new(0);

/// RuntimeTestTransaction serializes these callbacks; it does not own their
/// atomics. Reset their borrowed pointers and modes even when an assertion unwinds.
struct CallbackState;

impl CallbackState {
    fn new() -> Self {
        Self::reset();
        Self
    }

    fn reset() {
        OWNER.store(ptr::null_mut(), Ordering::SeqCst);
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        DROPS.store(0, Ordering::SeqCst);
        DROP_RAISES.store(false, Ordering::SeqCst);
        CONSTRUCTOR_CALLS.store(0, Ordering::SeqCst);
        CONSTRUCTOR_RETURNS_NONE.store(false, Ordering::SeqCst);
        CONSTRUCTOR_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        RECURSIVE_INGRESS.store(0, Ordering::SeqCst);
        BOOTSTRAP_TEXT_CALLS.store(0, Ordering::SeqCst);
        SUBCLASS_CHECKS.store(0, Ordering::SeqCst);
        SUBCLASS_CHECK_RAISES.store(false, Ordering::SeqCst);
        INGRESS_CLASS.store(ptr::null_mut(), Ordering::SeqCst);
        INGRESS_CLASS_REFS.store(0, Ordering::SeqCst);
        TEXT_CALLBACK_FAILS.store(false, Ordering::SeqCst);
        TEXT_PAYLOAD_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        TEXT_DROPS.store(0, Ordering::SeqCst);
        TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
        UNICODE_OWNER.store(ptr::null_mut(), Ordering::SeqCst);
        UNICODE_PHASE.store(0, Ordering::SeqCst);
    }
}

impl Drop for CallbackState {
    fn drop(&mut self) {
        Self::reset();
    }
}

unsafe extern "C" fn failing_constructor(
    _class: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    CONSTRUCTOR_CALLS.fetch_add(1, Ordering::SeqCst);
    if CONSTRUCTOR_RETURNS_NONE.load(Ordering::SeqCst) {
        let result_type = CONSTRUCTOR_RESULT_TYPE.load(Ordering::SeqCst);
        if !result_type.is_null() {
            return Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: result_type,
            }));
        }
        return unsafe { object::Py_NewRef(&raw mut Py_None) };
    }
    unsafe {
        errors::PyErr_SetObject(
            (&raw mut PyExc_LookupError).cast(),
            FAILURE.load(Ordering::SeqCst),
        );
    }
    ptr::null_mut()
}

unsafe extern "C" fn recursive_constructor(
    class: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    // Bound the negative control too: a missing normalization guard must fail
    // the RecursionError assertion instead of overflowing the test process.
    if CONSTRUCTOR_CALLS.fetch_add(1, Ordering::SeqCst) >= 64 {
        unsafe {
            errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast(),
                FAILURE.load(Ordering::SeqCst),
            );
        }
        return ptr::null_mut();
    }
    unsafe {
        match RECURSIVE_INGRESS.load(Ordering::SeqCst) {
            0 => errors::PyErr_SetNone(class.cast()),
            1 => errors::PyErr_Restore(
                object::Py_NewRef(class.cast()),
                ptr::null_mut(),
                ptr::null_mut(),
            ),
            2 => {
                let mut raised = errors::OwnedCError {
                    exc_type: object::Py_NewRef(class.cast()),
                    value: ptr::null_mut(),
                    traceback: ptr::null_mut(),
                };
                errors::PyErr_NormalizeException(
                    &raw mut raised.exc_type,
                    &raw mut raised.value,
                    &raw mut raised.traceback,
                );
                errors::restore_current_error_exact(raised);
            }
            _ => errors::PyErr_SetString(class.cast(), c"recursive constructor".as_ptr()),
        }
    }
    ptr::null_mut()
}

unsafe extern "C" fn unavailable_bootstrap_text(_data: *const u8, _len: usize) -> u64 {
    BOOTSTRAP_TEXT_CALLS.fetch_add(1, Ordering::SeqCst);
    0
}

unsafe extern "C" fn silent_constructor(
    _class: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    CONSTRUCTOR_CALLS.fetch_add(1, Ordering::SeqCst);
    ptr::null_mut()
}

unsafe extern "C" fn exception_subclass_check(
    _self: *mut PyObject,
    _candidate: *mut PyObject,
) -> *mut PyObject {
    SUBCLASS_CHECKS.fetch_add(1, Ordering::SeqCst);
    if SUBCLASS_CHECK_RAISES.load(Ordering::SeqCst) {
        unsafe {
            errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast(),
                FAILURE.load(Ordering::SeqCst),
            );
        }
        ptr::null_mut()
    } else {
        unsafe { object::Py_NewRef((&raw mut Py_True).cast()) }
    }
}

unsafe extern "C" fn payload_drop(value: *mut PyObject) {
    let class = INGRESS_CLASS.load(Ordering::SeqCst);
    if !class.is_null() {
        INGRESS_CLASS_REFS.store(
            unsafe { (*class).ob_base.ob_base.ob_refcnt } as usize,
            Ordering::SeqCst,
        );
    }
    DROPS.fetch_add(1, Ordering::SeqCst);
    drop(unsafe { Box::from_raw(value) });
    if DROP_RAISES.load(Ordering::SeqCst) {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"cleanup failure".as_ptr(),
            )
        };
    }
}

unsafe extern "C" fn payload_str(_value: *mut PyObject) -> *mut PyObject {
    let owner = OWNER.load(Ordering::SeqCst);
    let replacement = unsafe { OwnedPyObject::from_owned(sequences::PyTuple_New(1)) };
    let text =
        unsafe { OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"after".as_ptr())) };
    assert!(!replacement.as_ptr().is_null() && !text.as_ptr().is_null());
    unsafe { sequences::PyTuple_SetItem(replacement.as_ptr(), 0, text.into_ptr()) };
    let old = unsafe { std::mem::replace(&mut (*owner).args, replacement.into_ptr()) };
    unsafe { refcount::Py_DECREF(old) };
    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        0,
        "render owns the old args tuple"
    );
    let failure = FAILURE.load(Ordering::SeqCst);
    if !failure.is_null() {
        unsafe { errors::PyErr_SetObject((&raw mut PyExc_LookupError).cast(), failure) };
        return ptr::null_mut();
    }
    unsafe { strings::PyUnicode_FromString(c"before".as_ptr()) }
}

unsafe fn text(value: *mut PyObject) -> String {
    let value = unsafe { OwnedPyObject::from_owned(value) };
    let error = errors::take_current_error();
    assert!(!value.as_ptr().is_null(), "stringifier failed");
    assert!(error.is_none(), "stringifier left a C error");
    let mut length = 0;
    let data = unsafe { strings::PyUnicode_AsUTF8AndSize(value.as_ptr(), &raw mut length) };
    let error = errors::take_current_error();
    assert!(!data.is_null());
    assert!(error.is_none(), "UTF-8 extraction left a C error");
    String::from_utf8(
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) }.to_vec(),
    )
    .unwrap()
}

unsafe fn exception(ty: *mut PyTypeObject, values: &[*mut PyObject]) -> OwnedPyObject {
    assert_eq!(unsafe { typeobj::PyType_Ready(ty) }, 0);
    let args = unsafe { OwnedPyObject::from_owned(sequences::PyTuple_New(values.len() as isize)) };
    assert!(!args.as_ptr().is_null());
    for (index, &value) in values.iter().enumerate() {
        unsafe {
            refcount::Py_INCREF(value);
            assert_eq!(
                sequences::PyTuple_SetItem(args.as_ptr(), index as isize, value),
                0
            );
        }
    }
    let result = unsafe {
        OwnedPyObject::from_owned(errors::molt_native_exception_new(
            ty,
            args.as_ptr(),
            ptr::null_mut(),
        ))
    };
    assert!(
        !result.as_ptr().is_null(),
        "native exception allocation failed for {:?}",
        unsafe { std::ffi::CStr::from_ptr((*ty).tp_name) }
    );
    result
}

#[test]
fn c_error_ingress_preserves_native_identity_and_api_specific_context() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        for managed in [false, true] {
            let value_owner = if managed {
                // The public C constructor owns native storage. Construct the
                // other representation through the runtime's allocator.
                let value = crate::builtins::exceptions::alloc_exception(&py, "IndexError", "");
                assert!(!value.is_null());
                OwnedPyObject::from_owned(
                    GLOBAL_BRIDGE.owned_handle_to_pyobj(crate::MoltObject::from_ptr(value).bits()),
                )
            } else {
                exception(&raw mut PyExc_IndexError, &[])
            };
            let value = value_owner.as_ptr();
            assert!(!value.is_null());
            assert_eq!(
                GLOBAL_BRIDGE.molt_handle_for_pyobj(value).is_some(),
                managed
            );
            let handled_owner = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(
                (&raw mut PyExc_ValueError).cast(),
            ));
            let middle_owner = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(
                (&raw mut PyExc_TypeError).cast(),
            ));
            let handled = handled_owner.as_ptr();
            let middle = middle_owner.as_ptr();
            assert!(!handled.is_null() && !middle.is_null());
            // The active context already reaches the exception being raised.
            // SetObject must break that edge before replacing value.__context__.
            errors::PyException_SetContext(handled, object::Py_NewRef(middle));
            errors::PyException_SetContext(middle, object::Py_NewRef(value));
            errors::PyErr_SetHandledException(handled);
            errors::PyErr_SetObject((&raw mut PyExc_LookupError).cast(), value);
            let raised = errors::take_current_error().expect("normalized C error");
            assert_eq!(raised.value, value);
            assert_eq!(raised.exc_type, (&raw mut PyExc_IndexError).cast());
            let context = OwnedPyObject::from_owned(errors::PyException_GetContext(value));
            assert_eq!(context.as_ptr(), handled);
            drop(context);
            let context = OwnedPyObject::from_owned(errors::PyException_GetContext(middle));
            assert!(context.as_ptr().is_null());
            drop(context);
            drop(raised);

            let trace_class = crate::builtin_classes(&py).traceback;
            let trace_class = crate::obj_from_bits(trace_class).as_ptr().unwrap();
            let traceback_bits = crate::alloc_instance_for_class(&py, trace_class);
            let traceback_owner =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(traceback_bits));
            let traceback = traceback_owner.as_ptr();
            assert!(!traceback.is_null());
            assert_eq!(errors::PyException_SetTraceback(value, traceback), 0);
            errors::PyErr_SetHandledException(middle);
            let mut normalized = errors::OwnedCError {
                exc_type: object::Py_NewRef((&raw mut PyExc_LookupError).cast()),
                value: object::Py_NewRef(value),
                traceback: ptr::null_mut(),
            };
            errors::PyErr_NormalizeException(
                &raw mut normalized.exc_type,
                &raw mut normalized.value,
                &raw mut normalized.traceback,
            );
            assert_eq!(normalized.exc_type, (&raw mut PyExc_IndexError).cast());
            assert_eq!(normalized.value, value);
            assert!(normalized.traceback.is_null());
            let retained_traceback =
                OwnedPyObject::from_owned(errors::PyException_GetTraceback(value));
            assert_eq!(retained_traceback.as_ptr(), traceback);
            drop(retained_traceback);
            let context = OwnedPyObject::from_owned(errors::PyException_GetContext(value));
            assert_eq!(
                context.as_ptr(),
                handled,
                "Normalize does not chain handled context"
            );
            drop(context);

            errors::PyErr_Restore(
                std::mem::replace(&mut normalized.exc_type, ptr::null_mut()),
                std::mem::replace(&mut normalized.value, ptr::null_mut()),
                std::mem::replace(&mut normalized.traceback, ptr::null_mut()),
            );
            let restored = errors::take_current_error().expect("restored exact error");
            assert_eq!(restored.value, value);
            let retained_traceback =
                OwnedPyObject::from_owned(errors::PyException_GetTraceback(value));
            assert!(retained_traceback.as_ptr().is_null());
            drop(retained_traceback);
            let context = OwnedPyObject::from_owned(errors::PyException_GetContext(value));
            assert_eq!(
                context.as_ptr(),
                handled,
                "Restore does not chain handled context"
            );
            drop(context);
            drop(restored);

            // Restore's exact-class rule differs from SetObject/Normalize's
            // subclass rule: LookupError(IndexError()) constructs a new value.
            errors::PyErr_Restore(
                object::Py_NewRef((&raw mut PyExc_LookupError).cast()),
                object::Py_NewRef(value),
                ptr::null_mut(),
            );
            let rebuilt = errors::take_current_error().expect("rebuilt restore value");
            assert_ne!(rebuilt.value, value);
            assert_eq!(rebuilt.exc_type, (&raw mut PyExc_LookupError).cast());
            let args = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                rebuilt.value,
                c"args".as_ptr(),
            ));
            assert_eq!(sequences::PyTuple_Size(args.as_ptr()), 1);
            assert_eq!(sequences::PyTuple_GetItem(args.as_ptr(), 0), value);
            drop(args);
            drop(rebuilt);

            errors::PyErr_SetHandledException(ptr::null_mut());
            errors::PyException_SetContext(value, ptr::null_mut());
            errors::PyException_SetContext(handled, ptr::null_mut());
            drop(traceback_owner);
            drop(value_owner);
            drop(handled_owner);
            drop(middle_owner);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn c_error_normalization_transfers_constructor_failure_and_rejects_invalid_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let failure_owner = exception(&raw mut PyExc_LookupError, &[]);
        let failure = failure_owner.as_ptr();
        FAILURE.store(failure, Ordering::SeqCst);
        CONSTRUCTOR_CALLS.store(0, Ordering::SeqCst);
        CONSTRUCTOR_RETURNS_NONE.store(false, Ordering::SeqCst);
        let mut class = NativeType::subtype(&raw mut PyExc_ValueError, c"FailingException");
        class.tp_new = Some(failing_constructor);
        assert_eq!(class.ready(), 0);
        let class_ptr = (&raw mut *class).cast::<PyObject>();
        errors::PyErr_SetNone(class_ptr);
        let raised =
            errors::take_current_error().expect("constructor failure survives normalization");
        assert_eq!(raised.value, failure);
        drop(raised);

        errors::PyErr_Restore(
            object::Py_NewRef(class_ptr),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        let raised = errors::take_current_error().expect("Restore preserves constructor failure");
        assert_eq!(raised.value, failure);
        drop(raised);

        let trace_class = crate::obj_from_bits(crate::builtin_classes(&py).traceback)
            .as_ptr()
            .unwrap();
        let trace_bits = crate::alloc_instance_for_class(&py, trace_class);
        let traceback = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits));
        let mut normalized = errors::OwnedCError {
            exc_type: object::Py_NewRef(class_ptr),
            value: ptr::null_mut(),
            traceback: object::Py_NewRef(traceback.as_ptr()),
        };
        errors::PyErr_NormalizeException(
            &raw mut normalized.exc_type,
            &raw mut normalized.value,
            &raw mut normalized.traceback,
        );
        let occurred = errors::PyErr_Occurred();
        let pending = errors::take_current_error();
        assert_eq!(normalized.exc_type, (&raw mut PyExc_LookupError).cast());
        assert_eq!(normalized.value, failure);
        assert_eq!(
            normalized.traceback,
            traceback.as_ptr(),
            "constructor failure retains the input traceback"
        );
        assert!(occurred.is_null());
        drop(pending);
        drop(normalized);
        drop(traceback);

        CONSTRUCTOR_RETURNS_NONE.store(true, Ordering::SeqCst);
        errors::PyErr_SetNone(class_ptr);
        let raised = errors::take_current_error().expect("invalid constructor result");
        assert_eq!(raised.exc_type, (&raw mut PyExc_TypeError).cast());
        assert!(!raised.value.is_null());
        drop(raised);

        let mut invalid_result =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"InvalidExceptionResult");
        invalid_result.tp_dealloc = Some(payload_drop);
        assert_eq!(invalid_result.ready(), 0);
        CONSTRUCTOR_RESULT_TYPE.store(&raw mut *invalid_result, Ordering::SeqCst);
        DROP_RAISES.store(true, Ordering::SeqCst);
        let drops = DROPS.load(Ordering::SeqCst);
        errors::PyErr_SetNone(class_ptr);
        let raised =
            errors::take_current_error().expect("invalid result's cleanup preserves TypeError");
        assert_eq!(raised.exc_type, (&raw mut PyExc_TypeError).cast());
        assert_eq!(DROPS.load(Ordering::SeqCst), drops + 1);
        drop(raised);
        DROP_RAISES.store(false, Ordering::SeqCst);
        CONSTRUCTOR_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);

        // Restore and explicit Normalize steal their input. Retiring a last
        // owner whose destructor raises cannot replace the constructor failure.
        CONSTRUCTOR_RETURNS_NONE.store(false, Ordering::SeqCst);
        DROP_RAISES.store(true, Ordering::SeqCst);
        for explicit in [false, true] {
            let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *invalid_result,
            })));
            let drops = DROPS.load(Ordering::SeqCst);
            let raised = if explicit {
                let mut raised = errors::OwnedCError {
                    exc_type: object::Py_NewRef(class_ptr),
                    value: payload.into_ptr(),
                    traceback: ptr::null_mut(),
                };
                errors::PyErr_NormalizeException(
                    &raw mut raised.exc_type,
                    &raw mut raised.value,
                    &raw mut raised.traceback,
                );
                let occurred = errors::PyErr_Occurred();
                let _pending = errors::take_current_error();
                assert!(occurred.is_null());
                raised
            } else {
                errors::PyErr_Restore(
                    object::Py_NewRef(class_ptr),
                    payload.into_ptr(),
                    ptr::null_mut(),
                );
                errors::take_current_error().expect("Restore failure with reentrant input cleanup")
            };
            assert_eq!(raised.value, failure);
            assert_eq!(DROPS.load(Ordering::SeqCst), drops + 1);
            drop(raised);
        }
        DROP_RAISES.store(false, Ordering::SeqCst);

        class.tp_new = Some(silent_constructor);
        errors::PyErr_SetNone(class_ptr);
        let raised = errors::take_current_error().expect("silent constructor becomes SystemError");
        assert_eq!(raised.exc_type, (&raw mut PyExc_SystemError).cast());
        assert!(
            !raised.value.is_null(),
            "production failure is a real exception instance"
        );
        drop(raised);

        // A callable that is not an exception class must not execute at all.
        let mut unrelated = NativeType::subtype(&raw mut PyBaseObject_Type, c"UnrelatedCallable");
        unrelated.tp_new = Some(failing_constructor);
        assert_eq!(unrelated.ready(), 0);
        let calls = CONSTRUCTOR_CALLS.load(Ordering::SeqCst);
        errors::PyErr_SetNone((&raw mut *unrelated).cast());
        let raised = errors::take_current_error().expect("invalid exception class");
        assert_eq!(CONSTRUCTOR_CALLS.load(Ordering::SeqCst), calls);
        assert_eq!(raised.exc_type, (&raw mut PyExc_SystemError).cast());
        drop(raised);
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        CONSTRUCTOR_RETURNS_NONE.store(false, Ordering::SeqCst);
        drop(failure_owner);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn c_error_normalization_bounds_every_entry_and_recovers_after_recursion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let failure_owner = exception(&raw mut PyExc_LookupError, &[]);
        let failure = failure_owner.as_ptr();
        FAILURE.store(failure, Ordering::SeqCst);
        let mut class = NativeType::subtype(&raw mut PyExc_ValueError, c"RecursiveException");
        class.tp_new = Some(recursive_constructor);
        assert_eq!(class.ready(), 0);
        let class_ptr = (&raw mut *class).cast::<PyObject>();
        for ingress in 0..4 {
            RECURSIVE_INGRESS.store(ingress, Ordering::SeqCst);
            CONSTRUCTOR_CALLS.store(0, Ordering::SeqCst);
            let raised = match ingress {
                0 => {
                    errors::PyErr_SetNone(class_ptr);
                    errors::take_current_error().expect("bounded SetObject normalization")
                }
                1 => {
                    errors::PyErr_Restore(
                        object::Py_NewRef(class_ptr),
                        ptr::null_mut(),
                        ptr::null_mut(),
                    );
                    errors::take_current_error().expect("bounded Restore normalization")
                }
                2 => {
                    let mut raised = errors::OwnedCError {
                        exc_type: object::Py_NewRef(class_ptr),
                        value: ptr::null_mut(),
                        traceback: ptr::null_mut(),
                    };
                    errors::PyErr_NormalizeException(
                        &raw mut raised.exc_type,
                        &raw mut raised.value,
                        &raw mut raised.traceback,
                    );
                    let occurred = errors::PyErr_Occurred();
                    let _pending = errors::take_current_error();
                    assert!(occurred.is_null());
                    raised
                }
                _ => {
                    errors::PyErr_SetString(class_ptr, c"recursive constructor".as_ptr());
                    errors::take_current_error().expect("bounded SetString normalization")
                }
            };
            assert_eq!(raised.exc_type, (&raw mut PyExc_RecursionError).cast());
            assert!(!raised.value.is_null());
            assert_eq!(
                text(typeobj::PyObject_Str(raised.value)),
                "maximum recursion depth exceeded while normalizing an exception",
                "constructor recursion preserves the canonical diagnostic"
            );
            assert!(CONSTRUCTOR_CALLS.load(Ordering::SeqCst) < 64);
            drop(raised);

            errors::PyErr_SetNone((&raw mut PyExc_ValueError).cast());
            let recovered =
                errors::take_current_error().expect("depth scope released after recursion");
            assert_eq!(recovered.exc_type, (&raw mut PyExc_ValueError).cast());
            assert!(!recovered.value.is_null());
            drop(recovered);
        }
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        drop(failure_owner);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn c_error_setstring_owns_borrowed_inputs_through_replacement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut class = NativeType::subtype(&raw mut PyExc_ValueError, c"BorrowedInputException");
        assert_eq!(class.ready(), 0);
        let class_ptr = &raw mut *class;
        let class_refs = class.ob_base.ob_base.ob_refcnt;
        let mut payload_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"ReplacementFinalizer");
        payload_type.tp_dealloc = Some(payload_drop);
        assert_eq!(payload_type.ready(), 0);
        let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *payload_type,
        })));
        let previous = exception(class_ptr, &[payload.as_ptr()]);
        drop(payload);
        errors::PyErr_SetRaisedException(previous.into_ptr());
        INGRESS_CLASS.store(class_ptr, Ordering::SeqCst);
        INGRESS_CLASS_REFS.store(0, Ordering::SeqCst);
        DROPS.store(0, Ordering::SeqCst);
        DROP_RAISES.store(true, Ordering::SeqCst);
        errors::PyErr_SetString(errors::PyErr_Occurred(), c"replacement".as_ptr());
        let replaced = errors::take_current_error().expect("replacement survives finalizer errors");
        DROP_RAISES.store(false, Ordering::SeqCst);
        INGRESS_CLASS.store(ptr::null_mut(), Ordering::SeqCst);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        assert!(
            INGRESS_CLASS_REFS.load(Ordering::SeqCst) > class_refs as usize,
            "the fixture owner plus ingress pin must survive old-error finalization"
        );
        assert_eq!(replaced.exc_type, class_ptr.cast());
        assert_eq!(text(typeobj::PyObject_Str(replaced.value)), "replacement");
        drop(replaced);

        errors::PyErr_SetString(
            class_ptr.cast(),
            c"message borrowed from old error".as_ptr(),
        );
        let previous = errors::take_current_error().expect("old error owns borrowed message");
        let argument =
            sequences::PyTuple_GetItem((*previous.value.cast::<PyBaseExceptionObject>()).args, 0);
        let message = strings::PyUnicode_AsUTF8AndSize(argument, ptr::null_mut());
        assert!(!message.is_null());
        errors::restore_current_error_exact(previous);
        errors::PyErr_SetString(errors::PyErr_Occurred(), message);
        let replaced = errors::take_current_error().expect("borrowed inputs are pinned");
        assert_eq!(replaced.exc_type, class_ptr.cast());
        assert_eq!(
            text(typeobj::PyObject_Str(replaced.value)),
            "message borrowed from old error"
        );
        drop(replaced);
        assert_eq!(class.ob_base.ob_base.ob_refcnt, class_refs);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn c_error_normalization_uses_generic_subclass_callbacks_but_restore_is_exact() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let failure_owner = exception(&raw mut PyExc_LookupError, &[]);
        let failure = failure_owner.as_ptr();
        let input_owner = exception(&raw mut PyExc_IndexError, &[]);
        let input = input_owner.as_ptr();
        FAILURE.store(failure, Ordering::SeqCst);
        let mut methods = [
            PyMethodDef {
                ml_name: c"__subclasscheck__".as_ptr(),
                ml_meth: Some(exception_subclass_check),
                ml_flags: METH_O,
                ml_doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        let mut meta = NativeType::subtype(&raw mut PyType_Type, c"ExceptionCheckMeta");
        meta.tp_methods = methods.as_mut_ptr();
        assert_eq!(meta.ready(), 0);
        let mut class = NativeType::subtype(&raw mut PyExc_ValueError, c"ProtocolException");
        class.ob_base.ob_base.ob_type = &raw mut *meta;
        assert_eq!(class.ready(), 0);
        let class_ptr = (&raw mut *class).cast::<PyObject>();
        for raises in [false, true] {
            SUBCLASS_CHECK_RAISES.store(raises, Ordering::SeqCst);
            for explicit in [false, true] {
                SUBCLASS_CHECKS.store(0, Ordering::SeqCst);
                let raised = if explicit {
                    let mut raised = errors::OwnedCError {
                        exc_type: object::Py_NewRef(class_ptr),
                        value: object::Py_NewRef(input),
                        traceback: ptr::null_mut(),
                    };
                    errors::PyErr_NormalizeException(
                        &raw mut raised.exc_type,
                        &raw mut raised.value,
                        &raw mut raised.traceback,
                    );
                    let occurred = errors::PyErr_Occurred();
                    let _pending = errors::take_current_error();
                    assert!(occurred.is_null());
                    raised
                } else {
                    errors::PyErr_SetObject(class_ptr, input);
                    errors::take_current_error().expect("subclass callback normalization")
                };
                assert_eq!(
                    SUBCLASS_CHECKS.load(Ordering::SeqCst),
                    1,
                    "raises={raises}, explicit={explicit}"
                );
                assert_eq!(
                    raised.exc_type,
                    if raises {
                        (&raw mut PyExc_LookupError).cast()
                    } else {
                        (&raw mut PyExc_IndexError).cast()
                    },
                    "raises={raises}, explicit={explicit}, actual_class={:?}",
                    exc_singleton_name(raised.exc_type),
                );
                assert_eq!(
                    raised.value,
                    if raises { failure } else { input },
                    "raises={raises}, explicit={explicit}"
                );
                drop(raised);
            }
        }
        SUBCLASS_CHECKS.store(0, Ordering::SeqCst);
        errors::PyErr_Restore(
            object::Py_NewRef(class_ptr),
            object::Py_NewRef(input),
            ptr::null_mut(),
        );
        let restored = errors::take_current_error().expect("Restore uses exact class construction");
        assert_eq!(SUBCLASS_CHECKS.load(Ordering::SeqCst), 0);
        assert_eq!(restored.exc_type, class_ptr);
        assert_ne!(restored.value, input);
        drop(restored);
        SUBCLASS_CHECK_RAISES.store(false, Ordering::SeqCst);
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        drop(input_owner);
        drop(failure_owner);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn c_error_bootstrap_admits_type_only_before_constructor_allocation() {
    const CHILD: &str = "MOLT_EXCEPTION_BOOTSTRAP_TEST_CHILD";
    if let Some(mode) = std::env::var_os(CHILD) {
        let _callbacks = CallbackState::new();
        assert!(molt_cpython_abi::hooks::hooks().is_none());
        // Reuse the production initialization TLS guard. A normal runtime test
        // transaction would register full hooks and erase the pre-init boundary.
        object::prepare_runtime_thread_state_lifetime();
        object::arm_runtime_thread_state_lifetime();
        let _initializing = object::RuntimeInitializationThreadStateGuard::enter();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        if mode == "partial" {
            let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
            hooks.alloc_str = Some(unavailable_bootstrap_text);
            assert!(unsafe { molt_cpython_abi::hooks::try_set_runtime_hooks(hooks) });
        } else {
            assert_eq!(mode, "pre-init");
        }
        unsafe {
            // This test deliberately precedes readiness and allocation hooks.
            // Declare only the class identity; no inherited slots or READY flag.
            let mut class: PyTypeObject = std::mem::zeroed();
            class.ob_base.ob_base.ob_refcnt = 1;
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            class.tp_name = c"BootstrapException".as_ptr();
            class.tp_base = &raw mut PyExc_ValueError;
            class.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE;
            class.tp_new = Some(silent_constructor);
            let class_ptr = (&raw mut class).cast::<PyObject>();
            CONSTRUCTOR_CALLS.store(0, Ordering::SeqCst);
            BOOTSTRAP_TEXT_CALLS.store(0, Ordering::SeqCst);
            for ingress in 0..4 {
                let raised = match ingress {
                    0 => {
                        errors::PyErr_SetString(class_ptr, c"unavailable construction".as_ptr());
                        errors::take_current_error().expect("bootstrap SetString indicator")
                    }
                    1 => {
                        errors::PyErr_SetObject(class_ptr, (&raw mut Py_True).cast());
                        errors::take_current_error().expect("bootstrap SetObject indicator")
                    }
                    2 => {
                        errors::PyErr_Restore(
                            object::Py_NewRef(class_ptr),
                            object::Py_NewRef((&raw mut Py_True).cast()),
                            ptr::null_mut(),
                        );
                        errors::take_current_error().expect("bootstrap Restore indicator")
                    }
                    _ => {
                        let mut raised = errors::OwnedCError {
                            exc_type: object::Py_NewRef(class_ptr),
                            value: object::Py_NewRef((&raw mut Py_True).cast()),
                            traceback: ptr::null_mut(),
                        };
                        errors::PyErr_NormalizeException(
                            &raw mut raised.exc_type,
                            &raw mut raised.value,
                            &raw mut raised.traceback,
                        );
                        let occurred = errors::PyErr_Occurred();
                        let _pending = errors::take_current_error();
                        assert!(occurred.is_null());
                        raised
                    }
                };
                assert_eq!(raised.exc_type, class_ptr);
                assert!(
                    raised.value.is_null(),
                    "arbitrary payload is never a normalized instance"
                );
                assert!(raised.traceback.is_null());
                assert_eq!(CONSTRUCTOR_CALLS.load(Ordering::SeqCst), 0);
                assert_eq!(BOOTSTRAP_TEXT_CALLS.load(Ordering::SeqCst), 0);
                drop(raised);
                assert!(errors::PyErr_Occurred().is_null());
            }
            assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
        }
        println!("bootstrap normalization admission verified");
        return;
    }
    // Hook registration is process-global and one-shot. Match the existing
    // lifecycle suite's serialized exact-test child custody, without changing
    // the parent's hook table or running a second runtime inside it.
    for mode in ["pre-init", "partial"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cpython_abi_hooks::exception_rendering_tests::c_error_bootstrap_admits_type_only_before_constructor_allocation",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, mode)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("bootstrap normalization admission verified"),
            "{mode}: {output:?}",
        );
    }
}

#[test]
fn declaring_slots_own_foreign_arguments_and_distinguish_null_fields() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        DROPS.store(0, Ordering::SeqCst);
        assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_BaseException), 0);
        assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_Exception), 0);
        assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_LookupError), 0);
        assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_KeyError), 0);
        assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_ValueError), 0);
        assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_OSError), 0);

        let arg = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"x".as_ptr()));
        let key_owner = exception(&raw mut PyExc_KeyError, &[arg.as_ptr()]);
        let key = key_owner.as_ptr();
        assert_eq!(text(errors::molt_native_exception_str(key)), "x");
        assert_eq!(text(typeobj::PyObject_Str(key)), "'x'");
        assert_eq!(
            text(errors::molt_native_exception_repr(key)),
            "KeyError('x')"
        );
        assert_eq!(text(typeobj::PyObject_Repr(key)), "KeyError('x')");
        let owner = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            (&raw mut PyExc_KeyError).cast(),
            c"__str__".as_ptr(),
        ));
        let base_owner = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            (&raw mut PyExc_BaseException).cast(),
            c"__str__".as_ptr(),
        ));
        assert!(
            !owner.as_ptr().is_null()
                && !base_owner.as_ptr().is_null()
                && owner.as_ptr() != base_owner.as_ptr()
        );
        drop(owner);
        drop(base_owner);
        drop(key_owner);
        drop(arg);

        let mut payload_type = NativeType::subtype(&raw mut PyBaseObject_Type, c"RenderingPayload");
        payload_type.tp_str = Some(payload_str);
        payload_type.tp_dealloc = Some(payload_drop);
        assert_eq!(payload_type.ready(), 0);
        let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *payload_type,
        })));
        let error_owner = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
        let error = error_owner.as_ptr();
        drop(payload);
        OWNER.store(error.cast(), Ordering::SeqCst);
        assert_eq!(text(typeobj::PyObject_Str(error)), "before");
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        assert_eq!(text(typeobj::PyObject_Str(error)), "after");
        drop(error_owner);
        OWNER.store(ptr::null_mut(), Ordering::SeqCst);

        DROPS.store(0, Ordering::SeqCst);
        let failure_owner = exception(&raw mut PyExc_LookupError, &[]);
        let failure = failure_owner.as_ptr();
        FAILURE.store(failure, Ordering::SeqCst);
        DROP_RAISES.store(true, Ordering::SeqCst);
        let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *payload_type,
        })));
        let error_owner = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
        let error = error_owner.as_ptr();
        drop(payload);
        OWNER.store(error.cast(), Ordering::SeqCst);
        let rendered = OwnedPyObject::from_owned(typeobj::PyObject_Str(error));
        let raised = errors::take_current_error();
        assert!(rendered.as_ptr().is_null());
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        let raised = raised.expect("callback exception");
        assert_eq!(
            raised.value, failure,
            "cleanup preserves the exact callback exception"
        );
        DROP_RAISES.store(false, Ordering::SeqCst);
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        drop(raised);
        drop(failure_owner);
        drop(error_owner);
        OWNER.store(ptr::null_mut(), Ordering::SeqCst);

        let os_error_owner = exception(&raw mut PyExc_OSError, &[]);
        let os_error = os_error_owner.as_ptr();
        let physical = os_error.cast::<PyOSErrorObject>();
        assert_eq!(text(typeobj::PyObject_Str(os_error)), "");
        (*physical).filename = object::Py_NewRef(&raw mut Py_None);
        assert_eq!(
            text(typeobj::PyObject_Str(os_error)),
            "[Errno None] None: None"
        );
        (*physical).filename2 = object::Py_NewRef(&raw mut Py_None);
        assert_eq!(
            text(typeobj::PyObject_Str(os_error)),
            "[Errno None] None: None -> None"
        );
        let old = std::mem::replace(&mut (*physical).filename, ptr::null_mut());
        refcount::Py_DECREF(old);
        assert_eq!(text(typeobj::PyObject_Str(os_error)), "");
        drop(os_error_owner);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn raised_exception_transfer_preserves_identity_through_finalizers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        raised_exception_transfer_cases();
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn python_text_transport_preserves_codepoints_and_callback_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        python_text_transport_cases();
        assert!(errors::PyErr_Occurred().is_null());
    });
}

unsafe fn raised_exception_transfer_cases() {
    unsafe {
        let value_owner = exception(&raw mut PyExc_LookupError, &[]);
        let value = value_owner.as_ptr();
        // Direct physical storage models a native exception's existing traceback;
        // Get/SetRaisedException must transfer it without validation or rebuilding.
        let traceback = OwnedPyObject::from_owned(sequences::PyTuple_New(0));
        let traceback = traceback.into_ptr();
        (*value.cast::<PyBaseExceptionObject>()).traceback = traceback;
        refcount::Py_INCREF(value);
        errors::PyErr_SetRaisedException(value);
        let occurred = errors::PyErr_Occurred();
        let refs = (*value).ob_refcnt;
        let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(occurred, (&raw mut PyExc_LookupError).cast());
        assert_eq!(refs, 2, "SetRaisedException steals its reference");
        assert_eq!(
            raised.as_ptr(),
            value,
            "the exact existing instance is transferred"
        );
        assert!(errors::PyErr_Occurred().is_null());
        let empty = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(empty.as_ptr().is_null());
        assert_eq!((*value).ob_refcnt, 2);
        let preserved_traceback =
            OwnedPyObject::from_owned(errors::PyException_GetTraceback(raised.as_ptr()));
        assert_eq!(preserved_traceback.as_ptr(), traceback);
        drop(preserved_traceback);
        drop(raised);
        drop(value_owner);

        // Retiring a previous exception runs arbitrary finalizers. A finalizer
        // that raises cannot replace the caller's new indicator or undo clear.
        let mut payload_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"RaisedCleanupPayload");
        payload_type.tp_dealloc = Some(payload_drop);
        assert_eq!(payload_type.ready(), 0);
        DROP_RAISES.store(true, Ordering::SeqCst);
        let drops_before = DROPS.load(Ordering::SeqCst);
        for clear in [false, true] {
            let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *payload_type,
            })));
            let old = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
            drop(payload);
            errors::PyErr_SetRaisedException(old.into_ptr());
            let next = if clear {
                OwnedPyObject::from_owned(ptr::null_mut())
            } else {
                exception(&raw mut PyExc_LookupError, &[])
            };
            let next = next.into_ptr();
            errors::PyErr_SetRaisedException(next);
            let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(
                raised.as_ptr(),
                next,
                "finalizer errors must not replace the caller's state"
            );
            assert!(errors::PyErr_Occurred().is_null());
            drop(raised);
        }
        assert_eq!(DROPS.load(Ordering::SeqCst), drops_before + 2);
        DROP_RAISES.store(false, Ordering::SeqCst);
    }
}

// CallbackState resets this fixture state inside RuntimeTestTransaction's serial scope.
static TEXT_CALLBACK_FAILS: AtomicBool = AtomicBool::new(false);
static TEXT_PAYLOAD_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static TEXT_DROPS: AtomicUsize = AtomicUsize::new(0);
static TEXT_DROP_RAISES: AtomicBool = AtomicBool::new(false);
static UNICODE_OWNER: AtomicPtr<PyUnicodeErrorObject> = AtomicPtr::new(ptr::null_mut());
static UNICODE_PHASE: AtomicUsize = AtomicUsize::new(0);

unsafe fn raw_fixture(bytes: &[u8]) -> *mut PyObject {
    let bits = unsafe { super::hook_alloc_str(bytes.as_ptr(), bytes.len()) };
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

unsafe fn borrowed_raw(value: *mut PyObject) -> Vec<u8> {
    assert!(!value.is_null(), "Python-text result failed");
    let bits = GLOBAL_BRIDGE
        .molt_handle_for_pyobj(value)
        .expect("fixture string handle");
    let mut length = 0;
    let data = unsafe { super::hook_str_data(bits.bits(), &raw mut length) };
    assert!(!data.is_null());
    unsafe { std::slice::from_raw_parts(data, length).to_vec() }
}

unsafe fn take_raw(value: *mut PyObject) -> Vec<u8> {
    let value = unsafe { OwnedPyObject::from_owned(value) };
    let error = errors::take_current_error();
    let result = unsafe { borrowed_raw(value.as_ptr()) };
    assert!(error.is_none(), "Python-text result left a C error");
    result
}

unsafe fn percent(template: &[u8], arg: *mut PyObject) -> *mut PyObject {
    let format = unsafe { OwnedPyObject::from_owned(raw_fixture(template)) };
    unsafe { strings::PyUnicode_Format(format.as_ptr(), arg) }
}

unsafe extern "C" fn text_payload_str(_value: *mut PyObject) -> *mut PyObject {
    if TEXT_CALLBACK_FAILS.load(Ordering::SeqCst) {
        unsafe {
            errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast(),
                FAILURE.load(Ordering::SeqCst),
            )
        };
        return ptr::null_mut();
    }
    unsafe { raw_fixture(b"s\xed\xa0\x80\xed\xb0\x80\0z") }
}

unsafe extern "C" fn text_payload_repr(_value: *mut PyObject) -> *mut PyObject {
    unsafe { raw_fixture(b"r\xed\xb0\x80\xed\xa0\x80\0z") }
}

unsafe extern "C" fn text_payload_drop(value: *mut PyObject) {
    TEXT_DROPS.fetch_add(1, Ordering::SeqCst);
    drop(unsafe { Box::from_raw(value) });
    if TEXT_DROP_RAISES.load(Ordering::SeqCst) {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"mapping cleanup failure".as_ptr(),
            )
        };
    }
}

unsafe extern "C" fn text_mapping_item(
    _mapping: *mut PyObject,
    key: *mut PyObject,
) -> *mut PyObject {
    assert_eq!(unsafe { borrowed_raw(key) }, b"key\xed\xa0\x80\0");
    Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: TEXT_PAYLOAD_TYPE.load(Ordering::SeqCst),
    }))
}

unsafe extern "C" fn unicode_reason_str(_value: *mut PyObject) -> *mut PyObject {
    assert_eq!(UNICODE_PHASE.swap(1, Ordering::SeqCst), 0);
    unsafe { raw_fixture(b"reason\xed\xa0\x80\0") }
}

unsafe extern "C" fn unicode_encoding_str(_value: *mut PyObject) -> *mut PyObject {
    assert_eq!(UNICODE_PHASE.swap(2, Ordering::SeqCst), 1);
    let owner = UNICODE_OWNER.load(Ordering::SeqCst);
    unsafe {
        (*owner).start = 1;
        (*owner).end = 2;
        raw_fixture(b"enc\xed\xb0\x80")
    }
}

unsafe fn python_text_transport_cases() {
    unsafe {
        for class in [
            &raw mut PyExc_SyntaxError,
            &raw mut PyExc_UnicodeError,
            &raw mut PyExc_UnicodeEncodeError,
            &raw mut PyExc_UnicodeDecodeError,
            &raw mut PyExc_UnicodeTranslateError,
            &raw mut PyExc_SystemError,
            &raw mut PyExc_MemoryError,
        ] {
            assert_eq!(typeobj::PyType_Ready(class), 0);
        }

        let mut payload_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"PythonTextPayload");
        payload_type.tp_str = Some(text_payload_str);
        payload_type.tp_repr = Some(text_payload_repr);
        payload_type.tp_dealloc = Some(text_payload_drop);
        assert_eq!(payload_type.ready(), 0);
        TEXT_PAYLOAD_TYPE.store(&raw mut *payload_type, Ordering::SeqCst);
        let payload_owner = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *payload_type,
        })));
        let payload = payload_owner.as_ptr();

        let value_owner = exception(&raw mut PyExc_ValueError, &[payload]);
        let value = value_owner.as_ptr();
        assert_eq!(
            take_raw(errors::molt_native_exception_repr(value)),
            b"ValueError(r\xed\xb0\x80\xed\xa0\x80\0z)"
        );
        assert_eq!(
            take_raw(typeobj::PyObject_Str(value)),
            b"s\xed\xa0\x80\xed\xb0\x80\0z"
        );
        assert_eq!(
            take_raw(object::PyObject_ASCII(payload)),
            b"r\\udc00\\ud800\0z"
        );
        drop(value_owner);

        let mut named_type = NativeType::subtype(&raw mut PyExc_ValueError, c"pkg.HighError");
        assert_eq!(named_type.ready(), 0);
        // A malformed external C name exercises diagnostic decoding only;
        // inherited slots and namespace ownership were admitted while valid.
        named_type.tp_name = c"pkg.High\xed\xa0\x80Error".as_ptr();
        let named = exception(&raw mut *named_type, &[]);
        assert_eq!(
            take_raw(errors::molt_native_exception_repr(named.as_ptr())),
            b"High\xef\xbf\xbd\xef\xbf\xbd\xef\xbf\xbdError()"
        );
        assert_eq!(
            take_raw(typeobj::PyType_GetName(&raw mut *named_type)),
            b"High\xef\xbf\xbd\xef\xbf\xbd\xef\xbf\xbdError"
        );
        drop(named);

        let syntax_owner = exception(&raw mut PyExc_SyntaxError, &[]);
        let syntax = syntax_owner.as_ptr();
        let fields = syntax.cast::<PySyntaxErrorObject>();
        (*fields).msg = object::Py_NewRef(payload);
        (*fields).filename = raw_fixture(b"file\xed\xb0\x80\0.py");
        (*fields).lineno = numbers::PyLong_FromLong(7);
        assert_eq!(
            take_raw(typeobj::PyObject_Str(syntax)),
            b"s\xed\xa0\x80\xed\xb0\x80\0z (file\xed\xb0\x80\0.py, line 7)"
        );
        drop(syntax_owner);

        let os_error_owner = exception(&raw mut PyExc_OSError, &[]);
        let os_error = os_error_owner.as_ptr();
        let fields = os_error.cast::<PyOSErrorObject>();
        (*fields).myerrno = numbers::PyLong_FromLong(5);
        (*fields).strerror = object::Py_NewRef(payload);
        (*fields).filename = object::Py_NewRef(payload);
        assert_eq!(
            take_raw(typeobj::PyObject_Str(os_error)),
            b"[Errno 5] s\xed\xa0\x80\xed\xb0\x80\0z: r\xed\xb0\x80\xed\xa0\x80\0z"
        );
        drop(os_error_owner);

        let mut reason_type = NativeType::subtype(&raw mut PyBaseObject_Type, c"Reason");
        reason_type.tp_str = Some(unicode_reason_str);
        reason_type.tp_dealloc = Some(text_payload_drop);
        assert_eq!(reason_type.ready(), 0);
        let mut encoding_type = NativeType::subtype(&raw mut PyBaseObject_Type, c"Encoding");
        encoding_type.tp_str = Some(unicode_encoding_str);
        encoding_type.tp_dealloc = Some(text_payload_drop);
        assert_eq!(encoding_type.ready(), 0);
        let unicode_owner = exception(&raw mut PyExc_UnicodeEncodeError, &[]);
        let unicode = unicode_owner.as_ptr();
        let fields = unicode.cast::<PyUnicodeErrorObject>();
        (*fields).reason = Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *reason_type,
        }));
        (*fields).encoding = Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *encoding_type,
        }));
        (*fields).object = raw_fixture(b"\xed\xa0\x80\xed\xb0\x80");
        (*fields).start = 0;
        (*fields).end = 1;
        UNICODE_OWNER.store(fields, Ordering::SeqCst);
        assert_eq!(take_raw(typeobj::PyObject_Str(unicode)), b"'enc\xed\xb0\x80' codec can't encode character '\\udc00' in position 1: reason\xed\xa0\x80\0");
        assert_eq!(UNICODE_PHASE.load(Ordering::SeqCst), 2);
        UNICODE_OWNER.store(ptr::null_mut(), Ordering::SeqCst);
        drop(unicode_owner);

        assert_eq!(
            take_raw(percent(b"[%7.3s]", payload)),
            b"[    s\xed\xa0\x80\xed\xb0\x80]"
        );
        assert_eq!(
            take_raw(percent(b"%r", payload)),
            b"r\xed\xb0\x80\xed\xa0\x80\0z"
        );
        assert_eq!(take_raw(percent(b"%a", payload)), b"r\\udc00\\ud800\0z");
        let ordinary = OwnedPyObject::from_owned(raw_fixture(b"x"));
        assert_eq!(
            take_raw(percent(b"%r", ordinary.as_ptr())),
            b"'x'",
            "%r dispatches repr even for strings"
        );
        drop(ordinary);

        let high_owner = OwnedPyObject::from_owned(strings::PyUnicode_FromOrdinal(0xd800));
        let high = high_owner.as_ptr();
        let low_owner = OwnedPyObject::from_owned(strings::PyUnicode_FromOrdinal(0xdc00));
        let low = low_owner.as_ptr();
        let pair_owner = OwnedPyObject::from_owned(strings::PyUnicode_Concat(high, low));
        let pair = pair_owner.as_ptr();
        assert_eq!(borrowed_raw(pair), b"\xed\xa0\x80\xed\xb0\x80");
        assert_eq!(strings::PyUnicode_GetLength(pair), 2);
        assert_eq!(strings::PyUnicode_ReadChar(pair, 1), 0xdc00);
        assert_eq!(strings::PyUnicode_FindChar(pair, 0xdc00, 0, 2, 1), 1);
        assert_eq!(strings::PyUnicode_Tailmatch(pair, low, 0, 2, 1), 1);
        assert_eq!(
            take_raw(strings::PyUnicode_Substring(pair, 1, 2)),
            b"\xed\xb0\x80"
        );
        let mut ucs4 = [0u32; 3];
        assert_eq!(
            strings::PyUnicode_AsUCS4(pair, ucs4.as_mut_ptr(), 3, 1),
            ucs4.as_mut_ptr()
        );
        assert_eq!(ucs4, [0xd800, 0xdc00, 0]);
        let codepoints = [0xd800u16, 0xdc00, 0];
        assert_eq!(
            take_raw(strings::PyUnicode_FromKindAndData(
                2,
                codepoints.as_ptr().cast(),
                3
            )),
            b"\xed\xa0\x80\xed\xb0\x80\0"
        );
        assert_eq!(take_raw(percent(b"%3c", high)), b"  \xed\xa0\x80");
        let low_int = OwnedPyObject::from_owned(numbers::PyLong_FromLong(0xdc00));
        assert_eq!(take_raw(percent(b"%c", low_int.as_ptr())), b"\xed\xb0\x80");
        drop(low_int);
        let tuple = OwnedPyObject::from_owned(sequences::PyTuple_New(2));
        sequences::PyTuple_SetItem(tuple.as_ptr(), 0, object::Py_NewRef(high));
        sequences::PyTuple_SetItem(tuple.as_ptr(), 1, object::Py_NewRef(low));
        let separator = OwnedPyObject::from_owned(raw_fixture(b"\0"));
        assert_eq!(
            take_raw(strings::PyUnicode_Join(separator.as_ptr(), tuple.as_ptr())),
            b"\xed\xa0\x80\0\xed\xb0\x80"
        );
        drop(tuple);
        let empty = OwnedPyObject::from_owned(raw_fixture(b""));
        assert_eq!(
            take_raw(strings::PyUnicode_Replace(
                pair,
                empty.as_ptr(),
                separator.as_ptr(),
                -1
            )),
            b"\0\xed\xa0\x80\0\xed\xb0\x80\0"
        );
        drop(empty);
        drop(separator);

        let mut mapping_slots: PyMappingMethods = std::mem::zeroed();
        mapping_slots.mp_subscript = text_mapping_item as *mut std::ffi::c_void;
        let mut mapping_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"PythonTextMapping");
        mapping_type.tp_as_mapping = (&raw mut mapping_slots).cast();
        assert_eq!(mapping_type.ready(), 0);
        assert!(
            mapping_type.tp_dealloc.is_some(),
            "object subtype inherits its production deallocator"
        );
        let mapping =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *mapping_type, 0));
        assert!(!mapping.as_ptr().is_null());
        TEXT_DROPS.store(0, Ordering::SeqCst);
        assert_eq!(
            take_raw(percent(b"%(key\xed\xa0\x80\0)s", mapping.as_ptr())),
            b"s\xed\xa0\x80\xed\xb0\x80\0z"
        );
        assert_eq!(TEXT_DROPS.load(Ordering::SeqCst), 1);
        let rendered =
            OwnedPyObject::from_owned(percent(b"%(key\xed\xa0\x80\0)", mapping.as_ptr()));
        let occurred = errors::PyErr_Occurred();
        let parse_error = errors::take_current_error();
        assert!(rendered.as_ptr().is_null());
        assert_eq!(
            TEXT_DROPS.load(Ordering::SeqCst),
            2,
            "lookup owner released on parse failure"
        );
        assert!(!occurred.is_null());
        drop(parse_error);
        errors::PyErr_Clear();
        let failure_owner = exception(&raw mut PyExc_LookupError, &[]);
        let failure = failure_owner.as_ptr();
        FAILURE.store(failure, Ordering::SeqCst);
        TEXT_CALLBACK_FAILS.store(true, Ordering::SeqCst);
        TEXT_DROP_RAISES.store(true, Ordering::SeqCst);
        let rendered =
            OwnedPyObject::from_owned(percent(b"%(key\xed\xa0\x80\0)s", mapping.as_ptr()));
        let raised = errors::take_current_error();
        assert!(rendered.as_ptr().is_null());
        let raised = raised.expect("callback failure preserved");
        assert_eq!(raised.value, failure);
        assert_eq!(TEXT_DROPS.load(Ordering::SeqCst), 3);
        TEXT_CALLBACK_FAILS.store(false, Ordering::SeqCst);
        TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        drop(raised);
        drop(failure_owner);

        // Public UTF-8 ingress stays strict even though internal text accepts surrogates.
        for invalid in [
            b"\xed\xa0\x80".as_slice(),
            b"\xed\xb0\x80",
            b"\xff",
            b"\xc0\x80",
            b"\xf4\x90\x80\x80",
        ] {
            let value = OwnedPyObject::from_owned(strings::PyUnicode_FromStringAndSize(
                invalid.as_ptr().cast(),
                invalid.len() as isize,
            ));
            let matches =
                errors::PyErr_ExceptionMatches((&raw mut PyExc_UnicodeDecodeError).cast());
            let _error = errors::take_current_error();
            assert!(value.as_ptr().is_null());
            assert_eq!(matches, 1);
            errors::PyErr_Clear();
        }
        let valid = b"caf\xc3\xa9\0";
        assert_eq!(
            take_raw(strings::PyUnicode_FromStringAndSize(
                valid.as_ptr().cast(),
                valid.len() as isize
            )),
            valid
        );
        let data = strings::PyUnicode_AsUTF8AndSize(pair, ptr::null_mut());
        let matches = errors::PyErr_ExceptionMatches((&raw mut PyExc_UnicodeEncodeError).cast());
        let error = errors::take_current_error();
        assert!(data.is_null());
        assert_eq!(matches, 1);
        drop(error);
        errors::PyErr_Clear();
        let encoded = OwnedPyObject::from_owned(strings::PyUnicode_AsUTF8String(pair));
        let matches = errors::PyErr_ExceptionMatches((&raw mut PyExc_UnicodeEncodeError).cast());
        let error = errors::take_current_error();
        assert!(encoded.as_ptr().is_null());
        assert_eq!(matches, 1);
        drop(error);
        errors::PyErr_Clear();
        // AsEncodedString now delegates to the real runtime codec; its error
        // policies are exercised by cpython_abi_hooks::unicode, not stub hooks.
        let encoded = OwnedPyObject::from_owned(strings::PyUnicode_AsASCIIString(pair));
        let matches = errors::PyErr_ExceptionMatches((&raw mut PyExc_UnicodeEncodeError).cast());
        let error = errors::take_current_error();
        assert!(encoded.as_ptr().is_null());
        assert_eq!(matches, 1);
        drop(error);
        errors::PyErr_Clear();
        let encoded = OwnedPyObject::from_owned(strings::PyUnicode_AsLatin1String(pair));
        let matches = errors::PyErr_ExceptionMatches((&raw mut PyExc_UnicodeEncodeError).cast());
        let error = errors::take_current_error();
        assert!(encoded.as_ptr().is_null());
        assert_eq!(matches, 1);
        drop(error);
        errors::PyErr_Clear();

        // Allocation interception and malformed internal-byte publication are
        // covered by the ABI allocation-boundary test without replacing hooks.
        drop(pair_owner);
        drop(high_owner);
        drop(low_owner);
        drop(payload_owner);
        TEXT_PAYLOAD_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
    }
}
