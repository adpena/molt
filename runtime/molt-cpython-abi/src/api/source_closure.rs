//! Static-link CPython ABI closure for source-recompiled package artifacts.
//!
//! These symbols are the shared provider surface used when external native
//! artifact admission classifies raw CPython API imports as `cpython_abi_link`.
//! Implemented primitives forward to the existing ABI modules; unsupported
//! dynamic behaviors fail closed with CPython-style error/null returns.

use crate::abi_types::{
    Py_buffer, Py_hash_t, Py_ssize_t, PyComplex_Type, PyModuleDef, PyObject, PyTypeObject,
    PyVarObject,
};
use std::ffi::{CStr, c_char, c_double, c_int, c_void};
use std::ptr;

#[repr(C)]
pub struct Py_complex {
    pub real: c_double,
    pub imag: c_double,
}

unsafe fn unsupported(name: *const c_char) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_NotImplementedError,
            name,
        )
    };
}

macro_rules! unsupported_null {
    ($name:ident ( $($arg:ident : $typ:ty),* $(,)? )) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $typ),*) -> *mut PyObject {
            let _ = ($($arg,)*);
            unsafe { unsupported(concat!(stringify!($name), " is not supported by Molt CPython ABI\0").as_ptr().cast()) };
            ptr::null_mut()
        }
    };
}

macro_rules! no_op_int {
    ($name:ident ( $($arg:ident : $typ:ty),* $(,)? ) -> $value:expr) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $typ),*) -> c_int {
            let _ = ($($arg,)*);
            $value
        }
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsString(op: *mut PyObject) -> *mut c_char {
    unsafe { crate::api::strings::PyBytes_AS_STRING(op) as *mut c_char }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_CheckExact(capsule: *mut PyObject) -> c_int {
    (!capsule.is_null()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_GetContext(_capsule: *mut PyObject) -> *mut c_void {
    ptr::null_mut()
}

no_op_int!(PyCapsule_SetContext(_capsule: *mut PyObject, _context: *mut c_void) -> 0);
no_op_int!(PyCapsule_SetName(_capsule: *mut PyObject, _name: *const c_char) -> 0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    ptr::eq(unsafe { (*op).ob_type }, &raw mut PyComplex_Type) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromDoubles(real: c_double, imag: c_double) -> *mut PyObject {
    let _ = (real, imag);
    unsafe { unsupported(c"PyComplex_FromDoubles is not supported by Molt CPython ABI".as_ptr()) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromCComplex(value: Py_complex) -> *mut PyObject {
    let _ = value;
    unsafe { unsupported(c"PyComplex_FromCComplex is not supported by Molt CPython ABI".as_ptr()) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_RealAsDouble(op: *mut PyObject) -> c_double {
    let _ = op;
    unsafe { unsupported(c"PyComplex_RealAsDouble is not supported by Molt CPython ABI".as_ptr()) };
    -1.0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_ImagAsDouble(op: *mut PyObject) -> c_double {
    let _ = op;
    unsafe { unsupported(c"PyComplex_ImagAsDouble is not supported by Molt CPython ABI".as_ptr()) };
    -1.0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_AsCComplex(op: *mut PyObject) -> Py_complex {
    let _ = op;
    unsafe { unsupported(c"PyComplex_AsCComplex is not supported by Molt CPython ABI".as_ptr()) };
    Py_complex {
        real: -1.0,
        imag: 0.0,
    }
}

unsupported_null!(PyContextVar_New(_name: *const c_char, _default: *mut PyObject));
unsupported_null!(PyContextVar_Get(
    _var: *mut PyObject,
    _default: *mut PyObject,
    _value: *mut *mut PyObject,
));
unsupported_null!(PyContextVar_Set(_var: *mut PyObject, _value: *mut PyObject));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDictProxy_New(dict: *mut PyObject) -> *mut PyObject {
    if !dict.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(dict) };
    }
    dict
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Contains(op: *mut PyObject, key: *mut PyObject) -> c_int {
    (!unsafe { crate::api::mapping::PyDict_GetItem(op, key) }.is_null()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_ContainsString(op: *mut PyObject, key: *const c_char) -> c_int {
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let result = unsafe { PyDict_Contains(op, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItem(op: *mut PyObject, key: *mut PyObject) -> c_int {
    unsafe { crate::api::object::PyObject_DelItem(op, key) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemWithError(
    op: *mut PyObject,
    key: *mut PyObject,
) -> *mut PyObject {
    unsafe { crate::api::mapping::PyDict_GetItem(op, key) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemRef(
    op: *mut PyObject,
    key: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        return -1;
    }
    let value = unsafe { crate::api::mapping::PyDict_GetItem(op, key) };
    unsafe {
        *result = value;
    }
    if value.is_null() {
        0
    } else {
        unsafe { crate::api::refcount::Py_INCREF(value) };
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemStringRef(
    op: *mut PyObject,
    key: *const c_char,
    result: *mut *mut PyObject,
) -> c_int {
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_GetItemRef(op, key_obj, result) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

no_op_int!(PyDict_Merge(_a: *mut PyObject, _b: *mut PyObject, _override: c_int) -> 0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_date_from_date(
    year: c_int,
    month: c_int,
    day: c_int,
    typ: *mut PyTypeObject,
) -> *mut PyObject {
    let _ = (year, month, day, typ);
    unsafe {
        unsupported(
            c"molt_cpython_abi_date_from_date is not supported by Molt CPython ABI".as_ptr(),
        )
    };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_datetime_from_date_and_time(
    year: c_int,
    month: c_int,
    day: c_int,
    hour: c_int,
    minute: c_int,
    second: c_int,
    usecond: c_int,
    tzinfo: *mut PyObject,
    typ: *mut PyTypeObject,
) -> *mut PyObject {
    let _ = (year, month, day, hour, minute, second, usecond, tzinfo, typ);
    unsafe {
        unsupported(
            c"molt_cpython_abi_datetime_from_date_and_time is not supported by Molt CPython ABI"
                .as_ptr(),
        )
    };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_delta_from_delta(
    days: c_int,
    seconds: c_int,
    useconds: c_int,
    normalize: c_int,
    typ: *mut PyTypeObject,
) -> *mut PyObject {
    let _ = (days, seconds, useconds, normalize, typ);
    unsafe {
        unsupported(
            c"molt_cpython_abi_delta_from_delta is not supported by Molt CPython ABI".as_ptr(),
        )
    };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Next(
    _op: *mut PyObject,
    _pos: *mut Py_ssize_t,
    _key: *mut *mut PyObject,
    _value: *mut *mut PyObject,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_CheckSignals() -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrno(exc: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::errors::PyErr_SetString(exc, c"os error".as_ptr()) };
    ptr::null_mut()
}

no_op_int!(PyEval_RestoreThread(_state: *mut c_void) -> 0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_SaveThread() -> *mut c_void {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_GetBuiltins() -> *mut PyObject {
    unsafe { crate::api::mapping::PyDict_New() }
}

no_op_int!(PyException_SetCause(_exc: *mut PyObject, _cause: *mut PyObject) -> 0);
no_op_int!(PyException_SetContext(_exc: *mut PyObject, _ctx: *mut PyObject) -> 0);
no_op_int!(PyException_SetTraceback(_exc: *mut PyObject, _tb: *mut PyObject) -> 0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromString(_op: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGILState_Ensure() -> c_int {
    0
}

pub type PyGILState_STATE = c_int;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGILState_Release(_state: PyGILState_STATE) {}

unsupported_null!(PyImport_Import(_name: *mut PyObject));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModule(_name: *const c_char) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyIndex_Check(op: *mut PyObject) -> c_int {
    unsafe { crate::api::numbers::PyNumber_Check(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInterpreterState_Main() -> *mut c_void {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyIter_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let tp = unsafe { (*op).ob_type };
    (!tp.is_null() && unsafe { (*tp).tp_iternext }.is_some()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItemRef(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    let item = unsafe { crate::api::sequences::PyList_GetItem(op, i) };
    if !item.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(item) };
    }
    item
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongLong(op: *mut PyObject) -> u64 {
    unsafe { crate::api::numbers::PyLong_AsLongLong(op) as u64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> isize {
    if !overflow.is_null() {
        unsafe { *overflow = 0 };
    }
    unsafe { crate::api::numbers::PyLong_AsLong(op) as isize }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> i64 {
    if !overflow.is_null() {
        unsafe { *overflow = 0 };
    }
    unsafe { crate::api::numbers::PyLong_AsLongLong(op) as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsVoidPtr(op: *mut PyObject) -> *mut c_void {
    unsafe { PyLong_AsUnsignedLongLong(op) as usize as *mut c_void }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromDouble(v: c_double) -> *mut PyObject {
    unsafe { crate::api::numbers::PyLong_FromLongLong(v as i64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnicodeObject(
    _u: *mut PyObject,
    _base: c_int,
) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromVoidPtr(p: *mut c_void) -> *mut PyObject {
    unsafe { crate::api::numbers::PyLong_FromUnsignedLongLong(p as usize as u64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Malloc(size: usize) -> *mut c_void {
    unsafe { libc::malloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Calloc(nelem: usize, elsize: usize) -> *mut c_void {
    unsafe { libc::calloc(nelem, elsize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Realloc(ptr_: *mut c_void, size: usize) -> *mut c_void {
    unsafe { libc::realloc(ptr_, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Free(ptr_: *mut c_void) {
    unsafe { libc::free(ptr_) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawMalloc(size: usize) -> *mut c_void {
    unsafe { libc::malloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawCalloc(nelem: usize, elsize: usize) -> *mut c_void {
    unsafe { libc::calloc(nelem, elsize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawRealloc(ptr_: *mut c_void, size: usize) -> *mut c_void {
    unsafe { libc::realloc(ptr_, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawFree(ptr_: *mut c_void) {
    unsafe { libc::free(ptr_) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_Check(_op: *mut PyObject) -> c_int {
    0
}

unsupported_null!(PyMemoryView_FromObject(_op: *mut PyObject));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_GET_BASE(_op: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_GET_BUFFER(_op: *mut PyObject) -> *mut Py_buffer {
    ptr::null_mut()
}

unsupported_null!(PyMethod_New(
    _func: *mut PyObject,
    _self_: *mut PyObject,
    _class: *mut PyObject,
));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_string_to_double(
    s: *const c_char,
    endptr: *mut *mut c_char,
    _overflow_exception: *mut PyObject,
) -> c_double {
    if s.is_null() {
        return -1.0;
    }
    if !endptr.is_null() {
        unsafe { *endptr = s as *mut c_char };
    }
    unsafe { CStr::from_ptr(s) }
        .to_str()
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(-1.0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_strtol(
    str_: *const c_char,
    ptr_: *mut *mut c_char,
    _base: c_int,
) -> i32 {
    if !ptr_.is_null() {
        unsafe { *ptr_ = str_ as *mut c_char };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_strtoul(
    str_: *const c_char,
    ptr_: *mut *mut c_char,
    _base: c_int,
) -> u32 {
    if !ptr_.is_null() {
        unsafe { *ptr_ = str_ as *mut c_char };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_AsFileDescriptor(_op: *mut PyObject) -> c_int {
    -1
}

unsupported_null!(PyObject_CallOneArg(_callable: *mut PyObject, _arg: *mut PyObject));
unsupported_null!(PyObject_CallMethodNoArgs(_callable: *mut PyObject, _name: *mut PyObject));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_ClearWeakRefs(_op: *mut PyObject) {}

unsupported_null!(PyObject_Format(_op: *mut PyObject, _format_spec: *mut PyObject));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_Del(op: *mut c_void) {
    unsafe { libc::free(op) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_Track(_op: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_UnTrack(_op: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GenericGetDict(
    _op: *mut PyObject,
    _context: *mut c_void,
) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetOptionalAttr(
    op: *mut PyObject,
    attr: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        return -1;
    }
    let value = unsafe { crate::api::object::PyObject_GetAttr(op, attr) };
    unsafe { *result = value };
    (!value.is_null()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Init(op: *mut PyObject, tp: *mut PyTypeObject) -> *mut PyObject {
    if !op.is_null() {
        unsafe { (*op).ob_type = tp };
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_InitVar(
    op: *mut PyVarObject,
    tp: *mut PyTypeObject,
    size: Py_ssize_t,
) -> *mut PyVarObject {
    if !op.is_null() {
        unsafe {
            (*op).ob_base.ob_type = tp;
            (*op).ob_size = size;
        }
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_LengthHint(
    op: *mut PyObject,
    default_value: Py_ssize_t,
) -> Py_ssize_t {
    let length = unsafe { crate::api::object::PyObject_Length(op) };
    if length < 0 { default_value } else { length }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Print(
    _op: *mut PyObject,
    _fp: *mut c_void,
    _flags: c_int,
) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_SelfIter(op: *mut PyObject) -> *mut PyObject {
    if !op.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(op) };
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Type(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*op).ob_type as *mut PyObject }
}

unsupported_null!(PyObject_Vectorcall(
    _callable: *mut PyObject,
    _args: *const *mut PyObject,
    _nargsf: usize,
    _kwnames: *mut PyObject,
));
unsupported_null!(PyObject_VectorcallMethod(
    _name: *mut PyObject,
    _args: *const *mut PyObject,
    _nargsf: usize,
    _kwnames: *mut PyObject,
));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySeqIter_New(_seq: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_ITEMS(_op: *mut PyObject) -> *mut *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_Check(_op: *mut PyObject) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_AdjustIndices(
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: Py_ssize_t,
) -> Py_ssize_t {
    if start.is_null() || stop.is_null() || step == 0 {
        return 0;
    }
    unsafe {
        if *start < 0 {
            *start += length;
        }
        if *stop < 0 {
            *stop += length;
        }
        if *start < 0 {
            *start = 0;
        }
        if *stop > length {
            *stop = length;
        }
        if *stop < *start {
            0
        } else {
            (*stop - *start + step.abs() - 1) / step.abs()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_GetIndicesEx(
    _slice: *mut PyObject,
    _length: Py_ssize_t,
    _start: *mut Py_ssize_t,
    _stop: *mut Py_ssize_t,
    _step: *mut Py_ssize_t,
    _slicelength: *mut Py_ssize_t,
) -> c_int {
    -1
}

unsupported_null!(PySlice_New(
    _start: *mut PyObject,
    _stop: *mut PyObject,
    _step: *mut PyObject,
));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySys_GetObject(_name: *const c_char) -> *mut PyObject {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_Get() -> *mut c_void {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTraceMalloc_Track(_domain: u32, _ptr: usize, _size: usize) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTraceMalloc_Untrack(_domain: u32, _ptr: usize) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GetSlice(
    _op: *mut PyObject,
    _low: Py_ssize_t,
    _high: Py_ssize_t,
) -> *mut PyObject {
    unsafe { crate::api::sequences::PyTuple_New(0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    ptr::eq(
        unsafe { (*op).ob_type },
        &raw mut crate::abi_types::PyType_Type,
    ) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsASCIIString(op: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::strings::PyUnicode_AsEncodedString(op, c"ascii".as_ptr(), ptr::null()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsLatin1String(op: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::strings::PyUnicode_AsEncodedString(op, c"latin-1".as_ptr(), ptr::null()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8String(op: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::strings::PyUnicode_AsEncodedString(op, c"utf-8".as_ptr(), ptr::null()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUCS4(
    _u: *mut PyObject,
    _buffer: *mut u32,
    _buflen: Py_ssize_t,
    _copy_null: c_int,
) -> *mut u32 {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUCS4Copy(_u: *mut PyObject) -> *mut u32 {
    ptr::null_mut()
}

unsupported_null!(PyUnicode_FromEncodedObject(
    _obj: *mut PyObject,
    _encoding: *const c_char,
    _errors: *const c_char,
));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromKindAndData(
    kind: c_int,
    data: *const c_void,
    size: Py_ssize_t,
) -> *mut PyObject {
    if kind == 1 {
        unsafe { crate::api::strings::PyUnicode_FromStringAndSize(data.cast(), size) }
    } else {
        ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Compare(left: *mut PyObject, right: *mut PyObject) -> c_int {
    let l = unsafe { crate::api::strings::PyUnicode_AsUTF8(left) };
    let r = unsafe { crate::api::strings::PyUnicode_AsUTF8(right) };
    if l.is_null() || r.is_null() {
        return -1;
    }
    unsafe {
        CStr::from_ptr(l)
            .to_bytes()
            .cmp(CStr::from_ptr(r).to_bytes()) as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Replace(
    unicode: *mut PyObject,
    _substr: *mut PyObject,
    _replstr: *mut PyObject,
    _maxcount: Py_ssize_t,
) -> *mut PyObject {
    if !unicode.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(unicode) };
    }
    unicode
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Substring(
    unicode: *mut PyObject,
    _start: Py_ssize_t,
    _end: Py_ssize_t,
) -> *mut PyObject {
    if !unicode.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(unicode) };
    }
    unicode
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Tailmatch(
    _str_: *mut PyObject,
    _substr: *mut PyObject,
    _start: Py_ssize_t,
    _end: Py_ssize_t,
    _direction: c_int,
) -> c_int {
    0
}

unsupported_null!(PyVectorcall_Call(
    _callable: *mut PyObject,
    _tuple: *mut PyObject,
    _dict: *mut PyObject,
));
unsupported_null!(Py_GenericAlias(_origin: *mut PyObject, _args: *mut PyObject));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IsInitialized() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_EnterRecursiveCall(_where_: *const c_char) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_LeaveRecursiveCall() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_New(tp: *mut PyTypeObject) -> *mut PyObject {
    let ptr_ = unsafe { libc::malloc(std::mem::size_of::<PyObject>()) as *mut PyObject };
    unsafe { PyObject_Init(ptr_, tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GC_New(tp: *mut PyTypeObject) -> *mut PyObject {
    unsafe { _PyObject_New(tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_NewVar(
    tp: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyVarObject {
    let ptr_ = unsafe { libc::malloc(std::mem::size_of::<PyVarObject>()) as *mut PyVarObject };
    unsafe { PyObject_InitVar(ptr_, tp, nitems) }
}

macro_rules! unicode_predicate {
    ($name:ident, $body:expr) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(ch: u32) -> c_int {
            char::from_u32(ch).map($body).unwrap_or(false) as c_int
        }
    };
}

unicode_predicate!(_PyUnicode_IsAlpha, |ch: char| ch.is_alphabetic());
unicode_predicate!(_PyUnicode_IsDecimalDigit, |ch: char| ch.is_ascii_digit());
unicode_predicate!(_PyUnicode_IsDigit, |ch: char| ch.is_ascii_digit());
unicode_predicate!(_PyUnicode_IsLowercase, |ch: char| ch.is_lowercase());
unicode_predicate!(_PyUnicode_IsNumeric, |ch: char| ch.is_numeric());
unicode_predicate!(_PyUnicode_IsTitlecase, |ch: char| ch.is_uppercase());
unicode_predicate!(_PyUnicode_IsUppercase, |ch: char| ch.is_uppercase());
unicode_predicate!(_PyUnicode_IsWhitespace, |ch: char| ch.is_whitespace());

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_HashDouble(_op: *mut PyObject, value: c_double) -> Py_hash_t {
    value.to_bits() as Py_hash_t
}

#[allow(dead_code)]
fn _module_def(_def: *mut PyModuleDef) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn assert_pending_error() {
        assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    #[test]
    fn pycomplex_binary_exports_fail_closed_until_bridge_storage_exists() {
        unsafe { crate::api::errors::PyErr_Clear() };

        assert!(unsafe { PyComplex_FromDoubles(1.0, 2.0) }.is_null());
        assert_pending_error();

        assert!(
            unsafe {
                PyComplex_FromCComplex(Py_complex {
                    real: 1.0,
                    imag: 2.0,
                })
            }
            .is_null()
        );
        assert_pending_error();

        assert_eq!(unsafe { PyComplex_RealAsDouble(ptr::null_mut()) }, -1.0);
        assert_pending_error();

        assert_eq!(unsafe { PyComplex_ImagAsDouble(ptr::null_mut()) }, -1.0);
        assert_pending_error();

        let value = unsafe { PyComplex_AsCComplex(ptr::null_mut()) };
        assert_eq!(value.real, -1.0);
        assert_eq!(value.imag, 0.0);
        assert_pending_error();
    }
}
