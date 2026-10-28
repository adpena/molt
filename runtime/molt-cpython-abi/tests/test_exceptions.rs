//! Tests for PyErr_SetString, PyErr_Occurred, PyErr_Clear, PyErr_SetNone,
//! PyErr_Print, PyErr_Format.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    PyBaseExceptionObject, PyExc_TypeError, PyExc_ValueError, PyExc_Warning, PyObject,
};
use std::ptr;

use molt_cpython_abi::hooks::PendingExceptionClass;
thread_local! { static PARTIAL_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static UNADMITTED_TEXT_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn init() -> support::AbiTestThreadStateTransaction {
    unsafe extern "C" fn text_without_class_authority(_: *const u8, _: usize) -> u64 {
        UNADMITTED_TEXT_ALLOCATIONS.with(|calls| calls.set(calls.get() + 1));
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetNone(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast(),
            );
        }
        0
    }
    unsafe extern "C" fn pending() -> std::ffi::c_int {
        PARTIAL_PENDING.with(|state| i32::from(state.get()))
    }
    unsafe extern "C" fn pending_class() -> PendingExceptionClass {
        if PARTIAL_PENDING.with(std::cell::Cell::get) {
            PendingExceptionClass::NativeClass(
                &raw mut molt_cpython_abi::abi_types::PyExc_MemoryError,
            )
        } else {
            PendingExceptionClass::None
        }
    }
    unsafe extern "C" fn clear() {
        PARTIAL_PENDING.with(|state| state.set(false));
    }
    unsafe extern "C" fn save_pending(
        callback: unsafe extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int,
        context: *mut std::ffi::c_void,
    ) {
        let saved = PARTIAL_PENDING.with(|state| state.replace(false));
        if unsafe { callback(context) } == 0 {
            PARTIAL_PENDING.with(|state| state.set(saved));
        }
    }
    let mut hooks = support::stub_runtime_hooks();
    hooks.exception_pending = pending;
    hooks.pending_exception_class = pending_class;
    hooks.clear_pending_exception = clear;
    hooks.with_saved_pending_exception = save_pending;
    hooks.alloc_str = Some(text_without_class_authority);
    support::enter_abi_test(hooks)
}

#[test]
fn diagnostic_text_requires_class_custody_before_allocation() {
    use molt_cpython_abi::api::{errors, refcount};
    let _abi_test = init();
    unsafe {
        errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"bootstrap".as_ptr());
        assert_eq!(UNADMITTED_TEXT_ALLOCATIONS.with(std::cell::Cell::get), 0);
        assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_ValueError).cast());
        let raised = errors::PyErr_GetRaisedException();
        let raised_owner = refcount::OwnedPyObject::from_owned(raised);
        assert!(
            !raised.is_null(),
            "native construction remains available without managed text"
        );
        assert_eq!((*raised).ob_type, &raw mut PyExc_ValueError);
        drop(raised_owner);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

// ---------------------------------------------------------------------------
// PyErr_SetString / PyErr_Occurred / PyErr_Clear
// ---------------------------------------------------------------------------

#[test]
fn test_no_exception_initially() {
    let _abi_test = init();
    // Clear any leftover state from other tests
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

#[test]
fn partial_provider_class_query_does_not_require_value_projection() {
    use molt_cpython_abi::abi_types::{PyExc_Exception, PyExc_MemoryError};
    use molt_cpython_abi::api::{errors, refcount};
    let _abi_test = init();
    PARTIAL_PENDING.with(|state| state.set(true));
    unsafe {
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_MemoryError).cast()
        );
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_Exception).cast()),
            1
        );
        let raised = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(raised.as_ptr().is_null());
        drop(raised);
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_MemoryError).cast()
        );
        assert!(errors::take_current_error().is_none());
        errors::PyErr_Clear();
        assert!(errors::PyErr_Occurred().is_null());
    }
    assert!(!PARTIAL_PENDING.with(std::cell::Cell::get));
}

#[test]
fn test_warning_exception_singleton_is_exported() {
    let _abi_test = init();
    let warning = (&raw mut PyExc_Warning).cast::<PyObject>();
    assert!(!warning.is_null());
}

#[test]
fn test_set_string_and_occurred() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"test error".as_ptr());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null(), "Exception should be set");

    // Clear and verify
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred2 = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred2.is_null(), "Exception should be cleared");
}

#[test]
fn take_current_error_transfers_physical_instance_and_clears_indicator() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"pyinit failed".as_ptr());
    }

    let error = molt_cpython_abi::api::errors::take_current_error()
        .expect("PyErr_SetString must install an exception");
    assert!(std::ptr::eq(error.exc_type, exc));
    assert!(!error.value.is_null());
    assert!(std::ptr::eq(
        unsafe { (*error.value).ob_type }.cast::<PyObject>(),
        exc
    ));
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}

#[test]
fn test_set_string_with_null_message() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_TypeError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, ptr::null());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_string_with_null_exc_type() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(ptr::null_mut(), c"msg".as_ptr());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyErr_SetNone
// ---------------------------------------------------------------------------

#[test]
fn test_set_none() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe { molt_cpython_abi::api::errors::PyErr_SetNone(exc) };

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyErr_Clear idempotent
// ---------------------------------------------------------------------------

#[test]
fn test_clear_when_no_exception() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Clearing when nothing set should be a noop
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

#[test]
fn test_double_clear() {
    let _abi_test = init();
    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"err".as_ptr());
    }
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

// ---------------------------------------------------------------------------
// PyErr_Print
// ---------------------------------------------------------------------------

#[test]
fn test_print_clears_exception() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"printed error".as_ptr());
    }

    // PyErr_Print should print and then clear
    unsafe { molt_cpython_abi::api::errors::PyErr_Print() };

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        occurred.is_null(),
        "Exception should be cleared after Print"
    );
}

#[test]
fn test_print_when_no_exception() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Should not crash
    unsafe { molt_cpython_abi::api::errors::PyErr_Print() };
}

// ---------------------------------------------------------------------------
// PyErr_Format
// ---------------------------------------------------------------------------

#[test]
fn test_format_sets_exception_returns_null() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_TypeError).cast::<PyObject>();
    let result = unsafe { molt_cpython_abi::api::errors::PyErr_Format(exc, c"bad type".as_ptr()) };
    assert!(result.is_null(), "PyErr_Format should return NULL");

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_fetch_consumes_current_error_message() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"fetch me".as_ptr());
    }

    let mut exc_type = ptr::null_mut();
    let mut exc_value = ptr::null_mut();
    let mut exc_tb = ptr::null_mut();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Fetch(&mut exc_type, &mut exc_value, &mut exc_tb);
    }
    let exc_type_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(exc_type) };
    let exc_value_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(exc_value) };
    let exc_tb_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(exc_tb) };

    // Fetch transfers the REAL type (Python/errors.c), not a Py_None sentinel:
    // Fetch/Restore round-trips must preserve exception identity.
    assert!(
        std::ptr::eq(exc_type, exc),
        "Fetch must hand back the real exception type"
    );
    // Bootstrap mode lacks the runtime str allocator, but physical exception
    // construction remains available and preserves the requested class.
    assert!(!exc_value.is_null());
    assert!(std::ptr::eq(
        unsafe { (*exc_value).ob_type }.cast::<PyObject>(),
        exc
    ));
    assert!(exc_tb.is_null());
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    // Restore re-installs what Fetch produced: the round-trip preserves type.
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Restore(
            exc_type_owner.into_ptr(),
            exc_value_owner.into_ptr(),
            exc_tb_owner.into_ptr(),
        );
    }
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut PyExc_ValueError).cast::<PyObject>(),
            )
        },
        1,
        "a Fetch -> Restore round-trip must preserve the exception type \
         (the pre-fix Restore stored a fabricated placeholder)"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn setobject_never_installs_the_argument_as_the_exception_value() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc_type = (&raw mut PyExc_TypeError).cast::<PyObject>();
    let payload = &raw mut molt_cpython_abi::abi_types::Py_None;
    unsafe { molt_cpython_abi::api::errors::PyErr_SetObject(exc_type, payload) };
    let mut fetched_type = ptr::null_mut();
    let mut fetched_value = ptr::null_mut();
    let mut fetched_tb = ptr::null_mut();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Fetch(
            &mut fetched_type,
            &mut fetched_value,
            &mut fetched_tb,
        );
    }
    let fetched_type_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(fetched_type) };
    let fetched_value_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(fetched_value) };
    let fetched_tb_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(fetched_tb) };
    assert!(std::ptr::eq(fetched_type, exc_type));
    assert!(
        !std::ptr::eq(fetched_value, payload),
        "SetObject(TypeError, Py_None) must construct TypeError() when runtime authority exists, never install Py_None as the error value"
    );
    let args = unsafe { (*fetched_value.cast::<PyBaseExceptionObject>()).args };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(args) },
        0,
        "CPython 3.12 normalizes an explicit Py_None value as zero arguments"
    );
    assert!(fetched_tb.is_null());
    drop(fetched_type_owner);
    drop(fetched_value_owner);
    drop(fetched_tb_owner);
}

#[test]
fn setnone_without_runtime_authority_preserves_class_and_empty_args() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc_type = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe { molt_cpython_abi::api::errors::PyErr_SetNone(exc_type) };
    let mut fetched_type = ptr::null_mut();
    let mut fetched_value = ptr::null_mut();
    let mut fetched_tb = ptr::null_mut();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Fetch(
            &mut fetched_type,
            &mut fetched_value,
            &mut fetched_tb,
        );
    }
    let fetched_type_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(fetched_type) };
    let fetched_value_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(fetched_value) };
    let fetched_tb_owner =
        unsafe { molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(fetched_tb) };
    assert!(std::ptr::eq(fetched_type, exc_type));
    assert!(!fetched_value.is_null());
    let args = unsafe { (*fetched_value.cast::<PyBaseExceptionObject>()).args };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(args) },
        0
    );
    drop(fetched_type_owner);
    drop(fetched_value_owner);
    drop(fetched_tb_owner);
}

#[test]
fn test_set_from_errno_sets_exception_returns_null() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_SetFromErrno(
            (&raw mut molt_cpython_abi::abi_types::PyExc_OSError).cast::<PyObject>(),
        )
    };
    assert!(result.is_null(), "PyErr_SetFromErrno should return NULL");

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// Overwrite exception
// ---------------------------------------------------------------------------

#[test]
fn test_overwrite_exception() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let val_exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    let type_exc = (&raw mut PyExc_TypeError).cast::<PyObject>();

    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(val_exc, c"first".as_ptr());
    }
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    // Overwrite with a different exception
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(type_exc, c"second".as_ptr());
    }
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// Fail-open burndown teeth: PyErr_SetObject / PyErr_ExceptionMatches /
// PyErr_WriteUnraisable no longer drop their real argument.
// ---------------------------------------------------------------------------

#[test]
fn test_err_setobject_sets_exception_without_generic_placeholder() {
    // F1 teeth: PyErr_SetObject previously dropped `value` (`let _ = value;`) and
    // set a generic c"<exception>" message. It must set the exception with the
    // caller's type and NOT fabricate the "<exception>" placeholder payload.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    // A NULL value cannot be resolved; the fix records the type with an empty
    // message rather than the misleading "<exception>" string.
    unsafe { molt_cpython_abi::api::errors::PyErr_SetObject(exc, ptr::null_mut()) };
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyErr_SetObject must leave an exception pending"
    );
    let error = molt_cpython_abi::api::errors::take_current_error()
        .expect("PyErr_SetObject must install an exception");
    assert!(std::ptr::eq(error.exc_type, exc));
    assert!(!error.value.is_null());
    let args = unsafe { (*error.value.cast::<PyBaseExceptionObject>()).args };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(args) },
        0
    );
}

#[test]
fn test_err_exception_matches_no_exception_returns_zero() {
    // PyErr_ExceptionMatches with no pending exception must return 0 (CPython's
    // PyErr_GivenExceptionMatches(NULL, exc) == 0), NOT the old "is any set" 1.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    let rc = unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(exc) };
    assert_eq!(rc, 0, "no pending exception => ExceptionMatches returns 0");
}

#[test]
fn test_err_exception_matches_does_not_report_any_pending() {
    // The old stub dropped `exc` and returned "is ANY exception set", so a pending
    // TypeError falsely matched PyExc_KeyError. The fix compares the pending
    // exception's stored type against `exc`. Under the stub bridge the PyExc_*
    // statics resolve to type bits 0, so a genuine match cannot be asserted here;
    // what we CAN prove is the burned-down fail-open: a pending exception must NOT
    // make ExceptionMatches return 1 unconditionally.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let type_exc = (&raw mut PyExc_TypeError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(type_exc, c"boom".as_ptr());
    }
    // A NULL candidate must never match.
    assert_eq!(
        unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(ptr::null_mut()) },
        0,
        "ExceptionMatches(NULL) must be 0 even with a pending exception"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_err_write_unraisable_does_not_panic_with_object() {
    // F1 teeth: PyErr_WriteUnraisable previously dropped `obj` (`let _ = obj;`).
    // It now includes obj's context in the report and must clear the exception.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = (&raw mut PyExc_ValueError).cast::<PyObject>();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"unraisable".as_ptr());
    }
    // NULL obj is a valid input (no context); must not panic and must clear.
    unsafe { molt_cpython_abi::api::errors::PyErr_WriteUnraisable(ptr::null_mut()) };
    assert!(
        unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyErr_WriteUnraisable must clear the reported exception"
    );
}

// ─── F1 identity-plumbing gates (ledger errors.rs:55/:189/:264/:276/:100) ───

#[test]
fn occurred_returns_the_real_pending_type() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = (&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast::<PyObject>();
    unsafe { molt_cpython_abi::api::errors::PyErr_SetString(exc, c"k".as_ptr()) };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        std::ptr::eq(occurred, exc),
        "PyErr_Occurred must return the REAL exception type (identity tests \
         like `PyErr_Occurred() == PyExc_StopIteration` depend on it) — the \
         pre-fix Py_None sentinel mis-decided every such probe"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn exception_matches_walks_the_builtin_subclass_chain() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Pending IndexError must match LookupError AND Exception AND BaseException
    // (the documented 3.12 hierarchy), and must NOT match KeyError/TypeError.
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_IndexError).cast::<PyObject>(),
            c"idx".as_ptr(),
        );
    }
    let m = |e: *mut PyObject| unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(e) };
    assert_eq!(
        m((&raw mut molt_cpython_abi::abi_types::PyExc_IndexError).cast::<PyObject>()),
        1
    );
    assert_eq!(
        m((&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast::<PyObject>()),
        1,
        "except LookupError must catch a pending IndexError (subclass walk)"
    );
    assert_eq!(
        m((&raw mut molt_cpython_abi::abi_types::PyExc_Exception).cast::<PyObject>()),
        1
    );
    assert_eq!(
        m((&raw mut molt_cpython_abi::abi_types::PyExc_BaseException).cast::<PyObject>()),
        1
    );
    assert_eq!(
        m((&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast::<PyObject>()),
        0
    );
    assert_eq!(m((&raw mut PyExc_TypeError).cast::<PyObject>()), 0);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn given_exception_matches_is_subclass_aware() {
    let _abi_test = init();
    let given = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ModuleNotFoundError).cast::<PyObject>(),
            (&raw mut molt_cpython_abi::abi_types::PyExc_ImportError).cast::<PyObject>(),
        )
    };
    assert_eq!(given, 1, "ModuleNotFoundError IS an ImportError");
    let not = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ImportError).cast::<PyObject>(),
            (&raw mut molt_cpython_abi::abi_types::PyExc_ModuleNotFoundError).cast::<PyObject>(),
        )
    };
    assert_eq!(not, 0, "the subclass relation is directional");
}

#[test]
fn bad_internal_call_is_system_error() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe { molt_cpython_abi::api::errors::PyErr_BadInternalCall() };
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
            )
        },
        1,
        "PyErr_BadInternalCall sets SystemError (Python/errors.c), not RuntimeError"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn set_from_errno_reports_refusal_without_string_allocator() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_SetFromErrno(
            (&raw mut molt_cpython_abi::abi_types::PyExc_OSError).cast::<PyObject>(),
        )
    };
    assert!(result.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// Python/errors.c publishes a new raised instance before decrefing the old
// instance. The three legacy normalization entry points deliberately differ
// in when they detach a constructor failure. Observe those phases through real
// native deallocators, without replacing the exception they are observing.
mod native_retirement {
    use super::*;
    use molt_cpython_abi::abi_types::{
        Py_TPFLAGS_BASE_EXC_SUBCLASS, Py_TPFLAGS_READY, PyBaseObject_Type, PyTraceBack_Type,
        PyType_Type, PyTypeObject,
    };
    use molt_cpython_abi::api::{errors, object, refcount};
    use std::cell::{Cell, RefCell};

    const OLD: usize = 1;
    const INPUT: usize = 2;
    const INVALID_RESULT: usize = 3;
    const CONSTRUCTOR: usize = 4;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Observation {
        role: usize,
        exception: usize,
        traceback: usize,
    }

    #[derive(Clone, Copy)]
    enum ConstructorResult {
        Exception,
        OtherException,
        Failure,
        Invalid,
    }

    thread_local! {
        static OBSERVATIONS: RefCell<Vec<Observation>> = const { RefCell::new(Vec::new()) };
        static RESULT: Cell<ConstructorResult> = const { Cell::new(ConstructorResult::Exception) };
        static FAILURE: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
        static INPUT_TYPE: Cell<*mut PyTypeObject> = const { Cell::new(ptr::null_mut()) };
        static INPUT_OWNER: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
        static INPUT_OBSERVED_ERROR: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
    }

    #[repr(C)]
    struct NativeException {
        base: PyBaseExceptionObject,
        role: usize,
    }

    #[repr(C)]
    struct NativeInput {
        base: PyObject,
        role: usize,
    }

    unsafe fn observe(role: usize) {
        if role == 0 {
            return;
        }
        let exception = unsafe { errors::PyErr_GetRaisedException() };
        let traceback = if exception.is_null() {
            ptr::null_mut()
        } else {
            unsafe { errors::PyException_GetTraceback(exception) }
        };
        if role == INPUT {
            let retained = if exception.is_null() {
                ptr::null_mut()
            } else {
                unsafe { object::Py_NewRef(exception) }
            };
            let previous = INPUT_OBSERVED_ERROR.with(|slot| slot.replace(retained));
            unsafe { refcount::Py_XDECREF(previous) };
        }
        OBSERVATIONS.with(|events| {
            events.borrow_mut().push(Observation {
                role,
                exception: exception as usize,
                traceback: traceback as usize,
            });
        });
        unsafe {
            refcount::Py_XDECREF(traceback);
            errors::PyErr_SetRaisedException(exception);
        }
    }

    unsafe extern "C" fn exception_drop(value: *mut PyObject) {
        let value = unsafe { Box::from_raw(value.cast::<NativeException>()) };
        unsafe { observe(value.role) };
        for field in [
            value.base.dict,
            value.base.args,
            value.base.notes,
            value.base.traceback,
            value.base.context,
            value.base.cause,
        ] {
            unsafe { refcount::Py_XDECREF(field) };
        }
    }

    unsafe extern "C" fn input_drop(value: *mut PyObject) {
        let value = unsafe { Box::from_raw(value.cast::<NativeInput>()) };
        unsafe { observe(value.role) };
    }

    unsafe fn exception(class: *mut PyTypeObject, role: usize) -> refcount::OwnedPyObject {
        let mut base: PyBaseExceptionObject = unsafe { std::mem::zeroed() };
        base.ob_base.ob_refcnt = 1;
        base.ob_base.ob_type = class;
        unsafe {
            refcount::OwnedPyObject::from_owned(
                Box::into_raw(Box::new(NativeException { base, role })).cast(),
            )
        }
    }

    unsafe fn input(class: *mut PyTypeObject, role: usize) -> refcount::OwnedPyObject {
        unsafe {
            refcount::OwnedPyObject::from_owned(
                Box::into_raw(Box::new(NativeInput {
                    base: PyObject {
                        ob_refcnt: 1,
                        ob_type: class,
                    },
                    role,
                }))
                .cast(),
            )
        }
    }

    unsafe extern "C" fn construct(
        class: *mut PyTypeObject,
        _args: *mut PyObject,
        _kwargs: *mut PyObject,
    ) -> *mut PyObject {
        unsafe { observe(CONSTRUCTOR) };
        // SetObject borrows the input. Its owner is a removable external edge:
        // release that edge during the callback so the API's pin is the last
        // owner after the call-argument tuple has retired.
        let external = INPUT_OWNER.with(|owner| owner.replace(ptr::null_mut()));
        unsafe { refcount::Py_XDECREF(external) };
        match RESULT.with(Cell::get) {
            ConstructorResult::Exception => unsafe { exception(class, 0).into_ptr() },
            ConstructorResult::OtherException => unsafe {
                object::Py_NewRef(FAILURE.with(Cell::get))
            },
            ConstructorResult::Failure => {
                let failure = FAILURE.with(Cell::get);
                unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(failure)) };
                ptr::null_mut()
            }
            ConstructorResult::Invalid => unsafe {
                input(INPUT_TYPE.with(Cell::get), INVALID_RESULT).into_ptr()
            },
        }
    }

    fn classes() -> (support::StaticType, support::StaticType) {
        let mut exception_class = support::StaticType::new();
        exception_class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        exception_class.tp_name = c"NativePhaseError".as_ptr();
        exception_class.tp_base = &raw mut PyExc_ValueError;
        exception_class.tp_flags = Py_TPFLAGS_READY | Py_TPFLAGS_BASE_EXC_SUBCLASS;
        exception_class.tp_basicsize = std::mem::size_of::<NativeException>() as isize;
        exception_class.tp_dealloc = Some(exception_drop);
        exception_class.tp_new = Some(construct);
        let mut input_class = support::StaticType::new();
        input_class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        input_class.tp_name = c"NativePhaseInput".as_ptr();
        input_class.tp_base = &raw mut PyBaseObject_Type;
        input_class.tp_flags = Py_TPFLAGS_READY;
        input_class.tp_basicsize = std::mem::size_of::<NativeInput>() as isize;
        input_class.tp_dealloc = Some(input_drop);
        (exception_class, input_class)
    }

    struct PendingReset;
    impl Drop for PendingReset {
        fn drop(&mut self) {
            unsafe { errors::PyErr_Clear() };
        }
    }

    struct ConstructorControl;
    impl Drop for ConstructorControl {
        fn drop(&mut self) {
            FAILURE.with(|value| value.set(ptr::null_mut()));
            INPUT_TYPE.with(|value| value.set(ptr::null_mut()));
            let external = INPUT_OWNER.with(|value| value.replace(ptr::null_mut()));
            let observed = INPUT_OBSERVED_ERROR.with(|value| value.replace(ptr::null_mut()));
            // Explicit fixture rollback after an assertion failure. Semantic
            // callback retirement above is ordinary and is what the tests see.
            errors::with_preserved_error(|| unsafe {
                refcount::Py_XDECREF(external);
                refcount::Py_XDECREF(observed);
            });
        }
    }

    fn observations() -> Vec<Observation> {
        OBSERVATIONS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }

    #[test]
    fn saved_error_scope_distinguishes_success_failure_unwind_and_cleanup() {
        let _abi_test = init();
        let (class, _input_class) = classes();
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyTraceBack_Type,
        };
        let prior = unsafe { exception(class.as_ptr(), 0) };
        let next = unsafe { exception(class.as_ptr(), 0) };
        let _pending = PendingReset;
        unsafe {
            assert_eq!(
                errors::PyException_SetTraceback(prior.as_ptr(), &raw mut traceback),
                0
            );
            assert_eq!(
                errors::PyException_SetTraceback(next.as_ptr(), &raw mut traceback),
                0
            );
            errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
            PARTIAL_PENDING.with(|state| state.set(true));
            let result = errors::with_saved_error(|| {
                assert!(!errors::raised_error_pending());
                Ok(37)
            });
            assert_eq!(result.unwrap(), 37);
            assert!(
                PARTIAL_PENDING.with(Cell::get),
                "success restores allocation-free runtime state"
            );
            let restored = errors::take_current_error().expect("incoming C state restored");
            assert_eq!(restored.value, prior.as_ptr());
            assert_eq!(restored.traceback, &raw mut traceback);
            drop(restored);
            errors::PyErr_Clear();

            errors::PyErr_SetRaisedException(exception(class.as_ptr(), OLD).into_ptr());
            observations();
            let failed: Result<(), _> = errors::with_saved_error(|| {
                errors::PyErr_SetRaisedException(object::Py_NewRef(next.as_ptr()));
                Err(molt_cpython_abi::ErrorIndicatorSet)
            });
            assert!(failed.is_err());
            assert_eq!(
                observations(),
                [Observation {
                    role: OLD,
                    exception: next.as_ptr() as usize,
                    traceback: (&raw mut traceback) as usize,
                }],
                "failure retires old state under the selected exact error"
            );
            let selected = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(selected.as_ptr(), next.as_ptr());
            drop(selected);

            for protected_cleanup in [false, true] {
                errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                PARTIAL_PENDING.with(|state| state.set(true));
                if protected_cleanup {
                    assert_eq!(
                        errors::with_preserved_error(|| {
                            assert!(!errors::raised_error_pending());
                            errors::PyErr_SetRaisedException(object::Py_NewRef(next.as_ptr()));
                            PARTIAL_PENDING.with(|state| state.set(true));
                            53
                        }),
                        53
                    );
                } else {
                    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let _: Result<(), _> = errors::with_saved_error(|| {
                            assert!(!errors::raised_error_pending());
                            errors::PyErr_SetRaisedException(object::Py_NewRef(next.as_ptr()));
                            PARTIAL_PENDING.with(|state| state.set(true));
                            panic!("preparation unwind oracle");
                        });
                    }));
                    assert!(
                        unwind.is_err(),
                        "unwind resumes only after the C callback returns"
                    );
                }
                assert!(PARTIAL_PENDING.with(Cell::get));
                let restored = errors::take_current_error().expect("incoming C state restored");
                assert_eq!(restored.value, prior.as_ptr());
                assert_eq!(restored.traceback, &raw mut traceback);
                drop(restored);
                errors::PyErr_Clear();
            }
        }
    }

    #[test]
    fn failed_saved_error_scope_keeps_partial_provider_failure_selected() {
        let _abi_test = init();
        let (class, _input_class) = classes();
        let _pending = PendingReset;
        for incoming_runtime in [false, true] {
            unsafe {
                errors::PyErr_SetRaisedException(exception(class.as_ptr(), 0).into_ptr());
            }
            PARTIAL_PENDING.with(|state| state.set(incoming_runtime));
            let allocations = UNADMITTED_TEXT_ALLOCATIONS.with(Cell::get);
            let result: Result<(), _> = errors::with_saved_error(|| {
                assert!(!errors::raised_error_pending());
                PARTIAL_PENDING.with(|state| state.set(true));
                Err(molt_cpython_abi::ErrorIndicatorSet)
            });
            assert!(result.is_err());
            assert!(errors::take_current_error().is_none());
            assert!(PARTIAL_PENDING.with(Cell::get));
            assert_eq!(UNADMITTED_TEXT_ALLOCATIONS.with(Cell::get), allocations);
            unsafe {
                assert_eq!(
                    errors::PyErr_Occurred(),
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
                );
                assert!(errors::PyErr_GetRaisedException().is_null());
                assert!(
                    PARTIAL_PENDING.with(Cell::get),
                    "failed projection does not consume the partial provider state"
                );
                errors::PyErr_Clear();
            }
        }
    }

    #[test]
    fn explicit_context_chain_consumes_owner_breaks_cycles_and_preserves_cause() {
        let _abi_test = init();
        let (class, _input_class) = classes();
        unsafe {
            let selected = exception(class.as_ptr(), 0);
            let prior = exception(class.as_ptr(), 0);
            let link = exception(class.as_ptr(), 0);
            let cause = exception(class.as_ptr(), 0);
            errors::PyException_SetCause(selected.as_ptr(), object::Py_NewRef(cause.as_ptr()));
            for cycle in [false, true] {
                errors::PyException_SetContext(selected.as_ptr(), ptr::null_mut());
                errors::PyException_SetContext(prior.as_ptr(), object::Py_NewRef(link.as_ptr()));
                errors::PyException_SetContext(
                    link.as_ptr(),
                    object::Py_NewRef(if cycle {
                        selected.as_ptr()
                    } else {
                        prior.as_ptr()
                    }),
                );
                let before = (*prior.as_ptr()).ob_refcnt;
                assert!(errors::chain_owned_exception_context(
                    selected.as_ptr(),
                    object::Py_NewRef(prior.as_ptr()),
                ));
                assert_eq!((*prior.as_ptr()).ob_refcnt, before + 1);
                let context = refcount::OwnedPyObject::from_owned(errors::PyException_GetContext(
                    selected.as_ptr(),
                ));
                assert_eq!(context.as_ptr(), prior.as_ptr());
                let observed_cause = refcount::OwnedPyObject::from_owned(
                    errors::PyException_GetCause(selected.as_ptr()),
                );
                assert_eq!(observed_cause.as_ptr(), cause.as_ptr());
                assert_eq!(
                    (*selected.as_ptr().cast::<PyBaseExceptionObject>()).suppress_context,
                    1
                );
                if cycle {
                    assert!(errors::PyException_GetContext(link.as_ptr()).is_null());
                }
                // A repeated instance never gains a self context or a leaked
                // reference; the existing graph is preserved.
                let before = (*selected.as_ptr()).ob_refcnt;
                assert!(errors::chain_owned_exception_context(
                    selected.as_ptr(),
                    object::Py_NewRef(selected.as_ptr()),
                ));
                assert_eq!((*selected.as_ptr()).ob_refcnt, before);
                drop(context);
                drop(observed_cause);
                for value in [selected.as_ptr(), prior.as_ptr(), link.as_ptr()] {
                    errors::PyException_SetContext(value, ptr::null_mut());
                }
            }
            errors::PyException_SetCause(selected.as_ptr(), ptr::null_mut());
            assert!(errors::PyErr_Occurred().is_null());
        }
    }

    #[test]
    fn replacement_publishes_exact_next_before_native_old_owner_retirement() {
        let _abi_test = init();
        let (class, _input_class) = classes();
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyTraceBack_Type,
        };
        let next = unsafe { exception(class.as_ptr(), 0) };
        let _pending = PendingReset;
        unsafe {
            assert_eq!(
                errors::PyException_SetTraceback(next.as_ptr(), &raw mut traceback),
                0
            );
            for operation in ["raised", "setobject", "restore", "clear", "raised-null"] {
                errors::PyErr_SetRaisedException(exception(class.as_ptr(), OLD).into_ptr());
                observations();
                match operation {
                    "raised" => errors::PyErr_SetRaisedException(object::Py_NewRef(next.as_ptr())),
                    "setobject" => errors::PyErr_SetObject(class.as_ptr().cast(), next.as_ptr()),
                    "restore" => errors::PyErr_Restore(
                        object::Py_NewRef(class.as_ptr().cast()),
                        object::Py_NewRef(next.as_ptr()),
                        object::Py_NewRef(&raw mut traceback),
                    ),
                    "clear" => errors::PyErr_Clear(),
                    "raised-null" => errors::PyErr_SetRaisedException(ptr::null_mut()),
                    _ => unreachable!(),
                }
                let expected = if matches!(operation, "clear" | "raised-null") {
                    ptr::null_mut()
                } else {
                    next.as_ptr()
                };
                assert_eq!(
                    observations(),
                    [Observation {
                        role: OLD,
                        exception: expected as usize,
                        traceback: if expected.is_null() {
                            0
                        } else {
                            (&raw mut traceback) as usize
                        },
                    }],
                    "{operation} must publish before the old native destructor runs",
                );
                let selected =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(selected.as_ptr(), expected, "{operation}");
                drop(selected);
                assert!(errors::PyErr_Occurred().is_null());
                assert_eq!((*next.as_ptr()).ob_refcnt, 1);
            }
            drop(next);
            assert_eq!(traceback.ob_refcnt, 1);
        }
    }

    #[test]
    fn constructor_entry_clears_prior_only_when_setobject_requires_construction() {
        let _abi_test = init();
        let (class, _input_class) = classes();
        let _pending = PendingReset;
        let _control = ConstructorControl;
        RESULT.with(|result| result.set(ConstructorResult::Exception));
        unsafe {
            for operation in ["setnone", "setstring"] {
                errors::PyErr_SetRaisedException(exception(class.as_ptr(), OLD).into_ptr());
                observations();
                match operation {
                    "setnone" => errors::PyErr_SetNone(class.as_ptr().cast()),
                    "setstring" => {
                        errors::PyErr_SetString(class.as_ptr().cast(), c"input".as_ptr())
                    }
                    _ => unreachable!(),
                }
                assert_eq!(
                    observations(),
                    [
                        Observation {
                            role: OLD,
                            exception: 0,
                            traceback: 0
                        },
                        Observation {
                            role: CONSTRUCTOR,
                            exception: 0,
                            traceback: 0
                        },
                    ],
                    "{operation}",
                );
                let selected =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!selected.as_ptr().is_null(), "{operation}");
                assert_eq!((*selected.as_ptr()).ob_type, class.as_ptr(), "{operation}");
                drop(selected);
            }
        }
    }

    #[test]
    fn constructor_failure_and_invalid_result_retire_inputs_in_ingress_order() {
        let _abi_test = init();
        let (class, input_class) = classes();
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyTraceBack_Type,
        };
        let failure = unsafe { exception(class.as_ptr(), 0) };
        let _pending = PendingReset;
        let _control = ConstructorControl;
        FAILURE.with(|value| value.set(failure.as_ptr()));
        INPUT_TYPE.with(|value| value.set(input_class.as_ptr()));
        unsafe {
            assert_eq!(
                errors::PyException_SetTraceback(failure.as_ptr(), &raw mut traceback),
                0
            );
            for invalid in [false, true] {
                RESULT.with(|result| {
                    result.set(if invalid {
                        ConstructorResult::Invalid
                    } else {
                        ConstructorResult::Failure
                    })
                });
                for ingress in ["setobject", "restore", "normalize"] {
                    observations();
                    let argument = input(input_class.as_ptr(), INPUT);
                    let selected = match ingress {
                        "setobject" => {
                            let argument = argument.into_ptr();
                            INPUT_OWNER.with(|owner| owner.set(argument));
                            errors::PyErr_SetObject(class.as_ptr().cast(), argument);
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException())
                        }
                        "restore" => {
                            errors::PyErr_Restore(
                                object::Py_NewRef(class.as_ptr().cast()),
                                argument.into_ptr(),
                                ptr::null_mut(),
                            );
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException())
                        }
                        "normalize" => {
                            let mut triple = errors::OwnedCError {
                                exc_type: object::Py_NewRef(class.as_ptr().cast()),
                                value: argument.into_ptr(),
                                traceback: ptr::null_mut(),
                            };
                            errors::PyErr_NormalizeException(
                                &raw mut triple.exc_type,
                                &raw mut triple.value,
                                &raw mut triple.traceback,
                            );
                            assert!(errors::PyErr_Occurred().is_null());
                            refcount::OwnedPyObject::from_owned(std::mem::replace(
                                &mut triple.value,
                                ptr::null_mut(),
                            ))
                        }
                        _ => unreachable!(),
                    };
                    assert!(!selected.as_ptr().is_null(), "{ingress}, invalid={invalid}");
                    if invalid {
                        assert_eq!((*selected.as_ptr()).ob_type, &raw mut PyExc_TypeError);
                    } else {
                        assert_eq!(selected.as_ptr(), failure.as_ptr(), "{ingress}");
                    }
                    let input_error = refcount::OwnedPyObject::from_owned(
                        INPUT_OBSERVED_ERROR.with(|slot| slot.replace(ptr::null_mut())),
                    );
                    assert!(!input_error.as_ptr().is_null());
                    if ingress == "setobject" {
                        // This binary deliberately has no text-class authority.
                        // All three note text attempts fail, so CPython's final
                        // type-only note allocation leaves the provider's refusal visible
                        // while the input retires. The original constructor
                        // failure is restored only after that retirement.
                        assert_eq!(
                            (*input_error.as_ptr()).ob_type,
                            &raw mut molt_cpython_abi::abi_types::PyExc_SystemError
                        );
                        assert_ne!(input_error.as_ptr(), selected.as_ptr());
                    } else {
                        assert_eq!(input_error.as_ptr(), selected.as_ptr());
                    }
                    let expected_tb = if invalid {
                        0
                    } else {
                        (&raw mut traceback) as usize
                    };
                    let mut expected = vec![Observation {
                        role: CONSTRUCTOR,
                        exception: 0,
                        traceback: 0,
                    }];
                    if invalid {
                        expected.push(Observation {
                            role: INVALID_RESULT,
                            exception: selected.as_ptr() as usize,
                            traceback: expected_tb,
                        });
                    }
                    expected.push(Observation {
                        role: INPUT,
                        exception: input_error.as_ptr() as usize,
                        traceback: if ingress == "setobject" {
                            0
                        } else {
                            expected_tb
                        },
                    });
                    assert_eq!(observations(), expected, "{ingress}, invalid={invalid}");
                    assert!(INPUT_OWNER.with(Cell::get).is_null());
                    drop(selected);
                    assert!(errors::PyErr_Occurred().is_null());
                }
            }
            drop(failure);
            assert_eq!(traceback.ob_refcnt, 1);
        }
    }

    #[test]
    fn normalize_retains_requested_type_only_for_a_constructor_created_value() {
        let _abi_test = init();
        let (requested, _input_class) = classes();
        let (other_class, _other_input_class) = classes();
        let other = unsafe { exception(other_class.as_ptr(), 0) };
        let _pending = PendingReset;
        let _control = ConstructorControl;
        FAILURE.with(|value| value.set(other.as_ptr()));
        RESULT.with(|result| result.set(ConstructorResult::OtherException));
        unsafe {
            for ingress in ["setobject", "restore", "normalize"] {
                observations();
                let mut triple = errors::OwnedCError {
                    exc_type: object::Py_NewRef(requested.as_ptr().cast()),
                    value: ptr::null_mut(),
                    traceback: ptr::null_mut(),
                };
                match ingress {
                    "normalize" => errors::PyErr_NormalizeException(
                        &raw mut triple.exc_type,
                        &raw mut triple.value,
                        &raw mut triple.traceback,
                    ),
                    "restore" => {
                        errors::PyErr_Restore(
                            std::mem::replace(&mut triple.exc_type, ptr::null_mut()),
                            ptr::null_mut(),
                            ptr::null_mut(),
                        );
                        errors::PyErr_Fetch(
                            &raw mut triple.exc_type,
                            &raw mut triple.value,
                            &raw mut triple.traceback,
                        );
                    }
                    "setobject" => {
                        errors::PyErr_SetObject(requested.as_ptr().cast(), ptr::null_mut());
                        refcount::Py_DECREF(std::mem::replace(
                            &mut triple.exc_type,
                            ptr::null_mut(),
                        ));
                        errors::PyErr_Fetch(
                            &raw mut triple.exc_type,
                            &raw mut triple.value,
                            &raw mut triple.traceback,
                        );
                    }
                    _ => unreachable!(),
                }
                assert_eq!(triple.value, other.as_ptr(), "{ingress}");
                assert_eq!(
                    triple.exc_type,
                    if ingress == "normalize" {
                        requested.as_ptr().cast()
                    } else {
                        other_class.as_ptr().cast()
                    },
                    "{ingress} has its own legacy type contract",
                );
                assert_eq!(
                    observations(),
                    [Observation {
                        role: CONSTRUCTOR,
                        exception: 0,
                        traceback: 0
                    }]
                );
                drop(triple);
                assert!(errors::PyErr_Occurred().is_null());
                assert_eq!((*other.as_ptr()).ob_refcnt, 1);
            }
        }
    }

    #[test]
    fn restore_retires_requested_type_after_the_actual_value_is_published() {
        const REQUESTED_TYPE: usize = 5;
        unsafe extern "C" fn observe_type_drop(_value: *mut PyObject) {
            unsafe { observe(REQUESTED_TYPE) };
            // StaticType retains physical storage custody, independently of
            // this last semantic reference to the requested class.
        }

        let _abi_test = init();
        let mut metaclass = support::StaticType::new();
        metaclass.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        metaclass.tp_name = c"NativePhaseMeta".as_ptr();
        metaclass.tp_base = &raw mut PyType_Type;
        metaclass.tp_flags =
            Py_TPFLAGS_READY | molt_cpython_abi::abi_types::Py_TPFLAGS_TYPE_SUBCLASS;
        metaclass.tp_basicsize = std::mem::size_of::<PyTypeObject>() as isize;
        metaclass.tp_dealloc = Some(observe_type_drop);
        metaclass.tp_call = Some(molt_cpython_abi::api::typeobj::molt_type_call);
        let (mut requested, _input_class) = classes();
        requested.ob_base.ob_base.ob_type = metaclass.as_ptr();
        let (actual_class, _actual_input_class) = classes();
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyTraceBack_Type,
        };
        let actual = unsafe { exception(actual_class.as_ptr(), 0) };
        let _pending = PendingReset;
        let _control = ConstructorControl;
        FAILURE.with(|value| value.set(actual.as_ptr()));
        RESULT.with(|result| result.set(ConstructorResult::OtherException));
        unsafe {
            assert_eq!(requested.ob_base.ob_base.ob_refcnt, 1);
            // Restore supplies the traceback after construction. The actual
            // instance remains independently owned so the observer can inspect
            // its exact selected identity and traceback safely.
            observations();
            errors::PyErr_Restore(
                requested.as_ptr().cast(),
                ptr::null_mut(),
                object::Py_NewRef(&raw mut traceback),
            );
            assert_eq!(
                observations(),
                [
                    Observation {
                        role: CONSTRUCTOR,
                        exception: 0,
                        traceback: 0
                    },
                    Observation {
                        role: REQUESTED_TYPE,
                        exception: actual.as_ptr() as usize,
                        traceback: (&raw mut traceback) as usize
                    },
                ]
            );
            let selected = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(selected.as_ptr(), actual.as_ptr());
            assert_eq!(requested.ob_base.ob_base.ob_refcnt, 0);
            drop(selected);
            drop(actual);
            assert_eq!(traceback.ob_refcnt, 1);
        }
    }
}
