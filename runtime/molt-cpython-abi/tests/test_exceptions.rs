//! Tests for PyErr_SetString, PyErr_Occurred, PyErr_Clear, PyErr_SetNone,
//! PyErr_Print, PyErr_Format.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{PyExc_TypeError, PyExc_ValueError, PyExc_Warning};
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyErr_SetString / PyErr_Occurred / PyErr_Clear
// ---------------------------------------------------------------------------

#[test]
fn test_no_exception_initially() {
    init();
    // Clear any leftover state from other tests
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

#[test]
fn test_warning_exception_singleton_is_exported() {
    init();
    let warning = &raw mut PyExc_Warning;
    assert!(!warning.is_null());
}

#[test]
fn test_set_string_and_occurred() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
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
fn test_take_current_error_message_consumes_exception() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"pyinit failed".as_ptr());
    }

    assert_eq!(
        molt_cpython_abi::api::errors::take_current_error_message().as_deref(),
        Some("pyinit failed")
    );
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}

#[test]
fn test_set_string_with_null_message() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_TypeError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, ptr::null());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_string_with_null_exc_type() {
    init();
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Clearing when nothing set should be a noop
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

#[test]
fn test_double_clear() {
    init();
    let exc = &raw mut PyExc_ValueError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Should not crash
    unsafe { molt_cpython_abi::api::errors::PyErr_Print() };
}

// ---------------------------------------------------------------------------
// PyErr_Format
// ---------------------------------------------------------------------------

#[test]
fn test_format_sets_exception_returns_null() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_TypeError;
    let result = unsafe { molt_cpython_abi::api::errors::PyErr_Format(exc, c"bad type".as_ptr()) };
    assert!(result.is_null(), "PyErr_Format should return NULL");

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_fetch_consumes_current_error_message() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"fetch me".as_ptr());
    }

    let mut exc_type = ptr::null_mut();
    let mut exc_value = ptr::null_mut();
    let mut exc_tb = ptr::null_mut();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Fetch(&mut exc_type, &mut exc_value, &mut exc_tb);
    }

    // Fetch transfers the REAL type (Python/errors.c), not a Py_None sentinel:
    // Fetch/Restore round-trips must preserve exception identity.
    assert!(std::ptr::eq(exc_type, exc), "Fetch must hand back the real exception type");
    // Under stub hooks the str authority is absent, so the value cannot be
    // materialized; crucially Fetch must NOT clobber the indicator with a
    // MemoryError while trying (with a runtime it is the message str).
    assert!(exc_value.is_null());
    assert!(exc_tb.is_null());
    assert_eq!(
        molt_cpython_abi::api::errors::take_current_error_message(),
        None,
        "Fetch clears the indicator and leaves NO substitute exception"
    );

    // Restore re-installs what Fetch produced: the round-trip preserves type.
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Restore(exc_type, exc_value, exc_tb);
    }
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(&raw mut PyExc_ValueError)
        },
        1,
        "a Fetch -> Restore round-trip must preserve the exception type \
         (the pre-fix Restore stored a fabricated placeholder)"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_from_errno_sets_exception_returns_null() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_SetFromErrno(
            &raw mut molt_cpython_abi::abi_types::PyExc_OSError,
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let val_exc = &raw mut PyExc_ValueError;
    let type_exc = &raw mut PyExc_TypeError;

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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = &raw mut PyExc_ValueError;
    // A NULL value cannot be resolved; the fix records the type with an empty
    // message rather than the misleading "<exception>" string.
    unsafe { molt_cpython_abi::api::errors::PyErr_SetObject(exc, ptr::null_mut()) };
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyErr_SetObject must leave an exception pending"
    );
    let msg = molt_cpython_abi::api::errors::take_current_error_message();
    assert_ne!(
        msg.as_deref(),
        Some("<exception>"),
        "PyErr_SetObject must not fabricate the generic <exception> placeholder"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_err_exception_matches_no_exception_returns_zero() {
    // PyErr_ExceptionMatches with no pending exception must return 0 (CPython's
    // PyErr_GivenExceptionMatches(NULL, exc) == 0), NOT the old "is any set" 1.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = &raw mut PyExc_ValueError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let type_exc = &raw mut PyExc_TypeError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = &raw mut PyExc_ValueError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let exc = &raw mut molt_cpython_abi::abi_types::PyExc_KeyError;
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
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Pending IndexError must match LookupError AND Exception AND BaseException
    // (the documented 3.12 hierarchy), and must NOT match KeyError/TypeError.
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            &raw mut molt_cpython_abi::abi_types::PyExc_IndexError,
            c"idx".as_ptr(),
        );
    }
    let m = |e: *mut molt_cpython_abi::abi_types::PyObject| unsafe {
        molt_cpython_abi::api::errors::PyErr_ExceptionMatches(e)
    };
    assert_eq!(m(&raw mut molt_cpython_abi::abi_types::PyExc_IndexError), 1);
    assert_eq!(
        m(&raw mut molt_cpython_abi::abi_types::PyExc_LookupError),
        1,
        "except LookupError must catch a pending IndexError (subclass walk)"
    );
    assert_eq!(m(&raw mut molt_cpython_abi::abi_types::PyExc_Exception), 1);
    assert_eq!(m(&raw mut molt_cpython_abi::abi_types::PyExc_BaseException), 1);
    assert_eq!(m(&raw mut molt_cpython_abi::abi_types::PyExc_KeyError), 0);
    assert_eq!(m(&raw mut PyExc_TypeError), 0);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn given_exception_matches_is_subclass_aware() {
    init();
    let given = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            &raw mut molt_cpython_abi::abi_types::PyExc_ModuleNotFoundError,
            &raw mut molt_cpython_abi::abi_types::PyExc_ImportError,
        )
    };
    assert_eq!(given, 1, "ModuleNotFoundError IS an ImportError");
    let not = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            &raw mut molt_cpython_abi::abi_types::PyExc_ImportError,
            &raw mut molt_cpython_abi::abi_types::PyExc_ModuleNotFoundError,
        )
    };
    assert_eq!(not, 0, "the subclass relation is directional");
}

#[test]
fn bad_internal_call_is_system_error() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe { molt_cpython_abi::api::errors::PyErr_BadInternalCall() };
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                &raw mut molt_cpython_abi::abi_types::PyExc_SystemError,
            )
        },
        1,
        "PyErr_BadInternalCall sets SystemError (Python/errors.c), not RuntimeError"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn set_from_errno_reads_c_errno_shape() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_SetFromErrno(
            &raw mut molt_cpython_abi::abi_types::PyExc_OSError,
        )
    };
    assert!(result.is_null());
    let msg = molt_cpython_abi::api::errors::take_current_error_message().unwrap();
    assert!(
        msg.starts_with("[Errno "),
        "OSError message must carry CPython's '[Errno N] text' shape, got {msg:?}"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
