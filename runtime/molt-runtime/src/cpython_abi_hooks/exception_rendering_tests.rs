//! Real foreign exception slots with callback rebinding and physical presence.

use super::native_test_fixture::NativeType;
use crate::BuiltinExc;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::refcount::OwnedPyObject;
use molt_cpython_abi::api::{errors, numbers, object, refcount, sequences, strings, typeobj};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

static OWNER: AtomicPtr<PyBaseExceptionObject> = AtomicPtr::new(ptr::null_mut());
static FAILURE: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static DROPS: AtomicUsize = AtomicUsize::new(0);
static DROP_OBSERVES: AtomicBool = AtomicBool::new(false);
static DROP_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static DROP_TRACEBACK: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
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
        DROP_OBSERVES.store(false, Ordering::SeqCst);
        DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        DROP_TRACEBACK.store(ptr::null_mut(), Ordering::SeqCst);
        TEXT_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
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
        for counter in [
            &PERCENT_INT_CALLS,
            &PERCENT_INDEX_CALLS,
            &PERCENT_FLOAT_CALLS,
            &PERCENT_LEN_CALLS,
            &PERCENT_MOD_CALLS,
            &PERCENT_DROPS,
        ] {
            counter.store(0, Ordering::SeqCst);
        }
        PERCENT_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_DROP_SAW_OVERFLOW.store(false, Ordering::SeqCst);
        PERCENT_MANAGED_NATIVE_RESULT.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_KIND.store(0, Ordering::SeqCst);
        PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_OWNER.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_NAME.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_REMOVE.store(false, Ordering::SeqCst);
        PERCENT_BIND_FAIL.store(false, Ordering::SeqCst);
        PERCENT_BOUND_BOOL_SLOT.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_RESULT_KIND.store(0, Ordering::SeqCst);
        PERCENT_BOUND_RESULT_DROPS.store(0, Ordering::SeqCst);
        PERCENT_BOUND_RESULT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_INDEX_DROPS.store(0, Ordering::SeqCst);
        PERCENT_BOUND_INDEX_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_REFLECTED_LEFT.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_REFLECTED_MISSING.store(false, Ordering::SeqCst);
        LOOKUP_HASH_SEEN_PRIOR.store(ptr::null_mut(), Ordering::SeqCst);
        COMPLETION_RESULT.store(ptr::null_mut(), Ordering::SeqCst);
        COMPLETION_SEEN.store(ptr::null_mut(), Ordering::SeqCst);
        COMPLETION_FAILS.store(false, Ordering::SeqCst);
        COMPLETION_CALLS.store(0, Ordering::SeqCst);
        LOOKUP_HASH_MODE.store(0, Ordering::SeqCst);
        LOOKUP_HASH_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        LOOKUP_HASH_TRACEBACK.store(ptr::null_mut(), Ordering::SeqCst);
        NOTE_REPR_FAILS.store(false, Ordering::SeqCst);
        NOTE_REPR_SAW_CLEAR.store(false, Ordering::SeqCst);
        NOTE_OVERRIDE_CALLS.store(0, Ordering::SeqCst);
        NOTE_MUTATED_TRACEBACK.store(ptr::null_mut(), Ordering::SeqCst);
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
    if DROP_OBSERVES.load(Ordering::SeqCst) {
        unsafe {
            let error = errors::PyErr_GetRaisedException();
            DROP_ERROR.store(error, Ordering::SeqCst);
            let trace = if error.is_null() {
                ptr::null_mut()
            } else {
                errors::PyException_GetTraceback(error)
            };
            DROP_TRACEBACK.store(trace, Ordering::SeqCst);
            refcount::Py_XDECREF(trace);
            errors::PyErr_SetRaisedException(error);
        }
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
                let value =
                    crate::builtins::exceptions::alloc_exception(&py, BuiltinExc::IndexError, "");
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
        DROP_OBSERVES.store(true, Ordering::SeqCst);
        let drops = DROPS.load(Ordering::SeqCst);
        errors::PyErr_SetNone(class_ptr);
        let raised =
            errors::take_current_error().expect("invalid result's cleanup preserves TypeError");
        assert_eq!(raised.exc_type, (&raw mut PyExc_TypeError).cast());
        assert_eq!(DROPS.load(Ordering::SeqCst), drops + 1);
        assert_eq!(DROP_ERROR.load(Ordering::SeqCst), raised.value);
        drop(raised);
        DROP_OBSERVES.store(false, Ordering::SeqCst);
        CONSTRUCTOR_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);

        // Restore and explicit Normalize steal their input. Retiring a last
        // owner observes the constructor failure before normalization fetches it.
        CONSTRUCTOR_RETURNS_NONE.store(false, Ordering::SeqCst);
        DROP_OBSERVES.store(true, Ordering::SeqCst);
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
            assert_eq!(DROP_ERROR.load(Ordering::SeqCst), failure);
            assert_eq!(DROPS.load(Ordering::SeqCst), drops + 1);
            drop(raised);
        }
        DROP_OBSERVES.store(false, Ordering::SeqCst);

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
        DROP_OBSERVES.store(true, Ordering::SeqCst);
        errors::PyErr_SetString(errors::PyErr_Occurred(), c"replacement".as_ptr());
        let replaced = errors::take_current_error().expect("replacement survives legal finalizer");
        assert!(
            DROP_ERROR.load(Ordering::SeqCst).is_null(),
            "constructor path clears the old indicator before calling"
        );
        DROP_OBSERVES.store(false, Ordering::SeqCst);
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
        let _initializing = object::RuntimeInitializationThreadStateGuard::enter(
            std::ptr::from_ref(&_callbacks).expose_provenance(),
        );
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
        DROP_OBSERVES.store(true, Ordering::SeqCst);
        let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *payload_type,
        })));
        let error_owner = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
        let error = error_owner.as_ptr();
        drop(payload);
        OWNER.store(error.cast(), Ordering::SeqCst);
        let rendered = OwnedPyObject::from_owned(typeobj::PyObject_Str(error));
        let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(rendered.as_ptr().is_null());
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        assert!(!raised.as_ptr().is_null(), "callback exception");
        assert_eq!(
            raised.as_ptr(),
            failure,
            "cleanup preserves the exact callback exception"
        );
        assert_eq!(DROP_ERROR.load(Ordering::SeqCst), failure);
        DROP_OBSERVES.store(false, Ordering::SeqCst);
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
        raised_exception_transfer_cases(&_py);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

/// CPython's `PyType_Ready` returns 0 for a ready type whatever the error
/// indicator holds, and leaves the caller's exception in place.
#[test]
fn type_ready_on_a_ready_type_keeps_the_callers_exception() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let lookup_error = &raw mut PyExc_LookupError;
        assert_eq!(typeobj::PyType_Ready(lookup_error), 0);
        let pending = exception(&raw mut PyExc_ValueError, &[]);
        errors::PyErr_SetRaisedException(object::Py_NewRef(pending.as_ptr()));
        assert_eq!(typeobj::PyType_Ready(lookup_error), 0);
        let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(raised.as_ptr(), pending.as_ptr(), "the caller's exception");
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

unsafe fn raised_exception_transfer_cases(py: &crate::PyToken<'_>) {
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

        // A legal native deallocator sees the selected next exception (or
        // NULL on clear), then restores that exact state through the public API.
        let mut payload_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"RaisedCleanupPayload");
        payload_type.tp_dealloc = Some(payload_drop);
        assert_eq!(payload_type.ready(), 0);
        // A failed assertion must retire any pending payload before its native
        // type storage leaves scope, even when the transfer itself is defective.
        struct RaisedCleanup;
        impl Drop for RaisedCleanup {
            fn drop(&mut self) {
                DROP_OBSERVES.store(false, Ordering::SeqCst);
                unsafe { errors::PyErr_Clear() };
            }
        }
        let _cleanup = RaisedCleanup;
        DROP_OBSERVES.store(true, Ordering::SeqCst);
        let drops_before = DROPS.load(Ordering::SeqCst);
        let trace_class = crate::obj_from_bits(crate::builtin_classes(py).traceback)
            .as_ptr()
            .unwrap();
        let trace_bits = crate::alloc_instance_for_class(py, trace_class);
        let trace = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits));
        assert!(!trace.as_ptr().is_null());
        for runtime_old in [false, true] {
            for clear in [false, true] {
                let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut *payload_type,
                })));
                let old = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
                drop(payload);
                // Readiness and construction are fixture setup, not part of the
                // raised-state transfer. Finish them before installing the old
                // indicator; only SetRaisedException runs with that error pending.
                let next = if clear {
                    OwnedPyObject::from_owned(ptr::null_mut())
                } else {
                    exception(&raw mut PyExc_LookupError, &[])
                };
                if !next.as_ptr().is_null() {
                    assert_eq!(
                        errors::PyException_SetTraceback(next.as_ptr(), trace.as_ptr()),
                        0
                    );
                }
                assert!(errors::PyErr_Occurred().is_null());
                assert!(
                    GLOBAL_BRIDGE.molt_handle_for_pyobj(old.as_ptr()).is_none(),
                    "physical native old exception"
                );
                errors::PyErr_SetRaisedException(old.into_ptr());
                if runtime_old {
                    assert!(super::transfer_pending_cpython_exception());
                    let bits =
                        crate::exception_last_bits_noinc(py).expect("runtime-owned old state");
                    assert_eq!(
                        crate::object::object_type_id(crate::obj_from_bits(bits).as_ptr().unwrap()),
                        crate::TYPE_ID_FOREIGN
                    );
                }
                let next = next.into_ptr();
                errors::PyErr_SetRaisedException(next);
                let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(
                    raised.as_ptr(),
                    next,
                    "native finalizer preserves the caller's selected state"
                );
                assert_eq!(
                    DROP_ERROR.load(Ordering::SeqCst),
                    next,
                    "old owner retires after new state is visible"
                );
                assert_eq!(
                    DROP_TRACEBACK.load(Ordering::SeqCst),
                    if clear {
                        ptr::null_mut()
                    } else {
                        trace.as_ptr()
                    }
                );
                if !raised.as_ptr().is_null() {
                    let actual_trace = OwnedPyObject::from_owned(errors::PyException_GetTraceback(
                        raised.as_ptr(),
                    ));
                    assert_eq!(actual_trace.as_ptr(), trace.as_ptr());
                }
                assert!(errors::PyErr_Occurred().is_null());
                drop(raised);
            }
        }
        assert_eq!(DROPS.load(Ordering::SeqCst), drops_before + 4);
        DROP_OBSERVES.store(false, Ordering::SeqCst);
    }
}

// CallbackState resets this fixture state inside RuntimeTestTransaction's serial scope.
static TEXT_CALLBACK_FAILS: AtomicBool = AtomicBool::new(false);
static TEXT_PAYLOAD_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static TEXT_DROPS: AtomicUsize = AtomicUsize::new(0);
static TEXT_DROP_RAISES: AtomicBool = AtomicBool::new(false);
static TEXT_DROP_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
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
    unsafe {
        let error = errors::PyErr_GetRaisedException();
        TEXT_DROP_ERROR.store(error, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(error);
        drop(Box::from_raw(value));
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
        let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(rendered.as_ptr().is_null());
        assert!(!raised.as_ptr().is_null(), "callback failure preserved");
        assert_eq!(raised.as_ptr(), failure);
        assert_eq!(TEXT_DROPS.load(Ordering::SeqCst), 3);
        assert_eq!(TEXT_DROP_ERROR.load(Ordering::SeqCst), failure);
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

// CPython Objects/unicodeobject.c: PyUnicode_Format, mainformatlong and
// formatchar. These literals also replace the former four fake-hook formatter
// tests: every successful output below crosses the production runtime hook.
unsafe fn percent_tuple(
    values: &[*mut PyObject],
    native: Option<*mut PyTypeObject>,
) -> OwnedPyObject {
    unsafe {
        let result = OwnedPyObject::from_owned(match native {
            Some(class) => {
                molt_cpython_abi::api::memory::_PyObject_GC_NewVar(class, values.len() as isize)
                    .cast()
            }
            None => sequences::PyTuple_New(values.len() as isize),
        });
        assert!(!result.as_ptr().is_null());
        if native.is_some() {
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(result.as_ptr())
                    .is_none(),
                "fixture must exercise physical tuple storage"
            );
        }
        for (index, value) in values.iter().enumerate() {
            assert_eq!(
                sequences::PyTuple_SetItem(
                    result.as_ptr(),
                    index as isize,
                    object::Py_NewRef(*value)
                ),
                0
            );
        }
        result
    }
}

unsafe fn percent_raw_long(class: *mut PyTypeObject, digits: &[u32]) -> OwnedPyObject {
    unsafe {
        let result = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            class,
            digits.len().max(1) as isize,
        ));
        if result.as_ptr().is_null() {
            return result;
        }
        let integer = result.as_ptr().cast::<PyLongObject>();
        (*integer).long_value.lv_tag = if digits.is_empty() {
            1
        } else {
            digits.len() << 3
        };
        let tail = (&raw mut (*integer).long_value.ob_digit).cast::<u32>();
        for (index, digit) in digits.iter().enumerate() {
            tail.add(index).write(*digit);
        }
        result
    }
}

unsafe fn percent_error(
    template: &[u8],
    arg: *mut PyObject,
    class: *mut PyTypeObject,
    message: &[u8],
) {
    unsafe {
        let result = OwnedPyObject::from_owned(percent(template, arg));
        let matched = errors::PyErr_ExceptionMatches(class.cast());
        let failure = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!failure.as_ptr().is_null(), "formatter error indicator");
        assert!(
            result.as_ptr().is_null(),
            "invalid format unexpectedly succeeded"
        );
        assert_eq!(matched, 1);
        assert_eq!(take_raw(typeobj::PyObject_Str(failure.as_ptr())), message);
    }
}

#[test]
fn unicode_percent_format_capi_uses_runtime_numeric_authority() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let a = OwnedPyObject::from_owned(raw_fixture(b"ab"));
        let x = OwnedPyObject::from_owned(raw_fixture(b"xyz"));
        let seven = OwnedPyObject::from_owned(numbers::PyLong_FromLong(7));
        let ff = OwnedPyObject::from_owned(numbers::PyLong_FromLong(255));
        let pi = OwnedPyObject::from_owned(numbers::PyFloat_FromDouble("3.14159".parse().unwrap()));
        let hundred = OwnedPyObject::from_owned(numbers::PyFloat_FromDouble(100.0));
        let half = OwnedPyObject::from_owned(numbers::PyFloat_FromDouble(0.5));
        for native in [None, Some(&raw mut PyTuple_Type)] {
            let args = percent_tuple(
                &[a.as_ptr(), seven.as_ptr(), x.as_ptr(), ff.as_ptr()],
                native,
            );
            assert_eq!(
                take_raw(percent(b"[%5s][%-4d][%.2s][%#x]", args.as_ptr())),
                b"[   ab][7   ][xy][0xff]"
            );
            let args = percent_tuple(&[pi.as_ptr(), hundred.as_ptr(), half.as_ptr()], native);
            assert_eq!(
                take_raw(percent(b"%.2f|%e|%g", args.as_ptr())),
                b"3.14|1.000000e+02|0.5"
            );
            let args = percent_tuple(&[a.as_ptr()], native);
            percent_error(
                b"%s %s",
                args.as_ptr(),
                &raw mut PyExc_TypeError,
                b"not enough arguments for format string",
            );
            let args = percent_tuple(&[a.as_ptr(), x.as_ptr()], native);
            percent_error(
                b"%s",
                args.as_ptr(),
                &raw mut PyExc_TypeError,
                b"not all arguments converted during string formatting",
            );
            let empty = percent_tuple(&[], native);
            assert_eq!(
                take_raw(percent(b"literal %%", empty.as_ptr())),
                b"literal %"
            );
            percent_error(
                b"%(name)s",
                empty.as_ptr(),
                &raw mut PyExc_TypeError,
                b"format requires a mapping",
            );
        }
        percent_error(
            b"%d",
            a.as_ptr(),
            &raw mut PyExc_TypeError,
            b"%d format: a real number is required, not str",
        );
        // A freshly allocated PyLong is a boxed numeric identity, not necessarily
        // a NaN-box immediate. Both it and genuine unregistered C storage work.
        let boxed = OwnedPyObject::from_owned(numbers::PyLong_FromLong(0xdc00));
        let physical = percent_raw_long(&raw mut PyLong_Type, &[0xdc00]);
        assert!(!physical.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(physical.as_ptr())
                .is_none()
        );
        for number in [boxed.as_ptr(), physical.as_ptr()] {
            for (format, expected) in [
                (b"%c".as_slice(), b"\xed\xb0\x80".as_slice()),
                (b"%d", b"56320"),
                (b"%i", b"56320"),
                (b"%u", b"56320"),
                (b"%o", b"156000"),
                (b"%x", b"dc00"),
                (b"%X", b"DC00"),
                (b"%#010x", b"0x0000dc00"),
            ] {
                assert_eq!(take_raw(percent(format, number)), expected);
            }
        }
        // 2**160 + 123 proves the former i128 boundary is gone. The physical
        // fixture contains six public base-2**30 digits, not a formatter mirror.
        let huge = OwnedPyObject::from_owned(numbers::PyLong_FromString(
            c"1461501637330902918203684832716283019655932543099".as_ptr(),
            ptr::null_mut(),
            10,
        ));
        let native_huge = percent_raw_long(&raw mut PyLong_Type, &[123, 0, 0, 0, 0, 1024]);
        assert!(!native_huge.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(native_huge.as_ptr())
                .is_none()
        );
        for number in [huge.as_ptr(), native_huge.as_ptr()] {
            assert_eq!(
                take_raw(percent(b"%d", number)),
                b"1461501637330902918203684832716283019655932543099"
            );
            assert_eq!(
                take_raw(percent(b"%#x", number)),
                b"0x1000000000000000000000000000000000000007b"
            );
            percent_error(
                b"%c",
                number,
                &raw mut PyExc_OverflowError,
                b"%c arg not in range(0x110000)",
            );
        }
        let width = percent_raw_long(&raw mut PyLong_Type, &[6]);
        let precision = percent_raw_long(&raw mut PyLong_Type, &[4]);
        let args = percent_tuple(
            &[width.as_ptr(), precision.as_ptr(), seven.as_ptr()],
            Some(&raw mut PyTuple_Type),
        );
        assert_eq!(take_raw(percent(b"%*.*d", args.as_ptr())), b"  0007");
        let big_precision = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(2147483648));
        let args = percent_tuple(&[big_precision.as_ptr(), seven.as_ptr()], None);
        percent_error(
            b"%.*d",
            args.as_ptr(),
            &raw mut PyExc_OverflowError,
            b"Python int too large to convert to C int",
        );
        let args = percent_tuple(
            &[huge.as_ptr(), seven.as_ptr()],
            Some(&raw mut PyTuple_Type),
        );
        percent_error(
            b"%*d",
            args.as_ptr(),
            &raw mut PyExc_OverflowError,
            b"Python int too large to convert to C ssize_t",
        );
        percent_error(
            b"%.2147483648d",
            seven.as_ptr(),
            &raw mut PyExc_ValueError,
            b"precision too big",
        );
        let negative = OwnedPyObject::from_owned(numbers::PyLong_FromLong(-1));
        percent_error(
            b"%c",
            negative.as_ptr(),
            &raw mut PyExc_OverflowError,
            b"%c arg not in range(0x110000)",
        );
        for (value, kind, expected) in [
            (
                f64::NAN,
                &raw mut PyExc_ValueError,
                b"cannot convert float NaN to integer".as_slice(),
            ),
            (
                f64::INFINITY,
                &raw mut PyExc_OverflowError,
                b"cannot convert float infinity to integer".as_slice(),
            ),
        ] {
            let value = OwnedPyObject::from_owned(numbers::PyFloat_FromDouble(value));
            percent_error(b"%d", value.as_ptr(), kind, expected);
        }
        assert!(!crate::exception_pending(py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

static PERCENT_INT_CALLS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_INDEX_CALLS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_FLOAT_CALLS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_LEN_CALLS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_MOD_CALLS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_RESULT_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_DROPS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_DROP_SAW_OVERFLOW: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn percent_int(_value: *mut PyObject) -> *mut PyObject {
    PERCENT_INT_CALLS.fetch_add(1, Ordering::SeqCst);
    unsafe { numbers::PyLong_FromLong(23) }
}
unsafe extern "C" fn percent_index(_value: *mut PyObject) -> *mut PyObject {
    PERCENT_INDEX_CALLS.fetch_add(1, Ordering::SeqCst);
    if TEXT_CALLBACK_FAILS.load(Ordering::SeqCst) {
        unsafe {
            errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast(),
                FAILURE.load(Ordering::SeqCst),
            );
        }
        return ptr::null_mut();
    }
    let class = PERCENT_RESULT_TYPE.load(Ordering::SeqCst);
    unsafe {
        if class.is_null() {
            numbers::PyLong_FromLong(42)
        } else {
            percent_raw_long(class, &[42]).into_ptr()
        }
    }
}
unsafe extern "C" fn percent_float(_value: *mut PyObject) -> *mut PyObject {
    PERCENT_FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
    unsafe { numbers::PyFloat_FromDouble(9.5) }
}
unsafe extern "C" fn percent_tuple_len(_value: *mut PyObject) -> isize {
    PERCENT_LEN_CALLS.fetch_add(1, Ordering::SeqCst);
    99
}
unsafe extern "C" fn percent_long_drop(value: *mut PyObject) {
    PERCENT_DROPS.fetch_add(1, Ordering::SeqCst);
    let pending = unsafe { errors::PyErr_ExceptionMatches((&raw mut PyExc_OverflowError).cast()) };
    PERCENT_DROP_SAW_OVERFLOW.store(pending != 0, Ordering::SeqCst);
    unsafe {
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
    }
}
extern "C" fn percent_managed_mod(_self: u64, _rhs: u64) -> u64 {
    PERCENT_MOD_CALLS.fetch_add(1, Ordering::SeqCst);
    crate::with_gil_entry_nopanic!(py, {
        crate::MoltObject::from_ptr(crate::alloc_string(py, b"override")).bits()
    })
}

#[test]
fn unicode_percent_format_capi_preserves_native_conversion_and_tuple_protocols() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut slots: PyNumberMethods = std::mem::zeroed();
        slots.nb_int = percent_int as *const () as *mut _;
        slots.nb_index = percent_index as *const () as *mut _;
        slots.nb_float = percent_float as *const () as *mut _;
        let mut integer_type = NativeType::subtype(&raw mut PyLong_Type, c"percent.NativeInteger");
        integer_type.tp_as_number = (&raw mut slots).cast();
        assert_eq!(integer_type.ready(), 0);
        let integer = percent_raw_long(&raw mut *integer_type, &[17]);
        assert!(!integer.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(integer.as_ptr())
                .is_none()
        );
        for (format, expected) in [
            (b"%d".as_slice(), b"17".as_slice()),
            (b"%x", b"11"),
            (b"%c", b"\x11"),
        ] {
            assert_eq!(take_raw(percent(format, integer.as_ptr())), expected);
        }
        let one = OwnedPyObject::from_owned(numbers::PyLong_FromLong(1));
        let args = percent_tuple(&[integer.as_ptr(), one.as_ptr()], None);
        assert_eq!(
            take_raw(percent(b"%*d", args.as_ptr())),
            b"                1"
        );
        assert_eq!(PERCENT_INT_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(PERCENT_INDEX_CALLS.load(Ordering::SeqCst), 0);

        let mut float_slots: PyNumberMethods = std::mem::zeroed();
        float_slots.nb_int = percent_int as *const () as *mut _;
        float_slots.nb_float = percent_float as *const () as *mut _;
        let mut float_type = NativeType::subtype(&raw mut PyFloat_Type, c"percent.NativeFloat");
        float_type.tp_as_number = (&raw mut float_slots).cast();
        assert_eq!(float_type.ready(), 0);
        let float =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *float_type, 0));
        assert!(!float.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(float.as_ptr())
                .is_none()
        );
        (*float.as_ptr().cast::<PyFloatObject>()).ob_fval = 1.25;
        assert_eq!(take_raw(percent(b"%.2f", float.as_ptr())), b"1.25");
        assert_eq!(PERCENT_FLOAT_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(take_raw(percent(b"%d", float.as_ptr())), b"23");
        assert_eq!(PERCENT_INT_CALLS.load(Ordering::SeqCst), 1);

        let mut protocol_slots: PyNumberMethods = std::mem::zeroed();
        protocol_slots.nb_int = percent_int as *const () as *mut _;
        protocol_slots.nb_index = percent_index as *const () as *mut _;
        protocol_slots.nb_float = percent_float as *const () as *mut _;
        let mut protocol_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.Protocol");
        protocol_type.tp_as_number = (&raw mut protocol_slots).cast();
        assert_eq!(protocol_type.ready(), 0);
        let value =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *protocol_type, 0));
        assert_eq!(take_raw(percent(b"%d", value.as_ptr())), b"23");
        assert_eq!(take_raw(percent(b"%x", value.as_ptr())), b"2a");
        assert_eq!(take_raw(percent(b"%c", value.as_ptr())), b"*");
        assert_eq!(take_raw(percent(b"%.1f", value.as_ptr())), b"9.5");
        let before = PERCENT_INDEX_CALLS.load(Ordering::SeqCst);
        let args = percent_tuple(&[value.as_ptr(), one.as_ptr()], None);
        percent_error(
            b"%*d",
            args.as_ptr(),
            &raw mut PyExc_TypeError,
            b"* wants int",
        );
        assert_eq!(PERCENT_INDEX_CALLS.load(Ordering::SeqCst), before);
        let failure = exception(&raw mut PyExc_LookupError, &[]);
        FAILURE.store(failure.as_ptr(), Ordering::SeqCst);
        TEXT_CALLBACK_FAILS.store(true, Ordering::SeqCst);
        for template in [b"%x".as_slice(), b"%c"] {
            let result = OwnedPyObject::from_owned(percent(template, value.as_ptr()));
            let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(result.as_ptr().is_null());
            assert_eq!(error.as_ptr(), failure.as_ptr());
        }
        TEXT_CALLBACK_FAILS.store(false, Ordering::SeqCst);
        FAILURE.store(ptr::null_mut(), Ordering::SeqCst);

        let mut returned_type =
            NativeType::subtype(&raw mut PyLong_Type, c"percent.ReturnedInteger");
        returned_type.tp_dealloc = Some(percent_long_drop);
        assert_eq!(returned_type.ready(), 0);
        PERCENT_RESULT_TYPE.store(&raw mut *returned_type, Ordering::SeqCst);
        assert_eq!(take_raw(percent(b"%6x", value.as_ptr())), b"    2a");
        assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
        assert!(!PERCENT_DROP_SAW_OVERFLOW.load(Ordering::SeqCst));
        percent_error(
            b"%.2147483645x",
            value.as_ptr(),
            &raw mut PyExc_OverflowError,
            b"precision too large",
        );
        assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 2);
        assert!(PERCENT_DROP_SAW_OVERFLOW.load(Ordering::SeqCst));
        PERCENT_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);

        let mut tuple_slots: PySequenceMethods = std::mem::zeroed();
        tuple_slots.sq_length = percent_tuple_len as *const () as *mut _;
        let mut tuple_type = NativeType::subtype(&raw mut PyTuple_Type, c"percent.NativeTuple");
        tuple_type.tp_as_sequence = (&raw mut tuple_slots).cast();
        assert_eq!(tuple_type.ready(), 0);
        let args = percent_tuple(
            &[one.as_ptr(), integer.as_ptr()],
            Some(&raw mut *tuple_type),
        );
        assert_eq!(take_raw(percent(b"%d %d", args.as_ptr())), b"1 17");
        percent_error(
            b"%(key)s",
            args.as_ptr(),
            &raw mut PyExc_TypeError,
            b"format requires a mapping",
        );
        percent_error(
            b"literal",
            args.as_ptr(),
            &raw mut PyExc_TypeError,
            b"not all arguments converted during string formatting",
        );
        assert_eq!(PERCENT_LEN_CALLS.load(Ordering::SeqCst), 0);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn unicode_percent_format_declaring_slot_bypasses_format_subclass_mod() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let name = crate::attr_name_bits_from_bytes(py, b"PercentFormatSubclass").unwrap();
        let _name_owner =
            crate::PtrDropGuard::preserving(crate::obj_from_bits(name).as_ptr().unwrap());
        let class = crate::molt_class_new(name);
        let _class_owner =
            crate::PtrDropGuard::preserving(crate::obj_from_bits(class).as_ptr().unwrap());
        let base_result = crate::molt_class_set_base(class, crate::builtin_classes(py).str);
        crate::dec_ref_bits(py, base_result);
        let method_name = crate::attr_name_bits_from_bytes(py, b"__mod__").unwrap();
        let _method_name_owner =
            crate::PtrDropGuard::preserving(crate::obj_from_bits(method_name).as_ptr().unwrap());
        let callback = crate::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "percent_managed_mod",
                percent_managed_mod as *const (),
            ),
            2,
        );
        assert!(!callback.is_null());
        let _callback_owner = crate::PtrDropGuard::preserving(callback);
        let callback_bits = crate::MoltObject::from_ptr(callback).bits();
        let set_result = crate::molt_set_attr_name(class, method_name, callback_bits);
        crate::dec_ref_bits(py, set_result);
        crate::object::class_finish_definition(py, crate::obj_from_bits(class).as_ptr().unwrap())
            .unwrap();
        let template = OwnedPyObject::from_owned(raw_fixture(b"value=%d"));
        let input = GLOBAL_BRIDGE
            .molt_handle_for_pyobj(template.as_ptr())
            .unwrap()
            .bits();
        let subclass = crate::call::bind::call_bind_borrowed(py, class, None, &[input], &[], &[]);
        let subclass_owner =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(subclass));
        assert!(!subclass_owner.as_ptr().is_null());
        assert_eq!(crate::type_of_bits(py, subclass), class);
        let value = OwnedPyObject::from_owned(numbers::PyLong_FromLong(5000));
        assert_eq!(
            take_raw(strings::PyUnicode_Format(
                subclass_owner.as_ptr(),
                value.as_ptr()
            )),
            b"value=5000"
        );
        assert_eq!(PERCENT_MOD_CALLS.load(Ordering::SeqCst), 0);
        let rendered = crate::molt_mod(subclass, crate::MoltObject::from_int(5000).bits());
        assert_eq!(
            take_raw(GLOBAL_BRIDGE.owned_handle_to_pyobj(rendered)),
            b"override"
        );
        assert_eq!(PERCENT_MOD_CALLS.load(Ordering::SeqCst), 1);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn diagnostic_type_names_use_current_heap_qualname_and_module() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let mut slots = [PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        }];
        let mut spec = PyType_Spec {
            name: c"stale.Widget".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE) as u32,
            slots: slots.as_mut_ptr(),
        };
        let class = OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&raw mut spec));
        assert!(!class.as_ptr().is_null());
        let qualname = OwnedPyObject::from_owned(raw_fixture(b"Outer.Inner"));
        assert_eq!(
            object::PyObject_SetAttrString(
                class.as_ptr(),
                c"__qualname__".as_ptr(),
                qualname.as_ptr()
            ),
            0
        );
        let instance =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class.as_ptr().cast(), 0));
        for (module, expected, alternate) in [
            (
                b"pkg".as_slice(),
                b"pkg.Outer.Inner".as_slice(),
                b"pkg:Outer.Inner".as_slice(),
            ),
            (b"__main__", b"Outer.Inner", b"Outer.Inner"),
            (b"builtins", b"Outer.Inner", b"Outer.Inner"),
            (b"", b".Outer.Inner", b":Outer.Inner"),
            (
                b"pkg\xed\xa0\x80",
                b"pkg\xed\xa0\x80.Outer.Inner",
                b"pkg\xed\xa0\x80:Outer.Inner",
            ),
        ] {
            let module = OwnedPyObject::from_owned(raw_fixture(module));
            assert_eq!(
                object::PyObject_SetAttrString(
                    class.as_ptr(),
                    c"__module__".as_ptr(),
                    module.as_ptr()
                ),
                0
            );
            assert_eq!(
                take_raw(errors::molt_capi_type_fully_qualified_name(
                    class.as_ptr().cast(),
                    b'.'
                )),
                expected
            );
            assert_eq!(
                take_raw(errors::molt_capi_type_fully_qualified_name(
                    class.as_ptr().cast(),
                    b':'
                )),
                alternate
            );
            let bits = GLOBAL_BRIDGE
                .molt_value_for_pyobj(instance.as_ptr())
                .unwrap();
            let _owner = crate::obj_from_bits(bits)
                .as_ptr()
                .map(crate::PtrDropGuard::preserving);
            assert_eq!(
                crate::object::ops_format::format_diagnostic_type_name_bytes(
                    py,
                    crate::obj_from_bits(bits)
                ),
                expected
            );
            if crate::object::ops_sys::runtime_target_minor(py) >= 14 {
                let mut message = b"%c requires an int or a unicode character, not ".to_vec();
                message.extend_from_slice(expected);
                percent_error(b"%c", instance.as_ptr(), &raw mut PyExc_TypeError, &message);
            }
        }
        let module = OwnedPyObject::from_owned(raw_fixture(b"pkg"));
        assert_eq!(
            object::PyObject_SetAttrString(class.as_ptr(), c"__module__".as_ptr(), module.as_ptr()),
            0
        );
        assert_eq!(
            take_raw(errors::PyUnicode_FromFormat(
                c"%T|%#T|%N|%#N".as_ptr(),
                instance.as_ptr(),
                instance.as_ptr(),
                class.as_ptr(),
                class.as_ptr()
            )),
            b"pkg.Outer.Inner|pkg:Outer.Inner|pkg.Outer.Inner|pkg:Outer.Inner"
        );
        percent_error(
            b"%d",
            instance.as_ptr(),
            &raw mut PyExc_TypeError,
            b"%d format: a real number is required, not stale.Widget",
        );
        let message = if crate::object::ops_sys::runtime_target_minor(py) < 14 {
            b"%c requires int or char".as_slice()
        } else {
            b"%c requires an int or a unicode character, not pkg.Outer.Inner".as_slice()
        };
        percent_error(b"%c", instance.as_ptr(), &raw mut PyExc_TypeError, message);
        assert_eq!(
            object::PyObject_SetAttrString(
                class.as_ptr(),
                c"__module__".as_ptr(),
                &raw mut Py_None
            ),
            0
        );
        assert_eq!(
            take_raw(errors::molt_capi_type_fully_qualified_name(
                class.as_ptr().cast(),
                b'.'
            )),
            b"Outer.Inner"
        );
        assert!(errors::PyErr_Occurred().is_null());
    });
}

extern "C" fn percent_managed_int(_self: u64) -> u64 {
    PERCENT_INT_CALLS.fetch_add(1, Ordering::SeqCst);
    crate::MoltObject::from_int(23).bits()
}
extern "C" fn percent_managed_index(_self: u64) -> u64 {
    PERCENT_INDEX_CALLS.fetch_add(1, Ordering::SeqCst);
    crate::MoltObject::from_int(42).bits()
}
extern "C" fn percent_managed_float(_self: u64) -> u64 {
    PERCENT_FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
    crate::MoltObject::from_float(9.5).bits()
}
extern "C" fn percent_managed_trunc(_self: u64) -> u64 {
    PERCENT_INT_CALLS.fetch_add(1, Ordering::SeqCst);
    crate::MoltObject::from_int(99).bits()
}

fn percent_managed_protocol<'a, 'py>(
    py: &'a crate::PyToken<'py>,
    base: u64,
    methods: &[(&[u8], extern "C" fn(u64) -> u64)],
    arguments: &[u64],
) -> (
    crate::builtins::exceptions::ExceptionValue<'a, 'py>,
    crate::builtins::exceptions::ExceptionValue<'a, 'py>,
) {
    use crate::builtins::exceptions::ExceptionValue;
    let name = ExceptionValue::adopt(
        py,
        crate::attr_name_bits_from_bytes(py, b"ManagedPercentProtocol").unwrap(),
    );
    let class = ExceptionValue::adopt(py, crate::molt_class_new(name.bits()));
    let result = crate::molt_class_set_base(class.bits(), base);
    crate::dec_ref_bits(py, result);
    for (name, callback) in methods {
        let name = ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, name).unwrap());
        let function = crate::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "percent_managed_protocol",
                *callback as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(function).bits());
        let result = crate::molt_set_attr_name(class.bits(), name.bits(), function.bits());
        crate::dec_ref_bits(py, result);
    }
    unsafe {
        crate::object::class_finish_definition(
            py,
            crate::obj_from_bits(class.bits()).as_ptr().unwrap(),
        )
    }
    .unwrap();
    let instance = ExceptionValue::adopt(py, unsafe {
        crate::call::bind::call_bind_borrowed(py, class.bits(), None, arguments, &[], &[])
    });
    assert!(!crate::exception_pending(py));
    (class, instance)
}

#[test]
fn unicode_percent_format_managed_numeric_protocols_share_canonical_conversion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let (_class, instance) = percent_managed_protocol(
            py,
            crate::builtin_classes(py).object,
            &[
                (b"__int__", percent_managed_int),
                (b"__index__", percent_managed_index),
            ],
            &[],
        );
        let view =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(instance.bits()));
        assert!(!view.as_ptr().is_null());
        assert_eq!(take_raw(percent(b"%d", view.as_ptr())), b"23");
        assert_eq!(take_raw(percent(b"%x", view.as_ptr())), b"2a");
        assert_eq!(take_raw(percent(b"%c", view.as_ptr())), b"*");
        assert_eq!(PERCENT_INT_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(PERCENT_INDEX_CALLS.load(Ordering::SeqCst), 2);
        let args = percent_tuple(&[view.as_ptr(), view.as_ptr()], None);
        percent_error(
            b"%*d",
            args.as_ptr(),
            &raw mut PyExc_TypeError,
            b"* wants int",
        );
        assert_eq!(PERCENT_INDEX_CALLS.load(Ordering::SeqCst), 2);
        let (_trunc_class, trunc) = percent_managed_protocol(
            py,
            crate::builtin_classes(py).object,
            &[(b"__trunc__", percent_managed_trunc)],
            &[],
        );
        let trunc_view =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(trunc.bits()));
        assert!(!trunc_view.as_ptr().is_null());
        percent_error(
            b"%d",
            trunc_view.as_ptr(),
            &raw mut PyExc_TypeError,
            b"%d format: a real number is required, not ManagedPercentProtocol",
        );
        assert_eq!(
            PERCENT_INT_CALLS.load(Ordering::SeqCst),
            1,
            "PyNumber_Check must reject __trunc__-only values before conversion"
        );
        use crate::builtins::exceptions::ExceptionValue;
        let text = ExceptionValue::adopt(
            py,
            crate::MoltObject::from_ptr(crate::alloc_string(py, b"17")).bits(),
        );
        let bytes = ExceptionValue::adopt(
            py,
            crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"17")).bits(),
        );
        for (base, input) in [
            (crate::builtin_classes(py).str, text.bits()),
            (crate::builtin_classes(py).bytes, bytes.bits()),
            (crate::builtin_classes(py).bytearray, bytes.bits()),
        ] {
            let (_class, value) = percent_managed_protocol(
                py,
                base,
                &[(b"__float__", percent_managed_float)],
                &[input],
            );
            let view =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value.bits()));
            assert!(!view.as_ptr().is_null());
            assert_eq!(take_raw(percent(b"%d", view.as_ptr())), b"17");
        }
        assert_eq!(
            PERCENT_FLOAT_CALLS.load(Ordering::SeqCst),
            0,
            "numeric admission does not invoke __float__"
        );
        assert!(errors::PyErr_Occurred().is_null());
    });
}

static PERCENT_MANAGED_NATIVE_RESULT: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_MANAGED_FACTORY_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_MANAGED_FACTORY_KIND: AtomicUsize = AtomicUsize::new(0);
static PERCENT_MANAGED_DROP_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_BOUND_OWNER: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_BOUND_NAME: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_BOUND_REMOVE: AtomicBool = AtomicBool::new(false);
static PERCENT_BIND_FAIL: AtomicBool = AtomicBool::new(false);
static PERCENT_BOUND_BOOL_SLOT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(ptr::null_mut());
static PERCENT_BOUND_RESULT_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_BOUND_RESULT_KIND: AtomicUsize = AtomicUsize::new(0);
static PERCENT_BOUND_RESULT_DROPS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_BOUND_RESULT_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_BOUND_INDEX_DROPS: AtomicUsize = AtomicUsize::new(0);
static PERCENT_BOUND_INDEX_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_REFLECTED_LEFT: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static PERCENT_REFLECTED_MISSING: AtomicBool = AtomicBool::new(false);

/// Return a genuinely native object from a managed Python special method.
/// No conversion is performed here: the production shared validator owns it.
extern "C" fn percent_managed_native_result(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let borrowed = PERCENT_MANAGED_NATIVE_RESULT.load(Ordering::SeqCst);
            let temporary = if borrowed.is_null() {
                let class = PERCENT_MANAGED_FACTORY_TYPE.load(Ordering::SeqCst);
                if class.is_null() {
                    return crate::raise_exception::<_>(
                        py,
                        BuiltinExc::SystemError,
                        "native numeric fixture has no result",
                    );
                }
                match PERCENT_MANAGED_FACTORY_KIND.load(Ordering::SeqCst) {
                    1 => {
                        let value =
                            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class, 0));
                        if !value.as_ptr().is_null() {
                            (*value.as_ptr().cast::<PyFloatObject>()).ob_fval = 4.25;
                        }
                        value
                    }
                    2 => {
                        let mut digits = [0u32; 138];
                        digits[137] = 1;
                        percent_raw_long(class, &digits)
                    }
                    _ => percent_raw_long(class, &[5000]),
                }
            } else {
                OwnedPyObject::from_owned(ptr::null_mut())
            };
            let value = if borrowed.is_null() {
                temporary.as_ptr()
            } else {
                borrowed
            };
            match GLOBAL_BRIDGE.molt_value_for_pyobj(value) {
                Some(bits) => bits,
                None => {
                    super::propagate_native_failure(py, "native numeric callback fixture");
                    crate::MoltObject::none().bits()
                }
            }
        }
    })
}

unsafe extern "C" fn percent_managed_numeric_drop(value: *mut PyObject) {
    PERCENT_DROPS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        let error = errors::PyErr_GetRaisedException();
        PERCENT_MANAGED_DROP_ERROR.store(error, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(error);
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
        if TEXT_DROP_RAISES.load(Ordering::SeqCst) {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"numeric result cleanup".as_ptr(),
            );
        }
    }
}

struct PercentWarningPolicy;
impl Drop for PercentWarningPolicy {
    fn drop(&mut self) {
        crate::molt_warnings_resetwarnings();
    }
}
fn percent_warning_policy(py: &crate::PyToken<'_>, action: &[u8]) {
    use crate::builtins::exceptions::ExceptionValue;
    crate::molt_warnings_resetwarnings();
    let action = ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, action).unwrap());
    let result = crate::molt_warnings_simplefilter(
        action.bits(),
        crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::DeprecationWarning),
        crate::MoltObject::from_int(0).bits(),
        crate::MoltObject::from_bool(false).bits(),
    );
    crate::dec_ref_bits(py, result);
    assert!(!crate::exception_pending(py));
}

#[test]
fn managed_numeric_callbacks_accept_native_owners_and_preserve_public_identity() {
    use crate::builtins::{exceptions::ExceptionValue, numbers as numeric};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let _warnings = PercentWarningPolicy;
        percent_warning_policy(py, b"ignore");
        assert_eq!(
            crate::c_api::molt_int_as_i64(crate::MoltObject::from_float(42.0).bits()),
            42,
            "direct integral float carriers retain the existing public extractor domain"
        );
        let (_producer_class, producer) = percent_managed_protocol(
            py,
            crate::builtin_classes(py).object,
            &[
                (b"__int__", percent_managed_native_result),
                (b"__index__", percent_managed_native_result),
            ],
            &[],
        );
        let producer_view =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(producer.bits()));
        assert!(!producer_view.as_ptr().is_null());
        let mut slots: PyNumberMethods = std::mem::zeroed();
        slots.nb_int = percent_int as *const () as *mut _;
        slots.nb_index = percent_index as *const () as *mut _;
        let mut subtype = NativeType::subtype(&raw mut PyLong_Type, c"percent.NativeInteger");
        subtype.tp_as_number = (&raw mut slots).cast();
        assert_eq!(subtype.ready(), 0);
        let exact = percent_raw_long(&raw mut PyLong_Type, &[5000]);
        let subclass = percent_raw_long(&raw mut *subtype, &[5000]);
        for (native, is_exact) in [(exact.as_ptr(), true), (subclass.as_ptr(), false)] {
            assert!(!native.is_null());
            assert!(GLOBAL_BRIDGE.molt_handle_for_pyobj(native).is_none());
            let native_bits =
                ExceptionValue::adopt(py, GLOBAL_BRIDGE.molt_value_for_pyobj(native).unwrap());
            assert_eq!(
                crate::object_type_id(crate::obj_from_bits(native_bits.bits()).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN
            );
            assert!(numeric::index_integral_payload_bits(native_bits.bits()).is_none());
            assert!(numeric::index_i64_integral_bits(native_bits.bits()).is_none());
            assert!(numeric::index_bigint_integral_bits(native_bits.bits()).is_none());
            PERCENT_MANAGED_NATIVE_RESULT.store(native, Ordering::SeqCst);
            for (format, expected) in [
                (b"%d".as_slice(), b"5000".as_slice()),
                (b"%x", b"1388"),
                (b"%c", b"\xe1\x8e\x88"),
            ] {
                assert_eq!(take_raw(percent(format, producer_view.as_ptr())), expected);
            }
            let private =
                ExceptionValue::adopt(py, numeric::index_from_object(py, producer.bits()));
            let private_view = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(private.bits()),
            );
            assert_eq!(private_view.as_ptr(), native);
            assert_eq!(
                numeric::index_bigint_from_obj(py, producer.bits(), "index expected"),
                Some(num_bigint::BigInt::from(5000))
            );
            assert_eq!(
                numeric::sequence_index_i64(py, producer.bits(), "list"),
                Some(5000)
            );
            for result in [
                crate::molt_int_from_obj(producer.bits(), crate::MoltObject::none().bits(), 0),
                crate::molt_operator_index(producer.bits()),
            ] {
                let result = ExceptionValue::adopt(py, result);
                let view = OwnedPyObject::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(result.bits()),
                );
                assert!(!view.as_ptr().is_null());
                assert_eq!(numbers::PyLong_CheckExact(view.as_ptr()), 1);
                assert_eq!(numbers::PyLong_AsLongLong(view.as_ptr()), 5000);
                assert_eq!(view.as_ptr() == native, is_exact);
                assert_eq!(crate::c_api::molt_int_as_i64(result.bits()), 5000);
            }
            let direct =
                ExceptionValue::adopt(py, numeric::index_from_object(py, native_bits.bits()));
            assert_eq!(
                direct.bits(),
                native_bits.bits(),
                "private index ignores native subtype overrides"
            );
            let direct_int = ExceptionValue::adopt(
                py,
                crate::molt_int_from_obj(native_bits.bits(), crate::MoltObject::none().bits(), 0),
            );
            assert_eq!(
                numeric::integer_storage_i64(py, direct_int.bits()),
                Some(if is_exact { 5000 } else { 23 })
            );
            let (_int_class, managed_subclass) = percent_managed_protocol(
                py,
                crate::builtin_classes(py).int,
                &[],
                &[producer.bits()],
            );
            assert_eq!(
                numeric::index_i64_integral_bits(managed_subclass.bits()),
                Some(5000),
                "sealed int subclass fields require managed carriers"
            );
        }
        assert_eq!(PERCENT_INDEX_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(PERCENT_INT_CALLS.load(Ordering::SeqCst), 1);
        PERCENT_MANAGED_NATIVE_RESULT.store(ptr::null_mut(), Ordering::SeqCst);

        let (_float_producer_class, float_producer) = percent_managed_protocol(
            py,
            crate::builtin_classes(py).object,
            &[(b"__float__", percent_managed_native_result)],
            &[],
        );
        let float_producer_view = OwnedPyObject::from_owned(
            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(float_producer.bits()),
        );
        assert!(!float_producer_view.as_ptr().is_null());
        let mut float_slots: PyNumberMethods = std::mem::zeroed();
        float_slots.nb_float = percent_float as *const () as *mut _;
        let mut float_type = NativeType::subtype(&raw mut PyFloat_Type, c"percent.NativeFloat");
        float_type.tp_as_number = (&raw mut float_slots).cast();
        assert_eq!(float_type.ready(), 0);
        for (class, is_exact) in [(&raw mut PyFloat_Type, true), (&raw mut *float_type, false)] {
            let native = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class, 0));
            assert!(!native.as_ptr().is_null());
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(native.as_ptr())
                    .is_none()
            );
            (*native.as_ptr().cast::<PyFloatObject>()).ob_fval = 4.25;
            let native_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap(),
            );
            assert_eq!(
                crate::object_type_id(crate::obj_from_bits(native_bits.bits()).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN
            );
            PERCENT_MANAGED_NATIVE_RESULT.store(native.as_ptr(), Ordering::SeqCst);
            assert_eq!(
                take_raw(percent(b"%.2f", float_producer_view.as_ptr())),
                b"4.25"
            );
            assert_eq!(
                numeric::float_as_double(py, float_producer.bits()),
                Some(4.25)
            );
            let converted =
                ExceptionValue::adopt(py, crate::molt_float_from_obj(float_producer.bits()));
            let converted_view = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(converted.bits()),
            );
            assert!(!converted_view.as_ptr().is_null());
            assert_eq!(numbers::PyFloat_CheckExact(converted_view.as_ptr()), 1);
            assert_eq!(numbers::PyFloat_AsDouble(converted_view.as_ptr()), 4.25);
            assert_eq!(converted_view.as_ptr() == native.as_ptr(), is_exact);
            assert_eq!(crate::c_api::molt_float_as_f64(converted.bits()), 4.25);
            let capi =
                OwnedPyObject::from_owned(molt_cpython_abi::api::abstract_number::PyNumber_Float(
                    float_producer_view.as_ptr(),
                ));
            assert!(!capi.as_ptr().is_null());
            assert_eq!(capi.as_ptr() == native.as_ptr(), is_exact);
            let complex = ExceptionValue::adopt(
                py,
                crate::molt_complex_from_obj(
                    float_producer.bits(),
                    crate::MoltObject::none().bits(),
                    crate::MoltObject::from_bool(false).bits(),
                ),
            );
            let parts =
                numeric::complex_ref(numeric::complex_ptr_from_bits(complex.bits()).unwrap());
            assert_eq!((parts.re, parts.im), (4.25, 0.0));
            let (_float_class, managed_subclass) = percent_managed_protocol(
                py,
                crate::builtin_classes(py).float,
                &[],
                &[float_producer.bits()],
            );
            assert_eq!(
                crate::object::ops::as_float_extended(crate::obj_from_bits(
                    managed_subclass.bits()
                )),
                Some(4.25)
            );
            assert_eq!(numeric::float_as_double(py, native_bits.bits()), Some(4.25));
            let direct = ExceptionValue::adopt(py, crate::molt_float_from_obj(native_bits.bits()));
            assert_eq!(
                numeric::float_storage_value(py, direct.bits()),
                Ok(Some(if is_exact { 4.25 } else { 9.5 }))
            );
            PERCENT_MANAGED_NATIVE_RESULT.store(ptr::null_mut(), Ordering::SeqCst);
        }
        assert_eq!(
            PERCENT_FLOAT_CALLS.load(Ordering::SeqCst),
            1,
            "only direct float(subtype) invokes its override"
        );
        // Invalid native results retain the real native type in diagnostics.
        PERCENT_MANAGED_NATIVE_RESULT.store(exact.as_ptr(), Ordering::SeqCst);
        percent_error(
            b"%f",
            float_producer_view.as_ptr(),
            &raw mut PyExc_TypeError,
            b"ManagedPercentProtocol.__float__ returned non-float (type int)",
        );
        let wrong_float =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyFloat_Type, 0));
        assert!(!wrong_float.as_ptr().is_null());
        (*wrong_float.as_ptr().cast::<PyFloatObject>()).ob_fval = 4.25;
        PERCENT_MANAGED_NATIVE_RESULT.store(wrong_float.as_ptr(), Ordering::SeqCst);
        percent_error(
            b"%x",
            producer_view.as_ptr(),
            &raw mut PyExc_TypeError,
            b"%x format: an integer is required, not ManagedPercentProtocol",
        );
        percent_error(
            b"%d",
            producer_view.as_ptr(),
            &raw mut PyExc_TypeError,
            b"%d format: a real number is required, not ManagedPercentProtocol",
        );
        PERCENT_MANAGED_NATIVE_RESULT.store(ptr::null_mut(), Ordering::SeqCst);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn managed_native_numeric_result_cleanup_preserves_warning_and_conversion_errors() {
    use crate::builtins::exceptions::ExceptionValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let _warnings = PercentWarningPolicy;
        let mut integer = NativeType::subtype(&raw mut PyLong_Type, c"percent.ReturnedInteger");
        integer.tp_dealloc = Some(percent_managed_numeric_drop);
        assert_eq!(integer.ready(), 0);
        let mut float = NativeType::subtype(&raw mut PyFloat_Type, c"percent.ReturnedFloat");
        float.tp_dealloc = Some(percent_managed_numeric_drop);
        assert_eq!(float.ready(), 0);
        for (method, class, kind, label) in [
            (
                b"__int__".as_slice(),
                &raw mut *integer,
                0,
                "__int__ returned non-int (type percent.ReturnedInteger).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.",
            ),
            (
                b"__index__",
                &raw mut *integer,
                0,
                "__index__ returned non-int (type percent.ReturnedInteger).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.",
            ),
            (
                b"__float__",
                &raw mut *float,
                1,
                "ManagedPercentProtocol.__float__ returned non-float (type percent.ReturnedFloat).  The ability to return an instance of a strict subclass of float is deprecated, and may be removed in a future version of Python.",
            ),
        ] {
            let (_class, producer) = percent_managed_protocol(
                py,
                crate::builtin_classes(py).object,
                &[(method, percent_managed_native_result)],
                &[],
            );
            let view = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(producer.bits()),
            );
            assert!(!view.as_ptr().is_null());
            PERCENT_MANAGED_FACTORY_TYPE.store(class, Ordering::SeqCst);
            PERCENT_MANAGED_FACTORY_KIND.store(kind, Ordering::SeqCst);
            percent_warning_policy(py, b"error");
            for constructor in [false, true] {
                PERCENT_DROPS.store(0, Ordering::SeqCst);
                PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
                if constructor {
                    let result = ExceptionValue::adopt(
                        py,
                        match method {
                            b"__int__" => crate::molt_int_from_obj(
                                producer.bits(),
                                crate::MoltObject::none().bits(),
                                0,
                            ),
                            b"__index__" => crate::molt_operator_index(producer.bits()),
                            _ => crate::molt_float_from_obj(producer.bits()),
                        },
                    );
                    assert!(crate::obj_from_bits(result.bits()).is_none());
                } else {
                    let template = match method {
                        b"__int__" => b"%d",
                        b"__index__" => b"%x",
                        _ => b"%f",
                    };
                    let result = OwnedPyObject::from_owned(percent(template, view.as_ptr()));
                    assert!(result.as_ptr().is_null());
                }
                let matched =
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_DeprecationWarning).cast());
                let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!error.as_ptr().is_null());
                assert_eq!(matched, 1);
                assert_eq!(
                    take_raw(typeobj::PyObject_Str(error.as_ptr())),
                    label.as_bytes()
                );
                assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
                assert_eq!(
                    error.as_ptr(),
                    PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst)
                );
                TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
            }
        }
        percent_warning_policy(py, b"ignore");
        PERCENT_MANAGED_FACTORY_TYPE.store(&raw mut *integer, Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_KIND.store(2, Ordering::SeqCst);
        let (_class, producer) = percent_managed_protocol(
            py,
            crate::builtin_classes(py).object,
            &[(b"__index__", percent_managed_native_result)],
            &[],
        );
        let view =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(producer.bits()));
        assert!(!view.as_ptr().is_null());
        for conversion in 0..4 {
            PERCENT_DROPS.store(0, Ordering::SeqCst);
            PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
            if conversion == 0 {
                let result = ExceptionValue::adopt(py, crate::molt_float_from_obj(producer.bits()));
                assert!(crate::obj_from_bits(result.bits()).is_none());
            } else {
                let result = OwnedPyObject::from_owned(percent(
                    match conversion {
                        1 => b"%g",
                        2 => b"%.2147483645x",
                        _ => b"%c",
                    },
                    view.as_ptr(),
                ));
                assert!(result.as_ptr().is_null());
            }
            let matched = errors::PyErr_ExceptionMatches((&raw mut PyExc_OverflowError).cast());
            let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!error.as_ptr().is_null());
            assert_eq!(matched, 1);
            let expected = match conversion {
                0 | 1 => "int too large to convert to float",
                2 => "precision too large",
                _ => "%c arg not in range(0x110000)",
            };
            assert_eq!(
                take_raw(typeobj::PyObject_Str(error.as_ptr())),
                expected.as_bytes()
            );
            assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
            if conversion == 3 {
                assert!(
                    PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst).is_null(),
                    "%c retires index before creating its range error"
                );
            } else {
                assert_eq!(
                    error.as_ptr(),
                    PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst)
                );
            }
            TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
        }
        PERCENT_MANAGED_FACTORY_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn native_numeric_retirement_completes_error_transport_and_keeps_shielding_explicit() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let _warnings = PercentWarningPolicy;
        percent_warning_policy(py, b"ignore");
        let mut returned = NativeType::subtype(&raw mut PyLong_Type, c"percent.RetiredInteger");
        returned.tp_dealloc = Some(percent_managed_numeric_drop);
        assert_eq!(returned.ready(), 0);
        PERCENT_MANAGED_FACTORY_TYPE.store(&raw mut *returned, Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_KIND.store(0, Ordering::SeqCst);
        let failure = exception(&raw mut PyExc_LookupError, &[]);
        let trace_class = crate::obj_from_bits(crate::builtin_classes(py).traceback)
            .as_ptr()
            .unwrap();
        let trace_bits = crate::alloc_instance_for_class(py, trace_class);
        let traceback = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits));
        assert!(!traceback.as_ptr().is_null());
        assert_eq!(
            errors::PyException_SetTraceback(failure.as_ptr(), traceback.as_ptr()),
            0
        );
        for shielded in [false, true] {
            let victim = percent_managed_native_result(0);
            let victim_ptr = crate::obj_from_bits(victim).as_ptr().unwrap();
            let victim_owner = crate::PtrDropGuard::new(victim_ptr);
            assert_eq!(crate::object_type_id(victim_ptr), crate::TYPE_ID_FOREIGN);
            PERCENT_DROPS.store(0, Ordering::SeqCst);
            PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            errors::PyErr_SetRaisedException(object::Py_NewRef(failure.as_ptr()));
            super::propagate_native_failure(py, "retirement error fixture");
            assert!(crate::exception_pending(py));
            // A valid deallocator observes and restores the incoming error.
            // Explicit shielding separately suppresses hostile cleanup errors.
            TEXT_DROP_RAISES.store(shielded, Ordering::SeqCst);
            if shielded {
                errors::with_preserved_error(|| drop(victim_owner));
            } else {
                drop(victim_owner);
            }
            assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
            assert_eq!(
                PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst),
                if shielded {
                    ptr::null_mut()
                } else {
                    failure.as_ptr()
                }
            );
            assert!(
                crate::exception_pending(py),
                "native Get/Set roundtrip must complete into runtime custody"
            );
            let restored = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(restored.as_ptr(), failure.as_ptr());
            let restored_trace =
                OwnedPyObject::from_owned(errors::PyException_GetTraceback(restored.as_ptr()));
            assert_eq!(restored_trace.as_ptr(), traceback.as_ptr());
            TEXT_DROP_RAISES.store(false, Ordering::SeqCst);
        }
        // A raw C index result is a separate owned temporary from a foreign
        // wrapper, and must cross the same native retirement completion boundary.
        let mut slots: PyNumberMethods = std::mem::zeroed();
        slots.nb_index = percent_index as *const () as *mut _;
        let mut producer_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.RetirementProducer");
        producer_type.tp_as_number = (&raw mut slots).cast();
        assert_eq!(producer_type.ready(), 0);
        let producer =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *producer_type, 0));
        assert!(!producer.as_ptr().is_null());
        PERCENT_RESULT_TYPE.store(&raw mut *returned, Ordering::SeqCst);
        PERCENT_DROPS.store(0, Ordering::SeqCst);
        PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        let result = OwnedPyObject::from_owned(percent(b"%.2147483645x", producer.as_ptr()));
        let matched = errors::PyErr_ExceptionMatches((&raw mut PyExc_OverflowError).cast());
        let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(result.as_ptr().is_null());
        assert!(!error.as_ptr().is_null());
        assert_eq!(matched, 1);
        assert_eq!(
            take_raw(typeobj::PyObject_Str(error.as_ptr())),
            b"precision too large"
        );
        assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
        assert_eq!(
            error.as_ptr(),
            PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst)
        );
        PERCENT_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

unsafe extern "C" fn percent_bound_call(
    _self: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    CONSTRUCTOR_CALLS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        if PERCENT_BOUND_REMOVE.load(Ordering::SeqCst)
            && object::PyObject_SetAttr(
                PERCENT_BOUND_OWNER.load(Ordering::SeqCst),
                PERCENT_BOUND_NAME.load(Ordering::SeqCst),
                ptr::null_mut(),
            ) < 0
        {
            return ptr::null_mut();
        }
        if TEXT_CALLBACK_FAILS.load(Ordering::SeqCst) {
            errors::PyErr_SetRaisedException(object::Py_NewRef(FAILURE.load(Ordering::SeqCst)));
            ptr::null_mut()
        } else {
            let class = PERCENT_BOUND_RESULT_TYPE.load(Ordering::SeqCst);
            if class.is_null() {
                object::Py_NewRef(&raw mut Py_None)
            } else if PERCENT_BOUND_RESULT_KIND.load(Ordering::SeqCst) == 1 {
                let result = percent_raw_long(class, &[1]);
                if !result.as_ptr().is_null() {
                    (*result.as_ptr().cast::<PyLongObject>()).long_value.lv_tag |= 2;
                }
                result.into_ptr()
            } else {
                typeobj::PyType_GenericAlloc(class, 0)
            }
        }
    }
}

unsafe extern "C" fn percent_bound_result_drop(value: *mut PyObject) {
    PERCENT_BOUND_RESULT_DROPS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        let error = errors::PyErr_GetRaisedException();
        PERCENT_BOUND_RESULT_ERROR.store(error, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(error);
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
    }
}

static PERCENT_BOUND_DESCRIPTOR_DROPS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn percent_bound_descriptor_drop(value: *mut PyObject) {
    PERCENT_BOUND_DESCRIPTOR_DROPS.fetch_add(1, Ordering::SeqCst);
    unsafe { percent_managed_numeric_drop(value) };
}

unsafe extern "C" fn percent_bound_index_drop(value: *mut PyObject) {
    PERCENT_BOUND_INDEX_DROPS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        let error = errors::PyErr_GetRaisedException();
        PERCENT_BOUND_INDEX_ERROR.store(error, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(error);
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
    }
}

unsafe extern "C" fn percent_bound_index(_value: *mut PyObject) -> *mut PyObject {
    unsafe {
        percent_raw_long(
            PERCENT_RESULT_TYPE.load(Ordering::SeqCst),
            &[0, 0, 0, 0, 0, 1024],
        )
        .into_ptr()
    }
}

unsafe extern "C" fn percent_bound_get(
    _self: *mut PyObject,
    _instance: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if PERCENT_BIND_FAIL.load(Ordering::SeqCst) {
            if object::PyObject_SetAttr(
                PERCENT_BOUND_OWNER.load(Ordering::SeqCst),
                PERCENT_BOUND_NAME.load(Ordering::SeqCst),
                ptr::null_mut(),
            ) < 0
            {
                return ptr::null_mut();
            }
            errors::PyErr_SetRaisedException(object::Py_NewRef(FAILURE.load(Ordering::SeqCst)));
            return ptr::null_mut();
        }
        // The returned reference is the only owner of this bound callable.
        typeobj::PyType_GenericAlloc(PERCENT_MANAGED_FACTORY_TYPE.load(Ordering::SeqCst), 0)
    }
}

// These heap classes can retain instances of local static fixture types. Cut
// their namespace/MRO cycles while those declarations still live, including
// during assertion unwind; the ordinary field owner then releases the class.
struct PercentBoundClass(OwnedPyObject);

impl PercentBoundClass {
    fn as_ptr(&self) -> *mut PyObject {
        self.0.as_ptr()
    }
}

impl Drop for PercentBoundClass {
    fn drop(&mut self) {
        unsafe { typeobj::molt_type_clear(self.0.as_ptr()) };
    }
}

unsafe fn percent_bound_class(
    metaclass: *mut PyTypeObject,
    bases: *mut PyObject,
) -> PercentBoundClass {
    let mut slots = [
        PyType_Slot {
            slot: molt_cpython_abi::type_slots::Py_tp_new,
            pfunc: typeobj::PyType_GenericNew as *const () as *mut std::ffi::c_void,
        },
        PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        },
    ];
    let mut spec = PyType_Spec {
        name: c"percent.BoundOwner".as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as i32,
        itemsize: 0,
        flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
        slots: slots.as_mut_ptr(),
    };
    let class = PercentBoundClass(unsafe {
        OwnedPyObject::from_owned(typeobj::PyType_FromMetaclass(
            metaclass,
            ptr::null_mut(),
            &raw mut spec,
            bases,
        ))
    });
    assert!(!class.as_ptr().is_null());
    class
}

// Distinct public consumers select distinct generated dispatch authorities.
unsafe fn percent_bound_operation(
    class: *mut PyObject,
    receiver: *mut PyObject,
    kind: usize,
) -> bool {
    use molt_cpython_abi::api::{abstract_number, abstract_sequence};
    unsafe {
        let pointer = match kind {
            0 => return numbers::PyComplex_AsCComplex(receiver).real == -1.0,
            1 => object::PyObject_Bytes(receiver),
            2 => object::PyObject_Format(receiver, ptr::null_mut()),
            3 => return object::PyObject_LengthHint(receiver, 7) == -1,
            4 => {
                let mut args = [receiver];
                object::PyObject_VectorcallMethod(
                    PERCENT_BOUND_NAME.load(Ordering::SeqCst),
                    args.as_mut_ptr(),
                    1,
                    ptr::null_mut(),
                )
            }
            5 | 19 => abstract_number::PyNumber_Long(receiver),
            6 => typeobj::PyObject_Repr(receiver),
            7 => {
                let slot = typeobj::PyType_GetSlot(
                    class.cast(),
                    molt_cpython_abi::type_slots::Py_am_await,
                );
                assert!(!slot.is_null());
                let invoke: unsafe extern "C" fn(*mut PyObject) -> *mut PyObject =
                    std::mem::transmute(slot);
                invoke(receiver)
            }
            8 => return object::PyObject_IsTrue(receiver) == -1,
            10 => {
                let slot = PERCENT_BOUND_BOOL_SLOT.load(Ordering::SeqCst);
                assert!(!slot.is_null());
                let invoke: unsafe extern "C" fn(*mut PyObject) -> std::os::raw::c_int =
                    std::mem::transmute(slot);
                return invoke(receiver) == -1;
            }
            9 => return object::PyObject_Size(receiver) == -1,
            11 => return typeobj::PyObject_Hash(receiver) == -1,
            12 => return abstract_sequence::PySequence_Contains(receiver, &raw mut Py_None) == -1,
            13 => object::PyObject_GetAttrString(receiver, c"missing_after_binding".as_ptr()),
            14 => object::PyObject_GetIter(receiver),
            15 => object::PyObject_CallNoArgs(receiver),
            16 => {
                let args = OwnedPyObject::from_owned(sequences::PyTuple_New(0));
                assert!(!args.as_ptr().is_null());
                return (*class.cast::<PyTypeObject>())
                    .tp_init
                    .expect("generated init")(
                    receiver, args.as_ptr(), ptr::null_mut()
                ) == -1;
            }
            17 => object::PyObject_CallNoArgs(class),
            18 => (*class.cast::<PyTypeObject>())
                .tp_descr_get
                .expect("generated raw descriptor")(
                receiver, &raw mut Py_None, class
            ),
            _ => unreachable!("bound invocation fixture"),
        };
        let result = OwnedPyObject::from_owned(pointer);
        result.as_ptr().is_null()
    }
}

#[test]
fn native_bound_callable_retirement_preserves_error_identity_and_validation_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let _warnings = PercentWarningPolicy;
        percent_warning_policy(py, b"ignore");
        let minor = crate::object::ops_sys::runtime_target_minor(py);
        let failure = exception(&raw mut PyExc_LookupError, &[]);
        FAILURE.store(failure.as_ptr(), Ordering::SeqCst);
        let mut callable =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.BoundCallable");
        callable.tp_call = Some(percent_bound_call);
        callable.tp_dealloc = Some(percent_managed_numeric_drop);
        assert_eq!(callable.ready(), 0);
        let mut descriptor_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.BoundDescriptor");
        descriptor_type.tp_descr_get = Some(percent_bound_get);
        descriptor_type.tp_dealloc = Some(percent_bound_descriptor_drop);
        assert_eq!(descriptor_type.ready(), 0);
        PERCENT_MANAGED_FACTORY_TYPE.store(&raw mut *callable, Ordering::SeqCst);
        let mut result_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.BoundResult");
        result_type.tp_dealloc = Some(percent_bound_result_drop);
        assert_eq!(result_type.ready(), 0);
        let mut negative_type = NativeType::subtype(&raw mut PyLong_Type, c"percent.NegativeHint");
        negative_type.tp_dealloc = Some(percent_bound_result_drop);
        assert_eq!(negative_type.ready(), 0);
        let mut index_slots: PyNumberMethods = std::mem::zeroed();
        index_slots.nb_index = percent_bound_index as *const () as *mut _;
        let mut index_source_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.LengthIndexSource");
        index_source_type.tp_as_number = (&raw mut index_slots).cast();
        index_source_type.tp_dealloc = Some(percent_bound_result_drop);
        assert_eq!(index_source_type.ready(), 0);
        let mut index_result_type =
            NativeType::subtype(&raw mut PyLong_Type, c"percent.LengthIndexResult");
        index_result_type.tp_dealloc = Some(percent_bound_index_drop);
        assert_eq!(index_result_type.ready(), 0);
        let methods = [
            c"__complex__",
            c"__bytes__",
            c"__format__",
            c"__length_hint__",
            c"ordinary_method",
            c"__int__",
            c"__repr__",
            c"__await__",
            c"__bool__",
            c"__len__",
            c"__len__",
            c"__hash__",
            c"__contains__",
            c"__getattr__",
            c"__iter__",
            c"__call__",
            c"__init__",
            c"__new__",
            c"__get__",
            c"__trunc__",
        ];
        for (kind, method) in methods.into_iter().enumerate() {
            if kind == 19 && minor >= 14 {
                continue;
            }
            PERCENT_BOUND_DESCRIPTOR_DROPS.store(0, Ordering::SeqCst);
            let class = percent_bound_class(ptr::null_mut(), ptr::null_mut());
            let class_address = class.as_ptr().addr();
            let receiver =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class.as_ptr().cast(), 0));
            assert!(!receiver.as_ptr().is_null());
            let name = OwnedPyObject::from_owned(strings::PyUnicode_FromString(method.as_ptr()));
            assert!(!name.as_ptr().is_null());
            PERCENT_BOUND_OWNER.store(class.as_ptr(), Ordering::SeqCst);
            PERCENT_BOUND_NAME.store(name.as_ptr(), Ordering::SeqCst);
            let declaration_type = if kind == 18 {
                &raw mut *callable
            } else {
                &raw mut *descriptor_type
            };
            let declaration =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(declaration_type, 0));
            assert!(!declaration.as_ptr().is_null());
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), name.as_ptr(), declaration.as_ptr()),
                0
            );
            if kind == 10 {
                assert_eq!(
                    object::PyObject_SetAttrString(
                        class.as_ptr(),
                        c"__bool__".as_ptr(),
                        declaration.as_ptr()
                    ),
                    0
                );
                let slot = typeobj::PyType_GetSlot(
                    class.as_ptr().cast(),
                    molt_cpython_abi::type_slots::Py_nb_bool,
                );
                assert!(!slot.is_null());
                PERCENT_BOUND_BOOL_SLOT.store(slot, Ordering::SeqCst);
                assert_eq!(
                    object::PyObject_SetAttrString(
                        class.as_ptr(),
                        c"__bool__".as_ptr(),
                        ptr::null_mut()
                    ),
                    0
                );
            }
            drop(declaration);
            assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 0);
            // The raw __get__ callable removes its namespace entry during its
            // call; the invocation retain is then its sole owner.
            PERCENT_BOUND_REMOVE.store(kind == 18, Ordering::SeqCst);
            TEXT_CALLBACK_FAILS.store(true, Ordering::SeqCst);
            CONSTRUCTOR_CALLS.store(0, Ordering::SeqCst);
            PERCENT_DROPS.store(0, Ordering::SeqCst);
            PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            assert!(
                percent_bound_operation(class.as_ptr(), receiver.as_ptr(), kind),
                "{method:?}"
            );
            let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(error.as_ptr(), failure.as_ptr(), "{method:?}");
            assert_eq!(CONSTRUCTOR_CALLS.load(Ordering::SeqCst), 1, "{method:?}");
            assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1, "{method:?}");
            assert_eq!(
                PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst),
                failure.as_ptr(),
                "{method:?}"
            );
            if matches!(kind, 2 | 3 | 5 | 8 | 9 | 11 | 19) {
                TEXT_CALLBACK_FAILS.store(false, Ordering::SeqCst);
                PERCENT_DROPS.store(0, Ordering::SeqCst);
                PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                assert!(
                    percent_bound_operation(class.as_ptr(), receiver.as_ptr(), kind),
                    "invalid {method:?}"
                );
                let matched = errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast());
                let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!error.as_ptr().is_null());
                assert_eq!(matched, 1, "invalid {method:?}");
                assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
                let expected = if kind == 8 && minor < 14 {
                    error.as_ptr()
                } else {
                    ptr::null_mut()
                };
                assert_eq!(
                    PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst),
                    expected,
                    "validation {method:?}"
                );
            }
            if matches!(kind, 2 | 3 | 8 | 9 | 11) {
                // These callback results have no fixture owner. Their distinct
                // finalizer observes validation independently of the callable.
                TEXT_CALLBACK_FAILS.store(false, Ordering::SeqCst);
                PERCENT_BOUND_RESULT_TYPE.store(&raw mut *result_type, Ordering::SeqCst);
                PERCENT_BOUND_RESULT_KIND.store(0, Ordering::SeqCst);
                PERCENT_DROPS.store(0, Ordering::SeqCst);
                PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                PERCENT_BOUND_RESULT_DROPS.store(0, Ordering::SeqCst);
                PERCENT_BOUND_RESULT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                assert!(percent_bound_operation(
                    class.as_ptr(),
                    receiver.as_ptr(),
                    kind
                ));
                let matched = errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast());
                let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!error.as_ptr().is_null());
                assert_eq!(matched, 1);
                assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
                assert_eq!(PERCENT_BOUND_RESULT_DROPS.load(Ordering::SeqCst), 1);
                assert_eq!(
                    PERCENT_BOUND_RESULT_ERROR.load(Ordering::SeqCst),
                    if kind == 11 {
                        ptr::null_mut()
                    } else {
                        error.as_ptr()
                    }
                );
                assert_eq!(
                    PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst),
                    if kind == 8 && minor < 14 {
                        error.as_ptr()
                    } else {
                        ptr::null_mut()
                    }
                );
                if kind == 3 || kind == 9 {
                    PERCENT_BOUND_RESULT_TYPE.store(
                        if kind == 3 {
                            &raw mut *negative_type
                        } else {
                            &raw mut *index_source_type
                        },
                        Ordering::SeqCst,
                    );
                    PERCENT_BOUND_RESULT_KIND
                        .store(if kind == 3 { 1 } else { 0 }, Ordering::SeqCst);
                    PERCENT_RESULT_TYPE.store(&raw mut *index_result_type, Ordering::SeqCst);
                    PERCENT_BOUND_RESULT_DROPS.store(0, Ordering::SeqCst);
                    PERCENT_BOUND_RESULT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                    PERCENT_BOUND_INDEX_DROPS.store(0, Ordering::SeqCst);
                    PERCENT_BOUND_INDEX_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                    assert!(percent_bound_operation(
                        class.as_ptr(),
                        receiver.as_ptr(),
                        kind
                    ));
                    let expected_class = if kind == 3 {
                        &raw mut PyExc_ValueError
                    } else {
                        &raw mut PyExc_OverflowError
                    };
                    let matched = errors::PyErr_ExceptionMatches(expected_class.cast());
                    let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!error.as_ptr().is_null());
                    assert_eq!(matched, 1);
                    assert_eq!(PERCENT_BOUND_RESULT_DROPS.load(Ordering::SeqCst), 1);
                    assert!(PERCENT_BOUND_RESULT_ERROR.load(Ordering::SeqCst).is_null());
                    if kind == 9 {
                        assert_eq!(PERCENT_BOUND_INDEX_DROPS.load(Ordering::SeqCst), 1);
                        assert_eq!(
                            PERCENT_BOUND_INDEX_ERROR.load(Ordering::SeqCst),
                            error.as_ptr()
                        );
                    }
                    PERCENT_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
                }
                PERCENT_BOUND_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
            }
            if kind == 19 {
                percent_warning_policy(py, b"error");
                CONSTRUCTOR_CALLS.store(0, Ordering::SeqCst);
                PERCENT_DROPS.store(0, Ordering::SeqCst);
                PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                assert!(percent_bound_operation(
                    class.as_ptr(),
                    receiver.as_ptr(),
                    kind
                ));
                let matched =
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_DeprecationWarning).cast());
                let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!error.as_ptr().is_null());
                assert_eq!(matched, 1);
                assert_eq!(CONSTRUCTOR_CALLS.load(Ordering::SeqCst), 0);
                assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
                assert_eq!(
                    PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst),
                    error.as_ptr()
                );
                percent_warning_policy(py, b"ignore");
            }
            PERCENT_BOUND_REMOVE.store(false, Ordering::SeqCst);
            assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 0);
            PERCENT_BOUND_OWNER.store(ptr::null_mut(), Ordering::SeqCst);
            PERCENT_BOUND_NAME.store(ptr::null_mut(), Ordering::SeqCst);
            drop(receiver);
            drop(class);
            assert_eq!(
                PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst),
                usize::from(kind != 18),
                "fixture descriptor retirement {method:?}"
            );
            assert!(!crate::object::gc::native_gc_is_enrolled(class_address));
        }
        // A descriptor's self-removal makes the producer's retained descriptor
        // the final owner when binding fails, independently of invocation.
        PERCENT_BOUND_DESCRIPTOR_DROPS.store(0, Ordering::SeqCst);
        let class = percent_bound_class(ptr::null_mut(), ptr::null_mut());
        let class_address = class.as_ptr().addr();
        let receiver =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class.as_ptr().cast(), 0));
        assert!(!receiver.as_ptr().is_null());
        let name = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"__format__".as_ptr()));
        assert!(!name.as_ptr().is_null());
        PERCENT_BOUND_OWNER.store(class.as_ptr(), Ordering::SeqCst);
        PERCENT_BOUND_NAME.store(name.as_ptr(), Ordering::SeqCst);
        let descriptor =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *descriptor_type, 0));
        assert!(!descriptor.as_ptr().is_null());
        assert_eq!(
            object::PyObject_SetAttr(class.as_ptr(), name.as_ptr(), descriptor.as_ptr()),
            0
        );
        drop(descriptor);
        PERCENT_BIND_FAIL.store(true, Ordering::SeqCst);
        PERCENT_DROPS.store(0, Ordering::SeqCst);
        PERCENT_MANAGED_DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        assert!(percent_bound_operation(
            class.as_ptr(),
            receiver.as_ptr(),
            2
        ));
        let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(error.as_ptr(), failure.as_ptr());
        assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), 1);
        assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 1);
        assert_eq!(
            PERCENT_MANAGED_DROP_ERROR.load(Ordering::SeqCst),
            failure.as_ptr()
        );
        PERCENT_BIND_FAIL.store(false, Ordering::SeqCst);
        PERCENT_BOUND_OWNER.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_BOUND_NAME.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        drop(receiver);
        drop(class);
        assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 1);
        assert!(!crate::object::gc::native_gc_is_enrolled(class_address));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

unsafe extern "C" fn percent_reflected_getattribute(
    receiver: *mut PyObject,
    name: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if strings::PyUnicode_CompareWithASCIIString(name, c"__radd__".as_ptr()) == 0 {
            if PERCENT_REFLECTED_MISSING.load(Ordering::SeqCst)
                && receiver == PERCENT_REFLECTED_LEFT.load(Ordering::SeqCst)
            {
                errors::PyErr_SetString(
                    (&raw mut PyExc_AttributeError).cast(),
                    c"missing reflected method".as_ptr(),
                );
                return ptr::null_mut();
            }
            return typeobj::PyType_GenericAlloc(
                PERCENT_BOUND_RESULT_TYPE.load(Ordering::SeqCst),
                0,
            );
        }
        match PyType_Type.tp_getattro {
            Some(getattribute) => getattribute(receiver, name),
            None => {
                errors::PyErr_SetString(
                    (&raw mut PyExc_SystemError).cast(),
                    c"type attribute fixture unavailable".as_ptr(),
                );
                ptr::null_mut()
            }
        }
    }
}

unsafe extern "C" fn percent_reflected_compare(
    _left: *mut PyObject,
    _right: *mut PyObject,
    _operation: std::os::raw::c_int,
) -> *mut PyObject {
    unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(FAILURE.load(Ordering::SeqCst))) };
    ptr::null_mut()
}

#[test]
fn native_reflected_comparison_owners_retire_after_error_classification() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let failure = exception(&raw mut PyExc_LookupError, &[]);
        FAILURE.store(failure.as_ptr(), Ordering::SeqCst);
        let mut comparison_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.ReflectedComparison");
        comparison_type.tp_richcompare = Some(percent_reflected_compare);
        comparison_type.tp_dealloc = Some(percent_bound_result_drop);
        assert_eq!(comparison_type.ready(), 0);
        let mut callable =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.ReflectedCallable");
        callable.tp_call = Some(percent_bound_call);
        callable.tp_dealloc = Some(percent_managed_numeric_drop);
        assert_eq!(callable.ready(), 0);
        let mut descriptor_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"percent.ReflectedDescriptor");
        descriptor_type.tp_descr_get = Some(percent_bound_get);
        descriptor_type.tp_dealloc = Some(percent_bound_descriptor_drop);
        assert_eq!(descriptor_type.ready(), 0);
        let mut metaclass = NativeType::subtype(&raw mut PyType_Type, c"percent.ReflectedMeta");
        metaclass.tp_getattro = Some(percent_reflected_getattribute);
        assert_eq!(metaclass.ready(), 0);
        PERCENT_BOUND_RESULT_TYPE.store(&raw mut *comparison_type, Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_TYPE.store(&raw mut *callable, Ordering::SeqCst);
        PERCENT_BOUND_DESCRIPTOR_DROPS.store(0, Ordering::SeqCst);
        TEXT_CALLBACK_FAILS.store(true, Ordering::SeqCst);
        let left_class = percent_bound_class(&raw mut *metaclass, ptr::null_mut());
        let left_class_address = left_class.as_ptr().addr();
        let bases = percent_tuple(&[left_class.as_ptr()], None);
        let right_class = percent_bound_class(&raw mut *metaclass, bases.as_ptr());
        let right_class_address = right_class.as_ptr().addr();
        let declaration =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *descriptor_type, 0));
        assert!(!declaration.as_ptr().is_null());
        for class in [left_class.as_ptr(), right_class.as_ptr()] {
            for name in [c"__add__", c"__radd__"] {
                assert_eq!(
                    object::PyObject_SetAttrString(class, name.as_ptr(), declaration.as_ptr()),
                    0
                );
            }
        }
        let left =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(left_class.as_ptr().cast(), 0));
        let right =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(right_class.as_ptr().cast(), 0));
        assert!(!left.as_ptr().is_null());
        assert!(!right.as_ptr().is_null());
        PERCENT_REFLECTED_LEFT.store(left_class.as_ptr(), Ordering::SeqCst);
        for missing in [false, true] {
            PERCENT_REFLECTED_MISSING.store(missing, Ordering::SeqCst);
            PERCENT_BOUND_RESULT_DROPS.store(0, Ordering::SeqCst);
            PERCENT_BOUND_RESULT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            PERCENT_DROPS.store(0, Ordering::SeqCst);
            let result = OwnedPyObject::from_owned(
                molt_cpython_abi::api::abstract_number::PyNumber_Add(left.as_ptr(), right.as_ptr()),
            );
            assert!(result.as_ptr().is_null());
            let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(error.as_ptr(), failure.as_ptr());
            assert_eq!(
                PERCENT_BOUND_RESULT_DROPS.load(Ordering::SeqCst),
                if missing { 1 } else { 2 }
            );
            assert_eq!(
                PERCENT_BOUND_RESULT_ERROR.load(Ordering::SeqCst),
                if missing {
                    ptr::null_mut()
                } else {
                    error.as_ptr()
                }
            );
            assert_eq!(PERCENT_DROPS.load(Ordering::SeqCst), usize::from(missing));
        }
        PERCENT_REFLECTED_LEFT.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_REFLECTED_MISSING.store(false, Ordering::SeqCst);
        PERCENT_BOUND_RESULT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        PERCENT_MANAGED_FACTORY_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 0);
        drop(right);
        drop(left);
        drop(declaration);
        drop(right_class);
        assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 0);
        assert!(!crate::object::gc::native_gc_is_enrolled(
            right_class_address
        ));
        drop(bases);
        drop(left_class);
        assert_eq!(PERCENT_BOUND_DESCRIPTOR_DROPS.load(Ordering::SeqCst), 1);
        assert!(!crate::object::gc::native_gc_is_enrolled(
            left_class_address
        ));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn native_exception_field_publication_restores_error_before_displaced_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::{ExceptionFieldSlot, ExceptionStorage, ExceptionValue};
        let failure_ptr = crate::builtins::exceptions::alloc_exception(
            py,
            BuiltinExc::LookupError,
            "field publication",
        );
        assert!(!failure_ptr.is_null());
        let failure_bits = crate::MoltObject::from_ptr(failure_ptr).bits();
        let _failure = ExceptionValue::adopt(py, failure_bits);
        let failure = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(failure_bits) };
        assert!(!failure.is_null());
        let mut payload_type =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"FieldRetirementPayload");
        payload_type.tp_dealloc = Some(payload_drop);
        assert_eq!(unsafe { payload_type.ready() }, 0);
        unsafe {
            let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *payload_type,
            })));
            let target = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
            drop(payload);
            let replacement_ptr = crate::object::builders::alloc_tuple(py, &[]);
            assert!(!replacement_ptr.is_null());
            let replacement =
                ExceptionValue::adopt(py, crate::MoltObject::from_ptr(replacement_ptr).bits());
            DROP_OBSERVES.store(true, Ordering::SeqCst);
            crate::record_exception(py, crate::obj_from_bits(failure_bits).as_ptr().unwrap());
            assert!(
                ExceptionStorage::Native(target.as_ptr())
                    .publish(py, ExceptionFieldSlot::Args, replacement.bits(),)
                    .is_some()
            );
            assert_eq!(DROPS.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_ERROR.load(Ordering::SeqCst), failure);
            assert_eq!(crate::exception_last_bits_noinc(py), Some(failure_bits));
            assert!(
                ExceptionStorage::Native(target.as_ptr())
                    .publish(
                        py,
                        ExceptionFieldSlot::Traceback,
                        crate::MoltObject::from_int(9).bits(),
                    )
                    .is_none()
            );
            assert_eq!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                1
            );
            let rejected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!rejected.as_ptr().is_null());
            assert_ne!(rejected.as_ptr(), failure);
            assert!(
                (*target.as_ptr().cast::<PyBaseExceptionObject>())
                    .traceback
                    .is_null()
            );
            DROP_OBSERVES.store(false, Ordering::SeqCst);
        }
    });
}

#[test]
fn managed_exception_observers_and_replacement_admit_runtime_pending_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use crate::MoltObject;
        use crate::builtins::exceptions::{ExceptionFieldSlot, ExceptionStorage, ExceptionValue};
        unsafe {
            for warm in [false, true] {
                for set_object in [false, true] {
                    let prior_ptr = crate::builtins::exceptions::alloc_exception(
                        py,
                        BuiltinExc::LookupError,
                        "incoming raised state",
                    );
                    assert!(!prior_ptr.is_null());
                    let prior = ExceptionValue::adopt(py, MoltObject::from_ptr(prior_ptr).bits());
                    let prior_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(prior.bits());
                    assert!(!prior_view.is_null());
                    let next_bits = crate::builtins::exceptions::molt_exception_new_builtin_one(
                        u64::from(crate::ExcKind::from(crate::BuiltinExc::ValueError).code()),
                        MoltObject::from_int(42).bits(),
                    );
                    let next = ExceptionValue::adopt(py, next_bits);
                    let next_ptr = crate::obj_from_bits(next.bits())
                        .as_ptr()
                        .expect("ValueError");
                    assert!(crate::builtins::exceptions::exception_args_is_lazy_single(
                        crate::exception_args_bits(next_ptr),
                    ));
                    let context_ptr = crate::builtins::exceptions::alloc_exception(
                        py,
                        BuiltinExc::RuntimeError,
                        "retained field",
                    );
                    assert!(!context_ptr.is_null());
                    let context =
                        ExceptionValue::adopt(py, MoltObject::from_ptr(context_ptr).bits());
                    let context_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(context.bits());
                    assert!(!context_view.is_null());
                    let storage = ExceptionStorage::Managed(next_ptr);
                    assert!(
                        storage
                            .publish(py, ExceptionFieldSlot::Context, context.bits())
                            .is_some()
                    );
                    assert!(
                        storage
                            .publish(py, ExceptionFieldSlot::Cause, context.bits())
                            .is_some()
                    );
                    let mut next_view = ptr::null_mut();
                    if warm {
                        next_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(next.bits());
                        assert!(!next_view.is_null());
                    }
                    crate::record_exception(py, prior_ptr);
                    if !warm {
                        next_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(next.bits());
                        assert!(!next_view.is_null());
                    }
                    let args = OwnedPyObject::from_owned(errors::PyException_GetArgs(next_view));
                    assert!(!args.as_ptr().is_null());
                    assert_eq!(sequences::PyTuple_Size(args.as_ptr()), 1);
                    assert_eq!(
                        numbers::PyLong_AsLong(sequences::PyTuple_GetItem(args.as_ptr(), 0)),
                        42
                    );
                    let actual_context =
                        OwnedPyObject::from_owned(errors::PyException_GetContext(next_view));
                    let actual_cause =
                        OwnedPyObject::from_owned(errors::PyException_GetCause(next_view));
                    assert_eq!(actual_context.as_ptr(), context_view);
                    assert_eq!(actual_cause.as_ptr(), context_view);
                    assert!(errors::PyException_GetTraceback(next_view).is_null());
                    assert_eq!(
                        object::PyObject_IsSubclass(
                            object::Py_TYPE(next_view).cast(),
                            (&raw mut PyExc_Exception).cast(),
                        ),
                        1
                    );
                    let observed = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(observed.as_ptr(), prior_view);
                    crate::record_exception(py, prior_ptr);
                    if set_object {
                        errors::PyErr_SetObject((&raw mut PyExc_Exception).cast(), next_view);
                    } else {
                        errors::PyErr_SetRaisedException(object::Py_NewRef(next_view));
                    }
                    let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(
                        raised.as_ptr(),
                        next_view,
                        "matching instance is selected unchanged"
                    );
                    assert!(errors::PyErr_Occurred().is_null());
                }
            }
        }
    });
}

static LOOKUP_HASH_SEEN_PRIOR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static LOOKUP_HASH_MODE: AtomicUsize = AtomicUsize::new(0);
static LOOKUP_HASH_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static LOOKUP_HASH_TRACEBACK: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn lookup_hash(value: *mut PyObject) -> isize {
    let mode = LOOKUP_HASH_MODE.load(Ordering::SeqCst);
    unsafe {
        let prior = errors::PyErr_GetRaisedException();
        LOOKUP_HASH_SEEN_PRIOR.store(prior, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(prior);
        if mode == 0 {
            return 23;
        }
        // Explicit native failure with the exact admitted exception instance.
        if mode == 3 {
            return -1;
        }
        if mode == 1 {
            typeobj::PyObject_HashNotImplemented(value);
        } else {
            errors::PyErr_SetRaisedException(object::Py_NewRef(FAILURE.load(Ordering::SeqCst)));
        }
        let failure = errors::PyErr_GetRaisedException();
        if !failure.is_null() {
            errors::PyException_SetTraceback(failure, LOOKUP_HASH_TRACEBACK.load(Ordering::SeqCst));
        }
        LOOKUP_HASH_ERROR.store(failure, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(failure);
    }
    -1
}

#[test]
fn dictionary_typed_lookup_distinguishes_prior_errors_from_native_hash_failures() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::ExceptionValue;
        use molt_cpython_abi::api::mapping;
        unsafe {
            let prior_ptr = crate::builtins::exceptions::alloc_exception(
                py,
                BuiltinExc::RuntimeError,
                "prior dictionary error",
            );
            assert!(!prior_ptr.is_null());
            let prior = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(prior_ptr).bits());
            let prior_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(prior.bits());
            assert!(!prior_view.is_null());
            let failure = exception(&raw mut PyExc_LookupError, &[]);
            let trace_class = crate::obj_from_bits(crate::builtin_classes(py).traceback)
                .as_ptr()
                .unwrap();
            let trace_bits = crate::alloc_instance_for_class(py, trace_class);
            let trace = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits));
            assert!(!trace.as_ptr().is_null());
            FAILURE.store(failure.as_ptr(), Ordering::SeqCst);
            LOOKUP_HASH_TRACEBACK.store(trace.as_ptr(), Ordering::SeqCst);
            let mut key_type = NativeType::subtype(&raw mut PyBaseObject_Type, c"NativeHashKey");
            key_type.tp_hash = Some(lookup_hash);
            assert_eq!(key_type.ready(), 0);
            let hit =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *key_type, 0));
            let miss =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *key_type, 0));
            let dict = OwnedPyObject::from_owned(mapping::PyDict_New());
            assert!(
                !hit.as_ptr().is_null() && !miss.as_ptr().is_null() && !dict.as_ptr().is_null()
            );
            assert!(GLOBAL_BRIDGE.molt_handle_for_pyobj(hit.as_ptr()).is_none());
            assert_eq!(
                mapping::PyDict_SetItem(dict.as_ptr(), hit.as_ptr(), &raw mut Py_None),
                0
            );
            for runtime_prior in [false, true] {
                for get_ref in [false, true] {
                    for (key, present) in [(hit.as_ptr(), true), (miss.as_ptr(), false)] {
                        if runtime_prior {
                            crate::record_exception(py, prior_ptr);
                        } else {
                            errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                        }
                        if get_ref {
                            let mut value = ptr::null_mut();
                            let status =
                                mapping::PyDict_GetItemRef(dict.as_ptr(), key, &raw mut value);
                            let value = OwnedPyObject::from_owned(value);
                            assert_eq!(status, i32::from(present));
                            assert_eq!(
                                value.as_ptr(),
                                if present {
                                    &raw mut Py_None
                                } else {
                                    ptr::null_mut()
                                }
                            );
                        } else {
                            assert_eq!(
                                mapping::PyDict_GetItemWithError(dict.as_ptr(), key),
                                if present {
                                    &raw mut Py_None
                                } else {
                                    ptr::null_mut()
                                }
                            );
                        }
                        assert_eq!(
                            LOOKUP_HASH_SEEN_PRIOR.load(Ordering::SeqCst),
                            prior_view,
                            "native hash observes the admitted prior without detachment"
                        );
                        let selected =
                            OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(
                            selected.as_ptr(),
                            prior_view,
                            "hit/missing preserves the admitted prior error"
                        );
                    }
                    for mode in [1, 2] {
                        LOOKUP_HASH_MODE.store(mode, Ordering::SeqCst);
                        LOOKUP_HASH_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                        if runtime_prior {
                            crate::record_exception(py, prior_ptr);
                        } else {
                            errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                        }
                        if get_ref {
                            let mut value = ptr::null_mut();
                            assert_eq!(
                                mapping::PyDict_GetItemRef(
                                    dict.as_ptr(),
                                    miss.as_ptr(),
                                    &raw mut value
                                ),
                                -1
                            );
                            assert!(value.is_null());
                        } else {
                            assert!(
                                mapping::PyDict_GetItemWithError(dict.as_ptr(), miss.as_ptr())
                                    .is_null()
                            );
                        }
                        assert_eq!(
                            errors::PyErr_ExceptionMatches(if mode == 1 {
                                (&raw mut PyExc_TypeError).cast()
                            } else {
                                (&raw mut PyExc_LookupError).cast()
                            }),
                            1
                        );
                        let selected =
                            OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert!(!selected.as_ptr().is_null());
                        assert_eq!(selected.as_ptr(), LOOKUP_HASH_ERROR.load(Ordering::SeqCst));
                        if mode == 1 {
                            assert_eq!(
                                text(typeobj::PyObject_Str(selected.as_ptr())),
                                "unhashable type: 'NativeHashKey'"
                            );
                        } else {
                            assert_eq!(selected.as_ptr(), failure.as_ptr());
                        }
                        let actual_trace = OwnedPyObject::from_owned(
                            errors::PyException_GetTraceback(selected.as_ptr()),
                        );
                        assert_eq!(actual_trace.as_ptr(), trace.as_ptr());
                        LOOKUP_HASH_MODE.store(0, Ordering::SeqCst);
                    }
                }
            }
            // The real satellite producers share the same three-state
            // ABI. An absent/error lookup must not publish an output value.
            type Getter = unsafe extern "C" fn(*mut u8, u64, *mut u64) -> i32;
            let getters: &[(&str, Getter)] = &[
                #[cfg(feature = "stdlib_crypto")]
                (
                    "crypto",
                    crate::crypto_bridge::__molt_crypto_dict_get_in_place,
                ),
                #[cfg(feature = "stdlib_graphlib")]
                (
                    "graphlib",
                    crate::graphlib_bridge::__molt_graphlib_dict_get_in_place,
                ),
                #[cfg(feature = "stdlib_http")]
                ("http", crate::http_bridge::__molt_http_dict_get_in_place),
                #[cfg(feature = "stdlib_math")]
                ("math", crate::math_bridge::__molt_math_dict_get_in_place),
                #[cfg(feature = "stdlib_serial")]
                (
                    "serial",
                    (*crate::serial_bridge::__molt_serial_get_vtable()).dict_get_in_place,
                ),
            ];
            let dict_bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(dict.as_ptr())
                .unwrap()
                .bits();
            let dict_pointer = crate::obj_from_bits(dict_bits).as_ptr().unwrap();
            let hit_bits = molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                GLOBAL_BRIDGE.molt_value_for_pyobj(hit.as_ptr()).unwrap(),
            );
            let miss_bits = molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                GLOBAL_BRIDGE.molt_value_for_pyobj(miss.as_ptr()).unwrap(),
            );
            for &(name, getter) in getters {
                for runtime_prior in [false, true] {
                    for (key, mode, expected) in [
                        (hit_bits.bits(), 0, 1),
                        (miss_bits.bits(), 0, 0),
                        (hit_bits.bits(), 3, -1),
                        (miss_bits.bits(), 3, -1),
                    ] {
                        LOOKUP_HASH_MODE.store(mode, Ordering::SeqCst);
                        LOOKUP_HASH_SEEN_PRIOR.store(ptr::null_mut(), Ordering::SeqCst);
                        if runtime_prior {
                            crate::record_exception(py, prior_ptr);
                        } else {
                            errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                        }
                        let untouched = 0x0123_4567_89ab_cdef;
                        let mut value = untouched;
                        assert_eq!(
                            getter(dict_pointer, key, &raw mut value),
                            expected,
                            "{name}"
                        );
                        assert_eq!(
                            value,
                            if expected == 1 {
                                crate::MoltObject::none().bits()
                            } else {
                                untouched
                            },
                            "{name}"
                        );
                        assert_eq!(
                            LOOKUP_HASH_SEEN_PRIOR.load(Ordering::SeqCst),
                            prior_view,
                            "{name}"
                        );
                        let selected =
                            OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(
                            selected.as_ptr(),
                            prior_view,
                            "{name}: exact incoming identity survives success/miss or explicit same-instance failure"
                        );
                    }
                }
            }
            LOOKUP_HASH_MODE.store(0, Ordering::SeqCst);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn fallible_c_preparation_retires_runtime_prior_under_the_selected_native_failure() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"PreparationPriorPayload");
            payload_type.tp_dealloc = Some(payload_drop);
            assert_eq!(payload_type.ready(), 0);
            struct ClearRaised;
            impl Drop for ClearRaised {
                fn drop(&mut self) {
                    DROP_OBSERVES.store(false, Ordering::SeqCst);
                    unsafe { errors::PyErr_Clear() };
                }
            }
            let _clear = ClearRaised;
            let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *payload_type,
            })));
            let prior = exception(&raw mut PyExc_ValueError, &[payload.as_ptr()]);
            drop(payload);
            let selected = exception(&raw mut PyExc_LookupError, &[]);
            let trace_class = crate::obj_from_bits(crate::builtin_classes(py).traceback)
                .as_ptr()
                .unwrap();
            let trace_bits = crate::alloc_instance_for_class(py, trace_class);
            let trace = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits));
            assert!(!trace.as_ptr().is_null());
            assert_eq!(
                errors::PyException_SetTraceback(selected.as_ptr(), trace.as_ptr()),
                0
            );
            errors::PyErr_SetRaisedException(prior.into_ptr());
            assert!(super::transfer_pending_cpython_exception());
            let prior_bits = crate::exception_last_bits_noinc(py).expect("runtime prior");
            assert_eq!(
                crate::object::object_type_id(crate::obj_from_bits(prior_bits).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN
            );
            DROP_OBSERVES.store(true, Ordering::SeqCst);
            let result: Result<(), molt_cpython_abi::ErrorIndicatorSet> =
                errors::with_saved_error(|| {
                    assert!(errors::PyErr_Occurred().is_null());
                    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
                    errors::PyErr_SetRaisedException(object::Py_NewRef(selected.as_ptr()));
                    Err(molt_cpython_abi::ErrorIndicatorSet)
                });
            assert!(result.is_err());
            assert_eq!(DROPS.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_ERROR.load(Ordering::SeqCst), selected.as_ptr());
            assert_eq!(DROP_TRACEBACK.load(Ordering::SeqCst), trace.as_ptr());
            let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(raised.as_ptr(), selected.as_ptr());
            let actual_trace =
                OwnedPyObject::from_owned(errors::PyException_GetTraceback(raised.as_ptr()));
            assert_eq!(actual_trace.as_ptr(), trace.as_ptr());
        }
    });
}

static NOTE_REPR_FAILS: AtomicBool = AtomicBool::new(false);
static NOTE_REPR_SAW_CLEAR: AtomicBool = AtomicBool::new(false);
static NOTE_OVERRIDE_CALLS: AtomicUsize = AtomicUsize::new(0);
static NOTE_MUTATED_TRACEBACK: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static NOTE_INPUT_OWNER: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static NOTE_TIMING: AtomicBool = AtomicBool::new(false);
static NOTE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static NOTE_REPR_ORDER: AtomicUsize = AtomicUsize::new(0);
static NOTE_INPUT_ORDER: AtomicUsize = AtomicUsize::new(0);
static NOTE_GET_ORDER: AtomicUsize = AtomicUsize::new(0);
static NOTE_INPUT_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn note_constructor(
    class: *mut PyTypeObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        refcount::Py_XDECREF(NOTE_INPUT_OWNER.swap(ptr::null_mut(), Ordering::SeqCst));
        failing_constructor(class, args, kwargs)
    }
}

unsafe extern "C" fn note_input_drop(value: *mut PyObject) {
    unsafe {
        if NOTE_TIMING.load(Ordering::SeqCst) {
            NOTE_INPUT_ORDER.store(
                NOTE_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1,
                Ordering::SeqCst,
            );
            let error = errors::PyErr_GetRaisedException();
            NOTE_INPUT_ERROR.store(error, Ordering::SeqCst);
            errors::PyErr_SetRaisedException(error);
        }
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
    }
}

unsafe extern "C" fn note_failure_getattro(
    value: *mut PyObject,
    name: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if NOTE_TIMING.load(Ordering::SeqCst) {
            let mut length = 0;
            let bytes = strings::PyUnicode_AsUTF8AndSize(name, &raw mut length);
            if !bytes.is_null()
                && length == 9
                && std::slice::from_raw_parts(bytes.cast::<u8>(), 9) == b"__notes__"
            {
                NOTE_GET_ORDER.store(
                    NOTE_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1,
                    Ordering::SeqCst,
                );
            }
        }
        object::PyObject_GenericGetAttr(value, name)
    }
}

unsafe extern "C" fn note_argument_repr(_value: *mut PyObject) -> *mut PyObject {
    unsafe {
        NOTE_REPR_SAW_CLEAR.store(errors::PyErr_Occurred().is_null(), Ordering::SeqCst);
        if NOTE_TIMING.load(Ordering::SeqCst) {
            NOTE_REPR_ORDER.store(
                NOTE_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1,
                Ordering::SeqCst,
            );
        }
        let trace = NOTE_MUTATED_TRACEBACK.load(Ordering::SeqCst);
        if !trace.is_null() {
            errors::PyException_SetTraceback(FAILURE.load(Ordering::SeqCst), trace);
        }
        if NOTE_REPR_FAILS.load(Ordering::SeqCst) {
            errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"repr failed".as_ptr());
            ptr::null_mut()
        } else {
            strings::PyUnicode_FromString(c"<argument>".as_ptr())
        }
    }
}

unsafe extern "C" fn note_override(
    _self: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    NOTE_OVERRIDE_CALLS.fetch_add(1, Ordering::SeqCst);
    unsafe { object::Py_NewRef(&raw mut Py_None) }
}

#[test]
fn normalization_failure_note_uses_base_descriptor_and_preserves_original_failure() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut requested = NativeType::subtype(&raw mut PyExc_ValueError, c"NativeNoteError");
            requested.tp_new = Some(note_constructor);
            assert_eq!(requested.ready(), 0);
            let mut input_type = NativeType::subtype(&raw mut PyBaseObject_Type, c"NoteArgument");
            input_type.tp_repr = Some(note_argument_repr);
            input_type.tp_dealloc = Some(note_input_drop);
            assert_eq!(input_type.ready(), 0);
            struct NoteInputCleanup;
            impl Drop for NoteInputCleanup {
                fn drop(&mut self) {
                    NOTE_TIMING.store(false, Ordering::SeqCst);
                    unsafe {
                        refcount::Py_XDECREF(
                            NOTE_INPUT_OWNER.swap(ptr::null_mut(), Ordering::SeqCst),
                        );
                    }
                }
            }
            let _input_cleanup = NoteInputCleanup;
            let mut failure_type = NativeType::subtype(&raw mut PyExc_LookupError, c"NoteFailure");
            failure_type.tp_getattro = Some(note_failure_getattro);
            assert_eq!(failure_type.ready(), 0);
            let mut override_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"NoteOverride");
            override_type.tp_call = Some(note_override);
            assert_eq!(override_type.ready(), 0);
            let input =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *input_type, 0));
            let override_callable =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *override_type, 0));
            assert!(!input.as_ptr().is_null() && !override_callable.as_ptr().is_null());
            let trace_class = crate::obj_from_bits(crate::builtin_classes(py).traceback)
                .as_ptr()
                .unwrap();
            let trace_bits = crate::alloc_instance_for_class(py, trace_class);
            let trace = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits));
            assert!(!trace.as_ptr().is_null());
            let updated_trace_bits = crate::alloc_instance_for_class(py, trace_class);
            let updated_trace =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(updated_trace_bits));
            assert!(!updated_trace.as_ptr().is_null());
            for managed_failure in [false, true] {
                for (repr_fails, mutate_trace) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    let failure = if managed_failure {
                        let pointer = crate::builtins::exceptions::alloc_exception(
                            py,
                            BuiltinExc::LookupError,
                            "construction failed",
                        );
                        assert!(!pointer.is_null());
                        OwnedPyObject::from_owned(
                            GLOBAL_BRIDGE
                                .owned_handle_to_pyobj(crate::MoltObject::from_ptr(pointer).bits()),
                        )
                    } else {
                        exception(&raw mut *failure_type, &[])
                    };
                    assert!(!failure.as_ptr().is_null());
                    assert_eq!(
                        errors::PyException_SetTraceback(failure.as_ptr(), trace.as_ptr()),
                        0
                    );
                    assert_eq!(
                        object::PyObject_SetAttrString(
                            failure.as_ptr(),
                            c"add_note".as_ptr(),
                            override_callable.as_ptr()
                        ),
                        0
                    );
                    FAILURE.store(failure.as_ptr(), Ordering::SeqCst);
                    NOTE_REPR_FAILS.store(repr_fails, Ordering::SeqCst);
                    NOTE_REPR_SAW_CLEAR.store(false, Ordering::SeqCst);
                    NOTE_OVERRIDE_CALLS.store(0, Ordering::SeqCst);
                    NOTE_MUTATED_TRACEBACK.store(
                        if mutate_trace {
                            updated_trace.as_ptr()
                        } else {
                            ptr::null_mut()
                        },
                        Ordering::SeqCst,
                    );
                    let timing = !managed_failure && !repr_fails && !mutate_trace;
                    NOTE_TIMING.store(timing, Ordering::SeqCst);
                    NOTE_SEQUENCE.store(0, Ordering::SeqCst);
                    NOTE_REPR_ORDER.store(0, Ordering::SeqCst);
                    NOTE_INPUT_ORDER.store(0, Ordering::SeqCst);
                    NOTE_GET_ORDER.store(0, Ordering::SeqCst);
                    NOTE_INPUT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                    let argument = if timing {
                        let sole = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                            &raw mut *input_type,
                            0,
                        ));
                        assert!(!sole.as_ptr().is_null());
                        let sole = sole.into_ptr();
                        NOTE_INPUT_OWNER.store(sole, Ordering::SeqCst);
                        sole
                    } else {
                        input.as_ptr()
                    };
                    errors::PyErr_SetObject((&raw mut *requested).cast(), argument);
                    if timing {
                        assert!(NOTE_INPUT_OWNER.load(Ordering::SeqCst).is_null());
                        assert_eq!(
                            [
                                NOTE_REPR_ORDER.load(Ordering::SeqCst),
                                NOTE_INPUT_ORDER.load(Ordering::SeqCst),
                                NOTE_GET_ORDER.load(Ordering::SeqCst),
                            ],
                            [1, 2, 3]
                        );
                        assert!(
                            NOTE_INPUT_ERROR.load(Ordering::SeqCst).is_null(),
                            "successful note preparation retires its input with no raised error"
                        );
                    }
                    NOTE_TIMING.store(false, Ordering::SeqCst);
                    let expected_trace = if mutate_trace {
                        updated_trace.as_ptr()
                    } else {
                        trace.as_ptr()
                    };
                    let raised = if mutate_trace {
                        let mut fetched = errors::OwnedCError {
                            exc_type: ptr::null_mut(),
                            value: ptr::null_mut(),
                            traceback: ptr::null_mut(),
                        };
                        errors::PyErr_Fetch(
                            &raw mut fetched.exc_type,
                            &raw mut fetched.value,
                            &raw mut fetched.traceback,
                        );
                        assert_eq!(
                            fetched.traceback, expected_trace,
                            "note callbacks can replace the detached failure's traceback"
                        );
                        OwnedPyObject::from_owned(std::mem::replace(
                            &mut fetched.value,
                            ptr::null_mut(),
                        ))
                    } else {
                        OwnedPyObject::from_owned(errors::PyErr_GetRaisedException())
                    };
                    assert_eq!(raised.as_ptr(), failure.as_ptr());
                    let actual_trace = OwnedPyObject::from_owned(errors::PyException_GetTraceback(
                        raised.as_ptr(),
                    ));
                    assert_eq!(actual_trace.as_ptr(), expected_trace);
                    assert!(NOTE_REPR_SAW_CLEAR.load(Ordering::SeqCst));
                    assert_eq!(NOTE_OVERRIDE_CALLS.load(Ordering::SeqCst), 0);
                    let notes = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                        raised.as_ptr(),
                        c"__notes__".as_ptr(),
                    ));
                    assert!(!notes.as_ptr().is_null());
                    assert_eq!(sequences::PyList_Size(notes.as_ptr()), 1);
                    let note = sequences::PyList_GetItem(notes.as_ptr(), 0);
                    assert!(!note.is_null());
                    assert_eq!(
                        text(object::Py_NewRef(note)),
                        if repr_fails {
                            "Normalization failed: type=NativeNoteError args=<unknown>"
                        } else {
                            "Normalization failed: type=NativeNoteError args=<argument>"
                        }
                    );
                }
            }
            NOTE_MUTATED_TRACEBACK.store(ptr::null_mut(), Ordering::SeqCst);
            FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn failed_cold_publication_retires_consumed_roots_after_selecting_the_failure() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::ExceptionValue;
        unsafe {
            let mut payload_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"PublicationRootPayload");
            payload_type.tp_dealloc = Some(payload_drop);
            assert_eq!(payload_type.ready(), 0);
            for nested in [false, true] {
                let prior_ptr = crate::builtins::exceptions::alloc_exception(
                    py,
                    BuiltinExc::LookupError,
                    "prior publication error",
                );
                assert!(!prior_ptr.is_null());
                let _prior =
                    ExceptionValue::adopt(py, crate::MoltObject::from_ptr(prior_ptr).bits());
                let payload = OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut *payload_type,
                })));
                let payload_bits = GLOBAL_BRIDGE
                    .molt_value_for_pyobj(payload.as_ptr())
                    .expect("foreign payload");
                let payload_value = ExceptionValue::adopt(py, payload_bits);
                assert_eq!(
                    crate::object::object_type_id(
                        crate::obj_from_bits(payload_bits).as_ptr().unwrap()
                    ),
                    crate::TYPE_ID_FOREIGN
                );
                let child_ptr = crate::object::builders::alloc_list(py, &[]);
                assert!(!child_ptr.is_null());
                let child =
                    ExceptionValue::adopt(py, crate::MoltObject::from_ptr(child_ptr).bits());
                let root_ptr =
                    crate::object::builders::alloc_tuple(py, &[payload_bits, child.bits()]);
                assert!(!root_ptr.is_null());
                let root = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(root_ptr).bits());
                drop(payload_value);
                drop(payload);
                DROP_OBSERVES.store(true, Ordering::SeqCst);
                DROPS.store(0, Ordering::SeqCst);
                DROP_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                crate::record_exception(py, prior_ptr);
                let target = if nested { child.bits() } else { root.bits() };
                let (view, attempts) = GLOBAL_BRIDGE
                    .with_denied_publication_for_test(target, || {
                        GLOBAL_BRIDGE.owned_handle_to_pyobj(root.into_bits())
                    });
                assert!(view.is_null());
                assert_eq!(attempts, 1);
                assert_eq!(DROPS.load(Ordering::SeqCst), 1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_MemoryError).cast()),
                    1
                );
                let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!selected.as_ptr().is_null());
                assert_eq!(DROP_ERROR.load(Ordering::SeqCst), selected.as_ptr());
                DROP_OBSERVES.store(false, Ordering::SeqCst);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

mod decimal_output_limits {
    use super::*;
    use crate::builtins::exceptions::ExceptionValue;
    use molt_cpython_abi::hooks::DecodedHandleResult;
    use num_bigint::{BigInt, BigUint, Sign};
    use num_traits::{ToPrimitive, Zero};

    const LIMIT_ERROR: &[u8] = b"Exceeds the limit (640 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit";

    struct LimitRestore(usize);
    impl Drop for LimitRestore {
        fn drop(&mut self) {
            crate::builtins::sys_ext::molt_sys_set_int_max_str_digits(
                crate::MoltObject::from_int(self.0 as i64).bits(),
            );
        }
    }

    fn set_limit(limit: i64) {
        crate::builtins::sys_ext::molt_sys_set_int_max_str_digits(
            crate::MoltObject::from_int(limit).bits(),
        );
    }

    unsafe fn raw_integer(class: *mut PyTypeObject, value: &BigInt) -> OwnedPyObject {
        let mut magnitude = value.magnitude().clone();
        let mask = BigUint::from((1u32 << 30) - 1);
        let mut digits = Vec::new();
        while !magnitude.is_zero() {
            digits.push((&magnitude & &mask).to_u32().unwrap());
            magnitude >>= 30usize;
        }
        let result = unsafe { percent_raw_long(class, &digits) };
        if !result.as_ptr().is_null() && value.sign() == Sign::Minus {
            unsafe { (*result.as_ptr().cast::<PyLongObject>()).long_value.lv_tag |= 2 };
        }
        result
    }

    unsafe fn limit_failure(result: *mut PyObject, stage: &str) -> OwnedPyObject {
        unsafe {
            let result = OwnedPyObject::from_owned(result);
            assert!(
                result.as_ptr().is_null(),
                "{stage}: decimal conversion succeeded"
            );
            assert_eq!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                1,
                "{stage}: decimal conversion did not select ValueError"
            );
            let failure = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!failure.as_ptr().is_null());
            assert_eq!(
                take_raw(typeobj::PyObject_Str(failure.as_ptr())),
                LIMIT_ERROR
            );
            failure
        }
    }

    #[test]
    fn decimal_limit_reaches_managed_and_physical_c_integer_renderers() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(super::super::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let _restore = LimitRestore(crate::builtins::sys_ext::current_int_max_str_digits(py));
            let mut subtype = NativeType::subtype(&raw mut PyLong_Type, c"DecimalLimitInteger");
            assert_eq!(subtype.ready(), 0);
            let allowed = BigInt::from(10u8).pow(639);
            let rejected = &allowed * BigInt::from(10u8);
            for native_class in [None, Some(&raw mut PyLong_Type), Some(&raw mut *subtype)] {
                for sign in [1i32, -1i32] {
                    let route = match native_class {
                        None => "managed",
                        Some(class) if class == &raw mut PyLong_Type => "physical exact",
                        Some(_) => "physical subtype",
                    };
                    let stage = format!("{route}, sign={sign}");
                    let value = &rejected * sign;
                    let integer = match native_class {
                        Some(class) => raw_integer(class, &value),
                        None => OwnedPyObject::from_owned(
                            GLOBAL_BRIDGE.owned_handle_to_pyobj(crate::bigint_bits(py, value)),
                        ),
                    };
                    assert!(!integer.as_ptr().is_null());
                    if native_class.is_some() {
                        assert!(
                            GLOBAL_BRIDGE
                                .molt_handle_for_pyobj(integer.as_ptr())
                                .is_none()
                        );
                    }
                    set_limit(640);
                    limit_failure(
                        typeobj::PyObject_Str(integer.as_ptr()),
                        &format!("{stage}: str"),
                    );
                    limit_failure(
                        typeobj::PyObject_Repr(integer.as_ptr()),
                        &format!("{stage}: repr"),
                    );
                    for spec in [b"".as_slice(), b"d", b"n", b"+0800,d"] {
                        let spec_label = String::from_utf8_lossy(spec);
                        let spec = OwnedPyObject::from_owned(raw_fixture(spec));
                        assert!(!spec.as_ptr().is_null());
                        limit_failure(
                            object::PyObject_Format(integer.as_ptr(), spec.as_ptr()),
                            &format!("{stage}: format {spec_label:?}"),
                        );
                    }
                    for template in [b"%d".as_slice(), b"%i", b"%u", b"%s", b"%r", b"%a"] {
                        limit_failure(
                            percent(template, integer.as_ptr()),
                            &format!("{stage}: percent {template:?}"),
                        );
                    }
                    let hex = OwnedPyObject::from_owned(raw_fixture(b"x"));
                    assert_eq!(
                        take_raw(object::PyObject_Format(integer.as_ptr(), hex.as_ptr())),
                        (&rejected * sign).to_str_radix(16).as_bytes()
                    );
                    set_limit(0);
                    assert_eq!(
                        take_raw(typeobj::PyObject_Str(integer.as_ptr())).len(),
                        if sign < 0 { 642 } else { 641 }
                    );
                    set_limit(640);
                    limit_failure(
                        typeobj::PyObject_Str(integer.as_ptr()),
                        &format!("{stage}: str after restoring limit"),
                    );
                }
            }
            let allowed = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.owned_handle_to_pyobj(crate::bigint_bits(py, allowed)),
            );
            assert!(!allowed.as_ptr().is_null());
            assert_eq!(take_raw(typeobj::PyObject_Str(allowed.as_ptr())).len(), 640);
            let grouped = OwnedPyObject::from_owned(raw_fixture(b",d"));
            assert_eq!(
                take_raw(object::PyObject_Format(allowed.as_ptr(), grouped.as_ptr())).len(),
                853
            );
            let seven = OwnedPyObject::from_owned(numbers::PyLong_FromLong(7));
            assert_eq!(take_raw(percent(b"%.800d", seven.as_ptr())).len(), 800);
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    static SCALAR_CONVERSIONS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn forbidden_scalar_conversion(_value: *mut PyObject) -> *mut PyObject {
        SCALAR_CONVERSIONS.fetch_add(1, Ordering::SeqCst);
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"storage rendering invoked a conversion override".as_ptr(),
            );
        }
        ptr::null_mut()
    }

    unsafe extern "C" fn integer_float_presentation(_value: *mut PyObject) -> *mut PyObject {
        SCALAR_CONVERSIONS.fetch_add(1, Ordering::SeqCst);
        unsafe { numbers::PyFloat_FromDouble(9.5) }
    }

    unsafe extern "C" fn scalar_repr_override(_value: *mut PyObject) -> *mut PyObject {
        unsafe { raw_fixture(b"native-repr") }
    }

    unsafe extern "C" fn scalar_format_override(
        _value: *mut PyObject,
        _spec: *mut PyObject,
    ) -> *mut PyObject {
        unsafe { raw_fixture(b"native-format") }
    }

    unsafe fn scalar_format(value: *mut PyObject, spec: &[u8]) -> Vec<u8> {
        unsafe {
            let spec = OwnedPyObject::from_owned(raw_fixture(spec));
            assert!(!spec.as_ptr().is_null());
            take_raw(object::PyObject_Format(value, spec.as_ptr()))
        }
    }

    unsafe fn runtime_scalar_format(
        py: &crate::PyToken<'_>,
        value: *mut PyObject,
        spec: &[u8],
    ) -> Vec<u8> {
        unsafe {
            let value = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE
                    .molt_value_for_pyobj(value)
                    .expect("native scalar ingress"),
            );
            let spec = ExceptionValue::adopt(
                py,
                crate::attr_name_bits_from_bytes(py, spec).expect("format spec"),
            );
            let result = crate::molt_format_builtin(value.bits(), spec.bits());
            take_raw(GLOBAL_BRIDGE.owned_handle_to_pyobj(result))
        }
    }

    #[test]
    fn physical_scalar_rendering_reads_storage_without_rebinding_native_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(super::super::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|_py| unsafe {
            SCALAR_CONVERSIONS.store(0, Ordering::SeqCst);
            let mut integer_slots: PyNumberMethods = std::mem::zeroed();
            integer_slots.nb_int = forbidden_scalar_conversion as *const () as *mut _;
            integer_slots.nb_index = forbidden_scalar_conversion as *const () as *mut _;
            integer_slots.nb_float = integer_float_presentation as *const () as *mut _;
            let mut integer_type = NativeType::subtype(&raw mut PyLong_Type, c"RenderingInteger");
            integer_type.tp_as_number = (&raw mut integer_slots).cast();
            assert_eq!(integer_type.ready(), 0);
            let mut float_slots: PyNumberMethods = std::mem::zeroed();
            float_slots.nb_float = forbidden_scalar_conversion as *const () as *mut _;
            let mut float_type = NativeType::subtype(&raw mut PyFloat_Type, c"RenderingFloat");
            float_type.tp_as_number = (&raw mut float_slots).cast();
            assert_eq!(float_type.ready(), 0);
            let mut complex_type =
                NativeType::subtype(&raw mut PyComplex_Type, c"RenderingComplex");
            assert_eq!(complex_type.ready(), 0);

            for class in [&raw mut PyLong_Type, &raw mut *integer_type] {
                let value = raw_integer(class, &BigInt::from(7));
                assert!(!value.as_ptr().is_null());
                assert_eq!(take_raw(typeobj::PyObject_Str(value.as_ptr())), b"7");
                assert_eq!(take_raw(typeobj::PyObject_Repr(value.as_ptr())), b"7");
                assert_eq!(scalar_format(value.as_ptr(), b""), b"7");
                assert_eq!(scalar_format(value.as_ptr(), b"04d"), b"0007");
                assert_eq!(runtime_scalar_format(&_py, value.as_ptr(), b"04d"), b"0007");
                assert_eq!(scalar_format(value.as_ptr(), b"x"), b"7");
                assert_eq!(SCALAR_CONVERSIONS.load(Ordering::SeqCst), 0);
                assert!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(value.as_ptr())
                        .is_none()
                );
            }
            for class in [&raw mut PyFloat_Type, &raw mut *float_type] {
                let value = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class, 0));
                assert!(!value.as_ptr().is_null());
                (*value.as_ptr().cast::<PyFloatObject>()).ob_fval = 1.25;
                assert_eq!(take_raw(typeobj::PyObject_Str(value.as_ptr())), b"1.25");
                assert_eq!(take_raw(typeobj::PyObject_Repr(value.as_ptr())), b"1.25");
                assert_eq!(scalar_format(value.as_ptr(), b""), b"1.25");
                assert_eq!(scalar_format(value.as_ptr(), b".2f"), b"1.25");
                assert_eq!(runtime_scalar_format(&_py, value.as_ptr(), b".2f"), b"1.25");
                assert_eq!(SCALAR_CONVERSIONS.load(Ordering::SeqCst), 0);
                assert!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(value.as_ptr())
                        .is_none()
                );
            }
            for class in [&raw mut PyComplex_Type, &raw mut *complex_type] {
                let value = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class, 0));
                assert!(!value.as_ptr().is_null());
                (*value.as_ptr().cast::<PyComplexObject>()).cval = Py_complex {
                    real: 1.25,
                    imag: -2.5,
                };
                assert_eq!(
                    take_raw(typeobj::PyObject_Str(value.as_ptr())),
                    b"(1.25-2.5j)"
                );
                assert_eq!(
                    take_raw(typeobj::PyObject_Repr(value.as_ptr())),
                    b"(1.25-2.5j)"
                );
                assert_eq!(scalar_format(value.as_ptr(), b""), b"(1.25-2.5j)");
                assert_eq!(scalar_format(value.as_ptr(), b".2f"), b"1.25-2.50j");
                assert_eq!(
                    runtime_scalar_format(&_py, value.as_ptr(), b".2f"),
                    b"1.25-2.50j"
                );
                assert!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(value.as_ptr())
                        .is_none()
                );
            }
            for (value, text, digits) in [
                (
                    (&raw mut _Py_TrueStruct).cast(),
                    b"True".as_slice(),
                    b"1".as_slice(),
                ),
                (
                    (&raw mut _Py_FalseStruct).cast(),
                    b"False".as_slice(),
                    b"0".as_slice(),
                ),
            ] {
                assert_eq!(take_raw(typeobj::PyObject_Str(value)), text);
                assert_eq!(take_raw(typeobj::PyObject_Repr(value)), text);
                assert_eq!(scalar_format(value, b""), text);
                assert_eq!(scalar_format(value, b"d"), digits);
                let int_repr = PyLong_Type.tp_repr.expect("int repr slot");
                assert_eq!(take_raw(int_repr(value)), digits);
            }
            // Integer floating presentations use the original receiver's
            // __float__, unlike integer digits and float's own stored payload.
            let value = raw_integer(&raw mut *integer_type, &BigInt::from(7));
            assert!(!value.as_ptr().is_null());
            assert_eq!(scalar_format(value.as_ptr(), b".1f"), b"9.5");
            assert_eq!(SCALAR_CONVERSIONS.load(Ordering::SeqCst), 1);
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(value.as_ptr())
                    .is_none()
            );
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    #[test]
    fn native_scalar_rendering_preserves_public_overrides_and_explicit_base_semantics() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(super::super::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|_py| unsafe {
            let mut repr_type = NativeType::subtype(&raw mut PyLong_Type, c"RenderingReprOverride");
            repr_type.tp_repr = Some(scalar_repr_override);
            assert_eq!(repr_type.ready(), 0);
            let value = raw_integer(&raw mut *repr_type, &BigInt::from(7));
            assert!(!value.as_ptr().is_null());
            assert_eq!(
                take_raw(typeobj::PyObject_Repr(value.as_ptr())),
                b"native-repr"
            );
            assert_eq!(
                take_raw(typeobj::PyObject_Str(value.as_ptr())),
                b"native-repr"
            );
            assert_eq!(scalar_format(value.as_ptr(), b""), b"native-repr");
            assert_eq!(scalar_format(value.as_ptr(), b"d"), b"7");
            let base_repr = PyLong_Type.tp_repr.expect("int repr slot");
            assert_eq!(take_raw(base_repr(value.as_ptr())), b"7");
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(value.as_ptr())
                    .is_none()
            );

            let mut methods = [
                PyMethodDef {
                    ml_name: c"__format__".as_ptr(),
                    ml_meth: Some(scalar_format_override),
                    ml_flags: METH_O,
                    ml_doc: ptr::null(),
                },
                std::mem::zeroed(),
            ];
            let mut format_type =
                NativeType::subtype(&raw mut PyLong_Type, c"RenderingFormatOverride");
            format_type.tp_methods = methods.as_mut_ptr();
            assert_eq!(format_type.ready(), 0);
            let value = raw_integer(&raw mut *format_type, &BigInt::from(7));
            assert!(!value.as_ptr().is_null());
            assert_eq!(scalar_format(value.as_ptr(), b"d"), b"native-format");
            assert_eq!(
                runtime_scalar_format(&_py, value.as_ptr(), b"d"),
                b"native-format"
            );
            let base_format = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                (&raw mut PyLong_Type).cast(),
                c"__format__".as_ptr(),
            ));
            assert!(!base_format.as_ptr().is_null());
            let spec = OwnedPyObject::from_owned(raw_fixture(b"d"));
            assert!(!spec.as_ptr().is_null());
            let mut arguments = [value.as_ptr(), spec.as_ptr()];
            assert_eq!(
                take_raw(object::PyObject_Vectorcall(
                    base_format.as_ptr(),
                    arguments.as_mut_ptr(),
                    arguments.len(),
                    ptr::null_mut(),
                )),
                b"7"
            );
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(value.as_ptr())
                    .is_none()
            );
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    static MANAGED_SCALAR_REPRS: AtomicUsize = AtomicUsize::new(0);
    static MANAGED_SCALAR_CONVERSIONS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn managed_scalar_conversion_override(_value: u64) -> u64 {
        MANAGED_SCALAR_CONVERSIONS.fetch_add(1, Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            crate::raise_exception::<u64>(
                py,
                BuiltinExc::AssertionError,
                "repr called a conversion override",
            )
        })
    }

    extern "C" fn managed_scalar_repr_override(_value: u64) -> u64 {
        MANAGED_SCALAR_REPRS.fetch_add(1, Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            let text = crate::alloc_string(py, b"managed-repr");
            if text.is_null() {
                crate::MoltObject::none().bits()
            } else {
                crate::MoltObject::from_ptr(text).bits()
            }
        })
    }

    #[test]
    fn numeric_base_repr_reads_projected_managed_subtype_storage() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(super::super::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let classes = crate::builtin_classes(py);
            for (base, declaring, kind, field_kind, arguments, expected) in [
                (
                    classes.int,
                    &raw mut PyLong_Type,
                    MoltTypeTag::Int,
                    Some(crate::object::class_layout::ScalarValueKind::Int),
                    vec![crate::MoltObject::from_int(7).bits()],
                    b"7".as_slice(),
                ),
                (
                    classes.float,
                    &raw mut PyFloat_Type,
                    MoltTypeTag::Float,
                    Some(crate::object::class_layout::ScalarValueKind::Float),
                    vec![crate::MoltObject::from_float(1.25).bits()],
                    b"1.25".as_slice(),
                ),
                (
                    classes.complex,
                    &raw mut PyComplex_Type,
                    MoltTypeTag::Complex,
                    None,
                    vec![
                        crate::MoltObject::from_float(1.25).bits(),
                        crate::MoltObject::from_float(-2.5).bits(),
                    ],
                    b"(1.25-2.5j)".as_slice(),
                ),
            ] {
                let (_class, instance) = percent_managed_protocol(
                    py,
                    base,
                    &[
                        (b"__repr__", managed_scalar_repr_override),
                        (b"__int__", managed_scalar_conversion_override),
                        (b"__float__", managed_scalar_conversion_override),
                        (b"__complex__", managed_scalar_conversion_override),
                    ],
                    &arguments,
                );
                let instance_bits = instance.bits();
                let view = OwnedPyObject::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(instance_bits),
                );
                assert!(!view.as_ptr().is_null());
                // The original runtime owner may be released: the admitted C
                // reference must retain the instance and its borrowed payload.
                drop(instance);
                MANAGED_SCALAR_REPRS.store(0, Ordering::SeqCst);
                MANAGED_SCALAR_CONVERSIONS.store(0, Ordering::SeqCst);
                let receiver_refs = (*view.as_ptr()).ob_refcnt;
                let public_result =
                    OwnedPyObject::from_owned(typeobj::PyObject_Repr(view.as_ptr()));
                assert!(
                    !public_result.as_ptr().is_null(),
                    "{kind:?}: public repr failed"
                );
                assert_eq!(borrowed_raw(public_result.as_ptr()), b"managed-repr");
                assert_eq!(MANAGED_SCALAR_REPRS.load(Ordering::SeqCst), 1);
                let repr = (*declaring).tp_repr.expect("declaring numeric repr slot");
                let base_result = OwnedPyObject::from_owned(repr(view.as_ptr()));
                assert!(
                    !base_result.as_ptr().is_null(),
                    "{kind:?}: declaring base repr failed"
                );
                assert_eq!(borrowed_raw(base_result.as_ptr()), expected);
                assert_eq!(MANAGED_SCALAR_REPRS.load(Ordering::SeqCst), 1);
                assert_eq!(MANAGED_SCALAR_CONVERSIONS.load(Ordering::SeqCst), 0);
                assert_eq!((*view.as_ptr()).ob_refcnt, receiver_refs);
                assert_eq!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(view.as_ptr())
                        .unwrap()
                        .bits(),
                    instance_bits
                );
                assert!(errors::PyErr_Occurred().is_null());

                // Complex subclasses retain native scalar storage. Int/Float
                // subclasses use a sealed field even though their C view is generic.
                let Some(field_kind) = field_kind else {
                    assert!(matches!(
                        super::super::hook_scalar_payload_borrowed(instance_bits, kind).decode(),
                        DecodedHandleResult::Missing
                    ));
                    continue;
                };
                let wrong_kind = if matches!(kind, MoltTypeTag::Int) {
                    MoltTypeTag::Float
                } else {
                    MoltTypeTag::Int
                };
                assert!(matches!(
                    super::super::hook_scalar_payload_borrowed(instance_bits, wrong_kind).decode(),
                    DecodedHandleResult::Missing
                ));
                // The typed row alone must not let malformed payloads render
                // None or dispatch arbitrary representation/conversion methods.
                let class_ptr = crate::obj_from_bits(_class.bits()).as_ptr().unwrap();
                let offset =
                    crate::object::class_layout::scalar_value_offset(py, class_ptr, field_kind)
                        .unwrap();
                crate::object::accessors::object_field_set_ptr_raw(
                    py,
                    crate::obj_from_bits(instance_bits).as_ptr().unwrap(),
                    offset,
                    crate::MoltObject::none().bits(),
                )
                .unwrap();
                let malformed = OwnedPyObject::from_owned(repr(view.as_ptr()));
                assert!(
                    malformed.as_ptr().is_null(),
                    "{kind:?}: malformed scalar storage admitted"
                );
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast()),
                    1
                );
                let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!selected.as_ptr().is_null());
                assert_eq!(MANAGED_SCALAR_CONVERSIONS.load(Ordering::SeqCst), 0);
                assert_eq!(MANAGED_SCALAR_REPRS.load(Ordering::SeqCst), 1);
            }
        });
    }

    #[cfg(all(
        not(target_arch = "wasm32"),
        not(miri),
        feature = "l7-attestation-probe"
    ))]
    #[test]
    fn rejected_decimal_magnitude_does_not_allocate_decimal_storage() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let value = BigInt::from(1u8) << 1_000_000usize;
        struct StopTracking;
        impl Drop for StopTracking {
            fn drop(&mut self) {
                crate::attestation_probe::set_tracking(false);
            }
        }
        crate::attestation_probe::reset();
        let tracking = StopTracking;
        crate::attestation_probe::set_tracking(true);
        let result = molt_runtime_core::integer_decimal::decimal_magnitude(&value, || 640);
        drop(tracking);
        let observed = crate::attestation_probe::snapshot();
        assert_eq!(
            result,
            Err(molt_runtime_core::integer_decimal::DecimalLimitExceeded { limit: 640 })
        );
        assert_eq!(observed.allocations, 0);
        assert_eq!(observed.allocated_bytes, 0);
    }

    static CALLBACK_LIMIT: AtomicUsize = AtomicUsize::new(640);
    static CALLBACKS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn decimal_from_callback(_self: *mut PyObject) -> *mut PyObject {
        CALLBACKS.fetch_add(1, Ordering::SeqCst);
        set_limit(CALLBACK_LIMIT.load(Ordering::SeqCst) as i64);
        // 2^2190 has 660 decimal digits. Public allocation yields a real native
        // exact int; the callback does not pre-render or consult output policy.
        let mut digits = [0u32; 74];
        digits[73] = 1;
        unsafe { percent_raw_long(&raw mut PyLong_Type, &digits).into_ptr() }
    }

    #[test]
    fn decimal_percent_reads_policy_after_native_numeric_callback() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(super::super::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let _restore = LimitRestore(crate::builtins::sys_ext::current_int_max_str_digits(py));
            let mut methods: PyNumberMethods = std::mem::zeroed();
            methods.nb_int = decimal_from_callback as *const () as *mut _;
            let mut source =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"DecimalPolicyCallback");
            source.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            source.tp_as_number = (&raw mut methods).cast();
            assert_eq!(source.ready(), 0);
            let value =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *source, 0));
            assert!(!value.as_ptr().is_null());
            CALLBACKS.store(0, Ordering::SeqCst);
            set_limit(640);
            CALLBACK_LIMIT.store(700, Ordering::SeqCst);
            assert_eq!(take_raw(percent(b"%d", value.as_ptr())).len(), 660);
            CALLBACK_LIMIT.store(640, Ordering::SeqCst);
            limit_failure(percent(b"%d", value.as_ptr()), "native callback live limit");
            assert_eq!(CALLBACKS.load(Ordering::SeqCst), 2);
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    #[test]
    fn decimal_output_bridge_failures_do_not_publish_buffer_ownership() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(super::super::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let _restore = LimitRestore(crate::builtins::sys_ext::current_int_max_str_digits(py));
            set_limit(640);
            let value =
                ExceptionValue::adopt(py, crate::bigint_bits(py, BigInt::from(10u8).pow(640)));
            let sentinel = b"unpublished".as_ptr();
            let exports: &[extern "C" fn(u64, *mut *const u8, *mut usize) -> i32] = &[
                #[cfg(feature = "stdlib_math")]
                crate::math_bridge::__molt_math_format_obj,
                #[cfg(feature = "stdlib_math")]
                crate::math_bridge::__molt_math_format_obj_str,
                #[cfg(feature = "stdlib_http")]
                crate::http_bridge::__molt_http_format_obj_str,
            ];
            for export in exports {
                let mut out = sentinel;
                let mut len = 777usize;
                assert_eq!(export(value.bits(), &raw mut out, &raw mut len), 0);
                assert_eq!((out, len), (sentinel, 777));
                limit_failure(ptr::null_mut(), "failed bridge output publication");
            }
            // Serial is a typed hook table; exercise its real CSV consumer.
            #[cfg(feature = "stdlib_csv")]
            {
                struct Writer(u64);
                impl Drop for Writer {
                    fn drop(&mut self) {
                        crate::molt_csv_writer_drop(self.0);
                    }
                }
                let string = |bytes: &[u8]| {
                    let ptr = crate::alloc_string(py, bytes);
                    assert!(!ptr.is_null());
                    ExceptionValue::adopt(py, crate::MoltObject::from_ptr(ptr).bits())
                };
                let comma = string(b",");
                let quote = string(b"\"");
                let newline = string(b"\r\n");
                let writer = Writer(crate::molt_csv_writer_new(
                    comma.bits(),
                    quote.bits(),
                    crate::MoltObject::none().bits(),
                    crate::MoltObject::from_bool(true).bits(),
                    crate::MoltObject::from_int(0).bits(),
                    newline.bits(),
                ));
                assert_ne!(writer.0, crate::MoltObject::none().bits());
                let row_ptr = crate::alloc_list(py, &[value.bits()]);
                assert!(!row_ptr.is_null());
                let row = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(row_ptr).bits());
                let rendered = crate::molt_csv_writer_writerow(
                    writer.0,
                    row.bits(),
                    crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::Exception),
                );
                assert_eq!(rendered, crate::MoltObject::none().bits());
                limit_failure(ptr::null_mut(), "failed bridge output publication");
            }
            assert!(errors::PyErr_Occurred().is_null());
        });
    }
}

static COMPLETION_RESULT: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static COMPLETION_SEEN: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static COMPLETION_FAILS: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn completion_truth(_value: *mut PyObject) -> std::os::raw::c_int {
    unsafe {
        let prior = errors::PyErr_GetRaisedException();
        COMPLETION_SEEN.store(prior, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(prior);
    }
    if COMPLETION_FAILS.load(Ordering::SeqCst) {
        -1
    } else {
        1
    }
}

unsafe extern "C" fn completion_compare(
    _left: *mut PyObject,
    _right: *mut PyObject,
    _operation: std::os::raw::c_int,
) -> *mut PyObject {
    unsafe { object::Py_NewRef(COMPLETION_RESULT.load(Ordering::SeqCst)) }
}

unsafe extern "C" fn completion_get_attribute(
    _self: *mut PyObject,
    _name: *mut PyObject,
) -> *mut PyObject {
    if unsafe { completion_truth(_self) } < 0 {
        ptr::null_mut()
    } else {
        unsafe { object::Py_NewRef((&raw mut Py_True).cast()) }
    }
}

unsafe extern "C" fn completion_set_attribute(
    _self: *mut PyObject,
    _name: *mut PyObject,
    _value: *mut PyObject,
) -> std::os::raw::c_int {
    if unsafe { completion_truth(_self) } < 0 {
        -1
    } else {
        0
    }
}

unsafe extern "C" fn completion_invalid_callable(
    _self: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    // This is deliberately invalid for a generic call with a pending error,
    // unlike a successful direct numeric slot with an admitted prior.
    unsafe { object::Py_NewRef((&raw mut Py_True).cast()) }
}

#[test]
fn native_comparison_truth_status_preserves_prior_and_distinguishes_same_instance_failure() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let prior_ptr = crate::builtins::exceptions::alloc_exception(
                py,
                BuiltinExc::LookupError,
                "admitted comparison error",
            );
            assert!(!prior_ptr.is_null());
            let prior = crate::builtins::exceptions::ExceptionValue::adopt(
                py,
                crate::MoltObject::from_ptr(prior_ptr).bits(),
            );
            let prior_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(prior.bits());
            assert!(!prior_view.is_null());
            let mut number_slots: PyNumberMethods = std::mem::zeroed();
            number_slots.nb_bool = completion_truth as *const () as *mut _;
            let mut truth_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"NativeCompletionTruth");
            truth_type.tp_as_number = (&raw mut number_slots).cast();
            assert_eq!(truth_type.ready(), 0);
            let truth =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *truth_type, 0));
            assert!(!truth.as_ptr().is_null());
            COMPLETION_RESULT.store(truth.as_ptr(), Ordering::SeqCst);
            let mut comparison_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"NativeCompletionCompare");
            comparison_type.tp_richcompare = Some(completion_compare);
            comparison_type.tp_getattro = Some(completion_get_attribute);
            comparison_type.tp_setattro = Some(completion_set_attribute);
            assert_eq!(comparison_type.ready(), 0);
            let left = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *comparison_type,
                0,
            ));
            let right = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *comparison_type,
                0,
            ));
            assert!(!left.as_ptr().is_null() && !right.as_ptr().is_null());
            let left_bits = molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                GLOBAL_BRIDGE.molt_value_for_pyobj(left.as_ptr()).unwrap(),
            );
            let right_bits = molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                GLOBAL_BRIDGE.molt_value_for_pyobj(right.as_ptr()).unwrap(),
            );
            let mut callable_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"InvalidPendingCallable");
            callable_type.tp_call = Some(completion_invalid_callable);
            assert_eq!(callable_type.ready(), 0);
            let callable =
                OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *callable_type, 0));
            assert!(!callable.as_ptr().is_null());
            let attribute_name =
                OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"value".as_ptr()));
            assert!(!attribute_name.as_ptr().is_null());
            for runtime_prior in [false, true] {
                for fail in [false, true] {
                    COMPLETION_FAILS.store(fail, Ordering::SeqCst);
                    COMPLETION_SEEN.store(ptr::null_mut(), Ordering::SeqCst);
                    if runtime_prior {
                        crate::record_exception(py, prior_ptr);
                    } else {
                        errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                    }
                    let outcome = crate::object::ops_compare::compare_object_eq_bool(
                        py,
                        crate::obj_from_bits(left_bits.bits()),
                        crate::obj_from_bits(right_bits.bits()),
                    );
                    assert!(matches!(
                        (outcome, fail),
                        (crate::object::ops_compare::CompareBoolOutcome::True, false)
                            | (crate::object::ops_compare::CompareBoolOutcome::Error, true)
                    ));
                    assert_eq!(COMPLETION_SEEN.load(Ordering::SeqCst), prior_view);
                    let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(
                        selected.as_ptr(),
                        prior_view,
                        "explicit -1 retains the exact reraised instance"
                    );
                }
                for fail in [false, true] {
                    for setter in [false, true] {
                        COMPLETION_FAILS.store(fail, Ordering::SeqCst);
                        COMPLETION_SEEN.store(ptr::null_mut(), Ordering::SeqCst);
                        if runtime_prior {
                            crate::record_exception(py, prior_ptr);
                        } else {
                            errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                        }
                        if setter {
                            assert_eq!(
                                object::PyObject_SetAttr(
                                    left.as_ptr(),
                                    attribute_name.as_ptr(),
                                    (&raw mut Py_True).cast()
                                ),
                                if fail { -1 } else { 0 }
                            );
                        } else {
                            let result = OwnedPyObject::from_owned(object::PyObject_GetAttr(
                                left.as_ptr(),
                                attribute_name.as_ptr(),
                            ));
                            assert_eq!(
                                result.as_ptr(),
                                if fail {
                                    ptr::null_mut()
                                } else {
                                    (&raw mut Py_True).cast()
                                }
                            );
                        }
                        assert_eq!(COMPLETION_SEEN.load(Ordering::SeqCst), prior_view);
                        let selected =
                            OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(selected.as_ptr(), prior_view);
                    }
                }
                if runtime_prior {
                    crate::record_exception(py, prior_ptr);
                } else {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(prior_view));
                }
                let result =
                    OwnedPyObject::from_owned(object::PyObject_CallNoArgs(callable.as_ptr()));
                assert!(
                    result.as_ptr().is_null(),
                    "generic callable non-NULL plus pending is invalid"
                );
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast()),
                    1
                );
                let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!selected.as_ptr().is_null());
            }
        }
    });
}

static COMPLETION_CALLS: AtomicUsize = AtomicUsize::new(0);
static COMPLETION_MANAGED_ERROR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

extern "C" fn completion_managed_bool(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        COMPLETION_CALLS.fetch_add(1, Ordering::SeqCst);
        COMPLETION_SEEN.store(unsafe { errors::PyErr_Occurred() }, Ordering::SeqCst);
        let failure = COMPLETION_MANAGED_ERROR.load(Ordering::SeqCst);
        if failure.is_null() {
            return crate::MoltObject::from_bool(true).bits();
        }
        // Managed Python bodies enter with no pending error. The fixture holds
        // this exact instance while the callback selects it as its own failure.
        crate::record_exception(py, failure);
        crate::MoltObject::none().bits()
    })
}

#[test]
fn managed_comparison_result_truth_failure_keeps_the_same_exception_instance() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (_class, result) = percent_managed_protocol(
                py,
                crate::builtin_classes(py).object,
                &[(b"__bool__", completion_managed_bool)],
                &[],
            );
            let failure_ptr = crate::builtins::exceptions::alloc_exception(
                py,
                BuiltinExc::LookupError,
                "managed comparison truth failure",
            );
            assert!(!failure_ptr.is_null());
            let failure = crate::builtins::exceptions::ExceptionValue::adopt(
                py,
                crate::MoltObject::from_ptr(failure_ptr).bits(),
            );
            let failure_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(failure.bits());
            assert!(!failure_view.is_null());
            crate::inc_ref_bits(py, result.bits());
            assert!(matches!(
                crate::object::ops_compare::comparison_value_to_bool(
                    py,
                    crate::object::ops_compare::CompareValueOutcome::Value(result.bits()),
                ),
                crate::object::ops_compare::CompareBoolOutcome::True
            ));
            assert_eq!(COMPLETION_CALLS.load(Ordering::SeqCst), 1);
            assert!(COMPLETION_SEEN.load(Ordering::SeqCst).is_null());
            assert!(errors::PyErr_Occurred().is_null());

            // Clear this borrowed pointer before its owning ExceptionValue can
            // retire, including if a callback or assertion unwinds.
            struct HeldFailure;
            impl Drop for HeldFailure {
                fn drop(&mut self) {
                    COMPLETION_MANAGED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                }
            }
            let held_failure = HeldFailure;
            COMPLETION_MANAGED_ERROR.store(failure_ptr, Ordering::SeqCst);
            COMPLETION_CALLS.store(0, Ordering::SeqCst);
            crate::inc_ref_bits(py, result.bits());
            let outcome = crate::object::ops_compare::comparison_value_to_bool(
                py,
                crate::object::ops_compare::CompareValueOutcome::Value(result.bits()),
            );
            drop(held_failure);
            assert!(matches!(
                outcome,
                crate::object::ops_compare::CompareBoolOutcome::Error
            ));
            assert_eq!(COMPLETION_CALLS.load(Ordering::SeqCst), 1);
            assert!(COMPLETION_SEEN.load(Ordering::SeqCst).is_null());
            assert!(crate::exception_pending(py));
            let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(selected.as_ptr(), failure_view);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}
