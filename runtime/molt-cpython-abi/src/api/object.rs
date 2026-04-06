//! Object protocol — PyObject_* generic operations.
//!
//! These are the abstract object protocol functions that work on any
//! PyObject regardless of type. They delegate to type-specific slots
//! (tp_repr, tp_hash, tp_getattro, etc.) when available, falling back
//! to reasonable defaults.

use crate::abi_types::{Py_False, Py_None, Py_True, Py_ssize_t, PyObject, PyTypeObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::{c_char, c_int};
use std::ptr;

// ─── Attribute access ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetAttr(
    o: *mut PyObject,
    attr_name: *mut PyObject,
) -> *mut PyObject {
    if o.is_null() || attr_name.is_null() {
        return ptr::null_mut();
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
pub unsafe extern "C" fn PyObject_GenericSetAttr(
    o: *mut PyObject,
    name: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    if o.is_null() || name.is_null() {
        return -1;
    }
    // Without instance dict support, this is a no-op that succeeds silently.
    let _ = (o, name, value);
    0
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
