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
pub type PyDescrSetFunc =
    unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int;

/// `PyGetSetDef` getter: `PyObject *(*getter)(PyObject *self, void *closure)`.
/// Mirrors CPython `Include/descrobject.h`.
pub type getter = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> *mut PyObject;
/// `PyGetSetDef` setter: `int (*setter)(PyObject *self, PyObject *value, void *closure)`.
pub type setter = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut c_void) -> c_int;

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

/// `tp_getset` array entry — computed attribute descriptor definition.
/// Layout is byte-identical to CPython 3.12 `PyGetSetDef`
/// (`Include/descrobject.h`): `{name, get, set, doc, closure}`. Static C
/// extensions (every numpy type with a `tp_getset` table) declare these
/// statically and rely on `PyType_Ready` to turn each into a `getset_descriptor`
/// stored in `tp_dict`.
#[repr(C)]
pub struct PyGetSetDef {
    pub name: *const c_char,
    pub get: Option<getter>,
    pub set: Option<setter>,
    pub doc: *const c_char,
    pub closure: *mut c_void,
}

unsafe impl Send for PyGetSetDef {}
unsafe impl Sync for PyGetSetDef {}

/// `tp_members` array entry — struct-member attribute descriptor definition.
/// Layout is byte-identical to CPython 3.12 `PyMemberDef`
/// (`Include/descrobject.h`): `{name, type, offset, flags, doc}`.
#[repr(C)]
pub struct PyMemberDef {
    pub name: *const c_char,
    pub type_: c_int,
    pub offset: Py_ssize_t,
    pub flags: c_int,
    pub doc: *const c_char,
}

unsafe impl Send for PyMemberDef {}
unsafe impl Sync for PyMemberDef {}

/// Common descriptor header — CPython `PyDescrObject` (`PyDescr_COMMON`).
/// Every descriptor (getset, member, method) embeds this as its first field so
/// `d_type`/`d_name` are readable through a common `PyDescrObject*` view.
#[repr(C)]
pub struct PyDescrObject {
    pub ob_base: PyObject,
    /// Type the descriptor belongs to (owned reference).
    pub d_type: *mut PyTypeObject,
    /// Attribute name as an interned `str` object (owned reference).
    pub d_name: *mut PyObject,
    /// Qualified name (unused by the subset numpy needs; kept for layout parity).
    pub d_qualname: *mut PyObject,
}

/// `getset_descriptor` object — CPython `PyGetSetDescrObject`. Holds a borrowed
/// pointer to the caller's static `PyGetSetDef` (which must outlive the type).
#[repr(C)]
pub struct PyGetSetDescrObject {
    pub d_common: PyDescrObject,
    pub d_getset: *mut PyGetSetDef,
}

unsafe impl Send for PyGetSetDescrObject {}
unsafe impl Sync for PyGetSetDescrObject {}

/// `member_descriptor` object — CPython `PyMemberDescrObject`. Holds a borrowed
/// pointer to the caller's static `PyMemberDef`.
#[repr(C)]
pub struct PyMemberDescrObject {
    pub d_common: PyDescrObject,
    pub d_member: *mut PyMemberDef,
}

unsafe impl Send for PyMemberDescrObject {}
unsafe impl Sync for PyMemberDescrObject {}

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
pub struct Py_tss_t {
    pub _is_initialized: c_int,
    pub _key: usize,
}

#[repr(C)]
pub struct PyASCIIObject {
    pub ob_base: PyObject,
    pub length: Py_ssize_t,
    pub hash: Py_hash_t,
    pub state: c_uint,
    pub wstr: *mut u32,
}

#[repr(C)]
pub struct PyCompactUnicodeObject {
    pub base: PyASCIIObject,
    pub utf8_length: Py_ssize_t,
    pub utf8: *mut c_char,
    pub wstr_length: Py_ssize_t,
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

// ── Number / Sequence / Mapping / Async / Buffer protocol tables ──────────────
//
// `PyTypeObject` stores `tp_as_number`/`tp_as_sequence`/`tp_as_mapping`/
// `tp_as_async`/`tp_as_buffer` as opaque `*mut c_void` (nothing on the Rust side
// dereferences them by field — the typed view lives in the C header). To let
// `PyType_FromSpecWithBases` place `Py_nb_*`/`Py_sq_*`/`Py_mp_*`/`Py_am_*`/
// `Py_bf_*` slot pointers into the correct field, these structs mirror the exact
// field order of CPython 3.12's `PyNumberMethods`/`PySequenceMethods`/
// `PyMappingMethods`/`PyAsyncMethods`/`PyBufferProcs` (and this crate's own
// `include/Python.h`). Every member is a function pointer or `void*`, so all
// fields are pointer-sized and the layout is byte-identical to the header; we
// type them as `*mut c_void` because we only *store* the slot pointer here, never
// call through it. Field order is load-bearing — a reordering is a silent
// miscompile — so it is kept 1:1 with the header and CPython `Include/cpython/
// object.h`.
#[repr(C)]
pub struct PyNumberMethods {
    pub nb_add: *mut c_void,
    pub nb_subtract: *mut c_void,
    pub nb_multiply: *mut c_void,
    pub nb_remainder: *mut c_void,
    pub nb_divmod: *mut c_void,
    pub nb_power: *mut c_void,
    pub nb_negative: *mut c_void,
    pub nb_positive: *mut c_void,
    pub nb_absolute: *mut c_void,
    pub nb_bool: *mut c_void,
    pub nb_invert: *mut c_void,
    pub nb_lshift: *mut c_void,
    pub nb_rshift: *mut c_void,
    pub nb_and: *mut c_void,
    pub nb_xor: *mut c_void,
    pub nb_or: *mut c_void,
    pub nb_int: *mut c_void,
    pub nb_reserved: *mut c_void,
    pub nb_float: *mut c_void,
    pub nb_inplace_add: *mut c_void,
    pub nb_inplace_subtract: *mut c_void,
    pub nb_inplace_multiply: *mut c_void,
    pub nb_inplace_remainder: *mut c_void,
    pub nb_inplace_power: *mut c_void,
    pub nb_inplace_lshift: *mut c_void,
    pub nb_inplace_rshift: *mut c_void,
    pub nb_inplace_and: *mut c_void,
    pub nb_inplace_xor: *mut c_void,
    pub nb_inplace_or: *mut c_void,
    pub nb_floor_divide: *mut c_void,
    pub nb_true_divide: *mut c_void,
    pub nb_inplace_floor_divide: *mut c_void,
    pub nb_inplace_true_divide: *mut c_void,
    pub nb_index: *mut c_void,
    pub nb_matrix_multiply: *mut c_void,
    pub nb_inplace_matrix_multiply: *mut c_void,
}

#[repr(C)]
pub struct PySequenceMethods {
    pub sq_length: *mut c_void,
    pub sq_concat: *mut c_void,
    pub sq_repeat: *mut c_void,
    pub sq_item: *mut c_void,
    pub was_sq_slice: *mut c_void,
    pub sq_ass_item: *mut c_void,
    pub was_sq_ass_slice: *mut c_void,
    pub sq_contains: *mut c_void,
    pub sq_inplace_concat: *mut c_void,
    pub sq_inplace_repeat: *mut c_void,
}

#[repr(C)]
pub struct PyMappingMethods {
    pub mp_length: *mut c_void,
    pub mp_subscript: *mut c_void,
    pub mp_ass_subscript: *mut c_void,
}

#[repr(C)]
pub struct PyAsyncMethods {
    pub am_await: *mut c_void,
    pub am_aiter: *mut c_void,
    pub am_anext: *mut c_void,
    pub am_send: *mut c_void,
}

#[repr(C)]
pub struct PyBufferProcs {
    pub bf_getbuffer: *mut c_void,
    pub bf_releasebuffer: *mut c_void,
}

/// `struct _specialization_cache` (CPython v3.12.0 Include/cpython/object.h),
/// embedded by value at the tail of `PyHeapTypeObject`.
#[repr(C)]
pub struct SpecializationCache {
    pub getitem: *mut PyObject,
    pub getitem_version: u32,
}

/// `PyHeapTypeObject` / `struct _heaptypeobject` — the real allocation shape of a
/// heap type (a type created by `PyType_FromSpec*`). Field order is byte-for-byte
/// CPython v3.12.0 (Include/cpython/object.h) and matches
/// `include/Python.h`'s `_heaptypeobject`: the `PyTypeObject` header, the five
/// inline protocol sub-tables, then `ht_name`/`ht_slots`/`ht_qualname`/
/// `ht_cached_keys`/`ht_module`/`_ht_tpname`/`_spec_cache`. A `PyType_FromSpec`
/// type MUST be allocated as this (not a bare `Box<PyTypeObject>`) or an
/// extension's `((PyHeapTypeObject*)type)->ht_name`/`ht_module` reads run OOB past
/// the 416-byte `PyTypeObject` (matrix PyTypeObject #3, L3). The five sub-table
/// fields are present for layout fidelity (so `ht_*` land at the CPython offsets);
/// the runtime still points `tp_as_*` at the separately-boxed `ensure_*` tables.
#[repr(C)]
pub struct PyHeapTypeObject {
    pub ht_type: PyTypeObject,
    pub as_async: PyAsyncMethods,
    pub as_number: PyNumberMethods,
    pub as_mapping: PyMappingMethods,
    pub as_sequence: PySequenceMethods,
    pub as_buffer: PyBufferProcs,
    pub ht_name: *mut PyObject,
    pub ht_slots: *mut PyObject,
    pub ht_qualname: *mut PyObject,
    /// `struct _dictkeysobject *` — opaque to the ABI.
    pub ht_cached_keys: *mut c_void,
    pub ht_module: *mut PyObject,
    pub _ht_tpname: *mut c_char,
    pub _spec_cache: SpecializationCache,
}

unsafe impl Send for PyHeapTypeObject {}
unsafe impl Sync for PyHeapTypeObject {}

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
// ── Full tp_flags surface (values verified against CPython v3.12.0
// Include/object.h). The `*_SUBCLASS` fast-check bits + the protocol/behaviour
// flags were previously undefined on the Rust side, so the builtin static type
// shells could not carry them and `PyType_FastSubclass`/`PyType_HasFeature`
// answered wrong (matrix PyTypeObject #1/#2, L3).
pub const Py_TPFLAGS_MANAGED_WEAKREF: c_ulong = 1 << 3;
pub const Py_TPFLAGS_MANAGED_DICT: c_ulong = 1 << 4;
pub const Py_TPFLAGS_SEQUENCE: c_ulong = 1 << 5;
pub const Py_TPFLAGS_MAPPING: c_ulong = 1 << 6;
pub const Py_TPFLAGS_DISALLOW_INSTANTIATION: c_ulong = 1 << 7;
pub const Py_TPFLAGS_IMMUTABLETYPE: c_ulong = 1 << 8;
pub const Py_TPFLAGS_HAVE_VECTORCALL: c_ulong = 1 << 11;
pub const Py_TPFLAGS_METHOD_DESCRIPTOR: c_ulong = 1 << 17;
pub const Py_TPFLAGS_VALID_VERSION_TAG: c_ulong = 1 << 19;
/// Undocumented CPython-internal flag (Include/object.h @ v3.12.0): the type
/// matches itself as a `match`-statement class pattern. Set on the self-matching
/// builtin leaf types (int/float/str/bytes/bytearray/list/tuple/dict/set).
pub const _Py_TPFLAGS_MATCH_SELF: c_ulong = 1 << 22;
pub const Py_TPFLAGS_ITEMS_AT_END: c_ulong = 1 << 23;
pub const Py_TPFLAGS_LONG_SUBCLASS: c_ulong = 1 << 24;
pub const Py_TPFLAGS_LIST_SUBCLASS: c_ulong = 1 << 25;
pub const Py_TPFLAGS_TUPLE_SUBCLASS: c_ulong = 1 << 26;
pub const Py_TPFLAGS_BYTES_SUBCLASS: c_ulong = 1 << 27;
pub const Py_TPFLAGS_UNICODE_SUBCLASS: c_ulong = 1 << 28;
pub const Py_TPFLAGS_DICT_SUBCLASS: c_ulong = 1 << 29;
pub const Py_TPFLAGS_TYPE_SUBCLASS: c_ulong = 1 << 31;
/// `Py_TPFLAGS_DEFAULT` is **0** on a standard (non-STACKLESS) CPython v3.12.0
/// build (`Include/object.h`: `Py_TPFLAGS_HAVE_STACKLESS_EXTENSION | 0`, and the
/// stackless bit is 0 off-Stackless). The prior `= Py_TPFLAGS_BASETYPE` was a
/// duplicate-authority drift vs the C header's correct `Py_TPFLAGS_DEFAULT (0)`
/// (matrix PyTypeObject #5); a flag computation seeded from it would silently
/// mark every type BASETYPE.
pub const Py_TPFLAGS_DEFAULT: c_ulong = 0;

/// The `(major<<24)|(minor<<16)|(micro<<8)|level` hex Python version — the ONE
/// version authority the ABI is pinned to. The `Py_Version` data symbol below
/// re-exports it and the immortal-refcount authority derives from it, so there
/// is a single source of truth for "which CPython are we".
pub const PY_VERSION_HEX: c_ulong = 0x030c00f0;

/// Target CPython minor version, extracted from [`PY_VERSION_HEX`] (12 for 3.12).
// `as u32` is required where `c_ulong` is 64-bit (Linux/macOS LP64) and a no-op
// where it is 32-bit (Windows LLP64) — the lint fires only on the latter.
#[allow(clippy::unnecessary_cast)]
pub const TARGET_PY_MINOR: u32 = ((PY_VERSION_HEX >> 16) & 0xff) as u32;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static Py_Version: c_ulong = PY_VERSION_HEX;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_OptimizeFlag: c_int = 0;

// ─── Immortal reference-count authority ────────────────────────────────────
//
// THE single source of truth for the immortal `ob_refcnt` encoding. Every
// static singleton — `Py_None`/`Py_True`/`Py_False`, the `PyExc_*` singletons,
// the builtin `Py*_Type` statics, the canonical `_Py_*Struct` data symbols, the
// `NotImplemented`/`Ellipsis`/UTC sentinels — MUST initialise `ob_refcnt` to
// [`IMMORTAL_REFCNT`], and refcount immortality is decided ONLY by
// [`is_immortal_refcnt`]. This collapses the four historical encodings the
// binary-contract matrix flagged (`1`, `0` (zeroed), `1 << 30`, and the C
// header macro) to ONE; the anti-duplication invariant is machine-checked by
// `all_static_singletons_share_the_one_immortal_encoding` /
// `no_raw_immortal_literal_outside_the_authority` below.
//
// Value + detection mirror CPython's `_Py_IMMORTAL_REFCNT` / `_Py_IsImmortal`,
// verified against the primary source (python/cpython Include/object.h in
// v3.12.0/v3.13.0, Include/refcount.h in v3.14.0):
//   3.12/3.13  64-bit: UINT_MAX (0xFFFF_FFFF); 32-bit: UINT_MAX>>2 (0x3FFF_FFFF)
//   3.14       64-bit: _Py_IMMORTAL_INITIAL_REFCNT = 3<<30; 32-bit: 5<<28
//   _Py_IsImmortal 64-bit: (int32)ob_refcnt < 0; 32-bit: == the sentinel (3.12/
//   3.13) / >= the sentinel (3.14). Both 3.12 and 3.14 64-bit values keep bit 31
//   of the low word set, so the 64-bit predicate is version-independent.
// Because the value derives from [`TARGET_PY_MINOR`] (i.e. the single
// [`PY_VERSION_HEX`] knob), the constant is version-gated honest-early (M02)
// without introducing a second version authority.

/// CPython's immortal `ob_refcnt` initial value for a target minor version and
/// pointer width (in bytes). `const fn` so it can drive `static` initialisers.
const fn immortal_refcnt_for(py_minor: u32, ptr_bytes: usize) -> Py_ssize_t {
    if ptr_bytes > 4 {
        // 64-bit (LP64/LLP64). `_Py_IsImmortal` tests `(int32)ob_refcnt < 0`, so
        // any value whose low-word bit 31 is set reads immortal.
        if py_minor >= 14 {
            (3_u64 << 30) as Py_ssize_t // _Py_IMMORTAL_INITIAL_REFCNT (3.14)
        } else {
            u32::MAX as Py_ssize_t // _Py_IMMORTAL_REFCNT == UINT_MAX (3.12/3.13)
        }
    } else if py_minor >= 14 {
        (5_u32 << 28) as Py_ssize_t // _Py_IMMORTAL_INITIAL_REFCNT 32-bit (3.14)
    } else {
        (u32::MAX >> 2) as Py_ssize_t // UINT_MAX>>2 == 0x3FFF_FFFF (3.12/3.13)
    }
}

/// THE immortal `ob_refcnt` value, for the pinned target version and this
/// target's pointer width. Matches the C header's `_Py_IMMORTAL_REFCNT`.
pub const IMMORTAL_REFCNT: Py_ssize_t =
    immortal_refcnt_for(TARGET_PY_MINOR, core::mem::size_of::<*const ()>());

// Compile-time invariant, enforced on BOTH the native and wasm32 builds (the
// host-only runtime tests never execute the 32-bit path): the immortal value is
// a positive refcount and, on 64-bit, carries low-word bit 31 so CPython's
// `_Py_IsImmortal` ((int32)ob_refcnt < 0) classifies it immortal.
const _: () = {
    assert!(IMMORTAL_REFCNT > 0, "immortal refcount must be a positive ob_refcnt");
    #[cfg(target_pointer_width = "64")]
    assert!(
        IMMORTAL_REFCNT & 0x8000_0000 != 0,
        "64-bit immortal value must set low-word bit 31 (_Py_IsImmortal)"
    );
};

/// Immortality predicate — the sole immortal test, mirroring CPython
/// `_Py_IsImmortal` for the target pointer width. `Py_INCREF`/`Py_DECREF` route
/// through this instead of an ad-hoc threshold.
#[inline]
pub fn is_immortal_refcnt(rc: Py_ssize_t) -> bool {
    #[cfg(target_pointer_width = "64")]
    {
        // (int32)rc < 0  ⇔  low-word bit 31 set. Mask form is cast-truncation-free.
        rc & 0x8000_0000 != 0
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        if TARGET_PY_MINOR >= 14 {
            rc >= IMMORTAL_REFCNT
        } else {
            rc == IMMORTAL_REFCNT
        }
    }
}

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
    ob_refcnt: IMMORTAL_REFCNT, // effectively immortal
    ob_type: std::ptr::null_mut(),
};

/// `Py_True` — the live molt-header singleton (`include/Python.h`:
/// `#define Py_True (&Py_True)`), the symbol every witness extension compiled
/// against molt's own header resolves. It is a value-carrying `PyLongObject`
/// (CPython v3.12.0 `Objects/boolobject.c`:
/// `struct _longobject _Py_TrueStruct = { PyObject_HEAD_INIT(&PyBool_Type)
/// { .lv_tag = _PyLong_TRUE_TAG, { 1 } } }`), so an extension's inlined
/// `((PyLongObject*)Py_True)->long_value.ob_digit[0]` reads `1` IN BOUNDS instead
/// of reading OOB past a bare `PyObject` into adjacent static memory (matrix L1
/// #5 / binary-contract data-symbol #5, the `LAYOUT_MISMATCH` corruption vector).
/// `_PyLong_TRUE_TAG = TAG_FROM_SIGN_AND_SIZE(1,1) = (1-1)|(1<<3) = 8`
/// (Include/internal/pycore_long.h). `ob_type = &PyBool_Type` is set in the
/// const initialiser (was patched at runtime); immortal via the ONE
/// [`IMMORTAL_REFCNT`] authority. Reconciled to the same canonical `True` handle
/// as `_Py_TrueStruct` through `bridge::pyobj_to_handle_static`.
#[unsafe(no_mangle)]
pub static mut Py_True: PyLongObject = PyLongObject {
    ob_base: PyObject {
        ob_refcnt: IMMORTAL_REFCNT,
        ob_type: &raw mut PyBool_Type,
    },
    long_value: PyLongValue { lv_tag: 8, ob_digit: [1] },
};

/// `Py_False` — value-carrying `PyLongObject` twin of [`Py_True`] (CPython
/// v3.12.0 boolobject.c: `_PyLong_FALSE_TAG = TAG_FROM_SIGN_AND_SIZE(0,0) =
/// (1-0)|(0<<3) = 1`, `ob_digit[0] = 0`), so `((PyLongObject*)Py_False)->
/// long_value.ob_digit[0]` reads `0` IN BOUNDS.
#[unsafe(no_mangle)]
pub static mut Py_False: PyLongObject = PyLongObject {
    ob_base: PyObject {
        ob_refcnt: IMMORTAL_REFCNT,
        ob_type: &raw mut PyBool_Type,
    },
    long_value: PyLongValue { lv_tag: 1, ob_digit: [0] },
};

/// Sentinel returned by rich comparison when the operation is not supported.
/// Extensions compare against this pointer to decide whether to try the
/// reflected operation.  Must be distinct from Py_None.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_NotImplementedSentinel: PyObject = PyObject {
    ob_refcnt: IMMORTAL_REFCNT,
    ob_type: std::ptr::null_mut(),
};

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Py_EllipsisObject: PyObject = PyObject {
    ob_refcnt: IMMORTAL_REFCNT,
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
pub static mut PyModuleDef_Type: PyTypeObject = unsafe { std::mem::zeroed() };
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut PyCFunction_Type: PyTypeObject = unsafe { std::mem::zeroed() };
pub static mut PyCMethod_Type: PyTypeObject = unsafe { std::mem::zeroed() };
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
    ob_refcnt: IMMORTAL_REFCNT,
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
            // Builtin type statics are `std::mem::zeroed()` (ob_refcnt == 0 ==
            // MORTAL). Immortal-init them through the single authority so a net
            // over-DECREF cannot `tp_dealloc` a statically-allocated type.
            $ty.ob_base.ob_base.ob_refcnt = IMMORTAL_REFCNT;
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
        set_name!(PyModuleDef_Type, b"moduledef\0");
        set_name!(PyCFunction_Type, b"builtin_function_or_method\0");
        set_name!(PyCMethod_Type, b"builtin_method\0");
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
        // Structural (element-wise) comparison. Without this slot two distinct
        // tuple objects with equal contents compare unequal by object identity,
        // which breaks numpy ufunc dispatch (get_info_no_cast) — see
        // `molt_tuple_richcompare`.
        PyTuple_Type.tp_richcompare = Some(crate::api::sequences::molt_tuple_richcompare);
        // Sibling structural-comparison slots (close the same zeroed-shell class on
        // the container types numpy/scipy touch): list is element-wise like tuple;
        // dict is EQ/NE key-value equality. Without them two distinct-but-equal
        // list/dict objects compare unequal by object identity in do_richcompare.
        PyList_Type.tp_richcompare = Some(crate::api::sequences::molt_list_richcompare);
        PyDict_Type.tp_richcompare = Some(crate::api::mapping::molt_dict_richcompare);
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
        PyCMethod_Type.tp_call = Some(crate::api::object::molt_cfunction_call);
        PyCMethod_Type.tp_dealloc = Some(crate::api::object::molt_cfunction_dealloc);
        PyMethod_Type.tp_call = Some(crate::api::object::molt_method_call);
        PyMethod_Type.tp_dealloc = Some(crate::api::object::molt_method_dealloc);
        // CPython's `PyType_Type.tp_call = type_call` — calling a type object
        // (class instantiation from C) drives `tp_new`/`tp_init`. A C-extension
        // metatype (numpy's `PyArrayDTypeMeta_Type` sets `tp_base = &PyType_Type`
        // at import) inherits this via PyType_Ready slot inheritance; without it
        // every `SomeDTypeClass()` call fails "'numpy._DTypeMeta' object is not
        // callable" during `_multiarray_umath` init.
        PyType_Type.tp_call = Some(crate::api::typeobj::molt_type_call);

        set_name!(PyNone_Type, b"NoneType\0");
        set_name!(PyNotImplemented_Type, b"NotImplementedType\0");
        set_name!(PyType_Type, b"type\0");
        set_name!(PyBaseObject_Type, b"object\0");
        set_name!(PyFrozenSet_Type, b"frozenset\0");

        // ── Builtin type-object shells: full CPython v3.12.0 `tp_flags` / `tp_base`
        // / `tp_basicsize`. The `set_name!` above left `tp_flags = READY` only, so
        // `PyType_FastSubclass(&PyLong_Type, LONG_SUBCLASS)`, `PyType_HasFeature(_,
        // BASETYPE)` and the `tp_base`-chain `PyType_IsSubtype` all answered wrong
        // (numpy's inlined feature/subclass tests miss). Per-type `tp_flags` verified
        // against each `Objects/*.c` static initialiser PLUS the `*_SUBCLASS` bits
        // `inherit_special` folds in at ready-time (e.g. bool inherits LONG_SUBCLASS
        // from int); `tp_base` set so the subtype chain terminates at `object`
        // (matrix PyTypeObject #1/#2/#5, L3). `tp_basicsize` is set for the numeric
        // /object leaves numpy subclasses (their layout is a real molt struct);
        // container/str/module basicsize is left to the bridge (molt does not store
        // them as fixed CPython structs) — flagged, not silently faked.
        macro_rules! shell {
            ($ty:expr, $flags:expr, $base:expr) => {
                $ty.tp_flags = ($flags) | Py_TPFLAGS_READY;
                $ty.tp_base = $base;
            };
        }
        let object: *mut PyTypeObject = &raw mut PyBaseObject_Type;
        // object — root of the hierarchy; DEFAULT|BASETYPE, no base.
        PyBaseObject_Type.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE | Py_TPFLAGS_READY;
        PyBaseObject_Type.tp_base = std::ptr::null_mut();
        PyBaseObject_Type.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
        // type — the metaclass. HAVE_GC|BASETYPE|TYPE_SUBCLASS|HAVE_VECTORCALL|
        // ITEMS_AT_END. (tp_basicsize = sizeof(PyHeapTypeObject) + tp_itemsize are
        // set with the heap-type work.) numpy's `_DTypeMeta` sets tp_base=&PyType_Type.
        shell!(
            PyType_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_HAVE_GC
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_TYPE_SUBCLASS
                | Py_TPFLAGS_HAVE_VECTORCALL
                | Py_TPFLAGS_ITEMS_AT_END,
            object
        );
        // `type` instances ARE heap types: CPython `PyType_Type.tp_basicsize =
        // sizeof(PyHeapTypeObject)`, `tp_itemsize = sizeof(PyMemberDef)`. A metatype
        // that leaves its own basicsize 0 (numpy's `_DTypeMeta`, tp_base=&PyType_Type)
        // inherits this at ready-time, so its instances have room for the ht_* tail.
        PyType_Type.tp_basicsize = std::mem::size_of::<PyHeapTypeObject>() as Py_ssize_t;
        PyType_Type.tp_itemsize = std::mem::size_of::<PyMemberDef>() as Py_ssize_t;
        // int — LONG_SUBCLASS|BASETYPE|MATCH_SELF; variable-length (ob_digit tail).
        shell!(
            PyLong_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_LONG_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        PyLong_Type.tp_basicsize =
            core::mem::offset_of!(PyLongObject, long_value.ob_digit) as Py_ssize_t;
        PyLong_Type.tp_itemsize = std::mem::size_of::<u32>() as Py_ssize_t;
        // bool — subclass of int (inherits LONG_SUBCLASS); NOT BASETYPE (final).
        shell!(
            PyBool_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_LONG_SUBCLASS,
            &raw mut PyLong_Type
        );
        PyBool_Type.tp_basicsize =
            core::mem::offset_of!(PyLongObject, long_value.ob_digit) as Py_ssize_t;
        PyBool_Type.tp_itemsize = std::mem::size_of::<u32>() as Py_ssize_t;
        // float — BASETYPE|MATCH_SELF. CPython PyFloatObject = {PyObject_HEAD; double}.
        shell!(
            PyFloat_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        PyFloat_Type.tp_basicsize =
            (std::mem::size_of::<PyObject>() + std::mem::size_of::<std::os::raw::c_double>())
                as Py_ssize_t;
        // complex — BASETYPE.
        shell!(
            PyComplex_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE,
            object
        );
        PyComplex_Type.tp_basicsize = std::mem::size_of::<PyComplexObject>() as Py_ssize_t;
        // str — UNICODE_SUBCLASS|BASETYPE|MATCH_SELF.
        shell!(
            PyUnicode_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_UNICODE_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        // bytes — BYTES_SUBCLASS|BASETYPE|MATCH_SELF; variable-length (ob_sval tail).
        shell!(
            PyBytes_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_BYTES_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        PyBytes_Type.tp_basicsize = core::mem::offset_of!(PyBytesObject, ob_sval) as Py_ssize_t;
        PyBytes_Type.tp_itemsize = std::mem::size_of::<std::os::raw::c_char>() as Py_ssize_t;
        // bytearray — BASETYPE|MATCH_SELF.
        shell!(
            PyByteArray_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        PyByteArray_Type.tp_basicsize = std::mem::size_of::<PyByteArrayObject>() as Py_ssize_t;
        // list — HAVE_GC|BASETYPE|LIST_SUBCLASS|MATCH_SELF|SEQUENCE.
        shell!(
            PyList_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_HAVE_GC
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_LIST_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF
                | Py_TPFLAGS_SEQUENCE,
            object
        );
        // tuple — HAVE_GC|BASETYPE|TUPLE_SUBCLASS|MATCH_SELF|SEQUENCE.
        shell!(
            PyTuple_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_HAVE_GC
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_TUPLE_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF
                | Py_TPFLAGS_SEQUENCE,
            object
        );
        // dict — HAVE_GC|BASETYPE|DICT_SUBCLASS|MATCH_SELF|MAPPING.
        shell!(
            PyDict_Type,
            Py_TPFLAGS_DEFAULT
                | Py_TPFLAGS_HAVE_GC
                | Py_TPFLAGS_BASETYPE
                | Py_TPFLAGS_DICT_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF
                | Py_TPFLAGS_MAPPING,
            object
        );
        // set / frozenset — HAVE_GC|BASETYPE|MATCH_SELF.
        shell!(
            PySet_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC | Py_TPFLAGS_BASETYPE | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        shell!(
            PyFrozenSet_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC | Py_TPFLAGS_BASETYPE | _Py_TPFLAGS_MATCH_SELF,
            object
        );
        // module — HAVE_GC|BASETYPE.
        shell!(
            PyModule_Type,
            Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC | Py_TPFLAGS_BASETYPE,
            object
        );

        Py_None.ob_type = &raw mut PyNone_Type;
        // `Py_True`/`Py_False` set `ob_base.ob_type = &PyBool_Type` in their const
        // initialiser (they are value-carrying `PyLongObject`s now), so no runtime
        // ob_type patch is needed here.
        Py_NotImplementedSentinel.ob_type = &raw mut PyNotImplemented_Type;
        Py_EllipsisObject.ob_type = &raw mut PyBaseObject_Type;
        PyDateTime_TimeZone_UTC_Object.ob_type = &raw mut PyDateTime_TZInfoType;
    }
}

/// Register the runtime's canonical CPython-ABI *sentinel* data objects in the
/// object bridge so a native extension that resolves them (via the split-runtime
/// GOT data retarget) and hands them straight back to the runtime — as a
/// `PyDict_SetItem` value or key — resolves through `pyobj_to_handle` instead of
/// failing the bridge lookup.
///
/// Scope: every canonical CPython-ABI data object the runtime owns and that a
/// native extension can hand back by *pointer identity* — the exception
/// singletons, the `Ellipsis` / `NotImplemented` / UTC sentinels, and the static
/// *type* objects (`PyBool_Type`, `PyLong_Type`, ...). The type statics MUST be
/// registered up front: numpy references builtin types (`&PyLong_Type`,
/// `&PyUnicode_Type`, ...) *by address* — as `PyDict_SetItem` keys in its
/// scalar-type → DType registry — without ever calling `PyType_Ready` on them
/// (they are already `Py_TPFLAGS_READY` from `init_static_types`), so relying on
/// the `PyType_Ready` bridge registration alone leaves them unresolved. This is
/// the full canonical data-symbol object set (see
/// `wasm_cpython_abi_data_symbol_names()` in `src/molt/_wasm_runtime_exports.py`,
/// cross-checked by `test_register_static_abi_objects_covers_type_statics`), minus
/// the integer/flag constants (`Py_EQ`, `Py_OptimizeFlag`, ...) which are not
/// `PyObject`s. `Py_None` / `Py_True` / `Py_False` are resolved by identity in
/// `pyobj_to_handle_static` and must NOT be raw-registered (that would shadow their
/// canonical NaN-boxed handles); note their *type* objects (`PyNone_Type`,
/// `PyBool_Type`, `PyNotImplemented_Type`) ARE registered here — a type object is
/// distinct from the singleton instance.
///
/// Idempotent (`register_raw_pyobj` no-ops on a re-seen pointer), so it is safe to
/// call from the `Once`-guarded `molt_cpython_abi_init` and to overlap with the
/// per-type `PyType_Ready` registration.
pub fn register_static_abi_objects() {
    let mut bridge = crate::bridge::GLOBAL_BRIDGE.lock();
    for ptr in exc_singleton_ptrs() {
        unsafe { bridge.register_raw_pyobj(ptr) };
    }
    let sentinels: [*mut PyObject; 3] = [
        &raw mut Py_NotImplementedSentinel,
        &raw mut Py_EllipsisObject,
        &raw mut PyDateTime_TimeZone_UTC_Object,
    ];
    for ptr in sentinels {
        unsafe { bridge.register_raw_pyobj(ptr) };
    }
    for ptr in type_static_ptrs() {
        unsafe { bridge.register_raw_pyobj(ptr) };
    }
}

/// Addresses of every canonical static *type* object the runtime owns, for bridge
/// registration. These are the `Py*_Type` data symbols a native extension resolves
/// (via the split-runtime GOT data retarget) and hands back to the runtime as a
/// `PyDict_SetItem` key/value or `PyModule_AddObject` value. Kept in lock-step with
/// the `*_Type` entries of `wasm_cpython_abi_data_symbol_names()` — the split-runtime
/// export authority — by `test_register_static_abi_objects_covers_type_statics`.
pub fn type_static_ptrs() -> Vec<*mut PyObject> {
    vec![
        &raw mut PyBaseObject_Type as *mut PyObject,
        &raw mut PyBool_Type as *mut PyObject,
        &raw mut PyByteArray_Type as *mut PyObject,
        &raw mut PyBytes_Type as *mut PyObject,
        &raw mut PyCFunction_Type as *mut PyObject,
        &raw mut PyCapsule_Type as *mut PyObject,
        &raw mut PyComplex_Type as *mut PyObject,
        &raw mut PyContextVar_Type as *mut PyObject,
        &raw mut PyDateTime_DateTimeType as *mut PyObject,
        &raw mut PyDateTime_DateType as *mut PyObject,
        &raw mut PyDateTime_DeltaType as *mut PyObject,
        &raw mut PyDateTime_TZInfoType as *mut PyObject,
        &raw mut PyDateTime_TimeType as *mut PyObject,
        &raw mut PyDictProxy_Type as *mut PyObject,
        &raw mut PyDict_Type as *mut PyObject,
        &raw mut PyFloat_Type as *mut PyObject,
        &raw mut PyFrozenSet_Type as *mut PyObject,
        &raw mut PyGetSetDescr_Type as *mut PyObject,
        &raw mut PyList_Type as *mut PyObject,
        &raw mut PyLong_Type as *mut PyObject,
        &raw mut PyMemberDescr_Type as *mut PyObject,
        &raw mut PyMemoryView_Type as *mut PyObject,
        &raw mut PyMethodDescr_Type as *mut PyObject,
        &raw mut PyMethod_Type as *mut PyObject,
        &raw mut PyModuleDef_Type as *mut PyObject,
        &raw mut PyModule_Type as *mut PyObject,
        &raw mut PyNone_Type as *mut PyObject,
        &raw mut PyNotImplemented_Type as *mut PyObject,
        &raw mut PySet_Type as *mut PyObject,
        &raw mut PySlice_Type as *mut PyObject,
        &raw mut PyTuple_Type as *mut PyObject,
        &raw mut PyType_Type as *mut PyObject,
        &raw mut PyUnicode_Type as *mut PyObject,
        &raw mut Py_GenericAliasType as *mut PyObject,
    ]
}

// ─── Exception singletons ──────────────────────────────────────────────────
//
// Extensions receive these as opaque `*mut PyObject` passed to PyErr_SetString.
// The exact type/content doesn't matter — they're identity-compared by the bridge.
// We create one sentinel PyObject per exception class.

// Expands an exception-singleton parent spec: `ROOT` (BaseException) has no
// parent; anything else names its base-class singleton.
macro_rules! exc_parent_expand {
    (ROOT) => {
        None
    };
    ($parent:ident) => {
        Some(&raw mut $parent as *mut PyObject)
    };
}

// One `pub static mut $name: PyObject` per exception class, plus a single
// authoritative name lookup ([`exc_singleton_name`]) AND the base-class edge
// ([`exc_singleton_parent`]) generated from the same list so the three can
// never drift. The list is the sole source of truth; each entry's parent is
// the documented Python 3.12 builtin exception hierarchy
// (https://docs.python.org/3.12/library/exceptions.html#exception-hierarchy),
// pinned by `exception_hierarchy_matches_python_3_12` below.
macro_rules! exc_singletons {
    ($($name:ident => $parent:tt),* $(,)?) => {
        $(
            #[unsafe(no_mangle)]
            pub static mut $name: PyObject = PyObject {
                // Immortal: CPython's PyExc_* are immortal type objects, so a
                // borrowed-ref over-DECREF must NOT free this static. Routed
                // through the single [`IMMORTAL_REFCNT`] authority (was `1`,
                // which molt's own Py_DECREF treated as MORTAL → static-free).
                ob_refcnt: IMMORTAL_REFCNT,
                ob_type: std::ptr::null_mut(),
            };
        )*

        /// If `ptr` is the address of one of the exception singletons, return
        /// its C name (e.g. `"PyExc_Exception"`). Pointer-identity only — no
        /// dereference — so it is safe for any `*const PyObject`.
        pub fn exc_singleton_name(ptr: *const PyObject) -> Option<&'static str> {
            $(
                if std::ptr::eq(ptr, &raw const $name) {
                    return Some(stringify!($name));
                }
            )*
            None
        }

        /// The base class of a builtin exception singleton per the Python 3.12
        /// hierarchy, or `None` for `BaseException` (root) and for a pointer
        /// that is not an exception singleton. Powers the subclass walk in
        /// `PyErr_GivenExceptionMatches` (`except LookupError` catching a
        /// pending `IndexError`). Pointer-identity only — no dereference.
        pub fn exc_singleton_parent(ptr: *const PyObject) -> Option<*mut PyObject> {
            $(
                if std::ptr::eq(ptr, &raw const $name) {
                    return exc_parent_expand!($parent);
                }
            )*
            None
        }

        /// Addresses of every exception singleton, for bridge registration. These
        /// are canonical runtime data symbols a native extension resolves (via the
        /// split-runtime GOT data retarget) and then hands back to the runtime as a
        /// `PyDict_SetItem` value (numpy's `error = Exception`), so the bridge must
        /// resolve them in `pyobj_to_handle` instead of failing the lookup.
        pub fn exc_singleton_ptrs() -> Vec<*mut PyObject> {
            vec![
                $( &raw mut $name as *mut PyObject, )*
            ]
        }
    };
}

exc_singletons!(
    PyExc_BaseException => ROOT,
    PyExc_Exception => PyExc_BaseException,
    PyExc_ValueError => PyExc_Exception,
    PyExc_TypeError => PyExc_Exception,
    PyExc_RuntimeError => PyExc_Exception,
    PyExc_MemoryError => PyExc_Exception,
    PyExc_IndexError => PyExc_LookupError,
    PyExc_KeyError => PyExc_LookupError,
    PyExc_AttributeError => PyExc_Exception,
    PyExc_OverflowError => PyExc_ArithmeticError,
    PyExc_ZeroDivisionError => PyExc_ArithmeticError,
    PyExc_ImportError => PyExc_Exception,
    PyExc_ModuleNotFoundError => PyExc_ImportError,
    PyExc_StopIteration => PyExc_Exception,
    PyExc_NotImplementedError => PyExc_RuntimeError,
    PyExc_OSError => PyExc_Exception,
    // CPython aliases IOError to the OSError object itself; the ABI keeps a
    // distinct singleton, so subclass-of-OSError is the closest sound edge
    // (an `except OSError` catches a pending IOError, as in CPython).
    PyExc_IOError => PyExc_OSError,
    PyExc_FileNotFoundError => PyExc_OSError,
    PyExc_PermissionError => PyExc_OSError,
    PyExc_FileExistsError => PyExc_OSError,
    PyExc_IsADirectoryError => PyExc_OSError,
    PyExc_NotADirectoryError => PyExc_OSError,
    PyExc_TimeoutError => PyExc_OSError,
    PyExc_ArithmeticError => PyExc_Exception,
    PyExc_FloatingPointError => PyExc_ArithmeticError,
    PyExc_LookupError => PyExc_Exception,
    PyExc_AssertionError => PyExc_Exception,
    PyExc_EOFError => PyExc_Exception,
    PyExc_NameError => PyExc_Exception,
    PyExc_UnboundLocalError => PyExc_NameError,
    PyExc_SyntaxError => PyExc_Exception,
    PyExc_SystemError => PyExc_Exception,
    PyExc_SystemExit => PyExc_BaseException,
    PyExc_UnicodeError => PyExc_ValueError,
    PyExc_UnicodeDecodeError => PyExc_UnicodeError,
    PyExc_UnicodeEncodeError => PyExc_UnicodeError,
    PyExc_BufferError => PyExc_Exception,
    PyExc_RecursionError => PyExc_RuntimeError,
    PyExc_GeneratorExit => PyExc_BaseException,
    PyExc_KeyboardInterrupt => PyExc_BaseException,
    PyExc_ConnectionError => PyExc_OSError,
    PyExc_ConnectionResetError => PyExc_ConnectionError,
    PyExc_BrokenPipeError => PyExc_ConnectionError,
    PyExc_Warning => PyExc_Exception,
    PyExc_DeprecationWarning => PyExc_Warning,
    PyExc_RuntimeWarning => PyExc_Warning,
    PyExc_FutureWarning => PyExc_Warning,
    PyExc_ImportWarning => PyExc_Warning,
    PyExc_UserWarning => PyExc_Warning,
);

#[cfg(test)]
mod exc_hierarchy_tests {
    use super::*;

    /// Table-drift gate for the hand-synced parent edges: every chain must
    /// terminate at BaseException, and the documented 3.12 subclass chains
    /// hold (https://docs.python.org/3.12/library/exceptions.html).
    #[test]
    fn exception_hierarchy_matches_python_3_12() {
        fn chain(mut ptr: *mut PyObject) -> Vec<&'static str> {
            let mut names = vec![exc_singleton_name(ptr).unwrap()];
            while let Some(parent) = exc_singleton_parent(ptr) {
                names.push(exc_singleton_name(parent).unwrap());
                ptr = parent;
            }
            names
        }
        // Spot-pin the load-bearing chains.
        assert_eq!(
            chain(&raw mut PyExc_IndexError),
            ["PyExc_IndexError", "PyExc_LookupError", "PyExc_Exception", "PyExc_BaseException"]
        );
        assert_eq!(
            chain(&raw mut PyExc_OverflowError),
            [
                "PyExc_OverflowError",
                "PyExc_ArithmeticError",
                "PyExc_Exception",
                "PyExc_BaseException"
            ]
        );
        assert_eq!(
            chain(&raw mut PyExc_UnicodeDecodeError),
            [
                "PyExc_UnicodeDecodeError",
                "PyExc_UnicodeError",
                "PyExc_ValueError",
                "PyExc_Exception",
                "PyExc_BaseException"
            ]
        );
        assert_eq!(
            chain(&raw mut PyExc_BrokenPipeError),
            [
                "PyExc_BrokenPipeError",
                "PyExc_ConnectionError",
                "PyExc_OSError",
                "PyExc_Exception",
                "PyExc_BaseException"
            ]
        );
        assert_eq!(
            chain(&raw mut PyExc_ModuleNotFoundError),
            [
                "PyExc_ModuleNotFoundError",
                "PyExc_ImportError",
                "PyExc_Exception",
                "PyExc_BaseException"
            ]
        );
        // EVERY singleton chain terminates at BaseException with no cycle
        // (bounded walk) — a wrong edge cannot hide.
        for ptr in exc_singleton_ptrs() {
            let names = chain(ptr);
            assert!(names.len() <= 8, "suspicious chain length for {names:?}");
            assert_eq!(*names.last().unwrap(), "PyExc_BaseException");
        }
    }
}

/// Best-effort human description of a `*mut PyObject` that failed bridge
/// resolution (`pyobj_to_handle` returned `None`), for the C-API silent-failure
/// diagnostic. Reads only the object header (`ob_type` → `tp_name`) plus an
/// identity check against the exception singletons, so it is safe for any
/// non-null pointer a C extension may hand us.
///
/// # Safety
/// `ptr` must be null or point to a readable `PyObject` header.
pub unsafe fn describe_unresolved_pyobject(ptr: *const PyObject) -> String {
    if ptr.is_null() {
        return "NULL".to_string();
    }
    if let Some(name) = exc_singleton_name(ptr) {
        return format!("exception-singleton {name}");
    }
    let ob_type = unsafe { (*ptr).ob_type };
    if ob_type.is_null() {
        // A non-null pointer to a bare `PyObject { ob_refcnt, ob_type: NULL }`
        // that is neither None/True/False nor one of the runtime's exception
        // singletons. In a split-runtime build this is the signature of a
        // *duplicated* cpython-abi data sentinel: the C extension linked its own
        // uninitialized copy of `Py_None`/`Py_True`/`Py_False`/`PyExc_*` at a
        // different address than the runtime's, so pointer-identity resolution
        // misses it. Emit the pointer plus the runtime's canonical sentinel
        // addresses so the split-runtime data-symbol duplication is diagnosable
        // directly from the failure record.
        let addr = ptr as usize;
        let none = &raw const Py_None as usize;
        let t = &raw const Py_True as usize;
        let f = &raw const Py_False as usize;
        return format!(
            "bare-sentinel(ob_type=NULL, addr={addr:#x}; runtime Py_None={none:#x} Py_True={t:#x} Py_False={f:#x})"
        );
    }
    let tp_name = unsafe { (*ob_type).tp_name };
    let type_name = if tp_name.is_null() {
        "<unnamed>".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(tp_name) }
            .to_string_lossy()
            .into_owned()
    };
    if std::ptr::eq(ob_type as *const PyTypeObject, &raw const PyType_Type) {
        format!("type-object '{type_name}'")
    } else {
        format!("instance-of '{type_name}'")
    }
}

/// Py_HASH_EXTERNAL constant — used by some extensions.
pub const Py_HASH_EXTERNAL: c_int = 0;
pub const PyBUF_SIMPLE: c_int = 0;
pub const PyBUF_WRITABLE: c_int = 0x0001;
pub const PyBUF_WRITEABLE: c_int = PyBUF_WRITABLE;
pub const PyBUF_READ: c_int = 0x0100;
pub const PyBUF_WRITE: c_int = 0x0200;
pub const PyBUF_FORMAT: c_int = 0x0004;
pub const PyBUF_ND: c_int = 0x0008;
pub const PyBUF_STRIDES: c_int = 0x0010 | PyBUF_ND;
pub const PyBUF_C_CONTIGUOUS: c_int = 0x0020 | PyBUF_STRIDES;
pub const PyBUF_F_CONTIGUOUS: c_int = 0x0040 | PyBUF_STRIDES;
pub const PyBUF_ANY_CONTIGUOUS: c_int = 0x0080 | PyBUF_STRIDES;
pub const PyBUF_INDIRECT: c_int = 0x0100 | PyBUF_STRIDES;
pub const PyBUF_CONTIG_RO: c_int = PyBUF_ND;
pub const PyBUF_CONTIG: c_int = PyBUF_ND | PyBUF_WRITABLE;
pub const PyBUF_RECORDS_RO: c_int = PyBUF_STRIDES | PyBUF_FORMAT;
pub const PyBUF_RECORDS: c_int = PyBUF_STRIDES | PyBUF_FORMAT | PyBUF_WRITABLE;
pub const PyBUF_FULL_RO: c_int = PyBUF_INDIRECT | PyBUF_FORMAT;
pub const PyBUF_FULL: c_int = PyBUF_INDIRECT | PyBUF_FORMAT | PyBUF_WRITABLE;

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
    /// Embedded descriptor storage — CPython's `ob_array` model
    /// (Objects/memoryobject.c `memory_alloc` places shape/strides in the
    /// memoryview object's own tail storage and `init_shape_strides` re-points
    /// `view.shape`/`view.strides` into it). For memoryviews built by
    /// `PyMemoryView_FromBuffer` the copied descriptor VALUES live here, so
    /// `view.format`/`shape`/`strides` point into the object itself: the
    /// descriptor dies with the object, there is no side allocation to free,
    /// and `PyBuffer_Release` stays pure obj-dispatch (no registry, no
    /// `internal` deref). Fields are appended after `base`, so the
    /// `ob_base`/`view`/`base` prefix layout seen by C is unchanged.
    pub ob_shape: [Py_ssize_t; 64],
    pub ob_strides: [Py_ssize_t; 64],
    pub ob_format: [u8; 16],
}

// Literal capacities above keep the C-layout generator
// (tools/gen_cpython_abi_layout.py) parseable; these bind them to the single
// authority so they cannot drift.
const _: () = assert!(crate::hooks::MOLT_BUFFER_MAX_NDIM == 64);
const _: () = assert!(crate::hooks::MOLT_BUFFER_FORMAT_CAP == 16);

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

#[cfg(test)]
mod unresolved_pyobject_tests {
    use super::*;

    #[test]
    fn exc_singleton_name_identifies_exception_globals_by_address() {
        // A real exception-singleton pointer resolves to its C name; an
        // unrelated pointer does not. Address-identity only, so this is the
        // authority the silent-failure diagnostic relies on.
        assert_eq!(
            exc_singleton_name(&raw const PyExc_Exception),
            Some("PyExc_Exception")
        );
        assert_eq!(
            exc_singleton_name(&raw const PyExc_ValueError),
            Some("PyExc_ValueError")
        );
        let mut unrelated = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        assert_eq!(
            exc_singleton_name(&raw mut unrelated as *const PyObject),
            None
        );
    }

    #[test]
    fn describe_unresolved_pyobject_classifies_common_shapes() {
        // NULL.
        assert_eq!(
            unsafe { describe_unresolved_pyobject(std::ptr::null()) },
            "NULL"
        );
        // A runtime exception singleton is named directly.
        assert_eq!(
            unsafe { describe_unresolved_pyobject(&raw const PyExc_Exception) },
            "exception-singleton PyExc_Exception"
        );
        // A bare sentinel (ob_type == NULL) that is not an exception singleton —
        // the split-runtime duplicated-data-symbol signature. The message names
        // the runtime's canonical sentinel addresses for comparison.
        let mut bare = PyObject {
            ob_refcnt: IMMORTAL_REFCNT,
            ob_type: std::ptr::null_mut(),
        };
        let desc = unsafe { describe_unresolved_pyobject(&raw mut bare) };
        assert!(
            desc.starts_with("bare-sentinel(ob_type=NULL"),
            "unexpected description: {desc}"
        );
        assert!(
            desc.contains("runtime Py_None="),
            "missing runtime addrs: {desc}"
        );
        // A type object (ob_type == &PyType_Type) is reported as a type-object.
        let mut named_type: PyTypeObject = unsafe { std::mem::zeroed() };
        named_type.tp_name = c"widget".as_ptr();
        let mut type_instance = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut named_type,
        };
        // Point PyType_Type's identity check: mark the object's type as
        // PyType_Type so the classifier reports a type-object.
        unsafe {
            type_instance.ob_type = &raw mut PyType_Type;
            PyType_Type.tp_name = c"type".as_ptr();
        }
        let desc = unsafe { describe_unresolved_pyobject(&raw mut type_instance) };
        assert!(desc.starts_with("type-object "), "unexpected: {desc}");
    }

    #[test]
    fn type_static_ptrs_are_distinct_and_nonnull() {
        // Exactly the canonical `Py*_Type` data-symbol set (34 statics). Guards
        // against an accidental drop/duplicate when the type static list changes.
        let ptrs = type_static_ptrs();
        assert_eq!(ptrs.len(), 34, "type static count drifted");
        for p in &ptrs {
            assert!(!p.is_null());
        }
        let mut addrs: Vec<usize> = ptrs.iter().map(|p| *p as usize).collect();
        addrs.sort_unstable();
        addrs.dedup();
        assert_eq!(
            addrs.len(),
            34,
            "duplicate type static in type_static_ptrs()"
        );
    }

    #[test]
    fn register_static_abi_objects_resolves_type_statics_and_singletons() {
        // Regression for the numpy `_multiarray_umath` `PyDict_SetItem(unresolved
        // key)` frontier: a builtin type static (`&PyLong_Type` &c.) handed back by
        // the extension as a dict key must resolve through `pyobj_to_handle` after
        // `register_static_abi_objects`, instead of failing the bridge lookup. This
        // asserts the bridge RESOLVES the canonical objects — it does NOT weaken the
        // unresolved-object check (an unregistered pointer still returns `None`).
        register_static_abi_objects();
        let bridge = crate::bridge::GLOBAL_BRIDGE.lock();
        for ptr in type_static_ptrs() {
            assert!(
                bridge.pyobj_to_handle(ptr).is_some(),
                "type static @ {ptr:p} did not resolve after registration"
            );
        }
        for ptr in exc_singleton_ptrs() {
            assert!(
                bridge.pyobj_to_handle(ptr).is_some(),
                "exception singleton @ {ptr:p} did not resolve after registration"
            );
        }
        // Negative control: a fresh, never-registered pointer still fails to
        // resolve — the unresolved check is intact, not blanket-weakened.
        let mut stray = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        assert!(
            bridge.pyobj_to_handle(&raw mut stray).is_none(),
            "unregistered pointer must remain unresolved"
        );
    }
}

#[cfg(test)]
mod immortal_authority_tests {
    //! Lane ABI-SINGLETON-IMMORTAL — the immortal-refcount authority is SINGLE
    //! (one value, one predicate) and every static singleton routes through it.
    use super::*;

    /// The one immortal value equals CPython's `_Py_IMMORTAL_REFCNT` for this
    /// target width (primary source: python/cpython v3.12.0 Include/object.h —
    /// `UINT_MAX` / `UINT_MAX >> 2`), and the predicate mirrors `_Py_IsImmortal`.
    #[test]
    fn immortal_refcnt_matches_cpython_for_this_width() {
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            IMMORTAL_REFCNT,
            u32::MAX as Py_ssize_t,
            "3.12/3.13 64-bit immortal must be UINT_MAX (0xFFFF_FFFF)"
        );
        #[cfg(target_pointer_width = "32")]
        assert_eq!(
            IMMORTAL_REFCNT,
            (u32::MAX >> 2) as Py_ssize_t,
            "3.12/3.13 32-bit immortal must be UINT_MAX>>2 (0x3FFF_FFFF)"
        );
        assert!(
            is_immortal_refcnt(IMMORTAL_REFCNT),
            "the immortal value must read immortal"
        );
        // A fresh/mortal refcount is NOT immortal — this is exactly what the old
        // `ob_refcnt: 1` exception singletons had, so the pre-fix state fails the
        // single-encoding gate below by construction.
        assert!(!is_immortal_refcnt(1), "refcount 1 (the old exc encoding) is mortal");
        assert!(!is_immortal_refcnt(0), "refcount 0 (zeroed type static) is mortal");
        assert!(!is_immortal_refcnt(4096), "an ordinary refcount is mortal");
    }

    /// Single-authority behavioural gate: after init, EVERY static singleton the
    /// runtime owns carries exactly `IMMORTAL_REFCNT` — no `1`, `0`, or `1<<30`
    /// survivor. A regression that re-introduces a second encoding on any of
    /// these trips here.
    #[test]
    fn all_static_singletons_share_the_one_immortal_encoding() {
        crate::bridge::molt_cpython_abi_init();
        let mut singletons: Vec<*mut PyObject> = vec![
            &raw mut Py_None,
            (&raw mut Py_True).cast::<PyObject>(),
            (&raw mut Py_False).cast::<PyObject>(),
            &raw mut Py_NotImplementedSentinel,
            &raw mut Py_EllipsisObject,
            &raw mut PyDateTime_TimeZone_UTC_Object,
        ];
        singletons.extend(exc_singleton_ptrs());
        singletons.extend(type_static_ptrs());
        for p in singletons {
            let rc = unsafe { (*p).ob_refcnt };
            assert_eq!(
                rc, IMMORTAL_REFCNT,
                "singleton @ {p:p} uses a non-authority refcnt encoding: {rc:#x}"
            );
            assert!(is_immortal_refcnt(rc), "singleton @ {p:p} not detected immortal");
        }
    }

    /// Mask-proof (memory-corruption regression): a static exception singleton
    /// survives a net-negative over-DECREF. Pre-fix its `ob_refcnt` was `1`, which
    /// molt's own `Py_DECREF` treated as MORTAL, so a borrowed-ref over-DECREF
    /// reached 0 and `release_pyobj`/`tp_dealloc`'d statically-allocated memory
    /// AND dropped its bridge identity (breaking later `PyErr_SetString` matches).
    #[test]
    fn exc_singleton_survives_over_decref() {
        crate::bridge::molt_cpython_abi_init();
        let exc = &raw mut PyExc_ValueError;
        let rc_before = unsafe { (*exc).ob_refcnt };
        assert!(
            is_immortal_refcnt(rc_before),
            "PyExc_ValueError must be immortal (pre-fix it was the mortal `1`)"
        );
        assert!(
            crate::bridge::GLOBAL_BRIDGE.lock().pyobj_to_handle(exc).is_some(),
            "exc singleton must be bridge-registered before the over-DECREF"
        );
        // 8 net-negative DECREFs — for a mortal `1` this reaches 0 and frees.
        for _ in 0..8 {
            unsafe { crate::api::refcount::Py_DECREF(exc) };
        }
        assert_eq!(
            unsafe { (*exc).ob_refcnt },
            rc_before,
            "immortal exception singleton refcount changed under DECREF"
        );
        assert!(
            crate::bridge::GLOBAL_BRIDGE.lock().pyobj_to_handle(exc).is_some(),
            "exc singleton lost bridge identity after over-DECREF (static-free regression)"
        );
    }

    /// Anti-duplication teeth (textual): no raw immortal-refcount literal survives
    /// outside the single authority. Needles are reconstructed at runtime so this
    /// scanner never matches itself. The legitimate flag `= 1 << 30`
    /// (`Py_TPFLAGS_BASE_EXC_SUBCLASS`) is not an `ob_refcnt` initialiser, so the
    /// `ob_refcnt:`/`ob_refcnt =` prefix keeps it out of scope.
    #[test]
    fn no_raw_immortal_literal_outside_the_authority() {
        let sources: [(&str, &str); 3] = [
            ("abi_types.rs", include_str!("abi_types.rs")),
            ("api/refcount.rs", include_str!("api/refcount.rs")),
            ("api/object.rs", include_str!("api/object.rs")),
        ];
        let forbidden: [String; 3] = [
            ["ob_refcnt: 1 ", "<< 30"].concat(), // struct-init immortal literal
            ["ob_refcnt = 1 ", "<< 30"].concat(), // assignment immortal literal
            ["(1 ", "<< 29)"].concat(),           // the old ad-hoc immortal threshold
        ];
        for (name, src) in sources {
            for needle in &forbidden {
                assert!(
                    !src.contains(needle.as_str()),
                    "{name} re-introduced a raw immortal encoding `{needle}` — \
                     route it through abi_types::IMMORTAL_REFCNT / is_immortal_refcnt",
                );
            }
        }
    }

    /// Regression: handing a registered immortal singleton back out through the
    /// bridge (`handle_to_pyobj`, the `raw_py` path) must NOT touch its refcount.
    /// Pre-fix the raw `ob_refcnt += 1` crept it up on every call; harmless under
    /// the old `>= 1<<29` threshold, but under CPython-faithful detection the
    /// first increment past `IMMORTAL_REFCNT` (bit 31 → clear) silently mortalises
    /// the static → a later DECREF frees static storage (UAF).
    #[test]
    fn bridge_never_increments_a_registered_immortal_singleton() {
        crate::bridge::molt_cpython_abi_init();
        let exc = &raw mut PyExc_TypeError;
        let mut bridge = crate::bridge::GLOBAL_BRIDGE.lock();
        let bits = bridge
            .pyobj_to_handle(exc)
            .expect("registered exc singleton resolves to a handle");
        let rc_before = unsafe { (*exc).ob_refcnt };
        for _ in 0..16 {
            let p = unsafe { bridge.handle_to_pyobj(bits) };
            assert!(std::ptr::eq(p, exc), "singleton handle round-trip lost identity");
        }
        assert_eq!(
            unsafe { (*exc).ob_refcnt },
            rc_before,
            "bridge incremented an immortal singleton (static-free/UAF vector)"
        );
        assert!(
            is_immortal_refcnt(unsafe { (*exc).ob_refcnt }),
            "singleton must remain immortal after bridge round-trips"
        );
    }

    /// Fix #3: the canonical `_Py_NoneStruct` data symbol has a non-null, correct
    /// `ob_type` (pre-fix `NULL` → `Py_TYPE(Py_None)->tp_name` null-deref) and
    /// resolves to the SAME `None` handle the runtime hands out for `Py_None`, so
    /// a real-CPython-header extension's `_Py_NoneStruct is None` holds.
    #[test]
    fn canonical_none_struct_is_typed_and_reconciled() {
        crate::bridge::molt_cpython_abi_init();
        let none_struct = &raw mut crate::api::object::_Py_NoneStruct;
        assert!(
            std::ptr::eq(unsafe { (*none_struct).ob_type }, &raw mut PyNone_Type),
            "_Py_NoneStruct.ob_type must be &PyNone_Type (was NULL → null-deref)"
        );
        assert!(is_immortal_refcnt(unsafe { (*none_struct).ob_refcnt }));
        let mut bridge = crate::bridge::GLOBAL_BRIDGE.lock();
        let via_struct = unsafe { bridge.molt_value_for_pyobj(none_struct) };
        let via_py_none = unsafe { bridge.molt_value_for_pyobj(&raw mut Py_None) };
        assert_eq!(
            via_struct, via_py_none,
            "_Py_NoneStruct must resolve to the same handle as Py_None"
        );
        assert_eq!(
            via_struct,
            Some(molt_lang_obj_model::MoltObject::none().bits()),
            "and that handle is canonical None"
        );
    }

    /// Fix #1: the canonical bool data symbols are value-carrying `PyLongObject`s,
    /// so an extension's inlined `((PyLongObject*)Py_True)->long_value.ob_digit[0]`
    /// reads `1`/`0` IN BOUNDS (pre-fix they were bare `PyObject` → OOB read).
    /// `lv_tag`/`ob_digit` verified against CPython v3.12.0 (`_PyLong_TRUE_TAG`=8,
    /// `_PyLong_FALSE_TAG`=1). Also reconciled to the same True/False handles.
    #[test]
    fn canonical_bool_structs_have_pylongobject_shape_and_reconcile() {
        crate::bridge::molt_cpython_abi_init();
        let t = &raw const crate::api::object::_Py_TrueStruct;
        let f = &raw const crate::api::object::_Py_FalseStruct;
        unsafe {
            assert_eq!((*t).long_value.ob_digit[0], 1, "True ob_digit[0]");
            assert_eq!((*t).long_value.lv_tag, 8, "True lv_tag == _PyLong_TRUE_TAG");
            assert!(
                std::ptr::eq((*t).ob_base.ob_type, &raw mut PyBool_Type),
                "True ob_type == &PyBool_Type"
            );
            assert!(is_immortal_refcnt((*t).ob_base.ob_refcnt));
            assert_eq!((*f).long_value.ob_digit[0], 0, "False ob_digit[0]");
            assert_eq!((*f).long_value.lv_tag, 1, "False lv_tag == _PyLong_FALSE_TAG");
            assert!(
                std::ptr::eq((*f).ob_base.ob_type, &raw mut PyBool_Type),
                "False ob_type == &PyBool_Type"
            );
        }
        let t_obj = (&raw mut crate::api::object::_Py_TrueStruct).cast::<PyObject>();
        let f_obj = (&raw mut crate::api::object::_Py_FalseStruct).cast::<PyObject>();
        let mut bridge = crate::bridge::GLOBAL_BRIDGE.lock();
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(t_obj) },
            unsafe { bridge.molt_value_for_pyobj((&raw mut Py_True).cast::<PyObject>()) },
            "_Py_TrueStruct must resolve to the same handle as Py_True"
        );
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(f_obj) },
            unsafe { bridge.molt_value_for_pyobj((&raw mut Py_False).cast::<PyObject>()) },
            "_Py_FalseStruct must resolve to the same handle as Py_False"
        );
    }

    /// Mask-proof (ABI-TYPEOBJECT-L4, singleton residual): the *live* molt-header
    /// singletons `Py_True`/`Py_False` — the exact storage `include/Python.h`'s
    /// `#define Py_True (&Py_True)` resolves for every witness extension — are now
    /// value-carrying `PyLongObject`s, NOT bare `PyObject`. So an extension's
    /// inlined `((PyLongObject*)Py_True)->long_value.ob_digit[0]` reads `1`/`0`
    /// IN BOUNDS. Pre-fix `Py_True` was a bare `PyObject` (16B native / 8B wasm32);
    /// the read of `long_value` at offset 16/8 landed OUT OF BOUNDS in adjacent
    /// static memory. Values verified against CPython v3.12.0 Objects/boolobject.c
    /// (`_PyLong_TRUE_TAG`=8, `_PyLong_FALSE_TAG`=1) + pycore_long.h
    /// (`TAG_FROM_SIGN_AND_SIZE`). This is the residual that made the LIVE symbol
    /// match the canonical `_Py_TrueStruct`/`_Py_FalseStruct` shape.
    #[test]
    fn live_bool_singletons_are_pylongobject_shaped_in_bounds() {
        crate::bridge::molt_cpython_abi_init();
        // The static's Rust TYPE is now `PyLongObject`; the whole crate would fail
        // to compile if it were still `PyObject` (the `long_value` field access
        // below requires it). Read the value-carrying bytes through a raw pointer,
        // exactly as an extension's `(PyLongObject*)Py_True` cast would.
        let t = &raw const Py_True;
        let f = &raw const Py_False;
        unsafe {
            // True: lv_tag == _PyLong_TRUE_TAG == 8, ob_digit[0] == 1, in bounds.
            assert_eq!((*t).long_value.lv_tag, 8, "Py_True lv_tag");
            assert_eq!(
                (*t).long_value.ob_digit[0],
                1,
                "((PyLongObject*)Py_True)->ob_digit[0] == 1 (in bounds)"
            );
            assert!(
                std::ptr::eq((*t).ob_base.ob_type, &raw mut PyBool_Type),
                "Py_True ob_type == &PyBool_Type"
            );
            assert!(is_immortal_refcnt((*t).ob_base.ob_refcnt));
            // False: lv_tag == _PyLong_FALSE_TAG == 1, ob_digit[0] == 0.
            assert_eq!((*f).long_value.lv_tag, 1, "Py_False lv_tag");
            assert_eq!(
                (*f).long_value.ob_digit[0],
                0,
                "((PyLongObject*)Py_False)->ob_digit[0] == 0 (in bounds)"
            );
            assert!(
                std::ptr::eq((*f).ob_base.ob_type, &raw mut PyBool_Type),
                "Py_False ob_type == &PyBool_Type"
            );
            assert!(is_immortal_refcnt((*f).ob_base.ob_refcnt));
        }
    }

    /// Mask-proof (ABI-TYPEOBJECT-L4, type-shell flags/base): the builtin static
    /// type shells now carry their full CPython v3.12.0 `tp_flags` (was `READY`
    /// only → every `*_SUBCLASS`/`BASETYPE` fast-check answered 0) and a `tp_base`
    /// chain (was NULL → `PyType_IsSubtype` could not walk to a base). Verifies the
    /// exact checks numpy's inlined feature tests rely on; fails pre-fix.
    #[test]
    fn builtin_type_shells_carry_correct_fast_subclass_flags_and_base() {
        crate::bridge::molt_cpython_abi_init();
        use crate::api::typeobj::{PyType_HasFeature, PyType_IsSubtype};
        unsafe {
            let long_t = &raw mut PyLong_Type;
            let bool_t = &raw mut PyBool_Type;
            let list_t = &raw mut PyList_Type;
            let dict_t = &raw mut PyDict_Type;
            let tuple_t = &raw mut PyTuple_Type;
            let unicode_t = &raw mut PyUnicode_Type;
            let type_t = &raw mut PyType_Type;
            let object_t = &raw mut PyBaseObject_Type;

            // FastSubclass = HasFeature(t, <TYPE>_SUBCLASS): each base carries its
            // own fast bit (pre-fix 0 → e.g. numpy's inlined PyLong_Check misses).
            assert_eq!(PyType_HasFeature(long_t, Py_TPFLAGS_LONG_SUBCLASS), 1, "int LONG_SUBCLASS");
            assert_eq!(PyType_HasFeature(list_t, Py_TPFLAGS_LIST_SUBCLASS), 1, "list LIST_SUBCLASS");
            assert_eq!(PyType_HasFeature(tuple_t, Py_TPFLAGS_TUPLE_SUBCLASS), 1, "tuple TUPLE_SUBCLASS");
            assert_eq!(PyType_HasFeature(dict_t, Py_TPFLAGS_DICT_SUBCLASS), 1, "dict DICT_SUBCLASS");
            assert_eq!(PyType_HasFeature(unicode_t, Py_TPFLAGS_UNICODE_SUBCLASS), 1, "str UNICODE_SUBCLASS");
            assert_eq!(PyType_HasFeature(type_t, Py_TPFLAGS_TYPE_SUBCLASS), 1, "type TYPE_SUBCLASS");

            // Subclassability: int/list/object ARE BASETYPE; bool is NOT (final).
            assert_eq!(PyType_HasFeature(long_t, Py_TPFLAGS_BASETYPE), 1, "int subclassable");
            assert_eq!(PyType_HasFeature(object_t, Py_TPFLAGS_BASETYPE), 1, "object subclassable");
            assert_eq!(PyType_HasFeature(bool_t, Py_TPFLAGS_BASETYPE), 0, "bool is final (not BASETYPE)");

            // bool IS an int subclass — via the fast bit AND the tp_base chain.
            assert_eq!(PyType_HasFeature(bool_t, Py_TPFLAGS_LONG_SUBCLASS), 1, "bool inherits LONG_SUBCLASS");
            assert_eq!(PyType_IsSubtype(bool_t, long_t), 1, "bool <: int");
            assert_eq!(PyType_IsSubtype(bool_t, object_t), 1, "bool <: object (chain)");
            assert_eq!(PyType_IsSubtype(long_t, object_t), 1, "int <: object");
            assert_eq!(PyType_IsSubtype(long_t, bool_t), 0, "int is NOT <: bool");

            // GC flag: containers carry HAVE_GC; leaf numerics do not.
            assert_eq!(PyType_HasFeature(list_t, Py_TPFLAGS_HAVE_GC), 1, "list HAVE_GC");
            assert_eq!(PyType_HasFeature(long_t, Py_TPFLAGS_HAVE_GC), 0, "int no HAVE_GC");
        }
    }

    /// `Py_TPFLAGS_DEFAULT` must be 0 on the standard 3.12 build (was `= BASETYPE`,
    /// a duplicate-authority drift vs the C header `Py_TPFLAGS_DEFAULT (0)`; matrix
    /// PyTypeObject #5).
    #[test]
    fn tpflags_default_is_zero_like_cpython_3_12() {
        assert_eq!(Py_TPFLAGS_DEFAULT, 0);
    }

    /// `tp_basicsize` on the value-carrying numeric/object leaves is non-zero and
    /// matches the molt struct layout (pre-fix 0 → a C subclass inherits a
    /// zero-size base and its instances are truncated). Verified against the exact
    /// CPython v3.12.0 variable-length scheme for int (offsetof(long_value.ob_digit)
    /// + itemsize=sizeof(digit)).
    #[test]
    fn builtin_numeric_leaves_have_correct_basicsize() {
        crate::bridge::molt_cpython_abi_init();
        // Read the fields through raw pointers into locals first (edition-2024
        // forbids borrowing a `static mut` field, which `assert_eq!` would do).
        let obj_t = &raw const PyBaseObject_Type;
        let long_t = &raw const PyLong_Type;
        let bool_t = &raw const PyBool_Type;
        let float_t = &raw const PyFloat_Type;
        let complex_t = &raw const PyComplex_Type;
        unsafe {
            let obj_bs = (*obj_t).tp_basicsize;
            let long_bs = (*long_t).tp_basicsize;
            let long_is = (*long_t).tp_itemsize;
            let bool_bs = (*bool_t).tp_basicsize;
            let float_bs = (*float_t).tp_basicsize;
            let complex_bs = (*complex_t).tp_basicsize;
            assert_eq!(
                obj_bs,
                std::mem::size_of::<PyObject>() as Py_ssize_t,
                "object basicsize == sizeof(PyObject)"
            );
            assert_eq!(
                long_bs,
                core::mem::offset_of!(PyLongObject, long_value.ob_digit) as Py_ssize_t,
                "int basicsize == offsetof(long_value.ob_digit)"
            );
            assert_eq!(long_is, std::mem::size_of::<u32>() as Py_ssize_t, "int itemsize == sizeof(digit)");
            assert_eq!(bool_bs, long_bs, "bool shares int's _longobject basicsize");
            assert!(
                float_bs
                    >= (std::mem::size_of::<PyObject>() + std::mem::size_of::<f64>()) as Py_ssize_t,
                "float basicsize holds a PyObject header + a double"
            );
            assert_eq!(
                complex_bs,
                std::mem::size_of::<PyComplexObject>() as Py_ssize_t,
                "complex basicsize == sizeof(PyComplexObject)"
            );
        }
    }

    /// Layout gate (ABI-TYPEOBJECT-L4 #4): the Rust `PyHeapTypeObject` mirror is
    /// byte-compatible with `include/Python.h`'s `_heaptypeobject` and CPython
    /// v3.12.0 `struct _heaptypeobject`. Field order is load-bearing (an
    /// extension casts `type` to `PyHeapTypeObject*` and reads the ht_* tail by
    /// offset), so this pins every offset: the header, the five inline protocol
    /// sub-tables in CPython order (no padding), then the pointer run
    /// ht_name/ht_slots/ht_qualname/ht_cached_keys/ht_module, _ht_tpname,
    /// _spec_cache. A reorder or stray pad fails here (drift catch); the C
    /// compiler's `_Static_assert` on the same struct is the cross-check.
    #[test]
    fn pyheaptypeobject_layout_matches_c_header() {
        use core::mem::{offset_of, size_of};
        assert_eq!(offset_of!(PyHeapTypeObject, ht_type), 0, "ht_type first");
        // Sub-tables follow the header in order with no padding.
        assert_eq!(offset_of!(PyHeapTypeObject, as_async), size_of::<PyTypeObject>());
        let after_subtables = size_of::<PyTypeObject>()
            + size_of::<PyAsyncMethods>()
            + size_of::<PyNumberMethods>()
            + size_of::<PyMappingMethods>()
            + size_of::<PySequenceMethods>()
            + size_of::<PyBufferProcs>();
        let p = size_of::<*mut PyObject>();
        assert_eq!(offset_of!(PyHeapTypeObject, ht_name), after_subtables, "ht_name past subtables");
        assert_eq!(offset_of!(PyHeapTypeObject, ht_slots), after_subtables + p);
        assert_eq!(offset_of!(PyHeapTypeObject, ht_qualname), after_subtables + 2 * p);
        assert_eq!(offset_of!(PyHeapTypeObject, ht_cached_keys), after_subtables + 3 * p);
        assert_eq!(offset_of!(PyHeapTypeObject, ht_module), after_subtables + 4 * p, "ht_module offset");
        assert_eq!(offset_of!(PyHeapTypeObject, _ht_tpname), after_subtables + 5 * p);
        // _spec_cache: { PyObject *getitem; uint32_t getitem_version; }.
        assert_eq!(offset_of!(SpecializationCache, getitem), 0);
        assert_eq!(offset_of!(SpecializationCache, getitem_version), p);
        // Strictly larger than a bare PyTypeObject (the pre-fix under-allocation).
        assert!(
            size_of::<PyHeapTypeObject>() > size_of::<PyTypeObject>(),
            "heap type must be larger than a bare PyTypeObject"
        );
    }
}
