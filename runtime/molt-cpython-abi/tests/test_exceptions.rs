//! Tests for PyErr_SetString, PyErr_Occurred, PyErr_Clear, PyErr_SetNone,
//! PyErr_Print, PyErr_Format.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    PyBaseExceptionObject, PyExc_TypeError, PyExc_ValueError, PyExc_Warning, PyObject,
};
use std::ptr;

use molt_cpython_abi::hooks::PendingExceptionClass;
thread_local! { static EMERGENCY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static UNADMITTED_TEXT_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn init() -> support::AbiTestThreadStateTransaction {
    unsafe extern "C" fn text_without_class_authority(_: *const u8, _: usize) -> u64 {
        UNADMITTED_TEXT_ALLOCATIONS.with(|calls| calls.set(calls.get() + 1));
        0
    }
    unsafe extern "C" fn pending() -> std::ffi::c_int {
        EMERGENCY.with(|state| i32::from(state.get()))
    }
    unsafe extern "C" fn pending_class() -> PendingExceptionClass {
        if EMERGENCY.with(std::cell::Cell::get) {
            PendingExceptionClass::EmergencyMemoryError
        } else {
            PendingExceptionClass::None
        }
    }
    unsafe extern "C" fn clear() {
        EMERGENCY.with(|state| state.set(false));
    }
    unsafe extern "C" fn preserve(
        callback: unsafe extern "C" fn(*mut std::ffi::c_void),
        context: *mut std::ffi::c_void,
    ) {
        let saved = EMERGENCY.with(|state| state.replace(false));
        unsafe { callback(context) };
        EMERGENCY.with(|state| state.set(saved));
    }
    let mut hooks = support::stub_runtime_hooks();
    hooks.exception_pending = pending;
    hooks.pending_exception_class = pending_class;
    hooks.clear_pending_exception = clear;
    hooks.with_preserved_pending_exception = preserve;
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
fn pending_emergency_type_query_needs_no_runtime_class_bootstrap() {
    use molt_cpython_abi::abi_types::{PyExc_Exception, PyExc_MemoryError};
    use molt_cpython_abi::api::{errors, refcount};
    let _abi_test = init();
    EMERGENCY.with(|state| state.set(true));
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
    assert!(!EMERGENCY.with(std::cell::Cell::get));
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
fn set_from_errno_reports_memoryerror_without_string_allocator() {
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
                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast::<PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
