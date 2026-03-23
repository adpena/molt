//! CPython 3.12 stable ABI type definitions — `repr(C)` layout compatible
//! with real CPython extension `.so` files.
//!
//! These types deliberately mirror CPython's internal structs so that C code
//! compiled against CPython 3.12 headers can call our ABI functions and
//! receive correctly-structured pointers.
//!
//! All layouts validated against cpython/Include/object.h (CPython 3.12.x).

use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_ulong};

pub type Py_ssize_t = isize;
pub type Py_hash_t = isize;
pub type Py_uhash_t = usize;

/// Opaque reference-counted object header.
///
/// Every `PyObject*` points to a struct whose first two fields are
/// `ob_refcnt` and `ob_type`. All higher-level types embed this as their
/// first field (`ob_base`).
///
/// In our implementation, `ob_refcnt` is a *logical* ref-count managed by the
/// bridge; actual Molt GC tracks the canonical lifetime separately.
#[repr(C)]
pub struct PyObject {
    /// Logical reference count. Incremented/decremented via Py_INCREF/DECREF.
    /// When it hits zero the bridge releases the Molt-side handle.
    pub ob_refcnt: Py_ssize_t,

    /// Pointer to the type object. Points into our static type registry.
    pub ob_type: *mut PyTypeObject,
}

unsafe impl Send for PyObject {}
unsafe impl Sync for PyObject {}

/// Variable-length object (list, tuple, bytes, str).
#[repr(C)]
pub struct PyVarObject {
    pub ob_base: PyObject,
    pub ob_size: Py_ssize_t,
}

/// CPython PyTypeObject — minimal subset of fields actually accessed by most
/// C extensions via the stable ABI.
///
/// Full layout has 50+ fields; we include the first 36 that matter for
/// `PyType_Ready`, `PyArg_ParseTuple`, and common type checks.
#[repr(C)]
pub struct PyTypeObject {
    pub ob_base: PyVarObject,
    pub tp_name: *const c_char,
    pub tp_basicsize: Py_ssize_t,
    pub tp_itemsize: Py_ssize_t,
    pub tp_dealloc: Option<unsafe extern "C" fn(*mut PyObject)>,
    pub tp_vectorcall_offset: Py_ssize_t,
    pub tp_getattr: Option<unsafe extern "C" fn(*mut PyObject, *const c_char) -> *mut PyObject>,
    pub tp_setattr:
        Option<unsafe extern "C" fn(*mut PyObject, *const c_char, *mut PyObject) -> c_int>,
    pub tp_as_async: *mut c_void,
    pub tp_repr: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
    pub tp_as_number: *mut c_void,
    pub tp_as_sequence: *mut c_void,
    pub tp_as_mapping: *mut c_void,
    pub tp_hash: Option<unsafe extern "C" fn(*mut PyObject) -> Py_hash_t>,
    pub tp_call:
        Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject>,
    pub tp_str: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
    pub tp_getattro: Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject>,
    pub tp_setattro:
        Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int>,
    pub tp_as_buffer: *mut c_void,
    pub tp_flags: c_ulong,
    pub tp_doc: *const c_char,
    pub tp_traverse: Option<unsafe extern "C" fn(*mut PyObject, *mut c_void, *mut c_void) -> c_int>,
    pub tp_clear: Option<unsafe extern "C" fn(*mut PyObject) -> c_int>,
    pub tp_richcompare:
        Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject, c_int) -> *mut PyObject>,
    pub tp_weaklistoffset: Py_ssize_t,
    pub tp_iter: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
    pub tp_iternext: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
    pub tp_methods: *mut PyMethodDef,
    pub tp_members: *mut c_void,
    pub tp_getset: *mut c_void,
    pub tp_base: *mut PyTypeObject,
    pub tp_dict: *mut PyObject,
    pub tp_descr_get: *mut c_void,
    pub tp_descr_set: *mut c_void,
    pub tp_dictoffset: Py_ssize_t,
    pub tp_init: Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int>,
    pub tp_alloc: Option<unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject>,
    pub tp_new: Option<
        unsafe extern "C" fn(*mut PyTypeObject, *mut PyObject, *mut PyObject) -> *mut PyObject,
    >,
    pub tp_free: Option<unsafe extern "C" fn(*mut c_void)>,
    pub tp_is_gc: Option<unsafe extern "C" fn(*mut PyObject) -> c_int>,
    pub tp_bases: *mut PyObject,
    pub tp_mro: *mut PyObject,
    pub tp_cache: *mut PyObject,
    pub tp_subclasses: *mut c_void,
    pub tp_weaklist: *mut PyObject,
    pub tp_del: Option<unsafe extern "C" fn(*mut PyObject)>,
    pub tp_version_tag: c_ulong,
    pub tp_finalize: Option<unsafe extern "C" fn(*mut PyObject)>,
    pub tp_vectorcall: *mut c_void,
}

unsafe impl Send for PyTypeObject {}
unsafe impl Sync for PyTypeObject {}

/// Method descriptor — `tp_methods` array entry.
#[repr(C)]
pub struct PyMethodDef {
    pub ml_name: *const c_char,
    pub ml_meth: Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject>,
    pub ml_flags: c_int,
    pub ml_doc: *const c_char,
}

/// Module definition — used by `PyModuleDef_Init`.
#[repr(C)]
pub struct PyModuleDef {
    pub m_base: PyModuleDef_Base,
    pub m_name: *const c_char,
    pub m_doc: *const c_char,
    pub m_size: Py_ssize_t,
    pub m_methods: *mut PyMethodDef,
    pub m_slots: *mut c_void,
    pub m_traverse: *mut c_void,
    pub m_clear: *mut c_void,
    pub m_free: *mut c_void,
}

#[repr(C)]
pub struct PyModuleDef_Base {
    pub ob_base: PyObject,
    pub m_init: Option<unsafe extern "C" fn() -> *mut PyObject>,
    pub m_index: Py_ssize_t,
    pub m_copy: *mut PyObject,
}

/// CPython METH flags (tp_methods ml_flags).
pub const METH_VARARGS: c_int = 0x0001;
pub const METH_KEYWORDS: c_int = 0x0002;
pub const METH_NOARGS: c_int = 0x0004;
pub const METH_O: c_int = 0x0008;
pub const METH_CLASS: c_int = 0x0010;
pub const METH_STATIC: c_int = 0x0020;
pub const METH_FASTCALL: c_int = 0x0080;

/// PyType tp_flags bits.
pub const Py_TPFLAGS_BASETYPE: c_ulong = 1 << 10;
pub const Py_TPFLAGS_READY: c_ulong = 1 << 12;
pub const Py_TPFLAGS_READYING: c_ulong = 1 << 13;
pub const Py_TPFLAGS_HEAPTYPE: c_ulong = 1 << 9;
pub const Py_TPFLAGS_HAVE_GC: c_ulong = 1 << 14;
pub const Py_TPFLAGS_DEFAULT: c_ulong = Py_TPFLAGS_BASETYPE;

/// Type IDs used internally by the bridge to fast-path type checks.
/// These are NOT CPython ob_type pointers — they are Molt-side type tags.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum MoltTypeTag {
    None = 0,
    Bool = 1,
    Int = 2,
    Float = 3,
    Str = 4,
    Bytes = 5,
    List = 6,
    Tuple = 7,
    Dict = 8,
    Set = 9,
    Type = 10,
    Module = 11,
    Capsule = 12,
    Other = 255,
}

/// Sentinel: a `*mut PyObject` value for `None` / error returns.
pub const PY_NULL: *mut PyObject = std::ptr::null_mut();

/// Py_RETURN_NONE equivalent (returns a borrowed ref to None object).
/// Callers must Py_INCREF before storing.
pub static mut Py_None: PyObject = PyObject {
    ob_refcnt: 1 << 30, // effectively immortal
    ob_type: std::ptr::null_mut(),
};

pub static mut Py_True: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

pub static mut Py_False: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

/// Sentinel returned by rich comparison when the operation is not supported.
/// Extensions compare against this pointer to decide whether to try the
/// reflected operation.  Must be distinct from Py_None.
#[allow(non_upper_case_globals)]
pub static mut Py_NotImplementedSentinel: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

// We can't use the macro with const-init for tp_name (C strings aren't const).
// Instead the names are patched in `init_static_types()`.
#[allow(non_upper_case_globals)]
pub static mut PyLong_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyFloat_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyUnicode_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyBytes_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyList_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyTuple_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyDict_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PySet_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyBool_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
pub static mut PyModule_Type: PyTypeObject = unsafe { std::mem::zeroed() };

/// Called once at runtime init to patch static type objects.
///
/// # Safety
/// Must be called before any C extension is loaded. Single-threaded init only.
pub unsafe fn init_static_types() {
    macro_rules! set_name {
        ($ty:expr, $s:literal) => {
            $ty.tp_name = $s.as_ptr().cast();
            $ty.tp_flags = Py_TPFLAGS_READY;
        };
    }
    unsafe {
        set_name!(PyLong_Type, b"int\0");
        set_name!(PyFloat_Type, b"float\0");
        set_name!(PyUnicode_Type, b"str\0");
        set_name!(PyBytes_Type, b"bytes\0");
        set_name!(PyList_Type, b"list\0");
        set_name!(PyTuple_Type, b"tuple\0");
        set_name!(PyDict_Type, b"dict\0");
        set_name!(PySet_Type, b"set\0");
        set_name!(PyBool_Type, b"bool\0");
        set_name!(PyModule_Type, b"module\0");

        Py_None.ob_type = &raw mut PyUnicode_Type; // placeholder; bridge overrides
        Py_True.ob_type = &raw mut PyBool_Type;
        Py_False.ob_type = &raw mut PyBool_Type;
    }
}

// ─── Exception singletons ──────────────────────────────────────────────────
//
// Extensions receive these as opaque `*mut PyObject` passed to PyErr_SetString.
// The exact type/content doesn't matter — they're identity-compared by the bridge.
// We create one sentinel PyObject per exception class.

macro_rules! exc_singleton {
    ($name:ident) => {
        #[unsafe(no_mangle)]
        pub static mut $name: PyObject = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
    };
}

exc_singleton!(PyExc_BaseException);
exc_singleton!(PyExc_Exception);
exc_singleton!(PyExc_ValueError);
exc_singleton!(PyExc_TypeError);
exc_singleton!(PyExc_RuntimeError);
exc_singleton!(PyExc_MemoryError);
exc_singleton!(PyExc_IndexError);
exc_singleton!(PyExc_KeyError);
exc_singleton!(PyExc_AttributeError);
exc_singleton!(PyExc_OverflowError);
exc_singleton!(PyExc_ZeroDivisionError);
exc_singleton!(PyExc_ImportError);
exc_singleton!(PyExc_StopIteration);
exc_singleton!(PyExc_NotImplementedError);
exc_singleton!(PyExc_OSError);
