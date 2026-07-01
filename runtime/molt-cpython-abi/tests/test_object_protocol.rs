//! Tests for object protocol: PyObject_Repr, Str, Hash, RichCompare,
//! TypeCheck, IsInstance, CallableCheck.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::Py_NotImplementedSentinel;
use molt_cpython_abi::abi_types::{
    METH_NOARGS, METH_O, Py_OptimizeFlag, PyMethodDef, PyMutex, PyObject,
};
use std::os::raw::c_char;
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyObject_Repr / PyObject_Str
// ---------------------------------------------------------------------------

#[test]
fn test_object_repr_returns_string() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let repr = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(py) };
    assert!(!repr.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(repr);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_object_repr_null_returns_null() {
    init();
    let repr = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(ptr::null_mut()) };
    assert!(repr.is_null());
}

#[test]
fn test_object_str_returns_string() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(py) };
    assert!(!s.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(s);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_memoryview_from_memory_has_type_and_null_base() {
    init();
    let mut byte = b'x' as c_char;
    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromMemory(&mut byte, 1, 0) };
    assert!(!view.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::memory::PyMemoryView_Check(view) },
        1
    );
    assert!(unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(view) }.is_null());
    let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
    assert!(!buffer.is_null());
    assert_eq!(unsafe { (*buffer).len }, 1);
    assert!(!unsafe { (*buffer).internal }.is_null());
    assert!(!unsafe { (*buffer).format }.is_null());
    assert!(!unsafe { (*buffer).shape }.is_null());
    assert!(!unsafe { (*buffer).strides }.is_null());
    unsafe {
        assert_eq!(*(*buffer).format as u8, b'B');
        assert_eq!(*(*buffer).shape, 1);
        assert_eq!(*(*buffer).strides, 1);
    }
    let same_view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromObject(view) };
    assert_eq!(same_view, view);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(same_view) };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
}

// ---------------------------------------------------------------------------
// PyObject_Hash
// ---------------------------------------------------------------------------

#[test]
fn test_object_hash_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let hash = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(py) };
    // Should return some non-zero value (pointer-based)
    assert_ne!(hash, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_object_length_hint_uses_default_for_unknown_object() {
    init();
    let hint = unsafe { molt_cpython_abi::api::object::PyObject_LengthHint(ptr::null_mut(), 17) };
    assert_eq!(hint, 17);
}

#[test]
fn test_object_self_iter_returns_new_reference_to_same_object() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let initial_refcnt = unsafe { (*py).ob_refcnt };
    let iter = unsafe { molt_cpython_abi::api::object::PyObject_SelfIter(py) };
    assert_eq!(iter, py);
    assert_eq!(unsafe { (*py).ob_refcnt }, initial_refcnt + 1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(iter);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_object_hash_different_objects_differ() {
    init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let ha = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(a) };
    let hb = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(b) };
    // Different pointers => different hashes (pointer-based hash)
    assert_ne!(ha, hb);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

// ---------------------------------------------------------------------------
// PyObject_TypeCheck
// ---------------------------------------------------------------------------

#[test]
fn test_object_typecheck_matching_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let tp = unsafe { (*py).ob_type };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_TypeCheck(py, tp) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_object_typecheck_mismatched_type() {
    init();
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let py_float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let float_tp = unsafe { (*py_float).ob_type };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_TypeCheck(py_int, float_tp) };
    assert_eq!(result, 0);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py_int);
        molt_cpython_abi::api::refcount::Py_DECREF(py_float);
    }
}

#[test]
fn test_object_typecheck_null_args() {
    init();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyObject_TypeCheck(ptr::null_mut(), ptr::null_mut())
        },
        0
    );
}

// ---------------------------------------------------------------------------
// PyObject_IsInstance
// ---------------------------------------------------------------------------

#[test]
fn test_isinstance_null_returns_zero() {
    init();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyObject_IsInstance(ptr::null_mut(), ptr::null_mut())
        },
        0
    );
}

// ---------------------------------------------------------------------------
// Py_TYPE
// ---------------------------------------------------------------------------

#[test]
fn test_py_type_returns_ob_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let tp = unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(py) };
    assert!(!tp.is_null());
    assert_eq!(tp, unsafe { (*py).ob_type });
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_py_type_null_returns_null() {
    init();
    let tp = unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(ptr::null_mut()) };
    assert!(tp.is_null());
}

#[test]
fn test_pyobject_type_returns_new_reference_to_ob_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let tp = unsafe { (*py).ob_type };
    let before = unsafe { (*tp).ob_base.ob_base.ob_refcnt };
    let type_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Type(py) };

    assert_eq!(type_obj, tp.cast::<PyObject>());
    assert_eq!(unsafe { (*tp).ob_base.ob_base.ob_refcnt }, before + 1);

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(type_obj);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_pyobject_type_null_sets_error_and_returns_null() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let type_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Type(ptr::null_mut()) };
    assert!(type_obj.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_pyeval_save_restore_thread_uses_singleton_thread_state() {
    init();
    let tstate = unsafe { molt_cpython_abi::api::object::PyEval_SaveThread() };
    assert!(!tstate.is_null());
    assert!(std::ptr::eq(tstate, unsafe {
        molt_cpython_abi::api::object::PyThreadState_Get()
    }));
    unsafe { molt_cpython_abi::api::object::PyEval_RestoreThread(tstate) };
}

#[test]
fn test_gil_check_mutex_and_unstable_unique_refs() {
    init();

    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyGILState_Check() },
        1
    );

    let mut mutex = PyMutex { _bits: 0 };
    unsafe { molt_cpython_abi::api::object::PyMutex_Lock(&mut mutex) };
    assert_eq!(mutex._bits, 1);
    unsafe { molt_cpython_abi::api::object::PyMutex_Unlock(&mut mutex) };
    assert_eq!(mutex._bits, 0);

    let mut obj = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(&mut obj) },
        1
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::object::PyUnstable_Object_IsUniqueReferencedTemporary(&mut obj)
        },
        1
    );
    obj.ob_refcnt = 2;
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(&mut obj) },
        0
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(ptr::null_mut())
        },
        0
    );

    assert_eq!(unsafe { Py_OptimizeFlag }, 0);
}

// ---------------------------------------------------------------------------
// PyCallable_Check
// ---------------------------------------------------------------------------

#[test]
fn test_callable_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_callable_check_on_int_returns_zero() {
    init();
    // Integers don't have tp_call
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

unsafe extern "C" fn return_none_noargs(
    _self_: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    if !args.is_null() {
        return ptr::null_mut();
    }
    let none = &raw mut molt_cpython_abi::abi_types::Py_None;
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(none) };
    none
}

unsafe extern "C" fn echo_single_arg(_self_: *mut PyObject, arg: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(arg) };
    arg
}

#[test]
fn test_cfunction_new_is_callable() {
    init();
    static NAME: &[u8] = b"f\0";
    let mut def = PyMethodDef {
        ml_name: NAME.as_ptr().cast(),
        ml_meth: Some(return_none_noargs),
        ml_flags: METH_NOARGS,
        ml_doc: ptr::null(),
    };
    let func =
        unsafe { molt_cpython_abi::api::object::PyCFunction_New(&raw mut def, ptr::null_mut()) };
    assert!(!func.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyCFunction_Check(func) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(func) },
        1
    );

    let result = unsafe { molt_cpython_abi::api::object::PyObject_CallNoArgs(func) };
    assert!(std::ptr::eq(
        result,
        &raw mut molt_cpython_abi::abi_types::Py_None
    ));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(result);
        molt_cpython_abi::api::refcount::Py_DECREF(func);
    }
}

#[test]
fn test_object_get_optional_attr_missing_clears_error() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(11) };
    let name = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"missing".as_ptr()) };
    let mut result: *mut PyObject = ptr::null_mut();
    let rc =
        unsafe { molt_cpython_abi::api::object::PyObject_GetOptionalAttr(py, name, &mut result) };
    assert_eq!(rc, 0);
    assert!(result.is_null());
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(name);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_method_new_binds_self_for_cfunction() {
    init();
    static NAME: &[u8] = b"echo\0";
    let mut def = PyMethodDef {
        ml_name: NAME.as_ptr().cast(),
        ml_meth: Some(echo_single_arg),
        ml_flags: METH_O,
        ml_doc: ptr::null(),
    };
    let func =
        unsafe { molt_cpython_abi::api::object::PyCFunction_New(&raw mut def, ptr::null_mut()) };
    let self_obj = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(77) };
    let method = unsafe { molt_cpython_abi::api::object::PyMethod_New(func, self_obj) };
    assert!(!method.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyMethod_Check(method) },
        1
    );
    assert!(std::ptr::eq(
        unsafe { molt_cpython_abi::api::object::PyMethod_GET_FUNCTION(method) },
        func
    ));
    assert!(std::ptr::eq(
        unsafe { molt_cpython_abi::api::object::PyMethod_GET_SELF(method) },
        self_obj
    ));

    let result = unsafe { molt_cpython_abi::api::object::PyObject_CallNoArgs(method) };
    assert!(std::ptr::eq(result, self_obj));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(result);
        molt_cpython_abi::api::refcount::Py_DECREF(method);
        molt_cpython_abi::api::refcount::Py_DECREF(self_obj);
        molt_cpython_abi::api::refcount::Py_DECREF(func);
    }
}

// ---------------------------------------------------------------------------
// PyObject_RichCompare / PyObject_RichCompareBool
// ---------------------------------------------------------------------------

const PY_LT: i32 = 0;
const PY_EQ: i32 = 2;
const PY_NE: i32 = 3;

#[test]
fn test_richcompare_same_object_eq() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    // Without tp_richcompare set, falls back to NotImplemented, then pointer identity
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(py, py, PY_EQ) };
    // Same pointer => EQ should be 1
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_richcompare_different_objects_ne() {
    init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(a, b, PY_NE) };
    // Different pointers => NE should be 1
    assert_eq!(result, 1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

#[test]
fn test_richcompare_returns_not_implemented_for_lt() {
    init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(a, b, PY_LT) };
    // Without tp_richcompare, returns NotImplemented sentinel
    assert!(std::ptr::eq(result, &raw mut Py_NotImplementedSentinel));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

#[test]
fn test_richcompare_null_is_safe() {
    init();
    // v=NULL should not crash
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyObject_RichCompare(
            ptr::null_mut(),
            ptr::null_mut(),
            PY_EQ,
        )
    };
    // Returns NotImplemented sentinel
    assert!(std::ptr::eq(result, &raw mut Py_NotImplementedSentinel));
}

#[test]
fn test_richcomparebool_null_returns_error() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(
            ptr::null_mut(),
            ptr::null_mut(),
            PY_LT,
        )
    };
    // LT on null => cannot compare => -1 (error)
    assert_eq!(result, -1);
}
