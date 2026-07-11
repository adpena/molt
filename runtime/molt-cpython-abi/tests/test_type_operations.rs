//! Tests for PyType_Ready, PyType_GenericAlloc, PyType_GenericNew,
//! type flag constants, and static type object initialisation.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use std::ffi::CStr;
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyType_Ready
// ---------------------------------------------------------------------------

#[test]
fn test_type_ready_null_returns_error() {
    init();
    let result = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(ptr::null_mut()) };
    assert_eq!(result, -1);
}

#[test]
fn test_type_ready_fails_closed_when_tp_dict_alloc_fails() {
    // PyType_Ready builds tp_dict via PyDict_New, which fails closed under the
    // stub hook table (alloc_dict returns 0 => NULL + MemoryError). PyType_Ready
    // then correctly returns -1 and does NOT set Py_TPFLAGS_READY rather than
    // marking a half-initialized type ready. (A real runtime supplies alloc_dict;
    // the fully-readied hierarchy is covered by test_type_ready_inheritance.rs
    // against a registered hook table.)
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_flags = 0;
    let result = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(&mut tp) };
    assert_eq!(result, -1, "PyType_Ready must fail closed when tp_dict cannot be allocated");
    assert_eq!(
        tp.tp_flags & Py_TPFLAGS_READY,
        0,
        "a failed PyType_Ready must not mark the type READY"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_type_ready_idempotent_when_already_ready() {
    // With Py_TPFLAGS_READY pre-set, PyType_Ready short-circuits to success
    // without touching tp_dict, so it is idempotent even under stubs.
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_flags = Py_TPFLAGS_READY;
    let flags_before = tp.tp_flags;
    let rc = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(&mut tp) };
    assert_eq!(rc, 0);
    assert_eq!(tp.tp_flags, flags_before);
}

// ---------------------------------------------------------------------------
// PyType_GenericAlloc
// ---------------------------------------------------------------------------

#[test]
fn test_generic_alloc_null_type_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::typeobj::PyType_GenericAlloc(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_generic_alloc_returns_object_with_refcount_one() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    let obj = unsafe { molt_cpython_abi::api::typeobj::PyType_GenericAlloc(&mut tp, 0) };
    assert!(!obj.is_null());
    assert_eq!(unsafe { (*obj).ob_refcnt }, 1);
    assert_eq!(unsafe { (*obj).ob_type }, &mut tp as *mut _);
    unsafe { molt_cpython_abi::api::memory::PyMem_Free(obj.cast()) };
}

#[test]
fn test_generic_alloc_initializes_var_object_size() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_basicsize = std::mem::size_of::<PyVarObject>() as Py_ssize_t;
    tp.tp_itemsize = std::mem::size_of::<*mut PyObject>() as Py_ssize_t;
    let obj = unsafe { molt_cpython_abi::api::typeobj::PyType_GenericAlloc(&mut tp, 3) };
    assert!(!obj.is_null());
    let var = obj.cast::<PyVarObject>();
    assert_eq!(unsafe { (*var).ob_base.ob_refcnt }, 1);
    assert_eq!(unsafe { (*var).ob_base.ob_type }, &mut tp as *mut _);
    assert_eq!(unsafe { (*var).ob_size }, 3);
    unsafe { molt_cpython_abi::api::memory::PyMem_Free(obj.cast()) };
}

// ---------------------------------------------------------------------------
// PyType_GenericNew
// ---------------------------------------------------------------------------

#[test]
fn test_generic_new_null_type_returns_null() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyType_GenericNew(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert!(result.is_null());
}

#[test]
fn test_generic_new_returns_valid_object() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    let obj = unsafe {
        molt_cpython_abi::api::typeobj::PyType_GenericNew(&mut tp, ptr::null_mut(), ptr::null_mut())
    };
    assert!(!obj.is_null());
    assert_eq!(unsafe { (*obj).ob_refcnt }, 1);
    unsafe { molt_cpython_abi::api::memory::PyMem_Free(obj.cast()) };
}

// ---------------------------------------------------------------------------
// Static type objects after init
// ---------------------------------------------------------------------------

#[test]
fn test_static_types_have_names() {
    init();
    unsafe {
        let name = CStr::from_ptr(PyLong_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "int");

        let name = CStr::from_ptr(PyFloat_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "float");

        let name = CStr::from_ptr(PyUnicode_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "str");

        let name = CStr::from_ptr(PyBytes_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "bytes");

        let name = CStr::from_ptr(PyList_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "list");

        let name = CStr::from_ptr(PyTuple_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "tuple");

        let name = CStr::from_ptr(PyDict_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "dict");

        let name = CStr::from_ptr(PySet_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "set");

        let name = CStr::from_ptr(PyBool_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "bool");

        let name = CStr::from_ptr(PyModule_Type.tp_name);
        assert_eq!(name.to_str().unwrap(), "module");
    }
}

#[test]
fn test_static_types_have_ready_flag() {
    init();
    unsafe {
        assert_ne!(PyLong_Type.tp_flags & Py_TPFLAGS_READY, 0);
        assert_ne!(PyFloat_Type.tp_flags & Py_TPFLAGS_READY, 0);
        assert_ne!(PyUnicode_Type.tp_flags & Py_TPFLAGS_READY, 0);
        assert_ne!(PyList_Type.tp_flags & Py_TPFLAGS_READY, 0);
        assert_ne!(PyTuple_Type.tp_flags & Py_TPFLAGS_READY, 0);
        assert_ne!(PyDict_Type.tp_flags & Py_TPFLAGS_READY, 0);
        assert_ne!(PyBool_Type.tp_flags & Py_TPFLAGS_READY, 0);
    }
}

// ---------------------------------------------------------------------------
// Type flag constants
// ---------------------------------------------------------------------------

#[test]
fn test_tpflags_constants() {
    assert_eq!(Py_TPFLAGS_BASETYPE, 1 << 10);
    assert_eq!(Py_TPFLAGS_READY, 1 << 12);
    assert_eq!(Py_TPFLAGS_READYING, 1 << 13);
    assert_eq!(Py_TPFLAGS_HEAPTYPE, 1 << 9);
    assert_eq!(Py_TPFLAGS_HAVE_GC, 1 << 14);
    // CPython v3.12.0 Include/object.h: DEFAULT is 0 on a standard build (was
    // wrongly pinned to BASETYPE here — a duplicate-authority drift; matrix #5).
    assert_eq!(Py_TPFLAGS_DEFAULT, 0);
    // Fast-subclass + protocol flag bit positions (verified against 3.12.0).
    assert_eq!(Py_TPFLAGS_MANAGED_WEAKREF, 1 << 3);
    assert_eq!(Py_TPFLAGS_MANAGED_DICT, 1 << 4);
    assert_eq!(Py_TPFLAGS_SEQUENCE, 1 << 5);
    assert_eq!(Py_TPFLAGS_MAPPING, 1 << 6);
    assert_eq!(Py_TPFLAGS_HAVE_VECTORCALL, 1 << 11);
    assert_eq!(Py_TPFLAGS_ITEMS_AT_END, 1 << 23);
    assert_eq!(Py_TPFLAGS_LONG_SUBCLASS, 1 << 24);
    assert_eq!(Py_TPFLAGS_LIST_SUBCLASS, 1 << 25);
    assert_eq!(Py_TPFLAGS_TUPLE_SUBCLASS, 1 << 26);
    assert_eq!(Py_TPFLAGS_BYTES_SUBCLASS, 1 << 27);
    assert_eq!(Py_TPFLAGS_UNICODE_SUBCLASS, 1 << 28);
    assert_eq!(Py_TPFLAGS_DICT_SUBCLASS, 1 << 29);
    assert_eq!(Py_TPFLAGS_BASE_EXC_SUBCLASS, 1 << 30);
    assert_eq!(Py_TPFLAGS_TYPE_SUBCLASS, 1 << 31);
}

// ---------------------------------------------------------------------------
// METH flag constants
// ---------------------------------------------------------------------------

#[test]
fn test_meth_flag_constants() {
    assert_eq!(METH_VARARGS, 0x0001);
    assert_eq!(METH_KEYWORDS, 0x0002);
    assert_eq!(METH_NOARGS, 0x0004);
    assert_eq!(METH_O, 0x0008);
    assert_eq!(METH_CLASS, 0x0010);
    assert_eq!(METH_STATIC, 0x0020);
    assert_eq!(METH_FASTCALL, 0x0080);
}

// ---------------------------------------------------------------------------
// MoltTypeTag
// ---------------------------------------------------------------------------

#[test]
fn test_type_tag_discriminants() {
    assert_eq!(MoltTypeTag::None as u8, 0);
    assert_eq!(MoltTypeTag::Bool as u8, 1);
    assert_eq!(MoltTypeTag::Int as u8, 2);
    assert_eq!(MoltTypeTag::Float as u8, 3);
    assert_eq!(MoltTypeTag::Str as u8, 4);
    assert_eq!(MoltTypeTag::Bytes as u8, 5);
    assert_eq!(MoltTypeTag::List as u8, 6);
    assert_eq!(MoltTypeTag::Tuple as u8, 7);
    assert_eq!(MoltTypeTag::Dict as u8, 8);
    assert_eq!(MoltTypeTag::Set as u8, 9);
    assert_eq!(MoltTypeTag::Type as u8, 10);
    assert_eq!(MoltTypeTag::Module as u8, 11);
    assert_eq!(MoltTypeTag::Capsule as u8, 12);
    assert_eq!(MoltTypeTag::Other as u8, 255);
}

// ---------------------------------------------------------------------------
// Singleton ob_type pointers after init
// ---------------------------------------------------------------------------

#[test]
fn test_py_true_has_bool_type() {
    init();
    unsafe {
        assert!(std::ptr::eq(Py_True.ob_base.ob_type, &raw mut PyBool_Type));
    }
}

#[test]
fn test_py_false_has_bool_type() {
    init();
    unsafe {
        assert!(std::ptr::eq(Py_False.ob_base.ob_type, &raw mut PyBool_Type));
    }
}

// ---------------------------------------------------------------------------
// Bridge-allocated int has PyLong_Type
// ---------------------------------------------------------------------------

#[test]
fn test_int_ob_type_is_pylong_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    assert!(!py.is_null());
    let tp = unsafe { (*py).ob_type };
    assert!(std::ptr::eq(tp, &raw mut PyLong_Type));
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_float_ob_type_is_pyfloat_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) };
    assert!(!py.is_null());
    let tp = unsafe { (*py).ob_type };
    assert!(std::ptr::eq(tp, &raw mut PyFloat_Type));
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}
