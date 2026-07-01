//! Object protocol — PyObject_* generic operations.
//!
//! These are the abstract object protocol functions that work on any
//! PyObject regardless of type. They delegate to type-specific slots
//! (tp_repr, tp_hash, tp_getattro, etc.) when available, falling back
//! to reasonable defaults.

use crate::abi_types::{
    _PyErr_StackItem, METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, METH_O, METH_VARARGS,
    Py_False, Py_None, Py_True, Py_ssize_t, PyCFunction, PyCFunctionFast,
    PyCFunctionFastWithKeywords, PyCFunctionObject, PyCFunctionWithKeywords, PyCodeObject,
    PyFrameObject, PyGenericAliasObject, PyInterpreterState, PyMethodDef, PyMethodObject, PyMutex,
    PyObject, PyThreadState, PyTypeObject,
};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

type VisitProc = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

// ─── Attribute access ─────────────────────────────────────────────────────

unsafe fn bridge_get_attr_from_name_bits(o: *mut PyObject, name_bits: u64) -> *mut PyObject {
    let obj_bits = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.pyobj_to_handle(o)
    };
    let Some(obj_bits) = obj_bits else {
        return ptr::null_mut();
    };
    let bits = unsafe { (hooks_or_stubs().object_get_attr)(obj_bits, name_bits) };
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetAttr(
    o: *mut PyObject,
    attr_name: *mut PyObject,
) -> *mut PyObject {
    if o.is_null() || attr_name.is_null() {
        return ptr::null_mut();
    }
    let name_bits = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.pyobj_to_handle(attr_name)
    };
    if let Some(name_bits) = name_bits {
        let result = unsafe { bridge_get_attr_from_name_bits(o, name_bits) };
        if !result.is_null() {
            return result;
        }
    }
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null()
        && let Some(getattro) = unsafe { (*tp).tp_getattro }
    {
        return unsafe { getattro(o, attr_name) };
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetAttrString(
    o: *mut PyObject,
    attr_name: *const c_char,
) -> *mut PyObject {
    if o.is_null() || attr_name.is_null() {
        return ptr::null_mut();
    }
    let obj_bits = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.pyobj_to_handle(o)
    };
    if obj_bits.is_some() {
        let name_bytes = unsafe { std::ffi::CStr::from_ptr(attr_name) }.to_bytes();
        let hooks = hooks_or_stubs();
        let name_bits = unsafe { (hooks.alloc_str)(name_bytes.as_ptr(), name_bytes.len()) };
        if name_bits != 0 {
            let result = unsafe { bridge_get_attr_from_name_bits(o, name_bits) };
            unsafe { (hooks.dec_ref)(name_bits) };
            if !result.is_null() {
                return result;
            }
        }
    }
    // Try tp_getattr (char*-based) first, then tp_getattro (PyObject*-based).
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null() {
        if let Some(getattr) = unsafe { (*tp).tp_getattr } {
            return unsafe { getattr(o, attr_name) };
        }
        if let Some(getattro) = unsafe { (*tp).tp_getattro } {
            let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(attr_name) };
            if name_obj.is_null() {
                return ptr::null_mut();
            }
            let result = unsafe { getattro(o, name_obj) };
            unsafe { crate::api::refcount::Py_DECREF(name_obj) };
            return result;
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_SetAttr(
    o: *mut PyObject,
    attr_name: *mut PyObject,
    v: *mut PyObject,
) -> c_int {
    if o.is_null() || attr_name.is_null() {
        return -1;
    }
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null()
        && let Some(setattro) = unsafe { (*tp).tp_setattro }
    {
        return unsafe { setattro(o, attr_name, v) };
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_SetAttrString(
    o: *mut PyObject,
    attr_name: *const c_char,
    v: *mut PyObject,
) -> c_int {
    if o.is_null() || attr_name.is_null() {
        return -1;
    }
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null() {
        if let Some(setattr) = unsafe { (*tp).tp_setattr } {
            return unsafe { setattr(o, attr_name, v) };
        }
        if let Some(setattro) = unsafe { (*tp).tp_setattro } {
            let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(attr_name) };
            if name_obj.is_null() {
                return -1;
            }
            let result = unsafe { setattro(o, name_obj, v) };
            unsafe { crate::api::refcount::Py_DECREF(name_obj) };
            return result;
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_HasAttr(o: *mut PyObject, attr_name: *mut PyObject) -> c_int {
    let result = unsafe { PyObject_GetAttr(o, attr_name) };
    if result.is_null() {
        unsafe { crate::api::errors::PyErr_Clear() };
        0
    } else {
        unsafe { crate::api::refcount::Py_DECREF(result) };
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetOptionalAttr(
    o: *mut PyObject,
    attr_name: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyObject_GetOptionalAttr result pointer is NULL".as_ptr(),
            );
        }
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    let attr = unsafe { PyObject_GetAttr(o, attr_name) };
    if attr.is_null() {
        unsafe { crate::api::errors::PyErr_Clear() };
        return 0;
    }
    unsafe {
        *result = attr;
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_LookupAttr(
    o: *mut PyObject,
    attr_name: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    unsafe { PyObject_GetOptionalAttr(o, attr_name, result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetOptionalAttrString(
    o: *mut PyObject,
    attr_name: *const c_char,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyObject_GetOptionalAttrString result pointer is NULL".as_ptr(),
            );
        }
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if attr_name.is_null() {
        return -1;
    }
    let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(attr_name) };
    if name_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyObject_GetOptionalAttr(o, name_obj, result) };
    unsafe { crate::api::refcount::Py_DECREF(name_obj) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_HasAttrString(
    o: *mut PyObject,
    attr_name: *const c_char,
) -> c_int {
    let result = unsafe { PyObject_GetAttrString(o, attr_name) };
    if result.is_null() {
        unsafe { crate::api::errors::PyErr_Clear() };
        0
    } else {
        unsafe { crate::api::refcount::Py_DECREF(result) };
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_HasAttrWithError(
    o: *mut PyObject,
    attr_name: *mut PyObject,
) -> c_int {
    let mut result = ptr::null_mut();
    let rc = unsafe { PyObject_GetOptionalAttr(o, attr_name, &raw mut result) };
    if rc > 0 {
        unsafe { crate::api::refcount::Py_DECREF(result) };
        1
    } else {
        rc
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_HasAttrStringWithError(
    o: *mut PyObject,
    attr_name: *const c_char,
) -> c_int {
    let mut result = ptr::null_mut();
    let rc = unsafe { PyObject_GetOptionalAttrString(o, attr_name, &raw mut result) };
    if rc > 0 {
        unsafe { crate::api::refcount::Py_DECREF(result) };
        1
    } else {
        rc
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GenericGetAttr(
    o: *mut PyObject,
    name: *mut PyObject,
) -> *mut PyObject {
    // Minimal implementation: check tp_dict on the type.
    if o.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return ptr::null_mut();
    }
    let tp_dict = unsafe { (*tp).tp_dict };
    if !tp_dict.is_null() {
        let result = unsafe { crate::api::mapping::PyDict_GetItem(tp_dict, name) };
        if !result.is_null() {
            unsafe { crate::api::refcount::Py_INCREF(result) };
            return result;
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GenericGetAttrWithDict(
    o: *mut PyObject,
    name: *mut PyObject,
    dict: *mut PyObject,
    suppress: c_int,
) -> *mut PyObject {
    if o.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    if !dict.is_null() {
        let result = unsafe { crate::api::mapping::PyDict_GetItem(dict, name) };
        if !result.is_null() {
            unsafe { crate::api::refcount::Py_INCREF(result) };
            return result;
        }
    }
    let result = unsafe { PyObject_GenericGetAttr(o, name) };
    if result.is_null() && suppress != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GenericGetDict(
    o: *mut PyObject,
    _context: *mut c_void,
) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return ptr::null_mut();
    }
    let offset = unsafe { (*tp).tp_dictoffset };
    if offset <= 0 {
        return ptr::null_mut();
    }
    let slot = unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>();
    let dict = unsafe { *slot };
    if dict.is_null() {
        let new_dict = unsafe { crate::api::mapping::PyDict_New() };
        if new_dict.is_null() {
            return ptr::null_mut();
        }
        unsafe {
            *slot = new_dict;
            crate::api::refcount::Py_INCREF(new_dict);
        }
        return new_dict;
    }
    unsafe { crate::api::refcount::Py_INCREF(dict) };
    dict
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GenericSetDict(
    o: *mut PyObject,
    value: *mut PyObject,
    _context: *mut c_void,
) -> c_int {
    if o.is_null() || value.is_null() {
        return -1;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return -1;
    }
    let offset = unsafe { (*tp).tp_dictoffset };
    if offset <= 0 {
        return -1;
    }
    let slot = unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>();
    unsafe {
        crate::api::refcount::Py_INCREF(value);
        let old = *slot;
        *slot = value;
        crate::api::refcount::Py_XDECREF(old);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GetDictPtr(o: *mut PyObject) -> *mut *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return ptr::null_mut();
    }
    let offset = unsafe { (*tp).tp_dictoffset };
    if offset <= 0 {
        return ptr::null_mut();
    }
    unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_ClearManagedDict(o: *mut PyObject) {
    if o.is_null() {
        return;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return;
    }
    let offset = unsafe { (*tp).tp_dictoffset };
    if offset <= 0 {
        return;
    }
    let slot = unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>();
    unsafe {
        let old = *slot;
        *slot = ptr::null_mut();
        crate::api::refcount::Py_XDECREF(old);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_VisitManagedDict(
    o: *mut PyObject,
    visit: Option<VisitProc>,
    arg: *mut c_void,
) -> c_int {
    if o.is_null() {
        return 0;
    }
    let Some(visit) = visit else {
        return 0;
    };
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return 0;
    }
    let offset = unsafe { (*tp).tp_dictoffset };
    if offset <= 0 {
        return 0;
    }
    let slot = unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>();
    let dict = unsafe { *slot };
    if dict.is_null() {
        0
    } else {
        unsafe { visit(dict, arg) }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_ClearWeakRefs(_o: *mut PyObject) {}

fn new_code_object(first_traceable: c_int) -> *mut PyCodeObject {
    let obj = Box::new(PyCodeObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyBaseObject_Type,
        },
        _co_firsttraceable: first_traceable,
    });
    Box::into_raw(obj)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCode_NewEmpty(
    _filename: *const c_char,
    _funcname: *const c_char,
    _firstlineno: c_int,
) -> *mut PyCodeObject {
    new_code_object(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Code_NewWithPosOnlyArgs(
    _argcount: c_int,
    _posonlyargcount: c_int,
    _kwonlyargcount: c_int,
    _nlocals: c_int,
    _stacksize: c_int,
    _flags: c_int,
    _code: *mut PyObject,
    _consts: *mut PyObject,
    _names: *mut PyObject,
    _varnames: *mut PyObject,
    _freevars: *mut PyObject,
    _cellvars: *mut PyObject,
    _filename: *mut PyObject,
    _name: *mut PyObject,
    _qualname: *mut PyObject,
    _firstlineno: c_int,
    _linetable: *mut PyObject,
    _exceptiontable: *mut PyObject,
) -> *mut PyCodeObject {
    new_code_object(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Object_IsUniquelyReferenced(obj: *mut PyObject) -> c_int {
    if obj.is_null() {
        return 0;
    }
    unsafe { ((*obj).ob_refcnt == 1) as c_int }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Object_IsUniqueReferencedTemporary(
    obj: *mut PyObject,
) -> c_int {
    unsafe { PyUnstable_Object_IsUniquelyReferenced(obj) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Object_EnableDeferredRefcount(_obj: *mut PyObject) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_SetImmortal(obj: *mut PyObject) {
    if obj.is_null() {
        return;
    }
    unsafe {
        (*obj).ob_refcnt = 1 << 30;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_IsOwnedByCurrentThread(_obj: *mut PyObject) -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrame_New(
    _tstate: *mut PyThreadState,
    _code: *mut PyCodeObject,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
) -> *mut PyFrameObject {
    let obj = Box::new(PyFrameObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyBaseObject_Type,
        },
        f_back: ptr::null_mut(),
        f_code: _code,
        f_globals: _globals,
        f_locals: _locals,
        f_lineno: 0,
    });
    Box::into_raw(obj)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrame_GetCode(frame: *mut PyFrameObject) -> *mut PyCodeObject {
    if frame.is_null() {
        return ptr::null_mut();
    }
    let code = unsafe { (*frame).f_code };
    if code.is_null() {
        return new_code_object(0);
    }
    unsafe { crate::api::refcount::Py_INCREF(code.cast::<PyObject>()) };
    code
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrame_GetBack(frame: *mut PyFrameObject) -> *mut PyFrameObject {
    if frame.is_null() {
        return ptr::null_mut();
    }
    let back = unsafe { (*frame).f_back };
    if !back.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(back.cast::<PyObject>()) };
    }
    back
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_GetFrame(tstate: *mut PyThreadState) -> *mut PyFrameObject {
    if tstate.is_null() {
        return ptr::null_mut();
    }
    unsafe { PyFrame_New(tstate, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTraceBack_Here(_frame: *mut PyFrameObject) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GenericSetAttr(
    o: *mut PyObject,
    name: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    if o.is_null() || name.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"NULL argument to PyObject_GenericSetAttr".as_ptr(),
            );
        }
        return -1;
    }
    let (obj_bits, name_bits, value_bits) = {
        let bridge = GLOBAL_BRIDGE.lock();
        (
            bridge.pyobj_to_handle(o),
            bridge.pyobj_to_handle(name),
            if value.is_null() {
                None
            } else {
                bridge.pyobj_to_handle(value)
            },
        )
    };
    let Some(obj_bits) = obj_bits else {
        return -1;
    };
    let Some(name_bits) = name_bits else {
        return -1;
    };
    let value_bits = value_bits.unwrap_or(0);
    unsafe { (hooks_or_stubs().object_set_attr)(obj_bits, name_bits, value_bits) }
}

// ─── Truthiness / identity ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsTrue(o: *mut PyObject) -> c_int {
    if o.is_null() {
        return 0;
    }
    // Singletons
    if std::ptr::eq(o, &raw mut Py_True) {
        return 1;
    }
    if std::ptr::eq(o, &raw mut Py_False) || std::ptr::eq(o, &raw mut Py_None) {
        return 0;
    }
    // Bridge to Molt object
    let bridge = GLOBAL_BRIDGE.lock();
    match bridge.pyobj_to_handle(o) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            if obj.is_none() {
                0
            } else if obj.is_bool() {
                obj.as_bool().unwrap_or(false) as c_int
            } else if obj.is_int() {
                (obj.as_int().unwrap_or(0) != 0) as c_int
            } else if obj.is_float() {
                (obj.as_float().unwrap_or(0.0) != 0.0) as c_int
            } else {
                // Default: non-null object is truthy
                1
            }
        }
        None => 1, // unknown non-null object is truthy
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Print(
    o: *mut PyObject,
    fp: *mut libc::FILE,
    _flags: c_int,
) -> c_int {
    if o.is_null() || fp.is_null() {
        return -1;
    }
    let rendered = unsafe { crate::api::typeobj::PyObject_Str(o) };
    if rendered.is_null() {
        return -1;
    }
    let text = unsafe { crate::api::strings::PyUnicode_AsUTF8(rendered) };
    if text.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(rendered) };
        return -1;
    }
    let rc = unsafe { libc::fputs(text, fp) };
    unsafe { crate::api::refcount::Py_DECREF(rendered) };
    if rc < 0 { -1 } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Format(
    o: *mut PyObject,
    format_spec: *mut PyObject,
) -> *mut PyObject {
    if o.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"PyObject_Format requires non-NULL object".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let mut owned_empty_spec = ptr::null_mut();
    let spec = if format_spec.is_null() {
        owned_empty_spec =
            unsafe { crate::api::strings::PyUnicode_FromStringAndSize(c"".as_ptr(), 0) };
        if owned_empty_spec.is_null() {
            return ptr::null_mut();
        }
        owned_empty_spec
    } else {
        format_spec
    };

    if !format_spec.is_null() && unsafe { crate::api::strings::PyUnicode_Check(spec) } == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"format_spec must be a str".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let (obj_bits, spec_bits) = {
        let bridge = GLOBAL_BRIDGE.lock();
        (bridge.pyobj_to_handle(o), bridge.pyobj_to_handle(spec))
    };
    if let (Some(obj_bits), Some(spec_bits)) = (obj_bits, spec_bits) {
        let out_bits = unsafe { (hooks_or_stubs().object_format)(obj_bits, spec_bits) };
        if out_bits != 0 {
            let out = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(out_bits) };
            if !owned_empty_spec.is_null() {
                unsafe { crate::api::refcount::Py_DECREF(owned_empty_spec) };
            }
            return out;
        }
    }

    let spec_is_empty = if format_spec.is_null() {
        true
    } else {
        (unsafe { crate::api::strings::PyUnicode_GetLength(spec) }) == 0
    };
    let out = if spec_is_empty {
        unsafe { crate::api::typeobj::PyObject_Str(o) }
    } else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"unsupported format string passed to object.__format__".as_ptr(),
            );
        }
        ptr::null_mut()
    };
    if !owned_empty_spec.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(owned_empty_spec) };
    }
    out
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Not(o: *mut PyObject) -> c_int {
    let truthy = unsafe { PyObject_IsTrue(o) };
    if truthy < 0 {
        -1
    } else {
        (truthy == 0) as c_int
    }
}

// ─── Length ───────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Length(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PyObject_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Size(o: *mut PyObject) -> Py_ssize_t {
    if o.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(o) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let obj = MoltObject::from_bits(bits);

    // Try list length
    if obj.is_ptr() {
        let tag = unsafe { (h.classify_heap)(bits) };
        match tag {
            t if t == crate::abi_types::MoltTypeTag::List as u8 => {
                return unsafe { (h.list_len)(bits) as Py_ssize_t };
            }
            t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
                return unsafe { (h.tuple_len)(bits) as Py_ssize_t };
            }
            t if t == crate::abi_types::MoltTypeTag::Dict as u8 => {
                return unsafe { (h.dict_len)(bits) as Py_ssize_t };
            }
            t if t == crate::abi_types::MoltTypeTag::Str as u8 => {
                let mut len: usize = 0;
                unsafe { (h.str_data)(bits, &raw mut len) };
                return len as Py_ssize_t;
            }
            t if t == crate::abi_types::MoltTypeTag::Bytes as u8 => {
                let mut len: usize = 0;
                unsafe { (h.bytes_data)(bits, &raw mut len) };
                return len as Py_ssize_t;
            }
            _ => {}
        }
    }
    -1
}

// ─── Item access (mapping/sequence protocol) ──────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_LengthHint(
    o: *mut PyObject,
    defaultvalue: Py_ssize_t,
) -> Py_ssize_t {
    let size = unsafe { PyObject_Size(o) };
    if size < 0 { defaultvalue } else { size }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetItem(o: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    if o.is_null() || key.is_null() {
        return ptr::null_mut();
    }
    // Try dict first
    let bridge = GLOBAL_BRIDGE.lock();
    let o_bits = match bridge.pyobj_to_handle(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);

    let h = hooks_or_stubs();
    let obj = MoltObject::from_bits(o_bits);

    if obj.is_ptr() {
        let tag = unsafe { (h.classify_heap)(o_bits) };
        // Dict: use dict_get
        if tag == crate::abi_types::MoltTypeTag::Dict as u8 {
            let bridge2 = GLOBAL_BRIDGE.lock();
            let key_bits = match bridge2.pyobj_to_handle(key) {
                Some(b) => b,
                None => return ptr::null_mut(),
            };
            drop(bridge2);
            let val_bits = unsafe { (h.dict_get)(o_bits, key_bits) };
            if val_bits == 0 {
                return ptr::null_mut();
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(val_bits) };
        }
        // List: use list_item with int key
        if tag == crate::abi_types::MoltTypeTag::List as u8 {
            let bridge2 = GLOBAL_BRIDGE.lock();
            let key_bits = match bridge2.pyobj_to_handle(key) {
                Some(b) => b,
                None => return ptr::null_mut(),
            };
            drop(bridge2);
            let key_obj = MoltObject::from_bits(key_bits);
            if let Some(idx) = key_obj.as_int() {
                let len = unsafe { (h.list_len)(o_bits) };
                let actual_idx = if idx < 0 { len as i64 + idx } else { idx };
                if actual_idx < 0 || actual_idx >= len as i64 {
                    return ptr::null_mut();
                }
                let item_bits = unsafe { (h.list_item)(o_bits, actual_idx as usize) };
                if item_bits == 0 {
                    return ptr::null_mut();
                }
                return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) };
            }
        }
        // Tuple: use tuple_item with int key
        if tag == crate::abi_types::MoltTypeTag::Tuple as u8 {
            let bridge2 = GLOBAL_BRIDGE.lock();
            let key_bits = match bridge2.pyobj_to_handle(key) {
                Some(b) => b,
                None => return ptr::null_mut(),
            };
            drop(bridge2);
            let key_obj = MoltObject::from_bits(key_bits);
            if let Some(idx) = key_obj.as_int() {
                let len = unsafe { (h.tuple_len)(o_bits) };
                let actual_idx = if idx < 0 { len as i64 + idx } else { idx };
                if actual_idx < 0 || actual_idx >= len as i64 {
                    return ptr::null_mut();
                }
                let item_bits = unsafe { (h.tuple_item)(o_bits, actual_idx as usize) };
                if item_bits == 0 {
                    return ptr::null_mut();
                }
                return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) };
            }
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_SetItem(
    o: *mut PyObject,
    key: *mut PyObject,
    v: *mut PyObject,
) -> c_int {
    if o.is_null() || key.is_null() || v.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let o_bits = match bridge.pyobj_to_handle(o) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);

    let h = hooks_or_stubs();
    let obj = MoltObject::from_bits(o_bits);

    if obj.is_ptr() {
        let tag = unsafe { (h.classify_heap)(o_bits) };
        if tag == crate::abi_types::MoltTypeTag::Dict as u8 {
            let bridge2 = GLOBAL_BRIDGE.lock();
            let key_bits = match bridge2.pyobj_to_handle(key) {
                Some(b) => b,
                None => return -1,
            };
            let val_bits = match bridge2.pyobj_to_handle(v) {
                Some(b) => b,
                None => return -1,
            };
            drop(bridge2);
            unsafe { (h.dict_set)(o_bits, key_bits, val_bits) };
            return 0;
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_DelItem(o: *mut PyObject, key: *mut PyObject) -> c_int {
    if o.is_null() || key.is_null() {
        return -1;
    }
    // For dicts: set the key to None as a deletion sentinel.
    let bridge = GLOBAL_BRIDGE.lock();
    let o_bits = match bridge.pyobj_to_handle(o) {
        Some(b) => b,
        None => return -1,
    };
    let key_bits = match bridge.pyobj_to_handle(key) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);

    let h = hooks_or_stubs();
    let obj = MoltObject::from_bits(o_bits);
    if obj.is_ptr() {
        let tag = unsafe { (h.classify_heap)(o_bits) };
        if tag == crate::abi_types::MoltTypeTag::Dict as u8 {
            let none_bits = MoltObject::none().bits();
            unsafe { (h.dict_set)(o_bits, key_bits, none_bits) };
            return 0;
        }
    }
    -1
}

// ─── Iterator protocol ───────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetIter(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null()
        && let Some(iter_fn) = unsafe { (*tp).tp_iter }
    {
        return unsafe { iter_fn(o) };
    }
    // For sequences, return the object itself as a pseudo-iterator.
    // Real iteration would require a proper iterator wrapper.
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyIter_Check(o: *mut PyObject) -> c_int {
    if o.is_null() {
        return 0;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return 0;
    }
    unsafe { (*tp).tp_iternext.is_some() as c_int }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyIter_Next(iter: *mut PyObject) -> *mut PyObject {
    if iter.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*iter).ob_type };
    if !tp.is_null()
        && let Some(iternext) = unsafe { (*tp).tp_iternext }
    {
        return unsafe { iternext(iter) };
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Next(iter: *mut PyObject) -> *mut PyObject {
    unsafe { PyIter_Next(iter) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_SelfIter(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(o) };
    o
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySeqIter_New(seq: *mut PyObject) -> *mut PyObject {
    unsafe { PyObject_GetIter(seq) }
}

// ─── Dir ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Dir(o: *mut PyObject) -> *mut PyObject {
    // Return an empty list — full dir() requires introspecting the MRO.
    let _ = o;
    unsafe { crate::api::sequences::PyList_New(0) }
}

// ─── Call protocol ────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    if callable.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*callable).ob_type };
    if !tp.is_null()
        && let Some(call) = unsafe { (*tp).tp_call }
    {
        return unsafe { call(callable, args, kwargs) };
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallObject(
    callable: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyObject_Call(callable, args, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallNoArgs(callable: *mut PyObject) -> *mut PyObject {
    let empty_tuple = unsafe { crate::api::sequences::PyTuple_New(0) };
    let result = unsafe { PyObject_Call(callable, empty_tuple, ptr::null_mut()) };
    unsafe { crate::api::refcount::Py_DECREF(empty_tuple) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallOneArg(
    callable: *mut PyObject,
    arg: *mut PyObject,
) -> *mut PyObject {
    if callable.is_null() || arg.is_null() {
        return ptr::null_mut();
    }
    let tuple = unsafe { crate::api::sequences::PyTuple_New(1) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(arg) };
    if unsafe { crate::api::sequences::PyTuple_SetItem(tuple, 0, arg) } != 0 {
        unsafe {
            crate::api::refcount::Py_DECREF(arg);
            crate::api::refcount::Py_DECREF(tuple);
        }
        return ptr::null_mut();
    }
    let result = unsafe { PyObject_Call(callable, tuple, ptr::null_mut()) };
    unsafe { crate::api::refcount::Py_DECREF(tuple) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallMethodNoArgs(
    obj: *mut PyObject,
    name: *mut PyObject,
) -> *mut PyObject {
    if obj.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let method = unsafe { PyObject_GetAttr(obj, name) };
    if method.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyObject_CallNoArgs(method) };
    unsafe { crate::api::refcount::Py_DECREF(method) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallMethodOneArg(
    obj: *mut PyObject,
    name: *mut PyObject,
    arg: *mut PyObject,
) -> *mut PyObject {
    if obj.is_null() || name.is_null() || arg.is_null() {
        return ptr::null_mut();
    }
    let method = unsafe { PyObject_GetAttr(obj, name) };
    if method.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyObject_CallOneArg(method, arg) };
    unsafe { crate::api::refcount::Py_DECREF(method) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_GenericAlias(
    origin: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    if origin.is_null() || args.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"Py_GenericAlias origin and args must not be NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe {
        crate::api::refcount::Py_INCREF(origin);
        crate::api::refcount::Py_INCREF(args);
    }
    let alias = Box::new(PyGenericAliasObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::Py_GenericAliasType,
        },
        origin,
        args,
    });
    Box::into_raw(alias).cast::<PyObject>()
}

pub unsafe extern "C" fn molt_generic_alias_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let alias = op.cast::<PyGenericAliasObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*alias).origin);
        crate::api::refcount::Py_XDECREF((*alias).args);
        drop(Box::from_raw(alias));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_AsFileDescriptor(o: *mut PyObject) -> c_int {
    if o.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"argument must be an int, or have a fileno() method".as_ptr(),
            );
        }
        return -1;
    }
    if unsafe { crate::api::numbers::PyLong_Check(o) } != 0 {
        return unsafe { crate::api::numbers::PyLong_AsLong(o) as c_int };
    }

    let fileno = unsafe { PyObject_GetAttrString(o, c"fileno".as_ptr()) };
    if fileno.is_null() {
        return -1;
    }
    let result = unsafe { PyObject_CallNoArgs(fileno) };
    unsafe { crate::api::refcount::Py_DECREF(fileno) };
    if result.is_null() {
        return -1;
    }
    let fd = unsafe { crate::api::numbers::PyLong_AsLong(result) as c_int };
    unsafe { crate::api::refcount::Py_DECREF(result) };
    fd
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_HasKeyWithError(
    obj: *mut PyObject,
    key: *mut PyObject,
) -> c_int {
    let item = unsafe { PyObject_GetItem(obj, key) };
    if item.is_null() {
        unsafe { crate::api::errors::PyErr_Clear() };
        0
    } else {
        unsafe { crate::api::refcount::Py_DECREF(item) };
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_HasKeyStringWithError(
    obj: *mut PyObject,
    key: *const c_char,
) -> c_int {
    if key.is_null() {
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyMapping_HasKeyWithError(obj, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

const PY_VECTORCALL_ARGUMENTS_OFFSET: usize = 1usize << (8 * std::mem::size_of::<usize>() - 1);

fn vectorcall_nargs(nargsf: usize) -> isize {
    (nargsf & !PY_VECTORCALL_ARGUMENTS_OFFSET) as isize
}

unsafe fn tuple_from_vectorcall_args(args: *mut *mut PyObject, nargs: isize) -> *mut PyObject {
    if nargs < 0 || (nargs > 0 && args.is_null()) {
        return ptr::null_mut();
    }
    let tuple = unsafe { crate::api::sequences::PyTuple_New(nargs) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    for index in 0..nargs {
        let arg = unsafe { *args.add(index as usize) };
        if arg.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(tuple) };
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_INCREF(arg) };
        let rc = unsafe { crate::api::sequences::PyTuple_SetItem(tuple, index, arg) };
        if rc != 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(arg);
                crate::api::refcount::Py_DECREF(tuple);
            }
            return ptr::null_mut();
        }
    }
    tuple
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Vectorcall(
    callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    if !kwnames.is_null() {
        return ptr::null_mut();
    }
    let nargs = vectorcall_nargs(nargsf);
    let tuple = unsafe { tuple_from_vectorcall_args(args, nargs) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyObject_Call(callable, tuple, ptr::null_mut()) };
    unsafe { crate::api::refcount::Py_DECREF(tuple) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_Vectorcall(
    callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyObject_Vectorcall(callable, args, nargsf, kwnames) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_VectorcallDict(
    callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargs: usize,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let tuple = unsafe { tuple_from_vectorcall_args(args, nargs as isize) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyObject_Call(callable, tuple, kwargs) };
    unsafe { crate::api::refcount::Py_DECREF(tuple) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyVectorcall_Call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyObject_Call(callable, args, kwargs) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_VectorcallMethod(
    name: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    let nargs = vectorcall_nargs(nargsf);
    if name.is_null() || nargs < 1 || args.is_null() {
        return ptr::null_mut();
    }
    let receiver = unsafe { *args };
    let method = unsafe { PyObject_GetAttr(receiver, name) };
    if method.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe {
        PyObject_Vectorcall(
            method,
            args.add(1),
            (nargs as usize - 1) | (nargsf & PY_VECTORCALL_ARGUMENTS_OFFSET),
            kwnames,
        )
    };
    unsafe { crate::api::refcount::Py_DECREF(method) };
    result
}

// ─── Type queries ─────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_TYPE(op: *mut PyObject) -> *mut PyTypeObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*op).ob_type }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IS_TYPE(op: *mut PyObject, tp: *mut PyTypeObject) -> c_int {
    if op.is_null() || tp.is_null() {
        return 0;
    }
    std::ptr::eq(unsafe { (*op).ob_type }, tp) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsSubclass(derived: *mut PyObject, cls: *mut PyObject) -> c_int {
    if derived.is_null() || cls.is_null() {
        return 0;
    }
    // Pointer identity check — full MRO traversal not available.
    std::ptr::eq(derived, cls) as c_int
}

// ─── Py_NewRef / Py_XNewRef (CPython 3.10+) ──────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_NewRef(op: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::refcount::Py_INCREF(op) };
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_XNewRef(op: *mut PyObject) -> *mut PyObject {
    if !op.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(op) };
    }
    op
}

// ─── Py_RETURN helpers ────────────────────────────────────────────────────

unsafe fn tuple_arg_len(args: *mut PyObject) -> Option<Py_ssize_t> {
    if args.is_null() {
        return Some(0);
    }
    let len = unsafe { crate::api::sequences::PyTuple_Size(args) };
    if len < 0 { None } else { Some(len) }
}

unsafe fn tuple_arg_item(args: *mut PyObject, index: Py_ssize_t) -> *mut PyObject {
    if args.is_null() {
        ptr::null_mut()
    } else {
        unsafe { crate::api::sequences::PyTuple_GetItem(args, index) }
    }
}

unsafe fn tuple_arg_vec(args: *mut PyObject) -> Option<Vec<*mut PyObject>> {
    let len = unsafe { tuple_arg_len(args) }?;
    let mut items = Vec::with_capacity(len as usize);
    for index in 0..len {
        let item = unsafe { tuple_arg_item(args, index) };
        if item.is_null() {
            return None;
        }
        items.push(item);
    }
    Some(items)
}

unsafe fn prepend_bound_self(self_: *mut PyObject, args: *mut PyObject) -> Option<*mut PyObject> {
    let len = unsafe { tuple_arg_len(args) }?;
    let bound_args = unsafe { crate::api::sequences::PyTuple_New(len + 1) };
    if bound_args.is_null() {
        return None;
    }
    unsafe { crate::api::refcount::Py_INCREF(self_) };
    if unsafe { crate::api::sequences::PyTuple_SetItem(bound_args, 0, self_) } != 0 {
        unsafe {
            crate::api::refcount::Py_DECREF(self_);
            crate::api::refcount::Py_DECREF(bound_args);
        }
        return None;
    }
    for index in 0..len {
        let item = unsafe { tuple_arg_item(args, index) };
        if item.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(bound_args) };
            return None;
        }
        unsafe { crate::api::refcount::Py_INCREF(item) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(bound_args, index + 1, item) } != 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(item);
                crate::api::refcount::Py_DECREF(bound_args);
            }
            return None;
        }
    }
    Some(bound_args)
}

pub unsafe extern "C" fn molt_cfunction_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    if unsafe { PyCFunction_Check(callable) } == 0 {
        return ptr::null_mut();
    }
    let cfunc = callable.cast::<PyCFunctionObject>();
    let ml = unsafe { (*cfunc).m_ml };
    if ml.is_null() {
        return ptr::null_mut();
    }
    let raw_func = match unsafe { (*ml).ml_meth } {
        Some(func) => func,
        None => return ptr::null_mut(),
    };
    let flags = unsafe { (*ml).ml_flags };
    if flags & METH_METHOD != 0 {
        return ptr::null_mut();
    }
    let self_ = unsafe { (*cfunc).m_self };

    if flags & METH_FASTCALL != 0 {
        let mut items = match unsafe { tuple_arg_vec(args) } {
            Some(items) => items,
            None => return ptr::null_mut(),
        };
        let ptr = if items.is_empty() {
            ptr::null_mut()
        } else {
            items.as_mut_ptr()
        };
        if flags & METH_KEYWORDS != 0 {
            let func: PyCFunctionFastWithKeywords = unsafe { std::mem::transmute(raw_func) };
            return unsafe { func(self_, ptr, items.len() as Py_ssize_t, kwargs) };
        }
        if !kwargs.is_null() {
            return ptr::null_mut();
        }
        let func: PyCFunctionFast = unsafe { std::mem::transmute(raw_func) };
        return unsafe { func(self_, ptr, items.len() as Py_ssize_t) };
    }

    if flags & METH_KEYWORDS != 0 {
        let func: PyCFunctionWithKeywords = unsafe { std::mem::transmute(raw_func) };
        return unsafe { func(self_, args, kwargs) };
    }
    if !kwargs.is_null() {
        return ptr::null_mut();
    }
    if flags & METH_NOARGS != 0 {
        if unsafe { tuple_arg_len(args) } != Some(0) {
            return ptr::null_mut();
        }
        return unsafe { raw_func(self_, ptr::null_mut()) };
    }
    if flags & METH_O != 0 {
        if unsafe { tuple_arg_len(args) } != Some(1) {
            return ptr::null_mut();
        }
        let item = unsafe { tuple_arg_item(args, 0) };
        if item.is_null() {
            return ptr::null_mut();
        }
        return unsafe { raw_func(self_, item) };
    }
    if flags & METH_VARARGS != 0 {
        return unsafe { raw_func(self_, args) };
    }
    ptr::null_mut()
}

pub unsafe extern "C" fn molt_cfunction_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let cfunc = op.cast::<PyCFunctionObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*cfunc).m_self);
        crate::api::refcount::Py_XDECREF((*cfunc).m_module);
        drop(Box::from_raw(cfunc));
    }
}

pub unsafe extern "C" fn molt_method_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    if callable.is_null() {
        return ptr::null_mut();
    }
    let method = callable.cast::<PyMethodObject>();
    let func = unsafe { (*method).im_func };
    let self_ = unsafe { (*method).im_self };
    if func.is_null() {
        return ptr::null_mut();
    }
    if self_.is_null() {
        return unsafe { PyObject_Call(func, args, kwargs) };
    }
    if kwargs.is_null()
        && unsafe { PyCFunction_Check(func) } != 0
        && unsafe { tuple_arg_len(args) } == Some(0)
    {
        let cfunc = func.cast::<PyCFunctionObject>();
        let ml = unsafe { (*cfunc).m_ml };
        if !ml.is_null() {
            let flags = unsafe { (*ml).ml_flags };
            if flags & METH_O != 0
                && flags & (METH_FASTCALL | METH_KEYWORDS | METH_METHOD) == 0
                && let Some(raw_func) = unsafe { (*ml).ml_meth }
            {
                return unsafe { raw_func((*cfunc).m_self, self_) };
            }
        }
    }
    let bound_args = match unsafe { prepend_bound_self(self_, args) } {
        Some(bound_args) => bound_args,
        None => return ptr::null_mut(),
    };
    let result = unsafe { PyObject_Call(func, bound_args, kwargs) };
    unsafe { crate::api::refcount::Py_DECREF(bound_args) };
    result
}

pub unsafe extern "C" fn molt_method_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let method = op.cast::<PyMethodObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*method).im_func);
        crate::api::refcount::Py_XDECREF((*method).im_self);
        drop(Box::from_raw(method));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_New(
    ml: *mut PyMethodDef,
    self_: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyCFunction_NewEx(ml, self_, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_NewEx(
    ml: *mut PyMethodDef,
    self_: *mut PyObject,
    module: *mut PyObject,
) -> *mut PyObject {
    if ml.is_null() || unsafe { (*ml).ml_meth }.is_none() {
        return ptr::null_mut();
    }
    unsafe {
        crate::api::refcount::Py_XINCREF(self_);
        crate::api::refcount::Py_XINCREF(module);
    }
    let obj = Box::new(PyCFunctionObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyCFunction_Type,
        },
        m_ml: ml,
        m_self: self_,
        m_module: module,
        m_weakreflist: ptr::null_mut(),
        vectorcall: None,
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMethod_New(func: *mut PyObject, self_: *mut PyObject) -> *mut PyObject {
    if func.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        crate::api::refcount::Py_INCREF(func);
        crate::api::refcount::Py_XINCREF(self_);
    }
    let obj = Box::new(PyMethodObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyMethod_Type,
        },
        im_func: func,
        im_self: self_,
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMethod_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    std::ptr::eq(
        unsafe { (*op).ob_type },
        &raw mut crate::abi_types::PyMethod_Type,
    ) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMethod_GET_FUNCTION(op: *mut PyObject) -> *mut PyObject {
    if unsafe { PyMethod_Check(op) } == 0 {
        return ptr::null_mut();
    }
    let method = op.cast::<PyMethodObject>();
    unsafe { (*method).im_func }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMethod_GET_SELF(op: *mut PyObject) -> *mut PyObject {
    if unsafe { PyMethod_Check(op) } == 0 {
        return ptr::null_mut();
    }
    let method = op.cast::<PyMethodObject>();
    unsafe { (*method).im_self }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    std::ptr::eq(
        unsafe { (*op).ob_type },
        &raw mut crate::abi_types::PyCFunction_Type,
    ) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_GetFunction(op: *mut PyObject) -> Option<PyCFunction> {
    if unsafe { PyCFunction_Check(op) } == 0 {
        return None;
    }
    let func = op.cast::<PyCFunctionObject>();
    if func.is_null() || unsafe { (*func).m_ml.is_null() } {
        return None;
    }
    unsafe { (*(*func).m_ml).ml_meth }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_GetSelf(op: *mut PyObject) -> *mut PyObject {
    if unsafe { PyCFunction_Check(op) } == 0 {
        return ptr::null_mut();
    }
    let func = op.cast::<PyCFunctionObject>();
    if func.is_null() {
        ptr::null_mut()
    } else {
        unsafe { (*func).m_self }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCFunction_GetFlags(op: *mut PyObject) -> c_int {
    if unsafe { PyCFunction_Check(op) } == 0 {
        return 0;
    }
    let func = op.cast::<PyCFunctionObject>();
    if func.is_null() || unsafe { (*func).m_ml.is_null() } {
        0
    } else {
        unsafe { (*(*func).m_ml).ml_flags }
    }
}

static mut MOLT_INTERPRETER_STATE: PyInterpreterState = PyInterpreterState { _molt_reserved: 0 };
static mut MOLT_ERR_STACK_ITEM: _PyErr_StackItem = _PyErr_StackItem {
    exc_type: ptr::null_mut(),
    exc_value: ptr::null_mut(),
    exc_traceback: ptr::null_mut(),
    previous_item: ptr::null_mut(),
};
static mut MOLT_THREAD_STATE: PyThreadState = PyThreadState {
    interp: &raw mut MOLT_INTERPRETER_STATE,
    current_exception: ptr::null_mut(),
    exc_info: &raw mut MOLT_ERR_STACK_ITEM,
    exc_state: _PyErr_StackItem {
        exc_type: ptr::null_mut(),
        exc_value: ptr::null_mut(),
        exc_traceback: ptr::null_mut(),
        previous_item: ptr::null_mut(),
    },
    _molt_reserved: 0,
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_Get() -> *mut PyThreadState {
    &raw mut MOLT_THREAD_STATE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IsInitialized() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGILState_Ensure() -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGILState_Release(_state: c_int) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGILState_Check() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMutex_Lock(mutex: *mut PyMutex) {
    if mutex.is_null() {
        return;
    }
    let lock = unsafe { &*((&raw mut (*mutex)._bits).cast::<AtomicUsize>()) };
    while lock
        .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMutex_Unlock(mutex: *mut PyMutex) {
    if mutex.is_null() {
        return;
    }
    let lock = unsafe { &*((&raw mut (*mutex)._bits).cast::<AtomicUsize>()) };
    lock.store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyThreadState_UncheckedGet() -> *mut PyThreadState {
    &raw mut MOLT_THREAD_STATE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_SaveThread() -> *mut PyThreadState {
    &raw mut MOLT_THREAD_STATE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_RestoreThread(_tstate: *mut PyThreadState) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInterpreterState_Get() -> *mut PyInterpreterState {
    &raw mut MOLT_INTERPRETER_STATE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInterpreterState_Main() -> *mut PyInterpreterState {
    &raw mut MOLT_INTERPRETER_STATE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_GetInterpreter(
    tstate: *mut PyThreadState,
) -> *mut PyInterpreterState {
    if tstate.is_null() {
        &raw mut MOLT_INTERPRETER_STATE
    } else {
        let interp = unsafe { (*tstate).interp };
        if interp.is_null() {
            &raw mut MOLT_INTERPRETER_STATE
        } else {
            interp
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_GetID(_tstate: *mut PyThreadState) -> u64 {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInterpreterState_GetID(_interp: *mut PyInterpreterState) -> i64 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInterpreterState_GetIDFromThreadState(
    _tstate: *mut PyThreadState,
) -> i64 {
    0
}

/// _Py_NoneStruct — alias for Py_None, used by some extensions.
#[unsafe(no_mangle)]
pub static mut _Py_NoneStruct: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

/// _Py_TrueStruct — alias for Py_True.
#[unsafe(no_mangle)]
pub static mut _Py_TrueStruct: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

/// _Py_FalseStruct — alias for Py_False.
#[unsafe(no_mangle)]
pub static mut _Py_FalseStruct: PyObject = PyObject {
    ob_refcnt: 1 << 30,
    ob_type: std::ptr::null_mut(),
};

// ─── Comparison constants ─────────────────────────────────────────────────

pub const PY_LT: c_int = 0;
pub const PY_LE: c_int = 1;
pub const PY_EQ: c_int = 2;
pub const PY_NE: c_int = 3;
pub const PY_GT: c_int = 4;
pub const PY_GE: c_int = 5;

/// Exported comparison constants for C extensions.
#[unsafe(no_mangle)]
pub static Py_LT: c_int = 0;
#[unsafe(no_mangle)]
pub static Py_LE: c_int = 1;
#[unsafe(no_mangle)]
pub static Py_EQ: c_int = 2;
#[unsafe(no_mangle)]
pub static Py_NE: c_int = 3;
#[unsafe(no_mangle)]
pub static Py_GT: c_int = 4;
#[unsafe(no_mangle)]
pub static Py_GE: c_int = 5;

// ─── PyObject_Bytes / PyObject_ASCII ──────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Bytes(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    // If it's already bytes, return it.
    if unsafe { crate::api::strings::PyBytes_Check(o) } != 0 {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    // Otherwise return b'' placeholder.
    unsafe { crate::api::strings::PyBytes_FromStringAndSize(ptr::null(), 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_ASCII(o: *mut PyObject) -> *mut PyObject {
    // For now, same as repr.
    unsafe { crate::api::typeobj::PyObject_Repr(o) }
}
