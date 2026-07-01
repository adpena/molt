//! CPython 3.12 stable ABI type definitions — `repr(C)` layout compatible
//! with real CPython extension `.so` files.
//!
//! These types deliberately mirror CPython's internal structs so that C code
//! compiled against CPython 3.12 headers can call our ABI functions and
//! receive correctly-structured pointers.
//!
//! All layouts validated against cpython/Include/object.h (CPython 3.12.x).

use std::ffi::c_void;
use std::os::raw::{c_char, c_double, c_int, c_uint, c_ulong};

pub type Py_ssize_t = isize;
pub type Py_hash_t = isize;
pub type Py_uhash_t = usize;
pub type PyCFunction = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
pub type PyCFunctionWithKeywords =
    unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;
pub type PyCFunctionFast =
    unsafe extern "C" fn(*mut PyObject, *mut *mut PyObject, Py_ssize_t) -> *mut PyObject;
pub type PyCFunctionFastWithKeywords = unsafe extern "C" fn(
    *mut PyObject,
    *mut *mut PyObject,
    Py_ssize_t,
    *mut PyObject,
) -> *mut PyObject;
pub type PyVectorcallFunc =
    unsafe extern "C" fn(*mut PyObject, *mut *mut PyObject, usize, *mut PyObject) -> *mut PyObject;
pub type PyCapsuleDestructor = unsafe extern "C" fn(*mut PyObject);
pub type PyDescrGetFunc =
    unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;
pub type PyDescrSetFunc = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int;

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

#[repr(C)]
pub struct PyMutex {
    pub _bits: usize,
}

unsafe impl Send for PyMutex {}
unsafe impl Sync for PyMutex {}

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
pub struct PyTupleObject {
    pub ob_base: PyVarObject,
    pub ob_item: *mut *mut PyObject,
}

unsafe impl Send for PyTupleObject {}
unsafe impl Sync for PyTupleObject {}

#[repr(C)]
pub struct PyLongValue {
    pub lv_tag: usize,
    pub ob_digit: [u32; 1],
}

#[repr(C)]
pub struct PyLongObject {
    pub ob_base: PyObject,
    pub long_value: PyLongValue,
}

unsafe impl Send for PyLongObject {}
unsafe impl Sync for PyLongObject {}

#[repr(C)]
pub struct PyBytesObject {
    pub ob_base: PyVarObject,
    pub ob_shash: Py_hash_t,
    pub ob_sval: [c_char; 1],
}

unsafe impl Send for PyBytesObject {}
unsafe impl Sync for PyBytesObject {}

#[repr(C)]
pub struct PyByteArrayObject {
    pub ob_base: PyVarObject,
    pub ob_alloc: Py_ssize_t,
    pub ob_bytes: *mut c_char,
    pub ob_start: *mut c_char,
    pub ob_exports: Py_ssize_t,
}

unsafe impl Send for PyByteArrayObject {}
unsafe impl Sync for PyByteArrayObject {}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Py_complex {
    pub real: c_double,
    pub imag: c_double,
}

#[repr(C)]
pub struct PyComplexObject {
    pub ob_base: PyObject,
    pub cval: Py_complex,
}

unsafe impl Send for PyComplexObject {}
unsafe impl Sync for PyComplexObject {}

#[repr(C)]
pub struct PyCapsuleObject {
    pub ob_base: PyObject,
    pub pointer: *mut c_void,
    pub name: *const c_char,
    pub context: *mut c_void,
    pub destructor: Option<PyCapsuleDestructor>,
}

unsafe impl Send for PyCapsuleObject {}
unsafe impl Sync for PyCapsuleObject {}

#[repr(C)]
pub struct PySliceObject {
    pub ob_base: PyObject,
    pub start: *mut PyObject,
    pub stop: *mut PyObject,
    pub step: *mut PyObject,
}

unsafe impl Send for PySliceObject {}
unsafe impl Sync for PySliceObject {}

#[repr(C)]
pub struct PyCodeObject {
    pub ob_base: PyObject,
    pub _co_firsttraceable: c_int,
}

#[repr(C)]
pub struct PyFrameObject {
    pub ob_base: PyObject,
    pub f_back: *mut PyFrameObject,
    pub f_code: *mut PyCodeObject,
    pub f_globals: *mut PyObject,
    pub f_locals: *mut PyObject,
    pub f_lineno: c_int,
}

unsafe impl Send for PyCodeObject {}
unsafe impl Sync for PyCodeObject {}
unsafe impl Send for PyFrameObject {}
unsafe impl Sync for PyFrameObject {}

#[repr(C)]
pub struct PyDateTime_Delta {
    pub ob_base: PyObject,
    pub hashcode: Py_hash_t,
    pub days: c_int,
    pub seconds: c_int,
    pub microseconds: c_int,
}

#[repr(C)]
pub struct PyDateTime_TZInfo {
    pub ob_base: PyObject,
}

#[repr(C)]
pub struct PyDateTime_Date {
    pub ob_base: PyObject,
    pub hashcode: Py_hash_t,
    pub hastzinfo: c_char,
    pub data: [u8; 4],
}

#[repr(C)]
pub struct PyDateTime_Time {
    pub ob_base: PyObject,
    pub hashcode: Py_hash_t,
    pub hastzinfo: c_char,
    pub data: [u8; 6],
    pub fold: u8,
    pub tzinfo: *mut PyObject,
}

#[repr(C)]
pub struct PyDateTime_DateTime {
    pub ob_base: PyObject,
    pub hashcode: Py_hash_t,
    pub hastzinfo: c_char,
    pub data: [u8; 10],
    pub fold: u8,
    pub tzinfo: *mut PyObject,
}

unsafe impl Send for PyDateTime_Delta {}
unsafe impl Sync for PyDateTime_Delta {}
unsafe impl Send for PyDateTime_TZInfo {}
unsafe impl Sync for PyDateTime_TZInfo {}
unsafe impl Send for PyDateTime_Date {}
unsafe impl Sync for PyDateTime_Date {}
unsafe impl Send for PyDateTime_Time {}
unsafe impl Sync for PyDateTime_Time {}
unsafe impl Send for PyDateTime_DateTime {}
unsafe impl Sync for PyDateTime_DateTime {}

#[repr(C)]
pub struct PyDictProxyObject {
    pub ob_base: PyObject,
    pub mapping: *mut PyObject,
}

#[repr(C)]
pub struct PyGenericAliasObject {
    pub ob_base: PyObject,
    pub origin: *mut PyObject,
    pub args: *mut PyObject,
}

unsafe impl Send for PyDictProxyObject {}
unsafe impl Sync for PyDictProxyObject {}
unsafe impl Send for PyGenericAliasObject {}
unsafe impl Sync for PyGenericAliasObject {}

#[repr(C)]
pub struct PyContextVarObject {
    pub ob_base: PyObject,
    pub name: *mut PyObject,
    pub default_value: *mut PyObject,
    pub current_value: *mut PyObject,
}

unsafe impl Send for PyContextVarObject {}
unsafe impl Sync for PyContextVarObject {}

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
    pub tp_descr_get: Option<PyDescrGetFunc>,
    pub tp_descr_set: Option<PyDescrSetFunc>,
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
    pub tp_watched: u8,
}

unsafe impl Send for PyTypeObject {}
unsafe impl Sync for PyTypeObject {}

/// Method descriptor — `tp_methods` array entry.
#[repr(C)]
pub struct PyMethodDef {
    pub ml_name: *const c_char,
    pub ml_meth: Option<PyCFunction>,
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
    pub m_slots: *mut PyModuleDef_Slot,
    pub m_traverse: *mut c_void,
    pub m_clear: *mut c_void,
    pub m_free: *mut c_void,
}

#[repr(C)]
pub struct PyModuleDef_Slot {
    pub slot: c_int,
    pub value: *mut c_void,
}

#[repr(C)]
pub struct PyModuleDef_Base {
    pub ob_base: PyObject,
    pub m_init: Option<unsafe extern "C" fn() -> *mut PyObject>,
    pub m_index: Py_ssize_t,
    pub m_copy: *mut PyObject,
}

#[repr(C)]
pub struct PyInterpreterState {
    pub _molt_reserved: c_int,
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct _PyErr_StackItem {
    pub exc_type: *mut PyObject,
    pub exc_value: *mut PyObject,
    pub exc_traceback: *mut PyObject,
    pub previous_item: *mut _PyErr_StackItem,
}

#[repr(C)]
pub struct PyThreadState {
    pub interp: *mut PyInterpreterState,
    pub current_exception: *mut PyObject,
    pub exc_info: *mut _PyErr_StackItem,
    pub exc_state: _PyErr_StackItem,
    pub _molt_reserved: c_int,
}

unsafe impl Send for PyInterpreterState {}
unsafe impl Sync for PyInterpreterState {}
unsafe impl Send for _PyErr_StackItem {}
unsafe impl Sync for _PyErr_StackItem {}
unsafe impl Send for PyThreadState {}
unsafe impl Sync for PyThreadState {}

#[repr(C)]
pub struct PyBaseExceptionObject {
    pub ob_base: PyObject,
    pub dict: *mut PyObject,
    pub args: *mut PyObject,
    pub notes: *mut PyObject,
    pub traceback: *mut PyObject,
    pub context: *mut PyObject,
    pub cause: *mut PyObject,
    pub suppress_context: c_char,
}

unsafe impl Send for PyBaseExceptionObject {}
unsafe impl Sync for PyBaseExceptionObject {}

#[repr(C)]
pub struct PyCFunctionObject {
    pub ob_base: PyObject,
    pub m_ml: *mut PyMethodDef,
    pub m_self: *mut PyObject,
    pub m_module: *mut PyObject,
    pub m_weakreflist: *mut PyObject,
    pub vectorcall: Option<PyVectorcallFunc>,
}

#[repr(C)]
pub struct PyCMethodObject {
    pub func: PyCFunctionObject,
    pub mm_class: *mut PyTypeObject,
}

#[repr(C)]
pub struct PyMethodObject {
    pub ob_base: PyObject,
    pub im_func: *mut PyObject,
    pub im_self: *mut PyObject,
}

unsafe impl Send for PyCFunctionObject {}
unsafe impl Sync for PyCFunctionObject {}
unsafe impl Send for PyCMethodObject {}
unsafe impl Sync for PyCMethodObject {}
unsafe impl Send for PyMethodObject {}
unsafe impl Sync for PyMethodObject {}

#[repr(C)]
pub struct PyType_Slot {
    pub slot: c_int,
    pub pfunc: *mut c_void,
}

#[repr(C)]
pub struct PyType_Spec {
    pub name: *const c_char,
    pub basicsize: c_int,
    pub itemsize: c_int,
    pub flags: c_uint,
    pub slots: *mut PyType_Slot,
}

unsafe impl Send for PyType_Slot {}
unsafe impl Sync for PyType_Slot {}
unsafe impl Send for PyType_Spec {}
unsafe impl Sync for PyType_Spec {}
unsafe impl Send for PyModuleDef_Slot {}
unsafe impl Sync for PyModuleDef_Slot {}

/// CPython METH flags (tp_methods ml_flags).
pub const METH_VARARGS: c_int = 0x0001;
pub const METH_KEYWORDS: c_int = 0x0002;
pub const METH_NOARGS: c_int = 0x0004;
pub const METH_O: c_int = 0x0008;
pub const METH_CLASS: c_int = 0x0010;
pub const METH_STATIC: c_int = 0x0020;
pub const METH_COEXIST: c_int = 0x0040;
pub const METH_FASTCALL: c_int = 0x0080;
pub const METH_METHOD: c_int = 0x0200;

/// PyType tp_flags bits.
pub const Py_TPFLAGS_BASETYPE: c_ulong = 1 << 10;
pub const Py_TPFLAGS_READY: c_ulong = 1 << 12;
pub const Py_TPFLAGS_READYING: c_ulong = 1 << 13;
pub const Py_TPFLAGS_HEAPTYPE: c_ulong = 1 << 9;
pub const Py_TPFLAGS_HAVE_GC: c_ulong = 1 << 14;
pub const Py_TPFLAGS_HAVE_VERSION_TAG: c_ulong = 1 << 18;
pub const Py_TPFLAGS_CHECKTYPES: c_ulong = 0;
pub const Py_TPFLAGS_HAVE_NEWBUFFER: c_ulong = 0;
pub const Py_TPFLAGS_IS_ABSTRACT: c_ulong = 1 << 20;
pub const Py_TPFLAGS_BASE_EXC_SUBCLASS: c_ulong = 1 << 30;
pub const Py_TPFLAGS_DEFAULT: c_ulong = Py_TPFLAGS_BASETYPE;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static Py_Version: c_ulong = 0x030c00f0;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_OptimizeFlag: c_int = 0;

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
#[unsafe(no_mangle)]
pub static mut Py_None: PyObject = PyObject {
    ob_refcnt: 1 << 30, // effectively immortal
    ob_type: std::ptr::null_mut(),
};

#[unsafe(no_mangle)]
pub static mut Py_True: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

#[unsafe(no_mangle)]
pub static mut Py_False: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

/// Sentinel returned by rich comparison when the operation is not supported.
/// Extensions compare against this pointer to decide whether to try the
/// reflected operation.  Must be distinct from Py_None.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_NotImplementedSentinel: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_EllipsisObject: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

// We can't use the macro with const-init for tp_name (C strings aren't const).
// Instead the names are patched in `init_static_types()`.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyLong_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyFloat_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyComplex_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyUnicode_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyBytes_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyByteArray_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyList_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyTuple_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDict_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDictProxy_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_GenericAliasType: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyContextVar_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PySet_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyBool_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyModule_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyCFunction_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyMethod_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyMethodDescr_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyMemberDescr_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyGetSetDescr_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyCapsule_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PySlice_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyMemoryView_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDateTime_DateType: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDateTime_DateTimeType: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDateTime_TimeType: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDateTime_DeltaType: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDateTime_TZInfoType: PyTypeObject = unsafe { std::mem::zeroed() };

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyDateTime_TimeZone_UTC_Object: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

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
        set_name!(PyComplex_Type, b"complex\0");
        set_name!(PyUnicode_Type, b"str\0");
        set_name!(PyBytes_Type, b"bytes\0");
        set_name!(PyByteArray_Type, b"bytearray\0");
        set_name!(PyList_Type, b"list\0");
        set_name!(PyTuple_Type, b"tuple\0");
        set_name!(PyDict_Type, b"dict\0");
        set_name!(PyDictProxy_Type, b"mappingproxy\0");
        set_name!(Py_GenericAliasType, b"types.GenericAlias\0");
        set_name!(PyContextVar_Type, b"_contextvars.ContextVar\0");
        set_name!(PySet_Type, b"set\0");
        set_name!(PyBool_Type, b"bool\0");
        set_name!(PyModule_Type, b"module\0");
        set_name!(PyCFunction_Type, b"builtin_function_or_method\0");
        set_name!(PyMethod_Type, b"method\0");
        set_name!(PyMethodDescr_Type, b"method_descriptor\0");
        set_name!(PyMemberDescr_Type, b"member_descriptor\0");
        set_name!(PyGetSetDescr_Type, b"getset_descriptor\0");
        set_name!(PyCapsule_Type, b"PyCapsule\0");
        set_name!(PySlice_Type, b"slice\0");
        set_name!(PyMemoryView_Type, b"memoryview\0");
        PyMemoryView_Type.tp_basicsize = std::mem::size_of::<PyMemoryViewObject>() as Py_ssize_t;
        set_name!(PyDateTime_DateType, b"datetime.date\0");
        set_name!(PyDateTime_DateTimeType, b"datetime.datetime\0");
        set_name!(PyDateTime_TimeType, b"datetime.time\0");
        set_name!(PyDateTime_DeltaType, b"datetime.timedelta\0");
        set_name!(PyDateTime_TZInfoType, b"datetime.tzinfo\0");

        PyTuple_Type.tp_dealloc = Some(crate::api::sequences::molt_tuple_dealloc);
        PyByteArray_Type.tp_dealloc = Some(crate::api::strings::molt_bytearray_dealloc);
        PyComplex_Type.tp_dealloc = Some(crate::api::numbers::molt_complex_dealloc);
        PyDictProxy_Type.tp_basicsize = std::mem::size_of::<PyDictProxyObject>() as Py_ssize_t;
        PyDictProxy_Type.tp_dealloc = Some(crate::api::mapping::molt_dictproxy_dealloc);
        Py_GenericAliasType.tp_basicsize =
            std::mem::size_of::<PyGenericAliasObject>() as Py_ssize_t;
        Py_GenericAliasType.tp_dealloc = Some(crate::api::object::molt_generic_alias_dealloc);
        PyContextVar_Type.tp_basicsize = std::mem::size_of::<PyContextVarObject>() as Py_ssize_t;
        PyContextVar_Type.tp_dealloc = Some(crate::api::contextvars::molt_contextvar_dealloc);
        PyCapsule_Type.tp_dealloc = Some(crate::api::capsule::molt_capsule_dealloc);
        PySlice_Type.tp_dealloc = Some(crate::api::slice::molt_slice_dealloc);
        PyMemoryView_Type.tp_dealloc = Some(crate::api::memory::molt_memoryview_dealloc);
        PyDateTime_DateType.tp_basicsize = std::mem::size_of::<PyDateTime_Date>() as Py_ssize_t;
        PyDateTime_DateTimeType.tp_basicsize =
            std::mem::size_of::<PyDateTime_DateTime>() as Py_ssize_t;
        PyDateTime_TimeType.tp_basicsize = std::mem::size_of::<PyDateTime_Time>() as Py_ssize_t;
        PyDateTime_DeltaType.tp_basicsize = std::mem::size_of::<PyDateTime_Delta>() as Py_ssize_t;
        PyDateTime_TZInfoType.tp_basicsize = std::mem::size_of::<PyDateTime_TZInfo>() as Py_ssize_t;
        PyDateTime_DateType.tp_dealloc = Some(crate::api::datetime::molt_datetime_dealloc);
        PyDateTime_DateTimeType.tp_dealloc = Some(crate::api::datetime::molt_datetime_dealloc);
        PyDateTime_TimeType.tp_dealloc = Some(crate::api::datetime::molt_datetime_dealloc);
        PyDateTime_DeltaType.tp_dealloc = Some(crate::api::datetime::molt_datetime_dealloc);
        PyCFunction_Type.tp_call = Some(crate::api::object::molt_cfunction_call);
        PyCFunction_Type.tp_dealloc = Some(crate::api::object::molt_cfunction_dealloc);
        PyMethod_Type.tp_call = Some(crate::api::object::molt_method_call);
        PyMethod_Type.tp_dealloc = Some(crate::api::object::molt_method_dealloc);

        set_name!(PyNone_Type, b"NoneType\0");
        set_name!(PyNotImplemented_Type, b"NotImplementedType\0");
        set_name!(PyType_Type, b"type\0");
        set_name!(PyBaseObject_Type, b"object\0");
        set_name!(PyFrozenSet_Type, b"frozenset\0");

        Py_None.ob_type = &raw mut PyNone_Type;
        Py_True.ob_type = &raw mut PyBool_Type;
        Py_False.ob_type = &raw mut PyBool_Type;
        Py_NotImplementedSentinel.ob_type = &raw mut PyNotImplemented_Type;
        Py_EllipsisObject.ob_type = &raw mut PyBaseObject_Type;
        PyDateTime_TimeZone_UTC_Object.ob_type = &raw mut PyDateTime_TZInfoType;
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
exc_singleton!(PyExc_ModuleNotFoundError);
exc_singleton!(PyExc_StopIteration);
exc_singleton!(PyExc_NotImplementedError);
exc_singleton!(PyExc_OSError);
exc_singleton!(PyExc_IOError);
exc_singleton!(PyExc_FileNotFoundError);
exc_singleton!(PyExc_PermissionError);
exc_singleton!(PyExc_FileExistsError);
exc_singleton!(PyExc_IsADirectoryError);
exc_singleton!(PyExc_NotADirectoryError);
exc_singleton!(PyExc_TimeoutError);
exc_singleton!(PyExc_ArithmeticError);
exc_singleton!(PyExc_FloatingPointError);
exc_singleton!(PyExc_LookupError);
exc_singleton!(PyExc_AssertionError);
exc_singleton!(PyExc_EOFError);
exc_singleton!(PyExc_NameError);
exc_singleton!(PyExc_UnboundLocalError);
exc_singleton!(PyExc_SyntaxError);
exc_singleton!(PyExc_SystemError);
exc_singleton!(PyExc_SystemExit);
exc_singleton!(PyExc_UnicodeError);
exc_singleton!(PyExc_UnicodeDecodeError);
exc_singleton!(PyExc_UnicodeEncodeError);
exc_singleton!(PyExc_BufferError);
exc_singleton!(PyExc_RecursionError);
exc_singleton!(PyExc_GeneratorExit);
exc_singleton!(PyExc_KeyboardInterrupt);
exc_singleton!(PyExc_ConnectionError);
exc_singleton!(PyExc_ConnectionResetError);
exc_singleton!(PyExc_BrokenPipeError);
exc_singleton!(PyExc_Warning);
exc_singleton!(PyExc_DeprecationWarning);
exc_singleton!(PyExc_RuntimeWarning);
exc_singleton!(PyExc_FutureWarning);
exc_singleton!(PyExc_ImportWarning);
exc_singleton!(PyExc_UserWarning);

/// Py_HASH_EXTERNAL constant — used by some extensions.
pub const Py_HASH_EXTERNAL: c_int = 0;
pub const PyBUF_WRITABLE: c_int = 0x0001;
pub const PyBUF_FORMAT: c_int = 0x0004;
pub const PyBUF_ND: c_int = 0x0008;
pub const PyBUF_STRIDES: c_int = 0x0010 | PyBUF_ND;
pub const PyBUF_INDIRECT: c_int = 0x0100 | PyBUF_STRIDES;
pub const PyBUF_RECORDS_RO: c_int = PyBUF_STRIDES | PyBUF_FORMAT;

#[allow(non_upper_case_globals)]
pub const Py_mp_subscript: c_int = 5;

/// Buffer protocol — minimal Py_buffer struct.
#[repr(C)]
pub struct Py_buffer {
    pub buf: *mut std::ffi::c_void,
    pub obj: *mut PyObject,
    pub len: Py_ssize_t,
    pub itemsize: Py_ssize_t,
    pub readonly: c_int,
    pub ndim: c_int,
    pub format: *mut std::os::raw::c_char,
    pub shape: *mut Py_ssize_t,
    pub strides: *mut Py_ssize_t,
    pub suboffsets: *mut Py_ssize_t,
    pub internal: *mut std::ffi::c_void,
}

#[repr(C)]
pub struct PyMemoryViewObject {
    pub ob_base: PyObject,
    pub view: Py_buffer,
    pub base: *mut PyObject,
}

unsafe impl Send for PyMemoryViewObject {}
unsafe impl Sync for PyMemoryViewObject {}

/// NoneType type object (for type(None) checks).
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyNone_Type: PyTypeObject = unsafe { std::mem::zeroed() };

/// NotImplemented type object.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyNotImplemented_Type: PyTypeObject = unsafe { std::mem::zeroed() };

/// Type type object.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyType_Type: PyTypeObject = unsafe { std::mem::zeroed() };

/// Base object type.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyBaseObject_Type: PyTypeObject = unsafe { std::mem::zeroed() };

/// FrozenSet type.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyFrozenSet_Type: PyTypeObject = unsafe { std::mem::zeroed() };
